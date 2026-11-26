//! The fixture the Console's memory rows share: a Console runtime with
//! the bundled memory store.

use super::*;
use crate::identity_first::agent_memory::AgentMemoryProvider;
use crate::memory::capabilities::StewardStore;
use crate::memory::records::{
    MemoryAuthor, MemoryKind, MemoryRecord, MemoryScope, NewMemoryRecord, TrustTier,
};
use crate::memory::staged::{StagedBatchKind, StagedMemoryStore, StagedMutationBatch, StagedOp};

pub(super) const REALM: &str = "team";
pub(super) const MOB: &str = "mob:main";

pub(super) type RowResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// A Console runtime with the bundled memory store.
pub(super) struct ConsoleMemoryFixture {
    runtime: MobRuntime,
    identity_runtime: Arc<IdentityRuntime>,
    pub(super) store: Arc<SqliteAgentMemoryStore>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

/// A caller of a Console that has an access controller: the view the
/// Console takes for the caller, and the principal its token names.
pub(super) struct ConsoleCaller<'a> {
    pub(super) controller: &'a crate::access::AccessController,
    pub(super) view: crate::access::AccessView,
    pub(super) principal: Option<&'a str>,
}

impl<'a> ConsoleCaller<'a> {
    /// The signed-in person `subject`, who owns no private member.
    pub(super) fn person(
        controller: &'a crate::access::AccessController,
        subject: &'a str,
    ) -> Self {
        Self {
            controller,
            view: controller.view_for_subject(Some(subject)),
            principal: Some(subject),
        }
    }

    /// The signed-in person `subject`, who owns the members private to
    /// `private_owner_key`.
    pub(super) fn owner(
        controller: &'a crate::access::AccessController,
        subject: &'a str,
        private_owner_key: &str,
    ) -> Self {
        Self {
            controller,
            view: controller.view_for_caller(Some(subject), Some(private_owner_key)),
            principal: Some(subject),
        }
    }
}

/// Record `identity` as a member the Console knows, private to the person
/// whose key is `private_to` when one is given.
pub(super) fn record_member(
    controller: &crate::access::AccessController,
    identity: &str,
    private_to: Option<&str>,
) {
    controller.record_agent_attributes(crate::access::AgentResourceAttributes {
        identity: identity.to_string(),
        agent_id: None,
        role: Some("worker".to_string()),
        labels: private_to
            .map(|owner| {
                (
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    owner.to_string(),
                )
            })
            .into_iter()
            .collect(),
    });
}

pub(super) fn mob_scope() -> MemoryScope {
    MemoryScope::Mob {
        realm: REALM.to_string(),
        mob: MOB.to_string(),
    }
}

