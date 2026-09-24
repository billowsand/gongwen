//! 视觉 diff 引擎：把两个版本展开成「印在纸上的字」再比较，产出附着在新版
//! 视觉块上的标注层 `RedlineOverlay`（方案见 docs/version-diff-redesign.md 第四节）。
//!
//! 与 `diff.rs` 的代码层 diff 各算各的：这里比的是编号已生成、Markdown 标记
//! 已剥离的渲染文本，只标真实文字的增删，格式变化（加粗、层级、段落拆合、
//! 空格、全半角）一律不标。导出侧把标注层序列化成带哨兵的 Markdown，交给原有
//! 导出链，哨兵在「解析后」注入，不再碰用户源码的语法结构。
//!
//! 比较规则（与方案第四节一一对应）：
//! 1. 按章节对齐：先比标题序列（只比文字），章节内再比正文；
//! 2. 正文按文字流比：段落边界是软分隔，拆分 / 合并不产生标记；
//! 3. 词级 token：jieba 分词，日期、数字 + 单位、文号、公式整体为一个 token；
//! 4. 归并：夹缝 ≤ 2 字吸收；分句改动过半或改动片段 ≥ 3 段则整句删旧插新；
//! 5. 归一化后再比：比较键忽略空白，标点改动照标；
//! 6. 移动识别：未能对齐的删 / 增块相似度 ≥ 0.8 配成移动，段首加移来注记；
//! 7. 公文要素字段整体替换：旧值删除线、新值加框，不做词级；
//! 8. 编号：标题与列表编号是程序生成的，不参与比较；新增标题整项加框，
//!    删除的标题不显示编号；
//! 9. 表格：逐格词级；整行增删整行标注；列数变化整表删旧插新；
//! 10. 公式：整体比较，变了就整体删旧插新。

mod compare;
mod model;
mod overlay;
mod postprocess;
mod serialize;
mod tokenize;

pub(crate) use compare::diff_documents;
pub(crate) use model::DocumentModel;
pub(crate) use postprocess::{redline_research_docx, redline_research_tex_files};
pub(crate) use serialize::to_marked_markdown;
