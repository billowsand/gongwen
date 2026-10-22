# 粘贴样本

从 Word、WPS、网页、Excel 复制出来的内容（剪贴板里的 HTML 副本）放在这里，每个样本
两份文件：

- `<名字>.html`：剪贴板里的 HTML 原文（**先脱敏**：单位、人名、电话、文号都换掉）；
- `<名字>.expected.md`：`rich_paste::convert` 在「空稿、非研究报告」下应当给出的
  Markdown。

`cargo test --locked rich_paste::samples` 会逐个比对；不一致时实际输出写到
`target/paste-samples/<名字>.actual.md`，看一眼就知道差在哪。

## 加一份真实样本

1. 在真机上复制要采样的内容（Word 一段公文、政府网站一页、微信公众号一篇、Excel 一块
   表格）。Windows 的 Word 只给 `StartFragment…EndFragment` 之间的片段，macOS / Linux
   的浏览器给整页，两种都能用。
2. 采下来：

   ```bash
   GONGWEN_PASTE_SAMPLE=word-notice-wps cargo test --locked capture_clipboard_html -- --ignored --nocapture
   ```

   它会读当前剪贴板的 HTML，写到本目录的 `word-notice-wps.html`。

3. 脱敏：把单位、人名、电话、文号、密级换成「某某单位」「张某」这类占位。
4. 生成期望输出，人工核一遍再提交：

   ```bash
   cargo test --locked dump_paste_sample_outputs -- --ignored --nocapture
   ```

   实际输出在 `target/paste-samples/`，核对后复制成同名的 `.expected.md`。

样本只放脱敏或自拟内容。真实公文原文不进仓库。
