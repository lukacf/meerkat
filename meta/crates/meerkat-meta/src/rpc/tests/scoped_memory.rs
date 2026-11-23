//! Rows for the scoped memory calls over the stdio RPC surface, and the
//! fixture they share: `meta/agent_memory/forget_in_scope` here.

use super::*;
use crate::memory::capabilities::StewardStore;
use crate::memory::records::{
    ManifestTier, MemoryAuthor, MemoryKind, MemoryRecord, MemoryScope, NewMemoryRecord,
    RecordStatus, TrustTier,
};
use crate::memory::staged::{StagedBatchKind, StagedMemoryStore, StagedMutationBatch, StagedOp};

pub(super) const REALM: &str = "team";
pub(super) const MOB: &str = "mob:main";

pub(super) type RowResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// A gateway with the bundled memory store and one registered identity,
/// answering the stdio RPC surface.
pub(super) struct ScopedMemoryFixture {
    runtime: UnifiedRuntime,
    identity_ctx: IdentityFirstContext,
    pub(super) store: Arc<SqliteAgentMemoryStore>,
    _temp_dir: tempfile::TempDir,
}

pub(super) fn mob_scope(mob: &str) -> MemoryScope {
    MemoryScope::Mob {
        realm: REALM.to_string(),
        mob: mob.to_string(),
    }
}

pub(super) fn identity_scope(identity: &str) -> MemoryScope {
    MemoryScope::Identity {
        realm: REALM.to_string(),
        identity: identity.to_string(),
    }
}

