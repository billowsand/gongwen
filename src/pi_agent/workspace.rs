//! 每次 Pi 运行一个持久工作区。
//!
//! ```text
//! <配置目录>/pi/workspaces/doc-<key>/run-<时间>-<UUID>/
//!   workspace/
//!     input/current.md     本轮正文快照（只读）
//!     input/request.md     本轮要求 + 技能索引 + 材料索引（只读）
//!     input/materials/     材料快照与 index.json（只读）
//!     skills/gongwen-markdown/  随包语法资料（只读）
//!     skills/pi-skills/    本轮启用的标准写作技能完整快照（只读）
//!     work/                模型的工作文件
//!     resources/           预留只读资源
//!   pi/session.sqlite      Durable 数据库，位于模型可读写区域之外
//! ```
//!
//! 工作区默认保留：中断、停止以后仍能查看本次文件。AI 管理页可按稿件只保留最近几轮，
//! 并清理已接受的旧轮次（见 [`prune_workspaces`]）。数据库也不放进 `workspace/`，
//! 模型没有路径能读到它。

use anyhow::Context;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::materials::{self, PiMaterialInput, PiMaterialMeta};
use super::skills::{Catalog, SkillSnapshot};

#[derive(Debug, Clone)]
pub(crate) struct PiWorkspace {
    /// 模型可见的工作区根目录。
    pub(crate) root: PathBuf,
    /// Durable 的 SQLite 文件，在 `workspace/` 之外。
    pub(crate) state: PathBuf,
    /// 运行目录（`workspace/` 的上一级）：运行记录、回执、成品快照都在这里。
    pub(crate) run: PathBuf,
    /// 给人看的相对位置（不含用户主目录）。
    pub(crate) label: String,
}

/// 工作区根：`配置目录/pi/workspaces`，不存在就建。
/// 槽位感知：诱饵系统用 `alt-pi`，绝不读真实系统的来源材料与运行记录。
pub(crate) fn base_dir() -> Result<PathBuf, String> {
    let dir = crate::storage::slot_subdir("pi")
        .map_err(|error| format!("读不到配置目录：{error:#}"))?
        .join("workspaces");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("建不了 Pi 工作区目录（{}）：{error}", dir.display()))?;
    Ok(dir)
}

/// 统计 / 清理时最多扫描多少个条目；超限就提前返回已统计值，不报错，界面不能被
/// 一个异常大的目录卡住。
const MAX_SCAN_ENTRIES: usize = 200_000;
/// 没有可读 `run.json` 的孤儿运行目录，额外要老过这个时长才删。
const ORPHAN_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// 工作区占用：（全部文件字节数, `run-*` 运行目录数）。只统计，不修改任何文件。
pub(crate) fn workspace_usage() -> Result<(u64, usize), String> {
    let base = base_dir()?;
    let mut bytes = 0u64;
    let mut runs = 0usize;
    let mut entries = 0usize;
    let mut pending = vec![base];
    while let Some(dir) = pending.pop() {
        let Ok(reader) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in reader.flatten() {
            if entries >= MAX_SCAN_ENTRIES {
                return Ok((bytes, runs));
            }
            entries += 1;
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if entry.file_name().to_string_lossy().starts_with("run-") {
                    runs += 1;
                }
                pending.push(entry.path());
            } else if file_type.is_file() {
                bytes = bytes.saturating_add(entry.metadata().map(|meta| meta.len()).unwrap_or(0));
            }
        }
    }
    Ok((bytes, runs))
}

