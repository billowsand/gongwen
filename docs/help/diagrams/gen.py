#!/usr/bin/env python3
"""帮助手册示意图批量生成器：输出 13 个 .drawio，再由 draw.io CLI 导出 PNG。

风格取自应用的 Claude 奶油主题：陶土强调色、暖灰描边。

版面两条硬约束，都是为了「塞进帮助正文那一栏还看得清」：

1. **内容宽度不超过 `W`（860）**。正文栏宽 780pt 封顶，图按等比缩到栏宽，
   所以屏幕上的实际字号 ≈ 图里字号 × 780/内容宽。原先七个阶段一字排开
   摊到 1240 宽，13 号字落到屏幕上只剩 8pt，眯着眼才认得出。
2. **一行放不下就折行**，不是把行拉长。折行的箭头走行与行之间的空档，
   所以 `gap_y` 要留够，不然回折那一段会压在盒子上。

字体用「Aa顺風顺水顺财神」（系统已装 `sfss.ttf`）。缺 `↔` 字形，文案里
一律改用 `→`。
"""

from __future__ import annotations

import html
import math
from pathlib import Path

# 写到脚本自己旁边，别钉死某台机器上的绝对路径。
OUT = Path(__file__).resolve().parent
OUT.mkdir(parents=True, exist_ok=True)

ACCENT = "#C96442"
INK = "#2A2927"
SOFT = "#F5F1EA"
CARD = "#FFFFFF"
LINE = "#D5CEC3"
BLUE = "#3B82A0"
GREEN = "#3E8E4E"
RED = "#B23B2E"
AMBER = "#C98A2B"
MUTED = "#8A8279"

# 字体家族名取自 sfss.ttf 的 Windows name 表（nameID 1）。简繁混写是字体
# 自己的写法，照抄，改一个字 Chromium 就找不到、静默退回默认字体。
FONT = "Aa顺風顺水顺财神"

S_TITLE = 21
S_BODY = 17
S_NOTE = 15
S_EDGE = 14

X0 = 40  # 左边距
W = 860  # 内容区宽度上限


def esc(text: str) -> str:
    """转义成 `html=1` 标签的 XML 属性值。

    标签要过两道：先被 XML 解析器读一次，再被当 HTML 渲染一次。所以

    * `<` `>` `&` 得转**两遍**，只转一遍的话 `<!-- [摘要] -->` 到了 HTML 那一步
      就是个注释，整行凭空消失；
    * 换行反过来只转一遍，要的就是它最后变成 HTML 的 `<br>`。XML 的属性值
      规范化会把字面换行压成空格——这正是从前每个「两行」的盒子都挤成一行的
      原因（`拟稿 有正文`、`确定性闸门 事实比对 · …`）。
    """
    escaped = html.escape(html.escape(text, quote=True), quote=True)
    return escaped.replace("\n", "&lt;br&gt;")


def box(cid: str, label: str, x: int, y: int, w: int, h: int,
        fill: str = CARD, stroke: str = LINE, font: str = INK,
        bold: bool = False, rounded: int = 8, size: int = S_BODY) -> str:
    weight = "fontStyle=1;" if bold else ""
    style = (
        f"rounded={1 if rounded else 0};whiteSpace=wrap;html=1;"
        f"fillColor={fill};strokeColor={stroke};fontColor={font};"
        f"fontFamily={FONT};{weight}fontSize={size};"
    )
    return (
        f'        <mxCell id="{cid}" value="{esc(label)}" style="{style}" '
        f'vertex="1" parent="1">\n'
        f'          <mxGeometry x="{x}" y="{y}" width="{w}" height="{h}" as="geometry"/>\n'
        f'        </mxCell>\n'
    )


