//! Original-task continuation: deliver a result to its owner as a new durable
//! input, under a host-owned exact key.
//!
//! A continuation is committed into the owner's runtime delivery inbox
//! together with the first binding of `(stable owner, key)`, so a replay of the
//! same submission returns the original receipt byte for byte (even after the
//! owner was repointed or retired), and the same key with other content is a
//! conflict that leaves the original untouched. Status is read from the store
//! only.

use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_core::lifecycle::InputId;
use meerkat_runtime::{
    ContinuationAdmission, ContinuationAdmissionOutcome, ContinuationAdmissionTransition,
    ContinuationKeyClaim, KeyedSubmitOutcome, LogicalRuntimeId, MemberDeliveryAddress,
    RuntimeDeliveryError, RuntimeDeliveryId, RuntimeDeliveryInbox, RuntimeDeliveryKind,
    RuntimeDeliveryStatus, RuntimeDeliverySubmission, delivery_digest_hex,
};
use serde::{Deserialize, Serialize};

/// Longest accepted continuation key, in bytes.
pub const MAX_CONTINUATION_KEY_BYTES: usize = 512;

/// A host-owned exact continuation key: non-empty, at most
/// [`MAX_CONTINUATION_KEY_BYTES`], free of control characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContinuationKey(String);

impl ContinuationKey {
    pub fn new(key: impl Into<String>) -> Result<Self, ContinuationSubmitError> {
        let key = key.into();
        if key.is_empty()
            || key.len() > MAX_CONTINUATION_KEY_BYTES
            || key.chars().any(char::is_control)
        {
            return Err(ContinuationSubmitError::Invalid(format!(
                "continuation keys are 1..={MAX_CONTINUATION_KEY_BYTES} bytes without control characters"
            )));
        }
        Ok(Self(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ContinuationKey {
    type Error = ContinuationSubmitError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ContinuationKey> for String {
    fn from(key: ContinuationKey) -> Self {
        key.0
    }
}

/// Who a continuation is for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContinuationOwner {
    /// A mob member, by stable identity. A new key binds the member's
    /// incarnation current at its first submission; the generation is never
    /// supplied by the caller and never refreshed by a later submission.
    Member { mob_id: String, identity: String },
    /// A standalone session.
    Session { session_id: SessionId },
}

impl ContinuationOwner {
    /// The canonical stable-owner key the key ledger binds under.
    fn ledger_owner(&self) -> String {
        fn escape(component: &str) -> String {
            component.replace('%', "%25").replace(':', "%3A")
        }
        match self {
            Self::Member { mob_id, identity } => {
                format!("member:{}:{}", escape(mob_id), escape(identity))
            }
            Self::Session { session_id } => format!("session:{session_id}"),
        }
    }
}

/// Who produced the result a continuation carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContinuationProducer {
    /// A host-side operation in the host's own namespace.
    Host { namespace: String },
    /// A `fork_off` child run.
    ForkOff,
    /// A council run.
    Council,
}

/// A reference to an immutable result; the reference is the contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationResultRef {
    pub producer: ContinuationProducer,
    /// Job id, run id or host operation id.
    pub producer_id: String,
    /// Digest of the immutable result bytes.
    pub result_digest: String,
    /// Optional bounded display text; never a substitute for the reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// How a continuation joins an owner that is already running.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationHandling {
    /// After the current turn (default).
    #[default]
    Queue,
    /// Into the current turn.
    Steer,
}

/// The committed payload of a continuation row: the delivery, and the
/// retained work of the run that dispatched it when a Meerkat producer
/// committed it from its job's record.
#[derive(Serialize, Deserialize)]
struct ContinuationRow {
    #[serde(flatten)]
    delivery: ContinuationDelivery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retained_work: Option<meerkat_core::retained_work::RetainedWorkIdentity>,
}

/// What a continuation delivers into its owner's conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContinuationBody {
    /// Content for the owner's next turn.
    Content {
        content: meerkat_core::types::ContentInput,
    },
    /// A durable system notice, admitted exactly as a detached job's
    /// completion record (`PromptInput::detached_job_completed`): fork_off
    /// and council outcomes keep the transcript they always had. It holds
    /// what the admitted append carries (kind, body, blocks) and no
    /// timestamp, so the same outcome is the same body however often it is
    /// submitted ([`ContinuationBody::notice`]).
    Notice {
        notice_kind: meerkat_core::types::SystemNoticeKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        blocks: Vec<meerkat_core::types::SystemNoticeBlock>,
    },
}

impl ContinuationBody {
    /// The notice body for `notice`. Its creation time is not kept: the
    /// admitted append never carries it.
    pub fn notice(notice: meerkat_core::types::SystemNoticeMessage) -> Self {
        Self::Notice {
            notice_kind: notice.kind,
            body: notice.body,
            blocks: notice.blocks,
        }
    }
}

impl From<meerkat_core::types::ContentInput> for ContinuationBody {
    fn from(content: meerkat_core::types::ContentInput) -> Self {
        Self::Content { content }
    }
}

impl From<&str> for ContinuationBody {
    fn from(text: &str) -> Self {
        Self::Content {
            content: text.into(),
        }
    }
}

impl From<String> for ContinuationBody {
    fn from(text: String) -> Self {
        Self::Content {
            content: text.into(),
        }
    }
}

/// One continuation, carrying its result at submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationDelivery {
    pub key: ContinuationKey,
    pub result: ContinuationResultRef,
    pub body: ContinuationBody,
    #[serde(default)]
    pub handling: ContinuationHandling,
}

impl ContinuationDelivery {
    /// The admission idempotency key for this delivery.
    ///
    /// `ForkOff` and `Council` results admit under their live key
    /// (`{tool}:{job_id}`, the continuation key itself), so a completion
    /// delivered live before an upgrade and re-submitted as a continuation
    /// dedupes against the earlier input. Host results admit under the
    /// hash-namespaced delivery id, which a host-chosen key can never collide
    /// with.
    pub fn admission_key(&self, delivery_id: &RuntimeDeliveryId) -> String {
        match self.result.producer {
            ContinuationProducer::ForkOff | ContinuationProducer::Council => {
                self.key.as_str().to_string()
            }
            ContinuationProducer::Host { .. } => delivery_id.as_str().to_string(),
        }
    }
}

