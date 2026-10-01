//! 研究报告公式：latex-rust 进程内排版成 SVG（字形转成路径），交给 Typst 当图贴。
//!
//! 与预览（`preview::math_render`）同一个排版器、同一套数学字体（STIX Two Math），
//! 预览看到的公式就是 PDF 上的公式。STIX 没有的字（`\text{中文}`）回退到随包方正
//! 书宋，与正文同一字形。
//!
//! 每个公式按 10pt 排一次，尺寸折成 em 写回模板数据（`w` 宽、`h` 基线以上、`dp`
//! 基线以下、`pad` 四周留白），模板按所在字号缩放——表格、文框里的小四公式与正文
//! 四号公式用同一份 SVG。SVG 四周各留 `pad` 的白边：字形常顶出盒模型（积分号、
//! 分子），不留会被裁掉。

use std::collections::HashMap;
use std::sync::OnceLock;

use latex_rust::{BoxContent, Dim, MathBox, MathFont, MathStyle, SvgOptions};
use serde_json::Value;

/// 排版字号（pt）：SVG 按这个字号出，模板里再按 em 缩放。
const SIZE_PT: f32 = 10.0;
/// 四周留白（em）。
const PAD_EM: f32 = 0.5;

fn font() -> Result<&'static MathFont, String> {
    static FONT: OnceLock<Result<MathFont, String>> = OnceLock::new();
    FONT.get_or_init(|| {
        let fallback = crate::portable_runtime::find_font_dir()
            .and_then(|dir| std::fs::read(dir.join("FZShuSong.ttf")).ok())
            .map(|data| Box::leak(data.into_boxed_slice()) as &'static [u8]);
        match fallback {
            Some(bytes) => MathFont::stix_two_math_with_fallback(bytes).map_err(|e| e.to_string()),
            None => MathFont::stix_two_math().map_err(|e| e.to_string()),
        }
    })
    .as_ref()
    .map_err(Clone::clone)
}

fn dim_f32(dim: &Dim) -> f32 {
    f32::from_bits(dim.to_ieee32_bits())
}

fn dim_from_f32(value: f32) -> Dim {
    Dim::from_ieee32_bits(value.to_bits())
}

/// 盒子四周各加 `PAD_EM` 的留白：左边垫一段 kern，高、深、宽各加留白。
fn pad_box(boxed: &MathBox) -> MathBox {
    let pad = dim_from_f32(PAD_EM);
    let kern = MathBox {
        width: pad.clone(),
        height: Dim::zero(),
        depth: Dim::zero(),
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: BoxContent::Kern(pad.clone()),
    };
    let mut inner = boxed.clone();
    inner.shift = Dim::zero();
    MathBox {
        width: &(&boxed.width + &pad) + &pad,
        height: &boxed.height + &pad,
        depth: &boxed.depth + &pad,
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: BoxContent::HList(vec![kern, inner]),
    }
}

/// 一个排好的公式：SVG 与盒模型尺寸（em）。
#[derive(Clone)]
struct Rendered {
    svg: Vec<u8>,
    width: f32,
    height: f32,
    depth: f32,
}

fn render(source: &str, display: bool) -> Result<Rendered, String> {
    let font = font()?;
    let ast = latex_rust::parse(source).map_err(|e| e.to_string())?;
    let style = if display {
        MathStyle::Display
    } else {
        MathStyle::Text
    };
    let boxed = latex_rust::layout(&ast, font, style).map_err(|e| e.to_string())?;
    let mut options = SvgOptions::new();
    options.font_size_pt = dim_from_f32(SIZE_PT);
    let svg =
        latex_rust::render_svg(&pad_box(&boxed), font, &options).map_err(|e| e.to_string())?;
    Ok(Rendered {
        svg: svg.into_bytes(),
        width: dim_f32(&boxed.width),
        height: dim_f32(&boxed.height),
        depth: dim_f32(&boxed.depth),
    })
}

/// 把模板数据里的公式全部排好：行内片段 `{"t":"math"}` 与独立公式块 `{"k":"math"}`
/// 补上 SVG 路径与尺寸，SVG 放进 `files`（虚拟路径 → 内容）。排不出来的公式写上
/// `err`，模板照源码印出，提示收进 `warnings`。
pub(crate) fn render_all(
    data: &mut Value,
    files: &mut HashMap<String, Vec<u8>>,
    warnings: &mut Vec<String>,
) {
    let mut cache: HashMap<(String, bool), Result<(String, Rendered), String>> = HashMap::new();
    walk(data, &mut |node| {
        let display = match (node.get("t"), node.get("k")) {
            (Some(Value::String(t)), _) if t == "math" => {
                node.get("d").and_then(Value::as_bool).unwrap_or(false)
            }
            (_, Some(Value::String(k))) if k == "math" => true,
            _ => return,
        };
        let Some(source) = node.get("v").and_then(Value::as_str).map(str::to_string) else {
            return;
        };
        let entry = cache.entry((source.clone(), display)).or_insert_with(|| {
            render(&source, display).map(|rendered| {
                let path = format!("/math/{}.svg", files.len());
                files.insert(path.clone(), rendered.svg.clone());
                (path, rendered)
            })
        });
        let Some(map) = node.as_object_mut() else {
            return;
        };
        match entry {
            Ok((path, r)) => {
                map.insert("f".into(), path.clone().into());
                map.insert("w".into(), round(r.width).into());
                map.insert("h".into(), round(r.height).into());
                map.insert("dp".into(), round(r.depth).into());
                map.insert("pad".into(), PAD_EM.into());
            }
            Err(error) => {
                warnings.push(format!("公式无法排版，已按源码印出：{source}（{error}）"));
                map.insert("err".into(), error.clone().into());
            }
        }
    });
}

fn round(value: f32) -> f64 {
    (f64::from(value) * 1e5).round() / 1e5
}

fn walk(value: &mut Value, visit: &mut impl FnMut(&mut Value)) {
    match value {
        Value::Object(map) => {
            for child in map.values_mut() {
                walk(child, visit);
            }
        }
        Value::Array(items) => {
            for child in items {
                walk(child, visit);
            }
        }
        _ => return,
    }
    visit(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_and_display_math_get_svg_files_and_metrics() {
        let mut data = serde_json::json!({
            "blocks": [
                {"k": "par", "c": [{"t": "math", "v": "x^2", "d": false}]},
                {"k": "math", "v": "\\frac{1}{3}"},
                {"k": "par", "c": [{"t": "math", "v": "x^2", "d": false}]},
            ]
        });
        let mut files = HashMap::new();
        let mut warnings = Vec::new();
        render_all(&mut data, &mut files, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(files.len(), 2, "同一公式只排一次");
        let inline = &data["blocks"][0]["c"][0];
        assert_eq!(inline["f"], data["blocks"][2]["c"][0]["f"]);
        assert!(inline["w"].as_f64().unwrap() > 0.5);
        let block = &data["blocks"][1];
        assert!(block["h"].as_f64().unwrap() > 0.5, "{block}");
        let svg = String::from_utf8(files[block["f"].as_str().unwrap()].clone()).unwrap();
        assert!(svg.contains("<path"), "{svg}");
    }

    #[test]
    fn unsupported_math_is_reported_not_fatal() {
        let mut data = serde_json::json!({"t": "math", "v": "\\frac{", "d": false});
        let mut files = HashMap::new();
        let mut warnings = Vec::new();
        render_all(&mut data, &mut files, &mut warnings);
        assert!(data.get("err").is_some());
        assert_eq!(warnings.len(), 1);
    }
}
