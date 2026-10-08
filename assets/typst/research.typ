// 研究报告模板（Typst 引擎）。
//
// 数据来自 /doc.json，由 mdx 的 typst_research 整理、src/export/typst/research.rs 补上
// 字体名与公式 SVG：章节号、图表号、文框号、列表序号都已算好，这里只负责排。版式对照
// mdx 的 md2tex.cls 与 template.tex，凡标「实测」的数值都是对着内置 Tectonic 的 PDF 量
// 出来的落点，改动前先跑对照测试（见 docs/typst-engine.md）。
//
// 坐标约定：竖向位置未特别说明的，都是「相对版心顶」的基线位置（mm）；pt 是 PostScript
// 点（bp），TeX 的 pt 另记为 texpt。

#let doc = json("/doc.json")
#let F = doc.fonts
#let terminal = doc.at("terminal", default: false)
#let dark = terminal and doc.at("dark", default: false)
#let ink = if dark { rgb("#E7EDF0") } else { black }
#let accent = if dark { rgb("#FFBD59") } else { rgb("#B53D30") }
#let cyan = if dark { rgb("#8DDAE9") } else { rgb("#365563") }
#let panel = if dark { rgb("#1C2831") } else { rgb("#F4F5F5") }
#let rule = if dark { rgb("#507A88") } else { rgb("#A0ADB3") }

// ---------------- 版式常量（md2tex.cls） ----------------
#let texpt = 25.4mm / 72.27
#let pitch = 24 * texpt                    // \normalsize 14bp / 24pt
#let body-size = 14pt
#let top-edge = 11.59pt                    // 实测：页首第一行基线在版心顶下 4.09mm
#let bottom-edge = body-size - top-edge
#let text-w = 156mm
#let text-h = 225mm
#let top-m = 37mm
#let inner-m = 28mm
#let outer-m = 26mm
#let flow-probe(kind, line: none) = context {
  [#metadata((kind: kind, line: line, page: here().position().page))<gw-flow>]
}
// 页码盒：fancyhdr 的 \headwidth 取的是 geometry 生效前的版心宽，页码因此不在版心正中，
// 奇数页以版心左缘、偶数页以版心右缘为准（实测）。
#let number-half = 75.57mm
#let number-base = 8.89mm                  // 页码基线在版心底下（实测）

// ---------------- 字体 ----------------
// 西文与数字一律 TeX Gyre Termes，只管拉丁字母（引号、破折号这些中西共用的符号归
// 中文字体，与 xeCJK 的分类一致）。标题类（黑体：章节、部分、目录章条目、图表题标签、
// 文框标题、封面黑体字；封面小标宋大标题）用 Termes Bold，见 heavy；其余 Regular，
// 正文里的 **加粗** 另走 fake-bold-cjk。
#let latin = (name: if terminal { F.mono } else { F.latin }, covers: "latin-in-cjk")
#let song = (latin, if terminal { F.hei } else { F.song }, F.fallback)
#let kai = (latin, if terminal { F.hei } else { F.kai }, F.fallback)
#let hei = (latin, F.hei, F.fallback)
#let xbs = (latin, if terminal { F.hei } else { F.xbs }, F.fallback)
// 标题类文字：西文落到 Termes Bold（中文字体只有常规字重，不受影响）。
#let heavy(font, ..args, body) = text(font: font, weight: "bold", ..args, body)
#let mono = ((name: F.mono, covers: "latin-in-cjk"), if terminal { F.hei } else { F.kai }, F.fallback)

// ---------------- 行内片段 ----------------
// 加粗：西文换 Termes Bold，汉字描边伪粗（xeCJK AutoFakeBold，只作用于中文字体）。
#let fake-bold-cjk(c) = {
  show regex("[^\u{0}-\u{24F}]+"): it => context text(stroke: 0.02857em + text.fill, it)
  text(weight: "bold", c)
}

#let math-box(r) = {
  if "err" in r { return text(font: mono, r.v) }
  box(width: r.w * 1em, height: (r.h + r.dp) * 1em, baseline: r.dp * 1em,
    // 图片放进 place 时按行内元素排、底边贴基线；外面再套一个定高的 box 才落在 dy 处。
    place(top + left, dx: -r.pad * 1em, dy: -r.pad * 1em,
      box(height: (r.h + r.dp + 2 * r.pad) * 1em, image(r.f, width: (r.w + 2 * r.pad) * 1em))))
}

#let ref-run(id) = context {
  let found = query(label(id))
  if found.len() > 0 { link(label(id), found.first().value) } else { text(fill: red, id) }
}

// 上标文献序号：与正文脚注号同一字号、同一高度（实测值见下方脚注一节）。整组用 Termes：
// 区间的连接号（en dash）按中西共用符号会落到中文字体，在上标里显得又宽又散。
#let cite-super(c) = super(typographic: false, baseline: -5.07pt, size: 10.5pt, text(font: F.latin, c))

// 汉字（不含标点）：xeCJK 在它与西文之间插 CJKecglue。
#let cjk-start(r) = r != none and r.t == "s" and r.v.len() > 0 and r.v.clusters().first().match(regex("^[\p{Han}]")) != none
#let cjk-end(r) = r != none and r.t == "s" and r.v.len() > 0 and r.v.clusters().last().match(regex("^[\p{Han}]")) != none

// 花脸稿：删除的字红色加删除线，新增的字套蓝框（与公文模板同一套画法）。
#let del-color = if dark { rgb("#FF9292") } else { rgb("#C00000") }
#let add-color = if dark { rgb("#8CCEFF") } else { rgb("#1F4E9E") }
#let del-mark(c) = text(fill: del-color, strike(stroke: 0.6pt + del-color, offset: -0.32em, c))
#let add-mark(c) = h(2pt) + highlight(fill: none, stroke: 0.5pt + add-color, top-edge: 0.96em,
  bottom-edge: -0.24em, extent: 1.5pt, c) + h(2pt)

// 随包方正字体只有 ①～⑩。⑪～⑳ 用数字和圆圈绘制，避免依赖本机补充字体而静默缺字。
#let generated-list-label(v) = {
  let extra = ("⑪", "⑫", "⑬", "⑭", "⑮", "⑯", "⑰", "⑱", "⑲", "⑳")
  let index = extra.position(c => c == v)
  if index == none { v } else {
    context box(width: 1em, height: 0.85em, baseline: 0.12em, {
      place(top + left, circle(radius: 0.425em, fill: none, stroke: 0.04em + text.fill))
      place(top + left, box(width: 0.85em, height: 0.85em, {
        set par(first-line-indent: 0pt, justify: false)
        align(center + horizon, text(font: F.latin, size: 0.52em,
          top-edge: "cap-height", bottom-edge: "baseline", str(index + 11)))
      }))
    })
  }
}

