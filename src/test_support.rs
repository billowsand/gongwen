//! 测试辅助。
//!
//! egui 0.36 起，`TexturesDelta` 析构时要求调用方已经处理（或显式 `clear`）纹理增量，
//! 否则触发 `debug_assert`。多数界面测试只关心形状与状态，不建渲染器，也不上传纹理，
//! 这里统一清掉未使用的增量；需要真实纹理的测试仍直接调用 `Context::run_ui` 并交给
//! `ui_snapshot::Canvas::absorb`。

use egui::{Context, FullOutput, RawInput, Ui};

/// 建一个允许缺字形的界面测试台。
///
/// egui_kittest 0.37 有两处默认行为变化，会让 0.36 一直通过的界面测试整片变红：
/// 一是「没有任何字体能画出某个字」直接 panic（便于发现快照里悄悄变成方框的字），
/// 二是每次 `run` 后断言每个控件都有无障碍名称。本项目测试大量直接排中文，而
/// 测试机上不一定有随包或系统中文字体（Linux CI 容器里就没有）；控件无障碍名称
/// 也从未按这个口径维护过。这里统一回到 0.36 的语义（方框照排、不做无障碍断言），
/// 免得测试结果随机器装了什么字体而变。需要更严的测试请自己用 `Harness::builder()`。
pub(crate) fn harness_ui<'a>(app: impl FnMut(&mut Ui) + 'a) -> egui_kittest::Harness<'a, ()> {
    egui_kittest::Harness::builder()
        .allow_missing_glyphs()
        .with_accessibility_check(false)
        .build_ui(app)
}

/// 带状态的界面测试台，语义同 [`harness_ui`]。
pub(crate) fn harness_ui_state<'a, State>(
    app: impl FnMut(&mut Ui, &mut State) + 'a,
    state: State,
) -> egui_kittest::Harness<'a, State> {
    egui_kittest::Harness::builder()
        .allow_missing_glyphs()
        .with_accessibility_check(false)
        .build_ui_state(app, state)
}

pub(crate) fn run_ui(
    ctx: &Context,
    raw: RawInput,
    add_contents: impl FnMut(&mut Ui),
) -> FullOutput {
    let mut output = ctx.run_ui(raw, add_contents);
    output.textures_delta.clear();
    output
}
