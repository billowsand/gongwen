// 公文模板（Typst 引擎）。
//
// 数据来自 /doc.json，由 src/export/typst 生成：编号、标题断行与压缩比例、表格列宽、
// 要素显示值、花脸稿标记都在 Rust 侧算好，这里只负责排。版式参数逐项对照
// gonghan-gwa.cls，凡是标了「实测」的数值都是对着内置 Tectonic 的 PDF 量出来的落点，
// 改动前先跑 Typst/TeX 对照测试（见 docs/typst-engine.md）。
//
// 坐标约定：未特别说明的竖向位置都是「相对版心顶」的基线位置；pt 是 PostScript 点，
// TeX 的 pt 另记为 texpt。

#let doc = json("/doc.json")
#let F = doc.fonts

// ---------------- 版式常量（gonghan-gwa.cls） ----------------
#let texpt = 25.4mm / 72.27
#let pitch = 28.98 * texpt                 // \BodyBaselineSkip
#let text-w = 156mm
#let text-h = 225mm
#let top-m = 37mm
#let inner-m = 28mm                        // 订口（单面时即左边距）
#let outer-m = 210mm - inner-m - text-w    // 26mm
#let red = rgb(255, 0, 0)
#let del-color = rgb("#C00000")
#let add-color = rgb("#1F4E9E")
#let topskip = 10 * texpt                  // 首页 picture 原点比版心顶低一个 \topskip

#let fam(main) = (main, ..F.fallback)
#let fs = fam(F.body)
#let kai = fam(F.heading2)
#let hei = fam(F.heading1)
#let xbs = fam(F.title)
#let song = fam(F.page_number)

// ---------------- 行内片段 ----------------
// 片段：t 文字；b 加粗；k 括号楷体四号；m 花脸稿（del / add）；g 中西文间隙（pt）；
// w 空占位（em，预览版的文号、日期留空）。

// 加粗：选了专用粗体字体就换字面，否则描边伪粗（xeCJK AutoFakeBold 的对应做法）。
// 描边颜色跟随文字颜色，删除线里的加粗字也是红的。
#let fake-bold(c) = context text(stroke: 0.02857em + text.fill, c)
#let bold(c) = if F.bold != none { text(font: fam(F.bold), stroke: none, c) } else { fake-bold(c) }

#let del-mark(c) = text(fill: del-color, strike(stroke: 0.6pt + del-color, offset: -0.32em, c))
// 新增框（\GwAdd）：0.5pt 竖边 + 1.5pt 留白再接文字，框随正文断行。Typst 的 highlight
// 向外扩边、不推文字，所以两侧各补 2pt 让文字落在与 TeX 相同的位置。
#let add-mark(c) = h(2pt) + highlight(fill: none, stroke: 0.5pt + add-color, top-edge: 0.96em,
  bottom-edge: -0.24em, extent: 1.5pt, c) + h(2pt)

#let one-run(r) = {
  if "g" in r { return h(r.g * 1pt, weak: true) }
  if "w" in r { return box(width: r.w * 1em) }
  let c = [#r.t]
  if r.at("k", default: false) { c = text(font: kai, size: 14pt, tracking: 0pt, c) }
  if r.at("b", default: false) { c = bold(c) }
  c
}

// 连续同类标注合成一组再套删除线 / 新增框：样式切换不打断标注。
#let runs(rs) = {
  let groups = ()
  for r in rs {
    let m = r.at("m", default: none)
    if groups.len() > 0 and groups.last().m == m {
      groups.last().items.push(r)
    } else {
      groups.push((m: m, items: (r,)))
    }
  }
  for g in groups {
    let c = g.items.map(one-run).join()
    if g.m == "del" { del-mark(c) } else if g.m == "add" { add-mark(c) } else { c }
  }
}
#let plain-of(rs) = rs.filter(r => "t" in r).map(r => r.t).join()

// 横向压窄：只压字宽、字高不变（TeX 的 \scalebox{s}[1]）。先按自然宽度排成一行再压；
// 外层再包 box——scale(reflow) 是块级元素，直接放进段落会被丢掉。
#let squeeze(s, body) = if s >= 1 { body } else {
  context {
    let w = measure(body).width
    box(scale(x: s * 100%, reflow: true, box(width: w, body)))
  }
}

// 姓名：2 字中间空 1 字，4 字压进 3 字宽、字高 0.9（TeX \resizebox{3em}{0.9em}）。
#let name-text(s) = {
  let cs = s.clusters()
  if cs.len() == 2 { cs.at(0) + h(1em) + cs.at(1) } else if cs.len() == 4 { box(scale(x: 75%, y: 90%, reflow: true, s)) } else { s }
}

