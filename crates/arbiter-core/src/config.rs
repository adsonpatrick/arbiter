//! Arbiter configuration domain types.

use serde::{Deserialize, Serialize};

/// Reasoning effort requested from an inference model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

/// The fixed model and reasoning pair used by M0 passthrough mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineTarget {
    pub model: String,
    pub reasoning_effort: ReasoningEffort,
}

impl BaselineTarget {
    #[must_use]
    pub fn m0() -> Self {
        Self {
            model: "gpt-5.6-terra".to_owned(),
            reasoning_effort: ReasoningEffort::Medium,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BaselineTarget, ReasoningEffort};

    #[test]
    fn m0_baseline_is_terra_with_medium_reasoning() {
        let baseline = BaselineTarget::m0();

        assert_eq!(baseline.model, "gpt-5.6-terra");
        assert_eq!(baseline.reasoning_effort, ReasoningEffort::Medium);
    }
}