impl ConsoleMemoryFixture {
    pub(super) async fn new(name: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (temp_dir, runtime) = build_empty_console_test_runtime(name).await?;
        let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: name.to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        }));
        let memory_dir = tempfile::tempdir()?;
        let store = Arc::new(SqliteAgentMemoryStore::open(memory_dir.path())?);
        identity_runtime
            .set_agent_memory(Some(AgentMemoryRuntimeInjector::new(
                store.clone(),
                AgentMemoryConfig::default(),
            )))
            .await;
        Ok(Self {
            runtime,
            identity_runtime,
            store,
            _dirs: (temp_dir, memory_dir),
        })
    }

    /// One request on the Console's RPC entry; `mutable` is whether the
    /// caller may change anything there.
    pub(super) async fn rpc(&self, method: &str, params: Value, mutable: bool) -> Value {
        Box::pin(handle_console_runtime_rpc(
            &self.runtime,
            None,
            None,
            None,
            None,
            None,
            Some(self.identity_runtime.clone()),
            None,
            None,
            rpc_request_with_params(method, params),
            mutable,
        ))
        .await
    }

    /// One request as `caller`, on a Console the caller may change things
    /// on: the caller's view decides what it reaches.
    pub(super) async fn rpc_as(
        &self,
        caller: &ConsoleCaller<'_>,
        method: &str,
        params: Value,
    ) -> Value {
        Box::pin(handle_console_runtime_rpc_with_visibility(
            &self.runtime,
            None,
            None,
            None,
            None,
            None,
            Some(self.identity_runtime.clone()),
            None,
            None,
            &crate::console_aggregator::AllowAllConsoleVisibilityPolicy,
            rpc_request_with_params(method, params),
            true,
            false,
            caller.principal,
            Some(caller.controller),
            Some(&caller.view),
            None,
            None,
            None,
            None,
            None,
            &crate::http_console::ConsoleSendGovernance::default(),
            &crate::topology_control::TopologyApplyScope::Anonymous,
        ))
        .await
    }

    /// One read of the memory panel on the Console's RPC entry, over this
    /// fixture's store, by a caller the Console keeps nothing from.
    pub(super) async fn panel_rpc(&self, method: &str, params: Value) -> Value {
        let panel: &dyn crate::memory::capabilities::MemoryPanelStore = &*self.store;
        Box::pin(handle_console_runtime_rpc_with_visibility(
            &self.runtime,
            None,
            None,
            None,
            None,
            None,
            Some(self.identity_runtime.clone()),
            None,
            None,
            &crate::console_aggregator::AllowAllConsoleVisibilityPolicy,
            rpc_request_with_params(method, params),
            true,
            false,
            None,
            None,
            None,
            Some(panel),
            None,
            None,
            None,
            None,
            &crate::http_console::ConsoleSendGovernance::default(),
            &crate::topology_control::TopologyApplyScope::Anonymous,
        ))
        .await
    }

    /// The methods the Console advertises to `caller`.
    pub(super) async fn advertised_to(&self, caller: &ConsoleCaller<'_>) -> Vec<String> {
        self.rpc_as(caller, "meta/capabilities", json!({})).await["result"]["methods"]
            .as_array()
            .expect("methods")
            .iter()
            .filter_map(|method| method.as_str().map(str::to_string))
            .collect()
    }

    /// A record of `scope` that an operator retired.
    pub(super) async fn seed_retired(&self, id: &str, scope: MemoryScope, title: &str, body: &str) {
        self.seed(id, scope.clone(), title, body).await;
        let removed = self
            .store
            .forget_authored(&scope, id, MemoryAuthor::Operator { principal: None })
            .await
            .expect("forget");
        assert!(removed.deleted, "{id}");
    }

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
                    description: String::new(),
                    body: body.to_string(),
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

    pub(super) async fn record(&self, id: &str) -> MemoryRecord {
        self.store
            .records_by_ids(REALM, &[id.to_string()])
            .await
            .expect("read")
            .remove(0)
    }

    /// The author of the record's audit rows of one op kind, oldest first.
    pub(super) fn audit_authors(&self, id: &str, op_kind: &str) -> Vec<Value> {
        let conn = rusqlite::Connection::open(self.store.path_for_realm(REALM)).expect("open");
        let mut stmt = conn
            .prepare(
                "SELECT detail FROM audit WHERE memory_id = ?1 AND op_kind = ?2 \
                 ORDER BY audit_id",
            )
            .expect("prepare");
        stmt.query_map([id, op_kind], |row| row.get::<_, String>(0))
            .expect("query")
            .map(|detail| {
                let detail: Value =
                    serde_json::from_str(&detail.expect("row")).expect("detail is JSON");
                detail["author"].clone()
            })
            .collect()
    }

    pub(super) async fn advertised(&self, mutable: bool) -> Vec<String> {
        self.rpc("meta/capabilities", json!({}), mutable).await["result"]["methods"]
            .as_array()
            .expect("methods")
            .iter()
            .filter_map(|method| method.as_str().map(str::to_string))
            .collect()
    }
}
