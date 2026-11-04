#!/usr/bin/env node
//! Pi 写作业务评测：真实 Pi host + 真实标准技能，资料源用自造夹具。
//!
//! 这个脚本只做四件事：搭一个和产品一致的工作区、按 JSONL 协议跑
//! `pi-host/src/main.mjs`、用夹具回答工具调用与提问、按 `cases.json` 的判据
//! 分开记录技术交付与业务判据。它不改生产编排，不连内网知识库，也不把模型
//! 正文清洗成「应该有的样子」。
//!
//! 用法（详见 tests/fixtures/pi-writing-eval/README.md）：
//!   node scripts/pi-writing-eval.mjs --list
//!   node scripts/pi-writing-eval.mjs --selftest
//!   node scripts/pi-writing-eval.mjs --case <名称> --model-config <临时 JSON|-> [--out <目录>]
//!   node scripts/pi-writing-eval.mjs --check <已有运行目录>
//!
//! 本脚本不读 `.env`、`.pi/auth`、真实应用配置或外部测试密钥文件；没有显式
//! 模型配置就拒绝跑模型。

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, "..");
const FIXTURES = path.join(REPO, "tests", "fixtures", "pi-writing-eval");
const HOST_MAIN = path.join(REPO, "pi-host", "src", "main.mjs");
const HOST_DIR = path.join(REPO, "pi-host");
const SKILLS_DIR = path.join(REPO, "assets", "pi-skills");
const MARKDOWN_SKILL = path.join(REPO, "skills", "gongwen-markdown");
const TMP_ROOT = path.join(REPO, "tmp");
export const PROTOCOL_VERSION = 3;
const MAX_EVENT_BYTES = 32 * 1024 * 1024;
const MAX_TOOL_ERROR = 2_000;
const DEFAULT_TIMEOUT_SECONDS = 600;
const ACTIVE_WRITING_SKILLS = new Set([
  "gongwen-draft", "gongwen-extract", "gongwen-imitate", "gongwen-reply",
  "gongwen-research", "gongwen-revise",
]);
const STYLE_SKILL = "gongwen-style-learn";

const HELP = `Pi 写作业务评测

  --list                              列出案例与技能
  --case <名称>                       跑一个案例（可重复用 --case，默认全部）
  --out <目录>                        运行目录，必须在仓库 tmp/ 下（默认 tmp/pi-writing-eval/<时间>）
  --model-config <文件|->             临时模型配置 JSON；- 表示从 stdin 读（不落盘）
  --timeout <秒>                      单个案例超时（默认 ${DEFAULT_TIMEOUT_SECONDS}）
  --case-delay <秒>                   案例之间的请求间隔，0–600，默认0；限流服务建议65
  --check <运行目录>                  只按已有结果重新判定，不连模型
  --selftest                          判据与 CLI 的离线自测，不连模型
  --list                              列案例
  --help                              显示本说明

环境变量：GONGWEN_PI_EVAL_MODEL_CONFIG，或
GONGWEN_PI_EVAL_BASE_URL / GONGWEN_PI_EVAL_API_KEY / GONGWEN_PI_EVAL_MODEL_ID
（可选 GONGWEN_PI_EVAL_CONTEXT_WINDOW / GONGWEN_PI_EVAL_MAX_TOKENS）。

资料源是夹具，Pi host 与模型是真实执行；关键词命中不代表写作质量。`;

/* ---------------------------------------------------------------- 基础工具 */

function clip(text, max) {
  const value = typeof text === "string" ? text : String(text ?? "");
  return value.length > max ? `${value.slice(0, max)}…` : value;
}

function sha256(text) {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

function scrub(text, secret) {
  let out = typeof text === "string" ? text : String(text ?? "");
  if (typeof secret === "string" && secret.length >= 4) {
    out = out.split(secret).join("[已隐藏]");
  }
  return out;
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function writeJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function within(root, target) {
  const relative = path.relative(root, target);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

export function parseArgs(argv) {
  const options = {
    cases: [],
    out: undefined,
    modelConfig: undefined,
    timeoutSeconds: DEFAULT_TIMEOUT_SECONDS,
    caseDelaySeconds: 0,
    check: undefined,
    selftest: false,
    list: false,
    help: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith("--")) {
        throw new Error(`${arg} 缺少参数`);
      }
      index += 1;
      return value;
    };
    switch (arg) {
      case "--case":
        options.cases.push(next());
        break;
      case "--out":
        options.out = next();
        break;
      case "--model-config":
        options.modelConfig = next();
        break;
      case "--timeout": {
        const value = Number(next());
        if (!Number.isFinite(value) || value <= 0) throw new Error("--timeout 必须是正数");
        options.timeoutSeconds = value;
        break;
      }
      case "--case-delay": {
        const value = Number(next());
        if (!Number.isFinite(value) || value < 0 || value > 600) throw new Error("--case-delay 必须为 0–600 秒");
        options.caseDelaySeconds = value;
        break;
      }
      case "--check":
        options.check = next();
        break;
      case "--selftest":
        options.selftest = true;
        break;
      case "--list":
        options.list = true;
        break;
      case "--help":
      case "-h":
        options.help = true;
        break;
      default:
        throw new Error(`不认识的参数：${arg}`);
    }
  }
  return options;
}

/* ---------------------------------------------------------------- 夹具读取 */

export function loadCases(dir = FIXTURES) {
  const file = path.join(dir, "cases.json");
  const parsed = readJson(file);
  if (!parsed || !Array.isArray(parsed.cases) || parsed.cases.length === 0) {
    throw new Error("cases.json 里没有案例");
  }
  return {
    dir,
    fixtures: typeof parsed.fixtures === "string" ? parsed.fixtures : "",
    cases: parsed.cases,
  };
}

export function loadBridge(dir = FIXTURES) {
  const file = path.join(dir, "bridge.json");
  if (!fs.existsSync(file)) return { knowledge: [], manuscripts: [], vocabulary: [], styles: [] };
  return readJson(file);
}

/** 案例自检：材料、选区、技能都必须真实存在，否则夹具坏了不该冒出「模型没做到」。 */
export function validateCases(fixtures) {
  const problems = [];
  const names = new Set();
  for (const item of fixtures.cases) {
    const name = item.name;
    if (!name || names.has(name)) problems.push(`案例名缺失或重复：${name}`);
    names.add(name);
    if (!fs.existsSync(path.join(SKILLS_DIR, item.skill ?? "", "SKILL.md"))) {
      problems.push(`${name} 指定的技能不存在：${item.skill}`);
    }
    const delivery = item.expect?.delivery;
    if (!["document", "report", "style"].includes(delivery)) {
      problems.push(`${name} 的 delivery 必须是 document、report 或 style`);
    }
    if (item.strategy !== undefined && item.strategy !== "phased" && item.strategy !== "auto") {
      problems.push(`${name} 的 strategy 只能是 phased 或 auto`);
    }
    if (item.strategy === "phased" && (item.materials ?? []).length === 0) {
      problems.push(`${name} 要求分阶段但没有材料，会退成常规而测不到阶段`);
    }
    if (item.current) {
      const file = path.join(fixtures.dir, item.current);
      if (!fs.existsSync(file)) problems.push(`${name} 的 current 不存在：${item.current}`);
    }
    if (item.selectionText) {
      if (!item.current) problems.push(`${name} 有选区但没有 current`);
      else {
        const text = fs.readFileSync(path.join(fixtures.dir, item.current), "utf8");
        const hits = text.split(item.selectionText).length - 1;
        if (hits !== 1) problems.push(`${name} 的选区原文在 current 里出现 ${hits} 次`);
      }
    }
    for (const material of item.materials ?? []) {
      if (!fs.existsSync(path.join(fixtures.dir, material.file))) {
        problems.push(`${name} 的材料不存在：${material.file}`);
      }
      if (!["fact", "incoming_letter", "style_sample", "structure_sample"].includes(material.role)) {
        problems.push(`${name} 的材料角色不合法：${material.role}`);
      }
    }
    for (const key of ["required", "forbidden", "requiredSummary"]) {
      const phrases = item.expect?.[key];
      if (phrases !== undefined && (!Array.isArray(phrases) || phrases.some(phrase => typeof phrase !== "string" || phrase === ""))) {
        problems.push(`${name} 的 ${key} 必须是非空字符串数组`);
      }
    }
    for (const group of item.expect?.requirePhrasesAny ?? []) {
      if (!Array.isArray(group) || group.length === 0) {
        problems.push(`${name} 的 requirePhrasesAny 有空组`);
      }
    }
  }
  if (problems.length > 0) throw new Error(`夹具自检失败：\n- ${problems.join("\n- ")}`);
  return true;
}

/* ---------------------------------------------------------------- 模型配置 */

const MODEL_ALIASES = {
  baseUrl: ["baseUrl", "base_url", "url"],
  apiKey: ["apiKey", "api_key", "key"],
  modelId: ["modelId", "model_id", "model"],
  contextWindow: ["contextWindow", "context_window"],
  maxTokens: ["maxTokens", "max_tokens"],
  reasoning: ["reasoning"],
  thinkingLevel: ["thinkingLevel", "thinking_level"],
};

function pick(source, names) {
  for (const name of names) {
    if (source && source[name] !== undefined && source[name] !== null && source[name] !== "") {
      return source[name];
    }
  }
  return undefined;
}

function looksLikeAuthFile(file) {
  const base = path.basename(file).toLowerCase();
  const normalized = file.replaceAll("\\", "/").toLowerCase();
  return base === ".env" || base.startsWith(".env.") || normalized.includes("/.pi/auth/");
}

/**
 * 解析模型配置。`raw` 是 JSON 对象或 JSON 文本。
 * 只保留内存中的密钥；写盘一律用 `redactedModel`。
 */
export function parseModelConfig(raw) {
  let value = raw;
  if (typeof raw === "string") {
    try {
      value = JSON.parse(raw);
    } catch {
      throw new Error("模型配置不是合法 JSON");
    }
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("模型配置必须是 JSON 对象");
  }
  const baseUrl = pick(value, MODEL_ALIASES.baseUrl);
  const modelId = pick(value, MODEL_ALIASES.modelId);
  if (typeof baseUrl !== "string" || !/^https?:\/\//.test(baseUrl)) {
    throw new Error("模型配置缺少 http(s) baseUrl");
  }
  if (typeof modelId !== "string" || modelId.trim() === "") {
    throw new Error("模型配置缺少 modelId");
  }
  const contextWindow = Number(pick(value, MODEL_ALIASES.contextWindow) ?? 128_000);
  const maxTokens = Number(pick(value, MODEL_ALIASES.maxTokens) ?? 8_192);
  if (!Number.isInteger(contextWindow) || contextWindow <= 0) {
    throw new Error("contextWindow 必须是正整数");
  }
  if (!Number.isInteger(maxTokens) || maxTokens <= 0 || maxTokens > contextWindow) {
    throw new Error("maxTokens 必须是正整数且不大于 contextWindow");
  }
  return {
    baseUrl,
    apiKey: typeof pick(value, MODEL_ALIASES.apiKey) === "string" ? pick(value, MODEL_ALIASES.apiKey) : "",
    modelId,
    contextWindow,
    maxTokens,
    reasoning: pick(value, MODEL_ALIASES.reasoning) === true,
    thinkingLevel: typeof pick(value, MODEL_ALIASES.thinkingLevel) === "string"
      ? pick(value, MODEL_ALIASES.thinkingLevel).toLowerCase()
      : "high",
  };
}

/** 写进报告的模型标识：只有地址主机名与模型名，没有密钥、路径或查询串。 */
export function redactedModel(config) {
  let endpointHost = "";
  try {
    endpointHost = new URL(config.baseUrl).host;
  } catch {
    endpointHost = "unparsed";
  }
  return {
    modelId: config.modelId,
    endpointHost,
    contextWindow: config.contextWindow,
    maxTokens: config.maxTokens,
    reasoning: config.reasoning,
    thinkingLevel: config.thinkingLevel,
  };
}

/** 显式给出路径或环境变量才读；默认不碰任何用户配置。 */
export function resolveModelConfig({ file, env = process.env, stdin }) {
  if (file === "-") {
    if (!stdin) throw new Error("--model-config - 需要从 stdin 读 JSON");
    return parseModelConfig(stdin);
  }
  if (file) {
    if (looksLikeAuthFile(file)) {
      throw new Error("拒绝读取环境凭据文件；请另存一份临时模型配置 JSON 再运行");
    }
    return parseModelConfig(fs.readFileSync(file, "utf8"));
  }
  if (env.GONGWEN_PI_EVAL_MODEL_CONFIG) {
    const fallback = env.GONGWEN_PI_EVAL_MODEL_CONFIG;
    if (looksLikeAuthFile(fallback)) {
      throw new Error("拒绝读取环境凭据文件；请另存一份临时模型配置 JSON 再运行");
    }
    return parseModelConfig(fs.readFileSync(fallback, "utf8"));
  }
  if (env.GONGWEN_PI_EVAL_BASE_URL || env.GONGWEN_PI_EVAL_MODEL_ID) {
    return parseModelConfig({
      baseUrl: env.GONGWEN_PI_EVAL_BASE_URL,
      apiKey: env.GONGWEN_PI_EVAL_API_KEY,
      modelId: env.GONGWEN_PI_EVAL_MODEL_ID,
      contextWindow: env.GONGWEN_PI_EVAL_CONTEXT_WINDOW,
      maxTokens: env.GONGWEN_PI_EVAL_MAX_TOKENS,
    });
  }
  throw new Error(
    "没有模型配置：用 --model-config 指定临时 JSON（或 - 从 stdin 读），或设置 GONGWEN_PI_EVAL_MODEL_CONFIG。脚本不会默认读取任何本机凭据。",
  );
}

/* ---------------------------------------------------------------- 工作区 */

function parseFrontmatter(text) {
  const match = /^---\r?\n([\s\S]*?)\r?\n---/.exec(text);
  if (!match) return {};
  const out = {};
  for (const line of match[1].split(/\r?\n/)) {
    const field = /^([A-Za-z_][A-Za-z0-9_-]*)\s*:\s*(.*)$/.exec(line);
    if (!field) continue;
    let value = field[2].trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1);
    }
    if (value !== "" && value !== "|" && value !== ">") out[field[1]] = value;
  }
  return out;
}

