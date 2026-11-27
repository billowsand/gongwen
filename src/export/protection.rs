//! 对外交付加密：明文只在内存中生成，加密后才允许写目标文件。
//! 导出口令独立设置，不由登录口令或本地 DEK 派生。
pub(crate) mod office;

use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{Document, EncryptionState, EncryptionVersion, Permissions};
use rand::Rng;
use zeroize::Zeroizing;

pub(crate) const PASSWORD_FILE: &str = ".export-password";

/// 与口令一起密封保存；旧版只有口令文本，读取时按已启用加密兼容。
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct ExportPolicy {
    pub enabled: bool,
    pub password: String,
}
impl Default for ExportPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            password: String::new(),
        }
    }
}
impl Drop for ExportPolicy {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
    }
}
impl ExportPolicy {
    pub(crate) fn load() -> Result<Self> {
        let handle = crate::vault::key_handle().context("请先解锁应用")?;
        let path = crate::storage::slot_file_for(handle.slot(), PASSWORD_FILE)?;
        let raw = match crate::file_limits::read_bytes(&path, 4096, "导出设置") {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            crate::vault::is_sealed_file(&raw),
            "导出设置未加密，已拒绝读取"
        );
        let bytes = Zeroizing::new(crate::vault::open_path_bytes(
            &handle, &path, "config", &raw,
        )?);
        // 合法旧口令也可能以 { 开头，因此先按完整 JSON 解析，再兼容旧口令文本。
        if let Ok(policy) = serde_json::from_slice::<Self>(&bytes) {
            return Ok(policy);
        }
        let password = String::from_utf8(bytes.to_vec()).context("导出设置损坏")?;
        validate_password(&password)?;
        Ok(Self {
            enabled: true,
            password,
        })
    }
    pub(crate) fn save(&self, handle: &crate::vault::KeyHandle) -> Result<()> {
        let current = crate::vault::key_handle().context("当前已锁定，未保存导出设置")?;
        ensure!(current.same_unlock(handle), "工作区已变化，未保存导出设置");
        let path = crate::storage::slot_file_for(handle.slot(), PASSWORD_FILE)?;
        let bytes = Zeroizing::new(serde_json::to_vec(self)?);
        let sealed = crate::vault::seal_path_bytes(handle, &path, "config", &bytes)?;
        crate::storage::atomic_write(&path, |file| {
            file.write_all(&sealed)?;
            Ok(())
        })
    }
}

pub(crate) fn encryption_enabled() -> bool {
    crate::vault::key_handle().is_some()
}

/// 只有「从未启用应用锁」或「用户明确选过普通模式」的工作区才允许明文写盘。
/// 锁文件存在（Locked/Broken）而当前没有密钥，说明处在锁定或备份恢复的过渡，
/// 不能按普通工作区继续写明文。
fn plaintext_workspace() -> bool {
    #[cfg(test)]
    if !crate::storage::test_config_dir_configured() {
        // 单元测试默认没有配置目录：按普通工作区处理，避免读开发机真实配置。
        return true;
    }
    matches!(
        crate::vault::Vault::probe(),
        crate::vault::Probe::Absent | crate::vault::Probe::Skipped
    )
}

pub(crate) struct Protection {
    password: Zeroizing<String>,
    handle: crate::vault::KeyHandle,
}

impl Protection {
    pub(crate) fn current() -> Result<Option<Self>> {
        if !encryption_enabled() {
            ensure!(
                plaintext_workspace(),
                "工作区已锁定或加密设置异常，已取消导出"
            );
            return Ok(None);
        }
        Self::saved_password()
    }

    /// 预览旧加密成果、登录改密校验与导出开关无关。
    pub(crate) fn saved_password() -> Result<Option<Self>> {
        let Some(handle) = crate::vault::key_handle() else {
            return Ok(None);
        };
        let policy = ExportPolicy::load()?;
        if policy.password.is_empty() {
            anyhow::bail!("请先到「设置 → 导出」单独设置导出密码");
        }
        let password = Zeroizing::new(policy.password.clone());
        validate_password(&password)?;
        Ok(Some(Self { password, handle }))
    }

    fn check_slot(&self) -> Result<()> {
        ensure!(
            crate::storage::current_slot() == self.handle.slot(),
            "工作区已变化，已取消导出"
        );
        let current = crate::vault::key_handle().context("当前已锁定，已取消导出")?;
        ensure!(
            current.same_unlock(&self.handle),
            "加密状态已变化，已取消导出"
        );
        Ok(())
    }

    /// 先在内存加密全部产物，任一格式失败都不写出该批文件。
    pub(crate) fn persist(
        &self,
        files: HashMap<PathBuf, Vec<u8>>,
    ) -> Result<HashMap<PathBuf, PathBuf>> {
        let mut protected = Vec::new();
        let mut paths = HashMap::new();
        for (path, bytes) in files {
            let bytes = Zeroizing::new(bytes);
            let (target, encrypted) = self.encode(&path, &bytes)?;
            ensure!(
                !paths.values().any(|p| p == &target),
                "加密后的输出文件名冲突"
            );
            paths.insert(path, target.clone());
            protected.push((target, encrypted));
        }
        self.check_slot()?;
        for (path, bytes) in protected {
            self.check_slot()?;
            crate::storage::atomic_write(&path, |file| {
                file.write_all(&bytes)?;
                Ok(())
            })?;
        }
        Ok(paths)
    }

