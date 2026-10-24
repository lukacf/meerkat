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
use meerkat_core::service::MobIncarnationId;
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
/// committed it from its job's record. A host original
/// ([`ContinuationOwnerService::submit_host_original`]) carries the marker
/// instead: its authority is bound for the admitting session at each
/// admission attempt, never stored here. The marker goes only with a `Host`
/// producer and never together with retained work.
#[derive(Serialize, Deserialize)]
struct ContinuationRow {
    #[serde(flatten)]
    delivery: ContinuationDelivery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retained_work: Option<meerkat_core::retained_work::RetainedWorkIdentity>,
    #[serde(default, skip_serializing_if = "is_false")]
    host_original: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Host namespaces Core owns (consent notices are `meerkat.consent`). No host
/// original is ever committed, confirmed, offered or admitted in one, whatever
/// a host's source answers for it.
pub const RESERVED_HOST_NAMESPACE_PREFIX: &str = "meerkat.";

/// Whether `namespace` is a Core-owned host namespace
/// ([`RESERVED_HOST_NAMESPACE_PREFIX`]).
pub fn is_reserved_host_namespace(namespace: &str) -> bool {
    namespace.starts_with(RESERVED_HOST_NAMESPACE_PREFIX)
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
    /// The submitter's observation time when the key was first committed
    /// (fork_off and council sample the clock at submission). Not part of
    /// the content: every replay returns the first receipt's value.
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

/// What [`ContinuationOwnerService::observe_retained_completion`] found for
/// the exact expected content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationObservation {
    /// No continuation is bound to the owner and key: nothing was committed.
    NotSubmitted,
    /// The key is bound to exactly the expected content; its current status.
    /// An applied status means the continuation was admitted as input to
    /// its owner, not that the owner's resulting work has completed.
    Matched {
        receipt: ContinuationReceipt,
        status: ContinuationStatus,
    },
}

/// Why an exact-content observation found no answer for the expected content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ContinuationObserveError {
    /// The owner and key are bound to other content: whatever that
    /// continuation's status is, it is not the expected one's.
    #[error("continuation key {} is bound to other content", existing.key.as_str())]
    ContentMismatch { existing: Box<ContinuationReceipt> },
    /// The expected delivery does not name the job's committed outcome.
    #[error("invalid continuation: {0}")]
    Invalid(String),
    #[error("continuation authority unavailable: {0}")]
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

/// What a status read observes about the incarnation behind a delivery
/// address: only what its owners' records prove, with nothing served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressObservation {
    /// The records prove the incarnation retired: it never serves again.
    Retired,
    /// Not proven retired: it may be served now or later. Serving it is
    /// delivery work, which an observation never does.
    MayServe,
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

    /// The incarnation state behind an address, for delivery: serving it
    /// may make its owner live (revive a member or a session, attach its
    /// executor).
    async fn resolve_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<AddressResolution, String>;

    /// Observe the incarnation behind an address without serving it: whether
    /// its owners' records prove it retired. A status read uses this. It
    /// revives no owner, attaches no executor, claims no hosting and starts
    /// no work; only [`Self::resolve_address`], for delivery, does that.
    /// `Err` when the records cannot be read now.
    ///
    /// The default proves nothing ([`AddressObservation::MayServe`]); a
    /// resolver whose records can prove a retirement reports it.
    async fn observe_address(
        &self,
        _address: &LogicalRuntimeId,
    ) -> Result<AddressObservation, String> {
        Ok(AddressObservation::MayServe)
    }

    /// The delivery address of the mob member whose current session is
    /// `session_id`, if any: a detached job that session submits records it,
    /// so the job's deliveries follow the member across a repoint to a
    /// successor session (#1790). The default knows of no members.
    async fn member_address_of_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<LogicalRuntimeId>, String> {
        Ok(None)
    }
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
        Ok(AddressResolution::Session(session_address_owner(address)?))
    }

    /// A session address names its own session; nothing here proves it
    /// retired.
    async fn observe_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<AddressObservation, String> {
        session_address_owner(address)?;
        Ok(AddressObservation::MayServe)
    }
}

