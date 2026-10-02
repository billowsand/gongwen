//! 在内存里收窄一支字体的字符覆盖：只改 cmap，字形与度量原样保留。
//!
//! 用在纸面预览上：研究报告的 PDF 里 TeX Gyre Termes 带 `covers: "latin-in-cjk"`，
//! 引号、破折号、省略号这些中西共用的符号归中文字体。egui 的字体族只会逐字回退、
//! 没有覆盖范围的说法，Termes 排在中文字体前面就会把这些符号抢走。把这几个码位从
//! cmap 里拿掉，预览与 PDF 的取字就一致了。

use std::collections::BTreeMap;

/// Typst `covers: "latin-in-cjk"` 让给中文字体的码位（typst-library
/// `text::Covers::LatinInCjk`）：间隔号、连接号、破折号、弯引号、省略号等。
pub fn is_shared_with_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{00B7}'
            | '\u{2013}'
            | '\u{2014}'
            | '\u{2018}'
            | '\u{2019}'
            | '\u{201C}'
            | '\u{201D}'
            | '\u{2025}'..='\u{2027}' | '\u{2E3A}'
    )
}

/// 只保留 `keep` 认可的码位，返回改写后的字体文件。cmap 换成单个 (3,10) format 12
/// 子表，其余表逐字节照抄。字体集合（TTC）与解析不了的文件返回 `None`。
pub fn restrict(data: &[u8], keep: impl Fn(char) -> bool) -> Option<Vec<u8>> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let mut map: BTreeMap<u32, u16> = BTreeMap::new();
    for subtable in face.tables().cmap?.subtables {
        if !subtable.is_unicode() {
            continue;
        }
        subtable.codepoints(|cp| {
            if let Some(ch) = char::from_u32(cp)
                && keep(ch)
                && let Some(glyph) = subtable.glyph_index(cp)
            {
                map.entry(cp).or_insert(glyph.0);
            }
        });
    }
    let cmap = format12_cmap(&map);

    let read_u16 = |at: usize| -> Option<u16> {
        Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
    };
    let read_u32 = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
    };
    let version = read_u32(0)?;
    if version == u32::from_be_bytes(*b"ttcf") {
        return None;
    }
    let count = usize::from(read_u16(4)?);
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::with_capacity(count);
    for index in 0..count {
        let record = 12 + index * 16;
        let tag: [u8; 4] = data.get(record..record + 4)?.try_into().ok()?;
        let offset = read_u32(record + 8)? as usize;
        let length = read_u32(record + 12)? as usize;
        let body = if &tag == b"cmap" {
            cmap.clone()
        } else {
            data.get(offset..offset.checked_add(length)?)?.to_vec()
        };
        tables.push((tag, body));
    }
    tables.sort_by_key(|(tag, _)| *tag);
    Some(assemble(version, &mut tables))
}

/// 码位 → 字形号，按「码位与字形号都连续」分组写成 format 12。
fn format12_cmap(map: &BTreeMap<u32, u16>) -> Vec<u8> {
    let mut groups: Vec<(u32, u32, u32)> = Vec::new();
    for (&cp, &glyph) in map {
        let glyph = u32::from(glyph);
        if let Some(last) = groups.last_mut()
            && last.1 + 1 == cp
            && last.2 + (cp - last.0) == glyph
        {
            last.1 = cp;
            continue;
        }
        groups.push((cp, cp, glyph));
    }
    let mut out = Vec::with_capacity(28 + groups.len() * 12);
    out.extend_from_slice(&0u16.to_be_bytes()); // version
    out.extend_from_slice(&1u16.to_be_bytes()); // numTables
    out.extend_from_slice(&3u16.to_be_bytes()); // platformID: Windows
    out.extend_from_slice(&10u16.to_be_bytes()); // encodingID: Unicode full
    out.extend_from_slice(&12u32.to_be_bytes()); // 子表偏移
    out.extend_from_slice(&12u16.to_be_bytes()); // format
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved
    out.extend_from_slice(&(16 + groups.len() as u32 * 12).to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // language
    out.extend_from_slice(&(groups.len() as u32).to_be_bytes());
    for (start, end, glyph) in groups {
        out.extend_from_slice(&start.to_be_bytes());
        out.extend_from_slice(&end.to_be_bytes());
        out.extend_from_slice(&glyph.to_be_bytes());
    }
    out
}

fn checksum(bytes: &[u8]) -> u32 {
    bytes.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// 按表目录重新拼出 sfnt 文件；各表 4 字节对齐，校验和与 head 的整体校验一并重算。
fn assemble(version: u32, tables: &mut [([u8; 4], Vec<u8>)]) -> Vec<u8> {
    // head.checkSumAdjustment 先归零，整体校验算完再填。
    for (tag, body) in tables.iter_mut() {
        if tag == b"head" && body.len() >= 12 {
            body[8..12].fill(0);
        }
    }
    let count = tables.len() as u16;
    let mut power = 1u16;
    let mut selector = 0u16;
    while power * 2 <= count {
        power *= 2;
        selector += 1;
    }
    let search_range = power * 16;
    let mut out = Vec::new();
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&selector.to_be_bytes());
    out.extend_from_slice(&(count * 16 - search_range).to_be_bytes());
    let mut offset = 12 + tables.len() * 16;
    let mut head_at = None;
    for (tag, body) in tables.iter() {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(body).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        if tag == b"head" {
            head_at = Some(offset);
        }
        offset += body.len().next_multiple_of(4);
    }
    for (_, body) in tables.iter() {
        out.extend_from_slice(body);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    if let Some(at) = head_at
        && out.len() >= at + 12
    {
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[at + 8..at + 12].copy_from_slice(&adjustment.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termes_gives_shared_punctuation_back_to_cjk() {
        let Some(dir) = crate::portable_runtime::find_font_dir() else {
            return; // 精简检出没有 runtime 字体，跳过。
        };
        let Ok(data) = std::fs::read(dir.join("texgyretermes-regular.otf")) else {
            return;
        };
        let original = ttf_parser::Face::parse(&data, 0).expect("Termes 应能解析");
        let restricted = restrict(&data, |ch| !is_shared_with_cjk(ch)).expect("应能改写 cmap");
        let face = ttf_parser::Face::parse(&restricted, 0).expect("改写后的字体应能解析");
        for ch in "“”‘’—–…·".chars() {
            assert!(original.glyph_index(ch).is_some(), "Termes 本来有 {ch}");
            assert!(face.glyph_index(ch).is_none(), "{ch} 应让给中文字体");
        }
        for ch in "Az09%é(".chars() {
            assert_eq!(face.glyph_index(ch), original.glyph_index(ch), "{ch}");
        }
        assert_eq!(face.units_per_em(), original.units_per_em());
        assert_eq!(face.number_of_glyphs(), original.number_of_glyphs());
        let a = original.glyph_index('A').unwrap();
        assert_eq!(face.glyph_hor_advance(a), original.glyph_hor_advance(a));
    }
}
