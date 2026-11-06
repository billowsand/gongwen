//! Pi Durable 写作执行。
//!
//! 生产路径只注册一个 OpenAI 兼容 provider，模型参数全部来自 init 消息；这里不读
//! `~/.pi`、不刷新模型目录、不登录、不下载技能。测试可以注入 faux provider，但只走
//! 同一套 Durable harness、工作区与 submit_document 工具。
//!
//! 常规与分阶段共用同一个 Durable Harness 与同一份交付权限：分阶段只是宿主按预算
//! 把长任务拆成几步短任务，中间成果经 `record_progress` 由宿主核验登记，最终仍必须
//! `submit_document` / `finish_report`。这里没有第二个推理循环，也没有通用流程 DSL。

import path from "node:path";
import { createHash } from "node:crypto";
import { Type } from "@earendil-works/pi-ai";
import { createModels, createProvider } from "@earendil-works/pi-ai/models";
import { openAICompletionsApi } from "@earendil-works/pi-ai/api/openai-completions.lazy";
import { BACKGROUND_CONTEXT, withCancel } from "@earendil-works/chord/context";
import {
  Harness,
  createRegistry,
  defineExtension,
  defineTool,
  watchEvents,
} from "@earendil-works/pi-durable";
import { JsonlStorage } from "@earendil-works/pi-durable/storage/jsonl";
import { createEditTool, createWriteTool } from "@earendil-works/pi-durable/tools";
import { WorkspaceGuard, WorkspaceEnv } from "./paths.mjs";
import { ProtectedFileSystem, ProtectedFiles } from "./encrypted-files.mjs";
import { createDomainTools, createToolBridge, BridgeError } from "./bridge.mjs";
import {
  DeliveryError,
  readReceipt,
  writeDocumentReceipt,
  writeReportReceipt,
} from "./delivery.mjs";
import { REPORT_TOOL_NAME, SUBMIT_TOOL_NAME, loadSkillIndex, systemPrompt, userPrompt } from "./prompt.mjs";

/** 只拦标题中的显式编号；普通年份（如“2027 年度……”）不是编号。 */
export function findHandNumberedHeadings(prose) {
  const numberedHeading = /^#{2,6}\s+(?:附录\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?|[Aa][Pp][Pp][Ee][Nn][Dd][Ii][Xx]\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?|[一二三四五六七八九十百零]+[、.．]|[（(][一二三四五六七八九十百零\d]+[）)]|第[一二三四五六七八九十百零\d]+(?:[章节条]|部分)|\d+(?:\.\d+)+|\d+[、.．]|\d{1,3}\s+)/u;
  return prose.split("\n").filter((line) => numberedHeading.test(line)).slice(0, 12);
}
import { budgetLabel, createToolQuota, decideStrategy, readBudgetChars, taskEstimate } from "./budget.mjs";
import {
  RECORD_TOOL_NAME,
  buildPlan,
  expandSections,
  expectedUnits,
  missingUses,
  phasePrompt,
  planSummary,
  recoverPlan,
} from "./phases.mjs";
import { ProgressError, ProgressRegistry, identityOf } from "./progress.mjs";
import { createBoundedReadTool } from "./read.mjs";

import { isStyleTask, createSubmitStyleTool } from "./style.mjs";

const PROVIDER_ID = "gongwen";
const CANDIDATE_INTERVAL_MS = 400;
/** 免密服务在 SDK 要求鉴权时使用的非秘密占位值，只发往用户配置的模型地址。 */
const LOCAL_PLACEHOLDER_KEY = "lm-studio-local";

export class AgentError extends Error {
  constructor(message, code = "agent_failed") {
    super(message);
    this.name = "AgentError";
    this.code = code;
  }
}

/** 为生成保留空间，同时让 32k 等窗口也能在一次长任务中切出可压缩的历史。 */
export function writingCompaction(model, outputReserve = undefined, workBudget = model.contextWindow) {
  const budget = Math.min(model.contextWindow, workBudget);
  const reserveTokens = Number.isInteger(outputReserve) && outputReserve > 0
    ? Math.min(16_384, outputReserve)
    : Math.min(16_384, Math.floor(budget / 2));
  return {
    enabled: true,
    reserveTokens: model.contextWindow - budget + reserveTokens,
    keepRecentTokens: Math.max(1, Math.min(4_096, Math.floor((budget - reserveTokens) / 4))),
    backgroundTokens: 0,
  };
}

/** 生产 provider：地址、密钥、模型与窗口全部由 init 显式给出。 */
export function buildModels(config) {
  const model = {
    id: config.model.modelId,
    name: config.model.modelId,
    api: "openai-completions",
    provider: PROVIDER_ID,
    baseUrl: config.model.baseUrl,
    reasoning: config.model.reasoning,
    input: ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: config.model.contextWindow,
    maxTokens: config.model.maxTokens,
    compat: {
      supportsDeveloperRole: false,
      supportsReasoningEffort: config.model.reasoning,
    },
    ...(config.model.reasoning && config.model.thinkingLevel
      ? { thinkingLevelMap: { high: "high", medium: "medium", low: "low" } }
      : {}),
  };
  const provider = createProvider({
    id: PROVIDER_ID,
    name: "公文助手模型",
    baseUrl: config.model.baseUrl,
    auth: {
      apiKey: {
        name: "公文助手模型",
        // 本机免密模型不需要用户填假密钥：SDK 要求鉴权时只在本地 provider 里给占位值。
        resolve: async () =>
          config.model.apiKey
            ? { auth: { apiKey: config.model.apiKey }, source: "应用设置" }
            : { auth: { apiKey: LOCAL_PLACEHOLDER_KEY }, source: "本机模型（免密占位）" },
      },
    },
    models: [model],
    api: openAICompletionsApi(),
  });
  const models = createModels();
  models.setProvider(provider);
  return { models, model };
}

