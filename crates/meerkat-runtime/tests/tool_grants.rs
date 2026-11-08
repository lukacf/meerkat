#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Scoped tool grants through the runtime: the grant service over a store,
//! a served session's grant gate on its runtime bindings, the visibility
//! owner's gate layer and the deadline driver, and grant requests a person
//! approves (`tool_grant.issue` decisions and their executor). Every test
//! runs on the in-memory store and on SQLite, over a host clock the test
//! moves.
//!
//! A served session here is its runtime bindings and the tool scope built
//! over their visibility owner, with the build's grant gate installed: what
//! the factory composes for a gated build. Its boundary is the agent's: the
//! gate's open set, or a closed gate when the grants cannot be read.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use meerkat_core::ToolScope;
use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentEligibility, ConsentEligibilityAnswer, ConsentIneligible,
};
use meerkat_core::approval::decision::{
    ApproverId, AttestedDecision, AttributedReview, DecisionHoldRequest, DecisionId,
    DecisionIngress, DecisionKind, DecisionOperationId, DecisionPhase, DecisionPrincipalKey,
    DecisionReference, DecisionRenderer, DecisionRendering, DecisionRequestBody,
    DecisionReviewVerdict, DecisionSubjectKey, InMemoryDecisionStore, PresentationAudience,
    PresentationRecord, Sha256Digest, StoredDecision,
};
use meerkat_core::approval::{ApprovalAssurance, ApprovalConsentDeliveryState};
use meerkat_core::session_owner_wake::{
    SessionOwnerWakeFuture, SessionOwnerWakeOutcome, SessionOwnerWakeRegistry, SessionOwnerWaker,
};
use meerkat_core::tool_grant::{
    GatedToolTable, ToolGrantAgentRequest, ToolGrantAgentRequestRefusal, ToolGrantCall,
    ToolGrantGate, ToolGrantId, ToolGrantIssuer, ToolGrantRefusal, ToolGrantSubject,
    ToolGrantTarget,
};
use meerkat_core::types::{SessionId, ToolDef, ToolName, ToolProvenance, ToolSourceKind};
use meerkat_core::{PrincipalKind, PrincipalRef, RunId, TrustDomainId};
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::decision::{
    DecisionApproverCurrentness, DecisionClock, DecisionClockError, DecisionExecution,
    DecisionExecutor, DecisionExecutorLoop, DecisionExecutorPass, DecisionPassOutcome,
    DurableDecisionService,
};
use meerkat_runtime::store::{InMemoryRuntimeStore, RuntimeStore, SqliteRuntimeStore};
use meerkat_runtime::tool_grant::{
    RuntimeSessionToolGrants, StoredToolGrant, TOOL_GRANT_ISSUE_BODY, TOOL_GRANT_ISSUE_KIND,
    TOOL_GRANT_ISSUE_RENDERER, ToolGrantAuditEvent, ToolGrantAuditRow, ToolGrantClock,
    ToolGrantClockError, ToolGrantExpiryDriver, ToolGrantIssueBody, ToolGrantIssueExecutor,
    ToolGrantIssueRequest, ToolGrantRefusalKind, ToolGrantRequest, ToolGrantRequestHost,
    ToolGrantRequestNotice, ToolGrantRequestNotices, ToolGrantRequestOrigin,
    ToolGrantRequestOutcome, ToolGrantRequestPolicy, ToolGrantRevocation, ToolGrantService,
    ToolGrantServiceError, ToolGrantStatus, ToolGrantSubjectObserver, ToolGrantSubjectPresence,
    ToolGrantWindow, hold_tool_grant_request, tool_grant_requests_of,
};

/// The host clock reading every test starts at, in UNIX ms.
const T0: u64 = 1_800_000_000_000;

/// A host clock the test moves.
#[derive(Debug)]
struct FakeClock(AtomicU64);

impl FakeClock {
    fn at(now_ms: u64) -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(now_ms)))
    }

    fn set(&self, now_ms: u64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }
}

impl ToolGrantClock for FakeClock {
    fn now_ms(&self) -> Result<u64, ToolGrantClockError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

#[derive(Debug, Clone, Copy)]
enum Backend {
    Memory,
    Sqlite,
}

const BACKENDS: [Backend; 2] = [Backend::Memory, Backend::Sqlite];

/// A store a test can open again, as a restarted process or another
/// process would: the in-memory store's shared state, or a new SQLite store
/// over the same file.
struct Durable {
    backend: Backend,
    memory: InMemoryRuntimeStore,
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl Durable {
    fn new(backend: Backend) -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("runtime.sqlite3");
        Self {
            backend,
            memory: InMemoryRuntimeStore::new(),
            _dir: dir,
            path,
        }
    }

    fn open(&self) -> Arc<dyn RuntimeStore> {
        match self.backend {
            Backend::Memory => Arc::new(self.memory.clone()),
            Backend::Sqlite => Arc::new(SqliteRuntimeStore::new(self.path.clone()).unwrap()),
        }
    }

    /// A runtime over the store, reading `clock`.
    fn machine(&self, clock: &Arc<FakeClock>) -> Arc<MeerkatMachine> {
        let machine = MeerkatMachine::persistent_without_blobs(self.open()).unwrap();
        machine.set_tool_grant_clock(Arc::clone(clock) as Arc<dyn ToolGrantClock>);
        Arc::new(machine)
    }
}

fn tool(name: &str) -> Arc<ToolDef> {
    Arc::new(
        ToolDef::new(
            name,
            format!("{name} fixture"),
            serde_json::json!({ "type": "object" }),
        )
        .with_provenance(ToolProvenance {
            kind: ToolSourceKind::Callback,
            source_id: name.into(),
            artifact: None,
        }),
    )
}

/// The names the `shell` family provides.
fn shell_family() -> BTreeSet<ToolName> {
    ["shell", "shell_job_status"]
        .into_iter()
        .map(ToolName::from)
        .collect()
}

/// The build's gate: the `shell` family and the single tool `deploy`.
fn gate_table() -> GatedToolTable {
    GatedToolTable::new(
        BTreeMap::from([("shell".to_string(), shell_family())]),
        BTreeSet::from([ToolName::from("deploy")]),
    )
}

fn shell_grant(session_id: &SessionId, lifetime: Duration) -> ToolGrantRequest {
    ToolGrantRequest {
        subject: ToolGrantSubject::session(session_id.clone()),
        target: ToolGrantTarget::Family {
            family: "shell".to_string(),
            names: shell_family(),
        },
        issuer: ToolGrantIssuer::Operator {
            caller: Some("operator".to_string()),
            decision_ref: None,
        },
        reason: Some("maintenance".to_string()),
        window: ToolGrantWindow::For(lifetime),
        run_scope: None,
    }
}

/// A served session: its tool scope over the runtime's visibility owner,
/// and its grant gate over the runtime's grants.
struct Served {
    scope: ToolScope,
    gate: ToolGrantGate,
}

async fn serve(machine: &MeerkatMachine, session_id: &SessionId) -> Served {
    let bindings = machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings prepare");
    let grants = Arc::clone(
        bindings
            .tool_grants()
            .expect("the runtime carries the session's grants"),
    );
    let tools: Arc<[Arc<ToolDef>]> = ["read_file", "shell", "shell_job_status", "deploy"]
        .into_iter()
        .map(tool)
        .collect();
    let scope = ToolScope::new_with_visibility_owner(
        tools,
        HashSet::new(),
        HashSet::new(),
        bindings.tool_visibility_owner().clone(),
    )
    .expect("tool scope");
    let table = gate_table();
    scope
        .replace_tool_grant_gate(table.gated_names())
        .expect("the build installs its gate");
    Served {
        scope,
        gate: ToolGrantGate::new(grants, table),
    }
}

impl Served {
    /// One model-request boundary, as the agent runs it, outside any run: a
    /// window grant opens its names there, a run-scoped grant does not.
    async fn boundary(&self) -> Result<(), ToolGrantRefusal> {
        match self.gate.open_names(None).await {
            Ok(open) => {
                self.scope
                    .apply_tool_grant_open_set(open.into_inner().into_iter().collect())
                    .expect("the open set applies");
                Ok(())
            }
            Err(refusal) => {
                self.scope
                    .apply_tool_grant_open_set(BTreeSet::new())
                    .expect("the empty set closes the gate");
                Err(refusal)
            }
        }
    }

    fn visible(&self) -> Vec<String> {
        self.scope
            .visible_tool_names()
            .unwrap()
            .into_iter()
            .map(|name| name.as_str().to_string())
            .collect()
    }

    /// One gated call's admission, as the dispatch gate asks it.
    async fn call(&self, name: &str, call_id: &str) -> Result<(), ToolGrantRefusal> {
        self.gate
            .admit_call(ToolGrantCall {
                name,
                call_id,
                run_id: None,
            })
            .await
    }
}

const OPEN: [&str; 3] = ["read_file", "shell", "shell_job_status"];
const CLOSED: [&str; 1] = ["read_file"];

/// The grant's row as stored, read without settling it.
async fn stored(store: &dyn RuntimeStore, grant: &StoredToolGrant) -> StoredToolGrant {
    let bytes = store
        .load_tool_grant(&grant.record.grant_id.to_string())
        .await
        .unwrap()
        .expect("the grant's row");
    serde_json::from_slice(&bytes).unwrap()
}

/// The session's audit rows as stored, read without settling anything.
async fn audit_events(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
) -> Vec<ToolGrantAuditEvent> {
    let key = ToolGrantSubject::session(session_id.clone()).key();
    store
        .read_tool_grant_audit(key.as_str(), None, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|(_, bytes)| ToolGrantAuditRow::decode(&bytes).unwrap().event)
        .collect()
}

/// A session owner that counts the wakes it is asked for.
struct CountingOwner(AtomicUsize);

impl SessionOwnerWaker for CountingOwner {
    fn wake_session_owner(&self, _session_id: &SessionId) -> SessionOwnerWakeFuture {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(SessionOwnerWakeOutcome::Ready) })
    }
}

/// Route `session_id`'s wakes to a counting owner; keep it alive.
fn watch_wakes(session_id: &SessionId) -> Arc<CountingOwner> {
    let owner = Arc::new(CountingOwner(AtomicUsize::new(0)));
    let waker: Arc<dyn SessionOwnerWaker> = owner.clone();
    SessionOwnerWakeRegistry::global().publish(session_id.clone(), Arc::downgrade(&waker));
    owner
}

#[tokio::test]
async fn granted_family_is_visible_until_its_deadline_and_hidden_after() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let served = serve(&machine, &session_id).await;

        served.boundary().await.unwrap();
        assert_eq!(
            served.visible(),
            CLOSED,
            "{backend:?}: gated names start closed"
        );

        let grant = machine
            .grant_tool(shell_grant(&session_id, Duration::from_mins(1)), None)
            .await
            .unwrap();
        assert_eq!(grant.status, ToolGrantStatus::Active);
        assert_eq!(grant.expires_at_ms, T0 + 60_000);
        served.boundary().await.unwrap();
        assert_eq!(
            served.visible(),
            OPEN,
            "{backend:?}: the grant opens its family"
        );

        clock.set(grant.expires_at_ms - 1);
        served.boundary().await.unwrap();
        assert_eq!(
            served.visible(),
            OPEN,
            "{backend:?}: one millisecond before the deadline the family is open"
        );

        clock.set(grant.expires_at_ms);
        served.boundary().await.unwrap();
        assert_eq!(
            served.visible(),
            CLOSED,
            "{backend:?}: at the deadline the family closes"
        );
        let listed = machine
            .list_tool_grants(&ToolGrantSubject::session(session_id.clone()), None)
            .await
            .unwrap();
        assert_eq!(listed.grants.len(), 1);
        assert_eq!(listed.grants[0].status, ToolGrantStatus::Expired);
        assert_eq!(listed.grants[0].ended_at_ms, Some(grant.expires_at_ms));
    }
}

