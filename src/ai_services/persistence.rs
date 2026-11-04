//! 配置目录下 JSON 文件的读写（风格档案、接口配置等）。
//!
//! 从旧 `agent::api` 原样搬来：路径都在配置目录里，写入走
//! `storage::atomic_write`（同目录暂存 + 刷盘 + 原子替换，Unix 下仅本人可读写），
//! 读不到文件按空配置处理、解析失败才报错。旧 `api` 单向 re-export 这里。

use serde::Serialize;
use std::path::PathBuf;

fn store_path(name: &str) -> anyhow::Result<PathBuf> {
    // 槽位感知：真实系统用原名，诱饵系统一律 alt- 前缀——接口密钥、
    // 环境地址这类敏感文件不能跨槽位共享（见 docs/security-lock-design.md）。
    crate::storage::slot_file(name)
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned + Default>(name: &str) -> anyhow::Result<T> {
    let path = store_path(name)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{} 解析失败：{e}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(anyhow::anyhow!("读取 {} 失败：{error}", path.display())),
    }
}

pub(crate) fn write_json<T: Serialize>(name: &str, value: &T) -> anyhow::Result<()> {
    let path = store_path(name)?;
    // 原子写 + 仅本人可读写：这里落的是接口密钥与配置，不能写到一半，
    // 也不能让同机的其他用户读到。
    let text = serde_json::to_string_pretty(value)?;
    crate::storage::atomic_write(&path, |file| {
        use std::io::Write as _;
        file.write_all(text.as_bytes())?;
        Ok(())
    })
}
