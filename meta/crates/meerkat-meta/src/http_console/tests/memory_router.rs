//! Rows that send the purge and the removal from a shared scope through the
//! Console's HTTP action, `POST /console/rpc`, as signed-in callers under
//! access rules that are on: the caller's token decides the view, and the
//! view decides what the door does.

use super::super::console_human_admission_tests::BOB;
use super::super::console_json_router_with_state;
use super::super::console_private_audience_tests::fully_composed_console;
use super::super::console_private_side_chat_tests::{
    ALICE, ROOT, TestResult, alice_claims, bearer, bob_claims, runtime,
};
use super::super::console_side_chat_tests::seeded_member;
use super::super::console_token_lapse_tests::console_rpc_as;
use super::*;
use crate::access::AccessRule;
use crate::identity_first::agent_memory::AgentMemoryProvider;
use crate::memory::SqliteAgentMemoryStore;
use crate::memory::capabilities::StewardStore;
use crate::memory::records::{
    MemoryAuthor, MemoryKind, MemoryRecord, MemoryScope, NewMemoryRecord, RecordStatus, TrustTier,
};
use crate::memory::staged::{StagedBatchKind, StagedMemoryStore, StagedMutationBatch, StagedOp};
use axum::Router;

const REALM: &str = "team";
/// A member every caller may know.
const SHARED: &str = "alpha";

/// A Console with sign-in, agent memory and the access rules `rules`, and a
/// second handle on its memory store to seed and to read rows with.
struct SignedInMemoryConsole {
    _runtime_dir: tempfile::TempDir,
    _memory_dir: tempfile::TempDir,
    mob: String,
    app: Router,
    store: SqliteAgentMemoryStore,
}

impl SignedInMemoryConsole {
    /// With the access rules `rules` on, and `root` the one administrator.
    async fn new(
        mob: &str,
        rules: Vec<AccessRule>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::composed(
            mob,
            Some(crate::access::AccessControlConfig {
                enabled: true,
                admins: vec![ROOT.to_string()],
                rules,
                ..crate::access::AccessControlConfig::default()
            }),
        )
        .await
    }

    /// With the access configuration `access`, or with none: sign-in for
    /// two people and no access controller. The Console reads its memory
    /// panel from the same store.
    async fn composed(
        mob: &str,
        access: Option<crate::access::AccessControlConfig>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (runtime_dir, runtime) = runtime(mob).await?;
        seeded_member(&runtime, "worker", SHARED, &[]).await?;
        let memory_dir = tempfile::tempdir()?;
        let mut state = fully_composed_console(&runtime, SHARED, memory_dir.path()).await?;
        state.access = access.map(AccessController::new).transpose()?;
        let store = SqliteAgentMemoryStore::open(memory_dir.path())?;
        state.memory_panel = Some(Arc::new(store.clone()));
        Ok(Self {
            _runtime_dir: runtime_dir,
            _memory_dir: memory_dir,
            mob: mob.to_string(),
            app: console_json_router_with_state(state),
            store,
        })
    }

    /// An active record `id` of `scope`.
    async fn seed(&self, id: &str, scope: MemoryScope) {
        let batch = StagedMutationBatch {
            kind: StagedBatchKind::FreshWrite,
            realm: REALM.to_string(),
            author: MemoryAuthor::Application,
            ops: vec![StagedOp::Create {
                id: Some(id.to_string()),
                scope,
                record: NewMemoryRecord {
                    kind: MemoryKind::Fact,
                    title: "Home address".to_string(),
                    description: String::new(),
                    body: "12 Example Lane".to_string(),
                    tags: Vec::new(),
                    evidence: Vec::new(),
                    verification: None,
                },
                trust: TrustTier::AgentObserved,
                derived_from: Vec::new(),
                rationale: None,
                created_at_ms: None,
                updated_at_ms: None,
            }],
        };
        let token = self.store.stage(batch).await.expect("stage");
        self.store.commit(token).await.expect("commit");
    }

    async fn record(&self, id: &str) -> MemoryRecord {
        self.store
            .records_by_ids(REALM, &[id.to_string()])
            .await
            .expect("read")
            .remove(0)
    }

