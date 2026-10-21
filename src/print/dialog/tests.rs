//! 打印对话框的逻辑单测与样张（`print_dialog_samples`）。

use super::*;
use std::path::PathBuf;

fn dialog(page_count: usize) -> PrintDialog {
    PrintDialog::new(
        1,
        PrintRequest {
            path: PathBuf::from("/tmp/请示.pdf"),
            title: "关于××的请示".to_string(),
            page_count,
            current_page: 2,
            hints: super::super::PrintHints::default(),
            pdf_key: None,
        },
        "Office",
        BTreeMap::new(),
    )
}

fn printer(name: &str, is_default: bool) -> PrinterInfo {
    PrinterInfo {
        name: name.to_string(),
        is_default,
        state: PrinterState::Unknown,
    }
}

#[test]
fn remembered_printer_wins_over_the_system_default() {
    let mut dialog = dialog(5);
    dialog.printers_loaded(Ok(vec![printer("HP", true), printer("Office", false)]));
    assert_eq!(dialog.selected.as_deref(), Some("Office"));

    let mut dialog = PrintDialog::new(2, dialog.request.clone(), "已删除的打印机", BTreeMap::new());
    dialog.printers_loaded(Ok(vec![printer("HP", false), printer("Canon", true)]));
    assert_eq!(dialog.selected.as_deref(), Some("Canon"));

    dialog.printers_loaded(Ok(vec![printer("HP", false)]));
    assert_eq!(dialog.selected.as_deref(), Some("HP"));
}

#[test]
fn failed_listing_falls_back_to_the_system_default() {
    let mut dialog = dialog(5);
    dialog.printers_loaded(Err("未找到 lpstat".to_string()));
    assert_eq!(dialog.selected, None);
    let job = dialog.job(&dialog.pages()).expect("仍可打印");
    assert_eq!(job.printer, None);
}

#[test]
fn state_updates_reach_the_matching_printer() {
    let mut dialog = dialog(5);
    dialog.printers_loaded(Ok(vec![printer("HP", true)]));
    dialog.printer_state("HP", PrinterState::Offline);
    dialog.printer_state("不存在", PrinterState::Ready);
    let Printers::Loaded(printers) = &dialog.printers else {
        panic!("清单应已载入");
    };
    assert_eq!(printers[0].state, PrinterState::Offline);
}

#[test]
fn page_choices_become_spans() {
    let mut dialog = dialog(10);
    dialog.printers_loaded(Ok(vec![printer("Office", true)]));
    let job = dialog.job(&dialog.pages()).expect("全部");
    assert_eq!(job.pages, None);

    dialog.range = Range::Current;
    let job = dialog.job(&dialog.pages()).expect("当前页");
    assert_eq!(job.pages, Some(vec![(3, 3)]));

    dialog.range = Range::Custom;
    dialog.custom = "1-3，5".to_string();
    dialog.copies = 2;
    dialog.collate = false;
    let job = dialog.job(&dialog.pages()).expect("自定义");
    assert_eq!(job.pages, Some(vec![(1, 3), (5, 5)]));
    assert_eq!((job.copies, job.collate), (2, false));

    dialog.custom = "12".to_string();
    assert!(dialog.job(&dialog.pages()).is_none());
}

#[test]
fn single_copy_never_asks_for_collation() {
    let dialog = dialog(3);
    let job = dialog.job(&dialog.pages()).expect("全部");
    assert_eq!((job.copies, job.collate), (1, false));
}

fn prefs(duplex: Option<Duplex>, scaling: Scaling) -> PrinterPrefs {
    PrinterPrefs {
        duplex,
        color: Some(ColorMode::Mono),
        paper: Some("8".to_string()),
        tray: Some("7".to_string()),
        scaling,
        manual_reverse: true,
    }
}

fn dialog_with_prefs(prefs: BTreeMap<String, PrinterPrefs>) -> PrintDialog {
    PrintDialog::new(
        1,
        PrintRequest {
            path: PathBuf::from("/tmp/请示.pdf"),
            title: "关于××的请示".to_string(),
            page_count: 12,
            current_page: 2,
            hints: super::super::PrintHints::default(),
            pdf_key: None,
        },
        "Office",
        prefs,
    )
}

