//! Canonical lifecycle authority for ONE source-owned forked-participant
//! capability record (issue #159).
//!
//! Scope discipline: this machine owns the legality of a single capability
//! record — reservation identity, durable fork activation identity, bounded
//! attachment admission, revocation, expiry, and cleanup debt. It is
//! deliberately record-scoped rather than registry-scoped: the store persists
//! one machine state per capability record, so there are no state fields keyed
//! by capability identity and no global index living inside the machine. The
//! one collection it does own — `granted_attachment_ids` — is scoped to this
//! record and bounded by its own reuse budget.
//!
//! Two facts the machine never derives for itself:
//!
//! - **Time.** The machine never reads a clock. Expiry arrives as an explicit
//!   `expired` observation on `Attach` / `ObserveExpiry`; the shell samples the
//!   wall clock once per decision and passes the observation in.
//! - **Authentication.** The machine never validates a credential. Attach and
//!   revoke carry an `authentication_valid` observation; the machine owns only
//!   what an invalid observation means (typed effect, no state change).
//!
//! Totality discipline: every (phase, command class) pair has an explicit
//! transition that names its own `to` phase. Rejection, denial, and replay arms
//! are written per phase and return to the exact phase they matched, so no
//! typed refusal can mutate lifecycle state as a side effect. There is no
//! catch-all wildcard arm.
//!
//! Representation: this machine has ONE canonical serde representation —
//! snake_case for the phase enum and every payload enum, accepted and emitted
//! exactly once, with no compatibility aliases. (The generated kernel mirror in
//! `meerkat-machine-kernels` carries the codegen's own schema string vocabulary
//! for the same named types; that is generated-artifact convention, not an
//! alias of this module's representation.)

use super::OptionValueExt;
use meerkat_machine_dsl::machine;

/// Cleanup debt state for the durable fork behind the capability.
///
/// `Deferred` is the honest "debt exists but is not actionable yet" state: a
/// revoked or expired capability that still holds an active attachment must
/// wait for the exact release before its cleanup becomes actionable.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ForkedParticipantCleanupState {
    #[default]
    NotRequired,
    Deferred,
    Pending,
    Complete,
}

/// Why a reservation command was refused.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ForkedParticipantReservationRejection {
    /// Empty request fingerprint or non-positive max uses.
    #[default]
    MalformedRequest,
    /// A different request tried to take an identity already bound to one.
    FingerprintConflict,
    /// The record already carries a durable fork activation or a terminal fact.
    AlreadyProvisioned,
}

/// Why a fork activation (or activation-failure) record was refused.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ForkedParticipantActivationRejection {
    /// No reservation exists for this record.
    #[default]
    NotReserved,
    /// The record is reserved for a different request fingerprint.
    FingerprintMismatch,
    /// Empty fork activation identity.
    MalformedActivation,
    /// A different durable activation contradicts the recorded one.
    ActivationConflict,
    /// The capability already reached a terminal fact.
    CapabilityTerminal,
}

/// Why an attach command was denied.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ForkedParticipantAttachDenial {
    /// The caller's authentication observation was invalid.
    #[default]
    AuthenticationInvalid,
    /// No durable fork activation is recorded yet.
    NotActive,
    /// Empty attachment identity.
    MalformedAttachment,
    /// A different attachment already holds the capability.
    Busy,
    /// This attachment identity was already granted by this record; it may
    /// never consume a second use.
    AttachmentAlreadyReleased,
    /// The capability is expired (or expiry is pending release).
    Expired,
    /// The capability is revoked (or revocation is pending release).
    Revoked,
    /// The bounded reuse budget is spent.
    Exhausted,
}

/// Why a release command was refused.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ForkedParticipantReleaseRejection {
    /// This record never granted this attachment identity.
    #[default]
    NoActiveAttachment,
    /// A different attachment currently holds the capability.
    AttachmentMismatch,
}

/// Why a revoke command was denied.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ForkedParticipantRevocationDenial {
    /// The caller's authentication observation was invalid.
    #[default]
    AuthenticationInvalid,
    /// Nothing is reserved or activated for this record.
    NotProvisioned,
    /// The capability already reached a different terminal fact.
    AlreadyTerminal,
}

/// Why an expiry observation changed nothing.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ForkedParticipantExpiryIgnore {
    /// The shell observed that the capability has not expired.
    #[default]
    NotExpired,
    /// Nothing is reserved or activated for this record.
    NotProvisioned,
    /// Expiry (or the dominating revocation) is already recorded.
    AlreadyRecorded,
    /// The capability already reached a terminal fact.
    Terminal,
}

/// Why a cleanup-completion record was refused.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ForkedParticipantCleanupRejection {
    /// The capability is still live.
    #[default]
    NotTerminal,
    /// The capability still holds an active attachment.
    AttachmentOutstanding,
    /// The record never accrued cleanup debt.
    NoCleanupDebt,
}

