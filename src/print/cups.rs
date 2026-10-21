//! CUPS 后端（macOS、Linux、麒麟）：`lpstat` 列打印机，`lpoptions` 查状态，`lp` 出纸。
//!
//! 这些命令的提示文字会翻译成系统语言，macOS 上设 `LC_ALL=C` 也照样输出中文
//! （CUPS 在苹果平台遇到 C 语言环境会改用系统首选语言）。所以只解析不随语言变的
//! 部分：`lpstat -e` 每行一个名字；`lpstat -d` 只取冒号后、且确实在打印机清单里的
//! 名字；状态取 `lpoptions -p` 输出的 `printer-state` / `printer-state-reasons` 键值。
//!
//! 用户数据（打印机名、任务名、文件路径）都作为独立参数交给 `Command`，不经 shell；
//! 页码先解析成区间再格式化，不原样透传；文件名前有 `--`。

use super::{
    ColorMode, Duplex, MediaOption, PrintJob, PrintOutcome, PrinterCaps, PrinterInfo, PrinterState,
    Scaling, pages,
};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

pub(super) fn lp_available() -> bool {
    program("lp").is_some()
}

pub(super) fn list_printers() -> Result<Vec<PrinterInfo>, String> {
    let names = parse_names(&run("lpstat", &["-e"])?);
    // 没有默认打印机时 `lpstat -d` 可能以非零码退出，这不算错。
    let default = run("lpstat", &["-d"])
        .ok()
        .and_then(|output| parse_default(&output, &names));
    Ok(names
        .into_iter()
        .map(|name| PrinterInfo {
            is_default: default.as_deref() == Some(name.as_str()),
            name,
            state: PrinterState::Unknown,
        })
        .collect())
}

pub(super) fn printer_state(name: &str) -> PrinterState {
    match run("lpoptions", &["-p", name]) {
        Ok(output) => parse_state(&output),
        Err(_) => PrinterState::Unknown,
    }
}

/// 查一台打印机支持的选项。`lpoptions -l` 每行形如
/// `Key/Label: 值1 *值2 值3`，带 `*` 的是当前默认值。
// 第二步的对话框接上后就有调用方，届时去掉这行。
#[allow(dead_code)]
pub(super) fn printer_caps(name: &str) -> PrinterCaps {
    run("lpoptions", &["-p", name, "-l"])
        .map(|output| parse_caps(&output))
        .unwrap_or_default()
}

/// 交给 `lp`，成败看退出码。`lp` 自己的报错（没有默认打印机之类）比我们编的话
/// 准确，所以原样转出去；这里不改语言环境，报错跟随系统语言。
pub(super) fn print(job: &PrintJob) -> Result<PrintOutcome, String> {
    let program = program("lp")
        .ok_or_else(|| "未找到 lp 命令，本机可能没有安装 CUPS 打印服务".to_string())?;
    let output = Command::new(program)
        .args(lp_args(job))
        .output()
        .map_err(|error| format!("无法启动打印：{error}"))?;
    if output.status.success() {
        return Ok(PrintOutcome::Printed);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    Err(if detail.is_empty() {
        "打印命令执行失败，请检查打印机是否可用".to_string()
    } else {
        detail.to_string()
    })
}

/// `lp` 的参数。只在选项与默认不同时才加，命令行越短越不容易撞上老版本 CUPS 的毛病。
pub(super) fn lp_args(job: &PrintJob) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    if let Some(printer) = &job.printer {
        args.push("-d".into());
        args.push(printer.into());
    }
    if job.copies > 1 {
        args.push("-n".into());
        args.push(job.copies.to_string().into());
        args.push("-o".into());
        args.push(
            if job.collate {
                "collate=true"
            } else {
                "collate=false"
            }
            .into(),
        );
    }
    if let Some(spans) = &job.pages {
        args.push("-P".into());
        args.push(pages::format_spans(spans).into());
    }
    // IPP 标准选项优先，CUPS 自己映射到 PPD；`-o` 都放在 `-t` 与 `--` 之前。
    if let Some(duplex) = job.duplex {
        push_option(&mut args, duplex_option(duplex).to_string());
    }
    if let Some(color) = job.color {
        let mode = match color {
            ColorMode::Color => "print-color-mode=color",
            ColorMode::Mono => "print-color-mode=monochrome",
        };
        push_option(&mut args, mode.to_string());
    }
    if let Some(paper) = &job.paper {
        push_option(&mut args, format!("PageSize={paper}"));
    }
    if let Some(tray) = &job.tray {
        push_option(&mut args, format!("InputSlot={tray}"));
    }
    // 实际大小是 CUPS 的 auto（放不下才缩），只有「适合纸张」需要明说。
    if job.scaling == Scaling::Fit {
        push_option(&mut args, "print-scaling=fit".to_string());
    }
    // 手动双面第二轮：出纸面朝上的机器要倒序，翻面后正反才对得上。
    if job.reverse {
        push_option(&mut args, "outputorder=reverse".to_string());
    }
    if !job.title.trim().is_empty() {
        args.push("-t".into());
        args.push(job.title.as_str().into());
    }
    // `--` 之后即使文件名以短横线开头也不会被当成选项。
    args.push("--".into());
    args.push(job.path.as_os_str().to_owned());
    args
}

