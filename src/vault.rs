//! 应用锁：双密码验证、数据密钥（DEK）句柄、文件 AEAD 与明文⇄密文迁移。
//! 设计见 docs/security-lock-design.md。
//!
//! 核心不变量：**磁盘上不存在任何能区分「一套还是两套密码」的结构**。
//! 锁文件里永远有两个槽位，形状固定；没设密码的槽位放一把随机密钥的哑验证器，
//! 从尺寸到格式都和真验证器一致。密码对不对不靠比对，靠「能不能解开」——
//! 错误提示只能是统一的「密码不正确」，不许出现「没有第二密码」这类区分性文案。
//!
//! 密钥体系（第二期起）：
//! - 密码经 Argon2id 派生「包裹密钥」，只用于解开槽里的验证器和**包裹的数据密钥**；
//! - 数据密钥（DEK）是每槽一把的随机 32 字节，真正的 manuscripts.db（SQLCipher）
//!   与 config.json 等文件的加密都走它。**改密码 = 换包裹，不碰数据文件**——
//!   这是与方案文档「密码 → 密钥 → 一切」的唯一出入，换来的是改密零成本、
//!   且绝不可能出现「新密码解不开旧库」的自锁。

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
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use zeroize::Zeroizing;

/// 锁文件在配置目录里的名字：中性，不带语义。
const VAULT_FILE: &str = "vault.dat";
/// 当前格式版本。读到的 `v` 不认识就按损坏处理，不许猜。
const FORMAT_V: u32 = 2;
/// 第一期开发期格式（v1，槽里只有验证器、没有包裹 DEK）。只在开发期存在过、
/// 未随发布流出；解锁成功时无损升级到 v2（补一把全新的 DEK）。
const LEGACY_V: u32 = 1;
/// 固定槽位数。槽 0 = 真实系统，槽 1 = 诱饵系统（与 `DataSlot` 顺序一致）。
const SLOT_COUNT: usize = 2;
/// 被密封的固定明文：验证密钥对不对就靠解它，明文本身不含任何槽位信息。
const SEAL_PLAINTEXT: &[u8] = b"gongwen-vault-slot-v1";
const KEY_LEN: usize = 32;
const DEK_LEN: usize = 32;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
/// AEAD 认证标签长度（AES-256-GCM 固定 16 字节）。
const TAG_LEN: usize = 16;
/// 密码最短长度（按 UTF-8 字节计）。
const MIN_PASSWORD_LEN: usize = 4;
const MAX_PASSWORD_LEN: usize = 256;

/// Argon2id 参数：64 MiB / 3 遍 / 4 线程，台式机约半秒一次。
/// 只在解锁与改密时跑，不在每帧路径上；参数写死，改参数等于换格式。
const ARGON_M_COST: u32 = 64 * 1024;
const ARGON_T_COST: u32 = 3;
const ARGON_P_COST: u32 = 4;

/// 一个槽位的验证器与数据密钥包裹。`salt` 是 hex(16 字节盐)；`seal` 是
/// hex(12 字节 nonce ‖ 密文+标签)；`wrapped` 是 hex(12 字节 nonce ‖
/// AEAD(包裹密钥, DEK)+标签)。三个都空 = 「明确跳过」文件里的空槽；
/// v2 锁文件里两个槽都非空（哑槽也是密文形状，尺寸一致）；v1 老文件的槽
/// 没有 wrapped 字段，解锁命中时才懒升级补写。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlotSeal {
    salt: String,
    seal: String,
    /// v1 锁文件没有这一字段：serde 默认空串，解锁成功时升级补写。
    #[serde(default)]
    wrapped: String,
}

