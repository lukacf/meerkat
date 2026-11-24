//! The record of who a mob a member created is private to, when the mob is
//! created under an id another mob held before: the later mob takes its own
//! creator's owner, and what the host owes before a create is asked first.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use meerkat_mob::{AgentIdentity, MobId, MobRuntimeMode};
use serde_json::json;

use super::child_mob_tests::{Host, lifetime, unique};
use super::*;
use crate::private_owners::{CREATE_RELEASE_FAILED, MobCreateRelease};

const PERSON_A: &str = "console-caller:v1:person-a";
const PERSON_B: &str = "console-caller:v1:person-b";

/// One runtime over a state directory of its own, with the agent mob tools.
async fn runtime_over(dir: &Path, host: &Host) -> MobRuntime {
    lifetime(dir, &unique("created-owner-root"), host, |spec| spec)
        .await
        .expect("lifetime")
}

/// Seat `member` in the runtime's own mob, private to `owner` when one is
/// given, and answer the session its turns run on.
async fn seat(
    runtime: &MobRuntime,
    member: &str,
    owner: Option<&str>,
) -> meerkat_core::types::SessionId {
    let identity = AgentIdentity::from(member);
    let mut spawn = SpawnMemberSpec::new(ProfileName::from("coordinator"), identity.clone());
    spawn.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    spawn.labels = owner.map(|owner| {
        BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            owner.to_string(),
        )])
    });
    Box::pin(runtime.handle().spawn_spec(spawn))
        .await
        .expect("seat the creating member");
    runtime
        .handle()
        .resolve_bridge_session_id(&identity)
        .await
        .expect("the creating member's session")
}

fn created_definition(mob_id: &MobId) -> MobDefinition {
    serde_json::from_value(json!({
        "id": mob_id.to_string(),
        "profiles": { "worker": {
            "model": "gpt-5.5",
            "runtime_mode": "turn_driven",
            "tools": { "comms": true }
        } }
    }))
    .expect("created mob definition")
}

/// Create the mob `mob_id` for the member whose session is `creator`, as
/// that member's own `mob_create` does.
async fn create(
    runtime: &MobRuntime,
    mob_id: &MobId,
    creator: &meerkat_core::types::SessionId,
) -> Result<MobId, MobError> {
    runtime
        .agent_mob_mcp_state()
        .expect("agent mob state")
        .mob_create_definition_with_owner_bridge_session(
            created_definition(mob_id),
            creator.clone(),
            true,
            false,
        )
        .await
}

async fn destroy(runtime: &MobRuntime, mob_id: &MobId) {
    runtime
        .agent_mob_mcp_state()
        .expect("agent mob state")
        .mob_destroy(mob_id)
        .await
        .expect("destroy the created mob");
}

/// Seat `member` in the created mob `mob_id` and answer who its roster row
/// says it is private to.
async fn seated_owner(runtime: &MobRuntime, mob_id: &MobId, member: &str) -> Option<String> {
    let state = runtime.agent_mob_mcp_state().expect("agent mob state");
    let identity = AgentIdentity::from(member);
    state
        .mob_spawn(
            mob_id,
            ProfileName::from("worker"),
            identity.clone(),
            Some(MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("seat a member of the created mob");
    let entry = state
        .handle_for(mob_id)
        .await
        .expect("the created mob")
        .get_member(&identity)
        .await
        .expect("read the roster")
        .expect("the seated member's row");
    crate::access::private_owner(&entry.labels).map(str::to_string)
}

async fn stand_down(runtime: MobRuntime) {
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    let _ = runtime
        .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(90))
        .await;
}

/// A mob a private member created is destroyed, and a member private to no
/// one creates a mob under the same id: the later mob is private to no one,
/// and so is a member seated in it, also one that takes a name the earlier
/// mob's member had.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mob_created_under_a_used_id_by_a_shared_member_seats_shared_members() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = runtime_over(dir.path(), &host).await;
    let owners = runtime.private_owners().clone();
    let private = seat(&runtime, "private-creator", Some(PERSON_A)).await;
    let shared = seat(&runtime, "shared-creator", None).await;
    let mob_id = MobId::from(unique("created-owner").as_str());

    create(&runtime, &mob_id, &private)
        .await
        .expect("the first create");
    assert_eq!(
        seated_owner(&runtime, &mob_id, "worker-a").await.as_deref(),
        Some(PERSON_A),
        "a member of a private member's mob is private to that member's person"
    );
    destroy(&runtime, &mob_id).await;

    create(&runtime, &mob_id, &shared)
        .await
        .expect("the create under the used id");
    assert_eq!(
        owners.owner_of(mob_id.as_str(), "worker-a").as_deref(),
        None,
        "a create forgets the members the record held under the id: a name the later mob has \
         not seated answers no earlier member's person"
    );
    assert_eq!(
        owners.recorded_mob_owner(mob_id.as_str()),
        Some(None),
        "the later mob is private to whoever its own creator is private to: no one"
    );
    assert_eq!(
        seated_owner(&runtime, &mob_id, "worker-a").await,
        None,
        "a member of the later mob is private to no one"
    );
    assert_eq!(
        owners.owner_of(mob_id.as_str(), "worker-a"),
        None,
        "the record answers the later member under a name the earlier mob's member had"
    );
    stand_down(runtime).await;
}

