//! `scripts/pi-writing-eval.mjs` 的离线单测：判据、CLI 与夹具自检，不连模型、不启动 host。
//!
//! 运行：node --test scripts/pi-writing-eval.test.mjs

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {
  deliveredText,
  createFixtureBridge,
  judgeCase,
  loadBridge,
  loadCases,
  parseArgs,
  parseModelConfig,
  prepareWorkspace,
  redactedModel,
  resolveModelConfig,
  selftest,
  validateCases,
} from "./pi-writing-eval.mjs";

const sha256 = (text) => createHash("sha256").update(text, "utf8").digest("hex");

function documentCase(expect = {}) {
  return {
    name: "unit",
    skill: "gongwen-revise",
    kind: "通知",
    expect: { delivery: "document", required: ["2026年12月1日"], forbidden: ["市水务局"], ...expect },
  };
}

const BASE = "# 通知\n\n第一段。\n\n第二段要改。\n\n第三段。\n";
const START = BASE.indexOf("第二段要改。");

function goodResult(overrides = {}) {
  const content = "# 通知\n\n第一段。\n\n第二段已改，2026年12月1日前完成。\n\n第三段。\n";
  return {
    exitCode: 0,
    timedOut: false,
    events: [{ type: "submitted" }, { type: "completed", submitted: true }],
    submitted: {
      path: "work/final.md",
      summary: "改好了",
      content,
      sha256: sha256(content),
      bytes: Buffer.byteLength(content, "utf8"),
    },
    report: null,
    receipt: { kind: "document", sha256: sha256(content), bytes: Buffer.byteLength(content, "utf8") },
    receiptContent: content,
    base: BASE,
    selection: { start: START, end: START + "第二段要改。".length, text: "第二段要改。" },
    toolCalls: [{ name: "check_document", ok: true }],
    questions: [],
    ...overrides,
  };
}

test("fixtures validate and every case declares its own criteria", () => {
  const fixtures = loadCases();
  assert.equal(validateCases(fixtures), true);
  assert.ok(fixtures.fixtures.includes("夹具"));
  const names = fixtures.cases.map((item) => item.name);
  assert.equal(new Set(names).size, names.length);
  for (const item of fixtures.cases) {
    assert.ok(item.expect.delivery === "document" || item.expect.delivery === "report", item.name);
    assert.ok(Array.isArray(item.expect.manual) && item.expect.manual.length > 0, `${item.name} 没有人工判断项`);
    assert.ok(fs.existsSync(path.join("assets", "pi-skills", item.skill, "SKILL.md")), item.name);
  }
  // 需求点名的场景都在。
  for (const required of [
    "draft-from-material",
    "condense-selection",
    "reply-unknown-stance",
    "material-conflict",
    "policy-source-missing",
    "extract-only",
    "research-sources",
    "style-apply",
  ]) {
    assert.ok(names.includes(required), required);
  }
});

test("evaluation workspaces carry the same material index as production, including empty materials", () => {
  const fixtures = loadCases();
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pi-eval-index-"));
  try {
    for (const name of ["material-conflict", "research-sources"]) {
      const prepared = prepareWorkspace({caseDef:fixtures.cases.find(item=>item.name===name),fixtures,bridge:{},root:path.join(root,name)});
      const index = JSON.parse(fs.readFileSync(path.join(prepared.workspace,"input/materials/index.json"),"utf8"));
      assert.deepEqual(index,prepared.materials);
      for (const entry of index) {
        const bytes=fs.readFileSync(path.join(prepared.workspace,entry.file));
        assert.equal(bytes.length,entry.bytes);
        assert.equal(createHash("sha256").update(bytes).digest("hex"),entry.sha256);
      }
    }
  } finally { fs.rmSync(root,{recursive:true,force:true}); }
});

test("bridge fixtures use the production tool shapes and are marked as fixtures", () => {
  const bridge = loadBridge();
  for (const doc of bridge.knowledge) {
    assert.ok(Number.isInteger(doc.docId) && doc.title && doc.text, "kb 条目");
  }
  for (const doc of bridge.manuscripts) {
    assert.ok(Number.isInteger(doc.id) && doc.title && typeof doc.markdown === "string", "ms 条目");
  }
  for (const style of bridge.styles) {
    assert.ok(typeof style.id === "string" && style.id !== "", "style id");
    assert.ok(Array.isArray(style.kinds), "style kinds");
  }
  const policy = loadCases().cases.find((item) => item.name === "policy-source-missing");
  assert.equal(policy.expect.delivery, "report");
  assert.equal(policy.expect.reportOnly, true);
});

test("a clean delivery passes both technical and business criteria", () => {
  const judge = judgeCase(documentCase({ unchangedOutsideSelection: true, maxSelectionChars: 40 }), goodResult());
  assert.equal(judge.technicalPassed, true, JSON.stringify(judge.technical));
  assert.equal(judge.businessPassed, true, JSON.stringify(judge.business));
  assert.equal(judge.keywordCoverage.note.includes("不是写作质量总分"), true);
  assert.deepEqual(judge.manual, []);
});

