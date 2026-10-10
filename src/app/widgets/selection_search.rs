//! 要素下拉框检索：缓存汉字、无声调拼音与首字母，筛选不改变词库顺序或选中值。

use super::SelectOption;
use eframe::egui;
use pinyin::ToPinyinMulti;

#[derive(Clone, Default)]
struct SearchState {
    query: String,
    source: Vec<SelectOption>,
    keys: Vec<Vec<SearchKey>>,
    last_frame: Option<u64>,
}

fn compact(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '\'')
        .flat_map(char::to_lowercase)
        .map(|ch| if ch == 'ü' { 'v' } else { ch })
        .collect()
}

#[derive(Clone)]
struct SearchKey {
    literal: String,
    syllables: Vec<Vec<String>>,
}

impl SearchKey {
    fn new(text: &str) -> Self {
        let literal = compact(text);
        let syllables = literal
            .chars()
            .map(|ch| {
                if let Some(readings) = ch.to_pinyin_multi() {
                    let mut values = readings
                        .into_iter()
                        .map(|py| py.plain().replace('ü', "v"))
                        .collect::<Vec<_>>();
                    values.sort();
                    values.dedup();
                    values
                } else {
                    vec![ch.to_string()]
                }
            })
            .collect();
        Self { literal, syllables }
    }

    fn contains(&self, query: &str) -> bool {
        self.literal.contains(query)
            || self.phonetic_contains(query, false)
            || self.phonetic_contains(query, true)
    }

    /// 按字推进匹配位置，兼容多音字，不枚举整条名称的读音组合。
    /// 允许从中间音节或首字母开始查找，也允许关键词停在音节中间。
    fn phonetic_contains(&self, query: &str, initials: bool) -> bool {
        let mut positions = Vec::<usize>::new();
        for readings in &self.syllables {
            let mut next = Vec::new();
            for reading in readings {
                let reading = if initials {
                    &reading[..reading.chars().next().unwrap().len_utf8()]
                } else {
                    reading.as_str()
                };
                if reading.contains(query) {
                    return true;
                }
                for &position in &positions {
                    let remaining = &query[position..];
                    if reading.starts_with(remaining) {
                        return true;
                    }
                    if remaining.starts_with(reading) {
                        next.push(position + reading.len());
                    }
                }
                for (start, _) in reading.char_indices() {
                    let suffix = &reading[start..];
                    if query.starts_with(suffix) {
                        next.push(suffix.len());
                    }
                }
            }
            next.sort_unstable();
            next.dedup();
            positions = next;
        }
        false
    }
}

fn option_keys(option: &SelectOption) -> Vec<SearchKey> {
    // 分开索引别名，避免两个不相邻的名称拼成一个不存在的匹配。
    [&option.value, &option.full, &option.label]
        .into_iter()
        .map(String::as_str)
        .chain(option.search_terms.split_whitespace())
        .map(SearchKey::new)
        .collect()
}

fn matches(keys: &[SearchKey], query: &str) -> bool {
    query.split_whitespace().all(|term| {
        let query = compact(term);
        // 汉字按原文检索，不转成首字母，免得“海军”误命中任何缩写含 hj 的单位。
        !query.is_empty() && keys.iter().any(|key| key.contains(&query))
    })
}

