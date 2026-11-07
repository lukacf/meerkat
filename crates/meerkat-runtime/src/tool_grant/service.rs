//! [`ToolGrantService`]: scoped tool grants over a [`RuntimeStore`].
//!
//! The service holds no grant state. Each grant is one stored row: its
//! immutable descriptor and the state of its generated
//! `ToolGrantLifecycleMachine`. Every grant, revoke, list, audit read, timer
//! fire, model-request boundary, gated dispatch and run end reads the rows it
//! needs, reads the host clock once (after its reads), rebuilds each machine
//! from its row and applies one input. It decides only from the effects of
//! that input: `UseAdmitted` admits, `UseRefused` refuses, and an `Expired`,
//! `Revoked` or `RunEnded` effect is a lifecycle change it writes back with a
//! compare-and-swap, in one store write with the change's audit row. On a
//! conflict it reloads the row and applies the input once more; a second
//! conflict is [`ToolGrantServiceError::Conflict`].
//!
//! A grant may be scoped to one run of its subject session: it then admits
//! uses of that run only, and ends with it ([`ToolGrantService::end_run`],
//! or the first use of a later run).
//!
//! A grant a person approved through a `tool_grant.issue` decision is
//! issued under the id its decision derives, with the decision as its
//! issuer ([`ToolGrantService::issued_for`]; see [`super::request`]).

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use meerkat_core::RunId;
use meerkat_core::approval::decision::DecisionId;
use meerkat_core::tool_grant::{
    GatedToolTable, ToolGrantCall, ToolGrantId, ToolGrantIssuer, ToolGrantRecord, ToolGrantRefusal,
    ToolGrantSubject, ToolGrantSubjectKey, ToolGrantTarget,
};
use meerkat_core::types::ToolNameSet;
use serde::{Deserialize, Serialize};

use super::audit::{
    ToolGrantAuditEvent, ToolGrantAuditPage, ToolGrantAuditRecord, ToolGrantAuditRow,
    ToolGrantExpiryDriver, ToolGrantRefusalKind, UnreadableToolGrantAuditRow,
};
use super::dsl::{
    ToolGrantEndReason, ToolGrantLifecycleEffect, ToolGrantLifecycleInput,
    ToolGrantLifecycleMachineAuthority, ToolGrantLifecycleMachineMutator,
    ToolGrantLifecycleMachineState, ToolGrantPhase, ToolGrantRunId, ToolGrantUseRefusal,
};
use super::request::{ToolGrantIssueBody, ToolGrantRequestHost};
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
    /// The run it was scoped to ended before its deadline. Terminal. A
    /// binary older than this status reads such a row as unreadable: it
    /// opens nothing there.
    RunEnded,
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
    /// The run a run-scoped grant was issued for: it admits uses of that run
    /// only, and ends with it. Absent for a grant bounded by its window
    /// only, whose row a binary older than run scopes still reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_scope: Option<RunId>,
}

