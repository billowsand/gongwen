//! `draft_page` 的单元测试：表单校验、导出链接、Markdown 文本操作、表格与切换文种。
//!
//! 由 `src/draft_page.rs` 拆分而来，作为 `draft_page` 的子模块编译（仅测试构建）。

use super::*;
use crate::export::ColumnAlign;
use crate::validator;

/// 表单的必填标记必须和审校的必填校验说同一套话。填报区显示"齐了"、
/// 生成完审校又报"缺少主送单位"，比不标还糟。
#[test]
fn form_check_agrees_with_validator_on_an_empty_draft() {
    let rules = crate::models::SecurityRules::default();
    for kind in TemplateKind::ALL {
        let mut draft = DraftInput {
            kind,
            ..Default::default()
        };
        draft.profile.security_level.clear();
        let check = check_form(&draft);
        let missing: usize = FormSection::ALL
            .iter()
            .map(|section| check.progress[section.index()].missing())
            .sum();
        assert!(
            check.errors.contains_key("security_level"),
            "{kind:?}：密级是所有文种的必填项"
        );
        // 审校报的“缺少某某单位/领导”都属于要素，表单必须已经标出来。
        // 正文类提示（结语、一级标题、议程固定字段）不在要素区的职责内。
        let element_warnings = validator::validate(&draft, "正文", &[], &rules)
            .into_iter()
            .filter(|warning| {
                warning.contains("缺少")
                    && !warning.contains("结语")
                    && !warning.contains("一级标题")
                    && !warning.contains("固定字段")
            })
            .collect::<Vec<_>>();
        for warning in element_warnings {
            assert!(
                missing > 0,
                "{kind:?}：审校报“{warning}”，表单却认为必填项已齐"
            );
        }
    }
}

/// 必填项填全之后，三段的进度必须都到头——否则徽章会永远停在“缺 N 项”。
#[test]
fn form_check_clears_once_required_letter_elements_are_filled() {
    let mut draft = DraftInput {
        kind: TemplateKind::OfficialLetter,
        ..Default::default()
    };
    draft.profile.security_level = "机密".into();
    draft.profile.issuing_unit = "甲单位".into();
    draft.profile.recipient = "乙单位".into();
    let check = check_form(&draft);
    for section in FormSection::ALL {
        assert_eq!(
            check.progress[section.index()].missing(),
            0,
            "{}段仍报缺项：{:?}",
            section.label(),
            check.errors
        );
    }
}

/// 三段折叠头、必填标签、内联报错、文号复合行和缩略导航图都是纯绘制代码，
/// 编译期查不出 id 冲突、越界或 0 宽度这类问题，拿真实布局跑一遍每个文种。
#[test]
fn form_chrome_draws_without_panicking() {
    let ctx = egui::Context::default();
    for kind in TemplateKind::ALL {
        let mut draft = DraftInput {
            kind,
            ..Default::default()
        };
        draft.profile.security_level.clear();
        let check = check_form(&draft);
        let mut manual = BTreeSet::new();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(
                layout_thumbnail(ui, kind, &check).is_none(),
                "没有点击时缩略图不应产生跳转"
            );
            for section in FormSection::ALL {
                let mut open = true;
                section_header_ui(
                    ui,
                    section,
                    kind,
                    &mut open,
                    check.progress[section.index()],
                );
                assert!(open, "没有点击时分段的展开状态不应改变");
            }
            egui::Grid::new("smoke_grid").num_columns(2).show(ui, |ui| {
                required_row_label(ui, "密级", Some("说明"));
                ui.label("控件");
                ui.end_row();
                field_error(ui, &check, "security_level");
                document_number_row(
                    ui,
                    &mut draft.profile,
                    &[],
                    &mut manual,
                    true,
                    240.0,
                    "例如：X政函",
                    "smoke_code",
                );
            });
        });
    }
}