/// The session a standalone-session address names.
fn session_address_owner(address: &LogicalRuntimeId) -> Result<SessionId, String> {
    if address
        .member_address()
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err("member addresses need a mob-aware continuation resolver".into());
    }
    address
        .0
        .strip_prefix("rt:session:")
        .and_then(|raw| SessionId::parse(raw).ok())
        .ok_or_else(|| format!("{address} is not a session delivery address"))
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
        host_original: bool,
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
        // Domain-separated: a host original is never the same submission as
        // a plain continuation of the same delivery under the same key.
        if host_original {
            parts.push(b"host-original");
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
        self.commit(owner, delivery, None, false, committed_at_ms, None)
            .await
    }

    /// Commit a host original for `owner`: a continuation of a `Host`
    /// namespace that its committed owner holds exactly
    /// ([`HostOriginalRecord::from_owner`]). On a runtime with a native work
    /// authorization host it is admitted only through that namespace owner's
    /// binder, confirmed again at each admission attempt; elsewhere it is
    /// admitted as any continuation. Replays and conflicts follow
    /// [`Self::submit`]. A host original is never the same submission as a
    /// plain continuation under the same key: choose one entry per key.
    ///
    /// The record is checked for consistency here, not trusted as a gate:
    /// the drain confirms the row again against the bound namespace owner.
    ///
    /// # Errors
    /// [`ContinuationSubmitError::Invalid`] for a reserved or empty namespace,
    /// a delivery or owner other than the record's, or (on a governed
    /// runtime) handling other than `Queue`; otherwise as [`Self::submit`].
    pub async fn submit_host_original(
        &self,
        owner: &ContinuationOwner,
        delivery: ContinuationDelivery,
        original: &HostOriginalRecord,
        committed_at_ms: u64,
    ) -> Result<ContinuationReceipt, ContinuationSubmitError> {
        let ContinuationProducer::Host { namespace } = &delivery.result.producer else {
            return Err(ContinuationSubmitError::Invalid(
                "a host original is delivered under its host namespace".into(),
            ));
        };
        if namespace.is_empty() || is_reserved_host_namespace(namespace) {
            return Err(ContinuationSubmitError::Invalid(format!(
                "host namespace {namespace:?} is reserved or empty"
            )));
        }
        if *namespace != original.namespace
            || delivery.result.producer_id != original.producer_id
            || delivery != original.facts.delivery
            || *owner != original.facts.owner
        {
            return Err(ContinuationSubmitError::Invalid(
                "the delivery is not the host original its namespace owner committed".into(),
            ));
        }
        if self.runtime.has_native_work_authorization_host()
            && delivery.handling != ContinuationHandling::Queue
        {
            return Err(ContinuationSubmitError::Invalid(
                "a governed host original joins its owner after the current turn (queue)".into(),
            ));
        }
        self.commit(owner, delivery, None, true, committed_at_ms, None)
            .await
    }

    /// [`Self::submit`] for a member owner, bound only to the member's mob
    /// incarnation `incarnation`. A handle kept from an earlier incarnation of
    /// the mob (one whose actor stopped, while the host now holds a mob
    /// created again under its id) commits nothing for the current
    /// incarnation's member.
    ///
    /// The incarnation is checked inside the commit, on the address the commit
    /// binds, so no destroy and re-create can slip between the check and the
    /// commit:
    /// - the key is not bound yet: it binds only while the owner's current
    ///   address names `incarnation`, and is
    ///   [`ContinuationSubmitError::OwnerRetired`] otherwise;
    /// - the key is bound at `incarnation`: replayed, or a conflict, as for
    ///   [`Self::submit`];
    /// - the key is bound at another incarnation:
    ///   [`ContinuationSubmitError::OwnerRetired`] while the current address
    ///   does not name `incarnation` (this caller is stale); replayed, or a
    ///   conflict, as for [`Self::submit`], while it does (the current
    ///   incarnation reusing a key an earlier one bound).
    ///
    /// A [`ContinuationOwner::Session`] owner has no mob incarnation and is
    /// unchanged: for it this is [`Self::submit`].
    pub async fn submit_for_member_incarnation(
        &self,
        owner: &ContinuationOwner,
        incarnation: MobIncarnationId,
        delivery: ContinuationDelivery,
        committed_at_ms: u64,
    ) -> Result<ContinuationReceipt, ContinuationSubmitError> {
        if self.runtime.has_native_work_authorization_host() {
            return Err(ContinuationSubmitError::NoAdmissibleWorkBinding);
        }
        self.commit(
            owner,
            delivery,
            None,
            false,
            committed_at_ms,
            Self::member_incarnation(owner, incarnation),
        )
        .await
    }

    /// The incarnation a member owner's commit is bound to; `None` for a
    /// session owner, which has none.
    fn member_incarnation(
        owner: &ContinuationOwner,
        incarnation: MobIncarnationId,
    ) -> Option<MobIncarnationId> {
        match owner {
            ContinuationOwner::Member { .. } => Some(incarnation),
            ContinuationOwner::Session { .. } => None,
        }
    }

    /// Whether `address` is a member address of mob incarnation `incarnation`.
    /// An address minted before 0.9.4 names no incarnation, so it names none
    /// a caller expects.
    fn address_names_incarnation(
        address: &LogicalRuntimeId,
        incarnation: MobIncarnationId,
    ) -> Result<bool, ContinuationSubmitError> {
        address
            .member_address()
            .map(|member| member.is_some_and(|member| member.incarnation == Some(incarnation)))
            .map_err(|error| ContinuationSubmitError::Invalid(error.to_string()))
    }

    /// Whether the owner's current address names `incarnation`. An owner with
    /// no current address names none.
    async fn current_address_names_incarnation(
        &self,
        owner: &ContinuationOwner,
        incarnation: MobIncarnationId,
    ) -> Result<bool, ContinuationSubmitError> {
        match self
            .resolver
            .current_address(owner)
            .await
            .map_err(ContinuationSubmitError::Unavailable)?
        {
            Some(address) => Self::address_names_incarnation(&address, incarnation),
            None => Ok(false),
        }
    }

    /// A key already bound: replayed, or a conflict, as for [`Self::submit`].
    /// For a commit bound to `expected`, a binding at another incarnation is
    /// [`ContinuationSubmitError::OwnerRetired`] unless the owner's current
    /// address names `expected` (see [`Self::submit_for_member_incarnation`]).
    async fn replay_bound_key(
        &self,
        owner: &ContinuationOwner,
        key: &ContinuationKey,
        binding: &meerkat_runtime::ContinuationKeyBinding,
        digest: &str,
        expected: Option<MobIncarnationId>,
    ) -> Result<ContinuationReceipt, ContinuationSubmitError> {
        if let Some(expected) = expected
            && !Self::address_names_incarnation(&binding.address, expected)?
            && !self
                .current_address_names_incarnation(owner, expected)
                .await?
        {
            return Err(ContinuationSubmitError::OwnerRetired);
        }
        Self::replay_or_conflict(owner, key, binding, digest)
    }

    /// `expected`: the mob incarnation a member owner's commit is bound to
    /// (see [`Self::submit_for_member_incarnation`]); `None` binds whatever
    /// the owner's current address names.
    async fn commit(
        &self,
        owner: &ContinuationOwner,
        delivery: ContinuationDelivery,
        retained_work: Option<meerkat_core::retained_work::RetainedWorkIdentity>,
        host_original: bool,
        committed_at_ms: u64,
        expected: Option<MobIncarnationId>,
    ) -> Result<ContinuationReceipt, ContinuationSubmitError> {
        let ledger_owner = owner.ledger_owner();
        let digest =
            Self::submission_digest(owner, &delivery, retained_work.as_ref(), host_original)?;
        if let Some(binding) = self
            .inbox
            .continuation_key_binding(&ledger_owner, delivery.key.as_str())
            .await
            .map_err(unavailable)?
        {
            return self
                .replay_bound_key(owner, &delivery.key, &binding, &digest, expected)
                .await;
        }
        let Some(address) = self
            .resolver
            .current_address(owner)
            .await
            .map_err(ContinuationSubmitError::Unavailable)?
        else {
            return Err(ContinuationSubmitError::OwnerRetired);
        };
        if let Some(expected) = expected
            && !Self::address_names_incarnation(&address, expected)?
        {
            return Err(ContinuationSubmitError::OwnerRetired);
        }
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
            host_original,
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
                self.replay_bound_key(owner, &key, &binding, &digest, expected)
                    .await
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
        self.check_retained_completion(owner, &delivery, job)?;
        self.commit(
            owner,
            delivery,
            job.facts.retained_work.clone(),
            false,
            committed_at_ms,
            None,
        )
        .await
    }

    /// [`Self::submit_retained_completion`] for a member owner, bound only to
    /// the member's mob incarnation `incarnation`, exactly as
    /// [`Self::submit_for_member_incarnation`] binds a submission. A
    /// [`ContinuationOwner::Session`] owner is unchanged: for it this is
    /// [`Self::submit_retained_completion`].
    pub async fn submit_retained_completion_for_member_incarnation(
        &self,
        owner: &ContinuationOwner,
        incarnation: MobIncarnationId,
        delivery: ContinuationDelivery,
        job: &RetainedJobRecord,
        committed_at_ms: u64,
    ) -> Result<ContinuationReceipt, ContinuationSubmitError> {
        self.check_retained_completion(owner, &delivery, job)?;
        self.commit(
            owner,
            delivery,
            job.facts.retained_work.clone(),
            false,
            committed_at_ms,
            Self::member_incarnation(owner, incarnation),
        )
        .await
    }

    /// The delivery names exactly `job`'s committed outcome, owed to `owner`,
    /// and a governed runtime can admit it.
    fn check_retained_completion(
        &self,
        owner: &ContinuationOwner,
        delivery: &ContinuationDelivery,
        job: &RetainedJobRecord,
    ) -> Result<(), ContinuationSubmitError> {
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
        Ok(())
    }

    /// Read-only: whether the expected retained completion (`delivery` with
    /// `job`'s committed retained work) is exactly the content bound to
    /// `(owner, delivery.key)`, and if so its current status. The comparison
    /// uses the same submission digest a commit binds, so a status observed
    /// for other content under the same key is never reported as this one's.
    /// Never commits, and never serves the owner, whatever it finds (see
    /// [`Self::continuation_status`]).
    ///
    /// For a [`ContinuationOwner::Session`] owner the job's owner session is
    /// checked against it here, as a commit checks it. For a
    /// [`ContinuationOwner::Member`] owner neither the digest nor this call
    /// covers the job's owner session: as for a commit, the session serving
    /// the member's bound incarnation is verified at delivery, so a match
    /// here does not promise it.
    ///
    /// # Errors
    /// [`ContinuationObserveError::ContentMismatch`] when the key is bound to
    /// other content; [`ContinuationObserveError::Invalid`] when `delivery`
    /// does not name `job`'s committed outcome (the checks of
    /// [`Self::submit_retained_completion`]); store failures otherwise.
    pub async fn observe_retained_completion(
        &self,
        owner: &ContinuationOwner,
        delivery: &ContinuationDelivery,
        job: &RetainedJobRecord,
    ) -> Result<ContinuationObservation, ContinuationObserveError> {
        let owed_elsewhere = match owner {
            ContinuationOwner::Session { session_id } => &job.facts.owner_session_id != session_id,
            ContinuationOwner::Member { .. } => false,
        };
        if delivery.result.producer != job.producer
            || delivery.result.producer_id != job.job_id
            || owed_elsewhere
            || delivery.result.result_digest != job.facts.result_digest
        {
            return Err(ContinuationObserveError::Invalid(
                "the delivery does not name the job's committed outcome".into(),
            ));
        }
        let digest =
            Self::submission_digest(owner, delivery, job.facts.retained_work.as_ref(), false)
                .map_err(|error| ContinuationObserveError::Unavailable(error.to_string()))?;
        let Some(binding) = self
            .inbox
            .continuation_key_binding(&owner.ledger_owner(), delivery.key.as_str())
            .await
            .map_err(|error| ContinuationObserveError::Unavailable(error.to_string()))?
        else {
            return Ok(ContinuationObservation::NotSubmitted);
        };
        let receipt = Self::receipt(owner, &delivery.key, &binding);
        if binding.submission_digest != digest {
            return Err(ContinuationObserveError::ContentMismatch {
                existing: Box::new(receipt),
            });
        }
        let status = self
            .continuation_status(owner, &delivery.key)
            .await
            .map_err(|error| match error {
                ContinuationStatusError::Unavailable(detail) => {
                    ContinuationObserveError::Unavailable(detail)
                }
                ContinuationStatusError::Inconsistent(detail) => {
                    ContinuationObserveError::Inconsistent(detail)
                }
            })?;
        Ok(ContinuationObservation::Matched { receipt, status })
    }

    /// The authoritative status of `(owner, key)`, read from the store.
    ///
    /// A row not yet admitted is [`ContinuationStatus::Stranded`] only when
    /// its owners' records prove the owner retired
    /// ([`ContinuationAddressResolver::observe_address`]) and, read again
    /// after that observation, no reservation exists for it; it is pending
    /// otherwise. Stranded is final: a drain observes the address after it
    /// reserves and before it admits, so a reservation made before this
    /// observation is seen by the second read, and a drain that reserves
    /// after it finds the address retired, releases its reservation and
    /// admits nothing. A reservation left on a retired address is settled
    /// by the delivery owner from the reserved session's ledger. Nothing is
    /// served to answer: no owner is revived, no executor attached and no
    /// hosting claimed, which stays delivery work. An observation that
    /// cannot be made now is [`ContinuationStatusError::Unavailable`].
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
                    // crash; only that session's ledger can tell, read by the
                    // next delivery attempt or, once the address retired, by
                    // the delivery owner under the session's custody (it
                    // applies or releases the reservation). So a reserved row
                    // is never reported stranded.
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
                    .observe_address(&binding.address)
                    .await
                    .map_err(ContinuationStatusError::Unavailable)?
                {
                    // Observe, then re-read: a drain reserves before it
                    // observes, so a reservation made before this observation
                    // is seen here, and one made after it finds the address
                    // retired and admits nothing.
                    AddressObservation::Retired => match self
                        .inbox
                        .continuation_admission(&binding.address, &delivery_id)
                        .await
                        .map_err(|error| status_unavailable(&error))?
                    {
                        None => Ok(ContinuationStatus::Stranded {
                            receipt,
                            cause: StrandedCause::OwnerRetired,
                        }),
                        Some(ContinuationAdmission::Reserved { .. }) => {
                            Ok(ContinuationStatus::Pending {
                                receipt,
                                sequence: delivery_sequence,
                                admitted: None,
                            })
                        }
                        Some(ContinuationAdmission::Applied {
                            session_id,
                            input_id,
                        }) => Ok(ContinuationStatus::Pending {
                            receipt,
                            sequence: delivery_sequence,
                            admitted: Some((session_id, input_id)),
                        }),
                    },
                    AddressObservation::MayServe => Ok(ContinuationStatus::Pending {
                        receipt,
                        sequence: delivery_sequence,
                        admitted: None,
                    }),
                }
            }
        }
    }
}

