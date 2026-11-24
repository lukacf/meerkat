//! Rows of the family of an agent's changes and reads for a mob a host
//! serves beside its launch mob.
//!
//! The fixture is the launch of the identity plane's rows: a launch whose
//! launch mob has its identity context and whose composer stands for the
//! gateways'. The rows ask the family as the stdio host asks it: one
//! resolved mob, its handle and its family; the Console's rows ask its
//! request door as a signed-in person.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use meerkat_mob::MobId;

use super::UnifiedRuntime;
use super::mob_entries_tests::{
    Sessions, configured_agent, identity_context, second_mob, set_of, shut_down, transcript_showing,
};
use crate::gateway_wiring::HostMobIdentities;
use crate::identity_first::{DurableAgentSpec, RosterContext, RosterError, RosterProvider};

/// The roster provider of a launch: one for every mob of the host. It
/// answers the roster configured for the mob whose definition it is asked
/// with.
#[derive(Default)]
struct LaunchRoster {
    by_mob: Mutex<BTreeMap<MobId, Vec<DurableAgentSpec>>>,
}

impl LaunchRoster {
    fn configure(&self, mob_id: &MobId, agents: &[&str]) {
        self.by_mob
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                mob_id.clone(),
                agents.iter().map(|name| configured_agent(name)).collect(),
            );
    }
}

#[async_trait::async_trait]
impl RosterProvider for LaunchRoster {
    async fn roster(&self, context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(context
            .mob_definition
            .as_ref()
            .and_then(|definition| {
                self.by_mob
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&definition.id)
                    .cloned()
            })
            .unwrap_or_default())
    }
}

/// The definition of a mob of the rows: one profile, `worker`, under an id
/// of its own. The profile has mob tools: an agent of it holds the spawn
/// grant an agent's own identity tools ask for.
fn definition(tag: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"mob-family-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n[profiles.worker.tools]\ncomms = true\nmob = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition with one profile")
}

/// A runtime whose launch declares the mob of `definition` to its mob set,
/// in memory, as the launch of the entries' rows does, with one
/// difference: the agent mob tools are installed in the slot of the
/// launch's own session builder, as a gateway installs them, so a member
/// whose profile has mob tools is built with them. With it, the launch's
/// session service.
async fn launch(
    temp: &tempfile::TempDir,
    definition: meerkat_mob::MobDefinition,
) -> (UnifiedRuntime, Sessions) {
    launch_with_model(
        temp,
        definition,
        Arc::new(meerkat_client::TestClient::default()),
    )
    .await
}

/// [`launch`], with `model` as the model of its mobs' members.
async fn launch_with_model(
    temp: &tempfile::TempDir,
    definition: meerkat_mob::MobDefinition,
    model: Arc<dyn meerkat_client::LlmClient>,
) -> (UnifiedRuntime, Sessions) {
    use crate::blob_store::{Base64BlobStoreAdapter, ObjectStoreBlobStore};
    use crate::mob_handle_runtime::{MobBootstrapOptions, MobBootstrapSpec};

    let store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blobs: Arc<dyn meerkat_core::BlobStore> = Arc::new(Base64BlobStoreAdapter::new(Arc::new(
        ObjectStoreBlobStore::memory(),
    )));
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(store.clone(), blobs.clone())
            .expect("the runtime machine"),
    );
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    let mut builder = meerkat::FactoryAgentBuilder::new(
        meerkat::AgentFactory::new(temp.path())
            .builtins(false)
            .comms(true),
        meerkat::Config::default(),
    );
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let mob_tools = Arc::clone(&builder.default_mob_tools);
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        8,
        Arc::new(meerkat_store::MemoryStore::new()),
        store.clone(),
        blobs,
    ));
    let namespace = definition.id.to_string();
    let spec = MobBootstrapSpec::declared(
        definition,
        crate::mob_composition_manifest::DeclaredMobStore::in_memory(),
        service.clone(),
    )
    .with_session_runtime_adapter(Arc::clone(&machine))
    .expect("the launch adopts the runtime machine")
    .with_runtime_delivery(crate::MobRuntimeDelivery::new(store, jobs))
    .with_agent_mob_tools(mob_tools)
    .expect("the agent mob tools")
    .with_child_mob_storage(crate::ChildMobStorage::ProcessBound)
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: false,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(model),
    });
    let runtime = Box::pin(UnifiedRuntime::bootstrap_with_options(
        spec,
        crate::MetaConfig {
            modules: Vec::new(),
            discovery: crate::DiscoverySpec {
                namespace,
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Vec::new(),
        std::time::Duration::from_secs(2),
        crate::runtime::RuntimeOptions::default(),
        Arc::new(crate::runtime::InMemoryMetadataStore::new()),
    ))
    .await
    .expect("a runtime over a declared store");
    (runtime, service)
}

/// A launch whose launch mob has its identity context and holds the agents
/// `desk` and `front`, with the session service every mob of the host
/// builds its members' sessions in.
async fn launch_with_identities(temp: &tempfile::TempDir) -> (UnifiedRuntime, Sessions) {
    let (mut host, sessions) = launch(temp, definition("host")).await;
    let of_the_launch_mob = identity_context(host.mob_runtime(), "default");
    Box::pin(host.install_and_bootstrap_identity_first_context(
        of_the_launch_mob,
        &[configured_agent("desk"), configured_agent("front")],
    ))
    .await
    .expect("the launch mob's identity bootstrap");
    (host, sessions)
}

/// The composer both gateways and the builder hand the runtime, over the
/// launch's one roster provider, with the values of a launch on a durable
/// store that is no candidate.
fn gateways_composer(roster: &Arc<LaunchRoster>) -> HostMobIdentities {
    HostMobIdentities::new(Arc::clone(roster) as Arc<dyn RosterProvider>, true)
}

/// The agent the rows create in each mob: one name in both.
const SCOUT: &str = "scout";

/// The policy of the rows: the stdio host may create, archive, unarchive
/// and remove an agent of the profile `worker` at once (an unarchive is
/// judged as an archive is), a person may make every change of one but a
/// change of its authority at once (the policy allows no change of an
/// agent's authority at once), the Console's own surface may create one
/// at once, and nothing else is allowed.
fn the_host_changes_workers() -> Arc<crate::identity_creation::IdentityCreationPolicy> {
    let workers = |change: &[&str], requester: serde_json::Value| {
        serde_json::json!({
            "change": change,
            "requester": requester,
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        })
    };
    Arc::new(
        crate::identity_creation::IdentityCreationPolicy::parse(&serde_json::json!({
            "enabled": true,
            "approval": {"rules": [
                workers(
                    &["create", "archive", "remove"],
                    serde_json::json!({"kind": "host", "surface": "rpc"}),
                ),
                workers(
                    &[
                        "create",
                        "descriptive",
                        "capability",
                        "peers",
                        "profile",
                        "remove",
                        "archive",
                        "adopt",
                    ],
                    serde_json::json!({"kind": "human"}),
                ),
                workers(
                    &["create"],
                    serde_json::json!({"kind": "host", "surface": "console"}),
                ),
            ]},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled"),
    )
}

/// One call of the family of an agent's changes and reads, asked for the
/// mob `mob_id` of `host` (the launch mob for `None`) as the stdio host
/// asks it: the answer's result, or why there is none (the mob is not
/// served, it has no family, the method is outside the family, or the
/// family's own error).
async fn asked_of_the_family(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mob = host
        .resolve_mob(mob_id)
        .await
        .map_err(|error| format!("the mob is not served: {error:?}"))?;
    let family = mob
        .identity_runtime_ref()
        .and_then(|identities| identities.identity_family())
        .ok_or_else(|| "the mob has no family of its agents' changes".to_string())?;
    let handle = mob.mob_runtime().handle();
    let surface = crate::rpc::identity_methods::IdentityFamilySurface {
        mob: &handle,
        family: family.as_ref(),
        requester: crate::rpc::identity_methods::stdio_requester(),
        access: None,
    };
    let answer = crate::rpc::identity_methods::handle_identity_family_rpc(
        &surface,
        method,
        serde_json::json!(1),
        &params,
    )
    .await
    .ok_or_else(|| format!("{method} is outside the family"))?;
    match (answer.result, answer.error) {
        (Some(result), _) => Ok(result),
        (None, Some(error)) => Err(format!("{}: {:?}", error.message, error.data)),
        (None, None) => Err("the family answered neither a result nor an error".to_string()),
    }
}

/// The names of the agents a list of the family names, sorted.
fn listed_names(list: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = list["agents"]
        .as_array()
        .map(|agents| {
            agents
                .iter()
                .filter_map(|agent| agent["name"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The names a list of the family asked for `mob_id` answers.
async fn listed_in(host: &UnifiedRuntime, mob_id: Option<&MobId>) -> Result<Vec<String>, String> {
    asked_of_the_family(host, mob_id, "meta/identity/list", serde_json::json!({}))
        .await
        .map(|list| listed_names(&list))
}

/// A launch whose launch mob holds the agents `desk` and `front`, and a
/// second mob of it that holds the agents `desk` and `back`: an agent of
/// one name in each. With `policy`, the launch attached it before the
/// second mob was composed.
async fn two_mobs(
    temp: &tempfile::TempDir,
    policy: Option<Arc<crate::identity_creation::IdentityCreationPolicy>>,
) -> (UnifiedRuntime, MobId) {
    let (host, _sessions, second_id) = two_mobs_and_their_sessions(temp, policy).await;
    (host, second_id)
}

/// [`two_mobs`], with the session service of the launch.
async fn two_mobs_and_their_sessions(
    temp: &tempfile::TempDir,
    policy: Option<Arc<crate::identity_creation::IdentityCreationPolicy>>,
) -> (UnifiedRuntime, Sessions, MobId) {
    let (host, sessions) = launch_with_identities(temp).await;
    if let Some(policy) = policy {
        assert!(
            host.attach_identity_policy(
                policy,
                crate::identity_creation::IdentityFamilyOptions::new()
            ),
            "the launch attaches its identity policy"
        );
    }
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(&roster)));
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&host, of_the_second).await;
    (host, sessions, second_id)
}

/// [`two_mobs`] under the rows' policy.
async fn two_mobs_under_one_identity_policy(temp: &tempfile::TempDir) -> (UnifiedRuntime, MobId) {
    two_mobs(temp, Some(the_host_changes_workers())).await
}

/// The stdio host creates the agent [`SCOUT`] of the profile `worker` in
/// the mob `mob_id`.
async fn scout_created_in(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
) -> Result<serde_json::Value, String> {
    asked_of_the_family(
        host,
        mob_id,
        "meta/identity/create",
        serde_json::json!({
            "request_id": "create-the-scout",
            "agents": [{"name": SCOUT, "profile": "worker"}],
        }),
    )
    .await
}

/// One change of the agent [`SCOUT`] of the mob `mob_id` (`archive`,
/// `unarchive` or `remove`), asked with the digest a get answers now.
async fn scout_changed_in(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
    change: &str,
) -> Result<serde_json::Value, String> {
    let read = asked_of_the_family(
        host,
        mob_id,
        "meta/identity/get",
        serde_json::json!({"name": SCOUT}),
    )
    .await?;
    asked_of_the_family(
        host,
        mob_id,
        &format!("meta/identity/{change}"),
        serde_json::json!({
            "request_id": format!("{change}-the-scout"),
            "name": SCOUT,
            "expected_spec_digest": read["spec_digest"],
        }),
    )
    .await
}

/// Whether the agent [`SCOUT`] of the mob `mob_id` is archived, as a get
/// answers it.
async fn scout_archived_in(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
) -> Result<Option<bool>, String> {
    asked_of_the_family(
        host,
        mob_id,
        "meta/identity/get",
        serde_json::json!({"name": SCOUT}),
    )
    .await
    .map(|agent| agent["archived"].as_bool())
}

/// A second mob of a launch that attached an identity policy has the
/// family of its own agents: a list asked for it names that mob's agents
/// and none of the launch mob's, and the launch mob's list is its own.
#[tokio::test]
async fn a_second_mob_of_a_launch_with_an_identity_policy_has_the_family_of_its_own_agents() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, second_id) = two_mobs_under_one_identity_policy(&temp).await;
    let of_the_second = listed_in(&host, Some(&second_id)).await;
    let of_the_launch_mob = listed_in(&host, None).await;
    shut_down(&host, &[&second_id]).await;
    assert_eq!(
        of_the_second,
        Ok(vec!["back".to_string(), "desk".to_string()]),
        "a list asked for the second mob names the second mob's agents"
    );
    assert_eq!(
        of_the_launch_mob,
        Ok(vec!["desk".to_string(), "front".to_string()]),
        "a list asked for the launch mob names the launch mob's agents"
    );
}

/// An archive of a second mob's agent archives that agent alone: the
/// launch mob's agent of the same name stays as it was, and stays listed.
#[tokio::test]
async fn an_archive_of_a_second_mobs_agent_archives_that_agent_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, second_id) = two_mobs_under_one_identity_policy(&temp).await;
    let in_the_launch_mob = scout_created_in(&host, None).await;
    let in_the_second = scout_created_in(&host, Some(&second_id)).await;
    let archive = scout_changed_in(&host, Some(&second_id), "archive").await;
    let archived_in_the_second = scout_archived_in(&host, Some(&second_id)).await;
    let archived_in_the_launch_mob = scout_archived_in(&host, None).await;
    shut_down(&host, &[&second_id]).await;
    assert!(
        in_the_launch_mob.is_ok() && in_the_second.is_ok(),
        "an agent of one name is created in each mob: {:?} {:?}",
        in_the_launch_mob.as_ref().err(),
        in_the_second.as_ref().err()
    );
    assert!(
        archive.is_ok(),
        "the archive of the second mob's agent is carried out: {:?}",
        archive.as_ref().err()
    );
    assert_eq!(
        archived_in_the_second,
        Ok(Some(true)),
        "the second mob's agent is archived"
    );
    assert_eq!(
        archived_in_the_launch_mob,
        Ok(Some(false)),
        "the launch mob's agent of the same name is not archived"
    );
}

/// An unarchive of a second mob's archived agent brings that agent back
/// alone: the launch mob's agent of the same name, archived too, stays
/// archived.
#[tokio::test]
async fn an_unarchive_of_a_second_mobs_agent_brings_that_agent_back_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, second_id) = two_mobs_under_one_identity_policy(&temp).await;
    let created = (
        scout_created_in(&host, None).await,
        scout_created_in(&host, Some(&second_id)).await,
    );
    let archived = (
        scout_changed_in(&host, None, "archive").await,
        scout_changed_in(&host, Some(&second_id), "archive").await,
    );
    let unarchive = scout_changed_in(&host, Some(&second_id), "unarchive").await;
    let archived_in_the_second = scout_archived_in(&host, Some(&second_id)).await;
    let archived_in_the_launch_mob = scout_archived_in(&host, None).await;
    shut_down(&host, &[&second_id]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok() && archived.0.is_ok() && archived.1.is_ok(),
        "an agent of one name is created and archived in each mob: {created:?} {archived:?}"
    );
    assert!(
        unarchive.is_ok(),
        "the unarchive of the second mob's agent is carried out: {:?}",
        unarchive.as_ref().err()
    );
    assert_eq!(
        archived_in_the_second,
        Ok(Some(false)),
        "the second mob's agent is no longer archived"
    );
    assert_eq!(
        archived_in_the_launch_mob,
        Ok(Some(true)),
        "the launch mob's agent of the same name stays archived"
    );
}

/// A removal of a second mob's agent removes that agent alone: the launch
/// mob's agent of the same name is still listed.
#[tokio::test]
async fn a_removal_of_a_second_mobs_agent_removes_that_agent_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, second_id) = two_mobs_under_one_identity_policy(&temp).await;
    let created = (
        scout_created_in(&host, None).await,
        scout_created_in(&host, Some(&second_id)).await,
    );
    let removal = scout_changed_in(&host, Some(&second_id), "remove").await;
    let of_the_second = listed_in(&host, Some(&second_id)).await;
    let of_the_launch_mob = listed_in(&host, None).await;
    shut_down(&host, &[&second_id]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok(),
        "an agent of one name is created in each mob: {created:?}"
    );
    assert!(
        removal.is_ok(),
        "the removal of the second mob's agent is carried out: {:?}",
        removal.as_ref().err()
    );
    assert_eq!(
        of_the_second,
        Ok(vec!["back".to_string(), "desk".to_string()]),
        "the second mob no longer lists the removed agent"
    );
    assert_eq!(
        of_the_launch_mob,
        Ok(vec![
            "desk".to_string(),
            "front".to_string(),
            SCOUT.to_string()
        ]),
        "the launch mob still lists its agent of the same name"
    );
}

/// A second mob of a launch that attached no identity policy serves the
/// family's reads and refuses its changes, as the launch mob of such a
/// launch does.
#[tokio::test]
async fn a_second_mob_of_a_launch_without_an_identity_policy_serves_reads_and_refuses_changes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, second_id) = two_mobs(&temp, None).await;
    let of_the_second = listed_in(&host, Some(&second_id)).await;
    let create = scout_created_in(&host, Some(&second_id)).await;
    let after = listed_in(&host, Some(&second_id)).await;
    shut_down(&host, &[&second_id]).await;
    assert_eq!(
        of_the_second,
        Ok(vec!["back".to_string(), "desk".to_string()]),
        "a list asked for the second mob is served"
    );
    assert!(
        create.is_err(),
        "a create asked for the second mob is refused: {create:?}"
    );
    assert_eq!(
        after, of_the_second,
        "nothing was created in the second mob"
    );
}

/// A policy the launch attaches after a second mob was composed reaches
/// that mob: its changes are served from then on.
#[tokio::test]
async fn a_policy_attached_after_a_second_mob_was_composed_reaches_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, second_id) = two_mobs(&temp, None).await;
    let before = scout_created_in(&host, Some(&second_id)).await;
    let attached = host.attach_identity_policy(
        the_host_changes_workers(),
        crate::identity_creation::IdentityFamilyOptions::new(),
    );
    let after = scout_created_in(&host, Some(&second_id)).await;
    let of_the_second = listed_in(&host, Some(&second_id)).await;
    shut_down(&host, &[&second_id]).await;
    assert!(
        before.is_err(),
        "before the policy a create is refused: {before:?}"
    );
    assert!(attached, "the launch attaches its identity policy");
    assert!(
        after.is_ok(),
        "after the policy the same create is carried out: {:?}",
        after.as_ref().err()
    );
    assert_eq!(
        of_the_second,
        Ok(vec![
            "back".to_string(),
            "desk".to_string(),
            SCOUT.to_string()
        ]),
        "the second mob lists the created agent"
    );
}

/// Send `text` to the agent [`SCOUT`] of the mob `mob_id` through that
/// mob's own identity runtime, and answer the transcript of the session its
/// member holds once it shows the text: `Err` says which step gave none.
async fn scout_given_a_turn(
    host: &UnifiedRuntime,
    sessions: &Sessions,
    mob_id: Option<&MobId>,
    text: &str,
) -> Result<String, String> {
    let mob = host
        .resolve_mob(mob_id)
        .await
        .map_err(|error| format!("the mob is not served: {error:?}"))?;
    let identities = mob
        .identity_runtime_ref()
        .cloned()
        .ok_or_else(|| "the mob has no identity runtime".to_string())?;
    let scout = crate::identity_first::AgentIdentity::parse(SCOUT).expect("identity");
    identities
        .send(&scout, &meerkat_core::ContentInput::Text(text.to_string()))
        .await
        .map_err(|error| format!("the send was refused: {error}"))?;
    let member = crate::identity_creation::core_member_name(SCOUT)
        .ok_or_else(|| "the agent's name is no member name".to_string())?;
    let session = mob
        .mob_runtime()
        .handle()
        .resolve_bridge_session_id(&member)
        .await
        .ok_or_else(|| "the agent's member holds no session".to_string())?;
    Ok(transcript_showing(sessions, &session, text).await)
}