#let runs(rs) = {
  for (i, r) in rs.enumerate() {
    let prev = if i > 0 { rs.at(i - 1) } else { none }
    let next = rs.at(i + 1, default: none)
    let piece = if r.t == "s" { r.v }
    // 生成编号统一留四分之一字间距，并与后文相连，不能孤悬在行尾。
    // 编号是独立片段，正文里的括注、数字和标题编号不受影响。
    else if r.t == "list-label" { box(generated-list-label(r.v) + h(0.25em)) + "\u{2060}" }
    else if r.t == "b" { fake-bold-cjk(runs(r.c)) }
    else if r.t == "i" { text(font: kai, style: "italic", runs(r.c)) }
    else if r.t == "code" { text(font: mono, r.v) }
    else if r.t == "link" { link(r.url, text(fill: if terminal { cyan } else { blue }, r.v)) }
    else if r.t == "img" {
      box(image(r.src, width: text-w, ..if r.page != none { (page: r.page) }))
    }
    // \ref 的编号紧贴汉字（hyperref 的链接盒挡住了 CJKecglue）。
    else if r.t == "ref" { box(ref-run(r.id)) }
    else if r.t == "cite" {
      let narrative = r.at("n", default: false)
      if narrative and r.keys.first() not in doc.text_cites {
        // 键不在文献库里的 `@key` 不是引用，是正文碰巧写的 `@`：原样印。
        "@" + r.keys.first()
      } else {
        let numbers = if doc.bibliography { r.keys.map(k => cite(label(k))).join() } else { "[" + r.keys.join(",") + "]" }
        if narrative {
          // 叙述式（“见文献[1]”）：序号作句子成分，与正文平排；方括号与前后汉字之间
          // 有 CJKecglue（Termes 词间空）。
          if cjk-end(prev) { h(0.25em) }
          numbers
          if cjk-start(next) { h(0.25em) }
        } else {
          cite-super(numbers)
        }
      }
    }
    else if r.t == "fn" { footnote(r.v) }
    else if r.t == "math" { math-box(r) }
    let m = r.at("m", default: none)
    if m == "del" { del-mark(piece) } else if m == "add" { add-mark(piece) } else { piece }
  }
}

// 文献引用：GB/T 7714 顺序编码。hayagriva 的样式整组标成上标，这里一律先去掉，
// 上标由 cite-super 统一加：`[@key]` 整组上标、字号与位置同脚注号，紧贴前一个字，
// 不加 CJKecglue；叙述式 `@key` 与正文平排。同组的几个 cite 紧挨着，Typst 并成一组，
// 由样式排序、压缩成 [1–3]、[2,4]。
#show cite: it => { show super: s => s.body; it }

// 交叉引用的落点：编号存在零高的 metadata 里，引用处查出来印。
#let anchor(id, number) = if id != none { [#metadata(number)#label(id)] }

// ---------------- 页码 ----------------
// \clearemptydoublepage：在上一页末尾放一个零高的 <gw-clear>，再另起奇数页。标记所在页
// 是奇数页时，中间插进来的空白页不印页码。\pagenumbering / \setcounter{page} 跟着这个
// 标记走（num：样式与起始值），从它之后的第一个奇数页起算；目录之后的 "resume" 接着
// 目录之前的页号往下数。封面在第一个页码段之前，不印页码。
#let next-odd(m) = if calc.odd(m) { m + 2 } else { m + 1 }
#let page-segments() = {
  let out = ()
  let prev = none
  let toc-start = none
  for m in query(<gw-clear>) {
    let num = m.value
    if num == none { continue }
    let first = next-odd(m.location().page())
    let start = if num.start == "resume" {
      if prev == none or toc-start == none { 1 } else { prev.start + (toc-start - prev.first) }
    } else { num.start }
    if num.style == "I" { toc-start = first } else { prev = (start: start, first: first) }
    out.push((first: first, start: start, style: num.style))
  }
  out
}
#let page-label(p, segments) = {
  let seg = segments.filter(s => s.first <= p)
  if seg.len() == 0 { return none }
  let s = seg.last()
  numbering(s.style, s.start + (p - s.first))
}
#let blank-page(p) = query(<gw-clear>).any(m => {
  let at = m.location().page()
  at == p - 1 and calc.odd(at)
})

