# Omarchy 主题适配方案

> 状态：第一期已实现，待 Omarchy 真机验收；第二、三期未开工。2026-10-28。进度与出入见第九节。

## 一、要解决什么

公文助手在 Omarchy（Arch + Hyprland）上已经处理了窗口外形（`src/linux_desktop.rs`：
平铺会话只留关闭按钮、不画自绘边缘），但配色仍是自己那六套预设。Omarchy 用户的
整个桌面——终端、Waybar、Hyprland 边框、浏览器、编辑器——会随 `omarchy theme set`
一起换色，公文助手是唯一不跟着变的窗口，显得格格不入。

目标只有一句：**在 Omarchy 上，公文助手的界面外壳跟着当前 Omarchy 主题走，切主题
即时变色；离开 Omarchy 一切照旧。**

不做的事：

- 不改公文纸面的取色原则。纸面只承接明暗与冷暖，不吃主题强调色（`theme.rs`
  `sheet_for_family` 的注释），导出的 DOCX / PDF 仍是白纸黑字红头。
- 不跟 Omarchy 换字体。界面中文字体与公文字体是我们自己的体系。
- 不往用户家目录写 Omarchy 钩子或模板（见第三节方案 B 为何不选）。
- 不为 Omarchy 再造一套固定的「Omarchy 风」预设——Omarchy 本身没有单一配色，
  它有 23 套内置主题加用户自装主题，静态预设只能贴其中一套。

## 二、Omarchy 主题现状（2026-10 核对 basecamp/omarchy master）

- 当前主题目录：`~/.local/state/omarchy/current/theme/`（新版）；旧版在
  `~/.config/omarchy/current/theme/`。目录旁有 `theme.name`（主题显示名）与
  `theme.generation`（每切一次换一个值）。
- 主题的唯一真源是 **`colors.toml`**，全是 `key = "#rrggbb"`：

  | 组 | 键 |
  |---|---|
  | 模式 | `mode`（`"dark"` / `"light"`） |
  | 强调 | `accent`、`selection`、`muted` |
  | 底色 | `background`、`dark_background`、`darker_background`、`lighter_background` |
  | 文字 | `foreground`、`dark_foreground`、`light_foreground`、`bright_foreground` |
  | 色相 | `red` `yellow` `orange` `green` `cyan` `blue` `magenta` `brown` |
  | 亮色相 | `bright_red` … `bright_magenta`（六个） |

- 其余配置（kitty、alacritty、fcitx5、VS Code、Claude Code……）都由
  `default/themed/*.tpl` 模板从 `colors.toml` 渲染出来；用户模板放
  `~/.config/omarchy/themed/*.tpl`，同名优先。切主题末尾会调
  `omarchy-hook theme-set <名字>`。
- 旧主题可能只有 `color0`–`color15` 等旧键名，Omarchy 自己靠
  `omarchy-theme-color` 做别名回退。
- 几个值得注意的「怪」主题：
  - `rose-pine`（明色）：`green` 实际是蓝绿 `#286983`，`cyan` 实际是玫瑰色；
    `lighter_background` 反而比 `background` 暗。**键名不可靠，只能当色值用**。
  - `white`：`accent` 是中灰 `#6e6e6e`，几乎无彩度。
  - `matte-black` / `vantablack`：背景接近纯黑，`muted` 与背景几乎同色。
  - `tokyo-night`：`dark_foreground` `#565f89` 压在 `#1a1b26` 上对比度不到 3:1。

## 三、方案比较

| | A. 程序直接读 `colors.toml`（推荐） | B. 安装 Omarchy 用户模板 | C. 做一套固定预设 |
|---|---|---|---|
| 做法 | 内置解析与映射，监视当前主题目录 | 往 `~/.config/omarchy/themed/` 放 `gongwen.json.tpl`，Omarchy 每次切主题替我们渲染 | 在 `ThemeName` 里加一套「Omarchy」静态色板 |
| 覆盖面 | 全部内置主题 + 任意用户主题 | 同左 | 只贴一套 |
| 侵入性 | 只读，不写用户配置 | deb / PKGBUILD 不能写家目录，只能首次启动由程序写入，卸载留残 | 无 |
| 派生色 | 我们自己算，能做对比度校正 | 模板只有 `mix`，没法按对比度挑色 | 手调 |
| 依赖 Omarchy 版本 | 只依赖 `colors.toml` 键名 | 还依赖模板语法与渲染脚本 | 无 |

