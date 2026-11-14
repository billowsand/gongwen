//! Writes a prepared ISO to the IS903 optical LUN using the device's vendor protocol.
//! Reference: aiburn PROTOCOL.md, section 11. Hosts never stage the image on disk.

use anyhow::{Context, Result, ensure};
use zeroize::Zeroizing;

#[cfg(target_os = "linux")]
#[path = "is903_writer/linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "is903_writer/macos.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "is903_writer/windows.rs"]
mod platform;

const SECTOR: usize = 2048;
const DEVICE_BLOCK: usize = 512;
const TRANSFER: usize = 64 * 1024;
pub(crate) const MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Target {
    pub(crate) session: u64,
    pub(crate) serial: String,
    pub(crate) disk: String,
    pub(crate) revision: u64,
    location: u64,
    #[cfg(target_os = "macos")]
    media_registry_id: u64,
}

pub(crate) fn scan() -> Result<Vec<Target>> {
    platform::scan()
}

pub(crate) fn write_iso(
    target: &Target,
    image: &[u8],
    authorize: impl Fn() -> Result<()>,
) -> Result<()> {
    validate_image(image)?;
    recheck(target)?;
    platform::write(target, image, &authorize)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "is903_writer/helper.rs"]
mod helper;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) use platform::helper_main;

fn valid_serial(serial: &str) -> bool {
    !serial.is_empty() && serial.len() <= 128 && serial.bytes().all(|byte| byte.is_ascii_graphic())
}

fn recheck(target: &Target) -> Result<()> {
    let current = scan()?;
    let mut matches = current
        .iter()
        .filter(|device| device.serial == target.serial);
    ensure!(
        matches.next() == Some(target) && matches.next().is_none(),
        "IS903 已拔出、换盘或存在重复序列号"
    );
    Ok(())
}

fn validate_image(image: &[u8]) -> Result<()> {
    ensure!(
        image.len() >= SECTOR * 17
            && image.len() <= MAX_IMAGE_BYTES
            && image.len().is_multiple_of(SECTOR),
        "ISO 镜像长度无效或超过 IS903 导出上限"
    );
    let pvd = &image[16 * SECTOR..17 * SECTOR];
    ensure!(&pvd[..7] == b"\x01CD001\x01", "ISO 主卷描述符无效");
    let expected = u32::try_from(image.len() / SECTOR)?;
    ensure!(
        pvd[80..84] == expected.to_le_bytes() && pvd[84..88] == expected.to_be_bytes(),
        "ISO 卷空间与镜像大小不一致"
    );
    Ok(())
}

trait Scsi {
    /// Transfers the complete buffer, including checking transport and SCSI status.
    fn transfer(&self, cdb: &[u8], input: bool, data: &mut [u8]) -> Result<()>;
}

fn optical_capacity(drive: &impl Scsi) -> Result<u64> {
    let mut inquiry = [0; 36];
    drive.transfer(&[0x12, 0, 0, 0, 36, 0], true, &mut inquiry)?;
    ensure!(inquiry[0] & 0x1f == 5, "IS903 未处于 CD-ROM 模式");

    let mut capacity = [0; 8];
    drive.transfer(&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], true, &mut capacity)?;
    let last = u32::from_be_bytes(capacity[..4].try_into()?);
    let block_size = u32::from_be_bytes(capacity[4..].try_into()?);
    ensure!(block_size as usize == SECTOR, "IS903 光盘扇区大小异常");
    Ok((u64::from(last) + 1) * u64::from(block_size))
}

fn command_group(drive: &impl Scsi) -> Result<u8> {
    let mut inquiry = [0; 134];
    drive.transfer(
        b"\xfc\x00\x00\x00\x00\x00\x00\x00\x00\x80\x00\x00INQU",
        true,
        &mut inquiry,
    )?;
    ensure!(
        inquiry.windows(5).any(|part| part == b"IS903"),
        "设备未报告 IS903 固件"
    );
    let start = inquiry
        .windows(4)
        .position(|part| part == b"INQU")
        .context("设备命令组响应无效")?;
    let table = inquiry
        .get(start..start + usize::from(inquiry[5]) * 4)
        .context("设备命令组表越界")?;
    let index = table
        .as_chunks::<4>()
        .0
        .iter()
        .position(|tag| tag == b"P.M.")
        .context("设备不支持 IS903 光盘写入")?;
    Ok(u8::try_from(index)?)
}