/// A mob a shared member created is destroyed, and a member private to a
/// person creates a mob under the same id: the later mob and a member
/// seated in it are private to that person.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mob_created_under_a_used_id_by_a_private_member_seats_that_persons_members() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = runtime_over(dir.path(), &host).await;
    let owners = runtime.private_owners().clone();
    let shared = seat(&runtime, "shared-creator", None).await;
    let private = seat(&runtime, "private-creator", Some(PERSON_B)).await;
    let mob_id = MobId::from(unique("created-owner").as_str());

    create(&runtime, &mob_id, &shared)
        .await
        .expect("the first create");
    assert_eq!(seated_owner(&runtime, &mob_id, "worker-a").await, None);
    destroy(&runtime, &mob_id).await;

    create(&runtime, &mob_id, &private)
        .await
        .expect("the create under the used id");
    assert_eq!(
        owners.recorded_mob_owner(mob_id.as_str()),
        Some(Some(PERSON_B.to_string())),
        "the later mob is private to its own creator's person"
    );
    assert_eq!(
        seated_owner(&runtime, &mob_id, "worker-a").await.as_deref(),
        Some(PERSON_B),
        "a member of the later mob is private to its creator's person"
    );
    stand_down(runtime).await;
}

/// The earlier mob was one person's member's and the later one another
/// person's member's: the later mob takes its own creator's person.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mob_created_under_a_used_id_takes_its_own_creators_owner() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = runtime_over(dir.path(), &host).await;
    let owners = runtime.private_owners().clone();
    let first = seat(&runtime, "first-creator", Some(PERSON_A)).await;
    let second = seat(&runtime, "second-creator", Some(PERSON_B)).await;
    let mob_id = MobId::from(unique("created-owner").as_str());

    create(&runtime, &mob_id, &first)
        .await
        .expect("the first create");
    destroy(&runtime, &mob_id).await;
    create(&runtime, &mob_id, &second)
        .await
        .expect("the create under the used id");
    assert_eq!(
        owners.recorded_mob_owner(mob_id.as_str()),
        Some(Some(PERSON_B.to_string())),
        "the later mob is private to its own creator's person"
    );
    assert_eq!(
        seated_owner(&runtime, &mob_id, "worker-a").await.as_deref(),
        Some(PERSON_B),
    );
    stand_down(runtime).await;
}

/// A release that fails while `fail` is set, and counts how often it was
/// asked.
struct Release {
    fail: AtomicBool,
    asked: AtomicUsize,
}

/// What the failing release says of its own failure: a caller is never
/// shown it.
const RELEASE_OWN_WORDS: &str = "the store at /a/host/path refused the write";

#[async_trait]
impl MobCreateRelease for Release {
    fn what(&self) -> &'static str {
        "the records of a test release"
    }

    async fn release_before_create(&self, _mob_id: &MobId) -> Result<(), String> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            Err(RELEASE_OWN_WORDS.to_string())
        } else {
            Ok(())
        }
    }
}

