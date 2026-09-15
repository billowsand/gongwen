//! 分章输出：emitter 把各章内容分别产出为部件（`data/`、`appendix/` 下的
//! 子文件），主 tex 用 `\input{...}` 按序引用。这里只负责把部件写盘。

use std::fs;
use std::path::Path;

/// 把 `(相对路径, 内容)` 形式的部件写到输出目录下，按需创建子目录。
pub fn write_parts(out_dir: &Path, parts: &[(String, String)]) -> std::io::Result<()> {
    for (rel, content) in parts {
        let path = out_dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content)?;
    }
    Ok(())
}
