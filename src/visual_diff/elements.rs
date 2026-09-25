//! 公文要素的就地标注：版头 / 版记要素两版对照，给出「旧值删除线、新值加框」。
//!
//! 方案需求第 9 条与规则 7：主送、抄送、成文日期、发文字号、落款单位、密级这些
//! 要素改了，就在纸面原位把旧值画删除线、新值加框，**整字段替换，不做词级**。
//! 没改的字段不给任何记号，三处渲染照旧走各自的原生路径（定稿导出逐字节不变）。
//!
//! 纸上印的不是 `DraftInput` 的原始字段值，所以比较和标注都基于
//! `export::element_display` 的**显示值**（主送经层级展开、落款按文种取全称或
//! 简称、密级拼保密期限……），与三处渲染同一份来源；不用
//! `visual_diff::model::DocumentModel::from_inputs` 里那套原始拼接——那只适合做
//! 比较。
//!
//! 按部件标的地方（与渲染命令一一对应）：
//! - 成文日期在 TeX 里是 `\SignatureYear` / `\SignatureMonth` / `\SignatureDay`
//!   三个命令：年 / 月 / 日按部件分别标，变了的部件整体删旧插新。日期切不开
//!   （自由文本）时整串一个部件。
//! - 发文字号在 TeX 里同样是三个命令（`\DepartmentCode` / `\Year` /
//!   `\DocumentNumber`），中间夹着类文件写死的〔〕号，整串塞不进任何一个参数，
//!   因此与日期同一规则：代字 / 年份 / 序号按部件分别标。
//! - 落款单位每行一个部件（白头件 / 红头呈批件多单位分行）。

use crate::export::element_display::{
    PREVIEW_PLACEHOLDER, addressee_display, copies_to_display, date_display_line,
    date_display_parts, is_preview_version, number_display_parts, security_display,
    signing_unit_display,
};
use crate::export::{mark_added, mark_deleted};
use crate::models::DraftInput;
use crate::units::UnitDisplay;

/// 一个字段（或部件）的新旧显示值。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FieldMark {
    old: String,
    new: String,
}

impl FieldMark {
    fn new(old: String, new: String) -> Self {
        Self { old, new }
    }

    /// 两侧显示值是否不同。相同就是纸面没变。
    pub(crate) fn changed(&self) -> bool {
        self.old != self.new
    }

    /// 没变返回 `None`，变了返回（旧显示值，新显示值）。
    pub(crate) fn change(&self) -> Option<(&str, &str)> {
        self.changed()
            .then_some((self.old.as_str(), self.new.as_str()))
    }

    /// 带哨兵文本：没变返回新值原文；变了返回删旧插新——任一侧为空则只有另一侧。
    pub(crate) fn marked(&self) -> String {
        if !self.changed() {
            return self.new.clone();
        }
        match (self.old.is_empty(), self.new.is_empty()) {
            (true, true) => String::new(),
            (true, false) => mark_added(&self.new),
            (false, true) => mark_deleted(&self.old),
            (false, false) => format!("{}{}", mark_deleted(&self.old), mark_added(&self.new)),
        }
    }
}

/// 成文日期的标注：能按「年月日」切开时按部件标，切不开时整串一个部件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DateMarks {
    /// 年 / 月 / 日三个部件，变了的部件整体删旧插新。
    Parts([FieldMark; 3]),
    /// 自由文本日期：整串一个部件。
    Whole(FieldMark),
}

impl DateMarks {
    /// 按部件标时的（年，月，日）。
    pub(crate) fn parts(&self) -> Option<[&FieldMark; 3]> {
        match self {
            Self::Parts(parts) => Some([&parts[0], &parts[1], &parts[2]]),
            Self::Whole(_) => None,
        }
    }

    /// 整串标时的那一个部件。
    pub(crate) fn whole(&self) -> Option<&FieldMark> {
        match self {
            Self::Parts(_) => None,
            Self::Whole(whole) => Some(whole),
        }
    }

    /// 有没有任何部件变了。
    pub(crate) fn changed(&self) -> bool {
        match self {
            Self::Parts(parts) => parts.iter().any(FieldMark::changed),
            Self::Whole(whole) => whole.changed(),
        }
    }