function sha256(text) {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

/** 可暂停的计时器：等用户回答时停表，回答后按剩余时间继续。 */
function makePausableTimer(ms, onExpire) {
  let remaining = ms;
  let started = Date.now();
  let paused = false;
  let cleared = false;
  let handle = setTimeout(() => {
    if (!paused && !cleared) onExpire();
  }, remaining);
  return {
    pause() {
      if (paused || cleared) return;
      paused = true;
      clearTimeout(handle);
      remaining = Math.max(0, remaining - (Date.now() - started));
    },
    resume() {
      if (!paused || cleared) return;
      paused = false;
      started = Date.now();
      handle = setTimeout(() => {
        if (!paused && !cleared) onExpire();
      }, remaining);
    },
    clear() {
      cleared = true;
      clearTimeout(handle);
    },
  };
}

function createSubmitTool({ guard, limits, state, onSubmitted, emit, deliveryDir, runId, requestId, phase, files }) {
  return defineTool({
    name: SUBMIT_TOOL_NAME,
    description:
      "提交本轮的一份完整 UTF-8 Markdown 成品。path 必须是 work/ 下的 Markdown 文件；提交内容在调用时固定，之后的文件修改不再影响已交付结果。与 finish_report 只能选一个。",
    parameters: Type.Object({
      path: Type.String({ description: "work/ 下的 Markdown 文件路径" }),
      summary: Type.String({ description: "一句话说明本轮改了什么、依据了哪些材料" }),
    }),
    execute: async (args, api, _context) => {
      if (state.report) {
        throw new AgentError("本轮已经交了侧栏清单；同一轮不能再提交正文。", "submit_duplicate");
      }
      if (phase?.task?.skill === "gongwen-extract") {
        throw new AgentError("本技能只交侧栏清单，不提交正文文件。请调用 finish_report({summary,items})，不要调用 submit_document。", "report_required");
      }
      if (phase?.phased && phase.id !== "deliver") {
        throw new AgentError(
          `现在还在分阶段写作的「${phase.title ?? phase.id}」这一步，不能提交半稿。请先写完 work/ 里的阶段成果并调用 ${RECORD_TOOL_NAME} 登记，最后一步再提交。`,
          "phase_premature_submit",
        );
      }
      if (phase?.phased && phase.id === "deliver" && !phase.report && !phase.registry?.has("final")) {
        throw new AgentError(
          "合稿成果还没有登记：请先完成完整候选稿并登记 final，再提交。不能把半稿当成品。",
          "phase_incomplete",
        );
      }
      const read = guard.readSubmission(args.path, { maxFileBytes: limits.maxFileBytes });
      if (!read.ok) {
        throw new AgentError(`提交失败：${read.error.message}`, "submit_invalid");
      }
      const { relative, text } = read.value;
      if (!phase?.task?.selection) {
        const research = phase?.task?.skill === "gongwen-research" || phase?.task?.kind === "研究报告";
        const prose = text.replace(/```[^\n]*\n[\s\S]*?```/g, "").replace(/`[^`\n]+`/g, "");
        const repair = phase.phased ? ` 修改文件后先用 record_progress 重新登记 phase=review, unit=final, path=${relative}, uses=${JSON.stringify(phase.registry.get("final")?.uses ?? [])}，再提交。` : "修改文件后重新提交。";
        const issues = [];
        if (phase?.task?.kind !== "会议纪要" && !/^#\s+[^#\s]/u.test(prose.trimStart())) issues.push("第一行必须是唯一的 # 具体题名，再写摘要/正文标记");
        const numbered = findHandNumberedHeadings(prose);
        if (numbered.length) issues.push(`标题不要手写编号（包括第几部分），删除编号并保留 # 层级和标题文字：${numbered.join("；")}`);
        const invalid = research && prose.match(/\[\^[^\]\r\n]+\](?![:：]\s*[(（])/u);
        if (invalid) {
          issues.push(`脚注语法不受应用支持：${invalid[0]}。请改为行内 [^s1]:(来源与原文摘录)，不要裸标记或文末定义；也可直接用资料题名说明出处`);
        }
        if (research) {
          let appendix = false;
          let chapters = 0;
          for (const [lineIndex, line] of prose.split("\n").entries()) {
            const marker = line.match(/^\s*<!--\s*\[([^\]]+)\]\s*-->\s*$/u)?.[1];
            if (marker && ["目录", "摘要", "正文", "附录", "附件", "版本变更记录", "参考文献"].includes(marker)) {
              appendix = marker === "附录" || marker === "附件";
              chapters = 0;
            } else if (appendix && /^##\s+[^#]/u.test(line)) {
              chapters += 1;
              if (chapters > 1) {
                issues.push(`附录区在第 ${lineIndex + 1} 行发现额外标题「${line.trim().slice(0, 100)}」；每份附录的 <!-- [附录] --> 标记必须放在对应 ## 标题之前，版本变更记录也要先写 <!-- [版本变更记录] -->。只在任务需要时添加附录或版本记录，不要为了模板而补空章节。`);
                break;
              }
            }
          }
        }
        if (issues.length) throw new AgentError(`Markdown 技术格式检查未通过：${issues.join("；")}。${repair}`, "submit_invalid");
      }
      if (phase?.phased) {
        validatePhaseDelivery(phase, guard);
        const final = phase.registry.get("final");
        if (!final || final.path !== relative || final.sha256 !== sha256(text)) {
          throw new AgentError("提交文件与已登记合稿不一致，请回到合稿阶段核验后再交付。", "phase_incomplete");
        }
      }
      const bytes = Buffer.byteLength(text, "utf8");
      if (bytes > limits.maxFileBytes) {
        throw new AgentError(`提交文件超过上限（${bytes} 字节）`, "submit_too_large");
      }
      const hash = sha256(text);
      const summary = typeof args.summary === "string" ? args.summary.slice(0, 2_000) : "";
      if (state.snapshot) {
        if (state.snapshot.path !== relative || state.snapshot.sha256 !== hash) {
          throw new AgentError("本轮已经提交过另一份成品；同一轮只认第一次提交。", "submit_duplicate");
        }
        return {
          output: [{ type: "text", text: `本轮已提交 ${relative}（${state.snapshot.bytes} 字节），以第一次提交为准。` }],
        };
      }
      // 先把内容复制到运行目录并落回执，再发事件：两个顺序之间崩溃都能用回执交回
      // 同一份产物，且绝不重读模型可写的 work/。
      let receipt;
      try {
        receipt = writeDocumentReceipt(deliveryDir, {
          runId,
          requestId,
          relative,
          content: text,
          sha256: hash,
          bytes,
          summary,
          callId: api?.callId ?? "",
          files,
        });
      } catch (error) {
        throw new AgentError(
          `提交失败：写不出交付回执（${error instanceof DeliveryError ? error.message : error?.message ?? error}）`,
          "receipt_failed",
        );
      }
      state.snapshot = { path: relative, content: text, sha256: hash, bytes, summary, receipt };
      emit({
        type: "progress",
        message: `已提交 ${relative}，等待程序检查`,
      });
      onSubmitted?.(state.snapshot);
      return {
        output: [
          {
            type: "text",
            text: `已提交 ${relative}（${bytes} 字节，sha256 ${hash.slice(0, 12)}…）。不要再修改这个文件。`,
          },
        ],
        control: { terminate: true },
      };
    },
  });
}

