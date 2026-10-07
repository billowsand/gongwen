//! 一轮任务的模型用量记录，挂起续跑时追加到同一轮。

use super::ModelRole;
use crate::lmstudio::{Usage, context};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct UsageTotals {
    pub(crate) calls: Vec<CallUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CallUsage {
    pub(crate) role: ModelRole,
    pub(crate) model: String,
    pub(crate) tokens: Usage,
    pub(crate) elapsed_ms: u64,
    pub(crate) estimated: bool,
    #[serde(default)]
    pub(crate) produced_output: bool,
}

impl UsageTotals {
    pub(crate) fn append(&mut self, other: Self) {
        self.calls.extend(other.calls);
    }

    pub(crate) fn record(
        &mut self,
        role: ModelRole,
        model: &str,
        input: usize,
        output: &str,
        usage: Option<Usage>,
        elapsed_ms: u64,
    ) {
        let estimated = usage.is_none();
        let tokens = usage.unwrap_or_else(|| {
            let prompt_tokens = input as u64;
            let completion_tokens = context::estimate_tokens(output) as u64;
            Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
            }
        });
        self.calls.push(CallUsage {
            role,
            model: model.into(),
            tokens,
            elapsed_ms,
            estimated,
            produced_output: !output.is_empty(),
        });
    }

    #[cfg(test)]
    pub(crate) fn draft_models(&self) -> Vec<String> {
        let mut models = Vec::new();
        for call in self
            .calls
            .iter()
            .filter(|call| call.role == ModelRole::Draft)
        {
            if call.produced_output && !models.contains(&call.model) {
                models.push(call.model.clone());
            }
        }
        models
    }

    pub(crate) fn label(&self, elapsed: std::time::Duration) -> String {
        let input: u64 = self.calls.iter().map(|c| c.tokens.prompt_tokens).sum();
        let output: u64 = self.calls.iter().map(|c| c.tokens.completion_tokens).sum();
        let approx = if self.calls.iter().any(|c| c.estimated) {
            "约 "
        } else {
            ""
        };
        let seconds = elapsed.as_secs();
        let duration = if seconds >= 60 {
            format!("{} 分 {} 秒", seconds / 60, seconds % 60)
        } else {
            format!("{seconds} 秒")
        };
        format!(
            "模型调用 {} 次 · {approx}输入 {} / 输出 {} token · 用时 {duration}",
            self.calls.len(),
            number(input),
            number(output)
        )
    }
}

fn number(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_accumulate_across_resume_and_mark_estimates() {
        let mut first = UsageTotals::default();
        first.record(
            ModelRole::Draft,
            "主",
            10,
            "稿",
            Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 2,
                total_tokens: 12,
            }),
            1,
        );
        let mut resumed = UsageTotals::default();
        resumed.record(ModelRole::Assist, "复核", 20, "检查", None, 2);
        first.append(resumed);
        assert_eq!(first.calls.len(), 2);
        assert!(
            first
                .label(std::time::Duration::from_secs(72))
                .contains("约 输入 30")
        );
        assert_eq!(first.draft_models(), ["主"]);
    }
}
