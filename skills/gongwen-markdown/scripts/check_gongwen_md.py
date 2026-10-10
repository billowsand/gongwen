#!/usr/bin/env python3
"""公文助手 Markdown 自检脚本。

只用标准库。检查的是公文助手（gongwen-assistant）解析器会出问题、或校对规则会拦下
的写法，不评判文采。用法：

    python check_gongwen_md.py 稿件.md --kind 公函
    python check_gongwen_md.py 稿件.md --kind research
    cat 稿件.md | python check_gongwen_md.py - --kind 白头件

退出码：有 ERROR 为 1，否则为 0。WARN 需要人工判断。
"""

from __future__ import annotations

import argparse
import re
import sys

KINDS = {
    "official-letter": "公函",
    "phone-notice": "电话通知",
    "phone-record": "电话记录单",
    "plain-document": "普通公文",
    "meeting-agenda": "会议议程",
    "white-paper": "白头件",
    "red-head-approval": "红头呈批件",
    "research-report": "研究报告",
}
ALIASES = {
    "公函": "official-letter",
    "函": "official-letter",
    "letter": "official-letter",
    "电话通知": "phone-notice",
    "电话记录单": "phone-record",
    "notice": "phone-notice",
    "普通公文": "plain-document",
    "plain": "plain-document",
    "会议议程": "meeting-agenda",
    "agenda": "meeting-agenda",
    "白头件": "white-paper",
    "呈批件": "white-paper",
    "white": "white-paper",
    "红头呈批件": "red-head-approval",
    "red": "red-head-approval",
    "研究报告": "research-report",
    "research": "research-report",
}

# 与程序 export::headings::clean_heading_number 同一组手写编号前缀。
MANUAL_NUMBER = re.compile(
    r"^(?:附录\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?\s"
    r"|(?i:appendix)\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?\s"
    r"|第[一二三四五六七八九十百零\d]+[章节条部分]|[（(][一二三四五六七八九十百零]+[）)]"
    r"|[一二三四五六七八九十百零]+[、,.．]|[（(]\d+[）)]|\d+(?:\.\d+)+|\d+[.．、]\s|\d+\s)"
)
COMMENT = re.compile(r"^<!--\s*(.+?)\s*-->$")
BRACKETED = re.compile(r"^[\[【]\s*([^\]】]+?)\s*[\]】]$")
OFFICIAL_MARKERS = {
    "正文", "body", "附件", "附录", "attachment", "attachments", "appendix",
    "居中", "center", "居右", "右对齐", "right",
    "序号表", "序号表格", "numbered-table", "numbered_table", "numberedtable",
}
RESEARCH_MARKERS = OFFICIAL_MARKERS | {
    "摘要", "abstract", "目录", "toc", "版本变更记录", "changelog", "version",
    "参考文献", "references", "bibliography",
    "部分", "part", "不编号", "unnumbered", "nonumber", "no-number", "no_number",
}
ELEMENT_LINE = re.compile(
    r"^(?:主送(?:单位)?[：:]|抄送(?:单位)?[：:]|承办(?:单位)?[：:]|联系人[：:]|联系电话[：:]"
    r"|发文字号[：:]|密级[：:]|保密期限[：:]|印发|签发人[：:])"
)
DATE_ONLY = re.compile(r"^[0-9〇一二三四五六七八九十]{2,4}年[0-9〇一二三四五六七八九十]{1,2}月[0-9〇一二三四五六七八九十]{1,3}日$")
LIST_ITEM = re.compile(r"^(?:\d{1,9}\.\s|[-*]\s)")
RELATIVE_DATE = re.compile(r"今天|明天|后天|大后天|昨天|本周[一二三四五六日天]|下周[一二三四五六日天]?")


class Report:
    def __init__(self) -> None:
        self.items: list[tuple[str, int, str]] = []

    def error(self, line: int, message: str) -> None:
        self.items.append(("ERROR", line, message))

    def warn(self, line: int, message: str) -> None:
        self.items.append(("WARN", line, message))

    def print(self) -> int:
        for level, line, message in sorted(self.items, key=lambda item: item[1]):
            where = f"第{line}行" if line else "全文"
            print(f"{level}\t{where}\t{message}")
        errors = sum(1 for level, _, _ in self.items if level == "ERROR")
        warns = len(self.items) - errors
        print(f"\n共 {errors} 个错误、{warns} 个提醒。", file=sys.stderr)
        return 1 if errors else 0


