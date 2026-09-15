//! 研究报告 → tex pipeline（纯 Rust 实现，不依赖 pandoc）。
//!
//! 与 tex_official 的主要差异：
//! - 文档类：ctexbook + md2tex.cls
//! - 标题格式：H2 → 第X章，H3 → X.Y，H4 → X.Y.Z，H5 → subsubsection
//! - 列表前缀：1. 2. 3. / (1) (2) / a. b. / I. II. / (A) (B) / 1) 2)
//! - 表格：使用 longtblr 环境
//! - 支持特殊章节标记：Abstract / Appendix / Changelog / Body
//! - 分章输出：每章切为 data/ 部件，附录切为 appendix/ 部件，主文件 \input 引用

use crate::common::ast::{Block, Inline, MarkerKind};
use crate::common::table_to_longtblr::emit_longtblr;

/// 文档模式：控制特殊章节的处理方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionMode {
    Normal,    // 普通正文模式
    Abstract,  // 摘要模式：收集内容，包装 \begin{abstract}
    Appendix,  // 附录模式
    Changelog, // 版本变更记录：不编号
    Reference, // 参考文献：不编号
}

/// 研究报告 tex emitter
///
/// 分章输出：普通模式下每个 \chapter（H1/H2）切出一个 `data/chapterNN.tex`
/// 部件，附录模式下每个 \chapter 切出 `appendix/appendixNN.tex` 部件；
/// 摘要、版本变更记录、参考文献等前置/后置内容留在主文件，主文件按出现
/// 顺序用 `\input{...}` 引用各部件。`finish()` 返回 (主文件 body, 部件列表)。
pub struct TexResearchEmitter {
    /// 当前写入缓冲区（主文件段或当前部件段）
    out: String,
    /// 已定稿的主文件内容（含 \input 引用行）
    main: String,
    /// 已定稿的部件：(相对路径, 内容)
    parts: Vec<(String, String)>,
    /// 当前正在写入的部件路径；None 表示当前写入主文件
    current_part: Option<String>,
    /// data/chapterNN 序号
    data_idx: usize,
    /// appendix/appendixNN 序号
    appendix_file_idx: usize,
    /// 待挂接到下一个标题/表格的交叉引用锚点（Block::Label 设置）
    pending_label: Option<String>,
    /// 摘要收集（摘要模式下收集的内容）
    abstract_content: Vec<String>,
    /// 当前是否在摘要模式
    in_abstract: bool,
    /// 当前章节模式
    mode: SectionMode,
    /// 章节计数器
    chapter_num: usize,
    /// 小节计数器
    section_num: usize,
    /// 子小节计数器
    subsection_num: usize,
    /// 附录索引
    appendix_idx: usize,
    /// 是否已经输出过 \appendix
    has_emitted_appendix: bool,
    /// 附录段内是否已用 H1 开章（是则 H2 起整体下移一级：H2→section）
    appendix_saw_h1: bool,
    /// 摘要模式下是否已经跳过第一个标题
    abstract_skipped_heading: bool,
    /// 版本变更记录模式下是否已经输出标题
    changelog_heading_done: bool,
    /// 参考文献模式下是否已经输出标题
    reference_heading_done: bool,
    /// 列表状态
    l1: usize,
    l2: usize,
    l3: usize,
    l4: usize,
    l5: usize,
    l6: usize,
    in_list: bool,
    list_level: u8,
    /// 当前打开的列表环境栈：栈顶是最深层。用栈而非 Option 是为了支持
    /// 多级列表嵌套（L1 asparaenum 内嵌 L2 inparaenum）。
    list_env: Vec<(u8, &'static str)>,
    /// 最近一次 \item 行写入 out 之后的位置；遇到更深层级时，
    /// 在此位置插入 \begin{...} 把子环境挂在父 \item 之后。
    last_item_end: usize,
}

impl TexResearchEmitter {
    pub fn new() -> Self {
        Self {
            out: String::new(),
            main: String::new(),
            parts: Vec::new(),
            current_part: None,
            data_idx: 0,
            appendix_file_idx: 0,
            pending_label: None,
            abstract_content: Vec::new(),
            in_abstract: false,
            mode: SectionMode::Normal,
            chapter_num: 0,
            section_num: 0,
            subsection_num: 0,
            appendix_idx: 0,
            has_emitted_appendix: false,
            appendix_saw_h1: false,
            abstract_skipped_heading: false,
            changelog_heading_done: false,
            reference_heading_done: false,
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
        }
    }