    /// `method` on `POST /console/rpc` as the holder of `claims`.
    async fn rpc_as(&self, claims: Value, method: &str, params: Value) -> Value {
        console_rpc_as(&self.app, Some(&bearer(claims)), method, params).await
    }
}

/// A rule that gives `subject` `actions` on `agents`, or with no agent
/// named when `agents` is empty.
fn allow(id: &str, subject: &str, actions: &[&str], agents: &[&str]) -> AccessRule {
    AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: actions.iter().map(ToString::to_string).collect(),
        agents: agents.iter().map(ToString::to_string).collect(),
        ..AccessRule::default()
    }
}

/// The rules' refusal of `action` on `resource`, whole.
fn refused(action: &str, resource: Value) -> Value {
    json!({
        "code": -32030,
        "message": format!("access denied: {action}"),
        "data": { "kind": "access_denied", "action": action, "resource": resource },
    })
}

/// The purge through the Console's HTTP action: a signed-in caller who may
/// view the agent and holds no purge grant on it is refused by the rules
/// and nothing is blanked; one who holds the grant purges the record.
#[tokio::test]
async fn the_http_action_purges_only_for_a_caller_who_holds_the_grant() -> TestResult {
    let admin = crate::access::ACTION_AGENT_MEMORY_ADMIN;
    let console = SignedInMemoryConsole::new(
        "memory-router-purge",
        vec![
            allow("alice-views", ALICE, &[ACTION_AGENT_VIEW], &[SHARED]),
            allow("bob-purges", BOB, &[ACTION_AGENT_VIEW, admin], &[SHARED]),
        ],
    )
    .await?;
    let scope = MemoryScope::Identity {
        realm: REALM.to_string(),
        identity: SHARED.to_string(),
    };
    console.seed("mem-own", scope.clone()).await;
    console
        .store
        .forget_authored(
            &scope,
            "mem-own",
            MemoryAuthor::Operator { principal: None },
        )
        .await?;
    let purge = json!({ "realm": REALM, "identity": SHARED, "memory_id": "mem-own" });

    let alices = console
        .rpc_as(alice_claims(), "meta/agent_memory/purge", purge.clone())
        .await;
    assert!(alices["result"].is_null(), "{alices:#?}");
    assert_eq!(
        alices["error"],
        refused(admin, json!(SHARED)),
        "{alices:#?}"
    );
    assert_eq!(console.record("mem-own").await.body, "12 Example Lane");

    let bobs = console
        .rpc_as(bob_claims(), "meta/agent_memory/purge", purge)
        .await;
    assert_eq!(
        bobs["result"],
        json!({ "memory_id": "mem-own", "purged": true, "purged_ids": ["mem-own"] }),
        "{bobs:#?}"
    );
    let record = console.record("mem-own").await;
    assert_eq!(record.status, RecordStatus::Tombstoned);
    assert_eq!((record.title.as_str(), record.body.as_str()), ("", ""));
    Ok(())
}

/// The removal from a mob's shared memory through the Console's HTTP
/// action: a signed-in caller without `mob.memory.delete` is refused by the
/// rules and the record stays; one who holds it removes the record.
#[tokio::test]
async fn the_http_action_removes_a_shared_record_only_for_a_caller_who_holds_the_grant()
-> TestResult {
    let delete = crate::access::ACTION_MOB_MEMORY_DELETE;
    let console = SignedInMemoryConsole::new(
        "memory-router-removal",
        vec![
            allow("alice-views", ALICE, &[ACTION_AGENT_VIEW], &[SHARED]),
            // The removal names no agent: the grant is a rule with none.
            allow("bob-removes", BOB, &[delete], &[]),
        ],
    )
    .await?;
    let scope = MemoryScope::Mob {
        realm: REALM.to_string(),
        mob: console.mob.clone(),
    };
    console.seed("mem-shared", scope).await;
    let removal = json!({
        "realm": REALM,
        "scope": { "kind": "mob", "key": console.mob },
        "memory_id": "mem-shared",
    });

    let alices = console
        .rpc_as(
            alice_claims(),
            "meta/agent_memory/forget_in_scope",
            removal.clone(),
        )
        .await;
    assert!(alices["result"].is_null(), "{alices:#?}");
    assert_eq!(alices["error"], refused(delete, Value::Null), "{alices:#?}");
    assert_eq!(
        console.record("mem-shared").await.status,
        RecordStatus::Active
    );

    let bobs = console
        .rpc_as(bob_claims(), "meta/agent_memory/forget_in_scope", removal)
        .await;
    assert_eq!(
        bobs["result"],
        json!({ "memory_id": "mem-shared", "deleted": true }),
        "{bobs:#?}"
    );
    assert_eq!(
        console.record("mem-shared").await.status,
        RecordStatus::Tombstoned
    );
    Ok(())
}

