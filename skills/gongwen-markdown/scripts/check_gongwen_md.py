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
    r"^(?:第[一二三四五六七八九十百零\d]+[章节条部分]|[（(][一二三四五六七八九十百零]+[）)]"
    r"|[一二三四五六七八九十百零]+[、,.．]|[（(]\d+[）)]|\d+(?:\.\d+)+|\d+[.．、]\s|\d+\s)"
)
MARKER = re.compile(r"^<!--\s*[\[【]\s*([^\]】]+?)\s*[\]】]\s*-->$")
OFFICIAL_MARKERS = {
    "正文", "body", "附件", "附录", "attachment", "attachments", "appendix",
    "居中", "center", "居右", "右对齐", "right",
    "序号表", "序号表格", "numbered-table", "numbered_table", "numberedtable",
}
RESEARCH_MARKERS = OFFICIAL_MARKERS | {
    "摘要", "abstract", "目录", "toc", "版本变更记录", "changelog", "version",
    "参考文献", "references", "bibliography",
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
    match = MARKER.match(line.strip())
    return match.group(1).strip().lower() if match else None


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
    dash_count = 0
    strong_words = 0
    anchors: set[str] = set()
    refs: list[tuple[int, str]] = []

    for number, (raw, line) in enumerate(zip(lines, stripped), start=1):
        if line.startswith("```"):
            in_fence = not in_fence
            if not research:
                report.error(number, "公文不使用代码块")
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
                if not in_attachment:
                    titles_in_body += 1
                    if titles_in_body > 1:
                        report.error(number, "正文区出现第二个 `#` 标题：`#` 只给文档标题和附件标题用")
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
        if line.startswith(">"):
            report.error(number, "不支持引用块 `>`，会原样印出")
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
        if RELATIVE_DATE.search(line):
            report.warn(number, "有相对日期（今天、下周等），应换算成“YYYY年M月D日（星期X）”")
        if re.search(r"(?:上午|下午|晚上)\s*\d{1,2}[:：]\d{2}", line):
            report.warn(number, "24 小时制时间前不加“上午/下午”")

        dash_count += line.count("——")
        strong_words += len(re.findall(r"必须|应当|不得|严禁", line))

        if line.startswith("|"):
            continue
        if research:
            for match in re.finditer(r"\{#([A-Za-z][\w:.-]*)\}", line):
                anchors.add(match.group(1))
            for match in re.finditer(r"\{@([A-Za-z][\w:.-]*)\}", line):
                refs.append((number, match.group(1)))
            if re.search(r"\[\^[^\]]+\](?![:：][(（])", line):
                report.warn(number, "脚注要写成 `[^id]:(内容)`，只写 `[^id]` 会原样印出")

    # 研究报告里标题、表题行上的锚点也要收进来（上面跳过了标题行）。
    if research:
        for line in stripped:
            for match in re.finditer(r"\{#([A-Za-z][\w:.-]*)\}", line):
                anchors.add(match.group(1))
        for number, ref in refs:
            if ref not in anchors:
                report.error(number, f"交叉引用 {{@{ref}}} 找不到对应的锚点 {{#{ref}}}")

    check_tables(stripped, report)

    if attachment_count and has_attachment_summary and not research:
        report.error(has_attachment_summary, "已有附件区段，程序会自动生成“附件：1.……”说明，不要手写")
    if dash_count > 1 and not research:
        report.warn(0, f"破折号用了 {dash_count} 处，全篇最多一处")
    if strong_words > 2 and not research:
        report.warn(0, f"“必须/应当/不得/严禁”合计 {strong_words} 处，建议不超过两处")

    if kind in {"white-paper", "red-head-approval"} and "妥否，请指示" not in text:
        report.error(0, "呈批件结尾必须有“妥否，请指示。”")
    if kind == "white-paper" and attachment_count:
        report.error(0, "白头件只写标题和正文，不带附件区段")
    if kind == "meeting-agenda":
        check_agenda(nonblank, report)

    tail = [line for _, line in nonblank[-3:]]
    if not research and any(DATE_ONLY.match(line) for line in tail):
        report.error(nonblank[-1][0], "文末不要手写成文日期，由程序按要素生成")
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
        help="文种：公函 / 电话通知 / 普通公文 / 会议议程 / 白头件 / 红头呈批件 / 研究报告（也认英文名）",
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
