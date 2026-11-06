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

use crate::password_policy;
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

/// 校验密码强度。规则与稿件包导出密码共用一套判据（`crate::password_policy`），
/// 但应用锁更严一档：长度必须**大于 10 个字符**（按字符数，不是字节数），
/// 且覆盖大写/小写/数字/符号/中文中的至少三类；常见口令、连续序列、
/// 连续重复字符一律拒绝。
///
/// 「暂不设置」（`skipped`）不写任何密码，因此不走这条校验。
fn check_password(password: &str) -> Result<()> {
    password_policy::validate_lock(password)
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
                // v1 → v2 升级：现生成 DEK、裹进当前槽并**必须写回成功**才交出句柄。
                // fail-closed：写回失败仍返回新 DEK 的话，本次会把数据库迁成这把
                // 临时密钥的密文，而磁盘上包裹仍是空的——下次解锁会换一把新
                // DEK，数据永久打不开。
                let dek = random_dek().ok()?;
                let wrapped = wrap_dek(&wrap_key, &dek).ok()?;
                self.data.slots[index].wrapped = wrapped;
                self.data.v = FORMAT_V;
                self.save().ok()?;
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
    /// 句柄创建时绑定的数据槽位。后台任务复制句柄后，即使全局当前槽位改变，
    /// 资源 AAD 与路径校验仍固定在原槽位，不能拿新槽位重写旧任务的数据。
    slot: DataSlot,
}