/// A create whose release fails is refused in the form a host's own create
/// refusal has, with the one fixed sentence and none of the release's own
/// words; nothing of the mob exists and the record holds nothing of it.
/// Asked again once the release passes, the create passes, and each create
/// asked the release once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_whose_release_fails_is_refused_and_passes_once_the_release_does() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = runtime_over(dir.path(), &host).await;
    let owners = runtime.private_owners().clone();
    let release = Arc::new(Release {
        fail: AtomicBool::new(true),
        asked: AtomicUsize::new(0),
    });
    owners.bind_create_release(release.clone());
    let creator = seat(&runtime, "private-creator", Some(PERSON_A)).await;
    let mob_id = MobId::from(unique("created-owner").as_str());
    let state = runtime.agent_mob_mcp_state().expect("agent mob state");

    let refused = create(&runtime, &mob_id, &creator)
        .await
        .expect_err("a create whose release fails is refused");
    assert!(
        meerkat_mob_mcp::MobMcpState::is_create_refusal(&refused),
        "{refused}"
    );
    let data = refused.structured_data().expect("the refusal's data");
    assert_eq!(data["kind"], meerkat_mob_mcp::MOB_CREATE_REFUSED_KIND);
    assert_eq!(data["reason"], CREATE_RELEASE_FAILED);
    assert!(
        !format!("{refused} {data}").contains(RELEASE_OWN_WORDS),
        "the release's own words stay with the host: {refused}"
    );
    assert!(
        matches!(
            state.handle_for(&mob_id).await,
            Err(MobError::MobNotFound(_))
        ),
        "nothing of the mob exists"
    );
    assert_eq!(
        owners.recorded_mob_owner(mob_id.as_str()),
        None,
        "the record holds nothing of a mob whose create was refused"
    );
    assert_eq!(release.asked.load(Ordering::SeqCst), 1);

    release.fail.store(false, Ordering::SeqCst);
    create(&runtime, &mob_id, &creator)
        .await
        .expect("the create passes once the release does");
    assert_eq!(
        release.asked.load(Ordering::SeqCst),
        2,
        "each create asks the release once"
    );
    assert_eq!(
        owners.recorded_mob_owner(mob_id.as_str()),
        Some(Some(PERSON_A.to_string()))
    );
    stand_down(runtime).await;
}

/// A first create under an id no mob held records its creator's owner: a
/// private member's person, and no one for a shared member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_create_records_its_creators_owner() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = runtime_over(dir.path(), &host).await;
    let owners = runtime.private_owners().clone();
    let private = seat(&runtime, "private-creator", Some(PERSON_A)).await;
    let shared = seat(&runtime, "shared-creator", None).await;
    let of_private = MobId::from(unique("created-owner").as_str());
    let of_shared = MobId::from(unique("created-owner").as_str());

    create(&runtime, &of_private, &private)
        .await
        .expect("a private member's create");
    create(&runtime, &of_shared, &shared)
        .await
        .expect("a shared member's create");
    assert_eq!(
        owners.recorded_mob_owner(of_private.as_str()),
        Some(Some(PERSON_A.to_string()))
    );
    assert_eq!(owners.recorded_mob_owner(of_shared.as_str()), Some(None));
    assert_eq!(
        seated_owner(&runtime, &of_private, "worker-a")
            .await
            .as_deref(),
        Some(PERSON_A)
    );
    assert_eq!(seated_owner(&runtime, &of_shared, "worker-a").await, None);
    stand_down(runtime).await;
}

/// A destroy alone changes nothing in the record: what it holds of the
/// destroyed mob and of its members stays until a mob is created under the
/// id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_destroy_leaves_the_record_of_the_mob_as_it_was() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = runtime_over(dir.path(), &host).await;
    let owners = runtime.private_owners().clone();
    let private = seat(&runtime, "private-creator", Some(PERSON_A)).await;
    let mob_id = MobId::from(unique("created-owner").as_str());

    create(&runtime, &mob_id, &private)
        .await
        .expect("the create");
    assert_eq!(
        seated_owner(&runtime, &mob_id, "worker-a").await.as_deref(),
        Some(PERSON_A)
    );
    destroy(&runtime, &mob_id).await;
    assert_eq!(
        owners.recorded_mob_owner(mob_id.as_str()),
        Some(Some(PERSON_A.to_string())),
        "a destroy releases nothing"
    );
    assert_eq!(
        owners.owner_of(mob_id.as_str(), "worker-a").as_deref(),
        Some(PERSON_A),
        "a destroyed mob's member is still known as its person's"
    );
    stand_down(runtime).await;
}

// ---------------------------------------------------------------------------
// The schedules a launch keeps under a mob's id, at a create under that id
// ---------------------------------------------------------------------------

