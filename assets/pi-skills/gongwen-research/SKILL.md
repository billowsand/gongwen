---
name: gongwen-research
description: 检索资料、分节论证并整合成研究报告，引用必须有可追溯出处，最后提交完整 Markdown。
---

# 写研究报告

## 先看什么

1. 读 `input/request.md` 确定研究问题、范围、篇幅与交付形态。
2. 读 `skills/gongwen-markdown/references/research-report.md`，按研究报告的
   题名、摘要、章节、图表、公式与文献规则写 Markdown。
3. 先检索：用 `kb_search` / `ms_search` 找本机知识库与稿件库里的材料；
   `kb_read` / `ms_read` 读全文。结果里的证据编号就是可引用的依据。

## 怎么写

- 先列提纲，再逐节写，最后整合；中间文件放 `work/`。
- **每个关键判断都要有出处。** 引用只使用工具实际返回过的资料，不要自己编
  文献名、作者、年份或链接；找不到出处的表述宁可不写。
- 事实材料里的数字、日期、单位名称照原文使用；推算得到的数字要写明算法，
  不要伪装成原文数据。
- 图表用技能文件规定的 Markdown 表格或 Mermaid；公式用 `$…$` / `$$…$$`。
- 引用工具返回的证据编号只用于自查；正文引用格式按技能文件写。

## 交付

- 完整报告写入 `work/` 下的一个 Markdown 文件，调用
  `submit_document({path, summary})`。summary 说明研究问题与主要依据。
- 只做资料调研、不写正文时，可以用 `finish_report` 交一份资料清单到侧栏。
