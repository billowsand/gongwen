//! 测试专用 host 引导：不经过 src/main.mjs，而是在生产 `runAgentJob` 上注入
//! 受控的崩溃窗口。只被 `node --test` 使用，不随运行时打包。
//!
//! 用法：`node fixtures/host-hooks.mjs <init.json>`
//! 环境变量 `GONGWEN_PI_TEST_HOOK`：
//! - `write-block`：`work/final.md` 已经原子落盘、工具结果还没提交时阻塞；
//! - `receipt-block`：交付回执已经落盘、`submitted` 协议行还没写出时阻塞。
//! 两种情况都会先写 `GONGWEN_PI_TEST_SIGNAL` 指向的文件，再由测试强杀进程。

import fs from "node:fs";
import { runAgentJob } from "../src/agent.mjs";
import { PROTOCOL_VERSION, validateInit } from "../src/protocol.mjs";
import { WorkspaceEnv } from "../src/paths.mjs";

const hook = process.env.GONGWEN_PI_TEST_HOOK ?? "";
const signal = process.env.GONGWEN_PI_TEST_SIGNAL ?? "";
const initPath = process.argv[2];
if (!initPath) {
  process.stderr.write("缺少 init.json\n");
  process.exit(2);
}

const gate = new Int32Array(new SharedArrayBuffer(4));
function blockForever(label) {
  if (signal) fs.writeFileSync(signal, label);
  // 同步阻塞整个事件循环：工具结果与协议行都不会再往前走，直到测试强杀。
  Atomics.wait(gate, 0, 0, 120_000);
}

if (hook === "write-block") {
  const original = WorkspaceEnv.prototype.writeFile;
  WorkspaceEnv.prototype.writeFile = async function writeFile(p, content, context) {
    const result = await original.call(this, p, content, context);
    if (result?.ok && String(p).replaceAll("\\", "/").endsWith("work/final.md")) {
      blockForever("written");
    }
    return result;
  };
}

const raw = fs.readFileSync(initPath, "utf8");
const config = validateInit(JSON.parse(raw));
const base = { protocol: PROTOCOL_VERSION, requestId: config.requestId, runId: config.runId };
const write = process.stdout.write.bind(process.stdout);
const emit = (event) => {
  if (
    hook === "receipt-block" &&
    event.type === "progress" &&
    String(event.message ?? "").startsWith("已提交 ")
  ) {
    // 回执已经落盘；这里卡住的是工具结果与随后的 submitted / completed 协议行。
    blockForever("receipt");
  }
  write(`${JSON.stringify({ ...base, ...event })}\n`);
};

try {
  await runAgentJob({ config, emit });
  // 正常结束时也只结束进程，不补协议行（测试只关心阻塞窗口）。
  setTimeout(() => process.exit(0), 50);
} catch (error) {
  process.stderr.write(`[host-hooks] ${error?.message ?? error}\n`);
  setTimeout(() => process.exit(1), 50);
}