// ---------------- 孤行探针 ----------------
// 段首、段尾各放一个带位置的 metadata，Rust 侧收集后拼成与 \GwaTail 同格式的报告。
// 份号逐份编制时只有第一份带探针（与 \GwCopyQuiet 一致）。
#let probing = state("gw-probing", doc.probe)
#let left-edge(page-no) = if doc.duplex and calc.even(page-no) { outer-m } else { inner-m }
#let probe-head(line) = if line != none {
  context if probing.get() {
    let p = here().position()
    [#metadata((line: line, page: p.page, y: p.y.pt(), bottom: (top-m + text-h).pt()))<gwa-head>]
  }
}
#let probe-tail(line, hsize: text-w) = if line != none {
  context if probing.get() {
    let p = here().position()
    [#metadata((line: line, page: p.page, x: p.x.pt(), y: p.y.pt(), char: 16.0,
      hsize: hsize.pt(), left: left-edge(counter(page).get().first()).pt(),
      pitch: pitch.pt(), top: top-m.pt()))<gwa-tail>]
  }
}

// ---------------- 正文块 ----------------
#let para(rs, line: none, hsize: text-w, indent: true) = {
  let body = probe-head(line) + runs(rs) + probe-tail(line, hsize: hsize)
  if indent { par(body) } else { par(first-line-indent: 0pt, body) }
}

#let heading-font(level) = if level == 2 { hei } else if level == 3 { kai } else { fs }
// 标题段：2 级黑体、3 级楷体、4/5 级仿宋合成加粗（固定伪粗，不随「专用粗体」）。
#let heading-content(level, rs) = {
  let c = text(font: heading-font(level), tracking: 0pt, runs(rs))
  if level >= 4 { fake-bold(c) } else { c }
}
#let heading-block(b, hsize: text-w) = par(
  probe-head(b.line) + heading-content(b.level, b.runs) + probe-tail(b.line, hsize: hsize))
// 紧缩：标题（带句号）与紧随正文接成一段。
#let compact-block(b, hsize: text-w) = par(
  probe-head(b.line) + heading-content(b.level, b.head) + runs(b.runs) + probe-tail(b.line, hsize: hsize))

#let aligned-block(b) = {
  let a = if b.align == "center" { center } else { right }
  align(a, par(first-line-indent: 0pt, justify: false, runs(b.runs)))
}

// 插图（TeX：\begin{center}\includegraphics[width=\textwidth]）。实测图前距上一行字底
// 3.75mm、图后距下一行字顶 8.9mm。块之间夹了硬间距 v 时 Typst 不再插段间距，图前直接给。
#let image-block(b) = {
  v(3.75mm, weak: false)
  block(above: 0pt, below: 8.9mm, width: 100%, align(center, image(b.src, width: 100%)))
}

// ---------------- 表格（longtblr） ----------------
// 列宽按 tabularray 的规则换算：X[比例] 按「内容宽」分配，每列左右各 6pt 留白另计；
// Q[wd=..] 是定宽内容（em 按表格四号 14pt）。行高 = 线 + 2pt + 21bp 支柱 + 2pt。
#let col-sep = 6 * texpt
#let rule-w = 0.4 * texpt
#let table-sep = 6.28mm                    // 实测：表前后距（tabularray presep/postsep）
#let table-widths(cols, total) = {
  let fixed = cols.filter(c => c.kind == "em").map(c => c.v * 14pt).sum(default: 0pt)
  let ratio = cols.filter(c => c.kind == "fr").map(c => c.v).sum(default: 0)
  let avail = total - cols.len() * 2 * col-sep - fixed
  cols.map(c => if c.kind == "em" { c.v * 14pt + 2 * col-sep } else { avail * c.v / ratio + 2 * col-sep })
}
#let cell-content(c, header) = {
  let body = if "name" in c { name-text(c.name) } else if "lines" in c { c.lines.map(runs).join(linebreak()) } else { runs(c.runs) }
  if header { body = text(font: hei, body) }
  squeeze(c.at("scale", default: 1), body)
}
#let continued-note = text(size: 14pt)[下一页继续]
#let table-block(t, total: text-w, after-table: false) = {
  let widths = table-widths(t.cols, total)
  let n = t.cols.len()
  let al(a) = if a == "l" { left } else if a == "r" { right } else { center }
  let mk(c, header) = {
    let args = (:)
    if c.at("colspan", default: 1) > 1 { args.colspan = c.colspan }
    if c.at("rowspan", default: 1) > 1 { args.rowspan = c.rowspan }
    table.cell(..args, align: al(c.align) + horizon, cell-content(c, header))
  }
  let id = t.id
  // 相邻两张表：TeX 的表后距与表前距相加，Typst 的块间距取大者，这里补足。
  if after-table { v(table-sep, weak: false) }
  block(above: table-sep, below: table-sep, {
    // \fontsize{14bp}{21bp}：每行一个 21bp 的支柱（基线上 0.7、下 0.3），行距即 21bp。
    set text(size: 14pt, tracking: 0pt, top-edge: 0.7 * 21pt, bottom-edge: -0.3 * 21pt)
    set par(leading: 0pt, spacing: 0pt, first-line-indent: 0pt, justify: false)
    table(
      columns: widths,
      // TeX 的横线占高度，Typst 的线压在格边上：补半根线宽。
      inset: (x: col-sep, y: 2 * texpt + rule-w / 2),
      stroke: rule-w + black,
      table.header(..t.rows.at(0).map(c => mk(c, true))),
      ..t.rows.slice(1).map(r => r.map(c => mk(c, false))).flatten(),
      // 续表提示（tabularray 的 contfoot）：每页重复的表尾，只在表格还没结束的页上
      // 有内容；最后一页内容为空、留白为 0，不占高度。
      table.footer(repeat: true, table.cell(colspan: n, stroke: none, inset: 0pt, align: right,
        context {
          let end = locate(label("gw-tbl-end-" + id)).page()
          if here().page() < end { block(above: 0pt, inset: (top: 2 * texpt), continued-note) }
        })),
    )
    [#metadata(id)#label("gw-tbl-end-" + id)]
  })
}

