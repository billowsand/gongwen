//! 交互状态和本地链接回归；无需构造整个应用或启动模型。

use super::*;

#[test]
fn switching_files_and_skills_preserves_unsaved_buffers() {
    let mut page = SkillsPage {
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
fn links_resolve_within_package_and_reject_external_paths() {
    assert_eq!(
        markdown::resolve("SKILL.md", "references/依据.md#第一节"),
        Some("references/依据.md".into())
    );
    assert_eq!(
        markdown::resolve("references/说明.md", "../assets/example.png"),
        Some("assets/example.png".into())
    );
    assert_eq!(
        markdown::resolve("SKILL.md", "references/%E4%BE%9D%E6%8D%AE.md"),
        Some("references/依据.md".into())
    );
    for path in [
        "../secret.txt",
        "../../secret.txt",
        "C:/secret.txt",
        "https://example.com",
        "//server/file",
        "%2Fsecret.txt",
        "%2e%2e/secret.txt",
    ] {
        assert!(markdown::resolve("SKILL.md", path).is_none(), "{path}");
    }
}

#[test]
fn clicking_a_markdown_reference_opens_the_package_file() {
    let ctx = egui::Context::default();
    let mut files = package::Package::default();
    files
        .files
        .insert("references/依据.md".into(), b"reference".to_vec());
    let size = egui::vec2(600.0, 300.0);
    let frame = |events| {
        let mut target = None;
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| {
                target =
                    markdown::preview(ui, "[打开依据](references/依据.md)", "SKILL.md", &files);
            },
        );
        (output, target)
    };
    let (output, _) = frame(Vec::new());
    let pos = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.text() == "打开依据" => {
                Some(text.pos + text.galley.rect.center().to_vec2())
            }
            _ => None,
        })
        .expect("链接已经渲染");
    frame(vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        },
    ]);
    let (_, target) = frame(vec![egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    }]);
    assert_eq!(target, Some("references/依据.md".into()));
}