fn office_prefs(prefs: PrinterPrefs) -> BTreeMap<String, PrinterPrefs> {
    BTreeMap::from([("Office".to_string(), prefs)])
}

#[test]
fn remembered_preferences_fill_the_controls_when_nothing_else_does() {
    let mut dialog = dialog_with_prefs(office_prefs(prefs(Some(Duplex::ShortEdge), Scaling::Fit)));
    dialog.printers_loaded(Ok(vec![printer("Office", true)]));
    dialog.caps_loaded("Office", caps(true, true));
    assert_eq!(dialog.duplex, Some(Duplex::ShortEdge));
    assert_eq!(dialog.color, Some(ColorMode::Mono));
    assert_eq!(dialog.paper.as_deref(), Some("8"));
    assert_eq!(dialog.tray.as_deref(), Some("7"));
    assert_eq!(dialog.scaling, Scaling::Fit);
    assert!(dialog.manual_reverse);
    // 预填不算「用户动过」，所以能力重算时还能被文档默认值盖掉。
    assert!(!dialog.duplex_touched);
}

#[test]
fn a_choice_the_user_made_wins_over_remembered_preferences() {
    let mut dialog = dialog_with_prefs(office_prefs(prefs(Some(Duplex::ShortEdge), Scaling::Fit)));
    dialog.printers_loaded(Ok(vec![printer("Office", true)]));
    dialog.caps_loaded("Office", caps(true, true));
    dialog.duplex = Some(Duplex::Simplex);
    dialog.duplex_touched = true;
    dialog.scaling = Scaling::Actual;
    dialog.scaling_touched = true;
    dialog.request.hints = super::super::PrintHints {
        duplex_typeset: true,
        send_package: false,
    };
    dialog.facts_loaded(None);
    assert_eq!(dialog.duplex, Some(Duplex::Simplex));
    assert_eq!(dialog.scaling, Scaling::Actual);
}

#[test]
fn the_document_default_wins_over_remembered_preferences() {
    let mut dialog = dialog_with_prefs(office_prefs(prefs(
        Some(Duplex::ShortEdge),
        Scaling::Actual,
    )));
    dialog.request.hints = super::super::PrintHints {
        duplex_typeset: true,
        send_package: false,
    };
    dialog.printers_loaded(Ok(vec![printer("Office", true)]));
    dialog.caps_loaded("Office", caps(true, true));
    assert_eq!(dialog.duplex, Some(Duplex::LongEdge));
    // 整份横排再盖一次：短边。
    dialog.facts_loaded(Some(PdfFacts {
        all_rotated: true,
        has_red: false,
    }));
    assert_eq!(dialog.duplex, Some(Duplex::ShortEdge));
}

#[test]
fn without_preferences_the_printer_default_stays() {
    let mut dialog = dialog(12);
    dialog.printers_loaded(Ok(vec![printer("Office", true)]));
    dialog.caps_loaded("Office", caps(true, true));
    assert_eq!(dialog.duplex, None);
    assert_eq!(dialog.color, None);
    assert_eq!(dialog.paper, None);
    assert_eq!(dialog.tray, None);
    assert_eq!(dialog.scaling, Scaling::Actual);
    assert!(!dialog.manual_reverse);
}

#[test]
fn remembered_duplex_is_dropped_on_a_printer_without_the_unit() {
    let mut dialog =
        dialog_with_prefs(office_prefs(prefs(Some(Duplex::LongEdge), Scaling::Actual)));
    dialog.printers_loaded(Ok(vec![printer("Office", true)]));
    dialog.caps_loaded("Office", caps(false, true));
    assert_eq!(dialog.duplex, None);
}

fn caps(duplex: bool, color: bool) -> PrinterCaps {
    use super::super::MediaOption;
    PrinterCaps {
        duplex,
        color,
        papers: vec![
            MediaOption {
                id: "9".to_string(),
                label: "A4".to_string(),
            },
            MediaOption {
                id: "8".to_string(),
                label: "A3".to_string(),
            },
        ],
        default_paper: Some("9".to_string()),
        trays: vec![MediaOption {
            id: "7".to_string(),
            label: "纸盒 1".to_string(),
        }],
        default_tray: Some("7".to_string()),
    }
}