def marker_name(line: str) -> str | None:
    match = COMMENT.match(line.strip())
    if not match:
        return None
    name = match.group(1).strip()
    bracketed = BRACKETED.match(name)
    if bracketed:
        name = bracketed.group(1)
    return name.strip().lower() or None


def table_cells(line: str) -> int:
    value = line.strip()
    if value.startswith("|"):
        value = value[1:]
    if value.endswith("|"):
        value = value[:-1]
    return len(value.split("|"))


def check(text: str, kind: str) -> Report:
    report = Report()
    research = kind == "research-report"
    lines = text.replace("\r\n", "\n").split("\n")
    stripped = [line.strip() for line in lines]
    nonblank = [(index + 1, line) for index, line in enumerate(stripped) if line]
    if not nonblank:
        report.error(0, "内容为空")
        return report

    first_no, first = nonblank[0]
    if first.startswith("```"):
        report.error(first_no, "文件以代码围栏开头：交付的 .md 内容里不能带 ``` 包裹")
    if first == "---":
        report.error(first_no, "疑似 YAML frontmatter：要素在程序表单里填，不写元数据")
    if not first.startswith("# "):
        report.error(first_no, "第一行应是唯一的 `# 正式标题`")

    allowed = RESEARCH_MARKERS if research else OFFICIAL_MARKERS
    in_attachment = False
    expecting_title = True
    titles_in_body = 0
    attachment_count = 0
    last_level = 1
    has_attachment_summary = None
    in_fence = False
    in_part = False
    anchors: set[str] = set()
    refs: list[tuple[int, str]] = []

    for number, (raw, line) in enumerate(zip(lines, stripped), start=1):
        if line.startswith("```"):
            if not in_fence and not re.fullmatch(r"```\s*mermaid\s*", line, re.I):
                report.error(number, "只用 ```mermaid 围栏画图表，其他代码块不要写")
            in_fence = not in_fence
            continue
        if in_fence or not line:
            continue

        name = marker_name(line)
        if name is not None:
            if name not in allowed:
                report.warn(number, f"不认识的标记 <!-- [{name}] -->，会被忽略")
            if name in {"附件", "attachment", "attachments", "附录", "appendix"}:
                in_attachment = True
                expecting_title = True
                attachment_count += 1
                last_level = 1
            if name in {"部分", "part"}:
                in_part = True
            if name in {
                "附件", "附录", "attachment", "attachments", "appendix",
                "参考文献", "references", "bibliography",
                "版本变更记录", "changelog", "version",
            }:
                in_part = False
            continue
        if line.startswith("<!--"):
            continue

        heading = re.match(r"^(#{1,})\s+(.*)$", line)
        if heading:
            level = len(heading.group(1))
            content = heading.group(2)
            if research:
                content = re.sub(r"\s*\{#[A-Za-z][\w:.-]*\}\s*$", "", content)
            if level == 1:
                if not in_attachment and not (research and in_part):
                    titles_in_body += 1
                    if titles_in_body > 1:
                        report.error(number, "正文区出现第二个 `#` 标题：`#` 只给文档标题和附件标题用（研究报告“部分”区除外）")
                elif not expecting_title and not research:
                    report.error(number, "一份附件里只能有一个 `#` 附件标题，下一份附件前要再写 <!-- [附件] -->")
                expecting_title = False
                if re.match(r"^附件\s*\d*\s*[：:、]?", content) and not research:
                    report.warn(number, "附件标题不要写“附件1”，程序自动编号；`#` 后直接写附件正式标题")
            else:
                if level > 5:
                    report.error(number, f"{level} 级标题不编号，最多用到 #####")
                if MANUAL_NUMBER.match(content):
                    report.error(number, f"标题手写了编号「{content[:8]}…」，程序会剥掉重编，请删去")
                if level > last_level + 1 and last_level >= 1 and level > 2:
                    report.warn(number, f"标题跳级：上一个标题是 {last_level} 级 `#`，这里直接用了 {level} 级")
                if content.endswith("。") and not research:
                    report.warn(number, "标题末尾一般不加句号")
            last_level = level
            continue

        if line in {"---", "***", "___"} or re.fullmatch(r"-{3,}|\*{3,}|_{3,}", line):
            report.error(number, "不要写分隔线，版记横线由程序画")
        if re.match(r">(\s|>|$)", line) and not research:
            report.warn(number, "公文没有引文、文框版式，引用块 `>` 按普通段落排；行首要写字面 `>` 时写 `\\>`")
        if re.search(r"(?<!!)\[[^\]]+\]\((?:https?:|www\.)", line):
            report.warn(number, "公文不放超链接，`[文字](网址)` 会原样印出")
        if re.search(r"<(?!!--)[a-zA-Z/][^>]*>", line):
            report.warn(number, "不要写 HTML 标签")
        if "![" in line and not re.fullmatch(r"!\[[^\]]*\]\(\S+\)(?:\s*\{#[A-Za-z][\w:.-]*\})?", line):
            report.error(number, "图片必须独占一行：`![图注](images/x.png)`")
        if raw.startswith((" ", "\t", "　")) and not LIST_ITEM.match(line):
            report.warn(number, "行首有空格或全角空格：程序自动首行缩进，不要手打")

        if not in_attachment and ELEMENT_LINE.match(line):
            report.error(number, "疑似版头 / 版记要素，这些在程序表单里填，不写进正文")
        if re.match(r"^附件[：:]\s*(?:1|一)", line):
            has_attachment_summary = number

        if kind in {"white-paper", "red-head-approval"} and LIST_ITEM.match(line):
            report.error(number, "呈批件不得使用列表，段内枚举写成“一是……；二是……。”")
        if not research and re.search(r"\$[^$\s][^$]*\$", line):
            report.warn(number, "公文文种不支持公式，`$` 按普通字符印出")
        if re.search(r'"[^"]*[一-鿿][^"]*"', line):
            report.warn(number, "中文里用了半角引号，改用全角“”")
        date_line = re.sub(r"[“‘「『][^”’」』]*[”’」』]", "", line)
        if kind not in {"research-report", "phone-record"} and not in_attachment and not line.startswith(">") and RELATIVE_DATE.search(date_line):
            report.warn(number, "有相对日期（今天、下周等），请核对材料基准日期后明确日程或期限，不直接按导出当天换算")
        if re.search(r"(?:上午|下午|晚上)\s*\d{1,2}[:：]\d{2}", line):
            report.warn(number, "24 小时制时间前不加“上午/下午”")


        if line.startswith("|"):
            continue
        if research:
            for match in re.finditer(r"\{#([A-Za-z][\w:.-]*)\}", line):
                anchors.add(match.group(1))
            for match in re.finditer(r"\{@([A-Za-z][\w:.-]*)\}", line):
                refs.append((number, match.group(1)))
            if re.search(r"\[\^[^\]]+\](?![:：][(（])", line):
                report.warn(number, "脚注要写成 `[^id]:(内容)`，只写 `[^id]` 会原样印出")
        elif re.search(r"\[\^[^\]]+\]", line):
            report.warn(number, "公文文种不支持脚注，`[^id]` 会原样印出")

    # 研究报告里标题、表题行上的锚点也要收进来（上面跳过了标题行）。
    if research:
        first_defined: dict[str, int] = {}
        for index, line in enumerate(stripped, start=1):
            match = re.search(r"\{#([A-Za-z][\w:.-]*)\}\s*$", line)
            if match:
                anchor = match.group(1)
                if anchor in first_defined:
                    report.error(index, f"锚点 {{#{anchor}}} 与第 {first_defined[anchor]} 行重复，导出 PDF 会中止；改成不同的 id（可加 -2）")
                else:
                    first_defined[anchor] = index
            for match in re.finditer(r"\{#([A-Za-z][\w:.-]*)\}", line):
                anchors.add(match.group(1))
        for number, ref in refs:
            if ref not in anchors:
                report.error(number, f"交叉引用 {{@{ref}}} 找不到对应的锚点 {{#{ref}}}")

    check_tables(stripped, report)

    if attachment_count and has_attachment_summary and not research:
        report.error(has_attachment_summary, "已有附件区段，程序会按附件标题自动生成附件说明，不要手写")
    # 呈批件是版式，只在主标题明确为请示时核对结语；不设标点或用词配额。
    section = "正文"
    main_title = None
    own_lines = []
    for line in stripped:
        marker = re.match(r"^<!--\s*[\[【]?(正文|附件|附录|body|attachment|appendix)[\]】]?\s*-->$", line, re.I)
        if marker:
            section = "正文" if marker.group(1).lower() in {"正文", "body"} else "附件"
            continue
        if section != "正文" or line.startswith(">"):
            continue
        if main_title is None and line.startswith("# "):
            main_title = line[2:].strip()
        elif not line.startswith("#"):
            own_lines.append(re.sub(r"[“‘「『][^”’」』]*[”’」』]", "", line))
    closing = r"(?:妥否|当否|可否|是否妥当)[，,]?\s*请(?:批示|指示|示下|审批)|请予(?:批复|批准)|特此请示"
    if kind in {"white-paper", "red-head-approval"} and main_title and main_title.endswith("请示"):
        if not any(re.search(rf"(?:^|[。！？])\s*(?:以上意见|以上请示|以上事项)?(?:{closing})[。！.!]?\s*$", line) for line in own_lines):
            report.warn(0, "请示缺少请求性结语，请核对是否需要“妥否，请批示”等结语；报告不加")
    if kind == "white-paper" and attachment_count:
        report.error(0, "白头件只写标题和正文，不带附件区段")
    if kind == "meeting-agenda":
        check_agenda(nonblank, report)

    tail = nonblank[-3:]
    if not research:
        for date_no, date_line in tail:
            if DATE_ONLY.match(date_line):
                report.error(date_no, "文末不要手写成文日期，由程序按要素生成")
    return report


