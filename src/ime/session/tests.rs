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

/// 敲一个字符，并把候选窗也画出来（调用顺序与 `App::ui` 一致）。
fn type_char_with_candidates(ctx: &egui::Context, ime: &mut Ime, c: char) {
    let input = egui::RawInput {
        events: vec![egui::Event::Text(c.to_string())],
        ..Default::default()
    };
    let _ = ctx.run_ui(input, |ui| {
        let ctx = ui.ctx();
        ctx.output_mut(|output| output.ime = Some(ime_output()));
        ime.begin_frame(ctx);
        ime.candidates_ui(ctx);
        ime.end_frame(ctx);
    });
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

/// 没在组句时，带 Ctrl 的组合键原样交给应用。
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

/// 大写字母上屏之后再敲一个切不出音节的字母（`Ai` 里的 `i`、`v`）：引擎查不出候选，
/// 拼音串还得画在候选窗里。空候选的布局每页格数不能是 0——候选窗拿它算页数就是除零，
/// panic 从 winit 的窗口回调里穿出去，应用当场闪退。
#[test]
fn unparsable_pinyin_after_an_uppercase_commit_keeps_the_candidate_window_alive() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);

    let routed = type_char(&ctx, &mut ime, 'A');
    assert_eq!(inserted(&routed), "A", "大写字母直接上屏");

    type_char_with_candidates(&ctx, &mut ime, 'v');
    assert_eq!(ime.preedit.text, "v", "切不动的拼音也要显示出来");
    assert_eq!(ime.layout.len(), 0, "这段拼音没有候选");
}

/// 没有候选时的布局照样有每页格数：`CandidateLayout::default()` 的 0 会让 `pages()` 除零。
#[test]
fn an_empty_layout_keeps_a_non_zero_page_size() {
    assert_eq!(empty_layout(5).pages(), 0);
    assert_eq!(empty_layout(0).pages(), 0);
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

/// 公文词表标记按「词 + 读音」对：标了的词才带圆点，同一页的别的词不带；
/// 带标记的候选窗照常画得出来。
#[test]
fn lexicon_marks_follow_word_and_reading() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    ime.lexicon_marks
        .insert(("开放".to_owned(), "kai fang".to_owned()));
    // 同字不同音的不算：基础词库里原来那个读法不该跟着带标记
    ime.lexicon_marks
        .insert(("开发".to_owned(), "kai fa ge".to_owned()));
    focus(&ctx, &mut ime);
    for c in "kaif".chars() {
        type_char_with_candidates(&ctx, &mut ime, c);
    }
    let marked: Vec<(String, bool)> = ime
        .layout
        .page(0)
        .into_iter()
        .filter_map(|cell| cell.candidate())
        .map(|candidate| (candidate.text.clone(), ime.lexicon_marked(candidate)))
        .collect();
    assert!(marked.contains(&("开放".to_owned(), true)), "{marked:?}");
    assert!(marked.contains(&("开发".to_owned(), false)), "{marked:?}");
}

/// 跑一帧，焦点落在 `id` 上、并把它声明成不走输入法的字段（密码框那样）。
fn exempt_frame(
    ctx: &egui::Context,
    ime: &mut Ime,
    id: egui::Id,
    events: Vec<egui::Event>,
) -> Vec<egui::Event> {
    let input = egui::RawInput {
        events,
        ..Default::default()
    };
    let mut routed = Vec::new();
    let _ = ctx.run_ui(input, |ui| {
        let ctx = ui.ctx();
        ctx.memory_mut(|memory| memory.request_focus(id));
        ctx.output_mut(|output| output.ime = Some(ime_output()));
        ime.begin_frame(ctx);
        routed = ctx.input(|i| i.events.clone());
        crate::ime::exempt::declare(ctx, id);
        ime.end_frame(ctx);
    });
    routed
}

/// 声明过不走输入法的字段（密码框）：字母、数字、标点原样进文本框，
/// 不组句（拼音不会明文出现在候选窗里）、不选词、不转全角。
#[test]
fn exempt_fields_bypass_the_ime() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    let id = egui::Id::new("password");
    // 头一帧焦点刚落进来、声明也才第一次出现：下一帧起才生效
    let _ = exempt_frame(&ctx, &mut ime, id, Vec::new());

    let text: Vec<egui::Event> = "kai1,."
        .chars()
        .map(|c| egui::Event::Text(c.to_string()))
        .collect();
    let routed = exempt_frame(&ctx, &mut ime, id, text);
    assert_eq!(inserted(&routed), "kai1,.", "按键必须原样交给文本框");
    assert!(ime.preedit.text.is_empty(), "密码不能出现在拼音串里");
}

