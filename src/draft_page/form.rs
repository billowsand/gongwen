//! 公文要素填报区：分段表单、必填校验与版面缩略图。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::form`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::{
    CONTENT_WIDTH, DraftAction, FORM_CONTROL_HEIGHT, FORM_FIELD_MIN_WIDTH, FORM_LAYOUT_GUTTER,
    LABEL_WIDTH, SelectOption, contact_pair, joint_responsible_editor, layout_options,
    multi_select, plain_options, row_label, row_label_with_info, single_select,
    switch_template_profile, warn,
};
use crate::draft_page::DraftPage;
use crate::models::{
    CorrespondenceScope, DraftInput, JointIssuanceMode, LetterVersion, ReviewNote, SecurityLevel,
    StyleMode, TemplateKind, VocabularyCategory, split_units,
};
use crate::prompt;
use crate::theme;
use crate::units;
use crate::units::UnitDisplay;
use crate::validator;
use eframe::egui;
use std::collections::BTreeSet;

/// 公文要素按纸面部位分成的三段。填表的人脑子里是一张纸：先版头、再主体、
/// 最后版记。表单顺序与打印出来的纸一致，比按数据类型分组好找得多——
/// 密级印在纸的最上面，就不该排在表单第三节。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormSection {
    /// 密级、发文机关标志（红头）、文号。
    Header,
    /// 主送单位、标题、正文体例。
    Body,
    /// 抄送、承办、落款、成文日期、印发。
    Record,
}

impl FormSection {
    pub(crate) const ALL: [Self; 3] = [Self::Header, Self::Body, Self::Record];

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Header => 0,
            Self::Body => 1,
            Self::Record => 2,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Header => "版头",
            Self::Body => "主体",
            Self::Record => "版记",
        }
    }

    /// 副标题写这一段在纸上对应哪几样东西，让分组名不必自我解释。
    pub(crate) fn hint(self, kind: TemplateKind) -> &'static str {
        match (self, kind) {
            (Self::Header, TemplateKind::PlainDocument) => "密级",
            (Self::Header, TemplateKind::MeetingAgenda) => "密级",
            (Self::Header, _) => "密级 · 红头 · 文号",
            (Self::Body, TemplateKind::MeetingAgenda) => "会议时间 · 地点 · 人员",
            (Self::Body, TemplateKind::WhitePaper | TemplateKind::RedHeadApproval) => {
                "呈报领导 · 标题"
            }
            (Self::Body, TemplateKind::PlainDocument) => "标题 · 体例",
            (Self::Body, _) => "主送 · 标题",
            (Self::Record, TemplateKind::PlainDocument) => "打印方式",
            (Self::Record, TemplateKind::PhoneNotice | TemplateKind::MeetingAgenda) => "成文日期",
            (Self::Record, TemplateKind::WhitePaper | TemplateKind::RedHeadApproval) => {
                "落款 · 承办 · 成文日期"
            }
            (Self::Record, _) => "抄送 · 承办 · 成文日期",
        }
    }

    pub(crate) fn icon(self) -> theme::Icon {
        match self {
            Self::Header => theme::Icon::Building,
            Self::Body => theme::Icon::Type,
            Self::Record => theme::Icon::Rows,
        }
    }
}

/// 三段的展开状态，外加缩略导航图点下的待跳转目标。
pub(crate) struct FormSectionState {
    open: [bool; 3],
    pending_jump: Option<FormSection>,
}

impl Default for FormSectionState {
    fn default() -> Self {
        Self {
            // 默认全开：要素区本来就默认收起，展开它的人就是来看全貌的。
            open: [true; 3],
            pending_jump: None,
        }
    }
}

/// 一段的填报进度。`total` 只数必填项——把选填项也算进分母，进度条永远到不了头。
#[derive(Default, Clone, Copy)]
pub(crate) struct SectionProgress {
    done: usize,
    total: usize,
}

impl SectionProgress {
    pub(crate) fn missing(self) -> usize {
        self.total.saturating_sub(self.done)
    }
}

/// 填报体检结果：每段的进度，以及"必填但还空着"的字段与说明。
///
/// 规则与 [`validator`] 里的必填校验一一对应，区别只在时机：那边是生成/审校
/// 时才跑、结果进右侧抽屉，这边每帧就地算，直接标在字段下面。
#[derive(Default)]
pub(crate) struct FormCheck {
    pub(crate) progress: [SectionProgress; 3],
    pub(crate) errors: std::collections::BTreeMap<&'static str, String>,
}

impl FormCheck {
    /// 记一个必填项。`filled` 为假时把说明挂到字段 id 上，渲染时就地显示。
    pub(crate) fn require(
        &mut self,
        section: FormSection,
        id: &'static str,
        filled: bool,
        message: &str,
    ) {
        let progress = &mut self.progress[section.index()];
        progress.total += 1;
        if filled {
            progress.done += 1;
        } else {
            self.errors.insert(id, message.to_string());
        }
    }

    pub(crate) fn get(&self, id: &str) -> Option<&String> {
        self.errors.get(id)
    }
}

/// 按当前文种算出必填清单。判定条件照抄 [`validator::validate`] 里对应的分支，
/// 避免表单说"齐了"、审校又报"缺少主送单位"。
pub(crate) fn check_form(draft: &DraftInput) -> FormCheck {
    let mut check = FormCheck::default();
    let profile = &draft.profile;
    let filled = |value: &str| !value.trim().is_empty();

    // 密级是所有文种的必填项。
    check.require(
        FormSection::Header,
        "security_level",
        filled(&profile.security_level),
        "密级是所有文稿类型的必填项",
    );

    match draft.kind {
        TemplateKind::OfficialLetter => {
            let joint = profile.joint_issuance_mode == JointIssuanceMode::Mode1;
            let issuing = if joint {
                &profile.joint_issuing_units
            } else {
                &profile.issuing_unit
            };
            check.require(
                FormSection::Header,
                "issuing_unit",
                filled(issuing),
                "公函必须有发文单位",
            );
            if joint {
                let units = split_units(&profile.joint_issuing_units);
                check.require(
                    FormSection::Header,
                    "joint_issuing_units_count",
                    units.len() >= 2,
                    "联合发文模式1至少需要两个发文单位",
                );
                check.require(
                    FormSection::Header,
                    "main_issuing_unit",
                    units
                        .iter()
                        .any(|unit| unit == profile.main_issuing_unit.trim()),
                    "须从发文单位中指定一个主发文单位",
                );
            }
            check.require(
                FormSection::Body,
                "recipient",
                filled(&profile.recipient),
                "公函必须有主送单位",
            );
        }
        TemplateKind::PhoneNotice => {
            check.require(
                FormSection::Header,
                "phone_notice_issuing_unit",
                filled(&profile.issuing_unit),
                "电话通知必须有发文单位",
            );
            check.require(
                FormSection::Body,
                "phone_notice_recipient",
                filled(&profile.recipient),
                "电话通知必须有主送单位",
            );
        }
        TemplateKind::PlainDocument => {}
        TemplateKind::MeetingAgenda => {
            // 议程三项可留空由模型提取，但留空就意味着要人工核对，仍按必填计数。
            check.require(
                FormSection::Body,
                "meeting_time",
                filled(&draft.meeting_time),
                "留空则由模型从素材提取，生成后需人工核对",
            );
            check.require(
                FormSection::Body,
                "meeting_location",
                filled(&profile.meeting_location),
                "留空则由模型从素材提取，生成后需人工核对",
            );
            check.require(
                FormSection::Body,
                "attendees",
                filled(&draft.attendees),
                "留空则由模型从素材归纳，生成后需人工核对",
            );
        }
        TemplateKind::WhitePaper => {
            check.require(
                FormSection::Body,
                "reporting_leaders",
                filled(&profile.reporting_leaders),
                "白头件必须有呈报领导",
            );
            check.require(
                FormSection::Record,
                "signing_unit",
                filled(&profile.signing_unit),
                "白头件必须有落款单位",
            );
        }
        TemplateKind::RedHeadApproval => {
            check.require(
                FormSection::Header,
                "red_approval_issuing_unit",
                filled(&profile.issuing_unit),
                "红头呈批件必须有发文单位",
            );
            check.require(
                FormSection::Body,
                "red_approval_reporting_leaders",
                filled(&profile.reporting_leaders),
                "红头呈批件必须有呈报领导",
            );
            check.require(
                FormSection::Record,
                "red_approval_signing_unit",
                filled(&profile.signing_unit),
                "红头呈批件必须有落款单位",
            );
            check.require(
                FormSection::Record,
                "red_approval_responsible",
                !crate::models::joint_responsible_entries(profile).is_empty(),
                "至少需要一条承办单位、联系人和电话",
            );
        }
    }
    check
}