    pub(crate) fn persist_archive(
        &self,
        path: &Path,
        entries: Vec<(String, Vec<u8>)>,
    ) -> Result<PathBuf> {
        self.check_slot()?;
        ensure!(!entries.is_empty(), "没有可归档的内容");
        let mut names = std::collections::HashSet::new();
        let mut total = 0u64;
        for (name, bytes) in &entries {
            ensure!(
                !name.is_empty()
                    && !matches!(name.as_str(), "." | "..")
                    && !name.contains(['/', '\\', ':', '\0'])
                    && names.insert(name),
                "归档文件名无效或重复"
            );
            total = total
                .checked_add(bytes.len() as u64)
                .context("压缩包大小溢出")?;
        }
        ensure!(
            total <= crate::file_limits::MAX_PACKAGE_BYTES,
            "压缩包超过大小上限"
        );
        let bytes = encrypt_archive(entries, &self.password)?;
        self.check_slot()?;
        let path = path.with_extension("7z");
        crate::storage::atomic_write(&path, |file| {
            file.write_all(&bytes)?;
            Ok(())
        })?;
        Ok(path)
    }

    pub(crate) fn encode(&self, path: &Path, bytes: &[u8]) -> Result<(PathBuf, Vec<u8>)> {
        self.check_slot()?;
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "pdf" => Ok((path.into(), encrypt_pdf(bytes, &self.password)?)),
            "docx" | "xlsx" => Ok((path.into(), encrypt_office(bytes, &self.password)?)),
            "zip" => {
                // 输入是程序刚生成的 ZIP；逐项转入 7z，不先解压到磁盘。
                let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
                let mut entries = Vec::new();
                let mut total = 0u64;
                for i in 0..zip.len() {
                    let mut entry = zip.by_index(i)?;
                    ensure!(entry.enclosed_name().is_some(), "源码包路径不合法");
                    if entry.is_dir() {
                        continue;
                    }
                    total = total.checked_add(entry.size()).context("源码包大小溢出")?;
                    ensure!(
                        total <= crate::file_limits::MAX_PACKAGE_BYTES,
                        "源码包超过大小上限"
                    );
                    let mut body = Vec::new();
                    entry.read_to_end(&mut body)?;
                    entries.push((entry.name()?.into_owned(), body));
                }
                Ok((
                    path.with_extension("7z"),
                    encrypt_archive(entries, &self.password)?,
                ))
            }
            // 份号清单、单独的写作成果等以独立 7z 交付。
            "txt" | "md" => {
                let name = path
                    .file_name()
                    .context("缺少输出文件名")?
                    .to_string_lossy()
                    .into_owned();
                Ok((
                    path.with_extension("7z"),
                    encrypt_archive(vec![(name, bytes.to_vec())], &self.password)?,
                ))
            }
            _ => anyhow::bail!("该文件格式尚不支持加密导出，已取消本次导出"),
        }
    }
}

#[cfg(test)]
pub(crate) fn write_final(path: &Path, bytes: &[u8]) -> Result<PathBuf> {
    write_final_with(Protection::current()?.as_ref(), path, bytes)
}

pub(crate) fn write_final_with(
    protection: Option<&Protection>,
    path: &Path,
    bytes: &[u8],
) -> Result<PathBuf> {
    if let Some(protection) = protection {
        let paths = protection.persist(HashMap::from([(path.to_path_buf(), bytes.to_vec())]))?;
        return paths.get(path).cloned().context("加密输出未生成");
    }
    ensure!(
        crate::vault::key_handle().is_none(),
        "工作区加密状态已变化，已取消导出"
    );
    // 锁已启用但当前没有密钥（锁定/恢复过渡）：不能把成品按普通工作区写明文。
    ensure!(
        plaintext_workspace(),
        "工作区已锁定或加密设置异常，已取消明文导出"
    );
    crate::storage::atomic_write(path, |file| {
        file.write_all(bytes)?;
        Ok(())
    })?;
    Ok(path.to_path_buf())
}

fn encrypt_office(bytes: &[u8], password: &str) -> Result<Vec<u8>> {
    ensure!(bytes.starts_with(b"PK"), "Office 产物不是有效的 OOXML 包");
    let mut writer = ms_offcrypto_writer::Ecma376AgileWriter::create(
        &mut rand::rng(),
        password,
        Cursor::new(Vec::new()),
    )
    .context("无法初始化 Office 加密")?;
    writer.write_all(bytes)?;
    Ok(writer.into_inner().context("Office 加密失败")?.into_inner())
}

