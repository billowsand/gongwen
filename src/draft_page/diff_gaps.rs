//! 可编辑统一 diff 的底座：在 `TextEdit` 的排版结果里给「已删除的旧行」让出空隙。
//!
//! egui 的 `TextEdit` 画的就是编辑内容本身，行间插不进不属于文本的东西。做法是
//! 在 `TextEdit::layouter` 里拿到正常排好的 galley，克隆一份，把某一行起之后的
//! 所有 `PlacedRow::pos.y` 往下挪「已删除行」的总高度，同步扩大 `rect` /
//! `mesh_bounds`。`TextEdit` 画字、光标定位、点击命中、选区、滚动到光标、输入法
//! 光标矩形全都读这些行坐标，于是行间天然空出一块；`show` 之后再用 painter 在
//! 空隙里画红底的旧行（只读、不可选中）。
//!
//! 第 ③ 期技术验证（spike）的结论见本文件测试与 `docs/version-diff-handoff.md`。

// spike 阶段只有测试在用；接进版本对照编辑器后去掉这一行。
#![cfg_attr(not(test), allow(dead_code))]

use eframe::egui;
use eframe::egui::epaint::text::Galley;

/// 一处空隙：开在新文本第 `line` 行（0 基源码行）之前；`line` 等于总行数时开在末尾。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Gap {
    pub(crate) line: usize,
    pub(crate) height: f32,
}

/// 每一条源码行的第一个 galley 行下标。源码行以 `\n` 结束；自动折行时一条源码行
/// 占好几个 galley 行，只有最后一个 `ends_with_newline`。
pub(crate) fn line_first_rows(galley: &Galley) -> Vec<usize> {
    let mut firsts = vec![0];
    for (index, row) in galley.rows.iter().enumerate() {
        if row.ends_with_newline {
            firsts.push(index + 1);
        }
    }
    firsts
}

/// 按像素取整：行坐标是按像素对齐的，挪动量不取整字会发虚。
fn round_to_pixels(value: f32, pixels_per_point: f32) -> f32 {
    if pixels_per_point > 0.0 {
        (value * pixels_per_point).round() / pixels_per_point
    } else {
        value
    }
}

/// 在 galley 里开出空隙，返回挪过行的新 galley。空隙按 `line` 严格升序给出
/// （同一行前的多段删除由调用方合成一处）；超出行数的空隙一律开在末尾。
pub(crate) fn open_gaps(galley: &Galley, gaps: &[Gap]) -> Galley {
    debug_assert!(
        gaps.windows(2).all(|pair| pair[0].line < pair[1].line),
        "空隙须按行号严格升序"
    );
    let mut shifted = galley.clone();
    if gaps.is_empty() {
        return shifted;
    }
    let firsts = line_first_rows(galley);
    let ppp = galley.pixels_per_point;
    // 每个 galley 行要往下挪多少：它前面（含同一位置）所有空隙之和。
    let mut offsets = vec![0.0_f32; galley.rows.len()];
    let mut tail = 0.0_f32;
    for gap in gaps {
        let height = round_to_pixels(gap.height, ppp);
        match firsts.get(gap.line).filter(|row| **row < galley.rows.len()) {
            Some(&row) => offsets[row..]
                .iter_mut()
                .for_each(|offset| *offset += height),
            None => tail += height,
        }
    }
    for (placed, offset) in shifted.rows.iter_mut().zip(&offsets) {
        placed.pos.y += offset;
    }
    let grow = offsets.last().copied().unwrap_or(0.0) + tail;
    shifted.rect.max.y += grow;
    if shifted.mesh_bounds.is_positive() {
        shifted.mesh_bounds.max.y += offsets.last().copied().unwrap_or(0.0);
    }
    shifted
}

/// 挪过之后每处空隙在 galley 坐标里的纵向范围 `(上沿, 下沿)`，与 `gaps` 一一对应。
/// 删除的旧行就画在这里。
pub(crate) fn gap_spans(shifted: &Galley, gaps: &[Gap]) -> Vec<(f32, f32)> {
    let firsts = line_first_rows(shifted);
    let ppp = shifted.pixels_per_point;
    let mut tail = shifted.rows.last().map_or(0.0, |row| row.max_y());
    gaps.iter()
        .map(|gap| {
            let height = round_to_pixels(gap.height, ppp);
            match firsts.get(gap.line).and_then(|row| shifted.rows.get(*row)) {
                Some(row) => (row.min_y() - height, row.min_y()),
                None => {
                    let span = (tail, tail + height);
                    tail += height;
                    span
                }
            }
        })
        .collect()
}

