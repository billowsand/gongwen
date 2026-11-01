//! 交付检查：哈希、路径、来源与现有公文引用闸门。
//!
//! `reviewed_draft` 的清理 / validator / 版式估算在界面线程另跑；这里补的是它不
//! 负责的部分：提交内容必须与 host 声明的固定版本一致，关键事实必须能在本轮正文
//! 或用户要求里找到，公文引用不能被删改，交付正文里不能留下内部工作区路径。

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

/// 交付前的来源检查。返回硬问题；非空就不装提案。
///
/// 本阶段只把当前正文与用户要求当事实来源；技能模板、范文和模型自己的说明都不是。
/// 不为了“让它过”放行新出现的单位、人员、日期、数字或文件依据。
pub(crate) fn check_delivery(
    before: &str,
    request: &str,
    content: &str,
    vocabulary: &[VocabularyEntry],
) -> Vec<String> {
    let mut issues = Vec::new();
    if content.trim().is_empty() {
        issues.push("提交稿为空".to_string());
    }
    if content.contains("work/")
        || content.contains("input/current.md")
        || content.contains("input/request.md")
        || content.contains("skills/gongwen-markdown")
    {
        issues.push("交付正文里还留着内部工作区路径".to_string());
    }
    let normalize = crate::export::normalize_ordered_list_punctuation;
    if !before.trim().is_empty() && normalize(before).trim() == normalize(content).trim() {
        issues.push("提交稿与当前正文完全相同，没有可接受的修改".to_string());
    }
    if let Err(error) = crate::document_reference::ensure_preserved(before, content) {
        issues.push(format!("公文引用检查未通过：{error}"));
    }

    let text = crate::agent::evidence::strip_citations(content);
    let text = PENDING.replace_all(&text, "");
    let sources = format!("{before}\n{request}");
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
            issues.push(format!(
                "{}「{}」未在本轮正文或用户要求中找到，请补充依据或修订",
                fact.kind.label(),
                fact.value
            ));
        }
    }
    issues.sort();
    issues.dedup();
    issues.truncate(20);
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn new_numbers_dates_and_units_are_rejected() {
        let issues = check_delivery(
            "# 通知\n\n请各单位报送材料。\n",
            "把报送时间改到十月。",
            "# 通知\n\n请各单位于2026年10月8日报送材料，共12项。\n",
            &[],
        );
        assert!(issues.iter().any(|issue| issue.contains("2026年10月8日")));
        assert!(issues.iter().any(|issue| issue.contains("12项")));
    }

    #[test]
    fn facts_already_in_the_request_are_allowed() {
        let issues = check_delivery(
            "# 通知\n\n请各单位报送材料。\n",
            "报送时间改成2026年10月8日，共12项。",
            "# 通知\n\n请各单位于2026年10月8日报送材料，共12项。\n",
            &[],
        );
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn workspace_paths_do_not_leak_into_the_delivery() {
        let issues = check_delivery("", "写一份通知。", "# 通知\n\n见 work/final.md。\n", &[]);
        assert!(issues.iter().any(|issue| issue.contains("工作区路径")));
    }

    #[test]
    fn an_unchanged_document_is_not_a_delivery() {
        let doc = "# 通知\n\n正文。\n";
        let issues = check_delivery(doc, "润色", doc, &[]);
        assert!(issues.iter().any(|issue| issue.contains("完全相同")));
    }

    #[test]
    fn removing_a_document_citation_is_rejected() {
        let before = "# 通知\n\n根据《国务院关于进一步加强安全生产工作的通知》（国发〔2026〕3号），现通知如下。\n";
        let after = "# 通知\n\n现通知如下。\n";
        let issues = check_delivery(before, "精简", after, &[]);
        assert!(issues.iter().any(|issue| issue.contains("公文引用")));
    }
}