impl SlotSeal {
    fn empty() -> Self {
        Self {
            salt: String::new(),
            seal: String::new(),
            wrapped: String::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.salt.is_empty() && self.seal.is_empty() && self.wrapped.is_empty()
    }

    /// 哑槽：随机包裹密钥封固定明文、裹随机 DEK。尺寸、格式与真槽完全一致，
    /// 解开它的密码不存在——这正是「看不出有没有第二密码」的关键。
    fn dummy() -> Result<Self> {
        let salt = random_bytes_16();
        let mut wrap_key = Zeroizing::new([0u8; KEY_LEN]);
        wrap_key[..16].copy_from_slice(&random_bytes_16());
        wrap_key[16..].copy_from_slice(&random_bytes_16());
        let dek = random_dek()?;
        Self::build(&salt, &wrap_key, &dek)
    }

    /// 真槽：`password` 派生包裹密钥，封验证器、裹 `dek`（没有则现生成一把）。
    fn sealed(password: &str, dek: Option<&[u8; DEK_LEN]>) -> Result<(Self, [u8; DEK_LEN])> {
        let generated = match dek {
            Some(_) => None,
            None => Some(random_dek()?),
        };
        let dek = dek.or(generated.as_ref()).context("数据密钥缺失")?;
        let salt = random_bytes_16();
        let wrap_key = derive_key(password, &salt)?;
        let slot = Self::build(&salt, &wrap_key, dek)?;
        Ok((slot, *dek))
    }

    fn build(salt: &[u8; SALT_LEN], wrap_key: &[u8; KEY_LEN], dek: &[u8; DEK_LEN]) -> Result<Self> {
        let cipher = Aes256Gcm::new(wrap_key.into());
        let seal_nonce = random_nonce()?;
        let seal = cipher
            .encrypt(&seal_nonce, SEAL_PLAINTEXT)
            .context("验证器密封失败")?;
        let wrap_nonce = random_nonce()?;
        let wrapped = cipher
            .encrypt(&wrap_nonce, &dek[..])
            .context("数据密钥包裹失败")?;
        let mut seal_bytes = Vec::with_capacity(NONCE_LEN + seal.len());
        seal_bytes.extend_from_slice(&seal_nonce);
        seal_bytes.extend_from_slice(&seal);
        let mut wrapped_bytes = Vec::with_capacity(NONCE_LEN + wrapped.len());
        wrapped_bytes.extend_from_slice(&wrap_nonce);
        wrapped_bytes.extend_from_slice(&wrapped);
        Ok(SlotSeal {
            salt: hex_encode(salt),
            seal: hex_encode(&seal_bytes),
            wrapped: hex_encode(&wrapped_bytes),
        })
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
    /// 正常待解锁（v1 老格式也算：解锁成功时原地升级 v2）。
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

fn index_slot(index: usize) -> DataSlot {
    if index == 1 {
        DataSlot::Decoy
    } else {
        DataSlot::Real
    }
}

/// 随机字节取自 uuid v4（getrandom 驱动），不另引 rand。
fn random_bytes_16() -> [u8; 16] {
    *uuid::Uuid::new_v4().as_bytes()
}

fn random_nonce() -> Result<CipherNonce> {
    nonce_from(&random_bytes_16()[..NONCE_LEN])
}

/// 一把新的随机数据密钥。
fn random_dek() -> Result<[u8; DEK_LEN]> {
    let head = random_bytes_16();
    let tail = random_bytes_16();
    let mut dek = [0u8; DEK_LEN];
    dek[..16].copy_from_slice(&head);
    dek[16..].copy_from_slice(&tail);
    Ok(dek)
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

/// 试解一个槽：验证器解得开就返回包裹密钥；解不开（密码错 / 哑槽 / 形状坏）
/// 一律 None——上层不允许区分原因。
fn try_open_slot(password: &str, slot: &SlotSeal) -> Option<Zeroizing<[u8; KEY_LEN]>> {
    if slot.is_empty() {
        return None;
    }
    let salt = hex_decode::<SALT_LEN>(&slot.salt).ok()?;
    let sealed = hex_decode_vec(&slot.seal).ok()?;
    if sealed.len() <= NONCE_LEN {
        return None;
    }
    let wrap_key = derive_key(password, &salt).ok()?;
    let cipher = Aes256Gcm::new((&*wrap_key).into());
    let nonce = nonce_from(&sealed[..NONCE_LEN]).ok()?;
    let plain = cipher.decrypt(&nonce, &sealed[NONCE_LEN..]).ok()?;
    (plain == SEAL_PLAINTEXT).then_some(wrap_key)
}

/// 用包裹密钥解开槽里的数据密钥。
fn unwrap_dek(slot: &SlotSeal, wrap_key: &[u8; KEY_LEN]) -> Result<[u8; DEK_LEN]> {
    let wrapped = hex_decode_vec(&slot.wrapped).context("数据密钥包裹损坏")?;
    anyhow::ensure!(
        wrapped.len() == NONCE_LEN + DEK_LEN + TAG_LEN,
        "数据密钥包裹长度不符"
    );
    let cipher = Aes256Gcm::new(wrap_key.into());
    let nonce = nonce_from(&wrapped[..NONCE_LEN])?;
    let plain = cipher
        .decrypt(&nonce, &wrapped[NONCE_LEN..])
        .context("数据密钥解不开")?;
    plain
        .try_into()
        .map_err(|_| anyhow::anyhow!("数据密钥长度不符"))
}

/// 解锁结果：命中哪个槽、以及该槽的数据密钥。
pub(crate) struct Unlocked {
    pub(crate) slot: DataSlot,
    pub(crate) dek: Zeroizing<[u8; DEK_LEN]>,
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
                Ok(data) if data.v != FORMAT_V && data.v != LEGACY_V => {
                    Probe::Broken("锁文件版本不识别".into())
                }
                Ok(data) if data.slots.iter().all(SlotSeal::is_empty) => Probe::Skipped,
                Ok(_) => Probe::Locked,
                Err(error) => Probe::Broken(format!("锁文件内容损坏：{error}")),
            },
            Err(error) => Probe::Broken(format!("锁文件读不了：{error}")),
        }
    }

    /// 启用锁：写主密码，第二密码可选（缺省写哑槽）。
    /// `skipped = true` 写空槽文件（用户选「暂不设置」）。
    /// 返回 (锁, 主槽的解锁结果)；skipped 时后者为 None。
    pub(crate) fn create(
        real: &str,
        decoy: Option<&str>,
        skipped: bool,
    ) -> Result<(Self, Option<Unlocked>)> {
        let (data, unlocked) = if skipped {
            (
                VaultData {
                    v: FORMAT_V,
                    slots: [SlotSeal::empty(), SlotSeal::empty()],
                },
                None,
            )
        } else {
            check_password(real)?;
            let (real_seal, real_dek) = SlotSeal::sealed(real, None)?;
            let decoy_seal = match decoy {
                Some(password) => {
                    if password == real {
                        bail!("两个密码不能相同");
                    }
                    check_password(password)?;
                    SlotSeal::sealed(password, None)?.0
                }
                None => SlotSeal::dummy()?,
            };
            (
                VaultData {
                    v: FORMAT_V,
                    slots: [real_seal, decoy_seal],
                },
                Some(Unlocked {
                    slot: DataSlot::Real,
                    dek: Zeroizing::new(real_dek),
                }),
            )
        };
        let vault = Self { data };
        vault.save()?;
        Ok((vault, unlocked))
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
        if data.v != FORMAT_V && data.v != LEGACY_V {
            bail!("锁文件版本不识别");
        }
        Ok(Self { data })
    }

    /// 试两个槽，命中哪个返回哪个槽位与数据密钥；都不中返回 None。
    /// 顺序固定（先真实后诱饵），文案层不区分失败原因。
    /// v1 老格式命中时原地升级 v2：给命中槽补一把全新 DEK 并裹好写回
    ///（v1 时代还没有加密数据，新 DEK 不会解不开任何已有密文；
    /// 只补命中那一个槽，另一把密码的槽等自己的密码解锁时再升级）。
    pub(crate) fn unlock(&mut self, password: &str) -> Option<Unlocked> {
        for index in 0..self.data.slots.len() {
            let Some(wrap_key) = try_open_slot(password, &self.data.slots[index]) else {
                continue;
            };
            if self.data.slots[index].wrapped.is_empty() {
                // v1 → v2 升级：现生成 DEK、裹进当前槽，写回失败下次再补。
                let dek = random_dek().ok()?;
                if let Ok(wrapped) = wrap_dek(&wrap_key, &dek) {
                    self.data.slots[index].wrapped = wrapped;
                    self.data.v = FORMAT_V;
                    let _ = self.save();
                }
                return Some(Unlocked {
                    slot: index_slot(index),
                    dek: Zeroizing::new(dek),
                });
            }
            let Ok(dek) = unwrap_dek(&self.data.slots[index], &wrap_key) else {
                return None;
            };
            return Some(Unlocked {
                slot: index_slot(index),
                dek: Zeroizing::new(dek),
            });
        }
        None
    }

    /// 改某个槽位的密码（重盐、重封验证器、重裹数据密钥）。
    /// - 主密码必须传入当前 DEK（调用方从密钥句柄取）：改密只换包裹，
    ///   数据文件一个都不动；
    /// - 第二密码总是生成全新 DEK——等于重置第二密码系统，旧诱饵数据
    ///   随之不可读，调用方负责先擦诱饵文件；
    /// - 第二密码与主密码相同则拒绝。返回新的解锁结果（主槽时 DEK 不变）。
    pub(crate) fn set_password(
        &mut self,
        slot: DataSlot,
        password: &str,
        current_dek: Option<&[u8; DEK_LEN]>,
    ) -> Result<Unlocked> {
        check_password(password)?;
        let index = slot_index(slot);
        if index == 1 {
            // 借用检查：新密码不能恰好也是主密码。
            if try_open_slot(password, &self.data.slots[0]).is_some() {
                bail!("第二密码不能和主密码相同");
            }
        }
        let (new_slot, dek) = if index == 0 {
            let dek = current_dek.context("改主密码需要当前数据密钥")?;
            SlotSeal::sealed(password, Some(dek))?
        } else {
            SlotSeal::sealed(password, None)?
        };
        self.data.slots[index] = new_slot;
        self.save()?;
        Ok(Unlocked {
            slot,
            dek: Zeroizing::new(dek),
        })
    }

    /// 清除第二密码：写成哑槽，并擦掉诱饵槽位的全部数据文件
    ///（旧诱饵数据用旧 DEK 加密，DEK 随哑槽丢弃，本来就不可读，擦掉更干净）。
    pub(crate) fn clear_decoy(&mut self) -> Result<()> {
        self.data.slots[1] = SlotSeal::dummy()?;
        self.save()?;
        storage::wipe_slot_files(DataSlot::Decoy);
        Ok(())
    }

    /// 删除锁文件（停用完成与「锁设置损坏」清除共用）。只删这一份，
    /// 配置目录里其它任何文件都不碰。
    pub(crate) fn reset_file() -> Result<()> {
        let path = Self::path()?;
        if path.exists() {
            std::fs::remove_file(&path).context("删除锁文件失败")?;
        }
        Ok(())
    }
}

/// 用包裹密钥把 DEK 裹成槽里的 `wrapped` 字段形状（hex）。
fn wrap_dek(wrap_key: &[u8; KEY_LEN], dek: &[u8; DEK_LEN]) -> Result<String> {
    let cipher = Aes256Gcm::new(wrap_key.into());
    let nonce = random_nonce()?;
    let wrapped = cipher
        .encrypt(&nonce, &dek[..])
        .context("数据密钥包裹失败")?;
    let mut bytes = Vec::with_capacity(NONCE_LEN + wrapped.len());
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&wrapped);
    Ok(hex_encode(&bytes))
}

