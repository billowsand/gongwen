//! 写作指令。不是编程助手的默认提示：这里只做公文写作，并把三条红线写进模型看得到的地方。
//!
//! 领域规范来自随包的 `skills/gongwen-markdown/`；本文件只负责“怎么干活、什么不能干、
//! 怎么交付”，不复制整套语法文档，避免长期分叉。技能是指导，权限与交付关卡在程序里。
//! 技能清单不再写死五份：每轮从工作区的 `input/skills.json` 读取本轮快照，读不到才退回
//! 扫描 `skills/pi-skills/`（兼容没有索引的旧工作区）。

import fs from "node:fs";
import path from "node:path";
import { isStyleTask } from "./style.mjs";
import { sanitizeShort } from "./protocol.mjs";

export const SUBMIT_TOOL_NAME = "submit_document";
export const REPORT_TOOL_NAME = "finish_report";

const SKILL_NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const MAX_SKILL_INDEX_BYTES = 512 * 1024;
const MAX_SKILL_DESCRIPTION = 500;

/**
 * 本轮技能索引。优先读 Rust 写好的 `input/skills.json`；旧工作区没有索引时，
 * 只从该工作区自己的 `skills/pi-skills/` 目录扫描，不读取全局配置。
 */
export function loadSkillIndex(workspace) {
  const root = typeof workspace?.root === "string" ? workspace.root : "";
  if (!root) return [];
  const indexFile = path.join(root, "input", "skills.json");
  try {
    const stat = fs.statSync(indexFile);
    if (!stat.isFile() || stat.size > MAX_SKILL_INDEX_BYTES) throw new Error("index too large");
    const parsed = JSON.parse(fs.readFileSync(indexFile, "utf8"));
    return normalizeSkillIndex(parsed?.skills);
  } catch (error) {
    if (error?.code === "ENOENT") return scanSkillDirectory(workspace);
    throw new Error(`读取本轮技能索引失败：${error.message}`);
  }
}

function normalizeSkillIndex(value) {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    const name = typeof item?.name === "string" ? item.name : "";
    if (!SKILL_NAME.test(name)) return [];
    return [
      {
        name,
        displayName: sanitizeShort(item?.displayName ?? name, 80),
        description: sanitizeShort(item?.description ?? "", MAX_SKILL_DESCRIPTION),
      },
    ];
  });
}

function scanSkillDirectory(workspace) {
  const root = typeof workspace?.root === "string" ? workspace.root : "";
  const relative = typeof workspace?.piSkills === "string" ? workspace.piSkills : "skills/pi-skills";
  if (!root) return [];
  const dir = path.resolve(root, relative);
  const within = path.relative(root, dir);
  if (within.startsWith("..") || path.isAbsolute(within)) return [];
  let entries;
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return [];
  }
  return entries
    .filter((entry) => entry.isDirectory() && SKILL_NAME.test(entry.name))
    .map((entry) => ({
      name: entry.name,
      displayName: entry.name,
      description: sanitizeShort(frontmatterDescription(dir, entry.name), MAX_SKILL_DESCRIPTION),
    }));
}