/// 导入内容要独占段落：插入点两侧各自补到有空行为止，文首文末不补。
#[test]
fn imported_block_gets_blank_lines_on_both_sides() {
    let splice = |text: &str, pos: usize, block: &str| {
        let lead = blank_line_padding(&text[..pos], true);
        let tail = blank_line_padding(&text[pos..], false);
        let mut out = text.to_string();
        out.insert_str(pos, &format!("{lead}{block}{tail}"));
        out
    };
    // 空稿：两侧都不补。
    assert_eq!(splice("", 0, "# 标题"), "# 标题");
    // 插在一行中间：前后各补一个空行，把那一行切开（偏移按字节，汉字占 3 字节）。
    assert_eq!(splice("前后", 3, "表格"), "前\n\n表格\n\n后");
    // 已有一个换行的一侧只补差的那个。
    assert_eq!(splice("上\n", 4, "块"), "上\n\n块");
    // 两侧已经是空行就一个都不补。
    assert_eq!(splice("上\n\n\n\n下", 5, "块"), "上\n\n块\n\n下");
}

/// 前缀匹配必须用 `-` 作分界：`关于X` 不应误匹配 `关于X的补充`，
/// 同时要认得版本名称、“待提交”和重复导出产生的编号变体，以及旧时间戳目录。
#[test]
fn export_links_stem_matching_uses_dash_boundary() {
    let mut links = ExportLinks {
        stem: Some("普通公文-关于X".into()),
        ..ExportLinks::default()
    };
    let matches = |name: &str| {
        ExportLinks {
            stem: Some("普通公文-关于X".into()),
            ..ExportLinks::default()
        }
        .matches(std::ffi::OsStr::new(name))
    };
    assert!(matches("普通公文-关于X-202601011200"));
    assert!(matches("普通公文-关于X-送审稿"));
    assert!(matches("普通公文-关于X-待提交-2"));
    assert!(matches("普通公文-关于X-2"));
    assert!(matches("普通公文-关于X"));
    // 摊在根目录的成品（不带文件夹）也认。
    assert!(matches("普通公文-关于X.pdf"));
    // 标题互为前缀时不能误匹配。
    assert!(!matches("普通公文-关于X的补充-202601011300"));
    assert!(!matches("普通公文-关于X的补充.pdf"));
    // 完全无关的文稿。
    assert!(!matches("电话通知-202601011300"));
    assert!(!matches("其他文件.txt"));
    // 未绑定文稿时全部认（退化为旧行为）。
    links.stem = None;
    assert!(links.matches(std::ffi::OsStr::new("电话通知-202601011300")));
}

/// 输出目录里混着当前文稿的多次导出与别的文稿的最新导出时，
/// 成品入口必须只认当前文稿最近导出文件夹里的文件，不能串到别的文稿。
#[test]
fn export_links_only_open_latest_dir_of_current_stem() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    let stem = "XX〔2025〕12号-关于测试的通知";

    // 按时间顺序创建：当前文稿旧导出 → 当前文稿新导出 → 别的文稿最新导出。
    let a_old = dir.join(format!("{stem}-202601011000"));
    let a_new = dir.join(format!("{stem}-202601011100"));
    let b_latest = dir.join("YY〔2025〕3号-关于别的事-202601011200");
    for folder in [&a_old, &a_new, &b_latest] {
        std::fs::create_dir_all(folder).unwrap();
    }
    // 当前文稿旧导出只产 pdf；新导出两种格式齐全。
    touch_export(&a_old, "pdf");
    touch_export(&a_new, "pdf");
    touch_export(&a_new, "docx");
    // 别的文稿最新导出也有 pdf——旧实现会在这里取到它的 pdf/docx。
    touch_export(&b_latest, "pdf");
    touch_export(&b_latest, "docx");
    // 拉开目录修改时间，避免同一时间单位内排序不稳。
    std::thread::sleep(Duration::from_millis(20));

    let mut links = ExportLinks::default();
    links.refresh(dir.to_str().unwrap(), Some(stem));
    for kind in ExportKind::ALL {
        let path = links.path(kind).expect("当前文稿最新导出应点亮该格式按钮");
        assert_eq!(
            path.parent().map(Path::to_path_buf),
            Some(a_new.clone()),
            "{kind:?} 应指向当前文稿最新导出目录"
        );
    }
}

