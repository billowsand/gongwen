//! Pi 标准技能的配置目录存储：`<配置目录>/pi/skills/<name>/`。
//!
//! - 内置八份是只读默认（含 references）；用户目录里的同名技能覆盖默认，删掉用户副本即恢复内置。
//! - 停用状态单独写在 `<配置目录>/pi/skills-disabled.json`，不改写旧 `skills/` 目录。
//! - 新建、复制、导入都用同目录暂存目录加 rename 落盘；保存 SKILL.md 走
//!   `storage::atomic_write`。解析失败不写文件，已有包和编辑缓冲不会被清掉。
//! - 包内辅助文件原样保留；脚本只是文件，不执行、不扩大 host 权限。

use super::parser;
use crate::ai_services::skill_package::{Limits, Package, no_link, relative_path_with};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const LIMITS: Limits = Limits::PI;
const DISABLED_FILE: &str = "pi/skills-disabled.json";
/// 公共语法包是程序随包只读资源，用户技能不能占用这个目录名。
pub(crate) const RESERVED_NAME: &str = "gongwen-markdown";
const MAX_DISABLED_BYTES: u64 = 64 * 1024;

/// 新建技能模板。正文只写自然语言，不放 flow / step / tools 等旧执行字段。
const TEMPLATE: &str = "---\nname: {name}\ndescription: 一句话写清这个技能做什么、什么时候用。\n{metadata}---\n\n# {title}\n\n## 什么时候用\n\n写清适用的任务和不该用的场景。\n\n## 怎么做\n\n用自然语言写步骤：先看哪些材料、按什么顺序写、哪些事实必须核对。\n\n## 交付\n\n生成或修改正文时调用 submit_document；只审核或找资料时调用 finish_report。\n";

pub(crate) fn skills_root() -> Result<PathBuf, String> {
    let pi =
        crate::storage::slot_subdir("pi").map_err(|error| format!("读不到配置目录：{error:#}"))?;
    no_link(&pi).map_err(|error| format!("{error:#}"))?;
    let root = pi.join("skills");
    no_link(&root).map_err(|error| format!("{error:#}"))?;
    std::fs::create_dir_all(&root)
        .map_err(|error| format!("建不了 Pi 技能目录（{}）：{error}", root.display()))?;
    no_link(&root).map_err(|error| format!("{error:#}"))?;
    Ok(root)
}

fn disabled_path() -> Result<PathBuf, String> {
    let pi =
        crate::storage::slot_subdir("pi").map_err(|error| format!("读不到配置目录：{error:#}"))?;
    no_link(&pi).map_err(|error| format!("{error:#}"))?;
    crate::storage::slot_file(DISABLED_FILE).map_err(|error| format!("{error:#}"))
}

/// 用户技能目录。名字不合规或占用公共语法包目录时直接拒绝。
pub(crate) fn user_dir(name: &str) -> Result<PathBuf, String> {
    if !parser::valid_name(name) {
        return Err(format!(
            "技能名「{name}」不合规：只能用小写字母、数字与单个短横线，最长 64；不能用路径或设备名。"
        ));
    }
    if name == RESERVED_NAME {
        return Err(format!(
            "「{RESERVED_NAME}」是程序随包的公共语法包，不能作为用户技能名。"
        ));
    }
    let root = skills_root()?;
    let dir = root.join(name);
    no_link(&dir).map_err(|error| format!("{error:#}"))?;
    Ok(dir)
}

type Bundles = BTreeMap<String, BTreeMap<String, Vec<u8>>>;

fn bundle_path() -> Result<PathBuf, String> {
    Ok(crate::storage::slot_subdir("pi")
        .map_err(|e| e.to_string())?
        .join("user-skills.json"))
}

