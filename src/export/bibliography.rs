//! 研究报告的文献库：把随稿件保存的 `.bib` 解析成起草页要用的样子。
//!
//! 起草页的「文献引用」下拉、源码里 `[@key]` 的悬停卡、预览的参考文献表和审校
//! 都从这里取：每条文献的作者、年份、题名、类型，以及它在参考文献表里的整条
//! 著录。著录用 hayagriva 按 `gb-7714-2015-numeric` 排，与 Typst 排 PDF 用的是
//! 同一个库、同一份样式、同一个语言（`zh-CN`），起草页里看到的就是纸上印的那行。
//!
//! 解析与排版只取决于 `.bib` 内容，按内容哈希缓存最近一份（[`library`]）：
//! 编辑器每帧都要问一声哪些键在库里，不能每帧重排一遍文献表。

use super::crossref;
use biblatex::{Bibliography, ChunksExt, EntryType, ParseErrorKind, Person, Token};
use hayagriva::archive::ArchivedStyle;
use hayagriva::citationberg::{LocaleCode, Style};
use hayagriva::{BibliographyDriver, BibliographyRequest, CitationItem, CitationRequest};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

/// 文献库里的一条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BibEntry {
    pub(crate) key: String,
    /// 文献类型的中文叫法（期刊、专著、学位论文……），对应国标的文献类型标识。
    pub(crate) kind: &'static str,
    /// 第一责任者，多人时加“等”（西文加 et al.）；没有作者时退到编者、机构。
    pub(crate) author: String,
    pub(crate) year: String,
    pub(crate) title: String,
    /// 参考文献表里的整条著录，不带序号；`.bib` 有错排不出来时为空。
    pub(crate) formatted: String,
    /// 搜索用：键、全部责任者、题名、年份，转小写拼在一起。
    search: String,
}

impl BibEntry {
    /// 搜索框里的字是否落在这条文献上。`query` 已转小写、去掉首尾空白。
    pub(crate) fn matches(&self, query: &str) -> bool {
        query.is_empty() || self.search.contains(query)
    }
}

/// `.bib` 里第一处让 PDF 排不出来的错：行号从 1 起。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BibProblem {
    pub(crate) line: usize,
    pub(crate) message: String,
}

/// 一份解析好的文献库。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Library {
    /// 按 `.bib` 里的先后。解析失败时只剩正则认出来的键，其余字段为空——
    /// 下拉照样能列出来，只是看不到作者题名。
    pub(crate) entries: Vec<BibEntry>,
    /// 有错就导不出 PDF（hayagriva 整份拒收），起草页要提前说出来。
    pub(crate) problem: Option<BibProblem>,
}

impl Library {
    pub(crate) fn get(&self, key: &str) -> Option<&BibEntry> {
        self.entries.iter().find(|entry| entry.key == key)
    }

    pub(crate) fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
}

/// 取 `bib` 对应的文献库：内容没变就复用上一次的结果。
pub(crate) fn library(bib: &str) -> Arc<Library> {
    static CACHE: Mutex<Option<(u64, Arc<Library>)>> = Mutex::new(None);
    let mut hasher = std::hash::DefaultHasher::new();
    bib.hash(&mut hasher);
    let key = hasher.finish();
    if let Ok(cache) = CACHE.lock()
        && let Some((cached, library)) = cache.as_ref()
        && *cached == key
    {
        return Arc::clone(library);
    }
    let library = Arc::new(parse(bib));
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((key, Arc::clone(&library)));
    }
    library
}

