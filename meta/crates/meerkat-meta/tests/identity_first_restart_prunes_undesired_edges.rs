//! A managed peer edge the topology no longer wants is unwired across a
//! restart (an application privacy rule: one group's agents must not keep
//! reaching another group's).
//!
//! Every boot runs the production activation path
//! (`install_and_bootstrap_identity_first_context`) on durable state:
//! persistent mob storage, so the mob's wiring and event ledger survive the
//! restart and replay, while the restarted process starts with an empty
//! managed-edge set. Boot 1 wires a team through the topology provider;
//! boot 2 restarts with a provider that drops the lead's edges to the
//! members, which must be gone after boot 2's restore, and the members'
//! edge kept.
//!
//! A member this process never materialized (a worker or an identity Core
//! restored, say) does not stop the reconcile of the other edges either: a
//! raw member's wires are left out, and an identity member's are read through
//! the incarnation Core committed for it, so a dropped edge is still unwired.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::identity_first::contracts::TopologyProvider;
use meerkat_meta::identity_first::orchestrator::RestoreOutcome;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, AgentRuntimeId, AgentRuntimeServices, ContinuityStore,
    DurabilityPolicy, DurableAgentSpec, IdentityBootstrapMode, IdentityFirstRuntimeContext,
    IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider,
    ManagedPeerEdge, MobSessionBridge, MutableRosterProvider, SessionBridge, TopologyContext,
    TopologyError,
};
use meerkat_meta::mob_composition_manifest::persistent_mob_storage;
use meerkat_meta::{
    DesiredPeerEdge, DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
};
use meerkat_mob::{MobDefinition, ProfileName, SpawnMemberSpec};

#[path = "support/llm_usage.rs"]
mod llm_usage;

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).unwrap()
}

fn spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: id(name),
        profile: ProfileName::from("personal"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
"#
    ))
    .expect("parse the team definition")
}

/// The host's topology, which the test shrinks between boots.
struct TeamTopology {
    edges: Mutex<Vec<ManagedPeerEdge>>,
}

#[async_trait]
impl TopologyProvider for TeamTopology {
    async fn compute_edges(
        &self,
        _target_identities: &[AgentIdentity],
        _context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        Ok(self.edges.lock().unwrap().clone())
    }
}

/// Answers every turn; no turn runs in this test, but the runtime needs a
/// client.
#[derive(Clone, Default)]
struct OkClient;

impl meerkat_client::LlmClient for OkClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

struct Booted {
    unified: meerkat_meta::UnifiedRuntime,
    bridge: Arc<dyn SessionBridge>,
    runtime_ids: BTreeMap<AgentRuntimeId, AgentIdentity>,
}

/// Boot on durable state the way a deployment does: persistent mob storage, a
/// SQLite session store and a durable continuity store, activated through
/// `install_and_bootstrap_identity_first_context`.
async fn boot(
    state_path: &std::path::Path,
    definition: MobDefinition,
    roster: &[DurableAgentSpec],
    topology: Option<Arc<dyn TopologyProvider>>,
) -> Booted {
    let (unified, bridge, outcomes) = boot_in_mode(
        state_path,
        definition,
        roster,
        topology,
        IdentityBootstrapMode::EagerMaterialize,
    )
    .await;
    Booted {
        unified,
        bridge,
        runtime_ids: runtime_ids(&outcomes),
    }
}

