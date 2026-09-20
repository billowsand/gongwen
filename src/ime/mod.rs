//! 应用内输入法。
//!
//! 公文助手自带拼音输入法：引擎、词库与整句模型都在本进程里，不走系统输入法，
//! 也没有独立进程或管道。打开应用就是中文输入，`Shift` 切换中英。
//!
//! * `cursor`：与**系统**输入法的对接——每帧报一次真正的光标矩形；接管键盘时
//!   把它整个关掉，免得两套输入法同时抢按键。
//! * `data` / `engine`：`.qj` 数据文件定位与引擎装配。
//! * `keys`：按键分流（哪些键归引擎、哪些交给应用）。
//! * `session`：组句状态、上屏、键盘接管。
//! * `candidates`：候选窗与拼音串。
//!
//! 词库与语言模型是 `.qj` 数据文件（vendor 自字在输入法，见 `vendor/qingjian/README.md`），
//! 找得到才启用；找不到就退回系统输入法，不挡用户打字。

use qingjian_core::{FumaScheme, ShuangpinScheme};

mod candidates;
mod cursor;
mod data;
mod engine;
mod keys;
mod lexicon;
mod session;

pub(crate) use cursor::follow_cursor;
pub(crate) use session::Ime;

/// 输入法设置：由应用配置映射过来，改一项就作用到引擎上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImeSettings {
    /// 是否启用应用内输入法。关掉就用系统输入法。
    pub(crate) enabled: bool,

    /// 双拼方案；`None` 是全拼。
    pub(crate) shuangpin: Option<ShuangpinScheme>,

    /// 双拼辅助码（形码）方案；`None` 是不用辅码。
    pub(crate) fuma: Option<FumaScheme>,

    /// 中文模式下的全角标点（，。：；）。
    pub(crate) full_width_punctuation: bool,

    /// 一页几个候选（1–9）。
    pub(crate) page_size: usize,

    /// 上一页 / 下一页键。
    pub(crate) page_keys: (char, char),
}

impl Default for ImeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            shuangpin: None,
            fuma: None,
            full_width_punctuation: true,
            page_size: 5,
            page_keys: ('[', ']'),
        }
    }
}

/// 一页最多几个候选。数字键只有 1–9，再多也按不到。
pub(crate) const MAX_PAGE_SIZE: usize = 9;

/// 翻页键的三组预设（与上游输入法一致）。
pub(crate) const PAGE_KEY_OPTIONS: [&str; 3] = ["[]", ",.", "-="];

impl ImeSettings {
    /// 从配置里的原始取值来：双拼方案名、翻页键、每页候选数。
    ///
    /// 认不出的名字一律当全拼 / 默认翻页键，不报错：配置是可以手改的，
    /// 一个字符串写错不该让整个输入法不能打字。
    pub(crate) fn from_config(
        enabled: bool,
        shuangpin: &str,
        full_width_punctuation: bool,
        page_size: usize,
        page_keys: &str,
    ) -> Self {
        Self {
            enabled,
            shuangpin: shuangpin.trim().parse().ok(),
            // 辅码由 `with_fuma` 补：它单独从配置里来，不影响其他几项的解析。
            fuma: None,
            full_width_punctuation,
            page_size: page_size.clamp(1, MAX_PAGE_SIZE),
            page_keys: parse_page_keys(page_keys).unwrap_or(Self::default().page_keys),
        }
    }

    /// 界面上列的双拼方案：第一项是全拼（配置里写空串），其后是引擎支持的四套。
    pub(crate) fn shuangpin_options() -> Vec<(&'static str, &'static str)> {
        let mut options: Vec<(&'static str, &'static str)> = vec![("", "全拼")];
        options.extend(
            ShuangpinScheme::ALL
                .into_iter()
                .map(|scheme| (scheme.key(), scheme.label())),
        );
        options
    }

    /// 界面上列的辅码方案：第一项是不用（配置里写空串），其后是引擎支持的方案。
    pub(crate) fn fuma_options() -> Vec<(&'static str, &'static str)> {
        let mut options: Vec<(&'static str, &'static str)> = vec![("", "不用辅码")];
        options.extend(
            FumaScheme::ALL
                .into_iter()
                .map(|scheme| (scheme.key(), scheme.label())),
        );
        options
    }

