//! 对外交付加密：明文只在内存中生成，加密后才允许写目标文件。
//! 导出口令独立设置，不由登录口令或本地 DEK 派生。
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
        ensure!(
            current.slot() == handle.slot() && current.dek() == handle.dek(),
            "工作区已变化，未保存导出设置"
        );
        let path = crate::storage::slot_file_for(handle.slot(), PASSWORD_FILE)?;
        let bytes = Zeroizing::new(serde_json::to_vec(self)?);
        let sealed = crate::vault::seal_path_bytes(handle, &path, "config", &bytes)?;
        crate::storage::atomic_write(&path, |file| {
            file.write_all(&sealed)?;
            Ok(())
        })
    }
}

pub(crate) fn encryption_enabled() -> Result<bool> {
    if crate::vault::key_handle().is_none() {
        return Ok(false);
    }
    Ok(ExportPolicy::load()?.enabled)
}

pub(crate) struct Protection {
    password: Zeroizing<String>,
    handle: crate::vault::KeyHandle,
}

impl Protection {
    pub(crate) fn current() -> Result<Option<Self>> {
        if !encryption_enabled()? {
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
            anyhow::bail!("请先到「设置 → 导出格式」单独设置导出密码");
        }
        let password = Zeroizing::new(policy.password.clone());
        validate_password(&password)?;
        Ok(Some(Self { password, handle }))
    }

    #[cfg(test)]
    pub(crate) fn matches_password(&self, password: &str) -> bool {
        self.password.as_str() == password
    }

    fn check_slot(&self) -> Result<()> {
        ensure!(
            crate::storage::current_slot() == self.handle.slot(),
            "工作区已变化，已取消导出"
        );
        let current = crate::vault::key_handle().context("当前已锁定，已取消导出")?;
        ensure!(
            current.slot() == self.handle.slot() && current.dek() == self.handle.dek(),
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

    pub(crate) fn encode(&self, path: &Path, bytes: &[u8]) -> Result<(PathBuf, Vec<u8>)> {
        self.check_slot()?;
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "pdf" => Ok((path.into(), encrypt_pdf(bytes, &self.password)?)),
            "docx" => Ok((path.into(), encrypt_word(bytes, &self.password)?)),
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
                    entries.push((entry.name().to_owned(), body));
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

pub(crate) fn write_final(path: &Path, bytes: &[u8]) -> Result<PathBuf> {
    if let Some(protection) = Protection::current()? {
        let paths = protection.persist(HashMap::from([(path.to_path_buf(), bytes.to_vec())]))?;
        return paths.get(path).cloned().context("加密输出未生成");
    }
    crate::storage::atomic_write(path, |file| {
        file.write_all(bytes)?;
        Ok(())
    })?;
    Ok(path.to_path_buf())
}

fn encrypt_word(bytes: &[u8], password: &str) -> Result<Vec<u8>> {
    ensure!(bytes.starts_with(b"PK"), "Word 产物不是有效的 OOXML 包");
    let mut writer = ms_offcrypto_writer::Ecma376AgileWriter::create(
        &mut rand::rng(),
        password,
        Cursor::new(Vec::new()),
    )
    .context("无法初始化 Word 加密")?;
    writer.write_all(bytes)?;
    Ok(writer.into_inner().context("Word 加密失败")?.into_inner())
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
        let encrypted = encrypt_word(&bytes, PASSWORD).unwrap();
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
                "docx" => {
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
    fn disabled_encryption_writes_plaintext_and_keeps_old_pdf_password_available() {
        let workspace = Workspace::new();
        workspace.configure();
        let encrypted = encrypt_pdf(&pdf(), PASSWORD).unwrap();
        let mut policy = ExportPolicy::load().unwrap();
        policy.enabled = false;
        policy.save(&crate::vault::key_handle().unwrap()).unwrap();
        assert!(Protection::current().unwrap().is_none());
        let path = workspace.0.path().join("明文.md");
        assert_eq!(write_final(&path, b"plain body").unwrap(), path);
        assert_eq!(std::fs::read(path).unwrap(), b"plain body");
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