fn read_bundles() -> Result<Bundles, String> {
    let path = bundle_path()?;
    let mut bundles: Bundles = if path.try_exists().map_err(|e| e.to_string())? {
        let raw =
            crate::vault::read_path(&path, "pi", 128 * 1024 * 1024).map_err(|e| e.to_string())?;
        serde_json::from_slice(&raw).map_err(|e| format!("用户技能容器损坏：{e}"))?
    } else {
        BTreeMap::new()
    };
    let root = skills_root()?;
    let mut legacy = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if no_link(&entry.path()).is_err() {
            continue;
        }
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let package = Package::read_dir_with_reader(&entry.path(), LIMITS, |file| {
            crate::vault::read_path(file, "pi", LIMITS.max_file)
        })
        .map_err(|e| e.to_string())?;
        if let Some(previous) = bundles.get(&name) {
            if !package
                .files
                .iter()
                .all(|(key, value)| previous.get(key) == Some(value))
            {
                return Err(format!("用户技能「{name}」容器与旧目录冲突，已保留两份"));
            }
        } else {
            bundles.insert(name, package.files);
        }
        legacy.push(entry.path());
    }
    if !legacy.is_empty() {
        write_bundles(&bundles)?;
        for dir in legacy {
            // 保存成功后逐个覆写旧文件；失败上报，下次读取继续，不宣称迁移完成。
            let mut pending = vec![dir.clone()];
            while let Some(parent) = pending.pop() {
                for entry in std::fs::read_dir(parent).map_err(|e| e.to_string())? {
                    let entry = entry.map_err(|e| e.to_string())?;
                    no_link(&entry.path()).map_err(|e| e.to_string())?;
                    if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                        pending.push(entry.path());
                    } else {
                        crate::vault::erase_file_checked(&entry.path())
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
            std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
        }
    }
    Ok(bundles)
}

fn write_bundles(bundles: &Bundles) -> Result<(), String> {
    let bytes = serde_json::to_vec(bundles).map_err(|e| e.to_string())?;
    if bytes.len() > 128 * 1024 * 1024 {
        return Err("用户技能总量超过容器上限".into());
    }
    crate::vault::write_path(&bundle_path()?, "pi", &bytes).map_err(|e| e.to_string())
}

pub(crate) fn has_user_copy(name: &str) -> bool {
    read_bundles().is_ok_and(|bundles| bundles.contains_key(name))
}

pub(crate) fn user_dirs() -> Result<Vec<(String, PathBuf)>, String> {
    let root = skills_root()?;
    let mut dirs: BTreeMap<_, _> = read_bundles()?
        .keys()
        .map(|name| (name.clone(), root.join(name)))
        .collect();
    for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_symlink() {
            dirs.insert(
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            );
        }
    }
    Ok(dirs.into_iter().collect())
}

pub(crate) fn known_name(name: &str) -> bool {
    super::builtin_text(name).is_some() || has_user_copy(name)
}

/// 内置技能的完整包：SKILL.md 与它按需读取的 references 一起编进二进制。
/// 新建工作区、复制为用户副本、导出都走这一份，不在源码目录之外另找文件。
pub(crate) fn builtin_package(name: &str) -> Result<Package, String> {
    if !super::BUILTINS.contains(&name) {
        return Err(format!("没有名为「{name}」的内置 Pi 技能"));
    }
    let mut package = Package::default();
    let mut count = 0usize;
    for (skill, relative, text) in super::FILES {
        if *skill != name {
            continue;
        }
        package
            .insert_with((*relative).to_string(), text.as_bytes().to_vec(), LIMITS)
            .map_err(|error| format!("{error:#}"))?;
        count += 1;
    }
    if count == 0 {
        return Err(format!("内置技能「{name}」没有任何文件"));
    }
    Ok(package)
}

/// 读一份技能的完整包：用户副本优先，没有用户副本就用内置。
pub(crate) fn load_package(name: &str) -> Result<Package, String> {
    if let Some(files) = read_bundles()?.remove(name) {
        return Ok(Package { files });
    }
    builtin_package(name)
}