fn push_option(args: &mut Vec<OsString>, value: String) {
    args.push("-o".into());
    args.push(value.into());
}

/// 双面方式 → CUPS 的 `sides` 取值。
fn duplex_option(duplex: Duplex) -> &'static str {
    match duplex {
        // 手动双面的每一轮都只打一面，双面由人翻纸完成。
        Duplex::Simplex | Duplex::Manual => "sides=one-sided",
        Duplex::LongEdge => "sides=two-sided-long-edge",
        Duplex::ShortEdge => "sides=two-sided-short-edge",
    }
}

fn run(name: &str, args: &[&str]) -> Result<String, String> {
    let program =
        program(name).ok_or_else(|| format!("未找到 {name} 命令，本机可能没有安装 CUPS"))?;
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("无法运行 {name}：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{name} 执行失败：{}", stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn program(name: &str) -> Option<PathBuf> {
    program_in_path(&std::env::var_os("PATH")?, name)
}

/// 在给定的 PATH 串里找命令。与环境变量解耦，便于单测。
fn program_in_path(path: &std::ffi::OsStr, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// `lpstat -e`：每行一个打印机名（CUPS 的队列名不含空白）。
fn parse_names(output: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !names.iter().any(|name| name == line) {
            names.push(line.to_string());
        }
    }
    names
}

/// `lpstat -d`：「system default destination: X」或其译文「系统默认目的位置：X」。
/// 只取最后一个冒号之后的部分，并要求它确实是一台已知打印机——没有默认打印机时
/// 那句话里没有名字，各种译文也就都不会误认。
fn parse_default(output: &str, names: &[String]) -> Option<String> {
    output.lines().find_map(|line| {
        let (_, tail) = line.rsplit_once([':', '：'])?;
        let candidate = tail.trim();
        names
            .iter()
            .find(|name| name.as_str() == candidate)
            .cloned()
    })
}

/// `lpoptions -p 名字` 输出一行以空白分隔的 `键=值`，值里有空白时用引号括起。
fn parse_options(output: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut token = String::new();
    let mut quote: Option<char> = None;
    let mut chars = output.chars();
    let mut flush = |token: &mut String| {
        if let Some((key, value)) = token.split_once('=') {
            pairs.push((key.to_string(), value.to_string()));
        }
        token.clear();
    };
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    token.push(next);
                }
            }
            (Some(open), ch) if ch == open => quote = None,
            (Some(_), ch) => token.push(ch),
            (None, '\'' | '"') => quote = Some(ch),
            (None, ch) if ch.is_whitespace() => flush(&mut token),
            (None, ch) => token.push(ch),
        }
    }
    flush(&mut token);
    pairs
}

/// 由 IPP 的 `printer-state`（3 空闲、4 处理中、5 停止）与 `printer-state-reasons`
/// 推出状态。原因带 `-report` / `-warning` 后缀的只是提示（如墨粉偏低），不当回事；
/// 唯独 offline 不论后缀都算脱机——CUPS 连不上打印机时报的就是 `offline-report`。
fn parse_state(output: &str) -> PrinterState {
    let options = parse_options(output);
    let value = |key: &str| {
        options
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    };
    let reasons: Vec<&str> = value("printer-state-reasons")
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|reason| !reason.is_empty() && *reason != "none")
        .collect();
    if reasons.iter().any(|reason| reason.starts_with("offline")) {
        return PrinterState::Offline;
    }
    let serious: Vec<&str> = reasons
        .iter()
        .filter(|reason| !reason.ends_with("-report") && !reason.ends_with("-warning"))
        .map(|reason| reason.trim_end_matches("-error"))
        .collect();
    let attention = [
        ("media-empty", "缺纸"),
        ("media-needed", "缺纸"),
        ("media-jam", "卡纸"),
        ("door-open", "机盖未关"),
        ("cover-open", "机盖未关"),
        ("toner-empty", "缺墨粉"),
        ("marker-supply-empty", "缺墨"),
        ("output-area-full", "出纸槽已满"),
    ];
    if let Some((_, label)) = attention
        .iter()
        .find(|(reason, _)| serious.contains(reason))
    {
        return PrinterState::Attention((*label).to_string());
    }
    match value("printer-state") {
        Some("5") => PrinterState::Paused,
        _ if serious.contains(&"paused") => PrinterState::Paused,
        Some("4") => PrinterState::Printing,
        Some("3") => PrinterState::Ready,
        _ => PrinterState::Unknown,
    }
}

