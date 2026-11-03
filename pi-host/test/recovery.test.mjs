import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  RECEIPT_MAX_BYTES,
  readReceipt,
  receiptEvents,
  writeDocumentReceipt,
  writeReportReceipt,
} from "../src/delivery.mjs";
import { acquireOwner } from "../src/owner.mjs";
import { pathToFileURL } from "node:url";
import { PROTOCOL_VERSION, validateInit } from "../src/protocol.mjs";

const hostDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

test("an uncaught error after completed cannot be overwritten by the success exit timer", {timeout:20_000}, async () => {
  const workspace = tempRun();
  const content = "# 已提交\n\n正文。\n";
  writeDocumentReceipt(workspace.run, {runId:"run-1",requestId:"req-1",relative:"work/final.md",content,
    bytes:Buffer.byteLength(content),sha256:(await import("node:crypto")).createHash("sha256").update(content).digest("hex"),summary:"测试",callId:"c1"});
  const entry = path.join(workspace.base,"late-error.mjs");
  fs.writeFileSync(entry, `
    import fs from 'node:fs';
    import {PassThrough} from 'node:stream';
    const input=new PassThrough();
    Object.defineProperty(process,'stdin',{value:input});
    input.write(fs.readFileSync(process.argv[2],'utf8')+String.fromCharCode(10));
    const write=process.stdout.write.bind(process.stdout);
    process.stdout.write=function(value,...rest){
      const result=write(value,...rest);
      if(String(value).includes('"type":"completed"')) queueMicrotask(()=>{throw new Error('late-test-failure')});
      return result;
    };
    await import(${JSON.stringify(pathToFileURL(path.join(hostDir,"src/main.mjs")).href)});
  `);
  const host = startHost(workspace,{script:path.relative(hostDir,entry),extra:{recovery:{mode:"resume",requestId:"req-1"}}});
  try {
    const code = await exitCode(host);
    assert.ok(host.state.lines.some(event=>event.type==="completed"),host.state.stderr + host.state.output);
    assert.notEqual(code,0,"完成事件之后的异常仍须让进程失败");
    assert.match(host.state.stderr,/late-test-failure/);
  } finally { await host.close(); }
});
const CASE_TIMEOUT = 60_000;

function tempRun() {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-pi-recovery-"));
  const root = path.join(base, "workspace");
  const run = path.join(base, "run");
  for (const sub of ["input", "work", "skills/gongwen-markdown"]) {
    fs.mkdirSync(path.join(root, sub), { recursive: true });
  }
  fs.mkdirSync(run, { recursive: true });
  fs.writeFileSync(path.join(root, "input", "current.md"), "# 当前\n\n原有内容。\n");
  fs.writeFileSync(path.join(root, "input", "request.md"), "写一份通知。\n");
  fs.writeFileSync(path.join(root, "skills", "gongwen-markdown", "SKILL.md"), "# 技能\n");
  return { base, root, run, state: path.join(run, "session.sqlite") };
}

function initMessage(workspace, extra = {}) {
  return {
    type: "init",
    protocol: PROTOCOL_VERSION,
    requestId: "req-1",
    runId: "run-1",
    workspace: {
      root: workspace.root,
      state: workspace.state,
      delivery: workspace.run,
      current: "input/current.md",
      request: "input/request.md",
      skills: "skills/gongwen-markdown",
    },
    model: {
      baseUrl: "http://127.0.0.1:9/v1",
      apiKey: "sk-test-not-real",
      modelId: "stub-model",
      contextWindow: 32768,
      maxTokens: 1024,
      reasoning: false,
    },
    task: { request: "写一份通知。", kind: "通知", systemDate: "2026-11-01" },
    ...extra,
  };
}

function sse(res, payload) {
  res.write(`data: ${JSON.stringify(payload)}\n\n`);
}

function chunk(choices, usage) {
  return {
    id: "chatcmpl-stub",
    object: "chat.completion.chunk",
    created: Math.floor(Date.now() / 1000),
    model: "stub-model",
    choices,
    ...(usage ? { usage } : {}),
  };
}

