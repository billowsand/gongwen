//! AI 管理页「数据接口」分区（`docs/http-skill-design.md` 第一层）。
//!
//! 左右两栏：
//! - 左栏是接口列表，按服务器分组——同一台服务器上的接口通常共用一把 Key，组标题上标出
//!   Key 的状态，点组标题进「服务与密钥」（见 [`service`]）。密钥挂在服务器下面，不再有
//!   一张独立的密钥表；
//! - 右栏是选中项：接口详情（见 [`detail`]）顶上是名称、地址与密钥绑定条，下面分「接口说明」
//!   「试一下」「技术配置」三页；「从文档添加」（见 [`import`]）也在右栏里做。
//!
//! 实测在后台线程里跑，界面每帧取一次结果；测试结论记进 `api-tests.json`。

mod assist;
mod detail;
mod forms;
mod import;
mod service;

use super::agent_settings::code_block;
use crate::agent::api::{
    self, ApiEndpoint, ApiMethod, ApiSecrets, ApiStore, ApiTestLog, TestRecord, TestStatus, Trial,
};
use crate::agent::api_import::auth::{AuthPlace, AuthSpec};
use crate::agent::api_import::redact;
use crate::agent::apidef;
use crate::app::GongwenApp;
use crate::models::AppConfig;
use crate::theme;
use detail::Detail;
use eframe::egui;
use import::ImportFlow;
use serde_json::{Map, Value};
use service::ServiceState;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

/// 原始返回最多显示这么多字，再长的界面上看不完，也拖慢界面。
const RAW_PREVIEW_CHARS: usize = 4000;
/// 测试结果里预览几条。
const PREVIEW_ITEMS: usize = 5;
/// 左栏列表宽度的上下限。
const LIST_MIN_WIDTH: f32 = 230.0;
const LIST_MAX_WIDTH: f32 = 300.0;
/// 左栏底部状态条的高度（未保存提示、图例）。
const LIST_FOOTER_HEIGHT: f32 = 58.0;

/// 数据接口分区的状态。接口与密钥要点保存才落盘；测试记录随测随存。
#[derive(Default)]
pub(crate) struct ApisPage {
    loaded: bool,
    store: ApiStore,
    secrets: ApiSecrets,
    log: ApiTestLog,
    dirty: bool,
    view: View,
    message: Option<(bool, String)>,
    /// 左栏的搜索词。
    search: String,
    detail: Detail,
    service: ServiceState,
    import: Option<ImportFlow>,
    /// 后台在跑的批量实测（换了 Key 以后共用它的接口、服务页「全部重测」）：接口 id → 结果。
    batch: Vec<(String, Receiver<Trial>)>,
}

/// 右栏显示什么。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
enum View {
    /// 什么都没选（还没有接口时显示上手说明）。
    #[default]
    None,
    Endpoint(usize),
    /// 一台服务器（`http://host:port`）：它的密钥与上面的接口。
    Service(String),
    Import,
}

/// 「添加鉴权」的小表单：密钥放请求头还是地址参数、叫什么、要不要 Bearer。
pub(super) struct AuthForm {
    place: AuthPlace,
    name: String,
    bearer: bool,
}

impl Default for AuthForm {
    fn default() -> Self {
        Self {
            place: AuthPlace::Header,
            name: "Authorization".into(),
            bearer: true,
        }
    }
}

/// 画「添加鉴权」表单；点了「加上」返回鉴权方式。
pub(super) fn auth_form_ui(ui: &mut egui::Ui, form: &mut AuthForm) -> Option<AuthSpec> {
    let mut added = None;
    ui.horizontal_wrapped(|ui| {
        ui.label("密钥放在");
        ui.selectable_value(&mut form.place, AuthPlace::Header, "请求头");
        ui.selectable_value(&mut form.place, AuthPlace::Query, "地址参数");
        ui.label("名字");
        ui.add(
            egui::TextEdit::singleline(&mut form.name)
                .hint_text("Authorization / X-API-Key / token")
                .desired_width(150.0),
        );
        if form.place == AuthPlace::Header {
            ui.checkbox(&mut form.bearer, "值前加 Bearer");
        }
        let spec = AuthSpec {
            place: form.place,
            name: form.name.trim().to_string(),
            bearer: form.bearer && form.place == AuthPlace::Header,
            basis: "手动添加".into(),
            value: String::new(),
        };
        if ui
            .add_enabled(spec.valid_name(), egui::Button::new("加上"))
            .on_disabled_hover_text(
                "名字用英文字母开头，只能有字母、数字、下划线（请求头还可以有短横线）",
            )
            .clicked()
        {
            added = Some(spec);
        }
    });
    added
}

/// 新加的鉴权用哪个密钥名：按字段名起，已有同名的沿用（同一个服务通常共用一个 Key）。
pub(super) fn secret_for(spec: &AuthSpec, secrets: &mut ApiSecrets) -> String {
    let name = redact::secret_name(&spec.name);
    secrets.secrets.entry(name.clone()).or_default();
    name
}

/// `http://10.0.0.9:8080/api/x` → `http://10.0.0.9:8080`。
pub(super) fn host_of(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split(['/', '?']).next().unwrap_or_default();
            format!("{scheme}://{host}").to_ascii_lowercase()
        }
        None => String::new(),
    }
}

