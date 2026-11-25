//! Rows for the topology control of a mob a host serves beside its default
//! mob.
//!
//! The fixture is a launch that declares its mob to its mob set over a state
//! directory, as the gateways do, and that sets a topology policy with a
//! state file, as a Rust host does: the mobs the set builds for the host
//! beside the default mob are stored under that directory. Every row holds
//! two mobs whose members have the same names, so an answer of one mob
//! cannot pass for the other's.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_mob::{CloseIfIdleOutcome, MobDefinition, MobId, SpawnMemberSpec};

use super::UnifiedRuntime;
use super::mob_entries_tests::{definition, identity_context, second_mob, set_of, shut_down};
use crate::blob_store::{Base64BlobStoreAdapter, ObjectStoreBlobStore};
use crate::mob_handle_runtime::{MobBootstrapOptions, MobBootstrapSpec};
use crate::runtime::{InMemoryMetadataStore, RuntimeOptions};
use crate::topology_control::{
    TopologyAction, TopologyApplyRequest, TopologyBootstrapConfig, TopologyControlError,
    TopologyControlMode, TopologyControlPolicy, TopologyEdge, TopologyEdgeSnapshot,
    TopologyEndpoint, TopologyMutation, TopologyOperationStatus, TopologySnapshot,
};
use crate::{DiscoverySpec, MetaConfig, MobRuntimeDelivery};

/// The file name every topology state file ends with.
const STATE_FILE_SUFFIX: &str = "topology-control.json";

/// What every topology answer of a mob whose state could not be opened
/// says: the same words whatever the reason was.
const STATE_NOT_OPENED: &str = "the topology state of this mob could not be opened: its topology \
                                is refused until the mob is loaded again";

/// Whether `answer` is the refusal of a mob whose topology state could not
/// be opened, in its one fixed wording.
fn says_only_that_the_state_was_not_opened<T>(answer: &Result<T, TopologyControlError>) -> bool {
    matches!(
        answer,
        Err(TopologyControlError::Persistence(words)) if words == STATE_NOT_OPENED
    )
}

/// A policy that lets an operator edit, one operation at a time.
pub(super) fn editable() -> TopologyControlPolicy {
    TopologyControlPolicy {
        mode: TopologyControlMode::Editable,
        ..TopologyControlPolicy::default()
    }
}

/// A policy with the mode `mode` and the defaults.
fn policy_of(mode: TopologyControlMode) -> TopologyControlPolicy {
    TopologyControlPolicy {
        mode,
        ..TopologyControlPolicy::default()
    }
}

/// Where the mobs a host builds beside its default mob are kept.
#[derive(Clone, Copy)]
pub(super) enum OtherMobs {
    /// Each in a store file under the state directory.
    Stored,
    /// In memory, for the life of the process.
    InMemory,
}

/// What outlives a launch: the state directory and the stores the sessions,
/// the runtime and the jobs are kept in. Each [`TopologyHost::launch`] is a
/// launch of its own over them, as a restart of a host is.
pub(super) struct TopologyHost {
    dir: tempfile::TempDir,
    definition: MobDefinition,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    blobs: Arc<dyn meerkat_core::BlobStore>,
    sessions: Arc<meerkat_store::MemoryStore>,
    jobs: Arc<dyn meerkat::DetachedJobStore>,
    metadata: Arc<InMemoryMetadataStore>,
}

impl TopologyHost {
    pub(super) fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("a state directory"),
            definition: definition("topology-host"),
            runtime_store: Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            blobs: Arc::new(Base64BlobStoreAdapter::new(Arc::new(
                ObjectStoreBlobStore::memory(),
            ))),
            sessions: Arc::new(meerkat_store::MemoryStore::new()),
            jobs: Arc::new(meerkat::MemoryDetachedJobStore::new()),
            metadata: Arc::new(InMemoryMetadataStore::new()),
        }
    }

    /// The state directory of every launch of this host.
    fn state_dir(&self) -> &Path {
        self.dir.path()
    }

    /// The topology state file of the host's default mob.
    fn default_mobs_state_file(&self) -> PathBuf {
        self.dir.path().join(STATE_FILE_SUFFIX)
    }

    /// What a launch over the state directory declares: its mob, declared
    /// to its mob set over a store in a file, with the mobs the set builds
    /// beside it kept as `other_mobs` says.
    fn spec(&self, other_mobs: OtherMobs) -> MobBootstrapSpec {
        let machine = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(
                Arc::clone(&self.runtime_store),
                Arc::clone(&self.blobs),
            )
            .expect("the runtime machine"),
        );
        let mut builder = FactoryAgentBuilder::new(
            AgentFactory::new(self.dir.path())
                .builtins(false)
                .comms(true),
            Config::default(),
        );
        builder.default_detached_job_store = Some(Arc::clone(&self.jobs));
        let sessions: Arc<meerkat_store::MemoryStore> = Arc::clone(&self.sessions);
        let service = Arc::new(meerkat_session::PersistentSessionService::new(
            builder,
            8,
            sessions,
            Arc::clone(&self.runtime_store),
            Arc::clone(&self.blobs),
        ));
        let child_mob_storage = match other_mobs {
            OtherMobs::Stored => crate::ChildMobStorage::Durable(
                crate::MetaStorageLayout::with_injected_roots(self.dir.path().to_path_buf(), None),
            ),
            OtherMobs::InMemory => crate::ChildMobStorage::ProcessBound,
        };
        MobBootstrapSpec::declared(
            self.definition.clone(),
            crate::mob_composition_manifest::declared_mob_store(
                crate::MetaStorageLayout::with_injected_roots(self.dir.path().to_path_buf(), None)
                    .event_log_db(),
            ),
            service,
        )
        .with_session_runtime_adapter(machine)
        .expect("the launch adopts the runtime machine")
        .with_runtime_delivery(MobRuntimeDelivery::new(
            Arc::clone(&self.runtime_store),
            Arc::clone(&self.jobs),
        ))
        .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
        .expect("the agent mob tools")
        .with_child_mob_storage(child_mob_storage)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        })
    }

    /// The modules of a launch: none, under the mob's own namespace.
    fn modules(&self) -> MetaConfig {
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: self.definition.id.to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        }
    }

    /// A launch over the state directory with the topology policy `policy`
    /// and the default mob's state file: its mob is declared to its mob set
    /// over a store in a file, and the mobs the set builds beside it are
    /// kept as `other_mobs` says. Its requests resolve the host's other
    /// mobs.
    pub(super) async fn launch(
        &self,
        policy: TopologyControlPolicy,
        other_mobs: OtherMobs,
    ) -> UnifiedRuntime {
        let metadata: Arc<InMemoryMetadataStore> = Arc::clone(&self.metadata);
        Box::pin(UnifiedRuntime::bootstrap_with_options_and_topology(
            self.spec(other_mobs),
            self.modules(),
            Vec::new(),
            Duration::from_secs(10),
            RuntimeOptions::default(),
            metadata,
            TopologyBootstrapConfig {
                policy,
                state_path: Some(self.default_mobs_state_file()),
            },
        ))
        .await
        .expect("a runtime over a declared store with a topology policy")
    }

    /// [`Self::launch`] by the runtime builder, as a Rust host that composes
    /// its runtime with it does: the same declared mob, the state directory
    /// as the builder's persistent state (it keeps the default mob's
    /// topology state there, in [`Self::default_mobs_state_file`]) and the
    /// topology policy `policy` set on the builder.
    async fn launch_with_the_builder(
        &self,
        policy: TopologyControlPolicy,
        other_mobs: OtherMobs,
    ) -> UnifiedRuntime {
        Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(self.spec(other_mobs))
                .persistent_state(self.dir.path())
                .module_config(self.modules())
                .timeout(Duration::from_secs(10))
                .roster_provider(Arc::new(crate::identity_first::MutableRosterProvider::new(
                    Vec::new(),
                )))
                .topology_control(policy)
                .expect("a topology policy on the builder")
                .build(),
        )
        .await
        .expect("a runtime built with the builder over a declared store")
    }

    /// The topology state files kept for the mob `mob_id` beside its store
    /// under the state directory, sorted: every file there whose name holds
    /// the mob's id and ends with [`STATE_FILE_SUFFIX`].
    pub(super) fn state_files_of(&self, mob_id: &MobId) -> Vec<PathBuf> {
        files_under(
            &crate::child_mob_storage::child_mob_root(self.dir.path()),
            |name| name.contains(mob_id.as_str()) && name.ends_with(STATE_FILE_SUFFIX),
        )
    }

    /// Every file under the directory of the other mobs' stores whose name
    /// holds the id of `mob_id` and names topology state: a state file or
    /// its lock.
    fn state_files_and_locks_of(&self, mob_id: &MobId) -> Vec<PathBuf> {
        files_under(
            &crate::child_mob_storage::child_mob_root(self.dir.path()),
            |name| name.contains(mob_id.as_str()) && name.contains(STATE_FILE_SUFFIX),
        )
    }
}

