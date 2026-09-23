//! AI 技能包：把公文助手的 Markdown 公文子集与各文种的格式要求，做成一份通用的
//! Agent Skill（`SKILL.md` + 参考文档 + 模板 + 自检脚本），供 Claude、Codex、
//! OpenCode、DeerFlow、pi 这类外部模型工具加载，让它们直接写出能粘贴进起草页的
//! `.md`。
//!
//! 源文件在仓库的 `skills/gongwen-markdown/`，这里用 `include_str!` 整套编进
//! 二进制：设置页「上手指引」随时能导出，不依赖安装目录里有没有那份 `.skill`。
//! 安装包另带一份现成的 `.skill`（`scripts/package-portable.ps1` 打的），两者内容
//! 同源。
//!
//! 技能包里说的每条规则都得跟解析器（`export::parse`）和提示词（`prompt`）对得上；
//! 改了语法或文种规则，记得同步 `skills/gongwen-markdown/` 下的文档。

use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 技能名：`SKILL.md` frontmatter 里的 `name`，也是包内顶层文件夹名。
pub(crate) const SKILL_NAME: &str = "gongwen-markdown";

/// 导出 `.skill` 文件时的默认文件名。
pub(crate) const ARCHIVE_FILE_NAME: &str = "gongwen-markdown.skill";

macro_rules! skill_file {
    ($path:literal) => {
        (
            $path,
            include_str!(concat!("../skills/gongwen-markdown/", $path)),
        )
    };
}

/// 技能包的全部文件：（包内相对路径，内容）。新增文件要在这里登记，
/// `every_file_on_disk_is_embedded` 会盯着两边一致。
pub(crate) const FILES: &[(&str, &str)] = &[
    skill_file!("SKILL.md"),
    skill_file!("references/markdown-syntax.md"),
    skill_file!("references/document-types.md"),
    skill_file!("references/research-report.md"),
    skill_file!("references/writing-style.md"),
    skill_file!("templates/official-letter.md"),
    skill_file!("templates/phone-notice.md"),
    skill_file!("templates/plain-document.md"),
    skill_file!("templates/meeting-agenda.md"),
    skill_file!("templates/white-paper.md"),
    skill_file!("templates/red-head-approval.md"),
    skill_file!("templates/research-report.md"),
    skill_file!("scripts/check_gongwen_md.py"),
];