/// The stdio host creates an agent of one name in each of two mobs, each
/// through the family of its own mob, and each takes a turn: a send to the
/// launch mob's agent runs in the launch mob's member and a send to the
/// second mob's agent in the second mob's, and neither sees the other's.
#[tokio::test]
async fn a_create_in_each_of_two_mobs_makes_an_agent_of_one_name_in_each_and_each_takes_a_turn() {
    const TO_THE_LAUNCH_MOB: &str = "a word for the scout of the launch mob";
    const TO_THE_SECOND: &str = "a word for the scout of the second mob";
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, sessions, second_id) =
        two_mobs_and_their_sessions(&temp, Some(the_host_changes_workers())).await;
    let created = (
        scout_created_in(&host, None).await,
        scout_created_in(&host, Some(&second_id)).await,
    );
    let of_the_launch_mob = scout_given_a_turn(&host, &sessions, None, TO_THE_LAUNCH_MOB).await;
    let of_the_second = scout_given_a_turn(&host, &sessions, Some(&second_id), TO_THE_SECOND).await;
    let members = (
        set_of(&host)
            .handle_for(host.mob_handle().mob_id())
            .await
            .is_ok(),
        set_of(&host).handle_for(&second_id).await.is_ok(),
    );
    shut_down(&host, &[&second_id]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok(),
        "an agent of one name is created in each mob: {created:?}"
    );
    assert!(members.0 && members.1, "both mobs are served by the set");
    assert!(
        of_the_launch_mob
            .as_ref()
            .is_ok_and(|turn| turn.contains(TO_THE_LAUNCH_MOB) && !turn.contains(TO_THE_SECOND)),
        "the launch mob's agent took its own turn alone: {of_the_launch_mob:?}"
    );
    assert!(
        of_the_second
            .as_ref()
            .is_ok_and(|turn| turn.contains(TO_THE_SECOND) && !turn.contains(TO_THE_LAUNCH_MOB)),
        "the second mob's agent took its own turn alone: {of_the_second:?}"
    );
}

// ---- The Console's door ----

/// The secret the rows' console signs its sign-in tokens with.
const SIGNING_SECRET: &str = "family-rows-signing";
/// An administrator of the rows' access configuration.
const ADMIN: &str = "root@example.test";
/// A person one access rule names: an allow that selects the second mob.
const OF_THE_SECOND_MOB: &str = "second@example.test";
/// A person one access rule names: an allow that names no mob, which
/// grants in the launch mob only.
const OF_THE_LAUNCH_MOB: &str = "launch@example.test";
/// An id no mob of the rows' host has.
const ABSENT_MOB: &str = "family-rows-absent-mob";

/// The reference application of `host` as a console that requires a
/// sign-in and has access rules: one administrator, one person who may
/// view and create agents in the mob `second_id`, and one who may do so in
/// the launch mob. Its sign-in names shared accounts, as a shared secret
/// does unless its configuration says otherwise: a caller is the host on
/// its console surface, and no verified person.
fn console_with_access(host: &mut UnifiedRuntime, second_id: &MobId) -> axum::Router {
    console_whose_sign_in_names(host, second_id, "shared")
}

/// [`console_with_access`] whose sign-in names persons: a caller is the
/// verified person its token names.
fn console_of_persons_with_access(host: &mut UnifiedRuntime, second_id: &MobId) -> axum::Router {
    console_whose_sign_in_names(host, second_id, "personal")
}

/// The console of the rows, whose sign-in's `accounts` are `accounts`
/// (`"shared"` or `"personal"`).
fn console_whose_sign_in_names(
    host: &mut UnifiedRuntime,
    second_id: &MobId,
    accounts: &str,
) -> axum::Router {
    let views_and_creates = || {
        vec![
            crate::access::ACTION_AGENT_VIEW.to_string(),
            crate::access::ACTION_AGENT_IDENTITY_CREATE.to_string(),
        ]
    };
    let controller = crate::access::AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![
            crate::access::AccessRule {
                id: "works-in-the-second-mob".to_string(),
                subjects: vec![OF_THE_SECOND_MOB.to_string()],
                actions: views_and_creates(),
                mobs: vec![second_id.to_string()],
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "works-in-the-launch-mob".to_string(),
                subjects: vec![OF_THE_LAUNCH_MOB.to_string()],
                actions: views_and_creates(),
                ..crate::access::AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    host.set_access_controller(controller);
    let decisions = crate::console_auth_config::parse_console_auth_config(&serde_json::json!({
        "shared_secret": SIGNING_SECRET,
        "accounts": accounts,
        "email_allowlist": [ADMIN, OF_THE_SECOND_MOB, OF_THE_LAUNCH_MOB],
    }))
    .expect("console sign-in configuration");
    host.build_reference_app_router(decisions)
}

/// One request of the Console's RPC door as the signed-in person `email`:
/// the whole JSON answer, or `Null` when the door answered no JSON.
async fn console_rpc(
    app: &axum::Router,
    email: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    use tower::ServiceExt as _;

    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &serde_json::json!({
            "iss": "http://127.0.0.1/meta-gateway",
            "aud": "persistent-gateway",
            "sub": email,
            "email": email,
            "exp": chrono::Utc::now().timestamp() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(SIGNING_SECRET.as_bytes()),
    )
    .expect("token");
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .body(axum::body::Body::from(body.to_string()))
        .expect("POST request");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        app.clone().oneshot(request),
    )
    .await
    .expect("the door answers in time")
    .expect("the door answers");
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("the answer's body");
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// The Console creates an agent in the mob its request names: a create
/// that names the second mob makes the agent there and not in the launch
/// mob, and a create that names no mob makes one in the launch mob and not
/// in the second.
#[tokio::test]
async fn the_console_creates_an_agent_in_the_mob_its_request_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, second_id) = two_mobs_under_one_identity_policy(&temp).await;
    let app = console_with_access(&mut host, &second_id);
    let scout = |request_id: &str| {
        serde_json::json!({
            "request_id": request_id,
            "agents": [{"name": SCOUT, "profile": "worker"}],
        })
    };
    let mut for_the_second = scout("console-create-in-the-second");
    for_the_second["mob_id"] = serde_json::json!(second_id.to_string());
    let in_the_second = console_rpc(&app, ADMIN, "meta/identity/create", for_the_second).await;
    let of_the_second = listed_in(&host, Some(&second_id)).await;
    let of_the_launch_mob = listed_in(&host, None).await;
    let in_the_launch_mob = console_rpc(
        &app,
        ADMIN,
        "meta/identity/create",
        serde_json::json!({
            "request_id": "console-create-in-the-launch-mob",
            "agents": [{"name": "runner", "profile": "worker"}],
        }),
    )
    .await;
    let of_the_second_after = listed_in(&host, Some(&second_id)).await;
    let of_the_launch_mob_after = listed_in(&host, None).await;
    shut_down(&host, &[&second_id]).await;
    assert!(
        in_the_second["error"].is_null() && in_the_second["result"]["outcome"] == "done",
        "the create that names the second mob is carried out: {in_the_second}"
    );
    assert_eq!(
        of_the_second,
        Ok(vec![
            "back".to_string(),
            "desk".to_string(),
            SCOUT.to_string()
        ]),
        "the agent is created in the second mob"
    );
    assert_eq!(
        of_the_launch_mob,
        Ok(vec!["desk".to_string(), "front".to_string()]),
        "and not in the launch mob"
    );
    assert!(
        in_the_launch_mob["error"].is_null() && in_the_launch_mob["result"]["outcome"] == "done",
        "the create that names no mob is carried out: {in_the_launch_mob}"
    );
    assert_eq!(
        of_the_launch_mob_after,
        Ok(vec![
            "desk".to_string(),
            "front".to_string(),
            "runner".to_string()
        ]),
        "the agent of a create that names no mob is created in the launch mob"
    );
    assert_eq!(
        of_the_second_after, of_the_second,
        "and not in the second mob"
    );
}

/// A Console caller who may not address a second mob learns nothing of its
/// agents: every method of the family that names that mob is answered as
/// one that names an id no mob has, with the id the request named and
/// nothing else. A caller who may address the mob is answered its agents.
#[tokio::test]
async fn a_console_caller_who_may_not_address_a_second_mob_learns_nothing_of_its_agents() {
    use crate::rpc::identity_methods::IDENTITY_FAMILY_METHODS;

    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, second_id) = two_mobs_under_one_identity_policy(&temp).await;
    let app = console_with_access(&mut host, &second_id);
    let second = second_id.to_string();
    let mut wrong = Vec::new();
    for method in IDENTITY_FAMILY_METHODS {
        let of_the_second = console_rpc(
            &app,
            OF_THE_LAUNCH_MOB,
            method,
            serde_json::json!({"mob_id": second, "name": "desk"}),
        )
        .await;
        let of_no_mob = console_rpc(
            &app,
            OF_THE_LAUNCH_MOB,
            method,
            serde_json::json!({"mob_id": ABSENT_MOB, "name": "desk"}),
        )
        .await;
        let refusal_of = |mob_id: &str| {
            serde_json::json!({
                "code": -32602,
                "message": format!(
                    "request targets mob '{mob_id}', which this gateway does not serve to the caller"
                ),
                "data": {"kind": "foreign_mob_target", "requested_mob_id": mob_id},
            })
        };
        if of_the_second["error"] != refusal_of(&second)
            || of_no_mob["error"] != refusal_of(ABSENT_MOB)
        {
            wrong.push(format!(
                "{method}: the second mob: {of_the_second}; an id no mob has: {of_no_mob}"
            ));
        }
    }
    // A caller whose rule selects the second mob reads an agent of it. (A
    // read that names no agent, a list for one, is judged with no
    // resource, which a rule that selects mobs never grants.)
    let to_its_own_person = console_rpc(
        &app,
        OF_THE_SECOND_MOB,
        "meta/identity/get",
        serde_json::json!({"mob_id": second, "name": "back"}),
    )
    .await;
    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "a caller who may not address the second mob is answered as for an id no mob has:\n{}",
        wrong.join("\n")
    );
    assert_eq!(
        to_its_own_person["result"]["name"],
        serde_json::json!("back"),
        "a caller who may address the second mob reads an agent of it: {to_its_own_person}"
    );
}

/// The kind of the family's refusal `answer` carries, when it is one.
fn refusal_kind(answer: &serde_json::Value) -> Option<&str> {
    answer["error"]["data"]["kind"].as_str()
}

/// The Console's methods of the family tell two mobs apart. An
/// administrator, a verified person, creates an agent in the second mob
/// alone, and then asks every method of the family the Console serves per
/// mob twice: a request that names the second mob is answered about that
/// mob's agent, and the same request that names no mob is answered that
/// the launch mob has no such agent, or with the launch mob's own answer.
#[tokio::test]
async fn the_consoles_methods_of_the_family_tell_two_mobs_apart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, second_id) = two_mobs_under_one_identity_policy(&temp).await;
    let app = console_of_persons_with_access(&mut host, &second_id);
    let second = second_id.to_string();
    // One request of the family as the administrator, for the second mob
    // or for no mob.
    let ask = |method: &'static str, for_the_second: bool, mut params: serde_json::Value| {
        if for_the_second {
            params["mob_id"] = serde_json::Value::String(second.clone());
        }
        console_rpc(&app, ADMIN, method, params)
    };
    let mut wrong = Vec::new();

    // create, list
    let created = ask(
        "meta/identity/create",
        true,
        serde_json::json!({
            "request_id": "family-create",
            "agents": [{"name": SCOUT, "profile": "worker"}],
        }),
    )
    .await;
    if created["result"]["outcome"] != "done" {
        wrong.push(format!("create for the second mob: {created}"));
    }
    let listed_for_the_second = ask("meta/identity/list", true, serde_json::json!({})).await;
    let listed_for_no_mob = ask("meta/identity/list", false, serde_json::json!({})).await;
    if !listed_names(&listed_for_the_second["result"]).contains(&SCOUT.to_string())
        || listed_names(&listed_for_no_mob["result"]).contains(&SCOUT.to_string())
    {
        wrong.push(format!(
            "list: the second mob {listed_for_the_second}; no mob {listed_for_no_mob}"
        ));
    }

    // operation, directory
    let operation = serde_json::json!({"request_id": "family-create"});
    let operation_for_the_second = ask("meta/identity/operation", true, operation.clone()).await;
    let operation_for_no_mob = ask("meta/identity/operation", false, operation).await;
    if operation_for_the_second["result"]["outcome"] != "done"
        || operation_for_no_mob["result"]["outcome"] != "unknown"
    {
        wrong.push(format!(
            "operation: the second mob {operation_for_the_second}; no mob {operation_for_no_mob}"
        ));
    }
    let directory_for_the_second =
        ask("meta/identity/directory", true, serde_json::json!({})).await["result"].to_string();
    let directory_for_no_mob =
        ask("meta/identity/directory", false, serde_json::json!({})).await["result"].to_string();
    if !directory_for_the_second.contains("\"back\"")
        || directory_for_the_second.contains("\"front\"")
        || !directory_for_no_mob.contains("\"front\"")
        || directory_for_no_mob.contains("\"back\"")
    {
        wrong.push(format!(
            "directory: the second mob {directory_for_the_second}; no mob {directory_for_no_mob}"
        ));
    }

    // changes_hold: held in the second mob, a create there is refused and
    // a create in the launch mob is carried out.
    let held = ask(
        "meta/identity/changes_hold",
        true,
        serde_json::json!({"held": true}),
    )
    .await;
    let runner = |request_id: &str| {
        serde_json::json!({
            "request_id": request_id,
            "agents": [{"name": "runner", "profile": "worker"}],
        })
    };
    let while_held_in_the_second =
        ask("meta/identity/create", true, runner("family-held-second")).await;
    let while_held_in_no_mob =
        ask("meta/identity/create", false, runner("family-held-launch")).await;
    let released = ask(
        "meta/identity/changes_hold",
        true,
        serde_json::json!({"held": false}),
    )
    .await;
    if held["result"]["held"] != true
        || refusal_kind(&while_held_in_the_second) != Some("changes_held")
        || while_held_in_no_mob["result"]["outcome"] != "done"
        || released["result"]["held"] != false
    {
        wrong.push(format!(
            "changes_hold: {held}; a create in the second mob {while_held_in_the_second}; a \
             create in no mob {while_held_in_no_mob}; {released}"
        ));
    }

    // The methods that name one agent: asked about an agent only the
    // second mob has, with the digest a get of the second mob answers.
    // For the second mob each is answered about that agent; for no mob
    // each is answered that no agent has the name.
    for (method, name, more) in [
        ("meta/identity/get", SCOUT, serde_json::json!({})),
        (
            "meta/identity/plan",
            SCOUT,
            serde_json::json!({"change": "archive"}),
        ),
        (
            "meta/identity/update",
            SCOUT,
            serde_json::json!({"set": {"display_name": "Scout"}}),
        ),
        ("meta/identity/archive", SCOUT, serde_json::json!({})),
        ("meta/identity/unarchive", SCOUT, serde_json::json!({})),
        ("meta/identity/adopt", "back", serde_json::json!({})),
        ("meta/identity/remove", SCOUT, serde_json::json!({})),
    ] {
        let read = ask("meta/identity/get", true, serde_json::json!({"name": name})).await;
        let mut params = more;
        params["name"] = serde_json::json!(name);
        if method != "meta/identity/get" {
            params["request_id"] = serde_json::json!(format!("family-{method}"));
            params["expected_spec_digest"] = read["result"]["spec_digest"].clone();
        }
        let for_no_mob = ask(method, false, params.clone()).await;
        let for_the_second = ask(method, true, params).await;
        let answered_about_the_agent = !for_the_second["result"].is_null()
            || refusal_kind(&for_the_second).is_some_and(|kind| kind != "identity_unknown");
        if !answered_about_the_agent || refusal_kind(&for_no_mob) != Some("identity_unknown") {
            wrong.push(format!(
                "{method} of {name}: the second mob {for_the_second}; no mob {for_no_mob}"
            ));
        }
    }
    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "a method of the family does not tell the two mobs apart:\n{}",
        wrong.join("\n")
    );
}

// ---- An agent's own identity tools ----

/// The identity tools the catalog of the session of the agent `name` of
/// the mob `mob_id` holds, sorted, whether the next model request shows
/// them or keeps them behind the deferred catalog: `Err` says which step
/// gave none.
async fn identity_tools_offered_to(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
    name: &str,
) -> Result<Vec<String>, String> {
    let mob = host
        .resolve_mob(mob_id)
        .await
        .map_err(|error| format!("the mob is not served: {error:?}"))?;
    let identities = mob
        .identity_runtime_ref()
        .cloned()
        .ok_or_else(|| "the mob has no identity runtime".to_string())?;
    let agent = crate::identity_first::AgentIdentity::parse(name).expect("identity");
    let session = Box::pin(identities.materialize(&agent))
        .await
        .map_err(|error| format!("the agent does not wake: {error}"))?
        .map(|scope| scope.session_id().clone())
        .ok_or_else(|| "the agent wakes in no session".to_string())?;
    let sessions = host
        .mob_runtime()
        .session_service()
        .ok_or_else(|| "the launch has no session service".to_string())?;
    let catalog = sessions
        .tool_catalog_view(&session)
        .await
        .map_err(|error| format!("the session's catalog is not read: {error}"))?
        .ok_or_else(|| "the session reports no catalog".to_string())?;
    let names: Vec<String> = catalog
        .entries
        .into_iter()
        .map(|entry| entry.name.into_string())
        .collect();
    let mut tools: Vec<String> = names
        .iter()
        .filter(|tool| crate::identity_creation::is_identity_tool(tool))
        .cloned()
        .collect();
    if tools.is_empty() {
        return Err(format!("the catalog holds no identity tool: {names:?}"));
    }
    tools.sort();
    Ok(tools)
}

/// An agent of a second mob is offered the identity tools of its own mob,
/// as an agent of the launch mob is offered the launch mob's: all eight,
/// under the launch's one identity policy.
#[tokio::test]
async fn an_agent_of_a_second_mob_is_offered_the_identity_tools_of_its_own_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, second_id) = two_mobs_under_one_identity_policy(&temp).await;
    let in_the_launch_mob = identity_tools_offered_to(&host, None, "front").await;
    let in_the_second = identity_tools_offered_to(&host, Some(&second_id), "back").await;
    shut_down(&host, &[&second_id]).await;
    let mut all: Vec<String> = crate::identity_creation::IDENTITY_TOOLS
        .iter()
        .map(ToString::to_string)
        .collect();
    all.sort();
    assert_eq!(
        in_the_launch_mob,
        Ok(all.clone()),
        "an agent of the launch mob is offered the identity tools"
    );
    assert_eq!(
        in_the_second,
        Ok(all),
        "an agent of the second mob is offered the identity tools of its own mob"
    );
}
// ---- Changes a person decides ----

/// The approver id the rows' policy names for a removal.
const APPROVERS: &str = "team:owners";
/// The chat person the rows' approver map lets decide for [`APPROVERS`].
const OWNER: &str = "U123";

/// The policy of the rows about held changes: the host creates an agent of
/// the profile `worker` at once, and a person decides every removal of
/// one, whether the host or a signed-in person asks it.
fn a_person_decides_removals() -> Arc<crate::identity_creation::IdentityCreationPolicy> {
    let decided = serde_json::json!({"kind": "require_approval", "approver": {"id": APPROVERS}});
    Arc::new(
        crate::identity_creation::IdentityCreationPolicy::parse(&serde_json::json!({
            "enabled": true,
            "approval": {"rules": [
                {"change": ["create"], "requester": {"kind": "host"},
                 "target": {"profile": ["worker"]}, "outcome": {"kind": "allow"}},
                {"change": ["remove"], "requester": {"kind": "host"},
                 "target": {"profile": ["worker"]}, "outcome": decided},
                {"change": ["remove"], "requester": {"kind": "human"},
                 "target": {"profile": ["worker"]}, "outcome": decided},
            ]},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled"),
    )
}

/// A launch that keeps durable decisions in `temp`, whose approver map lets
/// the chat person [`OWNER`] decide for [`APPROVERS`], under the policy
/// [`a_person_decides_removals`]: its launch mob holds `desk` and `front`,
/// and a second mob of it, made from the definition answered with it,
/// holds `desk` and `back`. With them, the launch's roster provider.
async fn two_mobs_whose_removals_a_person_decides(
    temp: &tempfile::TempDir,
) -> (
    UnifiedRuntime,
    Arc<LaunchRoster>,
    meerkat_mob::MobDefinition,
    MobId,
) {
    let (mut host, _sessions) = launch_with_identities(temp).await;
    removals_decided_by_a_person(&mut host, temp.path());
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(&roster)));
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&host, of_the_second.clone()).await;
    (host, roster, of_the_second, second_id)
}

/// `host` keeps durable decisions in the directory `kept`, its approver map
/// lets the chat person [`OWNER`] decide for [`APPROVERS`], and it attaches
/// the policy [`a_person_decides_removals`]. A launch over a directory an
/// earlier launch kept its decisions in reads that launch's decisions.
fn removals_decided_by_a_person(host: &mut UnifiedRuntime, kept: &std::path::Path) {
    decisions_kept_under(host, kept, a_person_decides_removals());
}

/// `host` keeps durable decisions in the directory `kept`, its approver map
/// lets the chat person [`OWNER`] decide for [`APPROVERS`], and it attaches
/// the identity policy `policy`.
fn decisions_kept_under(
    host: &mut UnifiedRuntime,
    kept: &std::path::Path,
    policy: Arc<crate::identity_creation::IdentityCreationPolicy>,
) {
    decisions_kept_in(host, kept);
    assert!(
        host.attach_identity_policy(
            policy,
            crate::identity_creation::IdentityFamilyOptions::new()
        ),
        "the launch attaches its identity policy"
    );
}

