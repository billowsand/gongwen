//! 让输入法候选窗跟着光标走。
//!
//! egui 每帧把编辑框的整体矩形和光标矩形一起交给后端（`IMEOutput::rect` 与
//! `IMEOutput::cursor_rect`），但 egui-winit 只取前者，把**整个编辑框**当成
//! "IME 光标区域"发给 `Window::set_ime_cursor_area`（lib.rs 里
//! `let ime_rect_px = pixels_per_point * ime.rect;`，`cursor_rect` 全文没有引用；
//! eframe 的 web 后端用的才是 `cursor_rect`）。Markdown 编辑框铺满整个中央区，
//! 于是这块"光标区域"有大半个窗口那么大。
//!
//! 配上 Hyprland 摆放候选窗的算法（`InputMethodPopup.cpp::updateBox()`：默认放在
//! 光标矩形**下方**，若 `parentBox.y + cursorBox.y + cursorBox.height + popupSize.y`
//! 越过显示器下沿就翻到光标**上方**），两个症状就都对上了：
//!
//! * 窗口模式：矩形高度约等于编辑框高度，候选窗被放到编辑框底下——看上去像焊死
//!   在界面下方，打字时纹丝不动；
//! * 全屏：矩形高度约等于屏幕高度，下沿判定必然越界，于是翻到"光标上方"，即
//!   `cursorBox.y - popupSize.y`，是个负数。Hyprland 只钳制右边沿，不钳上边沿，
//!   候选窗就被推出屏幕顶部——看不见候选框，但输入链路完好，照样能打字选词。
//!
//! 修法是把 `rect` 就地改写成 `cursor_rect`，让 egui-winit 自己那一次
//! `set_ime_cursor_area` 直接发对。
//!
//! 之前试过另一条路——保留 egui-winit 的错矩形，帧末再用
//! `ViewportCommand::IMERect` 补发一次正确的。它能修好窗口模式，却修不好全屏：
//! 那样一帧里会发生两次 `zwp_text_input_v3::commit`，先是错矩形、后是对矩形，
//! 而 compositor 每次 commit 都要跑一遍 `updateAllPopups()`。中间那一帧把弹窗摆到
//! 屏幕外之后，全屏下它就再没回来。少发一次、一次发对，才是稳的。
//!
//! 这层补丁对全应用的 TextEdit 生效，不只是 Markdown 编辑框。

use eframe::egui;

/// 在 `App::ui` 结尾调用：把本帧要报给输入法的"光标区域"收窄成真正的光标。
pub(crate) fn follow_cursor(ctx: &egui::Context) {
    let reported = ctx.output_mut(|output| {
        let ime = output.ime.as_mut()?;
        ime.rect = ime.cursor_rect;
        Some(ime.rect)
    });
    if std::env::var_os("GONGWEN_IME_DEBUG").is_some() {
        log_rect(ctx, reported);
    }
}

/// `GONGWEN_IME_DEBUG=1` 时把每次变化的光标矩形打到 stderr。
///
/// 候选窗摆错位置时，光靠肉眼分不清是我们报错了坐标，还是 compositor 摆错了；
/// 这里打出的是送进 `set_ime_cursor_area` 之前的那组数（egui 点，原点在窗口
/// 左上角），和窗口尺寸一起看就能判断：数落在窗口内 = 我们这边没问题。
fn log_rect(ctx: &egui::Context, rect: Option<egui::Rect>) {
    use std::sync::Mutex;
    static LAST: Mutex<Option<egui::Rect>> = Mutex::new(None);

    let Ok(mut last) = LAST.lock() else {
        return;
    };
    if *last == rect {
        return;
    }
    *last = rect;
    let screen = ctx.viewport_rect();
    let fullscreen = ctx.input(|input| input.viewport().fullscreen);
    match rect {
        Some(rect) => eprintln!(
            "[ime] cursor=({:.1},{:.1} {:.1}x{:.1}) window={:.1}x{:.1} ppp={:.2} fullscreen={fullscreen:?}",
            rect.min.x,
            rect.min.y,
            rect.width(),
            rect.height(),
            screen.width(),
            screen.height(),
            ctx.pixels_per_point(),
        ),
        None => eprintln!("[ime] 无编辑焦点"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ime_output(ctx: &egui::Context) -> Option<egui::output::IMEOutput> {
        ctx.output(|output| output.ime)
    }

    /// 用一个离屏 Context 跑一帧，帧内塞进 egui 那样的 IMEOutput，
    /// 再验证 `follow_cursor` 把上报矩形收窄到了光标上。
    fn run_frame(ime: Option<egui::output::IMEOutput>) -> Option<egui::output::IMEOutput> {
        let ctx = egui::Context::default();
        let mut seen = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let ctx = ui.ctx();
            ctx.output_mut(|output| output.ime = ime);
            follow_cursor(ctx);
            seen = ime_output(ctx);
        });
        seen
    }

    fn tall_editor_with_caret_at(caret_y: f32) -> egui::output::IMEOutput {
        egui::output::IMEOutput {
            // 铺满中央区的 Markdown 编辑框。
            rect: egui::Rect::from_min_size(egui::pos2(40.0, 80.0), egui::vec2(900.0, 940.0)),
            cursor_rect: egui::Rect::from_min_size(
                egui::pos2(120.0, caret_y),
                egui::vec2(1.0, 24.0),
            ),
            should_interrupt_composition: false,
        }
    }

    #[test]
    fn narrows_the_reported_area_from_the_whole_editor_to_the_caret() {
        let caret = tall_editor_with_caret_at(300.0);
        let reported = run_frame(Some(caret)).expect("IME 输出应当保留");
        // egui-winit 只看 `rect`，所以必须是它被改写，而不是另发一份。
        assert_eq!(reported.rect, caret.cursor_rect);
        assert_eq!(reported.cursor_rect, caret.cursor_rect);
    }

    #[test]
    fn reported_area_stays_as_short_as_one_text_row() {
        // 高度是全屏下候选窗被翻到屏幕外的直接原因：Hyprland 拿
        // `cursorBox.y + cursorBox.height + popupSize.y` 判下沿越界。
        let reported = run_frame(Some(tall_editor_with_caret_at(300.0))).expect("IME 输出应当保留");
        assert!(
            reported.rect.height() < 40.0,
            "上报高度应当只有一行文字，实际 {}",
            reported.rect.height()
        );
    }

    #[test]
    fn leaves_an_unfocused_frame_alone() {
        assert!(run_frame(None).is_none());
    }
}
