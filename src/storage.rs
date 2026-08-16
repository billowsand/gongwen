use crate::models::AppConfig;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::{fs, path::PathBuf};

const ZIP_PASSWORD_FILE: &str = ".zip-password";
const MAX_REMEMBERED_PASSWORD_BYTES: u64 = 1024;

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

/// 读取用户明确选择“记住”的 ZIP 密码。密码单独存放，避免进入 config.json、
/// 配置版本历史或稿件导出包。Unix 下写入时把权限限制为仅当前用户可读写。
pub fn load_remembered_zip_password() -> Result<Option<String>> {
    let path = config_dir()?.join(ZIP_PASSWORD_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let metadata = fs::metadata(&path)?;
    if metadata.len() > MAX_REMEMBERED_PASSWORD_BYTES {
        anyhow::bail!("记住的 ZIP 密码文件异常，已拒绝读取");
    }
    let password = fs::read_to_string(&path)
        .with_context(|| format!("读取记住的 ZIP 密码失败：{}", path.display()))?;
    Ok((!password.is_empty()).then_some(password))
}

pub fn save_remembered_zip_password(password: Option<&str>) -> Result<()> {
    let path = config_dir()?.join(ZIP_PASSWORD_FILE);
    if let Some(password) = password {
        let parent = path.parent().context("ZIP 密码路径缺少父目录")?;
        fs::create_dir_all(parent)?;
        let temp = path.with_extension("password.tmp");
        write_private_file(&temp, password.as_bytes())?;
        if path.exists() {
            fs::remove_file(&path)?;
        }
        fs::rename(&temp, &path)?;
    } else if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes)?;
    Ok(())
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
        assert!(
            config
                .profiles
                .iter()
                .any(|profile| profile.kind == TemplateKind::RedHeadApproval)
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
        // 本地模型服务（LM Studio / Ollama）不提供 rerank 端点，示例配置默认走对话大模型重排。
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

    /// 校对词表是 v0.4 才有的字段，此前的配置里没有它，载入时必须回落空覆盖层
    /// ——而不是让整份配置反序列化失败，把用户的词库和模板一起丢掉。
    #[test]
    fn configs_written_before_the_proofread_lexicon_still_load() {
        let legacy = r#"{"output_dir":"C:/out"}"#;
        let config: AppConfig = serde_json::from_str(legacy).expect("老配置应能反序列化");
        assert!(config.proofread.overrides.is_empty());
        assert!(config.proofread.custom.is_empty());
    }

    /// 词表改动必须能存下来：停用一条内置、自建一条，存盘再读回来要一模一样。
    #[test]
    fn proofread_changes_survive_a_save_and_load_round_trip() {
        use crate::models::ProofreadRule;

        let mut config = AppConfig::default();
        config.proofread.override_mut("TYP-001").enabled = Some(false);
        config.proofread.custom.push(ProofreadRule {
            id: "USR-001".into(),
            wrong: "我办".into(),
            suggestion: "本办".into(),
            level: "疑似".into(),
            condition: "总是".into(),
            group: "自定义".into(),
            note: String::new(),
            enabled: true,
        });

        let raw = serde_json::to_string(&config).expect("序列化");
        let restored: AppConfig = serde_json::from_str(&raw).expect("反序列化");
        assert_eq!(restored.proofread, config.proofread);

        // 合并后确实生效：被停用的那条不再命中，自建的那条会命中。
        let lexicon = crate::proofread::Lexicon::resolved(&restored.proofread);
        let notes = lexicon.check("按上级布署办理，材料报我办。");
        assert!(notes.iter().all(|note| note.entry_id != "TYP-001"));
        assert!(notes.iter().any(|note| note.entry_id == "USR-001"));
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
