// 公函签收记录单模板（Typst 引擎）。
//
// 数据来自 /doc.json，由 src/export/typst/receipt.rs 生成：要素显示值、签收单位清单
// （主送在前、抄送在后、按最终显示值去重）、表格列宽都在 Rust 侧算好，这里只负责排。
// 版式：A4 纵向、黑白、156mm 版心；题名小标宋二号居中；要素行黑体标签 + 仿宋取值；
// 签收表五列（序号 / 签收单位 / 签收方式 / 签收人 / 签收日期），表头黑体四号、
// 内容仿宋四号，外框稍粗、内部细线，内容行至少 18mm 供手写签名；跨页重复表头、行不拆分；
// 表下送文信息行距放宽到 12mm，横线上方留足书写高度；页码「— N —」每页都印。
// 密级只在要素行印一次，不在表格里逐行重复。

#let doc = json("/doc.json")
#let F = doc.fonts

// ---------------- 版式常量 ----------------
#let texpt = 25.4mm / 72.27
#let text-w = 156mm
#let text-h = 225mm
#let top-m = 37mm
#let bottom-m = 35mm
#let left-m = 28mm
#let right-m = 26mm
#let table-size = 14pt          // 四号
#let title-size = 22pt          // 二号
#let line-pitch = 22pt          // 要素行的基线距
#let row-pitch = 21pt           // 表格行的基线距
#let footer-pitch = 12mm        // 送文信息（手写）行的基线距

// 字体角色在应用内是字符串，导出的 Typst 包里是一串系统字体别名（逐个别名回退）。
#let fam(main) = if type(main) == array { (..main, ..F.fallback) } else { (main, ..F.fallback) }
#let fs = fam(F.body)
#let hei = fam(F.heading1)
#let xbs = fam(F.title)
#let song = if F.page_number_latin != none { (F.page_number_latin, ..fam(F.page_number)) } else { fam(F.page_number) }

// ---------------- 页面与页码 ----------------
// 「—~页码~—」：两侧是词间空（0.5em）。签收单每页都印页码（公文首页不印）。
#let page-number-text(n) = text(font: fs, size: table-size, tracking: 0pt, top-edge: "baseline",
  bottom-edge: "baseline", [—#h(0.5em)#text(font: song)[#n]#h(0.5em)—])

#let receipt-number = context {
  let n = counter(page).get().first()
  // 背景层的坐标从纸边算起，要先平移一个左边距，页码才在版心里居中。
  place(top + left, dx: left-m, dy: top-m + text-h + 30 * texpt,
    box(width: text-w, align(center, page-number-text(n))))
}

#set page(paper: "a4", margin: (top: top-m, bottom: bottom-m, left: left-m, right: right-m),
  background: receipt-number)

#set text(font: fs, size: table-size, lang: "zh", region: "cn", tracking: 0pt,
  top-edge: 0.85 * table-size, bottom-edge: -0.15 * table-size,
  cjk-latin-spacing: none, costs: (runt: 0%, orphan: 0%, widow: 0%))
#set par(justify: false, leading: line-pitch - table-size, spacing: 0pt, first-line-indent: 0pt)

// ---------------- 通用部件 ----------------
// 留白：一条与 n 个全角字等长的横线，供手写。Word 侧是带下划线的全角空格，两端一样长。
#let blank(n) = box(width: n * 1em, height: 0.62em, baseline: bottom,
  stroke: (bottom: 0.4pt), outset: (bottom: 1.5pt))

// 一处「标签 + 取值」：取值为空就留出手写空间。
#let field-value(f) = if f.value != "" { text(font: fs, f.value) } else { blank(f.blank_chars) }

// 一行要素：若干组「标签 + 取值」，组间空一个全角字。
#let field-content(fields) = {
  let parts = ()
  for (i, f) in fields.enumerate() {
    parts.push(text(font: hei, f.label))
    parts.push(field-value(f))
    if i < fields.len() - 1 { parts.push(text(font: fs, "　")) }
  }
  parts.join()
}