function toolCall(res, name, args) {
  sse(
    res,
    chunk([
      {
        index: 0,
        delta: {
          role: "assistant",
          tool_calls: [
            { index: 0, id: `call-${name}`, type: "function", function: { name, arguments: JSON.stringify(args) } },
          ],
        },
        finish_reason: null,
      },
    ]),
  );
  sse(
    res,
    chunk([{ index: 0, delta: {}, finish_reason: "tool_calls" }], {
      prompt_tokens: 10,
      completion_tokens: 5,
      total_tokens: 15,
    }),
  );
  res.write("data: [DONE]\n\n");
  res.end();
}

/**
 * 本地 OpenAI 兼容桩。脚本模式：
 * - `submit`：直接提交 `work/final.md`；
 * - `read-then-submit`：先 read 当前文件，再提交（用于证明模型读的是磁盘上的内容）；
 * - `tool`：先写文件，再调 check_document（测试故意不回答）；
 * - `ask`：先调 ask_user（测试故意不回答），恢复后写文件再提交。
 */
async function startStub(mode) {
  const requests = [];
  const server = http.createServer((req, res) => {
    if (!req.url.endsWith("/chat/completions")) {
      res.writeHead(404).end();
      return;
    }
    let body = "";
    req.on("data", (part) => {
      body += part;
    });
    req.on("end", () => {
      const parsed = JSON.parse(body || "{}");
      requests.push(parsed);
      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
      const messages = parsed.messages ?? [];
      const toolResults = messages.filter((message) => message.role === "tool");
      const toolCalls = messages.flatMap((message) =>
        (message.tool_calls ?? []).map((call) => call.function?.name),
      );
      if (mode === "submit") {
        toolCall(res, "submit_document", { path: "work/final.md", summary: "提交现有文件" });
        return;
      }
      if (mode === "read-then-submit") {
        if (!toolCalls.includes("read")) {
          toolCall(res, "read", { path: "work/final.md" });
        } else {
          toolCall(res, "submit_document", {
            path: "work/final.md",
            summary: "读回已写盘的文件后提交",
          });
        }
        return;
      }
      if (mode === "tool") {
        if (toolResults.length === 0) {
          toolCall(res, "write", {
            path: "work/final.md",
            content: "# 关于测试的通知\n\n请各单位抓好落实。\n",
          });
        } else {
          toolCall(res, "submit_document", {
            path: "work/final.md",
            summary: "按已写盘的文件提交",
          });
        }
        return;
      }
      // ask
      if (!toolCalls.includes("write")) {
        if (toolResults.length === 0) {
          toolCall(res, "ask_user", {
            questions: [{ question: "重点写哪方面", options: ["责任", "时限"] }],
          });
        } else {
          toolCall(res, "write", {
            path: "work/final.md",
            content: "# 已确认的答复\n\n按回答写好的正文。\n",
          });
        }
      } else {
        toolCall(res, "submit_document", { path: "work/final.md", summary: "恢复后提交" });
      }
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  return { server, requests, baseUrl: `http://127.0.0.1:${port}/v1` };
}

/**
 * 起一个 host。`script` 是 `src/main.mjs` 或测试引导；测试引导从 argv 读 init 文件，
 * main 从 stdin 读。返回的 `close()` 一定杀进程并等退出。
 */
function startHost(workspace, { script = "src/main.mjs", extra = {}, env = {} } = {}) {
  const init = initMessage(workspace, extra);
  const args = [path.join(hostDir, script)];
  if (script !== "src/main.mjs") {
    const initPath = path.join(workspace.run, "init-test.json");
    fs.writeFileSync(initPath, JSON.stringify(init));
    args.push(initPath);
  }
  const child = spawn(process.execPath, args, {
    cwd: hostDir,
    stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env, ...env },
  });
  const state = { lines: [], stderr: "", exited: undefined, output: "" };
  let buffer = "";
  child.stdout.on("data", (data) => {
    state.output += data;
    buffer += data;
    let index;
    while ((index = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, index);
      buffer = buffer.slice(index + 1);
      if (line.trim() === "") continue;
      try {
        state.lines.push(JSON.parse(line));
      } catch {
        state.lines.push({ type: "unparsed", line });
      }
    }
  });
  child.stderr.on("data", (data) => {
    state.stderr += data;
  });
  child.on("exit", (code, signal) => {
    state.exited = { code, signal };
  });
  if (script === "src/main.mjs") {
    child.stdin.write(`${JSON.stringify(init)}\n`);
  }
  const close = async () => {
    if (!state.exited) {
      child.kill("SIGKILL");
      await waitFor(() => state.exited !== undefined, 10_000, "进程退出").catch(() => {});
    }
  };
  return { child, state, close };
}

async function waitFor(predicate, timeoutMs = 30_000, label = "条件") {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const value = predicate();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error(`等待${label}超时`);
}

async function exitCode(host, timeoutMs = 60_000) {
  await waitFor(() => host.state.exited !== undefined, timeoutMs, "host 退出");
  return host.state.exited.code;
}

async function killAndWait(host) {
  if (!host.state.exited) {
    host.child.kill("SIGKILL");
    await waitFor(() => host.state.exited !== undefined, 10_000, "进程退出");
  }
}

function allMessages(stub) {
  return stub.requests.flatMap((request) => request.messages ?? []);
}

function sawInterrupted(stub) {
  return allMessages(stub).some(
    (message) =>
      message.role === "tool" && String(message.content ?? "").includes("was interrupted"),
  );
}

test("document receipts bind run, hash and immutable snapshot", () => {
  const workspace = tempRun();
  const content = "# 成品\n";
  writeDocumentReceipt(workspace.run, {
    runId: "run-1",
    requestId: "req-1",
    relative: "work/final.md",
    content,
    sha256: "0".repeat(64),
    bytes: Buffer.byteLength(content, "utf8"),
    summary: "测试",
    callId: "c1",
  });
  assert.equal(fs.readFileSync(path.join(workspace.run, "final.md"), "utf8"), content);
  assert.throws(() => readReceipt(workspace.run, "run-1"), /哈希/);
  assert.throws(() => readReceipt(workspace.run, "run-2"), /另一轮/);
});

test("a report receipt is capped before writing, never after reading", () => {
  const workspace = tempRun();
  const items = Array.from({ length: 100 }, (_, index) => ({
    title: `第 ${index} 条`,
    detail: "很长的说明。".repeat(800),
    severity: "建议",
  }));
  const receipt = writeReportReceipt(workspace.run, {
    runId: "run-1",
    requestId: "req-1",
    summary: "审完",
    items,
    callId: "c2",
  });
  const size = fs.statSync(path.join(workspace.run, "receipt.json")).size;
  assert.ok(size <= RECEIPT_MAX_BYTES, `回执 ${size} 字节超过写上限`);
  assert.ok(receipt.items.length > 0);
  assert.ok(receipt.omitted > 0, "超出写上限的条目要明确计入 omitted");
  const read = readReceipt(workspace.run, "run-1");
  assert.equal(read.items.length, receipt.items.length);
  const events = receiptEvents(workspace.run, read);
  assert.equal(events[0].type, "report");
  assert.equal(events[1].submitted, false);
});

test("the owner lock excludes a second process and is reacquirable after the holder dies", { timeout: CASE_TIMEOUT }, async () => {
  const workspace = tempRun();
  const lock = path.join(workspace.run, "host-owner.sqlite");
  const signal = path.join(workspace.run, "holder.pid");
  const holder = spawn(
    process.execPath,
    [path.join(hostDir, "fixtures", "owner-holder.mjs"), lock, signal],
    { cwd: hostDir, stdio: ["ignore", "pipe", "pipe"] },
  );
  let holderExit;
  holder.on("exit", (code) => {
    holderExit = code;
  });
  let secondChild;
  const second = { stderr: "", exited: undefined };
  try {
    await waitFor(() => fs.existsSync(signal), 20_000, "持锁进程就绪");
    assert.throws(
      () => acquireOwner(lock, { runId: "run-1", pid: 999 }),
      (error) => error.code === "owner_busy" && /另一个窗口/.test(error.message),
    );
    // 另一个真实进程也必须被挡住，并且给出可读原因、不破坏锁库。
    secondChild = spawn(
      process.execPath,
      [
        path.join(hostDir, "fixtures", "owner-holder.mjs"),
        lock,
        path.join(workspace.run, "second.pid"),
      ],
      { cwd: hostDir, stdio: ["ignore", "pipe", "pipe"] },
    );
    secondChild.stderr.on("data", (data) => {
      second.stderr += data;
    });
    secondChild.on("exit", (code) => {
      second.exited = code;
    });
    await waitFor(() => second.exited !== undefined, 20_000, "第二个持锁进程退出");
    assert.equal(second.exited, 3);
    assert.match(second.stderr, /owner_busy/);
    assert.ok(!fs.existsSync(path.join(workspace.run, "second.pid")), "被挡住时不能写信号");
    // 杀掉持锁者：OS 释放锁，不需要任何清理代码。
    holder.kill("SIGKILL");
    await waitFor(() => holderExit !== undefined, 20_000, "持锁进程退出");
    const after = acquireOwner(lock, { runId: "run-1", pid: 1000 });
    after.release();
  } finally {
    try {
      holder.kill("SIGKILL");
    } catch {
      // 已退出。
    }
    try {
      secondChild?.kill("SIGKILL");
    } catch {
      // 已退出。
    }
  }
});

test("init refuses a delivery directory inside the model workspace", () => {
  const workspace = tempRun();
  const init = initMessage(workspace);
  init.workspace.delivery = path.join(workspace.root, "run");
  assert.throws(() => validateInit(init), /工作区之外/);
  const config = validateInit(
    initMessage(workspace, { recovery: { mode: "resume", requestId: "req-1" } }),
  );
  assert.equal(config.recovery.resume, true);
  assert.throws(
    () => validateInit(initMessage(workspace, { recovery: { mode: "replay" } })),
    /恢复模式/,
  );
});

test("a kill between receipt write and the submitted JSONL line still delivers the same artifact", { timeout: CASE_TIMEOUT }, async () => {
  const workspace = tempRun();
  const stub = await startStub("submit");
  const content = "# 恢复的成品\n\n正文。\n";
  fs.writeFileSync(path.join(workspace.root, "work", "final.md"), content);
  const signal = path.join(workspace.run, "blocked.signal");
  const host = startHost(workspace, {
    script: "fixtures/host-hooks.mjs",
    extra: { model: { ...initMessage(workspace).model, baseUrl: stub.baseUrl } },
    env: { GONGWEN_PI_TEST_HOOK: "receipt-block", GONGWEN_PI_TEST_SIGNAL: signal },
  });
  try {
    await waitFor(() => fs.existsSync(signal), 30_000, "回执写后阻塞信号");
    const receipt = readReceipt(workspace.run, "run-1");
    assert.ok(receipt, "kill 前回执必须已经落盘");
    assert.equal(receipt.kind, "document");
    assert.ok(
      !host.state.lines.some((line) => line.type === "completed"),
      "kill 前不能已经发出 completed",
    );
    assert.ok(
      !host.state.lines.some((line) => line.type === "submitted"),
      "submitted 行正是在这个窗口里被卡住的",
    );
  } finally {
    await host.close();
    stub.server.close();
  }

  // 恢复：不启动模型，直接按回执交回同一份快照。
  const resumeStub = await startStub("submit");
  const resumed = startHost(workspace, {
    extra: {
      model: { ...initMessage(workspace).model, baseUrl: resumeStub.baseUrl },
      recovery: { mode: "resume", requestId: "req-1" },
    },
  });
  try {
    assert.equal(await exitCode(resumed), 0, resumed.state.stderr);
    assert.equal(resumeStub.requests.length, 0, "有回执时不能再调用模型");
    const submitted = resumed.state.lines.find((line) => line.type === "submitted");
    assert.ok(submitted, resumed.state.output);
    assert.equal(submitted.content, content);
    const completed = resumed.state.lines.find((line) => line.type === "completed");
    assert.equal(completed.submitted, true);
  } finally {
    await resumed.close();
    resumeStub.server.close();
  }
});

test("a write that hit the disk but never committed is not replayed; the model reads the current file", { timeout: CASE_TIMEOUT }, async () => {
  const workspace = tempRun();
  const stub = await startStub("tool");
  const signal = path.join(workspace.run, "written.signal");
  const host = startHost(workspace, {
    script: "fixtures/host-hooks.mjs",
    extra: { model: { ...initMessage(workspace).model, baseUrl: stub.baseUrl } },
    env: { GONGWEN_PI_TEST_HOOK: "write-block", GONGWEN_PI_TEST_SIGNAL: signal },
  });
  let written;
  try {
    await waitFor(() => fs.existsSync(signal), 30_000, "写盘后阻塞信号");
    written = fs.readFileSync(path.join(workspace.root, "work", "final.md"), "utf8");
    assert.ok(written.includes("请各单位抓好落实"));
    assert.ok(
      !fs.existsSync(path.join(workspace.run, "receipt.json")),
      "工具结果尚未提交，还没有回执",
    );
  } finally {
    await host.close();
    stub.server.close();
  }

  const resumeStub = await startStub("read-then-submit");
  const resumed = startHost(workspace, {
    extra: {
      model: { ...initMessage(workspace).model, baseUrl: resumeStub.baseUrl },
      recovery: { mode: "resume", requestId: "req-1" },
    },
  });
  try {
    assert.equal(await exitCode(resumed), 0, resumed.state.stderr);
    assert.ok(sawInterrupted(resumeStub), "非 replay-safe 的 write 必须以 interrupted 结果结束");
    const readBack = allMessages(resumeStub).some(
      (message) => message.role === "tool" && String(message.content ?? "").includes("请各单位抓好落实"),
    );
    assert.ok(readBack, "恢复后模型必须读到磁盘上已有的文件");
    const submitted = resumed.state.lines.find((line) => line.type === "submitted");
    assert.ok(submitted);
    assert.equal(submitted.content, written, "提交的必须是写盘后未重做的文件内容");
    assert.equal(fs.readFileSync(path.join(workspace.root, "work", "final.md"), "utf8"), written);
  } finally {
    await resumed.close();
    resumeStub.server.close();
  }
});

test("an interrupted question is not awaited again; the model continues without a stale promise", { timeout: CASE_TIMEOUT }, async () => {
  const workspace = tempRun();
  const stub = await startStub("ask");
  const host = startHost(workspace, {
    extra: { model: { ...initMessage(workspace).model, baseUrl: stub.baseUrl } },
  });
  try {
    await waitFor(
      () => host.state.lines.some((line) => line.type === "questions"),
      30_000,
      "提问事件",
    );
  } finally {
    await host.close();
    stub.server.close();
  }

  const resumeStub = await startStub("ask");
  const resumed = startHost(workspace, {
    extra: {
      model: { ...initMessage(workspace).model, baseUrl: resumeStub.baseUrl },
      recovery: { mode: "resume", requestId: "req-1" },
    },
  });
  try {
    assert.equal(await exitCode(resumed), 0, resumed.state.stderr);
    assert.ok(sawInterrupted(resumeStub), "旧 callId 的提问必须以中断结果结束，不能一直等失效回答");
    assert.ok(resumed.state.lines.some((line) => line.type === "submitted"));
  } finally {
    await resumed.close();
    resumeStub.server.close();
  }
});
