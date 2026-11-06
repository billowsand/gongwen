import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { JsonlStorage } from "@earendil-works/pi-durable/storage/jsonl";
import { createModels, fauxAssistantMessage, fauxProvider, fauxToolCall } from "@earendil-works/pi-ai";
import { createHash } from "node:crypto";
import {
  openResource,
  resourceAad,
  resourceIdentity,
  sealGwa1ForTest,
  sealGwa2,
} from "../src/encryption.mjs";
import { ProtectedFileSystem, ProtectedFiles } from "../src/encrypted-files.mjs";
import { WorkspaceEnv, WorkspaceGuard } from "../src/paths.mjs";
import { readReceipt, writeDocumentReceipt } from "../src/delivery.mjs";
import { ProgressRegistry } from "../src/progress.mjs";
import { PROTOCOL_VERSION, createEmitter, validateInit } from "../src/protocol.mjs";
import { runAgentJob } from "../src/agent.mjs";

const KEY_HEX = Array.from({ length: 32 }, (_, index) => index.toString(16).padStart(2, "0")).join("");
const KEY = Buffer.from(KEY_HEX, "hex");
const NONCE = Buffer.from("000102030405060708090a0b", "hex");
const VECTOR_PLAIN = Buffer.from("# 保密正文\n", "utf8");
const VECTOR_LOGICAL = "pi/runs/run-1/final.md";
const VECTOR_HEX = "47574132000102030405060708090a0b642232a458006d9d6bec346d276e72cd4dea0db75b75f3a33f98214e5db4d3";

function fixture(slot = "real") {
  const base = fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-pi-encrypted-")));
  const first = slot === "decoy" ? "alt-pi" : "pi";
  const root = path.join(base, first, "workspace");
  const run = path.join(base, first, "runs", "run-1");
  for (const directory of [path.join(root, "input"), path.join(root, "work"), path.join(root, "skills", "gongwen-markdown"), run]) {
    fs.mkdirSync(directory, { recursive: true });
  }
  const encryption = { keyHex: KEY_HEX, slot, configRoot: base };
  const files = new ProtectedFiles(encryption);
  return { base, root, run, state: path.join(base, first, "session"), encryption, files };
}

function initWithEncryption(encryption) {
  const space = fixture(encryption.slot === "decoy" ? "decoy" : "real");
  return validateInit({
    type: "init",
    protocol: PROTOCOL_VERSION,
    requestId: "req-encrypted",
    runId: "run-encrypted",
    workspace: {
      root: space.root,
      state: space.state,
      delivery: space.run,
      encryption,
    },
    model: {
      baseUrl: "http://127.0.0.1:9/v1",
      apiKey: "sk-test-only",
      modelId: "test-model",
      contextWindow: 32768,
      maxTokens: 4096,
    },
    task: { request: "写一份通知。", kind: "通知" },
  });
}

test("GWA2 固定向量与 Rust AAD 逐字节一致，可解也可写", () => {
  assert.equal(
    resourceAad("real", VECTOR_LOGICAL).toString("hex"),
    "676f6e6777656e2d7265736f757263652d763200300070690070692f72756e732f72756e2d312f66696e616c2e6d64",
  );
  const sealed = sealGwa2(KEY, "real", VECTOR_LOGICAL, VECTOR_PLAIN, NONCE);
  assert.equal(sealed.toString("hex"), VECTOR_HEX);
  assert.deepEqual(openResource(KEY, "real", VECTOR_LOGICAL, Buffer.from(VECTOR_HEX, "hex")), VECTOR_PLAIN);

  const space = fixture();
  const target = path.join(space.base, VECTOR_LOGICAL);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.writeFileSync(target, sealed);
  assert.equal(space.files.readText(target), VECTOR_PLAIN.toString("utf8"));
  space.files.atomicWrite(target, VECTOR_PLAIN);
  const written = fs.readFileSync(target);
  assert.equal(written.subarray(0, 4).toString("ascii"), "GWA2");
  assert.equal(written.length, VECTOR_PLAIN.length + 32);
  assert.deepEqual(openResource(KEY, "real", VECTOR_LOGICAL, written), VECTOR_PLAIN);
});