选 **A**。B 唯一的长处是「主题作者能手写覆盖」，这个用 A 的扩展点就能拿到：
当前主题目录里若有 `gongwen.toml`（同样是 `key = "#hex"`，键名就是我们 `Theme` 的
字段名），逐键覆盖自动映射的结果。我们不安装、不生成它，只是认它。

## 四、整体设计

### 4.1 新增主题项「跟随 Omarchy」

- `ThemeName` 加一项 `Omarchy`（配置里存 `"omarchy"`）。
- 只有探测到 Omarchy 时才出现在设置页主题卡片与菜单「外观主题」里（不进
  `ThemeName::ALL` 的静态展示，另给一个 `ThemeName::available()`）。
- 卡片副标题显示 Omarchy 当前主题名（读 `theme.name`），例如「跟随 Omarchy ·
  Tokyo Night」。
- 选中它时「明暗跟随系统」开关置灰：明暗已由 Omarchy 的 `mode` 决定。
- 配置里是 `omarchy` 但当前机器找不到 Omarchy 主题目录（换了桌面、目录被删）：
  本次退回 Claude 奶油并在状态栏说明，**不改写配置**，回到 Omarchy 自动恢复。

### 4.2 探测

`src/theme/omarchy/detect.rs`，只在 Linux 编译真实逻辑，其余平台恒为「无」：

1. 环境变量 `GONGWEN_OMARCHY_THEME_DIR`（开发调试用，Windows 上也认，便于在本机
   拿官方 `colors.toml` 样本调色）；
2. `$XDG_STATE_HOME`（缺省 `~/.local/state`）`/omarchy/current/theme/colors.toml`；
3. `~/.config/omarchy/current/theme/colors.toml`（旧版）。

找到即认为是 Omarchy，不再额外要求 Hyprland 会话——用户可能在别的合成器里开着
Omarchy 配置，跟随与否让他自己选。

### 4.3 解析

不引入 `toml` 依赖：`colors.toml` 只有平铺的 `key = "#rrggbb"` 行，手写逐行解析
（跳过空行与 `#` 注释，值必须是 6 位十六进制，其余行忽略）。读前按
`src/file_limits.rs` 卡体积（1 MB，取整是为了让超限提示「超过 N MB」读得通）。解析失败不崩，退回上一次成功的配色并
在状态栏提示。

旧键名回退表照抄 Omarchy 的语义：`color0`→`background`、`color1`→`red`、
`color2`→`green`、`color3`→`yellow`、`color4`→`blue`、`color5`→`magenta`、
`color6`→`cyan`、`color7`→`foreground`、`color8`→`muted`、`color9`–`color15`→各
`bright_*`。缺失的键按第 4.4 节的派生规则从已有键算出；`background`、`foreground`
两项都缺才算失败。

### 4.4 映射：colors.toml → `Theme`

原则：

1. **按色值角色映射，不信键名的明暗含义**。四个 `*_background` 按亮度排序后再
   分配层级，避免 `rose-pine` 那种「lighter 反而更暗」。
2. **主底色保持原样**：`surface`（卡片、编辑区）直接用 `background`，这是 Omarchy
   终端里看到的那个颜色，认得出来。
3. **派生色在 OKLab 里混**，比 sRGB 线性混合在深色主题上不发灰。
4. **对比度兜底**：文字类颜色混出来之后过一道 WCAG 检查，不够就沿亮度方向推，
   直到达标。主文字 ≥ 7:1，次级 ≥ 4.5:1，弱化 ≥ 3:1（对 `surface` 与 `canvas` 两者
   取较差值）。

记号：`bg` = background，`fg` = foreground，`mix(a, b, t)` = 从 a 向 b 混 t。
`deeper(n)` = 把四个底色按「离 fg 越远越深」排序后，比 `bg` 深 n 级的那个
（不存在时用 `mix(bg, 黑或白, 4%·n)` 补）。