/// The inbox commit receipt (fact 1): the obligation exists durably.
///
/// Byte-identical on every replay of the same submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationReceipt {
    pub owner: ContinuationOwner,
    /// The delivery address the first submission bound.
    pub address: String,
    pub key: ContinuationKey,
    pub delivery_id: String,
    pub submission_digest: String,
    pub committed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContinuationSubmitError {
    /// The same owner and key were committed with other content. Only this
    /// submission is rejected; the existing continuation keeps its status.
    #[error("continuation key {} is already bound to other content", existing.key.as_str())]
    Conflict { existing: Box<ContinuationReceipt> },
    /// The owner has no live incarnation to bind a new key to.
    #[error("the continuation owner is retired")]
    OwnerRetired,
    #[error("invalid continuation: {0}")]
    Invalid(String),
    /// The store or the owner's resolution could not be read. Never an implied
    /// non-commit.
    #[error("continuation authority unavailable: {0}")]
    Unavailable(String),
    /// The owner's runtime admits work through a native work authorization
    /// host, and this continuation carries no native work binding it could be
    /// admitted under. Nothing was committed; the owner's turns and session
    /// are untouched.
    #[error(
        "the owner's runtime has a native work authorization host and this continuation carries \
         no native work binding"
    )]
    NoAdmissibleWorkBinding,
}

/// Why a committed continuation will never be admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrandedCause {
    OwnerRetired,
}

/// Authoritative continuation status, read from the store only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationStatus {
    /// Permits exactly one thing: resubmitting the identical owner, key and
    /// delivery. It authorizes no new key and no new producer execution.
    NotCommitted,
    /// Committed and not yet acknowledged. `admitted` is set in the window
    /// after the input reached the owner and before the acknowledgement.
    Pending {
        receipt: ContinuationReceipt,
        sequence: u64,
        admitted: Option<(SessionId, InputId)>,
    },
    /// Durably admitted as an input to `session` (fact 2).
    Applied {
        receipt: ContinuationReceipt,
        session: SessionId,
        input: InputId,
    },
    /// Committed, never admitted, and the owner is retired.
    Stranded {
        receipt: ContinuationReceipt,
        cause: StrandedCause,
    },
    /// Settled as refused: a terminal policy outcome, never admitted. The
    /// owner's later deliveries proceeded past it.
    Refused {
        receipt: ContinuationReceipt,
        sequence: u64,
        reason: meerkat_runtime::RuntimeDeliveryRefusalReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContinuationStatusError {
    #[error("continuation status unavailable: {0}")]
    Unavailable(String),
    /// The store holds facts that cannot all be true.
    #[error("continuation authority is inconsistent: {0}")]
    Inconsistent(String),
}

/// The state of the incarnation behind a delivery address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressResolution {
    /// Served by this session now.
    Session(SessionId),
    /// Live but not served by a session at the moment.
    NotServed,
    /// The incarnation is retired; it never serves again.
    Retired,
}

/// Resolves continuation owners and delivery addresses. Hosts with mobs
/// resolve members through their roster.
#[async_trait::async_trait]
pub trait ContinuationAddressResolver: Send + Sync {
    /// The address a NEW key for `owner` binds to: the owner's current
    /// incarnation. `Ok(None)` when it has none (retired).
    async fn current_address(
        &self,
        owner: &ContinuationOwner,
    ) -> Result<Option<LogicalRuntimeId>, String>;

    /// The incarnation state behind an address.
    async fn resolve_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<AddressResolution, String>;
}

/// Resolves standalone-session owners only: a session's address is its own
/// runtime, served by that session.
#[derive(Debug, Default, Clone, Copy)]
pub struct SessionAddressResolver;

#[async_trait::async_trait]
impl ContinuationAddressResolver for SessionAddressResolver {
    async fn current_address(
        &self,
        owner: &ContinuationOwner,
    ) -> Result<Option<LogicalRuntimeId>, String> {
        match owner {
            ContinuationOwner::Session { session_id } => {
                Ok(Some(LogicalRuntimeId::for_session(session_id)))
            }
            ContinuationOwner::Member { .. } => {
                Err("member owners need a mob-aware continuation resolver".into())
            }
        }
    }

    async fn resolve_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<AddressResolution, String> {
        if address
            .member_address()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Err("member addresses need a mob-aware continuation resolver".into());
        }
        let session = address
            .0
            .strip_prefix("rt:session:")
            .and_then(|raw| SessionId::parse(raw).ok())
            .ok_or_else(|| format!("{address} is not a session delivery address"))?;
        Ok(AddressResolution::Session(session))
    }
}

/// Submits continuations and reads their status.
#[derive(Clone)]
pub struct ContinuationOwnerService {
    inbox: RuntimeDeliveryInbox,
    resolver: Arc<dyn ContinuationAddressResolver>,
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
}

impl std::fmt::Debug for ContinuationOwnerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContinuationOwnerService")
            .finish_non_exhaustive()
    }
}

fn unavailable(error: impl std::fmt::Display) -> ContinuationSubmitError {
    ContinuationSubmitError::Unavailable(error.to_string())
}

impl ContinuationOwnerService {
    /// A service for owners served by `runtime`. When `runtime` has a
    /// native work authorization host installed, [`Self::submit`] refuses
    /// with [`ContinuationSubmitError::NoAdmissibleWorkBinding`] before
    /// committing anything (no key claim, no row), since a continuation
    /// carries no native work binding yet.
    pub fn new(
        inbox: RuntimeDeliveryInbox,
        resolver: Arc<dyn ContinuationAddressResolver>,
        runtime: Arc<meerkat_runtime::MeerkatMachine>,
    ) -> Self {
        Self {
            inbox,
            resolver,
            runtime,
        }
    }

