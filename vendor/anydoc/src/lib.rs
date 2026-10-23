//! anydoc converts documents to GitHub-Flavored Markdown.
//!
//! Recovery and skipped-content events are reported through the [`log`]
//! facade (debug/warn level); logging never changes conversion behavior and
//! its messages are not a stable API.

#![warn(missing_docs)]

pub mod model;

mod error;
mod formats;
mod package;
mod render;
mod shared;

pub use error::ConvertError;

use std::path::Path;

/// Serialize a parsed [`model::Document`] to Markdown, exactly as
/// [`to_markdown_bytes`] does after parsing. Lets callers rewrite the model
/// first, e.g. point embedded images at files they stored from
/// [`model::Document::assets`].
///
/// gongwen patch: upstream keeps the renderer private; see vendor/anydoc/README.md.
pub fn document_to_markdown(doc: &model::Document) -> String {
    render::markdown::document_to_markdown(doc)
}

/// Input format. Selects the parser; container variants that share a parser
/// (docm, xlsm, ...) map onto these via [`Format::from_bytes`] or
/// [`Format::from_extension`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Binary Word 97-2003 (`.doc`).
    Doc,
    /// WordprocessingML (`.docx`, `.docm`), both Transitional and Strict.
    Docx,
    /// OpenDocument Text (`.odt`).
    Odt,
    /// Converted with [pdf-inspector], which emits Markdown directly:
    /// [`to_document`] is unsupported for PDFs. Scanned or image-only pages
    /// need OCR, which anydoc does not do: the document errors with
    /// [`ConvertError::NeedsOcr`] naming them.
    ///
    /// [pdf-inspector]: https://github.com/firecrawl/pdf-inspector
    Pdf,
    /// Binary PowerPoint 97-2003 (`.ppt`, `.pps`, `.pot`).
    Ppt,
    /// PresentationML (`.pptx`, `.pptm`, `.ppsx`, `.ppsm`).
    Pptx,
    /// Rich Text Format (`.rtf`).
    Rtf,
    /// EPUB 2 and 3 (`.epub`).
    Epub,
    /// Excel workbooks: `.xlsx`, `.xlsm`, binary `.xlsb`, and legacy
    /// OLE-based `.xls`.
    Excel,
    /// OpenDocument Spreadsheet (`.ods`).
    Ods,
    /// OpenDocument Presentation (`.odp`).
    Odp,
    /// Delimiter-separated text (`.csv`). Carries no signature, so it has to
    /// be named rather than detected.
    Csv,
}

impl Format {
    /// Detect the format from the content itself: the signature and identity
    /// each container specification designates (PDF header, RTF open group,
    /// OLE stream names, ZIP package mimetype/content types). Plain-text
    /// formats (CSV) carry no signature and return `None`; so does anything
    /// unrecognized.
    pub fn from_bytes(bytes: &[u8]) -> Option<Format> {
        formats::detect::from_bytes(bytes)
    }

    /// The format a bare extension names (no leading dot), matched
    /// case-insensitively. `None` for anything unrecognized.
    pub fn from_extension(ext: &str) -> Option<Format> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "doc" => Format::Doc,
            "docx" | "docm" => Format::Docx,
            "odt" => Format::Odt,
            "pdf" => Format::Pdf,
            "pptx" | "pptm" | "ppsx" | "ppsm" => Format::Pptx,
            "ppt" | "pps" | "pot" => Format::Ppt,
            "rtf" => Format::Rtf,
            "epub" => Format::Epub,
            "xlsx" | "xlsm" | "xlsb" | "xls" => Format::Excel,
            "ods" => Format::Ods,
            "odp" => Format::Odp,
            "csv" => Format::Csv,
            _ => return None,
        })
    }

    /// The format a path's extension names. `None` when the path has no
    /// extension or names nothing recognized.
    pub fn from_path(path: &Path) -> Option<Format> {
        path.extension().and_then(|e| e.to_str()).and_then(Format::from_extension)
    }
}

/// Convert a document file to Markdown. The format is detected from the
/// file content ([`Format::from_bytes`]); the extension is the fallback for
/// signature-less formats (CSV) and unrecognizable containers.
pub fn to_markdown(path: impl AsRef<Path>) -> Result<String, ConvertError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)?;
    let Some(format) = Format::from_bytes(&bytes).or_else(|| Format::from_path(path)) else {
        return Err(ConvertError::Unsupported(format!(
            "unrecognized file content and extension: {}",
            path.display()
        )));
    };
    to_markdown_bytes(&bytes, format)
}

/// Convert an in-memory document to Markdown. Pass a [`Format`] to select the
/// parser, or `None` to detect it from the content ([`Format::from_bytes`]),
/// which signature-less formats (CSV) have to name explicitly.
pub fn to_markdown_bytes(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
) -> Result<String, ConvertError> {
    let format = resolve_format(bytes, format.into())?;
    // PDFs convert to Markdown directly (pdf-inspector) without passing
    // through the document model.
    if format == Format::Pdf {
        return formats::pdf::to_markdown(bytes);
    }
    Ok(document_to_markdown(&to_document(bytes, format)?))
}