function listFiles(root, base = root) {
  const out = [];
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const full = path.join(root, entry.name);
    if (entry.isDirectory()) out.push(...listFiles(full, base));
    else if (entry.isFile()) out.push(path.relative(base, full).replaceAll("\\", "/"));
  }
  return out.sort();
}

/** 技能快照：与 Rust `SkillSnapshot` 相同的目录结构、`input/skills.json` 与 request 块。 */
export function installSkills(workspace, selected, kind = "") {
  const target = path.join(workspace, "skills", "pi-skills");
  const entries = [];
  const allowed = kind === "风格档案" ? new Set([STYLE_SKILL]) : ACTIVE_WRITING_SKILLS;
  for (const name of [...allowed].sort()) {
    const source = path.join(SKILLS_DIR, name);
    if (!fs.existsSync(path.join(source, "SKILL.md"))) throw new Error(`当前内置技能不存在：${name}`);
    const text = fs.readFileSync(path.join(source, "SKILL.md"), "utf8");
    const meta = parseFrontmatter(text);
    fs.cpSync(source, path.join(target, name), { recursive: true });
    const files = listFiles(path.join(target, name)).map((relative) => {
      const bytes = fs.readFileSync(path.join(target, name, relative));
      return { path: relative, bytes: bytes.length, sha256: sha256(bytes.toString("utf8")) };
    });
    entries.push({
      name,
      displayName: meta.display_name ?? meta["display-name"] ?? meta.name ?? name,
      description: meta.description ?? "",
      path: `skills/pi-skills/${name}`,
      files,
    });
  }
  fs.mkdirSync(path.join(workspace, "input"), { recursive: true });
  writeJson(path.join(workspace, "input", "skills.json"), {
    protocol: 1,
    ...(selected ? { selected } : {}),
    skills: entries,
  });
  return entries;
}

function fixtureDateContext(caseDef) {
  const todayText = caseDef.systemDate || "2026-11-04";
  const today = new Date(`${todayText}T00:00:00Z`);
  if (!Number.isFinite(today.getTime())) throw new Error(`无效的自造日期：${todayText}`);
  const monday = new Date(today.getTime() - ((today.getUTCDay() + 6) % 7) * 86_400_000);
  const dateOnly = (date) => date.toISOString().slice(0, 10);
  const weekday = ["星期日", "星期一", "星期二", "星期三", "星期四", "星期五", "星期六"][today.getUTCDay()];
  return {
    now: `${todayText}T09:00:00+08:00`,
    timezone: "Asia/Shanghai (+08:00)",
    today: `${todayText}（${weekday}）`,
    tomorrow: dateOnly(new Date(today.getTime() + 86_400_000)),
    this_week: `${dateOnly(monday)} 至 ${dateOnly(new Date(monday.getTime() + 6 * 86_400_000))}`,
    next_week: `${dateOnly(new Date(monday.getTime() + 7 * 86_400_000))} 至 ${dateOnly(new Date(monday.getTime() + 13 * 86_400_000))}`,
    note: "评测使用自造固定时钟，不代表运行当天真实日期。",
  };
}

export function buildRequestFile({ request, caseDef, fixtures, workspace }) {
  const lines = [String(request ?? "").trim(), ""];
  lines.push("## 技能", "");
  lines.push("本轮可用技能（只读快照，位于 skills/pi-skills/<目录>/SKILL.md）。它们是自然语言步骤，不是流程脚本。");
  for (const entry of installSkillIndexOnly(workspace)) {
    lines.push(`- ${entry.name}（${entry.displayName}）：${entry.description} → ${entry.path}/SKILL.md`);
  }
  lines.push("", "## 材料", "");
  const materials = caseDef.materials ?? [];
  if (materials.length === 0) lines.push("本轮没有附材料。");
  for (const [index, material] of materials.entries()) {
    const text = fs.readFileSync(path.join(fixtures.dir, material.file), "utf8");
    const file = `input/materials/${String(index + 1).padStart(2, "0")}.md`;
    fs.mkdirSync(path.join(workspace, "input", "materials"), { recursive: true });
    fs.writeFileSync(path.join(workspace, file), text, "utf8");
    lines.push(
      `- ${file}：${material.title}（${material.role}，来源：${material.source}，sha256 ${sha256(text).slice(0, 12)}…）`,
    );
  }
  lines.push(
    "",
    "角色含义：fact = 事实材料；incoming_letter = 来函；style_sample = 写法样稿；structure_sample = 结构范文。",
    "只有 fact 与 incoming_letter 可以作为本次事项的事实来源；样稿与范文只提供写法，不能搬用其中单位、人员、日期、数字和结论。",
  );
  return lines.join("\n");
}

