//! Chat Completions 的思考深度参数。默认不干预，不靠提示词模拟深度。

use crate::models::{LmStudioConfig, ReasoningFormat};
use serde_json::{Value, json};

pub(super) fn apply(config: &LmStudioConfig, payload: &mut Value) {
    let Some(effort) = config.reasoning_effort.wire_value() else {
        return;
    };
    match config.reasoning_format {
        ReasoningFormat::ReasoningEffort => payload["reasoning_effort"] = json!(effort),
        ReasoningFormat::ReasoningObject => payload["reasoning"] = json!({"effort": effort}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AppConfig, ReasoningEffort};

    #[test]
    fn old_config_keeps_the_service_default_and_new_settings_round_trip() {
        let mut config: LmStudioConfig = serde_json::from_str(r#"{"model":"test"}"#).unwrap();
        let mut payload = json!({"model": "test"});
        apply(&config, &mut payload);
        assert_eq!(payload, json!({"model": "test"}));
        config.reasoning_effort = ReasoningEffort::High;
        config.reasoning_format = ReasoningFormat::ReasoningObject;
        let saved = serde_json::to_string(&config).unwrap();
        let restored: LmStudioConfig = serde_json::from_str(&saved).unwrap();
        apply(&restored, &mut payload);
        assert_eq!(payload["reasoning"], json!({"effort": "high"}));
        assert!(payload.get("reasoning_effort").is_none());
    }

    #[test]
    fn draft_depth_reaches_streaming_payload_but_never_sentence_review() {
        let mut config = AppConfig::default();
        config.lm_studio.model = "test".into();
        config.lm_studio.reasoning_effort = ReasoningEffort::Max;
        let draft = config.draft_chat().unwrap();
        for stream in [false, true] {
            let payload = super::super::chat_payload(&draft, "s", "u", 0.0, 4096, false, stream);
            assert_eq!(payload["reasoning_effort"], "max");
            let disabled = super::super::chat_payload(&draft, "s", "u", 0.0, 512, true, stream);
            assert!(disabled.get("reasoning_effort").is_none());
        }
        let review = config.revise_chat(false).unwrap();
        let payload = super::super::chat_payload(&review, "s", "u", 0.0, 512, false, false);
        assert!(payload.get("reasoning_effort").is_none());
        assert_eq!(
            config.assist_chat().unwrap().reasoning_effort,
            ReasoningEffort::Max
        );
        config.revise_model.enabled = true;
        config.revise_model.model = "independent-review".into();
        assert_eq!(
            config.assist_chat().unwrap().reasoning_effort,
            ReasoningEffort::Default
        );
    }
}
