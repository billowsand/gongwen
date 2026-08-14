//! TeX 编译：优先使用应用自带的 Tectonic、离线 bundle 与字体；开发环境再兼容
//! 本机 XeLaTeX/Tectonic。输出 PDF 后清理中间文件。
//! 编译在后台线程执行，主界面通过轮询拿到结果，不会假死。

use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::models::{FontConfig, FontRole};
use crate::portable_runtime::{self, PortableTexRuntime};

static COMPILE_WORKSPACE_ID: AtomicU64 = AtomicU64::new(1);
// Tectonic 首次运行会在共享缓存中生成格式文件；Windows 下并发写同一临时文件会失败。
static PORTABLE_TECTONIC_LOCK: Mutex<()> = Mutex::new(());

/// GUI 程序启动控制台型 TeX 引擎时，Windows 默认会短暂创建命令行窗口。
/// 仅编译子进程使用 `CREATE_NO_WINDOW`；stdout/stderr 仍由父进程捕获用于错误提示。
fn tex_command(program: &Path) -> Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = Command::new(program);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new(program)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TexEngine {
    PortableTectonic(PortableTexRuntime),
    Xelatex(PathBuf),
    Tectonic(PathBuf),
}

/// 发布包自带的 Tectonic 始终优先，保证 bundle 与字体版本固定；开发环境没有
/// 便携运行时时，才兼容本机 XeLaTeX 或 PATH 中的 Tectonic。
fn find_tex_engine() -> Result<Option<TexEngine>> {
    if let Some(runtime) = portable_runtime::find_tex_runtime()? {
        return Ok(Some(TexEngine::PortableTectonic(runtime)));
    }
    Ok(find_xelatex()
        .map(TexEngine::Xelatex)
        .or_else(|| find_command("tectonic").map(TexEngine::Tectonic)))
}

/// 在 PATH 中查找 xelatex，找不到再查 TeX Live / MiKTeX 的常见安装位置。
fn find_xelatex() -> Option<PathBuf> {
    find_command("xelatex").or_else(|| {
        common_xelatex_install_paths()
            .into_iter()
            .find(|candidate| candidate.is_file())
    })
}

/// 沿 PATH 逐个目录查找命令，兼容 Windows 常见的可执行扩展名。
fn find_command(command: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let base = dir.join(command);
        for candidate in [
            base.clone(),
            base.with_extension("exe"),
            base.with_extension("bat"),
            base.with_extension("cmd"),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// TeX Live 与 MiKTeX 的默认安装布局（含用户级 MiKTeX）。
fn common_xelatex_install_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    // TeX Live：C:\texlive\<年份>\bin\windows\xelatex.exe
    if let Ok(read) = std::fs::read_dir(r"C:\texlive") {
        for entry in read.flatten() {
            if entry.path().is_dir() {
                out.push(entry.path().join("bin").join("windows").join("xelatex.exe"));
            }
        }
    }
    // MiKTeX 系统级与用户级。
    out.push(PathBuf::from(
        r"C:\Program Files\MiKTeX\miktex\bin\x64\xelatex.exe",
    ));
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        out.push(
            PathBuf::from(local)
                .join("Programs")
                .join("MiKTeX")
                .join("miktex")
                .join("bin")
                .join("x64")
                .join("xelatex.exe"),
        );
    }
    out
}

/// 检测到 TeX 引擎时编译 `.pdf`；未检测到时返回 `Ok(None)` 并保留 `.tex`。
/// 编译失败时返回错误，调用方仍可把已经生成的 `.tex` 展示给用户。
pub fn compile_pdf_if_available(tex_path: &Path, fonts: &FontConfig) -> Result<Option<PathBuf>> {
    let Some(engine) = find_tex_engine()? else {
        return Ok(None);
    };
    let dir = tex_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = tex_path.file_name().context("TeX 文件路径缺少文件名")?;
    let stem = tex_path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("gongwen");

    match engine {
        TexEngine::PortableTectonic(runtime) => {
            compile_with_portable_tectonic(tex_path, dir, stem, &runtime, fonts)?;
        }
        TexEngine::Xelatex(program) => {
            // 两遍排版以解析目录、页码和交叉引用。
            for _ in 0..2 {
                let output = tex_command(&program)
                    .current_dir(dir)
                    .arg("-interaction=nonstopmode")
                    .arg("-halt-on-error")
                    .arg("-synctex=0")
                    .arg(file_name)
                    .output()
                    .with_context(|| format!("无法启动 xelatex：{}", program.display()))?;
                if !output.status.success() {
                    return Err(compile_error(dir, stem));
                }
            }
        }
        TexEngine::Tectonic(program) => {
            let output = tex_command(&program)
                .current_dir(dir)
                .arg(file_name)
                .output()
                .with_context(|| format!("无法启动 tectonic：{}", program.display()))?;
            if !output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!(
                    "tectonic 编译失败\nstdout:\n{}\nstderr:\n{}",
                    stdout.trim(),
                    stderr.trim()
                );
            }
        }
    }

    let pdf = dir.join(format!("{stem}.pdf"));
    if !pdf.exists() {
        bail!("TeX 编译完成但未生成 PDF 文件：{}", pdf.display());
    }
    cleanup_intermediates(dir, stem);
    Ok(Some(pdf))
}

