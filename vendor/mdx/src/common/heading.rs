//! 标题旧编号去除：处理多种中英编号样式，保留纯标题文本。
//!
//! 取自 md_to_docx_rust::clean_heading_number；patterns 同时是 md2tex 那 6 条的超集，
//! 因此公文 / 研报两种 emitter 可共用。

use regex::Regex;
use std::sync::OnceLock;

/// 14 条按优先级排列的去编号规则。
const HEADING_PATTERNS: &[&str] = &[
    // 0a. 附录编号（附录E / 附录 A / 附录A.1）
    r"^附录\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?\s*",
    // 0b. Appendix 编号（Appendix A / Appendix A.1）
    r"^(?i:appendix)\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?\s*",
    // 1. 第X章/节/条/部分（中文数字或阿拉伯数字）
    r"^第[一二三四五六七八九十百零\d]+[章节条部分]\s*[、.．]?\s*",
    // 2. 全角括号中文数字 （一）（二）
    r"^[（(][一二三四五六七八九十百零]+[）)]\s*[、.．]?\s*",
    // 3. 中文数字+顿号/点号
    r"^[一二三四五六七八九十百零]+[、,.．]\s*",
    // 4. 全角括号阿拉伯数字 （1）（2）
    r"^[（(]\d+[）)]\s*[、.．]?\s*",
    // 5. 半角括号阿拉伯数字 (1)(2)
    r"^\(\d+\)\s*[、.．]?\s*",
    // 6. 多级数字编号 1.1 / 1.1.1 / 1.1.1.1
    r"^\d+(?:\.\d+)+[.．]?\s*",
    // 7. 阿拉伯数字+点号/顿号+空格
    r"^\d+[.．、]\s+",
    // 8. 阿拉伯数字+空格。只移除编号和空白，不能吞掉标题首字符。
    r"^\d+\s+",
    // 9. 圆圈数字 ①②③
    r"^[①②③④⑤⑥⑦⑧⑨⑩⑪⑫⑬⑭⑮⑯⑰⑱⑲⑳㉑㉒㉓㉔㉕㉖㉗㉘㉙㉚㉛㉜㉝㉞㉟㊱㊲㊳㊴㊵㊶㊷㊸㊹㊺㊻㊼㊽㊾㊿]\s*[.．、]?\s*",
    // 10. 带圈数字 ⑴⑵⑶
    r"^[⑴⑵⑶⑷⑸⑹⑺⑻⑼⑽⑾⑿⒀⒁⒂⒃⒄⒅⒆⒇]\s*[.．、]?\s*",
    // 11. 罗马数字 ⅠⅡⅢ
    r"^[ⅠⅡⅢⅣⅤⅥⅦⅧⅨⅩ]\s*[.．、]?\s*",
    // 12. 字母编号 A. a)
    r"^[A-Za-z][)）.．]\s*",
];

fn regexes() -> &'static [Regex] {
    static REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
    REGEXES.get_or_init(|| {
        HEADING_PATTERNS
            .iter()
            .map(|p| Regex::new(p).expect("invalid heading pattern"))
            .collect()
    })
}

/// 去掉 markdown 标题里的旧编号（"一、" / "（一）" / "1." / "1.1.1" / 圆圈数字 等）。
pub fn clean(text: &str) -> String {
    let mut cleaned = text.to_string();
    for re in regexes() {
        cleaned = re.replace(&cleaned, "").to_string();
    }
    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_chinese_chapter() {
        assert_eq!(clean("第一章 引言"), "引言");
        assert_eq!(clean("第二节 背景"), "背景");
    }

    #[test]
    fn strips_chinese_ordinal() {
        assert_eq!(clean("一、引言"), "引言");
        assert_eq!(clean("二. 方法"), "方法");
    }

    #[test]
    fn strips_parenthesized_chinese() {
        assert_eq!(clean("（一）目标"), "目标");
        assert_eq!(clean("(二) 范围"), "范围");
    }

    #[test]
    fn strips_multilevel_arabic() {
        assert_eq!(clean("1.1 简介"), "简介");
        assert_eq!(clean("1.2.3 细节"), "细节");
        assert_eq!(clean("1.1.1.1 子项"), "子项");
    }

    #[test]
    fn strips_single_arabic_without_eating_title() {
        assert_eq!(clean("3 第三章标题"), "第三章标题");
        assert_eq!(clean("12 战略目标"), "战略目标");
    }

    #[test]
    fn passes_through_clean_titles() {
        assert_eq!(clean("纯标题"), "纯标题");
    }

    #[test]
    fn strips_appendix_numbering() {
        assert_eq!(clean("附录E 集成任务清单"), "集成任务清单");
        assert_eq!(clean("附录A：术语表"), "术语表");
        assert_eq!(clean("附录 1 补充材料"), "补充材料");
        assert_eq!(clean("Appendix E Task List"), "Task List");
    }
}