/// [`boot`] with the identity bootstrap mode given, returning the restore
/// outcomes as they are: under a lazy mode the identities register without
/// materializing.
async fn boot_in_mode(
    state_path: &std::path::Path,
    definition: MobDefinition,
    roster: &[DurableAgentSpec],
    topology: Option<Arc<dyn TopologyProvider>>,
    mode: IdentityBootstrapMode,
) -> (
    meerkat_meta::UnifiedRuntime,
    Arc<dyn SessionBridge>,
    BTreeMap<AgentIdentity, RestoreOutcome>,
) {
    std::fs::create_dir_all(state_path).expect("state root");
    let (storage, provenance) = persistent_mob_storage(state_path.join("mob.sqlite3"))
        .expect("open persistent mob storage");
    let spec = MobBootstrapSpec::persistent(
        definition.clone(),
        storage,
        provenance,
        state_path.to_path_buf(),
        16,
    )
    .await
    .expect("compose persistent Meta stores")
    // Creates no agent child mob. It restarts in this process, which the
    // durable child-mob default refuses as a duplicate owner; explicit
    // ProcessBound keeps this test's cold-recovery oracle.
    .with_child_mob_storage(meerkat_meta::ChildMobStorage::ProcessBound)
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(OkClient)),
    });
    let mut unified = meerkat_meta::UnifiedRuntime::bootstrap(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: "edge-prune-restart".to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        std::time::Duration::from_secs(10),
    )
    .await
    .expect("bootstrap the UnifiedRuntime");
    let bridge: Arc<dyn SessionBridge> = Arc::new(MobSessionBridge::with_session_service(
        unified.mob_handle(),
        unified
            .mob_runtime()
            .session_service()
            .cloned()
            .expect("the persistent runtime has a session service"),
    ));
    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.sqlite3"))
            .expect("continuity store"),
    );
    let identity_rt = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: store as Arc<dyn ContinuityStore>,
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "edge-prune-restart".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(unified.mob_handle())),
    );
    let context = Arc::new(IdentityFirstRuntimeContext::new_with_bootstrap_mode(
        identity_rt,
        Arc::new(MutableRosterProvider::new(roster.to_vec())),
        topology,
        None,
        Some(definition),
        mode,
    ));
    let result = unified
        .install_and_bootstrap_identity_first_context(context, roster)
        .await
        .expect("activate and restore the identity-first runtime");
    (unified, bridge, result.outcomes)
}

/// The identity edges Core's roster holds, either directed half, read from
/// the roster itself: an identity this process has not materialized has no
/// bridge binding to read them through.
async fn roster_identity_edges(
    unified: &meerkat_meta::UnifiedRuntime,
) -> BTreeSet<(AgentIdentity, AgentIdentity)> {
    let members = unified.mob_handle().list_members_including_retiring().await;
    let identity_of: BTreeMap<String, AgentIdentity> = members
        .iter()
        .filter_map(|member| {
            let label = member.labels.get("agent_identity")?;
            Some((member.agent_identity.to_string(), id(label)))
        })
        .collect();
    members
        .iter()
        .flat_map(|member| {
            let identity_of = &identity_of;
            member.wired_to.iter().filter_map(move |peer| {
                let a = identity_of.get(&member.agent_identity.to_string())?;
                let b = identity_of.get(&peer.to_string())?;
                Some(pair(a, b))
            })
        })
        .collect()
}

/// The live identity edges, as unordered identity pairs.
async fn live_identity_edges(
    bridge: &Arc<dyn SessionBridge>,
    runtime_ids: &BTreeMap<AgentRuntimeId, AgentIdentity>,
) -> BTreeSet<(AgentIdentity, AgentIdentity)> {
    bridge
        .current_member_wires()
        .await
        .expect("inspect the live wiring")
        .into_iter()
        .filter_map(|(a, b)| {
            let a = runtime_ids.get(&a)?.clone();
            let b = runtime_ids.get(&b)?.clone();
            Some(if a <= b { (a, b) } else { (b, a) })
        })
        .collect()
}

fn runtime_ids(
    outcomes: &BTreeMap<AgentIdentity, RestoreOutcome>,
) -> BTreeMap<AgentRuntimeId, AgentIdentity> {
    outcomes
        .iter()
        .map(|(identity, outcome)| match outcome {
            RestoreOutcome::Created { binding, .. } | RestoreOutcome::Resumed { binding, .. } => {
                (binding.agent_runtime_id.clone(), identity.clone())
            }
            other => panic!("{identity} did not materialize: {other:?}"),
        })
        .collect()
}

fn pair(a: &AgentIdentity, b: &AgentIdentity) -> (AgentIdentity, AgentIdentity) {
    if a <= b {
        (a.clone(), b.clone())
    } else {
        (b.clone(), a.clone())
    }
}

