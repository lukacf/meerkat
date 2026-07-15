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
    BridgeHostBindPayload, BridgeHostBindResponse, BridgeHostCapabilityRequirements,
    BridgeHostMemberRecord, BridgeHostRebindPayload, BridgeHostReboundResponse,
    BridgeHostRevokePayload, BridgeHostRevokedResponse, BridgeHostStatusPayload,
    BridgeHostStatusResponse, BridgeMaterializePayload, BridgeMaterializedResponse,
    BridgeMemberReleasedResponse, BridgePeerIdentity, BridgePeerSpec, BridgePeerTrustPayload,
    BridgeProtocolVersion, BridgeRejectionCause, BridgeReleasePayload, BridgeReply,
    BridgeTurnOutcomeAck, BridgeTurnOutcomeRecord, MaterializeLaunchMode, MaterializeLaunchOutcome,
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
    /// The opened realm persistence backend is durable/event-sourced (A7).
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
        let descriptor = WireHostBindingDescriptor {
            kind: WireHostBindingDescriptorKind::Host,
            address: self.address.clone(),
            identity: self.identity.clone(),
            bootstrap_token: BridgeBootstrapToken::new(token),
            live_endpoint: self.live_endpoint.clone(),
        };
        serde_json::to_string_pretty(&descriptor).map_err(|err| MobHostActorError::RecordSerde {
            detail: format!("descriptor serialization failed: {err}"),
        })
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
pub async fn record_materialized_member(
    authority: &mut MobHostBindingAuthorityAuthority,
    persistence: &dyn MobHostBindingPersistence,
    member_key: &MemberKey,
    row: MaterializedMemberRow,
) -> Result<MaterializedIdentityWitness, MobHostActorError> {
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
    let runtime = meerkat_comms::CommsRuntime::inproc_only_with_keypair_and_silent_intents(
        participant_name,
        None,
        keypair,
        Arc::new(std::collections::HashSet::new()),
    )
    .map_err(|err| MobHostActorError::Comms {
        detail: format!("failed to construct host comms runtime '{participant_name}': {err}"),
    })?;
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
    runtime.require_peer_comms_machine_authority();
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
}

/// Running mob host actor (single-owner responder task).
pub struct MobHostActorHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<()>,
    observation_watch_rx: watch::Receiver<HostObservationProjection>,
    observation_pending_tx: mpsc::Sender<HostTurnOutcomePendingRequest>,
    observation_record_tx: mpsc::Sender<HostTurnOutcomeRecordRequest>,
    observation_ack_tx: mpsc::Sender<HostTurnOutcomeAckRequest>,
}