function installSkillIndexOnly(workspace) {
  const file = path.join(workspace, "input", "skills.json");
  const parsed = JSON.parse(fs.readFileSync(file, "utf8"));
  return parsed.skills.map((entry) => ({
    name: entry.name,
    displayName: entry.displayName,
    description: entry.description,
    path: entry.path,
  }));
}

/* ---------------------------------------------------------------- 夹具桥 */

const FIXTURE_NOTE = "夹具工具桥：资料全部自造，未连接真实知识库 / 稿件库 / 词库 / 风格库。";

function idList(values) {
  return Array.isArray(values) ? values.filter((value) => typeof value === "string" && value.trim() !== "") : [];
}

function fixtureRenderStyle(style) {
  const examples = (style.examples ?? [])
    .map((example) => `［${example.role ?? "例子"}］${example.text ?? ""}`)
    .join("\n");
  return `【写法风格：${style.name}】${style.description ?? ""}${examples ? `\n范例（只学写法）：\n${examples}` : ""}`;
}

const DATE_PATTERNS = [
  /(?:19|20)\d{2}年\d{1,2}月\d{1,2}日/g,
  /\d{1,2}月\d{1,2}日/g,
];
const NUMBER_PATTERN = /\d+(?:\.\d+)?(?:%|个|人|次|万元|亿元|天|小时|分钟|米|公里|县|区)/g;
const DOCUMENT_PATTERN = /《[^》\n]{2,60}》/g;

function collectMatches(text, patterns) {
  const out = [];
  for (const pattern of patterns) {
    for (const match of text.matchAll(pattern)) {
      const value = match[0];
      if (!out.includes(value)) out.push(value);
    }
  }
  return out;
}

/**
 * 夹具侧的确定性检查：只做「日期 / 数字 / 书名号里的文件名在本轮语料里找不找得到」，
 * 与 Rust 生产闸门不是同一实现，返回结构相同，note 里写明。
 */
function fixtureCheckDocument(text, corpus) {
  const hard = [];
  const seen = new Set();
  for (const value of [
    ...collectMatches(text, DATE_PATTERNS),
    ...collectMatches(text, [NUMBER_PATTERN]),
    ...collectMatches(text, [DOCUMENT_PATTERN]),
  ]) {
    if (seen.has(value)) continue;
    seen.add(value);
    if (!corpus.includes(value)) hard.push(`「${value}」在本轮夹具资料里找不到出处`);
    if (hard.length >= 20) break;
  }
  return {
    hard,
    soft: [],
    note: `${FIXTURE_NOTE} 这里只做字面出处检查，生产用的 Rust 闸门（要素、词表、公文引用）没有跑。`,
  };
}

export function createFixtureBridge({ caseDef, fixtures, bridge, workspace, base, request, materialTexts }) {
  const trace = { tools: [], questions: [] };
  const knowledge = bridge.knowledge ?? [];
  const manuscripts = bridge.manuscripts ?? [];
  const vocabulary = bridge.vocabulary ?? [];
  const styles = bridge.styles ?? [];
  const answers = caseDef.answers ?? {};
  let evidenceSeq = 0;
  const returnedFacts = [];

  const corpusText = () => {
    const factMaterials = materialTexts.filter((_, index) => ["fact", "incoming_letter"].includes(caseDef.materials?.[index]?.role));
    const parts = [base, request, ...factMaterials, ...returnedFacts];
    for (const field of caseDef.elements?.description ?? []) parts.push(String(field.value ?? ""));
    return parts.join("\n");
  };

  const knownNames = new Set([
    "kb_search",
    "kb_read",
    "ms_search",
    "ms_read",
    "vocab_search",
    "current_date",
    "check_document",
    "doc_elements",
    "style_list",
    "style_read",
  ]);

  const searchable = (doc, query) => {
    const haystack = `${doc.title ?? ""}\n${doc.section ?? ""}\n${doc.text ?? doc.markdown ?? ""}\n${idList(doc.keywords).join(" ")}`;
    const terms = query.split(/[\s，。、；：！？（）()《》]+/).filter((term) => term.length >= 2);
    if (terms.length === 0) return haystack.includes(query);
    return terms.some((term) => haystack.includes(term))
      || idList(doc.keywords).some((keyword) => query.includes(keyword));
  };

  const handleImpl = (name, args) => {
    if (!knownNames.has(name)) {
      return { ok: false, error: `夹具工具桥没有工具：${name}` };
    }
    const values = args && typeof args === "object" ? args : {};
    switch (name) {
      case "kb_search": {
        const query = String(values.query ?? "").trim();
        if (query === "") return { ok: false, error: "缺少参数 query" };
        const top = Math.min(Math.max(Number(values.top ?? 5) || 5, 1), 10);
        const hits = knowledge.filter((doc) => searchable(doc, query)).slice(0, top);
        return {
          ok: true,
          result: {
            results: hits.map((doc) => ({
              evidence: `K${++evidenceSeq}`,
              doc_id: doc.docId,
              title: doc.title,
              section: doc.section,
              text: doc.text,
              fixture: true,
            })),
            ...(hits.length === 0 ? { note: "夹具知识库没有找到相关片段" } : {}),
            fixtureNote: FIXTURE_NOTE,
          },
        };
      }
      case "kb_read": {
        const docId = Number(values.doc_id);
        const doc = knowledge.find((item) => item.docId === docId);
        if (!doc) return { ok: false, error: `夹具知识库没有编号为 ${values.doc_id} 的文档` };
        return {
          ok: true,
          result: {
            evidence: `K${++evidenceSeq}`,
            doc_id: doc.docId,
            title: doc.title,
            text: doc.text,
            fixture: true,
            fixtureNote: FIXTURE_NOTE,
          },
        };
      }
      case "ms_search": {
        const query = String(values.query ?? "").trim();
        if (query === "") return { ok: false, error: "缺少参数 query" };
        const limit = Math.min(Math.max(Number(values.limit ?? 5) || 5, 1), 10);
        const hits = manuscripts.filter((doc) => searchable(doc, query)).slice(0, limit);
        return {
          ok: true,
          result: {
            results: hits.map((doc) => ({
              evidence: `M${++evidenceSeq}`,
              id: doc.id,
              title: doc.title,
              kind: doc.kind,
              status: doc.status,
              doc_number: doc.doc_number,
              doc_date: doc.doc_date,
              fixture: true,
            })),
            ...(hits.length === 0 ? { note: "夹具稿件库没有找到相关稿件" } : {}),
            fixtureNote: FIXTURE_NOTE,
          },
        };
      }
      case "ms_read": {
        const id = Number(values.id);
        const doc = manuscripts.find((item) => item.id === id);
        if (!doc) return { ok: false, error: `夹具稿件库没有编号为 ${values.id} 的稿件` };
        return {
          ok: true,
          result: {
            evidence: `M${++evidenceSeq}`,
            id: doc.id,
            title: doc.title,
            kind: doc.kind,
            status: doc.status,
            markdown: doc.markdown,
            fixture: true,
            fixtureNote: FIXTURE_NOTE,
          },
        };
      }
      case "vocab_search": {
        const query = String(values.query ?? "").trim();
        if (query === "") return { ok: false, error: "缺少参数 query" };
        const category = values.category === "unit" || values.category === "单位"
          ? "单位"
          : values.category === "person" || values.category === "人员"
            ? "人员"
            : "";
        const unit = String(values.unit ?? "").trim();
        const hits = vocabulary
          .filter((entry) => (category === "" || entry.category === category)
            && (!unit || entry.unitIdentity === unit || entry.unit === unit)
            && `${entry.name}${idList(entry.aliases).join("")}`.includes(query))
          .slice(0, 20);
        return {
          ok: true,
          result: {
            results: hits.map((entry) => ({
              name: entry.name,
              category: entry.category,
              code: entry.code,
              position: entry.position,
              unit: entry.unit ?? "",
              phone: entry.phone,
              aliases: entry.aliases,
              note: entry.note,
            })),
            note: "词库只提供标准写法，不产生事实证据",
            fixtureNote: FIXTURE_NOTE,
          },
        };
      }
      case "check_document": {
        let text = typeof values.text === "string" ? values.text : "";
        if (values.path !== undefined) {
          if (text !== "") return { ok: false, error: "path 与 text 只能选一个" };
          const relative = String(values.path);
          const target = path.resolve(workspace, relative);
          if (!within(path.join(workspace, "work"), target) || !fs.existsSync(target)) {
            return { ok: false, error: `夹具检查只读 work/ 下存在的文件：${relative}` };
          }
          text = fs.readFileSync(target, "utf8");
        }
        if (text.trim() === "") text = base;
        if (text.trim() === "") return { ok: false, error: "没有可检查的正文" };
        return { ok: true, result: fixtureCheckDocument(text, corpusText()) };
      }
      case "current_date":
        return { ok: true, result: { ...fixtureDateContext(caseDef), fixtureNote: "评测自造固定时钟，不代表真实环境日期。" } };
      case "doc_elements": {
        const elements = caseDef.elements ?? {};
        return {
          ok: true,
          result: {
            kind: caseDef.kind ?? "",
            title: elements.title ?? "",
            description: elements.description ?? [],
            authorized: elements.authorized ?? [],
            note: `${FIXTURE_NOTE} 授权类只读：不要编造、不要建议、不要出题、不要写入正文。`,
            fixture: true,
          },
        };
      }
      case "style_list": {
        const kind = caseDef.kind ?? "";
        const rows = styles
          .filter((style) => style.enabled !== false && (idList(style.kinds).length === 0 || idList(style.kinds).includes(kind)))
          .slice(0, 20);
        return {
          ok: true,
          result: {
            kind,
            results: rows.map((style) => ({
              id: style.id,
              name: style.name,
              note: style.note ?? "",
              kinds: idList(style.kinds),
              occasions: idList(style.occasions),
              examples: Array.isArray(style.examples) ? style.examples.length : 0,
              description_chars: String(style.description ?? "").length,
            })),
            count: rows.length,
            truncated: false,
            note: "夹具风格：档案与范例是写法样稿（style_sample），不是事实来源；不登记为证据。",
            fixtureNote: FIXTURE_NOTE,
          },
        };
      }
      case "style_read": {
        const id = String(values.id ?? "");
        if (id === "") return { ok: false, error: "缺少参数 id" };
        const style = styles.find((item) => item.id === id);
        if (!style) {
          return {
            ok: true,
            result: {
              id,
              available: false,
              reason: "unknown",
              message: `夹具里没有编号为「${id}」的风格档案。`,
              note: "没有换成别的风格。",
              fixtureNote: FIXTURE_NOTE,
            },
          };
        }
        if (style.enabled === false) {
          return {
            ok: true,
            result: {
              id,
              available: false,
              reason: "disabled",
              message: `风格「${style.name}」已停用，不能使用。`,
              note: "没有换成别的风格。",
              fixtureNote: FIXTURE_NOTE,
            },
          };
        }
        const kinds = idList(style.kinds);
        if (kinds.length > 0 && !kinds.includes(caseDef.kind ?? "")) {
          return {
            ok: true,
            result: {
              id,
              available: false,
              reason: "not_applicable",
              message: `风格「${style.name}」不适用于当前文种（${caseDef.kind}）。`,
              note: "没有换成别的风格。",
              fixtureNote: FIXTURE_NOTE,
            },
          };
        }
        return {
          ok: true,
          result: {
            id,
            name: style.name,
            available: true,
            text: fixtureRenderStyle(style),
            note: "夹具风格：档案与范例是写法样稿（style_sample），不是事实来源；不登记为证据。",
            fixtureNote: FIXTURE_NOTE,
          },
        };
      }
      default:
        return { ok: false, error: `夹具工具桥没有实现：${name}` };
    }
  };

  const handle = (name, args) => {
    const outcome = handleImpl(name, args);
    if (outcome.ok && ["kb_search", "kb_read", "ms_search", "ms_read"].includes(name)) {
      for (const row of outcome.result.results ?? [outcome.result]) {
        returnedFacts.push(row.title ?? "", row.section ?? "", row.text ?? row.markdown ?? "");
      }
    }
    return outcome;
  };
  return { handle, trace, corpusText, recordAnswers: (values) => returnedFacts.push(...values.map(answer => String(answer.value))) };
}

