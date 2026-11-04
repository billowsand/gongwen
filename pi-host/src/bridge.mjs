//! 工具桥：host 把领域工具与提问转成 `tool_request` / `questions`，按 `callId` 等
//! Rust 的 `tool_result`。不对外开 MCP 服务；工具名与次数由这里和 Rust 双重限制。

import { Type } from "@earendil-works/pi-ai";
import { defineTool } from "@earendil-works/pi-durable";
import { outputLimitsForChars } from "./read.mjs";

/** 一轮最多多少次工具调用（与 Rust 上限一致）。 */
export const MAX_TOOL_CALLS = 60;
export const MAX_QUESTION_BATCHES = 2;
export const MAX_QUESTIONS = 3;
/**
 * 同时挂着的请求上限：Rust 侧一次只接一个 `tool_request`，其余在桥里排队。
 * 排队也计入预算，所以这里只是把 `Map` 的规模封死，正常永远到不了。
 */
export const MAX_PENDING_REQUESTS = MAX_TOOL_CALLS + MAX_QUESTION_BATCHES;

/** 领域工具的名字与 schema。Rust 还会再校验一遍。 */
export const DOMAIN_TOOLS = [
  ["kb_search", "检索本机知识库，返回带证据编号的相关片段", Type.Object({
    query: Type.String({ description: "一句话说明要找什么" }),
    top: Type.Optional(Type.Number({ description: "最多返回几段，1–10" })),
  })],
  ["kb_read", "读本机知识库的一篇文档全文", Type.Object({
    doc_id: Type.Number({ description: "kb_search 返回的 doc_id" }),
    offset: Type.Optional(Type.Integer({ minimum: 0, description: "从字符位置继续读取；用上次返回的 next_offset，不是字节或行号" })),
  })],
  ["ms_search", "检索稿件库（标题 / 文号 / 备注）", Type.Object({
    query: Type.String({ description: "检索词" }),
    limit: Type.Optional(Type.Number({ description: "最多几篇，1–10" })),
  })],
  ["ms_read", "读稿件库的一篇稿件（可指定版本）", Type.Object({
    id: Type.Number({ description: "稿件编号" }),
    version: Type.Optional(Type.Number({ description: "历史版本号；省略读当前版本" })),
    offset: Type.Optional(Type.Integer({ minimum: 0, description: "从字符位置继续读取；用上次返回的 next_offset，不是字节或行号" })),
  })],
  ["api_search", "在内网数据接口里按用途搜索只读接口，返回接口 id、说明与参数签名", Type.Object({
    query: Type.String({ description: "要找的数据，例如「森林火灾统计」「政策文件」" }),
    limit: Type.Optional(Type.Number({ description: "最多返回几个，1–5" })),
  })],
  ["api_call", "调用一个只读内网数据接口取数据；api 填接口 id，args 只放接口签名里的参数", Type.Object({
    api: Type.String({ description: "api_search 返回的接口 id" }),
    args: Type.Optional(Type.Record(Type.String(), Type.Any(), {
      description: "接口参数，按 api_search 返回的参数签名填；没有参数就省略",
    })),
  })],
  ["current_date", "读取本机当前日期、星期、时区及本周下周范围", Type.Object({})],
  ["vocab_search", "查标准单位名称、职能任务、人员职务简介及所属单位", Type.Object({
    query: Type.Optional(Type.String({ description: "名称、编码、职能或人员简介关键词；留空按类别列出" })),
    unit: Type.Optional(Type.String({ description: "按所属单位筛选人员或下属单位，优先用查询返回的单位 identity" })),
    offset: Type.Optional(Type.Integer({ minimum: 0, description: "继续查询使用 next_offset" })),
    limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 40 })),
    category: Type.Optional(Type.String({ description: "unit / person，省略为不限" })),
  })],
  ["check_document", "检查文稿格式、公文引用保护、要素和文字规则（不做事实核查）", Type.Object({
    path: Type.Optional(Type.String({ description: "优先使用：work/ 下待检查的 Markdown 文件路径；host 读取全文，不要重复生成全文参数" })),
    text: Type.Optional(Type.String({ description: "要检查的 Markdown；省略检查本轮正文" })),
  })],
  ["doc_elements", "只读当前表单已填的公文要素（授权类只读）", Type.Object({})],
  ["style_list", "列出当前文种已启用的写法风格档案（只读；档案与范例是写法样稿，不是事实来源）", Type.Object({})],
  ["style_read", "按 id 读一份写法风格档案（只读；只学写法，不搬用其中单位、人员、日期、数字与结论）", Type.Object({
    id: Type.String({ description: "style_list 返回的档案 id" }),
  })],
];