impl KeyHandle {
    /// SQLCipher `PRAGMA hexkey` 用的 hex 串（不带引号）。用 `Zeroizing` 包住，
    /// 临时副本在进程内尽量短命；不要把它写日志、错误串或持久文件。
    pub(crate) fn dek_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(hex_encode(&self.dek[..]))
    }

    pub(crate) fn db_active(&self) -> bool {
        self.db_active
    }

    pub(crate) fn dek(&self) -> &[u8; DEK_LEN] {
        &self.dek
    }

    pub(crate) fn slot(&self) -> DataSlot {
        self.slot
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
        slot: handle.slot,
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

/// 仅测试用：按进程当前槽位创建句柄。生产路径一律用 [`set_key_handle_for_slot`]。
#[cfg(test)]
pub(crate) fn set_key_handle(dek: [u8; DEK_LEN], db_active: bool) {
    let handle = Some(KeyHandle {
        dek: Zeroizing::new(dek),
        db_active,
        slot: storage::current_slot(),
    });
    #[cfg(test)]
    TEST_KEY_HANDLE.with(|cell| *cell.borrow_mut() = handle);
    #[cfg(not(test))]
    if let Ok(mut guard) = KEY_HANDLE.write() {
        *guard = handle;
    }
}

/// 显式绑定槽位的句柄创建：生产路径一律用它，避免「先设句柄、后切槽位」时
/// 句柄记错槽位（诱饵密码解锁会拿到 Real 句柄，所有 alt-* 资源全部读写失败）。
pub(crate) fn set_key_handle_for_slot(dek: [u8; DEK_LEN], db_active: bool, slot: DataSlot) {
    let handle = Some(KeyHandle {
        dek: Zeroizing::new(dek),
        db_active,
        slot,
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

/// GWA1 文件头：魔数 + 版本。只保留给历史数据与锁内部小封套的兼容读取。
pub(crate) const FILE_MAGIC: &[u8; 4] = b"GWA1";
/// GWA2 资源封套。与 GWA1 的二进制布局相同，但加密时把槽位、资源用途与逻辑路径
/// 放进 AAD；同一把 DEK 下互换两个文件也会认证失败。
pub(crate) const RESOURCE_MAGIC: &[u8; 4] = b"GWA2";

/// 密封文件相对明文增加的固定字节数：魔数、nonce 与认证标签。
/// 带明文上限的读取要给它留出预算，否则恰好达到上限的原文加密后会读不回来。
pub(crate) const SEALED_FILE_OVERHEAD: usize = FILE_MAGIC.len() + NONCE_LEN + TAG_LEN;

pub(crate) fn is_sealed_file(bytes: &[u8]) -> bool {
    bytes.starts_with(FILE_MAGIC) || bytes.starts_with(RESOURCE_MAGIC)
}

pub(crate) fn is_current_resource_file(bytes: &[u8]) -> bool {
    bytes.starts_with(RESOURCE_MAGIC)
}

/// 锁内部使用的 GWA1 整份加密：MAGIC ‖ nonce ‖ AEAD(DEK, 明文)。
/// 应用管理的磁盘资源不要再用它，改用 [`seal_path_bytes`] 写入 GWA2。
pub(crate) fn seal_bytes(dek: &[u8; DEK_LEN], plain: &[u8]) -> Result<Vec<u8>> {
    seal_with_magic(dek, FILE_MAGIC, &[], plain)
}

fn seal_with_magic(
    dek: &[u8; DEK_LEN],
    magic: &[u8; 4],
    aad: &[u8],
    plain: &[u8],
) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(dek.into());
    let nonce = random_nonce()?;
    let ciphertext = cipher
        .encrypt(&nonce, aes_gcm::aead::Payload { msg: plain, aad })
        .context("文件加密失败")?;
    let mut out = Vec::with_capacity(magic.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(magic);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// 锁内部 GWA1 解密。GWA2 必须带预期路径上下文，走 [`open_path_bytes`]。
pub(crate) fn open_bytes(dek: &[u8; DEK_LEN], sealed: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(sealed.starts_with(FILE_MAGIC), "不是 GWA1 加密文件");
    open_with_aad(dek, sealed, &[])
}

fn open_with_aad(dek: &[u8; DEK_LEN], sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(is_sealed_file(sealed), "不是加密文件");
    anyhow::ensure!(sealed.len() >= SEALED_FILE_OVERHEAD, "加密文件长度不符");
    let cipher = Aes256Gcm::new(dek.into());
    let nonce = nonce_from(&sealed[FILE_MAGIC.len()..FILE_MAGIC.len() + NONCE_LEN])?;
    cipher
        .decrypt(
            &nonce,
            aes_gcm::aead::Payload {
                msg: &sealed[FILE_MAGIC.len() + NONCE_LEN..],
                aad,
            },
        )
        .context("文件解密失败（密钥、用途或路径不符，或内容损坏）")
}

/// GWA2 的 AAD 与 Node host 的 `secure-file.mjs` 保持一致。
/// 槽位使用中性数字，逻辑路径统一 `/`，诱饵目录的物理 `alt-` 前缀不进入 ID。
fn resource_aad(slot: DataSlot, kind: &str, logical_id: &str) -> Result<Vec<u8>> {
    anyhow::ensure!(!kind.is_empty() && kind.len() <= 64, "资源用途标识不合法");
    anyhow::ensure!(
        !logical_id.is_empty() && logical_id.len() <= 4096,
        "资源逻辑路径不合法"
    );
    anyhow::ensure!(
        kind.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-'),
        "资源用途标识只能含小写字母与短横线"
    );
    let slot = match slot {
        DataSlot::Real => "0",
        DataSlot::Decoy => "1",
    };
    Ok(format!("gongwen-resource-v2\0{slot}\0{kind}\0{logical_id}").into_bytes())
}

/// 对可能尚未创建的文件也做 realpath：已存在就 canonicalize；不存在就递归
/// 规范化父目录再拼回文件名。配置目录本身不存在时退回词法路径（首次启动）。
fn canonicalize_allow_missing(path: &Path) -> Result<PathBuf> {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return Ok(canonical);
    }
    let Some(parent) = path.parent() else {
        return Ok(path.to_path_buf());
    };
    let Some(name) = path.file_name() else {
        return Ok(path.to_path_buf());
    };
    let parent = canonicalize_allow_missing(parent)?;
    Ok(parent.join(name))
}

/// 从配置目录内的物理路径推出稳定逻辑 ID。真实与诱饵槽有相同逻辑 ID，
/// 但 AAD 的槽位不同；备份/恢复换目录不会改变 ID。
pub(crate) fn resource_identity(path: &Path) -> Result<(DataSlot, String)> {
    // 逻辑 ID 必须对「同一个物理文件」稳定：配置目录可能经符号链接/junction 访问，
    // 文件也可能是新文件（父目录存在、自身还没创建）。统一按 realpath 规范化
    // 后再相对配置目录，Node host 侧也按同样口径算 AAD，跨进程才不会失配。
    let root = canonicalize_allow_missing(&storage::config_dir()?)?;
    let absolute = canonicalize_allow_missing(path)?;
    let relative = absolute
        .strip_prefix(&root)
        .with_context(|| format!("资源不在配置目录内：{}", path.display()))?;
    let mut parts = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(value) => Ok(value.to_string_lossy().into_owned()),
            _ => bail!("资源逻辑路径含非法组件：{}", path.display()),
        })
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(!parts.is_empty(), "资源逻辑路径为空");
    let slot = if let Some(rest) = parts[0].strip_prefix(storage::DECOY_PREFIX) {
        parts[0] = rest.to_string();
        DataSlot::Decoy
    } else {
        DataSlot::Real
    };
    Ok((slot, parts.join("/")))
}

pub(crate) fn seal_path_bytes(
    handle: &KeyHandle,
    path: &Path,
    kind: &str,
    plain: &[u8],
) -> Result<Vec<u8>> {
    let (slot, id) = resource_identity(path)?;
    anyhow::ensure!(slot == handle.slot(), "拒绝使用另一数据槽位的密钥写资源");
    let aad = resource_aad(slot, kind, &id)?;
    seal_with_magic(handle.dek(), RESOURCE_MAGIC, &aad, plain)
}

/// 读取 GWA2 时调用方必须给出预期物理路径与用途；GWA1 旧数据仍可解，交由
/// 解锁迁移升级为 GWA2。
pub(crate) fn open_path_bytes(
    handle: &KeyHandle,
    path: &Path,
    kind: &str,
    sealed: &[u8],
) -> Result<Vec<u8>> {
    let (slot, id) = resource_identity(path)?;
    anyhow::ensure!(slot == handle.slot(), "拒绝用当前槽位读取另一槽位资源");
    if sealed.starts_with(FILE_MAGIC) {
        return open_bytes(handle.dek(), sealed);
    }
    let aad = resource_aad(slot, kind, &id)?;
    open_with_aad(handle.dek(), sealed, &aad)
}

/// 应用管理文件统一写入口：有句柄写 GWA2；无句柄只允许普通工作区。
/// 目标存在但读不出时同样拒绝，避免把「权限错误」误判成可以明文覆盖。
pub(crate) fn write_path(path: &Path, kind: &str, plain: &[u8]) -> Result<()> {
    write_path_impl(path, kind, plain, true)
}

/// 可丢弃缓存（Mermaid 图/警告等）的写入：同样密封，但允许覆盖「当前密钥解不开」
/// 的旧密文——缓存命中判定会把它当未命中重新渲染，错误密钥下的旧缓存必须能替换。
pub(crate) fn write_path_replacing(path: &Path, kind: &str, plain: &[u8]) -> Result<()> {
    write_path_impl(path, kind, plain, false)
}

fn write_path_impl(
    path: &Path,
    kind: &str,
    plain: &[u8],
    verify_existing_ciphertext: bool,
) -> Result<()> {
    let bytes = match key_handle() {
        Some(handle) => {
            // 目标已存在且是密文时，先用当前密钥试解一次：解不开说明是另一代次 /
            // 另一槽 / 已损坏的密文，覆盖会毁掉唯一副本（配置解密失败后回落默认
            // 配置再保存就会走到这里）。大文件跳过这项检查，避免每次写都整份解密。
            if verify_existing_ciphertext
                && path.is_file()
                && std::fs::metadata(path).is_ok_and(|meta| meta.len() <= 1024 * 1024)
            {
                let raw = std::fs::read(path)
                    .with_context(|| format!("无法确认 {} 是否可解密", path.display()))?;
                if is_sealed_file(&raw) {
                    open_path_bytes(&handle, path, kind, &raw).with_context(|| {
                        format!(
                            "{} 已加密但当前密钥无法解开，拒绝覆盖（请先从备份恢复或确认锁设置）",
                            path.display()
                        )
                    })?;
                }
            }
            seal_path_bytes(&handle, path, kind, plain)?
        }
        None => {
            if path.exists() {
                let raw = std::fs::read(path)
                    .with_context(|| format!("无法确认 {} 是否已加密", path.display()))?;
                anyhow::ensure!(
                    !is_sealed_file(&raw),
                    "目标已加密但当前未解锁，拒绝明文覆盖"
                );
            }
            plain.to_vec()
        }
    };
    storage::atomic_write(path, |file| {
        use std::io::Write as _;
        file.write_all(&bytes)?;
        Ok(())
    })
}

/// 应用管理文件统一读入口；`max_bytes` 限制解密后的明文，而非封套大小。
pub(crate) fn read_path(path: &Path, kind: &str, max_bytes: u64) -> Result<Vec<u8>> {
    let raw = crate::file_limits::read_bytes(
        path,
        max_bytes.saturating_add(SEALED_FILE_OVERHEAD as u64),
        "受保护文件",
    )?;
    let plain = if is_sealed_file(&raw) {
        let handle = key_handle().context("文件已加密，尚未解锁")?;
        open_path_bytes(&handle, path, kind, &raw)?
    } else {
        raw
    };
    if plain.len() as u64 > max_bytes {
        return Err(crate::file_limits::too_large("受保护文件", max_bytes).into());
    }
    Ok(plain)
}

/// 应用锁启用后必须整份密封的文件名单。唯一来源是 [`crate::storage::SEALED_FILE_NAMES`]，
/// 这里只做转出：解锁迁移、明文嗅探、解封、擦除都必须用同一份，漏一项就是永久明文。
/// 数据库不在这份名单里（走 SQLCipher 迁移）。
pub(crate) const SEALED_FILE_NAMES: [&str; 7] = crate::storage::SEALED_FILE_NAMES;

// ── 明文 ⇄ 密文迁移 ─────────────────────────────────────────────────

/// 目录不存在表示没有待迁移内容；其他错误必须上抛，不能把「没读到」当成「已迁完」。
fn read_dir_if_exists(path: &Path) -> Result<Option<std::fs::ReadDir>> {
    match std::fs::read_dir(path) {
        Ok(entries) => Ok(Some(entries)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("读取目录 {} 失败", path.display())),
    }
}

/// 解锁后为当前槽位做的数据迁移：库不够密文就整体搬进密文库，小文件逐个密封。
/// 幂等：已是密文就直接返回。任何一步失败返回 Err，由闸门翻译成提示。
pub(crate) fn ensure_slot_encrypted() -> Result<()> {
    let key = key_dek()?;
    migrate_database(&storage::manuscript_db_path()?, Migration::ToEncrypted)?;
    for name in SEALED_FILE_NAMES {
        let path = storage::slot_file(name)?;
        if path.is_file() {
            seal_plaintext_file(&path, &key)?;
        }
    }
    // 接口定义目录（apis/*.openapi.json）与旧接口迁移兜底文件同样是敏感明文：
    // 里有内网地址与参数说明。目录不在 SEALED_FILE_NAMES（是动态文件名），
    // 逐文件密封；启用加密前留下的明文在这里一次迁完。
    if let Some(entries) = read_dir_if_exists(&storage::slot_subdir("apis")?)? {
        for entry in entries {
            let entry = entry.context("读取接口定义目录条目失败")?;
            let path = entry.path();
            if entry
                .metadata()
                .with_context(|| format!("读取 {} 元数据失败", path.display()))?
                .is_file()
            {
                seal_plaintext_file(&path, &key)?;
            }
        }
    }
    // `config.json` 的副本（身份迁移前备份）与旧接口 `apis.json.bak`：都在槽位根下。
    // 只处理**当前槽位**的条目，绝不能拿当前 DEK 去封另一个槽的文件。
    if let Some(entries) = read_dir_if_exists(&storage::config_dir()?)? {
        let slot = storage::current_slot();
        for entry in entries {
            let entry = entry.context("读取配置目录条目失败")?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if storage::slot_relative_name(&name, slot).is_some_and(|name| {
                name.starts_with("config.json.") || name.ends_with("apis.json.bak")
            }) {
                seal_plaintext_file(&entry.path(), &key)?;
            }
        }
    }
    // 用户资源：图片、PDF 插图与 Mermaid 缓存整文件 AEAD。帮助截图是随包公开
    // 内容，永远保持明文（诱饵槽缺图时还要按字节比对回落）。
    seal_resource_dirs(&key)?;
    // Pi（工作区、Durable JSONL、来源、接口缓存、用户技能）、个人词表与诊断日志
    // 都是逐文件 GWA2，不在退出时打包，也不允许 host 运行期间写明文。
    for name in ["pi", "ime", "skills", "logs"] {
        let dir = storage::slot_subdir(name)?;
        for_each_regular_file(&dir, |path| seal_plaintext_file(path, &key))?;
    }
    Ok(())
}

/// `pi/` 等私有目录里不参与资源加密的瞬时文件：Node host 的 SQLite 占用锁
/// （`host-owner.lock` 与它的 -wal/-shm）、Rust 的 `rust-owner.lock`、原子写
/// 残留的 `*.tmp`。它们不含业务数据；一旦被当成资源封成 GWA2，host 下次用
/// SQLite 打开锁文件会报「file is not a database」，恢复/续写直接失败。
pub(crate) fn is_transient_private_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.contains(".lock") || name.ends_with(".tmp")
}

/// 不跟随链接地遍历应用私有目录。任何目录/条目/元数据错误都 fail-closed；
/// 文件数设硬上限，避免损坏目录让解锁迁移无限扫描。
fn for_each_regular_file(root: &Path, visit: impl Fn(&Path) -> Result<()>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = pending.pop() {
        let Some(entries) = read_dir_if_exists(&dir)? else {
            continue;
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("读取 {} 目录条目失败", dir.display()))?;
            visited += 1;
            anyhow::ensure!(visited <= 200_000, "私有资源目录条目过多，已停止迁移");
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path)
                .with_context(|| format!("读取 {} 元数据失败", path.display()))?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file() && !is_transient_private_file(&path) {
                visit(&path)?;
            }
        }
    }
    Ok(())
}