/** 审核 / 找资料的清单：只进侧栏，不产生正文提案。 */
function createReportTool({ state, emit, deliveryDir, runId, requestId, phase, files }) {
  return defineTool({
    name: REPORT_TOOL_NAME,
    description:
      "交一份结构化审核或资料清单（只进侧栏，不替换正文）。只在用户只要审核 / 找资料、明确不要改正文时使用；与 submit_document 只能选一个。",
    parameters: Type.Object({
      summary: Type.String({ description: "一句话说明审了什么 / 找了什么" }),
      items: Type.Array(
        Type.Object({
          title: Type.String({ description: "问题或资料名称" }),
          detail: Type.Optional(Type.String({ description: "位置、依据与建议" })),
          severity: Type.Optional(Type.String({ description: "必改 / 建议 / 提示" })),
        }),
      ),
    }),
    execute: async (args, api) => {
      if (state.snapshot) {
        throw new AgentError("本轮已经提交了正文；同一轮不能再交清单。", "report_conflict");
      }
      if (isDocumentSkill(phase?.task?.skill)) {
        throw new AgentError("当前内置技能要求交付完整正文，不能用 finish_report 代替。请将完整 Markdown 写入 work/ 文件并调用 submit_document。", "document_required");
      }
      if (phase?.phased && phase.id !== "deliver") {
        throw new AgentError(
          `现在还在分阶段写作的「${phase.title ?? phase.id}」这一步，不能提前交清单。请先完成材料提炼并登记，最后一步再交清单。`,
          "phase_premature_submit",
        );
      }
      if (phase?.phased) validatePhaseDelivery(phase, phase.guard);
      let receipt;
      try {
        receipt = writeReportReceipt(deliveryDir, {
          runId,
          requestId,
          summary: String(args.summary ?? ""),
          items: Array.isArray(args.items) ? args.items : [],
          callId: api?.callId ?? "",
          files,
        });
      } catch (error) {
        throw new AgentError(
          `交清单失败：写不出交付回执（${error instanceof DeliveryError ? error.message : error?.message ?? error}）`,
          "receipt_failed",
        );
      }
      // 事件与回执用同一份（已按写上限裁过）条目，两侧看到的一致。
      state.report = {
        summary: receipt.summary,
        items: receipt.items,
        omitted: receipt.omitted ?? 0,
      };
      emit({ type: "report", summary: state.report.summary, items: state.report.items });
      return {
        output: [
          {
            type: "text",
            text: `已交回侧栏清单（${receipt.items.length} 项${receipt.omitted ? `，另有 ${receipt.omitted} 项超出上限未带上` : ""}），正文没有改变。`,
          },
        ],
        control: { terminate: true },
      };
    },
  });
}

/** Required phase dependencies are derived from the host's plan and live registry. */
function requiredPhaseUses(phase, spec) {
  const registry = phase?.registry;
  if (!registry) return spec.uses ?? [];
  const materialUnits = registry.active()
    .filter((record) => record.kind === "distill" || record.kind === "study")
    .map((record) => record.unit);
  if (spec.kind === "outline") return materialUnits;
  if (spec.kind === "section") return ["outline", ...materialUnits];
  if (spec.kind === "final") {
    return [
      "outline",
      ...registry.active().filter((record) => record.kind === "section").map((record) => record.unit),
    ];
  }
  return spec.uses ?? [];
}

const DOCUMENT_SKILLS = new Set([
  "gongwen-draft",
  "gongwen-revise",
  "gongwen-imitate",
  "gongwen-research",
  "gongwen-reply",
]);

function isDocumentSkill(skill) {
  return DOCUMENT_SKILLS.has(skill);
}

/**
 * 中间成果登记。模型只能提交「阶段 + 单元 + 路径 + 可回查引文」，由宿主核验文件、
 * 哈希、原文定位与依赖后写入运行目录；它不能写正文，也不能代替最终提交。
 */
