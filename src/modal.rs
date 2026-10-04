//! 模态对话框的统一守卫。
//!
//! `egui::Modal` 只拦得住它下面各层的指针和**新的**焦点：弹出之前已经拿着焦点的
//! 文本框（典型是正文编辑框）照样收键盘，应用内输入法也按焦点上屏；不看焦点的
//! 按键处理（查找条的 Esc、PDF 翻页、F7 跳差异）更是照吃不误。这里补上这两块，
//! 任何地方开的 `egui::Modal` 都自动生效，不必逐个登记。
//!
//! 判断依据是 egui 记下的「上一帧最顶层的模态层」：模态在哪一帧画出，下一帧起
//! 才算数。帧首调用时这正好是本帧该遵守的结论。

use eframe::egui;

/// 帧首守卫：有模态时把焦点从模态下面的控件收走，模态关掉后还回去。
#[derive(Default)]
pub(crate) struct ModalGuard {
    /// 模态弹出时被收走焦点的控件，关掉后还给它。
    saved_focus: Option<egui::Id>,
    /// 本帧有没有模态盖着。
    active: bool,
}

impl ModalGuard {
    /// 每帧最先调用，要早于输入法接管键盘：输入法按焦点决定往哪儿上屏，
    /// 焦点在这里先收走，它这一帧就会丢掉组字、不再往背后的编辑框里送字。
    pub(crate) fn begin_frame(&mut self, ctx: &egui::Context) {
        let modal_open = ctx.memory(|memory| memory.top_modal_layer().is_some());
        if modal_open {
            if let Some(id) = ctx.memory(|memory| memory.focused())
                && focus_is_below_modal(ctx, id)
            {
                if self.saved_focus.is_none() {
                    self.saved_focus = Some(id);
                }
                ctx.memory_mut(|memory| memory.surrender_focus(id));
            }
        } else if self.active {
            // 模态刚关：焦点没被别处接走、原控件还在，就还回去，接着打字不用再点一下。
            if let Some(id) = self.saved_focus.take()
                && ctx.memory(|memory| memory.focused().is_none())
                && ctx.read_response(id).is_some()
            {
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
        }
        self.active = modal_open;
    }

    /// 本帧有没有模态盖着。应用级快捷键据此整体停用。
    pub(crate) fn active(&self) -> bool {
        self.active
    }
}

/// 焦点控件是不是在最顶层模态的下面。上一帧没画出来的控件判断不了，按不在下面处理，
/// 免得模态自己首帧请求的焦点被误收。
fn focus_is_below_modal(ctx: &egui::Context, id: egui::Id) -> bool {
    ctx.read_response(id).is_some_and(|response| {
        !ctx.memory(|memory| memory.is_above_modal_layer(response.layer_id))
    })
}

/// 模态确认框怎样算「被关掉」。被关掉一律按取消处理，绝不触发确认动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dismiss {
    /// Esc、点遮罩都算取消。纯确认框用。
    EscOrBackdrop,
    /// 只认 Esc。带输入的表单用：点歪了落在遮罩上不该丢掉已填的内容。
    EscOnly,
}

/// [`dialog`] 的结果。
pub(crate) struct DialogResponse<R> {
    pub(crate) inner: R,
    /// 按 [`Dismiss`] 的规则被关掉了（或内容里调了 `ui.close()`），调用方按取消处理。
    pub(crate) dismissed: bool,
}

/// 统一样式的模态确认框：遮罩挡住背后的点击，焦点与快捷键由 [`ModalGuard`] 收走。
/// 标题、宽度由这里定；按钮与正文由调用方画，`inner` 原样带回。
pub(crate) fn dialog<R>(
    ctx: &egui::Context,
    id: egui::Id,
    title: &str,
    width: f32,
    dismiss: Dismiss,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> DialogResponse<R> {
    let response = egui::Modal::new(id)
        .frame(
            crate::theme::card()
                .inner_margin(egui::Margin::same(16))
                .corner_radius(12)
                .shadow(crate::theme::float_shadow(
                    if crate::theme::current().dark { 75 } else { 32 },
                )),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.heading(title);
            ui.add_space(6.0);
            add_contents(ui)
        });
    let dismissed = match dismiss {
        Dismiss::EscOrBackdrop => response.should_close(),
        // 照 `ModalResponse::should_close` 的规则，只是不认遮罩：只有最顶层、没开下拉
        // 时才吃 Esc，免得 Esc 本该先关掉的下拉被连带关掉整个框。
        Dismiss::EscOnly => {
            response.response.should_close()
                || (response.is_top_modal
                    && !response.any_popup_open
                    && ctx.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                    }))
        }
    };
    DialogResponse {
        inner: response.inner,
        dismissed,
    }
}

