//! Lossless projection of the released tracker's facts. The SDK tracker owns
//! attribution and readiness; Meerkat's generated request owner owns admission.

use meerkat_core::live_execution::backend::{
    LiveBackendCandidates, LiveBackendOwnership, LiveBackendResponseKey, LiveBackendScope,
    LiveCompletedFunctionView,
};
use meerkat_core::live_execution::request::{LiveDelegationAttribution, LiveProviderReference};
use oai_rt_rs::live::{
    Field, FunctionCall, FunctionCallTracker, ResponseAttribution, ResponseKey, ServerEvent,
    ServerFrame,
};

pub fn observe_backend_scope(
    tracker: &mut FunctionCallTracker,
    frame: &ServerFrame,
) -> Result<LiveBackendScope, LiveBackendProjectionError> {
    let ServerEvent::Response { delegation_id, .. } = &frame.event else {
        return Err(LiveBackendProjectionError::NotBackendFrame);
    };
    let envelope_attribution = match delegation_id {
        Field::Absent => LiveDelegationAttribution::Absent {},
        Field::Null => LiveDelegationAttribution::ExplicitNull {},
        Field::Value(value) => LiveDelegationAttribution::Known {
            delegation: reference(value)?,
        },
    };
    let delegation = match delegation_id {
        Field::Value(value) => Some(value.as_str()),
        Field::Absent | Field::Null => None,
    };
    let event = frame
        .response_event()
        .map_err(|_| {
            tracker.mark_uncertain(delegation);
            LiveBackendProjectionError::InvalidResponseEvent
        })?
        .ok_or(LiveBackendProjectionError::NotBackendFrame)?;
    let attribution = tracker
        .observe(delegation, &event)
        .map_err(|_| LiveBackendProjectionError::ConflictingResponseEvidence)?;
    let ownership = match &attribution {
        ResponseAttribution::Owned(key) => LiveBackendOwnership::Owned {
            response: response_key(key)?,
        },
        ResponseAttribution::Unowned => LiveBackendOwnership::Unowned {},
        ResponseAttribution::Ambiguous(keys) => {
            if !(2..=32).contains(&keys.len()) {
                return Err(LiveBackendProjectionError::InvalidAmbiguity);
            }
            let candidates = keys
                .iter()
                .map(response_key)
                .collect::<Result<Vec<_>, _>>()?;
            LiveBackendOwnership::Ambiguous {
                candidates: LiveBackendCandidates::new(candidates)
                    .map_err(|_| LiveBackendProjectionError::InvalidAmbiguity)?,
            }
        }
    };
    Ok(LiveBackendScope {
        envelope_attribution,
        ownership,
    })
}

fn reference(value: &str) -> Result<LiveProviderReference, LiveBackendProjectionError> {
    if value.len() > 128 {
        return Err(LiveBackendProjectionError::IdentityTooLarge);
    }
    LiveProviderReference::new(value).map_err(|_| LiveBackendProjectionError::InvalidIdentity)
}

fn response_key(key: &ResponseKey) -> Result<LiveBackendResponseKey, LiveBackendProjectionError> {
    Ok(LiveBackendResponseKey {
        response: reference(&key.response_id)?,
        delegation: key.delegation_id.as_deref().map(reference).transpose()?,
    })
}

pub struct LiveReadyFunctionBatch<'a> {
    response: LiveBackendResponseKey,
    calls: &'a [FunctionCall],
}

impl LiveReadyFunctionBatch<'_> {
    pub fn digest(
        &self,
    ) -> Result<
        meerkat_core::live_execution::backend::LiveFunctionBatchDigest,
        meerkat_core::live_execution::backend::LiveBackendValueError,
    > {
        meerkat_core::live_execution::backend::LiveFunctionBatchDigest::of(
            &self.response,
            self.calls(),
        )
    }

    pub fn response(&self) -> &LiveBackendResponseKey {
        &self.response
    }

    pub fn calls(&self) -> impl ExactSizeIterator<Item = LiveCompletedFunctionView<'_>> {
        self.calls.iter().map(|call| LiveCompletedFunctionView {
            call_id: &call.call_id,
            name: &call.name,
            arguments: &call.args,
        })
    }
}

impl std::fmt::Debug for LiveReadyFunctionBatch<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveReadyFunctionBatch")
            .field("response", &self.response)
            .field("call_count", &self.calls.len())
            .finish()
    }
}

/// `None` is unfinished/unsafe, not an empty complete batch. No payload clone,
/// argument parsing, execution permission, or continuation claim happens here.
pub fn ready_function_batch<'a>(
    tracker: &'a FunctionCallTracker,
    key: &ResponseKey,
) -> Result<Option<LiveReadyFunctionBatch<'a>>, LiveBackendProjectionError> {
    let Some(calls) = tracker.ready_calls(key) else {
        return Ok(None);
    };
    if calls.len() > 128 {
        return Err(LiveBackendProjectionError::CallCountExceeded);
    }
    Ok(Some(LiveReadyFunctionBatch {
        response: response_key(key)?,
        calls,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveBackendProjectionError {
    #[error("live backend projection requires a response frame")]
    NotBackendFrame,
    #[error("live backend event does not satisfy the released response schema")]
    InvalidResponseEvent,
    #[error("live backend event conflicts with tracked response evidence")]
    ConflictingResponseEvidence,
    #[error("live backend identity is empty")]
    InvalidIdentity,
    #[error("live backend identity exceeds its bounded projection")]
    IdentityTooLarge,
    #[error("live backend ambiguity requires 2-32 distinct responses")]
    InvalidAmbiguity,
    #[error("live backend batch exceeds 128 completed calls")]
    CallCountExceeded,
}