#let render-blocks(blocks, total: text-w, hsize: text-w) = {
  let prev = none
  for b in blocks {
    if b.k == "par" { para(b.runs, line: b.at("line", default: none), hsize: hsize) } else if b.k == "heading" { heading-block(b, hsize: hsize) } else if b.k == "compact" { compact-block(b, hsize: hsize) } else if b.k == "aligned" { aligned-block(b) } else if b.k == "table" { table-block(b, total: total, after-table: prev == "table") } else if b.k == "image" { image-block(b) }
    prev = b.k
  }
}

// ---------------- 页面与页码 ----------------
// 「—~页码~—」：两侧是 TeX 的 ~，即宋体四号的词间空（0.5em）。
#let page-number-text(n) = text(font: fs, size: 14pt, tracking: 0pt, top-edge: "baseline",
  bottom-edge: "baseline", [—#h(0.5em)#text(font: song)[#n]#h(0.5em)—])

// 竖页页码：基线在版心底下 \footskip=30pt；单面居中，双面奇右偶左（按页码奇偶）。
// 每份文件（份号逐份编制）的首页不印页码。
#let portrait-number = context {
  let n = counter(page).get().first()
  if n > 1 {
    let a = if not doc.duplex { center } else if calc.odd(n) { right } else { left }
    place(top + left, dx: left-edge(n), dy: top-m + text-h + 30 * texpt,
      box(width: text-w, align(a, page-number-text(n))))
  }
}

#set page(paper: "a4",
  margin: if doc.duplex { (inside: inner-m, outside: outer-m, top: top-m, bottom: 297mm - top-m - text-h) } else { (left: inner-m, right: outer-m, top: top-m, bottom: 297mm - top-m - text-h) },
  binding: left,
  background: portrait-number)

#set text(font: fs, size: 16pt, lang: "zh", region: "cn", top-edge: 13.6pt, bottom-edge: -2.4pt,
  tracking: 0pt, overhang: false, cjk-latin-spacing: none,
  // TeX 不罚段末孤字（xeCJK CheckSingle 只管极窄的情形，见 cls 注释）；Typst 默认罚，会为躲
  // 末行单字把前面几行的断点全部挪动。孤行改由探针报到审校面板。
  costs: (runt: 0%))
// 断行与两端对齐按 xeCJK 的规矩：汉字固定一个字宽（三号 16pt，不压字距），行太长时
// 只压标点（上限见 vendor/typst-layout 的补丁），行太短时字间拉开，上限取 CJKglue 的
// plus 0.08\baselineskip。
#set par(justify: true, leading: pitch - 16pt, spacing: pitch - 16pt,
  justification-limits: (tracking: (min: 0pt, max: 0.08 * pitch)),
  first-line-indent: (amount: 2em, all: true))
#set strike(background: false)

// ---------------- 通用部件 ----------------
// 在给定基线放一行（top-edge 设为基线，place 的 dy 就是基线位置，相对版心顶）。
#let at-baseline(dx: 0mm, y, width: text-w, align-to: left, body) = place(top + left, dx: dx, dy: y,
  box(width: width, align(align-to, text(top-edge: "baseline", bottom-edge: "baseline", body))))

