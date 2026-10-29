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

use meerkat_core::ToolScope;
use meerkat_core::session_owner_wake::{
    SessionOwnerWakeFuture, SessionOwnerWakeOutcome, SessionOwnerWakeRegistry, SessionOwnerWaker,
};
use meerkat_core::tool_grant::{
    GatedToolTable, ToolGrantCall, ToolGrantGate, ToolGrantId, ToolGrantIssuer, ToolGrantRefusal,
    ToolGrantSubject, ToolGrantTarget,
};
use meerkat_core::types::{SessionId, ToolDef, ToolName, ToolProvenance, ToolSourceKind};
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::store::{InMemoryRuntimeStore, RuntimeStore, SqliteRuntimeStore};
use meerkat_runtime::tool_grant::{
    StoredToolGrant, ToolGrantAuditEvent, ToolGrantAuditRow, ToolGrantClock, ToolGrantClockError,
    ToolGrantExpiryDriver, ToolGrantRefusalKind, ToolGrantRequest, ToolGrantRevocation,
    ToolGrantService, ToolGrantServiceError, ToolGrantStatus, ToolGrantWindow,
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
        },
        reason: Some("maintenance".to_string()),
        window: ToolGrantWindow::For(lifetime),
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
    /// One model-request boundary, as the agent runs it.
    async fn boundary(&self) -> Result<(), ToolGrantRefusal> {
        match self.gate.open_names().await {
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
            .grant_tool(shell_grant(&session_id, Duration::from_secs(60)))
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
            .list_tool_grants(&ToolGrantSubject::session(session_id.clone()))
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
            .grant_tool(shell_grant(&session_id, Duration::from_secs(60)))
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
            .grant_tool(shell_grant(&session_id, Duration::from_secs(60)))
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
            .list_tool_grants(&ToolGrantSubject::session(session_id.clone()))
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
            .grant_tool(shell_grant(&session_id, Duration::from_secs(60)))
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
            .list_tool_grants(&ToolGrantSubject::session(session_id.clone()))
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
            .grant_tool(shell_grant(&session_id, Duration::from_secs(60)))
            .await
            .unwrap();
        assert_eq!(
            machine.tool_grant_deadlines().armed_deadline(&session_id),
            None,
            "{backend:?}: an unserved session is not armed"
        );

        clock.set(grant.expires_at_ms + 1);
        let subject = ToolGrantSubject::session(session_id.clone());
        let listed = machine.list_tool_grants(&subject).await.unwrap();
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
            .grant_tool(shell_grant(&session_id, Duration::from_millis(200)))
            .await
            .unwrap();
        assert_eq!(
            machine.tool_grant_deadlines().armed_deadline(&session_id),
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
            machine.tool_grant_deadlines().armed_deadline(&session_id),
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
            .grant_tool(shell_grant(&session_id, Duration::from_secs(60)))
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
            .grant_tool(shell_grant(&session_id, Duration::from_secs(60)))
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
            .grant_tool(shell_grant(&session_id, Duration::from_secs(60)))
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

        let listed = machine.list_tool_grants(&subject).await.unwrap();
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
            .grant_tool(shell_grant(&session_id, Duration::from_secs(60)))
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
            .grant_tool(shell_grant(&session_id, Duration::from_secs(600)))
            .await
            .unwrap();
        machine
            .revoke_tool_grant(&revoked.record.grant_id, ToolGrantRevocation::default())
            .await
            .unwrap();
        let _served = serve(&machine, &session_id).await;
        assert_eq!(
            machine.tool_grant_deadlines().armed_deadline(&session_id),
            None,
            "{backend:?}"
        );

        let listed = machine.list_tool_grants(&subject).await.unwrap();
        assert_eq!(
            listed.grants[0].status,
            ToolGrantStatus::Revoked,
            "{backend:?}"
        );
        assert_eq!(
            machine.tool_grant_deadlines().armed_deadline(&session_id),
            None,
            "{backend:?}: a revoked grant arms no deadline, though its deadline lies ahead"
        );

        let live = machine
            .grant_tool(shell_grant(&session_id, Duration::from_secs(300)))
            .await
            .unwrap();
        machine.tool_grant_deadlines().disarm(&session_id);
        machine.list_tool_grants(&subject).await.unwrap();
        assert_eq!(
            machine.tool_grant_deadlines().armed_deadline(&session_id),
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
        .grant_tool(shell_grant(&session_id, Duration::from_secs(60)))
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
