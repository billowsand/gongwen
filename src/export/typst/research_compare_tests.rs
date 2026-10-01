//! 研究报告 Typst / TeX 双引擎对照：同一份稿子分别走 mdx + 内置 Tectonic 与 Typst，
//! PDF 落到 `tmp/typst-compare/research-<用例>/`，再由 `scripts/typst-compare.py`
//! 逐行比对基线位置。依赖本机 `runtime/`，默认忽略：
//!
//! ```text
//! cargo test --locked typst_tex_compare_research -- --ignored --nocapture
//! python scripts/typst-compare.py research-full research-project ...
//! ```

use std::path::{Path, PathBuf};

use crate::models::{DraftInput, NumberingConfig, TemplateKind};

fn compare_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tmp")
        .join("typst-compare")
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

数据作为新型生产要素，已快速融入生产、分配、流通、消费和社会服务管理等各环节[@wang2020]。2022年发布的“数据二十条”明确提出推进公共数据确权授权机制建设[@gov2022; @oecd2019]，各地随即开展了形式多样的探索。

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

fn run(only_typst: bool) {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let numbering = NumberingConfig::default();
    let base = compare_dir().join("research-images");
    write_images(&base);
    for case in cases() {
        let dir = compare_dir().join(case.name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut input = base_input();
        (case.tweak)(&mut input);
        if !only_typst {
            let tex = dir.join("tex.tex");
            crate::export::research::write_tex_with_base(
                &tex,
                &input,
                case.markdown,
                &numbering,
                &base,
            )
            .unwrap();
            crate::texcompile::compile_research_pdf(&tex).unwrap();
        }
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
        for warning in &outcome.warnings {
            eprintln!("{}: {warning}", case.name);
        }
    }
}

#[test]
#[ignore = "依赖本机 runtime，手动运行"]
fn typst_tex_compare_research() {
    run(false);
}

#[test]
#[ignore = "依赖本机 runtime，手动运行"]
fn typst_tex_compare_research_typst_only() {
    run(true);
}