fn partition_command(group: u8, opcode: u8, mode: u8, address: u32, size: u16) -> [u8; 16] {
    let mut command = *b"\xfc\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00P.M.";
    command[1..4].copy_from_slice(&[group, opcode, mode]);
    command[4..8].copy_from_slice(&address.to_be_bytes());
    command[8..10].copy_from_slice(&size.to_be_bytes());
    command
}

fn exchange(drive: &impl Scsi, group: u8, mode: u8) -> Result<()> {
    let mut challenge = [0; 2];
    drive.transfer(&partition_command(group, 8, 0, 0, 2), true, &mut challenge)?;
    let mut state = u16::from_be_bytes(challenge).reverse_bits() >> 1;
    let mut response = *b"Vm3RuP42j/ \0";
    for byte in &mut response {
        for position in 0..8 {
            let bit = (state ^ (state >> 1)) & 1;
            *byte ^= (bit as u8) << position;
            state = (state >> 1) | (bit << 14);
        }
    }
    drive.transfer(
        &partition_command(group, 0xff, mode, 0, 12),
        false,
        &mut response,
    )
}

fn burn(drive: &impl Scsi, image: &[u8], ready: &dyn Fn() -> Result<()>) -> Result<()> {
    ensure!(
        image.len() as u64 <= optical_capacity(drive)?,
        "镜像超过 IS903 光盘容量"
    );
    let group = command_group(drive)?;
    ready()?;
    exchange(drive, group, 0x80).context("IS903 写入前握手失败")?;
    let mut partitions = [0; 4096];
    drive.transfer(&partition_command(group, 3, 5, 0, 8), true, &mut partitions)?;
    ensure!(
        partitions
            .as_chunks::<DEVICE_BLOCK>()
            .0
            .iter()
            .any(|record| record[0] == 5 || record[..2] == [0, 0]),
        "IS903 缺少光盘分区记录"
    );
    let mut buffer = Zeroizing::new(vec![0; TRANSFER]);
    for (index, chunk) in image.chunks(TRANSFER).enumerate() {
        let address = u32::try_from(index * (TRANSFER / DEVICE_BLOCK))?;
        let blocks = u16::try_from(chunk.len() / DEVICE_BLOCK)?;
        let payload = &mut buffer[..chunk.len()];
        payload.copy_from_slice(chunk);
        drive
            .transfer(
                &partition_command(group, 0, 1, address, blocks),
                false,
                payload,
            )
            .context("IS903 写入失败，原光盘内容可能已部分覆盖")?;
    }
    exchange(drive, group, 0x81).context("IS903 写入收尾失败，盘上可能已留下数据")
}

fn verify(drive: &impl Scsi, image: &[u8]) -> Result<()> {
    ensure!(
        image.len() as u64 <= optical_capacity(drive)?,
        "IS903 回读容量异常"
    );
    let mut buffer = Zeroizing::new(vec![0u8; TRANSFER]);
    for (index, expected) in image.chunks(TRANSFER).enumerate() {
        let mut command = [0; 10];
        command[0] = 0x28;
        let address = u32::try_from(index * (TRANSFER / SECTOR))?;
        command[2..6].copy_from_slice(&address.to_be_bytes());
        let sectors = u16::try_from(expected.len() / SECTOR)?;
        command[7..9].copy_from_slice(&sectors.to_be_bytes());
        let part = &mut buffer[..expected.len()];
        drive.transfer(&command, true, part)?;
        ensure!(
            part == expected,
            "IS903 重新枚举后回读不一致，光盘扇区 {address}"
        );
    }
    Ok(())
}
