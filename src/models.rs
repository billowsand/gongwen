//! 全局数据模型：应用配置、文种模板、AI 服务商、密级规则、外观字体与稿件输入。
//!
//! 各功能域已拆分到 `models/` 子模块，这里保留测试并从子模块再导出全部类型，
//! 使 `crate::models::X` 的引用路径保持不变。

mod ai;
mod config;
mod draft;
mod provider;
mod rag;
mod security;
mod template;
mod typography;
mod vocabulary;

pub(crate) use ai::AiPrompt;
pub(crate) use ai::ProofreadConfig;
pub(crate) use ai::ProofreadOverride;
pub(crate) use ai::ProofreadRule;
pub(crate) use ai::builtin_ai_prompts;
pub(crate) use config::AUTO_SAVE_INTERVAL_MAX_SECS;
pub(crate) use config::AUTO_SAVE_INTERVAL_MIN_SECS;
pub(crate) use config::AppConfig;
pub(crate) use config::UI_SCALE_MAX;
pub(crate) use config::UI_SCALE_MIN;
pub(crate) use config::UI_SCALE_PRESETS;
pub(crate) use draft::DraftInput;
pub(crate) use draft::ExportSelection;
pub(crate) use draft::GeneratedDraft;
pub(crate) use draft::ManuscriptStatus;
pub(crate) use draft::PhoneRecordMetadata;
pub(crate) use draft::PrintConfig;
pub(crate) use draft::PrinterPrefs;
pub(crate) use draft::ResearchMetadata;
pub(crate) use draft::ResearchPalette;
pub(crate) use draft::ResearchTemplate;
pub(crate) use draft::ReviewNote;
pub(crate) use draft::StyleMode;
pub(crate) use provider::EmbeddingConfig;
pub(crate) use provider::LmStudioConfig;
pub(crate) use provider::ModelKind;
pub(crate) use provider::ModelRef;
pub(crate) use provider::ModelRefError;
pub(crate) use provider::PROVIDER_PRESETS;
pub(crate) use provider::ProviderConfig;
pub(crate) use provider::ProviderPreset;
pub(crate) use provider::ProxyConfig;
pub(crate) use provider::ProxyMode;
pub(crate) use provider::ReasoningEffort;
pub(crate) use provider::ReasoningFormat;
pub(crate) use provider::ReviseModelConfig;
pub(crate) use provider::classify_model;
pub(crate) use provider::preset_for_url;
pub(crate) use rag::RagConfig;
pub(crate) use rag::RerankConfig;
pub(crate) use rag::RerankMode;
pub(crate) use security::SecurityLevel;
pub(crate) use security::SecurityRules;
pub(crate) use security::join_units;
pub(crate) use security::split_period_digits;
pub(crate) use security::split_units;
pub(crate) use template::CorrespondenceScope;
pub(crate) use template::JointContact;
pub(crate) use template::JointIssuanceMode;
pub(crate) use template::JointResponsibleEntry;
pub(crate) use template::LetterVersion;
pub(crate) use template::RED_APPROVAL_MAX_RESPONSIBLE;
pub(crate) use template::TemplateKind;
pub(crate) use template::TemplateProfile;
pub(crate) use template::format_copy_number;
pub(crate) use template::is_joint_signature;
pub(crate) use template::joint_responsible_entries;
pub(crate) use template::sync_joint_responsible;
pub(crate) use typography::BoldStyle;
pub(crate) use typography::DiagramTheme;
pub(crate) use typography::EDITOR_FONT_SIZE_MAX;
pub(crate) use typography::EDITOR_FONT_SIZE_MIN;
pub(crate) use typography::EditorFontFace;
pub(crate) use typography::EditorFontPreset;
pub(crate) use typography::EditorFontScheme;
pub(crate) use typography::EditorFontSlot;
pub(crate) use typography::FontChoice;
pub(crate) use typography::FontConfig;
pub(crate) use typography::FontRole;
pub(crate) use typography::HeadingNumbering;
pub(crate) use typography::ImeConfig;
pub(crate) use typography::ImePhrase;
pub(crate) use typography::ListNumbering;
pub(crate) use typography::NumberingConfig;
pub(crate) use typography::PaperMode;
pub(crate) use typography::RibbonTab;
pub(crate) use typography::ThemeName;
pub(crate) use vocabulary::VocabularyCategory;
pub(crate) use vocabulary::VocabularyEntry;
pub(crate) use vocabulary::VocabularySetupStatus;

