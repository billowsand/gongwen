# 应用内打印对话框设计

> 状态：三期均已实现（2026-10-21），**待真机验收**。
> 进度、与方案的出入、已知坑见文末第十一节。

## 一、现状与问题

打印入口只有一个：PDF 查看器工具栏的「打印」（`src/pdf_viewer.rs`），动作经
`apply_pdf_actions`（`src/app/tabs.rs`）丢到后台线程跑 `print_pdf::print_pdf`，结果
经 `WorkerResult::PdfPrinted` 回状态栏。底层分两条路：

| 平台 | 现在的做法 | 问题 |
|---|---|---|
| Windows | `PrintDlgW` 老式系统对话框 → hayro 逐页光栅化 → GDI `StretchDIBits` | 对话框是 Win95 风格，双面、颜色、纸盒都藏在驱动「首选项」里；见下方两个版式缺陷 |
| macOS / Linux（含麒麟） | 直接 `lp -- 文件` | **完全没有对话框**，直冲默认打印机：选不了打印机、份数、页码、双面 |

Windows 那条路还有两个影响公文版式的缺陷：

1. **缩印**。`fit_rect` 把页面塞进 `HORZRES × VERTRES`（可打印区），可打印区要扣掉
   硬件边距（常见每边 4 mm 左右），整页按约 96% 缩小。GB/T 9704 的版心 156 × 225 mm
   打出来约 150 mm 宽，量尺子就能发现。
2. **横页按横的缩进竖纸**。Typst 出的横向附件、整份横排是「竖向纸张 + 内容转 90° +
   `/Rotate 90`」（见 `docs/typst-engine.md`「横向附件」），`render_dimensions()` 已经把
   `/Rotate` 算进去，得到横的尺寸，再 `fit_rect` 进竖纸，整页缩到约 70%。打印应该按
   **纸张方向**出图（不应用 `/Rotate`），这正是 pdflscape 结构想要的效果。

## 二、目标与不做的事

**目标**：点「打印」弹出应用自己的打印对话框，三平台一致：

- 选打印机（带默认标记与状态：就绪 / 脱机 / 暂停 / 出错）；
- 份数、逐份打印；页码范围（全部 / 当前页 / 自定义 `1-3,5`）；
- 双面（单面 / 长边翻转 / 短边翻转 / **手动双面**）；
- 颜色（彩色 / 黑白）；纸张与纸盒；缩放（**实际大小**默认 / 适合纸张）；
- 公文场景的默认值与提醒（见第五节）；
- 记住上次用的打印机和各打印机的偏好；
- 保留「系统对话框…」逃生口，驱动独有的高级功能（装订、打孔）走它。

**不做**：

- 不做打印队列管理（暂停、删除别人的任务）；只回报本次任务是否送达。
- 不做「套红打印」（红头纸上只打正文、不打版头）。这要重新排版，不是打印选项，
  另立方案。
- 不引入 GTK / Qt 打印对话框或任何外部打印库：Windows 用 `windows-sys` 的
  Win32 打印 API，Unix 只调 CUPS 命令行（`lpstat`、`lpoptions`、`lp`）。

## 三、界面

用 `egui::Modal` 做成应用内模态框，不开独立的系统窗口（第七节有取舍）。

```
┌ 打印：关于××工作的请示.pdf ─────────────────────────────────────┐
│ ┌──────────────┐  打印机  [ HP LaserJet M403（默认）     ▾ ] ● 就绪 │
│ │              │          [ 打印机首选项… ]（仅 Windows）            │
│ │   页面预览    │  份数    [ 2 ]   ☑ 逐份打印                        │
│ │  （纸张轮廓   │  页码    ◉ 全部（12 页）  ○ 当前页（3）            │
│ │   + 内容）    │          ○ 自定义 [ 1-3,5        ]                 │
│ │              │  双面    [ 长边翻转 ▾ ]                             │
│ │  ◀  3/12  ▶  │  颜色    [ 黑白 ▾ ]                                 │
│ └──────────────┘  纸张    A4    纸盒 [ 自动选择 ▾ ]                  │
│                   缩放    ◉ 实际大小（公文推荐）  ○ 适合纸张          │
│ ⚠ 版头为红色，当前选了黑白，将以灰色打出。                            │
│ 共 12 页 → 双面 6 张纸 × 2 份 = 12 张                                │
│ [ 使用系统对话框… ]                              [ 取消 ]  [ 打印 ]  │
└────────────────────────────────────────────────────────────────┘
```

