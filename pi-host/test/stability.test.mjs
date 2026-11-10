//! 稳定性回归：崩溃残留临时文件、明文长度回推、追加/读取上限、
//! 锁文件受控自愈、恢复时会话丢失、字面/realpath 路径口径一致。

import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { JsonlStorage } from "@earendil-works/pi-durable/storage/jsonl";
import {
  resourceAad,
  resourceIdentity,
  sealGwa1ForTest,
  sealGwa2,
} from "../src/encryption.mjs";
import {
  DEFAULT_READ_MAX_BYTES,
  MAX_ENCRYPTED_APPEND_BYTES,
  ProtectedFileSystem,
  ProtectedFiles,
  cleanStaleTemps,
  isAtomicTempName,
} from "../src/encrypted-files.mjs";
import { WorkspaceEnv } from "../src/paths.mjs";
import { acquireOwner } from "../src/owner.mjs";
import { PROTOCOL_VERSION, validateInit } from "../src/protocol.mjs";
import { assertResumeSession, runAgentJob } from "../src/agent.mjs";

const KEY_HEX = "ab".repeat(32);
const KEY = Buffer.from(KEY_HEX, "hex");

function tempBase(prefix) {
  return fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), prefix)));
}

/** 与生产布局一致：配置目录/pi/{workspace,runs,session}。 */
function fixture(slot = "real") {
  const base = tempBase("gongwen-pi-stability-");
  const first = slot === "decoy" ? "alt-pi" : "pi";
  const root = path.join(base, first, "workspace");
  const state = path.join(base, first, "session");
  const delivery = path.join(base, first, "runs", "run-1");
  for (const dir of [path.join(root, "input"), path.join(root, "work"), path.join(root, "skills", "gongwen-markdown"), delivery]) {
    fs.mkdirSync(dir, { recursive: true });
  }
  const encryption = { keyHex: KEY_HEX, slot, configRoot: base };
  const files = new ProtectedFiles(encryption);
  return { base, root, state, delivery, encryption, files };
}

function staleTemp(dir) {
  return path.join(dir, `.${process.pid}-11111111-2222-4333-8444-555555555555.tmp`);
}

function initConfig(space, extra = {}) {
  return validateInit({
    type: "init",
    protocol: PROTOCOL_VERSION,
    requestId: "req-stability",
    runId: "run-stability",
    workspace: {
      root: space.root,
      state: space.state,
      delivery: space.delivery,
      encryption: space.encryption,
    },
    model: {
      baseUrl: "http://127.0.0.1:9/v1",
      apiKey: "",
      modelId: "test-model",
      contextWindow: 32768,
      maxTokens: 4096,
    },
    task: { request: "写一份通知。", kind: "通知" },
    ...extra,
  });
}

const openStorage = (space) =>
  JsonlStorage.open(
    space.state,
    new ProtectedFileSystem({ cwd: path.dirname(space.state), files: space.files }),
    BACKGROUND_CONTEXT,
    { fsync: true },
  );