/** 只回答夹具明确给出的答案；没有匹配就交空列表，不替模型编事实。 */
function answerQuestions(questions, caseDef, trace) {
  const fixtures = caseDef.answers ?? {};
  const answers = [];
  for (const [index, question] of (Array.isArray(questions) ? questions : []).entries()) {
    const text = String(question?.question ?? "");
    const key = Object.keys(fixtures).find((candidate) => text.includes(candidate));
    if (key === undefined) {
      trace.questions.push({ question: text, answered: false });
      continue;
    }
    trace.questions.push({ question: text, answered: true, key });
    answers.push({ id: index + 1, value: String(fixtures[key]) });
  }
  return answers;
}

/* ---------------------------------------------------------------- 运行一轮 */

function copyMarkdownSkill(workspace) {
  fs.cpSync(MARKDOWN_SKILL, path.join(workspace, "skills", "gongwen-markdown"), { recursive: true });
}

export function prepareWorkspace({ caseDef, fixtures, bridge, root }) {
  const workspace = path.join(root, "workspace");
  const run = path.join(root, "run");
  for (const sub of ["input", "work", "resources"]) {
    fs.mkdirSync(path.join(workspace, sub), { recursive: true });
  }
  fs.mkdirSync(run, { recursive: true });
  const base = caseDef.current
    ? fs.readFileSync(path.join(fixtures.dir, caseDef.current), "utf8")
    : "";
  fs.writeFileSync(path.join(workspace, "input", "current.md"), base, "utf8");
  installSkills(workspace, caseDef.skill, caseDef.kind);
  copyMarkdownSkill(workspace);
  const materialTexts = (caseDef.materials ?? []).map((material) =>
    fs.readFileSync(path.join(fixtures.dir, material.file), "utf8"),
  );
  fs.writeFileSync(
    path.join(workspace, "input", "request.md"),
    buildRequestFile({ request: caseDef.request, caseDef, fixtures, workspace }),
    "utf8",
  );
  // work/ 里预置一份当前正文，方便修改类任务直接改（生产也允许模型自己建）。
  if (base.trim() !== "") fs.writeFileSync(path.join(workspace, "work", "final.md"), base, "utf8");

  const selection = computeSelection(base, caseDef.selectionText);
  const materials = (caseDef.materials ?? []).map((material, index) => {
    const text = materialTexts[index];
    return {
      file: `input/materials/${String(index + 1).padStart(2, "0")}.md`,
      title: material.title,
      source: material.source,
      role: material.role,
      sha256: sha256(text),
      bytes: Buffer.byteLength(text, "utf8"),
    };
  });
  fs.mkdirSync(path.join(workspace, "input", "materials"), { recursive: true });
  writeJson(path.join(workspace, "input", "materials", "index.json"), materials);
  let styleCandidates = 0;
  if (caseDef.skill === STYLE_SKILL && caseDef.kind === "风格档案") {
    // 与 src/ai_services/style.rs 的候选策略保持一致：每篇取开头、匹配过渡、结尾段并去重。
    const candidates = [];
    for (const [index, text] of materialTexts.entries()) {
      const sampleTitle = caseDef.materials[index]?.title ?? `样稿${index + 1}`;
      const paragraphs = text.replace(/^#{1,6}\s+/gmu, "").split(/\n\s*\n/u)
        .map((paragraph) => paragraph.trim()).filter((paragraph) => [...paragraph].length >= 30);
      const push = (role, paragraph) => {
        const candidate = { role, text: [...paragraph].slice(0, 800).join(""), source: sampleTitle };
        if (!candidates.some((item) => item.text === candidate.text)) candidates.push(candidate);
      };
      if (paragraphs[0]) push("开头", paragraphs[0]);
      const middle = paragraphs.slice(1).find((paragraph) => ["为此", "同时", "此外", "一是", "要", "各地", "各单位", "下一步"].some((lead) => paragraph.startsWith(lead)));
      if (middle) push("过渡", middle);
      if (paragraphs.length >= 2) push("结尾", paragraphs.at(-1));
    }
    styleCandidates = candidates.length;
    writeJson(path.join(workspace, "input", "style-guide.json"), {
      stats: { docs: materialTexts.length, chars: materialTexts.reduce((sum, text) => sum + [...text].length, 0) },
      candidates,
    });
  }
  return {
    workspace,
    run,
    base,
    selection,
    materials,
    materialTexts,
    styleCandidates,
    bridge,
  };
}

function computeSelection(base, selectionText) {
  if (!selectionText) return undefined;
  const start = base.indexOf(selectionText);
  if (start < 0) throw new Error("选区原文不在当前正文里");
  return { start, end: start + selectionText.length, text: selectionText };
}

function hostInit({ caseDef, requestId, runId, prepared, model, timeoutSeconds }) {
  return {
    type: "init",
    protocol: PROTOCOL_VERSION,
    requestId,
    runId,
    workspace: {
      root: prepared.workspace,
      state: path.join(prepared.run, "session.sqlite"),
      delivery: prepared.run,
      current: "input/current.md",
      request: "input/request.md",
      skills: "skills/gongwen-markdown",
      piSkills: "skills/pi-skills",
      materials: "input/materials",
    },
    model: {
      baseUrl: model.baseUrl,
      apiKey: model.apiKey,
      modelId: model.modelId,
      contextWindow: model.contextWindow,
      maxTokens: model.maxTokens,
      reasoning: model.reasoning,
      thinkingLevel: model.thinkingLevel,
    },
    // 评测用例可以明确要求分阶段；不传就是自动路由，与实际入口一致。
    strategy: { preference: caseDef.strategy === "phased" ? "phased" : "auto", capability: {} },
    task: {
      request: caseDef.request,
      kind: caseDef.kind ?? "",
      systemDate: caseDef.systemDate ?? "",
      writingContext: caseDef.writingContext ?? {
        date: fixtureDateContext(caseDef),
        organization: { results: [], note: "模拟夹具未设置组织背景；需要时使用本轮合成词库查询。" },
        fixtureNote: "当前日期和空组织背景均为本评测自造数据，不表示真实环境。",
      },
      ...(caseDef.skill ? { skill: caseDef.skill } : {}),
      ...(prepared.selection ? { selection: prepared.selection } : {}),
      materials: prepared.materials,
    },
    limits: {
      maxFileBytes: 8 * 1024 * 1024,
      maxCandidateBytes: 2 * 1024 * 1024,
      timeoutSeconds: Math.max(1, Math.floor(timeoutSeconds)),
      maxQuestionBatches: 2,
      maxQuestions: 3,
    },
  };
}

/**
 * 启动真实 host 跑一个案例。模型地址 / 密钥只在 init 里出现，返回值不含 init 原文。
 */
export async function runCase({
  caseDef,
  fixtures,
  bridge,
  model,
  root,
  timeoutSeconds = DEFAULT_TIMEOUT_SECONDS,
}) {
  if (!fs.existsSync(HOST_MAIN)) throw new Error(`找不到 Pi host：${HOST_MAIN}`);
  if (!fs.existsSync(path.join(HOST_DIR, "node_modules", "@earendil-works", "pi-durable"))) {
    throw new Error("pi-host/node_modules 不完整：请先在 pi-host 下安装生产依赖");
  }
  const prepared = prepareWorkspace({ caseDef, fixtures, bridge, root });
  const stamp = new Date().toISOString().replace(/[-:.TZ]/g, "").slice(0, 14);
  const requestId = `eval-${caseDef.name}-${stamp}`;
  const runId = `eval-${caseDef.name}-${stamp}`;
  const init = hostInit({ caseDef, requestId, runId, prepared, model, timeoutSeconds });
  const toolBox = createFixtureBridge({
    caseDef,
    fixtures,
    bridge,
    workspace: prepared.workspace,
    base: prepared.base,
    request: caseDef.request,
    materialTexts: prepared.materialTexts,
  });

  const startedAt = new Date().toISOString();
  const started = Date.now();
  const child = spawn(process.execPath, [HOST_MAIN], {
    cwd: HOST_DIR,
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
    env: {
      PATH: process.env.PATH ?? "",
      SystemRoot: process.env.SystemRoot ?? "",
      TEMP: process.env.TEMP ?? "",
      TMP: process.env.TMP ?? "",
    },
  });

  const events = [];
  const toolCalls = [];
  let stdout = "";
  let outputBytes = 0;
  let stderr = "";
  let timedOut = false;

  const timer = setTimeout(() => {
    timedOut = true;
    try {
      child.kill();
    } catch {
      // 已经退出。
    }
  }, Math.max(1, timeoutSeconds) * 1000);

  const send = (value) => {
    if (child.stdin.writable) child.stdin.write(`${JSON.stringify(value)}\n`);
  };
  const sendToolResult = (callId, outcome) => {
    if (outcome.ok) send({ type: "tool_result", callId, ok: true, result: outcome.result });
    else send({ type: "tool_result", callId, ok: false, error: clip(outcome.error, MAX_TOOL_ERROR) });
  };

  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    outputBytes += Buffer.byteLength(chunk, "utf8");
    stdout += chunk;
    if (outputBytes > MAX_EVENT_BYTES) {
      timedOut = true;
      child.kill();
      return;
    }
    let index = stdout.indexOf("\n");
    while (index >= 0) {
      const line = stdout.slice(0, index).trim();
      stdout = stdout.slice(index + 1);
      index = stdout.indexOf("\n");
      if (line === "") continue;
      let event;
      try {
        event = JSON.parse(line);
      } catch {
        continue;
      }
      events.push(event);
      if (event.type === "tool_request") {
        const call = { name: event.name, args: event.args, ok: undefined, fixture: true };
        toolCalls.push(call);
        let outcome;
        try {
          outcome = toolBox.handle(event.name, event.args);
        } catch (error) {
          outcome = { ok: false, error: `夹具工具桥异常：${error?.message ?? error}` };
        }
        call.ok = outcome.ok;
        if (!outcome.ok) call.error = clip(outcome.error, MAX_TOOL_ERROR);
        sendToolResult(event.callId, outcome);
      } else if (event.type === "questions") {
        const answers = answerQuestions(event.questions, caseDef, toolBox.trace);
        toolBox.recordAnswers(answers);
        sendToolResult(event.callId, {
          ok: true,
          result: {
            answers,
            ...(answers.length === 0 ? { note: "夹具没有为这些问题提供答案，不要自行编造。" } : {}),
            fixture: true,
          },
        });
      }
    }
  });
  child.stdin.on("error", () => {
    // host 提前退出时写 stdin 会 EPIPE；这里只影响本轮，结果由退出码与事件判定。
  });
  child.stderr.on("data", (chunk) => {
    stderr = (stderr + chunk.toString("utf8")).slice(-64 * 1024);
  });
  child.stdin.write(`${JSON.stringify(init)}\n`);
  // 不能马上 end：host 把 stdin 关闭当成本轮取消（生产由 Rust 保持管道打开）。

  const exitCode = await new Promise((resolve) => {
    child.on("close", (code) => resolve(code ?? -1));
    child.on("error", () => resolve(-1));
  });
  clearTimeout(timer);
  try {
    child.stdin.destroy();
  } catch {
    // 已经关闭。
  }

  const submittedEvent = events.find((event) => event.type === "submitted");
  const reportEvent = events.find((event) => event.type === "report");
  const receipt = readReceiptIfAny(prepared.run);
  const receiptContent = receipt?.kind === "document"
    ? readIfPresent(path.join(prepared.run, "final.md"))
    : receipt?.kind === "style"
      ? readIfPresent(path.join(prepared.run, "final.json"))
      : "";
  const progressText = readIfPresent(path.join(prepared.run, "progress.json"));
  let progress;
  if (progressText) {
    try {
      progress = JSON.parse(progressText);
    } catch {
      progress = { unreadable: true };
    }
  }
  const submitted = submittedEvent
    ? {
        path: submittedEvent.path,
        summary: submittedEvent.summary,
        content: submittedEvent.content,
        sha256: submittedEvent.sha256,
        bytes: submittedEvent.bytes,
      }
    : null;
  const report = reportEvent
    ? { summary: reportEvent.summary, items: reportEvent.items ?? [] }
    : null;

  return {
    case: caseDef.name,
    skill: caseDef.skill,
    kind: caseDef.kind,
    startedAt,
    durationMs: Date.now() - started,
    exitCode,
    timedOut,
    events: events.map((event) => redactEvent(event, model.apiKey)),
    submitted,
    report,
    receipt,
    receiptContent,
    styleCandidates: prepared.styleCandidates,
    progress,
    base: prepared.base,
    selection: prepared.selection,
    toolCalls,
    questions: toolBox.trace.questions,
    stderr: scrub(stderr, model.apiKey),
    artifacts: {
      delivery: path.relative(root, prepared.run).replaceAll("\\", "/"),
      workspace: path.relative(root, prepared.workspace).replaceAll("\\", "/"),
    },
  };
}

