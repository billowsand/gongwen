//! 研究报告封面的版式口径：TeX 模板、Word 与宿主程序的预览三处共用。
//!
//! 七种文件类型共用一张 A4 网格（页边 25 mm，版心宽 160 mm），自上而下为
//! 密级/编号 → 文种 → 标识行 → 通栏反线 → 题名与稿次 → 专属信息格 → 落款。
//! 两类文件的差别只有两处：
//! - 项目类（立项论证、建设实施、技术实现、项目总结）用单反线，落款上方印
//!   四格阶段条，当前阶段加粗；
//! - 研究类（战略咨询、专题研究、外文翻译及其余）用一粗一细双反线，
//!   落款上方印署名行（课题组、编译审校）。
//!
//! 全部用黑色，文种用黑体小二、不着色，避免排成红头文件的样子。

/// 文件类型的预设。表单下拉按这个顺序列出；也允许手填其它名称。
pub const DOC_TYPE_PRESETS: [&str; 7] = [
    "立项论证报告",
    "建设实施方案",
    "技术实现方案",
    "项目总结报告",
    "战略咨询报告",
    "专题研究报告",
    "外文翻译",
];

/// 项目类阶段条的四格。
pub const PROJECT_STAGES: [&str; 4] = ["立项论证", "建设实施", "技术实现", "项目总结"];

/// 封面所属的类别。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Family {
    /// 项目类，`stage` 是 [`PROJECT_STAGES`] 的下标。
    Project { stage: usize },
    /// 研究类。
    Research(ResearchKind),
}

/// 研究类里各文种只影响表单提示，封面排法相同。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResearchKind {
    Consulting,
    Topic,
    Translation,
    Other,
}

impl Family {
    /// 按文件类型归类。先认预设全名，再按关键词兜底，认不出的一律按研究类排。
    pub fn of(doc_type: &str) -> Self {
        let t = doc_type.trim();
        let has = |keys: &[&str]| keys.iter().any(|key| t.contains(key));
        if has(&["立项", "可行性"]) {
            Self::Project { stage: 0 }
        } else if has(&["建设实施", "实施方案", "建设方案"]) {
            Self::Project { stage: 1 }
        } else if has(&["技术实现", "技术方案"]) {
            Self::Project { stage: 2 }
        } else if has(&["项目总结", "验收"]) {
            Self::Project { stage: 3 }
        } else if has(&["翻译", "译文", "译稿"]) {
            Self::Research(ResearchKind::Translation)
        } else if has(&["咨询"]) {
            Self::Research(ResearchKind::Consulting)
        } else if has(&["专题"]) {
            Self::Research(ResearchKind::Topic)
        } else {
            Self::Research(ResearchKind::Other)
        }
    }

    pub fn is_project(self) -> bool {
        matches!(self, Self::Project { .. })
    }

    /// 项目类的当前阶段。
    pub fn stage(self) -> Option<usize> {
        match self {
            Self::Project { stage } => Some(stage),
            Self::Research(_) => None,
        }
    }
}

/// 封面日期统一写成汉字数字：`2026年9月`、`2026-09` → `二〇二六年九月`。
/// 认不出的写法原样保留。
pub fn chinese_date(raw: &str) -> String {
    let raw = raw.trim();
    let Ok(re) = regex::Regex::new(r"^(\d{4})\s*[-/.年]\s*(\d{1,2})\s*月?$") else {
        return raw.to_string();
    };
    let Some(caps) = re.captures(raw) else {
        return raw.to_string();
    };
    let month: u32 = caps[2].parse().unwrap_or(0);
    if !(1..=12).contains(&month) {
        return raw.to_string();
    }
    const DIGITS: [char; 10] = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
    let year: String = caps[1]
        .chars()
        .map(|ch| DIGITS[ch.to_digit(10).unwrap_or(0) as usize])
        .collect();
    let month = match month {
        10 => "十".to_string(),
        11 | 12 => format!("十{}", DIGITS[(month - 10) as usize]),
        _ => DIGITS[month as usize].to_string(),
    };
    format!("{year}年{month}月")
}

/// 封面左上角的密级。公开件不标密级，返回空串。
pub fn security_label(level: &str) -> &str {
    match level.trim() {
        "公开" => "",
        level => level,
    }
}