/// 解析并排好全部条目。走 [`library`] 拿带缓存的版本。
pub(crate) fn parse(bib: &str) -> Library {
    if bib.trim().is_empty() {
        return Library::default();
    }
    let bibliography = match Bibliography::parse(bib) {
        Ok(bibliography) => bibliography,
        Err(error) => {
            let entries = crossref::bibtex_keys(bib)
                .into_iter()
                .map(|key| BibEntry {
                    search: key.to_lowercase(),
                    key,
                    kind: "",
                    author: String::new(),
                    year: String::new(),
                    title: String::new(),
                    formatted: String::new(),
                })
                .collect();
            return Library {
                entries,
                problem: Some(BibProblem {
                    line: line_of(bib, error.span.start),
                    message: parse_message(&error.kind),
                }),
            };
        }
    };
    let (formatted, problem) = match format_all(&bibliography) {
        Ok(formatted) => (formatted, None),
        Err(problem) => (HashMap::new(), Some(problem.at(bib))),
    };
    let entries = bibliography
        .iter()
        .map(|entry| {
            let people = people(entry);
            let author = first_author(entry, &people);
            let year = year(entry);
            let title = field(entry, "title");
            let search = format!(
                "{} {} {} {} {}",
                entry.key,
                people.iter().map(full_name).collect::<Vec<_>>().join(" "),
                author,
                title,
                year
            )
            .to_lowercase();
            BibEntry {
                key: entry.key.clone(),
                kind: kind_label(&entry.entry_type),
                author,
                year,
                title,
                formatted: formatted.get(&entry.key).cloned().unwrap_or_default(),
                search,
            }
        })
        .collect();
    Library { entries, problem }
}

/// hayagriva 转换出错时只给字节位置，行号要回到原文里数。
struct Pending {
    offset: usize,
    message: String,
}

impl Pending {
    fn at(self, bib: &str) -> BibProblem {
        BibProblem {
            line: line_of(bib, self.offset),
            message: self.message,
        }
    }
}

/// 按 GB/T 7714—2015 顺序编码制排出每一条的著录（键 → 不带序号的著录）。
fn format_all(bibliography: &Bibliography) -> Result<HashMap<String, String>, Pending> {
    let library = hayagriva::io::from_biblatex(bibliography).map_err(|errors| {
        let error = errors.into_iter().next();
        Pending {
            offset: error.as_ref().map_or(0, |error| error.span.start),
            message: error.map_or_else(
                || "字段格式不对".to_string(),
                |error| format!("字段格式不对（{}）", error.kind),
            ),
        }
    })?;
    let Style::Independent(style) = ArchivedStyle::Gb77142015Numeric.get() else {
        unreachable!("GB/T 7714 是独立样式");
    };
    let locales = hayagriva::archive::locales();
    // 研究报告模板 `#set text(lang: "zh", region: "cn")`，Typst 据此给 hayagriva
    // 的就是 zh-CN。
    let locale = LocaleCode("zh-CN".to_string());
    let mut driver = BibliographyDriver::new();
    for entry in library.iter() {
        driver.citation(CitationRequest::new(
            vec![CitationItem::with_entry(entry)],
            &style,
            Some(locale.clone()),
            &locales,
            None,
        ));
    }
    let rendered = driver.finish(BibliographyRequest::new(&style, Some(locale), &locales));
    Ok(rendered
        .bibliography
        .map(|bibliography| {
            bibliography
                .items
                .into_iter()
                .map(|item| (item.key, format!("{:#}", item.content).trim().to_string()))
                .collect()
        })
        .unwrap_or_default())
}