/// The files directly under `dir` whose name `wanted` selects, sorted;
/// none when the directory does not exist.
fn files_under(dir: &Path, wanted: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(&wanted)
        })
        .collect();
    files.sort();
    files
}

/// The mob that the state file `path` says it belongs to.
fn authority_of(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let state: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    state["authority"].as_str().map(str::to_string)
}

/// Seat the members `alice` and `bob` in the host's default mob.
async fn seat_in_the_default_mob(host: &UnifiedRuntime) {
    for member in ["alice", "bob"] {
        let seated = Box::pin(host.spawn(SpawnMemberSpec::new("worker", member))).await;
        assert!(
            seated.is_ok(),
            "the default mob seats {member}: {:?}",
            seated.as_ref().err()
        );
    }
}

/// Seat the members `alice` and `bob` in the resolved mob `mob`.
async fn seat_in(mob: &super::mob_entry::MobEntryRef<'_>) {
    for member in ["alice", "bob"] {
        let seated = Box::pin(mob.spawn(SpawnMemberSpec::new("worker", member))).await;
        assert!(
            seated.is_ok(),
            "the mob {} seats {member}: {:?}",
            mob.mob_id(),
            seated.as_ref().err()
        );
    }
}

/// The edge between `alice` and `bob` of the mob a request is for.
fn alice_bob() -> TopologyEdge {
    TopologyEdge::new(
        TopologyEndpoint::local("alice"),
        TopologyEndpoint::local("bob"),
    )
    .expect("an edge between two members")
}

/// A request that connects `alice` and `bob` at `expected_revision` under
/// the idempotency key `key`.
pub(super) fn connect_alice_bob(expected_revision: u64, key: &str) -> TopologyApplyRequest {
    TopologyApplyRequest {
        expected_revision,
        idempotency_key: key.to_string(),
        operations: vec![TopologyMutation {
            action: TopologyAction::Connect,
            edge: alice_bob(),
        }],
        reason: None,
        risk_tier: None,
    }
}

/// The edge between `alice` and `bob` in `snapshot`, when it holds one.
pub(super) fn alice_bob_in(snapshot: &TopologySnapshot) -> Option<&TopologyEdgeSnapshot> {
    let endpoint = |identity: &str| TopologyEndpoint {
        authority: Some(snapshot.authority.clone()),
        identity: identity.to_string(),
    };
    let edge = TopologyEdge::new(endpoint("alice"), endpoint("bob")).ok()?;
    snapshot
        .edges
        .iter()
        .find(|candidate| candidate.edge == edge)
}

/// Whether an operator connected `alice` and `bob` in `snapshot`'s mob and
/// the two are wired.
fn connected_by_an_operator(snapshot: &TopologySnapshot) -> bool {
    alice_bob_in(snapshot).is_some_and(|edge| edge.operator_added && edge.actual)
}

/// A mob a host serves beside its default mob has a topology controller of
/// its own that reads the host's policy and belongs to that mob: it is not
/// the default mob's controller, it answers the policy the launch set, and
/// it refuses to be bound to any other mob.
#[tokio::test]
async fn a_second_mobs_topology_controller_is_bound_to_its_mob_and_reads_the_hosts_policy() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");

    assert!(
        !mob.topology_controller()
            .is_the_same_controller_in_tests(host.topology_controller()),
        "the second mob has a controller of its own"
    );
    assert_eq!(
        mob.topology_controller().policy(),
        editable(),
        "the second mob's controller reads the policy the launch set"
    );
    assert_eq!(
        mob.topology_runtime_handle().policy(),
        host.topology_runtime_handle().policy(),
        "and its seam answers the default mob's policy"
    );
    let bound_elsewhere = mob
        .topology_controller()
        .bind_authority("a-mob-of-another-name")
        .await;
    assert!(
        matches!(bound_elsewhere, Err(TopologyControlError::Persistence(_))),
        "the second mob's controller is bound to its mob and refuses another: \
         {bound_elsewhere:?}"
    );
    let bound_again = mob
        .topology_controller()
        .bind_authority(second_id.to_string())
        .await;
    assert!(
        bound_again.is_ok(),
        "and takes its own mob again: {bound_again:?}"
    );
    drop(mob);
    shut_down(&host, &[&second_id]).await;
}

/// A host has one topology policy. A change of it at run time reaches every
/// mob of the host at once, the mobs beside the default mob included, in
/// both directions.
#[tokio::test]
async fn a_change_of_the_hosts_topology_policy_reaches_every_mob() {
    let fixture = TopologyHost::new();
    let host = fixture
        .launch(policy_of(TopologyControlMode::ReadOnly), OtherMobs::Stored)
        .await;
    let second_id = second_mob(&host, definition("second")).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    assert_eq!(
        mob.topology_controller().policy().mode,
        TopologyControlMode::ReadOnly,
        "the second mob reads the mode the launch set"
    );

    host.set_topology_control_policy(editable())
        .expect("the host changes its policy");
    assert_eq!(
        host.topology_controller().policy().mode,
        TopologyControlMode::Editable
    );
    assert_eq!(
        mob.topology_controller().policy().mode,
        TopologyControlMode::Editable,
        "the change reached the second mob"
    );
    let resolved_after = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob resolves again");
    assert_eq!(
        resolved_after.topology_runtime_handle().policy().mode,
        TopologyControlMode::Editable,
        "and a seam taken after it"
    );

    host.set_topology_control_policy(TopologyControlPolicy::default())
        .expect("the host disables topology control");
    assert_eq!(
        mob.topology_controller().policy().mode,
        TopologyControlMode::Disabled,
        "a change back reached the second mob too"
    );
    drop(resolved_after);
    drop(mob);
    shut_down(&host, &[&second_id]).await;
}

/// An apply in a second mob changes that mob alone. Both mobs hold members
/// of the same names; the operator connects them in the second mob. The
/// second mob's revision moves and its two members are wired; the default
/// mob's revision, its wiring and the bytes of its state file are what they
/// were.
#[tokio::test]
async fn topology_apply_in_a_second_mob_is_isolated() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in_the_default_mob(&host).await;
    seat_in(&mob).await;
    let default_before = host
        .topology_runtime_handle()
        .query()
        .await
        .expect("the default mob's topology");
    let default_file_before =
        std::fs::read(fixture.default_mobs_state_file()).expect("the default mob's state file");

    let applied = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "connect-in-second"), "operator")
        .await;
    assert!(
        applied
            .as_ref()
            .is_ok_and(|receipt| receipt.status == TopologyOperationStatus::Applied
                && receipt.base_revision == 0
                && receipt.revision == 1),
        "an operator connects two members of the second mob: {applied:?}"
    );
    assert_eq!(
        std::fs::read(fixture.default_mobs_state_file()).expect("the default mob's state file"),
        default_file_before,
        "the default mob's state file was not written"
    );
    let second_after = mob
        .topology_runtime_handle()
        .query()
        .await
        .expect("the second mob's topology");
    assert_eq!(second_after.authority, second_id.to_string());
    assert_eq!(second_after.revision, 1);
    assert!(
        connected_by_an_operator(&second_after),
        "the second mob's members are wired by the operator: {:?}",
        second_after.edges
    );
    let default_after = host
        .topology_runtime_handle()
        .query()
        .await
        .expect("the default mob's topology");
    assert_eq!(default_after.authority, host.mob_id());
    assert_eq!(
        default_after.revision, default_before.revision,
        "the default mob's revision did not move"
    );
    assert!(
        alice_bob_in(&default_after).is_none_or(|edge| !edge.actual && !edge.operator_added),
        "the default mob's members of the same names are not wired: {:?}",
        default_after.edges
    );
    assert_eq!(host.topology_controller().revision().await, 0);
    drop(mob);
    shut_down(&host, &[&second_id]).await;
}