/// The scope the notes of the shared member's name were set aside under,
/// by the key the setting aside itself writes.
fn set_aside_scope() -> MemoryScope {
    MemoryScope::Identity {
        realm: REALM.to_string(),
        identity: crate::memory::sqlite_store::set_aside_scope_key(SHARED),
    }
}

/// A set-aside note (`mem-aside`, retired as a set-aside note is), a record
/// of the same scope that is not retired (`mem-aside-live`), and a record
/// under the plain name (`mem-plain`).
async fn seed_set_aside(console: &SignedInMemoryConsole) {
    console.seed("mem-aside", set_aside_scope()).await;
    let removed = console
        .store
        .forget_authored(
            &set_aside_scope(),
            "mem-aside",
            MemoryAuthor::Operator { principal: None },
        )
        .await
        .expect("forget");
    assert!(removed.deleted, "the note is retired");
    console.seed("mem-aside-live", set_aside_scope()).await;
    console
        .seed(
            "mem-plain",
            MemoryScope::Identity {
                realm: REALM.to_string(),
                identity: SHARED.to_string(),
            },
        )
        .await;
}

fn named(memory_id: &str) -> Value {
    json!({ "realm": REALM, "identity": SHARED, "memory_id": memory_id })
}

/// The ids the memory panel lists to the holder of `claims`, active and
/// retired.
async fn listed(console: &SignedInMemoryConsole, claims: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for status in ["active", "tombstoned"] {
        let answer = console
            .rpc_as(
                claims.clone(),
                "meta/memory/panel/records",
                json!({ "realm": REALM, "status": status }),
            )
            .await;
        assert!(
            answer["result"]["records"].is_array(),
            "the panel lists {status} records: {answer:#?}"
        );
        ids.extend(
            answer["result"]["records"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|record| record["id"].as_str().map(str::to_string)),
        );
    }
    ids
}

/// What a person who is no administrator gets for a set-aside note: the
/// panel's list does not hold it, its read by id shows nothing of it, the
/// purge answers as for a record that is not there, the forget answers as
/// for an id no record holds, and nothing changes.
async fn assert_not_reached(console: &SignedInMemoryConsole, claims: &Value, who: &str) {
    let shown = listed(console, claims).await;
    assert!(
        shown.contains(&"mem-plain".to_string()),
        "{who}: the list holds the plain name's record: {shown:?}"
    );
    assert!(
        !shown.iter().any(|id| id.starts_with("mem-aside")),
        "{who}: {shown:?}"
    );
    let read = console
        .rpc_as(
            claims.clone(),
            "meta/memory/panel/record",
            json!({ "realm": REALM, "memory_id": "mem-aside" }),
        )
        .await;
    assert!(
        read["result"].is_null() && !read["error"].is_null(),
        "{who}: {read:#?}"
    );
    let read = read.to_string();
    assert!(
        !read.contains("12 Example Lane") && !read.contains("Home address"),
        "{who}: {read}"
    );

    let purge = console
        .rpc_as(
            claims.clone(),
            "meta/agent_memory/purge",
            named("mem-aside"),
        )
        .await;
    assert!(purge["error"].is_null(), "{who}: {purge:#?}");
    assert_eq!(
        purge["result"],
        json!({ "memory_id": "mem-aside", "purged": false, "purged_ids": [] }),
        "{who}: {purge:#?}"
    );
    let forget = console
        .rpc_as(
            claims.clone(),
            "meta/agent_memory/forget",
            named("mem-aside-live"),
        )
        .await;
    let no_such_record = console
        .rpc_as(
            claims.clone(),
            "meta/agent_memory/forget",
            named("mem-nowhere"),
        )
        .await;
    assert_eq!(
        forget.to_string().replace("mem-aside-live", "mem-nowhere"),
        no_such_record.to_string(),
        "{who}: the id opens nothing"
    );

    let note = console.record("mem-aside").await;
    assert_eq!(note.status, RecordStatus::Tombstoned, "{who}");
    assert_eq!(note.body, "12 Example Lane", "{who}");
    assert_eq!(
        console.record("mem-aside-live").await.status,
        RecordStatus::Active,
        "{who}"
    );
}

