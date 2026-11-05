// 批量工具调用的桥内排队：Rust 只有一个 `pending_tool` 槽位，桥必须按 FIFO
// 一次只发一个，收到匹配回复再发下一个。这里不碰模型，只测协议行为。

import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import {
  MAX_QUESTION_BATCHES,
  MAX_QUESTIONS,
  MAX_TOOL_CALLS,
  createToolBridge,
} from "../src/bridge.mjs";
import { createToolQuota, PHASED_TOOL_LIMIT, REGULAR_TOOL_LIMIT } from "../src/budget.mjs";
import { HOST_VERSION } from "../src/protocol.mjs";

function makeBridge(options = {}) {
  const events = [];
  const counts = { paused: 0, resumed: 0 };
  const bridge = createToolBridge({
    emit: (event) => events.push(event),
    pause: () => {
      counts.paused += 1;
    },
    resume: () => {
      counts.resumed += 1;
    },
    ...options,
  });
  return { bridge, events, counts };
}

/** 回一条工具结果，callId 默认取最近一次发出的请求。 */
function reply(events, ok, result, index = -1) {
  const event = index < 0 ? events.at(-1) : events[index];
  return { type: "tool_result", callId: event.callId, ok, result, error: ok ? "" : String(result) };
}

test("已有积压队列时 emit 失败仍发出下一条，失败请求不冒充成功", async () => {
  const events = [];
  const bridge = createToolBridge({ emit: event => {
    if (event.callId === "c2") throw new Error("第二条发出失败");
    events.push(event);
  } });
  const first = bridge.call("doc_elements", {});
  const second = bridge.call("kb_search", { query: "甲" });
  const rejected = assert.rejects(second, /第二条发出失败/);
  const third = bridge.call("ms_search", { query: "乙" });
  bridge.handle(reply(events, true, { first: true }));
  await first;
  await rejected;
  assert.deepEqual(events.map(event => event.callId), ["c1", "c3"]);
  bridge.handle(reply(events, true, { third: true }));
  assert.deepEqual(await third, { third: true });
  assert.equal(bridge.pending(), 0);
});

test("队首延迟返回后，其余积压请求同步回复也不递归串单", async () => {
  const events = [];
  const bridge = createToolBridge({ emit: event => {
    events.push(event);
    if (event.callId !== "c1") bridge.handle({ type: "tool_result", callId: event.callId, ok: true, result: event.callId });
  } });
  const all = [bridge.call("doc_elements", {}), bridge.call("doc_elements", {}), bridge.call("doc_elements", {})];
  assert.equal(events.length, 1);
  bridge.handle({ type: "tool_result", callId: "c1", ok: true, result: "c1" });
  assert.deepEqual(await Promise.all(all), ["c1", "c2", "c3"]);
  assert.equal(bridge.pending(), 0);
});

test("同批两个请求按 FIFO 逐个发出，第二个等首个结果，各自拿到自己的结果", async () => {
  const { bridge, events } = makeBridge();
  const first = bridge.call("kb_search", { query: "防火" });
  const second = bridge.call("ms_search", { query: "消防" });

  assert.equal(events.length, 1, "只发出一个");
  assert.equal(events[0].name, "kb_search");
  assert.equal(bridge.pending(), 2, "两个都还挂着");
  // 还没发出的第二个请求（排队 ID）不能被伪回复完成，也不能推进队列。
  const queuedId = "c2";
  assert.equal(
    bridge.handle({ type: "tool_result", callId: queuedId, ok: true, result: { forged: true } }),
    false,
  );
  assert.equal(events.length, 1, "伪回复不推进队列");

  assert.equal(bridge.handle(reply(events, true, { hits: ["甲"] })), true);
  assert.equal(events.length, 2, "首个有结果后才发第二个");
  assert.equal(events[1].name, "ms_search");
  assert.deepEqual(await first, { hits: ["甲"] });

  assert.equal(bridge.handle(reply(events, true, { items: 2 })), true);
  assert.deepEqual(await second, { items: 2 });
  assert.equal(bridge.pending(), 0);
  // 重复 / 迟到回复没有等待者。
  assert.equal(bridge.handle({ type: "tool_result", callId: "c2", ok: true, result: {} }), false);
});