test("fact checks exclude samples and only admit actually returned sources or answers", () => {
  const box = createFixtureBridge({
    caseDef: { kind: "通知", materials: [{role:"fact"},{role:"style_sample"}], answers:{"未知":"2027年4月1日"} },
    fixtures: {}, workspace: process.cwd(), base: "原稿", request: "检查",
    materialTexts: ["事实2026年12月1日", "样稿2025年3月15日"],
    bridge: {
      knowledge: [{docId:1,title:"可检索资料",text:"2028年1月1日",keywords:["可检索"]}],
      styles: [{id:"style",name:"风格",description:"2030年2月1日",enabled:true,examples:[]}],
    },
  });
  assert.ok(box.corpusText().includes("2026年12月1日"));
  for (const value of ["2025年3月15日","2027年4月1日","2028年1月1日","2030年2月1日"]) {
    assert.equal(box.corpusText().includes(value),false,value);
  }
  box.handle("style_read",{id:"style"});
  assert.equal(box.corpusText().includes("2030年2月1日"),false);
  box.handle("kb_read",{doc_id:1});
  assert.ok(box.corpusText().includes("2028年1月1日"));
  const first = box.handle("kb_search",{query:"可检索"}).result.results[0].evidence;
  const second = box.handle("kb_search",{query:"可检索"}).result.results[0].evidence;
  assert.notEqual(first,second,"再次检索不能重复分配证据编号");
  box.recordAnswers([{value:"2027年4月1日"}]);
  assert.ok(box.corpusText().includes("2027年4月1日"));
});

test("unchanged outside text cannot satisfy facts required inside the selected passage", () => {
  const result = goodResult();
  const judge = judgeCase(documentCase({ unchangedOutsideSelection:true, required:["第一段。"] }),result);
  assert.equal(judge.businessPassed,false,"选区外的原句不能掩盖选区内事实缺失");
});

test("missing facts, lifted sample facts and selection violations each fail separately", () => {
  const missingFact = judgeCase(
    documentCase(),
    goodResult({ submitted: { ...goodResult().submitted, content: "# 通知\n\n第一段。\n" } }),
  );
  assert.equal(missingFact.technicalPassed, false, "正文被掏空时技术判据也应该发现");
  assert.equal(missingFact.businessPassed, false);

  const lifted = judgeCase(
    documentCase(),
    goodResult({ submitted: { ...goodResult().submitted, content: "# 通知\n\n市水务局。2026年12月1日。\n" } }),
  );
  assert.equal(lifted.businessPassed, false);
  assert.ok(lifted.business.some((item) => item.name.includes("市水务局") && !item.ok));

  const outside = judgeCase(
    documentCase({ unchangedOutsideSelection: true }),
    goodResult({
      submitted: {
        ...goodResult().submitted,
        content: "# 通知\n\n第一段。\n\n第二段已改，2026年12月1日前完成。\n\n第三段被改。\n",
      },
    }),
  );
  assert.equal(outside.technicalPassed, false);
  assert.ok(outside.technical.some((item) => item.name === "选区外逐字未变" && !item.ok));
});

test("the judge distinguishes a report-only task from a document task", () => {
  const documentTaskButReport = judgeCase(
    documentCase(),
    goodResult({ submitted: null, report: { summary: "只看看", items: [{ title: "问题" }] }, receipt: undefined, receiptContent: "" }),
  );
  assert.equal(documentTaskButReport.technicalPassed, false);

  const reportCase = {
    name: "report-unit",
    expect: { delivery: "report", requirePhrasesAny: [["未找到原文", "未找到"]] },
  };
  const ok = judgeCase(
    reportCase,
    goodResult({
      submitted: null,
      receipt: undefined,
      receiptContent: "",
      events: [{ type: "report" }, { type: "completed", submitted: false }],
      report: { summary: "未找到原文，需人工核对", items: [{ title: "《××办法》", detail: "未找到原文" }] },
    }),
  );
  assert.equal(ok.technicalPassed, true, JSON.stringify(ok.technical));
  assert.equal(ok.businessPassed, true, JSON.stringify(ok.business));

  const falselyVerified = judgeCase(
    reportCase,
    goodResult({
      submitted: null,
      receipt: undefined,
      receiptContent: "",
      events: [{ type: "report" }, { type: "completed", submitted: false }],
      report: { summary: "已核实，没有问题", items: [{ title: "《××办法》", detail: "一致" }] },
    }),
  );
  assert.equal(falselyVerified.businessPassed, false, "找不到原文却说已核实应判失败");

  const documentForReportTask = judgeCase(reportCase, goodResult());
  assert.equal(documentForReportTask.technicalPassed, false);
});