/// 清理旧工作区：每个 `doc-*` 下保留最新 `keep_per_doc` 个运行目录，其余只删
/// 「mtime 超过 `min_age`」且「已接受（写进正文过）」的轮次；`run.json` 读不出的
/// 孤儿目录额外要求超过 1 小时。Rust 正持着 `rust-owner.lock` 的目录不删，
/// Running / Interrupted / Failed / Parked / Delivered 也不删——它们是「接着写」的依据。
///
/// 返回（删除的运行目录数, 释放的字节数）。
pub(crate) fn prune_workspaces(
    keep_per_doc: usize,
    min_age: std::time::Duration,
) -> Result<(usize, u64), String> {
    let base = base_dir()?;
    let now = std::time::SystemTime::now();
    let mut pruned = 0usize;
    let mut freed = 0u64;
    let reader = std::fs::read_dir(&base)
        .map_err(|error| format!("读不了 Pi 工作区目录（{}）：{error}", base.display()))?;
    for entry in reader.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || !entry.file_name().to_string_lossy().starts_with("doc-") {
            continue;
        }
        let Ok(runs) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        // 目录名里带时间戳（`run-YYYYMMDD-HHMMSS-xxxxxxxx`），按名字降序即由新到旧。
        let mut run_dirs: Vec<(String, PathBuf)> = Vec::new();
        for run in runs.flatten() {
            let name = run.file_name().to_string_lossy().into_owned();
            if !name.starts_with("run-") {
                continue;
            }
            let Ok(file_type) = run.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                run_dirs.push((name, run.path()));
            }
        }
        run_dirs.sort_by(|left, right| right.0.cmp(&left.0));
        for (index, (_name, run_dir)) in run_dirs.into_iter().enumerate() {
            if index < keep_per_doc {
                continue;
            }
            let Ok(metadata) = std::fs::metadata(&run_dir) else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let age = now.duration_since(modified).unwrap_or_default();
            if age < min_age {
                continue;
            }
            match super::recovery::PiRunRecord::read_from(&run_dir) {
                Ok(record) if record.status == super::recovery::PiRunStatus::Accepted => {}
                // 有记录但不是「已接受」：这是接着写的依据，不删。
                Ok(_) => continue,
                // 只有确认 run.json 不存在才算孤儿。损坏、权限错误等不等于没有记录，
                // 里面仍可能有尚未取回的成品，不能自动删除。
                Err(_) => match std::fs::symlink_metadata(run_dir.join("run.json")) {
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            && age >= ORPHAN_MIN_AGE => {}
                    _ => continue,
                },
            }
            // 占用检查放在最后：它会在没有锁文件时创建一个，只对真准备删的目录做。
            let Some(_cleanup_owner) = lock_for_cleanup(&run_dir) else {
                continue;
            };
            let bytes = dir_bytes(&run_dir);
            if std::fs::remove_dir_all(&run_dir).is_ok() {
                pruned += 1;
                freed = freed.saturating_add(bytes);
            }
        }
    }
    Ok((pruned, freed))
}

/// 取得清理所有权，直到删除完成才释放，防止检查与删除之间另一窗口开始运行。
///
/// Rust 在准备、等 host、写终态的整个运行期间都持着 `rust-owner.lock` 的
/// `try_lock`；拿不到锁就跳过该目录。锁文件不存在时创建它，
/// 这样崩溃残留的空锁文件与从未开跑的目录都不影响清理。
///
/// 不用 `host-owner.sqlite` 判断：host 退出后它留在目录里是常态。
fn lock_for_cleanup(run_dir: &Path) -> Option<std::fs::File> {
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(run_dir.join("rust-owner.lock"))
    else {
        // 打不开锁文件（权限、路径异常）时拿不准，保守跳过。
        return None;
    };
    lock.try_lock().ok()?;
    Some(lock)
}

/// 目录下文件总字节数（不跟随符号链接）。只用于回报释放空间，达到扫描上限即停。
fn dir_bytes(root: &Path) -> u64 {
    let mut bytes = 0u64;
    let mut entries = 0usize;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(reader) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in reader.flatten() {
            if entries >= MAX_SCAN_ENTRIES {
                return bytes;
            }
            entries += 1;
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                bytes = bytes.saturating_add(entry.metadata().map(|meta| meta.len()).unwrap_or(0));
            }
        }
    }
    bytes
}