#let page-number = context {
  let p = here().page()
  let label = if blank-page(p) { none } else { page-label(p, page-segments()) }
  if label != none {
    let x = if calc.odd(p) { inner-m + number-half } else { 210mm - inner-m - number-half }
    place(top + left, dx: x - 50mm, dy: top-m + text-h + number-base,
      box(width: 100mm, align(center, text(font: song, size: 14pt, top-edge: "baseline",
        bottom-edge: "baseline", [—#h(0.25em)#label#h(0.25em)—]))))
  }
}

// \cleardoublepage：另起奇数页，中间的空白页照印页码。
#let clear-double() = pagebreak(weak: true, to: "odd")
#let clear-empty(num: none) = {
  block(height: 0pt, spacing: 0pt, [#metadata(num)<gw-clear>])
  pagebreak(to: "odd")
}
#let front = doc.blocks.len() > 0 and doc.blocks.first().k == "front"

// 研究终端：整页方格底纹、图框、真实元数据栏。装饰位于页边，不覆盖正文。
#let terminal-paper = context {
  let p = here().page()
  let total = counter(page).final().first()
  let logical = if blank-page(p) { none } else { page-label(p, page-segments()) }
  let grid-color = if dark { rgb("#1C2931") } else { rgb("#EDF0F1") }
  for i in range(0, 43) {
    place(top + left, dx: i * 5mm, line(start: (0pt, 0pt), end: (0pt, 297mm), stroke: 0.2pt + grid-color))
  }
  for i in range(0, 60) {
    place(top + left, dy: i * 5mm, line(length: 210mm, stroke: 0.2pt + grid-color))
  }
  place(top + left, dx: 15mm, dy: 12mm, rect(width: 180mm, height: 273mm, fill: none, stroke: 0.5pt + rule))
  place(top + left, dx: 18mm, dy: 16mm, box(width: 174mm, text(font: mono, size: 8pt, fill: cyan,
    grid(columns: (1fr, auto), gutter: 5mm, doc.cover.doc-type, doc.cover.number))))
  place(top + left, dx: 15mm, dy: 25mm, line(length: 180mm, stroke: 0.5pt + rule))
  place(top + left, dx: 15mm, dy: 275mm, line(length: 180mm, stroke: 0.5pt + rule))
  place(top + left, dx: 18mm, dy: 279mm, box(width: 174mm, text(font: mono, size: 8pt, fill: cyan,
    grid(columns: (1fr, auto, auto), gutter: 5mm, doc.cover.version,
      if logical != none { [P #logical] }, [#p / #total]))))
}

#set page(paper: "a4", binding: left,
  margin: (inside: inner-m, outside: outer-m, top: top-m, bottom: 297mm - top-m - text-h),
  fill: if dark { rgb("#141B21") } else { white },
  background: if terminal { terminal-paper } else { page-number })

#set text(font: song, fill: ink, size: body-size, lang: "zh", region: "cn", top-edge: top-edge,
  // 正文逐行利用页尾空间；标题单挂交给实测审校提示，供人工精调。
  bottom-edge: -bottom-edge, overhang: false, costs: (runt: 0%, orphan: 0%, widow: 0%))
#set par(justify: true, leading: pitch - body-size, spacing: pitch - body-size,
  justification-limits: (tracking: (min: 0pt, max: 0.02 * pitch)),
  first-line-indent: (amount: 2em, all: true))
#set block(spacing: pitch - body-size)

// 脚注：小五号的 \footnotesize（7.5pt），正文里的脚注号 10.5pt 上标（实测）。
#set footnote(numbering: "1")
#show footnote: it => {
  let n = counter(footnote).at(it.location()).first()
  link(it.location(), super(baseline: -5.07pt, size: 10.5pt, text(font: latin, str(n))))
}
// 实测：脚注线在正文末行字身下缘下 1.96mm，脚注基线在线下 3.21mm。
#set footnote.entry(separator: line(length: 0.4 * text-w, stroke: 0.4 * texpt),
  clearance: 1.8mm, gap: 3.21mm - 0.66 * 7.5pt - 0.62mm, indent: 0pt)
#show footnote.entry: it => {
  set text(size: 7.5pt, top-edge: "cap-height", bottom-edge: -3.06pt)
  set par(first-line-indent: 0pt, leading: 9 * texpt - 7.5pt, justify: true)
  let n = counter(footnote).at(it.note.location()).first()
  h(10.5pt) + super(baseline: -2.72pt, size: 4.98pt, text(font: latin, str(n))) + it.note.body
}

// ---------------- 标题 ----------------
// 目录条目：每个进目录的标题前放一个 <gw-toc>，目录按它们的页码排。
#let toc-entry(kind, prefix, text) = [#metadata((kind: kind, prefix: prefix, text: text))<gw-toc>]

// PDF 书签：一个看不见、不占地方的 heading（版面上的标题另排，免得 heading 自带的字号
// 与字重掺进来）。
#let bookmark(level, body) = place(hide(heading(level: level, outlined: false, bookmarked: true, body)))
#set heading(numbering: none)

// 行末的全角标点：Typst 会把它压掉半个字，居中、居右的行因此偏出去；xeCJK 不压。
// 行末补一个空盒子挡住。
#let keep-end = box()

// 在给定基线排一行居中的字：字身上缘固定在基线上 1em，公式不超过 1em 就不会把行撑低。
#let centered-at(y, body) = place(top + left, dy: y - 1em, box(width: text-w, align(center,
  text(top-edge: 1em, bottom-edge: "baseline", body + keep-end))))

// 章（ctexbook \chapter）：另起奇数页，小二黑体居中，「第1章」与题名间空一字。
// 实测：章题基线在版心顶下 28.71mm，其后第一行基线 51.20mm。
#let chapter-top = 28.71mm
#let chapter-next = 51.20mm
// 终端的编号是独立构件：章用圆形气泡，目录及不编号章用方形签牌。
#let terminal-dash() = line(length: 100%, stroke: (paint: rule, thickness: 0.45pt, dash: "dashed"))
#let terminal-badge(value, size: 15mm, round: false, font-size: 19pt) = {
  let body = align(center + horizon, text(font: mono, fill: accent, weight: "bold", size: font-size,
    top-edge: "cap-height", bottom-edge: "baseline", value))
  if round { circle(radius: size / 2, inset: 0pt, fill: none, stroke: 1pt + accent, body) }
  else { rect(width: size, height: size, inset: 0pt, fill: none, stroke: 1pt + accent, body) }
}
#let terminal-stamp(body) = block(spacing: 0pt, {
  set par(first-line-indent: 0pt, justify: false)
  box(line(length: 7mm, stroke: 0.7pt + accent))
  h(3mm)
  text(font: mono, size: 8pt, fill: cyan, body)
})
#let terminal-id(number) = if number == none { "※" } else if number.match(regex("^[0-9]+$")) != none {
  if int(number) < 10 { "0" + number } else { number }
} else { number }
#let terminal-heading(body, prefix: none, number: none, badge: "※", stamp: "RESEARCH / 研究报告") = block(
  width: 100%, above: 8mm, below: 0pt, breakable: false, {
    set block(spacing: 0pt)
    set par(first-line-indent: 0pt, justify: false, leading: 7pt, spacing: 0pt)
    terminal-stamp(if prefix != none { prefix + " / CHAPTER " + terminal-id(number) } else { stamp })
    v(7mm)
    grid(columns: (15mm, 1fr), column-gutter: 5mm, align: horizon,
      terminal-badge(if number != none { terminal-id(number) } else { badge }, round: number != none),
      heavy(hei, size: 25pt, top-edge: "bounds", bottom-edge: "bounds", body))
    v(9mm)
    terminal-dash()
    v(10mm, weak: false)
  })
#let chapter-block(prefix, body, level: 2, pre: none, number: none) = {
  let pre = { place(flow-probe("boundary")); pre }
  let title = if prefix != none { prefix + h(1em) + body } else { body }
  if terminal {
    return {
      clear-double()
      block(width: 100%, spacing: 0pt, {
        pre
        bookmark(level, title)
        terminal-heading(body, prefix: prefix, number: number)
      })
    }
  }
  clear-double()
  block(height: chapter-next - top-edge, width: 100%, spacing: 0pt, {
    pre
    bookmark(level, title)
    set text(font: hei, weight: "bold", size: 18pt)
    centered-at(chapter-top, title)
  })
}

