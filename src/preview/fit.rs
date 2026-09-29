//! 主标题、附件标题与居中表格格的排布，与导出同一套判定（`export::title`）：
//! 超出一行较多时按分词在词边界均衡换行，词不被拆到两行。
//!
//! 判定只看纯文字与宽度，预览与导出调同一个函数、用同一份宽度（标题取版心，
//! 表格取导出的智能列宽 twip），因此分几行、断在哪与 Word/PDF 一致。

use super::Metrics;
use super::layout::{
    job, layout, mark_gutter_rows, place, push_galley_tints, row_spans, text_format,
};
use super::marks;
use crate::export::{self, title};
use crate::theme;
use eframe::egui::{self, Align};
use std::collections::HashMap;
use std::sync::Arc;

/// 一段文字排进预览前的整理结果。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fit {
    /// 原样文字（可带花脸稿哨兵、表格格里的行内标记）；换行方案已用 `\n` 接好各行。
    pub(crate) text: String,
}

impl Fit {
    fn from_plan(marked: &str, plan: &title::TitlePlan) -> Self {
        match plan {
            title::TitlePlan::SingleLine | title::TitlePlan::Compressed => Self::plain(marked),
            title::TitlePlan::Wrapped(lines) => Self {
                text: export::redline_slice_lines(marked, lines).join("\n"),
            },
        }
    }

    pub(crate) fn plain(text: &str) -> Self {
        Self {
            text: text.to_string(),
        }
    }
}

/// 公文主标题、附件标题：二号字排满版心宽。`marked` 是带哨兵的纯文本
/// （见 [`marks::plain_keep_marks`]），哨兵不占版面，判定前先滤掉。
pub(crate) fn document_title(ctx: &egui::Context, marked: &str) -> Fit {
    cached(ctx, ("title", marked), || {
        let plain = export::strip_redline(marked);
        Fit::from_plan(marked, &title::title_plan(&plain, title::chars_per_line()))
    })
}

/// 红头呈批件首页标题：小二号，只占批示栏左侧的窄栏。
pub(crate) fn red_approval_title(ctx: &egui::Context, marked: &str) -> Fit {
    cached(ctx, ("red-title", marked), || {
        let plain = export::strip_redline(marked);
        let plan = title::title_plan(&plain, title::red_approval_chars_per_line());
        Fit::from_plan(marked, &plan)
    })
}

/// 居中表格格：判定见 [`export::table::centered_cell_plan`]，与 PDF 的 longtblr 同一条。
/// 不处理的格原样返回。
pub(crate) fn table_cell(
    ctx: &egui::Context,
    cell: &str,
    is_name: bool,
    width_twips: usize,
) -> Fit {
    cached(
        ctx,
        ("cell", cell, is_name, width_twips),
        || match export::table::centered_cell_plan(cell, is_name, width_twips) {
            Some((plan, _)) => Fit::from_plan(cell, &plan),
            None => Fit::plain(cell),
        },
    )
}

/// 缓存条目上限：表格每个居中格一条，超过就整个清掉重来。
const CACHE_LIMIT: usize = 2048;

/// 分词断行要跑 jieba，预览每帧都重画看得见的标题与表格，按内容缓存判定结果。
fn cached(ctx: &egui::Context, key: impl std::hash::Hash, compute: impl FnOnce() -> Fit) -> Fit {
    let key = super::memo::key(key);
    let id = egui::Id::new("gw-preview-fit");
    if let Some(fit) = ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<HashMap<u64, Arc<Fit>>>(id)
            .get(&key)
            .cloned()
    }) {
        return (*fit).clone();
    }
    let fit = compute();
    ctx.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<HashMap<u64, Arc<Fit>>>(id);
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, Arc::new(fit.clone()));
    });
    fit
}

/// 按排布结果把一段带哨兵的文字排成居中 galley：换行方案各行已由 `\n` 分开。
pub(crate) fn centered_galley(
    ui: &egui::Ui,
    metrics: &Metrics,
    fit: &Fit,
    format: egui::TextFormat,
    width: f32,
) -> Arc<egui::Galley> {
    let mut job = job(width);
    job.halign = Align::Center;
    marks::append_marked_text(&mut job, metrics, &fit.text, format);
    layout(ui, job)
}

/// 公文主标题、附件标题：小标宋二号，相对版心居中，排布见 [`document_title`]。
pub(crate) fn title_block(ui: &mut egui::Ui, metrics: &Metrics, marked: &str) {
    let fit = document_title(ui.ctx(), marked);
    let format = text_format(
        metrics.font(theme::FONT_BIAOSONG, super::TITLE_PT),
        metrics.line,
    );
    let galley = centered_galley(ui, metrics, &fit, format, metrics.content);
    let height = galley.size().y;
    let rows = row_spans(&galley);
    // 与 `line_block` 居中分支一样直接画在版心正中，每一行都相对版心宽居中。
    place(ui, metrics, height, |painter, rect| {
        let origin = egui::pos2(rect.left() + metrics.content / 2.0, rect.top());
        push_galley_tints(metrics, &galley, origin);
        marks::paint_galley_marks(painter, metrics, origin, &galley);
        painter.galley(origin, galley, theme::paper::ink());
        mark_gutter_rows(metrics, rect, &rows);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_titles_break_at_the_same_words_as_the_export() {
        let ctx = egui::Context::default();
        let text = "关于进一步加强全市基层治理体系和治理能力现代化建设的实施方案";
        let fit = document_title(&ctx, text);
        let title::TitlePlan::Wrapped(lines) = title::title_plan(text, title::chars_per_line())
        else {
            panic!("应分行");
        };
        assert_eq!(fit.text, lines.join("\n"));
    }

    #[test]
    fn redline_sentinels_stay_on_their_lines_and_do_not_count_as_width() {
        let ctx = egui::Context::default();
        let plain = "关于进一步加强全市基层治理体系和治理能力现代化建设的实施方案";
        // 「关于」之后整段是新增：哨兵跨过断行处，各行拼回去仍是原文。
        let marked = format!("关于{}", export::mark_added(&plain["关于".len()..]));
        let fit = document_title(&ctx, &marked);
        assert_eq!(export::strip_redline(&fit.text).replace('\n', ""), plain);
        assert!(fit.text.contains('\n'));
    }

    #[test]
    fn short_titles_and_cells_are_left_alone() {
        let ctx = egui::Context::default();
        assert_eq!(
            document_title(&ctx, "重点工作通知"),
            Fit::plain("重点工作通知")
        );
        // 姓名格、加粗格不参与。
        let long = "国家发展和改革委员会办公厅综合司";
        assert_eq!(table_cell(&ctx, long, true, 1500), Fit::plain(long));
        let bold = format!("**{long}**");
        assert_eq!(table_cell(&ctx, &bold, false, 1500), Fit::plain(&bold));
        assert_ne!(table_cell(&ctx, long, false, 1500), Fit::plain(long));
    }
}
