//! 从会话视图（`pi.live` + 条目窗口）提取侧栏可读的信息，正文与模型输出留在原处。
//!
//! 1.5.0 起上游移除 `watchEvents()`，改为 `Conversation.watch()` 每次投递一份
//! `ConversationView` 快照（pi.live、条目窗口、内建文档）。这里把连续快照 diff 成
//! 原来的活动行语义：模型状态行、读/写/改工作稿、重试与失败提示。
//! 事件到状态的对应关系见 pi-durable 1.5.0 CHANGELOG 的 watchEvents 迁移表。
//!
//! 视图是外部数据：`docs` / `entries` / `tools` 缺字段、类型不对都不能让这里抛错。
//! watch 监听器一旦抛错，Chord 会以 `listener_error` 结束整条 watch，后续进度就静默丢了。

/** 单个字符串行：去掉控制字符并截断，避免把整段正文带进侧栏。 */
function singleLine(value) {
  return Array.from(String(value ?? "").replace(/[\u0000-\u001f\u007f]/gu, " ").trim()).slice(0, 40).join("");
}

/** 视图字段一律当外部 JSON 处理：只有数组/对象才继续往下取。 */
function asArray(value) {
  return Array.isArray(value) ? value : [];
}

function asObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value : undefined;
}

function readOutcome(result) {
  // 只取有界 read 工具生成的固定范围表头，文件内容留在原处。
  const content = asArray(result?.content);
  const text = typeof content[0]?.text === "string" ? content[0].text : "";
  const lines = text.match(/^【已读第 (\d+)–(\d+) 行，共 (\d+) 行】/u);
  if (lines) {
    const [, start, end, total] = lines;
    return start === "1" && end === total ? "已读取全文" : `已读第 ${start}–${end} 行，共 ${total} 行`;
  }
  const characters = text.match(/^【已读字符 (\d+)–(\d+)，全文 (\d+) 字符】/u);
  if (characters) {
    const [, start, end, total] = characters;
    return start === "0" && end === total ? "已读取全文" : `已读字符 ${start}–${end}，共 ${total} 字符`;
  }
  return "已读取一段资料";
}

/** 模型发出的工具调用参数在助理条目里（槽位不带参数）：按 callId 倒查当前窗口。 */
function findToolCallArguments(view, callId) {
  const entries = asArray(view?.entries);
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    for (const message of asArray(entries[index]?.model)) {
      if (message?.role !== "assistant") continue;
      for (const block of asArray(message.content)) {
        if (block?.type === "toolCall" && block.id === callId) return block.arguments;
      }
    }
  }
  return undefined;
}

/** 工具结果条目：优先按槽位记录的 entry id，兜底按 toolCallId 倒查。 */
function findToolResult(view, slot) {
  const entries = asArray(view?.entries);
  const entryId = slot?.entry;
  if (typeof entryId === "string" && entryId !== "") {
    const entry = entries.find((item) => item?.id === entryId);
    const result = asArray(entry?.model)[0];
    if (result?.role === "toolResult") return result;
  }
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const result = asArray(entries[index]?.model)[0];
    if (result?.role === "toolResult" && result.toolCallId === slot?.callId) return result;
  }
  return undefined;
}

/**
 * 跟踪一份会话视图，产出侧栏活动行。
 * 返回对象的 `update(view, hooks)` 在每次 watch 投递时调用；
 * `lastRunFailure` 记录看到的最近一次 run 失败，供恢复路径判断「连续被中断」。
 *
 * hooks：
 * - onRetry(attempt)：模型调用退避重试（原 auto_retry_start）；
 * - onRunFailure(failure)：一轮没有交出回答（原 task_failed）；
 * - onToolSettled(name)：一个工具槽位结束（原 tool_execution_end）；
 * - onStreamTick()：回答仍在流式生成（原 message_update，只驱动候选稿节流刷新）。
 */