/// 空隙的屏幕矩形：`galley_pos` 取自 `TextEditOutput::galley_pos`。
pub(crate) fn gap_rects(
    shifted: &Galley,
    galley_pos: egui::Pos2,
    gaps: &[Gap],
    width: f32,
) -> Vec<egui::Rect> {
    gap_spans(shifted, gaps)
        .into_iter()
        .map(|(top, bottom)| {
            egui::Rect::from_min_max(
                egui::pos2(galley_pos.x, galley_pos.y + top),
                egui::pos2(galley_pos.x + width, galley_pos.y + bottom),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    //! 技术验证清单（docs/version-diff-handoff.md「第 ③ 期实施指南」）：
    //! 每条都在真 egui 上下文里跑真 `TextEdit`，布局器开出空隙。
    use super::*;
    use crate::draft_page::editor::editor_line_visuals;
    use std::sync::Arc;

    const TEXT: &str = "第一行\n第二行\n第三行\n第四行\n第五行";
    /// 空隙开在第三行（0 基第 2 行）之前，高度是两行删除行。
    const GAP_LINE: usize = 2;

    struct Harness {
        ctx: egui::Context,
        text: String,
        gaps: Vec<Gap>,
        clock: f64,
        /// 窗口高度：滚动测试要一个装不下全文的矮窗口。
        height: f32,
    }

    struct Frame {
        galley: Arc<Galley>,
        galley_pos: egui::Pos2,
        cursor: Option<egui::text::CCursorRange>,
        visuals: Vec<(f32, f32)>,
        ime: Option<egui::output::IMEOutput>,
        scroll_offset: f32,
        viewport: egui::Rect,
    }

    impl Harness {
        fn new(text: &str, gaps: Vec<Gap>) -> Self {
            let ctx = egui::Context::default();
            crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
            Self {
                ctx,
                text: text.to_string(),
                gaps,
                clock: 0.0,
                height: 800.0,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Frame {
            self.clock += 0.1;
            let height = self.height;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, height),
                )),
                time: Some(self.clock),
                events,
                ..Default::default()
            };
            let gaps = self.gaps.clone();
            let text = &mut self.text;
            let mut result = None;
            let output = self.ctx.clone().run_ui(raw, |ui| {
                let scroll = egui::ScrollArea::vertical()
                    .id_salt("spike_scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let mut layouter =
                            |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap: f32| {
                                let mut job = egui::text::LayoutJob::simple(
                                    buffer.as_str().to_string(),
                                    egui::FontId::proportional(16.0),
                                    egui::Color32::BLACK,
                                    wrap,
                                );
                                job.sections[0].format.line_height = Some(20.0);
                                let base = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
                                Arc::new(open_gaps(&base, &gaps))
                            };
                        egui::TextEdit::multiline(text)
                            .id(egui::Id::new("spike_editor"))
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .desired_width(500.0)
                            .layouter(&mut layouter)
                            .show(ui)
                    });
                let output = scroll.inner;
                result = Some(Frame {
                    visuals: editor_line_visuals(&output)
                        .into_iter()
                        .map(|line| (line.top, line.bottom))
                        .collect(),
                    galley: output.galley.clone(),
                    galley_pos: output.galley_pos,
                    cursor: output.cursor_range,
                    ime: None,
                    scroll_offset: scroll.state.offset.y,
                    viewport: scroll.inner_rect,
                });
            });
            let mut frame = result.expect("帧里跑过闭包");
            frame.ime = output.platform_output.ime;
            frame
        }

        fn click(&mut self, at: egui::Pos2) -> Frame {
            self.frame(vec![egui::Event::PointerMoved(at)]);
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }]);
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }])
        }

        fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) -> Frame {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }])
        }
    }

    fn gaps() -> Vec<Gap> {
        vec![Gap {
            line: GAP_LINE,
            height: 40.0,
        }]
    }

    /// 源码第 `line` 行的首字符下标（字符计）。
    fn line_start(text: &str, line: usize) -> usize {
        text.split('\n')
            .take(line)
            .map(|line| line.chars().count() + 1)
            .sum()
    }

    /// 屏幕上第 `line` 行正中的一点（行内第 2 个字附近）。
    fn line_point(frame: &Frame, line: usize) -> egui::Pos2 {
        let row = &frame.galley.rows[line_first_rows(&frame.galley)[line]];
        frame.galley_pos + egui::vec2(row.pos.x + 20.0, row.rect().center().y)
    }

    #[test]
    fn rows_after_the_gap_move_down_and_the_galley_grows() {
        let mut harness = Harness::new(TEXT, gaps());
        let frame = harness.frame(Vec::new());
        let rows = &frame.galley.rows;
        assert_eq!(rows.len(), 5);
        let step = rows[1].pos.y - rows[0].pos.y;
        assert!(
            (rows[2].pos.y - rows[1].pos.y - step - 40.0).abs() < 0.5,
            "空隙 40"
        );
        assert!(
            (rows[3].pos.y - rows[2].pos.y - step).abs() < 0.5,
            "空隙之后行距照旧"
        );
        let rects = gap_rects(&frame.galley, frame.galley_pos, &gaps(), 500.0);
        assert_eq!(rects.len(), 1);
        let rect = rects[0];
        let above = frame.galley_pos.y + rows[1].max_y();
        let below = frame.galley_pos.y + rows[2].min_y();
        assert!((rect.top() - above).abs() < 0.5 && (rect.bottom() - below).abs() < 0.5);
    }

    #[test]
    fn clicking_below_the_gap_lands_on_that_line_not_on_the_deleted_one() {
        let mut harness = Harness::new(TEXT, gaps());
        let frame = harness.frame(Vec::new());
        let at = line_point(&frame, GAP_LINE);
        let frame = harness.click(at);
        let cursor = frame.cursor.expect("点过后有光标").primary.index.0;
        let start = line_start(TEXT, GAP_LINE);
        assert!(
            (start..start + 3).contains(&cursor),
            "光标 {cursor} 应在第三行（{start} 起）"
        );
    }

    #[test]
    fn clicking_inside_the_gap_snaps_to_a_neighbouring_line() {
        let mut harness = Harness::new(TEXT, gaps());
        let frame = harness.frame(Vec::new());
        let rows = &frame.galley.rows;
        // 空隙上四分之一：靠上面那行；下四分之一：靠下面那行。
        let top = frame.galley_pos.y + rows[1].max_y();
        let bottom = frame.galley_pos.y + rows[2].min_y();
        let x = frame.galley_pos.x + 20.0;
        let upper = harness.click(egui::pos2(x, top + (bottom - top) * 0.2));
        let cursor = upper.cursor.unwrap().primary.index.0;
        let second = line_start(TEXT, 1);
        assert!(
            (second..second + 4).contains(&cursor),
            "靠上 → 第二行：{cursor}"
        );
        let lower = harness.click(egui::pos2(x, bottom - (bottom - top) * 0.2));
        let cursor = lower.cursor.unwrap().primary.index.0;
        let third = line_start(TEXT, 2);
        assert!(
            (third..third + 4).contains(&cursor),
            "靠下 → 第三行：{cursor}"
        );
    }

    #[test]
    fn dragging_across_the_gap_selects_both_sides() {
        let mut harness = Harness::new(TEXT, gaps());
        let frame = harness.frame(Vec::new());
        let from = line_point(&frame, 1);
        let to = line_point(&frame, 3);
        harness.frame(vec![egui::Event::PointerMoved(from)]);
        harness.frame(vec![egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        for step in 1..=4 {
            let t = step as f32 / 4.0;
            harness.frame(vec![egui::Event::PointerMoved(from + (to - from) * t)]);
        }
        let frame = harness.frame(vec![egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        let range = frame.cursor.expect("拖选后有选区");
        let (lo, hi) = (
            range.primary.index.0.min(range.secondary.index.0),
            range.primary.index.0.max(range.secondary.index.0),
        );
        assert!(
            lo < line_start(TEXT, 2) && hi > line_start(TEXT, 3),
            "{lo}..{hi}"
        );
    }

    #[test]
    fn arrow_keys_step_over_the_gap_one_line_at_a_time() {
        let mut harness = Harness::new(TEXT, gaps());
        let frame = harness.frame(Vec::new());
        harness.click(line_point(&frame, 1));
        let down = harness.key(egui::Key::ArrowDown, egui::Modifiers::NONE);
        let cursor = down.cursor.unwrap().primary.index.0;
        let third = line_start(TEXT, 2);
        assert!(
            (third..third + 4).contains(&cursor),
            "↓ 越过空隙到第三行：{cursor}"
        );
        let up = harness.key(egui::Key::ArrowUp, egui::Modifiers::NONE);
        let cursor = up.cursor.unwrap().primary.index.0;
        let second = line_start(TEXT, 1);
        assert!(
            (second..second + 4).contains(&cursor),
            "↑ 回到第二行：{cursor}"
        );
    }

    /// PageUp / PageDown：egui 的 `TextEdit` 本身就不处理（0.35 源码里没有这两个键
    /// 的分支），现有编辑器也没接。这里只锁住「按了不出错、光标不乱跳」。
    #[test]
    fn page_keys_are_ignored_by_text_edit() {
        let mut harness = Harness::new(TEXT, gaps());
        let frame = harness.frame(Vec::new());
        let before = harness.click(line_point(&frame, 1)).cursor.unwrap();
        let after = harness.key(egui::Key::PageDown, egui::Modifiers::NONE);
        assert_eq!(after.cursor.unwrap(), before);
    }

    #[test]
    fn scrolling_to_the_cursor_accounts_for_the_gap() {
        // 一屏只放得下几行、空隙很高：光标移到空隙后面，滚动区必须把它滚进来。
        //
        // 每按一次键之后空转两帧：真实界面里按键之间总有空闲帧。若一帧一键连发，
        // egui 的滚动目标会在还没落地时被下一个目标叠加，**不开空隙也会**滚过头
        // （无空隙对照实测：该滚 81px 滚了 119px），这是 egui 自身的行为，与空隙无关。
        let text = (1..=30)
            .map(|index| format!("第{index}行"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut harness = Harness::new(
            &text,
            vec![Gap {
                line: 5,
                height: 600.0,
            }],
        );
        harness.height = 200.0;
        let frame = harness.frame(Vec::new());
        harness.click(line_point(&frame, 1));
        for _ in 0..6 {
            harness.key(egui::Key::ArrowDown, egui::Modifiers::NONE);
            harness.frame(Vec::new());
            harness.frame(Vec::new());
        }
        let frame = harness.frame(Vec::new());
        let cursor = frame.cursor.unwrap().primary;
        let rect = frame.galley.pos_from_cursor(cursor);
        let on_screen = rect.translate(frame.galley_pos.to_vec2());
        assert!(
            frame.scroll_offset > 600.0,
            "滚过了空隙：{}",
            frame.scroll_offset
        );
        assert!(
            frame.viewport.contains_rect(on_screen.shrink(1.0)),
            "光标 {on_screen:?} 应在可视区 {:?} 里",
            frame.viewport
        );
    }

    #[test]
    fn line_numbers_and_hybrid_decorations_follow_the_shifted_rows() {
        // 行号与混合模式装饰都走 `editor_line_visuals`，它读的是 galley 行坐标。
        let mut harness = Harness::new(TEXT, gaps());
        let frame = harness.frame(Vec::new());
        assert_eq!(frame.visuals.len(), 5);
        let row = &frame.galley.rows[2];
        let expected = frame.galley_pos.y + row.pos.y;
        assert!(
            (frame.visuals[2].0 - expected).abs() < 0.5,
            "第三行的行号贴着挪过的行：{} vs {expected}",
            frame.visuals[2].0
        );
        let gap = frame.visuals[2].0 - frame.visuals[1].1;
        assert!((gap - 40.0).abs() < 0.5, "行号之间也空出了空隙：{gap}");
    }

    #[test]
    fn the_ime_caret_follows_the_shifted_rows() {
        let mut harness = Harness::new(TEXT, gaps());
        let frame = harness.frame(Vec::new());
        harness.click(line_point(&frame, 3));
        harness.ctx.memory_mut(|memory| {
            memory.request_focus(egui::Id::new("spike_editor"));
        });
        let frame = harness.frame(Vec::new());
        let ime = frame.ime.expect("编辑框聚焦时应报告输入法光标");
        let row = &frame.galley.rows[3];
        let top = frame.galley_pos.y + row.min_y();
        let bottom = frame.galley_pos.y + row.max_y();
        let caret = ime.cursor_rect.center().y;
        assert!(
            caret > top && caret < bottom,
            "输入法光标 y={caret} 应在第四行 {top}..{bottom}"
        );
    }

    #[test]
    fn find_highlight_backgrounds_move_with_their_rows() {
        // 查找高亮是 section 背景，画在行网格里：tessellate 之后找高亮色的顶点，
        // 必须落在挪过之后的那一行里。
        let ctx = egui::Context::default();
        crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let highlight = egui::Color32::from_rgb(255, 200, 0);
        let mut shifted_row_top = 0.0;
        let output = ctx.run_ui(Default::default(), |ui| {
            let mut job = egui::text::LayoutJob::default();
            for (index, line) in TEXT.split('\n').enumerate() {
                let mut format = egui::TextFormat {
                    font_id: egui::FontId::proportional(16.0),
                    line_height: Some(20.0),
                    ..Default::default()
                };
                if index == 3 {
                    format.background = highlight;
                }
                let sep = if index == 0 { "" } else { "\n" };
                job.append(&format!("{sep}{line}"), 0.0, format);
            }
            let base = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
            let shifted = Arc::new(open_gaps(&base, &gaps()));
            shifted_row_top = shifted.rows[3].pos.y;
            ui.painter()
                .galley(egui::pos2(0.0, 0.0), shifted, egui::Color32::BLACK);
        });
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let ys: Vec<f32> = primitives
            .iter()
            .filter_map(|primitive| match &primitive.primitive {
                egui::epaint::Primitive::Mesh(mesh) => Some(mesh),
                _ => None,
            })
            .flat_map(|mesh| mesh.vertices.iter())
            .filter(|vertex| vertex.color == highlight)
            .map(|vertex| vertex.pos.y)
            .collect();
        assert!(!ys.is_empty(), "画出了高亮底色");
        let top = ys.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            top >= shifted_row_top - 1.0,
            "高亮底色 {top} 应在挪过之后的第四行（{shifted_row_top} 起）"
        );
    }

    #[test]
    fn typing_reuses_the_cached_layout_and_the_shift_is_cheap() {
        // egui 按 LayoutJob 哈希缓存 galley：文本不变时拿到的是同一个 Arc，
        // 挪行只是克隆行表（每行一个 Arc<Row>），2000 行也远在 1ms 以内。
        let ctx = egui::Context::default();
        crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let text = (1..=2000)
            .map(|index| format!("第{index}行，这是一段用于测试的正文内容。"))
            .collect::<Vec<_>>()
            .join("\n");
        let gaps: Vec<Gap> = (0..200)
            .map(|index| Gap {
                line: index * 10,
                height: 20.0,
            })
            .collect();
        let mut elapsed = Vec::new();
        let mut bases: Vec<Arc<Galley>> = Vec::new();
        for _ in 0..3 {
            let _ = ctx.run_ui(Default::default(), |ui| {
                let job = egui::text::LayoutJob::simple(
                    text.clone(),
                    egui::FontId::proportional(16.0),
                    egui::Color32::BLACK,
                    500.0,
                );
                let base = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
                let started = std::time::Instant::now();
                let shifted = open_gaps(&base, &gaps);
                elapsed.push(started.elapsed());
                assert_eq!(shifted.rows.len(), base.rows.len());
                bases.push(base);
            });
        }
        assert!(Arc::ptr_eq(&bases[1], &bases[2]), "文本不变时排版命中缓存");
        let slowest = elapsed.iter().max().unwrap();
        assert!(
            slowest.as_micros() < 5_000,
            "2000 行开 200 处空隙应远低于 5ms：{slowest:?}"
        );
    }

    #[test]
    fn source_lines_map_to_their_first_wrapped_row() {
        let ctx = egui::Context::default();
        crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let text = format!("短行\n{}\n末行", "很长的一行".repeat(30));
        let _ = ctx.run_ui(Default::default(), |ui| {
            let job = egui::text::LayoutJob::simple(
                text.clone(),
                egui::FontId::proportional(16.0),
                egui::Color32::BLACK,
                200.0,
            );
            let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
            let firsts = line_first_rows(&galley);
            assert_eq!(firsts[0], 0);
            assert_eq!(firsts[1], 1);
            assert!(firsts[2] > 2, "第二行折成了好几行：{firsts:?}");
            assert_eq!(firsts.len(), 3);
            // 空隙开在折行段落之前：整段一起下移。
            let shifted = open_gaps(
                &galley,
                &[Gap {
                    line: 1,
                    height: 30.0,
                }],
            );
            for row in 1..galley.rows.len() {
                assert!((shifted.rows[row].pos.y - galley.rows[row].pos.y - 30.0).abs() < 0.5);
            }
            assert!((shifted.rows[0].pos.y - galley.rows[0].pos.y).abs() < 0.01);
        });
    }

    #[test]
    fn a_gap_after_the_last_line_only_grows_the_galley() {
        let ctx = egui::Context::default();
        crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let _ = ctx.run_ui(Default::default(), |ui| {
            let job = egui::text::LayoutJob::simple(
                TEXT.to_string(),
                egui::FontId::proportional(16.0),
                egui::Color32::BLACK,
                500.0,
            );
            let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
            let gaps = [Gap {
                line: 5,
                height: 40.0,
            }];
            let shifted = open_gaps(&galley, &gaps);
            assert_eq!(shifted.rows[4].pos, galley.rows[4].pos);
            assert!((shifted.rect.height() - galley.rect.height() - 40.0).abs() < 0.5);
            let spans = gap_spans(&shifted, &gaps);
            assert!((spans[0].0 - galley.rows[4].max_y()).abs() < 0.5);
        });
    }
}
