//! 构建时登记内置技能包中的全部文件，辅助资料随二进制离线分发。

use std::path::Path;

pub(super) fn generate() {
    fn walk(root: &Path, dir: &Path, entries: &mut Vec<String>) {
        let mut paths = std::fs::read_dir(dir)
            .expect("读取内置技能目录")
            .map(|entry| entry.expect("读取目录项").path())
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let metadata = std::fs::symlink_metadata(&path).expect("读取技能文件属性");
            assert!(
                !metadata.file_type().is_symlink(),
                "内置技能不能使用符号链接"
            );
            if path.is_dir() {
                walk(root, &path, entries);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .expect("技能文件在资源目录内")
                    .to_str()
                    .expect("文件名是 UTF-8")
                    .replace('\\', "/");
                let Some((id, name)) = relative.split_once('/') else {
                    continue;
                };
                if root.join(id).join("SKILL.md").is_file() {
                    entries.push(format!(
                        "({id:?}, {name:?}, include_bytes!({:?})),",
                        path.to_str().expect("路径是 UTF-8")
                    ));
                }
            }
        }
    }
    let manifest =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("构建目录"));
    let root = manifest.join("assets/agent-skills");
    println!("cargo:rerun-if-changed=assets/agent-skills");
    println!("cargo:rerun-if-changed=scripts/build_skill_assets.rs");
    let mut entries = Vec::new();
    walk(&root, &root, &mut entries);
    let source = format!(
        "const BUILTIN_FILES: &[(&str, &str, &[u8])] = &[\n{}\n];\n",
        entries.join("\n")
    );
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("构建输出目录"));
    std::fs::write(output.join("builtin_skill_files.rs"), source).expect("登记内置技能包资源");
}
