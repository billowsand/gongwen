//! 公文 → tex pipeline。
//!
//! 直接拼 LaTeX 字符串，不调 pandoc。视觉与 docx_official 一致：
//! - 正文 仿宋 三号（zihao{3}）、1.5 倍行距、2em 首行缩进
//! - 标题 H1 居中 方正小标宋简体 二号；H2 黑体 + "一、"；H3 楷体 + "（一）"；
//!   H4 仿宋 + "1."；H5 仿宋粗体 + "(1)"
//! - 列表前缀循环 ①②③ → ⑴⑵⑶ → a.b.c. → I.II.III. → (A)(B) → 1)2)
//! - 表格 longtblr（tabularray）全边框：自动分析内容决定对齐与列宽，
//!   支持表题（caption）与 `{#tbl:id}` 锚点，可跨页断开、表头续页重复
//!
//! 内嵌 `official.cls`，运行时复制到输出目录，供 `xelatex` 编译用。

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::common::ast::{Block, Inline};
use crate::common::numbering::{int_to_roman, number_to_chinese, number_to_uppercase_letter};
use crate::parser;

const OFFICIAL_CLS: &str = include_str!("../resources/official/official.cls");

#[allow(dead_code)]
const CIRCLE_NUMBERS_1: &[&str] = &[
    "⑴", "⑵", "⑶", "⑷", "⑸", "⑹", "⑺", "⑻", "⑼", "⑽", "⑾", "⑿", "⒀", "⒁", "⒂", "⒃", "⒄", "⒅", "⒆",
    "⒇",
];
#[allow(dead_code)]
const CIRCLE_NUMBERS_2: &[&str] = &[
    "①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨", "⑩", "⑪", "⑫", "⑬", "⑭", "⑮", "⑯", "⑰", "⑱", "⑲",
    "⑳",
];

/// 把列表 level 映射到 paralist 环境名：
/// 1 → asparaenum（首行缩进的段落式条目）
/// 2..=6 → inparaenum（行内紧排）
fn list_env_name(level: u8) -> &'static str {
    match level {
        1 => "asparaenum",
        _ => "inparaenum",
    }
}

/// 公文 tex 入口。
///
/// 输出布局：主 tex 文件 + `data/` 下的分节部件（主文件 `\input` 引用）+
/// `figures/` 下的图片（markdown 引用的本地图片复制至此并改写路径）。
pub fn run(input: &Path, output: Option<&Path>, compile_pdf: bool) -> Result<()> {
    let kind = crate::input::classify(input)?;
    let raw = crate::input::collect_raw(input)?;
    let (metadata, content) = crate::common::front_matter::parse(&raw);
    let content = crate::input::strip_horizontal_rules(&content);
    let output_path = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| crate::input::default_output(input, "tex"));

    println!("正在转换: {}", input.display());

    let base_dir = match kind {
        crate::input::InputKind::Directory => input,
        crate::input::InputKind::File => input.parent().unwrap_or(Path::new(".")),
    };

    let mut blocks = parser::parse(&content);
    let citations =
        crate::common::citation::validate(&blocks, metadata.bibliography.as_deref(), base_dir)?;
    crate::common::crossref::check_or_bail(
        &blocks,
        crate::common::crossref::Support::FiguresAndTables,
    )?;

    let out_dir = output_path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(out_dir)
        .with_context(|| format!("创建输出目录 {} 失败", out_dir.display()))?;
    if let Some(path) = citations.copy_to(out_dir)? {
        println!("  Bib 文件已复制到 {}", path.display());
    }
    for (_, dst) in crate::common::images::relocate(&mut blocks, base_dir, out_dir) {
        println!("  图片已复制到 {}", dst.display());
    }

    let mut emitter = TexEmitter::new();
    emitter.emit_all(&blocks);

    let (body, parts) = emitter.finish();
    let tex = wrap_document(&body, citations.has_citations);

    fs::write(&output_path, tex).with_context(|| format!("写入 {} 失败", output_path.display()))?;
    crate::common::parts::write_parts(out_dir, &parts)
        .with_context(|| format!("写入分章部件到 {} 失败", out_dir.display()))?;

    let cls_dst = out_dir.join("official.cls");
    fs::write(&cls_dst, OFFICIAL_CLS)
        .with_context(|| format!("复制 official.cls 到 {} 失败", cls_dst.display()))?;
    println!("  已复制 official.cls 到 {}", cls_dst.display());

    if compile_pdf {
        crate::tex_compile::compile_pdf_if_available(&output_path)?;
    }

    println!("[完成] 转换完成: {}", output_path.display());
    Ok(())
}

