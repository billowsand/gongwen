//! 送批材料合并导出：按导出计划逐件编译，合成一个 PDF。
//!
//! 方案见 `docs/send-package-design.md` 第 5 节。几条硬规矩：
//! - 每件用计划里取好的版本快照编译，版式与它单独导出时完全一样，页码各自独立；
//! - 每一部分（目录页、主件、各随行件）都从奇数页开始：页数为奇数的部分后面补一张
//!   与其末页同尺寸的空白页（最后一部分不补），双面打印后各件能拆开分别装订；
//! - 任何一件编译失败整个导出失败，先写临时文件、成功后才改名到目标路径，
//!   不留缺件的半成品，也不截断已有的同名文件。
//!
//! 编译函数由调用方传入：正式导出走 TeX，测试里换成现成的 PDF 字节。

use crate::manuscript::send_package::{ExportItemRecord, SendPackagePlan};
use crate::models::{DraftInput, TemplateKind, TemplateProfile};
use anyhow::{Context, Result, bail};
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::path::{Path, PathBuf};

/// 目录页的标题。
pub const TOC_TITLE: &str = "送批材料目录";

/// 合并后一部分的页数与其后补的空白页数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartPages {
    pub pages: usize,
    pub blanks: usize,
}

/// 一次合并导出的结果，供写导出记录与提示用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendPackageOutcome {
    pub path: PathBuf,
    pub total_pages: usize,
    pub toc: Option<PartPages>,
    /// 主件在前，随行件按顺序；页数不含补的空白页。
    pub items: Vec<ExportItemRecord>,
}

/// 按计划导出。`compile(要素, 正文, 文件名主干)` 返回一件的 PDF 字节；
/// `progress` 收到的是给状态栏看的进度文字。
pub fn export_send_package(
    plan: &SendPackagePlan,
    with_toc: bool,
    output: &Path,
    mut compile: impl FnMut(&DraftInput, &str, &str) -> Result<Vec<u8>>,
    mut progress: impl FnMut(&str),
) -> Result<SendPackageOutcome> {
    if let Some(entry) = plan.entries.iter().find(|entry| entry.revision.is_none()) {
        let reason = entry.blocker.map(|b| b.reason()).unwrap_or("取不到版本");
        let title = if entry.title.is_empty() {
            "（本机未找到的稿件）"
        } else {
            entry.title.as_str()
        };
        bail!("《{title}》不能进包：{reason}");
    }
    let total = plan.entries.len();
    let mut parts = Vec::with_capacity(total + 1);
    let mut page_counts = Vec::with_capacity(total);
    for (index, entry) in plan.entries.iter().enumerate() {
        let revision = entry.revision.as_ref().expect("上面已拦下无版本的件");
        progress(&format!(
            "正在编译第 {}/{total} 件：《{}》…",
            index + 1,
            entry.title
        ));
        let stem = crate::export::document_stem_prefix(&revision.snapshot, &entry.title);
        let pdf = compile(&revision.snapshot, &revision.content_markdown, &stem)
            .with_context(|| format!("《{}》编译失败", entry.title))?;
        page_counts
            .push(page_count(&pdf).with_context(|| format!("《{}》的 PDF 无法读取", entry.title))?);
        parts.push(pdf);
    }
    if with_toc {
        progress("正在排目录页…");
        let owner = plan.entries[0]
            .revision
            .as_ref()
            .expect("上面已拦下无版本的件");
        let toc = compile(
            &toc_snapshot(&owner.snapshot),
            &toc_markdown(plan, &page_counts),
            "送批材料目录",
        )
        .context("目录页编译失败")?;
        parts.insert(0, toc);
    }
    progress("正在合并 PDF…");
    let merged = merge_pdfs(&parts)?;
    write_atomically(output, &merged.bytes)?;

    let (toc, item_parts) = if with_toc {
        (Some(merged.parts[0]), &merged.parts[1..])
    } else {
        (None, &merged.parts[..])
    };
    let items = plan
        .entries
        .iter()
        .zip(item_parts)
        .enumerate()
        .map(|(index, (entry, part))| {
            let revision = entry.revision.as_ref().expect("上面已拦下无版本的件");
            ExportItemRecord {
                sort_order: index as i64,
                document_uuid: entry.document_uuid.clone(),
                revision_uuid: revision.revision_uuid.clone(),
                payload_hash: revision.payload_hash.clone(),
                visible_number: revision.visible_number,
                title: entry.title.clone(),
                kind: entry.kind,
                page_count: part.pages as i64,
                blank_pages: part.blanks as i64,
            }
        })
        .collect();
    Ok(SendPackageOutcome {
        path: output.to_path_buf(),
        total_pages: merged.total_pages,
        toc,
        items,
    })
}