/// 声明是逐帧的：控件不再声明，同一个焦点就回到输入法。
#[test]
fn exemption_lapses_when_the_field_stops_declaring() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    let id = egui::Id::new("field");
    let _ = exempt_frame(&ctx, &mut ime, id, Vec::new());
    let _ = exempt_frame(&ctx, &mut ime, id, Vec::new());
    assert!(ime.focus_exempt());

    // 这一帧不声明（同一个焦点）
    let input = egui::RawInput::default();
    let _ = ctx.run_ui(input, |ui| {
        let ctx = ui.ctx();
        ctx.memory_mut(|memory| memory.request_focus(id));
        ctx.output_mut(|output| output.ime = Some(ime_output()));
        ime.begin_frame(ctx);
        ime.end_frame(ctx);
    });
    assert!(!ime.focus_exempt(), "上一帧没声明，就该回到输入法");
}

/// 敲一串拼音。
fn type_str(ctx: &egui::Context, ime: &mut Ime, text: &str) {
    for c in text.chars() {
        let _ = type_char(ctx, ime, c);
    }
}

/// 组句中的快捷键：先把拼音原样上屏，快捷键本身照样交给应用，而且排在拼音后面。
#[test]
fn shortcuts_while_composing_flush_the_pinyin_first() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    type_str(&ctx, &mut ime, "kai");

    let undo = key_event(egui::Key::Z, true, egui::Modifiers::CTRL);
    let routed = frame(&ctx, &mut ime, vec![undo.clone()]);
    assert_eq!(routed, vec![egui::Event::Text("kai".into()), undo]);
    assert!(ime.preedit.text.is_empty());
}

/// 粘贴不走 `Key` 事件，同样要先把拼音上屏，别粘到拼音前面去。
#[test]
fn pasting_while_composing_flushes_the_pinyin_first() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    type_str(&ctx, &mut ime, "kai");

    let paste = egui::Event::Paste("正文".into());
    let routed = frame(&ctx, &mut ime, vec![paste.clone()]);
    assert_eq!(routed, vec![egui::Event::Text("kai".into()), paste]);
}

/// 单按一下 Ctrl 还不是快捷键，拼音留着。
#[test]
fn a_lone_modifier_keeps_the_composition() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    type_str(&ctx, &mut ime, "kai");

    let routed = frame(
        &ctx,
        &mut ime,
        vec![key_event(
            egui::Key::ControlLeft,
            true,
            egui::Modifiers::CTRL,
        )],
    );
    assert!(inserted(&routed).is_empty(), "{routed:?}");
    assert_eq!(ime.preedit.text, "kai");
}

/// Ctrl+退格删的是拼音里最后一个音节，不是候选窗背后的正文。
#[test]
fn ctrl_backspace_deletes_a_syllable_of_the_composition() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    type_str(&ctx, &mut ime, "kaifa");

    let routed = frame(
        &ctx,
        &mut ime,
        vec![key_event(egui::Key::Backspace, true, egui::Modifiers::CTRL)],
    );
    assert!(routed.is_empty(), "不该交给文本框：{routed:?}");
    assert_eq!(ime.preedit.text, "kai");
}

/// Delete 删拼音光标后的字母，不碰正文。
#[test]
fn delete_removes_the_letter_after_the_pinyin_caret() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    type_str(&ctx, &mut ime, "kaifa");

    let none = egui::Modifiers::NONE;
    let _ = frame(&ctx, &mut ime, vec![key_event(egui::Key::Home, true, none)]);
    let routed = frame(
        &ctx,
        &mut ime,
        vec![key_event(egui::Key::Delete, true, none)],
    );
    assert!(routed.is_empty(), "不该交给文本框：{routed:?}");
    let raw = ime.engine().expect("引擎").composition().text().to_owned();
    assert_eq!(raw, "aifa");
}

/// 挪拼音光标后，候选窗里的光标竖线跟着走。
#[test]
fn moving_the_pinyin_caret_updates_the_preedit() {
    let ctx = egui::Context::default();
    let mut ime = mini_ime();
    focus(&ctx, &mut ime);
    type_str(&ctx, &mut ime, "kaifa");
    let end = ime.preedit.caret;

    let none = egui::Modifiers::NONE;
    let _ = frame(
        &ctx,
        &mut ime,
        vec![key_event(egui::Key::ArrowLeft, true, none)],
    );
    assert!(ime.preedit.caret < end, "光标应当左移：{:?}", ime.preedit);

    let _ = frame(&ctx, &mut ime, vec![key_event(egui::Key::Home, true, none)]);
    assert_eq!(ime.preedit.caret, 0);
}
