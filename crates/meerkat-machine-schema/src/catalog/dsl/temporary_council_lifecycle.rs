//! Canonical lifecycle authority for ONE temporary-council record (issue #159).
//!
//! A temporary council is a bounded conversation between forked-participant
//! capabilities seated in a REAL, short-lived mob. This machine owns the
//! council record's own lifecycle: which request identity the record is bound
//! to, how far the coordinator progressed, whether an immutable result was
//! sealed (and whether that seal was the normal terminal or a crash-recovered
//! `CoordinatorInterrupted` one), and whether cleanup converged or retained
//! debt.
//!
//! # Scope discipline
//!
//! Record-scoped, not registry-scoped: the store persists one machine state per
//! council record, so there is no index or collection keyed by council identity
//! inside the machine.
//!
//! The machine deliberately does NOT own:
//!
//! - **Mob, member, or capability lifecycle.** `MobMachine`, the member
//!   machines, and `ForkedParticipantLifecycleMachine` remain canonical there.
//!   This machine never mirrors a member phase.
//! - **Time.** It reads no clock. Deadline expiry reaches the coordinator as a
//!   typed exit reason payload the shell records alongside the sealed result.
//! - **Result and receipt bodies.** The exchange receipts, the merge outcome,
//!   and the cleanup debt detail are durable sidecar data. The machine owns
//!   only the legality of sealing and settling them.
//!
//! # Totality discipline
//!
//! Every (phase, command class) pair has an explicit transition naming its own
//! `to` phase. Rejection and replay arms return to the exact phase they
//! matched, so no typed refusal mutates lifecycle state. There is no catch-all
//! arm.
//!
//! # Recovery classification
//!
//! `ClassifyRecovery` is the machine-owned answer to "does a recovery sweep
//! still owe this record work, is its result already sealed, and does cleanup
//! still need to run". The shell mirrors the emitted verdict and holds no
//! phase predicate of its own — that is what keeps `list_unfinished`,
//! result-sealing, and settlement from becoming a second authority.

use meerkat_machine_dsl::machine;

/// Which terminal a sealed council result carries.
///
/// `Unsealed` is the honest pre-terminal value: no result exists yet. The two
/// sealed classes are kept apart because they mean materially different things
/// to a caller — one is an executed council, the other is a crash-recovered
/// record that must never be re-executed.
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
pub enum TemporaryCouncilExitClass {
    /// No immutable result has been sealed.
    #[default]
    Unsealed,
    /// The coordinator ran the council to a terminal of its own.
    Executed,
    /// A recovery sweep sealed the record because its coordinator died.
    CoordinatorInterrupted,
}

/// Why a coordinator claim was denied.
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
pub enum TemporaryCouncilClaimDenial {
    /// Empty claim identity.
    #[default]
    MalformedClaim,
    /// The record carries no bound request yet.
    NotOpened,
    /// Another coordinator holds a lease the shell did NOT observe as expired.
    HeldByAnotherCoordinator,
    /// The council already settled; there is nothing left to execute.
    AlreadySettled,
}

/// Why an `Open` command was refused.
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
pub enum TemporaryCouncilOpenRejection {
    /// Empty request fingerprint.
    #[default]
    MalformedRequest,
    /// A materially different request tried to take a bound council identity.
    FingerprintConflict,
}

/// Why a phase-advance command was refused.
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
pub enum TemporaryCouncilAdvanceRejection {
    /// The record carries no bound request yet.
    #[default]
    NotOpened,
    /// The record already advanced past the requested step.
    AlreadyAdvanced,
}

/// Why a result-seal command was refused.
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
pub enum TemporaryCouncilSealRejection {
    /// The record carries no bound request yet.
    #[default]
    NotOpened,
    /// A normal seal was attempted before the merge step was entered.
    NotMerging,
    /// A result is already sealed under a different terminal class.
    AlreadySealed,
}

/// Why a cleanup-outcome command was refused.
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
pub enum TemporaryCouncilCleanupRejection {
    /// Cleanup cannot be recorded before an immutable result exists.
    #[default]
    ResultNotSealed,
    /// The record already settled; debt cannot be re-opened.
    AlreadySettled,
}