    fn submission_digest(
        owner: &ContinuationOwner,
        delivery: &ContinuationDelivery,
        retained_work: Option<&meerkat_core::retained_work::RetainedWorkIdentity>,
    ) -> Result<String, ContinuationSubmitError> {
        let owner = serde_json::to_vec(owner).map_err(unavailable)?;
        let delivery = serde_json::to_vec(delivery).map_err(unavailable)?;
        let mut parts: Vec<&[u8]> = vec![b"continuation-submission", &owner, &delivery];
        let retained_work = retained_work
            .map(serde_json::to_vec)
            .transpose()
            .map_err(unavailable)?;
        if let Some(retained_work) = retained_work.as_deref() {
            parts.push(b"retained-work");
            parts.push(retained_work);
        }
        Ok(delivery_digest_hex(&parts))
    }

    /// The delivery id of a key first bound at `address`: a domain-separated
    /// hash, so it can never equal another delivery family's id.
    fn delivery_id(
        address: &LogicalRuntimeId,
        key: &ContinuationKey,
    ) -> Result<RuntimeDeliveryId, ContinuationSubmitError> {
        RuntimeDeliveryId::new(format!(
            "continuation:{}",
            delivery_digest_hex(&[
                b"continuation-delivery",
                address.0.as_bytes(),
                key.as_str().as_bytes()
            ])
        ))
        .map_err(|error| ContinuationSubmitError::Invalid(error.to_string()))
    }

    fn receipt(
        owner: &ContinuationOwner,
        key: &ContinuationKey,
        binding: &meerkat_runtime::ContinuationKeyBinding,
    ) -> ContinuationReceipt {
        ContinuationReceipt {
            owner: owner.clone(),
            address: binding.address.0.clone(),
            key: key.clone(),
            delivery_id: binding.delivery_id.clone(),
            submission_digest: binding.submission_digest.clone(),
            committed_at_ms: binding.committed_at_ms,
        }
    }

    fn replay_or_conflict(
        owner: &ContinuationOwner,
        key: &ContinuationKey,
        binding: &meerkat_runtime::ContinuationKeyBinding,
        digest: &str,
    ) -> Result<ContinuationReceipt, ContinuationSubmitError> {
        let receipt = Self::receipt(owner, key, binding);
        if binding.submission_digest == digest {
            Ok(receipt)
        } else {
            Err(ContinuationSubmitError::Conflict {
                existing: Box::new(receipt),
            })
        }
    }

    /// Commit a continuation for `owner`, or replay the original receipt.
    ///
    /// Replay is decided from the key ledger before the owner's liveness, so
    /// a replay after retirement still returns the original receipt.
    pub async fn submit(
        &self,
        owner: &ContinuationOwner,
        delivery: ContinuationDelivery,
        committed_at_ms: u64,
    ) -> Result<ContinuationReceipt, ContinuationSubmitError> {
        if self.runtime.has_native_work_authorization_host() {
            return Err(ContinuationSubmitError::NoAdmissibleWorkBinding);
        }
        self.commit(owner, delivery, None, committed_at_ms).await
    }

    async fn commit(
        &self,
        owner: &ContinuationOwner,
        delivery: ContinuationDelivery,
        retained_work: Option<meerkat_core::retained_work::RetainedWorkIdentity>,
        committed_at_ms: u64,
    ) -> Result<ContinuationReceipt, ContinuationSubmitError> {
        let ledger_owner = owner.ledger_owner();
        let digest = Self::submission_digest(owner, &delivery, retained_work.as_ref())?;
        if let Some(binding) = self
            .inbox
            .continuation_key_binding(&ledger_owner, delivery.key.as_str())
            .await
            .map_err(unavailable)?
        {
            return Self::replay_or_conflict(owner, &delivery.key, &binding, &digest);
        }
        let Some(address) = self
            .resolver
            .current_address(owner)
            .await
            .map_err(ContinuationSubmitError::Unavailable)?
        else {
            return Err(ContinuationSubmitError::OwnerRetired);
        };
        let delivery_id = Self::delivery_id(&address, &delivery.key)?;
        let key = delivery.key.clone();
        let source = match &delivery.result.producer {
            ContinuationProducer::Host { namespace } => format!("host:{namespace}"),
            ContinuationProducer::ForkOff => "fork_off".to_string(),
            ContinuationProducer::Council => "council".to_string(),
        };
        let payload = serde_json::to_vec(&ContinuationRow {
            delivery,
            retained_work,
        })
        .map_err(unavailable)?;
        let submission = RuntimeDeliverySubmission::new(
            delivery_id,
            RuntimeDeliveryKind::Continuation,
            source,
            1,
            key.as_str(),
            payload,
        )
        .map_err(|error| ContinuationSubmitError::Invalid(error.to_string()))?;
        match self
            .inbox
            .submit_with_key_claim(
                &address,
                submission,
                ContinuationKeyClaim {
                    owner: ledger_owner,
                    key: key.as_str().to_string(),
                    submission_digest: digest.clone(),
                    committed_at_ms,
                },
            )
            .await
            .map_err(unavailable)?
        {
            KeyedSubmitOutcome::Committed { binding, .. } => {
                Ok(Self::receipt(owner, &key, &binding))
            }
            KeyedSubmitOutcome::AlreadyBound(binding) => {
                Self::replay_or_conflict(owner, &key, &binding, &digest)
            }
        }
    }

    /// Whether this service's runtime admits work through a native work
    /// authorization host, so only retained completions can be committed.
    pub fn governs_work_authority(&self) -> bool {
        self.runtime.has_native_work_authorization_host()
    }

