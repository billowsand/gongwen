# 公文助手：未引入 Rust 库调研

调研日期：2026-09-30。基于本地 0.6.12 的 Cargo.toml、Cargo.lock 与相关实现，结合上游文档、仓库和发布记录。结论是项目适配判断，不是性能实测；本次未安装候选依赖、未修改应用代码。

## 筛选范围

同时排除直接依赖和 Cargo.lock 中已有的间接依赖。因此不重复调研 lopdf、pulldown-cmark、unicode-segmentation、unicode-normalization、rayon、tracing、zeroize 等已经进入依赖树的库。

项目已经具备 jieba + SQLite FTS5 关键词检索、向量与关键词 RRF 融合、本地服务重排、公文专用 Markdown 切块、保守三方合并，以及 PDF、DOCX、公式、Mermaid 导出预览。候选库必须说明增加什么能力，或替代哪段维护成本较高的实现。

尤其考虑 Windows x64 与 Linux ARM64 / GLIBC 2.28 发布要求。涉及本地推理的候选，还需要单独验证模型文件、动态库、CPU 指令集、包体积和完全断网运行。

## 建议排序

| 候选 | 项目用途 | 建议 | 接入成本估计 |
| --- | --- | --- | --- |
| nucleo-matcher | 稿件、词表、单位人员、命令的快捷模糊查找 | 优先做用户可见的小功能 | 低至中 |
| atomic-write-file | 配置等独立文件的原子替换 | 优先评估，也可用已有 tempfile 完成 | 低 |
| proptest | 中文偏移、修订锚点、合并、切块边界测试 | 优先用于核心纯函数 | 低至中，仅开发依赖 |
| insta | 解析结果、导出中间文本、合并提案快照 | 推荐有选择地使用 | 低，仅开发依赖 |
| egui_kittest | 修订采纳、归档冻结、冲突选择的界面行为测试 | 推荐固定 0.35 系列试点 | 中，仅开发依赖 |
| keyring | 用户选择记住的 ZIP 密码、模型服务密钥 | 推荐按平台接入 | 中 |
| fastembed | 无需外部服务的进程内中文 embedding / rerank | 值得做独立原型 | 高 |
| PaddleOCR 的 Rust 实现 + ort | 扫描 PDF、图片取字 | 业务价值高，先验证发布平台 | 高 |
| zhconv | 知识库检索中的简繁归一 | 有相应资料需求时接入 | 中 |
| notify-debouncer-full | 用户指定知识库目录的变化检测 | 确定要做目录同步时接入 | 中 |
| sqlite-vec | 把向量查询下推 SQLite | 等规模和测量证明收益 | 中至高 |
| rusqlite_migration | 集中管理数据库升级 | 暂不为引库重写已有迁移 | 中至高 |

接入成本包含业务集成和验证，不是安装 crate 的时间。

## 优先候选

### 1. nucleo-matcher：最适合做快捷检索