#[cfg(test)]
pub(crate) use config::DEFAULT_AUTO_SAVE_INTERVAL_SECS;
#[cfg(test)]
pub(crate) use security::LONG_TERM;

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    #[test]
    fn research_template_defaults_preserve_legacy_snapshot_hash() {
        use sha2::{Digest, Sha256};
        let input = DraftInput::default();
        let legacy = serde_json::to_string(&input).unwrap();
        assert!(!legacy.contains("\"template\":\"classic\""));
        assert!(!legacy.contains("\"palette\""));
        // 模拟旧程序按无新增字段的 JSON 字节计算出的校验值。
        let mut hash = Sha256::new();
        for part in [legacy.as_bytes(), "正文".as_bytes(), "备注".as_bytes()] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        let expected: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert!(!legacy.contains("\"landscape\""), "横页没勾不写入快照");
        let restored: DraftInput = serde_json::from_str(&legacy).unwrap();
        assert_eq!(restored.research.template, ResearchTemplate::Classic);
        assert!(!restored.profile.landscape);
        assert_eq!(
            crate::manuscript::sync::payload_hash(&restored, "正文", "备注").unwrap(),
            expected
        );
        let mut terminal = restored;
        terminal.research.template = ResearchTemplate::Terminal;
        terminal.research.palette = ResearchPalette::Bright;
        let encoded = serde_json::to_string(&terminal).unwrap();
        let decoded: DraftInput = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.research, terminal.research);
        assert_ne!(
            crate::manuscript::sync::payload_hash(&decoded, "正文", "备注").unwrap(),
            expected
        );
    }
    use super::*;

    #[test]
    fn backup_models_load_old_config_and_report_deleted_providers() {
        let mut config: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(config.draft_backup_chat().unwrap().is_none());
        assert!(config.revise_backup_chat(true).unwrap().is_none());
        config.draft_backup_model = ModelRef {
            provider_id: "gone".into(),
            model: "backup".into(),
        };
        let error = config.draft_backup_chat().unwrap_err().to_string();
        assert!(error.contains("文字起草备用模型"));
        assert!(error.contains("AI 管理 → 模型服务"));
        config.revise_model.backup_model_ref = config.draft_backup_model.clone();
        assert!(
            config
                .revise_backup_chat(true)
                .unwrap_err()
                .to_string()
                .contains("文字复核备用模型")
        );
        config.providers.push(ProviderConfig {
            id: "gone".into(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            ..Default::default()
        });
        let backup = config.revise_backup_chat(true).unwrap().unwrap();
        assert_eq!(backup.model, "backup");
        assert_eq!(backup.temperature, 0.0);
        assert_eq!(backup.max_tokens, config.lm_studio.max_tokens);
        assert_eq!(backup.context_window, 0);
    }

    /// 复核没启用时辅助角色用的是起草模型，备用也跟着起草走；启用了复核模型就各管各的。
    #[test]
    fn the_assist_role_inherits_the_draft_backup_when_it_runs_on_the_draft_model() {
        let mut config: AppConfig = serde_json::from_str("{}").unwrap();
        config.providers.push(ProviderConfig {
            id: "p".into(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            ..Default::default()
        });
        config.draft_backup_model = ModelRef {
            provider_id: "p".into(),
            model: "起草备用".into(),
        };
        config.lm_studio.temperature = 0.7;
        let assist = config.assist_backup_chat().unwrap().unwrap();
        assert_eq!(assist.model, "起草备用");
        assert_eq!(assist.temperature, 0.0, "辅助角色要可复现");

        config.revise_model.enabled = true;
        config.revise_model.model_ref = ModelRef {
            provider_id: "p".into(),
            model: "复核".into(),
        };
        assert!(
            config.assist_backup_chat().unwrap().is_none(),
            "有自己的复核模型、没配复核备用：不借起草的备用"
        );
        config.revise_model.backup_model_ref = ModelRef {
            provider_id: "p".into(),
            model: "复核备用".into(),
        };
        assert_eq!(
            config.assist_backup_chat().unwrap().unwrap().model,
            "复核备用"
        );
    }

    /// 用途归类：rerank 关键词最具体（bge-reranker 不能归成向量）；
    /// 服务自报优先于名字启发式；认不出的按对话处理。
    #[test]
    fn classify_model_by_name_and_probe() {
        assert_eq!(
            classify_model("bge-reranker-v2-m3", None),
            ModelKind::Rerank
        );
        assert_eq!(
            classify_model("bge-reranker-v2-m3", Some(ModelKind::Chat)),
            ModelKind::Rerank,
            "服务把 reranker 报成 llm 时仍以名字为准"
        );
        assert_eq!(classify_model("bge-m3", None), ModelKind::Embedding);
        assert_eq!(
            classify_model("bge-large-zh-v1.5", None),
            ModelKind::Embedding
        );
        assert_eq!(
            classify_model("text-embedding-3-large", None),
            ModelKind::Embedding
        );
        assert_eq!(
            classify_model("nomic-embed-text", None),
            ModelKind::Embedding
        );
        assert_eq!(
            classify_model("multilingual-e5-large", None),
            ModelKind::Embedding
        );
        assert_eq!(classify_model("qwen3-8b", None), ModelKind::Unknown);
        assert_eq!(
            classify_model("qwen3-8b", Some(ModelKind::Chat)),
            ModelKind::Chat
        );
        assert_eq!(
            classify_model("some-model", Some(ModelKind::Embedding)),
            ModelKind::Embedding,
            "服务自报向量时，名字里没有线索也认"
        );
        assert_eq!(
            classify_model("some-model", Some(ModelKind::Unknown)),
            ModelKind::Unknown
        );
    }

    /// kinds 字段为空时不写进配置，旧配置没有它也能读。
    #[test]
    fn provider_kinds_serde_roundtrip() {
        let provider = ProviderConfig::default();
        let json = serde_json::to_string(&provider).unwrap();
        assert!(!json.contains("kinds"));

        let legacy = r#"{"id":"p1","name":"x","models":["m"]}"#;
        let parsed: ProviderConfig = serde_json::from_str(legacy).unwrap();
        assert!(parsed.kinds.is_empty());
    }

    #[test]
    fn opencode_go_preset_repairs_only_missing_default_address() {
        let preset = PROVIDER_PRESETS
            .iter()
            .find(|p| p.id == "opencode-go")
            .unwrap();
        assert_eq!(preset.base_url, "https://opencode.ai/zen/go/v1");
        assert!(preset.needs_key);
        assert_eq!(preset_for_url(preset.base_url).unwrap().id, preset.id);
        let mut config = AppConfig::default();
        config.providers = vec![
            ProviderConfig {
                id: "p1".into(),
                name: preset.name.into(),
                api_key: "test-key".into(),
                models: vec!["deepseek-v4-flash".into()],
                ..Default::default()
            },
            ProviderConfig {
                id: "p2".into(),
                name: preset.name.into(),
                base_url: "https://example.com/v1".into(),
                ..Default::default()
            },
            ProviderConfig {
                id: "p3".into(),
                name: "自定义提供商".into(),
                ..Default::default()
            },
        ];
        assert!(config.migrate_providers());
        assert_eq!(config.providers[0].base_url, preset.base_url);
        assert_eq!(config.providers[0].api_key, "test-key");
        assert_eq!(config.providers[0].models, ["deepseek-v4-flash"]);
        assert_eq!(config.providers[1].base_url, "https://example.com/v1");
        assert!(config.providers[2].base_url.is_empty());
        assert!(!config.migrate_providers());
    }

    /// 旧配置（内联地址密钥）迁移成提供商清单与模型引用：同一地址密钥只建一条，
    /// 按地址认出预设名，旧字段清空。
    #[test]
    fn legacy_inline_endpoints_migrate_into_providers() {
        let mut config = AppConfig::default();
        config.lm_studio.base_url = "http://127.0.0.1:1234/v1".into();
        config.lm_studio.model = "qwen3-8b".into();
        config.revise_model.enabled = true;
        config.revise_model.model = "qwen3-4b".into();
        // 复核地址留空 = 沿用起草，迁移后应指向同一家提供商。
        config.rag.embedding.base_url = "https://api.siliconflow.cn/v1".into();
        config.rag.embedding.api_key = "sk-x".into();
        config.rag.embedding.model = "bge-m3".into();

        assert!(config.migrate_providers());
        assert_eq!(config.providers.len(), 2);
        assert_eq!(config.providers[0].name, "LM Studio");
        assert_eq!(config.providers[1].name, "硅基流动");
        assert_eq!(config.draft_model.model, "qwen3-8b");
        assert_eq!(config.revise_model.model_ref.provider_id, "p1");
        assert_eq!(config.rag.embedding.model_ref.provider_id, "p2");
        assert!(config.lm_studio.base_url.is_empty(), "旧字段应清空");
        assert!(config.revise_model.model.is_empty());

        // 幂等：已有提供商时不再迁。
        assert!(!config.migrate_providers());
    }

    /// 提供商引用模式下，沿用起草与独立复核分别使用对应的窗口配置。
    #[test]
    fn referenced_assist_inherits_the_draft_window_only_when_reusing_it() {
        let mut config = AppConfig::default();
        config.lm_studio.model = "DeepSeek-V4.1-Flash".into();
        config.lm_studio.context_window = 1_048_576;
        config.migrate_providers();
        assert!(!config.draft_model.is_empty());
        assert_eq!(config.assist_chat().unwrap().context_window, 1_048_576);
        config.revise_model.enabled = true;
        assert_eq!(config.revise_chat(false).unwrap().context_window, 1_048_576);
        config.revise_model.model_ref = ModelRef {
            provider_id: config.draft_model.provider_id.clone(),
            model: "qwen3-4b".into(),
        };
        let assist = config.assist_chat().unwrap();
        assert_eq!(assist.model, "qwen3-4b");
        assert_eq!(assist.context_window, 0);
    }

    /// 迁移后的解析：起草 / 复核 / embedding 都拿到提供商的地址密钥。
    #[test]
    fn resolvers_fill_endpoint_from_the_provider() {
        let mut config = AppConfig::default();
        config.lm_studio.base_url = "https://www.dmxapi.cn/v1".into();
        config.lm_studio.api_key = "dmx-key".into();
        config.lm_studio.model = "DeepSeek-V4-Flash".into();
        config.revise_model.enabled = true;
        config.revise_model.model = "qwen3-4b".into();
        config.rag.embedding.base_url = "https://www.dmxapi.cn/v1".into();
        config.rag.embedding.api_key = "dmx-key".into();
        config.rag.embedding.model = "bge-m3".into();
        config.migrate_providers();

        let draft = config.draft_chat().unwrap();
        assert_eq!(draft.base_url, "https://www.dmxapi.cn/v1");
        assert_eq!(draft.api_key, "dmx-key");
        assert_eq!(draft.model, "DeepSeek-V4-Flash");

        let revise = config.revise_chat(false).unwrap();
        assert_eq!(revise.model, "qwen3-4b");
        assert_eq!(revise.temperature, 0.0);
        assert_eq!(revise.base_url, "https://www.dmxapi.cn/v1");

        let rag = config.resolved_rag();
        assert_eq!(rag.embedding.model, "bge-m3");
        assert_eq!(rag.embedding.api_key, "dmx-key");

        // 提供商被删除：起草报出指向「模型服务」的错误，引用保留。
        config.providers.retain(|p| p.id != "p1");
        let error = config.draft_chat().unwrap_err().to_string();
        assert!(error.contains("模型服务"), "{error}");
        assert_eq!(config.draft_model.model, "DeepSeek-V4-Flash");
        // 知识库解析不报错而是留空地址，交给运行时的降级与告警。
        assert!(config.resolved_rag().embedding.base_url.is_empty());
    }

    /// 「研报」分区卡只在研究报告下出现，其余分区卡各文种都在。
    #[test]
    fn only_research_reports_get_the_research_ribbon_tab() {
        for kind in TemplateKind::ALL {
            let shown: Vec<&str> = RibbonTab::ALL
                .into_iter()
                .filter(|tab| tab.shown_for(kind))
                .map(RibbonTab::label)
                .collect();
            if kind.is_research() {
                assert_eq!(shown.len(), RibbonTab::ALL.len(), "{kind:?}");
                assert!(shown.contains(&"研报"), "{kind:?}");
            } else {
                assert_eq!(shown.len(), RibbonTab::ALL.len() - 1, "{kind:?}");
                assert!(!shown.contains(&"研报"), "{kind:?}");
            }
        }
    }

    /// 旧配置没有 `ai_prompts` 字段，载入后必须补齐全部预置项并编好 id。
    #[test]
    fn missing_ai_prompts_are_backfilled_with_unique_ids() {
        let mut config = AppConfig::default();
        assert!(config.ai_prompts.is_empty());
        config.ensure_ai_prompts();

        assert_eq!(config.ai_prompts.len(), builtin_ai_prompts().len());
        assert!(config.ai_prompts.iter().all(|entry| entry.id != 0));
        let ids = config
            .ai_prompts
            .iter()
            .map(|entry| entry.id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), config.ai_prompts.len(), "id 不得重复");
        // 保底项永远在，指令为空即“只规整格式”。
        let format = config
            .ai_prompts
            .iter()
            .find(|entry| entry.builtin_key == "format")
            .expect("应有格式规整预置项");
        assert!(format.instruction.is_empty());
        assert!(format.kinds.is_empty());
    }

    /// 密级是所有文稿类型的必要填报项目：新模板默认“机密、20年”。
    #[test]
    fn profile_defaults_to_confidential_20_years() {
        let profile = TemplateProfile::default();
        assert_eq!(profile.security_level, "机密");
        assert_eq!(profile.security_period, "20年");
        for kind in TemplateKind::ALL {
            let profile = TemplateProfile::for_kind(kind);
            assert_eq!(profile.security_level, "机密", "{kind:?}");
            assert_eq!(profile.security_period, "20年", "{kind:?}");
        }
    }

    /// 旧配置里各模板可能存了空密级（当时允许“不标注密级”），载入时统一回填默认值；
    /// 用户显式保存过的非空密级一律不动。
    #[test]
    fn ensure_security_defaults_backfills_empty_levels_only() {
        let mut config = AppConfig::default();
        let mut empty = TemplateProfile::for_kind(TemplateKind::OfficialLetter);
        empty.security_level.clear();
        empty.security_period.clear();
        let mut custom = TemplateProfile::for_kind(TemplateKind::PhoneNotice);
        custom.security_level = "秘密".into();
        custom.security_period = "10年".into();
        config.profiles = vec![empty, custom];

        config.ensure_security_defaults();

        let official = config
            .profiles
            .iter()
            .find(|profile| profile.kind == TemplateKind::OfficialLetter)
            .expect("应有公函模板");
        assert_eq!(official.security_level, "机密");
        assert_eq!(official.security_period, "20年");
        let phone = config
            .profiles
            .iter()
            .find(|profile| profile.kind == TemplateKind::PhoneNotice)
            .expect("应有电话通知模板");
        assert_eq!(phone.security_level, "秘密");
        assert_eq!(phone.security_period, "10年");
    }

    /// 旧稿件联系人未绑承办单位（unit 为空）：按索引回落 joint_responsible_units。
    #[test]
    fn joint_responsible_entries_fall_back_to_units_by_index() {
        let mut profile = TemplateProfile::default();
        profile.joint_responsible_units = "甲处室、乙处室".into();
        profile.joint_contacts = vec![
            JointContact {
                unit: String::new(),
                name: "张三".into(),
                phone: "010-11111111".into(),
            },
            JointContact {
                unit: String::new(),
                name: "李四".into(),
                phone: "010-22222222".into(),
            },
        ];
        let entries = joint_responsible_entries(&profile);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].unit, "甲处室");
        assert_eq!(entries[0].name, "张三");
        assert_eq!(entries[1].unit, "乙处室");
        assert_eq!(entries[1].name, "李四");
    }

    /// 旧稿件承办单位多于联系人：多出来的单位仍要成行进版记，只是联系人、电话为空。
    #[test]
    fn joint_responsible_entries_keep_units_without_contacts() {
        let mut profile = TemplateProfile::default();
        profile.joint_responsible_units = "甲处室、乙处室、丙处室".into();
        profile.joint_contacts = vec![JointContact {
            unit: String::new(),
            name: "张三".into(),
            phone: "010-11111111".into(),
        }];
        let entries = joint_responsible_entries(&profile);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "张三");
        assert_eq!(entries[1].unit, "乙处室");
        assert!(entries[1].name.is_empty());
        assert_eq!(entries[2].unit, "丙处室");
    }

    /// 成对录入后联系人自带承办单位：条目直接采用 contact.unit，不再依赖索引。
    #[test]
    fn joint_responsible_entries_prefer_contact_unit() {
        let mut profile = TemplateProfile::default();
        profile.joint_responsible_units = "甲处室".into();
        profile.joint_contacts = vec![JointContact {
            unit: "乙处室".into(),
            name: "王五".into(),
            phone: "010-33333333".into(),
        }];
        let entries = joint_responsible_entries(&profile);
        assert_eq!(entries[0].unit, "乙处室");
        assert_eq!(entries[0].phone, "010-33333333");
    }

    /// 完全空白的占位行（单位、联系人都没填）不进入成对视图，避免版记空行。
    #[test]
    fn joint_responsible_entries_skip_blank_placeholders() {
        let mut profile = TemplateProfile::default();
        profile.joint_responsible_units = "甲处室".into();
        profile.joint_contacts = vec![
            JointContact {
                unit: "甲处室".into(),
                name: "张三".into(),
                phone: "010-11111111".into(),
            },
            JointContact::default(),
        ];
        let entries = joint_responsible_entries(&profile);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].unit, "甲处室");
    }

    /// 成对录入写回：joint_responsible_units 与 joint_contacts 同步且一一对应。
    #[test]
    fn sync_joint_responsible_writes_both_fields_in_lockstep() {
        let mut profile = TemplateProfile::default();
        sync_joint_responsible(
            &mut profile,
            &[
                JointResponsibleEntry {
                    unit: "甲处室".into(),
                    name: "张三".into(),
                    phone: "010-11111111".into(),
                },
                JointResponsibleEntry {
                    unit: "乙处室".into(),
                    name: "李四".into(),
                    phone: "010-22222222".into(),
                },
            ],
        );
        assert_eq!(profile.joint_responsible_units, "甲处室、乙处室");
        assert_eq!(profile.joint_contacts.len(), 2);
        assert_eq!(profile.joint_contacts[0].unit, "甲处室");
        assert_eq!(profile.joint_contacts[0].name, "张三");
        assert_eq!(profile.joint_contacts[1].unit, "乙处室");
        assert_eq!(profile.joint_contacts[1].phone, "010-22222222");
    }

    /// 旧稿件 JSON 的 JointContact 没有 unit 字段：反序列化自动补空串，后续回落配对。
    /// 份号那一行画在「版记」分区、紧挨「打印方式」的下面。两个判定必须同时
    /// 为真，否则界面上会出现「有打印方式没份号」或反过来的错位。
    #[test]
    fn the_copy_number_row_sits_with_the_print_row_for_letters() {
        assert!(TemplateKind::OfficialLetter.uses_letter_layout());
        assert!(TemplateKind::OfficialLetter.has_copy_numbering());
        assert!(TemplateKind::MeetingMinutes.has_copy_numbering());
        // 其余文种版头没有份号位，不画这一行。
        for kind in [
            TemplateKind::PhoneNotice,
            TemplateKind::PlainDocument,
            TemplateKind::MeetingAgenda,
            TemplateKind::WhitePaper,
            TemplateKind::RedHeadApproval,
        ] {
            assert!(!kind.has_copy_numbering(), "{kind:?} 不该出现份号行");
        }
    }

    #[test]
    fn old_joint_contact_json_deserializes_with_default_unit() {
        let json = r#"{"name":"张三","phone":"010-11111111"}"#;
        let contact: JointContact = serde_json::from_str(json).unwrap();
        assert_eq!(contact.unit, "");
        assert_eq!(contact.name, "张三");
        assert_eq!(contact.phone, "010-11111111");
    }

    /// 旧配置不含界面字体时应回退系统默认，编译字体仍正常载入。
    #[test]
    fn font_config_without_legacy_ui_font_loads() {
        let json = r#"{
            "use_system_fonts": true,
            "title": {"family": "SimSun", "display": "宋体", "path": "C:\\Windows\\Fonts\\simsun.ttc"},
            "heading1": {"family": "SimHei", "display": "黑体", "path": "C:\\Windows\\Fonts\\simhei.ttf"},
            "heading2": {"family": "KaiTi", "display": "楷体", "path": "C:\\Windows\\Fonts\\simkai.ttf"},
            "body": {"family": "FangSong", "display": "仿宋", "path": "C:\\Windows\\Fonts\\simfang.ttf"},
            "pagenumber": {"family": "SimSun", "display": "宋体", "path": "C:\\Windows\\Fonts\\simsun.ttc"}
        }"#;
        let config: FontConfig = serde_json::from_str(json).expect("旧配置应能反序列化");
        assert!(config.active_ui_font().is_none());
        assert_eq!(config.active(FontRole::Body).unwrap().family, "FangSong");
    }

    /// 自定义界面字体应能随配置保存并原样读回。
    #[test]
    fn ui_font_round_trips_through_json() {
        let mut config = FontConfig::default();
        config.ui_font = FontChoice {
            family: "Source Han Sans SC".into(),
            display: "思源黑体".into(),
            path: "C:\\fonts\\SourceHanSansSC-Regular.otf".into(),
        };
        let json = serde_json::to_string(&config).expect("应能序列化");
        let back: FontConfig = serde_json::from_str(&json).expect("应能反序列化");
        assert_eq!(config, back);
        assert_eq!(back.active_ui_font().unwrap().display, "思源黑体");
    }

    /// 主题字段序列化后能原样读回；旧配置没有 `theme` 字段时回退默认 Claude。
    #[test]
    fn theme_name_round_trips_and_old_config_defaults_to_claude() {
        let mut config = AppConfig::default();
        config.theme = ThemeName::Nord;
        let json = serde_json::to_string(&config).expect("应能序列化");
        let back: AppConfig = serde_json::from_str(&json).expect("应能反序列化");
        assert_eq!(back.theme, ThemeName::Nord, "round-trip 后主题不变");

        // 去掉 theme 键模拟旧配置：载入后回退默认。
        let mut value: serde_json::Value = serde_json::from_str(&json).expect("应能解析为 JSON 值");
        value.as_object_mut().expect("配置应为对象").remove("theme");
        let old: AppConfig = serde_json::from_value(value.clone()).expect("旧配置应能反序列化");
        assert_eq!(old.theme, ThemeName::default(), "旧配置回退默认主题");

        // 移除主题后仍能读取完整旧配置，其他设置不受影响。
        for removed in [
            "sky",
            "lilac",
            "green",
            "everforest",
            "solarized_light",
            "latte",
            "gruvbox_light",
            "dracula",
            "tokyo_night",
        ] {
            value["theme"] = serde_json::json!(removed);
            let migrated: AppConfig =
                serde_json::from_value(value.clone()).expect("旧主题配置应能读取");
            assert_eq!(migrated.theme, ThemeName::Claude);
            assert_eq!(migrated.paper, back.paper);
        }
    }

    /// 补齐是幂等的：反复载入不会把预置项复制成好几份，也不会重排用户改过的内容。
    #[test]
    fn backfilling_ai_prompts_is_idempotent_and_keeps_user_edits() {
        let mut config = AppConfig::default();
        config.ensure_ai_prompts();
        let polish_id = config
            .ai_prompts
            .iter()
            .find(|entry| entry.builtin_key == "polish")
            .expect("应有润色预置项")
            .id;
        config
            .ai_prompts
            .iter_mut()
            .find(|entry| entry.id == polish_id)
            .unwrap()
            .instruction = "我改过的内容".into();

        let before = config.ai_prompts.clone();
        config.ensure_ai_prompts();
        assert_eq!(config.ai_prompts, before);
    }

    /// 自定义条目和预置项混在一起时，补齐只补缺的那条，并给新条目排不冲突的 id。
    #[test]
    fn backfilling_only_adds_the_missing_builtins() {
        let mut config = AppConfig::default();
        config.ai_prompts = vec![AiPrompt {
            id: 7,
            name: "我的提示词".into(),
            instruction: "精简".into(),
            kinds: vec![TemplateKind::WhitePaper],
            builtin_key: String::new(),
        }];
        config.ensure_ai_prompts();

        assert_eq!(config.ai_prompts.len(), builtin_ai_prompts().len() + 1);
        assert_eq!(config.ai_prompts[0].id, 7, "已有条目的 id 不变");
        assert!(
            config.ai_prompts.iter().skip(1).all(|entry| entry.id > 7),
            "新补的 id 必须避开已用的 7"
        );
        assert_eq!(config.next_ai_prompt_id(), 12);
    }

    #[test]
    fn prompts_without_kinds_apply_to_every_template() {
        let general = AiPrompt {
            kinds: vec![],
            ..Default::default()
        };
        assert!(
            TemplateKind::ALL
                .into_iter()
                .all(|kind| general.applies_to(kind))
        );
        assert_eq!(general.kinds_label(), "全部文种");
        assert!(!general.is_builtin());

        let scoped = AiPrompt {
            kinds: vec![TemplateKind::WhitePaper, TemplateKind::PlainDocument],
            builtin_key: "expand".into(),
            ..Default::default()
        };
        assert!(scoped.applies_to(TemplateKind::WhitePaper));
        assert!(!scoped.applies_to(TemplateKind::MeetingAgenda));
        assert_eq!(scoped.kinds_label(), "白头件（呈批件）、普通公文");
        assert!(scoped.is_builtin());
    }

    /// 扩写类提示词不该出现在会议议程和电话通知的选择面板里——那两种文种没有
    /// 可展开的论述段落。
    #[test]
    fn expand_preset_is_scoped_away_from_agenda_and_phone_notice() {
        let expand = builtin_ai_prompts()
            .into_iter()
            .find(|entry| entry.builtin_key == "expand")
            .expect("应有充实论述预置项");
        assert!(!expand.applies_to(TemplateKind::MeetingAgenda));
        assert!(!expand.applies_to(TemplateKind::PhoneNotice));
        assert!(expand.applies_to(TemplateKind::WhitePaper));
    }

    /// 预置项的 key 是恢复默认时的锚点，必须唯一。
    #[test]
    fn builtin_keys_are_unique_and_non_empty() {
        let keys = builtin_ai_prompts()
            .into_iter()
            .map(|entry| entry.builtin_key)
            .collect::<Vec<_>>();
        assert!(keys.iter().all(|key| !key.is_empty()));
        let unique = keys.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), keys.len());
    }

    #[test]
    fn period_options_are_capped_by_level() {
        let rules = SecurityRules::default();
        let secret = rules.period_options(SecurityLevel::Secret);
        assert!(secret.contains(&"10年".to_string()));
        assert!(!secret.contains(&"20年".to_string()));
        assert!(secret.contains(&LONG_TERM.to_string()));

        let top = rules.period_options(SecurityLevel::TopSecret);
        assert!(top.contains(&"30年".to_string()));
        assert!(!top.contains(&"50年".to_string()));

        // “内部”没有保密期限，下拉为空、默认期限也是空。
        assert!(rules.period_options(SecurityLevel::Internal).is_empty());
        assert!(rules.default_period(SecurityLevel::Internal).is_empty());
    }

    #[test]
    fn check_rejects_period_beyond_level_limit() {
        let rules = SecurityRules::default();
        assert!(rules.check(SecurityLevel::Secret, "10年").is_none());
        assert!(
            rules
                .check(SecurityLevel::Secret, "20年")
                .is_some_and(|message| message.contains("不得超过10年"))
        );
        assert!(rules.check(SecurityLevel::TopSecret, "长期").is_none());
        assert!(rules.check(SecurityLevel::Confidential, "").is_some());
        assert!(rules.check(SecurityLevel::Unmarked, "10年").is_some());
        assert!(rules.check(SecurityLevel::Unmarked, "").is_none());
        // “内部”不标注保密期限：空为正常，带期限（包括旧稿的“长期”）给出提示。
        assert!(rules.check(SecurityLevel::Internal, "").is_none());
        assert!(rules.check(SecurityLevel::Internal, "长期").is_some());
    }

    #[test]
    fn custom_limits_are_respected() {
        let rules = SecurityRules {
            confidential_max_years: 30,
            ..SecurityRules::default()
        };
        assert!(rules.check(SecurityLevel::Confidential, "30年").is_none());
        assert!(
            rules
                .period_options(SecurityLevel::Confidential)
                .contains(&"30年".to_string())
        );
    }

    #[test]
    fn units_round_trip_through_separators() {
        let units = split_units("甲单位、乙单位，丙单位;丁单位");
        assert_eq!(units, ["甲单位", "乙单位", "丙单位", "丁单位"]);
        assert_eq!(join_units(&units), "甲单位、乙单位、丙单位、丁单位");
    }

    #[test]
    fn split_period_digits_keeps_leading_numbers() {
        assert_eq!(split_period_digits("10年"), ("10", "年"));
        assert_eq!(split_period_digits("6个月"), ("6", "个月"));
        assert_eq!(split_period_digits("长期"), ("", "长期"));
        assert_eq!(split_period_digits(""), ("", ""));
        assert_eq!(split_period_digits("  5年"), ("5", "年"));
    }

    #[test]
    fn manuscript_status_transitions() {
        use ManuscriptStatus::{Archived, Draft, New, Published};
        assert!(New.can_transition(Draft));
        assert!(!New.can_transition(Published));
        assert!(!New.can_transition(Archived));
        assert!(Draft.can_transition(Published));
        assert!(Draft.can_transition(Archived));
        assert!(!Draft.can_transition(New));
        assert!(Published.can_transition(Draft));
        assert!(Published.can_transition(Archived));
        assert!(!Published.can_transition(New));
        assert!(!Archived.can_transition(Draft));
        assert!(!Archived.can_transition(Archived));
        assert_eq!(New.label(), "新建");
        assert_eq!(Draft.label(), "草稿");
        assert_eq!(Published.label(), "发布");
        assert_eq!(Archived.label(), "归档");
    }

    #[test]
    fn phone_record_snapshot_compatibility() {
        let old = serde_json::to_value(DraftInput::default()).unwrap();
        assert!(old.get("phone_record").is_none(), "旧快照校验值不应变化");
        let mut record: DraftInput = serde_json::from_value(old).unwrap();
        record.kind = TemplateKind::PhoneRecord;
        record.phone_record.caller_unit = "市政府办公室".into();
        record.phone_record.suggestion = "建议参会。".into();
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<DraftInput>(&json).unwrap(), record);
        assert!(!record.kind.has_document_number());
        assert!(!record.kind.has_copy_numbering());
    }

    #[test]
    fn draft_input_serde_round_trip() {
        let draft = DraftInput {
            kind: TemplateKind::MeetingAgenda,
            title_hint: "标题提示".into(),
            date: "2026年8月6日".into(),
            date_is_auto: false,
            meeting_time: "8月10日上午9时".into(),
            attendees: "张三、李四".into(),
            profile: TemplateProfile {
                document_number: "某教函〔2026〕12号".into(),
                ..TemplateProfile::for_kind(TemplateKind::MeetingAgenda)
            },
            research: Default::default(),
            phone_record: Default::default(),
            minutes: Default::default(),
        };
        let json = serde_json::to_string(&draft).expect("快照应能序列化");
        let back: DraftInput = serde_json::from_str(&json).expect("快照应能反序列化");
        assert_eq!(back.kind, TemplateKind::MeetingAgenda);
        assert_eq!(back.date, "2026年8月6日");
        assert_eq!(back.profile.document_number, "某教函〔2026〕12号");
        assert!(!back.date_is_auto);
        assert_eq!(back.attendees, "张三、李四");
    }
    /// 三套模板都要能被反认出来，否则设置页选完一套立刻显示成「自定义」。
    #[test]
    fn every_editor_font_preset_is_recognised_from_its_scheme() {
        for preset in EditorFontPreset::ALL {
            assert_eq!(
                EditorFontPreset::matching(&preset.scheme()),
                Some(preset),
                "{} 这套模板认不回来了",
                preset.label()
            );
        }
    }

    /// 默认方案必须就是「编辑器字体」那套：老配置升级上来显示不能变。
    #[test]
    fn the_default_editor_font_scheme_is_the_editor_preset() {
        assert_eq!(
            EditorFontScheme::default(),
            EditorFontPreset::Editor.scheme()
        );
        for slot in EditorFontSlot::ALL {
            assert_eq!(
                EditorFontScheme::default().face(slot),
                EditorFontFace::Editor,
                "{} 默认不该换字面",
                slot.label()
            );
        }
    }

    /// 旧配置文件里没有这一项，读出来要落到默认值上，而不是报错。
    #[test]
    fn a_config_without_editor_fonts_falls_back_to_the_default_scheme() {
        let config: AppConfig = serde_json::from_str("{}").expect("空配置应能读出默认值");
        assert_eq!(config.editor_fonts, EditorFontScheme::default());
        let json = serde_json::to_string(&config).expect("配置应能序列化");
        let back: AppConfig = serde_json::from_str(&json).expect("配置应能反序列化");
        assert_eq!(back.editor_fonts, config.editor_fonts);
    }

    /// 界面缩放、更新检查地址与示例稿标记都是后加字段：旧配置省略它们时
    /// 要落回默认值，序列化后再读也不能丢。「跟随系统明暗」也是后加字段，
    /// 旧配置缺省时必须默认开启，升级后界面才会自动与系统明暗一致。
    #[test]
    fn new_settings_fall_back_to_defaults_in_old_configs() {
        let config: AppConfig = serde_json::from_str("{}").expect("空配置应能读出默认值");
        assert_eq!(config.ui_scale, 1.0);
        assert_eq!(config.clamped_ui_scale(), 1.0);
        assert!(config.theme_follows_system, "旧配置也要默认跟随系统明暗");
        assert_eq!(
            config.auto_save_interval_secs,
            DEFAULT_AUTO_SAVE_INTERVAL_SECS
        );
        assert_eq!(
            config.clamped_auto_save_interval_secs(),
            DEFAULT_AUTO_SAVE_INTERVAL_SECS
        );
        assert_eq!(
            config.update_check_url,
            crate::update_check::DEFAULT_UPDATE_CHECK_URL
        );
        assert!(!config.samples_seeded);

        let json = serde_json::to_string(&config).expect("配置应能序列化");
        let back: AppConfig = serde_json::from_str(&json).expect("配置应能反序列化");
        assert_eq!(back.ui_scale, config.ui_scale);
        assert_eq!(back.theme_follows_system, config.theme_follows_system);
        assert_eq!(back.auto_save_interval_secs, config.auto_save_interval_secs);
        assert_eq!(back.update_check_url, config.update_check_url);
        assert_eq!(back.samples_seeded, config.samples_seeded);
    }

    /// 界面缩放的预设档位要覆盖常用档，且全部落在允许范围内。
    #[test]
    fn ui_scale_presets_stay_inside_the_supported_range() {
        for percent in [90, 100, 120, 150] {
            assert!(
                UI_SCALE_PRESETS.contains(&percent),
                "预设里缺少常用档位 {percent}%"
            );
        }
        for percent in UI_SCALE_PRESETS {
            let scale = percent as f32 / 100.0;
            assert!(
                (UI_SCALE_MIN..=UI_SCALE_MAX).contains(&scale),
                "预设档位 {percent}% 超出允许范围"
            );
        }
    }

    #[test]
    fn auto_save_interval_is_clamped_to_the_supported_range() {
        let mut config = AppConfig::default();
        config.auto_save_interval_secs = 5;
        assert_eq!(
            config.clamped_auto_save_interval_secs(),
            AUTO_SAVE_INTERVAL_MIN_SECS
        );
        config.auto_save_interval_secs = 9000;
        assert_eq!(
            config.clamped_auto_save_interval_secs(),
            AUTO_SAVE_INTERVAL_MAX_SECS
        );
        config.auto_save_interval_secs = 45;
        assert_eq!(config.clamped_auto_save_interval_secs(), 45);
        let json = serde_json::to_string(&config).expect("配置应能序列化");
        let back: AppConfig = serde_json::from_str(&json).expect("配置应能反序列化");
        assert_eq!(back.auto_save_interval_secs, 45);
    }

    #[test]
    fn ui_scale_is_clamped_to_the_supported_range() {
        let mut config = AppConfig::default();
        config.ui_scale = 0.1;
        assert_eq!(config.clamped_ui_scale(), UI_SCALE_MIN);
        config.ui_scale = 3.0;
        assert_eq!(config.clamped_ui_scale(), UI_SCALE_MAX);
        config.ui_scale = 1.25;
        assert_eq!(config.clamped_ui_scale(), 1.25);
        config.ui_scale = f32::NAN;
        assert_eq!(config.clamped_ui_scale(), 1.0);
    }
}
