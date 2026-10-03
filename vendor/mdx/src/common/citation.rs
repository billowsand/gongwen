use anyhow::{Context, Result};
use biblatex::Bibliography;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::ast::{Block, Inline};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validated {
    pub has_citations: bool,
    /// 文献库里的全部键。叙述式引用 `@key` 只有键在这里面才算引用，否则按原文印。
    pub keys: BTreeSet<String>,
    /// 正文里真正引到文献的叙述式引用键（键在文献库里）。
    pub text_cited: BTreeSet<String>,
    source: Option<PathBuf>,
}

impl Validated {
    /// 叙述式引用 `@key` 是否真的引了文献库里的一条。
    pub fn knows(&self, key: &str) -> bool {
        self.keys.contains(key)
    }
}

/// 校验文献引用。方括号引用 `[@key]` 的键必须都在文献库里，否则报错；叙述式引用
/// `@key` 只认文献库里有的键，其余是正文里碰巧写了个 `@`（账号、记号），不报错，
/// 排版时原样印出（[`Validated::knows`]）。
pub fn validate(blocks: &[Block], declared: Option<&str>, base_dir: &Path) -> Result<Validated> {
    let Collected { cited, text_cited } = collect(blocks);
    let Some(declared) = declared else {
        if cited.is_empty() {
            return Ok(Validated {
                has_citations: false,
                keys: BTreeSet::new(),
                text_cited: BTreeSet::new(),
                source: None,
            });
        }
        anyhow::bail!("Markdown 使用了文献引用，但 front matter 未声明 bibliography");
    };

    let source = base_dir.join(declared);
    if !source.is_file() {
        anyhow::bail!("Bib 文件不存在或不是普通文件: {}", source.display());
    }
    let content = fs::read_to_string(&source)
        .with_context(|| format!("读取 Bib 文件 {} 失败", source.display()))?;
    let bibliography = Bibliography::parse(&content)
        .map_err(|e| anyhow::anyhow!("Bib 文件 {} 格式错误: {}", source.display(), e))?;

    let available: BTreeSet<&str> = bibliography.keys().collect();
    let missing: Vec<&str> = cited
        .iter()
        .map(String::as_str)
        .filter(|key| !available.contains(key))
        .collect();
    if !missing.is_empty() {
        anyhow::bail!("Bib 文件缺少引用键: {}", missing.join(", "));
    }

    let text_cited: BTreeSet<String> = text_cited
        .into_iter()
        .filter(|key| available.contains(key.as_str()))
        .collect();
    Ok(Validated {
        has_citations: !cited.is_empty() || !text_cited.is_empty(),
        keys: available.into_iter().map(str::to_string).collect(),
        text_cited,
        source: Some(source),
    })
}

impl Validated {
    pub fn copy_to(&self, out_dir: &Path) -> Result<Option<PathBuf>> {
        if !self.has_citations {
            return Ok(None);
        }

        let source = self.source.as_ref().expect("validated citation source");
        let destination = out_dir.join("references.bib");
        if destination.exists() && fs::canonicalize(source)? == fs::canonicalize(&destination)? {
            return Ok(Some(destination));
        }

        fs::copy(source, &destination).with_context(|| {
            format!(
                "复制 Bib 文件 {} 到 {} 失败",
                source.display(),
                destination.display()
            )
        })?;
        Ok(Some(destination))
    }
}

/// 正文里的引用键：方括号引用与叙述式引用分开收。
#[derive(Default)]
struct Collected {
    cited: BTreeSet<String>,
    text_cited: BTreeSet<String>,
}

fn collect(blocks: &[Block]) -> Collected {
    let mut keys = Collected::default();
    for block in blocks {
        match block {
            Block::Paragraph(inlines)
            | Block::List {
                content: inlines, ..
            } => collect_inlines(inlines, &mut keys),
            Block::Table { rows, .. } => {
                for cell in rows.iter().flatten() {
                    collect_inlines(&crate::common::inline::parse(cell), &mut keys);
                }
            }
            _ => {}
        }
    }
    keys
}

