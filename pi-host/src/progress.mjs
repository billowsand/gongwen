//! 分阶段成果登记与依赖失效。
//!
//! 登记文件写在运行目录（工作区之外，模型没有可写路径能碰到）：`progress.json`。
//! 宿主只相信**磁盘上实际存在**的文件、实际算出的哈希，以及能在原文里定位到的
//! 引文；模型填的「已完成」、路径与偏移一律不采信。登记是幂等的：同一阶段同一单元
//! 重复提交同一份成果不会产生第二份记录。

import path from "node:path";
import { createHash } from "node:crypto";
import { plaintextFiles } from "./encrypted-files.mjs";

export const PROGRESS_VERSION = 1;
export const MAX_RECORDS = 400;
const MAX_SOURCES = 8;
const MAX_USES = MAX_RECORDS;
export const MAX_QUOTE_CHARS = 2_000;
const MAX_SUMMARY_CHARS = 2_000;
const MAX_UNIT_CHARS = 200;
const MAX_SECTIONS = 12;
const MAX_SECTION_TITLE = 120;

export class ProgressError extends Error {
  constructor(message, code = "progress_invalid") {
    super(message);
    this.name = "ProgressError";
    this.code = code;
  }
}

function sha256(text) {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

function clamp(text, max) {
  const value = typeof text === "string" ? text : String(text ?? "");
  return value.length > max ? value.slice(0, max) : value;
}

function nowIso() {
  return new Date().toISOString();
}

function readTextFile(file, maxBytes, allowEmpty = false, files = plaintextFiles) {
  let stat;
  try {
    stat = files.info(file);
  } catch (error) {
    throw new ProgressError(
      error?.code === "ENOENT" ? "成果文件还没有写出来" : `读不到文件：${error.message}`,
    );
  }
  if (stat.kind !== "file") throw new ProgressError("登记路径不是普通文件");
  if (stat.size === 0 && !allowEmpty) throw new ProgressError("成果文件是空的");
  if (stat.size > maxBytes) throw new ProgressError(`文件超过单文件上限（${stat.size} 字节）`);
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(files.readBytes(file, maxBytes));
    if (text.trim() === "" && !allowEmpty) throw new ProgressError("成果文件只有空白");
    return text;
  } catch (error) {
    if (error instanceof ProgressError) throw error;
    throw new ProgressError(`文件不是合法 UTF-8：${error.message}`);
  }
}

/**
 * 运行身份：正文、要求、材料与选区的实际哈希。恢复时用来确认登记属于同一份输入，
 * 不把新正文混进旧任务的阶段成果。
 */
export function identityOf({ guard, workspace, task }) {
  const current = guard.read(workspace.current, guard.root, 8 * 1024 * 1024);
  const request = guard.read(workspace.request, guard.root, 8 * 1024 * 1024);
  const texts = {
    current: current.ok ? readTextFile(current.value, 8 * 1024 * 1024, true, guard.files) : "",
    request: request.ok ? readTextFile(request.value, 8 * 1024 * 1024, true, guard.files) : "",
  };
  const materials = (task.materials ?? []).map((material) => ({
    file: material.file,
    sha256: (() => {
      const resolved = guard.read(material.file, guard.root, 8 * 1024 * 1024);
      if (!resolved.ok) throw new ProgressError(`材料身份无法核验：${material.file}`);
      return sha256(readTextFile(resolved.value, 8 * 1024 * 1024, true, guard.files));
    })(),
    role: material.role,
  }));
  return {
    currentSha256: sha256(texts.current),
    requestSha256: sha256(texts.request),
    selectionSha256: sha256(task.selection?.text ?? ""),
    materials,
  };
}

function materialIndex(task) {
  const map = new Map();
  for (const material of task.materials ?? []) {
    map.set(material.file, material);
  }
  return map;
}

export class ProgressRegistry {
  constructor({ deliveryDir, runId, requestId, identity, maxBytes = 8 * 1024 * 1024, files = plaintextFiles }) {
    this.file = path.join(deliveryDir, "progress.json");
    this.files = files;
    this.runId = runId;
    this.requestId = requestId;
    this.identity = identity;
    this.maxBytes = maxBytes;
    this.records = [];
    this.invalidated = [];
    this.phases = {};
    this.cursor = undefined;
    this.plan = undefined;
    this.planningBudget = undefined;
    this.answers = [];
    this.evidence = [];
  }

