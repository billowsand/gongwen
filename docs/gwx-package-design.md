# 稿件包专用后缀 .gwx：方案

> 状态：第一步（后缀与统一入口）已实现（2026-10-22），第二、三步未做。进度见第十一节。

## 一、要做的事

把稿件管理里「导出 / 导入」用的加密 ZIP 稿件包换成专用后缀 **`.gwx`**，并在系统里登记：
双击 `.gwx` 就由公文助手打开，直接进入**现有的导入流程**（输入口令 → 导入预览逐篇处理），
和现在在稿件管理里点「导入」再选文件的效果完全一样，只是省掉了点按钮、选文件两步。

| 场景 | 行为 |
| --- | --- |
| 程序没开，双击 `.gwx` | 启动公文助手，首帧后切到稿件管理并弹出该包的口令框 |
| 程序已开，双击 `.gwx` | 不另开窗口；已开的窗口被唤到前台，切到稿件管理并弹出口令框 |
| 把 `.gwx` 拖进窗口 | 同上 |
| 稿件管理点「导入」 | 文件框同时列出 `.gwx` 和旧 `.zip` |
| 稿件管理「导出」 | 默认文件名和过滤器改成 `.gwx` |

**不变的：**

- 包内格式与现在的 schema v2 一字不改（`manifests.json` + `vocabulary.json` + `images/` + `pdf/`，
  AES-256 加密、一律要口令），所以新版导入照样读旧 `.zip`，旧 `.gwx` 改名成 `.zip` 也能被旧版程序导入。
- 导入判定、合并、暂存分支全部沿用 `manuscript_io::sync`，本方案不碰（见 `docs/offline-sync-redesign.md`）。
- 「批量导出 PDF」的压缩包、「数据安全」里的整库备份仍是 `.zip`：前者是给别人看 PDF 的，
  后者不是稿件包，都不能走稿件导入。
- 不做「文件即文档、编辑后写回文件」的 Word 式用法，`.gwx` 只是交换包。

## 二、导入入口收拢成一个

现在导入只有一个入口 `start_import_manuscript`（`src/app/manuscript_ui/sync_io.rs:293`）：
弹文件框 → `open_zip_password_dialog(ZipPasswordPurpose::Import(path))`。新增的三个来源
（命令行参数、拖放、已开窗口收到转交）都汇到一个新函数：

```rust
/// 收到一个要导入的稿件包（按钮、启动参数、拖放、二次启动转交都走这里）。
pub(crate) fn receive_import_package(&mut self, path: PathBuf)
```

它做四件事：

1. **核对**：路径存在、是普通文件、扩展名是 `gwx` / `zip`（大小写不敏感），体积不超过
   `file_limits::MAX_PACKAGE_BYTES`（4 GB，只为挡住误选的超大文件把磁盘写满；包内各条目另有上限）；
   不合格只在状态栏提示，不弹窗。
2. **复制到程序暂存目录**（`cache_dir()/incoming/<随机名>/<原名>`，每个包一个子目录，副本保留原文件名）。原因：邮件客户端、
   即时通讯软件双击附件时给的是它们自己的临时文件，关掉预览窗口就可能被删；而现有导入在
   预览时读一遍包、确认时（`import_with_actions`）还要重读一遍，中间文件没了就会失败。
   U 盘拔掉也是同一个问题。按钮选的文件同样复制，路径行为统一。临时文件在导入完成、取消或
   程序下次启动时清掉。
3. **排队**：一律放进 `pending_imports: VecDeque<PathBuf>`。每帧的 `pump_pending_imports`
   在口令框、导入预览、待处理分支合并框都没开时，删掉上一个副本（`active_import`），
   再取下一个。这样确认、取消、出错关框都不用各自接线。状态栏显示「还有 N 个等待导入」。
   在资源管理器里选中多个 `.gwx` 回车，Windows 会为每个文件各起一个进程，第二步做完后都按这个队列依次处理。
4. **切页并弹口令框**：切到 `NavPage::Manuscript`，调用现有的 `open_zip_password_dialog`。
   口令框正文第一行加粗显示文件名（副本保留原名），让用户知道导入的是哪个包。

`start_import_manuscript` 改成：文件框过滤器换成 `("公文稿件包", &["gwx", "zip"])`，
选中后调 `receive_import_package`。

## 三、启动参数

`main.rs` 目前不读命令行参数。改动：

- `main` 里用 `std::env::args_os().skip(1)` 找出所有 `.gwx` / `.zip` 路径（相对路径按当前目录转绝对路径），
  其他参数忽略。