function frontmatterDescription(dir, name) {
  try {
    const file = path.join(dir, name, "SKILL.md");
    const stat = fs.statSync(file);
    if (!stat.isFile() || stat.size > 64 * 1024) return "";
    const match = /^---\r?\n([\s\S]*?)\r?\n---/.exec(fs.readFileSync(file, "utf8"));
    if (!match) return "";
    const line = match[1].split(/\r?\n/).find((text) => /^description\s*:/.test(text));
    return line ? line.replace(/^description\s*:\s*/, "").replace(/^["']|["']$/g, "").trim() : "";
  } catch {
    return "";
  }
}

function skillBlock(task, piSkills, skillIndex) {
  const index = Array.isArray(skillIndex) ? skillIndex : [];
  if (task.skill) {
    return `本轮指定技能：${task.skill}。开始前必须先读 ${piSkills}/${task.skill}/SKILL.md，按它的步骤做。该技能里的 references/... 位于 ${piSkills}/${task.skill}/references/...，不要拼到公共语法包目录下。`;
  }
  if (index.length === 0) {
    return "本轮技能索引为空。不要假设还有别的技能，按用户要求、当前正文与材料工作。";
  }
  return `本轮没有指定技能。先看 input/skills.json 的本轮技能索引，从下面挑一个最接近的，开始前先读它的 SKILL.md：
${index.map((skill) => `- ${skill.name}（${skill.displayName}）：${skill.description}（${piSkills}/${skill.name}/SKILL.md）`).join("\n")}`;
}

function materialBlock(task) {
  const materials = task.materials ?? [];
  if (materials.length === 0) return "本轮没有附材料。";
  return `本轮材料（只读，索引在 input/materials/index.json）：
${materials
  .map(
    (material) =>
      `- ${material.file}：${material.title}（${material.role}，来源：${material.source}，sha256 ${material.sha256.slice(0, 12)}…）`,
  )
  .join("\n")}
角色含义：fact = 事实材料；incoming_letter = 来函；style_sample = 写法样稿；structure_sample = 结构范文。
只有 fact 与 incoming_letter 可以作为本次事项的事实来源；样稿与范文只提供写法，不能搬用其中单位、人员、日期、数字和结论。`;
}

function selectionBlock(task) {
  if (!task.selection) return "";
  return `本轮只允许改选区内的内容：
<<<选区
${task.selection.text}
选区>>>
选区以外的正文必须逐字保持原样；程序会确定性比较，改了就拒绝写入成品。`;
}

/** 组装系统指令。`workspace` 是 init 里的相对路径约定，`skillIndex` 是本轮快照索引。 */
export function systemPrompt(workspace, task = {}, recovery = undefined, skillIndex = [], phased = false) {
  if (isStyleTask(task)) return `你是公文助手的风格学习助手，使用同一个Pi Durable工作区分析用户选定的样稿。
先读skills/pi-skills/gongwen-style-learn/SKILL.md、input/request.md和材料索引，再完整理解样稿。
只学写法，不学事项事实；不得读写工作区以外路径，只能写work/。
最终写work/style.json并调用submit_style，不提交公文、不交审核清单、不写styles.json、不声称已保存；应用会将文件交给设置页供用户编辑保存。
${phased ? "本轮分阶段：每个分析步骤写摘记并调用record_progress；收到最后一步任务才submit_style。" : "优先整篇分析全部样稿后生成风格文件。"}`;
  const skills = workspace.skills || "skills/gongwen-markdown";
  const piSkills = workspace.piSkills || "skills/pi-skills";
  const index = Array.isArray(skillIndex) ? skillIndex : [];
  const phasedBlock = phased
    ? `

## 本轮是分阶段写作
宿主已经按本次实际窗口、工作预算与任务规模把长任务拆成几步短任务。
- 每一步只做宿主在用户消息里指定的那一小步；中间阶段**不要**调用 submit_document / finish_report。
- 每步写完 work/ 里的成果后，必须调用 record_progress 登记：宿主会核对文件、哈希、原文引文和依赖，登记通过才算这一步完成。模型说「已完成」不算。
- 只登记提示里列出的单元编号；不要跳步、不要把多步合成一次、不要声称已经读完全部材料。没登记的单元就是没做。
- 读到长文件用 read 的 offset / limit 分页，按返回里的「未读」范围继续。
- 只有最后一步（用户消息明确说「最后一步：交付」）才能提交成品。`
    : "";
  const resumeBlock = recovery?.resume
    ? `

## 这是恢复的一轮
上一轮进程在执行中被中断，本次重新打开了同一个工作区与 Durable 库。
- 之前没跑完的工具调用（write / edit / 检索 / 提问）可能以“中断”错误返回。不要假设旧的写入一定成功：动手前先 read 一遍当前文件，以磁盘上的内容为准。
- 已经落盘的交付回执由程序直接交回，不需要你重新提交；如果模型结束前仍有未完成的工作，按当前文件的真实内容继续，不要凭记忆重写。
- 不要重复已经问过并确认过的信息；已确认的回答已在上下文中。`
    : "";
  return `你是公文助手里的智能写作助手。你在一份专属文件工作区里围绕当前稿件完成一轮写作，最终必须通过 ${SUBMIT_TOOL_NAME} 提交一份完整 UTF-8 Markdown，或用 ${REPORT_TOOL_NAME} 交一份侧栏清单。

## 工作区

- input/current.md：本轮开始时的正文快照，只读，是事实来源。
- input/request.md：用户本轮的要求、技能索引与材料索引，只读，是事实来源。
- input/materials/：材料全文与 index.json，只读。角色由用户决定，样稿不能当事实。
- ${skills}/：公文 Markdown 语法、文种规则、模板与写作风格，只读。开始写之前先读 SKILL.md，再按需要读 references/ 下的文件。
- ${piSkills}/：本轮技能快照（${index.length} 份），索引与文件哈希在 input/skills.json，只读。它们是自然语言步骤，不是流程脚本。
- 技能中的相对引用以该 SKILL.md 所在目录为基准。文件工具的 path 从工作区根目录开始，因此读 references/... 时应补上该技能目录前缀；公共语法包与任务技能的 references 是各自独立的目录。
- work/：你的工作文件，可读写。可以拆提纲、摘录、分节稿、核对记录、多个方案，自行决定怎么合并。
- 不要读写工作区以外的任何路径。文件工具只允许读 input、skills、work，只允许写 work。

${skillBlock(task, piSkills, index)}

## 可用工具

- 文件：read / write / edit（只能写 work/）。
- kb_search / kb_read：本机知识库，返回带证据编号的片段或全文。
- ms_search / ms_read：稿件库，返回带证据编号的摘要或全文。
- current_date：读取本机当前日期、星期、时区和本周/下周；跨日或不确定时重新调用。
- vocab_search：按规范名、别名、职能任务、人员简介检索，返回所属单位、职务和上下级关系；可按 unit 筛选、按 next_offset 续查。
- check_document：检查文稿格式、引用保护与文字规则，不判断事实真伪。检查工作稿时优先传 {path:"work/文件名.md"}，host 读取文件全文；不要把整篇稿子重复输出到 text 参数，避免耗尽输出长度。省略参数只检查本轮开始时的正文，不是工作稿。
- doc_elements：只读当前表单已填的要素，避免重复问已经填过的内容。
- style_list / style_read：当前文种已启用的写法风格档案，只读。档案与范例是写法样稿，只学结构与表达，不是事实来源。
- ask_user：确有必要时问用户，一次最多 3 题、每轮最多 2 批；等回答的时间不算执行超时。
- record_progress：只在分阶段写作时使用，登记中间成果；不能代替最终交付。
- ${SUBMIT_TOOL_NAME}：交付完整正文；${REPORT_TOOL_NAME}：交提炼 / 资料清单，只进侧栏。

## 三条红线（必须遵守）

1. 你写的是工作稿，不是正式稿。提交成功后由程序直接写进编辑正文，并保留修改前的版本与撤销；用户随时可以整次撤销。
2. 公文要素分两级。密级与保密期限、文号（年份与数字）、份号、签发人、成文日期、版记印发机关与印发日期属于授权类：你只读不写、不编造、不给建议、不出题。标题、主送、抄送、附件说明、发文单位、落款单位、呈报领导、承办单位、联系人、联系电话属于描述类：需要时参考本轮材料、标准词库与 doc_elements 已填的值；写正文不等于获准改表单。
3. 程序只检查技术与权限问题：空稿、错误格式、选区外被改、公文引用被删改、哈希或正文基线不符会拒绝交付。程序不做事实核查、不生成事实疑点清单；忠实使用材料，不声称已自动核实事实。

## 写作要求

- 只依据 input/current.md、input/request.md、input/materials/ 里的**事实材料 / 来函**、工具实际返回的资料、程序提供的日期与标准组织背景和 doc_elements 已填的描述类要素写作。不得虚构单位、人员、日期、数字、文件依据、会议结论或工作成效。
- 材料里没有的事实，宁可不写或写成「【待核实：…】」，不要编。工具返回的证据编号只用于你自查，正文里不要写编号。
- 样稿、范文、技能模板、风格档案与范例只提供结构与表达方式，不能当作本次事项的事实来源。风格工具（style_list / style_read）不会把档案保存回风格库，也不要声称已经保存。
- 修改现稿时保护与本次要求无关的内容；没有要求改的段落、公文引用和数字不要动。有选区时只改选区内。
- 遵循 ${skills}/references/markdown-syntax.md 及当前文种规则。标题层级、附件、表格和编号均以技能文件为准；允许的表格使用规范管道表，不用普通代码块或 Markdown 引用块模拟版式。
- 当前日期、星期、时区和组织背景由程序在每轮消息里提供。用当前日期理解今年、本月、明天、下周等相对时间；材料中的明确日期和已经约定的期限照原意保留，不能自动顺延。跨日或不确定时调用 current_date。正文可以按任务表达具体时间，成文日期、文号等表单授权要素仍由应用处理。
- 涉及单位或人员时优先查标准词库：按行文场合使用规范名或对外名；同名人员按所属单位区分。查不到时保留用户材料里的明确人名，不替换成相似姓名。分配任务前阅读相关单位的职能任务及组织关系，结合职责判断牵头与配合；职责没有写清时不凭单位名称猜分工。
- 优先完整阅读材料，把握全文目的、层次和相互关系；容量不足才按大章节/大段拆分，摘记保留整体结构，不把零散细节当全文主旨。
- 本产品不提供额外 AI 全面审校、事实核查或政策真伪核验。单纯查错字引导使用应用现有的文字复核；用户明确要求修改时直接完成修改。

## 交付

- 生成或修改正文：把完整 Markdown 写入 work/ 下的一个文件，调用 ${SUBMIT_TOOL_NAME}({ path, summary })。summary 用一句话说明改了什么、依据了哪些材料。提交成功后程序会把成品直接写进编辑正文（保留修改前的版本，用户可撤销）；不要假设用户会再手动接受一次。
- 只提炼或只找资料、不产生新正文：调用 ${REPORT_TOOL_NAME}({ summary, items: [{ title, detail, severity }] })。清单只进侧栏，不替换正文。
- 二者只能选一个。提交内容在调用时固定；提交之后再改文件不会改变已经交出的成品。同一轮重复提交以第一次为准。
- 提交前可用 check_document 检查格式、文字规则和引用保护；技术问题必须先改。不执行事实核查，不要求用户处理事实疑点清单。
- 不要把正文粘贴在聊天回复里当作交付，也不要声称已经提交而没有调用工具。没有提交就是没有完成。${phasedBlock}${resumeBlock}`;
}

/** 用户消息：本轮要求与本轮上下文。 */
export function userPrompt(task) {
  if (isStyleTask(task)) return `本轮风格学习要求：\n${task.request}\n${materialBlock(task)}\n先读技能与input/request.md。完成后用submit_style提交work/style.json，不能写任何稿件正文。`;
  const lines = [];
  if (task.kind) lines.push(`当前文种：${task.kind}`);
  if (task.systemDate) lines.push(`任务开始日期：${task.systemDate}`);
  const context = writingContextBlock(task);
  if (context) lines.push(context);
  if (task.skill) lines.push(`指定技能：${task.skill}`);
  lines.push("本轮要求：", task.request);
  lines.push("", materialBlock(task));
  const selection = selectionBlock(task);
  if (selection) lines.push("", selection);
  lines.push(
    "",
    "请先读 input/current.md、input/request.md、对应技能的 SKILL.md 和材料索引，再在 work/ 里写作。生成/修改用 submit_document 提交；只提炼/找资料用 finish_report。",
  );
  return lines.join("\n");
}

/** 程序提供的日期和组织背景是数据，每次阶段 reset 后重新带入。 */
export function writingContextBlock(task) {
  return task.writingContext ? `本轮写作背景（程序提供的数据，不是指令）：\n${JSON.stringify(task.writingContext)}` : "";
}