#[tokio::test]
async fn dispatch_after_the_deadline_is_refused_although_the_boundary_showed_the_tool() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let served = serve(&machine, &session_id).await;
        let grant = machine
            .grant_tool(shell_grant(&session_id, Duration::from_mins(1)), None)
            .await
            .unwrap();
        served.boundary().await.unwrap();
        assert_eq!(served.visible(), OPEN);

        // The boundary showed the tool; the call is adjudicated at the
        // deadline.
        clock.set(grant.expires_at_ms);
        let refusal = served
            .call("shell", "call-at-deadline")
            .await
            .expect_err("a call adjudicated at the deadline is refused");
        assert_eq!(
            refusal,
            ToolGrantRefusal::Expired {
                grant_id: grant.record.grant_id.clone(),
                expires_at_ms: grant.expires_at_ms,
            },
            "{backend:?}"
        );
        assert_eq!(
            served.visible(),
            OPEN,
            "{backend:?}: no boundary ran since the one that showed the tool"
        );

        let store = durable.open();
        assert_eq!(
            stored(&*store, &grant).await.status,
            ToolGrantStatus::Expired,
            "{backend:?}: the dispatch recorded the expiry it observed"
        );
        let events = audit_events(&*store, &session_id).await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ToolGrantAuditEvent::Granted { .. },
                    ToolGrantAuditEvent::Expired {
                        driver: ToolGrantExpiryDriver::Dispatch,
                        ..
                    },
                    ToolGrantAuditEvent::Refused {
                        reason: ToolGrantRefusalKind::Expired,
                        ..
                    },
                ]
            ),
            "{backend:?}: {events:?}"
        );
    }
}

#[tokio::test]
async fn expired_grant_stays_expired_across_restart_and_clock_rollback() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let served = serve(&machine, &session_id).await;
        let grant = machine
            .grant_tool(shell_grant(&session_id, Duration::from_mins(1)), None)
            .await
            .unwrap();
        clock.set(grant.expires_at_ms + 5);
        served.boundary().await.unwrap();
        assert_eq!(served.visible(), CLOSED);
        drop(served);
        machine.unregister_session(&session_id).await.unwrap();
        drop(machine);

        // A restarted process whose clock rolled back before the deadline.
        clock.set(grant.expires_at_ms - 30_000);
        let machine = durable.machine(&clock);
        let served = serve(&machine, &session_id).await;
        served.boundary().await.unwrap();
        assert_eq!(
            served.visible(),
            CLOSED,
            "{backend:?}: an expired grant opens nothing after a rollback"
        );
        let refusal = served
            .call("shell", "call-after-rollback")
            .await
            .expect_err("an expired grant admits nothing after a rollback");
        assert!(
            matches!(refusal, ToolGrantRefusal::Expired { .. }),
            "{backend:?}: {refusal:?}"
        );
        let listed = machine
            .list_tool_grants(&ToolGrantSubject::session(session_id.clone()), None)
            .await
            .unwrap();
        assert_eq!(listed.grants[0].status, ToolGrantStatus::Expired);
        assert_eq!(
            listed.grants[0].ended_at_ms,
            Some(grant.expires_at_ms + 5),
            "{backend:?}: the expiry keeps the reading that observed it"
        );
    }
}

#[tokio::test]
async fn live_grant_keeps_its_deadline_across_restart() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let served = serve(&machine, &session_id).await;
        let grant = machine
            .grant_tool(shell_grant(&session_id, Duration::from_mins(1)), None)
            .await
            .unwrap();
        served.boundary().await.unwrap();
        assert_eq!(served.visible(), OPEN);
        drop(served);
        machine.unregister_session(&session_id).await.unwrap();
        drop(machine);

        clock.set(T0 + 30_000);
        let machine = durable.machine(&clock);
        let listed = machine
            .list_tool_grants(&ToolGrantSubject::session(session_id.clone()), None)
            .await
            .unwrap();
        assert_eq!(
            listed.grants[0].status,
            ToolGrantStatus::Active,
            "{backend:?}"
        );
        assert_eq!(
            listed.grants[0].expires_at_ms, grant.expires_at_ms,
            "{backend:?}"
        );

        let served = serve(&machine, &session_id).await;
        assert_eq!(
            served.visible(),
            CLOSED,
            "{backend:?}: a restored gate starts closed until its first boundary"
        );
        served.boundary().await.unwrap();
        assert_eq!(served.visible(), OPEN, "{backend:?}");
        served.call("shell", "call-after-restart").await.unwrap();

        clock.set(grant.expires_at_ms);
        served.boundary().await.unwrap();
        assert_eq!(
            served.visible(),
            CLOSED,
            "{backend:?}: the deadline did not move"
        );
    }
}

#[tokio::test]
async fn listing_an_unserved_session_settles_due_grants_without_registering_it() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let wakes = watch_wakes(&session_id);
        let grant = machine
            .grant_tool(shell_grant(&session_id, Duration::from_mins(1)), None)
            .await
            .unwrap();
        assert_eq!(
            machine.tool_grant_deadlines().armed_deadline(
                &session_id,
                &ToolGrantSubject::session(session_id.clone()).key()
            ),
            None,
            "{backend:?}: an unserved session is not armed"
        );

        clock.set(grant.expires_at_ms + 1);
        let subject = ToolGrantSubject::session(session_id.clone());
        let listed = machine.list_tool_grants(&subject, None).await.unwrap();
        assert_eq!(
            listed.grants[0].status,
            ToolGrantStatus::Expired,
            "{backend:?}"
        );
        assert_eq!(listed.grants[0].ended_at_ms, Some(grant.expires_at_ms + 1));
        let audit = machine
            .read_tool_grant_audit(&subject, None, 10)
            .await
            .unwrap();
        assert!(
            matches!(
                audit
                    .rows
                    .iter()
                    .map(|record| &record.row.event)
                    .collect::<Vec<_>>()
                    .as_slice(),
                [
                    ToolGrantAuditEvent::Granted { .. },
                    ToolGrantAuditEvent::Expired {
                        driver: ToolGrantExpiryDriver::Read,
                        ..
                    },
                ]
            ),
            "{backend:?}: {audit:?}"
        );
        assert!(
            !machine.contains_session(&session_id).await,
            "{backend:?}: nothing registered the session"
        );
        assert_eq!(
            wakes.0.load(Ordering::SeqCst),
            0,
            "{backend:?}: nothing woke the session's owner"
        );
    }
}

#[tokio::test]
async fn deadline_timer_expires_served_session_grants_without_waking() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let _served = serve(&machine, &session_id).await;
        let wakes = watch_wakes(&session_id);
        let grant = machine
            .grant_tool(shell_grant(&session_id, Duration::from_millis(200)), None)
            .await
            .unwrap();
        assert_eq!(
            machine.tool_grant_deadlines().armed_deadline(
                &session_id,
                &ToolGrantSubject::session(session_id.clone()).key()
            ),
            Some(grant.expires_at_ms),
            "{backend:?}: a grant on a served session arms its deadline"
        );

        // The deadline passes with no boundary, dispatch or read.
        clock.set(grant.expires_at_ms);
        let store = durable.open();
        let mut expired = None;
        for _ in 0..200 {
            let row = stored(&*store, &grant).await;
            if row.status == ToolGrantStatus::Expired {
                expired = Some(row);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let expired = expired.expect("the deadline timer expires the grant");
        assert_eq!(expired.ended_at_ms, Some(grant.expires_at_ms));
        let events = audit_events(&*store, &session_id).await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ToolGrantAuditEvent::Granted { .. },
                    ToolGrantAuditEvent::Expired {
                        driver: ToolGrantExpiryDriver::Timer,
                        ..
                    },
                ]
            ),
            "{backend:?}: {events:?}"
        );
        assert_eq!(
            wakes.0.load(Ordering::SeqCst),
            0,
            "{backend:?}: the timer woke nothing"
        );
        assert_eq!(
            machine.tool_grant_deadlines().armed_deadline(
                &session_id,
                &ToolGrantSubject::session(session_id.clone()).key()
            ),
            None,
            "{backend:?}: no live grant is left to arm"
        );
    }
}

#[tokio::test]
async fn revoke_through_another_service_instance_is_refused_at_the_next_dispatch() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let served = serve(&machine, &session_id).await;
        let grant = machine
            .grant_tool(shell_grant(&session_id, Duration::from_mins(1)), None)
            .await
            .unwrap();
        served.boundary().await.unwrap();
        served.call("shell", "call-before-revoke").await.unwrap();

        // Another process's grant service over the same store.
        let other = ToolGrantService::new(durable.open());
        other.set_clock(Arc::clone(&clock) as Arc<dyn ToolGrantClock>);
        let revoked = other
            .revoke(
                &grant.record.grant_id,
                ToolGrantRevocation {
                    actor: Some("operator".to_string()),
                    reason: Some("window closed early".to_string()),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            revoked.grant.status,
            ToolGrantStatus::Revoked,
            "{backend:?}"
        );
        assert!(revoked.changed, "{backend:?}: this revoke ended the grant");

        let refusal = served
            .call("shell", "call-after-revoke")
            .await
            .expect_err("the next dispatch refuses a grant revoked elsewhere");
        assert_eq!(
            refusal,
            ToolGrantRefusal::Revoked {
                grant_id: grant.record.grant_id.clone(),
            },
            "{backend:?}"
        );
        served.boundary().await.unwrap();
        assert_eq!(served.visible(), CLOSED, "{backend:?}");
    }
}

#[tokio::test]
async fn lifecycle_audit_rows_commit_with_the_row_and_refusals_append() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let subject = ToolGrantSubject::session(session_id.clone());
        let served = serve(&machine, &session_id).await;
        let store = durable.open();

        let grant = machine
            .grant_tool(shell_grant(&session_id, Duration::from_mins(1)), None)
            .await
            .unwrap();
        assert_eq!(stored(&*store, &grant).await, grant);
        assert!(
            matches!(
                audit_events(&*store, &session_id).await.as_slice(),
                [ToolGrantAuditEvent::Granted { .. }]
            ),
            "{backend:?}: the grant's row and its audit row commit together"
        );

        served.call("shell", "call-admitted").await.unwrap();
        assert_eq!(
            audit_events(&*store, &session_id).await.len(),
            1,
            "{backend:?}: an admission appends nothing"
        );

        let refusal = served
            .call("deploy", "call-ungranted")
            .await
            .expect_err("no grant covers deploy");
        assert_eq!(refusal, ToolGrantRefusal::Required, "{backend:?}");

        clock.set(T0 + 10_000);
        let revoked = machine
            .revoke_tool_grant(
                &grant.record.grant_id,
                ToolGrantRevocation {
                    actor: Some("operator".to_string()),
                    reason: Some("done".to_string()),
                },
            )
            .await
            .unwrap();
        assert!(
            revoked.changed,
            "{backend:?}: the first revoke ends the grant"
        );
        assert_eq!(
            stored(&*store, &grant).await.status,
            ToolGrantStatus::Revoked
        );
        let refusal = served
            .call("shell", "call-after-revoke")
            .await
            .expect_err("a revoked grant admits nothing");
        assert!(matches!(refusal, ToolGrantRefusal::Revoked { .. }));

        // A revoke of a grant that already ended changes nothing and records
        // nothing.
        let again = machine
            .revoke_tool_grant(&grant.record.grant_id, ToolGrantRevocation::default())
            .await
            .unwrap();
        assert_eq!(again.grant.status, ToolGrantStatus::Revoked);
        assert!(
            !again.changed,
            "{backend:?}: a second revoke changes nothing"
        );

        let grant_id = grant.record.grant_id.clone();
        let events = audit_events(&*store, &session_id).await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ToolGrantAuditEvent::Granted { .. },
                    ToolGrantAuditEvent::Refused {
                        reason: ToolGrantRefusalKind::Required,
                        grant_id: None,
                        ..
                    },
                    ToolGrantAuditEvent::Revoked {
                        observed_at_ms: 1_800_000_010_000,
                        ..
                    },
                    ToolGrantAuditEvent::Refused {
                        reason: ToolGrantRefusalKind::Revoked,
                        grant_id: Some(_),
                        ..
                    },
                ]
            ),
            "{backend:?}: {events:?}"
        );
        match &events[1] {
            ToolGrantAuditEvent::Refused { tool, call_id, .. } => {
                assert_eq!(tool.as_str(), "deploy");
                assert_eq!(call_id, "call-ungranted");
            }
            other => panic!("{backend:?}: expected the refusal row, got {other:?}"),
        }
        match &events[2] {
            ToolGrantAuditEvent::Revoked {
                grant_id: revoked,
                actor,
                reason,
                ..
            } => {
                assert_eq!(revoked, &grant_id);
                assert_eq!(actor.as_deref(), Some("operator"));
                assert_eq!(reason.as_deref(), Some("done"));
            }
            other => panic!("{backend:?}: expected the revoke row, got {other:?}"),
        }

        let page = machine
            .read_tool_grant_audit(&subject, Some(0), 2)
            .await
            .unwrap();
        assert_eq!(
            page.rows
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            [1, 2],
            "{backend:?}: the log reads from a cursor, at most a limit"
        );
    }
}

