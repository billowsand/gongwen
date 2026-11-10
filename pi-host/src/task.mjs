//! 任务助手作业：材料整理（extract）与单任务表单填写（fill）的交付校验。
//!
//! 两种作业都由内置技能指定，交付都走同一个 `submit_task` 与 `kind = task` 回执。
//! 这里只做软件契约层校验：JSON 对象、mode 与本轮技能一致、items / fields 形状与
//! 体积上限；不判断事实是否真实，也不替模型补字段。

/** `submit_task` 的工具名；prompt 与 agent 共用，避免两处写死。 */
export const TASK_SUBMIT_TOOL_NAME = "submit_task";
/** 任务交付 JSON 的上限，与 Rust 侧 `MAX_TASK_DELIVERY_BYTES` 一致。 */
export const TASK_MAX_BYTES = 2 * 1024 * 1024;
/** 表单填写只认这六栏。 */
export const TASK_FIELDS = ["name", "owner", "progress", "next_step", "risks", "deadline"];
/** 材料整理允许的候选类型。 */
export const TASK_ITEM_KINDS = [
  "new_task",
  "progress",
  "decision",
  "opinion",
  "owner",
  "deadline",
  "risk",
  "scope",
  "completion_evidence",
];

const EXTRACT_SKILL = "gongwen-task-extract";
const FILL_SKILL = "gongwen-task-fill";

/** host 侧判定：任务作业只看技能名，与 Rust 的 `is_task_skill` 一致。 */
export function isTaskTask(task) {
  return task?.skill === EXTRACT_SKILL || task?.skill === FILL_SKILL;
}

/** 本轮任务作业的模式：`extract` / `fill`；不是任务作业时返回 `undefined`。 */
export function taskMode(task) {
  if (task?.skill === EXTRACT_SKILL) return "extract";
  if (task?.skill === FILL_SKILL) return "fill";
  return undefined;
}

function isPlainObject(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function requireEvidence(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${label}缺少原文连续摘录 evidence`);
  }
}

function validateExtract(value) {
  if (!Array.isArray(value.items)) throw new Error("材料整理结果必须包含 items 数组");
  for (const [index, item] of value.items.entries()) {
    const label = `items 第 ${index + 1} 项`;
    if (!isPlainObject(item)) throw new Error(`${label}必须是对象`);
    if (typeof item.kind !== "string" || !TASK_ITEM_KINDS.includes(item.kind)) {
      throw new Error(`${label}的 kind 必须是 ${TASK_ITEM_KINDS.join(" / ")} 之一`);
    }
    requireEvidence(item.evidence, label);
  }
  if (value.notes !== undefined && !Array.isArray(value.notes)) throw new Error("notes 必须是数组");
}

function validateFill(value) {
  if (!isPlainObject(value.fields)) throw new Error("表单填写结果必须包含 fields 对象");
  for (const key of Object.keys(value.fields)) {
    if (!TASK_FIELDS.includes(key)) throw new Error(`fields 含未知栏目 ${key}，只填 ${TASK_FIELDS.join(" / ")}`);
  }
  for (const key of TASK_FIELDS) {
    const field = value.fields[key];
    if (field === undefined || field === null) continue;
    if (!isPlainObject(field)) throw new Error(`fields.${key} 必须是 null 或 {value,evidence}`);
    if (field.value !== undefined && field.value !== null && typeof field.value !== "string") {
      throw new Error(`fields.${key}.value 必须是字符串或 null`);
    }
    requireEvidence(field.evidence, `fields.${key} `);
  }
  if (value.others !== undefined && (!Number.isSafeInteger(value.others) || value.others < 0)) {
    throw new Error("others 必须是非负整数");
  }
  if (value.notes !== undefined && !Array.isArray(value.notes)) throw new Error("notes 必须是数组");
}

/**
 * 校验一份任务交付文本。成功返回解析后的对象；失败抛普通 `Error`，由调用方包成
 * `AgentError`（`submit_task` 会回给模型具体的修复提示）。
 */
export function validateTaskPayload(text, skill) {
  const mode = taskMode({ skill });
  if (!mode) throw new Error("当前技能不是任务作业，不能交付任务 JSON");
  const source = typeof text === "string" ? text : "";
  const bytes = Buffer.byteLength(source, "utf8");
  if (bytes > TASK_MAX_BYTES) {
    throw new Error(`任务文件超过上限（${bytes} / ${TASK_MAX_BYTES} 字节）`);
  }
  let value;
  try {
    value = JSON.parse(source);
  } catch (error) {
    throw new Error(`任务文件不是合法 JSON：${error.message}`);
  }
  if (!isPlainObject(value)) throw new Error("任务文件必须是 JSON 对象");
  if (value.mode !== mode) {
    throw new Error(`任务文件的 mode 必须是 "${mode}"，收到 ${JSON.stringify(value.mode ?? null)}`);
  }
  if (mode === "extract") validateExtract(value);
  else validateFill(value);
  return value;
}