pub(crate) fn read_text(name: &str, relative: &str) -> Result<String, String> {
    let package = load_package(name)?;
    let relative = relative_path_with(relative, LIMITS).map_err(|error| format!("{error:#}"))?;
    let bytes = package
        .files
        .get(&relative)
        .ok_or_else(|| format!("「{name}」里没有 {relative}"))?;
    String::from_utf8(bytes.clone()).map_err(|_| format!("{relative} 不是 UTF-8 文本"))
}

pub(crate) fn save_text(name: &str, relative: &str, text: &str) -> Result<(), String> {
    if !has_user_copy(name) {
        return Err(format!(
            "「{name}」还是内置只读技能。请先「复制为我的技能」再编辑。"
        ));
    }
    let relative = relative_path_with(relative, LIMITS).map_err(|error| format!("{error:#}"))?;
    if text.len() as u64 > LIMITS.max_file {
        return Err(format!(
            "文件超过 {} MB，没有保存。",
            LIMITS.max_file / 1024 / 1024
        ));
    }
    if relative == "SKILL.md" {
        parser::parse(name, text)?;
    }
    let package = load_package(name)?;
    let previous = package.files.get(&relative).map_or(0, Vec::len);
    if !package.files.contains_key(&relative) && package.files.len() >= LIMITS.max_files {
        return Err(format!(
            "技能包文件数量超过 {}，没有保存。",
            LIMITS.max_files
        ));
    }
    if package.files.values().map(Vec::len).sum::<usize>() - previous + text.len()
        > LIMITS.max_total
    {
        return Err(format!(
            "技能包超过 {} MB，没有保存。",
            LIMITS.max_total / 1024 / 1024
        ));
    }
    let mut bundles = read_bundles()?;
    bundles
        .get_mut(name)
        .ok_or("用户技能不存在")?
        .insert(relative, text.as_bytes().to_vec());
    write_bundles(&bundles)?;
    super::bump_revision();
    Ok(())
}

pub(crate) fn create(name: &str, display_name: Option<&str>) -> Result<(), String> {
    if !parser::valid_name(name) {
        return Err(format!(
            "技能名「{name}」不合规：只能用小写字母、数字与单个短横线，最长 64。"
        ));
    }
    if name == RESERVED_NAME {
        return Err(format!("「{RESERVED_NAME}」是公共语法包，不能新建覆盖。"));
    }
    if super::builtin_text(name).is_some() {
        return Err(format!(
            "「{name}」是内置技能；要改它请用「复制为我的技能」。"
        ));
    }
    user_dir(name)?;
    if has_user_copy(name) {
        return Err(format!("已经有名为「{name}」的技能了。"));
    }
    let display = display_name
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    let metadata = match &display {
        Some(text) => {
            let value = serde_yaml::to_string(text).map_err(|error| format!("{error}"))?;
            format!("metadata:\n  display_name: {}\n", value.trim())
        }
        None => String::new(),
    };
    let title = display.clone().unwrap_or_else(|| name.to_string());
    let text = TEMPLATE
        .replace("{name}", name)
        .replace("{metadata}", &metadata)
        .replace("{title}", &title);
    parser::parse(name, &text)?;
    let mut package = Package::default();
    package
        .insert_with("SKILL.md".into(), text.into_bytes(), LIMITS)
        .map_err(|error| format!("{error:#}"))?;
    install_new(name, &package)
}

pub(crate) fn copy_builtin(name: &str) -> Result<(), String> {
    if super::builtin_text(name).is_none() {
        return Err(format!("「{name}」不是内置技能，不能复制内置版本。"));
    }
    if has_user_copy(name) {
        return Err(format!("「{name}」已经有我的副本了，直接编辑它即可。"));
    }
    install_new(name, &builtin_package(name)?)
}

