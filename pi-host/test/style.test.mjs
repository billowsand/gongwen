import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { createModels, fauxProvider, fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai";
import { runAgentJob } from "../src/agent.mjs";
import { validateInit, PROTOCOL_VERSION } from "../src/protocol.mjs";
import { validateStyleDraft } from "../src/style.mjs";
import { receiptEvents } from "../src/delivery.mjs";

function fixture() {
  const base = fs.mkdtempSync(path.join(os.tmpdir(),"gongwen-style-"));
  const root = path.join(base,"workspace"), run = path.join(base,"run");
  for (const dir of ["input/materials","work","skills/gongwen-markdown","skills/pi-skills/gongwen-style-learn"]) fs.mkdirSync(path.join(root,dir),{recursive:true});
  fs.mkdirSync(run);
  fs.writeFileSync(path.join(root,"input/current.md"),"");
  fs.writeFileSync(path.join(root,"input/request.md"),"学习写法");
  fs.writeFileSync(path.join(root,"input/materials/01.md"),"为推动工作落实，现就有关安排通知如下。各单位明确责任，协同推进，确保各项任务落地见效。");
  fs.writeFileSync(path.join(root,"input/materials/index.json"),"[]");
  fs.writeFileSync(path.join(root,"input/style-guide.json"),JSON.stringify({candidates:[{role:"开头",text:"为推动工作落实",source:"样稿"}]}));
  const config=validateInit({type:"init",protocol:PROTOCOL_VERSION,requestId:"style-input",runId:"style-run",workspace:{root,delivery:run,state:path.join(run,"session")},model:{baseUrl:"http://127.0.0.1:9/v1",modelId:"faux-style",contextWindow:262144,maxTokens:4096},task:{kind:"风格档案",request:"学习风格，不写正文",skill:"gongwen-style-learn",systemDate:"2031-03-16",materials:[{file:"input/materials/01.md",role:"style_sample",bytes:120,title:"样稿",source:"上传样稿"}]}});
  const faux=fauxProvider({provider:"faux-style",models:[{id:"faux-style",contextWindow:262144,maxTokens:4096}]});
  const models=createModels();models.setProvider(faux.provider);
  return {base,root,run,config,faux,models,model:faux.getModel()};
}
const draft={name:"简洁部署",note:"条理明确",occasions:["部署"],description:"开头直陈目的，主体先写目标再按责任分条安排；结尾简短。",example_numbers:[1]};
const call=(name,args)=>fauxAssistantMessage([fauxToolCall(name,args)],{stopReason:"toolUse"});

test("风格JSON拒绝越权字段、空描述与重复编号",()=>{
  assert.equal(validateStyleDraft(JSON.stringify(draft)).name,draft.name);
  assert.throws(()=>validateStyleDraft(JSON.stringify({...draft,enabled:true})),/未知字段/);
  assert.throws(()=>validateStyleDraft(JSON.stringify({...draft,description:""})),/为空/);
  assert.throws(()=>validateStyleDraft(JSON.stringify({...draft,example_numbers:[1,1]})),/范例/);
});

test("同一真实Harness交付风格文件，回执重放不调用模型且没有正文产物",{timeout:15000},async()=>{
  const space=fixture();
  try {
    space.faux.setResponses([
      call("write",{path:"work/style.json",content:JSON.stringify(draft)}),
      call("submit_style",{path:"work/style.json",summary:"总结写法"}),
    ]);
    const result=await runAgentJob({...space,emit:()=>{}});
    assert.equal(result.snapshot.receipt.kind,"style");
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(space.run,"final.json"),"utf8")),draft);
    assert.equal(fs.existsSync(path.join(space.run,"final.md")),false);
    assert.equal(fs.readFileSync(path.join(space.root,"input/current.md"),"utf8"),"");
    assert.equal(fs.existsSync(path.join(space.base,"styles.json")),false);
    space.config.recovery={resume:true};
    space.faux.setResponses([()=>{throw new Error("重放不应调用模型");}]);
    const replay=await runAgentJob({...space,emit:()=>{}});
    assert.equal(replay.receipt.kind,"style");
    const events=receiptEvents(space.run,replay.receipt);
    assert.equal(events[0].content,JSON.stringify(draft));
    assert.equal(events[1].submitted,true);
  } finally {fs.rmSync(space.base,{recursive:true,force:true});}
});

test("候选范例越界反馈给Pi修正，文件工具不能越过work写风格库",{timeout:15000},async()=>{
  const space=fixture();
  try {
    space.faux.setResponses([
      call("write",{path:"../styles.json",content:"越权"}),
      call("write",{path:"work/style.json",content:JSON.stringify({...draft,example_numbers:[2]})}),
      call("submit_style",{path:"work/style.json",summary:"越界测试"}),
      context=>{
        assert.match(JSON.stringify(context),/范例编号不在本轮候选清单/);
        return call("write",{path:"work/style.json",content:JSON.stringify(draft)});
      },
      call("submit_style",{path:"work/style.json",summary:"已改正"}),
    ]);
    const result=await runAgentJob({...space,emit:()=>{}});
    assert.equal(result.snapshot.receipt.kind,"style");
    assert.equal(fs.existsSync(path.join(space.base,"styles.json")),false);
  } finally {fs.rmSync(space.base,{recursive:true,force:true});}
});