fn line_of(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text.as_bytes()[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

fn parse_message(kind: &ParseErrorKind) -> String {
    let token = |token: &Token| match token {
        Token::Identifier => "条目键或字段名",
        Token::OpeningBrace => "左花括号 {",
        Token::ClosingBrace => "右花括号 }",
        Token::Comma => "逗号",
        Token::QuotationMark => "双引号",
        Token::Equals => "等号",
        Token::DecimalPoint => "小数点",
    };
    match kind {
        ParseErrorKind::UnexpectedEof => "文件意外结束，多半少了右花括号".to_string(),
        ParseErrorKind::Expected(expected) => format!("这里应有{}", token(expected)),
        ParseErrorKind::Unexpected(found) => format!("这里不该有{}", token(found)),
        ParseErrorKind::UnknownAbbreviation(name) => format!("用了未定义的缩写 {name}"),
        ParseErrorKind::MalformedCommand => "有一个写坏了的 LaTeX 命令".to_string(),
        ParseErrorKind::DuplicateKey(key) => format!("条目键 {key} 定义了不止一次"),
        ParseErrorKind::ResolutionError(error) => format!("crossref 关联出错（{error}）"),
        other => format!("格式错误（{other}）"),
    }
}

/// 字段原文；没有这个字段为空。
fn field(entry: &biblatex::Entry, name: &str) -> String {
    entry
        .fields
        .get(name)
        .map(|chunks| chunks.format_verbatim().trim().to_string())
        .unwrap_or_default()
}

fn year(entry: &biblatex::Entry) -> String {
    let year = field(entry, "year");
    if !year.is_empty() {
        return year;
    }
    field(entry, "date").chars().take(4).collect()
}

/// 责任者：作者，没有就用编者。
fn people(entry: &biblatex::Entry) -> Vec<Person> {
    entry
        .author()
        .ok()
        .filter(|people| !people.is_empty())
        .or_else(|| {
            entry
                .editors()
                .ok()
                .and_then(|editors| editors.into_iter().next())
                .map(|(people, _)| people)
        })
        .unwrap_or_default()
}

fn is_cjk(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
    })
}