/// A launch's schedule service over a store in memory, with the release of
/// a mob's schedules as a composition hands it to the runtime.
fn schedules_of(
    runtime: &MobRuntime,
) -> (
    meerkat::ScheduleService,
    Arc<crate::schedule_wiring::MobAgentSchedules>,
) {
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new()));
    let registry = crate::schedule_wiring::ScheduleMobTargetRegistry::default();
    registry.adopt_mob_runtime(
        runtime,
        &crate::member_key::DefaultMob::new(runtime.handle().mob_id().clone()),
    );
    let release = Arc::new(crate::schedule_wiring::MobAgentSchedules::new(
        service.clone(),
        registry,
    ));
    (service, release)
}

/// A schedule that fires once, a day from now, as a send to the member
/// `member` of the mob `mob_id`.
async fn schedule_for(
    service: &meerkat::ScheduleService,
    name: &str,
    mob_id: &MobId,
    member: &str,
) -> meerkat::ScheduleId {
    service
        .create(meerkat::CreateScheduleRequest {
            name: Some(name.to_owned()),
            description: None,
            trigger: meerkat::TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
            },
            target: meerkat::TargetBinding::Mob(Box::new(meerkat::MobTargetBinding::Member {
                mob_id: mob_id.to_string(),
                member_id: member.to_owned(),
                action: meerkat::ScheduledMobAction::Send {
                    content: meerkat_core::ContentInput::Text("Check in.".into()),
                    render_metadata: None,
                },
            })),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        })
        .await
        .expect("the schedule is created")
        .schedule_id
}

async fn phase(
    service: &meerkat::ScheduleService,
    id: &meerkat::ScheduleId,
) -> meerkat::SchedulePhase {
    service.get(id).await.expect("the schedule is read").phase
}

/// A runtime whose launch declared its schedules.
async fn declared_runtime(dir: &Path, host: &Host) -> MobRuntime {
    lifetime(dir, &unique("created-owner-root"), host, |spec| {
        spec.with_mob_schedules_declared()
    })
    .await
    .expect("lifetime")
}

/// A create under a mob's id ends every schedule that names that id: one
/// written for a mob that held the id before, and one written before any
/// mob held it. A schedule that names another mob stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_ends_the_schedules_that_name_its_id() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = declared_runtime(dir.path(), &host).await;
    let (service, release) = schedules_of(&runtime);
    runtime.mob_schedules().bind(release);
    let creator = seat(&runtime, "shared-creator", None).await;
    let used = MobId::from(unique("created-owner").as_str());
    let fresh = MobId::from(unique("created-owner").as_str());
    let beside = MobId::from(unique("created-owner").as_str());

    create(&runtime, &used, &creator)
        .await
        .expect("the first create");
    create(&runtime, &beside, &creator)
        .await
        .expect("the create of a mob beside it");
    let of_the_earlier = schedule_for(&service, "of-the-earlier", &used, "worker-a").await;
    let written_ahead = schedule_for(&service, "written-ahead", &fresh, "worker-a").await;
    let of_another = schedule_for(&service, "of-another", &beside, "worker-a").await;
    destroy(&runtime, &used).await;

    create(&runtime, &used, &creator)
        .await
        .expect("the create under the used id");
    create(&runtime, &fresh, &creator)
        .await
        .expect("the first create under an id a schedule names");
    assert_eq!(
        phase(&service, &of_the_earlier).await,
        meerkat::SchedulePhase::Deleted,
        "a schedule written for an earlier mob of the id is ended"
    );
    assert_eq!(
        phase(&service, &written_ahead).await,
        meerkat::SchedulePhase::Deleted,
        "a schedule written before any mob held the id is ended"
    );
    assert_eq!(
        phase(&service, &of_another).await,
        meerkat::SchedulePhase::Active,
        "a schedule that names another mob is as it was"
    );
    stand_down(runtime).await;
}

