---
name: 自由任务
description: 其他技能都对不上时用：模型自己决定查稿件、查知识库、算日期数字、写工作稿或记下结论来完成你的要求；改了稿就是提案，没改稿就是一份答复清单
hint: 直接说要做什么，例如：把文中所有日期列出来，看看哪些已经过期
triggers: []
when: { text: any }
output: auto
tools: [doc.read, doc.outline, doc.selection, doc.elements, doc.stats, ws.read, ws.write, ws.replace, ws.insert, ws.section, ws.diff, kb.search, kb.read, kb.list, ms.search, ms.read, ms.versions, vocab.units, vocab.normalize, rules.style, rules.lexicon, check.elements, check.proofread, check.facts, check.placeholders, check.references, calc.date, calc.workday, calc.ratio, calc.stats, calc.table, calc.money, calc.number, calc.unit, text.keywords, text.diff, finding.add, note, "http.call:*"]
params:
  max_turns: 12
  max_calls: 24
flow:
  - step: agent
    prompt: 任务
---

# 自由任务

没有专门的技能对得上时，交给自主步骤：模型看要求，自己决定读正文、检索、计算、写工作稿，
程序执行每一次工具调用并记在过程里。改了工作稿的交成提案（照样过事实闸门、由你接受）；
只是回答问题的，交成一份答复清单。

## 任务

当前文种：{kind}。

用户的要求：
{request}

先想清楚要做什么，再用工具去做：
- 要改稿的，先用 ws_read 看工作稿（开始时就是当前正文），用 ws_replace / ws_section / ws_write 修改；
- 只是问问题、查东西的，不要改工作稿，把结论用 finding_add 一条条记下（group 写「答复」或问题类别）；
- 用到知识库或稿件库查到的事实：写进稿子的照工具结果里的 [K编号] 标注；写进结论的出处写文档标题，
  不要写文档 id、知识库编号这类内部编号；
- 要查内网系统里的数据（统计数、名录、办件情况……），用数据接口工具（只查询）；接口多时先用
  api_search 找接口，再用 api_call 调。
全部做完调用 finish。