/// `host` keeps durable decisions in the directory `kept`, and its approver
/// map lets the chat person [`OWNER`] decide for [`APPROVERS`]. It attaches
/// no identity policy.
fn decisions_kept_in(host: &mut UnifiedRuntime, kept: &std::path::Path) {
    let access: crate::AccessControlConfig = toml::from_str(&format!(
        "enabled = false\n\n[approvers.\"{APPROVERS}\"]\nsubjects = [\"owner@example.org\"]\n\
         ingress_subjects = [{{ ingress = \"chat\", subject = \"{OWNER}\" }}]\n"
    ))
    .expect("the approver map");
    host.set_access_controller(crate::access::AccessController::new(access).expect("controller"));
    let ingresses: crate::DecisionIngressDeclarations =
        serde_json::from_value(serde_json::json!({"ingresses": [
            {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
        ]}))
        .expect("ingress declarations");
    host.set_durable_decisions(crate::RuntimeDecisions::in_state_dir(
        kept,
        meerkat_core::RealmId::parse("team").expect("realm"),
        ingresses,
        Arc::new(
            crate::runtime::SqliteMetadataStore::open(kept.join("metadata.sqlite3"))
                .expect("metadata store"),
        ),
        "family-rows",
        true,
    ));
}

/// The decision that holds the change `asked` answered, when it answered
/// that a person decides it; else why it holds none.
fn held_by(asked: &Result<serde_json::Value, String>) -> Result<String, String> {
    let answer = asked.as_ref().map_err(Clone::clone)?;
    if answer["outcome"] != "pending_decision" {
        return Err(format!("the change is not held: {answer}"));
    }
    answer["decision_id"]
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| format!("the held change names no decision: {answer}"))
}

/// The row of the decision `decision_id` in the host's decisions.
fn decision_row(
    host: &UnifiedRuntime,
    decision_id: &str,
) -> Result<meerkat_core::approval::decision::StoredDecision, String> {
    let decision_id = meerkat_core::approval::decision::DecisionId::new(decision_id)
        .map_err(|error| format!("no decision id: {error}"))?;
    host.durable_decisions()
        .owner()
        .get(&decision_id)
        .map_err(|error| format!("the decision is not read: {error}"))
}

/// The phase of the decision `decision_id`, as its row reads now.
fn phase_of(
    host: &UnifiedRuntime,
    decision_id: &str,
) -> Result<meerkat_core::approval::decision::DecisionPhase, String> {
    decision_row(host, decision_id).map(|row| row.state.phase)
}

/// The phase of the decision `decision_id` once it is one of `wanted`, or
/// the phase it still reads after ten seconds.
async fn phase_once(
    host: &UnifiedRuntime,
    decision_id: &str,
    wanted: &[meerkat_core::approval::decision::DecisionPhase],
) -> Result<meerkat_core::approval::decision::DecisionPhase, String> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let phase = phase_of(host, decision_id)?;
        if wanted.contains(&phase) || tokio::time::Instant::now() >= deadline {
            return Ok(phase);
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// The chat person [`OWNER`] approves the decision `decision_id`: the
/// decision is presented to its approvers, and the person's decide is
/// attested by the declared ingress.
async fn approved(host: &UnifiedRuntime, decision_id: &str) -> Result<(), String> {
    use meerkat_core::approval::decision::{
        ApproverId, DecisionReference, PresentationAudience, PresentationRecord,
    };

    struct NoNotices;
    #[async_trait::async_trait]
    impl crate::DecisionNoticeSink for NoNotices {
        async fn deliver(&self, _line: String) -> bool {
            true
        }
    }

    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis(),
    )
    .map_err(|error| error.to_string())?;
    let row = decision_row(host, decision_id)?;
    host.durable_decisions()
        .owner()
        .present(
            &row.decision_id,
            &PresentationRecord::complete(
                PresentationAudience::Approver {
                    approver_id: ApproverId::new(APPROVERS).map_err(|error| error.to_string())?,
                },
                &row.descriptor.rendering,
                None,
                DecisionReference::new("chat:owners").map_err(|error| error.to_string())?,
                DecisionReference::new(format!("message-{now_ms}"))
                    .map_err(|error| error.to_string())?,
                now_ms,
            ),
        )
        .map_err(|error| format!("the decision is not presented: {error}"))?;
    let connection = crate::StdioDecisionConnection::new(
        host.durable_decisions().realm(),
        Arc::new(NoNotices) as Arc<dyn crate::DecisionNoticeSink>,
    );
    let line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "decide",
        "method": "meta/decisions/decide",
        "params": {
            "decision_id": decision_id,
            "approve": true,
            "presented_digest": row.descriptor.subject_digest.to_string(),
            "attestation": {
                "ingress": "chat",
                "subject": OWNER,
                "scope": "chat-session-1",
                "event_ref": format!("chat:event:{decision_id}"),
                "observed_at_ms": now_ms,
            },
        },
    })
    .to_string();
    let answer: serde_json::Value = serde_json::from_str(
        &connection
            .handle_line(host, &line)
            .await
            .ok_or_else(|| "the decide is no decision method".to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if answer["error"].is_null() {
        Ok(())
    } else {
        Err(format!("the decide is refused: {answer}"))
    }
}

/// The id of the decision of the kind `identity.remove` filed under
/// `subject_key`.
fn removal_filed_under(subject_key: String) -> String {
    use meerkat_core::approval::decision::{DecisionId, DecisionKind, DecisionSubjectKey};

    DecisionId::derive(
        &DecisionKind::new("identity.remove").expect("a decision kind"),
        &DecisionSubjectKey::new(subject_key).expect("a subject key"),
    )
    .to_string()
}

/// A removal of a second mob's agent that a person decides is held, and
/// once the person approved it, it is carried out in that mob: its agent
/// is gone, and the launch mob's agent of the same name stays.
#[tokio::test]
async fn a_held_change_of_a_second_mob_is_finished_by_its_own_mob() {
    use meerkat_core::approval::decision::DecisionPhase;

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _roster, _definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let in_the_launch_mob = scout_created_in(&host, None).await;
    let in_the_second = scout_created_in(&host, Some(&second_id)).await;
    let held = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let while_held = listed_in(&host, Some(&second_id)).await;
    let ended = match &held {
        Ok(decision_id) => match approved(&host, decision_id).await {
            Ok(()) => {
                phase_once(
                    &host,
                    decision_id,
                    &[
                        DecisionPhase::Executed,
                        DecisionPhase::ExecutionRefused,
                        DecisionPhase::Expired,
                        DecisionPhase::Withdrawn,
                    ],
                )
                .await
            }
            Err(error) => Err(error),
        },
        Err(error) => Err(error.clone()),
    };
    let of_the_second = listed_in(&host, Some(&second_id)).await;
    let of_the_launch_mob = listed_in(&host, None).await;
    shut_down(&host, &[&second_id]).await;
    assert!(
        in_the_launch_mob.is_ok() && in_the_second.is_ok(),
        "an agent of one name is created in each mob: {:?} {:?}",
        in_the_launch_mob.as_ref().err(),
        in_the_second.as_ref().err()
    );
    assert!(
        held.is_ok(),
        "the removal of the second mob's agent is held for a person: {:?}",
        held.as_ref().err()
    );
    assert!(
        while_held
            .as_ref()
            .is_ok_and(|names| names.contains(&SCOUT.to_string())),
        "a held removal removes nothing: {while_held:?}"
    );
    assert_eq!(
        ended,
        Ok(DecisionPhase::Executed),
        "the approved removal is carried out"
    );
    assert_eq!(
        of_the_second,
        Ok(vec!["back".to_string(), "desk".to_string()]),
        "the second mob's agent is gone"
    );
    assert!(
        of_the_launch_mob
            .as_ref()
            .is_ok_and(|names| names.contains(&SCOUT.to_string())),
        "the launch mob's agent of the same name stays: {of_the_launch_mob:?}"
    );
}

/// The launch mob's held change is filed as it was before a host served
/// other mobs: under the mob's id and the operation.
#[tokio::test]
async fn the_launch_mobs_held_change_is_filed_as_before() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _roster, _definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, None).await;
    let asked = scout_changed_in(&host, None, "remove").await;
    let held = held_by(&asked);
    let launch_mob = host.mob_runtime().handle().mob_id().to_string();
    shut_down(&host, &[&second_id]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    let operation = asked
        .as_ref()
        .ok()
        .and_then(|answer| answer["operation_id"].as_str())
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        held,
        Ok(removal_filed_under(format!("{launch_mob}/{operation}"))),
        "the launch mob's held removal is filed under its mob's id and its operation"
    );
}

/// A second mob's held change is filed under that mob's incarnation: the
/// one minted when the mob was created.
#[tokio::test]
async fn a_second_mobs_held_change_is_filed_under_its_mobs_incarnation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _roster, _definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let asked = scout_changed_in(&host, Some(&second_id), "remove").await;
    let held = held_by(&asked);
    let incarnation = host
        .resolve_mob(Some(&second_id))
        .await
        .map(|mob| mob.mob_runtime().handle().incarnation().to_string())
        .map_err(|error| format!("{error:?}"));
    shut_down(&host, &[&second_id]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    let operation = asked
        .as_ref()
        .ok()
        .and_then(|answer| answer["operation_id"].as_str())
        .unwrap_or_default()
        .to_string();
    let incarnation = incarnation.expect("the second mob is served");
    assert_eq!(
        held,
        Ok(removal_filed_under(format!(
            "{second_id}@{incarnation}/{operation}"
        ))),
        "the second mob's held removal is filed under its mob's id, its incarnation and its \
         operation"
    );
}

/// A destroy of a mob withdraws the held changes of its agents and leaves
/// another mob's: the second mob's held removal ends withdrawn once the
/// mob is destroyed, and the launch mob's stays held.
#[tokio::test]
async fn a_destroy_of_a_mob_withdraws_its_held_changes_and_leaves_another_mobs() {
    use meerkat_core::approval::decision::DecisionPhase;

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _roster, _definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = (
        scout_created_in(&host, None).await,
        scout_created_in(&host, Some(&second_id)).await,
    );
    let of_the_launch_mob = held_by(&scout_changed_in(&host, None, "remove").await);
    let of_the_second = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    let after_the_destroy = match &of_the_second {
        Ok(decision_id) => phase_once(&host, decision_id, &[DecisionPhase::Withdrawn]).await,
        Err(error) => Err(error.clone()),
    };
    let of_the_launch_mob_after = match &of_the_launch_mob {
        Ok(decision_id) => phase_of(&host, decision_id),
        Err(error) => Err(error.clone()),
    };
    shut_down(&host, &[]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok(),
        "an agent of one name is created in each mob: {created:?}"
    );
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        after_the_destroy,
        Ok(DecisionPhase::Withdrawn),
        "the destroyed mob's held removal is withdrawn"
    );
    assert_eq!(
        of_the_launch_mob_after,
        Ok(DecisionPhase::Open),
        "the launch mob's held removal stays held"
    );
}

/// A create withdraws the held changes of an earlier mob of its id: a
/// removal held for a mob that was destroyed, and that no pass over the
/// host's mobs ended, is withdrawn by the create of a mob under the id.
#[tokio::test]
async fn a_create_withdraws_the_held_changes_of_an_earlier_mob_of_its_id() {
    use meerkat_core::approval::decision::DecisionPhase;

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _roster, definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let held = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    // No pass follows the set from here on, and the destroy asks the table
    // for none: the destroy ends nothing of the host's by itself.
    host.hold_the_follower_of_host_mobs_in_tests().await;
    host.destroys_of_host_mobs_ask_for_no_pass_in_tests();
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    let before_the_create = match &held {
        Ok(decision_id) => phase_of(&host, decision_id),
        Err(error) => Err(error.clone()),
    };
    let created_again = Box::pin(set_of(&host).mob_create_definition(definition)).await;
    let after_the_create = match &held {
        Ok(decision_id) => phase_of(&host, decision_id),
        Err(error) => Err(error.clone()),
    };
    shut_down(&host, &[&second_id]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        before_the_create,
        Ok(DecisionPhase::Open),
        "with no pass over the host's mobs the destroyed mob's removal is still held"
    );
    assert_eq!(
        created_again.as_ref().ok(),
        Some(&second_id),
        "a mob is created under the destroyed mob's id: {:?}",
        created_again.as_ref().err()
    );
    assert_eq!(
        after_the_create,
        Ok(DecisionPhase::Withdrawn),
        "the create withdrew the earlier mob's held removal"
    );
}

/// A change a person approved for an earlier mob of an id is not carried
/// out in a later mob of the id. The removal is approved while the mob
/// holds its changes, so it waits; the mob is destroyed and a mob is
/// created under its id with an agent of the same name: the approved
/// removal ends refused, and the later mob's agent stays.
#[tokio::test]
async fn a_change_approved_for_an_earlier_mob_of_an_id_is_not_carried_out_in_a_later_one() {
    use meerkat_core::approval::decision::DecisionPhase;

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _roster, definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let held = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let changes_held = asked_of_the_family(
        &host,
        Some(&second_id),
        "meta/identity/changes_hold",
        serde_json::json!({"held": true}),
    )
    .await;
    let approval = match &held {
        Ok(decision_id) => approved(&host, decision_id).await,
        Err(error) => Err(error.clone()),
    };
    // The approved removal is tried and waits: its mob holds its changes.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let while_the_mob_holds = match &held {
        Ok(decision_id) => phase_of(&host, decision_id),
        Err(error) => Err(error.clone()),
    };
    let still_there = listed_in(&host, Some(&second_id)).await;
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    let created_again = Box::pin(set_of(&host).mob_create_definition(definition)).await;
    let in_the_later_mob = scout_created_in(&host, Some(&second_id)).await;
    host.durable_decisions().notify_work();
    let ended = match &held {
        Ok(decision_id) => {
            phase_once(
                &host,
                decision_id,
                &[
                    DecisionPhase::Executed,
                    DecisionPhase::ExecutionRefused,
                    DecisionPhase::Expired,
                    DecisionPhase::Withdrawn,
                ],
            )
            .await
        }
        Err(error) => Err(error.clone()),
    };
    let refused_with = held.as_ref().ok().and_then(|decision_id| {
        decision_row(&host, decision_id).ok().and_then(|row| {
            row.execution_refusal
                .map(|refusal| refusal.as_str().to_string())
        })
    });
    let of_the_later_mob = listed_in(&host, Some(&second_id)).await;
    shut_down(&host, &[&second_id]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert!(
        held.is_ok(),
        "the removal is held: {:?}",
        held.as_ref().err()
    );
    assert!(
        changes_held.is_ok(),
        "the mob holds its changes: {changes_held:?}"
    );
    assert!(approval.is_ok(), "the removal is approved: {approval:?}");
    assert_eq!(
        while_the_mob_holds,
        Ok(DecisionPhase::Approved),
        "the approved removal waits while its mob holds its changes"
    );
    assert!(
        still_there
            .as_ref()
            .is_ok_and(|names| names.contains(&SCOUT.to_string())),
        "the agent is still there: {still_there:?}"
    );
    assert!(
        destroyed.is_ok() && created_again.as_ref().ok() == Some(&second_id),
        "the mob is destroyed and a mob is created under its id: {:?} {:?}",
        destroyed.as_ref().err(),
        created_again.as_ref().err()
    );
    assert!(
        in_the_later_mob.is_ok(),
        "an agent of the same name is created in the later mob: {in_the_later_mob:?}"
    );
    assert_eq!(
        (ended, refused_with.as_deref()),
        (Ok(DecisionPhase::ExecutionRefused), Some("mob_replaced")),
        "the removal approved for the earlier mob ends refused"
    );
    assert!(
        of_the_later_mob
            .as_ref()
            .is_ok_and(|names| names.contains(&SCOUT.to_string())),
        "the later mob's agent stays: {of_the_later_mob:?}"
    );
}

/// A later mob of an id reads no operation of an earlier one: after a mob
/// whose removal was held is destroyed and a mob is created under its id,
/// the operation the earlier mob's host asked reads as one nobody asked,
/// and the same request asked in the later mob is held anew, under
/// another decision.
#[tokio::test]
async fn a_later_mob_of_an_id_reads_no_operation_of_an_earlier_one() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _roster, definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let of_the_earlier_mob = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    let created_again = Box::pin(set_of(&host).mob_create_definition(definition)).await;
    let operation = asked_of_the_family(
        &host,
        Some(&second_id),
        "meta/identity/operation",
        serde_json::json!({"request_id": "remove-the-scout"}),
    )
    .await;
    let in_the_later_mob = scout_created_in(&host, Some(&second_id)).await;
    let of_the_later_mob = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    shut_down(&host, &[&second_id]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert!(
        of_the_earlier_mob.is_ok(),
        "the removal is held in the earlier mob: {:?}",
        of_the_earlier_mob.as_ref().err()
    );
    assert!(
        destroyed.is_ok() && created_again.as_ref().ok() == Some(&second_id),
        "the mob is destroyed and a mob is created under its id: {:?} {:?}",
        destroyed.as_ref().err(),
        created_again.as_ref().err()
    );
    assert_eq!(
        operation
            .as_ref()
            .map(|answer| answer["outcome"].as_str().unwrap_or_default().to_string()),
        Ok("unknown".to_string()),
        "the earlier mob's operation reads as one nobody asked: {operation:?}"
    );
    assert!(
        in_the_later_mob.is_ok(),
        "an agent of the same name is created in the later mob: {in_the_later_mob:?}"
    );
    assert!(
        of_the_later_mob.is_ok() && of_the_later_mob != of_the_earlier_mob,
        "the same request in the later mob is held under a decision of its own: \
         {of_the_later_mob:?} against {of_the_earlier_mob:?}"
    );
}
// ---- The Console's reads of held changes ----

/// The reference application of `host` as a console whose sign-in names
/// persons and whose access rules are enforced: both persons of the rows
/// hold the approver id [`APPROVERS`] and may read decisions; one of them
/// may address the mob `second_id`, the other no mob but the launch mob.
fn console_of_approvers(host: &mut UnifiedRuntime, second_id: &MobId) -> axum::Router {
    let mut access: crate::AccessControlConfig = toml::from_str(&format!(
        "enabled = true\nadmins = [\"{ADMIN}\"]\n\n[approvers.\"{APPROVERS}\"]\n\
         subjects = [\"{OF_THE_SECOND_MOB}\", \"{OF_THE_LAUNCH_MOB}\"]\n"
    ))
    .expect("the approver map");
    access.rules = vec![
        crate::access::AccessRule {
            id: "views-the-second-mob".to_string(),
            subjects: vec![OF_THE_SECOND_MOB.to_string()],
            actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
            mobs: vec![second_id.to_string()],
            ..crate::access::AccessRule::default()
        },
        crate::access::AccessRule {
            id: "reads-decisions".to_string(),
            subjects: vec![OF_THE_SECOND_MOB.to_string(), OF_THE_LAUNCH_MOB.to_string()],
            actions: vec![crate::access::ACTION_DECISIONS_VIEW.to_string()],
            ..crate::access::AccessRule::default()
        },
    ];
    host.set_access_controller(
        crate::access::AccessController::new(access).expect("access configuration"),
    );
    let decisions = crate::console_auth_config::parse_console_auth_config(&serde_json::json!({
        "shared_secret": SIGNING_SECRET,
        "accounts": "personal",
        "email_allowlist": [ADMIN, OF_THE_SECOND_MOB, OF_THE_LAUNCH_MOB],
    }))
    .expect("console sign-in configuration");
    host.build_reference_app_router(decisions)
}