fn wrap_document(body: &str, has_citations: bool) -> String {
    let mut s = String::new();
    s.push_str("\\documentclass{official}\n");
    if has_citations {
        s.push_str("\\usepackage{gbt7714}\n");
        s.push_str("\\citestyle{numbers}\n");
        s.push_str("\\renewcommand{\\refname}{参考文献}\n");
    }
    s.push('\n');
    s.push_str("\\begin{document}\n\n");
    s.push_str(body);
    if !body.ends_with('\n') {
        s.push('\n');
    }
    if has_citations {
        s.push_str("\n\\bibliography{references}\n");
    }
    s.push_str("\n\\end{document}\n");
    s
}

// ========== Emitter ==========

struct TexEmitter {
    /// 当前写入缓冲区（主文件段或当前部件段）
    out: String,
    /// 已定稿的主文件内容（含 \input 引用行）
    main: String,
    /// 已定稿的部件：(相对路径, 内容)
    parts: Vec<(String, String)>,
    /// 当前正在写入的部件路径；None 表示当前写入主文件
    current_part: Option<String>,
    /// data/sectionNN 序号
    data_idx: usize,
    h2: usize,
    h3: usize,
    h4: usize,
    h5: usize,
    l1: usize,
    l2: usize,
    l3: usize,
    l4: usize,
    l5: usize,
    l6: usize,
    in_list: bool,
    list_level: u8,
    /// 当前打开的列表环境栈：栈顶是最深层。空栈表示无环境打开。
    /// 用栈而非单一 Option 是为了支持多级列表嵌套（L1 asparaenum 内嵌 L2 inparaenum）。
    list_env: Vec<(u8, &'static str)>,
    /// 最近一次 \item 行写入 out 之后的位置；当遇到更深层级时，
    /// 在此位置插入 \begin{...} 把子环境挂在父 \item 之后。
    last_item_end: usize,
    /// 待挂接到下一个表格的交叉引用锚点（Block::Label 设置）。
    /// 公文标题无自动编号，标题锚点由 emit_heading 丢弃。
    pending_label: Option<String>,
}

impl TexEmitter {
    fn new() -> Self {
        Self {
            out: String::new(),
            main: String::new(),
            parts: Vec::new(),
            current_part: None,
            data_idx: 0,
            h2: 0,
            h3: 0,
            h4: 0,
            h5: 0,
            l1: 0,
            l2: 0,
            l3: 0,
            l4: 0,
            l5: 0,
            l6: 0,
            in_list: false,
            list_level: 0,
            list_env: Vec::new(),
            last_item_end: 0,
            pending_label: None,
        }
    }

    /// 收尾：把当前缓冲区定稿，返回 (主文件 body, 部件列表)。
    fn finish(mut self) -> (String, Vec<(String, String)>) {
        // 文件末尾可能停在列表中间，关掉任何未闭合的环境
        self.reset_list();
        self.finalize_current();
        (self.main, self.parts)
    }

    /// 当前缓冲区定稿：归入当前部件或主文件。
    fn finalize_current(&mut self) {
        let buf = std::mem::take(&mut self.out);
        match self.current_part.take() {
            Some(path) => self.parts.push((path, buf)),
            None => self.main.push_str(&buf),
        }
    }

    /// 切出一个新部件；主文件中按序留下 \input 引用。
    fn start_data_part(&mut self) {
        // 部件切换前先关掉未闭合的列表环境，保证每个部件文件自身 LaTeX 完整
        self.reset_list();
        self.finalize_current();
        self.data_idx += 1;
        let path = format!("data/section{:02}.tex", self.data_idx);
        self.main.push_str(&format!("\\input{{{}}}\n\n", path));
        self.current_part = Some(path);
    }

    fn emit_all(&mut self, blocks: &[Block]) {
        for b in blocks {
            self.emit_block(b);
        }
    }

