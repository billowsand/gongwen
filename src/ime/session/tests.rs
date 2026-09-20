//! 组句与键盘接管的测试。
//!
//! 用一份手写的迷你词库（TSV）装配真引擎，所以不需要随包的 `.qj` 数据也能跑，
//! 而且测的是整条链路：事件进 → 引擎 → 事件出。

use super::*;
use crate::ime::data::ImeData;
use crate::ime::engine;

/// 一份能打出「开发 / 开放」的词库。
const MINI_DICT: &str = "开发\tkai fa\t9000\n开放\tkai fang\t8000\n";

/// 用迷你词库装一个可用的输入法。
fn mini_ime() -> Ime {
    let dir = tempfile::tempdir().expect("临时目录");
    let dict = dir.path().join("dict.tsv");
    std::fs::write(&dict, MINI_DICT).expect("写词库");
    let data = ImeData { dict, lm: None };
    let assembly = engine::assemble(&data, None, &dir.path().join("dicts")).expect("装配引擎");
    Ime::for_test(assembly, ImeSettings::default())
}

/// 编辑框每帧报给后端的 IME 输出。
fn ime_output() -> egui::output::IMEOutput {
    egui::output::IMEOutput {
        rect: egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 300.0)),
        cursor_rect: egui::Rect::from_min_size(egui::pos2(120.0, 200.0), egui::vec2(1.0, 20.0)),
        should_interrupt_composition: false,
    }
}

/// 跑一帧，返回这一帧最终的事件队列。
///
/// 帧内先摆好「编辑框持有焦点」的样子（`output.ime` 非空），`end_frame` 据此记下
/// `editable_focus`，下一帧才接管键盘；所以调用方要先跑一帧空事件。
fn frame(ctx: &egui::Context, ime: &mut Ime, events: Vec<egui::Event>) -> Vec<egui::Event> {
    let input = egui::RawInput {
        events,
        ..Default::default()
    };
    let mut routed = Vec::new();
    let _ = ctx.run_ui(input, |ui| {
        let ctx = ui.ctx();
        ctx.output_mut(|output| output.ime = Some(ime_output()));
        ime.begin_frame(ctx);
        routed = ctx.input(|i| i.events.clone());
        ime.end_frame(ctx);
    });
    routed
}

/// 让输入法认为编辑框持有焦点。
fn focus(ctx: &egui::Context, ime: &mut Ime) {
    let _ = frame(ctx, ime, Vec::new());
}

/// 敲一个字符。
fn type_char(ctx: &egui::Context, ime: &mut Ime, c: char) -> Vec<egui::Event> {
    frame(ctx, ime, vec![egui::Event::Text(c.to_string())])
}

/// 事件队列里插进文本框的文本（可能有几段）。
fn inserted(events: &[egui::Event]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            egui::Event::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn key_event(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    }
}

#[test]
fn single_char_only_accepts_one_character() {
    assert_eq!(single_char("a"), Some('a'));
    assert_eq!(single_char("开发"), None);
    assert_eq!(single_char(""), None);
}

/// 敲完整串拼音再空格，文本里出现的是中文，拼音一个都没漏出去。
#[test]
fn typing_pinyin_and_pressing_space_commits_chinese() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);

    for c in "kaifa".chars() {
        let routed = type_char(&ctx, &mut ime, c);
        assert!(routed.is_empty(), "拼音不该漏进文本框：{routed:?}");
    }
    assert_eq!(ime.preedit.text, "kai'fa", "候选窗里显示切分后的拼音");

    let routed = type_char(&ctx, &mut ime, ' ');
    assert_eq!(inserted(&routed), "开发", "空格上屏首选");
    assert!(ime.preedit.text.is_empty(), "上屏后拼音串要清干净");
}

/// 数字键选当页第几个候选。
#[test]
fn digits_pick_a_candidate() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    for c in "kaifa".chars() {
        let _ = type_char(&ctx, &mut ime, c);
    }
    let routed = type_char(&ctx, &mut ime, '2');
    assert_eq!(inserted(&routed), "开放");
}

/// 退格删一个字母，拼音跟着短一位。
#[test]
fn backspace_removes_one_letter() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    for c in "kaifan".chars() {
        let _ = type_char(&ctx, &mut ime, c);
    }
    assert_eq!(ime.preedit.text, "kai'fan");
    let routed = frame(
        &ctx,
        &mut ime,
        vec![key_event(egui::Key::Backspace, true, egui::Modifiers::NONE)],
    );
    assert!(routed.is_empty(), "退格被输入法吃掉");
    assert_ne!(ime.preedit.text, "kai'fan", "拼音串应当变短");
}

/// Esc 丢掉整段，不往文本框里塞东西。
#[test]
fn escape_clears_the_composition() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    for c in "kaifa".chars() {
        let _ = type_char(&ctx, &mut ime, c);
    }
    let routed = frame(
        &ctx,
        &mut ime,
        vec![key_event(egui::Key::Escape, true, egui::Modifiers::NONE)],
    );
    assert!(routed.is_empty());
    assert!(ime.preedit.text.is_empty());
}