#let line-par(body) = par(first-line-indent: 0pt, justify: false, text(tracking: 0pt, body))

#let security-line = if doc.security != none { line-par(text(font: hei, runs(doc.security))) }

// 标题：程序给出各行与压缩比；size 默认二号 22pt。
#let title-block(t, size: 22pt) = {
  set par(first-line-indent: 0pt, justify: false)
  set text(font: xbs, size: size, tracking: 0pt)
  for line in t.lines { align(center, par(squeeze(t.scale, runs(line)))) }
}

#let recipient-line = if doc.recipient != none {
  line-par(text(font: kai, runs(doc.recipient) + "："))
}

// 附件说明：空两行，各行首行缩进两字。多个附件时序号对齐（\phantom{附件}）。
#let summary-block = if doc.summary.len() > 0 {
  v(2 * pitch, weak: false)
  for (i, line) in doc.summary.enumerate() {
    let prefix = if doc.summary.len() == 1 or i == 0 { [附件] } else { hide[附件] }
    par(prefix + runs(line))
  }
}

// 红头：字距撑满 156mm，单字间距上限 1em；排不下则整体横向压缩（\IssuingUnitHeader）。
#let issuing-header(name) = context {
  let size = 29 * texpt
  let t(s) = text(font: xbs, size: size, fill: red, tracking: 0pt, s)
  let chars = name.clusters()
  let natural = measure(t(name)).width
  if natural >= text-w { scale(x: text-w / natural * 100%, reflow: true, t(name)) } else if chars.len() < 2 { t(name) } else { chars.map(c => t(c)).join(h(calc.min((text-w - natural) / (chars.len() - 1), size))) }
}

// ---------------- 落款 ----------------
// 函稿落款（\SignatureContent）：11cm 宽的盒子靠右，单位与日期在盒内居中。
// 一个块、块外不留间距：位置完全由调用方的留白决定，量高与实排一致。
#let letter-signature(c) = block(width: text-w, above: 0pt, below: 0pt, {
  set par(first-line-indent: 0pt, justify: false)
  set text(tracking: 0pt)
  align(right, box(width: 11cm, align(center, runs(c.unit) + linebreak() + runs(c.date))))
})

// 联合发文落款（\JointSignatureContent）：两列各 72mm、列间 12pt（tabular 的 	abcolsep×2），
// 整表居中；单位多于两个时行间留 45mm 盖章位，奇数个时末一个跨两列居中；之后空 6mm
// 排成文日期，压在主发文单位那一列下。行距取 TeX 实测：单位行基线间 6.77mm（留盖章位时
// 51.77mm），末行到日期 13.19mm；首行基线在块顶下 8.2mm（center 环境的 topsep）。
#let joint-signature(c) = block(width: text-w, above: 0pt, below: 0pt, {
  set par(first-line-indent: 0pt, justify: false, spacing: 0pt)
  set text(tracking: 0pt)
  let col = 72mm
  let sep = 12 * texpt
  let row-gap = if c.gaps { 51.77mm } else { 6.77mm }
  // align 必须在 par 外面：段落里的 align 不起作用。
  let line(body) = align(center, par(body))
  let two(a, b) = box(width: col, align(center, a)) + h(sep) + box(width: col, align(center, b))
  v(8.2mm - 13.6pt, weak: false)
  for (i, row) in c.rows.enumerate() {
    if i > 0 { v(row-gap - 16pt, weak: false) }
    if row.right == none {
      line(box(width: 2 * col + sep, align(center, runs(row.left))))
    } else {
      line(two(runs(row.left), runs(row.right)))
    }
  }
  v(13.19mm - 16pt, weak: false)
  let date = runs(c.date)
  line(if c.date-column == 0 { two(date, []) } else if c.date-column == 1 { two([], date) } else { date })
  // center 环境结束处的 topsep：不印东西，但 TeX 量落款高度时算在里面，影响空几行。
  v(8.2mm - 13.6pt, weak: false)
})

// 带签字空间的落款（白头件、红头呈批件，\gwa@roomsignature）：单位右对齐、右侧留
// 4cm 签字；单位之间空一行；末单位后空一行排日期，日期居中于「单位块 + 签字空间」。
#let room-signature(c) = block(width: text-w, above: 0pt, below: 0pt, {
  set par(first-line-indent: 0pt, justify: false)
  set text(tracking: 0pt)
  let room = 4cm
  let uw = c.unit-width-mm * 1mm
  for (i, u) in c.units.enumerate() {
    if i > 0 { v(pitch, weak: false) }
    par(align(right, box(width: text-w - room, align(right, runs(u))) + h(room)))
  }
  v(pitch, weak: false)
  par(h(text-w - room - uw) + box(width: uw + room, align(center, runs(c.date))))
})

