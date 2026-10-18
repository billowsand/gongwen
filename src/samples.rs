//! 首启示例稿：编进二进制，首次启动时写入稿件库。
//!
//! 示例只做参考，不参与任何业务逻辑；写库失败不阻塞启动，配置里的
//! `samples_seeded` 也不置位，下次启动会再试。

use crate::manuscript::{ManuscriptFilter, ManuscriptStore, NewManuscript};
use crate::models::{AppConfig, DraftInput, ManuscriptStatus, TemplateKind};
use anyhow::Result;
use chrono::Local;

pub(crate) const OFFICIAL_LETTER: &str = include_str!("../assets/samples/official-letter.md");
pub(crate) const WHITE_PAPER: &str = include_str!("../assets/samples/white-paper.md");

/// 一篇示例稿的固定信息。标题同时用于写库去重，必须与 Markdown 首个 `#` 一致。
pub(crate) struct SampleDocument {
    pub(crate) title: &'static str,
    pub(crate) kind: TemplateKind,
    pub(crate) markdown: &'static str,
}

pub(crate) const DOCUMENTS: [SampleDocument; 2] = [
    SampleDocument {
        title: "（示例）关于商请协助做好政务信息资源共享工作的函",
        kind: TemplateKind::OfficialLetter,
        markdown: OFFICIAL_LETTER,
    },
    SampleDocument {
        title: "（示例）关于改善机关食堂用餐安排的请示",
        kind: TemplateKind::WhitePaper,
        markdown: WHITE_PAPER,
    },
];

impl SampleDocument {
    /// 按示例词库的常用写法填好要素，让预览打开就是一份完整公文。
    ///
    /// 用户自己的词库还没有这些词条时，校验器会给出「不在标准词库中」的软提示；
    /// 示例稿不改动用户词库，提示留给用户自行取舍。
    pub(crate) fn draft(&self, config: &AppConfig) -> DraftInput {
        let mut profile = config.profile(self.kind);
        profile.kind = self.kind;
        profile.security_level = "内部".into();
        profile.security_period.clear();
        profile.document_year = Local::now().format("%Y").to_string();
        match self.kind {
            TemplateKind::OfficialLetter => {
                profile.issuing_unit = "某某单位".into();
                profile.recipient = "各有关单位".into();
                profile.copies_to = "某某单位办公室".into();
                profile.responsible_unit = "综合处".into();
                profile.contact_person = "张三".into();
                profile.contact_phone = "000-12345678".into();
                profile.signing_unit = "某某单位".into();
                profile.document_number = "12".into();
                crate::element_fields::sync_letter_department_code(
                    &mut profile,
                    &config.vocabulary,
                );
                // 新装或词库还没绑定代字时给一个示例代字，预览才是一份完整函稿。
                if profile.department_code.trim().is_empty() {
                    profile.department_code = "某函".into();
                }
            }
            TemplateKind::WhitePaper => {
                profile.reporting_leaders = "张三".into();
                profile.signing_unit = "某某单位".into();
                profile.responsible_unit = "综合处".into();
                profile.contact_person = "张三".into();
                profile.contact_phone = "000-12345678".into();
            }
            _ => {}
        }
        DraftInput {
            kind: self.kind,
            title_hint: self.title.into(),
            profile,
            ..Default::default()
        }
    }

    fn new_manuscript(&self, config: &AppConfig) -> NewManuscript {
        NewManuscript {
            snapshot: self.draft(config),
            content_markdown: self.markdown.to_string(),
            notes: "首次启动载入的示例稿，可直接修改或删除。".into(),
            status: ManuscriptStatus::Draft,
            created_at: None,
            updated_at: None,
            published_at: None,
            archived_at: None,
        }
    }
}

/// 把尚未写入的示例稿补进稿件库，返回新建数量。
///
/// `samples_seeded` 为真时直接返回；即使标记丢失，也按标题去重，
/// 保证重复调用只留下同样两篇。
pub(crate) fn seed(store: &mut ManuscriptStore, config: &mut AppConfig) -> Result<usize> {
    if config.samples_seeded {
        return Ok(0);
    }
    let existing = store
        .list(&ManuscriptFilter::default())?
        .into_iter()
        .map(|row| row.title)
        .collect::<std::collections::HashSet<_>>();
    let mut created = 0;
    for sample in &DOCUMENTS {
        if existing.contains(sample.title) {
            continue;
        }
        store.create(&sample.new_manuscript(config), None)?;
        created += 1;
    }
    config.samples_seeded = true;
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{MarkdownBlock, attachment_names, parse_markdown};
    use crate::validator;
    use std::path::Path;

    #[test]
    fn sample_titles_match_their_markdown() {
        for sample in &DOCUMENTS {
            assert_eq!(
                crate::export::extract_title(sample.markdown, ""),
                sample.title
            );
        }
    }

    /// 两篇示例都要过解析器：语法不漏进正文、附件区段结构完整。
    /// 再用空词库跑一遍校验器，确认要素齐全、不会被拦在导出前。
    #[test]
    fn samples_parse_and_validate() {
        let config = AppConfig::default();
        for sample in &DOCUMENTS {
            let blocks = parse_markdown(sample.markdown);
            assert!(
                matches!(blocks.first(), Some(MarkdownBlock::Title(_))),
                "{} 第一块应是文档标题",
                sample.title
            );
            for block in &blocks {
                if let MarkdownBlock::Paragraph(text) = block {
                    assert!(
                        !text.starts_with("<!--")
                            && !text.starts_with('#')
                            && !text.starts_with('|'),
                        "{} 有语法漏进正文：{text}",
                        sample.title
                    );
                }
            }
            if sample.kind == TemplateKind::OfficialLetter {
                assert_eq!(attachment_names(&blocks).len(), 1);
            } else {
                assert!(attachment_names(&blocks).is_empty());
            }

            let draft = sample.draft(&config);
            let warnings = validator::validate(
                &draft,
                sample.markdown,
                &config.vocabulary,
                &config.security_rules,
            );
            assert!(
                warnings.is_empty(),
                "{} 的示例要素应当能直接成篇，校验告警：{warnings:?}",
                sample.title
            );
        }
    }

    /// 连跑两次（第二次故意清掉标记）只应留下两篇。
    #[test]
    fn seeding_is_idempotent() {
        let mut store = ManuscriptStore::open(Path::new(":memory:")).expect("内存稿件库应能打开");
        let mut config = AppConfig::default();
        assert_eq!(seed(&mut store, &mut config).unwrap(), 2);
        assert!(config.samples_seeded);
        assert_eq!(store.list(&ManuscriptFilter::default()).unwrap().len(), 2);

        config.samples_seeded = false;
        assert_eq!(seed(&mut store, &mut config).unwrap(), 0);
        assert!(config.samples_seeded);
        assert_eq!(store.list(&ManuscriptFilter::default()).unwrap().len(), 2);
    }
}
