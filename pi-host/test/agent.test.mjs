import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  createModels,
  fauxAssistantMessage,
  fauxProvider,
  fauxText,
  fauxToolCall,
} from "@earendil-works/pi-ai";
import { AgentError, runAgentJob } from "../src/agent.mjs";
import { PROTOCOL_VERSION, validateInit } from "../src/protocol.mjs";

// 这些测试用 faux provider，只验证真实 Durable harness、工作区限制与提交协议；
// 生产入口 main.mjs 永远构造 OpenAI 兼容 provider，不走这里。

function makeWorkspace() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-pi-agent-"));
  fs.mkdirSync(path.join(root, "input"), { recursive: true });
  fs.mkdirSync(path.join(root, "work"), { recursive: true });
  fs.mkdirSync(path.join(root, "skills", "gongwen-markdown"), { recursive: true });
  fs.writeFileSync(path.join(root, "input", "current.md"), "# 当前正文\n\n原有内容。\n");
  fs.writeFileSync(path.join(root, "input", "request.md"), "把正文改短。\n");
  fs.writeFileSync(path.join(root, "skills", "gongwen-markdown", "SKILL.md"), "# 技能\n");
  return { root, state: path.join(root, "..", `${path.basename(root)}.sqlite`) };
}

function makeConfig(workspace) {
  return validateInit({
    type: "init",
    protocol: PROTOCOL_VERSION,
    requestId: "req-test",
    runId: "run-test",
    workspace: {
      root: workspace.root,
      state: workspace.state,
      current: "input/current.md",
      request: "input/request.md",
      skills: "skills/gongwen-markdown",
    },
    model: {
      baseUrl: "http://127.0.0.1:9/v1",
      apiKey: "sk-test-not-a-real-key",
      modelId: "faux-model",
      contextWindow: 32768,
      maxTokens: 4096,
      reasoning: false,
    },
    task: { request: "把正文改短。", kind: "通知", systemDate: "2026-11-01" },
  });
}

function fakeBackend() {
  const faux = fauxProvider({ provider: "faux-test" });
  const models = createModels();
  models.setProvider(faux.provider);
  const model = faux.getModel();
  return { faux, models, model };
}

test("a written file submitted through submit_document becomes the delivered snapshot", async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const { faux, models, model } = fakeBackend();
  faux.setResponses([
    fauxAssistantMessage(
      [fauxToolCall("write", { path: "work/final.md", content: "# 成品\n\n改短后的正文。\n" })],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage(
      [fauxToolCall("submit_document", { path: "work/final.md", summary: "改短正文" })],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage([fauxText("已提交。")]),
  ]);
  const events = [];
  const result = await runAgentJob({ config, models, model, emit: (event) => events.push(event) });
  assert.ok(result.snapshot, "应当有提交");
  assert.equal(result.snapshot.path, "work/final.md");
  assert.match(result.snapshot.content, /改短后的正文/);
  assert.match(result.snapshot.sha256, /^[0-9a-f]{64}$/);
  assert.ok(events.some((event) => event.type === "candidate" && event.path === "work/final.md"));
});

test("editing the file after submit does not change the delivered snapshot", async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const { faux, models, model } = fakeBackend();
  faux.setResponses([
    fauxAssistantMessage(
      [fauxToolCall("write", { path: "work/final.md", content: "# 第一版\n" })],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage(
      [fauxToolCall("submit_document", { path: "work/final.md", summary: "第一版" })],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage(
      [fauxToolCall("write", { path: "work/final.md", content: "# 提交后的改动\n" })],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage([fauxText("结束")]),
  ]);
  const result = await runAgentJob({ config, models, model, emit: () => {} });
  assert.equal(result.snapshot.content, "# 第一版\n");
});

test("a run that ends without submit_document is not a delivery", async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const { faux, models, model } = fakeBackend();
  faux.setResponses([fauxAssistantMessage([fauxText("我写好了，见聊天。")])]);
  await assert.rejects(
    () => runAgentJob({ config, models, model, emit: () => {} }),
    (error) => error instanceof AgentError && error.code === "not_submitted",
  );
});

test("submitting a path outside work/ is refused", async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const { faux, models, model } = fakeBackend();
  fs.writeFileSync(path.join(workspace.root, "input", "current.md"), "# 当前正文\n");
  faux.setResponses([
    fauxAssistantMessage(
      [fauxToolCall("submit_document", { path: "input/current.md", summary: "越界" })],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage([fauxText("结束")]),
  ]);
  await assert.rejects(
    () => runAgentJob({ config, models, model, emit: () => {} }),
    (error) => error instanceof AgentError && error.code === "not_submitted",
  );
});

test("an empty submitted file is refused", async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const { faux, models, model } = fakeBackend();
  fs.writeFileSync(path.join(workspace.root, "work", "empty.md"), "   \n");
  faux.setResponses([
    fauxAssistantMessage(
      [fauxToolCall("submit_document", { path: "work/empty.md", summary: "空文件" })],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage([fauxText("结束")]),
  ]);
  await assert.rejects(
    () => runAgentJob({ config, models, model, emit: () => {} }),
    (error) => error instanceof AgentError && error.code === "not_submitted",
  );
});