/// The ids of the decisions a Console list answers `email`, whole or by
/// their status.
async fn decisions_listed_to(app: &axum::Router, email: &str) -> Vec<String> {
    let answer = console_rpc(app, email, "meta/decisions/list", serde_json::json!({})).await;
    let ids = |member: &str| -> Vec<String> {
        answer["result"][member]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row["decision_id"].as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut listed = ids("decisions");
    listed.extend(ids("statuses"));
    listed
}

/// A held change of a mob a Console caller may not address reads as not
/// there. Two persons hold the approver id of a second mob's held removal;
/// the one who may address that mob reads the decision, and to the one who
/// may not, a get answers as for an id no decision has and a list does not
/// name it.
#[tokio::test]
async fn a_held_change_of_a_mob_a_console_caller_may_not_address_reads_as_not_there() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _roster, _definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let held = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let app = console_of_approvers(&mut host, &second_id);
    let no_decision = removal_filed_under("no-such-mob/no-such-operation".to_string());
    let decision_id = held.clone().unwrap_or_else(|_| no_decision.clone());
    let get = |email: &'static str, decision_id: &str| {
        console_rpc(
            &app,
            email,
            "meta/decisions/get",
            serde_json::json!({"decision_id": decision_id}),
        )
    };
    let to_the_mobs_approver = get(OF_THE_SECOND_MOB, &decision_id).await;
    let to_another_approver = get(OF_THE_LAUNCH_MOB, &decision_id).await;
    let about_no_decision = get(OF_THE_LAUNCH_MOB, &no_decision).await;
    let listed_to_the_mobs_approver = decisions_listed_to(&app, OF_THE_SECOND_MOB).await;
    let listed_to_another_approver = decisions_listed_to(&app, OF_THE_LAUNCH_MOB).await;
    shut_down(&host, &[&second_id]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert!(
        held.is_ok(),
        "the removal is held: {:?}",
        held.as_ref().err()
    );
    assert_eq!(
        to_the_mobs_approver["result"]["decision"]["decision_id"],
        serde_json::json!(decision_id),
        "an approver who may address the mob reads its held removal: {to_the_mobs_approver}"
    );
    assert!(
        listed_to_the_mobs_approver.contains(&decision_id),
        "and is listed it: {listed_to_the_mobs_approver:?}"
    );
    let apart_from_the_id =
        |answer: &serde_json::Value, id: &str| answer["error"].to_string().replace(id, "ID");
    assert!(
        to_another_approver["result"].is_null()
            && !to_another_approver["error"].is_null()
            && apart_from_the_id(&to_another_approver, &decision_id)
                == apart_from_the_id(&about_no_decision, &no_decision),
        "an approver who may not address the mob is answered as for an id no decision has: \
         {to_another_approver} against {about_no_decision}"
    );
    assert!(
        !listed_to_another_approver.contains(&decision_id),
        "and is not listed it: {listed_to_another_approver:?}"
    );
}

/// The Console's reads of a held change of a mob beside the launch mob, as
/// a caller who addresses that mob by an access rule is answered.
#[path = "console_decisions_standing_mob_tests.rs"]
mod console_decisions_standing_mob;
// ---- A restart, an unload and a load ----

/// A launch over the state `stored` keeps, as a gateway's launch over its
/// state directory is: the launch mob has its identity context and holds
/// the agents `desk` and `front`, the launch attached the rows' policy,
/// and the mobs it hosts beside the launch mob get their identity planes
/// from the gateways' composer over `roster`. With it, the launch's
/// session service.
async fn stored_launch_with_identities(
    stored: &super::mob_entries::lazy_start_tests::StoredHost,
    roster: &Arc<LaunchRoster>,
) -> (UnifiedRuntime, Sessions) {
    stored_launch_under(stored, roster, the_host_changes_workers()).await
}

/// [`stored_launch_with_identities`], under the identity policy `policy`.
async fn stored_launch_under(
    stored: &super::mob_entries::lazy_start_tests::StoredHost,
    roster: &Arc<LaunchRoster>,
    policy: Arc<crate::identity_creation::IdentityCreationPolicy>,
) -> (UnifiedRuntime, Sessions) {
    let (mut host, sessions, _machine, ()) = stored.launch_over(|_| ()).await;
    let of_the_launch_mob = identity_context(host.mob_runtime(), "default");
    Box::pin(host.install_and_bootstrap_identity_first_context(
        of_the_launch_mob,
        &[configured_agent("desk"), configured_agent("front")],
    ))
    .await
    .expect("the launch mob's identity bootstrap");
    assert!(
        host.attach_identity_policy(
            policy,
            crate::identity_creation::IdentityFamilyOptions::new()
        ),
        "the launch attaches its identity policy"
    );
    host.compose_host_mob_identities(Arc::new(gateways_composer(roster)));
    (host, sessions)
}

/// Whether `host`'s mob set knows the mob `mob_id` and has not loaded it.
async fn not_loaded(host: &UnifiedRuntime, mob_id: &MobId) -> bool {
    let known = super::mob_entries::lazy_start_tests::known(host).await;
    known.not_loaded.contains(mob_id) && !known.loaded.contains(mob_id)
}

/// A restart keeps the agents created in each of two mobs. An agent of one
/// name is created in the launch mob and in a second mob; the host is shut
/// down and launched again over the same state. The second mob is not
/// loaded by the launch, by its policy or by its driver; the first request
/// of the family that names it loads it and is served by its own family:
/// each mob still lists its agent, the second mob's agent is changed
/// through the family, and it takes a turn.
#[tokio::test]
async fn a_restart_keeps_the_agents_created_in_each_of_two_mobs() {
    const AFTER_THE_RESTART: &str = "a word for the scout after the restart";
    let stored = super::mob_entries::lazy_start_tests::StoredHost::new();
    let roster = Arc::new(LaunchRoster::default());
    let (first, _sessions) = stored_launch_with_identities(&stored, &roster).await;
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&first, of_the_second).await;
    let created = (
        scout_created_in(&first, None).await,
        scout_created_in(&first, Some(&second_id)).await,
    );
    shut_down(&first, &[]).await;
    drop(first);

    let (host, sessions) = stored_launch_with_identities(&stored, &roster).await;
    // The launch, its policy and its driver's first pass had their time.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let asleep_after_the_launch = not_loaded(&host, &second_id).await;
    let of_the_launch_mob = listed_in(&host, None).await;
    let of_the_second = listed_in(&host, Some(&second_id)).await;
    let loaded_by_the_request = !not_loaded(&host, &second_id).await;
    let archived = scout_changed_in(&host, Some(&second_id), "archive").await;
    let unarchived = scout_changed_in(&host, Some(&second_id), "unarchive").await;
    let turn = scout_given_a_turn(&host, &sessions, Some(&second_id), AFTER_THE_RESTART).await;
    shut_down(&host, &[]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok(),
        "an agent of one name is created in each mob: {created:?}"
    );
    assert!(
        asleep_after_the_launch,
        "nothing of the family loads the second mob before a request names it"
    );
    assert_eq!(
        of_the_launch_mob,
        Ok(vec![
            "desk".to_string(),
            "front".to_string(),
            SCOUT.to_string()
        ]),
        "the launch mob keeps its created agent over the restart"
    );
    assert_eq!(
        of_the_second,
        Ok(vec![
            "back".to_string(),
            "desk".to_string(),
            SCOUT.to_string()
        ]),
        "the second mob keeps its created agent over the restart"
    );
    assert!(
        loaded_by_the_request,
        "the request of the family that names the second mob loads it"
    );
    assert!(
        archived.is_ok() && unarchived.is_ok(),
        "the restored second mob's agent is changed through its own family: {:?} {:?}",
        archived.as_ref().err(),
        unarchived.as_ref().err()
    );
    assert!(
        turn.as_ref()
            .is_ok_and(|transcript| transcript.contains(AFTER_THE_RESTART)),
        "the restored second mob's agent takes a turn: {turn:?}"
    );
}

/// An unload and a load of a second mob keep the agents created in it. The
/// mob set unloads the second mob; the next request of the family that
/// names it loads it again with its own family: it lists its created
/// agent, the agent is changed through the family, and it takes a turn.
#[tokio::test]
async fn an_unload_and_a_load_of_a_second_mob_keep_the_agents_created_in_it() {
    const AFTER_THE_LOAD: &str = "a word for the scout after the load";
    let stored = super::mob_entries::lazy_start_tests::StoredHost::new();
    let roster = Arc::new(LaunchRoster::default());
    let (host, sessions) = stored_launch_with_identities(&stored, &roster).await;
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&host, of_the_second).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let before_the_unload = listed_in(&host, Some(&second_id)).await;
    // The mob set unloads a mob once it is idle: the seats of its agents
    // settle first.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut unloaded = set_of(&host).unload_idle_mobs_for_tests().await;
    let mut asleep = not_loaded(&host, &second_id).await;
    while !asleep && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        unloaded = set_of(&host).unload_idle_mobs_for_tests().await;
        asleep = not_loaded(&host, &second_id).await;
    }
    // What keeps a mob that did not unload: asked only then, of a mob that
    // is busy, which the question leaves untouched.
    let kept_by = if asleep {
        None
    } else {
        match set_of(&host).handle_for(&second_id).await {
            Ok(handle) => Some(format!("{:?}", handle.close_if_idle().await)),
            Err(error) => Some(format!("no handle: {error}")),
        }
    };
    let after_the_load = listed_in(&host, Some(&second_id)).await;
    let loaded_by_the_request = !not_loaded(&host, &second_id).await;
    let archived = scout_changed_in(&host, Some(&second_id), "archive").await;
    let unarchived = scout_changed_in(&host, Some(&second_id), "unarchive").await;
    let turn = scout_given_a_turn(&host, &sessions, Some(&second_id), AFTER_THE_LOAD).await;
    shut_down(&host, &[]).await;
    let with_the_scout = Ok(vec![
        "back".to_string(),
        "desk".to_string(),
        SCOUT.to_string(),
    ]);
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert_eq!(
        before_the_unload, with_the_scout,
        "the second mob lists its created agent"
    );
    assert!(
        asleep,
        "the mob set unloads the second mob: it unloaded {unloaded:?}; the mob is kept by \
         {kept_by:?}"
    );
    assert_eq!(
        after_the_load, with_the_scout,
        "the second mob keeps its created agent over an unload and a load"
    );
    assert!(
        loaded_by_the_request,
        "the request of the family that names the second mob loads it"
    );
    assert!(
        archived.is_ok() && unarchived.is_ok(),
        "the loaded second mob's agent is changed through its own family: {:?} {:?}",
        archived.as_ref().err(),
        unarchived.as_ref().err()
    );
    assert!(
        turn.as_ref()
            .is_ok_and(|transcript| transcript.contains(AFTER_THE_LOAD)),
        "the loaded second mob's agent takes a turn: {turn:?}"
    );
}
// ---- The Console's withdraw of a held change ----

/// The Console's withdraw of a held change tells two mobs apart. A
/// signed-in person asks, through the Console, for a removal in the second
/// mob that a person decides. The same person's withdraw of that request
/// which names no mob finds no held change (the launch mob holds none
/// under it); the withdraw which names the second mob withdraws it.
#[tokio::test]
async fn the_consoles_withdraw_of_a_held_change_tells_two_mobs_apart() {
    use meerkat_core::approval::decision::DecisionPhase;

    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _roster, _definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let app = console_of_approvers(&mut host, &second_id);
    let second = second_id.to_string();
    let read = console_rpc(
        &app,
        ADMIN,
        "meta/identity/get",
        serde_json::json!({"mob_id": second, "name": SCOUT}),
    )
    .await;
    let asked = console_rpc(
        &app,
        ADMIN,
        "meta/identity/remove",
        serde_json::json!({
            "mob_id": second,
            "request_id": "console-remove",
            "name": SCOUT,
            "expected_spec_digest": read["result"]["spec_digest"],
        }),
    )
    .await;
    let for_no_mob = console_rpc(
        &app,
        ADMIN,
        "meta/identity/withdraw",
        serde_json::json!({"request_id": "console-remove"}),
    )
    .await;
    let for_the_second = console_rpc(
        &app,
        ADMIN,
        "meta/identity/withdraw",
        serde_json::json!({"mob_id": second, "request_id": "console-remove"}),
    )
    .await;
    let ended = asked["result"]["decision_id"]
        .as_str()
        .map(|decision_id| phase_of(&host, decision_id));
    shut_down(&host, &[&second_id]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert_eq!(
        asked["result"]["outcome"],
        serde_json::json!("pending_decision"),
        "the person's removal in the second mob is held: {asked}"
    );
    assert_eq!(
        refusal_kind(&for_no_mob),
        Some("decision_not_found"),
        "a withdraw that names no mob finds no held change of the launch mob: {for_no_mob}"
    );
    assert_eq!(
        for_the_second["result"]["decision_id"], asked["result"]["decision_id"],
        "a withdraw that names the second mob withdraws its held change: {for_the_second}"
    );
    assert_eq!(
        ended,
        Some(Ok(DecisionPhase::Withdrawn)),
        "the held change is withdrawn"
    );
}
// ---- A rule that names agents, in a mob beside the launch mob ----

/// A policy of the rows about names: the stdio host creates an agent of
/// the profile `worker` at once (and names its peers), with the approval
/// rules `rules` after that one, the approval default `default` and the
/// peer rules `peers`.
fn policy_with_rules(
    default: &str,
    rules: &[serde_json::Value],
    peers: serde_json::Value,
) -> Arc<crate::identity_creation::IdentityCreationPolicy> {
    let mut all = vec![serde_json::json!({
        "change": ["create", "peers"],
        "requester": {"kind": "host", "surface": "rpc"},
        "target": {"profile": ["worker"]},
        "outcome": {"kind": "allow"},
    })];
    all.extend(rules.iter().cloned());
    Arc::new(
        crate::identity_creation::IdentityCreationPolicy::parse(&serde_json::json!({
            "enabled": true,
            "peers": peers,
            "approval": {"default": {"kind": default}, "rules": all},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled"),
    )
}

/// Peer rules that allow every peer.
fn every_peer_allowed() -> serde_json::Value {
    serde_json::json!({"default": "allow"})
}

/// What the rules give one change, asked as a plan of the family of the
/// mob `mob_id` (the launch mob for `None`) by `requester`: the planned
/// outcome's token (`allow`, `require_approval`), or the kind of the
/// refusal (`change_denied`, `peer_not_allowed`). A plan holds nothing and
/// writes nothing.
async fn planned_as(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
    requester: crate::identity_creation::ChangeRequester,
    params: serde_json::Value,
) -> Result<String, String> {
    asked_as(host, mob_id, requester, "meta/identity/plan", params)
        .await
        .map(|plan| {
            plan["outcome"]
                .as_str()
                .unwrap_or("a plan with no outcome")
                .to_string()
        })
}

/// One call of the family of the mob `mob_id` (the launch mob for `None`)
/// by `requester`: the answer's result, or the kind of the refusal
/// (`change_denied`, `approval_unavailable`).
async fn asked_as(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
    requester: crate::identity_creation::ChangeRequester,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mob = host
        .resolve_mob(mob_id)
        .await
        .map_err(|error| format!("the mob is not served: {error:?}"))?;
    let family = mob
        .identity_runtime_ref()
        .and_then(|identities| identities.identity_family())
        .ok_or_else(|| "the mob has no family of its agents' changes".to_string())?;
    let handle = mob.mob_runtime().handle();
    let surface = crate::rpc::identity_methods::IdentityFamilySurface {
        mob: &handle,
        family: family.as_ref(),
        requester,
        access: None,
    };
    let answer = crate::rpc::identity_methods::handle_identity_family_rpc(
        &surface,
        method,
        serde_json::json!(1),
        &params,
    )
    .await
    .ok_or_else(|| format!("{method} is outside the family"))?;
    match (answer.result, answer.error) {
        (Some(result), _) => Ok(result),
        (None, Some(error)) => Err(error
            .data
            .as_ref()
            .and_then(|data| data["kind"].as_str())
            .unwrap_or("a refusal with no kind")
            .to_string()),
        (None, None) => Err("neither a result nor a refusal".to_string()),
    }
}

/// The plan of a create of the agent `runner` of the profile `worker`,
/// with the peers `peers`.
fn a_create_of_a_runner(peers: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "change": "create",
        "request_id": "plan-a-runner",
        "agents": [{"name": "runner", "profile": "worker", "peers": peers}],
    })
}

/// The plan of the change `change` (`update`, `archive` or `remove`) of the
/// agent [`SCOUT`] of the mob `mob_id`, asked by the stdio host with the
/// digest a get answers now.
async fn planned_of_the_scout(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
    change: &str,
) -> Result<String, String> {
    let read = asked_of_the_family(
        host,
        mob_id,
        "meta/identity/get",
        serde_json::json!({"name": SCOUT}),
    )
    .await?;
    let mut params = serde_json::json!({
        "change": change,
        "request_id": format!("plan-{change}-of-the-scout"),
        "name": SCOUT,
        "expected_spec_digest": read["spec_digest"],
    });
    if change == "update" {
        params["set"] = serde_json::json!({"display_name": "Scout"});
    }
    planned_as(
        host,
        mob_id,
        crate::rpc::identity_methods::stdio_requester(),
        params,
    )
    .await
}

/// The rule of the rows about the asker: the agent named `desk` may create
/// an agent of the profile `worker` at once. Each mob of the rows has an
/// agent of that name.
fn the_agent_named_desk_creates_workers() -> Arc<crate::identity_creation::IdentityCreationPolicy> {
    policy_with_rules(
        "deny",
        &[serde_json::json!({
            "change": ["create"],
            "requester": {"kind": "agent", "identity": ["desk"]},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        })],
        every_peer_allowed(),
    )
}

/// A rule that allows an agent by its name allows nothing to the agent of
/// that name in a second mob. The launch mob's agent of the name keeps the
/// rule; in a second mob the name is chosen by whoever creates the agent
/// there, so the rule does not allow it anything: its create falls to the
/// default and is denied.
#[tokio::test]
async fn a_rule_that_allows_an_agent_by_its_name_allows_nothing_to_that_name_in_a_second_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, second_id) = two_mobs(&temp, Some(the_agent_named_desk_creates_workers())).await;
    let desk = || crate::identity_creation::ChangeRequester::agent_recorded("desk");
    let in_the_launch_mob = planned_as(&host, None, desk(), a_create_of_a_runner(&[])).await;
    let in_the_second =
        planned_as(&host, Some(&second_id), desk(), a_create_of_a_runner(&[])).await;
    shut_down(&host, &[&second_id]).await;
    assert_eq!(
        in_the_launch_mob,
        Ok("allow".to_string()),
        "the launch mob's agent of the name keeps its rule"
    );
    assert_eq!(
        in_the_second,
        Err("change_denied".to_string()),
        "the second mob's agent of the same name is allowed nothing by it"
    );
}

/// A rule that allows a change of an agent by its name allows none of the
/// agent of that name in a second mob: an update, an archive and a removal
/// of the launch mob's agent are allowed by it, and each of them is denied
/// for the second mob's agent of the same name.
#[tokio::test]
async fn a_rule_that_allows_a_change_of_an_agent_by_its_name_allows_none_in_a_second_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let policy = policy_with_rules(
        "deny",
        &[serde_json::json!({
            "change": ["descriptive", "archive", "remove"],
            "requester": {"kind": "host", "surface": "rpc"},
            "target": {"identity": [SCOUT]},
            "outcome": {"kind": "allow"},
        })],
        every_peer_allowed(),
    );
    let (host, second_id) = two_mobs(&temp, Some(policy)).await;
    let created = (
        scout_created_in(&host, None).await,
        scout_created_in(&host, Some(&second_id)).await,
    );
    let mut wrong = Vec::new();
    for change in ["update", "archive", "remove"] {
        let in_the_launch_mob = planned_of_the_scout(&host, None, change).await;
        let in_the_second = planned_of_the_scout(&host, Some(&second_id), change).await;
        if in_the_launch_mob != Ok("allow".to_string())
            || in_the_second != Err("change_denied".to_string())
        {
            wrong.push(format!(
                "{change}: the launch mob {in_the_launch_mob:?}, the second mob {in_the_second:?}"
            ));
        }
    }
    shut_down(&host, &[&second_id]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok(),
        "an agent of one name is created in each mob: {created:?}"
    );
    assert!(
        wrong.is_empty(),
        "a change of the launch mob's agent is allowed by its name and the same change of the \
         second mob's agent of that name is denied:\n{}",
        wrong.join("\n")
    );
}

