---
name: 规范化
description: 规范标点、数字用法、用语与单位名称：单位、人员名称按标准词库换成规范名称（程序做），其余由模型在事实锁定下统一；词表与规则复扫的结果照常在审校抽屉里
hint: 例如：把全文规范一下，单位用全称，数字用法统一
triggers: [规范化, 规范一下, 统一格式, 标点, 数字用法, 用全称, 规范名称]
when: { text: present }
output: proposal
tools: [doc.read, doc.selection, vocab.normalize, ws.write, llm.generate]
flow:
  - tool: vocab.normalize
    args: { text: "{document}" }
    save_as: normalized
  - tool: ws.write
    args: { text: "{normalized.text}" }
  - step: generate
    mode: rewrite
    source: workspace
    prompt: 规范化
---

# 规范化

先由程序按标准词库把单位、人员的简称与别名换成规范名称（`vocab.normalize`，确定性），再由模型在
事实锁定下统一标点、数字与用语（`source: workspace` 在换好名称的工作稿上改）。

## 规范化

{preset}

在不改变任何事实与观点的前提下，把全文规范一下：

1. 标点用全角中文标点，引号、书名号、括号成对；
2. 数字用法：公历年月日、计量与统计数字用阿拉伯数字，约数、成语、序数词按惯例用汉字；
3. 用语规范：去掉口语和网络用语，不用生造的简称；
4. 单位、人员名称已经换成规范名称，保持不动。

{request}
