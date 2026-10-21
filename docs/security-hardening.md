# 安全加固说明（Rust 侧）

这份文档记录 2026-10 的一次代码级安全加固：审计了仓库里的「不安全写法」，
逐类改成了安全写法，并把结论固化成 lint 门禁与回归测试。

加固的边界：**只动 Rust 侧的内存安全、崩溃面、资源耗尽、注入与文件操作**。
不改业务规则、不改版式、不改数据格式。

## 一、审计结论

| 类别 | 审计前 | 审计后 |
|---|---|---|
| `unsafe` 块 / `unsafe fn` | 16 处，集中在 4 个文件 | 不再有 `unsafe fn`；`unsafe` 块全部收敛在 4 个 FFI 模块（`print::win`（原 `print_pdf`）、`file_clipboard`、`macos_window`、`theme/icons`），每块只包一个调用并带 SAFETY 注释 |
| 用切片首地址相减反推偏移 | 5 处（`as_ptr() as usize` 地址相减） | 0 处 |
| 纯 Rust 模块的 `unsafe` 门禁 | 无 | 全 crate `#![deny(unsafe_code)]`，只有 4 个 FFI 模块在声明处 `#[allow(unsafe_code)]` |
| 用户数据可触发的 panic | 40 余处 | 逐条改成错误返回 / 降级 / 安全兜底 |
| 无上限的文件与压缩包读取 | 5 处 | 统一走 `src/file_limits.rs` 与各处上限 |
| 外部进程调用未隔离参数 | 4 处 | 绝对路径 + `--` + 模式串净化 |
| 动态 SQL 拼接 | 1 处（表名） | 表名白名单 |
| 依赖漏洞扫描 | 无 | CI 里加 `rustsec/audit-check`（每周定时 + 每次推送） |

## 二、分类清单与处置

### A 类：用地址相减反推偏移（脆弱写法）

`str::as_ptr() as usize` 相减去求「子切片在原文里的偏移」。转成整数后再相减
本身不是未定义行为，但它把「子切片必须借自同一个原文」这条约束藏在了类型
系统之外：上游一旦改成返回拷贝、或换了被减的那根基准串，就会悄悄算出错误
偏移，debug 构建下还会因减法溢出直接 panic。改成在解析时就记下偏移：

- `src/export/parse.rs`：新增 `research_report_title_span`，扫描时就把题名的字节
  范围记下来；
- `src/validator.rs`、`src/export/typst/research.rs`：改用该范围；
- `src/highlight.rs`、`src/draft_page/markdown.rs`：子串是原串的后缀，用长度差算
  偏移；
- `src/export/parse.rs`（列表项）：分组时就把内容起点记进 `SourceOrderedLine`，
  行尾有空白时也不会算错。

### B 类：FFI 的 unsafe 契约

- `src/file_clipboard.rs`：`unsafe fn set_data` 改成**安全**函数，分配、写入、
  释放都收在内部；新增 `GlobalHandle` 守卫，任何错误路径都不漏内存；
  `SetClipboardData` 失败时所有权仍在本地，成功才交给系统；新增
  `OpenedClipboard` 守卫，早退也会 `CloseClipboard`，`set_data` 以
  `&OpenedClipboard` 参数在类型上保证「剪贴板已打开」；`DROPFILES` 头改为逐字段
  小端写出，不再把 `repr(C)` 结构按字节重新解释（少一个 `unsafe` 块）；宽字符与
  `DROPEFFECT` 改用显式小端。macOS 一侧的整段 `unsafe` 拆成逐调用的小块。
- `src/print_pdf.rs`（现 `src/print/win/`）：原来一整段 `unsafe { ... }` 包住几十行 Win32 调用，拆成
  若干小块，每块一句 SAFETY；`DC` 与打印对话框交回的全局内存改用 RAII 守卫
  （`DcGuard`、`GlobalMemory`），中途早退也归还；`size_of as u32`、`count as u16`
  之类的截断改成 `try_from`。
- `src/macos_window.rs`：补齐每处 AppKit 调用的 SAFETY 说明（主线程、非空指针、
  生命周期）。
- `src/theme/icons.rs`：SAFETY 注释保留并说明「主线程 + 图像由 `Retained` 持有」。
- 全局门禁：`src/main.rs` 加 `#![deny(unsafe_code)]`、`#![deny(unsafe_op_in_unsafe_fn)]`、
  `#![warn(clippy::missing_safety_doc)]`、`#![warn(clippy::undocumented_unsafe_blocks)]`；
  只有 `file_clipboard`、`macos_window`（`src/main.rs`）、`print::win`（`src/print/mod.rs`）
  与 `theme::icons`（`src/theme.rs`）在 `mod` 声明处 `#[allow(unsafe_code)]`。门禁是「默认禁止、
  逐个放行」，新加的模块不用记得加任何东西就受约束。

### C 类：崩溃面

