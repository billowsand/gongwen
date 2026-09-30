//! 检索浮层行为：展开收起、稳定位置、定位及输入法提交。
use super::super::index::Entry;
use super::*;
use crate::app::NavPage;
use egui_kittest::{Harness, kittest::Queryable};

#[test]
fn keyboard_opens_selected_result_and_empty_search_never_opens() {
    let mut state = QuickFind::new(
        vec![
            Entry::new("甲稿".into(), "草稿".into(), "", Target::Manuscript(1)),
            Entry::new("乙稿".into(), "草稿".into(), "", Target::Manuscript(2)),
        ],
        None,
    );
    state.query = "稿".into();
    let mut opened = None;
    let mut harness = Harness::new_ui(|ui| {
        if let Some(target) = state.ui(ui).0 {
            opened = Some(target);
        }
    });
    harness.run();
    harness.key_press(egui::Key::ArrowDown);
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    drop(harness);
    assert_eq!(opened, Some(Target::Manuscript(2)));
    state.query = "完全没有匹配项".into();
    opened = None;
    let mut harness = Harness::new_ui(|ui| {
        if let Some(target) = state.ui(ui).0 {
            opened = Some(target);
        }
    });
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    drop(harness);
    assert_eq!(opened, None);
}

#[test]
fn clicking_a_page_result_returns_its_identity() {
    let mut state = QuickFind::new(
        vec![Entry::new(
            "设置".into(),
            "常用页面".into(),
            "",
            Target::Page(NavPage::Settings),
        )],
        None,
    );
    state.query = "设置".into();
    let mut opened = None;
    let mut harness = Harness::new_ui(|ui| {
        if let Some(target) = state.ui(ui).0 {
            opened = Some(target);
        }
    });
    harness.run();
    harness.get_by_label("设置\n常用页面").click();
    harness.run();
    drop(harness);
    assert_eq!(opened, Some(Target::Page(NavPage::Settings)));
}

#[test]
fn ime_commit_does_not_open_a_result_until_the_next_enter() {
    let mut state = QuickFind::new(
        vec![Entry::new(
            "设置".into(),
            "常用页面".into(),
            "",
            Target::Page(NavPage::Settings),
        )],
        None,
    );
    let mut opened = None;
    let mut harness = Harness::new_ui(|ui| {
        if let Some(target) = state.ui(ui).0 {
            opened = Some(target);
        }
    });
    harness.run();
    harness.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "设置".into(),
        active_range_chars: None,
    }));
    harness.run();
    // event()/key_press() 会逐个事件各跑一帧；这里需要模拟同一帧的选字提交。
    harness.input_mut().events.extend([
        egui::Event::Ime(egui::ImeEvent::Commit("设置".into())),
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    harness.run();
    drop(harness);
    assert_eq!(opened, None);
    let mut harness = Harness::new_ui(|ui| {
        if let Some(target) = state.ui(ui).0 {
            opened = Some(target);
        }
    });
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    drop(harness);
    assert_eq!(opened, Some(Target::Page(NavPage::Settings)));
}

#[test]
fn empty_query_collapses_and_expanding_keeps_the_search_box_in_place() {
    let state = QuickFind::new(
        vec![Entry::new(
            "设置".into(),
            "常用页面".into(),
            "",
            Target::Page(NavPage::Settings),
        )],
        None,
    );
    let mut harness = Harness::new_ui_state(
        |ui, (state, opened): &mut (QuickFind, Option<Target>)| {
            let response = modal(ui.ctx()).show(ui.ctx(), |ui| state.ui(ui));
            if let Some(target) = response.inner.0 {
                *opened = Some(target);
            }
        },
        (state, None),
    );
    harness.set_size(egui::vec2(900.0, 650.0));
    harness.run();
    // Area 在窗口尺寸改变后需要下一帧更新锚点。
    harness.run();
    let empty_rect = harness.ctx.read_response(search_id()).unwrap().rect;
    assert!(harness.query_by_label("设置\n常用页面").is_none());
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().1, None);
    harness.state_mut().0.query = "设".into();
    harness.run();
    assert!(harness.query_by_label("设置\n常用页面").is_some());
    let expanded_rect = harness.ctx.read_response(search_id()).unwrap().rect;
    assert!(
        (empty_rect.top() - expanded_rect.top()).abs() < 0.5,
        "空输入：{empty_rect:?}；展开：{expanded_rect:?}"
    );
    assert!((empty_rect.left() - expanded_rect.left()).abs() < 0.5);
    harness.state_mut().0.query = "   ".into();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert!(harness.query_by_label("设置\n常用页面").is_none());
    assert_eq!(harness.state().1, None);
    assert!(harness.state().0.results.is_empty());
}

#[test]
fn escape_and_keycap_both_close_the_modal() {
    let state = QuickFind::new(Vec::new(), None);
    let mut harness = Harness::new_ui_state(
        |ui, (state, closed): &mut (QuickFind, bool)| {
            let response = modal(ui.ctx()).show(ui.ctx(), |ui| state.ui(ui));
            *closed |= response.inner.1 || response.should_close();
        },
        (state, false),
    );
    harness.run();
    harness.get_by_label("关闭快捷查找").click();
    harness.run();
    assert!(harness.state().1);
    harness.state_mut().1 = false;
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(harness.state().1);
}