/// A launch that declared its schedules refuses a create until the release
/// of a mob's schedules is bound to its runtime, in the form a host's own
/// create refusal has and with the one fixed sentence; once bound, the
/// create passes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_waits_for_the_release_of_declared_schedules() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = declared_runtime(dir.path(), &host).await;
    let (_service, release) = schedules_of(&runtime);
    let creator = seat(&runtime, "shared-creator", None).await;
    let mob_id = MobId::from(unique("created-owner").as_str());

    let refused = create(&runtime, &mob_id, &creator)
        .await
        .expect_err("a create while the declared schedules' release is not bound");
    assert!(
        meerkat_mob_mcp::MobMcpState::is_create_refusal(&refused),
        "{refused}"
    );
    let data = refused.structured_data().expect("the refusal's data");
    assert_eq!(data["reason"], CREATE_RELEASE_FAILED);

    runtime.mob_schedules().bind(release);
    create(&runtime, &mob_id, &creator)
        .await
        .expect("the create passes once the release is bound");
    stand_down(runtime).await;
}

/// A release that cannot read the schedules refuses the create with the one
/// fixed sentence, and none of the store's own words reach the caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_is_refused_when_the_schedules_cannot_be_read() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = declared_runtime(dir.path(), &host).await;
    let registry = crate::schedule_wiring::ScheduleMobTargetRegistry::default();
    registry.adopt_mob_runtime(
        &runtime,
        &crate::member_key::DefaultMob::new(runtime.handle().mob_id().clone()),
    );
    runtime
        .mob_schedules()
        .bind(Arc::new(crate::schedule_wiring::MobAgentSchedules::new(
            meerkat::ScheduleService::new(Arc::new(meerkat::DisabledScheduleStore)),
            registry,
        )));
    let creator = seat(&runtime, "shared-creator", None).await;
    let mob_id = MobId::from(unique("created-owner").as_str());

    let refused = create(&runtime, &mob_id, &creator)
        .await
        .expect_err("a create whose schedules cannot be read");
    assert!(
        meerkat_mob_mcp::MobMcpState::is_create_refusal(&refused),
        "{refused}"
    );
    let data = refused.structured_data().expect("the refusal's data");
    assert_eq!(data["reason"], CREATE_RELEASE_FAILED);
    assert_eq!(
        refused.to_string(),
        format!("session error: mob create refused for mob id '{mob_id}': {CREATE_RELEASE_FAILED}"),
        "the answer is the fixed sentence and nothing of the store"
    );
    assert!(
        matches!(
            runtime
                .agent_mob_mcp_state()
                .expect("agent mob state")
                .handle_for(&mob_id)
                .await,
            Err(MobError::MobNotFound(_))
        ),
        "nothing of the mob exists"
    );
    stand_down(runtime).await;
}

/// A launch that declared no schedules creates as before: nothing is asked
/// and nothing is ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launch_without_declared_schedules_creates_as_before() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = runtime_over(dir.path(), &host).await;
    let (service, _release) = schedules_of(&runtime);
    let creator = seat(&runtime, "shared-creator", None).await;
    let mob_id = MobId::from(unique("created-owner").as_str());
    let named = schedule_for(&service, "named", &mob_id, "worker-a").await;

    create(&runtime, &mob_id, &creator)
        .await
        .expect("the create");
    assert_eq!(
        phase(&service, &named).await,
        meerkat::SchedulePhase::Active,
        "no release is bound, so no schedule is ended"
    );
    stand_down(runtime).await;
}

/// A destroy ends no schedule, and the release asked for the launch's own
/// mob ends none either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_destroy_and_the_launch_mob_end_no_schedule() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = declared_runtime(dir.path(), &host).await;
    let (service, release) = schedules_of(&runtime);
    runtime.mob_schedules().bind(release.clone());
    let creator = seat(&runtime, "shared-creator", None).await;
    let mob_id = MobId::from(unique("created-owner").as_str());
    let own = runtime.handle().mob_id().clone();

    create(&runtime, &mob_id, &creator)
        .await
        .expect("the create");
    let of_the_created = schedule_for(&service, "of-the-created", &mob_id, "worker-a").await;
    let of_the_launch = schedule_for(&service, "of-the-launch", &own, "shared-creator").await;
    destroy(&runtime, &mob_id).await;
    assert_eq!(
        phase(&service, &of_the_created).await,
        meerkat::SchedulePhase::Active,
        "a destroy releases nothing"
    );

    let released = release
        .release_mob(&own)
        .await
        .expect("the release asked for the launch's own mob");
    assert!(released.is_empty(), "{released:?}");
    assert_eq!(
        phase(&service, &of_the_launch).await,
        meerkat::SchedulePhase::Active,
        "the launch mob's schedules are never released"
    );
    stand_down(runtime).await;
}