/// 一段的折叠头：图标 + 名称 + 副标题 + 完成度徽章，整行可点。徽章的意义在于
/// 折叠状态下也能看出哪一段还缺东西，不必逐段展开找。
pub(crate) fn section_header_ui(
    ui: &mut egui::Ui,
    section: FormSection,
    kind: TemplateKind,
    open: &mut bool,
    progress: SectionProgress,
) -> egui::Response {
    let chevron = if *open {
        theme::Icon::ChevronDown
    } else {
        theme::Icon::ChevronRight
    };
    let inner = ui.horizontal(|ui| {
        ui.add(
            chevron
                .image()
                .tint(theme::text_soft())
                .fit_to_exact_size(egui::vec2(13.0, 13.0)),
        );
        ui.add(
            section
                .icon()
                .image()
                .tint(theme::accent())
                .fit_to_exact_size(egui::vec2(15.0, 15.0)),
        );
        ui.strong(section.label());
        ui.label(
            egui::RichText::new(section.hint(kind))
                .size(11.0)
                .color(theme::text_muted()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if progress.total == 0 {
                return;
            }
            let missing = progress.missing();
            let (color, text) = if missing == 0 {
                (
                    theme::success(),
                    format!("{} / {}", progress.done, progress.total),
                )
            } else {
                (theme::danger(), format!("缺 {missing} 项"))
            };
            ui.colored_label(color, egui::RichText::new(text).size(11.0));
        });
    });
    let response = inner.response.interact(egui::Sense::click());
    if response.clicked() {
        *open = !*open;
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 必填字段的标签：名称后跟一个红星。必填与否是填表时最想先知道的事，
/// 不该等到点了生成、在右侧抽屉里才发现。
pub(crate) fn required_row_label(ui: &mut egui::Ui, label: &str, tip: Option<&str>) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append(
        label,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: theme::text(),
            ..Default::default()
        },
    );
    job.append(
        " *",
        0.0,
        egui::TextFormat {
            font_id: font,
            color: theme::danger(),
            ..Default::default()
        },
    );
    let height = if label.contains('\n') {
        FORM_CONTROL_HEIGHT + 14.0
    } else {
        FORM_CONTROL_HEIGHT
    };
    let response = ui.add_sized(
        [LABEL_WIDTH, height],
        egui::Label::new(job).wrap_mode(egui::TextWrapMode::Wrap),
    );
    if let Some(tip) = tip {
        response.on_hover_text(tip.to_string());
    }
}

/// 必填但还空着时，在字段正下方补一行红字。必须在 `end_row` 之后调用：
/// 它自己占满一整行（标签列留空）。
pub(crate) fn field_error(ui: &mut egui::Ui, check: &FormCheck, id: &str) {
    let Some(message) = check.get(id) else {
        return;
    };
    ui.label("");
    ui.horizontal(|ui| {
        ui.add(
            theme::Icon::TriangleAlert
                .image()
                .tint(theme::danger())
                .fit_to_exact_size(egui::vec2(13.0, 13.0)),
        );
        ui.colored_label(
            theme::danger(),
            egui::RichText::new(message.as_str()).size(11.0),
        );
    });
    ui.end_row();
}

/// 文号那一行。机关代字、年份、序号本来就拼成一个东西（X政函〔2026〕12号），
/// 拆成三行既占地方，又看不出拼起来长什么样，所以并作一行并在下面给出成品。
#[allow(clippy::too_many_arguments)]
pub(crate) fn document_number_row(
    ui: &mut egui::Ui,
    profile: &mut crate::models::TemplateProfile,
    department_codes: &[SelectOption],
    manual_fields: &mut BTreeSet<String>,
    allow_free_text: bool,
    field_width: f32,
    code_hint: &str,
    code_id: &'static str,
) {
    row_label_with_info(
        ui,
        "文号",
        "机关代字、发文年份与序号合成一个文号；序号只在「正式版」下可填。",
    );
    let year_width = 42.0;
    let seq_width = 34.0;
    let code_width = (field_width - year_width - seq_width - 62.0).max(72.0);
    let formal = profile.letter_version == LetterVersion::Formal;
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            single_select(
                ui,
                code_id,
                &mut profile.department_code,
                department_codes,
                manual_fields,
                allow_free_text,
                code_width,
                code_hint,
            );
            ui.label("〔");
            ui.add(
                egui::TextEdit::singleline(&mut profile.document_year)
                    .hint_text("2026")
                    .char_limit(4)
                    .desired_width(year_width),
            );
            ui.label("〕");
            ui.add_enabled(
                formal,
                egui::TextEdit::singleline(&mut profile.document_number)
                    .hint_text("12")
                    .desired_width(seq_width),
            );
            ui.label("号");
        });
        let code = profile.department_code.trim();
        let year = profile.document_year.trim();
        if !code.is_empty() && !year.is_empty() {
            let number = profile.document_number.trim();
            let text = if !formal {
                format!("预览版：{code}〔{year}〕 号（流水号留空）")
            } else if number.is_empty() {
                format!("成文后显示为 {code}〔{year}〕□号")
            } else {
                format!("成文后显示为 {code}〔{year}〕{number}号")
            };
            ui.label(
                egui::RichText::new(text)
                    .size(11.0)
                    .color(theme::text_muted()),
            );
        }
    });
    ui.end_row();
}

