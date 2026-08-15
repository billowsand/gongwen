#!/usr/bin/env python3
"""splitter.py 的配置模板 —— 复制为 config_<module>.py 并按需修改后运行：

    python3 scripts/splitter.py config_<module>.py

本模板字段齐全，包含全部可选配置的说明。配置由 splitter.py 以 exec 方式加载，
因此文件顶部不要放多余的可执行代码。"""
# 需要拆分的源文件（拆分后仍作为模块根保留）
SRC = "src/xxx.rs"
# 子模块输出目录（自动创建；SRC 保留为模块根，二者可并存）
OUT_DIR = "src/xxx"
# 模块在 crate 中的路径，决定子模块导入根类型时的前缀，如：
#   "app" / "draft_page" / "preview" / "export::docx" / "export"
MODULE = "xxx"

# 源文件头部（模块文档 + use 块）的 0 基行区间，从模块根中剥离、
# 由脚本按需重建。末尾应停在第一个条目之前（保留它的文档注释）。
HEADER_STRIP = range(0, 40)

# 需要无条件从根删除的 0 基行（例如被整体移走的 impl 块的收尾 `}`）。
REMOVE_LINES = set()

# 若源码里有一个巨大的 impl 块（如 `impl GongwenApp` / `impl DraftPage<'_>`），
# 把其中的方法分发到各子文件、由每个文件自行包裹 impl 块：
#   IMPL_WRAP = (起始1基行, 结束1基行)  范围内的缩进方法会被包进 IMPL_WRAP_SIG
# 没有大 impl 块时设 (0, 0)。
IMPL_WRAP = (0, 0)
IMPL_WRAP_SIG = "impl X<'_>"

# `use super::{...}` 相对导入解析到哪个模块（例如 docx.rs 的 super 是 export）。
# 没有则留空字典。
SUPER_MAP = {"super": "crate::export"}

# 每个子文件额外追加的 import 行；"*" 对所有子文件生效，键名对单个文件生效，
# "root" 对模块根生效（例如 docx 系需要 `use docx_rs::*;`）。
PER_FILE_EXTRA = {
    "*": ["use docx_rs::*;"],
}

# 插在根文件模块文档之后、import 之前的代码行（例如保留原有的 mod 声明）。
ROOT_PREAMBLE = []

# 根文件的模块文档（//! 行）。
ROOT_DOC = [
    "//! 模块说明……",
]

# 1 基行号 -> 目标子文件名（不含 .rs）。未列出的条目保留在根。
# 方法行也要列出（缩进的 `    fn`）；被整块移动的小 impl（如 `impl Foo`）连同
# 其方法行一起列给同一文件，脚本会把 impl 收尾 `}` 一并带过去。
# 特殊值 "REMOVE"：从根删除且不进任何子文件（如被各文件自行包裹的大 impl 头）。
TARGETS = {
    1: "a", 2: "a",
    3: "b",
    # 999: "REMOVE",
}

# 子文件可能从 `crate::MODULE::` 导入的名字（根保留项 + 全部再导出项）。
# 脚本按文件名出现与否自动挑选；同文件内已定义的名字会被排除。
ROOT_ITEMS = [
    "root_kept_item",
    "reexported_a",
    "reexported_b",
]

# 插在根文件 mod 声明之后的再导出。测试专用名字务必用 `#[cfg(test)]` 标注
# （非测试构建会因 unused 报错）。"*" 不适合这里 —— 用显式列表更稳。
RE_EXPORTS = """pub(crate) use a::reexported_a;
#[cfg(test)]
pub(crate) use b::reexported_b;"""

# 子模块名（与 TARGETS 值一致，排序即文件生成顺序）。
MODS = ["a", "b"]

# 各子文件的一行模块说明。
DOCS = {
    "a": "……",
    "b": "……",
}

# 子文件内需要提升可见性的行（原样匹配，逐条替换为前面加 pub(crate)）。
# 典型场景：被 pub(crate) 方法签名引用的私有类型 / 被兄弟模块构造的私有字段。
PUB_UPGRADES = {
    "a": ["struct PrivateType {", "    field: String,"],
}

# 根文件内需要提升可见性的行（同上，作用于保留在根的内容）。
ROOT_PUB_UPGRADES = ["struct Metrics {"]

# ---------------------------------------------------------------------------
# 可选进阶（按需打开）：
#
# 1. 手工清理脚本（cleanup_*.py）：拆分生成后，unused-import 类警告按
#    "warning: ... --> 文件:行" 逐条处理，惯用模式是把仅测试用到的名字移进
#    mod tests 的 use、把测试专用再导出打上 #[cfg(test)]、删除纯内部名字。
#
# 2. 提交前验证清单：cargo check（0 错误 0 警告）、cargo clippy、cargo build、
#    cargo test（结果须与拆分前一致）。每拆一个文件单独提交一次。
