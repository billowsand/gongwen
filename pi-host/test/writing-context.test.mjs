import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { decideStrategy, taskEstimate, chunkChars, readBudgetChars } from "../src/budget.mjs";
import { materialRanges, buildPlan, recoverPlan, phasePrompt } from "../src/phases.mjs";
import { ProgressRegistry, identityOf } from "../src/progress.mjs";
import { WorkspaceGuard } from "../src/paths.mjs";
import { userPrompt } from "../src/prompt.mjs";

const context = { date: { today: "2031年3月16日", timezone: "UTC+08:00", now: "2031年3月16日（星期日）" }, organization: { results: [{ name: "市数据局", duties: "统筹数据共享" }] } };

test("256K及以上按实际容量使用完整材料，不因份数或旧短步骤偏好拆碎", () => {
  const config = { model: { contextWindow: 262144, maxTokens: 16384 }, strategy: { capability: { needsShortSteps: true } }, task: { request: "结合全文起草通知", materials: [1,2,3,4].map(() => ({ bytes: 60000, role: "fact" })), writingContext: context } };
  const decision = decideStrategy(config);
  assert.equal(decision.mode, "regular");
  assert.equal(decision.workBudget, 262144);
  assert.ok(readBudgetChars(decision) > 100000);
  assert.ok(chunkChars(decision) > 100000);
  const larger = decideStrategy({ ...config, model: { ...config.model, contextWindow: 1048576 } });
  assert.equal(larger.workBudget, 1048576);
});

test("整篇不切，超长时按大章节分，范围连续且Unicode不被截断", () => {
  assert.deepEqual(materialRanges("全文材料", 100), [[0,4]]);
  const text = "甲".repeat(70) + "\n## 第二章\n" + "乙".repeat(90) + "😀丙".repeat(30);
  const ranges = materialRanges(text, 100);
  assert.equal(ranges[0][1], 71);
  assert.equal(ranges.map(([a,b]) => text.slice(a,b)).join(""), text);
  assert.ok(ranges.every(([a,b],i) => b > a && b-a <= 100 && (i === 0 || a === ranges[i-1][1])));
});

test("跨日恢复的当前日期与原任务日期并列，组织职责在阶段reset后仍可见", () => {
  const task = { request: "安排下周工作", systemDate: "2031年3月15日", writingContext: context };
  assert.match(userPrompt(task), /2031年3月15日/);
  assert.match(userPrompt(task), /2031年3月16日/);
  const prompt = phasePrompt({ config: { task }, decision: {window:262144,workBudget:262144}, phase: {id:"outline",units:[{kind:"outline",unit:"outline"}]}, plan:{materials:[]}, registry:{active:()=>[],answers:[],evidence:[]} });
  assert.match(prompt, /统筹数据共享/);
  assert.match(prompt, /2031年3月16日/);
});

test("扩大窗口恢复保存原分块：前半材料的登记不能覆盖后半材料", () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-plan-v1-"));
  try {
    for (const dir of ["input/materials", "work", "run"]) fs.mkdirSync(path.join(base,dir),{recursive:true});
    const text = "甲".repeat(3000) + "乙".repeat(3000);
    const file = "input/materials/01.md";
    fs.writeFileSync(path.join(base,file),text);
    fs.writeFileSync(path.join(base,"input/current.md"),"");
    fs.writeFileSync(path.join(base,"input/request.md"),"写通知");
    const guard = new WorkspaceGuard(base);
    const config = { workspace:{current:"input/current.md",request:"input/request.md"}, limits:{maxFileBytes:8388608}, model:{contextWindow:16384,maxTokens:4096}, strategy:{preference:"phased"}, task:{request:"写通知",materials:[{file,role:"fact",bytes:Buffer.byteLength(text)}]} };
    let decision = decideStrategy(config);
    let plan = buildPlan({config,decision,guard});
    const options = {deliveryDir:path.join(base,"run"),runId:"r",requestId:"q",identity:identityOf({guard,workspace:config.workspace,task:config.task})};
    let registry = ProgressRegistry.load(options);
    ({plan,decision}=recoverPlan(plan,decision,registry));
    assert.equal(plan.sections.length,2);
    fs.writeFileSync(path.join(base,"work/note.md"),"前半材料摘记");
    registry.record({spec:plan.sections[0],phase:"distill",unit:plan.sections[0].id,file:"work/note.md",sources:[{path:file,quote:"甲甲"}]},{guard,workspace:config.workspace,task:config.task,maxFileBytes:8388608});
    config.model.contextWindow=262144;
    decision=decideStrategy(config);
    const rederived=buildPlan({config,decision,guard});
    assert.equal(rederived.sections.length,1);
    registry=ProgressRegistry.load(options);
    const restored=recoverPlan(rederived,decision,registry);
    assert.deepEqual(restored.plan.sections.map(s=>[s.start,s.end]),[[0,3000],[3000,6000]]);
    assert.equal(registry.status("distill",restored.plan.sections).ok,false);
    assert.equal(registry.status("distill",restored.plan.sections).missing.length,1);
    assert.throws(()=>recoverPlan(rederived,{...decision,materialsBudget:1},registry),/当前可用窗口小于/);
  } finally { fs.rmSync(base,{recursive:true,force:true}); }
});
