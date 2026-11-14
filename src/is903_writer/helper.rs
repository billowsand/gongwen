//! IPC for a one-shot IS903 helper. Only a target, bounded image and final write token
//! cross the pipe. Helper runs before application storage and UI initialization.
use super::{MAX_IMAGE_BYTES, SECTOR, Target, recheck, validate_image};
use anyhow::{Context, Result, bail, ensure};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use zeroize::Zeroizing;

pub(super) const ARG: &str = "--is903-helper-v1";
const MAX_HEADER: usize = 1024;

pub(super) fn trusted_file(path: &Path) -> Result<()> {
    for (index, component) in path.ancestors().enumerate() {
        let metadata = std::fs::symlink_metadata(component)
            .with_context(|| format!("缺少管理员安装的组件 {}", component.display()))?;
        ensure!(
            metadata.uid() == 0
                && metadata.mode() & 0o022 == 0
                && if index == 0 {
                    metadata.is_file()
                } else {
                    metadata.is_dir()
                },
            "IS903 助手文件或目录权限不安全：{}",
            component.display()
        );
    }
    Ok(())
}

pub(super) fn send(
    target: &Target,
    image: &[u8],
    authorize: &dyn Fn() -> Result<()>,
    input: &mut impl Read,
    output: &mut impl Write,
) -> Result<()> {
    let header = serde_json::to_vec(target)?;
    ensure!(header.len() <= MAX_HEADER, "IS903 设备参数超限");
    recheck(target)?;
    authorize()?;
    output.write_all(&(header.len() as u32).to_le_bytes())?;
    output.write_all(&(image.len() as u32).to_le_bytes())?;
    output.write_all(&header)?;
    // Wait until the helper validates the requested target before transmitting the image.
    expect_status(input, b'H')?;
    output.write_all(image)?;
    expect_status(input, b'R')?;
    authorize()?;
    output.write_all(b"W")?;
    expect_status(input, b'O').context("收到写入确认后失败，介质可能已有数据")
}

fn status(input: &mut impl Read) -> Result<u8> {
    let mut byte = [0];
    input.read_exact(&mut byte).context("IS903 助手意外退出")?;
    Ok(byte[0])
}

fn expect_status(input: &mut impl Read, expected: u8) -> Result<()> {
    match status(input)? {
        byte if byte == expected => Ok(()),
        b'E' => {
            let mut message = Vec::new();
            input.take(2048).read_to_end(&mut message)?;
            bail!("IS903 刻录失败：{}", String::from_utf8_lossy(&message))
        }
        _ => bail!("IS903 助手响应无效"),
    }
}

unsafe extern "C" {
    fn geteuid() -> u32;
    fn setrlimit(resource: std::ffi::c_int, limits: *const [u64; 2]) -> std::ffi::c_int;
}

pub(super) fn run(
    helper: &str,
    core_limit: std::ffi::c_int,
    trusted_install: impl FnOnce() -> Result<()>,
    burn_and_verify: impl FnOnce(&Target, &[u8], &dyn Fn() -> Result<()>) -> Result<()>,
) -> Option<i32> {
    let mut args = std::env::args_os();
    let name = args.next()?;
    if Path::new(&name).file_name()?.to_str()? != "gongwen-is903" {
        return None;
    }
    let result = (|| {
        ensure!(
            args.next().is_some_and(|arg| arg == ARG) && args.next().is_none(),
            "IS903 助手仅接受固定版本的烧录请求"
        );
        // SAFETY: geteuid has no arguments or preconditions.
        ensure!(unsafe { geteuid() } == 0, "IS903 助手需要管理员授权");
        ensure!(
            std::env::current_exe()? == Path::new(helper),
            "IS903 助手安装位置无效"
        );
        trusted_install()?;
        ensure!(
            // SAFETY: The two u64 values are the native soft and hard rlim_t fields on supported Unix targets.
            unsafe { setrlimit(core_limit, &[0, 0]) } == 0,
            "无法关闭 IS903 助手的内存转储"
        );

        let mut input = std::io::stdin().lock();
        let mut lengths = [0; 8];
        input.read_exact(&mut lengths)?;
        let header_len = u32::from_le_bytes(lengths[..4].try_into()?) as usize;
        let image_len = u32::from_le_bytes(lengths[4..].try_into()?) as usize;
        ensure!(
            (1..=MAX_HEADER).contains(&header_len),
            "IS903 设备参数长度无效"
        );
        ensure!(
            (SECTOR * 17..=MAX_IMAGE_BYTES).contains(&image_len)
                && image_len.is_multiple_of(SECTOR),
            "ISO 镜像长度无效"
        );
        let mut header = vec![0; header_len];
        input.read_exact(&mut header)?;
        let target: Target = serde_json::from_slice(&header)?;
        recheck(&target)?;
        notify(b"H")?;
        let mut image = Zeroizing::new(vec![0; image_len]);
        input.read_exact(&mut image)?;
        validate_image(&image)?;
        recheck(&target)?;
        drop(input);
        burn_and_verify(&target, &image, &|| {
            notify(b"R")?;
            ensure!(
                status(&mut std::io::stdin().lock())? == b'W',
                "IS903 写入已取消"
            );
            Ok(())
        })?;
        notify(b"O")
    })();
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            let _ = notify(format!("E{error:#}").as_bytes());
            1
        }
    })
}

fn notify(message: &[u8]) -> Result<()> {
    let mut output = std::io::stdout().lock();
    output.write_all(message)?;
    output.flush()?;
    Ok(())
}