export function observeActivity(task, emit) {
  const materials = new Map(
    asArray(task?.materials)
      .filter((item) => typeof item?.file === "string")
      .map(({ file, title }) => [file, singleLine(title)]),
  );
  // 槽位跟踪表：callId → { action, done }。槽位离开 live.tools / live.nestedTools 后清理，
  // 长会话、多阶段运行不会只增不减；同一个 callId 只报一次动作行、一次结果行。
  const calls = new Map();
  const report = (activityId, message, outcome) =>
    emit({ type: "tool", name: "activity", activityId, message, outcome });

  let modelLine;
  const setModelLine = (message) => {
    if (message === modelLine) return;
    modelLine = message;
    report("model", message, "");
  };
  let retryAttempt = 0;
  let runFailureKey = "";

  const nameOf = (slot) => String(slot?.name ?? "");

  /** 文件类动作行：只认 read / write / edit，其余工具不显示动作行（但仍要报「槽位结束」）。 */
  const actionOf = (view, slot) => {
    const args = asObject(findToolCallArguments(view, slot?.callId)) ?? asObject(slot?.arguments);
    const file = String(args?.path ?? "").replaceAll("\\", "/").replace(/^\.\//u, "");
    const title = materials.get(file);
    const action = {
      read: title ? `读材料 · 《${title}》` : ({
        "input/current.md": "读当前正文",
        "input/request.md": "读本轮要求",
        "input/materials/index.json": "查看材料目录",
      })[file] ?? (file.startsWith("work/") ? "读工作稿" : "读工作区资料"),
      write: "写入工作稿",
      edit: "修改工作稿",
    }[slot?.name];
    return typeof action === "string" ? action : undefined;
  };

  /** 槽位结束的结果行：结果条目缺失按「未完成」处理，读工具取有界读取表头。 */
  const outcomeOf = (view, slot) => {
    const result = findToolResult(view, slot);
    if (!result) return "操作未完成";
    if (result.isError === true) return "操作失败";
    return slot?.name === "read" ? readOutcome(result) : "已保存工作稿";
  };

  /** 模型发起的工具槽位：首次出现报动作行，结束时报结果行并通知宿主。 */
  const trackToolSlot = (view, slot, seen, hooks) => {
    const callId = slot?.callId;
    if (typeof callId !== "string" || callId === "") return;
    seen.add(callId);
    const done = slot.status === "done";
    const known = calls.get(callId);
    if (!known) {
      const action = actionOf(view, slot);
      calls.set(callId, { action, done });
      // 首帧就可能已经是 done（watch 把崩溃前积压的提交折成一帧）：动作行与结果行一起报。
      if (typeof action === "string") report(`file:${callId}`, action, done ? outcomeOf(view, slot) : undefined);
      if (done) hooks.onToolSettled?.(nameOf(slot));
      return;
    }
    if (known.done || !done) return;
    known.done = true;
    if (typeof known.action === "string") report(`file:${callId}`, known.action, outcomeOf(view, slot));
    hooks.onToolSettled?.(nameOf(slot));
  };

  /** 嵌套调用槽位（工具内部再调工具）：不显示动作行，但结束时同样要通知宿主。 */
  const trackNestedSlot = (slot, seen, hooks) => {
    const callId = slot?.callId;
    if (typeof callId !== "string" || callId === "") return;
    seen.add(callId);
    const done = slot.status === "done";
    const known = calls.get(callId);
    if (!known) {
      calls.set(callId, { action: undefined, done });
      if (done) hooks.onToolSettled?.(nameOf(slot));
      return;
    }
    if (known.done || !done) return;
    known.done = true;
    hooks.onToolSettled?.(nameOf(slot));
  };

  const api = {
    lastRunFailure: undefined,
    /** 当前仍在跟踪的工具槽位数（供测试与排查确认槽位已清理）。 */
    get trackedCalls() {
      return calls.size;
    },
    update(view, hooks = {}) {
      const live = asObject(asObject(view?.docs)?.["pi.live"]);
      const generation = asObject(live?.generation);
      const run = live?.run;
      // 模型活动行：本次生成请求在飞（generation 在）→ 正在生成；run 在但
      // 在跑工具/等下一轮 → 等待返回；run 结束 → 清行。
      if (run && generation) setModelLine("模型正在生成本轮内容");
      else if (run) setModelLine("等待模型返回本轮内容");
      else setModelLine("");

      // 不把 token 流拼成正文交给界面；进度只说明还在写。
      if (generation?.message) hooks.onStreamTick?.();

      // 退避重试出现且 attempt 前进时提示一次（attempt 从 1 起，0 表示尚未重试）。
      const attempt = generation?.attempt ?? 0;
      if (run && generation?.retry && attempt > retryAttempt) {
        retryAttempt = attempt;
        hooks.onRetry?.(attempt);
      }
      if (!run) retryAttempt = 0;

      // 工具槽位：新槽位报动作行，结束报结果；每个槽位结束都通知宿主（提交 / 清单工具
      // 由调用方过滤），候选稿刷新不依赖工具是不是文件类。
      const seen = new Set();
      for (const slot of asArray(live?.tools)) trackToolSlot(view, slot, seen, hooks);
      for (const slot of asArray(live?.nestedTools)) trackNestedSlot(slot, seen, hooks);
      for (const callId of calls.keys()) {
        if (!seen.has(callId)) calls.delete(callId);
      }

      // 一轮结束却没有回答：报告一次（按 taskId+reason+detail 去重）。
      const failure = asObject(live?.runFailure);
      if (failure) {
        api.lastRunFailure = failure;
        const key = `${failure.taskId ?? ""}:${failure.reason ?? ""}:${failure.detail ?? ""}`;
        if (key !== runFailureKey) {
          runFailureKey = key;
          hooks.onRunFailure?.(failure);
        }
      } else if (run) {
        // 新 run 的提交会清掉 runFailure：陈旧失败不能留给恢复路径，否则会误判 Parked。
        api.lastRunFailure = undefined;
        runFailureKey = "";
      }
    },
  };
  return api;
}