impl PiWorkspace {
    /// 未采纳候选稿仅供继续修改，不覆盖正式快照，也不登记为事实来源。
    pub(crate) fn write_candidate(&self, text: &str) -> Result<(), String> {
        write_text(&self.root.join("input/candidate.md"), text)?;
        let request_path = self.root.join("input/request.md");
        let mut request = std::fs::read_to_string(&request_path)
            .map_err(|error| format!("读取本轮要求失败：{error}"))?;
        request.push_str("\n\n## 继续修改未采纳稿\n请读取 input/candidate.md，以此作为本轮修改基准。它不是事实来源；事实仍以 input/current.md、用户要求和事实材料为准。只改选区时，以正式快照中的选区及其外部原文为准。\n");
        write_text(&request_path, &request)
    }

    pub(crate) fn prepare(
        base: &Path,
        doc_key: u64,
        current: &str,
        request: &str,
        materials: &[PiMaterialInput],
        catalog: &Catalog,
        selected: Option<&str>,
    ) -> Result<(Self, Vec<PiMaterialMeta>), String> {
        let skills = SkillSnapshot::build(catalog, selected)?;
        Self::prepare_with_snapshot(base, doc_key, current, request, materials, &skills)
    }

    pub(crate) fn prepare_with_snapshot(
        base: &Path,
        doc_key: u64,
        current: &str,
        request: &str,
        materials: &[PiMaterialInput],
        skills: &SkillSnapshot,
    ) -> Result<(Self, Vec<PiMaterialMeta>), String> {
        let doc_dir = base.join(format!("doc-{doc_key}"));
        std::fs::create_dir_all(&doc_dir)
            .map_err(|error| format!("建不了工作区子目录（{}）：{error}", doc_dir.display()))?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        // 目录名带 UUID：两个窗口的 doc_key / seq 可能相同，不能靠重名探测区分。
        let name = format!("run-{stamp}-{}", short_uuid());
        let run_dir = doc_dir.join(&name);
        let root = run_dir.join("workspace");
        let state_dir = run_dir.join("pi");
        // 建目录与写文件整段交给 `build_run`：材料 / 技能 / 要求写失败时下面统一删掉
        // 这次刚建的运行目录，不留半成品孤儿目录。只删 `run_dir` 这一层，不碰 doc-*
        // 与其它轮次的目录。
        let prepared = build_run(&root, &state_dir, current, request, materials, skills);
        let metas = match prepared {
            Ok(metas) => metas,
            Err(error) => {
                // 清理失败只是留下目录，不能盖掉真正的准备错误。
                let _ = std::fs::remove_dir_all(&run_dir);
                return Err(error);
            }
        };
        let label = format!("pi/workspaces/doc-{doc_key}/{name}");
        Ok((
            Self {
                root,
                state: state_dir.join("session.sqlite"),
                run: run_dir,
                label,
            },
            metas,
        ))
    }

    /// 恢复时重新打开原工作区：路径由 `recovery::resolve_workspace` 校验过。
    pub(crate) fn reopen(root: PathBuf, label: String) -> Result<Self, String> {
        let run = root.parent().ok_or("Pi 工作区没有运行目录")?.to_path_buf();
        let state = run.join("pi").join("session.sqlite");
        Ok(Self {
            root,
            state,
            run,
            label,
        })
    }

    /// 工作区里的材料元数据（恢复时按 `index.json` 读回）。
    pub(crate) fn read_materials(&self) -> Result<Vec<PiMaterialMeta>, String> {
        let path = self.root.join("input/materials/index.json");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = super::recovery::read_bounded(&path, 256 * 1024)?;
        let metas: Vec<PiMaterialMeta> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Pi 材料索引读不出来：{error}"))?;
        Ok(metas
            .into_iter()
            .take(super::materials::MAX_MATERIALS)
            .collect())
    }

    /// 模型实际读到的要求文件（要求原文 + 技能索引 + 材料索引）。
    pub(crate) fn read_request(&self) -> Result<String, String> {
        let bytes = super::recovery::read_bounded(
            &self.root.join("input/request.md"),
            super::recovery::MAX_SNAPSHOT_BYTES,
        )?;
        String::from_utf8(bytes).map_err(|_| "工作区里的要求文件不是 UTF-8".to_string())
    }

