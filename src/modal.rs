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
}