    /// 收尾：把当前缓冲区定稿，返回 (主文件 body, 部件列表)。
    pub fn finish(mut self) -> (String, Vec<(String, String)>) {
        // 文件末尾可能停在列表中间，关掉未闭合的环境
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

    /// 切回主文件（区段标记等内容始终落在主文件）。
    fn start_main(&mut self) {
        if self.current_part.is_some() {
            // 切回主文件前先关闭未闭合的列表环境，避免 LaTeX 环境跨文件
            self.reset_list();
            self.finalize_current();
        }
    }

    /// 切出一个新部件；主文件中按序留下 \input 引用。
    fn start_part(&mut self, path: String) {
        self.reset_list();
        self.finalize_current();
        self.main.push_str(&format!("\\input{{{}}}\n\n", path));
        self.current_part = Some(path);
    }

    fn start_data_part(&mut self) {
        self.data_idx += 1;
        self.start_part(format!("data/chapter{:02}.tex", self.data_idx));
    }

    fn start_appendix_part(&mut self) {
        self.appendix_file_idx += 1;
        self.start_part(format!(
            "appendix/appendix{:02}.tex",
            self.appendix_file_idx
        ));
    }

    pub fn emit_all(&mut self, blocks: &[Block]) {
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
            Block::Marker(kind) => {
                self.handle_marker(kind);
            }
            Block::Label(id) => {
                // 锚点作用于紧随其后的标题/表格块
                self.pending_label = Some(id.clone());
            }
            Block::CodeBlock { lang, content } => {
                self.emit_code_block(lang, content);
            }
            Block::Empty => {
                // 忽略空行
            }
        }
    }

    fn handle_marker(&mut self, kind: &MarkerKind) {
        // 区段标记产生的内容（\appendix、摘要、参考文献等）始终落在主文件
        self.start_main();
        // 区段切换必然打断列表——关闭当前未闭合的列表环境，
        // 否则 \appendix / \chapter* 等会出现在 \begin{asparaenum} 内部
        self.reset_list();
        match kind {
            MarkerKind::Abstract => {
                self.mode = SectionMode::Abstract;
                self.in_abstract = true;
                self.abstract_skipped_heading = false;
            }
            MarkerKind::Appendix => {
                // 如果之前在摘要模式，先完成摘要
                if self.in_abstract {
                    self.finish_abstract();
                }
                self.mode = SectionMode::Appendix;
                if !self.has_emitted_appendix {
                    self.out.push_str(
                        "\\appendix

",
                    );
                    self.has_emitted_appendix = true;
                }
                self.appendix_idx = 0;
                self.appendix_saw_h1 = false;
            }
            MarkerKind::Changelog => {
                // 如果之前在摘要模式，先完成摘要
                if self.in_abstract {
                    self.finish_abstract();
                }
                self.mode = SectionMode::Changelog;
                self.changelog_heading_done = false;
                self.reset_counters();
            }
            MarkerKind::Reference => {
                // 如果之前在摘要模式，先完成摘要
                if self.in_abstract {
                    self.finish_abstract();
                }
                self.mode = SectionMode::Reference;
                // 章节标题由第一个 H1 通过 emit_heading 输出（与 Changelog 一致），
                // 避免这里再 emit 一份导致重复
                self.reset_counters();
            }
            MarkerKind::Body => {
                // 如果之前在摘要模式，先完成摘要
                if self.in_abstract {
                    self.finish_abstract();
                }
                self.mode = SectionMode::Normal;
                self.reset_counters();
            }
        }
    }

    fn reset_counters(&mut self) {
        self.chapter_num = 0;
        self.section_num = 0;
        self.subsection_num = 0;
    }