/// The machine's key of a run.
fn run_key(run_id: &RunId) -> ToolGrantRunId {
    ToolGrantRunId(run_id.to_string())
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
            ToolGrantStatus::RunEnded => {
                (ToolGrantPhase::RunEnded, Some(ToolGrantEndReason::RunEnded))
            }
        };
        ToolGrantLifecycleMachineAuthority::recover_from_state(ToolGrantLifecycleMachineState {
            lifecycle_phase,
            grant_id: self.record.grant_id.to_string(),
            issued_at_ms: self.issued_at_ms,
            expires_at_ms: self.expires_at_ms,
            ended_at_ms: self.ended_at_ms,
            end_reason,
            run_scope: self.run_scope.as_ref().map(run_key),
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
            ToolGrantPhase::RunEnded => ToolGrantStatus::RunEnded,
            ToolGrantPhase::Unissued => {
                return Err(ToolGrantServiceError::Machine {
                    detail: format!("grant {} has no issued state to store", record.grant_id),
                });
            }
        };
        let run_scope = match &state.run_scope {
            None => None,
            Some(run) => Some(
                uuid::Uuid::parse_str(&run.0)
                    .map(RunId::from_uuid)
                    .map_err(|error| ToolGrantServiceError::Machine {
                        detail: format!(
                            "grant {} names the run {:?}, which is not a run id: {error}",
                            record.grant_id, run.0
                        ),
                    })?,
            ),
        };
        Ok(Self {
            record,
            status,
            issued_at_ms: state.issued_at_ms,
            expires_at_ms: state.expires_at_ms,
            ended_at_ms: state.ended_at_ms,
            run_scope,
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
    /// The run of its subject session it is scoped to, if any: it then
    /// admits uses of that run only, and ends with it. The window still
    /// applies.
    pub run_scope: Option<RunId>,
}

/// Who revoked a grant, and why, for the audit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolGrantRevocation {
    pub actor: Option<String>,
    pub reason: Option<String>,
}

/// A revoke's answer: the grant as it now stands, and whether this revoke
/// ended it. Revoking a grant that already ended changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokedToolGrant {
    pub grant: StoredToolGrant,
    pub changed: bool,
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
    /// An expiry, or the end of a run, observed by this driver.
    Observed(ToolGrantExpiryDriver),
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
            ToolGrantLifecycleEffect::Expired { .. }
                | ToolGrantLifecycleEffect::Revoked { .. }
                | ToolGrantLifecycleEffect::RunEnded { .. }
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
                        AuditContext::Observed(driver) => driver,
                        AuditContext::Revocation(_) => ToolGrantExpiryDriver::Revoke,
                    },
                },
            },
            ToolGrantLifecycleEffect::Revoked { observed_at_ms, .. } => {
                let (actor, reason) = match context {
                    AuditContext::Revocation(revocation) => {
                        (revocation.actor.clone(), revocation.reason.clone())
                    }
                    AuditContext::Observed(_) => (None, None),
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
            ToolGrantLifecycleEffect::RunEnded { observed_at_ms, .. } => {
                // The machine ends only a run-scoped grant with its run.
                let Some(run_id) = row.run_scope.clone() else {
                    return Err(ToolGrantServiceError::Machine {
                        detail: format!("grant {grant_id} ended with a run it was not scoped to"),
                    });
                };
                ToolGrantAuditRow {
                    at_ms: *observed_at_ms,
                    event: ToolGrantAuditEvent::RunEnded {
                        grant_id: grant_id.clone(),
                        run_id,
                        observed_at_ms: *observed_at_ms,
                        driver: match context {
                            AuditContext::Observed(driver) => driver,
                            AuditContext::Revocation(_) => ToolGrantExpiryDriver::Revoke,
                        },
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
/// for `UseAdmitted`, the refusal for `UseRefused`. A grant scoped to a run
/// that is not the call's (another run, which ended the grant, or no run)
/// covers nothing there: the call needs a grant.
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
            ToolGrantLifecycleEffect::UseRefused {
                reason: ToolGrantUseRefusal::RunEnded | ToolGrantUseRefusal::OutsideRun,
                ..
            } => return Ok(Err(ToolGrantRefusal::Required)),
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
    /// Per subject, the earliest deadline of the grants of it that opened
    /// them: when the subject's deadline driver looks again.
    pub(crate) next_deadlines: BTreeMap<ToolGrantSubjectKey, u64>,
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
    /// What the runtime takes its agents' grant requests with, while it
    /// takes them.
    request_host: StdRwLock<Option<ToolGrantRequestHost>>,
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
    /// policy, taking no grant requests.
    #[must_use]
    pub fn new(store: Arc<dyn RuntimeStore>) -> Self {
        Self {
            store,
            clock: StdRwLock::new(Arc::new(HostToolGrantClock)),
            policy: StdRwLock::new(ToolGrantPolicy::default()),
            request_host: StdRwLock::new(None),
        }
    }

    /// Take the agents' grant requests through `host` (`None`: take none).
    pub fn set_request_host(&self, host: Option<ToolGrantRequestHost>) {
        *self
            .request_host
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = host;
    }

    /// What the agents' grant requests are held with, while the runtime
    /// takes them.
    #[must_use]
    pub fn request_host(&self) -> Option<ToolGrantRequestHost> {
        self.request_host
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
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
        self.issue(request, ToolGrantId::mint(), now_ms).await
    }

    /// The grant the approved `tool_grant.issue` decision `decision_id`
    /// issued: the row under the id the decision derives
    /// ([`ToolGrantId::approved`]), when that decision issued it.
    ///
    /// # Errors
    /// Store failures, and a row under that id this binary cannot read.
    pub async fn issued_for(
        &self,
        decision_id: &DecisionId,
    ) -> Result<Option<StoredToolGrant>, ToolGrantServiceError> {
        let grant_id = ToolGrantId::approved(decision_id).to_string();
        let Some(bytes) = self.store.load_tool_grant(&grant_id).await? else {
            return Ok(None);
        };
        let row = StoredToolGrant::decode(&grant_id, &bytes)?;
        let issued_here = matches!(
            &row.record.issuer,
            ToolGrantIssuer::Approval { approval_id } if approval_id == decision_id.as_str()
        );
        Ok(issued_here.then_some(row))
    }

    /// Issue the grant the approved `tool_grant.issue` decision `decision_id`
    /// holds (`body`), at the clock reading `now_ms`: under the id the
    /// decision derives, issuer [`ToolGrantIssuer::Approval`] with the
    /// decision's id, for `[now_ms, now_ms + duration)`, scoped as the
    /// request asked. The grant the decision already issued is answered as it
    /// stands, so a replayed execution meets it.
    ///
    /// # Errors
    /// As [`Self::grant`]; [`ToolGrantServiceError::Conflict`] when another
    /// grant holds the id the decision derives.
    pub(crate) async fn issue_approved_at(
        &self,
        decision_id: &DecisionId,
        body: &ToolGrantIssueBody,
        now_ms: u64,
    ) -> Result<StoredToolGrant, ToolGrantServiceError> {
        if body.target.names().next().is_none()
            || body.target.names().any(|name| name.as_str().is_empty())
        {
            return Err(ToolGrantServiceError::EmptyTarget);
        }
        let grant_id = ToolGrantId::approved(decision_id);
        let request = ToolGrantRequest {
            subject: body.subject.clone(),
            target: body.target.clone(),
            issuer: ToolGrantIssuer::Approval {
                approval_id: decision_id.to_string(),
            },
            reason: body.reason.clone(),
            window: ToolGrantWindow::For(Duration::from_millis(body.duration_ms)),
            run_scope: body.run_scope.clone(),
        };
        match self.issue(request, grant_id.clone(), now_ms).await {
            Err(ToolGrantServiceError::Conflict { .. }) => self
                .issued_for(decision_id)
                .await?
                .ok_or(ToolGrantServiceError::Conflict {
                    grant_id: grant_id.to_string(),
                }),
            issued => issued,
        }
    }

    /// Issue `request` as the grant `grant_id`, at the clock reading
    /// `now_ms`: the machine's `Issue`, then one store write of the row and
    /// its `granted` audit row. A row already stored under the id is a
    /// [`ToolGrantServiceError::Conflict`].
    async fn issue(
        &self,
        request: ToolGrantRequest,
        grant_id: ToolGrantId,
        now_ms: u64,
    ) -> Result<StoredToolGrant, ToolGrantServiceError> {
        let expires_at_ms = match request.window {
            ToolGrantWindow::For(lifetime) => {
                now_ms.saturating_add(u64::try_from(lifetime.as_millis()).unwrap_or(u64::MAX))
            }
            ToolGrantWindow::Until { expires_at_ms } => expires_at_ms,
        };
        let max_lifetime_ms = self.policy().max_lifetime_ms();
        let mut authority = ToolGrantLifecycleMachineAuthority::new();
        let effects = ToolGrantLifecycleMachineMutator::apply(
            &mut authority,
            ToolGrantLifecycleInput::Issue {
                grant_id: grant_id.to_string(),
                now_ms,
                expires_at_ms,
                max_lifetime_ms,
                run_scope: request.run_scope.as_ref().map(run_key),
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
                run_scope: row.run_scope.clone(),
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
    /// answered as it stands. The answer says whether this revoke changed
    /// the grant: its input's effects ended it (`Revoked`, or `Expired` for a
    /// revoke at or after the deadline).
    ///
    /// # Errors
    /// [`ToolGrantServiceError::NotFound`], [`ToolGrantServiceError::CorruptRow`]
    /// for a row this binary cannot read, and store and clock failures.
    pub async fn revoke(
        &self,
        grant_id: &ToolGrantId,
        revocation: ToolGrantRevocation,
    ) -> Result<RevokedToolGrant, ToolGrantServiceError> {
        let store = self.store.as_ref();
        let read = load_row(store, &grant_id.to_string()).await?;
        let now_ms = self.now_ms()?;
        let (grant, effects) = apply_and_commit(
            store,
            read,
            &|| ToolGrantLifecycleInput::Revoke { now_ms },
            AuditContext::Revocation(&revocation),
        )
        .await?;
        let changed = effects.iter().any(|effect| {
            matches!(
                effect,
                ToolGrantLifecycleEffect::Revoked { .. } | ToolGrantLifecycleEffect::Expired { .. }
            )
        });
        Ok(RevokedToolGrant { grant, changed })
    }

    /// Revoke every live grant of `subject`: its owner ended the subject (a
    /// mob member identity whose identity ended). Each grant is revoked as
    /// [`Self::revoke`] revokes one: a grant at or after its deadline records
    /// the expiry it reached instead, and a grant that already ended is left
    /// as it stands. A row this binary cannot read admits nothing and is left
    /// as it is, logged. A store without the grant domain holds no grant, so
    /// there it ends none. Answers how many grants this call ended.
    ///
    /// # Errors
    /// Store, clock and conflict failures; a grant the call did not reach
    /// stays live, so the caller retries the whole end.
    pub async fn revoke_subject(
        &self,
        subject: &ToolGrantSubjectKey,
        revocation: ToolGrantRevocation,
    ) -> Result<usize, ToolGrantServiceError> {
        let store = self.store.as_ref();
        let rows = match subject_rows(store, std::slice::from_ref(subject)).await {
            Ok(rows) => rows,
            Err(ToolGrantServiceError::Store(RuntimeStoreError::Unsupported(_))) => return Ok(0),
            Err(error) => return Err(error),
        };
        rows.warn_unreadable("revoke_subject");
        let mut ended = 0;
        for read in rows.readable {
            if read.row.status != ToolGrantStatus::Active {
                continue;
            }
            let now_ms = self.now_ms()?;
            let effects = match apply_and_commit(
                store,
                read,
                &|| ToolGrantLifecycleInput::Revoke { now_ms },
                AuditContext::Revocation(&revocation),
            )
            .await
            {
                Ok((_, effects)) => effects,
                // A row whose machine refuses to rebuild admits nothing.
                Err(ToolGrantServiceError::CorruptRow { grant_id, detail }) => {
                    tracing::warn!(
                        %grant_id,
                        %detail,
                        adjudication = "revoke_subject",
                        "an unreadable tool grant row admits nothing"
                    );
                    continue;
                }
                Err(error) => return Err(error),
            };
            if effects.iter().any(|effect| {
                matches!(
                    effect,
                    ToolGrantLifecycleEffect::Revoked { .. }
                        | ToolGrantLifecycleEffect::Expired { .. }
                )
            }) {
                ended += 1;
            }
        }
        Ok(ended)
    }

    /// One grant as its row stands, read without settling it.
    ///
    /// # Errors
    /// [`ToolGrantServiceError::NotFound`], [`ToolGrantServiceError::CorruptRow`]
    /// for a row this binary cannot read, and store failures.
    pub async fn load(
        &self,
        grant_id: &ToolGrantId,
    ) -> Result<StoredToolGrant, ToolGrantServiceError> {
        Ok(load_row(self.store.as_ref(), &grant_id.to_string())
            .await?
            .row)
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
    /// expired first, so the log records its expiry. A row this binary
    /// cannot read (written by a newer binary, or damaged) is reported in
    /// the page's `unreadable` and logged, and the rest of the page is
    /// served.
    ///
    /// # Errors
    /// Store and clock failures.
    pub async fn audit(
        &self,
        subject: &ToolGrantSubject,
        after: Option<u64>,
        limit: usize,
    ) -> Result<ToolGrantAuditPage, ToolGrantServiceError> {
        let key = subject.key();
        self.settle(&key, ToolGrantExpiryDriver::Read).await?;
        let mut page = ToolGrantAuditPage::default();
        for (sequence, bytes) in self
            .store
            .read_tool_grant_audit(key.as_str(), after, limit)
            .await?
        {
            match ToolGrantAuditRow::decode(&bytes) {
                Ok(row) => page.rows.push(ToolGrantAuditRecord { sequence, row }),
                Err(error) => {
                    tracing::warn!(
                        subject = %key,
                        sequence,
                        %error,
                        "an unreadable tool grant audit row is reported, not served"
                    );
                    page.unreadable.push(UnreadableToolGrantAuditRow {
                        sequence,
                        detail: error.to_string(),
                    });
                }
            }
        }
        Ok(page)
    }

    /// Settle `subject_key`'s due grants as expired, observed by `driver`,
    /// and answer every readable grant with the next deadline of the grants
    /// still live. A grant is live when its own `AdmitUse` at the reading
    /// (of its own run, for a run-scoped grant) admits: an ended grant arms
    /// no deadline.
    pub(crate) async fn settle(
        &self,
        subject_key: &ToolGrantSubjectKey,
        driver: ToolGrantExpiryDriver,
    ) -> Result<SettledGrants, ToolGrantServiceError> {
        let store = self.store.as_ref();
        let rows = subject_rows(store, std::slice::from_ref(subject_key)).await?;
        rows.warn_unreadable("settle");
        let now_ms = self.now_ms()?;
        let mut grants = Vec::with_capacity(rows.readable.len());
        let mut unreadable = rows.unreadable;
        let mut next_deadline_ms: Option<u64> = None;
        for read in rows.readable {
            let (row, _) = match apply_and_commit(
                store,
                read,
                &|| ToolGrantLifecycleInput::Expire { now_ms },
                AuditContext::Observed(driver),
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
            let own_use = ToolGrantLifecycleInput::AdmitUse {
                now_ms,
                run_id: row.run_scope.as_ref().map(run_key),
            };
            let live = matches!(
                apply(&row, own_use).and_then(|applied| use_verdict(&row, &applied.effects)),
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

    /// A model-request boundary of the run `run_id`: the gated names of
    /// `gated` a live grant of `subjects` opens at this moment. A grant the
    /// boundary finds past its deadline is settled as expired, and a grant
    /// scoped to an earlier run is settled as ended with it; a row this
    /// binary cannot read opens nothing.
    ///
    /// # Errors
    /// `Unavailable` when the grants cannot be read or settled: the gate
    /// stays closed.
    pub(crate) async fn adjudicate_boundary(
        &self,
        subjects: &[ToolGrantSubjectKey],
        gated: &GatedToolTable,
        run_id: Option<&RunId>,
    ) -> Result<BoundaryAdjudication, ToolGrantRefusal> {
        let store = self.store.as_ref();
        let rows = subject_rows(store, subjects)
            .await
            .map_err(|error| unavailable(&error))?;
        rows.warn_unreadable("boundary");
        let now_ms = self.now_ms().map_err(|error| unavailable(&error))?;
        let run_id = run_id.map(run_key);
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
            let (row, effects) = match apply_and_commit(
                store,
                read,
                &|| ToolGrantLifecycleInput::AdmitUse {
                    now_ms,
                    run_id: run_id.clone(),
                },
                AuditContext::Observed(ToolGrantExpiryDriver::Boundary),
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
                adjudication
                    .next_deadlines
                    .entry(row.record.subject.key())
                    .and_modify(|next| *next = (*next).min(row.expires_at_ms))
                    .or_insert(row.expires_at_ms);
            }
        }
        Ok(adjudication)
    }

    /// One gated call's admission. The grants are read first; the clock is
    /// read after that read, and an admission returns with nothing awaited
    /// after the clock reading, so the caller hands the call on at the
    /// moment it was adjudicated. A refusal settles any grant it found past
    /// its deadline, or scoped to an earlier run than the call's, then
    /// appends a `refused` audit row; a failed append is logged and never
    /// turns the refusal into an admission.
    ///
    /// # Errors
    /// The refusal: no live grant covers the name (in the call's run, for a
    /// run-scoped grant), the covering grant expired or was revoked, or the
    /// grants could not be read.
    pub(crate) async fn admit_call(
        &self,
        subjects: &[ToolGrantSubjectKey],
        call: ToolGrantCall<'_>,
    ) -> Result<(), ToolGrantRefusal> {
        let store = self.store.as_ref();
        let rows = match subject_rows(store, subjects).await {
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
        let run_id = call.run_id.map(run_key);
        let call_use = || ToolGrantLifecycleInput::AdmitUse {
            now_ms,
            run_id: run_id.clone(),
        };
        // Adjudicate every covering grant at this one reading before
        // anything else awaits: one admitted grant admits the call.
        let mut observed_ends = Vec::new();
        let mut refusal = ToolGrantRefusal::Required;
        // The subject whose log records the refusal: the explaining grant's,
        // else the session's.
        let mut refusal_subject = subjects.first().cloned();
        for read in rows.readable {
            if !read.row.record.target.covers(call.name) {
                continue;
            }
            let verdict = apply(&read.row, call_use()).and_then(|applied| {
                use_verdict(&read.row, &applied.effects)
                    .map(|verdict| (verdict, applied.next.is_some()))
            });
            let (found, observed_end) = match verdict {
                Ok((Ok(()), _)) => return Ok(()),
                Ok((Err(found), observed_end)) => (found, observed_end),
                Err(error) => (unavailable(&error), false),
            };
            if refusal_rank(&found) > refusal_rank(&refusal) {
                refusal = found;
                refusal_subject = Some(read.row.record.subject.key());
            }
            if observed_end {
                observed_ends.push(read);
            }
        }
        // Refused: settle the ends this adjudication observed.
        for read in observed_ends {
            let grant_id = read.grant_id.clone();
            if let Err(error) = apply_and_commit(
                store,
                read,
                &call_use,
                AuditContext::Observed(ToolGrantExpiryDriver::Dispatch),
            )
            .await
            {
                tracing::warn!(
                    %grant_id,
                    %error,
                    "a gated dispatch could not record the end of a tool grant"
                );
            }
        }
        self.record_refusal(refusal_subject.as_ref(), call, &refusal)
            .await;
        Err(refusal)
    }

    /// The run `run_id` of the subjects' session ended, as `driver` observed
    /// it: end every grant of `subjects` scoped to that run. A grant bounded
    /// by its window only, a grant of another run and an ended grant are
    /// left as they are; a grant whose deadline already passed records its
    /// expiry instead. A row this binary cannot read is logged and left.
    ///
    /// # Errors
    /// Store and clock failures, and a row another writer kept changing.
    pub async fn end_run(
        &self,
        subjects: &[ToolGrantSubjectKey],
        run_id: &RunId,
        driver: ToolGrantExpiryDriver,
    ) -> Result<(), ToolGrantServiceError> {
        let clock = Arc::clone(
            &*self
                .clock
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        end_run_over(
            self.store.as_ref(),
            clock.as_ref(),
            subjects,
            run_id,
            driver,
        )
        .await
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
}

/// End every grant of `subjects` in `store` scoped to the run `run_id`, at
/// one reading of `clock` taken after the rows are read; see
/// [`ToolGrantService::end_run`]. Durable-tail recovery calls it over the
/// store it recovers, for the run it closed.
pub(crate) async fn end_run_over(
    store: &dyn RuntimeStore,
    clock: &dyn ToolGrantClock,
    subjects: &[ToolGrantSubjectKey],
    run_id: &RunId,
    driver: ToolGrantExpiryDriver,
) -> Result<(), ToolGrantServiceError> {
    let rows = subject_rows(store, subjects).await?;
    rows.warn_unreadable("run end");
    let now_ms = clock.now_ms().map_err(|_| ToolGrantServiceError::Clock)?;
    let run_id = run_key(run_id);
    for read in rows.readable {
        match apply_and_commit(
            store,
            read,
            &|| ToolGrantLifecycleInput::EndRun {
                run_id: run_id.clone(),
                now_ms,
            },
            AuditContext::Observed(driver),
        )
        .await
        {
            Ok(_) => {}
            // A row whose machine refuses to rebuild ends nothing; it admits
            // nothing either.
            Err(ToolGrantServiceError::CorruptRow { grant_id, detail }) => {
                tracing::warn!(
                    %grant_id,
                    %detail,
                    adjudication = "run end",
                    "an unreadable tool grant row admits nothing"
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// End the run-scoped grants of the run `run_id` of `session_id`, which
/// durable-tail recovery closed in `store` (it committed the run's tail as
/// interrupted or completed), at the host clock. Best effort: a failure is
/// logged, and the grants then admit no use of a later run either, whose
/// first use ends them.
pub(crate) async fn end_recovered_run(
    store: &dyn RuntimeStore,
    session_id: &meerkat_core::types::SessionId,
    run_id: &RunId,
) {
    let subject = ToolGrantSubject::session(session_id.clone()).key();
    if let Err(error) = end_run_over(
        store,
        &HostToolGrantClock,
        std::slice::from_ref(&subject),
        run_id,
        ToolGrantExpiryDriver::Recovery,
    )
    .await
    {
        // A store without tool grants has none to end.
        if matches!(
            error,
            ToolGrantServiceError::Store(RuntimeStoreError::Unsupported(_))
        ) {
            return;
        }
        tracing::warn!(
            %session_id,
            %run_id,
            %error,
            "recovery could not end the run-scoped tool grants of the run it closed"
        );
    }
}

async fn load_row(
    store: &dyn RuntimeStore,
    grant_id: &str,
) -> Result<ReadRow, ToolGrantServiceError> {
    let bytes =
        store
            .load_tool_grant(grant_id)
            .await?
            .ok_or_else(|| ToolGrantServiceError::NotFound {
                grant_id: grant_id.to_string(),
            })?;
    let row = StoredToolGrant::decode(grant_id, &bytes)?;
    Ok(ReadRow {
        grant_id: grant_id.to_string(),
        bytes,
        row,
    })
}

/// Every row filed under `subjects`: the readable ones, and the ones this
/// binary cannot decode, which admit nothing and block nothing.
async fn subject_rows(
    store: &dyn RuntimeStore,
    subjects: &[ToolGrantSubjectKey],
) -> Result<SubjectRows, ToolGrantServiceError> {
    let mut rows = SubjectRows {
        readable: Vec::new(),
        unreadable: Vec::new(),
    };
    for subject in subjects {
        for (grant_id, bytes) in store.list_tool_grants(subject.as_str()).await? {
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

/// Apply one input to a row's rebuilt machine and commit what its effects
/// record: the row after an `Expired`, `Revoked` or `RunEnded` effect, with
/// that effect's audit row, in one store write. A CAS conflict (another
/// writer changed the row first) reloads the row and applies the input once
/// more; a second conflict is [`ToolGrantServiceError::Conflict`].
///
/// Returns the row as it now stands and the effects of the input as last
/// applied.
async fn apply_and_commit(
    store: &dyn RuntimeStore,
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
        if store
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
            read = load_row(store, &read.grant_id).await?;
        }
    }
    Err(ToolGrantServiceError::Conflict {
        grant_id: read.grant_id,
    })
}
