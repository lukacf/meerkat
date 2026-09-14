use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Provider, TurnUsage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopedTokenAccountingStatus {
    Pending,
    NotApplicable,
    Unmeasured,
    Measured,
    Disputed,
}

/// A bounded reference to ordinary normalized accounting, not a billing account.
/// The complete attribution remains with the ordinary `TurnUsage` owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopedEffectTokenAccounting {
    NotApplicable {},
    Unmeasured {},
    Measured {
        normalized_tokens: u64,
        reported_provider: Provider,
        reported_model_digest: [u8; 32],
        normalized_counter_digest: [u8; 32],
        identity_disputed: bool,
    },
}

impl ScopedEffectTokenAccounting {
    pub fn for_unmeasured_target(target: &super::ScopedEffectTarget) -> Self {
        match target {
            super::ScopedEffectTarget::ModelComputation { .. } => Self::Unmeasured {},
            super::ScopedEffectTarget::ToolDispatch { .. }
            | super::ScopedEffectTarget::DescendantAdmission { .. } => Self::NotApplicable {},
        }
    }

    pub const fn status(self) -> ScopedTokenAccountingStatus {
        match self {
            Self::NotApplicable {} => ScopedTokenAccountingStatus::NotApplicable,
            Self::Unmeasured {} => ScopedTokenAccountingStatus::Unmeasured,
            Self::Measured {
                identity_disputed: false,
                ..
            } => ScopedTokenAccountingStatus::Measured,
            Self::Measured {
                identity_disputed: true,
                ..
            } => ScopedTokenAccountingStatus::Disputed,
        }
    }

    pub const fn known_tokens(self) -> Option<u64> {
        match self {
            Self::Measured {
                normalized_tokens, ..
            } => Some(normalized_tokens),
            Self::NotApplicable {} | Self::Unmeasured {} => None,
        }
    }

    pub fn from_turn_usage(
        usage: &TurnUsage,
        selected_provider: Provider,
        selected_model: &str,
    ) -> Self {
        let accounting = usage.accounting();
        let reported_model_digest: [u8; 32] = Sha256::digest(accounting.model.as_bytes()).into();
        let mut digest = Sha256::new();
        digest.update(b"meerkat.scoped-normalized-counter.v1\0");
        digest.update(accounting.provider.as_str().as_bytes());
        digest.update([0]);
        digest.update(reported_model_digest);
        digest.update(usage.presented_tokens().to_be_bytes());
        digest.update(usage.output_tokens.to_be_bytes());
        Self::Measured {
            normalized_tokens: usage.normalized_total_tokens(),
            reported_provider: accounting.provider,
            reported_model_digest,
            normalized_counter_digest: digest.finalize().into(),
            identity_disputed: matches!(
                crate::agent::classify_provider_turn_usage_identity(
                    usage,
                    selected_provider,
                    selected_model,
                ),
                crate::agent::TurnUsageIdentityVerdict::Disputed(_)
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_unmeasured_accounting_rejects_extra_fields_without_changing_encoding()
    -> Result<(), Box<dyn std::error::Error>> {
        for (value, kind) in [
            (
                ScopedEffectTokenAccounting::NotApplicable {},
                "not_applicable",
            ),
            (ScopedEffectTokenAccounting::Unmeasured {}, "unmeasured"),
        ] {
            let encoded = serde_json::to_value(value)?;
            assert_eq!(encoded, serde_json::json!({"kind": kind}));
            assert_eq!(
                serde_json::from_value::<ScopedEffectTokenAccounting>(encoded.clone())?,
                value
            );
            assert_eq!(value.known_tokens(), None);
            for field in [
                "normalized_tokens",
                "identity_disputed",
                "unexpected_authority",
            ] {
                let mut altered = encoded.clone();
                altered[field] = serde_json::json!(0);
                assert!(serde_json::from_value::<ScopedEffectTokenAccounting>(altered).is_err());
            }
        }
        Ok(())
    }

    #[test]
    fn scoped_tokens_reuse_normalization_without_repairing_disputed_identity() {
        let usage = TurnUsage::host_declared(
            Provider::Anthropic,
            "reported-model",
            crate::Usage {
                input_tokens: 45,
                output_tokens: 7,
                ..Default::default()
            },
        );
        let original = usage.clone();
        let observed = ScopedEffectTokenAccounting::from_turn_usage(
            &usage,
            Provider::OpenAI,
            "selected-model",
        );
        assert!(matches!(
            observed,
            ScopedEffectTokenAccounting::Measured {
                normalized_tokens: 52,
                reported_provider: Provider::Anthropic,
                identity_disputed: true,
                ..
            }
        ));
        assert_eq!(usage, original);
        assert_eq!(
            observed,
            ScopedEffectTokenAccounting::from_turn_usage(
                &usage,
                Provider::OpenAI,
                "selected-model"
            ),
        );
        assert_ne!(observed, ScopedEffectTokenAccounting::Unmeasured {});
    }
}