def title(label: str, w: int = 380, y: int = 20, h: int = 50) -> str:
    """居中的标题条。"""
    return box("t", label, X0 + (W - w) // 2, y, w, h, fill=SOFT, bold=True, size=S_TITLE)


def edge(cid: str, src: str, tgt: str, label: str = "",
         stroke: str = MUTED, dashed: bool = False,
         exit_xy: tuple[float, float] | None = None,
         entry_xy: tuple[float, float] | None = None,
         points: list[tuple[int, int]] | None = None) -> str:
    """一条正交连线。

    `exit_xy` / `entry_xy` 钉住出入点（0~1 的相对坐标），`points` 给途经点——
    折行的回折段和跨越整行的长线都得手动指路，交给自动路由会从盒子身上压过去。
    """
    lab = f' value="{esc(label)}"' if label else ""
    labstyle = (
        f"labelBackgroundColor=#FFFFFF;fontSize={S_EDGE};fontFamily={FONT};"
        f"fontColor={INK};"
        if label
        else ""
    )
    ports = ""
    if exit_xy:
        ports += f"exitX={exit_xy[0]};exitY={exit_xy[1]};exitDx=0;exitDy=0;"
    if entry_xy:
        ports += f"entryX={entry_xy[0]};entryY={entry_xy[1]};entryDx=0;entryDy=0;"
    style = (
        f"edgeStyle=orthogonalEdgeStyle;rounded=0;html=1;"
        f"strokeColor={stroke};strokeWidth=1.8;endArrow=blockThin;endFill=1;"
        f"{ports}{labstyle}{'dashed=1;' if dashed else ''}"
    )
    geo = '          <mxGeometry relative="1" as="geometry"/>\n'
    if points:
        pts = "".join(
            f'              <mxPoint x="{px}" y="{py}"/>\n' for px, py in points
        )
        geo = (
            '          <mxGeometry relative="1" as="geometry">\n'
            '            <Array as="points">\n'
            f'{pts}'
            '            </Array>\n'
            '          </mxGeometry>\n'
        )
    return (
        f'        <mxCell id="{cid}"{lab} style="{style}" '
        f'edge="1" parent="1" source="{src}" target="{tgt}">\n'
        f'{geo}'
        f'        </mxCell>\n'
    )


def note(cid: str, label: str, x: int, y: int, w: int, h: int,
         fill: str = SOFT, stroke: str = LINE, font: str = INK) -> str:
    style = (
        f"rounded=1;whiteSpace=wrap;html=1;fillColor={fill};strokeColor={stroke};"
        f"fontColor={font};fontFamily={FONT};fontSize={S_NOTE};align=left;"
        f"spacingLeft=12;spacingRight=10;"
    )
    return (
        f'        <mxCell id="{cid}" value="{esc(label)}" style="{style}" '
        f'vertex="1" parent="1">\n'
        f'          <mxGeometry x="{x}" y="{y}" width="{w}" height="{h}" as="geometry"/>\n'
        f'        </mxCell>\n'
    )


def notes(items: list[tuple[str, str]], y: int, h: int = 50, gap: int = 6,
          w: int = W, x: int = X0, **kw) -> tuple[str, int]:
    """整栏宽度的说明条竖着码。返回 (xml, 下一个 y)。

    早先是三条并排铺到 1080 宽，为了并排把整张图拉宽，字也跟着被缩小。
    竖着码宽度不涨，每条还能写全。
    """
    out = ""
    for cid, text in items:
        out += note(cid, text, x, y, w, h, **kw)
        y += h + gap
    return out, y


def grid(prefix: str, items: list[str], y: int, cols: int, cell_h: int,
         gap_x: int = 20, gap_y: int = 64, **kw) -> tuple[str, list[str], int]:
    """把若干盒子铺成 `cols` 列的网格，排不下自动折行，末行居中。

    返回 (xml, 单元 id 列表, 末行底部 y)。
    """
    n = len(items)
    rows = math.ceil(n / cols)
    cell_w = (W - gap_x * (cols - 1)) // cols
    out, ids = "", []
    for i, label in enumerate(items):
        r, col = divmod(i, cols)
        in_row = min(cols, n - r * cols)
        row_w = in_row * cell_w + (in_row - 1) * gap_x
        left = X0 + (W - row_w) // 2
        cid = f"{prefix}{i + 1}"
        ids.append(cid)
        out += box(cid, label, left + col * (cell_w + gap_x),
                   y + r * (cell_h + gap_y), cell_w, cell_h, **kw)
    return out, ids, y + rows * cell_h + (rows - 1) * gap_y


def chain(prefix: str, ids: list[str], cols: int, stroke: str = MUTED) -> str:
    """给网格里的盒子按阅读顺序串箭头。

    行内是普通的左右箭头；换行那一根从上一行末尾的底边出、下一行开头的顶边入，
    正交路由会让它「下 → 横穿行间空档 → 上」，正好走在两行之间的留白里。
    """
    out = ""
    for i in range(len(ids) - 1):
        wraps = (i + 1) % cols == 0
        out += edge(
            f"{prefix}e{i}", ids[i], ids[i + 1], stroke=stroke,
            exit_xy=(0.5, 1) if wraps else None,
            entry_xy=(0.5, 0) if wraps else None,
        )
    return out


def page(name: str, cells: str) -> str:
    return (
        '<mxfile host="gongwen-help">\n'
        f'  <diagram name="{esc(name)}" id="page1">\n'
        '    <mxGraphModel dx="1200" dy="800" grid="1" gridSize="10" guides="1" '
        'tooltips="1" connect="1" arrows="1" fold="1" page="1" pageScale="1" '
        'pageWidth="1000" pageHeight="800" math="0" shadow="0">\n'
        '      <root>\n'
        '        <mxCell id="0"/>\n'
        '        <mxCell id="1" parent="0"/>\n'
        f'{cells}      </root>\n'
        '    </mxGraphModel>\n'
        '  </diagram>\n'
        '</mxfile>\n'
    )


DIAGRAMS: dict[str, str] = {}

# ── 1. 设计理念：模型只起草正文 ────────────────────────────────────────────
# 两条纵列（模型一侧 / 程序一侧）汇到「用户裁决」，闸门串在模型这一列上。
# 原先闸门挂在右边、两条虚线横穿中间，标签正好压在盒子和线上。
c = title("公文助手的设计理念", w=380)
c += box("m", "大语言模型", 40, 100, 380, 64, fill="#F3E5DC", stroke=ACCENT, bold=True)
c += box("p", "本地程序", 520, 100, 380, 64, fill="#DCE8EE", stroke=BLUE, bold=True)
c += box("m2", "只起草正文文字", 40, 196, 380, 54)
c += box("p2", "要素 · 版式 · 导出", 520, 196, 380, 54)
c += box("g", "确定性闸门\n事实比对 · 要素校验 · 词表复扫", 40, 304, 380, 100,
         fill="#F7E4E0", stroke=RED, bold=True)
c += box("drop", "不过即丢弃", 40, 458, 380, 56, fill=SOFT, stroke=MUTED, bold=True)
c += box("u", "用户裁决", 520, 380, 380, 80, fill="#E4EFE4", stroke=GREEN, bold=True)
c += edge("e1", "m", "m2")
c += edge("e2", "p", "p2")
c += edge("e3", "m2", "g", "落地前必过", RED, dashed=True)
c += edge("e4", "g", "drop", "不过", RED, dashed=True)
c += edge("e5", "g", "u", "通过", ACCENT)
c += edge("e6", "p2", "u", "结构化字段", BLUE)
c += notes(
    [
        ("n1", "红线一　AI 永不直接写入正文，产物只能是等你采纳的修订建议"),
        ("n2", "红线二　AI 永不碰公文要素：单位 / 人员 / 文号 / 密级 / 成文日期"),
        ("n3", "红线三　任何 AI 产物落地前必过确定性闸门，不过就丢弃，没有「大概可以」"),
    ],
    y=548, h=52, fill="#F7E4E0", stroke=RED,
)[0]
DIAGRAMS["diag-concept"] = page("设计理念", c)

# ── 2. 界面五区 ─────────────────────────────────────────────────────────────
c = title("主窗口五区", w=280)
c += box("b1", "标题栏　菜单 · 标题 · 保存 / 提交版本 / 导出 · 窗口控制",
         X0, 92, W, 56, fill="#F3E5DC", stroke=ACCENT, bold=True)
c += box("b2", "标签栏　稿件 · 导航页 · PDF 混排在同一条标签栏上", X0, 158, W, 50)
c += box("b3", "功能区　开始 · 插入 · 研报 · 格式 · 审校 · 视图 · 输出",
         X0, 218, W, 56, fill="#DCE8EE", stroke=BLUE, bold=True)
c += box("b4a", "左栏\n文档要素填报\n版头 / 主体 / 版记", X0, 284, 240, 210,
         fill="#E4EFE4", stroke=GREEN, bold=True)
c += box("b4b", "中央\n审校稿编辑与版式预览", 300, 284, 340, 210, fill=CARD, bold=True)
c += box("b4c", "右抽屉\n版本历史\n审校提示", 660, 284, 240, 210,
         fill="#F5EDDC", stroke=AMBER, bold=True)
c += box("b5", "状态栏　状态文案 · 审校提示计数 · 版本历史 · 输入法",
         X0, 504, W, 50, fill=SOFT)
c += notes(
    [
        ("n1", "要素表单默认收起：打开稿件第一眼是满屏稿子，要填要素再展开"),
        ("n2", "刻度条导航在预览右缘，靠近它会推出大纲板"),
    ],
    y=584,
)[0]
DIAGRAMS["diag-layout"] = page("界面五区", c)

# ── 3. 上手五步 ─────────────────────────────────────────────────────────────
c = title("五分钟出第一份稿子", w=360)
cells, ids, bottom = grid(
    "s",
    [
        "1　接上本地模型\nLM Studio / Ollama",
        "2　测试连接\n选一个中文指令模型",
        "3　建标准词库\n单位 · 错写 · 联系人",
        "4　存模板默认要素\n发文单位 · 联系人 · 呈报领导",
        "5　出稿 → 改稿\n→ 审校 → 导出",
    ],
    y=96, cols=3, cell_h=108, stroke=ACCENT, bold=True,
)
c += cells + chain("s", ids, cols=3, stroke=ACCENT)
c += notes(
    [
        ("n1", "首次运行 → 设置 → 上手指引，有同样的五步，可以对着做"),
        ("n2", "第 3 步先录三五个单位、两三个联系人就够出稿，后面边用边补"),
        ("n3", "第 4 步存一次，下次新建同文种稿子自动带出这些要素"),
    ],
    y=bottom + 44,
)[0]
DIAGRAMS["diag-quickstart"] = page("上手五步", c)

# ── 4. SOP 七阶段 ───────────────────────────────────────────────────────────
c = title("SOP 办理进度：七阶段状态表", w=440)
cells, ids, bottom = grid(
    "g",
    [
        "拟稿\n有正文",
        "要素齐备\n必填不缺",
        "文字校对\n必错清零",
        "表达复核\n小模型逐句",
        "存疑清零\n待核实落实",
        "版式验证\n编译成功过",
        "入库送审\n固化成版本",
    ],
    y=96, cols=4, cell_h=96, stroke=ACCENT, bold=True,
)
c += cells + chain("g", ids, cols=4, stroke=ACCENT)
legend_y = bottom + 42
c += box("lg1", "已通过", X0, legend_y, 180, 44, fill="#E4EFE4", stroke=GREEN, bold=True)
c += box("lg2", "待办", X0 + 200, legend_y, 180, 44, fill="#F5EDDC", stroke=AMBER, bold=True)
c += box("lg3", "不适用", X0 + 400, legend_y, 180, 44, fill=SOFT, stroke=LINE, bold=True)
n, nxt = notes(
    [("n1", "SOP 是状态表，不是向导：不强制顺序、不挡操作，只把「还差什么」摊开")],
    y=legend_y + 76, fill="#DCE8EE", stroke=BLUE,
)
c += n
n, nxt = notes(
    [("n2", "缺要素不挡导出，但签发前必须补齐——导出唯一闸门是「正文为空」")],
    y=nxt, fill="#F7E4E0", stroke=RED,
)
c += n
c += notes([("n3", "「不适用」如电话通知本来就没有文号，文号检查自动跳过")], y=nxt)[0]
DIAGRAMS["diag-sop"] = page("SOP 七阶段", c)

# ── 5. 版头/主体/版记 ───────────────────────────────────────────────────────
# 纸面三段横着铺满整栏，说明条竖着码在下面；原先说明挤在右侧 380 宽的窄柱里。
c = title("公文纸面三段与要素表单对应", w=440)
c += box("ph", "版头", X0, 92, 150, 90, fill="#F3E5DC", stroke=ACCENT, bold=True)
c += box("ph2", "密级 · 保密期限 · 红头发文单位 · 机关代字〔年〕序号号\n"
                "特殊处理 · 函稿版本 · 逐份编号", 206, 92, 694, 90)
c += box("ln", "反　线", X0, 192, W, 30, fill=ACCENT, stroke=ACCENT, rounded=0,
         font="#FFFFFF", bold=True, size=S_NOTE)
c += box("pb", "主体", X0, 232, 150, 86, fill="#DCE8EE", stroke=BLUE, bold=True)
c += box("pb2", "主送单位 · 标题（带标题提示） · 正文体例\n标题与列表编号 · 紧缩样式",
         206, 232, 694, 86)
c += box("pr", "版记", X0, 328, 150, 96, fill="#E4EFE4", stroke=GREEN, bold=True)
c += box("pr2", "抄送 · 承办单位 · 联合承办（单位 → 人 → 电话）\n"
                "联系人 / 电话 · 呈报领导 · 落款 · 成文日期 · 双面页码 · 份数",
         206, 328, 694, 96)
c += box("nav", "版面缩略导航图\n点色块跳到对应段\n缺必填项在图上标红点",
         X0, 452, 300, 104, fill="#F5EDDC", stroke=AMBER, bold=True)
c += note("n0", "密级规则：秘密 ≤ 10 年 · 机密 ≤ 20 年 · 绝密 ≤ 30 年（绝密可长期）",
          360, 452, 540, 104, fill="#F7E4E0", stroke=RED)
c += notes(
    [
        ("n1", "表单按纸面部位分三段，顺序与打印出来的纸从上到下一致；按文种保存默认要素，下次新建同文种自动带出"),
        ("n2", "联合发文：主办单位 + 联合单位 + 各自承办联系人成对；行文方式分内部 / 外部、单独发文 / 联合发文"),
    ],
    y=584, h=54,
)[0]
DIAGRAMS["diag-profile"] = page("版头主体版记", c)

# ── 6. 功能区七分区 ─────────────────────────────────────────────────────────
c = title("功能区七分区", w=280)
c += box("sw", "最左　要素填报区开关", X0, 92, 420, 56,
         fill="#E4EFE4", stroke=GREEN, bold=True)
c += box("vw", "最右　五个视图图标", X0 + 440, 92, 420, 56,
         fill="#DCE8EE", stroke=BLUE, bold=True)
cells, _, bottom = grid(
    "r",
    [
        "开始\n保存 · 提交版本\n查找 · AI · 校验",
        "插入\n表格 · 图片\n公文构件 · 词库",
        "研报 ★\n区段 · 锚引\n文献 · 脚注",
        "格式\n标题层级 · 加粗\n规范化",
        "审校\n重新校验\n版本对照 · 字数",
        "视图\n显示方式\n缩放 · 面板开关",
        "输出\n导出 · 打开目录\n成品三入口",
    ],
    y=180, cols=4, cell_h=118, gap_y=24, stroke=ACCENT, bold=True,
)
c += cells
n, nxt = notes([("n1", "★ 研报分区只在「研究报告」文种下显示")], y=bottom + 40)
c += n
n, nxt = notes(
    [("n2", "PDF 不是独立勾选项：只要导出了 .tex 就自动编译 PDF")],
    y=nxt, fill="#F7E4E0", stroke=RED,
)
c += n
c += notes(
    [
        ("n3", "功能区第二行可整体收起（双击当前分区卡），正文多出一条工具栏的高度"),
        ("n4", "公文构件插入的是字面文本加注释标记，导出时按规范处理"),
    ],
    y=nxt,
)[0]
DIAGRAMS["diag-ribbon"] = page("功能区七分区", c)

# ── 7. 研究报告结构 ─────────────────────────────────────────────────────────
c = title("研究报告结构与标记", w=340)
c += box("cover", "封面元数据\n密级 · 文件号 · 版本\n编制单位 · 日期 · 文献名",
         X0, 92, 270, 104, fill="#F3E5DC", stroke=ACCENT, bold=True)
c += box("abs", "摘要\n<!-- [摘要] -->", X0, 216, 270, 70, stroke=BLUE)
c += box("body", "正文（分章）\n<!-- [正文] -->", X0, 306, 270, 74, stroke=BLUE, bold=True)
c += box("app", "附录\n<!-- [附录] -->", X0, 400, 270, 70, stroke=BLUE)
c += box("chg", "版本变更记录\n<!-- [版本变更记录] -->", 335, 216, 270, 70, stroke=GREEN)
c += box("bib", "参考文献\n<!-- [参考文献] -->\n+ references.bib", 335, 306, 270, 74,
         stroke=GREEN)
c += note("mk", "行内标记\n{@id}　锚点\n[@key]　文献引用\n[^id]　行内脚注\n「表：」　表题",
          630, 92, 270, 160, fill="#DCE8EE", stroke=BLUE)
# 标签里写 `$$ … $$` 会被 draw.io 当成公式吃掉，只剩下省略号；改用文字描述。
c += note("fm", "数学公式\n行内：单个 $ 包起来\n独立：两个 $ 单独成行\n\n"
                "公文文种不支持公式，\n$ 始终是普通字符",
          630, 272, 270, 160, fill="#F5EDDC", stroke=AMBER)
n, nxt = notes([("n1", "研究报告只出 TeX / PDF，不支持导出 Word")], y=496,
               fill="#F7E4E0", stroke=RED)
c += n
c += notes(
    [("n2", "预览字形是 STIX Two Math，导出用 TeX CM 字体——字形不同属正常，版式以导出为准")],
    y=nxt,
)[0]
DIAGRAMS["diag-research"] = page("研究报告结构", c)

# ── 8. 导出链路 ─────────────────────────────────────────────────────────────
c = title("导出链路", w=240)
c += box("src", "Markdown 审校稿", X0, 168, 240, 90, fill="#F3E5DC", stroke=ACCENT, bold=True)
c += box("md", "xxx.md", 320, 92, 220, 64)
c += box("docx", "xxx.docx", 320, 176, 220, 64)
c += box("tex", "xxx.tex", 320, 260, 220, 64)
c += box("zip", "xxx-源码包.zip\n.tex + 字体 + 图片附件", 600, 92, 300, 80)
c += box("pdf", "xxx.pdf\nTectonic 离线编译", 600, 192, 300, 80,
         fill="#DCE8EE", stroke=BLUE, bold=True)
c += box("orphan", "孤行探针\n实测坐标 → 可点击提示", 600, 292, 300, 80,
         fill="#F5EDDC", stroke=AMBER)
c += edge("e1", "src", "md")
c += edge("e2", "src", "docx")
c += edge("e3", "src", "tex")
c += edge("e4", "tex", "pdf", "自动", BLUE)
c += edge("e5", "tex", "zip")
c += edge("e6", "tex", "orphan", dashed=True)
n, nxt = notes(
    [("gate", "唯一导出闸门是「正文为空」。缺要素 / 必错未清 / 待核实未落实 → 只进审校面板，不挡导出")],
    y=404, h=54, fill="#F7E4E0", stroke=RED,
)
c += n
c += notes(
    [
        ("n1", "每次导出建同名子目录：输出目录 / <主干名> /　主干名 = 文种前缀 + 名称 + 分钟级时间戳（会议议程无时间戳）"),
        ("n2", "覆盖策略：覆盖同名，或生成 -2、-3 副本（默认生成副本）"),
        ("n3", "编译失败不阻断导出：.md / .docx / .tex 照常生成，PDF 缺失的报错进审校抽屉"),
    ],
    y=nxt, h=52,
)[0]
DIAGRAMS["diag-export"] = page("导出链路", c)

# ── 9. AI 三红线闸门数据流 ──────────────────────────────────────────────────
# 三道闸门串成一行（本来就是串行必过），折行走 S 形，比原先三个挂在右侧、
# 六条线挤向中间清楚得多。
c = title("AI 产物落地前必过三道闸门", w=420)
c += box("in", "正文 + 要素\n要素模型碰不到", X0, 96, 250, 88,
         fill="#F3E5DC", stroke=ACCENT, bold=True)
c += box("model", "模型起草\n只写正文文字", 345, 96, 250, 88, stroke=ACCENT, bold=True)
c += box("prop", "修订建议\n待采纳", 650, 96, 250, 88, fill="#F5EDDC", stroke=AMBER, bold=True)
c += box("g1", "闸门一\n事实闸门 · 事实比对", 650, 238, 250, 96,
         fill="#F7E4E0", stroke=RED, bold=True)
c += box("g2", "闸门二\n要素校验 · 校验规则", 345, 238, 250, 96,
         fill="#F7E4E0", stroke=RED, bold=True)
c += box("g3", "闸门三\n词表与规则复扫", X0, 238, 250, 96,
         fill="#F7E4E0", stroke=RED, bold=True)
c += box("ok", "用户逐项确认\n事实变化清单", X0, 408, 250, 88,
         fill="#E4EFE4", stroke=GREEN, bold=True)
c += box("self", "采纳即自检\n改动段重跑校验\n引入新问题自动回滚", 345, 408, 250, 88,
         fill="#DCE8EE", stroke=BLUE, bold=True)
c += box("drop", "任一道不过\n整份丢弃", 650, 408, 250, 88, fill=SOFT, stroke=MUTED, bold=True)
c += edge("e1", "in", "model")
c += edge("e2", "model", "prop")
c += edge("e3", "prop", "g1", stroke=RED)
c += edge("e4", "g1", "g2", stroke=RED)
c += edge("e5", "g2", "g3", stroke=RED)
c += edge("e6", "g3", "ok", "三道全过", GREEN, exit_xy=(0.25, 1), entry_xy=(0.5, 0))
c += edge("e7", "ok", "self", stroke=BLUE)
# 「不过」这一根要横穿回右侧。走闸门行与末行之间那道空档（y=372），从 g3 底边
# 偏右出发——偏左那个出口让给上面的「三道全过」，两根才不会叠在一起。
c += edge("e8", "g3", "drop", "不过", RED, dashed=True,
          exit_xy=(0.75, 1), entry_xy=(0.5, 0), points=[(227, 372), (775, 372)])
c += notes(
    [("n1", "确定性规则是 AI 的裁判，不是竞争者——不会为了迁就模型输出而放宽校验")],
    y=530, fill="#F7E4E0", stroke=RED,
)[0]
DIAGRAMS["diag-ai-flow"] = page("AI 闸门", c)

# ── 10. 事实单确认流程 ──────────────────────────────────────────────────────
c = title("事实单三步：确定性拆分 → 人确认 → 才起草", w=560)
c += box("mat", "材料原文\n会议纪要 / 来函 / 要点", X0, 96, 250, 96,
         fill="#F3E5DC", stroke=ACCENT, bold=True)
c += box("ext", "提取事实单\n程序确定性拆分\n★ 全程不调用模型", 345, 96, 250, 96,
         fill="#F7E4E0", stroke=RED, bold=True)
c += box("fact", "事实单（可编辑）\n事项 · 时间 · 地点\n单位 · 人员 · 数字 · 依据", 650, 96, 250, 96,
         stroke=AMBER, bold=True)
c += box("human", "人逐项确认\n改文字 · 删条目 · 加条目\n唯一能定事实的地方", 650, 250, 250, 96,
         fill="#E4EFE4", stroke=GREEN, bold=True)
c += box("gen", "按确认后的事实起草\n此刻才调模型", 345, 250, 250, 96, stroke=ACCENT, bold=True)
c += box("out", "修订建议提案\n左当前稿 / 右提案\n事实变化逐项确认", X0, 250, 250, 96,
         fill="#DCE8EE", stroke=BLUE, bold=True)
c += edge("e1", "mat", "ext")
c += edge("e2", "ext", "fact")
c += edge("e3", "fact", "human")
c += edge("e4", "human", "gen", "确认后", GREEN)
c += edge("e5", "gen", "out")
n, nxt = notes(
    [("n1", "模型从头到尾没机会自己定事实——这就是红线二在起草这一侧的落地方式")],
    y=384, fill="#F7E4E0", stroke=RED,
)
c += n
c += notes([("n2", "材料成文 / 知识起草 / 大纲起草三条工作流共用这套流程")], y=nxt)[0]
DIAGRAMS["diag-ai-workbench"] = page("事实单流程", c)

# ── 11. 稿件状态机 ──────────────────────────────────────────────────────────
c = title("稿件生命周期状态机", w=340)
c += box("draft", "草稿\n可编辑", X0, 120, 250, 110,
         fill="#DCE8EE", stroke=BLUE, bold=True, size=S_TITLE - 2)
c += box("pub", "发布\n送审 / 印发后", 345, 120, 250, 110,
         fill="#E4EFE4", stroke=GREEN, bold=True, size=S_TITLE - 2)
c += box("arc", "归档\n唯一终态 · 冻结", 650, 120, 250, 110,
         fill="#F5EDDC", stroke=AMBER, bold=True, size=S_TITLE - 2)
# 两个方向的箭头共用格子之间那道 60 宽的缝，标签必须短到塞得进去，
# 不然文字会把自己的箭头整根盖住。
c += edge("e1", "draft", "pub", "发布", GREEN, exit_xy=(1, 0.32), entry_xy=(0, 0.32))
c += edge("e2", "pub", "draft", "退回", BLUE, exit_xy=(0, 0.72), entry_xy=(1, 0.72))
c += edge("e3", "pub", "arc", "归档", AMBER)
# 草稿直接归档要绕过中间的「发布」，从底下走。
c += edge("e4", "draft", "arc", "直接归档", AMBER,
          exit_xy=(0.5, 1), entry_xy=(0.5, 1), points=[(160, 272), (780, 272)])
c += notes(
    [("n1", "归档没有出边：正文与要素全部冻结，这是唯一的终态")],
    y=306, fill="#F7E4E0", stroke=RED,
)[0]
c += notes(
    [
        ("n2", "归档后唯一还能改的是盖章扫描 PDF 附件的增删"),
        ("n3", "想改内容 → 用「基于此公文新建」另起一篇，原件保持冻结"),
        ("n4", "状态色：草稿蓝 · 发布绿 · 归档橙，稿件表与标签上通用"),
    ],
    y=362,
)[0]
DIAGRAMS["diag-manuscript"] = page("稿件状态机", c)

# ── 12. 数据目录结构 ────────────────────────────────────────────────────────
c = title("数据存放：用户配置目录", w=360)
c += box("root", "LocalTools / GongwenAssistant / config /", X0 + 180, 92, 500, 64,
         fill="#F3E5DC", stroke=ACCENT, bold=True)
c += box("d1", "config.json\n词库 · 提示词 · 设置 · 模板默认要素", X0, 196, 420, 90, stroke=BLUE)
c += box("d2", "manuscripts.db\n稿件 + 知识库 + 公文词表（同一个 SQLite）",
         X0 + 440, 196, 420, 90, stroke=BLUE, bold=True)
c += box("d3", "images/\n插入的图片与 PDF 附件（文档只存相对路径）", X0, 306, 420, 90, stroke=BLUE)
c += box("d4", "ime/\n词频 · 用户词 · dicts/　辅码表在 fuma/", X0 + 440, 306, 420, 90, stroke=BLUE)
c += box("d5", ".zip-password\n稿件 ZIP 密码，权限 0600", X0, 416, 420, 76, fill=SOFT)
c += note("n4", "卸载重装不丢数据：数据在用户配置目录，不在安装目录",
          X0 + 440, 416, 420, 76, fill="#F7E4E0", stroke=RED)
c += note("n1", "Windows　%APPDATA%\\LocalTools\\GongwenAssistant\\config\\\n"
                "Linux　　~/.config/LocalTools/GongwenAssistant/config/\n"
                "macOS　　~/Library/Application Support/LocalTools/GongwenAssistant/config/",
          X0, 516, W, 96, fill="#DCE8EE", stroke=BLUE)
n, nxt = notes(
    [("n2", "最小备份三样：config.json + manuscripts.db + images/")],
    y=624, fill="#E4EFE4", stroke=GREEN,
)
c += n
c += notes(
    [("n3", "换机器：导出稿件 ZIP（随附 vocabulary.json）→ 目标机导入并勾选「合并词库」")],
    y=nxt,
)[0]
DIAGRAMS["diag-data"] = page("数据目录", c)

# ── 13. 五种视图 ────────────────────────────────────────────────────────────
c = title("起草页五种视图", w=300)
cells, _, bottom = grid(
    "v",
    [
        "源码\n带语法高亮的 Markdown\n（默认）",
        "实时排版\n非活动行按公文版式排\n只在光标行显示标记",
        "版式预览\n公文字体与行距\n看纸面效果",
        "分栏\n左源码 / 右版式\n对照着改",
        "版本对照\n左最新提交 / 右当前\n看改了什么",
    ],
    y=96, cols=3, cell_h=128, gap_y=28, stroke=ACCENT, bold=True,
)
c += cells
n, nxt = notes(
    [("n1", "不熟 Markdown → 先用「实时排版」：只在光标那一行看得到标记")],
    y=bottom + 40, fill="#DCE8EE", stroke=BLUE,
)
c += n
c += notes(
    [
        ("n2", "对版、看成品 → 用「版式预览」；预览右缘有刻度条，靠近推出大纲板"),
        ("n3", "源码字面三套模板：编辑器字体 / 标题随公文 / 与公文一致（设置 → 字体）"),
        ("n4", "字号：Ctrl + 加减号 / 0 复位，或 Ctrl + 滚轮——只改屏幕字号，不影响导出"),
    ],
    y=nxt,
)[0]
DIAGRAMS["diag-editor-views"] = page("五种视图", c)


def main() -> None:
    for name, xml in DIAGRAMS.items():
        path = OUT / f"{name}.drawio"
        path.write_text(xml, encoding="utf-8")
        print(f"wrote {path}")


if __name__ == "__main__":
    main()
