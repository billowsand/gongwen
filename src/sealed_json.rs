//! 配置目录下 JSON 文件的读写（风格档案、接口配置等）。
//!
//! 从旧 `agent::api` 原样搬来：路径都在配置目录里，写入走
//! `storage::atomic_write`（同目录暂存 + 刷盘 + 原子替换，Unix 下仅本人可读写），
//! 读不到文件按空配置处理、解析失败才报错。旧 `api` 单向 re-export 这里。

use anyhow::Context as _;
use serde::Serialize;
use std::path::PathBuf;

fn store_path(name: &str) -> anyhow::Result<PathBuf> {
    // 槽位感知：真实系统用原名，诱饵系统一律 alt- 前缀——接口密钥、
    // 环境地址这类敏感文件不能跨槽位共享（见 docs/security-lock-design.md）。
    crate::storage::slot_file(name)
}

/// 应用锁启用后要 AEAD 加密的接口/风格文件。
/// **名单只有一份**：`storage::SEALED_JSON_NAMES`，vault 的迁移/擦除名单也由
/// `storage::SEALED_FILE_NAMES` 派生（二期曾因名单不一致漏掉 api-envs/api-tests）。
fn is_sealed_name(name: &str) -> bool {
    crate::storage::SEALED_JSON_NAMES.contains(&name)
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned + Default>(name: &str) -> anyhow::Result<T> {
    let path = store_path(name)?;
    match std::fs::read(&path) {
        Ok(raw) => {
            let bytes = if crate::vault::is_sealed_file(&raw) {
                if !is_sealed_name(name) {
                    return Err(anyhow::anyhow!(
                        "{} 是加密文件但不在密钥名单里",
                        path.display()
                    ));
                }
                let handle = crate::vault::key_handle()
                    .with_context(|| format!("{} 已加密，尚未解锁", path.display()))?;
                crate::vault::open_path_bytes(&handle, &path, "config", &raw)
                    .with_context(|| format!("{} 解密失败", path.display()))?
            } else {
                raw
            };
            serde_json::from_slice::<T>(&bytes)
                .map_err(|e| anyhow::anyhow!("{} 解析失败：{e}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(anyhow::anyhow!("读取 {} 失败：{error}", path.display())),
    }
}

pub(crate) fn write_json<T: Serialize>(name: &str, value: &T) -> anyhow::Result<()> {
    let path = store_path(name)?;
    // 原子写 + 仅本人可读写：这里落的是接口密钥与配置，不能写到一半，
    // 也不能让同机的其他用户读到。
    let bytes = serde_json::to_vec_pretty(value)?;
    if is_sealed_name(name) {
        return crate::vault::write_path(&path, "config", &bytes);
    }
    crate::storage::atomic_write(&path, |file| {
        use std::io::Write as _;
        file.write_all(&bytes)?;
        Ok(())
    })
}
