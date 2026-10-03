//! G 组：计算与文本（内置、确定性）。公文里天天用、又最容易被模型算错或写错的，交给程序算。

use super::{
    Input, Permission, Tool, ToolCtx, ToolOutput, arg_f64, arg_i64, arg_numbers, arg_str,
    arg_strings, arg_usize, optional, parse_number, required,
};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde_json::{Map, Value, json};

pub(super) const TOOLS: [&dyn Tool; 10] = [
    &DateTool, &Workday, &Ratio, &Stats, &Table, &Money, &Number, &Unit, &Keywords, &TextDiff,
];

// —— 日期 ——

/// 「2026年10月3日」「2026-10-03」「2026/10/3」「2026.10.3」「今天」→ 日期。
pub(crate) fn parse_date(text: &str) -> Option<NaiveDate> {
    let text = text.trim();
    if matches!(text, "" | "今天" | "今日" | "today") {
        return Some(chrono::Local::now().date_naive());
    }
    let digits: Vec<i64> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    match digits.as_slice() {
        [year, month, day, ..] => NaiveDate::from_ymd_opt(*year as i32, *month as u32, *day as u32),
        _ => None,
    }
}

fn weekday_label(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "星期一",
        Weekday::Tue => "星期二",
        Weekday::Wed => "星期三",
        Weekday::Thu => "星期四",
        Weekday::Fri => "星期五",
        Weekday::Sat => "星期六",
        Weekday::Sun => "星期日",
    }
}

/// 「2026年10月3日（星期六）」。
pub(crate) fn format_date(date: NaiveDate) -> String {
    format!(
        "{}年{}月{}日（{}）",
        date.year(),
        date.month(),
        date.day(),
        weekday_label(date.weekday())
    )
}

fn month_end(date: NaiveDate) -> NaiveDate {
    let (year, month) = if date.month() == 12 {
        (date.year() + 1, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1).expect("下月一日") - Duration::days(1)
}

struct DateTool;

impl Tool for DateTool {
    fn id(&self) -> &'static str {
        "calc.date"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "日期推算：某天加减若干天、两天相隔几天、星期几、所属季度、月末"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            optional("date", "起始日期，如 2026年10月3日；不给就是今天"),
            optional("op", "info（默认：星期、季度、月末）/ add / diff"),
            optional("days", "op 为 add 时加几天，可为负数"),
            optional("to", "op 为 diff 时的另一个日期"),
        ];
        INPUTS
    }
    fn run(
        &self,
        _ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let raw = arg_str(args, "date").unwrap_or_default();
        let date = parse_date(&raw).ok_or_else(|| format!("看不懂日期「{raw}」"))?;
        match arg_str(args, "op").as_deref().unwrap_or("info") {
            "add" => {
                let days = arg_i64(args, "days").ok_or("op 为 add 时要给 days")?;
                let result = date + Duration::days(days);
                Ok(ToolOutput::new(
                    json!({"date": format_date(result), "iso": result.to_string()}),
                    format!(
                        "{} {} {} 天 → {}",
                        format_date(date),
                        if days < 0 { "减" } else { "加" },
                        days.abs(),
                        format_date(result)
                    ),
                ))
            }
            "diff" => {
                let other_raw = arg_str(args, "to").ok_or("op 为 diff 时要给 to")?;
                let other =
                    parse_date(&other_raw).ok_or_else(|| format!("看不懂日期「{other_raw}」"))?;
                let days = (other - date).num_days();
                Ok(ToolOutput::new(
                    json!({"days": days}),
                    format!(
                        "{} 到 {} 相隔 {days} 天",
                        format_date(date),
                        format_date(other)
                    ),
                ))
            }
            "info" => {
                let quarter = (date.month() - 1) / 3 + 1;
                Ok(ToolOutput::new(
                    json!({
                        "date": format_date(date),
                        "weekday": weekday_label(date.weekday()),
                        "quarter": quarter,
                        "month_end": format_date(month_end(date)),
                    }),
                    format!("{}，第{quarter}季度", format_date(date)),
                ))
            }
            other => Err(format!("不认识的 op「{other}」")),
        }
    }
}

struct Workday;

