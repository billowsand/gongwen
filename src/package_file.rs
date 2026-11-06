//! 稿件包文件（`.gwx`）：后缀判断、启动参数解析，以及把要导入的包先复制到
//! 程序自己的暂存目录。
//!
//! 新包使用内容与文件名加密的 7z；旧 ZIP 格式的 `.gwx` / `.zip` 仍可导入。
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
/// 容器格式按文件头识别，不依赖后缀；也接受解压工具改名后的 `.7z`。
pub const IMPORT_EXTENSIONS: &[&str] = &[EXTENSION, "zip", "7z"];
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
/// 旧 `.zip` / 通用 `.7z` 后缀改成 `.gwx`，避免新 7z 内容被误标成 ZIP。
pub fn with_package_extension(path: PathBuf) -> PathBuf {
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        if ext.eq_ignore_ascii_case(EXTENSION) {
            return path;
        }
        if ext.eq_ignore_ascii_case("zip") || ext.eq_ignore_ascii_case("7z") {
            return path.with_extension(EXTENSION);
        }
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
    create_private_dir(root).with_context(|| format!("无法创建暂存目录 {}", root.display()))?;
    create_private_dir(&dir).with_context(|| format!("无法创建暂存目录 {}", dir.display()))?;
    let target = dir.join(name);
    let copied = fs::copy(source, &target);
    match copied {
        Ok(bytes) if bytes == meta.len() => {
            // `fs::copy` 会照搬源文件权限，这里统一收紧到仅当前用户可读写。
            #[cfg(unix)]
            if let Err(error) = restrict_to_owner(&target) {
                let _ = fs::remove_dir_all(&dir);
                return Err(error)
                    .with_context(|| format!("无法设置暂存文件权限 {}", target.display()));
            }
            Ok(target)
        }
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

/// 建暂存目录并收紧到仅当前用户：暂存里有用户的稿件包副本。
/// Windows 上没有对应的权限位，只建目录。
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Unix 下把文件收紧到仅当前用户可读写。
#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
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
        assert!(is_package_path(Path::new("新包.7Z")));
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
            PathBuf::from("d/给旧版.gwx")
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

    /// 系统登记（Windows 安装包、Linux 的 MIME 定义与 desktop）与代码里的后缀要对得上，
    /// 文档图标也得在：改了一处忘了另一处，双击就不再打开本程序。
    #[test]
    fn system_registrations_agree_with_the_extension() {
        let mime_type = "application/x-gongwen-package";
        let mime = include_str!("../assets/linux/cn.localtools.GongwenAssistant.xml");
        assert!(mime.contains(&format!(r#"<mime-type type="{mime_type}">"#)));
        assert!(mime.contains(&format!(r#"<glob pattern="*.{EXTENSION}"/>"#)));

        let desktop = include_str!("../assets/linux/cn.localtools.GongwenAssistant.desktop");
        assert!(desktop.contains(&format!("\nMimeType={mime_type};\n")));
        assert!(desktop.contains("\nExec=gongwen-assistant %F\n"));

        let iss = include_str!("../scripts/gongwen-assistant.iss");
        assert!(iss.contains(&format!(r#"Subkey: "Software\Classes\.{EXTENSION}""#)));
        assert!(iss.contains(r#"ValueData: "{app}\gwx-document.ico""#));
        assert!(iss.contains(r#"""{app}\{#MyAppExeName}"" ""%1"""#));
        assert!(!include_bytes!("../assets/doc-icon/gwx-document.ico").is_empty());
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

    #[cfg(unix)]
    #[test]
    fn staged_copy_and_directories_are_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("来函.gwx");
        fs::write(&source, b"PK-content").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap();
        let root = temp.path().join("incoming");
        let staged = stage(&source, &root).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(staged.parent().unwrap()), 0o700);
        assert_eq!(mode(&staged), 0o600);
    }
}
