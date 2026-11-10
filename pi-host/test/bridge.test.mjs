import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {
  DOMAIN_TOOLS,
  MAX_QUESTION_BATCHES,
  createDomainTools,
  createToolBridge,
} from "../src/bridge.mjs";
import {
  REPORT_TOOL_NAME,
  SUBMIT_TOOL_NAME,
  loadSkillIndex,
  systemPrompt,
  userPrompt,
} from "../src/prompt.mjs";
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
    workspace: { root: "C:\\work", state: "C:\\state\\session", delivery: "C:\\run" },
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

test("writing domain tools exclude the task database", () => {
  const names = DOMAIN_TOOLS.map(([name]) => name);
  for (const name of [
    "kb_search",
    "kb_read",
    "ms_search",
    "ms_read",
    "vocab_search",
    "check_document",
    "doc_elements",
    "style_list",
    "style_read",
  ]) {
    assert.ok(names.includes(name), name);
  }
  const events = [];
  const tools = createDomainTools(createToolBridge({ emit: (event) => events.push(event) }));
  const writingNames = names.filter((name) => name !== "task_search" && name !== "task_read");
  assert.deepEqual(tools.map((tool) => tool.name), [...writingNames, "ask_user"]);
  assert.ok(!names.includes("bash"), "不向模型提供 shell");
  // 风格工具只是读档案，没有写、没有保存、没有第二个模型。
  const styleTools = tools.filter((tool) => ["style_list", "style_read"].includes(tool.name));
  assert.equal(styleTools.length, 2);
  assert.ok(!names.some((name) => /style_(write|save|learn)/.test(name)));
});

test("knowledge and manuscript reads expose character offsets and retain the budgeted output ceiling", async () => {
  const events = [];
  const bridge = createToolBridge({ emit: (event) => events.push(event) });
  const tools = createDomainTools(bridge, { readChars: 240_000 });
  for (const name of ["kb_read", "ms_read"]) {
    const tool = tools.find((item) => item.name === name);
    assert.ok(tool.parameters.properties.offset, `${name} has offset schema`);
    assert.equal(tool.outputLimits.maxBytes > 50 * 1024, true, `${name} output limit follows read budget`);
    assert.equal(tool.outputLimits.retain, "head");
  }
  const pending = tools.find((item) => item.name === "kb_read").execute({ doc_id: 7, offset: 126 }, { callId: "kb" });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(events[0].name, "kb_read");
  assert.deepEqual(events[0].args, { doc_id: 7, offset: 126, limit_chars: 240_000 });
  bridge.handle({ type: "tool_result", callId: events[0].callId, ok: true, result: { text: "下一页", next_offset: null } });
  const result = await pending;
  assert.match(result.output[0].text, /下一页/);
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
  assert.match(text, /style_list \/ style_read/);
  assert.match(text, /不会把档案保存回风格库/);
  assert.match(text, /references\/\.\.\. 位于 skills\/pi-skills\/gongwen-revise\/references\/\.\.\./);
  assert.match(text, /相对引用以该 SKILL\.md 所在目录为基准/);
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

test("auto skill prompt follows the round index instead of a hardcoded five", () => {
  const workspace = { skills: "skills/gongwen-markdown", piSkills: "skills/pi-skills" };
  const index = [
    { name: "custom-a", displayName: "自定义甲", description: "只做甲事" },
    { name: "custom-b", displayName: "自定义乙", description: "只做乙事" },
  ];
  const text = systemPrompt(workspace, { materials: [] }, undefined, index);
  assert.match(text, /custom-a/);
  assert.match(text, /自定义甲/);
  assert.match(text, /只做甲事/);
  assert.doesNotMatch(text, /gongwen-imitate/, "不能再写死五份技能");
  assert.match(text, /本轮技能快照（2 份）/);
  const empty = systemPrompt(workspace, { materials: [] }, undefined, []);
  assert.doesNotMatch(empty, /gongwen-draft/);
  assert.match(empty, /技能索引为空/);
  const selected = systemPrompt(workspace, { skill: "custom-a" }, undefined, index);
  assert.match(selected, /skills\/pi-skills\/custom-a\/SKILL.md/);
});

test("skill index is read from the workspace, not from a global directory", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-pi-skills-"));
  try {
    fs.mkdirSync(path.join(root, "input"), { recursive: true });
    fs.writeFileSync(
      path.join(root, "input", "skills.json"),
      JSON.stringify({
        protocol: 1,
        skills: [
          { name: "mine", displayName: "我的", description: "索引里的说明" },
          { name: "../escape", displayName: "越界", description: "应该丢掉" },
        ],
      }),
    );
    const index = loadSkillIndex({ root, piSkills: "skills/pi-skills" });
    assert.deepEqual(index.map((skill) => skill.name), ["mine"]);
    assert.equal(index[0].description, "索引里的说明");
    // 旧工作区没有 skills.json：只扫描自己的技能目录。
    fs.rmSync(path.join(root, "input", "skills.json"));
    const dir = path.join(root, "skills", "pi-skills", "legacy-skill");
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(
      path.join(dir, "SKILL.md"),
      "---\nname: legacy-skill\ndescription: 旧工作区的技能\n---\n正文\n",
    );
    const fallback = loadSkillIndex({ root, piSkills: "skills/pi-skills" });
    assert.deepEqual(fallback.map((skill) => skill.name), ["legacy-skill"]);
    assert.equal(fallback[0].description, "旧工作区的技能");
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("a larger valid skill index is never silently truncated", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-pi-many-skills-"));
  try {
    fs.mkdirSync(path.join(root, "input"), { recursive: true });
    const skills = Array.from({ length: 65 }, (_, i) => ({ name: `skill-${i}`, description: `说明 ${i}` }));
    const file = path.join(root, "input", "skills.json");
    fs.writeFileSync(file, JSON.stringify({ protocol: 1, skills }));
    const index = loadSkillIndex({ root });
    assert.equal(index.length, 65);
    assert.match(systemPrompt({ root }, {}, undefined, index), /skill-64/);
    fs.writeFileSync(file, "损坏索引");
    assert.throws(() => loadSkillIndex({ root }), /读取本轮技能索引失败/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
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

test("wiki tool allowance is independent of regular tool allowance", async () => {
  const events = [];
  const bridge = createToolBridge({ emit: event => events.push(event), maxCalls: 1 });
  const reply = async (name) => {
    const result = bridge.call(name, {});
    bridge.handle({ type: "tool_result", callId: events.at(-1).callId, ok: true });
    await result;
  };
  await reply("kb_search");
  for (let index = 0; index < 12; index += 1) await reply("wiki_search");
  assert.equal(bridge.used(), 1);
  await assert.rejects(bridge.call("wiki_read", {}), /维基工具.*12/);
  await assert.rejects(bridge.call("kb_search", {}), /工具调用次数超过上限（1）/);
});
