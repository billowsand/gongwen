//! 锤子稿纸：用本地绘图表现木纹、纸张与压边，不依赖外部纹理资源。

use super::{Color32, PaperFamily, Theme, current};
use eframe::egui::{self, Rect, Stroke};

pub(super) const LABEL: &str = "锤子稿纸";

pub(super) const fn palette() -> Theme {
    let mut theme = Theme::claude();
    theme.label = LABEL;
    theme.paper_family = PaperFamily::Parchment;
    theme.canvas = Color32::from_rgb(213, 204, 189);
    theme.surface = Color32::from_rgb(248, 246, 240);
    theme.surface_sunk = Color32::from_rgb(237, 233, 225);
    theme.surface_hover = Color32::from_rgb(235, 231, 222);
    theme.surface_active = Color32::from_rgb(215, 205, 190);
    theme.border = Color32::from_rgb(216, 207, 193);
    theme.border_strong = Color32::from_rgb(172, 157, 139);
    theme.text = Color32::from_rgb(73, 64, 57);
    theme.text_soft = Color32::from_rgb(99, 87, 83);
    theme.text_muted = Color32::from_rgb(145, 129, 110);
    theme.accent = Color32::from_rgb(112, 96, 81);
    theme.accent_hover = Color32::from_rgb(124, 108, 92);
    theme.accent_active = Color32::from_rgb(92, 78, 65);
    theme.accent_soft = Color32::from_rgb(231, 225, 214);
    theme.md.body = theme.text;
    theme.md.marker = Color32::from_rgb(159, 140, 117);
    theme.md.title = theme.text;
    theme.md.heading = theme.text;
    theme.md.strong = theme.accent_active;
    theme.md.strong_bg = theme.accent_soft;
    theme.md.bullet = theme.accent;
    theme.md.table_pipe = theme.text_muted;
    theme.md.table_rule = theme.border_strong;
    theme.md.table_cell = theme.text_soft;
    theme.md.code = theme.accent;
    theme.md.quoted = theme.text_soft;
    theme.md.anchor_bg = theme.accent_soft;
    theme
}

pub fn active() -> bool {
    current().label == LABEL
}

pub fn manuscript_active() -> bool {
    active() && !super::paper::is_dark()
}

/// 顶栏专用的浅字，不把全应用的正文色改成白色。
pub fn chrome_ink() -> Color32 {
    Color32::from_rgb(250, 246, 237)
}

/// 四顶点渐变保留轻微体积感，不使用逐像素绘图。
pub fn gradient(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    painter.add(gradient_shape(rect, top, bottom));
}

fn gradient_shape(rect: Rect, top: Color32, bottom: Color32) -> egui::Shape {
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [
        (rect.left_top(), top),
        (rect.right_top(), top),
        (rect.right_bottom(), bottom),
        (rect.left_bottom(), bottom),
    ] {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv: egui::epaint::WHITE_UV,
            color,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    egui::Shape::mesh(mesh)
}

/// 标签条先占背景槽，排完内容再确定高度，避免覆盖标签或误铺到正文区。
pub fn tab_background(painter: &egui::Painter, slot: egui::layers::ShapeIdx, rect: Rect) {
    if active() {
        // 以面板裁剪边界统一纸面起点，避免控件高度和边距产生一像素断层。
        let base = painter.clip_rect().bottom() - 2.0;
        painter.set(
            slot,
            egui::Shape::Vec(vec![
                gradient_shape(
                    Rect::from_min_max(rect.min, egui::pos2(rect.right(), base)),
                    Color32::from_rgb(163, 153, 139),
                    Color32::from_rgb(145, 133, 117),
                ),
                egui::Shape::rect_filled(
                    Rect::from_min_max(egui::pos2(rect.left(), base), rect.max),
                    0,
                    crate::theme::surface(),
                ),
            ]),
        );
    }
}

mod tabs;
#[cfg(test)]
pub use tabs::document_tab;
pub use tabs::{document_tab_to, ribbon_shape, rounded_gradient};

/// 分组只增加一行小标题，不影响各命令原有的启用、提示和点击动作。
pub fn command_group(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        let row = ui.horizontal(add);
        ui.add_sized(
            egui::vec2(row.response.rect.width(), 14.0),
            egui::Label::new(
                egui::RichText::new(label)
                    .size(11.0)
                    .color(super::text_muted()),
            )
            .halign(egui::Align::Center),
        );
    });
}