export class BridgeError extends Error {
  constructor(message) {
    super(message);
    this.name = "BridgeError";
  }
}

/**
 * 建立工具桥。`emit` 只写协议；`pause/resume` 用于把等用户回答的时间从执行超时里扣除。
 *
 * 模型可以在同一轮里批量发起工具调用，而 Rust 侧只有 `pending_tool` 一个槽位：
 * 桥把 `call` / `ask` 放进同一条有界 FIFO，任何时候最多只有一个请求已经发出、
 * 正在等 `tool_result`，收到匹配的回复再发下一个。排队中的请求不占 Rust 的槽位，
 * 但计入工具次数 / 提问批次预算。
 */
export function createToolBridge({ emit, pause, resume, maxCalls = MAX_TOOL_CALLS }) {
  /** callId → waiter；包含已发出和排队中的请求。 */
  const pending = new Map();
  /** 还没发出的 waiter，FIFO。 */
  const queue = [];
  /** 已发出、正在等回复的 waiter。 */
  let active = null;
  let nextCall = 1;
  let calls = 0;
  let batches = 0;
  /** 本轮的绝对工具额度；分阶段时按阶段增量上调，但总上限仍由宿主封顶。 */
  let callLimit = Number.isInteger(maxCalls) && maxCalls > 0 ? maxCalls : MAX_TOOL_CALLS;
  /** 取消 / 进程结束后不再发协议、也不再接纳新请求。 */
  let closed = false;
  /** `pump` 防重入：`emit` 里同步回来的回复不能把调度栈搅乱。 */
  let pumping = false;

  /** 结束一个 waiter：只生效一次，必要时恰好恢复一次计时。 */
  const settle = (waiter, ok, value) => {
    if (waiter.settled) return false;
    waiter.settled = true;
    pending.delete(waiter.callId);
    if (active === waiter) active = null;
    if (waiter.paused) {
      waiter.paused = false;
      try {
        resume?.();
      } catch {
        // 计时器出问题也不能把队列卡住。
      }
    }
    if (ok) waiter.resolve(value);
    else waiter.reject(value);
    return true;
  };

  /** 队列空着且没有在途请求时，发出队首那一个。 */
  const pump = () => {
    if (pumping) return;
    pumping = true;
    try {
      while (!closed && active === null && queue.length > 0) {
        const waiter = queue.shift();
        if (waiter.settled || pending.get(waiter.callId) !== waiter) continue;
        active = waiter;
        try {
          // 只有真正发出提问时才暂停计时；排队期间照常算工具执行时间。
          if (waiter.type === "questions") {
            pause?.();
            waiter.paused = true;
          }
          emit({ type: waiter.type, callId: waiter.callId, ...waiter.payload });
        } catch (error) {
          // 写给 Rust 失败只拒绝这一条，后面的继续排队；`settle` 会恢复计时。
          settle(waiter, false, error);
          continue;
        }
        // `emit` 里若同步回来了回复，waiter 已结束、active 已清空，循环接着发下一个。
      }
    } finally {
      pumping = false;
    }
  };

  /** 入队并尝试立即发出；所有请求共用这一条 FIFO。 */
  const enqueue = (type, payload) => {
    const callId = `c${nextCall++}`;
    return new Promise((resolve, reject) => {
      const waiter = { callId, type, payload, resolve, reject, settled: false, paused: false };
      pending.set(callId, waiter);
      queue.push(waiter);
      pump();
    });
  };

  const call = (name, args) => {
    if (closed) return Promise.reject(new BridgeError("本轮已取消，不能再调用工具"));
    calls += 1;
    if (calls > callLimit) {
      return Promise.reject(new BridgeError(`工具调用次数超过上限（${callLimit}）`));
    }
    if (pending.size >= MAX_PENDING_REQUESTS) {
      return Promise.reject(new BridgeError(`等待中的工具请求过多（${MAX_PENDING_REQUESTS}）`));
    }
    return enqueue("tool_request", { name, args });
  };

  const ask = (questions) => {
    if (closed) return Promise.reject(new BridgeError("本轮已取消，不能再提问"));
    if (!Array.isArray(questions) || questions.length === 0) {
      return Promise.reject(new BridgeError("至少写一道题"));
    }
    if (questions.length > MAX_QUESTIONS) {
      return Promise.reject(new BridgeError(`一次最多问 ${MAX_QUESTIONS} 道`));
    }
    batches += 1;
    if (batches > MAX_QUESTION_BATCHES) {
      return Promise.reject(new BridgeError(`本轮提问批次已用完（最多 ${MAX_QUESTION_BATCHES} 批）`));
    }
    if (pending.size >= MAX_PENDING_REQUESTS) {
      return Promise.reject(new BridgeError(`等待中的工具请求过多（${MAX_PENDING_REQUESTS}）`));
    }
    return enqueue("questions", { batch: batches, questions });
  };

  /**
   * 处理一条来自 stdin 的 `tool_result`。只认当前已发出请求的 callId：
   * 未知 ID、还没发出的排队 ID、重复与迟到回复一律返回 false，不推进队列。
   */
  const handle = (value) => {
    if (!value || value.type !== "tool_result") return false;
    const waiter = active;
    if (!waiter || waiter.settled) return false;
    if (value.callId !== waiter.callId) return false;
    const ok = value.ok === true;
    const outcome = ok ? (value.result ?? {}) : new BridgeError(value.error || "工具调用失败");
    if (!settle(waiter, ok, outcome)) return false;
    pump();
    return true;
  };

  /**
   * 取消 / 进程结束时清空在途与排队请求，不把取消当成功；
   * 之后不再发出任何请求，晚到的回复没有等待者。可重复调用。
   */
  const abortAll = (error) => {
    closed = true;
    const waiters = [...pending.values()];
    queue.length = 0;
    const reason = error instanceof Error ? error : new BridgeError("本轮已取消");
    for (const waiter of waiters) settle(waiter, false, reason);
  };

  return {
    call,
    ask,
    handle,
    abortAll,
    pending: () => pending.size,
    used: () => calls,
    limit: () => callLimit,
    /** 上调 / 下调本轮的绝对工具额度。分阶段只按阶段需要加，不无限放宽。 */
    setLimit(value) {
      if (Number.isInteger(value) && value > 0) callLimit = Math.min(MAX_TOOL_CALLS + 24, value);
    },
  };
}

