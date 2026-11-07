//! Pi 技能管理页的状态与渲染回归；不启动真实模型，只写临时配置目录。

use super::*;

struct ConfigDir {
    dir: tempfile::TempDir,
}

impl ConfigDir {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        Self { dir }
    }
}

impl Drop for ConfigDir {
    fn drop(&mut self) {
        crate::storage::set_test_config_dir(None);
        let _ = &self.dir;
    }
}

#[test]
fn switching_files_and_skills_preserves_unsaved_buffers() {
    let _config = ConfigDir::new();
    let mut page = PiSkillsPage {
        loaded: true,
        selected: Some("one".into()),
        ..Default::default()
    };
    page.buffers.insert(
        ("one".into(), "references/依据.md".into()),
        Buffer {
            text: "修改后".into(),
            saved: "原文".into(),
        },
    );
    page.file = "references/依据.md".into();
    page.select("two");
    assert_eq!(page.file, "SKILL.md");
    page.select("one");
    assert!(page.dirty("one"));
    assert_eq!(
        page.buffers[&("one".into(), "references/依据.md".into())].text,
        "修改后"
    );
}

#[test]
fn actions_call_the_new_pi_store_and_reload_the_catalog() {
    let _config = ConfigDir::new();
    let mut page = PiSkillsPage::default();
    page.reload();
    assert!(page.catalog.get("gongwen-draft").is_some());
    page.toggle("gongwen-draft", false);
    assert!(!page.catalog.get("gongwen-draft").unwrap().enabled);
    page.toggle("gongwen-draft", true);
    assert!(page.catalog.get("gongwen-draft").unwrap().enabled);

    page.copy_builtin("gongwen-draft");
    assert!(page.catalog.get("gongwen-draft").unwrap().overridden());
    page.open_file("SKILL.md");
    assert!(
        page.buffers
            .contains_key(&("gongwen-draft".into(), "SKILL.md".into()))
    );
    page.buffers
        .get_mut(&("gongwen-draft".into(), "SKILL.md".into()))
        .unwrap()
        .text = "---\nname: gongwen-draft\ndescription: 改坏\n---\n正文".into();
    page.save_current();
    assert!(
        page.message.as_ref().is_some_and(|(ok, _)| *ok),
        "{:?}",
        page.message
    );
    page.restore_builtin("gongwen-draft");
    assert!(!page.catalog.get("gongwen-draft").unwrap().user);
    assert!(!page.dirty("gongwen-draft"));
}

#[test]
fn invalid_save_keeps_the_edit_buffer() {
    let _config = ConfigDir::new();
    pi_skills::store::create("mine", None).unwrap();
    let mut page = PiSkillsPage::default();
    page.reload();
    page.select("mine");
    page.open_file("SKILL.md");
    let key = ("mine".into(), "SKILL.md".into());
    page.buffers.get_mut(&key).unwrap().text = "没有 frontmatter".into();
    page.save_current();
    assert!(!page.message.as_ref().unwrap().0);
    assert_eq!(page.buffers[&key].text, "没有 frontmatter");
    assert_ne!(page.buffers[&key].saved, "没有 frontmatter");
    // 磁盘上仍是可解析的模板。
    assert!(
        pi_skills::store::skill_text("mine")
            .unwrap()
            .contains("name: mine")
    );
}

#[test]
fn a_broken_skill_can_be_opened_and_repaired() {
    let _config = ConfigDir::new();
    pi_skills::store::create("repair-me", None).unwrap();
    let original = pi_skills::store::skill_text("repair-me").unwrap();
    pi_skills::store::corrupt_skill_for_test("repair-me", "坏掉的文件");
    let mut page = PiSkillsPage::default();
    page.reload();
    page.select("repair-me");
    assert!(page.current().unwrap().error.is_some());
    page.open_file("SKILL.md");
    let key = ("repair-me".into(), "SKILL.md".into());
    assert_eq!(page.buffers[&key].text, "坏掉的文件");
    page.buffers.get_mut(&key).unwrap().text = original;
    page.save_current();
    assert!(page.message.as_ref().unwrap().0);
    assert!(page.current().unwrap().error.is_none());
}

#[test]
fn page_renders_wide_narrow_and_disabled_states() {
    let _config = ConfigDir::new();
    pi_skills::store::set_enabled("gongwen-review", false).unwrap();
    pi_skills::store::create("custom-skill", Some("自定义")).unwrap();
    let ctx = egui::Context::default();
    theme::configure_icons(&ctx);
    for width in [520.0, 980.0, 1500.0] {
        for mode in [Mode::Overview, Mode::Files] {
            for selected in ["custom-skill", "gongwen-review"] {
                let mut page = PiSkillsPage {
                    loaded: true,
                    catalog: pi_skills::load_catalog(),
                    selected: Some(selected.into()),
                    mode,
                    file: "SKILL.md".into(),
                    ..Default::default()
                };
                for _ in 0..2 {
                    let output = crate::test_support::run_ui(
                        &ctx,
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 900.0),
                            )),
                            ..Default::default()
                        },
                        |ui| page.ui(ui),
                    );
                    assert!(!output.shapes.is_empty());
                }
            }
        }
    }
}

#[test]
fn delete_is_confirmed_and_removes_the_whole_package() {
    let _config = ConfigDir::new();
    pi_skills::store::create("gone", None).unwrap();
    pi_skills::store::save_text("gone", "references/a.md", "辅助").unwrap();
    let mut page = PiSkillsPage::default();
    page.reload();
    page.select("gone");
    page.confirm_delete = true;
    page.delete("gone");
    assert!(page.catalog.get("gone").is_none());
    assert!(!pi_skills::store::user_dir("gone").unwrap().exists());
}