fn ready_dialog() -> PrintDialog {
    let mut dialog = dialog(12);
    dialog.printers_loaded(Ok(vec![printer("Office", true)]));
    dialog.caps_loaded("Office", caps(true, true));
    dialog
}

#[test]
fn capability_choices_become_job_fields() {
    let mut dialog = ready_dialog();
    dialog.duplex = Some(Duplex::ShortEdge);
    dialog.color = Some(ColorMode::Mono);
    dialog.paper = Some("8".to_string());
    dialog.tray = Some("7".to_string());
    dialog.scaling = Scaling::Fit;
    let job = dialog.job(&dialog.pages()).expect("全部");
    assert_eq!(job.duplex, Some(Duplex::ShortEdge));
    assert_eq!(job.color, Some(ColorMode::Mono));
    assert_eq!(job.paper.as_deref(), Some("8"));
    assert_eq!(job.tray.as_deref(), Some("7"));
    assert_eq!(job.scaling, Scaling::Fit);
}

#[test]
fn untouched_options_are_left_to_the_printer() {
    let dialog = ready_dialog();
    let job = dialog.job(&dialog.pages()).expect("全部");
    assert_eq!(job.duplex, None);
    assert_eq!(job.color, None);
    assert_eq!(job.paper, None);
    assert_eq!(job.tray, None);
    assert_eq!(job.scaling, Scaling::Actual);
}

#[test]
fn a_printer_without_duplex_never_gets_a_duplex_choice() {
    let mut dialog = dialog(12);
    dialog.printers_loaded(Ok(vec![printer("Office", true)]));
    dialog.caps_loaded("Office", caps(false, false));
    // 换打印机前留下的长边选择、以及单色机上的彩色选择，都不提交。
    dialog.duplex = Some(Duplex::LongEdge);
    dialog.color = Some(ColorMode::Color);
    let job = dialog.job(&dialog.pages()).expect("全部");
    assert_eq!(job.duplex, None);
    assert_eq!(job.color, None);
}

#[test]
fn document_hints_fill_in_the_duplex_default_until_the_user_touches_it() {
    let mut dialog = dialog(12);
    dialog.request.hints = super::super::PrintHints {
        duplex_typeset: true,
        send_package: false,
    };
    dialog.printers_loaded(Ok(vec![printer("Office", true)]));
    // 能力回来之前不瞎猜。
    assert_eq!(dialog.duplex, None);
    dialog.caps_loaded("Office", caps(true, true));
    assert_eq!(dialog.duplex, Some(Duplex::LongEdge));
    // 事实回来（整份横排）后改成短边。
    dialog.facts_loaded(Some(PdfFacts {
        all_rotated: true,
        has_red: false,
    }));
    assert_eq!(dialog.duplex, Some(Duplex::ShortEdge));
    assert_eq!(
        dialog.job(&dialog.pages()).expect("全部").duplex,
        Some(Duplex::ShortEdge)
    );
}

#[test]
fn a_duplex_choice_the_user_made_wins_over_the_document_default() {
    let mut dialog = dialog(12);
    dialog.request.hints = super::super::PrintHints {
        duplex_typeset: true,
        send_package: false,
    };
    dialog.printers_loaded(Ok(vec![printer("Office", true)]));
    dialog.duplex = Some(Duplex::Simplex);
    dialog.duplex_touched = true;
    dialog.caps_loaded("Office", caps(true, true));
    assert_eq!(dialog.duplex, Some(Duplex::Simplex));
}

#[test]
fn no_duplex_default_on_a_printer_without_the_unit() {
    let mut dialog = dialog(12);
    dialog.request.hints = super::super::PrintHints {
        duplex_typeset: false,
        send_package: true,
    };
    dialog.printers_loaded(Ok(vec![printer("Office", true)]));
    dialog.caps_loaded("Office", caps(false, true));
    assert_eq!(dialog.duplex, None);
}