machine! {
    machine ForkedParticipantLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::forked_participant_lifecycle",

        state {
            lifecycle_phase: ForkedParticipantLifecycleState,
            // Exact request identity the reservation is bound to. Empty only in
            // the `Empty` phase.
            request_fingerprint: String,
            // Configured reuse budget. Positive in every non-`Empty` phase.
            max_uses: u64,
            // Attachments granted so far. Never exceeds `max_uses`.
            use_count: u64,
            // Exact durable fork activation identity. Empty until activation.
            fork_activation_id: String,
            // The one attachment currently holding the capability, if any.
            active_attachment_id: Option<String>,
            // Every attachment identity this record has ever granted. This is
            // the exact dedup structure behind attach idempotency and duplicate
            // release convergence: an identity in this set can never consume a
            // second use, no matter how many other attachments intervened. It
            // is record-scoped and bounded by `max_uses` (its cardinality is
            // pinned to `use_count`, which is bounded by the budget).
            granted_attachment_ids: Set<String>,
            cleanup_state: Enum<ForkedParticipantCleanupState>,
        }

        init(Empty) {
            request_fingerprint = "",
            max_uses = 0,
            use_count = 0,
            fork_activation_id = "",
            active_attachment_id = None,
            granted_attachment_ids = EmptySet,
            cleanup_state = ForkedParticipantCleanupState::NotRequired,
        }

        terminal [Revoked, Expired, Exhausted]

        phase ForkedParticipantLifecycleState {
            // No reservation yet.
            Empty,
            // Request identity and reuse budget bound; no durable fork yet.
            Reserved,
            // Durable fork creation failed or aborted. The SAME request may
            // retry; a different request may not steal the identity.
            ActivationFailed,
            // Durable fork recorded, capability detached and usable.
            Active,
            // Exactly one attachment holds the capability.
            Attached,
            // Revoked while attached: no new work, cleanup deferred until the
            // exact release arrives.
            RevocationPendingAttached,
            // Expired while attached: same shape as revocation-pending.
            ExpiryPendingAttached,
            Revoked,
            Expired,
            Exhausted,
        }

        input ForkedParticipantLifecycleInput {
            // Bind the record to one request identity and one positive reuse
            // budget.
            Reserve { request_fingerprint: String, max_uses: u64 },
            // Record the exact durable fork the source runtime created.
            RecordForkActivation { request_fingerprint: String, fork_activation_id: String },
            // Record that durable fork creation failed or aborted.
            RecordForkActivationFailure { request_fingerprint: String },
            // Attach one participant. `authentication_valid` and `expired` are
            // shell observations; the machine reads no clock and validates no
            // credential.
            Attach { attachment_id: String, authentication_valid: bool, expired: bool },
            // Release the exact active attachment.
            Release { attachment_id: String },
            // Revoke the capability. `authentication_valid` is a shell
            // observation.
            Revoke { authentication_valid: bool },
            // Feed one expiry observation. The machine owns what it means.
            ObserveExpiry { expired: bool },
            // Record that the durable fork behind a terminal capability was
            // cleaned up.
            CompleteCleanup {},
        }

        effect ForkedParticipantLifecycleEffect {
            CapabilityReserved { request_fingerprint: String, max_uses: u64 },
            ReservationReplayed { request_fingerprint: String },
            ReservationRejected { reason: Enum<ForkedParticipantReservationRejection> },
            ForkActivated { fork_activation_id: String },
            ForkActivationReplayed { fork_activation_id: String },
            ForkActivationFailed { request_fingerprint: String },
            ForkActivationFailureReplayed { request_fingerprint: String },
            ActivationRejected { reason: Enum<ForkedParticipantActivationRejection> },
            AttachmentGranted { attachment_id: String, use_index: u64, remaining_uses: u64 },
            AttachmentGrantReplayed { attachment_id: String, use_index: u64 },
            AttachDenied { attachment_id: String, reason: Enum<ForkedParticipantAttachDenial> },
            AttachmentReleased { attachment_id: String, use_count: u64 },
            ReleaseReplayed { attachment_id: String },
            ReleaseRejected { attachment_id: String, reason: Enum<ForkedParticipantReleaseRejection> },
            CapabilityExhausted { use_count: u64 },
            CapabilityExpired { cleanup_pending: bool },
            ExpiryPendingRecorded,
            ExpiryObservationIgnored { reason: Enum<ForkedParticipantExpiryIgnore> },
            CapabilityRevoked { cleanup_pending: bool },
            RevocationPendingRecorded,
            RevocationConverged,
            RevocationDenied { reason: Enum<ForkedParticipantRevocationDenial> },
            CleanupCompleted,
            CleanupCompletionReplayed,
            CleanupCompletionRejected { reason: Enum<ForkedParticipantCleanupRejection> },
        }

        // A reservation always binds a positive reuse budget; only the empty
        // record carries a zero budget.
        invariant reserved_capability_has_positive_max_uses {
            self.lifecycle_phase == Phase::Empty || self.max_uses > 0
        }

        // Bounded reuse: granted attachments never exceed the configured budget.
        invariant use_count_within_max_uses {
            self.use_count <= self.max_uses
        }

        // The dedup set IS the use ledger: one granted identity, one use. This
        // also bounds the set by `max_uses` through the invariant above.
        invariant granted_attachments_match_use_count {
            self.granted_attachment_ids.len() == self.use_count
        }

        // The active holder is always one of the identities this record granted.
        invariant active_holder_is_a_granted_attachment {
            self.active_attachment_id == None
                || self.granted_attachment_ids.contains(self.active_attachment_id.get("value"))
        }

        // An attachment exists only in the three attached phases...
        invariant attachment_only_while_attached {
            self.active_attachment_id == None
                || self.lifecycle_phase == Phase::Attached
                || self.lifecycle_phase == Phase::RevocationPendingAttached
                || self.lifecycle_phase == Phase::ExpiryPendingAttached
        }

        // ...and those phases always hold exactly one (the state field is a
        // single `Option`, so "at most one" is structural).
        invariant attached_phase_holds_one_attachment {
            (
                self.lifecycle_phase != Phase::Attached
                && self.lifecycle_phase != Phase::RevocationPendingAttached
                && self.lifecycle_phase != Phase::ExpiryPendingAttached
            )
            || self.active_attachment_id != None
        }

        // Terminal capabilities are detached: no terminal state can hand back
        // an active attachment.
        invariant terminal_capability_is_detached {
            (
                self.lifecycle_phase != Phase::Revoked
                && self.lifecycle_phase != Phase::Expired
                && self.lifecycle_phase != Phase::Exhausted
            )
            || self.active_attachment_id == None
        }

        // Cleanup can never be complete while an attachment is outstanding, and
        // only a terminal record can be complete at all.
        invariant cleanup_complete_requires_detached_terminal {
            self.cleanup_state != ForkedParticipantCleanupState::Complete
                || (
                    (
                        self.lifecycle_phase == Phase::Revoked
                        || self.lifecycle_phase == Phase::Expired
                        || self.lifecycle_phase == Phase::Exhausted
                    )
                    && self.active_attachment_id == None
                )
        }

        // Deferred debt exists only while an attachment blocks cleanup.
        invariant deferred_cleanup_requires_attachment {
            self.cleanup_state != ForkedParticipantCleanupState::Deferred
                || self.lifecycle_phase == Phase::RevocationPendingAttached
                || self.lifecycle_phase == Phase::ExpiryPendingAttached
        }

        // An empty record carries no capability facts at all.
        invariant empty_record_has_no_capability_facts {
            self.lifecycle_phase != Phase::Empty
                || (
                    self.request_fingerprint == ""
                    && self.max_uses == 0
                    && self.use_count == 0
                    && self.fork_activation_id == ""
                    && self.active_attachment_id == None
                    && self.granted_attachment_ids.len() == 0
                    && self.cleanup_state == ForkedParticipantCleanupState::NotRequired
                )
        }

        // Nothing can be granted before a durable fork exists, so every
        // pre-activation phase has an empty use ledger.
        invariant pre_activation_record_has_no_grants {
            (
                self.lifecycle_phase != Phase::Empty
                && self.lifecycle_phase != Phase::Reserved
                && self.lifecycle_phase != Phase::ActivationFailed
            )
            || (
                self.use_count == 0
                && self.granted_attachment_ids.len() == 0
                && self.active_attachment_id == None
            )
        }

        // Every usable or use-consuming phase is backed by a recorded durable
        // fork activation.
        invariant usable_capability_has_fork_activation {
            (
                self.lifecycle_phase != Phase::Active
                && self.lifecycle_phase != Phase::Attached
                && self.lifecycle_phase != Phase::RevocationPendingAttached
                && self.lifecycle_phase != Phase::ExpiryPendingAttached
                && self.lifecycle_phase != Phase::Exhausted
            )
            || self.fork_activation_id != ""
        }

        // Verdict effects: the calling surface mirrors them as its typed result.
        disposition CapabilityReserved => local seam SurfaceResultAlignment,
        disposition ReservationReplayed => local seam SurfaceResultAlignment,
        disposition ReservationRejected => local seam SurfaceResultAlignment,
        disposition ForkActivated => local seam SurfaceResultAlignment,
        disposition ForkActivationReplayed => local seam SurfaceResultAlignment,
        disposition ForkActivationFailed => local seam SurfaceResultAlignment,
        disposition ForkActivationFailureReplayed => local seam SurfaceResultAlignment,
        disposition ActivationRejected => local seam SurfaceResultAlignment,
        disposition AttachmentGranted => local seam SurfaceResultAlignment,
        disposition AttachmentGrantReplayed => local seam SurfaceResultAlignment,
        disposition AttachDenied => local seam SurfaceResultAlignment,
        disposition AttachmentReleased => local seam SurfaceResultAlignment,
        disposition ReleaseReplayed => local seam SurfaceResultAlignment,
        disposition ReleaseRejected => local seam SurfaceResultAlignment,
        disposition ExpiryObservationIgnored => local seam SurfaceResultAlignment,
        disposition RevocationDenied => local seam SurfaceResultAlignment,
        disposition CleanupCompleted => local seam SurfaceResultAlignment,
        disposition CleanupCompletionReplayed => local seam SurfaceResultAlignment,
        disposition CleanupCompletionRejected => local seam SurfaceResultAlignment,
        // Terminal/pending capability facts. They record machine-owned truth
        // (including cleanup debt) rather than commanding an owner; the
        // cross-machine teardown route is a later composition, not a claim this
        // machine makes today.
        disposition CapabilityExhausted => local seam NoOwnerRealization,
        disposition CapabilityExpired => local seam NoOwnerRealization,
        disposition CapabilityRevoked => local seam NoOwnerRealization,
        disposition ExpiryPendingRecorded => local seam NoOwnerRealization,
        disposition RevocationPendingRecorded => local seam NoOwnerRealization,
        disposition RevocationConverged => local seam NoOwnerRealization,

        // ------------------------------------------------------------------
        // Reserve
        // Empty -> Reserved binds one request fingerprint and one positive reuse budget.
        // Exact replay converges; a conflicting request is a typed reject that never
        // rebinds the identity; every post-activation phase refuses without mutating.
        // ------------------------------------------------------------------

        transition ReserveEmpty {
            on input Reserve { request_fingerprint, max_uses }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            guard "well_formed_request" { request_fingerprint != "" && max_uses > 0 }
            update {
                self.request_fingerprint = request_fingerprint;
                self.max_uses = max_uses;
            }
            to Reserved
            emit CapabilityReserved { request_fingerprint: self.request_fingerprint, max_uses: self.max_uses }
        }

        transition ReserveEmptyMalformed {
            on input Reserve { request_fingerprint, max_uses }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            guard "malformed_request" { request_fingerprint == "" || max_uses == 0 }
            update {}
            to Empty
            emit ReservationRejected { reason: ForkedParticipantReservationRejection::MalformedRequest }
        }

        transition ReserveReservedReplay {
            on input Reserve { request_fingerprint, max_uses }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "exact_request_replay" { request_fingerprint == self.request_fingerprint && max_uses == self.max_uses }
            update {}
            to Reserved
            emit ReservationReplayed { request_fingerprint: self.request_fingerprint }
        }

        transition ReserveReservedConflict {
            on input Reserve { request_fingerprint, max_uses }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "conflicting_request" { request_fingerprint != self.request_fingerprint || max_uses != self.max_uses }
            update {}
            to Reserved
            emit ReservationRejected { reason: ForkedParticipantReservationRejection::FingerprintConflict }
        }

        // The create-failed state is retryable by the SAME request only.
        transition ReserveActivationFailedRetry {
            on input Reserve { request_fingerprint, max_uses }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "exact_request_retry" { request_fingerprint == self.request_fingerprint && max_uses == self.max_uses }
            update {}
            to Reserved
            emit CapabilityReserved { request_fingerprint: self.request_fingerprint, max_uses: self.max_uses }
        }

        transition ReserveActivationFailedConflict {
            on input Reserve { request_fingerprint, max_uses }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "conflicting_request" { request_fingerprint != self.request_fingerprint || max_uses != self.max_uses }
            update {}
            to ActivationFailed
            emit ReservationRejected { reason: ForkedParticipantReservationRejection::FingerprintConflict }
        }

        transition ReserveAlreadyProvisionedActive {
            on input Reserve { request_fingerprint, max_uses }
            guard "active" { self.lifecycle_phase == Phase::Active }
            update {}
            to Active
            emit ReservationRejected { reason: ForkedParticipantReservationRejection::AlreadyProvisioned }
        }

        transition ReserveAlreadyProvisionedAttached {
            on input Reserve { request_fingerprint, max_uses }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
            emit ReservationRejected { reason: ForkedParticipantReservationRejection::AlreadyProvisioned }
        }

        transition ReserveAlreadyProvisionedRevocationPendingAttached {
            on input Reserve { request_fingerprint, max_uses }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            update {}
            to RevocationPendingAttached
            emit ReservationRejected { reason: ForkedParticipantReservationRejection::AlreadyProvisioned }
        }

        transition ReserveAlreadyProvisionedExpiryPendingAttached {
            on input Reserve { request_fingerprint, max_uses }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            update {}
            to ExpiryPendingAttached
            emit ReservationRejected { reason: ForkedParticipantReservationRejection::AlreadyProvisioned }
        }

        transition ReserveAlreadyProvisionedRevoked {
            on input Reserve { request_fingerprint, max_uses }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            update {}
            to Revoked
            emit ReservationRejected { reason: ForkedParticipantReservationRejection::AlreadyProvisioned }
        }

        transition ReserveAlreadyProvisionedExpired {
            on input Reserve { request_fingerprint, max_uses }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            update {}
            to Expired
            emit ReservationRejected { reason: ForkedParticipantReservationRejection::AlreadyProvisioned }
        }

        transition ReserveAlreadyProvisionedExhausted {
            on input Reserve { request_fingerprint, max_uses }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            update {}
            to Exhausted
            emit ReservationRejected { reason: ForkedParticipantReservationRejection::AlreadyProvisioned }
        }

        // ------------------------------------------------------------------
        // RecordForkActivation
        // Reserved -> Active on the exact durable fork. Exact replay converges in every
        // post-activation phase; anything else is a typed reject that preserves the phase.
        // ------------------------------------------------------------------

        transition ActivateEmpty {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::NotReserved }
        }

        transition ActivateReserved {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "exact_request" { request_fingerprint == self.request_fingerprint }
            guard "well_formed_activation" { fork_activation_id != "" }
            update {
                self.fork_activation_id = fork_activation_id;
            }
            to Active
            emit ForkActivated { fork_activation_id: self.fork_activation_id }
        }

        transition ActivateReservedMismatch {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "request_mismatch" { request_fingerprint != self.request_fingerprint }
            update {}
            to Reserved
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::FingerprintMismatch }
        }

        transition ActivateReservedMalformed {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "exact_request" { request_fingerprint == self.request_fingerprint }
            guard "malformed_activation" { fork_activation_id == "" }
            update {}
            to Reserved
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::MalformedActivation }
        }

        // A late success record for the same request resolves the ambiguous
        // create-failed state instead of leaking the durable fork.
        transition ActivateActivationFailedRecovery {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "exact_request" { request_fingerprint == self.request_fingerprint }
            guard "well_formed_activation" { fork_activation_id != "" }
            update {
                self.fork_activation_id = fork_activation_id;
            }
            to Active
            emit ForkActivated { fork_activation_id: self.fork_activation_id }
        }

        transition ActivateActivationFailedMismatch {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "request_mismatch" { request_fingerprint != self.request_fingerprint }
            update {}
            to ActivationFailed
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::FingerprintMismatch }
        }

        transition ActivateActivationFailedMalformed {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "exact_request" { request_fingerprint == self.request_fingerprint }
            guard "malformed_activation" { fork_activation_id == "" }
            update {}
            to ActivationFailed
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::MalformedActivation }
        }

        transition ActivateActiveReplay {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "exact_activation_replay" {
                request_fingerprint == self.request_fingerprint
                    && fork_activation_id == self.fork_activation_id
            }
            update {}
            to Active
            emit ForkActivationReplayed { fork_activation_id: self.fork_activation_id }
        }

        transition ActivateActiveConflict {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "activation_conflict" {
                request_fingerprint != self.request_fingerprint
                    || fork_activation_id != self.fork_activation_id
            }
            update {}
            to Active
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::ActivationConflict }
        }

        transition ActivateReplayAfterActivationAttached {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "exact_activation_replay" {
                request_fingerprint == self.request_fingerprint
                    && fork_activation_id == self.fork_activation_id
                    && self.fork_activation_id != ""
            }
            update {}
            to Attached
            emit ForkActivationReplayed { fork_activation_id: self.fork_activation_id }
        }

        transition ActivateReplayAfterActivationRevocationPendingAttached {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            guard "exact_activation_replay" {
                request_fingerprint == self.request_fingerprint
                    && fork_activation_id == self.fork_activation_id
                    && self.fork_activation_id != ""
            }
            update {}
            to RevocationPendingAttached
            emit ForkActivationReplayed { fork_activation_id: self.fork_activation_id }
        }

        transition ActivateReplayAfterActivationExpiryPendingAttached {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            guard "exact_activation_replay" {
                request_fingerprint == self.request_fingerprint
                    && fork_activation_id == self.fork_activation_id
                    && self.fork_activation_id != ""
            }
            update {}
            to ExpiryPendingAttached
            emit ForkActivationReplayed { fork_activation_id: self.fork_activation_id }
        }

        transition ActivateReplayAfterActivationRevoked {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "exact_activation_replay" {
                request_fingerprint == self.request_fingerprint
                    && fork_activation_id == self.fork_activation_id
                    && self.fork_activation_id != ""
            }
            update {}
            to Revoked
            emit ForkActivationReplayed { fork_activation_id: self.fork_activation_id }
        }

        transition ActivateReplayAfterActivationExpired {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "exact_activation_replay" {
                request_fingerprint == self.request_fingerprint
                    && fork_activation_id == self.fork_activation_id
                    && self.fork_activation_id != ""
            }
            update {}
            to Expired
            emit ForkActivationReplayed { fork_activation_id: self.fork_activation_id }
        }

        transition ActivateReplayAfterActivationExhausted {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "exact_activation_replay" {
                request_fingerprint == self.request_fingerprint
                    && fork_activation_id == self.fork_activation_id
                    && self.fork_activation_id != ""
            }
            update {}
            to Exhausted
            emit ForkActivationReplayed { fork_activation_id: self.fork_activation_id }
        }

        transition ActivateAttachedConflictAttached {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "activation_conflict" {
                request_fingerprint != self.request_fingerprint
                    || fork_activation_id != self.fork_activation_id
                    || self.fork_activation_id == ""
            }
            update {}
            to Attached
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::ActivationConflict }
        }

        transition ActivateAttachedConflictRevocationPendingAttached {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            guard "activation_conflict" {
                request_fingerprint != self.request_fingerprint
                    || fork_activation_id != self.fork_activation_id
                    || self.fork_activation_id == ""
            }
            update {}
            to RevocationPendingAttached
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::ActivationConflict }
        }

        transition ActivateAttachedConflictExpiryPendingAttached {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            guard "activation_conflict" {
                request_fingerprint != self.request_fingerprint
                    || fork_activation_id != self.fork_activation_id
                    || self.fork_activation_id == ""
            }
            update {}
            to ExpiryPendingAttached
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::ActivationConflict }
        }

        transition ActivateTerminalConflictRevoked {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "activation_conflict" {
                request_fingerprint != self.request_fingerprint
                    || fork_activation_id != self.fork_activation_id
                    || self.fork_activation_id == ""
            }
            update {}
            to Revoked
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::CapabilityTerminal }
        }

        transition ActivateTerminalConflictExpired {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "activation_conflict" {
                request_fingerprint != self.request_fingerprint
                    || fork_activation_id != self.fork_activation_id
                    || self.fork_activation_id == ""
            }
            update {}
            to Expired
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::CapabilityTerminal }
        }

        transition ActivateTerminalConflictExhausted {
            on input RecordForkActivation { request_fingerprint, fork_activation_id }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "activation_conflict" {
                request_fingerprint != self.request_fingerprint
                    || fork_activation_id != self.fork_activation_id
                    || self.fork_activation_id == ""
            }
            update {}
            to Exhausted
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::CapabilityTerminal }
        }

        // ------------------------------------------------------------------
        // RecordForkActivationFailure
        // The typed create-failed/abort record. Only the reserving request may record it;
        // replay converges and every other phase refuses without mutating.
        // ------------------------------------------------------------------

        transition FailActivationReserved {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "exact_request" { request_fingerprint == self.request_fingerprint }
            update {}
            to ActivationFailed
            emit ForkActivationFailed { request_fingerprint: self.request_fingerprint }
        }

        transition FailActivationReservedMismatch {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "request_mismatch" { request_fingerprint != self.request_fingerprint }
            update {}
            to Reserved
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::FingerprintMismatch }
        }

        transition FailActivationReplay {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "exact_request" { request_fingerprint == self.request_fingerprint }
            update {}
            to ActivationFailed
            emit ForkActivationFailureReplayed { request_fingerprint: self.request_fingerprint }
        }

        transition FailActivationFailedMismatch {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "request_mismatch" { request_fingerprint != self.request_fingerprint }
            update {}
            to ActivationFailed
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::FingerprintMismatch }
        }

        transition FailActivationAfterActivationActive {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "active" { self.lifecycle_phase == Phase::Active }
            update {}
            to Active
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::ActivationConflict }
        }

        transition FailActivationAfterActivationAttached {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::ActivationConflict }
        }

        transition FailActivationAfterActivationRevocationPendingAttached {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            update {}
            to RevocationPendingAttached
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::ActivationConflict }
        }

        transition FailActivationAfterActivationExpiryPendingAttached {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            update {}
            to ExpiryPendingAttached
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::ActivationConflict }
        }

        transition FailActivationAfterActivationExhausted {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            update {}
            to Exhausted
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::ActivationConflict }
        }

        transition FailActivationNotReservedEmpty {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::NotReserved }
        }

        transition FailActivationNotReservedRevoked {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            update {}
            to Revoked
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::NotReserved }
        }

        transition FailActivationNotReservedExpired {
            on input RecordForkActivationFailure { request_fingerprint }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            update {}
            to Expired
            emit ActivationRejected { reason: ForkedParticipantActivationRejection::NotReserved }
        }

        // ------------------------------------------------------------------
        // Attach
        // Bounded, single-holder admission over shell-observed authentication and expiry.
        // An invalid authentication observation never changes state in any phase, and an
        // attachment identity that was already granted can never consume a second use.
        // ------------------------------------------------------------------

        transition AttachAuthenticationInvalidEmpty {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Empty
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AuthenticationInvalid
            }
        }

        transition AttachAuthenticationInvalidReserved {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Reserved
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AuthenticationInvalid
            }
        }

        transition AttachAuthenticationInvalidActivationFailed {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to ActivationFailed
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AuthenticationInvalid
            }
        }

        transition AttachAuthenticationInvalidActive {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Active
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AuthenticationInvalid
            }
        }

        transition AttachAuthenticationInvalidAttached {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Attached
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AuthenticationInvalid
            }
        }

        transition AttachAuthenticationInvalidRevocationPendingAttached {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to RevocationPendingAttached
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AuthenticationInvalid
            }
        }

        transition AttachAuthenticationInvalidExpiryPendingAttached {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to ExpiryPendingAttached
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AuthenticationInvalid
            }
        }

        transition AttachAuthenticationInvalidRevoked {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Revoked
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AuthenticationInvalid
            }
        }

        transition AttachAuthenticationInvalidExpired {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Expired
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AuthenticationInvalid
            }
        }

        transition AttachAuthenticationInvalidExhausted {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Exhausted
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AuthenticationInvalid
            }
        }

        transition AttachNotActiveEmpty {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to Empty
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::NotActive
            }
        }

        transition AttachNotActiveReserved {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to Reserved
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::NotActive
            }
        }

        transition AttachNotActiveActivationFailed {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to ActivationFailed
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::NotActive
            }
        }

        transition AttachActiveMalformed {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "authentication_valid" { authentication_valid == true }
            guard "malformed_attachment" { attachment_id == "" }
            update {}
            to Active
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::MalformedAttachment
            }
        }

        // Expired-before-attach terminalizes and accrues cleanup debt; the attach
        // itself is denied and consumes no use.
        transition AttachActiveExpired {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "authentication_valid" { authentication_valid == true }
            guard "well_formed_attachment" { attachment_id != "" }
            guard "expiry_observed" { expired == true }
            update {
                self.cleanup_state = ForkedParticipantCleanupState::Pending;
            }
            to Expired
            emit CapabilityExpired { cleanup_pending: true }
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::Expired
            }
        }

        // Exact dedup over the whole capability lifetime: ANY attachment identity
        // this record already granted is refused, not merely the most recent one.
        transition AttachActiveAlreadyReleased {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "authentication_valid" { authentication_valid == true }
            guard "well_formed_attachment" { attachment_id != "" }
            guard "not_expired" { expired == false }
            guard "already_granted_attachment" { self.granted_attachment_ids.contains(attachment_id) }
            update {}
            to Active
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AttachmentAlreadyReleased
            }
        }

        transition AttachActiveGrant {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "authentication_valid" { authentication_valid == true }
            guard "well_formed_attachment" { attachment_id != "" }
            guard "not_expired" { expired == false }
            guard "fresh_attachment" { self.granted_attachment_ids.contains(attachment_id) == false }
            guard "reuse_budget_available" { self.use_count < self.max_uses }
            update {
                self.use_count += 1;
                self.active_attachment_id = Some(attachment_id);
                self.granted_attachment_ids.insert(attachment_id);
            }
            to Attached
            emit AttachmentGranted {
                attachment_id: attachment_id,
                use_index: self.use_count,
                remaining_uses: self.max_uses - self.use_count
            }
        }

        // Totality arm: a live grant path always terminalizes on the release that
        // spends the budget, so this shape is reachable only from a recovered
        // record whose budget was already spent while detached.
        transition AttachActiveBudgetSpent {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "authentication_valid" { authentication_valid == true }
            guard "well_formed_attachment" { attachment_id != "" }
            guard "not_expired" { expired == false }
            guard "fresh_attachment" { self.granted_attachment_ids.contains(attachment_id) == false }
            guard "reuse_budget_spent" { self.use_count >= self.max_uses }
            update {}
            to Active
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::Exhausted
            }
        }

        // Exact attach replay returns the original grant and consumes no use.
        // Expiry is not consumed here: replaying an existing grant is not new
        // work, and `ObserveExpiry` owns expiry recording while attached.
        transition AttachAttachedReplay {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "authentication_valid" { authentication_valid == true }
            guard "exact_attachment_replay" { self.active_attachment_id == Some(attachment_id) }
            update {}
            to Attached
            emit AttachmentGrantReplayed { attachment_id: attachment_id, use_index: self.use_count }
        }

        transition AttachAttachedAlreadyReleased {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "authentication_valid" { authentication_valid == true }
            guard "different_attachment" { self.active_attachment_id != Some(attachment_id) }
            guard "already_granted_attachment" { self.granted_attachment_ids.contains(attachment_id) }
            update {}
            to Attached
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::AttachmentAlreadyReleased
            }
        }

        transition AttachAttachedBusy {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "authentication_valid" { authentication_valid == true }
            guard "different_attachment" { self.active_attachment_id != Some(attachment_id) }
            guard "fresh_attachment" { self.granted_attachment_ids.contains(attachment_id) == false }
            update {}
            to Attached
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::Busy
            }
        }

        transition AttachRevokedCapabilityRevocationPendingAttached {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to RevocationPendingAttached
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::Revoked
            }
        }

        transition AttachRevokedCapabilityRevoked {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to Revoked
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::Revoked
            }
        }

        transition AttachExpiredCapabilityExpiryPendingAttached {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to ExpiryPendingAttached
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::Expired
            }
        }

        transition AttachExpiredCapabilityExpired {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to Expired
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::Expired
            }
        }

        transition AttachExhaustedCapabilityExhausted {
            on input Attach { attachment_id, authentication_valid, expired }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to Exhausted
            emit AttachDenied {
                attachment_id: attachment_id,
                reason: ForkedParticipantAttachDenial::Exhausted
            }
        }

        // ------------------------------------------------------------------
        // Release
        // Only the exact active attachment releases. A duplicate release of any granted
        // identity converges through a typed replay instead of failing ambiguously.
        // ------------------------------------------------------------------

        transition ReleaseAttachedWithBudgetLeft {
            on input Release { attachment_id }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "exact_attachment" { self.active_attachment_id == Some(attachment_id) }
            guard "reuse_budget_left" { self.use_count < self.max_uses }
            update {
                self.active_attachment_id = None;
            }
            to Active
            emit AttachmentReleased { attachment_id: attachment_id, use_count: self.use_count }
        }

        transition ReleaseAttachedExhausts {
            on input Release { attachment_id }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "exact_attachment" { self.active_attachment_id == Some(attachment_id) }
            guard "reuse_budget_spent" { self.use_count >= self.max_uses }
            update {
                self.active_attachment_id = None;
                self.cleanup_state = ForkedParticipantCleanupState::Pending;
            }
            to Exhausted
            emit AttachmentReleased { attachment_id: attachment_id, use_count: self.use_count }
            emit CapabilityExhausted { use_count: self.use_count }
        }

        transition ReleaseRevocationPending {
            on input Release { attachment_id }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            guard "exact_attachment" { self.active_attachment_id == Some(attachment_id) }
            update {
                self.active_attachment_id = None;
                self.cleanup_state = ForkedParticipantCleanupState::Pending;
            }
            to Revoked
            emit AttachmentReleased { attachment_id: attachment_id, use_count: self.use_count }
            emit CapabilityRevoked { cleanup_pending: true }
        }

        transition ReleaseExpiryPending {
            on input Release { attachment_id }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            guard "exact_attachment" { self.active_attachment_id == Some(attachment_id) }
            update {
                self.active_attachment_id = None;
                self.cleanup_state = ForkedParticipantCleanupState::Pending;
            }
            to Expired
            emit AttachmentReleased { attachment_id: attachment_id, use_count: self.use_count }
            emit CapabilityExpired { cleanup_pending: true }
        }

        transition ReleaseDuplicateWhileAttachedAttached {
            on input Release { attachment_id }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "different_attachment" { self.active_attachment_id != Some(attachment_id) }
            guard "already_granted_attachment" { self.granted_attachment_ids.contains(attachment_id) }
            update {}
            to Attached
            emit ReleaseReplayed { attachment_id: attachment_id }
        }

        transition ReleaseAttachmentMismatchAttached {
            on input Release { attachment_id }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "different_attachment" { self.active_attachment_id != Some(attachment_id) }
            guard "unknown_attachment" { self.granted_attachment_ids.contains(attachment_id) == false }
            update {}
            to Attached
            emit ReleaseRejected {
                attachment_id: attachment_id,
                reason: ForkedParticipantReleaseRejection::AttachmentMismatch
            }
        }

        transition ReleaseDuplicateWhileAttachedRevocationPendingAttached {
            on input Release { attachment_id }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            guard "different_attachment" { self.active_attachment_id != Some(attachment_id) }
            guard "already_granted_attachment" { self.granted_attachment_ids.contains(attachment_id) }
            update {}
            to RevocationPendingAttached
            emit ReleaseReplayed { attachment_id: attachment_id }
        }

        transition ReleaseAttachmentMismatchRevocationPendingAttached {
            on input Release { attachment_id }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            guard "different_attachment" { self.active_attachment_id != Some(attachment_id) }
            guard "unknown_attachment" { self.granted_attachment_ids.contains(attachment_id) == false }
            update {}
            to RevocationPendingAttached
            emit ReleaseRejected {
                attachment_id: attachment_id,
                reason: ForkedParticipantReleaseRejection::AttachmentMismatch
            }
        }

        transition ReleaseDuplicateWhileAttachedExpiryPendingAttached {
            on input Release { attachment_id }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            guard "different_attachment" { self.active_attachment_id != Some(attachment_id) }
            guard "already_granted_attachment" { self.granted_attachment_ids.contains(attachment_id) }
            update {}
            to ExpiryPendingAttached
            emit ReleaseReplayed { attachment_id: attachment_id }
        }

        transition ReleaseAttachmentMismatchExpiryPendingAttached {
            on input Release { attachment_id }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            guard "different_attachment" { self.active_attachment_id != Some(attachment_id) }
            guard "unknown_attachment" { self.granted_attachment_ids.contains(attachment_id) == false }
            update {}
            to ExpiryPendingAttached
            emit ReleaseRejected {
                attachment_id: attachment_id,
                reason: ForkedParticipantReleaseRejection::AttachmentMismatch
            }
        }

        transition ReleaseDuplicateConvergesActive {
            on input Release { attachment_id }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "already_granted_attachment" { self.granted_attachment_ids.contains(attachment_id) }
            update {}
            to Active
            emit ReleaseReplayed { attachment_id: attachment_id }
        }

        transition ReleaseUnknownAttachmentActive {
            on input Release { attachment_id }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "unknown_attachment" { self.granted_attachment_ids.contains(attachment_id) == false }
            update {}
            to Active
            emit ReleaseRejected {
                attachment_id: attachment_id,
                reason: ForkedParticipantReleaseRejection::NoActiveAttachment
            }
        }

        transition ReleaseDuplicateConvergesRevoked {
            on input Release { attachment_id }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "already_granted_attachment" { self.granted_attachment_ids.contains(attachment_id) }
            update {}
            to Revoked
            emit ReleaseReplayed { attachment_id: attachment_id }
        }

        transition ReleaseUnknownAttachmentRevoked {
            on input Release { attachment_id }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "unknown_attachment" { self.granted_attachment_ids.contains(attachment_id) == false }
            update {}
            to Revoked
            emit ReleaseRejected {
                attachment_id: attachment_id,
                reason: ForkedParticipantReleaseRejection::NoActiveAttachment
            }
        }

        transition ReleaseDuplicateConvergesExpired {
            on input Release { attachment_id }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "already_granted_attachment" { self.granted_attachment_ids.contains(attachment_id) }
            update {}
            to Expired
            emit ReleaseReplayed { attachment_id: attachment_id }
        }

        transition ReleaseUnknownAttachmentExpired {
            on input Release { attachment_id }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "unknown_attachment" { self.granted_attachment_ids.contains(attachment_id) == false }
            update {}
            to Expired
            emit ReleaseRejected {
                attachment_id: attachment_id,
                reason: ForkedParticipantReleaseRejection::NoActiveAttachment
            }
        }

        transition ReleaseDuplicateConvergesExhausted {
            on input Release { attachment_id }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "already_granted_attachment" { self.granted_attachment_ids.contains(attachment_id) }
            update {}
            to Exhausted
            emit ReleaseReplayed { attachment_id: attachment_id }
        }

        transition ReleaseUnknownAttachmentExhausted {
            on input Release { attachment_id }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "unknown_attachment" { self.granted_attachment_ids.contains(attachment_id) == false }
            update {}
            to Exhausted
            emit ReleaseRejected {
                attachment_id: attachment_id,
                reason: ForkedParticipantReleaseRejection::NoActiveAttachment
            }
        }

        // A pre-activation record has granted nothing (invariant
        // `pre_activation_record_has_no_grants`), so every release is unknown.
        transition ReleaseUnknownAttachmentEmpty {
            on input Release { attachment_id }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit ReleaseRejected {
                attachment_id: attachment_id,
                reason: ForkedParticipantReleaseRejection::NoActiveAttachment
            }
        }

        transition ReleaseUnknownAttachmentReserved {
            on input Release { attachment_id }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            update {}
            to Reserved
            emit ReleaseRejected {
                attachment_id: attachment_id,
                reason: ForkedParticipantReleaseRejection::NoActiveAttachment
            }
        }

        transition ReleaseUnknownAttachmentActivationFailed {
            on input Release { attachment_id }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            update {}
            to ActivationFailed
            emit ReleaseRejected {
                attachment_id: attachment_id,
                reason: ForkedParticipantReleaseRejection::NoActiveAttachment
            }
        }

        // ------------------------------------------------------------------
        // Revoke
        // Authenticated, convergent, and attachment-aware. An invalid authentication
        // observation never changes state in any phase.
        // ------------------------------------------------------------------

        transition RevokeAuthenticationInvalidEmpty {
            on input Revoke { authentication_valid }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Empty
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AuthenticationInvalid }
        }

        transition RevokeAuthenticationInvalidReserved {
            on input Revoke { authentication_valid }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Reserved
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AuthenticationInvalid }
        }

        transition RevokeAuthenticationInvalidActivationFailed {
            on input Revoke { authentication_valid }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to ActivationFailed
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AuthenticationInvalid }
        }

        transition RevokeAuthenticationInvalidActive {
            on input Revoke { authentication_valid }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Active
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AuthenticationInvalid }
        }

        transition RevokeAuthenticationInvalidAttached {
            on input Revoke { authentication_valid }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Attached
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AuthenticationInvalid }
        }

        transition RevokeAuthenticationInvalidRevocationPendingAttached {
            on input Revoke { authentication_valid }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to RevocationPendingAttached
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AuthenticationInvalid }
        }

        transition RevokeAuthenticationInvalidExpiryPendingAttached {
            on input Revoke { authentication_valid }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to ExpiryPendingAttached
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AuthenticationInvalid }
        }

        transition RevokeAuthenticationInvalidRevoked {
            on input Revoke { authentication_valid }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Revoked
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AuthenticationInvalid }
        }

        transition RevokeAuthenticationInvalidExpired {
            on input Revoke { authentication_valid }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Expired
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AuthenticationInvalid }
        }

        transition RevokeAuthenticationInvalidExhausted {
            on input Revoke { authentication_valid }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "authentication_invalid" { authentication_valid == false }
            update {}
            to Exhausted
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AuthenticationInvalid }
        }

        transition RevokeEmpty {
            on input Revoke { authentication_valid }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to Empty
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::NotProvisioned }
        }

        // No durable fork was ever created, so revocation accrues no debt.
        transition RevokeReserved {
            on input Revoke { authentication_valid }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to Revoked
            emit CapabilityRevoked { cleanup_pending: false }
        }

        // Create-failed is ambiguous about durable residue, so revocation accrues
        // cleanup debt.
        transition RevokeActivationFailed {
            on input Revoke { authentication_valid }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "authentication_valid" { authentication_valid == true }
            update {
                self.cleanup_state = ForkedParticipantCleanupState::Pending;
            }
            to Revoked
            emit CapabilityRevoked { cleanup_pending: true }
        }

        transition RevokeActive {
            on input Revoke { authentication_valid }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "authentication_valid" { authentication_valid == true }
            update {
                self.cleanup_state = ForkedParticipantCleanupState::Pending;
            }
            to Revoked
            emit CapabilityRevoked { cleanup_pending: true }
        }

        transition RevokeAttached {
            on input Revoke { authentication_valid }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "authentication_valid" { authentication_valid == true }
            update {
                self.cleanup_state = ForkedParticipantCleanupState::Deferred;
            }
            to RevocationPendingAttached
            emit RevocationPendingRecorded
        }

        // Revocation dominates a pending expiry: both wait for the exact release,
        // and the stronger terminal fact wins.
        transition RevokeExpiryPendingAttached {
            on input Revoke { authentication_valid }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            guard "authentication_valid" { authentication_valid == true }
            update {
                self.cleanup_state = ForkedParticipantCleanupState::Deferred;
            }
            to RevocationPendingAttached
            emit RevocationPendingRecorded
        }

        transition RevokeRevocationPendingReplay {
            on input Revoke { authentication_valid }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to RevocationPendingAttached
            emit RevocationConverged
        }

        transition RevokeRevokedReplay {
            on input Revoke { authentication_valid }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to Revoked
            emit RevocationConverged
        }

        transition RevokeAlreadyTerminalExpired {
            on input Revoke { authentication_valid }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to Expired
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AlreadyTerminal }
        }

        transition RevokeAlreadyTerminalExhausted {
            on input Revoke { authentication_valid }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "authentication_valid" { authentication_valid == true }
            update {}
            to Exhausted
            emit RevocationDenied { reason: ForkedParticipantRevocationDenial::AlreadyTerminal }
        }

        // ------------------------------------------------------------------
        // ObserveExpiry
        // The shell reports the observation, the machine owns what it means. A
        // `not expired` observation is an explicit typed no-op in every phase.
        // ------------------------------------------------------------------

        transition ExpiryNotObservedEmpty {
            on input ObserveExpiry { expired }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            guard "not_expired" { expired == false }
            update {}
            to Empty
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::NotExpired }
        }

        transition ExpiryNotObservedReserved {
            on input ObserveExpiry { expired }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "not_expired" { expired == false }
            update {}
            to Reserved
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::NotExpired }
        }

        transition ExpiryNotObservedActivationFailed {
            on input ObserveExpiry { expired }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "not_expired" { expired == false }
            update {}
            to ActivationFailed
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::NotExpired }
        }

        transition ExpiryNotObservedActive {
            on input ObserveExpiry { expired }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "not_expired" { expired == false }
            update {}
            to Active
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::NotExpired }
        }

        transition ExpiryNotObservedAttached {
            on input ObserveExpiry { expired }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "not_expired" { expired == false }
            update {}
            to Attached
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::NotExpired }
        }

        transition ExpiryNotObservedRevocationPendingAttached {
            on input ObserveExpiry { expired }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            guard "not_expired" { expired == false }
            update {}
            to RevocationPendingAttached
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::NotExpired }
        }

        transition ExpiryNotObservedExpiryPendingAttached {
            on input ObserveExpiry { expired }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            guard "not_expired" { expired == false }
            update {}
            to ExpiryPendingAttached
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::NotExpired }
        }

        transition ExpiryNotObservedRevoked {
            on input ObserveExpiry { expired }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "not_expired" { expired == false }
            update {}
            to Revoked
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::NotExpired }
        }

        transition ExpiryNotObservedExpired {
            on input ObserveExpiry { expired }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "not_expired" { expired == false }
            update {}
            to Expired
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::NotExpired }
        }

        transition ExpiryNotObservedExhausted {
            on input ObserveExpiry { expired }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "not_expired" { expired == false }
            update {}
            to Exhausted
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::NotExpired }
        }

        transition ExpiryObservedEmpty {
            on input ObserveExpiry { expired }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            guard "expired" { expired == true }
            update {}
            to Empty
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::NotProvisioned }
        }

        transition ExpireReserved {
            on input ObserveExpiry { expired }
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            guard "expired" { expired == true }
            update {}
            to Expired
            emit CapabilityExpired { cleanup_pending: false }
        }

        transition ExpireActivationFailed {
            on input ObserveExpiry { expired }
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            guard "expired" { expired == true }
            update {
                self.cleanup_state = ForkedParticipantCleanupState::Pending;
            }
            to Expired
            emit CapabilityExpired { cleanup_pending: true }
        }

        transition ExpireActive {
            on input ObserveExpiry { expired }
            guard "active" { self.lifecycle_phase == Phase::Active }
            guard "expired" { expired == true }
            update {
                self.cleanup_state = ForkedParticipantCleanupState::Pending;
            }
            to Expired
            emit CapabilityExpired { cleanup_pending: true }
        }

        transition ExpireAttached {
            on input ObserveExpiry { expired }
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            guard "expired" { expired == true }
            update {
                self.cleanup_state = ForkedParticipantCleanupState::Deferred;
            }
            to ExpiryPendingAttached
            emit ExpiryPendingRecorded
        }

        transition ExpiryPendingReplay {
            on input ObserveExpiry { expired }
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            guard "expired" { expired == true }
            update {}
            to ExpiryPendingAttached
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::AlreadyRecorded }
        }

        transition ExpiryUnderRevocationPending {
            on input ObserveExpiry { expired }
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            guard "expired" { expired == true }
            update {}
            to RevocationPendingAttached
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::AlreadyRecorded }
        }

        transition ExpiryAfterTerminalRevoked {
            on input ObserveExpiry { expired }
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "expired" { expired == true }
            update {}
            to Revoked
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::Terminal }
        }

        transition ExpiryAfterTerminalExpired {
            on input ObserveExpiry { expired }
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "expired" { expired == true }
            update {}
            to Expired
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::Terminal }
        }

        transition ExpiryAfterTerminalExhausted {
            on input ObserveExpiry { expired }
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "expired" { expired == true }
            update {}
            to Exhausted
            emit ExpiryObservationIgnored { reason: ForkedParticipantExpiryIgnore::Terminal }
        }

        // ------------------------------------------------------------------
        // CompleteCleanup
        // Admitted only for a terminal, detached record that actually carries cleanup
        // debt. Replay converges; every other shape is a typed refusal that mutates
        // nothing.
        // ------------------------------------------------------------------

        transition CompleteCleanupPendingDebtRevoked {
            on input CompleteCleanup {}
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "cleanup_debt_pending" { self.cleanup_state == ForkedParticipantCleanupState::Pending }
            update {
                self.cleanup_state = ForkedParticipantCleanupState::Complete;
            }
            to Revoked
            emit CleanupCompleted
        }

        transition CompleteCleanupReplayRevoked {
            on input CompleteCleanup {}
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "cleanup_already_complete" { self.cleanup_state == ForkedParticipantCleanupState::Complete }
            update {}
            to Revoked
            emit CleanupCompletionReplayed
        }

        transition CompleteCleanupWithoutDebtRevoked {
            on input CompleteCleanup {}
            guard "revoked" { self.lifecycle_phase == Phase::Revoked }
            guard "no_cleanup_debt" {
                self.cleanup_state != ForkedParticipantCleanupState::Pending
                    && self.cleanup_state != ForkedParticipantCleanupState::Complete
            }
            update {}
            to Revoked
            emit CleanupCompletionRejected { reason: ForkedParticipantCleanupRejection::NoCleanupDebt }
        }

        transition CompleteCleanupPendingDebtExpired {
            on input CompleteCleanup {}
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "cleanup_debt_pending" { self.cleanup_state == ForkedParticipantCleanupState::Pending }
            update {
                self.cleanup_state = ForkedParticipantCleanupState::Complete;
            }
            to Expired
            emit CleanupCompleted
        }

        transition CompleteCleanupReplayExpired {
            on input CompleteCleanup {}
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "cleanup_already_complete" { self.cleanup_state == ForkedParticipantCleanupState::Complete }
            update {}
            to Expired
            emit CleanupCompletionReplayed
        }

        transition CompleteCleanupWithoutDebtExpired {
            on input CompleteCleanup {}
            guard "expired" { self.lifecycle_phase == Phase::Expired }
            guard "no_cleanup_debt" {
                self.cleanup_state != ForkedParticipantCleanupState::Pending
                    && self.cleanup_state != ForkedParticipantCleanupState::Complete
            }
            update {}
            to Expired
            emit CleanupCompletionRejected { reason: ForkedParticipantCleanupRejection::NoCleanupDebt }
        }

        transition CompleteCleanupPendingDebtExhausted {
            on input CompleteCleanup {}
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "cleanup_debt_pending" { self.cleanup_state == ForkedParticipantCleanupState::Pending }
            update {
                self.cleanup_state = ForkedParticipantCleanupState::Complete;
            }
            to Exhausted
            emit CleanupCompleted
        }

        transition CompleteCleanupReplayExhausted {
            on input CompleteCleanup {}
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "cleanup_already_complete" { self.cleanup_state == ForkedParticipantCleanupState::Complete }
            update {}
            to Exhausted
            emit CleanupCompletionReplayed
        }

        transition CompleteCleanupWithoutDebtExhausted {
            on input CompleteCleanup {}
            guard "exhausted" { self.lifecycle_phase == Phase::Exhausted }
            guard "no_cleanup_debt" {
                self.cleanup_state != ForkedParticipantCleanupState::Pending
                    && self.cleanup_state != ForkedParticipantCleanupState::Complete
            }
            update {}
            to Exhausted
            emit CleanupCompletionRejected { reason: ForkedParticipantCleanupRejection::NoCleanupDebt }
        }

        transition CompleteCleanupWhileAttachedAttached {
            on input CompleteCleanup {}
            guard "attached" { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
            emit CleanupCompletionRejected { reason: ForkedParticipantCleanupRejection::AttachmentOutstanding }
        }

        transition CompleteCleanupWhileAttachedRevocationPendingAttached {
            on input CompleteCleanup {}
            guard "revocation_pending_attached" { self.lifecycle_phase == Phase::RevocationPendingAttached }
            update {}
            to RevocationPendingAttached
            emit CleanupCompletionRejected { reason: ForkedParticipantCleanupRejection::AttachmentOutstanding }
        }

        transition CompleteCleanupWhileAttachedExpiryPendingAttached {
            on input CompleteCleanup {}
            guard "expiry_pending_attached" { self.lifecycle_phase == Phase::ExpiryPendingAttached }
            update {}
            to ExpiryPendingAttached
            emit CleanupCompletionRejected { reason: ForkedParticipantCleanupRejection::AttachmentOutstanding }
        }

        transition CompleteCleanupNotTerminalEmpty {
            on input CompleteCleanup {}
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit CleanupCompletionRejected { reason: ForkedParticipantCleanupRejection::NotTerminal }
        }

        transition CompleteCleanupNotTerminalReserved {
            on input CompleteCleanup {}
            guard "reserved" { self.lifecycle_phase == Phase::Reserved }
            update {}
            to Reserved
            emit CleanupCompletionRejected { reason: ForkedParticipantCleanupRejection::NotTerminal }
        }

        transition CompleteCleanupNotTerminalActivationFailed {
            on input CompleteCleanup {}
            guard "activation_failed" { self.lifecycle_phase == Phase::ActivationFailed }
            update {}
            to ActivationFailed
            emit CleanupCompletionRejected { reason: ForkedParticipantCleanupRejection::NotTerminal }
        }

        transition CompleteCleanupNotTerminalActive {
            on input CompleteCleanup {}
            guard "active" { self.lifecycle_phase == Phase::Active }
            update {}
            to Active
            emit CleanupCompletionRejected { reason: ForkedParticipantCleanupRejection::NotTerminal }
        }
    }
}

