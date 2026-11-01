//! 工具桥：host 把领域工具与提问转成 `tool_request` / `questions`，按 `callId` 等
//! Rust 的 `tool_result`。不对外开 MCP 服务；工具名与次数由这里和 Rust 双重限制。

import { Type } from "@earendil-works/pi-ai";
import { defineTool } from "@earendil-works/pi-durable";

/** 一轮最多多少次工具调用（与 Rust 上限一致）。 */
export const MAX_TOOL_CALLS = 60;
export const MAX_QUESTION_BATCHES = 2;
export const MAX_QUESTIONS = 3;

/** 领域工具的名字与 schema。Rust 还会再校验一遍。 */
export const DOMAIN_TOOLS = [
  ["kb_search", "检索本机知识库，返回带证据编号的相关片段", Type.Object({
    query: Type.String({ description: "一句话说明要找什么" }),
    top: Type.Optional(Type.Number({ description: "最多返回几段，1–10" })),
  })],
  ["kb_read", "读本机知识库的一篇文档全文", Type.Object({
    doc_id: Type.Number({ description: "kb_search 返回的 doc_id" }),
  })],
  ["ms_search", "检索稿件库（标题 / 文号 / 备注）", Type.Object({
    query: Type.String({ description: "检索词" }),
    limit: Type.Optional(Type.Number({ description: "最多几篇，1–10" })),
  })],
  ["ms_read", "读稿件库的一篇稿件（可指定版本）", Type.Object({
    id: Type.Number({ description: "稿件编号" }),
    version: Type.Optional(Type.Number({ description: "历史版本号；省略读当前版本" })),
  })],
  ["vocab_search", "查标准词库里的单位 / 人员写法", Type.Object({
    query: Type.String({ description: "名称或编码的一部分" }),
    category: Type.Optional(Type.String({ description: "unit / person，省略为不限" })),
  })],
  ["check_document", "跑程序的确定性检查（事实来源、公文引用、要素、词表）", Type.Object({
    path: Type.Optional(Type.String({ description: "优先使用：work/ 下待检查的 Markdown 文件路径；host 读取全文，不要重复生成全文参数" })),
    text: Type.Optional(Type.String({ description: "要检查的 Markdown；省略检查本轮正文" })),
  })],
  ["doc_elements", "只读当前表单已填的公文要素（授权类只读）", Type.Object({})],
];

export class BridgeError extends Error {
  constructor(message) {
    super(message);
    this.name = "BridgeError";
  }
}

/**
 * 建立工具桥。`emit` 只写协议；`pause/resume` 用于把等用户回答的时间从执行超时里扣除。
 */
export function createToolBridge({ emit, pause, resume }) {
  const pending = new Map();
  let nextCall = 1;
  let calls = 0;
  let batches = 0;

  const settle = (callId, fn) => {
    const waiter = pending.get(callId);
    if (!waiter) return false;
    pending.delete(callId);
    fn(waiter);
    return true;
  };

  const request = (type, payload) => {
    const callId = `c${nextCall++}`;
    return new Promise((resolve, reject) => {
      pending.set(callId, { resolve, reject, type });
      try {
        emit({ type, callId, ...payload });
      } catch (error) {
        pending.delete(callId);
        reject(error);
      }
    });
  };

  const call = (name, args) => {
    calls += 1;
    if (calls > MAX_TOOL_CALLS) {
      return Promise.reject(new BridgeError(`工具调用次数超过上限（${MAX_TOOL_CALLS}）`));
    }
    return request("tool_request", { name, args });
  };

  const ask = (questions) => {
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
    pause?.();
    return request("questions", { batch: batches, questions }).finally(() => resume?.());
  };

  /** 处理一条来自 stdin 的 `tool_result`；错 ID / 迟到回复返回 false，直接丢弃。 */
  const handle = (value) => {
    if (!value || value.type !== "tool_result") return false;
    return settle(value.callId, (waiter) => {
      if (value.ok) waiter.resolve(value.result ?? {});
      else waiter.reject(new BridgeError(value.error || "工具调用失败"));
    });
  };

  /** 取消 / 进程结束时清掉所有等待者，不把取消当成功。 */
  const abortAll = (error) => {
    for (const [, waiter] of pending) waiter.reject(error);
    pending.clear();
  };

  return { call, ask, handle, abortAll, pending: () => pending.size };
}

function textResult(value) {
  return { output: [{ type: "text", text: JSON.stringify(value) }] };
}

/** 把领域工具注册成 Durable 工具；执行时都走同一座桥。 */
export function createDomainTools(bridge, { readDocument } = {}) {
  const tools = DOMAIN_TOOLS.map(([name, description, parameters]) =>
    defineTool({
      name,
      description,
      parameters,
      execute: async (args) => {
        let values = args ?? {};
        if (name === "check_document" && values.path !== undefined) {
          if (values.text !== undefined) throw new BridgeError("path 与 text 只能选一个");
          if (!readDocument) throw new BridgeError("当前工具没有文件读取器");
          values = { text: readDocument(values.path) };
        }
        return textResult(await bridge.call(name, values));
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
        return textResult(result);
      },
    }),
  );
  return tools;
}
