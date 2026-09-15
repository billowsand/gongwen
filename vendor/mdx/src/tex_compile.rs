use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const TECTONIC_PATH_ENV: &str = "MDX_TECTONIC_PATH";
const TECTONIC_BUNDLE_ENV: &str = "MDX_TECTONIC_BUNDLE";

#[derive(Debug, Clone, PartialEq, Eq)]
enum TexEngine {
    BundledTectonic {
        executable: PathBuf,
        bundle: PathBuf,
    },
    SystemTectonic,
    Xelatex,
}

/// Compile a generated `.tex` file to PDF when a supported TeX engine exists.
///
/// This is best-effort for engine discovery: release archives use their bundled
/// Tectonic runtime first, while source installations can fall back to a system
/// `xelatex` or `tectonic`. If none exists, conversion still succeeds and leaves
/// the `.tex` for manual compilation.
pub fn compile_pdf_if_available(tex_path: &Path) -> Result<()> {
    let Some(engine) = detect_engine()? else {
        println!("  未检测到 xelatex 或 tectonic，跳过 PDF 编译");
        return Ok(());
    };

    match engine {
        TexEngine::BundledTectonic { executable, bundle } => {
            compile_with_bundled_tectonic(tex_path, &executable, &bundle)?
        }
        TexEngine::SystemTectonic => compile_with_system_tectonic(tex_path)?,
        TexEngine::Xelatex => compile_with_xelatex(tex_path)?,
    }

    let pdf_path = tex_path.with_extension("pdf");
    if !pdf_path.exists() {
        anyhow::bail!("TeX 编译完成但未找到 PDF: {}", pdf_path.display());
    }

    cleanup_intermediates(tex_path)?;
    Ok(())
}

fn detect_engine() -> Result<Option<TexEngine>> {
    // 发布包内置的 Tectonic + 本地 bundle 最可控，应始终优先于用户系统环境。
    if let Some((executable, bundle)) = discover_bundled_runtime()? {
        return Ok(Some(TexEngine::BundledTectonic { executable, bundle }));
    }

    // 从源码安装或只分发轻量二进制时保留原系统引擎回退。
    if command_available("xelatex") {
        return Ok(Some(TexEngine::Xelatex));
    }
    if command_available("tectonic") {
        return Ok(Some(TexEngine::SystemTectonic));
    }
    Ok(None)
}

fn command_available(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn discover_bundled_runtime() -> Result<Option<(PathBuf, PathBuf)>> {
    let configured_executable = std::env::var_os(TECTONIC_PATH_ENV).map(PathBuf::from);
    let configured_bundle = std::env::var_os(TECTONIC_BUNDLE_ENV).map(PathBuf::from);

    if configured_executable.is_some() || configured_bundle.is_some() {
        let executable = configured_executable.ok_or_else(|| {
            anyhow::anyhow!("设置了 {TECTONIC_BUNDLE_ENV}，但缺少 {TECTONIC_PATH_ENV}")
        })?;
        let bundle = configured_bundle.ok_or_else(|| {
            anyhow::anyhow!("设置了 {TECTONIC_PATH_ENV}，但缺少 {TECTONIC_BUNDLE_ENV}")
        })?;
        return validate_bundled_runtime(executable, bundle).map(Some);
    }

    let executable_dir = std::env::current_exe()
        .context("无法定位 mdx 可执行文件")?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("mdx 可执行文件没有父目录"))?;
    discover_runtime_below(&executable_dir)
}

fn discover_runtime_below(executable_dir: &Path) -> Result<Option<(PathBuf, PathBuf)>> {
    let runtime_dir = executable_dir.join("runtime");
    if !runtime_dir.exists() {
        return Ok(None);
    }

    let executable = runtime_dir.join(tectonic_executable_name());
    let bundle = runtime_dir.join("bundle");
    validate_bundled_runtime(executable, bundle).map(Some)
}