// 若干要素行：一行一段，基线距 `pitch`。取值过长折行时悬挂缩进到首个标签之后，
// 续行与取值对齐，不顶到标签底下。
#let field-lines(rows, pitch: line-pitch) = {
  set par(leading: pitch - table-size, spacing: pitch - table-size)
  for row in rows {
    let hang = row.at(0).label.clusters().len() * 1em
    par(hanging-indent: hang, field-content(row))
  }
}

// ---------------- 签收表 ----------------
#let widths = doc.column_widths_mm.map(v => v * 1mm)
#let n-cols = widths.len()
#let row-count = doc.rows.len() + 1

// 每一格各自画自己的四条边，所以外框要逐格指定：外框 1.2pt，内部 0.5pt。
// 表格级的 stroke 回调分不出「这一格的左边还是右边」，只能逐格给字典。
#let cell-stroke(x, y) = {
  let thick = (:)
  if x == 0 { thick.left = 1.2pt }
  if x == n-cols - 1 { thick.right = 1.2pt }
  if y == 0 { thick.top = 1.2pt }
  if y == row-count - 1 { thick.bottom = 1.2pt }
  if thick.len() == 0 { (rest: 0.5pt) } else { thick + (rest: 0.5pt) }
}

// 表头：黑体四号居中，上下各 2mm。
#let header-cells = doc.columns.enumerate().map(pair => table.cell(
  inset: (x: 1mm, y: 2mm),
  align: center + horizon,
  stroke: cell-stroke(pair.at(0), 0),
  text(font: hei, size: table-size, pair.at(1)),
))

// 一行签收记录：签收方式的两个勾选项上下叠放，签收人与签收日期留白。
#let sign-method-text = doc.sign_methods.map(m => "□" + m).join(linebreak())
// 最小行高靠序号格里一个定高的块撑出来（定高块不会被内容撑大，其余格垂直居中）；
// 长单位名换行超过最小行高时，整行随单位格继续增长。
#let row-inset-y = 1mm
#let row-strut(body) = block(width: 100%, height: doc.row_min_height_mm * 1mm - 2 * row-inset-y,
  align(center + horizon, body))
#let body-cells = doc.rows.enumerate().map(row-pair => {
  let y = row-pair.at(0) + 1
  let r = row-pair.at(1)
  let texts = (
    row-strut(text(font: fs, size: table-size, r.index)),
    text(font: fs, size: table-size, r.unit),
    text(font: fs, size: table-size, sign-method-text),
    [],
    [],
  )
  let aligns = (center, left, center, center, center)
  texts.enumerate().map(cell-pair => table.cell(
    inset: (x: 1mm, y: row-inset-y),
    align: aligns.at(cell-pair.at(0)) + horizon,
    stroke: cell-stroke(cell-pair.at(0), y),
    cell-pair.at(1),
  ))
}).flatten()

#let receipt-table = block(above: 0pt, below: 0pt, {
  set text(font: fs, size: table-size, tracking: 0pt,
    top-edge: 0.7 * row-pitch, bottom-edge: -0.3 * row-pitch)
  set par(leading: 0pt, spacing: 0pt, first-line-indent: 0pt, justify: false)
  table(
    columns: widths,
    inset: (x: 1mm, y: 1mm),
    stroke: 0.5pt,
    table.header(..header-cells),
    ..body-cells,
  )
})

// ---------------- 正文 ----------------
#align(center, par(text(font: xbs, size: title-size, tracking: 0pt, doc.title)))
#v(6mm, weak: false)

#field-lines(doc.fields)
#v(4mm, weak: false)
#receipt-table
// 送文信息是手写栏：第一行横线距表格底边也留一个手写行距，两行整体不拆页。
#block(breakable: false, above: 0pt, below: 0pt, {
  v(footer-pitch - table-size, weak: false)
  field-lines(doc.footer, pitch: footer-pitch)
})