#let signature-of(c) = if c.k == "letter" { letter-signature(c) } else if c.k == "joint" { joint-signature(c) } else { room-signature(c) }

// 落款定位（\gwa@placeclosing）：先试空 3 行，放不下依次试 2 行、1 行；都放不下就
// 另起一页并标「此页无正文」。extra 是要一并容纳的版记等高度。
// 位置从落款**之前**的锚点取：若用 context 自身的 here()，它输出的分页会反过来改变
// 自己的位置，迭代不收敛。锚点包进零高的块：裸 metadata 紧挨分页会被带到下一页。
// notice-base / notice-gap：「此页无正文」基线距版心顶、它与落款首行基线之差（实测，
// 函稿与带签字空间的落款走的不是同一段宏，留白不同）。
#let closing-serial = counter("gw-closing")
#let place-closing(closing, extra: 0pt, notice-base: 29.95mm, notice-gap: 44.25mm, first-gap: 0mm) = {
  closing-serial.step()
  context {
    let id = "gw-closing-" + str(closing-serial.get().first())
    block(height: 0pt, spacing: 0pt, [#metadata(id)#label(id)])
  }
  context {
    let id = "gw-closing-" + str(closing-serial.get().first())
    let y = locate(label(id)).position().y
    let bottom = top-m + text-h
    // 「留白 + 落款」装成一个块，量的就是实际排出去的那个块（TeX 同样装箱量高再 \unvbox）。
    let boxed(n) = block(width: text-w, above: 0pt, below: 0pt, breakable: false, {
      v(n * pitch + first-gap, weak: false)
      closing
    })
    let fit = (3, 2, 1).find(n => y + measure(boxed(n)).height + extra <= bottom)
    if fit != none {
      boxed(fit)
    } else {
      pagebreak()
      // \NoBodyNotice：\vspace*{58pt} 后一行；之后空 3 行接落款。
      v(notice-base - 13.6pt, weak: false)
      par(text("（此页无正文）"))
      v(notice-gap - 2.4pt - 13.6pt, weak: false)
      closing
    }
  }
}

// ---------------- 版记 ----------------
// 几何取 TeX 实测（booktabs 线距），以块顶为原点、单位 mm：
//   单条承办（\arraystretch=1）：顶线中心 0.30、抄送行基线 5.84、中线 8.41、承办行基线 13.80、
//     底线 16.52；
//   联合发文（\arraystretch=1.15，承办逐行列出）：顶线 0.30、抄送 6.41、中线 9.24、首行 15.20、
//     其后每行 +6.81、末行到底线 2.99。
// 块底贴版心底，底线比版心底再低 1.46mm，与 TeX 一致。
#let record-geometry(r) = {
  let k = calc.max(r.rows.len(), 1)
  if r.joint {
    let last = 15.20 + (k - 1) * 6.81
    (copies: 6.41, mid: 9.24, first: 15.20, step: 6.81, bottom: last + 2.99)
  } else {
    (copies: 5.84, mid: 8.41, first: 13.80, step: 6.81, bottom: 16.52)
  }
}
#let record-height(r) = (record-geometry(r).bottom - 1.46) * 1mm
// TeX 量版记（\gwa@closingneed）用的是整个 vbox：顶线上缘到底线下缘，再加 2pt 余量
// （\ClosingMeasureSlack）。决定落款空几行时按这个算，才与 TeX 的取舍一致。
#let record-need(r) = (record-geometry(r).bottom + 0.3) * 1mm + 2 * texpt
#let copies-note(r) = [（共印#r.print-copies;份）]
#let copies-line(r) = {
  if r.copies-to != none {
    // 抄送单位悬挂缩进 3 字；「（共印 N 份）」钉在最后一行右端，放不下就换行贴右。
    context {
      let note = copies-note(r)
      let head = box(width: 3em, [抄送：])
      let body = head + runs(r.copies-to)
      let w = measure(body).width
      let room = text-w - w
      if room >= (measure(note).width + 2em).to-absolute() { body + h(1fr) + note }
      else { body + linebreak() + h(1fr) + note }
    }
  } else { h(1fr) + copies-note(r) }
}
#let record-block(r) = {
  let g = record-geometry(r)
  block(width: text-w, height: record-height(r), breakable: false, above: 0pt, below: 0pt, {
    set text(size: 14pt, tracking: 0pt)
    set par(first-line-indent: 0pt, justify: false, hanging-indent: 3em, leading: 22.56pt - 14pt)
    let rule(y, w) = place(top + left, dy: y * 1mm - w / 2, line(length: text-w, stroke: w))
    let at(y, body) = place(top + left, dy: y * 1mm, box(width: text-w,
      text(top-edge: "baseline", bottom-edge: "baseline", body)))
    rule(0.30, 0.6mm)
    at(g.copies, copies-line(r))
    rule(g.mid, 0.3mm)
    for (i, row) in r.rows.enumerate() {
      let (lu, lc, lp) = if i == 0 { ([承办单位：], [联系人：], [联系电话：]) }
        else { (h(5em), h(4em), []) }
      at(g.first + i * g.step, grid(columns: (1fr, 1fr, 11em), align: (left, center, right),
        lu + row.unit, lc + name-text(row.contact), lp + row.phone))
    }
    rule(g.bottom, 0.6mm)
  })
}
#let footer-record(r) = { v(1fr); record-block(r) }