fn validate_bundled_runtime(executable: PathBuf, bundle: PathBuf) -> Result<(PathBuf, PathBuf)> {
    let executable = absolute_path(executable)?;
    let bundle = absolute_path(bundle)?;
    if !executable.is_file() {
        anyhow::bail!("内置 Tectonic 不存在: {}", executable.display());
    }
    if !bundle.is_dir() {
        anyhow::bail!("内置 Tectonic bundle 不存在: {}", bundle.display());
    }
    let digest_path = bundle.join("SHA256SUM");
    if !digest_path.is_file() {
        anyhow::bail!("内置 Tectonic bundle 缺少 SHA256SUM: {}", bundle.display());
    }
    let digest = fs::read_to_string(&digest_path)
        .with_context(|| format!("读取 {} 失败", digest_path.display()))?;
    let digest = digest.trim();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("内置 Tectonic bundle 的 SHA256SUM 无效");
    }
    Ok((executable, bundle))
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()
            .context("无法读取当前目录")?
            .join(path))
    }
}

fn tectonic_executable_name() -> &'static str {
    if cfg!(windows) {
        "tectonic.exe"
    } else {
        "tectonic"
    }
}

fn compile_with_bundled_tectonic(tex_path: &Path, executable: &Path, bundle: &Path) -> Result<()> {
    let dir = output_dir(tex_path);
    let file_name = tex_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("无效的 tex 文件路径: {}", tex_path.display()))?;

    println!("  使用内置 Tectonic 离线编译 PDF...");
    run_command(
        Command::new(executable)
            .current_dir(dir)
            .arg("-X")
            .arg("compile")
            .arg("--bundle")
            .arg(tectonic_bundle_arg(bundle))
            .arg("--only-cached")
            .arg("--untrusted")
            .arg(file_name),
        "内置 tectonic",
    )?;
    println!("  PDF 编译完成");
    Ok(())
}

fn tectonic_bundle_arg(bundle: &Path) -> OsString {
    // Tectonic 0.17 feeds the argument through `url::Url` before trying a local
    // path. A Windows drive prefix such as `C:` is therefore mistaken for a URL
    // scheme unless we supply an explicit file URL.
    #[cfg(windows)]
    {
        OsString::from(format!(
            "file:///{}",
            bundle.to_string_lossy().replace('\\', "/")
        ))
    }

    #[cfg(not(windows))]
    {
        bundle.as_os_str().to_owned()
    }
}

fn compile_with_system_tectonic(tex_path: &Path) -> Result<()> {
    let dir = output_dir(tex_path);
    let file_name = tex_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("无效的 tex 文件路径: {}", tex_path.display()))?;

    println!("  检测到系统 tectonic，正在编译 PDF...");
    run_command(
        Command::new("tectonic")
            .current_dir(dir)
            .arg("-X")
            .arg("compile")
            .arg("--untrusted")
            .arg(file_name),
        "tectonic",
    )?;
    println!("  PDF 编译完成");
    Ok(())
}

fn compile_with_xelatex(tex_path: &Path) -> Result<()> {
    let dir = output_dir(tex_path);
    let file_name = tex_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("无效的 tex 文件路径: {}", tex_path.display()))?;
    let stem = tex_path
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| anyhow::anyhow!("无效的 tex 文件名: {}", tex_path.display()))?;

    println!("  检测到 xelatex，正在编译 PDF...");
    run_xelatex(dir, file_name)?;

    let uses_bibliography = tex_uses_bibliography(tex_path);
    if uses_bibliography {
        if !command_available("bibtex") {
            anyhow::bail!("文档包含参考文献，但系统中未检测到 bibtex");
        }
        println!("  检测到参考文献，正在运行 BibTeX...");
        run_command(Command::new("bibtex").current_dir(dir).arg(stem), "bibtex")?;
    }

    run_xelatex(dir, file_name)?;
    if uses_bibliography {
        run_xelatex(dir, file_name)?;
    }
    println!("  PDF 编译完成");
    Ok(())
}