function readIfPresent(file) {
  try {
    return fs.readFileSync(file, "utf8");
  } catch {
    return "";
  }
}

function readReceiptIfAny(run) {
  const file = path.join(run, "receipt.json");
  if (!fs.existsSync(file)) return undefined;
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return { unreadable: true };
  }
}

/** 事件里可能有模型正文；只抹密钥，不改正文。 */
function redactEvent(event, secret) {
  const text = JSON.stringify(event);
  if (!secret || !text.includes(secret)) return event;
  return JSON.parse(scrub(text, secret));
}

/* ---------------------------------------------------------------- 判据 */

export function deliveredText(result) {
  if (result.submitted?.content) return result.submitted.content;
  if (result.report) {
    const items = (result.report.items ?? [])
      .map((item) => `${item.title ?? ""}\n${item.detail ?? ""}`)
      .join("\n");
    return `${result.report.summary ?? ""}\n${items}`;
  }
  return "";
}

function check(list, name, ok, detail) {
  list.push({ name, ok: Boolean(ok), ...(detail === undefined ? {} : { detail: String(detail) }) });
}

/**
 * 分开记录技术交付与业务判据。`manual` 不转成分数；关键词命中只算「指定事实在不在」。
 */
/**
 * 数字 / 日期判据的等价形式：只去掉紧邻数字的空白（「10 月 8 日」=「10月8日」）。
 * 不改动交付原文，也不参与字数上限：字数判据始终用原文计数，不能借归一化放宽。
 */
export function compactNumbers(text) {
  return String(text ?? "").replace(/(?<=\d)\s+|\s+(?=\d)/gu, "");
}