#let chapter-prefix(p) = {
  // 「第1章」：汉字与数字之间是黑体的词间空（xeCJK CJKecglue）。
  let digits = p.match(regex("[0-9A-Z]+"))
  if digits == none { return p }
  p.slice(0, digits.start) + h(0.304em) + digits.text + if digits.end < p.len() { h(0.304em) + p.slice(digits.end) }
}

#let chapter(b, id: none) = {
  let prefix = if b.prefix != none { chapter-prefix(b.prefix) } else { none }
  let pre = {
    if terminal { [#metadata(none)#label(id)] }
    if b.toc { toc-entry("chapter", b.prefix, b.text) }
    if b.number != none { counter(footnote).update(0) }
  }
  chapter-block(if terminal { b.prefix } else { prefix }, runs(b.text), pre: pre, number: b.number)
  anchor(b.label, b.number)
}

// 节（titlesec）：缩进两字，黑体四号，编号与题名空半字，前后不加距离。
// 紧跟在章题之后不再加段距（章题块已经量到下一行基线）；紧跟表格时实测比正文多空 1.83mm。
#let section(b, prev: none) = {
  if b.toc and b.level == 1 { toc-entry("section", b.number, b.text) }
  let num = if b.number != none { b.number + h(0.5em) }
  if terminal {
    block(breakable: false, above: if prev == "chapter" { 0pt } else { 6mm }, below: 4mm, {
      bookmark(b.level + 2, num + runs(b.text))
      set par(first-line-indent: 0pt, justify: false)
      {
        if b.level == 1 and prev != "chapter" { terminal-dash(); v(4mm) }
        grid(columns: (auto, 1fr), column-gutter: 4mm, align: horizon,
          if b.number != none { box(stroke: 0.8pt + accent, inset: (x: 2mm, y: if b.level == 1 { 1.5mm } else { 1mm }), text(font: mono, size: if b.level == 1 { 13pt } else { 11pt }, fill: accent, b.number)) } else { [] },
          heavy(hei, size: if b.level == 1 { 17pt } else if b.level == 2 { 15pt } else { 14pt }, runs(b.text) + flow-probe("heading", line: b.at("line", default: none))))
      }
    })
    anchor(b.label, b.number)
    return
  }
  let above = if prev == "chapter" { 0pt } else if prev == "table" { 10.47mm - top-edge + 1.83mm } else { pitch - body-size }
  block(breakable: false, above: above, below: pitch - body-size, {
    bookmark(b.level + 2, num + runs(b.text))
    par(first-line-indent: 0pt, justify: false, if not terminal { h(2em) } + heavy(hei, fill: if terminal { cyan } else { ink }, num + runs(b.text)) + flow-probe("heading", line: b.at("line", default: none)))
  })
  anchor(b.label, b.number)
}

// 部分（\part）：独占一页，小一黑体居中；book 类在部分页之后另加一张不印页码的空白页。
// 实测「第一部分」基线在版心顶下 77.23mm，题名 94.42mm。
#let part(b, children: (), serial: none) = {
  clear-double()
  let head = if b.number != none { [第#b.number;部分] }
  if terminal {
    // 明细表只取本部分的章；页码用真正的章节位置查询，不能由样张数字写死。
    block(width: 100%, spacing: 0pt, {
      toc-entry("part", if b.number != none { "第" + b.number + "部分" } else { none }, b.text)
      bookmark(1, if head != none { head + h(1em) } + runs(b.text))
      set par(first-line-indent: 0pt, justify: false)
      v(8mm)
      terminal-stamp("研究分部 / PART " + terminal-id(serial))
      v(12mm)
      grid(columns: (8mm, 1fr), column-gutter: 4mm, align: horizon,
        terminal-badge(if serial != none { "P" + serial } else { "P" }, size: 8mm, font-size: 10pt),
        text(font: mono, size: 10pt, fill: cyan, head))
      v(7mm)
      heavy(hei, size: 30pt, top-edge: "bounds", bottom-edge: "bounds", runs(b.text))
      v(12mm)
      text(font: mono, size: 9pt, fill: cyan, "章节明细 / CHAPTER LIST")
      v(3mm)
      line(length: text-w, stroke: 0.6pt + rule)
      for child in children {
        context {
          let target = query(label(child.id)).first().location()
          let num = page-label(target.page(), page-segments())
          block(width: 100%, above: 3mm, below: 3mm, breakable: false, link(target,
            grid(columns: (10mm, 1fr, auto, 10mm), column-gutter: 3mm, align: horizon,
              terminal-badge(if child.b.number != none { child.b.number } else { "—" }, size: 4mm, font-size: 7pt, round: true),
              text(size: 12pt, runs(child.b.text)),
              text(font: mono, size: 8pt, fill: cyan, "起始"),
              align(right, text(font: mono, size: 10pt, fill: cyan, num)))))
          terminal-dash()
        }
      }
      anchor(b.label, b.number)
    })
    if serial != none {
      // 斜线只填充数字轮廓内部，与参考的线刻部号一致。
      v(1fr)
      align(right, text(font: mono, size: 120pt, weight: "bold", top-edge: "bounds", bottom-edge: "bounds",
        stroke: 0.6pt + cyan, fill: tiling(size: (3mm, 3mm), relative: "parent",
          place(line(start: (0pt, 3mm), end: (3mm, 0pt), stroke: 0.3pt + rule))), terminal-id(serial)))
    }
    clear-empty()
    return
  }
  block(width: 100%, height: 94.42mm, spacing: 0pt, {
    toc-entry("part", if b.number != none { "第" + b.number + "部分" } else { none }, b.text)
    bookmark(1, if head != none { head + h(1em) } + runs(b.text))
    set text(font: hei, weight: "bold", size: 24pt)
    if head != none { centered-at(77.23mm, head) }
    centered-at(94.42mm, runs(b.text))
  })
  anchor(b.label, b.number)
  clear-empty()
}

// ---------------- 正文块 ----------------
#let para(c) = par(flow-probe("body") + runs(c))

#let aligned(b) = {
  let a = if b.align == "center" { center } else { right }
  align(a, par(first-line-indent: 0pt, justify: false, runs(b.c) + keep-end))
}

