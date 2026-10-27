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
    skill_file!("templates/phone-record.md"),
    skill_file!("templates/plain-document.md"),
    skill_file!("templates/meeting-agenda.md"),
    skill_file!("templates/meeting-minutes.md"),
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
            // 会议纪要的纪要名称在表单里印成红头，正文标题可选，模板不写。
            if name == "meeting-minutes.md" {
                assert!(
                    !blocks
                        .iter()
                        .any(|block| matches!(block, MarkdownBlock::Title(_))),
                    "{path} 不写 `# 标题`"
                );
            } else {
                assert!(
                    matches!(blocks.first(), Some(MarkdownBlock::Title(_))),
                    "{path} 第一块应是文档标题"
                );
            }
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

    /// 技能包的**模型实测**验收：技能文档是给外部模型看的，模板测试只证明
    /// 「我们自己照骨架写」能过解析器，证明不了「模型照文档写」能过。
    ///
    /// 这里读 `scripts/gongwen-skill-eval.py` 产出的 `manifest.json`，把每份
    /// 清洗后的模型产物同时交给解析器与校验器：
    /// - 结构：第一块是文档标题、正文只有一个 `#`、标题不手写编号、语法不漏成正文、
    ///   附件标记后有标题且文种允许带附件；
    /// - 校验器：与同一文种的技能模板逐条对比 `validator::validate` 的告警，
    ///   只留下模型稿**独有**的那些（要素表单本身的告警两边都有，相减即可）。
    ///
    /// 设 `GONGWEN_SKILL_EVAL_MANIFEST` 指向 manifest.json 时运行；没设就跳过，
    /// 免得默认 `cargo test` 依赖本地评测产物。
    #[test]
    #[ignore = "需要模型产物：设置 GONGWEN_SKILL_EVAL_MANIFEST 指向 manifest.json 后运行"]
    fn model_outputs_survive_the_parser_and_validator() {
        let Ok(manifest_path) = std::env::var("GONGWEN_SKILL_EVAL_MANIFEST") else {
            eprintln!("未设置 GONGWEN_SKILL_EVAL_MANIFEST，跳过");
            return;
        };
        let manifest_path = PathBuf::from(manifest_path);
        assert!(
            manifest_path.is_file(),
            "manifest 不存在：{}",
            manifest_path.display()
        );
        let root = manifest_path
            .parent()
            .expect("manifest 有父目录")
            .to_path_buf();
        // 评测脚本可能是 PowerShell / Python 写的，入口带 BOM 也认。
        let manifest_text = fs::read_to_string(&manifest_path).expect("读 manifest");
        let manifest: serde_json::Value =
            serde_json::from_str(manifest_text.trim_start_matches('\u{feff}'))
                .expect("manifest 是 JSON");
        let cases = manifest["cases"].as_array().expect("manifest.cases 是数组");
        assert!(!cases.is_empty(), "manifest 里没有用例");

        let mut failures: Vec<String> = Vec::new();
        let mut checked = 0usize;
        for case in cases {
            let file = case["file"].as_str().expect("case.file");
            let kind_name = case["kind"].as_str().expect("case.kind");
            let label = format!(
                "{}｜{}｜{}｜{}",
                case["condition"].as_str().unwrap_or("?"),
                case["provider"].as_str().unwrap_or("?"),
                case["model"].as_str().unwrap_or("?"),
                kind_name
            );
            let text = match fs::read_to_string(root.join(file)) {
                Ok(text) => text,
                Err(error) => {
                    failures.push(format!("{label}：读不出 {file}（{error}）"));
                    continue;
                }
            };
            checked += 1;
            let mut problems = syntax_problems_in_model_output(kind_name, &text);
            problems.extend(validator_problems_for_model_output(kind_name, &text));
            if !problems.is_empty() {
                failures.push(format!("{label}（{file}）：{}", problems.join("；")));
            }
        }
        eprintln!(
            "模型实测：共 {} 份产物，{} 份过不了解析器或校验器",
            checked,
            failures.len()
        );
        assert!(
            failures.is_empty(),
            "模型产物不合格（技能文档没让模型写对）：\n{}",
            failures.join("\n")
        );
    }

    /// 模型产物独有、技能模板没有的校验器告警。
    ///
    /// 同一文种的技能模板当基准，只留模型稿独有的告警——要素表单本身的告警
    /// （缺主送、缺密级……）两边都有，相减就消掉了。`【待核实：……】` 是技能文档
    /// 要求的占位写法，不算模型写错，单独放过。
    fn validator_problems_for_model_output(kind_name: &str, text: &str) -> Vec<String> {
        use crate::models::SecurityRules;
        use crate::validator::validate;

        let input = eval_draft(kind_name);
        let rules = SecurityRules::default();
        let baseline: BTreeSet<String> = validate(&input, skill_template(kind_name), &[], &rules)
            .into_iter()
            .collect();
        validate(&input, text, &[], &rules)
            .into_iter()
            .filter(|warning| !baseline.contains(warning))
            .filter(|warning| !warning.contains("正文含“待核实”字段"))
            .map(|warning| format!("校验器：{warning}"))
            .collect()
    }

    /// 一份模型产物在解析器层面的问题（不看文采，只看会被程序读错的地方）。
    fn syntax_problems_in_model_output(kind_name: &str, text: &str) -> Vec<String> {
        use crate::export::{MarkdownBlock, MarkdownSection, parse_markdown};

        let research = kind_name == "research-report";
        // 研究报告的附录标题是 `## 附录标题`（见 research-report.md），不是 `#`；
        // 公文文种的附件才用 `# 附件正式标题`。
        let hash_title_required = !research;
        // 这几个文种只能有标题和正文，不该出现附件区段（见 prompt::kind_rules）。
        let attachments_allowed =
            !matches!(kind_name, "white-paper" | "meeting-agenda" | "phone-record");
        let mut problems = Vec::new();
        let blocks = parse_markdown(text);
        // 会议纪要的正文标题可选（纪要名称在表单里印成红头）。
        let title_optional = kind_name == "meeting-minutes";
        if !title_optional && !matches!(blocks.first(), Some(MarkdownBlock::Title(_))) {
            problems.push("第一块不是文档标题（第一行必须是一个 `# 标题`）".into());
        }
        let mut section = MarkdownSection::Body;
        let mut body_titles = 0usize;
        let mut attachment_markers = 0usize;
        let mut attachment_needs_title = false;
        for block in &blocks {
            match block {
                MarkdownBlock::Marker(next) => {
                    if hash_title_required
                        && section == MarkdownSection::Attachment
                        && attachment_needs_title
                    {
                        problems.push("有一份附件没写 `# 附件标题`".into());
                    }
                    section = *next;
                    if *next == MarkdownSection::Attachment {
                        attachment_markers += 1;
                        attachment_needs_title = true;
                    }
                }
                MarkdownBlock::Title(_) => {
                    if section == MarkdownSection::Body {
                        body_titles += 1;
                    } else {
                        attachment_needs_title = false;
                    }
                }
                MarkdownBlock::Html(raw) => {
                    // 认不出来的注释标记不印到纸上，不算漏语法；真的 HTML 标签算。
                    if !raw.trim_start().starts_with("<!--") {
                        problems.push(format!("HTML 标签会印到纸上：{}", head(raw)));
                    }
                }
                MarkdownBlock::Paragraph(raw) => {
                    for leak in ["<!--", "```", "|", "#"] {
                        if raw.starts_with(leak) {
                            problems.push(format!("语法漏进正文：{}", head(raw)));
                            break;
                        }
                    }
                }
                MarkdownBlock::Heading(level, raw) => {
                    if *level > 5 {
                        problems.push(format!("标题层级超过五级（{level} 个 #）：{}", head(raw)));
                    }
                    if raw.trim().is_empty() {
                        problems.push("有空标题".into());
                    }
                }
                _ => {}
            }
        }
        if hash_title_required && attachment_needs_title {
            problems.push("最后一份附件没写 `# 附件标题`".into());
        }
        if !attachments_allowed && attachment_markers > 0 {
            problems.push(format!(
                "{}不该有附件区段，却写了 {attachment_markers} 个附件标记",
                kind_name
            ));
        }
        // 研究报告正文的 `#` 是报告题名（「部分」区的 `#` 也归到这里），所以只要求
        // 至少一个；其余文种正文区只能有一个 `#`。
        if research {
            if body_titles == 0 {
                problems.push("研究报告正文区没有 `# 报告题名`".into());
            }
        } else if title_optional {
            if body_titles > 1 {
                problems.push(format!(
                    "正文区的 `#` 标题有 {body_titles} 个，会议纪要至多 1 个"
                ));
            }
        } else if body_titles != 1 {
            problems.push(format!(
                "正文区的 `#` 标题有 {body_titles} 个，应当恰好 1 个"
            ));
        }
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
            // 手写编号只对 `##`~`#####` 剥：解析器只对 level > 1 的标题调
            // `clean_heading_number`（parse.rs:720-726），`#` 文档标题与附件标题不剥，
            // `######` 正文里本来就不排。
            if !(2..=5).contains(&hashes)
                || !trimmed
                    .as_bytes()
                    .get(hashes)
                    .is_some_and(u8::is_ascii_whitespace)
            {
                continue;
            }
            let heading = trimmed[hashes..].trim();
            if crate::export::clean_heading_number(heading) != heading {
                problems.push(format!(
                    "第{}行标题手写了编号「{}」，程序会剥掉重编",
                    index + 1,
                    head(heading)
                ));
            }
        }

        // 表格以分隔行的列数为准整形（`parse_table_cells`）：某一行多出来的单元格会被
        // 悄悄丢掉，正文里就少了字。解析器自己只认「含 `|` 且至少两格」的表头行与
        // 「每格至少三个短横」的分隔行，这里照它的判据扫一遍源码。
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        let mut index = 0usize;
        while index < lines.len() {
            let row_is_table = lines[index].contains('|') && table_cells(lines[index]).len() >= 2;
            let next_is_separator = lines
                .get(index + 1)
                .is_some_and(|line| is_table_separator_line(line));
            if row_is_table && next_is_separator {
                let width = table_cells(lines[index + 1]).len();
                let mut row = index + 2;
                while row < lines.len()
                    && lines[row].contains('|')
                    && table_cells(lines[row]).len() >= 2
                {
                    let cells = table_cells(lines[row]).len();
                    if cells > width {
                        problems.push(format!(
                            "第{}行表格有 {cells} 格，多于分隔行的 {width} 列，多出来的格会被丢掉",
                            row + 1
                        ));
                    }
                    row += 1;
                }
                index = row.max(index + 1);
                continue;
            }
            index += 1;
        }
        problems
    }

    /// 与 `export::parse::table_source_cells` 同一口径：剥掉首尾各一根竖线后按 `|` 切。
    fn table_cells(line: &str) -> Vec<&str> {
        let mut value = line.trim();
        if let Some(rest) = value.strip_prefix('|') {
            value = rest;
        }
        if let Some(rest) = value.strip_suffix('|') {
            value = rest;
        }
        value.split('|').collect()
    }

    /// 与 `export::parse::is_table_separator` 同一口径：至少两格，每格去掉冒号后
    /// 至少三个短横。
    fn is_table_separator_line(line: &str) -> bool {
        let cells = table_cells(line);
        cells.len() >= 2
            && cells.iter().all(|cell| {
                let value = cell.trim().trim_matches(':');
                value.chars().count() >= 3 && value.chars().all(|ch| ch == '-')
            })
    }

    /// 评测用的一份稿件：文种与最低限度的锁定要素。要素本身的告警在模板与模型稿
    /// 两边都会出现，对比时相减，所以这里不求填全，只求两边取同一个值。
    fn eval_draft(kind_name: &str) -> crate::models::DraftInput {
        use crate::models::{DraftInput, TemplateKind, TemplateProfile};

        let kind = match kind_name {
            "official-letter" => TemplateKind::OfficialLetter,
            "phone-notice" => TemplateKind::PhoneNotice,
            "phone-record" => TemplateKind::PhoneRecord,
            "plain-document" => TemplateKind::PlainDocument,
            "meeting-agenda" => TemplateKind::MeetingAgenda,
            "meeting-minutes" => TemplateKind::MeetingMinutes,
            "white-paper" => TemplateKind::WhitePaper,
            "red-head-approval" => TemplateKind::RedHeadApproval,
            "research-report" => TemplateKind::ResearchReport,
            other => panic!("manifest 里有不认识的文种：{other}"),
        };
        DraftInput {
            kind,
            date: "2026年10月18日".into(),
            profile: TemplateProfile {
                kind,
                issuing_unit: "星海省教育厅".into(),
                department_code: "星教函".into(),
                document_number: "12".into(),
                security_level: "秘密".into(),
                security_period: "10年".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// 该文种的技能模板正文，校验器对比的基准。
    fn skill_template(kind_name: &str) -> &'static str {
        let wanted = format!("templates/{kind_name}.md");
        FILES
            .iter()
            .find(|(path, _)| *path == wanted.as_str())
            .map(|(_, content)| *content)
            .unwrap_or_else(|| panic!("技能包里没有 {wanted}"))
    }

    /// 报错信息里只放开头一小段，够定位就行。
    fn head(text: &str) -> String {
        let trimmed = text.trim();
        let mut out: String = trimmed.chars().take(40).collect();
        if trimmed.chars().count() > 40 {
            out.push('…');
        }
        out
    }

    /// 验收判据自己也要有测试：故意写坏的稿子必须被挑出来，写对的稿子不能被冤枉。
    #[test]
    fn model_output_checks_catch_the_usual_mistakes() {
        let good = "# 关于××××的函\n\n<!-- [正文] -->\n\n正文一段。\n\n## 总体要求\n\n内容。\n\n特此函告。\n";
        assert_eq!(
            syntax_problems_in_model_output("official-letter", good),
            Vec::<String>::new()
        );

        let manual_number = "# 关于××××的函\n\n<!-- [正文] -->\n\n## 一、总体要求\n\n内容。\n";
        assert!(
            syntax_problems_in_model_output("official-letter", manual_number)
                .iter()
                .any(|problem| problem.contains("手写了编号")),
            "手写编号没被挑出来"
        );

        let untitled_attachment =
            "# 关于××××的函\n\n<!-- [正文] -->\n\n正文。\n\n<!-- [附件] -->\n\n没有标题的内容。\n";
        assert!(
            syntax_problems_in_model_output("official-letter", untitled_attachment)
                .iter()
                .any(|problem| problem.contains("附件没写")),
            "没有标题的附件没被挑出来"
        );

        // 白头件只写标题和正文（见 prompt::kind_rules），带附件区段要报出来。
        let white_with_attachment =
            "# 关于××××的请示\n\n正文。\n\n<!-- [附件] -->\n\n# 附件标题\n\n内容。\n";
        assert!(
            syntax_problems_in_model_output("white-paper", white_with_attachment)
                .iter()
                .any(|problem| problem.contains("不该有附件区段")),
            "白头件的附件区段没被挑出来"
        );

        // 三份文档里出现两个 `#`：正文区只许一个。
        let two_titles = "# 标题\n\n正文。\n\n# 又一个标题\n\n内容。\n";
        assert!(
            syntax_problems_in_model_output("official-letter", two_titles)
                .iter()
                .any(|problem| problem.contains("恰好 1 个")),
            "第二个文档标题没被挑出来"
        );

        // 会议纪要不写 `# 标题` 是常态，不算错；写两个才算。
        let minutes = "<!-- [正文] -->\n\n会议认为，工作有成效。一是摸清底数。\n";
        assert_eq!(
            syntax_problems_in_model_output("meeting-minutes", minutes),
            Vec::<String>::new()
        );
        let minutes_two_titles = format!("# 甲纪要\n\n# 乙纪要\n\n{minutes}");
        assert!(
            syntax_problems_in_model_output("meeting-minutes", &minutes_two_titles)
                .iter()
                .any(|problem| problem.contains("至多 1 个"))
        );

        // 研究报告的附录标题是 `## 附录标题`，不是 `#`，不能按公文附件去要求 `#`。
        let research_appendix = "# 报告题名\n\n<!-- [正文] -->\n\n## 章标题\n\n<!-- [附录] -->\n\n## 附录一的标题\n\n内容。\n";
        assert_eq!(
            syntax_problems_in_model_output("research-report", research_appendix),
            Vec::<String>::new()
        );

        // `#` 文档标题与附件标题不剥手写编号（parse.rs:720-726 只剥 level > 1），
        // 所以「# 2026 年度……」不算错；`### 2026 年第三季度…` 才算。
        let year_title =
            "# 2026 年度全市政务数据资源共享开放工作要点\n\n<!-- [正文] -->\n\n内容。\n";
        assert_eq!(
            syntax_problems_in_model_output("plain-document", year_title),
            Vec::<String>::new()
        );
        let year_heading =
            "# ××工作要点\n\n<!-- [正文] -->\n\n### 2026 年第三季度共享数据统计\n\n内容。\n";
        assert!(
            syntax_problems_in_model_output("plain-document", year_heading)
                .iter()
                .any(|problem| problem.contains("手写了编号")),
            "「数字 + 空格」开头的标题没被挑出来"
        );

        // 校验器对比：技能文档要求的 `【待核实：……】` 占位不算写错；
        // 技能模板里没有的问题（空附件标记）要按校验器的话报出来。
        let placeholder = "# 关于××××的函\n\n<!-- [正文] -->\n\n请于【待核实：反馈时间】前反馈。\n";
        assert_eq!(
            validator_problems_for_model_output("official-letter", placeholder),
            Vec::<String>::new()
        );
        let empty_attachment = "# 关于××××的函\n\n<!-- [正文] -->\n\n正文。\n\n<!-- [附件] -->\n";
        assert!(
            validator_problems_for_model_output("official-letter", empty_attachment)
                .iter()
                .any(|problem| problem.contains("附件")),
            "空附件标记没被校验器挑出来"
        );

        // 表格多出来的格会被解析器悄悄丢掉：四列表里混进五格的行要报出来。
        let wide_row = "# ××工作要点\n\n<!-- [正文] -->\n\n| 序号 | 事项 | 单位 | 时限 |\n| --- | --- | --- | --- |\n| || 基础支撑组 |||\n";
        assert!(
            syntax_problems_in_model_output("plain-document", wide_row)
                .iter()
                .any(|problem| problem.contains("多于分隔行")),
            "多出一格的表格行没被挑出来"
        );
        let correct_row = "# ××工作要点\n\n<!-- [正文] -->\n\n| 序号 | 事项 | 单位 | 时限 |\n| --- | --- | --- | --- |\n| 基础支撑组 |||\n";
        assert_eq!(
            syntax_problems_in_model_output("plain-document", correct_row),
            Vec::<String>::new()
        );
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
