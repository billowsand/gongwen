//! 研究报告 Typst 样张：封面、目录、部分、文框、长表、公式、文献各排一遍，PDF 与模板
//! 数据落到 `tmp/typst-samples/research-<用例>/`，改模板后目视检查用。依赖本机随包字体，
//! 默认忽略：
//!
//! ```text
//! cargo test --locked typst_samples_research -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

use crate::models::{DraftInput, NumberingConfig, TemplateKind};

fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tmp")
        .join("typst-samples")
}

/// 插图：一张横图、一张竖图，写进 `base/images/`。
fn write_images(base: &Path) {
    let dir = base.join("images");
    std::fs::create_dir_all(&dir).unwrap();
    let mut wide = image::RgbImage::from_pixel(1600, 900, image::Rgb([255, 255, 255]));
    for x in 0..1600 {
        for t in 0..6 {
            wide.put_pixel(x, 40 + t, image::Rgb([0, 0, 0]));
            wide.put_pixel(x, 854 + t, image::Rgb([0, 0, 0]));
        }
    }
    wide.save(dir.join("arch.png")).unwrap();
    let tall = image::RgbImage::from_pixel(600, 900, image::Rgb([220, 235, 255]));
    tall.save(dir.join("tall.png")).unwrap();
}

const BIB: &str = r#"@article{wang2020,
  title = {公共数据授权运营的制度逻辑},
  author = {王小明 and 李华},
  journal = {电子政务},
  year = {2020},
  number = {5},
  pages = {12--20}
}
@book{oecd2019,
  title = {Enhancing Access to and Sharing of Data},
  author = {{OECD}},
  publisher = {OECD Publishing},
  address = {Paris},
  year = {2019}
}
@misc{gov2022,
  title = {关于构建数据基础制度更好发挥数据要素作用的意见},
  author = {{中共中央} and {国务院}},
  year = {2022},
  url = {https://www.gov.cn/zhengce/2022-12/19/content_5732695.htm}
}
"#;

const FULL: &str = r#"# 公共数据授权运营机制研究

<!-- [摘要] -->

本报告梳理了公共数据授权运营的政策脉络与地方实践，**提出以场景牵引、分级授权、收益共享为核心的运营机制**。研究采用文献分析与案例比较相结合的方法。

关键词：公共数据；授权运营；数据要素

<!-- [目录] -->

<!-- [正文] -->

## 研究背景 {#chap:bg}

数据作为新型生产要素，已快速融入生产、分配、流通、消费和社会服务管理等各环节[@wang2020]。2022年发布的“数据二十条”明确提出推进公共数据确权授权机制建设[@gov2022; @oecd2019]，各地随即开展了形式多样的探索。王小明等@wang2020从制度逻辑切入，文献@oecd2019则侧重国际比较，三者合观[@oecd2019; @wang2020; @gov2022]；运营方账号 @admin 不是文献引用，原样印出。

### 政策脉络 {#sec:policy}

国家层面的政策演进大体可以分为三个阶段[^n1]:(阶段划分参考了国家数据局2024年发布的政策汇编。)，见表{@tbl:stage}。本节使用 *斜体强调* 与 `code` 两种行内格式，并给出一个链接 [国家数据局](https://www.nda.gov.cn)。

表：公共数据政策演进的三个阶段 {#tbl:stage}

| 阶段 | 时间 | 标志性文件 | 主要特征 |
| --- | --- | --- | --- |
| 起步期 | 2015—2019年 | 促进大数据发展行动纲要 | 强调开放共享，以政府数据开放平台建设为主 |
| 探索期 | 2020—2022年 | 关于构建更加完善的要素市场化配置体制机制的意见 | 提出培育数据要素市场 |
| 深化期 | 2023年至今 | 关于加快公共数据资源开发利用的意见 | 明确授权运营的制度框架 |

#### 地方立法

截至2025年底，已有十余个省市出台公共数据管理条例。

##### 条例要点

条例普遍规定了授权运营的主体、程序与收益分配方式。

### 研究方法

本研究综合运用以下方法：

1. 文献分析法，梳理国内外相关研究；
2. 案例比较法，选取三个典型城市：
    1. 甲市，以场景牵引为特色；
    2. 乙市，以平台运营为特色；
3. 专家访谈法，访谈了12位专家。

## 现状分析 {#chap:status}

### 总体架构

公共数据授权运营的总体架构如图{@fig:arch}所示。

![公共数据授权运营总体架构](images/arch.png){#fig:arch}

运营主体的收益模型可以写成 $R = \sum_{i=1}^{n} p_i q_i - C$，其中 $p_i$ 为单价、$q_i$ 为调用量。独立公式如下：

$$\int_0^1 x^2\,dx = \frac{1}{3}$$

> 坚持统筹发展和安全，坚持有效市场和有为政府相结合，推动公共数据有条件无偿使用。
>
> ——《关于加快公共数据资源开发利用的意见》

> [!案例] 某市“场景牵引”授权运营做法 {#case:city}
>
> 一是先定场景、后给数据，按场景逐项审批。
>
> 二是运营机构分场景准入[^n2]:(准入条件由市数据局另行制定。)：
> - 具备相应的技术能力；
> - 建立数据安全管理制度。

如案例{@case:city}所示，场景牵引能有效控制数据流转范围。

<!-- [序号表] -->
| 序号 | 事项 | 责任单位 |
| :---: | --- | --- |
| 制度建设 |  |  |
|  | 出台授权运营管理办法 | 市数据局 |
|  | 制定收益分配细则 | 市财政局 |
| 平台建设 |  |  |
|  | 建设授权运营平台 | 市大数据中心 |

<!-- [居中] -->
**表外居中的一行说明**

<!-- [居右] -->
数据来源：课题组整理

```python
def revenue(prices, volumes, cost):
    return sum(p * q for p, q in zip(prices, volumes)) - cost
```

## 对策建议

### 完善制度体系

建议加快出台公共数据授权运营管理办法，明确授权条件、运营规范和收益分配机制，如第{@chap:bg}章与{@sec:policy}节所述。

![竖图示例](images/tall.png)

<!-- [附录] -->

## 调研城市基本情况

### 甲市

甲市常住人口约800万人。

表：调研城市主要指标

| 城市 | 人口（万人） | GDP（亿元） |
| --- | ---: | ---: |
| 甲市 | 800 | 9000 |
| 乙市 | 650 | 7200 |

<!-- [版本变更记录] -->

## 版本变更记录

| 版本 | 日期 | 说明 |
| --- | --- | --- |
| V1.0 | 2026年3月 | 初稿 |
| V1.1 | 2026年5月 | 补充案例 |

<!-- [参考文献] -->

## 参考文献
"#;

/// 部分、不编号章、文框编号、长表跨页、项目类封面。
const PARTS: &str = r#"<!-- [不编号] -->
## 前言

本报告分为两个部分。不编号章里的表格用流水号。

表：前言里的表

| 项目 | 说明 |
| --- | --- |
| 甲 | 乙 |

<!-- [部分] -->

# 现状分析 {#part:xz}

## 研究背景

正文段落，章号跨部分连续。见第{@part:xz}部分。

> [!专栏] 新加坡的数据治理经验 {#box:sg}
>
> 新加坡设立了个人数据保护委员会。

> [!专栏] 欧盟的经验

> [!例子]
>
> 一个没有标题的例子。

# 对策建议

## 总体思路

### 基本原则

表：长表跨页 {#tbl:long}

| 序号 | 任务 | 责任单位 | 完成时限 |
| --- | --- | --- | --- |
| 1 | 建立公共数据目录体系并实行动态更新管理 | 市数据局 | 2026年6月 |
| 2 | 建设统一的授权运营平台 | 市大数据中心 | 2026年9月 |
| 3 | 制定授权运营管理办法 | 市数据局 | 2026年3月 |
| 4 | 开展数据资产登记试点 | 市财政局 | 2026年12月 |
| 5 | 建立收益分配机制 | 市财政局 | 2027年3月 |
| 6 | 组织开展场景征集 | 各区政府 | 2026年4月 |
| 7 | 培育数据运营服务商 | 市工信局 | 2026年10月 |
| 8 | 开展安全评估 | 市网信办 | 2026年8月 |
| 9 | 建立监督考核机制 | 市政府办公厅 | 2026年12月 |
| 10 | 编制年度报告 | 市数据局 | 2027年1月 |
| 11 | 建立公共数据目录体系并实行动态更新管理 | 市数据局 | 2026年6月 |
| 12 | 建设统一的授权运营平台 | 市大数据中心 | 2026年9月 |
| 13 | 制定授权运营管理办法 | 市数据局 | 2026年3月 |
| 14 | 开展数据资产登记试点 | 市财政局 | 2026年12月 |
| 15 | 建立收益分配机制 | 市财政局 | 2027年3月 |
| 16 | 组织开展场景征集 | 各区政府 | 2026年4月 |
| 17 | 培育数据运营服务商 | 市工信局 | 2026年10月 |
| 18 | 开展安全评估 | 市网信办 | 2026年8月 |
| 19 | 建立监督考核机制 | 市政府办公厅 | 2026年12月 |
| 20 | 编制年度报告 | 市数据局 | 2027年1月 |
| 21 | 建立公共数据目录体系并实行动态更新管理 | 市数据局 | 2026年6月 |
| 22 | 建设统一的授权运营平台 | 市大数据中心 | 2026年9月 |
| 23 | 制定授权运营管理办法 | 市数据局 | 2026年3月 |
| 24 | 开展数据资产登记试点 | 市财政局 | 2026年12月 |

见表{@tbl:long}与专栏{@box:sg}。

<!-- [不编号] -->
## 结束语

结束语正文。

表：结束语里的表

| 项目 | 说明 |
| --- | --- |
| 丙 | 丁 |

<!-- [附录] -->

## 附录标题

> [!专栏] 附录里的专栏

附录正文。
"#;

/// 公式：行内、独立、矩阵、cases、对齐、标题与单元格里的公式。
const MATH: &str = r#"<!-- [正文] -->

## 模型设定与$\alpha$系数

设效用函数为 $U(x) = x^{\alpha}$，其中 $0 < \alpha \leqslant 1$。分段定义如下：

$$f(x) = \begin{cases} x^2, & x \geq 0 \\ -x, & x < 0 \end{cases}$$

矩阵形式：

$$\mathbf{A} = \begin{pmatrix} a_{11} & a_{12} \\ a_{21} & a_{22} \end{pmatrix}, \quad \mathbb{R}^n$$

$$\begin{aligned} y &= ax + b \\ z &= \frac{\partial y}{\partial x} \end{aligned}$$

中文条件与上下标、分数里的中文：

$$\sqrt[n]{a^n} = |a| \quad (n\ \text{为偶数})$$

行内公式 $x_{\text{中文}}$，分式 $\frac{\text{分子}}{\text{分母}}$。

| 符号 | 含义 |
| --- | --- |
| $\beta$ | 弹性系数 |
| $\sum_{i} w_i$ | 权重和 |

<!-- [居中] -->
$$E = mc^2$$

> 引文里的公式：
> $$\lim_{n\to\infty} \left(1+\frac{1}{n}\right)^n = e$$
"#;

struct Case {
    name: &'static str,
    markdown: &'static str,
    tweak: fn(&mut DraftInput),
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "research-full",
            markdown: FULL,
            tweak: |input| {
                input.research.security = "内部".into();
                input.research.file_type = "专题研究报告".into();
                input.research.file_number = "ZT-2026-07".into();
                input.research.version = "征求意见稿".into();
                input.research.byline = "公共数据课题组 编写".into();
                input.research.original_title = "Research on Public Data Authorization".into();
                input.research.bibliography_content = BIB.into();
            },
        },
        Case {
            name: "research-parts",
            markdown: PARTS,
            tweak: |input| {
                input.title_hint = "全市一体化政务数据共享平台（二期）建设项目实施方案".into();
                input.research.security = "秘密".into();
                input.research.security_years = "5年".into();
                input.research.file_type = "建设实施方案".into();
                input.research.ident = "项目编号：XM-2026-014".into();
            },
        },
        Case {
            name: "research-math",
            markdown: MATH,
            tweak: |input| {
                input.title_hint = "公式排版测试".into();
            },
        },
        Case {
            name: "research-structure",
            markdown: "# 公共数据授权运营机制研究\n\n<!-- [目录] -->\n\n<!-- [部分] -->\n\n# 公共数据授权运营的现状与主要问题\n\n## 公共数据开发利用政策演进与授权运营制度建设的研究背景\n\n本章梳理政策演进与运营现状。\n\n### 政策脉络\n\n研究采用文献分析与案例比较。\n\n## 地方授权运营实践与主要问题\n\n### 发展现状\n\n正文。\n\n### 主要问题\n\n正文。\n\n# 实施路径与保障机制\n\n## 授权运营收益模型 $R=pq$ 与实施路径\n\n### 总体思路\n\n正文。\n\n<!-- [附录] -->\n\n## 调研说明\n\n附录内容。\n",
            tweak: |input| {
                input.research.file_number = "ZT-2026-07".into();
                input.research.file_type = "专题研究报告".into();
            },
        },
        Case {
            name: "research-details",
            markdown: "# 研究终端细节样张\n\n## 研究方法\n\n### 数据来源\n\n正文。\n\n#### 样本选取\n\n正文。\n\n##### 分类依据\n\n正文。\n\n> 授权运营应坚持统筹发展和安全。公式 $R=pq$。\n>\n> ——《研究材料》\n\n> [!引理] 有效样本 {#lem:sample}\n>\n> 按预设分层条件选取样本。\n\n> [!推论] 运营机制\n>\n> 结论由有效样本分析得到。\n> 1. 第一项。\n> 2. 第二项。\n> $$R=pq$$\n\n> [!专栏] 具有较长标题的研究案例说明以及标题中的公式 $R=pq$ 用于检验自动换行与类型签牌是否重叠\n>\n> 长标题正文。\n\n正文引用引理{@lem:sample}。\n",
            tweak: |input| {
                input.research.file_type = "专题研究报告".into();
            },
        },
        Case {
            name: "research-body-flow",
            markdown: "# 段落与列表转换验收\n\n<!-- [摘要] -->\n\n摘要第一行\n接在同一段。\n\n<!-- [正文] -->\n\n## 转换规则\n\n短句前半\n接上后半。\n\nEnglish\nwords stay together.\n\n要求如下：\n1. **段内甲**；\n2. 段内乙含公式 $x=1$。\n\n3. 独立列表甲\n1. 独立列表乙\n\n后续段落第一行\n接在同一段。\n\n### 特殊结构\n\n```text\n代码第一行\n代码第二行\n```\n\n$$\nx + y\n= z\n$$\n\n<!-- [居中] -->\n居中第一行\n居中第二行\n\n表：边界检查 {#tbl:boundary}\n\n| 项目 | 结果 |\n| --- | --- |\n| 段内列表 | 同段 |\n| 独立列表 | 分段 |\n\n表{@tbl:boundary}保留交叉引用。\n",
            tweak: |input| {
                input.title_hint = "段落与列表转换验收".into();
            },
        },
    ]
}

fn base_input() -> DraftInput {
    let mut input = DraftInput {
        kind: TemplateKind::ResearchReport,
        ..Default::default()
    };
    input.research.institution = "某某市数据研究中心".into();
    input.research.date = "2026年9月".into();
    input
}

fn render_pages(bytes: Vec<u8>, dir: &Path) {
    use hayro::{
        RenderCache, RenderSettings,
        hayro_interpret::{InterpreterSettings, hayro_syntax::Pdf},
    };
    let pdf = Pdf::new(bytes).unwrap();
    let cache = RenderCache::new();
    for (i, page) in pdf.pages().iter().enumerate() {
        let scale = 1000.0 / page.render_dimensions().0;
        let pixmap = hayro::render(
            page,
            &cache,
            &InterpreterSettings::default(),
            &RenderSettings {
                x_scale: scale,
                y_scale: scale,
                width: Some(1000),
                ..Default::default()
            },
        );
        image::RgbaImage::from_raw(
            u32::from(pixmap.width()),
            u32::from(pixmap.height()),
            pixmap.data_as_u8_slice().to_vec(),
        )
        .unwrap()
        .save(dir.join(format!("page-{}.png", i + 1)))
        .unwrap();
    }
}

fn run() {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let numbering = NumberingConfig::default();
    let base = sample_dir().join("research-images");
    write_images(&base);
    for case in cases() {
        let dir = sample_dir().join(case.name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut input = base_input();
        (case.tweak)(&mut input);
        let data =
            super::research::document_json(&input, case.markdown, &numbering, &base).unwrap();
        std::fs::write(dir.join("doc.json"), data).unwrap();
        let outcome = super::research::write_pdf_with_base(
            &dir.join("typst.pdf"),
            &input,
            case.markdown,
            &numbering,
            &base,
        )
        .unwrap();
        if case.name == "research-body-flow" {
            render_pages(outcome.pdf.clone(), &dir);
        }
        for warning in &outcome.warnings {
            eprintln!("{}: {warning}", case.name);
        }
        assert!(
            !outcome
                .warnings
                .iter()
                .any(|w| w.starts_with("公式无法排版")),
            "{} 公式应正常出图：{:?}",
            case.name,
            outcome.warnings
        );
    }
}

#[test]
#[ignore = "依赖本机 runtime，手动运行"]
fn typst_samples_research() {
    run();
}

/// 扫过页尾临界位置，必须出现标题接一行正文、剩余两行续到下一页的样张。
/// 继续扫到只放得下标题的位置，必须报单挂并定位，不能由排版器自动带走标题。
#[test]
#[ignore = "依赖本机 runtime，手动运行"]
fn typst_samples_compact_pagination() {
    use crate::models::{FontConfig, ResearchTemplate};
    use crate::typst_engine::{self, Template, TypstJob};
    use crate::units::UnitDisplay;
    use crate::visual_diff::ElementMarks;

    assert!(crate::portable_runtime::find_font_dir().is_some());
    let fonts = FontConfig::default();
    let set = typst_engine::font_set(&fonts).unwrap();
    let numbering = NumberingConfig::default();
    for (kind, template) in [
        (TemplateKind::ResearchReport, ResearchTemplate::Classic),
        (TemplateKind::ResearchReport, ResearchTemplate::Terminal),
        (TemplateKind::OfficialLetter, ResearchTemplate::Classic),
    ] {
        let mut found = false;
        let mut hanging_found = false;
        for count in 10..45 {
            let mut input = DraftInput {
                kind,
                ..Default::default()
            };
            input.research.template = template;
            let chapter = if kind == TemplateKind::ResearchReport {
                "## 分页验收\n\n"
            } else {
                ""
            };
            let markdown = format!(
                "# 紧致分页验收\n\n<!-- [正文] -->\n\n{chapter}{}### 页尾接排标题\n\n接排正文{}次行起点{}段末标记。\n",
                "填充一行正文。\n\n".repeat(count),
                "甲".repeat(if kind == TemplateKind::ResearchReport {
                    25
                } else {
                    22
                }),
                "乙".repeat(27),
            );
            let dir = sample_dir().join(if template == ResearchTemplate::Terminal {
                "terminal-compact-pagination"
            } else if kind == TemplateKind::ResearchReport {
                "research-compact-pagination"
            } else {
                "official-compact-pagination"
            });
            std::fs::create_dir_all(&dir).unwrap();
            let data = if kind == TemplateKind::ResearchReport {
                super::research::document_json(&input, &markdown, &numbering, &dir).unwrap()
            } else {
                super::document_json(
                    &input,
                    &markdown,
                    &UnitDisplay::new(&[]),
                    &numbering,
                    &ElementMarks::default(),
                    set.families.clone(),
                )
                .unwrap()
            };
            let job = TypstJob {
                data,
                base_dir: &dir,
                template: if kind == TemplateKind::ResearchReport {
                    Template::Research
                } else {
                    Template::Official
                },
                files: Default::default(),
            };
            let items = typst_engine::text_fonts_for_test(&job, &set).unwrap();
            let page = |needle: &str| {
                items
                    .iter()
                    .find(|item| item.text.contains(needle))
                    .map(|item| item.page)
            };
            let head = page("页尾接排标题").unwrap();
            let first = page("接排正文").unwrap();
            let second = page("次行起点").unwrap();
            if head != first {
                let outcome = typst_engine::compile(&job, &set).unwrap();
                let metrics =
                    crate::orphan_probe::find_hanging_headings(outcome.proof.as_deref().unwrap());
                let line = markdown
                    .lines()
                    .position(|line| line.starts_with("### 页尾"))
                    .unwrap()
                    + 1;
                let metric = metrics
                    .iter()
                    .find(|metric| metric.source_line == line)
                    .unwrap();
                assert_eq!((metric.page, metric.following_page), (head, first));
                let note = crate::orphan_probe::heading_warning(metric, &markdown);
                assert_eq!(
                    note.span,
                    crate::export::block_span_for_line(&markdown, line)
                );
                assert!(note.message.contains("人工"));
                let hanging_dir = dir.join("hanging");
                std::fs::create_dir_all(&hanging_dir).unwrap();
                std::fs::write(hanging_dir.join("typst.pdf"), &outcome.pdf).unwrap();
                render_pages(outcome.pdf, &hanging_dir);
                hanging_found = true;
            } else if second == first + 1 && !found {
                let outcome = typst_engine::compile(&job, &set).unwrap();
                assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
                std::fs::write(dir.join("doc.json"), &job.data).unwrap();
                std::fs::write(dir.join("typst.pdf"), &outcome.pdf).unwrap();
                render_pages(outcome.pdf, &dir);
                found = true;
            }
            if found && hanging_found {
                break;
            }
        }
        assert!(found, "{kind:?} 应允许标题接一行正文后跨页");
        assert!(hanging_found, "{kind:?} 应保留并提示标题单挂，供人工精调");
    }
}

/// 四种编号分别覆盖段内、独立列表，以及单数、两位数、圈号上限和长条目折行。
#[test]
#[ignore = "依赖本机 runtime，手动运行"]
fn typst_samples_research_list_numbering() {
    use crate::models::{ListNumbering, ResearchPalette, ResearchTemplate};

    let base = sample_dir();
    for style in ListNumbering::ALL {
        let style_name = match style {
            ListNumbering::Circled => "circled",
            ListNumbering::HalfParen => "half-paren",
            ListNumbering::FullParen => "full-paren",
            ListNumbering::DecimalDot => "decimal-dot",
        };
        let numbering = NumberingConfig {
            list1: style,
            list2: style,
            ..Default::default()
        };
        let markdown = format!(
            "# 列表编号间距验收\n\n## 编号样式：{}\n\n\
             ### 单位数编号\n\n\
             段内条目：\n1. 普通中文；\n2. **加粗中文**；\n3. English text；\n4. $x=1$ 的公式开头。\n\n\
             1. 普通中文独立条目\n2. **加粗中文独立条目**\n3. English text\n4. $x=1$ 的公式开头\n\n\
             ### 两位数与折行\n\n\
             段内条目：\n9. 中文九；\n1. **中文十**；\n1. English eleven。\n\n\
             9. 中文九\n1. **中文十**\n1. 长条目折行检查：研究报告应保留清楚可辨的列表编号，编号与中文正文、加粗正文及英文之间应有适当间距，条目换行后也应保持自然的正文排版。\n\n\
             ### 圈号上限与回退\n\n\
             段内条目：\n20. 中文二十；\n1. **中文二十一**。\n\n\
             20. 中文二十\n1. **中文二十一**\n",
            style.label()
        );
        for (theme, template, palette) in [
            (
                "classic",
                ResearchTemplate::Classic,
                ResearchPalette::Bright,
            ),
            ("dark", ResearchTemplate::Terminal, ResearchPalette::Dark),
            (
                "bright",
                ResearchTemplate::Terminal,
                ResearchPalette::Bright,
            ),
        ] {
            let dir = base.join(format!("research-lists-{theme}-{style_name}"));
            std::fs::create_dir_all(&dir).unwrap();
            let mut input = base_input();
            input.research.template = template;
            input.research.palette = palette;
            let data =
                super::research::document_json(&input, &markdown, &numbering, &base).unwrap();
            // 不带公式的同样源码另排一遍检查真实字形；不需要加载公式 SVG。
            let plain = markdown.replace("$x=1$", "公式");
            let plain_data =
                super::research::document_json(&input, &plain, &numbering, &base).unwrap();
            let set = crate::typst_engine::font_set(&crate::models::FontConfig::default()).unwrap();
            let text = crate::typst_engine::text_fonts_for_test(
                &crate::typst_engine::TypstJob {
                    data: plain_data,
                    base_dir: &base,
                    template: crate::typst_engine::Template::Research,
                    files: Default::default(),
                },
                &set,
            )
            .unwrap();
            assert!(
                text.iter().all(|item| item.missing_glyphs == 0),
                "{theme}/{style_name} 不应缺少编号或文字字形：{text:?}"
            );
            std::fs::write(dir.join("doc.json"), &data).unwrap();
            std::fs::write(dir.join("source.md"), &markdown).unwrap();
            let outcome = super::research::write_pdf_with_base(
                &dir.join("typst.pdf"),
                &input,
                &markdown,
                &numbering,
                &base,
            )
            .unwrap();
            assert!(
                outcome.warnings.is_empty(),
                "{theme}/{style_name}: {:?}",
                outcome.warnings
            );
            render_pages(outcome.pdf, &dir);
        }
    }
}

/// 两种配色覆盖同一组完整研究语法与不同文件类型，同时留 PNG 供目视验收。
#[test]
#[ignore = "依赖本机 runtime，手动运行"]
fn typst_samples_research_terminal() {
    use crate::models::{ResearchPalette, ResearchTemplate};
    let base = sample_dir().join("research-terminal-images");
    write_images(&base);
    crate::storage::set_test_config_dir(Some(base.clone()));
    for palette in [ResearchPalette::Dark, ResearchPalette::Bright] {
        for case in cases() {
            let mut markdown = case.markdown.to_owned();
            if case.name == "research-details" {
                markdown.push_str("\n\n");
                for i in 1..=35 {
                    markdown.push_str(&format!("> 跨页材料第{i}段：按预设条件组织样本，核验来源、口径与结论，保留必要的研究过程记录。\n>\n"));
                }
                markdown.push_str("> ——跨页研究材料\n");
            }
            let name = format!(
                "terminal-{}-{}",
                if palette == ResearchPalette::Dark {
                    "dark"
                } else {
                    "bright"
                },
                case.name
            );
            let dir = sample_dir().join(&name);
            std::fs::create_dir_all(&dir).unwrap();
            let mut input = base_input();
            (case.tweak)(&mut input);
            input.research.template = ResearchTemplate::Terminal;
            input.research.palette = palette;
            let outcome = super::research::write_pdf_with_base(
                &dir.join("typst.pdf"),
                &input,
                &markdown,
                &NumberingConfig::default(),
                &base,
            )
            .unwrap();
            crate::export::research::write_docx(
                &dir.join("terminal.docx"),
                &input,
                &markdown,
                &NumberingConfig::default(),
            )
            .unwrap();
            let data = super::research::document_json(
                &input,
                &markdown,
                &NumberingConfig::default(),
                &base,
            )
            .unwrap();
            std::fs::write(dir.join("doc.json"), data).unwrap();
            assert!(
                outcome
                    .warnings
                    .iter()
                    .all(|w| !w.starts_with("公式无法排版")),
                "{name}: {:?}",
                outcome.warnings
            );
            render_pages(outcome.pdf, &dir);
        }
    }
    crate::storage::set_test_config_dir(None);
}
