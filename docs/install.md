# 安装指南

## 获取发布版

GitHub Releases 提供 Windows x64 安装程序（`gongwen-assistant-<版本>-win-x64-setup.exe`）、Linux ARM64/AMD64 Debian 包、面向 Arch/Omarchy 的 ARM64/AMD64 Wayland 运行包与 `PKGBUILD`，以及 macOS Apple Silicon 的 DMG。各平台发布包都内含应用、Tectonic、离线 bundle 与字体，安装后即可离线导出 Markdown、DOCX、TeX 并编译 PDF。

macOS 安装：打开 DMG，把「公文助手」拖进 Applications。应用未做开发者签名与公证，首次打开需右键点图标选「打开」，并在系统弹窗中确认；要求 macOS 12 及以上、Apple Silicon。

发布流水线从 `gongwen-runtime` 仓库下载平台 runtime，与 `scripts/package-portable.ps1` 一起组装为完整安装包：Windows 用 Inno Setup 打成安装程序，Debian Linux 用 `scripts/package-deb.sh` 打成 deb（安装到 `/opt/gongwen-assistant`），Arch/Omarchy 用 Portal 文件选择器重新构建并生成运行包与带校验和的 `PKGBUILD`，macOS 用 `scripts/package-dmg.sh` 打成拖拽安装的 DMG。相关资产受授权和体积限制，不进入源码仓库。

## 在 Omarchy / Arch Linux 安装

从同一 GitHub Release 下载生成的 `PKGBUILD`，在它所在的空目录中运行：

```bash
makepkg -si
```

`makepkg` 会按当前架构下载 `linux-amd64-omarchy` 或 `linux-arm64-omarchy` 运行包，并由 Pacman 安装依赖、桌面入口和图标。此版本显式使用原生 Wayland、XDG Desktop Portal 与稳定的 `cn.localtools.GongwenAssistant` app id；在 Hyprland 平铺会话中会自动使用紧凑窗口控件。

从源码构建相同变体时使用：

```bash
cargo build --release --locked --no-default-features --features linux-portal-dialogs
```

文件选择依赖 `xdg-desktop-portal-gtk`；应用内直接打印还需要可提供 `lp` 命令的 CUPS
（Windows 与 macOS 无此依赖：Windows 由应用自行光栅化页面走原生 GDI 打印，macOS 自带 `lp`）。

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
- 可选：本地模型服务（LM Studio 或 Ollama），以及本机 XeLaTeX/Tectonic。

```powershell
cargo run
```

首次使用：在“设置”中配置本地模型服务接口并选择模型；在“标准词库”维护单位与人员；在“起草”页选择文种、填写要素并生成草稿；审校后导出。
