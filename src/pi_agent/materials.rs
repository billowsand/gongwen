//! 本轮材料快照：粘贴材料与 `@` 引用在后台读成文件，写入 `input/materials/`。
//!
//! 每个文件用**程序生成的编号文件名**（`01.md`、`02.md`……），标题与来源只写进
//! `index.json`，不拿用户文字拼路径。角色由界面选择或程序确定，模型不能把
//! 「写法样稿 / 结构范文」升级成事实来源。
//!
//! 读不到、超限或角色不明一律明确失败，不静默丢掉。

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

/// 材料用途。只有 [`Self::Fact`] 与来函可以作为本次事项的事实来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MaterialRole {
    /// 事实材料：用户提供的依据，可以从中取事实。
    #[default]
    Fact,
    /// 写法样稿：只学表达、语气、句式。
    StyleSample,
    /// 结构范文：只学章节与条款组织。
    StructureSample,
    /// 来函：本次事项的来文，可以取事实。
    IncomingLetter,
}

impl MaterialRole {
    pub(crate) const ALL: [Self; 4] = [
        Self::Fact,
        Self::StyleSample,
        Self::StructureSample,
        Self::IncomingLetter,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Fact => "事实材料",
            Self::StyleSample => "写法样稿",
            Self::StructureSample => "结构范文",
            Self::IncomingLetter => "来函",
        }
    }

    /// 是否允许作为事实来源。样稿与范文永远不是。
    pub(crate) fn is_fact_source(self) -> bool {
        matches!(self, Self::Fact | Self::IncomingLetter)
    }
}

/// 待写入的一份材料。`source` 是给人看的来源（粘贴、稿件库、知识库）。
#[derive(Debug, Clone)]
pub(crate) struct PiMaterialInput {
    pub(crate) title: String,
    pub(crate) source: String,
    pub(crate) role: MaterialRole,
    pub(crate) text: String,
}

/// 写进 init 与 `index.json` 的元数据。
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiMaterialMeta {
    /// `input/materials/01.md` 这类工作区内路径。
    pub(crate) file: String,
    pub(crate) title: String,
    pub(crate) source: String,
    pub(crate) role: MaterialRole,
    pub(crate) sha256: String,
    pub(crate) bytes: usize,
}

pub(crate) const MAX_MATERIALS: usize = 20;
pub(crate) const MAX_MATERIAL_BYTES: usize = 512 * 1024;
pub(crate) const MAX_MATERIAL_TOTAL_BYTES: usize = 4 * 1024 * 1024;

fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 把材料写进 `<workspace>/input/materials/`，返回元数据清单。
pub(crate) fn write_all(
    workspace: &Path,
    materials: &[PiMaterialInput],
) -> Result<Vec<PiMaterialMeta>, String> {
    if materials.len() > MAX_MATERIALS {
        return Err(format!(
            "本轮材料过多（{} 份，最多 {MAX_MATERIALS} 份），请分批处理。",
            materials.len()
        ));
    }
    let mut total = 0usize;
    let mut metas = Vec::with_capacity(materials.len());
    let dir = workspace.join("input").join("materials");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("建不了材料目录（{}）：{error}", dir.display()))?;
    for (index, material) in materials.iter().enumerate() {
        if material.text.trim().is_empty() {
            return Err(format!(
                "材料《{}》是空的，请移除后再发送。",
                material.title
            ));
        }
        let bytes = material.text.len();
        if bytes > MAX_MATERIAL_BYTES {
            return Err(format!(
                "材料《{}》超过单份上限（{} / {} 字节）。",
                material.title, bytes, MAX_MATERIAL_BYTES
            ));
        }
        total = total.saturating_add(bytes);
        if total > MAX_MATERIAL_TOTAL_BYTES {
            return Err(format!(
                "本轮材料合计超过上限（{} 字节，最多 {} 字节）。",
                total, MAX_MATERIAL_TOTAL_BYTES
            ));
        }
        let name = format!("{:02}.md", index + 1);
        let relative = format!("input/materials/{name}");
        let path = dir.join(&name);
        crate::vault::write_path(&path, "pi", material.text.as_bytes())
            .map_err(|error| format!("写不出材料《{}》：{error:#}", material.title))?;
        metas.push(PiMaterialMeta {
            file: relative,
            title: material.title.trim().to_string(),
            source: material.source.trim().to_string(),
            role: material.role,
            sha256: sha256_hex(&material.text),
            bytes,
        });
    }
    let index =
        serde_json::to_vec_pretty(&metas).map_err(|error| format!("组装材料索引失败：{error}"))?;
    let path = dir.join("index.json");
    crate::vault::write_path(&path, "pi", &index)
        .map_err(|error| format!("写不出材料索引（{}）：{error:#}", path.display()))?;
    Ok(metas)
}