/// A row this binary cannot read (written by a newer binary, or damaged)
/// admits nothing and blocks nothing: the subject's readable live grant still
/// opens and admits its names, every other gated name stays closed, a list
/// reports the row as unreadable, and a revoke of it is refused.
#[tokio::test]
async fn an_unreadable_row_admits_nothing_and_blocks_no_readable_grant() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let subject = ToolGrantSubject::session(session_id.clone());
        let served = serve(&machine, &session_id).await;
        let grant = machine
            .grant_tool(shell_grant(&session_id, Duration::from_mins(1)), None)
            .await
            .unwrap();
        let unreadable_id = ToolGrantId::mint();
        assert!(
            durable
                .open()
                .put_tool_grant_if_absent(
                    &unreadable_id.to_string(),
                    subject.key().as_str(),
                    br#"{"written_by":"a newer binary"}"#,
                    &[],
                )
                .await
                .unwrap()
        );

        served.boundary().await.unwrap();
        assert_eq!(
            served.visible(),
            OPEN,
            "{backend:?}: the readable live grant still opens its family"
        );
        served.call("shell", "call-covered").await.unwrap();
        let refusal = served
            .call("deploy", "call-uncovered")
            .await
            .expect_err("no readable grant covers deploy");
        assert_eq!(refusal, ToolGrantRefusal::Required, "{backend:?}");

        let listed = machine.list_tool_grants(&subject, None).await.unwrap();
        assert_eq!(listed.grants.len(), 1, "{backend:?}");
        assert_eq!(listed.grants[0].record.grant_id, grant.record.grant_id);
        assert_eq!(listed.unreadable.len(), 1, "{backend:?}");
        assert_eq!(listed.unreadable[0].grant_id, unreadable_id.to_string());

        let refused = machine
            .revoke_tool_grant(&unreadable_id, ToolGrantRevocation::default())
            .await
            .expect_err("an unreadable row cannot be revoked");
        assert!(
            matches!(
                &refused,
                ToolGrantServiceError::CorruptRow { grant_id, .. }
                    if *grant_id == unreadable_id.to_string()
            ),
            "{backend:?}: {refused:?}"
        );
    }
}

/// An audit row this binary cannot read (an event kind a newer binary
/// wrote) is reported in its page as unreadable, and the rest of the page
/// is served: the log stays auditable.
#[tokio::test]
async fn an_unreadable_audit_row_is_reported_and_the_rest_of_the_page_served() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let subject = ToolGrantSubject::session(session_id.clone());
        durable
            .open()
            .append_tool_grant_audit(
                subject.key().as_str(),
                &[br#"{"at_ms":1,"event":{"kind":"activated"}}"#.to_vec()],
            )
            .await
            .unwrap();
        let grant = machine
            .grant_tool(shell_grant(&session_id, Duration::from_mins(1)), None)
            .await
            .unwrap();

        let page = machine
            .read_tool_grant_audit(&subject, None, 10)
            .await
            .unwrap();
        assert_eq!(page.len(), 2, "{backend:?}: {page:?}");
        assert_eq!(page.unreadable.len(), 1, "{backend:?}: {page:?}");
        assert_eq!(page.unreadable[0].sequence, 0, "{backend:?}");
        assert_eq!(
            page.rows.len(),
            1,
            "{backend:?}: the rest of the page is served"
        );
        assert_eq!(page.rows[0].sequence, 1, "{backend:?}");
        assert!(
            matches!(
                &page.rows[0].row.event,
                ToolGrantAuditEvent::Granted { grant_id, .. }
                    if *grant_id == grant.record.grant_id
            ),
            "{backend:?}: {page:?}"
        );
        assert_eq!(page.last_sequence(), Some(1), "{backend:?}");
    }
}

/// Settling arms the deadline driver only for a grant still live at the
/// reading: a revoked grant whose deadline lies ahead arms nothing, and a
/// live grant arms its own deadline.
#[tokio::test]
async fn settling_arms_no_deadline_for_a_revoked_grant() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let subject = ToolGrantSubject::session(session_id.clone());
        // Granted and revoked while no runtime serves the session, so nothing
        // is armed yet.
        let revoked = machine
            .grant_tool(shell_grant(&session_id, Duration::from_mins(10)), None)
            .await
            .unwrap();
        machine
            .revoke_tool_grant(&revoked.record.grant_id, ToolGrantRevocation::default())
            .await
            .unwrap();
        let _served = serve(&machine, &session_id).await;
        assert_eq!(
            machine.tool_grant_deadlines().armed_deadline(
                &session_id,
                &ToolGrantSubject::session(session_id.clone()).key()
            ),
            None,
            "{backend:?}"
        );

        let listed = machine.list_tool_grants(&subject, None).await.unwrap();
        assert_eq!(
            listed.grants[0].status,
            ToolGrantStatus::Revoked,
            "{backend:?}"
        );
        assert_eq!(
            machine.tool_grant_deadlines().armed_deadline(
                &session_id,
                &ToolGrantSubject::session(session_id.clone()).key()
            ),
            None,
            "{backend:?}: a revoked grant arms no deadline, though its deadline lies ahead"
        );

        let live = machine
            .grant_tool(shell_grant(&session_id, Duration::from_mins(5)), None)
            .await
            .unwrap();
        machine.tool_grant_deadlines().disarm(&session_id);
        machine.list_tool_grants(&subject, None).await.unwrap();
        assert_eq!(
            machine.tool_grant_deadlines().armed_deadline(
                &session_id,
                &ToolGrantSubject::session(session_id.clone()).key()
            ),
            Some(live.expires_at_ms),
            "{backend:?}: a live grant arms its own deadline"
        );
    }
}

#[tokio::test]
async fn grant_store_failure_closes_the_gate() {
    let durable = Durable::new(Backend::Memory);
    let clock = FakeClock::at(T0);
    let machine = durable.machine(&clock);
    let session_id = SessionId::new();
    let served = serve(&machine, &session_id).await;
    machine
        .grant_tool(shell_grant(&session_id, Duration::from_mins(1)), None)
        .await
        .unwrap();
    served.boundary().await.unwrap();
    assert_eq!(served.visible(), OPEN);

    durable
        .memory
        .fail_tool_grant_store_for_test(Some("grant store offline"));
    let refusal = served
        .boundary()
        .await
        .expect_err("a boundary that cannot read the grants closes the gate");
    assert!(
        matches!(refusal, ToolGrantRefusal::Unavailable { .. }),
        "{refusal:?}"
    );
    assert_eq!(served.visible(), CLOSED);
    let refusal = served
        .call("shell", "call-while-offline")
        .await
        .expect_err("a dispatch that cannot read the grants is refused");
    assert!(
        matches!(refusal, ToolGrantRefusal::Unavailable { .. }),
        "{refusal:?}"
    );

    durable.memory.fail_tool_grant_store_for_test(None);
    served.boundary().await.unwrap();
    assert_eq!(
        served.visible(),
        OPEN,
        "the next readable boundary opens it again"
    );
}

// ===========================================================================
// Run scope
// ===========================================================================

/// The shell family for `lifetime`, scoped to the run `run_id`.
fn run_scoped_shell_grant(
    session_id: &SessionId,
    run_id: &meerkat_core::RunId,
    lifetime: Duration,
) -> ToolGrantRequest {
    ToolGrantRequest {
        run_scope: Some(run_id.clone()),
        ..shell_grant(session_id, lifetime)
    }
}

/// The single tool `deploy` for `lifetime`, bounded by its window only.
fn deploy_window_grant(session_id: &SessionId, lifetime: Duration) -> ToolGrantRequest {
    ToolGrantRequest {
        target: ToolGrantTarget::Tool {
            name: ToolName::from("deploy"),
        },
        ..shell_grant(session_id, lifetime)
    }
}

impl Served {
    /// One model-request boundary of the run `run_id`.
    async fn boundary_in(
        &self,
        run_id: Option<&meerkat_core::RunId>,
    ) -> Result<(), ToolGrantRefusal> {
        match self.gate.open_names(run_id).await {
            Ok(open) => {
                self.scope
                    .apply_tool_grant_open_set(open.into_inner().into_iter().collect())
                    .expect("the open set applies");
                Ok(())
            }
            Err(refusal) => {
                self.scope
                    .apply_tool_grant_open_set(BTreeSet::new())
                    .expect("the empty set closes the gate");
                Err(refusal)
            }
        }
    }

    /// One gated call of the run `run_id`, or of no run.
    async fn call_in(
        &self,
        name: &str,
        call_id: &str,
        run_id: Option<&meerkat_core::RunId>,
    ) -> Result<(), ToolGrantRefusal> {
        self.gate
            .admit_call(ToolGrantCall {
                name,
                call_id,
                run_id,
            })
            .await
    }
}

#[tokio::test]
async fn a_run_scoped_grant_admits_only_its_run() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let served = serve(&machine, &session_id).await;
        let run_1 = meerkat_core::RunId::new();
        let run_2 = meerkat_core::RunId::new();

        let grant = machine
            .grant_tool(
                run_scoped_shell_grant(&session_id, &run_1, Duration::from_mins(10)),
                Some(&session_id),
            )
            .await
            .unwrap();
        assert_eq!(grant.status, ToolGrantStatus::Active, "{backend:?}");
        assert_eq!(grant.run_scope, Some(run_1.clone()), "{backend:?}");

        // Its own run: the boundary shows the family, and a call is handed on.
        served.boundary_in(Some(&run_1)).await.unwrap();
        assert_eq!(
            served.visible(),
            OPEN,
            "{backend:?}: its run sees the family"
        );
        served
            .call_in("shell", "call-in-its-run", Some(&run_1))
            .await
            .expect("a call of the grant's own run is admitted");

        // Another run: the call is refused; no grant covers the name there.
        let refusal = served
            .call_in("shell", "call-in-another-run", Some(&run_2))
            .await
            .expect_err("a call of another run is refused");
        assert_eq!(refusal, ToolGrantRefusal::Required, "{backend:?}");

        let listed = machine
            .list_tool_grants(
                &ToolGrantSubject::session(session_id.clone()),
                Some(&session_id),
            )
            .await
            .unwrap();
        assert_eq!(listed.grants.len(), 1, "{backend:?}");
        assert_eq!(
            listed.grants[0].run_scope,
            Some(run_1.clone()),
            "{backend:?}: the listing names the grant's run"
        );
    }
}

#[tokio::test]
async fn run_end_ends_the_grant() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let served = serve(&machine, &session_id).await;
        let run_1 = meerkat_core::RunId::new();
        let run_2 = meerkat_core::RunId::new();

        let scoped = machine
            .grant_tool(
                run_scoped_shell_grant(&session_id, &run_1, Duration::from_mins(10)),
                Some(&session_id),
            )
            .await
            .unwrap();
        let window = machine
            .grant_tool(
                deploy_window_grant(&session_id, Duration::from_mins(10)),
                Some(&session_id),
            )
            .await
            .unwrap();
        served.boundary_in(Some(&run_1)).await.unwrap();
        assert_eq!(
            served.visible(),
            ["deploy", "read_file", "shell", "shell_job_status"],
            "{backend:?}"
        );

        // The run ends: the session runtime signals it through the seam.
        clock.set(T0 + 1_000);
        served
            .gate
            .end_run(&run_1)
            .await
            .expect("the run's end is recorded");

        let store = durable.open();
        let ended = stored(&*store, &scoped).await;
        assert_eq!(ended.status, ToolGrantStatus::RunEnded, "{backend:?}");
        assert_eq!(ended.ended_at_ms, Some(T0 + 1_000), "{backend:?}");
        assert_eq!(
            stored(&*store, &window).await.status,
            ToolGrantStatus::Active,
            "{backend:?}: a window grant outlives the run"
        );
        let events = audit_events(&*store, &session_id).await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ToolGrantAuditEvent::Granted { .. },
                    ToolGrantAuditEvent::Granted { .. },
                    ToolGrantAuditEvent::RunEnded {
                        observed_at_ms,
                        driver: ToolGrantExpiryDriver::Run,
                        ..
                    },
                ] if *observed_at_ms == T0 + 1_000
            ),
            "{backend:?}: {events:?}"
        );

        // The next run's first boundary shows only what the window grant opens.
        served.boundary_in(Some(&run_2)).await.unwrap();
        assert_eq!(served.visible(), ["deploy", "read_file"], "{backend:?}");

        // Ending the run again changes nothing.
        served.gate.end_run(&run_1).await.unwrap();
        assert_eq!(
            audit_events(&*store, &session_id).await.len(),
            3,
            "{backend:?}: one end is recorded once"
        );
    }
}