test("工具失败只拒绝那一个 Promise，队列继续推进", async () => {
  const { bridge, events } = makeBridge();
  const first = bridge.call("check_document", {});
  const second = bridge.call("doc_elements", {});

  assert.equal(bridge.handle(reply(events, false, "未知工具：bash")), true);
  await assert.rejects(first, /未知工具：bash/);
  assert.equal(events.length, 2, "失败后仍发下一个");
  assert.equal(events[1].name, "doc_elements");

  assert.equal(bridge.handle(reply(events, true, { units: [] })), true);
  assert.deepEqual(await second, { units: [] });
  assert.equal(bridge.pending(), 0);
});

test("emit 抛错只拒绝当前请求，后续排队请求不被卡死", async () => {
  let attempts = 0;
  const bridge = createToolBridge({
    emit: () => {
      attempts += 1;
      throw new Error("写协议失败");
    },
  });
  const first = bridge.call("kb_search", { query: "甲" });
  const second = bridge.ask([{ question: "还要吗" }]);
  await assert.rejects(first, /写协议失败/);
  await assert.rejects(second, /写协议失败/);
  assert.equal(attempts, 2, "第二个提问也尝试发出过");
  assert.equal(bridge.pending(), 0);
});

test("emit 里同步回来的回复不破坏调度，批量请求全部完成", async () => {
  const events = [];
  const bridge = createToolBridge({
    emit: (event) => {
      events.push(event);
      bridge.handle({
        type: "tool_result",
        callId: event.callId,
        ok: true,
        result: { echo: event.callId },
      });
    },
  });
  const results = await Promise.all([
    bridge.call("kb_search", {}),
    bridge.call("kb_read", {}),
    bridge.call("ms_search", {}),
  ]);
  assert.equal(events.length, 3);
  assert.deepEqual(
    results.map((value) => value.echo),
    ["c1", "c2", "c3"],
  );
  assert.equal(bridge.pending(), 0);
});

test("提问排在工具后面：只有真正发出时才暂停计时，回答后恰好恢复一次", async () => {
  const { bridge, events, counts } = makeBridge();
  const tool = bridge.call("kb_search", { query: "甲" });
  const ask = bridge.ask([{ question: "重点写哪方面", options: ["责任", "时限"] }]);
  const tool2 = bridge.call("ms_search", { query: "乙" });

  assert.equal(events.length, 1);
  assert.equal(counts.paused, 0, "排队中的提问不能把工具执行时间算成等用户");
  assert.equal(bridge.pending(), 3);

  bridge.handle(reply(events, true, { hits: [] }));
  await tool;
  assert.equal(events.at(-1).type, "questions");
  assert.equal(counts.paused, 1);
  assert.equal(counts.resumed, 0);

  bridge.handle(reply(events, true, { answers: [{ id: 1, value: "责任" }] }));
  assert.equal(counts.resumed, 1);
  assert.deepEqual((await ask).answers[0].value, "责任");
  assert.equal(events.at(-1).name, "ms_search", "提问结束后轮到后面的工具");
  assert.equal(counts.paused, 1, "后面的工具不暂停计时");

  bridge.handle(reply(events, true, { items: 1 }));
  assert.deepEqual(await tool2, { items: 1 });
  assert.equal(counts.resumed, 1);
  assert.equal(bridge.pending(), 0);
});

