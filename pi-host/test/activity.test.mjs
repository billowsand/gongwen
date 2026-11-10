//! 会话视图 diff（activity.mjs）的单元测试：用合成的 ConversationView / pi.live 快照
//! 覆盖模型状态行、重试提示、文件动作与结果行、槽位结束通知、失败去重与槽位清理。
//! 这些用例不连网络、不启 Durable harness，只喂快照。

import { test } from "node:test";
import assert from "node:assert/strict";
import { observeActivity } from "../src/activity.mjs";

/** 一次 watch 投递的合成视图。 */
function makeView(live, entries = []) {
  return { conversation: {}, entries, docs: live === undefined ? {} : { "pi.live": live } };
}

function makeObserver(task = {}) {
  const events = [];
  const calls = { retry: [], failure: [], settled: [], tick: 0 };
  const api = observeActivity(task, (event) => events.push(event));
  const hooks = {
    onRetry: (attempt) => calls.retry.push(attempt),
    onRunFailure: (failure) => calls.failure.push(failure),
    onToolSettled: (name) => calls.settled.push(name),
    onStreamTick: () => {
      calls.tick += 1;
    },
  };
  const modelLines = () => events.filter((event) => event.activityId === "model").map((event) => event.message);
  const fileRows = () => events.filter((event) => String(event.activityId).startsWith("file:"));
  return { api, events, calls, hooks, modelLines, fileRows };
}

function assistantCall(callId, name, args) {
  return {
    id: `a-${callId}`,
    model: [{ role: "assistant", content: [{ type: "toolCall", id: callId, name, arguments: args }] }],
  };
}

function toolResult(entryId, callId, text, isError = false) {
  return {
    id: entryId,
    model: [{ role: "toolResult", toolCallId: callId, isError, content: [{ type: "text", text }] }],
  };
}

function slot(callId, name, status, extra = {}) {
  return { callId, name, status, ...extra };
}

test("模型状态行按 run / generation 出现与消失，重复状态不重复上报", () => {
  const { api, hooks, modelLines } = makeObserver();
  const waiting = makeView({ run: { taskId: "t1" } });
  const generating = makeView({ run: { taskId: "t1" }, generation: { attempt: 1 } });

  api.update(waiting, hooks);
  assert.deepEqual(modelLines(), ["等待模型返回本轮内容"]);

  api.update(waiting, hooks);
  assert.deepEqual(modelLines(), ["等待模型返回本轮内容"], "同一状态不重复上报");

  api.update(generating, hooks);
  assert.deepEqual(modelLines(), ["等待模型返回本轮内容", "模型正在生成本轮内容"]);

  api.update(makeView({}), hooks);
  assert.deepEqual(modelLines(), ["等待模型返回本轮内容", "模型正在生成本轮内容", ""]);
});

test("流式回答只触发节流刷新，不产生正文事件", () => {
  const { api, hooks, calls, events } = makeObserver();
  api.update(makeView({ run: { taskId: "t" }, generation: { attempt: 1, message: { content: [] } } }), hooks);
  assert.equal(calls.tick, 1, "generation.message 出现即通知宿主刷新候选稿");
  assert.equal(events.filter((event) => event.activityId !== "model").length, 0);
});

test("退避重试只在 attempt 前进时提示一次，新 run 重新计数", () => {
  const { api, hooks, calls } = makeObserver();
  const retried = (attempt) =>
    makeView({ run: { taskId: "t" }, generation: { attempt, retry: { at: 1, error: "连接失败" } } });
  const generating = makeView({ run: { taskId: "t" }, generation: { attempt: 1 } });

  api.update(retried(1), hooks);
  api.update(retried(1), hooks);
  assert.deepEqual(calls.retry, [1], "同一次重试只提示一次");

  api.update(generating, hooks);
  api.update(retried(2), hooks);
  assert.deepEqual(calls.retry, [1, 2], "attempt 前进后再提示");

  api.update(makeView({}), hooks);
  api.update(retried(1), hooks);
  assert.deepEqual(calls.retry, [1, 2, 1], "上一轮结束后重新从 1 计数");
});