impl ScopedMemoryFixture {
    pub(super) async fn new(
        namespace: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: namespace.to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: namespace.to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });
        let store = Arc::new(SqliteAgentMemoryStore::open(
            temp_dir.path().join("agent-memory"),
        )?);
        let provider: Arc<dyn AgentMemoryProvider> = store.clone();
        identity_rt
            .set_agent_memory(Some(
                crate::identity_first::AgentMemoryRuntimeInjector::new(
                    provider.clone(),
                    crate::identity_first::AgentMemoryConfig::default(),
                ),
            ))
            .await;
        identity_rt
            .register(DurableAgentSpec {
                identity: AgentIdentity::parse("identity:luka")?,
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: Default::default(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await
            .expect("the identity registers");
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::new(identity_rt),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: Some(provider),
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };
        Ok(Self {
            runtime,
            identity_ctx,
            store,
            _temp_dir: temp_dir,
        })
    }

    /// One request on the stdio surface; the whole JSON-RPC response.
    pub(super) async fn rpc(&self, method: &str, params: Value) -> Value {
        let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        serde_json::from_str(
            &handle_unified_rpc_json(
                &self.runtime,
                &request.to_string(),
                Duration::from_secs(1),
                None,
                Some(&self.identity_ctx),
            )
            .await,
        )
        .expect("a JSON-RPC response")
    }

    /// The methods the gateway advertises.
    pub(super) async fn advertised(&self) -> Vec<String> {
        self.rpc("meta/capabilities", json!({})).await["result"]["methods"]
            .as_array()
            .expect("methods")
            .iter()
            .filter_map(|method| method.as_str().map(str::to_string))
            .collect()
    }

    /// An active record written by the application, as a host would.
    pub(super) async fn seed(&self, id: &str, scope: MemoryScope, title: &str, body: &str) {
        let batch = StagedMutationBatch {
            kind: StagedBatchKind::FreshWrite,
            realm: REALM.to_string(),
            author: MemoryAuthor::Application,
            ops: vec![StagedOp::Create {
                id: Some(id.to_string()),
                scope,
                record: NewMemoryRecord {
                    kind: MemoryKind::Fact,
                    title: title.to_string(),
                    description: format!("about {title}"),
                    body: body.to_string(),
                    tags: vec!["seeded".to_string()],
                    evidence: Vec::new(),
                    verification: None,
                },
                trust: TrustTier::AgentObserved,
                derived_from: Vec::new(),
                rationale: Some(format!("seeded: {title}")),
                created_at_ms: None,
                updated_at_ms: None,
            }],
        };
        let token = self.store.stage(batch).await.expect("stage");
        self.store.commit(token).await.expect("commit");
    }

    pub(super) async fn record(&self, id: &str) -> MemoryRecord {
        self.store
            .records_by_ids(REALM, &[id.to_string()])
            .await
            .expect("read")
            .remove(0)
    }

    pub(super) async fn active_titles(&self, scope: &MemoryScope) -> Vec<String> {
        self.store
            .manifest(std::slice::from_ref(scope), ManifestTier::Full)
            .await
            .expect("manifest")
            .into_iter()
            .map(|meta| meta.title)
            .collect()
    }

    /// The audit rows of one record, oldest first: the op kind and the
    /// row's typed detail. Read from the realm's database file, as an
    /// operator's inspection would.
    pub(super) fn audit_rows(&self, id: &str) -> Vec<(String, Value)> {
        let conn = rusqlite::Connection::open(self.store.path_for_realm(REALM)).expect("open");
        let mut stmt = conn
            .prepare("SELECT op_kind, detail FROM audit WHERE memory_id = ?1 ORDER BY audit_id")
            .expect("prepare");
        stmt.query_map([id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("query")
        .map(|row| {
            let (kind, detail) = row.expect("row");
            (kind, serde_json::from_str(&detail).expect("detail is JSON"))
        })
        .collect()
    }
}

fn forget_in_scope_params(kind: &str, key: Option<&str>, memory_id: &str) -> Value {
    let mut scope = json!({ "kind": kind });
    if let Some(key) = key {
        scope["key"] = json!(key);
    }
    json!({ "realm": REALM, "scope": scope, "memory_id": memory_id })
}

/// Until this call no host call could take a record out of the mob's shared
/// memory: forget reaches an identity's scope only.
#[tokio::test]
async fn forget_in_scope_tombstones_a_mob_record_and_writes_one_audit_row() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-forget-in-scope-mob").await?;
    fixture
        .seed(
            "mem-shared",
            mob_scope(MOB),
            "Shared deploy note",
            "everyone reads this",
        )
        .await;

    let response = fixture
        .rpc(
            "meta/agent_memory/forget_in_scope",
            forget_in_scope_params("mob", Some(MOB), "mem-shared"),
        )
        .await;

    assert!(response["error"].is_null(), "{response:#?}");
    assert_eq!(
        response["result"],
        json!({ "memory_id": "mem-shared", "deleted": true })
    );
    assert_eq!(
        fixture.record("mem-shared").await.status,
        RecordStatus::Tombstoned
    );
    // The next build's index of the mob's scope no longer lists it.
    assert!(fixture.active_titles(&mob_scope(MOB)).await.is_empty());
    let tombstones: Vec<Value> = fixture
        .audit_rows("mem-shared")
        .into_iter()
        .filter(|(kind, _)| kind == "tombstone")
        .map(|(_, detail)| detail)
        .collect();
    assert_eq!(tombstones.len(), 1, "{tombstones:#?}");
    assert_eq!(
        tombstones[0]["author"],
        json!({ "author": "application" }),
        "a host's removal is the application's"
    );
    Ok(())
}

/// The call never confirms an id in a scope the caller did not name, and a
/// repeated removal removes nothing.
#[tokio::test]
async fn forget_in_scope_answers_deleted_false_for_another_mob_an_identity_record_and_a_second_call()
-> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-forget-in-scope-false").await?;
    fixture
        .seed("mem-shared", mob_scope(MOB), "Shared note", "shared body")
        .await;
    fixture
        .seed(
            "mem-own",
            identity_scope("identity:luka"),
            "Own note",
            "own body",
        )
        .await;
    let call = |params: Value| fixture.rpc("meta/agent_memory/forget_in_scope", params);
    let not_deleted = |id: &str| json!({ "memory_id": id, "deleted": false });

    let other_mob = call(forget_in_scope_params(
        "mob",
        Some("mob:other"),
        "mem-shared",
    ))
    .await;
    assert_eq!(
        other_mob["result"],
        not_deleted("mem-shared"),
        "{other_mob:#?}"
    );
    let identity_record = call(forget_in_scope_params("mob", Some(MOB), "mem-own")).await;
    assert_eq!(
        identity_record["result"],
        not_deleted("mem-own"),
        "{identity_record:#?}"
    );
    let unknown = call(forget_in_scope_params("mob", Some(MOB), "mem-nowhere")).await;
    assert_eq!(
        unknown["result"],
        not_deleted("mem-nowhere"),
        "{unknown:#?}"
    );
    assert_eq!(
        fixture.record("mem-shared").await.status,
        RecordStatus::Active
    );
    assert_eq!(fixture.record("mem-own").await.status, RecordStatus::Active);

    let first = call(forget_in_scope_params("mob", Some(MOB), "mem-shared")).await;
    assert_eq!(first["result"]["deleted"], json!(true), "{first:#?}");
    let second = call(forget_in_scope_params("mob", Some(MOB), "mem-shared")).await;
    assert_eq!(second["result"], not_deleted("mem-shared"), "{second:#?}");
    Ok(())
}