fn compile_with_portable_tectonic(
    tex_path: &Path,
    dir: &Path,
    stem: &str,
    runtime: &PortableTexRuntime,
    fonts: &FontConfig,
) -> Result<()> {
    let _compile_guard = PORTABLE_TECTONIC_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let workspace =
        PortableCompileWorkspace::create(tex_path, dir, &runtime.fonts, &runtime.bundle, fonts)?;
    let cache_dir = portable_runtime::tectonic_cache_dir()?;
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("无法创建 Tectonic 格式缓存目录：{}", cache_dir.display()))?;

    let output = tex_command(&runtime.tectonic)
        .current_dir(dir)
        .env("TECTONIC_CACHE_DIR", &cache_dir)
        .env("TECTONIC_UNTRUSTED_MODE", "1")
        .arg("-X")
        .arg("compile")
        .arg("--bundle")
        .arg(&workspace.bundle_file_name)
        .arg("--only-cached")
        .arg("--untrusted")
        .arg("--print")
        .arg(&workspace.tex_file_name)
        .output()
        .with_context(|| {
            format!(
                "无法启动内置 Tectonic {}：{}",
                portable_runtime::TECTONIC_VERSION,
                runtime.tectonic.display()
            )
        })?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "内置 Tectonic 离线编译失败\nstdout:\n{}\nstderr:\n{}",
            stdout.trim(),
            stderr.trim()
        );
    }

    if !workspace.pdf_path.is_file() {
        bail!(
            "内置 Tectonic 编译完成但未生成 PDF：{}",
            workspace.pdf_path.display()
        );
    }
    let destination = dir.join(format!("{stem}.pdf"));
    std::fs::copy(&workspace.pdf_path, &destination).with_context(|| {
        format!(
            "无法把 Tectonic 输出复制到目标文件：{}",
            destination.display()
        )
    })?;
    Ok(())
}

/// 在导出目录中创建一次性的 TeX 包装文件与相对字体目录。Tectonic 在 Windows
/// 上不能可靠地让 fontspec 读取带盘符的绝对字体路径，因此用相对路径加载字体；
/// 同时保留原目录作为工作目录，避免改变其他相对资源的解析方式。
struct PortableCompileWorkspace {
    dir: PathBuf,
    stem: String,
    tex_file_name: OsString,
    bundle_file_name: OsString,
    tex_path: PathBuf,
    bundle_path: PathBuf,
    pdf_path: PathBuf,
    font_dir: PathBuf,
}