    /// Commit the outcome of a Meerkat producer's job (fork_off, council)
    /// for its owner, carrying the job's retained work.
    ///
    /// `job` is the job's committed record read from its owner. The delivery
    /// must name exactly that job: the same producer and job id, with the
    /// result the job committed, owed to the job's owner session (a session
    /// owner must be that session; a member owner is confirmed against the
    /// session serving it at admission). On a governed runtime the job must
    /// retain the native work binding of the run that dispatched it; the
    /// binding is then confirmed again at admission. Replays and conflicts
    /// follow [`Self::submit`].
    pub async fn submit_retained_completion(
        &self,
        owner: &ContinuationOwner,
        delivery: ContinuationDelivery,
        job: &RetainedJobRecord,
        committed_at_ms: u64,
    ) -> Result<ContinuationReceipt, ContinuationSubmitError> {
        let owed_elsewhere = match owner {
            ContinuationOwner::Session { session_id } => &job.facts.owner_session_id != session_id,
            ContinuationOwner::Member { .. } => false,
        };
        if delivery.result.producer != job.producer
            || delivery.result.producer_id != job.job_id
            || owed_elsewhere
            || delivery.result.result_digest != job.facts.result_digest
        {
            return Err(ContinuationSubmitError::Invalid(
                "the delivery does not name the job's committed outcome".into(),
            ));
        }
        if self.runtime.has_native_work_authorization_host() && job.facts.retained_work.is_none() {
            return Err(ContinuationSubmitError::NoAdmissibleWorkBinding);
        }
        self.commit(
            owner,
            delivery,
            job.facts.retained_work.clone(),
            committed_at_ms,
        )
        .await
    }

    /// The authoritative status of `(owner, key)`, read from the store.
    pub async fn continuation_status(
        &self,
        owner: &ContinuationOwner,
        key: &ContinuationKey,
    ) -> Result<ContinuationStatus, ContinuationStatusError> {
        let status_unavailable =
            |error: &dyn std::fmt::Display| ContinuationStatusError::Unavailable(error.to_string());
        let Some(binding) = self
            .inbox
            .continuation_key_binding(&owner.ledger_owner(), key.as_str())
            .await
            .map_err(|error| status_unavailable(&error))?
        else {
            return Ok(ContinuationStatus::NotCommitted);
        };
        let receipt = Self::receipt(owner, key, &binding);
        let delivery_id = RuntimeDeliveryId::new(binding.delivery_id.clone())
            .map_err(|error| ContinuationStatusError::Inconsistent(error.to_string()))?;
        let verdict = self
            .inbox
            .delivery_status(&binding.address, &delivery_id)
            .await
            .map_err(|error| status_unavailable(&error))?;
        let admission = self
            .inbox
            .continuation_admission(&binding.address, &delivery_id)
            .await
            .map_err(|error| status_unavailable(&error))?;
        match verdict {
            RuntimeDeliveryStatus::NotCommitted => {
                Err(ContinuationStatusError::Inconsistent(format!(
                    "key ledger names delivery {} that its inbox never committed",
                    binding.delivery_id
                )))
            }
            RuntimeDeliveryStatus::Refused {
                delivery_sequence,
                reason,
            } => Ok(ContinuationStatus::Refused {
                receipt,
                sequence: delivery_sequence,
                reason,
            }),
            RuntimeDeliveryStatus::AcknowledgedAhead { .. } => {
                Err(ContinuationStatusError::Inconsistent(format!(
                    "continuation delivery {} was acknowledged out of band",
                    binding.delivery_id
                )))
            }
            // A continuation row has one owner and never binds recipients.
            RuntimeDeliveryStatus::Mixed { .. } => {
                Err(ContinuationStatusError::Inconsistent(format!(
                    "continuation delivery {} settled a recipient group",
                    binding.delivery_id
                )))
            }
            RuntimeDeliveryStatus::Applied { .. } => match admission {
                Some(ContinuationAdmission::Applied {
                    session_id,
                    input_id,
                }) => Ok(ContinuationStatus::Applied {
                    receipt,
                    session: session_id,
                    input: input_id,
                }),
                other => Err(ContinuationStatusError::Inconsistent(format!(
                    "continuation delivery {} is acknowledged with admission {other:?}",
                    binding.delivery_id
                ))),
            },
            RuntimeDeliveryStatus::Pending { delivery_sequence } => {
                match admission {
                    Some(ContinuationAdmission::Applied {
                        session_id,
                        input_id,
                    }) => {
                        return Ok(ContinuationStatus::Pending {
                            receipt,
                            sequence: delivery_sequence,
                            admitted: Some((session_id, input_id)),
                        });
                    }
                    // The input may have reached the reserved session before a
                    // crash; only the next delivery attempt can tell, so a
                    // reserved row is never reported stranded.
                    Some(ContinuationAdmission::Reserved { .. }) => {
                        return Ok(ContinuationStatus::Pending {
                            receipt,
                            sequence: delivery_sequence,
                            admitted: None,
                        });
                    }
                    None => {}
                }
                match self
                    .resolver
                    .resolve_address(&binding.address)
                    .await
                    .map_err(ContinuationStatusError::Unavailable)?
                {
                    AddressResolution::Retired => Ok(ContinuationStatus::Stranded {
                        receipt,
                        cause: StrandedCause::OwnerRetired,
                    }),
                    AddressResolution::Session(_) | AddressResolution::NotServed => {
                        Ok(ContinuationStatus::Pending {
                            receipt,
                            sequence: delivery_sequence,
                            admitted: None,
                        })
                    }
                }
            }
        }
    }
}

/// Why a continuation was not admitted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContinuationAdmitError {
    /// A terminal policy outcome: the delivery is settled as refused, never
    /// admitted, and the deliveries after it proceed.
    #[error("continuation refused: {0:?}")]
    Refused(meerkat_runtime::RuntimeDeliveryRefusalReason),
    /// Not admitted now (an unavailable owner, an observation or store
    /// failure); the delivery stays pending and is retried.
    #[error("continuation not admitted: {0}")]
    Failed(String),
}

impl From<String> for ContinuationAdmitError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