#[tokio::test]
async fn a_later_run_ends_a_stale_run_scoped_grant_at_use() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let served = serve(&machine, &session_id).await;
        let run_1 = meerkat_core::RunId::new();
        let run_2 = meerkat_core::RunId::new();

        // The run's end signal is lost (a crash, a dropped run): the grant
        // still reads active.
        let grant = machine
            .grant_tool(
                run_scoped_shell_grant(&session_id, &run_1, Duration::from_mins(10)),
                Some(&session_id),
            )
            .await
            .unwrap();
        served.boundary_in(Some(&run_1)).await.unwrap();
        assert_eq!(served.visible(), OPEN, "{backend:?}");

        // The session's next run: its first boundary ends the stale grant,
        // because a session has one current run.
        clock.set(T0 + 2_000);
        served.boundary_in(Some(&run_2)).await.unwrap();
        assert_eq!(served.visible(), CLOSED, "{backend:?}");
        let store = durable.open();
        let ended = stored(&*store, &grant).await;
        assert_eq!(ended.status, ToolGrantStatus::RunEnded, "{backend:?}");
        assert_eq!(ended.ended_at_ms, Some(T0 + 2_000), "{backend:?}");

        // A dispatch of a later run ends a stale grant the same way.
        let run_3 = meerkat_core::RunId::new();
        let deploy = machine
            .grant_tool(
                ToolGrantRequest {
                    run_scope: Some(run_2.clone()),
                    ..deploy_window_grant(&session_id, Duration::from_mins(10))
                },
                Some(&session_id),
            )
            .await
            .unwrap();
        clock.set(T0 + 3_000);
        let refusal = served
            .call_in("deploy", "call-in-a-later-run", Some(&run_3))
            .await
            .expect_err("a later run's call is refused");
        assert_eq!(refusal, ToolGrantRefusal::Required, "{backend:?}");
        assert_eq!(
            stored(&*store, &deploy).await.status,
            ToolGrantStatus::RunEnded,
            "{backend:?}"
        );

        let events = audit_events(&*store, &session_id).await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ToolGrantAuditEvent::Granted { .. },
                    ToolGrantAuditEvent::RunEnded {
                        driver: ToolGrantExpiryDriver::Boundary,
                        ..
                    },
                    ToolGrantAuditEvent::Granted { .. },
                    ToolGrantAuditEvent::RunEnded {
                        driver: ToolGrantExpiryDriver::Dispatch,
                        ..
                    },
                    ToolGrantAuditEvent::Refused {
                        reason: ToolGrantRefusalKind::Required,
                        ..
                    },
                ]
            ),
            "{backend:?}: {events:?}"
        );
    }
}

#[tokio::test]
async fn a_host_call_outside_a_run_is_refused_without_ending_the_grant() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let served = serve(&machine, &session_id).await;
        let run_1 = meerkat_core::RunId::new();

        let grant = machine
            .grant_tool(
                run_scoped_shell_grant(&session_id, &run_1, Duration::from_mins(10)),
                Some(&session_id),
            )
            .await
            .unwrap();
        let refusal = served
            .call_in("shell", "host-call", None)
            .await
            .expect_err("a call outside any run is refused");
        assert_eq!(refusal, ToolGrantRefusal::Required, "{backend:?}");
        let store = durable.open();
        assert_eq!(
            stored(&*store, &grant).await.status,
            ToolGrantStatus::Active,
            "{backend:?}: the refusal ends nothing"
        );
        // A boundary outside a run opens nothing either, and ends nothing.
        served.boundary_in(None).await.unwrap();
        assert_eq!(served.visible(), CLOSED, "{backend:?}");
        assert_eq!(
            stored(&*store, &grant).await.status,
            ToolGrantStatus::Active,
            "{backend:?}"
        );

        // The grant's own run still uses it.
        served.boundary_in(Some(&run_1)).await.unwrap();
        assert_eq!(served.visible(), OPEN, "{backend:?}");
        served
            .call_in("shell", "call-in-its-run", Some(&run_1))
            .await
            .unwrap();
    }
}

/// A process dies while a run that holds a run-scoped grant is open. At the
/// next start, durable-tail recovery commits the run's tail as interrupted:
/// the run is over, so its run-scoped grants end with it. A window grant and
/// a grant of another run are left as they are.
#[tokio::test]
async fn durable_tail_recovery_of_an_interrupted_run_ends_its_run_scoped_grants() {
    use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, StopReason, ToolResult,
        TranscriptMessageIdentity, UserMessage,
    };
    use meerkat_runtime::identifiers::LogicalRuntimeId;
    use meerkat_runtime::recovery::{DurableTailRecoveryOutcome, recover_durable_tail};
    use meerkat_runtime::store::{PreparedRuntimeSessionCommit, PreparedWholeBlobProvisionalTail};

    let store = InMemoryRuntimeStore::new();
    let grants = ToolGrantService::new(Arc::new(store.clone()));
    let run_id = meerkat_core::RunId::new();
    let other_run = meerkat_core::RunId::new();
    let mut committed = meerkat_core::Session::new();
    committed.push(Message::User(UserMessage::text("run the shell")));
    let session_id = committed.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);

    let scoped = grants
        .grant(run_scoped_shell_grant(
            &session_id,
            &run_id,
            Duration::from_hours(1),
        ))
        .await
        .unwrap();
    let other = grants
        .grant(ToolGrantRequest {
            run_scope: Some(other_run.clone()),
            ..deploy_window_grant(&session_id, Duration::from_hours(1))
        })
        .await
        .unwrap();
    let window = grants
        .grant(deploy_window_grant(&session_id, Duration::from_hours(1)))
        .await
        .unwrap();

    let committed_authority = store
        .commit_prepared_session_boundary(
            &runtime_id,
            PreparedRuntimeSessionCommit::snapshot_only(
                BoundSessionCommit::sealed(Arc::new(committed.clone())).unwrap(),
            ),
        )
        .await
        .unwrap()
        .authority()
        .and_then(|authority| authority.whole_blob())
        .cloned()
        .expect("the first boundary returns WholeBlob store authority");
    let mut interrupted = committed;
    interrupted.push(Message::BlockAssistant(BlockAssistantMessage {
        blocks: vec![AssistantBlock::ToolUse {
            id: "shell-call".to_string(),
            name: "shell".to_string(),
            args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        stop_reason: Some(StopReason::ToolUse),
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
        assistant_message_id: None,
    }));
    interrupted.push(Message::tool_results(vec![ToolResult::new(
        "shell-call".to_string(),
        "partial output".to_string(),
        false,
    )]));
    let candidate = BoundSessionCommit::sealed(Arc::new(interrupted)).unwrap();
    store
        .write_prepared_whole_blob_provisional_tail(
            &runtime_id,
            PreparedWholeBlobProvisionalTail::prepare(
                committed_authority,
                run_id.clone(),
                1,
                &candidate,
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let outcome = recover_durable_tail(&store, &session_id)
        .await
        .expect("the interrupted run's tail recovers");
    assert!(
        matches!(outcome, DurableTailRecoveryOutcome::Committed { .. }),
        "{outcome:?}"
    );

    assert_eq!(
        stored(&store, &scoped).await.status,
        ToolGrantStatus::RunEnded,
        "the recovered run's grant ended with it"
    );
    assert_eq!(stored(&store, &other).await.status, ToolGrantStatus::Active);
    assert_eq!(
        stored(&store, &window).await.status,
        ToolGrantStatus::Active
    );
    let events = audit_events(&store, &session_id).await;
    assert!(
        events.iter().any(|event| matches!(
            event,
            ToolGrantAuditEvent::RunEnded {
                grant_id,
                driver: ToolGrantExpiryDriver::Recovery,
                ..
            } if *grant_id == scoped.record.grant_id
        )),
        "{events:?}"
    );
}

// ---------------------------------------------------------------------------
// Approval-carried grants: `tool_grant.issue` decisions
// ---------------------------------------------------------------------------

/// The decision owner reads the same clock as the grant service.
impl DecisionClock for FakeClock {
    fn now_ms(&self) -> Result<u64, DecisionClockError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

fn principal_of(kind: PrincipalKind, id: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        kind,
        id,
        TrustDomainId::new("example.org").expect("trust domain"),
    )
    .expect("qualified principal")
}

/// The principal a host's request names as its requester.
fn host_requester() -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(&principal_of(PrincipalKind::RuntimeHost, "host"))
        .expect("principal key")
}

/// The decided party answers as current.
struct CurrentApprover;

impl ConsentEligibility for CurrentApprover {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Ok(())
    }
}

impl DecisionApproverCurrentness for CurrentApprover {
    fn approver_answer(&self, _decision: &StoredDecision) -> ConsentEligibilityAnswer {
        ConsentEligibilityAnswer::Eligible(Arc::new(CurrentApprover))
    }
}

/// The notices a host was handed, each admitted to its session.
#[derive(Default)]
struct RecordedNotices(std::sync::Mutex<Vec<ToolGrantRequestNotice>>);

impl RecordedNotices {
    fn outcomes(&self) -> Vec<(DecisionId, ToolGrantRequestOutcome)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|notice| (notice.decision_id.clone(), notice.outcome.clone()))
            .collect()
    }
}

#[async_trait::async_trait]
impl ToolGrantRequestNotices for RecordedNotices {
    async fn deliver(
        &self,
        notice: &ToolGrantRequestNotice,
    ) -> Option<ApprovalConsentDeliveryState> {
        self.0.lock().unwrap().push(notice.clone());
        let ToolGrantSubject::Session { session_id } = &notice.subject else {
            return None;
        };
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session_id.clone(),
            input_id: format!("notice:{}", notice.decision_id),
        })
    }
}

/// The run each subject runs now, as the host sees it: a session's own, or
/// the run of the session a member identity's member holds; and the member
/// identities that ended.
#[derive(Default)]
struct HostRuns {
    runs: std::sync::Mutex<BTreeMap<String, RunId>>,
    ended: std::sync::Mutex<BTreeSet<String>>,
    unknown: std::sync::Mutex<BTreeSet<String>>,
}

impl HostRuns {
    fn set(&self, session_id: &SessionId, run_id: Option<RunId>) {
        self.set_subject(&ToolGrantSubject::session(session_id.clone()), run_id);
    }

    fn set_subject(&self, subject: &ToolGrantSubject, run_id: Option<RunId>) {
        let key = subject.key().as_str().to_owned();
        let mut runs = self.runs.lock().unwrap();
        match run_id {
            Some(run_id) => runs.insert(key, run_id),
            None => runs.remove(&key),
        };
    }

    /// Whether the host cannot tell what `subject` is now.
    fn set_unknown(&self, subject: &ToolGrantSubject, unknown: bool) {
        let key = subject.key().as_str().to_owned();
        let mut set = self.unknown.lock().unwrap();
        if unknown {
            set.insert(key);
        } else {
            set.remove(&key);
        }
    }

    /// `subject`'s identity ended: its member retired, its entry ended, or
    /// its mob was reset or replaced.
    fn end_subject(&self, subject: &ToolGrantSubject) {
        self.ended
            .lock()
            .unwrap()
            .insert(subject.key().as_str().to_owned());
    }
}

#[async_trait::async_trait]
impl ToolGrantSubjectObserver for HostRuns {
    async fn presence(&self, subject: &ToolGrantSubject) -> ToolGrantSubjectPresence {
        let key = subject.key().as_str().to_owned();
        if self.unknown.lock().unwrap().contains(&key) {
            return ToolGrantSubjectPresence::Unknown;
        }
        if self.ended.lock().unwrap().contains(&key) {
            return ToolGrantSubjectPresence::Ended;
        }
        ToolGrantSubjectPresence::Held {
            current_run: self.runs.lock().unwrap().get(&key).cloned(),
        }
    }
}

/// A grant service and a decision owner over one clock, with the
/// `tool_grant.issue` executor a host registers.
struct RequestRig {
    clock: Arc<FakeClock>,
    grants: Arc<ToolGrantService>,
    decisions: DurableDecisionService,
    notices: Arc<RecordedNotices>,
    runs: Arc<HostRuns>,
    executor: Arc<ToolGrantIssueExecutor>,
}

/// A request lasts ten minutes from its issuance.
const REQUESTED_FOR: Duration = Duration::from_mins(10);

impl RequestRig {
    fn new(durable: &Durable) -> Self {
        let clock = FakeClock::at(T0);
        let grants = Arc::new(ToolGrantService::new(durable.open()));
        grants.set_clock(Arc::clone(&clock) as Arc<dyn ToolGrantClock>);
        let decisions = DurableDecisionService::with_clock(
            Arc::new(InMemoryDecisionStore::new()),
            Arc::clone(&clock) as Arc<dyn DecisionClock>,
        );
        let notices = Arc::new(RecordedNotices::default());
        let runs = Arc::new(HostRuns::default());
        let executor = Arc::new(
            ToolGrantIssueExecutor::new(
                Arc::clone(&grants),
                Arc::clone(&runs) as Arc<dyn ToolGrantSubjectObserver>,
                Arc::clone(&notices) as Arc<dyn ToolGrantRequestNotices>,
            )
            .expect("the executor"),
        );
        Self {
            clock,
            grants,
            decisions,
            notices,
            runs,
            executor,
        }
    }