/// Why a continuation was not admitted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ContinuationAdmitError {
    /// A terminal policy outcome: the delivery is settled as refused, never
    /// admitted, and the deliveries after it proceed.
    #[error("continuation refused: {0:?}")]
    Refused(meerkat_runtime::RuntimeDeliveryRefusalReason),
    /// Not admitted now (an unavailable owner, an observation or store
    /// failure); the delivery stays pending and is retried.
    #[error("continuation not admitted: {0}")]
    Failed(String),
    /// Not admitted now because the runtime is not ready to admit it, for a
    /// typed cause: for example an original whose admission its source owner
    /// has not settled yet. The delivery stays pending and is retried, as for
    /// [`Self::Failed`], and its drain reports the cause.
    #[error("continuation not admitted yet: {0}")]
    NotReady(meerkat_core::authorization::ControllerReadinessFailure),
    /// Not admitted because the delivery address retired after the drain
    /// resolved it: the drain released its own reservation, so the delivery
    /// reads stranded, and nothing is admitted for it again.
    #[error("continuation not admitted: its delivery address retired")]
    AddressRetired,
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

/// The committed owner of fork_off and council jobs (the mob runtime), and
/// of the `Host` namespaces a host composes into it (its committed setup
/// results and its host originals): the one place a retained completion's
/// job, and a host original, is confirmed. It answers
/// [`RetainedJobLookup::Absent`] (and [`HostOriginalLookup::Absent`]) for any
/// producer or namespace it does not own. A composed source forwards the host
/// original methods for its declared namespaces only; Core refuses the
/// reserved namespaces ([`RESERVED_HOST_NAMESPACE_PREFIX`]) whatever a source
/// answers.
#[async_trait::async_trait]
pub trait RetainedJobSource: Send + Sync {
    /// What this owner committed about `producer`'s job `job_id`.
    async fn retained_job(
        &self,
        producer: &ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup;

    /// What this owner committed about host original `producer_id` in its
    /// `namespace`: the exact continuation, the owner it is owed to and the
    /// original it binds. The default owns no host original.
    async fn host_original(&self, namespace: &str, producer_id: &str) -> HostOriginalLookup {
        let _ = (namespace, producer_id);
        HostOriginalLookup::Absent
    }

    /// The trusted host's binder of host originals in `namespace`: the one
    /// way a governed runtime admits them. `None`, the default, offers none,
    /// and such host originals are refused there.
    fn host_original_binder(&self, namespace: &str) -> Option<Arc<dyn HostOriginalBinder>> {
        let _ = namespace;
        None
    }
}

/// What the committed owner of a host namespace holds about one host
/// original: the exact continuation it owes (key, result reference, body
/// and handling), the owner it is owed to, and the original whose authority
/// a binding must name. Facts only: nothing here grants authority.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostOriginalFacts {
    pub owner: ContinuationOwner,
    pub delivery: ContinuationDelivery,
    pub original_work: meerkat_runtime::OriginalWorkRef,
}

impl HostOriginalFacts {
    pub fn new(
        owner: ContinuationOwner,
        delivery: ContinuationDelivery,
        original_work: meerkat_runtime::OriginalWorkRef,
    ) -> Self {
        Self {
            owner,
            delivery,
            original_work,
        }
    }
}

impl std::fmt::Debug for HostOriginalFacts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostOriginalFacts")
            .field("owner", &self.owner)
            .field("delivery", &self.delivery)
            .finish_non_exhaustive()
    }
}

/// A namespace owner's answer for one host original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostOriginalLookup {
    /// The owner holds it and committed these facts.
    Found(HostOriginalFacts),
    /// The owner definitively holds no such original in that namespace.
    Absent,
    /// The owner could not be read now; retryable, never a verdict.
    Unavailable(String),
}

/// A namespace owner's committed record of one host original, read from it.
/// Process-only and minted only through [`HostOriginalRecord::from_owner`].
/// Any source can be passed, so it is a consistency check at submit, not a
/// gate: the drain confirms the row again against the bound owner.
pub struct HostOriginalRecord {
    namespace: String,
    producer_id: String,
    facts: HostOriginalFacts,
}

impl HostOriginalRecord {
    /// Read host original `producer_id` of `namespace` from its committed
    /// owner.
    ///
    /// # Errors
    /// The owner could not be read.
    pub async fn from_owner(
        owner: &dyn RetainedJobSource,
        namespace: impl Into<String>,
        producer_id: impl Into<String>,
    ) -> Result<Option<Self>, String> {
        let namespace = namespace.into();
        let producer_id = producer_id.into();
        match owner.host_original(&namespace, &producer_id).await {
            HostOriginalLookup::Found(facts) => Ok(Some(Self {
                namespace,
                producer_id,
                facts,
            })),
            HostOriginalLookup::Absent => Ok(None),
            HostOriginalLookup::Unavailable(error) => Err(error),
        }
    }

    /// The committed facts.
    pub fn facts(&self) -> &HostOriginalFacts {
        &self.facts
    }
}

impl std::fmt::Debug for HostOriginalRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostOriginalRecord")
            .field("namespace", &self.namespace)
            .field("producer_id", &self.producer_id)
            .finish_non_exhaustive()
    }
}

/// What Core confirmed for one host original, handed to its binder.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostOriginalRef {
    pub namespace: String,
    pub producer_id: String,
    pub owner: ContinuationOwner,
    pub original_work: meerkat_runtime::OriginalWorkRef,
}

impl std::fmt::Debug for HostOriginalRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostOriginalRef")
            .field("namespace", &self.namespace)
            .field("producer_id", &self.producer_id)
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

/// The native admission a bound host original's settlement is promoted for:
/// the admitting session and the input its native admission accepted as new
/// work.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostOriginalAdmission {
    pub session: SessionId,
    pub input: InputId,
    pub deduplicated: bool,
}

/// The host's settlement custody for one bound host original (for example
/// its pending candidate of the original). Core calls
/// [`Self::accepted`] at most once, only when native admission ACCEPTED that
/// exact bound input as new work. Dropping it unsettled is the host's
/// withdrawal: Core drops it only when nothing was handed to native
/// admission, or native admission settled `NotAdmitted`, or a replay
/// deduplicated onto an existing input. When the admission is unresolved it
/// is handed to [`HostOriginalBinder::retain_unresolved`] instead.
pub trait HostOriginalSettlement: Send + Sync {
    /// Promote the host's custody for the admission. An error is reported
    /// (a warn trace), never a refusal: the input was admitted. On an error
    /// Core still applies and acknowledges the delivery and never calls this
    /// again; the host keeps its candidate pending and settles it from the
    /// native record (`with_admitted_original` / `durable_admitted_original`)
    /// within its own bound, the same obligation as an `AdmissionPending`
    /// answer of its source.
    fn accepted(self: Box<Self>, admission: &HostOriginalAdmission) -> Result<(), String>;
}

/// One bound host original with its host settlement custody.
#[non_exhaustive]
pub struct BoundHostOriginal {
    /// Core's exact continuation input with only its authority association
    /// set and the trusted ingress observation attached (for example through
    /// `bind_scoped_original`, passing Core's admission key as its event key).
    pub input: meerkat_runtime::Input,
    pub settlement: Box<dyn HostOriginalSettlement>,
}

impl BoundHostOriginal {
    pub fn new(input: meerkat_runtime::Input, settlement: Box<dyn HostOriginalSettlement>) -> Self {
        Self { input, settlement }
    }
}

impl std::fmt::Debug for BoundHostOriginal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundHostOriginal")
            .field("input", &self.input.id())
            .finish_non_exhaustive()
    }
}

/// Why a binder bound nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostOriginalRefusal {
    /// The host's actual verdict: the original is never admitted for this
    /// owner and session. Terminal for the row (`AuthorityDenied`).
    Denied,
    /// The binder does not own this original: an immutably missing binding.
    NotOwned,
    /// Not answerable now, for a typed cause (a held turn boundary, a session
    /// that is not live, contention): the row stays pending.
    NotReady(meerkat_core::authorization::ControllerReadinessFailure),
    /// The host could not be read now; the row stays pending.
    Unavailable(String),
}

/// A binder's bounded precheck of an owner, for setup.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostOriginalBinderReadiness {
    /// The binder could bind for the owner's current session now.
    Ready,
    /// The binder's actual verdict for this owner now.
    Denied,
    /// Not answerable now, for a typed cause.
    NotReady(meerkat_core::authorization::ControllerReadinessFailure),
}