test("崩溃残留的半截密文临时文件：listDir 跳过、清理后会话仍可打开", async () => {
  const space = fixture();
  let storage = await openStorage(space);
  await storage.commit([{ type: "conversation", value: { id: 7 } }], BACKGROUND_CONTEXT);
  console.error("DEBUG-111-after-commit", storage.mainPath, fs.existsSync(storage.mainPath), space.state);
  await storage.close(BACKGROUND_CONTEXT);

  // 模拟写临时文件途中被 SIGKILL：只写了一半封套。
  const partial = path.join(space.state, path.basename(staleTemp(space.state)));
  fs.writeFileSync(partial, Buffer.concat([Buffer.from("GWA2", "ascii"), Buffer.alloc(10)]));
  assert.equal(isAtomicTempName(path.basename(partial)), true);

  // 1) listDir 不再因半截密文抛错，也不把临时文件当资源。
  const listed = space.files.listDir(space.state);
  assert.equal(listed.some((entry) => entry.name === path.basename(partial)), false);
  // 2) 未清理时也能打开（recover 的 listDir 会跳过它）。
  storage = await openStorage(space);
  const reopened = await storage.conversation(7, BACKGROUND_CONTEXT);
  if (reopened === undefined) {
    const mainPath = path.join(space.state, "main.jsonl");
    let main = "";
    let head = "";
    try {
      main = space.files.readText(mainPath);
      head = fs.readFileSync(mainPath).subarray(0, 8).toString("hex");
    } catch (error) {
      main = `read failed: ${error?.message ?? error}`;
    }
    const found = [];
    const walk = (dir, depth) => {
      if (depth > 6 || found.length > 20) return;
      let entries = [];
      try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { return; }
      for (const entry of entries) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) walk(full, depth + 1);
        else if (entry.name === "main.jsonl") found.push(full);
      }
    };
    walk(space.base, 0);
    console.error("DEBUG-111", JSON.stringify({ cwd: process.cwd(), listing: fs.readdirSync(space.state), head, main, found }));
  }
  assert.deepEqual(reopened, { id: 7 });
  await storage.close(BACKGROUND_CONTEXT);
  // 3) 启动清理会删掉它。
  assert.equal(cleanStaleTemps(space.state), 1);
  assert.equal(fs.existsSync(partial), false);
  storage = await openStorage(space);
  assert.deepEqual(await storage.conversation(7, BACKGROUND_CONTEXT), { id: 7 });
  await storage.close(BACKGROUND_CONTEXT);
});

test("work/ 里残留半截临时文件不影响模型写入（配额扫描跳过）", async () => {
  const space = fixture();
  fs.writeFileSync(staleTemp(path.join(space.root, "work")), Buffer.concat([Buffer.from("GWA2", "ascii"), Buffer.alloc(8)]));
  const env = new WorkspaceEnv({ root: space.root, cwd: space.root, files: space.files });
  const written = await env.writeFile(path.join(space.root, "work", "draft.md"), "# 草稿\n", BACKGROUND_CONTEXT);
  assert.equal(written.ok, true, JSON.stringify(written));
  const read = await env.readTextFile("work/draft.md", BACKGROUND_CONTEXT);
  assert.equal(read.ok, true);
  assert.equal(read.value, "# 草稿\n");
});

test("info() 用物理长度回推明文长度：不解密、封套不完整按损坏报错", () => {
  const space = fixture();
  // 故意用另一条逻辑路径密封：旧实现 info() 会整份解密并因 AAD 不符失败。
  const target = path.join(space.delivery, "run.json");
  const plain = Buffer.from("1234567890", "utf8");
  fs.writeFileSync(target, sealGwa2(KEY, "real", "pi/runs/run-1/别的文件.json", plain));
  assert.equal(space.files.info(target).size, plain.length);
  // 真读仍然 fail-closed：AAD 不符必须报解密失败。
  assert.throws(() => space.files.readText(target), /解密失败/);

  const short = path.join(space.delivery, "short.json");
  fs.writeFileSync(short, Buffer.concat([Buffer.from("GWA2", "ascii"), Buffer.alloc(8)]));
  assert.throws(() => space.files.info(short), /封套不完整|长度不符/);

  // 明文文件按 stat 大小。
  const text = path.join(space.delivery, "plain.txt");
  fs.writeFileSync(text, "明文内容");
  assert.equal(space.files.info(text).size, Buffer.byteLength("明文内容"));
  assert.equal(space.files.info(space.delivery).kind, "directory");
});

