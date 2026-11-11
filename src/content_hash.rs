/// 正文内容哈希：去掉所有空白后取 64 位 FNV-1a，十六进制。
///
/// 只为“同一篇内容是否已入库”服务，不做密码学用途。去空白是因为同一篇公文
/// 走稿件库和走 md 文件两条路进来时，缩进和换行常有细微差别。
pub fn content_hash(markdown: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in markdown.bytes().filter(|b| !b.is_ascii_whitespace()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