    fn emit_heading(&mut self, level: u8, text: &str) {
        let escaped = escape_latex(text);
        // 待挂接的锚点；摘要/变更记录/参考文献等不编号标题直接丢弃
        let label_cmd = self
            .pending_label
            .take()
            .map(|l| format!("\\label{{{}}}", l))
            .unwrap_or_default();

        if self.in_abstract {
            if !self.abstract_skipped_heading {
                self.abstract_skipped_heading = true;
            } else {
                self.abstract_content
                    .push(format!("\\textbf{{{}}}", escaped));
            }
            return;
        }

        if self.mode == SectionMode::Changelog {
            if !self.changelog_heading_done {
                self.out.push_str(&format!("\\chapter*{{{}}}\n\n", escaped));
                self.changelog_heading_done = true;
            } else {
                self.out.push_str(&format!("\\section*{{{}}}\n\n", escaped));
            }
            return;
        }

        if self.mode == SectionMode::Reference {
            if !self.reference_heading_done {
                self.out.push_str(&format!("\\chapter*{{{}}}\n\n", escaped));
                self.reference_heading_done = true;
            } else {
                self.out.push_str(&format!("\\section*{{{}}}\n\n", escaped));
            }
            return;
        }

        if self.mode == SectionMode::Appendix {
            // 附录模式：\appendix 后 LaTeX 自动生成 "附录A / B / ..." 编号。
            // 两种写法兼容：
            // - 附录以 H1 开章（"# 附录A ..."）：H1→chapter，H2 起下移一级；
            // - 仅用 <!-- [附录] --> 标记：H2→chapter，H3→section（沿用旧约定）。
            if level == 1 {
                self.appendix_saw_h1 = true;
            }
            let shifted = self.appendix_saw_h1;
            match (level, shifted) {
                (1, _) | (2, false) => {
                    self.appendix_idx += 1;
                    self.start_appendix_part();
                    self.out
                        .push_str(&format!("\\chapter{{{}}}{}\\par\n\n", escaped, label_cmd));
                }
                (2, true) | (3, false) => {
                    self.out
                        .push_str(&format!("\\section{{{}}}{}\\par\n\n", escaped, label_cmd));
                }
                (3, true) | (4, false) => {
                    self.out.push_str(&format!(
                        "\\subsection{{{}}}{}\\par\n\n",
                        escaped, label_cmd
                    ));
                }
                _ => {
                    self.out.push_str(&format!(
                        "\\subsubsection{{{}}}{}\\par\n\n",
                        escaped, label_cmd
                    ));
                }
            }
            return;
        }

        match level {
            // level 1/2 → \chapter，每章切出 data/ 部件
            1 | 2 => {
                self.chapter_num += 1;
                self.section_num = 0;
                self.subsection_num = 0;
                self.start_data_part();
                self.out
                    .push_str(&format!("\\chapter{{{}}}{}\\par\n\n", escaped, label_cmd));
            }
            // level 3 → \section
            3 => {
                self.section_num += 1;
                self.subsection_num = 0;
                self.out
                    .push_str(&format!("\\section{{{}}}{}\\par\n\n", escaped, label_cmd));
            }
            // level 4 → \subsection
            4 => {
                self.subsection_num += 1;
                self.out.push_str(&format!(
                    "\\subsection{{{}}}{}\\par\n\n",
                    escaped, label_cmd
                ));
            }
            // level 5 → \subsubsection
            5 => {
                self.out.push_str(&format!(
                    "\\subsubsection{{{}}}{}\\par\n\n",
                    escaped, label_cmd
                ));
            }
            // level 6+ → 忽略
            _ => {}
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

        if self.in_abstract {
            // 摘要模式：收集内容
            self.abstract_content.push(body.clone());
            return;
        }

        self.out.push_str(&body);
        self.out.push_str(
            "\\par

",
        );
    }

    fn emit_figure(&mut self, alt: &str, url: &str, label: Option<&str>) {
        // [H]（float 宏包）：图片就地排版，不漂移到页首/页尾
        let mut fig = String::from("\\begin{figure}[H]\n\\centering\n");
        fig.push_str(&format!(
            "\\includegraphics[width=\\textwidth]{{{}}}\n",
            escape_href_url(url)
        ));
        if !alt.is_empty() {
            fig.push_str(&format!("\\caption{{{}}}\n", escape_latex(alt)));
            // \label 必须跟在 \caption 之后，引用的才是图号
            if let Some(lab) = label {
                fig.push_str(&format!("\\label{{{}}}\n", lab));
            }
        }
        fig.push_str("\\end{figure}");

        if self.in_abstract {
            self.abstract_content.push(fig);
            return;
        }
        self.out.push_str(&fig);
        self.out.push_str("\n\n");
    }

    fn emit_table(&mut self, rows: &[Vec<String>], caption: Option<&str>) {
        if rows.is_empty() {
            return;
        }
        // 表题尾部锚点（Block::Label → pending_label）
        let label = self.pending_label.take();

        if self.in_abstract {
            // 摘要模式：收集表格
            let table_latex = emit_longtblr(rows, caption, label.as_deref());
            self.abstract_content.push(table_latex);
            return;
        }

        let table_latex = emit_longtblr(rows, caption, label.as_deref());
        self.out.push_str(&table_latex);
        self.out.push_str(
            "

",
        );
    }

    fn emit_code_block(&mut self, lang: &Option<String>, content: &str) {
        if self.in_abstract {
            return;
        }

        let lang_str = lang.as_deref().unwrap_or("");
        self.out.push_str("\\begin{lstlisting}");
        if let Some(language) = listings_language(lang_str) {
            self.out.push_str("[language=");
            self.out.push_str(language);
            self.out.push(']');
        }
        self.out.push('\n');
        self.out.push_str(content);
        self.out.push_str(
            "\n\\end{lstlisting}

",
        );
    }

    fn emit_list_item(&mut self, level: u8, content: &[Inline]) {
        // 摘要内仍走旧的 prefix+body 路径，不包环境（摘要作为整体渲染）
        if self.in_abstract {
            let prefix = self.list_prefix(level);
            let body = render_inlines(content);
            self.abstract_content.push(format!("{}{}", prefix, body));
            return;
        }

        let body = render_inlines(content);
        let env = list_env_name(level);
        match self.list_env.last() {
            None => {
                self.out.push_str(&format!("\\begin{{{}}}\n", env));
                self.list_env.push((level, env));
            }
            Some(&(cur_level, cur_env)) if cur_level == level && cur_env == env => {
                // 同层续项
            }
            Some(&(cur_level, _)) if level > cur_level => {
                // 进入更深层级：子环境 \begin 插在上一个 \item 之后
                let env_open = format!("\\begin{{{}}}\n", env);
                self.out.insert_str(self.last_item_end, &env_open);
                self.last_item_end += env_open.len();
                self.list_env.push((level, env));
            }
            Some(_) => {
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
                        while let Some(&(_, cur_env)) = self.list_env.last() {
                            self.out.push_str(&format!("\\end{{{}}}\n", cur_env));
                            self.list_env.pop();
                        }
                        self.out.push_str(&format!("\\begin{{{}}}\n", env));
                        self.list_env.push((level, env));
                    }
                } else {
                    self.out.push_str(&format!("\\begin{{{}}}\n", env));
                    self.list_env.push((level, env));
                }
            }
        }
        let item_str = format!("\\item {}\n", body);
        self.out.push_str(&item_str);
        self.last_item_end = self.out.len();
        self.in_list = true;
        self.list_level = level;
    }