#[test]
fn manual_duplex_submits_the_front_pass_and_waits_for_the_user() {
    let mut dialog = dialog(5);
    dialog.printers_loaded(Ok(vec![printer("HP", true)]));
    // 没有双面单元也能选手动双面：每一轮只打单面。
    dialog.caps_loaded("HP", caps(false, true));
    dialog.duplex = Some(Duplex::Manual);
    dialog.duplex_touched = true;
    let pages = dialog.pages();
    let outcome = dialog.print_outcome(&pages).expect("应能打印");
    let DialogOutcome::Print(job) = outcome else {
        panic!("手动双面第一轮要交给外壳");
    };
    assert_eq!(job.copies, 1);
    assert_eq!(job.duplex, Some(Duplex::Manual));
    assert_eq!(job.pages, Some(vec![(1, 1), (3, 3), (5, 5)]));
    assert!(!job.reverse);
    assert_eq!(
        dialog.stage,
        Stage::Printing {
            manual: Some(ManualPass::First)
        }
    );
    assert!(!dialog.manual_finished(&Ok(PrintOutcome::Printed { job: None })));
    assert_eq!(dialog.stage, Stage::WaitingFlip);
    assert!(
        dialog
            .manual_passes
            .as_ref()
            .is_some_and(|passes| passes.odd)
    );

    dialog.manual_reverse = true;
    let outcome = dialog.begin_second_pass().expect("继续打印反面");
    let DialogOutcome::Print(job) = outcome else {
        panic!("第二轮也要交给外壳");
    };
    assert_eq!(job.pages, Some(vec![(2, 2), (4, 4)]));
    assert!(job.reverse);
    assert!(dialog.manual_finished(&Ok(PrintOutcome::Printed { job: None })));
}

#[test]
fn a_failed_manual_pass_comes_back_with_the_error() {
    let mut dialog = dialog(5);
    dialog.printers_loaded(Ok(vec![printer("HP", true)]));
    dialog.caps_loaded("HP", caps(true, true));
    dialog.duplex = Some(Duplex::Manual);
    dialog.duplex_touched = true;
    let pages = dialog.pages();
    dialog.print_outcome(&pages).expect("第一轮");
    assert!(!dialog.manual_finished(&Err("卡纸".to_string())));
    assert_eq!(dialog.stage, Stage::Editing);
    assert!(
        dialog
            .manual_error
            .as_deref()
            .is_some_and(|error| error.contains("卡纸"))
    );
}

#[test]
fn cancelling_the_front_pass_returns_to_editing_instead_of_waiting_for_a_flip() {
    let mut dialog = dialog(5);
    dialog.printers_loaded(Ok(vec![printer("HP", true)]));
    dialog.caps_loaded("HP", caps(false, true));
    dialog.duplex = Some(Duplex::Manual);
    dialog.duplex_touched = true;
    let pages = dialog.pages();
    dialog.print_outcome(&pages).expect("第一轮");
    // 正面一张没出完就取消了：没有可翻的纸，不能停在「等待翻面」。
    assert!(!dialog.manual_finished(&Ok(PrintOutcome::Cancelled)));
    assert_eq!(dialog.stage, Stage::Editing);
    assert!(dialog.manual_error.is_some());
}

#[test]
fn toggling_reverse_survives_later_default_recalculation() {
    let mut dialog = dialog(5);
    dialog.printers_loaded(Ok(vec![printer("HP", true)]));
    dialog.manual_reverse = true;
    dialog.manual_reverse_dirty = true;
    let (name, prefs) = dialog.take_preferences_update().expect("要写回偏好");
    assert_eq!(name, "HP");
    assert!(prefs.manual_reverse);
    // 能力随后才到，预填值重算一遍，刚改的倒序不能被旧偏好改回去。
    dialog.caps_loaded("HP", caps(false, true));
    assert!(dialog.manual_reverse);
}

#[test]
fn manual_duplex_needs_a_known_page_count() {
    let mut dialog = dialog(0);
    dialog.printers_loaded(Ok(vec![printer("HP", true)]));
    dialog.caps_loaded("HP", caps(true, true));
    dialog.duplex = Some(Duplex::Manual);
    dialog.duplex_touched = true;
    let pages = dialog.pages();
    assert!(dialog.print_outcome(&pages).is_none());
    assert!(
        dialog
            .manual_error
            .as_deref()
            .is_some_and(|error| error.contains("页数未知"))
    );
}