/// The facts the committed owner of a producer's job holds about it: who the
/// outcome is owed to, the run it resumes and the result it committed.
/// Facts only: nothing here grants authority, and a governed admission asks
/// the configured owner for them again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedJobFacts {
    pub owner_session_id: SessionId,
    pub retained_work: Option<meerkat_core::retained_work::RetainedWorkIdentity>,
    /// Digest of the job's committed result, in the producer's result digest
    /// form ([`ContinuationResultRef::result_digest`]).
    pub result_digest: String,
}

/// A job owner's answer for one job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetainedJobLookup {
    /// The owner holds the job and committed these facts.
    Found(RetainedJobFacts),
    /// The owner definitively holds no such job (never committed, or gone
    /// for good): an immutably missing binding.
    Absent,
    /// The owner could not be read now; retryable, never a verdict.
    Unavailable(String),
}

/// The committed owner of fork_off and council jobs (the mob runtime): the
/// one place a retained completion's job is confirmed.
#[async_trait::async_trait]
pub trait RetainedJobSource: Send + Sync {
    /// What this owner committed about `producer`'s job `job_id`.
    async fn retained_job(
        &self,
        producer: &ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup;
}

/// The continuation services a host composition binds once its mob runtime
/// exists, after the delivery owner was armed: the address resolver for
/// member addresses and the committed owner of fork_off and council jobs.
/// Shared by every clone of a persistence bundle.
///
/// The slot is set once per binder: [`Self::bind`] hands the binder a
/// [`ContinuationBinding`] that keeps the services alive, and the slot frees
/// when the binder drops it. While a binding lives, another binder may take
/// the slot over only by naming that binding's generation
/// ([`Self::rebind`]); a stale or blind takeover is refused, typed. Before
/// anything is bound, member addresses read as not served and retained
/// completions find no job owner, so their rows stay pending, never refused
/// or settled.
#[derive(Default)]
pub struct ContinuationHostBindings {
    slot: std::sync::Mutex<BindingSlot>,
}

#[derive(Default)]
struct BindingSlot {
    /// The generation of the latest binding; 0 before the first.
    generation: u64,
    bound: Option<std::sync::Weak<BoundContinuationServices>>,
}

impl BindingSlot {
    fn live(&self) -> Option<Arc<BoundContinuationServices>> {
        self.bound.as_ref().and_then(std::sync::Weak::upgrade)
    }
}

struct BoundContinuationServices {
    resolver: Arc<dyn ContinuationAddressResolver>,
    job_source: Arc<dyn RetainedJobSource>,
}

/// The generation of one binding of a [`ContinuationHostBindings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContinuationBindingGeneration(u64);

/// A live binding of a host's continuation services. The slot holds them
/// only while this lives.
pub struct ContinuationBinding {
    _services: Arc<BoundContinuationServices>,
    generation: ContinuationBindingGeneration,
}

impl ContinuationBinding {
    pub fn generation(&self) -> ContinuationBindingGeneration {
        self.generation
    }
}

impl std::fmt::Debug for ContinuationBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContinuationBinding")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Why a binder could not take a host's continuation slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ContinuationBindError {
    /// Another binding of this generation is live.
    #[error("the host's continuation services are bound (generation {0:?})")]
    AlreadyBound(ContinuationBindingGeneration),
    /// The takeover named a binding that is not the current one.
    #[error("the replaced continuation binding {replaced:?} is not the current one ({current:?})")]
    StaleGeneration {
        replaced: ContinuationBindingGeneration,
        current: ContinuationBindingGeneration,
    },
}

impl std::fmt::Debug for ContinuationHostBindings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let slot = self.slot();
        f.debug_struct("ContinuationHostBindings")
            .field("generation", &slot.generation)
            .field("bound", &slot.live().is_some())
            .finish()
    }
}

impl ContinuationHostBindings {
    fn slot(&self) -> std::sync::MutexGuard<'_, BindingSlot> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn install(
        slot: &mut BindingSlot,
        resolver: Arc<dyn ContinuationAddressResolver>,
        job_source: Arc<dyn RetainedJobSource>,
    ) -> ContinuationBinding {
        let services = Arc::new(BoundContinuationServices {
            resolver,
            job_source,
        });
        slot.generation = slot.generation.saturating_add(1);
        slot.bound = Some(Arc::downgrade(&services));
        ContinuationBinding {
            _services: services,
            generation: ContinuationBindingGeneration(slot.generation),
        }
    }

    /// Bind the host's address resolver and job owner into a free slot: one
    /// never bound, or whose binding was dropped.
    ///
    /// # Errors
    /// [`ContinuationBindError::AlreadyBound`] while another binding lives.
    pub fn bind(
        &self,
        resolver: Arc<dyn ContinuationAddressResolver>,
        job_source: Arc<dyn RetainedJobSource>,
    ) -> Result<ContinuationBinding, ContinuationBindError> {
        let mut slot = self.slot();
        if slot.live().is_some() {
            return Err(ContinuationBindError::AlreadyBound(
                ContinuationBindingGeneration(slot.generation),
            ));
        }
        Ok(Self::install(&mut slot, resolver, job_source))
    }

    /// Take the slot over from the binding of generation `replaced`, which
    /// must be the current one (live or dropped).
    ///
    /// # Errors
    /// [`ContinuationBindError::StaleGeneration`] when `replaced` is not the
    /// current binding.
    pub fn rebind(
        &self,
        replaced: ContinuationBindingGeneration,
        resolver: Arc<dyn ContinuationAddressResolver>,
        job_source: Arc<dyn RetainedJobSource>,
    ) -> Result<ContinuationBinding, ContinuationBindError> {
        let mut slot = self.slot();
        let current = ContinuationBindingGeneration(slot.generation);
        if replaced != current {
            return Err(ContinuationBindError::StaleGeneration { replaced, current });
        }
        Ok(Self::install(&mut slot, resolver, job_source))
    }

    /// The bound address resolver, while a binding lives.
    pub fn resolver(&self) -> Option<Arc<dyn ContinuationAddressResolver>> {
        self.slot().live().map(|bound| Arc::clone(&bound.resolver))
    }

    /// The bound job owner, while a binding lives.
    pub fn job_source(&self) -> Option<Arc<dyn RetainedJobSource>> {
        self.slot()
            .live()
            .map(|bound| Arc::clone(&bound.job_source))
    }

    /// Resolve `address` through the bound resolver, or as a plain session
    /// address when none is bound. A resolver failure is never a verdict:
    /// the address reads as not served, so its rows stay pending.
    pub async fn resolve_address(&self, address: &LogicalRuntimeId) -> AddressResolution {
        match self.resolver() {
            Some(resolver) => resolver
                .resolve_address(address)
                .await
                .unwrap_or(AddressResolution::NotServed),
            None => crate::default_address_resolution(address),
        }
    }
}