test("加密 append 有明文上限（EFBIG）、未给上限的读取有默认上限", () => {
  const space = fixture();
  const files = new ProtectedFiles(space.encryption, { maxAppendBytes: 64, maxReadBytes: 128 });
  const target = path.join(space.delivery, "session.jsonl");
  files.atomicWrite(target, "x".repeat(40));
  files.appendFile(target, "y".repeat(10));
  assert.equal(files.readText(target).length, 50);
  assert.throws(
    () => files.appendFile(target, "z".repeat(20)),
    (thrown) => {
      assert.equal(thrown.code, "EFBIG");
      assert.match(thrown.message, /上限/);
      assert.match(thrown.message, /会话|文件过大/);
      return true;
    },
  );
  // 超限失败不能破坏旧内容。
  assert.equal(files.readText(target).length, 50);

  const big = path.join(space.delivery, "big.bin");
  files.atomicWrite(big, Buffer.alloc(200));
  assert.throws(() => files.readBytes(big), (thrown) => thrown.code === "EFBIG");

  // 生产默认值就是 64 MiB / 256 MiB。
  assert.equal(MAX_ENCRYPTED_APPEND_BYTES, 64 * 1024 * 1024);
  assert.equal(DEFAULT_READ_MAX_BYTES, 256 * 1024 * 1024);
});

test("被误封的 host-owner.lock 受控自愈：不碰业务文件、owner_busy 语义不变", () => {
  const dir = tempBase("gongwen-pi-owner-");
  // GWA2 头 = 解锁迁移误封的锁文件。
  const lock = path.join(dir, "host-owner.lock");
  fs.writeFileSync(lock, Buffer.concat([Buffer.from("GWA2", "ascii"), Buffer.alloc(32)]));
  const owner = acquireOwner(lock, { runId: "run-1" });
  assert.equal(fs.readFileSync(lock).subarray(0, 4).toString("latin1").startsWith("GWA"), false);
  // 已持有锁时另一个 host 仍然拿到 owner_busy（不能自愈成双写）。
  assert.throws(() => acquireOwner(lock, {}), (thrown) => thrown.code === "owner_busy");
  owner.release();
  const again = acquireOwner(lock, {});
  again.release();

  // GWA1 同样自愈。
  const legacy = path.join(dir, "host-owner.sqlite");
  fs.writeFileSync(legacy, sealGwa1ForTest(KEY, Buffer.from("old lock")));
  const legacyOwner = acquireOwner(legacy, {});
  legacyOwner.release();

  // 不是已知锁名的文件绝不自愈、绝不删除。
  const other = path.join(dir, "notes.txt");
  fs.writeFileSync(other, Buffer.concat([Buffer.from("GWA2", "ascii"), Buffer.alloc(8)]));
  assert.throws(() => acquireOwner(other, {}), (thrown) => thrown.code === "owner_failed" || thrown.code === "owner_open");
  assert.equal(fs.existsSync(other), true);
});

test("resume：progress 有记录但 main.jsonl 缺失/为空 → 明确拒绝，无进度时不变", async () => {
  const space = fixture();
  space.files.atomicWrite(path.join(space.root, "input", "current.md"), "# 当前正文\n");
  space.files.atomicWrite(path.join(space.root, "input", "request.md"), "写一份通知。\n");
  const progressPath = path.join(space.delivery, "progress.json");
  const progress = JSON.stringify({
    version: 1,
    runId: "run-stability",
    requestId: "req-stability",
    identity: {},
    records: [{ unit: "outline", bytes: 12, sha256: "0".repeat(64) }],
    answers: [],
    evidence: [],
  });

  // 没有进度文件：保持旧行为（允许继续，由 Durable 新建空会话）。
  const config = initConfig(space, { recovery: { mode: "resume", requestId: "req-stability" } });
  assert.doesNotThrow(() => assertResumeSession(config, space.files));

  // 进度在、会话缺失：明确报 resume_session_missing。
  space.files.atomicWrite(progressPath, progress);
  assert.throws(
    () => assertResumeSession(config, space.files),
    (thrown) => thrown.code === "resume_session_missing" && /Durable 会话/.test(thrown.message),
  );

  // 会话为空（密封后的空文件）：同样拒绝。
  space.files.atomicWrite(path.join(space.state, "main.jsonl"), "");
  assert.throws(() => assertResumeSession(config, space.files), (thrown) => thrown.code === "resume_session_missing");

  // 会话有内容：不报错。
  space.files.atomicWrite(
    path.join(space.state, "main.jsonl"),
    `${JSON.stringify({ format: 1, type: "commit", seq: 1, writes: [] })}\n`,
  );
  assert.doesNotThrow(() => assertResumeSession(config, space.files));

  // 端到端：host 在恢复入口就拒绝，不静默重跑。
  fs.rmSync(path.join(space.state, "main.jsonl"));
  await assert.rejects(
    () => runAgentJob({ config, files: space.files, emit: () => {} }),
    (thrown) => thrown.code === "resume_session_missing",
  );
});