- 打印机列表异步加载：先显示「正在查找打印机…」，加载完默认选中「上次用的 → 系统默认」。
  打印机不支持的选项（无双面单元、单色机）对应控件置灰并在悬停里说明，不藏起来。
- 自定义页码实时校验，非法输入就地标红、「打印」置灰；页码超过总页数提示截断。
- 底部一行算纸张数，是机关里最常被问的「要几张纸」。
- 「打印」按下后对话框变成进度态（「正在送第 3/12 页」+ 取消），送完自动关闭，
  结果仍回状态栏。
- 快捷键：Ctrl/Cmd+P 在 PDF 标签里打开对话框；Enter 打印、Esc 取消。

**手动双面**（没有双面单元的打印机，机关里很常见，本机的 HP DeskJet 4900 就是）：

1. 先打奇数页（按「出纸面朝下 / 朝上」决定是否倒序，偏好按打印机记住）；
2. 对话框停在「请把打出的纸整叠翻面放回纸盒，然后点『继续打印偶数页』」；
3. 再打偶数页；总页数为奇数时偶数轮末尾补一张空白，保证正反面对齐。

## 四、选项与三平台映射

| 选项 | Windows（GDI + DEVMODE） | macOS / Linux（CUPS `lp`） |
|---|---|---|
| 打印机列表 | `EnumPrintersW`（LOCAL \| CONNECTIONS，level 4 先出名字）+ `GetDefaultPrinterW` | `lpstat -e`、`lpstat -d`（只取冒号后的已知名字） |
| 状态 | `GetPrinterW` level 6 的 `dwStatus` + level 4 的「脱机使用」属性（逐台、后台） | `lpoptions -p 名字` 的 `printer-state` 与 `printer-state-reasons`（逐台、后台） |
| 能力 | `DeviceCapabilitiesW`：`DC_DUPLEX`、`DC_COLORDEVICE`、`DC_PAPERS`+`DC_PAPERNAMES`、`DC_BINS`+`DC_BINNAMES`、`DC_COLLATE` | `lpoptions -p 名字 -l`：`Duplex`、`ColorModel`、`PageSize`、`InputSlot`、`Collate` |
| 指定打印机 | `CreateDCW("WINSPOOL", 名字, null, devmode)` | `-d 名字` |
| 份数 / 逐份 | `dmCopies`、`dmCollate`；驱动不支持时程序逐份重复（现有逻辑） | `-n N -o collate=true` |
| 页码 | 程序只渲染选中页 | `-P 1-3,5` |
| 双面 | `dmDuplex` = `DMDUP_SIMPLEX` / `DMDUP_VERTICAL`（长边）/ `DMDUP_HORIZONTAL`（短边） | `-o sides=one-sided / two-sided-long-edge / two-sided-short-edge` |
| 手动双面 | 程序分两轮渲染奇 / 偶页 | `-o page-set=odd/even`，倒序加 `-o outputorder=reverse` |
| 颜色 | `dmColor` = `DMCOLOR_MONOCHROME` / `DMCOLOR_COLOR` | `-o print-color-mode=monochrome/color` |
| 纸张 | `dmPaperSize`（A4 = `DMPAPER_A4`） | `-o media=A4` |
| 纸盒 | `dmDefaultSource`（取自 `DC_BINS`） | `-o InputSlot=…`（取自 PPD） |
| 缩放 | 程序按 `PHYSICALOFFSETX/Y` 实算（第六节） | `-o print-scaling=none`（实际大小）/ `fit` |
| 任务名 | `DOCINFOW.lpszDocName` | `-t 标题` |
| 驱动高级选项 | 「打印机首选项…」= `DocumentPropertiesW(DM_IN_PROMPT)`，改完的 DEVMODE 整块留用 | 无（CUPS 的选项已在上面） |
| 系统对话框 | 保留现有 `PrintDlgW` 路径 | 「系统打开」交给预览 / 阅读器 |

要点：

- `lpstat` 的输出随系统语言翻译，**macOS 上设 `LC_ALL=C` 也照样是中文**（CUPS 在苹果平台
  遇到 C 语言环境会改用系统首选语言）。所以不解析句子：名字取 `lpstat -e`（每行一个），
  默认打印机取 `lpstat -d` 最后一个冒号之后、且确实在清单里的名字，状态取 `lpoptions -p`
  输出的 IPP 键值。
