//! 独立密码保护内容和目录头；仅接受本程序新版 7z 备份。
use super::*;
use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
use sevenz_rust2::{ArchiveEntry, ArchiveReader, ArchiveWriter, Password};

pub(super) type Reader = ArchiveReader<fs::File>;
pub(super) const FORMAT_FILE: &str = "backup-format.json";
pub(super) const FORMAT: &[u8] =
    br#"{"format":"gongwen-backup","version":1,"encrypted_workspace":true}"#;

pub(super) fn writer(file: fs::File, password: &str) -> Result<ArchiveWriter<fs::File>> {
    crate::password_policy::validate_export(password)?;
    let mut writer = ArchiveWriter::new(file)?;
    writer.set_encrypt_header(true);
    methods(&mut writer, password);
    writer.push_archive_entry(ArchiveEntry::new_file(FORMAT_FILE), Some(FORMAT))?;
    Ok(writer)
}

pub(super) fn methods(writer: &mut ArchiveWriter<fs::File>, password: &str) {
    let mut aes = AesEncoderOptions::new(Password::from(password));
    aes.num_cycles_power = 19;
    writer.set_content_methods(vec![aes.into(), Lzma2Options::from_level(1).into()]);
}

pub(super) fn open(path: &Path, password: &str) -> Result<Reader> {
    anyhow::ensure!(!password.is_empty(), "请输入备份密码");
    let file = fs::File::open(path)?;
    anyhow::ensure!(
        file.metadata()?.len() <= BACKUP_LIMITS.total_bytes + 64 * 1024 * 1024,
        "备份文件过大"
    );
    let mut reader =
        ArchiveReader::new_with_memory_limit(file, Password::from(password), 64 * 1024)
            .context("备份密码错误或文件损坏；只支持新版加密 7z 备份")?;
    reader.set_thread_count(1);
    anyhow::ensure!(
        reader.archive().files.len() <= BACKUP_LIMITS.entries,
        "备份条目太多"
    );
    let mut budget = ArchiveBudget::default();
    let mut names = std::collections::HashSet::new();
    for entry in &reader.archive().files {
        let rel = safe_relative_path(entry.name())?;
        anyhow::ensure!(names.insert(rel), "备份存在重复路径");
        anyhow::ensure!(
            !entry.is_directory() && !entry.is_anti_item(),
            "备份包含非普通文件条目"
        );
        // 本程序只生成普通文件；拒绝外部制作的链接/设备属性。
        anyhow::ensure!(entry.windows_attributes == 0, "备份包含不支持的文件属性");
        budget.account(entry.name(), entry.size(), BudgetPhase::Restore)?;
    }
    anyhow::ensure!(
        reader
            .archive()
            .files
            .iter()
            .any(|e| e.name() == FORMAT_FILE && e.size() <= 128),
        "备份格式标识缺失或过大"
    );
    let format = reader
        .read_file(FORMAT_FILE)
        .context("备份缺少新版格式标识")?;
    anyhow::ensure!(format == FORMAT, "不支持的备份格式");
    Ok(reader)
}

pub(super) fn io_error(error: anyhow::Error) -> sevenz_rust2::Error {
    std::io::Error::other(format!("{error:#}")).into()
}