#[test]
fn flow_cards_wrap_within_the_panel_and_stack_conditions_below_titles() {
    let ctx = egui::Context::default();
    for skill in skill::builtin_skills() {
        let mut flows = vec![skill.flow.as_slice()];
        while let Some(steps) = flows.pop() {
            flows.extend(
                steps
                    .iter()
                    .filter(|s| !s.body.is_empty())
                    .map(|s| s.body.as_slice()),
            );
            for width in [140.0, 360.0, 720.0, 1100.0] {
                let mut bounds = egui::Rect::NOTHING;
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width + 40.0, 2000.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        ui.set_width(width);
                        bounds = ui.available_rect_before_wrap();
                        detail::flow_ui(ui, steps, 8);
                    },
                );
                let cards: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Rect(rect) if rect.fill == theme::surface_sunk() => {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(cards.len(), steps.len());
                for (index, card) in cards.iter().enumerate() {
                    assert!(
                        card.left() >= bounds.left() - 0.1 && card.right() <= bounds.right() + 0.1,
                        "{} 第 {} 步越界：{card:?}，容器 {bounds:?}",
                        skill.id,
                        index + 1
                    );
                    if index > 0 {
                        let previous = cards[index - 1];
                        if card.left() >= previous.right() {
                            assert!(
                                (card.top() - previous.top()).abs() < 0.1,
                                "同行卡片应顶部对齐"
                            );
                        }
                        assert!(
                            card.left() >= previous.right() || card.top() >= previous.bottom(),
                            "流程卡片发生重叠"
                        );
                    }
                    let texts: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::epaint::Shape::Text(text) if card.contains(text.pos) => {
                                Some(text)
                            }
                            _ => None,
                        })
                        .collect();
                    assert_eq!(texts.len(), if steps[index].when.is_some() { 2 } else { 1 });
                    for text in &texts {
                        let rect = text.galley.rect.translate(text.pos.to_vec2());
                        assert!(
                            card.expand(0.1).contains_rect(rect),
                            "卡片文字越界：{rect:?}"
                        );
                    }
                    if texts.len() == 2 {
                        assert!(
                            texts[1].pos.y >= texts[0].pos.y + texts[0].galley.size().y,
                            "条件说明应位于步骤标题下方"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn overview_and_files_render_at_wide_and_narrow_widths() {
    let ctx = egui::Context::default();
    theme::configure_icons(&ctx);
    let skills = skill::builtin_skills();
    for width in [520.0, 980.0, 1600.0] {
        for files_mode in [false, true] {
            let mut files = package::Package::default();
            files.files.insert(
                "SKILL.md".into(),
                skill::builtin_text("research-draft")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            );
            let mut page = SkillsPage {
                loaded: true,
                skills: skills.clone(),
                selected: Some("research-draft".into()),
                files_mode,
                file: "SKILL.md".into(),
                package: Some(("research-draft".into(), files)),
                ..Default::default()
            };
            for _ in 0..2 {
                let output = ctx.run_ui(
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
                assert!(!page.dirty("research-draft"));
            }
        }
    }
}

#[test]
#[ignore = "输出技能管理真实界面样张到 tmp/，用于目视检查"]
fn skill_manager_samples() {
    let dir = tempfile::tempdir().unwrap();
    crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
    theme::set_current(crate::models::ThemeName::Green);
    let ctx = egui::Context::default();
    theme::configure_icons(&ctx);
    theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
    theme::configure_style(&ctx);
    ctx.set_pixels_per_point(1.5);
    let mut page = SkillsPage::default();
    page.reload();
    page.select("research-draft");
    let mut canvas = crate::ui_snapshot::Canvas::default();
    let mut shoot = |page: &mut SkillsPage, name: &str, size: egui::Vec2| {
        let mut frame = || {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    theme::card().inner_margin(18).show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        page.ui(ui);
                    });
                },
            )
        };
        for _ in 0..20 {
            canvas.absorb(&frame().textures_delta);
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .join(format!("skill-manager-{name}.png"));
        canvas.render(&ctx, frame(), size, theme::canvas(), &path);
        println!("{}", path.display());
    };
    shoot(&mut page, "overview", egui::vec2(1500.0, 950.0));
    page.select("imitate");
    shoot(&mut page, "imitate", egui::vec2(1500.0, 950.0));
    page.select("policy-report");
    shoot(&mut page, "policy-report", egui::vec2(1500.0, 950.0));
    shoot(&mut page, "policy-report-narrow", egui::vec2(600.0, 950.0));
    page.select("research-draft");
    page.files_mode = true;
    shoot(&mut page, "builtin", egui::vec2(1500.0, 950.0));
    package::copy_for_editing("research-draft").unwrap();
    package::save_file("research-draft", "references/引用规范.md", "# 引用规范\n\n事实、数字与政策表述必须注明来源。\n\n## 核验要求\n\n- 核对文件名称和文号。\n- 未确认的信息保留待核实标记。\n\n[返回入口](../SKILL.md)\n").unwrap();
    package::save_file(
        "research-draft",
        "references/核验清单.md",
        "# 核验清单\n\n| 项目 | 要求 |\n| --- | --- |\n| 数字 | 核对原文 |\n| 日期 | 人工确认 |\n",
    )
    .unwrap();
    package::save_file(
        "research-draft",
        "assets/通知示例.md",
        "# 通知示例\n\n供起草时参考结构。\n",
    )
    .unwrap();
    let entry = format!(
        "{}\n\n## 参考文件\n\n[引用规范](references/引用规范.md)\n\n[核验清单](references/核验清单.md)\n",
        skill_files::source_text("research-draft").unwrap()
    );
    package::save_file("research-draft", "SKILL.md", &entry).unwrap();
    page.reload();
    page.file = "references/引用规范.md".into();
    shoot(&mut page, "files", egui::vec2(1500.0, 950.0));
    page.source_mode = true;
    shoot(&mut page, "editor", egui::vec2(1500.0, 950.0));
    page.source_mode = false;
    shoot(&mut page, "narrow", egui::vec2(600.0, 850.0));
    crate::storage::set_test_config_dir(None);
}
