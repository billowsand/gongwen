---
name: 润色
description: 在事实锁定下修改现有正文：按要求精简、调整语气、理顺表达，可以只改选区；单位、人员、日期、数字、文件依据的变化必须另行确认
hint: 说说怎么改，例如：压缩第二部分，不改任务、责任单位和时限
triggers: [润色, 修改, 改一下, 改改, 改得, 通顺, 调整, 理顺, 优化, 改写]
when: { text: present }
output: proposal
tools: [doc.read, doc.selection, check.facts, llm.generate, ws.write]
flow:
  - step: generate
    mode: rewrite
    prompt: 润色
---

# 润色

开头的 `flow` 是流程。`generate` 的 `mode: rewrite` 表示在现有正文上改写：程序会把正文、事实锁定
清单和（有选区时）「只许改这一段」的硬约束一并交给模型；结果是提案，用户接受才落入正文。

可以复制到配置目录的 `skills/polish/SKILL.md` 后修改提示词。

## 润色

{preset}

{request}