- IPP 标准选项（`sides`、`print-color-mode`、`media`）优先，CUPS 自己映射到 PPD；
  只有纸盒、介质类型这类没有标准名的才用 PPD 键。无 PPD 的免驱（IPP Everywhere）
  打印机 `lpoptions -l` 也会列出生成的选项，列不出来就只给标准选项。
- 用户数据不拼进命令：打印机名、页码串都作为独立参数传给 `Command`，页码串在程序
  里先解析成区间再格式化，不原样透传；文件名前照旧加 `--`。
- Windows 网络打印机的 `EnumPrintersW` level 2 可能卡几秒，所以列表用 level 4、
  状态与能力逐台在后台查，查不到就显示「状态未知」，不阻塞打印。

## 五、公文场景的默认值与提醒

调用方在打开对话框时附带 `PrintHints`（导出成品、送批材料知道这些，打开归档扫描件
时为空），对话框再从 PDF 自身补测：

| 情形 | 判据 | 默认 / 提醒 |
|---|---|---|
| 版式按双面排的 | 模板 `duplex_printing`（奇右偶左页码） | 默认长边双面；选了单面提醒「页码按双面排，单面打印页码会左右交替」 |
| 整份横排 | 每页都是 `/Rotate 90` | 默认短边翻转（`typst-engine.md` 的约定）；选长边时提醒 |
| 混有横向附件 | 部分页 `/Rotate 90` | 仍是长边（pdflscape 结构就是为此），不提醒 |
| 送批材料合并件 | 调用方标记 | 默认双面；选单面提醒「每件从奇数页起排，单面会多出空白页」 |
| 红头 / 红色印章 | 模板为红头文种 | 选黑白或单色机时提醒「版头红色将以灰色打出」，不拦 |
| 纸张不符 | 页面尺寸 ≠ 所选纸张 | 提醒，并建议切「适合纸张」 |

这些都只是默认值与提醒，不拦打印。用户改过的选项按打印机记住，下次以记住的为准；
但「版式按双面排」这类来自文档本身的默认值每次重新判定。

## 六、架构

### 模块

`print_pdf.rs` 扩成模块文件夹 `src/print/`（单文件会超 2000 行）：

| 文件 | 内容 |
|---|---|
| `mod.rs` | `PrintRequest`、`PrintHints`、`PrintSettings`、`PrinterInfo`、`PrinterCaps`、`PrintOutcome`；页码串解析、张数计算、提醒判定（纯函数，全可单测） |
| `dialog.rs` | egui 对话框：状态机（加载中 → 可编辑 → 打印中 → 手动双面等翻面 → 完成） |
| `cups.rs` | `lpstat` / `lpoptions` 输出解析、`lp` 参数组装、执行与任务号回读 |
| `win/mod.rs`、`win/enum.rs`、`win/devmode.rs`、`win/gdi.rs` | 打印机枚举、能力查询、DEVMODE 读改、GDI 渲染（现有代码迁入 `gdi.rs`） |
| `preview.rs` | 对话框里的页面预览：复用 PDF 标签已渲染的纹理，按纸张轮廓与缩放模式摆放 |

`unsafe` 仍只在 Windows 子模块里，按 `docs/security-hardening.md` 写 SAFETY 注释、
句柄用守卫（`DcGuard`、`DocGuard` 照用；新增 `PrinterHandleGuard` 管 `OpenPrinterW` /
`ClosePrinter`）。`windows-sys` 需要加 `Win32_Graphics_Printing` 特性（枚举、
`DocumentPropertiesW`、`GetDefaultPrinterW` 在这里），具体函数所在特性以 windows-sys
文档为准。

### 数据流与线程

```
PDF 标签「打印」/ Ctrl+P
  → PdfAction::Print(PrintRequest{ path, title, current_page, page_count, hints })
  → 主线程打开 PrintDialog，后台线程枚举打印机 → WorkerResult::PrintersLoaded
  → 选中某台 → 后台查能力与状态（按打印机缓存）→ WorkerResult::PrinterCaps
  → 点「打印」→ 后台线程 print::submit(request, settings)
       进度 → WorkerResult::PrintProgress；结束 → WorkerResult::PdfPrinted（现有）
  → 手动双面：第一轮结束对话框停在「翻面」态，用户点继续再提交第二轮
```