/// 地址去掉服务器部分：`http://10.0.0.9/api/x?a=1` → `/api/x?a=1`。
fn path_of(url: &str) -> &str {
    match url.split_once("://") {
        Some((_, rest)) => rest.find(['/', '?']).map_or("/", |at| &rest[at..]),
        None => url,
    }
}

/// 密钥输入框：打码，提示把 Key 粘进来。
pub(super) fn key_field(ui: &mut egui::Ui, value: &mut String) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .password(true)
            .hint_text("把 Key 粘到这里")
            .desired_width(260.0),
    )
}

impl ApisPage {
    fn load(&mut self) {
        let mut errors = Vec::new();
        match ApiStore::load() {
            Ok(store) => self.store = store,
            Err(error) => errors.push(format!("{error:#}")),
        }
        match ApiSecrets::load() {
            Ok(secrets) => self.secrets = secrets,
            Err(error) => errors.push(format!("{error:#}")),
        }
        self.log = ApiTestLog::load().unwrap_or_default();
        if !errors.is_empty() {
            self.message = Some((false, errors.join("；")));
        }
        self.loaded = true;
        if !self.store.endpoints.is_empty() {
            self.open(0);
        }
    }

    fn save(&mut self) -> anyhow::Result<()> {
        self.store.save()?;
        self.secrets.save()?;
        self.dirty = false;
        Ok(())
    }

    /// 打开一个接口：默认先看「接口说明」。
    fn open(&mut self, index: usize) {
        self.detail = Detail::default();
        if let Some(endpoint) = self.store.endpoints.get(index) {
            detail::preselect(&mut self.detail, endpoint);
        }
        self.view = View::Endpoint(index);
    }

    fn open_service(&mut self, host: String) {
        self.service = ServiceState::default();
        self.view = View::Service(host);
    }

    /// 记一次测试结论并落盘（测试记录不等「保存」）。
    fn record(&mut self, endpoint: &ApiEndpoint, trial: &Trial) {
        self.log.record(endpoint, trial);
        if let Err(error) = self.log.save() {
            self.message = Some((false, format!("测试记录保存失败：{error:#}")));
        }
    }

    /// 用样例值在后台把这几个接口各测一次（已经在测的不重复发）。
    fn retest(&mut self, indices: &[usize]) {
        for &index in indices {
            let Some(endpoint) = self.store.endpoints.get(index) else {
                continue;
            };
            if self.batch.iter().any(|(id, _)| *id == endpoint.id) {
                continue;
            }
            let rx = spawn_trial(
                endpoint.clone(),
                endpoint.trial_args(),
                self.secrets.clone(),
            );
            self.batch.push((endpoint.id.clone(), rx));
        }
    }

    /// 这个接口正在批量重测。
    fn retesting(&self, endpoint: &ApiEndpoint) -> bool {
        self.batch.iter().any(|(id, _)| *id == endpoint.id)
    }

    /// 取回批量实测的结果，记进测试记录；正开着的接口顺带显示在「试一下」里。
    fn poll_batch(&mut self, ctx: &egui::Context) {
        let mut done = Vec::new();
        self.batch.retain(|(id, rx)| match rx.try_recv() {
            Ok(trial) => {
                done.push((id.clone(), trial));
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        });
        for (id, trial) in done {
            let Some(index) = self.store.endpoints.iter().position(|e| e.id == id) else {
                continue;
            };
            let endpoint = self.store.endpoints[index].clone();
            self.record(&endpoint, &trial);
            if self.view == View::Endpoint(index) && self.detail.test.is_none() {
                self.detail.result = Some(trial);
            }
        }
        if !self.batch.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// 在用这个密钥的接口。
    fn users_of(&self, secret: &str) -> Vec<usize> {
        self.store
            .endpoints
            .iter()
            .enumerate()
            .filter(|(_, e)| e.secret_names().iter().any(|n| n == secret))
            .map(|(i, _)| i)
            .collect()
    }

    /// 没有接口在用的密钥。
    fn orphan_secrets(&self) -> Vec<String> {
        self.secrets
            .secrets
            .keys()
            .filter(|name| self.users_of(name).is_empty())
            .cloned()
            .collect()
    }

    /// 新建一个空接口并打开它的说明编辑。
    fn add_blank(&mut self) {
        let n = self.store.endpoints.len() + 1;
        let mut id = format!("api{n}");
        let mut k = n;
        while self.store.get(&id).is_some() {
            k += 1;
            id = format!("api{k}");
        }
        self.store.endpoints.push(ApiEndpoint {
            id,
            name: format!("新接口 {n}"),
            url: "http://".into(),
            ..ApiEndpoint::default()
        });
        self.dirty = true;
        let index = self.store.endpoints.len() - 1;
        self.open(index);
        self.detail.start_editing(&self.store.endpoints[index]);
    }

    fn start_import(&mut self) {
        self.import.get_or_insert_with(ImportFlow::default);
        self.view = View::Import;
        self.message = None;
    }
}

impl GongwenApp {
    /// 底部「保存」顺带保存数据接口页没保存的修改。
    pub(super) fn save_pending_apis(&mut self) {
        let page = &mut self.agent_settings.apis;
        if !page.dirty {
            return;
        }
        match page.save() {
            Ok(()) => page.message = Some((true, "已随设置一并保存。".into())),
            Err(error) => self.status = format!("数据接口保存失败：{error:#}"),
        }
    }

    pub(super) fn apis_section_ui(&mut self, ui: &mut egui::Ui) {
        section_ui(ui, &mut self.agent_settings.apis, &self.config);
    }
}

/// 整个分区：左栏列表、右栏当前项，两栏各自滚动。
fn section_ui(ui: &mut egui::Ui, page: &mut ApisPage, config: &AppConfig) {
    if !page.loaded {
        page.load();
    }
    page.poll_batch(ui.ctx());
    let height = ui.available_height().max(320.0);
    let width = ui.available_width();
    let list_width = (width * 0.28).clamp(LIST_MIN_WIDTH, LIST_MAX_WIDTH);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(list_width, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(list_width);
                ui.set_height(height);
                list_column(ui, page);
            },
        );
        theme::divider_v(ui, height);
        ui.add_space(14.0);
        let detail_width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(detail_width, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(detail_width);
                ui.set_height(height);
                match page.view.clone() {
                    View::Endpoint(index) if index < page.store.endpoints.len() => {
                        detail::endpoint_ui(ui, page, index, config);
                    }
                    View::Service(host) => service::service_ui(ui, page, &host),
                    View::Import => import::import_ui(ui, page, config),
                    View::Endpoint(_) | View::None => {
                        page.view = View::None;
                        welcome_ui(ui, page);
                    }
                }
            },
        );
    });
}