impl Tool for Workday {
    fn id(&self) -> &'static str {
        "calc.workday"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "工作日推算：某天之后第 N 个工作日是哪天，扣除周末与给定的节假日，计入调休上班日"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            optional("date", "起始日期；不给就是今天"),
            required("days", "第几个工作日（正整数）"),
            optional("holidays", "节假日列表；不给就只扣周末"),
            optional("workdays", "调休上班的周末日期列表"),
        ];
        INPUTS
    }
    fn run(
        &self,
        _ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let raw = arg_str(args, "date").unwrap_or_default();
        let start = parse_date(&raw).ok_or_else(|| format!("看不懂日期「{raw}」"))?;
        let days = arg_usize(args, "days")
            .filter(|d| *d > 0)
            .ok_or("days 要是正整数")?;
        let holidays: Vec<NaiveDate> = arg_strings(args, "holidays")
            .iter()
            .filter_map(|d| parse_date(d))
            .collect();
        let workdays: Vec<NaiveDate> = arg_strings(args, "workdays")
            .iter()
            .filter_map(|d| parse_date(d))
            .collect();
        let mut current = start;
        let mut counted = 0;
        while counted < days {
            current += Duration::days(1);
            let weekend = matches!(current.weekday(), Weekday::Sat | Weekday::Sun);
            let working = workdays.contains(&current) || (!weekend && !holidays.contains(&current));
            if working {
                counted += 1;
            }
        }
        let mut summary = format!(
            "{} 之后第 {days} 个工作日是 {}",
            format_date(start),
            format_date(current)
        );
        let note = if holidays.is_empty() {
            summary.push_str("（未提供节假日表，只扣除了周末）");
            "未提供节假日表，只扣除了周末"
        } else {
            ""
        };
        Ok(ToolOutput::new(
            json!({"date": format_date(current), "iso": current.to_string(), "note": note}),
            summary,
        ))
    }
}

// —— 数字 ——

/// 按位数格式化，不做四舍六入五成双以外的花样（`{:.N}`）。
fn fixed(value: f64, digits: usize) -> String {
    format!("{value:.digits$}")
}

struct Ratio;

impl Tool for Ratio {
    fn id(&self) -> &'static str {
        "calc.ratio"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "同比、环比、增长率、占比、百分点差，按公文习惯写成「同比增长12.5%」这样的文字"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("current", "本期数（算百分点差时是本期百分比）"),
            optional("previous", "上期数 / 去年同期数"),
            optional("total", "算占比时的总数"),
            optional(
                "kind",
                "yoy 同比 / mom 环比 / growth 增长率（默认）/ share 占比 / points 百分点差",
            ),
            optional("digits", "保留几位小数，默认 1"),
        ];
        INPUTS
    }
    fn run(
        &self,
        _ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let current = arg_f64(args, "current").ok_or("current 要是数字")?;
        let digits = arg_usize(args, "digits").unwrap_or(1).min(6);
        let kind = arg_str(args, "kind").unwrap_or_else(|| {
            if args.contains_key("total") {
                "share".into()
            } else {
                "growth".into()
            }
        });
        let (value, text) = match kind.as_str() {
            "share" => {
                let total = arg_f64(args, "total").ok_or("算占比要给 total")?;
                if total == 0.0 {
                    return Err("总数为 0，无法算占比".into());
                }
                let share = current / total * 100.0;
                (share, format!("占比{}%", fixed(share, digits)))
            }
            "points" => {
                let previous = arg_f64(args, "previous").ok_or("算百分点差要给 previous")?;
                let diff = current - previous;
                let text = if fixed(diff.abs(), digits) == fixed(0.0, digits) {
                    "持平".to_string()
                } else {
                    format!(
                        "{}{}个百分点",
                        if diff > 0.0 { "提高" } else { "下降" },
                        fixed(diff.abs(), digits)
                    )
                };
                (diff, text)
            }
            "growth" | "yoy" | "mom" => {
                let previous = arg_f64(args, "previous").ok_or("算增长率要给 previous")?;
                if previous == 0.0 {
                    return Err("基数为 0，无法计算增长率".into());
                }
                let rate = (current - previous) / previous.abs() * 100.0;
                let prefix = match kind.as_str() {
                    "yoy" => "同比",
                    "mom" => "环比",
                    _ => "",
                };
                let text = if fixed(rate.abs(), digits) == fixed(0.0, digits) {
                    format!("{prefix}持平")
                } else {
                    format!(
                        "{prefix}{}{}%",
                        if rate > 0.0 { "增长" } else { "下降" },
                        fixed(rate.abs(), digits)
                    )
                };
                (rate, text)
            }
            other => return Err(format!("不认识的 kind「{other}」")),
        };
        Ok(ToolOutput::new(
            json!({"value": value, "text": text}),
            text.clone(),
        ))
    }
}

