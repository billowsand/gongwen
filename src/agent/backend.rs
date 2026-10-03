//! 模型调用接口。研究式起草的每一步都经它调模型；测试时换成按脚本回复的假模型。

use crate::lmstudio::{self, ChatOptions, Finish, StreamDelta};
use crate::models::{AppConfig, LmStudioConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 这一步该用哪个模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelRole {
    /// 起草与补全：要写得好，用起草模型。
    Draft,
    /// 列问题、核对来源、出澄清题：要稳定可复现，温度 0；配了复核模型就用它。
    Assist,
}

/// 一次补全的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Completion {
    pub(crate) content: String,
    /// 撞上输出上限，正文可能不完整。
    pub(crate) truncated: bool,
}

pub(crate) trait ModelBackend {
    /// 跑一次补全，正文与思考的增量都经 `on_delta` 回调。用户点了停止时返回错误。
    fn complete(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion>;

    /// 用户是否已经点了停止。流程在两步之间查它，不必等下一次模型调用才停。
    fn cancelled(&self) -> bool;
}

/// 接本机 / 内网的 OpenAI 兼容接口。
pub(crate) struct LmBackend {
    draft: LmStudioConfig,
    assist: LmStudioConfig,
    cancel: Arc<AtomicBool>,
}

impl LmBackend {
    pub(crate) fn new(config: &AppConfig, cancel: Arc<AtomicBool>) -> Self {
        Self {
            draft: config.lm_studio.clone(),
            assist: assist_config(config),
            cancel,
        }
    }
}

/// 辅助步骤用的接入配置：配了复核模型就用它，否则沿用起草模型；温度固定 0。
///
/// 输出上限沿用起草模型的设置，不用复核那套按句长算的小上限：思考型模型的推理也算在
/// 上限里，给小了正文出不来。
pub(crate) fn assist_config(config: &AppConfig) -> LmStudioConfig {
    let revise = &config.revise_model;
    let mut assist = if revise.enabled && !revise.model.trim().is_empty() {
        revise.resolve(&config.lm_studio)
    } else {
        config.lm_studio.clone()
    };
    assist.temperature = 0.0;
    assist.max_tokens = config.lm_studio.max_tokens;
    assist
}

impl ModelBackend for LmBackend {
    fn complete(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion> {
        let config = match role {
            ModelRole::Draft => &self.draft,
            ModelRole::Assist => &self.assist,
        };
        let outcome = lmstudio::generate_stream(
            config,
            system,
            user,
            config.temperature,
            config.max_tokens,
            ChatOptions::default(),
            &self.cancel,
            on_delta,
        )?;
        if outcome.finish == Finish::Cancelled {
            anyhow::bail!("已停止生成");
        }
        Ok(Completion {
            content: outcome.content,
            truncated: outcome.finish == Finish::Length,
        })
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assist_falls_back_to_the_draft_model_with_zero_temperature() {
        let mut config = AppConfig::default();
        config.lm_studio.model = "draft".into();
        config.lm_studio.max_tokens = 32000;
        let assist = assist_config(&config);
        assert_eq!(assist.model, "draft");
        assert_eq!(assist.temperature, 0.0);
        assert_eq!(assist.max_tokens, 32000);

        config.revise_model.enabled = true;
        config.revise_model.model = "small".into();
        let assist = assist_config(&config);
        assert_eq!(assist.model, "small");
        assert_eq!(assist.max_tokens, 32000, "不用复核那套 512 的小上限");
    }
}
