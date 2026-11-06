import { test } from "node:test";
import assert from "node:assert/strict";
import {
  PROTOCOL_VERSION,
  ProtocolError,
  createEmitter,
  readJsonLines,
  sanitizeShort,
  validateInit,
} from "../src/protocol.mjs";
import { Readable } from "node:stream";

function baseInit(overrides = {}) {
  return {
    type: "init",
    protocol: PROTOCOL_VERSION,
    requestId: "req-1",
    runId: "run-1",
    workspace: {
      root: process.platform === "win32" ? "C:\\tmp\\gongwen-ws" : "/tmp/gongwen-ws",
      state: process.platform === "win32" ? "C:\\tmp\\gongwen-state" : "/tmp/gongwen-state",
      delivery: process.platform === "win32" ? "C:\\tmp\\gongwen-run" : "/tmp/gongwen-run",
      current: "input/current.md",
      request: "input/request.md",
      skills: "skills/gongwen-markdown",
    },
    model: {
      baseUrl: "http://127.0.0.1:1234/v1",
      apiKey: "",
      modelId: "local-model",
      contextWindow: 32768,
      maxTokens: 4096,
      reasoning: false,
    },
    task: { request: "写一份通知。", kind: "通知", systemDate: "2026-11-01" },
    ...overrides,
  };
}

test("validateInit accepts a complete message and normalizes limits", () => {
  const config = validateInit(baseInit());
  assert.equal(config.requestId, "req-1");
  assert.equal(config.workspace.current, "input/current.md");
  assert.equal(config.model.contextWindow, 32768);
  assert.equal(config.limits.maxFileBytes, 8 * 1024 * 1024);
});

test("validateInit rejects protocol mismatches and relative workspace paths", () => {
  assert.throws(
    () => validateInit(baseInit({ protocol: PROTOCOL_VERSION + 1 })),
    (error) => error instanceof ProtocolError && /协议版本/.test(error.message),
  );
  const relative = baseInit();
  relative.workspace.root = "workspace";
  assert.throws(() => validateInit(relative), /绝对路径/);
  const missingState = baseInit();
  delete missingState.workspace.state;
  assert.throws(() => validateInit(missingState), /workspace.state/);
});

test("validateInit refuses non-http model endpoints and inverted limits", () => {
  const init = baseInit();
  init.model.baseUrl = "file:///etc/passwd";
  assert.throws(() => validateInit(init), /http/);
  const inverted = baseInit();
  inverted.model.maxTokens = inverted.model.contextWindow + 1;
  assert.throws(() => validateInit(inverted), /maxTokens/);
});

test("sanitizeShort strips control characters and bounds length", () => {
  const text = sanitizeShort(`a\u0000b\r\nc${"x".repeat(20)}`, 10);
  assert.ok(!text.includes("\u0000"));
  assert.ok(text.endsWith("…"));
  assert.ok(text.length <= 11);
});

test("createEmitter scrubs secrets from short fields but not document content", () => {
  const lines = [];
  const emit = createEmitter({ write: (line) => lines.push(line), secrets: ["sk-secret-1234"] });
  emit({ type: "progress", message: "key sk-secret-1234 failed", content: "正文 sk-secret-1234" });
  const value = JSON.parse(lines[0]);
  assert.equal(value.message, "key [已隐藏] failed");
  assert.equal(value.content, "正文 sk-secret-1234");
});

test("createEmitter refuses output beyond its line and message bounds", () => {
  const emit = createEmitter({ write: () => {}, maxLineBytes: 50 });
  assert.throws(() => emit({ type: "progress", message: "x".repeat(200) }), /过大/);
  const limited = createEmitter({ write: () => {}, maxMessages: 1 });
  limited({ type: "progress", message: "ok" });
  assert.throws(() => limited({ type: "progress", message: "again" }), /超过上限/);
});

test("readJsonLines rejects an oversized line instead of truncating it", async () => {
  const stream = Readable.from([`{"type":"init","x":"${"a".repeat(200)}"}\n`]);
  await assert.rejects(
    () => readJsonLines(stream, { maxLineBytes: 64, onValue: () => true }),
    /输入行过大/,
  );
});

test("readJsonLines parses multiple lines and stops when asked", async () => {
  const seen = [];
  const stream = Readable.from(['{"type":"init"}\n{"type":"cancel"}\n{"type":"ping"}\n']);
  await readJsonLines(stream, {
    onValue: (value) => {
      seen.push(value.type);
      return value.type !== "cancel";
    },
  });
  assert.deepEqual(seen, ["init", "cancel"]);
});

test("malformed init errors do not echo credential fragments", async () => {
  const secret = "sk-private-test";
  const stream = Readable.from([`{"apiKey":"${secret}", broken}\n`]);
  await assert.rejects(
    () => readJsonLines(stream, { onValue: () => true }),
    (error) => error instanceof ProtocolError && !error.message.includes(secret) && !error.message.includes("apiKey"),
  );
});
