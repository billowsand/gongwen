//! 大段粘贴作为临时材料；发送时保存全文快照，重跑不依赖临时文件。

use std::io::Write;

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
    pub(crate) material: Material,
    pub(crate) chars: usize,
    // 持有句柄，取消引用、发送成功或关闭稿件时自动清理。
    pub(crate) file: tempfile::NamedTempFile,
}

impl PastedFile {
    pub(super) fn new(text: &str) -> std::io::Result<Self> {
        let mut file = tempfile::Builder::new()
            .prefix("gongwen-paste-")
            .suffix(".txt")
            .tempfile()?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        Ok(Self {
            chars: text.chars().count(),
            material: Material {
                name: file
                    .path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                text: text.to_owned(),
            },
            file,
        })
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