/// 版面缩略导航图：把公文画成一张小纸，三块分别是版头、主体和版记，点哪块跳哪段。
/// 表单再怎么分组也不如直接指着纸说"我要改这儿"——公文的字段本就一一对应纸上
/// 的固定位置，缩略图是这层对应关系最省事的表达。
pub(crate) fn layout_thumbnail(
    ui: &mut egui::Ui,
    kind: TemplateKind,
    check: &FormCheck,
) -> Option<FormSection> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(76.0, 106.0), egui::Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, egui::CornerRadius::same(4), theme::surface());
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, theme::border_strong()),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(5.0);
    // 版头最高、版记最矮，大致是公文在纸上的实际比例。
    let ratios = [0.38, 0.36, 0.26];
    let mut picked = None;
    let mut top = inner.top();
    for (index, section) in FormSection::ALL.into_iter().enumerate() {
        let height = inner.height() * ratios[index] - 3.0;
        let zone = egui::Rect::from_min_size(
            egui::pos2(inner.left(), top),
            egui::vec2(inner.width(), height),
        );
        top += height + 4.5;
        let response = ui.interact(
            zone,
            ui.id().with(("form_thumbnail", index)),
            egui::Sense::click(),
        );
        let hovered = response.hovered();
        painter.rect_filled(
            zone,
            egui::CornerRadius::same(3),
            if hovered {
                theme::accent_soft()
            } else {
                theme::surface_sunk()
            },
        );
        if hovered {
            painter.rect_stroke(
                zone,
                3.0,
                egui::Stroke::new(1.0, theme::accent()),
                egui::StrokeKind::Inside,
            );
        }
        thumbnail_marks(&painter, zone, section, kind);
        if check.progress[index].missing() > 0 {
            painter.circle_filled(
                egui::pos2(zone.right() - 4.0, zone.top() + 4.0),
                2.5,
                theme::danger(),
            );
        }
        if response
            .on_hover_text(format!("{}：{}", section.label(), section.hint(kind)))
            .clicked()
        {
            picked = Some(section);
        }
    }
    picked
}

/// 缩略图每一块里的示意笔画：版头画红色的机关标志和反线，主体画标题与正文行，
/// 版记画几条短线。没有这些笔画，三个色块看不出是一张公文。
pub(crate) fn thumbnail_marks(
    painter: &egui::Painter,
    zone: egui::Rect,
    section: FormSection,
    kind: TemplateKind,
) {
    let line = |y: f32, left: f32, width: f32, thickness: f32, color: egui::Color32| {
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(zone.left() + left, y),
                egui::vec2(width, thickness),
            ),
            egui::CornerRadius::same(1),
            color,
        );
    };
    let w = zone.width();
    let muted = theme::text_muted();
    match section {
        FormSection::Header => {
            // 密级在左上角，红头居中，下面一条红色反线。
            line(zone.top() + 4.0, 5.0, w * 0.24, 2.0, muted);
            if kind.uses_letter_layout() || kind == TemplateKind::RedHeadApproval {
                line(zone.top() + 12.0, w * 0.16, w * 0.68, 4.5, theme::danger());
                line(zone.bottom() - 9.0, 5.0, w - 10.0, 1.5, theme::danger());
                line(zone.bottom() - 5.0, w * 0.34, w * 0.32, 2.0, muted);
            } else {
                line(zone.top() + 13.0, w * 0.22, w * 0.56, 3.0, muted);
            }
        }
        FormSection::Body => {
            // 主送顶格，标题居中，正文三行。
            line(zone.top() + 4.0, 5.0, w * 0.4, 2.0, muted);
            line(
                zone.top() + 11.0,
                w * 0.28,
                w * 0.44,
                3.0,
                theme::text_soft(),
            );
            for row in 0..3 {
                line(
                    zone.top() + 20.0 + row as f32 * 5.0,
                    5.0,
                    if row == 2 { w * 0.6 } else { w - 10.0 },
                    1.5,
                    muted,
                );
            }
        }
        FormSection::Record => {
            line(zone.top() + 4.0, 5.0, w * 0.46, 1.5, muted);
            line(zone.top() + 10.0, 5.0, w * 0.72, 1.5, muted);
            line(zone.top() + 16.0, 5.0, w * 0.3, 1.5, muted);
        }
    }
}

