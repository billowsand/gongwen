#!/usr/bin/env python3
"""公文助手技能包模型评测脚本（可复用）。

用途：让外部大模型照着 `skills/gongwen-markdown/` 技能文档起草公文正文，把原始输出
清洗后交给 `check_gongwen_md.py` 判定，统计「模型产出能否被公文助手采纳」的通过率与
失败模式，供技能文档作者定位该改哪条规则。

**密钥只从本机 gitignore 的密钥文件或环境变量读取，绝不写进仓库文件。**

用法：

    # 1) 冒烟：确认模型名可用（每个模型发一次极小请求）
    python scripts/gongwen-skill-eval.py smoke

    # 2) 基线：qny 3 模型 × 6 文种 × 2 条件 = 36 次
    python scripts/gongwen-skill-eval.py run --matrix baseline

    # 3) 交叉验证：dmxapi / siliconflow 各 2 模型 × 6 文种 × 条件 A
    python scripts/gongwen-skill-eval.py run --matrix cross

    # 4) 全量
    python scripts/gongwen-skill-eval.py run --matrix all --run-id 20261018-full

产物目录：`tmp/gongwen-skill-eval/<run-id>/`

    manifest.json                    固定结构（Rust 验收测试按它读）
    summary.tsv / summary.json       逐次结果与聚合
    report.md                        自动生成的失败模式摘要（供写最终报告）
    prompts/<kind>-<A|B>.md          实际使用的系统提示与任务，便于复现
    <A|B>/<kind>/<provider>__<model>.md       清洗后正文
    <A|B>/<kind>/<provider>__<model>.raw.md   模型原始输出（含代码围栏）

清洗严格按 `src/prompt.rs` 的 `sanitize_model_markdown`：只剥思考块、最外层代码围栏、
首尾寒暄，**不修正任何格式**，否则测不出模型的错。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import re
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SKILL_DIR = ROOT / "skills" / "gongwen-markdown"
CHECK_SCRIPT = SKILL_DIR / "scripts" / "check_gongwen_md.py"
DEFAULT_KEYS_FILE = ROOT / "model_api_test.md.txt"
OUT_ROOT = ROOT / "tmp" / "gongwen-skill-eval"

# ---------------------------------------------------------------------------
# 模型矩阵与提供商
# ---------------------------------------------------------------------------

PROVIDERS: dict[str, dict[str, str]] = {
    "qny": {"base": "https://api.qnaigc.com", "env": "QNY_API_KEY"},
    "dmxapi": {"base": "https://www.dmxapi.cn", "env": "DMXAPI_API_KEY"},
    "siliconflow": {"base": "https://api.siliconflow.cn", "env": "SILICONFLOW_API_KEY"},
}

# 各提供商的并发与最小请求间隔。七牛（qny）按 RPM 限流，实测 4 并发时会出现
# `rate_limit_rpm_exceeded`，所以单独限并发并拉开间隔；其余两家也轻量节流。
PROVIDER_LIMITS: dict[str, dict[str, float]] = {
    "qny": {"concurrency": 2, "min_interval": 2.0},
    "dmxapi": {"concurrency": 2, "min_interval": 1.0},
    "siliconflow": {"concurrency": 2, "min_interval": 1.0},
}

URL_PROVIDER_HINTS = [
    ("qnaigc", "qny"),
    ("dmxapi", "dmxapi"),
    ("siliconflow", "siliconflow"),
]

MODEL_MATRIX: dict[str, list[str]] = {
    "qny": [
        "minimax/minimax-m2.7",
        "qwen/qwen3.8-flash-next",
        "deepseek/deepseek-v4-flash",
    ],
    "dmxapi": ["MiniMax-M2.7", "deepseek-v4-flash"],
    "siliconflow": ["deepseek-ai/DeepSeek-V4-Flash", "Qwen/Qwen3.8-27B"],
}

# 基线只跑七牛三个模型：3 模型 × 6 文种 × 2 条件 = 36 次。
BASELINE_MATRIX: dict[str, list[str]] = {"qny": MODEL_MATRIX["qny"]}

# 交叉验证按任务要求只看条件 A，每个模型 6 个文种。
CROSS_MATRIX: dict[str, list[str]] = {
    "dmxapi": ["MiniMax-M2.7", "deepseek-v4-flash"],
    "siliconflow": ["deepseek-ai/DeepSeek-V4-Flash", "Qwen/Qwen3.8-27B"],
}

KINDS = [
    "official-letter",
    "plain-document",
    "meeting-agenda",
    "white-paper",
    "phone-notice",
    "research-report",
]

KIND_LABELS = {
    "official-letter": "公函",
    "plain-document": "普通公文",
    "meeting-agenda": "会议议程",
    "white-paper": "白头件（请示）",
    "phone-notice": "电话通知",
    "research-report": "研究报告",
}

TEMPLATE_FILES = {kind: f"templates/{kind}.md" for kind in KINDS}

# 与 check_gongwen_md.py 的标记口径一致；用于自动判定「附件区段是否缺失」。
ATTACHMENT_MARKER_RE = re.compile(r"<!--\s*[\[【]?\s*(?:附件|附录|attachment|appendix)", re.I)
BODY_MARKER_RE = re.compile(r"<!--\s*[\[【]?\s*(?:正文|body)", re.I)

# ---------------------------------------------------------------------------
# 固定用例素材（写死在脚本里，便于复现；含明确时间地点与数据，方便发现编造）
# ---------------------------------------------------------------------------

CASES: dict[str, dict[str, str]] = {
    "official-letter": {
        "request": (
            "文种是公函（平行文），主送单位是 XX 市自然资源和规划局（主送抬头不要写进正文，"
            "由程序「文档要素」表单生成）。请以《XX 市城市供水条例》和 2026 年度城市供水管网"
            "普查工作安排为依据，商请对方提供地下管线数据、协助核对管线冲突点，写清反馈时间、"
            "报送方式和会商安排。正文里要引用附件（写“见附件1”“见附件2”），并把两份附件的"
            "全文一并写入本稿：每份附件各用一个附件区段。结语用“特此函商，请予支持为荷。”。"
            "有附件全文时不要手写附件说明。"
        ),
        "material": (
            "【背景】根据《XX 市城市供水条例》和《XX 市 2026 年度城市供水管网普查工作方案》，"
            "市水务局牵头开展全市供水管网普查，需要与市自然资源和规划局共享地下管线数据。\n"
            "【商请事项一】请提供 2023 年 1 月 1 日至 2025 年 12 月 31 日期间，中心城区（含高新区、"
            "经开区）供水管网覆盖范围内的地下管线数据，包括管线走向、埋深、管径、材质、权属单位，"
            "格式为 Shapefile 和 Excel 各一份。\n"
            "【商请事项二】请协助核对已排查出的 32 处供水管线与燃气、电力、通信管线冲突点，"
            "于 2026 年 11 月 20 日（星期五）17:00 前将核对结果及电子版数据反馈市水务局供排水管理科。\n"
            "【会商安排】2026 年 12 月 5 日（星期六）9:30 在市水务局三楼会议室召开数据会商会议，"
            "请对方派 1 名负责同志和 1 名技术负责人参加。\n"
            "【附件1】《XX 市供水管网普查数据需求清单》，表头为“序号 | 数据项 | 格式要求 | 备注”，"
            "共 4 行：① 管线走向，Shapefile，含坐标系说明；② 埋深与管径，Excel，单位毫米；"
            "③ 管线材质，Excel，按材质分类；④ 权属单位，Excel，注明管理单位全称。\n"
            "【附件2】《2026 年度供水管网普查工作时间安排》，内容为：2026 年 11 月 20 日前完成"
            "数据反馈；11 月 25 日至 30 日完成冲突点核对；12 月 5 日召开数据会商会议；"
            "12 月 20 日前形成普查阶段报告。"
        ),
        "expect_attachments": True,
    },
    "plain-document": {
        "request": (
            "文种是普通公文。请起草《2026 年度全市政务数据资源共享开放工作要点》。正文要有"
            "“总体要求”“主要任务”“保障措施”几部分，其中“主要任务”用一张序号表（分组行 + "
            "事项行，按序号表写法）列出 6 项重点任务；另用一张普通表格列出 2026 年第三季度"
            "共享数据统计（3 行数据）。不要写发文单位、主送、落款、成文日期。"
        ),
        "material": (
            "【总体要求】以数据要素市场化配置改革为主线，2026 年底前实现市级部门政务数据目录"
            "编制覆盖率 100%，高频共享数据接口平均响应时间压缩至 500 毫秒以内。\n"
            "【基本情况·统计表数据】2026 年第三季度：7 月共享数据量 1280 万条、接口调用 96.4 万次、"
            "同比增长 12.5%；8 月共享数据量 1345 万条、接口调用 103.2 万次、同比增长 14.1%；"
            "9 月共享数据量 1412 万条、接口调用 110.8 万次、同比增长 15.3%。\n"
            "【主要任务·序号表数据】分两组。基础支撑组：① 完善政务数据目录编制，责任单位市大"
            "数据中心，完成时限 2026 年 11 月底；② 升级共享交换平台，责任单位市大数据中心，"
            "完成时限 2026 年 12 月底；③ 建立数据质量抽检机制，责任单位市数据局，完成时限"
            "2026 年 10 月底。应用赋能组：④ 发布第三批高频共享清单，责任单位市数据局，完成时限"
            "2026 年 11 月底；⑤ 推进“一网通办”数据直连，责任单位市政务服务中心，完成时限"
            "2026 年 12 月底；⑥ 开展公共数据授权运营试点，责任单位市大数据中心，完成时限"
            "2026 年 12 月底。\n"
            "【保障措施】加强组织领导，市数据局牵头每月调度一次；强化安全监管，落实数据分类"
            "分级保护；纳入年度考核，2026 年 12 月开展评估。"
        ),
        "expect_attachments": False,
    },
    "meeting-agenda": {
        "request": (
            "文种是会议议程。请严格按公文助手的会议议程固定骨架写：第一行一个 # 标题，然后"
            "依次是“一、时间地点：”“二、参加人员：”“三、研讨内容：”三行，研讨内容下用 "
            "1. 2. 3. 4. 编号，不加会议目的、背景、工作要求等骨架外段落。"
        ),
        "material": (
            "【会议标题】全市冬季供水保障工作专题会议议程\n"
            "【时间地点】2026 年 11 月 6 日（星期五）14:30，市水务局三楼会议室。\n"
            "【参加人员】局分管领导，供排水管理科、安全监督科、市供水集团负责人，各区水务局"
            "分管负责同志。\n"
            "【研讨内容】① 通报 2026 年迎峰度冬供水保障准备情况（市供水集团）；② 研究城区老旧"
            "管网抢修物资储备方案（供排水管理科）；③ 部署低温冰冻天气应急值守工作（安全监督科）；"
            "④ 局分管领导讲话。"
        ),
        "expect_attachments": False,
    },
    "white-paper": {
        "request": (
            "文种是白头件（内部呈批件），标题是请示。按呈批件写法：大节用 ## 标题；段内枚举"
            "写成“一是……；二是……；三是……。”，不得使用 - 或 1. 列表；结尾写“妥否，请批示。”。"
            "不要写主送领导、落款、成文日期、联系人。"
        ),
        "material": (
            "【背景】2026 年全市计划改造老旧供水管网 86 公里，截至 9 月 30 日已完成 61 公里，"
            "剩余 25 公里涉及 3 个街道、12 个小区、约 1.8 万户居民。\n"
            "【资金情况】年度预算 4200 万元，已支出 3480 万元，剩余 720 万元；剩余工程概算"
            "需 1150 万元，资金缺口 430 万元。\n"
            "【前期工作】一是完成剩余 25 公里管网勘察设计；二是完成施工和监理招标；三是落实"
            "交通疏解和停水告知方案。\n"
            "【下步建议】建议追加 2026 年度供水管网改造专项资金 430 万元；2026 年 11 月 15 日前"
            "完成审批；2026 年 12 月 20 日前完工。"
        ),
        "expect_attachments": False,
    },
    "phone-notice": {
        "request": (
            "文种是电话通知（下行文）。请起草通知，写明排查、加装保温材料、应急值守三项要求，"
            "明确时间节点和报告渠道；对下级用“要”“确保”等；结语“特此通知。”。不要写主送、"
            "落款、成文日期、联系人电话。"
        ),
        "material": (
            "【依据】气象部门预测，2026 年 12 月至 2027 年 2 月全市平均气温较常年偏低 1—2℃，"
            "极端最低气温可能降至零下 12℃。\n"
            "【要求一】各区水务局、市供水集团要于 2026 年 11 月 10 日前完成所辖供水设施防冻"
            "保温排查，建立问题清单。\n"
            "【要求二】要于 2026 年 11 月 30 日前完成保温材料加装和裸露管道包覆，确保不留死角。\n"
            "【要求三】要建立 24 小时应急值班制度，发现管道冻裂、爆管等情况 2 小时内向市水务局"
            "值班室报告；市水务局于 2026 年 12 月 1 日至 3 日组织抽查。\n"
            "【报告渠道】有关情况向市水务局供排水管理科反映。"
        ),
        "expect_attachments": False,
    },
    "research-report": {
        "request": (
            "文种是研究报告。请按研究报告写法起草：第一行 # 报告题名；正文前有 <!-- [目录] -->、"
            "<!-- [摘要] -->、<!-- [正文] --> 区段；正文分“研究背景”“现状分析”“问题成因”"
            "“对策建议”四章（## 标题）；“现状分析”章标题挂锚点 {#sec:status}，正文中用 "
            "{@sec:status} 交叉引用一次；提供一张漏损率表格，表题写成“表：2023—2025 年全市"
            "公共供水管网漏损率”，表题挂锚点 {#tbl:loss}，正文引用 {@tbl:loss}；材料没有提供"
            "文献库，不要写文献引用；不要手写章节编号。"
        ),
        "material": (
            "【调研主题】城市公共供水管网漏损治理路径研究。\n"
            "【数据】2023 年、2024 年、2025 年全市公共供水管网漏损率分别为 11.2%、10.4%、9.8%；"
            "供水管网总长 3860 公里，其中服役超过 20 年的老旧管网占 27.4%；分区计量覆盖率为 68%。\n"
            "【调研过程】2026 年 5 月至 7 月，选取 3 个区、6 个街道、12 个小区开展实地调研，"
            "回收入户问卷 480 份，访谈供水企业、物业和居民代表 26 人。\n"
            "【主要发现】漏损集中在老旧管网和二次供水设施；夜间最小流量监测覆盖率不足；"
            "部分小区计量表具超期未检定；抢修响应平均用时 47 分钟。\n"
            "【对策建议】推进分区计量和压力调控，2027 年底前分区计量覆盖率提高到 90%；"
            "分三年改造服役超过 20 年的老旧管网；建设智慧漏损监测平台；完善计量表具周期检定制度。"
        ),
        "expect_attachments": False,
    },
}

# ---------------------------------------------------------------------------
# 失败模式分类
# ---------------------------------------------------------------------------

# 固定分类（与任务要求一致；另把「首行不是唯一主标题」单列，便于文档作者定位）。
MODE_LABELS = {
    "manual-numbering": "手写编号",
    "number-space-title": "标题以“数字+空格”开头（年份也会被剥）",
    "header-footer-elements": "写了版头版记要素（文号/落款/日期/主送/抄送/联系人/密级）",
    "code-fence-residue": "代码围栏未剥净",
    "attachment-marker": "附件标记/附件说明写法错",
    "attachment-1-title": "附件标题写了“附件1/附件1：”",
    "agenda-skeleton": "会议议程骨架被改",
    "table-columns": "表格列数不一致",
    "table-separator": "表格缺少合格分隔行（整表会按正文印出）",
    "list-in-approval": "呈批件用了列表",
    "relative-date": "相对日期",
    "research-marker-anchor": "研究报告标记/锚点/引用错",
    "extra-explanation": "输出夹带解释/清单",
    "title-first-line": "首行不是唯一的 # 主标题",
    "other": "其他",
}

MODE_ORDER = list(MODE_LABELS)

ELEMENT_LINE_RE = re.compile(
    r"^(?:主送(?:单位)?[：:]|抄送(?:单位)?[：:]|承办(?:单位)?[：:]|联系人[：:]|联系电话[：:]"
    r"|发文字号[：:]|密级[：:]|保密期限[：:]|印发|签发人[：:]|发文机关[：:]|落款(?:单位)?[：:])"
)
DATE_ONLY_RE = re.compile(
    r"^[0-9〇一二三四五六七八九十]{2,4}年[0-9〇一二三四五六七八九十]{1,2}月"
    r"[0-9〇一二三四五六七八九十]{1,3}日$"
)
RELATIVE_DATE_RE = re.compile(r"今天|明天|后天|大后天|昨天|本周[一二三四五六日天]|下周[一二三四五六日天]?")
LIST_ITEM_RE = re.compile(r"^(?:\d{1,9}\.\s|[-*]\s)")
MANUAL_PARAGRAPH_RE = re.compile(r"^[一二三四五六七八九十百零]+、|^[（(][一二三四五六七八九十百零]+[）)]")
CHATTER_TAIL_RE = re.compile(
    r"^(?:以上(?:是|为|就)|希望(?:对|能|可以)|如需|如您|如有(?:不妥|需要)|注[:：]|说明[:：]|备注[:：]|"
    r"（注|\(注)"
)


def classify_clean(
    kind: str, clean: str, raw_text: str, messages: list[dict]
) -> tuple[list[str], dict[str, str], list[dict]]:
    """由清洗稿 + check 逐条 message 得出失败模式、级别与证据。"""
    mode_hits: dict[str, list[str]] = {}
    for message in messages:
        if message.get("level") != "ERROR":
            continue
        mode = classify_check_message(message.get("message", ""))
        mode_hits.setdefault(mode, []).append(message.get("message", ""))
    for mode, evidence in classify_text(kind, clean, raw_text, CASES[kind]):
        mode_hits.setdefault(mode, []).append(evidence)
    ordered = [mode for mode in MODE_ORDER if mode in mode_hits]
    levels = mode_levels(messages, ordered)
    evidence_list = [{"mode": mode, "evidence": mode_hits[mode][:3]} for mode in ordered]
    return ordered, levels, evidence_list


def _is_heading(line: str) -> bool:
    return re.match(r"^#{1,6}\s+", line) is not None


def _strip_quoted(line: str) -> str:
    return re.sub(r"[“‘「『][^”’」』]*[”’」』]", "", line)


def classify_check_message(message: str) -> str:
    """把 check_gongwen_md.py 的一条 message 归到失败模式。"""
    if "手写了编号" in message:
        # 「2026 年第三季度…」这类是「数字+空格」被剥，单列，便于文档作者定位。
        if re.search(r"标题手写了编号「\s*\d+\s", message):
            return "number-space-title"
        return "manual-numbering"
    if "不构成合格表格" in message or "没有分隔行" in message:
        return "table-separator"
    if "版头" in message or "成文日期" in message:
        return "header-footer-elements"
    if "代码围栏" in message or "代码块" in message or "mermaid 围栏" in message:
        return "code-fence-residue"
    if "白头件" in message:
        return "attachment-marker"
    if "附件" in message:
        return "attachment-marker"
    if "会议议程" in message or "研讨内容" in message:
        return "agenda-skeleton"
    if "表格" in message or "分隔行" in message or "格，多于表头" in message:
        return "table-columns"
    if "呈批件不得使用列表" in message:
        return "list-in-approval"
    if "相对日期" in message:
        return "relative-date"
    if "锚点" in message or "交叉引用" in message or "脚注" in message:
        return "research-marker-anchor"
    if "第一行应是" in message or "内容为空" in message:
        return "title-first-line"
    if "第二个 `#` 标题" in message:
        return "title-first-line"
    return "other"


def mode_levels(messages: list[dict], modes: list[str]) -> dict[str, str]:
    """标出每个失败模式的最高级别：有对应 ERROR 记 ERROR，否则记 WARN。

    附件标题写“附件1”这类问题 check 脚本只给 WARN；报告里要能区分，
    否则「通过率」会被误读。
    """
    levels = {mode: "WARN" for mode in modes}
    for message in messages:
        if message.get("level") != "ERROR":
            continue
        mode = classify_check_message(message.get("message", ""))
        if mode in levels:
            levels[mode] = "ERROR"
    return levels


def classify_text(kind: str, clean: str, raw: str, case: dict) -> list[tuple[str, str]]:
    """文本层面的失败模式补充（check 脚本看不到、但确实是格式错的问题）。

    返回 (mode, evidence) 列表。只判断客观可复现的格式问题，不评价内容。
    """
    found: list[tuple[str, str]] = []
    lines = clean.splitlines()
    stripped = [line.strip() for line in lines]
    nonblank = [(i + 1, line) for i, line in enumerate(stripped) if line]

    # 首行 / 夹带解释
    if nonblank and not nonblank[0][1].startswith("# "):
        found.append(("title-first-line", f"首行：{nonblank[0][1][:60]}"))
    elif not nonblank:
        found.append(("title-first-line", "清洗后内容为空"))
    if nonblank and nonblank[0][1].startswith("# "):
        # 主标题之前还有非空行 = 未剥净的寒暄
        pass
    if nonblank:
        tail = nonblank[-1][1]
        if CHATTER_TAIL_RE.match(tail) and not _is_heading(tail):
            found.append(("extra-explanation", f"文末：{tail[:60]}"))
    if raw.strip() and not clean.strip():
        found.append(("extra-explanation", "清洗后无正文，全部被判定为寒暄/围栏"))

    # 代码围栏残留（清洗只剥最外层空/markdown 围栏）
    if re.search(r"(?m)^\s*```", clean):
        fence_lines = [line for line in stripped if line.startswith("```")]
        if fence_lines:
            found.append(("code-fence-residue", f"正文仍有围栏：{fence_lines[0][:40]}"))

    # 版头版记要素（check 只在正文段内报，这里补充文末日期与常见抬头）
    for number, line in nonblank:
        if line.startswith(("|", "<!--", "#")):
            continue
        if ELEMENT_LINE_RE.match(line):
            found.append(("header-footer-elements", f"第{number}行：{line[:50]}"))
            break
    tail_lines = nonblank[-3:]
    for number, line in tail_lines:
        if DATE_ONLY_RE.match(line):
            found.append(("header-footer-elements", f"文末手写日期：{line}"))
            break

    # 相对日期
    for number, line in nonblank:
        cleaned = _strip_quoted(line)
        if kind not in {"research-report"} and RELATIVE_DATE_RE.search(cleaned):
            found.append(("relative-date", f"第{number}行：{line[:50]}"))
            break

    # 呈批件列表
    if kind in {"white-paper", "red-head-approval"}:
        for number, line in nonblank:
            if LIST_ITEM_RE.match(line):
                found.append(("list-in-approval", f"第{number}行：{line[:50]}"))
                break

    # 手写编号：本该用 # 的地方写成了段落编号（会议议程的“一、”是唯一例外）
    if kind != "meeting-agenda":
        in_attachment = False
        in_table = False
        for number, line in nonblank:
            if line.startswith("<!--") and re.search(r"附件|附录|attachment|appendix", line):
                in_attachment = True
            if line.startswith("|"):
                in_table = True
                continue
            in_table = False
            if in_attachment or in_table or _is_heading(line) or line.startswith("<!--"):
                continue
            # “一是……”“二是……”是正文枚举，不算手写标题编号
            if re.match(r"^[一二三四五六七八九十]+是", line):
                continue
            if MANUAL_PARAGRAPH_RE.match(line):
                found.append(("manual-numbering", f"第{number}行：{line[:50]}"))
                break

    # 附件：要求写附件却整段缺失 / 白头件带了附件区段 / 写了“附件1”式假标题
    if kind == "white-paper" and ATTACHMENT_MARKER_RE.search(clean):
        found.append(("attachment-marker", "白头件出现附件区段标记"))
    if case.get("expect_attachments") and not ATTACHMENT_MARKER_RE.search(clean):
        found.append(("attachment-marker", "素材要求带附件，正文没有 <!-- [附件] --> 区段"))
    for number, line in nonblank:
        if re.match(r"^#{1,6}\s*附件\s*[0-9一二三四五六七八九十]", line) or re.match(
            r"^附件\s*[0-9一二三四五六七八九十]\s*[：:、]", line
        ):
            found.append(("attachment-1-title", f"第{number}行：{line[:50]}"))
            break

    # 表格：与当前 check 脚本同一判据（含 | 且 ≥2 格才算一行；分隔行每格 ≥3 短横、≥2 格）
    index = 0
    while index < len(stripped):
        line = stripped[index]
        if not _is_table_row(line):
            index += 1
            continue
        if index + 1 < len(stripped) and _is_table_separator(stripped[index + 1]):
            width = _table_cells(stripped[index + 1])
            if _table_cells(line) != width:
                found.append(("table-columns", f"第{index + 1}行表头列数与分隔行不一致"))
                break
            j = index + 2
            bad = False
            while j < len(stripped) and _is_table_row(stripped[j]):
                if _table_cells(stripped[j]) > width:
                    found.append(("table-columns", f"第{j + 1}行有 {_table_cells(stripped[j])} 格，多于表头 {width} 列"))
                    bad = True
                    break
                j += 1
            if bad:
                break
            index = j
            continue
        start = index
        while index < len(stripped) and _is_table_row(stripped[index]):
            index += 1
        rows = index - start
        following = stripped[index] if index < len(stripped) else ""
        if rows >= 2 or _looks_like_separator(following):
            found.append(("table-separator", f"第{start + 1}行起 {rows} 行含 | 但没有合格分隔行"))
            break

    # 标题以“数字+空格”开头（解析器只对 `##`~`#####` 剥编号，`#` 文档标题不算）
    for number, line in nonblank:
        if re.match(r"^#{2,5}\s+\d+\s", line):
            found.append(("number-space-title", f"第{number}行：{line[:50]}"))
            break

    # 研究报告标记缺失
    if kind == "research-report":
        if not BODY_MARKER_RE.search(clean):
            found.append(("research-marker-anchor", "缺少 <!-- [正文] --> 区段标记"))
        missing = [marker for marker in ("<!-- [摘要] -->", "<!-- [目录] -->") if marker not in clean]
        if missing:
            found.append(("research-marker-anchor", "缺少区段标记：" + "、".join(missing)))

    # 议程骨架（正文层面兜底）
    if kind == "meeting-agenda":
        for label in ("一、时间地点：", "二、参加人员：", "三、研讨内容："):
            if not any(line.startswith(label) for line in stripped):
                found.append(("agenda-skeleton", f"缺少固定行“{label}”"))
        for number, line in nonblank[1:]:
            if line.startswith("#") or line.startswith(("- ", "* ", "|")):
                found.append(("agenda-skeleton", f"第{number}行：{line[:50]}"))
                break

    return found


def _table_cells(line: str) -> int:
    value = line.strip()
    if value.startswith("|"):
        value = value[1:]
    if value.endswith("|"):
        value = value[:-1]
    return len(value.split("|"))


def _is_table_row(line: str) -> bool:
    return "|" in line and _table_cells(line) >= 2


def _is_table_separator(line: str) -> bool:
    value = line.strip()
    cells = value.split("|")
    if value.startswith("|"):
        cells = cells[1:]
    if value.endswith("|"):
        cells = cells[:-1]
    if len(cells) < 2:
        return False
    return all(
        len(cell.strip().strip(":")) >= 3 and set(cell.strip().strip(":")) <= {"-"}
        for cell in cells
    )


def _looks_like_separator(line: str) -> bool:
    """像分隔行但不是合格分隔行的行（与 check 脚本同一判据）。"""
    value = line.strip()
    if "-" not in value or "|" not in value:
        return False
    return re.fullmatch(r"[|\-:\s]+", value) is not None


# ---------------------------------------------------------------------------
# 密钥读取与 HTTP（必须走 Python urllib/ssl：本机 schannel 的 TLS 会失败）
# ---------------------------------------------------------------------------


class EvalError(RuntimeError):
    pass


class RateGate:
    """按提供商限制并发数并拉开请求起始间隔（七牛按 RPM 限流）。"""

    def __init__(self, concurrency: int, min_interval: float) -> None:
        import threading

        self._semaphore = threading.BoundedSemaphore(max(1, concurrency))
        self._lock = threading.Lock()
        self._min_interval = max(0.0, min_interval)
        self._next_start = 0.0

    def __enter__(self) -> "RateGate":
        self._semaphore.acquire()
        with self._lock:
            now = time.monotonic()
            wait = max(0.0, self._next_start - now)
            if wait:
                time.sleep(wait)
                now = time.monotonic()
            self._next_start = now + self._min_interval
        return self

    def __exit__(self, *_exc_info: object) -> None:
        self._semaphore.release()


def normalize_base(url: str) -> str:
    base = url.strip().rstrip("/")
    if base.endswith("/v1"):
        base = base[: -len("/v1")]
    return base


def provider_from_url(url: str) -> str | None:
    low = url.lower()
    for hint, provider in URL_PROVIDER_HINTS:
        if hint in low:
            return provider
    return None


def load_keys(keys_file: Path | None) -> dict[str, dict[str, str]]:
    """从密钥文件与环境变量读取提供商地址与密钥；读不到就报错退出。

    密钥文件格式（本机 gitignore，不入库）：标题行，然后若干组
    `url:<地址>` / 地址行 + 紧跟的一行密钥；组前可能有 `qny`、`dmxapi` 这类提供商名。
    """
    credentials: dict[str, dict[str, str]] = {}
    if keys_file and keys_file.is_file():
        lines = keys_file.read_text(encoding="utf-8-sig").splitlines()
        index = 0
        pending: str | None = None
        while index < len(lines):
            line = lines[index].strip()
            index += 1
            if not line or line.startswith("#"):
                continue
            low = line.lower()
            if low in PROVIDERS:
                pending = low
                continue
            url = None
            if low.startswith("url:"):
                url = line[4:].strip()
            elif low.startswith("http://") or low.startswith("https://"):
                url = line
            if url is None:
                pending = None
                continue
            while index < len(lines) and not lines[index].strip():
                index += 1
            if index >= len(lines):
                break
            key = lines[index].strip()
            index += 1
            provider = provider_from_url(url) or pending
            if provider and provider in PROVIDERS:
                credentials[provider] = {
                    "base": normalize_base(url),
                    "key": key,
                }
            pending = None
    env_map = {
        "qny": os.environ.get("QNY_API_KEY") or os.environ.get("QNAIGC_API_KEY"),
        "dmxapi": os.environ.get("DMXAPI_API_KEY"),
        "siliconflow": os.environ.get("SILICONFLOW_API_KEY"),
    }
    for provider, key in env_map.items():
        if key:
            credentials.setdefault(
                provider,
                {"base": PROVIDERS[provider]["base"], "key": key.strip()},
            )
            if not credentials[provider].get("key"):
                credentials[provider]["key"] = key.strip()
    return credentials


def build_url(base: str, path: str) -> str:
    return f"{base.rstrip('/')}{path}"


def http_json(
    method: str,
    url: str,
    key: str,
    payload: dict | None,
    timeout: float,
) -> tuple[int, dict]:
    data = None if payload is None else json.dumps(payload, ensure_ascii=False).encode("utf-8")
    request = urllib.request.Request(url, data=data, method=method)
    request.add_header("Accept", "application/json")
    request.add_header("Content-Type", "application/json")
    request.add_header("Authorization", f"Bearer {key}")
    request.add_header("User-Agent", "gongwen-skill-eval/1.0")
    context = ssl.create_default_context()
    with urllib.request.urlopen(request, timeout=timeout, context=context) as response:
        body = response.read().decode("utf-8", errors="replace")
        return response.status, json.loads(body)


def chat_completion(
    provider: str,
    base: str,
    key: str,
    model: str,
    messages: list[dict],
    max_tokens: int,
    timeout: float,
    retries: int,
) -> dict:
    url = build_url(base, "/v1/chat/completions")
    payload = {
        "model": model,
        "messages": messages,
        "temperature": 0.3,
        "stream": False,
        "max_tokens": max_tokens,
    }
    last_error = ""
    for attempt in range(retries + 1):
        try:
            status, body = http_json("POST", url, key, payload, timeout)
        except urllib.error.HTTPError as error:
            detail = ""
            try:
                detail = error.read().decode("utf-8", errors="replace")
            except Exception:  # noqa: BLE001 - 读错误体失败不影响主流程
                detail = ""
            last_error = f"HTTP {error.code}: {detail[:500]}"
            # 少数兼容层只认 max_completion_tokens
            if error.code == 400 and "max_tokens" in detail and "max_completion_tokens" not in payload:
                payload.pop("max_tokens", None)
                payload["max_completion_tokens"] = max_tokens
                continue
            if error.code in {408, 409, 425, 429, 500, 502, 503, 504} and attempt < retries:
                time.sleep(min(60.0, 6.0 * (attempt + 1)) + random.uniform(0, 1.5))
                continue
            raise EvalError(last_error) from error
        except (urllib.error.URLError, TimeoutError, ssl.SSLError, ConnectionError) as error:
            last_error = f"{type(error).__name__}: {error}"
            if attempt < retries:
                time.sleep(min(60.0, 6.0 * (attempt + 1)) + random.uniform(0, 1.5))
                continue
            raise EvalError(last_error) from error
        except json.JSONDecodeError as error:
            last_error = f"响应不是 JSON：{error}"
            if attempt < retries:
                time.sleep(2.0 * (attempt + 1))
                continue
            raise EvalError(last_error) from error
        if isinstance(body, dict) and body.get("error"):
            last_error = json.dumps(body["error"], ensure_ascii=False)[:500]
            if attempt < retries:
                time.sleep(2.0 * (attempt + 1))
                continue
            raise EvalError(last_error)
        return body
    raise EvalError(last_error or "未知网络错误")


def extract_choice(body: dict) -> tuple[str, str | None, dict]:
    choices = body.get("choices") or []
    if not choices:
        raise EvalError("响应没有 choices")
    choice = choices[0] or {}
    message = choice.get("message") or {}
    content = message.get("content")
    if content is None:
        content = ""
    return str(content), choice.get("finish_reason"), body.get("usage") or {}


def usage_numbers(usage: dict) -> tuple[int | None, int | None, int | None]:
    prompt = usage.get("prompt_tokens", usage.get("input_tokens"))
    completion = usage.get("completion_tokens", usage.get("output_tokens"))
    total = usage.get("total_tokens")
    if total is None and isinstance(prompt, int) and isinstance(completion, int):
        total = prompt + completion
    return prompt, completion, total


# ---------------------------------------------------------------------------
# 清洗：严格复刻 src/prompt.rs 的 sanitize_model_markdown（只做三件事）
# ---------------------------------------------------------------------------


def strip_think_blocks(raw: str) -> str:
    text = raw.strip()
    for open_tag, close_tag in (("<think>", "</think>"), ("<thinking>", "</thinking>")):
        while True:
            start = text.find(open_tag)
            if start == -1:
                break
            end = text.find(close_tag, start)
            if end == -1:
                # 输出中断时，未闭合的思考块后面仍属于推理，不能当作正文交付。
                text = text[:start]
                break
            text = text[:start] + text[end + len(close_tag) :]
    return text.strip()


def unwrap_code_fence(text: str) -> str:
    lines = text.splitlines()
    fences = [index for index, line in enumerate(lines) if line.lstrip().startswith("```")]
    if len(fences) < 2:
        return text.strip()
    open_index, close_index = fences[0], fences[-1]
    if open_index == close_index:
        return text.strip()
    info = lines[open_index].strip()
    while info.startswith("```"):
        info = info[3:].strip()
    if info not in {"", "markdown", "md", "text"}:
        return text.strip()
    return "\n".join(lines[open_index + 1 : close_index]).strip()


LEADING_MARKERS = ["以下是", "下面是", "这是", "好的", "已按", "已经按"]
TRAILING_MARKERS = ["以上是", "以上为", "希望对", "如有不妥之处请指正", "如需调整", "如还需要"]


def is_chatter_line(line: str, markers: list[str]) -> bool:
    value = line.strip()
    if not value:
        return True
    if value.startswith("#") or value.startswith("|") or value.startswith("<!--"):
        return False
    if len(value) > 40:
        return False
    return any(marker in value for marker in markers)


def strip_chatter(text: str) -> str:
    lines = text.splitlines()
    title = next((index for index, line in enumerate(lines) if line.lstrip().startswith("# ")), None)
    if title is not None:
        preamble = [line for line in lines[:title] if line.strip()]
        if all(is_chatter_line(line, LEADING_MARKERS) for line in preamble):
            lines = lines[title:]
    while lines and is_chatter_line(lines[-1], TRAILING_MARKERS):
        lines.pop()
    return "\n".join(lines).strip()


def sanitize_model_markdown(raw: str) -> str:
    """顺序固定：思考块 → 围栏 → 寒暄（与 Rust 实现一致）。"""
    text = strip_think_blocks(raw)
    text = unwrap_code_fence(text)
    return strip_chatter(text)


# ---------------------------------------------------------------------------
# check_gongwen_md.py 调用
# ---------------------------------------------------------------------------


CHECK_LINE_RE = re.compile(r"^(ERROR|WARN)\t(第\d+行|全文)\t(.*)$")


def run_check(clean_file: Path, kind: str, timeout: float, check_script: Path = CHECK_SCRIPT) -> dict:
    command = [sys.executable, str(check_script), str(clean_file), "--kind", kind]
    env = dict(os.environ)
    env["PYTHONIOENCODING"] = "utf-8"
    try:
        completed = subprocess.run(
            command,
            cwd=str(ROOT),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            env=env,
        )
    except subprocess.TimeoutExpired:
        return {
            "exit_code": None,
            "messages": [],
            "stderr": f"check 脚本超时（{timeout}s）",
            "script_error": True,
        }
    messages = []
    for line in completed.stdout.splitlines():
        match = CHECK_LINE_RE.match(line.strip())
        if match:
            level, where, message = match.groups()
            number = 0
            if where != "全文":
                number = int(re.sub(r"\D", "", where) or 0)
            messages.append({"level": level, "line": number, "message": message})
    script_error = completed.returncode not in (0, 1)
    return {
        "exit_code": completed.returncode,
        "messages": messages,
        "stderr": completed.stderr[-2000:],
        "script_error": script_error,
    }


# ---------------------------------------------------------------------------
# 技能文件快照（保证一次 run 内提示词内容固定、两轮 before/after 可追版本）
# ---------------------------------------------------------------------------


def iter_skill_files(skill_dir: Path) -> list[Path]:
    files = [
        path
        for path in skill_dir.rglob("*")
        if path.is_file() and "__pycache__" not in path.parts and path.suffix != ".pyc"
    ]
    return sorted(files, key=lambda path: path.relative_to(skill_dir).as_posix())


def skill_tree_sha256(skill_dir: Path) -> str:
    """对技能目录下所有（文本）文件的相对路径与内容算一个哈希，用于证明用的是哪版文档。"""
    digest = hashlib.sha256()
    for path in iter_skill_files(skill_dir):
        relative = path.relative_to(skill_dir).as_posix()
        digest.update(relative.encode("utf-8"))
        digest.update(b"\n")
        digest.update(hashlib.sha256(path.read_bytes()).hexdigest().encode("ascii"))
        digest.update(b"\n")
    return digest.hexdigest()


def load_skill_snapshot(skill_dir: Path) -> dict:
    """载入技能树文本。

    技能包会被其他任务改动，若每次调用都读磁盘，同一次 run 会混用不同版本；
    这里一次性载入，run 内所有请求都用同一份文本。
    """
    texts: dict[str, str] = {}
    for path in iter_skill_files(skill_dir):
        relative = path.relative_to(skill_dir).as_posix()
        try:
            texts[relative] = path.read_text(encoding="utf-8-sig")
        except UnicodeDecodeError:
            continue
    for required in ["SKILL.md", *TEMPLATE_FILES.values()]:
        if required not in texts:
            raise EvalError(f"技能目录缺少文件：{skill_dir / required}")
    metadata = {
        "skill_dir": str(skill_dir),
        "skill_tree_sha256": skill_tree_sha256(skill_dir),
        "files": {
            relative: {
                "sha256": hashlib.sha256(text.encode("utf-8")).hexdigest(),
                "bytes": len(text.encode("utf-8")),
            }
            for relative, text in sorted(texts.items())
        },
    }
    return {"texts": texts, "metadata": metadata}


def write_skill_snapshot(run_dir: Path, snapshot: dict) -> None:
    snapshot_dir = run_dir / "skill-snapshot"
    for relative, text in snapshot["texts"].items():
        path = snapshot_dir / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    (snapshot_dir / "SOURCES.json").write_text(
        json.dumps(snapshot["metadata"], ensure_ascii=False, indent=2), encoding="utf-8"
    )


# ---------------------------------------------------------------------------
# 提示词构建
# ---------------------------------------------------------------------------


def build_system_prompt(kind: str, condition: str, texts: dict[str, str]) -> str:
    if condition == "A":
        return texts["SKILL.md"]
    parts = [
        ("SKILL.md", texts["SKILL.md"]),
        ("references/document-types.md", texts["references/document-types.md"]),
        ("references/markdown-syntax.md", texts["references/markdown-syntax.md"]),
    ]
    if kind == "research-report":
        parts.append(("references/research-report.md", texts["references/research-report.md"]))
    template = TEMPLATE_FILES[kind]
    parts.append((template, texts[template]))
    return "\n\n".join(f"===== {name} =====\n{content}" for name, content in parts)


def build_user_prompt(kind: str) -> str:
    case = CASES[kind]
    return (
        f"请按「公文助手」技能规范起草一份{KIND_LABELS[kind]}（{kind}）。\n\n"
        f"【写作要求】\n{case['request']}\n\n"
        f"【素材】\n{case['material']}\n\n"
        "请输出完整的 Markdown 正文，也就是最终 .md 文件的内容。"
    )


def build_messages(kind: str, condition: str, texts: dict[str, str]) -> list[dict]:
    return [
        {"role": "system", "content": build_system_prompt(kind, condition, texts)},
        {"role": "user", "content": build_user_prompt(kind)},
    ]


def write_prompts(run_dir: Path, texts: dict[str, str]) -> None:
    prompt_dir = run_dir / "prompts"
    prompt_dir.mkdir(parents=True, exist_ok=True)
    for kind in KINDS:
        for condition in ("A", "B"):
            path = prompt_dir / f"{kind}-{condition}.md"
            text = (
                f"# {KIND_LABELS[kind]}（{kind}）· 条件 {condition}\n\n"
                "## system\n\n"
                f"{build_system_prompt(kind, condition, texts)}\n\n"
                "## user\n\n"
                f"{build_user_prompt(kind)}\n"
            )
            path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# 单次评测
# ---------------------------------------------------------------------------


def safe_name(value: str) -> str:
    name = re.sub(r"[^0-9A-Za-z._-]+", "-", value)
    name = re.sub(r"-{2,}", "-", name).strip("-")
    return name or "model"


_PREVIOUS_CACHE: dict[str, dict] = {}


def _previous_index(run_dir: Path) -> dict:
    key = str(run_dir / "summary.json")
    if key not in _PREVIOUS_CACHE:
        index: dict = {}
        path = run_dir / "summary.json"
        if path.is_file():
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
                for task in data.get("tasks", []):
                    index[
                        (task.get("provider"), task.get("model"), task.get("kind"), task.get("condition"))
                    ] = task
            except json.JSONDecodeError:
                pass
        _PREVIOUS_CACHE[key] = index
    return _PREVIOUS_CACHE[key]


def _load_previous_result(
    run_dir: Path, provider: str, model: str, kind: str, condition: str
) -> dict:
    return _previous_index(run_dir).get((provider, model, kind, condition), {})


def evaluate_one(
    *,
    run_dir: Path,
    provider: str,
    base: str,
    key: str,
    model: str,
    kind: str,
    condition: str,
    texts: dict[str, str],
    check_script: Path,
    max_tokens: int,
    timeout: float,
    retries: int,
    force: bool,
    gate: "RateGate | None" = None,
) -> dict:
    file_stem = f"{provider}__{safe_name(model)}"
    case_dir = run_dir / condition / kind
    case_dir.mkdir(parents=True, exist_ok=True)
    raw_file = case_dir / f"{file_stem}.raw.md"
    clean_file = case_dir / f"{file_stem}.md"
    relative_raw = raw_file.relative_to(run_dir).as_posix()
    relative_clean = clean_file.relative_to(run_dir).as_posix()

    result: dict = {
        "provider": provider,
        "model": model,
        "kind": kind,
        "condition": condition,
        "status": "ok",
        "file": relative_clean,
        "raw_file": relative_raw,
        "errors": 0,
        "warnings": 0,
        "modes": [],
        "mode_levels": {},
        "mode_evidence": [],
        "messages": [],
        "prompt_tokens": None,
        "completion_tokens": None,
        "total_tokens": None,
        "reasoning_tokens": None,
        "max_tokens_used": max_tokens,
        "latency_ms": None,
        "finish_reason": None,
        "check_exit_code": None,
        "error": "",
        "script_error": False,
    }
    if clean_file.exists() and not force:
        # 复用已有产物时仍重新判定，避免改动分类口径后汇总失真；
        # 已有 summary.json 里的用量与耗时原样带过来，不在重判时丢统计。
        result["cached"] = True
        previous = _load_previous_result(run_dir, provider, model, kind, condition)
        for key in (
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "reasoning_tokens",
            "max_tokens_used",
            "latency_ms",
            "finish_reason",
            "error",
        ):
            if previous.get(key) is not None:
                result[key] = previous[key]
        clean = clean_file.read_text(encoding="utf-8")
        check = run_check(clean_file, kind, timeout=120, check_script=check_script)
        result["check_exit_code"] = check["exit_code"]
        result["script_error"] = check["script_error"]
        result["messages"] = check["messages"]
        result["errors"] = sum(1 for message in check["messages"] if message["level"] == "ERROR")
        result["warnings"] = sum(1 for message in check["messages"] if message["level"] == "WARN")
        if check["script_error"]:
            result["status"] = "check-error"
            result["error"] = check["stderr"][-500:]
            return result
        raw_text = raw_file.read_text(encoding="utf-8") if raw_file.exists() else ""
        modes, levels, evidence = classify_clean(kind, clean, raw_text, check["messages"])
        result["modes"] = modes
        result["mode_levels"] = levels
        result["mode_evidence"] = evidence
        return result

    started = time.monotonic()
    try:
        if gate is not None:
            with gate:
                body = chat_completion(
                    provider,
                    base,
                    key,
                    model,
                    build_messages(kind, condition, texts),
                    max_tokens,
                    timeout,
                    retries,
                )
        else:
            body = chat_completion(
                provider,
                base,
                key,
                model,
                build_messages(kind, condition, texts),
                max_tokens,
                timeout,
                retries,
            )
        content, finish_reason, usage = extract_choice(body)
        # 推理型模型（DeepSeek-V4-Flash 等）的推理占用 completion 预算；预算用尽时
        # content 为空而 reasoning_content 很长，这里放大预算重试一次。
        if not str(content or "").strip() and finish_reason == "length" and max_tokens < 65536:
            bigger = min(max_tokens * 2, 65536)
            print(f"    {provider} {model} {kind} {condition} 仅输出推理，放大预算到 {bigger} 重试", flush=True)
            if gate is not None:
                with gate:
                    body = chat_completion(
                        provider, base, key, model, build_messages(kind, condition, texts),
                        bigger, timeout, retries,
                    )
            else:
                body = chat_completion(
                    provider, base, key, model, build_messages(kind, condition, texts),
                    bigger, timeout, retries,
                )
            content, finish_reason, usage = extract_choice(body)
            result["max_tokens_used"] = bigger
    except EvalError as error:
        result["status"] = "api-error"
        result["error"] = str(error)
        result["latency_ms"] = round((time.monotonic() - started) * 1000)
        return result
    result["latency_ms"] = round((time.monotonic() - started) * 1000)
    result["finish_reason"] = finish_reason
    prompt, completion, total = usage_numbers(usage)
    result["prompt_tokens"] = prompt
    result["completion_tokens"] = completion
    result["total_tokens"] = total
    details = usage.get("completion_tokens_details") or {}
    result["reasoning_tokens"] = details.get("reasoning_tokens")

    raw_file.write_text(content, encoding="utf-8")
    if not content.strip():
        result["status"] = "empty-output"
        result["error"] = (
            f"模型返回空 content（finish_reason={finish_reason}，"
            f"reasoning_tokens={result['reasoning_tokens']}）"
        )
        return result

    clean = sanitize_model_markdown(content)
    clean_file.write_text(clean, encoding="utf-8")

    check = run_check(clean_file, kind, timeout=120)
    result["check_exit_code"] = check["exit_code"]
    result["script_error"] = check["script_error"]
    messages = check["messages"]
    result["messages"] = messages
    result["errors"] = sum(1 for message in messages if message["level"] == "ERROR")
    result["warnings"] = sum(1 for message in messages if message["level"] == "WARN")
    if check["script_error"]:
        result["status"] = "check-error"
        result["error"] = check["stderr"][-500:]
        return result

    modes, levels, evidence = classify_clean(kind, clean, content, messages)
    result["modes"] = modes
    result["mode_levels"] = levels
    result["mode_evidence"] = evidence
    return result


def run_results(
    run_dir: Path,
    credentials: dict[str, dict[str, str]],
    tasks: list[tuple[str, str, str, str]],
    *,
    texts: dict[str, str],
    check_script: Path,
    max_tokens: int,
    timeout: float,
    retries: int,
    workers: int,
    force: bool,
) -> list[dict]:
    results: list[dict] = []
    gates = {
        provider: RateGate(
            int(PROVIDER_LIMITS.get(provider, {}).get("concurrency", 2)),
            float(PROVIDER_LIMITS.get(provider, {}).get("min_interval", 1.0)),
        )
        for provider in PROVIDERS
    }
    with ThreadPoolExecutor(max_workers=max(1, workers)) as executor:
        futures = {}
        for provider, model, kind, condition in tasks:
            credential = credentials.get(provider)
            if not credential:
                results.append(
                    {
                        "provider": provider,
                        "model": model,
                        "kind": kind,
                        "condition": condition,
                        "status": "missing-key",
                        "file": "",
                        "raw_file": "",
                        "errors": 0,
                        "warnings": 0,
                        "modes": [],
                        "mode_evidence": [],
                        "messages": [],
                        "error": f"没有 {provider} 的地址或密钥",
                    }
                )
                continue
            future = executor.submit(
                evaluate_one,
                run_dir=run_dir,
                provider=provider,
                base=credential["base"],
                key=credential["key"],
                model=model,
                kind=kind,
                condition=condition,
                texts=texts,
                check_script=check_script,
                max_tokens=max_tokens,
                timeout=timeout,
                retries=retries,
                force=force,
                gate=gates.get(provider),
            )
            futures[future] = (provider, model, kind, condition)
        for index, future in enumerate(as_completed(futures), start=1):
            provider, model, kind, condition = futures[future]
            try:
                result = future.result()
            except Exception as error:  # noqa: BLE001 - 单次失败不能中断整批
                result = {
                    "provider": provider,
                    "model": model,
                    "kind": kind,
                    "condition": condition,
                    "status": "unexpected-error",
                    "file": "",
                    "raw_file": "",
                    "errors": 0,
                    "warnings": 0,
                    "modes": [],
                    "mode_evidence": [],
                    "messages": [],
                    "error": f"{type(error).__name__}: {error}",
                }
            results.append(result)
            print(
                f"[{index}/{len(futures)}] {provider} {model} {kind} {condition} "
                f"→ {result['status']} E{result['errors']} W{result['warnings']} "
                f"{','.join(result['modes'])}",
                flush=True,
            )
    return results


# ---------------------------------------------------------------------------
# 汇总与报告
# ---------------------------------------------------------------------------


def aggregate(results: list[dict]) -> dict:
    ok = [result for result in results if result["status"] == "ok"]
    passed = [result for result in ok if result["errors"] == 0]
    warn_only = [result for result in ok if result["errors"] == 0 and result["warnings"] > 0]

    def group(key: str) -> dict:
        buckets: dict[str, dict] = {}
        for result in results:
            name = str(result.get(key) or "?")
            bucket = buckets.setdefault(name, {"total": 0, "ok": 0, "passed": 0, "errors": 0, "warnings": 0})
            bucket["total"] += 1
            if result["status"] == "ok":
                bucket["ok"] += 1
                bucket["errors"] += result["errors"]
                bucket["warnings"] += result["warnings"]
                if result["errors"] == 0:
                    bucket["passed"] += 1
        for bucket in buckets.values():
            bucket["pass_rate"] = round(bucket["passed"] / bucket["ok"], 4) if bucket["ok"] else None
        return buckets

    mode_counts: dict[str, int] = {}
    mode_by_kind: dict[str, dict[str, int]] = {}
    mode_by_model: dict[str, dict[str, int]] = {}
    mode_by_condition: dict[str, dict[str, int]] = {}
    message_counts: dict[str, int] = {}
    for result in ok:
        for mode in result["modes"]:
            mode_counts[mode] = mode_counts.get(mode, 0) + 1
            mode_by_kind.setdefault(result["kind"], {}).setdefault(mode, 0)
            mode_by_kind[result["kind"]][mode] += 1
            mode_by_model.setdefault(result["model"], {}).setdefault(mode, 0)
            mode_by_model[result["model"]][mode] += 1
            mode_by_condition.setdefault(result["condition"], {}).setdefault(mode, 0)
            mode_by_condition[result["condition"]][mode] += 1
        for message in result["messages"]:
            if message["level"] != "ERROR":
                continue
            key = re.sub(r"\d+", "N", message["message"])
            key = re.sub(r"第\s*N\s*行", "第N行", key)
            message_counts[key] = message_counts.get(key, 0) + 1

    focus_modes: dict[str, dict[str, dict[str, int]]] = {}
    level_map: dict[str, str] = {}
    for result in ok:
        for mode, level in (result.get("mode_levels") or {}).items():
            if level_map.get(mode) != "ERROR":
                level_map[mode] = level
    for focus in ("manual-numbering", "attachment-1-title", "number-space-title", "table-separator"):
        by_model_condition: dict[str, int] = {}
        by_kind: dict[str, int] = {}
        for result in ok:
            if focus not in result["modes"]:
                continue
            key = f"{result['model']} · 条件{result['condition']}"
            by_model_condition[key] = by_model_condition.get(key, 0) + 1
            by_kind[result["kind"]] = by_kind.get(result["kind"], 0) + 1
        focus_modes[focus] = {"by_model_condition": by_model_condition, "by_kind": by_kind}

    return {
        "calls": len(results),
        "ok": len(ok),
        "errors": sum(result["errors"] for result in ok),
        "warnings": sum(result["warnings"] for result in ok),
        "api_errors": sum(1 for result in results if result["status"] in {"api-error", "unexpected-error"}),
        "empty_output": sum(1 for result in results if result["status"] == "empty-output"),
        "missing_key": sum(1 for result in results if result["status"] == "missing-key"),
        "check_errors": sum(1 for result in results if result["status"] == "check-error"),
        "passed": len(passed),
        "pass_rate": round(len(passed) / len(ok), 4) if ok else None,
        "warn_only": len(warn_only),
        "by_kind": group("kind"),
        "by_model": group("model"),
        "by_provider": group("provider"),
        "by_condition": group("condition"),
        "failure_modes": {mode: mode_counts.get(mode, 0) for mode in MODE_ORDER if mode_counts.get(mode)},
        "failure_modes_by_kind": mode_by_kind,
        "failure_modes_by_model": mode_by_model,
        "failure_modes_by_condition": mode_by_condition,
        "focus_modes": focus_modes,
        "mode_levels": level_map,
        "error_messages": dict(sorted(message_counts.items(), key=lambda item: -item[1])[:60]),
    }


def write_summary(
    run_dir: Path,
    run_id: str,
    created: str,
    results: list[dict],
    aggregates: dict,
    run_meta: dict | None = None,
) -> None:
    summary = {
        "run": run_id,
        "created": created,
        "python": sys.version.split()[0],
        **(run_meta or {}),
        "matrix": {
            "providers": PROVIDERS,
            "models": MODEL_MATRIX,
        },
        "aggregates": aggregates,
        "tasks": results,
    }
    (run_dir / "summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2), encoding="utf-8"
    )

    columns = [
        "provider",
        "model",
        "kind",
        "condition",
        "status",
        "errors",
        "warnings",
        "modes",
        "prompt_tokens",
        "completion_tokens",
        "reasoning_tokens",
        "total_tokens",
        "max_tokens_used",
        "latency_ms",
        "check_exit_code",
        "file",
        "raw_file",
        "error",
    ]
    lines = ["\t".join(columns)]
    for result in sorted(
        results,
        key=lambda item: (item["kind"], item["condition"], item["provider"], item["model"]),
    ):
        row = []
        for column in columns:
            value = result.get(column, "")
            if column == "modes":
                value = ",".join(value or [])
            elif isinstance(value, list):
                value = json.dumps(value, ensure_ascii=False)
            row.append(str(value if value is not None else "").replace("\t", " ").replace("\n", " "))
        lines.append("\t".join(row))
    (run_dir / "summary.tsv").write_text("\n".join(lines) + "\n", encoding="utf-8")


def _cell(value: object) -> str:
    return str(value).replace("|", "\\|").replace("\n", " ")


def write_report(
    run_dir: Path,
    run_id: str,
    created: str,
    results: list[dict],
    aggregates: dict,
    run_meta: dict | None = None,
) -> None:
    run_meta = run_meta or {}
    lines: list[str] = []
    lines.append(f"# 公文助手技能包模型评测 · {run_id}")
    lines.append("")
    lines.append(f"- 生成时间：{created}")
    lines.append(f"- 技能树：`{run_meta.get('skill_dir', '?')}`")
    lines.append(f"- 技能树 sha256：`{run_meta.get('skill_tree_sha256', '?')}`")
    lines.append(f"- 判定脚本：`{run_meta.get('check_script', '?')}`"
                 f"（sha256 `{str(run_meta.get('check_script_sha256', '?'))[:16]}`）")
    lines.append(f"- 调用：{aggregates['calls']} 次，成功 {aggregates['ok']} 次，"
                 f"接口失败 {aggregates['api_errors']} 次，空输出 {aggregates['empty_output']} 次，"
                 f"check 脚本异常 {aggregates['check_errors']} 次")
    lines.append(f"- 通过率（0 ERROR）：{aggregates['passed']}/{aggregates['ok']}"
                 f" = {aggregates['pass_rate']}")
    lines.append(f"- 仅有 WARN、无 ERROR：{aggregates['warn_only']} 次")
    lines.append("")

    def pass_table(title: str, key: str) -> None:
        lines.append(f"## {title}")
        lines.append("")
        lines.append("| 分组 | 调用 | 成功 | 通过 | 通过率 | ERROR | WARN |")
        lines.append("| --- | ---: | ---: | ---: | ---: | ---: | ---: |")
        for name, bucket in sorted(aggregates[key].items()):
            lines.append(
                f"| {_cell(name)} | {bucket['total']} | {bucket['ok']} | {bucket['passed']} | "
                f"{bucket['pass_rate']} | {bucket['errors']} | {bucket['warnings']} |"
            )
        lines.append("")

    pass_table("按文种", "by_kind")
    pass_table("按模型", "by_model")
    pass_table("按条件", "by_condition")

    lines.append("## 失败模式计数（按文种）")
    lines.append("")
    kinds = sorted(aggregates["failure_modes_by_kind"])
    lines.append("| 失败模式 | " + " | ".join(kinds) + " | 合计 |")
    lines.append("| --- | " + " | ".join("---:" for _ in kinds) + " | ---: |")
    for mode in MODE_ORDER:
        counts = [aggregates["failure_modes_by_kind"].get(kind, {}).get(mode, 0) for kind in kinds]
        total = aggregates["failure_modes"].get(mode, 0)
        if not total:
            continue
        lines.append(f"| {MODE_LABELS[mode]} | " + " | ".join(str(count) for count in counts) + f" | {total} |")
    lines.append("")

    lines.append("## 失败模式 × 模型（条件 A/B 合并）")
    lines.append("")
    models = sorted(aggregates["failure_modes_by_model"])
    lines.append("| 失败模式 | " + " | ".join(_cell(model) for model in models) + " |")
    lines.append("| --- | " + " | ".join("---:" for _ in models) + " |")
    for mode in MODE_ORDER:
        counts = [aggregates["failure_modes_by_model"].get(model, {}).get(mode, 0) for model in models]
        if not any(counts):
            continue
        lines.append(f"| {MODE_LABELS[mode]} | " + " | ".join(str(count) for count in counts) + " |")
    lines.append("")

    lines.append("## 失败模式 × 条件")
    lines.append("")
    conditions = sorted(aggregates["failure_modes_by_condition"])
    lines.append("| 失败模式 | " + " | ".join(conditions) + " |")
    lines.append("| --- | " + " | ".join("---:" for _ in conditions) + " |")
    for mode in MODE_ORDER:
        counts = [aggregates["failure_modes_by_condition"].get(condition, {}).get(mode, 0) for condition in conditions]
        if not any(counts):
            continue
        lines.append(f"| {MODE_LABELS[mode]} | " + " | ".join(str(count) for count in counts) + " |")
    lines.append("")

    lines.append("## 重点模式下钻（模型 × 条件）")
    lines.append("")
    lines.append(
        "- 级别（check 脚本口径）：标题手写编号 = "
        f"{aggregates.get('mode_levels', {}).get('manual-numbering', 'ERROR')}；"
        "附件标题写“附件1” = "
        f"{aggregates.get('mode_levels', {}).get('attachment-1-title', 'WARN')}"
        "（该条 check 脚本只给 WARN，不计入 ERROR 通过率）"
    )
    lines.append("")
    focus_modes = aggregates.get("focus_modes", {})
    focus_order = [mode for mode in ("manual-numbering", "number-space-title",
                                     "attachment-1-title", "table-separator") if mode in focus_modes]
    keys = sorted(
        {key for mode in focus_order for key in focus_modes[mode].get("by_model_condition", {})}
    )
    if keys:
        lines.append("| 模型 · 条件 | " + " | ".join(MODE_LABELS[mode] for mode in focus_order) + " |")
        lines.append("| --- | " + " | ".join("---:" for _ in focus_order) + " |")
        for key in keys:
            cells = [
                str(focus_modes[mode].get("by_model_condition", {}).get(key, 0)) for mode in focus_order
            ]
            lines.append(f"| {_cell(key)} | " + " | ".join(cells) + " |")
        lines.append("")
        lines.append("| 文种 | " + " | ".join(MODE_LABELS[mode] for mode in focus_order) + " |")
        lines.append("| --- | " + " | ".join("---:" for _ in focus_order) + " |")
        for kind in KINDS:
            cells = [str(focus_modes[mode].get("by_kind", {}).get(kind, 0)) for mode in focus_order]
            lines.append(f"| {kind} | " + " | ".join(cells) + " |")
        lines.append("")

    lines.append("## 最常见的 ERROR 原文")
    lines.append("")
    lines.append("| 次数 | 检查脚本 message |")
    lines.append("| ---: | --- |")
    for message, count in list(aggregates["error_messages"].items())[:30]:
        lines.append(f"| {count} | {_cell(message)} |")
    lines.append("")

    lines.append("## 失败样例（每类至多 3 条）")
    lines.append("")
    for mode in MODE_ORDER:
        samples: list[str] = []
        for result in results:
            if result["status"] != "ok" or mode not in result["modes"]:
                continue
            evidences = next(
                (item["evidence"] for item in result["mode_evidence"] if item["mode"] == mode),
                [],
            )
            for evidence in evidences[:1]:
                samples.append(
                    f"- `{result['provider']}/{result['model']}` · {result['kind']} · "
                    f"条件{result['condition']} · {result['file']}：{evidence}"
                )
            if len(samples) >= 3:
                break
        if not samples:
            continue
        lines.append(f"### {MODE_LABELS[mode]}")
        lines.extend(samples)
        lines.append("")

    lines.append("## 逐次结果")
    lines.append("")
    lines.append("| 文种 | 条件 | 模型 | 状态 | ERROR | WARN | 失败模式 | 文件 |")
    lines.append("| --- | --- | --- | --- | ---: | ---: | --- | --- |")
    for result in sorted(
        results, key=lambda item: (item["kind"], item["condition"], item["provider"], item["model"])
    ):
        modes = ", ".join(MODE_LABELS[mode] for mode in result["modes"])
        lines.append(
            f"| {result['kind']} | {result['condition']} | {_cell(result['model'])} | {result['status']} | "
            f"{result['errors']} | {result['warnings']} | {_cell(modes)} | {result['file']} |"
        )
    lines.append("")
    (run_dir / "report.md").write_text("\n".join(lines), encoding="utf-8")


def write_manifest(
    run_dir: Path,
    run_id: str,
    created: str,
    results: list[dict],
    run_meta: dict | None = None,
) -> None:
    run_meta = run_meta or {}
    cases = []
    for result in sorted(
        results, key=lambda item: (item["kind"], item["condition"], item["provider"], item["model"])
    ):
        if result["status"] != "ok":
            continue
        clean_file = run_dir / result["file"]
        if not clean_file.is_file():
            continue
        cases.append(
            {
                "file": result["file"],
                "raw_file": result["raw_file"],
                "provider": result["provider"],
                "model": result["model"],
                "kind": result["kind"],
                "condition": result["condition"],
            }
        )
    manifest = {
        "run": run_id,
        "created": created,
        "skill_dir": run_meta.get("skill_dir", ""),
        "skill_tree_sha256": run_meta.get("skill_tree_sha256", ""),
        "cases": cases,
    }
    (run_dir / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8"
    )


# ---------------------------------------------------------------------------
# 冒烟
# ---------------------------------------------------------------------------


def smoke(credentials: dict[str, dict[str, str]], timeout: float, retries: int) -> int:
    failures = 0
    print("provider\tmodel\tstatus\tlatency_ms\tcontent\tusage")
    for provider, models in MODEL_MATRIX.items():
        credential = credentials.get(provider)
        if not credential:
            for model in models:
                print(f"{provider}\t{model}\tmissing-key\t-\t-\t-")
                failures += 1
            continue
        for model in models:
            started = time.monotonic()
            try:
                body = chat_completion(
                    provider,
                    credential["base"],
                    credential["key"],
                    model,
                    [{"role": "user", "content": "只回复两个字：可用"}],
                    16,
                    timeout,
                    retries,
                )
                content, _, usage = extract_choice(body)
                prompt, completion, _ = usage_numbers(usage)
                elapsed = round((time.monotonic() - started) * 1000)
                status = "ok" if content.strip() else "empty-content"
                if not content.strip():
                    failures += 1
                print(
                    f"{provider}\t{model}\t{status}\t{elapsed}\t"
                    f"{content.strip()[:20]!r}\t{prompt}/{completion}"
                )
            except EvalError as error:
                failures += 1
                print(f"{provider}\t{model}\terror\t-\t{str(error)[:120]}\t-")
    return 1 if failures else 0


# ---------------------------------------------------------------------------
# rescore：用当前判定脚本重评已保存的清洗稿（不重新调模型、不重新清洗）
# ---------------------------------------------------------------------------


def resolve_run_dir(run_ref: str) -> Path:
    path = Path(run_ref)
    if not path.is_absolute():
        candidate = OUT_ROOT / path
        path = candidate if candidate.exists() else path
    return path


def rescore_run(run_ref: str, check_script: Path) -> dict:
    run_dir = resolve_run_dir(run_ref)
    summary_path = run_dir / "summary.json"
    if not summary_path.is_file():
        raise EvalError(f"找不到 {summary_path}")
    data = json.loads(summary_path.read_text(encoding="utf-8"))
    tasks: list[dict] = data.get("tasks", [])
    rescored = 0
    for task in tasks:
        if task.get("status") not in {"ok", "check-error", "empty-output"}:
            continue
        relative = task.get("file") or ""
        if not relative:
            continue
        clean_path = run_dir / relative
        if not clean_path.is_file():
            continue
        clean = clean_path.read_text(encoding="utf-8")
        raw_relative = task.get("raw_file") or ""
        raw_path = run_dir / raw_relative
        raw_text = raw_path.read_text(encoding="utf-8") if raw_relative and raw_path.is_file() else ""
        check = run_check(clean_path, task.get("kind", ""), 120, check_script)
        task["check_exit_code"] = check["exit_code"]
        task["script_error"] = check["script_error"]
        task["messages"] = check["messages"]
        task["errors"] = sum(1 for message in check["messages"] if message["level"] == "ERROR")
        task["warnings"] = sum(1 for message in check["messages"] if message["level"] == "WARN")
        if check["script_error"]:
            task["status"] = "check-error"
            task["error"] = check["stderr"][-500:]
            continue
        task["status"] = "ok"
        modes, levels, evidence = classify_clean(task.get("kind", ""), clean, raw_text, check["messages"])
        task["modes"] = modes
        task["mode_levels"] = levels
        task["mode_evidence"] = evidence
        rescored += 1
    aggregates = aggregate(tasks)
    run_meta = {
        "skill_dir": data.get("skill_dir", ""),
        "skill_tree_sha256": data.get("skill_tree_sha256", ""),
        "check_script": str(check_script),
        "check_script_sha256": hashlib.sha256(check_script.read_bytes()).hexdigest(),
        "rescored_at": datetime.now().isoformat(timespec="seconds"),
        "rescored_tasks": rescored,
    }
    run_id = data.get("run") or run_dir.name
    created = data.get("created", "")
    write_summary(run_dir, run_id, created, tasks, aggregates, run_meta)
    write_manifest(run_dir, run_id, created, tasks, run_meta)
    write_report(run_dir, run_id, created, tasks, aggregates, run_meta)
    return aggregates


# ---------------------------------------------------------------------------
# before / after 对比
# ---------------------------------------------------------------------------


def load_run(run_ref: str) -> dict:
    path = Path(run_ref)
    if not path.is_absolute():
        candidate = OUT_ROOT / run_ref
        path = candidate if candidate.exists() else path
    summary_path = path / "summary.json"
    if not summary_path.is_file():
        raise EvalError(f"找不到 {summary_path}")
    data = json.loads(summary_path.read_text(encoding="utf-8"))
    data["_dir"] = path
    return data


def _run_metrics(tasks: list[dict]) -> dict:
    ok = [task for task in tasks if task.get("status") == "ok"]
    passed = [task for task in ok if not task.get("errors")]
    modes: dict[str, int] = {}
    focus: dict[str, dict[str, int]] = {
        "manual-numbering": {},
        "attachment-1-title": {},
        "number-space-title": {},
        "table-separator": {},
    }
    for task in ok:
        for mode in task.get("modes") or []:
            modes[mode] = modes.get(mode, 0) + 1
        for mode in focus:
            if mode in (task.get("modes") or []):
                key = f"{task.get('model')} · 条件{task.get('condition')}"
                focus[mode][key] = focus[mode].get(key, 0) + 1

    def by(key: str) -> dict[str, dict]:
        buckets: dict[str, dict] = {}
        for task in tasks:
            name = str(task.get(key) or "?")
            bucket = buckets.setdefault(name, {"total": 0, "ok": 0, "passed": 0, "errors": 0, "warnings": 0})
            bucket["total"] += 1
            if task.get("status") == "ok":
                bucket["ok"] += 1
                bucket["passed"] += 1 if not task.get("errors") else 0
                bucket["errors"] += task.get("errors") or 0
                bucket["warnings"] += task.get("warnings") or 0
        return buckets

    return {
        "calls": len(tasks),
        "ok": len(ok),
        "api_errors": sum(1 for task in tasks if task.get("status") in {"api-error", "unexpected-error"}),
        "empty_output": sum(1 for task in tasks if task.get("status") == "empty-output"),
        "passed": len(passed),
        "pass_rate": round(len(passed) / len(ok), 4) if ok else None,
        "errors": sum(task.get("errors") or 0 for task in ok),
        "warnings": sum(task.get("warnings") or 0 for task in ok),
        "modes": modes,
        "focus": focus,
        "by_kind": by("kind"),
        "by_model": by("model"),
        "by_condition": by("condition"),
    }


def compare_runs(refs: list[str], provider: str, out_path: Path | None) -> str:
    runs = [load_run(ref) for ref in refs]
    labels = [str(run.get("run") or Path(ref).name) for run, ref in zip(runs, refs)]

    def pick(data: dict) -> list[dict]:
        tasks = data.get("tasks", [])
        if provider == "all":
            return tasks
        return [task for task in tasks if task.get("provider") == provider]

    metrics = [_run_metrics(pick(run)) for run in runs]

    lines = [f"# 技能包评测对比：{' → '.join(labels)}", ""]
    lines.append(f"- 提供商过滤：`{provider}`")
    for label, run in zip(labels, runs):
        lines.append(
            f"- {label}：技能树 `{run.get('skill_tree_sha256', '?')}`；"
            f"判定脚本 `{str(run.get('check_script_sha256', '?'))[:16]}`"
            + (f"；重评于 {run.get('rescored_at')}" if run.get("rescored_at") else "")
        )
    lines.append("")

    lines.append("## 总体")
    lines.append("")
    lines.append("| 指标 | " + " | ".join(_cell(label) for label in labels) + " | 首末变化 |")
    lines.append("| --- | " + " | ".join("---:" for _ in labels) + " | ---: |")
    for label, key in [
        ("调用", "calls"),
        ("成功", "ok"),
        ("接口失败", "api_errors"),
        ("空输出", "empty_output"),
        ("通过（0 ERROR）", "passed"),
        ("通过率", "pass_rate"),
        ("ERROR 合计", "errors"),
        ("WARN 合计", "warnings"),
    ]:
        values = [metric[key] for metric in metrics]
        lines.append(
            "| " + label + " | " + " | ".join(str(value) for value in values) + " | "
            + _delta_text(values[0], values[-1]) + " |"
        )
    lines.append("")

    def group_table(title: str, key: str) -> None:
        names = sorted({name for metric in metrics for name in metric[key]})
        if not names:
            return
        lines.append(f"## 按{title}")
        lines.append("")
        header = [f"{title}"] + [f"{label} 通过/成功（ERROR）" for label in labels]
        lines.append("| " + " | ".join(header) + " |")
        lines.append("| --- | " + " | ".join("---" for _ in labels) + " |")
        for name in names:
            cells = []
            for metric in metrics:
                bucket = metric[key].get(name, {"ok": 0, "passed": 0, "errors": 0})
                cells.append(f"{bucket['passed']}/{bucket['ok']}（{bucket['errors']}）")
            lines.append(f"| {_cell(name)} | " + " | ".join(cells) + " |")
        lines.append("")

    group_table("文种", "by_kind")
    group_table("模型", "by_model")
    group_table("条件", "by_condition")

    focus_modes = ("manual-numbering", "attachment-1-title", "number-space-title", "table-separator")
    lines.append("## 失败模式")
    lines.append("")
    lines.append("| 失败模式 | " + " | ".join(_cell(label) for label in labels) + " |")
    lines.append("| --- | " + " | ".join("---:" for _ in labels) + " |")
    for mode in MODE_ORDER:
        counts = [metric["modes"].get(mode, 0) for metric in metrics]
        if not any(counts):
            continue
        lines.append(f"| {MODE_LABELS[mode]} | " + " | ".join(str(count) for count in counts) + " |")
    lines.append("")

    lines.append("## 重点模式：模型 × 条件")
    lines.append("")
    keys = sorted({key for metric in metrics for mode in focus_modes for key in metric["focus"].get(mode, {})})
    lines.append("| 模型 · 条件 | " + " | ".join(MODE_LABELS[mode] for mode in focus_modes) + " |")
    lines.append("| --- | " + " | ".join("---" for _ in focus_modes) + " |")
    for key in keys:
        cells = []
        for mode in focus_modes:
            values = [metric["focus"].get(mode, {}).get(key, 0) for metric in metrics]
            cells.append(" → ".join(str(value) for value in values))
        lines.append(f"| {_cell(key)} | " + " | ".join(cells) + " |")
    lines.append("")

    text = "\n".join(lines)
    if out_path is not None:
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(text, encoding="utf-8")
    print(text)
    if out_path is not None:
        print(f"\n对比报告：{out_path}")
    return text


def _delta_text(first: object, last: object) -> str:
    if first is None or last is None:
        return "-"
    if isinstance(first, int) and isinstance(last, int):
        return f"{last - first:+d}"
    first_float, last_float = float(first), float(last)
    if first_float <= 1:
        return f"{(last_float - first_float) * 100:+.1f} 个百分点"
    return f"{last_float - first_float:+.2f}"


# ---------------------------------------------------------------------------
# 入口
# ---------------------------------------------------------------------------


def build_tasks(
    matrix: str,
    only_models: list[str],
    only_kinds: list[str],
    conditions: list[str],
) -> list[tuple[str, str, str, str]]:
    if matrix == "baseline":
        matrix_models = BASELINE_MATRIX
        matrix_conditions = conditions
    elif matrix == "cross":
        matrix_models = CROSS_MATRIX
        matrix_conditions = conditions
    elif matrix == "all":
        matrix_models = {
            provider: sorted(set(BASELINE_MATRIX.get(provider, [])) | set(CROSS_MATRIX.get(provider, [])))
            for provider in PROVIDERS
        }
        matrix_conditions = conditions
    else:
        raise SystemExit(f"不认识的 matrix：{matrix}")
    kinds = [kind for kind in KINDS if not only_kinds or kind in only_kinds]
    tasks: list[tuple[str, str, str, str]] = []
    for provider, models in matrix_models.items():
        for model in models:
            if only_models and not any(token in model or token == provider for token in only_models):
                continue
            for kind in kinds:
                for condition in matrix_conditions:
                    tasks.append((provider, model, kind, condition))
    return tasks


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="公文助手技能包模型评测")
    subparsers = parser.add_subparsers(dest="command", required=True)

    def add_common(subparser: argparse.ArgumentParser) -> None:
        subparser.add_argument("--keys-file", default=str(DEFAULT_KEYS_FILE), help="密钥文件路径（gitignored）")
        subparser.add_argument("--timeout", type=float, default=300.0, help="单次 HTTP 超时秒数")
        subparser.add_argument("--retries", type=int, default=3, help="网络/限流重试次数")

    smoke_parser = subparsers.add_parser("smoke", help="每个模型做一次极小请求，确认模型名可用")
    add_common(smoke_parser)

    compare_parser = subparsers.add_parser("compare", help="对比两个或多个 run 的汇总（before/after/after3）")
    compare_parser.add_argument("runs", nargs="+", help="两个或更多 run-id 或目录，按时间顺序排")
    compare_parser.add_argument("--provider", default="qny", help="只比较该提供商；all 表示全部")
    compare_parser.add_argument("--out", default="", help="对比报告输出路径")

    rescore_parser = subparsers.add_parser(
        "rescore", help="用当前 check 脚本重评已保存的清洗稿（不重新调模型）"
    )
    rescore_parser.add_argument("runs", nargs="*", help="run-id 或目录；配合 --all 时可不填")
    rescore_parser.add_argument("--all", action="store_true", help="重评 tmp/gongwen-skill-eval 下所有 run")
    rescore_parser.add_argument("--check-script", default=str(CHECK_SCRIPT), help="判定脚本路径")
    rescore_parser.add_argument("--skip-selftest", action="store_true")

    run_parser = subparsers.add_parser("run", help="跑评测矩阵")
    add_common(run_parser)
    run_parser.add_argument("--matrix", choices=["baseline", "cross", "all"], default="baseline")
    run_parser.add_argument("--run-id", default="", help="产物子目录名；默认 baseline-<时间戳>")
    run_parser.add_argument("--max-tokens", type=int, default=32768)
    run_parser.add_argument("--workers", type=int, default=4)
    run_parser.add_argument("--conditions", default="A,B")
    run_parser.add_argument("--only-model", action="append", default=[], help="只跑匹配的模型（可重复）")
    run_parser.add_argument("--only-kind", action="append", default=[], help="只跑指定文种（可重复）")
    run_parser.add_argument("--force", action="store_true", help="覆盖已有清洗产物")
    run_parser.add_argument("--no-prompts", action="store_true", help="不写 prompts/ 目录")
    run_parser.add_argument(
        "--skill-dir",
        default="skills/gongwen-markdown",
        help="被测技能树目录（before/after 各冻结一份，run 内固定；相对路径按仓库根解析）",
    )
    run_parser.add_argument(
        "--check-script",
        default=str(CHECK_SCRIPT),
        help="判定用自检脚本（仪器，默认工作区版本，不随 --skill-dir 变）",
    )
    run_parser.add_argument("--skip-selftest", action="store_true", help="跳过 check 脚本的 --selftest")

    args = parser.parse_args(argv)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")

    if args.command == "compare":
        default_name = "compare-" + "-".join(Path(ref).name for ref in args.runs)
        out = Path(args.out) if args.out else OUT_ROOT / f"{default_name}.md"
        if not out.is_absolute():
            out = (ROOT / out).resolve()
        try:
            compare_runs(args.runs, args.provider, out)
        except EvalError as error:
            print(str(error), file=sys.stderr)
            return 2
        return 0

    if args.command == "rescore":
        check_script = Path(args.check_script)
        if not check_script.is_absolute():
            check_script = (ROOT / check_script).resolve()
        if not check_script.is_file():
            print(f"判定脚本不存在：{check_script}", file=sys.stderr)
            return 2
        if not args.skip_selftest:
            completed = subprocess.run(
                [sys.executable, str(check_script), "--selftest"],
                cwd=str(ROOT),
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            verdict = (completed.stdout + completed.stderr).strip().splitlines()
            print(f"check 脚本自检：{verdict[-1] if verdict else '无输出'}（exit={completed.returncode}）")
            if completed.returncode != 0:
                print("自检未通过，停止重评。", file=sys.stderr)
                return 3
        refs = list(args.runs)
        if args.all:
            refs = sorted(
                directory.name
                for directory in OUT_ROOT.iterdir()
                if directory.is_dir()
                and (directory / "summary.json").is_file()
                and not directory.name.startswith("skill-")
            )
        if not refs:
            print("没有指定要重评的 run。", file=sys.stderr)
            return 2
        print(f"判定脚本 sha256={hashlib.sha256(check_script.read_bytes()).hexdigest()}")
        for ref in refs:
            try:
                aggregates = rescore_run(ref, check_script)
            except EvalError as error:
                print(f"{ref}: {error}", file=sys.stderr)
                continue
            print(
                f"{ref}: 成功 {aggregates['ok']}/{aggregates['calls']}，"
                f"通过 {aggregates['passed']}（{aggregates['pass_rate']}），"
                f"ERROR {aggregates['errors']}，WARN {aggregates['warnings']}"
            )
        return 0

    keys_file = Path(args.keys_file)
    credentials = load_keys(keys_file if keys_file.is_file() else None)
    if not credentials:
        print(
            f"没有读到任何提供商的密钥：请检查 {keys_file} 或环境变量 "
            "QNY_API_KEY / DMXAPI_API_KEY / SILICONFLOW_API_KEY。"
            "本脚本不会把密钥写进仓库文件。",
            file=sys.stderr,
        )
        return 2
    available = ", ".join(
        f"{provider}={credential['base']}（密钥 {len(credential['key'])} 字符）"
        for provider, credential in credentials.items()
    )
    print(f"已读取提供商：{available}")

    if args.command == "smoke":
        return smoke(credentials, args.timeout, args.retries)

    conditions = [condition.strip().upper() for condition in args.conditions.split(",") if condition.strip()]
    conditions = [condition for condition in conditions if condition in {"A", "B"}]
    if not conditions:
        print("条件只能是 A/B", file=sys.stderr)
        return 2
    tasks = build_tasks(args.matrix, args.only_model, args.only_kind, conditions)
    if not tasks:
        print("没有匹配的任务", file=sys.stderr)
        return 2

    run_id = args.run_id or f"{args.matrix}-{datetime.now().strftime('%Y%m%d-%H%M%S')}"
    run_dir = OUT_ROOT / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    created = datetime.now().isoformat(timespec="seconds")

    skill_dir = Path(args.skill_dir)
    if not skill_dir.is_absolute():
        skill_dir = (ROOT / skill_dir).resolve()
    check_script = Path(args.check_script)
    if not check_script.is_absolute():
        check_script = (ROOT / check_script).resolve()
    if not (skill_dir / "SKILL.md").is_file():
        print(f"技能目录不存在或缺少 SKILL.md：{skill_dir}", file=sys.stderr)
        return 2
    if not check_script.is_file():
        print(f"判定脚本不存在：{check_script}", file=sys.stderr)
        return 2

    # 判定口径是仪器：评分前先确认自检脚本自身是好的。
    if not args.skip_selftest:
        completed = subprocess.run(
            [sys.executable, str(check_script), "--selftest"],
            cwd=str(ROOT),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        verdict = (completed.stdout + completed.stderr).strip().splitlines()
        print(f"check 脚本自检：{verdict[-1] if verdict else '无输出'}（exit={completed.returncode}）")
        if completed.returncode != 0:
            print("自检脚本未通过 --selftest，停止评分。", file=sys.stderr)
            return 3

    snapshot = load_skill_snapshot(skill_dir)
    texts = snapshot["texts"]
    skill_tree = snapshot["metadata"]["skill_tree_sha256"]
    print(f"技能树：{skill_dir}\n        sha256={skill_tree}")
    print(f"运行 {run_id}：{len(tasks)} 次调用，{args.workers} 并发 → {run_dir}")
    write_skill_snapshot(run_dir, snapshot)
    if not args.no_prompts:
        write_prompts(run_dir, texts)

    results = run_results(
        run_dir,
        credentials,
        tasks,
        texts=texts,
        check_script=check_script,
        max_tokens=args.max_tokens,
        timeout=args.timeout,
        retries=args.retries,
        workers=args.workers,
        force=args.force,
    )
    aggregates = aggregate(results)
    run_meta = {
        "skill_dir": str(skill_dir),
        "skill_tree_sha256": skill_tree,
        "check_script": str(check_script),
        "check_script_sha256": hashlib.sha256(check_script.read_bytes()).hexdigest(),
    }
    write_summary(run_dir, run_id, created, results, aggregates, run_meta)
    write_manifest(run_dir, run_id, created, results, run_meta)
    write_report(run_dir, run_id, created, results, aggregates, run_meta)
    print(
        f"完成：成功 {aggregates['ok']}/{aggregates['calls']}，"
        f"通过 {aggregates['passed']}（通过率 {aggregates['pass_rate']}）。"
    )
    print(f"汇总：{run_dir / 'summary.tsv'}")
    print(f"报告：{run_dir / 'report.md'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
