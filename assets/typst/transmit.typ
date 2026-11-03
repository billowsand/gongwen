// 公函电子加密传输单（《传发任务单》）模板（Typst 引擎）。
//
// 数据来自 /doc.json，由 src/export/typst/transmit.rs 生成：题名（传输任务名）、
// 传字号、主表每一格（分组标签、小节标签、取值）都在 Rust 侧算好，这里只负责排。
// 版式照用户给的 Word 模板复刻：A4 纵向；题名「{传输任务名}」用方正小标宋红色居中；
// 传字号黑体右对齐；主表五列（分组标签 / 小节标签 / 取值 / 副标签 / 副取值），
// 分组标签竖排；表下是机要表两行与「{保密单位} … 收到 / 发出」末行。
//
// 页边距、字号、行距、留白都从数据里来：排版发现超出一页时，Rust 侧会把这些值
// 收紧一档重排（见 transmit.rs 的收紧阶梯），这里只管按给的排。
// 字体里没有 ☑ 字形（GB2312 不含），空框与打勾框都用线画。

#let doc = json("/doc.json")
#let F = doc.fonts

// ---------------- 版式常量 ----------------
#let left-m = 28mm
#let right-m = 26mm
#let note-size = 12pt           // 备注、末行：小四
#let small-size = 11pt          // 机要表提示行
#let cell-inset-x = 1mm         // 单元格左右留白
#let vert-lead = 17pt           // 竖排分组标签的行距（每行拉开一点）
#let table-w = 162.5mm          // 主表与机要表的宽度（模板原样，比版心宽）
#let table-offset = -2.6mm      // 表格比版心左移（模板的 tblInd）

#let fam(main) = if type(main) == array { (..main, ..F.fallback) } else { (main, ..F.fallback) }
#let hei = fam(F.heading1)
#let kai = fam(F.heading2)
#let fs = fam(F.body)
#let xbs = fam(F.title)
#let red = rgb("#ff0000")

#set page(paper: "a4", margin: (
  top: doc.top_margin_mm * 1mm,
  bottom: doc.bottom_margin_mm * 1mm,
  left: left-m,
  right: right-m,
))
// 行盒高固定为一个字身（0.85em 上、0.15em 下），行高与模板一致；中西文之间不留缝。
#set text(font: fs, size: doc.label_size_pt * 1pt, lang: "zh", region: "cn", tracking: 0pt,
  top-edge: 0.85em, bottom-edge: -0.15em, cjk-latin-spacing: none)
#set par(justify: false, spacing: 0pt, first-line-indent: 0pt, leading: doc.lead_pt * 1pt)

// ---------------- 通用部件 ----------------
// 留白：一条与 n 个全角字等长的横线，供手写。
#let blank(n) = box(width: n * 1em, height: 0.62em, baseline: bottom,
  stroke: (bottom: 0.4pt), outset: (bottom: 1.5pt))

// 勾选框：字体里没有 ☑（GB2312 不含），空框用线画，勾选的框填黑、里面画一个白勾。
// 框要与文字排在同一行，所以用行内的 box。模板里的框是全角字（占一个字宽），
// 这里也让框占一个字宽，后面的文字才接得上。
#let cb-size = 0.6em
#let cb-checked-size = 0.85em
#let cb() = box(width: 1em, height: 1em, inset: 0pt, baseline: bottom,
  align(center + horizon, box(width: cb-size, height: cb-size, inset: 0pt, stroke: 0.5pt)))
#let cb-checked() = box(width: 1em, height: 1em, inset: 0pt, baseline: bottom,
  align(center + horizon, box(width: cb-checked-size, height: cb-checked-size, inset: 0pt,
    fill: black, radius: 0.08em,
    align(center + horizon, text(font: kai, size: 0.75em, fill: white, "√")))))

// 文字样式：黑体（标签与取值）、楷体（勾选字形）、仿宋（备注）。字号由数据给的是磅，
// 这里换成长度。
#let style-font(style) = if style == "kai" { kai } else if style == "fs" { fs } else { hei }
#let style-size(style, base) = base * 1pt

// 一段文字里的「□」「☑」换成画的框，「＿」换成一条填空的横线，
// 「\n」换成换行（备注可以粘多行），其余原样排出。
#let run-content(r, size) = {
  let parts = ()
  let lines = r.text.split("\n")
  for (index, line) in lines.enumerate() {
    if index > 0 {
      parts.push(linebreak())
    }
    let buf = ""
    for ch in line {
      if ch == "□" or ch == "☑" or ch == "＿" {
        if buf != "" {
          parts.push(text(font: style-font(r.style), size: style-size(r.style, size), buf))
          buf = ""
        }
        if ch == "□" {
          parts.push(cb())
        } else if ch == "☑" {
          parts.push(cb-checked())
        } else {
          parts.push(blank(1))
        }
      } else {
        buf = buf + ch
      }
    }
    if buf != "" {
      parts.push(text(font: style-font(r.style), size: style-size(r.style, size), buf))
    }
  }
  parts.join()
}