/// An identity's own record keeps `meta/agent_memory/forget`, a person's
/// operator memory is not reached, and a malformed scope is refused: each
/// is invalid params, and nothing is written.
#[tokio::test]
async fn forget_in_scope_refuses_identity_and_operator_scope_kinds() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-forget-in-scope-kinds").await?;
    fixture
        .seed(
            "mem-own",
            identity_scope("identity:luka"),
            "Own note",
            "own body",
        )
        .await;
    for params in [
        forget_in_scope_params("identity", Some("identity:luka"), "mem-own"),
        forget_in_scope_params("operator", Some("console-caller:v1:aa"), "mem-own"),
        forget_in_scope_params("team", Some(MOB), "mem-own"),
        forget_in_scope_params("mob", None, "mem-own"),
        forget_in_scope_params("mob", Some("  "), "mem-own"),
        json!({ "realm": REALM, "memory_id": "mem-own" }),
        json!({ "realm": REALM, "scope": "mob", "memory_id": "mem-own" }),
        json!({ "realm": REALM, "scope": { "kind": "mob", "key": MOB } }),
    ] {
        let response = fixture
            .rpc("meta/agent_memory/forget_in_scope", params.clone())
            .await;
        assert_eq!(
            response["error"]["code"],
            json!(-32602),
            "{params}: {response:#?}"
        );
    }
    assert_eq!(fixture.record("mem-own").await.status, RecordStatus::Active);
    Ok(())
}

/// Advertised beside forget, and one of the writes a candidate launch
/// refuses.
#[tokio::test]
async fn forget_in_scope_is_advertised_and_refused_on_a_candidate_launch() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-forget-in-scope-advertised").await?;
    let advertised = fixture.advertised().await;
    assert!(
        advertised
            .iter()
            .any(|method| method == "meta/agent_memory/forget_in_scope"),
        "{advertised:#?}"
    );
    assert!(
        crate::rpc::candidate_launch_refuses("meta/agent_memory/forget_in_scope", &json!({}), None),
        "a removal is a write the authoritative launch owns"
    );
    Ok(())
}

/// The removal names a shared scope. A record kept under a member's
/// owner lies in no shared scope: its id is confirmed in none, and the
/// record stays.
#[tokio::test]
async fn forget_in_scope_reaches_no_record_kept_under_a_members_owner() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-forget-in-scope-owner-kept").await?;
    let kept = identity_scope(&crate::memory::records::private_member_scope_key(
        "console-caller:v1:alice",
        "identity:luka",
    ));
    fixture
        .seed("mem-kept", kept.clone(), "Own note", "own body")
        .await;
    for params in [
        forget_in_scope_params("mob", Some(MOB), "mem-kept"),
        forget_in_scope_params("realm", None, "mem-kept"),
    ] {
        let response = fixture
            .rpc("meta/agent_memory/forget_in_scope", params.clone())
            .await;
        assert_eq!(
            response["result"],
            json!({ "memory_id": "mem-kept", "deleted": false }),
            "{params}: {response:#?}"
        );
    }
    let record = fixture.record("mem-kept").await;
    assert_eq!(record.status, RecordStatus::Active);
    assert_eq!(record.scope, kept);
    Ok(())
}
