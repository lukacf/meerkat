//! Runtime-internal executor effects.
//!
//! The DSL emits neutral facts. This module is the only place that turns those
//! facts into executable runtime-loop effects.

use crate::meerkat_machine::dsl;

/// Neutral fact projected from a committed MeerkatMachine DSL transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeEffectFact {
    CancelAfterBoundary { reason: String },
    StopRuntimeExecutor { reason: String },
}

impl RuntimeEffectFact {
    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::CancelAfterBoundary { reason } | Self::StopRuntimeExecutor { reason } => reason,
        }
    }
}

/// Sealed executable effect sent to the runtime loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeEffect {
    inner: RuntimeEffectInner,
}

/// Runtime-loop executor effects. Hard cancel is intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeEffectInner {
    CancelAfterBoundary { reason: String },
    StopRuntimeExecutor { reason: String },
}

impl RuntimeEffect {
    pub(crate) fn from_fact(fact: RuntimeEffectFact) -> Self {
        let inner = match fact {
            RuntimeEffectFact::CancelAfterBoundary { reason } => {
                RuntimeEffectInner::CancelAfterBoundary { reason }
            }
            RuntimeEffectFact::StopRuntimeExecutor { reason } => {
                RuntimeEffectInner::StopRuntimeExecutor { reason }
            }
        };
        Self { inner }
    }

    pub(crate) fn into_inner(self) -> RuntimeEffectInner {
        self.inner
    }
}

pub(crate) fn runtime_effect_facts_from_effects(
    effects: &[dsl::MeerkatMachineEffect],
) -> Vec<RuntimeEffectFact> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            dsl::MeerkatMachineEffect::RuntimeEffectFact { kind, reason } => match kind {
                dsl::RuntimeEffectKind::CancelAfterBoundary => {
                    Some(RuntimeEffectFact::CancelAfterBoundary {
                        reason: reason.clone(),
                    })
                }
                dsl::RuntimeEffectKind::StopRuntimeExecutor => {
                    Some(RuntimeEffectFact::StopRuntimeExecutor {
                        reason: reason.clone(),
                    })
                }
            },
            _ => None,
        })
        .collect()
}

pub(crate) fn runtime_effect_fact_from_effects(
    effects: &[dsl::MeerkatMachineEffect],
) -> Result<RuntimeEffectFact, String> {
    let Some(first) = runtime_effect_fact_optional_from_effects(effects)? else {
        return Err("DSL transition did not emit a RuntimeEffectFact".to_string());
    };
    Ok(first)
}

pub(crate) fn runtime_effect_fact_optional_from_effects(
    effects: &[dsl::MeerkatMachineEffect],
) -> Result<Option<RuntimeEffectFact>, String> {
    let mut facts = runtime_effect_facts_from_effects(effects).into_iter();
    let Some(first) = facts.next() else {
        return Ok(None);
    };
    if facts.next().is_some() {
        return Err("DSL transition emitted multiple RuntimeEffectFacts".to_string());
    }
    Ok(Some(first))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_effect_from_fact_maps_cancel_after_boundary() {
        let effect = RuntimeEffect::from_fact(RuntimeEffectFact::CancelAfterBoundary {
            reason: "peer admission".to_string(),
        });

        assert_eq!(
            effect.into_inner(),
            RuntimeEffectInner::CancelAfterBoundary {
                reason: "peer admission".to_string()
            }
        );
    }

    #[test]
    fn runtime_effect_from_fact_maps_stop_runtime_executor() {
        let effect = RuntimeEffect::from_fact(RuntimeEffectFact::StopRuntimeExecutor {
            reason: "shutdown".to_string(),
        });

        assert_eq!(
            effect.into_inner(),
            RuntimeEffectInner::StopRuntimeExecutor {
                reason: "shutdown".to_string()
            }
        );
    }

    #[test]
    fn runtime_effect_fact_projection_reads_generated_dsl_effect() {
        let effects = vec![dsl::MeerkatMachineEffect::RuntimeEffectFact {
            kind: dsl::RuntimeEffectKind::CancelAfterBoundary,
            reason: "from dsl".to_string(),
        }];

        assert_eq!(
            runtime_effect_fact_from_effects(&effects).expect("fact"),
            RuntimeEffectFact::CancelAfterBoundary {
                reason: "from dsl".to_string()
            }
        );
    }
}