/// 写出 `.skill` 文件。格式就是 zip，顶层一个 `gongwen-markdown/` 文件夹——
/// Claude 的技能上传认这个结构；别的工具解压后得到的也正好是技能目录。
pub(crate) fn write_archive(path: &Path) -> Result<()> {
    let file = fs::File::create(path)
        .with_context(|| format!("无法创建技能包文件：{}", path.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (relative, content) in FILES {
        // 脚本给可执行位，解压到 Linux / macOS 上能直接跑。
        let mode = if relative.ends_with(".py") {
            0o755
        } else {
            0o644
        };
        zip.start_file(
            format!("{SKILL_NAME}/{relative}"),
            options.unix_permissions(mode),
        )?;
        zip.write_all(content.as_bytes())?;
    }
    zip.finish()
        .with_context(|| format!("无法写完技能包文件：{}", path.display()))?;
    Ok(())
}

/// 在 `parent` 下展开成 `gongwen-markdown/` 技能目录，返回该目录。给 Codex、
/// OpenCode、pi 这类直接读技能目录的工具用：选中它们的 skills 目录即可。
///
/// 只覆盖技能包自己的文件，目录里用户另放的东西不动。
pub(crate) fn write_directory(parent: &Path) -> Result<PathBuf> {
    let root = parent.join(SKILL_NAME);
    for (relative, content) in FILES {
        let target = root.join(relative);
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir).with_context(|| format!("无法创建目录：{}", dir.display()))?;
        }
        fs::write(&target, content)
            .with_context(|| format!("无法写入技能文件：{}", target.display()))?;
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::io::Read;

    fn source_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("skills")
            .join(SKILL_NAME)
    }

    fn files_under(dir: &Path, base: &Path, out: &mut BTreeSet<String>) {
        for entry in fs::read_dir(dir).expect("技能目录可读") {
            let path = entry.expect("目录项").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "__pycache__") {
                    continue;
                }
                files_under(&path, base, out);
            } else {
                let relative = path.strip_prefix(base).expect("在技能目录内");
                out.insert(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    #[test]
    fn every_file_on_disk_is_embedded() {
        let dir = source_dir();
        let mut on_disk = BTreeSet::new();
        files_under(&dir, &dir, &mut on_disk);
        let embedded = FILES
            .iter()
            .map(|(path, _)| path.to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            on_disk, embedded,
            "skills/{SKILL_NAME}/ 与 FILES 登记不一致"
        );
    }

    #[test]
    fn skill_md_frontmatter_names_the_skill() {
        let skill = FILES
            .iter()
            .find(|(path, _)| *path == "SKILL.md")
            .map(|(_, content)| *content)
            .expect("有 SKILL.md");
        let front = skill
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(front, _)| front)
            .expect("SKILL.md 以 YAML frontmatter 开头");
        assert!(
            front
                .lines()
                .any(|line| line == format!("name: {SKILL_NAME}"))
        );
        let description = front
            .lines()
            .find_map(|line| line.strip_prefix("description: "))
            .expect("有 description");
        // Agent Skills 规范：description 不超过 1024 字符。
        assert!(description.chars().count() <= 1024);
    }

    #[test]
    fn archive_holds_every_file_under_the_skill_folder() {
        let temp = tempfile::tempdir().expect("临时目录");
        let path = temp.path().join(ARCHIVE_FILE_NAME);
        write_archive(&path).expect("写出技能包");
        let mut archive = zip::ZipArchive::new(fs::File::open(&path).expect("打开")).expect("zip");
        assert_eq!(archive.len(), FILES.len());
        for (relative, content) in FILES {
            let mut entry = archive
                .by_name(&format!("{SKILL_NAME}/{relative}"))
                .expect("包内有该文件");
            let mut text = String::new();
            entry.read_to_string(&mut text).expect("读出");
            assert_eq!(text, *content);
        }
    }

    /// 技能包里的公文模板必须能被解析器按预期读懂：标记、表格、标题都各归其类，
    /// 没有一行 Markdown 语法漏成正文段落印到纸上。解析器的语法一改，这里先红，
    /// 提醒同步 `skills/gongwen-markdown/`。
    #[test]
    fn official_templates_parse_without_leaking_syntax() {
        use crate::export::{MarkdownBlock, attachment_names, parse_markdown};
        for (path, content) in FILES {
            let Some(name) = path.strip_prefix("templates/") else {
                continue;
            };
            if name == "research-report.md" {
                continue;
            }
            let blocks = parse_markdown(content);
            assert!(
                matches!(blocks.first(), Some(MarkdownBlock::Title(_))),
                "{path} 第一块应是文档标题"
            );
            for block in &blocks {
                if let MarkdownBlock::Paragraph(text) = block {
                    for leak in ["<!--", "#", "|"] {
                        assert!(!text.starts_with(leak), "{path} 有语法漏进正文：{text}");
                    }
                }
                if let MarkdownBlock::Heading(_, text) = block {
                    assert!(!text.is_empty(), "{path} 有空标题");
                }
            }
            if name == "official-letter.md" {
                assert_eq!(attachment_names(&blocks).len(), 2, "公函模板带两份附件");
            }
            if name == "plain-document.md" {
                assert!(
                    blocks
                        .iter()
                        .any(|block| matches!(block, MarkdownBlock::Table { numbered: true, .. }))
                );
            }
        }
    }

    #[test]
    fn directory_export_writes_the_skill_folder() {
        let temp = tempfile::tempdir().expect("临时目录");
        let root = write_directory(temp.path()).expect("展开技能目录");
        assert_eq!(root, temp.path().join(SKILL_NAME));
        for (relative, content) in FILES {
            assert_eq!(
                fs::read_to_string(root.join(relative)).expect("文件存在"),
                *content
            );
        }
    }
}