/// Receipts and idempotency keys are a mob's own: the same key used for the
/// same request in two mobs of one host names two operations, each applied
/// in its mob at that mob's revision, and a replay in one mob answers that
/// mob's receipt.
#[tokio::test]
async fn one_idempotency_key_in_two_mobs_is_two_operations() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in_the_default_mob(&host).await;
    seat_in(&mob).await;

    let in_the_default = host
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "one-key"), "operator")
        .await
        .expect("the apply in the default mob");
    assert_eq!(in_the_default.status, TopologyOperationStatus::Applied);
    let in_the_second = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "one-key"), "operator")
        .await;
    assert!(
        in_the_second
            .as_ref()
            .is_ok_and(|receipt| receipt.status == TopologyOperationStatus::Applied
                && receipt.base_revision == 0
                && receipt.revision == 1),
        "the same key in the second mob is an operation of its own, applied at that mob's \
         revision: {in_the_second:?}"
    );
    if let Ok(in_the_second) = in_the_second {
        assert_ne!(
            in_the_second.operation_id, in_the_default.operation_id,
            "the two mobs' operations have two ids"
        );
        let replayed = mob
            .topology_runtime_handle()
            .apply(connect_alice_bob(0, "one-key"), "operator")
            .await
            .expect("a replay in the second mob");
        assert_eq!(
            replayed.operation_id, in_the_second.operation_id,
            "a replay in the second mob answers the second mob's receipt"
        );
        assert!(
            mob.topology_controller()
                .operation(&in_the_default.operation_id)
                .await
                .is_err(),
            "the second mob holds no receipt of the default mob's operation"
        );
        assert!(
            host.topology_controller()
                .operation(&in_the_second.operation_id)
                .await
                .is_err(),
            "and the default mob none of the second mob's"
        );
    }
    drop(mob);
    shut_down(&host, &[&second_id]).await;
}

/// The topology state of a mob beside the default mob is one file beside
/// that mob's store, named after the store, and it says which mob it
/// belongs to. The default mob's file keeps its place.
#[tokio::test]
async fn a_second_mobs_topology_state_is_beside_its_store_and_names_its_mob() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in(&mob).await;
    let applied = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "kept"), "operator")
        .await;
    assert!(
        applied.is_ok(),
        "an apply in the second mob: {:?}",
        applied.as_ref().err()
    );

    let stores: Vec<PathBuf> = crate::child_mob_storage::child_mob_databases(fixture.state_dir())
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(second_id.as_str()))
        })
        .collect();
    assert_eq!(stores.len(), 1, "the second mob's store file: {stores:?}");
    let store_name = stores[0]
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the store file's name")
        .to_string();
    let files = fixture.state_files_of(&second_id);
    assert_eq!(
        files.len(),
        1,
        "the second mob keeps one topology state file beside its store: {files:?}"
    );
    if let Some(file) = files.first() {
        assert_eq!(file.parent(), stores[0].parent(), "beside the store");
        assert!(
            file.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&format!("{store_name}."))),
            "named after the store file: {file:?}"
        );
        assert_eq!(
            authority_of(file),
            Some(second_id.to_string()),
            "the file says it belongs to the second mob"
        );
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file).expect("the state file reads"))
                .expect("the state file is JSON");
        assert_eq!(state["revision"], serde_json::json!(1));
    }
    assert_eq!(
        authority_of(&fixture.default_mobs_state_file()),
        Some(host.mob_id()),
        "the default mob's file is where the launch said and belongs to the default mob"
    );
    drop(mob);
    shut_down(&host, &[&second_id]).await;
}

/// A state file that says it belongs to another mob is not read as the
/// mob's whose place it is in. After a restart over a state directory in
/// which the second mob's file names another mob, the second mob is still
/// served, each read and each change of its topology is refused, and the
/// file still names the other mob. The refusal says nothing of the reason:
/// neither the other mob's name nor the place of the file reaches a caller,
/// on the seam or on the wire.
#[tokio::test]
async fn topology_state_in_another_mobs_place_is_refused() {
    let fixture = TopologyHost::new();
    let first = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&first, definition("second")).await;
    let mob = first
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in(&mob).await;
    drop(mob);
    let files = fixture.state_files_of(&second_id);
    assert_eq!(
        files.len(),
        1,
        "the second mob keeps one topology state file: {files:?}"
    );
    shut_down(&first, &[]).await;
    drop(first);
    let Some(file) = files.first() else {
        return;
    };
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(file).expect("the state file reads"))
            .expect("the state file is JSON");
    state["authority"] = serde_json::json!("a-mob-of-another-name");
    let misplaced = serde_json::to_vec_pretty(&state).expect("the state file's bytes");
    std::fs::write(file, &misplaced).expect("the state file is replaced");

    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob is served all the same: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        let read = mob.topology_runtime_handle().query().await;
        assert!(
            says_only_that_the_state_was_not_opened(&read),
            "a read of the second mob's topology is refused, and the refusal says nothing of \
             the reason: {read:?}"
        );
        let changed = mob
            .topology_runtime_handle()
            .apply(connect_alice_bob(0, "refused"), "operator")
            .await;
        assert!(
            says_only_that_the_state_was_not_opened(&changed),
            "a change of the second mob's topology is refused, and the refusal says nothing \
             of the reason: {changed:?}"
        );
    }
    let on_the_wire = stdio(
        &host,
        "meta/topology/query",
        serde_json::json!({"mob_id": second_id.as_str()}),
    )
    .await;
    assert_eq!(
        on_the_wire["error"]["data"]["kind"], "topology_persistence_failed",
        "the read of the second mob on the wire: {on_the_wire}"
    );
    let answered = on_the_wire.to_string();
    let state_dir = fixture.state_dir().display().to_string();
    assert!(
        !answered.contains("a-mob-of-another-name")
            && !answered.contains(&state_dir)
            && !answered.contains(STATE_FILE_SUFFIX),
        "the answer on the wire names neither the other mob nor the file: {answered}"
    );
    assert_eq!(
        authority_of(file).as_deref(),
        Some("a-mob-of-another-name"),
        "the file still says it belongs to the other mob"
    );
    shut_down(&host, &[]).await;
}

/// An apply keeps its mob loaded: while an apply in a second mob is open,
/// that mob's idle close answers busy with a foreground owner, and once the
/// apply's receipt is closed no foreground owner is left.
#[tokio::test]
async fn an_apply_keeps_its_mob_loaded() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in(&mob).await;
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the set serves the second mob");

    // The row holds the controller's admission, so the apply stays open.
    let controller = mob.topology_controller().clone();
    let admission = controller.mutation_guard().await;
    let seam = mob.topology_runtime_handle();
    let apply = seam.apply(connect_alice_bob(0, "held-open"), "operator");
    tokio::pin!(apply);
    let waits = tokio::time::timeout(Duration::from_millis(300), &mut apply).await;
    assert!(
        waits.is_err(),
        "the apply waits for the admission the row holds: {:?}",
        waits.ok()
    );
    let while_open = second.close_if_idle().await;
    assert!(
        matches!(
            while_open,
            Ok(CloseIfIdleOutcome::Busy(busy)) if busy.foreground_owners >= 1
        ),
        "the idle close of a mob with an open apply answers busy with a foreground owner: \
         {while_open:?}"
    );

    drop(admission);
    let applied = apply.await;
    assert!(
        applied
            .as_ref()
            .is_ok_and(|receipt| receipt.status == TopologyOperationStatus::Applied),
        "the apply ends once the admission is free: {applied:?}"
    );
    let after = second.close_if_idle().await;
    assert!(
        match after {
            Ok(CloseIfIdleOutcome::Busy(busy)) => busy.foreground_owners == 0,
            Ok(_) => true,
            Err(_) => false,
        },
        "no foreground owner is left once the receipt is closed: {after:?}"
    );
    drop(mob);
    shut_down(&host, &[&second_id]).await;
}