/// A producer job's committed record, read from its owner. Process-only and
/// minted only through [`RetainedJobRecord::from_owner`]; it is the only way
/// to submit a continuation that carries a native work binding.
pub struct RetainedJobRecord {
    producer: ContinuationProducer,
    job_id: String,
    facts: RetainedJobFacts,
}

impl RetainedJobRecord {
    /// Read `producer`'s job `job_id` from its committed owner.
    ///
    /// # Errors
    /// The owner could not be read.
    pub async fn from_owner(
        owner: &dyn RetainedJobSource,
        producer: ContinuationProducer,
        job_id: impl Into<String>,
    ) -> Result<Option<Self>, String> {
        let job_id = job_id.into();
        match owner.retained_job(&producer, &job_id).await {
            RetainedJobLookup::Found(facts) => Ok(Some(Self {
                producer,
                job_id,
                facts,
            })),
            RetainedJobLookup::Absent => Ok(None),
            RetainedJobLookup::Unavailable(error) => Err(error),
        }
    }
}

impl std::fmt::Debug for RetainedJobRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetainedJobRecord")
            .field("job_id", &self.job_id)
            .finish_non_exhaustive()
    }
}

/// Admits continuation deliveries into a session's runtime.
#[async_trait::async_trait]
pub trait ContinuationDeliverySink: Send + Sync {
    /// Admit the continuation `input` (built by
    /// [`meerkat_runtime::PromptInput::continuation`] and keyed by its
    /// admission key) to `session`. Idempotent by admission key within the
    /// session; returns the id of the input the key names.
    async fn admit(
        &self,
        session: &SessionId,
        input: meerkat_runtime::Input,
    ) -> Result<InputId, String>;

    /// The accepted row `session` holds under `admission_key`, if any. The
    /// caller decides whether it is this continuation's admission
    /// ([`meerkat_runtime::verify_exact_replay`]); the key alone never does.
    async fn admitted_input(
        &self,
        session: &SessionId,
        admission_key: &str,
    ) -> Result<Option<meerkat_runtime::input_state::StoredInputState>, String>;

    /// Whether this sink's runtime admits work through a native work
    /// authorization host. A continuation carries no native work binding yet,
    /// so the owner refuses it on such a runtime, typed and before any
    /// reservation or admission, instead of submitting an input without
    /// authority.
    fn governs_work_authority(&self) -> bool {
        false
    }

    /// Admit `input` as a governed resume of the retained work `request`
    /// names (see [`meerkat_runtime::MeerkatMachine::accept_retained_resume`]).
    /// A refusal is typed as [`ContinuationAdmitError::Refused`]. The default
    /// admits nothing.
    async fn admit_retained(
        &self,
        session: &SessionId,
        input: meerkat_runtime::Input,
        request: meerkat_runtime::retained_work::RetainedResumeRequest,
    ) -> Result<InputId, ContinuationAdmitError> {
        let _ = (session, input, request);
        Err(ContinuationAdmitError::Failed(
            "this continuation sink does not admit retained work".into(),
        ))
    }
}

/// Admits continuations straight into a session's runtime registration: a
/// durable system-originated content input that wakes an idle session. A
/// session the runtime has not registered refuses it, and the row stays
/// pending until an attachment commit retries it.
#[derive(Clone)]
pub struct MachineContinuationSink {
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
}

impl std::fmt::Debug for MachineContinuationSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachineContinuationSink")
            .finish_non_exhaustive()
    }
}

impl MachineContinuationSink {
    pub fn new(runtime: Arc<meerkat_runtime::MeerkatMachine>) -> Self {
        Self { runtime }
    }
}

#[async_trait::async_trait]
impl ContinuationDeliverySink for MachineContinuationSink {
    async fn admit(
        &self,
        session: &SessionId,
        input: meerkat_runtime::Input,
    ) -> Result<InputId, String> {
        match self
            .runtime
            .accept_input_with_completion(session, input)
            .await
            .map_err(|error| error.to_string())?
            .0
        {
            meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } => Ok(input_id),
            meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. } => Ok(existing_id),
            other => Err(format!(
                "continuation admission was not accepted: {other:?}"
            )),
        }
    }

    async fn admitted_input(
        &self,
        session: &SessionId,
        admission_key: &str,
    ) -> Result<Option<meerkat_runtime::input_state::StoredInputState>, String> {
        use meerkat_runtime::SessionServiceRuntimeExt as _;
        self.runtime
            .input_state_by_idempotency_key(session, admission_key)
            .await
            .map_err(|error| error.to_string())
    }

    fn governs_work_authority(&self) -> bool {
        self.runtime.has_native_work_authorization_host()
    }

    async fn admit_retained(
        &self,
        session: &SessionId,
        input: meerkat_runtime::Input,
        request: meerkat_runtime::retained_work::RetainedResumeRequest,
    ) -> Result<InputId, ContinuationAdmitError> {
        retained_admission_outcome(
            self.runtime
                .accept_retained_resume(session, input, request)
                .await
                .map(|(outcome, _completion)| outcome),
        )
    }
}

