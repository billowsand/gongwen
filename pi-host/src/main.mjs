//! host 入口：读一条 init，跑一轮 Durable，按 JSONL 回报。
//!
//! stdout 只写协议；任何依赖或异常的输出都改走 stderr。密钥只在 init 里出现，
//! 不会进入事件、日志或命令行。

import {
  HOST_VERSION,
  PROTOCOL_VERSION,
  ProtocolError,
  createEmitter,
  readJsonLines,
  validateInit,
} from "./protocol.mjs";
import { AgentError, runAgentJob } from "./agent.mjs";
import { acquireOwner } from "./owner.mjs";
import { receiptEvents } from "./delivery.mjs";
import path from "node:path";

// 先捕获真正的 stdout，再把其他写 stdout 的代码改道 stderr；协议只用 capturedWrite。
const capturedWrite = process.stdout.write.bind(process.stdout);
process.stdout.write = (chunk, encoding, callback) => process.stderr.write(chunk, encoding, callback);
console.log = (...args) => console.error(...args);
console.info = (...args) => console.error(...args);
console.debug = (...args) => console.error(...args);

function writeError(message) {
  try {
    process.stderr.write(`[pi-host] ${message}\n`);
  } catch {
    // stderr 关闭时不再抛。
  }
}

async function readInit() {
  let init;
  await readJsonLines(process.stdin, {
    onValue(value) {
      init = value;
      return false;
    },
  });
  if (!init) throw new ProtocolError("没有收到 init 消息");
  return init;
}

function finish(code, owner) {
  // 给 stdout 一点时间把最后一条协议行写完，再结束进程；Durable 的计时器与
  // SQLite 句柄不必逐个拆，Rust 已在收到终止事件后回收子进程。
  setTimeout(() => {
    try {
      owner?.release();
    } catch {
      // 释放失败不影响退出；OS 会在进程结束时回收。
    }
    // 完成事件之后发生的未捕获异常仍应让 Rust 拒绝交付，不能被成功定时器抹掉。
    process.exit(Number(process.exitCode) || code);
  }, 50);
}

async function main() {
  let config;
  let emit;
  try {
    const raw = await readInit();
    config = validateInit(raw);
    emit = createEmitter({
      write: capturedWrite,
      secrets: [config.model.apiKey],
    });
  } catch (error) {
    // 还没拿到可信配置：只报错误类型，不回显 init 内容（可能含密钥）。
    const message = error instanceof ProtocolError ? error.message : "init 解析失败";
    try {
      capturedWrite(`${JSON.stringify({ type: "error", code: "protocol", message })}\n`);
    } catch {
      // 忽略。
    }
    writeError(message);
    setTimeout(() => process.exit(1), 50);
    return;
  }

  const base = {
    protocol: PROTOCOL_VERSION,
    requestId: config.requestId,
    runId: config.runId,
  };
  const send = (event) => {
    try {
      emit({ ...base, ...event });
    } catch (error) {
      writeError(`发送协议消息失败：${error.message}`);
      throw error;
    }
  };

  // 同一轮工作区只能有一个 host：用 SQLite 独占锁，不靠界面 busy 或残留文件。
  let owner;
  try {
    owner = acquireOwner(path.join(config.workspace.delivery, "host-owner.sqlite"), {
      runId: config.runId,
    });
  } catch (error) {
    const code = error?.code === "owner_busy" ? "owner_busy" : "owner_failed";
    send({ type: "error", code, message: error?.message ?? "占用 Pi 工作区失败" });
    writeError(error?.message ?? "占用 Pi 工作区失败");
    finish(1, undefined);
    return;
  }

  let cancel;
  let cancelled = false;
  let bridge;
  const requestCancel = (reason) => {
    if (cancelled) return;
    cancelled = true;
    writeError(`收到取消：${reason}`);
    try {
      bridge?.abortAll(new Error("本轮已取消"));
    } catch {
      // 取消只影响本轮。
    }
    try {
      cancel?.();
    } catch {
      // 取消只影响本轮。
    }
  };

  // init 之后 stdin 只接受 cancel / ping / tool_result；Rust 进程崩溃导致 EOF 时也停下模型调用。
  readJsonLines(process.stdin, {
    onValue(value) {
      if (value?.type === "cancel") {
        requestCancel("收到 cancel");
      } else if (value?.type === "ping") {
        send({ type: "pong" });
      } else if (value?.type === "tool_result") {
        // 错 run / 错 call ID 或迟到回复：handle 返回 false，直接丢弃。
        bridge?.handle(value);
      }
      return true;
    },
  })
    .catch(() => {})
    .finally(() => requestCancel("stdin 已关闭"));

  send({
    type: "ready",
    hostVersion: HOST_VERSION,
    nodeVersion: process.versions.node,
    model: config.model.modelId,
  });

  try {
    const result = await runAgentJob({
      config,
      emit: send,
      onBridge: (value) => {
        bridge = value;
      },
      onCancel: (fn) => {
        cancel = fn;
      },
    });
    if (result.receipt) {
      // 恢复时找到已经落盘的回执：原样交回，不再调用模型。
      for (const event of receiptEvents(config.workspace.delivery, result.receipt)) {
        send(event);
      }
      finish(0, owner);
    } else if (result.report) {
      // 清单事件已在工具调用时发出；这里只补完成事件，不带 submitted。
      send({ type: "completed", submitted: false });
      finish(0, owner);
    } else if (result.snapshot) {
      send({
        type: "submitted",
        path: result.snapshot.path,
        summary: result.snapshot.summary,
        content: result.snapshot.content,
        sha256: result.snapshot.sha256,
        bytes: result.snapshot.bytes,
      });
      send({ type: "completed", submitted: true });
      finish(0, owner);
    } else {
      send({ type: "completed", submitted: false });
      finish(0, owner);
    }
  } catch (error) {
    const code = error instanceof AgentError ? error.code : "agent_failed";
    if (code === "cancelled" || cancelled) {
      send({ type: "cancelled", message: "已停止，未交付任何提案。" });
      finish(3, owner);
    } else {
      send({
        type: "error",
        code,
        message: error?.message || "智能写作失败",
      });
      finish(1, owner);
    }
  }
}

process.on("uncaughtException", (error) => {
  writeError(`未捕获异常：${error?.message || error}`);
  process.exitCode = 1;
});
process.on("unhandledRejection", (error) => {
  writeError(`未处理的 Promise 拒绝：${error?.message || error}`);
  process.exitCode = 1;
});

await main();