test("写工作稿：先报动作行，槽位结束再报结果行，并通知槽位结束", () => {
  const { api, hooks, calls, fileRows } = makeObserver();
  const entries = [assistantCall("c1", "write", { path: "work/final.md" }), toolResult("e1", "c1", "已写入")];
  api.update(makeView({ run: { taskId: "t" }, tools: [slot("c1", "write", "running")] }, entries), hooks);
  assert.deepEqual(fileRows(), [
    { type: "tool", name: "activity", activityId: "file:c1", message: "写入工作稿", outcome: undefined },
  ]);
  assert.deepEqual(calls.settled, [], "槽位未结束不通知");
  assert.equal(api.trackedCalls, 1);

  api.update(makeView({ run: { taskId: "t" }, tools: [slot("c1", "write", "done", { entry: "e1" })] }, entries), hooks);
  assert.equal(fileRows().length, 2, "动作行与结果行各一，动作行不重复");
  assert.equal(fileRows()[1].activityId, "file:c1");
  assert.equal(fileRows()[1].message, "写入工作稿");
  assert.equal(fileRows()[1].outcome, "已保存工作稿");
  assert.deepEqual(calls.settled, ["write"]);
});

test("读材料显示材料标题，读全文与操作失败的结果行区分开", () => {
  const task = { materials: [{ file: "input/materials/00.md", title: "关于开展某项工作的材料" }] };
  const { api, hooks, events, calls } = makeObserver(task);
  const entries = [assistantCall("c1", "read", { path: "input/materials/00.md" }), toolResult("e1", "c1", "【已读第 1–20 行，共 20 行】\n正文")];
  api.update(makeView({ run: { taskId: "t" }, tools: [slot("c1", "read", "running")] }, entries), hooks);
  api.update(makeView({ run: { taskId: "t" }, tools: [slot("c1", "read", "done", { entry: "e1" })] }, entries), hooks);
  const rows = events.filter((event) => event.activityId === "file:c1");
  assert.equal(rows[0].message, "读材料 · 《关于开展某项工作的材料》");
  assert.equal(rows[1].outcome, "已读取全文");
  assert.deepEqual(calls.settled, ["read"]);

  const failed = [assistantCall("c2", "read", { path: "work/x.md" }), toolResult("e2", "c2", "读不出来", true)];
  api.update(makeView({ run: { taskId: "t" }, tools: [slot("c2", "read", "running")] }, failed), hooks);
  api.update(makeView({ run: { taskId: "t" }, tools: [slot("c2", "read", "done", { entry: "e2" })] }, failed), hooks);
  const failedRows = events.filter((event) => event.activityId === "file:c2");
  assert.equal(failedRows[1].outcome, "操作失败");
});

test("非文件工具不产生活动行，但槽位结束同样通知（嵌套调用也通知）", () => {
  const { api, hooks, events, calls } = makeObserver();
  const entries = [assistantCall("c1", "api_search", { query: "预算" }), toolResult("e1", "c1", "找到 1 个接口")];
  api.update(makeView({ run: { taskId: "t" }, tools: [slot("c1", "api_search", "running")] }, entries), hooks);
  api.update(makeView({ run: { taskId: "t" }, tools: [slot("c1", "api_search", "done", { entry: "e1" })] }, entries), hooks);
  assert.equal(events.filter((event) => String(event.activityId).startsWith("file:")).length, 0, "非文件工具没有动作行");
  assert.deepEqual(calls.settled, ["api_search"]);

  // 嵌套调用槽位：不显示动作行，结束时同样要触发候选稿刷新。
  api.update(
    makeView({
      run: { taskId: "t" },
      nestedTools: [{ callId: "c1/k1", name: "read", status: "running", arguments: { path: "work/x.md" } }],
    }, entries),
    hooks,
  );
  assert.deepEqual(calls.settled, ["api_search"], "运行中的嵌套槽位不通知");
  api.update(
    makeView({
      run: { taskId: "t" },
      nestedTools: [{ callId: "c1/k1", name: "read", status: "done", arguments: { path: "work/x.md" } }],
    }, entries),
    hooks,
  );
  assert.deepEqual(calls.settled, ["api_search", "read"]);
  assert.equal(events.filter((event) => String(event.activityId).startsWith("file:")).length, 0);
});