// —— 左栏 ——

/// 同一台服务器上的接口。
struct Group {
    host: String,
    members: Vec<usize>,
}

/// 按服务器分组，组按第一次出现的先后排。
fn groups(store: &ApiStore, filter: &str) -> Vec<Group> {
    let filter = filter.trim().to_lowercase();
    let mut groups: Vec<Group> = Vec::new();
    for (index, endpoint) in store.endpoints.iter().enumerate() {
        if !filter.is_empty() {
            let hay = format!(
                "{} {} {} {}",
                endpoint.name, endpoint.description, endpoint.url, endpoint.id
            )
            .to_lowercase();
            if !hay.contains(&filter) {
                continue;
            }
        }
        let host = host_of(&endpoint.url);
        match groups.iter_mut().find(|g| g.host == host) {
            Some(group) => group.members.push(index),
            None => groups.push(Group {
                host,
                members: vec![index],
            }),
        }
    }
    groups
}

fn list_column(ui: &mut egui::Ui, page: &mut ApisPage) {
    list_toolbar(ui, page);
    if let Some((ok, text)) = &page.message {
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .size(theme::font_sizes::SMALL)
                    .color(if *ok {
                        theme::success()
                    } else {
                        theme::danger()
                    }),
            )
            .wrap(),
        );
    }
    ui.add_space(6.0);
    let scroll_height = (ui.available_height() - LIST_FOOTER_HEIGHT).max(120.0);
    let mut open = None;
    let mut open_service = None;
    egui::ScrollArea::vertical()
        .id_salt("api_list_scroll")
        .max_height(scroll_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if page.store.endpoints.is_empty() {
                theme::caption(ui, "还没有接口。点上面的「添加」。");
                return;
            }
            let groups = groups(&page.store, &page.search);
            if groups.is_empty() {
                theme::caption(ui, "没有匹配的接口。");
            }
            for group in &groups {
                let selected = page.view == View::Service(group.host.clone());
                if group_header(ui, page, group, selected).clicked() && !group.host.is_empty() {
                    open_service = Some(group.host.clone());
                }
                for &index in &group.members {
                    let selected = page.view == View::Endpoint(index);
                    if endpoint_row(ui, page, index, selected).clicked() {
                        open = Some(index);
                    }
                }
                ui.add_space(8.0);
            }
        });
    if let Some(index) = open {
        page.open(index);
    }
    if let Some(host) = open_service {
        page.open_service(host);
    }
    list_footer(ui, page);
}

/// 搜索框、「添加」菜单、导入导出菜单。
fn list_toolbar(ui: &mut egui::Ui, page: &mut ApisPage) {
    ui.horizontal(|ui| {
        let search_width = (ui.available_width() - 112.0).max(80.0);
        ui.add(theme::field(&mut page.search, "搜索接口", search_width));
        let add = theme::primary_icon_button(ui, theme::Icon::Plus, "添加");
        egui::Popup::menu(&add).show(|ui| {
            if ui
                .add(theme::menu_item(theme::Icon::WandSparkles, "从文档识别…"))
                .on_hover_text("粘贴接口文档、cURL 或选文件，程序自动填好并试调")
                .clicked()
            {
                page.start_import();
            }
            if ui
                .add(theme::menu_item(theme::Icon::Edit, "手动填写"))
                .clicked()
            {
                page.add_blank();
            }
        });
        let more = ui
            .add(
                egui::Button::image(theme::Icon::Menu.image_sized(16.0))
                    .image_tint_follows_text_color(true)
                    .frame_when_inactive(false),
            )
            .on_hover_text("导入 / 导出接口配置");
        egui::Popup::menu(&more).show(|ui| {
            if ui
                .add(theme::menu_item(theme::Icon::FileUp, "导入接口文件…"))
                .on_hover_text(
                    "OpenAPI / Swagger（JSON、YAML）、Postman 集合（可同时选环境文件）、cURL、                     以前导出的接口配置；可以一次选几个文件",
                )
                .clicked()
            {
                import_config(page);
            }
            if ui
                .add(theme::menu_item(theme::Icon::FileDown, "导出为 OpenAPI…"))
                .on_hover_text("每个服务一份 OpenAPI 3.0，能在 Postman、Apifox 里打开；不带密钥与本机环境地址")
                .clicked()
            {
                export_config(page);
            }
        });
    });
}