    fn emit_block(&mut self, block: &Block) {
        match block {
            Block::Heading { level, text } => {
                self.reset_list();
                self.emit_heading(*level, text);
            }
            Block::Paragraph(inlines) => {
                self.reset_list();
                self.emit_paragraph(inlines);
            }
            Block::List {
                ordered: _,
                level,
                content,
            } => {
                self.emit_list_item(*level, content);
            }
            Block::Table { rows, caption } => {
                self.reset_list();
                self.emit_table(rows, caption.as_deref());
            }
            Block::Marker(_) => {
                // 公文路径不响应区段标记，原样忽略
            }
            Block::Label(id) => {
                // 锚点作用于紧随其后的表格（公文表格经 longtblr caption 获得
                // 自动编号，可被 {@id} 引用）；标题锚点无编号，emit_heading 丢弃
                self.pending_label = Some(id.clone());
            }
            Block::CodeBlock { .. } => {
                // 公文路径暂不支持代码块
            }
            Block::Empty => {
                // 空行：不主动产出多余空行；段落分隔已由其他 emit_* 末尾的 "\n\n" 处理
            }
        }
    }

    fn emit_heading(&mut self, level: u8, text: &str) {
        // 公文标题手工编号、无 LaTeX 计数器，标题锚点无法被引用，直接丢弃
        self.pending_label.take();
        let escaped = escape_latex(text);
        match level {
            1 => {
                self.h2 = 0;
                self.h3 = 0;
                self.h4 = 0;
                self.h5 = 0;
                self.out.push_str(&format!(
                    "\\begin{{center}}{{\\xbsong\\zihao{{2}}{}}}\\end{{center}}\n\n",
                    escaped
                ));
            }
            2 => {
                self.h2 += 1;
                self.h3 = 0;
                self.h4 = 0;
                self.h5 = 0;
                // H2（"一、"级）是公文的顶级节，每节切出 data/ 部件
                self.start_data_part();
                let num = number_to_chinese(self.h2);
                self.out
                    .push_str(&format!("{{\\hei {}、{}}}\\par\n\n", num, escaped));
            }
            3 => {
                self.h3 += 1;
                self.h4 = 0;
                self.h5 = 0;
                let num = number_to_chinese(self.h3);
                self.out
                    .push_str(&format!("{{\\kai （{}）{}}}\\par\n\n", num, escaped));
            }
            4 => {
                self.h4 += 1;
                self.h5 = 0;
                self.out
                    .push_str(&format!("{}.{}\\par\n\n", self.h4, escaped));
            }
            5 => {
                self.h5 += 1;
                self.out
                    .push_str(&format!("\\textbf{{({}){}}}\\par\n\n", self.h5, escaped));
            }
            _ => {
                // 6 级以上原 docx 也直接忽略
            }
        }
    }

    fn emit_paragraph(&mut self, inlines: &[Inline]) {
        // 独占一段的图片（允许前后有空白文本）输出为 figure 环境
        if let Some((alt, url, label)) = sole_image(inlines) {
            self.emit_figure(alt, url, label);
            return;
        }
        let body = render_inlines(inlines);
        if body.trim().is_empty() {
            return;
        }
        self.out.push_str(&body);
        self.out.push_str("\\par\n\n");
    }

    fn emit_figure(&mut self, alt: &str, url: &str, label: Option<&str>) {
        // [H]（float 宏包）：图片就地排版，不漂移到页首/页尾
        self.out.push_str("\\begin{figure}[H]\n\\centering\n");
        self.out.push_str(&format!(
            "\\includegraphics[width=\\textwidth]{{{}}}\n",
            escape_href_url(url)
        ));
        if !alt.is_empty() {
            self.out
                .push_str(&format!("\\caption{{{}}}\n", escape_latex(alt)));
            // \label 必须跟在 \caption 之后，引用的才是图号
            if let Some(lab) = label {
                self.out.push_str(&format!("\\label{{{}}}\n", lab));
            }
        }
        self.out.push_str("\\end{figure}\n\n");
    }