/// 将已有完整技能包复制到一个新的用户技能名；目标冲突一律拒绝。
pub(crate) fn copy_to(source: &str, target: &str, display: Option<&str>) -> Result<(), String> {
    user_dir(target)?;
    if super::builtin_text(target).is_some() || has_user_copy(target) {
        return Err(format!("已经有名为「{target}」的技能了，没有覆盖。"));
    }
    let mut package = load_package(source)?;
    let text = package.files.get("SKILL.md").ok_or("缺少 SKILL.md")?;
    let text = std::str::from_utf8(text).map_err(|_| "SKILL.md 不是 UTF-8 文本")?;
    let renamed = parser::renamed(source, text, target, display)?;
    package.files.remove("SKILL.md");
    package
        .insert_with("SKILL.md".into(), renamed.into_bytes(), LIMITS)
        .map_err(|error| error.to_string())?;
    install_new(target, &package)
}

pub(crate) fn restore_builtin(name: &str) -> Result<(), String> {
    if super::builtin_text(name).is_none() {
        return Err(format!("「{name}」不是内置技能，没有可恢复的内置版本。"));
    }
    let mut disabled = read_disabled_strict()?;
    let mut bundles = read_bundles()?;
    bundles.remove(name);
    write_bundles(&bundles)?;
    disabled.retain(|id| id != name);
    write_disabled(&disabled)?;
    super::bump_revision();
    Ok(())
}

/// 删除用户副本。内置技能没有用户副本时不能删除，只能停用。
pub(crate) fn delete(name: &str) -> Result<(), String> {
    let mut bundles = read_bundles()?;
    if bundles.remove(name).is_none() {
        return Err(format!("没有可删除的用户技能「{name}」"));
    }
    write_bundles(&bundles)?;
    // 纯用户技能删除时一并清掉停用记录；内置覆盖删除后只恢复内置，保留原来的启停状态。
    if super::builtin_text(name).is_none()
        && let Ok(mut disabled) = read_disabled_strict()
    {
        let before = disabled.len();
        disabled.retain(|id| id != name);
        if disabled.len() != before {
            write_disabled(&disabled)?;
        }
    }
    super::bump_revision();
    Ok(())
}

pub(crate) fn set_enabled(name: &str, enabled: bool) -> Result<(), String> {
    if !known_name(name) {
        return Err(format!("没有名为「{name}」的 Pi 技能。"));
    }
    let mut disabled = read_disabled_strict()?;
    disabled.retain(|id| id != name);
    if !enabled {
        disabled.push(name.to_string());
    }
    write_disabled(&disabled)?;
    super::bump_revision();
    Ok(())
}

fn read_disabled_parts() -> (Vec<String>, Vec<String>) {
    let Ok(path) = disabled_path() else {
        return (Vec::new(), Vec::new());
    };
    if !path.is_file() {
        return (Vec::new(), Vec::new());
    }
    let bytes = match crate::vault::read_path(&path, "pi", MAX_DISABLED_BYTES) {
        Ok(bytes) => bytes,
        Err(error) => {
            return (
                Vec::new(),
                vec![format!("Pi 技能停用清单读不出来：{error}")],
            );
        }
    };
    match serde_json::from_slice::<Vec<String>>(&bytes) {
        Ok(ids) => (ids, Vec::new()),
        Err(error) => (
            Vec::new(),
            vec![format!(
                "Pi 技能停用清单不是合法 JSON（{}）：{error}；已按全部启用处理，下次启停前请先修好它。",
                path.display()
            )],
        ),
    }
}

fn read_disabled_strict() -> Result<Vec<String>, String> {
    let (ids, notes) = read_disabled_parts();
    if let Some(note) = notes.into_iter().next() {
        return Err(note);
    }
    Ok(ids)
}

fn write_disabled(ids: &[String]) -> Result<(), String> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    let path = disabled_path()?;
    let bytes = serde_json::to_vec_pretty(&ids).map_err(|error| error.to_string())?;
    crate::vault::write_path(&path, "pi", &bytes)
        .map_err(|error| format!("写不出技能停用清单（{}）：{error:#}", path.display()))
}