/// 用户资源目录（图片 / Mermaid 缓存）。两处都按槽位命名，随稿件长期保留。
fn resource_dirs() -> [(String, bool); 2] {
    [
        (crate::images::IMAGE_DIR.to_string(), true),
        (storage::MERMAID_CACHE_DIR.to_string(), false),
    ]
}

/// 把当前槽位资源目录里的明文文件封成密文；已是密文的跳过。公开帮助截图跳过。
fn seal_resource_dirs(key: &[u8; DEK_LEN]) -> Result<()> {
    for (name, skip_public) in resource_dirs() {
        let dir = storage::slot_subdir(&name)?;
        let Some(entries) = read_dir_if_exists(&dir)? else {
            continue;
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("读取 {} 目录条目失败", dir.display()))?;
            let path = entry.path();
            // 不跟随符号链接：否则会把链接目标读进加密库、再把链接替换成普通文件。
            let meta = std::fs::symlink_metadata(&path)
                .with_context(|| format!("读取 {} 元数据失败", path.display()))?;
            if meta.file_type().is_symlink() || !meta.is_file() {
                continue;
            }
            if skip_public && crate::images::is_public_help_file(&path) {
                continue;
            }
            seal_plaintext_file(&path, key)?;
        }
    }
    Ok(())
}

/// 停用加密时把当前槽位资源目录解回明文。
fn unseal_resource_dirs() -> Result<()> {
    let handle = key_handle().context("停用加密需要当前槽位密钥")?;
    for (name, _) in resource_dirs() {
        let dir = storage::slot_subdir(&name)?;
        let Some(entries) = read_dir_if_exists(&dir)? else {
            continue;
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("读取 {} 目录条目失败", dir.display()))?;
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path)
                .with_context(|| format!("读取 {} 元数据失败", path.display()))?;
            if meta.file_type().is_symlink() || !meta.is_file() {
                continue;
            }
            let raw =
                std::fs::read(&path).with_context(|| format!("读取 {} 失败", path.display()))?;
            if !is_sealed_file(&raw) {
                continue;
            }
            let plain = open_path_bytes(&handle, &path, resource_kind_for_path(&path)?, &raw)?;
            storage::atomic_write(&path, |file| {
                use std::io::Write as _;
                file.write_all(&plain)?;
                Ok(())
            })?;
        }
    }
    Ok(())
}

/// 资源目录里是否还有明文文件（帮助截图除外）。只读文件头，不整文件解密。
/// 这个探测接口只能返回 bool，所以任何 I/O 错误都按「仍需迁移」处理；真正的
/// 迁移会再次读取并向用户报告具体错误，而不是把目录跳过并宣称加密完成。
fn resource_dirs_need_encryption(slot: DataSlot) -> bool {
    for (name, skip_public) in resource_dirs() {
        let dir = match storage::slot_subdir_for(slot, &name) {
            Ok(dir) => dir,
            Err(_) => return true,
        };
        let entries = match read_dir_if_exists(&dir) {
            Ok(Some(entries)) => entries,
            Ok(None) => continue,
            Err(_) => return true,
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => return true,
            };
            let path = entry.path();
            match entry.metadata() {
                Ok(meta) if meta.is_file() => {}
                Ok(_) => continue,
                Err(_) => return true,
            }
            if skip_public && crate::images::is_public_help_file(&path) {
                continue;
            }
            if file_head_is_not_sealed(&path) {
                return true;
            }
        }
    }
    false
}

/// 按逻辑路径给出 AAD 的资源用途。用途与路径都绑定，不能用「图片」密文替换
/// 同路径形状的 Pi 记录或配置文件。
pub(crate) fn resource_kind_for_path(path: &Path) -> Result<&'static str> {
    let (_, id) = resource_identity(path)?;
    let first = id.split('/').next().unwrap_or_default();
    Ok(match first {
        "images" => "image",
        "mermaid-cache" => "mermaid",
        "pi" => "pi",
        "ime" => "ime",
        "skills" => "skill",
        "logs" => "log",
        "apis" => "api",
        _ => "config",
    })
}

/// 把一个已密封文件解回明文（停用加密用）。已是明文则不动。
fn unseal_plaintext_file(handle: &KeyHandle, path: &Path) -> Result<()> {
    let raw = std::fs::read(path).with_context(|| format!("读取 {} 失败", path.display()))?;
    if !is_sealed_file(&raw) {
        return Ok(());
    }
    let kind = resource_kind_for_path(path)?;
    let plain = open_path_bytes(handle, path, kind, &raw)?;
    storage::atomic_write(path, |file| {
        use std::io::Write as _;
        file.write_all(&plain)?;
        Ok(())
    })
}