struct Stats;

impl Tool for Stats {
    fn id(&self) -> &'static str {
        "calc.stats"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "一组数的合计、平均、最大最小、从大到小的排名"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("values", "数字列表，或用逗号、顿号分隔的一串数"),
            optional("labels", "与数字一一对应的名称，排名时带上"),
            optional("digits", "平均数保留几位小数，默认 2"),
        ];
        INPUTS
    }
    fn run(
        &self,
        _ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let values = arg_numbers(args, "values")
            .filter(|v| !v.is_empty())
            .ok_or("values 里要有数字")?;
        let labels = arg_strings(args, "labels");
        let digits = arg_usize(args, "digits").unwrap_or(2).min(6);
        let sum: f64 = values.iter().sum();
        let mean = sum / values.len() as f64;
        let max = values.iter().copied().fold(f64::MIN, f64::max);
        let min = values.iter().copied().fold(f64::MAX, f64::min);
        let mut order: Vec<usize> = (0..values.len()).collect();
        order.sort_by(|a, b| {
            values[*b]
                .partial_cmp(&values[*a])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let ranking: Vec<Value> = order
            .iter()
            .enumerate()
            .map(|(rank, index)| {
                json!({
                    "rank": rank + 1,
                    "label": labels.get(*index).cloned().unwrap_or_else(|| format!("第{}项", index + 1)),
                    "value": values[*index],
                })
            })
            .collect();
        Ok(ToolOutput::new(
            json!({"count": values.len(), "sum": sum, "mean": fixed(mean, digits), "max": max, "min": min, "ranking": ranking}),
            format!(
                "{} 个数：合计 {sum}，平均 {}",
                values.len(),
                fixed(mean, digits)
            ),
        ))
    }
}

/// Markdown 表格：(表头, 行)。
fn parse_table(text: &str) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let rows: Vec<Vec<String>> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|'))
        .map(|line| {
            line.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect()
        })
        .collect();
    let (header, rest) = rows.split_first()?;
    let body: Vec<Vec<String>> = rest
        .iter()
        .filter(|row| {
            !row.iter()
                .all(|cell| cell.chars().all(|c| matches!(c, '-' | ':' | ' ')) && !cell.is_empty())
        })
        .cloned()
        .collect();
    Some((header.clone(), body))
}

fn render_table(header: &[String], rows: &[Vec<String>]) -> String {
    let line = |cells: &[String]| format!("| {} |", cells.join(" | "));
    let mut out = vec![
        line(header),
        format!("|{}|", vec!["---"; header.len()].join("|")),
    ];
    out.extend(rows.iter().map(|row| line(row)));
    out.join("\n")
}

struct Table;

impl Tool for Table {
    fn id(&self) -> &'static str {
        "calc.table"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "表格运算：对 Markdown 表格的某一列求和、加占比列、加合计行，返回新表格"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("table", "Markdown 表格"),
            required("column", "要算的列：表头文字，或从 1 开始的列号"),
            optional(
                "ops",
                "要做什么：sum（默认）、share 加占比列、total 加合计行，可写多个",
            ),
            optional("digits", "占比保留几位小数，默认 1"),
        ];
        INPUTS
    }
    fn run(
        &self,
        _ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let raw = arg_str(args, "table").unwrap_or_default();
        let (mut header, mut rows) = parse_table(&raw).ok_or("没认出 Markdown 表格")?;
        let column_arg = arg_str(args, "column").unwrap_or_default();
        let column = header
            .iter()
            .position(|h| h == column_arg.trim())
            .or_else(|| {
                column_arg
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
            })
            .filter(|index| *index < header.len())
            .ok_or_else(|| format!("表格里没有列「{column_arg}」"))?;
        let digits = arg_usize(args, "digits").unwrap_or(1).min(6);
        let mut ops = arg_strings(args, "ops");
        if ops.is_empty() {
            ops.push("sum".into());
        }
        let values: Vec<f64> = rows
            .iter()
            .map(|row| {
                row.get(column)
                    .and_then(|cell| parse_number(cell))
                    .unwrap_or(0.0)
            })
            .collect();
        let sum: f64 = values.iter().sum();
        if ops.iter().any(|op| op == "share") {
            if sum == 0.0 {
                return Err("这一列合计为 0，无法算占比".into());
            }
            header.push("占比".into());
            for (row, value) in rows.iter_mut().zip(&values) {
                row.resize(header.len() - 1, String::new());
                row.push(format!("{}%", fixed(value / sum * 100.0, digits)));
            }
        }
        if ops.iter().any(|op| op == "total") {
            let mut total = vec![String::new(); header.len()];
            total[0] = "合计".into();
            total[column] = trim_number(sum);
            if header.last().is_some_and(|h| h == "占比") {
                total[header.len() - 1] = format!("{}%", fixed(100.0, digits));
            }
            rows.push(total);
        }
        let table = render_table(&header, &rows);
        Ok(ToolOutput::new(
            json!({"table": table, "sum": sum}),
            format!("表格「{}」列合计 {}", header[column], trim_number(sum)),
        ))
    }
}