impl PortableCompileWorkspace {
    fn create(
        source_tex: &Path,
        dir: &Path,
        runtime_fonts: &Path,
        runtime_bundle: &Path,
        fonts: &FontConfig,
    ) -> Result<Self> {
        let id = COMPILE_WORKSPACE_ID.fetch_add(1, Ordering::Relaxed);
        let stem = format!("__gongwen_tectonic_{}_{}", std::process::id(), id);
        let tex_file_name = OsString::from(format!("{stem}.tex"));
        let bundle_file_name = OsString::from(format!("{stem}.ttb"));
        let tex_path = dir.join(&tex_file_name);
        let bundle_path = dir.join(&bundle_file_name);
        let pdf_path = dir.join(format!("{stem}.pdf"));
        let font_dir_name = format!("{stem}_fonts");
        let font_dir = dir.join(&font_dir_name);

        std::fs::create_dir(&font_dir)
            .with_context(|| format!("无法创建 Tectonic 临时字体目录：{}", font_dir.display()))?;

        let result: Result<()> = (|| {
            if std::fs::hard_link(runtime_bundle, &bundle_path).is_err() {
                std::fs::copy(runtime_bundle, &bundle_path).with_context(|| {
                    format!("无法准备离线 TeX bundle：{}", runtime_bundle.display())
                })?;
            }
            for file in portable_runtime::FONT_FILES {
                let source = runtime_fonts.join(file);
                let destination = font_dir.join(file);
                if std::fs::hard_link(&source, &destination).is_err() {
                    std::fs::copy(&source, &destination)
                        .with_context(|| format!("无法准备便携式字体：{}", source.display()))?;
                }
            }
            // 选了本机字体的位置额外拷一份，文件名与 TeX 里的钩子约定一致。
            // 重命名后目录里不存在同名的粗体、斜体文件，fontspec 选到的字面
            // 因此是确定的；未选的位置继续用上面那批内置字体。
            for role in FontRole::ALL {
                let Some(choice) = fonts.active(role) else {
                    continue;
                };
                let source = PathBuf::from(choice.path.trim());
                let destination = font_dir.join(choice.compiled_file_name(role));
                if std::fs::hard_link(&source, &destination).is_err() {
                    std::fs::copy(&source, &destination).with_context(|| {
                        format!(
                            "无法准备{}“{}”的字体文件：{}",
                            role.label(),
                            choice.label(),
                            source.display()
                        )
                    })?;
                }
            }

            let source = std::fs::read_to_string(source_tex)
                .with_context(|| format!("无法读取 TeX 源文件：{}", source_tex.display()))?;
            let wrapped = format!("\\def\\GwaFontPath{{{font_dir_name}/}}\n{source}");
            std::fs::write(&tex_path, wrapped)
                .with_context(|| format!("无法写入 Tectonic 临时 TeX：{}", tex_path.display()))?;
            Ok(())
        })();

        if let Err(error) = result {
            let _ = std::fs::remove_file(&tex_path);
            let _ = std::fs::remove_file(&bundle_path);
            let _ = std::fs::remove_dir_all(&font_dir);
            return Err(error);
        }

        Ok(Self {
            dir: dir.to_owned(),
            stem,
            tex_file_name,
            bundle_file_name,
            tex_path,
            bundle_path,
            pdf_path,
            font_dir,
        })
    }
}

impl Drop for PortableCompileWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.tex_path);
        let _ = std::fs::remove_file(&self.bundle_path);
        for extension in ["aux", "log", "out", "toc", "xdv", "pdf"] {
            let _ = std::fs::remove_file(self.dir.join(format!("{}.{}", self.stem, extension)));
        }
        let _ = std::fs::remove_dir_all(&self.font_dir);
    }
}