function createRecordTool({ guard, limits, phase, registry, emit, onFatal }) {
  const invalidByUnit = new Map();
  let invalidTotal = 0;
  const rejectRegistration = (error, unit) => {
    invalidTotal += 1;
    const key = typeof unit === "string" ? unit.slice(0, 100) : "<missing>";
    const unitCount = (invalidByUnit.get(key) ?? 0) + 1;
    invalidByUnit.set(key, unitCount);
    if (unitCount >= 3 || invalidTotal >= 8) {
      const fatal = new AgentError(
        `阶段成果登记对当前单元累计失败 ${unitCount} 次（全阶段 ${invalidTotal} 次），已停止本单元以避免重复消耗；work/ 文件已保留，可修正引文后继续。最后错误：${error?.message ?? error}`,
        "progress_retry_limit",
      );
      onFatal?.(fatal);
      throw fatal;
    }
    throw error;
  };
  return defineTool({
    name: RECORD_TOOL_NAME,
    description:
      "分阶段写作的中间成果登记。写完本步的 work/ 文件后调用；宿主会核对文件、哈希、原文引文与依赖，登记成功才算这一步完成。常规策略下不需要它。",
    parameters: Type.Object({
      phase: Type.String({ description: "当前阶段：distill / outline / sections / review" }),
      unit: Type.String({ description: "提示里给出的单元编号，逐字照抄" }),
      path: Type.String({ description: "work/ 下保存本步成果的文件" }),
      summary: Type.String({ description: "一句话说明这一步做了什么" }),
      sources: Type.Optional(
        Type.Array(
          Type.Object({
            path: Type.String({ description: "input/ 下的原文件路径" }),
            fromLine: Type.Optional(Type.Integer({ minimum: 1, description: "推荐：read(numbered:true)显示的原文行号，1开始；宿主提取原文，不再填写quote" })),
            toLine: Type.Optional(Type.Integer({ minimum: 1, description: "结束行号，含该行；不填则只取fromLine这一行" })),
            quote: Type.Optional(Type.String({ description: "兼容方式：原文里连续的一句，必须逐字一致；不要与行号同时填写" })),
          }),
          { description: "仅distill材料提炼或study样稿研究登记原文来源；outline、sections、review不要填写sources，已登记来源由宿主继承" },
        ),
      ),
      uses: Type.Optional(Type.Array(Type.String(), { description: "可选：仅填写宿主未自动覆盖的额外已登记依赖；本轮必需依赖由宿主补齐" })),
      sections: Type.Optional(Type.Array(Type.String(), { description: "提纲阶段的章节标题" })),
    }),
    execute: async (args) => {
      if (!phase?.phased) {
        throw new AgentError("本轮是常规策略，不需要分阶段登记；请直接写完整成品并提交。", "progress_unused");
      }
      const accepted = phase.spec ?? (phase.id === "deliver" && !phase.report
        ? phase.plan.phases.find((item) => item.id === "review") : undefined);
      const spec = (accepted?.units ?? []).find((unit) => unit.id === args?.unit);
      if (!spec) {
        const available = (accepted?.units ?? []).map((item) => item.id).join("、") || "（无可登记单元）";
        return rejectRegistration(new AgentError(
          `阶段成果编号「${String(args?.unit ?? "")}」不属于当前「${accepted?.phase ?? phase.id}」阶段。可用编号：${available}。请从中选一个，逐字填写 unit；文件请放在 work/ 下。`,
          "progress_invalid_unit",
        ), args?.unit);
      }
      if (!(["distill", "study"].includes(spec.kind)) && Array.isArray(args?.sources) && args.sources.length > 0) {
        return rejectRegistration(new AgentError(
          "当前阶段不需要重复登记来源。请删除 sources 字段；本步只提交 phase、unit、path、summary（提纲另填 sections），已登记的材料来源与依赖由宿主继承。",
          "progress_sources_unexpected",
        ), args?.unit);
      }
      let result;
      try {
        result = registry.record(
        {
          spec: { ...spec, uses: requiredPhaseUses(phase, spec), attempt: phase.attempt ?? 1 },
          phase: args?.phase,
          unit: args?.unit,
          file: args?.path,
          summary: args?.summary,
          sources: args?.sources,
          uses: args?.uses,
          sections: args?.sections,
        },
        {
          guard,
          workspace: phase.workspace,
          task: phase.task,
          maxFileBytes: limits.maxFileBytes,
        },
        );
      } catch (error) {
        const failure = toAgentError(error);
        let hint = `本步文件路径应是 work/ 下的文件（建议：${spec.kind === "outline" ? "work/outline.md" : spec.kind === "section" ? `work/sections/${String(spec.index).padStart(2, "0")}.md` : spec.kind === "final" ? "work/final.md" : "work/notes/本步编号.md"}）。当前可用阶段编号：${(accepted?.units ?? []).map((item) => item.id).join("、") || "（无）"}。`;
        if (Number.isInteger(spec.sourceLine) && spec.file) {
          hint = `本单元已有原文行号。请直接将 sources 改为 [{path:${JSON.stringify(spec.file)},fromLine:${spec.sourceLine},toLine:${spec.sourceLine}}]，并删除 quote；不要自行重打原句。宿主会从该原文件行提取原文，仍会检查材料范围、来源角色和哈希。`;
        } else if (spec.kind === "distill" && spec.file) {
          const opened = guard.read(spec.file, phase.workspace.root, limits.maxFileBytes);
          if (opened.ok) {
            try {
              const raw = guard.files.readText(opened.value, limits.maxFileBytes);
              const start = Math.max(0, spec.start ?? 0);
              const end = Math.min(raw.length, spec.end ?? raw.length);
              const excerpt = raw.slice(start, Math.min(end, start + 220));
              if (excerpt) {
                hint = `请从本单元允许范围逐字复制一小段作 quote；数字、日期和其中的空格也必须原样复制，不要改写。原文片段：${JSON.stringify(excerpt)}`;
              }
            } catch {
              // 只提供提示；原来的登记校验仍是唯一裁判。
            }
          }
        }
        return rejectRegistration(new AgentError(`${failure.message}。${hint}`, failure.code), args?.unit);
      }
      const status = registry.status(phase.id, accepted.allUnits ?? accepted.units);
      if (phase.id === "distill") {
        emit({ type: "progress", message: `正在提炼第 ${status.done}/${status.total} 份材料…` });
      } else {
        emit({ type: "progress", message: `已登记${spec.title ? ` ${spec.title}` : ""}，宿主核对通过` });
      }
      return {
        output: [
          {
            type: "text",
            text: JSON.stringify({
              status: result.status,
              unit: spec.id,
              path: result.record.path,
              sha256: result.record.sha256,
              phase_done: status.done,
              phase_total: status.total,
              remaining: status.missing.map((item) => item.id),
              note: "宿主已核对；本步单元完成后立即结束本轮，等待宿主发下一步，不要自行写下一阶段或提交半稿。",
            }),
          },
        ],
        control: registry.status(phase.id, accepted.units).ok ? { terminate: true } : undefined,
      };
    },
  });
}

