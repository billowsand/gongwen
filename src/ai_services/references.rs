//! `@` 引用的一篇文章：稿件库或知识库里的文档（`Board::refs` 与风格档案都用它）。
//!
//! 从旧 `agent::board` 搬来，字段与序列化形态（`source` / `id` / `title`，枚举原名）不变，
//! 旧 `Board` 单向 re-export 这里的类型。

/// `@` 引用的一篇文章：稿件库或知识库里的文档。正文由引擎在后台线程读。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Reference {
    pub(crate) source: RefSource,
    pub(crate) id: i64,
    pub(crate) title: String,
}

/// 引用来自哪个库。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum RefSource {
    Manuscript,
    Knowledge,
}

impl RefSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            RefSource::Manuscript => "稿件库",
            RefSource::Knowledge => "知识库",
        }
    }
}