/// The identity runtime a host attaches to a second mob composes that mob's
/// own operator intent: it is handed the controller of the mob's entry, not
/// none and not the default mob's.
#[tokio::test]
async fn a_second_mobs_identity_runtime_composes_its_own_topology_intent() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    let context = identity_context(mob.mob_runtime(), "topology-second");
    let attached = mob
        .attach_identity_first_context(Arc::clone(&context))
        .await;
    assert!(
        attached.is_ok(),
        "a host attaches an identity context to its second mob: {:?}",
        attached.as_ref().err()
    );

    let composed_with = context.runtime.topology_controller_in_tests();
    assert!(
        composed_with.as_ref().is_some_and(
            |controller| controller.is_the_same_controller_in_tests(mob.topology_controller())
        ),
        "the second mob's identity runtime composes the intent of its mob's own controller"
    );
    assert!(
        composed_with
            .as_ref()
            .is_none_or(|controller| !controller
                .is_the_same_controller_in_tests(host.topology_controller())),
        "and not the default mob's"
    );
    drop(mob);
    shut_down(&host, &[&second_id]).await;
}

/// The topology state of a destroyed mob goes with it: after the mob set
/// destroyed the mob and the host released its topology state, neither the
/// state file nor its lock is left beside where the store was, and a mob
/// created under the same id starts at revision 0 with no operator intent.
#[tokio::test]
async fn the_topology_state_of_a_destroyed_mob_is_removed_and_a_new_mob_of_its_id_starts_at_revision_zero()
 {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in(&mob).await;
    let applied = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "before-the-destroy"), "operator")
        .await;
    assert!(
        applied.is_ok(),
        "an apply in the second mob: {:?}",
        applied.as_ref().err()
    );
    // What a request that took the mob's seam before the destroy still
    // holds of it: a clone of the mob's controller. (A held seam also holds
    // the mob's handle, and the mob set's destroy waits for every handle.)
    let held_from_before = mob.topology_controller().clone();
    drop(mob);
    assert!(
        !fixture.state_files_and_locks_of(&second_id).is_empty(),
        "the second mob keeps topology state before its destroy"
    );
    // While the mob set hosts the mob, its state is kept: a removal asked
    // too early is refused and removes nothing.
    let kept = fixture.state_files_and_locks_of(&second_id);
    let too_early = host
        .mob_resolver()
        .release_topology_state_of_a_destroyed_mob(&second_id)
        .await;
    assert!(
        too_early.is_err(),
        "a removal of the topology state of a mob that is hosted is refused: {too_early:?}"
    );
    assert_eq!(
        fixture.state_files_and_locks_of(&second_id),
        kept,
        "and removes nothing"
    );

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the mob set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    let released = host
        .mob_resolver()
        .release_topology_state_of_a_destroyed_mob(&second_id)
        .await;
    assert!(
        released.is_ok(),
        "the host releases the topology state of the destroyed mob: {released:?}"
    );
    assert_eq!(
        fixture.state_files_and_locks_of(&second_id),
        Vec::<PathBuf>::new(),
        "neither the state file nor its lock is left"
    );
    let released_again = host
        .mob_resolver()
        .release_topology_state_of_a_destroyed_mob(&second_id)
        .await;
    assert!(
        released_again.is_ok(),
        "a second release finds nothing and is no failure: {released_again:?}"
    );
    // The controller held from before the destroy reads and writes nothing
    // any more and brings no file back.
    let late_read = held_from_before.operation("no-such-operation").await;
    assert!(
        matches!(late_read, Err(TopologyControlError::Persistence(_))),
        "a read through the controller of the destroyed mob is refused: {late_read:?}"
    );
    let late_write = held_from_before.bind_authority(second_id.to_string()).await;
    assert!(
        matches!(late_write, Err(TopologyControlError::Persistence(_))),
        "a write through the controller of the destroyed mob is refused: {late_write:?}"
    );
    assert_eq!(
        fixture.state_files_and_locks_of(&second_id),
        Vec::<PathBuf>::new(),
        "and makes no file"
    );
    drop(held_from_before);

    let again = second_mob(&host, second_definition).await;
    assert_eq!(again, second_id, "a new mob under the freed id");
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the new mob resolves");
    assert_eq!(
        mob.topology_controller().revision().await,
        0,
        "the new mob starts at revision 0"
    );
    seat_in(&mob).await;
    let fresh = mob
        .topology_runtime_handle()
        .query()
        .await
        .expect("the new mob's topology");
    assert!(
        alice_bob_in(&fresh).is_none_or(|edge| !edge.operator_added),
        "and holds no operator intent of the destroyed mob: {:?}",
        fresh.edges
    );
    drop(mob);
    shut_down(&host, &[&second_id]).await;
}

/// A destroy through the host's method takes the destroyed mob's topology
/// state with it: once `meta/mobs/destroy` answered, neither the state file
/// nor its lock is left beside where the store was. The caller asks for no
/// release of its own.
#[tokio::test]
async fn a_destroy_through_the_hosts_method_leaves_no_topology_state_of_the_mob() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in(&mob).await;
    let applied = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "before-the-destroy"), "operator")
        .await;
    assert!(
        applied.is_ok(),
        "an apply in the second mob: {:?}",
        applied.as_ref().err()
    );
    drop(mob);
    assert!(
        !fixture.state_files_and_locks_of(&second_id).is_empty(),
        "the second mob keeps topology state before its destroy"
    );

    let destroyed = crate::rpc::mob_hosting_tests::result(
        &host,
        "meta/mobs/destroy",
        serde_json::json!({"mob_id": second_id.to_string()}),
    )
    .await;
    assert_eq!(
        destroyed["mob_id"],
        serde_json::json!(second_id.to_string())
    );
    assert_eq!(
        fixture.state_files_and_locks_of(&second_id),
        Vec::<PathBuf>::new(),
        "neither the state file nor its lock is left once the host's destroy answered"
    );
    shut_down(&host, &[]).await;
}

/// A destroy through the host's method that could not finish releases
/// nothing of the mob's topology state, and the repeat that finishes it
/// does. Here the mob is destroyed while a reader holds its store open, so
/// the storage cleanup is left owed and the method answers the destroy's
/// own "incomplete": the state file and its lock are as they were. Once
/// the reader is gone the repeated destroy answers the destroy's report,
/// and neither file is left.
#[tokio::test]
async fn a_destroy_that_could_not_finish_keeps_the_topology_state_until_its_repeat_finishes_it() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in(&mob).await;
    let applied = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "before-the-destroy"), "operator")
        .await;
    assert!(
        applied.is_ok(),
        "an apply in the second mob: {:?}",
        applied.as_ref().err()
    );
    drop(mob);
    let kept = fixture.state_files_and_locks_of(&second_id);
    assert!(
        !kept.is_empty(),
        "the second mob keeps topology state before its destroy"
    );
    let store = set_of(&host)
        .persistent_mob_store_path(&second_id)
        .expect("the second mob is kept in a store file");
    let reader = meerkat_mob::MobStorage::persistent(&store).expect("a reader of the mob's store");
    let names_the_mob = serde_json::json!({"mob_id": second_id.to_string()});

    let incomplete =
        crate::rpc::mob_hosting_tests::refusal(&host, "meta/mobs/destroy", names_the_mob.clone())
            .await;
    assert_eq!(
        incomplete["code"],
        serde_json::json!(-32603),
        "{incomplete}"
    );
    assert!(
        incomplete["message"]
            .as_str()
            .is_some_and(|message| message.contains("storage cleanup outstanding")),
        "the destroy answers what it still owes: {incomplete}"
    );
    assert_eq!(
        fixture.state_files_and_locks_of(&second_id),
        kept,
        "a destroy that did not finish releases nothing"
    );

    drop(reader);
    let repeated =
        crate::rpc::mob_hosting_tests::result(&host, "meta/mobs/destroy", names_the_mob).await;
    assert!(
        repeated["destroy_report"].is_object(),
        "the repeat answers the destroy's report: {repeated}"
    );
    assert_eq!(
        fixture.state_files_and_locks_of(&second_id),
        Vec::<PathBuf>::new(),
        "neither the state file nor its lock is left once the repeat finished the destroy"
    );
    shut_down(&host, &[]).await;
}

