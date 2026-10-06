//! 按节写的接口文档：每个接口一节（Markdown 标题），节里用「请求地址：」「请求方式：」
//! 字段行或两列的键值表写地址与方法，参数写成表格；也认文档开头的接口总览表。
//! Word 文档转成 Markdown 后是同样的结构（表格没有表头行时，转出来第一行是空表头）。
//!
//! 只认结构，不猜意思：地址、方法、参数名都取自原文。

use super::scan::{DocParam, ParamPlace, RawRequest, method_of};
use crate::agent::api::{ApiMethod, InputKind};
use regex::Regex;
use std::sync::LazyLock;

static MD_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.*?)\s*#*\s*$").expect("标题正则"));
/// 没有 `#` 的编号标题：`2.1 政策检索`、`三、统计查询`、`（一）政策检索`。
static NUMBERED_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\d+(?:\.\d+)+\.?|[一二三四五六七八九十]+、|[（(][一二三四五六七八九十]+[）)])\s*(\S.{0,40})$")
        .expect("编号标题正则")
});
/// 整行加粗：`**政策检索**`。
static BOLD_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\*\*([^*]{1,30})\*\*\s*[:：]?$").expect("加粗行正则"));
/// 字段行：`请求地址：/api/x`、`- **请求方式**：POST`。
static FIELD_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[-*+>]\s*|\d+[.、)]\s*)?(?:\*\*)?([^：:|*]{1,14}?)(?:\*\*)?\s*[：:]\s*(.+)$")
        .expect("字段行正则")
});
static METHOD_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(GET|POST|PUT|DELETE|PATCH)\b").expect("方法正则"));
static PATH_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"https?://[A-Za-z0-9.\-]+(?::\d+)?[^\s`'\x22<>，。；（）()]*|/[A-Za-z0-9_\-.~{}:/?=&%]+",
    )
    .expect("路径正则")
});
/// 说明里带的样例：「如：中小企业」「例如 2026-09」。
static EXAMPLE_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:(?:例如|比如)[：:]?\s*|(?:示例|样例|如|例)(?:[：:]\s*|\s+))[「“\x22`']?([^\s，,。；;）)」”\x22`']{1,40})")
        .expect("说明样例正则")
});

const URL_LABELS: &[&str] = &[
    "请求地址",
    "接口地址",
    "请求路径",
    "接口路径",
    "请求url",
    "接口url",
    "url",
    "url地址",
    "请求uri",
    "uri",
    "path",
    "访问地址",
    "调用地址",
    "地址",
    "路径",
];
const METHOD_LABELS: &[&str] = &[
    "请求方式",
    "请求方法",
    "请求类型",
    "方法",
    "method",
    "http方法",
    "httpmethod",
    "调用方式",
    "提交方式",
];
/// 接口一节里的小标题，不算新的一节。
const SUB_HEADINGS: &[&str] = &[
    "接口说明",
    "接口描述",
    "功能说明",
    "功能描述",
    "请求参数",
    "请求参数说明",
    "返回参数",
    "返回参数说明",
    "响应参数",
    "响应参数说明",
    "请求示例",
    "返回示例",
    "响应示例",
    "请求头",
    "请求体",
    "请求说明",
    "返回说明",
    "返回结果",
    "响应结果",
    "返回值",
    "返回数据",
    "错误码",
    "状态码",
    "备注",
    "说明",
    "入参",
    "出参",
    "header参数",
    "body参数",
    "query参数",
    "path参数",
    "路径参数",
    "查询参数",
    "公共参数",
    "调用示例",
    "示例",
];
/// 像接口名称的词：带这些词的标题当新的一节。
const ENDPOINT_WORDS: &[&str] = &[
    "查询", "检索", "获取", "列表", "详情", "统计", "接口", "搜索", "分页", "新增", "删除", "修改",
    "上传", "下载", "导出",
];
const REQUEST_WORDS: &[&str] = &[
    "请求参数",
    "请求字段",
    "入参",
    "输入参数",
    "查询参数",
    "query",
    "body",
    "请求体",
    "请求头",
    "header",
    "路径参数",
    "path",
];
/// 只认「返回参数」这类说法：接口说明里常有「返回政策列表」，不能因此把请求参数表当成返回。
const RESPONSE_WORDS: &[&str] = &[
    "返回参数",
    "返回字段",
    "返回结果",
    "返回示例",
    "返回值",
    "返回数据",
    "返回说明",
    "响应参数",
    "响应字段",
    "响应结果",
    "响应示例",
    "出参",
    "输出参数",
    "response",
    "错误码",
    "状态码",
];

