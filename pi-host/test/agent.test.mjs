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
import { AgentError, runAgentJob, writingCompaction } from "../src/agent.mjs";
import { PROTOCOL_VERSION, validateInit } from "../src/protocol.mjs";
import { clampMaxTokensToContext } from "@earendil-works/pi-ai/api/simple-options";

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
  const delivery = path.join(root, "..", `${path.basename(root)}-run`);
  fs.mkdirSync(delivery, { recursive: true });
  return { root, state: path.join(root, "..", `${path.basename(root)}.sqlite`), delivery };
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
      delivery: workspace.delivery,
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

test("同轮批量工具经真实 Durable harness 顺序往返后完成正文与回执交付", { timeout: 15_000 }, async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const { faux, models, model } = fakeBackend();
  const content = "# 批量工具测试\n\n已使用两项资料。\n";
  faux.setResponses([
    fauxAssistantMessage([
      fauxToolCall("kb_search", { query: "自造知识资料" }, { id: "kb-batch" }),
      fauxToolCall("ms_search", { query: "自造稿件资料" }, { id: "ms-batch" }),
    ], { stopReason: "toolUse" }),
    context => {
      const messages = JSON.stringify(context);
      assert.ok(messages.includes("knowledge-marker"));
      assert.ok(messages.includes("manuscript-marker"));
      return fauxAssistantMessage([fauxToolCall("write", { path: "work/batch.md", content })], { stopReason: "toolUse" });
    },
    fauxAssistantMessage([fauxToolCall("submit_document", { path: "work/batch.md", summary: "两项资料已返回" })], { stopReason: "toolUse" }),
  ]);
  let bridge;
  let inFlight = 0;
  let maxInFlight = 0;
  let maxPending = 0;
  const names = [];
  const result = await runAgentJob({ config, models, model, onBridge: value => { bridge = value; }, emit: event => {
    if (event.type !== "tool_request") return;
    names.push(event.name);
    maxInFlight = Math.max(maxInFlight, ++inFlight);
    // 模拟 Rust 单槽：结果返回前不接受下一条，特意延迟以暴露同批并发。
    setTimeout(() => {
      maxPending = Math.max(maxPending, bridge.pending());
      inFlight -= 1;
      bridge.handle({ type: "tool_result", callId: event.callId, ok: true,
        result: { marker: event.name === "kb_search" ? "knowledge-marker" : "manuscript-marker" } });
    }, 30);
  } });
  assert.equal(maxInFlight, 1, "不能同时向 Rust 发出两条工具请求");
  assert.equal(maxPending, 2, "真实 SDK 应已同轮发起两项调用，第二项实际在桥内排队");
  assert.deepEqual(names, ["kb_search", "ms_search"]);
  assert.equal(result.snapshot.content, content);
  assert.equal(fs.readFileSync(path.join(workspace.delivery, "final.md"), "utf8"), content);
  assert.equal(JSON.parse(fs.readFileSync(path.join(workspace.delivery, "receipt.json"), "utf8")).kind, "document");
});