  /** 读取旧登记；不存在就是新任务。身份不一致直接拒绝恢复。 */
  static load(options) {
    const registry = new ProgressRegistry(options);
    let raw;
    try {
      if (registry.files.info(registry.file).size > registry.maxBytes) throw new ProgressError("阶段成果登记超过大小上限", "progress_too_large");
      raw = registry.files.readText(registry.file, registry.maxBytes);
    } catch (error) {
      if (error?.code === "ENOENT") return registry;
      if (error instanceof ProgressError) throw error;
      throw new ProgressError(`读不到阶段成果登记：${error.message}`, "progress_unreadable");
    }
    if (Buffer.byteLength(raw, "utf8") > registry.maxBytes) {
      throw new ProgressError("阶段成果登记超过大小上限", "progress_too_large");
    }
    let value;
    try {
      value = JSON.parse(raw);
    } catch (error) {
      throw new ProgressError(`阶段成果登记损坏：${error.message}`, "progress_corrupt");
    }
    if (value?.version !== PROGRESS_VERSION) {
      throw new ProgressError("阶段成果登记版本不认识，已拒绝", "progress_version");
    }
    if (value.runId !== registry.runId) {
      throw new ProgressError("阶段成果登记属于另一轮运行，已拒绝", "progress_run_mismatch");
    }
    if (value.requestId !== registry.requestId) {
      throw new ProgressError("阶段成果登记属于另一份输入，已拒绝混入本轮", "progress_identity_mismatch");
    }
    if (!sameIdentity(value.identity, registry.identity)) {
      throw new ProgressError(
        "本轮正文、要求或材料与上次登记不一致，不能沿用旧阶段成果。",
        "progress_identity_mismatch",
      );
    }
    registry.records = Array.isArray(value.records) ? value.records : [];
    registry.invalidated = Array.isArray(value.invalidated) ? value.invalidated : [];
    registry.phases = value.phases && typeof value.phases === "object" ? value.phases : {};
    registry.plan = value.plan;
    registry.planningBudget = value.planningBudget;
    registry.cursor = value.cursor;
    registry.answers = Array.isArray(value.answers) ? value.answers.slice(-6) : [];
    registry.evidence = Array.isArray(value.evidence) ? value.evidence.slice(-200) : [];
    return registry;
  }

  active() {
    return this.records.filter((record) => !record.invalid);
  }

  get(unit) {
    return this.active().find((record) => record.unit === unit);
  }

  has(unit) {
    return Boolean(this.get(unit));
  }

  phaseRecords(phase) {
    return this.active().filter((record) => record.phase === phase);
  }

  outlineSections() {
    const record = this.get("outline");
    return Array.isArray(record?.sections) ? record.sections : [];
  }

  /** 把文件被改、来源被换、依赖失效的登记改成 invalid，并返回原因。 */
  refresh({ guard, workspace, task }) {
    for (const evidence of this.evidence) {
      const file = guard.read(evidence.path, guard.root, 1024 * 1024);
      if (!file.ok || sha256(readTextFile(file.value, 1024 * 1024, false, guard.files)) !== evidence.sha256) {
        throw new ProgressError("知识库/稿件库工具快照已被改动，不能作为原工具返回继续使用", "progress_identity_mismatch");
      }
    }
    const materials = materialIndex(task);
    const changed = [];
    for (const record of this.records) {
      if (record.invalid) continue;
      const reason = this.check(record, { guard, workspace, materials });
      if (reason) {
        record.invalid = true;
        record.invalidReason = reason;
        record.invalidAt = nowIso();
        this.invalidated.push({ unit: record.unit, reason, at: record.invalidAt });
        changed.push({ unit: record.unit, reason });
      }
    }
    // 依赖失效会传递：被依赖的单元失效，依赖它的章节与合稿也不能再用。
    let grew = true;
    while (grew) {
      grew = false;
      for (const record of this.records) {
        if (record.invalid) continue;
        const dead = (record.uses ?? []).find((unit) => {
          return !this.has(unit);
        });
        if (dead) {
          record.invalid = true;
          record.invalidReason = `依赖的阶段成果已失效：${dead}`;
          record.invalidAt = nowIso();
          this.invalidated.push({ unit: record.unit, reason: record.invalidReason, at: record.invalidAt });
          changed.push({ unit: record.unit, reason: record.invalidReason });
          grew = true;
        }
      }
    }
    if (changed.length > 0) this.persist();
    return changed;
  }