// 列表（paralist）：一级条目是段落式（asparaenum），⑴ 后退 0.2 字；更深的条目接排在
// 段内（inparaenum），① 后退 0.3 字。
#let list-label(e) = {
  let back = if e.level == 1 { 0.2em } else if e.level == 2 { 0.3em } else { 0em }
  text(font: song, e.label) + h(-back)
}
#let list-par(b) = {
  let first = b.items.first()
  par(flow-probe("body") + b.items.enumerate().map(((i, e)) => {
    if i > 0 { h(0.25em) }
    list-label(e) + runs(e.c)
  }).join())
}

// 表题与图题（caption）：标签黑体、题名宋体，小四，二者空半字。
#let caption-line(tag, body) = align(center, par(first-line-indent: 0pt, justify: false,
  text(size: 12pt, top-edge: 0.83em, bottom-edge: -0.17em,
    heavy(hei, tag) + h(0.5em) + if body != none { text(font: song, body) } + keep-end)))

// ---------------- 表格（longtblr） ----------------
#let col-sep = 6 * texpt
#let rule-w = 0.4 * texpt
#let table-widths(cols, total) = {
  let fixed = cols.filter(c => c.kind == "em").map(c => c.v * 12pt).sum(default: 0pt)
  let ratio = cols.filter(c => c.kind == "fr").map(c => c.v).sum(default: 0)
  let avail = total - cols.len() * 2 * col-sep - fixed
  cols.map(c => if c.kind == "em" { c.v * 12pt + 2 * col-sep } else { avail * c.v / ratio + 2 * col-sep })
}
#let table-serial = counter("gw-table")
// 实测：表题基线距上一行基线 10.46mm，表顶线在表题基线下 2.70mm；表底线到下一行基线
// 10.47mm。
#let table-block(t, prev: none) = {
  table-serial.step()
  let widths = table-widths(t.cols, text-w)
  let n = t.cols.len()
  let al(a) = if a == "l" { left } else if a == "r" { right } else { center }
  let mk(c, header) = {
    let args = (:)
    if c.colspan > 1 { args.colspan = c.colspan }
    if c.rowspan > 1 { args.rowspan = c.rowspan }
    let body = runs(c.c)
    // 表头：黑体字形、西文 Termes Regular（TeX 的 \heiti 不带 \enhei）。
    if header { body = text(font: hei, fill: if terminal { cyan } else { ink }, body) }
    table.cell(..args, align: al(c.align) + horizon, body)
  }
  let cell-or-skip(c, header) = if c == none { () } else { (mk(c, header),) }
  context {
    let id = "gw-tbl-end-" + str(table-serial.get().first())
    let start = "gw-tbl-start-" + str(table-serial.get().first())
    let tag = [表#h(0.25em)#t.number]
    let body = if t.caption != none { runs(t.caption) }
    // 表题放进表头：跨页时每页重复，续页上缀「（续表）」（tabularray 的 conthead）。
    let caption = table.cell(colspan: n, stroke: none, inset: (x: 0pt, top: 0pt, bottom: 2.70mm - 0.17 * 12pt),
      align: center, context {
        let first = locate(label(start)).page()
        let cont = if here().page() > first { h(0.25em) + [（续表）] }
        text(top-edge: 0.83em, bottom-edge: -0.17em, heavy(hei, tag)
          + h(0.5em) + text(font: song, body + cont) + keep-end)
      })
    // 紧跟章题：实测表题基线在版心顶下 53.23mm（章题块量到 51.20mm 处的下一行基线）。
    // 紧跟节标题时实测多空 1.90mm。
    let above = if prev == "chapter" { 53.23mm - (chapter-next - top-edge) - 0.83 * 12pt }
      else if prev == "section" { 10.46mm + 1.90mm - bottom-edge - 0.83 * 12pt }
      else { 10.46mm - bottom-edge - 0.83 * 12pt }
    block(above: above, below: 10.47mm - top-edge, width: 100%, {
      [#metadata(none)#label(start)]
      anchor(t.label, t.number)
      // \fontsize{12bp}{18pt}：每行一个 18pt 的支柱（基线上 0.7、下 0.3）。
      set text(size: 12pt, top-edge: 0.7 * 18 * texpt, bottom-edge: -0.3 * 18 * texpt)
      set par(leading: 0pt, spacing: 0pt, first-line-indent: 0pt, justify: false)
      table(
        columns: widths,
        inset: (x: col-sep, y: 2 * texpt + rule-w / 2),
        stroke: rule-w + if terminal { rule } else { black },
        table.header(caption, ..t.rows.at(0).map(c => cell-or-skip(c, true)).flatten()),
        ..t.rows.slice(1).map(r => r.map(c => cell-or-skip(c, false)).flatten()).flatten(),
        // 续表提示（tabularray 的 contfoot）：只在表格还没结束的页上有内容。
        table.footer(repeat: true, table.cell(colspan: n, stroke: none, inset: 0pt, align: right,
          context {
            let end = locate(label(id)).page()
            if here().page() < end { block(above: 0pt, inset: (top: 2 * texpt), text(size: 12pt)[下一页继续]) }
          })),
      )
      [#metadata(none)#label(id)]
    })
  }
}

// ---------------- 插图（figure [H]） ----------------
// 实测：图顶距上一行基线 3.07mm，图题基线在图底下 5.67mm，下一行基线距图题基线 13.47mm。
#let figure-block(f) = {
  let w = if f.width != none { f.width * text-w } else { text-w }
  let img = if f.width != none {
    image(f.src, width: w, ..if f.page != none { (page: f.page) })
  } else {
    image(f.src, width: text-w, height: 0.6 * text-h, fit: "contain", ..if f.page != none { (page: f.page) })
  }
  block(above: 3.07mm - bottom-edge, below: 13.47mm - 0.17 * 12pt - top-edge, width: 100%, breakable: false, {
    align(center, img)
    if f.caption != none {
      v(5.67mm - 0.83 * 12pt - (pitch - body-size), weak: false)
      caption-line([图#h(0.25em)#f.number], f.caption)
    }
    anchor(f.label, f.number)
  })
}

// ---------------- 代码（listings） ----------------
// 等宽 11bp / 16pt，浅灰底、细框、圆角；行号小号灰色排在框外左侧。
#let code-block(b) = {
  let lines = b.text.split("\n")
  block(above: 10 * texpt, below: 10 * texpt, width: 100%, inset: (left: 5 * texpt, right: 10 * texpt),
    block(width: 100%, fill: if terminal { panel } else { rgb(248, 248, 248) }, stroke: 0.4pt + if terminal { rule } else { rgb(220, 220, 220) }, radius: if terminal { 0pt } else { 2pt },
      inset: (left: 15 * texpt, right: 3pt, y: 6 * texpt), {
      set text(font: mono, size: 11pt, top-edge: 0.7 * 16 * texpt, bottom-edge: -0.3 * 16 * texpt)
      set par(first-line-indent: 0pt, justify: false, leading: 0pt, spacing: 0pt)
      grid(columns: (0pt, 1fr), row-gutter: 0pt,
        ..lines.enumerate().map(((i, l)) => (
          place(right, dx: -10 * texpt, text(size: 6.5pt, fill: rgb(153, 153, 153), font: (latin,), str(i + 1))),
          par(l),
        )).flatten())
    }))
}

// ---------------- 公式 ----------------
#let display-math(b) = {
  let m = b.at("m", default: none)
  let body = math-box(b)
  let body = if m == "del" { del-mark(body) } else if m == "add" { box(stroke: 0.5pt + add-color, inset: 3pt, body) } else { body }
  align(center, par(first-line-indent: 0pt, justify: false, body))
}

// ---------------- 引文与文框 ----------------
// 引文（mdxquote）：楷体，左右各缩进两字，首行再缩进两字，前后各空半行；出处靠右。
#let terminal-card-tag(name) = ("引文": "QUOTE", "引理": "LEMMA", "推论": "COROLLARY", "定理": "THEOREM",
  "命题": "PROPOSITION", "专栏": "PANEL", "案例": "CASE", "例子": "EXAMPLE", "做法": "PRACTICE").at(name, default: name)