test("a long 32k task compacts before continuing instead of shrinking output to one token", async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const faux = fauxProvider({ provider: "faux-32k", models: [{ id: "faux-model", contextWindow: 32_768, maxTokens: 16_384 }] });
  const models = createModels();
  models.setProvider(faux.provider);
  const model = faux.getModel();
  config.task.request += "\n" + "Background context. ".repeat(700);
  const content = "# 已写出的稿件\n\n保留正文。\n";
  fs.writeFileSync(path.join(workspace.root, "work/draft.md"), content);
  fs.writeFileSync(path.join(workspace.root, "input/long.md"), "Read-only source text. ".repeat(2_000));
  const reading = fauxAssistantMessage([fauxToolCall("read", { path: "input/long.md" })], { stopReason: "toolUse" });
  // 持续读取长材料，必须在剩余输出耗尽前切出历史并运行真实 Durable 压缩。
  let compacted = false;
  faux.setResponses([
    reading,
    (context) => {
      assert.match(JSON.stringify(context), /context summarization assistant/);
      compacted = true;
      return fauxAssistantMessage([fauxText("任务：提交现有 work/draft.md；材料仍在 input/long.md，不编造事实。")]);
    },
    (context) => {
      const output = clampMaxTokensToContext(model, context, model.maxTokens);
      assert.ok(output >= 4_096, `压缩后必须实际留出输出空间，当前只有 ${output}`);
      return fauxAssistantMessage([fauxToolCall("submit_document", { path: "work/draft.md", summary: "按已有稿件完成交付" })], { stopReason: "toolUse" });
    },
  ]);
  const result = await runAgentJob({ config, models, model, emit: () => {} });
  assert.ok(compacted, "必须实际触发 Durable 压缩");
  assert.equal(result.snapshot.content, content);
  const policy = writingCompaction(model);
  assert.equal(policy.reserveTokens, 16_384);
  assert.ok(policy.keepRecentTokens <= 4_096);
});

test("a length-truncated turn continues with path-based checking and explicit submission", async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const { faux, models, model } = fakeBackend();
  const content = "# 成稿\n\n已经写出的正文。\n";
  faux.setResponses([
    fauxAssistantMessage([fauxToolCall("write", { path: "work/draft.md", content })], { stopReason: "toolUse" }),
    fauxAssistantMessage([fauxText("检查参数尚未完成")], { stopReason: "length" }),
    fauxAssistantMessage([fauxToolCall("check_document", { path: "work/draft.md" })], { stopReason: "toolUse" }),
    fauxAssistantMessage([fauxToolCall("submit_document", { path: "work/draft.md", summary: "完成检查并提交" })], { stopReason: "toolUse" }),
  ]);
  let bridge;
  const events = [];
  const result = await runAgentJob({ config, models, model, onBridge: value => { bridge = value; }, emit: event => {
    events.push(event);
    if (event.type === "tool_request") {
      assert.equal(event.name, "check_document");
      assert.deepEqual(event.args, { text: content }, "实际文件内容交给 Rust，而非路径或占位文字");
      queueMicrotask(() => bridge.handle({ type: "tool_result", callId: event.callId, ok: true, result: { hard: [], soft: [] } }));
    }
  }});
  assert.equal(result.snapshot.content, content);
  assert.ok(events.some(event => event.type === "progress" && event.message.includes("继续检查与提交")));
});

test("plain completion is reminded but never silently promoted into a document", async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const { faux, models, model } = fakeBackend();
  const events = [];
  faux.setResponses([
    fauxAssistantMessage([fauxToolCall("write", { path: "work/draft.md", content: "# 候选\n" })], { stopReason: "toolUse" }),
    ...Array.from({ length: 3 }, () => fauxAssistantMessage([fauxText("写好了。")]))
  ]);
  await assert.rejects(() => runAgentJob({ config, models, model, emit: event => events.push(event) }), error => error.code === "not_submitted");
  assert.equal(events.filter(event => event.type === "progress" && event.message.includes("继续检查与提交")).length, 2);
  assert.equal(fs.existsSync(path.join(workspace.delivery, "receipt.json")), false);
  assert.equal(fs.readFileSync(path.join(workspace.root, "work/draft.md"), "utf8"), "# 候选\n");
  const replayEvents = [];
  // 重开数据库不能重新获得补交额度，也不能把上次留下的候选自动当成成品。
  config.recovery = { resume: true };
  await assert.rejects(() => runAgentJob({ config, models, model, emit: event => replayEvents.push(event) }), error => error.code === "not_submitted");
  assert.equal(fs.existsSync(path.join(workspace.delivery, "receipt.json")), false);
  // 用户明确点「接着写」可开新的一批有界补交，继续原文件，无需从头生成。
  config.recovery.continuationId = "manual-resume-1";
  faux.appendResponses([fauxAssistantMessage([fauxToolCall("submit_document", {
    path: "work/draft.md", summary: "继续原稿并提交"
  })], { stopReason: "toolUse" })]);
  const delivered = await runAgentJob({ config, models, model, emit: () => {} });
  assert.equal(delivered.snapshot.content, "# 候选\n");
});

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
  faux.setResponses(Array.from({ length: 3 }, () => fauxAssistantMessage([fauxText("我写好了，见聊天。")])));
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
    fauxAssistantMessage([fauxText("结束")]),
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
    fauxAssistantMessage([fauxText("结束")]),
    fauxAssistantMessage([fauxText("结束")]),
  ]);
  await assert.rejects(
    () => runAgentJob({ config, models, model, emit: () => {} }),
    (error) => error instanceof AgentError && error.code === "not_submitted",
  );
});