/// 覆盖旧版本的成品时，即使目录自身时间不变，“最近导出”也必须跟上。
#[test]
fn export_links_recognizes_overwritten_version_directory() {
    let root = tempfile::tempdir().unwrap();
    let stem = "普通公文-通知";
    let overwritten = root.path().join(format!("{stem}-送审稿"));
    let other = root.path().join(format!("{stem}-定稿"));
    let epoch = std::time::UNIX_EPOCH;
    for folder in [&overwritten, &other] {
        std::fs::create_dir(folder).unwrap();
        for extension in ["pdf", "docx"] {
            touch_export(folder, extension);
        }
    }
    // 两个目录实际的创建时间都在 epoch 之后，用远未来的成品时间模拟覆盖。
    let path = overwritten.join(format!("{stem}-送审稿.pdf"));
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new().set_modified(epoch + Duration::from_secs(4_000_000_000)),
        )
        .unwrap();
    let mut links = ExportLinks::default();
    links.refresh(root.path().to_str().unwrap(), Some(stem));
    assert_eq!(links.path(ExportKind::Pdf), Some(path.as_path()));
}

/// 当前文稿最新目录缺某种格式时，可以从同文稿更早的导出补齐，
/// 但不能越过它去拿别的文稿的成品。
#[test]
fn export_links_fills_missing_kind_from_older_dir_of_same_stem() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    let stem = "普通公文-工作安排";

    let a_old = dir.join(format!("{stem}-202601011000"));
    let a_new = dir.join(format!("{stem}-202601011100"));
    let b_latest = dir.join("普通公文-别的工作-202601011200");
    for folder in [&a_old, &a_new, &b_latest] {
        std::fs::create_dir_all(folder).unwrap();
    }
    touch_export(&a_old, "pdf");
    touch_export(&a_new, "docx");
    // 别的文稿更晚的导出也有 pdf 与 docx。
    touch_export(&b_latest, "pdf");
    touch_export(&b_latest, "docx");
    std::thread::sleep(Duration::from_millis(20));

    let mut links = ExportLinks::default();
    links.refresh(dir.to_str().unwrap(), Some(stem));
    assert_eq!(
        links
            .path(ExportKind::Pdf)
            .map(|p| p.parent().unwrap().to_path_buf()),
        Some(a_old.clone()),
        "pdf 应从同文稿更早导出补齐，不能取自别的文稿"
    );
    assert_eq!(
        links
            .path(ExportKind::Word)
            .map(|p| p.parent().unwrap().to_path_buf()),
        Some(a_new.clone())
    );
}

/// 按导出的落盘命名：成品文件名主干与所在文件夹同名。
fn touch_export(folder: &std::path::Path, extension: &str) {
    let stem = folder.file_name().unwrap().to_string_lossy();
    std::fs::write(folder.join(format!("{stem}.{extension}")), b"x").unwrap();
}

#[test]
fn markdown_find_returns_utf8_safe_ranges() {
    let text = "标题：重点工作\n正文：重点工作";
    let matches = markdown_matches(text, "重点", true);
    assert_eq!(matches.len(), 2);
    assert_eq!(&text[matches[0].clone()], "重点");
    assert_eq!(&text[matches[1].clone()], "重点");
    assert!(
        matches.iter().all(|range| {
            text.is_char_boundary(range.start) && text.is_char_boundary(range.end)
        })
    );
}

#[test]
fn markdown_find_can_ignore_ascii_case() {
    let text = "Markdown markdown MARKDOWN";
    assert_eq!(markdown_matches(text, "markdown", false).len(), 3);
    assert_eq!(markdown_matches(text, "markdown", true).len(), 1);
}