UI 线程从不调用打印 API 或外部命令；后台任务带 `egui::Context` 唤醒主线程，与现有
打印、PDF 渲染一致。打印中关闭 PDF 标签不影响任务；对话框里的「取消」在 Windows 上
走 `DocGuard` 的 `AbortDoc`，CUPS 上任务已整份交出，取消改为调用 `cancel 任务号`。

### Windows 的「实际大小」与横页修正

- 渲染时**不应用 `/Rotate`**，按 MediaBox 的纸张方向出位图（hayro 若不提供开关，就把
  渲染结果反向转 90°），竖纸上的横页不再缩小。
- 实际大小：目标矩形 = 页面点尺寸 / 72 × `LOGPIXELSX/Y`，原点减去
  `PHYSICALOFFSETX/Y`，即按纸张左上角定位而不是按可打印区；超出可打印区的部分由
  驱动裁掉（公文四边边距都远大于硬件边距，不会裁到内容）。
- 适合纸张：保留现有 `fit_rect` 逻辑，给非 A4 来源的 PDF 用。
- 光栅化上限仍是 300 DPI。

### 配置

`AppConfig` 新增 `print` 段：`last_printer`、按打印机名的偏好（双面方式、颜色、纸盒、
手动双面出纸方向、缩放），Windows 另存驱动首选项改过的 DEVMODE（base64，带驱动
版本号，版本对不上就丢弃）。打印机被删或改名时偏好自然失效，不报错。

## 七、取舍

- **应用内模态，而非独立系统窗口**。egui 0.35 能用 `show_viewport_deferred` 开第二个
  原生窗口，但：输入法与 `ModalGuard` 的焦点处理都按主窗口设计；麒麟上 Wayland / X11
  多窗口的置顶与焦点行为不稳；主题、DPI 还得两边对齐。模态框在主窗口里居中、盖住
  背后的编辑区，效果上已经是「独立的打印框」。确有需要（例如想把对话框拖到另一块屏）
  再改成 viewport，`dialog.rs` 的界面函数不用动。
- **自做对话框，而非升级到 `PrintDlgExW` / macOS `NSPrintPanel`**。系统对话框在三平台
  长得不一样，公文的默认值与提醒也塞不进去；macOS 的原生面板还得为 Unix 另写一套
  Objective-C 桥。CUPS 命令行在 macOS 与麒麟上一致，已经够用。
- **保留系统对话框入口**。驱动独有的功能（装订、打孔、水印、安全打印 PIN）没法逐台
  适配，Windows 留「打印机首选项…」与「使用系统对话框…」两个出口。

## 八、分期

| 期 | 内容 | 验收 |
|---|---|---|
| 一 | `src/print/` 拆分；对话框骨架；打印机列表、默认与状态；份数、逐份、页码（全部 / 当前 / 自定义）；记住上次打印机；Windows 改用 `CreateDCW` + DEVMODE；**实际大小与横页修正**；Unix 由 `lp` 改为带 `-d -n -P -t` | 三平台能选非默认打印机打出指定页；Windows 打出的版心量得 156 mm；横向附件满页 |
| 二 | 能力查询；双面、颜色、纸张、纸盒、缩放；第五节的默认值与提醒；「打印机首选项…」「使用系统对话框…」 | 双面长短边在真机上翻转正确；单色机置灰彩色；整份横排默认短边 |
| 三 | 手动双面向导；对话框内页面预览与张数；按打印机记住偏好；打印进度与取消；CUPS 任务号回读 | 无双面单元的打印机手动双面正反对齐（含奇数页） |

每期都跑 `cargo fmt`、`clippy -D warnings`、`cargo test`，并更新
`docs/help/11-export.md`「打印」一节。

## 九、测试

- 纯函数单测：页码串解析（`1-3,5`、空格、全角逗号与连字符、倒序、越界、空串）、
  张数计算（单面 / 双面 / 手动双面 × 份数 × 奇偶页数）、提醒判定。
- CUPS：用固定的 `lpstat -e/-d/-p`、`lpoptions -l` 输出样本测解析（含本机 HP DeskJet
  这类没有 `Duplex` 键的）；`lp` 参数组装测「打印机名带空格 / 以短横线开头」不被当
  选项；沿用现有的假 `lp` 脚本测法，脚本把收到的参数写到文件里供断言。
- Windows：DEVMODE 字段映射写成「设置 → 字段值」的纯函数单测；`EnumPrintersW` 与
  GDI 只能真机验。