// 卡片仍可跨页：上角标跟首片、下角标跟末片，不把长引文锁成不可拆的盒子。
#let terminal-card(name, number: none, title: (), body) = block(width: 100%, above: 5mm, below: 5mm,
  fill: panel, stroke: 0.5pt + rule, inset: 4mm, {
    set block(spacing: 0pt)
    set text(font: hei, size: 12pt, top-edge: "cap-height", bottom-edge: "baseline")
    set par(first-line-indent: 0pt, justify: false, leading: 8pt, spacing: 3mm)
    place(top + left, dx: -4mm, dy: -4mm, {
      place(line(start: (0pt, 3.6mm), end: (0pt, 0pt), stroke: 1.2pt + accent))
      place(line(length: 3.6mm, stroke: 1.2pt + accent))
    })
    block(sticky: true, grid(columns: (1fr, auto), column-gutter: 4mm, align: top,
      heavy(hei, name + if number != none { h(0.5em) + text(font: mono, fill: accent, number) }
        + if title.len() > 0 { h(0.8em) + runs(title) }),
      box(stroke: 0.4pt + rule, inset: (x: 1mm, y: 0.6mm), text(font: mono, size: 7pt, fill: cyan, terminal-card-tag(name)))))
    v(3mm, weak: false)
    body
    block(height: 0pt, width: 100%, above: 0pt, below: 0pt, {
      place(top + right, dx: 4mm, dy: 4mm, {
        place(line(start: (-3.6mm, 0pt), end: (0pt, 0pt), stroke: 1.2pt + accent))
        place(line(start: (0pt, -3.6mm), end: (0pt, 0pt), stroke: 1.2pt + accent))
      })
    })
  })
#let quote-block(b) = {
  if terminal {
    return terminal-card("引文", {
      for l in b.items {
        if l.k == "source" { align(right, par(runs(l.c))) }
        else if l.k == "math" { display-math(l) }
        else if l.k == "list" { par(text(font: mono, l.label) + h(0.3em) + runs(l.c)) }
        else { par(runs(l.c)) }
      }
    })
  }
  block(above: 0.5 * pitch + (pitch - body-size), below: 0.5 * pitch + (pitch - body-size),
  inset: (left: 2em, right: 2em), width: 100%, {
  set text(font: kai)
  for l in b.items {
    if l.k == "source" { align(right, par(first-line-indent: 0pt, justify: false, runs(l.c))) }
    else if l.k == "math" { display-math(l) }
    else if l.k == "list" { par(text(font: song, l.label) + h(-0.2em) + runs(l.c)) }
    else { par(runs(l.c)) }
  }
})
}

// 文框（mdxboxtblr）：0.6pt 细框、浅灰底，标题行黑体居中，内文楷体小四、一行一格。
// 实测：标题行基线在框顶下 6.01mm，行距 8.44mm，末行基线到框底 5.45mm。
#let box-block(b, prev: none) = {
  if terminal {
    return terminal-card(b.name, number: b.number, title: b.title, {
      anchor(b.label, b.number)
      for l in b.items {
        if l.k == "math" { align(center, par(math-box(l))) }
        else if l.k == "list" { par(text(font: mono, l.label) + h(0.3em) + runs(l.c)) }
        else { par(runs(l.c)) }
      }
    })
  }
  let title = heavy(hei, b.name + h(0.5em) + b.number + h(1em) + runs(b.title))
  // 实测：框顶距上一行基线 5.10mm，框底到下一行基线 8.43mm；两框相接时框间 8.85mm
  // （tabularray 的 presep 与 postsep 相加）。
  // 落在页首时框顶仍在版心顶下 3.62mm（TeX 的 presep 前有 \label，不会被页首吃掉）：
  // 这一段放进框外的块里，块前距相应减掉。
  let keep = if prev == "chapter" { 0mm } else { 3.62mm }
  let above = if prev == "box" { 8.85mm } else if prev == "chapter" { 5.10mm - bottom-edge - 3.62mm } else { 5.10mm - bottom-edge }
  block(above: above - keep, below: 8.43mm - top-edge, width: 100%, {
  v(keep, weak: false)
  block(above: 0pt, below: 0pt, width: 100%,
    fill: if terminal { panel } else { luma(93%) }, stroke: 0.6pt + if terminal { accent } else { black }, inset: (x: 1em + 0.6pt, top: 6.01mm - 0.83 * 12pt, bottom: 5.45mm - 0.17 * 12pt), {
    set text(size: 12pt, font: kai, top-edge: 0.83em, bottom-edge: -0.17em)
    set par(first-line-indent: 0pt, leading: 8.44mm - 12pt, spacing: 8.44mm - 12pt, justify: true)
    anchor(b.label, b.number)
    align(if terminal { left } else { center }, par(justify: false, text(fill: if terminal { accent } else { ink }, title)))
    for l in b.items {
      if l.k == "math" { align(center, par(math-box(l))) }
      else if l.k == "list" { par(h(2em) + text(font: song, l.label) + h(-0.2em) + runs(l.c)) }
      else { par(h(2em) + runs(l.c)) }
    }
  })
  })
}