#[test]
fn regex_find_reports_invalid_patterns_and_expands_named_captures() {
    let text = "日期：2026-08-08";
    let matches = markdown_matches_mode(
        text,
        r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})",
        true,
        true,
    )
    .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(
        expanded_replacement(
            text,
            r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})",
            "$year年$month月$day日",
            true,
            true,
            &matches[0],
        ),
        "2026年08月08日"
    );
    assert!(markdown_matches_mode(text, "(", true, true).is_err());
}

#[test]
fn reverse_replacement_keeps_all_match_ranges_valid() {
    let mut text = "甲处、甲处、甲处".to_string();
    let matches = markdown_matches(&text, "甲处", true);
    for range in matches.into_iter().rev() {
        text.replace_range(range, "乙单位");
    }
    assert_eq!(text, "乙单位、乙单位、乙单位");
}

/// 表格解析：光标所在的行列要认得出来，分隔行上的光标算表头。
#[test]
fn table_at_locates_cursor_row_and_column() {
    let text = "前言

| 序号 | 名称 |
| --- | :---: |
| 1 | 甲 |
| 2 | 乙 |

后记";
    let at = |needle: &str| text.find(needle).expect("片段存在");

    let table = table_at(text, at("甲")).expect("光标在表格里");
    assert_eq!(table.rows.len(), 3, "表头 + 两行数据，分隔行不算");
    assert_eq!(table.row, 1);
    assert_eq!(table.column, 1);
    assert_eq!(table.aligns, vec![ColumnAlign::Auto, ColumnAlign::Center]);

    // 分隔行上的光标归到表头，列仍按竖线数。
    let table = table_at(text, at(":---:")).expect("分隔行也在表格里");
    assert_eq!(table.row, 0);
    assert_eq!(table.column, 1);

    assert!(table_at(text, at("前言")).is_none());
    assert!(table_at(text, at("后记")).is_none());
}

/// 缺了分隔行的几行竖线不是表格，不能当表格改。
#[test]
fn table_at_rejects_pipes_without_a_separator_row() {
    let text = "| 甲 | 乙 |
| 丙 | 丁 |";
    assert!(table_at(text, 3).is_none());
}

/// 序号表格的骨架：标记行 + 表头 + 一条分组行 + 两条数据行，插完就是序号表。
#[test]
fn numbered_table_skeleton_parses_back_as_a_numbered_table() {
    let skeleton = numbered_table_skeleton(3);
    assert_eq!(skeleton.lines().next(), Some(NUMBERED_TABLE_MARKER));
    let blocks = crate::export::parse_markdown(&skeleton);
    let crate::export::MarkdownBlock::Table {
        rows,
        spans,
        numbered,
        ..
    } = &blocks[1]
    else {
        panic!("骨架应当解析为序号表：{blocks:?}");
    };
    assert!(numbered);
    assert_eq!(rows.len(), 4, "表头 + 一条分组行 + 两条数据行");
    assert_eq!(rows[0][0], "序号");
    assert_eq!(rows[1][0], "（一）分组标题");
    assert_eq!(rows[2][0], "1");
    assert_eq!(rows[3][0], "2");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].column_span, 3);
}

/// 光标换到别处，序号表格的判定跟着变；普通表格一直是 false。
#[test]
fn numbered_table_at_follows_the_marker_line() {
    let numbered = "<!-- [序号表] -->
| 序号 | 标题 |
| --- | --- |
| 标题1 | 甲 |";
    let inside = numbered.find("标题1").unwrap();
    assert_eq!(numbered_table_at(numbered, inside), Some(true));
    let above = numbered.find("<!--").unwrap();
    assert_eq!(numbered_table_at(numbered, above), None, "光标在表格外");

    let plain = "| 序号 | 标题 |
| --- | --- |
| 标题1 | 甲 |";
    assert_eq!(
        numbered_table_at(plain, plain.find("标题1").unwrap()),
        Some(false)
    );

    // 中间隔了别的非空行，标记不再算数。
    let separated = "<!-- [序号表] -->
正文一句。

| 序号 | 标题 |
| --- | --- |
| 标题1 | 甲 |";
    let inside = separated.find("标题1").unwrap();
    assert_eq!(numbered_table_at(separated, inside), Some(false));
}