test("诱饵槽去掉首段 alt- 后使用相同逻辑 ID，但 AAD 槽位不同", () => {
  const real = resourceIdentity(path.join("/tmp/config", "pi/session/main.jsonl"), { slot: "real", configRoot: "/tmp/config" });
  const decoy = resourceIdentity(path.join("/tmp/config", "alt-pi/session/main.jsonl"), { slot: "decoy", configRoot: "/tmp/config" });
  assert.equal(real.logicalId, "pi/session/main.jsonl");
  assert.equal(decoy.logicalId, real.logicalId);
  assert.notDeepEqual(resourceAad("real", real.logicalId), resourceAad("decoy", decoy.logicalId));
});

test("密文、追加与临时文件均不含明文，读取上限按明文计算", () => {
  const space = fixture();
  const target = path.join(space.run, "final.md");
  const first = "业务明文-第一部分-";
  const second = "业务明文-第二部分";
  space.files.atomicWrite(target, first);
  space.files.appendFile(target, second);
  const raw = fs.readFileSync(target);
  assert.equal(raw.subarray(0, 4).toString("ascii"), "GWA2");
  assert.equal(raw.includes(Buffer.from(first)), false);
  assert.equal(raw.includes(Buffer.from(second)), false);
  assert.equal(space.files.readText(target), first + second);
  assert.equal(space.files.info(target).size, Buffer.byteLength(first + second));
  assert.deepEqual(space.files.readBytes(target, Buffer.byteLength(first + second)), Buffer.from(first + second));
  assert.throws(() => space.files.readBytes(target, Buffer.byteLength(first + second) - 1), /明文读取上限/);
  assert.equal(fs.readdirSync(space.run).some((name) => name.endsWith(".tmp")), false);

  const renamed = path.join(space.run, "renamed.md");
  space.files.renameFile(target, renamed);
  assert.equal(space.files.readText(renamed), first + second, "重命名必须按新路径重新密封");
  assert.throws(() => space.files.readText(target), (error) => error.code === "ENOENT");
});

test("GWA1 只读兼容，覆盖写后升级为 GWA2", () => {
  const space = fixture();
  const target = path.join(space.run, "legacy.json");
  const text = '{"legacy":"旧格式内容"}';
  fs.writeFileSync(target, sealGwa1ForTest(KEY, Buffer.from(text), NONCE));
  assert.equal(space.files.readText(target), text);
  space.files.atomicWrite(target, text);
  assert.equal(fs.readFileSync(target).subarray(0, 4).toString("ascii"), "GWA2");
  assert.equal(space.files.readText(target), text);
});

test("路径越界、..、槽位前缀与跨路径/跨槽密文都被拒绝", () => {
  const space = fixture();
  const target = path.join(space.run, "receipt.json");
  space.files.atomicWrite(target, "认证绑定内容");
  assert.throws(() => space.files.readText(path.join(path.dirname(space.base), "outside.txt")), /configRoot/);
  assert.throws(() => space.files.readText(`${space.base}/pi/runs/run-1/../run-1/receipt.json`), /\.\./);
  assert.throws(
    () => resourceIdentity(path.join(space.base, "alt-pi/runs/run-1/receipt.json"), space.encryption),
    /槽位不一致/,
  );
  const decoy = fixture("decoy");
  assert.throws(
    () => resourceIdentity(path.join(decoy.base, "pi/runs/run-1/receipt.json"), decoy.encryption),
    /槽位不一致/,
  );

  const other = path.join(space.run, "other.json");
  fs.copyFileSync(target, other);
  assert.throws(() => space.files.readText(other), /解密失败/);
  const crossed = path.join(decoy.base, "alt-pi/runs/run-1/crossed.json");
  fs.copyFileSync(target, crossed);
  assert.throws(() => decoy.files.readText(crossed), /解密失败/);
});

test("WorkspaceGuard / WorkspaceEnv 的提交、读取与 fileInfo 都使用明文", async () => {
  const space = fixture();
  const content = "# 加密工作区\n\n正文不能明文落盘。\n";
  space.files.atomicWrite(path.join(space.root, "input/current.md"), "# 当前正文\n");
  space.files.atomicWrite(path.join(space.root, "input/request.md"), "写通知\n");
  const guard = new WorkspaceGuard(space.root, space.files);
  const env = new WorkspaceEnv({ root: space.root, files: space.files });
  const written = await env.writeFile("work/final.md", content, BACKGROUND_CONTEXT);
  assert.equal(written.ok, true);
  const info = await env.fileInfo("work/final.md", BACKGROUND_CONTEXT);
  assert.equal(info.ok, true);
  assert.equal(info.value.size, Buffer.byteLength(content));
  const read = await env.readTextFile("work/final.md", BACKGROUND_CONTEXT);
  assert.deepEqual(read, { ok: true, value: content });
  assert.equal(guard.readSubmission("work/final.md").value.text, content);
  assert.equal(guard.latestCandidate().text, content);
  assert.equal(fs.readFileSync(path.join(space.root, "work/final.md")).includes(Buffer.from("正文不能明文落盘")), false);
});

