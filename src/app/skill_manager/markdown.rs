//! 本地文件的轻量 Markdown 预览：标题、段落、强调、链接、图片、代码和表格。
//! 相对链接在技能包内解析，不把资源链接交给浏览器或执行脚本。

use super::*;
use regex::Regex;
use std::sync::LazyLock;

static INLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(!?\[([^\]]+)\]\(([^)]+)\))|\*\*([^*]+)\*\*|`([^`]+)`").expect("Markdown 行内匹配")
});

pub(super) fn resolve(current: &str, target: &str) -> Option<String> {
    let target = target.split('#').next()?.split('?').next()?.trim();
    if target.is_empty() || target.contains(':') || target.starts_with(['/', '\\']) {
        return None;
    }
    let mut decoded = Vec::new();
    let bytes = target.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let target = String::from_utf8(decoded).ok()?.replace('\\', "/");
    if target.starts_with('/') || target.contains(':') {
        return None;
    }
    let mut parts = current.split('/').collect::<Vec<_>>();
    parts.pop();
    for part in target.split('/') {
        match part {
            "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    package::relative_path(&parts.join("/")).ok()
}

pub(super) fn preview(
    ui: &mut egui::Ui,
    text: &str,
    current: &str,
    files: &package::Package,
) -> Option<String> {
    let mut next = None;
    let mut lines = text.lines().peekable();
    if lines.peek().is_some_and(|line| line.trim() == "---") {
        lines.next();
        let mut yaml = Vec::new();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
            yaml.push(line);
        }
        egui::CollapsingHeader::new("技能配置 · YAML")
            .id_salt((current, "frontmatter"))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(yaml.join("\n"))
                        .monospace()
                        .color(theme::text_soft()),
                );
            });
        ui.add_space(8.0);
    }
    let mut paragraph = String::new();
    let mut fence: Option<(String, String)> = None;
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if let Some((marker, code)) = fence.as_mut() {
            if trimmed.starts_with(marker.as_str()) {
                egui::Frame::new()
                    .fill(theme::surface_sunk())
                    .corner_radius(6)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(code.trim_end()).monospace());
                    });
                ui.add_space(8.0);
                fence = None;
            } else {
                code.push_str(line);
                code.push('\n');
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(ui, &mut paragraph, current, files, &mut next);
            fence = Some((trimmed[..3].into(), String::new()));
        } else if trimmed.is_empty() {
            flush(ui, &mut paragraph, current, files, &mut next);
        } else if trimmed == "---" || trimmed == "***" {
            flush(ui, &mut paragraph, current, files, &mut next);
            ui.separator();
        } else if let Some((level, heading)) = heading(trimmed) {
            flush(ui, &mut paragraph, current, files, &mut next);
            ui.add_space(10.0);
            ui.label(egui::RichText::new(heading).strong().size(match level {
                1 => 25.0,
                2 => 19.0,
                _ => 16.0,
            }));
            ui.add_space(4.0);
        } else if trimmed.starts_with('|') && lines.peek().is_some_and(|line| table_separator(line))
        {
            flush(ui, &mut paragraph, current, files, &mut next);
            lines.next();
            let mut rows = vec![cells(trimmed)];
            while lines
                .peek()
                .is_some_and(|line| line.trim().starts_with('|'))
            {
                rows.push(cells(lines.next().expect("表格行")));
            }
            egui::Grid::new((current, "md_table", rows.len(), rows[0].join("|")))
                .striped(true)
                .spacing(egui::vec2(14.0, 8.0))
                .show(ui, |ui| {
                    for (index, row) in rows.iter().enumerate() {
                        for cell in row {
                            if index == 0 {
                                ui.strong(cell);
                            } else {
                                inline(ui, cell, current, files, &mut next);
                            }
                        }
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
        } else if trimmed.starts_with('>') {
            flush(ui, &mut paragraph, current, files, &mut next);
            egui::Frame::new()
                .fill(theme::surface_sunk())
                .inner_margin(10)
                .show(ui, |ui| {
                    inline(
                        ui,
                        trimmed.trim_start_matches('>').trim(),
                        current,
                        files,
                        &mut next,
                    );
                });
        } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") || ordered(trimmed) {
            flush(ui, &mut paragraph, current, files, &mut next);
            ui.horizontal_wrapped(|ui| {
                let shown = if trimmed.starts_with(['-', '*']) {
                    format!("• {}", &trimmed[2..])
                } else {
                    trimmed.to_string()
                };
                inline(ui, &shown, current, files, &mut next);
            });
        } else {
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(trimmed);
        }
    }
    flush(ui, &mut paragraph, current, files, &mut next);
    if let Some((_, code)) = fence {
        ui.label(egui::RichText::new(code).monospace());
    }
    next
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|b| *b == b'#').count();
    (level > 0 && level <= 6 && line.as_bytes().get(level) == Some(&b' '))
        .then(|| (level, line[level..].trim()))
}

fn ordered(line: &str) -> bool {
    let count = line.bytes().take_while(u8::is_ascii_digit).count();
    count > 0 && line[count..].starts_with(". ")
}

fn table_separator(line: &str) -> bool {
    line.contains('-')
        && line
            .trim()
            .chars()
            .all(|c| matches!(c, '|' | ':' | '-' | ' '))
}

fn cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn flush(
    ui: &mut egui::Ui,
    paragraph: &mut String,
    current: &str,
    files: &package::Package,
    next: &mut Option<String>,
) {
    if !paragraph.is_empty() {
        inline(ui, paragraph, current, files, next);
        paragraph.clear();
        ui.add_space(8.0);
    }
}

fn inline(
    ui: &mut egui::Ui,
    text: &str,
    current: &str,
    files: &package::Package,
    next: &mut Option<String>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut end = 0;
        for captures in INLINE.captures_iter(text) {
            let matched = captures.get(0).expect("行内标记");
            if matched.start() > end {
                ui.label(&text[end..matched.start()]);
            }
            if let (Some(label), Some(target)) = (captures.get(2), captures.get(3)) {
                let resolved = resolve(current, target.as_str());
                let exists = resolved
                    .as_ref()
                    .is_some_and(|path| files.files.contains_key(path));
                if matched.as_str().starts_with('!')
                    && exists
                    && resolved
                        .as_ref()
                        .is_some_and(|path| super::browser::is_image(path))
                {
                    let path = resolved.as_ref().expect("包内图片");
                    let bytes = &files.files[path];
                    let response = ui.add(
                        egui::Image::from_bytes(
                            super::browser::image_uri("preview", path, bytes),
                            bytes.clone(),
                        )
                        .max_width(ui.available_width().min(700.0))
                        .shrink_to_fit()
                        .sense(egui::Sense::click()),
                    );
                    if response.clicked() {
                        *next = resolved;
                    }
                } else {
                    let response = ui
                        .add_enabled(exists, egui::Link::new(label.as_str()))
                        .on_hover_text(if exists {
                            target.as_str().to_string()
                        } else {
                            format!("无法在技能包内打开：{}", target.as_str())
                        });
                    if response.clicked() {
                        *next = resolved;
                    }
                }
            } else if let Some(strong) = captures.get(4) {
                ui.strong(strong.as_str());
            } else if let Some(code) = captures.get(5) {
                ui.label(
                    egui::RichText::new(code.as_str())
                        .monospace()
                        .background_color(theme::surface_sunk()),
                );
            }
            end = matched.end();
        }
        if end < text.len() {
            ui.label(&text[end..]);
        }
    });
}
