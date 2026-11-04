//! Pi 交付的技术与权限检查：路径、体积、哈希、空稿、基线、选区和公文引用保护。
//! 2026-11-04 产品修订：不执行事实核查，不产生事实疑点清单；旧回执字段继续兼容。

use crate::models::VocabularyEntry;
use sha2::{Digest, Sha256};

use super::protocol::MAX_DELIVERY_BYTES;

/// host 声明的成品路径必须是 `work/` 下的相对路径。绝对路径、`..`、反斜杠都拒绝。
pub(crate) fn safe_work_path(path: &str) -> bool {
    if path.len() > 512
        || path.contains('\0')
        || path.contains('\\')
        || path.starts_with('/')
        || (path.len() > 1 && path.as_bytes()[1] == b':')
    {
        return false;
    }
    let mut parts = path.split('/');
    if parts.next() != Some("work") {
        return false;
    }
    let mut rest = parts.peekable();
    if rest.peek().is_none() {
        return false;
    }
    rest.all(|part| !part.is_empty() && part != "." && part != "..")
}

pub(crate) fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 校验提交的固定版本：路径、UTF-8 文本、字节数、哈希、体积。
pub(crate) fn verify_submission(
    path: &str,
    bytes: usize,
    sha256: &str,
    content: &str,
) -> Result<(), String> {
    if !safe_work_path(path) {
        return Err(format!("提交路径不在 work/ 下或写法不安全：{path}"));
    }
    if content.trim().is_empty() {
        return Err("提交的 Markdown 是空的".into());
    }
    if content.contains('\0') {
        return Err("提交的 Markdown 含 NUL 字节".into());
    }
    let actual = content.len();
    if actual > MAX_DELIVERY_BYTES {
        return Err(format!(
            "提交的 Markdown 超过上限（{} / {} 字节）",
            actual, MAX_DELIVERY_BYTES
        ));
    }
    if actual != bytes {
        return Err(format!(
            "提交字节数与 host 声明不一致（收到 {actual}，声明 {bytes}）"
        ));
    }
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("提交哈希格式不对".into());
    }
    let expected = sha256_hex(content);
    if expected != sha256.to_ascii_lowercase() {
        return Err("提交内容与哈希不一致，已拒绝交付".into());
    }
    Ok(())
}

/// 用户只允许改的正文选区（字节区间，基于本轮正文快照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PiSelection {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl PiSelection {
    /// 选区必须落在本轮快照内，且两端都是 UTF-8 字符边界。
    pub(crate) fn validate(self, before: &str) -> Result<Self, String> {
        if self.start >= self.end || self.end > before.len() {
            return Err("选区范围不在本轮正文里".into());
        }
        if !before.is_char_boundary(self.start) || !before.is_char_boundary(self.end) {
            return Err("选区没有落在 UTF-8 字符边界上".into());
        }
        Ok(self)
    }

    pub(crate) fn text(self, before: &str) -> &str {
        before.get(self.start..self.end).unwrap_or("")
    }
}

/// 一次交付检查的全部输入。`base` 是本轮写作基准（可能是未采纳的候选稿），
/// 只用于判断「有没有新改动」，**不**作为事实来源。
pub(crate) struct DeliveryCheck<'a> {
    pub(crate) before: &'a str,
    pub(crate) base: &'a str,
    pub(crate) request: &'a str,
    pub(crate) content: &'a str,
    pub(crate) selection: Option<PiSelection>,
    /// 用户提供的事实材料与工具实际返回的资料（样稿不在内）。
    pub(crate) fact_sources: &'a [String],
    pub(crate) vocabulary: &'a [VocabularyEntry],
}

/// 一次交付检查的结果。
///
/// `blockers` 是技术 / 权限保护（空稿、工作区路径泄漏、选区越界造成的选区外改动、
/// 公文引用被删改、与本轮基准完全相同）：非空就不写入正文。
/// `advisories` 保留旧调用/回执兼容；新交付始终为空，不产生事实核查工作。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DeliveryReport {
    pub(crate) blockers: Vec<String>,
    pub(crate) advisories: Vec<String>,
}

impl DeliveryReport {
    /// 阻止原因（给人看）：多条用中文分号连接。
    pub(crate) fn blocking_reason(&self) -> Option<String> {
        (!self.blockers.is_empty()).then(|| self.blockers.join("；"))
    }