fn encrypt_pdf(bytes: &[u8], password: &str) -> Result<Vec<u8>> {
    let mut doc = Document::load_mem(bytes).context("无法读取待加密 PDF")?;
    ensure!(!doc.is_encrypted(), "PDF 已加密，无法重复加密");
    let key = Zeroizing::new(rand::random::<[u8; 32]>());
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let state = EncryptionState::try_from(EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: key.as_ref(),
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        // 两种口令均非空；不依赖复制/打印权限实现保密。
        owner_password: password,
        user_password: password,
        permissions: Permissions::all(),
    })?;
    doc.encrypt(&state)?;
    let mut output = Vec::new();
    doc.save_to(&mut output)?;
    Ok(output)
}

fn encrypt_archive(entries: Vec<(String, Vec<u8>)>, password: &str) -> Result<Vec<u8>> {
    use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, Password};
    // 每个条目单独生成 salt 与 IV：7z 的 AES 参数是按「当前内容方法」用的，
    // 整包共用一套参数会让同 key+IV 的 CBC 流泄露条目之间相同前缀的信息。
    // 头部加密也复用同一套方法，所以最后还要换一套新参数再 finish。
    fn methods(password: &str) -> Vec<sevenz_rust2::EncoderConfiguration> {
        let mut aes = AesEncoderOptions::new(Password::from(password));
        aes.num_cycles_power = 19;
        vec![aes.into(), Lzma2Options::from_level(1).into()]
    }
    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new()))?;
    writer.set_encrypt_header(true);
    writer.set_content_methods(methods(password));
    for (name, bytes) in entries {
        writer.set_content_methods(methods(password));
        writer.push_archive_entry(ArchiveEntry::new_file(&name), Some(bytes.as_slice()))?;
    }
    writer.set_content_methods(methods(password));
    Ok(writer.finish()?.into_inner())
}

/// 本程序打开加密成果时只在内存中解密，供预览与打印复用。
pub(crate) fn pdf_for_use(bytes: Vec<u8>) -> Result<Vec<u8>> {
    // 普通/外来 PDF 仍交原有阅读器处理；探测失败不能额外收紧 hayro 的兼容范围。
    if !Document::load_metadata_mem(&bytes).is_ok_and(|metadata| metadata.encrypted) {
        return Ok(bytes);
    }
    let protection =
        Protection::saved_password()?.context("该 PDF 需要导出密码，请用 PDF 阅读器打开")?;
    let mut doc = Document::load_mem_with_options(
        &bytes,
        lopdf::LoadOptions::with_password(&protection.password),
    )
    .context("当前导出密码无法打开该 PDF，请用 PDF 阅读器输入生成该文件时的密码")?;
    // 带口令加载会解密对象并移除 Encrypt；不能对明文对象再解密一次。
    ensure!(!doc.is_encrypted(), "PDF 解密未完成");
    let mut output = Vec::new();
    doc.save_to(&mut output)?;
    Ok(output)
}

/// PDF AES-256 口令上限是 127 个 UTF-8 字节；统一拒绝截断，三种格式使用同一完整口令。
pub(crate) fn validate_password(password: &str) -> Result<()> {
    crate::password_policy::validate_export(password)?;
    ensure!(
        password.len() <= 127,
        "导出密码不能超过 127 个 UTF-8 字节（中文通常每字 3 字节）"
    );
    Ok(())
}

/// 随机导出密码：按音节拼读，混入大小写、数字与符号，便于口述与手抄。
///
/// 不用「随机字节转十六进制」：那种串既没有拼读规则，也抄不准。这里每次
/// 生成三个可拼读的音节词（辅音/双字母起首 + 元音 [+ 词末尾辅音]），
/// 词间插符号与数字，形状类似 `Varen#Millu72Tako`；最后整体过一遍导出密码
/// 强度校验，不合格就重来。规则只有一处：[`crate::password_policy::validate_export`]。
pub(crate) fn generated_password() -> String {
    let mut rng = rand::rng();
    loop {
        let password = pronounceable_password(&mut rng);
        if crate::password_policy::validate_export(&password).is_ok() {
            return password;
        }
    }
}

/// 音节素材：辅音（含常见双字母组合）、元音、尾辅音。
const ONSETS: &[&str] = &[
    "b", "d", "f", "g", "k", "l", "m", "n", "p", "r", "s", "t", "v", "z", "br", "dr", "fr", "gr",
    "kl", "pl", "st", "tr", "ch", "sh", "th",
];
const VOWELS: &[char] = &['a', 'e', 'i', 'o', 'u'];
/// 约四成五的词在词末加一个尾辅音，读起来有节奏，也不至于太长。
const CODAS: &[&str] = &["n", "r", "l", "s", "m", "k", "t"];
/// 只收在文本框、阅读器、解压工具里都不会引发转义歧义的符号。
const SYMBOLS: &[char] = &['!', '#', '%', '&', '*', '+', '=', '?', '@', '~', '^', '-'];

