//! 已退役的「使用帮助」遗留物识别。
//!
//! 应用不再内置用户手册与帮助截图。旧版本会把六张界面截图写进 `images/`
//! （加密工作区里也保持明文），并可能在配置目录留下帮助 PDF。这里只保留
//! 识别这六张随包公开截图的常量，让旧数据的资源迁移与全量备份校验继续放行；
//! 不含正文、不再写文件，也不参与任何界面。

use sha2::{Digest, Sha256};

/// 旧版随包帮助截图。文件名、字节长度与 SHA-256 都与最后一个带帮助的版本一致。
pub(crate) struct RetiredImage {
    pub(crate) name: &'static str,
    pub(crate) len: u64,
    pub(crate) sha256: &'static str,
}

pub(crate) const IMAGES: &[RetiredImage] = &[
    RetiredImage {
        name: "help-ui-elements.png",
        len: 580_162,
        sha256: "fe2f35c764c81f4b2a7068abc93b5dc2d4a168e343cdb31373612e175d6a38a6",
    },
    RetiredImage {
        name: "help-ui-hero.png",
        len: 266_705,
        sha256: "c1dca2ca6b0b3d8c16d73a83d0573f0f2500e3332ca5234e55f53ae6291215e8",
    },
    RetiredImage {
        name: "help-ui-lexicon.png",
        len: 527_138,
        sha256: "242018ef2ab83644be4f267f70144b4e397b69b4c9a6829d4a1f81793ec530fd",
    },
    RetiredImage {
        name: "help-ui-preview-nav.png",
        len: 619_488,
        sha256: "4bdfdb79fba5848275132eae35e56944ebf6c6ddbc967386c9bfb9c2c79741c3",
    },
    RetiredImage {
        name: "help-ui-revision.png",
        len: 632_846,
        sha256: "ab001853cf7a3d7dc4980fc8a7a02693aa86a78c24af6c23ed67aa6de55a5e24",
    },
    RetiredImage {
        name: "help-ui-versions.png",
        len: 337_506,
        sha256: "c476ffabac84b726dc20f19cf644cb33fc98012cf88d39222ba3d7b08215411d",
    },
];

fn image(name: &str) -> Option<&'static RetiredImage> {
    IMAGES.iter().find(|image| image.name == name)
}

/// 文件名是旧帮助截图时返回它的公开字节长度；其它文件返回 `None`。
pub(crate) fn len_of(name: &str) -> Option<u64> {
    image(name).map(|image| image.len)
}

/// 文件名与内容都与旧版随包截图一致时才算公开资源。名称相同不能证明内容公开：
/// 用户或旧版程序可能覆盖过同名文件。
pub(crate) fn matches(name: &str, bytes: &[u8]) -> bool {
    let Some(image) = image(name) else {
        return false;
    };
    bytes.len() as u64 == image.len && sha256_hex(bytes) == image.sha256
}

fn sha256_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len().saturating_mul(2));
    for &byte in digest.iter() {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_help_images_are_matched_by_name_length_and_content() {
        let first = &IMAGES[0];
        assert_eq!(len_of(first.name), Some(first.len));
        assert!(len_of("help-private.png").is_none());
        assert!(!matches(first.name, b"not the bundled screenshot"));
        assert!(!matches("help-private.png", b"not the bundled screenshot"));
    }

    #[test]
    fn sha256_hex_matches_the_standard_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
