//! 读取本应用导出的 Office Agile 文件。核验密文 HMAC 后才交给工作簿解析器。
use aes::Aes256;
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha512};
use std::io::{Cursor, Read};
use zeroize::Zeroizing;

pub(crate) const COMPOUND_MAGIC: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
const CONTENT_KEY: [u8; 8] = [0x14, 0x6e, 0x0b, 0xe7, 0xab, 0xac, 0xd0, 0xd6];
const HMAC_KEY: [u8; 8] = [0x5f, 0xb2, 0xad, 0x01, 0x0c, 0xb9, 0xe1, 0xf6];
const HMAC_VALUE: [u8; 8] = [0xa0, 0x67, 0x7f, 0x02, 0xb2, 0x2c, 0x84, 0x33];
const SEGMENT_BYTES: usize = 4096;
const ENCRYPTION_NS: &str = "http://schemas.microsoft.com/office/2006/encryption";
const PASSWORD_NS: &str = "http://schemas.microsoft.com/office/2006/keyEncryptor/password";

fn decode<const N: usize>(node: roxmltree::Node<'_, '_>, name: &str) -> Result<[u8; N]> {
    let attribute = node.attribute(name).context("Office 加密参数缺失")?;
    let bytes = STANDARD.decode(attribute)?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Office 加密参数长度无效"))
}

fn salt(node: roxmltree::Node<'_, '_>) -> Result<[u8; 16]> {
    for (key, value) in [
        ("cipherAlgorithm", "AES"),
        ("cipherChaining", "ChainingModeCBC"),
        ("hashAlgorithm", "SHA512"),
        ("blockSize", "16"),
        ("keyBits", "256"),
        ("hashSize", "64"),
        ("saltSize", "16"),
    ] {
        ensure!(
            node.attribute(key) == Some(value),
            "此工作簿的 Office 加密参数不受支持"
        );
    }
    decode(node, "saltValue")
}

fn digest(parts: &[&[u8]]) -> [u8; 64] {
    let mut hash = Sha512::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().into()
}

fn decrypt_blocks(bytes: &mut [u8], key: &[u8; 32], iv: &[u8; 16]) -> Result<()> {
    cbc::Decryptor::<Aes256>::new(key.into(), iv.into())
        .decrypt_padded_mut::<NoPadding>(bytes)
        .map_err(|_| anyhow::anyhow!("Office 密文块长度无效"))?;
    Ok(())
}

// Office Agile AES-256/SHA-512。文件上限、加密描述和 KDF 迭代次数均在分配/计算前检查。
pub(crate) fn decrypt(bytes: &[u8], password: &str) -> Result<Zeroizing<Vec<u8>>> {
    let limit = crate::file_limits::MAX_DOCUMENT_BYTES;
    ensure!(bytes.len() as u64 <= limit, "加密工作簿超过大小上限");
    let mut compound = cfb::CompoundFile::open(Cursor::new(bytes)).context("Office 容器无效")?;
    let info = compound
        .open_stream("/EncryptionInfo")
        .context("缺少 Office 加密描述")?;
    ensure!(
        (8..=64 * 1024).contains(&info.len()),
        "Office 加密描述长度无效"
    );
    let mut metadata = Vec::new();
    info.take(64 * 1024).read_to_end(&mut metadata)?;
    ensure!(
        metadata.starts_with(&[4, 0, 4, 0, 0x40, 0, 0, 0]),
        "此工作簿的 Office 加密版本不受支持"
    );
    let xml = roxmltree::Document::parse(std::str::from_utf8(&metadata[8..])?)?;
    let root = xml.root_element();
    ensure!(
        root.has_tag_name((ENCRYPTION_NS, "encryption")),
        "Office 加密描述无效"
    );
    let key_data = root
        .children()
        .find(|node| node.has_tag_name((ENCRYPTION_NS, "keyData")))
        .context("缺少 Office 内容密钥参数")?;
    let integrity = root
        .children()
        .find(|node| node.has_tag_name((ENCRYPTION_NS, "dataIntegrity")))
        .context("缺少 Office 完整性参数")?;
    let password_data = root
        .descendants()
        .find(|node| node.has_tag_name((PASSWORD_NS, "encryptedKey")))
        .context("工作簿未使用受支持的密码加密")?;
    let content_salt = salt(key_data)?;
    let password_salt = salt(password_data)?;
    let spins = password_data
        .attribute("spinCount")
        .context("缺少 Office 密码迭代次数")?
        .parse::<u32>()?;
    ensure!(spins <= 1_000_000, "Office 密码迭代次数超过支持范围");

    let utf16 = Zeroizing::new(
        password
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let mut hash = Zeroizing::new(digest(&[&password_salt, &utf16]));
    for index in 0..spins {
        *hash = digest(&[&index.to_le_bytes(), &hash[..]]);
    }
    let derived = Zeroizing::new(digest(&[&hash[..], &CONTENT_KEY]));
    let mut key = Zeroizing::new(decode::<32>(password_data, "encryptedKeyValue")?);
    decrypt_blocks(&mut key[..], derived[..32].try_into()?, &password_salt)?;

    let iv = |block: &[u8]| -> [u8; 16] {
        let mut iv = [0; 16];
        iv.copy_from_slice(&digest(&[&content_salt, block])[..16]);
        iv
    };
    let mut auth_key = Zeroizing::new(decode::<64>(integrity, "encryptedHmacKey")?);
    let mut expected = Zeroizing::new(decode::<64>(integrity, "encryptedHmacValue")?);
    decrypt_blocks(&mut auth_key[..], &key, &iv(&HMAC_KEY))?;
    decrypt_blocks(&mut expected[..], &key, &iv(&HMAC_VALUE))?;

    let stream = compound
        .open_stream("/EncryptedPackage")
        .context("缺少 Office 密文工作簿")?;
    ensure!(
        (8..=limit).contains(&stream.len()),
        "Office 密文工作簿长度无效"
    );
    let mut payload = Zeroizing::new(Vec::new());
    stream.take(limit).read_to_end(&mut payload)?;
    ensure!(payload.len() >= 8, "Office 密文工作簿头不完整");
    let mut mac = Hmac::<Sha512>::new_from_slice(&auth_key[..])?;
    mac.update(&payload);
    mac.verify_slice(&expected[..])
        .map_err(|_| anyhow::anyhow!("工作簿密码错误或文件已损坏"))?;

    let length = u64::from_le_bytes(payload[..8].try_into()?);
    ensure!(
        length <= limit && length.div_ceil(16) * 16 == (payload.len() - 8) as u64,
        "Office 工作簿声明长度与实际密文不一致"
    );
    for (index, segment) in payload[8..].chunks_mut(SEGMENT_BYTES).enumerate() {
        decrypt_blocks(segment, &key, &iv(&(index as u32).to_le_bytes()))?;
    }
    let length = length as usize;
    payload.copy_within(8..8 + length, 0);
    payload[length..].fill(0);
    payload.truncate(length);
    Ok(payload)
}