fn next_mob_id(prefix: &str) -> String {
    static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{prefix}-{}",
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn with_profile(name: &str, profile: &str) -> DurableAgentSpec {
    let mut spec = spec(name);
    spec.profile = ProfileName::from(profile);
    spec
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edge_the_topology_dropped_is_unwired_after_a_restart() {
    let mob_id = next_mob_id("edge-prune-restart");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let lead = id("identity:lead-1");
    let member = id("identity:member-1");
    let peer = id("identity:member-2");
    let roster = vec![
        spec("identity:lead-1"),
        spec("identity:member-1"),
        spec("identity:member-2"),
    ];
    let members = ManagedPeerEdge::new(member.clone(), peer.clone()).unwrap();
    let topology = Arc::new(TeamTopology {
        edges: Mutex::new(vec![
            ManagedPeerEdge::new(lead.clone(), member.clone()).unwrap(),
            ManagedPeerEdge::new(lead.clone(), peer.clone()).unwrap(),
            members.clone(),
        ]),
    });

    // Boot 1: the team is wired as the topology declares.
    let booted = boot(
        &state_path,
        definition(&mob_id),
        &roster,
        Some(topology.clone() as Arc<dyn TopologyProvider>),
    )
    .await;
    let live = live_identity_edges(&booted.bridge, &booted.runtime_ids).await;
    assert_eq!(
        live,
        BTreeSet::from([
            pair(&lead, &member),
            pair(&lead, &peer),
            pair(&member, &peer),
        ]),
        "boot 1 wires the team"
    );
    booted.unified.shutdown().await;
    drop(booted);

    // The host's topology now keeps the members away from the lead.
    *topology.edges.lock().unwrap() = vec![members];

    // Boot 2: the same durable state; the mob replays the lead edges, and
    // the restarted process never wired them itself.
    let booted = boot(
        &state_path,
        definition(&mob_id),
        &roster,
        Some(topology.clone() as Arc<dyn TopologyProvider>),
    )
    .await;
    let live = live_identity_edges(&booted.bridge, &booted.runtime_ids).await;
    assert_eq!(
        live,
        BTreeSet::from([pair(&member, &peer)]),
        "after the restart only the desired edge is wired"
    );
    booted.unified.shutdown().await;
}

fn fight_definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[[wiring.role_wiring]]
a = "member"
b = "helper"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.lead.tools]
comms = true

[profiles.member]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.member.tools]
comms = true

[profiles.helper]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.helper.tools]
comms = true
"#
    ))
    .expect("parse the role-wired definition")
}

/// No behaviour change without a topology provider: the definition's
/// `role_wiring` edge, wired at spawn time by the mob, is not this runtime's
/// to prune, and survives a restart and a reconcile.
#[tokio::test(flavor = "multi_thread")]
async fn without_a_topology_provider_spawn_time_wiring_survives_a_restart() {
    let mob_id = next_mob_id("edge-keep-no-provider");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let member = id("identity:member");
    let helper = id("identity:helper");
    let roster = vec![
        with_profile("identity:helper", "helper"),
        with_profile("identity:member", "member"),
    ];

    let booted = boot(&state_path, fight_definition(&mob_id), &roster, None).await;
    let live = live_identity_edges(&booted.bridge, &booted.runtime_ids).await;
    assert_eq!(
        live,
        BTreeSet::from([pair(&member, &helper)]),
        "the mob wires the definition's edge at spawn"
    );
    booted.unified.shutdown().await;
    drop(booted);

    let booted = boot(&state_path, fight_definition(&mob_id), &roster, None).await;
    let report = booted.unified.reconcile_edges().await;
    assert!(report.failures.is_empty(), "{report:?}");
    let live = live_identity_edges(&booted.bridge, &booted.runtime_ids).await;
    assert_eq!(
        live,
        BTreeSet::from([pair(&member, &helper)]),
        "without a topology provider the restart and reconcile keep the edge"
    );
    booted.unified.shutdown().await;
}

/// Wire and unwire events in the mob event ledger, over every member edge.
async fn wiring_events(unified: &meerkat_meta::UnifiedRuntime) -> (usize, usize) {
    let events = unified
        .mob_handle()
        .events()
        .replay_all()
        .await
        .expect("replay the mob event ledger");
    let mut wired = 0;
    let mut unwired = 0;
    for event in events {
        match event.kind {
            meerkat_mob::MobEventKind::MembersWired { .. } => wired += 1,
            meerkat_mob::MobEventKind::MembersWiredBatch { edges } => wired += edges.len(),
            meerkat_mob::MobEventKind::MembersUnwired { .. } => unwired += 1,
            _ => {}
        }
    }
    (wired, unwired)
}