// ---------------- 附件 ----------------
// 「附件」标识（黑体三号）、空一行、附件标题，标题之后直接接内容。
// 横向附件用横页：页码仍在竖页时的位置（横页左缘、旋转 90°），与 TeX 的 pdflscape 一致。
#let landscape-number = context {
  let n = counter(page).get().first()
  let a = if not doc.duplex { horizon } else if calc.odd(n) { bottom } else { top }
  place(top + left, dx: 297mm - (top-m + text-h + 30 * texpt), dy: left-edge(n),
    box(height: text-w, align(a, rotate(90deg, reflow: true, page-number-text(n)))))
}
#let attachment(a) = {
  let content(total) = {
    line-par(text(font: hei, a.label))
    v(pitch, weak: false)
    title-block(a.title)
    render-blocks(a.blocks, total: total)
  }
  if a.landscape {
    // 横页上缘是竖页的左边距：双面印刷时偶数页左边距是外侧 26mm。这个 context 的
    // 位置已经落在新开的横页上，页码直接取当前值（横向附件跨多页时后续页沿用同一组边距）。
    context {
      let n = counter(page).get().first()
      let (top, bottom) = if doc.duplex and calc.even(n) { (outer-m, inner-m) } else { (inner-m, outer-m) }
      page(flipped: true,
        margin: (left: 297mm - top-m - text-h, right: top-m, top: top, bottom: bottom),
        background: landscape-number, content(text-h))
    }
  } else {
    pagebreak()
    content(text-w)
  }
}

// ================= 红头呈批件 =================
// 首页：框线、红头、文号、批示栏按页面绝对坐标；承办区量出真实高度贴版心底；
// 正文先排在 96mm 窄栏，额度满后把一段切开、余下的续排到第二页全宽（\parshape 的替代）。
#let narrow-w = 96mm
#let line-count(content, width) = {
  let h = measure(block(width: width, content)).height
  int(calc.round((h + (pitch - 16pt)) / pitch))
}
#let take-runs(rs, k) = {
  let out = ()
  for r in rs {
    if k <= 0 { break }
    if "t" not in r { out.push(r); continue }
    let cs = r.t.clusters()
    let piece = r
    piece.t = cs.slice(0, calc.min(k, cs.len())).join()
    out.push(piece)
    k -= cs.len()
  }
  out
}
#let drop-runs(rs, k) = {
  let out = ()
  for r in rs {
    if "t" not in r { if k <= 0 { out.push(r) }; continue }
    let cs = r.t.clusters()
    if k >= cs.len() { k -= cs.len(); continue }
    let piece = r
    piece.t = cs.slice(calc.max(k, 0)).join()
    out.push(piece)
    k = 0
  }
  out
}
#let char-count(rs) = rs.filter(r => "t" in r).map(r => r.t.clusters().len()).sum(default: 0)
#let no-line-start = "，。、；：？！）》」』”’…—%"

