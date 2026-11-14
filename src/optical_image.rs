//! An in-memory, single-session data-disc image for exported documents.
//!
//! ISO 9660 (ECMA-119 sections 8 and 9) provides a flat 8.3 fallback tree.
//! The Joliet supplementary volume gives the same file extents their display names.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};

const BLOCK: usize = 2048;
const MAX_FILES: usize = 4096;
pub(crate) const MAX_IMAGE_BYTES: usize = 512 * 1024 * 1024;
const MIN_TRACK_BLOCKS: usize = 300;
const PRIMARY_DESCRIPTOR: usize = 16;
const JOLIET_DESCRIPTOR: usize = 17;
const DESCRIPTOR_TERMINATOR: usize = 18;
const PRIMARY_PATH: usize = 19;
const JOLIET_PATH: usize = 21;
const FIRST_DIRECTORY: usize = 23;
const DIRECTORY_DATE: [u8; 7] = [100, 1, 1, 0, 0, 0, 0];
const VOLUME_DATE: &[u8; 16] = b"2000010100000000";

struct File {
    primary: Vec<u8>,
    joliet: Vec<u8>,
    bytes: Vec<u8>,
    sector: usize,
}

/// Build a data track from unique, flat basenames. Reject names that cannot be
/// represented consistently on the recipient's Joliet reader.
pub(crate) fn build(files: BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>> {
    ensure!(
        !files.is_empty() && files.len() <= MAX_FILES,
        "光盘文件数须在 1 至 {MAX_FILES} 之间"
    );
    let mut seen = BTreeSet::new();
    let mut files = files
        .into_iter()
        .enumerate()
        .map(|(index, (name, bytes))| {
            let disc_name = joliet_name(&name)?;
            ensure!(
                seen.insert(disc_name.to_uppercase()),
                "光盘文件名在忽略大小写或缩短后冲突，请改名后重试"
            );
            let joliet = disc_name
                .encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect();
            let extension: String = name
                .rsplit_once('.')
                .into_iter()
                .flat_map(|(_, suffix)| suffix.chars())
                .filter(char::is_ascii_alphanumeric)
                .take(3)
                .map(|c| c.to_ascii_uppercase())
                .collect();
            Ok(File {
                primary: format!("F{index:07}.{extension};1").into_bytes(),
                joliet,
                bytes,
                sector: 0,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let primary = tree(FIRST_DIRECTORY, files.iter().map(|f| f.primary.len()));
    let mut joliet_order = (0..files.len()).collect::<Vec<_>>();
    joliet_order.sort_unstable_by(|&left, &right| files[left].joliet.cmp(&files[right].joliet));
    let joliet = tree(
        primary.sector + primary.blocks,
        joliet_order.iter().map(|&index| files[index].joliet.len()),
    );

    let mut next_sector = joliet.sector + joliet.blocks;
    for file in &mut files {
        file.sector = next_sector;
        // Distinct extents also for empty files, with a reserved block to keep
        // each extent within the volume even when the last file is empty.
        let blocks = file.bytes.len().div_ceil(BLOCK).max(1);
        next_sector = next_sector
            .checked_add(blocks)
            .context("光盘镜像大小超限")?;
        ensure!(
            next_sector <= MAX_IMAGE_BYTES / BLOCK,
            "光盘镜像超过 {} MiB，请拆分导出",
            MAX_IMAGE_BYTES / (1024 * 1024)
        );
    }

    let volume_blocks = next_sector.max(MIN_TRACK_BLOCKS);
    let mut image = Vec::new();
    image
        .try_reserve_exact(volume_blocks * BLOCK)
        .context("无法为光盘镜像分配内存")?;
    image.resize(volume_blocks * BLOCK, 0);

    write_volume(&mut image, primary, false, volume_blocks);
    write_volume(&mut image, joliet, true, volume_blocks);
    descriptor_header(sector(&mut image, DESCRIPTOR_TERMINATOR), 255);
    write_directory(
        &mut image,
        primary,
        files.iter().map(|f| (f.primary.as_slice(), f)),
    );
    write_directory(
        &mut image,
        joliet,
        joliet_order.iter().map(|&index| {
            let file = &files[index];
            (file.joliet.as_slice(), file)
        }),
    );
    for file in files {
        let start = file.sector * BLOCK;
        image[start..start + file.bytes.len()].copy_from_slice(&file.bytes);
    }

    Ok(image)
}

fn joliet_name(name: &str) -> Result<String> {
    ensure!(
        !name.is_empty()
            && name.trim() == name
            && !name.ends_with('.')
            && name.chars().all(|c| {
                u32::from(c) <= 0xFFFF
                    && !c.is_control()
                    && !matches!(
                        c,
                        '\u{fffe}'
                            | '\u{ffff}'
                            | '"'
                            | '*'
                            | '/'
                            | ':'
                            | ';'
                            | '<'
                            | '>'
                            | '?'
                            | '\\'
                            | '|'
                    )
            }),
        "光盘文件名含 Joliet 不支持的字符，请改名后重试"
    );
    let name = if name.chars().count() > 64 {
        let (stem, extension) = name
            .rsplit_once('.')
            .filter(|(stem, suffix)| !stem.is_empty() && suffix.chars().count() <= 16)
            .map_or((name, ""), |(stem, _)| (stem, &name[stem.len()..]));
        let shortened: String = stem.chars().take(64 - extension.chars().count()).collect();
        format!("{}{extension}", shortened.trim_end_matches([' ', '.']))
    } else {
        name.to_owned()
    };
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let legacy_port = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
    ensure!(
        !legacy_port && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL"),
        "文件名为系统保留名称，请改名后重试"
    );
    Ok(name)
}

#[derive(Clone, Copy)]
struct Directory {
    sector: usize,
    blocks: usize,
}

fn record_size(identifier: usize) -> usize {
    (33 + identifier).next_multiple_of(2)
}

fn entry_offset(offset: usize, len: usize) -> usize {
    if offset % BLOCK + len > BLOCK {
        offset.next_multiple_of(BLOCK)
    } else {
        offset
    }
}

fn tree(sector: usize, names: impl Iterator<Item = usize>) -> Directory {
    let bytes = names.fold(2 * record_size(1), |offset, name| {
        let len = record_size(name);
        entry_offset(offset, len) + len
    });
    Directory {
        sector,
        blocks: bytes.div_ceil(BLOCK),
    }
}

fn both16(dst: &mut [u8], value: u16) {
    dst[..2].copy_from_slice(&value.to_le_bytes());
    dst[2..4].copy_from_slice(&value.to_be_bytes());
}

fn both32(dst: &mut [u8], value: usize) {
    // Layout and byte lengths are bounded by MAX_IMAGE_BYTES.
    let value = value as u32;
    dst[..4].copy_from_slice(&value.to_le_bytes());
    dst[4..8].copy_from_slice(&value.to_be_bytes());
}

fn sector(image: &mut [u8], index: usize) -> &mut [u8] {
    &mut image[index * BLOCK..(index + 1) * BLOCK]
}

fn descriptor_header(dst: &mut [u8], kind: u8) {
    dst[..7].copy_from_slice(&[kind, b'C', b'D', b'0', b'0', b'1', 1]);
}

fn write_volume(image: &mut [u8], root: Directory, joliet: bool, blocks: usize) {
    let (descriptor, path, kind) = if joliet {
        (JOLIET_DESCRIPTOR, JOLIET_PATH, 2)
    } else {
        (PRIMARY_DESCRIPTOR, PRIMARY_PATH, 1)
    };
    let dst = sector(image, descriptor);
    descriptor_header(dst, kind);
    for field in [
        8..40,
        40..72,
        190..318,
        318..446,
        446..574,
        574..702,
        702..739,
        739..776,
        776..813,
    ] {
        if joliet {
            for pair in dst[field].as_chunks_mut::<2>().0 {
                pair[1] = b' ';
            }
        } else {
            dst[field].fill(b' ');
        }
    }
    let label = "GONGWEN";
    if joliet {
        for (pair, unit) in dst[40..72]
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(label.encode_utf16())
        {
            pair.copy_from_slice(&unit.to_be_bytes());
        }
        dst[88..91].copy_from_slice(b"%/E");
    } else {
        dst[40..40 + label.len()].copy_from_slice(label.as_bytes());
    }
    both32(&mut dst[80..88], blocks);
    both16(&mut dst[120..124], 1);
    both16(&mut dst[124..128], 1);
    both16(&mut dst[128..132], BLOCK as u16);
    both32(&mut dst[132..140], 10); // Root is the only path table entry.
    dst[140..144].copy_from_slice(&(path as u32).to_le_bytes());
    dst[148..152].copy_from_slice(&((path + 1) as u32).to_be_bytes());
    record(
        &mut dst[156..190],
        &[0],
        root.sector,
        root.blocks * BLOCK,
        true,
    );
    dst[813..829].copy_from_slice(VOLUME_DATE);
    dst[830..846].copy_from_slice(VOLUME_DATE);
    dst[847..863].fill(b'0'); // No expiration.
    dst[864..880].copy_from_slice(VOLUME_DATE);
    dst[881] = 1;

    let little = sector(image, path);
    little[0] = 1;
    little[2..6].copy_from_slice(&(root.sector as u32).to_le_bytes());
    little[6..8].copy_from_slice(&1u16.to_le_bytes());

    let big = sector(image, path + 1);
    big[0] = 1;
    big[2..6].copy_from_slice(&(root.sector as u32).to_be_bytes());
    big[6..8].copy_from_slice(&1u16.to_be_bytes());
}

fn record(dst: &mut [u8], name: &[u8], start: usize, size: usize, directory: bool) {
    dst[0] = dst.len() as u8;
    both32(&mut dst[2..10], start);
    both32(&mut dst[10..18], size);
    dst[18..25].copy_from_slice(&DIRECTORY_DATE);
    dst[25] = if directory { 2 } else { 0 };
    both16(&mut dst[28..32], 1);
    dst[32] = name.len() as u8;
    dst[33..33 + name.len()].copy_from_slice(name);
}

fn write_directory<'a>(
    image: &mut [u8],
    root: Directory,
    files: impl Iterator<Item = (&'a [u8], &'a File)>,
) {
    let start = root.sector * BLOCK;
    let size = root.blocks * BLOCK;
    let dst = &mut image[start..start + size];
    let special = record_size(1);
    record(&mut dst[..special], &[0], root.sector, size, true);
    record(
        &mut dst[special..2 * special],
        &[1],
        root.sector,
        size,
        true,
    );

    let mut offset = 2 * special;
    for (name, file) in files {
        let len = record_size(name.len());
        offset = entry_offset(offset, len);
        record(
            &mut dst[offset..offset + len],
            name,
            file.sector,
            file.bytes.len(),
            false,
        );
        offset += len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joliet_directory_crosses_block_boundaries_and_shares_file_extents() {
        let inputs: BTreeMap<_, _> = (0u8..48)
            .map(|index| {
                (
                    format!("{index:02}号文件.pdf"),
                    vec![index; usize::from(index) * 111],
                )
            })
            .collect();
        let image = build(inputs.clone()).unwrap();

        let pvd = &image[PRIMARY_DESCRIPTOR * BLOCK..][..BLOCK];
        let svd = &image[JOLIET_DESCRIPTOR * BLOCK..][..BLOCK];
        assert_eq!(&svd[..7], b"\x02CD001\x01");
        assert_eq!(&svd[88..91], b"%/E");
        let read32 = |bytes: &[u8]| u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert_eq!(read32(&svd[80..]), image.len() / BLOCK);
        let file_records = |volume: &[u8]| {
            let root = read32(&volume[158..]) * BLOCK;
            let end = root + read32(&volume[166..]);
            let mut offset = root + 68; // "." and ".."
            let mut entries = Vec::new();
            while offset < end {
                let length = usize::from(image[offset]);
                if length == 0 {
                    offset = (offset + 1).next_multiple_of(BLOCK);
                    continue;
                }
                assert!((offset % BLOCK) + length <= BLOCK);
                let entry = &image[offset..offset + length];
                entries.push((
                    read32(&entry[2..]),
                    read32(&entry[10..]),
                    &entry[33..33 + usize::from(entry[32])],
                ));
                offset += length;
            }
            entries
        };
        let primary = file_records(pvd);
        let joliet = file_records(svd);
        assert_eq!(primary.len(), inputs.len());
        assert_eq!(joliet.len(), inputs.len());
        for (((p_start, p_len, _), (j_start, j_len, id)), (name, bytes)) in
            primary.into_iter().zip(joliet).zip(inputs)
        {
            assert_eq!((p_start, p_len), (j_start, j_len));
            assert!(
                name.encode_utf16()
                    .flat_map(u16::to_be_bytes)
                    .eq(id.iter().copied())
            );
            assert_eq!(&image[j_start * BLOCK..][..j_len], &bytes);
        }
    }

    #[test]
    fn shortens_long_names_and_rejects_collisions_or_non_basenames() {
        let long = format!("{}.docx", "文".repeat(80));
        let expected = format!("{}.docx", "文".repeat(59));
        assert_eq!(joliet_name(&long).unwrap(), expected);
        assert!(build(BTreeMap::from([(long, vec![]), (expected, vec![])])).is_err());
        assert!(
            build(BTreeMap::from([
                ("draft.pdf".into(), vec![]),
                ("DRAFT.PDF".into(), vec![]),
            ]))
            .is_err()
        );
        assert!(build(BTreeMap::from([("../稿件.pdf".into(), vec![])])).is_err());
    }
}
