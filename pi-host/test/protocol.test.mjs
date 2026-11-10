import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import {
  HOST_VERSION,
  PROTOCOL_VERSION,
  ProtocolError,
  createEmitter,
  readJsonLines,
  sanitizeShort,
  validateInit,
} from "../src/protocol.mjs";
import { Readable } from "node:stream";

/** 读仓库内的 JSON 文件；路径按测试文件位置解析，不依赖当前工作目录。 */
function readRepoJson(relative) {
  return JSON.parse(fs.readFileSync(new URL(relative, import.meta.url), "utf8"));
}

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

// 版本对账：`packaging/pi-runtime.json`、host 的 `package.json` / lockfile 与
// `protocol.mjs`、Rust `src/pi_agent/protocol.rs` 必须描述同一套版本。升级 Pi 依赖、
// 改 host 或协议时只要漏改一处，这里直接失败——随包运行时不允许悄悄带旧版本。
// （运行时 manifest 与打包配置的哈希对账仍由 scripts/pi-runtime-verify.mjs 与 Rust
// 的 shipped_pi_runtime_manifest 覆盖，本用例只守源码侧的版本常量与依赖声明。）
test("host 版本与协议在 protocol.mjs、打包配置、package.json/lock 与 Rust 四处一致", () => {
  const packageJson = readRepoJson("../package.json");
  const packageLock = readRepoJson("../package-lock.json");
  const runtime = readRepoJson("../../packaging/pi-runtime.json");
  const rust = fs.readFileSync(new URL("../../src/pi_agent/protocol.rs", import.meta.url), "utf8");

  // 1) host 版本：Node 常量是当前版本的唯一来源，Rust / 打包配置 / 包清单都要跟它一致。
  assert.match(
    rust,
    new RegExp(`HOST_VERSION: &str = "${HOST_VERSION}"`),
    "Rust src/pi_agent/protocol.rs 的 HOST_VERSION 与 host 不一致",
  );
  assert.equal(runtime.host.version, HOST_VERSION, "packaging/pi-runtime.json 的 host.version 漏改");
  assert.equal(packageJson.version, HOST_VERSION, "pi-host/package.json 的 version 漏改");
  assert.equal(packageLock.version, HOST_VERSION, "package-lock.json 根 version 漏改");
  assert.equal(packageLock.packages[""].version, HOST_VERSION, "package-lock.json packages[\"\"].version 漏改");

  // 2) 协议版本：Node / Rust / 打包配置三处必须同时改，任何一处不一致都要拦下。
  const rustProtocol = rust.match(/PROTOCOL_VERSION: u32 = (\d+)/)?.[1];
  assert.ok(rustProtocol, "Rust src/pi_agent/protocol.rs 缺少 PROTOCOL_VERSION 常量");
  assert.equal(Number(rustProtocol), PROTOCOL_VERSION, "Rust 协议版本与 host 不一致");
  assert.equal(runtime.host.protocol, PROTOCOL_VERSION, "packaging/pi-runtime.json 的 host.protocol 漏改");

  // 3) Pi 三包：打包配置、package.json 精确版本、lockfile 实际装到的版本必须完全一致。
  const piPackages = ["@earendil-works/chord", "@earendil-works/pi-ai", "@earendil-works/pi-durable"];
  assert.deepEqual(
    Object.keys(runtime.pi).sort(),
    [...piPackages].sort(),
    "packaging/pi-runtime.json 的 pi 依赖表必须正好是 chord / pi-ai / pi-durable 三包",
  );
  for (const name of piPackages) {
    const declared = packageJson.dependencies?.[name];
    assert.equal(typeof declared, "string", `pi-host/package.json 没有声明 ${name}`);
    assert.match(declared, /^\d+\.\d+\.\d+$/, `${name} 必须锁定精确版本，不能用范围`);
    assert.equal(packageLock.packages[""]?.dependencies?.[name], declared, `${name} 的 lockfile 根依赖与 package.json 不一致`);
    assert.equal(
      packageLock.packages[`node_modules/${name}`]?.version,
      declared,
      `${name} 的 lockfile 实际版本与 package.json 不一致（先 npm install 再提交 lockfile）`,
    );
    assert.equal(runtime.pi[name], declared, `${name} 的打包配置版本与 package.json 不一致`);
  }

  // 4) TypeBox 自 pi-ai 1.5 起不再从上游导出，host 直依赖它：必须精确锁定且已装进 lockfile。
  const typebox = packageJson.dependencies?.typebox;
  assert.equal(typeof typebox, "string", "pi-host/package.json 缺少 typebox 直依赖");
  assert.match(typebox, /^\d+\.\d+\.\d+$/, "typebox 必须锁定精确版本");
  assert.equal(packageLock.packages[""]?.dependencies?.typebox, typebox, "typebox 的 lockfile 根依赖与 package.json 不一致");
  assert.equal(
    packageLock.packages["node_modules/typebox"]?.version,
    typebox,
    "typebox 的 lockfile 实际版本与 package.json 不一致",
  );
});