#let red-split(blocks, quota) = {
  let first = ()
  let rest = ()
  let used = 0
  let continued = false
  for b in blocks {
    if continued { rest.push(b); continue }
    if b.k == "barrier" or b.k == "table" or b.k == "image" {
      // 表格与图片不留在首页：从这里起全部送到第二页。
      continued = true
      if b.k != "barrier" { rest.push(b) }
      continue
    }
    let c = if b.k == "par" { para(b.runs) } else if b.k == "heading" { heading-block(b) } else if b.k == "compact" { compact-block(b) } else { aligned-block(b) }
    let n = line-count(c, narrow-w)
    if used + n <= quota { first.push(b); used += n; continue }
    continued = true
    let r = quota - used
    if (b.k != "par") or r <= 0 { rest.push(b); continue }
    // 二分找能在 r 行内排下的最长前缀；续排部分不得以行首禁则标点开头。
    let (lo, hi) = (0, char-count(b.runs))
    while lo < hi {
      let mid = calc.div-euclid(lo + hi + 1, 2)
      if line-count(para(take-runs(b.runs, mid)), narrow-w) <= r { lo = mid } else { hi = mid - 1 }
    }
    while lo > 0 {
      let tail = drop-runs(b.runs, lo).filter(r => "t" in r)
      if tail.len() == 0 or not no-line-start.contains(tail.first().t.clusters().first()) { break }
      lo -= 1
    }
    if lo == 0 { rest.push(b); continue }
    first.push((k: "p-head", runs: take-runs(b.runs, lo), line: b.at("line", default: none)))
    rest.push((k: "p-tail", runs: drop-runs(b.runs, lo), line: b.at("line", default: none)))
  }
  (first: first, rest: rest, continued: continued)
}
#let red-render-first(b) = {
  if b.k == "p-head" {
    // 截断的前半段：末行两端对齐，读起来接着下一页。
    par(probe-head(b.line) + runs(b.runs) + linebreak(justify: true))
  } else { render-blocks((b,), hsize: narrow-w) }
}
#let red-render-rest(blocks) = {
  for b in blocks {
    if b.k == "p-tail" {
      par(first-line-indent: 0pt, runs(b.runs) + probe-tail(b.line))
    } else { render-blocks((b,)) }
  }
}