fn import_config(page: &mut ApisPage) {
    let Some(paths) = rfd::FileDialog::new()
        .add_filter("接口文件", &["json", "yaml", "yml", "txt", "sh"])
        .pick_files()
    else {
        return;
    };
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read_to_string(&path) {
            Ok(text) => files.push((name, text)),
            Err(error) => errors.push(format!("{name}：读不了（{error}）")),
        }
    }
    let first_new = page.store.endpoints.len();
    let outcome = apidef::import::import_files(&mut page.store, &files);
    for (name, value) in outcome.secrets {
        let slot = page.secrets.secrets.entry(name).or_default();
        if slot.trim().is_empty() {
            *slot = value;
        }
    }
    let mut lines = errors;
    lines.extend(outcome.report);
    if outcome.added > 0 {
        page.dirty = true;
        lines.insert(0, format!("导入了 {} 个接口，记得保存。", outcome.added));
        page.open(first_new);
    }
    page.message = Some((
        outcome.added > 0,
        lines.join(
            "
",
        ),
    ));
}

fn export_config(page: &mut ApisPage) {
    let files = apidef::export(&page.store);
    if files.is_empty() {
        page.message = Some((false, "还没有接口可以导出。".into()));
        return;
    }
    let written = if let [(name, doc)] = files.as_slice() {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(name)
            .add_filter("OpenAPI（JSON）", &["json"])
            .add_filter("OpenAPI（YAML）", &["yaml", "yml"])
            .save_file()
        else {
            return;
        };
        write_doc(&path, doc).map(|()| path.display().to_string())
    } else {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        files
            .iter()
            .try_for_each(|(name, doc)| write_doc(&dir.join(name), doc))
            .map(|()| format!("{}（{} 个服务）", dir.display(), files.len()))
    };
    page.message = Some(match written {
        Ok(place) => (true, format!("已导出到 {place}（不含密钥）")),
        Err(error) => (false, format!("导出失败：{error:#}")),
    });
}

/// 按扩展名写 JSON 或 YAML。
fn write_doc(path: &std::path::Path, doc: &Value) -> anyhow::Result<()> {
    let yaml = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
    std::fs::write(path, apidef::to_text(doc, yaml)?)?;
    Ok(())
}

/// 一组接口的 Key 状态。
enum KeyState {
    /// 都不带密钥。
    None,
    Missing,
    Rejected,
    Ok(Vec<String>),
}

fn key_state(page: &ApisPage, members: &[usize]) -> KeyState {
    let mut names: Vec<String> = Vec::new();
    let mut missing = false;
    let mut rejected = false;
    for &index in members {
        let endpoint = &page.store.endpoints[index];
        for name in endpoint.secret_names() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        missing |= !page.secrets.missing(endpoint).is_empty();
        rejected |= matches!(page.log.status(endpoint), TestStatus::Failed(r) if r.auth);
    }
    if names.is_empty() {
        KeyState::None
    } else if missing {
        KeyState::Missing
    } else if rejected {
        KeyState::Rejected
    } else {
        KeyState::Ok(names)
    }
}

/// Key 状态的小标签：组标题、服务页标题用。
fn key_chip(ui: &mut egui::Ui, state: &KeyState) -> egui::Response {
    match state {
        KeyState::None => small_chip(ui, None, "无需鉴权", theme::text_muted(), None),
        KeyState::Missing => small_chip(
            ui,
            Some(theme::Icon::Shield),
            "缺 Key",
            theme::warn(),
            Some(theme::warn_soft()),
        ),
        KeyState::Rejected => small_chip(
            ui,
            Some(theme::Icon::Shield),
            "Key 被拒",
            theme::danger(),
            Some(theme::danger_soft()),
        ),
        KeyState::Ok(names) => {
            let text = match names.len() {
                1 => names[0].clone(),
                n => format!("{} 等 {n} 个", names[0]),
            };
            small_chip(
                ui,
                Some(theme::Icon::Shield),
                &text,
                theme::success(),
                Some(theme::success_soft()),
            )
        }
    }
}

/// 小号胶囊标签，可带图标；`bg` 为空时只有文字。
fn small_chip(
    ui: &mut egui::Ui,
    icon: Option<theme::Icon>,
    text: &str,
    fg: egui::Color32,
    bg: Option<egui::Color32>,
) -> egui::Response {
    egui::Frame::new()
        .fill(bg.unwrap_or(egui::Color32::TRANSPARENT))
        .corner_radius(egui::CornerRadius::same(255))
        .inner_margin(egui::Margin::symmetric(7, 1))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.horizontal(|ui| {
                // 放在右对齐的行里时 horizontal 也从右往左排：图标要后加才落在左边。
                let reversed = ui.layout().prefer_right_to_left();
                let label = egui::RichText::new(text)
                    .size(theme::font_sizes::SMALL)
                    .color(fg);
                if reversed {
                    ui.label(label.clone());
                }
                if let Some(icon) = icon {
                    ui.add(icon.image_sized(11.0).tint(fg));
                }
                if !reversed {
                    ui.label(label);
                }
            });
        })
        .response
}