const NAME_COLUMNS: &[&str] = &[
    "参数名",
    "参数",
    "参数名称",
    "字段名",
    "字段",
    "字段名称",
    "名称",
    "name",
    "参数key",
    "key",
    "属性",
    "属性名",
    "变量名",
];
const TYPE_COLUMNS: &[&str] = &["类型", "参数类型", "数据类型", "字段类型", "type", "格式"];
const REQUIRED_COLUMNS: &[&str] = &[
    "必填",
    "是否必填",
    "必须",
    "是否必须",
    "必选",
    "是否必选",
    "required",
    "可选",
    "是否可选",
    "可为空",
    "是否可为空",
];
const DESCRIPTION_COLUMNS: &[&str] = &[
    "说明",
    "描述",
    "含义",
    "备注",
    "参数说明",
    "字段说明",
    "参数描述",
    "description",
    "释义",
    "中文名",
    "中文名称",
];
const EXAMPLE_COLUMNS: &[&str] = &[
    "示例",
    "示例值",
    "样例",
    "样例值",
    "例子",
    "example",
    "参考值",
];
const PLACE_COLUMNS: &[&str] = &["位置", "参数位置", "in", "传参方式", "参数来源"];
const OVERVIEW_NAME_COLUMNS: &[&str] = &[
    "接口名称",
    "接口名",
    "名称",
    "功能",
    "接口功能",
    "用途",
    "说明",
    "描述",
    "接口说明",
];

/// 统一写法：去空白、加粗、反引号，转小写，去掉结尾冒号。
fn norm(text: &str) -> String {
    clean(text)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
        .trim_end_matches([':', '：'])
        .to_string()
}

/// 单元格与字段值：去掉 Markdown 记号与转义。
fn clean(text: &str) -> String {
    let text = text.replace("<br>", " ").replace("**", "").replace('`', "");
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek()
            && next.is_ascii_punctuation()
        {
            out.push(next);
            chars.next();
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

fn is_label(text: &str, labels: &[&str]) -> bool {
    labels.contains(&norm(text).as_str())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Url,
    Method,
}

fn field_of(label: &str) -> Option<Field> {
    if is_label(label, URL_LABELS) {
        Some(Field::Url)
    } else if is_label(label, METHOD_LABELS) {
        Some(Field::Method)
    } else {
        None
    }
}

/// 标题的分类。
enum Heading {
    /// 新的一节（可能是一个接口）。
    Section(String),
    /// 「请求地址」「请求方式」这类字段名做的小标题：值在下一行。
    Field(Field),
    /// 「请求参数」「返回示例」这类小标题：只决定后面表格的归属。
    Context(String),
}

fn heading_of(line: &str) -> Option<Heading> {
    let title = if let Some(caps) = MD_HEADING.captures(line) {
        caps[2].to_string()
    } else if let Some(caps) = BOLD_LINE.captures(line) {
        caps[1].to_string()
    } else if let Some(caps) = NUMBERED_HEADING.captures(line)
        && !caps[1].contains(['：', ':', '，', ',', '。', '；', ';', '|'])
    {
        caps[1].to_string()
    } else {
        return None;
    };
    let title = strip_numbering(&clean(&title));
    if let Some(field) = field_of(&title) {
        return Some(Heading::Field(field));
    }
    let normalized = norm(&title);
    if SUB_HEADINGS.contains(&normalized.as_str()) {
        return Some(Heading::Context(title));
    }
    let short = normalized.chars().count() <= 8;
    let endpoint_like = ENDPOINT_WORDS.iter().any(|w| normalized.contains(w));
    let context_like = ["参数", "示例", "返回", "响应", "错误码", "状态码"]
        .iter()
        .any(|w| normalized.contains(w));
    if short && context_like && !endpoint_like {
        Some(Heading::Context(title))
    } else {
        Some(Heading::Section(title))
    }
}

/// 去掉标题前的编号：`2.1 `、`三、`、`（一）`、`1. `。
fn strip_numbering(title: &str) -> String {
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:\d+(?:\.\d+)*[.、]?|[一二三四五六七八九十]+、|[（(][一二三四五六七八九十\d]+[）)])\s*")
            .expect("编号正则")
    });
    NUMBER.replace(title.trim(), "").trim().to_string()
}

/// 参数归属：请求还是返回。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Request(ParamPlace),
    Response,
    Unknown,
}

/// 看上下文里最后提到的是请求参数还是返回参数。
fn role_of(context: &str) -> Role {
    let context = context.to_lowercase();
    let tail: String = {
        let chars: Vec<char> = context.chars().collect();
        chars[chars.len().saturating_sub(200)..].iter().collect()
    };
    let last = |words: &[&str]| words.iter().filter_map(|w| tail.rfind(w)).max();
    match (last(REQUEST_WORDS), last(RESPONSE_WORDS)) {
        (Some(request), Some(response)) if response > request => Role::Response,
        (None, Some(_)) => Role::Response,
        (Some(_), _) => Role::Request(place_of(&tail)),
        (None, None) => Role::Unknown,
    }
}

