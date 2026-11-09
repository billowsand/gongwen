//! 从会话视图（`pi.live` + 条目窗口）提取侧栏可读的信息，正文与模型输出留在原处。
//!
//! 1.5.0 起上游移除 `watchEvents()`，改为 `Conversation.watch()` 每次投递一份
//! `ConversationView` 快照（pi.live、条目窗口、内建文档）。这里把连续快照 diff 成
//! 原来的活动行语义：模型状态行、读/写/改工作稿、重试与失败提示。
//! 事件到状态的对应关系见 pi-durable 1.5.0 CHANGELOG 的 watchEvents 迁移表。

function singleLine(value) {
  return Array.from(String(value ?? "").replace(/[\u0000-\u001f\u007f]/gu, " ").trim()).slice(0, 40).join("");
}

function readOutcome(result) {
  // 只取有界 read 工具生成的固定范围表头，文件内容留在原处。
  const text = result.content?.[0]?.text ?? "";
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
  for (let index = view.entries.length - 1; index >= 0; index -= 1) {
    for (const message of view.entries[index].model ?? []) {
      if (message.role !== "assistant") continue;
      for (const block of message.content ?? []) {
        if (block.type === "toolCall" && block.id === callId) return block.arguments;
      }
    }
  }
  return undefined;
}

/** 工具结果条目：优先按槽位记录的 entry id，兜底按 toolCallId 倒查。 */
function findToolResult(view, slot) {
  if (slot.entry) {
    const result = view.entries.find((entry) => entry.id === slot.entry)?.model?.[0];
    if (result?.role === "toolResult") return result;
  }
  for (let index = view.entries.length - 1; index >= 0; index -= 1) {
    const result = view.entries[index].model?.[0];
    if (result?.role === "toolResult" && result.toolCallId === slot.callId) return result;
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
  const materials = new Map((task.materials ?? []).map(({ file, title }) => [file, singleLine(title)]));
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

  const api = {
    lastRunFailure: undefined,
    update(view, hooks = {}) {
      const live = view.docs["pi.live"];
      const generation = live?.generation;
      // 模型活动行：本次生成请求在飞（generation 在）→ 正在生成；run 在但
      // 在跑工具/等下一轮 → 等待返回；run 结束 → 清行。
      if (live?.run && generation) setModelLine("模型正在生成本轮内容");
      else if (live?.run) setModelLine("等待模型返回本轮内容");
      else setModelLine("");

      // 不把 token 流拼成正文交给界面；进度只说明还在写。
      if (generation?.message) hooks.onStreamTick?.();

      // 退避重试出现且 attempt 前进时提示一次（attempt 从 1 起，0 表示尚未重试）。
      const attempt = generation?.attempt ?? 0;
      if (live?.run && generation?.retry && attempt > retryAttempt) {
        retryAttempt = attempt;
        hooks.onRetry?.(attempt);
      }
      if (!live?.run) retryAttempt = 0;

      // 工具槽位：新槽位报动作行，槽位结束报结果。
      for (const slot of live?.tools ?? []) {
        const known = calls.get(slot.callId);
        if (!known) {
          const file = String(findToolCallArguments(view, slot.callId)?.path ?? "")
            .replaceAll("\\", "/").replace(/^\.\//u, "");
          const title = materials.get(file);
          const action = {
            read: title ? `读材料 · 《${title}》` : ({
              "input/current.md": "读当前正文",
              "input/request.md": "读本轮要求",
              "input/materials/index.json": "查看材料目录",
            })[file] ?? (file.startsWith("work/") ? "读工作稿" : "读工作区资料"),
            write: "写入工作稿",
            edit: "修改工作稿",
          }[slot.name];
          if (typeof action !== "string") continue;
          calls.set(slot.callId, { action, done: false });
          report(`file:${slot.callId}`, action);
          continue;
        }
        if (known.done || slot.status !== "done") continue;
        known.done = true;
        const result = findToolResult(view, slot);
        let outcome = "操作未完成";
        if (result) {
          outcome = result.isError
            ? "操作失败"
            : slot.name === "read"
              ? readOutcome(result)
              : "已保存工作稿";
        }
        report(`file:${slot.callId}`, known.action, outcome);
        hooks.onToolSettled?.(slot.name);
      }

      // 一轮结束却没有回答：报告一次（按 taskId+reason 去重）。
      const failure = live?.runFailure;
      if (failure) {
        api.lastRunFailure = failure;
        const key = `${failure.taskId}:${failure.reason}:${failure.detail ?? ""}`;
        if (key !== runFailureKey) {
          runFailureKey = key;
          hooks.onRunFailure?.(failure);
        }
      }
    },
  };
  return api;
}
