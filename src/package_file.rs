//! 稿件包文件（`.gwx`）：后缀判断、启动参数解析，以及把要导入的包先复制到
//! 程序自己的暂存目录。
//!
//! 包内格式就是稿件管理导出的加密 ZIP（见 `manuscript_io`），只是换了专用后缀，
//! 好在系统里登记成「双击用公文助手打开」；旧 `.zip` 稿件包照样能导入。
//! 方案见 `docs/gwx-package-design.md`。
//!
//! 为什么先复制：邮件客户端、即时通讯软件双击附件时给的是它们自己的临时文件，
//! 关掉预览窗口就可能被删；U 盘也可能在预览和确认之间被拔掉。而导入在预览时
//! 读一遍包、确认时还要重读一遍，中间文件没了就会失败。

use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// 稿件包的专用后缀。
pub const EXTENSION: &str = "gwx";
/// 导入时认的后缀：新包 `.gwx` 与旧版导出的 `.zip`。
pub const IMPORT_EXTENSIONS: &[&str] = &[EXTENSION, "zip"];
/// 文件框里的过滤器名。
pub const FILTER_NAME: &str = "公文稿件包";

/// 后缀是不是稿件包（大小写不敏感）。只看后缀，内容对不对由导入时读清单判定。
pub fn is_package_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            IMPORT_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// 导出时保存框给回的路径：用户把后缀删了或改成别的，补上 `.gwx`。
/// 明确写成 `.zip` 的保留——与还没升级的旧版程序互传时要用。
pub fn with_package_extension(path: PathBuf) -> PathBuf {
    if is_package_path(&path) {
        return path;
    }
    // 不用 set_extension：「稿件v1.2」这样的名字会被截成「稿件v1.gwx」。
    let mut name = path.file_name().map(OsString::from).unwrap_or_default();
    name.push(".");
    name.push(EXTENSION);
    path.with_file_name(name)
}

/// 从命令行参数里挑出要导入的稿件包（双击 `.gwx` 时系统把路径作为参数传进来）。
/// 相对路径按 `cwd` 补全；不是稿件包后缀的参数一律忽略。
pub fn startup_paths(args: impl IntoIterator<Item = OsString>, cwd: &Path) -> Vec<PathBuf> {
    args.into_iter()
        .map(PathBuf::from)
        .filter(|path| is_package_path(path))
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        })
        .collect()
}

/// 暂存目录：本用户缓存目录下的 `incoming/`，只有当前用户能写。
pub fn staging_root() -> Result<PathBuf> {
    Ok(crate::storage::cache_dir()?.join("incoming"))
}

/// 把 `source` 复制进 `root` 下一个独立子目录，返回副本路径。副本保留原文件名，
/// 口令框里显示的就是用户双击的那个名字。
pub fn stage(source: &Path, root: &Path) -> Result<PathBuf> {
    let meta = fs::metadata(source).with_context(|| format!("找不到 {}", source.display()))?;
    if !meta.is_file() {
        bail!("{} 不是文件", source.display());
    }
    if meta.len() > crate::file_limits::MAX_PACKAGE_BYTES {
        return Err(
            crate::file_limits::too_large("稿件包", crate::file_limits::MAX_PACKAGE_BYTES).into(),
        );
    }
    let name = source
        .file_name()
        .with_context(|| format!("{} 没有文件名", source.display()))?;
    let dir = root.join(uuid::Uuid::new_v4().simple().to_string());
    fs::create_dir_all(&dir).with_context(|| format!("无法创建暂存目录 {}", dir.display()))?;
    let target = dir.join(name);
    let copied = fs::copy(source, &target);
    match copied {
        Ok(bytes) if bytes == meta.len() => Ok(target),
        Ok(_) => {
            let _ = fs::remove_dir_all(&dir);
            bail!("复制 {} 时文件大小发生变化，请稍后再试", source.display())
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&dir);
            Err(error).with_context(|| format!("无法读取 {}", source.display()))
        }
    }
}

/// 导入用完一个副本后删掉它所在的子目录。只删 `root` 底下的东西，
/// 不是 [`stage`] 给出的路径（比如测试里直接传进来的）什么也不做。
pub fn discard(staged: &Path, root: &Path) {
    if let Some(dir) = staged.parent()
        && dir.parent() == Some(root)
    {
        let _ = fs::remove_dir_all(dir);
    }
}

/// 启动时清掉上次留下的副本（上次退出时还没处理完，或程序被强行结束）。
pub fn clear_staging(root: &Path) {
    let _ = fs::remove_dir_all(root);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_package_extensions_case_insensitively() {
        assert!(is_package_path(Path::new("a/稿件.gwx")));
        assert!(is_package_path(Path::new("稿件.GWX")));
        assert!(is_package_path(Path::new("旧包.zip")));
        assert!(!is_package_path(Path::new("稿件.docx")));
        assert!(!is_package_path(Path::new("gwx")));
    }

    #[test]
    fn export_path_gets_gwx_unless_already_a_package() {
        assert_eq!(
            with_package_extension(PathBuf::from("d/公文稿件")),
            PathBuf::from("d/公文稿件.gwx")
        );
        assert_eq!(
            with_package_extension(PathBuf::from("d/稿件v1.2")),
            PathBuf::from("d/稿件v1.2.gwx")
        );
        assert_eq!(
            with_package_extension(PathBuf::from("d/给旧版.zip")),
            PathBuf::from("d/给旧版.zip")
        );
        assert_eq!(
            with_package_extension(PathBuf::from("d/a.GWX")),
            PathBuf::from("d/a.GWX")
        );
    }

    #[test]
    fn startup_paths_keep_packages_and_resolve_relative() {
        let cwd = Path::new("/work");
        let args = ["--flag", "稿件.gwx", "/abs/旧.zip", "说明.txt"].map(OsString::from);
        assert_eq!(
            startup_paths(args, cwd),
            vec![cwd.join("稿件.gwx"), PathBuf::from("/abs/旧.zip")]
        );
    }

    #[test]
    fn stage_copies_into_its_own_dir_and_discard_removes_it() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("来函.gwx");
        fs::write(&source, b"PK-content").unwrap();
        let root = temp.path().join("incoming");

        let first = stage(&source, &root).unwrap();
        let second = stage(&source, &root).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.file_name().unwrap(), "来函.gwx");
        assert_eq!(fs::read(&first).unwrap(), b"PK-content");

        // 原文件被删掉（邮件客户端关了预览）后副本仍在。
        fs::remove_file(&source).unwrap();
        assert!(first.exists());

        discard(&first, &root);
        assert!(!first.parent().unwrap().exists());
        assert!(second.exists());

        // 不在暂存目录下的路径不动。
        let outside = temp.path().join("别处.gwx");
        fs::write(&outside, b"x").unwrap();
        discard(&outside, &root);
        assert!(outside.exists());

        clear_staging(&root);
        assert!(!root.exists());
    }

    #[test]
    fn stage_rejects_missing_file_and_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("incoming");
        assert!(stage(&temp.path().join("没有.gwx"), &root).is_err());
        let dir = temp.path().join("目录.gwx");
        fs::create_dir(&dir).unwrap();
        assert!(stage(&dir, &root).is_err());
    }
}
