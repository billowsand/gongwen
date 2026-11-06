//! 对外成品命名：固定目录、两种纸面状态、42 字符、中间省略。
use super::clean_filename as safe_filename;
use crate::models::{DraftInput, LetterVersion, TemplateKind};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) const MAX_EXPORT_FILENAME_CHARS: usize = 42;
pub(super) const EXPORT_FILE_SUFFIXES: [&str; 7] = [
    "-源码包.zip",
    ".docx",
    ".pdf",
    ".份号清单.txt",
    "-Typst包.zip",
    "_签收记录单.docx",
    "_签收记录单.pdf",
];

pub(super) fn shorten(value: &str, budget: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= budget {
        return value.into();
    }
    if budget == 0 {
        return String::new();
    }
    let left = budget / 2;
    let right = budget - left - 1;
    format!(
        "{}…{}",
        chars[..left].iter().collect::<String>(),
        chars[chars.len() - right..].iter().collect::<String>()
    )
}

fn version(input: &DraftInput) -> &'static str {
    match input.kind {
        TemplateKind::PlainDocument | TemplateKind::PhoneRecord | TemplateKind::ResearchReport => {
            ""
        }
        _ => match input.profile.letter_version {
            LetterVersion::Preview => "-签字版",
            LetterVersion::Formal => "-盖章版",
        },
    }
}

fn prefix(input: &DraftInput, numbered: bool) -> String {
    let kind = match input.kind {
        TemplateKind::RedHeadApproval => "红头呈批件",
        TemplateKind::WhitePaper => "白头件",
        _ => input.kind.label(),
    };
    if matches!(
        input.kind,
        TemplateKind::OfficialLetter | TemplateKind::RedHeadApproval
    ) {
        let code = input.profile.department_code.trim();
        let serial = input.profile.document_number.trim();
        let number = if numbered && !serial.is_empty() {
            let year = input.document_year();
            if year.is_empty() {
                format!("{code}{serial}号")
            } else {
                format!("{code}〔{year}〕{serial}号")
            }
        } else {
            code.to_string()
        };
        if !number.is_empty() {
            return format!("{kind}-{}-", safe_filename(&number));
        }
    }
    format!("{kind}-")
}

pub(crate) fn document_stem_prefix(input: &DraftInput, title: &str) -> String {
    let before = prefix(input, false);
    let budget = MAX_EXPORT_FILENAME_CHARS.saturating_sub(before.chars().count());
    // 极长机关代字由文件名校验给出明确错误；目录也始终遵守长度上限。
    shorten(
        &format!("{before}{}", shorten(&safe_filename(title), budget)),
        MAX_EXPORT_FILENAME_CHARS,
    )
}

pub(super) fn export_file_stem(
    input: &DraftInput,
    title: &str,
    _version_name: Option<&str>,
    _unique: &str,
) -> Result<String> {
    let before = prefix(input, input.profile.letter_version == LetterVersion::Formal);
    let tail = version(input);
    let reserved = EXPORT_FILE_SUFFIXES
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap();
    let fixed = before.chars().count() + tail.chars().count() + reserved;
    anyhow::ensure!(
        fixed + 3 <= MAX_EXPORT_FILENAME_CHARS,
        "文类、机关代字或文号过长，无法在 42 字符内保留标题首尾，请缩短机关代字或文号"
    );
    Ok(format!(
        "{before}{}{tail}",
        shorten(&safe_filename(title), MAX_EXPORT_FILENAME_CHARS - fixed)
    ))
}

pub(super) fn transmit_file_name(unit: &str) -> String {
    format!(
        "传发任务单-{}.pdf",
        shorten(&safe_filename(unit), MAX_EXPORT_FILENAME_CHARS - 6 - 4)
    )
}

// 只清理本程序登记过的同一纸面状态的旧名字，保留另一状态和用户自行放入的文件。
// 文件名限制为单个路径组件，不能从清单逃逸到目录之外。
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Outputs {
    stem: String,
    files: Vec<String>,
}

pub(super) fn record_outputs(
    dir: &Path,
    input: &DraftInput,
    stem: &str,
    files: &[PathBuf],
    receipt: bool,
    complete: bool,
) -> Result<()> {
    let path = dir.join(".gongwen-exports.json");
    let mut entries: BTreeMap<String, Outputs> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("导出文件清单损坏，未清理旧成品")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(error.into()),
    };
    let mut names: Vec<String> = files
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .filter(|n| n.starts_with(stem))
        .collect();
    let roster = format!("{stem}.份号清单.txt");
    if dir.join(&roster).is_file() {
        names.push(roster);
    }
    update_slot(
        dir,
        entries.entry(version(input).into()).or_default(),
        stem,
        names,
        false,
        complete,
    )?;
    if receipt {
        let names = files
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .filter(|n| n.starts_with("传发任务单-"))
            .collect();
        update_slot(
            dir,
            entries.entry("传发任务单".into()).or_default(),
            "传发任务单",
            names,
            true,
            complete,
        )?;
    }
    crate::storage::atomic_write(&path, |file| {
        serde_json::to_writer(file, &entries)?;
        Ok(())
    })
}

fn update_slot(
    dir: &Path,
    current: &mut Outputs,
    stem: &str,
    names: Vec<String>,
    replace: bool,
    complete: bool,
) -> Result<()> {
    if complete && (replace || current.stem != stem) {
        for old in &current.files {
            if !names.contains(old)
                && Path::new(old).components().count() == 1
                && !old.starts_with('.')
            {
                let path = dir.join(old);
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(e)
                            .with_context(|| format!("无法清理旧导出文件：{}", path.display()));
                    }
                }
            }
        }
        current.files.clear();
    }
    if complete {
        current.stem = stem.into();
    } else {
        // 部分成功的名字也登记，下一次完整成功后统一收掉残留。
        current.stem.clear();
    }
    for name in names {
        if !current.files.contains(&name) {
            current.files.push(name);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_elision_preserves_unicode_ends_including_titles_longer_than_eighty() {
        assert_eq!(shorten("中文ABC结尾", 5), "中文…结尾");
        assert_eq!(shorten("短题名", 5), "短题名");
        assert_eq!(shorten("长标题", 1), "…");
        let name = transmit_file_name(&format!("某市{}管理局", "专项事务".repeat(30)));
        assert!(name.starts_with("传发任务单-某市"));
        assert!(name.ends_with("管理局.pdf"));
        assert_eq!(name.chars().count(), 42);
    }

    #[test]
    fn partial_output_is_tracked_and_cleaned_after_success() {
        let dir = tempfile::tempdir().unwrap();
        let input = DraftInput::default();
        let a = dir.path().join("原名.docx");
        let b = dir.path().join("中间名.docx");
        for p in [&a, &b] {
            std::fs::write(p, "内容").unwrap();
        }
        record_outputs(
            dir.path(),
            &input,
            "原名",
            std::slice::from_ref(&a),
            false,
            true,
        )
        .unwrap();
        record_outputs(
            dir.path(),
            &input,
            "中间名",
            std::slice::from_ref(&b),
            false,
            false,
        )
        .unwrap();
        assert!(a.exists() && b.exists());
        record_outputs(
            dir.path(),
            &input,
            "原名",
            std::slice::from_ref(&a),
            false,
            true,
        )
        .unwrap();
        assert!(a.exists());
        assert!(!b.exists());
    }
}