/// Parse an in-memory document into the document model. Pass a [`Format`] to
/// select the parser, or `None` to detect it from the content.
///
/// Unsupported for [`Format::Pdf`]: PDF conversion produces Markdown
/// directly and has no document-model form; use [`to_markdown_bytes`].
pub fn to_document(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
) -> Result<model::Document, ConvertError> {
    formats::parse(bytes, resolve_format(bytes, format.into())?)
}

/// gongwen patch: HTML element tree -> document model, for clipboard paste.
///
/// The EPUB frontend uses the same tree walker, but it lives in a private
/// module and is only reachable through a package parser. Pasting HTML from a
/// browser or Word needs the body element the caller already built, plus the
/// page's stylesheet text when the source kept it.
///
/// Links keep their label but lose the target (the paste pipeline renders
/// plain text only), and images are never fetched: `image_source` reports
/// "no source", so a fragment carrying `src` URLs cannot reach the network.
pub mod html {
    pub use crate::package::xml::{Attr, Element, Node};

    use crate::model::{AnchorId, Asset, AssetId, Document, ImageSource, LinkTarget};
    use crate::shared::html::{HtmlCtx, Stylesheet};
    use std::cell::RefCell;

    /// gongwen patch: image bytes a clipboard fragment brought along, together
    /// with the media type declared for them (a data URL carries one; a local
    /// file path usually leaves it empty). The type lets the caller sort out
    /// unsupported formats (SVG, TIFF) without sniffing the bytes itself.
    #[derive(Debug, Clone)]
    pub struct PastedImage {
        /// Declared media type such as image/png; may be empty.
        pub media_type: String,
        /// Raw image bytes.
        pub bytes: Vec<u8>,
    }

    /// Walk `body`'s children into a [`Document`]. `css` is the stylesheet text
    /// collected from the page (may be empty). The result carries no notes or
    /// assets: clipboard fragments never come with them.
    pub fn to_document(body: &Element, css: &str) -> Result<Document, crate::ConvertError> {
        collect(body, css, &|_src: &str| None)
    }

    /// gongwen patch: like to_document, but resolve may hand back the bytes of
    /// an image src (typically an inline data URL or a local file path the
    /// caller read itself). Resolved images become entries in
    /// Document::assets and are referenced with ImageSource::Asset;
    /// unresolved ones stay ImageSource::Unavailable. Nothing here touches the
    /// network or the filesystem: resolve owns that policy, and returning None
    /// simply leaves the image without a source.
    pub fn to_document_with_images(
        body: &Element,
        css: &str,
        resolve: &dyn Fn(&str) -> Option<PastedImage>,
    ) -> Result<Document, crate::ConvertError> {
        collect(body, css, resolve)
    }

    fn collect(
        body: &Element,
        css: &str,
        resolve: &dyn Fn(&str) -> Option<PastedImage>,
    ) -> Result<Document, crate::ConvertError> {
        let mut sheet = Stylesheet::default();
        sheet.add(css);
        let assets = RefCell::new(Vec::new());
        let ctx = PasteCtx { resolve, assets: &assets };
        let blocks = crate::shared::html::to_blocks(body, &sheet, &ctx)?;
        Ok(Document { blocks, assets: assets.into_inner(), ..Document::default() })
    }

    /// Clipboard fragments have no document to resolve hrefs, image parts, or
    /// anchor ids against.
    struct PasteCtx<'a> {
        resolve: &'a dyn Fn(&str) -> Option<PastedImage>,
        assets: &'a RefCell<Vec<Asset>>,
    }

    impl HtmlCtx for PasteCtx<'_> {
        fn link_target(&self, _href: &str) -> Option<LinkTarget> {
            None
        }

        fn image_source(&self, src: &str) -> Result<Option<ImageSource>, crate::ConvertError> {
            let Some(image) = (self.resolve)(src) else {
                return Ok(None);
            };
            let mut assets = self.assets.borrow_mut();
            let id = AssetId(assets.len());
            assets.push(Asset {
                id,
                media_type: image.media_type,
                origin_part: "clipboard".to_string(),
                bytes: image.bytes,
            });
            Ok(Some(ImageSource::Asset(id)))
        }

        fn anchor_id(&self, raw: &str) -> AnchorId {
            raw.to_string()
        }
    }
}

fn resolve_format(bytes: &[u8], format: Option<Format>) -> Result<Format, ConvertError> {
    format.or_else(|| Format::from_bytes(bytes)).ok_or_else(|| {
        ConvertError::Unsupported("unrecognized file content: name the format explicitly".into())
    })
}