/// 中文名姓名连写（`{王, 明}` 也拼回“王明”），西文名照“名 姓”。
fn full_name(person: &Person) -> String {
    if is_cjk(&person.name) || is_cjk(&person.given_name) {
        format!("{}{}", person.name, person.given_name)
    } else {
        [person.given_name.as_str(), person.name.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// 下拉里那一栏：第一责任者（中文全名、西文只写姓），多人时加“等”或 et al.。
fn first_author(entry: &biblatex::Entry, people: &[Person]) -> String {
    let Some(first) = people.first() else {
        // 机构作者：GB/T 7714 允许主要责任者是机构。
        return ["organization", "institution", "school", "publisher"]
            .into_iter()
            .map(|name| field(entry, name))
            .find(|value| !value.is_empty())
            .unwrap_or_default();
    };
    let cjk = is_cjk(&first.name) || is_cjk(&first.given_name);
    let name = if cjk {
        full_name(first)
    } else if first.name.is_empty() {
        first.given_name.clone()
    } else {
        first.name.clone()
    };
    match (people.len() > 1, cjk) {
        (false, _) => name,
        (true, true) => format!("{name} 等"),
        (true, false) => format!("{name} et al."),
    }
}

/// BibTeX 条目类型的中文叫法，按 GB/T 7714 的文献类型标识归类。
fn kind_label(kind: &EntryType) -> &'static str {
    match kind {
        EntryType::Article | EntryType::Periodical | EntryType::SuppPeriodical => "期刊",
        EntryType::Book
        | EntryType::MvBook
        | EntryType::Booklet
        | EntryType::Collection
        | EntryType::MvCollection
        | EntryType::Reference
        | EntryType::MvReference
        | EntryType::Manual => "专著",
        EntryType::InBook
        | EntryType::BookInBook
        | EntryType::SuppBook
        | EntryType::InCollection
        | EntryType::SuppCollection
        | EntryType::InReference => "析出",
        EntryType::InProceedings | EntryType::Proceedings | EntryType::MvProceedings => "会议",
        EntryType::Thesis | EntryType::MastersThesis | EntryType::PhdThesis => "学位论文",
        EntryType::Report | EntryType::TechReport => "报告",
        EntryType::Patent => "专利",
        EntryType::Online => "电子资源",
        EntryType::Software => "软件",
        EntryType::Dataset => "数据集",
        EntryType::Unknown(name) => match name.to_ascii_lowercase().as_str() {
            "standard" => "标准",
            "newspaper" => "报纸",
            "www" | "webpage" | "electronic" => "电子资源",
            "database" => "数据集",
            "archive" => "档案",
            "map" => "舆图",
            _ => "其他",
        },
        EntryType::Misc | EntryType::Unpublished | EntryType::Set | EntryType::XData => "其他",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIB: &str = "@article{wang2020,\n  author = {王明 and 李华},\n  title = {数字政府建设的制度逻辑},\n  journal = {中国行政管理},\n  year = {2020},\n  volume = {36},\n  number = {2},\n  pages = {10--18},\n  langid = {chinese}\n}\n\n@book{smith2019,\n  author = {Smith, John and Doe, Jane},\n  title = {Public Administration},\n  publisher = {Springer},\n  address = {Berlin},\n  year = {2019}\n}\n\n@standard{gbt7714,\n  author = {全国信息与文献标准化技术委员会},\n  title = {信息与文献 参考文献著录规则: GB/T 7714—2015},\n  publisher = {中国标准出版社},\n  address = {北京},\n  date = {2015-05-15}\n}\n";

    #[test]
    fn entries_carry_author_year_title_and_kind_in_bib_order() {
        let library = parse(BIB);
        assert_eq!(library.problem, None);
        let keys: Vec<_> = library.entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["wang2020", "smith2019", "gbt7714"]);
        let wang = library.get("wang2020").expect("wang2020");
        assert_eq!(
            (wang.author.as_str(), wang.year.as_str(), wang.kind),
            ("王明 等", "2020", "期刊")
        );
        assert_eq!(wang.title, "数字政府建设的制度逻辑");
        let smith = library.get("smith2019").expect("smith2019");
        assert_eq!(
            (smith.author.as_str(), smith.kind),
            ("Smith et al.", "专著")
        );
        let standard = library.get("gbt7714").expect("gbt7714");
        assert_eq!((standard.year.as_str(), standard.kind), ("2015", "标准"));
    }

    /// 著录是参考文献表里那一行：带文献类型标识，不带序号。
    #[test]
    fn formatted_entries_follow_gbt7714_without_the_number() {
        let library = parse(BIB);
        let wang = &library.get("wang2020").expect("wang2020").formatted;
        assert!(wang.contains("王明"), "{wang}");
        assert!(wang.contains("[J]"), "{wang}");
        assert!(!wang.starts_with('['), "{wang}");
        let smith = &library.get("smith2019").expect("smith2019").formatted;
        assert!(smith.contains("[M]"), "{smith}");
    }

    #[test]
    fn search_covers_key_people_title_and_year() {
        let library = parse(BIB);
        let hits = |query: &str| {
            library
                .entries
                .iter()
                .filter(|entry| entry.matches(query))
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(hits("李华"), ["wang2020"]);
        assert_eq!(hits("jane"), ["smith2019"]);
        assert_eq!(hits("2015"), ["gbt7714"]);
        assert_eq!(hits("制度逻辑"), ["wang2020"]);
        assert_eq!(hits("").len(), 3);
    }

    /// 解析不了时报出行号，键照样用正则列出来，下拉不至于空掉。
    #[test]
    fn a_broken_bib_reports_the_line_and_still_lists_keys() {
        let bib = "@article{a,\n  title = {甲}\n}\n\n@article{b,\n  title = {乙\n";
        let library = parse(bib);
        let problem = library.problem.expect("应报错");
        assert!(problem.line >= 5, "{problem:?}");
        let keys: Vec<_> = library.entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["a", "b"]);

        let duplicated = parse("@article{a, title={甲}}\n@book{a, title={乙}}\n");
        let problem = duplicated.problem.expect("重复键应报错");
        assert!(problem.message.contains("a 定义了不止一次"), "{problem:?}");
        assert_eq!(problem.line, 2);
    }

    #[test]
    fn library_is_cached_by_content() {
        assert_eq!(*library(BIB), parse(BIB));
        assert!(library("").entries.is_empty());
    }
}