    /// A host's request for `session`'s `deploy`, for ten minutes.
    fn request(
        &self,
        session_id: &SessionId,
        request_id: &str,
        run_scope: Option<RunId>,
    ) -> StoredDecision {
        self.request_for(
            ToolGrantSubject::session(session_id.clone()),
            request_id,
            run_scope,
        )
    }

    /// A host's request for `subject`'s `deploy`, for ten minutes.
    fn request_for(
        &self,
        subject: ToolGrantSubject,
        request_id: &str,
        run_scope: Option<RunId>,
    ) -> StoredDecision {
        hold_tool_grant_request(
            &self.decisions,
            &ToolGrantRequestPolicy::default(),
            self.grants.policy().max_lifetime_ms(),
            self.clock.0.load(Ordering::SeqCst),
            ToolGrantIssueRequest {
                subject,
                target: ToolGrantTarget::Tool {
                    name: ToolName::from("deploy"),
                },
                duration: REQUESTED_FOR,
                run_scope,
                reason: Some("ship the fix".to_owned()),
                origin: ToolGrantRequestOrigin::Host {
                    request_id: request_id.to_owned(),
                },
                requester: host_requester(),
            },
        )
        .expect("hold the request")
    }

    /// Present `decision` to its approver and decide it as a person.
    fn decide(&self, decision: &StoredDecision, approve: bool) -> StoredDecision {
        let now_ms = self.clock.0.load(Ordering::SeqCst);
        let rendering = &decision.descriptor.rendering;
        self.decisions
            .present(
                &decision.decision_id,
                &PresentationRecord::complete(
                    PresentationAudience::Approver {
                        approver_id: decision.descriptor.approver_id.clone(),
                    },
                    rendering,
                    None,
                    DecisionReference::new("console").expect("reference"),
                    DecisionReference::new("message-1").expect("reference"),
                    now_ms,
                ),
            )
            .expect("present");
        let decided = AttestedDecision::new(
            AuthenticatedApprover::new(
                principal_of(PrincipalKind::Human, "alice"),
                ApprovalAssurance::PersonAuthenticated,
            )
            .expect("authenticated human"),
            DecisionIngress::new("console").expect("ingress"),
            DecisionReference::new("event-1").expect("reference"),
            now_ms,
            decision.descriptor.approver_id.clone(),
            rendering.subject_digest(),
            approve,
        )
        .expect("attested decision");
        self.decisions
            .decide(&decision.decision_id, &decided, &CurrentApprover)
            .expect("decide")
    }

    /// One pass of an executor loop that registers the request executor.
    async fn pass(&self) -> DecisionExecutorPass {
        let executor: Arc<dyn DecisionExecutor> = self.executor.clone();
        let mut executors = DecisionExecutorLoop::start(
            self.decisions.clone(),
            [executor],
            Arc::new(CurrentApprover),
        )
        .expect("start the loop");
        executors.pass().await
    }
}

fn outcome_of(pass: &DecisionExecutorPass, decision_id: &DecisionId) -> Vec<DecisionPassOutcome> {
    pass.outcomes
        .iter()
        .filter(|(id, _)| id == decision_id)
        .map(|(_, outcome)| outcome.clone())
        .collect()
}

#[tokio::test]
async fn a_request_holds_a_decision_and_issues_nothing_until_approved() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let rig = RequestRig::new(&durable);
        let session_id = SessionId::new();
        let subject = ToolGrantSubject::session(session_id.clone());

        let held = rig.request(&session_id, "ask-1", None);
        assert_eq!(
            held.descriptor.kind.as_str(),
            TOOL_GRANT_ISSUE_KIND,
            "{backend:?}"
        );
        assert_eq!(held.state.phase, DecisionPhase::Open, "{backend:?}");
        assert_eq!(
            held.descriptor.approver_id.as_str(),
            "operator",
            "{backend:?}"
        );
        assert_eq!(held.descriptor.requester, host_requester(), "{backend:?}");
        assert_eq!(held.descriptor.reviewer, None, "{backend:?}");
        assert_eq!(
            held.descriptor.deadline_at_ms,
            T0 + 3_600_000,
            "{backend:?}"
        );
        let body = ToolGrantIssueBody::of(&held).expect("the held body");
        assert_eq!(body.subject, subject, "{backend:?}");
        assert_eq!(body.duration_ms, 600_000, "{backend:?}");
        assert_eq!(body.reason.as_deref(), Some("ship the fix"), "{backend:?}");

        // The same host ask again is another request; the same one answers
        // the decision it held.
        let replayed = rig.request(&session_id, "ask-1", None);
        assert_eq!(replayed.decision_id, held.decision_id, "{backend:?}");

        let pass = rig.pass().await;
        assert!(outcome_of(&pass, &held.decision_id).is_empty(), "{pass:?}");
        assert!(
            rig.grants.list(&subject).await.unwrap().grants.is_empty(),
            "{backend:?}: nothing is issued before a person approves"
        );
        assert!(
            rig.grants
                .issued_for(&held.decision_id)
                .await
                .unwrap()
                .is_none(),
            "{backend:?}"
        );
        let requests = tool_grant_requests_of(&rig.decisions, &subject, None).unwrap();
        assert_eq!(
            requests
                .iter()
                .map(|(decision, _)| decision.decision_id.clone())
                .collect::<Vec<_>>(),
            vec![held.decision_id.clone()],
            "{backend:?}: the list projects the open request"
        );
        assert!(rig.notices.outcomes().is_empty(), "{backend:?}");
    }
}

#[tokio::test]
async fn an_approved_request_issues_one_grant_with_the_decision_as_issuer() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let rig = RequestRig::new(&durable);
        let session_id = SessionId::new();
        let subject = ToolGrantSubject::session(session_id.clone());
        let held = rig.request(&session_id, "ask-1", None);
        rig.clock.set(T0 + 5_000);
        assert_eq!(rig.decide(&held, true).state.phase, DecisionPhase::Approved);

        rig.clock.set(T0 + 10_000);
        let pass = rig.pass().await;
        assert_eq!(
            outcome_of(&pass, &held.decision_id),
            vec![DecisionPassOutcome::Executed],
            "{backend:?}: {pass:?}"
        );
        let grant = rig
            .grants
            .issued_for(&held.decision_id)
            .await
            .unwrap()
            .expect("the approved grant");
        assert_eq!(
            grant.record.grant_id,
            ToolGrantId::approved(&held.decision_id)
        );
        assert_eq!(
            grant.record.issuer,
            ToolGrantIssuer::Approval {
                approval_id: held.decision_id.to_string()
            }
        );
        assert_eq!(grant.record.subject, subject);
        assert_eq!(grant.record.reason.as_deref(), Some("ship the fix"));
        assert_eq!(grant.status, ToolGrantStatus::Active);
        assert_eq!(
            grant.issued_at_ms,
            T0 + 10_000,
            "the window starts at execution"
        );
        assert_eq!(grant.expires_at_ms, T0 + 10_000 + 600_000);
        assert_eq!(grant.run_scope, None);
        assert_eq!(rig.grants.list(&subject).await.unwrap().grants.len(), 1);
        assert_eq!(
            rig.decisions.get(&held.decision_id).unwrap().state.phase,
            DecisionPhase::Executed
        );
        // The approved grant's id parses back.
        assert_eq!(
            ToolGrantId::parse(&grant.record.grant_id.to_string()).unwrap(),
            grant.record.grant_id
        );

        // The next pass owes the request's notice: granted.
        rig.pass().await;
        assert_eq!(
            rig.notices.outcomes(),
            vec![(
                held.decision_id.clone(),
                ToolGrantRequestOutcome::Granted {
                    grant_id: grant.record.grant_id.clone()
                }
            )],
            "{backend:?}"
        );
    }
}

#[tokio::test]
async fn a_replayed_execution_meets_the_derived_id_and_settles_executed() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let rig = RequestRig::new(&durable);
        let session_id = SessionId::new();
        let subject = ToolGrantSubject::session(session_id.clone());
        let held = rig.request(&session_id, "ask-1", None);
        rig.decide(&held, true);

        // An earlier executor committed the grant and stopped before its
        // execution was recorded: the permit stays outstanding.
        let approved = rig.decisions.get(&held.decision_id).unwrap();
        let permit = rig
            .decisions
            .authorize_execution(
                &held.decision_id,
                &approved.descriptor.operation_id,
                &approved.descriptor.request_digest,
                &ConsentEligibilityAnswer::Eligible(Arc::new(CurrentApprover)),
            )
            .expect("permit");
        assert_eq!(
            rig.executor.execute(&approved, &permit).await,
            DecisionExecution::Committed
        );
        // A replay under the same permit meets the issued grant.
        assert_eq!(
            rig.executor.execute(&approved, &permit).await,
            DecisionExecution::Committed
        );
        assert_eq!(rig.grants.list(&subject).await.unwrap().grants.len(), 1);
        // The executor stops here: the permit stays outstanding in the store.
        drop(permit);

        // The next pass settles the outstanding permit from the grant it
        // finds and issues nothing more.
        let pass = rig.pass().await;
        assert_eq!(
            outcome_of(&pass, &held.decision_id),
            vec![DecisionPassOutcome::SettledExecuted],
            "{backend:?}: {pass:?}"
        );
        assert_eq!(rig.grants.list(&subject).await.unwrap().grants.len(), 1);
        assert_eq!(
            rig.decisions.get(&held.decision_id).unwrap().state.phase,
            DecisionPhase::Executed
        );
    }
}

#[tokio::test]
async fn a_rejected_or_expired_request_issues_nothing_and_lists_declined() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let rig = RequestRig::new(&durable);
        let session_id = SessionId::new();
        let subject = ToolGrantSubject::session(session_id.clone());
        let rejected = rig.request(&session_id, "ask-1", None);
        let expiring = rig.request(&session_id, "ask-2", None);
        assert_ne!(rejected.decision_id, expiring.decision_id);
        assert_eq!(
            rig.decide(&rejected, false).state.phase,
            DecisionPhase::Rejected
        );

        // The request's deadline passes undecided.
        rig.clock.set(T0 + 3_600_000);
        let pass = rig.pass().await;
        assert!(
            outcome_of(&pass, &expiring.decision_id).contains(&DecisionPassOutcome::Expired),
            "{backend:?}: {pass:?}"
        );
        rig.pass().await;

        assert!(rig.grants.list(&subject).await.unwrap().grants.is_empty());
        assert!(
            tool_grant_requests_of(&rig.decisions, &subject, None)
                .unwrap()
                .is_empty(),
            "{backend:?}: no request is open"
        );
        let mut declined = tool_grant_requests_of(&rig.decisions, &subject, Some(T0))
            .unwrap()
            .into_iter()
            .map(|(decision, _)| (decision.decision_id, decision.state.phase))
            .collect::<Vec<_>>();
        declined.sort_by(|left, right| left.0.cmp(&right.0));
        let mut expected = vec![
            (rejected.decision_id.clone(), DecisionPhase::Rejected),
            (expiring.decision_id.clone(), DecisionPhase::Expired),
        ];
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(declined, expected, "{backend:?}");
        let mut notices = rig.notices.outcomes();
        notices.sort_by(|left, right| left.0.cmp(&right.0));
        let mut expected = vec![
            (
                rejected.decision_id.clone(),
                ToolGrantRequestOutcome::Rejected,
            ),
            (
                expiring.decision_id.clone(),
                ToolGrantRequestOutcome::Expired,
            ),
        ];
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(notices, expected, "{backend:?}");
    }
}