/// What a runtime will do with the host originals of one namespace for one
/// owner, as its BOUND composition answers at setup. Advisory: the decisive
/// verdict is per original, at native admission.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostOriginalReadiness {
    /// No native work authorization host: host continuations are admitted
    /// as they always were.
    Ungoverned,
    /// Governed, but no continuation services are bound yet: ask again after
    /// binding.
    Unbound,
    /// Governed, and the namespace is reserved or the bound source offers no
    /// binder for it: every host original is refused (`NoAdmissibleWorkBinding`).
    NotOffered,
    /// Governed, and the bound source offers a binder: its precheck now.
    Offered(HostOriginalBinderReadiness),
}

/// The trusted host's binder of host originals in the namespaces its source
/// owns: on a governed runtime, the one way a host original is admitted.
///
/// Contract:
/// - [`Self::bind`] and [`Self::readiness`] are bounded. They never await a
///   member turn, an identity lifecycle operation, a writer holding the
///   host's roster or entries, or anything that waits on the delivery owner
///   (which drains every runtime in one sequential pass). When they cannot
///   answer now they answer `NotReady` with a typed cause. Pin the session's
///   controller without waiting (the session services' non-waiting pins), or
///   keep a pin retained per live actor, refreshed off the drain path and
///   announced with [`ContinuationHostBindings::source_custody_changed`].
/// - [`Self::bind`] binds Core's exact input for the session's CURRENT
///   destination and pinned controller, fresh at each attempt: it sets only
///   the authority association (naming `original.original_work`) and the
///   trusted ingress observation. With `bind_scoped_original`, pass Core's
///   admission key (the input's idempotency key) as its event key. Core
///   refuses any other change. Attach the ingress observation only through
///   `Input::with_ingress_context` or `bind_scoped_original`, which check it
///   against the final input: an observation of another input set on the
///   header directly is refused by native admission and leaves the row
///   pending. The original is in the host's own source custody before
///   `bind` returns.
/// - Core never makes a governed session live, on any sink: a session with
///   no live actor (a cold session) is the host's to recover through its own
///   governed recovery, which binds its destination and agent before the
///   agent is constructed. Until then [`Self::bind`] answers
///   `NotReady(ExecutorUnavailable)` and [`Self::readiness`] answers
///   `NotReady(ExecutorUnavailable)` (so the setup signal reads
///   `Offered(NotReady(..))`). The host's executor attachment (or
///   [`ContinuationHostBindings::source_custody_changed`]) wakes the delivery
///   owner for the next attempt. A host that wants ingestion to wake a cold
///   session runs its own recovery; Core never does. A pending host original
///   holds its session's later deliveries in order until it settles, so a
///   cold session's later rows wait for the host's recovery too.
/// - A binder may make bounded host-side checks before it binds, such as
///   re-checking that the owner covers the source's audience from a snapshot
///   or a try-read of the registered roster row. Such a check answers
///   `Denied` only on an actual failure (terminal for the row) and `NotReady`
///   when the state cannot be read now. It covers the member and session
///   being bound and stops at the bind: an admitted row is never refused
///   later.
#[async_trait::async_trait]
pub trait HostOriginalBinder: Send + Sync {
    /// Bind `input`, Core's continuation input for the confirmed host original
    /// `original`, for `session`.
    async fn bind(
        &self,
        original: &HostOriginalRef,
        session: &SessionId,
        input: meerkat_runtime::Input,
    ) -> Result<BoundHostOriginal, HostOriginalRefusal>;

    /// The binder's bounded precheck for `owner` in `namespace`. It never
    /// enrolls, binds or admits anything, and never waits on a busy member.
    async fn readiness(
        &self,
        namespace: &str,
        owner: &ContinuationOwner,
    ) -> HostOriginalBinderReadiness;

    /// Keep the custody of a bound host original whose native admission is
    /// unresolved (the input may have been admitted): the host settles it
    /// later from the runtime's own record of the admission.
    fn retain_unresolved(&self, settlement: Box<dyn HostOriginalSettlement>, detail: &str);
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
///
/// The bindings also carry the delivery owners' source hint
/// ([`Self::subscribe_source_changes`]): every binding and rebinding, and
/// every [`Self::source_custody_changed`] the bound job owner announces,
/// wakes the delivery owners subscribed to it to retry the rows they hold
/// blocked. The hint is only a wake, never a fact: a retry asks the bound
/// source again, and the source alone decides.
pub struct ContinuationHostBindings {
    slot: std::sync::Mutex<BindingSlot>,
    /// Advances after every binding and every announced custody change.
    source_changes: tokio::sync::watch::Sender<u64>,
}

impl Default for ContinuationHostBindings {
    fn default() -> Self {
        Self {
            slot: std::sync::Mutex::default(),
            source_changes: tokio::sync::watch::Sender::new(0),
        }
    }
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
            .field("source_changes", &*self.source_changes.borrow())
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
        let binding = {
            let mut slot = self.slot();
            if slot.live().is_some() {
                return Err(ContinuationBindError::AlreadyBound(
                    ContinuationBindingGeneration(slot.generation),
                ));
            }
            Self::install(&mut slot, resolver, job_source)
        };
        // Published once the services are readable through the slot.
        self.publish_source_change();
        Ok(binding)
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
        let binding = {
            let mut slot = self.slot();
            let current = ContinuationBindingGeneration(slot.generation);
            if replaced != current {
                return Err(ContinuationBindError::StaleGeneration { replaced, current });
            }
            Self::install(&mut slot, resolver, job_source)
        };
        // Published once the services are readable through the slot.
        self.publish_source_change();
        Ok(binding)
    }

