//! 预览里的 PDF 插图：把第一页光栅化成一张纹理，画在版心里。
//!
//! 插进公文和研究报告的图片可以是 PDF（矢量图从 Visio、matplotlib、draw.io 导
//! 出来大多是这个格式），导出时 `\includegraphics` 按原矢量嵌进去，预览这边却
//! 一直只画一张占位卡片——排得合不合适只能等编译完再看。
//!
//! 光栅化走的是应用里已有的 `hayro`（PDF 阅读页用的同一套）。要紧的是别让它挡
//! 住画面：取字节、解析和渲染都放后台线程，主线程该画什么画什么，渲完再敲醒界面。
//! 同一个对象只渲一次，纹理按「逻辑引用 + 内容指纹」缓存在 egui 的 `Context` 里，
//! 缩放时由 GPU 采样，不重渲染。字节从哪来由调用方给一个加载闭包决定：预览走
//! 资源库（`images::read`），测试和外部基准目录可以自己给夹具。

use eframe::egui;
use hayro::{
    RenderCache, RenderSettings,
    hayro_interpret::{InterpreterSettings, hayro_syntax::Pdf},
    vello_cpu::color::palette::css::WHITE,
};
use std::sync::{Arc, Mutex};

/// 光栅化宽度。预览放到最大倍率，版心也就 950 逻辑像素宽，渲到 1200 已经够
/// 用；再往上纹理按平方涨，清晰度却看不出差别。
const RENDER_WIDTH: f32 = 1200.0;
/// 纹理高度上限。极窄极长的页面按它反过来压宽度，免得纹理边长超出显卡的上限。
const MAX_HEIGHT: f32 = 4000.0;

/// 后台线程填这个槽位；主线程下一帧取走。
type Slot = Arc<Mutex<Option<Result<egui::ColorImage, String>>>>;

#[derive(Clone)]
enum Thumb {
    Rendering(Slot),
    Ready(egui::TextureHandle),
    Failed(String),
}

/// 一张 PDF 插图此刻的状态。
pub(crate) enum PdfPage {
    /// 后台还在读取/渲染，这一帧先画占位卡片。
    Rendering,
    Ready(egui::TextureHandle),
    Failed(String),
}

/// 取这份 PDF 第一页的纹理；没有就地渲染，只登记一次后台任务。
///
/// `src` 是 Markdown 里的逻辑引用，`stamp` 是资源内容指纹（库里是内容 SHA
/// 前缀，旧文件是修改时间）：同一个路径换了对象就是另一张纹理，不必手动清缓存。
///
/// `load` 只在缓存未命中时被后台线程调用一次，负责把字节读出来（资源库优先，
/// 旧文件回退由 `images::read` 处理）；缓存命中时闭包直接丢弃。
pub(crate) fn page(
    ctx: &egui::Context,
    src: &str,
    stamp: u64,
    load: impl FnOnce() -> Result<Arc<Vec<u8>>, String> + Send + 'static,
) -> PdfPage {
    let id = egui::Id::new(("gw_pdf_figure", src, stamp));

    match ctx.data(|data| data.get_temp::<Thumb>(id)) {
        None => {
            let slot: Slot = Arc::new(Mutex::new(None));
            spawn(load, slot.clone(), ctx.clone());
            ctx.data_mut(|data| data.insert_temp(id, Thumb::Rendering(slot)));
            PdfPage::Rendering
        }
        Some(Thumb::Rendering(slot)) => {
            // 线程只写一次就结束，这里拿不到锁说明正好在写，下一帧再看。
            let Ok(mut done) = slot.try_lock() else {
                return PdfPage::Rendering;
            };
            match done.take() {
                None => PdfPage::Rendering,
                Some(Ok(image)) => {
                    let texture = ctx.load_texture(
                        format!("gw_pdf_figure/{src}@{stamp}"),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    ctx.data_mut(|data| data.insert_temp(id, Thumb::Ready(texture.clone())));
                    PdfPage::Ready(texture)
                }
                Some(Err(error)) => {
                    ctx.data_mut(|data| data.insert_temp(id, Thumb::Failed(error.clone())));
                    PdfPage::Failed(error)
                }
            }
        }
        Some(Thumb::Ready(texture)) => PdfPage::Ready(texture),
        Some(Thumb::Failed(error)) => PdfPage::Failed(error),
    }
}

fn spawn(
    load: impl FnOnce() -> Result<Arc<Vec<u8>>, String> + Send + 'static,
    slot: Slot,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        // 读字节（库里可能还要解密）与光栅化都留在后台线程。
        let result = load().and_then(|bytes| rasterize(&bytes));
        if let Ok(mut done) = slot.lock() {
            *done = Some(result);
        }
        // 主线程可能正闲着睡着，主动敲醒它。
        ctx.request_repaint();
    });
}

