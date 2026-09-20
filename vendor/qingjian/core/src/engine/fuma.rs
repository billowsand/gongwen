//! 辅码激活的判定：认出缓冲区末尾的辅码键，供解码归一化、候选筛选与拼音行显示用。
//!
//! 打完完整双拼之后接着敲的键就是辅码键，按敲了几个、能不能当拼音分成两档：
//!
//! - **一个键**（`ljm`）：`m` 还可能是下一个字的声母（`lan m…` → 蓝莓），两种读法都留着，
//!   辅码匹配的候选置顶（[`FumaCodes::First`]）。这一键不从解码里剥掉，简拼词照常出。
//! - **两个键**（`ljms`、`ljmS`）：这两键解不成一个完整音节，或者里面有大写（用户明说了是辅码），
//!   没有歧义，只留匹配的候选（[`FumaCodes::Both`]）。这两键从解码里剥掉。
//!   两键都是小写且正好是一个合法音节时（正常的双音节连打）不当辅码，落回一个键那档。
//!
//! 第一键大写表示两码反转顺序匹配（`Ke` / `KE` 匹配实际辅码 `ek`），与水杉（MSIME-Engine）一致。
//!
//! 判定挂在每次 [`Engine::decode`] 上，所以没开辅码时必须一个堆分配都不做。

use std::borrow::Cow;

use crate::shuangpin::Scheme;

use super::Engine;

/// 敲出的辅码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FumaCodes {
    /// 只敲了第一码（小写单键）：与「下一个字的声母」有歧义，匹配的候选置顶、其余照常出。
    First(char),

    /// 两码都敲了：已小写、已按反转规则排好。只留匹配的候选。
    Both([char; 2]),
}

impl FumaCodes {
    /// 这个候选文本对不对得上。首码那档只比第一码（词组的第一码是首字的），两码那档要全等。
    pub(super) fn admits(self, expected: [char; 2]) -> bool {
        match self {
            Self::First(code) => expected[0] == code,
            Self::Both(codes) => expected == codes,
        }
    }
}

/// 缓冲区里认出的辅码键。
pub(super) struct FumaInput<'a> {
    /// 喂给双拼解码的键串：两码那档剥掉末 2 键，一个键那档不剥（末键仍按拼音解，简拼词还要出）；
    /// 都已小写化，本来就全小写时不复制。
    pub base: Cow<'a, str>,

    /// 敲出的码。
    pub codes: FumaCodes,
}

impl Engine {
    /// 辅码生效所需的双拼方案：要有表且在双拼下。
    fn fuma_scheme(&self) -> Option<Scheme> {
        self.fuma.as_ref()?;
        self.shuangpin
    }

    /// 缓冲区（或任意键串）里认出的辅码键；没开辅码、不是双拼或条件不满足时为 `None`。
    pub(super) fn fuma_input<'a>(&self, keys: &'a str) -> Option<FumaInput<'a>> {
        let scheme = self.fuma_scheme()?;
        // 末 2 键按字节看：ASCII 字母不可能是多字节字符的一部分，所以字节是字母就等于字符是字母，
        // 也就保证了按字节切出来的前缀落在字符边界上
        let bytes = keys.as_bytes();
        if let [.., first, second] = bytes
            && keys.len() >= 4
        {
            let (first, second) = (*first as char, *second as char);
            if first.is_ascii_alphabetic()
                && second.is_ascii_alphabetic()
                && let Some(codes) = two_key_codes(scheme, first, second)
            {
                let base = lowercased(&keys[..keys.len() - 2]);
                if scheme.decode(&base).is_complete() {
                    return Some(FumaInput { base, codes });
                }
            }
        }
        // 一个键：末键是小写字母、去掉它之后的前缀解成完整双拼。整串仍交给解码（不剥），
        // 末键继续当下一个音节的声母，简拼词与辅码候选并存
        let [.., last] = bytes else {
            return None;
        };
        let last = *last as char;
        if !last.is_ascii_lowercase() || keys.len() < 3 {
            return None;
        }
        if !scheme
            .decode(&lowercased(&keys[..keys.len() - 1]))
            .is_complete()
        {
            return None;
        }
        Some(FumaInput {
            base: lowercased(keys),
            codes: FumaCodes::First(last),
        })
    }

    /// 解码时从末尾剥掉几个字节（一个键那档不剥，见 [`FumaInput::base`]）。
    pub(super) fn fuma_bytes(&self, keys: &str) -> usize {
        match self.fuma_input(keys).map(|input| input.codes) {
            Some(FumaCodes::Both(_)) => 2,
            _ => 0,
        }
    }

    /// 拼音行要补画的辅码段：**只有两码那档**，因为那 2 键被解码剥掉了，不补画就一点痕迹都没有。
    /// 首码那档的键还原样留在拼音里（`ljm` → `lan'm`），再画一遍就重复了。
    pub(super) fn fuma_keys(&self) -> Option<&str> {
        let keys = self.composition.scope();
        match self.fuma_input(keys)?.codes {
            FumaCodes::Both(_) => Some(&keys[keys.len() - 2..]),
            FumaCodes::First(_) => None,
        }
    }

    /// 辅码认到了就返回敲出的码。
    pub(super) fn fuma_codes(&self, keys: &str) -> Option<FumaCodes> {
        self.fuma_input(keys).map(|input| input.codes)
    }

    /// `text` 的期望辅码（词组按「首字第 1 码 + 末字第 1 码」现算）；首末字不在表里为 `None`。
    pub(super) fn fuma_expected(&self, text: &str) -> Option<[char; 2]> {
        self.fuma.as_ref()?.expected_codes(text)
    }

    /// 辅码两码都敲了时 `text` 能不能出候选（严格过滤）；没敲或只敲了首码一律放行
    /// （首码那档靠置顶表达偏好，不排除别的候选）。
    pub(super) fn fuma_admits(&self, text: &str) -> bool {
        let Some(FumaCodes::Both(codes)) = self.fuma_codes(self.composition.scope()) else {
            return true;
        };
        self.fuma_expected(text) == Some(codes)
    }

    /// 辅码功能是否开着（有表、双拼、非注音）：壳据此决定组句中 Shift + 字母进缓冲区还是临时打英文。
    pub fn fuma_enabled(&self) -> bool {
        self.fuma_scheme().is_some()
    }
}

/// 末 2 键能不能当辅码：含大写就是用户明说了（第一键大写表示反转顺序）；
/// 全小写时只有这两键解不成一个完整音节才算（能解成音节的是正常的双音节连打，不抢）。
fn two_key_codes(scheme: Scheme, first: char, second: char) -> Option<FumaCodes> {
    let mut codes = [first.to_ascii_lowercase(), second.to_ascii_lowercase()];
    if first.is_ascii_uppercase() {
        codes.swap(0, 1);
        return Some(FumaCodes::Both(codes));
    }
    if second.is_ascii_uppercase() {
        return Some(FumaCodes::Both(codes));
    }
    let pair: String = [first, second].iter().collect();
    (!scheme.decode(&pair).is_complete()).then_some(FumaCodes::Both(codes))
}

/// 小写化，本来就没有大写时原样借用（辅码判定挂在每次解码上，不能每次都复制一份键串）。
pub(super) fn lowercased(keys: &str) -> Cow<'_, str> {
    if keys.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(keys.to_ascii_lowercase())
    } else {
        Cow::Borrowed(keys)
    }
}