- 真机清单：Windows 本地 USB 打印机 + 网络共享打印机 + 「Microsoft Print to PDF」
  （可在没有打印机的机器上验版心尺寸）；麒麟 V10 + CUPS；macOS。

## 十、风险

- 部分 Windows 驱动无视 DEVMODE 里的双面 / 颜色字段，只认自己的私有区：这类打印机
  让用户走「打印机首选项…」，改完的 DEVMODE 整块用于本次并记住。
- `lpoptions -l` 的 PPD 键名各厂商不一（`Duplex` / `KMDuplex` / `EFDuplex`…）：双面只用
  IPP 标准的 `sides`，PPD 键只用于展示纸盒与介质，认不出的不展示。
- 手动双面的翻面方向因机器而异：第一次使用时向导里给一张示意图并让用户选
  「出纸面朝下 / 朝上」，按打印机记住；选错了最多废一轮纸，提示里写明先拿两页试。

## 十一、进度与交接

### 第一期（2026-10-21 完成，待真机验收）

代码：

| 文件 | 内容 |
|---|---|
| `src/print/mod.rs` | 类型（`PrintRequest`、`PrinterInfo`、`PrinterState`、`PrintJob`、`PrintOutcome`）、平台分派、体积上限、Windows 状态位映射 |
| `src/print/pages.rs` | 页码串解析（全角、中文标点、`8-` 开到末页、越界截断）、区间合并、`lp -P` 格式、出纸顺序（逐份 / 不逐份） |
| `src/print/raster.rs` | 平台无关的光栅计算：BGRA 转换、按 90° 转位图、`turns_for_paper`（撤掉 `/Rotate` 再按需转向）、`place_page`（实际大小 / 退回适合纸张） |
| `src/print/dialog.rs` | 对话框：打印机下拉 + 状态点、份数、逐份、全部 / 当前页 / 自定义、张数、提醒；含忽略型样张测试 `print_dialog_samples`（出到 `tmp/print-dialog-*.png`） |
| `src/print/cups.rs` | `lpstat` / `lpoptions` 解析、`lp` 参数组装（`-d -n -o collate -P -t --`） |
| `src/print/win/{mod,printers,gdi}.rs` | 枚举、默认打印机、状态、DEVMODE（份数与逐份交给驱动，不支持就程序逐份重复）、`CreateDCW`、GDI 出纸；保留 `PrintDlgW` 路径给「使用系统对话框…」 |
| `src/app/printing.rs` | 外壳：开对话框时后台列打印机、逐台补状态；打印与系统对话框都在后台线程；记住上次的打印机 |

配置新增 `AppConfig.print.last_printer`。PDF 标签加了 Ctrl/⌘+P。

与方案的出入：

- 「使用系统对话框…」提前到第一期：原有 `PrintDlgW` 代码改成复用新的 GDI 出纸，
  顺手保留，免得第二期再加回来。
- 缩放没有做成选项：Windows 一律「放得下就实际大小，放不下（如 A3 稿打 A4 纸）
  退回适合可打印区」，与 CUPS 的 `print-scaling=auto` 一致。第二期再给开关。
- Windows 状态用 level 6 + level 4，不用 level 2：`PRINTER_INFO_2W` 在 windows-sys
  里要额外开 `Win32_Security` 特性，而且 level 2 会连网络打印机，慢。
- 对话框里还没有页面预览（第三期）。

已知坑：

- hayro 的 `render` 一定按 `/Rotate` 出图，没有开关；打印按介质方向出，靠把位图再转
  回去（`raster::rotate_ccw`），一张 300 DPI 的 A4 多占一份约 35 MB 的临时内存。
- `DEVMODEW` 的 `dmCopies` 在联合体里；这里按值读出公共部分、改完写回，紧随其后的
  驱动私有区不动。改 DEVMODE 的其他字段（第二期的双面、颜色、纸张）照这个办法。
- 整个项目没法在 macOS 上交叉编译到 Windows（zstd 等 C 依赖要 MSVC 头文件），
  所以 Windows 代码是另建一个小工程交叉检查的：在草稿目录建 crate，只依赖
  `hayro` 与 `windows-sys`，把 `src/print/` 下除 `dialog.rs`、`cups.rs` 外的文件
  软链进去，`dialog.rs` 换成空壳，再 `cargo clippy --target x86_64-pc-windows-msvc`。
  这只能查类型与 clippy，**运行行为必须真机验**。

