//! Pi Durable 写作执行。
//!
//! 生产路径只注册一个 OpenAI 兼容 provider，模型参数全部来自 init 消息；这里不读
//! `~/.pi`、不刷新模型目录、不登录、不下载技能。测试可以注入 faux provider，但只走
//! 同一套 Durable harness、工作区与 submit_document 工具。

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
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
import { createEditTool, createReadTool, createWriteTool } from "@earendil-works/pi-durable/tools";
import { WorkspaceGuard, WorkspaceEnv } from "./paths.mjs";
import { createDomainTools, createToolBridge, BridgeError } from "./bridge.mjs";
import {
  DeliveryError,
  readReceipt,
  writeDocumentReceipt,
  writeReportReceipt,
} from "./delivery.mjs";
import { REPORT_TOOL_NAME, SUBMIT_TOOL_NAME, loadSkillIndex, systemPrompt, userPrompt } from "./prompt.mjs";

const PROVIDER_ID = "gongwen";
const CANDIDATE_INTERVAL_MS = 400;

export class AgentError extends Error {
  constructor(message, code = "agent_failed") {
    super(message);
    this.name = "AgentError";
    this.code = code;
  }
}

/** 为生成保留空间，同时让 32k 等窗口也能在一次长任务中切出可压缩的历史。 */
export function writingCompaction(model) {
  const reserveTokens = Math.min(16_384, Math.floor(model.contextWindow / 2));
  return {
    enabled: true,
    reserveTokens,
    keepRecentTokens: Math.max(1, Math.min(4_096, Math.floor((model.contextWindow - reserveTokens) / 4))),
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
        resolve: async () =>
          config.model.apiKey
            ? { auth: { apiKey: config.model.apiKey }, source: "应用设置" }
            : { auth: {}, source: "无密钥（本机模型）" },
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

function createSubmitTool({ guard, limits, state, onSubmitted, emit, deliveryDir, runId, requestId }) {
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
      const read = guard.readSubmission(args.path, { maxFileBytes: limits.maxFileBytes });
      if (!read.ok) {
        throw new AgentError(`提交失败：${read.error.message}`, "submit_invalid");
      }
      const { relative, text } = read.value;
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
      });      onSubmitted?.(state.snapshot);
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
function createReportTool({ state, emit, deliveryDir, runId, requestId }) {
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
      let receipt;
      try {
        receipt = writeReportReceipt(deliveryDir, {
          runId,
          requestId,
          summary: String(args.summary ?? ""),
          items: Array.isArray(args.items) ? args.items : [],
          callId: api?.callId ?? "",
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

function createExtension({ guard, limits, state, onSubmitted, emit, bridge, deliveryDir, runId, requestId }) {
  const tools = [
    ...createDomainTools(bridge, {
      readDocument: (path) => {
        const result = guard.readSubmission(path, { maxFileBytes: limits.maxFileBytes });
        if (!result.ok) throw new BridgeError(result.error.message);
        return result.value.text;
      },
    }),
    createReadTool(),
    createWriteTool(),
    createEditTool(),
    createSubmitTool({ guard, limits, state, onSubmitted, emit, deliveryDir, runId, requestId }),
    createReportTool({ state, emit, deliveryDir, runId, requestId }),
  ];
  return defineExtension({ name: "gongwen-writing", tools });
}

/**
 * 跑一轮。返回 `{ snapshot }`（提交了）、`{ report }`（只交清单）或 `{ snapshot: undefined }`
 * （模型结束但都没交）。`onCancel` 交给 main 注册取消；取消只结束本轮，不假装完成。
 */
export async function runAgentJob({ config, models, model, emit, onCancel, onBridge }) {
  const controller = withCancel(BACKGROUND_CONTEXT);
  onCancel?.(() => controller.cancel(new AgentError("已取消", "cancelled")));

  const deliveryDir = config.workspace.delivery;
  // 恢复时先看回执：上一次已经落盘了成品，就直接交回同一份，不重跑模型。
  if (config.recovery?.resume) {
    const receipt = readReceipt(deliveryDir, config.runId);
    if (receipt) {
      if (receipt.requestId !== config.requestId) {
        throw new AgentError("交付回执的输入标识与本轮不一致，已拒绝恢复。", "receipt_request_mismatch");
      }
      emit({ type: "progress", message: "找到上次已落盘的交付回执，直接交回同一份成品。" });
      return { receipt };
    }
  }

  const guard = new WorkspaceGuard(config.workspace.root);
  const state = { snapshot: undefined, report: undefined, parked: undefined };
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
  onBridge?.(bridge);
  const candidateScan = makeCandidateScanner({ guard, state, emit, limits: config.limits });

  try {
    const built = models && model ? { models, model } : buildModels(config);
    const selectedModels = built.models;
    const selectedModel = built.model;
    const registry = createRegistry();
    registry.install(
      createExtension({
        guard,
        limits: config.limits,
        state,
        emit,
        bridge,
        deliveryDir,
        runId: config.runId,
        requestId: config.requestId,
        onSubmitted: (snapshot) => submitResolve(snapshot),
      }),
    );
    storage = await openNodeSqliteStorage(config.workspace.state);
    harness = await Harness.open(
      storage,
      {
        models: selectedModels,
        registry,
        env: ({ cwd }) =>
          new WorkspaceEnv({
            root: config.workspace.root,
            cwd: cwd ?? config.workspace.root,
            maxFileBytes: config.limits.maxFileBytes,
          }),
        settings: {
          stream: { timeoutMs: Math.min(config.limits.timeoutMs, 180_000) },
          retry: { maxRetries: 1 },
          compaction: writingCompaction(selectedModel),
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
          loadSkillIndex(config.workspace),
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
    if (config.recovery?.continuationId) {
      // 先让崩溃前的 Durable 任务收尾，防止人工补交和原任务同时生成或提交。
      await root.waitForIdle(controller.context);
      if (state.snapshot) return { snapshot: state.snapshot };
      if (state.report) return { report: state.report };
    }
    const deliveryId = config.recovery?.continuationId
      ? `${config.runId}:resume:${config.recovery.continuationId}` : config.runId;
    // 文件交付是宿主契约。模型文字结束或长度截断都不等于交付；有限续跑，仍要求显式工具提交。
    // 每次补交的 requestId 固定，崩溃恢复时重开同一个 submission，不叠加新一轮额度。
    for (let attempt = 0; attempt <= 2; attempt += 1) {
      const submission = await root.submit(
        {
          type: "input",
          content: attempt === 0 ? userPrompt(config.task) :
            "上一段已经结束或达到输出长度上限，但本轮还没有交付回执。请继续完成剩余工作，不要重写已有成稿。先读取 work/ 中实际文件；检查稿件时用 check_document({path:\"work/文件名.md\"})，不要把全文再放进工具参数。生成/修改任务必须调用 submit_document({path,summary}) 提交你确认的完整成品；仅审核/找资料用 finish_report。不要只用聊天文字说已完成。",
          requestId: attempt === 0 ? config.runId : `${deliveryId}:delivery:${attempt}`,
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
        return { snapshot: outcome.value };
      }
      if (outcome.kind === "failed") {
        throw outcome.error;
      }
      if (state.snapshot) {
        return { snapshot: state.snapshot };
      }
      if (state.report) {
        return { report: state.report };
      }
      if (outcome.value.status === "unanswered") {
        throw new AgentError(
          `模型没有完成本轮：${outcome.value.reason}${outcome.value.detail ? `（${JSON.stringify(outcome.value.detail)}）` : ""}`,
          "unanswered",
        );
      }
      if (attempt < 2) {
        emit({ type: "progress", message: `尚未交付成品，正在继续检查与提交（${attempt + 1}/2）…` });
      }
    }
    throw new AgentError(
      "本轮仍未完成成品交付，已保留工作文件与运行记录。请稍后重试；无需调整最大输出 Token。",
      "not_submitted",
    );
  } catch (error) {
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
    throw error;
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