// ---------------------------------------------------------------------------
// A launch whose schedule tools stand over an injected schedule store
// ---------------------------------------------------------------------------

/// A launch that is handed its schedule store owns the schedule service
/// over it, so it hands the release of a mob's schedules to its own
/// runtime: a create under a mob's id ends the schedules in that store
/// that name the id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launch_over_an_injected_schedule_store_releases_at_a_create() {
    let dir = tempfile::tempdir().expect("temporary store");
    let store: Arc<dyn meerkat::ScheduleStore> = Arc::new(meerkat::MemoryScheduleStore::new());
    let definition = MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
         runtime_mode = \"turn_driven\"\n[profiles.worker.tools]\ncomms = true\n",
        unique("created-owner-root")
    ))
    .expect("root definition");
    let mut spec = MobBootstrapSpec::ephemeral_runtime_backed_inner(
        definition,
        MobStorage::in_memory(),
        dir.path().to_path_buf(),
        4,
        None,
        Some(Arc::clone(&store)),
        None,
        None,
        CapabilityFlags::default(),
        None,
        None,
    )
    .expect("bootstrap spec");
    spec.options.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
        meerkat_core::Provider::OpenAI,
    )));
    let runtime = MobRuntime::bootstrap(spec).await.expect("root bootstrap");
    let service = meerkat::ScheduleService::new(store);

    let creator = AgentIdentity::from("creator");
    Box::pin(
        runtime
            .handle()
            .spawn_spec(SpawnMemberSpec::host_root("worker", creator.clone())),
    )
    .await
    .expect("seat the creating member");
    let session = runtime
        .handle()
        .resolve_bridge_session_id(&creator)
        .await
        .expect("the creating member's session");
    let mob_id = MobId::from(unique("created-owner").as_str());
    let own = runtime.handle().mob_id().clone();
    let named = schedule_for(&service, "named", &mob_id, "worker-a").await;
    let of_the_launch = schedule_for(&service, "of-the-launch", &own, "creator").await;

    create(&runtime, &mob_id, &session)
        .await
        .expect("the create");
    assert_eq!(
        phase(&service, &named).await,
        meerkat::SchedulePhase::Deleted,
        "the schedule in the injected store that names the id is ended"
    );
    // The launch's own mob id is never released there either.
    let crate::mob_schedules_release::MobSchedulesStanding::Bound(release) =
        runtime.mob_schedules().standing()
    else {
        panic!("the launch bound the release of its schedules");
    };
    assert_eq!(release.release_mob(&own).await, Ok(0));
    assert_eq!(
        phase(&service, &of_the_launch).await,
        meerkat::SchedulePhase::Active,
        "the launch mob's schedules are never released"
    );
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
}

// ---------------------------------------------------------------------------
// Two creates under one id
// ---------------------------------------------------------------------------

/// A release that parks its first asking until the row lets it go, and
/// counts its askings.
#[derive(Default)]
struct ParkedRelease {
    parked: AtomicBool,
    reached: AtomicBool,
    go: tokio::sync::Notify,
    asked: AtomicUsize,
}

#[async_trait]
impl MobCreateRelease for ParkedRelease {
    fn what(&self) -> &'static str {
        "the records of a test release"
    }

    async fn release_before_create(&self, _mob_id: &MobId) -> Result<(), String> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if !self.parked.swap(true, Ordering::SeqCst) {
            self.reached.store(true, Ordering::SeqCst);
            self.go.notified().await;
        }
        Ok(())
    }
}