/// A rule that denies by an agent's name denies in a second mob too, and
/// one that asks for a person's decision by a name asks there too: a name
/// tightens in every mob. The rules that name no agent allow the host's
/// archive of every agent of the profile; beside them the rule by name is
/// the stricter one in both mobs, under a default of deny and under a
/// default of allow.
#[tokio::test]
async fn a_rule_that_tightens_by_an_agents_name_tightens_in_a_second_mob_too() {
    let mut wrong = Vec::new();
    for (default, by_name, expected) in [
        (
            "deny",
            serde_json::json!({"kind": "deny"}),
            Err("change_denied".to_string()),
        ),
        (
            "deny",
            serde_json::json!({"kind": "require_approval", "approver": {"id": APPROVERS}}),
            Ok("require_approval".to_string()),
        ),
        (
            "allow",
            serde_json::json!({"kind": "require_approval", "approver": {"id": APPROVERS}}),
            Ok("require_approval".to_string()),
        ),
    ] {
        let temp = tempfile::tempdir().expect("tempdir");
        let policy = policy_with_rules(
            default,
            &[
                serde_json::json!({
                    "change": ["archive"],
                    "requester": {"kind": "host", "surface": "rpc"},
                    "target": {"profile": ["worker"]},
                    "outcome": {"kind": "allow"},
                }),
                serde_json::json!({
                    "change": ["archive"],
                    "target": {"identity": [SCOUT]},
                    "outcome": by_name,
                }),
            ],
            every_peer_allowed(),
        );
        let (host, second_id) = two_mobs(&temp, Some(policy)).await;
        let created = (
            scout_created_in(&host, None).await,
            scout_created_in(&host, Some(&second_id)).await,
        );
        let in_the_launch_mob = planned_of_the_scout(&host, None, "archive").await;
        let in_the_second = planned_of_the_scout(&host, Some(&second_id), "archive").await;
        shut_down(&host, &[&second_id]).await;
        if created.0.is_err()
            || created.1.is_err()
            || in_the_launch_mob != expected
            || in_the_second != expected
        {
            wrong.push(format!(
                "a default of {default}, the rule by name {by_name}: created {created:?}; the \
                 launch mob {in_the_launch_mob:?}, the second mob {in_the_second:?}, expected \
                 {expected:?} in both"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a rule by name tightens in both mobs:\n{}",
        wrong.join("\n")
    );
}

/// A peer rule that allows by a name allows no peer of that name in a
/// second mob: under peer rules whose default is deny and that allow the
/// peer named `desk`, a create with that peer is planned in the launch
/// mob and refused for the peer in a second mob, whose agent of that name
/// the rule does not allow.
#[tokio::test]
async fn a_peer_rule_that_allows_by_a_name_allows_no_peer_of_that_name_in_a_second_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let policy = policy_with_rules(
        "deny",
        &[],
        serde_json::json!({
            "default": "deny",
            "rules": [{"id": "desk-by-name", "target": {"identity": ["desk"]}, "outcome": "allow"}],
        }),
    );
    let (host, second_id) = two_mobs(&temp, Some(policy)).await;
    let host_asks = crate::rpc::identity_methods::stdio_requester;
    let in_the_launch_mob =
        planned_as(&host, None, host_asks(), a_create_of_a_runner(&["desk"])).await;
    let in_the_second = planned_as(
        &host,
        Some(&second_id),
        host_asks(),
        a_create_of_a_runner(&["desk"]),
    )
    .await;
    shut_down(&host, &[&second_id]).await;
    assert_eq!(
        in_the_launch_mob,
        Ok("allow".to_string()),
        "the launch mob's peer of the name is allowed by its rule"
    );
    assert_eq!(
        in_the_second,
        Err("peer_not_allowed".to_string()),
        "the second mob's peer of the same name is not allowed by it"
    );
}

/// A peer rule that denies by a name denies in a second mob too: under
/// peer rules whose default is allow and that deny the peer named `desk`,
/// a create with that peer is refused for the peer in both mobs.
#[tokio::test]
async fn a_peer_rule_that_denies_by_a_name_denies_in_a_second_mob_too() {
    let temp = tempfile::tempdir().expect("tempdir");
    let policy = policy_with_rules(
        "deny",
        &[],
        serde_json::json!({
            "default": "allow",
            "rules": [{"id": "desk-by-name", "target": {"identity": ["desk"]}, "outcome": "deny"}],
        }),
    );
    let (host, second_id) = two_mobs(&temp, Some(policy)).await;
    let host_asks = crate::rpc::identity_methods::stdio_requester;
    let in_the_launch_mob =
        planned_as(&host, None, host_asks(), a_create_of_a_runner(&["desk"])).await;
    let in_the_second = planned_as(
        &host,
        Some(&second_id),
        host_asks(),
        a_create_of_a_runner(&["desk"]),
    )
    .await;
    let without_the_peer = planned_as(
        &host,
        Some(&second_id),
        host_asks(),
        a_create_of_a_runner(&[]),
    )
    .await;
    shut_down(&host, &[&second_id]).await;
    assert_eq!(
        (in_the_launch_mob, in_the_second),
        (
            Err("peer_not_allowed".to_string()),
            Err("peer_not_allowed".to_string())
        ),
        "the peer of the name is denied in both mobs"
    );
    assert_eq!(
        without_the_peer,
        Ok("allow".to_string()),
        "the same create without that peer is planned in the second mob"
    );
}

/// After a restart a rule that allows an agent by its name still allows
/// nothing to the agent of that name in a second mob: the second mob is
/// loaded again by the request that names it, and its family applies the
/// launch's policy as it did before.
#[tokio::test]
async fn after_a_restart_a_rule_by_name_still_allows_nothing_in_a_second_mob() {
    let stored = super::mob_entries::lazy_start_tests::StoredHost::new();
    let roster = Arc::new(LaunchRoster::default());
    let (first, _sessions) =
        stored_launch_under(&stored, &roster, the_agent_named_desk_creates_workers()).await;
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&first, of_the_second).await;
    shut_down(&first, &[]).await;
    drop(first);

    let (host, _sessions) =
        stored_launch_under(&stored, &roster, the_agent_named_desk_creates_workers()).await;
    let desk = || crate::identity_creation::ChangeRequester::agent_recorded("desk");
    let in_the_launch_mob = planned_as(&host, None, desk(), a_create_of_a_runner(&[])).await;
    let in_the_second =
        planned_as(&host, Some(&second_id), desk(), a_create_of_a_runner(&[])).await;
    shut_down(&host, &[]).await;
    assert_eq!(
        in_the_launch_mob,
        Ok("allow".to_string()),
        "the launch mob's agent of the name keeps its rule after the restart"
    );
    assert_eq!(
        in_the_second,
        Err("change_denied".to_string()),
        "the second mob's agent of the same name is allowed nothing by it after the restart"
    );
}

// ---- A mob the host's set keeps no record of ----

/// A launch over the state `stored` keeps, as [`stored_launch_under`] makes
/// one, that keeps its durable decisions in the directory `kept` and lets a
/// person decide removals ([`removals_decided_by_a_person`]). A later
/// launch over the same `kept` reads the decisions of the earlier one,
/// whatever state its mobs are stored in.
async fn stored_launch_whose_removals_a_person_decides(
    stored: &super::mob_entries::lazy_start_tests::StoredHost,
    roster: &Arc<LaunchRoster>,
    kept: &std::path::Path,
) -> (UnifiedRuntime, Sessions) {
    let (mut host, sessions, _machine, ()) = stored.launch_over(|_| ()).await;
    let of_the_launch_mob = identity_context(host.mob_runtime(), "default");
    Box::pin(host.install_and_bootstrap_identity_first_context(
        of_the_launch_mob,
        &[configured_agent("desk"), configured_agent("front")],
    ))
    .await
    .expect("the launch mob's identity bootstrap");
    removals_decided_by_a_person(&mut host, kept);
    host.compose_host_mob_identities(Arc::new(gateways_composer(roster)));
    (host, sessions)
}

/// A launch of the mob of `of_the_launch_mob`, as [`launch_with_identities`]
/// makes one, that keeps its durable decisions in `temp` and lets a person
/// decide removals. Its mobs' stores are its process's own: a second
/// launch with the same definition over the same `temp` is the same host
/// after its process ended, with the decisions of the first and none of
/// the mobs it hosted beside its launch mob.
async fn process_bound_launch_whose_removals_a_person_decides(
    temp: &tempfile::TempDir,
    of_the_launch_mob: meerkat_mob::MobDefinition,
    roster: &Arc<LaunchRoster>,
) -> (UnifiedRuntime, Sessions) {
    let (mut host, sessions) = launch(temp, of_the_launch_mob).await;
    let identities = identity_context(host.mob_runtime(), "default");
    Box::pin(host.install_and_bootstrap_identity_first_context(
        identities,
        &[configured_agent("desk"), configured_agent("front")],
    ))
    .await
    .expect("the launch mob's identity bootstrap");
    removals_decided_by_a_person(&mut host, temp.path());
    host.compose_host_mob_identities(Arc::new(gateways_composer(roster)));
    (host, sessions)
}

/// The phase the decision `decision_id` reads once the host's driver of
/// decisions was asked for a pass and had its time: the first phase other
/// than `held_as` it reads within two seconds, else `held_as`.
async fn phase_a_pass_leaves(
    host: &UnifiedRuntime,
    decision_id: &str,
    held_as: meerkat_core::approval::decision::DecisionPhase,
) -> Result<meerkat_core::approval::decision::DecisionPhase, String> {
    host.durable_decisions().notify_work();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let phase = phase_of(host, decision_id)?;
        if phase != held_as || tokio::time::Instant::now() >= deadline {
            return Ok(phase);
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Whether `host`'s mob set lists no mob of the id `mob_id`, loaded or not.
async fn unknown_to_the_set(host: &UnifiedRuntime, mob_id: &MobId) -> bool {
    let known = super::mob_entries::lazy_start_tests::known(host).await;
    !known.not_loaded.contains(mob_id) && !known.loaded.contains(mob_id)
}

/// A held change of a mob that an earlier launch destroyed is neither
/// withdrawn nor carried out. A removal is held in a second mob; the
/// launch destroys the mob and its process ends before the pass its
/// destroy asks for is made (the row holds that pass back), so the
/// removal is still held. The next launch
/// of the host reads the same decisions; its mob set has no record of the
/// mob, since nothing of this launch destroyed it. The removal stays
/// held, and once a person approved it, it is carried out in no mob and
/// still not withdrawn: it ends at its deadline.
///
/// The fixture keeps the mobs beside the launch mob with their process, so
/// the next launch has no store of the mob whether or not it was
/// destroyed: what the row shows from the outside is that a launch whose
/// set has no record of a mob leaves the mob's held change. "Stays" is
/// read for two seconds after the driver was asked for a pass
/// ([`phase_a_pass_leaves`]).
#[tokio::test]
async fn a_held_change_of_a_mob_an_earlier_launch_destroyed_is_neither_withdrawn_nor_carried_out() {
    use meerkat_core::approval::decision::DecisionPhase;

    let temp = tempfile::tempdir().expect("tempdir");
    let of_the_launch_mob = definition("host");
    let roster = Arc::new(LaunchRoster::default());
    let (first, _sessions) = process_bound_launch_whose_removals_a_person_decides(
        &temp,
        of_the_launch_mob.clone(),
        &roster,
    )
    .await;
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&first, of_the_second).await;
    let created = scout_created_in(&first, Some(&second_id)).await;
    let held = held_by(&scout_changed_in(&first, Some(&second_id), "remove").await);
    // No pass follows the set from here on, and the process ends before the
    // pass its destroy asks for is made (the row holds that pass back): the
    // destroy ends nothing that is held.
    first.hold_the_follower_of_host_mobs_in_tests().await;
    first.destroys_of_host_mobs_ask_for_no_pass_in_tests();
    let destroyed = Box::pin(set_of(&first).mob_destroy(&second_id)).await;
    let before_the_process_ended = match &held {
        Ok(decision_id) => phase_of(&first, decision_id),
        Err(error) => Err(error.clone()),
    };
    shut_down(&first, &[]).await;
    drop(first);

    let (host, _sessions) =
        process_bound_launch_whose_removals_a_person_decides(&temp, of_the_launch_mob, &roster)
            .await;
    let no_record_of_the_mob = unknown_to_the_set(&host, &second_id).await;
    let after_the_launch = match &held {
        Ok(decision_id) => phase_a_pass_leaves(&host, decision_id, DecisionPhase::Open).await,
        Err(error) => Err(error.clone()),
    };
    let approved_by_a_person = match &held {
        Ok(decision_id) => approved(&host, decision_id).await,
        Err(error) => Err(error.clone()),
    };
    let after_the_approval = match &held {
        Ok(decision_id) => phase_a_pass_leaves(&host, decision_id, DecisionPhase::Approved).await,
        Err(error) => Err(error.clone()),
    };
    shut_down(&host, &[]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert!(
        destroyed.is_ok(),
        "the first launch's set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        before_the_process_ended,
        Ok(DecisionPhase::Open),
        "the removal is still held when the process that destroyed its mob ends"
    );
    assert!(
        no_record_of_the_mob,
        "the next launch's set lists no mob of the destroyed mob's id"
    );
    assert_eq!(
        after_the_launch,
        Ok(DecisionPhase::Open),
        "a removal held for a mob that an earlier launch destroyed stays held"
    );
    assert_eq!(
        approved_by_a_person,
        Ok(()),
        "a person approves the removal that is still held"
    );
    assert_eq!(
        after_the_approval,
        Ok(DecisionPhase::Approved),
        "the approved removal is carried out in no mob and is not withdrawn"
    );
}

/// A launch whose mob set does not see a mob's store withdraws none of its
/// held changes. A removal is held in a second mob; the next launch keeps
/// the same decisions over another state directory, so its set lists no
/// mob of that id and recorded no destroy of one: the removal stays held
/// ("stays" is read for two seconds after the driver was asked for a
/// pass, [`phase_a_pass_leaves`]).
#[tokio::test]
async fn a_launch_whose_set_does_not_see_a_mobs_store_withdraws_none_of_its_held_changes() {
    use meerkat_core::approval::decision::DecisionPhase;

    let stored = super::mob_entries::lazy_start_tests::StoredHost::new();
    let kept = tempfile::tempdir().expect("tempdir");
    let roster = Arc::new(LaunchRoster::default());
    let (first, _sessions) =
        stored_launch_whose_removals_a_person_decides(&stored, &roster, kept.path()).await;
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&first, of_the_second).await;
    let created = scout_created_in(&first, Some(&second_id)).await;
    let held = held_by(&scout_changed_in(&first, Some(&second_id), "remove").await);
    shut_down(&first, &[]).await;
    drop(first);

    // Another state directory: this launch's set sees no store of the mob.
    let elsewhere = super::mob_entries::lazy_start_tests::StoredHost::new();
    let (host, _sessions) =
        stored_launch_whose_removals_a_person_decides(&elsewhere, &roster, kept.path()).await;
    let no_record_of_the_mob = unknown_to_the_set(&host, &second_id).await;
    let after_the_launch = match &held {
        Ok(decision_id) => phase_a_pass_leaves(&host, decision_id, DecisionPhase::Open).await,
        Err(error) => Err(error.clone()),
    };
    shut_down(&host, &[]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert!(
        no_record_of_the_mob,
        "the launch's set lists no mob of the id the removal was held for"
    );
    assert_eq!(
        after_the_launch,
        Ok(DecisionPhase::Open),
        "a removal held for a mob whose store this launch does not see stays held"
    );
}

// ---- A rule by name that is as strict as the rules that name no agent ----

/// The outcome of a rule that has a person decide for `approver` within
/// `timeout_ms`.
fn decided_for(approver: &str, timeout_ms: u64) -> serde_json::Value {
    serde_json::json!({
        "kind": "require_approval",
        "approver": {"id": approver},
        "timeout_ms": timeout_ms,
    })
}

/// A rule for a create of an agent of the profile `worker` asked by the
/// agents `requester` selects, with `outcome`.
fn a_create_asked_by(
    requester: serde_json::Value,
    outcome: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "change": ["create"],
        "requester": requester,
        "target": {"profile": ["worker"]},
        "outcome": outcome,
    })
}

/// The approver the rows' rule by name states, where it differs from the
/// approver of the rule that names no agent.
const ANOTHER_APPROVER: &str = "team:night";
/// The time limits the rows' rules state, in milliseconds.
const AN_HOUR: u64 = 3_600_000;
const A_MINUTE: u64 = 60_000;

/// A policy under which every agent's create of a worker is decided by a
/// person for [`APPROVERS`], and the create asked by the agent named
/// `desk` is decided, as strictly, for [`ANOTHER_APPROVER`].
fn an_equal_rule_by_name_states_another_approver()
-> Arc<crate::identity_creation::IdentityCreationPolicy> {
    policy_with_rules(
        "deny",
        &[
            a_create_asked_by(
                serde_json::json!({"kind": "agent"}),
                decided_for(APPROVERS, AN_HOUR),
            ),
            a_create_asked_by(
                serde_json::json!({"kind": "agent", "identity": ["desk"]}),
                decided_for(ANOTHER_APPROVER, AN_HOUR),
            ),
        ],
        every_peer_allowed(),
    )
}

/// What a call of the family answered, in one word: the outcome of its
/// result (`done`, `pending_decision`, `require_approval`), or the kind of
/// its refusal, however the refusal was carried.
fn in_a_word(asked: &Result<serde_json::Value, String>) -> String {
    match asked {
        Ok(answer) if answer["outcome"] == "refused" => answer["kind"]
            .as_str()
            .unwrap_or("a refusal with no kind")
            .to_string(),
        Ok(answer) => answer["outcome"]
            .as_str()
            .unwrap_or("an answer with no outcome")
            .to_string(),
        Err(kind) => kind.clone(),
    }
}

/// The create of the agent `runner` of the profile `worker`, as a verb.
fn the_create_of_a_runner() -> serde_json::Value {
    serde_json::json!({
        "request_id": "create-a-runner",
        "agents": [{"name": "runner", "profile": "worker"}],
    })
}

/// A rule by name that is as strict as the rules that name no agent chooses
/// no approver in a second mob. In the launch mob the two equally strict
/// rules state different approvers, so no one approver can decide and the
/// create is refused, as released. In a second mob the rule by name does
/// not count beside an equally strict rule that names no agent: the
/// approver of that rule decides, and the name makes no refusal.
#[tokio::test]
async fn an_equal_rule_by_name_chooses_no_approver_in_a_second_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, second_id) =
        two_mobs(&temp, Some(an_equal_rule_by_name_states_another_approver())).await;
    let desk = || crate::identity_creation::ChangeRequester::agent_recorded("desk");
    let in_the_launch_mob = asked_as(
        &host,
        None,
        desk(),
        "meta/identity/plan",
        a_create_of_a_runner(&[]),
    )
    .await;
    let in_the_second = asked_as(
        &host,
        Some(&second_id),
        desk(),
        "meta/identity/plan",
        a_create_of_a_runner(&[]),
    )
    .await;
    shut_down(&host, &[&second_id]).await;
    assert_eq!(
        in_the_launch_mob,
        Err("approval_unavailable".to_string()),
        "in the launch mob the two equally strict rules state different approvers"
    );
    let planned = in_the_second.expect("the second mob's create is planned");
    assert_eq!(
        (planned["outcome"].as_str(), planned["approver_id"].as_str()),
        (Some("require_approval"), Some(APPROVERS)),
        "in the second mob the approver of the rule that names no agent decides: {planned}"
    );
}

/// A rule by name that is as strict as the rules that name no agent sets no
/// time limit in a second mob, though it is declared first. In the launch
/// mob the first declared of the equally strict rules gives the decision
/// its time limit, as released; in a second mob the rule that names no
/// agent gives it.
#[tokio::test]
async fn an_equal_rule_by_name_sets_no_time_limit_in_a_second_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let policy = policy_with_rules(
        "deny",
        &[
            a_create_asked_by(
                serde_json::json!({"kind": "agent", "identity": ["desk"]}),
                decided_for(APPROVERS, A_MINUTE),
            ),
            a_create_asked_by(
                serde_json::json!({"kind": "agent"}),
                decided_for(APPROVERS, AN_HOUR),
            ),
        ],
        every_peer_allowed(),
    );
    let (host, second_id) = two_mobs(&temp, Some(policy)).await;
    let desk = || crate::identity_creation::ChangeRequester::agent_recorded("desk");
    let in_the_launch_mob = asked_as(
        &host,
        None,
        desk(),
        "meta/identity/plan",
        a_create_of_a_runner(&[]),
    )
    .await;
    let in_the_second = asked_as(
        &host,
        Some(&second_id),
        desk(),
        "meta/identity/plan",
        a_create_of_a_runner(&[]),
    )
    .await;
    shut_down(&host, &[&second_id]).await;
    let limit = |planned: &Result<serde_json::Value, String>| {
        planned
            .as_ref()
            .map(|plan| plan["timeout_ms"].as_u64())
            .map_err(Clone::clone)
    };
    assert_eq!(
        limit(&in_the_launch_mob),
        Ok(Some(A_MINUTE)),
        "in the launch mob the first declared of the equally strict rules sets the time limit"
    );
    assert_eq!(
        limit(&in_the_second),
        Ok(Some(AN_HOUR)),
        "in the second mob the rule that names no agent sets the time limit"
    );
}

/// The verb in a second mob does what its plan says under an equally
/// strict rule by name: the create the agent named `desk` asks is planned
/// as a person's decision and is held, where the launch mob refuses both
/// the plan and the create. No agent is made in either mob.
#[tokio::test]
async fn under_an_equal_rule_by_name_the_verb_in_a_second_mob_holds_what_its_plan_says() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _sessions) = launch_with_identities(&temp).await;
    decisions_kept_under(
        &mut host,
        temp.path(),
        an_equal_rule_by_name_states_another_approver(),
    );
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(&roster)));
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&host, of_the_second).await;
    let desk = || crate::identity_creation::ChangeRequester::agent_recorded("desk");
    let mut answered = Vec::new();
    for mob_id in [None, Some(&second_id)] {
        let planned = asked_as(
            &host,
            mob_id,
            desk(),
            "meta/identity/plan",
            a_create_of_a_runner(&[]),
        )
        .await;
        let asked = asked_as(
            &host,
            mob_id,
            desk(),
            "meta/identity/create",
            the_create_of_a_runner(),
        )
        .await;
        answered.push((in_a_word(&planned), in_a_word(&asked)));
    }
    let listed = (
        listed_in(&host, None).await,
        listed_in(&host, Some(&second_id)).await,
    );
    shut_down(&host, &[&second_id]).await;
    assert_eq!(
        answered,
        vec![
            (
                "approval_unavailable".to_string(),
                "approval_unavailable".to_string()
            ),
            (
                "require_approval".to_string(),
                "pending_decision".to_string()
            ),
        ],
        "the launch mob refuses the plan and the create; the second mob plans a person's \
         decision and holds the create"
    );
    assert_eq!(
        listed,
        (
            Ok(vec!["desk".to_string(), "front".to_string()]),
            Ok(vec!["back".to_string(), "desk".to_string()]),
        ),
        "no agent is made in either mob"
    );
}

