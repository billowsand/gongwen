//! 编号转换：中文数字、罗马数字、字母序号。

/// 1..=99 转中文数字（"一" / "十一" / "二十一"）；超出区间退化为阿拉伯数字。
pub fn number_to_chinese(num: usize) -> String {
    let chinese_nums = [
        "", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十",
    ];
    if num <= 10 {
        chinese_nums[num].to_string()
    } else if num < 20 {
        format!("十{}", chinese_nums[num - 10])
    } else if num < 100 {
        let tens = num / 10;
        let ones = num % 10;
        if ones == 0 {
            format!("{}十", chinese_nums[tens])
        } else {
            format!("{}十{}", chinese_nums[tens], chinese_nums[ones])
        }
    } else {
        num.to_string()
    }
}

/// 阿拉伯数字 → 大写罗马数字（"I"/"II"/"IX"/...）。
pub fn int_to_roman(num: usize) -> String {
    let val = [1000, 900, 500, 400, 100, 90, 50, 40, 10, 9, 5, 4, 1];
    let syms = [
        "M", "CM", "D", "CD", "C", "XC", "L", "XL", "X", "IX", "V", "IV", "I",
    ];
    let mut num = num;
    let mut result = String::new();
    for i in 0..val.len() {
        while num >= val[i] {
            result.push_str(syms[i]);
            num -= val[i];
        }
    }
    result
}

/// 1..=26 → "A".."Z"；27 → "AA"，依此类推（Excel 列名风格）。
pub fn number_to_uppercase_letter(num: usize) -> String {
    let mut num = num;
    let mut result = String::new();
    while num > 0 {
        let remainder = (num - 1) % 26;
        result.insert(0, (b'A' + remainder as u8) as char);
        num = (num - 1) / 26;
    }
    result
}

/// 公文 6 级列表前缀循环：①②③ → ⑴⑵⑶ → a.b.c. → I.II.III. → (A)(B) → 1)2)
///
/// 与 `ListState::next_prefix` 表达同一份循环表，但不依赖
/// 状态机；emitter 拿到 1..=6 的 level 和该 level 的 1-based 计数即可。
#[allow(dead_code)]
pub fn list_prefix(level: u8, count: usize) -> String {
    const CIRCLE_NUMBERS_2: &[&str] = &[
        "①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨", "⑩", "⑪", "⑫", "⑬", "⑭", "⑮", "⑯", "⑰", "⑱",
        "⑲", "⑳",
    ];
    const CIRCLE_NUMBERS_1: &[&str] = &[
        "⑴", "⑵", "⑶", "⑷", "⑸", "⑹", "⑺", "⑻", "⑼", "⑽", "⑾", "⑿", "⒀", "⒁", "⒂", "⒃", "⒄", "⒅",
        "⒆", "⒇",
    ];
    match level {
        1 => CIRCLE_NUMBERS_2
            .get(count - 1)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("({})", count)),
        2 => CIRCLE_NUMBERS_1
            .get(count - 1)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("({})", count)),
        3 => {
            let ch = (b'a' + ((count - 1) % 26) as u8) as char;
            format!("{}.", ch)
        }
        4 => format!("{}.", int_to_roman(count)),
        5 => format!("({})", number_to_uppercase_letter(count)),
        6 => format!("{})", count),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_numbers() {
        assert_eq!(number_to_chinese(1), "一");
        assert_eq!(number_to_chinese(10), "十");
        assert_eq!(number_to_chinese(11), "十一");
        assert_eq!(number_to_chinese(20), "二十");
        assert_eq!(number_to_chinese(25), "二十五");
        assert_eq!(number_to_chinese(99), "九十九");
        assert_eq!(number_to_chinese(100), "100");
    }

    #[test]
    fn roman_numerals() {
        assert_eq!(int_to_roman(1), "I");
        assert_eq!(int_to_roman(4), "IV");
        assert_eq!(int_to_roman(9), "IX");
        assert_eq!(int_to_roman(40), "XL");
        assert_eq!(int_to_roman(2024), "MMXXIV");
    }

    #[test]
    fn excel_columns() {
        assert_eq!(number_to_uppercase_letter(1), "A");
        assert_eq!(number_to_uppercase_letter(26), "Z");
        assert_eq!(number_to_uppercase_letter(27), "AA");
        assert_eq!(number_to_uppercase_letter(52), "AZ");
        assert_eq!(number_to_uppercase_letter(53), "BA");
    }

    #[test]
    fn list_prefix_levels() {
        assert_eq!(list_prefix(1, 1), "①");
        assert_eq!(list_prefix(2, 1), "⑴");
        assert_eq!(list_prefix(3, 1), "a.");
        assert_eq!(list_prefix(3, 27), "a.");
        assert_eq!(list_prefix(4, 4), "IV.");
        assert_eq!(list_prefix(5, 1), "(A)");
        assert_eq!(list_prefix(6, 7), "7)");
    }
}