- 先做「已开窗口转交」检查（第四节）；转交成功就直接退出，不建窗口。
- 否则把路径列表交给 `GongwenApp::new(cc, startup_imports)`，App 在首帧画完后
  （和现在补发最大化同一时机）逐个调 `receive_import_package`。稿件库没打开成功时提示
  「稿件库不可用，无法导入」，不丢弃路径之外的其他启动流程。

## 四、已开窗口时的转交

目标：程序已开时再双击 `.gwx`，不出现第二个窗口，由已开的窗口弹出导入流程。也避免两个进程
同时写同一个稿件库和 `open_tabs` 记录。

**不引入新依赖、不开网络端口**（本机端口可能触发防火墙提示，也不符合离线约定），用
「锁文件 + 收件目录」：

1. 主实例启动时以独占方式打开 `config_dir()/instance.lock` 并 `File::try_lock()`
   （标准库，Rust 1.89 起稳定），整个进程生命周期持有。进程崩溃时系统自动释放锁，不会有残留锁。
2. 新进程启动时同样 `try_lock()`：
   - 拿到锁：自己就是主实例，照常启动。
   - 拿不到锁：说明已有实例。把每个待导入路径写成 `config_dir()/inbox/<uuid>.txt`
     （先写 `.tmp` 再改名，保证对方读到的是完整内容），然后退出。没有待导入文件时
     （比如用户又点了一次开始菜单）只写一个「唤到前台」的空请求，同样退出。
3. 主实例起一个后台线程，每 300 ms 看一次 `inbox/`（只列目录，开销可忽略）；有新文件就读出
   路径、删掉请求文件，通过通道交给界面线程，并 `ctx.request_repaint()`。界面线程收到后：
   `ViewportCommand::Minimized(false)` → `ViewportCommand::Focus` → `receive_import_package`。
4. 前台激活：Windows 不允许后台进程随便抢前台，`Focus` 可能只让任务栏按钮闪烁。
   先接受这个结果（再补发一次 `RequestUserAttention`，确保任务栏会闪）。若真机上体验不好，
   第二步再让转交进程退出前调 `AllowSetForegroundWindow(ASFW_ANY)`（`windows-sys` 已是依赖，
   放进现有 FFI 模块，按安全约定写 SAFETY 注释）。
5. 收件目录里超过 10 分钟没被取走的请求（例如主实例当时卡死后被强杀）在下次启动时清掉，不补做导入，
   避免过了很久突然弹出口令框。

> 附带好处：以后从开始菜单再点一次图标也是把已开窗口唤到前台，而不是开第二个窗口。
> 这改变了现有行为，需在发布说明里写明。如果仍需要多开（目前没看到这样的用法），可以留一个
> `--new-window` 参数跳过锁检查。

## 五、系统登记

### Windows（Inno Setup，`scripts/gongwen-assistant.iss`）

安装是 `PrivilegesRequired=lowest`，登记写到当前用户（`HKA` 在低权限下即 `HKCU\Software\Classes`）：

```ini
[Setup]
ChangesAssociations=yes

[Registry]
Root: HKA; Subkey: "Software\Classes\.gwx"; ValueType: string; ValueName: ""; ValueData: "GongwenAssistant.Package"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.gwx\OpenWithProgids"; ValueType: string; ValueName: "GongwenAssistant.Package"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\GongwenAssistant.Package"; ValueType: string; ValueName: ""; ValueData: "公文助手稿件包"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\GongwenAssistant.Package\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\gwx-document.ico"
Root: HKA; Subkey: "Software\Classes\GongwenAssistant.Package\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""
```

- `[Registry]` 段**不放进** `#if MyUpgradeMode == "0"`：从旧版本用增量升级包升上来的用户也要拿到关联。
  增量包按校验值做差，`gwx-document.ico` 是新文件，第一次升级时会自动带上。
- 卸载时删掉 ProgId 整键、`.gwx` 只删默认值（别的程序可能也挂在 `.gwx` 下）。
- 便携版不登记（没有安装步骤）。需要的话后续在设置页加「关联 .gwx 文件」按钮，本期不做。

### Linux（deb，`scripts/package-deb.sh` 与 `package-linux-tar.sh`）

- 新增 `assets/linux/cn.localtools.GongwenAssistant.xml`（freedesktop shared-mime-info）：

  ```xml
  <mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
    <mime-type type="application/x-gongwen-package">
      <comment>公文助手稿件包</comment>
      <glob pattern="*.gwx"/>
      <sub-class-of type="application/zip"/>
      <icon name="application-x-gongwen-package"/>
    </mime-type>
  </mime-info>
  ```

  装到 `/usr/share/mime/packages/`。`sub-class-of` 写 zip，但 glob 优先，桌面不会把它当普通压缩包打开。