/// 整数不带小数点，小数去掉末尾的 0。
fn trim_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        let text = format!("{value:.6}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

// —— 金额 ——

const UPPER_DIGITS: [char; 10] = ['零', '壹', '贰', '叁', '肆', '伍', '陆', '柒', '捌', '玖'];

/// 人民币大写：1234.56 → 壹仟贰佰叁拾肆元伍角陆分；整数加「整」。
pub(crate) fn rmb_upper(amount: f64) -> Result<String, String> {
    if !(0.0..1e12).contains(&amount) {
        return Err("金额要在 0 到 1 万亿元之间".into());
    }
    let fen = (amount * 100.0).round() as u64;
    let (yuan, jiao, cents) = (fen / 100, (fen / 10) % 10, fen % 10);
    let mut out = if yuan == 0 {
        String::new()
    } else {
        integer_words(yuan, &UPPER_DIGITS, ['拾', '佰', '仟'], ["", "万", "亿"]) + "元"
    };
    if jiao == 0 && cents == 0 {
        if out.is_empty() {
            out.push_str("零元");
        }
        out.push('整');
        return Ok(out);
    }
    if jiao > 0 {
        out.push(UPPER_DIGITS[jiao as usize]);
        out.push('角');
    } else if yuan > 0 {
        out.push('零');
    }
    if cents > 0 {
        out.push(UPPER_DIGITS[cents as usize]);
        out.push('分');
    }
    Ok(out)
}

/// 整数 → 中文读法。按万、亿分节；节内与节间该补「零」的补一个。
fn integer_words(number: u64, digits: &[char; 10], small: [char; 3], big: [&str; 3]) -> String {
    if number == 0 {
        return digits[0].to_string();
    }
    let mut groups = Vec::new();
    let mut rest = number;
    while rest > 0 {
        groups.push((rest % 10_000) as u32);
        rest /= 10_000;
    }
    let mut out = String::new();
    let mut need_zero = false;
    for (index, group) in groups.iter().enumerate().rev() {
        if *group == 0 {
            need_zero = !out.is_empty();
            continue;
        }
        if (need_zero || (!out.is_empty() && *group < 1000)) && !out.ends_with(digits[0]) {
            out.push(digits[0]);
        }
        out.push_str(&group_words(*group, digits, small));
        out.push_str(big[index.min(2)]);
        need_zero = false;
    }
    out
}

/// 四位以内的一节。
fn group_words(group: u32, digits: &[char; 10], small: [char; 3]) -> String {
    let parts = [
        group / 1000,
        (group / 100) % 10,
        (group / 10) % 10,
        group % 10,
    ];
    let units = [Some(small[2]), Some(small[1]), Some(small[0]), None];
    let mut out = String::new();
    let mut zero = false;
    let mut started = false;
    for (digit, unit) in parts.iter().zip(units) {
        if *digit == 0 {
            zero = started;
            continue;
        }
        if zero {
            out.push(digits[0]);
            zero = false;
        }
        out.push(digits[*digit as usize]);
        if let Some(unit) = unit {
            out.push(unit);
        }
        started = true;
    }
    out
}

struct Money;

impl Tool for Money {
    fn id(&self) -> &'static str {
        "calc.money"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "金额：人民币大写、元 / 万元 / 亿元换算、千分位"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("amount", "金额数字"),
            optional("op", "upper 大写（默认）/ convert 换算 / format 千分位"),
            optional("from", "金额的单位：元（默认）/ 万元 / 亿元"),
            optional("to", "换算成：元 / 万元 / 亿元"),
            optional("digits", "换算结果保留几位小数，默认 2"),
        ];
        INPUTS
    }
    fn run(
        &self,
        _ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let amount = arg_f64(args, "amount").ok_or("amount 要是数字")?;
        let factor = |unit: &str| match unit {
            "元" => Ok(1.0),
            "万元" => Ok(1e4),
            "亿元" => Ok(1e8),
            other => Err(format!("不认识的金额单位「{other}」")),
        };
        let from = arg_str(args, "from").unwrap_or_else(|| "元".into());
        let yuan = amount * factor(&from)?;
        match arg_str(args, "op").as_deref().unwrap_or("upper") {
            "upper" => {
                let text = rmb_upper(yuan)?;
                Ok(ToolOutput::new(
                    json!({"text": text}),
                    format!("{amount}{from} → {text}"),
                ))
            }
            "convert" => {
                let to = arg_str(args, "to").ok_or("换算要给 to")?;
                let digits = arg_usize(args, "digits").unwrap_or(2).min(6);
                let value = yuan / factor(&to)?;
                let text = format!("{}{to}", fixed(value, digits));
                Ok(ToolOutput::new(
                    json!({"value": value, "text": text}),
                    format!("{amount}{from} → {text}"),
                ))
            }
            "format" => {
                let text = thousands(amount);
                Ok(ToolOutput::new(
                    json!({"text": text}),
                    format!("千分位：{text}"),
                ))
            }
            other => Err(format!("不认识的 op「{other}」")),
        }
    }
}