/// `lpoptions -l` 的输出 → 能力。键名与取值随 PPD 各厂商不一，只认 IPP 标准的
/// `Duplex` / `ColorModel` / `PageSize` / `InputSlot`，认不出的不展示。
#[cfg_attr(not(test), allow(dead_code))]
fn parse_caps(output: &str) -> PrinterCaps {
    let mut caps = PrinterCaps::default();
    // 没有 ColorModel 键时不知道能不能选彩色：当作能选，CUPS 不支持会忽略。
    let mut has_color_model = false;
    let mut gray = false;
    let mut color = false;
    for line in output.lines() {
        let Some((head, tail)) = line.split_once(':') else {
            continue;
        };
        // `Key/Label` 只取斜杠前的键名；没有标签时整段就是键名。
        let key = head.split_once('/').map_or(head, |(key, _)| key).trim();
        let values: Vec<&str> = tail.split_whitespace().collect();
        let default = values.iter().find_map(|value| value.strip_prefix('*'));
        match key {
            "Duplex" => caps.duplex = values.iter().any(|value| keyword(value) != "None"),
            "ColorModel" => {
                has_color_model = true;
                for value in &values {
                    let value = keyword(value);
                    gray |= value.contains("Gray");
                    color |=
                        value.contains("RGB") || value.contains("CMYK") || value.contains("Color");
                }
            }
            "PageSize" => {
                caps.papers = media_options(&values);
                caps.default_paper = default
                    .filter(|id| !id.starts_with("Custom."))
                    .map(str::to_string);
            }
            "InputSlot" => {
                caps.trays = media_options(&values);
                caps.default_tray = default.map(str::to_string);
            }
            _ => {}
        }
    }
    // 只有灰度与彩色两类都在，才谈得上「能选彩色」；纯灰度机不置灰就让人白选。
    caps.color = !has_color_model || (gray && color);
    caps
}

/// 选项值去掉表示默认值的 `*`。
#[cfg_attr(not(test), allow(dead_code))]
fn keyword(value: &str) -> &str {
    value.strip_prefix('*').unwrap_or(value)
}