    /// 合并另一遍检查（如规整之后的稿件再查一次）的结果。
    pub(crate) fn absorb(&mut self, other: DeliveryReport) {
        self.blockers.extend(other.blockers);
        self.advisories.extend(other.advisories);
        self.blockers.sort();
        self.blockers.dedup();
        self.blockers.truncate(20);
        self.advisories.sort();
        self.advisories.dedup();
        self.advisories.truncate(20);
    }
}

/// 交付前技术与权限检查；不核查事实。
pub(crate) fn check_delivery(ctx: &DeliveryCheck<'_>) -> DeliveryReport {
    let DeliveryCheck {
        before,
        base,
        content,
        selection,
        ..
    } = ctx;
    let mut issues = Vec::new();
    // 保留旧调用结构及回执字段以读取历史；新交付不再执行事实核查。
    let _ = (ctx.request, ctx.fact_sources, ctx.vocabulary);
    if content.trim().is_empty() {
        issues.push("提交稿为空".to_string());
    }
    if content.contains("work/")
        || content.contains("input/current.md")
        || content.contains("input/request.md")
        || content.contains("input/candidate.md")
        || content.contains("input/materials")
        || content.contains("skills/gongwen-markdown")
        || content.contains("skills/pi-skills")
    {
        issues.push("交付正文里还留着内部工作区路径".to_string());
    }
    let normalize = crate::export::normalize_ordered_list_punctuation;
    if !base.trim().is_empty() && normalize(base).trim() == normalize(content).trim() {
        issues.push("提交稿与本轮写作基准完全相同，没有可接受的修改".to_string());
    }
    if let Some(selection) = selection {
        let selection = match selection.validate(before) {
            Ok(selection) => selection,
            Err(error) => {
                issues.push(error);
                PiSelection { start: 0, end: 0 }
            }
        };
        let start = selection.start;
        let end = selection.end;
        if end > start {
            let prefix_ok = before
                .get(..start)
                .zip(content.get(..start))
                .is_some_and(|(old, new)| old == new);
            let tail = before.len().saturating_sub(end);
            let suffix_ok = content.len() >= tail
                && before
                    .get(end..)
                    .zip(content.get(content.len() - tail..))
                    .is_some_and(|(old, new)| old == new);
            if !prefix_ok || !suffix_ok {
                issues.push(
                    "提交稿改动了选区以外的正文：本轮只允许改选区内的内容，请恢复后重交"
                        .to_string(),
                );
            }
        }
    }
    if let Err(error) = crate::document_reference::ensure_preserved(before, content) {
        issues.push(format!("公文引用检查未通过：{error}"));
    }

    let mut report = DeliveryReport {
        blockers: issues,
        advisories: Vec::new(),
    };
    report.blockers.sort();
    report.blockers.dedup();
    report.blockers.truncate(20);
    report.advisories.sort();
    report.advisories.dedup();
    report.advisories.truncate(20);
    report
}

/// 规整只许改变选区内的字节：把规整稿在选区外的改动还原成模型提交的原文。
///
/// 锁定选区的任务要求模型选区外逐字保持原样（第一道检查已保证提交稿选区外与
/// 正文一致），但写入前还要过一遍确定性规整（清理版式回声、补标题、统一列表
/// 标点），这些规整全篇生效，会改动选区外的字节，第二道检查因此把忠实复现
/// 原文的稿子误拒。这里按字符做 Myers diff：规整造成的改动凡是不完全落在选区
/// 内的，一律还原成提交稿原文；选区内的规整保留。没有选区时原样返回规整稿。
///
/// diff 有 200ms 上限：超时会把剩余部分当成一大块替换，整块不落在选区内就整体
/// 还原——最坏结果是这轮不做规整，交付检查的安全性不变。
pub(crate) fn restore_outside_selection(
    submitted: &str,
    normalized: &str,
    selection: Option<PiSelection>,
) -> String {
    let Some(selection) = selection else {
        return normalized.to_string();
    };
    let submitted_chars: Vec<char> = submitted.chars().collect();
    let normalized_chars: Vec<char> = normalized.chars().collect();
    let old_offsets = char_byte_offsets(&submitted_chars);
    let new_offsets = char_byte_offsets(&normalized_chars);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    let mut out = String::with_capacity(normalized.len());
    for op in similar::capture_diff_slices_deadline(
        similar::Algorithm::Myers,
        &submitted_chars,
        &normalized_chars,
        Some(deadline),
    ) {
        let old_range = op.old_range();
        let new_range = op.new_range();
        let old_start = old_offsets[old_range.start];
        let old_end = old_offsets[old_range.end];
        if selection.start <= old_start && old_end <= selection.end {
            out.push_str(&normalized[new_offsets[new_range.start]..new_offsets[new_range.end]]);
        } else {
            out.push_str(&submitted[old_start..old_end]);
        }
    }
    out
}