/// 给模型看的材料索引（用户消息里的一段）。
pub(crate) fn prompt_block(metas: &[PiMaterialMeta]) -> String {
    if metas.is_empty() {
        return "本轮没有附材料。".into();
    }
    let mut lines = vec!["本轮材料（只读，全文在 input/materials/）：".to_string()];
    for meta in metas {
        lines.push(format!(
            "- {}：{}（{}，来源：{}）",
            meta.file,
            meta.title,
            meta.role.label(),
            meta.source
        ));
    }
    lines.push("写法样稿与结构范文只提供写法，不能当作本次事项的事实来源。".into());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(title: &str, role: MaterialRole, text: &str) -> PiMaterialInput {
        PiMaterialInput {
            title: title.into(),
            source: "粘贴".into(),
            role,
            text: text.into(),
        }
    }

    #[test]
    fn materials_get_numbered_files_and_an_index() {
        let temp = tempfile::tempdir().unwrap();
        let inputs = vec![
            material("../越界", MaterialRole::Fact, "事实甲"),
            material("样稿", MaterialRole::StyleSample, "样稿乙"),
        ];
        let metas = write_all(temp.path(), &inputs).unwrap();
        assert_eq!(metas[0].file, "input/materials/01.md");
        assert_eq!(metas[1].file, "input/materials/02.md");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("input/materials/01.md")).unwrap(),
            "事实甲"
        );
        let index =
            std::fs::read_to_string(temp.path().join("input/materials/index.json")).unwrap();
        assert!(index.contains("\"file\": \"input/materials/01.md\""));
        assert!(
            index.contains("\"title\": \"../越界\""),
            "用户文字只进元数据"
        );
        assert!(
            !index.contains("materials/../"),
            "用户文字不能出现在任何工作区路径里"
        );
        assert!(temp.path().join("input/materials/01.md").is_file());
        assert!(!temp.path().join("..").join("越界.md").exists());
    }

    #[test]
    fn only_fact_roles_become_fact_sources() {
        let temp = tempfile::tempdir().unwrap();
        let inputs = vec![
            material(
                "来函",
                MaterialRole::IncomingLetter,
                "我局定于10月8日检查。",
            ),
            material("样稿", MaterialRole::StyleSample, "去年10月9日开会。"),
            material("范文", MaterialRole::StructureSample, "第一章 总则。"),
        ];
        let _ = write_all(temp.path(), &inputs).unwrap();
        let fact_texts: Vec<&str> = inputs
            .iter()
            .filter(|material| material.role.is_fact_source())
            .map(|material| material.text.as_str())
            .collect();
        assert_eq!(fact_texts, vec!["我局定于10月8日检查。"]);
        assert!(!MaterialRole::StyleSample.is_fact_source());
    }

    #[test]
    fn empty_and_oversized_materials_fail_loudly() {
        let temp = tempfile::tempdir().unwrap();
        assert!(write_all(temp.path(), &[material("空", MaterialRole::Fact, "  ")]).is_err());
        let huge = "字".repeat(MAX_MATERIAL_BYTES);
        assert!(write_all(temp.path(), &[material("大", MaterialRole::Fact, &huge)]).is_err());
        let many = (0..=MAX_MATERIALS)
            .map(|i| material(&format!("m{i}"), MaterialRole::Fact, "x"))
            .collect::<Vec<_>>();
        assert!(write_all(temp.path(), &many).is_err());
    }
}