pub fn chrome(painter: &egui::Painter, rect: Rect) {
    if !active() {
        return;
    }
    gradient(
        painter,
        rect,
        Color32::from_rgb(139, 128, 114),
        Color32::from_rgb(120, 108, 95),
    );
    painter.hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(1.0, Color32::from_white_alpha(28)),
    );
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, Color32::from_black_alpha(28)),
    );
}

/// 木纹细线的列距。
const WOOD_PITCH: f32 = 3.0;
/// 每条纹线从顶到底的分段数。
const WOOD_SEGMENTS: usize = 12;
/// 纹线左右漂移的幅度（1.1）加半个线宽，判断是否露出时留的余量。
const WOOD_REACH: f32 = 2.0;

/// 淡桦木底板。固定坐标的细纹只画可见区域，滚动和鼠标移动不会让纹理闪烁。
pub fn wood(painter: &egui::Painter, rect: Rect) {
    if !active() || !rect.is_finite() {
        return;
    }
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    painter.extend(wood_shapes(rect, painter.clip_rect(), None));
}

/// 把木纹填进先占好的槽位；`cover` 是之后画上去的不透明卡片，被它整片盖住的
/// 纹线段不再生成。木纹每帧都要重新三角化，一层就有上万个顶点，不能白画。
pub fn wood_around(
    painter: &egui::Painter,
    slot: egui::layers::ShapeIdx,
    rect: Rect,
    cover: Option<Rect>,
) {
    if !active() || !rect.is_finite() {
        return;
    }
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    painter.set(
        slot,
        egui::Shape::Vec(wood_shapes(rect, painter.clip_rect(), cover)),
    );
}

/// 采样点只由 `rect` 决定，裁掉的只是看不见的列与段，露出部分与整幅绘制逐点一致。
fn wood_shapes(rect: Rect, visible: Rect, cover: Option<Rect>) -> Vec<egui::Shape> {
    let mut shapes = vec![gradient_shape(
        rect,
        Color32::from_rgb(241, 225, 198),
        Color32::from_rgb(238, 221, 188),
    )];
    let visible = visible.intersect(rect);
    if visible.width() <= 0.0 || visible.height() <= 0.0 || rect.height() <= 0.0 {
        return shapes;
    }
    let segments = WOOD_SEGMENTS as f32;
    let index_at = |y: f32| ((y - rect.top()) / rect.height() * segments).clamp(0.0, segments);
    let first = index_at(visible.top() - WOOD_REACH).floor() as usize;
    let last = index_at(visible.bottom() + WOOD_REACH).ceil() as usize;
    // 整列都落在卡片里时，只留卡片上下两头露出的段；两段相接就合成一条。
    let hidden = cover.map(|cover| {
        let cover = cover.shrink(WOOD_REACH);
        let upper = (index_at(cover.top()).ceil() as usize).min(last);
        let lower = (index_at(cover.bottom()).floor() as usize).max(first);
        (cover.x_range(), upper, lower)
    });
    let mut x = (rect.left() / WOOD_PITCH).floor() * WOOD_PITCH;
    x = x.max(((visible.left() - WOOD_REACH) / WOOD_PITCH).floor() * WOOD_PITCH);
    while x < rect.right() && x <= visible.right() + WOOD_REACH {
        let seed = x * 0.73;
        let alpha = 6 + ((seed.sin().abs() * 12.0) as u8);
        let stroke = Stroke::new(0.7, Color32::from_rgba_unmultiplied(135, 103, 61, alpha));
        let point = |i: usize| {
            let y = rect.top() + rect.height() * i as f32 / segments;
            let drift = (y * 0.008 + seed).sin() * 1.1;
            egui::pos2(x + drift, y)
        };
        let runs = match hidden {
            Some((columns, upper, lower)) if columns.contains(x) && upper < lower => {
                [(first, upper), (lower, last)]
            }
            _ => [(first, last), (0, 0)],
        };
        for (start, end) in runs {
            if start < end {
                shapes.push(egui::Shape::line(
                    (start..=end).map(&point).collect(),
                    stroke,
                ));
            }
        }
        x += WOOD_PITCH;
    }
    shapes
}