impl serde::Serialize for ForkedParticipantLifecycleState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Empty => "empty",
            Self::Reserved => "reserved",
            Self::ActivationFailed => "activation_failed",
            Self::Active => "active",
            Self::Attached => "attached",
            Self::RevocationPendingAttached => "revocation_pending_attached",
            Self::ExpiryPendingAttached => "expiry_pending_attached",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
            Self::Exhausted => "exhausted",
        })
    }
}

impl<'de> serde::Deserialize<'de> for ForkedParticipantLifecycleState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        match value.as_str() {
            "empty" => Ok(Self::Empty),
            "reserved" => Ok(Self::Reserved),
            "activation_failed" => Ok(Self::ActivationFailed),
            "active" => Ok(Self::Active),
            "attached" => Ok(Self::Attached),
            "revocation_pending_attached" => Ok(Self::RevocationPendingAttached),
            "expiry_pending_attached" => Ok(Self::ExpiryPendingAttached),
            "revoked" => Ok(Self::Revoked),
            "expired" => Ok(Self::Expired),
            "exhausted" => Ok(Self::Exhausted),
            other => Err(serde::de::Error::custom(format!(
                "invalid ForkedParticipantLifecycleState `{other}`"
            ))),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ForkedParticipantLifecycleMachineStateWire {
    lifecycle_phase: ForkedParticipantLifecycleState,
    #[serde(default)]
    request_fingerprint: String,
    #[serde(default)]
    max_uses: u64,
    #[serde(default)]
    use_count: u64,
    #[serde(default)]
    fork_activation_id: String,
    #[serde(default)]
    active_attachment_id: Option<String>,
    #[serde(default)]
    granted_attachment_ids: std::collections::BTreeSet<String>,
    #[serde(default)]
    cleanup_state: ForkedParticipantCleanupState,
}

impl From<&ForkedParticipantLifecycleMachineState> for ForkedParticipantLifecycleMachineStateWire {
    fn from(state: &ForkedParticipantLifecycleMachineState) -> Self {
        Self {
            lifecycle_phase: state.lifecycle_phase,
            request_fingerprint: state.request_fingerprint.clone(),
            max_uses: state.max_uses,
            use_count: state.use_count,
            fork_activation_id: state.fork_activation_id.clone(),
            active_attachment_id: state.active_attachment_id.clone(),
            granted_attachment_ids: state.granted_attachment_ids.clone(),
            cleanup_state: state.cleanup_state,
        }
    }
}

impl From<ForkedParticipantLifecycleMachineStateWire> for ForkedParticipantLifecycleMachineState {
    fn from(wire: ForkedParticipantLifecycleMachineStateWire) -> Self {
        Self {
            lifecycle_phase: wire.lifecycle_phase,
            request_fingerprint: wire.request_fingerprint,
            max_uses: wire.max_uses,
            use_count: wire.use_count,
            fork_activation_id: wire.fork_activation_id,
            active_attachment_id: wire.active_attachment_id,
            granted_attachment_ids: wire.granted_attachment_ids,
            cleanup_state: wire.cleanup_state,
        }
    }
}

impl serde::Serialize for ForkedParticipantLifecycleMachineState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ForkedParticipantLifecycleMachineStateWire::from(self).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for ForkedParticipantLifecycleMachineState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        ForkedParticipantLifecycleMachineStateWire::deserialize(deserializer).map(Self::from)
    }
}