#[test]
fn preview_pages_follow_the_page_selection() {
    let mut dialog = dialog(10);
    let pages = dialog.pages();
    assert_eq!(print_page_numbers(&pages, 10), (1..=10).collect::<Vec<_>>());
    dialog.range = Range::Custom;
    dialog.custom = "1-3,5".to_string();
    let pages = dialog.pages();
    assert_eq!(print_page_numbers(&pages, 10), vec![1, 2, 3, 5]);
}

#[test]
fn common_paper_sizes_are_known_for_the_preview() {
    assert_eq!(paper_size_mm("9", "A4"), Some((210.0, 297.0)));
    assert_eq!(paper_size_mm("A3", "A3"), Some((297.0, 420.0)));
    assert_eq!(paper_size_mm("1", "Letter"), Some((215.9, 279.4)));
    assert_eq!(paper_size_mm("custom", "自定义"), None);
}

#[test]
fn a_plain_print_keeps_the_dialog_open_until_it_finishes() {
    let mut dialog = ready_dialog();
    let pages = dialog.pages();
    let outcome = dialog.print_outcome(&pages).expect("应能打印");
    let DialogOutcome::Print(_) = outcome else {
        panic!("打印要交给外壳");
    };
    assert_eq!(dialog.stage, Stage::Printing { manual: None });
    // 打完就关。
    assert!(dialog.manual_finished(&Ok(PrintOutcome::Printed { job: None })));
}

#[test]
fn progress_and_cancel_are_reset_for_each_round() {
    let mut dialog = ready_dialog();
    dialog.start_printing(None);
    dialog.progress(2, 5);
    assert_eq!(dialog.progress, Some((2, 5)));
    dialog.request_cancel();
    assert!(dialog.cancelling());
    // 下一轮重新开始：取消标志与进度都清掉。
    dialog.start_printing(None);
    assert!(!dialog.cancelling());
    assert_eq!(dialog.progress, None);
}

#[test]
fn caps_are_requested_once_per_printer() {
    let mut dialog = dialog(12);
    assert_eq!(dialog.take_caps_request(), None, "还没选中打印机");
    dialog.printers_loaded(Ok(vec![printer("HP", true), printer("Office", false)]));
    // 记住的是 Office（构造对话框时传入的「上次用的」）。
    dialog.request_caps();
    assert_eq!(dialog.take_caps_request().as_deref(), Some("Office"));
    // 交出去以后、能力回来之前不再要，否则每帧都会起一个线程。
    dialog.request_caps();
    assert_eq!(dialog.take_caps_request(), None);
    dialog.caps_loaded("Office", caps(false, true));
    dialog.request_caps();
    assert_eq!(dialog.take_caps_request(), None, "有了缓存就不再查");
}

#[test]
fn switching_printers_drops_printer_specific_choices_and_queries_the_new_one() {
    let mut dialog = ready_dialog();
    dialog.paper = Some("8".to_string());
    dialog.tray = Some("7".to_string());
    dialog.selected = Some("HP".to_string());
    dialog.printer_changed();
    assert_eq!(dialog.paper, None);
    assert_eq!(dialog.tray, None);
    dialog.request_caps();
    assert_eq!(dialog.take_caps_request().as_deref(), Some("HP"));
}

#[test]
fn unknown_page_count_only_allows_printing_everything() {
    let mut dialog = dialog(0);
    assert!(dialog.job(&dialog.pages()).is_some());
    dialog.range = Range::Current;
    assert!(dialog.pages().is_err());
}

fn snapshot(dialog: &mut PrintDialog, name: &str) {
    use crate::theme;
    theme::set_current(crate::models::ThemeName::Mdex);
    let ctx = egui::Context::default();
    theme::configure_icons(&ctx);
    theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
    theme::configure_style(&ctx);
    // 对话框 500 宽，加上边距与五行选项后高度也要留够。
    let size = egui::vec2(820.0, 780.0);
    let mut canvas = crate::ui_snapshot::Canvas::default();
    let mut frame = || {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            },
            |ui| {
                // 样张里没有页面纹理：预览显示灰色占位。
                let _ = dialog.ui(ui.ctx(), &|_index| None);
            },
        )
    };
    for _ in 0..5 {
        canvas.absorb(&mut frame().textures_delta);
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tmp")
        .join(format!("print-dialog-{name}.png"));
    canvas.render(&ctx, frame(), size, theme::canvas(), &path);
}

