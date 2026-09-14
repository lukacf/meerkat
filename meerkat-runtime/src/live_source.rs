//! Immutable source reservation images and exact replay comparison.
//!
//! These records do not reserve a source or mint an admitted input. The
//! generated/transactional owner must look up the stable key before capturing
//! newer observations, and commit the record and frontier together.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::num::NonZeroU64;

use crate::live_ledger::transcript::LiveHeadReference;
use crate::live_request::AdmittedLiveExecutionRecord;
use crate::store::live_read::LiveCompositeRead;
use crate::store::{RuntimeSessionAuthority, RuntimeSessionPersistenceProfile};
use meerkat_core::execution_scope::ExecutionGrantRef;
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_execution::evidence::{LiveObservationInterval, LiveRequestEvidence};
use meerkat_core::live_execution::request::{
    LiveRequestCancelIntent, LiveRequestCancellationReason, LiveSourceIdentity, LiveSourceKey,
};
use meerkat_core::{SessionId, ops::OperationId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveSourceFingerprint([u8; 32]);

impl LiveSourceFingerprint {
    pub fn client_delegation(offset_ms: f64) -> Result<Self, LiveSourceRecordError> {
        if !offset_ms.is_finite() || offset_ms < 0.0 {
            return Err(LiveSourceRecordError::InvalidOffset);
        }
        let mut digest = Sha256::new();
        digest.update(b"meerkat.live-source-client.v1\0");
        digest.update(offset_ms.to_bits().to_be_bytes());
        Ok(Self(digest.finalize().into()))
    }

    pub fn function_call(name: &str, arguments: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"meerkat.live-source-function.v1\0");
        for value in [name, arguments] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        Self(digest.finalize().into())
    }

    pub fn application_request(interval: LiveObservationInterval) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"meerkat.live-source-application.v1\0");
        digest.update(interval.after().to_be_bytes());
        digest.update(interval.through().to_be_bytes());
        Self(digest.finalize().into())
    }
}

/// Persistable comparison material, never a reconstruction of store-issued
/// atomic read authority.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveActorContextReference {
    pub session_id: SessionId,
    pub profile: RuntimeSessionPersistenceProfile,
    pub revision: NonZeroU64,
    pub token: String,
}

impl std::fmt::Debug for LiveActorContextReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveActorContextReference")
            .field("session_id", &self.session_id)
            .field("profile", &self.profile)
            .field("revision", &self.revision)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

