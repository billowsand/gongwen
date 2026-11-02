//! 有效 Pi 技能目录：内置五份 + 用户目录覆盖 + 停用清单。

use super::parser;
use super::store;

#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub(crate) name: String,
    /// 界面显示名：标准 metadata 显示名 > 内置中文名 > name。
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) display_name: Option<String>,
    pub(crate) builtin: bool,
    /// 配置目录里有用户副本（内置技能被覆盖时为 true）。
    pub(crate) user: bool,
    pub(crate) enabled: bool,
    /// 读取或解析失败的原因；有错误时不能执行，也不列进 `/` 与下拉框。
    pub(crate) error: Option<String>,
}

impl Entry {
    pub(crate) fn loadable(&self) -> bool {
        self.enabled && self.error.is_none()
    }

    pub(crate) fn overridden(&self) -> bool {
        self.builtin && self.user
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Catalog {
    pub(crate) entries: Vec<Entry>,
    pub(crate) notes: Vec<String>,
    /// 旧流程技能目录还在：界面提示“保留但不供 Pi 执行”，不自动转换。
    pub(crate) legacy_flow: bool,
}

impl Catalog {
    pub(crate) fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    pub(crate) fn enabled(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|entry| entry.loadable())
            .collect()
    }

    /// 选了停用 / 删除 / 损坏的技能时给出明确原因，不偷偷切成自动。
    pub(crate) fn resolve(&self, selected: Option<&str>) -> Result<Option<String>, String> {
        let Some(selected) = selected else {
            if self.enabled().is_empty() {
                return Err(
                    "现在没有启用的 Pi 技能，Pi 不会启动。请到「AI 管理 → 技能」里启用至少一个。"
                        .into(),
                );
            }
            return Ok(None);
        };
        let Some(entry) = self.get(selected) else {
            return Err(format!(
                "选中的 Pi 技能「{selected}」已经被删除或改名。请回到侧栏重新选择技能，或改为自动。"
            ));
        };
        if let Some(error) = &entry.error {
            return Err(format!("选中的 Pi 技能「{selected}」还不能执行：{error}"));
        }
        if !entry.enabled {
            return Err(format!(
                "选中的 Pi 技能「{selected}」已停用。请先在「AI 管理 → 技能」里启用，或在侧栏改选其他技能。"
            ));
        }
        Ok(Some(selected.to_string()))
    }

    pub(crate) fn label_for(&self, name: &str) -> String {
        self.get(name)
            .map(|entry| entry.label.clone())
            .unwrap_or_else(|| name.to_string())
    }
}

/// 读配置目录，生成一份有效目录。单个技能坏了只记错误，不让整页打不开。
pub(crate) fn load() -> Catalog {
    let mut catalog = Catalog {
        legacy_flow: store::legacy_flow_present(),
        ..Catalog::default()
    };
    let (disabled, notes) = store::disabled_names();
    catalog.notes.extend(notes);
    let disabled = disabled
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    let mut user_entries = std::collections::BTreeMap::<String, Entry>::new();
    match store::user_dirs() {
        Ok(dirs) => {
            for (name, dir) in dirs {
                if name == store::RESERVED_NAME {
                    catalog.notes.push(format!(
                        "配置目录里的「{name}」是公共语法包名称，已忽略；用户技能不能覆盖它。"
                    ));
                    continue;
                }
                if !parser::valid_name(&name) {
                    catalog.notes.push(format!(
                        "配置目录里的技能目录「{name}」不合规（名字要用小写字母、数字和单个短横线），没有加载。"
                    ));
                    continue;
                }
                let entry = read_entry(&name, &dir, &disabled);
                user_entries.insert(name, entry);
            }
        }
        Err(error) => catalog.notes.push(error),
    }

    for (name, _) in super::BUILTINS {
        let mut entry = match user_entries.remove(*name) {
            Some(entry) => entry,
            None => builtin_entry(name, &disabled),
        };
        // 内置中文显示名是产品固定文案；用户没有写显示名时沿用。
        if entry.display_name.is_none()
            && let Some(label) = super::builtin_label(name)
        {
            entry.label = label.to_string();
        }
        catalog.entries.push(entry);
    }
    catalog.entries.extend(user_entries.into_values());
    catalog
}

fn builtin_entry(name: &str, disabled: &std::collections::BTreeSet<String>) -> Entry {
    let label = super::builtin_label(name).unwrap_or(name).to_string();
    match super::builtin_text(name) {
        Some(text) => match parser::parse(name, text) {
            Ok(parsed) => Entry {
                name: name.to_string(),
                label: parsed.display_name.clone().unwrap_or_else(|| label.clone()),
                description: parsed.description,
                display_name: parsed.display_name,
                builtin: true,
                user: false,
                enabled: !disabled.contains(name),
                error: None,
            },
            Err(error) => Entry {
                name: name.to_string(),
                label,
                description: String::new(),
                display_name: None,
                builtin: true,
                user: false,
                enabled: false,
                error: Some(error),
            },
        },
        None => Entry {
            name: name.to_string(),
            label,
            description: String::new(),
            display_name: None,
            builtin: true,
            user: false,
            enabled: false,
            error: Some(format!("没有内置技能「{name}」")),
        },
    }
}

fn read_entry(
    name: &str,
    dir: &std::path::Path,
    disabled: &std::collections::BTreeSet<String>,
) -> Entry {
    let label = name.to_string();
    let enabled = !disabled.contains(name);
    let file = dir.join("SKILL.md");
    for path in [dir, file.as_path()] {
        if let Err(error) = crate::ai_services::skill_package::no_link(path) {
            return broken_entry(name, enabled, format!("{error:#}"));
        }
    }
    if !file.is_file() {
        return Entry {
            name: name.to_string(),
            label,
            description: String::new(),
            display_name: None,
            builtin: super::builtin_text(name).is_some(),
            user: true,
            enabled: false,
            error: Some(format!("「{name}」缺了 SKILL.md")),
        };
    }
    let bytes = match crate::file_limits::read_bytes(
        &file,
        crate::ai_services::skill_package::Limits::PI.max_file,
        "技能文件",
    ) {
        Ok(bytes) => bytes,
        Err(error) => {
            return broken_entry(name, enabled, format!("{error}"));
        }
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return broken_entry(name, enabled, "SKILL.md 不是 UTF-8 文本".into());
    };
    match parser::parse(name, &text) {
        Ok(parsed) => Entry {
            name: name.to_string(),
            label: parsed
                .display_name
                .clone()
                .or_else(|| super::builtin_label(name).map(str::to_string))
                .unwrap_or_else(|| label.clone()),
            description: parsed.description,
            display_name: parsed.display_name,
            builtin: super::builtin_text(name).is_some(),
            user: true,
            enabled,
            error: None,
        },
        Err(error) => broken_entry(name, enabled, error),
    }
}

fn broken_entry(name: &str, enabled: bool, error: String) -> Entry {
    Entry {
        name: name.to_string(),
        label: super::builtin_label(name).unwrap_or(name).to_string(),
        description: String::new(),
        display_name: None,
        builtin: super::builtin_text(name).is_some(),
        user: true,
        enabled,
        error: Some(error),
    }
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
    fn builtins_are_listed_with_chinese_labels_and_catalog_can_be_empty() {
        let _dir = ConfigDir::new();
        let catalog = load();
        assert_eq!(catalog.enabled().len(), 5);
        assert_eq!(catalog.label_for("gongwen-draft"), "起草");
        assert_eq!(catalog.label_for("gongwen-research"), "研究报告");
        for name in ["gongwen-draft", "gongwen-revise", "gongwen-review"] {
            store::set_enabled(name, false).unwrap();
        }
        let catalog = load();
        assert_eq!(catalog.enabled().len(), 2);
        store::set_enabled("gongwen-review", true).unwrap();
        assert_eq!(load().enabled().len(), 3);
        assert_eq!(
            load().resolve(Some("gongwen-review")),
            Ok(Some("gongwen-review".into()))
        );
        assert!(load().resolve(Some("gongwen-review")).is_ok());
        store::set_enabled("gongwen-review", false).unwrap();
        let error = load().resolve(Some("gongwen-review")).unwrap_err();
        assert!(error.contains("已停用"), "{error}");
        assert!(load().resolve(Some("no-such")).is_err());
    }

    #[test]
    fn user_copy_overrides_builtin_and_broken_copy_does_not_fall_back() {
        let _dir = ConfigDir::new();
        store::copy_builtin("gongwen-draft").unwrap();
        store::save_text(
            "gongwen-draft",
            "SKILL.md",
            "---\nname: gongwen-draft\ndescription: 我的起草\nmetadata:\n  display_name: 我的起草技能\n---\n正文",
        )
        .unwrap();
        let catalog = load();
        let entry = catalog.get("gongwen-draft").unwrap();
        assert!(entry.overridden());
        assert_eq!(entry.description, "我的起草");
        assert_eq!(entry.label, "我的起草技能");
        // 用户副本写坏：显示错误，不悄悄退回内置版本。
        store::save_text(
            "gongwen-draft",
            "SKILL.md",
            "---\nname: gongwen-draft\ndescription: x\nflow:\n  - step: a\n---\n正文",
        )
        .unwrap_err();
        let user_file = store::user_dir("gongwen-draft").unwrap().join("SKILL.md");
        std::fs::write(
            &user_file,
            "---\nname: gongwen-draft\ndescription: x\n---\n",
        )
        .unwrap();
        let catalog = load();
        let entry = catalog.get("gongwen-draft").unwrap();
        assert!(entry.error.is_some());
        assert!(catalog.resolve(Some("gongwen-draft")).is_err());
    }

    #[test]
    fn reserved_and_invalid_directories_are_reported_not_loaded() {
        let _dir = ConfigDir::new();
        let root = store::skills_root().unwrap();
        std::fs::create_dir_all(root.join(store::RESERVED_NAME)).unwrap();
        std::fs::write(root.join(store::RESERVED_NAME).join("SKILL.md"), "x").unwrap();
        std::fs::create_dir_all(root.join("Bad_Name")).unwrap();
        let catalog = load();
        assert!(
            catalog
                .entries
                .iter()
                .all(|entry| entry.name != store::RESERVED_NAME)
        );
        assert!(catalog.notes.iter().any(|note| note.contains("公共语法包")));
        assert!(catalog.notes.iter().any(|note| note.contains("不合规")));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn an_unrelated_link_does_not_hide_other_user_skills() {
        let _dir = ConfigDir::new();
        store::create("keep-me", None).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = store::skills_root().unwrap();
        let link = root.join("bad-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        #[cfg(windows)]
        {
            // 路径由本测试的隔离临时目录提供，不经字符串插入命令。
            let result = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&link)
                .arg(outside.path())
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let catalog = load();
        #[cfg(windows)]
        std::fs::remove_dir(&link).unwrap();
        #[cfg(unix)]
        std::fs::remove_file(&link).unwrap();
        assert!(catalog.get("keep-me").unwrap().loadable());
        assert!(catalog.get("bad-link").unwrap().error.is_some());
        assert!(catalog.resolve(Some("bad-link")).is_err());
    }
}
