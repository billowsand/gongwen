import assert from "node:assert/strict";
import test from "node:test";
import {
  DOMAIN_TOOLS,
  MAX_QUESTION_BATCHES,
  createDomainTools,
  createToolBridge,
} from "../src/bridge.mjs";
import { REPORT_TOOL_NAME, SUBMIT_TOOL_NAME, systemPrompt, userPrompt } from "../src/prompt.mjs";
import { validateInit, PROTOCOL_VERSION } from "../src/protocol.mjs";

function bridgeWith(events) {
  let paused = 0;
  let resumed = 0;
  const bridge = createToolBridge({
    emit: (event) => events.push(event),
    pause: () => {
      paused += 1;
    },
    resume: () => {
      resumed += 1;
    },
  });
  return { bridge, paused: () => paused, resumed: () => resumed };
}

function init(overrides = {}) {
  return validateInit({
    type: "init",
    protocol: PROTOCOL_VERSION,
    requestId: "req-1",
    runId: "run-1",
    workspace: { root: "C:\\work", state: "C:\\state\\session.sqlite", delivery: "C:\\run" },
    model: {
      baseUrl: "http://127.0.0.1:1234/v1",
      apiKey: "sk-test",
      modelId: "fixture",
      contextWindow: 8192,
      maxTokens: 1024,
    },
    task: { request: "写一份通知", kind: "通知", systemDate: "2026-11-01" },
    ...overrides,
  });
}

test("tool calls round-trip by callId and ignore late or wrong replies", async () => {
  const events = [];
  const { bridge } = bridgeWith(events);
  const pending = bridge.call("kb_search", { query: "防火" });
  assert.equal(events[0].type, "tool_request");
  assert.equal(events[0].name, "kb_search");
  assert.equal(events[0].args.query, "防火");
  assert.equal(bridge.handle({ type: "tool_result", callId: "forged", ok: true, result: {} }), false);
  assert.equal(
    bridge.handle({ type: "tool_result", callId: events[0].callId, ok: true, result: { hits: 1 } }),
    true,
  );
  assert.deepEqual(await pending, { hits: 1 });
  // 迟到 / 重复的回复不再有等待者，直接丢弃。
  assert.equal(
    bridge.handle({ type: "tool_result", callId: events[0].callId, ok: true, result: {} }),
    false,
  );
});

test("tool errors reject with the Rust message and never resolve as success", async () => {
  const events = [];
  const { bridge } = createToolBridgeWithEvents(events);
  const pending = bridge.call("check_document", {});
  bridge.handle({
    type: "tool_result",
    callId: events[0].callId,
    ok: false,
    error: "未知工具：bash",
  });
  await assert.rejects(pending, /未知工具：bash/);
  assert.equal(bridge.pending(), 0);
});

function createToolBridgeWithEvents(events) {
  return {
    bridge: createToolBridge({ emit: (event) => events.push(event) }),
  };
}

test("ask_user pauses the timer, resumes on answer, and caps batches and questions", async () => {
  const events = [];
  const { bridge, paused, resumed } = bridgeWith(events);
  const pending = bridge.ask([{ question: "想强调哪方面", options: ["责任", "时限"] }]);
  assert.equal(paused(), 1);
  assert.equal(events[0].type, "questions");
  assert.equal(events[0].batch, 1);
  bridge.handle({
    type: "tool_result",
    callId: events[0].callId,
    ok: true,
    result: { answers: [{ id: 1, value: "责任" }] },
  });
  assert.deepEqual((await pending).answers[0].value, "责任");
  assert.equal(resumed(), 1);

  await assert.rejects(bridge.ask(new Array(4).fill({ question: "q" })), /最多问 3 道/);
  // 第一批已用；第二批可以，第三批被拒。
  const second = bridge.ask([{ question: "还有补充吗" }]);
  bridge.handle({
    type: "tool_result",
    callId: events.at(-1).callId,
    ok: true,
    result: { answers: [] },
  });
  await second;
  await assert.rejects(
    bridge.ask([{ question: "再问一次" }]),
    new RegExp(String(MAX_QUESTION_BATCHES)),
  );
});