/// Classify a retained-resume admission: only a terminal policy refusal is
/// [`ContinuationAdmitError::Refused`]; every other error is retryable.
pub fn retained_admission_outcome(
    outcome: Result<meerkat_runtime::AcceptOutcome, meerkat_runtime::RuntimeDriverError>,
) -> Result<InputId, ContinuationAdmitError> {
    match outcome {
        Ok(meerkat_runtime::AcceptOutcome::Accepted { input_id, .. }) => Ok(input_id),
        Ok(meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. }) => Ok(existing_id),
        Ok(other) => Err(ContinuationAdmitError::Failed(format!(
            "retained continuation admission was not accepted: {other:?}"
        ))),
        Err(meerkat_runtime::RuntimeDriverError::RetainedResumeRefused { reason }) => {
            Err(ContinuationAdmitError::Refused(reason.into()))
        }
        Err(error) => Err(ContinuationAdmitError::Failed(error.to_string())),
    }
}

/// Admit one committed continuation row into `session`, recovering an
/// admission that reached a session but not the inbox acknowledgement.
///
/// Reserve-first, through the typed admission index
/// ([`ContinuationAdmissionTransition`]): the index reserves the session and
/// input id before the input is admitted, and is applied once the session's
/// input ledger holds this exact admission. The ledger commits an input and
/// its idempotency key in one store transaction, so asking the reserved
/// session for the admission key is authoritative about whether anything was
/// admitted there; [`meerkat_runtime::verify_exact_replay`] decides whether
/// it is this continuation (same content and the same retained authority).
/// An exact replay is applied and nothing is admitted again, even if the
/// owner moved to another session since. Any other row under the key is a
/// conflict: the row stays pending and visible, and is never applied by key
/// alone. Nothing under the key: the reservation never reached its session
/// and is repointed to `session`. Nothing sweeps or times out a reservation:
/// it is reconciled by the owner's reconcile pass on arm and by the next
/// delivery attempt for the row. The caller acknowledges the row on `Ok`.
///
/// A continuation carries no native work binding yet: on a sink whose runtime
/// governs work authority the row is refused
/// ([`meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding`])
/// before anything is reserved or admitted. The drain settles it as refused,
/// never applied, and the rows after it proceed. This covers rows committed
/// before the runtime was governed (historical rows) and rows from any other
/// submitter; [`ContinuationOwnerService::submit`] already refuses new ones
/// on a governed runtime without committing them.
pub(crate) async fn admit_continuation_row(
    inbox: &RuntimeDeliveryInbox,
    address: &LogicalRuntimeId,
    record: &meerkat_runtime::RuntimeDeliveryRecord,
    sink: &dyn ContinuationDeliverySink,
    session: &SessionId,
    job_source: Option<&dyn RetainedJobSource>,
) -> Result<(), crate::job_delivery::JobOutboxProjectionError> {
    let delivery_id = record.submission.delivery_id();
    let outcome = async {
        let row: ContinuationRow = serde_json::from_slice(record.submission.payload())
            .map_err(|error| format!("continuation delivery payload is invalid: {error}"))?;
        let retained = if sink.governs_work_authority() {
            // Only a Meerkat-minted completion carries a native work
            // binding, and only its committed job owner can confirm it.
            let Some(identity) = row.retained_work.clone() else {
                return Err(ContinuationAdmitError::Refused(
                    meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding,
                ));
            };
            let source = job_source.ok_or_else(|| {
                ContinuationAdmitError::Failed(
                    "no retained job source is configured for this governed runtime".into(),
                )
            })?;
            verify_committed_job(source, &row.delivery, &identity, session).await?;
            Some(identity)
        } else {
            None
        };
        admit_continuation(
            inbox,
            address,
            record,
            sink,
            session,
            row.delivery,
            retained,
        )
        .await
    }
    .await;
    match outcome {
        Ok(()) => Ok(()),
        Err(ContinuationAdmitError::Refused(reason)) => {
            Err(crate::job_delivery::JobOutboxProjectionError::Refused {
                delivery_id: delivery_id.to_string(),
                reason,
            })
        }
        Err(ContinuationAdmitError::Failed(error)) => {
            Err(crate::job_delivery::JobOutboxProjectionError::Apply(
                crate::job_delivery::JobDeliveryApplyError::Infrastructure(error),
            ))
        }
    }
}

/// The committed job owner must confirm the completion exactly: the same
/// producer and job, the same retained work, the destination session it is
/// owed to and the result it committed. Anything else is an immutably
/// invalid binding; a failure to ask is retryable.
async fn verify_committed_job(
    source: &dyn RetainedJobSource,
    delivery: &ContinuationDelivery,
    identity: &meerkat_core::retained_work::RetainedWorkIdentity,
    session: &SessionId,
) -> Result<(), ContinuationAdmitError> {
    let invalid = || {
        ContinuationAdmitError::Refused(
            meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding,
        )
    };
    if !matches!(
        delivery.result.producer,
        ContinuationProducer::ForkOff | ContinuationProducer::Council
    ) {
        return Err(invalid());
    }
    let facts = match source
        .retained_job(&delivery.result.producer, &delivery.result.producer_id)
        .await
    {
        RetainedJobLookup::Found(facts) => facts,
        RetainedJobLookup::Absent => return Err(invalid()),
        RetainedJobLookup::Unavailable(error) => return Err(ContinuationAdmitError::Failed(error)),
    };
    if facts.retained_work.as_ref() != Some(identity)
        || &facts.owner_session_id != session
        || facts.result_digest != delivery.result.result_digest
    {
        return Err(invalid());
    }
    Ok(())
}