/// A create through the verb by an agent a rule allows by its name is made
/// in the launch mob alone: the verb and the plan apply one policy, so the
/// agent of the same name in a second mob is denied the create and no
/// agent is made there.
#[tokio::test]
async fn a_create_through_the_verb_by_an_agent_allowed_by_name_is_made_in_the_launch_mob_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, second_id) = two_mobs(&temp, Some(the_agent_named_desk_creates_workers())).await;
    let desk = || crate::identity_creation::ChangeRequester::agent_recorded("desk");
    let in_the_launch_mob = asked_as(
        &host,
        None,
        desk(),
        "meta/identity/create",
        the_create_of_a_runner(),
    )
    .await;
    let in_the_second = asked_as(
        &host,
        Some(&second_id),
        desk(),
        "meta/identity/create",
        the_create_of_a_runner(),
    )
    .await;
    let listed = (
        listed_in(&host, None).await,
        listed_in(&host, Some(&second_id)).await,
    );
    shut_down(&host, &[&second_id]).await;
    assert_eq!(
        (in_a_word(&in_the_launch_mob), in_a_word(&in_the_second)),
        ("done".to_string(), "change_denied".to_string()),
        "the launch mob's agent of the name creates; the second mob's agent of the name is denied"
    );
    assert_eq!(
        listed,
        (
            Ok(vec![
                "desk".to_string(),
                "front".to_string(),
                "runner".to_string()
            ]),
            Ok(vec!["back".to_string(), "desk".to_string()]),
        ),
        "the agent is made in the launch mob alone"
    );
}

/// A rule that names an agent beside a profile is a rule by name as a
/// whole: in a second mob it allows nothing, whether the name stands in
/// its target or in its requester. The launch mob's agents keep it.
#[tokio::test]
async fn a_rule_that_names_an_agent_beside_a_profile_allows_nothing_in_a_second_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let policy = policy_with_rules(
        "deny",
        &[
            serde_json::json!({
                "change": ["archive", "remove"],
                "requester": {"kind": "host", "surface": "rpc"},
                "target": {"identity": [SCOUT], "profile": ["worker"]},
                "outcome": {"kind": "allow"},
            }),
            a_create_asked_by(
                serde_json::json!({"kind": "agent", "identity": ["desk"], "profile": ["worker"]}),
                serde_json::json!({"kind": "allow"}),
            ),
        ],
        every_peer_allowed(),
    );
    let (host, second_id) = two_mobs(&temp, Some(policy)).await;
    let created = (
        scout_created_in(&host, None).await,
        scout_created_in(&host, Some(&second_id)).await,
    );
    let desk = || crate::identity_creation::ChangeRequester::agent_recorded("desk");
    let mut wrong = Vec::new();
    for change in ["archive", "remove"] {
        let in_the_launch_mob = planned_of_the_scout(&host, None, change).await;
        let in_the_second = planned_of_the_scout(&host, Some(&second_id), change).await;
        if in_the_launch_mob != Ok("allow".to_string())
            || in_the_second != Err("change_denied".to_string())
        {
            wrong.push(format!(
                "{change} of the named agent: the launch mob {in_the_launch_mob:?}, the second \
                 mob {in_the_second:?}"
            ));
        }
    }
    let by_the_launch_mobs_desk = planned_as(&host, None, desk(), a_create_of_a_runner(&[])).await;
    let by_the_seconds_desk =
        planned_as(&host, Some(&second_id), desk(), a_create_of_a_runner(&[])).await;
    if by_the_launch_mobs_desk != Ok("allow".to_string())
        || by_the_seconds_desk != Err("change_denied".to_string())
    {
        wrong.push(format!(
            "a create by the named agent: the launch mob {by_the_launch_mobs_desk:?}, the second \
             mob {by_the_seconds_desk:?}"
        ));
    }
    shut_down(&host, &[&second_id]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok(),
        "an agent of one name is created in each mob: {created:?}"
    );
    assert!(
        wrong.is_empty(),
        "a rule that names an agent beside a profile allows in the launch mob and nothing in a \
         second mob:\n{}",
        wrong.join("\n")
    );
}

// ---- Held changes of a mob that is not loaded ----

/// A change a person approved while its mob is not loaded is carried out
/// when the mob is loaded, and not before. After a restart the second mob
/// is not loaded; the removal held for its agent is approved, and the
/// host's driver neither loads the mob nor ends the removal. The first
/// request of the family that names the mob loads it, and the removal is
/// carried out in it: its agent is gone.
#[tokio::test]
async fn a_change_approved_while_its_mob_is_not_loaded_is_carried_out_once_the_mob_is_loaded() {
    use meerkat_core::approval::decision::DecisionPhase;

    let stored = super::mob_entries::lazy_start_tests::StoredHost::new();
    let kept = tempfile::tempdir().expect("tempdir");
    let roster = Arc::new(LaunchRoster::default());
    let (first, _sessions) =
        stored_launch_whose_removals_a_person_decides(&stored, &roster, kept.path()).await;
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&first, of_the_second).await;
    let created = scout_created_in(&first, Some(&second_id)).await;
    let held = held_by(&scout_changed_in(&first, Some(&second_id), "remove").await);
    shut_down(&first, &[]).await;
    drop(first);

    let (host, _sessions) =
        stored_launch_whose_removals_a_person_decides(&stored, &roster, kept.path()).await;
    let approved_by_a_person = match &held {
        Ok(decision_id) => approved(&host, decision_id).await,
        Err(error) => Err(error.clone()),
    };
    let while_not_loaded = match &held {
        Ok(decision_id) => phase_a_pass_leaves(&host, decision_id, DecisionPhase::Approved).await,
        Err(error) => Err(error.clone()),
    };
    let still_not_loaded = not_loaded(&host, &second_id).await;
    let listed_by_the_loading_request = listed_in(&host, Some(&second_id)).await;
    host.durable_decisions().notify_work();
    let once_loaded = match &held {
        Ok(decision_id) => phase_once(&host, decision_id, &[DecisionPhase::Executed]).await,
        Err(error) => Err(error.clone()),
    };
    let listed_after = listed_in(&host, Some(&second_id)).await;
    shut_down(&host, &[]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert_eq!(
        approved_by_a_person,
        Ok(()),
        "a person approves the removal while its mob is not loaded"
    );
    assert_eq!(
        while_not_loaded,
        Ok(DecisionPhase::Approved),
        "the approved removal waits while its mob is not loaded"
    );
    assert!(
        still_not_loaded,
        "the host's driver of decisions loads no mob for an approved change"
    );
    assert_eq!(
        listed_by_the_loading_request,
        Ok(vec![
            "back".to_string(),
            "desk".to_string(),
            SCOUT.to_string()
        ]),
        "the request that loads the mob still lists the agent"
    );
    assert_eq!(
        once_loaded,
        Ok(DecisionPhase::Executed),
        "the approved removal is carried out once its mob is loaded"
    );
    assert_eq!(
        listed_after,
        Ok(vec!["back".to_string(), "desk".to_string()]),
        "the agent is removed from its own mob"
    );
}

/// The policy of the rows about held changes, with a short time limit for
/// a removal: a person decides it within a second and a half.
fn a_person_decides_removals_soon() -> Arc<crate::identity_creation::IdentityCreationPolicy> {
    let soon = decided_for(APPROVERS, 1500);
    Arc::new(
        crate::identity_creation::IdentityCreationPolicy::parse(&serde_json::json!({
            "enabled": true,
            "approval": {"rules": [
                {"change": ["create"], "requester": {"kind": "host"},
                 "target": {"profile": ["worker"]}, "outcome": {"kind": "allow"}},
                {"change": ["remove"], "requester": {"kind": "host"},
                 "target": {"profile": ["worker"]}, "outcome": soon},
            ]},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled"),
    )
}

/// [`stored_launch_whose_removals_a_person_decides`], under the policy
/// [`a_person_decides_removals_soon`].
async fn stored_launch_whose_removals_are_decided_soon(
    stored: &super::mob_entries::lazy_start_tests::StoredHost,
    roster: &Arc<LaunchRoster>,
    kept: &std::path::Path,
) -> (UnifiedRuntime, Sessions) {
    let (mut host, sessions, _machine, ()) = stored.launch_over(|_| ()).await;
    let of_the_launch_mob = identity_context(host.mob_runtime(), "default");
    Box::pin(host.install_and_bootstrap_identity_first_context(
        of_the_launch_mob,
        &[configured_agent("desk"), configured_agent("front")],
    ))
    .await
    .expect("the launch mob's identity bootstrap");
    decisions_kept_under(&mut host, kept, a_person_decides_removals_soon());
    host.compose_host_mob_identities(Arc::new(gateways_composer(roster)));
    (host, sessions)
}

/// A held change of a mob that is not loaded expires at its deadline, and
/// loads nothing. After a restart the second mob is not loaded; the
/// removal held for its agent, which nobody decides, ends expired at the
/// first pass of the host's driver after its deadline, and the mob is
/// still not loaded.
#[tokio::test]
async fn a_held_change_of_a_mob_that_is_not_loaded_expires_at_its_deadline() {
    use meerkat_core::approval::decision::DecisionPhase;

    let stored = super::mob_entries::lazy_start_tests::StoredHost::new();
    let kept = tempfile::tempdir().expect("tempdir");
    let roster = Arc::new(LaunchRoster::default());
    let (first, _sessions) =
        stored_launch_whose_removals_are_decided_soon(&stored, &roster, kept.path()).await;
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&first, of_the_second).await;
    let created = scout_created_in(&first, Some(&second_id)).await;
    let held = held_by(&scout_changed_in(&first, Some(&second_id), "remove").await);
    shut_down(&first, &[]).await;
    drop(first);

    let (host, _sessions) =
        stored_launch_whose_removals_are_decided_soon(&stored, &roster, kept.path()).await;
    // The deadline is a second and a half after the hold: it has passed.
    tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
    host.durable_decisions().notify_work();
    let at_its_deadline = match &held {
        Ok(decision_id) => phase_once(&host, decision_id, &[DecisionPhase::Expired]).await,
        Err(error) => Err(error.clone()),
    };
    let still_not_loaded = not_loaded(&host, &second_id).await;
    shut_down(&host, &[]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert_eq!(
        at_its_deadline,
        Ok(DecisionPhase::Expired),
        "the removal nobody decided ends expired while its mob is not loaded"
    );
    assert!(
        still_not_loaded,
        "an expiry loads no mob: the second mob is still not loaded"
    );
}

// ---- The held body's incarnation, and equals of the whole request ----

/// The body of the decision `decision_id`, as JSON.
fn held_body(host: &UnifiedRuntime, decision_id: &str) -> Result<serde_json::Value, String> {
    let row = decision_row(host, decision_id)?;
    serde_json::from_str(row.descriptor.request.get())
        .map_err(|error| format!("the held body is no JSON: {error}"))
}

/// A second mob's held body names that mob's incarnation, and the launch
/// mob's names none. A removal is held in each mob: the body of the second
/// mob's decision carries the incarnation minted when that mob was created,
/// so the record itself says which mob of the id the change is of; the
/// launch mob's body has the members it always had and no other.
#[tokio::test]
async fn a_second_mobs_held_body_names_its_mobs_incarnation_and_the_launch_mobs_names_none() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _roster, _definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = (
        scout_created_in(&host, None).await,
        scout_created_in(&host, Some(&second_id)).await,
    );
    let of_the_launch_mob = held_by(&scout_changed_in(&host, None, "remove").await)
        .and_then(|decision_id| held_body(&host, &decision_id));
    let of_the_second = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await)
        .and_then(|decision_id| held_body(&host, &decision_id));
    let incarnation = host
        .resolve_mob(Some(&second_id))
        .await
        .map(|mob| mob.mob_runtime().handle().incarnation().to_string())
        .map_err(|error| format!("{error:?}"));
    shut_down(&host, &[&second_id]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok(),
        "an agent of one name is created in each mob: {created:?}"
    );
    let incarnation = incarnation.expect("the second mob is served");
    let of_the_second = of_the_second.expect("the second mob's removal is held");
    assert_eq!(
        (
            of_the_second["mob_id"].as_str(),
            of_the_second["mob_incarnation"].as_str()
        ),
        (Some(second_id.as_str()), Some(incarnation.as_str())),
        "the second mob's held body names its mob and that mob's incarnation: {of_the_second}"
    );
    let of_the_launch_mob = of_the_launch_mob.expect("the launch mob's removal is held");
    let members = of_the_launch_mob
        .as_object()
        .map(|body| body.keys().map(String::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    assert_eq!(
        members,
        ["format", "mob_id", "request"],
        "the launch mob's held body has the members it always had: {of_the_launch_mob}"
    );
}

/// A held body that names no incarnation is of no mob of its id. The body
/// of a second mob's held removal is of that mob's incarnation and of no
/// other; the same body without the member reads as a body, and is of no
/// incarnation: the record does not say which mob of the id it is of. The
/// launch mob's body names none. A member this version does not know still
/// makes a body unreadable.
#[tokio::test]
async fn a_held_body_that_names_no_incarnation_is_of_no_mob_of_its_id() {
    use crate::identity_creation::HeldIdentityChange;

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _roster, _definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = (
        scout_created_in(&host, None).await,
        scout_created_in(&host, Some(&second_id)).await,
    );
    let of_the_launch_mob = held_by(&scout_changed_in(&host, None, "remove").await)
        .and_then(|decision_id| held_body(&host, &decision_id));
    let of_the_second = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await)
        .and_then(|decision_id| held_body(&host, &decision_id));
    let incarnation = host
        .resolve_mob(Some(&second_id))
        .await
        .map(|mob| mob.mob_runtime().handle().incarnation().to_string())
        .map_err(|error| format!("{error:?}"));
    shut_down(&host, &[&second_id]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok(),
        "an agent of one name is created in each mob: {created:?}"
    );
    let incarnation = incarnation.expect("the second mob is served");
    let as_written = of_the_second.expect("the second mob's removal is held");
    let read =
        |body: &serde_json::Value| serde_json::from_value::<HeldIdentityChange>(body.clone());

    let of_its_mob = read(&as_written).expect("the body as written is read");
    assert!(
        of_its_mob.is_of_incarnation(&incarnation),
        "the body as written is of its mob's incarnation"
    );
    assert!(
        !of_its_mob.is_of_incarnation(&format!("{incarnation}-of-a-later-mob")),
        "and of no other"
    );

    let mut without_the_member = as_written.clone();
    if let Some(body) = without_the_member.as_object_mut() {
        body.remove("mob_incarnation");
    }
    let of_no_mob = read(&without_the_member).expect("a body without the member is read");
    assert_eq!(of_no_mob.mob_incarnation, None);
    assert!(
        !of_no_mob.is_of_incarnation(&incarnation),
        "a body that names no incarnation is of none"
    );

    let of_the_launch_mob = of_the_launch_mob.expect("the launch mob's removal is held");
    assert_eq!(
        read(&of_the_launch_mob)
            .map(|body| body.mob_incarnation)
            .ok(),
        Some(None),
        "the launch mob's body names no incarnation"
    );

    let mut with_an_unknown_member = as_written;
    if let Some(body) = with_an_unknown_member.as_object_mut() {
        body.insert("of_a_later_version".to_string(), serde_json::json!(true));
    }
    assert!(
        read(&with_an_unknown_member).is_err(),
        "a member this version does not know makes the body unreadable"
    );
}

/// Among equally strict outcomes of a whole request in a second mob, the
/// one from a rule that names no agent goes first. An update changes an
/// agent's display name and its peers. A rule by name has a person decide
/// the display name of the agent of that name within a minute, where the
/// rules that name no agent allow it; a rule that names no agent has a
/// person decide a change of peers within an hour. The two are equally
/// strict, for two changes of one request. In the launch mob the first
/// declared of them gives the decision its time limit, as released; in a
/// second mob the rule that names no agent gives it.
#[tokio::test]
async fn among_equals_of_a_whole_request_a_rule_that_names_no_agent_goes_first_in_a_second_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let of_workers = |change: &str, target: serde_json::Value, outcome: serde_json::Value| {
        serde_json::json!({
            "change": [change],
            "requester": {"kind": "host", "surface": "rpc"},
            "target": target,
            "outcome": outcome,
        })
    };
    let policy = policy_with_rules(
        "deny",
        &[
            of_workers(
                "descriptive",
                serde_json::json!({"identity": [SCOUT]}),
                decided_for(APPROVERS, A_MINUTE),
            ),
            of_workers(
                "descriptive",
                serde_json::json!({"profile": ["worker"]}),
                serde_json::json!({"kind": "allow"}),
            ),
            of_workers(
                "peers",
                serde_json::json!({"profile": ["worker"]}),
                decided_for(APPROVERS, AN_HOUR),
            ),
        ],
        every_peer_allowed(),
    );
    let (host, second_id) = two_mobs(&temp, Some(policy)).await;
    let created = (
        scout_created_in(&host, None).await,
        scout_created_in(&host, Some(&second_id)).await,
    );
    let mut planned = Vec::new();
    for mob_id in [None, Some(&second_id)] {
        let read = asked_of_the_family(
            &host,
            mob_id,
            "meta/identity/get",
            serde_json::json!({"name": SCOUT}),
        )
        .await;
        let plan = match read {
            Ok(read) => {
                asked_as(
                    &host,
                    mob_id,
                    crate::rpc::identity_methods::stdio_requester(),
                    "meta/identity/plan",
                    serde_json::json!({
                        "change": "update",
                        "request_id": "plan-a-name-and-a-peer",
                        "name": SCOUT,
                        "expected_spec_digest": read["spec_digest"],
                        "set": {"display_name": "Scout"},
                        "peers_add": ["desk"],
                    }),
                )
                .await
            }
            Err(error) => Err(error),
        };
        planned.push(plan.map(|plan| {
            (
                plan["outcome"].as_str().map(str::to_string),
                plan["timeout_ms"].as_u64(),
            )
        }));
    }
    shut_down(&host, &[&second_id]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok(),
        "an agent of one name is created in each mob: {created:?}"
    );
    assert_eq!(
        planned,
        vec![
            Ok((Some("require_approval".to_string()), Some(A_MINUTE))),
            Ok((Some("require_approval".to_string()), Some(AN_HOUR))),
        ],
        "the launch mob takes the limit of the first declared of the equals; the second mob \
         takes the limit of the rule that names no agent"
    );
}

// ---- The notice of an ended change, and a launch that keeps agent memory ----

/// The transcript of the session the agent `desk` of the mob `mob_id` (the
/// launch mob for `None`) holds, read until it shows `shown` when `awaited`
/// and as it stands otherwise; empty when the agent's member holds no
/// session.
async fn transcript_of_the_desk(
    host: &UnifiedRuntime,
    sessions: &Sessions,
    mob_id: Option<&MobId>,
    shown: &str,
    awaited: bool,
) -> Result<String, String> {
    let mob = host
        .resolve_mob(mob_id)
        .await
        .map_err(|error| format!("the mob is not served: {error:?}"))?;
    let member = crate::identity_creation::core_member_name("desk")
        .ok_or_else(|| "the agent's name is no member name".to_string())?;
    let Some(session) = mob
        .mob_runtime()
        .handle()
        .resolve_bridge_session_id(&member)
        .await
    else {
        return Ok(String::new());
    };
    Ok(if awaited {
        transcript_showing(sessions, &session, shown).await
    } else {
        super::mob_entries_tests::transcript(sessions, &session).await
    })
}

/// The notice of a second mob's ended change goes to that mob's agent, and
/// to no agent of the same name in another mob. The agent `desk` of a
/// second mob asks for a create that a person decides; the person approves
/// it, the create is carried out in the second mob, and the agent that
/// asked is told in a turn of its own. The launch mob's agent `desk` is
/// told nothing.
#[tokio::test]
async fn the_notice_of_a_second_mobs_ended_change_goes_to_that_mobs_agent() {
    use meerkat_core::approval::decision::DecisionPhase;
    const TOLD: &str = "was approved and carried out";

    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, sessions) = launch_with_identities(&temp).await;
    decisions_kept_in(&mut host, temp.path());
    let host = Arc::new(host);
    // The route a gateway gives the family: per delivery, the identity
    // runtime of the mob the notice names, while the host runs it.
    let of_the_host = Arc::downgrade(&host);
    let notices = crate::identity_creation::KeyedSendNotices::per_mob(move |mob_id: &str| {
        let host = of_the_host.upgrade()?;
        host.identity_runtime()
            .filter(|identities| identities.owner().handle().mob_id().as_str() == mob_id)
            .or_else(|| host.identity_runtime_of_host_mob(mob_id))
    });
    assert!(
        host.attach_identity_policy(
            policy_with_rules(
                "deny",
                &[a_create_asked_by(
                    serde_json::json!({"kind": "agent"}),
                    decided_for(APPROVERS, AN_HOUR),
                )],
                every_peer_allowed(),
            ),
            crate::identity_creation::IdentityFamilyOptions::new()
                .with_decision_notices(Arc::new(notices)),
        ),
        "the launch attaches its identity policy"
    );
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(&roster)));
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&host, of_the_second).await;

    let held = held_by(
        &asked_as(
            &host,
            Some(&second_id),
            crate::identity_creation::ChangeRequester::agent_recorded("desk"),
            "meta/identity/create",
            the_create_of_a_runner(),
        )
        .await,
    );
    let ended = match &held {
        Ok(decision_id) => match approved(&host, decision_id).await {
            Ok(()) => phase_once(&host, decision_id, &[DecisionPhase::Executed]).await,
            Err(error) => Err(error),
        },
        Err(error) => Err(error.clone()),
    };
    let told_to_the_seconds_desk =
        transcript_of_the_desk(&host, &sessions, Some(&second_id), TOLD, true).await;
    let told_to_the_launch_mobs_desk =
        transcript_of_the_desk(&host, &sessions, None, TOLD, false).await;
    let listed = (
        listed_in(&host, None).await,
        listed_in(&host, Some(&second_id)).await,
    );
    shut_down(&host, &[&second_id]).await;
    assert!(
        held.is_ok(),
        "the create the second mob's agent asks is held for a person: {:?}",
        held.as_ref().err()
    );
    assert_eq!(
        ended,
        Ok(DecisionPhase::Executed),
        "the approved create is carried out"
    );
    assert_eq!(
        listed,
        (
            Ok(vec!["desk".to_string(), "front".to_string()]),
            Ok(vec![
                "back".to_string(),
                "desk".to_string(),
                "runner".to_string()
            ]),
        ),
        "the agent is made in the second mob alone"
    );
    assert!(
        told_to_the_seconds_desk
            .as_ref()
            .is_ok_and(|transcript| transcript.contains(TOLD)),
        "the second mob's agent that asked is told the end of its change: \
         {told_to_the_seconds_desk:?}"
    );
    assert!(
        told_to_the_launch_mobs_desk
            .as_ref()
            .is_ok_and(|transcript| !transcript.contains(TOLD)),
        "the launch mob's agent of the same name is told nothing: \
         {told_to_the_launch_mobs_desk:?}"
    );
}

