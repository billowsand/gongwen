---
name: gongwen-review
description: 审核现有公文并列出结构化问题；用户要求改稿时再提交修改后的完整正文。
---

# 审核公文

## 先看什么

1. 读 `input/current.md` 与 `input/request.md`，确认审核范围（全文还是选区）。
2. 读 `skills/gongwen-markdown/` 的当前文种规则，作为审核判据。
3. 调用 `check_document` 或在需要时调用 `check_document({text})` 逐段复查；
   词表、文档规则与程序闸门的结论优先于你的主观判断。

## 怎么审

- 按顺序看：文种与结构、标题层级、序号层级、附件说明、公文引用、
  关键事实与日期、称谓与用字、语气与篇幅。
- 每条问题写清位置、问题、依据、建议改法；没有依据的猜测不要列。
- 用户只要清单时，不生成新正文：调用
  `finish_report({items: [{title, detail, severity}]})`，结果只进侧栏。
- 用户明确要求「直接改」时，再按修改技能写出完整修改稿并 `submit_document`。
- 授权类要素只读：可以指出与程序表单不一致，但不能建议或改写它们的值。

## 交付

- 只审不改：`finish_report`，不要提交正文。
- 审并改：`submit_document` 完整 Markdown；summary 说明修了哪些问题。