真机验收清单（第一期）：

1. Windows：本地 USB 打印机、网络共享打印机各打一次指定页码；选非默认打印机；
   2 份逐份与不逐份；驱动不支持多份时程序逐份重复是否正确。
2. Windows「Microsoft Print to PDF」打一份普通公文，量版心 156 × 225 mm、左边距
   28 mm 是否准确；打一份带横向附件的公文，横页是否满页、方向与竖页一致。
3. Windows 共享打印机脱机时：清单是否先出来、状态随后补上，对话框不卡。
4. 「使用系统对话框…」仍能正常打印、取消。
5. 麒麟 V10 / macOS：清单、默认标记、状态（关掉打印机看是否显示脱机）；
   自定义页码、份数是否被 CUPS 接受。
6. 上次用的打印机下次打开是否默认选中；该打印机被删后是否退回系统默认。

### 第二期（2026-10-21 完成，待真机验收）

代码：

| 文件 | 内容 |
|---|---|
| `src/print/mod.rs` | `Duplex` / `ColorMode` / `Scaling` / `MediaOption` / `PrinterCaps`、`PrintHints`、`default_duplex` 与 `print_warnings` 纯函数、`edit_preferences` 平台分派、定长宽字符名字截取 |
| `src/print/cups.rs` | `lpoptions -l` 能力解析（`parse_caps`）；`lp` 的 `-o sides` / `print-color-mode` / `PageSize` / `InputSlot` / `print-scaling` |
| `src/print/inspect.rs` | 用 hayro 测「整份横排」与「第 1 页有红色」；手写最小 PDF 的单测（不提交二进制样本） |
| `src/print/win/printers.rs` | `DeviceCapabilitiesW` 查双面、颜色、纸张、纸盒；默认 DEVMODE 抽成共用函数；`open_device` 收 `&PrintJob` 一次合并；`edit_preferences`（`DM_IN_PROMPT`） |
| `src/print/win/gdi.rs`、`src/print/raster.rs` | `place_page` 加 `force_fit`，「适合纸张」直接走 `fit_rect` |
| `src/print/dialog.rs` | 双面 / 颜色 / 纸张 / 纸盒 / 缩放五行、能力按打印机缓存、张数按双面折算、提醒区、Windows 首选项按钮与四行置灰 |
| `src/app/printing.rs` | 后台查能力、弹首选项、测 PDF 自身事实；样张扩为能力未到 / 有双面 / 无双面 / 提醒四种 |
| `src/pdf_viewer.rs`、`src/app/tabs.rs`、`src/app/send_package_ui.rs`、`src/draft_page/tasks.rs` | `PrintHints` 随 PDF 标签带上：起草页导出按 `profile.duplex_printing`，送批材料打开合并件时标 `send_package` |
| `docs/help/11-export.md` | 「打印」一节补选项、提醒、首选项与默认值 |

与方案的出入：

- 双面、颜色下拉各多一项「打印机默认」，对应 `PrintJob` 里的 `None`。规格只列了三个 / 两个取值，但「没动过」的状态要能显示、也要能改回默认；纸张与纸盒的第一项同理。
- 能力没回来时是一行「选项 + spinner」，不是五行各画一个 spinner：五行各一个观感很乱，规格的本意是别画空控件。
- 能力按需查：清单到达默认选中、用户换打印机各查一次，按打印机名缓存、换回不重查。没有预查整张清单——网络打印机逐台查会把对话框拖住。
- 换打印机时清掉纸张与纸盒（编号是每台打印机自己的）；双面、颜色保留但提交时按新打印机能力过滤。
- 提醒要等能力回来才算：判断单色机、纸张名字都要 `PrinterCaps`。
- 纸张 / 纸盒默认值：Windows 读驱动默认 DEVMODE 的 `dmPaperSize` / `dmDefaultSource`；默认值不在能力列表里就当没有（不显示选不中的默认纸型）。
- 样张画布从 760 × 560 加大到 820 × 780（多出五行，原尺寸会截断）；对话框宽度按规格从 460 加到 500。
- 首选项交回的 DEVMODE 存整块缓冲区字节（末尾不足 8 字节有补零），`open_device` 按 `dmSize + dmDriverExtra` 校验，驱动只读声明的长度。
- 第一期「缩放没有做成选项」的出入在这一期补上；`Scaling::Actual` 沿用第一期的实际大小逻辑。

已知坑：

