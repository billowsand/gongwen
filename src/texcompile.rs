//! TeX 编译：优先使用应用自带的 Tectonic、离线 bundle 与字体；开发环境再兼容
//! 本机 XeLaTeX/Tectonic。输出 PDF 后清理中间文件。
//! 编译在后台线程执行，主界面通过轮询拿到结果，不会假死。

use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

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
pub fn compile_pdf_if_available(tex_path: &Path) -> Result<Option<PathBuf>> {
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
            compile_with_portable_tectonic(tex_path, dir, stem, &runtime)?;
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
) -> Result<()> {
    let _compile_guard = PORTABLE_TECTONIC_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let workspace =
        PortableCompileWorkspace::create(tex_path, dir, &runtime.fonts, &runtime.bundle)?;
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
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let pdf = compile_pdf_if_available(tex).unwrap().unwrap();
        assert!(pdf.exists());
        assert_eq!(pdf.file_stem(), tex.file_stem());
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
        let pdf = compile_pdf_if_available(tex).unwrap().unwrap();
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
                name: "张三".into(),
                phone: "010-11111111".into(),
            },
            JointContact {
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
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        // 便于排查：把生成的 tex 保留一份到固定路径。
        let _ = std::fs::copy(tex, ".tmp/verify/generated-joint.tex");
        let pdf = compile_pdf_if_available(tex).unwrap().unwrap();
        assert!(pdf.exists());
        assert_eq!(pdf.file_stem(), tex.file_stem());
        // 中间文件被清理，.tex 源文件保留。
        assert!(!tex.with_extension("aux").exists());
        assert!(!tex.with_extension("log").exists());
        assert!(tex.exists());
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
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let pdf = compile_pdf_if_available(tex).unwrap().unwrap();
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
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let content = std::fs::read_to_string(tex).unwrap();
        assert_eq!(content.matches("\\begin{landscape}").count(), 1);
        assert_eq!(content.matches("\\end{landscape}").count(), 1);
        let pdf = compile_pdf_if_available(tex).unwrap().unwrap();
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
        )
        .unwrap();
        let tex = files
            .iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
            .unwrap();
        let content = std::fs::read_to_string(tex).unwrap();
        assert!(content.trim_end().contains("\\end{landscape}"));
        let pdf = compile_pdf_if_available(tex).unwrap().unwrap();
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
        let pdf = compile_pdf_if_available(tex).unwrap().unwrap();
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
        let pdf = compile_pdf_if_available(tex).unwrap().unwrap();
        assert!(pdf.exists());
        assert_eq!(pdf.file_stem(), tex.file_stem());
    }
}
