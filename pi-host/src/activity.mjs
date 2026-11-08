//! 从 Durable 的执行事件提取侧栏可读的信息，正文与模型输出留在原处。

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

export function observeActivity(task, emit) {
  const materials = new Map((task.materials ?? []).map(({ file, title }) => [file, singleLine(title)]));
  const calls = new Map();
  const report = (activityId, message, outcome) =>
    emit({ type: "tool", name: "activity", activityId, message, outcome });

  return (event) => {
    switch (event.type) {
      case "turn_start":
        report("model", "等待模型返回本轮内容");
        break;
      case "message_start":
        report("model", "模型正在生成本轮内容");
        break;
      case "turn_end":
      case "auto_retry_start":
        report("model", "", "");
        break;
      case "auto_retry_end":
        report("model", "等待模型返回本轮内容");
        break;
      case "tool_execution_start": {
        const file = String(event.args?.path ?? "").replaceAll("\\", "/").replace(/^\.\//u, "");
        const title = materials.get(file);
        const action = {
          read: title ? `读材料 · 《${title}》` : ({
            "input/current.md": "读当前正文",
            "input/request.md": "读本轮要求",
            "input/materials/index.json": "查看材料目录",
          })[file] ?? (file.startsWith("work/") ? "读工作稿" : "读工作区资料"),
          write: "写入工作稿",
          edit: "修改工作稿",
        }[event.toolName];
        if (typeof action !== "string") break;
        const id = `file:${event.toolCallId}`;
        calls.set(id, action);
        report(id, action);
        break;
      }
      case "tool_execution_end": {
        const id = `file:${event.toolCallId}`;
        const action = calls.get(id);
        if (!action) break;
        calls.delete(id);
        const result = event.entry?.model?.[0];
        if (result?.role !== "toolResult") {
          report(id, action, "操作未完成");
        } else if (result.isError) {
          report(id, action, "操作失败");
        } else if (event.toolName === "read") {
          report(id, action, readOutcome(result));
        } else {
          report(id, action, "已保存工作稿");
        }
        break;
      }
    }
  };
}