- `src/export/docx.rs`、`src/agent/tools/mod.rs`：`unreachable!()` 改成错误返回。
- 兜底分支要么在类型上消掉（守卫里直接把要用的值绑出来，如
  `src/export/typst/body.rs` 的紧凑标题、`src/visual_diff/compare.rs` 用
  `Vec::pop_if`、`src/rag.rs` 的重排档位），要么确保兜底后循环仍会前进——
  `while` 循环里在自增之前 `continue` 就是死循环，比 panic 更糟。
- 锁毒化：`src/agent/backend.rs`、`src/agent/api.rs`、`src/rag.rs`、
  `src/visual_diff/tokenize.rs`、`src/theme/palette.rs` 的 `lock().unwrap()` 改成
  `unwrap_or_else(|poisoned| poisoned.into_inner())`——一次 panic 不该让整个
  智能体侧栏与配色系统跟着崩。
- 用户数据可触发的 panic 逐条改掉（部分举例）：
  `src/proofread_rules/context.rs`（引文屏蔽的 UTF-8 回读）、
  `src/document_reference.rs`、`src/export/text.rs`、`src/ime/mod.rs`、
  `src/ime/session.rs`、`src/ime/encoder.rs`、`src/ime/manage.rs`、
  `src/units.rs`、`src/models/config.rs`、`src/manuscript/registry.rs`、
  `src/manuscript_io/sync.rs`、`src/manuscript_io/send_package.rs`、
  `src/preview/render.rs`、`src/preview/research_anchor.rs`、
  `src/export/typst/body.rs`、`src/export/typst/research_body.rs`、
  `src/export/bibliography.rs`、`src/draft_page/{editor,timeline,statistics}.rs`、
  `src/app/{chrome,jobs,session}.rs`、`src/app/manuscript_ui/{detail,list}.rs`、
  `src/app/skill_manager/browser.rs`、`src/app/management_samples.rs`、
  `src/visual_diff/compare.rs`、`src/rag.rs`。

### D 类：资源耗尽

- 新增 `src/file_limits.rs`：统一的「读文件前先卡体积」。文档 256 MB、PDF
  200 MB、字体 64 MB、文本 64 MB；读前查大小、读后再核长度。
- `src/file_limits.rs` 的 `read_bytes` 只打开一次文件：看句柄的大小，再用
  `take(max + 1)` 限量读，文件读到一半变大或报不出大小（管道、设备文件）
  也不会超过上限。
- `src/agent/skill_files/package.rs`：实际字节原本就由 `Package::insert` 按单文件
  16 MB、整个 ZIP 64 MB、512 个文件卡住；这次补上按**声明大小**的提前拒绝
  （128 MB）与条目数上限（4096，目录条目 `insert` 管不到）。
- `src/manuscript_io/sync.rs`、`src/manuscript_io.rs`：图片单张上限与插图入库
  共用 `file_limits::MAX_IMAGE_BYTES`（100 MB，与旧版同步导入接受的大小一致，
  插得进去的图就一定带得回来）；一次导入读进内存的图片合计 1 GB，按**实际
  读到的字节**累加——ZIP 里声明的大小可以造假，只能当作提前拒绝的依据。
  稿件 PDF 附件同样改成限量读，实际超限就跳过。
- `src/images.rs`：插图入库读文件走 `file_limits::read_bytes`。
- `src/net.rs` 新增 `read_body` / `read_error_body`：模型服务、embedding、rerank
  的响应体上限 32 MB，出错时的响应体只读 64 KB 给人看；流式响应单行（一个 SSE
  事件）上限 16 MB，超了明确报错（`src/lmstudio/{stream,converse}.rs`）。数据接口
  原本就有 4 MB 上限（`src/agent/api.rs`）。
- `with_capacity` 的乘法改成饱和运算（`src/manuscript/sync.rs`、
  `src/rag_client.rs`、`src/knowledge.rs`、`src/font_cmap.rs`、
  `src/file_clipboard.rs`）。

### E 类：数值

- `src/export/typst/frame.rs`：单位计数不再 `count() as u32` 截断。
- `src/print_pdf.rs`（现 `src/print/raster.rs`）：`fit_rect` 的浮点转整数显式夹到 `[1, i32::MAX]`、NaN 当 1。
  （Rust 的 `as` 浮点转整数本来就是饱和转换，不是未定义行为；这里是为了极端
  比例下也至少画出一个像素，并让意图写在代码里。）

### F 类：外部进程

- `src/app/widgets.rs`：`open` / `xdg-open` / `explorer` 一律传**绝对路径**
  （`canonicalize`），以 `-` 开头的相对路径不会被当成选项；macOS 与 Linux 的
  调用加 `--`；Windows 的 `explorer /select,` 拒绝含引号的路径（那会拆开命令行）。
- `src/theme/fonts.rs`：`fc-match` 的模式串来自用户填的字体名，去掉控制字符并限长。
- `src/print/cups.rs`：`lp` 本来就有 `--`，保持；打印机名、任务名、页码都作为独立参数
  交给 `lpstat` / `lpoptions` / `lp`，页码先解析成区间再格式化，不原样透传。

### G 类：文件与路径

- `src/export/mod.rs`：`safe_filename` 补上 Windows 保留设备名（CON、PRN、
  NUL、COM1…），带扩展名也算。