/// A release that is refused does not change the destroy's answer, and it
/// removes nothing. Here a mob is created under the id after the mob set
/// finished the destroy and before the host asked for the release of the
/// destroyed mob's topology state: the release is refused, since the mob
/// set hosts a mob under the id again. The destroy answers its report, no
/// topology state file is removed, and the new mob keeps the edit it took
/// meanwhile.
#[tokio::test]
async fn a_destroy_whose_topology_release_is_refused_answers_its_report_and_removes_nothing() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in(&mob).await;
    let applied = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "before-the-destroy"), "operator")
        .await;
    assert!(
        applied.is_ok(),
        "an apply in the second mob: {:?}",
        applied.as_ref().err()
    );
    drop(mob);

    let hold = crate::rpc::mob_hosting::destroy_seam::hold_the_next("release", second_id.as_str());
    let late = crate::rpc::mob_hosting_tests::result(
        &host,
        "meta/mobs/destroy",
        serde_json::json!({"mob_id": second_id.to_string()}),
    );
    tokio::pin!(late);
    crate::rpc::mob_hosting_tests::until_held(late.as_mut(), &hold).await;

    // The mob set finished the destroy: the id is free, and a new mob takes
    // it before the release is asked.
    let again = second_mob(&host, second_definition).await;
    assert_eq!(again, second_id, "a new mob under the freed id");
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the new mob resolves");
    seat_in(&mob).await;
    let applied = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "after-the-destroy"), "operator")
        .await;
    assert!(
        applied.is_ok(),
        "an apply in the new mob: {:?}",
        applied.as_ref().err()
    );
    let revision = mob.topology_controller().revision().await;
    let kept = fixture.state_files_and_locks_of(&second_id);

    hold.release.notify_one();
    let destroyed = tokio::time::timeout(Duration::from_mins(1), late)
        .await
        .expect("the destroy answers once its release was asked");
    assert_eq!(
        destroyed["mob_id"],
        serde_json::json!(second_id.to_string())
    );
    assert!(
        destroyed["destroy_report"].is_object(),
        "the destroy answers its report though its release was refused: {destroyed}"
    );
    assert_eq!(
        fixture.state_files_and_locks_of(&second_id),
        kept,
        "a refused release removes nothing"
    );
    assert_eq!(
        mob.topology_controller().revision().await,
        revision,
        "and the new mob keeps the edit it took"
    );
    drop(mob);
    shut_down(&host, &[&second_id]).await;
}

/// A mob with no store file has no durable place for operator intent, so
/// it is not edited: under a host whose policy is editable, a mob kept in
/// memory reads the mode `read_only`, an apply in it is refused as on a
/// read-only host, a plan is answered, and no topology state file is made
/// for it.
#[tokio::test]
async fn a_mob_without_a_store_file_keeps_no_topology_state_and_refuses_an_apply() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::InMemory).await;
    let second_id = second_mob(&host, definition("second")).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in(&mob).await;

    assert_eq!(
        host.topology_controller().policy().mode,
        TopologyControlMode::Editable,
        "the host's policy is editable"
    );
    assert_eq!(
        mob.topology_runtime_handle().policy().mode,
        TopologyControlMode::ReadOnly,
        "a mob kept in memory reads read-only"
    );
    let planned = mob
        .topology_runtime_handle()
        .plan(crate::topology_control::TopologyPlanRequest {
            expected_revision: 0,
            operations: vec![TopologyMutation {
                action: TopologyAction::Connect,
                edge: alice_bob(),
            }],
        })
        .await;
    assert!(
        planned.is_ok(),
        "a plan in it is answered: {:?}",
        planned.as_ref().err()
    );
    let applied = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "no-durable-place"), "operator")
        .await;
    assert!(
        matches!(applied, Err(TopologyControlError::ReadOnly)),
        "an apply in it is refused as read-only: {applied:?}"
    );
    assert_eq!(
        fixture.state_files_and_locks_of(&second_id),
        Vec::<PathBuf>::new(),
        "no topology state is kept for it"
    );
    drop(mob);
    shut_down(&host, &[&second_id]).await;
}

/// The operator intent of a second mob outlives a restart of its host: a
/// launch over the same state directory reads the second mob's revision and
/// its operator's connection back from that mob's own file.
#[tokio::test]
async fn a_second_mobs_topology_survives_a_restart() {
    let fixture = TopologyHost::new();
    let first = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&first, definition("second")).await;
    let mob = first
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in(&mob).await;
    let applied = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "before-the-restart"), "operator")
        .await;
    assert!(
        applied.is_ok(),
        "an apply in the second mob: {:?}",
        applied.as_ref().err()
    );
    drop(mob);
    shut_down(&first, &[]).await;
    drop(first);

    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob resolves after the restart: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        assert_eq!(
            mob.topology_controller().revision().await,
            1,
            "the second mob's revision is read back"
        );
        let after = mob
            .topology_runtime_handle()
            .query()
            .await
            .expect("the second mob's topology after the restart");
        assert!(
            alice_bob_in(&after).is_some_and(|edge| edge.operator_added),
            "the operator's connection is read back: {:?}",
            after.edges
        );
    }
    assert_eq!(
        host.topology_controller().revision().await,
        0,
        "the default mob's revision is its own"
    );
    shut_down(&host, &[]).await;
}

