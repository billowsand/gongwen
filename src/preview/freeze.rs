//! 宽度还在变的帧里不重排版面。
//!
//! 自适应缩放是窗格宽度的连续函数，宽度每帧变一点，字号就每帧全新：galley 缓存
//! 全部落空，整篇文档的汉字要按新字号重新栅格化进字体图集；图集一装满，epaint
//! 会把整套字体连同缓存推倒重建、重传整张纹理（fonts.rs 的 fill_ratio > 0.8 分支）。
//! 这正是拖动分隔条、收起时间轴（面板收起带动画）、缩放窗口时那一下下的顿挫——
//! 它是尖峰，不是普遍变慢，所以把字号量化成档位只能让它变稀，消不掉。
//! 长稿更明显：预览每帧排整篇，60 页的稿子实测每帧 100 ms 以上。
//!
//! 改成：宽度还在变的这些帧，版面沿用上一次落定的缩放（字号恒定、缓存全命中、
//! 图集一动不动），视觉上的缩放交给一次层变换连续完成——变换系数是浮点，要多
//! 连续有多连续，一格都不跳。宽度一停下来就按精确缩放重排一次，文字随即恢复锐利。
//!
//! 起草页的公文预览、版本对照右栏、稿件管理的版本对照窗共用这一套。

use super::{PreviewScale, fit_scale};
use eframe::egui;

/// 一帧内宽度变化超过这么多点，就当成跳变而非拖动：拖分隔条一帧只走几个像素，
/// 而切换显示方式、窗口最大化是一步到位的，没有"连续缩放"可言。
const DRAG_STEP_MAX: f32 = 64.0;

/// 一块预览区跨帧记住的缩放状态。每块预览各持一份，互不干扰。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScaleFreeze {
    /// 版面上一次"落定"时所用的缩放倍率。拖动分隔条、缩放窗口的过程中它保持
    /// 不变，让字号恒定、排版缓存全部命中；真正的视觉缩放交给层变换。
    /// 0 表示还没排过，第一帧直接按目标倍率落定。
    pub(crate) layout_scale: f32,
    /// 上一帧量到的预览可视宽度，用来判断宽度是否还在变化。
    pub(crate) last_width: f32,
}

/// [`show_frozen`] 的结果。
pub(crate) struct Frozen<R> {
    pub(crate) inner: R,
    /// 眼睛看到的倍率（按当前宽度自适应或手动锁定的那个），不是本帧用来排版的。
    /// 手动加减档应以它为起点。
    pub(crate) target: f32,
}

/// 在滚动区里画一块预览：`add` 拿到本帧该用的 [`PreviewScale`]，原样交给
/// `official_preview`。`zoom` 为 None 时按可视宽度自适应。
///
/// 必须在滚动区的内容回调里调用：可视宽度取自 `ui.clip_rect()`。
pub(crate) fn show_frozen<R>(
    ui: &mut egui::Ui,
    freeze: &mut ScaleFreeze,
    zoom: Option<f32>,
    add: impl FnOnce(&mut egui::Ui, PreviewScale) -> R,
) -> Frozen<R> {
    let visible = ui
        .clip_rect()
        .intersect(ui.ctx().input(|input| input.content_rect()));
    let target = fit_scale(visible.width(), zoom);
    // 只有"拖动幅度"的宽度变化才值得冻结版面：分隔条一帧走几个像素，中间那些
    // 帧连起来才是一个连续动作。首帧（滚动区还没量准，可视宽度是无穷大）、切换
    // 显示方式、窗口最大化这类一步到位的跳变没有连续过程可言，直接按精确倍率
    // 重排，省得白白糊一帧。
    let step = (visible.width() - freeze.last_width).abs();
    let settled = !(0.5..=DRAG_STEP_MAX).contains(&step) || freeze.layout_scale <= 0.0;
    freeze.last_width = visible.width();
    if settled {
        freeze.layout_scale = target;
    }
    let layout_scale = freeze.layout_scale;
    let ratio = target / layout_scale;
    // 以可视区顶边中点为支点：纸张本来就横向居中于 viewport，绕这个点缩放后
    // 依旧严丝合缝地居中，顶部那一行也钉在原处不漂。
    let transform = (!settled && (ratio - 1.0).abs() > 1e-4).then(|| {
        let pivot = egui::pos2(visible.center().x, visible.top()).to_vec2();
        egui::emath::TSTransform::from_translation(pivot)
            * egui::emath::TSTransform::from_scaling(ratio)
            * egui::emath::TSTransform::from_translation(-pivot)
    });

    // 变换会把裁剪矩形一并缩放，先按逆变换预补偿，变换之后正好落回真正的
    // 可视区，内容不会被切掉或漏出。
    let clip = ui.clip_rect();
    if let Some(transform) = transform {
        ui.set_clip_rect(transform.inverse().mul_rect(clip));
    }
    // 只圈住预览自己发出的这段图形。滚动条是 ScrollArea 在这段范围之外画的，
    // 因此不会跟着一起缩放。
    let layer = ui.layer_id();
    let first = ui.painter().add(egui::Shape::Noop);

    let inner = add(
        ui,
        PreviewScale {
            zoom: Some(layout_scale),
            // 裁剪矩形被预补偿过，量出来会偏窄，这里给真实窗格宽度。
            viewport: Some(visible.width()),
        },
    );

    if let Some(transform) = transform {
        let last = ui.painter().add(egui::Shape::Noop);
        ui.ctx().graphics_mut(|graphics| {
            graphics
                .entry(layer)
                .transform_range(first, last, transform);
        });
        ui.set_clip_rect(clip);
        // 拖动一停，还需要再来一帧才能发现"宽度没变"并按精确缩放重排。
        ui.ctx().request_repaint();
    }
    Frozen { inner, target }
}