test("交付回执与阶段进度只落 GWA2，恢复读到相同业务数据", () => {
  const space = fixture();
  const content = "# 交付正文\n\n哈希与路径均受保护。\n";
  writeDocumentReceipt(space.run, {
    runId: "run-1",
    requestId: "req-1",
    relative: "work/final.md",
    content,
    sha256: createHash("sha256").update(content).digest("hex"),
    bytes: Buffer.byteLength(content),
    summary: "测试交付",
    callId: "call-1",
    files: space.files,
  });
  for (const name of ["final.md", "receipt.json"]) {
    const raw = fs.readFileSync(path.join(space.run, name));
    assert.equal(raw.subarray(0, 4).toString("ascii"), "GWA2");
    assert.equal(raw.includes(Buffer.from("测试交付")), false);
  }
  assert.equal(readReceipt(space.run, "run-1", space.files).summary, "测试交付");

  const options = { deliveryDir: space.run, runId: "run-1", requestId: "req-1", identity: { currentSha256: "a" }, files: space.files };
  const registry = new ProgressRegistry(options);
  registry.answers = [{ answers: [{ value: "保密答复" }] }];
  registry.persist();
  assert.equal(fs.readFileSync(path.join(space.run, "progress.json")).includes(Buffer.from("保密答复")), false);
  assert.deepEqual(ProgressRegistry.load(options).answers, registry.answers);
});

test("Durable JSONL 加密提交后可关闭重开恢复，并继续追加提交", async () => {
  const space = fixture();
  const marker = "durable-会话业务明文-不应出现在磁盘";
  const open = () => JsonlStorage.open(
    space.state,
    new ProtectedFileSystem({ cwd: path.dirname(space.state), files: space.files }),
    BACKGROUND_CONTEXT,
    { fsync: true },
  );
  let storage = await open();
  await storage.commit([{ type: "conversation", value: { id: 2 } }], BACKGROUND_CONTEXT);
  await storage.commit([{ type: "entry", value: { id: 3, conversationId: 2, kind: "test", data: { marker } } }], BACKGROUND_CONTEXT);
  await storage.close(BACKGROUND_CONTEXT);

  for (const name of fs.readdirSync(space.state)) {
    const raw = fs.readFileSync(path.join(space.state, name));
    assert.equal(raw.subarray(0, 4).toString("ascii"), "GWA2", name);
    assert.equal(raw.includes(Buffer.from(marker)), false, name);
  }
  storage = await open();
  assert.deepEqual(await storage.conversation(2, BACKGROUND_CONTEXT), { id: 2 });
  assert.equal((await storage.entry(3, BACKGROUND_CONTEXT)).entry.data.marker, marker);
  await storage.commit([{ type: "entry", value: { id: 4, conversationId: 2, kind: "test", data: "第二次提交" } }], BACKGROUND_CONTEXT);
  await storage.close(BACKGROUND_CONTEXT);

  storage = await open();
  assert.equal((await storage.entry(4, BACKGROUND_CONTEXT)).entry.data, "第二次提交");
  await storage.close(BACKGROUND_CONTEXT);
});

test("旧 SQLite 会话文件不会被当成 JSONL 或自动迁移", async () => {
  const space = fixture();
  fs.writeFileSync(space.state, Buffer.from("SQLite format 3\u0000旧会话", "utf8"));
  await assert.rejects(() => JsonlStorage.open(
    space.state,
    new ProtectedFileSystem({ cwd: path.dirname(space.state), files: space.files }),
    BACKGROUND_CONTEXT,
  ));
  assert.equal(fs.readFileSync(space.state).subarray(0, 15).toString("ascii"), "SQLite format 3");
});

