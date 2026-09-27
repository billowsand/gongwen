//! 自定义短语：配置里的短语表交给引擎。
//!
//! 敲一串字母，在候选的固定位置出一段文字——常用套语（「特此函告」「妥否，请批示」）、
//! 单位全称、落款。匹配、排位、校验都是引擎做的（`qingjian_core::custom_phrase`），
//! 这里只管把配置换成引擎的写法、配置变了才重设。
//!
//! 日期（`rq`）、时间（`sj`）、星期（`xq`）引擎已内置，不在这张表里。

use qingjian_core::CustomPhrase;
use qingjian_core::custom_phrase::validate_phrases;

use super::session::Ime;
use crate::models::ImePhrase;

/// 配置里的短语换成引擎的写法。输入码或文字还空着的行（正在设置页里填）跳过：
/// 不然一行没填完，整张表都过不了校验。
fn to_engine(phrases: &[ImePhrase]) -> Vec<CustomPhrase> {
    phrases
        .iter()
        .filter(|phrase| !phrase.code.trim().is_empty() && !phrase.text.is_empty())
        .map(|phrase| CustomPhrase {
            code: phrase.code.trim().to_owned(),
            text: phrase.text.clone(),
            position: phrase.position,
            enabled: phrase.enabled,
        })
        .collect()
}

/// 校验短语表，设置页拿它提示哪里不对。没填完的行不算错。
pub(crate) fn validate(phrases: &[ImePhrase]) -> Result<(), String> {
    validate_phrases(&to_engine(phrases))
}

impl Ime {
    /// 应用短语表。表没变就什么都不做，可以每帧调。
    ///
    /// 表不合法时引擎保留上一份合法的（见 `Engine::set_custom_phrases`），设置页另有提示。
    pub(crate) fn apply_phrases(&mut self, phrases: &[ImePhrase]) {
        if self.phrases.as_slice() == phrases {
            return;
        }
        self.phrases = phrases.to_vec();
        self.push_phrases();
    }

    /// 把当前短语表交给引擎。引擎重新装配后也要调一次。
    pub(super) fn push_phrases(&mut self) {
        let phrases = to_engine(&self.phrases);
        if let Some(engine) = self.engine_mut()
            && let Err(error) = engine.set_custom_phrases(phrases)
        {
            eprintln!("[ime] 自定义短语不合法，沿用上一份：{error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phrase(code: &str, text: &str, position: usize) -> ImePhrase {
        ImePhrase {
            code: code.to_owned(),
            text: text.to_owned(),
            position,
            enabled: true,
        }
    }

    /// 没填完的行不交给引擎，也不算错。
    #[test]
    fn unfinished_rows_are_skipped() {
        let phrases = [phrase("", "特此函告", 1), phrase("tchg", "", 1)];
        assert!(to_engine(&phrases).is_empty());
        assert!(validate(&phrases).is_ok());
    }

    /// 输入码两边的空白去掉；大写、位置越界、同码同位都报错。
    #[test]
    fn validation_follows_the_engine_rules() {
        assert!(validate(&[phrase(" tchg ", "特此函告", 1)]).is_ok());
        assert!(validate(&[phrase("TCHG", "特此函告", 1)]).is_err());
        assert!(validate(&[phrase("tchg", "特此函告", 10)]).is_err());
        assert!(validate(&[phrase("tchg", "特此函告", 1), phrase("tchg", "特此通知", 1)]).is_err());
    }
}