    fn reset_list(&mut self) {
        // 列表环境结束后补一个空行：否则紧随其后的正文会被 LaTeX 当成
        // 同一段延续（paralist 的 asparaenum 尤其明显），首行缩进与段距全乱。
        let mut closed = false;
        while let Some((_, env)) = self.list_env.pop() {
            self.out.push_str(&format!("\\end{{{}}}\n", env));
            closed = true;
        }
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

    /// 列表前缀循环
    /// level 1: 1. 2. 3.
    /// level 2: (1) (2) (3)
    /// level 3: a. b. c.
    /// level 4: I. II. III.
    /// level 5: (A) (B) (C)
    /// level 6: 1) 2) 3)
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
                if self.mode == SectionMode::Reference {
                    format!("[{}] ", self.l1)
                } else {
                    format!("{}. ", self.l1)
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
                format!("({}) ", self.l2)
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
                format!("{}. ", ch)
            }
            4 => {
                if !self.in_list || self.list_level < 4 {
                    self.l4 = 0;
                    self.l5 = 0;
                    self.l6 = 0;
                }
                self.l4 += 1;
                let roman = int_to_roman(self.l4);
                format!("{}. ", roman)
            }
            5 => {
                if !self.in_list || self.list_level < 5 {
                    self.l5 = 0;
                    self.l6 = 0;
                }
                self.l5 += 1;
                let ch = (b'A' + ((self.l5 - 1) % 26) as u8) as char;
                format!("({}) ", ch)
            }
            6 => {
                if !self.in_list || self.list_level < 6 {
                    self.l6 = 0;
                }
                self.l6 += 1;
                format!("{}) ", self.l6)
            }
            _ => String::new(),
        }
    }

    /// 完成摘要收集，输出摘要环境
    pub fn finish_abstract(&mut self) {
        if !self.abstract_content.is_empty() {
            self.out.push_str("\\begin{abstract}\n");
            for item in &self.abstract_content {
                self.out.push_str(item);
                self.out.push_str(
                    "\\par

",
                );
            }
            self.out.push_str(
                "\\end{abstract}

",
            );
            self.abstract_content.clear();
        }
        self.in_abstract = false;
        self.mode = SectionMode::Normal;
    }
}

