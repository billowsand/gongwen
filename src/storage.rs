use crate::models::AppConfig;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::{fs, path::PathBuf};

pub fn config_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("cn", "LocalTools", "GongwenAssistant")
        .context("无法确定用户配置目录")?;
    Ok(dirs.config_dir().to_path_buf())
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.json"))
}

/// 稿件库 SQLite 文件，与 config.json 同目录，仅保存在本机。
pub fn manuscript_db_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("manuscripts.db"))
}

pub fn load() -> Result<AppConfig> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("读取配置失败：{}", path.display()))?;
    serde_json::from_str(&raw).context("配置文件格式无效")
}

pub fn save(config: &AppConfig) -> Result<()> {
    let path = config_path()?;
    let parent = path.parent().context("配置路径缺少父目录")?;
    fs::create_dir_all(parent)?;
    let temp = path.with_extension("json.tmp");
    let raw = serde_json::to_string_pretty(config)?;
    fs::write(&temp, raw)?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(&temp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        JointIssuanceMode, LetterVersion, SecurityLevel, TemplateKind, VocabularyCategory,
    };

    #[test]
    fn example_config_matches_the_current_schema() {
        let raw = include_str!("../config.example.json");
        let config: AppConfig = serde_json::from_str(raw).expect("示例配置应能反序列化");
        assert_eq!(config.security_rules.confidential_max_years, 20);
        assert!(config.export.docx);
        assert_eq!(
            config.profile(TemplateKind::OfficialLetter).letter_version,
            LetterVersion::Formal
        );
        assert_eq!(
            config
                .profile(TemplateKind::OfficialLetter)
                .joint_issuance_mode,
            JointIssuanceMode::Single
        );
        assert!(
            config
                .profiles
                .iter()
                .any(|profile| profile.kind == TemplateKind::PhoneNotice)
        );
        let contact = config
            .vocabulary
            .iter()
            .find(|entry| entry.category == VocabularyCategory::Person)
            .expect("示例配置应包含人员词条");
        assert_eq!(contact.phone, "000-12345678");
        // 机关代字是单位自身的属性，不再单列一类词条。
        assert!(
            config
                .vocabulary
                .iter()
                .any(|entry| entry.department_code == "某函")
        );
        // 人员挂在末端单位下，层级编码由上级编码加两位构成。
        assert_eq!(contact.unit, "0001");
        let branch = config
            .vocabulary
            .iter()
            .find(|entry| entry.canonical == "综合处")
            .expect("示例配置应包含下级单位");
        assert_eq!(branch.parent, "00");
        assert_eq!(branch.code, "0001");
        // 知识库 RAG 配置节
        assert_eq!(config.rag.recall_top_k, 20);
        assert_eq!(config.rag.rerank.path, "rerank");
        assert_eq!(config.rag.embedding.batch_size, 32);
        assert!(!config.rag.enabled);
        assert_eq!(config.rag.min_score_ratio, 0.5);
        // LM Studio 不提供 rerank 端点，示例配置默认走对话大模型重排。
        assert_eq!(config.rag.rerank.mode, crate::models::RerankMode::Llm);
    }

    /// 老配置里没有 `rag.rerank.mode` 字段，必须默认成「专用端点」，
    /// 保持既有行为不变——不能因为加了新枚举就把用户已配好的重排静默改掉。
    #[test]
    fn rerank_mode_defaults_to_api_for_existing_configs() {
        let legacy = r#"{"rag":{"rerank":{"model":"bge-reranker","path":"rerank"}}}"#;
        let config: AppConfig = serde_json::from_str(legacy).expect("老配置应能反序列化");
        assert_eq!(config.rag.rerank.mode, crate::models::RerankMode::Api);
        assert_eq!(config.rag.rerank.model, "bge-reranker");
        assert_eq!(
            config.rag.effective_rerank_mode(),
            crate::models::RerankMode::Api
        );
        // 选了专用端点却没填模型名时降级为不重排，而不是拿空模型名去请求。
        let mut empty = config.rag.clone();
        empty.rerank.model = String::new();
        assert_eq!(
            empty.effective_rerank_mode(),
            crate::models::RerankMode::None
        );
    }

    /// 手写或精简过的配置里可以只给出必填字段，其余一律取默认值。
    #[test]
    fn sparse_config_still_loads_with_defaults() {
        let sparse = r#"{
            "output_dir": "C:/out",
            "vocabulary": [
                {"category":"Unit","canonical":"某某省教育厅"},
                {"category":"Unit","canonical":"教师工作处","parent":"某某省教育厅"},
                {"category":"Person","canonical":"张三处长","unit":"教师工作处"}
            ],
            "last_template": "MeetingAgenda"
        }"#;
        let config: AppConfig = serde_json::from_str(sparse).expect("精简配置应能反序列化");
        assert_eq!(config.output_dir, "C:/out");
        assert!(config.vocabulary[0].phone.is_empty());
        // 缺省的 id 与层级编码留待载入时统一整理。
        assert_eq!(config.vocabulary[0].id, 0);
        assert!(config.vocabulary[0].code.is_empty());
        assert_eq!(
            config
                .vocabulary
                .iter()
                .filter(|entry| entry.category == VocabularyCategory::Unit)
                .count(),
            2
        );
        assert!(config.export.markdown);
        assert!(config.show_editor_line_numbers);
        assert_eq!(config.security_rules.secret_max_years, 10);
        assert!(!config.auto_export);
        assert!(!config.profile(TemplateKind::OfficialLetter).duplex_printing);
        assert_eq!(
            config.profile(TemplateKind::OfficialLetter).letter_version,
            LetterVersion::Formal
        );
        assert_eq!(
            config
                .profile(TemplateKind::OfficialLetter)
                .joint_issuance_mode,
            JointIssuanceMode::Single
        );
        assert_eq!(
            SecurityLevel::from_marking(""),
            SecurityLevel::Unmarked,
            "旧配置里空密级应视为不标注"
        );
    }
}
