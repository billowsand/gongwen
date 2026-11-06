//! 固定薄宿主：任务类型 → 阶段计划 → 每步短指令。
//!
//! 这里没有通用流程 DSL、没有分支图，也不替模型写作。阶段只有固定的几种，按任务
//! 裁剪；每一步都由同一套 Durable Harness 执行，中间成果只能经 `record_progress`
//! 由宿主核验后登记，最终仍必须 `submit_document` / `finish_report`。

import { chunkChars } from "./budget.mjs";
import { MAX_QUOTE_CHARS, ProgressError } from "./progress.mjs";
import { isStyleTask } from "./style.mjs";
import { writingContextBlock } from "./prompt.mjs";

export const RECORD_TOOL_NAME = "record_progress";

const MAX_PHASES = 20;
const MAX_SECTION_PHASES = 12;

/** 恢复只用原计划；扩大窗口不能把旧 #1 摘记误认为覆盖更大的新 #1。 */
export function recoverPlan(plan, decision, registry) {
  if (registry.plan?.phases?.length) {
    if (!Number.isFinite(registry.planningBudget) || decision.materialsBudget < registry.planningBudget) {
      throw new ProgressError("当前可用窗口小于原任务规划，不能改变分块后复用旧成果；请恢复原窗口或从头写。", "progress_plan_changed");
    }
    plan = registry.plan;
    decision = { ...decision, mode: "phased", reasons: [...decision.reasons, "沿用原任务分块计划"] };
  } else if (registry.records.length > 0 || registry.cursor?.phased) {
    throw new ProgressError("旧任务没有保存完整分块计划，不能安全接续；请从头写。", "progress_plan_missing");
  }
  registry.plan = plan;
  registry.planningBudget ??= decision.materialsBudget;
  registry.persist();
  return { plan, decision };
}

/** 给模型一条处于当前块内的完整非空行，避免让它重抄日期和数字。 */
function firstSourceLine(text, start, end) {
  let offset = start;
  if (start > 0 && text[start - 1] !== "\n") {
    const newline = text.indexOf("\n", start);
    if (newline < 0) return undefined;
    offset = newline + 1;
  }
  while (offset < end) {
    const newline = text.indexOf("\n", offset);
    const finish = newline < 0 ? text.length : newline;
    if (finish > end) return undefined;
    const line = text.slice(offset, finish);
    if (line.trim().length >= 2 && line.length <= MAX_QUOTE_CHARS) {
      return text.slice(0, offset).split("\n").length;
    }
    if (newline < 0) break;
    offset = newline + 1;
  }
  return undefined;
}