#[test]
#[ignore = "生成打印对话框样张，供目视检查"]
fn print_dialog_samples() {
    let mut loading = dialog(12);
    snapshot(&mut loading, "loading");

    let mut ready = dialog(12);
    ready.printers_loaded(Ok(vec![
        printer("HP_LaserJet_M403", true),
        printer("Office", false),
    ]));
    ready.printer_state("Office", PrinterState::Ready);
    ready.caps_loaded("Office", caps(true, true));
    snapshot(&mut ready, "ready");

    // 整份横排 + 选了长边 + 黑白 + 有红：三条公文提醒同时出现。
    let mut warned = dialog(12);
    warned.request.hints = super::super::PrintHints {
        duplex_typeset: true,
        send_package: false,
    };
    warned.printers_loaded(Ok(vec![printer("Office", true)]));
    warned.caps_loaded("Office", caps(true, true));
    warned.facts_loaded(Some(PdfFacts {
        all_rotated: true,
        has_red: true,
    }));
    warned.duplex = Some(Duplex::LongEdge);
    warned.color = Some(ColorMode::Mono);
    snapshot(&mut warned, "warnings-rotated-red");

    // 页面预览：没有页面纹理时显示灰色占位与提示。
    let mut preview = dialog(12);
    preview.printers_loaded(Ok(vec![printer("Office", true)]));
    preview.caps_loaded("Office", caps(true, true));
    snapshot(&mut preview, "preview-placeholder");

    // 手动双面等翻面（奇数页）：翻面说明、「继续打印反面」与倒序复选框。
    let mut flip = dialog(5);
    flip.printers_loaded(Ok(vec![printer("HP_Deskjet_4900", true)]));
    flip.caps_loaded("HP_Deskjet_4900", caps(false, true));
    flip.duplex = Some(Duplex::Manual);
    flip.duplex_touched = true;
    let pages = flip.pages();
    let _ = flip.print_outcome(&pages);
    let _ = flip.manual_finished(&Ok(PrintOutcome::Printed { job: None }));
    snapshot(&mut flip, "manual-flip-odd");

    // 能力还没回来：five 行只剩一个 spinner。
    let mut loading_caps = dialog(12);
    loading_caps.printers_loaded(Ok(vec![printer("Office", true)]));
    snapshot(&mut loading_caps, "caps-loading");

    // 没有双面单元的打印机：双面置灰，纸盒列表为空也置灰。
    let mut single = dialog(12);
    single.printers_loaded(Ok(vec![printer("HP_DeskJet_4900", true)]));
    single.caps_loaded(
        "HP_DeskJet_4900",
        PrinterCaps {
            color: true,
            papers: vec![super::super::MediaOption {
                id: "9".to_string(),
                label: "A4".to_string(),
            }],
            default_paper: Some("9".to_string()),
            ..Default::default()
        },
    );
    snapshot(&mut single, "caps-single-sided");

    ready.printer_state("Office", PrinterState::Offline);
    ready.copies = 2;
    ready.range = Range::Custom;
    ready.custom = "1-3,5-99".to_string();
    snapshot(&mut ready, "offline-custom");

    ready.custom = "5-3".to_string();
    snapshot(&mut ready, "bad-range");

    // 打印进行中：进度、取消与「转到后台」。
    let mut printing = ready_dialog();
    printing.start_printing(None);
    printing.progress(3, 12);
    snapshot(&mut printing, "printing");

    // 没有双面单元、稿件按双面排：指向手动双面。
    let mut no_unit = dialog(12);
    no_unit.request.hints.duplex_typeset = true;
    no_unit.printers_loaded(Ok(vec![printer("HP_DeskJet", true)]));
    no_unit.caps_loaded("HP_DeskJet", caps(false, true));
    snapshot(&mut no_unit, "no-duplex-unit");

    let mut empty = dialog(12);
    empty.printers_loaded(Ok(Vec::new()));
    snapshot(&mut empty, "no-printer");
}