#let runs-content(runs, size) = runs.map(r => run-content(r, size)).join()

// ---------------- 主表 ----------------
#let widths = doc.column_widths_mm.map(v => v * 1mm)

#let cell-content(c) = {
  // 本格自己的字号（表格里的「标题」一栏比别处小一号），没给就跟全局的走。
  let size = if c.size_pt > 0 { c.size_pt } else { doc.label_size_pt }
  // 本格自己的行距（备注格用模板的固定行距）。
  // 注意：set 规则只在本块内有效，所以要写在函数体里，不能塞进 if 分支。
  set par(leading: if c.layout.lead_pt > 0 { c.layout.lead_pt * 1pt } else { doc.lead_pt * 1pt })
  let body = if c.blank_chars > 0 { blank(c.blank_chars) } else { runs-content(c.runs, size) }
  // 竖排标签每行拉开一点，读起来才像模板里的竖排字。
  if c.vertical {
    set par(leading: vert-lead)
    body
  } else if c.layout.valign == "top" {
    // 顶对齐：文字在上，底下补一段空白，把这一行撑到模板给的高度。
    block(width: 100%, align(left + top, body + v(c.layout.tail_mm * 1mm, weak: false)))
  } else if c.strut_mm > 0 {
    block(width: 100%, height: c.strut_mm * 1mm - 2 * doc.cell_inset_y_mm * 1mm,
      align(center + horizon, body))
  } else {
    body
  }
}

#let cells = doc.cells.map(c => table.cell(
  colspan: c.colspan,
  rowspan: c.rowspan,
  // 首行缩进做在左留白上：表格单元格里排版不认 first-line-indent。
  inset: (left: cell-inset-x + c.layout.indent_em * doc.label_size_pt * 1pt,
    right: cell-inset-x, y: doc.cell_inset_y_mm * 1mm),
  align: if c.align == "left" { left + horizon } else { center + horizon },
  stroke: 0.5pt,
  cell-content(c),
))

// ---------------- 机要表 ----------------
// 两行六列，全部由机要密码机构填写，程序不代填。
#let machine-widths = doc.machine_column_widths_mm.map(v => v * 1mm)
#let machine-rows = (
  ("发方机要机构", "", "发送人", "", "电话", ""),
  ("收方机要机构", "", "接收人", "", "电话", ""),
)
#let machine-heights = (7.6mm, 8.4mm)
#let machine-cells = machine-rows.enumerate().map(row-pair => {
  let h = machine-heights.at(row-pair.at(0))
  row-pair.at(1).map(t => table.cell(
    inset: (x: cell-inset-x, y: 1.5mm),
    align: left + horizon,
    stroke: 0.5pt,
    block(width: 100%, height: h - 3mm, align(left + horizon, text(font: hei, size: doc.label_size_pt * 1pt, t))),
  ))
}).flatten()

// ---------------- 正文 ----------------
// 题名：传输任务名，红色小标宋居中。字距可以为负，把长题名压进一行。
#v(doc.title_gap_pt * 1pt, weak: false)
#align(center, text(
  font: xbs,
  size: doc.title_size_pt * 1pt,
  tracking: doc.title_track_pt * 1pt,
  fill: red,
  doc.title,
))
#v(doc.title_gap_pt * 1pt, weak: false)

// 传字号：右对齐到表格右边界。传字取自设置（「传」这样的一个字），号码留白手写。
// 注意：Typst 的代码块按行断句，表达式不能跨行续写，整条都要写在一行里。
#block(width: table-w, inset: (left: table-offset), {
  set par(leading: 0pt)
  let size = doc.label_size_pt * 1pt
  let serial = if doc.serial_prefix != "" {
    text(font: hei, size: size, "【" + doc.serial_prefix + "】") + blank(4) + text(font: hei, size: size, "号")
  } else {
    blank(5) + text(font: hei, size: size, "号")
  }
  align(right, serial)
})
#v(1.5pt, weak: false)

#block(width: table-w, inset: (left: table-offset), table(columns: widths, ..cells))

// 机要表：提示行与表格都与主表左边界对齐。
#block(width: table-w, inset: (left: table-offset), {
  set par(leading: 0pt)
  v(2mm, weak: false)
  text(font: fs, size: small-size, "以下内容由机要密码机构填写：")
  v(1mm, weak: false)
  table(columns: machine-widths, ..machine-cells)
})

// 末行：保密单位（红色小标宋）与收 / 发时间（仿宋，日期留白手填）。
// 保密负责单位取自设置；没填就保留模板上的「【保密单位】」占位。
#let security-unit-text = if doc.security_unit != "" {
  "【" + doc.security_unit + "】"
} else {
  "【保密单位】"
}
#block(width: table-w, inset: (left: table-offset), {
  set par(leading: 0pt)
  v(2mm, weak: false)
  text(font: xbs, size: note-size, fill: red, security-unit-text)
  h(1em)
  text(font: fs, size: note-size, "20　年　月　日　时　分收到、　月　日　时　分发出")
})