/// 读取 `.log` 末尾片段，构造可读的编译失败信息。
fn compile_error(dir: &Path, stem: &str) -> anyhow::Error {
    let log = dir.join(format!("{stem}.log"));
    let tail = std::fs::read_to_string(&log)
        .map(|content| {
            content
                .lines()
                .rev()
                .take(25)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    anyhow::anyhow!("xelatex 编译失败，日志末尾：\n{tail}")
}

/// 清理编译产生的中间文件，保留 `.pdf` 与 `.tex`。
fn cleanup_intermediates(dir: &Path, stem: &str) {
    const INTERMEDIATE_EXTS: &[&str] = &[
        "aux",
        "log",
        "out",
        "toc",
        "fls",
        "fdb_latexmk",
        "synctex.gz",
        "xdv",
    ];
    for extension in INTERMEDIATE_EXTS {
        let _ = std::fs::remove_file(dir.join(format!("{stem}.{extension}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        DraftInput, ExportSelection, JointContact, JointIssuanceMode, TemplateKind,
    };

    #[test]
    fn engine_detection_is_best_effort() {
        // 测试环境可能没有 TeX 引擎；检测不到时不报错。
        let _ = find_tex_engine();
    }

    #[test]
    fn common_install_paths_include_texlive_and_miktex() {
        let paths = common_xelatex_install_paths();
        assert!(
            paths
                .iter()
                .any(|path| path.to_string_lossy().ends_with("xelatex.exe"))
        );
    }

    /// 一张 100x50 红色 PNG，供带图编译测试写入配置目录的 images/ 下。
    /// 不用 1x1 的极小图：xdvipdfmx 在极小图与公文版式组合时曾出现
    /// “driver return code” 失败，标准尺寸图可稳定复现图片链路。
    fn test_png() -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let image = image::RgbaImage::from_pixel(100, 50, image::Rgba([255, 0, 0, 255]));
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .unwrap();
        cursor.into_inner()
    }

    /// 带图导出 → 内置 Tectonic 编译。同时实测 `--untrusted` 模式能否读取
    /// tex 同目录 images/ 子目录下的图片（不能读就改为平铺文件名预案）。
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_letter_with_embedded_image_under_untrusted_mode() {
        // 在配置目录 images/ 下放一张临时图（进程 id 命名，编译后清理）。
        let dir = crate::images::image_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("gw_test_{}.png", std::process::id());
        let img_path = dir.join(&name);
        std::fs::write(&img_path, test_png()).unwrap();
        let outcome = std::panic::catch_unwind(|| {
            let temp = tempfile::tempdir().unwrap();
            let mut input = DraftInput {
                kind: TemplateKind::OfficialLetter,
                ..Default::default()
            };
            input.profile.issuing_unit = "某单位".into();
            input.profile.recipient = "某部门".into();
            input.date = "2026年8月7日".into();
            let markdown = format!("# 关于测试的函\n\n正文。\n\n![示意图](images/{name})");
            let selection = ExportSelection {
                markdown: false,
                docx: false,
                tex: true,
                overwrite: true,
            };
            let files = crate::export::export_all(
                temp.path(),
                &input,
                &markdown,
                &selection,
                &crate::units::UnitDisplay::new(&[]),
                &FontConfig::default(),
            )
            .unwrap();
            let tex = files
                .iter()
                .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
                .unwrap();
            // 图片已随 tex 复制到输出目录。
            let img_out = tex.parent().unwrap().join("images").join(&name);
            assert!(img_out.exists(), "图片应复制到 tex 同目录 images/ 下");
            let pdf = compile_pdf_if_available(tex, &FontConfig::default())
                .unwrap()
                .unwrap();
            let pdf_stem = pdf.file_stem().map(|s| s.to_string_lossy().into_owned());
            let tex_stem = tex.file_stem().map(|s| s.to_string_lossy().into_owned());
            (pdf.exists(), pdf_stem, tex_stem)
        });
        let _ = std::fs::remove_file(&img_path);
        let (pdf_exists, pdf_stem, tex_stem) = outcome.expect("带图编译不应 panic");
        assert!(pdf_exists, "带图文档应编译出 PDF");
        assert_eq!(pdf_stem, tex_stem);
    }

    /// 白头件落款要先装箱量高再决定是否另起一页（见 cls 的 `\WhitePaperClosing`）。
    /// 这条只保证这段逻辑能编译通过；换页判断是否准确要看排版结果，不在此断言。
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_generated_white_paper() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::WhitePaper,
            ..Default::default()
        };
        input.profile.kind = TemplateKind::WhitePaper;
        input.profile.reporting_leaders = "张三、李四".into();
        // 落款单位取长名称：落款会折成两行，正是固定估值判断不了的情形。
        input.profile.signing_unit = "中央网信办新闻舆论处舆情分析研究中心综合协调办公室".into();
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let files = crate::export::export_all(
            temp.path(),
            &input,
            "# 关于报送某某工作情况的报告\n\n现将有关情况报告如下。\n",
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let pdf = compile_pdf_if_available(tex, &FontConfig::default())
            .unwrap()
            .unwrap();
        assert!(pdf.exists());
        assert_eq!(pdf.file_stem(), tex.file_stem());
    }

    /// 白头件附件与函稿走同一条链路：正文后列附件概要、落款后从新页排附件，
    /// 验证 cls 的 whitepaper 分支真的把 \AttachmentContent 排出来。
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_generated_white_paper_with_attachments() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::WhitePaper,
            ..Default::default()
        };
        input.profile.kind = TemplateKind::WhitePaper;
        input.profile.reporting_leaders = "张三".into();
        input.profile.signing_unit = "办公室".into();
        input.date = "2026年8月5日".into();
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let files = crate::export::export_all(
            temp.path(),
            &input,
            "# 关于报送某某工作情况的报告\n\n现将有关情况报告如下。\n<!-- [附件] -->\n# 2026年度情况统计表\n\n| 序号 | 事项 | 完成率 |\n| --- | --- | --- |\n| 1 | 标准化建设 | 100% |\n\n<!-- [附件] -->\n# 整改任务清单\n\n清单内容。",
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let tex_text = std::fs::read_to_string(tex).unwrap();
        assert!(
            tex_text.contains("\\SetAttachmentContent{"),
            "白头件 tex 应带附件内容：{tex_text}"
        );
        assert!(
            tex_text.contains("附件1：2026年度情况统计表"),
            "白头件正文后应列附件概要：{tex_text}"
        );
        let pdf = compile_pdf_if_available(tex, &FontConfig::default())
            .unwrap()
            .unwrap();
        assert!(pdf.exists());
        assert_eq!(pdf.file_stem(), tex.file_stem());
        // 便于排查：把生成的 tex 与 pdf 各保留一份到固定路径。
        let _ = std::fs::create_dir_all(".tmp/verify");
        let _ = std::fs::copy(tex, ".tmp/verify/white-paper-attachment.tex");
        let _ = std::fs::copy(&pdf, ".tmp/verify/white-paper-attachment.pdf");
    }
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_generated_white_paper_with_multi_unit_abbr_spread() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::WhitePaper,
            ..Default::default()
        };
        input.profile.kind = TemplateKind::WhitePaper;
        input.profile.reporting_leaders = "张三".into();
        input.profile.signing_unit = "省教育厅、教师处".into();
        input.profile.use_short_name_for_signature = true;
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let files = crate::export::export_all(
            temp.path(),
            &input,
            "# 关于报送某某工作情况的报告\n\n现将有关情况报告如下。\n",
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let pdf = compile_pdf_if_available(tex, &FontConfig::default())
            .unwrap()
            .unwrap();
        assert!(pdf.exists());
    }

    /// 红头呈批件以绝对坐标覆盖首页框架，正文按动态剩余基线连续流排；同时
    /// 覆盖一级标题、跨页长段落、多段正文、动态承办区、第二页落款和附件。
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_generated_red_head_approval() {
        let mut input = DraftInput {
            kind: TemplateKind::RedHeadApproval,
            ..Default::default()
        };
        input.profile.kind = TemplateKind::RedHeadApproval;
        input.profile.issuing_unit = "某某委员会办公室".into();
        input.profile.department_code = "某办呈".into();
        input.profile.document_year = "2026".into();
        input.profile.document_number = "12".into();
        input.profile.reporting_leaders = "张三、李四".into();
        input.profile.signing_unit = "某某委员会".into();
        input.profile.joint_responsible_units = "综合处".into();
        input.profile.joint_contacts = vec![crate::models::JointContact {
            unit: "综合处".into(),
            name: "王五".into(),
            phone: "010-12345678".into(),
        }];
        input.date = "2026年8月12日".into();
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let paragraph = "为进一步推进服务事项标准化、规范化、便利化（以下简称“标准化建设”），全面掌握各单位年度工作进展，请结合实际报送2026年度标准化建设情况。报送数据应与服务事项管理系统保持一致，其中涉及系统接口（API）调用、电子凭证共享和跨部门联办的内容，应当一并核实。现将有关事项函告如下。";
        for lead in [paragraph.to_string(), paragraph.repeat(2)] {
            let temp = tempfile::tempdir().unwrap();
            let markdown = format!(
                "# 关于报送2026年度服务事项标准化建设情况的函\n\n{lead}\n\n## 报送内容\n\n各单位应当围绕事项清单维护、服务指南规范、线上线下融合和服务效能提升等方面，全面梳理年度工作完成情况，客观反映工作成效、存在问题及下一步安排。"
            );
            let files = crate::export::export_all(
                temp.path(),
                &input,
                &markdown,
                &selection,
                &crate::units::UnitDisplay::new(&[]),
                &FontConfig::default(),
            )
            .unwrap();
            let tex = files
                .iter()
                .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
                .unwrap();
            let pdf = compile_pdf_if_available(tex, &FontConfig::default())
                .unwrap()
                .unwrap();
            assert!(pdf.exists());
        }
    }

    /// 代章：cls 的 \SignatureSealOnBehalf 条件分支与生成器注入的命令一起编译，
    /// 确认“（代章）”在落款单位下方排成一行。
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_generated_letter_with_seal_on_behalf() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        input.date = "2026年8月7日".into();
        let vocabulary = vec![crate::models::VocabularyEntry {
            category: crate::models::VocabularyCategory::Unit,
            canonical: "某单位".into(),
            seal_on_behalf: true,
            ..Default::default()
        }];
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let files = crate::export::export_all(
            temp.path(),
            &input,
            "# 关于代章测试的函\n\n正文。",
            &selection,
            &crate::units::UnitDisplay::new(&vocabulary),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let content = std::fs::read_to_string(tex).unwrap();
        assert!(
            content.contains("\\renewcommand{\\SignatureSealOnBehalf}{（代章）}"),
            "应注入代章命令：{content}"
        );
        let pdf = compile_pdf_if_available(tex, &FontConfig::default())
            .unwrap()
            .unwrap();
        assert!(pdf.exists());
        // 便于排查：把生成的 tex 与 pdf 各保留一份到固定路径。
        let _ = std::fs::copy(tex, ".tmp/verify/seal-letter.tex");
        let _ = std::fs::copy(&pdf, ".tmp/verify/seal-letter.pdf");
    }

    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_generated_letter_and_cleans_intermediates() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput::default();
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
        input.profile.main_issuing_unit = "甲单位".into();
        input.profile.joint_responsible_units = "甲处室、乙处室".into();
        input.profile.joint_contacts = vec![
            JointContact {
                unit: "甲处室".into(),
                name: "张三".into(),
                phone: "010-11111111".into(),
            },
            JointContact {
                unit: "乙处室".into(),
                name: "欧阳翠花".into(),
                phone: "010-22222222".into(),
            },
        ];
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let files = crate::export::export_all(
            temp.path(),
            &input,
            "# 测试函\n\n正文。",
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        // 便于排查：把生成的 tex 保留一份到固定路径。
        let _ = std::fs::copy(tex, ".tmp/verify/generated-joint.tex");
        let pdf = compile_pdf_if_available(tex, &FontConfig::default())
            .unwrap()
            .unwrap();
        assert!(pdf.exists());
        assert_eq!(pdf.file_stem(), tex.file_stem());
        // 中间文件被清理，.tex 源文件保留。
        assert!(!tex.with_extension("aux").exists());
        assert!(!tex.with_extension("log").exists());
        assert!(tex.exists());
    }

    /// 联合发文模式 1 只剩 1 个发文单位：落款走类默认的右侧单列（jointmodeone 关），
    /// 版记仍逐行列出承办单位/联系人/电话（jointrecord 开）。这条编译一遍，确保
    /// cls 里新拆出来的 jointrecord 选项与 \SetJointFooterRecord 能单独生效。
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_single_unit_joint_letter_with_multi_row_record() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput::default();
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "甲单位".into();
        input.profile.main_issuing_unit = "甲单位".into();
        input.profile.joint_responsible_units = "甲处室、乙处室".into();
        input.profile.joint_contacts = vec![
            JointContact {
                unit: "甲处室".into(),
                name: "张三".into(),
                phone: "010-11111111".into(),
            },
            JointContact {
                unit: "乙处室".into(),
                name: "欧阳翠花".into(),
                phone: "010-22222222".into(),
            },
        ];
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let files = crate::export::export_all(
            temp.path(),
            &input,
            "# 测试函\n\n正文。",
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let _ = std::fs::copy(tex, ".tmp/verify/generated-joint-single.tex");
        let pdf = compile_pdf_if_available(tex, &FontConfig::default())
            .unwrap()
            .unwrap();
        assert!(pdf.exists());
    }

    /// 指人专办、正文括号楷体四号、附件概要三条新特性走同一套 TeX 链路，
    /// 单独编译一遍确保 cls 的 \SecurityLine/\SpecialHandling 与正文 \kai\zihao{4} 可编译。
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_generated_letter_with_special_handling_parentheses_and_attachments() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "10年".into();
        input.profile.special_handling = true;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let files = crate::export::export_all(
            temp.path(),
            &input,
            "# 关于开展测试工作的函\n<!-- [正文] -->\n现就（有关事项）函告如下。\n<!-- [附件] -->\n# 附件1\n## 统计表\n内容。\n# 附件2\n## 说明材料\n内容。",
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let pdf = compile_pdf_if_available(tex, &FontConfig::default())
            .unwrap()
            .unwrap();
        assert!(pdf.exists());
        assert_eq!(pdf.file_stem(), tex.file_stem());
        // 便于排查：把生成的 tex 与 pdf 各保留一份到固定路径。
        let _ = std::fs::copy(tex, ".tmp/verify/special-letter.tex");
        let _ = std::fs::copy(&pdf, ".tmp/verify/special-letter.pdf");
    }

    /// 宽表触发单附件横页，并在下一个附件前恢复竖页；验证 pdflscape 与 longtblr 可共同编译。
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_generated_letter_with_smart_landscape_attachment() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let markdown = "# 关于开展测试工作的函\n正文。\n<!-- [附件] -->\n# 附件1\n## 整改情况表\n| 序号 | 事项类别 | 事项名称 | 存在问题 | 整改措施 | 责任部门 | 完成时限 | 当前状态 |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n| 1 | 线上办理 | 行政备案事项 | 移动端页面显示不完整，申请人无法正常上传附件。 | 优化页面适配，增加格式和大小提示并开展测试。 | 技术保障部门 | 2026年8月12日 | 已完成 |\n# 附件2\n## 说明材料\n内容。";
        let files = crate::export::export_all(
            temp.path(),
            &input,
            markdown,
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let content = std::fs::read_to_string(tex).unwrap();
        assert_eq!(content.matches("\\begin{landscape}").count(), 1);
        assert_eq!(content.matches("\\end{landscape}").count(), 1);
        let pdf = compile_pdf_if_available(tex, &FontConfig::default())
            .unwrap()
            .unwrap();
        assert!(pdf.exists());
    }

    /// 最后一个附件为横页时，结束横页后新开的竖页仍须把版记固定在版心底部。
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_footer_page_after_last_landscape_attachment() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        input.profile.responsible_unit = "综合处".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-12345678".into();
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let markdown = "# 关于开展测试工作的函\n正文。\n<!-- [附件] -->\n# 附件1\n## 整改情况表\n| 序号 | 事项类别 | 事项名称 | 存在问题 | 整改措施 | 责任部门 | 完成时限 | 当前状态 |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n| 1 | 线上办理 | 行政备案事项 | 移动端页面显示不完整，申请人无法正常上传附件。 | 优化页面适配，增加格式和大小提示并开展测试。 | 技术保障部门 | 2026年8月12日 | 已完成 |";
        let files = crate::export::export_all(
            temp.path(),
            &input,
            markdown,
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let content = std::fs::read_to_string(tex).unwrap();
        assert!(content.trim_end().contains("\\end{landscape}"));
        let pdf = compile_pdf_if_available(tex, &FontConfig::default())
            .unwrap()
            .unwrap();
        assert!(pdf.exists());
    }

    /// 长标题触发 jieba 换行：`\TitleContent` 里含 `\\` 分段，单独编译确认可排版。
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_generated_letter_with_wrapped_title() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let files = crate::export::export_all(
            temp.path(),
            &input,
            "# 关于转发国家互联网信息办公室有关网络安全和信息化工作重点任务实施方案的通知\n\n现函告如下。",
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let content = std::fs::read_to_string(tex).unwrap();
        assert!(content.contains("\\renewcommand{\\TitleContent}{"));
        assert!(
            content.contains("\\\\"),
            "换行标题应含 \\\\ 分段：{content}"
        );
        let pdf = compile_pdf_if_available(tex, &FontConfig::default())
            .unwrap()
            .unwrap();
        assert!(pdf.exists());
        assert_eq!(pdf.file_stem(), tex.file_stem());
    }

    /// 22 字标题触发横向压缩：`\TitleContent` 里含 `\scalebox`，单独编译确认可排版且字高不变。
    #[test]
    #[ignore = "需要本机安装 xelatex 才能运行"]
    fn compiles_generated_letter_with_compressed_title() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let files = crate::export::export_all(
            temp.path(),
            &input,
            "# 关于认真做好网络安全与信息化重点工作验收的函\n\n现函告如下。",
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let content = std::fs::read_to_string(tex).unwrap();
        assert!(
            content.contains("\\scalebox{0.91}[1]"),
            "压缩标题应含 \\scalebox：{content}"
        );
        let pdf = compile_pdf_if_available(tex, &FontConfig::default())
            .unwrap()
            .unwrap();
        assert!(pdf.exists());
        assert_eq!(pdf.file_stem(), tex.file_stem());
    }

    /// 五个位置全换成本机字体后仍能编译出 PDF。这条最关键：注入的钩子、临时目录里
    /// 的重命名字体文件和 Tectonic 的受限文件访问要同时成立，任何一环错了都编译不出来。
    #[test]
    #[ignore = "需要本机安装 xelatex 与常见中文字体才能运行"]
    fn compiles_letter_with_system_fonts_for_every_role() {
        // 都是 Windows 自带的单字面 ttf；缺任何一个就跳过，不把测试变成环境断言。
        let candidates = [
            (FontRole::Title, r"C:\Windows\Fonts\simhei.ttf"),
            (FontRole::Heading1, r"C:\Windows\Fonts\simhei.ttf"),
            (FontRole::Heading2, r"C:\Windows\Fonts\simkai.ttf"),
            (FontRole::Body, r"C:\Windows\Fonts\simfang.ttf"),
            (FontRole::PageNumber, r"C:\Windows\Fonts\simkai.ttf"),
        ];
        let mut fonts = FontConfig {
            use_system_fonts: true,
            ..FontConfig::default()
        };
        for (role, path) in candidates {
            let Some(font) = crate::system_fonts::read_font(Path::new(path)) else {
                return;
            };
            *fonts.choice_mut(role) = font.to_choice();
        }

        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let files = crate::export::export_all(
            temp.path(),
            &input,
            "# 关于开展本机字体编译测试的函\n\n## 一、任务目标\n\n验证换字体后仍能排版。\n\n### （一）具体要求\n\n照常执行。",
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &fonts,
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let content = std::fs::read_to_string(tex).unwrap();
        assert!(content.contains("\\def\\GwaFontSetupHook"));
        let pdf = compile_pdf_if_available(tex, &fonts).unwrap().unwrap();
        assert!(pdf.exists());
        // 字体目录是临时的，编译完必须清干净，不留在用户的导出目录里。
        let leftovers: Vec<_> = std::fs::read_dir(pdf.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains("_fonts"))
            .collect();
        assert!(leftovers.is_empty(), "临时字体目录应已清理");
    }

    /// 导出的 `.tex` 拿到别的机器上编译时走按字体名加载那条分支：这里直接调
    /// xelatex 编译，不经过内置 Tectonic，确认写进去的字体名真的能被解析。
    #[test]
    #[ignore = "需要本机安装 xelatex 与常见中文字体才能运行"]
    fn compiles_exported_tex_by_font_name_with_xelatex() {
        let Some(xelatex) = find_xelatex() else {
            return;
        };
        let Some(body) = crate::system_fonts::read_font(Path::new(r"C:\Windows\Fonts\simfang.ttf"))
        else {
            return;
        };
        let Some(title) = crate::system_fonts::read_font(Path::new(r"C:\Windows\Fonts\simhei.ttf"))
        else {
            return;
        };
        let mut fonts = FontConfig {
            use_system_fonts: true,
            ..FontConfig::default()
        };
        *fonts.choice_mut(FontRole::Body) = body.to_choice();
        // 标题必须指定：不指定时按名字加载会去找方正小标宋，本机不一定装了。
        *fonts.choice_mut(FontRole::Title) = title.to_choice();
        *fonts.choice_mut(FontRole::Heading1) = title.to_choice();

        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        let selection = ExportSelection {
            markdown: false,
            docx: false,
            tex: true,
            overwrite: true,
        };
        let files = crate::export::export_all(
            temp.path(),
            &input,
            "# 关于按字体名编译的函\n\n## 一、任务目标\n\n验证按名字加载。",
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &fonts,
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let dir = tex.parent().unwrap();
        let output = tex_command(&xelatex)
            .current_dir(dir)
            .arg("-interaction=nonstopmode")
            .arg("-halt-on-error")
            .arg(tex.file_name().unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "按字体名加载应能直接用 xelatex 编译：\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            dir.join(format!(
                "{}.pdf",
                tex.file_stem().unwrap().to_string_lossy()
            ))
            .is_file()
        );
    }
}