/// 列表里可点的一行：整行是点击区，选中时铺强调色淡底。
fn list_row<R>(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    selected: bool,
    margin: egui::Margin,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::Response {
    let frame = egui::Frame::new()
        .fill(if selected {
            theme::accent_soft()
        } else {
            egui::Color32::TRANSPARENT
        })
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(margin);
    theme::clickable_card(ui, id_salt, frame, selected, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    })
    .response
}

fn group_header(
    ui: &mut egui::Ui,
    page: &ApisPage,
    group: &Group,
    selected: bool,
) -> egui::Response {
    let state = key_state(page, &group.members);
    list_row(
        ui,
        ("api_group", &group.host),
        selected,
        egui::Margin::symmetric(8, 4),
        |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    theme::Icon::Globe
                        .image_sized(12.0)
                        .tint(theme::text_muted()),
                );
                let host = if group.host.is_empty() {
                    "未填地址".to_string()
                } else {
                    group
                        .host
                        .split_once("://")
                        .map_or(group.host.clone(), |(_, h)| h.to_string())
                };
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(host)
                            .monospace()
                            .size(theme::font_sizes::SMALL)
                            .color(theme::text_soft()),
                    )
                    .truncate(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    key_chip(ui, &state);
                });
            });
        },
    )
    .on_hover_text("这台服务器上的接口共用的密钥：点开填写、更换、全部重测")
}

fn endpoint_row(
    ui: &mut egui::Ui,
    page: &ApisPage,
    index: usize,
    selected: bool,
) -> egui::Response {
    let endpoint = &page.store.endpoints[index];
    let health = Health::of(page, endpoint);
    list_row(
        ui,
        ("api_row", index),
        selected,
        egui::Margin::symmetric(10, 6),
        |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.add_space(6.0);
                    theme::dot(ui, health.dot());
                });
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(egui::Label::new(&endpoint.name).truncate());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        method_tag(ui, endpoint.method);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(path_of(&endpoint.url))
                                    .monospace()
                                    .size(theme::font_sizes::SMALL)
                                    .color(theme::text_muted()),
                            )
                            .truncate(),
                        );
                    });
                });
            });
        },
    )
    .on_hover_text(health.text())
}

/// 请求方法的小标签：GET 蓝绿、POST 琥珀、改动类（PUT / PATCH）强调色、DELETE 红，扫一眼分得开。
fn method_tag(ui: &mut egui::Ui, method: ApiMethod) {
    let (fg, bg) = match method {
        ApiMethod::Get | ApiMethod::Head => (theme::info(), theme::surface_sunk()),
        ApiMethod::Post => (theme::warn(), theme::warn_soft()),
        ApiMethod::Put | ApiMethod::Patch => (theme::accent(), theme::accent_soft()),
        ApiMethod::Delete => (theme::danger(), theme::danger_soft()),
    };
    egui::Frame::new()
        .fill(bg)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(5, 0))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(method.label())
                    .monospace()
                    .size(11.0)
                    .strong()
                    .color(fg),
            );
        });
}

/// 左栏底部：没保存的修改、没接口在用的密钥、状态点图例。
fn list_footer(ui: &mut egui::Ui, page: &mut ApisPage) {
    ui.add_space(4.0);
    theme::hairline(ui);
    ui.add_space(4.0);
    if page.dirty {
        ui.horizontal(|ui| {
            ui.colored_label(theme::warn(), "有未保存的修改");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_icon_button(ui, theme::Icon::Save, "保存").clicked() {
                    page.message = Some(match page.save() {
                        Ok(()) => (true, "已保存，下次发送时生效。".into()),
                        Err(error) => (false, format!("保存失败：{error:#}")),
                    });
                }
            });
        });
        return;
    }
    let orphans = page.orphan_secrets();
    if !orphans.is_empty() {
        ui.horizontal(|ui| {
            theme::caption(ui, &format!("{} 个密钥没有接口在用", orphans.len()))
                .on_hover_text(format!("「{}」", orphans.join("」「")));
            if ui.small_button("清理").clicked() {
                for name in &orphans {
                    page.secrets.secrets.remove(name);
                }
                page.dirty = true;
            }
        });
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (color, label) in [
            (theme::success(), "调通"),
            (theme::danger(), "失败"),
            (theme::warn(), "要处理"),
            (theme::border_strong(), "未测"),
        ] {
            theme::dot(ui, color);
            theme::caption(ui, label);
            ui.add_space(6.0);
        }
    });
}