test("首帧就已完成的槽位：动作行与结果行一起报，不重复", () => {
  const { api, hooks, fileRows, calls } = makeObserver();
  const entries = [assistantCall("c1", "edit", { path: "work/final.md" }), toolResult("e1", "c1", "已修改")];
  const view = makeView({ run: { taskId: "t" }, tools: [slot("c1", "edit", "done", { entry: "e1" })] }, entries);
  api.update(view, hooks);
  assert.deepEqual(fileRows(), [
    { type: "tool", name: "activity", activityId: "file:c1", message: "修改工作稿", outcome: "已保存工作稿" },
  ]);
  assert.deepEqual(calls.settled, ["edit"]);
  api.update(view, hooks);
  assert.equal(fileRows().length, 1, "同一槽位不重复上报");
});

test("runFailure 按 taskId+reason+detail 去重，新 run 开始后清零", () => {
  const failure = { taskId: "t1", inputs: [], reason: "restart_limit", detail: "截断 2 次" };
  const { api, hooks, calls } = makeObserver();
  api.update(makeView({ runFailure: failure }), hooks);
  assert.equal(calls.failure.length, 1);
  assert.equal(api.lastRunFailure, failure);

  api.update(makeView({ runFailure: { ...failure } }), hooks);
  assert.equal(calls.failure.length, 1, "同一失败只报告一次");

  api.update(makeView({ run: { taskId: "t2" } }), hooks);
  assert.equal(api.lastRunFailure, undefined, "新 run 开始即清掉陈旧失败");

  api.update(makeView({ runFailure: failure }), hooks);
  assert.equal(calls.failure.length, 2, "清零后同一个失败再次出现仍要报告");
});

test("槽位移出 live.tools / nestedTools 后跟踪表清理", () => {
  const { api, hooks } = makeObserver();
  api.update(makeView({ run: { taskId: "t" }, tools: [slot("c1", "write", "running"), slot("c2", "read", "running")] }), hooks);
  assert.equal(api.trackedCalls, 2);
  api.update(makeView({ run: { taskId: "t" }, tools: [slot("c1", "write", "running")] }), hooks);
  assert.equal(api.trackedCalls, 1, "槽位离开后立即清理");

  api.update(makeView({ run: { taskId: "t" }, nestedTools: [slot("c9/k1", "read", "running")] }), hooks);
  assert.equal(api.trackedCalls, 1, "上一轮槽位表整个消失后只剩嵌套调用");
  api.update(makeView({ run: { taskId: "t" }, tools: [] }), hooks);
  assert.equal(api.trackedCalls, 0, "整轮结束（槽位表清空）后不残留");
});

test("缺字段与畸形快照不抛错，也不产生错误的行", () => {
  const { api, hooks, events } = makeObserver();
  const malformed = [
    undefined,
    null,
    {},
    { docs: null },
    { docs: {} },
    { docs: { "pi.live": null } },
    { docs: { "pi.live": "bad" } },
    { docs: { "pi.live": { tools: "not-an-array", nestedTools: 3 } } },
    { docs: { "pi.live": { tools: [null, {}, { callId: 42, status: "done" }, { callId: "c1" }] } } },
    { docs: { "pi.live": { runFailure: "bad" } } },
    { docs: { "pi.live": { run: {}, generation: { message: null } } } },
    { docs: { "pi.live": { tools: [{ callId: "c2", name: "read", status: "done", entry: {} }] } }, entries: "bad" },
    { docs: { "pi.live": { tools: [{ callId: "c3", name: "write", status: "running" }] } }, entries: [null, { model: 1 }] },
    { docs: { "pi.live": { run: null } }, entries: [{ id: "e", model: [{ role: "assistant", content: null }] }] },
  ];
  for (const view of malformed) assert.doesNotThrow(() => api.update(view, hooks));
  for (const event of events) {
    assert.doesNotMatch(String(event.message), /undefined/u, "畸形字段不能拼出 undefined");
    if (event.outcome !== undefined) assert.doesNotMatch(String(event.outcome), /undefined/u, "畸形字段不能拼出 undefined");
  }
});
