# vendor/anydoc

文档转 Markdown 库 `anydoc` 的一份副本，带两处补丁，经根 `Cargo.toml` 的
`[patch.crates-io]` 接入。导入代码见 `src/doc_import.rs` 与 `src/doc_import/embedded.rs`，
剪贴板粘贴见 `src/rich_paste/`。

## 来源

- crates.io `anydoc` **0.2.4**
- 上游：<https://github.com/firecrawl/anydoc>，许可证 MIT（见同目录 `LICENSE`）
- 只拷了 `src/`、`LICENSE` 与 `Cargo.toml`（crates.io 规范化后的版本，删掉了指向
  未拷贝的 `examples/convert.rs` 的 `[[example]]` 一节）；本 README 是本仓库写的说明。

## 补丁：公开文档模型的 Markdown 渲染

anydoc 解析 Word / PowerPoint / OpenDocument 等格式时会把内嵌图片的字节留在
`Document::assets`，但 `to_markdown` 渲染时内嵌图片只输出 alt 文本（Word 里通常为空），
图片在导入结果里就悄悄没了。上游公开了 `to_document`（拿到带图片的文档模型），渲染函数
`render::markdown::document_to_markdown` 却是私有的，只能二选一：要图片就得自己重写渲染，
要现成渲染就丢图片。

改法：`src/lib.rs` 新增公开函数 `anydoc::document_to_markdown(&Document) -> String`，
原样转调内部渲染。起草页导入先 `to_document`，把内嵌图片写进图片资源库、改写成指向
`images/...` 的外部图片，再调它渲染。没有图片的文档输出与 `to_markdown` 逐字一致。

改动处：

```text
src/lib.rs   document_to_markdown（新增公开函数；删掉了原来的同名 use）
```

这处补丁适合提给上游（公开渲染函数，或给 `to_markdown` 加一个处理内嵌图片的回调）；
上游合入后可以撤掉这份副本，改回 crates.io 依赖。

## 补丁二：公开 HTML 元素树入口

起草页粘贴时，剪贴板里的 HTML 由 gongwen 自己容错解析成元素树（见
`src/rich_paste/html.rs`），需要交给 anydoc 已有的 HTML 前端走成文档模型，但
`package`、`shared` 都是私有模块，`to_blocks` 只有 EPUB 用得到。

改法：`src/lib.rs` 新增 `pub mod html`，转出 `package::xml` 的 `Node` / `Element` /
`Attr` 三个类型，并新增 `html::to_document(&Element, css) -> Result<Document>`：

- 内部实现一个 `HtmlCtx`：`link_target` 返回 `None`（只留链接文字）、`image_source`
  返回 `Ok(None)`（图片一律不取，也不联网）、`anchor_id` 原样构造；
- 收集到的样式表文本交给 `Stylesheet::add` 后再调 `to_blocks`；
- **不改 `to_blocks` 本身的行为**，EPUB 的输出因此逐字不变。

`package/xml.rs` 里给 `Node`、`Element`、`Attr` 及其 `impl` 加了
`#[allow(missing_docs)]`：这些类型现在是对外 API，上游没写过文档，`#![warn(missing_docs)]`
会在依赖构建里刷一屏警告。

改动处：

```text
src/lib.rs           pub mod html { … }（新增公开入口）
src/package/xml.rs   四处 #[allow(missing_docs)]（Node / Element / Attr / impl Element）
```

这处补丁同样适合提给上游（公开 HTML 前端入口，或者把 `to_blocks` 挂到一个公开的
`html` 模块上）。

## 升级

1. 把新版本的 `anydoc` 源码替换进来（`src/`、`LICENSE`、`Cargo.toml`，保留本 README，
   照旧删掉 `[[example]]` 一节）；
2. 按上面两份清单重新打补丁（渲染函数、HTML 入口；`to_blocks` / `Stylesheet` / `HtmlCtx`
   的签名可能变，先看 `src/shared/html.rs`）；
3. 跑 `cargo test --locked doc_import`：`draft_import_stores_embedded_word_images` 核对
   图片入库，`realistic_import_samples_match_expected_markdown` 核对真实样例输出没变；
4. 跑 `cargo test --locked rich_paste`：粘贴的夹具测试（`tests/fixtures/paste/`）核对
   HTML 前端的输出没变。