- 仓库外小 crate 的 Windows 交叉检查只覆盖 `src/print/` 里平台无关的文件与 `win/`，覆盖不到 `dialog.rs` 与 `app/printing.rs` 的 Windows 分支；首选项按钮那条链路只能 Windows 真机或 CI 验。
- `DC_PAPERNAMES` 每项定长 64 个宽字符、`DC_BINNAMES` 24 个；名字占满时没有 NUL，按定长取（`fixed_wide_name`）。
- `DC_PAPERS` / `DC_PAPERNAMES`（以及 `DC_BINS` / `DC_BINNAMES`）两次调用的个数对不上就整组丢弃：宁可置灰，也不给对不上的编号。
- CUPS 没有 `ColorModel` 键时按「能选彩色」处理（CUPS 不支持会忽略 `-o` 参数），彩色能力判断偏乐观；只有明确列出灰度类时才判成单色机。
- `DM_IN_PROMPT` 的输入 / 输出缓冲区都要 8 字节对齐；首选项交回的字节按小端拼进 `Vec<u64>`，不写非对齐读写。
- 首选项与能力查询可能不一致（首选项里开了双面，`DC_DUPLEX` 仍报 1）：界面按首选项置灰，不再参考能力。
- 红色提醒是启发式：36 DPI 下 50 个红像素（`inspect.rs` 有注释），扫描件偏色、彩色插图边沿可能误报；只是提醒，不拦打印。
- 能力缓存只在本次打开期间有效，不落盘（第三期按打印机记偏好时再考虑持久化）。
- `inspect_pdf` 打不开、超体积上限都返回 `None`：事实只用于默认值与提醒，不能因为读不出来就拦住打印对话框。
- 横页默认短边、双面默认长边都要求 `caps.duplex` 为真；无双面单元的机器不会预置一个提交不出去的值。

真机验收清单（第二期）：

1. Windows：双面长边 / 短边在真机上翻转方向正确（竖纸装订边在左）；单色机彩色项置灰；纸张换 A4 / A3 / Letter、纸盒换 Tray1 / Tray2，实际出纸按所选。
2. Windows：「实际大小」打 A4 公文版心仍是 156 × 225 mm（缩放不回归）；「适合纸张」把 A3 稿缩进 A4、满页可用。
3. Windows 首选项：设装订 / 双面后打印，四行置灰并显示说明；「恢复」清掉；换打印机自动清掉；点取消不改状态；驱动不认 DEVMODE 时是否如实反映。
4. 默认值与提醒：按双面排的稿件打开默认长边；整份横排默认短边并提示；有红色版头选黑白有提示；送批材料合并件默认双面、选单面提示；非 A4 用实际大小提示。
5. macOS / 麒麟：本机 `lpoptions -l` 输出下纸张、纸盒、双面、颜色能选，实际出纸按所选（重点看没有 `Duplex` 键的机器、纸盒键名的厂商差异）。
6. 「使用系统对话框…」仍能正常打印、取消。

### 第三期（2026-10-21 完成，待真机验收）

代码：

| 文件 | 内容 |
|---|---|
| `src/models/draft.rs` | `PrintConfig.printers`（`BTreeMap<String, PrinterPrefs>`）与 `PrinterPrefs`；旧配置缺字段时按空表读 |
| `src/print/dialog.rs` | 按打印机预填偏好（用户动过 > 文档默认 > 偏好 > 打印机默认）；手动双面状态机与翻面面板；进度与取消；左侧预览栏（纸张轮廓、正面 / 反面、翻页）；纸张比例表 |
| `src/print/pages.rs` | `manual_duplex_passes`：按**要打的页的序号**分正反两轮，并标出页数为奇数 |
| `src/print/win/gdi.rs`、`win/mod.rs` | 每画完一页报 progress、页间检查 cancel（取消经 `DocGuard` 的 `AbortDoc` 作废任务）；倒序出纸序 |
| `src/print/win/printers.rs` | `Duplex::Manual` 也写 `DMDUP_SIMPLEX`（每一轮只打单面） |
| `src/print/cups.rs` | `-o outputorder=reverse`；取消在调用 `lp` 之前检查；从 `lp` 输出里按「打印机名-数字」取任务号（中英文单测） |
| `src/print/mod.rs` | `print(job, progress, cancel)`、`PrintOutcome::Printed { job }`、`PrintRequest.pdf_key` |
| `src/pdf_viewer.rs` | `PdfSession::page_texture`：只读已渲染纹理，不触发渲染 |
| `src/app/printing.rs`、`src/app/jobs.rs` | `Arc<AtomicBool>` 传取消、`PrintProgress` 回填、`PdfPrinted` 带 `dialog` 编号、手动双面结果转回对话框、偏好落盘 |
| `docs/help/11-export.md` | 帮助补手动双面、打印中进度与取消、页面预览、按打印机记住偏好 |