/// 控件上下压边，保持原控件的点击、禁用、焦点和键盘语义。
pub fn bevel(painter: &egui::Painter, rect: Rect, pressed: bool, dark: bool) {
    let (light, shade) = if pressed { (12, 65) } else { (85, 28) };
    let rect = rect.shrink(1.5);
    painter.hline(
        rect.x_range(),
        rect.top(),
        Stroke::new(1.0, Color32::from_white_alpha(light)),
    );
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, Color32::from_black_alpha(shade)),
    );
    if dark {
        painter.rect_stroke(
            rect.expand(1.0),
            3,
            Stroke::new(1.0, Color32::from_black_alpha(65)),
            egui::StrokeKind::Inside,
        );
    }
}

/// 先预留背景形状，等 TextEdit 排版完再填入，稿纸始终位于文字和选区下方。
/// 横线按实际视觉行（包括软换行）对齐，末尾空白延续最后一行的行高。
pub fn manuscript(
    painter: &egui::Painter,
    slot: egui::layers::ShapeIdx,
    mut rect: Rect,
    output: &egui::text_edit::TextEditOutput,
    numbered: bool,
) {
    if !manuscript_active() {
        return;
    }
    rect.max.y = rect.max.y.max(output.response.rect.bottom());
    let mut shapes = Vec::new();
    let clip = painter.clip_rect();
    let margin_x = output.galley_pos.x - if numbered { 8.0 } else { 12.0 };
    let line = Stroke::new(0.7, Color32::from_rgb(228, 223, 212));
    shapes.push(egui::Shape::rect_filled(
        Rect::from_min_max(rect.min, egui::pos2(margin_x, rect.bottom())),
        0,
        Color32::from_rgba_unmultiplied(209, 195, 171, 15),
    ));
    shapes.push(egui::Shape::line_segment(
        [
            egui::pos2(margin_x, rect.top()),
            egui::pos2(margin_x, rect.bottom()),
        ],
        Stroke::new(0.8, Color32::from_rgb(218, 169, 158)),
    ));
    let mut last_y = output.galley_pos.y;
    let mut step = 24.0;
    for row in &output.galley.rows {
        let y = output.galley_pos.y + row.pos.y + row.size.y;
        step = row.size.y.max(1.0);
        if y >= clip.top() && y <= clip.bottom() {
            shapes.push(egui::Shape::line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                line,
            ));
        }
        last_y = y;
    }
    let start = last_y + (((clip.top() - last_y) / step).floor().max(0.0) + 1.0) * step;
    let mut y = start;
    while y <= rect.bottom().min(clip.bottom()) {
        shapes.push(egui::Shape::line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            line,
        ));
        y += step;
    }
    // 少量细纤维只在可见范围绘制，透明度低于横线。
    let mut y = (clip.top().max(rect.top()) / 5.0).floor() * 5.0;
    while y < rect.bottom().min(clip.bottom()) {
        let x = rect.left() + ((y * 1.37).sin().abs() * rect.width());
        shapes.push(egui::Shape::line_segment(
            [
                egui::pos2(x, y),
                egui::pos2((x + 14.0).min(rect.right()), y),
            ],
            Stroke::new(0.5, Color32::from_black_alpha(5)),
        ));
        y += 5.0;
    }
    painter.set(slot, egui::Shape::Vec(shapes));
}

pub fn source_frame() -> egui::Frame {
    super::card()
        .fill(if manuscript_active() {
            Color32::from_rgb(251, 247, 237)
        } else {
            super::surface()
        })
        .inner_margin(egui::Margin {
            left: 24,
            right: 18,
            top: 20,
            bottom: 18,
        })
        .shadow(super::paper_shadow(38))
}

/// 管理页承托在一整张档案纸上，留出底板与纸边，长列表仍由原来的滚动区虚拟化。
pub fn management_page(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    if !active() {
        add(ui);
        return;
    }
    let rect = ui.available_rect_before_wrap();
    gradient(
        ui.painter(),
        rect,
        Color32::from_rgb(222, 214, 201),
        super::canvas(),
    );
    egui::Frame::new()
        .fill(super::surface())
        .stroke(Stroke::new(1.0, Color32::from_rgb(190, 179, 163)))
        .corner_radius(3)
        .shadow(super::paper_shadow(28))
        .outer_margin(egui::Margin::same(12))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            let paper = ui.available_rect_before_wrap();
            ui.set_min_size(paper.size());
            // 极淡的短纤维只画可见纸面，既不随行数增长，也不干扰表格与文字。
            let mut y = paper.top() + 7.0;
            while y < paper.bottom() {
                let x = paper.left() + (y * 0.83).sin().abs() * paper.width();
                ui.painter().line_segment(
                    [
                        egui::pos2(x, y),
                        egui::pos2((x + 11.0).min(paper.right()), y),
                    ],
                    Stroke::new(0.5, Color32::from_black_alpha(4)),
                );
                y += 19.0;
            }
            add(ui);
        });
}