    fn emit_list_item(&mut self, level: u8, content: &[Inline]) {
        let body = render_inlines(content);
        let env = list_env_name(level);
        match self.list_env.last() {
            None => {
                // 当前没有打开任何列表环境
                self.out.push_str(&format!("\\begin{{{}}}\n", env));
                self.list_env.push((level, env));
            }
            Some(&(cur_level, cur_env)) if cur_level == level && cur_env == env => {
                // 同层续项
            }
            Some(&(cur_level, _)) if level > cur_level => {
                // 进入更深层级：把子环境的 \begin 插在上一个 \item 之后，
                // 这样子环境作为父 \item 的内容出现，而不是兄弟。
                let env_open = format!("\\begin{{{}}}\n", env);
                self.out.insert_str(self.last_item_end, &env_open);
                self.last_item_end += env_open.len();
                self.list_env.push((level, env));
            }
            Some(_) => {
                // 回到更浅层级：关闭当前所有比 level 更深的环境，
                // 然后从栈顶（如果还有）继续。
                while let Some(&(cur_level, cur_env)) = self.list_env.last() {
                    if cur_level >= level {
                        self.out.push_str(&format!("\\end{{{}}}\n", cur_env));
                        self.list_env.pop();
                    } else {
                        break;
                    }
                }
                if let Some(&(cur_level, cur_env)) = self.list_env.last() {
                    if cur_level == level && cur_env == env {
                        // 回到同层，继续
                    } else {
                        // 回到的层级与当前要写的不一致——关闭剩余的全部
                        while let Some(&(_, cur_env)) = self.list_env.last() {
                            self.out.push_str(&format!("\\end{{{}}}\n", cur_env));
                            self.list_env.pop();
                        }
                        self.out.push_str(&format!("\\begin{{{}}}\n", env));
                        self.list_env.push((level, env));
                    }
                } else {
                    // 栈已空，开新的
                    self.out.push_str(&format!("\\begin{{{}}}\n", env));
                    self.list_env.push((level, env));
                }
            }
        }
        // 写入当前 \item
        let item_str = format!("\\item {}\n", body);
        self.out.push_str(&item_str);
        self.last_item_end = self.out.len();
        self.in_list = true;
        self.list_level = level;
    }

    fn emit_table(&mut self, rows: &[Vec<String>], caption: Option<&str>) {
        if rows.is_empty() {
            return;
        }
        // 表题尾部锚点（Block::Label → pending_label）
        let label = self.pending_label.take();

        // 与 research 同一套智能表格：longtblr 全边框、按内容自动分配
        // X 列宽比例、窄数字列固定 2em、表头黑体居中并续页重复；
        // 单元格内 \footnote 降级为全角括号内联注释（由 emit_longtblr 处理）
        let table_latex =
            crate::common::table_to_longtblr::emit_longtblr(rows, caption, label.as_deref());
        if table_latex.is_empty() {
            return;
        }
        self.out.push_str(&table_latex);
        self.out.push_str("\n\n");
    }

    fn reset_list(&mut self) {
        // 把所有打开的列表环境按从深到浅依次关闭，写到当前 out 末尾。
        let mut closed = false;
        while let Some((_, env)) = self.list_env.pop() {
            self.out.push_str(&format!("\\end{{{}}}\n", env));
            closed = true;
        }
        // 列表环境结束后补一个空行：否则紧随其后的正文会被 LaTeX 当成
        // 同一段延续（paralist 的 asparaenum 尤其明显），首行缩进与段距全乱。
        if closed {
            self.out.push('\n');
        }
        self.in_list = false;
        self.list_level = 0;
        self.l1 = 0;
        self.l2 = 0;
        self.l3 = 0;
        self.l4 = 0;
        self.l5 = 0;
        self.l6 = 0;
    }