    /// 按元数据读出材料全文；恢复新一轮工作区时用，读不到就明确失败。
    pub(crate) fn read_material_texts(&self) -> Result<Vec<PiMaterialInput>, String> {
        let mut inputs = Vec::new();
        for meta in self.read_materials()? {
            let path = self.root.join(&meta.file);
            let bytes =
                super::recovery::read_bounded(&path, super::materials::MAX_MATERIAL_BYTES as u64)?;
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("材料《{}》不是合法 UTF-8。", meta.title))?;
            inputs.push(PiMaterialInput {
                title: meta.title,
                source: meta.source,
                role: meta.role,
                text,
            });
        }
        Ok(inputs)
    }
}

/// 按同一份内存快照把一个运行目录的内容落盘；失败时调用方负责删掉 `run_dir`。
///
/// 建目录与写文件从这里开始：`input/current.md`、技能快照与索引、材料、要求文件、
/// 随包技能目录、`pi/` 状态目录。
fn build_run(
    root: &Path,
    state_dir: &Path,
    current: &str,
    request: &str,
    materials: &[PiMaterialInput],
    skills: &SkillSnapshot,
) -> Result<Vec<PiMaterialMeta>, String> {
    for sub in ["input", "work", "resources"] {
        std::fs::create_dir_all(root.join(sub)).map_err(|error| {
            format!("建不了工作区目录（{}）：{error}", root.join(sub).display())
        })?;
    }
    write_text(&root.join("input").join("current.md"), current)?;
    // 先按同一份内存快照落盘技能与索引，再用同一份快照写要求文件。
    skills.write_to(root)?;
    let mut request_file = vec![request.trim().to_string()];
    request_file.push(String::new());
    request_file.push(skills.request_block());
    request_file.push(String::new());
    request_file.push("## 材料".into());
    let metas = materials::write_all(root, materials)?;
    request_file.push(materials::prompt_block(&metas));
    write_text(
        &root.join("input").join("request.md"),
        &request_file.join("\n"),
    )?;
    crate::skill_pack::write_directory(&root.join("skills"))
        .map_err(|error| format!("写不出随包技能：{error:#}"))?;
    std::fs::create_dir_all(state_dir)
        .map_err(|error| format!("建不了 Pi 状态目录（{}）：{error}", state_dir.display()))?;
    Ok(metas)
}