/// 单击 Shift 切中英；英文模式下字母原样进文本框。
#[test]
fn clicking_shift_toggles_english() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    assert!(!ime.english(), "默认中文");

    let shift = egui::Modifiers {
        shift: true,
        ..egui::Modifiers::NONE
    };
    let _ = frame(
        &ctx,
        &mut ime,
        vec![key_event(egui::Key::ShiftLeft, true, shift)],
    );
    let _ = frame(
        &ctx,
        &mut ime,
        vec![key_event(
            egui::Key::ShiftLeft,
            false,
            egui::Modifiers::NONE,
        )],
    );
    assert!(ime.english(), "单击 Shift 之后是英文");

    let routed = type_char(&ctx, &mut ime, 'a');
    assert_eq!(inserted(&routed), "a", "英文模式的字母直接进文本框");
}

/// Shift 中间夹了别的键就不算单击，模式不变。
#[test]
fn shift_with_another_key_in_between_is_not_a_click() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);

    let shift = egui::Modifiers {
        shift: true,
        ..egui::Modifiers::NONE
    };
    let _ = frame(
        &ctx,
        &mut ime,
        vec![key_event(egui::Key::ShiftLeft, true, shift)],
    );
    let _ = type_char(&ctx, &mut ime, 'k');
    let _ = frame(
        &ctx,
        &mut ime,
        vec![key_event(
            egui::Key::ShiftLeft,
            false,
            egui::Modifiers::NONE,
        )],
    );
    assert!(!ime.english(), "Shift 中间夹了字母，不该切模式");
}

/// 带 Ctrl 的组合键一律交给应用。
#[test]
fn command_combinations_pass_through() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);

    let ctrl = egui::Modifiers {
        ctrl: true,
        ..egui::Modifiers::NONE
    };
    let event = key_event(egui::Key::S, true, ctrl);
    let routed = frame(&ctx, &mut ime, vec![event.clone()]);
    assert_eq!(routed, vec![event], "Ctrl+S 必须原样交给应用");
}

/// 中文模式里敲大写字母：拼音原样上屏 + 这个字母。
#[test]
fn uppercase_letters_flush_the_pinyin_first() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    for c in "kai".chars() {
        let _ = type_char(&ctx, &mut ime, c);
    }
    let routed = type_char(&ctx, &mut ime, 'A');
    assert_eq!(inserted(&routed), "kaiA");
    assert!(ime.preedit.text.is_empty());
}

/// 全角标点：引擎转得了的字符走我们的上屏。
#[test]
fn full_width_punctuation_is_committed_as_full_width() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    let routed = type_char(&ctx, &mut ime, '。');
    assert_eq!(inserted(&routed), "。");
}

/// 中文模式敲字母是**开始组句**，不是往文本框里插字母。
#[test]
fn letters_start_a_composition_instead_of_being_inserted() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    let routed = type_char(&ctx, &mut ime, 'z');
    assert!(routed.is_empty(), "拼音不该漏进文本框：{routed:?}");
    assert_eq!(ime.preedit.text, "z");
}

/// 关掉输入法时一个事件都不动，系统输入法照常。
#[test]
fn disabled_ime_leaves_the_keyboard_alone() {
    let ctx = egui::Context::default();
    let mut ime = Ime::new(ImeSettings {
        enabled: false,
        ..ImeSettings::default()
    });
    assert!(!ime.active());

    let event = egui::Event::Text("k".to_string());
    let routed = frame(&ctx, &mut ime, vec![event.clone()]);
    assert_eq!(routed, vec![event]);
}

/// 不接管键盘时 `output.ime` 要留给系统输入法（且光标矩形被收窄成光标那一行）。
#[test]
fn inactive_ime_keeps_the_system_input_method_reporting() {
    let ctx = egui::Context::default();
    let mut ime = Ime::new(ImeSettings {
        enabled: false,
        ..ImeSettings::default()
    });
    let mut seen = None;
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        let ctx = ui.ctx();
        ctx.output_mut(|output| output.ime = Some(ime_output()));
        ime.begin_frame(ctx);
        ime.end_frame(ctx);
        seen = ctx.output(|output| output.ime);
    });
    let seen = seen.expect("系统输入法的输出应当保留");
    assert_eq!(seen.rect, ime_output().cursor_rect);
}

/// 接管键盘时把 `output.ime` 摘掉：这就是「关掉系统输入法」。
#[test]
fn active_ime_turns_the_system_input_method_off() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    let mut seen = Some(ime_output());
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        let ctx = ui.ctx();
        ctx.output_mut(|output| output.ime = Some(ime_output()));
        ime.begin_frame(ctx);
        ime.end_frame(ctx);
        seen = ctx.output(|output| output.ime);
    });
    assert!(seen.is_none(), "接管键盘时系统输入法必须关掉");
    // 光标矩形记下来给候选窗定位。
    assert_eq!(ime.anchor, Some(ime_output().cursor_rect));
}