export function judgeCase(caseDef, result) {
  const technical = [];
  const business = [];
  const expect = caseDef.expect ?? {};
  const wantsDocument = expect.delivery === "document";
  const wantsStyle = expect.delivery === "style";
  const delivered = deliveredText(result);
  // 事实关键词比对用数字空格等价形式；字数与选区检查仍用原文。
  const matchable = compactNumbers(delivered);
  let factScope = delivered;
  let factMatchable = matchable;

  check(technical, "进程正常退出", result.exitCode === 0 && !result.timedOut, result.timedOut ? "超时" : `exitCode=${result.exitCode}`);
  check(technical, "收到 completed 事件", result.events.some((event) => event.type === "completed"), result.events.map((event) => event.type).join(","));
  check(technical, "没有 error 事件", !result.events.some((event) => event.type === "error"), result.events.find((event) => event.type === "error")?.message);
  const failedTools = result.toolCalls.filter((call) => call.ok === false);
  check(technical, "领域工具调用没有失败", failedTools.length === 0, failedTools.map((call) => `${call.name}: ${call.error}`).join("; "));

  if (wantsDocument) {
    check(technical, "交付了正文", Boolean(result.submitted), result.submitted?.path);
    check(technical, "没有同时交清单", !result.report, result.report ? "同一轮也交了 finish_report" : undefined);
    if (result.submitted) {
      const content = result.submitted.content ?? "";
      check(technical, "交付文件在 work/ 下", /^work\/[^\0]+\.(md|markdown)$/i.test(result.submitted.path ?? ""), result.submitted.path);
      check(technical, "交付正文不是空稿", content.trim() !== "");
      check(technical, "交付哈希与内容一致", result.submitted.sha256 === sha256(content), result.submitted.sha256);
      check(
        technical,
        "回执与交付一致",
        Boolean(result.receipt) &&
          result.receipt.kind === "document" &&
          result.receipt.sha256 === result.submitted.sha256 &&
          result.receipt.bytes === Buffer.byteLength(content, "utf8"),
        result.receipt?.kind,
      );
      check(
        technical,
        "回执成品快照与交付正文相同",
        result.receiptContent === content,
        result.receiptContent ? "final.md 与 submitted.content 不同" : "没有 final.md",
      );
    }
  } else if (expect.delivery === "report") {
    check(technical, "走了清单交付", Boolean(result.report), result.report ? "finish_report" : "没有 report 事件");
    check(technical, "没有提交正文", !result.submitted, result.submitted?.path);
    check(technical, "清单不是空的", (result.report?.items ?? []).length > 0 || String(result.report?.summary ?? "").trim() !== "");
  } else if (wantsStyle) {
    check(technical, "交付了独立风格文件", Boolean(result.submitted), result.submitted?.path);
    check(technical, "没有同时交正文或清单", !result.report && !result.events.some((event) => event.type === "report"));
    if (result.submitted) {
      const content = result.submitted.content ?? "";
      check(technical, "风格JSON位于work/下", /^work\/[^\0]+\.json$/iu.test(result.submitted.path ?? ""), result.submitted.path);
      check(technical, "风格回执类型正确", result.receipt?.kind === "style", result.receipt?.kind);
      check(technical, "风格哈希与字节数正确", Buffer.byteLength(content, "utf8") <= 64 * 1024 && result.submitted.sha256 === sha256(content) && result.submitted.bytes === Buffer.byteLength(content, "utf8"));
      check(technical, "风格回执哈希与字节数正确", result.receipt?.sha256 === result.submitted.sha256 && result.receipt?.bytes === Buffer.byteLength(content, "utf8"));
      check(technical, "风格快照与交付相同", result.receiptContent === content, result.receiptContent ? undefined : "没有 final.json");
      let profile;
      try { profile = JSON.parse(content); } catch { profile = undefined; }
      const allowed = new Set(["name", "note", "occasions", "description", "example_numbers"]);
      const objectOk = Boolean(profile && typeof profile === "object" && !Array.isArray(profile));
      check(business, "风格交付为JSON对象", objectOk);
      check(business, "风格字段符合独立草稿契约", objectOk && Object.keys(profile).every((key) => allowed.has(key)) && typeof profile.name === "string" && Boolean(profile.name.trim()) && [...profile.name].length <= 40 && typeof profile.description === "string" && Boolean(profile.description.trim()) && [...profile.description].length <= 12000, objectOk ? Object.keys(profile).join(",") : "JSON无效或不是对象");
      check(business, "风格可选字段类型与范围正确", objectOk && (profile.note === undefined || (typeof profile.note === "string" && [...profile.note].length <= 500)) && (profile.occasions === undefined || (Array.isArray(profile.occasions) && profile.occasions.length <= 8 && profile.occasions.every((item) => typeof item === "string" && Boolean(item.trim()) && [...item].length <= 60))) && (profile.example_numbers === undefined || (Array.isArray(profile.example_numbers) && profile.example_numbers.length <= 5 && profile.example_numbers.every((n) => Number.isSafeInteger(n) && n >= 1 && n <= 30))), objectOk ? "检查 note、occasions、example_numbers" : undefined);
      const numbers = profile?.example_numbers ?? [];
      check(business, "风格范例编号属于本轮候选且不重复", Array.isArray(numbers) && new Set(numbers).size === numbers.length && numbers.every((n) => Number.isSafeInteger(n) && n >= 1 && n <= Math.min(30, result.styleCandidates ?? 0)), JSON.stringify(numbers));
      for (const group of expect.requirePhrasesAny ?? []) {
        const hit = group.find((phrase) => content.includes(phrase));
        check(business, `风格倾向覆盖「${group[0]}」同义项`, Boolean(hit), hit ? `命中「${hit}」` : "该组说法均未出现");
      }
    }
  }

  if (expect.unchangedOutsideSelection) {
    const selection = result.selection;
    const content = result.submitted?.content ?? "";
    if (!selection || !result.submitted) {
      check(technical, "选区外逐字未变", false, "没有选区或没有正文");
    } else {
      const prefixOk = content.slice(0, selection.start) === result.base.slice(0, selection.start);
      const tail = result.base.length - selection.end;
      const suffixOk = tail === 0 || (content.length >= tail && content.slice(content.length - tail) === result.base.slice(result.base.length - tail));
      check(technical, "选区外逐字未变", prefixOk && suffixOk, prefixOk ? (suffixOk ? undefined : "选区后文字被改") : "选区前文字被改");
      const inside = content.length >= selection.start + tail ? content.slice(selection.start, content.length - tail) : "";
      factScope = inside;
      factMatchable = compactNumbers(inside);
      if (expect.maxSelectionChars !== undefined) {
        check(
          business,
          `选区内压到 ${expect.maxSelectionChars} 字以内`,
          inside.trim().length > 0 && inside.trim().length <= expect.maxSelectionChars,
          `选区内 ${inside.trim().length} 字`,
        );
      }
    }
  }

  for (const phrase of expect.required ?? []) {
    const ok = factMatchable.includes(compactNumbers(phrase));
    check(business, `保留事实「${phrase}」`, ok, ok ? undefined : "交付内容的任务范围里没有");
  }
  for (const phrase of expect.forbidden ?? []) {
    const ok = !matchable.includes(compactNumbers(phrase));
    check(business, `不得出现「${phrase}」`, ok, ok ? undefined : "交付内容里出现了");
  }
  for (const phrase of expect.requiredSummary ?? []) {
    check(business, `交付说明含「${phrase}」`, (result.submitted?.summary ?? "").includes(phrase), "独立检查交付说明，不允许用正文代替表单建议");
  }
  for (const group of expect.requirePhrasesAny ?? []) {
    const hit = group.find((phrase) => factMatchable.includes(compactNumbers(phrase)));
    check(business, `说明了「${group[0]}」类结论`, Boolean(hit), hit ? `命中「${hit}」` : "一个说法都没有");
  }
  if (expect.minChars !== undefined) {
    check(business, `正文不少于 ${expect.minChars} 字`, delivered.trim().length >= expect.minChars, `${delivered.trim().length} 字`);
  }
  if (expect.inlineFootnotesOnly) {
    const invalid = delivered.match(/\[\^[^\]\r\n]+\](?![:：]\s*[(（])/u);
    check(business, "脚注只用应用支持的行内语法", !invalid, invalid ? `不支持的独立引用或尾部定义：${invalid[0]}` : undefined);
  }
  if (expect.requiredToolsAny?.length) {
    const names = new Set(result.toolCalls.map((call) => call.name));
    const hit = expect.requiredToolsAny.find((name) => names.has(name));
    check(business, `调用了 ${expect.requiredToolsAny.join(" / ")}`, Boolean(hit), hit ? undefined : [...names].join(","));
  }

  if (caseDef.strategy === "phased") {
    const records = Array.isArray(result.progress?.records) ? result.progress.records : [];
    // 旧版本留痕且标失效是正常更正；只认可每个单元最后登记的有效版本。
    const latest = [...new Map(records.map((record) => [record.unit, record])).values()];
    const current = latest.filter((record) => !record.invalid);
    const units = new Set(current.map((record) => record.unit));
    const phaseNames = new Set(current.map((record) => record.phase));
    check(technical, "确实走了分阶段调度", result.events.some((event) => event.type === "strategy" && event.mode === "phased"), result.events.find((event) => event.type === "strategy")?.mode);
    check(technical, "登记了材料提炼成果", [...units].some((unit) => String(unit).startsWith("distill:") || String(unit).startsWith("study:")), [...units].join(","));
    check(technical, "登记了提纲与章节成果", units.has("outline") && [...units].some((unit) => String(unit).startsWith("section:")), [...units].join(","));
    check(technical, "最终提交前登记了完整候选稿", units.has("final"), phaseNames.has("review") ? undefined : "没有 review 阶段登记");
    const unresolved = latest.filter((record) => record.invalid || (record.uses ?? []).some((unit) => !units.has(unit)));
    check(technical, "当前成果与依赖均有有效登记", unresolved.length === 0, unresolved.map((record) => record.unit).join(","));
  }

  const required = expect.required ?? [];
  const forbidden = expect.forbidden ?? [];
  const keywordCoverage = {
    note: "只表示指定事实 / 禁项是否命中（数字空格等价），不是写作质量总分",
    requiredHit: required.filter((phrase) => matchable.includes(compactNumbers(phrase))).length,
    requiredTotal: required.length,
    forbiddenHit: forbidden.filter((phrase) => matchable.includes(compactNumbers(phrase))).length,
    forbiddenTotal: forbidden.length,
  };

  return {
    technical,
    business,
    manual: expect.manual ?? [],
    keywordCoverage,
    technicalPassed: technical.every((item) => item.ok),
    businessPassed: business.every((item) => item.ok),
  };
}

/* ---------------------------------------------------------------- 输出 */

function fmt(value) {
  return String(value ?? "").replaceAll("|", "\\|").replaceAll("\n", " ");
}