/// What a row tells a created agent to remember in a turn.
fn told_to_remember(fact: &str) -> String {
    format!("Remember this: {fact}.")
}

const TOLD_TO_THE_LAUNCH_MOBS_SCOUT: &str = "the launch scout walks the north fence";
const TOLD_TO_THE_SECONDS_SCOUT: &str = "the second scout walks the south fence";

/// A members' model that calls its member's own `memory` tool once for the
/// newest fact its conversation tells it to remember
/// ([`told_to_remember`]), and answers every other request in text.
#[derive(Clone, Default)]
struct RemembersTheFence;

#[async_trait::async_trait]
impl meerkat_client::LlmClient for RemembersTheFence {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let rendered = serde_json::to_string(&request.messages).unwrap_or_default();
        let call = [TOLD_TO_THE_LAUNCH_MOBS_SCOUT, TOLD_TO_THE_SECONDS_SCOUT]
            .into_iter()
            .filter_map(|fact| rendered.rfind(&told_to_remember(fact)).map(|at| (at, fact)))
            .max_by_key(|(at, _)| *at)
            .and_then(|(at, fact)| {
                let id = format!("call-remember-{}", fact.replace(' ', "-"));
                let after = rendered.get(at..).unwrap_or_default();
                (!after.contains(&id)).then(|| {
                    (
                        id,
                        serde_json::json!({
                            "action": "remember",
                            "title": fact,
                            "body": format!("As told in a turn: {fact}."),
                        }),
                    )
                })
            });
        let (lead, stop_reason) = match call {
            Some((id, args)) => (
                meerkat_client::LlmEvent::ToolCallComplete {
                    id,
                    name: crate::identity_first::agent_memory::MEMORY_TOOL_NAME.to_string(),
                    args,
                    meta: None,
                },
                meerkat_core::StopReason::ToolUse,
            ),
            None => (
                meerkat_client::LlmEvent::TextDelta {
                    delta: "noted".to_string(),
                    meta: None,
                },
                meerkat_core::StopReason::EndTurn,
            ),
        };
        let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
            request,
            meerkat_core::Provider::OpenAI,
            stop_reason,
        );
        Box::pin(futures::stream::iter(vec![Ok(lead), Ok(usage), Ok(done)]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// The titles `store` holds for the agent [`SCOUT`] in `realm`, sorted,
/// read until one of them is `awaited` or thirty seconds passed: a turn's
/// tool call lands after its send.
async fn remembered_by_the_scout(
    store: &crate::memory::sqlite_store::SqliteAgentMemoryStore,
    realm: &str,
    awaited: &str,
) -> Vec<String> {
    use crate::identity_first::agent_memory::{
        AgentMemoryProvider, AgentMemoryRecallRequest, AgentMemorySelection,
    };

    let scout = crate::identity_first::AgentIdentity::parse(SCOUT).expect("identity");
    let mut titles = Vec::new();
    for _ in 0..300 {
        titles = store
            .recall(AgentMemoryRecallRequest {
                identity: scout.clone(),
                realm: realm.to_string(),
                query_text: None,
                query_terms: Vec::new(),
                selection: AgentMemorySelection::Always,
                max_entries: 64,
            })
            .await
            .expect("the store recalls")
            .into_iter()
            .map(|record| record.title)
            .collect::<Vec<_>>();
        titles.sort();
        if titles.iter().any(|title| title == awaited) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    titles
}

/// On a launch that keeps agent memory, an agent created in each of two
/// mobs through the family of its own mob keeps its memory in its own
/// mob's store. The stdio host creates an agent of one name in the launch
/// mob and in a second mob; each is told a fact in a turn sent through its
/// own mob's identity runtime and remembers it with the `memory` tool its
/// build bound. The launch mob's agent's record is in the launch's store
/// and the second mob's agent's in the second mob's own, and neither store
/// holds the other's.
#[tokio::test]
async fn an_agent_created_in_each_of_two_mobs_of_a_launch_that_keeps_memory_remembers_in_its_own_mobs_store()
 {
    use super::mob_memory_tests::{config_of_the_launch, launch_with_memory, with_a_second_mob};

    let temp = tempfile::tempdir().expect("tempdir");
    let kept = with_a_second_mob(
        launch_with_memory(&temp, config_of_the_launch(), |spec| {
            spec.with_options(crate::mob_handle_runtime::MobBootstrapOptions {
                allow_ephemeral_sessions: false,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(RemembersTheFence)),
            })
        })
        .await,
    )
    .await;
    let attached = kept.host.attach_identity_policy(
        the_host_changes_workers(),
        crate::identity_creation::IdentityFamilyOptions::new(),
    );
    let created = (
        scout_created_in(&kept.host, None).await,
        scout_created_in(&kept.host, Some(&kept.second_id)).await,
    );
    let scout = crate::identity_first::AgentIdentity::parse(SCOUT).expect("identity");
    let text = |fact: &str| meerkat_core::ContentInput::Text(told_to_remember(fact));
    let to_the_launch_mobs_scout = kept
        .launch_context
        .runtime
        .send(&scout, &text(TOLD_TO_THE_LAUNCH_MOBS_SCOUT))
        .await
        .map(|_| ())
        .map_err(|error| error.to_string());
    let to_the_seconds_scout = match kept.second_context().await {
        Some(second) => second
            .runtime
            .send(&scout, &text(TOLD_TO_THE_SECONDS_SCOUT))
            .await
            .map(|_| ())
            .map_err(|error| error.to_string()),
        None => Err("the second mob has no identity plane".to_string()),
    };
    let in_the_seconds_store = remembered_by_the_scout(
        &kept.of_the_second(),
        &kept.realm_of_the_second(),
        TOLD_TO_THE_SECONDS_SCOUT,
    )
    .await;
    let in_the_launchs_store = remembered_by_the_scout(
        &kept.of_the_launch,
        &kept.config.realm,
        TOLD_TO_THE_LAUNCH_MOBS_SCOUT,
    )
    .await;
    kept.shut_down().await;
    assert!(attached, "the launch attaches its identity policy");
    assert!(
        created.0.is_ok() && created.1.is_ok(),
        "an agent of one name is created in each mob: {created:?}"
    );
    assert!(
        to_the_launch_mobs_scout.is_ok() && to_the_seconds_scout.is_ok(),
        "each created agent takes a turn through its own mob's identity runtime: \
         {to_the_launch_mobs_scout:?} {to_the_seconds_scout:?}"
    );
    assert_eq!(
        in_the_seconds_store,
        [TOLD_TO_THE_SECONDS_SCOUT.to_string()],
        "what the second mob's created agent remembered is in the second mob's store, and \
         nothing of the launch mob's agent of the same name"
    );
    assert_eq!(
        in_the_launchs_store,
        [TOLD_TO_THE_LAUNCH_MOBS_SCOUT.to_string()],
        "what the launch mob's created agent remembered is in the launch's store, and nothing \
         of the second mob's agent of the same name"
    );
}

// ---- A launch that keeps agent memory, over stored state ----

/// A definition with one profile whose agents keep memory and are offered
/// the identity tools, under an id of its own.
fn definition_keeping_memory_with_identity_tools(tag: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"mob-family-memory-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n[profiles.worker.tools]\ncomms = true\nmob = true\nmemory = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition with one profile that keeps memory")
}

/// The message that makes the members' model call `tool` with `arguments`
/// once, marked `mark`.
fn call_asked(mark: &str, tool: &str, arguments: &serde_json::Value) -> String {
    format!("{mark} [[call {tool}]]{arguments}[[end]]")
}

/// One request the members' model was sent: the newest message that names
/// a call, the tool results that follow it, the tools it was offered, and
/// whether the model ended the turn on it.
#[derive(Debug, Clone)]
struct SentToTheModel {
    asked: String,
    answers: Vec<String>,
    offered: Vec<String>,
    ended: bool,
}

/// The members' model of these rows. It makes the one call the newest
/// message of its conversation names ([`call_asked`]), then ends the turn
/// in text, and keeps every request. A tool that is not among the tools a
/// request offers, while the tool that loads a deferred tool is, it loads
/// first and calls in the next round, as an agent does with a tool that
/// waits behind its session's catalog.
#[derive(Clone, Default)]
struct DirectedModel {
    sent: Arc<Mutex<Vec<SentToTheModel>>>,
    /// The messages for whose call the model loaded the tool first.
    loaded_for: Arc<Mutex<Vec<String>>>,
    /// The tools the model called, in the order of its calls.
    called: Arc<Mutex<Vec<String>>>,
}

impl DirectedModel {
    /// The tool results the model was sent back in the turn marked `mark`,
    /// once the model ended that turn, or after thirty seconds as many as
    /// there are. The last of them answers the call the message names.
    async fn answered_in(&self, mark: &str) -> Vec<String> {
        let newest = |ended_only: bool| {
            self.sent
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .rev()
                .find(|sent| sent.asked.contains(mark) && (sent.ended || !ended_only))
                .map(|sent| sent.answers.clone())
        };
        for _ in 0..300 {
            if let Some(answers) = newest(true) {
                return answers;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        newest(false).unwrap_or_default()
    }

    /// Every tool call the model made, by the tool's name, in order: the
    /// model's own record, of every member it answered for.
    fn calls_made(&self) -> Vec<String> {
        self.called
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// What the model was sent, in short, for a row's failure message: per
    /// request the start of the message that names a call, how many tool
    /// results followed it, and whether the `memory` tool, the tool
    /// `identity_create` and the tool that loads a deferred tool were
    /// offered.
    fn sent_in_short(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|sent| {
                let offered = |name: &str| sent.offered.iter().any(|tool| tool == name);
                format!(
                    "{:?} answers={} memory={} identity_create={} tool_catalog_load={}",
                    sent.asked.chars().take(60).collect::<String>(),
                    sent.answers.len(),
                    offered("memory"),
                    offered("identity_create"),
                    offered(LOADS_A_DEFERRED_TOOL),
                )
            })
            .collect()
    }
}

/// The tool an agent loads a deferred tool of its session's catalog with.
const LOADS_A_DEFERRED_TOOL: &str = "tool_catalog_load";

/// The call the text `shown` names, if it names one: the tool and its
/// arguments. A message that is no user message is read in its JSON form,
/// where the arguments are escaped.
fn call_named_in(shown: &str) -> Option<(String, serde_json::Value)> {
    let (_, rest) = shown.split_once("[[call ")?;
    let (tool, rest) = rest.split_once("]]")?;
    let (arguments, _) = rest.split_once("[[end]]")?;
    let arguments = serde_json::from_str(arguments).ok().or_else(|| {
        let unescaped: String = serde_json::from_str(&format!("\"{arguments}\"")).ok()?;
        serde_json::from_str(&unescaped).ok()
    })?;
    Some((tool.to_string(), arguments))
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for DirectedModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        // The newest message that names a call, whatever kind of message
        // carries it (a turn's message reaches the model in a notice of the
        // runtime, not in a user message), and the tool results after it.
        let mut asked = String::new();
        let mut answers: Vec<String> = Vec::new();
        for message in &request.messages {
            if let meerkat_core::Message::ToolResults { results, .. } = message {
                answers.extend(results.iter().map(meerkat_core::ToolResult::text_content));
                continue;
            }
            let shown = match message {
                meerkat_core::Message::User(user) => {
                    meerkat_core::types::text_content(&user.content)
                }
                other => serde_json::to_string(other).unwrap_or_default(),
            };
            if shown.contains("[[call ") {
                asked = shown;
                answers.clear();
            }
        }
        let offered: Vec<String> = request
            .tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        let offers = |name: &str| offered.iter().any(|tool| tool == name);
        let loaded_first = self
            .loaded_for
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&asked);
        // One call a turn: the tool the message names. A tool the request
        // does not offer is loaded first when the request offers the load,
        // and called in the round after it.
        let call = match (call_named_in(&asked), answers.len()) {
            (Some((tool, _)), 0) if !offers(&tool) && offers(LOADS_A_DEFERRED_TOOL) => {
                self.loaded_for
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(asked.clone());
                Some((
                    LOADS_A_DEFERRED_TOOL.to_string(),
                    serde_json::json!({"names": [tool]}),
                ))
            }
            (Some(named), 0) => Some(named),
            (Some(named), 1) if loaded_first => Some(named),
            _ => None,
        };
        if let Some((tool, _)) = &call {
            self.called
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(tool.clone());
        }
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(SentToTheModel {
                asked,
                answers,
                offered,
                ended: call.is_none(),
            });
        let (lead, stop_reason) = match call {
            Some((name, args)) => (
                meerkat_client::LlmEvent::ToolCallComplete {
                    id: "fc_0".to_string(),
                    name,
                    args,
                    meta: None,
                },
                meerkat_core::StopReason::ToolUse,
            ),
            None => (
                meerkat_client::LlmEvent::TextDelta {
                    delta: "noted".to_string(),
                    meta: None,
                },
                meerkat_core::StopReason::EndTurn,
            ),
        };
        let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
            request,
            meerkat_core::Provider::OpenAI,
            stop_reason,
        );
        Box::pin(futures::stream::iter(vec![Ok(lead), Ok(usage), Ok(done)]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// What outlives a launch that keeps agent memory: its stored state, the
/// directory of its memory, its roster provider and its members' model.
struct MemoryKeptOverLaunches {
    stored: super::mob_entries::lazy_start_tests::StoredHost,
    memory: tempfile::TempDir,
    roster: Arc<LaunchRoster>,
    model: DirectedModel,
}

impl MemoryKeptOverLaunches {
    fn new() -> Self {
        Self {
            stored: super::mob_entries::lazy_start_tests::StoredHost::of(
                definition_keeping_memory_with_identity_tools("host"),
            ),
            memory: tempfile::tempdir().expect("a memory directory"),
            roster: Arc::new(LaunchRoster::default()),
            model: DirectedModel::default(),
        }
    }

    /// A launch over the stored state that keeps agent memory
    /// ([`memory_kept_by`]) and attaches the identity policy `policy`. With
    /// it, the launch's own memory store and the realm of the launch mob's
    /// records.
    async fn launch(
        &self,
        policy: Arc<crate::identity_creation::IdentityCreationPolicy>,
    ) -> (
        UnifiedRuntime,
        crate::memory::sqlite_store::SqliteAgentMemoryStore,
        String,
    ) {
        let spec =
            self.stored
                .spec()
                .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
                    allow_ephemeral_sessions: false,
                    notify_orchestrator_on_resume: true,
                    default_llm_client: Some(Arc::new(self.model.clone())),
                });
        let mut host = self.stored.bootstrap(spec).await;
        let (store, realm) =
            memory_kept_by(&mut host, self.memory.path(), &self.roster, policy).await;
        (host, store, realm)
    }

    /// A second mob of `host` whose roster holds an agent `desk`, made from
    /// a definition whose agents keep memory.
    async fn second_mob_of(&self, host: &UnifiedRuntime) -> MobId {
        second_mob_keeping_memory(host, &self.roster).await
    }
}

/// A second mob of `host` whose roster holds an agent `desk`, made from a
/// definition whose agents keep memory and are offered the identity tools.
async fn second_mob_keeping_memory(host: &UnifiedRuntime, roster: &Arc<LaunchRoster>) -> MobId {
    let of_the_second = definition_keeping_memory_with_identity_tools("second");
    roster.configure(&of_the_second.id, &["desk"]);
    second_mob(host, of_the_second).await
}

/// `host` keeps agent memory as a gateway's launch does (the wiring of the
/// memory rows' fixture): its memory store is under `memory_dir`, its
/// launch mob's agent `desk` is built with the launch's memory, the launch
/// hands its memory over for the mobs it hosts beside its launch mob, and
/// it attaches the identity policy `policy`. Answers the launch's own
/// memory store and the realm of the launch mob's records.
async fn memory_kept_by(
    host: &mut UnifiedRuntime,
    memory_dir: &std::path::Path,
    roster: &Arc<LaunchRoster>,
    policy: Arc<crate::identity_creation::IdentityCreationPolicy>,
) -> (crate::memory::sqlite_store::SqliteAgentMemoryStore, String) {
    {
        use crate::identity_first::agent_memory::{AgentMemoryConfig, AgentMemorySelection};
        use crate::identity_first::{
            AgentMemoryCustomizer, AgentMemoryRuntimeInjector, AgentRuntimeServices,
            DurabilityPolicy, IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig,
            LocalContinuityStore, LocalLeaseProvider,
        };
        use crate::memory::coordinator::{MobScopeResolver, StaticMobBinding};

        let launch_mob = host.mob_handle().mob_id().clone();
        let config = AgentMemoryConfig {
            selection: AgentMemorySelection::Always,
            ..AgentMemoryConfig::default()
        };
        let store = crate::memory::sqlite_store::SqliteAgentMemoryStore::open(
            memory_dir.join("agent-memory"),
        )
        .expect("the launch's memory store");
        let stack = crate::memory_wiring::attach_memory_engines(
            Arc::new(store.clone()),
            &config,
            &crate::memory_wiring::MemoryEnginesConfig::default(),
            crate::memory_wiring::MemoryStackSeams {
                persistent_state: Some(memory_dir.to_path_buf()),
                transcript_store: Some(Arc::new(meerkat_store::MemoryStore::new())),
                event_sink: Some(host.memory_event_sink()),
                ..Default::default()
            },
        )
        .expect("the launch's memory stack");
        let resolver: Arc<dyn MobScopeResolver> = Arc::new(StaticMobBinding {
            realm: config.realm.clone(),
            mob: launch_mob.to_string(),
        });
        let customizer: Arc<dyn crate::identity_first::AgentCustomizer> = Arc::new(
            AgentMemoryCustomizer::wrap(None, stack.provider.clone(), config.clone())
                .with_mob_resolver(Some(Arc::clone(&resolver))),
        );
        let injector = AgentMemoryRuntimeInjector::new(stack.provider.clone(), config.clone())
            .with_taint_tracker(stack.taint.clone())
            .with_mob_resolver(Some(resolver));

        let handle = host.mob_runtime().handle().observer();
        let identity_runtime = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    LocalContinuityStore::in_memory().expect("continuity store"),
                ),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: "family-memory-launch".to_string(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(crate::gateway_wiring::identity_session_bridge(
                    host.mob_runtime(),
                    handle.clone(),
                ))),
                default_timeout: None,
            })
            .with_runtime_services(AgentRuntimeServices::new(handle.clone())),
        );
        identity_runtime.set_agent_memory(Some(injector)).await;
        identity_runtime
            .set_agent_customizer(Some(Arc::clone(&customizer)))
            .await;
        roster.configure(&launch_mob, &["desk"]);
        let launch_context = Arc::new(IdentityFirstRuntimeContext::new(
            identity_runtime,
            Arc::clone(roster) as Arc<dyn RosterProvider>,
            None,
            Some(customizer),
            Some(handle.definition().clone()),
        ));
        Box::pin(host.install_and_bootstrap_identity_first_context(
            launch_context,
            &[configured_agent("desk")],
        ))
        .await
        .expect("the launch mob's identity bootstrap");
        HostMobIdentities::new(Arc::clone(roster) as Arc<dyn RosterProvider>, true)
            .with_agent_memory_per_mob(Some(crate::gateway_wiring::HostMobMemory::new(
                store.clone(),
                config.clone(),
                launch_mob,
            )))
            .hand_over_to(host);
        assert!(
            host.attach_identity_policy(
                policy,
                crate::identity_creation::IdentityFamilyOptions::new()
            ),
            "the launch attaches its identity policy"
        );
        (store, config.realm)
    }
}