    /// 镜像 docx_official::Converter::get_list_prefix 的行为，确保 6 级前缀循环
    /// 在 tex 输出中和 docx 完全一致。
    ///
    /// 已不再被 emit_list_item 调用（改用 paralist 环境标签），保留以备 docx
    /// 对齐检查或未来回退使用。
    #[allow(dead_code)]
    fn list_prefix(&mut self, level: u8) -> String {
        match level {
            1 => {
                if !self.in_list || self.list_level > 1 {
                    self.l2 = 0;
                    self.l3 = 0;
                    self.l4 = 0;
                    self.l5 = 0;
                    self.l6 = 0;
                }
                if !self.in_list {
                    self.l1 = 0;
                }
                self.l1 += 1;
                if self.l1 <= CIRCLE_NUMBERS_2.len() {
                    CIRCLE_NUMBERS_2[self.l1 - 1].to_string()
                } else {
                    format!("({})", self.l1)
                }
            }
            2 => {
                if !self.in_list || self.list_level != 2 {
                    if self.list_level > 2 {
                        self.l3 = 0;
                        self.l4 = 0;
                        self.l5 = 0;
                        self.l6 = 0;
                    }
                    if !self.in_list || self.list_level < 2 {
                        self.l2 = 0;
                        self.l3 = 0;
                        self.l4 = 0;
                        self.l5 = 0;
                        self.l6 = 0;
                    }
                }
                self.l2 += 1;
                if self.l2 <= CIRCLE_NUMBERS_1.len() {
                    CIRCLE_NUMBERS_1[self.l2 - 1].to_string()
                } else {
                    format!("({})", self.l2)
                }
            }
            3 => {
                if !self.in_list || self.list_level < 3 {
                    self.l3 = 0;
                    self.l4 = 0;
                    self.l5 = 0;
                    self.l6 = 0;
                }
                self.l3 += 1;
                let ch = (b'a' + ((self.l3 - 1) % 26) as u8) as char;
                format!("{}.", ch)
            }
            4 => {
                if !self.in_list || self.list_level < 4 {
                    self.l4 = 0;
                    self.l5 = 0;
                    self.l6 = 0;
                }
                self.l4 += 1;
                format!("{}.", int_to_roman(self.l4))
            }
            5 => {
                if !self.in_list || self.list_level < 5 {
                    self.l5 = 0;
                    self.l6 = 0;
                }
                self.l5 += 1;
                format!("({})", number_to_uppercase_letter(self.l5))
            }
            6 => {
                if !self.in_list || self.list_level < 6 {
                    self.l6 = 0;
                }
                self.l6 += 1;
                format!("{})", self.l6)
            }
            _ => String::new(),
        }
    }
}

// ========== 字符串工具 ==========

/// 段落内容去掉纯空白 Text 后只剩一张图片时，返回其 (alt, url, label)。
fn sole_image(inlines: &[Inline]) -> Option<(&str, &str, Option<&str>)> {
    let meaningful: Vec<&Inline> = inlines
        .iter()
        .filter(|ip| !matches!(ip, Inline::Text(t) if t.trim().is_empty()))
        .collect();
    match meaningful.as_slice() {
        [Inline::Image { alt, url, label }] => Some((alt, url, label.as_deref())),
        _ => None,
    }
}

fn render_inlines(inlines: &[Inline]) -> String {
    let mut s = String::new();
    for ip in inlines {
        match ip {
            Inline::Text(t) => s.push_str(&escape_latex(t)),
            Inline::Bold(children) => {
                s.push_str("\\textbf{");
                s.push_str(&render_inlines(children));
                s.push('}');
            }
            Inline::Italic(children) => {
                s.push_str("\\textit{");
                s.push_str(&render_inlines(children));
                s.push('}');
            }
            Inline::Code(t) => {
                s.push_str("\\texttt{");
                s.push_str(&escape_latex(t));
                s.push('}');
            }
            Inline::Link { text, url } => {
                s.push_str("\\href{");
                s.push_str(&escape_href_url(url));
                s.push_str("}{");
                s.push_str(&escape_latex(text));
                s.push('}');
            }
            // 行内图片（未独占一段）：直接插入 \includegraphics，宽度默认 \textwidth
            Inline::Image { url, .. } => {
                s.push_str("\\includegraphics[width=\\textwidth]{");
                s.push_str(&escape_href_url(url));
                s.push('}');
            }
            // 交叉引用：公文中仅图片/表格 label 有效，章节锚点会被忽略（\ref 显示 ??）
            Inline::CrossRef(id) => {
                s.push_str("\\ref{");
                s.push_str(id);
                s.push('}');
            }
            Inline::Citation(keys) => {
                s.push_str("\\cite{");
                s.push_str(&keys.join(","));
                s.push('}');
            }
            Inline::Footnote(t) => {
                s.push_str("\\footnote{");
                s.push_str(&escape_latex(t));
                s.push('}');
            }
        }
    }
    s
}

fn escape_href_url(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\textbackslash{}"),
            '%' => out.push_str("\\%"),
            '#' => out.push_str("\\#"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            other => out.push(other),
        }
    }
    out
}