// ── 进程级数据密钥句柄 ────────────────────────────────────────────────

/// 解锁后的数据密钥句柄：SQLCipher PRAGMA key 与文件 AEAD 都从这里取。
/// `db_active` 区分「会话内 DB 是否已按密文开」——设置页刚启用密码保护时，
/// 磁盘上的库还是明文（迁移安排在下次解锁），本会话必须继续按明文开库。
pub(crate) struct KeyHandle {
    dek: Zeroizing<[u8; DEK_LEN]>,
    db_active: bool,
}

impl KeyHandle {
    /// SQLCipher `PRAGMA hexkey` 用的 hex 串（不带引号）。
    pub(crate) fn dek_hex(&self) -> String {
        hex_encode(&self.dek[..])
    }

    pub(crate) fn db_active(&self) -> bool {
        self.db_active
    }

    pub(crate) fn dek(&self) -> &[u8; DEK_LEN] {
        &self.dek
    }
}

static KEY_HANDLE: RwLock<Option<KeyHandle>> = RwLock::new(None);

// 测试按线程覆盖句柄：测试并行跑，全局句柄会被先跑完的测试的
// `reset()` 清掉（同 `storage::TEST_CONFIG_DIR` 的思路，见那里注释）。
#[cfg(test)]
thread_local! {
    static TEST_KEY_HANDLE: std::cell::RefCell<Option<KeyHandle>> =
        const { std::cell::RefCell::new(None) };
}