fn install_new(name: &str, package: &Package) -> Result<(), String> {
    user_dir(name)?; // 名称校验，虚拟路径不落盘。
    let mut bundles = read_bundles()?;
    if bundles.contains_key(name) {
        return Err(format!("已经有名为「{name}」的技能"));
    }
    bundles.insert(name.to_owned(), package.files.clone());
    write_bundles(&bundles)?;
    super::bump_revision();
    Ok(())
}

fn extract_package(files: &Package, prefix: &str) -> Package {
    let prefix = format!("{prefix}/");
    Package {
        files: files
            .files
            .iter()
            .filter_map(|(name, bytes)| {
                name.strip_prefix(&prefix)
                    .map(|name| (name.to_string(), bytes.clone()))
            })
            .collect(),
    }
}

fn collect_packages(path: &Path) -> Result<BTreeMap<String, Package>, String> {
    let mut packages = BTreeMap::new();
    if path.is_file() {
        // 归档可有外层封装目录；技能内部仍在 check_import 中按 PI 深度逐项校验。
        let archive_limits = Limits {
            max_depth: 64,
            ..LIMITS
        };
        let files =
            Package::read_zip_with(path, archive_limits).map_err(|error| format!("{error:#}"))?;
        if files.files.contains_key("SKILL.md") {
            let id = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            packages.insert(id, files.clone());
        }
        let roots = files
            .files
            .keys()
            .filter_map(|name| name.strip_suffix("/SKILL.md").map(str::to_owned))
            .collect::<Vec<_>>();
        for root in roots {
            let id = root.rsplit('/').next().unwrap_or_default().to_string();
            if packages
                .insert(id.clone(), extract_package(&files, &root))
                .is_some()
            {
                return Err(format!("技能包里有两个同名的技能「{id}」。"));
            }
        }
    } else if path.join("SKILL.md").is_file() {
        let id = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        packages.insert(
            id,
            Package::read_dir_with(path, LIMITS).map_err(|error| format!("{error:#}"))?,
        );
    } else {
        for entry in std::fs::read_dir(path)
            .map_err(|error| format!("读不了导入目录（{}）：{error}", path.display()))?
        {
            let entry = entry.map_err(|error| format!("读导入目录失败：{error}"))?;
            let dir = entry.path();
            if dir.join("SKILL.md").is_file() {
                let id = dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string();
                let package =
                    Package::read_dir_with(&dir, LIMITS).map_err(|error| format!("{error:#}"))?;
                if packages.insert(id.clone(), package).is_some() {
                    return Err(format!("导入目录里有两个同名的技能「{id}」。"));
                }
            }
        }
    }
    if packages.is_empty() {
        return Err(format!("{} 里没有找到 SKILL.md。", path.display()));
    }
    Ok(packages)
}

fn check_import(name: &str, package: &Package) -> Result<(), String> {
    for relative in package.files.keys() {
        relative_path_with(relative, LIMITS).map_err(|error| format!("{error:#}"))?;
    }
    if !parser::valid_name(name) {
        return Err(format!(
            "技能名「{name}」不合规：只能用小写字母、数字与单个短横线，最长 64。"
        ));
    }
    if name == RESERVED_NAME {
        return Err(format!(
            "「{RESERVED_NAME}」是程序随包的公共语法包，不能用用户技能覆盖。"
        ));
    }
    let entry = package
        .files
        .get("SKILL.md")
        .ok_or_else(|| format!("「{name}」缺少 SKILL.md。"))?;
    let text = std::str::from_utf8(entry)
        .map_err(|_| format!("「{name}」的 SKILL.md 不是 UTF-8 文本。"))?;
    parser::parse(name, text)?;
    if has_user_copy(name) {
        return Err(format!(
            "已经存在名为「{name}」的技能，导入不会覆盖。请先复制或删除现有技能，再导入。"
        ));
    }
    Ok(())
}