/// 把一个文件迁到当前 GWA2；明文与 GWA1 都读取后原子替换，GWA2 保持不变。
/// 失败时保留原文件并报错，下次解锁继续迁移。
fn seal_plaintext_file(path: &Path, key: &[u8; DEK_LEN]) -> Result<()> {
    let raw = std::fs::read(path).with_context(|| format!("读取 {} 失败", path.display()))?;
    if is_current_resource_file(&raw) {
        return Ok(());
    }
    let handle = key_handle().context("资源迁移需要固定槽位密钥句柄")?;
    anyhow::ensure!(handle.dek() == key, "资源迁移密钥上下文发生变化");
    let kind = resource_kind_for_path(path)?;
    let plain = if raw.starts_with(FILE_MAGIC) {
        open_path_bytes(&handle, path, kind, &raw)?
    } else {
        raw
    };
    let sealed = seal_path_bytes(&handle, path, kind, &plain)?;
    storage::atomic_write(path, |file| {
        use std::io::Write as _;
        file.write_all(&sealed)?;
        Ok(())
    })
}

/// 文件是否带密封魔数。这个函数服务于「能否安全当作无加密数据」的探测：
/// 不存在才是 false；存在但读不出来时保守返回 true，绝不放行重置或明文降级。
fn sealed_header_at(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let mut head = [0u8; FILE_MAGIC.len()];
    use std::io::Read as _;
    match file.read(&mut head) {
        Ok(n) => is_sealed_file(&head[..n]),
        Err(_) => true,
    }
}

/// 动态文件目录里是否存在密封数据；目录或条目读不出来同样按有数据处理。
fn directory_has_encrypted_data(path: PathBuf) -> bool {
    let mut pending = vec![path];
    let mut visited = 0usize;
    while let Some(dir) = pending.pop() {
        let entries = match read_dir_if_exists(&dir) {
            Ok(Some(entries)) => entries,
            Ok(None) => continue,
            Err(_) => return true,
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => return true,
            };
            visited += 1;
            if visited > 200_000 {
                return true;
            }
            let path = entry.path();
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() => continue,
                Ok(meta) if meta.is_dir() => pending.push(path),
                Ok(meta) if meta.is_file() => {
                    if sealed_header_at(&path) {
                        return true;
                    }
                }
                Ok(_) => {}
                Err(_) => return true,
            }
        }
    }
    false
}

/// 这个槽位在磁盘上是否已有**密文形态**的数据（配置、库或动态资源文件）。
///
/// 用途：锁文件丢失/损坏、或用户选「暂不设置」时，若磁盘上已有加密数据，
/// 就绝不能以没有密钥的工作区进入——旧实现会在那种状态下把密封配置当损坏
/// 改名，再以明文写回默认配置（静默降级），并且看起来「数据全没了」。
/// 这里只做文件头嗅探，不打开数据库。
pub(crate) fn slot_has_encrypted_data(slot: DataSlot) -> bool {
    // 先看「明确的密文证据」：密封的配置/密钥文件、密封的动态目录（apis/pi/ime/
    // skills/logs 与图片/图表缓存）。
    let sealed_names = SEALED_FILE_NAMES
        .iter()
        .skip(1)
        .any(|name| storage::slot_file_for(slot, name).is_ok_and(|path| sealed_header_at(&path)));
    let sealed_dirs = ["apis", "pi", "ime", "skills", "logs"]
        .into_iter()
        .any(|name| storage::slot_subdir_for(slot, name).is_ok_and(directory_has_encrypted_data));
    let sealed_resources = resource_dirs().into_iter().any(|(name, _)| {
        storage::slot_subdir_for(slot, &name).is_ok_and(directory_has_encrypted_data)
    });
    let vault_file_exists = storage::config_dir()
        .map(|dir| dir.join(VAULT_FILE).exists())
        .unwrap_or(false);
    if sealed_names || sealed_dirs || sealed_resources {
        return true;
    }
    if storage::slot_file_for(slot, "config.json").is_ok_and(|path| sealed_header_at(&path)) {
        return true;
    }
    // 库文件「读不出明文头」本身不足以证明是加密库：普通模式下坏块/第三方工具
    // 改过的库也会命中，导致启动进入「锁文件丢失」死局。只有同时存在锁文件时，
    // 才把非明文库算作加密数据；库证据单独出现时按损坏处理，仍走普通启动并
    // 如实报「稿件库打开失败」。
    if vault_file_exists
        && let Ok(path) = storage::slot_file_for(slot, "manuscripts.db")
        && std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 0)
        && !file_head_is_plaintext_db(&path)
    {
        return true;
    }
    false
}

/// 这个槽位还有没有「需要搬进密文」的明文数据？闸门解锁前先问它：
/// 没有就直接进工作区，不显示「正在加密」卡片、不起后台线程。
/// 只做文件头嗅探（十几字节），不打开数据库——解锁路径每次都要问形态，
/// 大库每次跑完整性扫描太重。
pub(crate) fn slot_needs_encryption(slot: DataSlot) -> bool {
    if let Ok(path) = storage::slot_file_for(slot, "manuscripts.db")
        && path.is_file()
        && file_head_is_plaintext_db(&path)
    {
        return true;
    }
    let sealed_names_pending = SEALED_FILE_NAMES.iter().any(|name| {
        storage::slot_file_for(slot, name)
            .map(|path| path.is_file() && file_head_is_not_sealed(&path))
            .unwrap_or(false)
    });
    if sealed_names_pending {
        return true;
    }
    // 接口定义目录与旧接口/配置副本：文件名是动态的，逐个嗅探文件头。
    if let Ok(dir) = storage::slot_file_for(slot, "apis") {
        match read_dir_if_exists(&dir) {
            Ok(Some(entries)) => {
                for entry in entries {
                    let Ok(entry) = entry else {
                        return true;
                    };
                    let path = entry.path();
                    match entry.metadata() {
                        Ok(meta) if meta.is_file() && file_head_is_not_sealed(&path) => {
                            return true;
                        }
                        Ok(_) => {}
                        Err(_) => return true,
                    }
                }
            }
            Ok(None) => {}
            Err(_) => return true,
        }
    }
    if let Ok(dir) = storage::config_dir() {
        match read_dir_if_exists(&dir) {
            Ok(Some(entries)) => {
                for entry in entries {
                    let Ok(entry) = entry else {
                        return true;
                    };
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if storage::slot_relative_name(&name, slot).is_some_and(|name| {
                        name.starts_with("config.json.") || name.ends_with("apis.json.bak")
                    }) && file_head_is_not_sealed(&entry.path())
                    {
                        return true;
                    }
                }
            }
            Ok(None) => {}
            Err(_) => return true,
        }
    }
    if resource_dirs_need_encryption(slot) {
        return true;
    }
    ["pi", "ime", "skills", "logs"].into_iter().any(|name| {
        storage::slot_subdir_for(slot, name)
            .map(|dir| tree_needs_migration(&dir))
            .unwrap_or(true)
    })
}

/// 保守探测私有目录：明文、GWA1、读取错误都意味着必须进入迁移流程。
fn tree_needs_migration(root: &Path) -> bool {
    if !root.exists() {
        return false;
    }
    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => return true,
        };
        for entry in entries {
            let Ok(entry) = entry else { return true };
            visited += 1;
            if visited > 200_000 {
                return true;
            }
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                return true;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file()
                && !is_transient_private_file(&path)
                && file_head_is_not_sealed(&path)
            {
                return true;
            }
        }
    }
    false
}

