//! 任务助手作业：材料整理（extract）与单任务表单填写（fill）的交付校验。
//!
//! 两种作业都由内置技能指定，交付都走同一个 `submit_task` 与 `kind = task` 回执。
//! 这里只做软件契约层校验：JSON 对象、mode 与本轮技能一致、items / fields 形状与
//! 体积上限；不判断事实是否真实，也不替模型补字段。

/** `submit_task` 的工具名；prompt 与 agent 共用，避免两处写死。 */
export const TASK_SUBMIT_TOOL_NAME = "submit_task";
/** 任务交付 JSON 的上限，与 Rust 侧 `MAX_TASK_DELIVERY_BYTES` 一致。 */
export const TASK_MAX_BYTES = 2 * 1024 * 1024;
/** 与 Rust 的 MAX_CANDIDATES 一致。超限时让模型在提交前修正。 */
const TASK_MAX_CANDIDATES = 100;
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

export function wikiDelivery(task) {
  switch (task?.skill) {
    case "gongwen-wiki-frame": return { file: "work/frame.json", key: "themes" };
    case "gongwen-wiki-write":
    case "gongwen-wiki-update": return { file: "work/pages.json", key: "pages" };
    default: return undefined;
  }
}

/** 与 Rust 侧 wiki_jobs.rs 的确定性闸门对齐的结构性上限；超出当场打回让模型改。 */
const WIKI_LIMITS = {
  themes: 30,
  categoriesPerTheme: 20,
  nodeNameChars: 60,
  guidanceChars: 600,
  writePages: 10,
  updatePages: 5,
  titleChars: 100,
  bodyChars: 20_000,
  docRefs: 50,
  noteChars: 200,
  changeNoteChars: 200,
};

export function isJsonTask(task) {
  return isTaskTask(task) || Boolean(wikiDelivery(task));
}

