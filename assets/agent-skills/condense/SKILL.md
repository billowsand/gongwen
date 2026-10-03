---
name: 精简扩写
description: 按字数要求压缩或展开现有正文（全文或选区），事实不变；改完量一下字数，离要求差得多就再改一次
hint: 说清目标字数，例如：压缩到800字，保留全部任务和时限
triggers: [精简, 压缩, 扩写, 扩充, 展开写, 缩写, 字以内, 字左右, "(压缩|精简|扩写|扩充|缩减|控制)(到|在|成).{0,4}\\d+\\s*字"]
when: { text: present }
output: proposal
tools: [doc.read, doc.selection, doc.stats, llm.generate, ws.write]
flow:
  - step: generate
    mode: rewrite
    prompt: 精简扩写
    fit_length: true
---

# 精简扩写

`generate` 的 `fit_length: true`：从你的话里读出目标字数（「800字」），改完数一下，差 15% 以上就带着
差距再改一次。字数不算空白与 Markdown 标记。

## 精简扩写

{preset}

按下面的要求调整篇幅：只压缩或展开表述，不增删事项，时间、数字、单位、人员、文件依据一律不动，
标题层级保持不变。压缩时先删重复和空话，展开时补充论述和措施的具体做法，不编造事实。

{request}
