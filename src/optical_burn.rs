//! 数据光盘刻录。镜像始终由调用方从内存供给。
use anyhow::{Result, ensure};
use zeroize::Zeroizing;

const SECTOR_SIZE: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Drive {
    id: String,
    label: String,
    connection: u64,
    media: Media,
    sectors: u64,
}

impl Drive {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Media {
    CdR,
    DvdR,
    DvdPlusR,
}

impl Media {
    fn label(self) -> &'static str {
        match self {
            Self::CdR => "CD-R",
            Self::DvdR => "DVD-R",
            Self::DvdPlusR => "DVD+R",
        }
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

pub(crate) fn scan() -> Result<Vec<Drive>> {
    platform::scan()
}

pub(crate) fn burn(target: &Drive, iso: &[u8], authorize: impl Fn() -> Result<()>) -> Result<()> {
    ensure!(
        iso.len() >= 17 * SECTOR_SIZE && iso.len().is_multiple_of(SECTOR_SIZE),
        "光盘镜像的长度不符合数据光盘扇区要求"
    );
    ensure!(
        (iso.len() / SECTOR_SIZE) as u64 <= target.sectors,
        "光盘容量不足"
    );
    // A short ISO is legal, but some CD recorders require at least 300 data
    // sectors per track. Zeros following the ISO volume do not change its files.
    const MIN_CD_TRACK_BYTES: usize = 300 * SECTOR_SIZE;
    if target.media != Media::CdR || iso.len() >= MIN_CD_TRACK_BYTES {
        return platform::burn(target, iso, &authorize);
    }
    ensure!(target.sectors >= 300, "光盘容量不足");
    let mut track = Zeroizing::new(vec![0; MIN_CD_TRACK_BYTES]);
    track[..iso.len()].copy_from_slice(iso);
    platform::burn(target, &track, &authorize)
}