impl LiveActorContextReference {
    pub(crate) fn matches_authority(&self, authority: &RuntimeSessionAuthority) -> bool {
        let token = match authority {
            RuntimeSessionAuthority::WholeBlob(actor) => actor.blob_sha256(),
            RuntimeSessionAuthority::HeadCanonical(actor) => actor.committed_head_token(),
        };
        self.session_id == *authority.session_id()
            && self.profile == authority.profile()
            && self.revision.get() == authority.store_revision()
            && self.token == token
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveSourceReadCoverage {
    CompleteToCapturedHead,
    WindowContinues,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSourceContextReference {
    pub actor: LiveActorContextReference,
    pub live_head: LiveHeadReference,
    pub channel_id: LiveChannelId,
    pub interval: LiveObservationInterval,
    pub read_coverage: LiveSourceReadCoverage,
    pub record_window_digest: [u8; 32],
}

impl LiveSourceContextReference {
    pub fn from_composite(
        read: &LiveCompositeRead,
        source: &LiveSourceKey,
        interval: LiveObservationInterval,
    ) -> Result<Self, LiveSourceRecordError> {
        let authority = read.authority();
        if authority.actor().session_id() != source.session_id()
            || authority.selection().channel_id() != Some(source.channel_id())
            || authority.selection().after_sequence() != interval.after()
        {
            return Err(LiveSourceRecordError::ContextMismatch);
        }
        let head = authority
            .live_head()
            .ok_or(LiveSourceRecordError::MissingLiveHead)?;
        if interval.through() > head.event_count {
            return Err(LiveSourceRecordError::PastDurableWatermark);
        }
        let token = match authority.actor() {
            RuntimeSessionAuthority::WholeBlob(actor) => actor.blob_sha256(),
            RuntimeSessionAuthority::HeadCanonical(actor) => actor.committed_head_token(),
        };
        Ok(Self {
            actor: LiveActorContextReference {
                session_id: source.session_id().clone(),
                profile: authority.actor().profile(),
                revision: NonZeroU64::new(authority.actor().store_revision())
                    .ok_or(LiveSourceRecordError::InvalidContextToken)?,
                token: token.to_owned(),
            },
            live_head: head.clone(),
            channel_id: source.channel_id().clone(),
            interval,
            read_coverage: if read.has_more() {
                LiveSourceReadCoverage::WindowContinues
            } else {
                LiveSourceReadCoverage::CompleteToCapturedHead
            },
            record_window_digest: *authority.record_window_digest(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveSourceRefusal {
    Empty,
    Gap,
    Budget,
    Permission,
    IngressClosed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveSourceDisposition {
    Reserved {},
    Refused {
        reason: LiveSourceRefusal,
    },
    Admitted {
        receipt: Box<AdmittedLiveExecutionRecord>,
    },
    CancelledWithoutRun {
        reason: LiveRequestCancellationReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LiveSourceReservationParts")]
pub struct LiveSourceReservationRecord {
    source: LiveSourceKey,
    request_id: OperationId,
    fingerprint: LiveSourceFingerprint,
    context: LiveSourceContextReference,
    frozen_request: Option<LiveRequestEvidence>,
    grant: Option<ExecutionGrantRef>,
    cancellation: Option<LiveRequestCancellationReason>,
    disposition: LiveSourceDisposition,
}

impl LiveSourceReservationRecord {
    pub fn source(&self) -> &LiveSourceKey {
        &self.source
    }
    pub fn request_id(&self) -> &OperationId {
        &self.request_id
    }
    pub fn context(&self) -> &LiveSourceContextReference {
        &self.context
    }
    pub fn disposition(&self) -> &LiveSourceDisposition {
        &self.disposition
    }
    pub fn frozen_request(&self) -> Option<&LiveRequestEvidence> {
        self.frozen_request.as_ref()
    }
    pub const fn reserved_frontier(&self) -> u64 {
        self.context.interval.through()
    }

    /// Return this exact image on replay. There is deliberately no parameter
    /// for new transcript text, a later watermark, or newly granted permission.
    pub fn replay(
        &self,
        source: &LiveSourceKey,
        fingerprint: LiveSourceFingerprint,
    ) -> Result<&Self, LiveSourceRecordError> {
        if source != &self.source {
            return Err(LiveSourceRecordError::SourceMismatch);
        }
        if fingerprint != self.fingerprint {
            return Err(LiveSourceRecordError::SourcePayloadConflict);
        }
        Ok(self)
    }

    /// Payload-image charge only; the source entry and any separately stored
    /// variable columns must be included by the accepting transaction.
    pub fn encoded_charge(
        &self,
    ) -> Result<crate::live_resources::LiveResourceCharge, LiveSourceRecordError> {
        let bytes = meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::encode_ledger_record(self)
            .map_err(|_| LiveSourceRecordError::EncodingBoundExceeded)?;
        crate::live_resources::LiveResourceCharge::for_encoded_record(&bytes)
            .map_err(|_| LiveSourceRecordError::EncodingBoundExceeded)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSourceReservationParts {
    pub source: LiveSourceKey,
    pub request_id: OperationId,
    pub fingerprint: LiveSourceFingerprint,
    pub context: LiveSourceContextReference,
    pub frozen_request: Option<LiveRequestEvidence>,
    pub grant: Option<ExecutionGrantRef>,
    pub cancellation: Option<LiveRequestCancellationReason>,
    pub disposition: LiveSourceDisposition,
}

impl TryFrom<LiveSourceReservationParts> for LiveSourceReservationRecord {
    type Error = LiveSourceRecordError;
    fn try_from(value: LiveSourceReservationParts) -> Result<Self, Self::Error> {
        if value.context.actor.session_id != *value.source.session_id()
            || value.context.live_head.session_id != *value.source.session_id()
            || value.context.channel_id != *value.source.channel_id()
        {
            return Err(LiveSourceRecordError::ContextMismatch);
        }
        if value.context.actor.token.is_empty() || value.context.actor.token.len() > 256 {
            return Err(LiveSourceRecordError::InvalidContextToken);
        }
        if value.context.live_head.generation == 0 || value.context.live_head.revision == 0 {
            return Err(LiveSourceRecordError::InvalidContextToken);
        }
        if value.context.interval.through() > value.context.live_head.event_count {
            return Err(LiveSourceRecordError::PastDurableWatermark);
        }
        if matches!(
            value.source.source(),
            LiveSourceIdentity::ClientDelegation { .. }
        ) && value.context.interval.through() != value.context.live_head.event_count
        {
            return Err(LiveSourceRecordError::IncompleteSnapshot);
        }
        if matches!(
            value.source.source(),
            LiveSourceIdentity::ApplicationRequest { .. }
        ) && value.fingerprint
            != LiveSourceFingerprint::application_request(value.context.interval)
        {
            return Err(LiveSourceRecordError::SourcePayloadConflict);
        }
        if matches!(
            value.source.source(),
            LiveSourceIdentity::ClientDelegation { .. }
        ) && value.context.interval.is_empty()
            && matches!(
                value.disposition,
                LiveSourceDisposition::Reserved {} | LiveSourceDisposition::Admitted { .. }
            )
        {
            return Err(LiveSourceRecordError::EmptyAutomaticSnapshot);
        }
        if matches!(
            value.disposition,
            LiveSourceDisposition::Refused {
                reason: LiveSourceRefusal::Empty
            }
        ) && !value.context.interval.is_empty()
        {
            return Err(LiveSourceRecordError::NonemptyEmptyRefusal);
        }
        if let Some(evidence) = &value.frozen_request {
            match (value.source.source(), evidence) {
                (
                    LiveSourceIdentity::ClientDelegation { .. }
                    | LiveSourceIdentity::ApplicationRequest { .. },
                    LiveRequestEvidence::ApplicationSnapshot { observations, .. },
                ) if observations == &value.context.interval => {}
                (
                    LiveSourceIdentity::FunctionCall { .. },
                    LiveRequestEvidence::StructuredFunctionRequest { .. },
                ) => {}
                _ => return Err(LiveSourceRecordError::EvidenceMismatch),
            }
        }
        if matches!(
            value.disposition,
            LiveSourceDisposition::Reserved {} | LiveSourceDisposition::Admitted { .. }
        ) && (value.frozen_request.is_none() || value.grant.is_none())
        {
            return Err(LiveSourceRecordError::MissingRequestOrGrant);
        }
        if matches!(
            value.frozen_request,
            Some(LiveRequestEvidence::ApplicationSnapshot { .. })
        ) && matches!(
            value.disposition,
            LiveSourceDisposition::Reserved {} | LiveSourceDisposition::Admitted { .. }
        ) && value.context.read_coverage != LiveSourceReadCoverage::CompleteToCapturedHead
        {
            return Err(LiveSourceRecordError::IncompleteSnapshot);
        }
        if let LiveSourceDisposition::Admitted { receipt } = &value.disposition
            && (receipt.source() != &value.source || Some(receipt.grant()) != value.grant.as_ref())
        {
            return Err(LiveSourceRecordError::AdmissionMismatch);
        }
        Ok(Self {
            source: value.source,
            request_id: value.request_id,
            fingerprint: value.fingerprint,
            context: value.context,
            frozen_request: value.frozen_request,
            grant: value.grant,
            cancellation: value.cancellation,
            disposition: value.disposition,
        })
    }
}

/// A cancellation may precede reservation itself. Such an entry has no
/// invented snapshot, frontier, input or run identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveSourceEntryRecord {
    CancellationOnly {
        intent: LiveRequestCancelIntent,
    },
    Reservation {
        record: Box<LiveSourceReservationRecord>,
    },
}

impl LiveSourceEntryRecord {
    pub(crate) fn preserves_frozen_content(&self, replacement: &Self) -> bool {
        match (self, replacement) {
            (Self::CancellationOnly { intent }, Self::CancellationOnly { intent: next }) => {
                intent == next
            }
            (Self::Reservation { record: old }, Self::Reservation { record: new }) => {
                old.source == new.source
                    && old.request_id == new.request_id
                    && old.fingerprint == new.fingerprint
                    && old.context == new.context
                    && old.frozen_request == new.frozen_request
                    && old.grant == new.grant
                    && old
                        .cancellation
                        .as_ref()
                        .is_none_or(|reason| new.cancellation.as_ref() == Some(reason))
                    && match &old.disposition {
                        LiveSourceDisposition::Admitted { receipt } => matches!(
                            &new.disposition, LiveSourceDisposition::Admitted { receipt: next } if next == receipt
                        ),
                        LiveSourceDisposition::Reserved {}
                        | LiveSourceDisposition::Refused { .. }
                        | LiveSourceDisposition::CancelledWithoutRun { .. } => true,
                    }
            }
            _ => false,
        }
    }

    pub fn source(&self) -> &LiveSourceKey {
        match self {
            Self::CancellationOnly { intent } => &intent.source,
            Self::Reservation { record } => record.source(),
        }
    }

    /// Exact encoded source-envelope charge. A store must additionally charge
    /// any separately persisted variable columns before accepting the write.
    pub fn encoded_charge(
        &self,
    ) -> Result<crate::live_resources::LiveResourceCharge, LiveSourceRecordError> {
        let bytes = meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::encode_ledger_record(self)
            .map_err(|_| LiveSourceRecordError::EncodingBoundExceeded)?;
        crate::live_resources::LiveResourceCharge::for_encoded_record(&bytes)
            .map_err(|_| LiveSourceRecordError::EncodingBoundExceeded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveSourceRecordError {
    #[error("automatic client work cannot reserve an empty observation prefix")]
    EmptyAutomaticSnapshot,
    #[error("an empty-prefix refusal must retain a zero-width interval")]
    NonemptyEmptyRefusal,
    #[error("live source offset must be finite and nonnegative")]
    InvalidOffset,
    #[error("live source key differs from the retained source")]
    SourceMismatch,
    #[error("live source replay changed immutable provider/application metadata")]
    SourcePayloadConflict,
    #[error("live source context does not bind this source session/channel/selection")]
    ContextMismatch,
    #[error("live source snapshot requires a committed live head")]
    MissingLiveHead,
    #[error("live source interval exceeds the captured durable watermark")]
    PastDurableWatermark,
    #[error("live source context token is empty or exceeds its bound")]
    InvalidContextToken,
    #[error("live source evidence does not match its source kind and interval")]
    EvidenceMismatch,
    #[error("reserved live work requires frozen request content and a separate grant reference")]
    MissingRequestOrGrant,
    #[error("live source cannot execute a truncated observation window")]
    IncompleteSnapshot,
    #[error("live source admission receipt belongs to another source or grant")]
    AdmissionMismatch,
    #[error("live source record exceeds its encoded storage bound")]
    EncodingBoundExceeded,
}
