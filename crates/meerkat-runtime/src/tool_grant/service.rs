//! [`ToolGrantService`]: scoped tool grants over a [`RuntimeStore`].
//!
//! The service holds no grant state. Each grant is one stored row: its
//! immutable descriptor and the state of its generated
//! `ToolGrantLifecycleMachine`. Every grant, revoke, list, audit read, timer
//! fire, model-request boundary and gated dispatch reads the rows it needs,
//! reads the host clock once (after its reads), rebuilds each machine from
//! its row and applies one input. It decides only from the effects of that
//! input: `UseAdmitted` admits, `UseRefused` refuses, and an `Expired` or
//! `Revoked` effect is a lifecycle change it writes back with a
//! compare-and-swap, in one store write with the change's audit row. On a
//! conflict it reloads the row and applies the input once more; a second
//! conflict is [`ToolGrantServiceError::Conflict`].

use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use meerkat_core::tool_grant::{
    GatedToolTable, ToolGrantCall, ToolGrantId, ToolGrantIssuer, ToolGrantRecord, ToolGrantRefusal,
    ToolGrantSubject, ToolGrantSubjectKey, ToolGrantTarget,
};
use meerkat_core::types::ToolNameSet;
use serde::{Deserialize, Serialize};

use super::audit::{
    ToolGrantAuditEvent, ToolGrantAuditRecord, ToolGrantAuditRow, ToolGrantExpiryDriver,
    ToolGrantRefusalKind,
};
use super::dsl::{
    ToolGrantEndReason, ToolGrantLifecycleEffect, ToolGrantLifecycleInput,
    ToolGrantLifecycleMachineAuthority, ToolGrantLifecycleMachineMutator,
    ToolGrantLifecycleMachineState, ToolGrantPhase, ToolGrantUseRefusal,
};
use super::{HostToolGrantClock, ToolGrantClock, ToolGrantPolicy};
use crate::store::{RuntimeStore, RuntimeStoreError};

/// A grant's status, as its stored row records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolGrantStatus {
    /// Issued, and not yet observed at or after its deadline.
    Active,
    /// Observed at or after its deadline. Terminal.
    Expired,
    /// Revoked before its deadline. Terminal.
    Revoked,
}

/// One grant's durable row: its immutable descriptor and the state of its
/// generated `ToolGrantLifecycleMachine`.
///
/// A grant read through the service is settled at the clock reading of
/// that read: a row past its deadline reads as expired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredToolGrant {
    pub record: ToolGrantRecord,
    pub status: ToolGrantStatus,
    /// The host clock reading the grant was issued at, in UNIX ms.
    pub issued_at_ms: u64,
    /// The grant's fixed deadline, in UNIX ms. It never moves.
    pub expires_at_ms: u64,
    /// The host clock reading the grant was observed to end at, in UNIX ms;
    /// set exactly when it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
}

impl StoredToolGrant {
    fn encode(&self) -> Result<Vec<u8>, ToolGrantServiceError> {
        serde_json::to_vec(self).map_err(|error| ToolGrantServiceError::CorruptRow {
            grant_id: self.record.grant_id.to_string(),
            detail: error.to_string(),
        })
    }

    fn decode(grant_id: &str, bytes: &[u8]) -> Result<Self, ToolGrantServiceError> {
        let row: Self =
            serde_json::from_slice(bytes).map_err(|error| ToolGrantServiceError::CorruptRow {
                grant_id: grant_id.to_string(),
                detail: error.to_string(),
            })?;
        if row.record.grant_id.to_string() != grant_id {
            return Err(ToolGrantServiceError::CorruptRow {
                grant_id: grant_id.to_string(),
                detail: format!("the row names the grant {}", row.record.grant_id),
            });
        }
        Ok(row)
    }