/// 稿次写在题名下方，统一加全角括号；已带括号的不重复加。
pub fn version_mark(version: &str) -> Option<String> {
    let version = version.trim();
    if version.is_empty() {
        return None;
    }
    if version.starts_with('（') || version.starts_with('(') {
        return Some(version.to_string());
    }
    Some(format!("（{version}）"))
}

/// 各元素的位置（距页顶，mm）与字号（pt）。TeX 模板里写的是同一组数，
/// 改这里必须同步 `resources/research/template.tex`。
pub mod layout {
    pub const PAGE_WIDTH: f32 = 210.0;
    pub const PAGE_HEIGHT: f32 = 297.0;
    /// 左右页边。
    pub const SIDE: f32 = 25.0;
    pub const TEXT_WIDTH: f32 = PAGE_WIDTH - 2.0 * SIDE;

    /// 密级（左）与编号（右）行的顶边。
    pub const META_TOP: f32 = 20.0;
    pub const META_PT: f32 = 12.0;

    /// 文种行的顶边：黑体小二，字距 0.5 字。
    pub const TYPE_TOP: f32 = 68.0;
    pub const TYPE_PT: f32 = 18.0;
    pub const TYPE_TRACKING_EM: f32 = 0.5;

    /// 标识行（项目编号 / 期号 / 课题编号 / 原文出处）的顶边。
    pub const IDENT_TOP: f32 = 83.0;
    pub const IDENT_PT: f32 = 14.0;

    /// 反线顶边。粗线 0.5 mm；研究类在粗线下 0.8 mm 处另加 0.2 mm 细线。
    pub const RULE_TOP: f32 = 94.0;
    pub const RULE_THICK: f32 = 0.5;
    pub const RULE_GAP: f32 = 0.8;
    pub const RULE_THIN: f32 = 0.2;

    /// 题名顶边：小标宋一号，行距 1.45 倍。
    pub const TITLE_TOP: f32 = 114.0;
    pub const TITLE_PT: f32 = 26.0;
    pub const TITLE_LEADING: f32 = 1.45;
    /// 题名与稿次、外文原题之间的间距。
    pub const TITLE_GAP: f32 = 6.0;
    pub const VERSION_PT: f32 = 15.0;
    pub const ORIGINAL_PT: f32 = 15.0;

    /// 项目类阶段条的顶边：底线 0.2 mm 灰，当前格 0.8 mm 黑。
    pub const STAGE_TOP: f32 = 218.0;
    pub const STAGE_PT: f32 = 10.5;
    pub const STAGE_GAP: f32 = 2.0;
    pub const STAGE_LINE: f32 = 0.2;
    pub const STAGE_LINE_ON: f32 = 0.8;
    /// 研究类署名行的顶边。
    pub const BYLINE_TOP: f32 = 224.0;
    pub const BYLINE_PT: f32 = 14.0;

    /// 落款单位顶边：黑体三号；日期在单位下 5 mm，宋体小三。
    pub const ORG_TOP: f32 = 245.0;
    pub const ORG_PT: f32 = 16.0;
    pub const DATE_GAP: f32 = 5.0;
    pub const DATE_PT: f32 = 15.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_fall_into_the_expected_family() {
        let stages: Vec<_> = DOC_TYPE_PRESETS
            .iter()
            .map(|t| Family::of(t).stage())
            .collect();
        assert_eq!(
            stages,
            [Some(0), Some(1), Some(2), Some(3), None, None, None]
        );
        assert_eq!(
            Family::of("外文翻译"),
            Family::Research(ResearchKind::Translation)
        );
        assert_eq!(
            Family::of("战略咨询报告"),
            Family::Research(ResearchKind::Consulting)
        );
        assert_eq!(
            Family::of("研究报告"),
            Family::Research(ResearchKind::Other)
        );
    }

    #[test]
    fn dates_are_written_in_chinese_numerals() {
        assert_eq!(chinese_date("2026年9月"), "二〇二六年九月");
        assert_eq!(chinese_date("2026-10"), "二〇二六年十月");
        assert_eq!(chinese_date("2027/12"), "二〇二七年十二月");
        assert_eq!(chinese_date("二〇二六年九月"), "二〇二六年九月");
        assert_eq!(chinese_date("2026年9月1日"), "2026年9月1日");
    }

    #[test]
    fn version_gets_full_width_brackets_once() {
        assert_eq!(version_mark("V1.2").as_deref(), Some("（V1.2）"));
        assert_eq!(version_mark("（送审稿）").as_deref(), Some("（送审稿）"));
        assert_eq!(version_mark("  "), None);
    }
}