test("failed tools and an abnormal exit are technical failures, not business ones", () => {
  const failed = judgeCase(documentCase(), goodResult({ toolCalls: [{ name: "kb_search", ok: false, error: "夹具没有" }] }));
  assert.equal(failed.technicalPassed, false);
  assert.ok(failed.technical.some((item) => item.name === "领域工具调用没有失败" && !item.ok));
  const crashed = judgeCase(documentCase(), goodResult({ exitCode: 1 }));
  assert.equal(crashed.technicalPassed, false);
  const errored = judgeCase(
    documentCase(),
    goodResult({ events: [...goodResult().events, { type: "error", message: "模型失败" }] }),
  );
  assert.equal(errored.technicalPassed, false);
});

test("deliveredText prefers the submitted document and never invents content", () => {
  assert.equal(deliveredText(goodResult()).includes("第二段已改"), true);
  assert.equal(
    deliveredText({ submitted: null, report: { summary: "概述", items: [{ title: "甲", detail: "乙" }] } }),
    "概述\n甲\n乙",
  );
  assert.equal(deliveredText({ submitted: null, report: null }), "");
});

test("research footnotes reject standalone references and standard Markdown footer definitions", () => {
  const caseDef = documentCase({required:[],forbidden:[],inlineFootnotesOnly:true});
  const result=goodResult();
  result.submitted.content="数据来自材料[^source-a]:(《资料标题》记载原文。)。";
  assert.equal(judgeCase(caseDef,result).businessPassed,true);
  result.submitted.content="数据来自材料[^source-a]。\n\n[^source-a]: 《资料标题》";
  assert.equal(judgeCase(caseDef,result).businessPassed,false);
});

test("recipient suggestions belong to the delivery summary while body headers remain forbidden", () => {
  const caseDef = documentCase({ requiredSummary: ["主送建议：市数据局"], forbidden: ["市数据局：", "市财政局："] });
  const valid = goodResult();
  valid.submitted.summary = "主送建议：市数据局；依据：来函落款";
  assert.equal(judgeCase(caseDef, valid).businessPassed, true);
  valid.submitted.summary = "已写好";
  assert.equal(judgeCase(caseDef, valid).businessPassed, false, "正文正常也不能漏掉收发关系判据");
  valid.submitted.summary = "主送建议：市数据局；依据：来函落款";
  valid.submitted.content += "\n市财政局：\n";
  assert.equal(judgeCase(caseDef, valid).businessPassed, false, "交付说明正确不能掩盖正文写错抬头");
});

test("CLI parsing is strict and the default has no model configuration", () => {
  const parsed = parseArgs(["--case", "a", "--case", "b", "--timeout", "30", "--out", "tmp/x"]);
  assert.deepEqual(parsed.cases, ["a", "b"]);
  assert.equal(parsed.timeoutSeconds, 30);
  assert.equal(parsed.out, "tmp/x");
  assert.equal(parseArgs(["--case-delay", "65"]).caseDelaySeconds, 65);
  assert.throws(() => parseArgs(["--case-delay", "601"]), /0–600/);
  assert.throws(() => parseArgs(["--case-delay", "NaN"]), /0–600/);
  assert.throws(() => parseArgs(["--unknown"]), /不认识的参数/);
  assert.throws(() => parseArgs(["--case"]), /缺少参数/);
  assert.throws(() => parseArgs(["--timeout", "0"]), /正数/);
  assert.throws(() => resolveModelConfig({ env: {} }), /没有模型配置/);
});

test("model config keeps the key out of the report and refuses auth files", () => {
  const config = parseModelConfig({
    baseUrl: "https://eval.invalid/v1?secret=1",
    apiKey: "sk-unit-secret-1234",
    modelId: "unit-model",
    contextWindow: 32000,
    maxTokens: 4096,
    reasoning: true,
    thinkingLevel: "high",
  });
  assert.equal(config.apiKey, "sk-unit-secret-1234");
  const redacted = redactedModel(config);
  const json = JSON.stringify(redacted);
  assert.equal(json.includes("sk-unit-secret-1234"), false);
  assert.equal(json.includes("?secret=1"), false);
  assert.equal(redacted.endpointHost, "eval.invalid");
  assert.equal(redacted.modelId, "unit-model");

  assert.throws(() => parseModelConfig({ modelId: "m" }), /baseUrl/);
  assert.throws(() => parseModelConfig({ baseUrl: "https://x.invalid/v1", modelId: "m", maxTokens: 999999 }), /maxTokens/);

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-pi-eval-test-"));
  try {
    const good = path.join(dir, "model.json");
    fs.writeFileSync(good, JSON.stringify({ baseUrl: "http://127.0.0.1:1/v1", modelId: "m" }));
    assert.equal(resolveModelConfig({ file: good, env: {} }).modelId, "m");
    const envFile = path.join(dir, ".env");
    fs.writeFileSync(envFile, "GONGWEN=1");
    assert.throws(() => resolveModelConfig({ file: envFile, env: {} }), /环境凭据文件/);
    assert.throws(
      () => resolveModelConfig({ file: undefined, env: { GONGWEN_PI_EVAL_MODEL_CONFIG: envFile } }),
      /环境凭据文件/,
    );
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("the script's own selftest passes without a model", () => {
  assert.equal(selftest(), true);
});