/// 增删行列之后整表重排：列宽按最宽的一格算，中文按两格宽。
#[test]
fn render_table_pads_by_display_width() {
    let rows = vec![
        vec!["序号".to_string(), "名称".to_string()],
        vec!["1".to_string(), "甲单位".to_string()],
    ];
    let rendered = render_table(&rows, &[ColumnAlign::Auto, ColumnAlign::Center], &[]);
    assert_eq!(
        rendered,
        "| 序号 | 名称   |
| ---- | :----: |
| 1    | 甲单位 |"
    );
    // 重新解析一遍，行列与对齐都不变。
    let table = table_at(&rendered, 3).expect("是一张表格");
    assert_eq!(table.rows, rows);
    assert_eq!(table.aligns, vec![ColumnAlign::Auto, ColumnAlign::Center]);
}

#[test]
fn merged_table_round_trips_multimarkdown_markers() {
    let source = "| 类别 | 项目 | 说明 |
| --- | --- | --- |
| 横向合并 || 备注 |
| 纵向合并 | 事项一 | 甲 |
| ^^ | 事项二 | 乙 |";
    let table = table_at(source, source.find("横向合并").unwrap()).unwrap();
    assert_eq!(table.spans.len(), 2);
    let rendered = render_table(&table.rows, &table.aligns, &table.spans);
    assert!(rendered.contains("横向合并"), "{rendered}");
    assert!(rendered.contains("||"), "{rendered}");
    assert!(rendered.contains("^^"), "{rendered}");
    let reparsed = table_at(&rendered, rendered.find("横向合并").unwrap()).unwrap();
    assert_eq!(reparsed.rows, table.rows);
    assert_eq!(reparsed.spans, table.spans);
}

/// 窄列写上对齐冒号之后，分隔行仍要能被认成分隔行：GFM 要求去掉冒号后
/// 还有三条短横，`:-:` 那样写出来的表会整张退化成普通段落。
#[test]
fn narrow_columns_still_render_a_valid_separator_row() {
    for align in [
        ColumnAlign::Auto,
        ColumnAlign::Left,
        ColumnAlign::Center,
        ColumnAlign::Right,
    ] {
        let rows = vec![
            vec!["甲".to_string(), "乙".to_string()],
            vec!["1".to_string(), "2".to_string()],
        ];
        let rendered = render_table(&rows, &[align, ColumnAlign::Auto], &[]);
        let separator = rendered.lines().nth(1).expect("有分隔行");
        assert!(
            is_table_separator_line(separator),
            "{align:?} 的分隔行不合法：{separator}"
        );
        // 导出器与光标定位也要认得它。
        let table = table_at(&rendered, 0).expect("是一张表格");
        assert_eq!(table.aligns, vec![align, ColumnAlign::Auto]);
        assert!(matches!(
            export::parse_markdown(&rendered).first(),
            Some(export::MarkdownBlock::Table { .. })
        ));
    }
}

/// 一列的「表格」不成表：GFM 要求分隔行至少切出两格，所以插入时兜底到两列。
#[test]
fn blank_table_never_has_a_single_column() {
    let table = blank_table(2, 1);
    assert_eq!(table_column_count(table.lines().next().expect("有表头")), 2);
    assert!(matches!(
        export::parse_markdown(&table).first(),
        Some(export::MarkdownBlock::Table { .. })
    ));
}

/// 网格选择器插出来的空表：首行是表头，分隔行认得出来。
#[test]
fn blank_table_has_a_header_and_separator() {
    let table = blank_table(3, 2);
    let lines = table.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 4, "3 行 + 1 条分隔行");
    assert!(is_table_separator_line(lines[1]), "{}", lines[1]);
    assert_eq!(table_column_count(lines[0]), 2);
    // 导出器也要认得它是一张表。
    assert!(matches!(
        export::parse_markdown(&table).first(),
        Some(export::MarkdownBlock::Table { rows, .. }) if rows.len() == 3
    ));
}

