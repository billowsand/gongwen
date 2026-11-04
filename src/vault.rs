//! 应用锁：双密码验证 + 数据槽位绑定。设计见 docs/security-lock-design.md。
//!
//! 核心不变量：**磁盘上不存在任何能区分「一套还是两套密码」的结构**。
//! 锁文件里永远有两个槽位，形状固定；没设密码的槽位放一把随机密钥的哑验证器，
//! 从尺寸到格式都和真验证器一致。密码对不对不靠比对，靠「能不能解开」——
//! 错误提示只能是统一的「密码不正确」，不许出现「没有第二密码」这类区分性文案。

use crate::storage::{self, DataSlot};
use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit};

/// AES-256-GCM 的 nonce 类型（aead 0.6 起 `Nonce<A>` 按密码器的 NonceSize 实例化）。
type CipherNonce = aes_gcm::aead::Nonce<Aes256Gcm>;

/// aead 0.6 里 `from_slice` 已弃用（hybrid-array 改走 `TryFrom`）。
fn nonce_from(bytes: &[u8]) -> Result<CipherNonce> {
    CipherNonce::try_from(bytes).map_err(|_| anyhow::anyhow!("nonce 长度不符"))
}
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use zeroize::Zeroizing;

/// 锁文件在配置目录里的名字：中性，不带语义。
const VAULT_FILE: &str = "vault.dat";
/// 当前格式版本。读到的 `v` 不一致就按损坏处理，不许猜。
const FORMAT_V: u32 = 1;
/// 固定槽位数。槽 0 = 真实系统，槽 1 = 诱饵系统（与 `DataSlot` 顺序一致）。
const SLOT_COUNT: usize = 2;
/// 被密封的固定明文：验证密钥对不对就靠解它，明文本身不含任何槽位信息。
const SEAL_PLAINTEXT: &[u8] = b"gongwen-vault-slot-v1";
const KEY_LEN: usize = 32;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
/// 密码最短长度（按 UTF-8 字节计）。
const MIN_PASSWORD_LEN: usize = 4;
const MAX_PASSWORD_LEN: usize = 256;

/// Argon2id 参数：64 MiB / 3 遍 / 4 线程，台式机约半秒一次。
/// 只在解锁与改密时跑，不在每帧路径上；参数写死，改参数等于换格式。
const ARGON_M_COST: u32 = 64 * 1024;
const ARGON_T_COST: u32 = 3;
const ARGON_P_COST: u32 = 4;

/// 一个槽位的验证器。`salt` 是 hex(16 字节盐)；`seal` 是
/// hex(12 字节 nonce ‖ 密文+16 字节认证标签)。两者都空 = 「明确跳过」文件
/// 里的空槽；真锁文件里永远两个槽都非空（哑槽也是密文形状）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlotSeal {
    salt: String,
    seal: String,
}

