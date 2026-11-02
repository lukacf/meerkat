//! Canonical lifecycle authority for ONE quarantined persisted mob store.
//!
//! A realm's mob-set restore keeps a persisted mob store it cannot restore
//! quarantined instead of failing the whole realm: the store is kept as
//! found, no actor runs for it, a lookup of its mob answers the typed
//! quarantine, and every other mob of the realm restores and serves. This
//! machine owns that one store's quarantine: why it is quarantined (its
//! class), whether a re-attempt of its restore is in flight, and how each
//! re-attempt ended.
//!
//! # Scope discipline
//!
//! Record-scoped, not registry-scoped: one machine state per quarantined
//! store, so there is no index keyed by store or mob inside the machine.
//! Volatile: the mob-set state holds it in memory and never persists it. The
//! durable fact is the store itself, and every restore pass of a new process
//! derives the quarantine again from what it finds.
//!
//! The machine deliberately does NOT own:
//!
//! - **The mob's own lifecycle.** `MobMachine` is canonical there once the
//!   store resumes; a quarantined store has no `MobMachine` state at all.
//! - **Retry policy or time.** It reads no clock and never re-attempts on
//!   its own. The shell asks for a re-attempt (an explicit retry, or an
//!   id-addressed verb that meets a `StoreUnavailable` quarantine) and the
//!   machine admits at most one at a time.
//! - **The restore itself.** The shell runs the re-attempt the machine
//!   admitted and reports how it ended; the machine owns what the outcome
//!   means for the record.
//!
//! # Totality discipline
//!
//! Every (phase, input) pair has an explicit transition naming its own `to`
//! phase. Refusal and stale arms return to the exact phase they matched, so
//! no typed refusal changes the record. There is no catch-all arm.

use meerkat_machine_dsl::machine;

/// Why a persisted mob store is quarantined.
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
pub enum MobQuarantineClass {
    /// A store could not serve a read the restore needed now: nothing is
    /// known to be wrong with the stored state, and a re-attempt may pass.
    #[default]
    StoreUnavailable,
    /// The mob's store, or stored state its restore needs, cannot be opened,
    /// read or decoded.
    StoreUnreadable,
    /// The restore refused the store for another typed reason (for example
    /// a store whose records disagree among themselves).
    ResumeRefused,
}

impl MobQuarantineClass {
    /// Stable token in structured data and on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StoreUnavailable => "store_unavailable",
            Self::StoreUnreadable => "store_unreadable",
            Self::ResumeRefused => "resume_refused",
        }
    }

    /// Whether the store may restore on a later attempt without a repair: a
    /// store that could not serve now. The other classes need the store, or
    /// the state it holds, repaired first.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::StoreUnavailable)
    }
}

impl std::fmt::Display for MobQuarantineClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a re-attempt of a quarantined store's restore ended.
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
pub enum MobStoreRestoreOutcome {
    /// The mob resumed and is managed.
    #[default]
    Managed,
    /// The mob is a child held before activation until its owner resolves.
    Held,
    /// The store held no mob and was swept, or the child was torn down with
    /// its archived owner.
    Settled,
    /// The re-attempt failed: the store stays quarantined under the class
    /// the attempt met.
    Failed,
}

/// Why a quarantine or a re-attempt request was refused.
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
pub enum MobStoreRestoreRefusal {
    /// A re-attempt of this store's restore is already in flight.
    #[default]
    RetryInFlight,
    /// The record holds no quarantine to re-attempt: it was never
    /// classified, or the store was released.
    NotQuarantined,
    /// The record was already classified; a pass classifies a store once.
    AlreadyClassified,
    /// A re-attempt outcome arrived while no re-attempt was in flight.
    NotRetrying,
}