/// No owner fights another: with a topology provider and the definition's
/// `role_wiring` declaring an edge the provider does not, the mob's
/// spawn-time wiring and the topology's reconcile both run, across a restart
/// and two reconciles per boot, and the edge stays wired with no wire/unwire
/// churn in the durable ledger. The provider's own edge is kept as well.
#[tokio::test(flavor = "multi_thread")]
async fn a_definition_declared_edge_is_never_churned_by_the_topology() {
    let mob_id = next_mob_id("edge-no-fight");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let lead = id("identity:lead");
    let member = id("identity:member");
    let helper = id("identity:helper");
    let roster = vec![
        with_profile("identity:helper", "helper"),
        with_profile("identity:member", "member"),
        with_profile("identity:lead", "lead"),
    ];
    let topology = Arc::new(TeamTopology {
        edges: Mutex::new(vec![
            ManagedPeerEdge::new(lead.clone(), member.clone()).unwrap(),
        ]),
    });
    let expected = BTreeSet::from([pair(&member, &helper), pair(&lead, &member)]);

    // One boot: the activation's restore and reconcile, then two more
    // reconciles.
    let run_boot = |label: &'static str| {
        let state_path = state_path.clone();
        let mob_id = mob_id.clone();
        let roster = roster.clone();
        let topology = topology.clone();
        async move {
            let booted = boot(
                &state_path,
                fight_definition(&mob_id),
                &roster,
                Some(topology as Arc<dyn TopologyProvider>),
            )
            .await;
            for round in 0..2 {
                let report = booted.unified.reconcile_edges().await;
                assert!(
                    report.failures.is_empty(),
                    "reconcile {round} ({label}): {report:?}"
                );
            }
            booted
        }
    };

    let booted = run_boot("boot 1").await;
    let live = live_identity_edges(&booted.bridge, &booted.runtime_ids).await;
    assert_eq!(
        live, expected,
        "boot 1: the definition's and the provider's edges"
    );
    let (wired_boot1, unwired_boot1) = wiring_events(&booted.unified).await;
    assert_eq!(unwired_boot1, 0, "boot 1 unwires nothing");
    assert!(wired_boot1 >= 2, "boot 1 wired both edges: {wired_boot1}");
    booted.unified.shutdown().await;
    drop(booted);

    let booted = run_boot("boot 2").await;
    let live = live_identity_edges(&booted.bridge, &booted.runtime_ids).await;
    assert_eq!(live, expected, "boot 2: both edges are still wired");
    let (wired_boot2, unwired_boot2) = wiring_events(&booted.unified).await;
    assert_eq!(
        (wired_boot2, unwired_boot2),
        (wired_boot1, 0),
        "no wire/unwire churn across the restart and its reconciles"
    );
    booted.unified.shutdown().await;
}

/// Boot the three-identity team, spawn `extra` on the mob itself with
/// `labels` (no bridge binding records it, as for a member Core restores that
/// this process has not materialized), wire it to the lead, and unwire the
/// lead from the member so the reconcile has a managed edge to put back.
async fn team_with_an_unbound_member(
    mob_prefix: &str,
    state_path: &std::path::Path,
    extra: &str,
    labels: BTreeMap<String, String>,
) -> Booted {
    let lead = id("identity:lead-1");
    let member = id("identity:member-1");
    let peer = id("identity:member-2");
    let roster = vec![
        spec("identity:lead-1"),
        spec("identity:member-1"),
        spec("identity:member-2"),
    ];
    let topology = Arc::new(TeamTopology {
        edges: Mutex::new(vec![
            ManagedPeerEdge::new(lead.clone(), member.clone()).unwrap(),
            ManagedPeerEdge::new(lead.clone(), peer.clone()).unwrap(),
            ManagedPeerEdge::new(member.clone(), peer).unwrap(),
        ]),
    });
    let booted = boot(
        state_path,
        definition(&next_mob_id(mob_prefix)),
        &roster,
        Some(topology as Arc<dyn TopologyProvider>),
    )
    .await;
    let handle = booted.unified.mob_handle();
    let mut extra_spec =
        SpawnMemberSpec::from_wire("personal".to_string(), extra.to_string(), None, None, None);
    if !labels.is_empty() {
        extra_spec = extra_spec.with_labels(labels);
    }
    handle
        .spawn_spec(extra_spec)
        .await
        .expect("spawn the unbound member");
    handle
        .wire(
            meerkat_mob::AgentIdentity::from(extra),
            meerkat_meta::member_comms_id::mob_member_id(lead.as_str()),
        )
        .await
        .expect("wire the unbound member to the lead");
    let runtime_of = |identity: &AgentIdentity| {
        booted
            .runtime_ids
            .iter()
            .find(|(_, bound)| *bound == identity)
            .map(|(runtime_id, _)| runtime_id.clone())
            .expect("a materialized identity")
    };
    booted
        .bridge
        .unwire_peer(&runtime_of(&lead), &runtime_of(&member))
        .await
        .expect("unwire the lead from the member");
    booted
}

