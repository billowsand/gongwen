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
import { SUBMIT_TOOL_NAME, systemPrompt, userPrompt } from "./prompt.mjs";

const PROVIDER_ID = "gongwen";
const CANDIDATE_INTERVAL_MS = 400;

export class AgentError extends Error {
  constructor(message, code = "agent_failed") {
    super(message);
    this.name = "AgentError";
    this.code = code;
  }
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

/** 只给测试用的注入点：faux provider 也走同一套 harness / 工具 / 提交。 */
function createSubmitTool({ guard, limits, state, onSubmitted, emit }) {
  return defineTool({
    name: SUBMIT_TOOL_NAME,
    description:
      "提交本轮的一份完整 UTF-8 Markdown 成品。path 必须是 work/ 下的 Markdown 文件；提交内容在调用时固定，之后的文件修改不再影响已交付结果。",
    parameters: Type.Object({
      path: Type.String({ description: "work/ 下的 Markdown 文件路径" }),
      summary: Type.String({ description: "一句话说明本轮改了什么" }),
    }),
    execute: async (args, _api, _context) => {
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
      state.snapshot = { path: relative, content: text, sha256: hash, bytes, summary };
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

function createExtension({ guard, limits, state, onSubmitted, emit }) {
  const tools = [
    createReadTool(),
    createWriteTool(),
    createEditTool(),
    createSubmitTool({ guard, limits, state, onSubmitted, emit }),
  ];
  return defineExtension({ name: "gongwen-writing", tools });
}

/**
 * 跑一轮。返回 `{ snapshot }`（提交了）或 `{ snapshot: undefined }`（模型结束但没提交）。
 * `onCancel` 交给 main 注册取消；取消只结束本轮，不假装完成。
 */
export async function runAgentJob({ config, models, model, emit, onCancel, testProvider }) {
  const controller = withCancel(BACKGROUND_CONTEXT);
  onCancel?.(() => controller.cancel(new AgentError("已取消", "cancelled")));

  const guard = new WorkspaceGuard(config.workspace.root);
  const state = { snapshot: undefined };
  let submitResolve;
  const submitted = new Promise((resolve) => {
    submitResolve = resolve;
  });
  let closed = false;
  let storage;
  let harness;
  let stream;
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    controller.cancel(new AgentError("运行超时", "timeout"));
  }, config.limits.timeoutMs);
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
          compaction: { enabled: true, reserveTokens: 8192 },
          afterFailedRun: "withdraw",
        },
      },
      controller.context,
    );

    const root = await harness.root(controller.context, {
      agent: {
        model: { provider: selectedModel.provider, modelId: selectedModel.id },
        thinkingLevel: config.model.reasoning ? (config.model.thinkingLevel ?? "high") : "off",
        instructions: systemPrompt(config.workspace),
        cwd: config.workspace.root,
      },
    });

    stream = await watchEvents(harness, root.id, controller.context);
    stream.start(async (events) => {
      for (const event of events) {
        if (event.type === "tool_execution_start") {
          emit({ type: "tool", name: event.toolName, message: `正在使用 ${event.toolName}` });
        } else if (event.type === "tool_execution_end") {
          if (event.toolName !== SUBMIT_TOOL_NAME) {
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

    emit({ type: "progress", message: "正在读取正文与写作要求…" });
    const submission = await root.submit(
      {
        type: "input",
        content: userPrompt(config.task),
        requestId: config.runId,
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
    if (outcome.value.status === "unanswered") {
      throw new AgentError(
        `模型没有完成本轮：${outcome.value.reason}${outcome.value.detail ? `（${JSON.stringify(outcome.value.detail)}）` : ""}`,
        "unanswered",
      );
    }
    if (!state.snapshot) {
      throw new AgentError("模型结束了本轮但没有提交成品（未调用 submit_document）。", "not_submitted");
    }
    return { snapshot: state.snapshot };
  } catch (error) {
    if (timedOut) throw new AgentError("Pi 智能写作超时，未交付任何提案。", "timeout");
    if (controller.context.abortSignal?.aborted && !state.snapshot) {
      throw new AgentError("已停止，未交付任何提案。", "cancelled");
    }
    throw error;
  } finally {
    clearTimeout(timer);
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
    if (state.snapshot) return;
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