test("取消拒绝在途与排队的请求，之后不再发出也不再接纳，晚到回复被忽略", async () => {
  const { bridge, events, counts } = makeBridge();
  const fired = [];
  const active = bridge.call("kb_search", { query: "甲" });
  const queuedTool = bridge.call("ms_search", { query: "乙" });
  const queuedAsk = bridge.ask([{ question: "还要吗" }]);
  // 先把三个 Promise 都挂上拒绝处理器，验证没有未处理的拒绝。
  for (const promise of [active, queuedTool, queuedAsk]) promise.catch((error) => fired.push(error));

  assert.equal(events.length, 1);
  bridge.abortAll(new Error("本轮已取消"));
  assert.equal(bridge.pending(), 0);
  assert.equal(counts.paused, 0, "排队中的提问从未发出");
  assert.equal(counts.resumed, 0, "没暂停过就不恢复");
  await assert.rejects(active, /本轮已取消/);
  await assert.rejects(queuedTool, /本轮已取消/);
  await assert.rejects(queuedAsk, /本轮已取消/);
  assert.equal(fired.length, 3);

  // 晚到回复没有等待者。
  assert.equal(
    bridge.handle({ type: "tool_result", callId: "c1", ok: true, result: { hits: [] } }),
    false,
  );
  assert.equal(events.length, 1);

  // 取消后不再接纳新的工具 / 提问，也不再 emit。
  await assert.rejects(bridge.call("kb_search", { query: "丙" }), /已取消/);
  await assert.rejects(bridge.ask([{ question: "再问" }]), /已取消/);
  assert.equal(events.length, 1);
  assert.equal(bridge.pending(), 0);

  // 重复取消是幂等的，不会二次恢复计时。
  bridge.abortAll(new Error("再取消一次"));
  assert.equal(counts.resumed, 0);
});

test("取消正在等待回答的提问时恰好恢复一次计时", async () => {
  const { bridge, events, counts } = makeBridge();
  const ask = bridge.ask([{ question: "是否继续" }]);
  assert.equal(events[0].type, "questions");
  assert.equal(counts.paused, 1);
  bridge.abortAll(new Error("已停止"));
  assert.equal(counts.resumed, 1);
  await assert.rejects(ask, /已停止/);
  bridge.abortAll(new Error("已停止"));
  assert.equal(counts.resumed, 1);
});

test("排队也计入工具 60 次预算，超出的立刻拒绝而不是无限积累", async () => {
  const { bridge, events } = makeBridge();
  const all = [];
  for (let i = 0; i < MAX_TOOL_CALLS; i += 1) all.push(bridge.call("doc_elements", {}));
  assert.equal(events.length, 1, "其余都在排队");
  assert.equal(bridge.pending(), MAX_TOOL_CALLS);
  await assert.rejects(bridge.call("doc_elements", {}), new RegExp(String(MAX_TOOL_CALLS)));
  assert.equal(bridge.pending(), MAX_TOOL_CALLS, "超预算的请求没有进队列");

  for (let i = 0; i < MAX_TOOL_CALLS; i += 1) {
    assert.equal(bridge.handle(reply(events, true, { index: i }, i)), true);
    assert.deepEqual(await all[i], { index: i });
  }
  assert.equal(events.length, MAX_TOOL_CALLS);
  assert.equal(bridge.pending(), 0);
});

test("提问批次上限对排队的批次同样生效", async () => {
  const { bridge, events } = makeBridge();
  const first = bridge.ask([{ question: "第一题" }]);
  const second = bridge.ask([{ question: "第二题" }]);
  assert.equal(events.length, 1);
  assert.equal(events[0].batch, 1);

  await assert.rejects(
    bridge.ask([{ question: "第三题" }]),
    new RegExp(String(MAX_QUESTION_BATCHES)),
  );
  await assert.rejects(
    bridge.ask(new Array(MAX_QUESTIONS + 1).fill({ question: "太多" })),
    /最多问/,
  );

  bridge.handle(reply(events, true, { answers: [] }));
  await first;
  assert.equal(events.at(-1).batch, 2, "第二批仍带自己的编号");
  bridge.handle(reply(events, true, { answers: [] }));
  await second;
  assert.equal(bridge.pending(), 0);
});

