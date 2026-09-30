//! 不触碰数据库和同步协议，只验证合并纯函数对中文与不同换行的保守性质。

use super::*;
use proptest::prelude::*;

fn document() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec![
            '甲', '乙', '中', 'A', ' ', '\t', '\r', '\n', '#', '*', '|', '$', '😀',
        ]),
        0..120,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

fn clean_text(chunks: Vec<MarkdownChunk>) -> String {
    chunks
        .into_iter()
        .map(|chunk| match chunk {
            MarkdownChunk::Text(text) => text,
            MarkdownChunk::Conflict { .. } => panic!("应当是无冲突合并"),
        })
        .collect()
}

proptest! {
    #[test]
    fn unchanged_side_preserves_the_other_side_exactly(base in document(), changed in document()) {
        prop_assert_eq!(&clean_text(merge_markdown(&base, &base, &changed)), &changed);
        prop_assert_eq!(&clean_text(merge_markdown(&base, &changed, &base)), &changed);
        prop_assert_eq!(clean_text(merge_markdown(&base, &changed, &changed)), changed);
    }

    #[test]
    fn edits_to_separated_lines_preserve_both_sides_and_their_order(
        left in "[甲乙中A0-9 *]{1,30}", right in "[丙丁文B0-9 *]{1,30}",
        crlf in any::<bool>(), final_newline in any::<bool>(),
    ) {
        let newline = if crlf { "\r\n" } else { "\n" };
        let ending = if final_newline { newline } else { "" };
        let base = format!("甲基线{newline}保留间隔{newline}乙基线{ending}");
        let local = format!("左改{left}{newline}保留间隔{newline}乙基线{ending}");
        let incoming = format!("甲基线{newline}保留间隔{newline}右改{right}{ending}");
        let expected = format!("左改{left}{newline}保留间隔{newline}右改{right}{ending}");
        prop_assert_eq!(&clean_text(merge_markdown(&base, &local, &incoming)), &expected);
        prop_assert_eq!(clean_text(merge_markdown(&base, &incoming, &local)), expected);
    }
}