impl SlotSeal {
    fn empty() -> Self {
        Self {
            salt: String::new(),
            seal: String::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.salt.is_empty() && self.seal.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultData {
    v: u32,
    slots: [SlotSeal; SLOT_COUNT],
}

/// 启动探测结果：闸门据此决定进引导、进锁屏还是直接开工作区。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Probe {
    /// 没有锁文件：首次启动，弹设置引导。
    Absent,
    /// 有锁文件但两个槽都是空的：用户明确选过「暂不设置」。
    Skipped,
    /// 正常待解锁。
    Locked,
    /// 锁文件损坏：原文件保留，提示重置。
    Broken(String),
}

fn slot_index(slot: DataSlot) -> usize {
    match slot {
        DataSlot::Real => 0,
        DataSlot::Decoy => 1,
    }
}

/// 随机字节取自 uuid v4（getrandom 驱动），不另引 rand。
fn random_bytes_16() -> [u8; 16] {
    *uuid::Uuid::new_v4().as_bytes()
}

fn derive_key(password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let params = argon2::Params::new(ARGON_M_COST, ARGON_T_COST, ARGON_P_COST, Some(KEY_LEN))
        .context("Argon2 参数无效")?;
    let argon2 = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    argon2
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .context("密钥派生失败")?;
    Ok(key)
}

/// 用 `password` 生成一个新验证器：新盐、新密钥、新 nonce。
fn make_seal(password: &str) -> Result<SlotSeal> {
    let salt = random_bytes_16();
    let key = derive_key(password, &salt)?;
    seal_with_key(&salt, &key)
}

/// 哑验证器：随机密钥封同一段固定明文。尺寸、格式与真验证器完全一致，
/// 解开它的密码不存在——这正是「看不出有没有第二密码」的关键。
fn dummy_seal() -> Result<SlotSeal> {
    let salt = random_bytes_16();
    // 32 字节密钥由两段独立随机数拼成，别用重复段凑长度。
    let head = random_bytes_16();
    let tail = random_bytes_16();
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    key[..16].copy_from_slice(&head);
    key[16..].copy_from_slice(&tail);
    seal_with_key(&salt, &key)
}

fn seal_with_key(salt: &[u8; SALT_LEN], key: &[u8; KEY_LEN]) -> Result<SlotSeal> {
    let cipher = Aes256Gcm::new(key.into());
    let nonce_bytes = random_bytes_16();
    let nonce = nonce_from(&nonce_bytes[..NONCE_LEN])?;
    let ciphertext = cipher
        .encrypt(&nonce, SEAL_PLAINTEXT)
        .context("验证器密封失败")?;
    let mut sealed = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    sealed.extend_from_slice(&nonce_bytes[..NONCE_LEN]);
    sealed.extend_from_slice(&ciphertext);
    Ok(SlotSeal {
        salt: hex_encode(salt),
        seal: hex_encode(&sealed),
    })
}

/// 密码对不对不靠比对，靠「能不能解开这个槽的验证器」。
fn try_seal(password: &str, slot: &SlotSeal) -> bool {
    if slot.is_empty() {
        return false;
    }
    let Ok(salt) = hex_decode::<SALT_LEN>(&slot.salt) else {
        return false;
    };
    let Ok(sealed) = hex_decode_vec(&slot.seal) else {
        return false;
    };
    if sealed.len() <= NONCE_LEN {
        return false;
    }
    let Ok(key) = derive_key(password, &salt) else {
        return false;
    };
    let cipher = Aes256Gcm::new((&*key).into());
    let Ok(nonce) = nonce_from(&sealed[..NONCE_LEN]) else {
        return false;
    };
    cipher
        .decrypt(&nonce, &sealed[NONCE_LEN..])
        .is_ok_and(|plain| plain == SEAL_PLAINTEXT)
}

fn check_password(password: &str) -> Result<()> {
    let len = password.len();
    if len < MIN_PASSWORD_LEN {
        bail!("密码至少 {MIN_PASSWORD_LEN} 个字符");
    }
    if len > MAX_PASSWORD_LEN {
        bail!("密码过长（最多 {MAX_PASSWORD_LEN} 个字符）");
    }
    Ok(())
}

/// 已加载的锁。所有改写都整文件原子替换，写一半不会留下「读得到但不完整」。
pub(crate) struct Vault {
    data: VaultData,
}

impl Vault {
    fn path() -> Result<PathBuf> {
        Ok(storage::config_dir()?.join(VAULT_FILE))
    }

    /// 启动探测：不持有状态，一次廉价的文件读。
    pub(crate) fn probe() -> Probe {
        let Ok(path) = Self::path() else {
            return Probe::Broken("无法确定配置目录".into());
        };
        if !path.exists() {
            return Probe::Absent;
        }
        match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<VaultData>(&raw) {
                Ok(data) if data.v != FORMAT_V => Probe::Broken("锁文件版本不识别".into()),
                Ok(data) if data.slots.iter().all(SlotSeal::is_empty) => Probe::Skipped,
                Ok(_) => Probe::Locked,
                Err(error) => Probe::Broken(format!("锁文件内容损坏：{error}")),
            },
            Err(error) => Probe::Broken(format!("锁文件读不了：{error}")),
        }
    }

    /// 启用锁：写主密码，第二密码可选（缺省写哑验证器）。
    /// `skipped = true` 写空槽文件（用户选「暂不设置」）。
    pub(crate) fn create(real: &str, decoy: Option<&str>, skipped: bool) -> Result<Self> {
        let data = if skipped {
            VaultData {
                v: FORMAT_V,
                slots: [SlotSeal::empty(), SlotSeal::empty()],
            }
        } else {
            check_password(real)?;
            let real_seal = make_seal(real)?;
            let decoy_seal = match decoy {
                Some(password) => {
                    if password == real {
                        bail!("两个密码不能相同");
                    }
                    check_password(password)?;
                    make_seal(password)?
                }
                None => dummy_seal()?,
            };
            VaultData {
                v: FORMAT_V,
                slots: [real_seal, decoy_seal],
            }
        };
        let vault = Self { data };
        vault.save()?;
        Ok(vault)
    }

    fn save(&self) -> Result<()> {
        let path = Self::path()?;
        let raw = serde_json::to_vec_pretty(&self.data).context("锁文件序列化失败")?;
        storage::atomic_write(&path, |file| {
            use std::io::Write as _;
            file.write_all(&raw)?;
            Ok(())
        })
        .context("锁文件保存失败")
    }

    /// 加载已有锁。调用前先 `probe()`；文件缺失或损坏时返回 Err。
    pub(crate) fn load() -> Result<Self> {
        let path = Self::path()?;
        let raw = std::fs::read_to_string(&path).context("锁文件不存在")?;
        let data: VaultData = serde_json::from_str(&raw).context("锁文件损坏")?;
        if data.v != FORMAT_V {
            bail!("锁文件版本不识别");
        }
        Ok(Self { data })
    }

    /// 试两个槽，命中哪个返回哪个槽位；都不中返回 None。
    /// 顺序固定（先真实后诱饵），文案层不区分失败原因。
    pub(crate) fn unlock(&self, password: &str) -> Option<DataSlot> {
        for (index, slot) in self.data.slots.iter().enumerate() {
            if try_seal(password, slot) {
                return Some(if index == 0 {
                    DataSlot::Real
                } else {
                    DataSlot::Decoy
                });
            }
        }
        None
    }

    /// 改某个槽位的密码（重盐重封）。第二密码与主密码相同则拒绝。
    pub(crate) fn set_password(&mut self, slot: DataSlot, password: &str) -> Result<()> {
        check_password(password)?;
        let index = slot_index(slot);
        if index == 1 && try_seal(password, &self.data.slots[0]) {
            bail!("第二密码不能和主密码相同");
        }
        self.data.slots[index] = make_seal(password)?;
        self.save()
    }

    /// 清除第二密码：写成哑验证器。不知道之前是否设置过也能安全执行——
    /// 这就是不设状态标记的原因。
    pub(crate) fn clear_decoy(&mut self) -> Result<()> {
        self.data.slots[1] = dummy_seal()?;
        self.save()
    }

    /// 停用：删锁文件。数据文件（两个槽位的库与配置）原样保留，
    /// 重新启用后主密码接管真实文件，第二密码从头开始。
    pub(crate) fn disable(&self) -> Result<()> {
        Self::reset_file()
    }

    /// 删除锁文件（停用与「锁设置损坏」清除共用）。只删这一份，
    /// 配置目录里其它任何文件都不碰。
    pub(crate) fn reset_file() -> Result<()> {
        let path = Self::path()?;
        if path.exists() {
            std::fs::remove_file(&path).context("删除锁文件失败")?;
        }
        Ok(())
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode<const N: usize>(text: &str) -> Result<[u8; N]> {
    let bytes = hex_decode_vec(text)?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("hex 长度不符"))
}

fn hex_decode_vec(text: &str) -> Result<Vec<u8>> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        bail!("hex 长度为奇数");
    }
    let value = |c: u8| -> Result<u8> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => bail!("hex 含非法字符"),
        }
    };
    let mut out = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks_exact(2) {
        out.push((value(pair[0])? << 4) | value(pair[1])?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn use_temp_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        dir
    }

    fn reset() {
        crate::storage::set_test_config_dir(None);
    }

    #[test]
    fn absent_then_skipped_then_locked_probe() {
        let dir = use_temp_dir();
        assert_eq!(Vault::probe(), Probe::Absent);
        Vault::create("", None, true).unwrap();
        assert_eq!(Vault::probe(), Probe::Skipped);
        Vault::create("主密码", None, false).unwrap();
        assert_eq!(Vault::probe(), Probe::Locked);
        drop(dir);
        reset();
    }

    #[test]
    fn real_and_decoy_passwords_open_their_own_slots() {
        let dir = use_temp_dir();
        let vault = Vault::create("真实口令abc", Some("诱饵口令xyz"), false).unwrap();
        assert_eq!(vault.unlock("真实口令abc"), Some(DataSlot::Real));
        assert_eq!(vault.unlock("诱饵口令xyz"), Some(DataSlot::Decoy));
        assert_eq!(vault.unlock("不对"), None);
        drop(dir);
        reset();
    }

    #[test]
    fn wrong_passwords_are_indistinguishable_from_missing_slots() {
        // 没有第二密码的锁：错的主密码、对的哑槽密码（不存在）、随机串——
        // 解锁结果一律是 None，UI 只能给出统一文案。
        let dir = use_temp_dir();
        let vault = Vault::create("只有主密码", None, false).unwrap();
        assert_eq!(vault.unlock("只有主密码"), Some(DataSlot::Real));
        assert_eq!(vault.unlock("任意猜测"), None);
        drop(dir);
        reset();
    }

    #[test]
    fn seal_shapes_are_constant_whether_or_not_decoy_is_set() {
        let a = use_temp_dir();
        Vault::create("主密码aaaa", None, false).unwrap();
        let without = std::fs::read_to_string(Vault::path().unwrap()).unwrap();
        reset();
        drop(a);

        let b = use_temp_dir();
        Vault::create("主密码aaaa", Some("第二密码bbbb"), false).unwrap();
        let with = std::fs::read_to_string(Vault::path().unwrap()).unwrap();
        reset();
        drop(b);

        assert_eq!(
            without.len(),
            with.len(),
            "设不设第二密码，文件尺寸必须一致"
        );
        let parse = |raw: &str| serde_json::from_str::<VaultData>(raw).unwrap();
        let (wa, wb) = (parse(&without), parse(&with));
        for index in 0..SLOT_COUNT {
            assert_eq!(wa.slots[index].salt.len(), wb.slots[index].salt.len());
            assert_eq!(wa.slots[index].seal.len(), wb.slots[index].seal.len());
        }
    }

    #[test]
    fn dummy_seal_slot_survives_clear_and_readd() {
        let dir = use_temp_dir();
        let mut vault = Vault::create("主密码aaaa", Some("第二密码bbbb"), false).unwrap();
        assert_eq!(vault.unlock("第二密码bbbb"), Some(DataSlot::Decoy));
        vault.clear_decoy().unwrap();
        assert_eq!(vault.unlock("第二密码bbbb"), None, "清除后旧第二密码失效");
        assert_eq!(vault.unlock("主密码aaaa"), Some(DataSlot::Real));
        vault.set_password(DataSlot::Decoy, "新第二密码cc").unwrap();
        assert_eq!(vault.unlock("新第二密码cc"), Some(DataSlot::Decoy));
        drop(dir);
        reset();
    }

    #[test]
    fn decoy_password_cannot_equal_real_password() {
        let dir = use_temp_dir();
        let mut vault = Vault::create("同一个密码", None, false).unwrap();
        assert!(vault.set_password(DataSlot::Decoy, "同一个密码").is_err());
        drop(dir);
        reset();
    }

    #[test]
    fn create_rejects_same_password_pair_and_short_passwords() {
        let dir = use_temp_dir();
        assert!(Vault::create("主密码aaaa", Some("主密码aaaa"), false).is_err());
        assert!(Vault::create("短", None, false).is_err());
        drop(dir);
        reset();
    }

    #[test]
    fn disable_removes_the_vault_file_and_keeps_data_files() {
        let dir = use_temp_dir();
        let vault = Vault::create("主密码aaaa", None, false).unwrap();
        std::fs::write(
            crate::storage::manuscript_db_path().unwrap(),
            b"fake db bytes",
        )
        .unwrap();
        vault.disable().unwrap();
        assert_eq!(Vault::probe(), Probe::Absent);
        assert!(
            crate::storage::manuscript_db_path().unwrap().exists(),
            "停用不动数据文件"
        );
        drop(dir);
        reset();
    }

    #[test]
    fn corrupted_vault_file_reports_broken_and_never_unlocks() {
        let dir = use_temp_dir();
        Vault::create("主密码aaaa", None, false).unwrap();
        std::fs::write(Vault::path().unwrap(), b"{ not json").unwrap();
        let probe = Vault::probe();
        assert!(matches!(probe, Probe::Broken(_)));
        // 损坏文件不许猜着解锁：load 必须失败，原文件保留待用户处置。
        assert!(Vault::load().is_err());
        assert!(Vault::path().unwrap().exists());
        drop(dir);
        reset();
    }

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        let bytes = [0u8, 1, 0xab, 0xff, 0x30];
        let text = hex_encode(&bytes);
        assert_eq!(hex_decode::<5>(&text).unwrap(), bytes);
        assert!(hex_decode::<4>(&text).is_err(), "长度不符要报错");
        assert!(hex_decode_vec("abc").is_err(), "奇数长度要报错");
        assert!(hex_decode_vec("zz").is_err(), "非法字符要报错");
    }
}