impl MobHostActorHandle {
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
    /// THE ownership-ledger anchor: the generated MobHostBindingAuthority is
    /// the sole owner of every host-side binding/materialize/release/turn
    /// fact; this shell only observes and realizes.
    binding_authority: MobHostBindingAuthorityAuthority,
    persistence: Arc<dyn MobHostBindingPersistence>,
    host_runtime: Arc<meerkat_comms::CommsRuntime>,
    host_comms: Arc<dyn CoreCommsRuntime>,
    host_dsl: Arc<meerkat_runtime::HandleDslAuthority>,
    bootstrap_token: HostBootstrapTokenSlot,
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
    /// Recorded-but-unrevived members (per-row revival failures at boot, or
    /// dead sessions a replay ensure could not recompose). Shell health
    /// observation only — feeds `HostStatus.healthy: false` (ADJ-19), never
    /// a machine decision.
    unrevived: BTreeSet<(String, String)>,
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
    let prepared_recovered_members = prepare_recovered_members(
        &records,
        binding_authority.state(),
        member_substrate_configured,
    )?;

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
    let mut actor = MobHostActor {
        binding_authority,
        persistence,
        host_runtime,
        host_comms,
        host_dsl,
        bootstrap_token,
        capabilities,
        capability_facts,
        descriptor,
        pending_descriptor_refresh: None,
        advertised_address,
        registry,
        registry_owner,
        materializer: member_host.map(HostMemberMaterializer::new),
        unrevived: BTreeSet::new(),
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
    // No `.await` is permitted between the public commit above and this task
    // ownership transfer. Boot revival (A20/§14.6) remains ahead of the
    // responder drain *inside* the owned task, so the first post-restart
    // command sees a coherent host without making caller cancellation a
    // half-start boundary. Revival itself is deliberately not selected
    // against shutdown: it owns async side effects and must reach its normal
    // rollback/commit boundary before the responder observes a closed handle.
    let join = tokio::spawn(async move {
        if let Some(host_id) = revival_host_id.as_deref() {
            actor
                .revive_recovered_members(host_id, prepared_recovered_members)
                .await;
        }
        // First projection publish + journal registrations for the revived
        // residency set (materialize/release re-sync per served command).
        actor.sync_member_observation().await;
        run_host_responder(
            actor,
            shutdown_rx,
            observation_pending_rx,
            observation_record_rx,
            observation_ack_rx,
        )
        .await;
    });
    Ok(MobHostActorHandle {
        shutdown_tx: Some(shutdown_tx),
        join,
        observation_watch_rx,
        observation_pending_tx,
        observation_record_tx,
        observation_ack_tx,
    })
}

const HOST_OBSERVATION_DRAIN_BATCH_LIMIT: usize = 64;

/// Notify-driven responder drain: the non-session analogue of the member
/// drain's `try_handle_supervisor_bridge_command`, adjudicating through
/// `MobHostBindingAuthority` instead of `MeerkatMachine`.
async fn run_host_responder(
    mut actor: MobHostActor,
    mut shutdown_rx: oneshot::Receiver<()>,
    mut observation_pending_rx: mpsc::Receiver<HostTurnOutcomePendingRequest>,
    mut observation_record_rx: mpsc::Receiver<HostTurnOutcomeRecordRequest>,
    mut observation_ack_rx: mpsc::Receiver<HostTurnOutcomeAckRequest>,
) {
    let notify = actor.host_runtime.inbox_notify();
    loop {
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
        let candidates = actor.host_comms.drain_peer_input_candidates().await;
        if candidates.is_empty() {
            let descriptor_refresh_retry_at = actor
                .pending_descriptor_refresh
                .as_ref()
                .map(PendingDescriptorRefresh::retry_at);
            tokio::select! {
                _ = &mut shutdown_rx => break,
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
        // A continuously non-empty inbox never enters the idle `select!`
        // above. Poll shutdown between finite drain batches so sustained peer
        // traffic cannot indefinitely starve actor termination (the same
        // responsiveness boundary used by the member upcall responder).
        match shutdown_rx.try_recv() {
            Ok(()) | Err(oneshot::error::TryRecvError::Closed) => break,
            Err(oneshot::error::TryRecvError::Empty) => {}
        }
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
        let state = self.binding_authority.state();
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
        if let MaterializeLaunchMode::Resume { session_id } = &payload.launch
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
                MaterializeLaunchMode::Resume { session_id } if session_id == old_session_id
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
        };
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
                let failure = match (durable_uncertainty, cleanup_failure) {
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
    /// ownership-discriminated archive (through the extracted disposal arc) → record (row move under witness) →
    /// deregister → reply. A disposal failure records NOTHING and replies
    /// typed — the release stays retryable at the same tuple.
    async fn serve_admitted_release(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: &BridgeReleasePayload,
        member_key: &MemberKey,
        reply_address: &str,
    ) {
        let recorded_session = self
            .binding_authority
            .state()
            .materialized_sessions
            .get(member_key)
            .map(|session| session.0.clone());
        let Some(session_id_str) = recorded_session else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                "release admitted without a recorded member session",
                Some(reply_address),
            )
            .await;
            return;
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
                return;
            }
        };

        let realm_backend_persistent = self
            .materializer
            .as_ref()
            .is_some_and(|materializer| materializer.substrate().realm_backend_persistent);
        let runtime_adapter = match self.materializer.as_ref() {
            Some(materializer) => Arc::clone(&materializer.substrate().runtime_adapter),
            None => return,
        };
        // Acquire the stable placed-residency transaction before teardown and
        // retain it through the durable release commit. All delayed effects
        // either finish against this exact incarnation first or observe the
        // final VacantPlaced state; none can validate G1 then mutate reuse G2.
        let residency_update = runtime_adapter
            .begin_member_residency_update(session_id.clone())
            .await;
        let Some(materializer) = self.materializer.as_mut() else {
            return; // gated by the caller; unreachable by construction
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
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("member runtime release failed: {error}"),
                        Some(reply_address),
                    )
                    .await;
                    return;
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
                    self.send_failure(
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("member session disposal failed: {error}"),
                        Some(reply_address),
                    )
                    .await;
                    return;
                }
            }
        };

        let (witness, member_pubkey) = match record_member_release(
            &mut self.binding_authority,
            self.persistence.as_ref(),
            member_key,
            payload.generation,
            payload.fence_token,
            machine_disposal,
        )
        .await
        {
            Ok(recorded) => recorded,
            Err(error) => {
                if error.is_durable_uncertainty() {
                    self.durable_uncertainty_fail_stop = Some(error.to_string());
                }
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("release record persistence failed: {error}"),
                    Some(reply_address),
                )
                .await;
                return;
            }
        };
        let residency_publication = match residency_update.vacate() {
            Ok(publication) => publication,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("release residency vacancy publication failed: {error}"),
                    Some(reply_address),
                )
                .await;
                return;
            }
        };
        self.registered_member_incarnations.remove(&session_id);
        self.sync_member_observation().await;
        drop(residency_publication);
        // Deregistration failure is logged + typed-Internal; the row is
        // already released — retry hits ReleaseReplay, which re-attempts
        // removal idempotently.
        if let Err(error) = self.deregister_member_identity(&member_pubkey, &witness) {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Internal,
                format!("member identity deregistration failed: {error}"),
                Some(reply_address),
            )
            .await;
            return;
        }
        self.unrevived
            .remove(&(payload.mob_id.clone(), payload.agent_identity.clone()));

        let reply = BridgeReply::MemberReleased(BridgeMemberReleasedResponse {
            disposal: wire_disposal,
        });
        self.send_reply(
            candidate,
            HostBridgeReply::completed(reply),
            Some(reply_address),
        )
        .await;
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
        for (identity, generation, fence_token, session_id, spec_digest) in rows {
            let healthy = if self
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
            token_valid: self.bootstrap_token.matches_bind_proof(&payload),
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
            members.push((identity.clone(), session_id, pubkey));
        }

        let mut residency_updates = Vec::with_capacity(members.len());
        for (identity, session_id, pubkey) in members {
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
            self.registry
                .remove_identity(&self.registry_owner, &pubkey)?;
            self.unrevived
                .remove(&(mob_id.to_string(), identity.clone()));
            residency_updates.push((session_id, update));
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
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

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
                }],
            )]),
            turn_outcomes: BTreeMap::new(),
            turn_outcome_acknowledged: BTreeMap::new(),
            tracked_input_cancellations: BTreeMap::new(),
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

        let actor = spawn_mob_host_actor(config())
            .await
            .expect("retry with the same once-owned registry must succeed");
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
                    }],
                )]),
                turn_outcome_acknowledged: BTreeMap::new(),
                tracked_input_cancellations: BTreeMap::new(),
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
                    },
                )]),
                released: BTreeMap::new(),
                turn_outcome_pending: BTreeMap::new(),
                turn_outcomes: BTreeMap::new(),
                turn_outcome_acknowledged: BTreeMap::new(),
                tracked_input_cancellations: BTreeMap::new(),
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
