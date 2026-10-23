//! 字段之间的全角空格：「联系人：张某　联系电话：12345678」。
//!
//! 导入管线的 `clean::normalize_spaces` 会删掉汉字之间的空白——那是转换器插进来的
//! 噪音（PDF 抽字、Word 源码折行）。可粘贴进来的这种空格是作者敲的分隔符，删掉就成了
//! 「张某联系电话」，意思都变了。导入那边的规则不动，这里只在粘贴管线里挑出最有把握
//! 的一种：空白后面紧跟「××：」这样的字段标签，就先换成占位符护住，结构还原之后再
//! 换回一个全角空格。
//!
//! 别的汉字间空白（「王　五」这种姓名补位、「一、　总体要求」编号后的空格）照旧交给
//! 清洗规则处理，和导入的结果一致。

/// 占位符：Unicode 非字符，正常文本里不会出现，清洗与结构还原都不把它当空白。
const GAP: char = '\u{FDD0}';
/// 字段标签最少、最多几个汉字（「电话：」到「联系人电话号码：」）。
const LABEL_MIN: usize = 2;
const LABEL_MAX: usize = 8;

/// 把「字段分隔空白」换成占位符。
///
/// 只认这样的空白：不在行首（行首是缩进）；含全角空格，或者是两个以上的半角空格、
/// 不换行空格、制表符（Word 另存文本用制表符对齐）；不跨行（Word HTML 源码的折行
/// 不是分隔符）；前面是非空白字符，后面紧跟 2–8 个汉字加冒号。
///
/// 可以重复调用：已经换好的占位符原样保留（HTML 那条路逐个文本节点护过一遍，
/// 汇到结构还原前还会再护一遍）。
pub(crate) fn protect(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if !is_gap_space(ch) {
            out.push(ch);
            index += 1;
            continue;
        }
        let mut end = index;
        while end < chars.len() && is_gap_space(chars[end]) {
            end += 1;
        }
        let run = &chars[index..end];
        let after_text = out
            .chars()
            .next_back()
            .is_some_and(|ch| !ch.is_whitespace());
        let separator = run.contains(&'\u{3000}') || run.len() >= 2;
        if after_text && separator && starts_label(&chars[end..]) {
            out.push(GAP);
        } else {
            out.extend(run);
        }
        index = end;
    }
    out
}

/// 结构还原之后把占位符换回全角空格。
pub(crate) fn restore(text: &str) -> String {
    text.replace(GAP, "\u{3000}")
}

/// 同一行里的空白（不含换行）。
fn is_gap_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\u{a0}' | '\u{3000}')
}

/// 从这里起是不是「××：」字段标签。
fn starts_label(rest: &[char]) -> bool {
    let han = rest.iter().take_while(|ch| is_han(**ch)).count();
    (LABEL_MIN..=LABEL_MAX).contains(&han) && matches!(rest.get(han), Some('：' | ':'))
}

fn is_han(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_before_a_field_label_are_protected() {
        let protected = protect("联系人：张某　联系电话：12345678");
        assert!(protected.contains(GAP), "{protected:?}");
        assert_eq!(restore(&protected), "联系人：张某　联系电话：12345678");
        // 两个以上半角空格、不换行空格、制表符也算分隔符，换回来统一成一个全角空格。
        for raw in [
            "联系人：张某  联系电话：1",
            "联系人：张某\u{a0}\u{a0}联系电话：1",
            "联系人：张某\t\t联系电话：1",
        ] {
            assert_eq!(
                restore(&protect(raw)),
                "联系人：张某　联系电话：1",
                "{raw:?}"
            );
        }
    }

    #[test]
    fn other_spaces_are_left_to_the_cleaner() {
        for raw in [
            // 姓名补位、编号后的空格：后面不是字段标签。
            "王　五",
            "一、　总体要求",
            // 行首缩进。
            "　　联系电话：12345678",
            // 只有一个半角空格：可能是转换噪音。
            "张某 联系电话：1",
            // 标签太长，是一句话不是字段。
            "张某　请各单位于本月底前将材料报送至办公室：",
        ] {
            assert_eq!(protect(raw), raw, "{raw:?}");
        }
        // 跨行的空白不是分隔符（Word HTML 源码折行）。
        assert_eq!(protect("张某\n  联系电话：1"), "张某\n  联系电话：1");
    }

    #[test]
    fn protecting_twice_changes_nothing() {
        let once = protect("联系人：张某　联系电话：1");
        assert_eq!(protect(&once), once);
    }
}