| `Theme` 字段 | 取值 | 说明 |
|---|---|---|
| `dark` | `mode == "dark"`；缺 `mode` 时按 `bg` 亮度判断 | |
| `canvas` | `deeper(1)` | 现有六套里 canvas 都比 surface 深一级 |
| `surface` | `bg` | |
| `surface_sunk` | `deeper(2)`（深色）／`mix(bg, fg, 4%)`（明色） | 输入框、表头 |
| `surface_hover` | `mix(bg, fg, 8%)` | 不用 `lighter_background`，键名不可靠 |
| `surface_active` | `selection`，与 `bg` 太近（ΔE<6）时 `mix(bg, fg, 14%)` | |
| `border` | `mix(bg, fg, 12%)` | |
| `border_strong` | `mix(bg, fg, 24%)` | 不用 `muted`：`matte-black` 的几乎等于底色，`white` 的又深到像文字 |
| `text` | `fg`，对比度兜底 7:1 | |
| `text_soft` | `mix(fg, bg, 22%)`，兜底 4.5:1 | |
| `text_muted` | `dark_foreground`，兜底 3:1 | 修 `tokyo-night` |
| `accent` | `accent`，对 `surface` 不足 3:1 时沿亮度推 | `white` 的灰强调可以接受，只保证看得见 |
| `accent_hover` | `mix(accent, fg, 12%)` | |
| `accent_active` | `mix(accent, bg, 25%)` | |
| `accent_soft` | `mix(bg, accent, 16%)` | |
| `warn` / `warn_soft` | `yellow` / `mix(bg, yellow, 16%)` | |
| `danger` / `danger_soft` | `red` / `mix(bg, red, 16%)` | |
| `success` / `success_soft` | `green` / `mix(bg, green, 16%)` | |
| `info` | `blue`；与 `accent` 同色时改用 `cyan` | |
| `paper_family` | 明色：`bg` 偏暖（b 通道明显低于 r）→ `Parchment`，否则 `Original`；深色：偏暖 → `Sandalwood`，否则 `NightBlue` | 只取明暗冷暖，不取强调色 |
| `dark_paper_family` | 同上规则只看冷暖 | |

`MdPalette`（Markdown 审校区高亮）沿用「终端配色先取一整组色相，再分派角色」的
做法（`palette.rs` 第 173 行注释）：

| 字段 | 取值 |
|---|---|
| `body` | `fg` |
| `marker` | `text_muted` |
| `title` | `accent` |
| `heading` | `bright_foreground`，缺失用 `text` |
| `strong` / `strong_bg` | `orange`（缺用 `yellow`）／ `mix(bg, strong, 14%)` |
| `bullet` | `accent` |
| `table_pipe` / `table_rule` | `border_strong` / `text_muted` |
| `table_cell` | `cyan` |
| `comment` / `comment_bg` | `text_muted` / `mix(bg, fg, 5%)` |
| `todo` / `todo_bg` | `danger` / `danger_soft` |
| `code` | `green` |
| `quoted` | `magenta` |
| `anchor_bg` | `surface_active` |
| `search_bg` | `mix(bg, yellow, 28%)` |

`md` 里的前景色同样过 4.5:1 兜底（对 `surface`）。

### 4.5 强调色实底上的文字

现状 `accent_text_for` 除 MDEX 夜墨外一律白字。Omarchy 大量深色主题的强调色是
浅色（`tokyo-night` `#7aa2f7`、`matte-black` 橙 `#e68e0d`），白字压不住。改成：
在 `Theme` 里加一个 `on_accent` 字段，六套固定主题按现状填（夜墨填墨色、其余填白），
Omarchy 主题取 `bg` 与 `bright_foreground`／白中对 `accent` 对比度较高的那个。
`accent_text_for` 改为直接读字段，去掉按 label 判断的特例。

### 4.6 外壳形状

Omarchy 默认 Hyprland 边框是直角细线。选「跟随 Omarchy」时外壳走 MDEX 那条路：
`chrome_radius` 返回 0、细描边。把 `is_mdex()` 里「直角外壳」这一含义抽成
`theme::square_chrome()`，MDEX 两套与 Omarchy 都返回真，`is_mdex()` 只保留品牌图标
那一处用途。