/// A raw roster member (no `agent_identity` label: a worker a member
/// spawned, say) that this process never materialized has no live runtime
/// binding, so the wire projection cannot name its incarnation. The identity
/// topology never manages a raw member's wires, so it must not stop the
/// reconcile of every other edge: its wire is left out, and the managed edges
/// converge.
#[tokio::test(flavor = "multi_thread")]
async fn a_raw_member_without_a_runtime_binding_does_not_stop_the_edge_reconcile() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let booted = team_with_an_unbound_member(
        "edge-unbound-raw-member",
        &temp.path().join("state"),
        "investigation-worker-1",
        BTreeMap::new(),
    )
    .await;
    let lead = id("identity:lead-1");
    let member = id("identity:member-1");
    let peer = id("identity:member-2");

    let report = booted.unified.reconcile_edges().await;
    assert!(report.failures.is_empty(), "nothing fails: {report:?}");
    assert_eq!(
        report.wired_edges,
        vec![DesiredPeerEdge::new(lead.as_str(), member.as_str()).unwrap()],
        "the unwired edge is wired again: {report:?}"
    );
    assert_eq!(report.retained_edges.len(), 2, "{report:?}");
    assert!(
        report.skipped_missing_members.is_empty(),
        "a raw member's wire is no managed edge to skip: {report:?}"
    );
    assert_eq!(
        live_identity_edges(&booted.bridge, &booted.runtime_ids).await,
        BTreeSet::from([
            pair(&lead, &member),
            pair(&lead, &peer),
            pair(&member, &peer),
        ]),
        "every managed edge is live"
    );
    booted.unified.shutdown().await;
}

/// An identity member (labelled `agent_identity`, as every identity-first
/// spawn is) that this process has not materialized has no binding either,
/// but Core holds its member: the wire projection names it by the incarnation
/// Core committed for it, the read the identity runtime's binding epoch makes,
/// so its present wire is never read as absent and nothing fails.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_member_without_a_runtime_binding_is_read_through_its_committed_incarnation() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let ghost = meerkat_meta::member_comms_id::mob_member_id("identity:ghost-1");
    let booted = team_with_an_unbound_member(
        "edge-unbound-identity-member",
        &temp.path().join("state"),
        ghost.as_str(),
        BTreeMap::from([("agent_identity".to_string(), "identity:ghost-1".to_string())]),
    )
    .await;

    let report = booted.unified.reconcile_edges().await;
    assert!(report.failures.is_empty(), "nothing fails: {report:?}");
    let wires = booted
        .bridge
        .current_member_wires()
        .await
        .expect("the projection names every wire");
    assert_eq!(
        wires.len(),
        4,
        "the team's three edges and the ghost's: {wires:?}"
    );
    assert!(
        wires
            .iter()
            .any(|(a, b)| a.as_str().contains("ghost-1") || b.as_str().contains("ghost-1")),
        "the ghost's wire is named by its incarnation: {wires:?}"
    );
    booted.unified.shutdown().await;
}