    /// 辅码方案从配置里的字符串来。认不出的名字当「不用辅码」。
    pub(crate) fn with_fuma(mut self, fuma: &str) -> Self {
        self.fuma = fuma.trim().parse().ok();
        self
    }
}

/// 翻页键：`"[]"` → `('[', ']')`。只认三组预设。
fn parse_page_keys(text: &str) -> Option<(char, char)> {
    let preset = PAGE_KEY_OPTIONS
        .iter()
        .find(|preset| preset.eq_ignore_ascii_case(text.trim()))?;
    let mut chars = preset.chars();
    Some((chars.next()?, chars.next()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认值：中文、全拼、全角标点、一页五个。
    #[test]
    fn defaults_are_full_pinyin_and_chinese() {
        let settings = ImeSettings::default();
        assert!(settings.enabled);
        assert!(settings.shuangpin.is_none());
        assert!(settings.full_width_punctuation);
        assert_eq!(settings.page_size, 5);
        assert_eq!(settings.page_keys, ('[', ']'));
    }

    #[test]
    fn from_config_reads_every_field() {
        let settings = ImeSettings::from_config(true, "xiaohe", false, 9, ",.");
        assert!(settings.enabled);
        assert_eq!(settings.shuangpin, Some(ShuangpinScheme::Xiaohe));
        assert!(!settings.full_width_punctuation);
        assert_eq!(settings.page_size, 9);
        assert_eq!(settings.page_keys, (',', '.'));
    }

    /// 空串与认不出的方案名都按全拼处理。
    #[test]
    fn unknown_shuangpin_falls_back_to_full_pinyin() {
        assert!(
            ImeSettings::from_config(true, "", true, 5, "[]")
                .shuangpin
                .is_none()
        );
        assert!(
            ImeSettings::from_config(true, "  ", true, 5, "[]")
                .shuangpin
                .is_none()
        );
        assert!(
            ImeSettings::from_config(true, "没这个方案", true, 5, "[]")
                .shuangpin
                .is_none()
        );
    }

    /// 翻页键只认三组预设，其余回默认。
    #[test]
    fn page_keys_only_accept_the_presets() {
        assert_eq!(
            ImeSettings::from_config(true, "", true, 5, "-=").page_keys,
            ('-', '=')
        );
        assert_eq!(
            ImeSettings::from_config(true, "", true, 5, ",.").page_keys,
            (',', '.')
        );
        assert_eq!(
            ImeSettings::from_config(true, "", true, 5, "ab").page_keys,
            ('[', ']')
        );
    }

    /// 每页候选数夹在 1–9。
    #[test]
    fn page_size_is_clamped() {
        assert_eq!(
            ImeSettings::from_config(true, "", true, 0, "[]").page_size,
            1
        );
        assert_eq!(
            ImeSettings::from_config(true, "", true, 99, "[]").page_size,
            MAX_PAGE_SIZE
        );
    }

    /// 双拼方案列表第一项是全拼，其后是引擎支持的四套。
    #[test]
    fn shuangpin_options_start_with_full_pinyin() {
        let options = ImeSettings::shuangpin_options();
        assert_eq!(options.first().copied(), Some(("", "全拼")));
        assert_eq!(options.len(), ShuangpinScheme::ALL.len() + 1);
    }

    /// 辅码：认得出的方案名照用，空串与错名字都是关着。
    #[test]
    fn fuma_comes_from_the_config_string() {
        let fuma = |text: &str| ImeSettings::default().with_fuma(text).fuma;
        assert_eq!(fuma("xiaohe"), Some(FumaScheme::Xiaohe));
        assert_eq!(fuma(""), None);
        assert_eq!(fuma("  "), None);
        assert_eq!(fuma("没这个方案"), None);
    }

    /// 辅码方案列表第一项是不用。
    #[test]
    fn fuma_options_start_with_off() {
        let options = ImeSettings::fuma_options();
        assert_eq!(options.first().copied(), Some(("", "不用辅码")));
        assert_eq!(options.len(), FumaScheme::ALL.len() + 1);
    }
}
