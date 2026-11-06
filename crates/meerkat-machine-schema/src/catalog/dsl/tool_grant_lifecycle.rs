//! Canonical lifecycle authority for ONE scoped tool grant.
//!
//! A scoped tool grant opens tool names a session build marked grant-gated,
//! for a bounded window. This machine owns the grant's own lifecycle:
//! issuance against a maximum lifetime, expiry at its deadline, revocation,
//! the end of the run a run-scoped grant was issued for, and the admission of
//! each use.
//!
//! # Scope discipline
//!
//! Record-scoped: the store persists one machine state per grant, next to the
//! grant's immutable descriptor (subject, target, issuer, reason). The machine
//! never interprets the descriptor; its owner service does.
//!
//! The machine deliberately does NOT own:
//!
//! - **Tool visibility.** `MeerkatMachine` stays the only owner of what a
//!   session can see. Its grant gate layer receives the open names an
//!   adjudication produced; it never reads a grant phase.
//! - **Time.** It reads no clock. Every input carries the host's `now_ms`
//!   observation, and the guards compare it with the grant's fixed
//!   `expires_at_ms`.
//! - **Runs.** It holds the key of the run a run-scoped grant was issued for
//!   and compares the run each use and each run end names with it. Which run
//!   a session runs is `MeerkatMachine`'s.
//!
//! # Run scope
//!
//! A grant issued with a run scope admits uses of that run only. A session
//! has one current run, so a use from another run proves the grant's run is
//! over: that use ends the grant, as the end of its run does. A use outside
//! any run (a host call) is refused and ends nothing. The window still
//! applies: a run-scoped grant expires at its deadline whatever its run does.
//!
//! # Latching
//!
//! `Expired`, `Revoked` and `RunEnded` are terminal. Once a grant reaches
//! one, no input moves it again, so a restart or a clock rollback can never
//! revive it. A revoke or a run end that arrives at or after the deadline
//! records expiry, so a `Revoked` or `RunEnded` grant always ended before its
//! deadline.
//!
//! # Totality discipline
//!
//! Every input has an explicit transition in every issued phase. `Expire`,
//! `Revoke`, `AdmitUse` and `EndRun` against an `Unissued` record are refused
//! by the generated kernel: their arms' phase guards fail there, so it
//! answers `GuardRejected`. The owner service creates a row only through
//! `Issue`, so it never sends them.

use super::OptionValueExt;

/// Canonical data declarations shared by every host instantiation of the
/// machine. These are closed vocabularies and an opaque key; they carry no
/// authority.
pub mod types {
    /// Why an issued grant ended.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum ToolGrantEndReason {
        /// The grant reached its deadline.
        #[default]
        Deadline,
        /// The grant was revoked before its deadline.
        Revoked,
        /// The run the grant was issued for ended before its deadline.
        RunEnded,
    }

    /// Why a use of a grant was refused.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum ToolGrantUseRefusal {
        /// The grant reached its deadline.
        #[default]
        Expired,
        /// The grant was revoked.
        Revoked,
        /// The run the grant was issued for ended.
        RunEnded,
        /// The grant is scoped to a run, and the use belongs to no run.
        OutsideRun,
    }

    /// Why an `Issue` input was refused.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum ToolGrantIssueRefusal {
        /// The window is already past, empty, or longer than the maximum
        /// lifetime.
        #[default]
        InvalidWindow,
        /// The record already carries an issued grant.
        AlreadyIssued,
    }

    /// An opaque run key: the run a run-scoped grant was issued for, or the
    /// run a use or a run end names. The machine only compares keys for
    /// equality.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct ToolGrantRunId(pub String);

    impl<T: Into<String>> From<T> for ToolGrantRunId {
        fn from(value: T) -> Self {
            Self(value.into())
        }
    }
}