/// 生成一个「三词 + 两组分隔」的口令，每词首字母大写。
///
/// 每词两个音节；尾辅音只加在词末，避免「辅音 + 双字母起首」拼出
/// `tratm` 这类读不出来的连缀。
fn pronounceable_password(rng: &mut impl Rng) -> String {
    let mut out = String::new();
    for word in 0..3 {
        match word {
            1 => out.push(SYMBOLS[rng.random_range(0..SYMBOLS.len())]),
            2 => {
                for _ in 0..rng.random_range(2..=3) {
                    out.push(char::from(b'0' + rng.random_range(0..10u8)));
                }
                out.push(SYMBOLS[rng.random_range(0..SYMBOLS.len())]);
            }
            _ => {}
        }
        let mut syllables = String::new();
        for _ in 0..2 {
            syllables.push_str(ONSETS[rng.random_range(0..ONSETS.len())]);
            syllables.push(VOWELS[rng.random_range(0..VOWELS.len())]);
        }
        if rng.random_bool(0.45) {
            syllables.push_str(CODAS[rng.random_range(0..CODAS.len())]);
        }
        // 首字母大写：既满足「含大写字母」，也让三个词一眼可分。
        let mut chars = syllables.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    const PASSWORD: &str = "Jade!River7Cloud";

    struct Workspace(tempfile::TempDir);
    impl Workspace {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            crate::storage::set_test_config_dir(Some(dir.path().into()));
            crate::vault::set_key_handle([37; 32], true);
            Self(dir)
        }
        fn configure(&self) {
            crate::vault::write_path(
                &self.0.path().join(PASSWORD_FILE),
                "config",
                PASSWORD.as_bytes(),
            )
            .unwrap();
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            crate::vault::clear_key_handle();
            crate::storage::set_test_config_dir(None);
        }
    }

    #[test]
    fn generated_password_is_pronounceable_and_meets_export_rules() {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(2026_1110);
        for _ in 0..500 {
            let password = pronounceable_password(&mut rng);
            crate::password_policy::validate_export(&password)
                .unwrap_or_else(|error| panic!("生成的口令不合规：{password}：{error}"));
            assert!(
                (16..=26).contains(&password.chars().count()),
                "长度异常：{password}"
            );
            for category in ["upper", "lower", "digit", "symbol"] {
                let present = password.chars().any(|ch| match category {
                    "upper" => ch.is_ascii_uppercase(),
                    "lower" => ch.is_ascii_lowercase(),
                    "digit" => ch.is_ascii_digit(),
                    _ => !ch.is_ascii_alphanumeric(),
                });
                assert!(present, "缺 {category}：{password}");
            }

            // 拼读规则：元音占字母的四分之一以上，且不出现 4 个连续辅音。
            let letters = password
                .chars()
                .filter(char::is_ascii_alphabetic)
                .map(|ch| ch.to_ascii_lowercase())
                .collect::<Vec<_>>();
            let vowels = letters.iter().filter(|ch| "aeiou".contains(**ch)).count();
            assert!(vowels * 4 >= letters.len(), "元音太少：{password}");
            let mut run = 0;
            for ch in &letters {
                run = if "aeiou".contains(*ch) { 0 } else { run + 1 };
                assert!(run < 4, "辅音连缀过长：{password}");
            }
        }
    }

    #[test]
    fn generated_password_survives_full_export_validation() {
        for _ in 0..50 {
            let password = generated_password();
            assert!(validate_password(&password).is_ok(), "{password}");
        }
    }

    fn pdf() -> Vec<u8> {
        use lopdf::{Object, Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        let pages = doc.new_object_id();
        let stream = doc.add_object(Stream::new(
            dictionary! {},
            b"BT (confidential-body) Tj ET".to_vec(),
        ));
        let page = doc.add_object(dictionary! {"Type" => "Page", "Parent" => pages, "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()], "Contents" => stream});
        doc.objects.insert(
            pages,
            dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}
                .into(),
        );
        let root = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", root);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn native_pdf_requires_password_and_previews_in_memory() {
        let workspace = Workspace::new();
        workspace.configure();
        let encrypted = encrypt_pdf(&pdf(), PASSWORD).unwrap();
        assert!(Document::load_metadata_mem(&encrypted).unwrap().encrypted);
        assert!(hayro::hayro_interpret::hayro_syntax::Pdf::new(encrypted.clone()).is_err());
        assert!(
            Document::load_mem_with_options(&encrypted, lopdf::LoadOptions::with_password("wrong"))
                .is_err()
        );
        let pdf = hayro::hayro_interpret::hayro_syntax::Pdf::new_with_password(
            encrypted.clone(),
            PASSWORD,
        )
        .unwrap();
        assert_eq!(pdf.pages().len(), 1);
        let plain = pdf_for_use(encrypted).unwrap();
        assert!(!Document::load_metadata_mem(&plain).unwrap().encrypted);
        assert_eq!(Document::load_mem(&plain).unwrap().get_pages().len(), 1);
    }

    #[test]
    fn office_agile_decrypts_with_independent_reader() {
        let mut bytes = Cursor::new(Vec::new());
        docx_rs::Docx::new()
            .add_paragraph(
                docx_rs::Paragraph::new()
                    .add_run(docx_rs::Run::new().add_text("这是正文和人员信息")),
            )
            .build()
            .pack(&mut bytes)
            .unwrap();
        let bytes = bytes.into_inner();
        let encrypted = encrypt_office(&bytes, PASSWORD).unwrap();
        assert_eq!(&encrypted[..8], b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1");
        // 此独立读取库不验证口令，错误口令会返回垃圾字节；必须比较解密内容。
        assert_ne!(
            office_crypto::decrypt_from_bytes(encrypted.clone(), "wrong")
                .ok()
                .as_deref(),
            Some(bytes.as_slice())
        );
        assert_eq!(
            office_crypto::decrypt_from_bytes(encrypted, PASSWORD).unwrap(),
            bytes
        );
    }

    #[test]
    fn protected_workbook_round_trips_and_rejects_wrong_password_or_modified_ciphertext() {
        use crate::models::{VocabularyCategory, VocabularyEntry};
        use std::io::{Read, Seek, SeekFrom};
        let workspace = Workspace::new();
        workspace.configure();
        let unit = VocabularyEntry {
            code: "01".into(),
            canonical: "示例机关".into(),
            identity: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        };
        let person = VocabularyEntry {
            category: VocabularyCategory::Person,
            code: "01".into(),
            canonical: "张三".into(),
            identity: uuid::Uuid::new_v4().to_string(),
            owner_identity: unit.identity.clone(),
            unit: "01".into(),
            phone: "010-12345678".into(),
            profile: "示例简介".into(),
            ..Default::default()
        };
        let entries = [unit, person];
        let raw = crate::vocabulary_xlsx::xlsx_bytes(&entries, &["01".into()]).unwrap();
        let path = workspace.0.path().join("标准词库.xlsx");
        assert_eq!(write_final(&path, &raw).unwrap(), path);
        let encrypted = std::fs::read(&path).unwrap();
        assert!(encrypted.starts_with(&office::COMPOUND_MAGIC));
        assert_eq!(
            office_crypto::decrypt_from_bytes(encrypted.clone(), PASSWORD).unwrap(),
            raw
        );
        let plaintext = office::decrypt(&encrypted, PASSWORD).unwrap();
        assert_eq!(&*plaintext, &raw);
        let parsed = crate::vocabulary_xlsx::parse_bytes(&plaintext, &[]).unwrap();
        assert_eq!(parsed.entries[1].identity, entries[1].identity);
        assert_eq!(parsed.entries[1].owner_identity, entries[0].identity);
        assert_eq!(parsed.entries[1].phone, entries[1].phone);
        assert_eq!(parsed.entries[1].profile, entries[1].profile);
        assert!(office::decrypt(&encrypted, "wrong").is_err());

        let mut compound = cfb::CompoundFile::open(Cursor::new(encrypted)).unwrap();
        let mut stream = compound.open_stream("/EncryptedPackage").unwrap();
        let mut byte = [0];
        stream.seek(SeekFrom::Start(8)).unwrap();
        stream.read_exact(&mut byte).unwrap();
        stream.seek(SeekFrom::Start(8)).unwrap();
        stream.write_all(&[byte[0] ^ 1]).unwrap();
        stream.flush().unwrap();
        drop(stream);
        assert!(office::decrypt(&compound.into_inner().into_inner(), PASSWORD).is_err());
    }

    #[test]
    fn protected_workbook_from_an_independent_agile_writer_imports() {
        // 夹具由 msoffcrypto-tool（Python 独立实现）按同一套 Agile 参数加密，内容是
        // 模拟词库。它验证读取端不只认本程序 writer 的输出。
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/vocabulary/agile-encrypted-standard-vocabulary.xlsx"
        ))
        .unwrap();
        let plaintext = office::decrypt(&bytes, "Fixture!Pass2026").unwrap();
        assert!(plaintext.starts_with(b"PK"));
        let parsed = crate::vocabulary_xlsx::parse_bytes(&plaintext, &[]).unwrap();
        assert!(!parsed.entries.is_empty());
    }

    #[test]
    fn protected_workbook_rejects_unsupported_encryption_parameters() {
        use std::io::{Read, Seek, SeekFrom};
        let workspace = Workspace::new();
        workspace.configure();
        let raw = crate::vocabulary_xlsx::xlsx_bytes(&[], &[]).unwrap();
        let path = workspace.0.path().join("模板.xlsx");
        write_final(&path, &raw).unwrap();
        let encrypted = std::fs::read(&path).unwrap();

        // 同一长度的 SHA512 → SHA256：模拟 Excel/WPS 用本程序不支持的算法，
        // 读取端应明确拒绝，不能按 SHA-512 硬解出一堆垃圾。
        let mut compound = cfb::CompoundFile::open(Cursor::new(encrypted)).unwrap();
        let mut info = compound.open_stream("/EncryptionInfo").unwrap();
        let mut metadata = Vec::new();
        info.read_to_end(&mut metadata).unwrap();
        let patched = String::from_utf8(metadata.clone())
            .unwrap()
            .replace("SHA512", "SHA256");
        assert_eq!(patched.len(), metadata.len());
        info.seek(SeekFrom::Start(0)).unwrap();
        info.write_all(patched.as_bytes()).unwrap();
        info.flush().unwrap();
        drop(info);
        let error = office::decrypt(&compound.into_inner().into_inner(), PASSWORD).unwrap_err();
        assert!(format!("{error:#}").contains("不受支持"), "{error:#}");
    }

    #[test]
    #[ignore = "需要真实 Excel/WPS 生成的加密工作簿：GONGWEN_ENCRYPTED_XLSX / GONGWEN_ENCRYPTED_XLSX_PASSWORD"]
    fn real_excel_encrypted_workbook_imports() {
        let path = std::env::var_os("GONGWEN_ENCRYPTED_XLSX").expect("请指定加密工作簿路径");
        let password = std::env::var("GONGWEN_ENCRYPTED_XLSX_PASSWORD").expect("请指定工作簿密码");
        let bytes = std::fs::read(path).unwrap();
        let plaintext = office::decrypt(&bytes, &password).unwrap();
        assert!(plaintext.starts_with(b"PK"), "解密结果应是 OOXML 包");
    }

    #[test]
    fn encrypted_export_replaces_previous_plaintext_siblings_and_manifest() {
        use crate::models::{ExportSelection, TemplateKind};
        let workspace = Workspace::new();
        let output_dir = workspace.0.path().join("out");
        let mut input = crate::models::DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "示例机关".into();
        input.profile.recipient = "收文机关".into();
        input.profile.signing_unit = "示例机关".into();
        let selection = ExportSelection {
            markdown: true,
            docx: false,
            pdf: false,
            typst: false,
            receipt: false,
            referenced_vocabulary: false,
            overwrite: true,
            print_trace: Default::default(),
        };
        let export = |selection: &ExportSelection| {
            crate::export::export_artifacts(
                &output_dir,
                &input,
                "# 测试公文\n\n正文。",
                selection,
                &crate::units::UnitDisplay::new(&[]),
                &Default::default(),
                &Default::default(),
                None,
                &Default::default(),
            )
            .unwrap()
        };

        // 1) 普通工作区先导出一次：留下明文源码包与导出清单。
        crate::vault::clear_key_handle();
        let plaintext = export(&selection);
        let plaintext_zip = plaintext
            .files
            .iter()
            .find(|path| path.extension().is_some_and(|ext| ext == "zip"))
            .expect("应生成明文源码包")
            .clone();
        assert!(plaintext_zip.is_file());
        let document_dir = plaintext_zip.parent().unwrap().to_path_buf();
        let manifest = document_dir.join(".gongwen-exports.json");
        assert!(manifest.is_file());

        // 2) 同一篇改按加密工作区导出：新产物是 .7z，旧明文源码包与清单里的旧名
        //    要一起收掉，不能让用户以为再导出一次就只剩加密成品。
        crate::vault::set_key_handle([37; 32], true);
        workspace.configure();
        let encrypted = export(&selection);
        assert!(
            encrypted
                .files
                .iter()
                .any(|path| path.extension().is_some_and(|ext| ext == "7z")),
            "{:?}",
            encrypted.files
        );
        assert!(!plaintext_zip.exists(), "旧的明文源码包应被清理");
        let manifest_text = std::fs::read_to_string(&manifest).unwrap();
        assert!(!manifest_text.contains("-源码包.zip"), "{manifest_text}");
        assert!(manifest_text.contains("-源码包.7z"), "{manifest_text}");
    }

    #[test]
    fn plaintext_export_is_refused_when_the_vault_is_locked() {
        let workspace = Workspace::new();
        // 锁文件存在 = 已启用应用锁；清掉密钥模拟锁定/备份恢复的过渡。
        let _ = crate::vault::Vault::create("Test#Valid2026Lock", None, false).unwrap();
        crate::vault::clear_key_handle();
        assert!(matches!(
            crate::vault::Vault::probe(),
            crate::vault::Probe::Locked
        ));

        assert!(Protection::current().is_err(), "锁定时不能当普通工作区");
        let path = workspace.0.path().join("out.pdf");
        let error = write_final_with(None, &path, b"%PDF-1.7").unwrap_err();
        assert!(format!("{error:#}").contains("已锁定"), "{error:#}");
        assert!(!path.exists());
    }

    #[test]
    fn live_letter_exports_partial_snapshot_and_only_its_referenced_workbook() {
        use crate::models::{
            DraftInput, ExportSelection, JointContact, VocabularyCategory, VocabularyEntry,
        };
        let workspace = Workspace::new();
        workspace.configure();
        let stored = VocabularyEntry {
            canonical: "归档机关名".into(),
            identity: uuid::Uuid::new_v4().to_string(),
            code: "01".into(),
            ..Default::default()
        };
        let child = VocabularyEntry {
            canonical: "业务处".into(),
            code: "0101".into(),
            identity: uuid::Uuid::new_v4().to_string(),
            owner_identity: stored.identity.clone(),
            parent: stored.code.clone(),
            ..Default::default()
        };
        let contact = VocabularyEntry {
            canonical: "李四".into(),
            category: VocabularyCategory::Person,
            phone: "010-22223333".into(),
            code: "01".into(),
            identity: uuid::Uuid::new_v4().to_string(),
            owner_identity: child.identity.clone(),
            unit: child.code.clone(),
            ..Default::default()
        };
        let inactive = VocabularyEntry {
            canonical: contact.canonical.clone(),
            code: "02".into(),
            identity: uuid::Uuid::new_v4().to_string(),
            phone: "010-99999999".into(),
            ..contact.clone()
        };
        let live = VocabularyEntry {
            canonical: "改名后的机关".into(),
            ..stored.clone()
        };
        let entries = [live, child.clone(), contact.clone(), inactive.clone()];
        let mut input = DraftInput::default();
        input.profile.issuing_unit = stored.key();
        input.profile.responsible_unit = stored.key();
        input.profile.recipient = "收文单位".into();
        input.profile.joint_contacts.push(JointContact {
            unit: child.key(),
            name: inactive.key(),
            phone: inactive.phone.clone(),
        });
        input
            .profile
            .additional_responsible_contacts
            .push(JointContact {
                unit: child.key(),
                name: contact.key(),
                phone: contact.phone.clone(),
            });
        input.profile.vocabulary_snapshot.push(stored.clone());
        assert!(crate::units::identity::ensure_historical_references(&input).is_err());

        let mut selection = ExportSelection {
            docx: true,
            pdf: crate::portable_runtime::find_font_dir().is_some(),
            markdown: false,
            ..Default::default()
        };
        let export = |selection: &ExportSelection| {
            crate::export::export_artifacts(
                workspace.0.path(),
                &input,
                "# 测试公函\n\n正文。",
                selection,
                &crate::units::UnitDisplay::new(&entries),
                &Default::default(),
                &Default::default(),
                None,
                &Default::default(),
            )
            .unwrap()
        };
        let without = export(&selection);
        assert!(
            without
                .files
                .iter()
                .all(|path| path.extension().unwrap() != "xlsx")
        );
        selection.referenced_vocabulary = true;
        let with = export(&selection);
        assert!(with.compile_error.is_none(), "{:?}", with.compile_error);
        let workbook = with
            .files
            .iter()
            .find(|path| path.extension().unwrap() == "xlsx")
            .unwrap();
        let bytes = office::decrypt(&std::fs::read(workbook).unwrap(), PASSWORD).unwrap();
        let sheet = crate::vocabulary_xlsx::parse_bytes(&bytes, &[]).unwrap();
        assert_eq!(sheet.entries.len(), 3);
        assert!(
            sheet
                .entries
                .iter()
                .any(|row| row.identity == stored.identity && row.canonical == stored.canonical)
        );
        assert!(
            sheet
                .entries
                .iter()
                .any(|row| row.identity == contact.identity && row.phone == contact.phone)
        );
        assert!(
            !sheet
                .entries
                .iter()
                .any(|row| row.identity == inactive.identity)
        );
        assert_eq!(input.profile.vocabulary_snapshot, vec![stored]);

        let docx = with
            .files
            .iter()
            .find(|path| path.extension().unwrap() == "docx")
            .unwrap();
        let bytes =
            office_crypto::decrypt_from_bytes(std::fs::read(docx).unwrap(), PASSWORD).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("word/document.xml").unwrap(), &mut xml)
            .unwrap();
        assert!(xml.contains("归档机关名") && xml.contains("010-22223333"));
        assert!(!xml.contains("改名后的机关") && !xml.contains("010-99999999"));
    }

    #[test]
    fn batch_outputs_and_roster_are_encrypted_and_source_zip_never_written() {
        let workspace = Workspace::new();
        workspace.configure();
        let root = workspace.0.path().join("output");
        let zip_path = root.join("源码包.zip");
        let roster_path = root.join("正文.份号清单.txt");
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("秘密标题.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all("这是秘密正文".as_bytes()).unwrap();
        let paths = Protection::current()
            .unwrap()
            .unwrap()
            .persist(HashMap::from([
                (zip_path.clone(), zip.finish().unwrap().into_inner()),
                (roster_path.clone(), "单位甲：份号01".as_bytes().to_vec()),
                (root.join("正文.pdf"), pdf()),
            ]))
            .unwrap();
        assert!(!zip_path.exists());
        assert!(!roster_path.exists());
        for source in [&zip_path, &roster_path] {
            let bytes = std::fs::read(&paths[source]).unwrap();
            let mut wrong = Cursor::new(&bytes);
            assert!(
                sevenz_rust2::Archive::read(&mut wrong, &sevenz_rust2::Password::from("wrong"))
                    .is_err()
            );
            let mut right = Cursor::new(&bytes);
            let archive =
                sevenz_rust2::Archive::read(&mut right, &sevenz_rust2::Password::from(PASSWORD))
                    .unwrap();
            assert_eq!(archive.files.len(), 1);
        }
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 3);
    }

    #[test]
    fn document_export_protects_all_selected_formats_and_companion_sheets() {
        let workspace = Workspace::new();
        workspace.configure();
        let root = workspace.0.path().join("exported");
        let mut input = crate::models::DraftInput {
            kind: crate::models::TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.recipient = "甲单位".into();
        input.profile.issuing_unit = "某局".into();
        input.profile.number_copies = true;
        // PDF 与两类随行单据都要随包字体；没有 runtime/fonts 的环境（CI）排不了版，
        // 只验其余格式的加密，依赖 PDF 的断言（有随行单据、有份号清单）随之跳过。
        let fonts_available = crate::portable_runtime::find_font_dir().is_some();
        let selection = crate::models::ExportSelection {
            markdown: true,
            docx: true,
            pdf: fonts_available,
            typst: true,
            receipt: fonts_available,
            ..Default::default()
        };
        let artifacts = super::super::export_artifacts(
            &root,
            &input,
            "# 加密交付测试\n\n正文与人员信息。",
            &selection,
            &crate::units::UnitDisplay::new(&[]),
            &crate::models::FontConfig::default(),
            &crate::models::NumberingConfig::default(),
            None,
            &crate::models::TransmitDefaults::default(),
        )
        .unwrap();
        assert!(
            artifacts.compile_error.is_none(),
            "{:?}",
            artifacts.compile_error
        );
        if fonts_available {
            assert!(artifacts.sheets.receipt >= 1);
            assert!(artifacts.sheets.transmit >= 1);
            assert!(
                artifacts
                    .files
                    .iter()
                    .any(|p| p.to_string_lossy().contains("份号清单.7z"))
            );
        }
        // Word 与源码包（Typst 包）各一份，缺字体时也必须照常加密交付。
        assert!(
            artifacts.files.len() >= 3,
            "加密产物过少：{:?}",
            artifacts.files
        );
        for path in artifacts.files {
            let bytes = std::fs::read(&path).unwrap();
            match path.extension().unwrap().to_str().unwrap() {
                "pdf" => assert!(Document::load_metadata_mem(&bytes).unwrap().encrypted),
                "docx" | "xlsx" => {
                    let plain = office_crypto::decrypt_from_bytes(bytes.clone(), PASSWORD).unwrap();
                    assert!(zip::ZipArchive::new(Cursor::new(&plain)).is_ok());
                    assert_ne!(
                        office_crypto::decrypt_from_bytes(bytes, "wrong").ok(),
                        Some(plain)
                    );
                }
                "7z" => {
                    assert!(
                        sevenz_rust2::Archive::read(
                            &mut Cursor::new(&bytes),
                            &sevenz_rust2::Password::empty()
                        )
                        .is_err()
                    );
                }
                ext => panic!("不得生成明文产物：{ext}"),
            }
        }
    }

    #[test]
    fn legacy_disabled_setting_still_writes_encrypted_and_keeps_old_pdf_access() {
        let workspace = Workspace::new();
        workspace.configure();
        let encrypted = encrypt_pdf(&pdf(), PASSWORD).unwrap();
        let mut policy = ExportPolicy::load().unwrap();
        policy.enabled = false;
        policy.save(&crate::vault::key_handle().unwrap()).unwrap();
        assert!(Protection::current().unwrap().is_some());
        let path = workspace.0.path().join("成品.md");
        let output = write_final(&path, b"protected body").unwrap();
        assert_eq!(output, path.with_extension("7z"));
        assert!(!path.exists());
        assert!(
            !Document::load_metadata_mem(&pdf_for_use(encrypted).unwrap())
                .unwrap()
                .encrypted
        );
        policy.enabled = true;
        policy.save(&crate::vault::key_handle().unwrap()).unwrap();
        assert!(Protection::current().unwrap().is_some());
    }

    #[test]
    fn missing_password_and_failed_batch_never_fall_back_to_plaintext() {
        let workspace = Workspace::new();
        assert!(Protection::current().is_err());
        workspace.configure();
        let root = workspace.0.path().join("output");
        let files = HashMap::from([
            (root.join("正文.pdf"), pdf()),
            (root.join("不支持.bin"), b"secret".to_vec()),
        ]);
        assert!(
            Protection::current()
                .unwrap()
                .unwrap()
                .persist(files)
                .is_err()
        );
        assert!(!root.exists());
        let protection = Protection::current().unwrap().unwrap();
        crate::vault::clear_key_handle();
        assert!(protection.encode(Path::new("x.pdf"), &pdf()).is_err());
    }
}