function isPlainObject(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

/** 按 Unicode 码点计字数，与 Rust 的 `chars().count()` 对齐（JS `.length` 会把
 *  代理对算两个，含表情的正文会被 host 误判超限）。 */
function charLength(value) {
  return [...value].length;
}

function validateFrameGuidance(node, label) {
  if (!Object.hasOwn(node, "guidance")) return; // 兼容旧交付
  if (typeof node.guidance !== "string"
    || charLength(node.guidance.trim()) > WIKI_LIMITS.guidanceChars) {
    throw new Error(`${label}的 guidance 必须是最多 ${WIKI_LIMITS.guidanceChars} 字的文字`);
  }
}

/**
 * 维基框架交付的结构校验（与 Rust 侧 parse_frame_delivery 对齐；这里只卡结构，
 * 依据编号是否真实由 Rust 闸门判定）。每个分类必须至少挂靠一篇依据文档——
 * 真机评测里模型交过零依据的「（待定）」占位分类，必须当场打回让模型改。
 */
function validateWikiFrame(themes) {
  if (themes.length > WIKI_LIMITS.themes) {
    throw new Error(`主题超过 ${WIKI_LIMITS.themes} 个，请收窄归纳范围`);
  }
  const themeNames = new Set();
  for (const [tIndex, theme] of themes.entries()) {
    const label = `themes 第 ${tIndex + 1} 项`;
    if (!isPlainObject(theme)) throw new Error(`${label}必须是对象`);
    if (typeof theme.name !== "string" || theme.name.trim() === "") {
      throw new Error(`${label}缺少主题名 name`);
    }
    if (charLength(theme.name.trim()) > WIKI_LIMITS.nodeNameChars) {
      throw new Error(`主题名超过 ${WIKI_LIMITS.nodeNameChars} 字：${theme.name}`);
    }
    if (themeNames.has(theme.name.trim())) {
      throw new Error(`主题「${theme.name.trim()}」重复；主题名必须唯一`);
    }
    themeNames.add(theme.name.trim());
    validateFrameGuidance(theme, `主题「${theme.name}」`);
    if (!Array.isArray(theme.categories) || theme.categories.length === 0) {
      throw new Error(`主题「${theme.name}」必须包含非空的 categories 数组`);
    }
    if (theme.categories.length > WIKI_LIMITS.categoriesPerTheme) {
      throw new Error(`主题「${theme.name}」下分类超过 ${WIKI_LIMITS.categoriesPerTheme} 个，请合并同类`);
    }
    const categoryNames = new Set();
    for (const [cIndex, category] of theme.categories.entries()) {
      const clabel = `主题「${theme.name}」第 ${cIndex + 1} 个分类`;
      if (!isPlainObject(category)) throw new Error(`${clabel}必须是对象`);
      if (typeof category.name !== "string" || category.name.trim() === "") {
        throw new Error(`${clabel}缺少分类名 name`);
      }
      if (charLength(category.name.trim()) > WIKI_LIMITS.nodeNameChars) {
        throw new Error(`分类名超过 ${WIKI_LIMITS.nodeNameChars} 字：${category.name}`);
      }
      if (categoryNames.has(category.name.trim())) {
        throw new Error(`主题「${theme.name.trim()}」下分类「${category.name.trim()}」重复`);
      }
      categoryNames.add(category.name.trim());
      validateFrameGuidance(category, `分类「${category.name}」`);
      if (!Array.isArray(category.doc_ids) || category.doc_ids.length === 0
        || category.doc_ids.some((id) => !Number.isSafeInteger(id))) {
        throw new Error(`分类「${category.name}」的 doc_ids 必须是至少一个资料编号；没有依据资料的分类不要交`);
      }
      if (category.doc_ids.length > WIKI_LIMITS.docRefs) {
        throw new Error(`分类「${category.name}」依据文档超过 ${WIKI_LIMITS.docRefs} 篇`);
      }
    }
  }
}

/**
 * 维基页面交付的结构校验（与 Rust 侧 parse_page_delivery / parse_update_delivery 对齐；
 * 依据真实性由 Rust 闸门判定）。更新作业每页必须带回 uuid。
 */
function validateWikiPages(pages, { requireUuid, maxPages }) {
  if (pages.length > maxPages) {
    throw new Error(`一次交付最多 ${maxPages} 页，本轮交了 ${pages.length} 页；请收窄范围重试`);
  }
  const seenTitles = new Set();
  const seenUuids = new Set();
  let guideCount = 0;
  for (const [index, page] of pages.entries()) {
    const label = `pages 第 ${index + 1} 页`;
    if (!isPlainObject(page)) throw new Error(`${label}必须是对象`);
    if (requireUuid && (typeof page.uuid !== "string" || page.uuid.trim() === "")) {
      throw new Error(`${label}缺少目标页 uuid；更新交付每页都要带回发起时给的 uuid`);
    }
    if (!requireUuid && page.uuid !== undefined) {
      throw new Error(`${label}不应带 uuid；新建页不要带 uuid，要改已有页请走更新作业`);
    }
    if (requireUuid) {
      if (seenUuids.has(page.uuid.trim())) {
        throw new Error(`本轮交付里页面 ${page.uuid.trim()} 重复`);
      }
      seenUuids.add(page.uuid.trim());
    }
    if (typeof page.title !== "string" || page.title.trim() === "") {
      throw new Error(`${label}缺少标题 title`);
    }
    if (charLength(page.title.trim()) > WIKI_LIMITS.titleChars) {
      throw new Error(`页面标题超过 ${WIKI_LIMITS.titleChars} 字：${page.title}`);
    }
    if (seenTitles.has(page.title.trim())) {
      throw new Error(`本轮交付里页面「${page.title.trim()}」重复`);
    }
    seenTitles.add(page.title.trim());
    if (page.kind !== "normal" && page.kind !== "guide") {
      throw new Error(`${label}的 kind 必须是 normal 或 guide`);
    }
    if (page.kind === "guide") {
      guideCount += 1;
      if (guideCount > 1) throw new Error("一次交付只能有一篇导览页");
    }
    if (typeof page.body !== "string" || page.body.trim() === "") {
      throw new Error(`页面「${page.title}」缺少正文 body`);
    }
    if (charLength(page.body) > WIKI_LIMITS.bodyChars) {
      throw new Error(`页面「${page.title}」正文超过 ${WIKI_LIMITS.bodyChars} 字，请拆成多页`);
    }
    if (page.change_note !== undefined
      && (typeof page.change_note !== "string" || charLength(page.change_note.trim()) > WIKI_LIMITS.changeNoteChars)) {
      throw new Error(`页面「${page.title}」变更说明超过 ${WIKI_LIMITS.changeNoteChars} 字，请压缩到一两句话`);
    }
    if (requireUuid && (typeof page.change_note !== "string" || page.change_note.trim() === "")) {
      throw new Error(`页面「${page.title}」缺 change_note：更新交付每页都要用一两句话说清这版改了什么`);
    }
    if (!Array.isArray(page.doc_refs) || page.doc_refs.length === 0
      || page.doc_refs.length > WIKI_LIMITS.docRefs
      || page.doc_refs.some((ref) => !isPlainObject(ref) || !Number.isSafeInteger(ref.doc_id)
        || typeof ref.note !== "string" || ref.note.trim() === ""
        || charLength(ref.note) > WIKI_LIMITS.noteChars)) {
      throw new Error(`页面「${page.title}」的 doc_refs 必须是 1 至 ${WIKI_LIMITS.docRefs} 条 {doc_id, note}，note 用${WIKI_LIMITS.noteChars}字以内说明支撑要点`);
    }
    // 正文里的每个〔E编号〕都要在 doc_refs 里有对应条目（与 Rust 闸门一致）：
    // 真机评测里模型正文标了 〔E3〕 却漏登记依据，跑完才被拒收。
    const refIds = new Set(page.doc_refs.map((ref) => ref.doc_id));
    for (const match of page.body.matchAll(/〔E(\d+)〕/g)) {
      if (!refIds.has(Number(match[1]))) {
        throw new Error(`页面「${page.title}」正文标记 〔E${match[1]}〕 在 doc_refs 里没有对应条目；补上该文档的依据，或删掉这个标记`);
      }
    }
  }
}

function requireEvidence(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${label}缺少原文连续摘录 evidence`);
  }
}

function validateExtract(value) {
  if (!Array.isArray(value.items)) throw new Error("材料整理结果必须包含 items 数组");
  if (value.items.length > TASK_MAX_CANDIDATES) {
    throw new Error(`items 最多 ${TASK_MAX_CANDIDATES} 条，请合并重复事项`);
  }
  for (const [index, item] of value.items.entries()) {
    const label = `items 第 ${index + 1} 项`;
    if (!isPlainObject(item)) throw new Error(`${label}必须是对象`);
    if (typeof item.kind !== "string" || !TASK_ITEM_KINDS.includes(item.kind)) {
      throw new Error(`${label}的 kind 必须是 ${TASK_ITEM_KINDS.join(" / ")} 之一`);
    }
    requireEvidence(item.evidence, label);
  }
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
    if (typeof field.value !== "string" || field.value.trim() === "") {
      throw new Error(`fields.${key}.value 必须是非空字符串，无值时将该栏写成 null`);
    }
    requireEvidence(field.evidence, `fields.${key} `);
  }
  if (value.others !== undefined && (!Number.isSafeInteger(value.others) || value.others < 0 || value.others > TASK_MAX_CANDIDATES)) {
    throw new Error(`others 必须是 0 至 ${TASK_MAX_CANDIDATES} 的整数`);
  }
}

/**
 * 校验一份任务交付文本。成功返回解析后的对象；失败抛普通 `Error`，由调用方包成
 * `AgentError`（`submit_task` 会回给模型具体的修复提示）。
 */
export function validateTaskPayload(text, skill) {
  const mode = taskMode({ skill });
  const wiki = wikiDelivery({ skill });
  if (!mode && !wiki) throw new Error("当前技能不属于 JSON 交付任务作业");
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
  if (wiki) {
    if (!Array.isArray(value[wiki.key]) || value[wiki.key].length === 0) {
      throw new Error(`维基交付必须包含非空的 ${wiki.key} 数组`);
    }
    if (wiki.key === "themes") validateWikiFrame(value.themes);
    else validateWikiPages(value.pages, {
      requireUuid: skill === "gongwen-wiki-update",
      maxPages: skill === "gongwen-wiki-update" ? WIKI_LIMITS.updatePages : WIKI_LIMITS.writePages,
    });
    return value;
  }
  if (value.mode !== mode) {
    throw new Error(`任务文件的 mode 必须是 "${mode}"，收到 ${JSON.stringify(value.mode ?? null)}`);
  }
  if (value.notes !== undefined && (!Array.isArray(value.notes) || value.notes.some(note => typeof note !== "string"))) {
    throw new Error("notes 必须是字符串数组");
  }
  if (mode === "extract") validateExtract(value);
  else validateFill(value);
  return value;
}