function textResult(value) {
  return { output: [{ type: "text", text: JSON.stringify(value) }] };
}

/** 把领域工具注册成 Durable 工具；执行时都走同一座桥。 */
export function createDomainTools(bridge, { readDocument, readChars, onAnswer, onResult } = {}) {
  const tools = DOMAIN_TOOLS.map(([name, description, parameters]) =>
    defineTool({
      name,
      description,
      parameters,
      ...((name === "kb_read" || name === "ms_read") && Number.isInteger(readChars)
        ? { outputLimits: outputLimitsForChars(readChars, { json: true }) }
        : {}),
      execute: async (args) => {
        let values = args ?? {};
        if ((name === "kb_read" || name === "ms_read") && Number.isInteger(readChars)) {
          values = { ...values, limit_chars: readChars };
        }
        if (name === "check_document" && values.path !== undefined) {
          if (values.text !== undefined) throw new BridgeError("path 与 text 只能选一个");
          if (!readDocument) throw new BridgeError("当前工具没有文件读取器");
          values = { text: readDocument(values.path) };
        }
        const result = await bridge.call(name, values);
        onResult?.(name, result);
        return textResult(result);
      },
    }),
  );
  tools.push(
    defineTool({
      name: "ask_user",
      description:
        "只在缺少只有用户知道的关键信息时提问；一次最多 3 题，每轮最多 2 批。不能问密级、文号、签发人、成文日期、份号这类授权要素，也不能问文种。",
      parameters: Type.Object({
        questions: Type.Array(
          Type.Object({
            question: Type.String({ description: "不超过 40 字的问题" }),
            options: Type.Optional(
              Type.Array(Type.String(), { description: "0 或 2–4 个选项；不给就让人自己写" }),
            ),
          }),
        ),
      }),
      execute: async (args) => {
        const result = await bridge.ask(args?.questions ?? []);
        onAnswer?.(result);
        return textResult(result);
      },
    }),
  );
  return tools;
}