fn run_xelatex(dir: &Path, file_name: &OsStr) -> Result<()> {
    run_command(
        Command::new("xelatex")
            .current_dir(dir)
            .arg("-interaction=nonstopmode")
            .arg("-halt-on-error")
            .arg(file_name),
        "xelatex",
    )
}

fn run_command(command: &mut Command, label: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("执行 {} 失败", label))?;
    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::bail!(
        "{} 编译失败\nstdout:\n{}\nstderr:\n{}",
        label,
        stdout.trim(),
        stderr.trim()
    )
}

fn tex_uses_bibliography(tex_path: &Path) -> bool {
    fs::read_to_string(tex_path)
        .map(|content| content.contains("\\bibliography{"))
        .unwrap_or(false)
}

fn cleanup_intermediates(tex_path: &Path) -> Result<()> {
    let dir = output_dir(tex_path);
    let stem = tex_path
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| anyhow::anyhow!("无效的 tex 文件名: {}", tex_path.display()))?;

    let generated_extensions = [
        "aux",
        "bbl",
        "bcf",
        "blg",
        "fdb_latexmk",
        "fls",
        "log",
        "out",
        "run.xml",
        "synctex.gz",
        "toc",
        "xdv",
    ];

    for ext in generated_extensions {
        let path = generated_path(dir, stem, ext);
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("删除 {} 失败", path.display()))?;
        }
    }

    Ok(())
}

fn output_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn generated_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    if ext.contains('.') {
        dir.join(format!("{}.{}", stem, ext))
    } else {
        dir.join(stem).with_extension(ext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_dir_defaults_to_current_dir_for_bare_file_name() {
        assert_eq!(output_dir(Path::new("report.tex")), Path::new("."));
        assert_eq!(output_dir(Path::new("./report.tex")), Path::new("."));
        assert_eq!(output_dir(Path::new("out/report.tex")), Path::new("out"));
    }

    #[test]
    fn cleanup_removes_latex_sidecars_for_same_stem() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tex = dir.path().join("report.tex");
        fs::write(&tex, "").unwrap();
        fs::write(dir.path().join("report.aux"), "").unwrap();
        fs::write(dir.path().join("report.log"), "").unwrap();
        fs::write(dir.path().join("report.run.xml"), "").unwrap();
        fs::write(dir.path().join("other.aux"), "").unwrap();
        fs::write(dir.path().join("report.pdf"), "").unwrap();
        fs::write(dir.path().join("md2tex.cls"), "").unwrap();

        cleanup_intermediates(&tex).expect("cleanup");

        assert!(!dir.path().join("report.aux").exists());
        assert!(!dir.path().join("report.log").exists());
        assert!(!dir.path().join("report.run.xml").exists());
        assert!(dir.path().join("other.aux").exists());
        assert!(dir.path().join("report.tex").exists());
        assert!(dir.path().join("report.pdf").exists());
        assert!(dir.path().join("md2tex.cls").exists());
    }

    #[test]
    fn bundled_runtime_requires_complete_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(discover_runtime_below(dir.path()).unwrap(), None);

        let runtime = dir.path().join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(runtime.join(tectonic_executable_name()), "").unwrap();
        let err = discover_runtime_below(dir.path()).unwrap_err();
        assert!(err.to_string().contains("bundle"));

        let bundle = runtime.join("bundle");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(bundle.join("SHA256SUM"), "0".repeat(64)).unwrap();
        let discovered = discover_runtime_below(dir.path()).unwrap().unwrap();
        assert_eq!(
            discovered.0,
            absolute_path(runtime.join(tectonic_executable_name())).unwrap()
        );
        assert_eq!(discovered.1, absolute_path(bundle).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn windows_bundle_path_is_passed_as_file_url() {
        assert_eq!(
            tectonic_bundle_arg(Path::new(r"C:\Program Files\mdx\runtime\bundle")),
            OsString::from("file:///C:/Program Files/mdx/runtime/bundle")
        );
    }
}