上游提供 Unicode 友好的模糊匹配与评分；核心 matcher 已用于 Helix，上游称核心实现完成度较高。只需要匹配算法时，官方建议选 nucleo-matcher，而不是带完整调度机制的 nucleo。许可为 MPL-2.0。[上游说明](https://github.com/helix-editor/nucleo)

可用于“快速打开稿件”、词表条目选择、单位人员候选、命令面板。建议生成三种检索键：原始中文、已有 pinyin 生成的全拼、拼音首字母，再由程序合并排名。这是本项目的集成方案，库本身不负责汉字转拼音。

它不理解中文语义，也不是错别字纠正器，不替换知识库 FTS5。匹配高亮位置要转换到项目使用的字节范围，不能直接把上游返回的索引当作 UTF-8 字节偏移。

最小验证：用中文标题、拼音、首字母、混合文号建立小样本，看排序与高亮是否符合预期，再测一万条候选的响应时间。

### 2. atomic-write-file：有明确的现有落点

上游 0.3.1 支持 Unix、Windows、WASI 的原子写入/覆盖，提交前保留原文件，并提供 Linux 崩溃测试。许可 BSD-3-Clause；2026-08 发布过更新。[官方文档](https://docs.rs/atomic-write-file/0.3.1/atomic_write_file/)、[仓库](https://github.com/andreacorbellini/rust-atomic-write-file)

本地 src/storage.rs 的 save 和 save_remembered_zip_password 都存在“先写临时文件，删除旧文件，再改名”的过程。如果删除后改名失败，旧路径就不存在了。该库可以集中处理独立文件替换，避免多个保存入口重复写平台细节。

不过，项目已经有 tempfile，稿件 ZIP 导出也已经使用临时文件 + persist。应先比较复用现有依赖的方案；要修的是保存语义，不一定要新增库。不用它替代 SQLite 的事务机制；私密文件权限、父目录持久化和断电保证仍需按平台核对。

最小验证：写入失败时旧配置可读；成功时完整替换；Windows 已有文件替换、文件占用、中文路径与权限均覆盖。

### 3. proptest：中文编辑和离线同步很契合

上游支持生成随机输入，并把失败自动缩减成较小的复现案例。许可 MIT / Apache-2.0；上游说明架构已经较稳定，目前主要是维护。[仓库](https://github.com/proptest-rs/proptest)

比给界面小改动堆测试更值得用在核心不变量上：

- 修订 span 必须落在合法 UTF-8 边界，失效锚点不得误采纳。
- 插入、删除、撤销后，正文必须精确还原。
- 三方合并中，一侧等于基线时保留另一侧；两侧完全相同不得制造冲突。
- 切块必须终止，中文、公式、表格与附件不能造成越界。

项目已有大量手写测试；该库补的是输入组合的空白，不要求替换已有样例。只放 dev-dependencies，不增加发布包运行依赖。

### 4. insta：锁住复杂输出的变化

上游提供可审阅的文本/结构快照，许可 Apache-2.0。[仓库](https://github.com/mitsuhiko/insta)

适用于公文 Markdown 解析后的结构、TeX 输出、稳定化后的 DOCX XML、版本差异与合并提案。固定 UUID、时间戳、路径等变化字段，避免每次运行都更新快照。

不建议直接对整个 DOCX ZIP 或 PDF 二进制做快照；也不能以“更新快照”代替检查编号、版式和要素是否正确。图像与分页仍需专门核对。

### 5. egui_kittest：补界面行为验证

官方 0.35.0 提供基于 AccessKit 的界面查询与模拟操作，可与本项目 egui 0.35 对齐；MIT / Apache-2.0。图像快照需要开启 wgpu 与 snapshot 功能。[0.35.0 文档](https://docs.rs/egui_kittest/0.35.0/egui_kittest/)

建议先做不依赖截图的行为测试：归档稿编辑入口不可用；冲突未处理时不能提交；建议采纳、撤销后正文和按钮状态一致。项目自绘的纸面和输入法候选可能需要补充可访问性标签。

先拆出可独立运行的小面板。截图测试的渲染后端与当前 glow 不同，不能视为正式导出效果的证明；跨平台字体差异也会增加维护成本。

### 6. keyring：记住密码的自然落点

Keyring 生态提供 Windows、macOS、Linux 原生凭据存储和 Secret Service 接口；上游目前有 keyring-core 与不同 store 的模块化设计，不能直接照搬旧版 feature 配置。许可 MIT / Apache-2.0。[仓库](https://github.com/open-source-cooperative/keyring-rs)、[生态说明](https://github.com/open-source-cooperative/keyring-rs/wiki/Keyring)

本地当前把用户选择记住的 ZIP 密码放到配置目录下独立文件。可改成凭据存储，并为模型服务密钥使用同样机制，配置只保存引用标识。

Linux 桌面不一定有已解锁的 Secret Service。不可用时应允许本次输入或不记住，避免阻塞起草与导出。凭据不跟稿件 ZIP 同步；迁移旧密码应在新存储写入并回读成功后处理旧文件。

## 值得做原型的功能扩展

### 7. fastembed：把知识库向量与重排放到进程内

上游支持同步调用、中文 bge-small-zh 等 embedding 与重排模型，使用 ort / ONNX 推理。许可 Apache-2.0。[仓库和模型清单](https://github.com/anush008/fastembed-rs)

这是外部 LM Studio / Ollama embedding、rerank 接口的可选替代后端，可以让用户不启动模型服务也能用语义知识库。保留 FTS5 和原有 RRF，不把起草模型一起搬进来。

成本主要在模型分发、内存、推理库和两平台打包。必须选中文模型，不能沿用默认英文模型；离线版要能显式加载本地资源，不能首次运行再下载。更换模型或向量维度必须重建索引并记录模型身份。

原型应先测真实公文问句与现有服务的召回质量，再测 CPU 延迟和内存。Linux ARM64 / GLIBC 2.28 是上线前的关键验证，本次未确认其二进制兼容。

### 8. PaddleOCR Rust 实现 + ort：扫描件取字

可先考察 [PaddleOCR-rs](https://github.com/Craun718/PaddleOCR-rs)（crate 示例为 paddleocr_rs_onnx，MIT）和 [paddle-ocr-rs / rapid-ocr-rs](https://github.com/mg-chao/paddle-ocr-rs)（Apache-2.0），后端参考 [ort](https://github.com/pykeio/ort)。这些是较小的第三方集成项目，不能把上游声明的功能当作本项目平台验证结果。

建议复用已有 PDF 光栅化入口，将扫描页变成图片，识别后作为带来源页码的待核对材料进入知识库。归档盖章 PDF 保留原件；识别文字不能自动修改正文或行文要素，事实单仍由用户逐项确认。

先只做印刷体中文和段落文字，不把手写批示、印章、表格结构还原列入第一版。用红头件、倾斜页、低清复印件和带章页面评估，尤其检查文号、日期和人名。

ort 默认路线涉及 ONNX Runtime 原生运行库，不能因包装器用 Rust 编写就假定是纯 Rust、零运行时或可直接通过 GLIBC 2.28 发布。模型权重、字符表与库许可证分别记录。

对比后排除 [ocrs](https://github.com/robertknight/ocrs)：官方目前说明只识别拉丁字母，并处于早期预览，不能承担本项目中文 OCR。

### 9. zhconv：检索中的简繁归一

支持简繁和地区用词变体。库代码 MIT / Apache-2.0，默认 MediaWiki 表为 GPL-2.0-or-later，可选 OpenCC 词典为 Apache-2.0，分发说明应区分代码与词表。[上游说明](https://github.com/Gowee/zhconv-rs)

建议用于知识库检索键或导入预览；索引可以增加简体副本，仍保存原文。不要自动转换单位、人名、引用原文或正在编辑的正文。转换可能改变长度，归一化副本的命中偏移不能直接回指原文。

若实际资料都是简体，收益有限，不需要先引入。

### 10. notify-debouncer-full：材料目录变化检测

上游支持跨平台文件变化检测与事件去抖；notify-debouncer-full 为 MIT / Apache-2.0。发布记录同时存在稳定和预发布分支，集成时应明确选择。[官方说明](https://github.com/notify-rs/notify/blob/main/README.md)、[发布记录](https://github.com/notify-rs/notify/releases)

仅在增加“用户指定目录作为知识库材料源”时有明确价值：目录变化后标记待更新，按内容哈希确认，更新受影响资料。不要监视用户整个文件系统，也不要让通知直接覆盖稿件正文。

网络盘、原子保存导致的改名、多次重复通知、遗漏事件都要处理。保留启动全量核对和手动刷新，文件事件只作为触发信号。

## 暂缓或仅借鉴

### 11. sqlite-vec：规模变大后再决定

提供 Rust 接入的 SQLite 向量扩展，核心是无额外依赖的 C，实现 vec0 与 float / int8 / binary 向量查询。官方明确为 pre-v1，可能破坏兼容。[仓库](https://github.com/asg017/sqlite-vec)、[Rust 示例](https://github.com/asg017/sqlite-vec/blob/main/examples/simple-rust/demo.rs)

本地 rag::retrieve 当前取出全部向量，在 Rust 中做余弦排序；数据量增大时，可以比较把 top-k 查询下推数据库的收益。sqlite-vec 不是“引入后自动获得 HNSW”的方案，不能把收益按近似索引估算。

先测一万、五万、十万块的耗时和峰值内存，检查文种过滤后的 top-k 是否与现状相符。现有 blob 与虚拟表迁移、静态注册和两平台构建都增加维护成本；小库不必改。

### 12. rusqlite_migration：版本匹配，但迁移不是空白

核实到 2.6.0 依赖 rusqlite ^0.40.0，与本项目版本对齐；用 user_version 跟踪升级，并支持迁移验证。[依赖和发布信息](https://docs.rs/crate/rusqlite_migration/2.6.0)、[文档](https://docs.rs/rusqlite_migration/2.6.0/rusqlite_migration/)

项目已自行维护 user_version，离线同步迁移把它升级到 8，同时还有幂等建表/补列逻辑。因此不能简单追加此库来接管同一个字段。要使用，必须先整理旧版本到当前版本的准确映射以及中途升级失败的行为。

当前建议借鉴其迁移组织与验证方法，除非以后确实要统一迁移体系，否则不做替换。

### 另外考察的方案

- [text-splitter](https://docs.rs/text-splitter/latest/text_splitter/)：通用 Markdown 语义切块适合普通外部材料，但不能直接替换现有公文切块；当前实现包含二级标题主边界、附件独立区段、长表头复写等业务规则。
- [diffy](https://docs.rs/diffy/latest/diffy/)：有文本三方合并和补丁，可做开发期对照或未来补丁导出。项目已有保守合并、要素成组、JSON 树冲突和 UUID 版本图，不建议为行级 merge 替换整个同步机制。
- [Ropey](https://github.com/cessen/ropey)：适合大文本编辑，MIT。但当前 TextEdit、光标、修订、diff 等大量围绕 String 和字节范围工作；只把 String 换成 Rope 再每帧转回字符串，未必有收益。先测长研究报告的真实瓶颈。
- [egui_tiles](https://docs.rs/crate/egui_tiles/0.17.1)：支持拖动、分栏和面板布局，值得借鉴。核实最新 0.17.1 依赖 egui ^0.36.0，而项目是 0.35，不能直接采用最新版。本次未确认旧版本的可用适配版本；即使对齐，也要评估自由布局是否真的比当前固定办文流程更易用。

## 落地建议

先做一轮小试点：nucleo-matcher 的快速查找、配置保存原子替换，以及 proptest / insta 覆盖核心纯函数。随后用 egui_kittest 验证几个不可出错的界面动作。

新业务能力优先选一个原型：扫描件取字，或进程内中文 embedding / rerank。两者都以 Windows x64、Linux ARM64 / GLIBC 2.28、断网运行的实测作为下一步是否引入的依据。

本次仅调研，不表示已经完成依赖解析、交叉编译、包体积测量、许可证清单落地或性能验收。
