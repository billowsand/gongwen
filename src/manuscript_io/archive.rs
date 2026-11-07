//! 稿件归档容器：新写 7z（内容 + 文件名加密），按魔数兼容读取旧 ZIP。
//! 全程从内存字节写加密流，不把正文或资源解压到临时目录。

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
use sevenz_rust2::{Archive, ArchiveEntry, ArchiveWriter, BlockDecoder, EncoderMethod, Password};

const SEVEN_Z_SIGNATURE: &[u8; 6] = b"7z\xbc\xaf\x27\x1c";
const MEMORY_LIMIT_KB: usize = 64 * 1024;
const MAX_ENTRIES: usize = 100_000;

// 7-Zip 常用的 2^19 轮 SHA-256 派生；库默认只有 2^8，必须显式提高。
fn aes_options(password: &Password) -> AesEncoderOptions {
    let mut options = AesEncoderOptions::new(password.clone());
    options.num_cycles_power = 19;
    options
}

pub(super) struct PackageWriter {
    writer: ArchiveWriter<File>,
    password: Password,
}

impl PackageWriter {
    pub(super) fn new(file: File, password: &str) -> Result<Self> {
        ensure!(!password.is_empty(), "请输入稿件包密码");
        let password = Password::from(password);
        let mut writer = ArchiveWriter::new(file)?;
        writer.set_encrypt_header(true);
        // 即使没有成功写入 PDF，也不能在 finish 时生成未加密的头部。
        writer.set_content_methods(vec![aes_options(&password).into()]);
        Ok(Self { writer, password })
    }

    pub(super) fn write(&mut self, name: &str, bytes: &[u8], compress: bool) -> Result<()> {
        // 每个条目独立成块、重新生成 salt / IV；按需读取附件时无需解压整包。
        let mut methods = vec![aes_options(&self.password).into()];
        methods.push(if compress {
            Lzma2Options::from_level(1).into()
        } else {
            EncoderMethod::COPY.into()
        });
        self.writer.set_content_methods(methods);
        self.writer
            .push_archive_entry(ArchiveEntry::new_file(name), Some(bytes))?;
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<File> {
        // 头部加密复用当前内容方法的 AES 参数；换一套新 salt/IV 再 finish，
        // 避免头部与最后一个条目共用同一组 CBC IV。
        self.writer
            .set_content_methods(vec![aes_options(&self.password).into()]);
        Ok(self.writer.finish()?)
    }
}

/// 旧 ZIP 稿件包先看 EOCD 里的条目总数：ZIP64 标记（0xFFFF）意味着条目数
/// 由 ZIP64 记录决定，可能在 `ZipArchive::new` 解析中央目录时先吃掉大量内存。
/// 正常稿件包不会有几万个条目，遇到标记直接拒绝。
fn ensure_zip_entry_budget(file: &mut File) -> Result<()> {
    let len = file.metadata()?.len();
    if len < 22 {
        bail!("ZIP 稿件包结构不完整");
    }
    let tail_len = len.min(64 * 1024);
    file.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = Vec::with_capacity(tail_len as usize);
    file.take(tail_len).read_to_end(&mut tail)?;
    file.seek(SeekFrom::Start(0))?;
    let Some(pos) = tail.windows(4).rposition(|w| w == b"PK\x05\x06") else {
        bail!("ZIP 稿件包缺少中央目录结尾");
    };
    let record = &tail[pos..];
    ensure!(record.len() >= 22, "ZIP 稿件包结构不完整");
    let entries = u16::from_le_bytes([record[10], record[11]]) as usize;
    ensure!(
        entries < u16::MAX as usize,
        "稿件包条目过多（ZIP64 大包不支持，请改用新版 7z 包）"
    );
    ensure!(entries <= MAX_ENTRIES, "稿件包条目过多");
    Ok(())
}

pub(super) enum PackageReader {
    Zip(zip::ZipArchive<File>),
    SevenZ {
        file: File,
        archive: Box<Archive>,
        password: Password,
    },
}

impl PackageReader {
    pub(super) fn open(path: &Path, password: &str) -> Result<Self> {
        let mut file =
            File::open(path).with_context(|| format!("无法打开导入文件 {}", path.display()))?;
        ensure!(
            file.metadata()?.len() <= crate::file_limits::MAX_PACKAGE_BYTES,
            "稿件包超过大小上限"
        );
        let mut magic = [0; 6];
        file.read_exact(&mut magic)
            .context("文件不是有效的稿件包")?;
        file.seek(SeekFrom::Start(0))?;
        if &magic == SEVEN_Z_SIGNATURE {
            let password = Password::from(password);
            let archive = Archive::read_with_memory_limit(&mut file, &password, MEMORY_LIMIT_KB)
                .context("密码错误或 7z 稿件包损坏，无法读取文件目录")?;
            ensure!(archive.files.len() <= MAX_ENTRIES, "稿件包条目过多");
            let mut names = HashSet::new();
            let mut total = 0u64;
            for entry in &archive.files {
                ensure!(names.insert(entry.name()), "稿件包存在重名条目");
                total = total.checked_add(entry.size()).context("稿件包大小溢出")?;
                ensure!(
                    total <= crate::file_limits::MAX_PACKAGE_BYTES,
                    "稿件包解压后超过大小上限"
                );
            }
            Ok(Self::SevenZ {
                file,
                archive: Box::new(archive),
                password,
            })
        } else if magic.starts_with(b"PK\x03\x04") || magic.starts_with(b"PK\x05\x06") {
            ensure_zip_entry_budget(&mut file)?;
            let archive = zip::ZipArchive::new(file).context("文件不是有效的 ZIP 稿件包")?;
            ensure!(archive.len() <= MAX_ENTRIES, "稿件包条目过多");
            Ok(Self::Zip(archive))
        } else {
            bail!("文件不是有效的公文助手稿件包（支持 7z 与旧 ZIP）")
        }
    }