/// 目录页的行文要素：普通公文版式（只有密级、标题、正文），密级沿用主件，
/// 免得目录把涉密材料的标题印在一张不标密级的纸上。
pub fn toc_snapshot(owner: &DraftInput) -> DraftInput {
    let (level, period) = owner.security_marking();
    let mut profile = TemplateProfile::for_kind(TemplateKind::PlainDocument);
    profile.security_level = level.to_string();
    profile.security_period = period.to_string();
    profile.special_handling = false;
    profile.style_mode = owner.profile.style_mode;
    DraftInput {
        kind: TemplateKind::PlainDocument,
        title_hint: TOC_TITLE.into(),
        date: owner.date.clone(),
        date_is_auto: false,
        profile,
        ..Default::default()
    }
}

/// 目录页正文：序号、名称、文种、版本、页数（不含补的空白页）。
pub fn toc_markdown(plan: &SendPackagePlan, page_counts: &[usize]) -> String {
    let mut out = format!(
        "# {TOC_TITLE}\n\n| 序号 | 名称 | 文种 | 版本 | 页数 |\n| --- | --- | --- | --- | --- |\n"
    );
    for (index, (entry, pages)) in plan.entries.iter().zip(page_counts).enumerate() {
        let version = match entry.revision.as_ref().and_then(|r| r.visible_number) {
            Some(number) => format!("v{number}"),
            None => "定稿".into(),
        };
        out.push_str(&format!(
            "| {} | {} | {} | {version} | {pages} |\n",
            index + 1,
            table_cell(&entry.title),
            entry.kind.label(),
        ));
    }
    let total: usize = page_counts.iter().sum();
    out.push_str(&format!(
        "\n共 {} 件，合计 {total} 页（不含为双面打印补的空白页）。\n",
        plan.entries.len()
    ));
    out
}

/// 表格单元格里不能出现管道符和换行，`^^` 在本应用的表格语法里是纵向合并。
fn table_cell(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let cell = flat.replace('|', "｜").replace("^^", "＾＾");
    if cell.is_empty() {
        "（无标题）".into()
    } else {
        cell
    }
}

/// PDF 的页数。
pub fn page_count(pdf: &[u8]) -> Result<usize> {
    Ok(Document::load_mem(pdf)
        .context("PDF 解析失败")?
        .get_pages()
        .len())
}

/// 合并结果。
pub struct MergedPdf {
    pub bytes: Vec<u8>,
    pub parts: Vec<PartPages>,
    pub total_pages: usize,
}

/// 可从上层页面树节点继承的页面属性（PDF 32000-1 表 30）。
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// 把若干 PDF 按顺序合成一个。页数为奇数的部分（最后一部分除外）后面补一张与其
/// 末页同尺寸的空白页。原文件的书签、命名目标等文档级结构不保留。
pub fn merge_pdfs(parts: &[Vec<u8>]) -> Result<MergedPdf> {
    if parts.is_empty() {
        bail!("没有可合并的 PDF");
    }
    let mut out = Document::with_version("1.7");
    let pages_id = out.new_object_id();
    let mut next_id = out.max_id + 1;
    let mut kids: Vec<Object> = Vec::new();
    let mut summary = Vec::with_capacity(parts.len());
    for (index, bytes) in parts.iter().enumerate() {
        let mut doc = Document::load_mem(bytes).context("PDF 解析失败")?;
        doc.renumber_objects_with(next_id);
        next_id = doc.max_id + 1;
        let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
        if page_ids.is_empty() {
            bail!("第 {} 份 PDF 没有页面", index + 1);
        }
        let mut last_media_box = None;
        for &page_id in &page_ids {
            // 换父节点前把继承来的属性落到页面自己身上，否则尺寸、字体会丢。
            let inherited = inherited_attributes(&doc, page_id)?;
            let page = doc
                .get_dictionary_mut(page_id)
                .context("PDF 页面对象损坏")?;
            for (key, value) in inherited {
                page.set(key, value);
            }
            page.set("Parent", pages_id);
            last_media_box = page.get(b"MediaBox").ok().cloned();
            kids.push(page_id.into());
        }
        out.objects.extend(doc.objects);
        let blanks = usize::from(page_ids.len() % 2 == 1 && index + 1 < parts.len());
        if blanks == 1 {
            let blank_id = (next_id, 0);
            next_id += 1;
            let mut blank = Dictionary::new();
            blank.set("Type", "Page");
            blank.set("Parent", pages_id);
            blank.set(
                "MediaBox",
                last_media_box.unwrap_or_else(|| {
                    // 找不到尺寸时按 A4。
                    vec![0.into(), 0.into(), 595.276.into(), 841.89.into()].into()
                }),
            );
            blank.set("Resources", Dictionary::new());
            out.objects.insert(blank_id, blank.into());
            kids.push(blank_id.into());
        }
        summary.push(PartPages {
            pages: page_ids.len(),
            blanks,
        });
    }
    out.max_id = next_id - 1;
    let total_pages = kids.len();
    let mut pages = Dictionary::new();
    pages.set("Type", "Pages");
    pages.set("Count", total_pages as i64);
    pages.set("Kids", kids);
    out.objects.insert(pages_id, pages.into());
    let mut catalog = Dictionary::new();
    catalog.set("Type", "Catalog");
    catalog.set("Pages", pages_id);
    let catalog_id = out.add_object(catalog);
    out.trailer.set("Root", catalog_id);
    // 各份原来的目录、页面树、书签已无人引用，清掉。
    out.prune_objects();
    out.renumber_objects();
    let mut bytes = Vec::new();
    out.save_to(&mut bytes).context("写出合并后的 PDF 失败")?;
    Ok(MergedPdf {
        bytes,
        parts: summary,
        total_pages,
    })
}