/// 导入文件夹或标准技能包。先校验全部包，再逐个安装；任何一个不合规都不写盘。
pub(crate) fn import(path: &Path) -> Result<Vec<String>, String> {
    let packages = collect_packages(path)?;
    for (name, package) in &packages {
        check_import(name, package)?;
    }
    // 再查一遍同名冲突，避免一组包里两个名字大小写不同却落到同一目录。
    let mut seen = BTreeMap::new();
    for (name, package) in &packages {
        let lower = name.to_lowercase();
        if seen.insert(lower, name).is_some() {
            return Err(format!("导入包里有重名的技能「{name}」。"));
        }
        install_new(name, package)?;
    }
    Ok(packages.keys().cloned().collect())
}

pub(crate) fn export(name: &str, target: &Path) -> Result<PathBuf, String> {
    let package = load_package(name)?;
    if target
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("zip") || extension.eq_ignore_ascii_case("skill")
        })
    {
        no_link(target).map_err(|error| format!("{error:#}"))?;
        package
            .write_zip(target, name)
            .map_err(|error| format!("导出失败：{error:#}"))?;
        Ok(target.to_path_buf())
    } else {
        let root = target.join(name);
        no_link(target).map_err(|error| format!("{error:#}"))?;
        no_link(&root).map_err(|error| format!("{error:#}"))?;
        package
            .write_dir(&root)
            .map_err(|error| format!("导出失败：{error:#}"))?;
        Ok(root.join("SKILL.md"))
    }
}

/// 旧流程技能目录是否还存在。只作界面提示，不读取、不迁移、不删除。
pub(crate) fn legacy_flow_present() -> bool {
    crate::storage::config_dir()
        .map(|config| config.join("skills").is_dir())
        .unwrap_or(false)
}

/// 供 catalog 使用的停用清单与提示。
pub(crate) fn disabled_names() -> (Vec<String>, Vec<String>) {
    read_disabled_parts()
}

/// 读技能文本（用户副本或内置），给测试与界面初始缓冲用。
#[cfg(test)]
pub(crate) fn skill_text(name: &str) -> Result<String, String> {
    read_text(name, "SKILL.md")
}

