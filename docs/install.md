# 安装指南

## 获取发布版

GitHub Releases 提供三种发布包：Windows x64 安装程序（`gongwen-assistant-<版本>-win-x64-setup.exe`）、Linux ARM64 Debian 包（`gongwen-assistant_<版本>_arm64.deb`，按 GLIBC 2.28 构建，兼容 Ubuntu 20.04 / 麒麟 V10 及更新的系统）与 Linux x86_64 的 Arch / Omarchy 包（`gongwen-assistant-<版本>-1-x86_64.pkg.tar.zst`，用 `pacman -U` 离线安装）。macOS 与 Linux AMD64 的 deb 不再提供，需要时请按下文从源码构建。三种发布包都内含应用与字体（PDF 由程序内置的 Typst 引擎排版），安装后即可离线导出 Markdown、DOCX 与 PDF。

安装后稿件包（`.gwx`）已与本程序关联：双击直接打开导入流程，Linux 装了 deb 或 tar
包也会注册对应的 MIME 类型与文档图标。程序已开着时，双击的稿件包交给已开的窗口
处理，不再另开一个窗口（需要独立窗口时用启动参数 `--new-window`）。

发布流水线从 `gongwen-runtime` 仓库下载平台 runtime，与 `scripts/package-portable.ps1`（Linux 用 `scripts/stage-linux-portable.sh`）一起组装为完整安装包：Windows 用 Inno Setup 打成安装程序，Debian Linux 用 `scripts/package-deb.sh` 打成 deb（安装到 `/opt/gongwen-assistant`），Arch / Omarchy 用 `scripts/package-arch-pkg.sh` 打成 `.pkg.tar.zst`（安装到 `/usr/lib/gongwen-assistant`，`pacman -U` 离线安装）。相关资产受授权和体积限制，不进入源码仓库。

本地打 Windows 安装包（`scripts/package-dev.ps1` / `scripts/package-installer.ps1`）需要
Inno Setup **6.3 或更新**：`.iss` 与 `scripts/ChineseSimplified.isl` 都按 6.7.3 校对过，
用的是 6.3 引入的 `x64compatible` 架构标识、且不再包含 6.5 前才有的 `FileExists` /
`ExistingFileNewer` 消息（旧版编译时会给“架构标识废弃”“消息名不认识”的告警）。

## 在 Omarchy / Arch Linux 安装

Release 提供 x86_64 的 Arch 软件包（Omarchy 4 的 amd64 机器直接用）。下载后离线安装：

```bash
pacman -U ./gongwen-assistant-<版本>-1-x86_64.pkg.tar.zst
```

包里已经带好应用与字体（PDF 由内置 Typst 引擎排版），安装后不联网也能导出 Markdown、DOCX 与 PDF；菜单项、图标与稿件包 `.gwx` 的 MIME 类型一并登记。该变体显式使用原生 Wayland、XDG Desktop Portal 与稳定的 `cn.localtools.GongwenAssistant` app id；在 Hyprland 平铺会话中会自动使用紧凑窗口控件。

要从源码构建相同变体时使用：

```bash
cargo build --release --locked --no-default-features --features linux-portal-dialogs
```

文件选择依赖 `xdg-desktop-portal-gtk`；应用内直接打印还需要可提供 `lp` 命令的 CUPS
（Windows 与 macOS 无此依赖）。三平台用同一个应用内打印对话框：Windows 由应用自行
光栅化页面走原生 GDI 打印（DEVMODE），macOS 自带 `lp`。

### Wayland 下的「应用无响应」系统弹窗

Hyprland 等 Wayland 合成器每 1.5 秒给客户端发一次存活探测，客户端的事件循环停摆
超过阈值就会被判为「应用无响应」，弹出系统对话框让人选「终止」还是「等待」。本应用
是原生 Wayland 客户端，文件选择框因此**开在后台线程**（`src/app/file_dialog.rs`）：
用户挑文件时主线程照常画界面、照常回应探测，挑完再接着做导入或导出。

早期版本用阻塞式文件框，主线程在用户挑文件期间整个停住，探测没人回应，于是弹出
「应用无响应」——应用本身其实照常可用。若仍看到该弹窗，说明主线程被别的事占住了
（例如超大文档的同步排版），不是文件框的问题。

### 输入法（fcitx5）候选窗定位

本应用是原生 Wayland 客户端，候选窗由 compositor 按 `input-method-v2` 协议绘制，
位置取决于应用上报的"光标区域"。上游 egui-winit 把整个编辑框当成光标区域上报，
在 Hyprland 下会让候选窗贴在编辑框底部不动；全屏时该区域几乎等于整块屏幕，
Hyprland 的下沿越界判定会把候选窗翻到光标上方的负坐标处，即屏幕之外——表现为
「看不见候选框但照样能打字、能选词」（参见 [hyprwm/Hyprland#5399][hypr-5399]、
[#8773][hypr-8773]）。`src/ime/cursor.rs` 把上报区域收窄成真正的光标矩形来规避。

候选窗位置仍然不对时，用调试开关打印实际上报的坐标：

```bash
GONGWEN_IME_DEBUG=1 gongwen-assistant
```

每次光标区域变化会向 stderr 打一行 `[ime] cursor=(x,y wxh) window=… ppp=… fullscreen=…`
（egui 点，原点在窗口左上角）。坐标落在窗口范围内即说明应用侧上报正常，问题在
compositor 或 fcitx5 一侧。此时可以绕开：把该显示器缩放改成 1 以外的值
（如 `monitor = ,preferred,auto,1.2`），或临时用 XWayland 启动
（`env -u WAYLAND_DISPLAY XMODIFIERS=@im=fcitx gongwen-assistant`），走 XIM 路径由
fcitx5 自己画候选窗。LibreOffice 之类不受影响的程序，是因为它们经 `GTK_IM_MODULE` /
`QT_IM_MODULE` 直连 fcitx5，绕过了 compositor 的弹窗路径；egui/winit 没有对应的旁路。

[hypr-5399]: https://github.com/hyprwm/Hyprland/issues/5399
[hypr-8773]: https://github.com/hyprwm/Hyprland/issues/8773

## 从源码构建

前置条件：

- Rust stable（edition 2024）；
- 可选：本地模型服务（LM Studio 或 Ollama）。

```powershell
cargo run
```

首次使用：在“设置”中配置本地模型服务接口并选择模型；在“标准词库”维护单位与人员；在“起草”页选择文种、填写要素并生成草稿；审校后导出。