  check(record, { guard, workspace, materials }) {
    const resolved = guard.read(record.path, guard.root, this.maxBytes);
    if (!resolved.ok) return resolved.error?.message ?? "成果文件已不在工作区";
    let stat;
    try {
      stat = guard.files.info(resolved.value);
    } catch {
      return "成果文件已被删除";
    }
    if (stat.kind !== "file") return "成果路径已不是普通文件";
    if (stat.size !== record.bytes) return "成果文件已被改动（字节数不同）";
    try {
      const text = guard.files.readText(resolved.value, this.maxBytes);
      if (sha256(text) !== record.sha256) return "成果文件已被改动（哈希不同）";
    } catch {
      return "成果文件读不出来";
    }
    for (const source of record.sources ?? []) {
      const sourcePath = guard.read(source.path, guard.root, this.maxBytes);
      if (!sourcePath.ok) return `依据文件已不可读：${source.path}`;
      let text;
      try {
        text = guard.files.readText(sourcePath.value, this.maxBytes);
      } catch {
        return `依据文件读不出来：${source.path}`;
      }
      if (sha256(text) !== source.sha256) return `依据文件已被替换：${source.path}`;
      if (!text.slice(source.start, source.end).includes(source.quote)) {
        return `依据位置已对不上：${source.path}`;
      }
    }
    // 材料角色以本轮索引为准，模型不能在登记里改角色。
    for (const unit of record.uses ?? []) {
      if (!this.records.some((item) => item.unit === unit && !item.invalid)) {
        return `依赖的阶段成果不存在或已失效：${unit}`;
      }
    }
    void workspace;
    void materials;
    return undefined;
  }

  /**
   * 登记一个阶段成果。`spec` 是当前阶段的单元定义（来自宿主计划，不是模型给的）。
   */
  record({ spec, phase, unit, file, summary, sources, uses, sections }, { guard, workspace, task, maxFileBytes }) {
    if (!spec || spec.id !== unit) {
      throw new ProgressError(`本轮这一步不接受单位「${unit}」；请按提示里的单位编号登记`);
    }
    if (phase !== spec.phase) {
      throw new ProgressError(`当前阶段是 ${spec.phase}，不能登记到 ${phase}`);
    }
    const resolved = guard.write(file, guard.root, maxFileBytes);
    if (!resolved.ok) throw new ProgressError(resolved.error?.message ?? "成果路径不可写");
    const relative = path.relative(guard.root, resolved.value).replaceAll("\\", "/");
    if (!relative.startsWith("work/")) {
      throw new ProgressError("阶段成果只能放在 work/ 下");
    }
    const text = readTextFile(resolved.value, maxFileBytes, false, guard.files);
    const hash = sha256(text);
    const materials = task.materials ?? [];
    const byFile = new Map(materials.map((material) => [material.file, material]));

    const normalizedSources = [];
    for (const source of Array.isArray(sources) ? sources.slice(0, MAX_SOURCES) : []) {
      const sourcePath = typeof source?.path === "string" ? source.path : "";
      let quote = typeof source?.quote === "string" ? source.quote : "";
      if (!sourcePath.startsWith("input/")) {
        throw new ProgressError("依据只能引用本轮 input/ 下的原文件，不能引用你自己的工作稿");
      }
      const opened = guard.read(sourcePath, guard.root, maxFileBytes);
      if (!opened.ok) throw new ProgressError(`依据文件读不到：${sourcePath}`);
      let sourceText;
      try {
        sourceText = readTextFile(opened.value, maxFileBytes, true, guard.files);
      } catch (error) {
        throw new ProgressError(`依据文件读不出来：${error.message}`);
      }
      let start;
      if (source?.fromLine !== undefined || source?.toLine !== undefined) {
        if (quote !== "") throw new ProgressError("依据定位只能选行号或 quote，不要同时填写");
        const from = source.fromLine;
        const to = source.toLine ?? from;
        const lines = sourceText.split("\n");
        if (!Number.isSafeInteger(from) || !Number.isSafeInteger(to) || from < 1 || to < from || to > lines.length) {
          throw new ProgressError(`依据行号越界：使用原文件 1–${lines.length} 行内的 fromLine/toLine`);
        }
        start = lines.slice(0, from - 1).reduce((offset, line) => offset + line.length + 1, 0);
        // 提取原文后仍核对材料角色、块范围、哈希和精确位置。
        quote = lines.slice(from - 1, to).join("\n");
      } else {
        start = sourceText.indexOf(quote, spec.kind === "distill" ? spec.start ?? 0 : 0);
      }
      if (quote.trim().length < 2) throw new ProgressError("每条依据都要带上有效原文行号或可回查的原文摘录");
      if (quote.length > MAX_QUOTE_CHARS) throw new ProgressError(`原文摘录超过 ${MAX_QUOTE_CHARS} 字符，请缩小行号范围或用短 quote`);
      if (start < 0) {
        throw new ProgressError(`摘录在原文里找不到：${sourcePath}（${quote.slice(0, 40)}…）`);
      }
      const material = byFile.get(sourcePath);
      if (material && material.role !== "fact" && material.role !== "incoming_letter") {
        const roleLabel = material.role === "style_sample" ? "写法样稿" : "结构范文";
        if (spec.kind !== "study") {
          throw new ProgressError(`${roleLabel}不能登记为事实摘录（${sourcePath}）`);
        }
      }
      if (spec.kind === "distill" && (!material || !["fact", "incoming_letter"].includes(material.role))) {
        throw new ProgressError(`这一块不是事实材料，不能作为事实摘录：${sourcePath}`);
      }
      if (spec.kind === "distill" && material && sourcePath !== spec.file) {
        throw new ProgressError(`这一块的材料是 ${spec.file}，不能拿别的文件代替`);
      }
      if (spec.kind === "distill" && Number.isInteger(spec.start) && Number.isInteger(spec.end)) {
        if (start < spec.start || start + quote.length > spec.end) {
          throw new ProgressError(
            `摘录不在本块范围（第 ${spec.start}–${spec.end} 字）。请只登记这一块里的事实。`,
          );
        }
      }
      normalizedSources.push({
        path: sourcePath,
        role: material?.role ?? "user",
        sha256: sha256(sourceText),
        start,
        end: start + quote.length,
        quote,
      });
    }

