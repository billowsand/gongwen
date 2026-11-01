import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { WorkspaceEnv, WorkspaceGuard, resolveInside } from "../src/paths.mjs";

function workspace() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-pi-ws-"));
  fs.mkdirSync(path.join(root, "input"), { recursive: true });
  fs.mkdirSync(path.join(root, "work", "sections"), { recursive: true });
  fs.mkdirSync(path.join(root, "skills", "gongwen-markdown"), { recursive: true });
  fs.writeFileSync(path.join(root, "input", "current.md"), "# 当前正文\n");
  fs.writeFileSync(path.join(root, "work", "final.md"), "# 候选\n");
  return root;
}

test("guard accepts paths inside the workspace and rejects escapes", () => {
  const root = workspace();
  const guard = new WorkspaceGuard(root);
  assert.equal(guard.read("work/final.md").ok, true);
  assert.equal(guard.read("input/current.md").ok, true);

  for (const attempt of [
    "work/../../outside.md",
    path.join(path.dirname(root), "outside.md"),
    process.platform === "win32" ? "C:\\Windows\\win.ini" : "/etc/passwd",
  ]) {
    const result = guard.read(attempt);
    assert.equal(result.ok, false, attempt);
    assert.match(result.error.message, /越出工作区/);
  }
});

test("write is limited to work/, even for an otherwise readable file", () => {
  const root = workspace();
  const guard = new WorkspaceGuard(root);
  assert.equal(guard.write("work/final.md").ok, true);
  assert.equal(guard.write("work/sections/a.md").ok, true);
  const input = guard.write("input/current.md");
  assert.equal(input.ok, false);
  assert.match(input.error.message, /只能写 work/);
  const skills = guard.write("skills/gongwen-markdown/SKILL.md");
  assert.equal(skills.ok, false);
});

test("a junction or symlink cannot carry a write outside the workspace", (t) => {
  const root = workspace();
  const outside = fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-pi-outside-"));
  const link = path.join(root, "work", "link");
  try {
    fs.symlinkSync(outside, link, process.platform === "win32" ? "junction" : "dir");
  } catch (error) {
    t.skip(`本机不能创建链接：${error.message}`);
    return;
  }
  const result = resolveInside(root, root, "work/link/escape.md", { mode: "write" });
  assert.equal(result.ok, false);
  assert.match(result.error.message, /符号链接|工作区外/);
});

test("a broken link pointing outside cannot be used to create a file there", (t) => {
  const root = workspace();
  const outside = fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-pi-outside-"));
  const missingTarget = path.join(outside, "not-created-yet.md");
  const link = path.join(root, "work", "broken.md");
  try {
    fs.symlinkSync(missingTarget, link, "file");
  } catch (error) {
    t.skip(`本机不能创建文件符号链接：${error.message}`);
    return;
  }
  const result = resolveInside(root, root, "work/broken.md", { mode: "write" });
  assert.equal(result.ok, false);
  assert.match(result.error.message, /符号链接|工作区外/);
  assert.equal(fs.existsSync(missingTarget), false);
});

test("WorkspaceEnv reads work and refuses writes to input", async () => {
  const root = workspace();
  const env = new WorkspaceEnv({ root });
  // context 只是穿透给底层；文件工具不会用到它。
  const context = {};
  const read = await env.readTextFile("work/final.md", context);
  assert.equal(read.ok, true);
  assert.match(read.value, /候选/);
  const write = await env.writeFile("input/current.md", "# 改写\n", context);
  assert.equal(write.ok, false);
  const normal = await env.writeFile("work/final.md", "# 新候选\n", context);
  assert.equal(normal.ok, true);
  assert.equal(fs.readFileSync(path.join(root, "work", "final.md"), "utf8"), "# 新候选\n");
});

test("submission is snapshotted as a string and survives later edits", () => {
  const root = workspace();
  const guard = new WorkspaceGuard(root);
  const read = guard.readSubmission("work/final.md");
  assert.equal(read.ok, true);
  fs.writeFileSync(path.join(root, "work", "final.md"), "# 后来的改动\n");
  assert.equal(read.value.text, "# 候选\n");
  assert.equal(read.value.relative, "work/final.md");
});

test("submission refuses empty, missing and oversized files", () => {
  const root = workspace();
  const guard = new WorkspaceGuard(root);
  fs.writeFileSync(path.join(root, "work", "empty.md"), "  \n");
  assert.equal(guard.readSubmission("work/empty.md").ok, false);
  assert.equal(guard.readSubmission("work/missing.md").ok, false);
  fs.writeFileSync(path.join(root, "work", "big.md"), "x".repeat(100));
  assert.equal(guard.readSubmission("work/big.md", { maxFileBytes: 20 }).ok, false);
});

test("submission validates UTF-8 bytes rather than rejecting a literal replacement character", () => {
  const root = workspace();
  const guard = new WorkspaceGuard(root);
  const target = path.join(root, "work", "encoding.md");
  fs.writeFileSync(target, Buffer.from([0xff, 0xfe, 0x61]));
  assert.equal(guard.readSubmission("work/encoding.md").ok, false);
  fs.writeFileSync(target, "# 包含原文符号\n\n�\n", "utf8");
  assert.equal(guard.readSubmission("work/encoding.md").ok, true);
});

test("latest candidate prefers work/final.md", () => {
  const root = workspace();
  fs.writeFileSync(path.join(root, "work", "sections", "a.md"), "# 章节\n");
  const newer = new Date(Date.now() + 10_000);
  fs.utimesSync(path.join(root, "work", "sections", "a.md"), newer, newer);
  const guard = new WorkspaceGuard(root);
  const candidate = guard.latestCandidate();
  assert.equal(candidate.relative, "work/final.md");
});

test("workspace quota covers multiple files and permits replacing a file", async () => {
  const root = workspace();
  const env = new WorkspaceEnv({ root, maxFileBytes: 100, maxTotalBytes: 20 });
  const rejected = await env.writeFile("work/another.md", "x".repeat(20), {});
  assert.equal(rejected.ok, false);
  assert.match(rejected.error.message, /总文件体积/);
  assert.equal(fs.existsSync(path.join(root, "work/another.md")), false);
  assert.equal((await env.writeFile("work/final.md", "x", {})).ok, true);
  assert.equal((await env.writeFile("work/another.md", "x".repeat(19), {})).ok, true);
});

test("workspace quota counts parent directories before creating them", async () => {
  const root = workspace();
  const env = new WorkspaceEnv({ root, maxWorkEntries: 3 });
  const rejected = await env.writeFile("work/new/nested/file.md", "正文", {});
  assert.equal(rejected.ok, false);
  assert.match(rejected.error.message, /数量超过/);
  assert.equal(fs.existsSync(path.join(root, "work/new")), false);
});