- `.desktop` 改两行：`Exec=gongwen-assistant %F`、加 `MimeType=application/x-gongwen-package;`。
- 文档图标按尺寸装到 `/usr/share/icons/hicolor/<尺寸>/mimetypes/application-x-gongwen-package.png`。
- `update-mime-database`、`update-desktop-database`、`gtk-update-icon-cache` 都由 dpkg 触发器
  自动跑（shared-mime-info、desktop-file-utils、hicolor 主题包自带），不必写 postinst。
  要在 `scripts/smoke-launch-linux.sh` / `verify-deb-install.sh` 里加一条检查：
  `xdg-mime query filetype x.gwx` 应返回 `application/x-gongwen-package`，
  `xdg-mime query default application/x-gongwen-package` 应是本程序的 desktop 文件。

### macOS

不再发布，本期不做（以后要做就在 `Info.plist` 加 `CFBundleDocumentTypes` / `UTExportedTypeDeclarations`，
并处理 `application:openURLs:` 事件——macOS 双击不是走命令行参数）。

## 六、文档图标

和程序图标区分开，但一眼能看出是公文助手的东西：

- 造型：白色纸页 + 右上角折角，纸页下半部放缩小的品牌标（`assets/icons/brand-mark.svg` 的几何），
  底部一条强调色色带写 `GWX`（16、24 两个小尺寸去掉文字，只留纸页和色带）。
- 颜色：取默认主题的强调色（`generate-brand-icons.py` 里 `claude` 主题的 `accent`），不随主题切换——
  系统里登记的图标只有一套。
- 生成：在 `scripts/generate-brand-icons.py` 里加一个 `document_icon()`，复用现有几何与绘制函数，
  输出到 `assets/doc-icon/`：`gwx-document.ico`（16/24/32/48/64/256 多尺寸）和
  `gwx-document-<尺寸>.png`（Linux 用）。生成物入库，和 `assets/app-icon/` 同样处理。
- 打包：`package-portable.ps1` 组装 `dist\win-x64-full` 时把 `gwx-document.ico` 复制到程序目录根；
  deb / tar 复制 png 到第五节的路径。

## 七、改动清单

| 文件 | 改动 |
| --- | --- |
| `src/main.rs` | 解析启动参数；单实例检查与转交；把路径交给 App |
| 新增 `src/single_instance.rs` | 锁文件、收件目录读写、后台轮询线程、过期请求清理（带单元测试） |
| `src/app.rs` | `GongwenApp::new` 收启动路径；首帧后派发；收件通道接线；处理 `dropped_files` |
| `src/app/manuscript_ui/sync_io.rs` | `receive_import_package`、导入队列、复制到临时目录；导出默认名 `.gwx`、过滤器改为 `.gwx`；保存框若用户删了后缀自动补 `.gwx` |
| `src/app/manuscript_ui.rs` | 口令框显示原文件名；处理完一个后弹出队列下一个 |
| `src/package_file.rs`（第一步已建） | 后缀判断、保存路径补后缀、启动参数解析、暂存目录复制 / 删除 / 启动清理 |
| `src/storage.rs` | `inbox`、`instance.lock` 路径函数（第二步） |
| `scripts/gongwen-assistant.iss` | `ChangesAssociations`、`[Registry]` 五条 |
| `scripts/package-portable.ps1` | 带上 `gwx-document.ico` |
| `scripts/package-deb.sh`、`package-linux-tar.sh` | mime xml、文档图标 |
| `assets/linux/*.desktop` | `Exec %F`、`MimeType` |
| `scripts/generate-brand-icons.py` + `assets/doc-icon/` | 文档图标 |
| `scripts/verify-deb-install.sh` | xdg-mime 检查 |
| `src/help/` 使用帮助、`README.md` | 稿件包改名 `.gwx`、双击导入、旧 `.zip` 仍可导入、与旧版本互传时改名即可 |

拖放：在 `App::update` 里读 `ctx.input(|i| i.raw.dropped_files.clone())`，只收 `gwx` / `zip`；
其他类型保持现状（目前没有拖放处理，不影响）。拖进来时可以顺手画一层「松开即导入稿件包」的提示，
用 `hovered_files` 判断。

## 八、分步实施

每步单独提交，都过 `fmt` / `clippy -D warnings` / `test`。

1. **后缀与统一入口**：导出改 `.gwx`、导入过滤器、`receive_import_package`（核对、复制、排队）、
   启动参数、拖放。做完这一步，命令行 `gongwen-assistant.exe 某包.gwx` 已经能直接进导入流程。
2. **已开窗口转交**：`single_instance.rs` 与接线。
3. **系统登记与图标**：生成文档图标、Inno 注册表、deb mime、使用帮助与 README。