不读 `hyprctl getoption decoration:rounding`：要起子进程，收益只是几个像素的圆角，
而且用户圆角调大时我们的面板圆角也不该跟着变大。

窗口图标：Wayland 不允许客户端设窗口图标（`icons.rs` 已注明），Omarchy 模式下
`apply_app_icon` 用 Claude 那张即可；标题栏品牌标按现有逻辑用 `accent` 染色，
自然跟着主题走。

### 4.7 切主题即时生效

- 一条后台线程，每秒 `stat` 一次 `theme.generation`（没有就看 `colors.toml` 与
  `gongwen.toml` 的 mtime），有变化才读文件、解析、映射，把新 `Theme` 放进
  `theme::omarchy` 的缓存（`RwLock<Option<Theme>>`），再 `ctx.request_repaint()`。
  只 `stat` 不读文件，开销可忽略；不在主线程做，不碰 Wayland 1.5 秒存活探测。
- 主线程每帧比较缓存版本号，变了且当前主题是 `Omarchy`，就走现有那套
  `set_current` → `reconfigure_style`，与 `sync_system_theme` 同一形状。
- Omarchy 切主题是先换目录、再渲染、再逐个通知应用，中途 `colors.toml` 可能短暂
  缺失：读失败时保持上一份配色，下一秒再试，不闪回 Claude 奶油。
- 不用 inotify / `notify` crate：目录本身是被整个替换的符号链接，inotify 要跟着重
  挂，轮询更简单可靠。不装 `omarchy-hook`，理由同第三节。

### 4.8 输入法候选窗、Mermaid、高亮缓存

- 应用内输入法候选窗取的是 `theme::*` 取色函数，自动跟随，无需改动。Omarchy 的
  fcitx5 主题与我们无关（我们不用 fcitx5）。
- Mermaid 图表样式与公文纸面一样属于「内容」，不跟界面主题，维持现状。
- `highlight.rs` 已经以 `theme::revision()` 做缓存键，`set_current` 会递增它，切主题
  后高亮自动重算。

## 五、模块与改动面

```
src/theme/omarchy/
  mod.rs        // 对外：available()、palette()、label()、theme_name()、start_watcher()
  detect.rs     // 4.2 探测
  colors.rs     // 4.3 解析 + 旧键回退 + gongwen.toml 覆盖
  derive.rs     // 4.4 映射：OKLab 混色、亮度排序、对比度兜底（纯函数，可测）
  watch.rs      // 4.7 后台轮询线程
```

其他改动：

- `src/models/typography.rs`：`ThemeName::Omarchy`、`available()`。
- `src/theme/palette.rs`：`Theme` 加 `on_accent`；`by_name` 加 `Omarchy` 分支（无缓存时
  退回 `Theme::claude()`）。
- `src/theme/widgets.rs`：`accent_text_for` 读 `on_accent`。
- `src/theme/icons.rs`：`square_chrome()`；`app_icon_png` 加分支。
- `src/app/settings.rs`、`src/app/chrome.rs`：主题列表改用 `available()`；Omarchy 卡片
  副标题；选中时置灰「明暗跟随系统」；每帧检查缓存版本。
- 使用帮助「外观」一节补一段；`README.md` 的 Linux 说明提一句。

## 六、验证

1. **样本测试**：把 Omarchy 23 套内置主题的 `colors.toml` 收进
   `tests/fixtures/omarchy/<主题名>.toml`（Omarchy 是 MIT 许可，在
   `THIRD_PARTY_NOTICES.md` 登记），单测逐套跑映射，断言：
   - 第 4.4 节的三档对比度全部达标（含 `md` 前景与 `on_accent`）；
   - `canvas`、`surface`、`surface_sunk`、`surface_hover`、`surface_active` 两两 ΔE ≥ 3
     （层次分得出来）；
   - `rose-pine`、`white`、`matte-black`、`tokyo-night` 四套「怪」主题各有专门断言。
