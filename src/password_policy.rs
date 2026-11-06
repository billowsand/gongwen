//! 密码强度策略：应用锁主/第二密码与稿件包导出密码共用同一套判据。
//!
//! 为什么单独成模块：这两处密码的**威胁模型完全一样**——
//! 锁文件 `vault.dat` 与导出用的 ZIP 容器都在磁盘上，攻击者可以拷走，
//! 拿密码去离线反复试。唯一的防线就是密码本身的强度，所以规则必须统一，
//! 不能一处松一处紧。
//!
//! 规则：
//! - 长度按**字符数**计（不是字节数，免得一个汉字只算 3 个字符）；
//! - 至少覆盖三类字符：大写、小写、数字、符号、中文；
//! - 拒绝常见口令、4 位及以上连续字母/数字、4 个及以上连续相同字符；
//! - 首尾不留空格。
//!
//! 应用锁的口径更严一档（长度必须**大于 10**）：它是整个加密体系的根，
//! 弱密码会让 Argon2id 的代价形同虚设——锁文件里存着「用密码派生的密钥
//! 加密的固定明文」，拷走即可离线穷举。简单字符等于没加密，所以宁可拒人。

use anyhow::{Result, bail};

/// 稿件包导出密码的最短长度（字符数）。
pub const EXPORT_MIN_CHARS: usize = 10;
/// 应用锁主/第二密码的最短长度（字符数）：必须**大于 10**。
pub const LOCK_MIN_CHARS: usize = 11;
/// 密码最长长度（字符数）。够用即可，避免超长输入拖慢 Argon2id。
pub const MAX_CHARS: usize = 128;
/// 至少要覆盖的字符类别数：大写、小写、数字、符号、中文。
const MIN_CATEGORIES: usize = 3;

/// 常见口令与弱模式片段（子串匹配，大小写不敏感）。
///
/// 只收**通用的**弱口令词。不收本程序自己的名字（“公文助手”）：用户很可能
/// 把工作单位/产品名写进密码，那是他们的自由，不该因为我们猜得到就拒人。
const COMMON: &[&str] = &[
    "password", "passwd", "qwerty", "admin", "letmein", "iloveyou", "welcome", "123456", "111111",
    "000000", "666666", "abcdef",
];

/// 应用锁密码的强度校验（更严：长度必须大于 10）。
///
/// 用户新设的应用锁主密码与第二密码都要过这一关。
pub fn validate_lock(password: &str) -> Result<()> {
    validate(password, LOCK_MIN_CHARS)
}

/// 稿件包导出密码的强度校验。
///
/// 导入历史稿件包时不调用本函数（外部工具生成的旧弱密码包要能导进来）；
/// 凡是**用户新设**的密码——应用锁主密码、第二密码、稿件包导出密码——
/// 都要过这一关。
pub fn validate_export(password: &str) -> Result<()> {
    validate(password, EXPORT_MIN_CHARS)
}