test("字面路径与 realpath 路径算出同一逻辑 ID/AAD，且能跨口径读写", () => {
  if (process.platform === "win32") {
    // Windows 建符号链接需要额外权限，CI 上跳过；逻辑本身由上一句断言覆盖。
    return;
  }
  const base = tempBase("gongwen-pi-link-");
  const realRoot = path.join(base, "real-config");
  fs.mkdirSync(realRoot, { recursive: true });
  const linkRoot = path.join(base, "config-link");
  fs.symlinkSync(realRoot, linkRoot, "dir");
  const space = {
    base,
    linkRoot,
    root: path.join(linkRoot, "pi", "workspace"),
  };
  fs.mkdirSync(path.join(space.root, "work"), { recursive: true });
  const files = new ProtectedFiles({ keyHex: KEY_HEX, slot: "real", configRoot: linkRoot });

  const literal = path.join(linkRoot, "pi", "workspace", "input", "current.md");
  const real = path.join(realRoot, "pi", "workspace", "input", "current.md");
  const fromLiteral = files.identity(literal);
  const fromReal = files.identity(real);
  assert.equal(fromLiteral.logicalId, "pi/workspace/input/current.md");
  assert.equal(fromReal.logicalId, fromLiteral.logicalId);
  assert.deepEqual(resourceAad("real", fromReal.logicalId), resourceAad("real", fromLiteral.logicalId));

  // 字面口径写、realpath 口径读（Node 的守卫会把工作区路径 realpath 一遍）。
  files.atomicWrite(literal, "# 正文\n");
  assert.equal(files.readText(real), "# 正文\n");
  files.atomicWrite(real, "# 第二版\n");
  assert.equal(files.readText(literal), "# 第二版\n");

  // 端到端：WorkspaceEnv 用字面 root，内部 realpath 后仍能读到 Rust 写的密文。
  const env = new WorkspaceEnv({ root: space.root, cwd: space.root, files });
  return (async () => {
    const read = await env.readTextFile("input/current.md", BACKGROUND_CONTEXT);
    assert.equal(read.ok, true, JSON.stringify(read));
    assert.equal(read.value, "# 第二版\n");
    // 模型/工具给的是相对路径（生产口径）：守卫内部 realpath 后仍要落到同一 AAD。
    const written = await env.writeFile("work/draft.md", "# 草稿\n", BACKGROUND_CONTEXT);
    assert.equal(written.ok, true, JSON.stringify(written));
    assert.equal(files.readText(path.join(realRoot, "pi", "workspace", "work", "draft.md")), "# 草稿\n");
  })();
});

test("路径身份仍然 fail-closed：越界、..、槽位不符都被拒绝", () => {
  const base = tempBase("gongwen-pi-guard-");
  const encryption = { keyHex: KEY_HEX, slot: "real", configRoot: base };
  const files = new ProtectedFiles(encryption);
  for (const bad of [
    `${base}/../outside.md`,
    `${base}/pi/../pi/x.md`,
    path.join(base, "alt-pi", "x.md"),
    base,
  ]) {
    assert.throws(() => files.identity(bad), /资源路径|槽位/);
  }
  // 诱饵槽必须带 alt- 首段，且 logicalId 与真实槽一致。
  const decoy = new ProtectedFiles({ ...encryption, slot: "decoy" });
  assert.throws(() => decoy.identity(path.join(base, "pi", "x.md")), /槽位/);
  assert.equal(decoy.identity(path.join(base, "alt-pi", "x.md")).logicalId, "pi/x.md");
  // realConfigRoot 口径不影响越界判定。
  assert.throws(
    () => resourceIdentity(`${base}/../outside.md`, { slot: "real", configRoot: base, realConfigRoot: base }),
    /不得包含 \.\./,
  );
});