// ---------------- 参考文献 ----------------
#let bib-block(b) = {
  if b.titled { chapter-block(none, [参考文献]) }
  // 条目之间实测比行距多 2.80mm（gbt7714 的 \itemsep）。
  set par(first-line-indent: 0pt, spacing: pitch - body-size - 0.69mm)
  block(above: 0pt, bibliography("/references.bib", title: none, style: "gb-7714-2015-numeric"))
}

// ---------------- 摘要、目录 ----------------
#let abstract(b, render) = {
  chapter-block(none, [摘要], pre: toc-entry("chapter", none, ((t: "s", v: "摘要"),)))
  render(b.blocks)
}

// 目录：标题小二黑体居中；章条目黑体、页码 Termes 加粗；节条目缩进、带点线。
// 实测：标题基线在版心顶下 29.52mm，首条目基线 52.02mm。
#let toc-block() = {
  clear-empty(num: (style: "I", start: 1))
  context {
    if terminal {
      block(width: 100%, spacing: 0pt, {
        bookmark(2, [目录])
        terminal-heading([目录], badge: "BOM", stamp: "CONTENTS / 研究目录")
      })
    } else { block(height: 52.02mm - top-edge, width: 100%, spacing: 0pt, {
      bookmark(2, [目录])
      set text(font: hei, weight: "bold", size: 18pt)
      centered-at(29.52mm, [目录])
    }) }
    let segments = page-segments()
    set par(first-line-indent: 0pt, justify: false)
    for e in query(<gw-toc>) {
      let p = e.location().page()
      let num = page-label(p, segments)
      let v = e.value
      let target = e.location()
      if terminal {
        let part = v.kind == "part"
        let section = v.kind == "section"
        let indent = if part { 0mm } else if section { 14mm } else { 0mm }
        let mark = if part {
          let parts = doc.blocks.filter(b => b.k == "part" and b.number != none)
          let indices = parts.enumerate().filter(((i, b)) => "第" + b.number + "部分" == v.prefix)
          text(font: mono, size: 11pt, fill: accent, "PART " + if indices.len() > 0 { str(indices.first().first() + 1) } else { "—" })
        } else if section { text(font: mono, fill: cyan, size: 10pt, v.prefix) }
        else {
          let number = if v.prefix != none { v.prefix.match(regex("[0-9A-Z]+")) } else { none }
          terminal-badge(if number != none { number.text } else { "—" }, size: 4mm, round: true, font-size: 7pt)
        }
        block(width: 100%, inset: (left: indent), above: if section { 0pt } else { 4mm }, below: 1.8mm, breakable: false,
          link(target, grid(columns: (if part { 22mm } else { 14mm }, 1fr), align: horizon, {
            mark
          }, par({
            text(font: hei, size: 12pt, weight: if section { "regular" } else { "bold" }, runs(v.text))
            box(width: 1fr, repeat(gap: 1.5mm, justify: false, text(fill: rule, ".")))
            h(2mm)
            box(width: 8mm, align(right, text(font: mono, size: 10pt, fill: cyan, num)))
          }))))
        if part { line(length: text-w, stroke: 0.5pt + rule) }
        continue
      }
      if v.kind == "section" {
        par(link(target, h(15.75pt) + if v.prefix != none { v.prefix + h(0.5em) } + runs(v.text)
          + box(width: 1fr, repeat(gap: 10.5pt - 0.25em, justify: false)[.]) + h(1.5em)
          + box(width: 1.55em, align(right, text(font: (latin,), num)))))
      } else {
        let label = if v.prefix != none { chapter-prefix(v.prefix) + h(0.8em) }
        par(link(target, heavy(hei, label + runs(v.text)) + h(1fr)
          + text(font: (latin,), weight: "bold", num)))
      }
    }
  }
}

// ---------------- 封面（template.tex 的 titlepage） ----------------
// 位置按距页面左上角的毫米数（TikZ 节点 anchor=north：节点顶即字形上缘）。
// 终端封面沿用文件类型的语义，不虚构摘要、图表、英译名或机构资料。
#let terminal-cover() = {
  let c = doc.cover
  let at(y, body) = place(top + left, dy: y, box(width: text-w, body))
  set par(first-line-indent: 0pt, justify: false)
  at(4mm, text(font: mono, size: 10pt, fill: cyan, "RESEARCH / 研究终端"))
  at(22mm, block(fill: accent, inset: (x: 3mm, y: 2mm), text(font: hei, fill: if dark { rgb("#141B21") } else { white }, size: 14pt, c.doc-type)))
  if c.ident != "" { at(40mm, text(size: 12pt, fill: cyan, c.ident)) }
  at(58mm, {
    heavy(hei, size: 28pt, top-edge: "bounds", bottom-edge: "bounds",
      par(leading: 12pt, c.title.map(runs).join(linebreak())))
    if c.original != "" { v(6mm); text(size: 12pt, c.original) }
  })
  at(118mm, line(length: text-w, stroke: 1pt + accent))
  at(128mm, {
    text(font: mono, size: 10pt, fill: cyan, "CONTENTS / 研究结构")
    v(4mm)
    let chapters = doc.blocks.filter(b => b.k == "chapter")
    for b in chapters.slice(0, calc.min(5, chapters.len())) {
      block(spacing: 2mm, text(size: 12pt, {
        text(font: mono, fill: accent, if b.prefix != none { b.prefix } else { "—" })
        h(4mm)
        runs(b.text)
      }))
    }
  })
  at(192mm, {
    if c.stage != none {
      grid(columns: (1fr, 1fr, 1fr, 1fr), gutter: 2mm,
        ..("立项论证", "建设实施", "技术实现", "项目总结").enumerate().map(((i, name)) =>
          block(stroke: 0.6pt + if i == c.stage { accent } else { rule }, inset: 2mm,
            text(size: 9pt, fill: if i == c.stage { accent } else { cyan }, name))))
    } else if c.byline.len() > 0 { text(size: 12pt, c.byline.join(h(1em))) }
    v(6mm)
    line(length: text-w, stroke: 0.5pt + rule)
    v(4mm)
    heavy(hei, size: 16pt, c.institution)
    v(3mm)
    text(size: 12pt, c.date)
  })
  if c.security != "" {
    at(-7mm, align(right, text(font: hei, size: 10pt, fill: accent,
      c.security + if c.security-years != "" { "★" + c.security-years })))
  }
}