fn rasterize(bytes: &[u8]) -> Result<egui::ColorImage, String> {
    let pdf =
        Pdf::new(bytes.to_vec()).map_err(|error| format!("PDF 文件结构无法解析：{error:?}"))?;
    let pages = pdf.pages();
    let Some(page) = pages.first() else {
        return Err("PDF 中没有可显示的页面".into());
    };
    let (page_width, page_height) = page.render_dimensions();
    if ![page_width, page_height]
        .iter()
        .all(|size| size.is_finite() && *size > 0.0)
    {
        return Err("PDF 页面尺寸无效".into());
    }
    let width = RENDER_WIDTH.min(MAX_HEIGHT * page_width / page_height);
    let scale = width / page_width;
    // hayro 出的是预乘 alpha，直接按预乘读；逐像素转换也留在这条线程上。
    let pixmap = hayro::render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: Some(width.max(1.0) as u16),
            bg_color: WHITE,
            ..Default::default()
        },
    );
    Ok(egui::ColorImage::from_rgba_premultiplied(
        [usize::from(pixmap.width()), usize::from(pixmap.height())],
        pixmap.data_as_u8_slice(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 一页最小 PDF，避免测试依赖系统 PDF 程序或原生动态库。
    fn minimal_pdf() -> Vec<u8> {
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0usize];
        let objects: [&[u8]; 4] = [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R >>",
            b"<< /Length 44 >>\nstream\n0 0 1 rg 20 20 160 260 re f\nendstream",
        ];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            bytes.extend_from_slice(object);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
        bytes.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets.iter().skip(1) {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len()
            )
            .as_bytes(),
        );
        bytes
    }

    /// 加载闭包：返回夹具字节，并记一次调用。
    fn fixture_load(calls: Arc<AtomicUsize>) -> impl FnOnce() -> Result<Arc<Vec<u8>>, String> {
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(minimal_pdf()))
        }
    }

    #[test]
    fn the_first_page_is_rasterized_at_the_target_width() {
        let image = rasterize(&minimal_pdf()).expect("最小 PDF 应能光栅化");

        assert_eq!(image.width(), RENDER_WIDTH as usize);
        // 200×300 的页面，高宽比要保住。
        assert_eq!(image.height(), (RENDER_WIDTH * 1.5) as usize);
    }

    /// 首帧只登记后台任务，纹理在随后的某一帧才到位——界面不会卡在这上面。
    #[test]
    fn the_texture_shows_up_after_the_background_render_not_during_the_first_frame() {
        let ctx = egui::Context::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let stamp = 7;

        assert!(
            matches!(
                page(&ctx, "images/图.pdf", stamp, fixture_load(calls.clone())),
                PdfPage::Rendering
            ),
            "首帧不该就地读取或光栅化"
        );

        let texture = (0..200)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                match page(&ctx, "images/图.pdf", stamp, fixture_load(calls.clone())) {
                    PdfPage::Ready(texture) => Some(texture),
                    PdfPage::Failed(error) => panic!("光栅化失败：{error}"),
                    PdfPage::Rendering => None,
                }
            })
            .expect("后台渲染应在两秒内完成");

        assert_eq!(
            texture.size(),
            [RENDER_WIDTH as usize, RENDER_WIDTH as usize * 3 / 2]
        );
        // 第二次要命中缓存，不再重渲，也不再调用加载闭包。
        assert!(matches!(
            page(&ctx, "images/图.pdf", stamp, fixture_load(calls.clone())),
            PdfPage::Ready(_)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "同一个对象只加载一次");
        // 同一路径换了对象（stamp 变了）要重新渲染。
        assert!(matches!(
            page(
                &ctx,
                "images/图.pdf",
                stamp + 1,
                fixture_load(calls.clone())
            ),
            PdfPage::Rendering
        ));
    }

    #[test]
    fn a_broken_file_reports_instead_of_panicking() {
        let error = rasterize(b"not a pdf at all").expect_err("坏文件应当报错");
        assert!(error.contains("PDF"), "{error}");

        // 加载闭包失败也要变成 Failed，而不是 panic 或永远渲染中。
        let ctx = egui::Context::default();
        assert!(matches!(
            page(&ctx, "images/坏.pdf", 1, || Err(
                "无法读取：文件缺失".into()
            )),
            PdfPage::Rendering
        ));
        let failed = (0..200).find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            match page(&ctx, "images/坏.pdf", 1, || Err("无法读取".into())) {
                PdfPage::Failed(error) => Some(error),
                PdfPage::Ready(_) => panic!("坏文件不应渲染成功"),
                PdfPage::Rendering => None,
            }
        });
        assert_eq!(failed.as_deref(), Some("无法读取：文件缺失"));
    }
}