/// A restart that leaves the identities unmaterialized (lazy bootstrap here;
/// in a deployment also an identity whose activation failed) records no
/// runtime binding for them in this process, while Core holds their members
/// and replays their wiring. The reconcile still reads every live wire: an
/// edge the topology dropped is unwired rather than read as absent, the kept
/// edge stays, and nothing fails.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_that_leaves_the_identities_unmaterialized_still_reconciles_every_edge() {
    let mob_id = next_mob_id("edge-unmaterialized-restart");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let lead = id("identity:lead-1");
    let member = id("identity:member-1");
    let peer = id("identity:member-2");
    let roster = vec![
        spec("identity:lead-1"),
        spec("identity:member-1"),
        spec("identity:member-2"),
    ];
    let members = ManagedPeerEdge::new(member.clone(), peer.clone()).unwrap();
    let topology = Arc::new(TeamTopology {
        edges: Mutex::new(vec![
            ManagedPeerEdge::new(lead.clone(), member.clone()).unwrap(),
            ManagedPeerEdge::new(lead.clone(), peer.clone()).unwrap(),
            members.clone(),
        ]),
    });
    let booted = boot(
        &state_path,
        definition(&mob_id),
        &roster,
        Some(topology.clone() as Arc<dyn TopologyProvider>),
    )
    .await;
    assert_eq!(
        roster_identity_edges(&booted.unified).await,
        BTreeSet::from([
            pair(&lead, &member),
            pair(&lead, &peer),
            pair(&member, &peer),
        ]),
        "boot 1 wires the team"
    );
    booted.unified.shutdown().await;
    drop(booted);

    // The topology now keeps the members away from the lead, and boot 2
    // registers the identities without materializing them.
    *topology.edges.lock().unwrap() = vec![members];
    let (unified, _bridge, _outcomes) = boot_in_mode(
        &state_path,
        definition(&mob_id),
        &roster,
        Some(topology as Arc<dyn TopologyProvider>),
        IdentityBootstrapMode::LazyMaterialize,
    )
    .await;
    if let Some(report) = unified.bootstrap_edges_report().await {
        assert!(
            report.failures.is_empty(),
            "the bootstrap reconcile fails nothing: {report:?}"
        );
    }
    let report = unified.reconcile_edges().await;
    assert!(report.failures.is_empty(), "nothing fails: {report:?}");
    assert_eq!(
        roster_identity_edges(&unified).await,
        BTreeSet::from([pair(&member, &peer)]),
        "the dropped edges are unwired and the kept edge stays"
    );
    unified.shutdown().await;
}

/// The managed path of the same restart: boot 2 reconciles the full topology
/// first, so the team's edges become this process's managed edges while their
/// identities stay unmaterialized, and the topology then drops the lead's
/// edges. A managed edge that is no longer desired is unwired through its
/// unbound endpoints, not dropped from the managed set with its wire left live.
#[tokio::test(flavor = "multi_thread")]
async fn a_managed_edge_between_unmaterialized_identities_is_unwired_once_dropped() {
    let mob_id = next_mob_id("edge-unmaterialized-managed");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let lead = id("identity:lead-1");
    let member = id("identity:member-1");
    let peer = id("identity:member-2");
    let roster = vec![
        spec("identity:lead-1"),
        spec("identity:member-1"),
        spec("identity:member-2"),
    ];
    let members = ManagedPeerEdge::new(member.clone(), peer.clone()).unwrap();
    let topology = Arc::new(TeamTopology {
        edges: Mutex::new(vec![
            ManagedPeerEdge::new(lead.clone(), member.clone()).unwrap(),
            ManagedPeerEdge::new(lead.clone(), peer.clone()).unwrap(),
            members.clone(),
        ]),
    });
    let booted = boot(
        &state_path,
        definition(&mob_id),
        &roster,
        Some(topology.clone() as Arc<dyn TopologyProvider>),
    )
    .await;
    booted.unified.shutdown().await;
    drop(booted);

    let (unified, _bridge, _outcomes) = boot_in_mode(
        &state_path,
        definition(&mob_id),
        &roster,
        Some(topology.clone() as Arc<dyn TopologyProvider>),
        IdentityBootstrapMode::LazyMaterialize,
    )
    .await;
    let report = unified.reconcile_edges().await;
    assert!(report.failures.is_empty(), "nothing fails: {report:?}");
    assert_eq!(
        report.retained_edges.len() + report.wired_edges.len(),
        3,
        "boot 2 takes the team's edges as its managed edges: {report:?}"
    );

    *topology.edges.lock().unwrap() = vec![members];
    let report = unified.reconcile_edges().await;
    assert!(report.failures.is_empty(), "nothing fails: {report:?}");
    assert_eq!(
        report.unwired_edges,
        vec![
            DesiredPeerEdge::new(lead.as_str(), member.as_str()).unwrap(),
            DesiredPeerEdge::new(lead.as_str(), peer.as_str()).unwrap(),
        ],
        "the dropped managed edges are unwired: {report:?}"
    );
    assert_eq!(
        roster_identity_edges(&unified).await,
        BTreeSet::from([pair(&member, &peer)]),
        "Core's roster holds only the kept edge"
    );
    unified.shutdown().await;
}