/// 一个接口眼下的状况：列表的状态点、详情标题旁的标签用同一套判断。
enum Health<'a> {
    Problems(usize),
    MissingKey(Vec<String>),
    AuthFailed(&'a TestRecord),
    Untested,
    Passed(&'a TestRecord),
    Failed(&'a TestRecord),
    Stale(&'a TestRecord),
}

impl<'a> Health<'a> {
    fn of(page: &'a ApisPage, endpoint: &ApiEndpoint) -> Self {
        let problems = endpoint.problems().len();
        if problems > 0 {
            return Self::Problems(problems);
        }
        let missing = page.secrets.missing(endpoint);
        if !missing.is_empty() {
            return Self::MissingKey(missing);
        }
        match page.log.status(endpoint) {
            TestStatus::Failed(record) if record.auth => Self::AuthFailed(record),
            TestStatus::Failed(record) => Self::Failed(record),
            TestStatus::Passed(record) => Self::Passed(record),
            TestStatus::Stale(record) => Self::Stale(record),
            TestStatus::Untested => Self::Untested,
        }
    }

    fn dot(&self) -> egui::Color32 {
        match self {
            Self::Passed(_) => theme::success(),
            Self::Failed(_) | Self::AuthFailed(_) | Self::Problems(_) => theme::danger(),
            Self::MissingKey(_) | Self::Stale(_) => theme::warn(),
            Self::Untested => theme::border_strong(),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Problems(n) => format!("配置有 {n} 个问题"),
            Self::MissingKey(_) => "缺密钥".into(),
            Self::AuthFailed(record) => format!("鉴权没过 · {}", record.at),
            Self::Untested => "未测试".into(),
            Self::Passed(record) => format!("测试通过 · {}", record.at),
            Self::Failed(record) => format!("测试失败 · {}", record.at),
            Self::Stale(_) => "配置改过，需重测".into(),
        }
    }

    /// 悬停说明。
    fn text(&self) -> String {
        match self {
            Self::Problems(_) => "配置有问题，见「技术配置」".into(),
            Self::MissingKey(names) => {
                format!("密钥「{}」还没填：在详情顶上粘贴", names.join("」「"))
            }
            Self::AuthFailed(record) => format!(
                "Key 可能不对或已过期，在详情顶上重新粘贴。{}",
                record.summary
            ),
            Self::Untested => "还没测过".into(),
            Self::Passed(record) | Self::Failed(record) => record.summary.clone(),
            Self::Stale(record) => format!("上次 {}：{}", record.at, record.summary),
        }
    }

    fn chip(&self, ui: &mut egui::Ui) -> egui::Response {
        let (fg, bg) = match self {
            Self::Passed(_) => (theme::success(), theme::success_soft()),
            Self::Failed(_) | Self::AuthFailed(_) | Self::Problems(_) => {
                (theme::danger(), theme::danger_soft())
            }
            Self::MissingKey(_) | Self::Stale(_) => (theme::warn(), theme::warn_soft()),
            Self::Untested => (theme::text_muted(), theme::surface_sunk()),
        };
        theme::chip(ui, &self.label(), fg, bg).on_hover_text(self.text())
    }
}

/// 还没选中任何东西：上手说明与两个入口。
fn welcome_ui(ui: &mut egui::Ui, page: &mut ApisPage) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.add(
            theme::Icon::Braces
                .image_sized(36.0)
                .tint(theme::border_strong()),
        );
        ui.add_space(10.0);
        let title = if page.store.endpoints.is_empty() {
            "添加第一个数据接口"
        } else {
            "在左边选一个接口"
        };
        ui.label(egui::RichText::new(title).size(theme::font_sizes::HEADING));
        ui.add_space(4.0);
        theme::caption(
            ui,
            "粘贴接口文档或 cURL 命令，程序会识别出接口、填好配置并试调一次，缺 Key 会问你。",
        );
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            // 两个按钮居中：先量出总宽再留左边距。
            let total = 250.0;
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            if theme::primary_icon_button(ui, theme::Icon::WandSparkles, "从文档添加").clicked()
            {
                page.start_import();
            }
            if ui
                .add(theme::secondary_icon_button(theme::Icon::Edit, "手动填写"))
                .clicked()
            {
                page.add_blank();
            }
        });
    });
}

// —— 实测（详情与导入共用） ——

/// 实测像是鉴权没过时的提示。
pub(super) fn auth_hint(ui: &mut egui::Ui, has_auth: bool) {
    theme::notice(
        ui,
        theme::Icon::Shield,
        theme::warn(),
        theme::warn_soft(),
        if has_auth {
            "看起来是鉴权没过：Key 不对、过期，或者接口要的带法和这里配的不一样。重新粘贴 Key，离开输入框会自动重测。"
        } else {
            "看起来接口要鉴权，但这里没配密钥：选好带法加上，再粘贴 Key。"
        },
    );
}

/// 后台实测一次。
pub(super) fn spawn_trial(
    endpoint: ApiEndpoint,
    args: Map<String, Value>,
    secrets: ApiSecrets,
) -> Receiver<Trial> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(api::trial(&endpoint, &args, &secrets));
    });
    rx
}

