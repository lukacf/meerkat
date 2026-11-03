//! Canonical lifecycle authority for ONE scoped tool grant.
//!
//! A scoped tool grant opens tool names a session build marked grant-gated,
//! for a bounded window. This machine owns the grant's own lifecycle:
//! issuance against a maximum lifetime, expiry at its deadline, revocation,
//! and the admission of each use.
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
//!
//! # Latching
//!
//! `Expired` and `Revoked` are terminal. Once a grant reaches either, no
//! input moves it again, so a restart or a clock rollback can never revive
//! it. A revoke that arrives at or after the deadline records expiry, so a
//! `Revoked` grant always ended before its deadline.
//!
//! # Totality discipline
//!
//! Every input has an explicit transition in every issued phase. `Expire`,
//! `Revoke` and `AdmitUse` against an `Unissued` record are refused by the
//! generated kernel: their arms' phase guards fail there, so it answers
//! `GuardRejected`. The owner service creates a row only through `Issue`, so
//! it never sends them.

use super::OptionValueExt;

/// Canonical data declarations shared by every host instantiation of the
/// machine. These are closed vocabularies; they carry no authority.
pub mod types {
    /// Why an issued grant ended.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum ToolGrantEndReason {
        /// The grant reached its deadline.
        #[default]
        Deadline,
        /// The grant was revoked before its deadline.
        Revoked,
    }

    /// Why a use of a grant was refused.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum ToolGrantUseRefusal {
        /// The grant reached its deadline.
        #[default]
        Expired,
        /// The grant was revoked.
        Revoked,
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
}

#[macro_export]
macro_rules! tool_grant_lifecycle_catalog_machine_dsl {
    ($rust_crate:literal, $rust_module:literal) => {
        pub use $crate::catalog::dsl::tool_grant_lifecycle::types::{
            ToolGrantEndReason, ToolGrantIssueRefusal, ToolGrantUseRefusal,
        };

        meerkat_machine_dsl::machine! {
            machine ToolGrantLifecycleMachine {
                version: 1,
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
                }

                init(Unissued) {
                    grant_id = "",
                    issued_at_ms = 0,
                    expires_at_ms = 0,
                    ended_at_ms = None,
                    end_reason = None,
                }

                terminal [Expired, Revoked]

                phase ToolGrantPhase {
                    // No grant issued on this record yet.
                    Unissued,
                    // Issued, and not yet observed past its deadline.
                    Active,
                    // Observed at or after its deadline. Terminal.
                    Expired,
                    // Revoked before its deadline. Terminal.
                    Revoked,
                }

                input ToolGrantLifecycleInput {
                    // Issue the grant for the window `now_ms..expires_at_ms`,
                    // which must be non-empty and at most `max_lifetime_ms`.
                    Issue { grant_id: String, now_ms: u64, expires_at_ms: u64, max_lifetime_ms: u64 },
                    // A deadline driver observed the clock.
                    Expire { now_ms: u64 },
                    // An operator revokes the grant.
                    Revoke { now_ms: u64 },
                    // One use asks for admission at `now_ms`.
                    AdmitUse { now_ms: u64 },
                }

                effect ToolGrantLifecycleEffect {
                    Issued { grant_id: String, expires_at_ms: u64 },
                    Expired { grant_id: String, expires_at_ms: u64, observed_at_ms: u64 },
                    Revoked { grant_id: String, observed_at_ms: u64 },
                    UseAdmitted { grant_id: String },
                    UseRefused { grant_id: String, reason: Enum<ToolGrantUseRefusal> },
                    IssueRefused { reason: Enum<ToolGrantIssueRefusal> },
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

                // Lifecycle facts: the owner service persists the row and its
                // audit rows in one commit.
                disposition Issued => local seam OwnerRealizationOnly,
                disposition Expired => local seam OwnerRealizationOnly,
                disposition Revoked => local seam OwnerRealizationOnly,
                // Verdicts: the calling surface mirrors them as its typed
                // result.
                disposition UseAdmitted => local seam SurfaceResultAlignment,
                disposition UseRefused => local seam SurfaceResultAlignment,
                disposition IssueRefused => local seam SurfaceResultAlignment,

                // ----------------------------------------------------------
                // Issue
                // ----------------------------------------------------------

                transition Issue {
                    on input Issue { grant_id, now_ms, expires_at_ms, max_lifetime_ms }
                    guard "unissued" { self.lifecycle_phase == Phase::Unissued }
                    guard "named_grant" { grant_id != "" }
                    guard "window_is_not_past_or_empty" { now_ms < expires_at_ms }
                    guard "window_within_maximum_lifetime" { expires_at_ms - now_ms <= max_lifetime_ms }
                    update {
                        self.grant_id = grant_id;
                        self.issued_at_ms = now_ms;
                        self.expires_at_ms = expires_at_ms;
                    }
                    to Active
                    emit Issued { grant_id: self.grant_id, expires_at_ms: self.expires_at_ms }
                }

                transition IssueRefusedWindow {
                    on input Issue { grant_id, now_ms, expires_at_ms, max_lifetime_ms }
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
                    on input Issue { grant_id, now_ms, expires_at_ms, max_lifetime_ms }
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

                transition AdmitLive {
                    on input AdmitUse { now_ms }
                    guard "active" { self.lifecycle_phase == Phase::Active }
                    guard "deadline_not_reached" { now_ms < self.expires_at_ms }
                    update {}
                    to Active
                    emit UseAdmitted { grant_id: self.grant_id }
                }

                // The first use at or after the deadline observes the expiry
                // and is refused.
                transition AdmitDue {
                    on input AdmitUse { now_ms }
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
                    on input AdmitUse { now_ms }
                    guard "expired" { self.lifecycle_phase == Phase::Expired }
                    update {}
                    to Expired
                    emit UseRefused { grant_id: self.grant_id, reason: ToolGrantUseRefusal::Expired }
                }

                transition AdmitEndedRevoked {
                    on input AdmitUse { now_ms }
                    guard "revoked" { self.lifecycle_phase == Phase::Revoked }
                    update {}
                    to Revoked
                    emit UseRefused { grant_id: self.grant_id, reason: ToolGrantUseRefusal::Revoked }
                }
            }
        }
    };
}

crate::tool_grant_lifecycle_catalog_machine_dsl!("self", "catalog::dsl::tool_grant_lifecycle");
