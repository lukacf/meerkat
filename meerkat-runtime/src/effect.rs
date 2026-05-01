//! Runtime-internal executor effects.
//!
//! The DSL emits neutral facts. This module is the only place that turns those
//! facts into executable runtime-loop effects.

use crate::meerkat_machine::{DslTransitionEffects, dsl};

/// Neutral fact projected from a committed MeerkatMachine DSL transition.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeEffectFact {
    CancelAfterBoundary { reason: String },
    StopRuntimeExecutor { reason: String },
}

impl RuntimeEffectFact {
    fn reason(&self) -> &str {
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
    fn from_fact(fact: RuntimeEffectFact) -> Self {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectedRuntimeEffect {
    effect: RuntimeEffect,
    reason: String,
}

impl ProjectedRuntimeEffect {
    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }

    pub(crate) fn into_effect(self) -> RuntimeEffect {
        self.effect
    }
}

fn project_runtime_effect_fact(fact: RuntimeEffectFact) -> ProjectedRuntimeEffect {
    let reason = fact.reason().to_string();
    ProjectedRuntimeEffect {
        effect: RuntimeEffect::from_fact(fact),
        reason,
    }
}

fn runtime_effect_facts_from_raw_effects(
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

fn runtime_effect_fact_from_raw_effects(
    effects: &[dsl::MeerkatMachineEffect],
) -> Result<RuntimeEffectFact, String> {
    let Some(first) = runtime_effect_fact_optional_from_raw_effects(effects)? else {
        return Err("DSL transition did not emit a RuntimeEffectFact".to_string());
    };
    Ok(first)
}

fn runtime_effect_fact_optional_from_raw_effects(
    effects: &[dsl::MeerkatMachineEffect],
) -> Result<Option<RuntimeEffectFact>, String> {
    let mut facts = runtime_effect_facts_from_raw_effects(effects).into_iter();
    let Some(first) = facts.next() else {
        return Ok(None);
    };
    if facts.next().is_some() {
        return Err("DSL transition emitted multiple RuntimeEffectFacts".to_string());
    }
    Ok(Some(first))
}

pub(crate) fn runtime_effect_projection_from_dsl_effects(
    effects: &DslTransitionEffects,
) -> Result<ProjectedRuntimeEffect, String> {
    runtime_effect_fact_from_raw_effects(effects.as_slice()).map(project_runtime_effect_fact)
}

pub(crate) fn runtime_effect_projection_optional_from_dsl_effects(
    effects: &DslTransitionEffects,
) -> Result<Option<ProjectedRuntimeEffect>, String> {
    runtime_effect_fact_optional_from_raw_effects(effects.as_slice())
        .map(|fact| fact.map(project_runtime_effect_fact))
}

#[cfg(test)]
pub(crate) fn runtime_effect_for_test(kind: dsl::RuntimeEffectKind, reason: &str) -> RuntimeEffect {
    let fact = match kind {
        dsl::RuntimeEffectKind::CancelAfterBoundary => RuntimeEffectFact::CancelAfterBoundary {
            reason: reason.to_string(),
        },
        dsl::RuntimeEffectKind::StopRuntimeExecutor => RuntimeEffectFact::StopRuntimeExecutor {
            reason: reason.to_string(),
        },
    };
    RuntimeEffect::from_fact(fact)
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
            runtime_effect_fact_from_raw_effects(&effects).expect("fact"),
            RuntimeEffectFact::CancelAfterBoundary {
                reason: "from dsl".to_string()
            }
        );
    }
}
