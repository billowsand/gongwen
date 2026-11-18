//! 状态栏的写作模型探测。只查询所选提供商的模型清单。

use super::{AiSection, GongwenApp, NavPage, TabRef, WorkerResult};
use crate::models::{ProviderConfig, ProxyConfig};
use crate::theme;
use eframe::egui;
use std::time::{Duration, Instant};

const PROBE_INTERVAL: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT_SECONDS: u64 = 5;

#[derive(Clone)]
struct ProbeTarget {
    model: String,
    base_url: String,
    api_key: String,
    proxy: ProxyConfig,
}

impl ProbeTarget {
    fn connection(&self) -> (&str, &str, &str, &ProxyConfig) {
        (&self.model, &self.base_url, &self.api_key, &self.proxy)
    }

    fn check(&self) -> ProbeResult {
        let models =
            crate::net::client_with_proxy(&self.proxy, &self.base_url, PROBE_TIMEOUT_SECONDS)
                .and_then(|client| {
                    crate::lmstudio::list_models_with_client(&client, &self.base_url, &self.api_key)
                });
        match models {
            Ok(models) if models.contains(&self.model) => ProbeResult::Listed,
            Ok(_) => ProbeResult::Missing,
            Err(error) => ProbeResult::Failed(error.to_string()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProbeResult {
    Listed,
    Missing,
    Failed(String),
}

#[derive(Default)]
pub(super) struct ModelHealth {
    target: Option<ProbeTarget>,
    revision: u64,
    in_flight: bool,
    next_check: Option<Instant>,
    result: Option<ProbeResult>,
}

impl ModelHealth {
    fn sync_target(
        &mut self,
        model: &str,
        provider: Option<&ProviderConfig>,
        proxy: &ProxyConfig,
        now: Instant,
    ) -> bool {
        let connection = provider.map(|p| (model, p.base_url.trim(), p.api_key.trim(), proxy));
        if self.target.as_ref().map(ProbeTarget::connection) == connection {
            return false;
        }
        self.target = connection.map(|(model, base_url, api_key, proxy)| ProbeTarget {
            model: model.to_string(),
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            proxy: proxy.clone(),
        });
        // 配置变更后旧连接的结果失效，下一次检查立即使用新配置。
        self.revision += 1;
        self.result = None;
        self.next_check = self.target.as_ref().map(|_| now);
        true
    }

    fn start_due(&mut self, now: Instant) -> Option<(u64, ProbeTarget)> {
        if self.in_flight || self.next_check? > now {
            return None;
        }
        let target = self.target.clone()?;
        self.in_flight = true;
        self.next_check = Some(now + PROBE_INTERVAL);
        Some((self.revision, target))
    }

    pub(super) fn complete(&mut self, revision: u64, result: ProbeResult) {
        self.in_flight = false;
        if revision == self.revision {
            self.result = Some(result);
        }
    }
}

impl GongwenApp {
    pub(super) fn tick_model_health(&mut self, ctx: &egui::Context) {
        let model_ref = &self.config.pi_model;
        // 服务商表单直接编辑配置，收起编辑或离开页面后再用新地址和密钥探测。
        let editing_provider = self.tabs.get(self.active_tab)
            == Some(&TabRef::Page(NavPage::AiPrompts))
            && self.ai_section == AiSection::Providers
            && self.provider_edit.contains(&model_ref.provider_id);
        let provider = if model_ref.is_empty() || editing_provider {
            None
        } else {
            self.config
                .require_provider(&model_ref.provider_id, "智能写作（Pi）")
                .ok()
        };
        let now = Instant::now();
        let health = &mut self.model_health;
        if health.sync_target(model_ref.model.trim(), provider, &self.config.proxy, now) {
            ctx.request_repaint();
        }
        if let Some((revision, target)) = health.start_due(now) {
            let tx = self.sender.clone();
            let wake = ctx.clone();
            std::thread::spawn(move || {
                let result = target.check();
                let _ = tx.send(WorkerResult::WritingModelHealth { revision, result });
                wake.request_repaint();
            });
        }
        if !health.in_flight
            && let Some(next) = health.next_check
        {
            // 窗口空闲时也要在到期时重绘，带起下一次检查。
            ctx.request_repaint_after(next.saturating_duration_since(now));
        }
    }

    pub(super) fn writing_model_chip(&self, ui: &mut egui::Ui) {
        let model = self.config.pi_model_label();
        let provider_id = &self.config.pi_model.provider_id;
        let (label, note, fg, bg) = if model.is_empty() {
            (
                "模型未选择",
                "请到「AI 管理 → 模型服务」选择写作模型。".into(),
                theme::warn(),
                theme::warn_soft(),
            )
        } else if let Err(error) = self.config.require_provider(provider_id, "智能写作（Pi）")
        {
            (
                "配置不可用",
                error.to_string(),
                theme::warn(),
                theme::warn_soft(),
            )
        } else {
            match &self.model_health.result {
                Some(ProbeResult::Listed) => (
                    "服务在线",
                    "服务可连接，且模型在服务返回的清单中。".into(),
                    theme::success(),
                    theme::surface_sunk(),
                ),
                Some(ProbeResult::Missing) => (
                    "模型未列出",
                    "服务可连接，返回的清单中未找到当前写作模型。".into(),
                    theme::warn(),
                    theme::warn_soft(),
                ),
                Some(ProbeResult::Failed(error)) => (
                    "检测失败",
                    format!("读取写作模型清单失败：{error}"),
                    theme::danger(),
                    theme::danger_soft(),
                ),
                None => (
                    "待检测",
                    "正在等待写作模型检测结果。".into(),
                    theme::text_muted(),
                    theme::surface_sunk(),
                ),
            }
        };
        let text = if model.is_empty() {
            label.to_string()
        } else {
            format!("{label} · {model}")
        };
        theme::chip(ui, &text, fg, bg).on_hover_text(note);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_at_startup_and_rechecks_after_failure_at_the_same_interval() {
        let mut health = ModelHealth::default();
        let now = Instant::now();
        let provider = ProviderConfig::default();
        let proxy = ProxyConfig::default();
        assert!(health.start_due(now).is_none());
        assert!(health.sync_target("writer", Some(&provider), &proxy, now));
        let (revision, _) = health.start_due(now).unwrap();
        assert!(health.start_due(now + PROBE_INTERVAL).is_none());
        health.complete(revision, ProbeResult::Failed("服务断开".into()));
        assert!(!health.sync_target("writer", Some(&provider), &proxy, now));
        assert!(health.start_due(now + Duration::from_secs(59)).is_none());
        let (revision, _) = health.start_due(now + PROBE_INTERVAL).unwrap();
        health.complete(revision, ProbeResult::Listed);
        assert_eq!(health.result, Some(ProbeResult::Listed));
    }

    #[test]
    fn replaces_the_target_and_ignores_the_previous_results() {
        let mut health = ModelHealth::default();
        let now = Instant::now();
        let mut provider = ProviderConfig::default();
        let proxy = ProxyConfig::default();
        health.sync_target("writer", Some(&provider), &proxy, now);
        let (previous, _) = health.start_due(now).unwrap();

        provider.api_key = "updated".into();
        assert!(health.sync_target("writer", Some(&provider), &proxy, now));
        assert!(health.start_due(now).is_none());
        health.complete(previous, ProbeResult::Listed);
        assert!(health.result.is_none());
        let (current, target) = health.start_due(now).unwrap();
        assert_eq!(target.api_key, "updated");
        health.complete(current, ProbeResult::Missing);
        assert_eq!(health.result, Some(ProbeResult::Missing));

        health.sync_target("", None, &proxy, now);
        assert!(health.result.is_none());
        assert!(health.start_due(now + PROBE_INTERVAL).is_none());
    }
}