/// 校验密码强度。不合格时给出**具体是哪一条不满足**，让用户知道怎么改。
fn validate(password: &str, min_chars: usize) -> Result<()> {
    if password.trim() != password {
        bail!("密码首尾不能包含空格");
    }
    let chars = password.chars().collect::<Vec<_>>();
    if chars.len() < min_chars {
        bail!("密码至少需要 {min_chars} 个字符；太短的密码很容易被猜出来");
    }
    if chars.len() > MAX_CHARS {
        bail!("密码不能超过 {MAX_CHARS} 个字符");
    }

    let categories = [
        chars.iter().any(|ch| ch.is_ascii_lowercase()),
        chars.iter().any(|ch| ch.is_ascii_uppercase()),
        chars.iter().any(|ch| ch.is_numeric()),
        chars.iter().any(|ch| !ch.is_alphanumeric()),
        chars.iter().any(|ch| ch.is_alphabetic() && !ch.is_ascii()),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if categories < MIN_CATEGORIES {
        bail!(
            "密码需包含大写字母、小写字母、数字、符号或中文中的至少 {MIN_CATEGORIES} 类；只用一种字符很容易被猜出来"
        );
    }

    let lower = password.to_lowercase();
    if COMMON.iter().any(|word| lower.contains(word)) {
        bail!("密码包含常见口令或简单序列，很容易被猜出来，请换一个更复杂的密码");
    }
    if has_ascii_sequence(&lower) {
        bail!("密码不能包含 4 位及以上连续字母或数字；这类组合很容易被猜出来");
    }
    if chars
        .windows(4)
        .any(|window| window.iter().all(|ch| *ch == window[0]))
    {
        bail!("密码不能包含 4 个及以上连续相同字符；这类组合很容易被猜出来");
    }
    Ok(())
}

/// 密码里是否含 4 位及以上连续递增或递减的 ASCII 字母/数字。
fn has_ascii_sequence(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.windows(4).any(|window| {
        window.iter().all(u8::is_ascii_alphanumeric)
            && (window.windows(2).all(|pair| pair[1] == pair[0] + 1)
                || window.windows(2).all(|pair| pair[0] == pair[1] + 1))
    })
}

/// 给界面用的一句话规则说明（应用锁）。两处入口（锁屏首次设置、设置页启用）
/// 共用，免得文案两处不一致。
pub const LOCK_HINT: &str = "长度必须大于 10 个字符，且包含大写字母、小写字母、数字、符号或中文中的至少三类；不能用常见口令、连续或重复字符。简单字符等于没有加密。";

/// 给界面用的一句话规则说明（稿件包导出密码）。
pub const EXPORT_HINT: &str = "至少 10 个字符，且包含大写字母、小写字母、数字、符号或中文中的至少三类；不能用常见口令、连续或重复字符。";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strong_passwords_pass() {
        for good in [
            "Zz9#mQ2vLp4xR7",
            "我的公文很安全2026!",
            "Correct-Horse-9",
            "Test#Real2026Lock",
        ] {
            assert!(validate_lock(good).is_ok(), "应通过：{good}");
        }
    }

    #[test]
    fn lock_password_must_be_longer_than_ten() {
        // 恰好 10 个字符：导出密码够用，应用锁不够（要求大于 10）。
        assert!(validate_export("Ab1!xY9zWq").is_ok());
        assert!(validate_lock("Ab1!xY9zWq").is_err());
        // 11 个字符：应用锁够用。
        assert!(validate_lock("Ab1!xY9zWq2").is_ok());
        // 中文按字符数计（不是字节数）：11 个汉字就是 11 个字符。
        assert!(validate_lock("安全!公文7云端备份甲").is_ok());
    }

    #[test]
    fn short_passwords_are_rejected() {
        assert!(validate_lock("短").is_err());
        assert!(validate_lock("Ab1!").is_err());
        assert!(validate_lock("Ab1!efgh").is_err());
    }

    #[test]
    fn single_category_passwords_are_rejected() {
        assert!(validate_lock("abcdefghij").is_err(), "只有小写");
        assert!(validate_lock("ABCDEFGHIJ").is_err(), "只有大写");
        assert!(validate_lock("0123456789").is_err(), "只有数字");
        assert!(validate_lock("!!!!!!!!!!").is_err(), "只有符号");
        assert!(validate_lock("公文助手加密密码").is_err(), "只有中文");
    }

    #[test]
    fn common_and_sequential_passwords_are_rejected() {
        assert!(validate_lock("Password!7Cloud").is_err(), "常见口令");
        assert!(validate_lock("1234567890").is_err(), "常见口令");
        assert!(validate_lock("abcdefghij").is_err(), "连续字母");
        assert!(validate_lock("Abcd1234!x").is_err(), "含连续字母");
        assert!(validate_lock("Aaaa1234!x").is_err(), "含连续重复字符");
        // 连续数字也算（“1234”这种是最容易猜的）。
        assert!(validate_lock("Xy7!1234Ab").is_err(), "含连续数字");
    }

    #[test]
    fn leading_or_trailing_space_is_rejected() {
        assert!(validate_lock(" Abcdefg1!x").is_err());
        assert!(validate_lock("Abcdefg1!x ").is_err());
    }

    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        // 11 个汉字 = 33 字节，字符数够；但类别只有中文一类，仍应被类别规则拒绝。
        let chinese = "公文助手加密安全密码甲";
        assert_eq!(chinese.chars().count(), 11);
        assert!(validate_lock(chinese).is_err(), "只有中文一类");
        // 加一个数字与一个符号后通过。
        assert!(validate_lock(&format!("{chinese}7!")).is_ok());
    }

    #[test]
    fn overlong_passwords_are_rejected() {
        let long = format!("Ab1!{}", "x".repeat(MAX_CHARS));
        assert!(validate_lock(&long).is_err());
    }
}
