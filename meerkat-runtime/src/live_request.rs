//! Persisted source admission and callback-chain content.
//!
//! These images issue no run, effect, cancellation or continuation authority.
//! Generated owners restore them against current grant/executor fences and the
//! ordinary callback owner before staging any work.

use std::collections::HashSet;
use std::num::NonZeroU64;

use meerkat_core::SessionId;
use meerkat_core::execution_scope::{
    ExecutionAdmissionCommitRef, ExecutionGrantRef, RunEffectScopeId, ScopedExecutorBinding,
};
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::live_execution::request::LiveSourceKey;
use meerkat_core::ops::OperationId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputRunIsolation {
    Ordinary {},
    ExclusiveLiveRequest {
        request_id: OperationId,
        source: LiveSourceKey,
    },
}

/// The ordinary-input handoff references immutable source storage rather than
/// carrying a second full request body. This record is not admission authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveExecutionRequestRecord {
    LiveRequest {
        provenance: meerkat_core::live_execution::evidence::DelegatedRequestProvenance,
        source_row: crate::live_ledger::source::LiveSourceRowDigest,
    },
}

impl LiveExecutionRequestRecord {
    pub fn isolation(&self) -> InputRunIsolation {
        let Self::LiveRequest { provenance, .. } = self;
        InputRunIsolation::ExclusiveLiveRequest {
            request_id: provenance.request_id().clone(),
            source: provenance.source().clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveAdmissionFateRecord {
    Unknown {},
    Refused {
        reason: crate::live_ledger::completion::LiveRequestRefusal,
    },
    CancelledWithoutRun {
        reason: meerkat_core::live_execution::request::LiveRequestCancellationReason,
    },
    Admitted {
        receipt: AdmittedLiveExecutionRecord,
    },
    TerminalWithoutDelivery {
        input_id: InputId,
    },
    RepairBlocked {
        input_id: InputId,
        reason: crate::live_ledger::completion::LiveRequestHold,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AdmittedLiveExecutionWire")]
pub struct AdmittedLiveExecutionRecord {
    source: LiveSourceKey,
    input_id: InputId,
    executor: ScopedExecutorBinding,
    grant: ExecutionGrantRef,
    ingress_generation_at_admission: NonZeroU64,
    commit: ExecutionAdmissionCommitRef,
}

impl AdmittedLiveExecutionRecord {
    pub fn new(
        source: LiveSourceKey,
        input_id: InputId,
        executor: ScopedExecutorBinding,
        grant: ExecutionGrantRef,
        ingress_generation_at_admission: NonZeroU64,
        commit: ExecutionAdmissionCommitRef,
    ) -> Result<Self, LiveRequestRecordError> {
        if source.session_id() != &executor.session_id {
            return Err(LiveRequestRecordError::ExecutorSessionMismatch);
        }
        Ok(Self {
            source,
            input_id,
            executor,
            grant,
            ingress_generation_at_admission,
            commit,
        })
    }

    pub fn source(&self) -> &LiveSourceKey {
        &self.source
    }
    pub fn input_id(&self) -> &InputId {
        &self.input_id
    }
    pub fn executor(&self) -> &ScopedExecutorBinding {
        &self.executor
    }
    pub fn grant(&self) -> &ExecutionGrantRef {
        &self.grant
    }
}

/// In-memory handoff from the atomic admission owner, not a deserializable
/// replacement for its persisted record.
///
/// ```compile_fail
/// use meerkat_runtime::live_request::AdmittedLiveExecutionAuthority;
/// let forged = serde_json::from_str::<AdmittedLiveExecutionAuthority>("{}");
/// ```
#[derive(Debug, Clone)]
pub struct AdmittedLiveExecutionAuthority {
    record: AdmittedLiveExecutionRecord,
}

impl AdmittedLiveExecutionAuthority {
    pub fn record(&self) -> &AdmittedLiveExecutionRecord {
        &self.record
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmittedLiveExecutionWire {
    source: LiveSourceKey,
    input_id: InputId,
    executor: ScopedExecutorBinding,
    grant: ExecutionGrantRef,
    ingress_generation_at_admission: NonZeroU64,
    commit: ExecutionAdmissionCommitRef,
}

impl TryFrom<AdmittedLiveExecutionWire> for AdmittedLiveExecutionRecord {
    type Error = LiveRequestRecordError;
    fn try_from(value: AdmittedLiveExecutionWire) -> Result<Self, Self::Error> {
        Self::new(
            value.source,
            value.input_id,
            value.executor,
            value.grant,
            value.ingress_generation_at_admission,
            value.commit,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveCallbackResultEvidence {
    Pending {},
    Complete {
        session_id: SessionId,
        run_id: RunId,
        batch_digest: [u8; 32],
        accepted_payload_digest: [u8; 32],
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveCallbackContinuationAdmission {
    NotSubmitted {},
    Unconfirmed {},
    Admitted {
        input_id: InputId,
        commit: ExecutionAdmissionCommitRef,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveCallbackSuspensionRecord {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub expected_tool_use_ids: Vec<String>,
    pub batch_digest: [u8; 32],
    pub results: LiveCallbackResultEvidence,
    pub continuation: LiveCallbackContinuationAdmission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveRequestRunLink {
    pub input_id: InputId,
    pub run_id: RunId,
    pub scope_id: RunEffectScopeId,
    pub callback: Option<LiveCallbackSuspensionRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveRequestChainTerminal {
    Succeeded,
    Failed,
    Cancelled,
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveRequestChainPhase {
    Queued {},
    Running {},
    SuspendedCallbacks {},
    Held {},
    Final { outcome: LiveRequestChainTerminal },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LiveRequestRunChainWire")]
pub struct LiveRequestRunChainRecord {
    request_id: OperationId,
    admitted: AdmittedLiveExecutionRecord,
    runs: Vec<LiveRequestRunLink>,
    phase: LiveRequestChainPhase,
}

impl LiveRequestRunChainRecord {
    pub fn new(
        request_id: OperationId,
        admitted: AdmittedLiveExecutionRecord,
        runs: Vec<LiveRequestRunLink>,
        phase: LiveRequestChainPhase,
    ) -> Result<Self, LiveRequestRecordError> {
        let mut input_ids = HashSet::new();
        let mut run_ids = HashSet::new();
        let mut scope_ids = HashSet::new();
        let mut previous: Option<&LiveRequestRunLink> = None;
        for run in &runs {
            if !input_ids.insert(&run.input_id)
                || !run_ids.insert(&run.run_id)
                || !scope_ids.insert(run.scope_id)
            {
                return Err(LiveRequestRecordError::RepeatedRunIdentity);
            }
            if let Some(previous) = previous {
                let Some(callback) = &previous.callback else {
                    return Err(LiveRequestRecordError::MissingContinuationLink);
                };
                if !matches!(&callback.continuation,
                    LiveCallbackContinuationAdmission::Admitted { input_id, .. } if input_id == &run.input_id)
                {
                    return Err(LiveRequestRecordError::MissingContinuationLink);
                }
            } else if run.input_id != *admitted.input_id() {
                return Err(LiveRequestRecordError::InitialInputMismatch);
            }
            if let Some(callback) = &run.callback {
                if callback.session_id != *admitted.source().session_id()
                    || callback.run_id != run.run_id
                {
                    return Err(LiveRequestRecordError::CallbackOwnerMismatch);
                }
                let distinct: HashSet<_> = callback.expected_tool_use_ids.iter().collect();
                if distinct.is_empty()
                    || distinct.len() != callback.expected_tool_use_ids.len()
                    || distinct.iter().any(|id| id.is_empty())
                {
                    return Err(LiveRequestRecordError::InvalidCallbackCallSet);
                }
                if !matches!(
                    callback.continuation,
                    LiveCallbackContinuationAdmission::NotSubmitted {}
                ) && !matches!(
                    callback.results,
                    LiveCallbackResultEvidence::Complete { .. }
                ) {
                    return Err(LiveRequestRecordError::ContinuationWithoutResults);
                }
                if let LiveCallbackResultEvidence::Complete {
                    session_id,
                    run_id,
                    batch_digest,
                    ..
                } = &callback.results
                    && (session_id != &callback.session_id
                        || run_id != &callback.run_id
                        || batch_digest != &callback.batch_digest)
                {
                    return Err(LiveRequestRecordError::CallbackOwnerMismatch);
                }
                if let LiveCallbackContinuationAdmission::Admitted { input_id, .. } =
                    &callback.continuation
                    && input_ids.contains(input_id)
                {
                    return Err(LiveRequestRecordError::RepeatedRunIdentity);
                }
            }
            previous = Some(run);
        }
        match (&phase, runs.last()) {
            (LiveRequestChainPhase::Queued {}, Some(_))
            | (
                LiveRequestChainPhase::Running {} | LiveRequestChainPhase::SuspendedCallbacks {},
                None,
            ) => {
                return Err(LiveRequestRecordError::PhaseRunMismatch);
            }
            (LiveRequestChainPhase::Running {}, Some(run)) if run.callback.is_some() => {
                return Err(LiveRequestRecordError::PhaseRunMismatch);
            }
            (LiveRequestChainPhase::SuspendedCallbacks {}, Some(run)) if run.callback.is_none() => {
                return Err(LiveRequestRecordError::PhaseRunMismatch);
            }
            (
                LiveRequestChainPhase::Final {
                    outcome: LiveRequestChainTerminal::Succeeded | LiveRequestChainTerminal::Failed,
                },
                last,
            ) if last.is_none_or(|run| run.callback.is_some()) => {
                return Err(LiveRequestRecordError::PhaseRunMismatch);
            }
            _ => {}
        }
        Ok(Self {
            request_id,
            admitted,
            runs,
            phase,
        })
    }

    pub fn request_id(&self) -> &OperationId {
        &self.request_id
    }
    pub fn admitted(&self) -> &AdmittedLiveExecutionRecord {
        &self.admitted
    }
    pub fn runs(&self) -> &[LiveRequestRunLink] {
        &self.runs
    }
    pub fn phase(&self) -> &LiveRequestChainPhase {
        &self.phase
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveRequestRunChainWire {
    request_id: OperationId,
    admitted: AdmittedLiveExecutionRecord,
    runs: Vec<LiveRequestRunLink>,
    phase: LiveRequestChainPhase,
}

impl TryFrom<LiveRequestRunChainWire> for LiveRequestRunChainRecord {
    type Error = LiveRequestRecordError;
    fn try_from(value: LiveRequestRunChainWire) -> Result<Self, Self::Error> {
        Self::new(value.request_id, value.admitted, value.runs, value.phase)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveRequestRecordError {
    #[error("live source and admitted executor name different sessions")]
    ExecutorSessionMismatch,
    #[error("live request chain repeats an input, run or effect scope")]
    RepeatedRunIdentity,
    #[error("live request first run does not own its admitted input")]
    InitialInputMismatch,
    #[error("live callback suspension names another session or run")]
    CallbackOwnerMismatch,
    #[error("live callback continuation lacks the exact admitted predecessor link")]
    MissingContinuationLink,
    #[error("live callback suspension requires distinct nonempty call identifiers")]
    InvalidCallbackCallSet,
    #[error("live callback continuation admission lacks complete ordinary callback results")]
    ContinuationWithoutResults,
    #[error("live request phase does not match its retained run and suspension content")]
    PhaseRunMismatch,
}