/** 保持整篇；超限优先在后半段的章节边界切，再选段落，最后才按字符切。 */
export function materialRanges(text, maxChars) {
  const ranges = [];
  let start = 0;
  while (start < text.length) {
    let end = Math.min(text.length, start + maxChars);
    if (end < text.length) {
      const floor = start + Math.floor(maxChars * 0.6);
      const slice = text.slice(floor, end);
      const headings = [...slice.matchAll(/\n(?=#{1,6}\s)/gu)];
      const chapter = headings.at(-1);
      const paragraph = slice.lastIndexOf("\n\n");
      if (chapter) end = floor + chapter.index + 1;
      else if (paragraph >= 0) end = floor + paragraph + 2;
      if (/[\uD800-\uDBFF]/u.test(text[end - 1]) && /[\uDC00-\uDFFF]/u.test(text[end])) end -= 1;
    }
    ranges.push([start, end]);
    start = end;
  }
  return ranges.length ? ranges : [[0, 0]];
}

/** 读材料原文用于分块（材料总量已由 Rust 侧限制在 4 MiB 内）。 */
function readMaterial(guard, file, maxFileBytes) {
  const resolved = guard.read(file, guard.root, maxFileBytes);
  if (!resolved.ok) throw new Error(`读不到材料 ${file}：${resolved.error?.message ?? "未知错误"}`);
  return guard.files.readText(resolved.value, maxFileBytes);
}

function roleLabel(role) {
  return {
    fact: "事实材料",
    incoming_letter: "来函",
    style_sample: "写法样稿",
    structure_sample: "结构范文",
  }[role] ?? "材料";
}

/**
 * 生成阶段计划。`decision.mode` 为 `regular` / `insufficient` 时返回 `[]`。
 *
 * 文档任务：提炼（有材料时）→ 提纲 → 逐节写作 → 合稿检查 → 交付。
 * 报告 / 提炼任务：提炼（有材料时）→ 交付。
 * 局部修改：只有交付。
 */
export function buildPlan({ config, decision, guard }) {
  if (decision.mode !== "phased") return { phases: [], sections: [], materials: [] };
  const task = config.task ?? {};
  const materials = task.materials ?? [];
  const estimate = decision.estimate;
  if (estimate.selection) return { phases: [], sections: [], materials: [] };

  const limit = Math.max(1, config.limits.maxFileBytes);
  const chunkSize = chunkChars(decision);
  const chunks = [];
  const studies = [];
  for (const material of materials) {
    const text = readMaterial(guard, material.file, limit);
    if (material.role === "fact" || material.role === "incoming_letter") {
      const ranges = materialRanges(text, chunkSize);
      for (const [index, [start, end]] of ranges.entries()) {
        chunks.push({
          phase: "distill", id: `distill:${material.file}#${index + 1}`,
          unit: `distill:${material.file}#${index + 1}`, kind: "distill", file: material.file,
          title: material.title, role: material.role, start, end,
          sourceLine: firstSourceLine(text, start, end), chars: end - start,
        });
      }
    } else {
      studies.push({
        phase: "distill",
        id: `study:${material.file}`,
        unit: `study:${material.file}`,
        kind: "study",
        file: material.file,
        title: material.title,
        role: material.role,
        chars: text.length,
        sourceLine: firstSourceLine(text, 0, text.length),
      });
    }
  }

  const phases = [];
  if (chunks.length + studies.length > 0) {
    phases.push({ id: "distill", title: "提炼材料", units: [...chunks, ...studies] });
  }
  if (estimate.report) {
    phases.push({ id: "deliver", title: "交付清单", final: true, report: true });
  } else {
    phases.push({
      id: "outline",
      title: "写提纲",
      units: [
        {
          phase: "outline",
          id: "outline",
          unit: "outline",
          kind: "outline",
          title: "写作提纲",
        },
      ],
    });
    phases.push({ id: "sections", title: "逐节写作", dynamic: "sections", units: [] });
    phases.push({
      id: "review",
      title: "合稿检查",
      units: [
        {
          phase: "review",
          id: "final",
          unit: "final",
          kind: "final",
          title: "完整候选稿",
          uses: ["outline"],
        },
      ],
    });
    phases.push({ id: "deliver", title: "交付正文", final: true });
  }
  return { phases, sections: chunks, materials };
}

/** 提纲登记后按章节展开逐节阶段；没有提纲时退成一节，仍走同一登记与检查。 */
export function expandSections(plan, registry) {
  const index = plan.phases.findIndex((phase) => phase.dynamic === "sections");
  if (index < 0) return [];
  const titles = registry.outlineSections();
  if (titles.length > MAX_SECTION_PHASES) throw new Error(`提纲超过 ${MAX_SECTION_PHASES} 节，请合并章节后重新登记，不能丢弃后续章节`);
  const sections = titles.length > 0 ? titles : ["全文"];
  const phases = sections.map((title, offset) => ({
    id: `section-${offset + 1}`,
    title: `写第 ${offset + 1}/${sections.length} 节`,
    section: { index: offset + 1, total: sections.length, title },
    units: [
      {
        phase: "sections",
        id: `section:${offset + 1}`,
        unit: `section:${offset + 1}`,
        kind: "section",
        title,
        index: offset + 1,
        total: sections.length,
        uses: ["outline"],
      },
    ],
  }));
  plan.phases.splice(index, 1, ...phases);
  if (plan.phases.length > MAX_PHASES) {
    throw new Error(`阶段数超过上限（${plan.phases.length}），请缩小材料或章节数`);
  }
  return phases;
}

/** 当前阶段要核验的单元。逐节阶段按提纲展开，合稿阶段要求引用全部章节。 */
export function expectedUnits(phase, registry) {
  if (phase.final) {
    return phase.report ? [] : [{ id: "final", kind: "final" }];
  }
  if (phase.dynamic === "sections") {
    const sections = registry.outlineSections();
    return (sections.length > 0 ? sections : ["全文"]).map((title, offset) => ({
      id: `section:${offset + 1}`,
      kind: "section",
      title,
    }));
  }
  return phase.units ?? [];
}

/** 依赖完整性：合稿必须引用全部章节，章节引用提纲，提纲引用全部材料成果。 */
export function missingUses(phase, registry) {
  const missing = [];
  if (phase.id === "review") {
    const record = registry.get("final");
    if (record) {
      if (!(record.uses ?? []).includes("outline")) missing.push("outline");
      const sectionUnits = registry
        .active()
        .filter((item) => item.kind === "section")
        .map((item) => item.unit);
      for (const unit of sectionUnits) {
        if (!(record.uses ?? []).includes(unit)) missing.push(unit);
      }
    }
  }
  if (phase.id === "outline") {
    const distill = registry.active().filter((item) => item.kind === "distill" || item.kind === "study");
    const record = registry.get("outline");
    if (record) {
      for (const item of distill) {
        if (!(record.uses ?? []).includes(item.unit)) missing.push(item.unit);
      }
    }
  }
  return missing;
}

function unitLines(units) {
  return units
    .map((unit) => {
      if (unit.kind === "distill") {
        return `- ${unit.unit}：${roleLabel(unit.role)}《${unit.title}》第 ${unit.start}–${unit.end} 字（本块约 ${unit.chars} 字）`;
      }
      if (unit.kind === "study") {
        return `- ${unit.unit}：${roleLabel(unit.role)}《${unit.title}》全文（只学写法 / 结构，不是事实）`;
      }
      if (unit.kind === "section") {
        return `- ${unit.unit}：第 ${unit.index}/${unit.total} 节「${unit.title}」`;
      }
      if (unit.kind === "final") return `- ${unit.unit}：完整候选稿`;
      return `- ${unit.unit}：${unit.title}`;
    })
    .join("\n");
}

function recordExample(unit) {
  const locator = Number.isInteger(unit.sourceLine)
    ? `fromLine: ${unit.sourceLine}, toLine: ${unit.sourceLine}`
    : 'quote: "从当前块逐字复制的短原句，数字与空格保持原样"';
  if (unit.kind === "distill") {
    return `record_progress({ phase: "distill", unit: "${unit.unit}", path: "work/notes/${unit.unit.replace(/[^\w]+/g, "-")}.md", summary: "这一块的关键事实", sources: [{ path: "${unit.file}", ${locator} }] })`;
  }
  if (unit.kind === "study") {
    return `record_progress({ phase: "distill", unit: "${unit.unit}", path: "work/notes/style.md", summary: "写法 / 结构小结", sources: [{ path: "${unit.file}", ${locator} }] })`;
  }
  if (unit.kind === "outline") {
    return `record_progress({ phase: "outline", unit: "outline", path: "work/outline.md", summary: "提纲", sections: ["总体要求", "工作安排"] })`;
  }
  if (unit.kind === "section") {
    return `record_progress({ phase: "sections", unit: "${unit.unit}", path: "work/sections/${String(unit.index).padStart(2, "0")}.md", summary: "第 ${unit.index} 节" })`;
  }
  return `record_progress({ phase: "review", unit: "final", path: "work/final.md", summary: "合稿完成" })`;
}

function commonBlock(decision) {
  return `分阶段写作说明（程序控制，不是可选建议）：
- 目的：把长任务拆成短步骤，优先整篇理解，容量不足时按大章节处理。本轮策略：分阶段。
- 窗口 ${decision.window} token，本次工作预算 ${decision.workBudget} token；材料一次放不下时按块处理。
- 中间阶段**不要**调用 submit_document / finish_report；本步完成并写文件后，必须调用 ${RECORD_TOOL_NAME} 登记。
- 宿主只认磁盘上真实存在、哈希对得上、引文能在原文里找到的成果。不要声称「已全部读完」；没登记的单元就是没做。
- 不要手写章节编号（如「一、」「1.」），编号由公文语法与导出程序处理；不要重复写主送单位；遵守要求里的字数与选区限制。
- 事项内容来自材料与实际读到的资料；当前日期只帮助解释相对时间，不能凭空当作事项发生日。标准组织背景可用于规范名称及结合职责安排分工。样稿与范文只学写法，不搬用其中的事项、日期、数字和结论。
- 需要读长文件时按行范围读（read 的 offset / limit），看返回里的「未读」范围继续，不要要求一次返回全文。`;
}

/** 某一步的完整用户消息。`missing` 非空时是纠错重试。 */
export function phasePrompt({ config, decision, phase, registry, plan, attempt = 1, missing = [] }) {
  const task = config.task ?? {};
  const header = `${writingContextBlock(task)}\n任务开始日期：${task.systemDate ?? ""}\n本轮材料全貌：${JSON.stringify(plan.materials.map(({title, role, file}) => ({title, role, file})))}\n本轮用户要求（原文，事实来源）：
${task.request}${registry.answers?.length ? `\n\n用户此前经提问工具确认的信息：\n${registry.answers.map((answer) => JSON.stringify(answer)).join("\n")}` : ""}${registry.evidence?.length ? `\n\n此前知识库/稿件库工具实际返回的只读快照，按需回读，不凭记忆补写：\n${registry.evidence.map((record) => record.path).join("\n")}` : ""}`;
  const correction = missing.length > 0
    ? `\n\n上一次这一步没有通过宿主核对，缺这些单元或依赖：\n${missing.map((item) => `- ${item}`).join("\n")}\n请补齐后重新登记；已登记的成果不要重写。`
    : "";
  const retry = attempt > 1 ? `\n\n这是本步第 ${attempt} 次尝试。` : "";

  if (phase.id === "distill") {
    return `${commonBlock(decision)}

## 第 1 步：提炼材料
${header}

需要处理的单元（每块单独写一份摘记，全部登记）：
${unitLines(phase.units)}

做法：
1. 本步只处理一个单元。事实块直接读取字符范围：${phase.units[0]?.kind === "distill" ? `read({ path: "${phase.units[0].file}", start: ${phase.units[0].start}, end: ${phase.units[0].end} })` : `read({ path: "${phase.units[0]?.file ?? "输入文件"}" })`}，按返回的未读位置继续；可回读材料目录与其他部分以理解全局，但只登记本单元范围。
2. 在这一块的摘记里写：与本次要求相关的事实（数字、日期、单位、责任、期限照抄原文）、以及可用的写法 / 结构要点（样稿只写写法）。
3. 写完后立刻登记这一块。优先使用下面示例的原文行号，宿主会提取原句；使用行号时不填 quote。没有可用完整行时才逐字填写短 quote（不改数字或空格）。自行找行号可用 read({path, numbered:true})，数字与竖线前缀不属于原文：
   ${recordExample(phase.units[0])}
4. 重复直到上面每一块都有登记。不要调用 submit_document。${correction}${retry}`;
  }

  if (phase.id === "outline") {
    const registered = registry.active().filter((item) => item.kind === "distill" || item.kind === "study");
    return `${commonBlock(decision)}

## 第 2 步：写提纲
${header}

先读 ${registered.map((item) => item.path).join("、") || "input/current.md"}，再写 work/outline.md：
- 只用 Markdown 标题层级表达章节（# / ##），不要手写「一、」「（一）」「1.」这类编号。
- 每节写一句写作目标，并标出依据来自哪些已登记的摘记。
- 章节数控制在 2–8 节；短通知可以只写 1–2 节。
- 登记时不要填写 sources；材料来源由宿主从已登记摘记继承。只提交提示要求的字段和 sections。

写完后登记（sections 必须与 outline.md 里的章节标题逐字一致）：
${recordExample(phase.units[0])}
必需的材料依赖由宿主自动补齐，通常不填 uses。
不要调用 submit_document。${correction}${retry}`;
  }

  if (phase.section) {
    const section = phase.section;
    const outline = registry.get("outline");
    return `${commonBlock(decision)}

## 第 3 步：写第 ${section.index}/${section.total} 节「${section.title}」
${header}

1. 先读 work/outline.md（${outline?.path ?? "work/outline.md"}）看全篇目标，只写本节，不要把相邻章节的内容也写进来。
2. 需要事实时读已登记的摘记：${registry.active().filter((item) => item.kind === "distill" || item.kind === "study").map((item) => item.path).join("、") || "input/current.md"}。
3. 本节稿写到 work/sections/${String(section.index).padStart(2, "0")}.md，只写本节正文，不写标题编号、不重复主送。
4. 写完后登记，不要填写 sources；本节的材料依据由宿主从已登记摘记继承：
   ${recordExample(phase.units[0])}
不要调用 submit_document。${correction}${retry}`;
  }

  if (phase.id === "review") {
    const sections = registry.active().filter((item) => item.kind === "section");
    return `${commonBlock(decision)}

## 第 4 步：合稿检查
${header}

1. 读取这些章节稿：${sections.map((item) => item.path).join("、")}，以及 work/outline.md。
2. 合成一份完整候选稿 work/final.md：补齐过渡、去重、检查是否漏掉要求里的事项；事实以已登记摘记与原文为准，不要新增无依据的数字和日期。
3. 用 check_document({ path: "work/final.md" }) 自查一遍；修正程序指出的格式、文字规则及技术问题。本任务不做额外事实核查。
4. 写完后登记，不要填写 sources；来源与章节依赖由宿主从已登记成果继承：
   ${recordExample(phase.units[0])}
outline 与全部章节单元会由宿主自动登记为依赖，通常不填 uses。
不要调用 submit_document。${correction}${retry}`;
  }

  if (isStyleTask(task)) return `${commonBlock(decision)}
## 最后一步：交付风格档案
${header}
读取全部样稿分析摘记：${registry.active().map(record=>record.path).join("、")}。
按input/request.md中的统计与候选范例写work/style.json，调用submit_style({path:"work/style.json",summary:"写法总结"})。
不调用submit_document或finish_report，不写公文正文或全局风格库。`;

  return `${commonBlock(decision)}

## 最后一步：交付
${header}

${phase.report ? `读取已登记的材料摘记：${registry.active().filter((record) => record.kind === "distill" || record.kind === "study").map((record) => record.path).join("、") || "input/current.md"}，整理用户要求的清单。无需生成正文文件。` : "读取 work/final.md，确认它就是完整成品（不是半稿、没有工作区路径、选区外没有被改）。"}然后：
${phase.report ? `- 当前是清单任务：调用 finish_report({ summary, items: [...] })。不要提交正文。` : `- 当前是正文任务：调用 submit_document({ path: "work/final.md", summary: "一句话说明改了什么、依据了哪些材料" })。finish_report 不能代替正文交付。`}

提交内容在调用时固定。不要只写聊天回复；没有调用工具就是没有完成。`;
}

/** 分阶段进度显示用：当前阶段第几步 / 共几步。 */
export function phaseIndex(plan, id) {
  const index = plan.phases.findIndex((phase) => phase.id === id);
  return index < 0 ? 0 : index + 1;
}

export function planSummary(plan) {
  return plan.phases.map((phase) => phase.title).join(" → ");
}