/// 页面自己没有、要从上层页面树节点继承的属性（就近的祖先优先）。
fn inherited_attributes(doc: &Document, page_id: ObjectId) -> Result<Vec<(Vec<u8>, Object)>> {
    let page = doc.get_dictionary(page_id).context("PDF 页面对象损坏")?;
    let mut missing: Vec<&[u8]> = INHERITABLE
        .iter()
        .copied()
        .filter(|key| !page.has(key))
        .collect();
    let mut found = Vec::new();
    let mut parent = page.get(b"Parent").and_then(Object::as_reference).ok();
    // 页面树不会很深；设个上限防止坏文件里 Parent 成环。
    for _ in 0..64 {
        let Some(id) = parent else { break };
        if missing.is_empty() {
            break;
        }
        let node = doc.get_dictionary(id).context("PDF 页面树节点损坏")?;
        missing.retain(|key| match node.get(key) {
            Ok(value) => {
                found.push((key.to_vec(), value.clone()));
                false
            }
            Err(_) => true,
        });
        parent = node.get(b"Parent").and_then(Object::as_reference).ok();
    }
    Ok(found)
}

/// 先写同目录下的临时文件，成功后改名到目标路径。
fn write_atomically(output: &Path, bytes: &[u8]) -> Result<()> {
    let temp = output.with_extension("pdf.tmp");
    let result = std::fs::write(&temp, bytes)
        .with_context(|| format!("无法写入 {}", temp.display()))
        .and_then(|()| {
            std::fs::rename(&temp, output)
                .with_context(|| format!("无法写入导出文件 {}", output.display()))
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::send_package::{PlanEntry, PlannedRevision};
    use lopdf::content::{Content, Operation};
    use lopdf::{Stream, dictionary};

    /// 造一个 `pages` 页的 PDF，每页内容里写上 `{tag}-{页码}` 作记号。
    /// `inherit` 为真时 MediaBox 与 Resources 只放在页面树节点上（XeLaTeX 的常见写法）。
    fn fake_pdf(tag: &str, pages: usize, size: (i64, i64), inherit: bool) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier",
        });
        let resources_id =
            doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
        let media_box: Object = vec![0.into(), 0.into(), size.0.into(), size.1.into()].into();
        let mut kids = Vec::new();
        for n in 1..=pages {
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 12.into()]),
                    Operation::new("Tj", vec![Object::string_literal(format!("{tag}-{n}"))]),
                    Operation::new("ET", vec![]),
                ],
            };
            let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
            let mut page = dictionary! {
                "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
            };
            if !inherit {
                page.set("MediaBox", media_box.clone());
                page.set("Resources", resources_id);
            }
            kids.push(doc.add_object(page).into());
        }
        let mut tree = dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => pages as i64 };
        if inherit {
            tree.set("MediaBox", media_box);
            tree.set("Resources", resources_id);
        }
        doc.objects.insert(pages_id, tree.into());
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    /// 合并后每一页的记号（空白页为 `None`）与生效的 MediaBox 宽高。
    fn page_marks(pdf: &[u8]) -> Vec<(Option<String>, (f32, f32))> {
        let doc = Document::load_mem(pdf).unwrap();
        doc.get_pages()
            .into_values()
            .map(|id| {
                let page = doc.get_dictionary(id).unwrap();
                let media = page.get(b"MediaBox").unwrap().as_array().unwrap();
                let size = (media[2].as_float().unwrap(), media[3].as_float().unwrap());
                assert!(page.has(b"Resources"), "每页都应带上 Resources");
                let mark = page.get(b"Contents").ok().map(|_| {
                    let content = doc.get_and_decode_page_content(id).unwrap();
                    content
                        .operations
                        .iter()
                        .find(|op| op.operator == "Tj")
                        .map(|op| {
                            String::from_utf8_lossy(op.operands[0].as_str().unwrap()).into_owned()
                        })
                        .unwrap()
                });
                (mark, size)
            })
            .collect()
    }

    #[test]
    fn merge_keeps_order_and_pads_odd_parts_to_start_on_odd_pages() {
        let parts = vec![
            fake_pdf("A", 3, (595, 842), false),
            fake_pdf("B", 2, (595, 842), true),
            fake_pdf("C", 1, (842, 595), true),
            fake_pdf("D", 1, (595, 842), false),
        ];
        let merged = merge_pdfs(&parts).unwrap();
        assert_eq!(
            merged.parts,
            vec![
                PartPages {
                    pages: 3,
                    blanks: 1
                },
                PartPages {
                    pages: 2,
                    blanks: 0
                },
                PartPages {
                    pages: 1,
                    blanks: 1
                },
                // 最后一部分不补。
                PartPages {
                    pages: 1,
                    blanks: 0
                },
            ]
        );
        assert_eq!(merged.total_pages, 9);
        let marks = page_marks(&merged.bytes);
        let labels: Vec<Option<&str>> = marks.iter().map(|(m, _)| m.as_deref()).collect();
        assert_eq!(
            labels,
            vec![
                Some("A-1"),
                Some("A-2"),
                Some("A-3"),
                None,
                Some("B-1"),
                Some("B-2"),
                Some("C-1"),
                None,
                Some("D-1"),
            ]
        );
        // 每部分的首页都落在奇数页（1 起算）。
        for first in [1, 5, 7, 9] {
            assert!(labels[first - 1].is_some_and(|m| m.ends_with("-1")));
        }
        // 继承来的 MediaBox 落到了页面上；空白页与前一页同尺寸（横向件后的空白页也是横向）。
        assert_eq!(marks[4].1, (595.0, 842.0));
        assert_eq!(marks[6].1, (842.0, 595.0));
        assert_eq!(marks[7].1, (842.0, 595.0));
        assert_eq!(page_count(&merged.bytes).unwrap(), 9);
    }

    #[test]
    fn merge_rejects_garbage() {
        assert!(merge_pdfs(&[]).is_err());
        assert!(merge_pdfs(&[b"not a pdf".to_vec()]).is_err());
    }

    fn entry(title: &str, kind: TemplateKind, number: Option<i64>) -> PlanEntry {
        PlanEntry {
            manuscript_id: Some(1),
            document_uuid: format!("uuid-{title}"),
            title: title.into(),
            kind,
            revision: number.map(|n| PlannedRevision {
                revision_uuid: format!("rev-{title}"),
                payload_hash: format!("hash-{title}"),
                visible_number: Some(n),
                snapshot: DraftInput {
                    kind,
                    title_hint: title.into(),
                    ..Default::default()
                },
                content_markdown: format!("# {title}"),
            }),
            blocker: number
                .is_none()
                .then_some(crate::manuscript::send_package::PlanBlocker::NeverCommitted),
            has_uncommitted: false,
            has_pending_branch: false,
            pinned: false,
        }
    }

    fn plan(entries: Vec<PlanEntry>) -> SendPackagePlan {
        SendPackagePlan {
            owner_id: 1,
            entries,
        }
    }

    fn temp_output(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("gw_send_package_{}_{tag}.pdf", std::process::id()))
    }

    #[test]
    fn export_compiles_each_version_in_order_and_records_pages() {
        let plan = plan(vec![
            entry("请示", TemplateKind::WhitePaper, Some(2)),
            entry("函稿", TemplateKind::OfficialLetter, Some(1)),
            entry("报告", TemplateKind::ResearchReport, Some(3)),
        ]);
        let output = temp_output("ok");
        let mut compiled = Vec::new();
        let outcome = export_send_package(
            &plan,
            true,
            &output,
            |snapshot, markdown, _stem| {
                compiled.push((snapshot.title_hint.clone(), markdown.to_string()));
                let pages = match snapshot.title_hint.as_str() {
                    "请示" => 3,
                    "函稿" => 2,
                    "报告" => 5,
                    TOC_TITLE => 1,
                    other => panic!("意外的编译对象：{other}"),
                };
                Ok(fake_pdf(&snapshot.title_hint, pages, (595, 842), true))
            },
            |_| {},
        )
        .unwrap();
        // 先逐件编译，目录页最后排（要用到各件页数）。
        let order: Vec<&str> = compiled.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(order, vec!["请示", "函稿", "报告", TOC_TITLE]);
        let toc = &compiled[3].1;
        assert!(
            toc.contains("| 1 | 请示 | 白头件（呈批件） | v2 | 3 |"),
            "{toc}"
        );
        assert!(toc.contains("| 3 | 报告 | 研究报告 | v3 | 5 |"), "{toc}");
        assert!(toc.contains("共 3 件，合计 10 页"), "{toc}");

        assert_eq!(
            outcome.toc,
            Some(PartPages {
                pages: 1,
                blanks: 1
            })
        );
        // 目录 1+1、请示 3+1、函稿 2、报告 5。
        assert_eq!(outcome.total_pages, 13);
        let pages: Vec<(i64, i64)> = outcome
            .items
            .iter()
            .map(|item| (item.page_count, item.blank_pages))
            .collect();
        assert_eq!(pages, vec![(3, 1), (2, 0), (5, 0)]);
        assert_eq!(outcome.items[1].revision_uuid, "rev-函稿");
        assert_eq!(outcome.items[2].sort_order, 2);
        assert_eq!(page_count(&std::fs::read(&output).unwrap()).unwrap(), 13);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn export_fails_whole_without_touching_existing_file() {
        let output = temp_output("fail");
        std::fs::write(&output, b"old").unwrap();

        let blocked = plan(vec![
            entry("请示", TemplateKind::WhitePaper, Some(1)),
            entry("通知", TemplateKind::PlainDocument, None),
        ]);
        let error = export_send_package(&blocked, false, &output, |_, _, _| unreachable!(), |_| {})
            .unwrap_err()
            .to_string();
        assert!(error.contains("《通知》不能进包"), "{error}");

        let failing = plan(vec![
            entry("请示", TemplateKind::WhitePaper, Some(1)),
            entry("函稿", TemplateKind::OfficialLetter, Some(1)),
        ]);
        let error = export_send_package(
            &failing,
            false,
            &output,
            |snapshot, _, _| {
                if snapshot.title_hint == "函稿" {
                    bail!("TeX 报错");
                }
                Ok(fake_pdf("x", 1, (595, 842), false))
            },
            |_| {},
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("《函稿》编译失败"),
            "{error:#}"
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"old");
        assert!(!output.with_extension("pdf.tmp").exists());
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn toc_page_goes_through_plain_document_tex_with_a_real_table() {
        let plan = plan(vec![
            entry("关于申请专项经费的请示", TemplateKind::WhitePaper, Some(2)),
            entry("关于商请支持的函", TemplateKind::OfficialLetter, Some(1)),
        ]);
        let owner = &plan.entries[0].revision.as_ref().unwrap().snapshot;
        let dir = std::env::temp_dir().join(format!("gw_toc_tex_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tex_path = dir.join("toc.tex");
        crate::export::write_tex_for_kind(
            &tex_path,
            &toc_snapshot(owner),
            &toc_markdown(&plan, &[3, 2]),
            &crate::units::UnitDisplay::new(&[]),
            &Default::default(),
            &Default::default(),
            &crate::visual_diff::ElementMarks::default(),
        )
        .unwrap();
        let tex = std::fs::read_to_string(&tex_path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(tex.contains("plaindocument"), "目录页应走普通公文版式");
        assert!(tex.contains(TOC_TITLE));
        // 管道表被解析成了表格，而不是原样的竖线文字。
        assert!(
            tex.contains("tabular") || tex.contains("longtable") || tex.contains("tblr"),
            "{tex}"
        );
        assert!(!tex.contains("| 序号 |"), "{tex}");
        assert!(tex.contains("关于商请支持的函"));
    }

    #[test]
    fn toc_snapshot_carries_owner_security_and_cells_are_escaped() {
        let mut owner = DraftInput {
            kind: TemplateKind::WhitePaper,
            date: "2026年9月30日".into(),
            ..Default::default()
        };
        owner.profile.security_level = "秘密".into();
        owner.profile.security_period = "1年".into();
        let toc = toc_snapshot(&owner);
        assert_eq!(toc.kind, TemplateKind::PlainDocument);
        assert_eq!(toc.security_marking(), ("秘密", "1年"));
        assert_eq!(toc.date, "2026年9月30日");

        assert_eq!(table_cell("甲|乙\n丙^^丁"), "甲｜乙 丙＾＾丁");
        assert_eq!(table_cell("  "), "（无标题）");
    }
}