/// Send `text` to the agent `name` of the mob `mob_id` (the launch mob for
/// `None`) through that mob's own identity runtime.
async fn told(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
    name: &str,
    text: String,
) -> Result<(), String> {
    let mob = host
        .resolve_mob(mob_id)
        .await
        .map_err(|error| format!("the mob is not served: {error:?}"))?;
    let identities = mob
        .identity_runtime_ref()
        .cloned()
        .ok_or_else(|| "the mob has no identity runtime".to_string())?;
    let agent = crate::identity_first::AgentIdentity::parse(name).expect("identity");
    identities
        .send(&agent, &meerkat_core::ContentInput::Text(text))
        .await
        .map(|_| ())
        .map_err(|error| format!("the send was refused: {error}"))
}

/// The call of the `memory` tool that remembers `fact`.
fn remember(fact: &str) -> serde_json::Value {
    serde_json::json!({
        "action": "remember",
        "title": fact,
        "body": format!("As told in a turn: {fact}."),
    })
}

/// The titles `store` holds for the agent `name` in `realm`, sorted, read
/// until one of them is `awaited` or thirty seconds passed; read once when
/// nothing is awaited (`""`).
async fn remembered_by(
    store: &crate::memory::sqlite_store::SqliteAgentMemoryStore,
    realm: &str,
    name: &str,
    awaited: &str,
) -> Vec<String> {
    use crate::identity_first::agent_memory::{
        AgentMemoryProvider, AgentMemoryRecallRequest, AgentMemorySelection,
    };

    let agent = crate::identity_first::AgentIdentity::parse(name).expect("identity");
    let mut titles = Vec::new();
    for _ in 0..300 {
        titles = store
            .recall(AgentMemoryRecallRequest {
                identity: agent.clone(),
                realm: realm.to_string(),
                query_text: None,
                query_terms: Vec::new(),
                selection: AgentMemorySelection::Always,
                max_entries: 64,
            })
            .await
            .expect("the store recalls")
            .into_iter()
            .map(|record| record.title)
            .collect::<Vec<_>>();
        titles.sort();
        if awaited.is_empty() || titles.iter().any(|title| title == awaited) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    titles
}

/// The incarnation of the mob `mob_id` as the host's mob set holds it now:
/// what a row hands the memory store to make that mob's own store value.
async fn incarnation_of(
    host: &UnifiedRuntime,
    mob_id: &MobId,
) -> meerkat_core::service::MobIncarnationId {
    set_of(host)
        .handle_for(mob_id)
        .await
        .expect("the set serves the mob")
        .incarnation()
}

const THE_LAUNCH_MOBS_FACT: &str = "the launch scout counts the east gate";
const THE_SECONDS_FACT: &str = "the second scout counts the west gate";

/// A restart of a launch that keeps agent memory keeps each mob's created
/// agent's memory. An agent of one name is created in the launch mob and
/// in a second mob, and each remembers a fact in a turn. The host is shut
/// down and launched again over the same state and the same memory. Each
/// agent, asked in a turn of the later launch, recalls its own fact and
/// nothing of the other mob's agent of the same name.
#[tokio::test]
async fn a_restart_of_a_launch_that_keeps_memory_keeps_each_mobs_created_agents_memory() {
    let kept = MemoryKeptOverLaunches::new();
    let (first, store, realm) = kept.launch(the_host_changes_workers()).await;
    let second_id = kept.second_mob_of(&first).await;
    let realm_of_the_second = format!("mob.{second_id}");
    let created = (
        scout_created_in(&first, None).await,
        scout_created_in(&first, Some(&second_id)).await,
    );
    let told_before = (
        told(
            &first,
            None,
            SCOUT,
            call_asked(
                "the-launch-scout-is-told",
                "memory",
                &remember(THE_LAUNCH_MOBS_FACT),
            ),
        )
        .await,
        told(
            &first,
            Some(&second_id),
            SCOUT,
            call_asked(
                "the-second-scout-is-told",
                "memory",
                &remember(THE_SECONDS_FACT),
            ),
        )
        .await,
    );
    let kept_before = (
        remembered_by(&store, &realm, SCOUT, THE_LAUNCH_MOBS_FACT).await,
        remembered_by(
            &store
                .of_hosted_mob(&second_id, incarnation_of(&first, &second_id).await)
                .expect("the second mob's store"),
            &realm_of_the_second,
            SCOUT,
            THE_SECONDS_FACT,
        )
        .await,
    );
    shut_down(&first, &[]).await;
    drop(first);
    drop(store);

    let (host, _store, _realm) = kept.launch(the_host_changes_workers()).await;
    let recall = serde_json::json!({"action": "recall"});
    let asked_after = (
        told(
            &host,
            None,
            SCOUT,
            call_asked("the-launch-scout-recalls", "memory", &recall),
        )
        .await,
        told(
            &host,
            Some(&second_id),
            SCOUT,
            call_asked("the-second-scout-recalls", "memory", &recall),
        )
        .await,
    );
    let recalled_by_the_launch_mobs = kept.model.answered_in("the-launch-scout-recalls").await;
    let recalled_by_the_seconds = kept.model.answered_in("the-second-scout-recalls").await;
    shut_down(&host, &[]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok(),
        "an agent of one name is created in each mob: {created:?}"
    );
    assert!(
        told_before.0.is_ok() && told_before.1.is_ok(),
        "each created agent takes a turn before the restart: {told_before:?}"
    );
    assert_eq!(
        kept_before,
        (
            vec![THE_LAUNCH_MOBS_FACT.to_string()],
            vec![THE_SECONDS_FACT.to_string()]
        ),
        "each agent's fact is in its own mob's store before the restart; the model was sent {:?}",
        kept.model.sent_in_short()
    );
    assert!(
        asked_after.0.is_ok() && asked_after.1.is_ok(),
        "each created agent takes a turn after the restart: {asked_after:?}"
    );
    let holds = |recalled: &[String], fact: &str| recalled.iter().any(|text| text.contains(fact));
    assert!(
        holds(&recalled_by_the_launch_mobs, THE_LAUNCH_MOBS_FACT)
            && !holds(&recalled_by_the_launch_mobs, THE_SECONDS_FACT),
        "after the restart the launch mob's agent recalls its own fact and nothing of the second \
         mob's agent: {recalled_by_the_launch_mobs:?}"
    );
    assert!(
        holds(&recalled_by_the_seconds, THE_SECONDS_FACT)
            && !holds(&recalled_by_the_seconds, THE_LAUNCH_MOBS_FACT),
        "after the restart the second mob's agent recalls its own fact and nothing of the launch \
         mob's agent: {recalled_by_the_seconds:?}"
    );
}

/// An unload and a load of a second mob of a launch that keeps agent memory
/// keep its created agent's memory. The agent remembers a fact in a turn;
/// the mob set unloads the second mob; the turn that asks the agent to
/// recall loads the mob again, and the agent recalls its fact.
#[tokio::test]
async fn an_unload_and_a_load_of_a_second_mob_that_keeps_memory_keep_its_created_agents_memory() {
    let kept = MemoryKeptOverLaunches::new();
    let (host, store, _realm) = kept.launch(the_host_changes_workers()).await;
    let second_id = kept.second_mob_of(&host).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let told_before = told(
        &host,
        Some(&second_id),
        SCOUT,
        call_asked(
            "the-second-scout-is-told",
            "memory",
            &remember(THE_SECONDS_FACT),
        ),
    )
    .await;
    let kept_before = remembered_by(
        &store
            .of_hosted_mob(&second_id, incarnation_of(&host, &second_id).await)
            .expect("the second mob's store"),
        &format!("mob.{second_id}"),
        SCOUT,
        THE_SECONDS_FACT,
    )
    .await;
    // The mob set unloads a mob once it is idle: the turn settles first.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut unloaded = set_of(&host).unload_idle_mobs_for_tests().await;
    let mut asleep = not_loaded(&host, &second_id).await;
    while !asleep && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        unloaded = set_of(&host).unload_idle_mobs_for_tests().await;
        asleep = not_loaded(&host, &second_id).await;
    }
    let kept_by = if asleep {
        None
    } else {
        match set_of(&host).handle_for(&second_id).await {
            Ok(handle) => Some(format!("{:?}", handle.close_if_idle().await)),
            Err(error) => Some(format!("no handle: {error}")),
        }
    };
    let asked_after = told(
        &host,
        Some(&second_id),
        SCOUT,
        call_asked(
            "the-second-scout-recalls",
            "memory",
            &serde_json::json!({"action": "recall"}),
        ),
    )
    .await;
    let loaded_by_the_turn = !not_loaded(&host, &second_id).await;
    let recalled = kept.model.answered_in("the-second-scout-recalls").await;
    shut_down(&host, &[]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert!(
        told_before.is_ok(),
        "the created agent takes a turn before the unload: {told_before:?}"
    );
    assert_eq!(
        kept_before,
        [THE_SECONDS_FACT.to_string()],
        "the agent's fact is in the second mob's store before the unload; the model was sent {:?}",
        kept.model.sent_in_short()
    );
    assert!(
        asleep,
        "the mob set unloads the second mob: it unloaded {unloaded:?}; the mob is kept by \
         {kept_by:?}"
    );
    assert!(
        asked_after.is_ok() && loaded_by_the_turn,
        "the turn after the unload loads the second mob: {asked_after:?}"
    );
    assert!(
        recalled.iter().any(|text| text.contains(THE_SECONDS_FACT)),
        "after the load the second mob's agent recalls its fact: {recalled:?}"
    );
}

const THE_RUNNERS_FACT: &str = "the runner carries the green satchel";

/// An agent created through an agent's own identity tool in a second mob
/// of a launch that keeps agent memory remembers in that mob's store. The
/// launch installs its agents' mob tools as a gateway does, so an agent
/// whose profile has them is offered the identity tools. The agent `desk`
/// of the second mob creates an agent with `identity_create` in a turn
/// (its model loads the tool from its session's catalog first when the
/// request does not offer it), under a rule that lets an agent of its
/// profile create one; the created agent is told a fact in a turn and
/// remembers it. The record is in the second mob's store and not in the
/// launch's, and the agent is made in the second mob alone. By the
/// model's own record of its calls the creating turn holds the identity
/// tool alone, or a load of it and then the tool: both exact lists stand,
/// because whether a tool is offered in a request or waits behind the
/// session's catalog is not this family's to decide.
#[tokio::test]
async fn an_agent_created_with_an_agents_own_identity_tool_in_a_second_mob_remembers_in_that_mobs_store()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let memory = tempfile::tempdir().expect("a memory directory");
    let roster = Arc::new(LaunchRoster::default());
    let model = DirectedModel::default();
    let policy = policy_with_rules(
        "deny",
        &[a_create_asked_by(
            serde_json::json!({"kind": "agent", "profile": ["worker"]}),
            serde_json::json!({"kind": "allow"}),
        )],
        every_peer_allowed(),
    );
    let (mut host, _sessions) = launch_with_model(
        &temp,
        definition_keeping_memory_with_identity_tools("host"),
        Arc::new(model.clone()),
    )
    .await;
    let (store, realm) = memory_kept_by(&mut host, memory.path(), &roster, policy).await;
    let second_id = second_mob_keeping_memory(&host, &roster).await;
    let asked_of_the_desk = told(
        &host,
        Some(&second_id),
        "desk",
        call_asked(
            "the-desk-creates-a-runner",
            "identity_create",
            &serde_json::json!({
                "request_id": "a-runner",
                "agents": [{"name": "runner", "profile": "worker"}],
            }),
        ),
    )
    .await;
    let answered_to_the_desk = model.answered_in("the-desk-creates-a-runner").await;
    let calls_of_the_create = model.calls_made();
    let listed = (
        listed_in(&host, None).await,
        listed_in(&host, Some(&second_id)).await,
    );
    let told_to_the_runner = told(
        &host,
        Some(&second_id),
        "runner",
        call_asked("the-runner-is-told", "memory", &remember(THE_RUNNERS_FACT)),
    )
    .await;
    let in_the_seconds_store = remembered_by(
        &store
            .of_hosted_mob(&second_id, incarnation_of(&host, &second_id).await)
            .expect("the second mob's store"),
        &format!("mob.{second_id}"),
        "runner",
        THE_RUNNERS_FACT,
    )
    .await;
    let in_the_launchs_store = remembered_by(&store, &realm, "runner", "").await;
    let calls_made = model.calls_made();
    shut_down(&host, &[&second_id]).await;
    assert!(
        asked_of_the_desk.is_ok(),
        "the second mob's agent takes the turn that asks it to create: {asked_of_the_desk:?}"
    );
    assert!(
        answered_to_the_desk
            .last()
            .is_some_and(|answer| answer.contains("done") && !answer.contains("refused")),
        "the identity tool answers that the create is done: {answered_to_the_desk:?}; the model \
         called {calls_made:?} and was sent {:?}",
        model.sent_in_short()
    );
    // What the row saw, shown when an assertion below fails.
    eprintln!(
        "the row saw: calls up to the create {calls_of_the_create:?}; all calls {calls_made:?}; \
         listed {listed:?}; the runner's turn {told_to_the_runner:?}; the second mob's store \
         {in_the_seconds_store:?}; the launch's store {in_the_launchs_store:?}; the model was \
         sent {:?}",
        model.sent_in_short()
    );
    assert!(
        calls_of_the_create == ["identity_create"]
            || calls_of_the_create == ["tool_catalog_load", "identity_create"],
        "up to the end of the turn that creates, the members' model called the identity tool \
         (after a load of it from the session's catalog where the request does not offer it) \
         and made no other call: {calls_of_the_create:?}"
    );
    let calls_after_the_create: Vec<&str> = calls_made
        .iter()
        .skip(calls_of_the_create.len())
        .map(String::as_str)
        .filter(|tool| *tool != LOADS_A_DEFERRED_TOOL)
        .collect();
    assert_eq!(
        calls_after_the_create,
        ["memory"],
        "after it the model made one call more, the created agent's call of its memory tool \
         (a load of that tool aside): the whole row's calls are {calls_made:?}"
    );
    assert_eq!(
        listed,
        (
            Ok(vec!["desk".to_string()]),
            Ok(vec!["desk".to_string(), "runner".to_string()]),
        ),
        "the agent is made in the second mob alone"
    );
    assert!(
        told_to_the_runner.is_ok(),
        "the created agent takes a turn: {told_to_the_runner:?}"
    );
    assert_eq!(
        in_the_seconds_store,
        [THE_RUNNERS_FACT.to_string()],
        "what the created agent remembered is in the second mob's store; the model was sent {:?}",
        model.sent_in_short()
    );
    assert_eq!(
        in_the_launchs_store,
        Vec::<String>::new(),
        "and nothing of it is in the launch's store"
    );
}

// ---- A held body without the incarnation, through a launch ----

/// A held change whose body names no incarnation is carried out in no mob,
/// though it is filed under a second mob's own key. A removal of the second
/// mob's agent is held as the family holds it. A record of the same request
/// is then filed beside it, under the key that mob's holder derives for
/// another operation of the same requester, with a body that lacks the
/// incarnation, as a body written before the body carried it. A person's
/// approval of that record is tried. The record ends withdrawn by the
/// host's driver, or, when the approval came first, refused as a change of
/// a mob that was replaced: either way the second mob's agent stays, and
/// the removal the family holds is still open.
#[tokio::test]
async fn a_held_change_whose_body_names_no_incarnation_is_carried_out_in_no_mob() {
    use meerkat_core::approval::decision::{
        DecisionHoldRequest, DecisionOperationId, DecisionPhase, DecisionRequestBody,
        DecisionSubjectKey,
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _roster, _definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let held = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let incarnation = host
        .resolve_mob(Some(&second_id))
        .await
        .map(|mob| mob.mob_runtime().handle().incarnation().to_string())
        .map_err(|error| format!("{error:?}"));
    let filed_beside = held
        .clone()
        .and_then(|decision_id| decision_row(&host, &decision_id))
        .and_then(|row| {
            let incarnation = incarnation.clone()?;
            let descriptor = row.descriptor;
            let operation = format!("{}-of-a-body-without-it", descriptor.operation_id.as_str());
            let mut body: serde_json::Value = serde_json::from_str(descriptor.request.get())
                .map_err(|error| format!("the held body is no JSON: {error}"))?;
            let named = body
                .as_object_mut()
                .and_then(|members| members.remove("mob_incarnation"));
            if named.as_ref().and_then(serde_json::Value::as_str) != Some(incarnation.as_str()) {
                return Err(format!(
                    "the family's own body names {named:?}, not its mob's incarnation"
                ));
            }
            let hold = DecisionHoldRequest::new(
                descriptor.kind,
                DecisionSubjectKey::new(format!("{second_id}@{incarnation}/{operation}"))
                    .map_err(|error| format!("no subject key: {error:?}"))?,
                DecisionOperationId::new(operation.as_str())
                    .map_err(|error| format!("no operation id: {error:?}"))?,
                descriptor.request_digest,
                DecisionRequestBody::from_value(&body)
                    .map_err(|error| format!("no request body: {error:?}"))?,
                descriptor.requester,
                descriptor.approver_id,
                descriptor.deadline_at_ms,
                descriptor.rendering,
                descriptor.renderer,
            );
            host.durable_decisions()
                .owner()
                .hold(hold, false)
                .map(|row| row.decision_id.to_string())
                .map_err(|error| format!("the record is not filed: {error}"))
        });
    // The approval may find the record ended by the driver already: what
    // it answers is kept for the failure text only.
    let approval = match &filed_beside {
        Ok(decision_id) => approved(&host, decision_id).await,
        Err(error) => Err(error.clone()),
    };
    host.durable_decisions().notify_work();
    let ended = match &filed_beside {
        Ok(decision_id) => {
            phase_once(
                &host,
                decision_id,
                &[
                    DecisionPhase::Executed,
                    DecisionPhase::ExecutionRefused,
                    DecisionPhase::Expired,
                    DecisionPhase::Withdrawn,
                ],
            )
            .await
        }
        Err(error) => Err(error.clone()),
    };
    let refused_with = filed_beside.as_ref().ok().and_then(|decision_id| {
        decision_row(&host, decision_id).ok().and_then(|row| {
            row.execution_refusal
                .map(|refusal| refusal.as_str().to_string())
        })
    });
    let of_the_second = listed_in(&host, Some(&second_id)).await;
    let the_familys_own = match &held {
        Ok(decision_id) => phase_of(&host, decision_id),
        Err(error) => Err(error.clone()),
    };
    shut_down(&host, &[&second_id]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert!(
        held.is_ok(),
        "the removal is held: {:?}",
        held.as_ref().err()
    );
    assert!(
        filed_beside.is_ok(),
        "a record with a body that names no incarnation is filed under the second mob's key: \
         {filed_beside:?}"
    );
    assert!(
        matches!(
            (&ended, refused_with.as_deref()),
            (Ok(DecisionPhase::Withdrawn), None)
                | (Ok(DecisionPhase::ExecutionRefused), Some("mob_replaced"))
        ),
        "the record is carried out in no mob: it ends withdrawn, or refused as a change of a \
         mob that was replaced: {ended:?} {refused_with:?} (its approval answered {approval:?})"
    );
    assert!(
        of_the_second
            .as_ref()
            .is_ok_and(|names| names.contains(&SCOUT.to_string())),
        "the second mob's agent stays: {of_the_second:?}"
    );
    assert_eq!(
        the_familys_own,
        Ok(DecisionPhase::Open),
        "the removal the family holds for that mob is still open"
    );
}