与方案的出入：

- **手动双面不补空白页**：页数为奇数时按任务规格改成提示「最后一张没有反面，放回前先把它取出」，背面轮少打一张空白。方案第三节写的是「偶数轮末尾补一张空白」，任务规格 3.2 写的是提示取出，按任务规格做。
- 翻面方向简化成「反面倒序出纸」一个复选框（逐台记住），没有做成方案里的「出纸面朝下 / 朝上」两个选项：实际只有一个方向可用，试一次就知道；任务规格也是这么写的。
- 打印失败时对话框留在屏幕上显示错误（普通打印也如此），而不是关掉只在状态栏报错——3.3 要求打印中不关，失败自然要能重试。
- 预览按纸张比例画轮廓，但拿不到页面在纸上的真实点位：实际大小 / 适合纸张用「填满纸面 / 内缩 5%」两种示意，页面纹理直接缩放显示，不按 PDF 页面尺寸重算版心。
- 预览的页码是「要打的页里的第几张 / 共几张」，不是 PDF 页码；「正面 / 反面」按这个顺序的奇偶标注。
- 取消只在 Windows 上能真的中止任务；CUPS 只有「交给 `lp` 之前」的窗口期，已经交出的整份任务收不回来（设计文档本来就是这么划的）。
- 手动双面背面轮为空（只剩一页）时点「继续打印反面」直接收场，不提交空任务。
- 窗口宽度 680（第二期的 500 + 180 预览栏 + 间距），右栏打印机下拉收窄到 200 并允许换行，否则 Windows 上首选项按钮会挤出对话框。

已知坑：

- `PdfSession::page_texture` 只读已有纹理；用户没滚到的页在预览里是占位。**不要**为预览另起渲染线程，会和 PDF 标签的渲染队列抢显存。
- 打印中对话框只认「取消」（`Dismiss::ButtonsOnly`），Esc 与遮罩都关不掉；取消后 Windows 走 `AbortDoc`，CUPS 已交出的任务收不回来。
- progress 的页数是**本轮**的页数：手动双面一轮里是正面或反面各自的页数，不是整份。
- `PrintOutcome::Printed` 现在带 CUPS 任务号；Windows 的 GDI 报不出任务号，状态栏只显示打印机名。
- 手动双面第二轮的「继续打印反面」失败后回到等待翻面状态，纸别动；再点会重新提交同一批反面页。第一轮失败回编辑态。
- 偏好按打印机名存，打印机改名后旧偏好留在配置里，不报错，也永远不再命中。
- 预览纸张比例表是手写常见纸型（A3 / A4 / A5 / A6 / B5 / Letter / Legal / Executive），认不出的按 A4 画；实际出纸仍按所选纸盒里的纸。
- 「反面倒序」改了当帧写进配置（`take_preferences_update`），其它选项在点打印时写。

真机验收清单（第三期）：

1. 无双面单元的打印机（本机 HP DeskJet）：手动双面打 5 页，正面 1 / 3 / 5、翻面后背面 2 / 4，正反对齐；换「反面倒序」再打一次，选对方向并被记住。
2. 手动双面途中取消、第一轮失败（抽纸 / 卡纸）各一次：回到编辑态能改；第二轮失败回到翻面态。
3. Windows 普通打印：进度逐页走；点取消能及时作废任务（打印队列里不留半成品）；状态栏没有任务号也正常。
4. macOS / 麒麟：CUPS 任务号能回读（状态栏「任务 X-12」）；打印中点取消不误报成功。
5. 预览：滚到当前页后打开对话框能看见纹理；没渲染的页是占位；◀ ▶ 在要打的页里翻、自定义页码后位置正确；双面时正反面标注与出纸顺序一致。
6. 按打印机记住：两台打印机各设不同双面 / 颜色 / 纸张，来回切换预填正确；旧配置能读；点打印后配置里有 `printers` 段。
7. 对话框在 1280 × 800 与高 DPI 缩放下不溢出、预览不变形。