async fn admit_continuation(
    inbox: &RuntimeDeliveryInbox,
    address: &LogicalRuntimeId,
    record: &meerkat_runtime::RuntimeDeliveryRecord,
    sink: &dyn ContinuationDeliverySink,
    session: &SessionId,
    delivery: ContinuationDelivery,
    retained: Option<meerkat_core::retained_work::RetainedWorkIdentity>,
) -> Result<(), ContinuationAdmitError> {
    let delivery_id = record.submission.delivery_id();
    let admission_key = delivery.admission_key(delivery_id);
    let handling = match delivery.handling {
        ContinuationHandling::Queue => meerkat_core::types::HandlingMode::Queue,
        ContinuationHandling::Steer => meerkat_core::types::HandlingMode::Steer,
    };
    let continuation_input = |input_id: InputId| {
        meerkat_runtime::Input::Prompt(match &delivery.body {
            ContinuationBody::Content { content } => meerkat_runtime::PromptInput::continuation(
                input_id,
                admission_key.clone(),
                content.clone(),
                handling,
            ),
            ContinuationBody::Notice {
                notice_kind,
                body,
                blocks,
            } => {
                let mut prompt = meerkat_runtime::PromptInput::detached_job_completed(
                    admission_key.clone(),
                    meerkat_core::types::SystemNoticeMessage::with_blocks(
                        *notice_kind,
                        body.clone(),
                        blocks.clone(),
                    ),
                );
                prompt.header.id = input_id;
                prompt
            }
        })
    };
    let transition = |transition: ContinuationAdmissionTransition| async move {
        match inbox
            .transition_continuation_admission(address, delivery_id, transition.clone())
            .await
            .map_err(|error| error.to_string())?
        {
            ContinuationAdmissionOutcome::Transitioned(_) => Ok(()),
            ContinuationAdmissionOutcome::Rejected { current } => Err(format!(
                "continuation admission index rejected {transition:?} over {current:?}"
            )),
        }
    };
    // The exact admission `session_id` holds under the key, if any; another
    // row under the key is a conflict, never this delivery.
    let retained_work = retained.as_ref();
    let exact_admission = |session_id: SessionId| {
        let expected = continuation_input(InputId::new());
        let admission_key = admission_key.clone();
        async move {
            let Some(row) = sink.admitted_input(&session_id, &admission_key).await? else {
                return Ok(None);
            };
            match retained_work {
                Some(identity) => {
                    meerkat_runtime::verify_exact_resume_replay(&row.state, &expected, identity)
                }
                None => meerkat_runtime::verify_exact_replay(&row.state, &expected),
            }
            .map_err(|error| {
                format!(
                    "session {session_id} holds input {} under continuation admission key \
                     {admission_key} that is not this delivery: {error}",
                    row.state.input_id
                )
            })?;
            Ok::<_, String>(Some(row.state.input_id))
        }
    };
    let input_id = match inbox
        .continuation_admission(address, delivery_id)
        .await
        .map_err(|error| error.to_string())?
    {
        Some(ContinuationAdmission::Applied { .. }) => return Ok(()),
        Some(ContinuationAdmission::Reserved {
            session_id: reserved,
            input_id,
        }) => {
            if let Some(admitted) = exact_admission(reserved.clone()).await? {
                return Ok(transition(ContinuationAdmissionTransition::Apply {
                    session_id: reserved,
                    input_id: admitted,
                })
                .await?);
            }
            if reserved != *session {
                transition(ContinuationAdmissionTransition::Repoint {
                    from: reserved,
                    session_id: session.clone(),
                    input_id: input_id.clone(),
                })
                .await?;
            }
            input_id
        }
        None => {
            // A completion delivered live under the same key before it became
            // a continuation stands for it only if it is an exact replay.
            if let Some(admitted) = exact_admission(session.clone()).await? {
                transition(ContinuationAdmissionTransition::Reserve {
                    session_id: session.clone(),
                    input_id: admitted.clone(),
                })
                .await?;
                return Ok(transition(ContinuationAdmissionTransition::Apply {
                    session_id: session.clone(),
                    input_id: admitted,
                })
                .await?);
            }
            let input_id = InputId::new();
            transition(ContinuationAdmissionTransition::Reserve {
                session_id: session.clone(),
                input_id: input_id.clone(),
            })
            .await?;
            input_id
        }
    };
    let input = continuation_input(input_id.clone());
    let admitted = match retained.as_ref() {
        Some(identity) => {
            let request = inbox
                .retained_resume_request(address, delivery_id, record.sequence, |stored| {
                    serde_json::from_slice::<ContinuationRow>(stored.payload())
                        .map_err(|error| error.to_string())?
                        .retained_work
                        .ok_or_else(|| "the committed row carries no retained work".to_string())
                })
                .await
                .map_err(|error| ContinuationAdmitError::Failed(error.to_string()))?;
            if request.identity() != identity {
                return Err(ContinuationAdmitError::Failed(
                    "the committed row changed since it was confirmed".into(),
                ));
            }
            sink.admit_retained(session, input, request).await?
        }
        None => sink
            .admit(session, input)
            .await
            .map_err(ContinuationAdmitError::Failed)?,
    };
    if admitted != input_id {
        // The session deduplicated the admission onto an existing row under
        // the key; it stands for this delivery only as an exact replay.
        let Some(exact) = exact_admission(session.clone()).await? else {
            return Err(ContinuationAdmitError::Failed(format!(
                "session {session} named input {admitted} for continuation admission key \
                 {admission_key} but holds no row under it"
            )));
        };
        if exact != admitted {
            return Err(ContinuationAdmitError::Failed(format!(
                "session {session} named input {admitted} for continuation admission key \
                 {admission_key} but holds {exact}"
            )));
        }
    }
    transition(ContinuationAdmissionTransition::Apply {
        session_id: session.clone(),
        input_id: admitted,
    })
    .await?;
    Ok(())
}

/// The address a member owner's current incarnation binds to.
pub fn member_delivery_address(
    mob_id: &str,
    identity: &str,
    generation: u64,
) -> Result<LogicalRuntimeId, ContinuationSubmitError> {
    LogicalRuntimeId::for_member(&MemberDeliveryAddress {
        mob_id: mob_id.to_string(),
        identity: identity.to_string(),
        generation,
    })
    .map_err(|error| ContinuationSubmitError::Invalid(error.to_string()))
}

impl From<RuntimeDeliveryError> for ContinuationSubmitError {
    fn from(error: RuntimeDeliveryError) -> Self {
        unavailable(error)
    }
}