/// 精简版实测结果（导入候选用）：结论、条目预览、请求与原始返回（折叠）。
pub(super) fn trial_ui(ui: &mut egui::Ui, trial: &Trial, salt: &str) {
    match &trial.error {
        None => {
            theme::notice(
                ui,
                theme::Icon::Check,
                theme::success(),
                theme::success_soft(),
                format!("调通了：{}", trial.summary()),
            );
        }
        Some(error) => {
            theme::notice(
                ui,
                theme::Icon::TriangleAlert,
                theme::danger(),
                theme::danger_soft(),
                error.clone(),
            );
        }
    }
    for item in trial.items.iter().take(PREVIEW_ITEMS) {
        ui.label(format!(
            "【{}】{}",
            item.title,
            crate::agent::tools::short(&item.text, 120)
        ));
        theme::caption(ui, &format!("出处：{} · 编号：{}", item.source, item.id));
    }
    if trial.items.len() > PREVIEW_ITEMS {
        theme::caption(ui, &format!("……共 {} 条", trial.items.len()));
    }
    if trial.request.is_some() || trial.raw.is_some() {
        egui::CollapsingHeader::new("请求与原始返回")
            .id_salt(("api_trial_raw", salt))
            .show(ui, |ui| {
                if let Some(request) = &trial.request {
                    ui.label("请求（密钥已打码）");
                    code_block(ui, &format!("api_request_{salt}"), request);
                }
                if let Some(raw) = &trial.raw {
                    ui.label(format!("原始返回（状态 {}）", raw.status));
                    let preview: String = raw.body.chars().take(RAW_PREVIEW_CHARS).collect();
                    code_block(ui, &format!("api_raw_{salt}"), &preview);
                }
            });
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::agent::api::{ApiHeader, ApiInput, MappedItem, RawResponse};

    /// 在一个 egui 帧里画一遍，返回画面上的文字。
    pub(in crate::app) fn render(mut add: impl FnMut(&mut egui::Ui)) -> Vec<String> {
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let raw = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 3000.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw(), |ui| add(ui));
        let output = ctx.run_ui(raw(), |ui| add(ui));
        fn collect(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
            match shape {
                egui::epaint::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        texts
    }

    pub(super) fn endpoint() -> ApiEndpoint {
        ApiEndpoint {
            id: "stat".into(),
            name: "火灾统计".into(),
            description: "按地区查森林火灾起数".into(),
            method: ApiMethod::Post,
            url: "http://10.0.0.8/stat".into(),
            body: "{\"q\": \"{region}\"}".into(),
            inputs: vec![ApiInput {
                name: "region".into(),
                description: "地区".into(),
                required: true,
                example: "全省".into(),
                ..ApiInput::default()
            }],
            ..ApiEndpoint::default()
        }
    }

    fn page_with(endpoints: Vec<ApiEndpoint>) -> ApisPage {
        let mut page = ApisPage {
            loaded: true,
            store: ApiStore {
                endpoints,
                ..Default::default()
            },
            ..ApisPage::default()
        };
        page.open(0);
        page
    }

    fn draw(page: &mut ApisPage) -> Vec<String> {
        let config = AppConfig::default();
        render(|ui| section_ui(ui, page, &config))
    }

    /// 用户给的 TypeSafe 文档（Mintlify）：一个接口、几种问题类型，各有请求示例。
    fn typesafe_endpoint() -> ApiEndpoint {
        let doc = include_str!("../agent/api_import/fixtures/typesafe.md");
        let material = crate::agent::api_import::Material::new(doc);
        let mut endpoint = crate::agent::api_import::analyze(&material, None, &[])
            .drafts
            .remove(0)
            .endpoint;
        endpoint.name = "文本评估".into();
        endpoint
    }

    #[test]
    fn several_examples_are_listed_and_can_be_picked_in_try() {
        let mut page = page_with(vec![typesafe_endpoint()]);
        let texts = draw(&mut page);
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(
            has("用法示例") && has("文档示例：Choice") && has("去试一下"),
            "接口说明列出样例：{texts:?}"
        );
        page.detail.tab = detail::Tab::Try;
        let texts = draw(&mut page);
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(
            has("文档示例：Noul") && has("文档示例：Score") && has("全部 4 组试一遍"),
            "{texts:?}"
        );
        assert!(has("存为样例"), "{texts:?}");
        assert_eq!(page.detail.example, Some(0), "打开时填好第一组");
        assert!(
            page.detail.args["questions"].contains("\"is_urgent\""),
            "{:?}",
            page.detail.args
        );
        assert!(
            page.detail.args["questions"].contains('\n'),
            "JSON 参数排成多行"
        );
    }

    #[test]
    fn running_every_example_reports_each_result() {
        let server = crate::agent::api::test_server::TestServer::start(vec![
            (
                200,
                serde_json::json!({"answers": {"q": {"type": "noul", "noul": 0.9}}}).to_string(),
            ),
            (422, "{\"detail\": \"bad question\"}".into()),
        ]);
        let mut endpoint = typesafe_endpoint();
        endpoint.url = format!("{}/v1/systemone", server.url);
        endpoint.examples.truncate(2);
        let mut page = page_with(vec![endpoint]);
        page.detail.tab = detail::Tab::Try;
        page.secrets.secrets.insert("token".into(), "k".into());
        let endpoint = page.store.endpoints[0].clone();
        detail::start_runs_for_test(&mut page, &endpoint);
        for _ in 0..200 {
            let _ = draw(&mut page);
            if page.detail_runs_done() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let texts = draw(&mut page);
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(has("2 组样例，调通 1 组"), "{texts:?}");
        assert!(
            server.request(0).contains("is_urgent"),
            "第一组发的是它自己的问题"
        );
    }

    #[test]
    fn the_list_groups_by_server_and_the_detail_reads_like_a_document() {
        let mut page = page_with(vec![endpoint()]);
        let trial = Trial::default();
        page.log.record(&page.store.endpoints[0], &trial);
        let texts = draw(&mut page);
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(
            has("10.0.0.8") && has("无需鉴权"),
            "按服务器分组：{texts:?}"
        );
        assert!(has("火灾统计") && has("/stat"), "{texts:?}");
        assert!(has("测试通过"), "{texts:?}");
        assert!(
            has("用途") && has("按地区查森林火灾起数") && has("查询条件"),
            "{texts:?}"
        );
        assert!(has("region") && has("地区") && has("必填"), "{texts:?}");
        assert!(has("返回什么") && has("http.call:stat"), "{texts:?}");
    }

    #[test]
    fn keys_are_bound_to_endpoints_and_shown_on_the_server_group() {
        let mut with_key = endpoint();
        with_key.headers.push(ApiHeader {
            name: "Authorization".into(),
            value: "Bearer {secret:token}".into(),
        });
        let mut page = page_with(vec![with_key]);
        page.secrets.secrets.insert("token".into(), String::new());
        let texts = draw(&mut page);
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(has("缺 Key"), "组标题标出缺 Key：{texts:?}");
        assert!(has("密钥 token") && has("还没填"), "{texts:?}");

        page.secrets.secrets.insert("token".into(), "s3cr3t".into());
        let texts = draw(&mut page);
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(
            has("请求头 Authorization: Bearer 密钥") && has("更换 Key"),
            "{texts:?}"
        );
        assert!(!has("s3cr3t"), "密钥打码显示：{texts:?}");

        page.open_service("http://10.0.0.8".into());
        let texts = draw(&mut page);
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(has("全部重测") && has("火灾统计"), "{texts:?}");
        assert!(!has("s3cr3t"), "密钥打码显示：{texts:?}");

        let mut bare = page_with(vec![endpoint()]);
        let texts = draw(&mut bare);
        assert!(
            texts.iter().any(|t| t.contains("这个接口不带密钥")),
            "{texts:?}"
        );
    }

    #[test]
    fn auth_failures_are_recognised() {
        let failed = |status: u16, body: &str| Trial {
            raw: Some(RawResponse {
                status,
                body: body.into(),
            }),
            error: Some("失败".into()),
            ..Trial::default()
        };
        assert!(failed(401, "").auth_rejected());
        assert!(failed(200, "<!DOCTYPE html><html>请登录</html>").auth_rejected());
        assert!(failed(200, "{\"code\": 401, \"msg\": \"x\"}").auth_rejected());
        assert!(failed(200, "{\"code\": 500, \"msg\": \"token 已过期\"}").auth_rejected());
        assert!(!failed(404, "not found").auth_rejected());
        assert!(!failed(500, "<html>服务器内部错误</html>").auth_rejected());
        assert!(!failed(200, "{\"code\": 500, \"msg\": \"查询超时\"}").auth_rejected());
        let mut record_log = ApiTestLog::default();
        record_log.record(&endpoint(), &failed(403, ""));
        assert!(record_log.records["stat"].auth);
    }

    #[test]
    fn technical_fields_live_in_their_own_tab_and_trials_show_input_and_output() {
        let mut page = page_with(vec![endpoint()]);
        let texts = draw(&mut page);
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(
            has("接口说明") && has("试一下") && has("技术配置"),
            "{texts:?}"
        );
        assert!(!has("请求体模板"), "技术配置在自己的页里：{texts:?}");

        page.detail.tab = detail::Tab::Try;
        page.detail.result = Some(Trial {
            request: Some("POST http://x".into()),
            raw: Some(RawResponse {
                status: 200,
                body: "{\"items\": [{\"title\": \"年度统计\", \"content\": \"共12起\"}]}".into(),
            }),
            items: vec![MappedItem {
                id: "1".into(),
                title: "年度统计".into(),
                text: "共12起".into(),
                source: "统计系统".into(),
            }],
            total: 1,
            error: None,
        });
        let texts = draw(&mut page);
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(has("请求") && has("响应") && has("发送请求"), "{texts:?}");
        assert!(has("HTTP 200") && has("取到 1 条"), "{texts:?}");
        assert!(has("年度统计") && has("共12起"), "{texts:?}");
        assert!(has("按这次的返回补全"), "{texts:?}");

        page.detail.tab = detail::Tab::Config;
        let texts = draw(&mut page);
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(has("请求体模板") && has("删除这个接口"), "{texts:?}");
    }

    #[test]
    fn editing_the_description_happens_in_place_and_can_be_discarded() {
        let mut page = page_with(vec![endpoint()]);
        page.detail.start_editing(&page.store.endpoints[0]);
        page.store.endpoints[0].description = "改过的说明".into();
        let texts = draw(&mut page);
        assert!(
            texts.iter().any(|t| t.contains("正在编辑说明")),
            "{texts:?}"
        );
        detail::discard_edit(&mut page, 0);
        assert_eq!(page.store.endpoints[0].description, "按地区查森林火灾起数");
        assert!(page.detail.editing.is_none());
    }

    #[test]
    fn path_and_host_split_the_url() {
        assert_eq!(
            host_of("http://10.0.0.9:8080/api/x"),
            "http://10.0.0.9:8080"
        );
        assert_eq!(path_of("http://10.0.0.9:8080/api/x?a=1"), "/api/x?a=1");
        assert_eq!(path_of("https://a.b?x=1"), "?x=1");
        assert_eq!(path_of("https://a.b"), "/");
    }
}