def check_tables(lines: list[str], report: Report) -> None:
    index = 0
    while index < len(lines):
        line = lines[index]
        if (
            line.startswith("|")
            and index + 1 < len(lines)
            and re.fullmatch(r"\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?", lines[index + 1])
        ):
            width = table_cells(lines[index + 1])
            if table_cells(line) != width:
                report.error(index + 1, "表头列数与分隔行不一致")
            index += 2
            while index < len(lines) and lines[index].startswith("|"):
                cells = table_cells(lines[index])
                if cells > width:
                    report.error(index + 1, f"这一行有 {cells} 格，多于表头的 {width} 列")
                index += 1
            continue
        if line.startswith("|") and (index == 0 or not lines[index - 1].startswith("|")):
            nxt = lines[index + 1] if index + 1 < len(lines) else ""
            if not nxt.startswith("|"):
                report.warn(index + 1, "以 | 开头的单行不构成表格，会按正文印出")
            else:
                report.error(index + 1, "表格缺少分隔行 `| --- | --- |`")
        index += 1


def check_agenda(nonblank: list[tuple[int, str]], report: Report) -> None:
    body = [line for _, line in nonblank]
    expected = ["一、时间地点：", "二、参加人员：", "三、研讨内容："]
    positions = []
    for label in expected:
        found = next((i for i, line in enumerate(body) if line.startswith(label)), None)
        if found is None:
            report.error(0, f"会议议程缺少固定行“{label}”")
        positions.append(found)
    if all(p is not None for p in positions) and positions != sorted(positions):
        report.error(0, "“一、时间地点”“二、参加人员”“三、研讨内容”顺序不能变")
    for number, line in nonblank[1:]:
        if line.startswith("#"):
            report.error(number, "会议议程除第一行标题外不用 `#` 标题")
        if line.startswith(("- ", "* ", "|")):
            report.error(number, "会议议程不用项目符号或表格，议程用 1. 2. 3. 编号")
    if positions[2] is not None:
        items = body[positions[2] + 1 :]
        extra = [line for line in items if not re.match(r"^\d+\.\s", line)]
        if extra:
            report.error(0, "“三、研讨内容：”之后只能是 1. 2. 3. 议程项，不加其他段落")
        for i, line in enumerate(items):
            if not re.match(r"^\d+\.\s", line):
                continue
            last = i == len(items) - 1
            line = line.rstrip("】")
            if last and not line.endswith("。"):
                report.warn(0, "最后一项议程以句号结尾")
            if not last and not line.endswith("；"):
                report.warn(0, f"非末项议程以分号结尾：{line[:12]}…")


def main() -> int:
    parser = argparse.ArgumentParser(description="检查 Markdown 是否符合公文助手的公文子集")
    parser.add_argument("file", help="Markdown 文件路径，- 表示从标准输入读取")
    parser.add_argument(
        "--kind",
        default="official-letter",
        help="文种：公函 / 电话通知 / 电话记录单 / 普通公文 / 会议议程 / 白头件 / 红头呈批件 / 研究报告（也认英文名）",
    )
    args = parser.parse_args()
    kind = ALIASES.get(args.kind, args.kind)
    if kind not in KINDS:
        parser.error(f"不认识的文种：{args.kind}")
    if args.file == "-":
        text = sys.stdin.buffer.read().decode("utf-8-sig")
    else:
        with open(args.file, encoding="utf-8-sig") as handle:
            text = handle.read()
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")
    print(f"按「{KINDS[kind]}」检查", file=sys.stderr)
    return check(text, kind).print()


if __name__ == "__main__":
    sys.exit(main())