machine! {
    machine TemporaryCouncilLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::temporary_council_lifecycle",

        state {
            lifecycle_phase: TemporaryCouncilLifecycleState,
            // Monotonic advance counter. Every real lifecycle advance bumps it;
            // replays and rejections never do.
            revision: u64,
            // Exact request identity this council id is bound to. Empty only in
            // the `Empty` phase.
            request_fingerprint: String,
            // Which terminal the sealed result carries.
            exit_class: Enum<TemporaryCouncilExitClass>,
            // Cleanup attempts recorded so far. Only a sealed record can have
            // any.
            cleanup_attempts: u64,
            // Identity of the coordinator that currently holds the execution
            // claim. Empty means unheld.
            claim_id: String,
            // Monotonic claim epoch. Every grant and every takeover advances
            // it, so a command carrying an older epoch is provably stale.
            claim_epoch: u64,
        }

        init(Empty) {
            revision = 0,
            request_fingerprint = "",
            exit_class = TemporaryCouncilExitClass::Unsealed,
            cleanup_attempts = 0,
            claim_id = "",
            claim_epoch = 0,
        }

        terminal [Settled]

        phase TemporaryCouncilLifecycleState {
            // No council request bound yet.
            Empty,
            // Request bound; participants are being seated and wired.
            Preparing,
            // Bounded discussion rounds are running.
            Running,
            // The explicit merge-back policy is being applied.
            Merging,
            // The immutable result is sealed; cleanup has not converged.
            Concluded,
            // Cleanup ran and retained typed debt.
            CleanupDebt,
            // Result sealed AND cleanup converged.
            Settled,
        }

        input TemporaryCouncilLifecycleInput {
            // Bind the record to one canonical request fingerprint.
            Open { request_fingerprint: String },
            // Take (or renew, or expiry-take-over) the execution claim.
            //
            // `lease_expired` is a SHELL OBSERVATION: the machine reads no
            // clock. The shell samples the persisted lease deadline once and
            // passes the verdict in; the machine owns what it MEANS — only an
            // observed-expired lease may be taken over, and every takeover
            // advances the epoch, which is what fences the previous executor.
            Claim { claim_id: String, lease_expired: bool },
            // Every participant is seated and wired; discussion may run.
            StartDiscussion { claim_id: String, claim_epoch: u64 },
            // Discussion is over; the merge-back policy may be applied.
            StartMerge { claim_id: String, claim_epoch: u64 },
            // Seal the immutable result produced by this coordinator.
            SealResult { claim_id: String, claim_epoch: u64 },
            // Seal a terminal for a council whose coordinator did not survive.
            SealInterruptedResult { claim_id: String, claim_epoch: u64 },
            // A cleanup attempt discharged every obligation.
            RecordCleanupSettled { claim_id: String, claim_epoch: u64 },
            // A cleanup attempt retained typed debt.
            RecordCleanupDebt { claim_id: String, claim_epoch: u64 },
            // Ask the machine what a recovery sweep still owes this record.
            // A read: it needs no claim and mutates nothing.
            ClassifyRecovery {},
        }

        effect TemporaryCouncilLifecycleEffect {
            CouncilOpened { request_fingerprint: String },
            CouncilOpenReplayed { request_fingerprint: String },
            CouncilOpenRejected { reason: Enum<TemporaryCouncilOpenRejection> },
            DiscussionStarted { revision: u64 },
            DiscussionStartReplayed { revision: u64 },
            MergeStarted { revision: u64 },
            MergeStartReplayed { revision: u64 },
            AdvanceRejected { reason: Enum<TemporaryCouncilAdvanceRejection> },
            ResultSealed { revision: u64, exit_class: Enum<TemporaryCouncilExitClass> },
            ResultSealReplayed { exit_class: Enum<TemporaryCouncilExitClass> },
            ResultSealRejected { reason: Enum<TemporaryCouncilSealRejection> },
            CleanupSettled { revision: u64, attempts: u64 },
            CleanupDebtRecorded { revision: u64, attempts: u64 },
            CleanupSettlementReplayed { attempts: u64 },
            CleanupRejected { reason: Enum<TemporaryCouncilCleanupRejection> },
            RecoveryClassified { unfinished: bool, result_sealed: bool, needs_cleanup: bool },
            ClaimGranted { claim_id: String, claim_epoch: u64, took_over: bool },
            ClaimRenewed { claim_id: String, claim_epoch: u64 },
            ClaimDenied { reason: Enum<TemporaryCouncilClaimDenial>, current_claim_epoch: u64 },
            // A command arrived under a claim this record no longer honours.
            // The stale executor is fenced: it may not advance, seal, or
            // settle anything.
            CommandFenced { current_claim_epoch: u64 },
        }

        // An unbound record carries no council facts at all.
        invariant empty_record_has_no_council_facts {
            self.lifecycle_phase != Phase::Empty
                || (
                    self.request_fingerprint == ""
                    && self.exit_class == TemporaryCouncilExitClass::Unsealed
                    && self.cleanup_attempts == 0
                    && self.revision == 0
                )
        }

        // Every bound record names the exact request it is bound to.
        invariant opened_record_is_fingerprint_bound {
            self.lifecycle_phase == Phase::Empty || self.request_fingerprint != ""
        }

        // A sealed phase always names which terminal it sealed...
        invariant sealed_phase_has_exit_class {
            (
                self.lifecycle_phase != Phase::Concluded
                && self.lifecycle_phase != Phase::CleanupDebt
                && self.lifecycle_phase != Phase::Settled
            )
            || self.exit_class != TemporaryCouncilExitClass::Unsealed
        }

        // ...and no pre-seal phase may claim one.
        invariant unsealed_phase_has_no_exit_class {
            (
                self.lifecycle_phase != Phase::Empty
                && self.lifecycle_phase != Phase::Preparing
                && self.lifecycle_phase != Phase::Running
                && self.lifecycle_phase != Phase::Merging
            )
            || self.exit_class == TemporaryCouncilExitClass::Unsealed
        }

        // Cleanup is only recordable after a result exists, so an unsealed
        // record can never carry an attempt.
        invariant cleanup_attempts_require_a_sealed_result {
            self.cleanup_attempts == 0
                || self.lifecycle_phase == Phase::CleanupDebt
                || self.lifecycle_phase == Phase::Settled
        }

        // Retained debt and settlement are both produced by an attempt.
        invariant debt_and_settlement_have_attempts {
            (
                self.lifecycle_phase != Phase::CleanupDebt
                && self.lifecycle_phase != Phase::Settled
            )
            || self.cleanup_attempts > 0
        }

        // A claim identity and a positive epoch always travel together.
        invariant claim_identity_and_epoch_agree {
            (self.claim_id == "" && self.claim_epoch == 0)
                || (self.claim_id != "" && self.claim_epoch > 0)
        }

        // Only a bound record can carry an execution claim.
        invariant only_a_bound_record_is_claimable {
            self.lifecycle_phase != Phase::Empty || self.claim_id == ""
        }

        // Verdict effects: the calling surface mirrors them as its typed result.
        disposition CouncilOpened => local seam SurfaceResultAlignment,
        disposition CouncilOpenReplayed => local seam SurfaceResultAlignment,
        disposition CouncilOpenRejected => local seam SurfaceResultAlignment,
        disposition DiscussionStarted => local seam SurfaceResultAlignment,
        disposition DiscussionStartReplayed => local seam SurfaceResultAlignment,
        disposition MergeStarted => local seam SurfaceResultAlignment,
        disposition MergeStartReplayed => local seam SurfaceResultAlignment,
        disposition AdvanceRejected => local seam SurfaceResultAlignment,
        disposition ResultSealed => local seam SurfaceResultAlignment,
        disposition ResultSealReplayed => local seam SurfaceResultAlignment,
        disposition ResultSealRejected => local seam SurfaceResultAlignment,
        disposition CleanupSettled => local seam SurfaceResultAlignment,
        disposition CleanupDebtRecorded => local seam SurfaceResultAlignment,
        disposition CleanupSettlementReplayed => local seam SurfaceResultAlignment,
        disposition CleanupRejected => local seam SurfaceResultAlignment,
        disposition RecoveryClassified => local seam SurfaceResultAlignment,
        disposition ClaimGranted => local seam SurfaceResultAlignment,
        disposition ClaimRenewed => local seam SurfaceResultAlignment,
        disposition ClaimDenied => local seam SurfaceResultAlignment,
        disposition CommandFenced => local seam SurfaceResultAlignment,

        // ------------------------------------------------------------------
        // Open
        // Empty -> Preparing binds one canonical request fingerprint. An exact
        // replay converges; a materially different request is a typed reject
        // that never rebinds the identity.
        // ------------------------------------------------------------------

        transition OpenEmpty {
            on input Open { request_fingerprint }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            guard "well_formed_request" { request_fingerprint != "" }
            update {
                self.request_fingerprint = request_fingerprint;
                self.revision += 1;
            }
            to Preparing
            emit CouncilOpened { request_fingerprint: self.request_fingerprint }
        }

        transition OpenEmptyMalformed {
            on input Open { request_fingerprint }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            guard "malformed_request" { request_fingerprint == "" }
            update {}
            to Empty
            emit CouncilOpenRejected { reason: TemporaryCouncilOpenRejection::MalformedRequest }
        }

        transition OpenReplayPreparing {
            on input Open { request_fingerprint }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "exact_request_replay" { request_fingerprint == self.request_fingerprint }
            update {}
            to Preparing
            emit CouncilOpenReplayed { request_fingerprint: self.request_fingerprint }
        }

        transition OpenReplayRunning {
            on input Open { request_fingerprint }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "exact_request_replay" { request_fingerprint == self.request_fingerprint }
            update {}
            to Running
            emit CouncilOpenReplayed { request_fingerprint: self.request_fingerprint }
        }

        transition OpenReplayMerging {
            on input Open { request_fingerprint }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "exact_request_replay" { request_fingerprint == self.request_fingerprint }
            update {}
            to Merging
            emit CouncilOpenReplayed { request_fingerprint: self.request_fingerprint }
        }

        transition OpenReplayConcluded {
            on input Open { request_fingerprint }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "exact_request_replay" { request_fingerprint == self.request_fingerprint }
            update {}
            to Concluded
            emit CouncilOpenReplayed { request_fingerprint: self.request_fingerprint }
        }

        transition OpenReplayCleanupDebt {
            on input Open { request_fingerprint }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "exact_request_replay" { request_fingerprint == self.request_fingerprint }
            update {}
            to CleanupDebt
            emit CouncilOpenReplayed { request_fingerprint: self.request_fingerprint }
        }

        transition OpenReplaySettled {
            on input Open { request_fingerprint }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "exact_request_replay" { request_fingerprint == self.request_fingerprint }
            update {}
            to Settled
            emit CouncilOpenReplayed { request_fingerprint: self.request_fingerprint }
        }

        transition OpenConflictPreparing {
            on input Open { request_fingerprint }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "conflicting_request" { request_fingerprint != self.request_fingerprint }
            update {}
            to Preparing
            emit CouncilOpenRejected { reason: TemporaryCouncilOpenRejection::FingerprintConflict }
        }

        transition OpenConflictRunning {
            on input Open { request_fingerprint }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "conflicting_request" { request_fingerprint != self.request_fingerprint }
            update {}
            to Running
            emit CouncilOpenRejected { reason: TemporaryCouncilOpenRejection::FingerprintConflict }
        }

        transition OpenConflictMerging {
            on input Open { request_fingerprint }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "conflicting_request" { request_fingerprint != self.request_fingerprint }
            update {}
            to Merging
            emit CouncilOpenRejected { reason: TemporaryCouncilOpenRejection::FingerprintConflict }
        }

        transition OpenConflictConcluded {
            on input Open { request_fingerprint }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "conflicting_request" { request_fingerprint != self.request_fingerprint }
            update {}
            to Concluded
            emit CouncilOpenRejected { reason: TemporaryCouncilOpenRejection::FingerprintConflict }
        }

        transition OpenConflictCleanupDebt {
            on input Open { request_fingerprint }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "conflicting_request" { request_fingerprint != self.request_fingerprint }
            update {}
            to CleanupDebt
            emit CouncilOpenRejected { reason: TemporaryCouncilOpenRejection::FingerprintConflict }
        }

        transition OpenConflictSettled {
            on input Open { request_fingerprint }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "conflicting_request" { request_fingerprint != self.request_fingerprint }
            update {}
            to Settled
            emit CouncilOpenRejected { reason: TemporaryCouncilOpenRejection::FingerprintConflict }
        }

        // ------------------------------------------------------------------
        // Claim
        // Exactly one coordinator may execute a council at a time. A claim is
        // granted when the record is unheld, renewed for the same holder, and
        // taken over ONLY on an observed-expired lease. Every grant and every
        // takeover advances `claim_epoch`, which is what fences the previous
        // executor: its next command carries the old epoch and is refused
        // without mutating anything.
        // ------------------------------------------------------------------

        transition ClaimNotOpened {
            on input Claim { claim_id, lease_expired }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::NotOpened,
                current_claim_epoch: self.claim_epoch
            }
        }

        transition ClaimSettled {
            on input Claim { claim_id, lease_expired }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            update {}
            to Settled
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::AlreadySettled,
                current_claim_epoch: self.claim_epoch
            }
        }

        transition ClaimMalformedPreparing {
            on input Claim { claim_id, lease_expired }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "malformed_claim" { claim_id == "" }
            update {}
            to Preparing
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::MalformedClaim,
                current_claim_epoch: self.claim_epoch
            }
        }

        transition ClaimMalformedRunning {
            on input Claim { claim_id, lease_expired }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "malformed_claim" { claim_id == "" }
            update {}
            to Running
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::MalformedClaim,
                current_claim_epoch: self.claim_epoch
            }
        }

        transition ClaimMalformedMerging {
            on input Claim { claim_id, lease_expired }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "malformed_claim" { claim_id == "" }
            update {}
            to Merging
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::MalformedClaim,
                current_claim_epoch: self.claim_epoch
            }
        }

        transition ClaimMalformedConcluded {
            on input Claim { claim_id, lease_expired }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "malformed_claim" { claim_id == "" }
            update {}
            to Concluded
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::MalformedClaim,
                current_claim_epoch: self.claim_epoch
            }
        }

        transition ClaimMalformedCleanupDebt {
            on input Claim { claim_id, lease_expired }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "malformed_claim" { claim_id == "" }
            update {}
            to CleanupDebt
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::MalformedClaim,
                current_claim_epoch: self.claim_epoch
            }
        }

        transition ClaimGrantPreparing {
            on input Claim { claim_id, lease_expired }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "well_formed_claim" { claim_id != "" }
            guard "unheld" { self.claim_id == "" }
            update {
                self.claim_id = claim_id;
                self.claim_epoch += 1;
            }
            to Preparing
            emit ClaimGranted {
                claim_id: self.claim_id,
                claim_epoch: self.claim_epoch,
                took_over: false
            }
        }

        transition ClaimGrantRunning {
            on input Claim { claim_id, lease_expired }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "well_formed_claim" { claim_id != "" }
            guard "unheld" { self.claim_id == "" }
            update {
                self.claim_id = claim_id;
                self.claim_epoch += 1;
            }
            to Running
            emit ClaimGranted {
                claim_id: self.claim_id,
                claim_epoch: self.claim_epoch,
                took_over: false
            }
        }

        transition ClaimGrantMerging {
            on input Claim { claim_id, lease_expired }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "well_formed_claim" { claim_id != "" }
            guard "unheld" { self.claim_id == "" }
            update {
                self.claim_id = claim_id;
                self.claim_epoch += 1;
            }
            to Merging
            emit ClaimGranted {
                claim_id: self.claim_id,
                claim_epoch: self.claim_epoch,
                took_over: false
            }
        }

        transition ClaimGrantConcluded {
            on input Claim { claim_id, lease_expired }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "well_formed_claim" { claim_id != "" }
            guard "unheld" { self.claim_id == "" }
            update {
                self.claim_id = claim_id;
                self.claim_epoch += 1;
            }
            to Concluded
            emit ClaimGranted {
                claim_id: self.claim_id,
                claim_epoch: self.claim_epoch,
                took_over: false
            }
        }

        transition ClaimGrantCleanupDebt {
            on input Claim { claim_id, lease_expired }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "well_formed_claim" { claim_id != "" }
            guard "unheld" { self.claim_id == "" }
            update {
                self.claim_id = claim_id;
                self.claim_epoch += 1;
            }
            to CleanupDebt
            emit ClaimGranted {
                claim_id: self.claim_id,
                claim_epoch: self.claim_epoch,
                took_over: false
            }
        }

        transition ClaimRenewPreparing {
            on input Claim { claim_id, lease_expired }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "well_formed_claim" { claim_id != "" }
            guard "same_holder" { self.claim_id != "" && claim_id == self.claim_id }
            update {}
            to Preparing
            emit ClaimRenewed { claim_id: self.claim_id, claim_epoch: self.claim_epoch }
        }

        transition ClaimRenewRunning {
            on input Claim { claim_id, lease_expired }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "well_formed_claim" { claim_id != "" }
            guard "same_holder" { self.claim_id != "" && claim_id == self.claim_id }
            update {}
            to Running
            emit ClaimRenewed { claim_id: self.claim_id, claim_epoch: self.claim_epoch }
        }

        transition ClaimRenewMerging {
            on input Claim { claim_id, lease_expired }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "well_formed_claim" { claim_id != "" }
            guard "same_holder" { self.claim_id != "" && claim_id == self.claim_id }
            update {}
            to Merging
            emit ClaimRenewed { claim_id: self.claim_id, claim_epoch: self.claim_epoch }
        }

        transition ClaimRenewConcluded {
            on input Claim { claim_id, lease_expired }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "well_formed_claim" { claim_id != "" }
            guard "same_holder" { self.claim_id != "" && claim_id == self.claim_id }
            update {}
            to Concluded
            emit ClaimRenewed { claim_id: self.claim_id, claim_epoch: self.claim_epoch }
        }

        transition ClaimRenewCleanupDebt {
            on input Claim { claim_id, lease_expired }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "well_formed_claim" { claim_id != "" }
            guard "same_holder" { self.claim_id != "" && claim_id == self.claim_id }
            update {}
            to CleanupDebt
            emit ClaimRenewed { claim_id: self.claim_id, claim_epoch: self.claim_epoch }
        }

        transition ClaimTakeoverPreparing {
            on input Claim { claim_id, lease_expired }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "well_formed_claim" { claim_id != "" }
            guard "other_holder" { self.claim_id != "" && claim_id != self.claim_id }
            guard "lease_observed_expired" { lease_expired }
            update {
                self.claim_id = claim_id;
                self.claim_epoch += 1;
            }
            to Preparing
            emit ClaimGranted {
                claim_id: self.claim_id,
                claim_epoch: self.claim_epoch,
                took_over: true
            }
        }

        transition ClaimTakeoverRunning {
            on input Claim { claim_id, lease_expired }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "well_formed_claim" { claim_id != "" }
            guard "other_holder" { self.claim_id != "" && claim_id != self.claim_id }
            guard "lease_observed_expired" { lease_expired }
            update {
                self.claim_id = claim_id;
                self.claim_epoch += 1;
            }
            to Running
            emit ClaimGranted {
                claim_id: self.claim_id,
                claim_epoch: self.claim_epoch,
                took_over: true
            }
        }

        transition ClaimTakeoverMerging {
            on input Claim { claim_id, lease_expired }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "well_formed_claim" { claim_id != "" }
            guard "other_holder" { self.claim_id != "" && claim_id != self.claim_id }
            guard "lease_observed_expired" { lease_expired }
            update {
                self.claim_id = claim_id;
                self.claim_epoch += 1;
            }
            to Merging
            emit ClaimGranted {
                claim_id: self.claim_id,
                claim_epoch: self.claim_epoch,
                took_over: true
            }
        }

        transition ClaimTakeoverConcluded {
            on input Claim { claim_id, lease_expired }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "well_formed_claim" { claim_id != "" }
            guard "other_holder" { self.claim_id != "" && claim_id != self.claim_id }
            guard "lease_observed_expired" { lease_expired }
            update {
                self.claim_id = claim_id;
                self.claim_epoch += 1;
            }
            to Concluded
            emit ClaimGranted {
                claim_id: self.claim_id,
                claim_epoch: self.claim_epoch,
                took_over: true
            }
        }

        transition ClaimTakeoverCleanupDebt {
            on input Claim { claim_id, lease_expired }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "well_formed_claim" { claim_id != "" }
            guard "other_holder" { self.claim_id != "" && claim_id != self.claim_id }
            guard "lease_observed_expired" { lease_expired }
            update {
                self.claim_id = claim_id;
                self.claim_epoch += 1;
            }
            to CleanupDebt
            emit ClaimGranted {
                claim_id: self.claim_id,
                claim_epoch: self.claim_epoch,
                took_over: true
            }
        }

        transition ClaimBusyPreparing {
            on input Claim { claim_id, lease_expired }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "well_formed_claim" { claim_id != "" }
            guard "other_holder" { self.claim_id != "" && claim_id != self.claim_id }
            guard "lease_not_expired" { lease_expired == false }
            update {}
            to Preparing
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::HeldByAnotherCoordinator,
                current_claim_epoch: self.claim_epoch
            }
        }

        transition ClaimBusyRunning {
            on input Claim { claim_id, lease_expired }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "well_formed_claim" { claim_id != "" }
            guard "other_holder" { self.claim_id != "" && claim_id != self.claim_id }
            guard "lease_not_expired" { lease_expired == false }
            update {}
            to Running
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::HeldByAnotherCoordinator,
                current_claim_epoch: self.claim_epoch
            }
        }

        transition ClaimBusyMerging {
            on input Claim { claim_id, lease_expired }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "well_formed_claim" { claim_id != "" }
            guard "other_holder" { self.claim_id != "" && claim_id != self.claim_id }
            guard "lease_not_expired" { lease_expired == false }
            update {}
            to Merging
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::HeldByAnotherCoordinator,
                current_claim_epoch: self.claim_epoch
            }
        }

        transition ClaimBusyConcluded {
            on input Claim { claim_id, lease_expired }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "well_formed_claim" { claim_id != "" }
            guard "other_holder" { self.claim_id != "" && claim_id != self.claim_id }
            guard "lease_not_expired" { lease_expired == false }
            update {}
            to Concluded
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::HeldByAnotherCoordinator,
                current_claim_epoch: self.claim_epoch
            }
        }

        transition ClaimBusyCleanupDebt {
            on input Claim { claim_id, lease_expired }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "well_formed_claim" { claim_id != "" }
            guard "other_holder" { self.claim_id != "" && claim_id != self.claim_id }
            guard "lease_not_expired" { lease_expired == false }
            update {}
            to CleanupDebt
            emit ClaimDenied {
                reason: TemporaryCouncilClaimDenial::HeldByAnotherCoordinator,
                current_claim_epoch: self.claim_epoch
            }
        }

        // ------------------------------------------------------------------
        // StartDiscussion
        // ------------------------------------------------------------------

        transition StartDiscussionPreparing {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update { self.revision += 1; }
            to Running
            emit DiscussionStarted { revision: self.revision }
        }

        transition StartDiscussionReplay {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Running
            emit DiscussionStartReplayed { revision: self.revision }
        }

        transition StartDiscussionNotOpened {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit AdvanceRejected { reason: TemporaryCouncilAdvanceRejection::NotOpened }
        }

        transition StartDiscussionAlreadyAdvancedMerging {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Merging
            emit AdvanceRejected { reason: TemporaryCouncilAdvanceRejection::AlreadyAdvanced }
        }

        transition StartDiscussionAlreadyAdvancedConcluded {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Concluded
            emit AdvanceRejected { reason: TemporaryCouncilAdvanceRejection::AlreadyAdvanced }
        }

        transition StartDiscussionAlreadyAdvancedCleanupDebt {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to CleanupDebt
            emit AdvanceRejected { reason: TemporaryCouncilAdvanceRejection::AlreadyAdvanced }
        }

        transition StartDiscussionAlreadyAdvancedSettled {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Settled
            emit AdvanceRejected { reason: TemporaryCouncilAdvanceRejection::AlreadyAdvanced }
        }

        // ------------------------------------------------------------------
        // StartMerge
        // A council that never reached a runnable discussion (for example a
        // seating failure) still enters merge, because the explicit merge-back
        // policy must produce its typed `NotAttempted` outcome.
        // ------------------------------------------------------------------

        transition StartMergePreparing {
            on input StartMerge { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update { self.revision += 1; }
            to Merging
            emit MergeStarted { revision: self.revision }
        }

        transition StartMergeRunning {
            on input StartMerge { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update { self.revision += 1; }
            to Merging
            emit MergeStarted { revision: self.revision }
        }

        transition StartMergeReplay {
            on input StartMerge { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Merging
            emit MergeStartReplayed { revision: self.revision }
        }

        transition StartMergeNotOpened {
            on input StartMerge { claim_id, claim_epoch }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit AdvanceRejected { reason: TemporaryCouncilAdvanceRejection::NotOpened }
        }

        transition StartMergeAlreadyAdvancedConcluded {
            on input StartMerge { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Concluded
            emit AdvanceRejected { reason: TemporaryCouncilAdvanceRejection::AlreadyAdvanced }
        }

        transition StartMergeAlreadyAdvancedCleanupDebt {
            on input StartMerge { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to CleanupDebt
            emit AdvanceRejected { reason: TemporaryCouncilAdvanceRejection::AlreadyAdvanced }
        }

        transition StartMergeAlreadyAdvancedSettled {
            on input StartMerge { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Settled
            emit AdvanceRejected { reason: TemporaryCouncilAdvanceRejection::AlreadyAdvanced }
        }

        // ------------------------------------------------------------------
        // SealResult (the coordinator's own terminal)
        // ------------------------------------------------------------------

        transition SealResultMerging {
            on input SealResult { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {
                self.exit_class = TemporaryCouncilExitClass::Executed;
                self.revision += 1;
            }
            to Concluded
            emit ResultSealed { revision: self.revision, exit_class: self.exit_class }
        }

        transition SealResultReplayConcluded {
            on input SealResult { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "already_executed" { self.exit_class == TemporaryCouncilExitClass::Executed }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Concluded
            emit ResultSealReplayed { exit_class: self.exit_class }
        }

        transition SealResultReplayCleanupDebt {
            on input SealResult { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "already_executed" { self.exit_class == TemporaryCouncilExitClass::Executed }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to CleanupDebt
            emit ResultSealReplayed { exit_class: self.exit_class }
        }

        transition SealResultReplaySettled {
            on input SealResult { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "already_executed" { self.exit_class == TemporaryCouncilExitClass::Executed }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Settled
            emit ResultSealReplayed { exit_class: self.exit_class }
        }

        transition SealResultConflictConcluded {
            on input SealResult { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "sealed_under_another_class" { self.exit_class != TemporaryCouncilExitClass::Executed }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Concluded
            emit ResultSealRejected { reason: TemporaryCouncilSealRejection::AlreadySealed }
        }

        transition SealResultConflictCleanupDebt {
            on input SealResult { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "sealed_under_another_class" { self.exit_class != TemporaryCouncilExitClass::Executed }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to CleanupDebt
            emit ResultSealRejected { reason: TemporaryCouncilSealRejection::AlreadySealed }
        }

        transition SealResultConflictSettled {
            on input SealResult { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "sealed_under_another_class" { self.exit_class != TemporaryCouncilExitClass::Executed }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Settled
            emit ResultSealRejected { reason: TemporaryCouncilSealRejection::AlreadySealed }
        }

        transition SealResultNotOpened {
            on input SealResult { claim_id, claim_epoch }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit ResultSealRejected { reason: TemporaryCouncilSealRejection::NotOpened }
        }

        transition SealResultNotMergingPreparing {
            on input SealResult { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Preparing
            emit ResultSealRejected { reason: TemporaryCouncilSealRejection::NotMerging }
        }

        transition SealResultNotMergingRunning {
            on input SealResult { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Running
            emit ResultSealRejected { reason: TemporaryCouncilSealRejection::NotMerging }
        }

        // ------------------------------------------------------------------
        // SealInterruptedResult (recovery's terminal for a dead coordinator)
        // Legal from every unsealed, bound phase: a crash can happen anywhere.
        // ------------------------------------------------------------------

        transition SealInterruptedPreparing {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {
                self.exit_class = TemporaryCouncilExitClass::CoordinatorInterrupted;
                self.revision += 1;
            }
            to Concluded
            emit ResultSealed { revision: self.revision, exit_class: self.exit_class }
        }

        transition SealInterruptedRunning {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {
                self.exit_class = TemporaryCouncilExitClass::CoordinatorInterrupted;
                self.revision += 1;
            }
            to Concluded
            emit ResultSealed { revision: self.revision, exit_class: self.exit_class }
        }

        transition SealInterruptedMerging {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {
                self.exit_class = TemporaryCouncilExitClass::CoordinatorInterrupted;
                self.revision += 1;
            }
            to Concluded
            emit ResultSealed { revision: self.revision, exit_class: self.exit_class }
        }

        transition SealInterruptedReplayConcluded {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "already_interrupted" { self.exit_class == TemporaryCouncilExitClass::CoordinatorInterrupted }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Concluded
            emit ResultSealReplayed { exit_class: self.exit_class }
        }

        transition SealInterruptedReplayCleanupDebt {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "already_interrupted" { self.exit_class == TemporaryCouncilExitClass::CoordinatorInterrupted }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to CleanupDebt
            emit ResultSealReplayed { exit_class: self.exit_class }
        }

        transition SealInterruptedReplaySettled {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "already_interrupted" { self.exit_class == TemporaryCouncilExitClass::CoordinatorInterrupted }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Settled
            emit ResultSealReplayed { exit_class: self.exit_class }
        }

        transition SealInterruptedConflictConcluded {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "sealed_under_another_class" { self.exit_class != TemporaryCouncilExitClass::CoordinatorInterrupted }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Concluded
            emit ResultSealRejected { reason: TemporaryCouncilSealRejection::AlreadySealed }
        }

        transition SealInterruptedConflictCleanupDebt {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "sealed_under_another_class" { self.exit_class != TemporaryCouncilExitClass::CoordinatorInterrupted }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to CleanupDebt
            emit ResultSealRejected { reason: TemporaryCouncilSealRejection::AlreadySealed }
        }

        transition SealInterruptedConflictSettled {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "sealed_under_another_class" { self.exit_class != TemporaryCouncilExitClass::CoordinatorInterrupted }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Settled
            emit ResultSealRejected { reason: TemporaryCouncilSealRejection::AlreadySealed }
        }

        transition SealInterruptedNotOpened {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit ResultSealRejected { reason: TemporaryCouncilSealRejection::NotOpened }
        }

        // ------------------------------------------------------------------
        // Cleanup outcomes
        // ------------------------------------------------------------------

        transition RecordCleanupSettledConcluded {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {
                self.cleanup_attempts += 1;
                self.revision += 1;
            }
            to Settled
            emit CleanupSettled { revision: self.revision, attempts: self.cleanup_attempts }
        }

        transition RecordCleanupSettledAfterDebt {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {
                self.cleanup_attempts += 1;
                self.revision += 1;
            }
            to Settled
            emit CleanupSettled { revision: self.revision, attempts: self.cleanup_attempts }
        }

        transition RecordCleanupSettledReplay {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Settled
            emit CleanupSettlementReplayed { attempts: self.cleanup_attempts }
        }

        transition RecordCleanupSettledNotSealedEmpty {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit CleanupRejected { reason: TemporaryCouncilCleanupRejection::ResultNotSealed }
        }

        transition RecordCleanupSettledNotSealedPreparing {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Preparing
            emit CleanupRejected { reason: TemporaryCouncilCleanupRejection::ResultNotSealed }
        }

        transition RecordCleanupSettledNotSealedRunning {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Running
            emit CleanupRejected { reason: TemporaryCouncilCleanupRejection::ResultNotSealed }
        }

        transition RecordCleanupSettledNotSealedMerging {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Merging
            emit CleanupRejected { reason: TemporaryCouncilCleanupRejection::ResultNotSealed }
        }

        transition RecordCleanupDebtConcluded {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {
                self.cleanup_attempts += 1;
                self.revision += 1;
            }
            to CleanupDebt
            emit CleanupDebtRecorded { revision: self.revision, attempts: self.cleanup_attempts }
        }

        transition RecordCleanupDebtRetry {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {
                self.cleanup_attempts += 1;
                self.revision += 1;
            }
            to CleanupDebt
            emit CleanupDebtRecorded { revision: self.revision, attempts: self.cleanup_attempts }
        }

        transition RecordCleanupDebtAlreadySettled {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Settled
            emit CleanupRejected { reason: TemporaryCouncilCleanupRejection::AlreadySettled }
        }

        transition RecordCleanupDebtNotSealedEmpty {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit CleanupRejected { reason: TemporaryCouncilCleanupRejection::ResultNotSealed }
        }

        transition RecordCleanupDebtNotSealedPreparing {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Preparing
            emit CleanupRejected { reason: TemporaryCouncilCleanupRejection::ResultNotSealed }
        }

        transition RecordCleanupDebtNotSealedRunning {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Running
            emit CleanupRejected { reason: TemporaryCouncilCleanupRejection::ResultNotSealed }
        }

        transition RecordCleanupDebtNotSealedMerging {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "claim_matches" { claim_id == self.claim_id && claim_epoch == self.claim_epoch }
            update {}
            to Merging
            emit CleanupRejected { reason: TemporaryCouncilCleanupRejection::ResultNotSealed }
        }

        transition StartDiscussionFencedPreparing {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Preparing
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition StartDiscussionFencedRunning {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Running
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition StartDiscussionFencedMerging {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Merging
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition StartDiscussionFencedConcluded {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Concluded
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition StartDiscussionFencedCleanupDebt {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to CleanupDebt
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition StartDiscussionFencedSettled {
            on input StartDiscussion { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Settled
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition StartMergeFencedPreparing {
            on input StartMerge { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Preparing
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition StartMergeFencedRunning {
            on input StartMerge { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Running
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition StartMergeFencedMerging {
            on input StartMerge { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Merging
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition StartMergeFencedConcluded {
            on input StartMerge { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Concluded
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition StartMergeFencedCleanupDebt {
            on input StartMerge { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to CleanupDebt
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition StartMergeFencedSettled {
            on input StartMerge { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Settled
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealResultFencedPreparing {
            on input SealResult { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Preparing
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealResultFencedRunning {
            on input SealResult { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Running
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealResultFencedMerging {
            on input SealResult { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Merging
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealResultFencedConcluded {
            on input SealResult { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Concluded
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealResultFencedCleanupDebt {
            on input SealResult { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to CleanupDebt
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealResultFencedSettled {
            on input SealResult { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Settled
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealInterruptedResultFencedPreparing {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Preparing
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealInterruptedResultFencedRunning {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Running
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealInterruptedResultFencedMerging {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Merging
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealInterruptedResultFencedConcluded {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Concluded
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealInterruptedResultFencedCleanupDebt {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to CleanupDebt
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition SealInterruptedResultFencedSettled {
            on input SealInterruptedResult { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Settled
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupSettledFencedPreparing {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Preparing
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupSettledFencedRunning {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Running
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupSettledFencedMerging {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Merging
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupSettledFencedConcluded {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Concluded
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupSettledFencedCleanupDebt {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to CleanupDebt
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupSettledFencedSettled {
            on input RecordCleanupSettled { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Settled
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupDebtFencedPreparing {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Preparing
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupDebtFencedRunning {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "running" { self.lifecycle_phase == Phase::Running }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Running
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupDebtFencedMerging {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Merging
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupDebtFencedConcluded {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Concluded
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupDebtFencedCleanupDebt {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to CleanupDebt
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        transition RecordCleanupDebtFencedSettled {
            on input RecordCleanupDebt { claim_id, claim_epoch }
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            guard "stale_claim" { claim_id != self.claim_id || claim_epoch != self.claim_epoch }
            update {}
            to Settled
            emit CommandFenced { current_claim_epoch: self.claim_epoch }
        }

        // ------------------------------------------------------------------
        // Recovery classification
        // One self-loop per phase, each emitting the exact verdict for that
        // phase. The shell mirrors it and owns no phase predicate of its own.
        // ------------------------------------------------------------------

        transition ClassifyRecoveryEmpty {
            on input ClassifyRecovery {}
            guard "empty" { self.lifecycle_phase == Phase::Empty }
            update {}
            to Empty
            emit RecoveryClassified { unfinished: true, result_sealed: false, needs_cleanup: true }
        }

        transition ClassifyRecoveryPreparing {
            on input ClassifyRecovery {}
            guard "preparing" { self.lifecycle_phase == Phase::Preparing }
            update {}
            to Preparing
            emit RecoveryClassified { unfinished: true, result_sealed: false, needs_cleanup: true }
        }

        transition ClassifyRecoveryRunning {
            on input ClassifyRecovery {}
            guard "running" { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit RecoveryClassified { unfinished: true, result_sealed: false, needs_cleanup: true }
        }

        transition ClassifyRecoveryMerging {
            on input ClassifyRecovery {}
            guard "merging" { self.lifecycle_phase == Phase::Merging }
            update {}
            to Merging
            emit RecoveryClassified { unfinished: true, result_sealed: false, needs_cleanup: true }
        }

        transition ClassifyRecoveryConcluded {
            on input ClassifyRecovery {}
            guard "concluded" { self.lifecycle_phase == Phase::Concluded }
            update {}
            to Concluded
            emit RecoveryClassified { unfinished: true, result_sealed: true, needs_cleanup: true }
        }

        transition ClassifyRecoveryCleanupDebt {
            on input ClassifyRecovery {}
            guard "cleanup_debt" { self.lifecycle_phase == Phase::CleanupDebt }
            update {}
            to CleanupDebt
            emit RecoveryClassified { unfinished: true, result_sealed: true, needs_cleanup: true }
        }

        transition ClassifyRecoverySettled {
            on input ClassifyRecovery {}
            guard "settled" { self.lifecycle_phase == Phase::Settled }
            update {}
            to Settled
            emit RecoveryClassified { unfinished: false, result_sealed: true, needs_cleanup: false }
        }
    }
}

impl serde::Serialize for TemporaryCouncilLifecycleState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Empty => "empty",
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Merging => "merging",
            Self::Concluded => "concluded",
            Self::CleanupDebt => "cleanup_debt",
            Self::Settled => "settled",
        })
    }
}

impl<'de> serde::Deserialize<'de> for TemporaryCouncilLifecycleState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        match value.as_str() {
            "empty" => Ok(Self::Empty),
            "preparing" => Ok(Self::Preparing),
            "running" => Ok(Self::Running),
            "merging" => Ok(Self::Merging),
            "concluded" => Ok(Self::Concluded),
            "cleanup_debt" => Ok(Self::CleanupDebt),
            "settled" => Ok(Self::Settled),
            other => Err(serde::de::Error::custom(format!(
                "invalid TemporaryCouncilLifecycleState `{other}`"
            ))),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct TemporaryCouncilLifecycleMachineStateWire {
    lifecycle_phase: TemporaryCouncilLifecycleState,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    request_fingerprint: String,
    #[serde(default)]
    exit_class: TemporaryCouncilExitClass,
    #[serde(default)]
    cleanup_attempts: u64,
    #[serde(default)]
    claim_id: String,
    #[serde(default)]
    claim_epoch: u64,
}

impl From<&TemporaryCouncilLifecycleMachineState> for TemporaryCouncilLifecycleMachineStateWire {
    fn from(state: &TemporaryCouncilLifecycleMachineState) -> Self {
        Self {
            lifecycle_phase: state.lifecycle_phase,
            revision: state.revision,
            request_fingerprint: state.request_fingerprint.clone(),
            exit_class: state.exit_class,
            cleanup_attempts: state.cleanup_attempts,
            claim_id: state.claim_id.clone(),
            claim_epoch: state.claim_epoch,
        }
    }
}

impl From<TemporaryCouncilLifecycleMachineStateWire> for TemporaryCouncilLifecycleMachineState {
    fn from(wire: TemporaryCouncilLifecycleMachineStateWire) -> Self {
        Self {
            lifecycle_phase: wire.lifecycle_phase,
            revision: wire.revision,
            request_fingerprint: wire.request_fingerprint,
            exit_class: wire.exit_class,
            cleanup_attempts: wire.cleanup_attempts,
            claim_id: wire.claim_id,
            claim_epoch: wire.claim_epoch,
        }
    }
}

impl serde::Serialize for TemporaryCouncilLifecycleMachineState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        TemporaryCouncilLifecycleMachineStateWire::from(self).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for TemporaryCouncilLifecycleMachineState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        TemporaryCouncilLifecycleMachineStateWire::deserialize(deserializer).map(Self::from)
    }
}