function createExtension({
  guard,
  limits,
  state,
  onSubmitted,
  emit,
  bridge,
  deliveryDir,
  runId,
  requestId,
  phase,
  registry,
  readChars,
  onFatal,
  files,
}) {
  if (isStyleTask(phase.task)) {
    return defineExtension({name:"gongwen-writing",tools:[
      createBoundedReadTool({guard,maxFileBytes:limits.maxFileBytes,maxChars:readChars}),
      createWriteTool(),createEditTool(),createRecordTool({guard,limits,phase,registry,emit,onFatal}),
      createSubmitStyleTool({guard,limits,state,onSubmitted,deliveryDir,runId,requestId,phase,files,validateCoverage:validatePhaseDelivery}),
    ]});
  }
  const tools = [
    ...createDomainTools(bridge, {
      readChars,
      onAnswer: (answer) => {
        if (Buffer.byteLength(JSON.stringify(answer), "utf8") > 32_000) throw new AgentError("确认信息超过大小上限", "answer_too_large");
        registry.answers = [...registry.answers, answer].slice(-6);
        registry.persist();
      },
      onResult: (name, result) => registry.cacheEvidence(name, result, guard),
      readDocument: (path) => {
        const result = guard.readSubmission(path, { maxFileBytes: limits.maxFileBytes });
        if (!result.ok) throw new BridgeError(result.error.message);
        return result.value.text;
      },
    }),
    createBoundedReadTool({ guard, maxFileBytes: limits.maxFileBytes, maxChars: readChars }),
    createWriteTool(),
    createEditTool(),
    createRecordTool({ guard, limits, phase, registry, emit, onFatal }),
    createSubmitTool({ guard, limits, state, onSubmitted, emit, deliveryDir, runId, requestId, phase, files }),
    createReportTool({ state, emit, deliveryDir, runId, requestId, phase, files }),
  ];
  return defineExtension({ name: "gongwen-writing", tools });
}

function readCurrentBytes(config, files) {
  try {
    const file = path.resolve(config.workspace.root, config.workspace.current);
    const stat = files.info(file);
    return stat.kind === "file" ? stat.size : 0;
  } catch {
    return 0;
  }
}

function toAgentError(error) {
  if (error instanceof AgentError) return error;
  if (error instanceof ProgressError) {
    return new AgentError(`阶段成果登记失败：${error.message}`, error.code);
  }
  return error;
}

/**
 * 跑一轮。返回 `{ snapshot }`（提交了）、`{ report }`（只交清单）或 `{ snapshot: undefined }`
 * （模型结束但都没交）。`onCancel` 交给 main 注册取消；取消只结束本轮，不假装完成。
 */