fn thousands(value: f64) -> String {
    let text = trim_number(value.abs());
    let (int, frac) = text
        .split_once('.')
        .map_or((text.as_str(), ""), |(i, f)| (i, f));
    let mut grouped = String::new();
    for (index, ch) in int.chars().enumerate() {
        if index > 0 && (int.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let sign = if value < 0.0 { "-" } else { "" };
    if frac.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{frac}")
    }
}

// —— 汉字数字 ——

const LOWER_DIGITS: [char; 10] = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'];

/// 整数 → 汉字：321 → 三百二十一；10–19 写「十X」。
pub(crate) fn to_chinese(number: u64) -> String {
    let text = integer_words(number, &LOWER_DIGITS, ['十', '百', '千'], ["", "万", "亿"]);
    text.strip_prefix("一十")
        .map_or(text.clone(), |rest| format!("十{rest}"))
}

/// 汉字 → 整数：三百二十一 → 321；「两」「〇」也认。
pub(crate) fn to_arabic(text: &str) -> Option<u64> {
    let digit = |c: char| match c {
        '零' | '〇' => Some(0),
        '一' | '壹' => Some(1),
        '二' | '两' | '贰' => Some(2),
        '三' | '叁' => Some(3),
        '四' | '肆' => Some(4),
        '五' | '伍' => Some(5),
        '六' | '陆' => Some(6),
        '七' | '柒' => Some(7),
        '八' | '捌' => Some(8),
        '九' | '玖' => Some(9),
        _ => None,
    };
    let (mut total, mut section, mut current) = (0u64, 0u64, 0u64);
    let mut seen = false;
    for c in text.trim().chars() {
        if let Some(d) = digit(c) {
            current = d;
            seen = true;
            continue;
        }
        let unit = match c {
            '十' | '拾' => 10,
            '百' | '佰' => 100,
            '千' | '仟' => 1000,
            '万' => 10_000,
            '亿' => 100_000_000,
            _ => return None,
        };
        seen = true;
        if unit >= 10_000 {
            section = (section + current) * unit;
            total += section;
            section = 0;
        } else {
            section += current.max(u64::from(unit == 10 && current == 0)) * unit;
        }
        current = 0;
    }
    seen.then_some(total + section + current)
}

struct Number;

impl Tool for Number {
    fn id(&self) -> &'static str {
        "calc.number"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "公文数字用法：阿拉伯数字与汉字数字互转（321 ↔ 三百二十一）"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("value", "数字或汉字数字"),
            optional("op", "to_chinese / to_arabic；不给按输入自动判断"),
        ];
        INPUTS
    }
    fn run(
        &self,
        _ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let value = arg_str(args, "value").unwrap_or_default();
        let op = arg_str(args, "op").unwrap_or_else(|| {
            if value.trim().parse::<u64>().is_ok() {
                "to_chinese".into()
            } else {
                "to_arabic".into()
            }
        });
        let text = match op.as_str() {
            "to_chinese" => to_chinese(
                value
                    .trim()
                    .parse()
                    .map_err(|_| format!("「{value}」不是非负整数"))?,
            ),
            "to_arabic" => to_arabic(&value)
                .ok_or_else(|| format!("看不懂汉字数字「{value}」"))?
                .to_string(),
            other => return Err(format!("不认识的 op「{other}」")),
        };
        Ok(ToolOutput::new(
            json!({"text": text}),
            format!("{value} → {text}"),
        ))
    }
}