test("完整 faux 写作链在加密工作区、JSONL 与交付目录之间往返", { timeout: 15_000 }, async () => {
  const space = fixture();
  space.files.atomicWrite(path.join(space.root, "input/current.md"), "# 当前正文\n");
  space.files.atomicWrite(path.join(space.root, "input/request.md"), "写一份通知。\n");
  space.files.atomicWrite(path.join(space.root, "skills/gongwen-markdown/SKILL.md"), "# 技能\n");
  const config = validateInit({
    type: "init",
    protocol: PROTOCOL_VERSION,
    requestId: "req-full-encrypted",
    runId: "run-full-encrypted",
    workspace: { root: space.root, state: space.state, delivery: space.run, encryption: space.encryption },
    model: { baseUrl: "http://127.0.0.1:9/v1", apiKey: "", modelId: "faux-model", contextWindow: 32768, maxTokens: 4096 },
    task: { request: "写一份通知。", kind: "通知" },
  });
  const faux = fauxProvider({ provider: "faux-encrypted" });
  const models = createModels();
  models.setProvider(faux.provider);
  const content = "# 完整加密链\n\n交付内容不落地为明文。\n";
  faux.setResponses([
    fauxAssistantMessage([fauxToolCall("write", { path: "work/final.md", content })], { stopReason: "toolUse" }),
    fauxAssistantMessage([fauxToolCall("submit_document", { path: "work/final.md", summary: "完成加密链测试" })], { stopReason: "toolUse" }),
  ]);
  const result = await runAgentJob({ config, models, model: faux.getModel(), emit: () => {} });
  assert.equal(result.snapshot.content, content);
  for (const target of [
    path.join(space.root, "work/final.md"),
    path.join(space.run, "final.md"),
    path.join(space.run, "receipt.json"),
    ...fs.readdirSync(space.state).map((name) => path.join(space.state, name)),
  ]) {
    const raw = fs.readFileSync(target);
    assert.equal(raw.subarray(0, 4).toString("ascii"), "GWA2", target);
    assert.equal(raw.includes(Buffer.from("交付内容不落地为明文")), false, target);
  }
});

test("旧 SQLite 不存在时，已交付回执仍可在加密目录直接重放", async () => {
  const space = fixture();
  space.files.atomicWrite(path.join(space.root, "input/current.md"), "# 当前正文\n");
  space.files.atomicWrite(path.join(space.root, "input/request.md"), "写一份通知。\n");
  const content = "# 旧版已交付成品\n";
  const receipt = writeDocumentReceipt(space.run, {
    runId: "run-receipt-only",
    requestId: "req-receipt-only",
    relative: "work/final.md",
    content,
    sha256: createHash("sha256").update(content).digest("hex"),
    bytes: Buffer.byteLength(content),
    summary: "旧版成品",
    callId: "call-old",
    files: space.files,
  });
  const config = validateInit({
    type: "init",
    protocol: PROTOCOL_VERSION,
    requestId: "req-receipt-only",
    runId: "run-receipt-only",
    workspace: { root: space.root, state: space.state, delivery: space.run, encryption: space.encryption },
    model: { baseUrl: "http://127.0.0.1:9/v1", apiKey: "", modelId: "unused", contextWindow: 32768, maxTokens: 4096 },
    task: { request: "写一份通知。", kind: "通知" },
    recovery: { mode: "resume", requestId: "req-receipt-only", continuationId: "continuation-new" },
  });
  const result = await runAgentJob({ config, emit: () => {} });
  assert.deepEqual(result, { receipt });
  assert.equal(fs.existsSync(space.state), false, "回执重放不应打开或创建 Durable 状态");
});

test("init 校验加密上下文，keyHex 纳入秘密抹除且没有密钥也能保持明文协议", () => {
  const space = fixture();
  const config = initWithEncryption(space.encryption);
  assert.deepEqual(config.workspace.encryption, space.encryption);
  for (const change of [
    { ...space.encryption, keyHex: "xyz" },
    { ...space.encryption, slot: "other" },
    { ...space.encryption, configRoot: "relative" },
  ]) {
    assert.throws(() => initWithEncryption(change), /workspace\.encryption/);
  }
  const lines = [];
  const emit = createEmitter({ write: (line) => lines.push(line), secrets: [KEY_HEX] });
  emit({ type: "error", message: `不得泄露 ${KEY_HEX}` });
  assert.equal(lines[0].includes(KEY_HEX), false);
  assert.ok(lines[0].includes("[已隐藏]"));
});