    /// 整字段的（旧显示值，新显示值）：任一部件变了就给，都没变返回 `None`。
    pub(crate) fn change(&self) -> Option<(String, String)> {
        self.changed().then(|| (self.old_line(), self.new_line()))
    }

    fn old_line(&self) -> String {
        self.line(true)
    }

    fn new_line(&self) -> String {
        self.line(false)
    }

    /// 按部件拼回「○年○月○日」整行；整串标时就是那一串。
    fn line(&self, old: bool) -> String {
        match self {
            Self::Whole(whole) => {
                if old {
                    whole.old.clone()
                } else {
                    whole.new.clone()
                }
            }
            Self::Parts(parts) => format!(
                "{}年{}月{}日",
                side(&parts[0], old),
                side(&parts[1], old),
                side(&parts[2], old)
            ),
        }
    }

    /// 带哨兵的整行文本：按部件时逐部件标注再拼「○年○月○日」，整串时整串标注。
    pub(crate) fn marked_line(&self) -> String {
        match self {
            Self::Whole(whole) => whole.marked(),
            Self::Parts(parts) => format!(
                "{}年{}月{}日",
                parts[0].marked(),
                parts[1].marked(),
                parts[2].marked()
            ),
        }
    }
}

/// 取部件某一侧的显示值。
fn side(part: &FieldMark, old: bool) -> &str {
    if old { &part.old } else { &part.new }
}

/// 一组公文要素的新旧对照：每个字段没变是「两侧同值」，变了才给出删旧插新。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ElementMarks {
    /// 主送机关位（函稿主送 / 白头件与红头呈批件的呈报领导）。
    recipient: FieldMark,
    /// 抄送机关。
    copies_to: FieldMark,
    /// 成文日期。
    date: DateMarks,
    /// 发文字号：代字 / 年份 / 序号。
    number: [FieldMark; 3],
    /// 落款单位，每行一个部件（多单位自上而下分行）。
    signing_units: Vec<FieldMark>,
    /// 密级（含保密期限）。
    security: FieldMark,
}

impl Default for DateMarks {
    fn default() -> Self {
        Self::Whole(FieldMark::default())
    }
}

impl ElementMarks {
    /// 要素全都没变：没有任何标注，三处渲染照旧走原生路径。
    pub(crate) fn is_empty(&self) -> bool {
        self.recipient.change().is_none()
            && self.copies_to.change().is_none()
            && self.date.change().is_none()
            && self.number.iter().all(|part| part.change().is_none())
            && self
                .signing_units
                .iter()
                .all(|part| part.change().is_none())
            && self.security.change().is_none()
    }

    /// 主送机关位（含呈报领导）。
    pub(crate) fn recipient(&self) -> &FieldMark {
        &self.recipient
    }

    /// 抄送机关。
    pub(crate) fn copies_to(&self) -> &FieldMark {
        &self.copies_to
    }

    /// 成文日期。
    pub(crate) fn date(&self) -> &DateMarks {
        &self.date
    }

    /// 发文字号的三个部件：代字、年份、序号。
    pub(crate) fn number(&self) -> &[FieldMark; 3] {
        &self.number
    }

    /// 发文字号整行的带哨兵文本：代字〔年份〕序号 + `gap` + 号，三个部件各自标注。
    /// 与 TeX 类文件的拼法一致（函稿 `gap` 是一个西文空格，红头呈批件紧挨）。
    pub(crate) fn number_marked_line(&self, gap: &str) -> String {
        let [code, year, serial] = &self.number;
        format!(
            "{}〔{}〕{}{gap}号",
            code.marked(),
            year.marked(),
            serial.marked()
        )
    }

    /// 落款单位的每行部件。
    pub(crate) fn signing_units(&self) -> &[FieldMark] {
        &self.signing_units
    }

    /// 密级（含保密期限）。
    pub(crate) fn security(&self) -> &FieldMark {
        &self.security
    }

