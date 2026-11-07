//! 印制追溯码：标准长方形 Data Matrix，排版完成后按页面坐标叠加，不参与正文分页。
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use datamatrix::{DataMatrix, SymbolSize};
use lopdf::{Dictionary, Document, Object, Stream, dictionary};
use sha2::{Digest, Sha256};

use crate::manuscript::{ManuscriptStore, print_trace::TraceRecord};
use crate::models::{DraftInput, PrintTraceConfig};

impl PrintTraceConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.enabled {
            let unit = self.unit.trim();
            ensure!(
                !unit.is_empty(),
                "请先在「设置 → 导出格式」填写追溯码的印制单位"
            );
            ensure!(
                unit.chars().count() <= 60 && !unit.chars().any(char::is_control),
                "印制单位应为不超过 60 字的单行文字"
            );
        }
        Ok(())
    }
}

pub(crate) fn display_code(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|part| String::from_utf8_lossy(part))
        .collect::<Vec<_>>()
        .join("-")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn new_code() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let value = u64::from_be_bytes(bytes[..8].try_into().expect("固定八字节"));
    format!("{:016}", value % 10_000_000_000_000_000)
}

fn barcode_svg(code: &str) -> Result<Vec<u8>> {
    ensure!(
        code.len() == 16 && code.bytes().all(|b| b.is_ascii_digit()),
        "印制编号格式错误"
    );
    let matrix = DataMatrix::encode(code.as_bytes(), SymbolSize::Rect8x32)
        .map_err(|error| anyhow::anyhow!("印制追溯码编码失败：{error:?}"))?;
    let bitmap = matrix.bitmap();
    // 四周各留一模块静区；模块保持正方形，不拉伸码制。
    let mut svg = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 34 10\" width=\"34\" height=\"10\"><path fill=\"black\" d=\"",
    );
    for (x, y) in bitmap.pixels() {
        svg.push_str(&format!("M{} {}h1v1h-1z", x + 1, y + 1));
    }
    svg.push_str("\"/></svg>");
    Ok(svg.into_bytes())
}

/// 编译透明小幅标识，再以 Form XObject 叠加到每页右下角。
/// 本函数不访问登记库或写文件，样张和回归也使用完全相同的路径。
pub(crate) fn stamp_pdf(pdf: &[u8], config: &PrintTraceConfig, code: &str) -> Result<Vec<u8>> {
    config.validate()?;
    let stamp = crate::typst_engine::compile_print_trace(
        &config.unit,
        &display_code(code),
        barcode_svg(code)?,
    )?;
    let mut doc = Document::load_mem(pdf).context("读取待加印制追溯码的 PDF 失败")?;
    let mut overlay = Document::load_mem(&stamp)?;
    overlay.renumber_objects_with(doc.max_id + 1);
    let page_id = *overlay
        .get_pages()
        .values()
        .next()
        .context("印制标识没有页面")?;
    let page = overlay.get_dictionary(page_id)?;
    let bbox = page.get(b"MediaBox")?.clone();
    let resources = page.get(b"Resources")?.clone();
    let content = overlay.get_page_content(page_id);
    doc.max_id = overlay.max_id;
    doc.objects.extend(overlay.objects);
    let form_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form", "FormType" => 1,
            "BBox" => bbox, "Resources" => resources,
        },
        content,
    ));
    let mm = 72.0_f32 / 25.4;
    for &id in doc.get_pages().values() {
        let page = doc.get_dictionary(id)?;
        let rect = page.get(b"MediaBox")?.as_array()?;
        let left = rect[0].as_float()?;
        let bottom = rect[1].as_float()?;
        let width = rect[2].as_float()? - left;
        let height = rect[3].as_float()? - bottom;
        let rotation = page
            .get(b"Rotate")
            .and_then(Object::as_i64)
            .unwrap_or(0)
            .rem_euclid(360);
        // 以阅读器看到的页面右下角为准，兼容引擎的横页 /Rotate 90。
        let (a, b, c, d, x, y) = match rotation {
            0 => (1., 0., 0., 1., left + width - 34. * mm, bottom + 7. * mm),
            90 => (
                0.,
                1.,
                -1.,
                0.,
                left + width - 7. * mm,
                bottom + height - 34. * mm,
            ),
            180 => (-1., 0., 0., -1., left + 34. * mm, bottom + height - 7. * mm),
            270 => (0., -1., 1., 0., left + 7. * mm, bottom + 34. * mm),
            _ => anyhow::bail!("不支持的 PDF 页面旋转角度"),
        };
        let mut resources = page_resources(&doc, id)?;
        let mut xobjects = match resources.get(b"XObject") {
            Ok(obj) => doc.dereference(obj)?.1.as_dict()?.clone(),
            Err(_) => Dictionary::new(),
        };
        ensure!(!xobjects.has(b"GWPrintTrace"), "该 PDF 已有印制追溯码");
        xobjects.set("GWPrintTrace", form_id);
        resources.set("XObject", xobjects);
        doc.get_dictionary_mut(id)?.set("Resources", resources);
        // 将原有内容包在 q/Q 中，避免其残余变换或裁切影响叠加层。
        let original = doc.get_page_contents(id);
        let before = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
        let after = doc.add_object(Stream::new(
            Dictionary::new(),
            format!("\nQ\nq {a} {b} {c} {d} {x} {y} cm /GWPrintTrace Do Q\n").into_bytes(),
        ));
        let mut contents = vec![Object::Reference(before)];
        contents.extend(original.into_iter().map(Object::Reference));
        contents.push(Object::Reference(after));
        doc.get_dictionary_mut(id)?.set("Contents", contents);
    }
    doc.prune_objects();
    doc.compress();
    let mut result = Vec::new();
    doc.save_to(&mut result)?;
    Ok(result)
}

