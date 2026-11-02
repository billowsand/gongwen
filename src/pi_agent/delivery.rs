//! 交付检查：哈希、路径、选区、来源与现有公文引用闸门。
//!
//! `reviewed_draft` 的清理 / validator / 版式估算在界面线程另跑；这里补的是它不
//! 负责的部分：提交内容必须与 host 声明的固定版本一致；只改选区时选区外必须逐字
//! 不变；关键事实必须能在本轮正文、用户要求、事实材料或**工具实际返回的证据**里
//! 找到；公文引用不能被删改；交付正文里不能留下内部工作区路径。
//!
//! 样稿、结构范文、模型自己的摘要与旧候选稿都不是新的事实来源。
//!
//! 结果分两类（[`DeliveryReport`]）：技术 / 权限保护是**阻止项**，成品不能写进正文；
//! 事实来源这类疑点是**提示项**，只展示给用户核对，不再要求人工确认才能写入
//! （`docs/pi-implementation/pi-direct-write-task.md` 结果契约 2）。分类由字段承载，
//! 界面与状态机都不解析中文原因串来判断类别。

use crate::models::VocabularyEntry;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

use super::protocol::MAX_DELIVERY_BYTES;

static PENDING: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"【待核实[^】]*】").expect("固定待核实标记正则"));

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
/// `advisories` 是仍然值得检查、值得展示的疑点（关键事实在本轮依据里找不到）：
/// 照常写入，只作为提示挂在结果卡与审校抽屉上。
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

/// 交付前的来源检查。阻止项非空就不能写入正文；提示项只展示。
pub(crate) fn check_delivery(ctx: &DeliveryCheck<'_>) -> DeliveryReport {
    let DeliveryCheck {
        before,
        base,
        request,
        content,
        selection,
        fact_sources,
        vocabulary,
    } = ctx;
    let mut issues = Vec::new();
    let mut advisories = Vec::new();
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

    let text = crate::ai_services::citations::strip_citations(content);
    let text = PENDING.replace_all(&text, "");
    let mut sources = format!("{before}\n{request}");
    for source in (*fact_sources).iter() {
        sources.push('\n');
        sources.push_str(source);
    }
    for fact in crate::ai_guard::extract_key_facts(&text, vocabulary) {
        if sources.contains(&fact.value) {
            continue;
        }
        let title = fact.value.trim_start_matches('《').trim_end_matches('》');
        let known_title = fact.kind == crate::ai_guard::FactKind::Document
            && (before
                .lines()
                .any(|line| line.trim().trim_start_matches('#').trim() == title)
                || request.lines().any(|line| line.trim() == title));
        if !known_title {
            // 事实疑点只提示：用户要求「直接写入并保留撤销记录」之后，来源核对不再
            // 是人工确认的前置条件，但仍要摆到人眼前。
            advisories.push(format!(
                "{}「{}」未在本轮正文、用户要求、事实材料或工具证据中找到依据",
                fact.kind.label(),
                fact.value
            ));
        }
    }
    let mut report = DeliveryReport {
        blockers: issues,
        advisories,
    };
    report.blockers.sort();
    report.blockers.dedup();
    report.blockers.truncate(20);
    report.advisories.sort();
    report.advisories.dedup();
    report.advisories.truncate(20);
    report
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

    /// 新数字、日期不再阻止写入，只作事实提示（`pi-direct-write-task.md` 结果契约 2）。
    #[test]
    fn new_numbers_and_dates_are_advised_but_do_not_block_the_write() {
        let report = check(
            "# 通知\n\n请各单位报送材料。\n",
            "把报送时间改到十月。",
            "# 通知\n\n请各单位于2026年10月8日报送材料，共12项。\n",
        );
        assert!(
            report.blockers.is_empty(),
            "事实疑点不能阻止写入：{report:?}"
        );
        assert!(
            report
                .advisories
                .iter()
                .any(|item| item.contains("2026年10月8日")),
            "{report:?}"
        );
        assert!(
            report.advisories.iter().any(|item| item.contains("12项")),
            "{report:?}"
        );
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
    fn evidence_can_authorise_a_fact_but_a_style_sample_cannot() {
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

    #[test]
    fn vocabulary_membership_does_not_authorise_a_new_task_fact() {
        // 词库证明「张三」是规范写法，不证明他承担了本次任务。没有材料 / 文献 / 用户确认时，
        // 正文新增「由张三负责」必须被事实闸门拦住。
        let vocabulary = vec![crate::models::VocabularyEntry {
            category: crate::models::VocabularyCategory::Person,
            canonical: "张三".into(),
            ..Default::default()
        }];
        let before = "# 通知\n\n请各单位报送材料。\n";
        let content = "# 通知\n\n请各单位报送材料，由张三负责汇总。\n";
        let issues = check_delivery(&DeliveryCheck {
            before,
            base: before,
            request: "加一个负责人",
            content,
            selection: None,
            fact_sources: &[],
            vocabulary: &vocabulary,
        });
        assert!(
            issues.advisories.iter().any(|issue| issue.contains("张三")),
            "只查到词库人员不足以授权事项事实：{issues:?}"
        );
        // 用户本轮明确说「由张三负责」就可以：确认后的人仍然要过事实检查。
        let issues = check_delivery(&DeliveryCheck {
            before,
            base: before,
            request: "由张三负责汇总",
            content,
            selection: None,
            fact_sources: &[],
            vocabulary: &vocabulary,
        });
        assert!(clean(&issues), "{issues:?}");
    }

    #[test]
    fn fabricated_citations_are_flagged_as_advisories() {
        let before = "# 通知\n\n请各单位报送材料。\n";
        let content = "# 通知\n\n根据《国务院关于进一步加强安全生产工作的通知》（国发〔2026〕9号），请各单位报送材料。\n";
        let issues = check(before, "补充依据", content);
        assert!(
            issues
                .advisories
                .iter()
                .any(|issue| issue.contains("国发〔2026〕9号")),
            "模型自报的引用不能当依据，但要提示出来：{issues:?}"
        );
    }

    #[test]
    fn old_candidate_additions_are_not_new_sources() {
        let before = "# 通知\n\n原文。\n";
        let base = "# 通知\n\n原文。新增2026年10月8日。\n";
        let content = "# 通知\n\n原文。新增2026年10月8日，并补充12项。\n";
        let issues = check_delivery(&DeliveryCheck {
            before,
            base,
            request: "再短一点",
            content,
            selection: None,
            fact_sources: &[],
            vocabulary: &[],
        });
        assert!(
            issues
                .advisories
                .iter()
                .any(|issue| issue.contains("2026年10月8日")),
            "旧候选里的日期不能自动成为新事实来源：{issues:?}"
        );
    }
}
