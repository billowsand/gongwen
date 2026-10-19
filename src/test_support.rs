//! 测试辅助。
//!
//! egui 0.36 起，`TexturesDelta` 析构时要求调用方已经处理（或显式 `clear`）纹理增量，
//! 否则触发 `debug_assert`。多数界面测试只关心形状与状态，不建渲染器，也不上传纹理，
//! 这里统一清掉未使用的增量；需要真实纹理的测试仍直接调用 `Context::run_ui` 并交给
//! `ui_snapshot::Canvas::absorb`。

use egui::{Context, FullOutput, RawInput, Ui};

pub(crate) fn run_ui(
    ctx: &Context,
    raw: RawInput,
    add_contents: impl FnMut(&mut Ui),
) -> FullOutput {
    let mut output = ctx.run_ui(raw, add_contents);
    output.textures_delta.clear();
    output
}