test("search, write and submit round-trip through the Rust tool bridge", async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const { faux, models, model } = fakeBackend();
  faux.setResponses([
    fauxAssistantMessage(
      [fauxToolCall("kb_search", { query: "防火 责任" })],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage(
      [
        fauxToolCall("write", {
          path: "work/final.md",
          content: "# 通知\n\n各单位应于每年十月开展一次检查。\n",
        }),
      ],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage(
      [fauxToolCall("submit_document", { path: "work/final.md", summary: "依据知识库" })],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage([fauxText("已提交。")]),
  ]);
  const events = [];
  let bridge;
  const result = await runAgentJob({
    config,
    models,
    model,
    emit: (event) => {
      events.push(event);
      if (event.type === "tool_request") {
        // 模拟 Rust：验证工具名后返回带证据编号的结果（不联网）。
        assert.equal(event.name, "kb_search");
        queueMicrotask(() =>
          bridge.handle({
            type: "tool_result",
            callId: event.callId,
            ok: true,
            result: {
              results: [
                {
                  evidence: "E1",
                  doc_id: 1,
                  title: "防火规定",
                  section: "第三条",
                  text: "各单位应于每年十月开展一次检查。",
                },
              ],
            },
          }),
        );
      }
    },
    onBridge: (value) => {
      bridge = value;
    },
  });
  assert.ok(result.snapshot, "检索后写文件再提交应交付快照");
  assert.match(result.snapshot.content, /每年十月/);
  assert.ok(events.some((event) => event.type === "tool_request" && event.name === "kb_search"));
  assert.ok(events.some((event) => event.type === "tool" && event.name === "kb_search"));
});

test("finish_report returns a sidebar report, never a document snapshot", async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const { faux, models, model } = fakeBackend();
  faux.setResponses([
    fauxAssistantMessage(
      [
        fauxToolCall("finish_report", {
          summary: "审完",
          items: [{ title: "称谓", detail: "第二条", severity: "建议" }],
        }),
      ],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage([fauxText("已交清单。")]),
  ]);
  const events = [];
  const result = await runAgentJob({ config, models, model, emit: (event) => events.push(event) });
  assert.equal(result.snapshot, undefined);
  assert.ok(result.report, "finish_report 应返回报告");
  assert.equal(result.report.items[0].title, "称谓");
  assert.ok(events.some((event) => event.type === "report"));
});

test("a report and a submitted document cannot both be delivered", async () => {
  const workspace = makeWorkspace();
  const config = makeConfig(workspace);
  const { faux, models, model } = fakeBackend();
  fs.writeFileSync(path.join(workspace.root, "work", "final.md"), "# 成品\n");
  faux.setResponses([
    fauxAssistantMessage(
      [
        fauxToolCall("finish_report", {
          summary: "审完",
          items: [{ title: "称谓" }],
        }),
      ],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage(
      [fauxToolCall("submit_document", { path: "work/final.md", summary: "也交正文" })],
      { stopReason: "toolUse" },
    ),
    fauxAssistantMessage([fauxText("结束")]),
  ]);
  const result = await runAgentJob({ config, models, model, emit: () => {} });
  assert.equal(result.snapshot, undefined, "清单已经终止本轮，不能再交正文");
  assert.ok(result.report);
});
