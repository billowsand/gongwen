# 公文助手

一个基于 Rust、egui 与本地模型服务（LM Studio / Ollama 等 OpenAI 兼容服务）的桌面公文写作应用。应用把行文要素、标准词库和模型起草分离：单位、人员、编号、日期等要素由界面维护，模型只生成 Markdown 正文，随后由本地导出器统一排版并输出 Word、TeX 和 PDF。

## 主要功能

- 支持函件、电话通知、普通公文、会议议程、白头件、红头呈批件等常用文种；
- 行文要素由界面锁定，模型只负责起草正文；
- 标准词库以树形结构管理单位、人员和联系方式，支持批量导入、增量合并与导出；
- 支持本地 OpenAI 兼容接口（LM Studio / Ollama 等），并可选用知识库检索（RAG）辅助起草；
- 提供可编辑 Markdown 审校稿、实时排版、公文预览和版本对照，支持插入图片（PNG/JPG/WebP/BMP/GIF/PDF），预览按页面宽度排版、导出随文档嵌入；
- 起草页按「开始、插入、格式、审校、视图、输出」六个分区组织功能区，插入表格（网格选行列、行列增删、列对齐）、标题层级、加粗、词库词条、中文日期等都在其中；保存、提交版本、导出常驻标题栏；
- 支持密级与保密期限、发文编号、单位落款等字段及规则校验；
- 可导出 Markdown、DOCX、TeX，并使用本地 Tectonic/XeLaTeX 编译 PDF；
- 导出的 PDF 与稿件 PDF 附件可在应用标签内直接预览，使用纯 Rust Hayro 渲染，支持连续滚动、跳页、缩放和适合宽度，只光栅化视口附近的页，并保留系统程序打开作为兼容入口；
- 界面字体不再随应用打包，Windows 默认为微软雅黑、Linux 默认为 Noto Sans SC，也可在设置中选择其他本机字体；
- 编译字体默认用内置的一套，也可在设置中改用本机字体，标题、一级标题、二级标题、正文、页码各自单选；
- 内置稿件库，支持草稿、发布、归档，以及版本历史、附件和 ZIP 导出/导入；
- 默认连接本机模型服务（LM Studio / Ollama 等），配置、词库与稿件均保存在本机。

## 设计思路

模型只负责起草 Markdown，固定字段、版式规则和导出结果由本地程序控制，避免模型改写关键信息或导致版式漂移。模型可以替换，而规则、词库和版式保持一致。

## 快速开始

### 获取发布版

GitHub Releases 提供 Windows x64 安装程序（`gongwen-assistant-<版本>-win-x64-setup.exe`）、Linux ARM64/AMD64 Debian 包、面向 Arch/Omarchy 的 ARM64/AMD64 Wayland 运行包与 `PKGBUILD`，以及 macOS Apple Silicon 的 DMG。各平台发布包都内含应用、Tectonic、离线 bundle 与字体，安装后即可离线导出 Markdown、DOCX、TeX 并编译 PDF。

macOS 安装：打开 DMG，把「公文助手」拖进 Applications。应用未做开发者签名与公证，首次打开需右键点图标选「打开」，并在系统弹窗中确认；要求 macOS 12 及以上、Apple Silicon。

发布流水线从 `gongwen-runtime` 仓库下载平台 runtime，与 `scripts/package-portable.ps1` 一起组装为完整安装包：Windows 用 Inno Setup 打成安装程序，Debian Linux 用 `scripts/package-deb.sh` 打成 deb（安装到 `/opt/gongwen-assistant`），Arch/Omarchy 用 Portal 文件选择器重新构建并生成运行包与带校验和的 `PKGBUILD`，macOS 用 `scripts/package-dmg.sh` 打成拖拽安装的 DMG。相关资产受授权和体积限制，不进入源码仓库。

### 在 Omarchy / Arch Linux 安装

从同一 GitHub Release 下载生成的 `PKGBUILD`，在它所在的空目录中运行：

```bash
makepkg -si
```

`makepkg` 会按当前架构下载 `linux-amd64-omarchy` 或 `linux-arm64-omarchy` 运行包，并由 Pacman 安装依赖、桌面入口和图标。此版本显式使用原生 Wayland、XDG Desktop Portal 与稳定的 `cn.localtools.GongwenAssistant` app id；在 Hyprland 平铺会话中会自动使用紧凑窗口控件。