#[cfg(test)]
pub(crate) fn corrupt_skill_for_test(name: &str, text: &str) {
    let mut bundles = read_bundles().unwrap();
    bundles
        .get_mut(name)
        .unwrap()
        .insert("SKILL.md".into(), text.as_bytes().to_vec());
    write_bundles(&bundles).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConfigDir {
        dir: tempfile::TempDir,
    }
    impl ConfigDir {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
            Self { dir }
        }
    }
    impl Drop for ConfigDir {
        fn drop(&mut self) {
            crate::storage::set_test_config_dir(None);
            let _ = &self.dir;
        }
    }

    #[test]
    fn new_skills_are_standard_and_edits_persist() {
        let _dir = ConfigDir::new();
        create("my-skill", Some("我的技能")).unwrap();
        let text = skill_text("my-skill").unwrap();
        assert!(text.contains("name: my-skill"));
        assert!(text.contains("display_name: 我的技能"));
        assert!(text.contains("submit_document"));
        save_text(
            "my-skill",
            "SKILL.md",
            &text.replace("## 怎么做", "## 具体步骤"),
        )
        .unwrap();
        assert!(skill_text("my-skill").unwrap().contains("## 具体步骤"));
        // 解析失败不写盘，旧文本还在。
        let before = skill_text("my-skill").unwrap();
        assert!(save_text("my-skill", "SKILL.md", "没有 frontmatter").is_err());
        assert_eq!(skill_text("my-skill").unwrap(), before);
        assert!(create("my-skill", None).is_err());
        assert!(create("gongwen-draft", None).is_err(), "内置要用复制");
        delete("my-skill").unwrap();
        assert!(!has_user_copy("my-skill"));
    }

    #[test]
    fn the_deepest_valid_file_survives_export_and_reimport() {
        let config = ConfigDir::new();
        create("deep", None).unwrap();
        let relative = "a/b/c/d/e/f/g/file.txt";
        save_text("deep", relative, "最深的合法文件").unwrap();
        let zip = config.dir.path().join("deep.zip");
        export("deep", &zip).unwrap();
        delete("deep").unwrap();
        import(&zip).unwrap();
        assert_eq!(read_text("deep", relative).unwrap(), "最深的合法文件");
        assert!(save_text("deep", "a/b/c/d/e/f/g/h/file.txt", "超深").is_err());
    }

    #[test]
    fn copying_a_custom_skill_preserves_its_full_package_and_metadata() {
        let _config = ConfigDir::new();
        create("original", None).unwrap();
        let source = "---\nname: original\ndescription: |\n  多行说明\n  第二行\nmetadata:\n  custom: [one, two]\n---\n\n# 步骤\n正文保留。\n";
        save_text("original", "SKILL.md", source).unwrap();
        save_text("original", "assets/example.txt", "辅助资源").unwrap();
        copy_to("original", "duplicate", Some("复制件")).unwrap();
        let copied = skill_text("duplicate").unwrap();
        let parsed = parser::parse("duplicate", &copied).unwrap();
        assert_eq!(parsed.display_name.as_deref(), Some("复制件"));
        assert!(copied.contains("custom:"));
        assert!(copied.contains("two"));
        assert_eq!(
            read_text("duplicate", "assets/example.txt").unwrap(),
            "辅助资源"
        );
        assert_eq!(skill_text("original").unwrap(), source);
        assert!(copy_to("original", "duplicate", None).is_err());
        assert!(copy_to("original", "gongwen-draft", None).is_err());
    }

    #[test]
    fn builtin_packages_carry_their_references_into_copies_and_exports() {
        let config = ConfigDir::new();
        let builtin = load_package("gongwen-revise").unwrap();
        for relative in [
            "SKILL.md",
            "references/condense.md",
            "references/normalize.md",
            "references/polish.md",
            "references/tone.md",
        ] {
            assert!(builtin.files.contains_key(relative), "缺 {relative}");
        }
        // 复制为用户副本：参考文件一起复制。
        copy_builtin("gongwen-revise").unwrap();
        assert_eq!(load_package("gongwen-revise").unwrap().files, builtin.files);
        // 导出 ZIP：参考文件在包里。
        let zip = config.dir.path().join("revise.zip");
        export("gongwen-revise", &zip).unwrap();
        let archive = Package::read_zip_with(
            &zip,
            Limits {
                max_depth: 16,
                ..LIMITS
            },
        )
        .unwrap();
        assert!(
            archive
                .files
                .contains_key("gongwen-revise/references/condense.md"),
            "{:?}",
            archive.files.keys().collect::<Vec<_>>()
        );
        // 恢复内置：包还在，不因为删掉用户副本就少文件。
        restore_builtin("gongwen-revise").unwrap();
        assert_eq!(load_package("gongwen-revise").unwrap().files, builtin.files);
    }

    #[test]
    fn builtin_overrides_can_be_copied_and_restored() {
        let _dir = ConfigDir::new();
        let original = skill_text("gongwen-draft").unwrap();
        copy_builtin("gongwen-draft").unwrap();
        assert!(has_user_copy("gongwen-draft"));
        let mut text = skill_text("gongwen-draft").unwrap();
        text = text.replace("按用户要求与材料起草", "按用户要求与材料起草（我的版本）");
        save_text("gongwen-draft", "SKILL.md", &text).unwrap();
        assert!(skill_text("gongwen-draft").unwrap().contains("我的版本"));
        restore_builtin("gongwen-draft").unwrap();
        assert!(!has_user_copy("gongwen-draft"));
        assert_eq!(skill_text("gongwen-draft").unwrap(), original);
    }

    #[test]
    fn disabled_state_is_separate_and_atomic() {
        let _dir = ConfigDir::new();
        set_enabled("gongwen-review", false).unwrap();
        let (disabled, notes) = disabled_names();
        assert_eq!(disabled, vec!["gongwen-review".to_string()]);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(disabled_path().unwrap().is_file());
        assert!(!legacy_flow_present());
        set_enabled("gongwen-review", true).unwrap();
        assert!(disabled_names().0.is_empty());
        assert!(set_enabled("no-such-skill", false).is_err());
    }

    #[test]
    fn deleting_a_builtin_override_keeps_its_disabled_state() {
        let _dir = ConfigDir::new();
        copy_builtin("gongwen-draft").unwrap();
        set_enabled("gongwen-draft", false).unwrap();
        delete("gongwen-draft").unwrap();
        assert!(!has_user_copy("gongwen-draft"));
        assert_eq!(disabled_names().0, vec!["gongwen-draft".to_string()]);
        // 纯用户技能删除后不留停用记录，重新创建时默认启用。
        create("temp-skill", None).unwrap();
        set_enabled("temp-skill", false).unwrap();
        delete("temp-skill").unwrap();
        assert!(!disabled_names().0.iter().any(|id| id == "temp-skill"));
    }

    #[test]
    fn package_round_trip_keeps_auxiliary_files_and_rejects_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let _config = ConfigDir::new();
        create("with-files", None).unwrap();
        save_text("with-files", "references/依据.md", "# 依据\n保留出处。").unwrap();
        save_text("with-files", "scripts/check.py", "print('不执行')").unwrap();
        save_text("with-files", "assets/示例.txt", "assets").unwrap();
        let zip = dir.path().join("with-files.zip");
        export("with-files", &zip).unwrap();
        let original = load_package("with-files").unwrap().files;
        delete("with-files").unwrap();
        assert_eq!(import(&zip).unwrap(), vec!["with-files".to_string()]);
        assert_eq!(load_package("with-files").unwrap().files, original);
        // 同名冲突拒绝，不覆盖原包。
        let error = import(&zip).unwrap_err();
        assert!(error.contains("不会覆盖"), "{error}");
        assert_eq!(load_package("with-files").unwrap().files, original);
        // 文件夹导出与导入。
        let folder = dir.path().join("out");
        export("with-files", &folder).unwrap();
        delete("with-files").unwrap();
        assert_eq!(import(&folder).unwrap(), vec!["with-files".to_string()]);
        delete("with-files").unwrap();
        assert!(!has_user_copy("with-files"));
    }

    #[test]
    fn legacy_flow_and_unsafe_archives_are_refused_before_any_write() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let _config = ConfigDir::new();
        // 旧 flow 包：明确要求改写，不安装。
        let flow = dir.path().join("mine.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&flow).unwrap());
        zip.start_file("mine/SKILL.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"---\nname: mine\ndescription: x\nflow:\n  - step: a\n---\nbody")
            .unwrap();
        zip.finish().unwrap();
        let error = import(&flow).unwrap_err();
        assert!(error.contains("旧流程"), "{error}");
        assert!(!has_user_copy("mine"));
        // 越界路径在写盘前拒绝。
        for (index, path) in ["mine/../escape.txt", "C:/escape.txt", "mine/CON.txt"]
            .iter()
            .enumerate()
        {
            let zip_path = dir.path().join(format!("unsafe-{index}.zip"));
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            zip.start_file("mine/SKILL.md", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"---\nname: mine\ndescription: x\n---\nbody")
                .unwrap();
            zip.start_file(*path, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"bad").unwrap();
            zip.finish().unwrap();
            assert!(import(&zip_path).is_err(), "{path}");
            assert!(!has_user_copy("mine"));
        }
    }

    #[test]
    fn invalid_package_does_not_install_auxiliary_files() {
        let dir = tempfile::tempdir().unwrap();
        let _config = ConfigDir::new();
        let root = dir.path().join("bad");
        std::fs::create_dir_all(root.join("references")).unwrap();
        std::fs::write(root.join("SKILL.md"), "无 YAML 入口").unwrap();
        std::fs::write(root.join("references/材料.md"), "材料").unwrap();
        let error = import(&root).unwrap_err();
        assert!(error.contains("bad"), "{error}");
        assert!(!user_dir("bad").unwrap().exists());
    }
}