// —— 计量单位 ——

/// (单位, 类别, 换算到基准单位的系数)。长度以米、面积以平方米、重量以千克为基准。
const UNITS: [(&str, &str, f64); 17] = [
    ("毫米", "长度", 0.001),
    ("厘米", "长度", 0.01),
    ("米", "长度", 1.0),
    ("千米", "长度", 1000.0),
    ("公里", "长度", 1000.0),
    ("平方米", "面积", 1.0),
    ("公顷", "面积", 10_000.0),
    ("平方千米", "面积", 1_000_000.0),
    ("平方公里", "面积", 1_000_000.0),
    ("亩", "面积", 10_000.0 / 15.0),
    ("万亩", "面积", 10_000.0 * 10_000.0 / 15.0),
    ("克", "重量", 0.001),
    ("千克", "重量", 1.0),
    ("公斤", "重量", 1.0),
    ("斤", "重量", 0.5),
    ("吨", "重量", 1000.0),
    ("万吨", "重量", 10_000_000.0),
];

struct Unit;

impl Tool for Unit {
    fn id(&self) -> &'static str {
        "calc.unit"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "计量单位换算：长度、面积（亩 / 公顷 / 平方米）、重量"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("value", "数值"),
            required("from", "原单位，如 亩"),
            required("to", "目标单位，如 公顷"),
            optional("digits", "保留几位小数，默认 2"),
        ];
        INPUTS
    }
    fn run(
        &self,
        _ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let value = arg_f64(args, "value").ok_or("value 要是数字")?;
        let find = |name: &str| {
            UNITS
                .iter()
                .find(|(unit, _, _)| *unit == name.trim())
                .ok_or_else(|| format!("不认识的单位「{name}」"))
        };
        let from_name = arg_str(args, "from").unwrap_or_default();
        let to_name = arg_str(args, "to").unwrap_or_default();
        let (from, from_kind, from_factor) = find(&from_name)?;
        let (to, to_kind, to_factor) = find(&to_name)?;
        if from_kind != to_kind {
            return Err(format!(
                "{from}是{from_kind}单位，{to}是{to_kind}单位，不能换算"
            ));
        }
        let digits = arg_usize(args, "digits").unwrap_or(2).min(6);
        let result = value * from_factor / to_factor;
        let text = format!("{}{to}", fixed(result, digits));
        Ok(ToolOutput::new(
            json!({"value": result, "text": text}),
            format!("{value}{from} = {text}"),
        ))
    }
}

// —— 文本 ——

/// 关键词里不要的常见虚词与公文套话。
const STOPWORDS: [&str; 24] = [
    "我们",
    "进一步",
    "有关",
    "相关",
    "工作",
    "开展",
    "加强",
    "做好",
    "一个",
    "以及",
    "进行",
    "关于",
    "通知",
    "要求",
    "根据",
    "按照",
    "坚持",
    "全面",
    "切实",
    "认真",
    "各级",
    "单位",
    "部门",
    "情况",
];

struct Keywords;

impl Tool for Keywords {
    fn id(&self) -> &'static str {
        "text.keywords"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "用分词抽一段文字的关键词（去掉常见虚词与套话），给检索拼检索词用"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            optional("text", "要抽关键词的文字；不给就用用户原话"),
            optional("top", "要几个，默认 8"),
        ];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let text = arg_str(args, "text").unwrap_or_else(|| ctx.board.request.clone());
        let top = arg_usize(args, "top").unwrap_or(8).clamp(1, 30);
        let tokens = crate::rag::tokenize(&text);
        let mut counts: Vec<(String, usize, usize)> = Vec::new();
        for (order, token) in tokens.split_whitespace().enumerate() {
            if token.chars().count() < 2
                || token
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_punctuation())
                || STOPWORDS.contains(&token)
            {
                continue;
            }
            match counts.iter_mut().find(|(word, _, _)| word == token) {
                Some(entry) => entry.1 += 1,
                None => counts.push((token.to_string(), 1, order)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
        let words: Vec<String> = counts
            .into_iter()
            .take(top)
            .map(|(word, _, _)| word)
            .collect();
        let summary = format!("关键词：{}", words.join("、"));
        Ok(ToolOutput::new(json!(words), summary))
    }
}

