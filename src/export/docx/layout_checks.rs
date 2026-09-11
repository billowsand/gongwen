//! 与 TeX 共用输入的版式回归样稿，供 Word / XeLaTeX 实际排版检查。
#![allow(clippy::field_reassign_with_default)]

use super::*;

#[test]
#[ignore = "生成实际版式对照文件：cargo test export_layout_comparison -- --ignored"]
fn export_layout_comparison() {
    // 目录可用 GONGWEN_LAYOUT_DIR 覆盖：上一轮的样稿还开在 Word 里时文件被锁住，
    // 换个目录就能重新生成而不打扰正在比对的窗口。
    let dir = match std::env::var_os("GONGWEN_LAYOUT_DIR") {
        Some(value) => std::path::PathBuf::from(value),
        None => std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp/layout-comparison"),
    };
    std::fs::create_dir_all(&dir).unwrap();
    for kind in TemplateKind::ALL {
        let mut input = DraftInput::default();
        input.kind = kind;
        input.date = "2026年9月11日".into();
        input.profile.issuing_unit = "星海省教育厅".into();
        input.profile.recipient = "云川市教育局".into();
        input.profile.reporting_leaders = "张三".into();
        input.profile.signing_unit = "星海省教育厅".into();
        input.profile.department_code = "星教函".into();
        input.profile.document_number = "12".into();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "10年".into();
        input.profile.copies_to = "青林市教育局".into();
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "李四".into();
        input.profile.contact_phone = "01012345678".into();
        let markdown = "# 关于开展教学检查工作的通知\n\n## 工作安排\n\n请各单位于2026年9月20日前报送材料，文件编号ABC123（联系Office456）。\n\n### 报送要求\n\n请认真核对有关数据，确保材料准确完整。\n\n## 组织实施\n\n各单位应明确专人负责，按时完成相关工作。";
        let stem = format!("{kind:?}");
        let display = UnitDisplay::new(&[]);
        write_docx(
            &dir.join(format!("{stem}.docx")),
            &input,
            markdown,
            &display,
        )
        .unwrap();
        crate::export::latex::write_tex(
            &dir.join(format!("{stem}.tex")),
            &input,
            markdown,
            &display,
            &crate::models::FontConfig::default(),
        )
        .unwrap();
    }
}