    /// The generated machine, rebuilt from this row. The machine's
    /// invariants refuse a torn row.
    fn authority(&self) -> Result<ToolGrantLifecycleMachineAuthority, ToolGrantServiceError> {
        let (lifecycle_phase, end_reason) = match self.status {
            ToolGrantStatus::Active => (ToolGrantPhase::Active, None),
            ToolGrantStatus::Expired => {
                (ToolGrantPhase::Expired, Some(ToolGrantEndReason::Deadline))
            }
            ToolGrantStatus::Revoked => {
                (ToolGrantPhase::Revoked, Some(ToolGrantEndReason::Revoked))
            }
        };
        ToolGrantLifecycleMachineAuthority::recover_from_state(ToolGrantLifecycleMachineState {
            lifecycle_phase,
            grant_id: self.record.grant_id.to_string(),
            issued_at_ms: self.issued_at_ms,
            expires_at_ms: self.expires_at_ms,
            ended_at_ms: self.ended_at_ms,
            end_reason,
        })
        .map_err(|error| ToolGrantServiceError::CorruptRow {
            grant_id: self.record.grant_id.to_string(),
            detail: error.to_string(),
        })
    }

    /// The row that stores `record` with the machine `state` a transition
    /// left: the durable projection of the generated state.
    fn projected(
        record: ToolGrantRecord,
        state: &ToolGrantLifecycleMachineState,
    ) -> Result<Self, ToolGrantServiceError> {
        let status = match state.lifecycle_phase {
            ToolGrantPhase::Active => ToolGrantStatus::Active,
            ToolGrantPhase::Expired => ToolGrantStatus::Expired,
            ToolGrantPhase::Revoked => ToolGrantStatus::Revoked,
            ToolGrantPhase::Unissued => {
                return Err(ToolGrantServiceError::Machine {
                    detail: format!("grant {} has no issued state to store", record.grant_id),
                });
            }
        };
        Ok(Self {
            record,
            status,
            issued_at_ms: state.issued_at_ms,
            expires_at_ms: state.expires_at_ms,
            ended_at_ms: state.ended_at_ms,
        })
    }
}

/// How long a grant lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGrantWindow {
    /// From the issuing clock reading, for this long.
    For(Duration),
    /// Until this absolute deadline, in UNIX ms.
    Until { expires_at_ms: u64 },
}

/// A request to issue one grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGrantRequest {
    /// Who the grant applies to.
    pub subject: ToolGrantSubject,
    /// What it opens. It opens only the names a build gated.
    pub target: ToolGrantTarget,
    /// Who issued it.
    pub issuer: ToolGrantIssuer,
    /// Why, for the audit.
    pub reason: Option<String>,
    /// How long it lasts; at most the policy's maximum lifetime.
    pub window: ToolGrantWindow,
}

/// Who revoked a grant, and why, for the audit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolGrantRevocation {
    pub actor: Option<String>,
    pub reason: Option<String>,
}

/// Why a grant operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ToolGrantServiceError {
    /// The window is past, empty, or longer than the maximum lifetime.
    #[error(
        "the grant window ending at {expires_at_ms} ms is past, empty or longer than the \
         maximum lifetime of {max_lifetime_ms} ms (now {now_ms} ms)"
    )]
    InvalidWindow {
        now_ms: u64,
        expires_at_ms: u64,
        max_lifetime_ms: u64,
    },
    /// The target names no tool.
    #[error("a tool grant must name at least one tool")]
    EmptyTarget,
    /// No grant has this id.
    #[error("tool grant {grant_id} was not found")]
    NotFound { grant_id: String },
    /// Another writer changed the grant twice while this one wrote.
    #[error("tool grant {grant_id} changed under a concurrent writer")]
    Conflict { grant_id: String },
    /// The host clock gave no representable time.
    #[error("the tool grant clock is unavailable")]
    Clock,
    /// A stored row does not decode, or its machine refused it.
    #[error("tool grant {grant_id} is unreadable: {detail}")]
    CorruptRow { grant_id: String, detail: String },
    /// The generated machine refused an input the service must not send.
    #[error("tool grant machine refused an input: {detail}")]
    Machine { detail: String },
    /// The store failed.
    #[error(transparent)]
    Store(#[from] RuntimeStoreError),
}

/// One stored row as read: its id, its exact bytes (the CAS expectation)
/// and the decoded row.
struct ReadRow {
    grant_id: String,
    bytes: Vec<u8>,
    row: StoredToolGrant,
}

/// What a lifecycle change's audit row records beyond its effect.
#[derive(Clone, Copy)]
enum AuditContext<'a> {
    /// An expiry observed by this driver.
    Expiry(ToolGrantExpiryDriver),
    /// A revoke: a `Revoked` effect records it, and an `Expired` effect
    /// records the expiry the revoke observed.
    Revocation(&'a ToolGrantRevocation),
}