fn page_resources(doc: &Document, id: lopdf::ObjectId) -> Result<Dictionary> {
    let (direct, inherited) = doc.get_page_resources(id)?;
    let mut resources = Dictionary::new();
    for id in inherited.iter().rev() {
        for (key, value) in doc.get_dictionary(*id)?.iter() {
            resources.set(key.clone(), value.clone());
        }
    }
    if let Some(direct) = direct {
        for (key, value) in direct.iter() {
            resources.set(key.clone(), value.clone());
        }
    }
    Ok(resources)
}

/// 只处理本轮已生成的 PDF。失败时移除本轮 PDF，不把无标识文件当成成功成品。
pub(crate) fn stamp_outputs(
    files: &[PathBuf],
    config: &PrintTraceConfig,
    input: &DraftInput,
    markdown: &str,
) -> Result<()> {
    let pdfs: Vec<_> = files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .collect();
    if pdfs.is_empty() {
        return Ok(());
    }
    let result: Result<()> = (|| {
        let store = ManuscriptStore::open(&crate::storage::manuscript_db_path()?)?;
        let source_hash = sha256_hex(&serde_json::to_vec(&(input, markdown))?);
        for path in &pdfs {
            let code = loop {
                let code = new_code();
                if store.find_print_traces(&code)?.is_empty() {
                    break code;
                }
            };
            let bytes = mdx::memory_files::read_output(path)?;
            let stamped = stamp_pdf(&bytes, config, &code)?;
            let record = TraceRecord {
                code,
                unit: config.unit.trim().into(),
                created_at: chrono::Local::now().to_rfc3339(),
                title: super::document_title(input, markdown),
                source_hash: source_hash.clone(),
                pdf_hash: sha256_hex(&stamped),
            };
            if mdx::memory_files::output_is_captured() {
                mdx::memory_files::write_output(path, &stamped)?;
            } else {
                crate::storage::atomic_write(path, |file| {
                    use std::io::Write as _;
                    file.write_all(&stamped)?;
                    Ok(())
                })?;
            }
            store.register_print_trace(&record)?;
        }
        Ok(())
    })();
    if result.is_err() && !mdx::memory_files::output_is_captured() {
        for path in pdfs {
            let _ = std::fs::remove_file(path);
        }
    }
    result.context("印制追溯登记失败，本轮 PDF 未保留")
}

#[cfg(test)]
mod tests;