#let cover() = {
  if terminal { return terminal-cover() }
  let c = doc.cover
  let at(x, y, anchor: center, body) = place(top + left, dx: x - inner-m - 100mm, dy: y - top-m,
    box(width: 200mm, align(anchor, text(top-edge: "bounds", bottom-edge: "bounds", body))))
  let sp = 0.25em
  if c.security != "" {
    let years = if c.security-years != "" { [★#c.security-years] }
    place(top + left, dx: 25mm - inner-m, dy: 20mm - top-m,
      heavy(hei, size: 12pt, top-edge: "bounds", bottom-edge: "bounds", [#c.security#years]))
  }
  if c.number != "" {
    place(top + left, dx: 185mm - inner-m - 100mm, dy: 20mm - top-m, box(width: 100mm, align(right,
      heavy(hei, size: 12pt, top-edge: "bounds", bottom-edge: "bounds", [编号：#c.number]))))
  }
  at(105mm, 68mm, heavy(hei, size: 18pt, c.doc-type.clusters().join(h(0.5em))))
  if c.ident != "" { at(105mm, 83mm, text(font: song, size: 14pt, c.ident)) }
  place(top + left, dx: 25mm - inner-m, dy: 94mm - top-m, rect(width: 160mm, height: 0.5mm, fill: black, stroke: none))
  if c.stage == none {
    place(top + left, dx: 25mm - inner-m, dy: 95.3mm - top-m, rect(width: 160mm, height: 0.2mm, fill: black, stroke: none))
  }
  context {
    let title = box(width: 150mm, align(center, heavy(xbs, size: 26pt, top-edge: "bounds", bottom-edge: "bounds",
      par(leading: 37.7pt - 26pt, first-line-indent: 0pt, justify: false, c.title.map(runs).join(linebreak())))))
    let y = 114mm
    place(top + left, dx: 105mm - 75mm - inner-m, dy: y - top-m, title)
    y += measure(title).height
    if c.version != "" {
      let v = text(font: song, size: 15pt, top-edge: "bounds", bottom-edge: "bounds", c.version)
      y += 6mm
      at(105mm, y, v)
      y += measure(v).height
    }
    if c.original != "" {
      at(105mm, y + 6mm, box(width: 150mm, text(font: (latin, F.song), style: "italic", size: 15pt,
        par(leading: 20pt - 15pt, first-line-indent: 0pt, justify: false, c.original))))
    }
  }
  if c.stage != none {
    for (i, name) in ("立项论证", "建设实施", "技术实现", "项目总结").enumerate() {
      let xl = 25mm + i * 40.5mm
      let current = i == c.stage
      place(top + left, dx: xl - inner-m, dy: 218mm - top-m,
        rect(width: 38.5mm, height: if current { 0.8mm } else { 0.2mm }, stroke: none,
          fill: if current { black } else { luma(65%) }))
      at(xl + 19.25mm, 221.2mm, heavy(hei, size: 10.5pt, fill: if current { black } else { luma(55%) }, name))
    }
  } else if c.byline.len() > 0 {
    at(105mm, 224mm, text(font: song, size: 14pt, c.byline.join(h(1em))))
  }
  context {
    let inst = heavy(hei, size: 16pt, top-edge: "bounds", bottom-edge: "bounds", c.institution)
    at(105mm, 245mm, inst)
    at(105mm, 245mm + measure(inst).height + 5mm, text(font: song, size: 15pt, c.date))
  }
}

// ================= 正文 =================
#let render(blocks) = {
  let prev = none
  let part-serial = 0
  for (i, b) in blocks.enumerate() {
    let k = b.k
    // 会主动起页的结构在新页内容里放边界；不能先占住旧页，让弱分页多造空白页。
    if not ("par", "list", "section", "chapter", "part", "abstract", "toc", "bib", "front").contains(k) { place(flow-probe("boundary")) }
    let after = prev
    prev = k
    if k == "par" { para(b.c) }
    else if k == "aligned" { aligned(b) }
    else if k == "list" { list-par(b) }
    else if k == "chapter" { chapter(b, id: "gw-chapter-" + str(i)) }
    else if k == "section" { section(b, prev: after) }
    else if k == "part" {
      if b.number != none { part-serial += 1 }
      let children = ()
      if terminal {
        for (j, child) in blocks.slice(i + 1).enumerate() {
          if child.k == "part" { break }
          if child.k == "chapter" {
            if not child.toc or (child.prefix != none and child.prefix.starts-with("附录")) { break }
            children.push((b: child, id: "gw-chapter-" + str(i + 1 + j)))
          }
        }
      }
      part(b, children: children, serial: if b.number != none { str(part-serial) })
    }
    else if k == "table" { table-block(b, prev: after) }
    else if k == "figure" { figure-block(b) }
    else if k == "code" { code-block(b) }
    else if k == "math" { display-math(b) }
    else if k == "quote" { quote-block(b) }
    else if k == "box" { box-block(b, prev: after) }
    else if k == "bib" { bib-block(b) }
    else if k == "front" { }
    else if k == "abstract" { abstract(b, render) }
    else if k == "toc" {
      toc-block()
      // 目录之后恢复阿拉伯页码，接着目录之前的页号：摘要单独编页时从 1 起。
      clear-empty(num: (style: "1", start: if front { 1 } else { "resume" }))
    }
  }
}

#cover()
// \mainmatter：正文从阿拉伯页码 1 起；摘要单独编页时摘要用小写罗马页码。
// 目录在首块时由目录自己从封面另起页，避免两次强制分页制造一组空白页。
#if not (terminal and doc.blocks.len() > 0 and doc.blocks.first().k == "toc") {
  clear-empty(num: if front { (style: "i", start: 1) } else { (style: "1", start: 1) })
}
#render(doc.blocks)