/// The answer of `host`'s stdio dispatch to `method` with `params`.
pub(super) async fn stdio(
    host: &UnifiedRuntime,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
    .to_string();
    let answer =
        crate::rpc::handle_unified_rpc_json(host, &request, Duration::from_secs(10), None, None);
    tokio::time::timeout(Duration::from_secs(30), answer)
        .await
        .ok()
        .and_then(|answer| serde_json::from_str(&answer).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// The stdio topology methods tell two mobs apart. On a host whose two
/// mobs hold members of the same names, an apply that names the second mob
/// connects that mob's members at that mob's revision; after it the query,
/// the plan, the operation read and the audit read each answer for the mob
/// their request names, a request that names no mob or the default mob by
/// its id reads the default mob as it was, and the capabilities of a
/// request that names the second mob report the host's policy.
#[tokio::test]
async fn the_stdio_topology_methods_tell_two_mobs_apart() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    let default_id = host.mob_handle().mob_id().clone();
    {
        let mob = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        seat_in_the_default_mob(&host).await;
        seat_in(&mob).await;
    }
    let named = |mob_id: Option<&MobId>, mut params: serde_json::Value| {
        if let Some(mob_id) = mob_id {
            params["mob_id"] = serde_json::json!(mob_id.as_str());
        }
        params
    };
    let operations = |action: &str| {
        serde_json::json!([{
            "action": action,
            "edge": {"a": {"identity": "alice"}, "b": {"identity": "bob"}},
        }])
    };
    let default_or_none = [None, Some(&default_id)];

    let capabilities = stdio(
        &host,
        "meta/capabilities",
        named(Some(&second_id), serde_json::json!({})),
    )
    .await;
    assert_eq!(
        capabilities["result"]["topology_control"]["mode"],
        serde_json::json!("editable"),
        "the capabilities of a request that names the second mob report the host's policy: \
         {capabilities}"
    );
    let applied = stdio(
        &host,
        "meta/topology/apply",
        named(
            Some(&second_id),
            serde_json::json!({
                "expected_revision": 0,
                "idempotency_key": "told-apart",
                "operations": operations("connect"),
            }),
        ),
    )
    .await;
    assert_eq!(
        applied["result"]["status"],
        serde_json::json!("applied"),
        "an apply that names the second mob is applied there: {applied}"
    );
    assert_eq!(applied["result"]["revision"], serde_json::json!(1));
    let operation_id = applied["result"]["operation_id"]
        .as_str()
        .expect("the operation's id")
        .to_string();

    let of_the_second = stdio(
        &host,
        "meta/topology/query",
        named(Some(&second_id), serde_json::json!({})),
    )
    .await;
    assert_eq!(
        of_the_second["result"]["authority"],
        serde_json::json!(second_id.as_str())
    );
    assert_eq!(of_the_second["result"]["revision"], serde_json::json!(1));
    let planned = stdio(
        &host,
        "meta/topology/plan",
        named(
            Some(&second_id),
            serde_json::json!({"expected_revision": 1, "operations": operations("disconnect")}),
        ),
    )
    .await;
    assert_eq!(
        planned["result"]["base_revision"],
        serde_json::json!(1),
        "a plan that names the second mob is made at that mob's revision: {planned}"
    );
    let read = stdio(
        &host,
        "meta/topology/operation/get",
        named(
            Some(&second_id),
            serde_json::json!({"operation_id": operation_id}),
        ),
    )
    .await;
    assert_eq!(
        read["result"]["operation_id"],
        serde_json::json!(operation_id),
        "the operation read that names the second mob finds its operation: {read}"
    );
    let names_the_operation = |page: &serde_json::Value| {
        page["result"]["records"].as_array().is_some_and(|records| {
            records
                .iter()
                .any(|record| record["operation_id"] == serde_json::json!(operation_id))
        })
    };
    let audited = stdio(
        &host,
        "meta/topology/audit/query",
        named(Some(&second_id), serde_json::json!({})),
    )
    .await;
    assert!(
        names_the_operation(&audited),
        "the audit read that names the second mob holds its operation: {audited}"
    );

    for mob_id in default_or_none {
        let of_the_default = stdio(
            &host,
            "meta/topology/query",
            named(mob_id, serde_json::json!({})),
        )
        .await;
        assert_eq!(
            of_the_default["result"]["authority"],
            serde_json::json!(default_id.as_str()),
            "a query that names {mob_id:?} reads the default mob: {of_the_default}"
        );
        assert_eq!(of_the_default["result"]["revision"], serde_json::json!(0));
        let planned = stdio(
            &host,
            "meta/topology/plan",
            named(
                mob_id,
                serde_json::json!({"expected_revision": 1, "operations": operations("connect")}),
            ),
        )
        .await;
        assert!(
            !planned["error"].is_null(),
            "a plan that names {mob_id:?} is not made at the second mob's revision: {planned}"
        );
        let read = stdio(
            &host,
            "meta/topology/operation/get",
            named(mob_id, serde_json::json!({"operation_id": operation_id})),
        )
        .await;
        assert!(
            !read["error"].is_null(),
            "the operation read that names {mob_id:?} does not find the second mob's \
             operation: {read}"
        );
        let audited = stdio(
            &host,
            "meta/topology/audit/query",
            named(mob_id, serde_json::json!({})),
        )
        .await;
        assert!(
            audited["result"]["records"].is_array() && !names_the_operation(&audited),
            "the audit read that names {mob_id:?} answers the default mob's records alone: \
             {audited}"
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// The topology state of a mob is opened again once the mob's entry went,
/// also while a request still holds what it took from that entry, and what
/// it holds writes nothing after it. A clone of the second mob's controller
/// (what a seam holds of the mob's state) is kept across the end of the
/// launch it was taken in; the next launch over the same state directory
/// opens the second mob's file and reads its revision back; a read and a
/// write through the kept clone are refused, and the file and the mob's
/// revision are what they were.
#[tokio::test]
async fn a_mobs_topology_state_opens_again_after_its_entry_went_while_an_earlier_clone_is_held() {
    let fixture = TopologyHost::new();
    let first = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&first, definition("second")).await;
    let mob = first
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in(&mob).await;
    let applied = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "before-the-entry-went"), "operator")
        .await;
    assert!(
        applied.is_ok(),
        "an apply in the second mob: {:?}",
        applied.as_ref().err()
    );
    let held_from_before = mob.topology_controller().clone();
    drop(mob);
    shut_down(&first, &[]).await;
    drop(first);

    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob resolves in the next launch: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        assert_eq!(
            mob.topology_controller().revision().await,
            1,
            "the next launch opened the second mob's state and read its revision back"
        );
        let files = fixture.state_files_of(&second_id);
        assert_eq!(files.len(), 1, "the second mob's state file: {files:?}");
        let before = files
            .first()
            .map(|file| std::fs::read(file).expect("the state file reads"));

        let late_read = held_from_before.operation("no-such-operation").await;
        assert!(
            matches!(late_read, Err(TopologyControlError::Persistence(_))),
            "a read through the controller of the entry that went is refused: {late_read:?}"
        );
        let late_write = held_from_before.bind_authority(second_id.to_string()).await;
        assert!(
            matches!(late_write, Err(TopologyControlError::Persistence(_))),
            "a write through the controller of the entry that went is refused: {late_write:?}"
        );
        assert_eq!(
            files
                .first()
                .map(|file| std::fs::read(file).expect("the state file reads")),
            before,
            "and writes nothing to the mob's state file"
        );
        assert_eq!(
            mob.topology_controller().revision().await,
            1,
            "the mob's revision is what it was"
        );
    }
    drop(held_from_before);
    shut_down(&host, &[]).await;
}

/// A topology read names its mob: with members of the same names in both
/// mobs, the seam of the second mob answers that mob's id and its own
/// members, and the runtime's own seam the default mob's.
#[tokio::test]
async fn topology_query_reaches_the_named_mob() {
    let fixture = TopologyHost::new();
    let host = fixture
        .launch(TopologyControlPolicy::default(), OtherMobs::Stored)
        .await;
    let second_id = second_mob(&host, definition("second")).await;
    let mob = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    seat_in_the_default_mob(&host).await;
    let seated = Box::pin(mob.spawn(SpawnMemberSpec::new("worker", "alice"))).await;
    assert!(
        seated.is_ok(),
        "the second mob seats alice: {:?}",
        seated.as_ref().err()
    );

    let of_the_second = mob
        .topology_runtime_handle()
        .query()
        .await
        .expect("the second mob's topology");
    let of_the_default = host
        .topology_runtime_handle()
        .query()
        .await
        .expect("the default mob's topology");
    let names = |snapshot: &TopologySnapshot| -> Vec<String> {
        let mut names: Vec<String> = snapshot
            .nodes
            .iter()
            .map(|node| node.endpoint.identity.clone())
            .collect();
        names.sort();
        names
    };
    assert_eq!(of_the_second.authority, second_id.to_string());
    assert_eq!(names(&of_the_second), vec!["alice".to_string()]);
    assert_eq!(of_the_default.authority, host.mob_id());
    assert_eq!(
        names(&of_the_default),
        vec!["alice".to_string(), "bob".to_string()]
    );
    let through_no_mob = host
        .resolve_mob(None)
        .await
        .expect("a request that names no mob")
        .topology_runtime_handle()
        .query()
        .await
        .expect("the topology of a request that names no mob");
    assert_eq!(
        through_no_mob.authority, of_the_default.authority,
        "a request that names no mob reads the default mob"
    );
    drop(mob);
    shut_down(&host, &[&second_id]).await;
}

/// The default mob's topology state keeps its place and its bytes: the
/// file the launch named, which says it belongs to the default mob, no file
/// of the default mob among the other mobs' stores, and a later launch
/// that changes nothing writes the file back byte for byte.
#[tokio::test]
async fn the_launch_mobs_topology_state_file_keeps_its_place_and_bytes() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    seat_in_the_default_mob(&host).await;
    let applied = host
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, "in-the-default-mob"), "operator")
        .await;
    assert!(
        applied.is_ok(),
        "an apply in the default mob: {:?}",
        applied.as_ref().err()
    );

    let file = fixture.default_mobs_state_file();
    assert_eq!(authority_of(&file), Some(host.mob_id()));
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).expect("the state file reads"))
            .expect("the state file is JSON");
    assert_eq!(state["revision"], serde_json::json!(1));
    assert!(
        file.with_extension("json.lock").is_file(),
        "its lock is beside it"
    );
    let default_id = host.mob_handle().mob_id().clone();
    assert_eq!(
        fixture.state_files_and_locks_of(&default_id),
        Vec::<PathBuf>::new(),
        "the default mob keeps no topology state among the other mobs' stores"
    );
    shut_down(&host, &[]).await;
    drop(host);

    // A later launch reads the file and writes back what it read.
    let written_by_the_first_launch = std::fs::read(&file).expect("the state file reads");
    let again = fixture.launch(editable(), OtherMobs::Stored).await;
    assert_eq!(
        again.topology_controller().revision().await,
        1,
        "the default mob's revision is read back"
    );
    shut_down(&again, &[]).await;
    drop(again);
    assert_eq!(
        std::fs::read(&file).expect("the state file reads"),
        written_by_the_first_launch,
        "a launch that changes nothing leaves the default mob's file byte for byte"
    );
}