/// 纸张 / 纸盒的取值列表成选项。`Custom.WIDTHxHEIGHT` 是让用户现填尺寸的占位，
/// 不是一种纸，去掉。
#[cfg_attr(not(test), allow(dead_code))]
fn media_options(values: &[&str]) -> Vec<MediaOption> {
    values
        .iter()
        .map(|value| keyword(value))
        .filter(|id| !id.starts_with("Custom."))
        .map(|id| MediaOption {
            id: id.to_string(),
            label: id.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_is_found_only_when_it_exists_on_the_path() {
        use std::os::unix::fs::PermissionsExt;

        let empty = tempfile::tempdir().expect("临时目录");
        let with_lp = tempfile::tempdir().expect("临时目录");
        let lp = with_lp.path().join("lp");
        std::fs::write(&lp, "#!/bin/sh\n").expect("写入假 lp");
        std::fs::set_permissions(&lp, std::fs::Permissions::from_mode(0o755)).expect("置可执行");

        // 只有空目录：找不到。
        let path = std::env::join_paths([empty.path()]).expect("拼 PATH");
        assert!(program_in_path(&path, "lp").is_none());

        // 后一个目录里有 lp：找得到，且返回的就是它。
        let path = std::env::join_paths([empty.path(), with_lp.path()]).expect("拼 PATH");
        assert_eq!(program_in_path(&path, "lp"), Some(lp));
    }

    #[test]
    fn names_are_one_per_line_without_duplicates() {
        let names = parse_names("HP_DeskJet\n\nOffice-3F\nHP_DeskJet\n");
        assert_eq!(names, vec!["HP_DeskJet", "Office-3F"]);
    }

    #[test]
    fn default_is_read_in_any_language_but_only_if_it_is_a_known_printer() {
        let names = vec!["HP_DeskJet".to_string(), "Office".to_string()];
        assert_eq!(
            parse_default("system default destination: Office\n", &names).as_deref(),
            Some("Office")
        );
        assert_eq!(
            parse_default("系统默认目的位置：HP_DeskJet\n", &names).as_deref(),
            Some("HP_DeskJet")
        );
        assert_eq!(
            parse_default("no system default destination\n", &names),
            None
        );
        assert_eq!(parse_default("无系统默认目的位置\n", &names), None);
    }

    /// 本机 HP DeskJet 的真实输出（节选）：连不上时报 offline-report。
    const DESKJET: &str = "copies=1 device-uri=ippusb://HP%20DeskJet/?uuid=1 \
        marker-names='tri-color\\ ink,black\\ ink' printer-info='HP DeskJet 4900 series' \
        printer-is-accepting-jobs=true printer-location='Ours的Mac mini' printer-state=3 \
        printer-state-reasons=offline-report printer-type=69243084";

    #[test]
    fn options_parser_respects_quotes_and_escapes() {
        let options = parse_options(DESKJET);
        let info = options
            .iter()
            .find(|(key, _)| key == "printer-info")
            .map(|(_, value)| value.as_str());
        assert_eq!(info, Some("HP DeskJet 4900 series"));
        let markers = options
            .iter()
            .find(|(key, _)| key == "marker-names")
            .map(|(_, value)| value.as_str());
        assert_eq!(markers, Some("tri-color ink,black ink"));
    }

    #[test]
    fn state_comes_from_ipp_state_and_reasons() {
        assert_eq!(parse_state(DESKJET), PrinterState::Offline);
        assert_eq!(
            parse_state("printer-state=3 printer-state-reasons=none"),
            PrinterState::Ready
        );
        assert_eq!(
            parse_state("printer-state=4 printer-state-reasons=toner-low-warning"),
            PrinterState::Printing
        );
        assert_eq!(
            parse_state("printer-state=5 printer-state-reasons=paused"),
            PrinterState::Paused
        );
        assert_eq!(
            parse_state("printer-state=5 printer-state-reasons=media-empty-error"),
            PrinterState::Attention("缺纸".to_string())
        );
        assert_eq!(parse_state(""), PrinterState::Unknown);
    }

    /// 本机 HP DeskJet 4900 的真实 `lpoptions -p … -l` 输出：没有 Duplex 键，
    /// 所以这台机器没有双面单元。
    const DESKJET_LPOPTIONS: &str = "Collate/Collate: True *False
ColorModel/Color Mode: Gray Gray16 DeviceGray DeviceGray16 *RGB AdobeRGB DeviceRGB DeviceRGB16
cupsPrintQuality/Quality: Draft *Normal High
PageSize/Media Size: 100x150mm 3.5x5 4x6 5x7 5x8 8x10 *A4 A5 A6 B5 Env10 EnvC6 EnvChou3 EnvChou4 EnvDL Executive ISOB5 Legal Letter Postcard Custom.WIDTHxHEIGHT
MediaType/MediaType: stationery photographic-glossy com.hp-specialty-glossy com.hp-matte-brochure stationery-lightweight *any";

    /// 编的样本：有双面单元与两个纸盒、没有 ColorModel 键。
    const WITH_DUPLEX: &str = "Duplex/2-Sided Printing: *None DuplexNoTumble DuplexTumble
InputSlot/Media Source: *Auto Tray1 Tray2
PageSize/Media Size: *A4 Letter";

    #[test]
    fn desktjet_reports_color_and_a4_but_no_duplex() {
        let caps = parse_caps(DESKJET_LPOPTIONS);
        assert!(!caps.duplex);
        assert!(caps.color);
        assert_eq!(caps.default_paper.as_deref(), Some("A4"));
        assert!(
            caps.papers
                .iter()
                .any(|paper| paper.id == "A4" && paper.label == "A4")
        );
        assert!(
            !caps
                .papers
                .iter()
                .any(|paper| paper.id.starts_with("Custom."))
        );
        assert_eq!(caps.papers.len(), 20);
        assert!(caps.trays.is_empty() && caps.default_tray.is_none());
    }

    #[test]
    fn duplex_and_trays_come_from_the_options_that_have_them() {
        let caps = parse_caps(WITH_DUPLEX);
        assert!(caps.duplex);
        assert_eq!(caps.default_tray.as_deref(), Some("Auto"));
        assert_eq!(
            caps.trays
                .iter()
                .map(|tray| tray.id.as_str())
                .collect::<Vec<_>>(),
            ["Auto", "Tray1", "Tray2"]
        );
        // 没有 ColorModel 键：不知道就让人选，CUPS 不支持会忽略。
        assert!(caps.color);
    }

    #[test]
    fn all_none_duplex_and_grayscale_printer_are_reported_as_such() {
        assert!(!parse_caps("Duplex/2-Sided Printing: *None").duplex);
        assert!(!parse_caps("ColorModel/Color Mode: *Gray Gray16").color);
        // 什么都没列出来时判断不了，按「不知道就让人选」处理。
        assert!(parse_caps("").color);
    }

    fn job() -> PrintJob {
        PrintJob {
            path: PathBuf::from("/tmp/-请示.pdf"),
            title: "关于××的请示".to_string(),
            printer: None,
            copies: 1,
            collate: true,
            pages: None,
            duplex: None,
            color: None,
            paper: None,
            tray: None,
            scaling: Scaling::Actual,
            devmode: None,
            reverse: false,
        }
    }

    #[test]
    fn plain_job_only_names_the_title_and_file() {
        let args = lp_args(&job());
        assert_eq!(
            args,
            ["-t", "关于××的请示", "--", "/tmp/-请示.pdf"]
                .map(OsString::from)
                .to_vec()
        );
    }

    #[test]
    fn options_become_separate_arguments() {
        let job = PrintJob {
            printer: Some("-odd name".to_string()),
            copies: 3,
            collate: false,
            pages: Some(vec![(1, 3), (5, 5)]),
            ..job()
        };
        let args = lp_args(&job);
        assert_eq!(
            args,
            [
                "-d",
                "-odd name",
                "-n",
                "3",
                "-o",
                "collate=false",
                "-P",
                "1-3,5",
                "-t",
                "关于××的请示",
                "--",
                "/tmp/-请示.pdf",
            ]
            .map(OsString::from)
            .to_vec()
        );
    }

    #[test]
    fn print_options_become_separate_arguments() {
        let job = PrintJob {
            duplex: Some(Duplex::ShortEdge),
            color: Some(ColorMode::Mono),
            paper: Some("A4".to_string()),
            tray: Some("Tray2".to_string()),
            scaling: Scaling::Fit,
            ..job()
        };
        assert_eq!(
            lp_args(&job),
            [
                "-o",
                "sides=two-sided-short-edge",
                "-o",
                "print-color-mode=monochrome",
                "-o",
                "PageSize=A4",
                "-o",
                "InputSlot=Tray2",
                "-o",
                "print-scaling=fit",
                "-t",
                "关于××的请示",
                "--",
                "/tmp/-请示.pdf",
            ]
            .map(OsString::from)
            .to_vec()
        );
    }

    #[test]
    fn reverse_output_is_an_extra_option() {
        let job = PrintJob {
            reverse: true,
            ..job()
        };
        assert_eq!(
            lp_args(&job),
            [
                "-o",
                "outputorder=reverse",
                "-t",
                "关于××的请示",
                "--",
                "/tmp/-请示.pdf",
            ]
            .map(OsString::from)
            .to_vec()
        );
        // 手动双面每一轮按单面交给 CUPS。
        assert_eq!(duplex_option(Duplex::Manual), "sides=one-sided");
    }

    /// 用假 `lp` 跑一遍真实的进程调用：参数原样到达，退出码与报错如实回传。
    #[test]
    fn lp_receives_each_argument_intact_and_reports_errors() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("临时目录");
        let record = dir.path().join("args.txt");
        let lp = dir.path().join("lp");
        let script = format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\n\
             [ \"$2\" = broken ] && {{ echo 'lp: 打印机不存在' >&2; exit 1; }}\nexit 0\n",
            record.display()
        );
        std::fs::write(&lp, script).expect("写入假 lp");
        std::fs::set_permissions(&lp, std::fs::Permissions::from_mode(0o755)).expect("置可执行");

        let run = |printer: &str| {
            let job = PrintJob {
                printer: Some(printer.to_string()),
                ..job()
            };
            let output = Command::new(&lp)
                .args(lp_args(&job))
                .output()
                .expect("运行假 lp");
            (
                output.status.success(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )
        };
        assert_eq!(run("Office"), (true, String::new()));
        let recorded = std::fs::read_to_string(&record).expect("读参数记录");
        assert_eq!(
            recorded.lines().collect::<Vec<_>>(),
            ["-d", "Office", "-t", "关于××的请示", "--", "/tmp/-请示.pdf"]
        );
        let (ok, stderr) = run("broken");
        assert!(!ok);
        assert!(stderr.contains("打印机不存在"));
    }
}
