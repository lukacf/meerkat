//! Mob member-host actor: the `rkat mob host` daemon's authority holder,
//! bridge responder, acceptor-registry driver, and R8 persistence driver
//! (multi-host mobs D6, §7.2 steps 1 & 3).
//!
//! The actor owns the generated [`MobHostBindingAuthority`] for the whole
//! daemon process (mob-keyed, A14) and serializes every mutation through one
//! tokio task — the `MobActor` single-owner discipline. The shell observes
//! typed envelope facts (sender match, address match, bootstrap-token
//! validity) and the machine adjudicates; no shell pre-check ever decides
//! admission (A11).
//!
//! Serving surface (DEC-P2-7 expired; phase 4 adds the trust pair):
//! `BindHost`, `RebindHost`, `MaterializeMember`, `ReleaseMember`,
//! `InstallPeerTrust`, `RemovePeerTrust`, and `HostStatus`. Everything
//! member-addressed (and the member-ORIGINATED operator upcall, which never
//! arrives host-addressed) keeps typed-rejecting
//! `BridgeRejectionCause::Unsupported` — the same fail-closed posture as the
//! member drain's `_ =>` arm. Peer-trust commands apply through the member
//! session's machine-gated direct-peer-endpoint seam (§10.4; the generated
//! reconciler is the only `apply_trust_mutation` caller — never a direct
//! TrustStore write). Materialization is served INLINE on the single
//! owner task (DEC-P3H-1): the controlling sender is single-flight per mob,
//! the member build never calls an LLM, and the owner task IS the
//! serialization — the machine's dedup rows stay the only idempotency memory.
//!
//! Durability (§14 R8, DEC-P2-6): each accepted binding persists as one
//! `runtime_mob_host_bindings` row (raw blob accessors on
//! [`meerkat_runtime::store::RuntimeStore`]); the typed record and the
//! transition-derived persistence witnesses live HERE. Phase 3 adds the
//! materialized/released member regions to the SAME row (one CAS blob per
//! mob, so the binding and member regions can never skew): the spec bytes
//! are the revival input (§15.7), written only under
//! `MaterializedMemberRecorded`/`MemberReleaseRecorded` transition
//! witnesses. In-memory authority state never advances past durable truth:
//! persist-before-commit, and a failed persist drops the prepared authority
//! and quiesces only the just-built volatile incarnation; its already-durable
//! session remains discoverable and resumable. Boot recovery folds the
//! rows into a [`MobHostBindingAuthorityState`], enters through the
//! generated `recover_from_state` seam (invariant-rejected state aborts
//! startup typed), then — when a member substrate is composed — revives
//! each recorded member from its stored spec with ZERO bridge traffic
//! (A20/§14.6); a per-member revival failure is typed + logged, the daemon
//! starts, and `HostStatus` reports that row `healthy: false`.
//!
//! Bootstrap token lifecycle (DEC-P2-4): random per issue, observed by the
//! machine as `token_valid` only, consumed on the first `HostBindAccepted`,
//! then re-minted with the descriptor rewritten — the daemon stays bindable
//! by the next mob without restart. A failed replacement publication remains
//! actor-owned pending work and is retried with capped backoff using that same
//! replacement token. Tokens are never persisted to the runtime store or sent
//! over the plaintext bridge; the 0600 descriptor file is the only disk
//! carrier and `BindHost` carries a request-bound HMAC.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use meerkat_contracts::wire::supervisor_bridge::{
    BridgeAck, BridgeBootstrapToken, BridgeCapabilities, BridgeCommand, BridgeCommandDecodeError,
    BridgeCreateForkedParticipantPayload, BridgeForkedParticipantCreatedResponse,
    BridgeForkedParticipantRevocationOutcome, BridgeForkedParticipantRevokedResponse,
    BridgeHostBindPayload, BridgeHostBindResponse, BridgeHostBindingDescriptorIssuedResponse,
    BridgeHostCapabilityRequirements, BridgeHostMemberRecord, BridgeHostRebindPayload,
    BridgeHostReboundResponse, BridgeHostRevokePayload, BridgeHostRevokedResponse,
    BridgeHostRuntimeIncarnation, BridgeHostStatusPayload, BridgeHostStatusResponse,
    BridgeIssueHostBindingDescriptorPayload, BridgeMaterializePayload, BridgeMaterializedResponse,
    BridgeMemberReleasedResponse, BridgePeerIdentity, BridgePeerSpec, BridgePeerTrustPayload,
    BridgeProtocolVersion, BridgeRejectionCause, BridgeReleasePayload, BridgeReply,
    BridgeRevokeForkedParticipantPayload, BridgeTurnOutcomeAck, BridgeTurnOutcomeRecord,
    MaterializeLaunchMode, MaterializeLaunchOutcome,
    MemberSessionDisposal as WireMemberSessionDisposal, RuntimeReleaseCause, WireFlowTurnOutcome,
    WireHostBindingDescriptor, WireHostBindingDescriptorKind, canonicalize_bridge_address,
    decode_bridge_command,
};
use meerkat_contracts::wire::{
    PortableMemberSpec, WireAuthBindingRef, portable_member_spec_digest,
};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, PeerAddress, PeerId, PeerRoute, SUPERVISOR_BRIDGE_INTENT, SendError,
    TrustedPeerDescriptor,
};
use meerkat_core::interaction::{InteractionContent, PeerIngressFact, PeerInputCandidate};

use crate::forked_participant::{
    ForkedParticipantAttachmentAssociation, ForkedParticipantAttachmentId,
    ForkedParticipantCapabilityId, ForkedParticipantError, ForkedParticipantOperationScope,
    ForkedParticipantOwnerRoute, ForkedParticipantRequest, ForkedParticipantRequestId,
    ForkedParticipantResumeProof, ForkedParticipantResumeRejection, ForkedParticipantReusePolicy,
    ForkedParticipantService, adjudicate_protected_resume, bridge_ref, domain_ref,
};
use crate::machines::mob_host_binding_authority::{
    AgentIdentity as AuthorityAgentIdentity, FenceToken as AuthorityFenceToken,
    FlowTurnOutcomeKind, Generation as AuthorityGeneration, HostAdmissionRejectKind,
    HostBindingPhase, InputId as AuthorityInputId, MaterializeRejectKind, MemberKey,
    MemberSessionDisposal as MachineMemberSessionDisposal, MobHostBindingAuthorityAuthority,
    MobHostBindingAuthorityEffect, MobHostBindingAuthorityInput, MobHostBindingAuthorityMutator,
    MobHostBindingAuthorityState, MobHostBindingAuthorityTransition, MobId as AuthorityMobId,
    PeerId as AuthorityPeerId, PeerSigningKey as AuthorityPeerSigningKey,
    SessionId as AuthoritySessionId, TrackedInputCancelKind, TurnKey,
};
use crate::runtime::bridge_protocol::derive_host_bind_bootstrap_proof;
use crate::runtime::host_materialize::{
    DecompiledMemberBuild, HostMemberMaterializer, LiveMemberRuntime, MaterializeDecompileError,
    MaterializeServingContext, RevivedMemberOutcome, assemble_preflight_observations,
    decompile_portable_spec, validate_portable_spec_structure,
};
use crate::runtime::host_observation::{
    HostObservationProjection, HostPendingReservationReply, HostTrackedInputCancelReply,
    HostTrackedTurnJournal, HostTurnOutcomeAckRequest, HostTurnOutcomePendingRequest,
    HostTurnOutcomeRecordRequest, PendingTurnObservation, SessionObservationFacts,
};
use crate::runtime::host_reply::HostBridgeReply;

use meerkat_runtime::SessionServiceRuntimeExt as _;
use meerkat_runtime::meerkat_machine::PeerEndpointStageError;

// Composition-facing re-export: the substrate bundle is named alongside
// `MobHostActorConfig` by every composing binary/fixture.
pub use crate::runtime::host_materialize::HostMemberSubstrate;

use meerkat_runtime::meerkat_machine::dsl as mm_dsl;

async fn commit_revived_member_publication(
    outcome: &mut RevivedMemberOutcome,
    incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    tracked_turn_journal: Option<Arc<dyn meerkat_runtime::member_observation::TrackedTurnJournal>>,
) -> Result<Option<meerkat_runtime::meerkat_machine::MemberResidencyPublication>, crate::MobError> {
    match (
        outcome.residency_update.take(),
        outcome.runtime_publication.take(),
    ) {
        (Some(update), Some(publication)) => publication
            .commit_serving_with_residency(update, incarnation, tracked_turn_journal)
            .await
            .map(|(_, residency)| Some(residency)),
        (Some(_update), None) => Err(crate::MobError::Internal(
            "revived runtime residency repair lacked an exact attachment publication lease"
                .to_string(),
        )),
        (None, None) => Ok(None),
        (None, Some(publication)) => {
            let cleanup = publication.abort().await;
            Err(crate::MobError::Internal(match cleanup {
                Ok(()) => "revived runtime had an attachment publication lease without a residency transaction"
                    .to_string(),
                Err(cleanup_error) => format!(
                    "revived runtime had an attachment publication lease without a residency transaction; exact cleanup also failed: {cleanup_error}"
                ),
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Typed faults of the mob host actor and its persistence/serving seams.
#[derive(Debug, thiserror::Error)]
pub enum MobHostActorError {
    /// Boot recovery was rejected by the generated invariant validation —
    /// the daemon must not start half-recovered.
    #[error("host binding authority recovery rejected: {detail}")]
    Recovery { detail: String },
    /// Raw runtime-store row access failed.
    #[error("host binding persistence failed: {0}")]
    Persistence(#[from] meerkat_runtime::store::RuntimeStoreError),
    /// Record (de)serialization failed.
    #[error("host binding record serde failed: {detail}")]
    RecordSerde { detail: String },
    /// A persistence write was attempted without the matching generated
    /// transition witness, or the witness did not match the record.
    #[error("host binding persistence witness rejected: {detail}")]
    Witness { detail: String },
    /// The durable row set diverged from the machine's recorded binding.
    #[error("durable host binding rows diverged from authority state: {detail}")]
    StoreDiverged { detail: String },
    /// A decoded materialized-member row disagrees with its durable outer
    /// key or with identity/integrity facts recorded inside the row. Recovery
    /// and replay both fail closed through this one typed classification.
    #[error(
        "durable materialized-member row for mob '{mob_id}' identity '{agent_identity}' is corrupt: {detail}"
    )]
    DurableMaterializedRowCorrupt {
        mob_id: String,
        agent_identity: String,
        detail: String,
    },
    /// A member-region write may have committed, but exact durable truth (or
    /// the matching prepared authority commit) could not be proven. The live
    /// host must stop serving mutations until cold recovery rebuilds both
    /// authorities from the durable row.
    #[error("durable host binding outcome is uncertain: {detail}")]
    DurableUncertainty { detail: String },
    /// The generated authority refused an input (no matching transition) or
    /// a prepared commit.
    #[error("host binding authority transition failed: {detail}")]
    Machine { detail: String },
    /// Acceptor identity registry mutation failed.
    #[error(transparent)]
    Registry(#[from] meerkat_comms::HostAcceptorError),
    /// Host comms runtime composition or trust seam failed.
    #[error("host comms runtime fault: {detail}")]
    Comms { detail: String },
    /// Descriptor publication (file sink or pairing watch) failed.
    #[error("host binding descriptor publication failed: {detail}")]
    Descriptor { detail: String },
    /// Tier-1 provider presence probe failed (credential backend fault).
    #[error(transparent)]
    Probe(#[from] ProviderPresenceProbeError),
    /// Internal invariant violation.
    #[error("mob host actor internal fault: {detail}")]
    Internal { detail: String },
    /// The host participant name could not be published because a *different*
    /// live public key already holds that route.
    ///
    /// Re-carries [`meerkat_comms::RegistrationRejection::NameOccupied`]'s
    /// `holder_pubkey` verbatim rather than flattening it into
    /// [`Self::Comms`]'s prose. Construct only from
    /// `crate::error::comms_name_occupancy_holder`.
    #[error(
        "{}",
        crate::error::participant_name_occupied_message(participant_name, holder_pubkey)
    )]
    ParticipantNameOccupied {
        participant_name: String,
        holder_pubkey: meerkat_comms::PubKey,
    },
}

impl MobHostActorError {
    fn is_durable_uncertainty(&self) -> bool {
        matches!(self, Self::DurableUncertainty { .. })
    }
}

// ---------------------------------------------------------------------------
// Bootstrap token slot (DEC-P2-4)
// ---------------------------------------------------------------------------

/// One-time bind bootstrap token slot — daemon-shell ceremony material.
///
/// Random per issue (never keypair-derived), consumed on `HostBindAccepted`,
/// re-minted immediately so the next mob can bind without a daemon restart.
/// The machine only ever sees the boolean `token_valid` observation. Restart
/// re-mints, so any pre-restart descriptor is invalid — strictly tighter
/// than §20.3's contract.
pub struct HostBootstrapTokenSlot {
    current: String,
}

impl HostBootstrapTokenSlot {
    /// Mint a fresh slot with a random one-time token.
    pub fn mint() -> Self {
        Self {
            current: mint_bootstrap_token(),
        }
    }

    /// Verify the request-bound proof against the current unconsumed token.
    ///
    /// The raw token remains in the out-of-band descriptor; only the HMAC is
    /// exposed on the signed-but-unencrypted bridge transport. Comparison is
    /// constant-time because a valid proof grants the fresh bind authority.
    pub fn matches_bind_proof(&self, payload: &BridgeHostBindPayload) -> bool {
        if self.current.is_empty() {
            return false;
        }
        let expected = derive_host_bind_bootstrap_proof(self.current.as_str(), payload);
        meerkat_comms::constant_time_str_eq(expected.as_str(), payload.bootstrap_proof.as_str())
    }

    /// The current one-time token (descriptor material only — never logged,
    /// never persisted to the runtime store).
    pub fn current(&self) -> &str {
        &self.current
    }

    /// Consume the current token and mint its replacement.
    pub fn consume_and_remint(&mut self) {
        self.current = mint_bootstrap_token();
    }
}

impl std::fmt::Debug for HostBootstrapTokenSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "HostBootstrapTokenSlot(<redacted, {}B>)",
            self.current.len()
        )
    }
}

fn mint_bootstrap_token() -> String {
    // 256 bits of OS randomness via two v4 UUIDs; hex-rendered so the token
    // survives every string carrier untouched.
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

// ---------------------------------------------------------------------------
// Tier-1 provider presence probe (R7, §14.5 — injected, DEC-P2-5)
// ---------------------------------------------------------------------------

/// Typed failure of the tier-1 provider presence probe.
#[derive(Debug, thiserror::Error)]
pub enum ProviderPresenceProbeError {
    /// The credential backend (token store) could not be opened — provider
    /// resolution with a faulted backend fails closed.
    #[error("provider presence probe credential backend unavailable: {detail}")]
    CredentialBackend { detail: String },
    /// The exact materialization identity could not be formed for probing.
    #[error("materialization preflight input is invalid: {detail}")]
    PreflightInput { detail: String },
}

/// Presence-level provider resolvability probe (gotcha 13: presence only —
/// zero network, zero OAuth, zero provider endpoints).
///
/// Implemented above this crate (the composing binary owns the effective
/// config chain and token store) and injected into the actor as a trait
/// object. The reported set is an admission HINT, never the gate (R7).
#[async_trait]
pub trait ProviderPresenceProbe: Send + Sync {
    async fn resolvable_providers(
        &self,
    ) -> Result<Vec<meerkat_core::Provider>, ProviderPresenceProbeError>;
}

// ---------------------------------------------------------------------------
// Capabilities composer (§14.5, R7)
// ---------------------------------------------------------------------------

/// Feature-compiled facts of the composing binary (§15 R4), passed in at
/// daemon composition.
#[derive(Debug, Clone, Copy)]
pub struct HostCapabilityFacts {
    /// The opened realm persistence backend provides durable session state (A7).
    pub durable_sessions: bool,
    /// A semantic memory store is compiled in and composed.
    pub memory_store: bool,
    /// Declarative MCP servers are compiled in.
    pub mcp: bool,
}

/// Builds the `BridgeCapabilities` for bind/rebind replies. Recomputed per
/// ceremony (cheap); staleness between ceremonies is declared by design —
/// the `HostStatus` refresh path is phase 3 (DEC-P2-7).
pub struct HostCapabilitiesComposer {
    probe: Arc<dyn ProviderPresenceProbe>,
    facts: HostCapabilityFacts,
}

impl HostCapabilitiesComposer {
    pub fn new(probe: Arc<dyn ProviderPresenceProbe>, facts: HostCapabilityFacts) -> Self {
        Self { probe, facts }
    }

    pub async fn compose(&self) -> Result<BridgeCapabilities, ProviderPresenceProbeError> {
        let resolvable_providers = self.probe.resolvable_providers().await?;
        Ok(BridgeCapabilities {
            // Member verbs mirror the member runtime defaults; hard cancel
            // flipped in phase 6 (DEC-P6E-7: the member drain serves the
            // machine-admitted hard-cancel arm).
            deliver_member_input: true,
            observe_member: true,
            interrupt_member: true,
            hard_cancel_member: true,
            tracked_input_cancel: true,
            retire_member: true,
            destroy_member: true,
            wire_member: true,
            unwire_member: true,
            durable_sessions: self.facts.durable_sessions,
            // The daemon runs member loops (D6).
            autonomous_members: true,
            memory_store: self.facts.memory_store,
            mcp: self.facts.mcp,
            // One owner for the engine version fact: the contracts crate.
            engine_version: meerkat_contracts::ContractVersion::CURRENT.to_string(),
            // Approval forwarding lands with member-originated upcalls
            // (phase 3+).
            approval_forwarding: false,
            resolvable_providers,
            ..BridgeCapabilities::default()
        })
    }
}

// ---------------------------------------------------------------------------
// Descriptor refresher (§7.2 step 1 + DEC-P2-4)
// ---------------------------------------------------------------------------

/// Where the serialized host binding descriptor lands (the daemon's 0600
/// file writer). Implemented by the composing binary.
///
/// Contract: `Err` means the supplied descriptor was not made visible. A
/// sink that replaces an existing public artifact must prepare privately and
/// commit atomically.
pub trait HostDescriptorSink: Send + Sync {
    fn publish(&self, descriptor_json: &str) -> Result<(), String>;
}

/// Rewrites the descriptor (file sink + pairing watch) whenever the
/// bootstrap token rotates. The identity/address/live-endpoint template is
/// fixed for the daemon's lifetime; only the token varies.
pub struct DescriptorRefresher {
    address: String,
    identity: meerkat_contracts::WireTrustedPeerIdentity,
    live_endpoint: Option<String>,
    watch_tx: watch::Sender<String>,
    sink: Arc<dyn HostDescriptorSink>,
}

impl DescriptorRefresher {
    pub fn new(
        address: String,
        identity: meerkat_contracts::WireTrustedPeerIdentity,
        live_endpoint: Option<String>,
        watch_tx: watch::Sender<String>,
        sink: Arc<dyn HostDescriptorSink>,
    ) -> Self {
        Self {
            address,
            identity,
            live_endpoint,
            watch_tx,
            sink,
        }
    }

    pub fn live_endpoint(&self) -> Option<&String> {
        self.live_endpoint.as_ref()
    }

    /// Serialize the descriptor with `token` without exposing it.
    ///
    /// Startup uses this prepare half before installing the acceptor-registry
    /// owner. That keeps serialization failure on the reversible side of the
    /// startup transaction.
    fn prepare(&self, token: &str) -> Result<String, MobHostActorError> {
        let descriptor = self.descriptor(token);
        serde_json::to_string_pretty(&descriptor).map_err(|err| MobHostActorError::RecordSerde {
            detail: format!("descriptor serialization failed: {err}"),
        })
    }

    fn descriptor(&self, token: &str) -> WireHostBindingDescriptor {
        WireHostBindingDescriptor {
            kind: WireHostBindingDescriptorKind::Host,
            address: self.address.clone(),
            identity: self.identity.clone(),
            bootstrap_token: BridgeBootstrapToken::new(token),
            live_endpoint: self.live_endpoint.clone(),
        }
    }

    /// Publish already-prepared descriptor JSON: durable file sink first,
    /// then the pairing watch slot.
    fn publish_prepared(&self, json: String) -> Result<(), MobHostActorError> {
        self.sink
            .publish(&json)
            .map_err(|detail| MobHostActorError::Descriptor { detail })?;
        // watch::Sender::send_replace never fails; pairing consumers read
        // the latest slot on their next PAIRING_COMPLETE.
        self.watch_tx.send_replace(json);
        Ok(())
    }

    /// Serialize the descriptor with `token` and publish it: durable file
    /// sink first, then the pairing watch slot.
    pub fn publish(&self, token: &str) -> Result<(), MobHostActorError> {
        self.publish_prepared(self.prepare(token)?)
    }
}

const HOST_DESCRIPTOR_REFRESH_INITIAL_RETRY_DELAY: Duration = Duration::from_millis(250);
const HOST_DESCRIPTOR_REFRESH_MAX_RETRY_DELAY: Duration = Duration::from_secs(5);

/// Actor-owned availability work created only after a freshly re-minted
/// bootstrap token could not be published. The token itself remains solely
/// in [`HostBootstrapTokenSlot`]; this state controls one publish attempt per
/// due timer tick and retains the obligation until publication succeeds or
/// the actor shuts down.
#[derive(Debug, Clone, Copy)]
struct PendingDescriptorRefresh {
    failed_attempts: u64,
    retry_delay: Duration,
    retry_at: Instant,
}

impl PendingDescriptorRefresh {
    fn after_publish_failure() -> Self {
        Self {
            failed_attempts: 0,
            retry_delay: HOST_DESCRIPTOR_REFRESH_INITIAL_RETRY_DELAY,
            retry_at: Instant::now() + HOST_DESCRIPTOR_REFRESH_INITIAL_RETRY_DELAY,
        }
    }

    fn is_due(&self) -> bool {
        Instant::now() >= self.retry_at
    }

    fn retry_at(&self) -> Instant {
        self.retry_at
    }

    fn attempt_number(&self) -> u64 {
        self.failed_attempts.saturating_add(1)
    }

    fn should_log_failure(&self) -> bool {
        self.failed_attempts == 1 || self.failed_attempts.is_power_of_two()
    }

    /// Retry publication of the slot's current token without consuming or
    /// replacing it. Failure schedules exactly one later attempt with capped
    /// exponential backoff; success lets the actor clear this pending state.
    fn retry(
        &mut self,
        descriptor: &DescriptorRefresher,
        bootstrap_token: &HostBootstrapTokenSlot,
    ) -> Result<(), MobHostActorError> {
        match descriptor.publish(bootstrap_token.current()) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.failed_attempts = self.failed_attempts.saturating_add(1);
                self.retry_delay =
                    (self.retry_delay * 2).min(HOST_DESCRIPTOR_REFRESH_MAX_RETRY_DELAY);
                self.retry_at = Instant::now() + self.retry_delay;
                Err(error)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// R8 typed record + transition-derived persistence witnesses (DEC-P2-6)
// ---------------------------------------------------------------------------

/// Typed content of one `runtime_mob_host_bindings` row (keyed by mob id at
/// the store layer). Phase 3 adds the materialized/released member regions
/// as `#[serde(default)]` fields (the turn-outcome journal region joins in
/// phase 6, §18 O2). Carries NO bootstrap token (never persisted) and no mob
/// definition/roster/profile/wiring (R8's second-roster prohibition) — the
/// per-member spec is BUILD material sanctioned by §15.7.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MobHostBindingRecord {
    pub supervisor_peer_id: String,
    pub supervisor_signing_key: [u8; 32],
    pub epoch: u64,
    /// Durable incarnation of this controller-to-host binding. Legacy rows
    /// deserialize as generation zero; every newly negotiated binding uses a
    /// strictly positive generation.
    #[serde(default)]
    pub binding_generation: u64,
    /// Exact capability snapshot accepted with the binding authority tuple.
    /// Reply-loss replay returns this snapshot instead of re-adjudicating an
    /// already-committed ceremony against a later runtime downgrade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_capabilities: Option<BridgeCapabilities>,
    /// Materialized member rows keyed by agent identity (§15.7 spec bytes +
    /// recorded-ack material; written only under
    /// `MaterializedMemberRecorded` witnesses).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub materialized: BTreeMap<String, MaterializedMemberRow>,
    /// Release dedup rows keyed by agent identity (written only under
    /// `MemberReleaseRecorded` witnesses).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub released: BTreeMap<String, ReleasedMemberRow>,
    /// §18 O2 turn-outcome journal rows keyed by agent identity (written
    /// only under `TurnOutcomeRecorded` witnesses; pruned with the release
    /// row-move for the released generation). The coarse dedup facts live
    /// in machine state; the row additionally retains the full wire
    /// outcome verbatim — recorded ONCE at classification time by the one
    /// shared classifier — as sidecar presentation material.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub turn_outcomes: BTreeMap<String, Vec<TurnOutcomeRow>>,
    /// Lifecycle-bounded exact ACK tombstones. Payload rows leave
    /// `turn_outcomes` after controller consumption, but their key remains
    /// until this member residency is released/superseded/revoked so an
    /// arbitrarily delayed delivery cannot recreate the effect. Tombstones
    /// do not consume the 256 live Pending+terminal quota. This region is an
    /// intentional lifecycle ledger, not a cache: one compact ACK row is
    /// roughly 100-120 bytes of compact JSON (UUID + generation + fence),
    /// owned by the `runtime_mob_host_bindings` blob until residency disposal.
    /// Accepted keys already retain a much larger exact runtime input-ledger
    /// row; stop-before-send `NoEffect` keys are the only net-new class. A
    /// future bounded protocol would need a controller-issued cumulative
    /// delivery watermark. Local eviction is forbidden because it would make
    /// delayed duplicate execution legal again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub turn_outcome_acknowledged: BTreeMap<String, Vec<TurnOutcomeAcknowledgedRow>>,
    /// Lifecycle-bounded durable cancellation receipts. `Cancelling` is a
    /// controlling intermediate that blocks redelivery before runtime
    /// quiescence; terminal receipts replay `NoEffect`/`Cancelled` exactly.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tracked_input_cancellations: BTreeMap<String, Vec<TrackedInputCancellationRow>>,
    /// Durable pre-accept capacity reservations. Each row pins the original
    /// durable event window so a retry/restart can reattach without executing
    /// the input a second time. A terminal CAS moves one exact row into
    /// `turn_outcomes`; release/rematerialization/revoke prune both regions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub turn_outcome_pending: BTreeMap<String, Vec<TurnOutcomePendingRow>>,
    /// Capability attachments this host admitted but could not prove either
    /// associated or released (issue #159 phase 3 slice B).
    ///
    /// Keyed by [`ForkedParticipantAttachmentAssociation::association_key`].
    /// An entry is an explicit, durable reconciliation OBLIGATION, not a
    /// lifecycle claim: the host admitted an attach, then hit a build/record
    /// outcome that either may have left member side effects behind or left
    /// the release itself unproven. Blind release would be a lifecycle lie in
    /// exactly the ambiguous case, so the obligation is retained and
    /// reconciled conservatively at boot and at revoke instead. Entries are
    /// discharged atomically by the materialized-row CAS that adopts the same
    /// association, by a proven later release, or by host revocation.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub forked_participant_obligations: BTreeMap<String, ForkedParticipantAttachmentObligation>,
}

/// Why one capability attachment is still owed reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkedParticipantObligationCause {
    /// The member build failed in a class that may have left side effects, so
    /// releasing would assert an absence this host cannot observe.
    AmbiguousBuild,
    /// The build failed definitively, but the compensating release itself did
    /// not succeed.
    ReleaseUnproven,
    /// The durable materialized-row write did not commit provably, so neither
    /// association nor release may be asserted.
    RecordUncertain,
}

/// One durable, un-forgettable capability-attachment reconciliation
/// obligation. Mechanical evidence only — see
/// [`MobHostBindingRecord::forked_participant_obligations`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForkedParticipantAttachmentObligation {
    /// Member identity the attachment was admitted for.
    pub agent_identity: String,
    /// The exact admitted association.
    pub association: ForkedParticipantAttachmentAssociation,
    /// Why reconciliation is still owed.
    pub cause: ForkedParticipantObligationCause,
    /// Operator-facing detail of the originating failure. Diagnostics only.
    pub detail: String,
}

/// Durable terminal receipt for one completed host revocation.
///
/// Stored in `runtime_mob_host_revocations`, never in the active binding
/// table. It carries only the authenticated supervisor tuple and the exact
/// success payload needed for reply-loss replay — no member spec/session row
/// survives here, so boot recovery cannot revive revoked residency.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MobHostRevocationReceipt {
    pub supervisor_peer_id: String,
    pub supervisor_signing_key: [u8; 32],
    pub epoch: u64,
    #[serde(default)]
    pub binding_generation: u64,
    pub released_members: Vec<String>,
}

/// One durable turn-outcome journal row (§18 O2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TurnOutcomeRow {
    pub input_id: String,
    pub generation: u64,
    pub fence_token: u64,
    /// Durable `StoredEvent.seq` of the turn's terminal event (gotcha 8).
    pub terminal_seq: u64,
    pub outcome: WireFlowTurnOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounded_result: Option<super::bridge_protocol::BridgeBoundedTurnResult>,
}

/// Compact durable proof that the controller consumed one exact terminal.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TurnOutcomeAcknowledgedRow {
    pub input_id: String,
    pub generation: u64,
    pub fence_token: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedTrackedInputCancelKind {
    NoEffect,
    Cancelling,
    Cancelled,
}

impl RecordedTrackedInputCancelKind {
    fn to_machine(self) -> TrackedInputCancelKind {
        match self {
            Self::NoEffect => TrackedInputCancelKind::NoEffect,
            Self::Cancelling => TrackedInputCancelKind::Cancelling,
            Self::Cancelled => TrackedInputCancelKind::Cancelled,
        }
    }
}

/// Durable cancellation receipt for one exact tracked delivery key.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrackedInputCancellationRow {
    pub input_id: String,
    pub generation: u64,
    pub fence_token: u64,
    pub outcome: RecordedTrackedInputCancelKind,
}

/// One durable Pending reservation for a directed turn. The exact
/// `(generation, fence_token, input_id)` tuple is the ownership key;
/// `window_start` is the first durable event seq that may belong to the turn.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TurnOutcomePendingRow {
    pub input_id: String,
    pub generation: u64,
    pub fence_token: u64,
    pub window_start: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounded_result_spec: Option<super::bridge_protocol::BridgeBoundedResultSpec>,
}

impl TurnOutcomeRow {
    /// Project onto the wire sidecar record (verbatim fields).
    pub fn to_wire(&self) -> BridgeTurnOutcomeRecord {
        BridgeTurnOutcomeRecord {
            input_id: self.input_id.clone(),
            generation: self.generation,
            fence_token: self.fence_token,
            terminal_seq: self.terminal_seq,
            outcome: self.outcome.clone(),
            bounded_result: self.bounded_result.clone(),
        }
    }

    /// The coarse machine journal kind this wire outcome folds to (the
    /// `FlowTurnOutcomeKind` vocabulary is the frozen catalog's; the wire
    /// enum is the presentation superset).
    pub fn machine_kind(&self) -> FlowTurnOutcomeKind {
        flow_turn_outcome_kind_from_wire(&self.outcome)
    }
}

/// Total map: wire terminal → coarse machine journal kind.
pub(crate) fn flow_turn_outcome_kind_from_wire(
    outcome: &WireFlowTurnOutcome,
) -> FlowTurnOutcomeKind {
    match outcome {
        WireFlowTurnOutcome::RunCompleted
        | WireFlowTurnOutcome::ExtractionSucceeded
        | WireFlowTurnOutcome::InteractionComplete => FlowTurnOutcomeKind::Completed,
        WireFlowTurnOutcome::ExtractionFailed { .. }
        | WireFlowTurnOutcome::RunFailed { .. }
        | WireFlowTurnOutcome::InteractionCallbackPending
        | WireFlowTurnOutcome::InteractionFailed { .. } => FlowTurnOutcomeKind::Failed,
        // Stream closed / cancelled without a classified terminal.
        WireFlowTurnOutcome::ChannelClosed => FlowTurnOutcomeKind::Canceled,
    }
}

/// One durable materialized-member row: the machine dedup tuple, the spec
/// bytes (§15.7 — the A20 revival input, digest-checked on read), and the
/// recorded-ack material `MaterializeReplay` returns verbatim.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MaterializedMemberRow {
    pub generation: u64,
    /// First durable event seq belonging to `generation`. Resume may reuse a
    /// session/event log, so generation changes do not imply seq reset to 1.
    #[serde(default = "default_generation_start_seq")]
    pub generation_start_seq: u64,
    pub fence_token: u64,
    pub session_id: String,
    pub spec_digest: String,
    pub spec: PortableMemberSpec,
    /// §15.4: the engine version that performed this build (also the replay
    /// ack's `engine_version`, verbatim).
    pub engine_version_at_build: String,
    pub member_pubkey: String,
    pub member_peer_id: String,
    pub launch_outcome: MaterializeLaunchOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_auth_binding: Option<WireAuthBindingRef>,
    /// Materialize-time supervisor endpoint material: boot revival re-seeds
    /// the member-side supervisor authority from the recovered record with
    /// zero bridge traffic (A20), so the endpoint must survive here.
    pub supervisor_name: String,
    pub supervisor_address: String,
    /// Mechanical routing evidence for the capability attachment this
    /// residency was materialized under (issue #159 phase 3 slice B).
    ///
    /// `None` is the ordinary case AND the shape a released/legacy row
    /// deserializes to: the field is additive, never required. It is NOT
    /// lifecycle truth — the source-owner service and its canonical lifecycle
    /// machine remain the only authority over attachment state. The row keeps
    /// it so host teardown (release, revoke, supersession) can route back to
    /// the exact attachment it admitted instead of guessing or forgetting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_participant_attachment: Option<ForkedParticipantAttachmentAssociation>,
}

const fn default_generation_start_seq() -> u64 {
    1
}

fn merge_host_capability_requirements(
    left: BridgeHostCapabilityRequirements,
    right: BridgeHostCapabilityRequirements,
) -> BridgeHostCapabilityRequirements {
    BridgeHostCapabilityRequirements {
        durable_sessions: left.durable_sessions || right.durable_sessions,
        autonomous_members: left.autonomous_members || right.autonomous_members,
        tracked_input_cancel: left.tracked_input_cancel || right.tracked_input_cancel,
        protocol_v4: left.protocol_v4 || right.protocol_v4,
    }
}

fn retained_host_capability_requirements(
    record: Option<&MobHostBindingRecord>,
) -> BridgeHostCapabilityRequirements {
    let Some(record) = record else {
        return BridgeHostCapabilityRequirements::default();
    };
    let autonomous_members = record.materialized.values().any(|row| {
        row.spec.profile.runtime_mode == meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost
    });
    let tracked_custody = record.turn_outcomes.values().any(|rows| !rows.is_empty())
        || record
            .turn_outcome_acknowledged
            .values()
            .any(|rows| !rows.is_empty())
        || record
            .turn_outcome_pending
            .values()
            .any(|rows| !rows.is_empty())
        || record
            .tracked_input_cancellations
            .values()
            .any(|rows| !rows.is_empty());
    let tracked_contract = tracked_custody || autonomous_members;
    BridgeHostCapabilityRequirements {
        durable_sessions: tracked_contract,
        autonomous_members,
        tracked_input_cancel: tracked_contract,
        protocol_v4: tracked_contract,
    }
}

fn missing_host_capabilities(
    required: BridgeHostCapabilityRequirements,
    capabilities: &BridgeCapabilities,
) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if required.durable_sessions && !capabilities.durable_sessions {
        missing.push("durable_sessions");
    }
    if required.autonomous_members && !capabilities.autonomous_members {
        missing.push("autonomous_members");
    }
    if required.tracked_input_cancel && !capabilities.tracked_input_cancel {
        missing.push("tracked_input_cancel");
    }
    if required.protocol_v4
        && !capabilities
            .supported_protocol_versions
            .contains(&BridgeProtocolVersion::V4)
    {
        missing.push("protocol_v4");
    }
    missing
}

fn exact_binding_capability_snapshot(
    record: Option<&MobHostBindingRecord>,
    supervisor: &BridgePeerIdentity,
    epoch: u64,
    binding_generation: u64,
) -> Option<BridgeCapabilities> {
    let record = record?;
    (record.supervisor_peer_id == supervisor.peer_id.as_str()
        && record.supervisor_signing_key == *supervisor.pubkey.as_bytes()
        && record.epoch == epoch
        && record.binding_generation == binding_generation)
        .then(|| record.accepted_capabilities.clone())
        .flatten()
}

/// One durable release-dedup row (the recorded disposal `ReleaseReplay`
/// returns, plus the pubkey whose acceptor identity replay re-deregisters).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReleasedMemberRow {
    pub generation: u64,
    pub fence_token: u64,
    pub disposal: RecordedDisposal,
    pub member_pubkey: String,
}

/// Serde mirror of the machine's [`MachineMemberSessionDisposal`] (the
/// generated enum carries no serde derives). Total in both directions —
/// DEC-P3H-6: the wire `AlreadyArchived` folds into `Archived` for the
/// machine/durable fact, so it is never replayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedDisposal {
    Archived,
    RuntimeReleasedOnlyHostOwned,
    RuntimeReleasedOnlyNoDurableSessions,
}

impl RecordedDisposal {
    pub fn from_machine(disposal: MachineMemberSessionDisposal) -> Self {
        match disposal {
            MachineMemberSessionDisposal::Archived => Self::Archived,
            MachineMemberSessionDisposal::RuntimeReleasedOnlyHostOwned => {
                Self::RuntimeReleasedOnlyHostOwned
            }
            MachineMemberSessionDisposal::RuntimeReleasedOnlyNoDurableSessions => {
                Self::RuntimeReleasedOnlyNoDurableSessions
            }
        }
    }

    pub fn to_machine(self) -> MachineMemberSessionDisposal {
        match self {
            Self::Archived => MachineMemberSessionDisposal::Archived,
            Self::RuntimeReleasedOnlyHostOwned => {
                MachineMemberSessionDisposal::RuntimeReleasedOnlyHostOwned
            }
            Self::RuntimeReleasedOnlyNoDurableSessions => {
                MachineMemberSessionDisposal::RuntimeReleasedOnlyNoDurableSessions
            }
        }
    }

    /// The wire projection of a RECORDED disposal (replay path): the
    /// success-class `AlreadyArchived` was folded to `Archived` at record
    /// time, so it never reappears here (DEC-P3H-6).
    pub fn to_wire(self) -> WireMemberSessionDisposal {
        match self {
            Self::Archived => WireMemberSessionDisposal::Archived,
            Self::RuntimeReleasedOnlyHostOwned => WireMemberSessionDisposal::RuntimeReleasedOnly {
                cause: RuntimeReleaseCause::HostOwnedSession,
            },
            Self::RuntimeReleasedOnlyNoDurableSessions => {
                WireMemberSessionDisposal::RuntimeReleasedOnly {
                    cause: RuntimeReleaseCause::NoDurableSessions,
                }
            }
        }
    }
}

/// Witness that a `MobHostBindingAuthority` transition accepted the binding
/// this record describes — the only way to persist a host-binding row.
/// Constructible solely from a transition whose effects contain the
/// matching `HostBindAccepted`/`HostRebindAccepted` for the mob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostBindingPersistenceAuthority {
    mob_id: AuthorityMobId,
    supervisor_peer_id: AuthorityPeerId,
    epoch: u64,
    binding_generation: u64,
    accepted_capabilities: Option<BridgeCapabilities>,
}

impl HostBindingPersistenceAuthority {
    pub fn from_transition(
        mob_id: &AuthorityMobId,
        transition: &MobHostBindingAuthorityTransition,
    ) -> Result<Self, MobHostActorError> {
        let accepted = transition.effects().iter().find_map(|effect| match effect {
            MobHostBindingAuthorityEffect::HostBindAccepted {
                mob_id: effect_mob,
                supervisor_peer_id,
                epoch,
                binding_generation,
            }
            | MobHostBindingAuthorityEffect::HostRebindAccepted {
                mob_id: effect_mob,
                supervisor_peer_id,
                epoch,
                binding_generation,
            } if effect_mob == mob_id => {
                Some((supervisor_peer_id.clone(), *epoch, *binding_generation))
            }
            _ => None,
        });
        let Some((supervisor_peer_id, epoch, binding_generation)) = accepted else {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "transition carries no host bind/rebind accept for mob '{}'",
                    mob_id.0
                ),
            });
        };
        Ok(Self {
            mob_id: mob_id.clone(),
            supervisor_peer_id,
            epoch,
            binding_generation,
            accepted_capabilities: None,
        })
    }

    fn with_accepted_capabilities(
        mut self,
        accepted_capabilities: Option<BridgeCapabilities>,
    ) -> Self {
        self.accepted_capabilities = accepted_capabilities;
        self
    }

    pub fn verify_record(
        &self,
        mob_id: &str,
        record: &MobHostBindingRecord,
    ) -> Result<(), MobHostActorError> {
        if self.mob_id.0 != mob_id
            || self.supervisor_peer_id.0 != record.supervisor_peer_id
            || self.epoch != record.epoch
            || self.binding_generation != record.binding_generation
            || self.accepted_capabilities != record.accepted_capabilities
        {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "host binding record does not match the accepting transition for mob '{mob_id}'"
                ),
            });
        }
        Ok(())
    }
}

/// Witness that a `MobHostBindingAuthority` transition revoked the binding —
/// the only way to delete a host-binding row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostBindingDeletionAuthority {
    mob_id: AuthorityMobId,
    supervisor_peer_id: AuthorityPeerId,
    epoch: u64,
    binding_generation: u64,
}

impl HostBindingDeletionAuthority {
    pub fn from_transition(
        mob_id: &AuthorityMobId,
        transition: &MobHostBindingAuthorityTransition,
    ) -> Result<Self, MobHostActorError> {
        let revoked = transition.effects().iter().find_map(|effect| match effect {
            MobHostBindingAuthorityEffect::HostBindingRevoked {
                mob_id: effect_mob,
                supervisor_peer_id,
                epoch,
                binding_generation,
            } if effect_mob == mob_id => {
                Some((supervisor_peer_id.clone(), *epoch, *binding_generation))
            }
            _ => None,
        });
        let Some((supervisor_peer_id, epoch, binding_generation)) = revoked else {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "transition carries no host binding revoke for mob '{}'",
                    mob_id.0
                ),
            });
        };
        Ok(Self {
            mob_id: mob_id.clone(),
            supervisor_peer_id,
            epoch,
            binding_generation,
        })
    }

    pub fn verify_mob(&self, mob_id: &str) -> Result<(), MobHostActorError> {
        if self.mob_id.0 != mob_id {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "deletion witness names mob '{}', not '{mob_id}'",
                    self.mob_id.0
                ),
            });
        }
        Ok(())
    }

    pub fn verify_receipt(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        receipt: &MobHostRevocationReceipt,
    ) -> Result<(), MobHostActorError> {
        self.verify_mob(mob_id)?;
        let expected_released: Vec<String> = expected.materialized.keys().cloned().collect();
        if expected.supervisor_peer_id != self.supervisor_peer_id.0
            || expected.epoch != self.epoch
            || expected.binding_generation != self.binding_generation
            || receipt.supervisor_peer_id != self.supervisor_peer_id.0
            || receipt.supervisor_signing_key != expected.supervisor_signing_key
            || receipt.epoch != self.epoch
            || receipt.binding_generation != self.binding_generation
            || receipt.released_members != expected_released
        {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "host revocation receipt does not match the machine-authorized binding tuple for mob '{mob_id}'"
                ),
            });
        }
        Ok(())
    }
}

/// Witness that a `MobHostBindingAuthority` transition recorded a member
/// materialization or release — the only way to CAS the member regions of a
/// host-binding row (DEC-P3H-7 discipline, DEC-P3H-9 ordering: this write
/// happens BETWEEN the prepared apply and the commit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberRowPersistenceAuthority {
    Materialized {
        member_key: MemberKey,
        generation: u64,
        fence_token: u64,
        session_id: String,
    },
    Released {
        member_key: MemberKey,
        disposal: MachineMemberSessionDisposal,
    },
}

impl MemberRowPersistenceAuthority {
    pub fn from_materialized_transition(
        member_key: &MemberKey,
        transition: &MobHostBindingAuthorityTransition,
    ) -> Result<Self, MobHostActorError> {
        transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                MobHostBindingAuthorityEffect::MaterializedMemberRecorded {
                    member_key: effect_key,
                    generation,
                    fence_token,
                    session_id,
                } if effect_key == member_key => Some(Self::Materialized {
                    member_key: member_key.clone(),
                    generation: generation.0,
                    fence_token: fence_token.0,
                    session_id: session_id.0.clone(),
                }),
                _ => None,
            })
            .ok_or_else(|| MobHostActorError::Witness {
                detail: format!(
                    "transition carries no MaterializedMemberRecorded for member '{}' of mob '{}'",
                    member_key.agent_identity.0, member_key.mob_id.0
                ),
            })
    }

    pub fn from_release_transition(
        member_key: &MemberKey,
        transition: &MobHostBindingAuthorityTransition,
    ) -> Result<Self, MobHostActorError> {
        transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                MobHostBindingAuthorityEffect::MemberReleaseRecorded {
                    member_key: effect_key,
                    disposal,
                } if effect_key == member_key => Some(Self::Released {
                    member_key: member_key.clone(),
                    disposal: *disposal,
                }),
                _ => None,
            })
            .ok_or_else(|| MobHostActorError::Witness {
                detail: format!(
                    "transition carries no MemberReleaseRecorded for member '{}' of mob '{}'",
                    member_key.agent_identity.0, member_key.mob_id.0
                ),
            })
    }

    /// Verify `next` realizes exactly the transition this witness carries:
    /// a materialize write must land the recorded tuple in the materialized
    /// region; a release write must land the recorded disposal in the
    /// release region AND clear the materialized entry (the machine's
    /// row-move mirror).
    pub fn verify_regions(
        &self,
        mob_id: &str,
        next: &MobHostBindingRecord,
    ) -> Result<(), MobHostActorError> {
        match self {
            Self::Materialized {
                member_key,
                generation,
                fence_token,
                session_id,
            } => {
                if member_key.mob_id.0 != mob_id {
                    return Err(MobHostActorError::Witness {
                        detail: format!(
                            "materialized-row witness names mob '{}', not '{mob_id}'",
                            member_key.mob_id.0
                        ),
                    });
                }
                let row = next
                    .materialized
                    .get(&member_key.agent_identity.0)
                    .ok_or_else(|| MobHostActorError::Witness {
                        detail: format!(
                            "materialized-row write for '{}' lacks the recorded row",
                            member_key.agent_identity.0
                        ),
                    })?;
                if row.generation != *generation
                    || row.fence_token != *fence_token
                    || row.session_id != *session_id
                {
                    return Err(MobHostActorError::Witness {
                        detail: format!(
                            "materialized row for '{}' does not match the recording transition",
                            member_key.agent_identity.0
                        ),
                    });
                }
                let outcomes_current = next
                    .turn_outcomes
                    .get(&member_key.agent_identity.0)
                    .is_none_or(|rows| {
                        rows.iter().all(|row| {
                            row.generation == *generation && row.fence_token == *fence_token
                        })
                    });
                let pending_current = next
                    .turn_outcome_pending
                    .get(&member_key.agent_identity.0)
                    .is_none_or(|rows| {
                        rows.iter().all(|row| {
                            row.generation == *generation && row.fence_token == *fence_token
                        })
                    });
                let acknowledged_current = next
                    .turn_outcome_acknowledged
                    .get(&member_key.agent_identity.0)
                    .is_none_or(|rows| {
                        rows.iter().all(|row| {
                            row.generation == *generation && row.fence_token == *fence_token
                        })
                    });
                let cancellations_current = next
                    .tracked_input_cancellations
                    .get(&member_key.agent_identity.0)
                    .is_none_or(|rows| {
                        rows.iter().all(|row| {
                            row.generation == *generation && row.fence_token == *fence_token
                        })
                    });
                if !outcomes_current
                    || !pending_current
                    || !acknowledged_current
                    || !cancellations_current
                {
                    return Err(MobHostActorError::Witness {
                        detail: format!(
                            "materialized row for '{}' retained a stale turn-journal tuple",
                            member_key.agent_identity.0
                        ),
                    });
                }
                Ok(())
            }
            Self::Released {
                member_key,
                disposal,
            } => {
                if member_key.mob_id.0 != mob_id {
                    return Err(MobHostActorError::Witness {
                        detail: format!(
                            "released-row witness names mob '{}', not '{mob_id}'",
                            member_key.mob_id.0
                        ),
                    });
                }
                let row = next
                    .released
                    .get(&member_key.agent_identity.0)
                    .ok_or_else(|| MobHostActorError::Witness {
                        detail: format!(
                            "release-row write for '{}' lacks the recorded row",
                            member_key.agent_identity.0
                        ),
                    })?;
                if row.disposal.to_machine() != *disposal {
                    return Err(MobHostActorError::Witness {
                        detail: format!(
                            "release row for '{}' does not match the recorded disposal",
                            member_key.agent_identity.0
                        ),
                    });
                }
                if next.materialized.contains_key(&member_key.agent_identity.0) {
                    return Err(MobHostActorError::Witness {
                        detail: format!(
                            "release-row write for '{}' left the materialized entry in place",
                            member_key.agent_identity.0
                        ),
                    });
                }
                if next
                    .turn_outcomes
                    .contains_key(&member_key.agent_identity.0)
                    || next
                        .turn_outcome_pending
                        .contains_key(&member_key.agent_identity.0)
                    || next
                        .turn_outcome_acknowledged
                        .contains_key(&member_key.agent_identity.0)
                    || next
                        .tracked_input_cancellations
                        .contains_key(&member_key.agent_identity.0)
                {
                    return Err(MobHostActorError::Witness {
                        detail: format!(
                            "release-row write for '{}' retained turn-journal ownership",
                            member_key.agent_identity.0
                        ),
                    });
                }
                Ok(())
            }
        }
    }
}

/// Authority token for one durable capability-attachment obligation
/// mutation.
///
/// Deliberately NOT transition-derived: the obligation region carries no
/// lifecycle truth (see
/// [`MobHostBindingRecord::forked_participant_obligations`]) and the generated
/// `MobHostBindingAuthority` therefore owns no input for it. What the token
/// does enforce is that a write is shaped by the exact admitted association
/// the shell is currently holding, so no caller can fabricate, rekey, or
/// silently drop an obligation while pretending to record one.
#[derive(Debug)]
pub enum ForkedParticipantObligationAuthority {
    /// Retain one obligation under its association key.
    Retained {
        mob_id: String,
        key: String,
        obligation: ForkedParticipantAttachmentObligation,
    },
    /// Discharge one obligation whose reconciliation was proven.
    Discharged { mob_id: String, key: String },
}

impl ForkedParticipantObligationAuthority {
    /// Derive a retention token from the exact admitted association.
    pub fn retain(
        mob_id: &str,
        agent_identity: &str,
        association: &ForkedParticipantAttachmentAssociation,
        cause: ForkedParticipantObligationCause,
        detail: impl Into<String>,
    ) -> Self {
        Self::Retained {
            mob_id: mob_id.to_string(),
            key: association.association_key(),
            obligation: ForkedParticipantAttachmentObligation {
                agent_identity: agent_identity.to_string(),
                association: association.clone(),
                cause,
                detail: detail.into(),
            },
        }
    }

    /// Derive a discharge token from the exact reconciled association.
    pub fn discharge(mob_id: &str, association: &ForkedParticipantAttachmentAssociation) -> Self {
        Self::Discharged {
            mob_id: mob_id.to_string(),
            key: association.association_key(),
        }
    }

    /// Apply this token to a record clone, producing the CAS successor.
    pub fn apply(&self, expected: &MobHostBindingRecord) -> MobHostBindingRecord {
        let mut next = expected.clone();
        match self {
            Self::Retained {
                key, obligation, ..
            } => {
                next.forked_participant_obligations
                    .insert(key.clone(), obligation.clone());
            }
            Self::Discharged { key, .. } => {
                next.forked_participant_obligations.remove(key);
            }
        }
        next
    }

    /// Verify `next` realizes exactly this token and nothing else.
    pub fn verify_next(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
    ) -> Result<(), MobHostActorError> {
        let (token_mob, key) = match self {
            Self::Retained { mob_id, key, .. } | Self::Discharged { mob_id, key } => (mob_id, key),
        };
        if token_mob != mob_id {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "capability obligation witness names mob '{token_mob}', not '{mob_id}'"
                ),
            });
        }
        match self {
            Self::Retained { obligation, .. } => {
                if next.forked_participant_obligations.get(key) != Some(obligation) {
                    return Err(MobHostActorError::Witness {
                        detail: format!(
                            "capability obligation write for mob '{mob_id}' lacks the retained entry"
                        ),
                    });
                }
            }
            Self::Discharged { .. } => {
                if next.forked_participant_obligations.contains_key(key) {
                    return Err(MobHostActorError::Witness {
                        detail: format!(
                            "capability obligation write for mob '{mob_id}' retained a discharged entry"
                        ),
                    });
                }
            }
        }
        if self.apply(expected) != *next {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "capability obligation write for mob '{mob_id}' altered a sibling region"
                ),
            });
        }
        Ok(())
    }
}

/// Reply-free outcome of one exact member-residency teardown.
#[derive(Debug)]
pub enum MemberTeardownOutcome {
    /// Disposal, capability release, and the durable release receipt all
    /// committed. Carries the wire disposal a bridge caller acks with.
    Released { disposal: WireMemberSessionDisposal },
    /// NOTHING was recorded. The materialized row — and any capability
    /// association it carries — is retained, and the same tuple stays exactly
    /// retryable.
    Retained {
        cause: BridgeRejectionCause,
        detail: String,
    },
    /// The durable release receipt committed, but a post-commit step did not.
    /// The row IS released; retry converges through release replay.
    ReleasedWithResidue {
        cause: BridgeRejectionCause,
        detail: String,
    },
}

/// One pending capability terminal correlated to the exact durable residency
/// that holds its attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingForkedParticipantCorrelation {
    /// Mob whose durable host binding row carries the residency.
    pub mob_id: String,
    /// Member identity of the residency.
    pub agent_identity: String,
    /// Recorded materialization generation (the release fence).
    pub generation: u64,
    /// Recorded materialization fence token (the release fence).
    pub fence_token: u64,
    /// Recorded member session the residency owns.
    pub session_id: String,
    /// Which terminal the capability is parked on.
    pub terminal: crate::forked_participant::ForkedParticipantPendingTerminal,
    /// The exact association both sides agree on.
    pub association: ForkedParticipantAttachmentAssociation,
}

/// Correlate parked capability terminals to the durable residencies holding
/// their attachments.
///
/// Correlation is by FULL association equality — the complete immutable
/// capability reference plus the exact attachment id — never by session id,
/// member name, or any other reconstructed string. Two rows cannot both match
/// one pending entry, because a capability admits one active attachment at a
/// time and the association is recorded verbatim on exactly the residency that
/// was materialized under it.
///
/// A pending entry with no matching row is deliberately NOT returned: this
/// host does not hold that attachment, so it has nothing to converge and must
/// not release anything on the strength of a name.
pub fn correlate_pending_forked_participant_attachments(
    pending: &[crate::forked_participant::ForkedParticipantPendingAttachment],
    records: &[(String, MobHostBindingRecord)],
) -> Vec<PendingForkedParticipantCorrelation> {
    let mut correlated = Vec::new();
    for entry in pending {
        let association = entry.association();
        for (mob_id, record) in records {
            for (agent_identity, row) in &record.materialized {
                if row.forked_participant_attachment.as_ref() != Some(&association) {
                    continue;
                }
                correlated.push(PendingForkedParticipantCorrelation {
                    mob_id: mob_id.clone(),
                    agent_identity: agent_identity.clone(),
                    generation: row.generation,
                    fence_token: row.fence_token,
                    session_id: row.session_id.clone(),
                    terminal: entry.terminal,
                    association: association.clone(),
                });
            }
        }
    }
    correlated
}

/// Typed tally of one autonomous convergence pass.
///
/// Retained work is COUNTED, never silently dropped: an operator reading the
/// sweep line must be able to see that a capability is still parked and why
/// the host could not converge it this tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ForkedParticipantConvergenceCounts {
    /// Residencies torn down and attachments released this pass.
    pub converged: usize,
    /// Correlated residencies whose teardown could not be proven this pass.
    /// Their rows and associations are retained for the next tick.
    pub retained: usize,
    /// Parked capabilities this host holds no residency for. Nothing is
    /// released for these: another holder owns the attachment.
    pub unheld: usize,
    /// Capability records whose parked phase could not be read.
    pub unreadable: usize,
}

/// Why one member teardown was withheld, leaving the row exactly retryable.
#[derive(Debug, Clone)]
pub struct MemberTeardownRetention {
    /// Typed wire cause for the caller's rejection.
    pub cause: BridgeRejectionCause,
    /// Operator-facing detail. Diagnostics only.
    pub detail: String,
    /// The durable outcome could not be classified; the actor must fail-stop.
    pub durable_uncertainty: bool,
}

impl std::fmt::Display for MemberTeardownRetention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

/// The durable half of one member teardown, run after the exact residency has
/// been proven disposed: release the associated capability attachment, then
/// record the member release under its transition witness.
///
/// The capability release comes FIRST and its failure withholds the receipt.
/// Once the row moves to the release region the association moves with it, so
/// a receipt written over an unproven release would strand an attachment with
/// nothing left pointing at it. Withholding instead keeps the materialized row
/// and its association intact, and the same tuple stays exactly retryable —
/// disposal is idempotent, and an already-released attachment converges.
///
/// Exact replay after a committed release never reaches here (the machine
/// answers `ReleaseReplay` from the release region), so no attachment is ever
/// released twice.
pub async fn commit_member_release_after_disposal(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    forked_participant_service: Option<&ForkedParticipantService>,
    member_key: &MemberKey,
    generation: u64,
    fence_token: u64,
    disposal: MachineMemberSessionDisposal,
) -> Result<(ReleasedIdentityWitness, String), MemberTeardownRetention> {
    let mob_id = member_key.mob_id.0.clone();
    let agent_identity = member_key.agent_identity.0.clone();
    let associated_attachment = match persistence.load(&mob_id).await {
        Ok(Some(record)) => record
            .materialized
            .get(&agent_identity)
            .and_then(|row| row.forked_participant_attachment.clone()),
        Ok(None) => None,
        Err(error) => {
            return Err(MemberTeardownRetention {
                cause: BridgeRejectionCause::Internal,
                detail: format!("release capability association load failed: {error}"),
                durable_uncertainty: false,
            });
        }
    };
    if let Some(association) = associated_attachment.as_ref()
        && let Err(failure) =
            release_forked_participant_association(forked_participant_service, association).await
    {
        return Err(MemberTeardownRetention {
            cause: failure.cause.clone(),
            detail: format!(
                "member release withheld: the associated capability attachment could not be \
                 released ({failure}); the materialized row and its association are retained for \
                 exact retry"
            ),
            durable_uncertainty: false,
        });
    }
    record_member_release(
        authority,
        persistence,
        member_key,
        generation,
        fence_token,
        disposal,
    )
    .await
    .map_err(|error| MemberTeardownRetention {
        cause: BridgeRejectionCause::Internal,
        detail: format!("release record persistence failed: {error}"),
        durable_uncertainty: error.is_durable_uncertainty(),
    })
}

/// Thin host conversion shell over THE containment rule.
///
/// Wire attachment -> typed proof, owner truth -> admission, typed rejection ->
/// this surface's wire vocabulary. It holds no comparison of its own: every
/// presence/reference/route/id decision lives in
/// [`adjudicate_protected_resume`], so the host and local paths cannot drift.
///
/// Refusals map to `ForkedParticipantTampered` rather than "not found": the
/// capability plainly exists, and what failed is that the caller offered
/// material which does not authenticate the operation. Route mismatch keeps
/// its own established cause. Every refusal happens before `attach`, so a
/// rejected request never consumes a use of the capability it failed to
/// present.
pub fn admit_resume_against_fork_protection(
    session_id: &str,
    protection: Option<&crate::forked_participant::ForkedParticipantForkProtection>,
    attachment: Option<
        &meerkat_contracts::wire::supervisor_bridge::BridgeForkedParticipantAttachment,
    >,
    owner_route: &ForkedParticipantOwnerRoute,
) -> Result<(), (BridgeRejectionCause, String)> {
    let proof = match attachment {
        None => ForkedParticipantResumeProof::Absent,
        Some(attachment) => match domain_ref(&attachment.capability) {
            Ok(full_ref) => ForkedParticipantResumeProof::HostCapabilityAttachment {
                full_ref,
                attachment_id: attachment.attachment_id.clone(),
                owner_route: owner_route.clone(),
            },
            // A reference that will not even parse cannot be the recorded one.
            // Presenting it is the same class of failure as presenting a
            // rewritten one, so it takes the same typed rejection instead of a
            // second, surface-local verdict.
            Err(_) => {
                return Err((
                    BridgeRejectionCause::ForkedParticipantTampered,
                    format!(
                        "resume of session '{session_id}' presented a malformed forked \
                         participant reference"
                    ),
                ));
            }
        },
    };
    match adjudicate_protected_resume(protection, &proof) {
        Ok(_) => Ok(()),
        Err(rejection) => {
            let cause = match rejection {
                ForkedParticipantResumeRejection::ForeignRoute { .. } => {
                    BridgeRejectionCause::ForkedParticipantRouteMismatch
                }
                _ => BridgeRejectionCause::ForkedParticipantTampered,
            };
            Err((
                cause,
                format!("resume of session '{session_id}': {rejection}"),
            ))
        }
    }
}

/// Witness that a transition (or a validated recovery fold) authorized
/// installing a member identity on the acceptor registry (DEC-P3H-7:
/// registration is post-persist/post-commit and re-attempted idempotently by
/// `MaterializeReplay` / boot revival).
#[derive(Debug)]
pub struct MaterializedIdentityWitness {
    member_key: MemberKey,
}

impl MaterializedIdentityWitness {
    pub fn from_transition(
        member_key: &MemberKey,
        transition: &MobHostBindingAuthorityTransition,
    ) -> Result<Self, MobHostActorError> {
        let matched = transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobHostBindingAuthorityEffect::MaterializedMemberRecorded { member_key: k, .. }
                | MobHostBindingAuthorityEffect::MaterializeReplay { member_key: k, .. }
                    if k == member_key
            )
        });
        if !matched {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "transition carries no materialize record/replay for member '{}' of mob '{}'",
                    member_key.agent_identity.0, member_key.mob_id.0
                ),
            });
        }
        Ok(Self {
            member_key: member_key.clone(),
        })
    }

    /// Recovery-path constructor: derivable only where the recovered
    /// authority state (the durable `MaterializedMemberRecorded`
    /// consequence) still carries the member's materialized row.
    pub(crate) fn from_recovered_state(
        state: &MobHostBindingAuthorityState,
        member_key: &MemberKey,
    ) -> Result<Self, MobHostActorError> {
        if !state.materialized_generations.contains_key(member_key) {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "recovered state carries no materialized row for member '{}' of mob '{}'",
                    member_key.agent_identity.0, member_key.mob_id.0
                ),
            });
        }
        Ok(Self {
            member_key: member_key.clone(),
        })
    }

    pub fn member_key(&self) -> &MemberKey {
        &self.member_key
    }
}

/// Witness that a transition released a member — the only way to remove its
/// acceptor identity (`ReleaseReplay` re-attempts removal idempotently).
#[derive(Debug)]
pub struct ReleasedIdentityWitness {
    member_key: MemberKey,
}

impl ReleasedIdentityWitness {
    pub fn from_transition(
        member_key: &MemberKey,
        transition: &MobHostBindingAuthorityTransition,
    ) -> Result<Self, MobHostActorError> {
        let matched = transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobHostBindingAuthorityEffect::MemberReleaseRecorded { member_key: k, .. }
                | MobHostBindingAuthorityEffect::ReleaseReplay { member_key: k, .. }
                    if k == member_key
            )
        });
        if !matched {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "transition carries no release record/replay for member '{}' of mob '{}'",
                    member_key.agent_identity.0, member_key.mob_id.0
                ),
            });
        }
        Ok(Self {
            member_key: member_key.clone(),
        })
    }

    pub fn member_key(&self) -> &MemberKey {
        &self.member_key
    }
}

/// Typed persistence seam over the raw `runtime_mob_host_bindings` accessors.
#[async_trait]
pub trait MobHostBindingPersistence: Send + Sync {
    async fn list_records(&self) -> Result<Vec<(String, MobHostBindingRecord)>, MobHostActorError>;
    async fn load(&self, mob_id: &str) -> Result<Option<MobHostBindingRecord>, MobHostActorError>;
    async fn list_revocations(
        &self,
    ) -> Result<Vec<(String, MobHostRevocationReceipt)>, MobHostActorError>;
    async fn load_revocation(
        &self,
        mob_id: &str,
    ) -> Result<Option<MobHostRevocationReceipt>, MobHostActorError>;
    async fn put_if_absent(
        &self,
        mob_id: &str,
        record: &MobHostBindingRecord,
        authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError>;
    async fn compare_and_put(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError>;
    /// CAS of the member (materialized/released) regions, gated by a
    /// materialize/release recording-transition witness. The supervisor
    /// binding region must be carried through unchanged.
    async fn compare_and_put_member_rows(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &MemberRowPersistenceAuthority,
    ) -> Result<bool, MobHostActorError>;

    /// CAS of the durable pre-accept Pending region, gated by the generated
    /// reserve/cancel transition witness. This region is part of the same mob
    /// binding blob so capacity cannot race terminal installation.
    async fn compare_and_put_turn_outcome_pending(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomePendingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let _ = (mob_id, expected, next, authority);
        Err(MobHostActorError::StoreDiverged {
            detail: "turn-outcome pending persistence is not supported by this binding \
                     persistence impl"
                .to_string(),
        })
    }

    /// CAS of the §18 O2 turn-outcome journal region, gated by a
    /// `TurnOutcomeRecorded` transition witness. Fail-closed default: a
    /// persistence impl that never learned the journal region rejects the
    /// write typed (the record path then records nothing and the
    /// controlling step resolves via the timeout ladder) — never a silent
    /// success.
    async fn compare_and_put_turn_outcomes(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomePersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let _ = (mob_id, expected, next, authority);
        Err(MobHostActorError::StoreDiverged {
            detail: "turn-outcome journal persistence is not supported by this binding \
                     persistence impl"
                .to_string(),
        })
    }
    /// CAS that prunes one explicitly acknowledged outcome row. Fail closed
    /// by default so a persistence implementation cannot claim an ack while
    /// retaining an unbounded durable row.
    async fn compare_and_put_turn_outcome_ack(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomeAckPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let _ = (mob_id, expected, next, authority);
        Err(MobHostActorError::StoreDiverged {
            detail: "turn-outcome acknowledgement persistence is not supported by this binding \
                     persistence impl"
                .to_string(),
        })
    }
    /// CAS of the lifecycle-bounded tracked-input cancellation region. A
    /// fresh/request transition may atomically consume Pending; every other
    /// sibling region is immutable.
    async fn compare_and_put_tracked_input_cancel(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TrackedInputCancelPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let _ = (mob_id, expected, next, authority);
        Err(MobHostActorError::StoreDiverged {
            detail: "tracked-input cancellation persistence is not supported by this binding persistence impl"
                .to_string(),
        })
    }
    /// CAS of the capability-attachment obligation region, gated by the
    /// association-shaped token. Fail-closed default: a persistence impl that
    /// never learned the region refuses the write typed, so the caller
    /// fail-stops with an unreconciled attachment rather than silently
    /// forgetting one.
    async fn compare_and_put_forked_participant_obligations(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &ForkedParticipantObligationAuthority,
    ) -> Result<bool, MobHostActorError> {
        let _ = (mob_id, expected, next, authority);
        Err(MobHostActorError::StoreDiverged {
            detail: "forked participant obligation persistence is not supported by this binding \
                     persistence impl"
                .to_string(),
        })
    }
    /// Atomically replace the expected active binding with its durable revoke
    /// receipt. No implementation may expose a delete-without-receipt path:
    /// that would make reply loss indistinguishable from an unbound mob.
    async fn revoke(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        receipt: &MobHostRevocationReceipt,
        authority: &HostBindingDeletionAuthority,
    ) -> Result<bool, MobHostActorError>;
}

/// The sole production impl: typed (de)serialization over the runtime
/// store's raw record-JSON accessors (the R8 table lives in
/// `meerkat-runtime`'s `SqliteRuntimeStore`; DEC-P2-6).
pub struct RuntimeStoreHostBindingPersistence {
    store: Arc<dyn meerkat_runtime::store::RuntimeStore>,
}

impl RuntimeStoreHostBindingPersistence {
    pub fn new(store: Arc<dyn meerkat_runtime::store::RuntimeStore>) -> Self {
        Self { store }
    }
}

fn encode_record(record: &MobHostBindingRecord) -> Result<Vec<u8>, MobHostActorError> {
    serde_json::to_vec(record).map_err(|err| MobHostActorError::RecordSerde {
        detail: err.to_string(),
    })
}

fn decode_record(bytes: &[u8]) -> Result<MobHostBindingRecord, MobHostActorError> {
    serde_json::from_slice(bytes).map_err(|err| MobHostActorError::RecordSerde {
        detail: err.to_string(),
    })
}

fn encode_revocation(receipt: &MobHostRevocationReceipt) -> Result<Vec<u8>, MobHostActorError> {
    serde_json::to_vec(receipt).map_err(|err| MobHostActorError::RecordSerde {
        detail: err.to_string(),
    })
}

fn decode_revocation(bytes: &[u8]) -> Result<MobHostRevocationReceipt, MobHostActorError> {
    serde_json::from_slice(bytes).map_err(|err| MobHostActorError::RecordSerde {
        detail: err.to_string(),
    })
}

#[async_trait]
impl MobHostBindingPersistence for RuntimeStoreHostBindingPersistence {
    async fn list_records(&self) -> Result<Vec<(String, MobHostBindingRecord)>, MobHostActorError> {
        let rows = self.store.list_mob_host_bindings().await?;
        rows.into_iter()
            .map(|(mob_id, bytes)| Ok((mob_id, decode_record(&bytes)?)))
            .collect()
    }

    async fn load(&self, mob_id: &str) -> Result<Option<MobHostBindingRecord>, MobHostActorError> {
        self.store
            .load_mob_host_binding(mob_id)
            .await?
            .map(|bytes| decode_record(&bytes))
            .transpose()
    }

    async fn list_revocations(
        &self,
    ) -> Result<Vec<(String, MobHostRevocationReceipt)>, MobHostActorError> {
        let rows = self.store.list_mob_host_revocations().await?;
        rows.into_iter()
            .map(|(mob_id, bytes)| Ok((mob_id, decode_revocation(&bytes)?)))
            .collect()
    }

    async fn load_revocation(
        &self,
        mob_id: &str,
    ) -> Result<Option<MobHostRevocationReceipt>, MobHostActorError> {
        self.store
            .load_mob_host_revocation(mob_id)
            .await?
            .map(|bytes| decode_revocation(&bytes))
            .transpose()
    }

    async fn put_if_absent(
        &self,
        mob_id: &str,
        record: &MobHostBindingRecord,
        authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        authority.verify_record(mob_id, record)?;
        Ok(self
            .store
            .put_mob_host_binding_if_absent(mob_id, &encode_record(record)?)
            .await?)
    }

    async fn compare_and_put(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        authority.verify_record(mob_id, next)?;
        Ok(self
            .store
            .compare_and_put_mob_host_binding(
                mob_id,
                &encode_record(expected)?,
                &encode_record(next)?,
            )
            .await?)
    }

    async fn compare_and_put_member_rows(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &MemberRowPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        authority.verify_regions(mob_id, next)?;
        // The member-region CAS never rewrites the binding region.
        if expected.supervisor_peer_id != next.supervisor_peer_id
            || expected.supervisor_signing_key != next.supervisor_signing_key
            || expected.epoch != next.epoch
            || expected.binding_generation != next.binding_generation
            || expected.accepted_capabilities != next.accepted_capabilities
        {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "member-row write for mob '{mob_id}' attempted to alter the binding region"
                ),
            });
        }
        Ok(self
            .store
            .compare_and_put_mob_host_binding(
                mob_id,
                &encode_record(expected)?,
                &encode_record(next)?,
            )
            .await?)
    }

    async fn compare_and_put_turn_outcome_pending(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomePendingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        authority.verify_next(next)?;
        if expected.supervisor_peer_id != next.supervisor_peer_id
            || expected.supervisor_signing_key != next.supervisor_signing_key
            || expected.epoch != next.epoch
            || expected.binding_generation != next.binding_generation
            || expected.accepted_capabilities != next.accepted_capabilities
            || expected.materialized != next.materialized
            || expected.released != next.released
            || expected.turn_outcomes != next.turn_outcomes
            || expected.turn_outcome_acknowledged != next.turn_outcome_acknowledged
            || expected.tracked_input_cancellations != next.tracked_input_cancellations
        {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "turn-outcome pending write for mob '{mob_id}' attempted to alter a sibling region"
                ),
            });
        }
        Ok(self
            .store
            .compare_and_put_mob_host_binding(
                mob_id,
                &encode_record(expected)?,
                &encode_record(next)?,
            )
            .await?)
    }

    async fn compare_and_put_turn_outcomes(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomePersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        authority.verify_next(next)?;
        // The journal-region CAS never rewrites the binding or member regions.
        if expected.supervisor_peer_id != next.supervisor_peer_id
            || expected.supervisor_signing_key != next.supervisor_signing_key
            || expected.epoch != next.epoch
            || expected.binding_generation != next.binding_generation
            || expected.accepted_capabilities != next.accepted_capabilities
            || expected.materialized != next.materialized
            || expected.released != next.released
            || expected.turn_outcome_acknowledged != next.turn_outcome_acknowledged
            || expected.tracked_input_cancellations != next.tracked_input_cancellations
        {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "turn-outcome write for mob '{mob_id}' attempted to alter a sibling region"
                ),
            });
        }
        Ok(self
            .store
            .compare_and_put_mob_host_binding(
                mob_id,
                &encode_record(expected)?,
                &encode_record(next)?,
            )
            .await?)
    }

    async fn compare_and_put_turn_outcome_ack(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomeAckPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        authority.verify_next(next)?;
        // Ack pruning changes only the outcome region. Binding and member
        // rows remain byte-for-byte anchored to the same durable snapshot.
        if expected.supervisor_peer_id != next.supervisor_peer_id
            || expected.supervisor_signing_key != next.supervisor_signing_key
            || expected.epoch != next.epoch
            || expected.binding_generation != next.binding_generation
            || expected.accepted_capabilities != next.accepted_capabilities
            || expected.materialized != next.materialized
            || expected.released != next.released
            || expected.turn_outcome_pending != next.turn_outcome_pending
            || expected.tracked_input_cancellations != next.tracked_input_cancellations
        {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "turn-outcome acknowledgement for mob '{mob_id}' attempted to alter a sibling region"
                ),
            });
        }
        Ok(self
            .store
            .compare_and_put_mob_host_binding(
                mob_id,
                &encode_record(expected)?,
                &encode_record(next)?,
            )
            .await?)
    }

    async fn compare_and_put_tracked_input_cancel(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TrackedInputCancelPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        authority.verify_next(next)?;
        if expected.supervisor_peer_id != next.supervisor_peer_id
            || expected.supervisor_signing_key != next.supervisor_signing_key
            || expected.epoch != next.epoch
            || expected.binding_generation != next.binding_generation
            || expected.accepted_capabilities != next.accepted_capabilities
            || expected.materialized != next.materialized
            || expected.released != next.released
            || expected.turn_outcomes != next.turn_outcomes
            || expected.turn_outcome_acknowledged != next.turn_outcome_acknowledged
        {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "tracked-input cancellation for mob '{mob_id}' attempted to alter a sibling region"
                ),
            });
        }
        Ok(self
            .store
            .compare_and_put_mob_host_binding(
                mob_id,
                &encode_record(expected)?,
                &encode_record(next)?,
            )
            .await?)
    }

    async fn compare_and_put_forked_participant_obligations(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &ForkedParticipantObligationAuthority,
    ) -> Result<bool, MobHostActorError> {
        authority.verify_next(mob_id, expected, next)?;
        Ok(self
            .store
            .compare_and_put_mob_host_binding(
                mob_id,
                &encode_record(expected)?,
                &encode_record(next)?,
            )
            .await?)
    }

    async fn revoke(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        receipt: &MobHostRevocationReceipt,
        authority: &HostBindingDeletionAuthority,
    ) -> Result<bool, MobHostActorError> {
        authority.verify_receipt(mob_id, expected, receipt)?;
        Ok(self
            .store
            .revoke_mob_host_binding(
                mob_id,
                &encode_record(expected)?,
                &encode_revocation(receipt)?,
            )
            .await?)
    }
}

// ---------------------------------------------------------------------------
// Recovery (R8 rows → generated recover_from_state seam)
// ---------------------------------------------------------------------------

/// Validate every durable fact that the authority fold and runtime revival
/// address through different representations. The outer store/map keys are
/// canonical; the embedded portable spec and recorded comms identity must
/// corroborate them before either path is allowed to construct residency.
fn validate_durable_materialized_member_row(
    mob_id: &str,
    agent_identity: &str,
    record: &MobHostBindingRecord,
    row: &MaterializedMemberRow,
) -> Result<(), MobHostActorError> {
    let corrupt = |detail: String| MobHostActorError::DurableMaterializedRowCorrupt {
        mob_id: mob_id.to_string(),
        agent_identity: agent_identity.to_string(),
        detail,
    };

    if row.spec.mob_id != mob_id {
        return Err(corrupt(format!(
            "outer mob id does not match portable spec mob id '{}'",
            row.spec.mob_id
        )));
    }
    if row.spec.agent_identity != agent_identity {
        return Err(corrupt(format!(
            "materialized map key does not match portable spec agent identity '{}'",
            row.spec.agent_identity
        )));
    }
    if record.binding_generation == 0 {
        return Err(corrupt(
            "materialized residency belongs to binding generation zero".to_string(),
        ));
    }
    // Generation zero is the domain's valid initial incarnation. The fence
    // and event floor, unlike generation, reserve zero as non-authoritative.
    if row.fence_token == 0 {
        return Err(corrupt("member fence token must be nonzero".to_string()));
    }
    if row.generation_start_seq == 0 {
        return Err(corrupt(
            "member generation start sequence must be nonzero".to_string(),
        ));
    }

    meerkat_core::types::SessionId::parse(&row.session_id)
        .map_err(|error| corrupt(format!("recorded member session id is invalid: {error}")))?;

    let recomputed_digest = portable_member_spec_digest(&row.spec)
        .map_err(|error| corrupt(format!("portable spec digest computation failed: {error}")))?;
    if recomputed_digest != row.spec_digest {
        return Err(corrupt(format!(
            "portable spec digest mismatch: recorded '{}', recomputed '{recomputed_digest}'",
            row.spec_digest
        )));
    }

    let member_pubkey = meerkat_comms::PubKey::from_pubkey_string(&row.member_pubkey)
        .map_err(|error| corrupt(format!("recorded member pubkey is invalid: {error}")))?;
    if member_pubkey.is_zero() {
        return Err(corrupt(
            "recorded member pubkey is the all-zero sentinel".to_string(),
        ));
    }
    let derived_member_peer_id = member_pubkey.to_peer_id().to_string();
    if row.member_peer_id != derived_member_peer_id {
        return Err(corrupt(format!(
            "recorded member peer id '{}' does not match pubkey-derived peer id '{derived_member_peer_id}'",
            row.member_peer_id
        )));
    }

    // The row's revival-only supervisor carrier must corroborate the outer
    // binding identity too. This also validates the stored peer name and
    // endpoint before boot can reach any member-side trust mutation.
    TrustedPeerDescriptor::unsigned_with_pubkey(
        row.supervisor_name.as_str(),
        record.supervisor_peer_id.as_str(),
        record.supervisor_signing_key,
        row.supervisor_address.as_str(),
    )
    .map_err(|error| {
        corrupt(format!(
            "recorded supervisor descriptor is invalid: {error}"
        ))
    })?;

    if let Some(association) = row.forked_participant_attachment.as_ref() {
        validate_forked_participant_association_shape(association, Some(&row.session_id))
            .map_err(corrupt)?;
        if !matches!(
            row.launch_outcome,
            MaterializeLaunchOutcome::ResumedLive | MaterializeLaunchOutcome::ResumedFromSnapshot
        ) {
            return Err(corrupt(
                "capability attachment association is recorded on a residency that did not \
                 resume its fork session"
                    .to_string(),
            ));
        }
    }

    Ok(())
}

/// Validate the mechanical shape of one attachment association.
///
/// Route OWNERSHIP (does this exact host/realm serve it) is checked separately
/// where the composed source-owner service is in scope; the shape facts here
/// are the ones a durable row must corroborate on its own. Deserialization has
/// already re-run every field's own validator, so what remains is the
/// cross-field agreement serde cannot express.
fn validate_forked_participant_association_shape(
    association: &ForkedParticipantAttachmentAssociation,
    expected_session_id: Option<&str>,
) -> Result<(), String> {
    let capability = &association.capability;
    if !matches!(
        capability.owner_route(),
        ForkedParticipantOwnerRoute::Host { .. }
    ) {
        return Err(
            "capability attachment association names a non-host owner route on a host-materialized \
             residency"
                .to_string(),
        );
    }
    if let Some(expected_session_id) = expected_session_id {
        let fork_session_id = capability.fork_session_id().to_string();
        if fork_session_id != expected_session_id {
            return Err(format!(
                "capability attachment association names fork session '{fork_session_id}', not the \
                 materialized session '{expected_session_id}'"
            ));
        }
    }
    let provenance = capability.provenance();
    if provenance.prefix_digest.trim().is_empty() {
        return Err("capability attachment association carries an empty prefix digest".to_string());
    }
    if provenance.source_session_id == *capability.fork_session_id() {
        return Err(
            "capability attachment association names the same session as source and fork"
                .to_string(),
        );
    }
    if capability.source_identity().as_str().trim().is_empty() {
        return Err(
            "capability attachment association carries an empty source identity".to_string(),
        );
    }
    Ok(())
}

/// Validate every recovered association/obligation against the exact route
/// this daemon serves.
///
/// Fail closed on principle: a durable association this host cannot route is
/// evidence that the row was written by (or moved from) a different owner, and
/// a host that quietly dropped it would later release nothing on teardown. A
/// host that recovered associations without composing the capability service
/// at all is the same class of failure — the residency exists but its
/// attachment could never be released.
pub fn validate_recovered_forked_participant_routes(
    records: &[(String, MobHostBindingRecord)],
    service: Option<&ForkedParticipantService>,
) -> Result<(), MobHostActorError> {
    for (mob_id, record) in records {
        let check = |agent_identity: &str,
                     association: &ForkedParticipantAttachmentAssociation,
                     expected_session_id: Option<&str>|
         -> Result<(), MobHostActorError> {
            let corrupt = |detail: String| MobHostActorError::DurableMaterializedRowCorrupt {
                mob_id: mob_id.clone(),
                agent_identity: agent_identity.to_string(),
                detail,
            };
            validate_forked_participant_association_shape(association, expected_session_id)
                .map_err(corrupt)?;
            let Some(service) = service else {
                return Err(corrupt(
                    "recovered capability attachment association has no composed source-owner \
                     service on this host"
                        .to_string(),
                ));
            };
            if association.capability.owner_route() != service.owner_route() {
                return Err(corrupt(
                    "recovered capability attachment association names another host route"
                        .to_string(),
                ));
            }
            Ok(())
        };
        for (agent_identity, row) in &record.materialized {
            if let Some(association) = row.forked_participant_attachment.as_ref() {
                check(agent_identity, association, Some(&row.session_id))?;
            }
        }
        for (key, obligation) in &record.forked_participant_obligations {
            if *key != obligation.association.association_key() {
                return Err(MobHostActorError::DurableMaterializedRowCorrupt {
                    mob_id: mob_id.clone(),
                    agent_identity: obligation.agent_identity.clone(),
                    detail: format!(
                        "capability obligation key '{key}' does not match its association"
                    ),
                });
            }
            check(&obligation.agent_identity, &obligation.association, None)?;
        }
    }
    Ok(())
}

fn validate_durable_materialized_member_records(
    records: &[(String, MobHostBindingRecord)],
) -> Result<(), MobHostActorError> {
    for (mob_id, record) in records {
        for (agent_identity, row) in &record.materialized {
            validate_durable_materialized_member_row(mob_id, agent_identity, record, row)?;
        }
    }
    Ok(())
}

/// Fold persisted binding rows into an authority state: the supervisor
/// binding region plus the phase-3 materialized/release regions (§14.6 —
/// key alignment is re-validated by the generated `recover_from_state`
/// invariants after [`validate_durable_materialized_member_records`] has
/// established the cross-representation facts that the generated state does
/// not carry. The turn-outcome journal region joins in phase 6.
fn authority_state_from_records(
    records: &[(String, MobHostBindingRecord)],
) -> MobHostBindingAuthorityState {
    let mut state = MobHostBindingAuthorityState::default();
    for (mob_id, record) in records {
        let mob = AuthorityMobId::from(mob_id.clone());
        state.supervisor_peer_ids.insert(
            mob.clone(),
            AuthorityPeerId::from(record.supervisor_peer_id.clone()),
        );
        state.supervisor_signing_keys.insert(
            mob.clone(),
            AuthorityPeerSigningKey::from(record.supervisor_signing_key),
        );
        state.supervisor_epochs.insert(mob.clone(), record.epoch);
        state
            .binding_generations
            .insert(mob.clone(), record.binding_generation);
        state
            .binding_generation_highwater
            .insert(mob.clone(), record.binding_generation);
        state
            .binding_phases
            .insert(mob.clone(), HostBindingPhase::Bound);
        for (identity, row) in &record.materialized {
            let key = MemberKey::new(mob.clone(), AuthorityAgentIdentity::from(identity.as_str()));
            state
                .materialized_generations
                .insert(key.clone(), AuthorityGeneration(row.generation));
            state
                .materialized_fences
                .insert(key.clone(), AuthorityFenceToken(row.fence_token));
            state
                .materialized_sessions
                .insert(key.clone(), AuthoritySessionId(row.session_id.clone()));
            state
                .materialized_spec_digests
                .insert(key, row.spec_digest.clone());
        }
        for (identity, row) in &record.released {
            let key = MemberKey::new(mob.clone(), AuthorityAgentIdentity::from(identity.as_str()));
            state
                .release_generations
                .insert(key.clone(), AuthorityGeneration(row.generation));
            state
                .release_fences
                .insert(key.clone(), AuthorityFenceToken(row.fence_token));
            state
                .release_disposals
                .insert(key, row.disposal.to_machine());
        }
        for (identity, rows) in &record.turn_outcome_pending {
            for row in rows {
                let turn_key = TurnKey::new(
                    mob.clone(),
                    AuthorityAgentIdentity::from(identity.as_str()),
                    AuthorityGeneration(row.generation),
                    AuthorityFenceToken(row.fence_token),
                    AuthorityInputId(row.input_id.clone()),
                );
                state
                    .turn_outcome_pending_window_starts
                    .insert(turn_key, row.window_start);
            }
        }
        for (identity, rows) in &record.turn_outcomes {
            for row in rows {
                let turn_key = TurnKey::new(
                    mob.clone(),
                    AuthorityAgentIdentity::from(identity.as_str()),
                    AuthorityGeneration(row.generation),
                    AuthorityFenceToken(row.fence_token),
                    AuthorityInputId(row.input_id.clone()),
                );
                state
                    .turn_outcome_terminal_seqs
                    .insert(turn_key.clone(), row.terminal_seq);
                state
                    .turn_outcome_kinds
                    .insert(turn_key, row.machine_kind());
            }
        }
        for (identity, rows) in &record.turn_outcome_acknowledged {
            for row in rows {
                let turn_key = TurnKey::new(
                    mob.clone(),
                    AuthorityAgentIdentity::from(identity.as_str()),
                    AuthorityGeneration(row.generation),
                    AuthorityFenceToken(row.fence_token),
                    AuthorityInputId(row.input_id.clone()),
                );
                state.turn_outcome_acknowledged.insert(turn_key, true);
            }
        }
        for (identity, rows) in &record.tracked_input_cancellations {
            for row in rows {
                let turn_key = TurnKey::new(
                    mob.clone(),
                    AuthorityAgentIdentity::from(identity.as_str()),
                    AuthorityGeneration(row.generation),
                    AuthorityFenceToken(row.fence_token),
                    AuthorityInputId(row.input_id.clone()),
                );
                state
                    .tracked_input_cancellations
                    .insert(turn_key, row.outcome.to_machine());
            }
        }
    }
    state
}

fn authority_state_from_persisted(
    records: &[(String, MobHostBindingRecord)],
    revocations: &[(String, MobHostRevocationReceipt)],
) -> Result<MobHostBindingAuthorityState, MobHostActorError> {
    validate_durable_materialized_member_records(records)?;
    let mut state = authority_state_from_records(records);
    let active: BTreeSet<&str> = records.iter().map(|(mob_id, _)| mob_id.as_str()).collect();
    for (mob_id, receipt) in revocations {
        if active.contains(mob_id.as_str()) {
            return Err(MobHostActorError::Recovery {
                detail: format!(
                    "mob '{mob_id}' has both an active host binding and a revoke receipt"
                ),
            });
        }
        let mob = AuthorityMobId::from(mob_id.clone());
        state.revoked_supervisor_peer_ids.insert(
            mob.clone(),
            AuthorityPeerId::from(receipt.supervisor_peer_id.clone()),
        );
        state.revoked_supervisor_signing_keys.insert(
            mob.clone(),
            AuthorityPeerSigningKey::from(receipt.supervisor_signing_key),
        );
        state
            .revoked_supervisor_epochs
            .insert(mob.clone(), receipt.epoch);
        state
            .revoked_binding_generations
            .insert(mob.clone(), receipt.binding_generation);
        state
            .binding_generation_highwater
            .insert(mob, receipt.binding_generation);
    }
    Ok(state)
}

/// Recover the binding authority from durable rows, or create a fresh one
/// when the store is empty. Invariant-rejected recovered state aborts
/// startup typed (fail closed — never a half-recovered daemon).
pub async fn recover_or_create_binding_authority(
    persistence: &dyn MobHostBindingPersistence,
) -> Result<MobHostBindingAuthorityAuthority, MobHostActorError> {
    let records = persistence.list_records().await?;
    let revocations = persistence.list_revocations().await?;
    recover_binding_authority_from_snapshot(&records, &revocations)
}

/// Recover against one already-loaded durable snapshot. Startup uses this
/// helper so authority recovery, projection seeding, and member revival all
/// consume the exact same validated rows; there is no second fallible store
/// scan after public startup state has begun to commit.
fn recover_binding_authority_from_snapshot(
    records: &[(String, MobHostBindingRecord)],
    revocations: &[(String, MobHostRevocationReceipt)],
) -> Result<MobHostBindingAuthorityAuthority, MobHostActorError> {
    if records.is_empty() && revocations.is_empty() {
        return Ok(MobHostBindingAuthorityAuthority::new());
    }
    let state = authority_state_from_persisted(records, revocations)?;
    MobHostBindingAuthorityAuthority::recover_from_state(state).map_err(|err| {
        MobHostActorError::Recovery {
            detail: err.to_string(),
        }
    })
}

/// Fully validated, effect-free input for one boot member revival.
///
/// Every conversion that can reject durable state is performed while host
/// startup is still private. The later revival walk may observe environmental
/// materialization failures, but it cannot abort startup after the descriptor
/// or acceptor registry has been exposed.
struct PreparedRecoveredMember {
    mob_id: String,
    identity: String,
    row: MaterializedMemberRow,
    binding_generation: u64,
    supervisor_epoch: u64,
    supervisor: TrustedPeerDescriptor,
    session_id: meerkat_core::types::SessionId,
    decompile: PreparedRecoveredDecompile,
    identity_witness: MaterializedIdentityWitness,
}

enum PreparedRecoveredDecompile {
    Ready(DecompiledMemberBuild),
    /// Host-local configuration absence is not durable-row corruption. Keep
    /// it as a prepared boot outcome so one unavailable member does not abort
    /// a multi-mob daemon, while every structural error still fails before
    /// descriptor publication.
    EnvironmentalFailure(PreparedRecoveredEnvironmentalFailure),
}

#[derive(Debug, thiserror::Error)]
enum PreparedRecoveredEnvironmentalFailure {
    #[error(transparent)]
    Decompile(#[from] MaterializeDecompileError),
    #[error("required environment key '{key}' is absent on this host")]
    MissingRequiredEnvKey { key: String },
}

fn prepare_recovered_members(
    records: &[(String, MobHostBindingRecord)],
    state: &MobHostBindingAuthorityState,
    member_substrate_configured: bool,
) -> Result<Vec<PreparedRecoveredMember>, MobHostActorError> {
    let mut prepared = Vec::new();
    for (mob_id, record) in records {
        for (identity, row) in &record.materialized {
            // The snapshot fold already performs this validation. Repeat it
            // here at the boundary that builds effect-carrying revival input
            // so future changes cannot accidentally weaken the transaction.
            validate_durable_materialized_member_row(mob_id, identity, record, row)?;
            validate_portable_spec_structure(&row.spec).map_err(|error| {
                MobHostActorError::DurableMaterializedRowCorrupt {
                    mob_id: mob_id.clone(),
                    agent_identity: identity.clone(),
                    detail: format!(
                        "recorded portable member spec is structurally invalid: {error}"
                    ),
                }
            })?;
            if !member_substrate_configured {
                continue;
            }
            let member_key = MemberKey::new(
                AuthorityMobId::from(mob_id.as_str()),
                AuthorityAgentIdentity::from(identity.as_str()),
            );
            let identity_witness =
                MaterializedIdentityWitness::from_recovered_state(state, &member_key)?;
            let supervisor = TrustedPeerDescriptor::unsigned_with_pubkey(
                row.supervisor_name.as_str(),
                record.supervisor_peer_id.as_str(),
                record.supervisor_signing_key,
                row.supervisor_address.clone(),
            )
            .map_err(|error| MobHostActorError::DurableMaterializedRowCorrupt {
                mob_id: mob_id.clone(),
                agent_identity: identity.clone(),
                detail: format!("recorded supervisor descriptor is invalid: {error}"),
            })?;
            let session_id =
                meerkat_core::types::SessionId::parse(&row.session_id).map_err(|error| {
                    MobHostActorError::DurableMaterializedRowCorrupt {
                        mob_id: mob_id.clone(),
                        agent_identity: identity.clone(),
                        detail: format!("recorded member session id is invalid: {error}"),
                    }
                })?;
            let missing_required_env_key = row
                .spec
                .required_env_keys
                .iter()
                .find(|key| std::env::var_os(key).is_none())
                .cloned();
            let decompile = if let Some(key) = missing_required_env_key {
                PreparedRecoveredDecompile::EnvironmentalFailure(
                    PreparedRecoveredEnvironmentalFailure::MissingRequiredEnvKey { key },
                )
            } else {
                match decompile_portable_spec(&row.spec) {
                    Ok(decompiled) => PreparedRecoveredDecompile::Ready(decompiled),
                    Err(error @ MaterializeDecompileError::McpEnvKeyMissing { .. }) => {
                        PreparedRecoveredDecompile::EnvironmentalFailure(error.into())
                    }
                    Err(error) => {
                        return Err(MobHostActorError::DurableMaterializedRowCorrupt {
                            mob_id: mob_id.clone(),
                            agent_identity: identity.clone(),
                            detail: format!(
                                "recorded portable member spec cannot be decompiled: {error}"
                            ),
                        });
                    }
                }
            };
            prepared.push(PreparedRecoveredMember {
                mob_id: mob_id.clone(),
                identity: identity.clone(),
                row: row.clone(),
                binding_generation: record.binding_generation,
                supervisor_epoch: record.epoch,
                supervisor,
                session_id,
                decompile,
                identity_witness,
            });
        }
    }
    Ok(prepared)
}

// ---------------------------------------------------------------------------
// Serving core (shell observes, machine decides) — comms-free, so the
// admission/persistence coupling is testable without a network
// ---------------------------------------------------------------------------

/// Shell observations for one `BindHost` command (A11 posture: pure facts,
/// never pre-decisions).
pub struct HostBindObservations {
    pub mob_id: String,
    pub supervisor: BridgePeerIdentity,
    pub epoch: u64,
    pub binding_generation: u64,
    pub sender_matches_supervisor: bool,
    pub address_matches: bool,
    pub token_valid: bool,
    pub accepted_capabilities: BridgeCapabilities,
}

/// Machine-adjudicated outcome of serving one `BindHost`.
#[derive(Debug)]
pub enum HostBindServeOutcome {
    /// Binding admitted. `fresh` distinguishes a first accept (token was
    /// consumed + re-minted) from an idempotent replay ack.
    Accepted {
        fresh: bool,
        supervisor: TrustedPeerDescriptor,
    },
    Rejected {
        cause: BridgeRejectionCause,
        reason: String,
    },
}

/// Shell observations for one `RebindHost` command.
pub struct HostRebindObservations {
    pub mob_id: String,
    pub supervisor: BridgePeerIdentity,
    pub epoch: u64,
    pub binding_generation: u64,
    /// Authenticated envelope signer matches the authority's RECORDED current
    /// supervisor. It is deliberately not compared with the proposed next
    /// supervisor carried by this request.
    pub sender_matches_supervisor: bool,
    pub accepted_capabilities: BridgeCapabilities,
}

/// Machine-adjudicated outcome of serving one `RebindHost`.
pub enum HostRebindServeOutcome {
    Accepted {
        supervisor: TrustedPeerDescriptor,
        /// The previously recorded supervisor peer id when rotation changed
        /// it (the stale trust edge to retire); `None` on idempotent replay.
        previous_supervisor_peer_id: Option<String>,
    },
    Rejected {
        cause: BridgeRejectionCause,
        reason: String,
    },
}

fn bridge_rejection_cause(kind: HostAdmissionRejectKind) -> BridgeRejectionCause {
    match kind {
        HostAdmissionRejectKind::NotBound => BridgeRejectionCause::NotBound,
        HostAdmissionRejectKind::StaleSupervisor => BridgeRejectionCause::StaleSupervisor,
        HostAdmissionRejectKind::SenderMismatch => BridgeRejectionCause::SenderMismatch,
        HostAdmissionRejectKind::InvalidBootstrapToken => {
            BridgeRejectionCause::InvalidBootstrapToken
        }

        HostAdmissionRejectKind::AddressMismatch => BridgeRejectionCause::AddressMismatch,
        HostAdmissionRejectKind::StaleFence => BridgeRejectionCause::StaleFence,
        HostAdmissionRejectKind::AlreadyBound => BridgeRejectionCause::AlreadyBound,
        // TurnDirectiveUnsupported belongs to the phase-3 command-admission
        // family; a bind/rebind can never produce it, and the wire cause set
        // has no dedicated carrier yet — Unsupported is the fail-closed map.
        HostAdmissionRejectKind::Unsupported
        | HostAdmissionRejectKind::TurnDirectiveUnsupported => BridgeRejectionCause::Unsupported,
    }
}

fn forked_participant_error_cause(error: &ForkedParticipantError) -> BridgeRejectionCause {
    use crate::machines::forked_participant_lifecycle::ForkedParticipantAttachDenial;
    match error {
        ForkedParticipantError::ForeignRoute { .. } => {
            BridgeRejectionCause::ForkedParticipantRouteMismatch
        }
        ForkedParticipantError::SourceOwnershipRejected { .. } => {
            BridgeRejectionCause::ForkedParticipantSourceMismatch
        }
        ForkedParticipantError::CapabilityRejected { .. } => {
            BridgeRejectionCause::ForkedParticipantTampered
        }
        ForkedParticipantError::AttachDenied { reason } => match reason {
            ForkedParticipantAttachDenial::Busy => BridgeRejectionCause::ForkedParticipantBusy,
            ForkedParticipantAttachDenial::Expired => {
                BridgeRejectionCause::ForkedParticipantExpired
            }
            ForkedParticipantAttachDenial::Revoked => {
                BridgeRejectionCause::ForkedParticipantRevoked
            }
            ForkedParticipantAttachDenial::Exhausted => {
                BridgeRejectionCause::ForkedParticipantExhausted
            }
            ForkedParticipantAttachDenial::AuthenticationInvalid
            | ForkedParticipantAttachDenial::NotActive
            | ForkedParticipantAttachDenial::MalformedAttachment
            | ForkedParticipantAttachDenial::AttachmentAlreadyReleased => {
                BridgeRejectionCause::ForkedParticipantTampered
            }
        },
        ForkedParticipantError::ConcurrentUpdate { .. } => {
            BridgeRejectionCause::ForkedParticipantBusy
        }
        ForkedParticipantError::Store(_) | ForkedParticipantError::Session(_) => {
            BridgeRejectionCause::Unavailable
        }
        _ => BridgeRejectionCause::Internal,
    }
}

// ---------------------------------------------------------------------------
// Host-side capability attachment association lifecycle (issue #159 phase 3
// slice B). The source-owner service remains the ONLY lifecycle authority;
// everything here is mechanical routing plus fail-closed bookkeeping.
// ---------------------------------------------------------------------------

/// Why one host-side capability release attempt could not be proven.
#[derive(Debug, Clone)]
pub struct ForkedParticipantReleaseFailure {
    /// Typed wire cause for the caller's rejection reply.
    pub cause: BridgeRejectionCause,
    /// Operator-facing detail. Diagnostics only; never pattern-matched.
    pub detail: String,
}

impl std::fmt::Display for ForkedParticipantReleaseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

/// Release exactly one associated capability attachment through its
/// source-owner service.
///
/// Convergence classes, deliberately explicit because teardown must be able to
/// finish after a partial earlier attempt:
///
/// * a granted release, an exact release replay, and every terminalizing
///   outcome (exhausted/revoked/expired) are all success — the attachment is
///   no longer active;
/// * `NoActiveAttachment` is success for the same reason: an earlier attempt
///   already released it, so a retry must not deadlock the row;
/// * `AttachmentMismatch` is NOT success — a different attachment is active
///   and asserting otherwise would be a lifecycle lie;
/// * an absent capability record is vacuous success: there is no attachment
///   left to release, and refusing forever would strand the residency.
pub async fn release_forked_participant_association(
    service: Option<&ForkedParticipantService>,
    association: &ForkedParticipantAttachmentAssociation,
) -> Result<(), ForkedParticipantReleaseFailure> {
    let Some(service) = service else {
        return Err(ForkedParticipantReleaseFailure {
            cause: BridgeRejectionCause::ForkedParticipantProtocolUnsupported,
            detail: "host holds a capability attachment association but composes no \
                     forked participant service to release it through"
                .to_string(),
        });
    };
    if association.capability.owner_route() != service.owner_route() {
        return Err(ForkedParticipantReleaseFailure {
            cause: BridgeRejectionCause::ForkedParticipantRouteMismatch,
            detail: "capability attachment association names another host route".to_string(),
        });
    }
    match service
        .release(&association.capability, &association.attachment_id)
        .await
    {
        Ok(_) => Ok(()),
        Err(ForkedParticipantError::ReleaseRejected {
            reason: crate::machines::forked_participant_lifecycle::ForkedParticipantReleaseRejection::NoActiveAttachment,
        }) => Ok(()),
        Err(ForkedParticipantError::Store(crate::store::MobStoreError::NotFound(detail))) => {
            tracing::warn!(
                attachment_id = %association.attachment_id,
                detail = %detail,
                "capability attachment association names a record the owner no longer holds; \
                 treating release as vacuously converged"
            );
            Ok(())
        }
        // A surviving release rejection (the mismatch arm) is unreconciled
        // teardown work, not a caller protocol error: the caller must retry
        // the same tuple, so it gets the cleanup-debt cause rather than the
        // holder-facing tamper/internal mapping.
        Err(error @ ForkedParticipantError::ReleaseRejected { .. }) => {
            Err(ForkedParticipantReleaseFailure {
                cause: BridgeRejectionCause::ForkedParticipantCleanupDebt,
                detail: format!("capability attachment release rejected: {error}"),
            })
        }
        Err(error) => Err(ForkedParticipantReleaseFailure {
            cause: forked_participant_error_cause(&error),
            detail: format!("capability attachment release failed: {error}"),
        }),
    }
}

/// Compare one incoming wire attachment against the durable association a
/// materialize REPLAY is answering.
///
/// Replay must be answer-only: it returns the already-recorded result and
/// performs no attach. Any difference — including `Some` against a recorded
/// `None`, or `None` against a recorded `Some` — means the two commands are
/// not the same command, so the replay is refused rather than allowed to
/// mutate capability lifecycle under an idempotency key that never named it.
pub fn replayed_forked_participant_attachment_matches(
    incoming: Option<
        &meerkat_contracts::wire::supervisor_bridge::BridgeForkedParticipantAttachment,
    >,
    durable: Option<&ForkedParticipantAttachmentAssociation>,
) -> bool {
    match (incoming, durable) {
        (None, None) => true,
        (Some(incoming), Some(durable)) => {
            incoming.attachment_id == durable.attachment_id.as_str()
                && bridge_ref(&durable.capability) == incoming.capability
        }
        _ => false,
    }
}

impl MobHostActor {
    /// Durably retain one un-forgettable capability reconciliation
    /// obligation, or report why it could not be retained.
    ///
    /// The caller is always already on a failure path, so this returns the
    /// composed operator detail rather than a second error type: an obligation
    /// that cannot be written is itself a durable-uncertainty fail-stop, never
    /// a silently dropped attachment.
    async fn retain_forked_participant_obligation(
        &self,
        mob_id: &str,
        agent_identity: &str,
        association: &ForkedParticipantAttachmentAssociation,
        cause: ForkedParticipantObligationCause,
        detail: &str,
    ) -> Result<(), String> {
        let token = ForkedParticipantObligationAuthority::retain(
            mob_id,
            agent_identity,
            association,
            cause,
            detail,
        );
        let Some(expected) = self
            .persistence
            .load(mob_id)
            .await
            .map_err(|error| format!("obligation record load failed: {error}"))?
        else {
            return Err(format!(
                "no durable host binding row for bound mob '{mob_id}'"
            ));
        };
        let next = token.apply(&expected);
        if next == expected {
            return Ok(());
        }
        match self
            .persistence
            .compare_and_put_forked_participant_obligations(mob_id, &expected, &next, &token)
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err("obligation write lost its compare-and-swap".to_string()),
            Err(error) => Err(format!("obligation write failed: {error}")),
        }
    }

    /// Durably discharge one reconciled obligation.
    async fn discharge_forked_participant_obligation(
        &self,
        mob_id: &str,
        association: &ForkedParticipantAttachmentAssociation,
    ) -> Result<(), String> {
        let token = ForkedParticipantObligationAuthority::discharge(mob_id, association);
        let Some(expected) = self
            .persistence
            .load(mob_id)
            .await
            .map_err(|error| format!("obligation record load failed: {error}"))?
        else {
            return Ok(());
        };
        let next = token.apply(&expected);
        if next == expected {
            return Ok(());
        }
        match self
            .persistence
            .compare_and_put_forked_participant_obligations(mob_id, &expected, &next, &token)
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err("obligation discharge lost its compare-and-swap".to_string()),
            Err(error) => Err(format!("obligation discharge failed: {error}")),
        }
    }

    /// Boot reconciliation of retained capability obligations.
    ///
    /// Conservative by construction: an obligation whose fork session may
    /// still be present as live or recorded residency is NOT released — it
    /// keeps its debt and makes the host observably unhealthy. Only definite
    /// absence (no materialized row naming the fork session, and no live
    /// session for it) licenses the compensating release.
    async fn reconcile_forked_participant_obligations(&mut self) {
        let records = match self.persistence.list_records().await {
            Ok(records) => records,
            Err(error) => {
                tracing::error!(
                    error = %error,
                    "capability obligation reconciliation could not read durable host rows"
                );
                return;
            }
        };
        for (mob_id, record) in records {
            for obligation in record.forked_participant_obligations.values() {
                let fork_session_id = obligation
                    .association
                    .capability
                    .fork_session_id()
                    .to_string();
                let recorded_residency = record
                    .materialized
                    .values()
                    .any(|row| row.session_id == fork_session_id);
                let live_residency = match meerkat_core::types::SessionId::parse(&fork_session_id) {
                    Ok(parsed) => match self.materializer.as_ref() {
                        Some(materializer) => materializer.session_live(&parsed).await,
                        None => false,
                    },
                    // An unparseable session id is not observable absence.
                    Err(_) => true,
                };
                if recorded_residency || live_residency {
                    tracing::warn!(
                        mob_id = %mob_id,
                        agent_identity = %obligation.agent_identity,
                        cause = ?obligation.cause,
                        "capability attachment obligation retains possible member presence; \
                         the debt is kept and HostStatus reports the host unhealthy"
                    );
                    self.forked_participant_debts
                        .insert((mob_id.clone(), obligation.association.association_key()));
                    continue;
                }
                match release_forked_participant_association(
                    self.forked_participant_service.as_ref(),
                    &obligation.association,
                )
                .await
                {
                    Ok(()) => {
                        if let Err(detail) = self
                            .discharge_forked_participant_obligation(
                                &mob_id,
                                &obligation.association,
                            )
                            .await
                        {
                            tracing::error!(
                                mob_id = %mob_id,
                                detail = %detail,
                                "capability attachment obligation was released but its durable \
                                 record could not be discharged"
                            );
                            self.forked_participant_debts
                                .insert((mob_id.clone(), obligation.association.association_key()));
                        }
                    }
                    Err(failure) => {
                        tracing::error!(
                            mob_id = %mob_id,
                            agent_identity = %obligation.agent_identity,
                            detail = %failure,
                            "capability attachment obligation could not be reconciled"
                        );
                        self.forked_participant_debts
                            .insert((mob_id.clone(), obligation.association.association_key()));
                    }
                }
            }
        }
    }
}

/// Compose the durable record for `mob` from the authority's binding tuple,
/// carrying `member_regions` (the materialized/released maps) forward
/// unchanged — the binding ceremony never touches the member regions, and a
/// CAS that dropped them would sever every recorded member (fail closed by
/// construction: regions ride the SAME blob).
fn record_from_authority_state(
    state: &MobHostBindingAuthorityState,
    mob: &AuthorityMobId,
    member_regions: Option<&MobHostBindingRecord>,
) -> Result<MobHostBindingRecord, MobHostActorError> {
    let (Some(peer), Some(key), Some(epoch), Some(binding_generation)) = (
        state.supervisor_peer_ids.get(mob),
        state.supervisor_signing_keys.get(mob),
        state.supervisor_epochs.get(mob),
        state.binding_generations.get(mob),
    ) else {
        return Err(MobHostActorError::Internal {
            detail: format!(
                "authority state has no complete binding tuple for mob '{}'",
                mob.0
            ),
        });
    };
    Ok(MobHostBindingRecord {
        supervisor_peer_id: peer.0.clone(),
        supervisor_signing_key: key.0,
        epoch: *epoch,
        binding_generation: *binding_generation,
        accepted_capabilities: member_regions
            .and_then(|record| record.accepted_capabilities.clone()),
        materialized: member_regions
            .map(|record| record.materialized.clone())
            .unwrap_or_default(),
        released: member_regions
            .map(|record| record.released.clone())
            .unwrap_or_default(),
        turn_outcome_pending: member_regions
            .map(|record| record.turn_outcome_pending.clone())
            .unwrap_or_default(),
        turn_outcomes: member_regions
            .map(|record| record.turn_outcomes.clone())
            .unwrap_or_default(),
        turn_outcome_acknowledged: member_regions
            .map(|record| record.turn_outcome_acknowledged.clone())
            .unwrap_or_default(),
        tracked_input_cancellations: member_regions
            .map(|record| record.tracked_input_cancellations.clone())
            .unwrap_or_default(),
        forked_participant_obligations: member_regions
            .map(|record| record.forked_participant_obligations.clone())
            .unwrap_or_default(),
    })
}

/// The exactly-one effect of a host bind/rebind/revoke transition. A
/// multi-effect or empty transition is a typed internal fault — never an
/// `_ => {}` silence (fail-closed effect fan-out).
fn single_host_effect(
    transition: &MobHostBindingAuthorityTransition,
) -> Result<&MobHostBindingAuthorityEffect, MobHostActorError> {
    match transition.effects() {
        [effect] => Ok(effect),
        effects => Err(MobHostActorError::Internal {
            detail: format!(
                "host binding transition emitted {} effects; exactly one expected",
                effects.len()
            ),
        }),
    }
}

/// Resolve an ambiguous CAS completion against one exact durable reread.
///
/// `Ok(false)` and store errors both leave the caller unable to infer whether
/// this exact write committed. Equality with `next` is the only proof that it
/// did; equality with `expected` is the only proof that it did not. Every
/// other observation is deliberately sticky uncertainty rather than a
/// generic conflict, because the prepared authority must neither commit nor
/// continue serving over an unknown durable terminal.
async fn reconcile_exact_member_region_cas(
    persistence: &dyn MobHostBindingPersistence,
    mob_id: &str,
    expected: &MobHostBindingRecord,
    next: &MobHostBindingRecord,
    write_outcome: Result<bool, MobHostActorError>,
    operation: &str,
) -> Result<(), MobHostActorError> {
    let write_error = match write_outcome {
        Ok(true) => return Ok(()),
        Ok(false) => None,
        Err(error) => Some(error),
    };
    let original = write_error
        .as_ref()
        .map_or_else(|| "CAS miss".to_string(), ToString::to_string);
    let current = match persistence.load(mob_id).await {
        Ok(Some(current)) => current,
        Ok(None) => {
            return Err(MobHostActorError::DurableUncertainty {
                detail: format!(
                    "{operation} for mob '{mob_id}' returned {original}; exact reread found no binding row"
                ),
            });
        }
        Err(error) => {
            return Err(MobHostActorError::DurableUncertainty {
                detail: format!(
                    "{operation} for mob '{mob_id}' returned {original}; exact reread failed: {error}"
                ),
            });
        }
    };

    if current == *next {
        return Ok(());
    }
    if current == *expected {
        return match write_error {
            Some(error) => Err(error),
            None => Err(MobHostActorError::StoreDiverged {
                detail: format!(
                    "{operation} CAS missed for mob '{mob_id}' and exact reread proved the expected row unchanged"
                ),
            }),
        };
    }

    Err(MobHostActorError::DurableUncertainty {
        detail: format!(
            "{operation} for mob '{mob_id}' returned {original}; exact reread found a third durable row"
        ),
    })
}

async fn require_exact_binding_after_uncertain_write(
    persistence: &dyn MobHostBindingPersistence,
    mob_id: &str,
    expected: &MobHostBindingRecord,
    context: &str,
) -> Result<(), MobHostActorError> {
    let mut last_observation = "row absent".to_string();
    for attempt in 0..3 {
        match persistence.load(mob_id).await {
            Ok(Some(stored)) if &stored == expected => return Ok(()),
            Ok(Some(_)) => {
                last_observation = "conflicting row present".to_string();
                break;
            }
            Ok(None) => last_observation = "row absent".to_string(),
            Err(error) => last_observation = format!("reread failed: {error}"),
        }
        if attempt < 2 {
            tokio::task::yield_now().await;
        }
    }
    Err(MobHostActorError::StoreDiverged {
        detail: format!(
            "{context} for mob '{mob_id}' did not converge to the exact durable binding row ({last_observation})"
        ),
    })
}

async fn require_exact_revocation_after_uncertain_write(
    persistence: &dyn MobHostBindingPersistence,
    mob_id: &str,
    expected: &MobHostRevocationReceipt,
    context: &str,
) -> Result<(), MobHostActorError> {
    let mut last_observation = "active/receipt state unresolved".to_string();
    for attempt in 0..3 {
        let active = persistence.load(mob_id).await;
        let receipt = persistence.load_revocation(mob_id).await;
        match (active, receipt) {
            (Ok(None), Ok(Some(stored))) if &stored == expected => return Ok(()),
            (Ok(Some(_)), Ok(_)) => {
                last_observation = "active binding row still present".to_string();
                break;
            }
            (Ok(None), Ok(Some(_))) => {
                last_observation = "conflicting revoke receipt present".to_string();
                break;
            }
            (Ok(None), Ok(None)) => {
                last_observation = "active row absent but revoke receipt missing".to_string();
            }
            (Err(error), _) => {
                last_observation = format!("active-row reread failed: {error}");
            }
            (_, Err(error)) => {
                last_observation = format!("receipt reread failed: {error}");
            }
        }
        if attempt < 2 {
            tokio::task::yield_now().await;
        }
    }
    Err(MobHostActorError::StoreDiverged {
        detail: format!(
            "{context} for mob '{mob_id}' did not converge to an absent active row plus exact durable revoke receipt ({last_observation})"
        ),
    })
}

/// Serve one `BindHost` admission against the authority + durable rows.
///
/// Persist-before-commit: a fresh accept writes the R8 row under the
/// transition witness BEFORE the prepared authority commits; any persistence
/// failure drops the prepared state (in-memory truth never advances past
/// durable truth). Token consumption + re-mint happens only on a FRESH
/// accept (DEC-P2-4); a replay never touches the slot.
pub async fn serve_host_bind(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    token: &mut HostBootstrapTokenSlot,
    observations: HostBindObservations,
) -> Result<HostBindServeOutcome, MobHostActorError> {
    let mob = AuthorityMobId::from(observations.mob_id.clone());
    let was_bound = authority.state().binding_phases.contains_key(&mob);

    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::ResolveHostBind {
            mob_id: mob.clone(),
            supervisor_peer_id: AuthorityPeerId::from(observations.supervisor.peer_id.as_str()),
            supervisor_signing_key: AuthorityPeerSigningKey::from(
                *observations.supervisor.pubkey.as_bytes(),
            ),
            epoch: observations.epoch,
            binding_generation: observations.binding_generation,
            sender_matches_supervisor: observations.sender_matches_supervisor,
            address_matches: observations.address_matches,
            token_valid: observations.token_valid,
        },
    )
    .map_err(|err| MobHostActorError::Machine {
        detail: err.to_string(),
    })?;

    match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::HostBindRejected { cause, .. } => {
            // No state change, no token consumption, no persistence.
            Ok(HostBindServeOutcome::Rejected {
                cause: bridge_rejection_cause(*cause),
                reason: format!("bind host rejected: {cause:?}"),
            })
        }
        MobHostBindingAuthorityEffect::HostBindAccepted { .. } => {
            let witness = HostBindingPersistenceAuthority::from_transition(&mob, &transition)?;
            if was_bound {
                // Idempotent replay: the durable row must already carry the
                // recorded binding — divergence is a typed fault, never a
                // silent overwrite. Member regions ride the stored row and
                // are never touched by the bind ceremony.
                let stored = persistence.load(&observations.mob_id).await?;
                let record = record_from_authority_state(prepared.state(), &mob, stored.as_ref())?;
                if stored.as_ref() != Some(&record) {
                    return Err(MobHostActorError::StoreDiverged {
                        detail: format!(
                            "replayed bind for mob '{}' does not match the durable row",
                            observations.mob_id
                        ),
                    });
                }
            } else {
                let mut record = record_from_authority_state(prepared.state(), &mob, None)?;
                record.accepted_capabilities = Some(observations.accepted_capabilities.clone());
                let witness =
                    witness.with_accepted_capabilities(record.accepted_capabilities.clone());
                match persistence
                    .put_if_absent(&observations.mob_id, &record, &witness)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        // A prior attempt may have committed this exact row
                        // and lost its terminal before the prepared machine
                        // state committed. Durable equality is sufficient to
                        // finish that SAME generation; a different row is a
                        // hard authority conflict.
                        let stored = persistence.load(&observations.mob_id).await?;
                        if stored.as_ref() != Some(&record) {
                            return Err(MobHostActorError::StoreDiverged {
                                detail: format!(
                                    "durable host binding row already exists with a different authority for mob '{}'",
                                    observations.mob_id
                                ),
                            });
                        }
                    }
                    Err(write_error) => {
                        // A store can commit and still lose the completion
                        // acknowledgement. Boundedly consult durable truth
                        // before deciding this ceremony failed. Exact equality
                        // converges in this live actor; absence, a conflicting
                        // row, or repeated read failure is durable uncertainty
                        // and the actor shell fail-stops until restart.
                        let write_error = write_error.to_string();
                        let mut exact = false;
                        let mut last_observation = "row absent".to_string();
                        for attempt in 0..3 {
                            match persistence.load(&observations.mob_id).await {
                                Ok(Some(stored)) if stored == record => {
                                    exact = true;
                                    break;
                                }
                                Ok(Some(_)) => {
                                    last_observation = "conflicting row present".to_string();
                                    break;
                                }
                                Ok(None) => {
                                    last_observation = "row absent".to_string();
                                }
                                Err(error) => {
                                    last_observation = format!("reread failed: {error}");
                                }
                            }
                            if attempt < 2 {
                                tokio::task::yield_now().await;
                            }
                        }
                        if !exact {
                            return Err(MobHostActorError::StoreDiverged {
                                detail: format!(
                                    "bind write for mob '{}' returned an ambiguous error ({write_error}); bounded durable reread did not prove the exact row ({last_observation})",
                                    observations.mob_id
                                ),
                            });
                        }
                    }
                }
            }
            authority
                .commit_prepared_authority(prepared)
                .map_err(|err| MobHostActorError::Machine {
                    detail: format!("prepared bind commit failed: {err:?}"),
                })?;
            let fresh = !was_bound;
            if fresh {
                token.consume_and_remint();
            }
            Ok(HostBindServeOutcome::Accepted {
                fresh,
                supervisor: observations.supervisor.into_trusted_peer_descriptor(),
            })
        }
        other => Err(MobHostActorError::Internal {
            detail: format!("unexpected effect for ResolveHostBind: {other:?}"),
        }),
    }
}

/// Serve one `RebindHost` admission (FLAG-2): strictly-monotonic epoch
/// advance, with the machine's replay arm idempotently re-acking the
/// recorded epoch so rotation retry converges. Same persist-before-commit
/// discipline as `serve_host_bind`; no token involvement.
pub async fn serve_host_rebind(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    observations: HostRebindObservations,
) -> Result<HostRebindServeOutcome, MobHostActorError> {
    let mob = AuthorityMobId::from(observations.mob_id.clone());
    // The CAS expected value is the DURABLE row (which carries the member
    // regions the rebind must never touch), not an in-memory reconstruction.
    let previous = if authority.state().binding_phases.contains_key(&mob) {
        persistence.load(&observations.mob_id).await?
    } else {
        None
    };

    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::ResolveHostRebind {
            mob_id: mob.clone(),
            supervisor_peer_id: AuthorityPeerId::from(observations.supervisor.peer_id.as_str()),
            supervisor_signing_key: AuthorityPeerSigningKey::from(
                *observations.supervisor.pubkey.as_bytes(),
            ),
            epoch: observations.epoch,
            binding_generation: observations.binding_generation,
            sender_matches_supervisor: observations.sender_matches_supervisor,
        },
    )
    .map_err(|err| MobHostActorError::Machine {
        detail: err.to_string(),
    })?;

    match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::HostBindRejected { cause, .. } => {
            Ok(HostRebindServeOutcome::Rejected {
                cause: bridge_rejection_cause(*cause),
                reason: format!("rebind host rejected: {cause:?}"),
            })
        }
        MobHostBindingAuthorityEffect::HostRebindAccepted { .. } => {
            let Some(expected) = previous else {
                return Err(MobHostActorError::Internal {
                    detail: format!(
                        "machine accepted rebind for unbound mob '{}'",
                        observations.mob_id
                    ),
                });
            };
            let mut record = record_from_authority_state(prepared.state(), &mob, Some(&expected))?;
            let binding_tuple_changed = expected.supervisor_peer_id != record.supervisor_peer_id
                || expected.supervisor_signing_key != record.supervisor_signing_key
                || expected.epoch != record.epoch
                || expected.binding_generation != record.binding_generation;
            if binding_tuple_changed {
                record.accepted_capabilities = Some(observations.accepted_capabilities.clone());
            }
            // Boot revival reconstructs each member-side supervisor trust
            // edge from these rows. Rebind therefore owns the canonical
            // endpoint refresh for every materialized member, including the
            // common same-key/same-peer address change on controller restart.
            // Carrying the new binding tuple with stale per-row routes would
            // revive members pointed at a dead endpoint.
            for row in record.materialized.values_mut() {
                row.supervisor_name = observations.supervisor.name.to_string();
                row.supervisor_address = observations.supervisor.address.to_string();
            }
            let witness = HostBindingPersistenceAuthority::from_transition(&mob, &transition)?
                .with_accepted_capabilities(record.accepted_capabilities.clone());
            if expected == record {
                // Idempotent replay ack: state unchanged; the durable row
                // must already match.
                let stored = persistence.load(&observations.mob_id).await?;
                if stored.as_ref() != Some(&record) {
                    return Err(MobHostActorError::StoreDiverged {
                        detail: format!(
                            "replayed rebind for mob '{}' does not match the durable row",
                            observations.mob_id
                        ),
                    });
                }
            } else {
                let write_outcome = persistence
                    .compare_and_put(&observations.mob_id, &expected, &record, &witness)
                    .await;
                match write_outcome {
                    Ok(true) => {}
                    Ok(false) => {
                        require_exact_binding_after_uncertain_write(
                            persistence,
                            &observations.mob_id,
                            &record,
                            "rebind CAS miss",
                        )
                        .await?;
                    }
                    Err(error) => {
                        require_exact_binding_after_uncertain_write(
                            persistence,
                            &observations.mob_id,
                            &record,
                            &format!("ambiguous rebind write error: {error}"),
                        )
                        .await?;
                    }
                }
            }
            authority
                .commit_prepared_authority(prepared)
                .map_err(|err| MobHostActorError::Machine {
                    detail: format!("prepared rebind commit failed: {err:?}"),
                })?;
            let previous_supervisor_peer_id = (expected.supervisor_peer_id
                != record.supervisor_peer_id)
                .then_some(expected.supervisor_peer_id);
            Ok(HostRebindServeOutcome::Accepted {
                supervisor: observations.supervisor.into_trusted_peer_descriptor(),
                previous_supervisor_peer_id,
            })
        }
        other => Err(MobHostActorError::Internal {
            detail: format!("unexpected effect for ResolveHostRebind: {other:?}"),
        }),
    }
}

/// Comms-free authenticated revoke core for binding-only fixtures/embedders.
/// Materialized rows require [`MobHostActor`]'s full cleanup choreography and
/// are rejected here. Exact receipt replay returns `Ok(true)`; a never-bound
/// mob returns `Ok(false)`.
pub async fn revoke_host_binding(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    mob_id: &str,
    sender_peer_id: &str,
    sender_signing_key: [u8; 32],
    epoch: u64,
    binding_generation: u64,
) -> Result<bool, MobHostActorError> {
    let mob = AuthorityMobId::from(mob_id.to_string());
    // Delete CAS expected value = the durable row (member regions included).
    let previous = if authority.state().binding_phases.contains_key(&mob) {
        persistence.load(mob_id).await?
    } else {
        None
    };

    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::RevokeHostBinding {
            mob_id: mob.clone(),
            sender_peer_id: AuthorityPeerId::from(sender_peer_id),
            sender_signing_key: AuthorityPeerSigningKey::from(sender_signing_key),
            epoch,
            binding_generation,
        },
    )
    .map_err(|err| MobHostActorError::Machine {
        detail: err.to_string(),
    })?;

    match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::HostBindRejected {
            cause: HostAdmissionRejectKind::NotBound,
            ..
        } => Ok(false),
        MobHostBindingAuthorityEffect::HostBindingRevokeReplayed { .. } => {
            let receipt = persistence.load_revocation(mob_id).await?.ok_or_else(|| {
                MobHostActorError::StoreDiverged {
                    detail: format!(
                        "machine replayed host revoke for mob '{mob_id}' without a durable receipt"
                    ),
                }
            })?;
            if receipt.supervisor_peer_id != sender_peer_id
                || receipt.supervisor_signing_key != sender_signing_key
                || receipt.epoch != epoch
                || receipt.binding_generation != binding_generation
            {
                return Err(MobHostActorError::StoreDiverged {
                    detail: format!(
                        "machine revoke receipt for mob '{mob_id}' diverges from durable tuple"
                    ),
                });
            }
            authority
                .commit_prepared_authority(prepared)
                .map_err(|err| MobHostActorError::Machine {
                    detail: format!("prepared revoke replay commit failed: {err:?}"),
                })?;
            Ok(true)
        }
        MobHostBindingAuthorityEffect::HostBindingRevoked { .. } => {
            let Some(expected) = previous else {
                return Err(MobHostActorError::Internal {
                    detail: format!("machine revoked unbound mob '{mob_id}'"),
                });
            };
            if !expected.materialized.is_empty() {
                return Err(MobHostActorError::Internal {
                    detail: format!(
                        "binding-only revoke helper cannot dispose {} materialized member(s) for mob '{mob_id}'",
                        expected.materialized.len()
                    ),
                });
            }
            let witness = HostBindingDeletionAuthority::from_transition(&mob, &transition)?;
            let receipt = MobHostRevocationReceipt {
                supervisor_peer_id: expected.supervisor_peer_id.clone(),
                supervisor_signing_key: expected.supervisor_signing_key,
                epoch: expected.epoch,
                binding_generation: expected.binding_generation,
                released_members: Vec::new(),
            };
            let write_outcome = persistence
                .revoke(mob_id, &expected, &receipt, &witness)
                .await;
            match write_outcome {
                Ok(true) => {}
                Ok(false) => {
                    require_exact_revocation_after_uncertain_write(
                        persistence,
                        mob_id,
                        &receipt,
                        "revoke CAS miss",
                    )
                    .await?;
                }
                Err(error) => {
                    require_exact_revocation_after_uncertain_write(
                        persistence,
                        mob_id,
                        &receipt,
                        &format!("ambiguous revoke write error: {error}"),
                    )
                    .await?;
                }
            }
            authority
                .commit_prepared_authority(prepared)
                .map_err(|err| MobHostActorError::Machine {
                    detail: format!("prepared revoke commit failed: {err:?}"),
                })?;
            Ok(true)
        }
        other => Err(MobHostActorError::Internal {
            detail: format!("unexpected effect for RevokeHostBinding: {other:?}"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Rung 0 — generic host-addressed command admission (§6.3, phase-3 P-2)
// ---------------------------------------------------------------------------

/// Shell observations for one host-addressed command's rung-0 admission.
/// Phase 3 always observes `turn_directive_present: false` (no host-addressed
/// command carries a turn directive; the member-drain delivery feed is phase
/// 6) with `turn_directive_supported` = the daemon's durable-sessions fact,
/// so the generated arm set is exercised end-to-end.
pub struct HostCommandObservations {
    pub mob_id: String,
    /// Canonical ingress identity (canonical peer id, or the id derived from
    /// the signed pubkey) — never a display name.
    pub sender_peer_id: String,
    pub epoch: u64,
    pub binding_generation: u64,
    pub turn_directive_present: bool,
    pub turn_directive_supported: bool,
}

/// Machine-adjudicated rung-0 outcome.
pub enum HostCommandAdmission {
    Admitted,
    Rejected {
        cause: BridgeRejectionCause,
        reason: String,
    },
}

/// Adjudicate one host-addressed command through the generated
/// `ResolveHostCommandAdmission` arms. Pure self-loop — apply-and-commit
/// immediately, no persistence.
pub fn resolve_host_command_admission(
    authority: &mut MobHostBindingAuthorityAuthority,
    observations: HostCommandObservations,
) -> Result<HostCommandAdmission, MobHostActorError> {
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::ResolveHostCommandAdmission {
            mob_id: AuthorityMobId::from(observations.mob_id.clone()),
            sender_peer_id: AuthorityPeerId::from(observations.sender_peer_id.as_str()),
            epoch: observations.epoch,
            binding_generation: observations.binding_generation,
            turn_directive_present: observations.turn_directive_present,
            turn_directive_supported: observations.turn_directive_supported,
        },
    )
    .map_err(|err| MobHostActorError::Machine {
        detail: err.to_string(),
    })?;
    let outcome = match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::HostCommandAdmitted { .. } => HostCommandAdmission::Admitted,
        MobHostBindingAuthorityEffect::HostCommandRejected { cause, .. } => {
            HostCommandAdmission::Rejected {
                cause: bridge_rejection_cause(*cause),
                reason: format!("host command rejected: {cause:?}"),
            }
        }
        other => {
            return Err(MobHostActorError::Internal {
                detail: format!("unexpected effect for ResolveHostCommandAdmission: {other:?}"),
            });
        }
    };
    authority
        .commit_prepared_authority(prepared)
        .map_err(|err| MobHostActorError::Machine {
            detail: format!("prepared command admission commit failed: {err:?}"),
        })?;
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Materialize admission / preflight / recording (comms-free serving core)
// ---------------------------------------------------------------------------

/// Machine-adjudicated materialize dedup outcome.
pub enum MaterializeAdmission {
    /// Fresh or superseding admit. `superseded_session_id` carries the
    /// PREVIOUS recorded session for the superseding arm (the A20 revival
    /// re-materialization) so the shell can dispose it before the new build
    /// — a live session the durable truth no longer names would otherwise
    /// leak until reconciliation.
    Admitted {
        superseded_session_id: Option<String>,
    },
    /// Idempotent replay at the recorded tuple + digest: the recorded
    /// `(session_id, spec_digest)` pair the durable row must corroborate.
    Replay {
        session_id: String,
        spec_digest: String,
    },
    Rejected {
        kind: MaterializeRejectKind,
    },
}

/// Adjudicate one materialize idempotency tuple. Self-loop; commit is
/// immediate (dedup memory records only successes, via
/// [`record_materialized_member`]).
pub fn resolve_materialize_admission(
    authority: &mut MobHostBindingAuthorityAuthority,
    member_key: &MemberKey,
    generation: u64,
    fence_token: u64,
    spec_digest: &str,
) -> Result<MaterializeAdmission, MobHostActorError> {
    let previously_recorded_session = authority
        .state()
        .materialized_sessions
        .get(member_key)
        .map(|session| session.0.clone());
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::ResolveMaterializeAdmission {
            member_key: member_key.clone(),
            generation: AuthorityGeneration(generation),
            fence_token: AuthorityFenceToken(fence_token),
            spec_digest: spec_digest.to_string(),
        },
    )
    .map_err(|err| MobHostActorError::Machine {
        detail: err.to_string(),
    })?;
    let outcome = match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::MaterializeAdmitted { .. } => {
            MaterializeAdmission::Admitted {
                superseded_session_id: previously_recorded_session,
            }
        }
        MobHostBindingAuthorityEffect::MaterializeReplay {
            session_id,
            spec_digest,
            ..
        } => MaterializeAdmission::Replay {
            session_id: session_id.0.clone(),
            spec_digest: spec_digest.clone(),
        },
        MobHostBindingAuthorityEffect::MaterializeRejected { cause, .. } => {
            MaterializeAdmission::Rejected { kind: *cause }
        }
        other => {
            return Err(MobHostActorError::Internal {
                detail: format!("unexpected effect for ResolveMaterializeAdmission: {other:?}"),
            });
        }
    };
    authority
        .commit_prepared_authority(prepared)
        .map_err(|err| MobHostActorError::Machine {
            detail: format!("prepared materialize admission commit failed: {err:?}"),
        })?;
    Ok(outcome)
}

/// Machine-adjudicated tier-2 preflight verdict (first-false-in-fixed-order
/// arms; the shell attaches the concrete offending detail when composing the
/// failure reply — the machine owns the verdict KIND).
pub enum MaterializePreflight {
    Admitted,
    Rejected { kind: MaterializeRejectKind },
}

pub fn resolve_materialize_preflight(
    authority: &mut MobHostBindingAuthorityAuthority,
    member_key: &MemberKey,
    generation: u64,
    fence_token: u64,
    observations: &crate::runtime::host_materialize::MaterializePreflightObservations,
) -> Result<MaterializePreflight, MobHostActorError> {
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::ResolveMaterializePreflight {
            member_key: member_key.clone(),
            generation: AuthorityGeneration(generation),
            fence_token: AuthorityFenceToken(fence_token),
            model_resolvable: observations.model_resolvable,
            binding_resolvable: observations.binding_resolvable,
            env_keys_present: observations.env_keys_present,
            stdio_commands_present: observations.stdio_commands_present,
            engine_protocol_supported: observations.engine_protocol_supported,
            durable_sessions_required: observations.durable_sessions_required,
            realm_backend_persistent: observations.realm_backend_persistent,
            memory_required: observations.memory_required,
            memory_capability: observations.memory_capability,
        },
    )
    .map_err(|err| MobHostActorError::Machine {
        detail: err.to_string(),
    })?;
    let outcome = match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::MaterializeAdmitted { .. } => MaterializePreflight::Admitted,
        MobHostBindingAuthorityEffect::MaterializeRejected { cause, .. } => {
            MaterializePreflight::Rejected { kind: *cause }
        }
        other => {
            return Err(MobHostActorError::Internal {
                detail: format!("unexpected effect for ResolveMaterializePreflight: {other:?}"),
            });
        }
    };
    authority
        .commit_prepared_authority(prepared)
        .map_err(|err| MobHostActorError::Machine {
            detail: format!("prepared materialize preflight commit failed: {err:?}"),
        })?;
    Ok(outcome)
}

/// Record one successful member build (DEC-P3H-9 ordering): prepare
/// `RecordMaterializedMember` → require the recording effect → durable
/// member-region CAS under the transition witness → commit. A persist failure
/// drops the prepared authority; the caller quiesces the exact volatile
/// incarnation while preserving its durable session for explicit resume.
/// Returns the identity witness that gates acceptor registration.
///
/// `forked_participant_attachment` is threaded as its own parameter rather
/// than pre-installed on `row` so the association cannot be forgotten by a
/// caller that assembled the row elsewhere: it lands in the SAME durable CAS
/// as the materialized row, and that same CAS discharges any pre-materialize
/// reconciliation obligation the association had accrued.
pub async fn record_materialized_member(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    row: MaterializedMemberRow,
    forked_participant_attachment: Option<ForkedParticipantAttachmentAssociation>,
) -> Result<MaterializedIdentityWitness, MobHostActorError> {
    let mut row = row;
    if row.forked_participant_attachment.is_some()
        && row.forked_participant_attachment != forked_participant_attachment
    {
        return Err(MobHostActorError::Internal {
            detail: "materialized row carries a capability association that disagrees with the \
                     admitted attachment"
                .to_string(),
        });
    }
    row.forked_participant_attachment = forked_participant_attachment;
    let mob_id = member_key.mob_id.0.clone();
    let identity = member_key.agent_identity.0.clone();
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::RecordMaterializedMember {
            member_key: member_key.clone(),
            generation: AuthorityGeneration(row.generation),
            fence_token: AuthorityFenceToken(row.fence_token),
            session_id: AuthoritySessionId(row.session_id.clone()),
            spec_digest: row.spec_digest.clone(),
        },
    )
    .map_err(|err| MobHostActorError::Machine {
        detail: err.to_string(),
    })?;
    match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::MaterializedMemberRecorded { .. } => {}
        other => {
            return Err(MobHostActorError::Internal {
                detail: format!("unexpected effect for RecordMaterializedMember: {other:?}"),
            });
        }
    }
    let row_witness =
        MemberRowPersistenceAuthority::from_materialized_transition(member_key, &transition)?;
    let identity_witness = MaterializedIdentityWitness::from_transition(member_key, &transition)?;

    let Some(expected) = persistence.load(&mob_id).await? else {
        return Err(MobHostActorError::StoreDiverged {
            detail: format!("no durable host binding row for bound mob '{mob_id}'"),
        });
    };
    let mut next = expected.clone();
    // A superseding generation retires the previous runtime's completion
    // authority in the same CAS as the new materialized row. Same-generation
    // fence refreshes keep their exact input-id dedup rows.
    if let Some(rows) = next.turn_outcomes.get_mut(&identity) {
        rows.retain(|outcome| {
            outcome.generation == row.generation && outcome.fence_token == row.fence_token
        });
        if rows.is_empty() {
            next.turn_outcomes.remove(&identity);
        }
    }
    if let Some(rows) = next.turn_outcome_pending.get_mut(&identity) {
        rows.retain(|pending| {
            pending.generation == row.generation && pending.fence_token == row.fence_token
        });
        if rows.is_empty() {
            next.turn_outcome_pending.remove(&identity);
        }
    }
    if let Some(rows) = next.turn_outcome_acknowledged.get_mut(&identity) {
        rows.retain(|acknowledged| {
            acknowledged.generation == row.generation && acknowledged.fence_token == row.fence_token
        });
        if rows.is_empty() {
            next.turn_outcome_acknowledged.remove(&identity);
        }
    }
    if let Some(rows) = next.tracked_input_cancellations.get_mut(&identity) {
        rows.retain(|cancel| {
            cancel.generation == row.generation && cancel.fence_token == row.fence_token
        });
        if rows.is_empty() {
            next.tracked_input_cancellations.remove(&identity);
        }
    }
    // §15.7: the stored spec replaces atomically WITH its dedup row (one
    // CAS blob); a superseding record overwrites the previous row in place.
    // The association rides the same blob, and adopting it discharges the
    // pre-materialize obligation the same attachment may have accrued.
    if let Some(association) = row.forked_participant_attachment.as_ref() {
        next.forked_participant_obligations
            .remove(&association.association_key());
    }
    next.materialized.insert(identity, row);
    let write_outcome = persistence
        .compare_and_put_member_rows(&mob_id, &expected, &next, &row_witness)
        .await;
    reconcile_exact_member_region_cas(
        persistence,
        &mob_id,
        &expected,
        &next,
        write_outcome,
        "materialized-member row write",
    )
    .await?;
    authority
        .commit_prepared_authority(prepared)
        .map_err(|err| MobHostActorError::DurableUncertainty {
            detail: format!(
                "materialized-member row is durably committed for mob '{mob_id}', but its prepared authority commit failed: {err:?}"
            ),
        })?;
    Ok(identity_witness)
}

// ---------------------------------------------------------------------------
// Release admission / recording (comms-free serving core, §19.L3)
// ---------------------------------------------------------------------------

/// Machine-adjudicated release admission outcome.
pub enum ReleaseAdmission {
    Admitted,
    /// Replay at the released tuple: the recorded disposal to re-ack, plus
    /// the transition-derived witness gating idempotent deregistration.
    Replay {
        disposal: MachineMemberSessionDisposal,
        witness: ReleasedIdentityWitness,
    },
    Rejected {
        kind: HostAdmissionRejectKind,
    },
}

pub fn resolve_release_admission(
    authority: &mut MobHostBindingAuthorityAuthority,
    member_key: &MemberKey,
    generation: u64,
    fence_token: u64,
) -> Result<ReleaseAdmission, MobHostActorError> {
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::ResolveReleaseAdmission {
            member_key: member_key.clone(),
            generation: AuthorityGeneration(generation),
            fence_token: AuthorityFenceToken(fence_token),
        },
    )
    .map_err(|err| MobHostActorError::Machine {
        detail: err.to_string(),
    })?;
    let outcome = match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::ReleaseAdmitted { .. } => ReleaseAdmission::Admitted,
        MobHostBindingAuthorityEffect::ReleaseReplay { disposal, .. } => ReleaseAdmission::Replay {
            disposal: *disposal,
            witness: ReleasedIdentityWitness::from_transition(member_key, &transition)?,
        },
        MobHostBindingAuthorityEffect::ReleaseRejected { cause, .. } => {
            ReleaseAdmission::Rejected { kind: *cause }
        }
        other => {
            return Err(MobHostActorError::Internal {
                detail: format!("unexpected effect for ResolveReleaseAdmission: {other:?}"),
            });
        }
    };
    authority
        .commit_prepared_authority(prepared)
        .map_err(|err| MobHostActorError::Machine {
            detail: format!("prepared release admission commit failed: {err:?}"),
        })?;
    Ok(outcome)
}

/// Record one performed disposal: prepare `RecordMemberRelease` → require
/// the recording effect → durable row-move CAS (materialized region entry
/// moves to the release region) under the transition witness → commit.
/// Returns the identity witness that gates acceptor deregistration.
pub async fn record_member_release(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    generation: u64,
    fence_token: u64,
    disposal: MachineMemberSessionDisposal,
) -> Result<(ReleasedIdentityWitness, String), MobHostActorError> {
    let mob_id = member_key.mob_id.0.clone();
    let identity = member_key.agent_identity.0.clone();
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::RecordMemberRelease {
            member_key: member_key.clone(),
            generation: AuthorityGeneration(generation),
            fence_token: AuthorityFenceToken(fence_token),
            disposal,
        },
    )
    .map_err(|err| MobHostActorError::Machine {
        detail: err.to_string(),
    })?;
    match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::MemberReleaseRecorded { .. } => {}
        other => {
            return Err(MobHostActorError::Internal {
                detail: format!("unexpected effect for RecordMemberRelease: {other:?}"),
            });
        }
    }
    let row_witness =
        MemberRowPersistenceAuthority::from_release_transition(member_key, &transition)?;
    let identity_witness = ReleasedIdentityWitness::from_transition(member_key, &transition)?;

    let Some(expected) = persistence.load(&mob_id).await? else {
        return Err(MobHostActorError::StoreDiverged {
            detail: format!("no durable host binding row for bound mob '{mob_id}'"),
        });
    };
    let Some(materialized_row) = expected.materialized.get(&identity).cloned() else {
        return Err(MobHostActorError::StoreDiverged {
            detail: format!(
                "durable row for mob '{mob_id}' lacks the materialized entry for '{identity}'"
            ),
        });
    };
    let member_pubkey = materialized_row.member_pubkey.clone();
    let mut next = expected.clone();
    next.materialized.remove(&identity);
    // No materialized owner remains after release, so no journal row for the
    // identity may remain visible or be revived by a late watcher.
    next.turn_outcomes.remove(&identity);
    next.turn_outcome_pending.remove(&identity);
    next.turn_outcome_acknowledged.remove(&identity);
    next.tracked_input_cancellations.remove(&identity);
    next.released.insert(
        identity,
        ReleasedMemberRow {
            generation,
            fence_token,
            disposal: RecordedDisposal::from_machine(disposal),
            member_pubkey: member_pubkey.clone(),
        },
    );
    let write_outcome = persistence
        .compare_and_put_member_rows(&mob_id, &expected, &next, &row_witness)
        .await;
    reconcile_exact_member_region_cas(
        persistence,
        &mob_id,
        &expected,
        &next,
        write_outcome,
        "member-release row move",
    )
    .await?;
    authority
        .commit_prepared_authority(prepared)
        .map_err(|err| MobHostActorError::DurableUncertainty {
            detail: format!(
                "member-release row move is durably committed for mob '{mob_id}', but its prepared authority commit failed: {err:?}"
            ),
        })?;
    Ok((identity_witness, member_pubkey))
}

/// Generated-transition witness for one durable Pending reservation mutation.
#[derive(Debug, Clone)]
pub enum TurnOutcomePendingPersistenceAuthority {
    Reserved {
        member_key: MemberKey,
        row: TurnOutcomePendingRow,
    },
    Canceled {
        member_key: MemberKey,
        generation: u64,
        fence_token: u64,
        input_id: String,
    },
}

impl TurnOutcomePendingPersistenceAuthority {
    fn reserved_from_transition(
        member_key: &MemberKey,
        row: &TurnOutcomePendingRow,
        transition: &MobHostBindingAuthorityTransition,
    ) -> Result<Self, MobHostActorError> {
        let matched = transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobHostBindingAuthorityEffect::TurnOutcomePendingReserved {
                    turn_key,
                    window_start,
                } if turn_key.mob_id == member_key.mob_id
                    && turn_key.agent_identity == member_key.agent_identity
                    && turn_key.generation == AuthorityGeneration(row.generation)
                    && turn_key.fence_token == AuthorityFenceToken(row.fence_token)
                    && turn_key.input_id == AuthorityInputId(row.input_id.clone())
                    && *window_start == row.window_start
            )
        });
        if !matched {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "transition carries no TurnOutcomePendingReserved fact for input '{}'",
                    row.input_id
                ),
            });
        }
        Ok(Self::Reserved {
            member_key: member_key.clone(),
            row: row.clone(),
        })
    }

    fn canceled_from_transition(
        member_key: &MemberKey,
        generation: u64,
        fence_token: u64,
        input_id: &str,
        transition: &MobHostBindingAuthorityTransition,
    ) -> Result<Self, MobHostActorError> {
        let matched = transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobHostBindingAuthorityEffect::TurnOutcomePendingCanceled { turn_key }
                    if turn_key.mob_id == member_key.mob_id
                        && turn_key.agent_identity == member_key.agent_identity
                        && turn_key.generation == AuthorityGeneration(generation)
                        && turn_key.fence_token == AuthorityFenceToken(fence_token)
                        && turn_key.input_id == AuthorityInputId(input_id.to_string())
            )
        });
        if !matched {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "transition carries no TurnOutcomePendingCanceled fact for input '{input_id}'"
                ),
            });
        }
        Ok(Self::Canceled {
            member_key: member_key.clone(),
            generation,
            fence_token,
            input_id: input_id.to_string(),
        })
    }

    pub fn verify_next(&self, next: &MobHostBindingRecord) -> Result<(), MobHostActorError> {
        match self {
            Self::Reserved { member_key, row } => {
                let carried = next
                    .turn_outcome_pending
                    .get(&member_key.agent_identity.0)
                    .is_some_and(|rows| rows.contains(row));
                let terminal = next
                    .turn_outcomes
                    .get(&member_key.agent_identity.0)
                    .is_some_and(|rows| {
                        rows.iter().any(|terminal| {
                            terminal.generation == row.generation
                                && terminal.fence_token == row.fence_token
                                && terminal.input_id == row.input_id
                        })
                    });
                let cancelled = next
                    .tracked_input_cancellations
                    .get(&member_key.agent_identity.0)
                    .is_some_and(|rows| {
                        rows.iter().any(|cancel| {
                            cancel.generation == row.generation
                                && cancel.fence_token == row.fence_token
                                && cancel.input_id == row.input_id
                        })
                    });
                let occupied = next
                    .turn_outcome_pending
                    .get(&member_key.agent_identity.0)
                    .map_or(0, Vec::len)
                    .saturating_add(
                        next.turn_outcomes
                            .get(&member_key.agent_identity.0)
                            .map_or(0, Vec::len),
                    );
                if !carried || terminal || cancelled || occupied > 256 {
                    return Err(MobHostActorError::Witness {
                        detail: format!(
                            "next record does not realize the bounded Pending reservation for input '{}'",
                            row.input_id
                        ),
                    });
                }
            }
            Self::Canceled {
                member_key,
                generation,
                fence_token,
                input_id,
            } => {
                let retained = next
                    .turn_outcome_pending
                    .get(&member_key.agent_identity.0)
                    .is_some_and(|rows| {
                        rows.iter().any(|row| {
                            row.generation == *generation
                                && row.fence_token == *fence_token
                                && row.input_id == *input_id
                        })
                    });
                if retained {
                    return Err(MobHostActorError::Witness {
                        detail: format!(
                            "next record still carries canceled Pending input '{input_id}'"
                        ),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Result of pre-effect Pending reservation adjudication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcomePendingReservationDisposition {
    Reserved {
        window_start: u64,
    },
    Replayed {
        window_start: u64,
    },
    TerminalReplay,
    /// Replay-only arbitration found a fresh exact key. No prepared state or
    /// durable row was committed; the caller must complete fresh preflight.
    FreshRequired,
    JournalFull,
    Stale,
}

/// Result of proven-non-acceptance Pending cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcomePendingCancelDisposition {
    Canceled,
    Absent,
}

/// Typed write permit for the turn-outcome journal region, constructible
/// only from a transition whose effects carry the matching
/// `TurnOutcomeRecorded` fact (the ADJ-15 witness pattern; ADJ-P6-6's
/// no-permit ruling covers the CONTROLLING-side pump cursors, not this
/// machine-fact region).
#[derive(Debug, Clone)]
pub struct TurnOutcomePersistenceAuthority {
    member_key: MemberKey,
    generation: u64,
    fence_token: u64,
    input_id: String,
    terminal_seq: u64,
}

impl TurnOutcomePersistenceAuthority {
    pub fn from_transition(
        member_key: &MemberKey,
        row: &TurnOutcomeRow,
        transition: &MobHostBindingAuthorityTransition,
    ) -> Result<Self, MobHostActorError> {
        let matched = transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobHostBindingAuthorityEffect::TurnOutcomeRecorded {
                    turn_key,
                    terminal_seq,
                    outcome,
                } if turn_key.mob_id == member_key.mob_id
                    && turn_key.agent_identity == member_key.agent_identity
                    && turn_key.generation == AuthorityGeneration(row.generation)
                    && turn_key.fence_token == AuthorityFenceToken(row.fence_token)
                    && turn_key.input_id == AuthorityInputId(row.input_id.clone())
                    && *terminal_seq == row.terminal_seq
                    && *outcome == row.machine_kind()
            )
        });
        if !matched {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "transition carries no TurnOutcomeRecorded fact for input '{}' of member \
                     '{}' in mob '{}'",
                    row.input_id, member_key.agent_identity.0, member_key.mob_id.0
                ),
            });
        }
        Ok(Self {
            member_key: member_key.clone(),
            generation: row.generation,
            fence_token: row.fence_token,
            input_id: row.input_id.clone(),
            terminal_seq: row.terminal_seq,
        })
    }

    /// Verify the `next` record actually carries the witnessed row.
    pub fn verify_next(&self, next: &MobHostBindingRecord) -> Result<(), MobHostActorError> {
        let carried = next
            .turn_outcomes
            .get(&self.member_key.agent_identity.0)
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.input_id == self.input_id
                        && row.generation == self.generation
                        && row.fence_token == self.fence_token
                        && row.terminal_seq == self.terminal_seq
                })
            });
        if !carried {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "next record does not carry the witnessed turn-outcome row for input '{}'",
                    self.input_id
                ),
            });
        }
        let pending_retained = next
            .turn_outcome_pending
            .get(&self.member_key.agent_identity.0)
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.input_id == self.input_id
                        && row.generation == self.generation
                        && row.fence_token == self.fence_token
                })
            });
        if pending_retained {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "next record retained Pending alongside terminal input '{}'",
                    self.input_id
                ),
            });
        }
        let cancelled = next
            .tracked_input_cancellations
            .get(&self.member_key.agent_identity.0)
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.input_id == self.input_id
                        && row.generation == self.generation
                        && row.fence_token == self.fence_token
                })
            });
        if cancelled {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "next record carried terminal alongside cancelled input '{}'",
                    self.input_id
                ),
            });
        }
        Ok(())
    }
}

/// Typed write permit for pruning one outcome row, constructible only from
/// the generated authority's exact acknowledgement effect.
#[derive(Debug, Clone)]
pub struct TurnOutcomeAckPersistenceAuthority {
    member_key: MemberKey,
    generation: u64,
    fence_token: u64,
    input_id: String,
}

impl TurnOutcomeAckPersistenceAuthority {
    pub fn from_transition(
        member_key: &MemberKey,
        ack: &BridgeTurnOutcomeAck,
        transition: &MobHostBindingAuthorityTransition,
    ) -> Result<Self, MobHostActorError> {
        let matched = transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobHostBindingAuthorityEffect::TurnOutcomeAcknowledged { turn_key }
                    if turn_key.mob_id == member_key.mob_id
                        && turn_key.agent_identity == member_key.agent_identity
                        && turn_key.generation == AuthorityGeneration(ack.generation)
                        && turn_key.fence_token == AuthorityFenceToken(ack.fence_token)
                        && turn_key.input_id == AuthorityInputId(ack.input_id.clone())
            )
        });
        if !matched {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "transition carries no TurnOutcomeAcknowledged fact for input '{}' of member \
                     '{}' in mob '{}'",
                    ack.input_id, member_key.agent_identity.0, member_key.mob_id.0
                ),
            });
        }
        Ok(Self {
            member_key: member_key.clone(),
            generation: ack.generation,
            fence_token: ack.fence_token,
            input_id: ack.input_id.clone(),
        })
    }

    pub fn verify_next(&self, next: &MobHostBindingRecord) -> Result<(), MobHostActorError> {
        let retained = next
            .turn_outcomes
            .get(&self.member_key.agent_identity.0)
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.generation == self.generation
                        && row.fence_token == self.fence_token
                        && row.input_id == self.input_id
                })
            });
        if retained {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "next record still carries acknowledged turn-outcome input '{}'",
                    self.input_id
                ),
            });
        }
        let acknowledged = next
            .turn_outcome_acknowledged
            .get(&self.member_key.agent_identity.0)
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.generation == self.generation
                        && row.fence_token == self.fence_token
                        && row.input_id == self.input_id
                })
            });
        if !acknowledged {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "next record lacks acknowledged turn-outcome tombstone for input '{}'",
                    self.input_id
                ),
            });
        }
        let cancelled = next
            .tracked_input_cancellations
            .get(&self.member_key.agent_identity.0)
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.generation == self.generation
                        && row.fence_token == self.fence_token
                        && row.input_id == self.input_id
                })
            });
        if cancelled {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "next record carried ACK and cancellation tombstones for input '{}'",
                    self.input_id
                ),
            });
        }
        Ok(())
    }
}

/// Transition-derived permit for installing or advancing one exact durable
/// tracked-input cancellation receipt.
#[derive(Debug, Clone)]
pub struct TrackedInputCancelPersistenceAuthority {
    member_key: MemberKey,
    input_id: String,
    generation: u64,
    fence_token: u64,
    expected_outcome: RecordedTrackedInputCancelKind,
}

impl TrackedInputCancelPersistenceAuthority {
    fn from_transition(
        member_key: &MemberKey,
        input_id: &str,
        generation: u64,
        fence_token: u64,
        transition: &MobHostBindingAuthorityTransition,
    ) -> Result<Self, MobHostActorError> {
        let expected_key = TurnKey::new(
            member_key.mob_id.clone(),
            member_key.agent_identity.clone(),
            AuthorityGeneration(generation),
            AuthorityFenceToken(fence_token),
            AuthorityInputId(input_id.to_string()),
        );
        let expected_outcome = transition.effects().iter().find_map(|effect| match effect {
            MobHostBindingAuthorityEffect::TrackedInputCancelNoEffect { turn_key }
                if turn_key == &expected_key =>
            {
                Some(RecordedTrackedInputCancelKind::NoEffect)
            }
            MobHostBindingAuthorityEffect::TrackedInputCancelRequested { turn_key }
                if turn_key == &expected_key =>
            {
                Some(RecordedTrackedInputCancelKind::Cancelling)
            }
            MobHostBindingAuthorityEffect::TrackedInputCancelCompleted { turn_key }
                if turn_key == &expected_key =>
            {
                Some(RecordedTrackedInputCancelKind::Cancelled)
            }
            _ => None,
        });
        let Some(expected_outcome) = expected_outcome else {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "transition carries no tracked-input cancellation mutation for input '{input_id}'"
                ),
            });
        };
        Ok(Self {
            member_key: member_key.clone(),
            input_id: input_id.to_string(),
            generation,
            fence_token,
            expected_outcome,
        })
    }

    pub fn verify_next(&self, next: &MobHostBindingRecord) -> Result<(), MobHostActorError> {
        let carried = next
            .tracked_input_cancellations
            .get(&self.member_key.agent_identity.0)
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.input_id == self.input_id
                        && row.generation == self.generation
                        && row.fence_token == self.fence_token
                        && row.outcome == self.expected_outcome
                })
            });
        let pending = next
            .turn_outcome_pending
            .get(&self.member_key.agent_identity.0)
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.input_id == self.input_id
                        && row.generation == self.generation
                        && row.fence_token == self.fence_token
                })
            });
        let terminal = next
            .turn_outcomes
            .get(&self.member_key.agent_identity.0)
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.input_id == self.input_id
                        && row.generation == self.generation
                        && row.fence_token == self.fence_token
                })
            });
        let acknowledged = next
            .turn_outcome_acknowledged
            .get(&self.member_key.agent_identity.0)
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.input_id == self.input_id
                        && row.generation == self.generation
                        && row.fence_token == self.fence_token
                })
            });
        if !carried || pending || terminal || acknowledged {
            return Err(MobHostActorError::Witness {
                detail: format!(
                    "next record does not realize tracked-input cancellation {:?} for input '{}'",
                    self.expected_outcome, self.input_id
                ),
            });
        }
        Ok(())
    }
}

/// Persist a Pending reservation before the runtime is allowed to accept the
/// directed-turn effect. Exact replay returns the original durable window;
/// capacity is machine-owned across Pending + terminal rows.
pub async fn reserve_turn_outcome_pending(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    row: &TurnOutcomePendingRow,
) -> Result<TurnOutcomePendingReservationDisposition, MobHostActorError> {
    reserve_turn_outcome_pending_with_mode(authority, persistence, member_key, row, true).await
}

/// Replay-only form used by the observation actor before fresh preflight. A
/// fresh exact key returns [`TurnOutcomePendingReservationDisposition::FreshRequired`]
/// without committing prepared machine state or persistence.
#[doc(hidden)]
pub async fn reserve_turn_outcome_pending_replay_only(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    row: &TurnOutcomePendingRow,
) -> Result<TurnOutcomePendingReservationDisposition, MobHostActorError> {
    reserve_turn_outcome_pending_with_mode(authority, persistence, member_key, row, false).await
}

/// Actor-linearized reservation with a replay-only mode. Applying the
/// generated input to a prepared authority lets the machine adjudicate the
/// exact key; a fresh effect is discarded before any durable write or
/// authority commit.
async fn reserve_turn_outcome_pending_with_mode(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    row: &TurnOutcomePendingRow,
    allow_fresh: bool,
) -> Result<TurnOutcomePendingReservationDisposition, MobHostActorError> {
    let mob_id = member_key.mob_id.0.clone();
    let identity = member_key.agent_identity.0.clone();
    let turn_key = TurnKey::new(
        member_key.mob_id.clone(),
        member_key.agent_identity.clone(),
        AuthorityGeneration(row.generation),
        AuthorityFenceToken(row.fence_token),
        AuthorityInputId(row.input_id.clone()),
    );
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::ReserveTurnOutcomePending {
            turn_key,
            window_start: row.window_start,
        },
    )
    .map_err(|error| MobHostActorError::Machine {
        detail: error.to_string(),
    })?;

    let disposition = match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::TurnOutcomePendingReserved { window_start, .. } => {
            if !allow_fresh {
                // `prepared` is intentionally dropped: replay arbitration
                // cannot mint Pending custody before fresh preflight.
                return Ok(TurnOutcomePendingReservationDisposition::FreshRequired);
            }
            let witness = TurnOutcomePendingPersistenceAuthority::reserved_from_transition(
                member_key,
                row,
                &transition,
            )?;
            let Some(expected) = persistence.load(&mob_id).await? else {
                return Err(MobHostActorError::StoreDiverged {
                    detail: format!("no durable host binding row for bound mob '{mob_id}'"),
                });
            };
            let mut next = expected.clone();
            next.turn_outcome_pending
                .entry(identity.clone())
                .or_default()
                .push(row.clone());
            let write_outcome = persistence
                .compare_and_put_turn_outcome_pending(&mob_id, &expected, &next, &witness)
                .await;
            reconcile_exact_member_region_cas(
                persistence,
                &mob_id,
                &expected,
                &next,
                write_outcome,
                "turn-outcome Pending reservation",
            )
            .await?;
            TurnOutcomePendingReservationDisposition::Reserved {
                window_start: *window_start,
            }
        }
        MobHostBindingAuthorityEffect::TurnOutcomePendingReplayed { window_start, .. } => {
            let durable = persistence.load(&mob_id).await?.ok_or_else(|| {
                MobHostActorError::StoreDiverged {
                    detail: format!("no durable host binding row for bound mob '{mob_id}'"),
                }
            })?;
            let replay_matches = durable
                .turn_outcome_pending
                .get(&identity)
                .is_some_and(|rows| {
                    rows.iter().any(|pending| {
                        pending.generation == row.generation
                            && pending.fence_token == row.fence_token
                            && pending.input_id == row.input_id
                            && pending.window_start == *window_start
                            && pending.bounded_result_spec == row.bounded_result_spec
                    })
                });
            if !replay_matches {
                return Err(MobHostActorError::StoreDiverged {
                    detail: format!(
                        "machine replayed Pending input '{}' without the matching durable row",
                        row.input_id
                    ),
                });
            }
            TurnOutcomePendingReservationDisposition::Replayed {
                window_start: *window_start,
            }
        }
        MobHostBindingAuthorityEffect::TurnOutcomePendingTerminalReplay { .. } => {
            let durable = persistence.load(&mob_id).await?.ok_or_else(|| {
                MobHostActorError::StoreDiverged {
                    detail: format!("no durable host binding row for bound mob '{mob_id}'"),
                }
            })?;
            let terminal_matches = durable.turn_outcomes.get(&identity).is_some_and(|rows| {
                rows.iter().any(|terminal| {
                    terminal.generation == row.generation
                        && terminal.fence_token == row.fence_token
                        && terminal.input_id == row.input_id
                        && match (&row.bounded_result_spec, &terminal.bounded_result) {
                            (None, None) => true,
                            (Some(spec), Some(result)) => {
                                result.result.label == spec.label
                                    && result.max_text_bytes == spec.max_text_bytes
                            }
                            _ => false,
                        }
                })
            });
            let acknowledged_matches = durable
                .turn_outcome_acknowledged
                .get(&identity)
                .is_some_and(|rows| {
                    rows.iter().any(|acknowledged| {
                        acknowledged.generation == row.generation
                            && acknowledged.fence_token == row.fence_token
                            && acknowledged.input_id == row.input_id
                    })
                });
            let cancelled_matches = durable
                .tracked_input_cancellations
                .get(&identity)
                .is_some_and(|rows| {
                    rows.iter().any(|cancel| {
                        cancel.generation == row.generation
                            && cancel.fence_token == row.fence_token
                            && cancel.input_id == row.input_id
                    })
                });
            if !terminal_matches && !acknowledged_matches && !cancelled_matches {
                return Err(MobHostActorError::StoreDiverged {
                    detail: format!(
                        "machine replayed terminal/acknowledged/cancelled input '{}' without matching durable authority",
                        row.input_id
                    ),
                });
            }
            TurnOutcomePendingReservationDisposition::TerminalReplay
        }
        MobHostBindingAuthorityEffect::TurnOutcomePendingJournalFull { .. } => {
            TurnOutcomePendingReservationDisposition::JournalFull
        }
        MobHostBindingAuthorityEffect::TurnOutcomePendingStale { .. } => {
            TurnOutcomePendingReservationDisposition::Stale
        }
        other => {
            return Err(MobHostActorError::Internal {
                detail: format!("unexpected effect for ReserveTurnOutcomePending: {other:?}"),
            });
        }
    };
    authority
        .commit_prepared_authority(prepared)
        .map_err(|error| {
            if matches!(
                disposition,
                TurnOutcomePendingReservationDisposition::Reserved { .. }
            ) {
                MobHostActorError::DurableUncertainty {
                    detail: format!(
                        "turn-outcome Pending reservation is durably committed for mob '{mob_id}', but its prepared authority commit failed: {error:?}"
                    ),
                }
            } else {
                MobHostActorError::Machine {
                    detail: format!("prepared Pending reservation commit failed: {error:?}"),
                }
            }
        })?;
    Ok(disposition)
}

/// Remove Pending only after runtime non-acceptance is proven. Ambiguous
/// errors intentionally skip this operation so restart/replay can converge.
pub async fn cancel_turn_outcome_pending(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    generation: u64,
    fence_token: u64,
    input_id: &str,
) -> Result<TurnOutcomePendingCancelDisposition, MobHostActorError> {
    let mob_id = member_key.mob_id.0.clone();
    let identity = member_key.agent_identity.0.clone();
    let turn_key = TurnKey::new(
        member_key.mob_id.clone(),
        member_key.agent_identity.clone(),
        AuthorityGeneration(generation),
        AuthorityFenceToken(fence_token),
        AuthorityInputId(input_id.to_string()),
    );
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::CancelTurnOutcomePending { turn_key },
    )
    .map_err(|error| MobHostActorError::Machine {
        detail: error.to_string(),
    })?;
    let disposition = match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::TurnOutcomePendingCanceled { .. } => {
            let witness = TurnOutcomePendingPersistenceAuthority::canceled_from_transition(
                member_key,
                generation,
                fence_token,
                input_id,
                &transition,
            )?;
            let Some(expected) = persistence.load(&mob_id).await? else {
                return Err(MobHostActorError::StoreDiverged {
                    detail: format!("no durable host binding row for bound mob '{mob_id}'"),
                });
            };
            let mut next = expected.clone();
            if let Some(rows) = next.turn_outcome_pending.get_mut(&identity) {
                rows.retain(|row| {
                    !(row.generation == generation
                        && row.fence_token == fence_token
                        && row.input_id == input_id)
                });
                if rows.is_empty() {
                    next.turn_outcome_pending.remove(&identity);
                }
            }
            let write_outcome = persistence
                .compare_and_put_turn_outcome_pending(&mob_id, &expected, &next, &witness)
                .await;
            reconcile_exact_member_region_cas(
                persistence,
                &mob_id,
                &expected,
                &next,
                write_outcome,
                "turn-outcome Pending cancellation",
            )
            .await?;
            TurnOutcomePendingCancelDisposition::Canceled
        }
        MobHostBindingAuthorityEffect::TurnOutcomePendingCancelReplay { .. } => {
            TurnOutcomePendingCancelDisposition::Absent
        }
        other => {
            return Err(MobHostActorError::Internal {
                detail: format!("unexpected effect for CancelTurnOutcomePending: {other:?}"),
            });
        }
    };
    authority
        .commit_prepared_authority(prepared)
        .map_err(|error| {
            if disposition == TurnOutcomePendingCancelDisposition::Canceled {
                MobHostActorError::DurableUncertainty {
                    detail: format!(
                        "turn-outcome Pending cancellation is durably committed for mob '{mob_id}', but its prepared authority commit failed: {error:?}"
                    ),
                }
            } else {
                MobHostActorError::Machine {
                    detail: format!("prepared Pending cancel commit failed: {error:?}"),
                }
            }
        })?;
    Ok(disposition)
}

/// Outcome of one journal record submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcomeRecordDisposition {
    /// Fresh row recorded (machine `TurnOutcomeRecorded`).
    Recorded,
    /// Redelivery converged on the recorded row (machine
    /// `TurnOutcomeReplayed`); no persistence write.
    Replayed,
    /// The watcher belongs to a released or superseded generation. The
    /// machine committed an explicit no-op and persistence was not touched.
    DroppedStale,
    /// No Pending/terminal authority remains for a duplicate watcher (for
    /// example after another watcher recorded and the row was acknowledged).
    DroppedUnreserved,
}

/// Outcome of one exact terminal acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcomeAckDisposition {
    /// The matching machine and durable row were pruned.
    Pruned,
    /// No matching row existed. No tombstone was retained.
    Absent,
}

/// Actor-authority result for exact tracked-input cancellation. `Cancelling`
/// is intentionally internal: the bridge may reply only after runtime
/// quiescence advances it to `Cancelled`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackedInputCancelDisposition {
    NoEffect,
    Cancelling,
    Cancelled,
    Terminal(BridgeTurnOutcomeRecord),
    Stale,
    Unreserved,
}

/// Record one tracked-turn terminal into the journal: prepare
/// `RecordTurnOutcome` → require the recording/replay effect → durable
/// row write under the transition witness (fresh only) → commit
/// (DEC-P6E-17; the `record_member_release` choreography).
pub async fn record_turn_outcome_journal(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    row: &TurnOutcomeRow,
) -> Result<TurnOutcomeRecordDisposition, MobHostActorError> {
    let encoded_bytes = serde_json::to_vec(&row.to_wire())
        .map_err(|error| MobHostActorError::RecordSerde {
            detail: format!("turn outcome size validation failed: {error}"),
        })?
        .len();
    if encoded_bytes > meerkat_runtime::member_observation::MAX_TURN_OUTCOME_RECORD_BYTES {
        return Err(MobHostActorError::Internal {
            detail: format!(
                "turn outcome record is {encoded_bytes} bytes (maximum {})",
                meerkat_runtime::member_observation::MAX_TURN_OUTCOME_RECORD_BYTES
            ),
        });
    }
    let mob_id = member_key.mob_id.0.clone();
    let identity = member_key.agent_identity.0.clone();
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::RecordTurnOutcome {
            turn_key: TurnKey::new(
                member_key.mob_id.clone(),
                member_key.agent_identity.clone(),
                AuthorityGeneration(row.generation),
                AuthorityFenceToken(row.fence_token),
                AuthorityInputId(row.input_id.clone()),
            ),
            terminal_seq: row.terminal_seq,
            outcome: row.machine_kind(),
        },
    )
    .map_err(|err| MobHostActorError::Machine {
        detail: err.to_string(),
    })?;
    match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::TurnOutcomeRecorded { .. } => {}
        MobHostBindingAuthorityEffect::TurnOutcomeReplayed { .. } => {
            // Dedup convergence: the recorded row wins; nothing to persist.
            authority
                .commit_prepared_authority(prepared)
                .map_err(|err| MobHostActorError::Machine {
                    detail: format!("prepared turn-outcome replay commit failed: {err:?}"),
                })?;
            return Ok(TurnOutcomeRecordDisposition::Replayed);
        }
        MobHostBindingAuthorityEffect::TurnOutcomeStaleDropped { .. } => {
            authority
                .commit_prepared_authority(prepared)
                .map_err(|err| MobHostActorError::Machine {
                    detail: format!("prepared stale turn-outcome drop commit failed: {err:?}"),
                })?;
            return Ok(TurnOutcomeRecordDisposition::DroppedStale);
        }
        MobHostBindingAuthorityEffect::TurnOutcomeUnreservedDropped { .. } => {
            authority
                .commit_prepared_authority(prepared)
                .map_err(|err| MobHostActorError::Machine {
                    detail: format!("prepared unreserved turn-outcome drop commit failed: {err:?}"),
                })?;
            return Ok(TurnOutcomeRecordDisposition::DroppedUnreserved);
        }
        other => {
            return Err(MobHostActorError::Internal {
                detail: format!("unexpected effect for RecordTurnOutcome: {other:?}"),
            });
        }
    }
    let witness = TurnOutcomePersistenceAuthority::from_transition(member_key, row, &transition)?;

    let Some(expected) = persistence.load(&mob_id).await? else {
        return Err(MobHostActorError::StoreDiverged {
            detail: format!("no durable host binding row for bound mob '{mob_id}'"),
        });
    };
    let mut next = expected.clone();
    if let Some(pending) = next.turn_outcome_pending.get_mut(&identity) {
        pending.retain(|existing| {
            !(existing.input_id == row.input_id
                && existing.generation == row.generation
                && existing.fence_token == row.fence_token)
        });
        if pending.is_empty() {
            next.turn_outcome_pending.remove(&identity);
        }
    }
    let rows = next.turn_outcomes.entry(identity).or_default();
    rows.retain(|existing| {
        !(existing.input_id == row.input_id
            && existing.generation == row.generation
            && existing.fence_token == row.fence_token)
    });
    rows.push(row.clone());
    let write_outcome = persistence
        .compare_and_put_turn_outcomes(&mob_id, &expected, &next, &witness)
        .await;
    reconcile_exact_member_region_cas(
        persistence,
        &mob_id,
        &expected,
        &next,
        write_outcome,
        "turn-outcome journal record",
    )
    .await?;
    authority
        .commit_prepared_authority(prepared)
        .map_err(|err| MobHostActorError::DurableUncertainty {
            detail: format!(
                "turn-outcome journal row is durably committed for mob '{mob_id}', but its prepared authority commit failed: {err:?}"
            ),
        })?;
    Ok(TurnOutcomeRecordDisposition::Recorded)
}

/// Prune one explicitly acknowledged terminal row. Unknown acknowledgements
/// commit the generated no-op transition and retain no negative memory, so a
/// later journal commit for the same key remains admissible.
pub async fn acknowledge_turn_outcome_journal(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    ack: &BridgeTurnOutcomeAck,
) -> Result<TurnOutcomeAckDisposition, MobHostActorError> {
    let mob_id = member_key.mob_id.0.clone();
    let identity = member_key.agent_identity.0.clone();
    let turn_key = TurnKey::new(
        member_key.mob_id.clone(),
        member_key.agent_identity.clone(),
        AuthorityGeneration(ack.generation),
        AuthorityFenceToken(ack.fence_token),
        AuthorityInputId(ack.input_id.clone()),
    );
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::AcknowledgeTurnOutcome { turn_key },
    )
    .map_err(|error| MobHostActorError::Machine {
        detail: error.to_string(),
    })?;
    match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::TurnOutcomeAckReplay { .. } => {
            authority
                .commit_prepared_authority(prepared)
                .map_err(|error| MobHostActorError::Machine {
                    detail: format!("prepared turn-outcome ack replay commit failed: {error:?}"),
                })?;
            Ok(TurnOutcomeAckDisposition::Absent)
        }
        MobHostBindingAuthorityEffect::TurnOutcomeAcknowledged { .. } => {
            let witness =
                TurnOutcomeAckPersistenceAuthority::from_transition(member_key, ack, &transition)?;
            let Some(expected) = persistence.load(&mob_id).await? else {
                return Err(MobHostActorError::StoreDiverged {
                    detail: format!("no durable host binding row for bound mob '{mob_id}'"),
                });
            };
            let mut next = expected.clone();
            let Some(rows) = next.turn_outcomes.get_mut(&identity) else {
                return Err(MobHostActorError::StoreDiverged {
                    detail: format!(
                        "machine retained acknowledged input '{}' but durable mob '{}' has no outcome rows for '{}'",
                        ack.input_id, mob_id, identity
                    ),
                });
            };
            let before = rows.len();
            rows.retain(|row| {
                !(row.generation == ack.generation
                    && row.fence_token == ack.fence_token
                    && row.input_id == ack.input_id)
            });
            if rows.len() == before {
                return Err(MobHostActorError::StoreDiverged {
                    detail: format!(
                        "machine retained acknowledged input '{}' but its durable row is absent for mob '{}'",
                        ack.input_id, mob_id
                    ),
                });
            }
            let tombstones = next
                .turn_outcome_acknowledged
                .entry(identity.clone())
                .or_default();
            tombstones.retain(|row| {
                !(row.generation == ack.generation
                    && row.fence_token == ack.fence_token
                    && row.input_id == ack.input_id)
            });
            tombstones.push(TurnOutcomeAcknowledgedRow {
                input_id: ack.input_id.clone(),
                generation: ack.generation,
                fence_token: ack.fence_token,
            });
            if rows.is_empty() {
                next.turn_outcomes.remove(&identity);
            }
            let write_outcome = persistence
                .compare_and_put_turn_outcome_ack(&mob_id, &expected, &next, &witness)
                .await;
            reconcile_exact_member_region_cas(
                persistence,
                &mob_id,
                &expected,
                &next,
                write_outcome,
                "turn-outcome acknowledgement",
            )
            .await?;
            authority
                .commit_prepared_authority(prepared)
                .map_err(|error| MobHostActorError::DurableUncertainty {
                    detail: format!(
                        "turn-outcome acknowledgement is durably committed for mob '{mob_id}', but its prepared authority commit failed: {error:?}"
                    ),
                })?;
            Ok(TurnOutcomeAckDisposition::Pruned)
        }
        other => Err(MobHostActorError::Internal {
            detail: format!("unexpected effect for AcknowledgeTurnOutcome: {other:?}"),
        }),
    }
}

fn tracked_input_cancel_turn_key(
    member_key: &MemberKey,
    generation: u64,
    fence_token: u64,
    input_id: &str,
) -> TurnKey {
    TurnKey::new(
        member_key.mob_id.clone(),
        member_key.agent_identity.clone(),
        AuthorityGeneration(generation),
        AuthorityFenceToken(fence_token),
        AuthorityInputId(input_id.to_string()),
    )
}

// Persistence must receive the exact prepared transition and tracked-input
// tuple; aggregating them would obscure the CAS witness boundary.
#[allow(clippy::too_many_arguments)]
async fn persist_tracked_input_cancel_transition(
    authority: &mut MobHostBindingAuthorityAuthority,
    prepared: crate::machines::mob_host_binding_authority::MobHostBindingAuthorityPreparedAuthority,
    transition: &MobHostBindingAuthorityTransition,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    input_id: &str,
    generation: u64,
    fence_token: u64,
    outcome: RecordedTrackedInputCancelKind,
) -> Result<(), MobHostActorError> {
    let mob_id = member_key.mob_id.0.clone();
    let identity = member_key.agent_identity.0.clone();
    let witness = TrackedInputCancelPersistenceAuthority::from_transition(
        member_key,
        input_id,
        generation,
        fence_token,
        transition,
    )?;
    let Some(expected) = persistence.load(&mob_id).await? else {
        return Err(MobHostActorError::StoreDiverged {
            detail: format!("no durable host binding row for bound mob '{mob_id}'"),
        });
    };
    let mut next = expected.clone();
    if let Some(rows) = next.turn_outcome_pending.get_mut(&identity) {
        rows.retain(|row| {
            !(row.input_id == input_id
                && row.generation == generation
                && row.fence_token == fence_token)
        });
        if rows.is_empty() {
            next.turn_outcome_pending.remove(&identity);
        }
    }
    let rows = next
        .tracked_input_cancellations
        .entry(identity)
        .or_default();
    rows.retain(|row| {
        !(row.input_id == input_id
            && row.generation == generation
            && row.fence_token == fence_token)
    });
    rows.push(TrackedInputCancellationRow {
        input_id: input_id.to_string(),
        generation,
        fence_token,
        outcome,
    });
    let write_outcome = persistence
        .compare_and_put_tracked_input_cancel(&mob_id, &expected, &next, &witness)
        .await;
    reconcile_exact_member_region_cas(
        persistence,
        &mob_id,
        &expected,
        &next,
        write_outcome,
        "tracked-input cancellation",
    )
    .await?;
    authority
        .commit_prepared_authority(prepared)
        .map_err(|error| MobHostActorError::DurableUncertainty {
            detail: format!(
                "tracked-input cancellation is durably committed for mob '{mob_id}', but its prepared authority commit failed: {error:?}"
            ),
        })?;
    Ok(())
}

/// Install/replay one exact tracked-input cancellation decision. The caller
/// must hold the observation layer's exact-key admission mutex while making
/// the runtime-presence observation and through this transition.
pub async fn cancel_tracked_input_journal(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    input_id: &str,
    generation: u64,
    fence_token: u64,
    runtime_input_present: bool,
) -> Result<TrackedInputCancelDisposition, MobHostActorError> {
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::CancelTrackedInput {
            turn_key: tracked_input_cancel_turn_key(member_key, generation, fence_token, input_id),
            runtime_input_present,
        },
    )
    .map_err(|error| MobHostActorError::Machine {
        detail: error.to_string(),
    })?;
    match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::TrackedInputCancelNoEffect { .. } => {
            persist_tracked_input_cancel_transition(
                authority,
                prepared,
                &transition,
                persistence,
                member_key,
                input_id,
                generation,
                fence_token,
                RecordedTrackedInputCancelKind::NoEffect,
            )
            .await?;
            Ok(TrackedInputCancelDisposition::NoEffect)
        }
        MobHostBindingAuthorityEffect::TrackedInputCancelRequested { .. } => {
            persist_tracked_input_cancel_transition(
                authority,
                prepared,
                &transition,
                persistence,
                member_key,
                input_id,
                generation,
                fence_token,
                RecordedTrackedInputCancelKind::Cancelling,
            )
            .await?;
            Ok(TrackedInputCancelDisposition::Cancelling)
        }
        MobHostBindingAuthorityEffect::TrackedInputCancelReplay { outcome, .. } => {
            let disposition = match outcome {
                TrackedInputCancelKind::NoEffect => TrackedInputCancelDisposition::NoEffect,
                TrackedInputCancelKind::Cancelling => TrackedInputCancelDisposition::Cancelling,
                TrackedInputCancelKind::Cancelled => TrackedInputCancelDisposition::Cancelled,
            };
            authority
                .commit_prepared_authority(prepared)
                .map_err(|error| MobHostActorError::Machine {
                    detail: format!("prepared tracked-input cancel replay failed: {error:?}"),
                })?;
            Ok(disposition)
        }
        MobHostBindingAuthorityEffect::TrackedInputCancelTerminal {
            terminal_seq,
            outcome,
            ..
        } => {
            let durable = persistence
                .load(&member_key.mob_id.0)
                .await?
                .ok_or_else(|| MobHostActorError::StoreDiverged {
                    detail: format!(
                        "no durable host binding row for bound mob '{}'",
                        member_key.mob_id.0
                    ),
                })?;
            let row = durable
                .turn_outcomes
                .get(&member_key.agent_identity.0)
                .and_then(|rows| {
                    rows.iter().find(|row| {
                        row.input_id == input_id
                            && row.generation == generation
                            && row.fence_token == fence_token
                            && row.terminal_seq == *terminal_seq
                            && row.machine_kind() == *outcome
                    })
                })
                .ok_or_else(|| MobHostActorError::StoreDiverged {
                    detail: format!(
                        "machine terminal for tracked input '{input_id}' lacks its exact durable payload row"
                    ),
                })?
                .to_wire();
            authority
                .commit_prepared_authority(prepared)
                .map_err(|error| MobHostActorError::Machine {
                    detail: format!("prepared tracked-input terminal replay failed: {error:?}"),
                })?;
            Ok(TrackedInputCancelDisposition::Terminal(row))
        }
        MobHostBindingAuthorityEffect::TrackedInputCancelAcknowledgedReplay { .. } => {
            authority
                .commit_prepared_authority(prepared)
                .map_err(|error| MobHostActorError::Machine {
                    detail: format!("prepared acknowledged cancel replay failed: {error:?}"),
                })?;
            Ok(TrackedInputCancelDisposition::Cancelled)
        }
        MobHostBindingAuthorityEffect::TrackedInputCancelUnreserved { .. } => {
            authority
                .commit_prepared_authority(prepared)
                .map_err(|error| MobHostActorError::Machine {
                    detail: format!("prepared unreserved cancel classification failed: {error:?}"),
                })?;
            Ok(TrackedInputCancelDisposition::Unreserved)
        }
        MobHostBindingAuthorityEffect::TrackedInputCancelStale { .. } => {
            authority
                .commit_prepared_authority(prepared)
                .map_err(|error| MobHostActorError::Machine {
                    detail: format!("prepared stale cancel classification failed: {error:?}"),
                })?;
            Ok(TrackedInputCancelDisposition::Stale)
        }
        other => Err(MobHostActorError::Internal {
            detail: format!("unexpected effect for CancelTrackedInput: {other:?}"),
        }),
    }
}

/// Advance a durable `Cancelling` receipt only after the exact runtime input
/// is terminal/quiescent.
pub async fn complete_tracked_input_cancel_journal(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    input_id: &str,
    generation: u64,
    fence_token: u64,
) -> Result<TrackedInputCancelDisposition, MobHostActorError> {
    let mut prepared = authority.prepare_authority();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut prepared,
        MobHostBindingAuthorityInput::CompleteTrackedInputCancel {
            turn_key: tracked_input_cancel_turn_key(member_key, generation, fence_token, input_id),
        },
    )
    .map_err(|error| MobHostActorError::Machine {
        detail: error.to_string(),
    })?;
    match single_host_effect(&transition)? {
        MobHostBindingAuthorityEffect::TrackedInputCancelCompleted { .. } => {
            persist_tracked_input_cancel_transition(
                authority,
                prepared,
                &transition,
                persistence,
                member_key,
                input_id,
                generation,
                fence_token,
                RecordedTrackedInputCancelKind::Cancelled,
            )
            .await?;
            Ok(TrackedInputCancelDisposition::Cancelled)
        }
        MobHostBindingAuthorityEffect::TrackedInputCancelReplay { outcome, .. } => {
            let disposition = match outcome {
                TrackedInputCancelKind::NoEffect => TrackedInputCancelDisposition::NoEffect,
                TrackedInputCancelKind::Cancelling => TrackedInputCancelDisposition::Cancelling,
                TrackedInputCancelKind::Cancelled => TrackedInputCancelDisposition::Cancelled,
            };
            authority
                .commit_prepared_authority(prepared)
                .map_err(|error| MobHostActorError::Machine {
                    detail: format!("prepared completed-cancel replay failed: {error:?}"),
                })?;
            Ok(disposition)
        }
        MobHostBindingAuthorityEffect::TrackedInputCancelUnreserved { .. } => {
            authority
                .commit_prepared_authority(prepared)
                .map_err(|error| MobHostActorError::Machine {
                    detail: format!("prepared unreserved cancel completion failed: {error:?}"),
                })?;
            Ok(TrackedInputCancelDisposition::Unreserved)
        }
        MobHostBindingAuthorityEffect::TrackedInputCancelStale { .. } => {
            authority
                .commit_prepared_authority(prepared)
                .map_err(|error| MobHostActorError::Machine {
                    detail: format!("prepared stale cancel completion failed: {error:?}"),
                })?;
            Ok(TrackedInputCancelDisposition::Stale)
        }
        other => Err(MobHostActorError::Internal {
            detail: format!("unexpected effect for CompleteTrackedInputCancel: {other:?}"),
        }),
    }
}

/// Wire projection of a materialize dedup/preflight reject. The shell
/// attaches the first offending concrete detail (model / key / server name);
/// the machine owns the verdict kind. `MemoryStoreUnavailable` maps to
/// `Unavailable` (ADJ-20 — the remedy is rebind after capability
/// re-declaration; no dedicated wire cause exists).
pub(crate) fn materialize_reject_wire_cause(
    kind: MaterializeRejectKind,
    spec: &PortableMemberSpec,
    first_missing_env_key: Option<&str>,
    first_missing_stdio_server: Option<&str>,
) -> (BridgeRejectionCause, String) {
    match kind {
        MaterializeRejectKind::NotBound => (
            BridgeRejectionCause::NotBound,
            "materialize rejected: mob is not bound on this host".to_string(),
        ),
        MaterializeRejectKind::StaleFence => (
            BridgeRejectionCause::StaleFence,
            "materialize rejected: superseded (generation, fence_token) tuple".to_string(),
        ),
        MaterializeRejectKind::SpecDigestMismatch => (
            BridgeRejectionCause::SpecDigestMismatch,
            "materialize rejected: idempotency tuple replayed with a different spec digest"
                .to_string(),
        ),
        MaterializeRejectKind::ModelUnresolvable => (
            BridgeRejectionCause::ModelUnresolvable {
                model: spec.profile.model.clone(),
            },
            format!(
                "materialize rejected: model '{}' is unresolvable on this host",
                spec.profile.model
            ),
        ),
        MaterializeRejectKind::AuthBindingUnresolvable => {
            let (realm, binding) = spec
                .overlay
                .auth_binding
                .as_ref()
                .map(|binding| {
                    (
                        binding.realm.as_str().to_string(),
                        binding.binding.as_str().to_string(),
                    )
                })
                .unwrap_or_else(|| {
                    (
                        "env_default".to_string(),
                        spec.profile.provider.as_str().to_string(),
                    )
                });
            let reason = format!(
                "materialize rejected: auth binding '{realm}/{binding}' is unresolvable on this host"
            );
            (
                BridgeRejectionCause::AuthBindingUnresolvable { realm, binding },
                reason,
            )
        }
        MaterializeRejectKind::EnvKeysMissing => {
            let key = first_missing_env_key.unwrap_or("<unknown>").to_string();
            let reason =
                format!("materialize rejected: required env key '{key}' is absent on this host");
            (BridgeRejectionCause::EnvKeyMissing { key }, reason)
        }
        MaterializeRejectKind::McpCommandMissing => {
            let server = first_missing_stdio_server.unwrap_or("<unknown>").to_string();
            let reason = format!(
                "materialize rejected: MCP stdio command for server '{server}' is absent on this host"
            );
            (BridgeRejectionCause::McpCommandMissing { server }, reason)
        }
        MaterializeRejectKind::RealmBackendUnavailable => (
            BridgeRejectionCause::RealmBackendUnavailable,
            "materialize rejected: durable sessions required but the realm backend is not persistent"
                .to_string(),
        ),
        MaterializeRejectKind::MemoryStoreUnavailable => (
            BridgeRejectionCause::Unavailable,
            "memory store unavailable on host".to_string(),
        ),
        MaterializeRejectKind::EngineProtocolUnsupported => (
            BridgeRejectionCause::UnsupportedProtocolVersion,
            "materialize rejected: the command's protocol version does not support multi-host"
                .to_string(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Host comms runtime recipe (the MobSupervisorBridge non-session recipe,
// host-identity flavor; W2.1 step 5)
// ---------------------------------------------------------------------------

/// The daemon's host-identity comms runtime: keypair-owned, NO listener of
/// its own (ingress arrives through the host acceptor's demux), with the
/// machine-gated classification + peer request/response authority installed
/// so `BindHost` from a not-yet-trusted supervisor is admissible under the
/// supervisor-bridge intent auth exemption.
pub struct HostCommsRuntime {
    pub runtime: Arc<meerkat_comms::CommsRuntime>,
    pub dsl: Arc<meerkat_runtime::HandleDslAuthority>,
    pub inbox_sender: meerkat_comms::InboxSender,
}

pub fn build_host_comms_runtime(
    participant_name: &str,
    keypair: meerkat_comms::Keypair,
) -> Result<HostCommsRuntime, MobHostActorError> {
    let runtime = meerkat_comms::CommsRuntime::inproc_control_only_with_keypair_and_silent_intents(
        participant_name,
        None,
        keypair,
        Arc::new(std::collections::HashSet::new()),
    )
    .map_err(
        |err| match crate::error::comms_name_occupancy_holder(&err) {
            Some(holder_pubkey) => MobHostActorError::ParticipantNameOccupied {
                participant_name: participant_name.to_string(),
                holder_pubkey,
            },
            None => MobHostActorError::Comms {
                detail: format!(
                    "failed to construct host comms runtime '{participant_name}': {err}"
                ),
            },
        },
    )?;
    let runtime = Arc::new(runtime);

    let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
    dsl.apply_signal(
        mm_dsl::MeerkatMachineSignal::Initialize,
        "mob_host_actor::initialize",
    )
    .map_err(|err| MobHostActorError::Comms {
        detail: format!("failed to initialize host peer authority '{participant_name}': {err}"),
    })?;
    let session_id = mm_dsl::SessionId::from(participant_name.to_string());
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::RegisterSession {
            session_id: session_id.clone(),
            // Honestly epochless: this is a comms peer-projection authority for
            // the host participant, not a runtime session entry, so there is no
            // entry runtime epoch to register under.
            runtime_epoch_id: None,
        },
        "mob_host_actor::register",
    )
    .map_err(|err| MobHostActorError::Comms {
        detail: format!("failed to register host peer authority '{participant_name}': {err}"),
    })?;
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::EnsureSessionWithExecutor { session_id },
        "mob_host_actor::ensure_executor",
    )
    .map_err(|err| MobHostActorError::Comms {
        detail: format!("failed to attach host peer authority '{participant_name}': {err}"),
    })?;

    meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(
        Arc::clone(&dsl),
        runtime.as_ref(),
    )
    .map_err(|err| MobHostActorError::Comms {
        detail: format!("failed to install host peer-comms authority '{participant_name}': {err}"),
    })?;
    runtime.install_peer_request_response_authority(
        meerkat_comms::PeerRequestResponseAuthority::new(
            Arc::new(meerkat_runtime::RuntimePeerInteractionHandle::new(
                Arc::clone(&dsl),
            )),
            Arc::new(meerkat_runtime::RuntimeInteractionStreamHandle::new(
                Arc::clone(&dsl),
            )),
        ),
    );

    let inbox_sender = runtime.tool_material().router().inbox_sender().clone();
    Ok(HostCommsRuntime {
        runtime,
        dsl,
        inbox_sender,
    })
}

// ---------------------------------------------------------------------------
// The actor
// ---------------------------------------------------------------------------

/// Composition material for [`spawn_mob_host_actor`].
pub struct MobHostActorConfig {
    /// Host-identity comms runtime (from [`build_host_comms_runtime`]).
    pub host_runtime: Arc<meerkat_comms::CommsRuntime>,
    /// The host runtime's peer request/response machine authority.
    pub host_dsl: Arc<meerkat_runtime::HandleDslAuthority>,
    /// The host runtime's inbox sender (registered into the acceptor demux).
    pub host_inbox_sender: meerkat_comms::InboxSender,
    /// The host's Ed25519 signing keypair (acceptor ack signing).
    pub host_keypair: Arc<meerkat_comms::Keypair>,
    /// The acceptor identity registry the daemon composed.
    pub registry: Arc<meerkat_comms::HostAcceptorIdentityRegistry>,
    /// R8 typed persistence over the realm runtime store.
    pub persistence: Arc<dyn MobHostBindingPersistence>,
    /// Tier-1 provider presence probe (injected; DEC-P2-5).
    pub probe: Arc<dyn ProviderPresenceProbe>,
    /// Feature-compiled capability facts of the composing binary.
    pub capability_facts: HostCapabilityFacts,
    /// The acceptor's advertised address (descriptor `address`, bind-address
    /// observation source).
    pub advertised_address: String,
    /// Advertised live ws/wss base URL iff the live plane composed (DL5/DL6).
    pub live_endpoint: Option<String>,
    /// Serialized-descriptor watch slot shared with the acceptor's pairing
    /// branch (the daemon refreshes it on every token re-mint).
    pub descriptor_watch_tx: watch::Sender<String>,
    /// Durable descriptor writer (the daemon's 0600 file sink).
    pub descriptor_sink: Arc<dyn HostDescriptorSink>,
    /// Member-build substrate (DEC-P3H-2). `None` ⇒ `MaterializeMember` /
    /// `ReleaseMember` typed-reject `Unavailable` (bind-only composition).
    pub member_host: Option<HostMemberSubstrate>,
    /// Cadence of the actor's own capability maintenance pass.
    ///
    /// `None` uses [`FORKED_PARTICIPANT_SWEEP_INTERVAL`]. This is a composition
    /// input rather than a hidden constant so a harness can drive the REAL
    /// periodic arm on a short cadence instead of waiting out production
    /// timing — no extra task, no test-only loop, the same single `select!`
    /// arm either way.
    pub forked_participant_sweep_interval: Option<Duration>,
}

/// Running mob host actor (single-owner responder task).
pub struct MobHostActorHandle {
    runtime_incarnation: BridgeHostRuntimeIncarnation,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<()>,
    initial_revival_rx: watch::Receiver<bool>,
    observation_watch_rx: watch::Receiver<HostObservationProjection>,
    observation_pending_tx: mpsc::Sender<HostTurnOutcomePendingRequest>,
    observation_record_tx: mpsc::Sender<HostTurnOutcomeRecordRequest>,
    observation_ack_tx: mpsc::Sender<HostTurnOutcomeAckRequest>,
}

impl MobHostActorHandle {
    /// Exact once-per-actor-boot token shared by `HostStatus` and every
    /// successful member-events page served by this host process.
    #[must_use]
    pub const fn runtime_incarnation(&self) -> BridgeHostRuntimeIncarnation {
        self.runtime_incarnation
    }

    /// Wait until the actor has completed its initial recovered-member revival
    /// walk and published the first observation projection.
    ///
    /// This is a process-readiness barrier, not a claim that every durable
    /// member revived successfully. Per-member environmental failures remain
    /// represented by the host's unhealthy observation state. The watch wait
    /// is cancellation-safe and may be retried on the same handle.
    pub async fn wait_for_initial_revival(&mut self) -> Result<(), MobHostActorError> {
        loop {
            if *self.initial_revival_rx.borrow() {
                return Ok(());
            }
            self.initial_revival_rx
                .changed()
                .await
                .map_err(|_| MobHostActorError::Internal {
                    detail: "mob host actor exited before initial member revival completed"
                        .to_string(),
                })?;
        }
    }

    /// Stop the responder drain and wait for it to exit.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let _ = self.join.await;
    }

    /// Read side of the actor's observation projection (DEC-P6E-2): the
    /// daemon hands this to `HostMemberObservation`.
    #[must_use]
    pub fn observation_watch(&self) -> watch::Receiver<HostObservationProjection> {
        self.observation_watch_rx.clone()
    }

    /// Journal record channel for out-of-actor journal writers (test
    /// probes; the actor registers per-session journals itself).
    #[must_use]
    pub fn observation_record_sender(&self) -> mpsc::Sender<HostTurnOutcomeRecordRequest> {
        self.observation_record_tx.clone()
    }

    /// Durable Pending reserve/cancel channel for the observation server.
    #[must_use]
    pub fn observation_pending_sender(&self) -> mpsc::Sender<HostTurnOutcomePendingRequest> {
        self.observation_pending_tx.clone()
    }

    /// Exact outcome acknowledgement channel for the observation server.
    #[must_use]
    pub fn observation_ack_sender(&self) -> mpsc::Sender<HostTurnOutcomeAckRequest> {
        self.observation_ack_tx.clone()
    }
}

/// The daemon's authority holder + bridge responder. All fields are owned
/// exclusively by the responder task; there is no interior mutability.
pub struct MobHostActor {
    /// Fresh per actor/process boot. The controlling mob observes this through
    /// authenticated `HostStatus` and successful member-events pages, and
    /// uses a change to re-realize its own route intent after volatile member
    /// trust rows were lost.
    runtime_incarnation: BridgeHostRuntimeIncarnation,
    /// THE ownership-ledger anchor: the generated MobHostBindingAuthority is
    /// the sole owner of every host-side binding/materialize/release/turn
    /// fact; this shell only observes and realizes.
    binding_authority: MobHostBindingAuthorityAuthority,
    persistence: Arc<dyn MobHostBindingPersistence>,
    host_runtime: Arc<meerkat_comms::CommsRuntime>,
    host_comms: Arc<dyn CoreCommsRuntime>,
    host_dsl: Arc<meerkat_runtime::HandleDslAuthority>,
    bootstrap_token: HostBootstrapTokenSlot,
    /// Actor-lifetime secret for target-bound delegated council proofs. Unlike
    /// the operator bootstrap token, successful unrelated binds do not rotate
    /// it and invalidate already-issued independent grants.
    delegated_bind_key: String,
    capabilities: HostCapabilitiesComposer,
    capability_facts: HostCapabilityFacts,
    descriptor: DescriptorRefresher,
    /// Availability obligation created when a fresh bind has already consumed
    /// its one-time token but the replacement descriptor was not published.
    /// Retries always use `bootstrap_token.current()` and never re-mint.
    pending_descriptor_refresh: Option<PendingDescriptorRefresh>,
    advertised_address: String,
    /// The acceptor identity registry + the generated authority owner token
    /// that gates its mutations (DEC-P3H-7).
    registry: Arc<meerkat_comms::HostAcceptorIdentityRegistry>,
    registry_owner: Arc<dyn Any + Send + Sync>,
    /// Member-build substrate (DEC-P3H-2); `None` ⇒ bind-only serving.
    materializer: Option<HostMemberMaterializer>,
    /// Source-owner capability service, composed once from the exact host
    /// realm, host id, shared capability store, and session runtime.
    forked_participant_service: Option<ForkedParticipantService>,
    /// Recorded-but-unrevived members (per-row revival failures at boot, or
    /// dead sessions a replay ensure could not recompose). Shell health
    /// observation only — feeds `HostStatus.healthy: false` (ADJ-19), never
    /// a machine decision.
    unrevived: BTreeSet<(String, String)>,
    /// Retained capability-attachment reconciliation obligations this boot
    /// could not conservatively discharge, keyed
    /// `(mob_id, association_key)`. Shell health observation only — feeds
    /// `HostStatus.healthy: false`, never a machine decision.
    forked_participant_debts: BTreeSet<(String, String)>,
    /// Dogma-#13 watch projection of the observation facts the member
    /// drain's serving arms need (session → mob/identity/generation +
    /// retained journal rows). The actor is the sole writer; the daemon's
    /// `HostMemberObservation` holds the read side.
    observation_watch_tx: watch::Sender<HostObservationProjection>,
    /// Journal record channel sender, cloned into every
    /// [`HostTrackedTurnJournal`] this actor registers.
    observation_record_tx: mpsc::Sender<HostTurnOutcomeRecordRequest>,
    /// In-memory mirror of the persisted `turn_outcomes` record region
    /// (wire outcomes verbatim), keyed `(mob_id, agent_identity)`. Loaded
    /// at recovery, updated only under the same witnesses that write the
    /// durable region — a projection of the row, never machine truth.
    turn_outcome_rows: BTreeMap<(String, String), Vec<TurnOutcomeRow>>,
    /// Durable materialization-generation event floors, projected from the
    /// materialized row region under the same witness-gated writes.
    generation_start_seqs: BTreeMap<(String, String), u64>,
    /// Host-materialized sessions currently carrying an exact incarnation
    /// registration on the member runtime adapter. Durable hosts additionally
    /// register the tracked-turn journal capability.
    registered_member_incarnations: HashMap<
        meerkat_core::types::SessionId,
        meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    >,
    /// Sticky fail-stop after a bind persistence outcome that durable reread
    /// could not classify. Serving any later command in that live process
    /// could act on a machine snapshot that lags the durable binding row;
    /// restart recovery is the only authority-restoring boundary.
    durable_uncertainty_fail_stop: Option<String>,
}

/// Prepare one validated durable snapshot, publish the initial descriptor,
/// install the recovered authority owner + host identity on the acceptor
/// demux, and transfer revival plus bridge serving to one owned task.
///
/// The descriptor/registry operations are the startup commit boundary. Every
/// fallible store read, recovered-state validation, descriptor serialization,
/// host-id lookup, and revival-input conversion happens before that boundary.
/// In particular, descriptor-sink failure cannot strand a generated owner in
/// the once-installed registry and make an otherwise valid retry impossible.
/// After the commit there is no await point in this constructor: the actor is
/// synchronously transferred into its responder task, so cancelling the
/// caller cannot strand a published descriptor without an owner task.
pub async fn spawn_mob_host_actor(
    config: MobHostActorConfig,
) -> Result<MobHostActorHandle, MobHostActorError> {
    let MobHostActorConfig {
        host_runtime,
        host_dsl,
        host_inbox_sender,
        host_keypair,
        registry,
        persistence,
        probe,
        capability_facts,
        advertised_address,
        live_endpoint,
        descriptor_watch_tx,
        descriptor_sink,
        member_host,
        forked_participant_sweep_interval,
    } = config;

    if host_runtime.public_key() != host_keypair.public_key() {
        return Err(MobHostActorError::Internal {
            detail: "mob host runtime identity does not match acceptor signing keypair".to_string(),
        });
    }

    // PREPARE: load every durable startup fact exactly once and derive all
    // fallible recovery inputs while the authority is still process-private.
    let records = persistence.list_records().await?;
    let revocations = persistence.list_revocations().await?;
    let binding_authority = recover_binding_authority_from_snapshot(&records, &revocations)?;

    let host_comms: Arc<dyn CoreCommsRuntime> = Arc::clone(&host_runtime) as _;
    let member_substrate_configured = member_host.is_some();
    let revival_host_id = if member_substrate_configured {
        Some(
            host_comms
                .peer_id()
                .ok_or_else(|| MobHostActorError::Internal {
                    detail: "mob host revival: host runtime peer_id unavailable".to_string(),
                })?
                .to_string(),
        )
    } else {
        None
    };
    let forked_participant_service = match (
        member_host
            .as_ref()
            .and_then(|substrate| substrate.forked_participant_realm.clone()),
        member_host
            .as_ref()
            .and_then(|substrate| substrate.forked_participant_store.clone()),
        member_host
            .as_ref()
            .and_then(|substrate| substrate.forked_participant_source_runtime.clone()),
        revival_host_id.as_deref(),
    ) {
        (Some(realm_id), Some(store), Some(runtime), Some(host_id)) => Some(
            ForkedParticipantService::new(
                ForkedParticipantOwnerRoute::Host {
                    realm_id,
                    host_id: crate::machines::mob_machine::HostId::from(host_id.to_string()),
                },
                store,
                runtime,
            )
            .map_err(|error| MobHostActorError::Internal {
                detail: format!("forked participant host service composition failed: {error}"),
            })?,
        ),
        (None, None, None, _) => None,
        _ => {
            return Err(MobHostActorError::Internal {
                detail: "forked participant host composition requires realm, store, and source runtime together".to_string(),
            });
        }
    };
    let prepared_recovered_members = prepare_recovered_members(
        &records,
        binding_authority.state(),
        member_substrate_configured,
    )?;
    // Capability associations are route-scoped facts, so they are validated
    // against the composed service (or its absence) rather than inside the
    // route-blind row fold. Startup is still process-private here: an
    // unroutable association aborts before anything is published.
    validate_recovered_forked_participant_routes(&records, forked_participant_service.as_ref())?;

    // The registry owner IS the generated authority's owner token: acceptor
    // identity mutations are machine-owner-gated, mirroring the comms trust
    // owner split (typed witness mob-side, Arc-ptr identity comms-side).
    let registry_owner: Arc<dyn Any + Send + Sync> =
        binding_authority.generated_authority_owner_token();

    let bootstrap_token = HostBootstrapTokenSlot::mint();
    let identity = meerkat_contracts::WireTrustedPeerIdentity::Ed25519PublicKey {
        public_key: host_keypair.public_key().to_pubkey_string(),
    };
    let advertised_address = canonicalize_bridge_address(&advertised_address);
    let descriptor = DescriptorRefresher::new(
        advertised_address.clone(),
        identity,
        live_endpoint,
        descriptor_watch_tx,
        descriptor_sink,
    );
    let prepared_descriptor = descriptor.prepare(bootstrap_token.current())?;

    let capabilities = HostCapabilitiesComposer::new(probe, capability_facts);

    // Observation plumbing (DEC-P6E-2): the projection watch + the journal
    // record channel exist for the actor's whole life; the daemon reads
    // them off the returned handle.
    let (observation_watch_tx, observation_watch_rx) =
        watch::channel(HostObservationProjection::default());
    let (observation_pending_tx, observation_pending_rx) =
        mpsc::channel::<HostTurnOutcomePendingRequest>(HOST_OBSERVATION_DRAIN_BATCH_LIMIT);
    let (observation_record_tx, observation_record_rx) =
        mpsc::channel::<HostTurnOutcomeRecordRequest>(HOST_OBSERVATION_DRAIN_BATCH_LIMIT);
    let (observation_ack_tx, observation_ack_rx) =
        mpsc::channel::<HostTurnOutcomeAckRequest>(HOST_OBSERVATION_DRAIN_BATCH_LIMIT);
    // Seed the wire-outcome row mirror from the recovered records (the
    // durable region is the source; this map is its projection).
    let mut turn_outcome_rows: BTreeMap<(String, String), Vec<TurnOutcomeRow>> = BTreeMap::new();
    let mut generation_start_seqs: BTreeMap<(String, String), u64> = BTreeMap::new();
    for (mob_id, record) in &records {
        for (identity, row) in &record.materialized {
            generation_start_seqs
                .insert((mob_id.clone(), identity.clone()), row.generation_start_seq);
        }
        for (identity, rows) in &record.turn_outcomes {
            turn_outcome_rows.insert((mob_id.clone(), identity.clone()), rows.clone());
        }
    }
    let runtime_incarnation = BridgeHostRuntimeIncarnation::new();
    let mut actor = MobHostActor {
        runtime_incarnation,
        binding_authority,
        persistence,
        host_runtime,
        host_comms,
        host_dsl,
        bootstrap_token,
        delegated_bind_key: mint_bootstrap_token(),
        capabilities,
        capability_facts,
        descriptor,
        pending_descriptor_refresh: None,
        advertised_address,
        registry,
        registry_owner,
        materializer: member_host.map(HostMemberMaterializer::new),
        forked_participant_service,
        unrevived: BTreeSet::new(),
        forked_participant_debts: BTreeSet::new(),
        observation_watch_tx,
        observation_record_tx: observation_record_tx.clone(),
        turn_outcome_rows,
        generation_start_seqs,
        registered_member_incarnations: HashMap::new(),
        durable_uncertainty_fail_stop: None,
    };

    // COMMIT: reserve the exact owner + identity without making it resolvable,
    // then publish the pre-serialized descriptor, then infallibly expose the
    // reserved identity. Registry conflicts are therefore detected before
    // publication, while a sink error drops the reservation and restores a
    // pristine retryable registry.
    let registry_reservation = actor.registry.reserve_identity(
        Arc::clone(&actor.registry_owner),
        host_keypair.public_key(),
        Arc::clone(&host_keypair),
    )?;
    actor.descriptor.publish_prepared(prepared_descriptor)?;
    registry_reservation.commit(host_inbox_sender);

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (initial_revival_tx, initial_revival_rx) = watch::channel(false);
    // No `.await` is permitted between the public commit above and this task
    // ownership transfer. Boot revival (A20/§14.6) remains ahead of the
    // responder drain *inside* the owned task, so the first post-restart
    // command sees a coherent host without making caller cancellation a
    // half-start boundary. Revival itself is deliberately not selected
    // against shutdown: it owns async side effects and must reach its normal
    // rollback/commit boundary before the responder observes a closed handle.
    // The readiness watch publishes only after revival and the first
    // projection sync; waiting on it never owns those actor-side effects.
    let sweep_interval =
        forked_participant_sweep_interval.unwrap_or(FORKED_PARTICIPANT_SWEEP_INTERVAL);
    let join = tokio::spawn(async move {
        if let Some(host_id) = revival_host_id.as_deref() {
            actor
                .revive_recovered_members(host_id, prepared_recovered_members)
                .await;
        }
        // Retained capability obligations are reconciled after revival so the
        // observation of "is this fork session actually present" is taken
        // against the post-revival residency set, not against a half-recovered
        // host that would look absent to a blind release.
        actor.reconcile_forked_participant_obligations().await;
        // First projection publish + journal registrations for the revived
        // residency set (materialize/release re-sync per served command).
        actor.sync_member_observation().await;
        initial_revival_tx.send_replace(true);
        run_host_responder(
            actor,
            shutdown_rx,
            observation_pending_rx,
            observation_record_rx,
            observation_ack_rx,
            sweep_interval,
        )
        .await;
    });
    Ok(MobHostActorHandle {
        runtime_incarnation,
        shutdown_tx: Some(shutdown_tx),
        join,
        initial_revival_rx,
        observation_watch_rx,
        observation_pending_tx,
        observation_record_tx,
        observation_ack_tx,
    })
}

const HOST_OBSERVATION_DRAIN_BATCH_LIMIT: usize = 64;
const FORKED_PARTICIPANT_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Wall-clock budget for the ONE maintenance sweep that runs after the
/// responder loop has exited.
///
/// Shutdown is not the place to finish durable capability work: every step the
/// sweep performs is idempotent and retried by the next process, so a sweep
/// that cannot complete promptly must not hold the actor's exit open. The
/// periodic tick inside the loop is deliberately NOT bounded this way — there
/// it is ordinary actor-owned work with the whole tick interval to run in.
const FORKED_PARTICIPANT_FINAL_SWEEP_BUDGET: Duration = Duration::from_secs(2);

/// Run the shutdown-path maintenance sweep under its fixed budget.
///
/// Returns `true` when the sweep completed, `false` when the budget elapsed
/// first and the remaining durable work was left for recovery.
pub async fn run_final_forked_participant_sweep(
    sweep: impl std::future::Future<Output = ()>,
) -> bool {
    tokio::time::timeout(FORKED_PARTICIPANT_FINAL_SWEEP_BUDGET, sweep)
        .await
        .is_ok()
}

/// Notify-driven responder drain: the non-session analogue of the member
/// drain's `try_handle_supervisor_bridge_command`, adjudicating through
/// `MobHostBindingAuthority` instead of `MeerkatMachine`.
async fn run_host_responder(
    mut actor: MobHostActor,
    mut shutdown_rx: oneshot::Receiver<()>,
    mut observation_pending_rx: mpsc::Receiver<HostTurnOutcomePendingRequest>,
    mut observation_record_rx: mpsc::Receiver<HostTurnOutcomeRecordRequest>,
    mut observation_ack_rx: mpsc::Receiver<HostTurnOutcomeAckRequest>,
    forked_participant_sweep_interval: Duration,
) {
    let notify = actor.host_runtime.inbox_notify();
    let mut forked_participant_tick = tokio::time::interval(forked_participant_sweep_interval);
    forked_participant_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    'responder: loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        // `inbox_notify` fires with `notify_waiters()`, so merely creating the
        // future is not enough: arm it before draining or an input arriving
        // between the empty drain and `select!` can lose its only wakeup.
        notified.as_mut().enable();
        // Descriptor availability is actor-owned work. At most one synchronous
        // sink attempt runs per due tick/batch boundary, and the pending state
        // retains its capped-backoff deadline across unrelated actor traffic.
        actor.retry_pending_descriptor_refresh_if_due();
        let mut candidates = Vec::new();
        for _ in 0..HOST_OBSERVATION_DRAIN_BATCH_LIMIT {
            match actor
                .host_comms
                .handoff_one_volatile_peer_input_candidate()
                .await
            {
                Ok(Some(candidate)) => candidates.push(candidate),
                Ok(None) => break,
                Err(error) => {
                    tracing::error!(
                        %error,
                        "host peer-input responder stopped with FIFO head retained"
                    );
                    break 'responder;
                }
            }
        }
        if candidates.is_empty() {
            let descriptor_refresh_retry_at = actor
                .pending_descriptor_refresh
                .as_ref()
                .map(PendingDescriptorRefresh::retry_at);
            tokio::select! {
                _ = &mut shutdown_rx => break,
                _ = forked_participant_tick.tick() => {
                    actor.sweep_forked_participants().await;
                    continue;
                }
                () = async {
                    if let Some(retry_at) = descriptor_refresh_retry_at {
                        tokio::time::sleep_until(retry_at).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    actor.retry_pending_descriptor_refresh_if_due();
                    continue;
                }
                () = &mut notified => continue,
                request = observation_pending_rx.recv() => {
                    match request {
                        Some(request) => {
                            actor.serve_turn_outcome_pending(request).await;
                            continue;
                        }
                        None => continue,
                    }
                }
                // §18 O2: tracked-turn journal records ride an internal
                // channel onto this task — the actor exclusively owns the
                // generated authority; watchers never touch it.
                request = observation_record_rx.recv() => {
                    match request {
                        Some(request) => {
                            actor.serve_turn_outcome_record(request).await;
                            continue;
                        }
                        None => continue,
                    }
                }
                request = observation_ack_rx.recv() => {
                    match request {
                        Some(request) => {
                            actor.serve_turn_outcome_ack(request).await;
                            continue;
                        }
                        None => continue,
                    }
                }
            }
        }
        for candidate in &candidates {
            actor.serve_candidate(candidate).await;
        }
        // Residency may have changed (materialize / release / revoke):
        // reconcile the observation projection + journal registrations.
        actor.sync_member_observation().await;
        for _ in 0..HOST_OBSERVATION_DRAIN_BATCH_LIMIT {
            let Ok(request) = observation_pending_rx.try_recv() else {
                break;
            };
            actor.serve_turn_outcome_pending(request).await;
        }
        // Journal records queued while candidates were being served.
        for _ in 0..HOST_OBSERVATION_DRAIN_BATCH_LIMIT {
            let Ok(request) = observation_record_rx.try_recv() else {
                break;
            };
            actor.serve_turn_outcome_record(request).await;
        }
        for _ in 0..HOST_OBSERVATION_DRAIN_BATCH_LIMIT {
            let Ok(request) = observation_ack_rx.try_recv() else {
                break;
            };
            actor.serve_turn_outcome_ack(request).await;
        }
        // The idle select is not reached while peer batches stay continuously
        // non-empty. Poll the interval once without waiting so overdue
        // capability maintenance still receives a fair turn between batches.
        let maintenance_due = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(
                std::pin::Pin::new(&mut forked_participant_tick)
                    .poll_tick(cx)
                    .is_ready(),
            )
        })
        .await;
        if maintenance_due {
            actor.sweep_forked_participants().await;
        }
        // A continuously non-empty inbox never enters the idle `select!`
        // above. Poll shutdown between finite drain batches so sustained peer
        // traffic cannot indefinitely starve actor termination (the same
        // responsiveness boundary used by the member upcall responder).
        match shutdown_rx.try_recv() {
            Ok(()) | Err(oneshot::error::TryRecvError::Closed) => break,
            Err(oneshot::error::TryRecvError::Empty) => {}
        }
    }

    if !run_final_forked_participant_sweep(actor.sweep_forked_participants()).await {
        tracing::warn!(
            "forked participant final maintenance sweep timed out; durable cleanup remains for recovery"
        );
    }

    // HOST LOSS RELEASES ITS MEMBERS' PARTICIPANT ROUTES. Dropping the actor is
    // not enough: each member route is held by its comms drain in a detached
    // task, so the refcount never reaches zero and `Drop` never runs. Under the
    // 0.8.24 one claim rule a successor may no longer rebind over a
    // still-published name even holding the same durable identity, so without
    // this an orphaned route refuses the very host that replaces it - and the
    // successor has no handle to clear it. The supervisor axis already releases
    // on shutdown; this is the member axis doing the same.
    if let Some(materializer) = actor.materializer.as_mut() {
        materializer.release_live_member_routes();
    }
}

async fn certify_stale_pending_absent(
    binding_authority: &MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    generation: u64,
    fence_token: u64,
    input_id: &str,
    stale_reason: &str,
) -> Result<(), String> {
    let old_member_key = MemberKey::new(
        AuthorityMobId::from(expected_member.mob_id.as_str()),
        AuthorityAgentIdentity::from(expected_member.agent_identity.as_str()),
    );
    let turn_key = TurnKey::new(
        old_member_key.mob_id,
        old_member_key.agent_identity,
        AuthorityGeneration(generation),
        AuthorityFenceToken(fence_token),
        AuthorityInputId(input_id.to_string()),
    );
    let machine_present = binding_authority
        .state()
        .turn_outcome_pending_window_starts
        .contains_key(&turn_key);
    let durable_present = match persistence.load(&expected_member.mob_id).await {
        Ok(Some(record)) => record
            .turn_outcome_pending
            .get(&expected_member.agent_identity)
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.generation == generation
                        && row.fence_token == fence_token
                        && row.input_id == input_id
                })
            }),
        Ok(None) => false,
        Err(error) => {
            return Err(format!(
                "{stale_reason}; could not certify stale Pending absence: {error}"
            ));
        }
    };
    if machine_present || durable_present {
        return Err(format!(
            "{stale_reason}; stale Pending still present (machine={machine_present}, durable={durable_present})"
        ));
    }
    Ok(())
}

impl MobHostActor {
    fn durable_fail_stop_reason(&self) -> Option<String> {
        self.durable_uncertainty_fail_stop.as_ref().map(|detail| {
            format!(
                "mob host is fail-stopped after uncertain binding persistence; restart required: {detail}"
            )
        })
    }

    fn retry_pending_descriptor_refresh_if_due(&mut self) {
        if self
            .pending_descriptor_refresh
            .as_ref()
            .is_some_and(PendingDescriptorRefresh::is_due)
        {
            self.retry_pending_descriptor_refresh();
        }
    }

    fn retry_pending_descriptor_refresh(&mut self) {
        let Some(mut pending) = self.pending_descriptor_refresh.take() else {
            return;
        };
        let attempt = pending.attempt_number();
        match pending.retry(&self.descriptor, &self.bootstrap_token) {
            Ok(()) => {
                tracing::info!(
                    attempt,
                    "mob host: replacement bootstrap descriptor publication recovered"
                );
            }
            Err(error) => {
                if pending.should_log_failure() {
                    tracing::warn!(
                        error = %error,
                        attempt,
                        retry_delay_ms = pending.retry_delay.as_millis() as u64,
                        "mob host: replacement bootstrap descriptor remains unavailable; retry remains pending"
                    );
                }
                self.pending_descriptor_refresh = Some(pending);
            }
        }
    }

    /// Rebuild + publish the observation projection and reconcile the
    /// per-session tracked-turn journal registrations on the member runtime
    /// adapter (DEC-P6E-2 + DEC-P6F-9's registration seam). Idempotent;
    /// called after boot revival and after every served command / journal
    /// record (bounded by resident-member count).
    async fn sync_member_observation(&mut self) {
        let Some(materializer) = self.materializer.as_ref() else {
            let _ = self
                .observation_watch_tx
                .send(HostObservationProjection::default());
            return;
        };
        let adapter = Arc::clone(&materializer.substrate().runtime_adapter);
        let state = self.binding_authority.state().clone();
        let durable_members: BTreeSet<(String, String)> = state
            .materialized_sessions
            .keys()
            .map(|member_key| {
                (
                    member_key.mob_id.0.clone(),
                    member_key.agent_identity.0.clone(),
                )
            })
            .collect();
        let mut durable_records = BTreeMap::new();
        for mob_id in state
            .materialized_sessions
            .keys()
            .map(|key| key.mob_id.0.clone())
            .collect::<BTreeSet<_>>()
        {
            if let Ok(Some(record)) = self.persistence.load(&mob_id).await {
                durable_records.insert(mob_id, record);
            }
        }
        let mut sessions = BTreeMap::new();
        let mut current: HashMap<
            meerkat_core::types::SessionId,
            meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        > = HashMap::new();
        let Some(host_id) = self.host_comms.peer_id().map(|peer_id| peer_id.to_string()) else {
            tracing::error!(
                "host comms runtime has no peer id; refusing tracked-turn journal registration"
            );
            let _ = self
                .observation_watch_tx
                .send(HostObservationProjection::default());
            return;
        };
        for (member_key, session) in &state.materialized_sessions {
            let Some(host_binding_generation) =
                state.binding_generations.get(&member_key.mob_id).copied()
            else {
                continue;
            };
            let Some(generation) = state.materialized_generations.get(member_key) else {
                continue;
            };
            let Some(fence_token) = state.materialized_fences.get(member_key) else {
                continue;
            };
            let Ok(session_id) = meerkat_core::types::SessionId::parse(&session.0) else {
                tracing::error!(
                    mob_id = %member_key.mob_id.0,
                    agent_identity = %member_key.agent_identity.0,
                    "recorded materialized session id is unparseable; observation skipped"
                );
                continue;
            };
            let turn_outcomes = self
                .turn_outcome_rows
                .get(&(
                    member_key.mob_id.0.clone(),
                    member_key.agent_identity.0.clone(),
                ))
                .map(|rows| {
                    rows.iter()
                        .filter(|row| {
                            row.generation == generation.0 && row.fence_token == fence_token.0
                        })
                        .map(TurnOutcomeRow::to_wire)
                        .collect()
                })
                .unwrap_or_default();
            let pending_turns = state
                .turn_outcome_pending_window_starts
                .iter()
                .filter(|(turn_key, _)| {
                    turn_key.mob_id == member_key.mob_id
                        && turn_key.agent_identity == member_key.agent_identity
                        && turn_key.generation == *generation
                        && turn_key.fence_token == *fence_token
                })
                .map(|(turn_key, window_start)| PendingTurnObservation {
                    input_id: turn_key.input_id.0.clone(),
                    generation: turn_key.generation.0,
                    fence_token: turn_key.fence_token.0,
                    window_start: *window_start,
                    bounded_result_spec: durable_records
                        .get(&turn_key.mob_id.0)
                        .and_then(|record| {
                            record.turn_outcome_pending.get(&turn_key.agent_identity.0)
                        })
                        .and_then(|rows| {
                            rows.iter().find(|row| {
                                row.input_id == turn_key.input_id.0
                                    && row.generation == turn_key.generation.0
                                    && row.fence_token == turn_key.fence_token.0
                            })
                        })
                        .and_then(|row| row.bounded_result_spec.clone()),
                })
                .collect();
            let incarnation = meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
                mob_id: member_key.mob_id.0.clone(),
                agent_identity: member_key.agent_identity.0.clone(),
                host_id: host_id.clone(),
                binding_generation: host_binding_generation,
                member_session_id: session.0.clone(),
                generation: generation.0,
                fence_token: fence_token.0,
            };
            sessions.insert(
                session.0.clone(),
                SessionObservationFacts {
                    incarnation: incarnation.clone(),
                    mob_id: member_key.mob_id.0.clone(),
                    agent_identity: member_key.agent_identity.0.clone(),
                    generation: generation.0,
                    fence_token: fence_token.0,
                    generation_start_seq: self
                        .generation_start_seqs
                        .get(&(
                            member_key.mob_id.0.clone(),
                            member_key.agent_identity.0.clone(),
                        ))
                        .copied()
                        .unwrap_or_else(default_generation_start_seq),
                    pending_turns,
                    turn_outcomes,
                },
            );
            // Residency publication is part of the materialize/revive actor
            // transaction, never an idempotent projection side effect. A
            // mismatch here is fail-closed projection evidence only.
            if adapter.member_incarnation(&session_id).as_ref() != Some(&incarnation) {
                tracing::error!(
                    mob_id = %member_key.mob_id.0,
                    agent_identity = %member_key.agent_identity.0,
                    session_id = %session_id,
                    expected = ?incarnation,
                    current = ?adapter.member_incarnation(&session_id),
                    "refusing to publish member observation without the actor-committed runtime residency"
                );
                sessions.remove(&session.0);
                continue;
            }
            current.insert(session_id, incarnation);
        }
        self.registered_member_incarnations = current;
        // Row-mirror hygiene: released/revoked members' rows were pruned
        // from the durable region; drop their mirror entries too.
        self.turn_outcome_rows
            .retain(|key, _| durable_members.contains(key));
        self.generation_start_seqs
            .retain(|key, _| durable_members.contains(key));
        let _ = self
            .observation_watch_tx
            .send(HostObservationProjection { sessions });
    }

    fn exact_current_member_incarnation(
        &self,
        expected: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    ) -> Result<MemberKey, String> {
        let member_key = MemberKey::new(
            AuthorityMobId::from(expected.mob_id.as_str()),
            AuthorityAgentIdentity::from(expected.agent_identity.as_str()),
        );
        let state = self.binding_authority.state();
        let session = state.materialized_sessions.get(&member_key);
        let generation = state.materialized_generations.get(&member_key);
        let fence_token = state.materialized_fences.get(&member_key);
        let binding_generation = state.binding_generations.get(&member_key.mob_id);
        let host_id = self.host_comms.peer_id().map(|peer_id| peer_id.to_string());
        let current = match (
            session,
            generation,
            fence_token,
            binding_generation,
            host_id,
        ) {
            (
                Some(session),
                Some(generation),
                Some(fence_token),
                Some(binding_generation),
                Some(host_id),
            ) => Some(
                meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
                    mob_id: member_key.mob_id.0.clone(),
                    agent_identity: member_key.agent_identity.0.clone(),
                    host_id,
                    binding_generation: *binding_generation,
                    member_session_id: session.0.clone(),
                    generation: generation.0,
                    fence_token: fence_token.0,
                },
            ),
            _ => None,
        };
        if current.as_ref() == Some(expected) {
            Ok(member_key)
        } else {
            Err(format!(
                "expected member residency {expected:?}; host actor current is {current:?}"
            ))
        }
    }

    /// Reserve/cancel the durable pre-effect Pending row on the actor's sole
    /// authority task. Projection publication precedes the reply.
    async fn serve_turn_outcome_pending(&mut self, request: HostTurnOutcomePendingRequest) {
        if let Some(reason) = self.durable_fail_stop_reason() {
            match request {
                HostTurnOutcomePendingRequest::Reserve { reply, .. } => {
                    let _ = reply.send(Err(reason));
                }
                HostTurnOutcomePendingRequest::Cancel { reply, .. } => {
                    let _ = reply.send(Err(reason));
                }
                HostTurnOutcomePendingRequest::CancelTracked { reply, .. }
                | HostTurnOutcomePendingRequest::CompleteTrackedCancel { reply, .. } => {
                    let _ = reply.send(Err(reason));
                }
            }
            return;
        }
        match request {
            HostTurnOutcomePendingRequest::Reserve {
                expected_member,
                generation,
                fence_token,
                input_id,
                bounded_result_spec,
                fresh_window_start,
                reply,
            } => {
                let member_key = match self.exact_current_member_incarnation(&expected_member) {
                    Ok(member_key) => member_key,
                    Err(_) => {
                        let _ = reply.send(Ok(HostPendingReservationReply::Stale));
                        return;
                    }
                };
                if generation != expected_member.generation
                    || fence_token != expected_member.fence_token
                {
                    let _ = reply.send(Ok(HostPendingReservationReply::Stale));
                    return;
                }
                let row = TurnOutcomePendingRow {
                    input_id,
                    generation,
                    fence_token,
                    // Replay arms ignore this proposal and return their
                    // authoritative existing window. In replay-only mode,
                    // MAX is an inert probe value: a fresh transition is
                    // discarded before persistence/authority commit below.
                    window_start: fresh_window_start.unwrap_or(u64::MAX),
                    bounded_result_spec,
                };
                let result = reserve_turn_outcome_pending_with_mode(
                    &mut self.binding_authority,
                    self.persistence.as_ref(),
                    &member_key,
                    &row,
                    fresh_window_start.is_some(),
                )
                .await;
                let result = match result {
                    Ok(disposition) => Ok(match disposition {
                        TurnOutcomePendingReservationDisposition::Reserved { window_start } => {
                            HostPendingReservationReply::Reserved { window_start }
                        }
                        TurnOutcomePendingReservationDisposition::Replayed { window_start } => {
                            HostPendingReservationReply::Replayed { window_start }
                        }
                        TurnOutcomePendingReservationDisposition::TerminalReplay => {
                            HostPendingReservationReply::TerminalReplay
                        }
                        TurnOutcomePendingReservationDisposition::FreshRequired => {
                            HostPendingReservationReply::FreshRequired
                        }
                        TurnOutcomePendingReservationDisposition::JournalFull => {
                            HostPendingReservationReply::JournalFull
                        }
                        TurnOutcomePendingReservationDisposition::Stale => {
                            HostPendingReservationReply::Stale
                        }
                    }),
                    Err(error) => {
                        if error.is_durable_uncertainty() {
                            self.durable_uncertainty_fail_stop = Some(error.to_string());
                        }
                        Err(error.to_string())
                    }
                };
                if result.is_ok() {
                    self.sync_member_observation().await;
                }
                let _ = reply.send(result);
            }
            HostTurnOutcomePendingRequest::Cancel {
                expected_member,
                generation,
                fence_token,
                input_id,
                reply,
            } => {
                if generation != expected_member.generation
                    || fence_token != expected_member.fence_token
                {
                    let _ = reply.send(Err(
                        "Pending cancellation key differs from its exact residency".to_string(),
                    ));
                    return;
                }
                let member_key = match self.exact_current_member_incarnation(&expected_member) {
                    Ok(member_key) => member_key,
                    Err(reason) => {
                        // A superseding materialization prunes the old
                        // generation's Pending region as part of its one
                        // actor-linearized machine+durable transition. The
                        // losing G1 accept then proves no effect and asks us
                        // to cancel that exact row. Treat this as converged
                        // only when both authorities already say it is
                        // absent; retaining either copy remains a loud error
                        // because returning success would mint a false
                        // definite-no-effect response.
                        let result = certify_stale_pending_absent(
                            &self.binding_authority,
                            self.persistence.as_ref(),
                            &expected_member,
                            generation,
                            fence_token,
                            &input_id,
                            &reason,
                        )
                        .await;
                        let _ = reply.send(result);
                        return;
                    }
                };
                // A definite runtime rejection is itself a terminal delivery
                // result. Consume Pending into the same durable NoEffect
                // tombstone used by the explicit cancel command; merely
                // deleting Pending would let an already-delayed duplicate
                // recreate and execute the key after the rejection reply.
                let result = cancel_tracked_input_journal(
                    &mut self.binding_authority,
                    self.persistence.as_ref(),
                    &member_key,
                    &input_id,
                    generation,
                    fence_token,
                    false,
                )
                .await;
                let result = match result {
                    Ok(TrackedInputCancelDisposition::NoEffect) => Ok(()),
                    Ok(other) => Err(format!(
                        "definite-no-effect Pending cancellation produced {other:?}"
                    )),
                    Err(error) => {
                        if error.is_durable_uncertainty() {
                            self.durable_uncertainty_fail_stop = Some(error.to_string());
                        }
                        Err(error.to_string())
                    }
                };
                if result.is_ok() {
                    self.sync_member_observation().await;
                }
                let _ = reply.send(result);
            }
            HostTurnOutcomePendingRequest::CancelTracked {
                expected_member,
                generation,
                fence_token,
                input_id,
                runtime_input_present,
                reply,
            } => {
                if generation != expected_member.generation
                    || fence_token != expected_member.fence_token
                {
                    let _ = reply.send(Err(
                        "tracked-input cancellation key differs from its exact residency"
                            .to_string(),
                    ));
                    return;
                }
                let member_key = match self.exact_current_member_incarnation(&expected_member) {
                    Ok(member_key) => member_key,
                    Err(_) => {
                        let _ = reply.send(Ok(HostTrackedInputCancelReply::Stale));
                        return;
                    }
                };
                let result = cancel_tracked_input_journal(
                    &mut self.binding_authority,
                    self.persistence.as_ref(),
                    &member_key,
                    &input_id,
                    generation,
                    fence_token,
                    runtime_input_present,
                )
                .await;
                let result = match result {
                    Ok(TrackedInputCancelDisposition::NoEffect) => {
                        Ok(HostTrackedInputCancelReply::NoEffect)
                    }
                    Ok(TrackedInputCancelDisposition::Cancelling) => {
                        Ok(HostTrackedInputCancelReply::Cancelling)
                    }
                    Ok(TrackedInputCancelDisposition::Cancelled) => {
                        Ok(HostTrackedInputCancelReply::Cancelled)
                    }
                    Ok(TrackedInputCancelDisposition::Terminal(record)) => {
                        Ok(HostTrackedInputCancelReply::Terminal(record))
                    }
                    Ok(TrackedInputCancelDisposition::Stale) => {
                        Ok(HostTrackedInputCancelReply::Stale)
                    }
                    Ok(TrackedInputCancelDisposition::Unreserved) => {
                        Ok(HostTrackedInputCancelReply::Unreserved)
                    }
                    Err(error) => {
                        if error.is_durable_uncertainty() {
                            self.durable_uncertainty_fail_stop = Some(error.to_string());
                        }
                        Err(error.to_string())
                    }
                };
                if result.is_ok() {
                    self.sync_member_observation().await;
                }
                let _ = reply.send(result);
            }
            HostTurnOutcomePendingRequest::CompleteTrackedCancel {
                expected_member,
                generation,
                fence_token,
                input_id,
                reply,
            } => {
                if generation != expected_member.generation
                    || fence_token != expected_member.fence_token
                {
                    let _ = reply.send(Err(
                        "tracked-input cancel completion key differs from its exact residency"
                            .to_string(),
                    ));
                    return;
                }
                let member_key = match self.exact_current_member_incarnation(&expected_member) {
                    Ok(member_key) => member_key,
                    Err(_) => {
                        let _ = reply.send(Ok(HostTrackedInputCancelReply::Stale));
                        return;
                    }
                };
                let result = complete_tracked_input_cancel_journal(
                    &mut self.binding_authority,
                    self.persistence.as_ref(),
                    &member_key,
                    &input_id,
                    generation,
                    fence_token,
                )
                .await;
                let result = match result {
                    Ok(TrackedInputCancelDisposition::NoEffect) => {
                        Ok(HostTrackedInputCancelReply::NoEffect)
                    }
                    Ok(TrackedInputCancelDisposition::Cancelling) => {
                        Ok(HostTrackedInputCancelReply::Cancelling)
                    }
                    Ok(TrackedInputCancelDisposition::Cancelled) => {
                        Ok(HostTrackedInputCancelReply::Cancelled)
                    }
                    Ok(TrackedInputCancelDisposition::Terminal(record)) => {
                        Ok(HostTrackedInputCancelReply::Terminal(record))
                    }
                    Ok(TrackedInputCancelDisposition::Stale) => {
                        Ok(HostTrackedInputCancelReply::Stale)
                    }
                    Ok(TrackedInputCancelDisposition::Unreserved) => {
                        Ok(HostTrackedInputCancelReply::Unreserved)
                    }
                    Err(error) => {
                        if error.is_durable_uncertainty() {
                            self.durable_uncertainty_fail_stop = Some(error.to_string());
                        }
                        Err(error.to_string())
                    }
                };
                if result.is_ok() {
                    self.sync_member_observation().await;
                }
                let _ = reply.send(result);
            }
        }
    }

    /// Serve one tracked-turn journal record (§18 O2): generated-authority
    /// dedup + witness-gated durable row write + projection re-publish.
    async fn serve_turn_outcome_record(&mut self, request: HostTurnOutcomeRecordRequest) {
        if let Some(reason) = self.durable_fail_stop_reason() {
            let _ = request.reply.send(Err(reason));
            return;
        }
        let member_key = match self.exact_current_member_incarnation(&request.expected_member) {
            Ok(member_key) => member_key,
            Err(reason) => {
                tracing::debug!(%reason, "dropped stale turn-outcome completion");
                let _ = request.reply.send(Ok(()));
                return;
            }
        };
        let mob_id = request.expected_member.mob_id.clone();
        let agent_identity = request.expected_member.agent_identity.clone();
        let row = TurnOutcomeRow {
            input_id: request.record.input_id.clone(),
            generation: request.record.generation,
            fence_token: request.record.fence_token,
            terminal_seq: request.record.terminal_seq,
            outcome: request.record.outcome.clone(),
            bounded_result: request.record.bounded_result.clone(),
        };
        if row.generation != request.expected_member.generation
            || row.fence_token != request.expected_member.fence_token
        {
            let _ = request.reply.send(Err(
                "turn-outcome record key differs from its exact residency".to_string(),
            ));
            return;
        }
        let result = record_turn_outcome_journal(
            &mut self.binding_authority,
            self.persistence.as_ref(),
            &member_key,
            &row,
        )
        .await;
        let reply = match result {
            Ok(TurnOutcomeRecordDisposition::Recorded) => {
                let rows = self
                    .turn_outcome_rows
                    .entry((mob_id.clone(), agent_identity.clone()))
                    .or_default();
                rows.retain(|existing| {
                    !(existing.input_id == row.input_id
                        && existing.generation == row.generation
                        && existing.fence_token == row.fence_token)
                });
                rows.push(row);
                self.sync_member_observation().await;
                Ok(())
            }
            Ok(TurnOutcomeRecordDisposition::Replayed) => Ok(()),
            Ok(TurnOutcomeRecordDisposition::DroppedStale) => {
                tracing::debug!(
                    mob_id = %mob_id,
                    agent_identity = %agent_identity,
                    generation = row.generation,
                    fence_token = row.fence_token,
                    input_id = %row.input_id,
                    "dropped stale turn-outcome completion after release/rematerialization"
                );
                Ok(())
            }
            Ok(TurnOutcomeRecordDisposition::DroppedUnreserved) => {
                tracing::debug!(
                    mob_id = %mob_id,
                    agent_identity = %agent_identity,
                    generation = row.generation,
                    fence_token = row.fence_token,
                    input_id = %row.input_id,
                    "dropped turn-outcome completion without Pending authority"
                );
                Ok(())
            }
            Err(error) => {
                if error.is_durable_uncertainty() {
                    self.durable_uncertainty_fail_stop = Some(error.to_string());
                }
                tracing::error!(
                    mob_id = %mob_id,
                    agent_identity = %agent_identity,
                    error = %error,
                    "turn-outcome journal record failed"
                );
                Err(error.to_string())
            }
        };
        let _ = request.reply.send(reply);
    }

    /// Prune exact outcome rows only after the controlling pump reports that
    /// it consumed them. Each row has its own generated transition and
    /// durable CAS; a partial batch failure is safe because a retry replays
    /// already-pruned keys and continues with the remaining rows.
    async fn serve_turn_outcome_ack(&mut self, request: HostTurnOutcomeAckRequest) {
        if let Some(reason) = self.durable_fail_stop_reason() {
            let _ = request.reply.send(Err(reason));
            return;
        }
        let member_key = match self.exact_current_member_incarnation(&request.expected_member) {
            Ok(member_key) => member_key,
            Err(reason) => {
                let _ = request.reply.send(Err(reason));
                return;
            }
        };
        let mob_id = request.expected_member.mob_id.clone();
        let agent_identity = request.expected_member.agent_identity.clone();
        let mut pruned_any = false;
        let mut result = Ok(());
        for ack in &request.acks {
            if ack.generation != request.expected_member.generation
                || ack.fence_token != request.expected_member.fence_token
            {
                result = Err(
                    "turn-outcome acknowledgement key differs from its exact residency".to_string(),
                );
                break;
            }
            match acknowledge_turn_outcome_journal(
                &mut self.binding_authority,
                self.persistence.as_ref(),
                &member_key,
                ack,
            )
            .await
            {
                Ok(TurnOutcomeAckDisposition::Pruned) => {
                    pruned_any = true;
                    if let Some(rows) = self
                        .turn_outcome_rows
                        .get_mut(&(mob_id.clone(), agent_identity.clone()))
                    {
                        rows.retain(|row| {
                            !(row.generation == ack.generation
                                && row.fence_token == ack.fence_token
                                && row.input_id == ack.input_id)
                        });
                    }
                }
                Ok(TurnOutcomeAckDisposition::Absent) => {}
                Err(error) => {
                    if error.is_durable_uncertainty() {
                        self.durable_uncertainty_fail_stop = Some(error.to_string());
                    }
                    tracing::error!(
                        mob_id = %mob_id,
                        agent_identity = %agent_identity,
                        generation = ack.generation,
                        fence_token = ack.fence_token,
                        input_id = %ack.input_id,
                        error = %error,
                        "turn-outcome acknowledgement failed"
                    );
                    result = Err(error.to_string());
                    break;
                }
            }
        }
        if pruned_any {
            let key = (mob_id, agent_identity);
            if self.turn_outcome_rows.get(&key).is_some_and(Vec::is_empty) {
                self.turn_outcome_rows.remove(&key);
            }
            // Publish before replying: the awaiting poll re-reads this watch
            // projection and cannot echo a row it just acknowledged.
            self.sync_member_observation().await;
        }
        let _ = request.reply.send(result);
    }

    async fn serve_candidate(&mut self, candidate: &PeerInputCandidate) {
        let InteractionContent::Request { intent, params, .. } = &candidate.interaction.content
        else {
            // Responses/acks/lifecycle events have no serving semantics on
            // the host responder; complete them so the queue never wedges.
            self.host_comms
                .mark_interaction_complete(&candidate.interaction.id);
            return;
        };
        if intent != SUPERVISOR_BRIDGE_INTENT {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Unsupported,
                format!("unsupported intent '{intent}' on the mob host responder"),
                None,
            )
            .await;
            return;
        }

        // Record the inbound peer request under complete request/response
        // authority BEFORE any decode or side effect (the member drain's
        // boundary discipline).
        if !self.record_inbound_request(candidate) {
            return;
        }

        let command = match decode_bridge_command(params.clone()) {
            Ok(command) => command,
            Err(error) => {
                let cause = match &error {
                    BridgeCommandDecodeError::UnsupportedProtocolVersion(_) => {
                        BridgeRejectionCause::UnsupportedProtocolVersion
                    }
                    BridgeCommandDecodeError::Invalid(_) => BridgeRejectionCause::Unsupported,
                };
                self.send_failure(
                    candidate,
                    cause,
                    format!("invalid bridge command: {error}"),
                    None,
                )
                .await;
                return;
            }
        };

        if let Some(reason) = self.durable_fail_stop_reason() {
            // The actor may be fail-stopped before it has installed durable
            // supervisor trust (for example, an ambiguous first BindHost
            // write). The command has already decoded, so retain the same
            // authenticated one-shot reply route used by ordinary typed
            // rejections instead of dropping the terminal as PeerNotFound.
            self.send_failure(candidate, BridgeRejectionCause::Internal, reason, None)
                .await;
            return;
        }

        match command {
            BridgeCommand::BindHost(payload) => {
                self.serve_bind_host_candidate(candidate, payload).await;
            }
            BridgeCommand::RebindHost(payload) => {
                self.serve_rebind_host_candidate(candidate, payload).await;
            }
            BridgeCommand::RevokeHost(payload) => {
                self.serve_revoke_host_candidate(candidate, payload).await;
            }
            BridgeCommand::MaterializeMember(payload) => {
                self.serve_materialize_member_candidate(candidate, *payload)
                    .await;
            }
            BridgeCommand::ReleaseMember(payload) => {
                self.serve_release_member_candidate(candidate, payload)
                    .await;
            }
            BridgeCommand::CreateForkedParticipant(payload) => {
                self.serve_create_forked_participant(candidate, payload)
                    .await;
            }
            BridgeCommand::RevokeForkedParticipant(payload) => {
                self.serve_revoke_forked_participant(candidate, payload)
                    .await;
            }
            BridgeCommand::InstallPeerTrust(payload) => {
                self.serve_install_peer_trust_candidate(candidate, payload)
                    .await;
            }
            BridgeCommand::RemovePeerTrust(payload) => {
                self.serve_remove_peer_trust_candidate(candidate, payload)
                    .await;
            }
            BridgeCommand::HostStatus(payload) => {
                self.serve_host_status_candidate(candidate, payload).await;
            }
            BridgeCommand::IssueHostBindingDescriptor(payload) => {
                self.serve_issue_host_binding_descriptor_candidate(candidate, payload)
                    .await;
            }
            // Everything member-addressed, plus the phase-6 operator upcall
            // (member-ORIGINATED — it never arrives host-addressed), stays
            // fail-closed:
            _ => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Unsupported,
                    "unsupported supervisor bridge command on the mob host (the host serves \
                     BindHost/RebindHost/RevokeHost/MaterializeMember/ReleaseMember/InstallPeerTrust/\
                     RemovePeerTrust/HostStatus)",
                    None,
                )
                .await;
            }
        }
    }

    /// Canonical ingress identity for rung-0 command admission: the admitted
    /// canonical peer id, or the id derived from the signed pubkey — never a
    /// display name (`sender_matches_bridge_peer` discipline). An
    /// unauthenticated candidate yields an empty id, which can never match a
    /// recorded supervisor — the machine's SenderMismatch arm adjudicates.
    fn ingress_sender_peer_id(candidate: &PeerInputCandidate) -> String {
        candidate
            .ingress
            .canonical_peer_id
            .map(|peer_id| peer_id.as_str())
            .or_else(|| {
                candidate.ingress.signing_pubkey.map(|pubkey| {
                    meerkat_core::comms::PeerId::from_ed25519_pubkey(&pubkey).as_str()
                })
            })
            .unwrap_or_default()
    }

    /// Rung 0 for every phase-3 host-addressed command (§3): the generated
    /// `ResolveHostCommandAdmission` arms adjudicate; `true` means admitted.
    async fn admit_host_command(
        &mut self,
        candidate: &PeerInputCandidate,
        mob_id: &str,
        epoch: u64,
        binding_generation: u64,
        declared_reply_address: &str,
    ) -> bool {
        let observations = HostCommandObservations {
            mob_id: mob_id.to_string(),
            sender_peer_id: Self::ingress_sender_peer_id(candidate),
            epoch,
            binding_generation,
            // P-2: no phase-3 host-addressed command carries a turn
            // directive; phase 6's member-drain feed flips only these two
            // booleans at ITS call site.
            turn_directive_present: false,
            turn_directive_supported: self.capability_facts.durable_sessions,
        };
        match resolve_host_command_admission(&mut self.binding_authority, observations) {
            Ok(HostCommandAdmission::Admitted) => true,
            Ok(HostCommandAdmission::Rejected { cause, reason }) => {
                self.send_failure(candidate, cause, reason, Some(declared_reply_address))
                    .await;
                false
            }
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("host command admission failed: {error}"),
                    Some(declared_reply_address),
                )
                .await;
                false
            }
        }
    }

    /// Refuse to serve a capability-protected fork session without its exact
    /// authenticated capability.
    ///
    /// Returns `Ok(())` when the launch may proceed: either it is not a
    /// `Resume`, or the resumed session belongs to no capability record, or it
    /// belongs to one AND the payload presents that record's exact immutable
    /// reference. Every other shape is refused before dedup replay, preflight,
    /// or any build effect — and, critically, before `attach`, so a refused
    /// request never consumes a use of the capability it failed to present.
    ///
    /// Both failure directions are `ForkedParticipantTampered`. The cause is
    /// not "not found" (the capability plainly exists) and not a route or
    /// source mismatch; it is the caller offering material that does not
    /// authenticate this operation — a visible session id standing in for a
    /// bearer, or a reference that is not the one the owner recorded. The
    /// reason string distinguishes the two for operators.
    async fn enforce_fork_session_containment(
        &self,
        payload: &BridgeMaterializePayload,
    ) -> Result<(), (BridgeRejectionCause, String)> {
        // Protection is a property of the capability store, and the store is
        // composed with the service. A host that owns no capability store
        // holds no fork children, so it protects nothing and cannot be the
        // owner being bypassed.
        let Some(service) = self.forked_participant_service.as_ref() else {
            return Ok(());
        };
        let MaterializeLaunchMode::Resume { session_id, .. } = &payload.launch else {
            // Fresh builds mint their own session; there is no visible id to
            // ride. A carried attachment on a non-Resume launch is already
            // refused at the wire boundary.
            return Ok(());
        };
        let Ok(resumed) = meerkat_core::types::SessionId::parse(session_id) else {
            // Not a session identity at all: no record can claim it, and the
            // ordinary launch path owns the malformed-id rejection.
            return Ok(());
        };
        let protection = match service.protected_fork_session(&resumed).await {
            Ok(protection) => protection,
            Err(error) => {
                // Fail closed: an unreadable containment answer must never be
                // read as "unprotected".
                return Err((
                    BridgeRejectionCause::Unavailable,
                    format!("fork session containment check failed: {error}"),
                ));
            }
        };
        admit_resume_against_fork_protection(
            session_id,
            protection.as_ref(),
            payload.forked_participant_attachment.as_ref(),
            service.owner_route(),
        )
    }

    /// One host maintenance pass over source-owned capabilities.
    ///
    /// Ordering matters and is the whole point of the pass:
    ///   1. observe expiry, so a capability whose TTL has passed records its
    ///      terminal (parking it when an attachment is still held);
    ///   2. converge every parked terminal this host actually holds — the
    ///      autonomous half that survives coordinator loss;
    ///   3. sweep cleanup, so a capability that step 2 just terminalized has
    ///      its fork archived in the same pass rather than a tick later.
    ///
    /// Step 3 is the ONLY archiver. Convergence never archives a fork itself;
    /// it releases the attachment and lets the existing terminal/cleanup
    /// machinery own the rest.
    async fn sweep_forked_participants(&mut self) {
        if self.forked_participant_service.is_none() {
            return;
        }
        let now = chrono::Utc::now();
        let expiry = match self.forked_participant_service.as_ref() {
            Some(service) => service.sweep_expiry(now).await,
            None => return,
        };
        let convergence = self.converge_pending_forked_participant_attachments().await;
        let cleanup = match self.forked_participant_service.as_ref() {
            Some(service) => service.sweep_cleanup(now).await,
            None => return,
        };
        match (expiry, cleanup) {
            (Ok(expiry), Ok(cleanup)) => {
                let expired = expiry.expired.len();
                let pending = expiry.expiry_pending_attached.len();
                let cleaned = cleanup.completed.len();
                let retained = cleanup.retained.len();
                if expired
                    + pending
                    + cleaned
                    + retained
                    + convergence.converged
                    + convergence.retained
                    + convergence.unheld
                    + convergence.unreadable
                    > 0
                {
                    tracing::info!(
                        expired,
                        expiry_pending = pending,
                        cleaned,
                        cleanup_retained = retained,
                        converged = convergence.converged,
                        convergence_retained = convergence.retained,
                        convergence_unheld = convergence.unheld,
                        convergence_unreadable = convergence.unreadable,
                        "forked participant host maintenance sweep"
                    );
                }
            }
            (expiry, cleanup) => {
                tracing::warn!(
                    expiry_ok = expiry.is_ok(),
                    cleanup_ok = cleanup.is_ok(),
                    converged = convergence.converged,
                    convergence_retained = convergence.retained,
                    "forked participant host maintenance sweep retained typed work"
                );
            }
        }
    }

    /// Autonomously converge capability terminals parked behind attachments
    /// this host holds.
    ///
    /// A capability that expires or is revoked while attached stays parked
    /// until its exact attachment is released. Normally the coordinator that
    /// took the attachment sends `ReleaseMember` and that release does it. If
    /// the coordinator never comes back, nothing else would: the residency
    /// would stay live forever and the fork would never be archived. This pass
    /// closes that hole from the holder's side.
    ///
    /// It does NOT synthesize a controller command. It resolves release
    /// admission through the generated authority at the residency's own
    /// recorded `(generation, fence_token)` tuple, then runs the SAME
    /// reply-free teardown the bridge `ReleaseMember` arm runs. Every fence,
    /// witness, and durable receipt is the ordinary one.
    ///
    /// One residency that cannot be torn down never blocks the rest: each
    /// correlation is independent, failures are counted and retried on later
    /// ticks, and the capability release only ever happens after this host has
    /// proven the residency disposed.
    async fn converge_pending_forked_participant_attachments(
        &mut self,
    ) -> ForkedParticipantConvergenceCounts {
        let mut counts = ForkedParticipantConvergenceCounts::default();
        let Some(service) = self.forked_participant_service.as_ref() else {
            return counts;
        };
        let report = match service.list_pending_attached().await {
            Ok(report) => report,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "capability pending-attached enumeration failed; convergence retries next tick"
                );
                return counts;
            }
        };
        if report.pending.is_empty() && report.unreadable.is_empty() {
            return counts;
        }
        counts.unreadable = report.unreadable.len();
        for (capability_id, detail) in &report.unreadable {
            tracing::error!(
                capability = %capability_id.correlation_hint(),
                detail = %detail,
                "capability record has an unreadable parked terminal; it cannot converge until repaired"
            );
        }
        let records = match self.persistence.list_records().await {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "capability convergence could not read durable host rows; retries next tick"
                );
                return counts;
            }
        };
        let correlations =
            correlate_pending_forked_participant_attachments(&report.pending, &records);
        counts.unheld = report.pending.len().saturating_sub(correlations.len());
        for correlation in correlations {
            let member_key = MemberKey::new(
                AuthorityMobId::from(correlation.mob_id.as_str()),
                AuthorityAgentIdentity::from(correlation.agent_identity.as_str()),
            );
            // The residency must still be the one that holds the attachment.
            // A row that moved on between the durable read and this step is
            // not ours to tear down at the tuple we read.
            let still_recorded = self
                .binding_authority
                .state()
                .materialized_sessions
                .get(&member_key)
                .is_some_and(|session| session.0 == correlation.session_id);
            if !still_recorded {
                counts.retained += 1;
                continue;
            }
            match resolve_release_admission(
                &mut self.binding_authority,
                &member_key,
                correlation.generation,
                correlation.fence_token,
            ) {
                Ok(ReleaseAdmission::Admitted) => {}
                // Already released durably: the association went with the row
                // and there is nothing left for this host to converge.
                Ok(ReleaseAdmission::Replay { .. }) => continue,
                Ok(ReleaseAdmission::Rejected { kind }) => {
                    counts.retained += 1;
                    tracing::warn!(
                        mob_id = %correlation.mob_id,
                        agent_identity = %correlation.agent_identity,
                        terminal = correlation.terminal.as_str(),
                        cause = ?kind,
                        "capability convergence release admission refused; retained for retry"
                    );
                    continue;
                }
                Err(error) => {
                    counts.retained += 1;
                    tracing::warn!(
                        mob_id = %correlation.mob_id,
                        agent_identity = %correlation.agent_identity,
                        %error,
                        "capability convergence release admission failed; retained for retry"
                    );
                    continue;
                }
            }
            match self
                .release_recorded_member_residency(
                    &member_key,
                    correlation.generation,
                    correlation.fence_token,
                )
                .await
            {
                MemberTeardownOutcome::Released { .. } => {
                    counts.converged += 1;
                    tracing::info!(
                        mob_id = %correlation.mob_id,
                        agent_identity = %correlation.agent_identity,
                        terminal = correlation.terminal.as_str(),
                        "host autonomously converged a parked capability terminal"
                    );
                }
                MemberTeardownOutcome::ReleasedWithResidue { cause, detail } => {
                    counts.converged += 1;
                    tracing::warn!(
                        mob_id = %correlation.mob_id,
                        agent_identity = %correlation.agent_identity,
                        cause = ?cause,
                        detail = %detail,
                        "capability convergence released the residency but left post-commit residue"
                    );
                }
                MemberTeardownOutcome::Retained { cause, detail } => {
                    counts.retained += 1;
                    tracing::warn!(
                        mob_id = %correlation.mob_id,
                        agent_identity = %correlation.agent_identity,
                        terminal = correlation.terminal.as_str(),
                        cause = ?cause,
                        detail = %detail,
                        "capability convergence retained a parked terminal; the row, its \
                         association, and the attachment all survive for the next tick"
                    );
                }
            }
        }
        counts
    }

    async fn validate_forked_participant_source(
        &self,
        source: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        binding_generation: u64,
    ) -> Result<(), BridgeRejectionCause> {
        let host_id = self
            .host_comms
            .peer_id()
            .map(|peer| peer.to_string())
            .ok_or(BridgeRejectionCause::Unavailable)?;
        if source.host_id != host_id || source.binding_generation != binding_generation {
            return Err(BridgeRejectionCause::ForkedParticipantRouteMismatch);
        }
        let record = self
            .persistence
            .load(&source.mob_id)
            .await
            .map_err(|_| BridgeRejectionCause::Unavailable)?
            .ok_or(BridgeRejectionCause::ForkedParticipantSourceMismatch)?;
        let row = record
            .materialized
            .get(&source.agent_identity)
            .ok_or(BridgeRejectionCause::ForkedParticipantSourceMismatch)?;
        if row.generation != source.generation
            || row.fence_token != source.fence_token
            || row.session_id != source.member_session_id
        {
            return Err(BridgeRejectionCause::StaleFence);
        }
        let session_id = meerkat_core::SessionId::parse(&source.member_session_id)
            .map_err(|_| BridgeRejectionCause::ForkedParticipantSourceMismatch)?;
        let Some(materializer) = self.materializer.as_ref() else {
            return Err(BridgeRejectionCause::Unavailable);
        };
        // A fork is taken at a COMPLETE BOUNDARY, so what must be refused is a
        // run in progress — not executor attachment. Every host-materialized
        // member is executor-attached by construction (ADJ-23: the host owns
        // the residency and holds the session attached between turns), so
        // demanding `Idle` here would refuse every source this host can
        // actually serve. `Attached` is precisely "executor attached, runtime
        // loop alive, waiting for input": a complete boundary.
        if !matches!(
            materializer
                .substrate()
                .runtime_adapter
                .runtime_state(&session_id)
                .await,
            Ok(meerkat_runtime::RuntimeState::Idle | meerkat_runtime::RuntimeState::Attached)
        ) {
            return Err(BridgeRejectionCause::ForkedParticipantBusy);
        }
        Ok(())
    }

    async fn admit_forked_participant_command(
        &mut self,
        candidate: &PeerInputCandidate,
        source: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        epoch: u64,
        binding_generation: u64,
        supervisor: &BridgePeerSpec,
    ) -> bool {
        if !self
            .admit_host_command(
                candidate,
                &source.mob_id,
                epoch,
                binding_generation,
                &supervisor.address,
            )
            .await
        {
            return false;
        }
        let declared = match BridgePeerIdentity::try_from(supervisor) {
            Ok(peer) => peer,
            Err(_) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::InvalidSupervisorSpec,
                    "forked participant supervisor identity is malformed",
                    Some(supervisor.address.as_str()),
                )
                .await;
                return false;
            }
        };
        if !declared_supervisor_matches_recorded_host_authority(
            self.binding_authority.state(),
            &source.mob_id,
            &declared,
        ) {
            self.send_failure(
                candidate,
                BridgeRejectionCause::SenderMismatch,
                "forked participant declared supervisor does not match host authority",
                Some(supervisor.address.as_str()),
            )
            .await;
            return false;
        }
        match self
            .validate_forked_participant_source(source, binding_generation)
            .await
        {
            Ok(()) => true,
            Err(cause) => {
                self.send_failure(
                    candidate,
                    cause,
                    "forked participant source does not match current durable host residency",
                    Some(supervisor.address.as_str()),
                )
                .await;
                false
            }
        }
    }

    async fn serve_create_forked_participant(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgeCreateForkedParticipantPayload,
    ) {
        if !payload.protocol_version.supports_forked_participants() {
            self.send_failure(
                candidate,
                BridgeRejectionCause::ForkedParticipantProtocolUnsupported,
                "forked participant operations require supervisor bridge V6",
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        }
        if !self
            .admit_forked_participant_command(
                candidate,
                &payload.source_member,
                payload.epoch,
                payload.binding_generation,
                &payload.supervisor,
            )
            .await
        {
            return;
        }
        let Some(service) = self.forked_participant_service.as_ref() else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::ForkedParticipantProtocolUnsupported,
                "host does not compose forked participant capability service",
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        };
        let request = match (
            ForkedParticipantRequestId::new(&payload.request_id),
            meerkat_core::SessionId::parse(&payload.source_member.member_session_id),
            payload.prefix_message_count.map(usize::try_from).transpose(),
        ) {
            (Ok(request_id), Ok(source_session_id), Ok(prefix_message_count)) => {
                ForkedParticipantRequest {
                    request_id,
                    source_identity: crate::ids::AgentIdentity::from(
                        payload.source_member.agent_identity.clone(),
                    ),
                    source_session_id,
                    owner_route: service.owner_route().clone(),
                    prefix_message_count,
                    scope: match payload.scope {
                        meerkat_contracts::wire::supervisor_bridge::BridgeForkedParticipantScope::Invoke => ForkedParticipantOperationScope::Invoke,
                        meerkat_contracts::wire::supervisor_bridge::BridgeForkedParticipantScope::Observe => ForkedParticipantOperationScope::Observe,
                        meerkat_contracts::wire::supervisor_bridge::BridgeForkedParticipantScope::InvokeAndObserve => ForkedParticipantOperationScope::InvokeAndObserve,
                    },
                    reuse: match payload.reuse {
                        meerkat_contracts::wire::supervisor_bridge::BridgeForkedParticipantReuse::OneShot => ForkedParticipantReusePolicy::OneShot,
                        meerkat_contracts::wire::supervisor_bridge::BridgeForkedParticipantReuse::BoundedReuse { max_uses } => ForkedParticipantReusePolicy::BoundedReuse { max_uses },
                    },
                    ttl: Duration::from_millis(payload.ttl_millis),
                }
            }
            _ => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::ForkedParticipantTampered,
                    "forked participant request has invalid identity or count",
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
        };
        match service.create(&request, chrono::Utc::now()).await {
            Ok(reference) => {
                self.send_reply(
                    candidate,
                    HostBridgeReply::completed(BridgeReply::ForkedParticipantCreated(
                        BridgeForkedParticipantCreatedResponse {
                            capability: bridge_ref(&reference),
                        },
                    )),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
            }
            Err(error) => {
                self.send_failure(
                    candidate,
                    forked_participant_error_cause(&error),
                    "forked participant create rejected",
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
            }
        }
    }

    async fn serve_revoke_forked_participant(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgeRevokeForkedParticipantPayload,
    ) {
        if !payload.protocol_version.supports_forked_participants() {
            self.send_failure(
                candidate,
                BridgeRejectionCause::ForkedParticipantProtocolUnsupported,
                "forked participant operations require supervisor bridge V6",
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        }
        if !self
            .admit_host_command(
                candidate,
                &payload.source_member.mob_id,
                payload.epoch,
                payload.binding_generation,
                &payload.supervisor.address,
            )
            .await
        {
            return;
        }
        let declared = match BridgePeerIdentity::try_from(&payload.supervisor) {
            Ok(peer) => peer,
            Err(_) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::InvalidSupervisorSpec,
                    "forked participant supervisor identity is malformed",
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
        };
        if !declared_supervisor_matches_recorded_host_authority(
            self.binding_authority.state(),
            &payload.source_member.mob_id,
            &declared,
        ) {
            self.send_failure(
                candidate,
                BridgeRejectionCause::SenderMismatch,
                "forked participant declared supervisor does not match host authority",
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        }
        let Some(service) = self.forked_participant_service.as_ref() else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::ForkedParticipantProtocolUnsupported,
                "host does not compose forked participant capability service",
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        };
        let reference = match domain_ref(&payload.capability) {
            Ok(reference) if reference.owner_route() == service.owner_route() => reference,
            Ok(_) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::ForkedParticipantRouteMismatch,
                    "forked participant reference names another owner route",
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
            Err(_) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::ForkedParticipantTampered,
                    "forked participant reference is malformed or tampered",
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
        };
        if reference.source_identity().as_str() != payload.source_member.agent_identity
            || reference.provenance().source_session_id.to_string()
                != payload.source_member.member_session_id
        {
            self.send_failure(
                candidate,
                BridgeRejectionCause::ForkedParticipantSourceMismatch,
                "forked participant reference names a different source",
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        }
        match service.revoke(reference.capability_id(), true).await {
            Ok(outcome) => {
                let outcome = match outcome {
                    crate::forked_participant::ForkedParticipantRevocationOutcome::Revoked {
                        cleanup_pending,
                    } => BridgeForkedParticipantRevocationOutcome::Revoked { cleanup_pending },
                    crate::forked_participant::ForkedParticipantRevocationOutcome::PendingAttachedRelease => {
                        BridgeForkedParticipantRevocationOutcome::PendingAttachedRelease
                    }
                    crate::forked_participant::ForkedParticipantRevocationOutcome::Converged => {
                        BridgeForkedParticipantRevocationOutcome::Converged
                    }
                };
                self.send_reply(
                    candidate,
                    HostBridgeReply::completed(BridgeReply::ForkedParticipantRevoked(
                        BridgeForkedParticipantRevokedResponse { outcome },
                    )),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
            }
            Err(error) => {
                self.send_failure(
                    candidate,
                    forked_participant_error_cause(&error),
                    "forked participant revoke rejected",
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
            }
        }
    }

    async fn serve_materialize_member_candidate(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgeMaterializePayload,
    ) {
        let reply_address = payload.supervisor.address.clone();
        let supervisor = match BridgePeerIdentity::try_from(&payload.supervisor) {
            Ok(supervisor) => supervisor,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::InvalidSupervisorSpec,
                    format!("materialize member failed: invalid supervisor peer spec: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };
        let Some(host_id) = self.host_comms.peer_id().map(|peer_id| peer_id.to_string()) else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                "materialize member failed: host runtime peer_id unavailable",
                Some(reply_address.as_str()),
            )
            .await;
            return;
        };

        // Rung 0: the COMMAND-level admission keys on spec.mob_id (the
        // phase-2 adjudication — no duplicate sibling exists).
        if !self
            .admit_host_command(
                candidate,
                &payload.spec.mob_id,
                payload.epoch,
                payload.binding_generation,
                &reply_address,
            )
            .await
        {
            return;
        }

        // The command signer was admitted against the durable host binding
        // above. Bind the payload's supervisor descriptor to that same
        // authority before it can seed member-side trust: a current
        // supervisor may not delegate control by carrying a different peer.
        if !declared_supervisor_matches_recorded_host_authority(
            self.binding_authority.state(),
            &payload.spec.mob_id,
            &supervisor,
        ) {
            self.send_failure(
                candidate,
                BridgeRejectionCause::SenderMismatch,
                "materialize member rejected: declared supervisor does not match the durable \
                 authenticated host authority",
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }

        // Shell integrity, pre-machine (T9): the RECOMPUTED digest is the
        // only value that ever enters `ResolveMaterializeAdmission` — never
        // the carried sibling.
        let recomputed_digest = match portable_member_spec_digest(&payload.spec) {
            Ok(digest) => digest,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("materialize member failed: spec digest recompute: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };
        if recomputed_digest != payload.spec_digest {
            self.send_failure(
                candidate,
                BridgeRejectionCause::SpecDigestMismatch,
                "materialize member rejected: the received spec does not hash to the digest \
                 the command carried",
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }

        if self.materializer.is_none() {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Unavailable,
                "host composed without a member substrate",
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }

        let member_key = MemberKey::new(
            AuthorityMobId::from(payload.spec.mob_id.as_str()),
            AuthorityAgentIdentity::from(payload.spec.agent_identity.as_str()),
        );

        // CONTAINMENT GATE (issue #159), ahead of dedup replay, preflight, and
        // any build effect.
        //
        // A fork child's session id is VISIBLE: it rides in the capability's
        // own provenance, in this host's durable rows, and in every reply that
        // names the residency. An ordinary `Resume` addresses a session by id
        // alone, so without this gate a caller who merely LEARNED the id could
        // materialize the branch and never present the bearer capability at
        // all — the authenticated attachment would be decorative.
        //
        // Protection is decided by OWNER truth (the durable capability record
        // keyed by fork child session), never by anything the caller supplied.
        // A session no record claims is not protected and its `Resume` is
        // completely unchanged.
        if let Err((cause, reason)) = self.enforce_fork_session_containment(&payload).await {
            self.send_failure(candidate, cause, reason, Some(reply_address.as_str()))
                .await;
            return;
        }

        // Dedup admission (A12: one idempotency key never names two builds).
        let admission = match resolve_materialize_admission(
            &mut self.binding_authority,
            &member_key,
            payload.generation,
            payload.fence_token,
            &recomputed_digest,
        ) {
            Ok(admission) => admission,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("materialize admission failed: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };

        let superseded_session_id = match admission {
            MaterializeAdmission::Rejected { kind } => {
                let (cause, reason) =
                    materialize_reject_wire_cause(kind, &payload.spec, None, None);
                self.send_failure(candidate, cause, reason, Some(reply_address.as_str()))
                    .await;
                return;
            }
            MaterializeAdmission::Replay {
                session_id,
                spec_digest,
            } => {
                self.serve_materialize_replay(
                    candidate,
                    &payload,
                    &supervisor,
                    &member_key,
                    &session_id,
                    &spec_digest,
                    &reply_address,
                )
                .await;
                return;
            }
            MaterializeAdmission::Admitted {
                superseded_session_id,
            } => superseded_session_id,
        };

        // Global host-authority ownership: a durable SessionId may name only
        // one member key across every mob served by this daemon. Check before
        // preflight/build/rebind side effects; the machine's injectivity
        // invariant independently fail-closes the eventual record commit.
        if let MaterializeLaunchMode::Resume { session_id, .. } = &payload.launch
            && let Some((owner, _)) = self
                .binding_authority
                .state()
                .materialized_sessions
                .iter()
                .find(|(owner, recorded)| recorded.0 == *session_id && *owner != &member_key)
        {
            let cause = BridgeRejectionCause::SessionOwnershipConflict {
                session_id: session_id.clone(),
            };
            tracing::warn!(
                requested_mob_id = %member_key.mob_id.0,
                requested_agent_identity = %member_key.agent_identity.0,
                owner_mob_id = %owner.mob_id.0,
                owner_agent_identity = %owner.agent_identity.0,
                session_id = %session_id,
                "cross-owner Resume rejected by host session injectivity"
            );
            self.send_failure(
                candidate,
                cause,
                format!("resume session '{session_id}' is already owned by another host member"),
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }

        // Tier-2 preflight (§14 R7): shell-read facts, machine verdict.
        let observations = {
            let Some(materializer) = self.materializer.as_ref() else {
                return; // checked above; unreachable by construction
            };
            match assemble_preflight_observations(
                &payload.spec,
                &payload.launch,
                payload.protocol_version,
                materializer.substrate(),
                self.capability_facts,
            )
            .await
            {
                Ok(observations) => observations,
                Err(error) => {
                    tracing::warn!(detail = %error, "materialize preflight observation failed locally");
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        "materialize preflight probe failed; inspect host logs",
                        Some(reply_address.as_str()),
                    )
                    .await;
                    return;
                }
            }
        };
        match resolve_materialize_preflight(
            &mut self.binding_authority,
            &member_key,
            payload.generation,
            payload.fence_token,
            &observations,
        ) {
            Ok(MaterializePreflight::Admitted) => {}
            Ok(MaterializePreflight::Rejected { kind }) => {
                let (cause, reason) = materialize_reject_wire_cause(
                    kind,
                    &payload.spec,
                    observations.first_missing_env_key.as_deref(),
                    observations.first_missing_stdio_server.as_deref(),
                );
                self.send_failure(candidate, cause, reason, Some(reply_address.as_str()))
                    .await;
                return;
            }
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("materialize preflight failed: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        }

        // The association the PREVIOUS residency (if any) was materialized
        // under, captured before the replacement row can overwrite it. Read
        // from the pre-filter supersession fact so the same-session ADOPT arm
        // is covered too: adopting a session under a new generation must not
        // leave the old generation's attachment active beside a new one.
        let superseded_association = match superseded_session_id.as_ref() {
            None => None,
            Some(old_session_id) => match self.persistence.load(&payload.spec.mob_id).await {
                Ok(Some(record)) => record
                    .materialized
                    .get(&payload.spec.agent_identity)
                    .filter(|row| &row.session_id == old_session_id)
                    .and_then(|row| row.forked_participant_attachment.clone()),
                Ok(None) => None,
                Err(error) => {
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!(
                            "materialize member failed: superseded row load for capability \
                             association: {error}"
                        ),
                        Some(reply_address.as_str()),
                    )
                    .await;
                    return;
                }
            },
        };

        // Superseding admit (the A20 revival re-materialization): make the
        // old incarnation non-serving before the new build, but retain its
        // durable snapshot until the replacement row AND replacement runtime
        // residency have committed. Any pre-commit failure therefore leaves
        // the old row cold-replayable instead of pointing at an absorbing
        // Archived/Retired snapshot.
        // EXCEPTION (§19.L1): a superseding RESUME naming exactly the
        // superseded session ADOPTS it. Its same-session generation-cutover
        // path owns quiescence and projection-drain sequencing itself.
        let superseded_session_id = superseded_session_id.filter(|old_session_id| {
            !matches!(
                &payload.launch,
                MaterializeLaunchMode::Resume { session_id, .. } if session_id == old_session_id
            )
        });
        let mut superseded_residency_update = None;
        if let Some(old_session_id) = superseded_session_id {
            let quiescence_result: Result<(), String> =
                match meerkat_core::types::SessionId::parse(&old_session_id) {
                    Ok(old_id) => {
                        let runtime_adapter = self.materializer.as_ref().map(|materializer| {
                            Arc::clone(&materializer.substrate().runtime_adapter)
                        });
                        match (runtime_adapter, self.materializer.as_mut()) {
                            (Some(runtime_adapter), Some(materializer)) => {
                                let update = runtime_adapter
                                    .begin_member_residency_update(old_id.clone())
                                    .await;
                                let result = materializer
                                    .quiesce_before_superseding_record(&old_id)
                                    .await;
                                superseded_residency_update = Some((old_id, update));
                                result
                            }
                            // Substrate presence was gated above; nothing to
                            // quiesce without one.
                            _ => Ok(()),
                        }
                    }
                    Err(error) => Err(error.to_string()),
                };
            if let Err(detail) = quiescence_result {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!(
                        "materialize member failed: superseded session '{old_session_id}' \
                         quiescence: {detail}"
                    ),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        }

        let admitted_forked_attachment = match payload.forked_participant_attachment.as_ref() {
            None => None,
            Some(attachment) => {
                let Some(service) = self.forked_participant_service.as_ref() else {
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::ForkedParticipantProtocolUnsupported,
                        "host does not compose forked participant capability service",
                        Some(reply_address.as_str()),
                    )
                    .await;
                    return;
                };
                let reference = match domain_ref(&attachment.capability) {
                    Ok(reference) if reference.owner_route() == service.owner_route() => reference,
                    Ok(_) => {
                        self.send_failure(
                            candidate,
                            BridgeRejectionCause::ForkedParticipantRouteMismatch,
                            "forked participant attachment names another host route",
                            Some(reply_address.as_str()),
                        )
                        .await;
                        return;
                    }
                    Err(_) => {
                        self.send_failure(
                            candidate,
                            BridgeRejectionCause::ForkedParticipantTampered,
                            "forked participant attachment reference is malformed",
                            Some(reply_address.as_str()),
                        )
                        .await;
                        return;
                    }
                };
                let session_matches = matches!(
                    &payload.launch,
                    MaterializeLaunchMode::Resume { session_id, .. }
                        if session_id == reference.fork_session_id().to_string().as_str()
                );
                if !session_matches {
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::ForkedParticipantSourceMismatch,
                        "forked participant attachment must resume its exact fork session",
                        Some(reply_address.as_str()),
                    )
                    .await;
                    return;
                }
                let attachment_id =
                    match ForkedParticipantAttachmentId::new(&attachment.attachment_id) {
                        Ok(id) => id,
                        Err(_) => {
                            self.send_failure(
                                candidate,
                                BridgeRejectionCause::ForkedParticipantTampered,
                                "forked participant attachment id is malformed",
                                Some(reply_address.as_str()),
                            )
                            .await;
                            return;
                        }
                    };
                let association =
                    ForkedParticipantAttachmentAssociation::new(reference, attachment_id);
                // Refuse before the attach, not after: admitting a second
                // attachment for a capability whose previous residency still
                // holds one would leave two active attachments for one
                // capability with no path that releases both.
                if superseded_association
                    .as_ref()
                    .is_some_and(|superseded| superseded == &association)
                {
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::ForkedParticipantBusy,
                        "forked participant attachment is already held by the residency this \
                         materialization would replace",
                        Some(reply_address.as_str()),
                    )
                    .await;
                    return;
                }
                match service
                    .attach(
                        &association.capability,
                        &association.attachment_id,
                        true,
                        chrono::Utc::now(),
                    )
                    .await
                {
                    Ok(_) => {
                        // A capability takes its branch BEFORE any target
                        // member exists, so the fork child commits with no
                        // member binding and an ordinary Resume would refuse
                        // it ("missing durable comms_name"). Seat it as this
                        // member through the narrow owner-side seam — identity
                        // only, never tool/auth/realm/transcript state — which
                        // is idempotent on the exact binding and refuses a
                        // different one. This happens AFTER the attach is
                        // admitted, so an unauthenticated caller can never
                        // reach it, and before the build, so the build sees an
                        // ordinary resumable member session.
                        if let Some(runtime) = self
                            .materializer
                            .as_ref()
                            .and_then(|m| m.substrate().forked_participant_source_runtime.clone())
                            && let Err(error) = runtime
                                .bind_fork_session_to_member(
                                    association.capability.fork_session_id(),
                                    &payload.spec.mob_id,
                                    payload.spec.profile_name.as_str(),
                                    &payload.spec.agent_identity,
                                )
                                .await
                        {
                            let detail = format!(
                                "forked participant branch could not be seated as a member:                                  {error}"
                            );
                            if let Err(failure) =
                                release_forked_participant_association(Some(service), &association)
                                    .await
                            {
                                self.send_failure(
                                    candidate,
                                    BridgeRejectionCause::ForkedParticipantCleanupDebt,
                                    format!("{detail}; the admitted attachment could not be                                              released either ({failure})"),
                                    Some(reply_address.as_str()),
                                )
                                .await;
                                return;
                            }
                            self.send_failure(
                                candidate,
                                BridgeRejectionCause::Internal,
                                detail,
                                Some(reply_address.as_str()),
                            )
                            .await;
                            return;
                        }
                        Some(association)
                    }
                    Err(error) => {
                        self.send_failure(
                            candidate,
                            forked_participant_error_cause(&error),
                            "forked participant attachment rejected",
                            Some(reply_address.as_str()),
                        )
                        .await;
                        return;
                    }
                }
            }
        };

        // Build (shell effect work, inline on the owner task — DEC-P3H-1).
        let mut outcome = {
            let Some(materializer) = self.materializer.as_mut() else {
                return; // checked above; unreachable by construction
            };
            match materializer
                .materialize(
                    &payload.spec,
                    &payload.launch,
                    MaterializeServingContext {
                        generation: payload.generation,
                        fence_token: payload.fence_token,
                        host_id: host_id.as_str(),
                        host_binding_generation: payload.binding_generation,
                        supervisor: &supervisor,
                        epoch: payload.epoch,
                    },
                )
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    // Failures record NOTHING machine-side (successes only);
                    // the materializer already quiesced the pre-registered
                    // volatile incarnation. If that cleanup could not prove
                    // every unrecorded carrier inert, continuing would let later
                    // commands mutate authority beside an unenumerable live
                    // runtime. Keep the rejection sender alive, but make the
                    // restart boundary sticky before replying.
                    let requires_fail_stop = error.requires_actor_fail_stop();
                    tracing::warn!(detail = %error, "member materialization failed locally");
                    let (cause, reason) = error.wire_cause();
                    // The admitted attachment is compensated ONLY when the
                    // failure definitively proves no member side effects
                    // survive. The fail-stop class is exactly the ambiguous
                    // one, so releasing there would assert an absence this
                    // host cannot observe; it durably retains the obligation
                    // instead and lets boot reconciliation decide.
                    if let Some(association) = admitted_forked_attachment.as_ref() {
                        let compensation = if requires_fail_stop {
                            Err(ForkedParticipantReleaseFailure {
                                cause: BridgeRejectionCause::ForkedParticipantCleanupDebt,
                                detail: "member build failed in an ambiguous class; the \
                                         attachment is retained for reconciliation rather than \
                                         released"
                                    .to_string(),
                            })
                        } else {
                            release_forked_participant_association(
                                self.forked_participant_service.as_ref(),
                                association,
                            )
                            .await
                        };
                        if let Err(failure) = compensation {
                            let obligation_cause = if requires_fail_stop {
                                ForkedParticipantObligationCause::AmbiguousBuild
                            } else {
                                ForkedParticipantObligationCause::ReleaseUnproven
                            };
                            let retained = self
                                .retain_forked_participant_obligation(
                                    &payload.spec.mob_id,
                                    &payload.spec.agent_identity,
                                    association,
                                    obligation_cause,
                                    &failure.detail,
                                )
                                .await;
                            // The originating typed build error is preserved
                            // in the reason: cleanup debt is additional truth
                            // about the same failure, never a replacement for
                            // why the build failed.
                            let detail = match retained {
                                Ok(()) => format!(
                                    "{reason}; capability attachment reconciliation is durably \
                                     retained ({failure})"
                                ),
                                Err(ref retain_error) => format!(
                                    "{reason}; capability attachment reconciliation could not be \
                                     retained ({failure}; {retain_error})"
                                ),
                            };
                            if requires_fail_stop || retained.is_err() {
                                self.durable_uncertainty_fail_stop = Some(detail.clone());
                            }
                            self.send_failure(
                                candidate,
                                BridgeRejectionCause::ForkedParticipantCleanupDebt,
                                detail,
                                Some(reply_address.as_str()),
                            )
                            .await;
                            return;
                        }
                    }
                    if requires_fail_stop {
                        self.durable_uncertainty_fail_stop = Some(reason.clone());
                    }
                    self.send_failure(candidate, cause, reason, Some(reply_address.as_str()))
                        .await;
                    return;
                }
            }
        };

        // Record (persist-before-commit, DEC-P3H-9), then register the
        // identity under the transition witness, then reply.
        let row = MaterializedMemberRow {
            generation: payload.generation,
            generation_start_seq: outcome.generation_start_seq,
            fence_token: payload.fence_token,
            session_id: outcome.session_id.to_string(),
            spec_digest: recomputed_digest.clone(),
            spec: payload.spec.clone(),
            engine_version_at_build: meerkat_contracts::ContractVersion::CURRENT.to_string(),
            member_pubkey: outcome.member_pubkey.clone(),
            member_peer_id: outcome.member_peer_id.clone(),
            launch_outcome: outcome.launch_outcome,
            resolved_auth_binding: outcome.resolved_auth_binding.clone(),
            supervisor_name: payload.supervisor.name.clone(),
            supervisor_address: payload.supervisor.address.clone(),
            forked_participant_attachment: None,
        };
        // Supersession compensation, immediately before the replacement row
        // commits: the old residency has already been quiesced, so its
        // attachment must not outlive it. Failure ABORTS the replacement —
        // nothing is recorded, and the attachment admitted for the new build
        // is released again so the abort leaves exactly one (the old) active.
        if let Some(superseded) = superseded_association
            .as_ref()
            .filter(|superseded| Some(*superseded) != admitted_forked_attachment.as_ref())
            && let Err(failure) = release_forked_participant_association(
                self.forked_participant_service.as_ref(),
                superseded,
            )
            .await
        {
            let mut detail = format!(
                "materialize member aborted: the superseded residency's capability attachment \
                 could not be released ({failure})"
            );
            if let Some(association) = admitted_forked_attachment.as_ref()
                && let Err(rollback) = release_forked_participant_association(
                    self.forked_participant_service.as_ref(),
                    association,
                )
                .await
            {
                let retained = self
                    .retain_forked_participant_obligation(
                        &payload.spec.mob_id,
                        &payload.spec.agent_identity,
                        association,
                        ForkedParticipantObligationCause::ReleaseUnproven,
                        &rollback.detail,
                    )
                    .await;
                detail = match retained {
                    Ok(()) => format!(
                        "{detail}; the replacement attachment could not be rolled back either \
                         ({rollback}) and is durably retained for reconciliation"
                    ),
                    Err(retain_error) => {
                        let detail = format!(
                            "{detail}; the replacement attachment could not be rolled back \
                             ({rollback}) nor durably retained ({retain_error})"
                        );
                        self.durable_uncertainty_fail_stop = Some(detail.clone());
                        detail
                    }
                };
            }
            if let Some(publication) = outcome.runtime_publication.take() {
                if let Err(error) = publication.abort().await {
                    let aborted =
                        format!("{detail}; exact unpublished attachment abort failed: {error}");
                    self.durable_uncertainty_fail_stop = Some(aborted.clone());
                    detail = aborted;
                }
                if let Some(materializer) = self.materializer.as_mut() {
                    materializer.forget_runtime_after_exact_publication_abort(&outcome.session_id);
                }
            }
            self.send_failure(
                candidate,
                BridgeRejectionCause::ForkedParticipantCleanupDebt,
                detail,
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }
        // The session document is already durable before this host-row CAS.
        // Publication failure or caller loss may quiesce only the exact volatile
        // incarnation; the session remains discoverable and explicitly resumable.
        if outcome.runtime_publication.is_none() {
            let failure =
                "materialized build lost its exact unpublished attachment lease before durable record"
                    .to_string();
            self.durable_uncertainty_fail_stop = Some(failure.clone());
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                failure,
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }
        let witness = match record_materialized_member(
            &mut self.binding_authority,
            self.persistence.as_ref(),
            &member_key,
            row,
            admitted_forked_attachment.clone(),
        )
        .await
        {
            Ok(witness) => witness,
            Err(error) => {
                // Regardless of whether the host-row CAS definitely missed or
                // became ambiguous, the already-durable session is not rollback
                // compensation. Quiesce only this exact attachment, actor, and
                // sidecar; discovery/resume owns retry of the preserved session.
                let durable_uncertainty = error.is_durable_uncertainty();
                let publication_abort_failure =
                    match outcome.runtime_publication.take() {
                        Some(publication) => publication.abort().await.err().map(|error| {
                            format!("exact unpublished attachment abort failed: {error}")
                        }),
                        None => Some(
                            "materialized build lost its exact unpublished attachment lease"
                                .to_string(),
                        ),
                    };
                if let Some(materializer) = self.materializer.as_mut() {
                    materializer.forget_runtime_after_exact_publication_abort(&outcome.session_id);
                }
                let cleanup_failure = publication_abort_failure;
                let cleanup_uncertain = cleanup_failure.is_some();
                let mut failure = match (durable_uncertainty, cleanup_failure) {
                    (true, Some(cleanup)) => format!(
                        "materialize record became durably uncertain ({error}); attempted runtime quiescence also failed: {cleanup}"
                    ),
                    (true, None) => format!(
                        "materialize record became durably uncertain ({error}); runtime quiesced and cold recovery is required"
                    ),
                    (false, Some(cleanup)) => format!(
                        "materialize record persistence failed ({error}); exact runtime quiescence failed while the durable session remained resumable: {cleanup}"
                    ),
                    (false, None) => format!(
                        "materialize record persistence failed ({error}); exact runtime quiesced and the durable session remains resumable"
                    ),
                };
                // The attachment was admitted but no row provably adopted it.
                // A durably-uncertain CAS may in fact have landed the
                // association, so releasing would be a guess; retain the
                // obligation instead. A definite miss licenses compensation,
                // and an unproven compensation degrades to the same
                // obligation rather than to silence.
                if let Some(association) = admitted_forked_attachment.as_ref() {
                    let compensation = if durable_uncertainty {
                        Err(ForkedParticipantReleaseFailure {
                            cause: BridgeRejectionCause::ForkedParticipantCleanupDebt,
                            detail: "materialized-row write is durably uncertain; the attachment \
                                     is retained for reconciliation rather than released"
                                .to_string(),
                        })
                    } else {
                        release_forked_participant_association(
                            self.forked_participant_service.as_ref(),
                            association,
                        )
                        .await
                    };
                    if let Err(release_failure) = compensation {
                        let obligation_cause = if durable_uncertainty {
                            ForkedParticipantObligationCause::RecordUncertain
                        } else {
                            ForkedParticipantObligationCause::ReleaseUnproven
                        };
                        failure = match self
                            .retain_forked_participant_obligation(
                                &payload.spec.mob_id,
                                &payload.spec.agent_identity,
                                association,
                                obligation_cause,
                                &release_failure.detail,
                            )
                            .await
                        {
                            Ok(()) => format!(
                                "{failure}; capability attachment reconciliation is durably \
                                 retained ({release_failure})"
                            ),
                            Err(retain_error) => {
                                self.durable_uncertainty_fail_stop = Some(failure.clone());
                                format!(
                                    "{failure}; capability attachment reconciliation could not be \
                                     retained ({release_failure}; {retain_error})"
                                )
                            }
                        };
                    }
                }
                if durable_uncertainty || cleanup_uncertain {
                    self.durable_uncertainty_fail_stop = Some(failure.clone());
                }
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    failure,
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };
        let incarnation = meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
            mob_id: payload.spec.mob_id.clone(),
            agent_identity: payload.spec.agent_identity.clone(),
            host_id: host_id.clone(),
            binding_generation: payload.binding_generation,
            member_session_id: outcome.session_id.to_string(),
            generation: payload.generation,
            fence_token: payload.fence_token,
        };
        let tracked_turn_journal: Option<
            Arc<dyn meerkat_runtime::member_observation::TrackedTurnJournal>,
        > = self
            .materializer
            .as_ref()
            .is_some_and(|materializer| materializer.substrate().durable_event_log.is_some())
            .then(|| {
                Arc::new(HostTrackedTurnJournal::new(
                    incarnation.clone(),
                    self.observation_record_tx.clone(),
                ))
                    as Arc<dyn meerkat_runtime::member_observation::TrackedTurnJournal>
            });
        let publication_result = match outcome.runtime_publication.take() {
            Some(publication) => publication
                .commit_serving_with_residency(
                    outcome.residency_update,
                    incarnation.clone(),
                    tracked_turn_journal,
                )
                .await
                .map(|(_, residency)| residency),
            None => Err(crate::MobError::Internal(
                "materialized build lost its exact host publication lease".to_string(),
            )),
        };
        let residency_publication = match publication_result {
            Ok(publication) => publication,
            Err(error) => {
                // The member row is already durable and names the attempted
                // incarnation. Preserve that snapshot and force the exact
                // unpublished attachment inert. Neither snapshot may be
                // archived until cold recovery has reread the committed row.
                if let Some(materializer) = self.materializer.as_mut() {
                    materializer.forget_runtime_after_exact_publication_abort(&outcome.session_id);
                }
                let failure = format!(
                    "materialized row committed but exact runtime residency publication failed ({error}); exact publication cleanup ran under the retained boundary and cold recovery is required"
                );
                self.durable_uncertainty_fail_stop = Some(failure.clone());
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    failure,
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };
        // The replacement's exact residency must be visible before the old
        // snapshot loses recovery authority. The old update has held its
        // stable slot closed throughout the build and durable-row CAS.
        let superseded_publication = if let Some((old_session_id, old_update)) =
            superseded_residency_update.take()
        {
            let old_publication = match old_update.vacate() {
                Ok(publication) => publication,
                Err(error) => {
                    let failure = format!(
                        "replacement row and residency committed, but superseded residency vacancy publication failed ({error}); cold recovery is required"
                    );
                    self.durable_uncertainty_fail_stop = Some(failure.clone());
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        failure,
                        Some(reply_address.as_str()),
                    )
                    .await;
                    return;
                }
            };
            Some((old_session_id, old_publication))
        } else {
            None
        };
        let incarnation_session_id = outcome.session_id.clone();
        if let Some((old_session_id, _)) = &superseded_publication {
            self.registered_member_incarnations.remove(old_session_id);
        }
        self.registered_member_incarnations
            .insert(incarnation_session_id, incarnation);
        self.unrevived.remove(&(
            payload.spec.mob_id.clone(),
            payload.spec.agent_identity.clone(),
        ));
        self.generation_start_seqs.insert(
            (
                payload.spec.mob_id.clone(),
                payload.spec.agent_identity.clone(),
            ),
            outcome.generation_start_seq,
        );
        self.sync_member_observation().await;
        drop(residency_publication);
        let superseded_session_to_dispose = superseded_publication
            .as_ref()
            .map(|(session_id, _)| session_id.clone());
        drop(superseded_publication);
        if let Some(superseded_session_id) = superseded_session_to_dispose {
            let cleanup_failure = match self.materializer.as_mut() {
                Some(materializer) => materializer
                    .dispose_after_superseding_commit(&superseded_session_id)
                    .await
                    .err(),
                None => Some("host materializer disappeared after replacement commit".to_string()),
            };
            if let Some(cleanup) = cleanup_failure {
                // The replacement is authoritative and published, while the
                // old runtime was already quiesced before the cutover. Stop
                // every mutation lane until restart rather than continuing
                // with an orphaned superseded snapshot.
                let failure = format!(
                    "replacement row and residency committed, but superseded session \
                     '{superseded_session_id}' cleanup failed ({cleanup}); cold recovery is required"
                );
                self.durable_uncertainty_fail_stop = Some(failure.clone());
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    failure,
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        }
        let live = crate::runtime::host_materialize::LiveMemberRuntime {
            runtime: Arc::clone(&outcome.member_runtime),
            ack_keypair: Arc::clone(&outcome.ack_keypair),
        };
        if let Err(error) = self.register_member_identity(&live, &witness) {
            // The row is durable and committed: the supervisor retries into
            // MaterializeReplay, which re-attempts registration idempotently.
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                format!("member identity registration failed: {error}"),
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }
        let reply = BridgeReply::MemberMaterialized(BridgeMaterializedResponse {
            member_pubkey: outcome.member_pubkey,
            member_peer_id: outcome.member_peer_id,
            advertised_address: self.advertised_address.clone(),
            session_id: outcome.session_id.to_string(),
            spec_digest: recomputed_digest,
            engine_version: meerkat_contracts::ContractVersion::CURRENT.to_string(),
            launch_outcome: outcome.launch_outcome,
            resolved_auth_binding: outcome.resolved_auth_binding,
        });
        self.send_reply(
            candidate,
            HostBridgeReply::completed(reply),
            Some(reply_address.as_str()),
        )
        .await;
    }

    /// ADJ-9 ensure-on-replay: verify the durable row corroborates the
    /// replay effect, recompose the member if its session is dead
    /// (recompose-if-dead from the durable spec row), re-attempt idempotent
    /// registration, then ack the RECORDED response verbatim.
    #[allow(clippy::too_many_arguments)]
    async fn serve_materialize_replay(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: &BridgeMaterializePayload,
        supervisor: &BridgePeerIdentity,
        member_key: &MemberKey,
        recorded_session_id: &str,
        recorded_spec_digest: &str,
        reply_address: &str,
    ) {
        let mob_id = member_key.mob_id.0.clone();
        let identity = member_key.agent_identity.0.clone();
        let Some(host_id) = self.host_comms.peer_id().map(|peer_id| peer_id.to_string()) else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                "materialize replay failed: host runtime peer_id unavailable",
                Some(reply_address),
            )
            .await;
            return;
        };
        let record = match self.persistence.load(&mob_id).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("durable host binding rows diverged: no row for mob '{mob_id}'"),
                    Some(reply_address),
                )
                .await;
                return;
            }
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("materialize replay row load failed: {error}"),
                    Some(reply_address),
                )
                .await;
                return;
            }
        };
        let Some(row) = record.materialized.get(&identity).cloned() else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                format!(
                    "durable host binding rows diverged: no materialized row for '{identity}' \
                     of mob '{mob_id}'"
                ),
                Some(reply_address),
            )
            .await;
            return;
        };
        if let Err(error) =
            validate_durable_materialized_member_row(&mob_id, &identity, &record, &row)
        {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                format!("materialize replay rejected corrupt durable row: {error}"),
                Some(reply_address),
            )
            .await;
            return;
        }
        if row.session_id != recorded_session_id || row.spec_digest != recorded_spec_digest {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                format!(
                    "durable materialized row for '{identity}' diverged from the machine's \
                     replay record"
                ),
                Some(reply_address),
            )
            .await;
            return;
        }
        // Replay is answer-only. The recorded association is compared, never
        // re-attached: a matching command replays its recorded result with
        // zero capability lifecycle effect, and a differing one (in either
        // direction, including presence against absence) is refused before it
        // can mutate anything.
        if !replayed_forked_participant_attachment_matches(
            payload.forked_participant_attachment.as_ref(),
            row.forked_participant_attachment.as_ref(),
        ) {
            self.send_failure(
                candidate,
                BridgeRejectionCause::SpecDigestMismatch,
                format!(
                    "materialize replay for '{identity}' presents a different capability \
                     attachment than the recorded residency; the idempotency key names one \
                     command only"
                ),
                Some(reply_address),
            )
            .await;
            return;
        }

        let observation_record_tx = self.observation_record_tx.clone();
        let tracked_turns_supported = self
            .materializer
            .as_ref()
            .is_some_and(|materializer| materializer.substrate().durable_event_log.is_some());
        if let Some(materializer) = self.materializer.as_mut() {
            let session_id = match meerkat_core::types::SessionId::parse(&row.session_id) {
                Ok(session_id) => session_id,
                Err(error) => {
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("recorded session id is invalid: {error}"),
                        Some(reply_address),
                    )
                    .await;
                    return;
                }
            };
            // Recompose-if-dead from the durable spec row; a healthy live
            // runtime returns idempotently but still repairs a missing exact
            // residency registration under the same transaction API.
            let supervisor_desc = supervisor.clone().into_trusted_peer_descriptor();
            let (member, committed_incarnation, residency_publication) = match materializer
                .revive_from_row(
                    &row.spec,
                    &row.session_id,
                    &row.member_pubkey,
                    row.generation,
                    row.fence_token,
                    host_id.as_str(),
                    payload.binding_generation,
                    &supervisor_desc,
                    payload.epoch,
                )
                .await
            {
                Ok(mut outcome) => {
                    let incarnation =
                        meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
                            mob_id: mob_id.clone(),
                            agent_identity: identity.clone(),
                            host_id: host_id.clone(),
                            binding_generation: payload.binding_generation,
                            member_session_id: row.session_id.clone(),
                            generation: row.generation,
                            fence_token: row.fence_token,
                        };
                    let journal: Option<
                        Arc<dyn meerkat_runtime::member_observation::TrackedTurnJournal>,
                    > = tracked_turns_supported.then(|| {
                        Arc::new(HostTrackedTurnJournal::new(
                            incarnation.clone(),
                            observation_record_tx.clone(),
                        ))
                            as Arc<dyn meerkat_runtime::member_observation::TrackedTurnJournal>
                    });
                    let residency_publication = match commit_revived_member_publication(
                        &mut outcome,
                        incarnation.clone(),
                        journal,
                    )
                    .await
                    {
                        Ok(publication) => publication,
                        Err(error) => {
                            self.send_failure(
                                candidate,
                                BridgeRejectionCause::Internal,
                                format!("materialize replay residency repair failed: {error}"),
                                Some(reply_address),
                            )
                            .await;
                            return;
                        }
                    };
                    (
                        Some(outcome.member),
                        Some(incarnation),
                        residency_publication,
                    )
                }
                Err(error) => {
                    self.unrevived.insert((mob_id.clone(), identity.clone()));
                    tracing::warn!(detail = %error, "materialize replay ensure failed locally");
                    let (cause, reason) = error.wire_cause();
                    self.send_failure(
                        candidate,
                        cause,
                        format!("materialize replay ensure failed: {reason}"),
                        Some(reply_address),
                    )
                    .await;
                    return;
                }
            };
            if let Some(incarnation) = committed_incarnation {
                self.registered_member_incarnations
                    .insert(session_id.clone(), incarnation);
            }
            // Exact replay can be the first same-process projection after a
            // durable materialize record won but residency publication (or
            // the process-local cache update immediately after it) failed.
            // Seed the event floor from the durable row before publishing so
            // that first repaired projection can never fall back to seq 1.
            self.generation_start_seqs
                .insert((mob_id.clone(), identity.clone()), row.generation_start_seq);
            self.sync_member_observation().await;
            drop(residency_publication);
            // Re-attempt idempotent registration (the member may have been
            // deregistered by a crash between commit and register). The
            // replay transition is the witness.
            if let Some(member) = member {
                let witness = MaterializedIdentityWitness::from_recovered_state(
                    self.binding_authority.state(),
                    member_key,
                );
                match witness {
                    Ok(witness) => {
                        if let Err(error) = self.register_member_identity(&member, &witness) {
                            self.send_failure(
                                candidate,
                                BridgeRejectionCause::Internal,
                                format!("member identity re-registration failed: {error}"),
                                Some(reply_address),
                            )
                            .await;
                            return;
                        }
                    }
                    Err(error) => {
                        self.send_failure(
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!("materialize replay witness failed: {error}"),
                            Some(reply_address),
                        )
                        .await;
                        return;
                    }
                }
                self.unrevived.remove(&(mob_id.clone(), identity.clone()));
            }
        }

        // The RECORDED ack, verbatim (including engine_version_at_build).
        let reply = BridgeReply::MemberMaterialized(BridgeMaterializedResponse {
            member_pubkey: row.member_pubkey,
            member_peer_id: row.member_peer_id,
            advertised_address: self.advertised_address.clone(),
            session_id: row.session_id,
            spec_digest: row.spec_digest,
            engine_version: row.engine_version_at_build,
            launch_outcome: row.launch_outcome,
            resolved_auth_binding: row.resolved_auth_binding,
        });
        self.send_reply(
            candidate,
            HostBridgeReply::completed(reply),
            Some(reply_address),
        )
        .await;
    }

    async fn serve_release_member_candidate(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgeReleasePayload,
    ) {
        let reply_address = payload.supervisor.address.clone();
        if !self
            .admit_host_command(
                candidate,
                &payload.mob_id,
                payload.epoch,
                payload.binding_generation,
                &reply_address,
            )
            .await
        {
            return;
        }
        if self.materializer.is_none() {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Unavailable,
                "host composed without a member substrate",
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }
        let member_key = MemberKey::new(
            AuthorityMobId::from(payload.mob_id.as_str()),
            AuthorityAgentIdentity::from(payload.agent_identity.as_str()),
        );

        let admission = match resolve_release_admission(
            &mut self.binding_authority,
            &member_key,
            payload.generation,
            payload.fence_token,
        ) {
            Ok(admission) => admission,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("release admission failed: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };
        match admission {
            ReleaseAdmission::Rejected { kind } => {
                // StaleMaterialized/StaleReleased ⇒ StaleFence; Ahead*/
                // UnknownMember ⇒ Unsupported — never silently absorbed.
                self.send_failure(
                    candidate,
                    bridge_rejection_cause(kind),
                    format!("release member rejected: {kind:?}"),
                    Some(reply_address.as_str()),
                )
                .await;
            }
            ReleaseAdmission::Replay { disposal, witness } => {
                // Re-verify deregistration idempotently, then re-ack the
                // recorded disposal.
                let released_pubkey = match self.persistence.load(&payload.mob_id).await {
                    Ok(record) => record.and_then(|record| {
                        record
                            .released
                            .get(&payload.agent_identity)
                            .map(|row| row.member_pubkey.clone())
                    }),
                    Err(error) => {
                        self.send_failure(
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!("release replay row load failed: {error}"),
                            Some(reply_address.as_str()),
                        )
                        .await;
                        return;
                    }
                };
                if let Some(pubkey) = released_pubkey {
                    if let Err(error) = self.deregister_member_identity(&pubkey, &witness) {
                        self.send_failure(
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!("release replay deregistration failed: {error}"),
                            Some(reply_address.as_str()),
                        )
                        .await;
                        return;
                    }
                }
                let reply = BridgeReply::MemberReleased(BridgeMemberReleasedResponse {
                    disposal: RecordedDisposal::from_machine(disposal).to_wire(),
                });
                self.send_reply(
                    candidate,
                    HostBridgeReply::completed(reply),
                    Some(reply_address.as_str()),
                )
                .await;
            }
            ReleaseAdmission::Admitted => {
                self.serve_admitted_release(candidate, &payload, &member_key, &reply_address)
                    .await;
            }
        }
    }

    /// §19.L3: OUTER quiesce → exact live-channel absence proof/close plus
    /// ownership-discriminated archive (through the extracted disposal arc) →
    /// record (row move under witness) → deregister → reply. A disposal
    /// failure records NOTHING and replies typed — the release stays retryable
    /// at the same tuple.
    ///
    /// The teardown itself is reply-free and shared
    /// ([`Self::release_recorded_member_residency`]); this arm only projects
    /// its typed outcome onto the bridge.
    async fn serve_admitted_release(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: &BridgeReleasePayload,
        member_key: &MemberKey,
        reply_address: &str,
    ) {
        match self
            .release_recorded_member_residency(member_key, payload.generation, payload.fence_token)
            .await
        {
            MemberTeardownOutcome::Released { disposal } => {
                let reply = BridgeReply::MemberReleased(BridgeMemberReleasedResponse { disposal });
                self.send_reply(
                    candidate,
                    HostBridgeReply::completed(reply),
                    Some(reply_address),
                )
                .await;
            }
            MemberTeardownOutcome::Retained { cause, detail }
            | MemberTeardownOutcome::ReleasedWithResidue { cause, detail } => {
                self.send_failure(candidate, cause, detail, Some(reply_address))
                    .await;
            }
        }
    }

    /// Perform one exact, reply-free member-residency teardown at an
    /// already-admitted `(generation, fence_token)` tuple.
    ///
    /// This is THE host-local release path. Both the bridge `ReleaseMember`
    /// arm and the host's own autonomous convergence sweep run it, so there is
    /// exactly one ordering of quiesce → disposal → capability release →
    /// durable receipt → deregistration, and exactly one set of retention
    /// rules when a step cannot be proven.
    async fn release_recorded_member_residency(
        &mut self,
        member_key: &MemberKey,
        generation: u64,
        fence_token: u64,
    ) -> MemberTeardownOutcome {
        let mob_id = member_key.mob_id.0.clone();
        let agent_identity = member_key.agent_identity.0.clone();
        let recorded_session = self
            .binding_authority
            .state()
            .materialized_sessions
            .get(member_key)
            .map(|session| session.0.clone());
        let Some(session_id_str) = recorded_session else {
            return MemberTeardownOutcome::Retained {
                cause: BridgeRejectionCause::Internal,
                detail: "release admitted without a recorded member session".to_string(),
            };
        };
        let session_id = match meerkat_core::types::SessionId::parse(&session_id_str) {
            Ok(session_id) => session_id,
            Err(error) => {
                return MemberTeardownOutcome::Retained {
                    cause: BridgeRejectionCause::Internal,
                    detail: format!("recorded session id is invalid: {error}"),
                };
            }
        };

        let realm_backend_persistent = self
            .materializer
            .as_ref()
            .is_some_and(|materializer| materializer.substrate().realm_backend_persistent);
        let runtime_adapter = match self.materializer.as_ref() {
            Some(materializer) => Arc::clone(&materializer.substrate().runtime_adapter),
            None => {
                return MemberTeardownOutcome::Retained {
                    cause: BridgeRejectionCause::Unavailable,
                    detail: "host composed without a member substrate".to_string(),
                };
            }
        };
        // Acquire the stable placed-residency transaction before teardown and
        // retain it through the durable release commit. All delayed effects
        // either finish against this exact incarnation first or observe the
        // final VacantPlaced state; none can validate G1 then mutate reuse G2.
        let residency_update = runtime_adapter
            .begin_member_residency_update(session_id.clone())
            .await;
        let Some(materializer) = self.materializer.as_mut() else {
            return MemberTeardownOutcome::Retained {
                cause: BridgeRejectionCause::Unavailable,
                detail: "host composed without a member substrate".to_string(),
            };
        };
        let (machine_disposal, wire_disposal) = if !realm_backend_persistent {
            // D5 typed degradation, declared at bind: runtime retire + claim
            // release + registry deregister only.
            match materializer.release_runtime_only(&session_id).await {
                Ok(()) => (
                    MachineMemberSessionDisposal::RuntimeReleasedOnlyNoDurableSessions,
                    WireMemberSessionDisposal::RuntimeReleasedOnly {
                        cause: RuntimeReleaseCause::NoDurableSessions,
                    },
                ),
                Err(error) => {
                    let retained_stage = crate::runtime::provisioner::MemberSessionDisposalArc::
                        runtime_retirement_progress_stage(&error);
                    let (cause, detail) = retained_stage.map_or_else(
                        || (
                            BridgeRejectionCause::Internal,
                            format!("member runtime release failed: {error}"),
                        ),
                        |stage| {
                            let detail = format!(
                                "member runtime release remains in progress at {stage}; exact teardown authority is retained"
                            );
                            (
                                BridgeRejectionCause::RuntimeRetirementInProgress { stage },
                                detail,
                            )
                        },
                    );
                    return MemberTeardownOutcome::Retained { cause, detail };
                }
            }
        } else {
            match materializer.dispose(&session_id).await {
                // DEC-P3H-6: wire keeps the AlreadyArchived success-class
                // distinction; the machine/durable fact folds it to
                // Archived (both mean the durable terminal holds).
                Ok(crate::runtime::provisioner::MemberSessionDisposalVerdict::Archived) => (
                    MachineMemberSessionDisposal::Archived,
                    WireMemberSessionDisposal::Archived,
                ),
                Ok(
                    crate::runtime::provisioner::MemberSessionDisposalVerdict::ArchivedWithRecoveredOpsRetired,
                ) => (
                    MachineMemberSessionDisposal::Archived,
                    WireMemberSessionDisposal::Archived,
                ),
                Ok(crate::runtime::provisioner::MemberSessionDisposalVerdict::AlreadyArchived) => (
                    MachineMemberSessionDisposal::Archived,
                    WireMemberSessionDisposal::AlreadyArchived,
                ),
                Ok(
                    crate::runtime::provisioner::MemberSessionDisposalVerdict::RuntimeReleasedOnlyHostOwned,
                ) => (
                    MachineMemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
                    WireMemberSessionDisposal::RuntimeReleasedOnly {
                        cause: RuntimeReleaseCause::HostOwnedSession,
                    },
                ),
                Err(error) => {
                    let retained_stage = crate::runtime::provisioner::MemberSessionDisposalArc::
                        runtime_retirement_progress_stage(&error);
                    let (cause, detail) = retained_stage.map_or_else(
                        || (
                            BridgeRejectionCause::Internal,
                            format!("member session disposal failed: {error}"),
                        ),
                        |stage| {
                            let detail = format!(
                                "member session disposal remains in progress at {stage}; exact teardown authority is retained"
                            );
                            (
                                BridgeRejectionCause::RuntimeRetirementInProgress { stage },
                                detail,
                            )
                        },
                    );
                    return MemberTeardownOutcome::Retained { cause, detail };
                }
            }
        };

        // Exact disposal has succeeded and the residency is gone. The durable
        // half — capability release then release receipt — is shared with
        // every other caller that has proven a disposal.
        let (witness, member_pubkey) = match commit_member_release_after_disposal(
            &mut self.binding_authority,
            self.persistence.as_ref(),
            self.forked_participant_service.as_ref(),
            member_key,
            generation,
            fence_token,
            machine_disposal,
        )
        .await
        {
            Ok(recorded) => recorded,
            Err(retention) => {
                if retention.durable_uncertainty {
                    self.durable_uncertainty_fail_stop = Some(retention.detail.clone());
                }
                return MemberTeardownOutcome::Retained {
                    cause: retention.cause,
                    detail: retention.detail,
                };
            }
        };
        let residency_publication = match residency_update.vacate() {
            Ok(publication) => publication,
            Err(error) => {
                return MemberTeardownOutcome::ReleasedWithResidue {
                    cause: BridgeRejectionCause::Internal,
                    detail: format!("release residency vacancy publication failed: {error}"),
                };
            }
        };
        self.registered_member_incarnations.remove(&session_id);
        self.sync_member_observation().await;
        drop(residency_publication);
        // Deregistration failure is logged + typed-Internal; the row is
        // already released — retry hits ReleaseReplay, which re-attempts
        // removal idempotently.
        if let Err(error) = self.deregister_member_identity(&member_pubkey, &witness) {
            return MemberTeardownOutcome::ReleasedWithResidue {
                cause: BridgeRejectionCause::Internal,
                detail: format!("member identity deregistration failed: {error}"),
            };
        }
        self.unrevived.remove(&(mob_id, agent_identity));
        MemberTeardownOutcome::Released {
            disposal: wire_disposal,
        }
    }

    /// §10.4 (phase 4): serve `InstallPeerTrust` — rung-0 admission →
    /// member-materialized + live-runtime checks (typed `Unavailable`,
    /// ADJ-P4-7) → apply through the member session's machine-gated
    /// direct-peer-endpoint seam (`stage_add_direct_peer_endpoint`; the
    /// generated reconciler inside is the ONLY `apply_trust_mutation`
    /// caller — never a direct TrustStore write, gotcha #7) → `Ack`.
    ///
    /// Idempotent by machine construction (DEC-P4H-5 — no dedup journal;
    /// trust rows are volatile by doctrine): re-installing the identical
    /// descriptor commits the generated `endpoint_already_direct` repair
    /// twin, re-emits the reconcile effect, and acks. A same-peer-id row
    /// with DIFFERENT endpoint material is removed (exact) before the add —
    /// one projected endpoint per peer id (ADJ-P4-8). When the descriptor's
    /// peer is itself materialized on THIS host, the arm substitutes the
    /// local inproc address (ADJ-P4-6): the owning host is the only party
    /// that knows the inproc names, and the controlling side always sends
    /// the machine-recorded descriptor unmodified.
    async fn serve_install_peer_trust_candidate(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgePeerTrustPayload,
    ) {
        let reply_address = payload.supervisor.address.clone();
        if !self
            .admit_host_command(
                candidate,
                &payload.mob_id,
                payload.epoch,
                payload.binding_generation,
                &reply_address,
            )
            .await
        {
            return;
        }
        let Some((session_id, live, runtime_adapter)) = self
            .admitted_trust_target(candidate, &payload, &reply_address)
            .await
        else {
            return;
        };
        // Peer-spec integrity, pre-machine: malformed material rejects
        // BEFORE any machine mutation (the member-drain WireMember
        // precedent). No re-validation of `payload.supervisor` — sender
        // identity was adjudicated at rung 0.
        let peer_desc = match TrustedPeerDescriptor::try_from(&payload.peer) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::InvalidPeerSpec,
                    format!("install peer trust rejected: invalid peer spec: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };
        let endpoint = self.localize_same_host_peer(mm_dsl::PeerEndpoint::from(&peer_desc));
        let comms = Arc::clone(&live.runtime) as Arc<dyn CoreCommsRuntime>;

        // ADJ-P4-8: single projected endpoint per peer id — remove a stale
        // same-peer-id row (exact recorded form) before adding the new one.
        let existing = match runtime_adapter.direct_peer_endpoints(&session_id).await {
            Ok(endpoints) => endpoints,
            Err(error) => {
                self.reject_peer_trust_stage(
                    candidate,
                    "install peer trust",
                    &error,
                    &reply_address,
                )
                .await;
                return;
            }
        };
        if let Some(stale) = existing
            .iter()
            .find(|row| row.peer_id == endpoint.peer_id && **row != endpoint)
            .cloned()
        {
            if let Err(error) = runtime_adapter
                .stage_remove_direct_peer_endpoint(&session_id, stale, Arc::clone(&comms))
                .await
            {
                self.reject_peer_trust_stage(
                    candidate,
                    "install peer trust (stale-endpoint removal)",
                    &error,
                    &reply_address,
                )
                .await;
                return;
            }
        }
        if let Err(error) = runtime_adapter
            .stage_add_direct_peer_endpoint(&session_id, endpoint, comms)
            .await
        {
            self.reject_peer_trust_stage(candidate, "install peer trust", &error, &reply_address)
                .await;
            return;
        }
        self.send_reply(
            candidate,
            HostBridgeReply::completed(BridgeReply::Ack(BridgeAck { ok: true })),
            Some(reply_address.as_str()),
        )
        .await;
    }

    /// §10.4 (phase 4): serve `RemovePeerTrust` — the same admission ladder
    /// as install; the removal targets the EXACT recorded endpoint for the
    /// peer id (the host actor's own `remove_peer_trust` find-by-peer-id
    /// discipline). An absent row stages the generated absent-endpoint
    /// repair so stale trust-store projection rows cannot stay behaviorally
    /// active, then acks — idempotent remove = ack (DEC-P4H-5).
    async fn serve_remove_peer_trust_candidate(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgePeerTrustPayload,
    ) {
        let reply_address = payload.supervisor.address.clone();
        if !self
            .admit_host_command(
                candidate,
                &payload.mob_id,
                payload.epoch,
                payload.binding_generation,
                &reply_address,
            )
            .await
        {
            return;
        }
        let Some((session_id, live, runtime_adapter)) = self
            .admitted_trust_target(candidate, &payload, &reply_address)
            .await
        else {
            return;
        };
        // Peer-spec integrity, pre-machine (same rung as install); the
        // removal itself keys on the canonical peer id only.
        if let Err(error) = TrustedPeerDescriptor::try_from(&payload.peer) {
            self.send_failure(
                candidate,
                BridgeRejectionCause::InvalidPeerSpec,
                format!("remove peer trust rejected: invalid peer spec: {error}"),
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }
        let comms = Arc::clone(&live.runtime) as Arc<dyn CoreCommsRuntime>;
        let existing = match runtime_adapter.direct_peer_endpoints(&session_id).await {
            Ok(endpoints) => endpoints,
            Err(error) => {
                self.reject_peer_trust_stage(
                    candidate,
                    "remove peer trust",
                    &error,
                    &reply_address,
                )
                .await;
                return;
            }
        };
        let staged = match existing
            .iter()
            .find(|row| row.peer_id.0 == payload.peer.peer_id)
            .cloned()
        {
            Some(recorded) => {
                runtime_adapter
                    .stage_remove_direct_peer_endpoint(&session_id, recorded, comms)
                    .await
            }
            None => {
                runtime_adapter
                    .stage_repair_remove_direct_peer_id(
                        &session_id,
                        payload.peer.peer_id.clone(),
                        comms,
                    )
                    .await
            }
        };
        if let Err(error) = staged {
            self.reject_peer_trust_stage(candidate, "remove peer trust", &error, &reply_address)
                .await;
            return;
        }
        self.send_reply(
            candidate,
            HostBridgeReply::completed(BridgeReply::Ack(BridgeAck { ok: true })),
            Some(reply_address.as_str()),
        )
        .await;
    }

    /// Shared trust-arm target resolution AFTER rung-0 admission: substrate
    /// presence → materialized-member row → live runtime. Every reject here
    /// is typed `Unavailable` (ADJ-P4-7 — retry-friendly, matching the
    /// NoPlacement→Unavailable and host-unavailability precedents): the
    /// controlling side re-drives outstanding obligations after
    /// revival/rebind. The trust arm never recomposes a dead runtime —
    /// ensure-on-replay (ADJ-9) is a materialize-lane behavior.
    async fn admitted_trust_target(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: &BridgePeerTrustPayload,
        reply_address: &str,
    ) -> Option<(
        meerkat_core::types::SessionId,
        LiveMemberRuntime,
        Arc<meerkat_runtime::MeerkatMachine>,
    )> {
        let Some(materializer) = self.materializer.as_ref() else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Unavailable,
                "host composed without a member substrate",
                Some(reply_address),
            )
            .await;
            return None;
        };
        let member_key = MemberKey::new(
            AuthorityMobId::from(payload.mob_id.as_str()),
            AuthorityAgentIdentity::from(payload.agent_identity.as_str()),
        );
        let recorded_session = self
            .binding_authority
            .state()
            .materialized_sessions
            .get(&member_key)
            .map(|session| session.0.clone());
        let Some(session_id_str) = recorded_session else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Unavailable,
                "member not materialized on this host",
                Some(reply_address),
            )
            .await;
            return None;
        };
        let session_id = match meerkat_core::types::SessionId::parse(&session_id_str) {
            Ok(session_id) => session_id,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("recorded session id is invalid: {error}"),
                    Some(reply_address),
                )
                .await;
                return None;
            }
        };
        let Some(live) = materializer.live_runtime(&session_id).cloned() else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Unavailable,
                "member runtime not live on this host",
                Some(reply_address),
            )
            .await;
            return None;
        };
        let runtime_adapter = Arc::clone(&materializer.substrate().runtime_adapter);
        Some((session_id, live, runtime_adapter))
    }

    /// ADJ-P4-6: when the descriptor's peer is itself materialized on THIS
    /// host with a live runtime, substitute the local inproc address so a
    /// same-host pair rides inproc instead of looping through its own TCP
    /// acceptor. Identity material (peer id, signing key) is NEVER
    /// substituted — only the transport address; the canonical peer id is
    /// globally unique (Ed25519-derived), so the match needs no mob filter
    /// beyond the rung-0 admission already performed.
    fn localize_same_host_peer(&self, endpoint: mm_dsl::PeerEndpoint) -> mm_dsl::PeerEndpoint {
        let Some(materializer) = self.materializer.as_ref() else {
            return endpoint;
        };
        for session in self
            .binding_authority
            .state()
            .materialized_sessions
            .values()
        {
            let Ok(session_id) = meerkat_core::types::SessionId::parse(&session.0) else {
                continue;
            };
            let Some(live) = materializer.live_runtime(&session_id) else {
                continue;
            };
            if live.runtime.public_key().to_peer_id().to_string() != endpoint.peer_id.0 {
                continue;
            }
            let mut localized = endpoint;
            localized.address =
                mm_dsl::PeerAddress(format!("inproc://{}", live.runtime.participant_name()));
            return localized;
        }
        endpoint
    }

    /// DEC-P4H-3 error mapping for the trust arms — never a panic, never a
    /// silent `Ok`: `SessionNotRegistered` ⇒ `Unavailable` (the runtime
    /// raced away; retryable); a parse-at-boundary endpoint rejection ⇒
    /// `InvalidPeerSpec`; any DSL/reconcile fault ⇒ `Internal`. A reconcile
    /// failure after machine commit stays fail-closed (no usable trust row);
    /// the controlling retry re-stages and the generated repair twin
    /// re-emits the reconcile effect, so the edge converges.
    async fn reject_peer_trust_stage(
        &self,
        candidate: &PeerInputCandidate,
        context: &str,
        error: &PeerEndpointStageError,
        reply_address: &str,
    ) {
        let cause = match error {
            PeerEndpointStageError::SessionNotRegistered => BridgeRejectionCause::Unavailable,
            PeerEndpointStageError::InvalidEndpoint(_) => BridgeRejectionCause::InvalidPeerSpec,
            PeerEndpointStageError::Dsl(_)
            | PeerEndpointStageError::MissingReconcileEffect
            | PeerEndpointStageError::LocalEndpoint(_)
            | PeerEndpointStageError::Reconcile(_) => BridgeRejectionCause::Internal,
        };
        self.send_failure(
            candidate,
            cause,
            format!("{context} failed: {error}"),
            Some(reply_address),
        )
        .await;
    }

    /// DEC-P3H-10: the §7.3/§9/§21.2 orphan-reconciliation + reachability
    /// verb. `healthy` is a shell observation from the live session census
    /// (ADJ-19 — it feeds a projection, never a machine decision).
    async fn serve_host_status_candidate(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgeHostStatusPayload,
    ) {
        let reply_address = payload.supervisor.address.clone();
        if !self
            .admit_host_command(
                candidate,
                &payload.mob_id,
                payload.epoch,
                payload.binding_generation,
                &reply_address,
            )
            .await
        {
            return;
        }
        // §14.5 refresh carrier: recomputed per status query.
        let capabilities = match self.capabilities.compose().await {
            Ok(capabilities) => capabilities,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("host status failed: capability probe: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };

        // Host-only durable custody (notably ACK tombstones) can outlive the
        // controller's active machine sets. Classify an incompatible
        // downgrade as typed Unsupported; the controller treats that status
        // rejection as a capability fail-stop rather than reachability loss.
        let retained = match self.persistence.load(&payload.mob_id).await {
            Ok(record) => record,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("host status failed: retained-contract read: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };
        let missing = missing_host_capabilities(
            retained_host_capability_requirements(retained.as_ref()),
            &capabilities,
        );
        if !missing.is_empty() {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Unsupported,
                format!(
                    "host status capability contract is incompatible with retained residency: {}",
                    missing.join(", ")
                ),
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }

        let mob = AuthorityMobId::from(payload.mob_id.as_str());
        let state = self.binding_authority.state();
        let mut rows: Vec<(String, u64, u64, String, String)> = Vec::new();
        for (key, generation) in &state.materialized_generations {
            if key.mob_id != mob {
                continue;
            }
            let (Some(fence), Some(session), Some(digest)) = (
                state.materialized_fences.get(key),
                state.materialized_sessions.get(key),
                state.materialized_spec_digests.get(key),
            ) else {
                // Key alignment is a generated invariant; a miss here is a
                // typed internal fault, never a silent partial row.
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!(
                        "materialized rows for '{}' lost key alignment",
                        key.agent_identity.0
                    ),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            };
            rows.push((
                key.agent_identity.0.clone(),
                generation.0,
                fence.0,
                session.0.clone(),
                digest.clone(),
            ));
        }

        let mut members = Vec::with_capacity(rows.len());
        let mob_has_capability_debt = self
            .forked_participant_debts
            .iter()
            .any(|(mob_id, _)| mob_id == &payload.mob_id);
        for (identity, generation, fence_token, session_id, spec_digest) in rows {
            let healthy = if mob_has_capability_debt {
                // An unreconciled capability attachment is a host-wide
                // obligation, not a per-row one: the debt names a fork session
                // whose owning row may be exactly the one being reported.
                false
            } else if self
                .unrevived
                .contains(&(payload.mob_id.clone(), identity.clone()))
            {
                false
            } else if let Some(materializer) = self.materializer.as_ref() {
                match meerkat_core::types::SessionId::parse(&session_id) {
                    Ok(parsed) => materializer.session_live(&parsed).await,
                    Err(_) => false,
                }
            } else {
                // No substrate ⇒ no live session can exist on this daemon.
                false
            };
            members.push(BridgeHostMemberRecord {
                agent_identity: identity,
                generation,
                fence_token,
                session_id,
                spec_digest,
                healthy,
            });
        }

        let reply = BridgeReply::HostStatus(BridgeHostStatusResponse {
            runtime_incarnation: self.runtime_incarnation,
            members,
            capabilities,
        });
        self.send_reply(
            candidate,
            HostBridgeReply::completed(reply),
            Some(reply_address.as_str()),
        )
        .await;
    }

    async fn serve_issue_host_binding_descriptor_candidate(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgeIssueHostBindingDescriptorPayload,
    ) {
        let reply_address = payload.supervisor.address.clone();
        if !payload.protocol_version.supports_forked_participants() {
            self.send_failure(
                candidate,
                BridgeRejectionCause::ForkedParticipantProtocolUnsupported,
                "host binding descriptor handoff requires forked-participant protocol support",
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }
        if !self
            .admit_host_command(
                candidate,
                &payload.mob_id,
                payload.epoch,
                payload.binding_generation,
                &reply_address,
            )
            .await
        {
            return;
        }

        let expected_host_peer_id = self
            .host_comms
            .peer_id()
            .map(|peer_id| peer_id.to_string())
            .unwrap_or_default();
        let descriptor = self.descriptor.descriptor("");
        let delegated_bootstrap_proof =
            crate::runtime::bridge_protocol::derive_delegated_host_bind_proof(
                &self.delegated_bind_key,
                &payload.target_supervisor,
                &payload.target_mob_id,
                &expected_host_peer_id,
                &self.advertised_address,
            );
        self.send_reply(
            candidate,
            HostBridgeReply::completed(BridgeReply::HostBindingDescriptorIssued(
                BridgeHostBindingDescriptorIssuedResponse {
                    descriptor,
                    target_supervisor: payload.target_supervisor,
                    delegated_bootstrap_proof,
                },
            )),
            Some(reply_address.as_str()),
        )
        .await;
    }

    /// Register a member identity on the acceptor demux, gated by a
    /// materialize transition/recovery witness (DEC-P3H-7). Idempotent for
    /// the SAME pubkey under the same owner: a stale entry (dead inbox from
    /// a previous incarnation) is replaced, because the member keypair is
    /// durable while the inbox is per-runtime.
    fn register_member_identity(
        &self,
        member: &crate::runtime::host_materialize::LiveMemberRuntime,
        _witness: &MaterializedIdentityWitness,
    ) -> Result<(), MobHostActorError> {
        let pubkey = member.runtime.public_key();
        let inbox_sender = member
            .runtime
            .tool_material()
            .router()
            .inbox_sender()
            .clone();
        match self.registry.register_identity(
            &self.registry_owner,
            pubkey,
            Arc::clone(&member.ack_keypair),
            inbox_sender.clone(),
        ) {
            Ok(()) => Ok(()),
            Err(meerkat_comms::HostAcceptorError::IdentityAlreadyRegistered { .. }) => {
                // Stale entry from a previous incarnation (dead inbox); the
                // member keypair is durable while the inbox is per-runtime —
                // replace under the same owner.
                self.registry
                    .remove_identity(&self.registry_owner, &pubkey)?;
                self.registry
                    .register_identity(
                        &self.registry_owner,
                        pubkey,
                        Arc::clone(&member.ack_keypair),
                        inbox_sender,
                    )
                    .map_err(MobHostActorError::from)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Remove a member identity from the acceptor demux, gated by a release
    /// transition witness. Removal of an absent entry is success (replay
    /// idempotency).
    fn deregister_member_identity(
        &self,
        member_pubkey: &str,
        _witness: &ReleasedIdentityWitness,
    ) -> Result<bool, MobHostActorError> {
        let pubkey = meerkat_comms::PubKey::from_pubkey_string(member_pubkey).map_err(|err| {
            MobHostActorError::Internal {
                detail: format!("recorded member pubkey is invalid: {err}"),
            }
        })?;
        Ok(self
            .registry
            .remove_identity(&self.registry_owner, &pubkey)?)
    }

    /// Boot revival walk (A20/§14.6): recompose every recovered materialized
    /// row from its stored spec against the recovered binding facts, then
    /// re-register the member identity under a recovery-derived witness.
    /// Integrity/key corruption and store faults abort startup typed before
    /// any runtime effect. Environmental recompose failures are logged and
    /// land in the unrevived health map.
    async fn revive_recovered_members(
        &mut self,
        host_id: &str,
        prepared_members: Vec<PreparedRecoveredMember>,
    ) {
        let tracked_turns_supported = self
            .materializer
            .as_ref()
            .is_some_and(|materializer| materializer.substrate().durable_event_log.is_some());
        let observation_record_tx = self.observation_record_tx.clone();
        for prepared in prepared_members {
            let PreparedRecoveredMember {
                mob_id,
                identity,
                row,
                binding_generation,
                supervisor_epoch,
                supervisor,
                session_id,
                decompile,
                identity_witness,
            } = prepared;
            let decompiled = match decompile {
                PreparedRecoveredDecompile::Ready(decompiled) => decompiled,
                PreparedRecoveredDecompile::EnvironmentalFailure(error) => {
                    tracing::error!(
                        mob_id = %mob_id,
                        identity = %identity,
                        error = %error,
                        "mob host revival: member environment is unavailable; daemon starts and \
                         HostStatus reports the row unhealthy"
                    );
                    self.unrevived.insert((mob_id, identity));
                    continue;
                }
            };
            let Some(materializer) = self.materializer.as_mut() else {
                tracing::error!(
                    mob_id = %mob_id,
                    identity = %identity,
                    "mob host revival: prepared member has no configured substrate"
                );
                self.unrevived.insert((mob_id, identity));
                continue;
            };
            match materializer
                .revive_prepared_from_row(
                    &row.spec,
                    decompiled,
                    &row.session_id,
                    &row.member_pubkey,
                    row.generation,
                    row.fence_token,
                    host_id,
                    binding_generation,
                    &supervisor,
                    supervisor_epoch,
                )
                .await
            {
                Ok(mut outcome) => {
                    let incarnation =
                        meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
                            mob_id: mob_id.clone(),
                            agent_identity: identity.clone(),
                            host_id: host_id.to_string(),
                            binding_generation,
                            member_session_id: row.session_id.clone(),
                            generation: row.generation,
                            fence_token: row.fence_token,
                        };
                    let journal: Option<
                        Arc<dyn meerkat_runtime::member_observation::TrackedTurnJournal>,
                    > = tracked_turns_supported.then(|| {
                        Arc::new(HostTrackedTurnJournal::new(
                            incarnation.clone(),
                            observation_record_tx.clone(),
                        ))
                            as Arc<dyn meerkat_runtime::member_observation::TrackedTurnJournal>
                    });
                    let residency_publication = match commit_revived_member_publication(
                        &mut outcome,
                        incarnation.clone(),
                        journal,
                    )
                    .await
                    {
                        Ok(publication) => publication,
                        Err(error) => {
                            tracing::error!(
                                mob_id = %mob_id,
                                identity = %identity,
                                error = %error,
                                "mob host revival: exact residency publication failed"
                            );
                            self.unrevived.insert((mob_id.clone(), identity.clone()));
                            continue;
                        }
                    };
                    self.registered_member_incarnations
                        .insert(session_id, incarnation);
                    if residency_publication.is_some() {
                        self.sync_member_observation().await;
                    }
                    drop(residency_publication);
                    let member = outcome.member;
                    if let Err(error) = self.register_member_identity(&member, &identity_witness) {
                        tracing::error!(
                            mob_id = %mob_id,
                            identity = %identity,
                            error = %error,
                            "mob host revival: member identity registration failed"
                        );
                        self.unrevived.insert((mob_id.clone(), identity.clone()));
                    }
                }
                Err(error) => {
                    tracing::error!(
                        mob_id = %mob_id,
                        identity = %identity,
                        error = %error,
                        "mob host revival: member recompose failed; daemon starts and \
                         HostStatus reports the row unhealthy"
                    );
                    self.unrevived.insert((mob_id.clone(), identity.clone()));
                }
            }
        }
    }

    async fn serve_bind_host_candidate(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgeHostBindPayload,
    ) {
        let supervisor = match BridgePeerIdentity::try_from(&payload.supervisor) {
            Ok(supervisor) => supervisor,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::InvalidSupervisorSpec,
                    format!("bind host failed: invalid supervisor peer spec: {error}"),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
        };
        let Some(host_peer_id) = self.host_comms.peer_id() else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                "bind host failed: host runtime peer_id unavailable",
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        };
        // Shell self-integrity: the machine cannot see the self-addressing
        // fact, so a caller that expected a DIFFERENT host identity is a
        // typed reject before machine dispatch (the member drain's
        // expected_peer_id discipline — observation, not pre-decision).
        if payload.expected_host_peer_id != host_peer_id.as_str() {
            self.send_failure(
                candidate,
                BridgeRejectionCause::InvalidPeerSpec,
                format!(
                    "bind host peer_id mismatch: expected '{}', actual '{}'",
                    payload.expected_host_peer_id,
                    host_peer_id.as_str()
                ),
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        }

        // Compose capabilities BEFORE machine dispatch so a probe fault can
        // never strand a committed binding without a success reply.
        let capabilities = match self.capabilities.compose().await {
            Ok(capabilities) => capabilities,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("bind host failed: capability probe: {error}"),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
        };
        let retained = match self.persistence.load(&payload.mob_id).await {
            Ok(record) => record,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("bind host failed: capability-contract read: {error}"),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
        };
        let required = merge_host_capability_requirements(
            payload.required_capabilities,
            retained_host_capability_requirements(retained.as_ref()),
        );
        // If the exact authority tuple is already durable, this is a
        // reply-loss replay. Its acceptance was adjudicated against the
        // capability snapshot persisted in the same transaction; a later
        // runtime downgrade must not make the controller unable to absorb
        // the already-advanced host authority.
        let replay_capabilities = exact_binding_capability_snapshot(
            retained.as_ref(),
            &supervisor,
            payload.epoch,
            payload.binding_generation,
        );
        let missing = if replay_capabilities.is_none() {
            missing_host_capabilities(required, &capabilities)
        } else {
            Vec::new()
        };
        if !missing.is_empty() {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Unsupported,
                format!(
                    "bind host rejected before authority mutation: required capabilities unavailable: {}",
                    missing.join(", ")
                ),
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        }
        let ceremony_capabilities = replay_capabilities.unwrap_or_else(|| capabilities.clone());

        let observations = HostBindObservations {
            mob_id: payload.mob_id.clone(),
            epoch: payload.epoch,
            binding_generation: payload.binding_generation,
            sender_matches_supervisor: sender_matches_bridge_peer(&candidate.ingress, &supervisor),
            address_matches: canonicalize_bridge_address(&payload.expected_address)
                == self.advertised_address,
            token_valid: self.bootstrap_token.matches_bind_proof(&payload)
                || payload
                    .delegated_bootstrap_proof
                    .as_ref()
                    .is_some_and(|proof| {
                        meerkat_comms::constant_time_str_eq(
                            proof.as_str(),
                            crate::runtime::bridge_protocol::derive_delegated_host_bind_proof(
                                &self.delegated_bind_key,
                                &payload.supervisor,
                                &payload.mob_id,
                                &payload.expected_host_peer_id,
                                &payload.expected_address,
                            )
                            .as_str(),
                        )
                    }),
            accepted_capabilities: capabilities,
            supervisor,
        };

        let outcome = match serve_host_bind(
            &mut self.binding_authority,
            self.persistence.as_ref(),
            &mut self.bootstrap_token,
            observations,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                self.durable_uncertainty_fail_stop = Some(error.to_string());
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("bind host failed: {error}"),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
        };

        match outcome {
            HostBindServeOutcome::Rejected { cause, reason } => {
                self.send_failure(
                    candidate,
                    cause,
                    reason,
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
            }
            HostBindServeOutcome::Accepted { fresh, supervisor } => {
                if fresh {
                    // DEC-P2-4: descriptor rewritten with the re-minted
                    // token. The binding is already durable; a rewrite
                    // failure only affects FUTURE binds, so it is an
                    // operator-visible availability obligation, not this
                    // ceremony's failure. The actor retries the exact current
                    // token; retry must never consume or re-mint it again.
                    match self.descriptor.publish(self.bootstrap_token.current()) {
                        Ok(()) => self.pending_descriptor_refresh = None,
                        Err(error) => {
                            self.pending_descriptor_refresh =
                                Some(PendingDescriptorRefresh::after_publish_failure());
                            tracing::error!(
                                error = %error,
                                retry_delay_ms = HOST_DESCRIPTOR_REFRESH_INITIAL_RETRY_DELAY
                                    .as_millis() as u64,
                                "mob host: descriptor rewrite after token re-mint failed; \
                                 replacement publication remains pending on the actor"
                            );
                        }
                    }
                }
                // Supervisor trust install is post-persist: failure leaves
                // the binding durable and the supervisor retries into the
                // idempotent replay ack, which re-attempts this install.
                if let Err(error) = self.install_supervisor_trust(supervisor).await {
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("bind host trust install failed: {error}"),
                        Some(payload.supervisor.address.as_str()),
                    )
                    .await;
                    return;
                }
                let reply = BridgeReply::BindHost(BridgeHostBindResponse {
                    // Canonical identity re-read from the runtime — never an
                    // echo of caller-supplied expected_* fields.
                    host_peer_id: host_peer_id.as_str(),
                    address: self.advertised_address.clone(),
                    binding_generation: payload.binding_generation,
                    capabilities: ceremony_capabilities,
                    live_endpoint: self.descriptor.live_endpoint().cloned(),
                });
                self.send_reply(
                    candidate,
                    HostBridgeReply::completed(reply),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
            }
        }
    }

    async fn serve_rebind_host_candidate(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgeHostRebindPayload,
    ) {
        let supervisor = match BridgePeerIdentity::try_from(&payload.supervisor) {
            Ok(supervisor) => supervisor,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::InvalidSupervisorSpec,
                    format!("rebind host failed: invalid supervisor peer spec: {error}"),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
        };
        let Some(host_peer_id) = self.host_comms.peer_id() else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                "rebind host failed: host runtime peer_id unavailable",
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        };
        // §14.5 restart-truthfulness: rebind re-declares capabilities in
        // full; recompute per ceremony.
        let capabilities = match self.capabilities.compose().await {
            Ok(capabilities) => capabilities,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("rebind host failed: capability probe: {error}"),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
        };
        let retained = match self.persistence.load(&payload.mob_id).await {
            Ok(record) => record,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("rebind host failed: capability-contract read: {error}"),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
        };
        let required = merge_host_capability_requirements(
            payload.required_capabilities,
            retained_host_capability_requirements(retained.as_ref()),
        );
        let replay_capabilities = exact_binding_capability_snapshot(
            retained.as_ref(),
            &supervisor,
            payload.epoch,
            payload.binding_generation,
        );
        let missing = if replay_capabilities.is_none() {
            missing_host_capabilities(required, &capabilities)
        } else {
            Vec::new()
        };
        if !missing.is_empty() {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Unsupported,
                format!(
                    "rebind host rejected before authority mutation: required capabilities unavailable: {}",
                    missing.join(", ")
                ),
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        }
        let ceremony_capabilities = replay_capabilities.unwrap_or_else(|| capabilities.clone());

        let observations = HostRebindObservations {
            mob_id: payload.mob_id.clone(),
            epoch: payload.epoch,
            binding_generation: payload.binding_generation,
            sender_matches_supervisor: sender_matches_recorded_host_supervisor(
                self.binding_authority.state(),
                &payload.mob_id,
                &candidate.ingress,
            ),
            accepted_capabilities: capabilities,
            supervisor,
        };

        let outcome = match serve_host_rebind(
            &mut self.binding_authority,
            self.persistence.as_ref(),
            observations,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                self.durable_uncertainty_fail_stop = Some(error.to_string());
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("rebind host failed: {error}"),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return;
            }
        };

        match outcome {
            HostRebindServeOutcome::Rejected { cause, reason } => {
                self.send_failure(
                    candidate,
                    cause,
                    reason,
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
            }
            HostRebindServeOutcome::Accepted {
                supervisor,
                previous_supervisor_peer_id,
            } => {
                if let Err(error) = self.install_supervisor_trust(supervisor.clone()).await {
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("rebind host trust install failed: {error}"),
                        Some(payload.supervisor.address.as_str()),
                    )
                    .await;
                    return;
                }
                if let Err(error) = self
                    .refresh_live_member_supervisors(
                        &payload.mob_id,
                        &supervisor,
                        payload.epoch,
                        payload.binding_generation,
                    )
                    .await
                {
                    // Binding + refreshed per-row endpoints are already
                    // durable. Same-epoch retry re-enters the accepted replay
                    // and repairs any live member trust edge before ACK.
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("rebind live member trust refresh failed: {error}"),
                        Some(payload.supervisor.address.as_str()),
                    )
                    .await;
                    return;
                }
                let reply = BridgeReply::HostRebound(BridgeHostReboundResponse {
                    host_peer_id: host_peer_id.as_str(),
                    binding_generation: payload.binding_generation,
                    capabilities: ceremony_capabilities,
                    live_endpoint: self.descriptor.live_endpoint().cloned(),
                });
                self.send_reply(candidate, HostBridgeReply::completed(reply), None)
                    .await;
                // The advancing request is signed by the OLD authority, so
                // its trust edge must remain until the response is sent.
                // Retire it only afterward; a lingering stale edge is an
                // operator-visible hygiene fault, not a ceremony failure.
                if let Some(previous_peer_id) = previous_supervisor_peer_id
                    && let Err(error) = self
                        .remove_supervisor_trust_if_unused(&previous_peer_id)
                        .await
                {
                    tracing::warn!(
                        error = %error,
                        peer_id = %previous_peer_id,
                        "mob host: failed to retire rotated-out supervisor trust edge"
                    );
                }
            }
        }
    }

    async fn serve_revoke_host_candidate(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgeHostRevokePayload,
    ) {
        let reply_address = payload.supervisor.address.clone();
        if let Err(error) = BridgePeerIdentity::try_from(&payload.supervisor) {
            self.send_failure(
                candidate,
                BridgeRejectionCause::InvalidSupervisorSpec,
                format!("revoke host failed: invalid supervisor peer spec: {error}"),
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }
        let sender_peer_id = Self::ingress_sender_peer_id(candidate);
        let sender_signing_key = candidate.ingress.signing_pubkey.unwrap_or([0; 32]);
        let mob = AuthorityMobId::from(payload.mob_id.as_str());

        let mut prepared = self.binding_authority.prepare_authority();
        let transition = match MobHostBindingAuthorityMutator::apply(
            &mut prepared,
            MobHostBindingAuthorityInput::RevokeHostBinding {
                mob_id: mob.clone(),
                sender_peer_id: AuthorityPeerId::from(sender_peer_id.as_str()),
                sender_signing_key: AuthorityPeerSigningKey::from(sender_signing_key),
                epoch: payload.epoch,
                binding_generation: payload.binding_generation,
            },
        ) {
            Ok(transition) => transition,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("revoke host admission failed: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };
        let effect = match single_host_effect(&transition) {
            Ok(effect) => effect,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("revoke host admission failed: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };

        match effect {
            MobHostBindingAuthorityEffect::HostBindRejected { cause, .. } => {
                self.send_failure(
                    candidate,
                    bridge_rejection_cause(*cause),
                    format!("revoke host rejected: {cause:?}"),
                    Some(reply_address.as_str()),
                )
                .await;
            }
            MobHostBindingAuthorityEffect::HostBindingRevokeReplayed { .. } => {
                let receipt = match self.persistence.load_revocation(&payload.mob_id).await {
                    Ok(Some(receipt))
                        if receipt.supervisor_peer_id == sender_peer_id
                            && receipt.supervisor_signing_key == sender_signing_key
                            && receipt.epoch == payload.epoch
                            && receipt.binding_generation == payload.binding_generation =>
                    {
                        receipt
                    }
                    Ok(_) => {
                        self.send_failure(
                            candidate,
                            BridgeRejectionCause::Internal,
                            "revoke host replay has no matching durable receipt",
                            Some(reply_address.as_str()),
                        )
                        .await;
                        return;
                    }
                    Err(error) => {
                        self.send_failure(
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!("revoke host receipt load failed: {error}"),
                            Some(reply_address.as_str()),
                        )
                        .await;
                        return;
                    }
                };
                if let Err(error) = self
                    .remove_supervisor_trust_if_unused(&receipt.supervisor_peer_id)
                    .await
                {
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("revoke host replay trust cleanup failed: {error}"),
                        Some(reply_address.as_str()),
                    )
                    .await;
                    return;
                }
                self.send_host_revoked_reply(candidate, &payload, &receipt)
                    .await;
            }
            MobHostBindingAuthorityEffect::HostBindingRevoked { .. } => {
                let expected = match self.persistence.load(&payload.mob_id).await {
                    Ok(Some(record)) => record,
                    Ok(None) => {
                        self.send_failure(
                            candidate,
                            BridgeRejectionCause::Internal,
                            "revoke host admitted without a durable active binding",
                            Some(reply_address.as_str()),
                        )
                        .await;
                        return;
                    }
                    Err(error) => {
                        self.send_failure(
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!("revoke host binding load failed: {error}"),
                            Some(reply_address.as_str()),
                        )
                        .await;
                        return;
                    }
                };
                let witness = match HostBindingDeletionAuthority::from_transition(&mob, &transition)
                {
                    Ok(witness) => witness,
                    Err(error) => {
                        self.send_failure(
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!("revoke host witness failed: {error}"),
                            Some(reply_address.as_str()),
                        )
                        .await;
                        return;
                    }
                };
                let residency_updates = match self
                    .cleanup_revoked_binding_members(&payload.mob_id, &expected, &witness)
                    .await
                {
                    Ok(updates) => updates,
                    Err(error) => {
                        // Prepared authority is dropped: binding + every durable
                        // row remain the retry anchor. The disposal arc has not
                        // certified every live channel absent, so no revocation
                        // receipt may be recorded. Any already-disposed row
                        // converges through idempotent disposal on the next call.
                        self.send_failure(
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!("revoke host member cleanup failed: {error}"),
                            Some(reply_address.as_str()),
                        )
                        .await;
                        return;
                    }
                };
                // Every identity in this receipt passed the disposal arc while
                // its exclusive residency update was held. Membership is
                // therefore also the durable proof that status(None) found no
                // live channel or exact close succeeded before this terminal.
                let receipt = MobHostRevocationReceipt {
                    supervisor_peer_id: expected.supervisor_peer_id.clone(),
                    supervisor_signing_key: expected.supervisor_signing_key,
                    epoch: expected.epoch,
                    binding_generation: expected.binding_generation,
                    released_members: expected.materialized.keys().cloned().collect(),
                };
                let write_outcome = self
                    .persistence
                    .revoke(&payload.mob_id, &expected, &receipt, &witness)
                    .await;
                let convergence = match write_outcome {
                    Ok(true) => Ok(()),
                    Ok(false) => {
                        require_exact_revocation_after_uncertain_write(
                            self.persistence.as_ref(),
                            &payload.mob_id,
                            &receipt,
                            "revoke CAS miss",
                        )
                        .await
                    }
                    Err(error) => {
                        require_exact_revocation_after_uncertain_write(
                            self.persistence.as_ref(),
                            &payload.mob_id,
                            &receipt,
                            &format!("ambiguous revoke write error: {error}"),
                        )
                        .await
                    }
                };
                if let Err(error) = convergence {
                    self.durable_uncertainty_fail_stop = Some(error.to_string());
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("revoke host durable terminal failed: {error}"),
                        Some(reply_address.as_str()),
                    )
                    .await;
                    return;
                }
                if let Err(error) = self.binding_authority.commit_prepared_authority(prepared) {
                    self.durable_uncertainty_fail_stop = Some(format!(
                        "revoke host durable receipt committed but prepared authority commit failed: {error:?}"
                    ));
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("revoke host prepared commit failed: {error:?}"),
                        Some(reply_address.as_str()),
                    )
                    .await;
                    return;
                }
                let mut residency_publications = Vec::with_capacity(residency_updates.len());
                for (session_id, update) in residency_updates {
                    let publication = match update.vacate() {
                        Ok(publication) => publication,
                        Err(error) => {
                            self.durable_uncertainty_fail_stop = Some(format!(
                                "revoke durable commit succeeded but residency vacancy publication failed: {error}"
                            ));
                            self.send_failure(
                                candidate,
                                BridgeRejectionCause::Internal,
                                format!("revoke residency vacancy publication failed: {error}"),
                                Some(reply_address.as_str()),
                            )
                            .await;
                            return;
                        }
                    };
                    self.registered_member_incarnations.remove(&session_id);
                    residency_publications.push(publication);
                }
                self.unrevived
                    .retain(|(mob_id, _)| mob_id != &payload.mob_id);
                self.sync_member_observation().await;
                drop(residency_publications);
                if let Err(error) = self
                    .remove_supervisor_trust_if_unused(&receipt.supervisor_peer_id)
                    .await
                {
                    // The durable terminal already holds. Reply failure is
                    // intentional: exact retry replays the receipt and
                    // re-attempts this transport hygiene step.
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("revoke host trust cleanup failed: {error}"),
                        Some(reply_address.as_str()),
                    )
                    .await;
                    return;
                }
                self.send_host_revoked_reply(candidate, &payload, &receipt)
                    .await;
            }
            other => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("unexpected revoke host effect: {other:?}"),
                    Some(reply_address.as_str()),
                )
                .await;
            }
        }
    }

    async fn cleanup_revoked_binding_members(
        &mut self,
        mob_id: &str,
        record: &MobHostBindingRecord,
        witness: &HostBindingDeletionAuthority,
    ) -> Result<
        Vec<(
            meerkat_core::types::SessionId,
            meerkat_runtime::meerkat_machine::MemberResidencyUpdate,
        )>,
        MobHostActorError,
    > {
        witness.verify_mob(mob_id)?;
        if record.materialized.is_empty() {
            // No residency to dispose, but a retained capability obligation
            // still dies with the record, so it must be reconciled first.
            for obligation in record.forked_participant_obligations.values() {
                release_forked_participant_association(
                    self.forked_participant_service.as_ref(),
                    &obligation.association,
                )
                .await
                .map_err(|failure| MobHostActorError::Internal {
                    detail: format!(
                        "retained capability attachment obligation for '{}' could not be \
                         reconciled before revocation: {failure}",
                        obligation.agent_identity
                    ),
                })?;
                self.forked_participant_debts
                    .remove(&(mob_id.to_string(), obligation.association.association_key()));
            }
            return Ok(Vec::new());
        }
        let Some(materializer) = self.materializer.as_mut() else {
            return Err(MobHostActorError::Internal {
                detail: format!(
                    "mob '{mob_id}' has materialized rows but the host has no member substrate"
                ),
            });
        };
        let persistent = materializer.substrate().realm_backend_persistent;
        let runtime_adapter = Arc::clone(&materializer.substrate().runtime_adapter);

        // Validate every durable address before the first teardown effect so
        // corrupt rows fail without needlessly creating partial cleanup.
        let mut members = Vec::with_capacity(record.materialized.len());
        for (identity, row) in &record.materialized {
            let session_id =
                meerkat_core::types::SessionId::parse(&row.session_id).map_err(|error| {
                    MobHostActorError::Internal {
                        detail: format!(
                            "revoke host row for '{identity}' has invalid session id: {error}"
                        ),
                    }
                })?;
            let pubkey =
                meerkat_comms::PubKey::from_pubkey_string(&row.member_pubkey).map_err(|error| {
                    MobHostActorError::Internal {
                        detail: format!(
                            "revoke host row for '{identity}' has invalid member pubkey: {error}"
                        ),
                    }
                })?;
            members.push((
                identity.clone(),
                session_id,
                pubkey,
                row.forked_participant_attachment.clone(),
            ));
        }

        let mut residency_updates = Vec::with_capacity(members.len());
        for (identity, session_id, pubkey, association) in members {
            let update = runtime_adapter
                .begin_member_residency_update(session_id.clone())
                .await;
            if persistent {
                materializer.dispose(&session_id).await.map_err(|error| {
                    MobHostActorError::Internal {
                        detail: format!("member '{identity}' session disposal failed: {error}"),
                    }
                })?;
            } else {
                materializer
                    .release_runtime_only(&session_id)
                    .await
                    .map_err(|error| MobHostActorError::Internal {
                        detail: format!("member '{identity}' runtime release failed: {error}"),
                    })?;
            }
            // Disposal proved this residency gone, so its attachment must go
            // with it — and it must go BEFORE any revoke receipt, because the
            // receipt deletes the row that carries the association. A failure
            // here aborts the revocation: durable rows stay, the receipt is
            // never written, and the exact retry converges. Revoke REPLAY
            // answers from the receipt without re-entering this arm, so no
            // attachment is released twice.
            if let Some(association) = association.as_ref() {
                release_forked_participant_association(
                    self.forked_participant_service.as_ref(),
                    association,
                )
                .await
                .map_err(|failure| MobHostActorError::Internal {
                    detail: format!(
                        "member '{identity}' capability attachment release failed: {failure}"
                    ),
                })?;
            }
            self.registry
                .remove_identity(&self.registry_owner, &pubkey)?;
            self.unrevived
                .remove(&(mob_id.to_string(), identity.clone()));
            residency_updates.push((session_id, update));
        }
        // Retained reconciliation obligations die with the record, so they are
        // discharged here or the revocation does not happen. Every member
        // residency on this binding has just been disposed, so absence is
        // established rather than assumed.
        for obligation in record.forked_participant_obligations.values() {
            release_forked_participant_association(
                self.forked_participant_service.as_ref(),
                &obligation.association,
            )
            .await
            .map_err(|failure| MobHostActorError::Internal {
                detail: format!(
                    "retained capability attachment obligation for '{}' could not be reconciled \
                     before revocation: {failure}",
                    obligation.agent_identity
                ),
            })?;
            self.forked_participant_debts
                .remove(&(mob_id.to_string(), obligation.association.association_key()));
        }
        Ok(residency_updates)
    }

    async fn remove_supervisor_trust_if_unused(
        &self,
        peer_id: &str,
    ) -> Result<(), MobHostActorError> {
        if self
            .binding_authority
            .state()
            .supervisor_peer_ids
            .values()
            .any(|active| active.0 == peer_id)
        {
            return Ok(());
        }
        self.remove_peer_trust(peer_id).await
    }

    async fn send_host_revoked_reply(
        &self,
        candidate: &PeerInputCandidate,
        payload: &BridgeHostRevokePayload,
        receipt: &MobHostRevocationReceipt,
    ) {
        let Some(host_peer_id) = self.host_comms.peer_id() else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                "revoke host completed but host runtime peer_id is unavailable",
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return;
        };
        self.send_reply(
            candidate,
            HostBridgeReply::completed(BridgeReply::HostRevoked(BridgeHostRevokedResponse {
                host_peer_id: host_peer_id.as_str(),
                mob_id: payload.mob_id.clone(),
                epoch: receipt.epoch,
                binding_generation: receipt.binding_generation,
                released_members: receipt.released_members.clone(),
            })),
            Some(payload.supervisor.address.as_str()),
        )
        .await;
    }

    async fn refresh_live_member_supervisors(
        &mut self,
        mob_id: &str,
        supervisor: &TrustedPeerDescriptor,
        epoch: u64,
        binding_generation: u64,
    ) -> Result<(), MobHostActorError> {
        let host_id = self
            .host_comms
            .peer_id()
            .ok_or_else(|| MobHostActorError::Internal {
                detail: "member supervisor refresh: host runtime peer_id unavailable".to_string(),
            })?
            .to_string();
        let mob = AuthorityMobId::from(mob_id.to_string());
        let sessions: Vec<String> = self
            .binding_authority
            .state()
            .materialized_sessions
            .iter()
            .filter(|(key, _)| key.mob_id == mob)
            .map(|(_, session)| session.0.clone())
            .collect();
        let Some(materializer) = self.materializer.as_mut() else {
            return Ok(());
        };
        for session_id in sessions {
            materializer
                .refresh_live_supervisor(
                    &session_id,
                    supervisor,
                    epoch,
                    host_id.as_str(),
                    binding_generation,
                )
                .await
                .map_err(|error| MobHostActorError::Internal {
                    detail: format!(
                        "member session '{session_id}' supervisor refresh failed: {error}"
                    ),
                })?;
        }
        Ok(())
    }

    /// Record the inbound request on the generated peer-interaction
    /// authority. `false` means the candidate was completed and must not be
    /// decoded or served.
    fn record_inbound_request(&self, candidate: &PeerInputCandidate) -> bool {
        let Some(handle) = self.host_comms.peer_request_response_authority_handle() else {
            tracing::warn!(
                interaction_id = %candidate.interaction.id,
                "mob host: rejected bridge request without complete peer request authority"
            );
            self.host_comms
                .mark_interaction_complete(&candidate.interaction.id);
            return false;
        };
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(candidate.interaction.id.0);
        if handle.inbound_state(corr_id).is_some() {
            return true;
        }
        if let Err(err) = handle.request_received(corr_id, candidate.interaction.handling_mode) {
            tracing::warn!(
                error = %err,
                corr_id = %corr_id,
                "mob host: PeerInteractionHandle::request_received rejected bridge command"
            );
            self.host_comms
                .mark_interaction_complete(&candidate.interaction.id);
            return false;
        }
        true
    }

    /// Install the bound supervisor as a trusted direct peer on the host
    /// runtime, machine-gated through the host's `HandleDslAuthority` and
    /// realized via the generated trust-reconcile obligation (the
    /// `from_transition` witness posture, FLAG-4). Idempotent: an already
    /// trusted endpoint is a no-op.
    async fn install_supervisor_trust(
        &self,
        supervisor: TrustedPeerDescriptor,
    ) -> Result<(), MobHostActorError> {
        self.publish_local_endpoint()?;
        let endpoint = mm_dsl::PeerEndpoint::from(&supervisor);
        // A restarted controller commonly keeps its key/PeerId while moving
        // to a fresh ephemeral address. Generated trust is exact-endpoint
        // authority, so remove every stale same-peer row before adding the
        // refreshed endpoint (the member trust path uses the same two-step
        // transition). Leaving both rows makes the router reject the add as a
        // conflicting generated source and permanently pins recovery to the
        // dead address.
        loop {
            let stale = self
                .host_dsl
                .snapshot_state()
                .direct_peer_endpoints
                .iter()
                .find(|existing| existing.peer_id == endpoint.peer_id && **existing != endpoint)
                .cloned();
            let Some(stale) = stale else {
                break;
            };
            self.remove_exact_peer_trust(stale).await?;
        }
        if self
            .host_dsl
            .snapshot_state()
            .direct_peer_endpoints
            .contains(&endpoint)
        {
            return Ok(());
        }
        let transition = self
            .host_dsl
            .apply_input_with_transition(
                mm_dsl::MeerkatMachineInput::AddDirectPeerEndpoint { endpoint },
                "mob_host_actor::install_supervisor_trust",
            )
            .map_err(|err| MobHostActorError::Comms {
                detail: format!("host trust projection rejected: {err}"),
            })?;
        let obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                self.host_dsl.peer_projection_freshness_authority(),
            );
        let obligation = match obligations.as_slice() {
            [obligation] => obligation.clone(),
            [] => {
                return Err(MobHostActorError::Comms {
                    detail: "host trust projection emitted no reconcile request".to_string(),
                });
            }
            _ => {
                return Err(MobHostActorError::Comms {
                    detail: "host trust projection emitted multiple reconcile requests".to_string(),
                });
            }
        };
        meerkat_runtime::comms_trust_reconcile::CommsTrustReconciler::new(Arc::clone(
            &self.host_comms,
        ))
        .reconcile(&obligation)
        .await
        .map(|_report| ())
        .map_err(|err| MobHostActorError::Comms {
            detail: format!("host trust reconciliation failed: {err}"),
        })
    }

    /// Remove a trusted direct peer endpoint from the host runtime (the
    /// rotated-out supervisor). No-ops when the endpoint is not present.
    async fn remove_peer_trust(&self, peer_id: &str) -> Result<(), MobHostActorError> {
        let snapshot = self.host_dsl.snapshot_state();
        let Some(endpoint) = snapshot
            .direct_peer_endpoints
            .iter()
            .find(|endpoint| endpoint.peer_id.0 == peer_id)
            .cloned()
        else {
            return Ok(());
        };
        self.remove_exact_peer_trust(endpoint).await
    }

    async fn remove_exact_peer_trust(
        &self,
        endpoint: mm_dsl::PeerEndpoint,
    ) -> Result<(), MobHostActorError> {
        let transition = self
            .host_dsl
            .apply_input_with_transition(
                mm_dsl::MeerkatMachineInput::RemoveDirectPeerEndpoint { endpoint },
                "mob_host_actor::remove_peer_trust",
            )
            .map_err(|err| MobHostActorError::Comms {
                detail: format!("host trust removal projection rejected: {err}"),
            })?;
        let obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                self.host_dsl.peer_projection_freshness_authority(),
            );
        let obligation = match obligations.as_slice() {
            [obligation] => obligation.clone(),
            [] => {
                return Err(MobHostActorError::Comms {
                    detail: "host trust removal projection emitted no reconcile request"
                        .to_string(),
                });
            }
            _ => {
                return Err(MobHostActorError::Comms {
                    detail: "host trust removal projection emitted multiple reconcile requests"
                        .to_string(),
                });
            }
        };
        meerkat_runtime::comms_trust_reconcile::CommsTrustReconciler::new(Arc::clone(
            &self.host_comms,
        ))
        .reconcile(&obligation)
        .await
        .map(|_report| ())
        .map_err(|err| MobHostActorError::Comms {
            detail: format!("host trust removal reconciliation failed: {err}"),
        })
    }

    fn publish_local_endpoint(&self) -> Result<(), MobHostActorError> {
        let missing = |what: &str| MobHostActorError::Comms {
            detail: format!("host runtime {what} unavailable"),
        };
        let name = self
            .host_comms
            .comms_name()
            .ok_or_else(|| missing("comms_name"))?;
        let peer_id = self
            .host_comms
            .peer_id()
            .ok_or_else(|| missing("peer_id"))?;
        let pubkey = self
            .host_comms
            .public_key_bytes()
            .ok_or_else(|| missing("public key"))?;
        let address = self
            .host_comms
            .advertised_address()
            .ok_or_else(|| missing("advertised address"))?;
        let self_descriptor =
            TrustedPeerDescriptor::unsigned_with_pubkey(name, peer_id.as_str(), pubkey, address)
                .map_err(|err| MobHostActorError::Comms {
                    detail: format!("host self endpoint invalid: {err}"),
                })?;
        self.host_dsl
            .apply_input(
                mm_dsl::MeerkatMachineInput::PublishLocalEndpoint {
                    endpoint: mm_dsl::PeerEndpoint::from(&self_descriptor),
                },
                "mob_host_actor::publish_local_endpoint",
            )
            .map_err(|err| MobHostActorError::Comms {
                detail: format!("host local endpoint publication rejected: {err}"),
            })?;
        Ok(())
    }

    async fn send_failure(
        &self,
        candidate: &PeerInputCandidate,
        cause: BridgeRejectionCause,
        reason: impl Into<String>,
        _declared_reply_address: Option<&str>,
    ) {
        self.send_reply(candidate, HostBridgeReply::rejected(cause, reason), None)
            .await;
    }

    async fn send_reply(
        &self,
        candidate: &PeerInputCandidate,
        reply: HostBridgeReply,
        _declared_reply_address: Option<&str>,
    ) {
        // Status construction and the serialization-failure downgrade (never
        // a completed status carrying a rejected-shaped body) live in the
        // host_reply constructor seam (DEC-P3F-5).
        let (status, result) = reply.into_wire(candidate.interaction.id);
        let Some(to) = self.resolve_response_route(candidate).await else {
            tracing::warn!(
                interaction_id = %candidate.interaction.id,
                "mob host: failed to resolve bridge response peer route"
            );
            self.host_comms
                .mark_interaction_complete(&candidate.interaction.id);
            return;
        };
        stage_authenticated_correlated_reply_endpoint(&self.host_comms, candidate).await;
        if let Err(error) = self
            .host_comms
            .send(CommsCommand::PeerResponse {
                to,
                in_reply_to: candidate.interaction.id,
                status,
                result,
                blocks: None,
                content_taint: None,
                handling_mode: None,
                objective_id: candidate.interaction.objective_id,
            })
            .await
        {
            if let Some(sender_peer_id) = candidate.ingress.canonical_peer_id
                && candidate.ingress.declared_reply_endpoint.is_some()
                && let Err(cleanup_error) = self
                    .host_comms
                    .unstage_correlated_reply_endpoint(sender_peer_id, candidate.interaction.id)
                    .await
                && !matches!(cleanup_error, SendError::Unsupported(_))
            {
                tracing::warn!(
                    interaction_id = %candidate.interaction.id,
                    error = %cleanup_error,
                    "mob host: failed to clear correlated reply endpoint after response failure"
                );
            }
            tracing::warn!(
                interaction_id = %candidate.interaction.id,
                error = %error,
                "mob host: failed to send bridge response"
            );
        }
        self.host_comms
            .mark_interaction_complete(&candidate.interaction.id);
    }

    async fn resolve_response_route(&self, candidate: &PeerInputCandidate) -> Option<PeerRoute> {
        if let Some(sender_route) = candidate.ingress.route.clone() {
            if let Some(route) = self.resolve_peer_route(sender_route.peer_id).await {
                return Some(route);
            }
            return Some(sender_route);
        }
        if let Some(sender_peer_id) = candidate.ingress.canonical_peer_id {
            return self
                .resolve_peer_route(sender_peer_id)
                .await
                .or_else(|| Some(PeerRoute::new(sender_peer_id)));
        }
        None
    }

    async fn resolve_peer_route(&self, peer_id: meerkat_core::comms::PeerId) -> Option<PeerRoute> {
        let peers = self.host_comms.peers().await;
        peers
            .iter()
            .find(|entry| entry.peer_id == peer_id)
            .map(|entry| PeerRoute::with_display_name(entry.peer_id, entry.name.clone()))
    }
}

/// Stage transport for this exact host-bridge response. Only the signed
/// Request envelope's correlation-bound reply endpoint is eligible. A
/// payload-declared supervisor address is domain data and must never mint
/// callback routing authority, even for an otherwise untrusted sender.
async fn stage_authenticated_correlated_reply_endpoint(
    host_comms: &Arc<dyn CoreCommsRuntime>,
    candidate: &PeerInputCandidate,
) {
    if let Some((sender_peer_id, signing_pubkey, endpoint)) =
        authenticated_correlated_reply_endpoint(candidate)
    {
        match host_comms
            .stage_correlated_reply_endpoint(
                sender_peer_id,
                candidate.interaction.id,
                signing_pubkey,
                endpoint,
            )
            .await
        {
            Ok(()) | Err(SendError::Unsupported(_)) => {}
            Err(error) => {
                tracing::warn!(
                    interaction_id = %candidate.interaction.id,
                    error = %error,
                    "mob host: failed to stage authenticated correlated reply endpoint"
                );
            }
        }
    }
}

/// Select the sole callback-routing authority accepted by the host actor:
/// the endpoint carried inside the signed request envelope, paired with the
/// ingress-authenticated peer id and signing key. Payload fields are absent
/// from this API by construction.
fn authenticated_correlated_reply_endpoint(
    candidate: &PeerInputCandidate,
) -> Option<(PeerId, [u8; 32], PeerAddress)> {
    Some((
        candidate.ingress.canonical_peer_id?,
        candidate.ingress.signing_pubkey?,
        candidate.ingress.declared_reply_endpoint.clone()?,
    ))
}

/// Canonical-identity sender match: the admitted ingress fact's canonical
/// peer id, or the signed pubkey when present — never a display name.
fn sender_matches_bridge_peer(sender: &PeerIngressFact, peer: &BridgePeerIdentity) -> bool {
    sender
        .canonical_peer_id
        .is_some_and(|sender_peer_id| sender_peer_id == peer.peer_id)
        || (!peer.pubkey.is_zero() && sender.signing_pubkey == Some(*peer.pubkey.as_bytes()))
}

/// Match a materialize payload's declared supervisor to the binding tuple
/// already authenticated by `ResolveHostCommandAdmission`. Both canonical
/// peer id and signing key must agree with durable machine truth; display
/// name and route address are non-authoritative transport metadata.
fn declared_supervisor_matches_recorded_host_authority(
    state: &MobHostBindingAuthorityState,
    mob_id: &str,
    supervisor: &BridgePeerIdentity,
) -> bool {
    let mob = AuthorityMobId::from(mob_id.to_string());
    let (Some(recorded_peer), Some(recorded_key)) = (
        state.supervisor_peer_ids.get(&mob),
        state.supervisor_signing_keys.get(&mob),
    ) else {
        return false;
    };
    supervisor.peer_id.as_str() == recorded_peer.0
        && supervisor.pubkey.as_bytes() == &recorded_key.0
}

/// Authenticate a host-authority rotation against durable machine truth.
/// The proposed next supervisor is intentionally absent from this check: a
/// self-consistent attacker payload conveys no authority.
fn sender_matches_recorded_host_supervisor(
    state: &MobHostBindingAuthorityState,
    mob_id: &str,
    sender: &PeerIngressFact,
) -> bool {
    let mob = AuthorityMobId::from(mob_id.to_string());
    let (Some(recorded_peer), Some(recorded_key)) = (
        state.supervisor_peer_ids.get(&mob),
        state.supervisor_signing_keys.get(&mob),
    ) else {
        return false;
    };
    sender
        .canonical_peer_id
        .is_some_and(|peer_id| peer_id.as_str() == recorded_peer.0)
        && sender.signing_pubkey == Some(recorded_key.0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::HandlingMode;
    use meerkat_core::interaction::{
        InboxInteraction, InteractionId, PeerIngressAuthDecision, PeerIngressAuthExemption,
        PeerIngressIdentity, PeerIngressKind, PeerInputClass,
    };

    #[test]
    fn required_host_capabilities_fail_before_authority_mutation() {
        let required = BridgeHostCapabilityRequirements {
            durable_sessions: true,
            autonomous_members: true,
            tracked_input_cancel: true,
            protocol_v4: true,
        };
        let capabilities = BridgeCapabilities {
            supported_protocol_versions: vec![BridgeProtocolVersion::V3],
            durable_sessions: false,
            autonomous_members: false,
            tracked_input_cancel: false,
            ..BridgeCapabilities::default()
        };

        assert_eq!(
            missing_host_capabilities(required, &capabilities),
            vec![
                "durable_sessions",
                "autonomous_members",
                "tracked_input_cancel",
                "protocol_v4",
            ]
        );
    }

    #[test]
    fn acknowledged_turn_tombstone_retains_tracked_host_contract() {
        let record = MobHostBindingRecord {
            supervisor_peer_id: "controller".to_string(),
            supervisor_signing_key: [7; 32],
            epoch: 1,
            binding_generation: 1,
            accepted_capabilities: None,
            materialized: BTreeMap::new(),
            released: BTreeMap::new(),
            turn_outcomes: BTreeMap::new(),
            turn_outcome_acknowledged: BTreeMap::from([(
                "worker".to_string(),
                vec![TurnOutcomeAcknowledgedRow {
                    input_id: "input-1".to_string(),
                    generation: 1,
                    fence_token: 11,
                }],
            )]),
            tracked_input_cancellations: BTreeMap::new(),
            forked_participant_obligations: BTreeMap::new(),
            turn_outcome_pending: BTreeMap::new(),
        };

        assert_eq!(
            retained_host_capability_requirements(Some(&record)),
            BridgeHostCapabilityRequirements {
                durable_sessions: true,
                autonomous_members: false,
                tracked_input_cancel: true,
                protocol_v4: true,
            }
        );
    }

    #[test]
    fn exact_binding_replay_uses_capabilities_committed_with_authority() {
        let signing_key = [9u8; 32];
        let peer_id = PeerId::from_ed25519_pubkey(&signing_key);
        let supervisor_spec = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: peer_id.to_string(),
            address: "tcp://127.0.0.1:4000".to_string(),
            pubkey: signing_key,
        };
        let supervisor =
            BridgePeerIdentity::try_from(&supervisor_spec).expect("valid supervisor identity");
        let accepted = BridgeCapabilities {
            durable_sessions: true,
            tracked_input_cancel: true,
            engine_version: "accepted".to_string(),
            ..BridgeCapabilities::default()
        };
        let record = MobHostBindingRecord {
            supervisor_peer_id: peer_id.to_string(),
            supervisor_signing_key: signing_key,
            epoch: 7,
            binding_generation: 11,
            accepted_capabilities: Some(accepted.clone()),
            materialized: BTreeMap::new(),
            released: BTreeMap::new(),
            turn_outcomes: BTreeMap::new(),
            turn_outcome_acknowledged: BTreeMap::new(),
            tracked_input_cancellations: BTreeMap::new(),
            forked_participant_obligations: BTreeMap::new(),
            turn_outcome_pending: BTreeMap::new(),
        };

        assert_eq!(
            exact_binding_capability_snapshot(Some(&record), &supervisor, 7, 11),
            Some(accepted)
        );
        assert_eq!(
            exact_binding_capability_snapshot(Some(&record), &supervisor, 8, 11),
            None,
            "a fresh authority tuple must be adjudicated against current capabilities"
        );
    }

    type RecordedCorrelatedReplyEndpoint = (PeerId, InteractionId, [u8; 32], PeerAddress);

    struct RecordingReplyStageRuntime {
        notify: Arc<tokio::sync::Notify>,
        staged: Arc<std::sync::Mutex<Vec<RecordedCorrelatedReplyEndpoint>>>,
    }

    impl RecordingReplyStageRuntime {
        fn new() -> Self {
            Self {
                notify: Arc::new(tokio::sync::Notify::new()),
                staged: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn staged(&self) -> Vec<RecordedCorrelatedReplyEndpoint> {
            self.staged
                .lock()
                .expect("recorded correlated reply endpoint mutex")
                .clone()
        }
    }

    #[async_trait]
    impl CoreCommsRuntime for RecordingReplyStageRuntime {
        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            Arc::clone(&self.notify)
        }

        async fn stage_correlated_reply_endpoint(
            &self,
            dest: PeerId,
            in_reply_to: InteractionId,
            signer_pubkey: [u8; 32],
            declared_endpoint: PeerAddress,
        ) -> Result<(), SendError> {
            self.staged
                .lock()
                .expect("recorded correlated reply endpoint mutex")
                .push((dest, in_reply_to, signer_pubkey, declared_endpoint));
            Ok(())
        }
    }

    fn bind_candidate_with_callback_facts(
        payload_address: &str,
        ingress_reply_endpoint: Option<&str>,
        signed_ingress: bool,
    ) -> PeerInputCandidate {
        let interaction_id = InteractionId(uuid::Uuid::new_v4());
        let signing_key = [7u8; 32];
        let sender_peer_id = PeerId::from_ed25519_pubkey(&signing_key);
        let host_peer_id = PeerId::from_ed25519_pubkey(&[9u8; 32]);
        let payload = BridgeHostBindPayload {
            supervisor: meerkat_contracts::wire::supervisor_bridge::BridgePeerSpec {
                name: "mob/__mob_supervisor__".to_string(),
                peer_id: sender_peer_id.to_string(),
                address: payload_address.to_string(),
                pubkey: signing_key,
            },
            epoch: 1,
            binding_generation: 1,
            protocol_version: meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V4,
            mob_id: "mob-callback-regression".to_string(),
            expected_host_peer_id: host_peer_id.to_string(),
            expected_address: "tcp://127.0.0.1:3000".to_string(),
            bootstrap_proof:
                meerkat_contracts::wire::supervisor_bridge::BridgeHostBootstrapProof::new(""),
            delegated_bootstrap_proof: None,
            required_capabilities: Default::default(),
        };
        let command = BridgeCommand::BindHost(
            crate::runtime::bridge_protocol::seal_host_bind_bootstrap_proof(
                payload,
                &BridgeBootstrapToken::new("test-token"),
            ),
        );
        let params = serde_json::to_value(command).expect("BindHost serializes");
        let interaction = InboxInteraction {
            sender_taint: None,
            id: interaction_id,
            from_route: Some(sender_peer_id),
            from: "mob/__mob_supervisor__".to_string(),
            content: InteractionContent::Request {
                intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                params,
                blocks: None,
            },
            rendered_text: "BindHost callback regression".to_string(),
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
            objective_id: None,
        };
        let identity = PeerIngressIdentity::new(
            sender_peer_id,
            "mob/__mob_supervisor__",
            meerkat_core::interaction::PeerIngressConvention::Request {
                request_id: interaction_id.to_string(),
                intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
            },
        );
        let identity = if signed_ingress {
            identity.with_signing_pubkey(signing_key)
        } else {
            identity
        };
        let ingress = PeerIngressFact::peer(
            interaction_id,
            PeerInputClass::ActionableRequest,
            PeerIngressKind::Request,
            Some(PeerIngressAuthDecision::Exempt(
                PeerIngressAuthExemption::SupervisorBridge,
            )),
            identity,
        )
        .with_declared_reply_endpoint(
            ingress_reply_endpoint
                .map(|address| PeerAddress::parse(address).expect("test callback endpoint parses")),
        );
        PeerInputCandidate::new(interaction, ingress, None)
    }

    #[tokio::test]
    async fn bind_payload_tcp_and_uds_addresses_never_mint_callback_authority() {
        let recording = Arc::new(RecordingReplyStageRuntime::new());
        let runtime: Arc<dyn CoreCommsRuntime> = recording.clone();
        for payload_address in [
            "tcp://127.0.0.1:6553",
            "uds:///tmp/meerkat-malicious-bind-callback.sock",
        ] {
            let candidate = bind_candidate_with_callback_facts(payload_address, None, true);
            let InteractionContent::Request { params, .. } = &candidate.interaction.content else {
                panic!("fixture must carry a BindHost request");
            };
            let BridgeCommand::BindHost(payload) =
                decode_bridge_command(params.clone()).expect("fixture BindHost decodes")
            else {
                panic!("fixture must decode as BindHost");
            };
            assert_eq!(payload.supervisor.address, payload_address);
            assert!(
                authenticated_correlated_reply_endpoint(&candidate).is_none(),
                "payload address {payload_address} must not become callback routing authority"
            );
            stage_authenticated_correlated_reply_endpoint(&runtime, &candidate).await;
        }

        let unsigned = bind_candidate_with_callback_facts(
            "tcp://127.0.0.1:6554",
            Some("tcp://127.0.0.1:6556"),
            false,
        );
        assert!(
            authenticated_correlated_reply_endpoint(&unsigned).is_none(),
            "open-auth/unsigned BindHost ingress cannot stage even endpoint-shaped metadata"
        );
        stage_authenticated_correlated_reply_endpoint(&runtime, &unsigned).await;
        assert!(
            recording.staged().is_empty(),
            "payload TCP/UDS and open-auth/unsigned ingress must make zero real staging calls"
        );
    }

    #[tokio::test]
    async fn signed_ingress_correlated_tcp_callback_remains_authoritative() {
        let recording = Arc::new(RecordingReplyStageRuntime::new());
        let runtime: Arc<dyn CoreCommsRuntime> = recording.clone();
        let candidate = bind_candidate_with_callback_facts(
            "uds:///tmp/meerkat-untrusted-payload-callback.sock",
            Some("tcp://127.0.0.1:6555"),
            true,
        );
        let expected_peer_id = candidate.ingress.canonical_peer_id.expect("sender peer id");
        let expected_key = candidate
            .ingress
            .signing_pubkey
            .expect("sender signing key");
        let expected_endpoint = candidate
            .ingress
            .declared_reply_endpoint
            .clone()
            .expect("signed request callback endpoint");
        assert_eq!(
            authenticated_correlated_reply_endpoint(&candidate),
            Some((expected_peer_id, expected_key, expected_endpoint)),
            "the exact signed request callback must remain available"
        );
        stage_authenticated_correlated_reply_endpoint(&runtime, &candidate).await;
        assert_eq!(
            recording.staged(),
            vec![(
                expected_peer_id,
                candidate.interaction.id,
                expected_key,
                candidate
                    .ingress
                    .declared_reply_endpoint
                    .clone()
                    .expect("signed request callback endpoint"),
            )],
            "production host staging must preserve the exact peer, correlation, signer, and callback tuple"
        );
    }

    #[derive(Clone)]
    struct LoadOnlyHostBindingPersistence {
        record: Option<MobHostBindingRecord>,
    }

    #[async_trait]
    impl MobHostBindingPersistence for LoadOnlyHostBindingPersistence {
        async fn list_records(
            &self,
        ) -> Result<Vec<(String, MobHostBindingRecord)>, MobHostActorError> {
            Ok(Vec::new())
        }

        async fn load(
            &self,
            _mob_id: &str,
        ) -> Result<Option<MobHostBindingRecord>, MobHostActorError> {
            Ok(self.record.clone())
        }

        async fn list_revocations(
            &self,
        ) -> Result<Vec<(String, MobHostRevocationReceipt)>, MobHostActorError> {
            Ok(Vec::new())
        }

        async fn load_revocation(
            &self,
            _mob_id: &str,
        ) -> Result<Option<MobHostRevocationReceipt>, MobHostActorError> {
            unreachable!("load-only test persistence")
        }

        async fn put_if_absent(
            &self,
            _mob_id: &str,
            _record: &MobHostBindingRecord,
            _authority: &HostBindingPersistenceAuthority,
        ) -> Result<bool, MobHostActorError> {
            unreachable!("load-only test persistence")
        }

        async fn compare_and_put(
            &self,
            _mob_id: &str,
            _expected: &MobHostBindingRecord,
            _next: &MobHostBindingRecord,
            _authority: &HostBindingPersistenceAuthority,
        ) -> Result<bool, MobHostActorError> {
            unreachable!("load-only test persistence")
        }

        async fn compare_and_put_member_rows(
            &self,
            _mob_id: &str,
            _expected: &MobHostBindingRecord,
            _next: &MobHostBindingRecord,
            _authority: &MemberRowPersistenceAuthority,
        ) -> Result<bool, MobHostActorError> {
            unreachable!("load-only test persistence")
        }

        async fn revoke(
            &self,
            _mob_id: &str,
            _expected: &MobHostBindingRecord,
            _receipt: &MobHostRevocationReceipt,
            _authority: &HostBindingDeletionAuthority,
        ) -> Result<bool, MobHostActorError> {
            unreachable!("load-only test persistence")
        }
    }

    struct EmptyProviderPresenceProbe;

    #[async_trait]
    impl ProviderPresenceProbe for EmptyProviderPresenceProbe {
        async fn resolvable_providers(
            &self,
        ) -> Result<Vec<meerkat_core::Provider>, ProviderPresenceProbeError> {
            Ok(Vec::new())
        }
    }

    struct FailFirstDescriptorSink {
        attempts: std::sync::atomic::AtomicUsize,
    }

    impl FailFirstDescriptorSink {
        fn new() -> Self {
            Self {
                attempts: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn attempts(&self) -> usize {
            self.attempts.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl HostDescriptorSink for FailFirstDescriptorSink {
        fn publish(&self, _descriptor_json: &str) -> Result<(), String> {
            let attempt = self
                .attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if attempt == 0 {
                Err("injected first descriptor publication failure".to_string())
            } else {
                Ok(())
            }
        }
    }

    fn test_member_incarnation()
    -> meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
        meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
            mob_id: "mob-1".to_string(),
            agent_identity: "worker-1".to_string(),
            host_id: "host-1".to_string(),
            binding_generation: 1,
            member_session_id: "session-g1".to_string(),
            generation: 1,
            fence_token: 11,
        }
    }

    #[tokio::test]
    async fn stale_pending_cancel_converges_only_after_machine_and_durable_absence() {
        let authority = MobHostBindingAuthorityAuthority::recover_from_state(
            MobHostBindingAuthorityState::default(),
        )
        .expect("empty generated authority state is valid");
        let expected = test_member_incarnation();
        certify_stale_pending_absent(
            &authority,
            &LoadOnlyHostBindingPersistence { record: None },
            &expected,
            expected.generation,
            expected.fence_token,
            "input-g1",
            "G1 is stale after G2 cutover",
        )
        .await
        .expect("already-pruned old Pending converges as successful cancellation");

        let retained = MobHostBindingRecord {
            supervisor_peer_id: "controller".to_string(),
            supervisor_signing_key: [7; 32],
            epoch: 1,
            binding_generation: 1,
            accepted_capabilities: None,
            materialized: BTreeMap::new(),
            released: BTreeMap::new(),
            turn_outcome_pending: BTreeMap::from([(
                expected.agent_identity.clone(),
                vec![TurnOutcomePendingRow {
                    input_id: "input-g1".to_string(),
                    generation: expected.generation,
                    fence_token: expected.fence_token,
                    window_start: 41,
                    bounded_result_spec: None,
                }],
            )]),
            turn_outcomes: BTreeMap::new(),
            turn_outcome_acknowledged: BTreeMap::new(),
            tracked_input_cancellations: BTreeMap::new(),
            forked_participant_obligations: BTreeMap::new(),
        };
        let error = certify_stale_pending_absent(
            &authority,
            &LoadOnlyHostBindingPersistence {
                record: Some(retained),
            },
            &expected,
            expected.generation,
            expected.fence_token,
            "input-g1",
            "G1 is stale after G2 cutover",
        )
        .await
        .expect_err("a retained durable G1 Pending must remain a loud ambiguity");
        assert!(error.contains("machine=false, durable=true"), "{error}");
    }

    #[test]
    fn bootstrap_token_slot_verifies_only_the_exact_request_bound_proof() {
        let mut slot = HostBootstrapTokenSlot::mint();
        let first = slot.current().to_string();
        let supervisor_key = [7_u8; 32];
        let mut payload = BridgeHostBindPayload {
            supervisor: BridgePeerSpec {
                name: "mob/supervisor/lead".to_string(),
                peer_id: PeerId::from_ed25519_pubkey(&supervisor_key).to_string(),
                address: "tcp://127.0.0.1:7000".to_string(),
                pubkey: supervisor_key,
            },
            epoch: 3,
            binding_generation: 2,
            protocol_version: BridgeProtocolVersion::V4,
            mob_id: "mob-proof".to_string(),
            expected_host_peer_id: PeerId::from_ed25519_pubkey(&[9_u8; 32]).to_string(),
            expected_address: "tcp://127.0.0.1:9000".to_string(),
            bootstrap_proof:
                meerkat_contracts::wire::supervisor_bridge::BridgeHostBootstrapProof::new(""),
            delegated_bootstrap_proof: None,
            required_capabilities: BridgeHostCapabilityRequirements::default(),
        };
        payload.bootstrap_proof = derive_host_bind_bootstrap_proof(&first, &payload);
        assert!(slot.matches_bind_proof(&payload));

        let mut raw_on_wire = payload.clone();
        raw_on_wire.bootstrap_proof =
            meerkat_contracts::wire::supervisor_bridge::BridgeHostBootstrapProof::new(
                first.clone(),
            );
        assert!(
            !slot.matches_bind_proof(&raw_on_wire),
            "presenting the raw bearer token on the wire must fail closed"
        );

        let mut changed_epoch = payload.clone();
        changed_epoch.epoch += 1;
        assert!(
            !slot.matches_bind_proof(&changed_epoch),
            "a proof cannot authorize a different epoch"
        );

        slot.consume_and_remint();
        assert!(
            !slot.matches_bind_proof(&payload),
            "a consumed token's proof must not authorize another fresh bind"
        );
        assert_ne!(slot.current(), first, "re-mint must rotate the token");
    }

    #[test]
    fn pending_descriptor_refresh_fail_once_then_retries_same_reminted_token() {
        let sink = Arc::new(FailFirstDescriptorSink::new());
        let (descriptor_watch_tx, descriptor_watch_rx) = watch::channel(String::new());
        let descriptor = DescriptorRefresher::new(
            "tcp://127.0.0.1:40142".to_string(),
            meerkat_contracts::WireTrustedPeerIdentity::Ed25519PublicKey {
                public_key: "test-host-public-key".to_string(),
            },
            None,
            descriptor_watch_tx,
            sink.clone(),
        );
        let mut slot = HostBootstrapTokenSlot::mint();
        let consumed = slot.current().to_string();
        slot.consume_and_remint();
        let replacement = slot.current().to_string();
        assert_ne!(replacement, consumed, "fresh bind must rotate the token");

        descriptor
            .publish(slot.current())
            .expect_err("injected first publication failure creates pending refresh work");
        assert!(
            descriptor_watch_rx.borrow().is_empty(),
            "failed sink publication must not expose the replacement token"
        );

        let mut pending = PendingDescriptorRefresh::after_publish_failure();
        assert_eq!(pending.attempt_number(), 1);
        pending
            .retry(&descriptor, &slot)
            .expect("the actor-owned retry publishes after the transient sink failure");

        assert_eq!(sink.attempts(), 2, "one initial attempt plus one retry");
        assert_eq!(
            slot.current(),
            replacement,
            "descriptor retry must not consume or re-mint the replacement token"
        );
        let published: WireHostBindingDescriptor =
            serde_json::from_str(descriptor_watch_rx.borrow().as_str())
                .expect("retry publishes a typed host descriptor");
        assert_eq!(published.bootstrap_token.as_str(), replacement);
    }

    #[tokio::test]
    async fn descriptor_sink_failure_leaves_acceptor_registry_retryable() {
        let identity_secret = [41u8; 32];
        let host = build_host_comms_runtime(
            "descriptor-retry-host",
            meerkat_comms::Keypair::from_secret(identity_secret),
        )
        .expect("host runtime builds");
        let host_keypair = Arc::new(meerkat_comms::Keypair::from_secret(identity_secret));
        let registry = Arc::new(meerkat_comms::HostAcceptorIdentityRegistry::new());
        let persistence: Arc<dyn MobHostBindingPersistence> =
            Arc::new(LoadOnlyHostBindingPersistence { record: None });
        let probe: Arc<dyn ProviderPresenceProbe> = Arc::new(EmptyProviderPresenceProbe);
        let sink = Arc::new(FailFirstDescriptorSink::new());
        let (descriptor_watch_tx, descriptor_watch_rx) = watch::channel(String::new());

        let config = || MobHostActorConfig {
            host_runtime: Arc::clone(&host.runtime),
            host_dsl: Arc::clone(&host.dsl),
            host_inbox_sender: host.inbox_sender.clone(),
            host_keypair: Arc::clone(&host_keypair),
            registry: Arc::clone(&registry),
            persistence: Arc::clone(&persistence),
            probe: Arc::clone(&probe),
            capability_facts: HostCapabilityFacts {
                durable_sessions: false,
                memory_store: false,
                mcp: false,
            },
            advertised_address: "tcp://127.0.0.1:40141".to_string(),
            live_endpoint: None,
            descriptor_watch_tx: descriptor_watch_tx.clone(),
            descriptor_sink: sink.clone(),
            member_host: None,
            forked_participant_sweep_interval: None,
        };

        match spawn_mob_host_actor(config()).await {
            Err(MobHostActorError::Descriptor { detail }) => {
                assert!(detail.contains("injected first descriptor publication failure"));
            }
            Err(other) => panic!("first startup failed through the wrong boundary: {other}"),
            Ok(actor) => {
                actor.shutdown().await;
                panic!("injected descriptor failure unexpectedly started the actor");
            }
        }
        assert!(
            descriptor_watch_rx.borrow().is_empty(),
            "failed sink publication must not advance pairing-watch visibility"
        );

        let mut actor = spawn_mob_host_actor(config())
            .await
            .expect("retry with the same once-owned registry must succeed");
        actor
            .wait_for_initial_revival()
            .await
            .expect("successful startup publishes initial revival readiness");
        assert!(
            !descriptor_watch_rx.borrow().is_empty(),
            "successful retry publishes the descriptor"
        );
        actor.shutdown().await;
    }

    #[tokio::test]
    async fn registry_conflict_fails_before_descriptor_publication() {
        let identity_secret = [42u8; 32];
        let host = build_host_comms_runtime(
            "descriptor-registry-conflict-host",
            meerkat_comms::Keypair::from_secret(identity_secret),
        )
        .expect("host runtime builds");
        let host_keypair = Arc::new(meerkat_comms::Keypair::from_secret(identity_secret));
        let registry = Arc::new(meerkat_comms::HostAcceptorIdentityRegistry::new());
        let foreign_owner: Arc<dyn Any + Send + Sync> = Arc::new(());
        registry
            .install_owner(foreign_owner)
            .expect("seed foreign registry owner");
        let sink = Arc::new(FailFirstDescriptorSink::new());
        let (descriptor_watch_tx, descriptor_watch_rx) = watch::channel(String::new());

        let error = match spawn_mob_host_actor(MobHostActorConfig {
            host_runtime: Arc::clone(&host.runtime),
            host_dsl: Arc::clone(&host.dsl),
            host_inbox_sender: host.inbox_sender.clone(),
            host_keypair,
            registry,
            persistence: Arc::new(LoadOnlyHostBindingPersistence { record: None }),
            probe: Arc::new(EmptyProviderPresenceProbe),
            capability_facts: HostCapabilityFacts {
                durable_sessions: false,
                memory_store: false,
                mcp: false,
            },
            advertised_address: "tcp://127.0.0.1:40142".to_string(),
            live_endpoint: None,
            descriptor_watch_tx,
            descriptor_sink: sink.clone(),
            member_host: None,
            forked_participant_sweep_interval: None,
        })
        .await
        {
            Ok(actor) => {
                actor.shutdown().await;
                panic!("foreign registry owner unexpectedly admitted startup");
            }
            Err(error) => error,
        };
        assert!(matches!(
            error,
            MobHostActorError::Registry(meerkat_comms::HostAcceptorError::OwnerAlreadyInstalled)
        ));
        assert_eq!(sink.attempts(), 0, "registry conflict precedes sink commit");
        assert!(
            descriptor_watch_rx.borrow().is_empty(),
            "registry conflict must not expose pairing descriptor state"
        );
    }

    #[tokio::test]
    async fn mismatched_runtime_and_acceptor_identity_fails_before_publication() {
        let host = build_host_comms_runtime(
            "descriptor-identity-mismatch-host",
            meerkat_comms::Keypair::from_secret([43u8; 32]),
        )
        .expect("host runtime builds");
        let sink = Arc::new(FailFirstDescriptorSink::new());
        let (descriptor_watch_tx, descriptor_watch_rx) = watch::channel(String::new());

        let error = match spawn_mob_host_actor(MobHostActorConfig {
            host_runtime: Arc::clone(&host.runtime),
            host_dsl: Arc::clone(&host.dsl),
            host_inbox_sender: host.inbox_sender.clone(),
            host_keypair: Arc::new(meerkat_comms::Keypair::from_secret([44u8; 32])),
            registry: Arc::new(meerkat_comms::HostAcceptorIdentityRegistry::new()),
            persistence: Arc::new(LoadOnlyHostBindingPersistence { record: None }),
            probe: Arc::new(EmptyProviderPresenceProbe),
            capability_facts: HostCapabilityFacts {
                durable_sessions: false,
                memory_store: false,
                mcp: false,
            },
            advertised_address: "tcp://127.0.0.1:40143".to_string(),
            live_endpoint: None,
            descriptor_watch_tx,
            descriptor_sink: sink.clone(),
            member_host: None,
            forked_participant_sweep_interval: None,
        })
        .await
        {
            Ok(actor) => {
                actor.shutdown().await;
                panic!("mismatched host identities unexpectedly admitted startup");
            }
            Err(error) => error,
        };
        assert!(
            matches!(error, MobHostActorError::Internal { detail } if detail.contains("does not match"))
        );
        assert_eq!(sink.attempts(), 0);
        assert!(descriptor_watch_rx.borrow().is_empty());
    }

    #[test]
    fn bootstrap_token_debug_is_redacted() {
        let slot = HostBootstrapTokenSlot::mint();
        let rendered = format!("{slot:?}");
        assert!(
            !rendered.contains(slot.current()),
            "token material must never appear in Debug output"
        );
    }

    #[test]
    fn reject_kind_maps_onto_wire_causes() {
        assert_eq!(
            bridge_rejection_cause(HostAdmissionRejectKind::AlreadyBound),
            BridgeRejectionCause::AlreadyBound
        );
        assert_eq!(
            bridge_rejection_cause(HostAdmissionRejectKind::StaleSupervisor),
            BridgeRejectionCause::StaleSupervisor
        );
        assert_eq!(
            bridge_rejection_cause(HostAdmissionRejectKind::InvalidBootstrapToken),
            BridgeRejectionCause::InvalidBootstrapToken
        );
        assert_eq!(
            bridge_rejection_cause(HostAdmissionRejectKind::TurnDirectiveUnsupported),
            BridgeRejectionCause::Unsupported
        );
    }

    #[test]
    fn authority_state_fold_recovers_binding_region() {
        let records = vec![(
            "mob-1".to_string(),
            MobHostBindingRecord {
                supervisor_peer_id: "peer-a".to_string(),
                supervisor_signing_key: [7u8; 32],
                epoch: 3,
                binding_generation: 1,
                accepted_capabilities: None,
                materialized: BTreeMap::new(),
                released: BTreeMap::new(),
                turn_outcome_pending: BTreeMap::new(),
                turn_outcomes: BTreeMap::new(),
                turn_outcome_acknowledged: BTreeMap::new(),
                tracked_input_cancellations: BTreeMap::new(),
                forked_participant_obligations: BTreeMap::new(),
            },
        )];
        let state = authority_state_from_records(&records);
        let mob = AuthorityMobId::from("mob-1");
        assert_eq!(
            state.supervisor_peer_ids.get(&mob),
            Some(&AuthorityPeerId::from("peer-a"))
        );
        assert_eq!(state.supervisor_epochs.get(&mob), Some(&3));
        assert_eq!(
            state.binding_phases.get(&mob),
            Some(&HostBindingPhase::Bound)
        );
        assert!(state.materialized_generations.is_empty());
    }

    #[test]
    fn recorded_disposal_maps_total_over_machine_and_wire() {
        for machine in [
            MachineMemberSessionDisposal::Archived,
            MachineMemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
            MachineMemberSessionDisposal::RuntimeReleasedOnlyNoDurableSessions,
        ] {
            let recorded = RecordedDisposal::from_machine(machine);
            assert_eq!(recorded.to_machine(), machine, "machine roundtrip");
            // AlreadyArchived is never a recorded value (DEC-P3H-6): the
            // wire projection of a RECORDED Archived is plain Archived.
            match (machine, recorded.to_wire()) {
                (MachineMemberSessionDisposal::Archived, WireMemberSessionDisposal::Archived)
                | (
                    MachineMemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
                    WireMemberSessionDisposal::RuntimeReleasedOnly {
                        cause: RuntimeReleaseCause::HostOwnedSession,
                    },
                )
                | (
                    MachineMemberSessionDisposal::RuntimeReleasedOnlyNoDurableSessions,
                    WireMemberSessionDisposal::RuntimeReleasedOnly {
                        cause: RuntimeReleaseCause::NoDurableSessions,
                    },
                ) => {}
                (machine, wire) => panic!("unexpected disposal projection {machine:?} -> {wire:?}"),
            }
        }
    }

    #[test]
    fn materialize_memory_reject_maps_to_unavailable() {
        // ADJ-20: no dedicated wire cause; the transient-class Unavailable
        // with the pinned reason string carries the fact.
        let spec_json = serde_json::json!({
            "mob_id": "mob-1",
            "profile_name": "worker",
            "agent_identity": "worker-1",
            "profile": {
                "model": "claude-opus-4-8",
                "provider": "anthropic",
                "tools": { "comms": true, "memory": true },
                "runtime_mode": "turn_driven",
            },
            "definition_extract": {},
            "overlay": {
                "system_prompt": { "prompt": "disable" },
                "runtime_mode": "turn_driven",
            },
        });
        let spec: PortableMemberSpec =
            serde_json::from_value(spec_json).expect("minimal spec decodes");
        let (cause, reason) = materialize_reject_wire_cause(
            MaterializeRejectKind::MemoryStoreUnavailable,
            &spec,
            None,
            None,
        );
        assert_eq!(cause, BridgeRejectionCause::Unavailable);
        assert_eq!(reason, "memory store unavailable on host");
    }

    #[test]
    fn fold_recovers_materialized_and_release_regions() {
        let spec_json = serde_json::json!({
            "mob_id": "mob-1",
            "profile_name": "worker",
            "agent_identity": "worker-1",
            "profile": {
                "model": "claude-opus-4-8",
                "provider": "anthropic",
                "tools": { "comms": true },
                "runtime_mode": "turn_driven",
            },
            "definition_extract": {},
            "overlay": {
                "system_prompt": { "prompt": "disable" },
                "runtime_mode": "turn_driven",
            },
        });
        let spec: PortableMemberSpec =
            serde_json::from_value(spec_json).expect("minimal spec decodes");
        let records = vec![(
            "mob-1".to_string(),
            MobHostBindingRecord {
                supervisor_peer_id: "peer-a".to_string(),
                supervisor_signing_key: [7u8; 32],
                epoch: 3,
                binding_generation: 1,
                accepted_capabilities: None,
                materialized: BTreeMap::from([(
                    "worker-1".to_string(),
                    MaterializedMemberRow {
                        generation: 2,
                        generation_start_seq: 1,
                        fence_token: 9,
                        session_id: "0195e9a0-0000-7000-8000-000000000001".to_string(),
                        spec_digest: "digest-1".to_string(),
                        spec,
                        engine_version_at_build: "0.0.0-test".to_string(),
                        member_pubkey: "ed25519:AA".to_string(),
                        member_peer_id: "peer-member".to_string(),
                        launch_outcome: MaterializeLaunchOutcome::Fresh,
                        resolved_auth_binding: None,
                        supervisor_name: "controller".to_string(),
                        supervisor_address: "tcp://127.0.0.1:1".to_string(),
                        forked_participant_attachment: None,
                    },
                )]),
                released: BTreeMap::from([(
                    "worker-0".to_string(),
                    ReleasedMemberRow {
                        generation: 1,
                        fence_token: 4,
                        disposal: RecordedDisposal::Archived,
                        member_pubkey: "ed25519:BB".to_string(),
                    },
                )]),
                turn_outcome_pending: BTreeMap::new(),
                turn_outcomes: BTreeMap::from([(
                    "worker-1".to_string(),
                    vec![TurnOutcomeRow {
                        input_id: "input-1".to_string(),
                        generation: 2,
                        fence_token: 9,
                        terminal_seq: 41,
                        outcome: WireFlowTurnOutcome::RunCompleted,
                        bounded_result: None,
                    }],
                )]),
                turn_outcome_acknowledged: BTreeMap::new(),
                tracked_input_cancellations: BTreeMap::new(),
                forked_participant_obligations: BTreeMap::new(),
            },
        )];
        let state = authority_state_from_records(&records);
        let live_key = MemberKey::new(
            AuthorityMobId::from("mob-1"),
            AuthorityAgentIdentity::from("worker-1"),
        );
        let released_key = MemberKey::new(
            AuthorityMobId::from("mob-1"),
            AuthorityAgentIdentity::from("worker-0"),
        );
        assert_eq!(
            state.materialized_generations.get(&live_key),
            Some(&AuthorityGeneration(2))
        );
        assert_eq!(
            state.materialized_fences.get(&live_key),
            Some(&AuthorityFenceToken(9))
        );
        assert_eq!(
            state.materialized_spec_digests.get(&live_key),
            Some(&"digest-1".to_string())
        );
        assert_eq!(
            state.release_disposals.get(&released_key),
            Some(&MachineMemberSessionDisposal::Archived)
        );
        // §18 O2: the turn-outcome journal region folds back into the
        // machine's dedup maps (coarse kind + terminal seq).
        let turn_key = TurnKey::new(
            AuthorityMobId::from("mob-1"),
            AuthorityAgentIdentity::from("worker-1"),
            AuthorityGeneration(2),
            AuthorityFenceToken(9),
            AuthorityInputId("input-1".to_string()),
        );
        assert_eq!(state.turn_outcome_terminal_seqs.get(&turn_key), Some(&41));
        assert_eq!(
            state.turn_outcome_kinds.get(&turn_key),
            Some(&FlowTurnOutcomeKind::Completed)
        );
        // The recovered state must pass the generated invariant validation.
        MobHostBindingAuthorityAuthority::recover_from_state(state)
            .expect("recovered state passes invariants");
    }

    fn boot_preparation_spec() -> PortableMemberSpec {
        serde_json::from_value(serde_json::json!({
            "mob_id": "mob-preflight",
            "profile_name": "worker",
            "agent_identity": "worker-1",
            "profile": {
                "model": "claude-opus-4-8",
                "provider": "anthropic",
                "tools": { "comms": true },
                "runtime_mode": "turn_driven"
            },
            "definition_extract": {},
            "overlay": {
                "system_prompt": { "prompt": "disable" },
                "runtime_mode": "turn_driven"
            }
        }))
        .expect("minimal portable boot spec decodes")
    }

    fn boot_preparation_records(spec: PortableMemberSpec) -> Vec<(String, MobHostBindingRecord)> {
        let spec_digest = meerkat_contracts::wire::portable_member_spec_digest(&spec)
            .expect("portable boot spec digest");
        let member_keypair = meerkat_comms::Keypair::from_secret([51u8; 32]);
        let member_pubkey = member_keypair.public_key();
        let supervisor_pubkey = meerkat_comms::PubKey::new([52u8; 32]);
        vec![(
            "mob-preflight".to_string(),
            MobHostBindingRecord {
                supervisor_peer_id: supervisor_pubkey.to_peer_id().to_string(),
                supervisor_signing_key: *supervisor_pubkey.as_bytes(),
                epoch: 1,
                binding_generation: 1,
                accepted_capabilities: None,
                materialized: BTreeMap::from([(
                    "worker-1".to_string(),
                    MaterializedMemberRow {
                        generation: 1,
                        generation_start_seq: 1,
                        fence_token: 9,
                        session_id: meerkat_core::types::SessionId::new().to_string(),
                        spec_digest,
                        spec,
                        engine_version_at_build: "0.0.0-test".to_string(),
                        member_pubkey: member_pubkey.to_pubkey_string(),
                        member_peer_id: member_pubkey.to_peer_id().to_string(),
                        launch_outcome: MaterializeLaunchOutcome::Fresh,
                        resolved_auth_binding: None,
                        supervisor_name: "controller".to_string(),
                        supervisor_address: "tcp://127.0.0.1:7801".to_string(),
                        forked_participant_attachment: None,
                    },
                )]),
                released: BTreeMap::new(),
                turn_outcome_pending: BTreeMap::new(),
                turn_outcomes: BTreeMap::new(),
                turn_outcome_acknowledged: BTreeMap::new(),
                tracked_input_cancellations: BTreeMap::new(),
                forked_participant_obligations: BTreeMap::new(),
            },
        )]
    }

    #[test]
    fn boot_preparation_rejects_redigested_undecompilable_portable_spec() {
        let mut spec_json =
            serde_json::to_value(boot_preparation_spec()).expect("serialize portable spec");
        spec_json["profile"]["output_schema"] = serde_json::json!("{not-json");
        let spec: PortableMemberSpec =
            serde_json::from_value(spec_json).expect("opaque invalid JSON envelope itself decodes");
        let records = boot_preparation_records(spec);
        let authority = recover_binding_authority_from_snapshot(&records, &[])
            .expect("all non-decompile durable facts are valid");

        for member_substrate_configured in [false, true] {
            let error = match prepare_recovered_members(
                &records,
                authority.state(),
                member_substrate_configured,
            ) {
                Ok(_) => panic!("undecompilable durable spec unexpectedly reached public startup"),
                Err(error) => error,
            };
            assert!(
                matches!(
                    error,
                    MobHostActorError::DurableMaterializedRowCorrupt { ref detail, .. }
                        if detail.contains("structurally invalid")
                            && detail.contains("output_schema")
                ),
                "unexpected preparation error: {error}"
            );
        }
    }

    #[test]
    fn boot_preparation_classifies_missing_mcp_env_as_member_unavailable() {
        let missing_key = format!(
            "MEERKAT_TEST_MISSING_BOOT_ENV_{}",
            uuid::Uuid::new_v4().simple()
        );
        assert!(std::env::var_os(&missing_key).is_none());
        let mut spec = boot_preparation_spec();
        spec.profile.tools.mcp_servers.insert(
            "missing-env".to_string(),
            meerkat_contracts::wire::PortableMcpDecl::Stdio {
                command: "missing-env-server".to_string(),
                args: Vec::new(),
                required_env_keys: vec![missing_key.clone()],
                connect_timeout_secs: None,
            },
        );
        let records = boot_preparation_records(spec);
        let authority = recover_binding_authority_from_snapshot(&records, &[])
            .expect("missing host env is not durable corruption");
        let prepared = match prepare_recovered_members(&records, authority.state(), true) {
            Ok(prepared) => prepared,
            Err(error) => panic!("missing member env aborted whole host startup: {error}"),
        };
        assert_eq!(prepared.len(), 1);
        assert!(matches!(
            &prepared[0].decompile,
            PreparedRecoveredDecompile::EnvironmentalFailure(
                PreparedRecoveredEnvironmentalFailure::Decompile(
                    MaterializeDecompileError::McpEnvKeyMissing { key, .. }
                )
            ) if key == &missing_key
        ));
    }

    #[test]
    fn boot_preparation_classifies_missing_top_level_env_as_member_unavailable() {
        let missing_key = format!(
            "MEERKAT_TEST_MISSING_REQUIRED_BOOT_ENV_{}",
            uuid::Uuid::new_v4().simple()
        );
        assert!(std::env::var_os(&missing_key).is_none());
        let mut spec = boot_preparation_spec();
        spec.required_env_keys.push(missing_key.clone());
        let records = boot_preparation_records(spec);
        let authority = recover_binding_authority_from_snapshot(&records, &[])
            .expect("missing host env is not durable corruption");
        let prepared = match prepare_recovered_members(&records, authority.state(), true) {
            Ok(prepared) => prepared,
            Err(error) => panic!("missing required env aborted whole host startup: {error}"),
        };
        assert!(matches!(
            &prepared[0].decompile,
            PreparedRecoveredDecompile::EnvironmentalFailure(
                PreparedRecoveredEnvironmentalFailure::MissingRequiredEnvKey { key }
            ) if key == &missing_key
        ));
    }

    #[test]
    fn missing_mcp_env_cannot_mask_later_structural_corruption() {
        let missing_key = format!(
            "MEERKAT_TEST_MISSING_MASKED_ENV_{}",
            uuid::Uuid::new_v4().simple()
        );
        assert!(std::env::var_os(&missing_key).is_none());
        let mut spec = boot_preparation_spec();
        spec.profile.tools.mcp_servers.insert(
            "masked-timeout".to_string(),
            meerkat_contracts::wire::PortableMcpDecl::Stdio {
                command: "masked-timeout-server".to_string(),
                args: Vec::new(),
                required_env_keys: vec![missing_key],
                connect_timeout_secs: Some(u64::MAX),
            },
        );
        let records = boot_preparation_records(spec);
        let authority = recover_binding_authority_from_snapshot(&records, &[])
            .expect("row digest and authority facts are valid");
        let error = match prepare_recovered_members(&records, authority.state(), true) {
            Ok(_) => panic!("missing env masked a structurally invalid durable timeout"),
            Err(error) => error,
        };
        assert!(
            matches!(
                error,
                MobHostActorError::DurableMaterializedRowCorrupt { ref detail, .. }
                    if detail.contains("supported range")
            ),
            "unexpected masked-corruption error: {error}"
        );
    }
}