/// 库文件头嗅探：`SQLite format 3` 魔数 = 明文库，需要迁移；文件不存在或
/// 已是密文 = 不需要。读不出文件头按明文对待（迁移会给出明确的损坏错误）。
fn file_head_is_plaintext_db(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let mut head = [0u8; 16];
    use std::io::Read as _;
    let n = match file.read(&mut head) {
        Ok(n) => n,
        Err(_) => return true,
    };
    if n == 0 {
        // 空库：开库时带 key 会按密文惰性创建，没有要搬的数据。
        return false;
    }
    n < 16 || head.starts_with(b"SQLite format 3")
}

/// 小文件嗅探：名单内文件存在、头又不是密封魔数 = 还没密封。
/// 存在但读不出来也按待迁移处理，让实际迁移返回错误，不能静默跳过。
fn file_head_is_not_sealed(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let mut head = [0u8; 4];
    use std::io::Read as _;
    match file.read(&mut head) {
        Ok(n) => n == 0 || !is_current_resource_file(&head[..n]),
        Err(_) => true,
    }
}

/// 停用的最后一步：把当前槽位的密文数据解回明文（库导出、小文件解封）。
pub(crate) fn decrypt_slot_files() -> Result<()> {
    migrate_database(&storage::manuscript_db_path()?, Migration::ToPlain)?;
    unseal_resource_dirs()?;
    let handle = key_handle().context("停用加密需要当前槽位密钥")?;
    // 与 ensure_slot_encrypted 的覆盖面必须对称：pi/ime/skills/logs、
    // apis/*.openapi.json、根目录的 config.json 副本与 apis.json.bak。
    // 少解一类，停用后就会留下「没有 DEK 的密文」并在下次启动进入锁丢失死局。
    for name in ["pi", "ime", "skills", "logs"] {
        let dir = storage::slot_subdir(name)?;
        for_each_regular_file(&dir, |path| unseal_plaintext_file(&handle, path))?;
    }
    if let Ok(dir) = storage::slot_subdir("apis")
        && let Some(entries) = read_dir_if_exists(&dir)?
    {
        for entry in entries {
            let entry = entry.context("读取接口定义目录条目失败")?;
            let path = entry.path();
            if entry
                .metadata()
                .with_context(|| format!("读取 {} 元数据失败", path.display()))?
                .is_file()
            {
                unseal_plaintext_file(&handle, &path)?;
            }
        }
    }
    if let Ok(dir) = storage::config_dir()
        && let Some(entries) = read_dir_if_exists(&dir)?
    {
        let slot = storage::current_slot();
        for entry in entries {
            let entry = entry.context("读取配置目录条目失败")?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if storage::slot_relative_name(&name, slot).is_some_and(|name| {
                name.starts_with("config.json.") || name.ends_with("apis.json.bak")
            }) {
                unseal_plaintext_file(&handle, &entry.path())?;
            }
        }
    }
    for name in SEALED_FILE_NAMES {
        let path = storage::slot_file(name)?;
        if path.is_file() {
            unseal_plaintext_file(&handle, &path)?;
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
        // 尽量把 WAL 里的页并回主库，减少下面清理时残留的明文页；有别的连接时
        // 这一步会失败，按尽力而为处理，后面仍会覆写删除 sidecar。
        let _ = src.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
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
        // sidecar 跟着**原名**走：rename 只挪了主库文件，`path-wal` / `path-shm`
        // 还在原处，里面可能残留迁移前的明文页（`leftover-wal` 从不出现）。
        let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
        if sidecar.exists() {
            overwrite_then_delete(&sidecar);
        }
        let old_sidecar = PathBuf::from(format!("{}{suffix}", leftover.display()));
        if old_sidecar.exists() {
            overwrite_then_delete(&old_sidecar);
        }
    }
    Ok(())
}

/// 源库是否已经是目标形态：带 key 能读 sqlite_master = 我们的密文；不带 key 能读 = 明文。
/// 两种读法都只碰第一页——`cipher_integrity_check` 会扫全库验 HMAC，解锁路径
/// 每次都要问形态，大库扫不起；第一页的 HMAC 校验已足够判定「是不是这把 key 的库」。
fn database_matches(path: &Path, encrypted: bool) -> Result<bool> {
    let key = key_dek()?;
    let check_with_key = |active: bool| -> bool {
        let Ok(conn) = rusqlite::Connection::open(path) else {
            return false;
        };
        if apply_key_with(&conn, &key, active).is_err() {
            return false;
        }
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .is_ok()
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
pub(crate) fn overwrite_then_delete(path: &Path) {
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
/// 标记明文格式：版本 + 当前真实槽 `wrapped` 指纹。指纹绑住「这一代锁封套」，
/// 改密后旧标记自动失效，避免把旧请求（或备份里翻出来的旧标记）当新请求重放。
const DISABLE_MARKER_VERSION: &str = "gongwen-vault-disable-v1";

/// 标记明文：只有拿得到当前 DEK 的调用方（已解锁）才写得出来。
fn disable_marker_plaintext() -> Result<Vec<u8>> {
    let vault = Vault::load()?;
    Ok(format!("{DISABLE_MARKER_VERSION}\n{}", vault.data.slots[0].wrapped).into_bytes())
}

/// 登记「停用密码保护」：标记用当前 DEK 密封。离线攻击者能写配置目录却拿不到
/// DEK，伪造不出可用的标记——旧实现只写一个明文文件，谁都能植入它让下次解锁
/// 自动把全部数据解成明文并删掉锁封套。
pub(crate) fn request_disable() -> Result<()> {
    if storage::current_slot() != DataSlot::Real {
        bail!("诱饵系统里不能停用密码保护");
    }
    let handle = key_handle().context("停用申请需要当前数据密钥")?;
    let plain = disable_marker_plaintext()?;
    let sealed = seal_bytes(handle.dek(), &plain)?;
    let path = storage::slot_file(DISABLE_PENDING_FILE)?;
    storage::atomic_write(&path, |file| {
        use std::io::Write as _;
        file.write_all(&sealed)?;
        Ok(())
    })
}

pub(crate) fn cancel_disable() {
    if let Ok(path) = storage::slot_file(DISABLE_PENDING_FILE) {
        let _ = std::fs::remove_file(path);
    }
}

/// 标记文件是否存在。**只给锁屏文案看**，不能当授权依据：文件的真伪要靠
/// [`disable_pending`] 用解锁后的 DEK 认证。
pub(crate) fn disable_marker_present() -> bool {
    storage::slot_file(DISABLE_PENDING_FILE)
        .map(|path| path.exists())
        .unwrap_or(false)
}

/// 停用在途：用刚解锁的 DEK 解标记，版本与 `wrapped` 指纹都要对得上。
/// 旧明文标记一律不认（视为没有请求），解不开也视为没有请求。
pub(crate) fn disable_pending(dek: &[u8; DEK_LEN]) -> bool {
    let Ok(path) = storage::slot_file(DISABLE_PENDING_FILE) else {
        return false;
    };
    let Ok(raw) = std::fs::read(&path) else {
        return false;
    };
    if !is_sealed_file(&raw) {
        return false;
    }
    let Ok(plain) = open_bytes(dek, &raw) else {
        return false;
    };
    disable_marker_plaintext().is_ok_and(|expected| plain == expected)
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
    // 长度已确认是偶数，as_chunks 的余数部分必为空。
    for pair in digits.as_chunks::<2>().0 {
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

    /// 测试用的合规密码：长度大于 10 且覆盖三类字符（应用锁的强度规则）。
    const REAL_PW: &str = "Test#Real2026Lock";
    const OLD_REAL_PW: &str = "Test#Old2026Lock";
    const NEW_REAL_PW: &str = "Test#New2026Lock";
    const DECOY_PW: &str = "Test#Decoy2026Cloud";
    const NEW_DECOY_PW: &str = "Test#NewDecoy2026Cloud";

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
        Vault::create(REAL_PW, None, false).unwrap();
        assert_eq!(Vault::probe(), Probe::Locked);
        drop(dir);
        reset();
    }

    #[test]
    fn real_and_decoy_passwords_open_their_own_slots() {
        let dir = use_temp_dir();
        let (mut vault, unlocked) = Vault::create(REAL_PW, Some(DECOY_PW), false).unwrap();
        let real_dek = *unlocked.unwrap().dek;
        let unlocked = vault.unlock(REAL_PW).unwrap();
        assert_eq!(unlocked.slot, DataSlot::Real);
        assert_eq!(*unlocked.dek, real_dek, "主密码派生的 DEK 必须稳定");
        assert_eq!(vault.unlock(DECOY_PW).unwrap().slot, DataSlot::Decoy);
        assert!(vault.unlock("不对").is_none());
        drop(dir);
        reset();
    }

    #[test]
    fn wrong_passwords_are_indistinguishable_from_missing_slots() {
        // 没有第二密码的锁：错的主密码、对的哑槽密码（不存在）、随机串——
        // 解锁结果一律是 None，UI 只能给出统一文案。
        let dir = use_temp_dir();
        let (mut vault, _) = Vault::create(REAL_PW, None, false).unwrap();
        assert!(vault.unlock(REAL_PW).is_some());
        assert!(vault.unlock("任意猜测").is_none());
        drop(dir);
        reset();
    }

    #[test]
    fn seal_shapes_are_constant_whether_or_not_decoy_is_set() {
        let a = use_temp_dir();
        Vault::create(REAL_PW, None, false).unwrap();
        let without = std::fs::read_to_string(Vault::path().unwrap()).unwrap();
        reset();
        drop(a);

        let b = use_temp_dir();
        Vault::create(REAL_PW, Some(DECOY_PW), false).unwrap();
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
        let (mut vault, _) = Vault::create(REAL_PW, Some(DECOY_PW), false).unwrap();
        assert_eq!(vault.unlock(DECOY_PW).unwrap().slot, DataSlot::Decoy);
        vault.clear_decoy().unwrap();
        assert!(vault.unlock(DECOY_PW).is_none(), "清除后旧第二密码失效");
        assert!(vault.unlock(REAL_PW).is_some());
        vault
            .set_password(DataSlot::Decoy, NEW_DECOY_PW, None)
            .unwrap();
        assert!(vault.unlock(NEW_DECOY_PW).is_some());
        drop(dir);
        reset();
    }

    #[test]
    fn decoy_password_cannot_equal_real_password() {
        let dir = use_temp_dir();
        let (mut vault, _) = Vault::create(REAL_PW, None, false).unwrap();
        assert!(vault.set_password(DataSlot::Decoy, REAL_PW, None).is_err());
        drop(dir);
        reset();
    }

    #[test]
    fn create_rejects_same_password_pair_and_short_passwords() {
        let dir = use_temp_dir();
        assert!(Vault::create(REAL_PW, Some(REAL_PW), false).is_err());
        assert!(Vault::create("短", None, false).is_err());
        // 太短、只有一类字符、常见口令一律拒收。
        assert!(Vault::create("Ab1!efgh", None, false).is_err());
        assert!(Vault::create("abcdefghijk", None, false).is_err());
        drop(dir);
        reset();
    }

    #[test]
    fn changing_real_password_keeps_the_same_dek() {
        let dir = use_temp_dir();
        let (mut vault, unlocked) = Vault::create(OLD_REAL_PW, None, false).unwrap();
        let old_dek = *unlocked.unwrap().dek;
        let rewrapped = vault
            .set_password(DataSlot::Real, NEW_REAL_PW, Some(&old_dek))
            .unwrap();
        assert_eq!(*rewrapped.dek, old_dek, "改密只换包裹，DEK 不动");
        assert!(vault.unlock(OLD_REAL_PW).is_none());
        assert_eq!(*vault.unlock(NEW_REAL_PW).unwrap().dek, old_dek);
        drop(dir);
        reset();
    }

    #[test]
    fn reset_file_removes_the_vault_and_keeps_data_files() {
        let dir = use_temp_dir();
        Vault::create(REAL_PW, None, false).unwrap();
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
        Vault::create(REAL_PW, None, false).unwrap();
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
        let (real_slot, _) = SlotSeal::sealed(REAL_PW, None).unwrap();
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
        let unlocked = vault.unlock(REAL_PW).expect("v1 主密码应能解锁");
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
        let second = again.unlock(REAL_PW).expect("v2 升级后仍应能解锁");
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
    fn gwa2_binds_slot_kind_and_logical_path_and_reads_legacy_gwa1() {
        let dir = use_temp_dir();
        set_key_handle([7; DEK_LEN], true);
        let handle = key_handle().unwrap();
        let path = crate::storage::config_dir()
            .unwrap()
            .join("pi/workspaces/doc-1/run-1/workspace/work/final.md");
        let plain = "机密正文：不得明文落盘".as_bytes();
        write_path(&path, "pi", plain).unwrap();
        let sealed = std::fs::read(&path).unwrap();
        assert!(sealed.starts_with(RESOURCE_MAGIC));
        assert!(!sealed.windows(plain.len()).any(|part| part == plain));
        assert_eq!(read_path(&path, "pi", 1024).unwrap(), plain);
        let other = path.with_file_name("other.md");
        assert!(open_path_bytes(&handle, &other, "pi", &sealed).is_err());
        assert!(open_path_bytes(&handle, &path, "log", &sealed).is_err());

        // GWA1 只读兼容；下一次解锁迁移会升级为 GWA2。
        std::fs::write(&other, seal_bytes(handle.dek(), plain).unwrap()).unwrap();
        assert_eq!(read_path(&other, "pi", 1024).unwrap(), plain);
        seal_plaintext_file(&other, handle.dek()).unwrap();
        assert!(std::fs::read(&other).unwrap().starts_with(RESOURCE_MAGIC));

        let decoy = dir.path().join("alt-pi/workspaces/doc-2/run-2/run.json");
        // 不改进程级当前槽位：路径本身决定逻辑槽，测试句柄显式绑定诱饵槽，
        // 避免并行测试里的全局槽位切换干扰其他迁移用例。
        assert!(seal_path_bytes(&handle, &decoy, "pi", plain).is_err());
        set_key_handle_for_slot([8; DEK_LEN], true, DataSlot::Decoy);
        write_path(&decoy, "pi", plain).unwrap();
        assert_eq!(read_path(&decoy, "pi", 1024).unwrap(), plain);
        drop(dir);
        reset();
    }

    /// KeyHandle 的槽位必须由调用方显式给出：即使全局 current_slot 还是 Real，
    /// 诱饵槽句柄也能正常读写 alt-* 资源；反过来用 Real 句柄写 alt 路径必须被拒。
    #[test]
    fn explicit_slot_handle_is_independent_of_global_slot() {
        let dir = use_temp_dir();
        // 默认全局槽位是 Real；这里显式给诱饵槽句柄。
        set_key_handle_for_slot([3; DEK_LEN], true, DataSlot::Decoy);
        let alt = dir.path().join("alt-pi/workspaces/doc-1/run-1/record.json");
        write_path(&alt, "pi", "诱饵内容".as_bytes()).unwrap();
        assert!(std::fs::read(&alt).unwrap().starts_with(RESOURCE_MAGIC));
        assert_eq!(read_path(&alt, "pi", 1024).unwrap(), "诱饵内容".as_bytes());
        // 同一句柄写真实槽路径必须拒绝（不能拿另一槽密钥封真槽文件）。
        let real = dir.path().join("pi/workspaces/doc-1/run-1/record.json");
        assert!(write_path(&real, "pi", "真实内容".as_bytes()).is_err());
        drop(dir);
        reset();
    }

    /// 停用加密必须与启用覆盖面对称：apis 定义与根目录配置副本也要解回明文，
    /// 否则会留下「没有 DEK 的密文」，下次启动进锁丢失死局。
    #[test]
    fn disable_decrypts_apis_and_root_copies() {
        let dir = use_temp_dir();
        set_key_handle([5; DEK_LEN], true);
        let api = crate::storage::slot_subdir("apis").unwrap();
        std::fs::create_dir_all(&api).unwrap();
        let api_file = api.join("服务.openapi.json");
        write_path(&api_file, "api", br#"{"openapi":"3.0.0"}"#).unwrap();
        let copy = crate::storage::config_dir()
            .unwrap()
            .join("config.json.损坏-20260101-000000");
        write_path(&copy, "config", b"{}").unwrap();
        assert!(is_sealed_file(&std::fs::read(&api_file).unwrap()));
        assert!(is_sealed_file(&std::fs::read(&copy).unwrap()));
        assert!(slot_has_encrypted_data(DataSlot::Real));

        decrypt_slot_files().unwrap();
        assert_eq!(std::fs::read(&api_file).unwrap(), br#"{"openapi":"3.0.0"}"#);
        assert_eq!(std::fs::read(&copy).unwrap(), b"{}");
        assert!(
            !slot_has_encrypted_data(DataSlot::Real),
            "解封后不应再有密文证据"
        );
        drop(dir);
        reset();
    }

    /// 迁移与探测跳过资源目录里的符号链接，不能把链接目标读进来再替换链接。
    #[cfg(unix)]
    #[test]
    fn resource_migration_skips_symlinks() {
        let dir = use_temp_dir();
        set_key_handle([11; DEK_LEN], true);
        let images = crate::images::image_dir().unwrap();
        std::fs::create_dir_all(&images).unwrap();
        let outside = dir.path().join("outside.png");
        std::fs::write(&outside, b"secret-outside").unwrap();
        let link = images.join("链接.png");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        ensure_slot_encrypted().unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&outside).unwrap(), b"secret-outside");
        drop(dir);
        reset();
    }

    /// 逻辑资源 ID 对同一物理文件稳定：配置目录经符号链接访问时，AAD 仍与
    /// 真实路径一致（Node host 也按 realpath 口径计算）。
    #[cfg(unix)]
    #[test]
    fn resource_identity_is_stable_across_symlinked_config_dir() {
        let real = tempfile::tempdir().unwrap();
        let link_parent = tempfile::tempdir().unwrap();
        let link = link_parent.path().join("config-link");
        std::os::unix::fs::symlink(real.path(), &link).unwrap();
        crate::storage::set_test_config_dir(Some(link.clone()));
        set_key_handle([12; DEK_LEN], true);
        let via_link = link.join("pi/workspaces/doc-1/run-1/run.json");
        write_path(&via_link, "pi", b"hello").unwrap();
        assert_eq!(read_path(&via_link, "pi", 1024).unwrap(), b"hello");
        // 用真实路径读同一份密文也必须能解开（AAD 相同）。
        let via_real = real.path().join("pi/workspaces/doc-1/run-1/run.json");
        assert_eq!(read_path(&via_real, "pi", 1024).unwrap(), b"hello");
        crate::storage::set_test_config_dir(None);
        reset();
    }

    #[test]
    fn node_gwa2_fixed_vector_decrypts_in_rust() {
        let dir = use_temp_dir();
        let dek: [u8; DEK_LEN] = std::array::from_fn(|index| index as u8);
        set_key_handle(dek, true);
        let path = dir.path().join("pi/runs/run-1/final.md");
        // pi-host/test/encryption.test.mjs：nonce 00..0b，明文为「# 保密正文\n」。
        let sealed = hex_decode_vec("47574132000102030405060708090a0b642232a458006d9d6bec346d276e72cd4dea0db75b75f3a33f98214e5db4d3").unwrap();
        let plain = open_path_bytes(&key_handle().unwrap(), &path, "pi", &sealed).unwrap();
        assert_eq!(plain, "# 保密正文\n".as_bytes());
        drop(dir);
        reset();
    }

    #[test]
    fn sealed_file_limit_applies_to_plaintext_not_the_envelope() {
        let dir = use_temp_dir();
        set_key_handle([9; DEK_LEN], true);
        let path = crate::storage::slot_subdir("logs")
            .unwrap()
            .join("crash-test.log");
        let plain = vec![b'x'; 1024];
        write_path(&path, "log", &plain).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 1056);
        assert_eq!(read_path(&path, "log", 1024).unwrap(), plain);
        assert!(read_path(&path, "log", 1023).is_err());
        drop(dir);
        reset();
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

        let (_vault, unlocked) = Vault::create(REAL_PW, None, false).unwrap();
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
            open_path_bytes(
                &key_handle().unwrap(),
                &crate::storage::config_path().unwrap(),
                "config",
                &config,
            )
            .unwrap(),
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
        let (_vault, unlocked) = Vault::create(REAL_PW, None, false).unwrap();
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

    #[test]
    fn slot_needs_encryption_sniffs_heads_per_slot() {
        let dir = use_temp_dir();
        // 什么都没有：不需要。
        assert!(!slot_needs_encryption(DataSlot::Real));
        // 明文库 + 明文 config：需要。
        plain_database();
        std::fs::write(crate::storage::config_path().unwrap(), b"{}").unwrap();
        assert!(slot_needs_encryption(DataSlot::Real));
        // 迁移完成后：不需要。
        let (_vault, unlocked) = Vault::create(REAL_PW, None, false).unwrap();
        set_key_handle(*unlocked.unwrap().dek, false);
        ensure_slot_encrypted().unwrap();
        assert!(
            !slot_needs_encryption(DataSlot::Real),
            "已迁移的槽位不该再显示「正在加密」"
        );
        // 两套密码各自只看自己槽位：真槽已密文，诱饵槽空文件也不需要。
        assert!(!slot_needs_encryption(DataSlot::Decoy));
        // 诱饵槽有自己的明文文件时独立判定，不影响真槽。
        std::fs::write(
            crate::storage::slot_file_for(DataSlot::Decoy, "config.json").unwrap(),
            b"{}",
        )
        .unwrap();
        assert!(slot_needs_encryption(DataSlot::Decoy));
        assert!(
            !slot_needs_encryption(DataSlot::Real),
            "诱饵的明文文件不该让真槽显示「正在加密」"
        );
        drop(dir);
        reset();
    }
    /// 停用标记必须用解锁后的 DEK 认证：旧式明文标记（离线可伪造）一律不认。
    #[test]
    fn disable_marker_must_be_authenticated() {
        use std::io::Write as _;
        let dir = use_temp_dir();
        let (_vault, unlocked) = Vault::create(REAL_PW, None, false).unwrap();
        let dek = *unlocked.unwrap().dek;
        set_key_handle(dek, true);
        let path = crate::storage::slot_file(DISABLE_PENDING_FILE).unwrap();
        // 伪造的旧明文标记：内容对、格式旧，必须视为没有请求。
        crate::storage::atomic_write(&path, |file| {
            file.write_all(b"disable-pending")?;
            Ok(())
        })
        .unwrap();
        assert!(!disable_pending(&dek), "明文标记不能触发停用");
        assert!(disable_marker_present(), "文件存在仍要能在锁屏提示");

        // 正常登记一份：只有同一把 DEK 能认证通过。
        request_disable().unwrap();
        assert!(disable_pending(&dek));
        assert!(!disable_pending(&[9u8; DEK_LEN]), "别的密钥解不开标记");
        cancel_disable();
        assert!(!disable_pending(&dek));
        assert!(!disable_marker_present());
        drop(dir);
        reset();
    }

    /// 启用加密时把风格档案、接口定义与旧接口兜底文件一起密封，且幂等。
    #[test]
    fn ensure_slot_encrypted_seals_styles_and_api_definitions() {
        let dir = use_temp_dir();
        set_key_handle([3u8; DEK_LEN], true);
        let styles = crate::storage::slot_file("styles.json").unwrap();
        std::fs::write(&styles, br#"{"styles":[]}"#).unwrap();
        let apis = crate::storage::slot_subdir("apis").unwrap();
        std::fs::create_dir_all(&apis).unwrap();
        let api = apis.join("示例.openapi.json");
        std::fs::write(&api, br#"{"openapi":"3.0.0"}"#).unwrap();
        let legacy = crate::storage::config_dir().unwrap().join("apis.json.bak");
        std::fs::write(&legacy, br#"{"legacy":true}"#).unwrap();

        assert!(slot_needs_encryption(DataSlot::Real));
        ensure_slot_encrypted().unwrap();
        for path in [&styles, &api, &legacy] {
            let raw = std::fs::read(path).unwrap();
            assert!(is_sealed_file(&raw), "{} 应已密封", path.display());
        }
        assert!(!slot_needs_encryption(DataSlot::Real));
        // 幂等：再跑一次不重复加密、不破坏已封文件。
        ensure_slot_encrypted().unwrap();
        assert!(is_sealed_file(&std::fs::read(&styles).unwrap()));
        drop(dir);
        reset();
    }

    /// 启用加密时把用户图片、PDF 插图与 Mermaid 缓存一起密封；随包帮助截图
    /// 保持明文（诱饵槽缺图时要按字节比对回落）。停用时全部解回明文。
    #[test]
    fn ensure_slot_encrypted_seals_user_resources_but_not_public_help() {
        let dir = use_temp_dir();
        set_key_handle([7u8; DEK_LEN], true);
        let images = crate::images::image_dir().unwrap();
        std::fs::create_dir_all(&images).unwrap();
        let user = images.join("20260101_扫描件.pdf");
        std::fs::write(&user, b"%PDF-user").unwrap();
        let (help_name, help_bytes) = crate::help::content::IMAGES[0];
        let help = images.join(help_name);
        std::fs::write(&help, help_bytes).unwrap();
        let cache = crate::storage::slot_subdir(crate::storage::MERMAID_CACHE_DIR).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        let diagram = cache.join("abc123.svg");
        std::fs::write(&diagram, b"<svg/>").unwrap();

        assert!(slot_needs_encryption(DataSlot::Real));
        ensure_slot_encrypted().unwrap();
        assert!(is_sealed_file(&std::fs::read(&user).unwrap()));
        assert_eq!(
            std::fs::read(&help).unwrap(),
            help_bytes,
            "公开帮助截图不参与加密"
        );
        assert!(is_sealed_file(&std::fs::read(&diagram).unwrap()));
        assert!(!slot_needs_encryption(DataSlot::Real));

        decrypt_slot_files().unwrap();
        assert_eq!(std::fs::read(&user).unwrap(), b"%PDF-user");
        assert!(!is_sealed_file(&std::fs::read(&diagram).unwrap()));
        drop(dir);
        reset();
    }

    /// 磁盘上已有密文数据时，闸门必须能识别出来（锁文件丢失不能直接进工作区）。
    #[test]
    fn encrypted_data_detection_covers_config_and_database() {
        let dir = use_temp_dir();
        assert!(!slot_has_encrypted_data(DataSlot::Real));
        let sealed = seal_bytes(&[5u8; DEK_LEN], b"{}").unwrap();
        std::fs::write(
            crate::storage::slot_file_for(DataSlot::Real, "config.json").unwrap(),
            sealed,
        )
        .unwrap();
        assert!(slot_has_encrypted_data(DataSlot::Real));
        assert!(!slot_has_encrypted_data(DataSlot::Decoy));
        // 明文库不算加密数据。
        drop(dir);
        reset();
        let dir = use_temp_dir();
        let path = crate::storage::manuscript_db_path().unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY);")
            .unwrap();
        drop(conn);
        assert!(!slot_has_encrypted_data(DataSlot::Real));
        drop(dir);
        reset();
    }
    /// 动态资源与接口定义即使没有配置/数据库，也足以证明锁文件后面还有加密数据。
    #[test]
    fn encrypted_data_detection_covers_resources_and_api_definitions() {
        for (name, slot) in [
            ("images", DataSlot::Real),
            ("mermaid-cache", DataSlot::Decoy),
            ("apis", DataSlot::Real),
        ] {
            let dir = use_temp_dir();
            let resource_dir = crate::storage::slot_subdir_for(slot, name).unwrap();
            std::fs::create_dir_all(&resource_dir).unwrap();
            std::fs::write(
                resource_dir.join("sealed.bin"),
                seal_bytes(&[13u8; DEK_LEN], b"secret").unwrap(),
            )
            .unwrap();
            assert!(
                slot_has_encrypted_data(slot),
                "{name} 里的密文必须阻止锁文件丢失后的明文重开"
            );
            drop(dir);
            reset();
        }
    }

    /// 资源目录读不出来时必须待迁移并报错，不能跳过目录后宣称加密完成。
    #[test]
    fn unreadable_resource_directory_fails_closed() {
        let dir = use_temp_dir();
        set_key_handle([14u8; DEK_LEN], true);
        let images = crate::images::image_dir().unwrap();
        // 用普通文件占住目录路径：read_dir 会得到确定性的 NotADirectory。
        std::fs::write(&images, b"not a directory").unwrap();

        assert!(slot_needs_encryption(DataSlot::Real));
        assert!(ensure_slot_encrypted().is_err());
        assert_eq!(std::fs::read(&images).unwrap(), b"not a directory");
        drop(dir);
        reset();
    }

    /// 迁移真实槽时不能拿真实 DEK 去封诱饵槽的配置副本（反之亦然）。
    #[test]
    fn ensure_slot_encrypted_only_touches_current_slot_copies() {
        let dir = use_temp_dir();
        set_key_handle([6u8; DEK_LEN], true);
        let real = crate::storage::config_dir()
            .unwrap()
            .join("config.json.身份迁移前备份");
        let decoy = crate::storage::config_dir()
            .unwrap()
            .join("alt-config.json.身份迁移前备份");
        std::fs::write(&real, b"{}").unwrap();
        std::fs::write(&decoy, b"{}").unwrap();
        ensure_slot_encrypted().unwrap();
        assert!(is_sealed_file(&std::fs::read(&real).unwrap()));
        assert_eq!(
            std::fs::read(&decoy).unwrap(),
            b"{}",
            "真实槽迁移不能碰诱饵槽的文件"
        );
        drop(dir);
        reset();
    }
}
