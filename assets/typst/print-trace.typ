// 独立印制标识：先排成小幅透明 PDF，再叠加到已完成的正文页面。
#let doc = json("/doc.json")
#set page(width: 24mm, height: 11mm, margin: 0pt, fill: none)
#set text(font: doc.font, size: 4.2pt, fill: black, fallback: false)
#set par(leading: 0pt, spacing: 0pt)
#let fit-line(value) = context {
  let body = text(value)
  let width = measure(body).width
  if width > 24mm {
    scale(x: 24mm / width * 100%, y: 100%, origin: left, reflow: true, body)
  } else { body }
}
#place(top + left, image("/trace.svg", width: 22.1mm))
#place(top + left, dy: 6.8mm, fit-line("印制单位：" + doc.unit))
#place(top + left, dy: 8.7mm, fit-line("印制编号：" + doc.code))
