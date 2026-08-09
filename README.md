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
- 内置稿件库，支持草稿、发布、归档，以及版本历史、附件和 ZIP 导出/导入；
- 默认连接本机 LM Studio，配置、词库与稿件均保存在本机。

## 设计思路

模型只负责起草 Markdown，固定字段、版式规则和导出结果由本地程序控制，避免模型改写关键信息或导致版式漂移。模型可以替换，而规则、词库和版式保持一致。

## 快速开始

### 获取发布版

GitHub Releases 提供 Windows x64、Linux x64 与 macOS ARM64 的源码构建二进制，压缩包内含可执行文件、README、许可与示例配置。这些二进制不包含 Tectonic、TeX bundle 与字体；使用 TeX/PDF 导出时可调用本机 XeLaTeX/Tectonic，Markdown 与 Word 导出不依赖 TeX。

完整 Windows 便携包由维护者通过 `scripts/package-portable.ps1` 构建并附加到 Release，包含应用、Tectonic、TeX bundle 与字体。相关资产受授权和体积限制，不进入源码仓库。

### 从源码构建

前置条件：

- Rust stable（edition 2024）；
- 可选：LM Studio，以及本机 XeLaTeX/Tectonic。

```powershell
cargo run
```

首次使用：在“设置”中配置 LM Studio 接口并选择模型；在“标准词库”维护单位与人员；在“起草”页选择文种、填写要素并生成草稿；审校后导出。

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