    if (spec.kind === "distill" && normalizedSources.length === 0) {
      throw new ProgressError("材料提炼成果至少要有 1 条能在原文定位的依据");
    }
    if (spec.kind === "study" && normalizedSources.length === 0) {
      throw new ProgressError("写法 / 结构小结也要至少 1 条原文摘录，方便回查");
    }

    const normalizedUses = [];
    for (const use of Array.isArray(uses) ? uses.slice(0, MAX_USES) : []) {
      const value = typeof use === "string" ? use : "";
      if (value === "") continue;
      if (value === unit) throw new ProgressError("阶段成果不能依赖自己");
      if (!this.has(value)) {
        const available = this.active().map((record) => record.unit).join("、") || "（当前还没有已登记成果）";
        throw new ProgressError(`依赖编号「${value}」不存在或尚未登记。当前可用的已登记编号：${available}`);
      }
      normalizedUses.push(value);
    }
    for (const required of spec.uses ?? []) {
      if (!this.has(required)) {
        throw new ProgressError(`这一步依赖的阶段成果还没有登记：${required}`);
      }
      if (!normalizedUses.includes(required)) normalizedUses.push(required);
    }

    let normalizedSections;
    if (spec.kind === "outline") {
      if (!Array.isArray(sections) || sections.length > MAX_SECTIONS) {
        throw new ProgressError(`提纲至少一节、最多 ${MAX_SECTIONS} 节，超出时请先合并，不能截掉后续章节`);
      }
      normalizedSections = [];
      for (const section of Array.isArray(sections) ? sections.slice(0, MAX_SECTIONS) : []) {
        const title = clamp(typeof section === "string" ? section : section?.title, MAX_SECTION_TITLE);
        if (title.trim() === "") continue;
        if (!text.includes(title.trim())) {
          throw new ProgressError(`提纲里找不到章节「${title}」；请先写进成果文件再登记`);
        }
        normalizedSections.push(title.trim());
      }
      if (normalizedSections.length === 0) {
        throw new ProgressError("提纲登记要列出章节标题（sections），至少一节");
      }
      if (new Set(normalizedSections).size !== normalizedSections.length) throw new ProgressError("提纲章节标题重复，请使用能区分的标题");
    }

    const existing = this.get(unit);
    if (
      existing &&
      existing.sha256 === hash &&
      existing.path === relative &&
      JSON.stringify(existing.uses ?? []) === JSON.stringify(normalizedUses) &&
      JSON.stringify(existing.sections ?? []) === JSON.stringify(normalizedSections ?? []) &&
      (existing.sources ?? []).length === normalizedSources.length &&
      normalizedSources.every((source, index) => {
        const old = existing.sources[index];
        return old?.path === source.path && old?.sha256 === source.sha256 && old?.quote === source.quote;
      })
    ) {
      return { status: "already", record: existing };
    }

    // 同一单元重新登记 = 新版本；依赖它的章节与合稿必须失效重做。
    if (existing) this.invalidateDependents(unit, "成果被新版本替换");