/// 标题层级：反复点不同层级不会把 `#` 越堆越多，降为正文能清干净。
#[test]
fn set_heading_replaces_existing_level() {
    assert_eq!(set_heading("正文一段", 2), "## 正文一段");
    assert_eq!(set_heading("## 一级标题", 3), "### 一级标题");
    assert_eq!(set_heading("### 二级标题", 0), "二级标题");
    assert_eq!(set_heading("  ## 带缩进", 2), "## 带缩进");
}

/// 项目符号是开关：第二次点回到普通段落。
#[test]
fn toggle_bullet_switches_both_ways() {
    assert_eq!(toggle_bullet("一条"), "- 一条");
    assert_eq!(toggle_bullet("- 一条"), "一条");
    assert_eq!(toggle_bullet("* 一条"), "一条");
    assert_eq!(toggle_bullet(""), "", "空行不加符号");
}

/// 选区覆盖到的每一行都要改到，选区落在行中间也按整行算。
#[test]
fn map_lines_covers_every_touched_line() {
    let text = "第一行
第二行
第三行";
    let start = text.find("一行").expect("命中");
    let end = text.find("二行").expect("命中");
    let (updated, span) = map_lines(text, &(start..end), |line| set_heading(line, 2));
    assert_eq!(
        updated,
        "## 第一行
## 第二行
第三行"
    );
    assert_eq!(
        &updated[span],
        "## 第一行
## 第二行"
    );
}

/// 加粗是开关：选中已加粗的文字（连标记一起选，或只选中间）都能取消。
#[test]
fn toggle_bold_wraps_and_unwraps() {
    let text = "这是重点内容";
    let range = text.find("重点").expect("命中")..text.find("内容").expect("命中");
    let (bolded, selection) = toggle_bold(text, &range);
    assert_eq!(bolded, "这是**重点**内容");
    assert_eq!(&bolded[selection.clone()], "重点");

    // 只选中间：靠两侧的标记识别出已加粗。
    let (plain, restored) = toggle_bold(&bolded, &selection);
    assert_eq!(plain, "这是重点内容");
    assert_eq!(&plain[restored], "重点");

    // 连标记一起选中同样能取消。
    let whole = bolded.find("**").expect("命中")..bolded.rfind("**").expect("命中") + 2;
    assert_eq!(toggle_bold(&bolded, &whole).0, "这是重点内容");
}

/// 清理空行：连续空行压成一行，行尾空格去掉，文末只留一个换行。
#[test]
fn tidy_blank_lines_collapses_runs() {
    assert_eq!(
        tidy_blank_lines(
            "标题   



正文


"
        ),
        "标题

正文
"
    );
    assert_eq!(tidy_blank_lines(""), "");
}

/// 字数按导出后的正文算：标记、竖线、图片引用都不计。
#[test]
fn body_stats_counts_visible_text_only() {
    let markdown = "# 测试函

<!-- [正文] -->

## 一、要求

**重点**内容。

| 甲 | 乙 |
| --- | --- |
| 1 | 2 |

![图](images/a.png)

又一段。";
    let (characters, paragraphs) = body_stats(markdown);
    // 测试函(3) + 要求(2，标题里手写的“一、”由导出器统一编号，不计)
    // + 重点内容。(5) + 表格四格(4) + 又一段。(4)
    assert_eq!(characters, 18);
    assert_eq!(paragraphs, 2, "只有两个真正的段落");
}

/// 中文日期是公文成文日期的写法：年份逐位、月日用中文数字。
#[test]
fn chinese_today_reads_like_a_document_date() {
    let today = chinese_today();
    assert!(export::chinese_date_parts(&today).is_some(), "{today}");
    assert!(!today.chars().any(|ch| ch.is_ascii_digit()), "{today}");
}