    /// Observe source changes of these continuation services: the value
    /// advances after every binding, every rebinding and every
    /// [`Self::source_custody_changed`]. It carries no state; a delivery
    /// owner asks the bound source again on every advance. The signal closes
    /// when these bindings are dropped.
    pub fn subscribe_source_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.source_changes.subscribe()
    }

    /// Announce that the bound job owner's committed facts may have become
    /// readable, for example once its custody is restored or its pin
    /// recorded, or that a source owner settled an original that a retained
    /// resume may wait on (it promoted or withdrew its candidate): every
    /// delivery owner subscribed to these bindings retries the rows it holds
    /// blocked, asking the source again. Call it after the facts are readable
    /// through the source. It grants nothing and asserts nothing; an
    /// announcement with nothing changed costs one retry.
    pub fn source_custody_changed(&self) {
        self.publish_source_change();
    }

    /// Announce, as [`Self::source_custody_changed`], each terminal `runtime`
    /// makes readable without a run: an exact cancellation abandoned a queued
    /// input, or an admission superseded or coalesced one. A delivery owner
    /// subscribed to these bindings then retries the rows it holds blocked
    /// and starts a round of its host's owed continuations, which reads the
    /// native terminal again, instead of waiting for a reconcile. `runtime`
    /// holds these bindings only weakly; installing again replaces the
    /// runtime's earlier [`meerkat_runtime::RunlessInputTerminalHint`].
    pub fn announce_runless_input_terminals_of(
        self: &Arc<Self>,
        runtime: &meerkat_runtime::MeerkatMachine,
    ) {
        let bindings = Arc::downgrade(self);
        runtime.set_runless_input_terminal_hint(Arc::new(move || {
            if let Some(bindings) = bindings.upgrade() {
                bindings.source_custody_changed();
            }
        }));
    }

    fn publish_source_change(&self) {
        self.source_changes
            .send_modify(|generation| *generation = generation.wrapping_add(1));
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

    /// What `runtime` will do with host originals of `namespace` owed to
    /// `owner`, as THIS binding's composition answers now: the setup-time
    /// signal a host reads before committing any. It reads the bound source,
    /// so setup and delivery cannot disagree about the composition. It
    /// enrolls, binds and admits nothing; the binder's precheck is bounded.
    pub async fn host_original_readiness(
        &self,
        runtime: &meerkat_runtime::MeerkatMachine,
        namespace: &str,
        owner: &ContinuationOwner,
    ) -> HostOriginalReadiness {
        if !runtime.has_native_work_authorization_host() {
            return HostOriginalReadiness::Ungoverned;
        }
        if namespace.is_empty() || is_reserved_host_namespace(namespace) {
            return HostOriginalReadiness::NotOffered;
        }
        let Some(source) = self.job_source() else {
            return HostOriginalReadiness::Unbound;
        };
        match source.host_original_binder(namespace) {
            Some(binder) => {
                HostOriginalReadiness::Offered(binder.readiness(namespace, owner).await)
            }
            None => HostOriginalReadiness::NotOffered,
        }
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

    /// Observe `address` through the bound resolver without serving it, as
    /// a continuation status read does
    /// ([`ContinuationAddressResolver::observe_address`]). With none bound,
    /// nothing proves a retirement. A delivery owner's drain observes the
    /// address with this after it reserved and before it admits, so it
    /// agrees with the status read on Retired.
    ///
    /// # Errors
    /// The resolver's records cannot be read now.
    pub async fn observe_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<AddressObservation, String> {
        match self.resolver() {
            Some(resolver) => resolver.observe_address(address).await,
            None => Ok(AddressObservation::MayServe),
        }
    }

    /// The delivery address of the mob member `session_id` serves now,
    /// through the bound resolver
    /// ([`ContinuationAddressResolver::member_address_of_session`]). `None`
    /// without a binding, for a session no member serves, and when the
    /// resolver cannot tell: a job then keeps its session's address.
    pub async fn member_address_of_session(
        &self,
        session_id: &SessionId,
    ) -> Option<LogicalRuntimeId> {
        let resolver = self.resolver()?;
        match resolver.member_address_of_session(session_id).await {
            Ok(address) => address,
            Err(error) => {
                tracing::warn!(
                    %session_id,
                    %error,
                    "the member a submitting session serves is unknown; its job keeps the session's address"
                );
                None
            }
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

    /// Whether this sink admits governed host originals
    /// ([`Self::admit_governed`]). A governed runtime refuses a host original
    /// typed, never leaving it pending, on a sink that answers `false`, the
    /// default.
    fn admits_governed(&self) -> bool {
        false
    }

    /// Prepare `session` for a governed host original before its binder
    /// binds it, as [`Self::admit_retained`] prepares a cold session (for
    /// example settling a crash-window provisional tail). It never makes a
    /// governed session live, on any sink: liveness belongs to the host, and
    /// its executor attachment wakes the delivery owner. The default prepares
    /// nothing.
    async fn prepare_governed(&self, session: &SessionId) -> Result<(), ContinuationAdmitError> {
        let _ = session;
        Ok(())
    }

    /// Admit the bound governed host original `input` into `session`, with
    /// `custody` settled by native admission exactly once
    /// (`MeerkatMachine::accept_input_with_completion_and_custody`). The raw
    /// native result is returned; the caller classifies it and acts on the
    /// recorded settlement. The default admits nothing and settles the custody
    /// `NotAdmitted`.
    async fn admit_governed(
        &self,
        session: &SessionId,
        input: meerkat_runtime::Input,
        custody: Box<dyn meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody>,
    ) -> Result<GovernedAdmissionOutcome, meerkat_runtime::RuntimeDriverError> {
        let _ = (session, input);
        let _ = custody.settle(
            meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement::NotAdmitted,
        );
        Err(meerkat_runtime::RuntimeDriverError::Internal(
            "this continuation sink does not admit governed host originals".into(),
        ))
    }

    /// The row `session` holds under the exact input id `input_id`, read from
    /// live state and then the durable witness. The default cannot read one
    /// and answers an error, which keeps the delivery pending: it never reads
    /// as "nothing admitted".
    async fn admitted_input_by_id(
        &self,
        session: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<meerkat_runtime::input_state::StoredInputState>, String> {
        let _ = (session, input_id);
        Err("this continuation sink cannot read an admission by its input id".into())
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

    fn admits_governed(&self) -> bool {
        true
    }

    async fn admit_governed(
        &self,
        session: &SessionId,
        input: meerkat_runtime::Input,
        custody: Box<dyn meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody>,
    ) -> Result<GovernedAdmissionOutcome, meerkat_runtime::RuntimeDriverError> {
        self.runtime
            .accept_input_with_completion_and_custody(session, input, custody)
            .await
            .map(|(outcome, _completion)| outcome.into())
    }

    async fn admitted_input_by_id(
        &self,
        session: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<meerkat_runtime::input_state::StoredInputState>, String> {
        use meerkat_runtime::SessionServiceRuntimeExt as _;
        self.runtime
            .input_state(session, input_id)
            .await
            .map_err(|error| error.to_string())
    }
}

/// Classify a retained-resume admission: only a terminal policy refusal is
/// [`ContinuationAdmitError::Refused`]; every other error is retryable, and a
/// runtime that is not ready for a typed cause keeps that cause
/// ([`ContinuationAdmitError::NotReady`]).
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
        Err(meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable { reason }) => {
            Err(ContinuationAdmitError::NotReady(reason))
        }
        Err(error) => Err(ContinuationAdmitError::Failed(error.to_string())),
    }
}

/// What native admission did with one bound governed host original.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GovernedAdmissionOutcome {
    /// Accepted as new work under this input id.
    Accepted(InputId),
    /// Deduplicated onto this existing input.
    Deduplicated(InputId),
    /// Neither (for example rejected), with its rendering.
    NotAccepted(String),
}

impl From<meerkat_runtime::AcceptOutcome> for GovernedAdmissionOutcome {
    fn from(outcome: meerkat_runtime::AcceptOutcome) -> Self {
        match outcome {
            meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } => Self::Accepted(input_id),
            meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. } => {
                Self::Deduplicated(existing_id)
            }
            other => Self::NotAccepted(format!("{other:?}")),
        }
    }
}

/// Classify a governed host original's native admission: only an actual
/// refusal verdict is [`ContinuationAdmitError::Refused`] (a denial is
/// `AuthorityDenied`, malformed facts `NoAdmissibleWorkBinding`); a runtime
/// that is not ready keeps its typed cause; anything else is retryable.
/// Classification only: the host's custody follows the native admission
/// settlement, never this result.
pub fn governed_admission_outcome(
    outcome: Result<GovernedAdmissionOutcome, meerkat_runtime::RuntimeDriverError>,
) -> Result<InputId, ContinuationAdmitError> {
    use meerkat_core::authorization::{ControllerReadinessFailure, OperationRefusalKind};
    use meerkat_runtime::RuntimeDeliveryRefusalReason;
    match outcome {
        Ok(GovernedAdmissionOutcome::Accepted(input_id)) => Ok(input_id),
        Ok(GovernedAdmissionOutcome::Deduplicated(existing_id)) => Ok(existing_id),
        Ok(GovernedAdmissionOutcome::NotAccepted(detail)) => Err(ContinuationAdmitError::Failed(
            format!("governed host original admission was not accepted: {detail}"),
        )),
        Err(meerkat_runtime::RuntimeDriverError::InputRefused { refusal }) => {
            match refusal.kind() {
                OperationRefusalKind::Denied => Err(ContinuationAdmitError::Refused(
                    RuntimeDeliveryRefusalReason::AuthorityDenied,
                )),
                OperationRefusalKind::MalformedFacts => Err(ContinuationAdmitError::Refused(
                    RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding,
                )),
                OperationRefusalKind::ReprepareRequired => Err(ContinuationAdmitError::NotReady(
                    ControllerReadinessFailure::PolicyChanged,
                )),
            }
        }
        Err(meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable { reason }) => {
            Err(ContinuationAdmitError::NotReady(reason))
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
/// delivery attempt for the row, and once its address retired by the owner
/// from the reserved session's ledger ([`settle_retired_reservation`]). The
/// caller acknowledges the row on `Ok`.
///
/// With an `observer`, the drain observes the address after its reservation
/// (a Reserve or Repoint) and before the admission: an address that retired
/// since the pass resolved it gets its reservation released and nothing
/// admitted ([`crate::job_delivery::JobOutboxProjectionError::AddressRetired`]).
/// The observation must agree on Retired with the status read
/// ([`ContinuationOwnerService::continuation_status`]), which is what makes a
/// stranded answer final.
///
/// A continuation carries no native work binding yet: on a sink whose runtime
/// governs work authority the row is refused
/// ([`meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding`])
/// before anything is reserved or admitted. The drain settles it as refused,
/// never applied, and the rows after it proceed. This covers rows committed
/// before the runtime was governed (historical rows) and rows from any other
/// submitter; [`ContinuationOwnerService::submit`] already refuses new ones
/// on a governed runtime without committing them.
///
/// A host original row (the marker set by
/// [`ContinuationOwnerService::submit_host_original`]) is routed first, on
/// every posture, to [`admit_host_original`].
pub(crate) async fn admit_continuation_row(
    inbox: &RuntimeDeliveryInbox,
    address: &LogicalRuntimeId,
    record: &meerkat_runtime::RuntimeDeliveryRecord,
    sink: &dyn ContinuationDeliverySink,
    session: &SessionId,
    job_source: Option<&dyn RetainedJobSource>,
    observer: Option<&dyn crate::RuntimeDeliveryHost>,
) -> Result<(), crate::job_delivery::JobOutboxProjectionError> {
    let delivery_id = record.submission.delivery_id();
    let outcome = async {
        let row: ContinuationRow = serde_json::from_slice(record.submission.payload())
            .map_err(|error| format!("continuation delivery payload is invalid: {error}"))?;
        if row.host_original {
            let target = AdmissionTarget {
                inbox,
                address,
                sink,
                session,
                observer,
            };
            return admit_host_original(&target, record, job_source, row).await;
        }
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
        let target = AdmissionTarget {
            inbox,
            address,
            sink,
            session,
            observer,
        };
        admit_continuation(&target, record, row.delivery, retained).await
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
        Err(ContinuationAdmitError::NotReady(reason)) => {
            Err(crate::job_delivery::JobOutboxProjectionError::NotReady {
                delivery_id: delivery_id.to_string(),
                reason,
            })
        }
        Err(ContinuationAdmitError::AddressRetired) => Err(
            crate::job_delivery::JobOutboxProjectionError::AddressRetired {
                delivery_id: delivery_id.to_string(),
            },
        ),
    }
}

/// The committed job owner must confirm the completion exactly: the same
/// producer and job, the same retained work, the destination session it is
/// owed to and the result it committed. Anything else is an immutably
/// invalid binding; a failure to ask is retryable.
///
/// A `Host` completion is confirmed the same way, by the configured source:
/// the host composes into it the owner of the namespaces it actually
/// produces (a setup result it committed). The namespace string itself
/// proves nothing; a source that does not own that namespace and job
/// answers [`RetainedJobLookup::Absent`], which refuses the row.
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
    match &delivery.result.producer {
        ContinuationProducer::ForkOff | ContinuationProducer::Council => {}
        // An empty namespace names no host operation.
        ContinuationProducer::Host { namespace } if !namespace.is_empty() => {}
        ContinuationProducer::Host { .. } => return Err(invalid()),
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

/// The exact input a continuation row is admitted as, under `admission_key`
/// with `input_id`: a durable content input, or a notice admitted exactly as a
/// detached job's completion record.
fn continuation_prompt(
    delivery: &ContinuationDelivery,
    admission_key: &str,
    input_id: InputId,
) -> meerkat_runtime::PromptInput {
    let handling = match delivery.handling {
        ContinuationHandling::Queue => meerkat_core::types::HandlingMode::Queue,
        ContinuationHandling::Steer => meerkat_core::types::HandlingMode::Steer,
    };
    match &delivery.body {
        ContinuationBody::Content { content } => meerkat_runtime::PromptInput::continuation(
            input_id,
            admission_key.to_string(),
            content.clone(),
            handling,
        ),
        ContinuationBody::Notice {
            notice_kind,
            body,
            blocks,
        } => {
            let mut prompt = meerkat_runtime::PromptInput::detached_job_completed(
                admission_key.to_string(),
                meerkat_core::types::SystemNoticeMessage::with_blocks(
                    *notice_kind,
                    body.clone(),
                    blocks.clone(),
                ),
            );
            prompt.header.id = input_id;
            prompt
        }
    }
}

/// Where a governed host original's refusal came from, for operators: the
/// delivery row keeps only the three typed reasons. Reported only as the
/// `refusal_origin` field of a warn trace, with the variant's Debug name:
/// `RowShape`, `ReservedNamespace`, `OwnerConfirmation`, `BinderDenied` (a
/// binder's `Denied`, and its `NotOwned` with the `NoAdmissibleWorkBinding`
/// reason), `BoundInput` or `NativeRefused`. Diagnostic only: no decision may
/// read it, and a later typed origin would be a new additive API.
#[derive(Debug, Clone, Copy)]
enum HostOriginalRefusalOrigin {
    RowShape,
    ReservedNamespace,
    OwnerConfirmation,
    BinderDenied,
    BoundInput,
    NativeRefused,
}

fn refused_host_original(
    origin: HostOriginalRefusalOrigin,
    reason: meerkat_runtime::RuntimeDeliveryRefusalReason,
    delivery_id: &RuntimeDeliveryId,
    detail: &str,
) -> ContinuationAdmitError {
    tracing::warn!(
        delivery_id = %delivery_id,
        refusal_origin = ?origin,
        refusal_reason = ?reason,
        detail,
        "host original refused"
    );
    ContinuationAdmitError::Refused(reason)
}

/// Records the native admission settlement of one bound host original. It
/// only records: the drain acts on the settlement after admission returns.
struct RecordedHostOriginalSettlement(
    Arc<
        std::sync::Mutex<
            Option<meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement>,
        >,
    >,
);

impl meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody
    for RecordedHostOriginalSettlement
{
    fn settle(
        self: Box<Self>,
        settlement: meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement,
    ) -> Option<meerkat_runtime::input_admission_custody::NativeInputAdmissionCompletion> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(settlement);
        None
    }
}

/// Whether `state`, the row a session holds under `input_id`, is this host
/// original's admission: Core's continuation input under that id and the
/// admission key, with the row's OWN recorded association (none for a row an
/// ungoverned sink admitted), as an exact replay. Never decided by key alone.
fn verify_admitted_host_original(
    state: &meerkat_runtime::input_state::InputState,
    delivery: &ContinuationDelivery,
    admission_key: &str,
    input_id: &InputId,
) -> Result<(), ContinuationAdmitError> {
    let mut expected = continuation_prompt(delivery, admission_key, input_id.clone());
    expected.header.authority_association = state
        .authority_contributors
        .iter()
        .find(|contributor| contributor.input_id() == &state.input_id)
        .map(|own| own.association().clone());
    meerkat_runtime::verify_exact_replay(state, &meerkat_runtime::Input::Prompt(expected)).map_err(
        |error| {
            ContinuationAdmitError::Failed(format!(
                "input {} is not this host original's admission under key {admission_key}: {error}",
                state.input_id
            ))
        },
    )
}

/// The checks a bound host original must pass before anything is reserved or
/// handed to native admission: it is exactly the input Core handed over with
/// only its authority association set and its trusted ingress observation
/// attached; the association names this session's runtime, a controller
/// grant lineage and model, and the confirmed original; the ingress names the
/// association's requester, actor and realm, with a controller client of the
/// association's selection. The runtime's shared check of a bound original
/// ([`meerkat_runtime::input_authority::check_bound_original`]), the one the
/// governed peer seam applies to a bound delivery.
fn check_bound_host_original(
    handed: &meerkat_runtime::PromptInput,
    bound: &meerkat_runtime::Input,
    session: &SessionId,
    original_work: &meerkat_runtime::OriginalWorkRef,
) -> Result<(), String> {
    meerkat_runtime::input_authority::check_bound_original(
        &meerkat_runtime::Input::Prompt(handed.clone()),
        bound,
        &LogicalRuntimeId::for_session(session),
        meerkat_runtime::input_authority::BoundOriginalPin::Original(original_work),
    )
}

/// Admit one host original row ([`ContinuationOwnerService::submit_host_original`]).
///
/// 1. The admission index first, on every posture: an `Applied` row is
///    acknowledged, and a `Reserved` row whose reserved session holds the
///    reserved input id is verified as this exact admission (with the row's
///    own recorded association) and applied. Neither the namespace owner nor
///    the binder is asked, so an admitted input is never refused later.
/// 2. Static checks on the paths that still admit: a `Host` producer, no
///    retained work, no reserved namespace. An ungoverned sink then admits a
///    plain continuation. A governed sink needs `Queue` handling and a sink
///    that admits governed host originals.
/// 3. The BOUND namespace owner confirms the exact delivery, and the key
///    ledger binds its owner and key to this row.
/// 4. The session is prepared (never made live here: liveness is the
///    host's), and the owner's binder binds Core's exact input fresh for it;
///    Core checks the bound input.
/// 5. A fresh id is reserved, and the address is observed again
///    ([`observe_after_reservation`]): retired since the pass resolved it,
///    the reservation is released and the custody dropped. Then native
///    admission settles the host's custody: promoted only on an accepted
///    admission of the bound id, dropped when nothing was admitted or a
///    replay deduplicated, retained unresolved otherwise.
async fn admit_host_original(
    target: &AdmissionTarget<'_>,
    record: &meerkat_runtime::RuntimeDeliveryRecord,
    job_source: Option<&dyn RetainedJobSource>,
    row: ContinuationRow,
) -> Result<(), ContinuationAdmitError> {
    use HostOriginalRefusalOrigin as Origin;
    use meerkat_runtime::RuntimeDeliveryRefusalReason::{AuthorityDenied, NoAdmissibleWorkBinding};
    use meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement;
    let AdmissionTarget {
        inbox,
        address,
        sink,
        session,
        observer: _,
    } = *target;
    let delivery_id = record.submission.delivery_id();
    let delivery = row.delivery;
    let admission_key = delivery.admission_key(delivery_id);
    let transition = |transition: ContinuationAdmissionTransition| {
        transition_admission(inbox, address, delivery_id, transition)
    };

    // 1. The admission index first.
    let reserved_id = match inbox
        .continuation_admission(address, delivery_id)
        .await
        .map_err(|error| error.to_string())?
    {
        Some(ContinuationAdmission::Applied { .. }) => return Ok(()),
        Some(ContinuationAdmission::Reserved {
            session_id: reserved,
            input_id,
        }) => match sink
            .admitted_input_by_id(&reserved, &input_id)
            .await
            .map_err(ContinuationAdmitError::Failed)?
        {
            Some(admitted) => {
                verify_admitted_host_original(
                    &admitted.state,
                    &delivery,
                    &admission_key,
                    &input_id,
                )?;
                transition(ContinuationAdmissionTransition::Apply {
                    session_id: reserved,
                    input_id,
                })
                .await?;
                return Ok(());
            }
            None => {
                if reserved != *session {
                    transition(ContinuationAdmissionTransition::Repoint {
                        from: reserved,
                        session_id: session.clone(),
                        input_id: input_id.clone(),
                    })
                    .await?;
                }
                Some(input_id)
            }
        },
        None => None,
    };

    // 2. Static checks on the paths that still admit.
    let namespace = match &delivery.result.producer {
        ContinuationProducer::Host { namespace }
            if !namespace.is_empty() && row.retained_work.is_none() =>
        {
            namespace.clone()
        }
        _ => {
            return Err(refused_host_original(
                Origin::RowShape,
                NoAdmissibleWorkBinding,
                delivery_id,
                "a host original row names a non-host producer or carries retained work",
            ));
        }
    };
    if is_reserved_host_namespace(&namespace) {
        return Err(refused_host_original(
            Origin::ReservedNamespace,
            NoAdmissibleWorkBinding,
            delivery_id,
            "a reserved host namespace",
        ));
    }
    if !sink.governs_work_authority() {
        // Ungoverned: the plain admission it always was.
        return admit_continuation(target, record, delivery, None).await;
    }
    if delivery.handling != ContinuationHandling::Queue || !sink.admits_governed() {
        return Err(refused_host_original(
            Origin::RowShape,
            NoAdmissibleWorkBinding,
            delivery_id,
            "a governed host original needs queue handling and a sink that admits it",
        ));
    }

    // 3. The bound namespace owner confirms the exact delivery.
    let source = job_source.ok_or_else(|| {
        ContinuationAdmitError::Failed(
            "no retained job source is configured for this governed runtime".into(),
        )
    })?;
    let facts = match source
        .host_original(&namespace, &delivery.result.producer_id)
        .await
    {
        HostOriginalLookup::Found(facts) => facts,
        HostOriginalLookup::Absent => {
            return Err(refused_host_original(
                Origin::OwnerConfirmation,
                NoAdmissibleWorkBinding,
                delivery_id,
                "its namespace owner holds no such host original",
            ));
        }
        HostOriginalLookup::Unavailable(error) => {
            return Err(ContinuationAdmitError::Failed(error));
        }
    };
    let ledger_digest =
        ContinuationOwnerService::submission_digest(&facts.owner, &delivery, None, true)
            .map_err(|error| ContinuationAdmitError::Failed(error.to_string()))?;
    let bound_to_row = inbox
        .continuation_key_binding(&facts.owner.ledger_owner(), delivery.key.as_str())
        .await
        .map_err(|error| error.to_string())?
        .is_some_and(|binding| {
            binding.address == *address
                && binding.delivery_id == delivery_id.as_str()
                && binding.submission_digest == ledger_digest
        });
    if facts.delivery != delivery || !bound_to_row {
        return Err(refused_host_original(
            Origin::OwnerConfirmation,
            NoAdmissibleWorkBinding,
            delivery_id,
            "its namespace owner committed another continuation or owner",
        ));
    }
    let Some(binder) = source.host_original_binder(&namespace) else {
        return Err(refused_host_original(
            Origin::OwnerConfirmation,
            NoAdmissibleWorkBinding,
            delivery_id,
            "its namespace owner offers no binder",
        ));
    };

    // 4. Prepare the session (never made live here), then bind fresh for it.
    sink.prepare_governed(session).await?;
    let input_id = reserved_id.clone().unwrap_or_else(InputId::new);
    let handed = continuation_prompt(&delivery, &admission_key, input_id.clone());
    let original = HostOriginalRef {
        namespace,
        producer_id: delivery.result.producer_id.clone(),
        owner: facts.owner.clone(),
        original_work: facts.original_work.clone(),
    };
    let bound = match binder
        .bind(
            &original,
            session,
            meerkat_runtime::Input::Prompt(handed.clone()),
        )
        .await
    {
        Ok(bound) => bound,
        Err(HostOriginalRefusal::Denied) => {
            return Err(refused_host_original(
                Origin::BinderDenied,
                AuthorityDenied,
                delivery_id,
                "its binder denied it",
            ));
        }
        Err(HostOriginalRefusal::NotOwned) => {
            return Err(refused_host_original(
                Origin::BinderDenied,
                NoAdmissibleWorkBinding,
                delivery_id,
                "its binder does not own it",
            ));
        }
        Err(HostOriginalRefusal::NotReady(reason)) => {
            return Err(ContinuationAdmitError::NotReady(reason));
        }
        Err(HostOriginalRefusal::Unavailable(error)) => {
            return Err(ContinuationAdmitError::Failed(error));
        }
    };
    let BoundHostOriginal {
        input: bound_input,
        settlement,
    } = bound;
    if let Err(detail) =
        check_bound_host_original(&handed, &bound_input, session, &facts.original_work)
    {
        // Never handed to native admission: dropping the custody withdraws it.
        drop(settlement);
        return Err(refused_host_original(
            Origin::BoundInput,
            NoAdmissibleWorkBinding,
            delivery_id,
            &detail,
        ));
    }

    // 5. Reserve a fresh id, then admit with native custody.
    if reserved_id.is_none() {
        transition(ContinuationAdmissionTransition::Reserve {
            session_id: session.clone(),
            input_id: input_id.clone(),
        })
        .await?;
    }
    // Reserve, then observe, as for every continuation: the pass resolved
    // the address before the binder bound, and it may have retired since.
    // Nothing has reached native admission, so dropping the custody
    // withdraws it, as for a bound input that fails its checks.
    if let Err(error) = observe_after_reservation(target, delivery_id, &input_id).await {
        drop(settlement);
        return Err(error);
    }
    let recorded = Arc::new(std::sync::Mutex::new(None));
    let result = sink
        .admit_governed(
            session,
            bound_input,
            Box::new(RecordedHostOriginalSettlement(Arc::clone(&recorded))),
        )
        .await;
    let settled = *recorded
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match (settled, result) {
        (
            Some(NativeInputAdmissionSettlement::Admitted),
            Ok(GovernedAdmissionOutcome::Accepted(admitted)),
        ) if admitted == input_id => {
            let admission = HostOriginalAdmission {
                session: session.clone(),
                input: admitted.clone(),
                deduplicated: false,
            };
            if let Err(error) = settlement.accepted(&admission) {
                // The input was admitted: the promotion failure is reported,
                // never a refusal.
                tracing::warn!(
                    delivery_id = %delivery_id,
                    input_id = %admitted,
                    error,
                    "host original admitted, but its host custody was not promoted"
                );
            }
            transition(ContinuationAdmissionTransition::Apply {
                session_id: session.clone(),
                input_id: admitted,
            })
            .await?;
            Ok(())
        }
        (
            Some(NativeInputAdmissionSettlement::Admitted),
            Ok(GovernedAdmissionOutcome::Deduplicated(existing_id)),
        ) => {
            // A replay onto an existing input: the retry's custody is
            // withdrawn, and the existing input stands for this delivery only
            // as its exact admission.
            drop(settlement);
            let existing = sink
                .admitted_input_by_id(session, &existing_id)
                .await
                .map_err(ContinuationAdmitError::Failed)?
                .ok_or_else(|| {
                    ContinuationAdmitError::Failed(format!(
                        "session {session} named input {existing_id} for host original key \
                         {admission_key} but holds no such row"
                    ))
                })?;
            verify_admitted_host_original(
                &existing.state,
                &delivery,
                &admission_key,
                &existing_id,
            )?;
            transition(ContinuationAdmissionTransition::Apply {
                session_id: session.clone(),
                input_id: existing_id,
            })
            .await?;
            Ok(())
        }
        (Some(NativeInputAdmissionSettlement::NotAdmitted), result) => {
            drop(settlement);
            match governed_admission_outcome(result) {
                Err(ContinuationAdmitError::Refused(reason)) => Err(refused_host_original(
                    Origin::NativeRefused,
                    reason,
                    delivery_id,
                    "native admission refused it",
                )),
                Err(error) => Err(error),
                Ok(other) => Err(ContinuationAdmitError::Failed(format!(
                    "native admission settled not admitted for input {other}"
                ))),
            }
        }
        (settled, result) => {
            // Admission may have happened: the custody stays with the host,
            // and the next attempt recovers by the reserved id.
            let detail = format!("native admission settled {settled:?}: {result:?}");
            binder.retain_unresolved(settlement, &detail);
            Err(ContinuationAdmitError::Failed(detail))
        }
    }
}

/// Where a drain admits one continuation row: the row's inbox and delivery
/// address, the sink and session serving that address, and the observer
/// that answers whether the address retired after the pass resolved it.
#[derive(Clone, Copy)]
struct AdmissionTarget<'a> {
    inbox: &'a RuntimeDeliveryInbox,
    address: &'a LogicalRuntimeId,
    sink: &'a dyn ContinuationDeliverySink,
    session: &'a SessionId,
    observer: Option<&'a dyn crate::RuntimeDeliveryHost>,
}

async fn admit_continuation(
    target: &AdmissionTarget<'_>,
    record: &meerkat_runtime::RuntimeDeliveryRecord,
    delivery: ContinuationDelivery,
    retained: Option<meerkat_core::retained_work::RetainedWorkIdentity>,
) -> Result<(), ContinuationAdmitError> {
    let AdmissionTarget {
        inbox,
        address,
        sink,
        session,
        observer: _,
    } = *target;
    let delivery_id = record.submission.delivery_id();
    let admission_key = delivery.admission_key(delivery_id);
    let retained_work = retained.as_ref();
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
            if let Some(admitted) =
                exact_admission(sink, &reserved, &delivery, &admission_key, retained_work).await?
            {
                return Ok(transition_admission(
                    inbox,
                    address,
                    delivery_id,
                    ContinuationAdmissionTransition::Apply {
                        session_id: reserved,
                        input_id: admitted,
                    },
                )
                .await?);
            }
            if reserved != *session {
                transition_admission(
                    inbox,
                    address,
                    delivery_id,
                    ContinuationAdmissionTransition::Repoint {
                        from: reserved,
                        session_id: session.clone(),
                        input_id: input_id.clone(),
                    },
                )
                .await?;
            }
            input_id
        }
        None => {
            // A completion delivered live under the same key before it became
            // a continuation stands for it only if it is an exact replay.
            if let Some(admitted) =
                exact_admission(sink, session, &delivery, &admission_key, retained_work).await?
            {
                transition_admission(
                    inbox,
                    address,
                    delivery_id,
                    ContinuationAdmissionTransition::Reserve {
                        session_id: session.clone(),
                        input_id: admitted.clone(),
                    },
                )
                .await?;
                return Ok(transition_admission(
                    inbox,
                    address,
                    delivery_id,
                    ContinuationAdmissionTransition::Apply {
                        session_id: session.clone(),
                        input_id: admitted,
                    },
                )
                .await?);
            }
            let input_id = InputId::new();
            transition_admission(
                inbox,
                address,
                delivery_id,
                ContinuationAdmissionTransition::Reserve {
                    session_id: session.clone(),
                    input_id: input_id.clone(),
                },
            )
            .await?;
            input_id
        }
    };
    observe_after_reservation(target, delivery_id, &input_id).await?;
    let input = continuation_input(&delivery, &admission_key, input_id.clone());
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
        let Some(exact) =
            exact_admission(sink, session, &delivery, &admission_key, retained_work).await?
        else {
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
    transition_admission(
        inbox,
        address,
        delivery_id,
        ContinuationAdmissionTransition::Apply {
            session_id: session.clone(),
            input_id: admitted,
        },
    )
    .await?;
    Ok(())
}

/// Reserve, then observe: the pass resolved `target`'s address before the
/// reservation `input_id` holds in its session, and it may have retired
/// since. A status read answers stranded only after it observed the
/// retirement and found no entry, so a drain that now finds the address
/// retired releases its reservation and admits nothing
/// ([`ContinuationAdmitError::AddressRetired`]), and one that reserved first
/// is seen by that read. Without an observer nothing is observed.
async fn observe_after_reservation(
    target: &AdmissionTarget<'_>,
    delivery_id: &RuntimeDeliveryId,
    input_id: &InputId,
) -> Result<(), ContinuationAdmitError> {
    let Some(observer) = target.observer else {
        return Ok(());
    };
    match observer.observe_address(target.address).await {
        Ok(AddressObservation::MayServe) => Ok(()),
        Ok(AddressObservation::Retired) => {
            transition_admission(
                target.inbox,
                target.address,
                delivery_id,
                ContinuationAdmissionTransition::Release {
                    session_id: target.session.clone(),
                    input_id: input_id.clone(),
                },
            )
            .await?;
            Err(ContinuationAdmitError::AddressRetired)
        }
        Err(error) => Err(ContinuationAdmitError::Failed(format!(
            "delivery address {} could not be observed after its reservation: {error}",
            target.address
        ))),
    }
}

/// The input `delivery` admits as `input_id`, under `admission_key`
/// ([`continuation_prompt`]).
fn continuation_input(
    delivery: &ContinuationDelivery,
    admission_key: &str,
    input_id: InputId,
) -> meerkat_runtime::Input {
    meerkat_runtime::Input::Prompt(continuation_prompt(delivery, admission_key, input_id))
}

/// Move the admission index entry of `delivery_id` by `transition`.
async fn transition_admission(
    inbox: &RuntimeDeliveryInbox,
    address: &LogicalRuntimeId,
    delivery_id: &RuntimeDeliveryId,
    transition: ContinuationAdmissionTransition,
) -> Result<(), String> {
    match inbox
        .transition_continuation_admission(address, delivery_id, transition.clone())
        .await
        .map_err(|error| error.to_string())?
    {
        ContinuationAdmissionOutcome::Transitioned(_) | ContinuationAdmissionOutcome::Released => {
            Ok(())
        }
        ContinuationAdmissionOutcome::Rejected { current } => Err(format!(
            "continuation admission index rejected {transition:?} over {current:?}"
        )),
    }
}

/// The exact admission `session_id` holds under `admission_key` for
/// `delivery`, if any. Any other row under the key is a conflict, never this
/// delivery.
async fn exact_admission(
    sink: &dyn ContinuationDeliverySink,
    session_id: &SessionId,
    delivery: &ContinuationDelivery,
    admission_key: &str,
    retained_work: Option<&meerkat_core::retained_work::RetainedWorkIdentity>,
) -> Result<Option<InputId>, String> {
    let expected = continuation_input(delivery, admission_key, InputId::new());
    let Some(row) = sink.admitted_input(session_id, admission_key).await? else {
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
    Ok(Some(row.state.input_id))
}

/// The admission `session` holds for the host original `delivery`, read as
/// its drain recovers it: by the reserved id `reserved_input` first (a
/// governed admission is indexed under its qualified key, so a raw-key read
/// misses it), then by the raw admission key (an ungoverned admission, made
/// before the runtime was governed or through the plain fallback). Either
/// row stands for this original only as its exact admission, verified with
/// the row's own id and recorded association
/// ([`verify_admitted_host_original`]); never by key alone.
///
/// # Errors
/// A read failed (a sink that cannot read by input id fails here, so the
/// reservation stays), or the row under the id or key is not this
/// original's admission.
async fn host_original_admission(
    sink: &dyn ContinuationDeliverySink,
    session: &SessionId,
    delivery: &ContinuationDelivery,
    admission_key: &str,
    reserved_input: &InputId,
) -> Result<Option<InputId>, String> {
    let stored = match sink.admitted_input_by_id(session, reserved_input).await? {
        Some(stored) => Some(stored),
        None => sink.admitted_input(session, admission_key).await?,
    };
    let Some(stored) = stored else {
        return Ok(None);
    };
    verify_admitted_host_original(
        &stored.state,
        delivery,
        admission_key,
        &stored.state.input_id,
    )
    .map_err(|error| error.to_string())?;
    Ok(Some(stored.state.input_id))
}

/// What settling a reservation on a retired address did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetiredReservation {
    /// The reserved session holds the admission: the index records it as
    /// applied, and the delivery reads admitted.
    Applied,
    /// The reserved session holds no admission: the reservation is
    /// released, and the delivery reads stranded.
    Released,
}

/// Settle the reservation `session` holds under `reserved_input` for
/// `record`, whose delivery address `address` retired, from that session's
/// input ledger. The caller holds the session's custody (it routes here, or
/// its cold claim is held), so no admission into the session is in flight
/// and the ledger's answer is final. The inbox row stays pending: a retired
/// address is never drained.
///
/// A host original ([`ContinuationOwnerService::submit_host_original`]) is
/// read as its drain recovers it ([`host_original_admission`]): by the
/// reserved id, then by the raw admission key. One corner reads stranded
/// although the session holds the original: native admission deduplicated a
/// governed retry onto an input another admission holds under the
/// original's qualified key, the attempt crashed before it applied that
/// input's id, and the address retired before a restarted drain bound the
/// original again (which would find the replay again and apply it). Only a
/// fresh bind derives that qualified key, and a retired address is never
/// bound again, so the reservation is released. The guard is the host's own
/// custody of the original: the admission that holds the existing input
/// promoted it (or the host settled it from the native record), so a host
/// that reads its custody does not resend the original under a new key.
/// Keeping the reservation instead would leave every governed reservation
/// whose input never reached the session pending forever.
///
/// # Errors
/// The session holds another input under the admission key, which is never
/// this delivery by key alone, so the reservation stays; or a read or a
/// write failed.
pub(crate) async fn settle_retired_reservation(
    inbox: &RuntimeDeliveryInbox,
    address: &LogicalRuntimeId,
    record: &meerkat_runtime::RuntimeDeliveryRecord,
    sink: &dyn ContinuationDeliverySink,
    session: &SessionId,
    reserved_input: &InputId,
) -> Result<RetiredReservation, String> {
    let delivery_id = record.submission.delivery_id();
    let row: ContinuationRow = serde_json::from_slice(record.submission.payload())
        .map_err(|error| format!("continuation delivery payload is invalid: {error}"))?;
    let admission_key = row.delivery.admission_key(delivery_id);
    let admitted = if row.host_original {
        host_original_admission(sink, session, &row.delivery, &admission_key, reserved_input)
            .await?
    } else {
        let retained_work = if sink.governs_work_authority() {
            row.retained_work.as_ref()
        } else {
            None
        };
        exact_admission(sink, session, &row.delivery, &admission_key, retained_work).await?
    };
    match admitted {
        Some(input_id) => {
            transition_admission(
                inbox,
                address,
                delivery_id,
                ContinuationAdmissionTransition::Apply {
                    session_id: session.clone(),
                    input_id,
                },
            )
            .await?;
            Ok(RetiredReservation::Applied)
        }
        None => {
            transition_admission(
                inbox,
                address,
                delivery_id,
                ContinuationAdmissionTransition::Release {
                    session_id: session.clone(),
                    input_id: reserved_input.clone(),
                },
            )
            .await?;
            Ok(RetiredReservation::Released)
        }
    }
}

/// The address a member owner's current incarnation binds to: `identity`
/// at `generation` in the incarnation `mob_incarnation` of mob `mob_id`
/// (`rt:mob-member:{mob}:{incarnation}:{identity}:{generation}`). A mob
/// destroyed and created again under the same id is a new incarnation, so
/// its members never serve the rows owed to the members of the old one.
pub fn member_delivery_address(
    mob_id: &str,
    mob_incarnation: MobIncarnationId,
    identity: &str,
    generation: u64,
) -> Result<LogicalRuntimeId, ContinuationSubmitError> {
    LogicalRuntimeId::for_member(&MemberDeliveryAddress {
        mob_id: mob_id.to_string(),
        incarnation: Some(mob_incarnation),
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
