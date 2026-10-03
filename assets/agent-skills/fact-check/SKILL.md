---
name: 事实核查
description: 逐条核对正文里的时间、数字、文件：检索知识库与数据接口，标出有出处、无出处、与资料矛盾；只出清单，不改稿
hint: 例如：核一下文中的数字和文件依据
triggers: [核查, 核实, 核对, 事实核查, 查证, 有没有出处, 对不对]
when: { text: present }
output: report
tools: [doc.read, kb.search, check.facts, llm.generate]
params:
  max: 15
flow:
  - step: fact_check
    prompt: 事实核对
---

# 事实核查

抽出正文里的时间、数字、文件（同一处只核最长的那个），你提供的材料里有的直接算有出处，其余逐条
检索：证据原文里字面撞上就认，说法不同时交模型判断。要让核查也查内网数据接口，在 `fact_check` 下写
`apis: [接口 id]`，并在 `tools` 里声明 `http.call:接口 id`。

## 事实核对

判断下面的证据与这句话里的「{value}」是什么关系。

【句子】
{sentence}

【证据】
{evidence}

只输出一行：
- 证据明确支持时写「支持 K编号」（例如「支持 K3」）；
- 证据的说法与它不一致时写「矛盾 K编号：证据里的说法」；
- 证据与它无关、证实不了时写「无关」。