从源码构建相同变体时使用：

```bash
cargo build --release --locked --no-default-features --features linux-portal-dialogs
```

文件选择依赖 `xdg-desktop-portal-gtk`；应用内直接打印还需要可提供 `lp` 命令的 CUPS。

### 从源码构建

前置条件：

- Rust stable（edition 2024）；
- 可选：本地模型服务（LM Studio 或 Ollama），以及本机 XeLaTeX/Tectonic。

```powershell
cargo run
```

首次使用：在“设置”中配置本地模型服务接口并选择模型；在“标准词库”维护单位与人员；在“起草”页选择文种、填写要素并生成草稿；审校后导出。

### 使用 Ollama

本应用通过 OpenAI 兼容协议接入本地模型服务，因此同样支持 Ollama：

1. 启动 Ollama 服务（`ollama serve`，默认端口 11434）并拉取所需模型；
2. 在“设置 → 本地模型服务设置”中，把对话、Embedding、Rerank 三个「接口地址」都改为 `http://127.0.0.1:11434/v1`（三个配置相互独立）；
3. 模型名填 Ollama 的模型 tag，例如对话模型 `qwen2.5:7b`、Embedding 模型 `bge-m3` 或 `nomic-embed-text`；
4. 重排方式选「用对话大模型重排」：Ollama 目前不提供 rerank 专用端点，无法使用「专用端点」模式，改用对话模型打分同样可用。

### 使用本机字体编译

排版默认使用随应用分发的内置字体：标题方正小标宋、一级标题黑体、二级标题楷体、正文仿宋、页码宋体。要换成本机装的字体，在“设置 → 编译字体”勾选「使用本机字体编译」，再给五个位置分别选字体，保存后下次导出即生效；某一项留作「内置」就继续用内置字体。屏幕上的公文预览同步切换，所见即所得。

内置 Tectonic 编译时按文件加载所选字体，导出的 `.tex` 拿到别的机器上编译时按字体名加载——目标机器上装了同名字体才能得到一致效果。字体文件后来被删除或卸载时会自动退回内置字体，并在导出结果里给出提示。

列表只收录 `.ttf` 与 `.otf`。字体集合（`.ttc`，例如 `simsun.ttc`、`msyh.ttc`）一个文件里装着多个字面，按文件加载必须额外指定字面序号，未在内置 Tectonic 上验证过，因此不可选；这类字体可另找单字面的 `.ttf` 版本。二级标题字体同时充当正文的斜体字面，改它会一并影响正文里的斜体。

词库模板可在“标准词库”页点击「下载空白模板」得到 `.xlsx` 空白模板（仅表头 + 数据验证），填好后用「导入 Excel」按编码优先合并；也可「导出 Excel」备份当前词库。示例配置见 `config.example.json`。运行时配置默认保存在用户配置目录的 `LocalTools/GongwenAssistant/config/config.json`，稿件库为同目录的 `manuscripts.db`。

导出稿件 ZIP 时会随附当前完整标准词库（`vocabulary.json`，含单位、人员与联系方式），拿到另一台电脑导入时，预览页会显示包内词库规模并默认勾选「合并包内标准词库到本机」——按层级编码/姓名增量合并，不覆盖目标机器已有的词条；取消勾选则只导入稿件。旧版稿件包不带词库，导入不受影响。

## 项目结构

```text
src/                应用代码
assets/             图标与界面资源
examples/           示例公文
scripts/            版本号更新与便携包构建脚本
runtime/            便携 TeX 运行时与字体（不进入 Git）
gonghan-gwa.cls     LaTeX 文档类
config.example.json 示例配置
```

## 开发与验证

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo check
```

GitHub Actions：

- `.github/workflows/ci.yml`：在 Windows、Linux、macOS 上执行格式、静态检查和测试；
- `.github/workflows/release.yml`：推送 `v*` 标签后构建各平台安装包（Windows setup.exe、Linux deb、macOS DMG）与 SHA256 校验和并创建 GitHub Release。

版本号只维护在 `Cargo.toml`，使用 `scripts/bump-version.ps1 -Part major|minor|patch` 同步更新 `Cargo.lock`。

## 许可

本项目以 [MIT](LICENSE) 协议开源。第三方组件与随发布包分发的字体许可见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