/// One input applied to a row's rebuilt machine: the transition's effects,
/// and the row after it when an effect records a lifecycle change.
struct Applied {
    effects: Vec<ToolGrantLifecycleEffect>,
    next: Option<StoredToolGrant>,
}

fn apply(
    row: &StoredToolGrant,
    input: ToolGrantLifecycleInput,
) -> Result<Applied, ToolGrantServiceError> {
    let mut authority = row.authority()?;
    let effects = ToolGrantLifecycleMachineMutator::apply(&mut authority, input)
        .map_err(|error| ToolGrantServiceError::Machine {
            detail: format!("grant {}: {error}", row.record.grant_id),
        })?
        .into_effects();
    let changed = effects.iter().any(|effect| {
        matches!(
            effect,
            ToolGrantLifecycleEffect::Expired { .. } | ToolGrantLifecycleEffect::Revoked { .. }
        )
    });
    let next = if changed {
        Some(StoredToolGrant::projected(
            row.record.clone(),
            authority.state(),
        )?)
    } else {
        None
    };
    Ok(Applied { effects, next })
}

/// The audit rows that record the lifecycle changes among `effects`.
fn lifecycle_audit_rows(
    row: &StoredToolGrant,
    effects: &[ToolGrantLifecycleEffect],
    context: AuditContext<'_>,
) -> Result<Vec<Vec<u8>>, ToolGrantServiceError> {
    let grant_id = row.record.grant_id.clone();
    let mut rows = Vec::new();
    for effect in effects {
        let audit = match effect {
            ToolGrantLifecycleEffect::Expired {
                expires_at_ms,
                observed_at_ms,
                ..
            } => ToolGrantAuditRow {
                at_ms: *observed_at_ms,
                event: ToolGrantAuditEvent::Expired {
                    grant_id: grant_id.clone(),
                    expires_at_ms: *expires_at_ms,
                    observed_at_ms: *observed_at_ms,
                    driver: match context {
                        AuditContext::Expiry(driver) => driver,
                        AuditContext::Revocation(_) => ToolGrantExpiryDriver::Revoke,
                    },
                },
            },
            ToolGrantLifecycleEffect::Revoked { observed_at_ms, .. } => {
                let (actor, reason) = match context {
                    AuditContext::Revocation(revocation) => {
                        (revocation.actor.clone(), revocation.reason.clone())
                    }
                    AuditContext::Expiry(_) => (None, None),
                };
                ToolGrantAuditRow {
                    at_ms: *observed_at_ms,
                    event: ToolGrantAuditEvent::Revoked {
                        grant_id: grant_id.clone(),
                        observed_at_ms: *observed_at_ms,
                        actor,
                        reason,
                    },
                }
            }
            _ => continue,
        };
        rows.push(
            audit
                .encode()
                .map_err(|error| ToolGrantServiceError::CorruptRow {
                    grant_id: grant_id.to_string(),
                    detail: error.to_string(),
                })?,
        );
    }
    Ok(rows)
}

/// What one grant's `AdmitUse` decided for a call, from its effects: `Ok`
/// for `UseAdmitted`, the refusal for `UseRefused`.
fn use_verdict(
    row: &StoredToolGrant,
    effects: &[ToolGrantLifecycleEffect],
) -> Result<Result<(), ToolGrantRefusal>, ToolGrantServiceError> {
    for effect in effects {
        match effect {
            ToolGrantLifecycleEffect::UseAdmitted { .. } => return Ok(Ok(())),
            ToolGrantLifecycleEffect::UseRefused {
                reason: ToolGrantUseRefusal::Expired,
                ..
            } => {
                return Ok(Err(ToolGrantRefusal::Expired {
                    grant_id: row.record.grant_id.clone(),
                    expires_at_ms: row.expires_at_ms,
                }));
            }
            ToolGrantLifecycleEffect::UseRefused {
                reason: ToolGrantUseRefusal::Revoked,
                ..
            } => {
                return Ok(Err(ToolGrantRefusal::Revoked {
                    grant_id: row.record.grant_id.clone(),
                }));
            }
            _ => {}
        }
    }
    Err(ToolGrantServiceError::Machine {
        detail: format!(
            "grant {} answered a use with neither admission nor refusal",
            row.record.grant_id
        ),
    })
}