pub(super) fn search_options(
    ui: &mut egui::Ui,
    field: &str,
    options: &[SelectOption],
    width: f32,
) -> Vec<SelectOption> {
    let id = ui.make_persistent_id(("selection_search", field));
    let mut state = ui.data_mut(|data| data.get_temp::<SearchState>(id).unwrap_or_default());
    if state.source != options {
        state.source = options.to_vec();
        state.keys = options.iter().map(option_keys).collect();
    }
    let frame = ui.ctx().cumulative_frame_nr();
    let just_opened = state.last_frame.is_none_or(|last| frame > last + 1);
    state.last_frame = Some(frame);
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .id(id.with("input"))
                .hint_text("汉字 / 拼音 / 首字母")
                .desired_width((width - 42.0).max(120.0)),
        );
        if just_opened {
            response.request_focus();
        }
        if ui.small_button("清空").clicked() {
            state.query.clear();
        }
    });
    let searching = !state.query.trim().is_empty();
    let filtered = options
        .iter()
        .zip(&state.keys)
        .filter(|(_, keys)| matches(keys, &state.query))
        .map(|(option, _)| {
            if searching {
                // 搜索后祖先可能不可见，直接给出全称，避免独立的“办公室”等名称歧义。
                SelectOption {
                    label: option.full.clone(),
                    depth: 0,
                    ..option.clone()
                }
            } else {
                option.clone()
            }
        })
        .collect::<Vec<_>>();
    if searching {
        ui.weak(format!("找到 {} 项", filtered.len()));
    }
    if filtered.is_empty() {
        ui.weak("没有匹配项，可换关键词或到标准词库维护");
    }
    ui.separator();
    ui.data_mut(|data| data.insert_temp(id, state));
    filtered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::VocabularyEntry;

    fn sample_options() -> Vec<SelectOption> {
        let entries: Vec<VocabularyEntry> =
            serde_json::from_str(include_str!("../../units/test_data/us-defense-units.json"))
                .unwrap();
        entries
            .into_iter()
            .map(|entry| SelectOption {
                value: entry.canonical.clone(),
                label: entry.canonical.clone(),
                full: entry.canonical,
                search_terms: format!("{} {}", entry.abbr, entry.code),
                parent: String::new(),
                depth: 0,
            })
            .collect()
    }

    #[test]
    fn defense_sample_matches_chinese_pinyin_initials_abbreviation_and_code() {
        let options = sample_options();
        assert_eq!(options.len(), 88);
        let option = options
            .iter()
            .find(|option| option.value == "美国国防部政策次长办公室")
            .unwrap();
        let keys = option_keys(option);
        for query in [
            "政策次长",
            "zhengce",
            "ZHENGCE CIZHANG",
            "zccz",
            "国防 zccz",
            "政策次长办",
            "90-01-01",
        ] {
            assert!(matches(&keys, query), "未命中：{query}");
        }
        assert!(!matches(&keys, "海军"));
        assert!(!matches(&keys, "90-01-01 海军"));
        assert!(!matches(&[SearchKey::new("环境局")], "海军"));
    }

    #[test]
    fn filtering_preserves_hidden_selections_and_vocabulary_order() {
        let options = sample_options();
        let selected = vec![options[0].value.clone(), options[1].value.clone()];
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let id = ui.make_persistent_id(("selection_search", "units"));
            ui.data_mut(|data| {
                data.insert_temp(
                    id,
                    SearchState {
                        query: "政策".into(),
                        ..SearchState::default()
                    },
                );
            });
            let filtered = search_options(ui, "units", &options, 300.0);
            assert!(!filtered.is_empty());
            assert!(!filtered.iter().any(|option| option.value == selected[0]));
            assert!(
                filtered
                    .iter()
                    .all(|option| option.depth == 0 && option.label == option.full)
            );
            assert_eq!(
                super::super::sort_by_vocabulary(selected.clone(), &options),
                selected
            );
            ui.data_mut(|data| {
                data.get_temp_mut_or_default::<SearchState>(id)
                    .query
                    .clear()
            });
            assert!(search_options(ui, "units", &options, 300.0) == options);
        });
    }

    #[test]
    fn aliases_are_searchable_and_latin_umlaut_is_normalized() {
        let mut option = super::super::plain_options(&["战略局".into()]).remove(0);
        option.search_terms = "陆军规划局 91-02".into();
        let keys = option_keys(&option);
        assert!(matches(&keys, "lujun guihua"));
        assert!(matches(&keys, "ljghj"));
        assert!(matches(&keys, "91-02"));
        let keys = vec![SearchKey::new("纪律委员会")];
        assert!(matches(&keys, "jilv"));
        assert!(matches(&keys, "JILÜ"));
    }

    #[test]
    fn polyphonic_unit_names_match_without_enumerating_combinations() {
        let keys = vec![SearchKey::new("重庆市银行行长办公室")];
        for query in ["chongqing", "cq", "yinhang", "hangzhang", "hz", "cq yh hz"] {
            assert!(matches(&keys, query), "未命中：{query}");
        }
        assert!(!matches(&keys, "chonghai"));
        assert!(!matches(&keys, "yhzh"));
    }

    /// 真实鼠标/键盘事件验证弹窗输入，不仅检查检索函数。
    #[test]
    fn single_select_popup_stays_open_while_typing_and_closes_after_selection() {
        let ctx = egui::Context::default();
        let options = sample_options();
        let mut value = String::new();
        let mut manual = std::collections::BTreeSet::new();
        let mut clock = 0;
        let mut frame = |events| {
            clock += 1;
            ctx.run_ui(
                egui::RawInput {
                    time: Some(f64::from(clock) / 60.0),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 700.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    super::super::single_select(
                        ui,
                        "unit",
                        &mut value,
                        &options,
                        &mut manual,
                        false,
                        360.0,
                        "单位",
                    );
                },
            )
        };
        fn text_rect(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
            fn find(shape: &egui::epaint::Shape, needle: &str) -> Option<egui::Rect> {
                match shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == needle => {
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                    }
                    egui::epaint::Shape::Vec(shapes) => {
                        shapes.iter().find_map(|shape| find(shape, needle))
                    }
                    _ => None,
                }
            }
            output
                .shapes
                .iter()
                .find_map(|shape| find(&shape.shape, needle))
        }
        fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        }
        frame(vec![]);
        let output = frame(vec![]);
        let button = text_rect(&output, "（未选择）").unwrap().center();
        frame(pointer(button, true));
        frame(pointer(button, false));
        frame(vec![]);
        let output = frame(vec![egui::Event::Text("zccz".into())]);
        let name = "美国国防部政策次长办公室";
        let candidate = text_rect(&output, name)
            .expect("输入拼音后弹窗应继续显示匹配单位")
            .center();
        assert!(text_rect(&output, "美国国防部").is_none());
        frame(pointer(candidate, true));
        frame(pointer(candidate, false));
        let output = frame(vec![]);
        assert!(text_rect(&output, "清空").is_none());
        assert_eq!(value, name);
    }
}