/// A Console with sign-in and no access configuration admits several
/// people and has no administrator. A note that was set aside from a plain
/// member name is shown to and reached by nobody there, through the
/// Console's HTTP action: neither signed-in person finds it in the memory
/// panel's list or reads it by its id, purges it or retires a record of
/// its scope.
#[tokio::test]
async fn nobody_reaches_a_set_aside_note_on_a_console_with_sign_in_and_no_access_configuration()
-> TestResult {
    let console = SignedInMemoryConsole::composed("memory-router-set-aside-nobody", None).await?;
    seed_set_aside(&console).await;
    for (claims, who) in [(alice_claims(), "alice"), (bob_claims(), "bob")] {
        assert_not_reached(&console, &claims, who).await;
    }
    Ok(())
}

/// The control: on a Console with an access configuration that lists one
/// of the two people as administrator (its rules not enforced), that
/// person finds the set-aside note in the panel's list, reads it by its
/// id, purges it and retires a record of its scope; the other person is
/// answered as on a Console without access configuration.
#[tokio::test]
async fn an_administrator_reads_purges_and_retires_a_set_aside_note_through_the_http_action()
-> TestResult {
    let console = SignedInMemoryConsole::composed(
        "memory-router-set-aside-admin",
        Some(crate::access::AccessControlConfig {
            enabled: false,
            admins: vec![ALICE.to_string()],
            ..crate::access::AccessControlConfig::default()
        }),
    )
    .await?;
    seed_set_aside(&console).await;
    assert_not_reached(&console, &bob_claims(), "bob").await;

    let alice = alice_claims();
    let shown = listed(&console, &alice).await;
    for id in ["mem-aside", "mem-aside-live", "mem-plain"] {
        assert!(shown.contains(&id.to_string()), "{id}: {shown:?}");
    }
    let read = console
        .rpc_as(
            alice.clone(),
            "meta/memory/panel/record",
            json!({ "realm": REALM, "memory_id": "mem-aside" }),
        )
        .await;
    assert!(read["error"].is_null(), "{read:#?}");
    assert_eq!(
        read["result"]["record"]["id"],
        json!("mem-aside"),
        "{read:#?}"
    );
    assert_eq!(
        read["result"]["record"]["body"],
        json!("12 Example Lane"),
        "{read:#?}"
    );

    let purge = console
        .rpc_as(alice.clone(), "meta/agent_memory/purge", named("mem-aside"))
        .await;
    assert_eq!(
        purge["result"],
        json!({ "memory_id": "mem-aside", "purged": true, "purged_ids": ["mem-aside"] }),
        "{purge:#?}"
    );
    assert_eq!(console.record("mem-aside").await.body, "");
    let forget = console
        .rpc_as(alice, "meta/agent_memory/forget", named("mem-aside-live"))
        .await;
    assert_eq!(
        forget["result"],
        json!({ "memory_id": "mem-aside-live", "deleted": true }),
        "{forget:#?}"
    );
    let retired = console.record("mem-aside-live").await;
    assert_eq!(retired.status, RecordStatus::Tombstoned);
    assert_eq!(retired.scope, set_aside_scope(), "the row stays");
    Ok(())
}