/// A mob created under the id of a destroyed mob never reads the destroyed
/// mob's topology state, also when nothing removed that state (a stop
/// between the destroy and the removal): the earlier mob's file is still
/// there, and the new mob starts at revision 0 with no operator intent and
/// keeps a file of its own. The removal after a later destroy of that id
/// takes the files of both.
#[tokio::test]
async fn a_mob_created_under_a_freed_id_never_reads_the_earlier_mobs_topology_state() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    {
        let mob = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        seat_in(&mob).await;
        let applied = mob
            .topology_runtime_handle()
            .apply(connect_alice_bob(0, "of-the-earlier-mob"), "operator")
            .await;
        assert!(
            applied.is_ok(),
            "an apply in the second mob: {:?}",
            applied.as_ref().err()
        );
    }
    let of_the_earlier_mob = fixture.state_files_of(&second_id);
    assert_eq!(
        of_the_earlier_mob.len(),
        1,
        "the second mob keeps one topology state file: {of_the_earlier_mob:?}"
    );

    // The destroy, and no removal after it.
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the mob set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    let again = second_mob(&host, second_definition).await;
    assert_eq!(again, second_id, "a new mob under the freed id");
    {
        let mob = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the new mob resolves");
        assert_eq!(
            mob.topology_controller().revision().await,
            0,
            "the new mob starts at revision 0"
        );
        seat_in(&mob).await;
        let fresh = mob
            .topology_runtime_handle()
            .query()
            .await
            .expect("the new mob's topology");
        assert!(
            alice_bob_in(&fresh).is_none_or(|edge| !edge.operator_added),
            "the new mob holds no operator intent of the earlier mob: {:?}",
            fresh.edges
        );
    }
    let of_both = fixture.state_files_of(&second_id);
    assert_eq!(
        of_both.len(),
        2,
        "the new mob keeps a file of its own beside the earlier mob's: {of_both:?}"
    );
    assert!(
        of_both.contains(&of_the_earlier_mob[0]),
        "the earlier mob's file was not taken over"
    );

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the mob set destroys the new mob: {:?}",
        destroyed.as_ref().err()
    );
    let released = host
        .mob_resolver()
        .release_topology_state_of_a_destroyed_mob(&second_id)
        .await;
    assert!(
        released.is_ok(),
        "the host releases the topology state of the destroyed mob: {released:?}"
    );
    assert_eq!(
        fixture.state_files_and_locks_of(&second_id),
        Vec::<PathBuf>::new(),
        "the removal takes the files of every mob that had the id"
    );
    shut_down(&host, &[]).await;
}

/// A mob whose topology state cannot be read is served all the same, with
/// its topology refused. After a restart over a state directory in which
/// the second mob's state file is not a state file, the mob resolves and
/// takes a new member; its topology read and its apply answer the
/// persistence refusal; its reconcile is refused, not run on the declared
/// edges alone; the file is left byte for byte for an operator to repair;
/// and the default mob's topology is served.
#[tokio::test]
async fn a_second_mob_whose_topology_state_cannot_be_read_serves_and_refuses_its_topology() {
    let fixture = TopologyHost::new();
    let first = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&first, definition("second")).await;
    {
        let mob = first
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        seat_in(&mob).await;
    }
    let files = fixture.state_files_of(&second_id);
    assert_eq!(
        files.len(),
        1,
        "the second mob keeps one topology state file: {files:?}"
    );
    shut_down(&first, &[]).await;
    drop(first);
    let Some(file) = files.first() else {
        return;
    };
    let damaged = b"not a topology state file".to_vec();
    std::fs::write(file, &damaged).expect("the state file is replaced");

    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob is served all the same: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        let seated = Box::pin(mob.spawn(SpawnMemberSpec::new("worker", "carol"))).await;
        assert!(
            seated.is_ok(),
            "the second mob takes a new member: {:?}",
            seated.as_ref().err()
        );
        let read = mob.topology_runtime_handle().query().await;
        assert!(
            says_only_that_the_state_was_not_opened(&read),
            "a read of the second mob's topology is refused in the one fixed wording: {read:?}"
        );
        let changed = mob
            .topology_runtime_handle()
            .apply(connect_alice_bob(0, "refused"), "operator")
            .await;
        assert!(
            says_only_that_the_state_was_not_opened(&changed),
            "a change of the second mob's topology is refused in the one fixed wording: \
             {changed:?}"
        );
        let reconciled = mob.reconcile_edges().await;
        assert!(
            !reconciled.is_complete(),
            "a reconcile of the second mob is refused, not run without its operator intent: \
             {reconciled:?}"
        );
    }
    assert_eq!(
        std::fs::read(file).expect("the state file reads"),
        damaged,
        "the file is left as it was found"
    );
    let of_the_default = host.topology_runtime_handle().query().await;
    assert!(
        of_the_default.is_ok(),
        "the default mob's topology is served: {:?}",
        of_the_default.as_ref().err()
    );
    shut_down(&host, &[]).await;
}

/// A host built with the runtime builder serves a mob beside its default
/// mob with topology control of its own, as a host launched by the
/// bootstrap does: the second mob resolves, its controller is not the
/// default mob's, it reads the policy set on the builder, also after a
/// change of it, and it keeps its state in one file beside its store that
/// names it.
#[tokio::test]
async fn a_second_mob_of_a_host_built_with_the_builder_has_topology_control_of_its_own() {
    let fixture = TopologyHost::new();
    let host = fixture
        .launch_with_the_builder(editable(), OtherMobs::Stored)
        .await;
    let second_id = second_mob(&host, definition("second")).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of a host built with the builder is served: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        assert!(
            !mob.topology_controller()
                .is_the_same_controller_in_tests(host.topology_controller()),
            "the second mob has a controller of its own"
        );
        assert_eq!(
            mob.topology_controller().policy(),
            editable(),
            "the second mob's controller reads the policy set on the builder"
        );
        assert!(
            mob.topology_controller().keeps_durable_state(),
            "the second mob keeps its topology state in a file"
        );
        let disabled = host.set_topology_control_policy(policy_of(TopologyControlMode::Disabled));
        assert!(
            disabled.is_ok(),
            "the host changes its policy: {:?}",
            disabled.as_ref().err()
        );
        assert_eq!(
            mob.topology_controller().policy().mode,
            TopologyControlMode::Disabled,
            "the change of the host's policy reaches the second mob"
        );
    }
    let files = fixture.state_files_of(&second_id);
    assert_eq!(
        files.len(),
        1,
        "the second mob keeps one topology state file: {files:?}"
    );
    assert_eq!(
        files
            .first()
            .and_then(|file| authority_of(file.as_path()))
            .as_deref(),
        Some(second_id.as_str()),
        "the file names the second mob"
    );
    shut_down(&host, &[]).await;
}