/// 这个 `Ui` 所在的层此刻能不能接键盘。不看焦点的按键处理要先问一句：
/// 有模态盖在上面时按键归模态，不能被背后的页面先吃掉。
pub(crate) fn takes_keys(ui: &egui::Ui) -> bool {
    ui.ctx()
        .memory(|memory| memory.is_above_modal_layer(ui.layer_id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_id() -> egui::Id {
        egui::Id::new("modal_guard_test_editor")
    }

    /// 画一帧：背后一个文本框，`modal` 为真时再盖一个模态。守卫在帧首跑，与应用里一致。
    fn frame(ctx: &egui::Context, guard: &mut ModalGuard, text: &mut String, modal: bool) {
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            guard.begin_frame(ui.ctx());
            ui.add(egui::TextEdit::singleline(text).id(editor_id()));
            if modal {
                egui::Modal::new(egui::Id::new("modal_guard_test_modal")).show(ui.ctx(), |ui| {
                    ui.label("确认？");
                });
            }
        });
    }

    #[test]
    fn modal_takes_focus_from_background_and_gives_it_back() {
        let ctx = egui::Context::default();
        let mut guard = ModalGuard::default();
        let mut text = String::new();
        frame(&ctx, &mut guard, &mut text, false);
        ctx.memory_mut(|memory| memory.request_focus(editor_id()));
        frame(&ctx, &mut guard, &mut text, false);
        assert!(ctx.memory(|memory| memory.has_focus(editor_id())));

        // 模态画出的那一帧还没登记，下一帧起守卫才收走焦点。
        frame(&ctx, &mut guard, &mut text, true);
        frame(&ctx, &mut guard, &mut text, true);
        assert!(guard.active());
        assert!(!ctx.memory(|memory| memory.has_focus(editor_id())));

        // 模态关掉：焦点还给原来的编辑框。
        frame(&ctx, &mut guard, &mut text, false);
        frame(&ctx, &mut guard, &mut text, false);
        assert!(!guard.active());
        assert!(ctx.memory(|memory| memory.has_focus(editor_id())));
    }

    #[test]
    fn layers_under_a_modal_do_not_take_keys() {
        let ctx = egui::Context::default();
        let mut taken = Vec::new();
        for modal in [false, true, true] {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                taken.push(takes_keys(ui));
                if modal {
                    egui::Modal::new(egui::Id::new("modal_guard_test_modal"))
                        .show(ui.ctx(), |ui| ui.label("确认？"));
                }
            });
        }
        assert_eq!(taken, [true, true, false]);
    }

    /// 跑一帧只有一个确认框的画面，返回它是否被关掉。
    fn dialog_frame(ctx: &egui::Context, dismiss: Dismiss, events: Vec<egui::Event>) -> bool {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut dismissed = false;
        let _ = ctx.run_ui(input, |ui| {
            dismissed = dialog(
                ui.ctx(),
                egui::Id::new("dialog_test"),
                "确认",
                200.0,
                dismiss,
                |ui| ui.label("内容"),
            )
            .dismissed;
        });
        dismissed
    }

    /// 在左上角遮罩上点一下（按下、松开分两帧）。
    fn click_backdrop(ctx: &egui::Context, dismiss: Dismiss) -> bool {
        let pos = egui::pos2(5.0, 5.0);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        dialog_frame(
            ctx,
            dismiss,
            vec![egui::Event::PointerMoved(pos), button(true)],
        );
        dialog_frame(ctx, dismiss, vec![button(false)])
    }

    fn press_escape(ctx: &egui::Context, dismiss: Dismiss) -> bool {
        dialog_frame(
            ctx,
            dismiss,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
    }

    #[test]
    fn backdrop_click_only_dismisses_plain_confirmations() {
        for (dismiss, expected) in [(Dismiss::EscOrBackdrop, true), (Dismiss::EscOnly, false)] {
            let ctx = egui::Context::default();
            // 先画两帧，让模态层登记、遮罩有了上一帧的位置。
            dialog_frame(&ctx, dismiss, Vec::new());
            dialog_frame(&ctx, dismiss, Vec::new());
            assert_eq!(click_backdrop(&ctx, dismiss), expected, "{dismiss:?}");
        }
    }

    #[test]
    fn escape_dismisses_every_dialog() {
        for dismiss in [Dismiss::EscOrBackdrop, Dismiss::EscOnly] {
            let ctx = egui::Context::default();
            dialog_frame(&ctx, dismiss, Vec::new());
            dialog_frame(&ctx, dismiss, Vec::new());
            assert!(press_escape(&ctx, dismiss), "{dismiss:?}");
        }
    }
}
