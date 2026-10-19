# 导入结构样例

这些是导入端（`src/doc_import/`）的真实公文样例，覆盖：

| 文件 | 覆盖点 |
|---|---|
| `01-fangxun-notice.docx` | 通知；软回车折行标题；主送；「一、」「（一）」手打编号；带正文的三级标题；干条列表；附件说明 + 两份附件；表格合并单元格；文首密级/文号；文末落款、成文日期、抄送、印发 |
| `02-approval.docx` | 请示；段内「一是……二是……三是……」拆行加粗；文末落款与日期 |
| `03-regulation.docx` | 办法；「第X章/节」标题；「第X条」保留正文并加粗条号 |
| `04-styled-plan.docx` | Word 标题样式；「一、」「（一）」编号与标题层级；List Number 自动列表；段内加粗强调；表格 |
| `05-word-export.txt` | Word 另存的纯文本（一行一段）；版头、版记剥离 |
| `06-wrapped.txt` | 空行分隔的硬折行纯文本；折行接回自然段 |
| `07-negative.docx` | 讲话稿；「一、二季度」「1.5亿元」「2、3月份」「3.2.1」不被误判；短句不被误加粗 |
| `08-fangxun-notice.pdf` | Typst 生成的两页 PDF；文字层抽取；PDF 并段修复（主送、加粗标题、落款、抄送）；中文折行空格清理 |

每个输入文件配一份同名 `.expected.md`，是 `to_draft_markdown` 的完整输出。测试：

```bash
cargo test --locked realistic_import_samples
```

改规则后先看实际输出，再更新期望：

```bash
cargo test --locked dump_import_sample_outputs -- --ignored --nocapture
# 实际输出写到 target/import-samples/*.actual.md，逐份对照后再更新同名 .expected.md
```

重新生成样例（需要 python-docx、typst）：

```bash
python3 tests/fixtures/import/generate_samples.py
typst compile tests/fixtures/import/08-fangxun-notice.typ tests/fixtures/import/08-fangxun-notice.pdf
```