## 九、验收

- 新导出的包后缀是 `.gwx`，用新版导入：口令、预览、合并与旧 `.zip` 完全一致；旧 `.zip` 仍能导入；
  `.gwx` 改名为 `.zip` 后旧版程序能导入。
- Windows 安装后：`.gwx` 显示文档图标；程序未开时双击 → 启动并弹口令框；程序已开（含最小化）
  时双击 → 不出第二个窗口，已开窗口到前台（或任务栏闪烁）并弹口令框；同时选中三个 `.gwx` 回车 →
  依次弹三次，状态栏显示排队数。
- 从邮件客户端或压缩软件里直接双击附件：预览后关掉那个软件，再点确认导入仍成功（验证复制到临时目录）。
- 口令错误 → 现有的「口令不对」提示，可重试；随便一个文件改名成 `.gwx` → 提示「不是公文助手稿件包」，不崩。
- 卸载后 `.gwx` 关联与图标消失；增量升级包从旧版升上来后关联生效。
- Linux deb：`xdg-mime` 两条检查通过，文件管理器双击能启动并弹口令框。
- 杀掉主进程后再双击 `.gwx`：能正常起新实例（锁已释放），不会被旧的收件请求打扰。

## 十、已知问题与取舍

- **与旧版本程序互传**：旧版的导入文件框只认 `.zip`，选不到 `.gwx`。内容格式没变，改名为 `.zip`
  即可；使用帮助里写明。内网各机升级到同一版本后就不再有这个问题。
- **从开始菜单再点图标不再开第二个窗口**（第四节附带的行为变化），发布说明写明；必要时留 `--new-window`。
- **Windows 前台激活**可能只闪任务栏，第一版接受，真机体验不好再加 `AllowSetForegroundWindow`。
- 包里的口令仍然必须输入（一律加密不变）；「记住口令」沿用现有的 `remembered_zip_password`。

## 十一、实施进度

### 第一步：后缀与统一入口（2026-10-22，已实现，待真机验收）

- `src/package_file.rs`：`EXTENSION` / `IMPORT_EXTENSIONS`、`is_package_path`、
  `with_package_extension`（保存框给回的路径没有稿件包后缀时补 `.gwx`，明确写 `.zip` 的保留）、
  `startup_paths`、`staging_root` / `stage` / `discard` / `clear_staging`，带单元测试。
- `src/app/manuscript_ui/sync_io.rs`：`receive_import_package`（核对 + 复制 + 入队）、
  `receive_startup_imports`（先清上次残留再收启动参数）、`receive_dropped_packages`
  （拖放 + 悬停提示层）、`pump_pending_imports`（每帧，见第二节第 3 点）；两处稿件包导出
  走 `save_package_dialog`（默认名与过滤器都是 `.gwx`）；「批量导出 PDF」仍是 `.zip`。
- `src/main.rs` 解析启动参数交给 `GongwenApp::new(cc, startup_imports)`。
- 界面文字：按钮「导入 ZIP」→「导入稿件包」，口令框标题「输入稿件包密码」/「设置加密密码」，
  导出说明按用途区分稿件包与 PDF 压缩包。

与方案的出入：

1. 暂存目录放在 `cache_dir()/incoming`（本用户缓存目录），不是 `config_dir()/tmp`：副本随时可丢，
   不该进配置目录，也不会被「数据安全」的整库备份带走。
2. 顺手修了一个既有问题：窄屏下打开稿件详情（单页详情）时，`manuscript_confirm_groups`
   不画，口令框、导入预览、详情页里打开的待处理分支合并框都看不到。这些都是模态框，与布局无关，
   现在无论宽窄都画。

已知坑：

- 保存框里用户删掉后缀时由程序补 `.gwx`，补后的路径若已有同名文件会直接覆盖（系统保存框的覆盖确认
  针对的是补后缀前的名字）。Windows 的保存框在选中过滤器时通常自己会补后缀，实际很少走到。
- 复制在界面线程同步进行，几百 MB 的包会让窗口卡一两秒；真机若明显再挪到后台线程。

真机验收（第一步）：

- 稿件管理导出得到 `.gwx`；导入按钮能选 `.gwx` 和旧 `.zip`，流程与原来一致。
- 命令行 `gongwen-assistant.exe 某包.gwx` 启动后直接在稿件管理弹口令框，口令框显示文件名。
- 把一个或多个 `.gwx` 拖进窗口：悬停时有「松开即导入稿件包」提示，松开后依次弹口令框。
- 口令框开着时再拖一个包进来：状态栏提示排队，前一个处理完自动弹下一个。
- 窄屏打开稿件详情时拖入稿件包，口令框能正常显示。