#[tokio::test]
async fn a_tool_grant_request_with_a_review_stage_is_refused() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let rig = RequestRig::new(&durable);
        let session_id = SessionId::new();
        let subject = ToolGrantSubject::session(session_id.clone());
        // A host holds a tool grant request with a review stage, which a
        // reviewer's allow approves.
        let body = ToolGrantIssueBody {
            body: TOOL_GRANT_ISSUE_BODY.to_owned(),
            subject: subject.clone(),
            target: ToolGrantTarget::Tool {
                name: ToolName::from("deploy"),
            },
            duration_ms: 600_000,
            run_scope: None,
            reason: None,
            origin: ToolGrantRequestOrigin::Host {
                request_id: "ask-1".to_owned(),
            },
        };
        let text = serde_json::to_string(&body).unwrap();
        let rendering = DecisionRendering::new("Grant deploy for 10 min.").unwrap();
        let kind = DecisionKind::new(TOOL_GRANT_ISSUE_KIND).unwrap();
        let subject_key =
            DecisionSubjectKey::new(format!("{}/tool:deploy/host:ask-1", subject.key())).unwrap();
        let decision_id = DecisionId::derive(&kind, &subject_key);
        let reviewer = principal_of(PrincipalKind::SharedAgent, "reviewer");
        let held = rig
            .decisions
            .hold(
                DecisionHoldRequest::new(
                    kind,
                    subject_key,
                    DecisionOperationId::new(format!(
                        "tool_grant:{}",
                        ToolGrantId::approved(&decision_id)
                    ))
                    .unwrap(),
                    Sha256Digest::of(text.as_bytes()),
                    DecisionRequestBody::new(
                        serde_json::value::RawValue::from_string(text.clone()).unwrap(),
                    )
                    .unwrap(),
                    host_requester(),
                    ApproverId::new("operator").unwrap(),
                    T0 + 3_600_000,
                    rendering.clone(),
                    DecisionRenderer::new(TOOL_GRANT_ISSUE_RENDERER).unwrap(),
                )
                .with_reviewer(DecisionPrincipalKey::of(&reviewer).unwrap()),
                true,
            )
            .expect("hold");
        rig.decisions
            .present(
                &held.decision_id,
                &PresentationRecord::complete(
                    PresentationAudience::Reviewer {
                        reviewer: DecisionPrincipalKey::of(&reviewer).unwrap(),
                    },
                    &rendering,
                    None,
                    DecisionReference::new("review").unwrap(),
                    DecisionReference::new("message-1").unwrap(),
                    T0,
                ),
            )
            .expect("present to the reviewer");
        let allowed = rig
            .decisions
            .review(
                &held.decision_id,
                &AttributedReview::new(
                    reviewer,
                    SessionId::new(),
                    DecisionReference::new("call-1").unwrap(),
                    DecisionReviewVerdict::Allow,
                    rendering.subject_digest(),
                )
                .unwrap(),
            )
            .expect("review");
        assert_eq!(allowed.state.phase, DecisionPhase::Approved);

        let pass = rig.pass().await;
        assert_eq!(
            outcome_of(&pass, &held.decision_id),
            vec![DecisionPassOutcome::ExecutionRefused],
            "{backend:?}: a reviewer's allow never issues a grant: {pass:?}"
        );
        let refused = rig.decisions.get(&held.decision_id).unwrap();
        assert_eq!(refused.state.phase, DecisionPhase::ExecutionRefused);
        assert_eq!(
            refused
                .execution_refusal
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("not_a_human_decision")
        );
        assert!(rig.grants.list(&subject).await.unwrap().grants.is_empty());
    }
}

#[tokio::test]
async fn a_current_run_request_approved_after_its_run_ended_is_closed_run_ended() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let rig = RequestRig::new(&durable);
        let session_id = SessionId::new();
        let subject = ToolGrantSubject::session(session_id.clone());
        let first_run = RunId::new();
        let second_run = RunId::new();
        rig.runs.set(&session_id, Some(first_run.clone()));
        let late = rig.request(&session_id, "ask-1", Some(first_run.clone()));
        rig.decide(&late, true);

        // The run ends and the session runs another before the execution.
        rig.runs.set(&session_id, Some(second_run.clone()));
        let pass = rig.pass().await;
        assert_eq!(
            outcome_of(&pass, &late.decision_id),
            vec![DecisionPassOutcome::ExecutionRefused],
            "{backend:?}: {pass:?}"
        );
        assert_eq!(
            rig.decisions
                .get(&late.decision_id)
                .unwrap()
                .execution_refusal
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("run_ended")
        );
        assert!(rig.grants.list(&subject).await.unwrap().grants.is_empty());
        rig.pass().await;
        assert_eq!(
            rig.notices.outcomes(),
            vec![(
                late.decision_id.clone(),
                ToolGrantRequestOutcome::Refused {
                    reason: "run_ended".to_owned()
                }
            )],
            "{backend:?}"
        );

        // A request approved while its run still runs issues the grant,
        // scoped to that run.
        let timely = rig.request(&session_id, "ask-2", Some(second_run.clone()));
        rig.decide(&timely, true);
        let pass = rig.pass().await;
        assert_eq!(
            outcome_of(&pass, &timely.decision_id),
            vec![DecisionPassOutcome::Executed],
            "{backend:?}: {pass:?}"
        );
        let grant = rig
            .grants
            .issued_for(&timely.decision_id)
            .await
            .unwrap()
            .expect("the run-scoped grant");
        assert_eq!(grant.run_scope, Some(second_run));
    }
}

/// A `current_run` request to a member identity names the run of the
/// session its member holds. A person's approval issues it while the member
/// still runs that run, scoped to it, and the member's end of the run ends
/// it; a request approved after its member's run ended is closed
/// `run_ended`.
#[tokio::test]
async fn an_identity_current_run_request_issues_for_its_members_run() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let rig = RequestRig::new(&durable);
        let member = member_subject();
        let member_session = SessionId::new();
        let first_run = RunId::new();
        let second_run = RunId::new();
        rig.runs.set_subject(&member, Some(first_run.clone()));
        let timely = rig.request_for(member.clone(), "ask-1", Some(first_run.clone()));
        rig.decide(&timely, true);
        let pass = rig.pass().await;
        assert_eq!(
            outcome_of(&pass, &timely.decision_id),
            vec![DecisionPassOutcome::Executed],
            "{backend:?}: {pass:?}"
        );
        let grant = rig
            .grants
            .issued_for(&timely.decision_id)
            .await
            .unwrap()
            .expect("the identity's run-scoped grant");
        assert_eq!(grant.record.subject, member, "{backend:?}");
        assert_eq!(grant.run_scope, Some(first_run.clone()), "{backend:?}");

        // The member's build ends the run for both its subjects.
        rig.grants
            .end_run(
                &[
                    member.key(),
                    ToolGrantSubject::session(member_session).key(),
                ],
                &first_run,
                ToolGrantExpiryDriver::Run,
            )
            .await
            .expect("end the member's run");
        let listed = rig.grants.list(&member).await.unwrap().grants;
        assert_eq!(listed.len(), 1, "{backend:?}: {listed:?}");
        assert_eq!(listed[0].status, ToolGrantStatus::RunEnded, "{backend:?}");

        // The member runs another run before a later approval executes.
        rig.runs.set_subject(&member, Some(second_run));
        let late = rig.request_for(member.clone(), "ask-2", Some(first_run));
        rig.decide(&late, true);
        let pass = rig.pass().await;
        assert_eq!(
            outcome_of(&pass, &late.decision_id),
            vec![DecisionPassOutcome::ExecutionRefused],
            "{backend:?}: {pass:?}"
        );
        assert_eq!(
            rig.decisions
                .get(&late.decision_id)
                .unwrap()
                .execution_refusal
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("run_ended"),
            "{backend:?}"
        );
    }
}

/// An approved request to a member identity that ended before its approval
/// executed (its member retired, its entry ended, or its mob was reset or
/// replaced) issues nothing: the execution is refused `subject_ended`, so a
/// later member of the name never holds a grant asked for an earlier one.
#[tokio::test]
async fn an_approved_request_to_an_ended_identity_issues_nothing() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let rig = RequestRig::new(&durable);
        let member = member_subject();
        let held = rig.request_for(member.clone(), "ask-1", None);
        rig.runs.end_subject(&member);
        rig.decide(&held, true);
        let pass = rig.pass().await;
        assert_eq!(
            outcome_of(&pass, &held.decision_id),
            vec![DecisionPassOutcome::ExecutionRefused],
            "{backend:?}: {pass:?}"
        );
        assert_eq!(
            rig.decisions
                .get(&held.decision_id)
                .unwrap()
                .execution_refusal
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("subject_ended"),
            "{backend:?}"
        );
        assert!(
            rig.grants
                .issued_for(&held.decision_id)
                .await
                .unwrap()
                .is_none(),
            "{backend:?}: nothing is issued to an ended identity"
        );
    }
}

/// An approved request whose subject the host cannot tell now (it cannot
/// read its record of the mob or the roster) issues nothing: the execution
/// is refused `subject_unknown`, fail closed, and a later pass, once the
/// host can tell again, issues nothing either.
#[tokio::test]
async fn an_approved_request_whose_subject_the_host_cannot_tell_issues_nothing() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let rig = RequestRig::new(&durable);
        let member = member_subject();
        let held = rig.request_for(member.clone(), "ask-1", None);
        rig.runs.set_unknown(&member, true);
        rig.decide(&held, true);
        let pass = rig.pass().await;
        assert_eq!(
            outcome_of(&pass, &held.decision_id),
            vec![DecisionPassOutcome::ExecutionRefused],
            "{backend:?}: {pass:?}"
        );
        assert_eq!(
            rig.decisions
                .get(&held.decision_id)
                .unwrap()
                .execution_refusal
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("subject_unknown"),
            "{backend:?}"
        );
        rig.runs.set_unknown(&member, false);
        rig.pass().await;
        assert!(
            rig.grants
                .issued_for(&held.decision_id)
                .await
                .unwrap()
                .is_none(),
            "{backend:?}: never issued, even once the host can tell again"
        );
    }
}

/// The principal a host names an agent's requests by.
fn agent_requester() -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(&principal_of(PrincipalKind::PersonalAgent, "agent"))
        .expect("principal key")
}

/// An agent's request through its session's grant seam holds a
/// `tool_grant.issue` decision whose requester is the principal the host
/// names its agents by, for the host's approver, about the agent's own
/// session: the agent names neither. Its run and tool call are the request's
/// key, so a replayed call answers the decision it held. A runtime whose host
/// configured no requests takes none.
#[tokio::test]
async fn an_agent_held_request_names_the_agent_as_requester() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let bindings = machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("bindings prepare");
        let grants = Arc::clone(
            bindings
                .tool_grants()
                .expect("the runtime carries the session's grants"),
        );
        let target = ToolGrantTarget::Family {
            family: "shell".to_string(),
            names: shell_family(),
        };
        let run_id = RunId::new();
        let ask = |call_id: &'static str| ToolGrantAgentRequest {
            target: &target,
            duration: REQUESTED_FOR,
            reason: Some("rotate the logs"),
            run_id: &run_id,
            call_id,
        };

        assert_eq!(
            grants.request_grant(ask("call-1")).await,
            Err(ToolGrantAgentRequestRefusal::NotTaken),
            "{backend:?}: no host configured"
        );

        let decisions = DurableDecisionService::with_clock(
            Arc::new(InMemoryDecisionStore::new()),
            Arc::clone(&clock) as Arc<dyn DecisionClock>,
        );
        machine.set_tool_grant_request_host(Some(ToolGrantRequestHost {
            decisions: decisions.clone(),
            policy: ToolGrantRequestPolicy::default(),
            requester: agent_requester(),
        }));

        let receipt = grants
            .request_grant(ask("call-1"))
            .await
            .expect("the request is held");
        let held = decisions.get(&receipt.decision_id).expect("the decision");
        assert_eq!(held.descriptor.kind.as_str(), TOOL_GRANT_ISSUE_KIND);
        assert_eq!(held.descriptor.requester, agent_requester(), "{backend:?}");
        assert_eq!(held.descriptor.approver_id.as_str(), "operator");
        assert_eq!(held.state.phase, DecisionPhase::Open);
        assert_eq!(receipt.phase, DecisionPhase::Open);
        assert_eq!(receipt.deadline_at_ms, held.descriptor.deadline_at_ms);
        assert_eq!(
            receipt.grant_id,
            ToolGrantId::approved(&receipt.decision_id),
            "{backend:?}"
        );
        let body = ToolGrantIssueBody::of(&held).expect("the held body");
        assert_eq!(body.subject, ToolGrantSubject::session(session_id.clone()));
        assert_eq!(body.target, target);
        assert_eq!(body.duration_ms, 600_000);
        assert_eq!(body.run_scope, None);
        assert_eq!(body.reason.as_deref(), Some("rotate the logs"));
        assert_eq!(
            body.origin,
            ToolGrantRequestOrigin::Agent {
                run_id: run_id.clone(),
                call_id: "call-1".to_owned(),
            }
        );

        // A replayed call answers the decision it held.
        let replayed = grants
            .request_grant(ask("call-1"))
            .await
            .expect("the replay answers");
        assert_eq!(replayed.decision_id, receipt.decision_id, "{backend:?}");
        assert!(
            machine
                .list_tool_grants(&ToolGrantSubject::session(session_id.clone()), None)
                .await
                .unwrap()
                .grants
                .is_empty(),
            "{backend:?}: nothing is issued"
        );
    }
}