/// Which refusal explains a refused call when several grants cover its
/// name: an expired grant, then a revoked one, then an unavailable read;
/// with none of them the call needs a grant.
fn refusal_rank(refusal: &ToolGrantRefusal) -> u8 {
    match refusal {
        ToolGrantRefusal::Expired { .. } => 3,
        ToolGrantRefusal::Revoked { .. } => 2,
        ToolGrantRefusal::Unavailable { .. } => 1,
        ToolGrantRefusal::Required => 0,
    }
}

fn unavailable(error: &ToolGrantServiceError) -> ToolGrantRefusal {
    ToolGrantRefusal::Unavailable {
        detail: error.to_string(),
    }
}

/// A served session's model-request boundary, adjudicated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BoundaryAdjudication {
    /// The gated names a live grant opens now.
    pub(crate) open: ToolNameSet,
    /// The earliest deadline of the grants that opened them.
    pub(crate) next_deadline_ms: Option<u64>,
}

/// A subject's grants, settled at one clock reading.
pub(crate) struct SettledGrants {
    pub(crate) grants: Vec<StoredToolGrant>,
    /// The rows of the subject this binary cannot read.
    pub(crate) unreadable: Vec<UnreadableToolGrant>,
    /// The earliest deadline of the grants still live at the reading: when
    /// the deadline driver looks again. An ended grant arms nothing.
    pub(crate) next_deadline_ms: Option<u64>,
}

/// A stored grant row this binary cannot read: written by a newer binary, or
/// damaged. It admits nothing and blocks nothing: every other grant of its
/// subject is adjudicated as before, a list reports it, and a revoke of it
/// is refused with [`ToolGrantServiceError::CorruptRow`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableToolGrant {
    /// The grant id the row is stored under.
    pub grant_id: String,
    /// Why the row could not be read.
    pub detail: String,
}

/// A subject's grants as a list answers them: every readable grant, settled
/// at one clock reading, and every row this binary cannot read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolGrantListing {
    pub grants: Vec<StoredToolGrant>,
    pub unreadable: Vec<UnreadableToolGrant>,
}

/// The rows filed under some subjects: the readable ones, and the ones this
/// binary cannot read.
struct SubjectRows {
    readable: Vec<ReadRow>,
    unreadable: Vec<UnreadableToolGrant>,
}

impl SubjectRows {
    /// Log the rows this binary cannot read: they admit nothing.
    fn warn_unreadable(&self, adjudication: &'static str) {
        for row in &self.unreadable {
            tracing::warn!(
                grant_id = %row.grant_id,
                detail = %row.detail,
                adjudication,
                "an unreadable tool grant row admits nothing"
            );
        }
    }
}

/// Scoped tool grants over a [`RuntimeStore`]. See the module docs.
pub struct ToolGrantService {
    store: Arc<dyn RuntimeStore>,
    clock: StdRwLock<Arc<dyn ToolGrantClock>>,
    policy: StdRwLock<ToolGrantPolicy>,
}

impl std::fmt::Debug for ToolGrantService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolGrantService")
            .field("policy", &self.policy())
            .finish_non_exhaustive()
    }
}

impl ToolGrantService {
    /// The grant service over `store`, on the host clock and the default
    /// policy.
    #[must_use]
    pub fn new(store: Arc<dyn RuntimeStore>) -> Self {
        Self {
            store,
            clock: StdRwLock::new(Arc::new(HostToolGrantClock)),
            policy: StdRwLock::new(ToolGrantPolicy::default()),
        }
    }

    /// Read `clock` instead of the host clock.
    pub fn set_clock(&self, clock: Arc<dyn ToolGrantClock>) {
        *self
            .clock
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = clock;
    }

    /// Issue grants under `policy`.
    pub fn set_policy(&self, policy: ToolGrantPolicy) {
        *self
            .policy
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = policy;
    }