test("cancelling rejects every waiter and is not a success", async () => {
  const events = [];
  const { bridge } = createToolBridgeWithEvents(events);
  const first = bridge.call("kb_search", { query: "甲" });
  const second = bridge.call("ms_search", { query: "乙" });
  bridge.abortAll(new Error("本轮已取消"));
  await assert.rejects(first, /已取消/);
  await assert.rejects(second, /已取消/);
  assert.equal(bridge.pending(), 0);
});

test("domain tools include every Rust-backed read-only tool plus doc_elements", () => {
  const names = DOMAIN_TOOLS.map(([name]) => name);
  for (const name of [
    "kb_search",
    "kb_read",
    "ms_search",
    "ms_read",
    "vocab_search",
    "check_document",
    "doc_elements",
  ]) {
    assert.ok(names.includes(name), name);
  }
  const events = [];
  const tools = createDomainTools(createToolBridge({ emit: (event) => events.push(event) }));
  assert.equal(tools.length, names.length + 1, "领域工具 + ask_user");
  assert.ok(!names.includes("bash"), "不向模型提供 shell");
});

test("prompt states material roles, selection protection and the two delivery paths", () => {
  const text = systemPrompt(
    { skills: "skills/gongwen-markdown", piSkills: "skills/pi-skills" },
    { skill: "gongwen-revise", materials: [], selection: { text: "第二段" } },
  );
  assert.match(text, /gongwen-revise/);
  assert.match(text, /submits?|submit_document/);
  assert.match(text, new RegExp(SUBMIT_TOOL_NAME));
  assert.match(text, new RegExp(REPORT_TOOL_NAME));
  assert.match(text, /样稿.*不能当作.*事实来源/s);
  assert.match(text, /doc_elements/);
  assert.doesNotMatch(text, /禁用表格/, "表格写法以技能文件为准");
  const user = userPrompt({
    kind: "通知",
    systemDate: "2026-11-01",
    request: "只改第二段",
    selection: { start: 1, end: 4, text: "第二段要改。" },
    materials: [
      {
        file: "input/materials/01.md",
        title: "来函",
        source: "稿件库",
        role: "incoming_letter",
        sha256: "a".repeat(64),
        bytes: 10,
      },
      {
        file: "input/materials/02.md",
        title: "样稿",
        source: "粘贴材料",
        role: "style_sample",
        sha256: "b".repeat(64),
        bytes: 10,
      },
    ],
  });
  assert.match(user, /incoming_letter/);
  assert.match(user, /style_sample/);
  assert.match(user, /选区以外的正文必须逐字保持原样/);
});

test("init carries skill, selection and material roles without trusting user paths", () => {
  const config = init({
    task: {
      request: "只改选区",
      kind: "通知",
      systemDate: "2026-11-01",
      skill: "gongwen-revise",
      selection: { start: 2, end: 6, text: "要改的" },
      materials: [
        {
          file: "input/materials/01.md",
          title: "来函",
          source: "粘贴",
          role: "incoming_letter",
          sha256: "a".repeat(64),
          bytes: 12,
        },
        {
          file: "../../etc/passwd",
          title: "越界",
          source: "粘贴",
          role: "fact",
        },
      ],
    },
  });
  assert.equal(config.task.skill, "gongwen-revise");
  assert.equal(config.task.selection.text, "要改的");
  assert.equal(config.task.materials.length, 1, "只收程序编号文件路径");
  assert.equal(config.task.materials[0].role, "incoming_letter");
  assert.equal(config.limits.maxQuestionBatches, 2);
  assert.equal(config.workspace.piSkills, "skills/pi-skills");
  assert.equal(config.workspace.materials, "input/materials");
});
