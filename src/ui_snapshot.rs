//! 测试用的软件渲染：把一帧 egui 输出画成 PNG，界面改版时目视检查。
//!
//! 只在测试里用（`cargo test ... -- --ignored` 跑出样张到 `tmp/`）。按 egui 的约定在 gamma 空间
//! 做预乘 alpha 混合，纹理双线性采样；不做多重采样——egui 自己用顶点 alpha 做了边缘羽化。

use eframe::egui;
use std::collections::HashMap;
use std::path::Path;

/// 一张纹理：宽、高、像素（预乘、gamma 空间）。
struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<egui::Color32>,
}

/// 跨帧攒下的纹理（字体图集、图标）。每帧的 `textures_delta` 都要交给 [`Canvas::absorb`]。
#[derive(Default)]
pub(crate) struct Canvas {
    textures: HashMap<egui::TextureId, Texture>,
}

impl Canvas {
    pub(crate) fn absorb(&mut self, delta: &egui::TexturesDelta) {
        for (id, change) in &delta.set {
            let egui::ImageData::Color(image) = &change.image;
            let [width, height] = image.size;
            match change.pos {
                None => {
                    self.textures.insert(
                        *id,
                        Texture {
                            width,
                            height,
                            pixels: image.pixels.clone(),
                        },
                    );
                }
                Some([x0, y0]) => {
                    if let Some(texture) = self.textures.get_mut(id) {
                        for y in 0..height {
                            for x in 0..width {
                                let (tx, ty) = (x0 + x, y0 + y);
                                if tx < texture.width && ty < texture.height {
                                    texture.pixels[ty * texture.width + tx] =
                                        image.pixels[y * width + x];
                                }
                            }
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    /// 把一帧画到 `path`。`size` 是屏幕大小（点）。
    pub(crate) fn render(
        &mut self,
        ctx: &egui::Context,
        output: egui::FullOutput,
        size: egui::Vec2,
        background: egui::Color32,
        path: &Path,
    ) {
        self.absorb(&output.textures_delta);
        let ppp = output.pixels_per_point;
        let width = (size.x * ppp).round() as usize;
        let height = (size.y * ppp).round() as usize;
        let bg = rgba(background);
        let mut buffer = vec![bg; width * height];
        for clipped in ctx.tessellate(output.shapes, ppp) {
            let egui::epaint::Primitive::Mesh(mesh) = clipped.primitive else {
                continue;
            };
            let Some(texture) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            let clip = egui::Rect::from_min_max(
                (clipped.clip_rect.min.to_vec2() * ppp).to_pos2(),
                (clipped.clip_rect.max.to_vec2() * ppp).to_pos2(),
            );
            for triangle in mesh.indices.as_chunks::<3>().0 {
                let [a, b, c] = triangle.map(|i| &mesh.vertices[i as usize]);
                raster(&mut buffer, width, height, clip, ppp, texture, [a, b, c]);
            }
        }
        let mut image = image::RgbaImage::new(width as u32, height as u32);
        for (pixel, value) in image.pixels_mut().zip(&buffer) {
            *pixel = image::Rgba(value.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        image.save(path).expect("写样张");
    }
}

fn rgba(color: egui::Color32) -> [f32; 4] {
    color.to_array().map(|v| v as f32 / 255.0)
}

fn sample(texture: &Texture, uv: egui::Pos2) -> [f32; 4] {
    let x = (uv.x * texture.width as f32 - 0.5).clamp(0.0, texture.width as f32 - 1.0);
    let y = (uv.y * texture.height as f32 - 0.5).clamp(0.0, texture.height as f32 - 1.0);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = (
        (x0 + 1).min(texture.width - 1),
        (y0 + 1).min(texture.height - 1),
    );
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |x: usize, y: usize| rgba(texture.pixels[y * texture.width + x]);
    let (p00, p10, p01, p11) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    std::array::from_fn(|i| {
        let top = p00[i] * (1.0 - fx) + p10[i] * fx;
        let bottom = p01[i] * (1.0 - fx) + p11[i] * fx;
        top * (1.0 - fy) + bottom * fy
    })
}

#[allow(clippy::too_many_arguments)]
fn raster(
    buffer: &mut [[f32; 4]],
    width: usize,
    height: usize,
    clip: egui::Rect,
    ppp: f32,
    texture: &Texture,
    [a, b, c]: [&egui::epaint::Vertex; 3],
) {
    let p = [a.pos, b.pos, c.pos].map(|pos| (pos.to_vec2() * ppp).to_pos2());
    let edge = |p0: egui::Pos2, p1: egui::Pos2, q: egui::Pos2| {
        (p1.x - p0.x) * (q.y - p0.y) - (p1.y - p0.y) * (q.x - p0.x)
    };
    let area = edge(p[0], p[1], p[2]);
    if area.abs() < 1e-6 {
        return;
    }
    let min_x = p
        .iter()
        .map(|q| q.x)
        .fold(f32::INFINITY, f32::min)
        .max(clip.min.x)
        .max(0.0);
    let max_x = p
        .iter()
        .map(|q| q.x)
        .fold(f32::NEG_INFINITY, f32::max)
        .min(clip.max.x)
        .min(width as f32);
    let min_y = p
        .iter()
        .map(|q| q.y)
        .fold(f32::INFINITY, f32::min)
        .max(clip.min.y)
        .max(0.0);
    let max_y = p
        .iter()
        .map(|q| q.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .min(clip.max.y)
        .min(height as f32);
    if min_x >= max_x || min_y >= max_y {
        return;
    }
    let colors = [a.color, b.color, c.color].map(rgba);
    for y in (min_y.floor() as usize)..(max_y.ceil() as usize).min(height) {
        for x in (min_x.floor() as usize)..(max_x.ceil() as usize).min(width) {
            let q = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
            if !clip.contains(q) {
                continue;
            }
            let w0 = edge(p[1], p[2], q) / area;
            let w1 = edge(p[2], p[0], q) / area;
            let w2 = edge(p[0], p[1], q) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let uv = egui::pos2(
                a.uv.x * w0 + b.uv.x * w1 + c.uv.x * w2,
                a.uv.y * w0 + b.uv.y * w1 + c.uv.y * w2,
            );
            let texel = sample(texture, uv);
            let src: [f32; 4] = std::array::from_fn(|i| {
                (colors[0][i] * w0 + colors[1][i] * w1 + colors[2][i] * w2) * texel[i]
            });
            let dst = &mut buffer[y * width + x];
            for i in 0..4 {
                dst[i] = src[i] + dst[i] * (1.0 - src[3]);
            }
        }
    }
}
