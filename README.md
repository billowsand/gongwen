# 公文助手

一个基于 Rust、egui 与本地 LM Studio 的桌面公文写作应用。应用把行文要素、标准词库和模型起草分离：单位、人员、编号、日期等要素由界面维护，模型只生成 Markdown 正文，随后由本地导出器统一排版并输出 Word、TeX 和 PDF。

## 主要功能

- 支持函件、电话通知、普通公文、会议议程、白头件等常用文种；
- 行文要素由界面锁定，模型只负责起草正文；
- 标准词库以树形结构管理单位、人员和联系方式，支持批量导入、增量合并与导出；
- 支持本地 LM Studio OpenAI 兼容接口，并可选用知识库检索（RAG）辅助起草；
- 提供可编辑 Markdown 审校稿、实时排版、公文预览和版本对照；
- 支持密级与保密期限、发文编号、单位落款等字段及规则校验；
- 可导出 Markdown、DOCX、TeX，并使用本地 Tectonic/XeLaTeX 编译 PDF；
- 编译字体默认用内置的一套，也可在设置中改用本机字体，标题、一级标题、二级标题、正文、页码各自单选；
- 内置稿件库，支持草稿、发布、归档，以及版本历史、附件和 ZIP 导出/导入；
- 默认连接本机 LM Studio，配置、词库与稿件均保存在本机。

## 设计思路

模型只负责起草 Markdown，固定字段、版式规则和导出结果由本地程序控制，避免模型改写关键信息或导致版式漂移。模型可以替换，而规则、词库和版式保持一致。

## 快速开始

### 获取发布版

GitHub Releases 提供 Windows x64 与 Linux ARM64 的源码构建二进制，压缩包内含可执行文件、README、许可与示例配置。这些精简包不包含 Tectonic、TeX bundle 与字体；使用 TeX/PDF 导出时可调用本机 XeLaTeX/Tectonic，Markdown 与 Word 导出不依赖 TeX。

完整便携包同样随 Release 发布（文件名带 `-full`），由发布流水线从 `gongwen-runtime` 仓库下载平台 runtime，与 `scripts/package-portable.ps1` 一起打包应用、Tectonic、离线 bundle 与字体。Windows x64 输出 zip，Linux ARM64 输出 tar.gz。相关资产受授权和体积限制，不进入源码仓库。

### 从源码构建

前置条件：

- Rust stable（edition 2024）；
- 可选：LM Studio，以及本机 XeLaTeX/Tectonic。

```powershell
cargo run
```

首次使用：在“设置”中配置 LM Studio 接口并选择模型；在“标准词库”维护单位与人员；在“起草”页选择文种、填写要素并生成草稿；审校后导出。

### 使用本机字体编译

排版默认使用随应用分发的内置字体：标题方正小标宋、一级标题黑体、二级标题楷体、正文仿宋、页码宋体。要换成本机装的字体，在“设置 → 编译字体”勾选「使用本机字体编译」，再给五个位置分别选字体，保存后下次导出即生效；某一项留作「内置」就继续用内置字体。屏幕上的公文预览同步切换，所见即所得。

内置 Tectonic 编译时按文件加载所选字体，导出的 `.tex` 拿到别的机器上编译时按字体名加载——目标机器上装了同名字体才能得到一致效果。字体文件后来被删除或卸载时会自动退回内置字体，并在导出结果里给出提示。

列表只收录 `.ttf` 与 `.otf`。字体集合（`.ttc`，例如 `simsun.ttc`、`msyh.ttc`）一个文件里装着多个字面，按文件加载必须额外指定字面序号，未在内置 Tectonic 上验证过，因此不可选；这类字体可另找单字面的 `.ttf` 版本。二级标题字体同时充当正文的斜体字面，改它会一并影响正文里的斜体。

词库模板可在“标准词库”页点击「下载空白模板」得到 `.xlsx` 空白模板（仅表头 + 数据验证），填好后用「导入 Excel」按编码优先合并；也可「导出 Excel」备份当前词库。示例配置见 `config.example.json`。运行时配置默认保存在用户配置目录的 `LocalTools/GongwenAssistant/config/config.json`，稿件库为同目录的 `manuscripts.db`。

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
- `.github/workflows/release.yml`：推送 `v*` 标签后构建各平台 Release，生成 ZIP 与 SHA256 校验和并创建 GitHub Release。

版本号只维护在 `Cargo.toml`，使用 `scripts/bump-version.ps1 -Part major|minor|patch` 同步更新 `Cargo.lock`。

## 许可

本项目以 [MIT](LICENSE) 协议开源。第三方组件与随发布包分发的字体许可见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