/// 收集一帧内所有文本及其中心点，用于定位可点击控件。
fn text_centers(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Pos2)> {
    fn collect(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                let s = text.galley.text().to_string();
                let rect = text.galley.rect.translate(text.pos.to_vec2());
                out.push((s, rect.center()));
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in shapes {
        collect(&clipped.shape, &mut out);
    }
    out
}

/// 回归测试：在文种下拉框里点选“普通公文”不应崩溃。
///
/// 曾因时序 bug 崩溃：`form_header_ui` 里的 `versioned` 在文种下拉框渲染之前
/// 求值，点选“普通公文”的那一帧 kind 已变、versioned 还是旧值 true，残留渲染
/// 的版本下拉框走到 `match` 的 `PlainDocument => unreachable!()`，整个程序退出。
/// 测试用真实点击事件走完 展开面板 → 打开文种下拉 → 点选“普通公文” 的路径。
#[test]
fn switching_kind_combo_to_plain_document_does_not_panic() {
    let ctx = egui::Context::default();
    crate::theme::configure_icons(&ctx);
    crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
    ctx.set_pixels_per_point(2.0);
    let mut config = AppConfig::default();
    let markdown = "# 测试标题\n\n正文内容。".to_string();
    let mut doc = DraftSession::with_markdown(1, &config, markdown);
    assert_eq!(doc.draft.kind, TemplateKind::OfficialLetter);
    let (sender, _keep) = std::sync::mpsc::channel();
    let mut status = String::new();
    let mut version_switch = None;
    let mut revert_confirm = None;
    let mut actions = Vec::new();
    let mut export_links = ExportLinks::default();
    let mut metrics = crate::metrics::Metrics::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));

    let mut frame = |ctx: &egui::Context, events: Vec<egui::Event>| {
        let raw = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let output = ctx.clone().run_ui(raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let mut page = DraftPage {
                    doc: &mut doc,
                    config: &mut config,
                    store: None,
                    sender: &sender,
                    status: &mut status,
                    version_switch: &mut version_switch,
                    revert_confirm: &mut revert_confirm,
                    actions: &mut actions,
                    export_links: &mut export_links,
                    metrics: &mut metrics,
                };
                page.create_ui(ui);
            });
        });
        output.shapes
    };
    // egui 的点击要按下、抬起各一帧才算数。
    let click = |frame: &mut dyn FnMut(
        &egui::Context,
        Vec<egui::Event>,
    ) -> Vec<egui::epaint::ClippedShape>,
                 ctx: &egui::Context,
                 pos: egui::Pos2| {
        let press = vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let release = vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }];
        let _ = frame(ctx, press);
        frame(ctx, release)
    };

    // 文档要素区默认收起，先点 ribbon 上的“文档要素”按钮展开它。
    let shapes = frame(&ctx, vec![]);
    let expand = text_centers(&shapes)
        .into_iter()
        .find(|(s, _)| s == "文档要素")
        .expect("应能找到 ribbon 上的“文档要素”按钮");
    let shapes = click(&mut frame, &ctx, expand.1);
    // 打开文种下拉框：先点当前文种“公函”，再等一帧让选项渲染出来。
    let combo = text_centers(&shapes)
        .into_iter()
        .find(|(s, p)| s == "公函" && p.x < 500.0)
        .expect("应能找到文种下拉框上的“公函”");
    let _ = click(&mut frame, &ctx, combo.1);
    let shapes = frame(&ctx, vec![]);
    // 点选“普通公文”。切换那一帧内 kind 即变，若时序 bug 复活会在这里 panic。
    let plain = text_centers(&shapes)
        .into_iter()
        .find(|(s, _)| s.contains("普通公文"))
        .expect("下拉里应有“普通公文”");
    let _ = click(&mut frame, &ctx, plain.1);
    assert_eq!(doc.draft.kind, TemplateKind::PlainDocument);
}