fn collect_inlines(inlines: &[Inline], keys: &mut Collected) {
    for inline in inlines {
        match inline {
            Inline::Citation(cited) => keys.cited.extend(cited.iter().cloned()),
            Inline::TextCitation(key) => {
                keys.text_cited.insert(key.clone());
            }
            // 加粗 / 斜体内部可再嵌套引用（parser 会递归解析，emitter 也会递归输出
            // \cite），校验必须同样下钻，否则加粗引用的 key 校验被绕过、has_citations
            // 假阴性会漏掉 \bibliography{references}。
            Inline::Bold(children) | Inline::Italic(children) => collect_inlines(children, keys),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ast::Block;
    use crate::parser;
    use std::fs;
    use std::path::Path;

    fn blocks(md: &str) -> Vec<Block> {
        parser::parse(md)
    }

    #[test]
    fn citation_requires_bibliography_declaration() {
        let err = validate(&blocks("见 [@missing]。"), None, Path::new("."))
            .unwrap_err()
            .to_string();
        assert!(err.contains("bibliography"));
    }

    #[test]
    fn valid_bib_passes_and_missing_keys_are_sorted() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("refs.bib"),
            "@article{a,title={A}}\n@article{b,title={B}}\n",
        )
        .unwrap();
        let ok = validate(&blocks("[@a; @b]"), Some("refs.bib"), dir.path()).unwrap();
        assert!(ok.has_citations);

        let err = validate(&blocks("[@z; @c; @z]"), Some("refs.bib"), dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("c, z"), "{err}");
    }

    #[test]
    fn malformed_and_duplicate_bibtex_fail() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bad.bib"), "@article{broken").unwrap();
        assert!(validate(&blocks("正文"), Some("bad.bib"), dir.path()).is_err());

        fs::write(
            dir.path().join("dup.bib"),
            "@article{same,title={A}}\n@book{same,title={B}}\n",
        )
        .unwrap();
        let err = validate(&blocks("正文"), Some("dup.bib"), dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("same"), "{err}");
    }

    #[test]
    fn declared_bib_is_checked_without_citations() {
        let dir = tempfile::tempdir().unwrap();
        let err = validate(&blocks("正文"), Some("absent.bib"), dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("absent.bib"));
    }

    #[test]
    fn copy_is_a_noop_when_source_is_output_references_bib() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("references.bib"), "@article{a,title={A}}\n").unwrap();
        let validated = validate(&blocks("[@a]"), Some("references.bib"), dir.path()).unwrap();

        let copied = validated.copy_to(dir.path()).unwrap().unwrap();

        assert_eq!(copied, dir.path().join("references.bib"));
    }

    #[test]
    fn copy_uses_stable_output_name() {
        let dir = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("library.bib"), "@article{a,title={A}}\n").unwrap();
        let validated = validate(&blocks("[@a]"), Some("library.bib"), dir.path()).unwrap();

        let copied = validated.copy_to(output.path()).unwrap().unwrap();

        assert_eq!(copied, output.path().join("references.bib"));
        assert!(copied.is_file());
    }

    #[test]
    fn collects_citations_nested_in_bold_and_italic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("refs.bib"),
            "@article{bold_ref,title={A}}\n@article{italic_ref,title={B}}\n",
        )
        .unwrap();

        // 加粗 / 斜体内部的引用必须被 has_citations 认出，否则会漏掉参考文献机制。
        let validated = validate(
            &blocks("重要结论 **见 [@bold_ref]**，另见 *[@italic_ref]*。"),
            Some("refs.bib"),
            dir.path(),
        )
        .unwrap();
        assert!(validated.has_citations);

        // 加粗里引用了不存在的 key 时，校验同样要拦下（不能被绕过）。
        let err = validate(&blocks("**[@nope]**"), Some("refs.bib"), dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn text_citations_count_only_when_the_key_is_in_the_library() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("refs.bib"),
            "@article{a,title={A}}
",
        )
        .unwrap();

        let validated = validate(&blocks("见文献@a。"), Some("refs.bib"), dir.path()).unwrap();
        assert!(validated.has_citations);
        assert!(validated.knows("a"));

        // 不在库里的 `@key` 不报错、不算引用：排版时原样印。
        let validated = validate(&blocks("联系 @admin。"), Some("refs.bib"), dir.path()).unwrap();
        assert!(!validated.has_citations);
        assert!(!validated.knows("admin"));

        // 没声明文献库时同理，不要求 front matter。
        let validated = validate(&blocks("联系 @admin。"), None, dir.path()).unwrap();
        assert!(!validated.has_citations);
    }

    #[test]
    fn validates_citation_inside_table_cell() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("refs.bib"),
            "@article{table_ref,title={A}}\n",
        )
        .unwrap();
        let table = "| 文献 |\n|---|\n| [@table_ref] |\n";

        let validated = validate(&blocks(table), Some("refs.bib"), dir.path()).unwrap();

        assert!(validated.has_citations);
    }
}
