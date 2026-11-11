# 公文助手双版本构建设计

## 编译交付目标

一次执行双版本构建命令即可在当前主机上交付 `gongwen-assistant`（AI 全版）和 `gongwen-assistant-no-ai`（非 AI 版）。Windows 文件名加 `.exe`。AI 全版沿用现有二进制名，供发布脚本直接使用。

在仓库根目录运行 `python3 scripts/build-editions.py`，Windows 使用 `py -3 scripts/build-editions.py`。默认输出到 `dist/editions/`，可用 `--output` 指定目录，Arch 加 `--linux-dialogs portal`。脚本对同一个 Cargo bin target 分别执行两次 `cargo build --locked --release`。两次编译串行复用同一 Cargo 缓存，产物先放入临时目录并通过 `--build-edition` 校验，两个结果都通过后才写入交付目录。

Cargo 特性对一次编译中的 crate 生效。两个 `[[bin]]` 条目无法让同一个 crate 在一次 `cargo build` 中分别启用和关闭 `ai`。这里由双版本构建命令统一编排，用户无需分别运行两条 Cargo 命令。

## 版本边界

| 功能 | AI 版 | 非 AI 版 |
| --- | --- | --- |
| 起草、预览、文种表单、稿件库与稿件搜索、版本对比、送批、导入导出、打印、输入法、应用锁 | 完整保留 | 完整保留 |
| 本地校对、校对词表、词库、格式与引用校验、审阅与撤销 | 完整保留 | 保留规则驱动的功能 |
| Pi 智能体、AI 侧栏与写作、旧 AI 会话和任务回放、模型服务商、AI 管理、风格学习、模型文字复核 | 完整保留 | 不编译 |
| 资料库与维基知识库，含资料入库、本地全文检索、向量索引、问答和维基作业 | 完整保留 | 整组不编译，稿件库自身搜索保留 |
| 任务助手 | 手工管理、AI 整理材料与填写表单 | 保留手工任务、进展记录、分类与归档，移除 AI 作业与入口 |
| 数据接口工作台 | 完整保留 | 首版随 AI 管理整组裁掉，含人工试跑 |
| 写作语法、帮助与示例 | 全版帮助与技能 | 使用独立的简明手册，保留文种示例，裁掉 AI 章节、截图与技能导出入口 |

非 AI 版可打开两版生成的普通稿件，已写入正文的文字和已提交的版本照常编辑、查看和导出。现有检查名称易混，非 AI 版界面统一用「规则校对」标示保留能力。

## Cargo 与源码边界

一个 crate、一份锁文件。`ai` 是全版的组合特性，目前由 `ai-workbench` 装配写作、模型复核、资料库及知识库等有共享依赖的 AI 功能。`ai-workbench` 依赖 `task-assistant`，后者也可独立编译手工任务，或从核心编辑器构建中省略。平台文件框特性独立选择。默认构建保留 AI 全版，Arch 的旧发布入口显式加回 `ai`。

| 构建 | Cargo 参数 |
| --- | --- |
| AI 版，Windows / macOS / Debian | `--no-default-features --features ai,linux-gtk-dialogs` |
| 非 AI 版，Windows / macOS / Debian | `--no-default-features --features task-assistant,linux-gtk-dialogs` |
| AI 版，Arch | `--no-default-features --features ai,linux-portal-dialogs` |
| 非 AI 版，Arch | `--no-default-features --features task-assistant,linux-portal-dialogs` |
| 核心编辑器（无任务助手），GTK 平台配置 | `--no-default-features --features linux-gtk-dialogs` |

GTK 与 portal 特性只影响 Linux，Windows 和 macOS 仍走各自的文件框。默认特性设为 `ai,linux-gtk-dialogs`，避免引入 Cargo 的负向 `no-ai` 特性。