/// An agent that asks again for a target its subject already has an open
/// or approved window request for is answered with that request, so the
/// approver's queue holds one request per target. Another target is another
/// request, and once the request ended a new call holds a new one.
#[tokio::test]
async fn an_agents_repeated_request_for_a_target_answers_its_open_request() {
    fn ask<'a>(
        target: &'a ToolGrantTarget,
        run_id: &'a RunId,
        call_id: &'a str,
    ) -> ToolGrantAgentRequest<'a> {
        ToolGrantAgentRequest {
            target,
            duration: REQUESTED_FOR,
            reason: Some("rotate the logs"),
            run_id,
            call_id,
        }
    }

    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let session_id = SessionId::new();
        let bindings = machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("bindings prepare");
        let grants = Arc::clone(
            bindings
                .tool_grants()
                .expect("the runtime carries the session's grants"),
        );
        let decisions = DurableDecisionService::with_clock(
            Arc::new(InMemoryDecisionStore::new()),
            Arc::clone(&clock) as Arc<dyn DecisionClock>,
        );
        machine.set_tool_grant_request_host(Some(ToolGrantRequestHost {
            decisions: decisions.clone(),
            policy: ToolGrantRequestPolicy::default(),
            requester: agent_requester(),
        }));
        let shell = ToolGrantTarget::Family {
            family: "shell".to_string(),
            names: shell_family(),
        };
        let deploy = ToolGrantTarget::Tool {
            name: ToolName::from("deploy"),
        };
        let run_id = RunId::new();

        let first = grants
            .request_grant(ask(&shell, &run_id, "call-1"))
            .await
            .expect("the request is held");
        let again = grants
            .request_grant(ask(&shell, &run_id, "call-2"))
            .await
            .expect("the second ask answers");
        assert_eq!(
            again.decision_id, first.decision_id,
            "{backend:?}: one open request per target"
        );
        let other = grants
            .request_grant(ask(&deploy, &run_id, "call-3"))
            .await
            .expect("another target");
        assert_ne!(
            other.decision_id, first.decision_id,
            "{backend:?}: another target is another request"
        );

        decisions
            .withdraw(
                &first.decision_id,
                &meerkat_runtime::decision::DecisionWithdrawer::Host,
            )
            .expect("the host withdraws the first request");
        let after = grants
            .request_grant(ask(&shell, &run_id, "call-4"))
            .await
            .expect("a new request");
        assert_ne!(
            after.decision_id, first.decision_id,
            "{backend:?}: an ended request is asked anew"
        );
        assert_eq!(after.phase, DecisionPhase::Open, "{backend:?}");
        let open = machine
            .list_tool_grant_requests(
                &decisions,
                &ToolGrantSubject::session(session_id.clone()),
                false,
            )
            .expect("the requests")
            .into_iter()
            .map(|(decision, _)| decision.decision_id)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            open,
            BTreeSet::from([other.decision_id.clone(), after.decision_id.clone()]),
            "{backend:?}: one open request for each target"
        );
    }
}

/// An agent's ask of `target` for ten minutes, from the call `call_id` of
/// the run `run_id`.
fn agent_ask<'a>(
    target: &'a ToolGrantTarget,
    run_id: &'a RunId,
    call_id: &'a str,
) -> ToolGrantAgentRequest<'a> {
    ToolGrantAgentRequest {
        target,
        duration: REQUESTED_FOR,
        reason: Some("rotate the logs"),
        run_id,
        call_id,
    }
}

/// A machine over `durable` at `clock` that takes its agents' grant
/// requests into a decision owner of its own, at the same clock.
fn machine_taking_requests(
    durable: &Durable,
    clock: &Arc<FakeClock>,
) -> (Arc<MeerkatMachine>, DurableDecisionService) {
    let machine = durable.machine(clock);
    let decisions = DurableDecisionService::with_clock(
        Arc::new(InMemoryDecisionStore::new()),
        Arc::clone(clock) as Arc<dyn DecisionClock>,
    );
    machine.set_tool_grant_request_host(Some(ToolGrantRequestHost {
        decisions: decisions.clone(),
        policy: ToolGrantRequestPolicy::default(),
        requester: agent_requester(),
    }));
    (machine, decisions)
}

/// An agent that asks again after its open request's deadline passed, before
/// the host's executor recorded the expiry, holds a new request: the request
/// that ran out is not the answer.
#[tokio::test]
async fn an_ask_after_the_open_requests_deadline_holds_a_new_one() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let (machine, _decisions) = machine_taking_requests(&durable, &clock);
        let session_id = SessionId::new();
        let bindings = machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("bindings prepare");
        let grants = Arc::clone(bindings.tool_grants().expect("the session's grants"));
        let shell = ToolGrantTarget::Family {
            family: "shell".to_string(),
            names: shell_family(),
        };
        let run_id = RunId::new();
        let first = grants
            .request_grant(agent_ask(&shell, &run_id, "call-1"))
            .await
            .expect("the request is held");
        clock.set(first.deadline_at_ms + 1);
        let again = grants
            .request_grant(agent_ask(&shell, &run_id, "call-2"))
            .await
            .expect("the second ask answers");
        assert_ne!(
            again.decision_id, first.decision_id,
            "{backend:?}: an ask after the open request ran out is answered with it ({:?})",
            again.phase
        );
        assert_eq!(again.phase, DecisionPhase::Open, "{backend:?}");
    }
}

/// An agent's ask is never answered with another subject's request: two
/// sessions asking for the same target each hold a request of their own.
#[tokio::test]
async fn an_agents_ask_never_joins_another_subjects_request() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let (machine, decisions) = machine_taking_requests(&durable, &clock);
        let shell = ToolGrantTarget::Family {
            family: "shell".to_string(),
            names: shell_family(),
        };
        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let bindings_a = machine
            .prepare_bindings(session_a.clone())
            .await
            .expect("bindings prepare");
        let bindings_b = machine
            .prepare_bindings(session_b.clone())
            .await
            .expect("bindings prepare");
        let grants_a = Arc::clone(bindings_a.tool_grants().expect("A's grants"));
        let grants_b = Arc::clone(bindings_b.tool_grants().expect("B's grants"));
        let run_id = RunId::new();

        let of_b = grants_b
            .request_grant(agent_ask(&shell, &run_id, "b-1"))
            .await
            .expect("B's request is held");
        let of_a = grants_a
            .request_grant(agent_ask(&shell, &run_id, "a-1"))
            .await
            .expect("A's request is held");
        assert_ne!(
            of_a.decision_id, of_b.decision_id,
            "{backend:?}: A's ask is answered with B's request"
        );
        for (receipt, session_id) in [(&of_a, &session_a), (&of_b, &session_b)] {
            let held = decisions.get(&receipt.decision_id).expect("the decision");
            let body = ToolGrantIssueBody::of(&held).expect("the body");
            assert_eq!(
                body.subject,
                ToolGrantSubject::session(session_id.clone()),
                "{backend:?}"
            );
        }
    }
}

/// An agent's ask is never answered with a run-scoped request of its
/// subject: a host's open request for one run's use of the target leaves the
/// agent's ask to hold a window request of its own.
#[tokio::test]
async fn an_agents_ask_never_joins_a_run_scoped_request() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let (machine, decisions) = machine_taking_requests(&durable, &clock);
        let shell = ToolGrantTarget::Family {
            family: "shell".to_string(),
            names: shell_family(),
        };
        let session_id = SessionId::new();
        let bindings = machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("bindings prepare");
        let grants = Arc::clone(bindings.tool_grants().expect("the session's grants"));
        let run_scoped = hold_tool_grant_request(
            &decisions,
            &ToolGrantRequestPolicy::default(),
            machine.tool_grant_service().policy().max_lifetime_ms(),
            T0,
            ToolGrantIssueRequest {
                subject: ToolGrantSubject::session(session_id.clone()),
                target: shell.clone(),
                duration: REQUESTED_FOR,
                run_scope: Some(RunId::new()),
                reason: None,
                origin: ToolGrantRequestOrigin::Host {
                    request_id: "host-run-scoped".to_owned(),
                },
                requester: host_requester(),
            },
        )
        .expect("the host's run-scoped request is held");

        let asked = grants
            .request_grant(agent_ask(&shell, &RunId::new(), "call-1"))
            .await
            .expect("the agent's request is held");
        assert_ne!(
            asked.decision_id, run_scoped.decision_id,
            "{backend:?}: the agent's ask is answered with a run-scoped request"
        );
        let held = decisions.get(&asked.decision_id).expect("the decision");
        let body = ToolGrantIssueBody::of(&held).expect("the body");
        assert_eq!(body.run_scope, None, "{backend:?}: a window request");
    }
}

/// An agent's ask that joins its subject's open window request is answered
/// with that request's duration, which can differ from the one it asked, and
/// says it joined; an ask that holds its own request answers its own
/// duration.
#[tokio::test]
async fn an_ask_that_joins_a_request_answers_with_that_requests_duration() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let (machine, decisions) = machine_taking_requests(&durable, &clock);
        let shell = ToolGrantTarget::Family {
            family: "shell".to_string(),
            names: shell_family(),
        };
        let session_id = SessionId::new();
        let bindings = machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("bindings prepare");
        let grants = Arc::clone(bindings.tool_grants().expect("the session's grants"));
        let hosts = hold_tool_grant_request(
            &decisions,
            &ToolGrantRequestPolicy::default(),
            machine.tool_grant_service().policy().max_lifetime_ms(),
            T0,
            ToolGrantIssueRequest {
                subject: ToolGrantSubject::session(session_id.clone()),
                target: shell.clone(),
                duration: Duration::from_mins(1),
                run_scope: None,
                reason: Some("the host's own reason".to_owned()),
                origin: ToolGrantRequestOrigin::Host {
                    request_id: "host-window".to_owned(),
                },
                requester: host_requester(),
            },
        )
        .expect("the host's window request is held");
        let run_id = RunId::new();

        let joined = grants
            .request_grant(agent_ask(&shell, &run_id, "call-1"))
            .await
            .expect("the ask answers");
        assert_eq!(joined.decision_id, hosts.decision_id, "{backend:?}");
        assert!(
            joined.joined,
            "{backend:?}: the ask joined the host's request"
        );
        assert_eq!(
            joined.duration_ms, 60_000,
            "{backend:?}: the joined request's duration, not the ten minutes asked"
        );

        let deploy = ToolGrantTarget::Tool {
            name: ToolName::from("deploy"),
        };
        let own = grants
            .request_grant(agent_ask(&deploy, &run_id, "call-2"))
            .await
            .expect("the request is held");
        assert!(!own.joined, "{backend:?}: a request of its own");
        assert_eq!(own.duration_ms, 600_000, "{backend:?}");
    }
}

/// A list shows a declined request only while its deadline is within the
/// grant policy's maximum lifetime of now: it reads the recent requests
/// through the store's phase and deadline index, never the whole decision
/// history. An open request is listed while it waits, and an older declined
/// decision stays readable as a decision.
#[tokio::test]
async fn a_declined_request_is_listed_only_within_the_grant_lifetime() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let rig = RequestRig::new(&durable);
        let machine = durable.machine(&rig.clock);
        let session_id = SessionId::new();
        let subject = ToolGrantSubject::session(session_id.clone());
        let rejected = rig.request(&session_id, "ask-1", None);
        rig.decide(&rejected, false);
        let deadline_ms = rejected.descriptor.deadline_at_ms;
        let lifetime_ms = machine.tool_grant_service().policy().max_lifetime_ms();
        let listed = |include_ended: bool| {
            machine
                .list_tool_grant_requests(&rig.decisions, &subject, include_ended)
                .expect("the requests")
                .into_iter()
                .map(|(decision, _)| decision.decision_id)
                .collect::<Vec<_>>()
        };

        rig.clock.set(deadline_ms + lifetime_ms);
        assert_eq!(
            listed(true),
            vec![rejected.decision_id.clone()],
            "{backend:?}: within the lifetime"
        );
        assert!(listed(false).is_empty(), "{backend:?}");

        rig.clock.set(deadline_ms + lifetime_ms + 1);
        assert!(listed(true).is_empty(), "{backend:?}: past the lifetime");
        assert_eq!(
            rig.decisions
                .get(&rejected.decision_id)
                .expect("the decision stays readable")
                .state
                .phase,
            DecisionPhase::Rejected
        );

        let open = rig.request(&session_id, "ask-2", None);
        assert_eq!(
            listed(true),
            vec![open.decision_id.clone()],
            "{backend:?}: an open request is listed"
        );
    }
}

/// The identity subject of the member rows: one member of one mob
/// incarnation.
fn member_subject() -> ToolGrantSubject {
    ToolGrantSubject::Identity {
        mob_id: "mob-1".to_string(),
        mob_incarnation: "incarnation-1".to_string(),
        identity: "worker-1".to_string(),
    }
}

