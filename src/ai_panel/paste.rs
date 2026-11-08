//! 大段粘贴作为临时材料；发送时保存全文快照，重跑不依赖临时文件。
//!
//! 这里**不落磁盘**：材料全文只存在内存里的 `Material.text`，随会话快照一起
//! 保存。旧实现把全文写进系统临时目录的 `.txt`，崩溃/强杀后明文残留，且文件
//! 本身没有任何用途——名字只是给界面和提示词看的标签。

pub(super) const PASTE_CHARS: usize = 2000;
pub(super) const PASTE_LINES: usize = 30;

pub(super) fn is_large(text: &str) -> bool {
    text.chars().take(PASTE_CHARS + 1).count() > PASTE_CHARS
        || text.lines().take(PASTE_LINES + 1).count() > PASTE_LINES
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Material {
    pub(crate) name: String,
    pub(crate) text: String,
}

#[derive(Debug)]
pub(crate) struct PastedFile {
    pub(crate) id: u64,
    pub(crate) material: Material,
    pub(crate) chars: usize,
}

impl PastedFile {
    pub(super) fn new(text: &str) -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let chars = text.chars().count();
        Self {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            chars,
            material: Material {
                // 中性标签：不带正文片段、不带路径，只用于界面与提示词里区分材料。
                name: format!(
                    "粘贴材料-{}-{chars}字",
                    chrono::Local::now().format("%H%M%S")
                ),
                text: text.to_owned(),
            },
        }
    }
}

/// 材料保持原文，要求单独标明；全文参与现有事实闸门与上下文预算。
pub(super) fn with_materials(request: &str, materials: &[Material]) -> String {
    if materials.is_empty() {
        return request.trim().to_owned();
    }
    let mut parts: Vec<String> = materials
        .iter()
        .map(|material| format!("【粘贴材料《{}》】\n{}", material.name, material.text))
        .collect();
    if !request.trim().is_empty() {
        parts.push(format!("【要求】\n{}", request.trim()));
    }
    parts.join("\n\n")
}
