//! 风格档案是独立产物，执行仍使用相同的 Durable Harness 与回执链。
import { createHash } from "node:crypto";
import { Type } from "@earendil-works/pi-ai";
import { defineTool } from "@earendil-works/pi-durable";
import { writeDocumentReceipt } from "./delivery.mjs";

export function isStyleTask(task) {
  return task?.skill === "gongwen-style-learn" && task?.kind === "风格档案";
}

export function validateStyleDraft(text) {
  if (Buffer.byteLength(text, "utf8") > 64 * 1024) throw new Error("风格文件超过64 KiB");
  const value = JSON.parse(text);
  if (!value || Array.isArray(value) || typeof value !== "object") throw new Error("风格文件必须是JSON对象");
  const allowed = ["name", "note", "occasions", "description", "example_numbers"];
  if (Object.keys(value).some(key => !allowed.includes(key))) throw new Error("风格文件含未知字段，不得写身份、来源、统计或启用状态");
  for (const [key,max,required] of [["name",40,true],["note",500,false],["description",12000,true]]) {
    if (!required && value[key] === undefined) continue;
    if (typeof value[key] !== "string" || [...value[key]].length > max || (required && !value[key].trim())) throw new Error(`风格字段 ${key} 为空或超限`);
  }
  if (value.occasions !== undefined && (!Array.isArray(value.occasions) || value.occasions.length > 8 || value.occasions.some(v => typeof v !== "string" || !v.trim() || [...v].length > 60))) throw new Error("适用场合格式不正确");
  if (value.example_numbers !== undefined && (!Array.isArray(value.example_numbers) || value.example_numbers.length > 5 || value.example_numbers.some(v => !Number.isSafeInteger(v) || v < 1 || v > 30) || new Set(value.example_numbers).size !== value.example_numbers.length)) throw new Error("范例编号格式不正确");
  return value;
}

export function createSubmitStyleTool({guard, limits, state, onSubmitted, deliveryDir, runId, requestId, phase, files, validateCoverage}) {
  return defineTool({
    name: "submit_style", description: "提交work/下完整的风格JSON文件，只送设置页编辑保存，绝不写正文。",
    parameters: Type.Object({path:Type.String(),summary:Type.String()}),
    execute: async (args, api) => {
      if (!isStyleTask(phase.task)) throw new Error("本轮不允许提交风格档案");
      if (phase.phased && phase.id !== "deliver") throw new Error("请先完成当前样稿分析并登记，最后阶段才交付风格文件");
      if (phase.phased) validateCoverage(phase, guard);
      if (!state.snapshot) {
        const read = guard.readSubmission(args.path, {maxFileBytes:Math.min(limits.maxFileBytes,64*1024)});
        if (!read.ok) throw new Error(read.error.message);
        if (!read.value.relative.endsWith(".json")) throw new Error("风格档案必须是JSON文件");
        const draft = validateStyleDraft(read.value.text);
        const guide = guard.read("input/style-guide.json", guard.root, 64*1024);
        if (!guide.ok) throw new Error("缺少本轮程序生成的风格学习数据");
        const candidates = JSON.parse(guard.files.readText(guide.value,64*1024)).candidates;
        if (!Array.isArray(candidates) || (draft.example_numbers??[]).some(n=>n>candidates.length)) throw new Error("范例编号不在本轮候选清单中，请重新选择");
        const bytes = Buffer.byteLength(read.value.text,"utf8");
        const sha256 = createHash("sha256").update(read.value.text).digest("hex");
        const receipt = writeDocumentReceipt(deliveryDir,{runId,requestId,relative:read.value.relative,content:read.value.text,sha256,bytes,summary:args.summary,callId:api?.callId??"",kind:"style",files});
        state.snapshot = {path:read.value.relative,content:read.value.text,sha256,bytes,summary:receipt.summary,receipt};
      }
      onSubmitted(state.snapshot);
      return {output:[{type:"text",text:"风格档案已交设置页供用户查看和保存；正文未改变。"}],control:{terminate:true}};
    },
  });
}
