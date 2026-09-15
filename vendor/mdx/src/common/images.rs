//! 图片落盘：把 `Inline::Image` 引用的本地图片复制到输出目录 `figures/` 下，
//! 并把 url 改写为 `figures/<文件名>`，保证主 tex 里的 `\includegraphics`
//! 相对路径在输出目录中可解析。远程图片（http/https）与找不到的本地图片
//! 保持原样（后者打印警告）。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::ast::{Block, Inline};

/// 遍历 blocks，就地改写图片 url 并复制文件。返回实际执行的复制列表 (源, 目标)。
pub fn relocate(blocks: &mut [Block], base_dir: &Path, out_dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut copied = Vec::new();
    // 目标文件名 → 来源路径，用于同名去重：同源复用同名，异源加序号后缀
    let mut name_owner: HashMap<String, PathBuf> = HashMap::new();

    for block in blocks.iter_mut() {
        let inlines: &mut Vec<Inline> = match block {
            Block::Paragraph(v) => v,
            Block::List { content, .. } => content,
            _ => continue,
        };
        for ip in inlines.iter_mut() {
            let Inline::Image { url, .. } = ip else {
                continue;
            };
            if is_remote(url) {
                continue;
            }
            let src = base_dir.join(url.as_str());
            if !src.is_file() {
                eprintln!("  警告：找不到图片 '{url}'，路径保持不变");
                continue;
            }

            let file_name = unique_name(&src, &mut name_owner);
            let dst = out_dir.join("figures").join(&file_name);
            if !dst.exists() {
                let result = dst
                    .parent()
                    .map(fs::create_dir_all)
                    .unwrap_or_else(|| Ok(()))
                    .and_then(|_| fs::copy(&src, &dst));
                match result {
                    Ok(_) => copied.push((src.clone(), dst)),
                    Err(e) => {
                        eprintln!("  警告：复制图片 '{}' 失败: {e}", src.display());
                        continue;
                    }
                }
            }
            *url = format!("figures/{file_name}");
        }
    }
    copied
}

fn is_remote(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// 取源文件名作为目标名；同名但来源不同时追加 `_2`、`_3`… 后缀。
fn unique_name(src: &Path, name_owner: &mut HashMap<String, PathBuf>) -> String {
    let base = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image.png".to_string());
    let (stem, ext) = match base.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (base.clone(), String::new()),
    };

    let mut candidate = base;
    let mut n = 1;
    loop {
        match name_owner.get(&candidate) {
            None => {
                name_owner.insert(candidate.clone(), src.to_path_buf());
                return candidate;
            }
            Some(owner) if owner == src => return candidate,
            _ => {
                n += 1;
                candidate = format!("{stem}_{n}{ext}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_block(alt: &str, url: &str) -> Block {
        Block::Paragraph(vec![Inline::Image {
            alt: alt.into(),
            url: url.into(),
            label: None,
        }])
    }

    #[test]
    fn copies_image_and_rewrites_url() {
        let src_dir = tempfile::tempdir().expect("src dir");
        let out_dir = tempfile::tempdir().expect("out dir");
        fs::create_dir_all(src_dir.path().join("figs")).unwrap();
        fs::write(src_dir.path().join("figs/a.png"), b"png").unwrap();

        let mut blocks = vec![image_block("图", "figs/a.png")];
        let copied = relocate(&mut blocks, src_dir.path(), out_dir.path());

        assert_eq!(copied.len(), 1);
        assert!(out_dir.path().join("figures/a.png").exists());
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("not a paragraph")
        };
        assert!(matches!(&inlines[0], Inline::Image { url, .. } if url == "figures/a.png"));
    }

    #[test]
    fn missing_image_kept_as_is() {
        let src_dir = tempfile::tempdir().expect("src dir");
        let out_dir = tempfile::tempdir().expect("out dir");
        let mut blocks = vec![image_block("图", "nope.png")];
        let copied = relocate(&mut blocks, src_dir.path(), out_dir.path());
        assert!(copied.is_empty());
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("not a paragraph")
        };
        assert!(matches!(&inlines[0], Inline::Image { url, .. } if url == "nope.png"));
    }

    #[test]
    fn remote_image_untouched() {
        let out_dir = tempfile::tempdir().expect("out dir");
        let mut blocks = vec![image_block("图", "https://x.test/a.png")];
        let copied = relocate(&mut blocks, Path::new("."), out_dir.path());
        assert!(copied.is_empty());
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("not a paragraph")
        };
        assert!(matches!(&inlines[0], Inline::Image { url, .. } if url == "https://x.test/a.png"));
    }

    #[test]
    fn same_basename_from_different_dirs_gets_suffix() {
        let src_dir = tempfile::tempdir().expect("src dir");
        let out_dir = tempfile::tempdir().expect("out dir");
        fs::create_dir_all(src_dir.path().join("a")).unwrap();
        fs::create_dir_all(src_dir.path().join("b")).unwrap();
        fs::write(src_dir.path().join("a/p.png"), b"1").unwrap();
        fs::write(src_dir.path().join("b/p.png"), b"2").unwrap();

        let mut blocks = vec![image_block("", "a/p.png"), image_block("", "b/p.png")];
        relocate(&mut blocks, src_dir.path(), out_dir.path());

        assert!(out_dir.path().join("figures/p.png").exists());
        assert!(out_dir.path().join("figures/p_2.png").exists());
    }
}