fn member_shell_grant(subject: &ToolGrantSubject, lifetime: Duration) -> ToolGrantRequest {
    ToolGrantRequest {
        subject: subject.clone(),
        ..shell_grant(&SessionId::new(), lifetime)
    }
}

/// A member's build served by `session_id`: the member seam over the same
/// grants, reading the member's identity first, then its session.
async fn serve_member(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    member: &ToolGrantSubject,
) -> Served {
    let session = serve(machine, session_id).await;
    let grants = Arc::new(RuntimeSessionToolGrants::for_member_subject(
        machine.tool_grant_service(),
        machine.tool_grant_deadlines().clone(),
        session_id.clone(),
        member,
    ));
    Served {
        scope: session.scope,
        gate: ToolGrantGate::new(grants, gate_table()),
    }
}

/// A member's build reads its identity's grants first, then its
/// session's; a plain session's build reads no identity grant; a refusal no
/// grant explains is audited under the member's identity.
#[tokio::test]
async fn a_member_seam_reads_its_identity_then_its_session() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let member = member_subject();
        let member_session = SessionId::new();
        let served = serve_member(&machine, &member_session, &member).await;

        let refusal = served
            .call("shell", "call-before-any-grant")
            .await
            .expect_err("no grant covers shell yet");
        assert_eq!(refusal, ToolGrantRefusal::Required, "{backend:?}");
        let store = durable.open();
        let member_audit: Vec<ToolGrantAuditEvent> = store
            .read_tool_grant_audit(member.key().as_str(), None, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, bytes)| ToolGrantAuditRow::decode(&bytes).unwrap().event)
            .collect();
        assert!(
            matches!(
                member_audit.as_slice(),
                [ToolGrantAuditEvent::Refused {
                    reason: ToolGrantRefusalKind::Required,
                    ..
                }]
            ),
            "{backend:?}: the refusal is audited under the identity: {member_audit:?}"
        );
        assert!(
            audit_events(&*store, &member_session).await.is_empty(),
            "{backend:?}: nothing is filed under the member's session"
        );

        machine
            .grant_tool(
                member_shell_grant(&member, Duration::from_mins(1)),
                Some(&member_session),
            )
            .await
            .unwrap();
        served.boundary().await.unwrap();
        assert_eq!(
            served.visible(),
            OPEN,
            "{backend:?}: the identity grant opens shell"
        );
        served.call("shell", "call-under-identity").await.unwrap();

        // A plain session's build reads its own grants only.
        let plain_session = SessionId::new();
        let plain = serve(&machine, &plain_session).await;
        plain.boundary().await.unwrap();
        assert_eq!(
            plain.visible(),
            CLOSED,
            "{backend:?}: an identity grant opens nothing for a plain session"
        );

        // The member's session grant counts too.
        let session_only = SessionId::new();
        let other_member = ToolGrantSubject::Identity {
            mob_id: "mob-1".to_string(),
            mob_incarnation: "incarnation-1".to_string(),
            identity: "worker-2".to_string(),
        };
        let other = serve_member(&machine, &session_only, &other_member).await;
        machine
            .grant_tool(shell_grant(&session_only, Duration::from_mins(1)), None)
            .await
            .unwrap();
        other.boundary().await.unwrap();
        assert_eq!(
            other.visible(),
            OPEN,
            "{backend:?}: a member's build reads its session's grants after its identity's"
        );
    }
}

/// An identity grant to a member whose session is
/// served here arms the session's deadline driver for the identity, and the
/// timer expires it with no boundary, dispatch or read.
#[tokio::test]
async fn an_identity_grant_of_a_served_member_expires_by_its_timer() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let member = member_subject();
        let session_id = SessionId::new();
        let _served = serve_member(&machine, &session_id, &member).await;
        let grant = machine
            .grant_tool(
                member_shell_grant(&member, Duration::from_millis(200)),
                Some(&session_id),
            )
            .await
            .unwrap();
        assert_eq!(
            machine
                .tool_grant_deadlines()
                .armed_deadline(&session_id, &member.key()),
            Some(grant.expires_at_ms),
            "{backend:?}: the grant arms its identity on the serving session"
        );

        clock.set(grant.expires_at_ms);
        let store = durable.open();
        let mut expired = None;
        for _ in 0..200 {
            let row = stored(&*store, &grant).await;
            if row.status == ToolGrantStatus::Expired {
                expired = Some(row);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let expired = expired.expect("the deadline timer expires the identity grant");
        assert_eq!(expired.ended_at_ms, Some(grant.expires_at_ms));
        let events: Vec<ToolGrantAuditEvent> = store
            .read_tool_grant_audit(member.key().as_str(), None, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, bytes)| ToolGrantAuditRow::decode(&bytes).unwrap().event)
            .collect();
        assert!(
            matches!(
                events.as_slice(),
                [
                    ToolGrantAuditEvent::Granted { .. },
                    ToolGrantAuditEvent::Expired {
                        driver: ToolGrantExpiryDriver::Timer,
                        ..
                    },
                ]
            ),
            "{backend:?}: {events:?}"
        );

        // A disarm of the session forgets its identity's deadline too.
        machine
            .grant_tool(
                member_shell_grant(&member, Duration::from_mins(5)),
                Some(&session_id),
            )
            .await
            .unwrap();
        machine.tool_grant_deadlines().disarm(&session_id);
        assert_eq!(
            machine
                .tool_grant_deadlines()
                .armed_deadline(&session_id, &member.key()),
            None,
            "{backend:?}"
        );
    }
}

/// Ending a subject revokes each of its live
/// grants, records who ended them, and leaves every other subject's grants
/// and its own ended grants as they stand. A read of one grant answers its
/// row (its subject included), and an unknown grant is not found.
#[tokio::test]
async fn ending_a_subject_revokes_its_live_grants_only() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let member = member_subject();
        let other = ToolGrantSubject::Identity {
            mob_id: "mob-1".to_string(),
            mob_incarnation: "incarnation-1".to_string(),
            identity: "worker-2".to_string(),
        };
        let first = machine
            .grant_tool(member_shell_grant(&member, Duration::from_mins(1)), None)
            .await
            .unwrap();
        let second = machine
            .grant_tool(member_shell_grant(&member, Duration::from_mins(2)), None)
            .await
            .unwrap();
        let revoked_before = machine
            .grant_tool(member_shell_grant(&member, Duration::from_mins(3)), None)
            .await
            .unwrap();
        machine
            .revoke_tool_grant(
                &revoked_before.record.grant_id,
                ToolGrantRevocation::default(),
            )
            .await
            .unwrap();
        let untouched = machine
            .grant_tool(member_shell_grant(&other, Duration::from_mins(1)), None)
            .await
            .unwrap();

        let ended = machine
            .end_tool_grants_of(
                &member,
                ToolGrantRevocation {
                    actor: Some("mob-1".to_string()),
                    reason: Some("identity ended".to_string()),
                },
            )
            .await
            .unwrap();
        assert_eq!(ended, 2, "{backend:?}: the two live grants end");
        let read = machine.tool_grant(&first.record.grant_id).await.unwrap();
        assert_eq!(read.record.subject, member, "{backend:?}");
        assert_eq!(read.status, ToolGrantStatus::Revoked, "{backend:?}");
        assert!(
            matches!(
                machine.tool_grant(&ToolGrantId::mint()).await,
                Err(ToolGrantServiceError::NotFound { .. })
            ),
            "{backend:?}: an unknown grant is not found"
        );
        let store = durable.open();
        for grant in [&first, &second] {
            assert_eq!(
                stored(&*store, grant).await.status,
                ToolGrantStatus::Revoked,
                "{backend:?}"
            );
        }
        assert_eq!(
            stored(&*store, &untouched).await.status,
            ToolGrantStatus::Active,
            "{backend:?}: another identity's grant stays live"
        );
        let events: Vec<ToolGrantAuditEvent> = store
            .read_tool_grant_audit(member.key().as_str(), None, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, bytes)| ToolGrantAuditRow::decode(&bytes).unwrap().event)
            .collect();
        let ends: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    ToolGrantAuditEvent::Revoked { reason: Some(reason), .. }
                        if reason == "identity ended"
                )
            })
            .collect();
        assert_eq!(ends.len(), 2, "{backend:?}: {events:?}");
        assert_eq!(
            machine
                .end_tool_grants_of(&member, ToolGrantRevocation::default())
                .await
                .unwrap(),
            0,
            "{backend:?}: a second end changes nothing"
        );
    }
}

/// A grant that ended with its subject (the identity it was issued to
/// ended) explains no refusal: a later build under the subject's key, a
/// name created again, needs a grant of its own, where an operator's revoke
/// is cited. Both read `revoked`.
#[tokio::test]
async fn a_grant_ended_with_its_subject_explains_no_refusal() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let member = member_subject();
        let member_session = SessionId::new();
        let served = serve_member(&machine, &member_session, &member).await;
        let ended = machine
            .grant_tool(
                member_shell_grant(&member, Duration::from_mins(1)),
                Some(&member_session),
            )
            .await
            .unwrap();
        assert_eq!(
            machine
                .end_tool_grants_of(&member, ToolGrantRevocation::default())
                .await
                .unwrap(),
            1,
            "{backend:?}"
        );
        assert_eq!(
            machine
                .tool_grant(&ended.record.grant_id)
                .await
                .unwrap()
                .status,
            ToolGrantStatus::Revoked,
            "{backend:?}: a grant ended with its subject reads revoked"
        );
        let refusal = served
            .call("shell", "call-after-the-subject-ended")
            .await
            .expect_err("an ended grant admits nothing");
        assert_eq!(
            refusal,
            ToolGrantRefusal::Required,
            "{backend:?}: a grant ended with its subject explains nothing"
        );

        let revoked = machine
            .grant_tool(
                member_shell_grant(&member, Duration::from_mins(1)),
                Some(&member_session),
            )
            .await
            .unwrap();
        machine
            .revoke_tool_grant(&revoked.record.grant_id, ToolGrantRevocation::default())
            .await
            .unwrap();
        let refusal = served
            .call("shell", "call-after-the-revoke")
            .await
            .expect_err("a revoked grant admits nothing");
        assert_eq!(
            refusal,
            ToolGrantRefusal::Revoked {
                grant_id: revoked.record.grant_id.clone(),
            },
            "{backend:?}: an operator's revoke is cited"
        );
    }
}

/// A run-scoped grant to a member identity whose identity ends before the
/// grant's run does ends with the identity: revoked, ended with its subject,
/// its run scope kept. A call of its own run is then refused as needing a
/// grant, citing none, the run's boundary shows nothing, and the run's end
/// changes nothing.
#[tokio::test]
async fn a_run_scoped_identity_grant_ends_with_its_identity() {
    for backend in BACKENDS {
        let durable = Durable::new(backend);
        let clock = FakeClock::at(T0);
        let machine = durable.machine(&clock);
        let member = member_subject();
        let member_session = SessionId::new();
        let served = serve_member(&machine, &member_session, &member).await;
        let run = RunId::new();
        let grant = machine
            .grant_tool(
                ToolGrantRequest {
                    run_scope: Some(run.clone()),
                    ..member_shell_grant(&member, Duration::from_mins(10))
                },
                Some(&member_session),
            )
            .await
            .unwrap();
        served.boundary_in(Some(&run)).await.unwrap();
        assert_eq!(served.visible(), OPEN, "{backend:?}");

        clock.set(T0 + 1_000);
        assert_eq!(
            machine
                .end_tool_grants_of(&member, ToolGrantRevocation::default())
                .await
                .unwrap(),
            1,
            "{backend:?}: the identity's end ends its run-scoped grant"
        );
        let store = durable.open();
        let ended = stored(&*store, &grant).await;
        assert_eq!(ended.status, ToolGrantStatus::Revoked, "{backend:?}");
        assert!(
            ended.subject_ended,
            "{backend:?}: it ended with its subject"
        );
        assert_eq!(ended.ended_at_ms, Some(T0 + 1_000), "{backend:?}");
        assert_eq!(ended.run_scope, Some(run.clone()), "{backend:?}");

        let refusal = served
            .call_in("shell", "call-of-its-run-after-the-end", Some(&run))
            .await
            .expect_err("a grant ended with its subject admits nothing");
        assert_eq!(
            refusal,
            ToolGrantRefusal::Required,
            "{backend:?}: no grant is cited"
        );
        served.boundary_in(Some(&run)).await.unwrap();
        assert_eq!(served.visible(), CLOSED, "{backend:?}");

        served.gate.end_run(&run).await.unwrap();
        assert_eq!(
            stored(&*store, &grant).await,
            ended,
            "{backend:?}: the run's end changes nothing"
        );
    }
}
