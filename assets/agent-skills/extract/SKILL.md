---
name: 要点提炼
description: 对选区或全文提炼要点，列成清单；不改稿
hint: 例如：提炼这段讲话的要点，五条以内
triggers: [提炼, 要点, 概括, 归纳, 总结一下, 梳理一下]
when: { text: present }
output: report
tools: [doc.read, doc.selection, llm.generate]
flow:
  - step: plan
    mode: list
    prompt: 提炼
    save_as: points
    confirm: false
    max: 10
  - step: report
    from: points
    group: 要点
---

# 要点提炼

## 提炼

从下面的文字里提炼要点，最多 {max} 条，每行一条，每条一句话，保留原文里的时间、数字、单位，不加评论。
有选区时只提炼选区，没有选区时提炼全文。

{request}

【选区】
{selection|（没有选区）}

【全文】
{document}