fn clone_handle(handle: &KeyHandle) -> KeyHandle {
    KeyHandle {
        dek: Zeroizing::new(*handle.dek),
        db_active: handle.db_active,
    }
}

pub(crate) fn key_handle() -> Option<KeyHandle> {
    #[cfg(test)]
    if let Some(handle) = TEST_KEY_HANDLE.with(|cell| cell.borrow().as_ref().map(clone_handle)) {
        return Some(handle);
    }
    KEY_HANDLE
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(clone_handle))
}

pub(crate) fn set_key_handle(dek: [u8; DEK_LEN], db_active: bool) {
    let handle = Some(KeyHandle {
        dek: Zeroizing::new(dek),
        db_active,
    });
    #[cfg(test)]
    TEST_KEY_HANDLE.with(|cell| *cell.borrow_mut() = handle);
    #[cfg(not(test))]
    if let Ok(mut guard) = KEY_HANDLE.write() {
        *guard = handle;
    }
}

/// 迁移完成后把句柄切到「按密文开库」。
pub(crate) fn activate_db_encryption() {
    #[cfg(test)]
    TEST_KEY_HANDLE.with(|cell| {
        if let Some(handle) = cell.borrow_mut().as_mut() {
            handle.db_active = true;
        }
    });
    #[cfg(not(test))]
    if let Ok(mut guard) = KEY_HANDLE.write()
        && let Some(handle) = guard.as_mut()
    {
        handle.db_active = true;
    }
}

pub(crate) fn clear_key_handle() {
    #[cfg(test)]
    TEST_KEY_HANDLE.with(|cell| *cell.borrow_mut() = None);
    #[cfg(not(test))]
    if let Ok(mut guard) = KEY_HANDLE.write() {
        *guard = None;
    }
}

// ── 文件 AEAD（config.json、密钥文件等整份加密） ─────────────────────

/// 加密文件的头：魔数 + 版本，明文文件永远不会以这四个字节开头。
pub(crate) const FILE_MAGIC: &[u8; 4] = b"GWA1";

pub(crate) fn is_sealed_file(bytes: &[u8]) -> bool {
    bytes.starts_with(FILE_MAGIC)
}