impl DraftPage<'_> {
    pub(crate) fn revalidate(&mut self) {
        self.doc.warnings = validator::validate(
            &self.doc.draft,
            &self.doc.generated_markdown,
            &self.config.vocabulary,
            &self.config.security_rules,
        )
        .into_iter()
        .map(ReviewNote::from)
        .collect();
        if self.doc.draft.kind.has_document_number()
            && self.doc.draft.profile.letter_version == LetterVersion::Formal
        {
            let year = self.doc.draft.document_year();
            let code = self.doc.draft.profile.department_code.trim();
            let serial = self
                .doc
                .draft
                .profile
                .document_number
                .trim()
                .parse::<u64>()
                .ok();
            if !year.is_empty()
                && !code.is_empty()
                && let Some(serial) = serial
                && let Some(highest) = self.store.as_deref().and_then(|store| {
                    store
                        .highest_letter_serial(&year, code, self.doc.manuscript_id)
                        .ok()
                        .flatten()
                })
                && serial < highest
            {
                self.doc.warnings.push(
                    format!(
                        "文号流水号 {serial} 低于机关代字“{code}”{year}年度已有最高序号 {highest}"
                    )
                    .into(),
                );
            }
        }
        // 孤行提示来自上一次编译的实测坐标，正文没动过才还算数；正文改过就退回
        // 纯字符宽度的粗估，等下次导出编译再拿准数。两者只能取其一，同时显示
        // 会对同一段落报出两条口径不同的提示。
        if self.doc.proof_markdown == self.doc.generated_markdown
            && !self.doc.proof_markdown.is_empty()
        {
            self.doc
                .warnings
                .extend(self.doc.proof_warnings.iter().cloned());
        } else {
            self.doc.proof_warnings.clear();
            self.doc.proof_markdown.clear();
            self.doc.warnings.extend(validator::estimate_layout_notes(
                &self.doc.generated_markdown,
            ));
        }
        // 只按文案排序去重：同一条提示不该因为 span 差一个字节就重复两遍。
        self.doc.warnings.sort_by(|a, b| a.message.cmp(&b.message));
        self.doc.warnings.dedup_by(|a, b| a.message == b.message);
    }

    /// 起草页各单位下拉框的候选池，按词库的树形顺序排列。
    /// 缩进和标签由 `layout_options` 按实际参与的子集算，这里只提供取值、全称和上级。
    pub(crate) fn unit_pool(&self, external: bool) -> Vec<SelectOption> {
        let display = UnitDisplay::new(&self.config.vocabulary);
        self.config
            .vocabulary
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .filter(|entry| !entry.canonical.trim().is_empty())
            .map(|entry| SelectOption {
                full: display.full_name_for(&entry.canonical, external),
                parent: display.parent_of(&entry.canonical),
                label: display.name_for(&entry.canonical, external),
                value: entry.canonical.trim().to_string(),
                depth: 0,
            })
            .collect()
    }

    /// 联系人及其绑定电话，两者始终成对取用。
    pub(crate) fn contacts(&self) -> Vec<(String, String)> {
        self.config
            .vocabulary
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Person)
            .map(|entry| {
                (
                    entry.canonical.trim().to_string(),
                    entry.phone.trim().to_string(),
                )
            })
            .filter(|(name, _)| !name.is_empty())
            .collect()
    }

    /// 规格 §2.3“人随事走”：只保留属于给定单位的人员；单位列表为空时不过滤。
    /// 公函可带承办上级单位人员；白头件呈报领导取落款单位及其上级单位的领导。
    /// 返回 `(人员, 是否应用了过滤)`，供界面提示。
    pub(crate) fn filtered_contacts(
        &self,
        units: &[String],
        include_parent_unit_handlers: bool,
        include_leader_superiors: bool,
    ) -> (Vec<(String, String)>, bool) {
        let clean = units
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if clean.is_empty() {
            return (self.contacts(), false);
        }
        let display = UnitDisplay::new(&self.config.vocabulary);
        let mut seen = std::collections::HashSet::new();
        let mut matched = Vec::new();
        for unit in &clean {
            let people = if include_parent_unit_handlers {
                display.responsible_people_of(unit)
            } else if include_leader_superiors {
                display.leaders_of(unit)
            } else {
                display.people_of(unit)
            };
            for (name, phone) in people {
                if seen.insert(name.clone()) {
                    matched.push((name, phone));
                }
            }
        }
        if matched.is_empty() {
            if include_parent_unit_handlers {
                // 公函版记严格执行承办权限，不能因候选为空而回落显示未授权人员。
                (matched, true)
            } else {
                // 白头件沿用原有兜底，避免呈报领导下拉框没有候选项。
                (self.contacts(), true)
            }
        } else {
            (matched, true)
        }
    }

    /// 顶部常驻区：文种、稿件版本、套版和版面缩略导航图。这几样决定"这是什么文、
    /// 下面三段里有哪些字段"，属于导航而不是填报，所以不跟着表单一起滚。
    pub(crate) fn form_header_ui(&mut self, ui: &mut egui::Ui, available_width: f32) {
        {
            let style = ui.style_mut();
            style.spacing.interact_size.y = FORM_CONTROL_HEIGHT;
            style.visuals.widgets.inactive.bg_fill = theme::surface();
            style.visuals.widgets.inactive.weak_bg_fill = theme::surface();
        }
        let content_width = available_width.clamp(*CONTENT_WIDTH.start(), *CONTENT_WIDTH.end());
        let field_width =
            (content_width - LABEL_WIDTH - FORM_LAYOUT_GUTTER).max(FORM_FIELD_MIN_WIDTH);
        let old_kind = self.doc.draft.kind;

        egui::Grid::new("template_grid")
            .num_columns(2)
            .striped(false)
            .min_row_height(FORM_CONTROL_HEIGHT)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                row_label_with_info(ui, "文种", self.doc.draft.kind.description());
                ui.horizontal(|ui| {
                    // 「预览版/正式版」是整篇稿子的状态而不是纸上的某个部位，
                    // 跟文种并排放在导航区，不占下面三段的行。
                    let versioned = self.doc.draft.kind != TemplateKind::PlainDocument;
                    let version_width = if versioned { 92.0 } else { 0.0 };
                    let kind_width = if versioned {
                        (field_width - version_width - 34.0).max(96.0)
                    } else {
                        field_width
                    };
                    egui::ComboBox::from_id_salt("template_kind")
                        .selected_text(self.doc.draft.kind.label())
                        .width(kind_width)
                        .show_ui(ui, |ui| {
                            for kind in TemplateKind::ALL {
                                ui.selectable_value(&mut self.doc.draft.kind, kind, kind.label());
                            }
                        });
                    if versioned {
                        let tip = match self.doc.draft.kind {
                            TemplateKind::OfficialLetter => {
                                "预览版自动留空函号流水号和成文日期的“日”；正式版使用填写值。"
                            }
                            TemplateKind::PhoneNotice | TemplateKind::WhitePaper => {
                                "预览版自动留空落款成文日期的“日”；正式版使用完整日期。"
                            }
                            TemplateKind::RedHeadApproval => {
                                "预览版自动留空发文序号和落款成文日期的“日”；正式版使用填写值。"
                            }
                            TemplateKind::MeetingAgenda => {
                                "会议议程无落款日期，稿件版本仅用于与其他文种保持一致。"
                            }
                            // 不能是 unreachable：`versioned` 在文种下拉框渲染之前求值，
                            // 用户点选“普通公文”的那一帧里 kind 已变、versioned 还是旧值 true，
                            // 版本下拉框会残留渲染这一帧，命中这里直接崩溃（曾因 unreachable 崩过）。
                            TemplateKind::PlainDocument => {
                                "普通公文无落款成文日期，稿件版本仅用于与其他文种保持一致。"
                            }
                        };
                        egui::ComboBox::from_id_salt("letter_version")
                            .selected_text(self.doc.draft.profile.letter_version.label())
                            .width(version_width)
                            .show_ui(ui, |ui| {
                                for version in LetterVersion::ALL {
                                    ui.selectable_value(
                                        &mut self.doc.draft.profile.letter_version,
                                        version,
                                        version.label(),
                                    );
                                }
                            })
                            .response
                            .on_hover_text(tip);
                    }
                });
                ui.end_row();

                // 套版：原先这个按钮埋在表单最底下，要滚到底才看得见。它其实是
                // 整个填报里最省事的一步——同一个处室发的文，八成字段年年一样。
                row_label_with_info(
                    ui,
                    "套版",
                    "把当前要素存成本文种的默认值；下次新建同类公文自动带出，只需改文号和主送。",
                );
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::icon_text_button(
                            theme::Icon::BookmarkCheck,
                            "存为本文种默认",
                        ))
                        .on_hover_text("下次新建本文种时自动带出当前这些要素")
                        .clicked()
                    {
                        self.actions.push(DraftAction::Persist);
                    }
                });
                ui.end_row();
            });

        if self.doc.draft.kind != old_kind {
            // 换模板等于换版式，上一次量到的孤行结果一律作废，交给下次编译重测。
            self.doc.proof_warnings.clear();
            self.doc.proof_markdown.clear();
            self.doc.warnings = switch_template_profile(
                self.config,
                &mut self.doc.draft,
                &self.doc.generated_markdown,
                old_kind,
            )
            .into_iter()
            .map(ReviewNote::from)
            .collect();
            self.doc.warnings.extend(validator::estimate_layout_notes(
                &self.doc.generated_markdown,
            ));
            self.doc.output_files.clear();
            self.doc.export_error = None;
        }

        ui.add_space(8.0);
        let check = check_form(&self.doc.draft);
        let kind = self.doc.draft.kind;
        ui.horizontal(|ui| {
            if let Some(section) = layout_thumbnail(ui, kind, &check) {
                self.doc.form_sections.pending_jump = Some(section);
                self.doc.form_sections.open[section.index()] = true;
            }
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new("点纸上的部位，直接跳到那一段要素")
                        .size(11.0)
                        .color(theme::text_soft()),
                );
                ui.add_space(4.0);
                for section in FormSection::ALL {
                    let progress = check.progress[section.index()];
                    if progress.total == 0 {
                        continue;
                    }
                    let missing = progress.missing();
                    let (color, text) = if missing == 0 {
                        (theme::success(), format!("{} 齐了", section.label()))
                    } else {
                        (
                            theme::danger(),
                            format!("{} 缺 {missing} 项", section.label()),
                        )
                    };
                    ui.colored_label(color, egui::RichText::new(text).size(11.0));
                }
            });
        });
        ui.add_space(4.0);
    }

    pub(crate) fn form_ui(&mut self, ui: &mut egui::Ui, available_width: f32) {
        // 左侧是输入表单，不使用表格的交替行底色。所有可交互控件统一为白色表面和
        // 相同最小高度，避免下拉框与条纹背景融在一起。
        {
            let style = ui.style_mut();
            style.spacing.interact_size.y = FORM_CONTROL_HEIGHT;
            style.visuals.widgets.inactive.bg_fill = theme::surface();
            style.visuals.widgets.inactive.weak_bg_fill = theme::surface();
        }
        let free_text = self.config.allow_free_text;
        // 单位共用一份名录：发文、主送、抄送、承办、落款都从这里选，一律按层级缩进显示。
        let external_names = self.doc.draft.kind == TemplateKind::OfficialLetter
            && self.doc.draft.profile.correspondence_scope == CorrespondenceScope::External;
        let unit_pool = self.unit_pool(external_names);
        let units = layout_options(&unit_pool, None);
        // 机关代字是单位的属性，候选项就是词库里各单位绑定过的代字。
        let department_codes = plain_options(&units::department_codes(&self.config.vocabulary));
        // 规格 §2.4：一个单位只能占发文、主送、抄送三者之一，勾选时互相禁用。
        let picked_as_recipient = split_units(&self.doc.draft.profile.recipient);
        let picked_as_copy = split_units(&self.doc.draft.profile.copies_to);
        let picked_as_issuing =
            if self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                split_units(&self.doc.draft.profile.joint_issuing_units)
            } else {
                split_units(&self.doc.draft.profile.issuing_unit)
            };
        let blocked_for_recipient = [picked_as_copy.clone(), picked_as_issuing.clone()].concat();
        let blocked_for_copies = [picked_as_recipient.clone(), picked_as_issuing].concat();
        // 规格 §2.3“人随事走”：联系人按承办单位过滤，呈报领导按落款单位过滤。
        let filter_units = match self.doc.draft.kind {
            TemplateKind::OfficialLetter
                if self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1 =>
            {
                split_units(&self.doc.draft.profile.joint_responsible_units)
            }
            TemplateKind::OfficialLetter => vec![self.doc.draft.profile.responsible_unit.clone()],
            TemplateKind::WhitePaper => split_units(&self.doc.draft.profile.signing_unit),
            TemplateKind::RedHeadApproval => split_units(&self.doc.draft.profile.signing_unit),
            _ => vec![],
        };
        let (contacts, people_filtered) = self.filtered_contacts(
            &filter_units,
            self.doc.draft.kind == TemplateKind::OfficialLetter,
            self.doc.draft.kind.is_approval(),
        );
        let people_filter_note = if people_filtered {
            match self.doc.draft.kind {
                TemplateKind::OfficialLetter
                    if self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1 =>
                {
                    Some("联系人已按所选承办单位过滤。".to_string())
                }
                TemplateKind::OfficialLetter => Some(format!(
                    "联系人已按承办单位“{}”过滤。",
                    self.doc.draft.profile.responsible_unit.trim()
                )),
                TemplateKind::WhitePaper => {
                    let units = split_units(&self.doc.draft.profile.signing_unit);
                    if units.is_empty() {
                        None
                    } else {
                        Some(format!(
                            "呈报领导已按落款单位“{}”及其上级领导过滤。",
                            units.join("、")
                        ))
                    }
                }
                TemplateKind::RedHeadApproval => {
                    let units = split_units(&self.doc.draft.profile.signing_unit);
                    if units.is_empty() {
                        None
                    } else {
                        Some(format!(
                            "呈报领导已按落款单位“{}”及其上级领导过滤；承办联系人在各承办条目内按单位过滤。",
                            units.join("、")
                        ))
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        let leader_options = contacts
            .iter()
            .map(|(name, _)| {
                let entry = self.config.vocabulary.iter().find(|entry| {
                    entry.category == VocabularyCategory::Person && entry.canonical.trim() == name
                });
                let position = entry.map(|entry| entry.position.trim()).unwrap_or_default();
                let code = entry.map(|entry| entry.code.trim()).unwrap_or_default();
                let full = if position.is_empty() {
                    name.clone()
                } else {
                    format!("{name} {position}")
                };
                let label = if code.is_empty() {
                    full.clone()
                } else {
                    format!("{code} {full}")
                };
                SelectOption {
                    value: name.clone(),
                    label,
                    full,
                    parent: String::new(),
                    depth: 0,
                }
            })
            .collect::<Vec<_>>();
        let rules = self.config.security_rules.clone();
        let content_width = available_width.clamp(*CONTENT_WIDTH.start(), *CONTENT_WIDTH.end());
        let field_width =
            (content_width - LABEL_WIDTH - FORM_LAYOUT_GUTTER).max(FORM_FIELD_MIN_WIDTH);
        let kind = self.doc.draft.kind;
        let check = check_form(&self.doc.draft);
        let jump = self.doc.form_sections.pending_jump.take();

        // ================= 版头：密级、红头、文号 =================
        let mut open = self.doc.form_sections.open[FormSection::Header.index()];
        let response = section_header_ui(
            ui,
            FormSection::Header,
            kind,
            &mut open,
            check.progress[FormSection::Header.index()],
        );
        if jump == Some(FormSection::Header) {
            response.scroll_to_me(Some(egui::Align::TOP));
        }
        self.doc.form_sections.open[FormSection::Header.index()] = open;
        if open {
            egui::Grid::new("header_grid")
                .num_columns(2)
                .striped(false)
                .min_row_height(FORM_CONTROL_HEIGHT)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    // 密级印在纸的最上面，表单里也放在最上面。密级与保密期限本来
                    // 就是一句话（“机密★20年”），拆成两行只是白白多占一行。
                    let mut level =
                        SecurityLevel::from_marking(&self.doc.draft.profile.security_level);
                    let previous = level;
                    required_row_label(
                        ui,
                        "密级",
                        Some("所有文稿类型必填，默认机密、保密期限20年；不支持“不标注密级”。"),
                    );
                    let half = ((field_width - 6.0) * 0.5).max(84.0);
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("security_level")
                            .selected_text(level.label())
                            .width(half)
                            .show_ui(ui, |ui| {
                                for option in SecurityLevel::ALL {
                                    if option == SecurityLevel::Unmarked {
                                        continue;
                                    }
                                    ui.selectable_value(&mut level, option, option.label());
                                }
                            });
                        if level != previous {
                            self.doc.draft.profile.security_level = level.marking().to_string();
                            self.doc.draft.profile.security_period = rules.default_period(level);
                        }
                        let options = rules.period_options(level);
                        if options.is_empty() {
                            self.doc.draft.profile.security_period.clear();
                        } else {
                            let max_tip = rules.max_years(level).map(|max| {
                                format!(
                                    "{}级公文的保密期限不得超过 {} 年；期限无法确定时选“长期”。",
                                    level.label(),
                                    max
                                )
                            });
                            let period = &mut self.doc.draft.profile.security_period;
                            let combo = egui::ComboBox::from_id_salt("security_period")
                                .selected_text(if period.trim().is_empty() {
                                    "（未选保密期限）"
                                } else {
                                    period.as_str()
                                })
                                .width(half)
                                .show_ui(ui, |ui| {
                                    for option in &options {
                                        if ui
                                            .selectable_label(period.as_str() == option, option)
                                            .clicked()
                                        {
                                            *period = option.clone();
                                        }
                                    }
                                });
                            if let Some(tip) = max_tip {
                                combo.response.on_hover_text(tip);
                            }
                        }
                    });
                    ui.end_row();
                    field_error(ui, &check, "security_level");

                    if let Some(message) =
                        rules.check(level, &self.doc.draft.profile.security_period)
                    {
                        ui.label("");
                        ui.add_sized(
                            [field_width, 20.0],
                            egui::Label::new(egui::RichText::new(message).color(warn()))
                                .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                        ui.end_row();
                    }

                    if kind != TemplateKind::PlainDocument {
                        row_label_with_info(
                            ui,
                            "指人专办",
                            "启用后在密级后空一个全角空格，并以黑体标注“指人专办”；未选择密级时不可用。",
                        );
                        let enabled = level != SecurityLevel::Unmarked;
                        ui.add_enabled(
                            enabled,
                            egui::Checkbox::new(
                                &mut self.doc.draft.profile.special_handling,
                                "启用",
                            ),
                        );
                        ui.end_row();
                    }

                    match kind {
                        TemplateKind::OfficialLetter => {
                            row_label_with_info(
                                ui,
                                "行文范围",
                                "对外行文使用单位规范全称，对内可用惯用简称；它同时决定红头和主送的写法。",
                            );
                            egui::ComboBox::from_id_salt("correspondence_scope")
                                .selected_text(
                                    self.doc.draft.profile.correspondence_scope.label(),
                                )
                                .width(field_width)
                                .show_ui(ui, |ui| {
                                    for scope in CorrespondenceScope::ALL {
                                        ui.selectable_value(
                                            &mut self.doc.draft.profile.correspondence_scope,
                                            scope,
                                            scope.label(),
                                        );
                                    }
                                });
                            ui.end_row();

                            row_label_with_info(
                                ui,
                                "发文方式",
                                "联合发文会把“发文单位”改成多选，并要求指定一个主发文单位用于红头。",
                            );
                            egui::ComboBox::from_id_salt("joint_issuance_mode")
                                .selected_text(self.doc.draft.profile.joint_issuance_mode.label())
                                .width(field_width)
                                .show_ui(ui, |ui| {
                                    for mode in JointIssuanceMode::ALL {
                                        ui.selectable_value(
                                            &mut self.doc.draft.profile.joint_issuance_mode,
                                            mode,
                                            mode.label(),
                                        );
                                    }
                                });
                            ui.end_row();

                            let joint = self.doc.draft.profile.joint_issuance_mode
                                == JointIssuanceMode::Mode1;
                            required_row_label(ui, "发文单位", Some("印在红头上的发文机关标志。"));
                            if joint {
                                multi_select(
                                    ui,
                                    "joint_issuing_units",
                                    &mut self.doc.draft.profile.joint_issuing_units,
                                    &units,
                                    &[],
                                    "",
                                    &mut self.doc.manual_fields,
                                    free_text,
                                    field_width,
                                );
                            } else {
                                let changed = single_select(
                                    ui,
                                    "issuing_unit",
                                    &mut self.doc.draft.profile.issuing_unit,
                                    &units,
                                    &mut self.doc.manual_fields,
                                    free_text,
                                    field_width,
                                    "使用标准全称",
                                );
                                // 规格 §2.1：单位绑定机关代字时自动带出。
                                if changed
                                    && self.doc.draft.profile.department_code.trim().is_empty()
                                {
                                    let code = UnitDisplay::new(&self.config.vocabulary)
                                        .department_code_of(&self.doc.draft.profile.issuing_unit);
                                    if !code.is_empty() {
                                        self.doc.draft.profile.department_code = code;
                                    }
                                }
                            }
                            ui.end_row();
                            field_error(ui, &check, "issuing_unit");
                            field_error(ui, &check, "joint_issuing_units_count");

                            if joint {
                                let selected_issuing =
                                    split_units(&self.doc.draft.profile.joint_issuing_units);
                                if !selected_issuing
                                    .iter()
                                    .any(|unit| unit == &self.doc.draft.profile.main_issuing_unit)
                                {
                                    self.doc.draft.profile.main_issuing_unit =
                                        selected_issuing.first().cloned().unwrap_or_default();
                                }
                                let issuing_preview = UnitDisplay::new(&self.config.vocabulary)
                                    .join_hierarchical_for(&selected_issuing, external_names);
                                required_row_label(
                                    ui,
                                    "主发文\n单位",
                                    Some(&if issuing_preview.is_empty() {
                                        "用于确定红头主单位；选择联合发文单位后可在此指定。"
                                            .to_string()
                                    } else {
                                        format!("用于确定红头主单位；落款将显示为：{issuing_preview}")
                                    }),
                                );
                                single_select(
                                    ui,
                                    "main_issuing_unit",
                                    &mut self.doc.draft.profile.main_issuing_unit,
                                    &layout_options(&unit_pool, Some(&selected_issuing)),
                                    &mut self.doc.manual_fields,
                                    false,
                                    field_width,
                                    "用于红头",
                                );
                                ui.end_row();
                                field_error(ui, &check, "main_issuing_unit");
                            }

                            document_number_row(
                                ui,
                                &mut self.doc.draft.profile,
                                &department_codes,
                                &mut self.doc.manual_fields,
                                free_text,
                                field_width,
                                "例如：X政函",
                                "department_code",
                            );
                        }
                        TemplateKind::PhoneNotice => {
                            required_row_label(
                                ui,
                                "发文单位",
                                Some("电话通知不设置机关代字、发文序号、抄送及承办联系版记。"),
                            );
                            single_select(
                                ui,
                                "phone_notice_issuing_unit",
                                &mut self.doc.draft.profile.issuing_unit,
                                &units,
                                &mut self.doc.manual_fields,
                                free_text,
                                field_width,
                                "使用标准全称",
                            );
                            ui.end_row();
                            field_error(ui, &check, "phone_notice_issuing_unit");
                        }
                        TemplateKind::RedHeadApproval => {
                            required_row_label(
                                ui,
                                "发文单位",
                                Some("用于首页红色发文机关标志；与承办单位、落款单位分别维护。"),
                            );
                            let changed = single_select(
                                ui,
                                "red_approval_issuing_unit",
                                &mut self.doc.draft.profile.issuing_unit,
                                &units,
                                &mut self.doc.manual_fields,
                                free_text,
                                field_width,
                                "使用标准全称",
                            );
                            if changed && self.doc.draft.profile.department_code.trim().is_empty() {
                                let code = UnitDisplay::new(&self.config.vocabulary)
                                    .department_code_of(&self.doc.draft.profile.issuing_unit);
                                if !code.is_empty() {
                                    self.doc.draft.profile.department_code = code;
                                }
                            }
                            ui.end_row();
                            field_error(ui, &check, "red_approval_issuing_unit");

                            document_number_row(
                                ui,
                                &mut self.doc.draft.profile,
                                &department_codes,
                                &mut self.doc.manual_fields,
                                free_text,
                                field_width,
                                "例如：X政呈",
                                "red_approval_department_code",
                            );
                        }
                        TemplateKind::PlainDocument
                        | TemplateKind::MeetingAgenda
                        | TemplateKind::WhitePaper => {
                            ui.label("");
                            ui.weak(match kind {
                                TemplateKind::WhitePaper => "白头件不设红头和文号。",
                                TemplateKind::MeetingAgenda => "会议议程不设红头和文号。",
                                _ => "普通公文不设红头、文号和版记。",
                            });
                            ui.end_row();
                        }
                    }
                });
        }

        ui.add_space(10.0);

        // ================= 主体：主送、标题、正文体例 =================
        let mut open = self.doc.form_sections.open[FormSection::Body.index()];
        let response = section_header_ui(
            ui,
            FormSection::Body,
            kind,
            &mut open,
            check.progress[FormSection::Body.index()],
        );
        if jump == Some(FormSection::Body) {
            response.scroll_to_me(Some(egui::Align::TOP));
        }
        self.doc.form_sections.open[FormSection::Body.index()] = open;
        if open {
            egui::Grid::new("body_grid")
                .num_columns(2)
                .striped(false)
                .min_row_height(FORM_CONTROL_HEIGHT)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    match kind {
                        TemplateKind::OfficialLetter => {
                            let recipient_preview = UnitDisplay::new(&self.config.vocabulary)
                                .join_hierarchical_for(
                                    &split_units(&self.doc.draft.profile.recipient),
                                    external_names,
                                );
                            required_row_label(
                                ui,
                                "主送单位",
                                Some(&if recipient_preview.is_empty() {
                                    "可从标准词库多选；成文时会按单位层级整理名称。".to_string()
                                } else {
                                    format!("成文时将显示为：{recipient_preview}")
                                }),
                            );
                            multi_select(
                                ui,
                                "recipient",
                                &mut self.doc.draft.profile.recipient,
                                &units,
                                &blocked_for_recipient,
                                "已在发文或抄送中选择",
                                &mut self.doc.manual_fields,
                                free_text,
                                field_width,
                            );
                            ui.end_row();
                            field_error(ui, &check, "recipient");
                        }
                        TemplateKind::PhoneNotice => {
                            required_row_label(ui, "主送单位", None);
                            multi_select(
                                ui,
                                "phone_notice_recipient",
                                &mut self.doc.draft.profile.recipient,
                                &units,
                                &[],
                                "",
                                &mut self.doc.manual_fields,
                                free_text,
                                field_width,
                            );
                            ui.end_row();
                            field_error(ui, &check, "phone_notice_recipient");
                        }
                        TemplateKind::MeetingAgenda => {
                            required_row_label(
                                ui,
                                "会议时间",
                                Some("留空时模型将从写作素材中提取，依据不足时标记待核实。"),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut self.doc.draft.meeting_time)
                                    .hint_text("例如：2026年8月5日（星期三）14:30")
                                    .desired_width(field_width),
                            );
                            ui.end_row();
                            field_error(ui, &check, "meeting_time");

                            required_row_label(ui, "会议地点", None);
                            ui.add(
                                egui::TextEdit::singleline(
                                    &mut self.doc.draft.profile.meeting_location,
                                )
                                .hint_text("会议室或线上会议")
                                .desired_width(field_width),
                            );
                            ui.end_row();
                            field_error(ui, &check, "meeting_location");

                            required_row_label(ui, "参加人员", None);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.doc.draft.attendees)
                                    .hint_text("人员/单位及范围")
                                    .desired_width(field_width),
                            );
                            ui.end_row();
                            field_error(ui, &check, "attendees");
                        }
                        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => {
                            let leaders_display = UnitDisplay::new(&self.config.vocabulary)
                                .reporting_leaders(&self.doc.draft.profile.reporting_leaders);
                            let mut leader_tip = people_filter_note.clone().unwrap_or_else(|| {
                                "呈报领导从标准词库中的人员条目选择，可为落款单位的领导或其上级领导。"
                                    .to_string()
                            });
                            if !leaders_display.is_empty() {
                                leader_tip.push_str(&format!(" 成文称谓：{leaders_display}"));
                            }
                            let id = if kind == TemplateKind::WhitePaper {
                                "reporting_leaders"
                            } else {
                                "red_approval_reporting_leaders"
                            };
                            required_row_label(ui, "呈报领导", Some(&leader_tip));
                            multi_select(
                                ui,
                                id,
                                &mut self.doc.draft.profile.reporting_leaders,
                                &leader_options,
                                &[],
                                "",
                                &mut self.doc.manual_fields,
                                free_text,
                                field_width,
                            );
                            ui.end_row();
                            field_error(ui, &check, id);
                        }
                        TemplateKind::PlainDocument => {}
                    }

                    row_label_with_info(
                        ui,
                        "标题提示",
                        "留空则由模型拟题；正文里写了“# 标题”时以正文为准。",
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.doc.draft.title_hint)
                            .hint_text("可留空，由模型拟题")
                            .desired_width(field_width),
                    );
                    ui.end_row();

                    row_label_with_info(
                        ui,
                        "排版风格",
                        "紧缩风格会把正文中层级最深的标题与紧随其后的正文合并为一行，例如：一、任务目标。测试正文。",
                    );
                    egui::ComboBox::from_id_salt("style_mode")
                        .selected_text(self.doc.draft.profile.style_mode.label())
                        .width(field_width)
                        .show_ui(ui, |ui| {
                            for mode in StyleMode::ALL {
                                ui.selectable_value(
                                    &mut self.doc.draft.profile.style_mode,
                                    mode,
                                    mode.label(),
                                );
                            }
                        });
                    ui.end_row();
                });
        }

        ui.add_space(10.0);

        // ================= 版记：抄送、承办、落款、成文日期 =================
        let mut open = self.doc.form_sections.open[FormSection::Record.index()];
        let response = section_header_ui(
            ui,
            FormSection::Record,
            kind,
            &mut open,
            check.progress[FormSection::Record.index()],
        );
        if jump == Some(FormSection::Record) {
            response.scroll_to_me(Some(egui::Align::TOP));
        }
        self.doc.form_sections.open[FormSection::Record.index()] = open;
        if open {
            egui::Grid::new("record_grid")
                .num_columns(2)
                .striped(false)
                .min_row_height(FORM_CONTROL_HEIGHT)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    match kind {
                        TemplateKind::OfficialLetter => {
                            let copies_preview = UnitDisplay::new(&self.config.vocabulary)
                                .join_hierarchical_for(
                                    &split_units(&self.doc.draft.profile.copies_to),
                                    external_names,
                                );
                            row_label_with_info(
                                ui,
                                "抄送单位",
                                if copies_preview.is_empty() {
                                    "可从标准词库多选；已作为发文或主送单位的名称不能重复选择。"
                                        .to_string()
                                } else {
                                    format!("成文时将显示为：{copies_preview}")
                                },
                            );
                            multi_select(
                                ui,
                                "copies_to",
                                &mut self.doc.draft.profile.copies_to,
                                &units,
                                &blocked_for_copies,
                                "已在发文或主送中选择",
                                &mut self.doc.manual_fields,
                                free_text,
                                field_width,
                            );
                            ui.end_row();

                            row_label(ui, "承办单位");
                            if self.doc.draft.profile.joint_issuance_mode
                                == JointIssuanceMode::Mode1
                            {
                                // 联合发文：承办单位与联系人成对录入，数量天然一致、一一对应。
                                joint_responsible_editor(
                                    ui,
                                    &mut self.doc.draft.profile,
                                    &units,
                                    &self.config.vocabulary,
                                    &mut self.doc.manual_fields,
                                    free_text,
                                    field_width,
                                    TemplateKind::OfficialLetter.max_responsible_entries(),
                                );
                            } else {
                                single_select(
                                    ui,
                                    "responsible_unit",
                                    &mut self.doc.draft.profile.responsible_unit,
                                    &units,
                                    &mut self.doc.manual_fields,
                                    free_text,
                                    field_width,
                                    "处室/部门标准名称",
                                );
                            }
                            ui.end_row();

                            if self.doc.draft.profile.joint_issuance_mode
                                != JointIssuanceMode::Mode1
                            {
                                let contact_tip = people_filter_note.clone().unwrap_or_else(|| {
                                    "联系人从标准词库选择，并自动带出与姓名绑定的电话。".to_string()
                                });
                                row_label_with_info(ui, "联系人\n及电话", contact_tip);
                                contact_pair(
                                    ui,
                                    &mut self.doc.draft.profile.contact_person,
                                    &mut self.doc.draft.profile.contact_phone,
                                    &contacts,
                                    &mut self.doc.manual_fields,
                                    free_text,
                                    field_width,
                                );
                                ui.end_row();
                            }
                        }
                        TemplateKind::RedHeadApproval => {
                            row_label_with_info(
                                ui,
                                "承办信息",
                                "每行一组承办单位、联系人和电话，最多 4 条；条目增加时首页底栏向上扩展。单位名放不下时自动横向压缩，不换行。",
                            );
                            joint_responsible_editor(
                                ui,
                                &mut self.doc.draft.profile,
                                &units,
                                &self.config.vocabulary,
                                &mut self.doc.manual_fields,
                                free_text,
                                field_width,
                                TemplateKind::RedHeadApproval.max_responsible_entries(),
                            );
                            ui.end_row();
                            field_error(ui, &check, "red_approval_responsible");

                            required_row_label(
                                ui,
                                "落款单位",
                                Some("与发文单位、承办单位相互独立；落款固定从第二页开始。"),
                            );
                            multi_select(
                                ui,
                                "red_approval_signing_unit",
                                &mut self.doc.draft.profile.signing_unit,
                                &units,
                                &[],
                                "",
                                &mut self.doc.manual_fields,
                                free_text,
                                field_width,
                            );
                            ui.end_row();
                            field_error(ui, &check, "red_approval_signing_unit");

                            row_label_with_info(
                                ui,
                                "落款名称",
                                "可按白头件规则使用单位简称；未维护简称时回落规范名称。",
                            );
                            ui.checkbox(
                                &mut self.doc.draft.profile.use_short_name_for_signature,
                                "使用简称",
                            );
                            ui.end_row();
                        }
                        TemplateKind::WhitePaper => {
                            required_row_label(
                                ui,
                                "落款单位",
                                Some("支持多单位：成文时各单位自上而下分行、行间空一行，便于分别签字。"),
                            );
                            multi_select(
                                ui,
                                "signing_unit",
                                &mut self.doc.draft.profile.signing_unit,
                                &units,
                                &[],
                                "",
                                &mut self.doc.manual_fields,
                                free_text,
                                field_width,
                            );
                            ui.end_row();
                            field_error(ui, &check, "signing_unit");

                            row_label_with_info(
                                ui,
                                "落款名称",
                                "勾选“使用简称”后落款单位显示词库简称（未维护简称的单位回落规范名称）；\
                                 显示文本少于 5 字时分散对齐到 5 字宽，便于签字。",
                            );
                            ui.horizontal(|ui| {
                                ui.checkbox(
                                    &mut self.doc.draft.profile.use_short_name_for_signature,
                                    "使用简称",
                                );
                                let display = UnitDisplay::new(&self.config.vocabulary);
                                let example = split_units(&self.doc.draft.profile.signing_unit)
                                    .into_iter()
                                    .filter(|unit| !unit.trim().is_empty())
                                    .map(|unit| {
                                        display.signature_name(
                                            &unit,
                                            self.doc.draft.profile.use_short_name_for_signature,
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join("、");
                                if !example.is_empty() {
                                    ui.weak(format!("落款将显示为：{example}"));
                                }
                            });
                            ui.end_row();
                        }
                        TemplateKind::PhoneNotice => {
                            ui.label("");
                            ui.weak("电话通知不设抄送、承办联系版记。");
                            ui.end_row();
                        }
                        TemplateKind::MeetingAgenda | TemplateKind::PlainDocument => {}
                    }

                    if kind != TemplateKind::PlainDocument {
                        row_label(ui, "成文日期");
                        ui.horizontal(|ui| {
                            let enabled = !self.doc.draft.date_is_auto;
                            ui.add_enabled(
                                enabled,
                                egui::TextEdit::singleline(&mut self.doc.draft.date)
                                    .hint_text("例如：2026年8月5日")
                                    .desired_width((field_width - 96.0).max(110.0)),
                            );
                            ui.checkbox(&mut self.doc.draft.date_is_auto, "本机日期")
                                .on_hover_text("勾选后每次生成都按本机当天日期刷新");
                        });
                        ui.end_row();
                    }

                    if kind.uses_letter_layout() {
                        row_label(ui, "打印方式");
                        ui.checkbox(&mut self.doc.draft.profile.duplex_printing, "双面打印")
                            .on_hover_text(
                                "勾选后页码按装订外侧排布：奇数页靠右、偶数页靠左；不勾选时页码居中",
                            );
                        ui.end_row();
                    }
                });
        }

        if kind == TemplateKind::MeetingAgenda {
            ui.add_space(6.0);
            ui.collapsing("查看会议议程固定 Markdown 格式", |ui| {
                ui.monospace(prompt::MEETING_AGENDA_MARKDOWN_TEMPLATE);
            });
        }
        ui.add_space(8.0);
    }
}