export function renderReport({ run }) {
  const lines = [
    "# Pi 写作业务评测报告",
    "",
    `- 运行目录：\`${run.outDir}\``,
    `- 夹具：${run.fixturesNote}`,
    `- 模型：${run.model.modelId}（${run.model.endpointHost}，窗口 ${run.model.contextWindow}，输出上限 ${run.model.maxTokens}，thinking ${run.model.reasoning ? run.model.thinkingLevel : "off"}）`,
    `- 开始：${run.startedAt}`,
    "",
    "> 资料源是夹具，Pi host 与模型是真实执行；这里不是内网知识库联机结果，也不是写作质量总分。",
    "",
  ];
  for (const item of run.cases) {
    const judge = item.judge;
    const mark = judge.technicalPassed && judge.businessPassed ? "通过" : "未通过";
    lines.push(`## ${item.case}（${mark}）`, "");
    lines.push(`- 交付：${item.result.submitted ? `${item.result.receipt?.kind === "style" ? "风格JSON" : "正文"} \`${item.result.submitted.path}\`` : item.result.report ? "侧栏清单" : "无"}`);
    lines.push(`- 技术判据：${judge.technical.filter((check) => check.ok).length}/${judge.technical.length}`);
    for (const check of judge.technical.filter((check) => !check.ok)) {
      lines.push(`  - 未过：${check.name}${check.detail ? `（${check.detail}）` : ""}`);
    }
    lines.push(`- 业务判据：${judge.business.filter((check) => check.ok).length}/${judge.business.length}`);
    for (const check of judge.business.filter((check) => !check.ok)) {
      lines.push(`  - 未过：${check.name}${check.detail ? `（${check.detail}）` : ""}`);
    }
    lines.push(
      `- 关键词覆盖：必含 ${judge.keywordCoverage.requiredHit}/${judge.keywordCoverage.requiredTotal}，禁项命中 ${judge.keywordCoverage.forbiddenHit}/${judge.keywordCoverage.forbiddenTotal}（不等于写作质量）`,
    );
    if (judge.manual.length > 0) {
      lines.push("- 人工判断（未折算成分数）：");
      for (const item of judge.manual) lines.push(`  - [ ] ${item}`);
    }
    if (item.result.toolCalls.length > 0) {
      lines.push(`- 工具轨迹：${item.result.toolCalls.map((call) => `${call.name}${call.ok === false ? "（失败）" : ""}`).join(" → ")}`);
    }
    lines.push("");
  }
  const passed = run.cases.filter((item) => item.judge.technicalPassed && item.judge.businessPassed).length;
  lines.push(`合计：${passed}/${run.cases.length} 个案例的技术与业务判据都通过。`, "");
  return lines.join("\n");
}

function caseArtifactsDir(outDir, name) {
  return path.join(outDir, "cases", name);
}

function storeCase(outDir, caseDef, result, judge) {
  const dir = caseArtifactsDir(outDir, caseDef.name);
  writeJson(path.join(dir, "result.json"), { case: caseDef.name, result, judge });
  writeJson(
    path.join(dir, "artifacts", "report.json"),
    result.report ?? { absent: true },
  );
  if (result.submitted) {
    fs.mkdirSync(path.join(dir, "artifacts"), { recursive: true });
    const fileName = result.receipt?.kind === "style" ? "style.json" : "final.md";
    fs.writeFileSync(path.join(dir, "artifacts", fileName), result.submitted.content ?? "", "utf8");
  }
  const receiptFile = path.join(dir, "run", "receipt.json");
  if (fs.existsSync(receiptFile)) {
    fs.mkdirSync(path.join(dir, "artifacts"), { recursive: true });
    fs.copyFileSync(receiptFile, path.join(dir, "artifacts", "receipt.json"));
  }
  fs.writeFileSync(
    path.join(dir, "events.jsonl"),
    result.events.map((event) => JSON.stringify(event)).join("\n") + "\n",
    "utf8",
  );
}

function selectCases(fixtures, names) {
  if (!names || names.length === 0) return fixtures.cases;
  const missing = names.filter((name) => !fixtures.cases.some((item) => item.name === name));
  if (missing.length > 0) throw new Error(`没有这些案例：${missing.join(", ")}`);
  return fixtures.cases.filter((item) => names.includes(item.name));
}

/* ---------------------------------------------------------------- 自测 */

function syntheticCase(overrides = {}) {
  return {
    name: "selftest",
    skill: "gongwen-revise",
    kind: "通知",
    expect: {
      delivery: "document",
      required: ["2026年12月1日"],
      forbidden: ["市水务局"],
      manual: ["看语气"],
      ...overrides,
    },
  };
}

function syntheticResult(overrides = {}) {
  const base = "# 通知\n\n第一段。\n\n第二段要改。\n\n第三段。\n";
  const start = base.indexOf("第二段要改。");
  const content = "# 通知\n\n第一段。\n\n第二段已改，2026年12月1日前完成。\n\n第三段。\n";
  const result = {
    exitCode: 0,
    timedOut: false,
    events: [{ type: "submitted" }, { type: "completed", submitted: true }],
    submitted: {
      path: "work/final.md",
      summary: "改好了",
      content,
      sha256: sha256(content),
      bytes: Buffer.byteLength(content, "utf8"),
    },
    report: null,
    receipt: {
      kind: "document",
      sha256: sha256(content),
      bytes: Buffer.byteLength(content, "utf8"),
    },
    receiptContent: content,
    base,
    selection: { start, end: start + "第二段要改。".length, text: "第二段要改。" },
    toolCalls: [{ name: "check_document", ok: true }],
    questions: [],
    ...overrides,
  };
  return result;
}

/** 判据自测：不用模型，只证明判据能区分好结果与几种典型坏结果。 */
export function selftest() {
  const problems = [];
  const expect = (condition, message) => {
    if (!condition) problems.push(message);
  };

  const good = judgeCase(syntheticCase({ unchangedOutsideSelection: true, maxSelectionChars: 40 }), syntheticResult());
  expect(good.technicalPassed, `正常结果的技术判据应通过：${JSON.stringify(good.technical)}`);
  expect(good.businessPassed, `正常结果的业务判据应通过：${JSON.stringify(good.business)}`);

  const missingFact = judgeCase(
    syntheticCase(),
    syntheticResult({
      submitted: {
        ...syntheticResult().submitted,
        content: "# 通知\n\n第一段。\n\n第二段已改。\n",
      },
    }),
  );
  expect(!missingFact.businessPassed, "缺事实时业务判据不应通过");

  const liftedStyleFact = judgeCase(
    syntheticCase(),
    syntheticResult({
      submitted: { ...syntheticResult().submitted, content: "# 通知\n\n市水务局。\n" },
    }),
  );
  expect(
    liftedStyleFact.business.some((check) => check.name.includes("市水务局") && !check.ok),
    "样稿事实被搬用时业务判据应报出来",
  );

  const outsideChanged = judgeCase(
    syntheticCase({ unchangedOutsideSelection: true }),
    syntheticResult({
      submitted: {
        ...syntheticResult().submitted,
        content: "# 通知\n\n第一段。\n\n第二段已改，2026年12月1日前完成。\n\n第三段被改。\n",
      },
    }),
  );
  expect(!outsideChanged.technicalPassed, "选区外被改时技术判据不应通过");

  const reportForDocumentTask = judgeCase(syntheticCase(), syntheticResult({ submitted: null, report: { summary: "只看看", items: [{ title: "问题" }] }, receipt: undefined, receiptContent: "" }));
  expect(!reportForDocumentTask.technicalPassed, "该交正文却只交清单时技术判据不应通过");

  const documentForReportTask = judgeCase(
    syntheticCase({ delivery: "report" }),
    syntheticResult(),
  );
  expect(!documentForReportTask.technicalPassed, "该只交清单却交正文时技术判据不应通过");

  const failedTool = judgeCase(syntheticCase(), syntheticResult({ toolCalls: [{ name: "kb_search", ok: false, error: "夹具没有" }] }));
  expect(!failedTool.technicalPassed, "工具失败时技术判据不应通过");

  const forbiddenInReportOnly = judgeCase(
    syntheticCase({ delivery: "report", requirePhrasesAny: [["未找到原文", "未找到"]] }),
    syntheticResult({
      submitted: null,
      receipt: undefined,
      receiptContent: "",
      events: [{ type: "report" }, { type: "completed", submitted: false }],
      report: { summary: "已核实，没有问题", items: [{ title: "政策文件", detail: "一致" }] },
    }),
  );
  expect(!forbiddenInReportOnly.businessPassed, "缺「未找到原文」时业务判据不应通过");

  // 数字 / 日期空格等价：原文写「10 月 8 日」也认作「10月8日」；字数判据仍按原文计数。
  const spacedNumbers = judgeCase(
    syntheticCase(),
    syntheticResult({
      submitted: {
        ...syntheticResult().submitted,
        content: "# 通知\n\n2026 年 12 月 1 日 前完成。\n",
      },
    }),
  );
  expect(spacedNumbers.businessPassed, "数字空格等价的日期应保留事实");
  const spacedButShort = judgeCase(
    syntheticCase({ minChars: 60 }),
    syntheticResult({
      submitted: {
        ...syntheticResult().submitted,
        content: "# 通知\n\n2026 年 12 月 1 日 前完成。\n",
      },
    }),
  );
  expect(!spacedButShort.businessPassed, "字数判据不能被数字空格归一化放宽");

  const phasedMissing = judgeCase(
    { ...syntheticCase(), strategy: "phased" },
    syntheticResult(),
  );
  expect(!phasedMissing.technicalPassed, "要求分阶段却没有登记成果时技术判据不应通过");
  const phasedOk = judgeCase(
    { ...syntheticCase(), strategy: "phased" },
    syntheticResult({
      events: [{ type: "strategy", mode: "phased" }, { type: "submitted" }, { type: "completed", submitted: true }],
      progress: {
        records: [
          { unit: "distill:input/materials/01.md#1", phase: "distill" },
          { unit: "outline", phase: "outline" },
          { unit: "section:1", phase: "sections" },
          { unit: "final", phase: "review" },
        ],
      },
    }),
  );
  expect(phasedOk.technicalPassed, `完整分阶段登记应通过技术判据：${JSON.stringify(phasedOk.technical)}`);

  const styleCase = {
    name: "style-selftest",
    expect: { delivery: "style", requirePhrasesAny: [["分条", "逐条列举"]], manual: [] },
  };
  const makeStyleResult = (profile, overrides = {}) => {
    const content = typeof profile === "string" ? profile : JSON.stringify(profile);
    const digest = sha256(content);
    return syntheticResult({
      submitted: { path: "work/style.json", summary: "识别风格", content, sha256: digest, bytes: Buffer.byteLength(content) },
      report: null,
      receipt: { kind: "style", sha256: digest, bytes: Buffer.byteLength(content) },
      receiptContent: content,
      styleCandidates: 2,
      ...overrides,
    });
  };
  const goodStyle = { name: "通知表达", description: "主体逐条列举工作安排。", example_numbers: [1] };
  const validStyle = judgeCase(styleCase, makeStyleResult(goodStyle));
  expect(validStyle.technicalPassed && validStyle.businessPassed, "有效风格 JSON 与候选编号应通过");
  const emptyStyle = judgeCase(styleCase, makeStyleResult({}));
  expect(!emptyStyle.businessPassed, "缺 name / description 的空对象不得通过风格判据");
  const invalidCandidateStyle = judgeCase(styleCase, makeStyleResult({ ...goodStyle, example_numbers: [99] }));
  expect(!invalidCandidateStyle.businessPassed, "不属于本轮候选的风格编号不得通过");
  const forgedStyleSnapshot = judgeCase(styleCase, makeStyleResult(goodStyle, { receiptContent: "{}" }));
  expect(!forgedStyleSnapshot.technicalPassed, "风格回执快照与提交不同不得通过");

  const args = parseArgs(["--case", "a", "--case", "b", "--timeout", "30", "--out", "tmp/x"]);
  expect(args.cases.length === 2 && args.timeoutSeconds === 30 && args.out === "tmp/x", "CLI 参数解析不对");
  let rejected = false;
  try {
    parseArgs(["--nope"]);
  } catch {
    rejected = true;
  }
  expect(rejected, "未知参数应被拒绝");

  const redacted = redactedModel(parseModelConfig({ baseUrl: "https://example.invalid/v1?k=1", apiKey: "sk-secret-value", modelId: "m" }));
  expect(redacted.endpointHost === "example.invalid" && !JSON.stringify(redacted).includes("sk-secret"), "模型标识没有脱敏");
  expect(!JSON.stringify(redacted).includes("?k=1"), "地址里的查询串不应写进报告");

  let refused = false;
  try {
    resolveModelConfig({ file: undefined, env: {}, stdin: undefined });
  } catch {
    refused = true;
  }
  expect(refused, "没有显式配置时应拒绝跑模型");

  if (problems.length > 0) throw new Error(`自测失败：\n- ${problems.join("\n- ")}`);
  return true;
}

/* ---------------------------------------------------------------- 主流程 */

function defaultOutDir() {
  const stamp = new Date().toISOString().replace(/[-:.TZ]/g, "").slice(0, 14);
  return path.join(TMP_ROOT, "pi-writing-eval", `${stamp}-${process.pid}`);
}

function ensureOutDir(out) {
  const resolved = path.resolve(REPO, out);
  if (!within(TMP_ROOT, resolved) || resolved === TMP_ROOT) {
    throw new Error(`--out 必须放在仓库 tmp/ 下：${out}`);
  }
  if (fs.existsSync(resolved) && fs.readdirSync(resolved).length > 0) {
    throw new Error("输出目录已存在且非空，请使用新目录；重新判定已有结果请用 --check");
  }
  fs.mkdirSync(resolved, { recursive: true });
  return resolved;
}

async function readStdin() {
  if (process.stdin.isTTY) throw new Error("--model-config - 需要从管道读 JSON");
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}

function printCaseList(fixtures) {
  for (const item of fixtures.cases) {
    console.log(`${item.name}\t${item.skill}\t${item.kind}\t${item.expect.delivery}`);
  }
}

async function checkExisting(dir) {
  const root = path.resolve(REPO, dir);
  if (!fs.existsSync(root)) throw new Error(`运行目录不存在：${dir}`);
  const fixtures = loadCases();
  validateCases(fixtures);
  const items = [];
  for (const caseDef of fixtures.cases) {
    const file = path.join(caseArtifactsDir(root, caseDef.name), "result.json");
    if (!fs.existsSync(file)) continue;
    const stored = readJson(file);
    const judge = judgeCase(caseDef, stored.result);
    items.push({
      case: caseDef.name,
      result: stored.result,
      judge,
    });
    console.log(
      `${caseDef.name}: 技术 ${judge.technicalPassed ? "通过" : "未通过"}，业务 ${judge.businessPassed ? "通过" : "未通过"}`,
    );
    for (const check of [...judge.technical, ...judge.business].filter((check) => !check.ok)) {
      console.log(`  - ${check.name}${check.detail ? `：${check.detail}` : ""}`);
    }
  }
  if (items.length === 0) throw new Error("运行目录里没有可判定的案例结果");
  const runFile = path.join(root, "run.json");
  const run = fs.existsSync(runFile) ? readJson(runFile) : { model: {}, fixturesNote: "", startedAt: "", outDir: root };
  run.cases = items;
  run.outDir = root;
  fs.writeFileSync(path.join(root, "report.md"), renderReport({ run }), "utf8");
  const passed = items.filter((item) => item.judge.technicalPassed && item.judge.businessPassed).length;
  return passed === items.length ? 0 : 1;
}

export async function main(argv = process.argv.slice(2), io = {}) {
  const options = parseArgs(argv);
  if (options.help) {
    console.log(HELP);
    return 0;
  }
  if (options.selftest) {
    selftest();
    console.log("selftest ok：判据与 CLI 的离线自测通过（没有连模型）");
    return 0;
  }
  const fixtures = loadCases();
  validateCases(fixtures);
  if (options.list) {
    printCaseList(fixtures);
    return 0;
  }
  if (options.check) {
    return checkExisting(options.check);
  }

  const stdin = io.stdin ?? (options.modelConfig === "-" ? await readStdin() : undefined);
  const model = resolveModelConfig({ file: options.modelConfig, stdin });
  const selected = selectCases(fixtures, options.cases);
  const outDir = ensureOutDir(options.out ?? defaultOutDir());
  const bridge = loadBridge();
  const run = {
    startedAt: new Date().toISOString(),
    outDir,
    fixturesNote: fixtures.fixtures,
    model: redactedModel(model),
    cases: [],
  };
  let passed = 0;
  let caseIndex = 0;
  for (const caseDef of selected) {
    if (caseIndex++ > 0 && options.caseDelaySeconds > 0) {
      console.log(`等候 ${options.caseDelaySeconds} 秒，避免服务商 RPM 限流…`);
      await new Promise(resolve => setTimeout(resolve, options.caseDelaySeconds * 1000));
    }
    console.log(`跑 ${caseDef.name}…`);
    const root = caseArtifactsDir(outDir, caseDef.name);
    fs.mkdirSync(root, { recursive: true });
    let result;
    try {
      result = await runCase({
        caseDef,
        fixtures,
        bridge,
        model,
        root,
        timeoutSeconds: options.timeoutSeconds,
      });
    } catch (error) {
      result = {
        case: caseDef.name,
        exitCode: -1,
        timedOut: false,
        events: [],
        submitted: null,
        report: null,
        receipt: undefined,
        receiptContent: "",
        base: "",
        selection: undefined,
        toolCalls: [],
        questions: [],
        stderr: "",
        error: scrub(error?.message ?? String(error), model.apiKey),
        durationMs: 0,
      };
    }
    const judge = judgeCase(caseDef, result);
    storeCase(outDir, caseDef, result, judge);
    run.cases.push({ case: caseDef.name, result, judge });
    const ok = judge.technicalPassed && judge.businessPassed;
    if (ok) passed += 1;
    console.log(`  ${ok ? "通过" : "未通过"}：技术 ${judge.technical.filter((item) => item.ok).length}/${judge.technical.length}，业务 ${judge.business.filter((item) => item.ok).length}/${judge.business.length}`);
    if (!ok) {
      for (const check of [...judge.technical, ...judge.business].filter((item) => !item.ok)) {
        console.log(`    - ${check.name}${check.detail ? `：${check.detail}` : ""}`);
      }
    }
  }
  writeJson(path.join(outDir, "run.json"), { ...run, fixturesNote: `${run.fixturesNote} 资料源是夹具；Pi host 与模型是真实执行。` });
  fs.writeFileSync(path.join(outDir, "report.md"), renderReport({ run }), "utf8");
  console.log(`\n合计 ${passed}/${run.cases.length} 通过；详细报告：${path.join(outDir, "report.md")}`);
  console.log("关键词命中不代表写作质量；人工判断项见 report.md。");
  return passed === run.cases.length ? 0 : 1;
}

const invoked = process.argv[1] ? path.resolve(process.argv[1]) : "";
if (invoked === fileURLToPath(import.meta.url)) {
  main().then(
    (code) => {
      process.exitCode = code;
    },
    (error) => {
      console.error(`pi-writing-eval：${error?.message ?? error}`);
      process.exitCode = 2;
    },
  );
}