/// 台账表头浅凹底：同一纸面上用低反差渐变和压线提示列标题。
pub fn ledger_header(ui: &egui::Ui) {
    if !active() {
        return;
    }
    let rect = ui.max_rect();
    gradient(
        ui.painter(),
        rect,
        Color32::from_rgb(235, 230, 221),
        Color32::from_rgb(241, 238, 231),
    );
    ui.painter().hline(
        rect.x_range(),
        rect.top(),
        Stroke::new(0.7, super::border()),
    );
}

/// 列表共用台账的细分隔线和左侧选中标记，不给每一行加独立卡片外框。
pub fn index_row(painter: &egui::Painter, rect: Rect, selected: bool) {
    if !active() {
        return;
    }
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(0.6, super::border()),
    );
    if selected {
        painter.vline(
            rect.left(),
            rect.y_range(),
            Stroke::new(2.0, super::accent()),
        );
    }
}

/// 顶部小按钮只用浅压边，避免浓黑外圈与明亮高光形成生硬的金属块。
pub fn chrome_edge(painter: &egui::Painter, rect: Rect, pressed: bool) {
    let rect = rect.shrink(1.0);
    painter.rect_stroke(
        rect,
        4,
        Stroke::new(
            0.7,
            Color32::from_black_alpha(if pressed { 45 } else { 30 }),
        ),
        egui::StrokeKind::Inside,
    );
    painter.hline(
        (rect.left() + 4.0)..=(rect.right() - 4.0),
        rect.top() + 0.8,
        Stroke::new(
            0.6,
            Color32::from_white_alpha(if pressed { 12 } else { 35 }),
        ),
    );
    painter.hline(
        (rect.left() + 4.0)..=(rect.right() - 4.0),
        rect.bottom() - 0.6,
        Stroke::new(0.6, Color32::from_black_alpha(20)),
    );
}

#[cfg(test)]
mod wood_tests {
    use super::*;

    fn lines(shapes: &[egui::Shape]) -> Vec<&[egui::Pos2]> {
        shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Path(path) => Some(path.points.as_slice()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn culled_wood_is_a_subset_of_the_full_sheet_and_skips_the_cover() {
        let rect = Rect::from_min_max(egui::pos2(10.0, 40.0), egui::pos2(1290.0, 860.0));
        let full = wood_shapes(rect, rect, None);
        let full_lines = lines(&full);
        assert_eq!(full_lines.len(), 427);
        assert!(
            full_lines
                .iter()
                .all(|line| line.len() == WOOD_SEGMENTS + 1)
        );
        let full_points: Vec<_> = full_lines.iter().flat_map(|line| line.iter()).collect();

        let cover = rect.shrink(10.0);
        let culled = wood_shapes(rect, rect, Some(cover));
        let culled_lines = lines(&culled);
        let culled_points: usize = culled_lines.iter().map(|line| line.len()).sum();
        assert!(culled_points * 3 < full_points.len());
        for line in &culled_lines {
            // 露出的纹线与整幅逐点重合，不另起采样。
            assert!(line.iter().all(|point| full_points.contains(&point)));
            // 整列落在卡片里时，每一段至少有一头露在卡片外面；贴边的列按余量整条保留。
            if cover.shrink(WOOD_REACH * 2.0).x_range().contains(line[0].x) {
                assert!(
                    line.windows(2)
                        .all(|pair| !cover.contains(pair[0]) || !cover.contains(pair[1]))
                );
            }
        }

        // 只露出一条窄缝时，只生成那一条附近的列和段。
        let visible = Rect::from_min_max(egui::pos2(300.0, 400.0), egui::pos2(330.0, 410.0));
        let slit = wood_shapes(rect, visible, None);
        let slit_lines = lines(&slit);
        assert!(slit_lines.len() <= 13);
        assert!(slit_lines.iter().all(|line| line.len() <= 3));
    }
}