export async function runAgentJob({ config, models, model, emit, onCancel, onBridge, files }) {
  files ??= new ProtectedFiles(config.workspace.encryption);
  const controller = withCancel(BACKGROUND_CONTEXT);
  onCancel?.(() => controller.cancel(new AgentError("已取消", "cancelled")));

  const deliveryDir = config.workspace.delivery;
  // 恢复时先看回执：上一次已经落盘了成品，就直接交回同一份，不重跑模型。
  if (config.recovery?.resume) {
    const receipt = readReceipt(deliveryDir, config.runId, files);
    if (receipt) {
      if ((receipt.kind === "style") !== isStyleTask(config.task)) {
        throw new AgentError("交付回执的产物类型与本轮任务不一致，已拒绝恢复。", "receipt_kind_mismatch");
      }
      if (receipt.requestId !== config.requestId) {
        throw new AgentError("交付回执的输入标识与本轮不一致，已拒绝恢复。", "receipt_request_mismatch");
      }
      emit({ type: "progress", message: "找到上次已落盘的交付回执，直接交回同一份成品。" });
      return { receipt };
    }
  }

  config.currentBytes = readCurrentBytes(config, files);
  const estimate = taskEstimate(config);
  let decision = decideStrategy(config, estimate);
  const guard = new WorkspaceGuard(config.workspace.root, files);
  emit({
    type: "strategy",
    mode: decision.mode,
    reasons: decision.reasons,
    window: decision.window,
    workBudget: decision.workBudget,
    materialsBudget: decision.materialsBudget,
    verified: decision.verified,
    estimate,
  });
  if (decision.mode === "insufficient") {
    throw new AgentError(
      `本次预算放不下最小写作指令与一次有效输出（窗口 ${decision.window} token）。请换用窗口更大的模型，或把材料拆成几轮。`,
      "budget_insufficient",
    );
  }

  let plan = buildPlan({ config, decision, guard });
  if (decision.mode === "phased" && plan.phases.length === 0) {
    // 局部修改 / 小任务：分阶段没有可拆的步骤，退回常规，并在事件里说明。
    decision = { ...decision, mode: "regular", reasons: [...decision.reasons, "没有可拆的阶段，按常规一轮完成"] };
  }
  if (decision.mode === "phased") {
    emit({
      type: "progress",
      message: `策略：分阶段（${decision.reasons.join("；")}；${budgetLabel(decision)}；步骤 ${planSummary(plan)}）`,
    });
  } else {
    emit({
      type: "progress",
      message: `策略：常规（${decision.reasons.join("；")}；${budgetLabel(decision)}）`,
    });
  }

  const identity = identityOf({ guard, workspace: config.workspace, task: config.task });
  let registry;
  try {
    registry = ProgressRegistry.load({
      deliveryDir,
      runId: config.runId,
      requestId: config.requestId,
      identity,
      files,
    });
    const previousMode = decision.mode;
    ({ plan, decision } = recoverPlan(plan, decision, registry));
    if (decision.mode !== previousMode) emit({ type: "strategy", ...decision });
    registry.refresh({ guard, workspace: config.workspace, task: config.task });
  } catch (error) {
    throw toAgentError(error);
  }
  if (decision.mode !== "phased" && (registry.records.length > 0 || registry.cursor?.phased)) {
    decision = { ...decision, mode: "phased", reasons: [...decision.reasons, "沿用已保存的分阶段进度"] };
    plan = buildPlan({ config, decision, guard });
    emit({ type: "strategy", ...decision });
  }
  const phase = {
    phased: decision.mode === "phased",
    id: "deliver",
    report: estimate.report,
    title: "交付",
    spec: undefined,
    attempt: 1,
    registry,
    workspace: config.workspace,
    task: config.task,
    guard,
    plan,
  };
  // Harness 一恢复就可能重放工具，必须先还原当时阶段的权限和单元。
  if (phase.phased && registry.cursor) {
    if (registry.get("outline") && plan.phases.some((item) => item.dynamic === "sections")) expandSections(plan, registry);
    const saved = plan.phases.find((item) => item.id === registry.cursor.phaseId);
    if (!saved) throw new AgentError("保存的阶段与当前计划不一致，已拒绝恢复。", "progress_identity_mismatch");
    const unit = registry.cursor.unit ? saved.units?.find((item) => item.id === registry.cursor.unit) : undefined;
    if (registry.cursor.unit && !unit) throw new AgentError("保存的材料块与当前计划不一致。", "progress_identity_mismatch");
    phase.id = saved.id;
    phase.title = saved.title;
    phase.spec = unit ? { ...saved, units: [unit], allUnits: saved.units } : saved;
  }

  const state = { snapshot: undefined, report: undefined, parked: undefined, terminalToolError: undefined };
  let submitResolve;
  const submitted = new Promise((resolve) => {
    submitResolve = resolve;
  });
  let closed = false;
  let storage;
  let harness;
  let stream;
  let timedOut = false;
  const timer = makePausableTimer(config.limits.timeoutMs, () => {
    timedOut = true;
    controller.cancel(new AgentError("运行超时", "timeout"));
  });
  const bridge = createToolBridge({
    emit,
    pause: () => timer.pause(),
    resume: () => timer.resume(),
  });
  const toolQuota = createToolQuota({ mode: decision.mode, toolBudget: decision.toolBudget, bridge });
  const grantTools = (count) => toolQuota.grant(count);
  onBridge?.(bridge);
  const candidateScan = makeCandidateScanner({ guard, state, emit, limits: config.limits });

  try {
    const built = models && model ? { models, model } : buildModels(config);
    const selectedModels = built.models;
    const selectedModel = built.model;
    const registryTools = createRegistry();
    registryTools.install(
      createExtension({
        guard,
        limits: config.limits,
        state,
        emit,
        bridge,
        deliveryDir,
        runId: config.runId,
        requestId: config.requestId,
        phase,
        registry,
        readChars: readBudgetChars(decision),
        files,
        onFatal: (error) => {
          state.terminalToolError = error;
          controller.cancel(error);
        },
        onSubmitted: (snapshot) => submitResolve(snapshot),
      }),
    );
    storage = await JsonlStorage.open(
      config.workspace.state,
      new ProtectedFileSystem({ cwd: path.dirname(config.workspace.state), files }),
      controller.context,
      { fsync: true },
    );
    harness = await Harness.open(
      storage,
      {
        models: selectedModels,
        registry: registryTools,
        env: ({ cwd }) =>
          new WorkspaceEnv({
            root: config.workspace.root,
            cwd: cwd ?? config.workspace.root,
            maxFileBytes: config.limits.maxFileBytes,
            files,
          }),
        settings: {
          stream: { timeoutMs: Math.min(config.limits.timeoutMs, 180_000) },
          retry: { maxRetries: 1 },
          compaction: writingCompaction(selectedModel, undefined, decision.workBudget),
          afterFailedRun: "withdraw",
          // 不自动无限重启：同一个任务连续被崩溃打断两次就停用，等人工决定。
          restartLimit: 2,
        },
        onReport: (error, info) => {
          if (info?.type === "task_parked") {
            state.parked = "restart_limit";
            emit({
              type: "progress",
              message: "任务连续被中断，宿主已停用自动重跑，需要人工处理。",
            });
          } else if (info?.type === "task_faulted") {
            emit({
              type: "progress",
              message: `任务失败：${String(error?.message ?? error).slice(0, 300)}`,
            });
          }
        },
      },
      controller.context,
    );

    // 继续上次进程留下的 run（未完成的生成与工具调用）。捕获异常时可能已经没有任何
    // 待跑任务，这不算失败。
    harness.resume();
    try {
      const inspection = await harness.inspect(controller.context);
      const parked = inspection.tasks.find(
        (task) => task.state?.kind === "blocked" && task.state.reason === "restart_limit",
      );
      if (parked) {
        state.parked = "restart_limit";
        throw new AgentError(
          "这次任务已经连续两次被中断，宿主停用了自动重跑。原件与运行记录都在本机：可以点「从头写」重新开一轮，或在「已中断」卡片上先「接着写」由人工确认一次。",
          "task_parked",
        );
      }
    } catch (error) {
      if (error instanceof AgentError) throw error;
      // inspect 失败不影响后续提交；真正的问题会在 wait 时暴露。
    }

    const root = await harness.root(controller.context, {
      agent: {
        model: { provider: selectedModel.provider, modelId: selectedModel.id },
        thinkingLevel: config.model.reasoning ? (config.model.thinkingLevel ?? "high") : "off",
        instructions: systemPrompt(
          config.workspace,
          config.task,
          config.recovery,
          loadSkillIndex(config.workspace, files),
          decision.mode === "phased",
        ),
        cwd: config.workspace.root,
      },
    });

    stream = await watchEvents(harness, root.id, controller.context);
    stream.start(async (events) => {
      for (const event of events) {
        if (event.type === "tool_execution_start") {
          emit({ type: "tool", name: event.toolName, message: `正在使用 ${event.toolName}` });
        } else if (event.type === "tool_execution_end") {
          if (event.toolName !== SUBMIT_TOOL_NAME && event.toolName !== REPORT_TOOL_NAME) {
            candidateScan();
          }
        } else if (event.type === "auto_retry_start") {
          emit({ type: "progress", message: `模型调用失败，正在重试（第 ${event.attempt} 次）` });
        } else if (event.type === "task_failed") {
          emit({ type: "progress", message: `任务失败：${event.message}` });
        } else if (event.type === "message_update") {
          // 不把 token 流拼成正文交给界面；进度只说明还在写。
          candidateScan.maybeTick();
        }
      }
    });

    emit({ type: "progress", message: "正在读取正文、材料与写作要求…" });
    if (config.recovery?.resume || config.recovery?.continuationId) {
      // 先让崩溃前的 Durable 任务收尾，防止人工补交和原任务同时生成或提交。
      await root.waitForIdle(controller.context);
      if (state.snapshot) return { snapshot: state.snapshot };
      if (state.report) return { report: state.report };
    }
    const deliveryId = config.recovery?.continuationId
      ? `${config.runId}:resume:${config.recovery.continuationId}` : config.runId;

    const submitPrompt = (attempt, requestPrefix, firstContent = undefined) =>
      root.submit(
        {
          type: "input",
          content: attempt > 0 && isStyleTask(config.task)
            ? "继续读取work/中的文件，完成风格JSON文件并用submit_style提交。不得使用submit_document或finish_report，不要只在聊天中说完成。"
            : attempt === 0 && firstContent
            ? firstContent
            : attempt === 0 ? userPrompt(config.task) :
            "上一段已经结束或达到输出长度上限，但本轮还没有交付回执。请继续完成剩余工作，不要重写已有成稿。先读取 work/ 中实际文件；检查稿件时用 check_document({path:\"work/文件名.md\"})，不要把全文再放进工具参数。生成/修改任务必须调用 submit_document({path,summary}) 提交你确认的完整成品；仅审核/找资料用 finish_report。不要只用聊天文字说已完成。",
          requestId: attempt === 0 ? `${requestPrefix}:first` : `${requestPrefix}:delivery:${attempt}`,
        },
        controller.context,
      );

    /** 最终交付：仍有 3 次有界补交，每次都要显式工具提交。 */
    const runDelivery = async (attempts, requestPrefix, firstContent = undefined) => {
      for (let attempt = 0; attempt <= attempts; attempt += 1) {
        if (phase.phased) {
          registry.cursor = { phased: true, phaseId: "deliver" };
          registry.persist();
        }
        const submission = await submitPrompt(attempt, requestPrefix, firstContent);
        const settledPromise = submission
          .wait(controller.context)
          .then((value) => ({ kind: "settled", value }))
          .catch((error) => ({ kind: "failed", error }));
        const submittedPromise = submitted.then((value) => ({ kind: "submitted", value }));
        const outcome = await Promise.race([settledPromise, submittedPromise]);

        if (outcome.kind === "submitted") return { snapshot: outcome.value };
        if (outcome.kind === "failed") throw outcome.error;
        if (state.snapshot) return { snapshot: state.snapshot };
        if (state.report) return { report: state.report };
        if (outcome.value.status === "unanswered") {
          throw new AgentError(
            `模型没有完成本轮：${outcome.value.reason}${outcome.value.detail ? `（${JSON.stringify(outcome.value.detail)}）` : ""}`,
            "unanswered",
          );
        }
        if (attempt < attempts) {
          emit({ type: "progress", message: `尚未交付成品，正在继续检查与提交（${attempt + 1}/${attempts}）…` });
        }
      }
      throw new AgentError(
        "本轮仍未完成成品交付，已保留工作文件与运行记录。请稍后重试；无需调整最大输出 Token。",
        "not_submitted",
      );
    };

    /** 跑一个中间阶段；最多两次纠错，仍无有效登记就停，不冒充完成。 */
    const runPhase = async (spec) => {
      phase.id = spec.id;
      phase.title = spec.title;
      phase.spec = spec;
      phase.attempt = 1;
      registry.refresh({ guard, workspace: config.workspace, task: config.task });
      // 失效后重跑不能重用 Durable 已完成的旧 submission。
      const revision = sha256(JSON.stringify(registry.records.map((record) => [record.unit, record.sha256, Boolean(record.invalid)]))).slice(0, 16);
      grantTools(spec.kindHint === "distill" ? Math.min(18, 6 + spec.units.length * 2) : 8);
      for (let attempt = 1; attempt <= 2; attempt += 1) {
        phase.attempt = attempt;
        const check = phaseStatus(spec, registry);
        if (check.ok) {
          registry.markPhase(spec.id, attempt);
          emit({ type: "progress", message: `${spec.title}已完成（${check.done}/${check.total}）` });
          return;
        }
        emit({
          type: "progress",
          message: attempt === 1
            ? `正在${spec.title}…`
            : `${spec.title}还缺 ${check.missing.length} 项，正在补齐（${attempt}/2）…`,
        });
        registry.cursor = {
          phased: true,
          phaseId: spec.id,
          unit: spec.units?.length === 1 ? spec.units[0].id : undefined,
        };
        registry.persist();
        // 每个短步骤从明确任务和已验证文件重新开始，避免旧步骤指令干扰小模型。
        await root.waitForIdle(controller.context);
        await root.reset("当前工作成果保存在 work/，接下来只执行宿主发来的新步骤。用户确认信息和必要文件由下一条任务提供。", controller.context);
        const submission = await root.submit(
          {
            type: "input",
            content: phasePrompt({
              config,
              decision,
              phase: spec,
              registry,
              plan,
              attempt,
              missing: check.missing.map((item) => item.id),
            }),
            requestId: `${deliveryId}:phase:${spec.id}:${spec.units?.[0]?.id ?? "all"}:${revision}:${attempt}`,
          },
          controller.context,
        );
        const settledPromise = submission
          .wait(controller.context)
          .then((value) => ({ kind: "settled", value }))
          .catch((error) => ({ kind: "failed", error }));
        const submittedPromise = submitted.then((value) => ({ kind: "submitted", value }));
        const outcome = await Promise.race([settledPromise, submittedPromise]);
        if (outcome.kind === "submitted") {
          throw new AgentError("分阶段写作中提前提交了成品，已拒绝。", "phase_premature_submit");
        }
        if (outcome.kind === "failed") throw outcome.error;
        if (state.report) {
          throw new AgentError("分阶段写作中提前交了清单，已拒绝。", "phase_premature_submit");
        }
        if (outcome.value.status === "unanswered") {
          throw new AgentError(
            `模型没有完成「${spec.title}」：${outcome.value.reason}`,
            "unanswered",
          );
        }
        registry.refresh({ guard, workspace: config.workspace, task: config.task });
        const after = phaseStatus(spec, registry);
        if (after.ok) {
          registry.markPhase(spec.id, attempt);
          emit({ type: "progress", message: `${spec.title}已完成（${after.done}/${after.total}）` });
          return;
        }
      }
      throw new AgentError(
        `「${spec.title}」没有登记有效成果，已暂停，不冒充完成。已登记的文件与运行记录都在本机。`,
        "phase_incomplete",
      );
    };

    const runPhases = async (startIndex = 0) => {
      for (let index = startIndex; index < plan.phases.length; index += 1) {
        const spec = plan.phases[index];
        if (spec.final) break;
        if (spec.dynamic === "sections") {
          // 提纲阶段完成后才知道有几节；这里按登记展开成逐节短任务。
          expandSections(plan, registry);
          index -= 1;
          continue;
        }
        if (spec.id === "distill") {
          for (const unit of spec.units) {
            await runPhase({ ...spec, units: [unit], allUnits: spec.units });
          }
        } else {
          await runPhase(spec);
        }
      }
    };

    if (decision.mode === "phased") {
      await runPhases(0);
      phase.id = "deliver";
      phase.title = "交付";
      phase.spec = undefined;
      grantTools(8);
      ensureReportCoverage(plan, registry, estimate);
      return await runDelivery(2, `${deliveryId}:deliver`, phasePrompt({
        config, decision, phase: plan.phases[plan.phases.length - 1], registry, plan,
      }));
    }

    // 常规策略：先按现有的一次性交付跑；没交出成品时有一次有界转换到分阶段。
    try {
      return await runDelivery(2, deliveryId);
    } catch (error) {
      const agentError = toAgentError(error);
      const convertible =
        agentError instanceof AgentError &&
        ["not_submitted", "unanswered"].includes(agentError.code) &&
        decision.preference === "auto" &&
        !estimate.selection &&
        // 只有材料能拆块时才值得转：没有材料的空转不是窗口不足，也不该靠阶段
        // 把同一件事再问一遍（否则「无成果」会被阶段循环掩盖）。
        estimate.materialTokens > 0 &&
        !state.snapshot &&
        !state.report;
      if (!convertible) throw agentError;
      const phased = { ...decision, mode: "phased", reasons: [...decision.reasons, "常规一轮没有交出成品，转为分阶段继续"] };
      plan = buildPlan({ config, decision: phased, guard });
      registry.plan = plan;
      registry.planningBudget = phased.materialsBudget;
      registry.persist();
      if (plan.phases.length === 0) throw agentError;
      decision = phased;
      toolQuota.promoteToPhased();
      phase.phased = true;
      phase.plan = plan;
      emit({
        type: "progress",
        message: `常规一轮没有交出成品，转为分阶段继续（${planSummary(plan)}）；已登记成果为 0，不重复任何已验证单元。`,
      });
      grantTools(24);
      await runPhases(0);
      phase.id = "deliver";
      phase.spec = undefined;
      grantTools(8);
      ensureReportCoverage(plan, registry, estimate);
      return await runDelivery(1, `${deliveryId}:converted`, phasePrompt({
        config, decision, phase: plan.phases[plan.phases.length - 1], registry, plan,
      }));
    }
  } catch (error) {
    if (state.terminalToolError) throw state.terminalToolError;
    if (timedOut) throw new AgentError("Pi 智能写作超时，未交付任何提案。", "timeout");
    if (controller.context.abortSignal?.aborted && !state.snapshot && !state.report) {
      throw new AgentError("已停止，未交付任何提案。", "cancelled");
    }
    if (state.parked === "restart_limit") {
      throw new AgentError(
        error?.message ??
          "这次任务连续被中断，宿主停用了自动重跑。可以点「从头写」重新开一轮。",
        "task_parked",
      );
    }
    if (error instanceof BridgeError) {
      throw new AgentError(`工具调用失败：${error.message}`, "tool_failed");
    }
    throw toAgentError(error);
  } finally {
    timer.clear();
    bridge.abortAll(new AgentError("本轮已结束", "cancelled"));
    if (!closed) {
      closed = true;
      try {
        await stream?.stop();
      } catch {
        // 监听停止失败不改变交付结论。
      }
      try {
        // 关闭会中止仍在跑的生成；提交的快照已经复制出来，不再读工作文件。
        await harness?.close(BACKGROUND_CONTEXT);
      } catch {
        // 关闭失败交给进程退出处理。
      }
      try {
        await storage?.close?.(BACKGROUND_CONTEXT);
      } catch {
        // 某些存储实现没有 close。
      }
    }
  }
}