/// 目录名里用的短 UUID（只要唯一，不做密码学用途）。
fn short_uuid() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect()
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    crate::storage::atomic_write(path, |file| {
        file.write_all(text.as_bytes())
            .context("写工作区输入文件失败")
    })
    .map_err(|error| format!("写工作区输入文件失败（{}）：{error:#}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path as FsPath;

    /// 把测试线程的配置目录指到临时目录，测试结束自动恢复。
    struct ConfigDirReset;
    impl Drop for ConfigDirReset {
        fn drop(&mut self) {
            crate::storage::set_test_config_dir(None);
        }
    }

    fn isolated_config_dir() -> (tempfile::TempDir, ConfigDirReset) {
        let config = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(config.path().to_path_buf()));
        (config, ConfigDirReset)
    }

    /// 造一个运行目录：工作区、锁文件与一份指定状态的运行记录都齐。
    fn fake_run(
        doc_dir: &FsPath,
        name: &str,
        status: super::super::recovery::PiRunStatus,
    ) -> PathBuf {
        let run_dir = doc_dir.join(name);
        std::fs::create_dir_all(run_dir.join("workspace")).unwrap();
        std::fs::write(run_dir.join("workspace/payload.md"), b"payload").unwrap();
        // 真实的运行目录里 Rust 侧一直持有这把锁；解锁状态说明这一轮已经结束。
        std::fs::write(run_dir.join("rust-owner.lock"), b"").unwrap();
        let workspace = PiWorkspace {
            root: run_dir.join("workspace"),
            state: run_dir.join("pi/session.sqlite"),
            run: run_dir.clone(),
            label: format!("pi/workspaces/doc-test/{name}"),
        };
        let mut record = super::super::recovery::PiRunRecord::start(
            "run-test".into(),
            "request-test".into(),
            &workspace,
            "正文",
            "正文",
            "正文",
            "要求",
            None,
            "要求",
            "通知",
            "2026-11-04",
            None,
            None,
            Vec::new(),
            "测试服务商",
            "测试模型",
        );
        record.status = status;
        record.write(&workspace).unwrap();
        run_dir
    }

    /// 准备失败（材料超限）不能留下半成品运行目录；doc-* 仍保留。
    #[test]
    fn a_failed_preparation_leaves_no_run_directory() {
        let base = tempfile::tempdir().unwrap();
        let skills = super::super::skills::SkillSnapshot::build_builtin(None).unwrap();
        let huge = "字".repeat(super::super::materials::MAX_MATERIAL_BYTES + 1);
        let materials = vec![PiMaterialInput {
            title: "超大材料".into(),
            source: "测试".into(),
            role: materials::MaterialRole::Fact,
            text: huge,
        }];
        let error =
            PiWorkspace::prepare_with_snapshot(base.path(), 7, "正文", "要求", &materials, &skills)
                .unwrap_err();
        assert!(error.contains("超过单份上限"), "{error}");
        let doc_dir = base.path().join("doc-7");
        assert!(doc_dir.is_dir(), "doc-* 里可能有别的轮次，不能删");
        assert_eq!(
            std::fs::read_dir(&doc_dir).unwrap().count(),
            0,
            "失败后不能留下 run 目录"
        );
    }

    /// 清理：删掉旧的已接受轮次，保留最新 N 个、Running 与正被本进程锁住的目录；
    /// `host-owner.sqlite` 留着不影响删除（host 退出后它是常态）。
    #[test]
    fn prune_removes_old_accepted_runs_and_keeps_what_is_needed() {
        let (_config, _reset) = isolated_config_dir();
        let base = crate::storage::config_dir().unwrap().join("pi/workspaces");
        let doc = base.join("doc-1");
        std::fs::create_dir_all(&doc).unwrap();
        use super::super::recovery::PiRunStatus;
        let old_a = fake_run(&doc, "run-20260101-000000-aaaaaaaa", PiRunStatus::Accepted);
        let old_b = fake_run(&doc, "run-20260102-000000-bbbbbbbb", PiRunStatus::Accepted);
        let running = fake_run(&doc, "run-20260103-000000-cccccccc", PiRunStatus::Running);
        let hosted = fake_run(&doc, "run-20260104-000000-dddddddd", PiRunStatus::Accepted);
        // host 退出后 host-owner.sqlite 还留在目录里，是常态，不能当成「正在跑」。
        std::fs::write(hosted.join("host-owner.sqlite"), b"").unwrap();
        let newest = fake_run(&doc, "run-20260105-000000-eeeeeeee", PiRunStatus::Accepted);
        // 本进程持着 rust-owner.lock：这一轮正在跑，不能删（它也比最新一轮旧）。
        let locked = fake_run(&doc, "run-20260106-000000-ffffffff", PiRunStatus::Accepted);
        let held_lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(locked.join("rust-owner.lock"))
            .unwrap();
        held_lock.try_lock().expect("测试进程应先拿到运行所有权锁");
        let keep_newest = fake_run(&doc, "run-20260107-000000-gggggggg", PiRunStatus::Accepted);
        // 第二个稿件：验证「保留最新 N 个」。
        let other_doc = base.join("doc-2");
        std::fs::create_dir_all(&other_doc).unwrap();
        let other_old = fake_run(
            &other_doc,
            "run-20260101-000000-aaaaaaaa",
            PiRunStatus::Accepted,
        );
        let other_older = fake_run(
            &other_doc,
            "run-20260102-000000-bbbbbbbb",
            PiRunStatus::Accepted,
        );
        let other_newest = fake_run(
            &other_doc,
            "run-20260103-000000-cccccccc",
            PiRunStatus::Accepted,
        );

        // keep_per_doc = 1：每个稿件只保留最新一轮（0107 / 0103）。
        let (count, freed) = prune_workspaces(1, std::time::Duration::ZERO).unwrap();
        assert_eq!(count, 6, "doc-1 删 4 个、doc-2 删 2 个");
        assert!(freed >= 42, "要回报释放的字节数：{freed}");
        assert!(
            !hosted.exists(),
            "host-owner.sqlite 留着不表示正在跑，应删除"
        );
        assert!(!newest.exists(), "不在最新 N 个里的已接受轮次应删除");
        assert!(!old_a.exists(), "旧的已接受轮次应删除");
        assert!(!old_b.exists(), "旧的已接受轮次应删除");
        assert!(running.exists(), "Running 是接着写的依据，不能删");
        assert!(locked.exists(), "本进程持着锁的目录不能删");
        assert!(keep_newest.exists(), "每个稿件保留最新一轮");
        assert!(!other_old.exists());
        assert!(!other_older.exists());
        assert!(other_newest.exists(), "每个稿件保留最新一轮");

        // min_age 没到：即使不再保留也先不删。
        let (count, freed) = prune_workspaces(0, std::time::Duration::from_secs(3600)).unwrap();
        assert_eq!((count, freed), (0, 0));
        assert!(other_newest.exists());
        drop(held_lock);
    }

    #[test]
    fn pruning_preserves_old_corrupt_records_and_removes_only_real_orphans() {
        let (_config, _reset) = isolated_config_dir();
        let doc = crate::storage::config_dir()
            .unwrap()
            .join("pi/workspaces/doc-3");
        let corrupt = doc.join("run-corrupt");
        let orphan = doc.join("run-orphan");
        std::fs::create_dir_all(&corrupt).unwrap();
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(corrupt.join("run.json"), "{bad json").unwrap();
        std::fs::write(corrupt.join("final.md"), "尚未取回的成品").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 3600);
        for dir in [&corrupt, &orphan] {
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options
                    .custom_flags(0x0200_0000) // FILE_FLAG_BACKUP_SEMANTICS：打开目录。
                    .access_mode(0x0100); // FILE_WRITE_ATTRIBUTES：只修改目录时间。
            }
            options.open(dir).unwrap().set_modified(old).unwrap();
        }
        let (count, _) = prune_workspaces(0, std::time::Duration::ZERO).unwrap();
        assert_eq!(count, 1);
        assert!(corrupt.join("final.md").exists());
        assert!(!orphan.exists());
    }

    /// 孤儿目录（run.json 不存在）额外要求超过 1 小时，刚建的不动。
    #[test]
    fn a_fresh_orphan_directory_is_not_pruned() {
        let (_config, _reset) = isolated_config_dir();
        let doc = crate::storage::config_dir()
            .unwrap()
            .join("pi/workspaces/doc-2");
        std::fs::create_dir_all(&doc).unwrap();
        let orphan = doc.join("run-20260101-000000-aaaaaaaa");
        std::fs::create_dir_all(orphan.join("workspace")).unwrap();
        std::fs::write(orphan.join("rust-owner.lock"), b"").unwrap();
        std::fs::write(orphan.join("broken.json"), b"{").unwrap();
        let (count, _) = prune_workspaces(0, std::time::Duration::ZERO).unwrap();
        assert_eq!(count, 0);
        assert!(orphan.exists(), "读不出记录但还很新：可能正在准备，不能删");
    }

    #[test]
    fn workspace_usage_counts_files_and_run_directories() {
        let (_config, _reset) = isolated_config_dir();
        let doc = crate::storage::config_dir()
            .unwrap()
            .join("pi/workspaces/doc-1");
        std::fs::create_dir_all(&doc).unwrap();
        fake_run(
            &doc,
            "run-20260101-000000-aaaaaaaa",
            super::super::recovery::PiRunStatus::Running,
        );
        fake_run(
            &doc,
            "run-20260102-000000-bbbbbbbb",
            super::super::recovery::PiRunStatus::Delivered,
        );
        let (bytes, runs) = workspace_usage().unwrap();
        assert_eq!(runs, 2);
        assert!(bytes >= 14, "应统计到文件字节：{bytes}");
    }

    #[test]
    fn candidate_is_delivered_separately_from_the_formal_snapshot() {
        let config = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(config.path().to_path_buf()));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let _reset = Reset;
        let dir = tempfile::tempdir().unwrap();
        let catalog = super::super::skills::load_catalog();
        let (workspace, _) =
            PiWorkspace::prepare(dir.path(), 1, "正式正文", "继续改", &[], &catalog, None).unwrap();
        workspace.write_candidate("尚未采纳的修改").unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.root.join("input/current.md")).unwrap(),
            "正式正文"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.root.join("input/candidate.md")).unwrap(),
            "尚未采纳的修改"
        );
        let request = std::fs::read_to_string(workspace.root.join("input/request.md")).unwrap();
        assert!(request.contains("不是事实来源"));
    }

    #[test]
    fn a_new_task_freezes_the_enabled_skill_catalog_into_its_workspace() {
        let config = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(config.path().to_path_buf()));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let _reset = Reset;
        use super::super::skills::store as skills;
        skills::create("custom-skill", Some("自定义技能")).unwrap();
        skills::save_text("custom-skill", "references/依据.md", "辅助依据").unwrap();
        skills::set_enabled("gongwen-draft", false).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let catalog = super::super::skills::load_catalog();
        let (workspace, metas) = PiWorkspace::prepare(
            dir.path(),
            1,
            "正式正文",
            "按自定义技能写",
            &[],
            &catalog,
            Some("custom-skill"),
        )
        .unwrap();
        let request = workspace.read_request().unwrap();
        assert!(request.contains("custom-skill"), "{request}");
        assert!(request.contains("自定义技能"), "{request}");
        assert!(
            !request.contains("gongwen-draft"),
            "停用技能不进本轮：{request}"
        );
        assert_eq!(
            std::fs::read_to_string(
                workspace
                    .root
                    .join("skills/pi-skills/custom-skill/references/依据.md")
            )
            .unwrap(),
            "辅助依据"
        );
        let index = std::fs::read_to_string(workspace.root.join("input/skills.json")).unwrap();
        assert!(index.contains("custom-skill"), "{index}");
        assert!(index.contains("sha256"), "{index}");
        assert!(!index.contains("gongwen-draft"), "{index}");
        // 全局技能改动不会改已经准备好的工作区。
        skills::save_text(
            "custom-skill",
            "SKILL.md",
            "---\nname: custom-skill\ndescription: 新版本\n---\n新版正文",
        )
        .unwrap();
        skills::set_enabled("custom-skill", false).unwrap();
        let frozen = std::fs::read_to_string(
            workspace
                .root
                .join("skills/pi-skills/custom-skill/SKILL.md"),
        )
        .unwrap();
        assert!(frozen.contains("一句话写清"), "{frozen}");
        std::fs::write(&workspace.state, b"sqlite").unwrap();
        let record = super::super::PiRunRecord::start(
            "run-frozen".into(),
            "request-frozen".into(),
            &workspace,
            "正式正文",
            "正式正文",
            "正式正文",
            &workspace.read_request().unwrap(),
            None,
            "按自定义技能写",
            "通知",
            "2026-11-02",
            Some("custom-skill"),
            None,
            metas,
            "测试服务商",
            "测试模型",
        );
        record.write(&workspace).unwrap();
        // 恢复只认原快照，即使全局技能已经停用。
        super::super::recovery::check_workspace_files(&workspace, &record).unwrap();
    }
}