/// 整份加密：MAGIC ‖ nonce ‖ AEAD(DEK, 明文)。
pub(crate) fn seal_bytes(dek: &[u8; DEK_LEN], plain: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(dek.into());
    let nonce = random_nonce()?;
    let ciphertext = cipher.encrypt(&nonce, plain).context("文件加密失败")?;
    let mut out = Vec::with_capacity(FILE_MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(FILE_MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// 整份解密。不是密封格式、或认证失败，都返回 Err（上层按损坏处理）。
pub(crate) fn open_bytes(dek: &[u8; DEK_LEN], sealed: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(is_sealed_file(sealed), "不是加密文件");
    anyhow::ensure!(
        sealed.len() >= FILE_MAGIC.len() + NONCE_LEN + TAG_LEN,
        "加密文件长度不符"
    );
    let cipher = Aes256Gcm::new(dek.into());
    let nonce = nonce_from(&sealed[FILE_MAGIC.len()..FILE_MAGIC.len() + NONCE_LEN])?;
    cipher
        .decrypt(&nonce, &sealed[FILE_MAGIC.len() + NONCE_LEN..])
        .context("文件解密失败（密钥不符或内容损坏）")
}

/// 需要 AEAD 保护的配置文件名单（相对配置目录、按当前槽位映射）。
/// 都在 `slot_file` 入口内；数据库不在这份名单里（走 SQLCipher 迁移）。
pub(crate) const SEALED_FILE_NAMES: [&str; 3] =
    ["config.json", ".zip-password", "api-secrets.json"];

// ── 明文 ⇄ 密文迁移 ─────────────────────────────────────────────────

/// 解锁后为当前槽位做的数据迁移：库不够密文就整体搬进密文库，小文件逐个密封。
/// 幂等：已是密文就直接返回。任何一步失败返回 Err，由闸门翻译成提示。
pub(crate) fn ensure_slot_encrypted() -> Result<()> {
    let key = key_dek()?;
    migrate_database(&storage::manuscript_db_path()?, Migration::ToEncrypted)?;
    for name in SEALED_FILE_NAMES {
        let path = storage::slot_file(name)?;
        if path.is_file() && !is_sealed_file(&std::fs::read(&path)?) {
            let plain =
                std::fs::read(&path).with_context(|| format!("读取 {} 失败", path.display()))?;
            let sealed = seal_bytes(&key, &plain)?;
            storage::atomic_write(&path, |file| {
                use std::io::Write as _;
                file.write_all(&sealed)?;
                Ok(())
            })?;
        }
    }
    Ok(())
}

/// 停用的最后一步：把当前槽位的密文数据解回明文（库导出、小文件解封）。
pub(crate) fn decrypt_slot_files() -> Result<()> {
    migrate_database(&storage::manuscript_db_path()?, Migration::ToPlain)?;
    for name in SEALED_FILE_NAMES {
        let path = storage::slot_file(name)?;
        if path.is_file() {
            let raw =
                std::fs::read(&path).with_context(|| format!("读取 {} 失败", path.display()))?;
            if is_sealed_file(&raw) {
                let plain = open_bytes(&key_dek()?, &raw)?;
                storage::atomic_write(&path, |file| {
                    use std::io::Write as _;
                    file.write_all(&plain)?;
                    Ok(())
                })?;
            }
        }
    }
    Ok(())
}

fn key_dek() -> Result<[u8; DEK_LEN]> {
    let handle = key_handle().context("迁移需要数据密钥句柄")?;
    Ok(*handle.dek())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Migration {
    ToEncrypted,
    ToPlain,
}

/// 用 SQLCipher 自带的 `sqlcipher_export` 把 `src` 整库导出到 `dest`（必须是
/// 新文件，已存在会先删掉重建）。`dest_key` 为 Some 时目标按密文建（ATTACH 带
/// 密钥），为 None 时建明文（`KEY ''`）——两端形态各自对齐，迁移与备份快照
/// 共用这一条路（SQLCipher 不支持 SQLite 在线备份 API）。
pub(crate) fn export_database(
    src: &rusqlite::Connection,
    dest: &Path,
    dest_key: Option<&[u8; DEK_LEN]>,
) -> Result<()> {
    let _ = std::fs::remove_file(dest);
    // 附库继承主连接的 openFlags：主连接不带 CREATE 时 ATTACH 建不了新文件，
    // 先建一个空文件占位（SQLCipher 会把空文件当新库初始化）。
    std::fs::File::create(dest)
        .with_context(|| format!("创建导出目标文件失败：{}", dest.display()))?;
    // 密钥必须作为 SQL 文本内联：SQLCipher 只对语句里的 x'..' 字面 token 认
    // 原始密钥写法，绑定参数会被当成普通口令，导出的库就用错密钥了。
    // 这是官方迁移写法；DEK 是进程内随机字节，语句串不出本进程、不进日志。
    let dest_lit = dest.to_string_lossy().replace('\'', "''");
    let key_lit = dest_key.map_or_else(
        || "''".to_string(),
        |key| format!("\"x'{}'\"", hex_encode(key)),
    );
    src.execute_batch(&format!(
        "ATTACH DATABASE '{dest_lit}' AS export_dest KEY {key_lit};"
    ))
    .context("挂载导出目标库失败")?;
    let export = src.query_row("SELECT sqlcipher_export('export_dest')", [], |_| Ok(()));
    let detach = src.execute("DETACH DATABASE export_dest", []);
    export.context("整库导出失败")?;
    detach.context("卸载导出目标库失败")?;
    Ok(())
}

/// 把 manuscripts.db（稿件 / 知识 / 词表 / 任务助手共用的那一个文件）整体
/// 搬成目标形态：`sqlcipher_export` 导出新库（形态见 [`export_database`]），
/// 搬完交换文件名、覆写删除旧文件与 WAL 残留。各库打开时都设 WAL，
/// 新库首次打开会自行切回 WAL，不用在这里管 journal mode。
fn migrate_database(path: &Path, direction: Migration) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let want_active = match direction {
        Migration::ToEncrypted => true,
        Migration::ToPlain => false,
    };
    if database_matches(path, want_active)? {
        return Ok(());
    }
    let key = key_dek()?;
    let parent = path.parent().context("数据库路径缺少父目录")?;
    let stamp = format!("迁移-{}", std::process::id());
    let temp = parent.join(format!(
        "{}.{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "manuscripts.db".into()),
        stamp
    ));
    let _ = std::fs::remove_file(&temp);
    let result = (|| -> Result<()> {
        let src = rusqlite::Connection::open(path)
            .with_context(|| format!("打开原库失败：{}", path.display()))?;
        // 源库形态与目标相反：去明文方向源是密文（带 key 开），去密文方向源是明文（不带 key）。
        apply_key_with(&src, &key, matches!(direction, Migration::ToPlain))?;
        // 源库形态确认：能列出 sqlite_master 才搬，不然是损坏或形态不符。
        src.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .context("原库读不了（已损坏或形态不符）")?;
        export_database(&src, &temp, if want_active { Some(&key) } else { None })
            .context("整库迁移导出失败")?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    // 交换：旧文件挪走覆写删除，新文件就位；WAL / SHM 是旧库形态的残留，一并清。
    let leftover = parent.join(format!(
        "{}.{}.old",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "manuscripts.db".into()),
        stamp
    ));
    let _ = std::fs::remove_file(&leftover);
    std::fs::rename(path, &leftover).context("挪走旧库失败")?;
    std::fs::rename(&temp, path).context("换上新库失败")?;
    overwrite_then_delete(&leftover);
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", leftover.display()));
        if sidecar.exists() {
            overwrite_then_delete(&sidecar);
        }
    }
    Ok(())
}

/// 源库是否已经是目标形态：带 key 完整性检查通过 = 密文；不带 key 能读 = 明文。
/// `cipher_integrity_check` 只在发现问题时返回行（每行一条错误描述），
/// 全部通过是零行——别指望 "ok" 行。
fn database_matches(path: &Path, encrypted: bool) -> Result<bool> {
    let key = key_dek()?;
    let check_with_key = |active: bool| -> bool {
        let Ok(conn) = rusqlite::Connection::open(path) else {
            return false;
        };
        if apply_key_with(&conn, &key, active).is_err() {
            return false;
        }
        if !active {
            return conn
                .query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
                .is_ok();
        }
        let mut statement = match conn.prepare("PRAGMA cipher_integrity_check") {
            Ok(statement) => statement,
            Err(_) => return false,
        };
        let checks = match statement.query_map([], |row| row.get::<_, String>(0)) {
            Ok(checks) => checks,
            Err(_) => return false,
        };
        // 零错误行 = 这把 key 的密文库；任何一行都是 HMAC / 读页失败。
        checks.flatten().next().is_none()
    };
    if check_with_key(encrypted) {
        return Ok(true);
    }
    if !check_with_key(!encrypted) {
        // 两种形态都开不了：库损坏。文案统一，不泄露形态细节。
        bail!("数据库无法读取，可能已损坏");
    }
    Ok(false)
}

/// 对单连接应用数据密钥。`active = false` 时什么也不做（明文库形态）。
/// 注意：必须用 `key = 'x'..'`（原始密钥写法），不能用 `PRAGMA hexkey`——
/// 这个 SQLCipher 构建里两者语义不同：hexkey 把解码后的 32 字节当口令再
/// 走一遍 PBKDF2，而 x'..' 与 `ATTACH ... KEY "x'..'"` 才是同一把原始密钥，
/// 混用会让迁移导出的库和日常开库对不上密钥。
fn apply_key_with(conn: &rusqlite::Connection, key: &[u8; DEK_LEN], active: bool) -> Result<()> {
    if !active {
        return Ok(());
    }
    conn.pragma_update(None, "key", format!("x'{}'", hex_encode(key)))
        .context("数据库密钥应用失败")?;
    Ok(())
}

/// 覆写一遍再删：明文库文件直接 delete 会被恢复工具捞回来。
/// 尽力而为——覆写失败仍删除，只是不保证抗恢复。
fn overwrite_then_delete(path: &Path) {
    if let Ok(metadata) = std::fs::metadata(path)
        && let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path)
    {
        use std::io::Write as _;
        let chunk = random_bytes_16();
        let mut written = 0u64;
        let mut buffer = Vec::with_capacity(65536);
        while buffer.len() < 65536 {
            buffer.extend_from_slice(&chunk);
        }
        while written < metadata.len() {
            let take = ((metadata.len() - written) as usize).min(buffer.len());
            if file.write_all(&buffer[..take]).is_err() {
                break;
            }
            written += take as u64;
        }
        let _ = file.sync_all();
    }
    let _ = std::fs::remove_file(path);
}

// ── 停用流程的跨启动标记 ────────────────────────────────────────────

/// 停用是两条退路的合流：本会话内由设置页触发（静态标记 + 请求锁定），
/// 锁定后闸门输入一次密码完成解密；若其间重启，靠这个标记文件在下次启动
/// 重新进入「输密码完成停用」的闸门形态。
const DISABLE_PENDING_FILE: &str = "vault.pending-disable";
static DISABLE_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn request_disable() -> Result<()> {
    let path = storage::config_dir()?.join(DISABLE_PENDING_FILE);
    storage::atomic_write(&path, |file| {
        use std::io::Write as _;
        file.write_all(b"disable-pending")?;
        Ok(())
    })?;
    DISABLE_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

pub(crate) fn cancel_disable() {
    DISABLE_PENDING.store(false, std::sync::atomic::Ordering::Relaxed);
    if let Ok(path) = storage::config_dir() {
        let _ = std::fs::remove_file(path.join(DISABLE_PENDING_FILE));
    }
}

/// 停用在途（本会话或上个会话标记了、还没完成解密）。
pub(crate) fn disable_pending() -> bool {
    if DISABLE_PENDING.load(std::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    storage::config_dir()
        .map(|dir| dir.join(DISABLE_PENDING_FILE).exists())
        .unwrap_or(false)
}

/// 停用完成（或用户取消）后清掉两条标记。
pub(crate) fn clear_disable_pending() {
    cancel_disable();
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
        clear_key_handle();
    }

    /// 建一个带两张表的明文库，返回路径。
    fn plain_database() -> PathBuf {
        let path = crate::storage::manuscript_db_path().unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE t1 (id INTEGER PRIMARY KEY, body TEXT);
             CREATE TABLE t2 (id INTEGER PRIMARY KEY, body TEXT);
             INSERT INTO t1 (body) VALUES ('公文'), ('助手');
             INSERT INTO t2 (body) VALUES ('诱饵');",
        )
        .unwrap();
        drop(conn);
        path
    }

    fn table_bodies(path: &Path) -> Vec<String> {
        let conn = rusqlite::Connection::open(path).unwrap();
        apply_key_with(&conn, key_dek().as_ref().unwrap(), true).unwrap();
        let mut stmt = conn.prepare("SELECT body FROM t1 ORDER BY id").unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
        rows.map(|row| row.unwrap()).collect()
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
        let (mut vault, unlocked) =
            Vault::create("真实口令abc", Some("诱饵口令xyz"), false).unwrap();
        let real_dek = *unlocked.unwrap().dek;
        let unlocked = vault.unlock("真实口令abc").unwrap();
        assert_eq!(unlocked.slot, DataSlot::Real);
        assert_eq!(*unlocked.dek, real_dek, "主密码派生的 DEK 必须稳定");
        assert_eq!(vault.unlock("诱饵口令xyz").unwrap().slot, DataSlot::Decoy);
        assert!(vault.unlock("不对").is_none());
        drop(dir);
        reset();
    }

    #[test]
    fn wrong_passwords_are_indistinguishable_from_missing_slots() {
        // 没有第二密码的锁：错的主密码、对的哑槽密码（不存在）、随机串——
        // 解锁结果一律是 None，UI 只能给出统一文案。
        let dir = use_temp_dir();
        let (mut vault, _) = Vault::create("只有主密码", None, false).unwrap();
        assert!(vault.unlock("只有主密码").is_some());
        assert!(vault.unlock("任意猜测").is_none());
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
            assert_eq!(wa.slots[index].wrapped.len(), wb.slots[index].wrapped.len());
        }
    }

    #[test]
    fn dummy_seal_slot_survives_clear_and_readd() {
        let dir = use_temp_dir();
        let (mut vault, _) = Vault::create("主密码aaaa", Some("第二密码bbbb"), false).unwrap();
        assert_eq!(vault.unlock("第二密码bbbb").unwrap().slot, DataSlot::Decoy);
        vault.clear_decoy().unwrap();
        assert!(
            vault.unlock("第二密码bbbb").is_none(),
            "清除后旧第二密码失效"
        );
        assert!(vault.unlock("主密码aaaa").is_some());
        vault
            .set_password(DataSlot::Decoy, "新第二密码cc", None)
            .unwrap();
        assert!(vault.unlock("新第二密码cc").is_some());
        drop(dir);
        reset();
    }

    #[test]
    fn decoy_password_cannot_equal_real_password() {
        let dir = use_temp_dir();
        let (mut vault, _) = Vault::create("同一个密码", None, false).unwrap();
        assert!(
            vault
                .set_password(DataSlot::Decoy, "同一个密码", None)
                .is_err()
        );
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
    fn changing_real_password_keeps_the_same_dek() {
        let dir = use_temp_dir();
        let (mut vault, unlocked) = Vault::create("旧主密码aa", None, false).unwrap();
        let old_dek = *unlocked.unwrap().dek;
        let rewrapped = vault
            .set_password(DataSlot::Real, "新主密码bb", Some(&old_dek))
            .unwrap();
        assert_eq!(*rewrapped.dek, old_dek, "改密只换包裹，DEK 不动");
        assert!(vault.unlock("旧主密码aa").is_none());
        assert_eq!(*vault.unlock("新主密码bb").unwrap().dek, old_dek);
        drop(dir);
        reset();
    }

    #[test]
    fn reset_file_removes_the_vault_and_keeps_data_files() {
        let dir = use_temp_dir();
        Vault::create("主密码aaaa", None, false).unwrap();
        std::fs::write(
            crate::storage::manuscript_db_path().unwrap(),
            b"fake db bytes",
        )
        .unwrap();
        Vault::reset_file().unwrap();
        assert_eq!(Vault::probe(), Probe::Absent);
        assert!(
            crate::storage::manuscript_db_path().unwrap().exists(),
            "删锁文件不动数据文件"
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
    fn legacy_v1_vault_upgrades_in_place_on_unlock() {
        let dir = use_temp_dir();
        // 手写一份 v1 锁文件（第一期格式：槽里只有 salt + seal，没有 wrapped 字段）。
        let (real_slot, _) = SlotSeal::sealed("主密码aaaa", None).unwrap();
        let legacy_slot = SlotSeal {
            wrapped: String::new(),
            ..real_slot
        };
        let legacy = VaultData {
            v: LEGACY_V,
            slots: [legacy_slot, SlotSeal::dummy().unwrap()],
        };
        // 把 wrapped 字段从 JSON 里抹掉，模拟真正的 v1 文件。
        let mut value = serde_json::to_value(&legacy).unwrap();
        for slot in value["slots"].as_array_mut().unwrap() {
            slot.as_object_mut().unwrap().remove("wrapped");
        }
        std::fs::write(
            Vault::path().unwrap(),
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .unwrap();
        assert_eq!(Vault::probe(), Probe::Locked, "v1 不算损坏");

        let mut vault = Vault::load().expect("v1 锁文件应能加载");
        let unlocked = vault.unlock("主密码aaaa").expect("v1 主密码应能解锁");
        assert_eq!(unlocked.slot, DataSlot::Real);
        assert_eq!(Vault::probe(), Probe::Locked);
        // 解锁后文件应已升级成 v2：只有命中那个槽补上 wrapped（懒升级——
        // 另一把密码的槽保持 v1 形状，等那把密码解锁时再补，不提前作废它）。
        let upgraded: VaultData =
            serde_json::from_str(&std::fs::read_to_string(Vault::path().unwrap()).unwrap())
                .unwrap();
        assert_eq!(upgraded.v, FORMAT_V);
        assert!(
            !upgraded.slots[0].wrapped.is_empty(),
            "命中槽补上数据密钥包裹"
        );
        assert!(
            upgraded.slots[1].wrapped.is_empty(),
            "未解锁的槽保持 v1 形状，等自己的密码解锁时再升级"
        );
        // 升级后的文件用同一把密码还能再解锁（DEK 已持久化，不重生成）。
        let mut again = Vault::load().unwrap();
        let second = again.unlock("主密码aaaa").expect("v2 升级后仍应能解锁");
        assert_eq!(second.dek, unlocked.dek, "升级补写的 DEK 必须稳定");
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

    #[test]
    fn sealed_files_round_trip_and_are_recognizable() {
        let dek = random_dek().unwrap();
        let plain = "机密内容".as_bytes();
        let sealed = seal_bytes(&dek, plain).unwrap();
        assert!(is_sealed_file(&sealed));
        assert!(!is_sealed_file(plain));
        assert_eq!(open_bytes(&dek, &sealed).unwrap(), plain);
        let mut tampered = sealed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert!(open_bytes(&dek, &tampered).is_err(), "篡改必须被认证拦住");
        let other = random_dek().unwrap();
        assert!(open_bytes(&other, &sealed).is_err(), "别的密钥解不开");
    }

    #[test]
    fn ensure_slot_encrypted_migrates_a_plain_database_and_files() {
        let dir = use_temp_dir();
        let path = plain_database();
        // 明文小文件各一份。
        std::fs::write(
            crate::storage::config_path().unwrap(),
            b"{\"output_dir\":\"x\"}",
        )
        .unwrap();
        std::fs::write(
            crate::storage::slot_file(".zip-password").unwrap(),
            b"zip-pass",
        )
        .unwrap();

        let (_vault, unlocked) = Vault::create("主密码aaaa", None, false).unwrap();
        let dek = *unlocked.unwrap().dek;
        set_key_handle(dek, false);
        ensure_slot_encrypted().unwrap();
        activate_db_encryption();

        // 库：带 key 能读、数据齐全；不带 key 打不开。
        assert_eq!(table_bodies(&path), vec!["公文", "助手"]);
        let raw_conn = rusqlite::Connection::open(&path).unwrap();
        assert!(
            raw_conn
                .query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
                .is_err(),
            "密文库不带 key 必须读不了"
        );
        drop(raw_conn);
        // 小文件已密封。
        let config = std::fs::read(crate::storage::config_path().unwrap()).unwrap();
        assert!(is_sealed_file(&config));
        assert_eq!(
            open_bytes(&dek, &config).unwrap(),
            b"{\"output_dir\":\"x\"}"
        );

        // 幂等：再来一遍不报错也不动数据。
        ensure_slot_encrypted().unwrap();
        assert_eq!(table_bodies(&path), vec!["公文", "助手"]);
        drop(dir);
        reset();
    }

    #[test]
    fn decrypt_slot_files_restores_plaintext() {
        let dir = use_temp_dir();
        let path = plain_database();
        std::fs::write(
            crate::storage::config_path().unwrap(),
            b"{\"output_dir\":\"x\"}",
        )
        .unwrap();
        let (_vault, unlocked) = Vault::create("主密码aaaa", None, false).unwrap();
        set_key_handle(*unlocked.unwrap().dek, false);
        ensure_slot_encrypted().unwrap();
        activate_db_encryption();

        decrypt_slot_files().unwrap();
        // 不带 key 又能读了。
        let conn = rusqlite::Connection::open(&path).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM t2", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        drop(conn);
        let config = std::fs::read(crate::storage::config_path().unwrap()).unwrap();
        assert!(!is_sealed_file(&config), "config 应回到明文");
        assert_eq!(config, b"{\"output_dir\":\"x\"}");
        drop(dir);
        reset();
    }
}
