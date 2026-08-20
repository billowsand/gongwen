//! 让输入法候选窗跟着光标走。
//!
//! egui 每帧会把编辑框的整体矩形和光标矩形一起交给后端（`IMEOutput::rect` 与
//! `IMEOutput::cursor_rect`），但 egui-winit 只取前者，把**整个编辑框**当成
//! "IME 光标区域"发给 `Window::set_ime_cursor_area`。Markdown 编辑框铺满整个中
//! 央区，于是：
//!
//! * Wayland（Hyprland/Omarchy + fcitx5）：text-input-v3 的 `set_cursor_rectangle`
//!   收到一块占满窗口的矩形，fcitx5 为了不遮住"光标"，只能把候选窗顶到这块矩形
//!   之外——看上去就是候选窗焊死在界面下方，打字时纹丝不动；
//! * 窗口全屏后这块矩形等于整个屏幕，候选窗被挤出屏幕，于是"看不见候选框但照样
//!   能打字、能选词"；
//! * Windows 的 `CFS_EXCLUDE` 同理，候选窗会跑到编辑框左上角而不是光标处。
//!
//! 修不了上游就在应用层盖一层：帧末读回本帧的 `IMEOutput`，用
//! `ViewportCommand::IMERect` 把真正的光标矩形再发一次。eframe 处理视口命令排在
//! `handle_platform_output` 之后（glow_integration.rs 里先 `handle_platform_output`
//! 再 `handle_viewport_output`），后发的这条覆盖前面那条，候选窗就贴着光标了。
//!
//! 这层补丁对所有 TextEdit 生效，不只是 Markdown 编辑框。

use eframe::egui;

/// 上一次发出去的 IME 状态，用来决定这帧要不要重发。
pub(crate) type ImeState = Option<egui::output::IMEOutput>;

/// 判断本帧是否需要重发光标矩形。
///
/// 条件与 egui-winit 自己的节流条件保持一致（矩形变了，或者这一帧有输入事件），
/// 确保它每次发出错误矩形时我们都能跟着纠正一次：预编辑串变化时编辑框整体矩形
/// 往往没动，只有"有事件"这一条能兜住。
fn should_send(last: ImeState, current: egui::output::IMEOutput, has_events: bool) -> bool {
    last != Some(current) || has_events
}

/// 在 `App::ui` 结尾调用：把本帧的光标矩形补发给窗口后端。
pub(crate) fn follow_cursor(ctx: &egui::Context, last: &mut ImeState) {
    let Some(current) = ctx.output(|output| output.ime) else {
        // 编辑框失焦：egui-winit 会 `set_ime_allowed(false)`，这里只需丢掉旧状态，
        // 下次聚焦时第一帧必定重发。
        *last = None;
        return;
    };
    if !should_send(*last, current, ctx.input(|input| !input.events.is_empty())) {
        return;
    }
    *last = Some(current);
    ctx.send_viewport_cmd(egui::ViewportCommand::IMERect(current.cursor_rect));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ime(cursor_x: f32) -> egui::output::IMEOutput {
        egui::output::IMEOutput {
            rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0)),
            cursor_rect: egui::Rect::from_min_size(
                egui::pos2(cursor_x, 120.0),
                egui::vec2(1.0, 20.0),
            ),
            should_interrupt_composition: false,
        }
    }

    #[test]
    fn sends_on_the_first_frame_and_whenever_the_caret_moves() {
        assert!(should_send(None, ime(10.0), false));
        assert!(should_send(Some(ime(10.0)), ime(30.0), false));
    }

    #[test]
    fn resends_while_there_are_input_events_even_if_the_caret_stays_put() {
        // 预编辑串在原地增删时光标矩形可能一模一样，但 egui-winit 会借着"有事件"
        // 重发整块编辑框矩形，这里必须跟着覆盖回去。
        assert!(should_send(Some(ime(10.0)), ime(10.0), true));
    }

    #[test]
    fn stays_quiet_on_idle_frames() {
        assert!(!should_send(Some(ime(10.0)), ime(10.0), false));
    }
}