#let red-record-block(rd) = {
  let (cu, cc, cp) = rd.cols-mm.map(v => v * 1mm)
  let gutter = 5.64mm
  let row(i, r) = {
    let lab(s) = text(fill: red, s)
    let (u, c, p) = if i == 0 {
      (lab[承办单位：] + r.unit, lab[联系人：] + name-text(r.contact), lab[电话：] + r.phone)
    } else {
      (h(5em) + r.unit, h(4em) + name-text(r.contact), [#r.phone])
    }
    let fit(w, body) = context {
      let natural = measure(body).width
      if natural <= w { body } else { box(scale(x: w / natural * 100%, reflow: true, box(width: natural, body))) }
    }
    par(first-line-indent: 0pt, justify: false,
      box(width: cu, fit(cu - gutter, u)) + box(width: cc, fit(cc - gutter, c))
        + box(width: cp, align(right, fit(cp, p))))
  }
  block(width: text-w, spacing: 0pt, {
    set block(spacing: 0pt)
    set text(tracking: 0pt)
    block(rect(width: text-w, height: 0.4mm, fill: red, stroke: none))
    v(1mm, weak: false)
    set par(spacing: pitch - 16pt)
    block(for (i, r) in rd.rows.enumerate() { row(i, r) })
  })
}

#let red-approval(serial) = context {
  let rec = red-record-block(doc.red)
  let rec-h = calc.min(measure(rec).height, text-h - 90mm)
  let rec-top = topskip + text-h - rec-h
  let frame-h = text-h - 48mm - rec-h
  if doc.security != none { at-baseline(topskip + 10mm, text(font: hei, tracking: 0pt, runs(doc.security))) }
  at-baseline(topskip + 30mm, align-to: center, issuing-header(doc.header.issuing))
  at-baseline(topskip + 43mm, align-to: center, text(tracking: 0pt, runs(doc.header.number)))
  place(top + left, dy: topskip + 48mm - 0.4mm, rect(width: text-w, height: 0.4mm, fill: red, stroke: none))
  place(top + left, dx: 100mm, dy: topskip + 48mm, rect(width: 0.4mm, height: frame-h, fill: red, stroke: none))
  at-baseline(dx: 100mm, topskip + 61mm, width: 56mm, align-to: center, text(fill: red, tracking: 0pt)[批#h(1em)示])
  place(top + left, dy: rec-top, rec)

  // 首页窄栏：标题、空行、呈报领导、正文，全部落在 28.98pt 的基线网格上。
  // 栏顶取「第一条标题基线 = 原点 + 55mm + 小标宋字身高」（实测）。
  let first-baseline = topskip + 55mm + 15.08pt
  let col-top = first-baseline - 13.6pt
  let title = block(width: narrow-w, title-block(doc.title, size: 18pt))
  let title-lines = doc.title.lines.len()
  // 网格行数：最后一条基线 + 字身下缘距承办区红线不少于 2mm（\RedFirstPageRemaining）。
  // TeX 实测：呈报领导之后的第一段正文比网格低 0.58mm（\unvbox 之后接段落时 \prevdepth
  // 的缘故），首页其余正文行随之整体下移，额度也按下移后的位置算。
  let body-shift = 0.58mm
  let body-first = first-baseline + (title-lines + 2) * pitch + body-shift
  let quota = int(calc.floor((rec-top - 2mm - body-first - 2.4pt) / pitch)) + 1
  let parts = red-split(doc.body, quota)

  v(col-top, weak: false)
  block(width: narrow-w, spacing: 0pt, {
    title
    v(pitch, weak: false)
    if doc.recipient != none { line-par(text(font: kai, runs(doc.recipient) + "：")) }
    // 硬间距叠在段间距之上（弱间距会与段间距合并成一个，起不到下移的作用）。
    v(body-shift, weak: false)
    for b in parts.first { red-render-first(b) }
  })
  pagebreak()
  red-render-rest(parts.rest)
  // 附件说明统一从第二页起排（含两行固定留白，首页额度判断不了）。
  summary-block
  // 落款不得出现在首页：正文续排了就紧随正文，否则另起一页标「此页无正文」。
  let closing = signature-of(doc.closing)
  if parts.continued or parts.rest.len() > 0 or doc.summary.len() > 0 {
    // 实测：带签字空间的落款，末行基线到落款首行基线 35.98mm（3 行 + 字身上下缘 − 0.22mm）。
    place-closing(closing, notice-base: 30.67mm, notice-gap: 40.74mm, first-gap: -0.22mm)
  } else {
    v(30.67mm - 13.6pt, weak: false)
    par(text("（此页无正文）"))
    v(40.74mm - 2.4pt - 13.6pt, weak: false)
    closing
  }
  for a in doc.attachments { attachment(a) }
}

// ================= 其余文种 =================
#let letter-header(serial) = {
  at-baseline(3.70mm, align-to: center, issuing-header(doc.header.issuing))
  place(top + left, dy: 9.19mm - 0.265mm, rect(width: text-w, height: 0.53mm, fill: red, stroke: none))
  // 份号/文号行基线在版心顶下 19.64mm（实测）：流式排版，后面的行按网格接下去。
  v(19.64mm - 13.6pt, weak: false)
  if doc.header.number != none {
    line-par(text(font: hei, serial) + h(1fr) + runs(doc.header.number))
  }
  security-line
}

#let letter-like(serial) = {
  let kind = doc.kind
  if kind == "letter" or kind == "phone" {
    letter-header(serial)
    v(pitch, weak: false)
  } else if kind == "whitepaper" {
    // 白头件：顶格密级，密级后空 10 行。
    if doc.security != none { security-line }
    v(10 * pitch, weak: false)
  } else if kind == "agenda" {
    if doc.security != none { security-line }
    v(pitch, weak: false)
  } else {
    // 普通公文：只有可选的密级行（后空一行）。
    if doc.security != none { security-line; v(pitch, weak: false) }
  }
  title-block(doc.title)
  v(pitch, weak: false)
  if kind != "plain" and kind != "agenda" { recipient-line }
  render-blocks(doc.body)
  summary-block
  if doc.closing != none {
    let closing = signature-of(doc.closing)
    if kind == "letter" and doc.attachments.len() == 0 and doc.record != none {
      // 无附件时版记与落款同页：落款之下至少留 1cm 再放版记。
      place-closing(closing, extra: record-need(doc.record) + 1cm,
        first-gap: if doc.closing.k == "joint" { 0mm } else { 3.32mm })
    } else if kind == "whitepaper" {
      place-closing(closing, notice-base: 30.67mm, notice-gap: 40.74mm, first-gap: -0.22mm)
    } else {
      // 函稿落款在 flushright + minipage[t] 里，实测比「3 行 + 字身」再低 3.32mm。
      place-closing(closing, first-gap: 3.32mm)
    }
  }
  for a in doc.attachments { attachment(a) }
  if kind == "letter" and doc.record != none {
    if doc.attachments.len() > 0 and doc.attachments.last().landscape { pagebreak() }
    footer-record(doc.record)
  }
}

#let render-copy(serial) = if doc.kind == "redapproval" { red-approval(serial) } else { letter-like(serial) }

// 份号逐份编制：每份换页、页码归 1；只有第一份带孤行探针。
#if doc.copies.len() == 0 {
  render-copy("01")
} else {
  for (i, serial) in doc.copies.enumerate() {
    if i > 0 {
      // 页码在每页开头自动加一，所以在上一份末尾归零，新一份首页才是 1；
      // 放在分页之后的话，新页页脚取到的还是旧值。
      counter(page).update(0)
      probing.update(false)
      pagebreak()
    }
    render-copy(serial)
  }
}