struct TextDiff;

impl Tool for TextDiff {
    fn id(&self) -> &'static str {
        "text.diff"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "两段文字逐句比较，列出删掉的与新增的句子"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[required("before", "原文"), required("after", "改后")];
        INPUTS
    }
    fn run(
        &self,
        _ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let before = arg_str(args, "before").unwrap_or_default();
        let after = arg_str(args, "after").unwrap_or_default();
        let (removed, added) = super::workspace::sentence_changes(&before, &after);
        let summary = format!("删去 {} 句、新增 {} 句", removed.len(), added.len());
        Ok(ToolOutput::new(
            json!({"removed": removed, "added": added}),
            summary,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::Fixture;
    use super::*;

    #[test]
    fn dates_parse_in_every_common_shape() {
        let expect = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        for text in ["2026年10月3日", "2026-10-03", "2026/10/3", "2026.10.03"] {
            assert_eq!(parse_date(text), Some(expect), "{text}");
        }
        assert_eq!(format_date(expect), "2026年10月3日（星期六）");
        assert!(parse_date("下周").is_none());
    }

    #[test]
    fn date_arithmetic_and_workdays() {
        let mut fixture = Fixture::new("");
        let out = fixture
            .call(
                "calc.date",
                json!({"date": "2026年10月3日", "op": "add", "days": 30}),
            )
            .unwrap();
        assert_eq!(out.value["date"], "2026年11月2日（星期一）");
        let out = fixture
            .call(
                "calc.date",
                json!({"date": "2026-10-03", "op": "diff", "to": "2026-12-01"}),
            )
            .unwrap();
        assert_eq!(out.value["days"], 59);
        let out = fixture
            .call("calc.date", json!({"date": "2026-02-10"}))
            .unwrap();
        assert_eq!(out.value["month_end"], "2026年2月28日（星期六）");
        assert_eq!(out.value["quarter"], 1);

        // 周五之后第 1 个工作日是下周一；把下周一设成假日就顺延到周二；周六调休上班就是周六。
        let friday = "2026-10-09";
        let out = fixture
            .call("calc.workday", json!({"date": friday, "days": 1}))
            .unwrap();
        assert_eq!(out.value["iso"], "2026-10-12");
        assert!(out.summary.contains("未提供节假日表"));
        let out = fixture
            .call(
                "calc.workday",
                json!({"date": friday, "days": 1, "holidays": ["2026-10-12"]}),
            )
            .unwrap();
        assert_eq!(out.value["iso"], "2026-10-13");
        let out = fixture
            .call(
                "calc.workday",
                json!({"date": friday, "days": 1, "workdays": "2026-10-10"}),
            )
            .unwrap();
        assert_eq!(out.value["iso"], "2026-10-10");
    }

    #[test]
    fn ratios_read_like_official_writing() {
        let mut fixture = Fixture::new("");
        let text = |fixture: &mut Fixture, args: Value| {
            fixture.call("calc.ratio", args).unwrap().value["text"].clone()
        };
        assert_eq!(
            text(
                &mut fixture,
                json!({"current": 112.5, "previous": 100, "kind": "yoy"})
            ),
            "同比增长12.5%"
        );
        assert_eq!(
            text(
                &mut fixture,
                json!({"current": 96.8, "previous": 100, "kind": "mom"})
            ),
            "环比下降3.2%"
        );
        assert_eq!(
            text(&mut fixture, json!({"current": 45.6, "total": 100})),
            "占比45.6%"
        );
        assert_eq!(
            text(
                &mut fixture,
                json!({"current": "38.5%", "previous": "36.4%", "kind": "points"})
            ),
            "提高2.1个百分点"
        );
        assert_eq!(
            text(&mut fixture, json!({"current": 100, "previous": 100})),
            "持平"
        );
        assert!(
            fixture
                .call("calc.ratio", json!({"current": 1, "previous": 0}))
                .unwrap_err()
                .contains("基数为 0")
        );
    }

    #[test]
    fn stats_and_tables() {
        let mut fixture = Fixture::new("");
        let out = fixture
            .call(
                "calc.stats",
                json!({"values": "30、50，20", "labels": ["甲", "乙", "丙"]}),
            )
            .unwrap();
        assert_eq!(out.value["sum"], 100.0);
        assert_eq!(out.value["mean"], "33.33");
        assert_eq!(out.value["ranking"][0]["label"], "乙");

        let table = "| 区县 | 起数 |\n|---|---|\n| 甲 | 3 |\n| 乙 | 1 |\n";
        let out = fixture
            .call(
                "calc.table",
                json!({"table": table, "column": "起数", "ops": ["share", "total"]}),
            )
            .unwrap();
        assert_eq!(
            out.value["table"],
            "| 区县 | 起数 | 占比 |\n|---|---|---|\n| 甲 | 3 | 75.0% |\n| 乙 | 1 | 25.0% |\n| 合计 | 4 | 100.0% |"
        );
        assert!(
            fixture
                .call("calc.table", json!({"table": table, "column": "人数"}))
                .unwrap_err()
                .contains("没有列")
        );
    }

    #[test]
    fn rmb_uppercase_handles_zeros_in_every_position() {
        let cases = [
            (0.0, "零元整"),
            (1234.56, "壹仟贰佰叁拾肆元伍角陆分"),
            (1001.0, "壹仟零壹元整"),
            (100_000.0, "壹拾万元整"),
            (100_010.0, "壹拾万零壹拾元整"),
            // 万位以下连续为零、千位不为零时「零」可写可不写（《支付结算办法》），这里不写。
            (10_005_000.0, "壹仟万伍仟元整"),
            (10_000_500.0, "壹仟万零伍佰元整"),
            (200_000_001.0, "贰亿零壹元整"),
            (3.05, "叁元零伍分"),
            (0.5, "伍角"),
        ];
        for (amount, expect) in cases {
            assert_eq!(rmb_upper(amount).unwrap(), expect, "{amount}");
        }
        let mut fixture = Fixture::new("");
        let out = fixture
            .call(
                "calc.money",
                json!({"amount": 12345, "op": "convert", "to": "万元"}),
            )
            .unwrap();
        assert_eq!(out.value["text"], "1.23万元");
        let out = fixture
            .call("calc.money", json!({"amount": 1234567.5, "op": "format"}))
            .unwrap();
        assert_eq!(out.value["text"], "1,234,567.5");
    }

    #[test]
    fn chinese_numbers_round_trip() {
        let cases = [
            (0, "零"),
            (10, "十"),
            (12, "十二"),
            (20, "二十"),
            (105, "一百零五"),
            (321, "三百二十一"),
            (10_010, "一万零一十"),
            (1_200_000, "一百二十万"),
        ];
        for (number, text) in cases {
            assert_eq!(to_chinese(number), text, "{number}");
            assert_eq!(to_arabic(text), Some(number), "{text}");
        }
        assert_eq!(to_arabic("两千"), Some(2000));
        assert_eq!(to_arabic("三季度"), None);
    }

    #[test]
    fn units_and_text_tools() {
        let mut fixture = Fixture::new("");
        let out = fixture
            .call(
                "calc.unit",
                json!({"value": 15, "from": "亩", "to": "公顷"}),
            )
            .unwrap();
        assert_eq!(out.value["text"], "1.00公顷");
        assert!(
            fixture
                .call("calc.unit", json!({"value": 1, "from": "亩", "to": "吨"}))
                .unwrap_err()
                .contains("不能换算")
        );

        let out = fixture
            .call(
                "text.keywords",
                json!({"text": "关于做好冬季森林防火工作的通知，森林防火责任要落实"}),
            )
            .unwrap();
        let words: Vec<String> = serde_json::from_value(out.value).unwrap();
        assert!(
            words.iter().take(2).any(|w| w.contains("森林")),
            "{words:?}"
        );
        assert!(!words.iter().any(|w| w == "通知" || w == "工作"));

        let out = fixture
            .call(
                "text.diff",
                json!({"before": "甲。乙。", "after": "甲。丙。"}),
            )
            .unwrap();
        assert_eq!(out.value["added"][0], "丙。");
    }
}