test("首批发出的请求仍是同步立即 emit，公开接口保持兼容", () => {
  const { bridge, events } = makeBridge();
  const pending = bridge.call("kb_search", { query: "甲" });
  assert.equal(events.length, 1);
  assert.equal(typeof events[0].callId, "string");
  assert.equal(events[0].type, "tool_request");
  assert.equal(typeof bridge.call, "function");
  assert.equal(typeof bridge.ask, "function");
  assert.equal(typeof bridge.handle, "function");
  assert.equal(typeof bridge.abortAll, "function");
  assert.equal(typeof bridge.pending, "function");
  assert.equal(bridge.pending(), 1);
  bridge.abortAll(new Error("收尾"));
  pending.catch(() => {});
});

test("常规额度可追加到分阶段硬上限，追加时不重置已用次数", async () => {
  const { bridge, events } = makeBridge();
  const quota = createToolQuota({ mode: "regular", toolBudget: REGULAR_TOOL_LIMIT, bridge });
  const perform = async () => {
    const pending = bridge.call("doc_elements", {});
    const event = events.at(-1);
    assert.equal(event.type, "tool_request");
    bridge.handle({ type: "tool_result", callId: event.callId, ok: true, result: {} });
    await pending;
  };

  assert.equal(bridge.limit(), REGULAR_TOOL_LIMIT);
  for (let i = 0; i < REGULAR_TOOL_LIMIT; i += 1) await perform();
  await assert.rejects(bridge.call("doc_elements", {}), /超过上限（60）/);
  assert.equal(bridge.used(), 60, "被拒绝的第61次不计入已用额度");

  quota.promoteToPhased();
  quota.grant(24);
  assert.equal(bridge.limit(), PHASED_TOOL_LIMIT);
  assert.equal(bridge.used(), 60, "转分阶段不得重置已用调用数");
  for (let i = 60; i < PHASED_TOOL_LIMIT; i += 1) await perform();
  assert.equal(bridge.used(), 84);
  await assert.rejects(bridge.call("doc_elements", {}), /超过上限（84）/);
  assert.equal(bridge.pending(), 0, "第85次不进入队列");
});

test("host and Rust agree on regular and phased hard limits", () => {
  const rust = fs.readFileSync(new URL("../../src/pi_agent/protocol.rs", import.meta.url), "utf8");
  const hostProtocol = fs.readFileSync(new URL("../src/protocol.mjs", import.meta.url), "utf8");
  const packageJson = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8"));
  const packageLock = JSON.parse(fs.readFileSync(new URL("../package-lock.json", import.meta.url), "utf8"));
  const runtime = JSON.parse(fs.readFileSync(new URL("../../packaging/pi-runtime.json", import.meta.url), "utf8"));
  const rustHostVersion = rust.match(/HOST_VERSION: &str = "([^"]+)"/)?.[1];
  assert.ok(rustHostVersion, "Rust host version constant exists");
  assert.match(rust, new RegExp(`MAX_REGULAR_TOOL_CALLS: usize = ${REGULAR_TOOL_LIMIT};`));
  assert.match(rust, new RegExp(`MAX_TOTAL_TOOL_CALLS: usize = ${PHASED_TOOL_LIMIT};`));
  assert.equal(HOST_VERSION, rustHostVersion);
  assert.match(hostProtocol, new RegExp(`HOST_VERSION = "${HOST_VERSION}"`));
  assert.equal(packageJson.version, HOST_VERSION);
  assert.equal(packageLock.version, HOST_VERSION);
  assert.equal(packageLock.packages[""].version, HOST_VERSION);
  assert.equal(runtime.host.version, HOST_VERSION);
  assert.equal(MAX_TOOL_CALLS, REGULAR_TOOL_LIMIT);
});
