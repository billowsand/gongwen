//! 技能配置到 Mermaid 的确定性转换，以及随界面主题更新的内存纹理缓存。

use super::*;
use crate::agent::skill::StepSpec;

#[derive(Default)]
pub(super) struct DiagramCache {
    colors: Option<[egui::Color32; 6]>,
    pixels_per_point: f32,
    max_texture_side: usize,
    images: BTreeMap<String, Result<(egui::TextureHandle, egui::Vec2), String>>,
}

impl DiagramCache {
    pub(super) fn show(&mut self, ui: &mut egui::Ui, steps: &[StepSpec], depth: usize) {
        if steps.is_empty() {
            return;
        }
        let palette = theme::current();
        let colors = [
            palette.surface_sunk,
            palette.border_strong,
            palette.text,
            palette.text_soft,
            palette.surface,
            palette.border,
        ];
        let pixels_per_point = ui.ctx().pixels_per_point();
        let max_texture_side = ui.input(|input| input.max_texture_side);
        if self.colors != Some(colors)
            || self.pixels_per_point != pixels_per_point
            || self.max_texture_side != max_texture_side
        {
            self.images.clear();
            self.colors = Some(colors);
            self.pixels_per_point = pixels_per_point;
            self.max_texture_side = max_texture_side;
        }
        // 主流程与子流程始终横向排，宽度不足时按面板等比缩小。
        let source = source(steps);
        if !self.images.contains_key(&source) {
            if self.images.len() >= 32 {
                self.images.clear();
            }
            let image = crate::mermaid::render_interface(
                &source,
                colors,
                pixels_per_point,
                max_texture_side,
            )
            .map(|(image, size)| {
                (
                    ui.ctx()
                        .load_texture("skill-workflow", image, egui::TextureOptions::LINEAR),
                    size,
                )
            })
            .map_err(|error| format!("{error:#}"));
            self.images.insert(source.clone(), image);
        }
        match &self.images[&source] {
            Ok((texture, natural_size)) => {
                let size = display_size(*natural_size, ui.available_width());
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    ui.add(egui::Image::new((texture.id(), *natural_size)).fit_to_exact_size(size));
                });
            }
            Err(error) => {
                ui.colored_label(theme::warn(), format!("流程图暂时无法显示：{error}"));
            }
        }
        if depth < 8 {
            for (index, step) in steps.iter().enumerate().filter(|(_, s)| !s.body.is_empty()) {
                egui::CollapsingHeader::new(format!("{} · 子流程", super::detail::step_name(step)))
                    .id_salt(("skill_subflow", depth, index, step.label()))
                    .show(ui, |ui| self.show(ui, &step.body, depth + 1));
            }
        }
    }
}

pub(super) fn display_size(size: egui::Vec2, width: f32) -> egui::Vec2 {
    size * (width.max(0.0) / size.x).min(1.0)
}

/// 标题与条件均只作标签，不将配置中的字符串当成 Mermaid 语句执行。
fn escape(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '&' | '"' | '<' | '>' | '\n' | '\r' | '[' | ']' | '{' | '}' | '#' | ';' | '\\' => {
                format!("#{};", ch as u32)
            }
            _ => ch.to_string(),
        })
        .collect()
}

pub(super) fn source(steps: &[StepSpec]) -> String {
    let mut source = String::from("flowchart LR\n");
    for (index, step) in steps.iter().enumerate() {
        let mut label = format!("{:02}  {}", index + 1, super::detail::step_name(step));
        if let Some(condition) = &step.when {
            let condition = match condition.as_str() {
                Some("has_sources") => "有资料时",
                Some("has_text") => "有正文时",
                _ => "按条件执行",
            };
            label.push_str(&format!("（{condition}）"));
        }
        if !step.body.is_empty() {
            label.push_str("（含子流程）");
        }
        source.push_str(&format!("  n{index}[\"{}\"]\n", escape(&label)));
        if index > 0 {
            source.push_str(&format!("  n{} --> n{index}\n", index - 1));
        }
    }
    source
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_flows_and_subflows_render_and_fit_narrow_panels() {
        let colors = colors(crate::models::ThemeName::Green);
        for skill in skill::builtin_skills() {
            let mut flows = vec![skill.flow.as_slice()];
            while let Some(steps) = flows.pop() {
                flows.extend(
                    steps
                        .iter()
                        .filter(|s| !s.body.is_empty())
                        .map(|s| s.body.as_slice()),
                );
                {
                    let source = source(steps);
                    let (image, size) =
                        crate::mermaid::render_interface(&source, colors, 1.5, 2048)
                            .unwrap_or_else(|error| panic!("{}：{error:#}", skill.id));
                    assert!(image.pixels.iter().any(|pixel| pixel.a() > 0));
                    assert!(image.width() <= 2048 && image.height() <= 2048);
                    assert_eq!(image.pixels[0].a(), 0, "流程图背景应透明");
                    for width in [100.0, 360.0, 1100.0] {
                        let displayed = display_size(size, width);
                        assert!(displayed.x <= width + 0.1);
                        assert!((displayed.x / displayed.y - size.x / size.y).abs() < 0.001);
                    }
                }
            }
        }
    }

    fn colors(name: crate::models::ThemeName) -> [egui::Color32; 6] {
        let theme = theme::by_name(name);
        [
            theme.surface_sunk,
            theme.border_strong,
            theme.text,
            theme.text_soft,
            theme.surface,
            theme.border,
        ]
    }

    #[test]
    fn interface_diagrams_use_each_theme_and_a_transparent_canvas() {
        for name in crate::models::ThemeName::ALL {
            let colors = colors(name);
            let (image, _) = crate::mermaid::render_interface(
                "flowchart LR\n A[开始] --> B[完成]",
                colors,
                1.0,
                2048,
            )
            .unwrap();
            assert_eq!(image.pixels[0].a(), 0);
            assert!(
                image.pixels.contains(&colors[0]),
                "{name:?} 未使用主题节点底色"
            );
        }
    }

    #[test]
    fn labels_cannot_inject_mermaid_nodes_or_html() {
        let step: StepSpec = serde_json::from_value(serde_json::json!({
            "tool": "a\"] --> injected[\"<b>&#quot;",
            "when": {"var": "x\nend\nsubgraph injected"},
        }))
        .unwrap();
        let source = source(&[step]);
        assert_eq!(source.lines().count(), 2);
        assert!(!source.contains("<b>"));
        assert!(!source.contains("\"] --> injected"));
        crate::mermaid::render_interface(
            &source,
            colors(crate::models::ThemeName::Green),
            1.0,
            2048,
        )
        .unwrap();
    }
}