/// LaTeX 9 个特殊字符的转义 + 反斜杠。
/// 必须 char 流处理，否则 `\\` 与后续替换互相干扰。
fn escape_latex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\textbackslash{}"),
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '$' => out.push_str("\\$"),
            '#' => out.push_str("\\#"),
            '_' => out.push_str("\\_"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ast::{Block, Inline};

    /// 测试辅助：主文件与全部部件按序拼接，便于对整体内容做断言
    fn test_body(e: TexEmitter) -> String {
        let (main, parts) = e.finish();
        let mut s = main;
        for (_, content) in parts {
            s.push_str(&content);
        }
        s
    }

    #[test]
    fn escapes_special_chars() {
        assert_eq!(
            escape_latex("a&b%c$d#e_f{g}h~i^j"),
            "a\\&b\\%c\\$d\\#e\\_f\\{g\\}h\\textasciitilde{}i\\textasciicircum{}j"
        );
    }

    #[test]
    fn inline_bold_italic_emit() {
        let inlines = vec![
            Inline::Text("一段".into()),
            Inline::Bold(vec![Inline::Text("重点".into())]),
            Inline::Text("和".into()),
            Inline::Italic(vec![Inline::Text("斜体".into())]),
        ];
        assert_eq!(
            render_inlines(&inlines),
            "一段\\textbf{重点}和\\textit{斜体}"
        );
    }

    #[test]
    fn h2_uses_chinese_numbering() {
        let mut e = TexEmitter::new();
        e.emit_block(&Block::Heading {
            level: 2,
            text: "引言".into(),
        });
        e.emit_block(&Block::Heading {
            level: 2,
            text: "方法".into(),
        });
        let body = test_body(e);
        assert!(body.contains("一、引言"), "got {}", body);
        assert!(body.contains("二、方法"), "got {}", body);
    }

    #[test]
    fn list_prefix_cycles() {
        let mut e = TexEmitter::new();
        e.emit_block(&Block::List {
            ordered: false,
            level: 1,
            content: vec![Inline::Text("a".into())],
        });
        e.emit_block(&Block::List {
            ordered: false,
            level: 1,
            content: vec![Inline::Text("b".into())],
        });
        e.emit_block(&Block::List {
            ordered: false,
            level: 2,
            content: vec![Inline::Text("c".into())],
        });
        let (body, _parts) = e.finish();
        // L1 两项应被包在 asparaenum 里
        assert!(
            body.contains("\\begin{asparaenum}") && body.contains("\\end{asparaenum}"),
            "expected asparaenum wrapper, got: {}",
            body
        );
        assert!(body.contains("\\item a"), "missing L1 item a: {}", body);
        assert!(body.contains("\\item b"), "missing L1 item b: {}", body);
        // L2 单独一项应被包在 inparaenum 里
        assert!(
            body.contains("\\begin{inparaenum}") && body.contains("\\end{inparaenum}"),
            "expected inparaenum wrapper, got: {}",
            body
        );
        assert!(body.contains("\\item c"), "missing L2 item c: {}", body);
    }

    #[test]
    fn list_env_closes_on_non_list_block() {
        let mut e = TexEmitter::new();
        e.emit_block(&Block::List {
            ordered: false,
            level: 1,
            content: vec![Inline::Text("第一项".into())],
        });
        e.emit_block(&Block::Heading {
            level: 2,
            text: "分隔标题".into(),
        });
        e.emit_block(&Block::List {
            ordered: false,
            level: 1,
            content: vec![Inline::Text("第二项".into())],
        });
        let (main, parts) = e.finish();
        // 列表项会被部件切分到 data/sectionNN.tex，扫描主文件 + 所有部件
        let mut all = main.clone();
        for (_path, body) in &parts {
            all.push('\n');
            all.push_str(body);
        }
        let begins = all.matches("\\begin{asparaenum}").count();
        let ends = all.matches("\\end{asparaenum}").count();
        assert_eq!(begins, 2, "expected 2 asparaenum opens, got: {}", all);
        assert_eq!(ends, 2, "expected 2 asparaenum closes, got: {}", all);
    }

    #[test]
    fn list_env_close_followed_by_blank_line() {
        // 列表结束后必须空一行，否则后续正文会并入列表最后一项所在段落
        let mut e = TexEmitter::new();
        e.emit_block(&Block::List {
            ordered: false,
            level: 1,
            content: vec![Inline::Text("条目".into())],
        });
        e.emit_block(&Block::Paragraph(vec![Inline::Text("后续正文".into())]));
        let body = test_body(e);
        assert!(
            body.contains("\\end{asparaenum}\n\n后续正文"),
            "列表环境与后续正文之间缺少空行：\n{}",
            body
        );
    }

    #[test]
    fn nested_list_renders_inner_env_inside_outer_item() {
        // - 新的第一级
        //   - 新的第二级
        // 应得到：父 asparaenum 内嵌子 inparaenum，而不是拆成两个并列环境
        let mut e = TexEmitter::new();
        e.emit_block(&Block::List {
            ordered: false,
            level: 1,
            content: vec![Inline::Text("新的第一级".into())],
        });
        e.emit_block(&Block::List {
            ordered: false,
            level: 2,
            content: vec![Inline::Text("新的第二级".into())],
        });
        let (body, _parts) = e.finish();
        // 子环境必须出现在父 \item 之后、父 \end 之间
        let begin_outer = body.find("\\begin{asparaenum}").expect("outer begin");
        let end_outer = body.find("\\end{asparaenum}").expect("outer end");
        let begin_inner = body.find("\\begin{inparaenum}").expect("inner begin");
        let item_parent = body.find("\\item 新的第一级").expect("parent item");
        let item_child = body.find("\\item 新的第二级").expect("child item");
        let end_inner = body.find("\\end{inparaenum}").expect("inner end");
        assert!(
            begin_outer < item_parent
                && item_parent < begin_inner
                && begin_inner < item_child
                && item_child < end_inner
                && end_inner < end_outer,
            "子环境应嵌套在父环境内，实际顺序：\n{}",
            body
        );
    }

    #[test]
    fn table_emits_longtblr() {
        let mut e = TexEmitter::new();
        e.emit_block(&Block::Table {
            rows: vec![
                vec!["列A".into(), "列B".into()],
                vec!["1".into(), "2".into()],
            ],
            caption: None,
        });
        let body = test_body(e);
        assert!(body.contains("\\begin{longtblr}"), "got {}", body);
        // 表头黑体
        assert!(body.contains("\\heiti 列A"), "got {}", body);
        // 全框线 + 表头续页重复
        assert!(body.contains("hlines"), "got {}", body);
        assert!(body.contains("vlines"), "got {}", body);
        assert!(body.contains("rowhead = 1"), "got {}", body);
        assert!(body.contains("\\end{longtblr}"), "got {}", body);
    }

    #[test]
    fn table_caption_passed_to_longtblr() {
        let mut e = TexEmitter::new();
        e.emit_block(&Block::Table {
            rows: vec![
                vec!["列A".into(), "列B".into()],
                vec!["1".into(), "2".into()],
            ],
            caption: Some("产品清单".into()),
        });
        let body = test_body(e);
        assert!(body.contains("caption={产品清单}"), "got {}", body);
    }

    #[test]
    fn table_label_passed_to_longtblr() {
        let mut e = TexEmitter::new();
        e.emit_block(&Block::Label("tbl:products".into()));
        e.emit_block(&Block::Table {
            rows: vec![
                vec!["列A".into(), "列B".into()],
                vec!["1".into(), "2".into()],
            ],
            caption: Some("产品清单".into()),
        });
        let body = test_body(e);
        assert!(body.contains("label={tbl:products}"), "got {}", body);
    }

    #[test]
    fn heading_discards_pending_label() {
        // 公文标题无自动编号：标题前的锚点必须丢弃，不得漂移到后续表格上
        let mut e = TexEmitter::new();
        e.emit_block(&Block::Label("sec:a".into()));
        e.emit_block(&Block::Heading {
            level: 3,
            text: "背景".into(),
        });
        e.emit_block(&Block::Table {
            rows: vec![
                vec!["列A".into(), "列B".into()],
                vec!["1".into(), "2".into()],
            ],
            caption: Some("产品清单".into()),
        });
        let body = test_body(e);
        assert!(!body.contains("label="), "got {}", body);
    }

    #[test]
    fn table_smart_colspec_narrow_numeric_column() {
        // 序号列（表体全是 2 位以内纯数字）固定 2em，其余列按内容比例分配
        let mut e = TexEmitter::new();
        e.emit_block(&Block::Table {
            rows: vec![
                vec!["序号".into(), "说明".into()],
                vec!["1".into(), "第一条说明文字，较长。".into()],
                vec!["12".into(), "第二条说明文字，同样较长。".into()],
            ],
            caption: None,
        });
        let body = test_body(e);
        assert!(body.contains("Q[c,wd=2em]"), "got {}", body);
        assert!(body.contains("X["), "got {}", body);
    }

    #[test]
    fn table_cell_bold_italic() {
        let mut e = TexEmitter::new();
        e.emit_block(&Block::Table {
            rows: vec![
                vec!["列A".into(), "列B".into()],
                vec!["**重点**".into(), "*斜体*".into()],
            ],
            caption: None,
        });
        let body = test_body(e);
        assert!(body.contains("\\textbf{重点}"));
        assert!(body.contains("\\textit{斜体}"));
        assert!(!body.contains("**重点**"));
    }

    #[test]
    fn standalone_image_emits_figure_with_textwidth() {
        let mut e = TexEmitter::new();
        e.emit_block(&Block::Paragraph(vec![Inline::Image {
            alt: "系统架构".into(),
            url: "figs/arch.png".into(),
            label: Some("fig:arch".into()),
        }]));
        let body = test_body(e);
        assert!(body.contains("\\begin{figure}[H]"), "got {}", body);
        assert!(body.contains("\\centering"));
        assert!(
            body.contains("\\includegraphics[width=\\textwidth]{figs/arch.png}"),
            "got {}",
            body
        );
        assert!(body.contains("\\caption{系统架构}"));
        // \label 必须跟在 \caption 之后
        let cap = body.find("\\caption{系统架构}").expect("caption");
        let lab = body.find("\\label{fig:arch}").expect("label");
        assert!(cap < lab, "got {}", body);
        assert!(body.contains("\\end{figure}"));
    }

    #[test]
    fn image_without_alt_omits_caption() {
        let mut e = TexEmitter::new();
        e.emit_block(&Block::Paragraph(vec![Inline::Image {
            alt: String::new(),
            url: "a.png".into(),
            label: None,
        }]));
        let body = test_body(e);
        assert!(body.contains("\\includegraphics[width=\\textwidth]{a.png}"));
        assert!(!body.contains("\\caption"));
    }

    #[test]
    fn inline_image_stays_inline() {
        let rendered = render_inlines(&[
            Inline::Text("见".into()),
            Inline::Image {
                alt: "图".into(),
                url: "a.png".into(),
                label: None,
            },
        ]);
        assert_eq!(rendered, "见\\includegraphics[width=\\textwidth]{a.png}");
    }

    #[test]
    fn crossref_renders_ref() {
        let rendered = render_inlines(&[
            Inline::Text("见图".into()),
            Inline::CrossRef("fig:arch".into()),
        ]);
        assert_eq!(rendered, "见图\\ref{fig:arch}");
    }

    #[test]
    fn citation_renders_as_latex_cite() {
        let rendered = render_inlines(&[Inline::Citation(vec!["a".into(), "b".into()])]);
        assert_eq!(rendered, "\\cite{a,b}");
    }

    #[test]
    fn wrapper_adds_bibliography_only_for_citations() {
        let cited = wrap_document("正文\\cite{a}\n", true);
        assert!(cited.contains("\\usepackage{gbt7714}"));
        assert!(cited.contains("\\citestyle{numbers}"));
        assert!(cited.contains("\\bibliography{references}"));
        assert!(!cited.contains("\\nocite"));

        let plain = wrap_document("正文\n", false);
        assert!(!plain.contains("gbt7714"));
        assert!(!plain.contains("bibliography{references}"));
    }

    #[test]
    fn official_class_prefers_jetbrains_mono_and_fzkai_for_code() {
        assert!(OFFICIAL_CLS.contains("\\setmonofont{JetBrains Mono}"));
        assert!(OFFICIAL_CLS.contains("\\setCJKmonofont{FZKai-Z03}"));
    }

    #[test]
    fn validation_failure_does_not_create_official_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("paper.md");
        let output = dir.path().join("out/report.tex");
        fs::write(&input, "---\nbibliography: missing.bib\n---\n正文 [@a]\n").unwrap();

        let err = run(&input, Some(&output), false).unwrap_err();

        assert!(err.to_string().contains("missing.bib"));
        assert!(!output.exists());
        assert!(!output.parent().unwrap().exists());
    }

    #[test]
    fn h2_sections_split_into_data_parts() {
        let mut e = TexEmitter::new();
        e.emit_block(&Block::Heading {
            level: 1,
            text: "标题".into(),
        });
        e.emit_block(&Block::Heading {
            level: 2,
            text: "引言".into(),
        });
        e.emit_block(&Block::Paragraph(vec![Inline::Text("引言正文".into())]));
        e.emit_block(&Block::Heading {
            level: 2,
            text: "方法".into(),
        });
        let (main, parts) = e.finish();

        // 主文件：标题保留，两节按序 \input 引用
        assert!(main.contains("标题"), "got {main}");
        assert!(main.contains("\\input{data/section01.tex}"));
        assert!(main.contains("\\input{data/section02.tex}"));
        assert!(!main.contains("一、引言"), "got {main}");

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0, "data/section01.tex");
        assert!(parts[0].1.contains("一、引言"));
        assert!(parts[0].1.contains("引言正文"));
        assert_eq!(parts[1].0, "data/section02.tex");
        assert!(parts[1].1.contains("二、方法"));
    }
}