/// Two members create a mob under one id at the same time: one create holds
/// the id, and the other is refused before the admission is asked for it.
/// The mob that stands is recorded for its own creator's person, and a
/// member seated in it is that person's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn of_two_creates_under_one_id_the_record_is_the_standing_mobs() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = runtime_over(dir.path(), &host).await;
    let owners = runtime.private_owners().clone();
    let release = Arc::new(ParkedRelease::default());
    owners.bind_create_release(release.clone());
    let private = seat(&runtime, "private-creator", Some(PERSON_A)).await;
    let shared = seat(&runtime, "shared-creator", None).await;
    let mob_id = MobId::from(unique("created-owner").as_str());
    let state = runtime.agent_mob_mcp_state().expect("agent mob state");

    // The private member's create parks inside its admission.
    let first = tokio::spawn({
        let (state, mob_id) = (Arc::clone(&state), mob_id.clone());
        async move {
            state
                .mob_create_definition_with_owner_bridge_session(
                    created_definition(&mob_id),
                    private,
                    true,
                    false,
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !release.reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the first create reaches its admission's release");
    // The shared member's create of the same id comes meanwhile.
    let second = tokio::time::timeout(
        Duration::from_mins(1),
        state.mob_create_definition_with_owner_bridge_session(
            created_definition(&mob_id),
            shared,
            true,
            false,
        ),
    )
    .await
    .expect("the second create is answered");
    release.go.notify_one();
    let first = tokio::time::timeout(Duration::from_mins(1), first)
        .await
        .expect("the first create is answered")
        .expect("first create task");

    // Whichever create stands, the record is its creator's.
    let standing = match (&first, &second) {
        (Ok(_), Err(_)) => Some(PERSON_A.to_string()),
        (Err(_), Ok(_)) => None,
        both => panic!("exactly one create stands: {both:?}"),
    };
    assert_eq!(
        owners.recorded_mob_owner(mob_id.as_str()),
        Some(standing.clone()),
        "the record is the standing mob's creator's"
    );
    assert_eq!(
        seated_owner(&runtime, &mob_id, "worker-a").await,
        standing,
        "a member seated in the standing mob takes its creator's person"
    );
    // The create that came second never reached the admission.
    assert!(
        first.is_ok(),
        "the create that held the id stands: {first:?}"
    );
    assert_eq!(
        release.asked.load(Ordering::SeqCst),
        1,
        "the admission was asked for the create that held the id alone"
    );
    stand_down(runtime).await;
}

/// The create tool on these mob tools says what a create does to schedules,
/// in one sentence, once, and no other tool carries the sentence: the agent
/// that writes a schedule for a mob reads its tools.
#[test]
fn the_create_tool_says_what_a_create_does_to_schedules() {
    let sentence = "A schedule that names a mob id is ended when a mob is created under that id: \
                    create the mob first, then write its schedules.";
    let def = |name: &str| {
        Arc::new(meerkat_core::types::ToolDef::new(
            name.to_string(),
            "What the tool does.",
            json!({"type": "object", "properties": {}}),
        ))
    };

    let create = idle_retire_patched_tool_def(&def("mob_create"));
    assert_eq!(
        create.description,
        format!("What the tool does.\n\nSCHEDULES:\n{sentence}"),
        "the create tool's description ends with the sentence"
    );
    let again = idle_retire_patched_tool_def(&create);
    assert_eq!(
        again.description, create.description,
        "a description that carries the sentence is left as it is"
    );
    assert_eq!(
        create.input_schema,
        def("mob_create").input_schema,
        "the create tool's arguments are untouched"
    );

    for other in [
        "delegate",
        "fork_off",
        "mob_spawn_member",
        "mob_destroy",
        "meerkat_schedule_create",
    ] {
        let patched = idle_retire_patched_tool_def(&def(other));
        assert!(
            !patched.description.contains("SCHEDULES:") && !patched.description.contains(sentence),
            "{other}: {}",
            patched.description
        );
    }
}

// ---------------------------------------------------------------------------
// A schedule that names a session, at a create under a mob's id
// ---------------------------------------------------------------------------

/// Seat `member` in the created mob `mob_id` and answer the session its
/// turns run on.
async fn seated_session(
    runtime: &MobRuntime,
    mob_id: &MobId,
    member: &str,
) -> meerkat_core::types::SessionId {
    let state = runtime.agent_mob_mcp_state().expect("agent mob state");
    let identity = AgentIdentity::from(member);
    state
        .mob_spawn(
            mob_id,
            ProfileName::from("worker"),
            identity.clone(),
            Some(MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("seat a member of the created mob");
    state
        .handle_for(mob_id)
        .await
        .expect("the created mob")
        .resolve_bridge_session_id(&identity)
        .await
        .expect("the seated member's session")
}

/// A schedule that fires once, a day from now, as a prompt to the session
/// `session`, in the form that follows the session when it is gone.
async fn schedule_for_session(
    service: &meerkat::ScheduleService,
    name: &str,
    session: &meerkat_core::types::SessionId,
) -> meerkat::ScheduleId {
    service
        .create(meerkat::CreateScheduleRequest {
            name: Some(name.to_owned()),
            description: None,
            trigger: meerkat::TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
            },
            target: meerkat::TargetBinding::session(
                meerkat::SessionTargetBinding::ResumableSession {
                    session_id: session.clone(),
                    action: meerkat::ScheduledSessionAction::Prompt {
                        prompt: meerkat_core::ContentInput::Text("Check in.".into()),
                        system_prompt: None,
                        render_metadata: None,
                        skill_refs: Vec::new(),
                        additional_instructions: Vec::new(),
                    },
                },
            ),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        })
        .await
        .expect("the schedule is created")
        .schedule_id
}

/// A schedule can name a session, and a fire follows a session that is gone
/// to the member its stored record names: a mob's id and a member's name.
/// So a schedule that names the session of a member of a mob of the id is
/// one of the schedules that name the id, and a create under the id ends it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_ends_the_schedules_that_name_a_session_of_a_member_of_its_id() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = declared_runtime(dir.path(), &host).await;
    let (service, release) = schedules_of(&runtime);
    runtime.mob_schedules().bind(release);
    let creator = seat(&runtime, "shared-creator", None).await;
    let used = MobId::from(unique("created-owner").as_str());

    create(&runtime, &used, &creator)
        .await
        .expect("the first create");
    let earlier = seated_session(&runtime, &used, "worker-a").await;
    let of_the_earlier = schedule_for_session(&service, "of-the-earlier", &earlier).await;
    destroy(&runtime, &used).await;

    create(&runtime, &used, &creator)
        .await
        .expect("the create under the used id");
    assert_eq!(
        phase(&service, &of_the_earlier).await,
        meerkat::SchedulePhase::Deleted,
        "a schedule that names the session of a member of an earlier mob of the id is ended"
    );
    stand_down(runtime).await;
}

/// A create under a mob's id leaves a schedule that names a session of a
/// member of another mob, of a member of the launch's own mob, and a
/// session of which nothing is stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_leaves_the_schedules_that_name_a_session_of_another_mob_or_of_none() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = declared_runtime(dir.path(), &host).await;
    let (service, release) = schedules_of(&runtime);
    runtime.mob_schedules().bind(release);
    let creator = seat(&runtime, "shared-creator", None).await;
    let created = MobId::from(unique("created-owner").as_str());
    let beside = MobId::from(unique("created-owner").as_str());

    create(&runtime, &beside, &creator)
        .await
        .expect("the create of a mob beside it");
    let of_another_mob = seated_session(&runtime, &beside, "worker-a").await;
    let kept = [
        (
            "of-another-mob",
            schedule_for_session(&service, "of-another-mob", &of_another_mob).await,
        ),
        (
            "of-the-launch-mob",
            schedule_for_session(&service, "of-the-launch-mob", &creator).await,
        ),
        (
            "of-none",
            schedule_for_session(&service, "of-none", &meerkat_core::types::SessionId::new()).await,
        ),
    ];

    create(&runtime, &created, &creator)
        .await
        .expect("the create under another id");
    for (name, id) in &kept {
        assert_eq!(
            phase(&service, id).await,
            meerkat::SchedulePhase::Active,
            "{name} is as it was"
        );
    }
    stand_down(runtime).await;
}

// ---------------------------------------------------------------------------
// The two gateways' wiring of the release
// ---------------------------------------------------------------------------

/// Both gateways say that their launch has schedules before its runtime is
/// built, and only when it has a schedule service; the composition's bind
/// comes after. Their launch functions are read here: no row builds a
/// gateway's own spec.
#[test]
fn both_gateways_declare_their_schedules_before_their_runtime_is_built() {
    let declaration = "        if schedule_host_inputs.is_some() {\n            \
                       spec = spec.with_mob_schedules_declared();\n        }\n";
    for (gateway, source) in [
        ("meta_gateway", include_str!("bin/meta_gateway.rs")),
        ("rpc_gateway", include_str!("rpc_gateway.rs")),
    ] {
        assert_eq!(
            source.matches(declaration).count(),
            1,
            "{gateway} declares its schedules once, under its schedule service"
        );
        let declared_at = source.find(declaration).expect("the declaration");
        let adopted_at = source
            .find("gateway_composition::adopt_schedule_mob_targets(")
            .expect("the composition's adoption of the schedules");
        assert!(
            declared_at < adopted_at,
            "{gateway} declares its schedules before its composition binds their release"
        );
    }
}