/// 从上下文或「位置」列认参数放在哪。
fn place_of(text: &str) -> ParamPlace {
    let text = text.to_lowercase();
    let last = |words: &[&str]| words.iter().filter_map(|w| text.rfind(w)).max();
    [
        (ParamPlace::Header, last(&["请求头", "header"])),
        (ParamPlace::Path, last(&["路径参数", "path"])),
        (
            ParamPlace::Query,
            last(&["query", "查询参数", "url参数", "地址参数"]),
        ),
        (ParamPlace::Body, last(&["body", "请求体", "json"])),
    ]
    .into_iter()
    .filter_map(|(place, at)| at.map(|at| (at, place)))
    .max_by_key(|(at, _)| *at)
    .map_or(ParamPlace::Unknown, |(_, place)| place)
}

/// 一个 Markdown 表格。
struct Table {
    rows: Vec<(usize, Vec<String>)>,
}

fn split_row(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = line.strip_suffix('|').unwrap_or(line);
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for c in line.chars() {
        if escaped {
            current.push('\\');
            current.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '|' {
            cells.push(clean(&current));
            current.clear();
        } else {
            current.push(c);
        }
    }
    cells.push(clean(&current));
    cells
}

fn is_separator(cells: &[String]) -> bool {
    cells.iter().all(|c| {
        let c = c.trim();
        c.is_empty() || c.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
    }) && cells.iter().any(|c| c.contains('-'))
}

/// 参数表的列位置。
#[derive(Debug, Clone, Default)]
struct ParamColumns {
    name: usize,
    kind: Option<usize>,
    required: Option<usize>,
    /// 列名是「可选 / 可为空」，取值要反过来读。
    optional: bool,
    description: Option<usize>,
    example: Option<usize>,
    place: Option<usize>,
}

fn find_column(cells: &[String], names: &[&str]) -> Option<usize> {
    cells.iter().position(|c| is_label(c, names))
}

fn param_columns(cells: &[String]) -> Option<ParamColumns> {
    let name = find_column(cells, NAME_COLUMNS)?;
    let kind = find_column(cells, TYPE_COLUMNS);
    let required = find_column(cells, REQUIRED_COLUMNS);
    let description = find_column(cells, DESCRIPTION_COLUMNS).filter(|i| *i != name);
    if kind.is_none() && required.is_none() && description.is_none() {
        return None;
    }
    // 总览表（有地址列）不是参数表。
    if find_column(cells, URL_LABELS).is_some() {
        return None;
    }
    let optional = required.is_some_and(|i| {
        let label = norm(&cells[i]);
        !label.contains('必') && label != "required"
    });
    Some(ParamColumns {
        name,
        kind,
        required,
        optional,
        description,
        example: find_column(cells, EXAMPLE_COLUMNS),
        place: find_column(cells, PLACE_COLUMNS),
    })
}

/// 总览表的列位置：至少要有地址列。
struct OverviewColumns {
    url: usize,
    method: Option<usize>,
    name: Option<usize>,
}

fn overview_columns(cells: &[String]) -> Option<OverviewColumns> {
    let url = cells.iter().position(|c| {
        let label = norm(c);
        URL_LABELS.contains(&label.as_str())
            || ["接口地址", "请求地址", "地址", "路径", "url"]
                .iter()
                .any(|w| label.ends_with(w))
    })?;
    let method = find_column(cells, METHOD_LABELS);
    let name = cells
        .iter()
        .enumerate()
        .position(|(i, c)| i != url && Some(i) != method && is_label(c, OVERVIEW_NAME_COLUMNS));
    Some(OverviewColumns { url, method, name })
}

fn cell(cells: &[String], index: Option<usize>) -> &str {
    index.and_then(|i| cells.get(i)).map_or("", |c| c.as_str())
}

/// 类型写法 → 参数类型。联合类型（`string | object`）里有字符串的按文字收，最宽松；
/// 纯对象、数组、映射（`map<string, Question>`、`array<string>`）整段按 JSON 传。
pub(super) fn kind_of(text: &str) -> Option<InputKind> {
    let text = text.to_lowercase();
    let mut depth = 0usize;
    let mut parts = vec![String::new()];
    for c in text.chars() {
        match c {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => {
                parts.push(String::new());
                continue;
            }
            _ => {}
        }
        parts.last_mut().expect("至少一段").push(c);
    }
    let parts: Vec<&str> = parts.iter().map(|p| p.trim()).collect();
    let starts = |words: &[&str]| {
        parts
            .iter()
            .any(|part| words.iter().any(|w| part.starts_with(w)))
    };
    if starts(&["string", "str", "text", "字符", "文本", "\"", "'"]) {
        return Some(InputKind::Text);
    }
    if starts(&[
        "object", "array", "list", "map", "dict", "json", "对象", "数组", "集合", "列表",
    ]) || text.ends_with("[]")
    {
        return Some(InputKind::Json);
    }
    Some(
        if [
            "int", "long", "number", "double", "float", "decimal", "数字", "数值", "整数", "整型",
        ]
        .iter()
        .any(|w| text.contains(w))
        {
            InputKind::Number
        } else if ["bool", "布尔"].iter().any(|w| text.contains(w)) {
            InputKind::Bool
        } else {
            InputKind::Text
        },
    )
}

/// 「是 / Y / 必填 / √」算必填；「否 / 非必填 / 选填」不算。
fn yes(text: &str) -> Option<bool> {
    let text = norm(text);
    if text.is_empty() {
        return None;
    }
    if [
        "否",
        "n",
        "no",
        "false",
        "非必填",
        "选填",
        "可选",
        "不必填",
        "非必须",
        "×",
    ]
    .iter()
    .any(|w| text == *w || text.starts_with(w))
    {
        return Some(false);
    }
    if [
        "是", "y", "yes", "true", "必填", "必须", "必选", "√", "✓", "required",
    ]
    .iter()
    .any(|w| text == *w || text.starts_with(w))
    {
        return Some(true);
    }
    None
}

fn param_from_row(
    cells: &[String],
    columns: &ParamColumns,
    default: ParamPlace,
) -> Option<DocParam> {
    let raw = cells.get(columns.name)?.trim();
    // 嵌套字段（`data.items`、`└ title`、`-- id`）不是请求参数的顶层。
    if raw.is_empty()
        || raw.starts_with(['└', '├', '│', '-', '·', '>', '|', '　', ' '])
        || raw.contains(['.', '[', ' '])
        || !raw.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        || !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    let kind = match columns.kind {
        Some(i) => kind_of(cells.get(i).map_or("", String::as_str))?,
        None => InputKind::Text,
    };
    let required = yes(cell(cells, columns.required))
        .map(|flag| flag != columns.optional)
        .unwrap_or(false);
    let description = cell(cells, columns.description).trim().to_string();
    let mut example = cell(cells, columns.example).trim().to_string();
    if matches!(example.as_str(), "无" | "-" | "—" | "/") {
        example.clear();
    }
    if example.is_empty()
        && let Some(caps) = EXAMPLE_IN_TEXT.captures(&description)
    {
        example = caps[1].trim().to_string();
    }
    let place = match columns.place {
        Some(i) => match place_of(cells.get(i).map_or("", String::as_str)) {
            ParamPlace::Unknown => default,
            place => place,
        },
        None => default,
    };
    Some(DocParam {
        name: raw.to_string(),
        kind,
        required,
        description,
        example,
        place,
    })
}

/// 从字段值里取地址与方法：`POST /api/x`、`` `GET` ``、`http://h/api/x （GET）`。
fn url_and_method(value: &str) -> (Option<String>, Option<String>) {
    let value = clean(value);
    let found = PATH_TOKEN.find(&value).filter(|m| m.as_str().len() > 1);
    // 方法只在地址以外找：`/api/fav/delete` 里的 delete 不是方法。
    let rest = match found {
        Some(m) => format!("{} {}", &value[..m.start()], &value[m.end()..]),
        None => value.clone(),
    };
    let method = METHOD_WORD
        .captures(&rest)
        .map(|caps| caps[1].to_ascii_uppercase());
    let url = found.map(|m| {
        m.as_str()
            .trim_end_matches(['.', ',', '。', '，', '；', ';', ')', '）'])
            .to_string()
    });
    (url, method)
}

/// 正在读的一节。
#[derive(Default)]
struct Section {
    title: Option<String>,
    start: usize,
    url: Option<String>,
    method: Option<String>,
    params: Vec<DocParam>,
    notes: Vec<String>,
    /// 上一个表格之后的文字与小标题：判断下一个参数表属于请求还是返回。
    context: String,
    /// 这一节里已经出现过「返回 / 响应」。
    seen_response: bool,
    /// 上一个小标题是「请求地址」这类字段名：值在下一行。
    pending: Option<Field>,
}

impl Section {
    fn new(title: Option<String>, start: usize) -> Self {
        Self {
            title,
            start,
            ..Self::default()
        }
    }

    fn set(&mut self, field: Field, value: &str) {
        let (url, method) = url_and_method(value);
        match field {
            Field::Url => {
                if self.url.is_none() {
                    self.url = url;
                }
                if self.method.is_none() {
                    self.method = method;
                }
            }
            Field::Method => {
                if self.method.is_none() {
                    self.method = method;
                }
                if self.url.is_none() && url.is_some() {
                    self.url = url;
                }
            }
        }
    }

    fn add_context(&mut self, text: &str) {
        self.context.push_str(text);
        self.context.push('\n');
        let lower = text.to_lowercase();
        if RESPONSE_WORDS.iter().any(|w| lower.contains(w)) {
            self.seen_response = true;
        }
    }

    fn table(&mut self, table: &Table, overview: &mut Vec<RawRequest>) {
        let mut params: Option<(ParamColumns, Role)> = None;
        let mut columns: Option<OverviewColumns> = None;
        let mut table_context = String::new();
        for (offset, cells) in &table.rows {
            let filled: Vec<&String> = cells.iter().filter(|c| !c.is_empty()).collect();
            if filled.is_empty() || is_separator(cells) {
                continue;
            }
            // 只有一格有字（或合并单元格）：「请求参数」这类分隔行。
            if filled.iter().all(|c| *c == filled[0]) {
                table_context = filled[0].clone();
                if RESPONSE_WORDS
                    .iter()
                    .any(|w| filled[0].to_lowercase().contains(w))
                {
                    self.seen_response = true;
                }
                params = None;
                columns = None;
                continue;
            }
            // 键值行：第一格是「请求地址」「请求方式」。
            if let Some(field) = field_of(&cells[0])
                && filled.len() >= 2
                && overview_columns(cells).is_none_or(|_| filled.len() == 2)
            {
                let value = cells[1..]
                    .iter()
                    .find(|c| !c.is_empty())
                    .cloned()
                    .unwrap_or_default();
                self.set(field, &value);
                params = None;
                columns = None;
                continue;
            }
            if let Some(found) = param_columns(cells) {
                let context = if table_context.is_empty() {
                    self.context.clone()
                } else {
                    table_context.clone()
                };
                let role = match role_of(&context) {
                    Role::Unknown if !self.seen_response && self.params.is_empty() => {
                        Role::Request(ParamPlace::Unknown)
                    }
                    Role::Unknown => Role::Response,
                    role => role,
                };
                params = Some((found, role));
                columns = None;
                continue;
            }
            if let Some(found) = overview_columns(cells) {
                columns = Some(found);
                params = None;
                continue;
            }
            if let Some((found, role)) = &params {
                let Role::Request(place) = role else {
                    continue;
                };
                match param_from_row(cells, found, *place) {
                    Some(param) => {
                        if !self.params.iter().any(|p| p.name == param.name) {
                            self.params.push(param);
                        }
                    }
                    None => {
                        if let Some(name) = cells.get(found.name)
                            && !name.trim().is_empty()
                            && found.kind.is_some_and(|i| {
                                kind_of(cells.get(i).map_or("", String::as_str)).is_none()
                            })
                        {
                            self.notes.push(format!(
                                "参数「{}」是对象或数组，暂只支持简单值，没有做成查询条件",
                                name.trim()
                            ));
                        }
                    }
                }
                continue;
            }
            if let Some(found) = &columns {
                let (url, method) = url_and_method(cell(cells, Some(found.url)));
                let method = method.or_else(|| url_and_method(cell(cells, found.method)).1);
                if let Some(url) = url {
                    let name = clean(cell(cells, found.name));
                    overview.push(request(
                        (!name.is_empty()).then_some(name),
                        url,
                        method,
                        *offset,
                        true,
                    ));
                }
            }
        }
        self.context.clear();
    }

    fn add_param(&mut self, param: DocParam) {
        if !self.params.iter().any(|p| p.name == param.name) {
            self.params.push(param);
        }
    }

    fn finish(self, out: &mut Vec<RawRequest>, loose: &mut Vec<(usize, DocParam)>) {
        let Some(url) = self.url else {
            loose.extend(self.params.into_iter().map(|p| (self.start, p)));
            return;
        };
        let mut found = request(self.title, url, self.method, self.start, false);
        let posts = self.params.iter().any(|p| p.place == ParamPlace::Body);
        if found.method_guessed && posts {
            found.method = Some(ApiMethod::Post);
        }
        if found.method_guessed {
            found.notes.push(
                if posts {
                    "文档没写请求方式，有请求体参数，按 POST 处理"
                } else {
                    "文档没写请求方式，按 GET 处理"
                }
                .into(),
            );
        }
        for param in self.params {
            if param.place == ParamPlace::Header {
                let lower = format!("{} {}", param.description, param.example).to_lowercase();
                let value = if lower.contains("bearer") {
                    format!(
                        "Bearer {}",
                        param.example.trim_start_matches("Bearer ").trim()
                    )
                } else {
                    param.example.clone()
                };
                found.headers.push((param.name, value));
            } else {
                found.params.push(param);
            }
        }
        found.notes.extend(self.notes);
        out.push(found);
    }
}

fn request(
    name: Option<String>,
    url: String,
    method: Option<String>,
    offset: usize,
    overview: bool,
) -> RawRequest {
    let (method, guessed) = match method {
        Some(word) => (method_of(&word), false),
        None => (Some(ApiMethod::Get), true),
    };
    RawRequest {
        method,
        url,
        offset,
        name,
        method_guessed: guessed,
        overview,
        ..RawRequest::default()
    }
}

/// Mintlify 一类文档站的参数标签：`<ParamField body="state" type="string" required>`。
static PARAM_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^<(ParamField|ResponseField)\b((?:[^>"/]|"[^"]*"|/[^>])*)(/?)>"#)
        .expect("参数标签正则")
});
static TAG_ATTR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"([A-Za-z_]+)="([^"]*)""#).expect("属性正则"));
static MD_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").expect("链接正则"));

fn unescape_entities(text: &str) -> String {
    text.replace("&#x22;", "\"")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// 参数标签 → 参数（只认请求参数；名字不像参数名的、返回字段不收）。
fn param_tag(attrs: &str) -> Option<DocParam> {
    let mut place = None;
    let mut name = String::new();
    let mut kind = String::new();
    for caps in TAG_ATTR.captures_iter(attrs) {
        match &caps[1] {
            "body" => (place, name) = (Some(ParamPlace::Body), caps[2].to_string()),
            "query" => (place, name) = (Some(ParamPlace::Query), caps[2].to_string()),
            "path" => (place, name) = (Some(ParamPlace::Path), caps[2].to_string()),
            "header" => (place, name) = (Some(ParamPlace::Header), caps[2].to_string()),
            "type" => kind = unescape_entities(&caps[2]),
            _ => {}
        }
    }
    let place = place?;
    let valid = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !valid {
        return None;
    }
    // 联合类型里只有一个字面量（`"noul"`）：这就是它的取值。
    let literal = kind.trim();
    let example = if literal.starts_with('"') && literal.ends_with('"') && literal.len() > 2 {
        literal.trim_matches('"').to_string()
    } else {
        String::new()
    };
    let required = Regex::new(r"\brequired\b")
        .map(|re| re.is_match(attrs))
        .unwrap_or(false);
    Some(DocParam {
        name,
        kind: kind_of(&kind).unwrap_or_default(),
        required,
        description: String::new(),
        example,
        place,
    })
}

/// 参数说明：去掉链接、行内代码记号，取第一句话。
fn tag_description(text: &str) -> String {
    let text = MD_LINK.replace_all(text.trim(), "$1").replace('`', "");
    let text = text.trim();
    let end = text
        .find(". ")
        .map(|i| i + 1)
        .or_else(|| text.find('。').map(|i| i + '。'.len_utf8()))
        .unwrap_or(text.len());
    text[..end].trim().to_string()
}

/// 按节、按表认出的请求。
#[cfg(test)]
pub(super) fn requests(text: &str) -> Vec<RawRequest> {
    parse(text).0
}

/// 按节、按表认出的请求，以及不在任何接口那一节里的参数（「请求体」单独成节的文档）：
/// (所在位置, 参数)。后者由调用方按请求体示例挂到接口上。
pub(super) fn parse(text: &str) -> (Vec<RawRequest>, Vec<(usize, DocParam)>) {
    let mut out = Vec::new();
    let mut loose = Vec::new();
    let mut overview = Vec::new();
    let mut section = Section::new(None, 0);
    let mut in_fence = false;
    let mut table: Option<Table> = None;
    let mut offset = 0;
    // 参数标签：嵌套深度（`<Expandable>` 里的是子字段，不收）与正在读说明的那个参数。
    let mut depth = 0usize;
    let mut tag: Option<DocParam> = None;
    for raw_line in text.split_inclusive('\n') {
        let start = offset;
        offset += raw_line.len();
        let line = raw_line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if line.starts_with("<Expandable") {
            depth += 1;
            continue;
        }
        if line.starts_with("</Expandable") {
            depth = depth.saturating_sub(1);
            continue;
        }
        if let Some(caps) = PARAM_TAG.captures(line) {
            if depth == 0 {
                if let Some(done) = tag.take() {
                    section.add_param(done);
                }
                let parsed = (&caps[1] == "ParamField")
                    .then(|| param_tag(&caps[2]))
                    .flatten();
                if caps[3].is_empty() {
                    tag = parsed;
                } else if let Some(done) = parsed {
                    section.add_param(done);
                }
            }
            continue;
        }
        if line.starts_with("</ParamField") || line.starts_with("</ResponseField") {
            if depth == 0
                && let Some(done) = tag.take()
            {
                section.add_param(done);
            }
            continue;
        }
        if let Some(param) = &mut tag {
            if depth == 0 && param.description.is_empty() && !line.is_empty() {
                param.description = tag_description(line);
            }
            continue;
        }
        if line.starts_with('|') {
            table
                .get_or_insert_with(|| Table { rows: Vec::new() })
                .rows
                .push((start, split_row(line)));
            continue;
        }
        if let Some(done) = table.take() {
            section.table(&done, &mut overview);
        }
        if line.is_empty() {
            continue;
        }
        if let Some(heading) = heading_of(line) {
            match heading {
                Heading::Section(title) => {
                    std::mem::replace(&mut section, Section::new(Some(title), start))
                        .finish(&mut out, &mut loose);
                }
                Heading::Field(field) => section.pending = Some(field),
                Heading::Context(title) => {
                    section.pending = None;
                    section.add_context(&title);
                }
            }
            continue;
        }
        if let Some(field) = section.pending.take() {
            section.set(field, line);
            continue;
        }
        if let Some(caps) = FIELD_LINE.captures(line)
            && let Some(field) = field_of(&caps[1])
        {
            section.set(field, &caps[2]);
            continue;
        }
        section.add_context(line);
    }
    if let Some(done) = table.take() {
        section.table(&done, &mut overview);
    }
    if let Some(done) = tag.take() {
        section.add_param(done);
    }
    section.finish(&mut out, &mut loose);
    out.extend(overview);
    out.sort_by_key(|r| r.offset);
    (out, loose)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一节一个接口，字段行 + 参数表（Markdown 写法）。
    const MARKDOWN: &str = "# 政策平台接口文档\n\n服务地址：http://10.0.0.9:8080\n\n\
## 1. 鉴权\n\n所有请求在请求头带 `X-API-Key`。\n\n\
## 2. 接口列表\n\n### 2.1 政策检索\n\n- **请求地址**：`/api/policy/search`\n- **请求方式**：POST\n\n\
#### 请求参数\n\n| 参数名 | 类型 | 是否必填 | 说明 |\n| --- | --- | --- | --- |\n\
| keyword | string | 是 | 关键词，如：中小企业 |\n| page_size | int | 否 | 每页条数 |\n| filters | object | 否 | 筛选条件 |\n\n\
#### 返回参数\n\n| 参数名 | 类型 | 说明 |\n| --- | --- | --- |\n| code | int | 0 成功 |\n| data | object | 数据 |\n\n\
### 2.2 政策详情\n\n请求地址：/api/policy/{id}\n\n请求方式：GET\n\n\
| 参数 | 类型 | 必填 | 说明 |\n|---|---|---|---|\n| id | string | 是 | 政策编号 |\n";

    #[test]
    fn markdown_sections_with_field_lines_and_parameter_tables() {
        let found = requests(MARKDOWN);
        assert_eq!(found.len(), 2, "{found:#?}");
        let search = &found[0];
        assert_eq!(search.name.as_deref(), Some("政策检索"));
        assert_eq!(search.url, "/api/policy/search");
        assert_eq!(search.method, Some(ApiMethod::Post));
        assert!(!search.method_guessed);
        let names: Vec<&str> = search.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["keyword", "page_size", "filters"],
            "返回参数不算，对象参数整段按 JSON 传"
        );
        assert_eq!(search.params[2].kind, InputKind::Json);
        assert!(search.params[0].required && !search.params[1].required);
        assert_eq!(search.params[0].example, "中小企业");
        assert_eq!(search.params[1].kind, InputKind::Number);
        let detail = &found[1];
        assert_eq!(detail.name.as_deref(), Some("政策详情"));
        assert_eq!(detail.url, "/api/policy/{id}");
        assert_eq!(detail.method, Some(ApiMethod::Get));
        assert_eq!(detail.params[0].name, "id");
    }

    /// Word 转出来的：表格没有表头行（第一行是空表头），键值表写地址与方法，
    /// 接口地址与参数在同一张表里，小标题没有 `#` 而是编号。
    #[test]
    fn word_tables_without_header_rows() {
        let doc = "3.1 月度统计查询\n\n|  |  |  |  |\n| --- | --- | --- | --- |\n\
| 接口地址 | http://10.1.2.3:9000/stat/monthly |  |  |\n| 请求方式 | GET |  |  |\n\
| 请求参数 |  |  |  |\n| 参数名称 | 数据类型 | 是否必须 | 描述 |\n\
| month | String | Y | 月份，例如 2026-09 |\n| region | String | N | 地区 |\n\
| 返回参数 |  |  |  |\n| 参数名称 | 数据类型 | 是否必须 | 描述 |\n| total | Integer | Y | 总数 |\n\n\
3.2 删除记录\n\n|  |  |\n| --- | --- |\n| 接口地址 | /stat/record/{id} |\n| 请求方式 | DELETE |\n";
        let found = requests(doc);
        assert_eq!(found.len(), 2, "{found:#?}");
        let stat = &found[0];
        assert_eq!(stat.name.as_deref(), Some("月度统计查询"));
        assert_eq!(stat.url, "http://10.1.2.3:9000/stat/monthly");
        assert_eq!(stat.method, Some(ApiMethod::Get));
        let names: Vec<&str> = stat.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["month", "region"]);
        assert!(stat.params[0].required && !stat.params[1].required);
        assert_eq!(stat.params[0].example, "2026-09");
        assert_eq!(found[1].method, None, "DELETE 记成改数据");
    }

    #[test]
    fn overview_tables_list_every_endpoint() {
        let doc = "## 接口一览\n\n| 序号 | 接口名称 | 请求方式 | 接口地址 |\n|---|---|---|---|\n\
| 1 | 政策检索 | POST | /api/policy/search |\n| 2 | 政策详情 | GET | /api/policy/{id} |\n\
| 3 | 统计 | | /api/stat |\n";
        let found = requests(doc);
        assert_eq!(found.len(), 3, "{found:#?}");
        assert!(found.iter().all(|r| r.overview));
        assert_eq!(found[0].name.as_deref(), Some("政策检索"));
        assert_eq!(found[0].method, Some(ApiMethod::Post));
        assert_eq!(found[1].url, "/api/policy/{id}");
        assert!(found[2].method_guessed);
    }

    #[test]
    fn header_parameters_and_label_headings() {
        let doc = "## 公文检索\n\n### 请求地址\n\n`/doc/search`\n\n### 请求方式\n\nGET\n\n\
### 请求头\n\n| 参数名 | 必填 | 说明 |\n|---|---|---|\n| Authorization | 是 | Bearer 令牌 |\n\n\
### 请求参数\n\n| 参数名 | 必填 | 说明 |\n|---|---|---|\n| q | 是 | 关键词 |\n";
        let found = requests(doc);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].url, "/doc/search");
        assert_eq!(found[0].method, Some(ApiMethod::Get));
        assert_eq!(
            found[0].headers,
            [("Authorization".to_string(), "Bearer ".to_string())]
        );
        assert_eq!(found[0].params.len(), 1);
        assert_eq!(found[0].params[0].name, "q");
    }

    #[test]
    fn param_field_tags_outside_the_endpoint_section_are_loose() {
        let doc = "## Evaluation endpoint\n\n```http\nPOST https://h/v1/eval\n```\n\n\
## Request body\n\n<ParamField body=\"state\" type=\"string | object | array\" required>\n  The content to evaluate. More words.\n</ParamField>\n\n\
<ParamField body=\"questions\" type=\"map<string, Question>\" required>\n  A map of [Question](#q) objects.\n\n  <Expandable title=\"map entries\">\n    <ParamField body=\"inner\" type=\"Question\">\n      nested\n    </ParamField>\n  </Expandable>\n</ParamField>\n\n\
### Noul\n\n<ParamField body=\"type\" type=\"&#x22;noul&#x22;\" required />\n\n<ResponseField name=\"answers\" type=\"map\">\n  x\n</ResponseField>\n";
        let (requests, loose) = parse(doc);
        assert!(requests.is_empty(), "请求行由 scan 认，这里不重复");
        let names: Vec<&str> = loose.iter().map(|(_, p)| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["state", "questions", "type"],
            "嵌套字段、返回字段不收"
        );
        assert_eq!(
            loose[0].1.kind,
            InputKind::Text,
            "联合类型里有字符串按文字收"
        );
        assert_eq!(loose[0].1.description, "The content to evaluate.");
        assert_eq!(loose[1].1.kind, InputKind::Json);
        assert_eq!(loose[1].1.description, "A map of Question objects.");
        assert!(loose[1].1.required && loose[1].1.place == ParamPlace::Body);
        assert_eq!(loose[2].1.example, "noul", "字面量类型就是取值");
        assert_eq!(kind_of("array<string>"), Some(InputKind::Json));
        assert_eq!(kind_of("integer"), Some(InputKind::Number));
    }

    #[test]
    fn values_and_flags_are_read_loosely() {
        assert_eq!(yes("非必填"), Some(false));
        assert_eq!(yes("必填"), Some(true));
        assert_eq!(yes("Y"), Some(true));
        assert_eq!(
            url_and_method("`POST` /api/x"),
            (Some("/api/x".into()), Some("POST".into()))
        );
        assert_eq!(
            url_and_method("http://h:8/api/x?a=1（GET）"),
            (Some("http://h:8/api/x?a=1".into()), Some("GET".into()))
        );
        assert_eq!(
            url_and_method("/gov/api/fav/delete"),
            (Some("/gov/api/fav/delete".into()), None),
            "地址里的 delete 不是方法"
        );
        assert_eq!(split_row(r"| a \| b | `c_d` |"), ["a | b", "c_d"]);
        assert!(matches!(
            heading_of("#### 返回参数"),
            Some(Heading::Context(_))
        ));
        assert!(matches!(
            heading_of("### 2.1 政策检索"),
            Some(Heading::Section(_))
        ));
        assert!(matches!(
            heading_of("**请求地址：**"),
            Some(Heading::Field(Field::Url))
        ));
        assert!(heading_of("1. keyword 为必填，size 选填").is_none());
    }
}