/** 最终交付前重新核对阶段文件及依赖，登记之后改稿或改交另一份半稿都不能绕过。 */
function validatePhaseDelivery(phase, guard) {
  phase.registry.refresh({ guard, workspace: phase.workspace, task: phase.task });
  for (const spec of phase.plan.phases) {
    if (spec.final || spec.dynamic) continue;
    if (!phaseStatus(spec, phase.registry).ok) {
      throw new AgentError(`阶段成果已经失效或不完整：${spec.title}，请恢复后重新核验。`, "phase_incomplete");
    }
  }
}

/** 报告 / 提炼任务：材料提炼必须覆盖全部材料块，不能只写「都读完了」。 */
function ensureReportCoverage(plan, registry, estimate) {
  if (!estimate.report) return;
  const distill = plan.phases.find((item) => item.id === "distill");
  if (!distill) return;
  const check = phaseStatus(distill, registry);
  if (!check.ok) {
    throw new AgentError("材料提炼还没有覆盖全部材料块，不能提前交清单。", "phase_incomplete");
  }
}

/** 阶段是否完成：单元齐全，且依赖引用完整（提纲引用全部材料、合稿引用全部章节）。 */
function phaseStatus(spec, registry) {
  const expected = expectedUnits(spec, registry);
  const status = registry.status(spec.id, expected);
  const missingUsesNow = missingUses(spec, registry);
  if (missingUsesNow.length > 0) {
    const seen = new Set(status.missing.map((item) => item.id));
    for (const id of missingUsesNow) {
      if (!seen.has(id)) status.missing.push({ id });
    }
    status.ok = false;
    status.total = Math.max(status.total, status.done + status.missing.length);
  }
  return status;
}

/** 候选稿刷新：只在 work/ 里读，按内容哈希去重，按时间节流。 */
function makeCandidateScanner({ guard, state, emit, limits }) {
  let lastHash = "";
  let lastAt = 0;
  let pending = false;
  const scan = () => {
    if (state.snapshot || state.report) return;
    const candidate = guard.latestCandidate({ maxBytes: limits.maxCandidateBytes });
    if (!candidate) return;
    const hash = sha256(candidate.text);
    if (hash === lastHash) return;
    lastHash = hash;
    lastAt = Date.now();
    emit({
      type: "candidate",
      path: candidate.relative,
      content: candidate.text,
      sha256: hash,
      bytes: Buffer.byteLength(candidate.text, "utf8"),
    });
  };
  const scanner = () => {
    if (pending) return;
    const wait = Math.max(0, CANDIDATE_INTERVAL_MS - (Date.now() - lastAt));
    if (wait === 0) {
      scan();
      return;
    }
    pending = true;
    setTimeout(() => {
      pending = false;
      scan();
    }, wait);
  };
  scanner.maybeTick = () => scanner();
  return scanner;
}