    /// The policy grants are issued under.
    #[must_use]
    pub fn policy(&self) -> ToolGrantPolicy {
        *self
            .policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// One clock reading, in UNIX ms.
    pub(crate) fn now_ms(&self) -> Result<u64, ToolGrantServiceError> {
        self.clock
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .now_ms()
            .map_err(|_| ToolGrantServiceError::Clock)
    }

    /// Issue one grant.
    ///
    /// # Errors
    /// [`ToolGrantServiceError::InvalidWindow`] for a window that is past,
    /// empty or longer than the policy's maximum lifetime,
    /// [`ToolGrantServiceError::EmptyTarget`] for a target with no name, and
    /// store and clock failures.
    pub async fn grant(
        &self,
        request: ToolGrantRequest,
    ) -> Result<StoredToolGrant, ToolGrantServiceError> {
        if request.target.names().next().is_none()
            || request.target.names().any(|name| name.as_str().is_empty())
        {
            return Err(ToolGrantServiceError::EmptyTarget);
        }
        let now_ms = self.now_ms()?;
        let expires_at_ms = match request.window {
            ToolGrantWindow::For(lifetime) => {
                now_ms.saturating_add(u64::try_from(lifetime.as_millis()).unwrap_or(u64::MAX))
            }
            ToolGrantWindow::Until { expires_at_ms } => expires_at_ms,
        };
        let max_lifetime_ms = self.policy().max_lifetime_ms();
        let grant_id = ToolGrantId::mint();
        let mut authority = ToolGrantLifecycleMachineAuthority::new();
        let effects = ToolGrantLifecycleMachineMutator::apply(
            &mut authority,
            ToolGrantLifecycleInput::Issue {
                grant_id: grant_id.to_string(),
                now_ms,
                expires_at_ms,
                max_lifetime_ms,
            },
        )
        .map_err(|error| ToolGrantServiceError::Machine {
            detail: error.to_string(),
        })?
        .into_effects();
        let mut issued = false;
        for effect in &effects {
            match effect {
                ToolGrantLifecycleEffect::Issued { .. } => issued = true,
                ToolGrantLifecycleEffect::IssueRefused { .. } => {
                    return Err(ToolGrantServiceError::InvalidWindow {
                        now_ms,
                        expires_at_ms,
                        max_lifetime_ms,
                    });
                }
                _ => {}
            }
        }
        if !issued {
            return Err(ToolGrantServiceError::Machine {
                detail: format!("grant {grant_id} was neither issued nor refused"),
            });
        }
        let record = ToolGrantRecord {
            grant_id: grant_id.clone(),
            subject: request.subject,
            target: request.target,
            issuer: request.issuer,
            reason: request.reason,
        };
        let row = StoredToolGrant::projected(record, authority.state())?;
        let granted = ToolGrantAuditRow {
            at_ms: now_ms,
            event: ToolGrantAuditEvent::Granted {
                grant_id: grant_id.clone(),
                target: row.record.target.clone(),
                issuer: row.record.issuer.clone(),
                reason: row.record.reason.clone(),
                expires_at_ms: row.expires_at_ms,
            },
        }
        .encode()
        .map_err(|error| ToolGrantServiceError::CorruptRow {
            grant_id: grant_id.to_string(),
            detail: error.to_string(),
        })?;
        let subject_key = row.record.subject.key();
        let inserted = self
            .store
            .put_tool_grant_if_absent(
                &grant_id.to_string(),
                subject_key.as_str(),
                &row.encode()?,
                &[granted],
            )
            .await?;
        if !inserted {
            return Err(ToolGrantServiceError::Conflict {
                grant_id: grant_id.to_string(),
            });
        }
        Ok(row)
    }

    /// Revoke one grant. A revoke at or after the deadline records the
    /// expiry the grant reached instead, and a grant that already ended is
    /// answered as it stands.
    ///
    /// # Errors
    /// [`ToolGrantServiceError::NotFound`], and store and clock failures.
    pub async fn revoke(
        &self,
        grant_id: &ToolGrantId,
        revocation: ToolGrantRevocation,
    ) -> Result<StoredToolGrant, ToolGrantServiceError> {
        let read = self.load_row(&grant_id.to_string()).await?;
        let now_ms = self.now_ms()?;
        let (row, _) = self
            .apply_and_commit(
                read,
                &|| ToolGrantLifecycleInput::Revoke { now_ms },
                AuditContext::Revocation(&revocation),
            )
            .await?;
        Ok(row)
    }

    /// Every grant of `subject`, each due one settled as expired first, and
    /// every row of the subject this binary cannot read.
    ///
    /// # Errors
    /// Store and clock failures.
    pub async fn list(
        &self,
        subject: &ToolGrantSubject,
    ) -> Result<ToolGrantListing, ToolGrantServiceError> {
        let settled = self
            .settle(&subject.key(), ToolGrantExpiryDriver::Read)
            .await?;
        Ok(ToolGrantListing {
            grants: settled.grants,
            unreadable: settled.unreadable,
        })
    }

    /// At most `limit` of `subject`'s audit rows, after `after` when given,
    /// in sequence order. Each due grant of the subject is settled as
    /// expired first, so the log records its expiry.
    ///
    /// # Errors
    /// Store and clock failures, and an audit row that does not decode.
    pub async fn audit(
        &self,
        subject: &ToolGrantSubject,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<ToolGrantAuditRecord>, ToolGrantServiceError> {
        let key = subject.key();
        self.settle(&key, ToolGrantExpiryDriver::Read).await?;
        self.store
            .read_tool_grant_audit(key.as_str(), after, limit)
            .await?
            .into_iter()
            .map(|(sequence, bytes)| {
                ToolGrantAuditRow::decode(&bytes)
                    .map(|row| ToolGrantAuditRecord { sequence, row })
                    .map_err(|error| ToolGrantServiceError::CorruptRow {
                        grant_id: format!("{key} audit row {sequence}"),
                        detail: error.to_string(),
                    })
            })
            .collect()
    }

    /// Settle `subject_key`'s due grants as expired, observed by `driver`,
    /// and answer every readable grant with the next deadline of the grants
    /// still live. A grant is live when its own `AdmitUse` at the reading
    /// admits: an ended grant arms no deadline.
    pub(crate) async fn settle(
        &self,
        subject_key: &ToolGrantSubjectKey,
        driver: ToolGrantExpiryDriver,
    ) -> Result<SettledGrants, ToolGrantServiceError> {
        let rows = self.subject_rows(std::slice::from_ref(subject_key)).await?;
        rows.warn_unreadable("settle");
        let now_ms = self.now_ms()?;
        let mut grants = Vec::with_capacity(rows.readable.len());
        let mut unreadable = rows.unreadable;
        let mut next_deadline_ms: Option<u64> = None;
        for read in rows.readable {
            let (row, _) = match self
                .apply_and_commit(
                    read,
                    &|| ToolGrantLifecycleInput::Expire { now_ms },
                    AuditContext::Expiry(driver),
                )
                .await
            {
                Ok(applied) => applied,
                // A row whose machine refuses to rebuild is reported, not
                // settled.
                Err(ToolGrantServiceError::CorruptRow { grant_id, detail }) => {
                    tracing::warn!(
                        %grant_id,
                        %detail,
                        adjudication = "settle",
                        "an unreadable tool grant row admits nothing"
                    );
                    unreadable.push(UnreadableToolGrant { grant_id, detail });
                    continue;
                }
                Err(error) => return Err(error),
            };
            let live = matches!(
                apply(&row, ToolGrantLifecycleInput::AdmitUse { now_ms })
                    .and_then(|applied| use_verdict(&row, &applied.effects)),
                Ok(Ok(()))
            );
            if live {
                next_deadline_ms = Some(
                    next_deadline_ms.map_or(row.expires_at_ms, |next| next.min(row.expires_at_ms)),
                );
            }
            grants.push(row);
        }
        Ok(SettledGrants {
            grants,
            unreadable,
            next_deadline_ms,
        })
    }

    /// A model-request boundary: the gated names of `gated` a live grant of
    /// `subjects` opens at this moment. A grant the boundary finds past its
    /// deadline is settled as expired; a row this binary cannot read opens
    /// nothing.
    ///
    /// # Errors
    /// `Unavailable` when the grants cannot be read or settled: the gate
    /// stays closed.
    pub(crate) async fn adjudicate_boundary(
        &self,
        subjects: &[ToolGrantSubjectKey],
        gated: &GatedToolTable,
    ) -> Result<BoundaryAdjudication, ToolGrantRefusal> {
        let rows = self
            .subject_rows(subjects)
            .await
            .map_err(|error| unavailable(&error))?;
        rows.warn_unreadable("boundary");
        let now_ms = self.now_ms().map_err(|error| unavailable(&error))?;
        let mut adjudication = BoundaryAdjudication::default();
        for read in rows.readable {
            let covered: Vec<_> = read
                .row
                .record
                .target
                .names()
                .filter(|name| gated.is_gated(name.as_str()))
                .cloned()
                .collect();
            if covered.is_empty() {
                continue;
            }
            let (row, effects) = match self
                .apply_and_commit(
                    read,
                    &|| ToolGrantLifecycleInput::AdmitUse { now_ms },
                    AuditContext::Expiry(ToolGrantExpiryDriver::Boundary),
                )
                .await
            {
                Ok(applied) => applied,
                // A row whose machine refuses to rebuild opens nothing.
                Err(ToolGrantServiceError::CorruptRow { grant_id, detail }) => {
                    tracing::warn!(
                        %grant_id,
                        %detail,
                        adjudication = "boundary",
                        "an unreadable tool grant row admits nothing"
                    );
                    continue;
                }
                Err(error) => return Err(unavailable(&error)),
            };
            if use_verdict(&row, &effects)
                .map_err(|error| unavailable(&error))?
                .is_ok()
            {
                for name in covered {
                    adjudication.open.insert(name);
                }
                adjudication.next_deadline_ms = Some(
                    adjudication
                        .next_deadline_ms
                        .map_or(row.expires_at_ms, |next| next.min(row.expires_at_ms)),
                );
            }
        }
        Ok(adjudication)
    }

    /// One gated call's admission. The grants are read first; the clock is
    /// read after that read, and an admission returns with nothing awaited
    /// after the clock reading, so the caller hands the call on at the
    /// moment it was adjudicated. A refusal settles any grant it found past
    /// its deadline, then appends a `refused` audit row; a failed append is
    /// logged and never turns the refusal into an admission.
    ///
    /// # Errors
    /// The refusal: no live grant covers the name, the covering grant
    /// expired or was revoked, or the grants could not be read.
    pub(crate) async fn admit_call(
        &self,
        subjects: &[ToolGrantSubjectKey],
        call: ToolGrantCall<'_>,
    ) -> Result<(), ToolGrantRefusal> {
        let rows = match self.subject_rows(subjects).await {
            Ok(rows) => rows,
            Err(error) => {
                let refusal = unavailable(&error);
                self.record_refusal(subjects.first(), call, &refusal).await;
                return Err(refusal);
            }
        };
        rows.warn_unreadable("dispatch");
        let now_ms = match self.now_ms() {
            Ok(now_ms) => now_ms,
            Err(error) => {
                let refusal = unavailable(&error);
                self.record_refusal(subjects.first(), call, &refusal).await;
                return Err(refusal);
            }
        };
        // Adjudicate every covering grant at this one reading before
        // anything else awaits: one admitted grant admits the call.
        let mut observed_expiries = Vec::new();
        let mut refusal = ToolGrantRefusal::Required;
        // The subject whose log records the refusal: the explaining grant's,
        // else the session's.
        let mut refusal_subject = subjects.first().cloned();
        for read in rows.readable {
            if !read.row.record.target.covers(call.name) {
                continue;
            }
            let verdict = apply(&read.row, ToolGrantLifecycleInput::AdmitUse { now_ms }).and_then(
                |applied| {
                    use_verdict(&read.row, &applied.effects)
                        .map(|verdict| (verdict, applied.next.is_some()))
                },
            );
            let (found, observed_expiry) = match verdict {
                Ok((Ok(()), _)) => return Ok(()),
                Ok((Err(found), observed_expiry)) => (found, observed_expiry),
                Err(error) => (unavailable(&error), false),
            };
            if refusal_rank(&found) > refusal_rank(&refusal) {
                refusal = found;
                refusal_subject = Some(read.row.record.subject.key());
            }
            if observed_expiry {
                observed_expiries.push(read);
            }
        }
        // Refused: settle the expiries this adjudication observed.
        for read in observed_expiries {
            let grant_id = read.grant_id.clone();
            if let Err(error) = self
                .apply_and_commit(
                    read,
                    &|| ToolGrantLifecycleInput::AdmitUse { now_ms },
                    AuditContext::Expiry(ToolGrantExpiryDriver::Dispatch),
                )
                .await
            {
                tracing::warn!(
                    %grant_id,
                    %error,
                    "a gated dispatch could not record a tool grant's expiry"
                );
            }
        }
        self.record_refusal(refusal_subject.as_ref(), call, &refusal)
            .await;
        Err(refusal)
    }

    /// Append a `refused` row for `call` to `subject`'s log. Best effort.
    async fn record_refusal(
        &self,
        subject: Option<&ToolGrantSubjectKey>,
        call: ToolGrantCall<'_>,
        refusal: &ToolGrantRefusal,
    ) {
        let Some(subject) = subject else {
            return;
        };
        let at_ms = self.now_ms().unwrap_or_default();
        let grant_id = match refusal {
            ToolGrantRefusal::Expired { grant_id, .. } | ToolGrantRefusal::Revoked { grant_id } => {
                Some(grant_id.clone())
            }
            _ => None,
        };
        let row = ToolGrantAuditRow {
            at_ms,
            event: ToolGrantAuditEvent::Refused {
                tool: meerkat_core::types::ToolName::from(call.name),
                reason: ToolGrantRefusalKind::of(refusal),
                grant_id,
                call_id: call.call_id.to_string(),
                run_id: call.run_id.map(ToString::to_string),
            },
        };
        let appended = match row.encode() {
            Ok(bytes) => self
                .store
                .append_tool_grant_audit(subject.as_str(), &[bytes])
                .await
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        if let Err(error) = appended {
            tracing::warn!(
                tool = call.name,
                %error,
                "a refused gated call's audit row could not be appended"
            );
        }
    }

    async fn load_row(&self, grant_id: &str) -> Result<ReadRow, ToolGrantServiceError> {
        let bytes = self.store.load_tool_grant(grant_id).await?.ok_or_else(|| {
            ToolGrantServiceError::NotFound {
                grant_id: grant_id.to_string(),
            }
        })?;
        let row = StoredToolGrant::decode(grant_id, &bytes)?;
        Ok(ReadRow {
            grant_id: grant_id.to_string(),
            bytes,
            row,
        })
    }

    /// Every row filed under `subjects`.
    /// Every row filed under `subjects`: the readable ones, and the ones this
    /// binary cannot decode, which admit nothing and block nothing.
    async fn subject_rows(
        &self,
        subjects: &[ToolGrantSubjectKey],
    ) -> Result<SubjectRows, ToolGrantServiceError> {
        let mut rows = SubjectRows {
            readable: Vec::new(),
            unreadable: Vec::new(),
        };
        for subject in subjects {
            for (grant_id, bytes) in self.store.list_tool_grants(subject.as_str()).await? {
                match StoredToolGrant::decode(&grant_id, &bytes) {
                    Ok(row) => rows.readable.push(ReadRow {
                        grant_id,
                        bytes,
                        row,
                    }),
                    Err(error) => rows.unreadable.push(UnreadableToolGrant {
                        grant_id,
                        detail: error.to_string(),
                    }),
                }
            }
        }
        Ok(rows)
    }

    /// Apply one input to a row's rebuilt machine and commit what its
    /// effects record: the row after an `Expired` or `Revoked` effect, with
    /// that effect's audit row, in one store write. A CAS conflict (another
    /// writer changed the row first) reloads the row and applies the input
    /// once more; a second conflict is [`ToolGrantServiceError::Conflict`].
    ///
    /// Returns the row as it now stands and the effects of the input as
    /// last applied.
    async fn apply_and_commit(
        &self,
        mut read: ReadRow,
        input: &(dyn Fn() -> ToolGrantLifecycleInput + Send + Sync),
        context: AuditContext<'_>,
    ) -> Result<(StoredToolGrant, Vec<ToolGrantLifecycleEffect>), ToolGrantServiceError> {
        for attempt in 0..2 {
            let applied = apply(&read.row, input())?;
            let Some(next) = applied.next else {
                return Ok((read.row, applied.effects));
            };
            let audit = lifecycle_audit_rows(&read.row, &applied.effects, context)?;
            let subject_key = read.row.record.subject.key();
            if self
                .store
                .compare_and_put_tool_grant(
                    &read.grant_id,
                    subject_key.as_str(),
                    &read.bytes,
                    &next.encode()?,
                    &audit,
                )
                .await?
            {
                return Ok((next, applied.effects));
            }
            if attempt == 0 {
                read = self.load_row(&read.grant_id).await?;
            }
        }
        Err(ToolGrantServiceError::Conflict {
            grant_id: read.grant_id,
        })
    }
}