2. **解析测试**：旧键名回退、缺键派生、坏值忽略、超大文件拒读、`gongwen.toml` 覆盖。
3. **样张**：一个 `#[ignore]` 测试按 23 套主题出一张色卡拼板（每套一格：各层底色、
   三档文字、强调实底与 `on_accent`、语义色、Markdown 高亮样例），写成 SVG 人眼过一遍。
   现有 `egui_kittest` 关了默认特性、没有渲染后端，不为这事打开 wgpu 截图。
4. **真机**：Omarchy 上 `omarchy theme set` 连切几套，看是否一秒内跟上、切换过程不
   闪回默认主题；明暗主题互切时纸面家族跟着换。

## 七、分期

1. **第一期（核心）**：探测、解析、映射、`on_accent`、`ThemeName::Omarchy`、设置页
   入口、23 套样本测试。此时切主题后需重启或重新点一次卡片才生效。
2. **第二期（即时跟随）**：后台轮询、主线程版本号检查、中途缺文件的容错、直角外壳。
3. **第三期（收尾）**：`gongwen.toml` 覆盖、截图拼板样张、使用帮助与 README。

## 八、待定

- **首次在 Omarchy 上启动是否自动选「跟随 Omarchy」**。倾向：只有配置文件是新建的
  （首启）才自动选，已有配置的老用户不动，只在主题卡片里多出一项。
- **`white` 这类无彩度主题**的强调色要不要另找一个色相（比如取 `blue`）。倾向不找：
  用户选了单色主题，界面就该是单色的，只保证可见。

## 九、进度与交接

### 第一期（已完成，2026-10-28，待真机验收）

代码在 `src/theme/omarchy/`：`detect.rs`（找目录、读 `theme.name`）、`colors.rs`
（逐行解析与旧键回退）、`derive.rs`（OKLab 混色与对比度兜底的换算）、`mod.rs`
（`refresh` / `available` / `palette` / `theme_name` 与缓存）。接入点：

- `ThemeName::Omarchy`（存为 `"omarchy"`），**不在 `ThemeName::ALL` 里**；设置页与
  应用菜单在明色、深色两组之后另起「Omarchy 桌面」一组，只有 `available()` 时出现。
- `by_name(Omarchy)` 读缓存，没读到过就退回 Claude 奶油。
- 启动时 `GongwenApp::new` 调一次 `omarchy::refresh()`；配置是 Omarchy 却读不到时
  状态栏说明，配置不改。
- `apply_theme(Omarchy)` 每次都重读；已选中时再点卡片或菜单项也会重读，这就是
  第一期「Omarchy 换主题后再点一下跟上」的办法。
- `Theme` 新增 `on_accent`，`accent_text_for` 改为读字段，原来按 MDEX 夜墨 label
  判断的特例去掉；六套固定预设填值与原行为一致（夜墨墨色，其余白字）。
- 测试：`cargo test --locked omarchy` 共 15 条，其中 23 套官方样本逐套过对比度
  （正文 7:1、次级与 Markdown 高亮 4.5:1、弱化 / 强调 / 语义 3:1、按钮字三态 4.5:1）
  与层次（ΔE ≥ 2）检查；样本放 `tests/fixtures/omarchy/`，已登记
  `THIRD_PARTY_NOTICES.md`。

与方案的出入：

- `border_strong` 不用 `muted`，一律 `mix(bg, fg, 24%)`（见 4.4 表）。
- 「明暗跟随系统」开关没有置灰：选「跟随 Omarchy」时 `apply_theme` 本来就会把它
  关掉，再勾上就是回到按系统明暗选 Claude 奶油 / 复古暗，语义清楚，不必禁用。
- `available()` 的含义是「读到过一份可用配色」，不是「探测到目录」，界面每帧调用
  不碰磁盘；目录在但文件坏了，卡片也不出现。
- 近黑主题除 `vantablack` 外还有 `last-horizon`（`#0c0b0c`，没有更暗的底）同样走
  「画布用原色、卡片提亮 6%」那条路。
- 查找命中衬底按 28% → 10% 逐档减淡，直到正文压在上面仍有 4.5:1（`everforest`
  的黄色混 28% 时只有 4.24）。
