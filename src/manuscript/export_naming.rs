//! 导出命名：只有正文与行文要素都符合提交快照时，才能沿用该版本的名称。

use super::ManuscriptStore;
use crate::models::DraftInput;
use anyhow::Result;

impl ManuscriptStore {
    /// 历史版载入时只认该版；平时认最新提交版。未提交修改不会冒用版本名称。
    /// 备注不参与比较，因为备注不属于导出的公文内容。
    pub(crate) fn export_version_name(
        &mut self,
        manuscript_id: i64,
        selected_version: Option<i64>,
        input: &DraftInput,
        markdown: &str,
    ) -> Result<Option<String>> {
        let number = match selected_version {
            Some(number) => Some(number),
            None => self.latest_manuscript_number(manuscript_id)?,
        };
        let Some(number) = number else {
            return Ok(None);
        };
        let Some(version) = self.get_manuscript_version(manuscript_id, number)? else {
            return Ok(None);
        };
        Ok(
            (version.snapshot == *input && version.content_markdown == markdown)
                .then_some(version.name),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::NewManuscript;

    #[test]
    fn export_version_name_requires_matching_content_and_elements() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let input = DraftInput::default();
        let old = "# 通知\n\n初稿正文。";
        let new = "# 通知\n\n修改正文。";
        let id = store
            .create(
                &NewManuscript {
                    snapshot: input.clone(),
                    content_markdown: old.into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(
            store.export_version_name(id, None, &input, old).unwrap(),
            None
        );
        store
            .commit_manuscript_version(id, "送审稿", "", &input, old, "")
            .unwrap();
        assert_eq!(
            store
                .export_version_name(id, None, &input, old)
                .unwrap()
                .as_deref(),
            Some("送审稿")
        );
        // 直接编辑与保存到库后再导出都按导出内容判定，不依赖标签的脏状态。
        assert_eq!(
            store.export_version_name(id, None, &input, new).unwrap(),
            None
        );
        let mut changed = input.clone();
        changed.profile.document_number = "12".into();
        assert_eq!(
            store.export_version_name(id, None, &changed, old).unwrap(),
            None
        );
        store
            .commit_manuscript_version(id, "定稿", "", &input, new, "")
            .unwrap();
        assert_eq!(
            store
                .export_version_name(id, None, &input, new)
                .unwrap()
                .as_deref(),
            Some("定稿")
        );
        assert_eq!(
            store
                .export_version_name(id, Some(1), &input, old)
                .unwrap()
                .as_deref(),
            Some("送审稿")
        );
        assert_eq!(
            store.export_version_name(id, Some(1), &input, new).unwrap(),
            None
        );
        assert_eq!(
            store
                .export_version_name(id, Some(1), &changed, old)
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .export_version_name(id, Some(99), &input, old)
                .unwrap(),
            None
        );
    }
}