/// A host built with the runtime builder over a declared store
/// (`.mob_spec(MobBootstrapSpec::declared(..))`: the way a Rust host says
/// that it hosts mobs beside its launch mob) serves a second mob whose
/// member takes a turn. The mob is created through the launch's mob set, a
/// member is seated in it from the mob's own profile, and a turn given to
/// that member starts with what it was given and runs to its end on the
/// launch's model client. The launch mob holds no member of that name.
#[tokio::test]
async fn a_member_of_a_second_mob_of_a_host_built_with_the_builder_takes_a_turn() {
    use futures::StreamExt as _;

    const THE_TASK: &str = "one short task for alice of the second mob";
    let fixture = TopologyHost::new();
    let host = fixture
        .launch_with_the_builder(editable(), OtherMobs::Stored)
        .await;
    let second_id = second_mob(&host, definition("second")).await;
    let alice = meerkat_mob::AgentIdentity::from("alice");
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of a host built with the builder is served: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        seat_in(&mob).await;
    }
    // Some(true): the turn that started with the task ran to its end.
    let mut the_turn_ended = None;
    let handle = set_of(&host).handle_for(&second_id).await;
    assert!(
        handle.is_ok(),
        "the mob set serves the second mob: {:?}",
        handle.as_ref().err()
    );
    if let Ok(handle) = handle {
        let events = handle.subscribe_agent_events(&alice).await;
        let member = handle.member(&alice).await;
        assert!(
            events.is_ok() && member.is_ok(),
            "the second mob's member and its events: {:?} {:?}",
            events.as_ref().err(),
            member.as_ref().err()
        );
        if let (Ok(mut events), Ok(member)) = (events, member) {
            let given = member.internal_turn(THE_TASK).await;
            assert!(
                given.is_ok(),
                "the second mob's member is given a turn: {:?}",
                given.as_ref().err()
            );
            the_turn_ended = tokio::time::timeout(Duration::from_secs(30), async {
                let mut started = false;
                while let Some(event) = events.next().await {
                    match &event.payload {
                        meerkat_core::AgentEvent::RunStarted { .. } => {
                            started = started
                                || serde_json::to_string(&event.payload)
                                    .is_ok_and(|run| run.contains(THE_TASK));
                        }
                        meerkat_core::AgentEvent::RunCompleted { .. } if started => {
                            return Some(true);
                        }
                        meerkat_core::AgentEvent::RunFailed { .. } if started => {
                            return Some(false);
                        }
                        _ => {}
                    }
                }
                None
            })
            .await
            .ok()
            .flatten();
        }
    }
    let the_launch_mob_holds_alice = host.mob_handle().member(&alice).await.is_ok();
    shut_down(&host, &[&second_id]).await;
    assert_eq!(
        the_turn_ended,
        Some(true),
        "the turn given to the second mob's member started with its task and ran to its end"
    );
    assert!(
        !the_launch_mob_holds_alice,
        "the launch mob holds no member of that name"
    );
}

/// After a restart of a host built with the runtime builder, a stored
/// second mob's controller is made from the builder's own: it opens the
/// state file the mob kept before the restart and reads the builder's
/// policy. The revision written into that file between the two launches is
/// what the mob's controller answers.
#[tokio::test]
async fn a_stored_second_mob_of_a_host_built_with_the_builder_reads_its_topology_state_after_a_restart()
 {
    let fixture = TopologyHost::new();
    let first = fixture
        .launch_with_the_builder(editable(), OtherMobs::Stored)
        .await;
    let second_id = second_mob(&first, definition("second")).await;
    let served = first.resolve_mob(Some(&second_id)).await.is_ok();
    assert!(
        served,
        "the second mob of a host built with the builder is served"
    );
    let files = fixture.state_files_of(&second_id);
    assert_eq!(
        files.len(),
        1,
        "the second mob keeps one topology state file: {files:?}"
    );
    shut_down(&first, &[]).await;
    drop(first);
    let Some(file) = files.first() else {
        return;
    };
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(file).expect("the state file reads"))
            .expect("the state file is JSON");
    state["revision"] = serde_json::json!(5);
    std::fs::write(
        file,
        serde_json::to_vec_pretty(&state).expect("the state file's bytes"),
    )
    .expect("the state file is replaced");

    let host = fixture
        .launch_with_the_builder(editable(), OtherMobs::Stored)
        .await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob resolves after the restart: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        assert!(
            mob.topology_controller().keeps_durable_state(),
            "the second mob's controller keeps its state in the file"
        );
        assert_eq!(
            mob.topology_controller().revision().await,
            5,
            "the second mob's controller reads the state file the mob kept"
        );
        assert_eq!(
            mob.topology_controller().policy(),
            editable(),
            "the second mob's controller reads the policy set on the builder"
        );
    }
    assert_eq!(
        fixture.state_files_of(&second_id),
        files,
        "the restart opens the same file and makes no other"
    );
    shut_down(&host, &[]).await;
}

/// A request to connect an agent of one mob of a host with an agent of
/// another mob of that host is refused, typed, and changes nothing. On a
/// host with three mobs whose members have the same names, a `plan` and an
/// `apply` sent to one mob that name a member of another mob as the far
/// end of the edge answer `topology_cross_authority_unsupported` where the
/// host's policy lets an operator edit and allows edges across mobs, and
/// `topology_cross_authority_disabled` where the policy does not allow
/// them, whether it lets an operator edit or is the policy of a host that
/// sets none. After all of them every mob is at revision 0 and no operator
/// connection is read in any.
#[tokio::test]
async fn a_connect_between_agents_of_two_mobs_of_one_host_is_refused() {
    let fixture = TopologyHost::new();
    let across_mobs = TopologyControlPolicy {
        mode: TopologyControlMode::Editable,
        allow_cross_authority: true,
        ..TopologyControlPolicy::default()
    };
    let host = fixture.launch(across_mobs.clone(), OtherMobs::Stored).await;
    let default_id = host.mob_handle().mob_id().clone();
    let second_id = second_mob(&host, definition("second")).await;
    let third_id = second_mob(&host, definition("third")).await;
    seat_in_the_default_mob(&host).await;
    for mob_id in [&second_id, &third_id] {
        let mob = host
            .resolve_mob(Some(mob_id))
            .await
            .expect("a mob of the host beside its default mob resolves");
        seat_in(&mob).await;
    }
    // The mob a request is sent to is named with `mob_id`; the far end of
    // the edge names the other mob as its authority. Both ends are `alice`:
    // each mob has one.
    let across = |far: &MobId| {
        serde_json::json!([{
            "action": "connect",
            "edge": {
                "a": {"identity": "alice"},
                "b": {"authority": far.as_str(), "identity": "alice"},
            },
        }])
    };
    let pairs = [
        (&second_id, &third_id),
        (&third_id, &second_id),
        (&default_id, &second_id),
        (&second_id, &default_id),
    ];
    let policies = [
        (
            "editable, edges across mobs allowed",
            across_mobs.clone(),
            "topology_cross_authority_unsupported",
            -32004,
        ),
        (
            "editable, edges across mobs not allowed",
            editable(),
            "topology_cross_authority_disabled",
            -32602,
        ),
        (
            "the policy of a host that sets none",
            TopologyControlPolicy::default(),
            "topology_cross_authority_disabled",
            -32602,
        ),
    ];

    let mut wrong = Vec::new();
    for (named, policy, kind, code) in policies {
        let set = host.set_topology_control_policy(policy);
        assert!(
            set.is_ok(),
            "the host takes the policy ({named}): {:?}",
            set.as_ref().err()
        );
        for (sent_to, far) in pairs {
            let requests = [
                (
                    "meta/topology/plan",
                    serde_json::json!({
                        "mob_id": sent_to.as_str(),
                        "expected_revision": 0,
                        "operations": across(far),
                    }),
                ),
                (
                    "meta/topology/apply",
                    serde_json::json!({
                        "mob_id": sent_to.as_str(),
                        "expected_revision": 0,
                        "idempotency_key": "across-two-mobs",
                        "operations": across(far),
                    }),
                ),
            ];
            for (method, params) in requests {
                let answered = stdio(&host, method, params).await;
                if !answered["result"].is_null()
                    || answered["error"]["data"]["kind"] != kind
                    || answered["error"]["code"] != code
                {
                    wrong.push(format!(
                        "{named}: {method} sent to {sent_to} with a far end in {far} is not \
                         refused with {kind} ({code}): {answered}"
                    ));
                }
            }
        }
    }

    let set = host.set_topology_control_policy(across_mobs);
    assert!(
        set.is_ok(),
        "the host takes its first policy again: {:?}",
        set.as_ref().err()
    );
    for mob_id in [&default_id, &second_id, &third_id] {
        let read = stdio(
            &host,
            "meta/topology/query",
            serde_json::json!({"mob_id": mob_id.as_str()}),
        )
        .await;
        let connected_by_an_operator = read["result"]["edges"].as_array().is_none_or(|edges| {
            edges
                .iter()
                .any(|edge| edge["operator_added"] == serde_json::json!(true))
        });
        if read["result"]["authority"] != mob_id.as_str()
            || read["result"]["revision"] != 0
            || connected_by_an_operator
        {
            wrong.push(format!(
                "the mob {mob_id} is not at revision 0 with no operator connection after the \
                 refused requests: {read}"
            ));
        }
    }
    shut_down(&host, &[&second_id, &third_id]).await;
    assert!(
        wrong.is_empty(),
        "a connect between agents of two mobs of one host is not refused as it should be:\n{}",
        wrong.join("\n")
    );
}