    /// 全部「标注单元」的带哨兵文本：单字段一个单元，日期与文号按部件、
    /// 落款单位按行各成单元。三方一致性测试逐单元比对预览 / DOCX / TeX。
    #[cfg(test)]
    pub(crate) fn marked_units(&self) -> Vec<String> {
        let mut units = vec![self.recipient.marked(), self.copies_to.marked()];
        match &self.date {
            DateMarks::Parts(parts) => {
                units.extend(parts.iter().map(FieldMark::marked));
            }
            DateMarks::Whole(whole) => units.push(whole.marked()),
        }
        units.extend(self.number.iter().map(FieldMark::marked));
        units.extend(self.signing_units.iter().map(FieldMark::marked));
        units.push(self.security.marked());
        units
    }
}

/// 两版要素对照：每个字段给出新旧显示值，显示值与三处渲染同一套
/// （`export::element_display`），预览版留白（文号序号、日期「日」位）两侧都
/// 换成占位后再比——纸面上没变的，不该出标注。
pub(crate) fn element_marks(
    old: &DraftInput,
    new: &DraftInput,
    display: &UnitDisplay,
) -> ElementMarks {
    ElementMarks {
        recipient: FieldMark::new(
            addressee_display(old, display),
            addressee_display(new, display),
        ),
        copies_to: FieldMark::new(
            copies_to_display(old, display),
            copies_to_display(new, display),
        ),
        date: date_marks(old, new),
        number: number_marks(old, new),
        signing_units: line_marks(signing_lines(old, display), signing_lines(new, display)),
        security: FieldMark::new(
            security_display(old).unwrap_or_default(),
            security_display(new).unwrap_or_default(),
        ),
    }
}

/// 成文日期：两侧都能按「年月日」切开就按部件比，否则整串比。
fn date_marks(old: &DraftInput, new: &DraftInput) -> DateMarks {
    let old_parts = date_parts_marked_view(old);
    let new_parts = date_parts_marked_view(new);
    if let ([year_old, month_old, day_old], [year_new, month_new, day_new]) =
        (old_parts.as_slice(), new_parts.as_slice())
    {
        return DateMarks::Parts([
            FieldMark::new(year_old.clone(), year_new.clone()),
            FieldMark::new(month_old.clone(), month_new.clone()),
            FieldMark::new(day_old.clone(), day_new.clone()),
        ]);
    }
    DateMarks::Whole(FieldMark::new(
        date_display_line(old),
        date_display_line(new),
    ))
}

/// 日期部件的对照视图：预览版「日」位留白，两侧都留白时纸面没变。
fn date_parts_marked_view(input: &DraftInput) -> Vec<String> {
    let mut parts = date_display_parts(input);
    if parts.len() == 3 && is_preview_version(input) {
        parts[2] = PREVIEW_PLACEHOLDER.to_string();
    }
    parts
}

/// 发文字号：代字 / 年份 / 序号三个部件分别比，预览版序号留白。
fn number_marks(old: &DraftInput, new: &DraftInput) -> [FieldMark; 3] {
    let old_parts = number_parts_marked_view(old);
    let new_parts = number_parts_marked_view(new);
    std::array::from_fn(|index| FieldMark::new(old_parts[index].clone(), new_parts[index].clone()))
}

fn number_parts_marked_view(input: &DraftInput) -> [String; 3] {
    let (code, year, serial) = number_display_parts(input);
    let serial = if is_preview_version(input) {
        PREVIEW_PLACEHOLDER.to_string()
    } else {
        serial
    };
    [code, year, serial]
}

/// 落款单位的显示行（每行一个单位），去掉空行。
fn signing_lines(input: &DraftInput, display: &UnitDisplay) -> Vec<String> {
    signing_unit_display(input, display)
        .into_iter()
        .filter(|unit| !unit.trim().is_empty())
        .collect()
}