    const record = {
      phase: spec.phase,
      unit,
      kind: spec.kind,
      title: spec.title ?? "",
      path: relative,
      sha256: hash,
      bytes: Buffer.byteLength(text, "utf8"),
      summary: clamp(summary, MAX_SUMMARY_CHARS),
      sources: normalizedSources,
      uses: normalizedUses,
      ...(normalizedSections ? { sections: normalizedSections } : {}),
      attempt: Number.isInteger(spec.attempt) ? spec.attempt : 1,
      at: nowIso(),
    };
    this.records.push(record);
    if (this.records.length > MAX_RECORDS) {
      throw new ProgressError("阶段成果登记条数超过上限，请合并重复文件", "progress_too_many");
    }
    this.persist();
    void workspace;
    return { status: "recorded", record };
  }

  invalidateDependents(unit, reason) {
    const queue = [unit];
    const seen = new Set();
    while (queue.length > 0) {
      const current = queue.shift();
      if (seen.has(current)) continue;
      seen.add(current);
      for (const record of this.records) {
        if (record.invalid) continue;
        if (record.unit === current || (record.uses ?? []).includes(current)) {
          record.invalid = true;
          record.invalidReason = reason;
          record.invalidAt = nowIso();
          this.invalidated.push({ unit: record.unit, reason, at: record.invalidAt });
          queue.push(record.unit);
        }
      }
    }
  }

  markPhase(phase, attempts) {
    this.phases[phase] = { attempts, at: nowIso() };
    this.cursor = undefined;
    this.persist();
  }

  /** 只缓存真实知识/稿件工具返回，风格、词库和模型自己的文字不能进入事实资料。 */
  cacheEvidence(name, value, guard) {
    if (!["kb_search", "kb_read", "ms_search", "ms_read", "wiki_search", "wiki_read", "wiki_doc_read"].includes(name)) return;
    const text = `# ${name} 工具实际返回\n\n${JSON.stringify(value, null, 2)}\n`;
    if (Buffer.byteLength(text, "utf8") > 1024 * 1024) throw new ProgressError("工具资料超过快照上限，请缩小检索或分页读取");
    const hash = sha256(text);
    if (this.evidence.some((record) => record.sha256 === hash)) return;
    if (this.evidence.length >= 200) throw new ProgressError("本轮工具资料快照达到上限，请缩小任务");
    const relative = `input/evidence/${hash}.md`;
    const resolved = guard.read(relative, guard.root, 1024 * 1024);
    if (!resolved.ok) throw new ProgressError("工具资料快照路径不安全");
    guard.files.atomicWrite(resolved.value, Buffer.from(text, "utf8"));
    this.evidence.push({ path: relative, name, sha256: hash });
    this.persist();
  }

  /** 阶段是否完成：每个预期单元都有有效登记。 */
  status(phase, expectedUnits) {
    const done = expectedUnits.filter((unit) => this.has(unit.id));
    const missing = expectedUnits.filter((unit) => !this.has(unit.id));
    return { ok: missing.length === 0, done: done.length, total: expectedUnits.length, missing };
  }

  persist() {
    const value = {
      version: PROGRESS_VERSION,
      runId: this.runId,
      requestId: this.requestId,
      identity: this.identity,
      plan: this.plan,
      planningBudget: this.planningBudget,
      records: this.records,
      invalidated: this.invalidated.slice(-200),
      phases: this.phases,
      cursor: this.cursor,
      answers: this.answers,
      evidence: this.evidence,
      at: nowIso(),
    };
    const bytes = Buffer.from(JSON.stringify(value, null, 2), "utf8");
    if (bytes.length > this.maxBytes) {
      throw new ProgressError("阶段成果登记超过大小上限", "progress_too_large");
    }
    this.files.atomicWrite(this.file, bytes);
  }
}

function sameIdentity(left, right) {
  if (!left || typeof left !== "object") return false;
  if (left.currentSha256 !== right.currentSha256) return false;
  if (left.requestSha256 !== right.requestSha256) return false;
  if ((left.selectionSha256 ?? "") !== (right.selectionSha256 ?? "")) return false;
  const leftMaterials = Array.isArray(left.materials) ? left.materials : [];
  const rightMaterials = Array.isArray(right.materials) ? right.materials : [];
  if (leftMaterials.length !== rightMaterials.length) return false;
  return rightMaterials.every((material, index) => {
    const old = leftMaterials[index];
    return old?.file === material.file && old?.sha256 === material.sha256 && old?.role === material.role;
  });
}
