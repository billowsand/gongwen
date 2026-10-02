//! 不走应用内输入法的文本框：密码、接口地址、API Key、模型名、拼音标注这类只收 ASCII 的字段。
//!
//! egui 报给后端的 `IMEOutput` 里只有光标矩形，分不出焦点落在密码框还是正文里。
//! 不区分的后果很具体：密码框里敲的字母会**明文**出现在候选窗的编码串里，组码中的
//! 数字键会把汉字选进密码，没在组码时的 `,` `.` 被转成全角——使用者以为设的是
//! 半角密码，存下的却是全角的。接口地址里的 `.` 还可能被当成翻页键。
//!
//! 所以由控件自己声明：画完文本框后把它的 `Response` 过一遍 [`exempt`]，每帧都要过。
//! 声明攒在 egui 的临时存储里，帧尾由输入法取走（见 `session::Ime::end_frame`），
//! 下一帧帧首焦点落在这些控件上就不接管键盘，按键原样交给文本框。
//!
//! 系统输入法照样关着：这类字段本来就不该出中文。

use std::collections::HashSet;

use eframe::egui;

/// 本帧声明过的豁免控件。
#[derive(Debug, Clone, Default)]
struct Declared(HashSet<egui::Id>);

/// 临时存储里的键。
fn key() -> egui::Id {
    egui::Id::new("gw-ime-exempt")
}

/// 声明这个文本框不走应用内输入法，原样返回 `Response` 以便接着链式调用。
pub(crate) fn exempt(response: egui::Response) -> egui::Response {
    declare(&response.ctx, response.id);
    response
}

/// [`exempt`] 的底层形式：按控件 Id 声明。
pub(crate) fn declare(ctx: &egui::Context, id: egui::Id) {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Declared>(key()).0.insert(id);
    });
}

/// 取走本帧的声明（帧尾调一次）。取走即清空：下一帧没再声明的控件就不算了。
pub(super) fn take(ctx: &egui::Context) -> HashSet<egui::Id> {
    ctx.data_mut(|data| {
        data.remove_temp::<Declared>(key())
            .map(|declared| declared.0)
            .unwrap_or_default()
    })
}