1. 在 `src/main.rs` 按模块用 `#[cfg(feature = "ai")]` 裁掉 Pi、旧 `agent`、`ai_panel`、模型客户端、RAG、维基、资料库及模型复核。应用层一起裁掉导航、设置、快捷键、工作线程请求与结果、定时轮询和启动初始化。将 AI 状态放在现有应用域对应的子结构中，减少公共 `App` 的条件字段。
2. 先提取共用代码再关模块。词库用的 `knowledge::content_hash` 移至共用模块。表单的会议方案 Markdown 示例从 `prompt` 移出。`element_fields` 留公共表单能力，旧 AI 题目及采纳适配留在 AI 域。`proofread`、`proofread_rules`、`proofread_xlsx` 和 `revision` 的规则修订部分保留。任务助手手工存储和页面保留，Pi 作业、候选审核和资料库桥接按功能拆开。
3. 在配置、持久化与密钥管理边界同步拆分。`manuscript.rs` 中的知识库建表、AI 会话和维基初始化转给 AI 域，共用稿件迁移保留原有版本序列。检查旧稿件反序列化及关联资源清理，即使正文含资料库引用也应能打开。`vault.rs` 的 Pi 封存树调用跟随 `ai`，共用锁屏、真/诱饵槽、加密和恢复行为继续执行。
4. `serde_yaml` 的直接依赖跟随 `ai-workbench`，传递依赖以 `cargo tree -e features` 为准。配置对比通过 `url::Url` 解析网址，查更新使用的 `reqwest` 仍随两版编译。Typst、PDF 渲染、Office 导入和 SQLCipher 也属于共用底座。分别记录冷构建耗时、增量构建耗时、可执行文件和安装包体积。

## 安装包发布

独立二进制各用自己的数据和缓存目录。AI 版沿用原目录，非 AI 版使用 `GongwenAssistantNoAi`。单实例转发随目录隔离。打包脚本执行 `--build-edition` 校验输入二进制。两版共用字体、基础词表与许可证。AI 版正式包包含通过校验的 `runtime/pi` 和外部写作技能包，Windows 和 Linux 还附带模型示例配置。非 AI 版的包名加 `-no-ai`，安装标识、安装目录和桌面入口也各自独立。

Windows 保留 AI 版完整包和现有增量升级流程。非 AI 版通过完整安装包升级，并登记独立的 `.gwx` 打开方式。Debian 和 Arch 为两版分别设置包名、命令和桌面标识，由 AI 包登记共享的 `.gwx` MIME 类型。macOS 为非 AI 版使用 `GongwenAssistantNoAi.app` 和 `com.billowsand.gongwen.no-ai`。

打标签时，当前启用的 Windows 发布作业为两版分别生成安装程序和 SHA-256。macOS、Debian 和 Arch 的双版打包步骤也已配置，可按现有平台开关启用。

## 打包与验收

1. 打包脚本以显式版本参数驱动构建、暂存和最终清单。二进制提供无 GUI 的版本识别参数，脚本校验实际编译版本。`build.rs` 的内置技能资源登记跟随 `ai` 特性，外部 AI 使用的 `.skill` 也只随 AI 包生成。构建产物和暂存目录分开，防止旧 `runtime/pi` 混入。AI 包继续校验 Pi manifest、Node 与平台匹配。非 AI 包从无 Node/npm 和无 Pi 暂存的环境完成打包，递归核对无 Pi host、技能、模型配置模板及 Pi 专用许可证清单。共用字体、输入法基础表、Typst 与帮助仍按原要求交付。帮助通过编译条件选择章节和截图，并检查目录及跨章引用。
2. CI 分别编译、Clippy 和测试两种特性组合。共用测试跑两版，Pi 协议、真机作业及历史 AI 测试只跑 AI 版。Windows 安装包、macOS DMG、Linux ARM64 deb、Arch 包分别验证安装及启动。非 AI 版 GUI 冒烟覆盖起草、规则校对、文种导出、手工任务和应用锁，并核对菜单、设置、快捷键和帮助中的 AI 入口均缺席。
3. 用当前正式版数据验收 AI 原位升级，验证两版都能通过 `.gwx` 打开普通稿件。在同机同工具链记录两版冷构建与增量构建耗时、二进制和安装包体积，再决定是否继续优化共用依赖。

先解耦共用代码并验证 AI 版回归，再切出非 AI 版并接通双版本构建入口。一次构建交付两个可运行的二进制后，再接入安装包与跨平台发布。
