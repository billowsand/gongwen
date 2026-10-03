---
name: 语气调整
description: 按行文方向调整措辞：上行文谦恭得体、平行文商洽有礼、下行文明确有力；只改语气，不动事实
hint: 说清改成什么口气，例如：改成向市政府请示的上行文语气
triggers: [语气, 口气, 措辞, 上行文, 下行文, 平行文, 太生硬, 太客气, 太强硬, 委婉]
when: { text: present }
output: proposal
tools: [doc.read, doc.selection, doc.elements, llm.generate, ws.write]
flow:
  - step: generate
    mode: rewrite
    prompt: 语气调整
---

# 语气调整

## 语气调整

{preset}

按下面的要求调整措辞与语气。当前文种：{kind}。

- 上行文：谦恭得体，不用命令口吻。请示的结语用「妥否，请批示」；报告的结语用「特此报告」，报告里不得请求批示；
- 平行文（函）：商洽有礼，用「请予支持为荷」「特此函告」等；
- 下行文（通知、意见）：明确有力，用「要」「务必」「切实」等，要求清楚可执行。

只改语气和措辞，不增删事项，时间、数字、单位、人员、文件依据一律不动。

{request}