/// 逐行配对（少的一侧按空值记），行变了就整行删旧插新。
fn line_marks(old: Vec<String>, new: Vec<String>) -> Vec<FieldMark> {
    let len = old.len().max(new.len());
    (0..len)
        .map(|index| {
            FieldMark::new(
                old.get(index).cloned().unwrap_or_default(),
                new.get(index).cloned().unwrap_or_default(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    //! 要素标注的不变式：每个字段改了 / 没改 / 一侧为空，标注文本去掉哨兵后
    //! 等于新显示值（删除侧等于旧显示值），要素全没变时没有任何哨兵。

    use super::*;
    use crate::export::strip_redline;
    use crate::models::{LetterVersion, TemplateKind};

    fn input() -> DraftInput {
        DraftInput {
            kind: TemplateKind::OfficialLetter,
            date: "2026年8月7日".into(),
            profile: crate::models::TemplateProfile {
                issuing_unit: "星海省教育厅".into(),
                recipient: "甲市教育局".into(),
                copies_to: "乙市教育局".into(),
                department_code: "星教函".into(),
                document_year: "2026".into(),
                document_number: "12".into(),
                signing_unit: "星海省教育厅".into(),
                security_level: "秘密".into(),
                security_period: "10年".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn display() -> UnitDisplay<'static> {
        UnitDisplay::new(&[])
    }

    /// 断言一个字段的标注（与 `generated_edits_always_restore_both_sides` 同一口径）：
    /// 去掉删除段（连同哨兵）后等于新显示值；删除侧另测等于旧显示值；
    /// 纸面全文是「删旧 + 插新」两段。
    fn assert_marked(mark: &FieldMark, old: &str, new: &str) {
        assert_eq!(mark.change(), Some((old, new)));
        let marked = mark.marked();
        assert_eq!(
            kept_after_dropping(&marked, crate::export::RedlineKind::Deleted),
            new,
            "去掉删除段后应是新显示值"
        );
        assert_eq!(
            only(&marked, crate::export::RedlineKind::Deleted),
            old,
            "删除侧应是旧显示值"
        );
        assert_eq!(
            strip_redline(&marked),
            format!("{old}{new}"),
            "纸面全文是删旧插新"
        );
    }

    /// 取出标注文本里**不含** `drop` 类块的全部文字（`drop` = Deleted 时留下的是
    /// 新版文字，`drop` = Added 时留下的是旧版文字）。
    fn kept_after_dropping(text: &str, drop: crate::export::RedlineKind) -> String {
        crate::export::redline_chunks(text)
            .into_iter()
            .filter(|chunk| chunk.kind != drop)
            .map(|chunk| chunk.text)
            .collect()
    }

    /// 取出标注文本里 `keep` 类块的全部文字。
    fn only(text: &str, keep: crate::export::RedlineKind) -> String {
        crate::export::redline_chunks(text)
            .into_iter()
            .filter(|chunk| chunk.kind == keep)
            .map(|chunk| chunk.text)
            .collect()
    }

    #[test]
    fn an_unmarked_element_set_carries_no_sentinels() {
        let old = input();
        let new = input();
        let marks = element_marks(&old, &new, &display());
        assert!(marks.is_empty(), "要素没变就该是空标注");
        for unit in marks.marked_units() {
            assert!(
                !unit.chars().any(crate::export::is_redline_sentinel),
                "没变的单元不该有哨兵：{unit:?}"
            );
            assert_eq!(unit, strip_redline(&unit));
        }
    }

    #[test]
    fn each_element_field_marks_whole_values_when_changed() {
        let old = input();
        let mut new = input();
        new.profile.recipient = "甲市教育局、乙市教育局".into();
        new.profile.copies_to = "丙市教育局".into();
        new.profile.security_level = "机密".into();
        new.profile.security_period = "5年".into();
        let marks = element_marks(&old, &new, &display());

        assert_marked(marks.recipient(), "甲市教育局", "甲市教育局、乙市教育局");
        assert_marked(marks.copies_to(), "乙市教育局", "丙市教育局");
        // 密级拼保密期限，整字段替换：改期限也连密级一起换。
        assert_marked(marks.security(), "秘密★10年", "机密★5年");
    }

    #[test]
    fn an_empty_side_leaves_only_the_other_side_marked() {
        let old = input();
        let mut new = input();
        new.profile.copies_to.clear();
        new.profile.security_level.clear();
        new.profile.security_period.clear();
        new.profile.signing_unit.clear();
        new.profile.issuing_unit.clear();
        let marks = element_marks(&old, &new, &display());

        let copies = marks.copies_to();
        assert_eq!(copies.change(), Some(("乙市教育局", "")));
        assert_eq!(copies.marked(), crate::export::mark_deleted("乙市教育局"));

        let security = marks.security();
        assert_eq!(security.marked(), crate::export::mark_deleted("秘密★10年"));

        // 落款单位删掉后只剩删除侧。
        assert_eq!(marks.signing_units().len(), 1);
        assert_eq!(
            marks.signing_units()[0].marked(),
            crate::export::mark_deleted("星海省教育厅")
        );

        // 反向：旧侧为空只剩新增侧。
        let mut new = input();
        let old_empty = DraftInput {
            date: "2026年8月7日".into(),
            ..DraftInput::default()
        };
        new.profile.recipient = "甲市教育局".into();
        let marks = element_marks(&old_empty, &new, &display());
        assert_eq!(
            marks.recipient().marked(),
            crate::export::mark_added("甲市教育局")
        );
    }

    #[test]
    fn date_parts_are_marked_separately() {
        let old = input();
        let mut new = input();
        new.date = "2026年9月7日".into();
        let marks = element_marks(&old, &new, &display());
        let parts = marks.date().parts().expect("两侧都切得开年月日");
        assert_eq!(parts[0].change(), None, "年没变");
        assert_eq!(parts[2].change(), None, "日没变");
        assert_marked(parts[1], "8", "9");
        // 整行拼回来：只有月带哨兵。
        let line = marks.date().marked_line();
        assert_eq!(
            line,
            format!(
                "2026年{}{}月7日",
                crate::export::mark_deleted("8"),
                crate::export::mark_added("9")
            )
        );
        assert_eq!(
            kept_after_dropping(&line, crate::export::RedlineKind::Deleted),
            "2026年9月7日",
            "去掉删除段后是新版日期行"
        );
    }

    #[test]
    fn a_preview_version_date_and_number_never_mark_the_blank_slots() {
        let mut old = input();
        old.profile.letter_version = LetterVersion::Preview;
        old.profile.document_number = "11".into();
        let mut new = old.clone();
        // 预览版纸面留白：序号与「日」改了也看不见，不该出标注。
        new.profile.document_number = "12".into();
        new.date = "2026年8月9日".into();
        let marks = element_marks(&old, &new, &display());
        assert!(marks.is_empty(), "纸面没变就不该有标注：{:?}", marks);
    }

    #[test]
    fn number_parts_are_marked_separately() {
        let old = input();
        let mut new = input();
        new.profile.department_code = "星政函".into();
        new.profile.document_number = "15".into();
        let marks = element_marks(&old, &new, &display());
        let [code, year, serial] = marks.number();
        assert_marked(code, "星教函", "星政函");
        assert_eq!(year.change(), None, "年份没变");
        assert_marked(serial, "12", "15");
    }

    #[test]
    fn signature_units_are_marked_line_by_line() {
        let old = input();
        let mut new = input();
        new.profile.signing_unit = "星海省教育厅、教师处".into();
        let marks = element_marks(&old, &new, &display());
        // 白头件才分行；公函一行整体替换。
        assert_eq!(marks.signing_units().len(), 1);
        assert_marked(
            &marks.signing_units()[0],
            "星海省教育厅",
            "星海省教育厅、教师处",
        );

        let mut old = input();
        old.kind = TemplateKind::WhitePaper;
        old.profile.use_short_name_for_signature = false;
        old.profile.signing_unit = "甲单位、乙单位".into();
        let mut new = old.clone();
        new.profile.signing_unit = "甲单位、丙单位".into();
        let marks = element_marks(&old, &new, &display());
        assert_eq!(marks.signing_units().len(), 2, "白头件多单位分行");
        assert_eq!(marks.signing_units()[0].change(), None, "首行没变");
        assert_marked(&marks.signing_units()[1], "乙单位", "丙单位");
    }

    #[test]
    fn a_free_text_date_falls_back_to_one_whole_mark() {
        let mut old = input();
        old.date = "2026/8/7".into();
        let mut new = old.clone();
        new.date = "2026/9/7".into();
        let marks = element_marks(&old, &new, &display());
        let whole = marks.date().whole().expect("切不开就整串一个部件");
        assert_marked(whole, "2026/8/7", "2026/9/7");
        assert_eq!(marks.date().marked_line(), whole.marked());
    }
}