- `src/agent/api.rs`：接口密钥 `api-secrets.json` 改成原子写（`storage::atomic_write`），
  Unix 下权限 0600——写一半的密钥文件比写失败更难查。
- `src/storage.rs`：`atomic_write` 提升为 `pub(crate)` 供复用。
- 临时目录不用可预测的名字：单篇 PDF 编译（`src/manuscript_io.rs`）改用
  `tempfile::Builder` 随机、独占创建、仅本用户可访问，析构即删；花脸稿打印预览
  （`src/draft_page/version_diff.rs`）原来是系统临时目录下的固定目录名，改放本用户
  缓存目录（`storage::cache_dir`）。在共享 `/tmp` 的 Linux 多用户机器上，可预测的
  目录能被别的用户抢先建好或换成软链接。
- 递归删除的目标校验已复核：`src/agent/skill_files/package.rs` 有
  `canonicalize` + 前缀校验；其余 `remove_dir_all` 都在临时目录或受控目录内。

### H 类：注入与依赖

- `src/export/typst/injection_tests.rs`（新增）：把「用户正文只当数据」钉成
  测试——正文里的 `#let … read("/etc/passwd")` 原样进 JSON，模板源码里没有它；
  `latex-rust` 解析 `\input` 必然失败（纯排版器，没有文件 IO）。
- `src/manuscript.rs`：`PRAGMA table_info` 的表名进不了参数化查询，改成白名单。
- `src/agent/api.rs`：数据接口地址路径里的变量不许是 `.` / `..` 路径段（值里夹
  `/`、`\` 的按段检查）。`percent_encode` 不转义点号，`..` 会被规范化成上一级，
  请求落到同一主机的别的路径上、还带着本接口的密钥——模型被材料里的提示词诱导
  时，能借此绕开「只调配好的接口」。
- `.github/workflows/ci.yml`：新增 `dependency-audit` 作业（`rustsec/audit-check`），
  每周一与每次推送都跑；作业要 `checks: write` 才能写回结果。
- 依赖：rustls、rustls-webpki、webpki-roots、quinn-proto 小版本升级，修掉 11 条
  公告；zip 8 的 3 条（只影响 `extract` 整包解压）与 quick-xml 0.38 的 2 条（只
  解析内置 CSL 样式）本项目调用不到，登记在 `.cargo/audit.toml` 并写明解除条件。

## 三、约定（改动时照着做）

1. **只在 FFI 模块里写 `unsafe`**：全 crate 默认 `deny(unsafe_code)`；新的 FFI 模块要在
   `mod` 声明处加 `#[allow(unsafe_code)]` 并在本文登记，别的模块不放行。
2. **每个 `unsafe` 块上面写 SAFETY 注释**，说清为什么这么写是安全的
   （clippy 的 `undocumented_unsafe_blocks` 会拦）。
3. **不要写 `unsafe fn` 给上层用**：把 unsafe 收在私有安全函数里，外部拿不到
   裸指针，也就不用把义务传下去。确实要暴露时，必须写 `/// # Safety` 段
   （`missing_safety_doc` 会拦）。
4. **句柄用 RAII 守卫**：Win32 的 `HDC` / `HGLOBAL`、文件句柄都不能靠人记得释放。
5. **读用户交来的文件先卡体积**：走 `crate::file_limits::read_bytes`。
6. **压缩包先卡整包量**：单文件上限之外，还要有整包上限与条目数上限。
7. **不把用户数据拼进命令、SQL 或模板源码**：正文走 JSON 数据通道，SQL 用参数
   （表名用白名单），外部进程用绝对路径加 `--`。
8. **别让用户数据触发 panic**：解析失败要变成错误提示，不是崩掉整个窗口。
   锁要用 `unwrap_or_else(|p| p.into_inner())`。

## 四、还没做的（已知取舍）

- **Typst 模板本身**是编译进二进制的固定文件，用户正文只进 JSON 数据通道；
  模板里仍有 `read(b.src)`（读插图），路径来自稿件的插图引用并做了白名单校验。
  要更严格，可以把 `read` 换成只允许 `images/` 前缀的受限读取器。
- **`anydoc` / `docx-rs` / `emf-core` 等第三方解析器**解析不可信文档时可能
  panic；本项目只能在调用前后兜底（出错提示），不能改它们的内部。
- **`cargo audit` 只在 `ci.yml` 里跑**：发布用的 `release.yml` 不依赖它，新公告
  不会挡住发版，但会让 main 的 CI 变红。`.cargo/audit.toml` 里忽略的公告每条都
  写了解除条件，依赖升级后要回头删。停止维护的依赖（bincode、paste、yaml-rust、
  unsafe-libyaml 等 9 个）只是告警，等依赖升级窗口再换。
- **外部命令按 PATH 查找**（`lp`、`fc-match`、`xdg-open`、`wl-copy` / `xclip`）：
  桌面程序的通行做法，未改成绝对路径。
- **加密 ZIP 的密码**仍以明文存在配置目录（权限 0600）。要更强得靠系统钥匙串，
  涉及跨平台依赖，未在本轮做。