/// 每个字符的字节起点，末尾再多一个全文长度，便于把字符下标换算成字节区间。
fn char_byte_offsets(chars: &[char]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(chars.len() + 1);
    let mut total = 0;
    for ch in chars {
        offsets.push(total);
        total += ch.len_utf8();
    }
    offsets.push(total);
    offsets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(before: &str, request: &str, content: &str) -> DeliveryReport {
        check_delivery(&DeliveryCheck {
            before,
            base: before,
            request,
            content,
            selection: None,
            fact_sources: &[],
            vocabulary: &[],
        })
    }

    /// 没有任何阻止项，也没有事实提示。
    fn clean(report: &DeliveryReport) -> bool {
        report.blockers.is_empty() && report.advisories.is_empty()
    }

    /// 规整只许改动选区内：选区外被规整（如列表项补句号、版式回声行被剔除）
    /// 的字节一律还原成提交稿原文；选区内的规整保留。
    #[test]
    fn normalization_outside_the_selection_is_reverted() {
        let submitted = "# 通知\n\n1. 加强领导\n\n选中这段要改。\n";
        // 规整：列表项补句号（选区外）、选区内文字按规整后的版本写入。
        let normalized = "# 通知\n\n1. 加强领导。\n\n选中这段已改。\n";
        let start = submitted.find("选中这段要改。").unwrap();
        let selection = PiSelection {
            start,
            end: start + "选中这段要改。".len(),
        };
        let restored = restore_outside_selection(submitted, normalized, Some(selection));
        assert_eq!(
            restored, "# 通知\n\n1. 加强领导\n\n选中这段已改。\n",
            "选区外的规整要还原，选区内的保留：{restored}"
        );
    }

    /// 文首补标题落在选区之外：还原，标题维持原状。
    #[test]
    fn a_title_inserted_before_the_selection_is_reverted() {
        let submitted = "正文。\n\n选中这段要改。\n";
        let normalized = "# 【待核实：标题】\n\n正文。\n\n选中这段已改。\n";
        let start = submitted.find("选中这段要改。").unwrap();
        let selection = PiSelection {
            start,
            end: start + "选中这段要改。".len(),
        };
        let restored = restore_outside_selection(submitted, normalized, Some(selection));
        assert_eq!(restored, "正文。\n\n选中这段已改。\n", "{restored}");
    }

    /// 没有选区：规整稿原样返回（整篇都允许规整）。
    #[test]
    fn without_a_selection_the_normalized_text_is_kept() {
        let submitted = "# 通知\n\n1. 加强领导\n";
        let normalized = "# 通知\n\n1. 加强领导。\n";
        assert_eq!(
            restore_outside_selection(submitted, normalized, None),
            normalized
        );
    }

    /// 中文等多字节字符的 diff 边界必须落在字符上，不能切在半截 UTF-8 里。
    #[test]
    fn multibyte_text_splices_on_char_boundaries() {
        let submitted = "根据《通知》（某办函〔2026〕1号），选中这段要改。\n";
        // 规整：选区内改写，文尾另起一行（选区外）——后者要还原。
        let normalized = "根据《通知》（某办函〔2026〕1号），选中这段已改。\n\n文末规整新增。\n";
        let start = submitted.find("选中这段要改。").unwrap();
        let selection = PiSelection {
            start,
            end: start + "选中这段要改。".len(),
        };
        let restored = restore_outside_selection(submitted, normalized, Some(selection));
        assert_eq!(
            restored, "根据《通知》（某办函〔2026〕1号），选中这段已改。\n",
            "{restored}"
        );
    }

    #[test]
    fn hash_and_size_must_match() {
        let content = "# 标题\n\n正文。\n";
        let hash = sha256_hex(content);
        assert!(verify_submission("work/final.md", content.len(), &hash, content).is_ok());
        assert!(verify_submission("work/final.md", content.len() + 1, &hash, content).is_err());
        assert!(
            verify_submission("work/final.md", content.len(), &"0".repeat(64), content).is_err()
        );
    }

    #[test]
    fn delivery_path_must_be_a_relative_work_file() {
        for path in ["work/final.md", "work/sections/一.md", "work/a/b.md"] {
            assert!(safe_work_path(path), "{path}");
        }
        for path in [
            "input/current.md",
            "final.md",
            "../work/final.md",
            "work/../input/current.md",
            "work\\final.md",
            "/work/final.md",
            "C:/work/final.md",
            "work/",
        ] {
            assert!(!safe_work_path(path), "{path}");
        }
    }

    /// 新数字、日期不再产生事实核查提示。
    #[test]
    fn new_dates_and_numbers_do_not_generate_fact_review_work() {
        let report = check(
            "# 通知\n\n原文。",
            "修改",
            "# 通知\n\n请于2026年10月8日报送12项材料。",
        );
        assert!(clean(&report), "不再生成事实核查提示：{report:?}");
    }

    #[test]
    fn facts_already_in_the_request_are_allowed() {
        let report = check(
            "# 通知\n\n请各单位报送材料。\n",
            "报送时间改成2026年10月8日，共12项。",
            "# 通知\n\n请各单位于2026年10月8日报送材料，共12项。\n",
        );
        assert!(clean(&report), "{report:?}");
    }

    #[test]
    fn delivery_does_not_classify_facts_by_source() {
        let before = "# 通知\n\n请各单位报送材料。\n";
        let content = "# 通知\n\n请各单位于2026年10月8日报送材料。\n";
        let evidence = vec!["市应急管理局定于2026年10月8日检查。".to_string()];
        let issues = check_delivery(&DeliveryCheck {
            before,
            base: before,
            request: "补充依据",
            content,
            selection: None,
            fact_sources: &evidence,
            vocabulary: &[],
        });
        assert!(clean(&issues), "{issues:?}");
        // 样稿不是事实材料：同样的日期放进样稿角色就不会出现在 fact_sources 里。
        let style_only = vec!["去年定于2026年10月8日检查。".to_string()];
        let issues = check_delivery(&DeliveryCheck {
            before,
            base: before,
            request: "仿照样稿",
            content,
            selection: None,
            fact_sources: &style_only,
            vocabulary: &[],
        });
        assert!(
            clean(&issues),
            "样稿文本一旦被误列进 fact_sources 就会放行；角色过滤在 materials 层完成"
        );
    }

    #[test]
    fn workspace_paths_do_not_leak_into_the_delivery() {
        let issues = check("", "写一份通知。", "# 通知\n\n见 work/final.md。\n");
        assert!(
            issues
                .blockers
                .iter()
                .any(|issue| issue.contains("工作区路径"))
        );
    }

    #[test]
    fn an_unchanged_document_is_not_a_delivery() {
        let doc = "# 通知\n\n正文。\n";
        let issues = check(doc, "润色", doc);
        assert!(
            issues
                .blockers
                .iter()
                .any(|issue| issue.contains("完全相同"))
        );
    }

    #[test]
    fn removing_a_document_citation_is_rejected() {
        let before = "# 通知\n\n根据《国务院关于进一步加强安全生产工作的通知》（国发〔2026〕3号），现通知如下。\n";
        let after = "# 通知\n\n现通知如下。\n";
        let issues = check(before, "精简", after);
        assert!(
            issues
                .blockers
                .iter()
                .any(|issue| issue.contains("公文引用"))
        );
    }

    #[test]
    fn selection_must_leave_the_outside_untouched() {
        let before = "# 通知\n\n第一段原样。\n\n第二段要改。\n\n第三段原样。\n";
        let start = before.find("第二段").unwrap();
        let end = start + "第二段要改。".len();
        let selection = PiSelection { start, end };
        let inside = before.replace("第二段要改。", "第二段已改好。");
        let issues = check_delivery(&DeliveryCheck {
            before,
            base: before,
            request: "只改第二段",
            content: &inside,
            selection: Some(selection),
            fact_sources: &[],
            vocabulary: &[],
        });
        assert!(clean(&issues), "{issues:?}");
        let outside = before.replace("第三段原样。", "第三段被顺手改了。");
        let issues = check_delivery(&DeliveryCheck {
            before,
            base: before,
            request: "只改第二段",
            content: &outside,
            selection: Some(selection),
            fact_sources: &[],
            vocabulary: &[],
        });
        assert!(
            issues
                .blockers
                .iter()
                .any(|issue| issue.contains("选区以外")),
            "{issues:?}"
        );
        assert!(PiSelection { start: 1, end: 2 }.validate("中文").is_err());
        assert!(PiSelection { start: 0, end: 99 }.validate("中文").is_err());
    }
}