- 外壳直角（4.6）放到第二期，第一期 Omarchy 主题仍是默认圆角（第二期已做）。

在 Windows / macOS 上调色：设环境变量 `GONGWEN_OMARCHY_THEME_DIR` 指向一个含
`colors.toml` 的目录（可拷 `tests/fixtures/omarchy/<主题>.toml` 改名），目录的上一级
放 `theme.name`（如 `tokyo-night`）就能看到卡片副标题。

### 待真机验收

1. Omarchy 上启动，设置页「外观」出现「Omarchy 桌面」组，卡片副标题是当前主题名。
2. 点选后整个界面换色；`omarchy theme set` 换一套，再点卡片（或菜单项）跟上。
3. 明暗主题互切（如 Tokyo Night ↔ Rose Pine），纸面家族跟着换，导出不变。
4. 选着「跟随 Omarchy」时退出，删掉或改坏 `colors.toml` 再启动：状态栏说明、界面
   为 Claude 奶油；恢复文件再启动自动回来。

### 第二期（已完成，2026-10-29，待真机验收）

- **后台轮询**：`src/theme/omarchy/watch.rs`。`GongwenApp::new` 在启动换算之后调
  `omarchy::start_watcher`，启动时读到过 Omarchy 配色才起线程（没有 Omarchy 的机器
  不空转），重复调用只起一条。每秒 `stat` 一次，比对指纹：主题目录 `canonicalize`
  后的真实路径、`colors.toml` 的修改时间与长度、`theme.name` 的修改时间。指纹变了才
  `refresh()`，成功后 `ctx.request_repaint()`。
- **版本号**：`omarchy::generation()`，每成功换算一次加一。`GongwenApp` 记
  `omarchy_generation`，`ui` 每帧在 `sync_system_theme` 之后调
  `sync_omarchy_theme`：版本号变了且界面正跟随 Omarchy，就 `set_current` +
  `reconfigure_style`，状态栏说「Omarchy 换了主题，界面已跟上：「X」」；不写配置。
  `apply_theme` 自己重读后同步记下版本号，免得下一帧再套一遍、状态栏被改写。
- **中途缺文件**：读失败时缓存不动，也不记这次指纹，下一秒重试；界面停在上一套，
  不闪回 Claude 奶油。
- **直角外壳**：`theme::square_chrome()`（`src/theme/icons.rs`）= MDEX 两套或当前
  label 是「跟随 Omarchy」。原来所有 `is_mdex()` 调用点都换成它，`is_mdex()` 降为
  私有，只剩标题栏品牌图标一处用。Omarchy 主题的 label 固定是 `LABEL`，测试逐套核对。

与方案的出入：

- 不看 `theme.generation`：没核实到 Omarchy 会写这个文件，指纹改用上面三项，
  覆盖「换符号链接」「原地覆写 colors.toml」「只改主题名」三种换法。`gongwen.toml`
  覆盖还没做（第三期），做时把它的修改时间加进指纹。
- 直角外壳不只是圆角归零，而是整套沿用 MDEX 的平面外壳：浮层与窗口不投影、功能区
  用淡色工具带加强调色短标记、文档标签选中用淡强调底加底线、纸边减淡、对照分栏的
  拖动点。这些原来都挂在 `is_mdex()` 上，本来就是「平面工具区」这一个意思，与
  Hyprland 平铺窗口的观感一致，没有逐项拆开。
- Omarchy 换主题时不重设窗口图标：「跟随 Omarchy」用的一直是 Claude 那张。

### 待真机验收（第二期）

1. 选着「跟随 Omarchy」，`omarchy theme set <另一套>`：一两秒内界面换色，状态栏说明
   新主题名；切换过程中不闪回 Claude 奶油。
2. 连切几套（含明暗互切），每次都跟上；纸面家族随明暗换。
3. 选着别的主题时 Omarchy 换主题：界面不动，设置页 Omarchy 卡片的副标题跟着变。
4. 外壳：圆角全部为直角，浮层无投影，功能区、文档标签是 MDEX 那种平面样式。

### 下一步：第三期

`gongwen.toml` 覆盖（加进指纹）、23 套色卡拼板样张、使用帮助「外观」与 README。