    pub(super) fn file_names(&self) -> Vec<String> {
        match self {
            Self::Zip(archive) => archive.file_names().map(str::to_string).collect(),
            Self::SevenZ { archive, .. } => {
                archive.files.iter().map(|e| e.name().to_string()).collect()
            }
        }
    }

    pub(super) fn contains(&self, name: &str) -> bool {
        match self {
            Self::Zip(archive) => archive.index_for_name(name).is_some(),
            Self::SevenZ { archive, .. } => archive.files.iter().any(|e| e.name() == name),
        }
    }

    pub(super) fn read(&mut self, name: &str, password: &str, max: u64) -> Result<Vec<u8>> {
        match self {
            Self::Zip(archive) => {
                let entry = archive
                    .by_name_decrypt(name, password.as_bytes())
                    .map_err(super::map_zip_password_error)?;
                ensure!(entry.size() <= max, "资源文件过大：{name}");
                read_bounded(entry, max).with_context(|| format!("无法读取包内资源：{name}"))
            }
            Self::SevenZ {
                file,
                archive,
                password,
            } => {
                let index = archive
                    .files
                    .iter()
                    .position(|e| e.name() == name)
                    .with_context(|| format!("稿件包缺少资源：{name}"))?;
                let entry = &archive.files[index];
                ensure!(
                    !entry.is_directory() && !entry.is_anti_item(),
                    "资源不是普通文件：{name}"
                );
                ensure!(entry.size() <= max, "资源文件过大：{name}");
                let Some(block) = archive.stream_map.file_block_index[index] else {
                    ensure!(entry.size() == 0, "资源缺少数据块：{name}");
                    return Ok(Vec::new());
                };
                let mut output = None;
                // 只解码目标块，读取保持有界；固实外部包的前置条目由库流式校验并跳过。
                BlockDecoder::new_with_memory_limit(
                    1,
                    MEMORY_LIMIT_KB,
                    block,
                    archive,
                    password,
                    file,
                )
                .for_each_entries(&mut |entry, reader| {
                    if entry.name() != name {
                        return Ok(true);
                    }
                    output = Some(read_bounded(reader, max)?);
                    Ok(false)
                })
                .with_context(|| format!("密码错误或资源损坏：{name}"))?;
                output.with_context(|| format!("稿件包缺少资源：{name}"))
            }
        }
    }
}

fn read_bounded(reader: impl Read, max: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(std::io::Error::other("资源解压后超过大小上限"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "Jade!River7Cloud";
    const NAME: &str = "pdf/关于人事安排的通知.pdf";
    const CONTENT: &[u8] = b"confidential document body for archive tests";

    /// 旧 ZIP 包的 EOCD 声明 ZIP64 条目数（0xFFFF）时必须先拒绝，
    /// 不能让 `ZipArchive::new` 先解析可能巨大的中央目录。
    #[test]
    fn zip64_packages_are_rejected_before_parsing_the_central_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.zip");
        let mut eocd = vec![b'P', b'K', 5, 6];
        eocd.extend_from_slice(&[0; 6]);
        eocd.extend_from_slice(&u16::MAX.to_le_bytes()); // total entries
        eocd.extend_from_slice(&[0; 10]); // 中央目录大小/偏移 + 注释长度
        std::fs::write(&path, &eocd).unwrap();
        let mut file = std::fs::File::open(&path).unwrap();
        let error = ensure_zip_entry_budget(&mut file).unwrap_err();
        assert!(format!("{error:#}").contains("条目过多"));
        // 正常的 1 条记录可以通过。
        let mut ok = vec![b'P', b'K', 5, 6];
        ok.extend_from_slice(&[0; 6]);
        ok.extend_from_slice(&1u16.to_le_bytes());
        ok.extend_from_slice(&[0; 10]);
        std::fs::write(&path, &ok).unwrap();
        let mut file = std::fs::File::open(&path).unwrap();
        ensure_zip_entry_budget(&mut file).unwrap();
    }

    fn write_sample(path: &Path) {
        let mut writer = PackageWriter::new(File::create(path).unwrap(), PASSWORD).unwrap();
        writer.write(NAME, CONTENT, false).unwrap();
        writer.write("empty.txt", b"", true).unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn filenames_and_content_require_password_regardless_of_extension() {
        let dir = tempfile::tempdir().unwrap();
        // 故意用旧后缀，验证检测格式不靠文件名。
        let path = dir.path().join("renamed.zip");
        write_sample(&path);
        let raw = std::fs::read(&path).unwrap();
        let name_utf16 = NAME
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        for secret in [
            NAME.as_bytes(),
            name_utf16.as_slice(),
            CONTENT,
            PASSWORD.as_bytes(),
        ] {
            assert!(!raw.windows(secret.len()).any(|w| w == secret));
        }
        assert!(PackageReader::open(&path, "").is_err());
        assert!(PackageReader::open(&path, "wrong").is_err());
        let mut reader = PackageReader::open(&path, PASSWORD).unwrap();
        assert_eq!(reader.file_names(), vec![NAME, "empty.txt"]);
        assert_eq!(reader.read(NAME, PASSWORD, 100).unwrap(), CONTENT);
        assert!(reader.read("empty.txt", PASSWORD, 1).unwrap().is_empty());
        assert!(reader.read(NAME, PASSWORD, 3).is_err());
        assert!(reader.read("missing", PASSWORD, 100).is_err());
    }

    #[test]
    fn damaged_encrypted_content_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("damaged.gwx");
        write_sample(&path);
        let mut raw = std::fs::read(&path).unwrap();
        // 首个非固实数据块就在 32 字节签名头之后；目录和头部校验仍完好。
        raw[32] ^= 0x80;
        std::fs::write(&path, raw).unwrap();
        let mut reader = PackageReader::open(&path, PASSWORD).unwrap();
        assert!(reader.read(NAME, PASSWORD, 100).is_err());
    }

    #[test]
    fn truncated_and_unknown_packages_return_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.gwx");
        for bytes in [b"".as_slice(), SEVEN_Z_SIGNATURE, b"not an archive"] {
            std::fs::write(&path, bytes).unwrap();
            assert!(PackageReader::open(&path, PASSWORD).is_err());
        }
    }

    /// 可选互操作验收：用官方 7zz 验证产物，并读取其生成的固实、文件名加密包。
    #[test]
    #[ignore = "需 GONGWEN_7ZZ 指向官方 7zz 可执行文件"]
    fn official_7zip_interoperability() {
        use std::process::Command;
        let executable = std::env::var_os("GONGWEN_7ZZ").expect("设置 GONGWEN_7ZZ");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ours.gwx");
        write_sample(&path);
        let output = Command::new(&executable)
            .args(["t", "-pJade!River7Cloud"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let output = Command::new(&executable)
            .args(["l", "-pwrong"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains(NAME));
        let source = dir.path().join("input");
        std::fs::create_dir_all(source.join("pdf")).unwrap();
        std::fs::write(source.join(NAME), CONTENT).unwrap();
        std::fs::write(source.join("second.txt"), b"second").unwrap();
        let external = dir.path().join("official.gwx");
        let output = Command::new(executable)
            .current_dir(&source)
            .args(["a", "-t7z", "-mhe=on", "-pJade!River7Cloud"])
            .arg(&external)
            .arg(".")
            .output()
            .unwrap();
        assert!(output.status.success());
        let mut reader = PackageReader::open(&external, PASSWORD).unwrap();
        assert_eq!(reader.read(NAME, PASSWORD, 100).unwrap(), CONTENT);
        assert_eq!(reader.read("second.txt", PASSWORD, 100).unwrap(), b"second");
    }
}