#[macro_export]
macro_rules! tool_grant_lifecycle_catalog_machine_dsl {
    ($rust_crate:literal, $rust_module:literal) => {
        pub use $crate::catalog::dsl::tool_grant_lifecycle::types::{
            ToolGrantEndReason, ToolGrantIssueRefusal, ToolGrantRunId, ToolGrantUseRefusal,
        };

        meerkat_machine_dsl::machine! {
            machine ToolGrantLifecycleMachine {
                version: 2,
                rust: $rust_crate / $rust_module,

                state {
                    lifecycle_phase: ToolGrantPhase,
                    // Service-minted grant identity. Empty only while Unissued.
                    grant_id: String,
                    // Host time the grant was issued at.
                    issued_at_ms: u64,
                    // Fixed absolute deadline. Never moves once issued.
                    expires_at_ms: u64,
                    // Host time the grant was observed to end.
                    ended_at_ms: Option<u64>,
                    // Why the grant ended.
                    end_reason: Option<Enum<ToolGrantEndReason>>,
                    // The run a run-scoped grant was issued for. Set only by
                    // Issue, never moved; absent for a grant bounded by its
                    // window only.
                    run_scope: Option<ToolGrantRunId>,
                }

                init(Unissued) {
                    grant_id = "",
                    issued_at_ms = 0,
                    expires_at_ms = 0,
                    ended_at_ms = None,
                    end_reason = None,
                    run_scope = None,
                }

                terminal [Expired, Revoked, RunEnded]

                phase ToolGrantPhase {
                    // No grant issued on this record yet.
                    Unissued,
                    // Issued, and not yet observed past its deadline.
                    Active,
                    // Observed at or after its deadline. Terminal.
                    Expired,
                    // Revoked before its deadline. Terminal.
                    Revoked,
                    // The run it was issued for ended before its deadline.
                    // Terminal.
                    RunEnded,
                }

                input ToolGrantLifecycleInput {
                    // Issue the grant for the window `now_ms..expires_at_ms`,
                    // which must be non-empty and at most `max_lifetime_ms`,
                    // for the run `run_scope` when one is named.
                    Issue {
                        grant_id: String,
                        now_ms: u64,
                        expires_at_ms: u64,
                        max_lifetime_ms: u64,
                        run_scope: Option<ToolGrantRunId>
                    },
                    // A deadline driver observed the clock.
                    Expire { now_ms: u64 },
                    // An operator revokes the grant.
                    Revoke { now_ms: u64 },
                    // One use asks for admission at `now_ms`, from the run
                    // `run_id`, or from no run.
                    AdmitUse { now_ms: u64, run_id: Option<ToolGrantRunId> },
                    // The run `run_id` of the grant's subject ended.
                    EndRun { run_id: ToolGrantRunId, now_ms: u64 },
                }

                effect ToolGrantLifecycleEffect {
                    Issued { grant_id: String, expires_at_ms: u64 },
                    Expired { grant_id: String, expires_at_ms: u64, observed_at_ms: u64 },
                    Revoked { grant_id: String, observed_at_ms: u64 },
                    UseAdmitted { grant_id: String },
                    UseRefused { grant_id: String, reason: Enum<ToolGrantUseRefusal> },
                    IssueRefused { reason: Enum<ToolGrantIssueRefusal> },
                    RunEnded { grant_id: String, observed_at_ms: u64 },
                }

                // An unissued record carries no grant facts at all.
                invariant unissued_is_empty {
                    self.lifecycle_phase != Phase::Unissued
                        || (
                            self.grant_id == ""
                            && self.issued_at_ms == 0
                            && self.expires_at_ms == 0
                            && self.ended_at_ms == None
                            && self.end_reason == None
                        )
                }

                // Every issued record names its grant and a non-empty window.
                invariant issued_window_is_nonempty {
                    self.lifecycle_phase == Phase::Unissued
                        || (self.grant_id != "" && self.issued_at_ms < self.expires_at_ms)
                }

                // A live grant has not ended.
                invariant active_has_no_end {
                    self.lifecycle_phase != Phase::Active
                        || (self.ended_at_ms == None && self.end_reason == None)
                }

                // An ended grant records when and why it ended.
                invariant ended_grants_record_their_end {
                    (
                        self.lifecycle_phase != Phase::Expired
                        && self.lifecycle_phase != Phase::Revoked
                        && self.lifecycle_phase != Phase::RunEnded
                    )
                    || (self.ended_at_ms != None && self.end_reason != None)
                }

                // Expiry is observed at or after the deadline, never before.
                invariant expiry_never_precedes_its_deadline {
                    self.lifecycle_phase != Phase::Expired
                        || (
                            self.end_reason == Some(ToolGrantEndReason::Deadline)
                            && self.ended_at_ms != None
                            && self.ended_at_ms.get("value") >= self.expires_at_ms
                        )
                }

                // A revocation ends the grant before its deadline; a revoke at
                // or after the deadline records expiry instead.
                invariant revocation_precedes_the_deadline {
                    self.lifecycle_phase != Phase::Revoked
                        || (
                            self.end_reason == Some(ToolGrantEndReason::Revoked)
                            && self.ended_at_ms != None
                            && self.ended_at_ms.get("value") < self.expires_at_ms
                        )
                }

                // A run end ends a run-scoped grant before its deadline; a run
                // end at or after the deadline records expiry instead.
                invariant run_ended_records_its_end {
                    self.lifecycle_phase != Phase::RunEnded
                        || (
                            self.end_reason == Some(ToolGrantEndReason::RunEnded)
                            && self.ended_at_ms != None
                            && self.ended_at_ms.get("value") < self.expires_at_ms
                            && self.run_scope != None
                        )
                }

                // Only Issue sets a run scope: an unissued record has none.
                invariant run_scope_set_only_by_issue {
                    self.lifecycle_phase != Phase::Unissued || self.run_scope == None
                }

                // Lifecycle facts: the owner service persists the row and its
                // audit rows in one commit.
                disposition Issued => local seam OwnerRealizationOnly,
                disposition Expired => local seam OwnerRealizationOnly,
                disposition Revoked => local seam OwnerRealizationOnly,
                disposition RunEnded => local seam OwnerRealizationOnly,
                // Verdicts: the calling surface mirrors them as its typed
                // result.
                disposition UseAdmitted => local seam SurfaceResultAlignment,
                disposition UseRefused => local seam SurfaceResultAlignment,
                disposition IssueRefused => local seam SurfaceResultAlignment,

                // ----------------------------------------------------------
                // Issue
                // ----------------------------------------------------------

                transition Issue {
                    on input Issue { grant_id, now_ms, expires_at_ms, max_lifetime_ms, run_scope }
                    guard "unissued" { self.lifecycle_phase == Phase::Unissued }
                    guard "named_grant" { grant_id != "" }
                    guard "window_is_not_past_or_empty" { now_ms < expires_at_ms }
                    guard "window_within_maximum_lifetime" { expires_at_ms - now_ms <= max_lifetime_ms }
                    update {
                        self.grant_id = grant_id;
                        self.issued_at_ms = now_ms;
                        self.expires_at_ms = expires_at_ms;
                        self.run_scope = run_scope;
                    }
                    to Active
                    emit Issued { grant_id: self.grant_id, expires_at_ms: self.expires_at_ms }
                }

                transition IssueRefusedWindow {
                    on input Issue { grant_id, now_ms, expires_at_ms, max_lifetime_ms, run_scope }
                    guard "unissued" { self.lifecycle_phase == Phase::Unissued }
                    guard "window_is_past_empty_or_overlong" {
                        expires_at_ms <= now_ms || expires_at_ms - now_ms > max_lifetime_ms
                    }
                    update {}
                    to Unissued
                    emit IssueRefused { reason: ToolGrantIssueRefusal::InvalidWindow }
                }

                transition IssueRefusedIssued {
                    per_phase [Active, Expired, Revoked]
                    on input Issue { grant_id, now_ms, expires_at_ms, max_lifetime_ms, run_scope }
                    update {}
                    to Active
                    emit IssueRefused { reason: ToolGrantIssueRefusal::AlreadyIssued }
                }

                // ----------------------------------------------------------
                // Expire
                // ----------------------------------------------------------

                transition ExpireDue {
                    on input Expire { now_ms }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "deadline_reached" { self.expires_at_ms <= now_ms }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.end_reason = Some(ToolGrantEndReason::Deadline);
                    }
                    to Expired
                    emit Expired {
                        grant_id: self.grant_id,
                        expires_at_ms: self.expires_at_ms,
                        observed_at_ms: now_ms
                    }
                }

                // An early timer fire: nothing changes and nothing is emitted.
                transition ExpireNotDue {
                    on input Expire { now_ms }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "deadline_not_reached" { now_ms < self.expires_at_ms }
                    update {}
                    to Active
                }

                transition ExpireSettled {
                    per_phase [Expired, Revoked]
                    on input Expire { now_ms }
                    update {}
                    to Expired
                }

                // ----------------------------------------------------------
                // Revoke
                // ----------------------------------------------------------

                transition RevokeActive {
                    on input Revoke { now_ms }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "deadline_not_reached" { now_ms < self.expires_at_ms }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.end_reason = Some(ToolGrantEndReason::Revoked);
                    }
                    to Revoked
                    emit Revoked { grant_id: self.grant_id, observed_at_ms: now_ms }
                }

                // A revoke at or after the deadline records the expiry the
                // grant already reached.
                transition RevokeAfterDeadline {
                    on input Revoke { now_ms }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "deadline_reached" { self.expires_at_ms <= now_ms }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.end_reason = Some(ToolGrantEndReason::Deadline);
                    }
                    to Expired
                    emit Expired {
                        grant_id: self.grant_id,
                        expires_at_ms: self.expires_at_ms,
                        observed_at_ms: now_ms
                    }
                }

                transition RevokeSettled {
                    per_phase [Expired, Revoked]
                    on input Revoke { now_ms }
                    update {}
                    to Expired
                }

                // ----------------------------------------------------------
                // AdmitUse
                // ----------------------------------------------------------

                // A live grant admits a use of its own run, or any use when
                // it is bounded by its window only. A use of its own run
                // names a run (`run_id != None`): stated, so the arm is
                // structurally exclusive with `AdmitOutsideRun`.
                transition AdmitLive {
                    on input AdmitUse { now_ms, run_id }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "deadline_not_reached" { now_ms < self.expires_at_ms }
                    guard "use_within_scope" {
                        self.run_scope == None
                            || (run_id != None && self.run_scope == run_id)
                    }
                    update {}
                    to Active
                    emit UseAdmitted { grant_id: self.grant_id }
                }

                // The first use at or after the deadline observes the expiry
                // and is refused.
                transition AdmitDue {
                    on input AdmitUse { now_ms, run_id }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "deadline_reached" { self.expires_at_ms <= now_ms }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.end_reason = Some(ToolGrantEndReason::Deadline);
                    }
                    to Expired
                    emit Expired {
                        grant_id: self.grant_id,
                        expires_at_ms: self.expires_at_ms,
                        observed_at_ms: now_ms
                    }
                    emit UseRefused { grant_id: self.grant_id, reason: ToolGrantUseRefusal::Expired }
                }

                transition AdmitEndedExpired {
                    on input AdmitUse { now_ms, run_id }
                    guard "expired" { self.lifecycle_phase == Phase::Expired }
                    update {}
                    to Expired
                    emit UseRefused { grant_id: self.grant_id, reason: ToolGrantUseRefusal::Expired }
                }

                transition AdmitEndedRevoked {
                    on input AdmitUse { now_ms, run_id }
                    guard "revoked" { self.lifecycle_phase == Phase::Revoked }
                    update {}
                    to Revoked
                    emit UseRefused { grant_id: self.grant_id, reason: ToolGrantUseRefusal::Revoked }
                }

                // Every arm below was added by version 2 (run scope). They
                // are declared after every arm version 1 released, so the
                // generated transition ids of version 1 keep their order.

                transition AdmitEndedRunEnded {
                    on input AdmitUse { now_ms, run_id }
                    guard "run_ended" { self.lifecycle_phase == Phase::RunEnded }
                    update {}
                    to RunEnded
                    emit UseRefused { grant_id: self.grant_id, reason: ToolGrantUseRefusal::RunEnded }
                }

                // A use from another run before the deadline: a session has
                // one current run, so the grant's run is over. The use ends
                // the grant, and is refused.
                transition AdmitOtherRun {
                    on input AdmitUse { now_ms, run_id }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "deadline_not_reached" { now_ms < self.expires_at_ms }
                    guard "use_from_another_run" {
                        self.run_scope != None && run_id != None && self.run_scope != run_id
                    }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.end_reason = Some(ToolGrantEndReason::RunEnded);
                    }
                    to RunEnded
                    emit RunEnded { grant_id: self.grant_id, observed_at_ms: now_ms }
                    emit UseRefused { grant_id: self.grant_id, reason: ToolGrantUseRefusal::RunEnded }
                }

                // A use of a run-scoped grant from no run (a host call) is
                // refused, and ends nothing: the grant's run may go on.
                transition AdmitOutsideRun {
                    on input AdmitUse { now_ms, run_id }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "deadline_not_reached" { now_ms < self.expires_at_ms }
                    guard "use_outside_any_run" { self.run_scope != None && run_id == None }
                    update {}
                    to Active
                    emit UseRefused { grant_id: self.grant_id, reason: ToolGrantUseRefusal::OutsideRun }
                }

                // ----------------------------------------------------------
                // EndRun
                // ----------------------------------------------------------

                // The grant's own run ended before its deadline.
                transition EndRunMatching {
                    on input EndRun { run_id, now_ms }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "deadline_not_reached" { now_ms < self.expires_at_ms }
                    guard "own_run" { self.run_scope == Some(run_id) }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.end_reason = Some(ToolGrantEndReason::RunEnded);
                    }
                    to RunEnded
                    emit RunEnded { grant_id: self.grant_id, observed_at_ms: now_ms }
                }

                // The grant's own run ended at or after its deadline: the run
                // end records the expiry the grant already reached.
                transition EndRunAfterDeadline {
                    on input EndRun { run_id, now_ms }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "deadline_reached" { self.expires_at_ms <= now_ms }
                    guard "own_run" { self.run_scope == Some(run_id) }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.end_reason = Some(ToolGrantEndReason::Deadline);
                    }
                    to Expired
                    emit Expired {
                        grant_id: self.grant_id,
                        expires_at_ms: self.expires_at_ms,
                        observed_at_ms: now_ms
                    }
                }

                // Another run ended, or the grant is bounded by its window
                // only: nothing changes and nothing is emitted.
                transition EndRunOther {
                    on input EndRun { run_id, now_ms }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "another_run" { self.run_scope != Some(run_id) }
                    update {}
                    to Active
                }

                transition EndRunSettled {
                    per_phase [Expired, Revoked, RunEnded]
                    on input EndRun { run_id, now_ms }
                    update {}
                    to Expired
                }

                // The other inputs of a run-ended grant settle without a
                // change, as they do for the other ended phases.

                transition IssueRefusedIssuedRunEnded {
                    on input Issue { grant_id, now_ms, expires_at_ms, max_lifetime_ms, run_scope }
                    guard "run_ended" { self.lifecycle_phase == Phase::RunEnded }
                    update {}
                    to RunEnded
                    emit IssueRefused { reason: ToolGrantIssueRefusal::AlreadyIssued }
                }

                transition ExpireSettledRunEnded {
                    on input Expire { now_ms }
                    guard "run_ended" { self.lifecycle_phase == Phase::RunEnded }
                    update {}
                    to RunEnded
                }

                transition RevokeSettledRunEnded {
                    on input Revoke { now_ms }
                    guard "run_ended" { self.lifecycle_phase == Phase::RunEnded }
                    update {}
                    to RunEnded
                }
            }
        }
    };
}

crate::tool_grant_lifecycle_catalog_machine_dsl!("self", "catalog::dsl::tool_grant_lifecycle");