machine! {
    machine MobStoreRestoreLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::mob_store_restore_lifecycle",

        state {
            lifecycle_phase: MobStoreRestoreLifecyclePhase,
            // Why the store is quarantined. Meaningful while the record is
            // Quarantined or Retrying; the class the last failed attempt met.
            class: Enum<MobQuarantineClass>,
            // Re-attempts that ended, in either outcome.
            attempts: u64,
        }

        init(Unclassified) {
            class = MobQuarantineClass::StoreUnavailable,
            attempts = 0,
        }

        terminal [Released]

        phase MobStoreRestoreLifecyclePhase {
            // The restore pass found the store and has not classified it.
            Unclassified,
            // The store is quarantined under its class.
            Quarantined,
            // One re-attempt of the store's restore is in flight.
            Retrying,
            // A re-attempt released the store: it is managed, held, or
            // settled, and the record has nothing left to own.
            Released,
        }

        input MobStoreRestoreLifecycleInput {
            // The restore pass could not restore the store: quarantine it.
            Quarantine { class: Enum<MobQuarantineClass> },
            // Ask for one re-attempt of the store's restore.
            RequestRetry {},
            // The re-attempt the machine admitted ended. `class` is the class
            // a failed attempt met; it is ignored for any other outcome.
            ResolveRetry { outcome: Enum<MobStoreRestoreOutcome>, class: Enum<MobQuarantineClass> },
        }

        effect MobStoreRestoreLifecycleEffect {
            StoreQuarantined { class: Enum<MobQuarantineClass> },
            RetryAdmitted { attempts: u64 },
            StoreReleased { outcome: Enum<MobStoreRestoreOutcome>, attempts: u64 },
            StoreStillQuarantined { class: Enum<MobQuarantineClass>, attempts: u64 },
            RequestRefused { reason: Enum<MobStoreRestoreRefusal> },
        }

        // A record the pass has not classified carries no attempt.
        invariant unclassified_record_has_no_attempts {
            self.lifecycle_phase != Phase::Unclassified || self.attempts == 0
        }

        // Only an attempt releases a store.
        invariant released_record_was_attempted {
            self.lifecycle_phase != Phase::Released || self.attempts > 0
        }

        // Verdict effects: the mob-set state mirrors them as its typed result.
        disposition StoreQuarantined => local seam SurfaceResultAlignment,
        disposition RetryAdmitted => local seam SurfaceResultAlignment,
        disposition StoreReleased => local seam SurfaceResultAlignment,
        disposition StoreStillQuarantined => local seam SurfaceResultAlignment,
        disposition RequestRefused => local seam SurfaceResultAlignment,

        // ------------------------------------------------------------------
        // Quarantine
        // The restore pass classifies a store once.
        // ------------------------------------------------------------------

        transition QuarantineUnclassified {
            on input Quarantine { class }
            guard "unclassified" { self.lifecycle_phase == Phase::Unclassified }
            update {
                self.class = class;
            }
            to Quarantined
            emit StoreQuarantined { class: self.class }
        }

        transition QuarantineQuarantined {
            on input Quarantine { class }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyClassified }
        }

        transition QuarantineRetrying {
            on input Quarantine { class }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyClassified }
        }

        transition QuarantineReleased {
            on input Quarantine { class }
            guard "released" { self.lifecycle_phase == Phase::Released }
            update {}
            to Released
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyClassified }
        }

        // ------------------------------------------------------------------
        // RequestRetry
        // At most one re-attempt is in flight; a second request is refused
        // at once and never waits for the first.
        // ------------------------------------------------------------------

        transition RequestRetryUnclassified {
            on input RequestRetry {}
            guard "unclassified" { self.lifecycle_phase == Phase::Unclassified }
            update {}
            to Unclassified
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotQuarantined }
        }

        transition RequestRetryQuarantined {
            on input RequestRetry {}
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Retrying
            emit RetryAdmitted { attempts: self.attempts }
        }

        transition RequestRetryRetrying {
            on input RequestRetry {}
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::RetryInFlight }
        }

        transition RequestRetryReleased {
            on input RequestRetry {}
            guard "released" { self.lifecycle_phase == Phase::Released }
            update {}
            to Released
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotQuarantined }
        }

        // ------------------------------------------------------------------
        // ResolveRetry
        // The admitted re-attempt ended: a failure keeps the store
        // quarantined under the class it met; any other outcome releases it.
        // ------------------------------------------------------------------

        transition ResolveRetryFailed {
            on input ResolveRetry { outcome, class }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            guard "failed" { outcome == MobStoreRestoreOutcome::Failed }
            update {
                self.class = class;
                self.attempts += 1;
            }
            to Quarantined
            emit StoreStillQuarantined { class: self.class, attempts: self.attempts }
        }

        transition ResolveRetryRestored {
            on input ResolveRetry { outcome, class }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            guard "not_failed" { outcome != MobStoreRestoreOutcome::Failed }
            update {
                self.attempts += 1;
            }
            to Released
            emit StoreReleased { outcome: outcome, attempts: self.attempts }
        }

        transition ResolveRetryUnclassified {
            on input ResolveRetry { outcome, class }
            guard "unclassified" { self.lifecycle_phase == Phase::Unclassified }
            update {}
            to Unclassified
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotRetrying }
        }

        transition ResolveRetryQuarantined {
            on input ResolveRetry { outcome, class }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotRetrying }
        }

        transition ResolveRetryReleased {
            on input ResolveRetry { outcome, class }
            guard "released" { self.lifecycle_phase == Phase::Released }
            update {}
            to Released
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotRetrying }
        }
    }
}