impl Default for TexResearchEmitter {
    fn default() -> Self {
        Self::new()
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

/// LaTeX 9 个特殊字符的转义 + 反斜杠。
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

fn listings_language(lang: &str) -> Option<&'static str> {
    match lang.to_ascii_lowercase().as_str() {
        "" | "text" | "plain" | "plaintext" => None,
        "rust" | "rs" => Some("Rust"),
        "python" | "py" => Some("Python"),
        "cpp" | "c++" | "cc" | "cxx" => Some("C++"),
        "c" => Some("C"),
        "java" => Some("Java"),
        "javascript" | "js" => Some("JavaScript"),
        "typescript" | "ts" => Some("JavaScript"),
        "bash" | "sh" | "shell" => Some("bash"),
        "html" => Some("HTML"),
        "xml" => Some("XML"),
        "sql" => Some("SQL"),
        "json" => None,
        _ => None,
    }
}

/// 整数转罗马数字
fn int_to_roman(n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let values = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut result = String::new();
    let mut remaining = n;
    for (value, symbol) in values {
        while remaining >= value {
            result.push_str(symbol);
            remaining -= value;
        }
    }
    result
}

/// 把列表 level 映射到 paralist 环境名：
/// 1 → asparaenum（首行缩进的段落式条目）
/// 2..=6 → inparaenum（行内紧排）
fn list_env_name(level: u8) -> &'static str {
    match level {
        1 => "asparaenum",
        _ => "inparaenum",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ast::MarkerKind;

    /// 测试辅助：主文件与全部部件按序拼接，便于对整体内容做断言
    fn test_body(e: TexResearchEmitter) -> String {
        let (main, parts) = e.finish();
        let mut s = main;
        for (_, content) in parts {
            s.push_str(&content);
        }
        s
    }

    #[test]
    fn test_chapter_heading() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Heading {
            level: 1,
            text: "引言".into(),
        });
        let body = test_body(e);
        assert!(body.contains("\\chapter{引言}"));
    }

    #[test]
    fn test_section_heading() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Heading {
            level: 3,
            text: "背景".into(),
        });
        let body = test_body(e);
        assert!(body.contains("\\section{背景}"));
    }

    #[test]
    fn test_abstract_marker() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Marker(MarkerKind::Abstract));
        e.emit_block(&Block::Paragraph(vec![Inline::Text("这是摘要内容".into())]));
        e.finish_abstract();
        let body = test_body(e);
        assert!(body.contains("\\begin{abstract}"));
        assert!(body.contains("\\end{abstract}"));
    }

    #[test]
    fn test_list_prefix() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::List {
            ordered: false,
            level: 1,
            content: vec![Inline::Text("第一项".into())],
        });
        e.emit_block(&Block::List {
            ordered: false,
            level: 1,
            content: vec![Inline::Text("第二项".into())],
        });
        e.emit_block(&Block::List {
            ordered: false,
            level: 2,
            content: vec![Inline::Text("子项".into())],
        });
        let body = test_body(e);
        // L1 两项应被包在 asparaenum 里
        assert!(
            body.contains("\\begin{asparaenum}") && body.contains("\\end{asparaenum}"),
            "expected asparaenum wrapper, got: {}",
            body
        );
        assert!(
            body.contains("\\item 第一项"),
            "missing L1 第一项: {}",
            body
        );
        assert!(
            body.contains("\\item 第二项"),
            "missing L1 第二项: {}",
            body
        );
        // L2 单独一项应被包在 inparaenum 里
        assert!(
            body.contains("\\begin{inparaenum}") && body.contains("\\end{inparaenum}"),
            "expected inparaenum wrapper, got: {}",
            body
        );
        assert!(body.contains("\\item 子项"), "missing L2 子项: {}", body);
    }

    #[test]
    fn test_escape_latex() {
        assert_eq!(
            escape_latex("a&b%c$d#e_f{g}h~i^j"),
            "a\\&b\\%c\\$d\\#e\\_f\\{g\\}h\\textasciitilde{}i\\textasciicircum{}j"
        );
    }

    #[test]
    fn test_appendix_marker() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Marker(MarkerKind::Appendix));
        e.emit_block(&Block::Heading {
            level: 1,
            text: "主要符号表".into(),
        });
        let body = test_body(e);
        assert!(body.contains("\\appendix"));
        assert!(body.contains("\\chapter{主要符号表}"));
    }

    #[test]
    fn test_appendix_h1_shifts_inner_levels() {
        // 附录以 H1 开章：H1→chapter，后续 H2→section、H3→subsection
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Marker(MarkerKind::Appendix));
        e.emit_block(&Block::Heading {
            level: 1,
            text: "关键算法模型清单".into(),
        });
        e.emit_block(&Block::Heading {
            level: 2,
            text: "能力簇总览".into(),
        });
        e.emit_block(&Block::Heading {
            level: 3,
            text: "第1簇 态势发现".into(),
        });
        let body = test_body(e);
        assert!(body.contains("\\chapter{关键算法模型清单}"));
        assert!(body.contains("\\section{能力簇总览}"));
        assert!(body.contains("\\subsection{第1簇 态势发现}"));
        assert!(!body.contains("\\chapter{能力簇总览}"));
    }

    #[test]
    fn test_changelog_marker() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Marker(MarkerKind::Changelog));
        e.emit_block(&Block::Heading {
            level: 1,
            text: "版本变更记录".into(),
        });
        e.emit_block(&Block::Paragraph(vec![Inline::Text(
            "v1.0 初始版本".into(),
        )]));
        let body = test_body(e);
        assert!(body.contains("\\chapter*{版本变更记录}"));
        assert!(body.contains("v1.0 初始版本"));
    }

    #[test]
    fn test_body_marker_resets_counters() {
        let mut e = TexResearchEmitter::new();
        // 先有一些章节
        e.emit_block(&Block::Heading {
            level: 1,
            text: "第一章".into(),
        });
        // 遇到 Body 标记
        e.emit_block(&Block::Marker(MarkerKind::Body));
        // 继续标题应该重新开始计数
        e.emit_block(&Block::Heading {
            level: 1,
            text: "新的第一章".into(),
        });
        let body = test_body(e);
        // 应该包含两个章节
        assert!(body.contains("\\chapter{第一章}"));
        assert!(body.contains("\\chapter{新的第一章}"));
    }

    #[test]
    fn test_abstract_with_multiple_paragraphs() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Marker(MarkerKind::Abstract));
        e.emit_block(&Block::Paragraph(vec![Inline::Text("第一段".into())]));
        e.emit_block(&Block::Paragraph(vec![Inline::Text("第二段".into())]));
        e.finish_abstract();
        let body = test_body(e);
        assert!(body.contains("\\begin{abstract}"));
        assert!(body.contains("第一段"));
        assert!(body.contains("第二段"));
        assert!(body.contains("\\end{abstract}"));
    }

    #[test]
    fn test_table_in_normal_mode() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Table {
            rows: vec![
                vec!["列A".into(), "列B".into()],
                vec!["1".into(), "2".into()],
            ],
            caption: None,
        });
        let body = test_body(e);
        assert!(body.contains("\\begin{longtblr}"));
        assert!(body.contains("\\end{longtblr}"));
    }

    #[test]
    fn test_table_cell_bold_italic() {
        let mut e = TexResearchEmitter::new();
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
        // 行内标记不应作为字面量出现
        assert!(!body.contains("**重点**"));
    }

    #[test]
    fn test_table_caption() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Table {
            rows: vec![
                vec!["列A".into(), "列B".into()],
                vec!["1".into(), "2".into()],
            ],
            caption: Some("测试表格".into()),
        });
        let body = test_body(e);
        assert!(body.contains("\\begin{longtblr}[caption={测试表格}]"));
    }

    #[test]
    fn test_subsection_heading() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Heading {
            level: 4,
            text: "子小节".into(),
        });
        let body = test_body(e);
        assert!(body.contains("\\subsection{子小节}"));
    }

    #[test]
    fn test_subsubsection_heading() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Heading {
            level: 5,
            text: "四级标题".into(),
        });
        let body = test_body(e);
        assert!(body.contains("\\subsubsection{四级标题}"));
    }

    #[test]
    fn test_h2_maps_to_chapter_like_pandoc_shift() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Heading {
            level: 2,
            text: "引言".into(),
        });
        let body = test_body(e);
        assert!(body.contains("\\chapter{引言}"));
    }

    #[test]
    fn test_code_block() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::CodeBlock {
            lang: Some("rust".into()),
            content: "fn main() {}".into(),
        });
        let body = test_body(e);
        assert!(body.contains("\\begin{lstlisting}[language=Rust]"));
        assert!(body.contains("fn main()"));
        assert!(body.contains("\\end{lstlisting}"));
    }

    #[test]
    fn test_code_block_sanitizes_unknown_language() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::CodeBlock {
            lang: Some("rust,caption={x}".into()),
            content: "fn main() {}".into(),
        });
        let body = test_body(e);
        assert!(body.contains("\\begin{lstlisting}\n"));
        assert!(!body.contains("caption"));
    }

    #[test]
    fn test_link_escapes_latex() {
        let rendered = render_inlines(&[Inline::Link {
            text: "a_b".into(),
            url: "https://e.test/a#b%20".into(),
        }]);
        assert_eq!(rendered, "\\href{https://e.test/a\\#b\\%20}{a\\_b}");
    }

    #[test]
    fn test_footnote_renders_latex_footnote() {
        let rendered = render_inlines(&[
            Inline::Text("正文".into()),
            Inline::Footnote("注释 100% 属实".into()),
        ]);
        assert_eq!(rendered, "正文\\footnote{注释 100\\% 属实}");
    }

    #[test]
    fn test_standalone_image_emits_figure_with_textwidth() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Paragraph(vec![Inline::Image {
            alt: "总体框架".into(),
            url: "figs/framework.png".into(),
            label: Some("fig:framework".into()),
        }]));
        let body = test_body(e);
        assert!(body.contains("\\begin{figure}[H]"), "got {}", body);
        assert!(body.contains("\\centering"));
        assert!(
            body.contains("\\includegraphics[width=\\textwidth]{figs/framework.png}"),
            "got {}",
            body
        );
        assert!(body.contains("\\caption{总体框架}"));
        // \label 必须跟在 \caption 之后
        let cap = body.find("\\caption{总体框架}").expect("caption");
        let lab = body.find("\\label{fig:framework}").expect("label");
        assert!(cap < lab, "got {}", body);
        assert!(body.contains("\\end{figure}"));
    }

    #[test]
    fn test_image_without_alt_omits_caption() {
        let mut e = TexResearchEmitter::new();
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
    fn test_inline_image_stays_inline() {
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
    fn test_heading_label_emits_label_after_chapter() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Label("chap:overview".into()));
        e.emit_block(&Block::Heading {
            level: 2,
            text: "概述".into(),
        });
        let (main, parts) = e.finish();
        assert!(!main.contains("\\label"));
        assert!(
            parts[0].1.contains("\\chapter{概述}\\label{chap:overview}"),
            "got {}",
            parts[0].1
        );
    }

    #[test]
    fn test_section_label_emits_label() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Label("sec:bg".into()));
        e.emit_block(&Block::Heading {
            level: 3,
            text: "背景".into(),
        });
        let body = test_body(e);
        assert!(
            body.contains("\\section{背景}\\label{sec:bg}"),
            "got {body}"
        );
    }

    #[test]
    fn test_crossref_renders_ref() {
        let rendered = render_inlines(&[
            Inline::Text("见第".into()),
            Inline::CrossRef("chap:overview".into()),
            Inline::Text("章".into()),
        ]);
        assert_eq!(rendered, "见第\\ref{chap:overview}章");
    }

    #[test]
    fn test_citation_renders_as_latex_cite() {
        let rendered = render_inlines(&[Inline::Citation(vec!["a".into(), "b".into()])]);
        assert_eq!(rendered, "\\cite{a,b}");
    }

    #[test]
    fn test_table_label_passed_to_longtblr() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Label("tbl:products".into()));
        e.emit_block(&Block::Table {
            rows: vec![
                vec!["列A".into(), "列B".into()],
                vec!["1".into(), "2".into()],
            ],
            caption: Some("产品清单".into()),
        });
        let body = test_body(e);
        assert!(
            body.contains("caption={产品清单}, label={tbl:products}"),
            "got {body}"
        );
    }

    #[test]
    fn test_appendix_emitted_after_normal_chapter() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Heading {
            level: 1,
            text: "正文".into(),
        });
        e.emit_block(&Block::Marker(MarkerKind::Appendix));
        e.emit_block(&Block::Heading {
            level: 1,
            text: "附录".into(),
        });
        let body = test_body(e);
        assert!(body.contains("\\appendix"));
        assert!(body.contains("\\chapter{附录}"));
    }

    #[test]
    fn test_chapters_split_into_data_parts() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Paragraph(vec![Inline::Text("前言".into())]));
        e.emit_block(&Block::Heading {
            level: 2,
            text: "第一章".into(),
        });
        e.emit_block(&Block::Paragraph(vec![Inline::Text("第一章正文".into())]));
        e.emit_block(&Block::Heading {
            level: 2,
            text: "第二章".into(),
        });
        e.emit_block(&Block::Paragraph(vec![Inline::Text("第二章正文".into())]));
        let (main, parts) = e.finish();

        // 主文件：前言保留，两章按序 \input 引用
        assert!(main.contains("前言"), "got {main}");
        assert!(!main.contains("\\chapter"), "got {main}");
        let first = main.find("\\input{data/chapter01.tex}").expect("input 1");
        let second = main.find("\\input{data/chapter02.tex}").expect("input 2");
        assert!(first < second);

        // 部件：各章内容各自成文
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0, "data/chapter01.tex");
        assert!(parts[0].1.contains("\\chapter{第一章}"));
        assert!(parts[0].1.contains("第一章正文"));
        assert_eq!(parts[1].0, "data/chapter02.tex");
        assert!(parts[1].1.contains("\\chapter{第二章}"));
    }

    #[test]
    fn test_appendix_split_into_appendix_parts() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Heading {
            level: 2,
            text: "正文".into(),
        });
        e.emit_block(&Block::Marker(MarkerKind::Appendix));
        e.emit_block(&Block::Heading {
            level: 1,
            text: "附录A 清单".into(),
        });
        e.emit_block(&Block::Paragraph(vec![Inline::Text("附录内容".into())]));
        let (main, parts) = e.finish();

        // \appendix 必须留在主文件且位于附录部件的 \input 之前
        let appendix_pos = main.find("\\appendix").expect("appendix");
        let input_pos = main
            .find("\\input{appendix/appendix01.tex}")
            .expect("appendix input");
        assert!(appendix_pos < input_pos, "got {main}");

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].0, "appendix/appendix01.tex");
        assert!(parts[1].1.contains("\\chapter{附录A 清单}"));
        assert!(parts[1].1.contains("附录内容"));
    }

    #[test]
    fn test_changelog_and_reference_stay_in_main() {
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::Heading {
            level: 2,
            text: "正文".into(),
        });
        e.emit_block(&Block::Marker(MarkerKind::Changelog));
        e.emit_block(&Block::Heading {
            level: 1,
            text: "版本变更记录".into(),
        });
        e.emit_block(&Block::Marker(MarkerKind::Reference));
        // 参考文献模式：标题由第一个 H1 产出，与 Changelog 行为一致
        e.emit_block(&Block::Heading {
            level: 1,
            text: "参考文献".into(),
        });
        e.emit_block(&Block::List {
            ordered: true,
            level: 1,
            content: vec![Inline::Text("文献一".into())],
        });
        let (main, parts) = e.finish();

        assert_eq!(parts.len(), 1, "只有正文一章: {parts:?}");
        assert!(main.contains("\\chapter*{版本变更记录}"));
        assert!(main.contains("\\chapter*{参考文献}"));
        assert!(main.contains("文献一"));
    }

    #[test]
    fn test_list_env_close_followed_by_blank_line() {
        // 列表结束后必须空一行，否则后续正文会并入列表最后一项所在段落
        let mut e = TexResearchEmitter::new();
        e.emit_block(&Block::List {
            ordered: false,
            level: 1,
            content: vec![Inline::Text("条目".into())],
        });
        e.emit_block(&Block::Paragraph(vec![Inline::Text("后续正文".into())]));
        let (body, _parts) = e.finish();
        assert!(
            body.contains("\\end{asparaenum}\n\n后续正文"),
            "列表环境与后续正文之间缺少空行：\n{}",
            body
        );
    }
}
