#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Scoped tool grants through the runtime: the grant service over a store,
//! a served session's grant gate on its runtime bindings, the visibility
//! owner's gate layer and the deadline driver. Every test runs on the
//! in-memory store and on SQLite, over a host clock the test moves.
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

use meerkat_core::session_owner_wake::{
    SessionOwnerWakeFuture, SessionOwnerWakeOutcome, SessionOwnerWakeRegistry, SessionOwnerWaker,
};
use meerkat_core::tool_grant::{
    GatedToolTable, GatedToolView, InstalledToolGrantGate, ToolGrantCall, ToolGrantGate,
    ToolGrantId, ToolGrantIssuer, ToolGrantRefusal, ToolGrantSubject, ToolGrantTarget,
};
use meerkat_core::types::{SessionId, ToolDef, ToolName, ToolProvenance, ToolSourceKind};
use meerkat_core::{GeneratedToolVisibilityOwner, ToolScope};
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::store::{InMemoryRuntimeStore, RuntimeStore, SqliteRuntimeStore};
use meerkat_runtime::tool_grant::{
    RuntimeSessionToolGrants, StoredToolGrant, ToolGrantAuditEvent, ToolGrantAuditRow,
    ToolGrantClock, ToolGrantClockError, ToolGrantExpiryDriver, ToolGrantRefusalKind,
    ToolGrantRequest, ToolGrantRevocation, ToolGrantService, ToolGrantServiceError,
    ToolGrantStatus, ToolGrantWindow,
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
    }
}

/// A served session: its tool scope over the runtime's visibility owner,
/// and its grant gate over the runtime's grants, read through that owner.
struct Served {
    scope: ToolScope,
    gate: ToolGrantGate,
    owner: GeneratedToolVisibilityOwner,
}

/// The build's gate as the visibility owner installs it.
fn gate_install() -> InstalledToolGrantGate {
    InstalledToolGrantGate::names(gate_table().gated_names())
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
    let owner = bindings.tool_visibility_owner().clone();
    let scope =
        ToolScope::new_with_visibility_owner(tools, HashSet::new(), HashSet::new(), owner.clone())
            .expect("tool scope");
    scope
        .replace_tool_grant_gate(gate_install())
        .expect("the build installs its gate");
    Served {
        scope,
        gate: ToolGrantGate::new(grants, gate_install(), owner.clone()),
        owner,
    }
}

impl Served {
    /// One model-request boundary, as the agent runs it: no MCP server
    /// serves these tools, so the view is the machine's gated names.
    async fn boundary(&self) -> Result<(), ToolGrantRefusal> {
        let gated = self
            .scope
            .visibility_state()
            .expect("the gate reads")
            .tool_grant_gated_names;
        let view = GatedToolView::new(gated, BTreeMap::new());
        match self.gate.open_names(&view).await {
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
                server: None,
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
        gate: ToolGrantGate::new(grants, gate_install(), session.owner.clone()),
        owner: session.owner,
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

/// A build asks which gated MCP servers a live server grant covers: a whole
/// server grant and one narrowed to some of its tools both cover their
/// server, a name grant covers none, and a server grant the build finds past
/// its deadline covers nothing and is settled expired by the build. A
/// boundary opens a server grant's gated names its server provides now,
/// within its tools.
#[tokio::test]
async fn a_build_reads_which_gated_servers_a_live_server_grant_covers() {
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
        let server = |server: &str, tools: Option<&[&str]>| ToolGrantTarget::McpServer {
            server: server.to_string(),
            tools: tools.map(|tools| tools.iter().map(|tool| ToolName::from(*tool)).collect()),
        };
        for (target, lifetime) in [
            (server("tickets", None), Duration::from_mins(10)),
            (
                server("calendar", Some(&["today"])),
                Duration::from_mins(10),
            ),
            (server("mail", None), Duration::from_mins(1)),
            (
                ToolGrantTarget::Tool {
                    name: ToolName::from("deploy"),
                },
                Duration::from_mins(10),
            ),
        ] {
            machine
                .grant_tool(
                    ToolGrantRequest {
                        subject: ToolGrantSubject::session(session_id.clone()),
                        target,
                        issuer: ToolGrantIssuer::operator(),
                        reason: None,
                        window: ToolGrantWindow::For(lifetime),
                    },
                    None,
                )
                .await
                .unwrap();
        }
        clock.set(T0 + 120_000);

        let asked: BTreeSet<String> = ["tickets", "calendar", "mail", "deploy", "unknown"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(
            grants
                .granted_servers(&asked)
                .await
                .expect("the grants read"),
            BTreeSet::from(["calendar".to_string(), "tickets".to_string()]),
            "{backend:?}"
        );
        assert!(
            grants
                .granted_servers(&BTreeSet::new())
                .await
                .expect("nothing asked reads nothing")
                .is_empty()
        );
        let events = audit_events(durable.open().as_ref(), &session_id).await;
        assert!(
            events.iter().any(|event| matches!(
                event,
                ToolGrantAuditEvent::Expired {
                    driver: ToolGrantExpiryDriver::Build,
                    ..
                }
            )),
            "{backend:?}: the build settles the lapsed grant: {events:?}"
        );

        let names = |names: &[&str]| -> BTreeSet<ToolName> {
            names.iter().map(|name| ToolName::from(*name)).collect()
        };
        let view = GatedToolView::new(
            names(&["close_ticket", "deploy", "open_ticket", "today", "tomorrow"]),
            BTreeMap::from([
                (
                    "tickets".to_string(),
                    names(&["close_ticket", "open_ticket"]),
                ),
                ("calendar".to_string(), names(&["today", "tomorrow"])),
            ]),
        );
        let open: BTreeSet<ToolName> = grants
            .open_names(&view)
            .await
            .expect("the grants read")
            .into_inner()
            .into_iter()
            .collect();
        assert_eq!(
            open,
            names(&["close_ticket", "deploy", "open_ticket", "today"]),
            "{backend:?}"
        );
    }
}
