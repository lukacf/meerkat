//! Rows of the schedules of a host that serves a second mob: a fire
//! addressed to a member of the second mob, and the schedule target
//! registry of the launch.
//!
//! The fixture is the declared launch of the entries' rows
//! ([`super::mob_entries_tests`]) with a gateway's schedule composition over
//! it: the schedule tools attached to the launch's session builder, the
//! registry adopted, and the schedule host spawned as a gateway spawns it.
//! Every launch lets a request resolve a second mob; no row opens anything
//! for it.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{
    CreateScheduleRequest, MobTargetBinding, Occurrence, OccurrencePhase, ScheduleService,
    ScheduledMobAction, TargetBinding, TriggerSpec,
};
use meerkat_mob::{MobId, SpawnMemberSpec};

use super::UnifiedRuntime;
use super::mob_entries::lazy_start_tests::{StoredHost, asleep, known};
use super::mob_entries_tests::{
    Sessions, configured_agent, definition, identity_context, launch, launch_over, second_mob,
    set_of, shut_down, transcript, transcript_showing,
};
use crate::console_schedules::{ConsoleScheduleSender, ConsoleScheduledSend};
use crate::gateway_composition::{
    GatewayScheduleHostInputs, ScheduleFiringRole, adopt_schedule_mob_targets,
    attach_gateway_schedule_tools, spawn_gateway_schedule_host,
};
use crate::identity_creation::{AgentSchedules, ScheduleEffect};
use crate::identity_first::AgentAddressability;
use crate::schedule_wiring::{MobAgentSchedules, SCHEDULE_STORE_FILE, ScheduleMobTargetRegistry};

/// How long a fire is followed to a terminal phase.
const FIRE_TERMINAL_WAIT: Duration = Duration::from_secs(20);

/// A host of two mobs with a gateway's schedule host over it.
struct ScheduledHost {
    host: UnifiedRuntime,
    sessions: Sessions,
    schedules: ScheduleService,
    schedule_host: Option<meerkat::surface::ScheduleHostHandle>,
    watchdog: tokio::task::JoinHandle<()>,
    second_id: MobId,
    second: meerkat_mob::MobHandle,
}

/// What a row's two mobs hold before the schedule host is spawned.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Desks {
    /// A plain member `desk` in each mob.
    Members,
    /// An identity plane in each mob, each with an agent `desk` that takes
    /// no sends from outside its mob.
    Agents,
}

impl ScheduledHost {
    /// The host, its second mob, a `desk` in each mob as `desks` says, and
    /// then the schedule host, spawned as a gateway spawns it.
    async fn boot(temp: &tempfile::TempDir, desks: Desks) -> Self {
        let schedule_state = temp.path().join("schedule-state");
        std::fs::create_dir_all(&schedule_state).expect("the schedule state directory");
        let (mut host, sessions, machine, attached) =
            launch_over(temp, definition("host"), |builder| {
                attach_gateway_schedule_tools(
                    builder,
                    &schedule_state,
                    ScheduleFiringRole::Executor,
                )
                .expect("the schedule tools attach")
            })
            .await;
        let second_id = second_mob(&host, definition("second")).await;
        let second = set_of(&host)
            .handle_for(&second_id)
            .await
            .expect("the set serves the second mob");
        match desks {
            Desks::Members => {
                Box::pin(host.spawn(SpawnMemberSpec::new("worker", "desk")))
                    .await
                    .expect("the default mob's desk");
                second
                    .spawn_spec(SpawnMemberSpec::new("worker", "desk"))
                    .await
                    .expect("the second mob's desk");
            }
            Desks::Agents => {
                let desk = || {
                    let mut desk = configured_agent("desk");
                    desk.addressability = AgentAddressability::InternalOnly;
                    desk
                };
                let of_the_default = identity_context(host.mob_runtime(), "default");
                Box::pin(
                    host.install_and_bootstrap_identity_first_context(of_the_default, &[desk()]),
                )
                .await
                .expect("the default mob's identity bootstrap");
                let of_the_second = {
                    let mob = host
                        .resolve_mob(Some(&second_id))
                        .await
                        .expect("the second mob of the host resolves");
                    let context = identity_context(mob.mob_runtime(), "second");
                    mob.attach_identity_first_context(Arc::clone(&context))
                        .await
                        .expect("a host attaches an identity context to its second mob");
                    context
                };
                Box::pin(of_the_second.bootstrap_roster(&[desk()]))
                    .await
                    .expect("the second mob's roster");
            }
        }
        let mob_state =
            adopt_schedule_mob_targets(&host, &attached.service, &attached.mob_target_registry)
                .await;
        let (schedule_host, watchdog) = spawn_gateway_schedule_host(
            &host,
            mob_state,
            GatewayScheduleHostInputs {
                schedule_service: attached.service.clone(),
                session_service: Arc::clone(&sessions),
                runtime_adapter: machine,
                schedule_store_path: schedule_state.join(SCHEDULE_STORE_FILE),
                firing_host_binding: attached.firing_host_binding.clone(),
                runnable_host: None,
                workgraph_service: None,
                owner_id: format!("mob-schedules-{}", uuid::Uuid::new_v4().simple()),
            },
        )
        .await;
        assert!(
            schedule_host.is_some(),
            "the schedule host spawns over the launch"
        );
        Self {
            host,
            sessions,
            schedules: attached.service,
            schedule_host,
            watchdog,
            second_id,
            second,
        }
    }

    /// One fire, due in a second, of a schedule whose target is the member
    /// `desk` of the mob `mob_id` with the prompt `word`, followed to a
    /// terminal phase or [`FIRE_TERMINAL_WAIT`].
    async fn fire_at_the_desk_of(&self, mob_id: &MobId, word: &str) -> Option<Occurrence> {
        let schedule = self
            .schedules
            .create(CreateScheduleRequest {
                name: Some(format!("a word for the desk of {mob_id}")),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() + chrono::Duration::seconds(1),
                },
                target: TargetBinding::Mob(Box::new(MobTargetBinding::Member {
                    mob_id: mob_id.to_string(),
                    member_id: crate::member_comms_id::mob_member_id("desk").to_string(),
                    action: ScheduledMobAction::Send {
                        content: meerkat_core::ContentInput::Text(word.to_string()),
                        render_metadata: None,
                    },
                })),
                misfire_policy: meerkat::MisfirePolicy::default(),
                overlap_policy: meerkat::OverlapPolicy::default(),
                missing_target_policy: meerkat::MissingTargetPolicy::default(),
                labels: BTreeMap::new(),
                planning_horizon_days: None,
                planning_horizon_occurrences: None,
            })
            .await
            .expect("the schedule is created");
        let deadline = tokio::time::Instant::now() + FIRE_TERMINAL_WAIT;
        loop {
            let occurrence = self
                .schedules
                .list_occurrences(&schedule.schedule_id)
                .await
                .expect("the occurrences are listed")
                .into_iter()
                .next();
            let terminal = occurrence.as_ref().is_some_and(|occurrence| {
                matches!(
                    occurrence.phase,
                    OccurrencePhase::Completed
                        | OccurrencePhase::Skipped
                        | OccurrencePhase::Misfired
                        | OccurrencePhase::Superseded
                        | OccurrencePhase::DeliveryFailed
                )
            });
            if terminal || tokio::time::Instant::now() >= deadline {
                return occurrence;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The sessions of the default mob's `desk` and of the second mob's.
    async fn desk_sessions(
        &self,
    ) -> (
        Option<meerkat_core::types::SessionId>,
        Option<meerkat_core::types::SessionId>,
    ) {
        let desk = crate::member_comms_id::mob_member_id("desk");
        (
            self.host
                .mob_handle()
                .resolve_bridge_session_id(&desk)
                .await,
            self.second.resolve_bridge_session_id(&desk).await,
        )
    }

    /// What the fire `fired` left, read once its word shows in a `desk` or
    /// ten seconds passed.
    async fn left_by(&self, fired: Option<Occurrence>, word: &str) -> Fired {
        let (in_the_default, in_the_second) = self.desk_sessions().await;
        assert!(
            in_the_default.is_some() && in_the_second.is_some() && in_the_default != in_the_second,
            "two members, one name: {in_the_default:?}, {in_the_second:?}"
        );
        let mut got = (false, false);
        if let (Some(in_the_default), Some(in_the_second)) = (in_the_default, in_the_second) {
            for _ in 0..100 {
                got = (
                    transcript(&self.sessions, &in_the_default)
                        .await
                        .contains(word),
                    transcript(&self.sessions, &in_the_second)
                        .await
                        .contains(word),
                );
                if got.0 || got.1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        Fired {
            phase: fired
                .as_ref()
                .map(|occurrence| format!("{:?}", occurrence.phase)),
            the_default_mobs_desk_got_it: got.0,
            the_second_mobs_desk_got_it: got.1,
        }
    }

    async fn stop(self) {
        if let Some(schedule_host) = self.schedule_host {
            schedule_host.shutdown().await;
        }
        self.watchdog.abort();
        let _ = self.watchdog.await;
        shut_down(&self.host, &[&self.second_id]).await;
    }
}

/// What a fire left: the phase of its occurrence when it was last read, and
/// which `desk` holds its word.
#[derive(Debug, PartialEq, Eq)]
struct Fired {
    phase: Option<String>,
    the_default_mobs_desk_got_it: bool,
    the_second_mobs_desk_got_it: bool,
}

fn completed() -> Option<String> {
    Some(format!("{:?}", OccurrencePhase::Completed))
}

/// A schedule for a member of a second mob of the host fires at that mob's
/// member: the fire is delivered through that mob's own internal lane, as a
/// fire for a member of the launch's mob is. It completes at its delivery,
/// a member that takes no turn from outside its mob gets the word, and the
/// launch mob's member of the same name does not.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_for_a_member_of_a_second_mob_fires_at_that_mobs_member() {
    const WORD: &str = "the scheduled word for the desk of the second mob";
    let temp = tempfile::tempdir().expect("tempdir");
    let hosted = ScheduledHost::boot(&temp, Desks::Members).await;
    let fired = hosted.fire_at_the_desk_of(&hosted.second_id, WORD).await;
    let left = hosted.left_by(fired, WORD).await;
    assert_eq!(
        left,
        Fired {
            phase: completed(),
            the_default_mobs_desk_got_it: false,
            the_second_mobs_desk_got_it: true,
        }
    );
    hosted.stop().await;
}

/// A schedule for an agent of a second mob fires at that mob's agent: with
/// an identity plane in each mob and an agent of one name in each, the fire
/// for the second mob's agent completes at its delivery and reaches that
/// agent's session, not the launch mob's agent of the same name.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_for_an_agent_of_a_second_mob_fires_at_that_mobs_agent() {
    const WORD: &str = "the scheduled word for the agent of the second mob";
    let temp = tempfile::tempdir().expect("tempdir");
    let hosted = ScheduledHost::boot(&temp, Desks::Agents).await;
    let fired = hosted.fire_at_the_desk_of(&hosted.second_id, WORD).await;
    let left = hosted.left_by(fired, WORD).await;
    assert_eq!(
        left,
        Fired {
            phase: completed(),
            the_default_mobs_desk_got_it: false,
            the_second_mobs_desk_got_it: true,
        }
    );
    hosted.stop().await;
}

/// A fire for a member of the launch's mob is delivered as before on a host
/// of two mobs: it completes at its delivery and reaches the launch mob's
/// member, not the second mob's member of the same name.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_for_a_member_of_the_launch_mob_still_fires_at_its_member() {
    const WORD: &str = "the scheduled word for the desk of the launch mob";
    let temp = tempfile::tempdir().expect("tempdir");
    let hosted = ScheduledHost::boot(&temp, Desks::Members).await;
    let default_id = hosted.host.mob_handle().mob_id().clone();
    let fired = hosted.fire_at_the_desk_of(&default_id, WORD).await;
    let left = hosted.left_by(fired, WORD).await;
    assert_eq!(
        left,
        Fired {
            phase: completed(),
            the_default_mobs_desk_got_it: true,
            the_second_mobs_desk_got_it: false,
        }
    );
    hosted.stop().await;
}

/// Every launch serves the second mob of its host to its schedule host: on
/// a runtime as a launch builds it, where the row sets nothing, a fire for
/// a second mob's member takes that mob's own internal lane. It completes
/// at its delivery, a member that takes no turn from outside its mob gets
/// the word, and the launch mob's member of the same name does not.
#[tokio::test(flavor = "multi_thread")]
async fn a_fire_for_a_second_mobs_member_takes_that_mobs_lane_on_a_runtime_as_a_launch_builds_it() {
    const WORD: &str = "a scheduled word every launch delivers inside the second mob";
    let temp = tempfile::tempdir().expect("tempdir");
    let hosted = ScheduledHost::boot(&temp, Desks::Members).await;
    let fired = hosted.fire_at_the_desk_of(&hosted.second_id, WORD).await;
    let left = hosted.left_by(fired, WORD).await;
    assert_eq!(
        left,
        Fired {
            phase: completed(),
            the_default_mobs_desk_got_it: false,
            the_second_mobs_desk_got_it: true,
        }
    );
    hosted.stop().await;
}

/// The target of a Console caller's schedule for the agent `agent` of the
/// host's default mob `default_mob`: a send that names no mob.
fn console_schedule_of_the_default_mobs(default_mob: &MobId, agent: &str) -> TargetBinding {
    let default_mob = crate::member_key::DefaultMob::new(default_mob.clone());
    ConsoleScheduledSend::new(
        &default_mob,
        default_mob.mob_id(),
        agent.to_owned(),
        "Post the morning summary.".to_owned(),
        ConsoleScheduleSender::Anonymous,
        0,
    )
    .target()
    .expect("a console schedule's target")
}

/// How many schedules the follow `following` plans to pause for the agent
/// `agent` of the mob `mob_id`.
async fn planned_for(following: &MobAgentSchedules, mob_id: &MobId, agent: &str) -> usize {
    let planned = following
        .planned(mob_id.as_str(), agent, ScheduleEffect::Paused)
        .await;
    assert!(!planned.incomplete, "the plan is whole: {planned:?}");
    planned.count
}

/// A schedule target registry keeps the default mob of its host: the first
/// one its host states. After it adopted the launch's wrapper, an adoption
/// of the wrapper of a second mob of the host that states the second mob as
/// the default mob is refused and changes nothing: a stored Console
/// schedule that names no mob stays the schedule of the launch mob's agent,
/// and is not the schedule of the second mob's agent of the same name. An
/// adoption of that wrapper that states the launch's mob keeps it so.
#[tokio::test]
async fn a_schedule_registry_keeps_its_first_default_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let default_id = host.mob_handle().mob_id().clone();
    let second_id = second_mob(&host, definition("second")).await;
    let the_launchs = crate::member_key::DefaultMob::new(default_id.clone());

    let registry = ScheduleMobTargetRegistry::default();
    registry.adopt_mob_runtime(host.mob_runtime(), &the_launchs);
    let schedules = ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new()));
    schedules
        .create(CreateScheduleRequest {
            name: Some("the summary of alpha".to_owned()),
            description: None,
            trigger: TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
            },
            target: console_schedule_of_the_default_mobs(&default_id, "alpha"),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        })
        .await
        .expect("the console schedule is stored");
    let following = MobAgentSchedules::new(schedules, registry.clone());
    assert_eq!(
        (
            planned_for(&following, &default_id, "alpha").await,
            planned_for(&following, &second_id, "alpha").await,
        ),
        (1, 0),
        "before a second adoption the schedule is the launch mob's agent's"
    );

    {
        let second = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        registry.adopt_mob_runtime(
            second.mob_runtime(),
            &crate::member_key::DefaultMob::new(second_id.clone()),
        );
    }
    assert_eq!(
        (
            planned_for(&following, &default_id, "alpha").await,
            planned_for(&following, &second_id, "alpha").await,
        ),
        (1, 0),
        "after the refused adoption of a second mob's wrapper the schedule is still the launch \
         mob's agent's, and not the second mob's agent's"
    );

    {
        let second = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        registry.adopt_mob_runtime(second.mob_runtime(), &the_launchs);
    }
    assert_eq!(
        (
            planned_for(&following, &default_id, "alpha").await,
            planned_for(&following, &second_id, "alpha").await,
        ),
        (1, 0),
        "an adoption that states the launch's mob keeps the default mob"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A launch composed as a gateway composes one that has a schedule service:
/// its schedules are declared before its runtime is built, under
/// `authority`; and the schedule service whose targets it then adopts, as
/// the gateways' composition does once the runtime is up.
async fn launch_adopting_its_schedules(
    temp: &tempfile::TempDir,
    authority: crate::mob_composition_manifest::CompositionAuthority,
) -> (UnifiedRuntime, ScheduleService) {
    let (host, _sessions, _machine, ()) = super::mob_entries_tests::launch_composed(
        temp,
        definition("host"),
        meerkat::Config::default(),
        |_| (),
        move |spec| {
            spec.with_mob_schedules_declared()
                .with_composition_authority(authority)
        },
    )
    .await;
    let schedules = ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new()));
    let registry = ScheduleMobTargetRegistry::default();
    adopt_schedule_mob_targets(&host, &schedules, &registry).await;
    (host, schedules)
}

/// A schedule of `schedules` that names the member `scout` of the mob
/// `mob_id`, due a day from now: its id. It stands for a schedule an earlier
/// mob of that id left.
async fn schedule_that_names(schedules: &ScheduleService, mob_id: &MobId) -> meerkat::ScheduleId {
    schedules
        .create(CreateScheduleRequest {
            name: Some("of an earlier mob".to_string()),
            description: None,
            trigger: TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
            },
            target: TargetBinding::Mob(Box::new(MobTargetBinding::Member {
                mob_id: mob_id.to_string(),
                member_id: "scout".to_string(),
                action: ScheduledMobAction::Send {
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
        .expect("create the schedule")
        .schedule_id
}

/// The launch that speaks for its composition hands its runtime the release
/// of a mob's schedules when it adopts its schedule targets: the create of a
/// mob beside the launch mob then ends a schedule that names that mob's id,
/// before the mob exists.
#[tokio::test(flavor = "multi_thread")]
async fn a_launchs_create_of_a_second_mob_ends_the_schedules_under_its_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, schedules) = launch_adopting_its_schedules(
        &temp,
        crate::mob_composition_manifest::CompositionAuthority::Authoritative,
    )
    .await;
    let of_the_second = definition("second");
    let earlier = schedule_that_names(&schedules, &of_the_second.id).await;

    let second_id = second_mob(&host, of_the_second).await;

    let after = schedules.get(&earlier).await.expect("the schedule is read");
    assert_eq!(
        after.phase,
        meerkat::SchedulePhase::Deleted,
        "the create ended the schedule an earlier mob of the id left"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A candidate launch declares its schedules and never binds their release:
/// its create of a mob beside the launch mob is refused, no mob stands
/// under the id, and the schedules are as they were. The host is answered
/// a refused create with the one sentence of a failed release; why the
/// release was not made is in the host's log.
#[tokio::test(flavor = "multi_thread")]
async fn a_candidate_launchs_create_of_a_second_mob_is_refused_and_ends_no_schedule() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, schedules) = launch_adopting_its_schedules(
        &temp,
        crate::mob_composition_manifest::CompositionAuthority::NonAuthoritative,
    )
    .await;
    let of_the_second = definition("second");
    let second_id = of_the_second.id.clone();
    let earlier = schedule_that_names(&schedules, &second_id).await;

    let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;

    assert!(
        matches!(
            &created,
            Err(error) if meerkat_mob_mcp::MobMcpState::is_create_refusal(error)
                && error.to_string().contains(crate::private_owners::CREATE_RELEASE_FAILED)
        ),
        "a candidate launch's create of a second mob is refused: {:?}",
        created.as_ref().map(|_| "created")
    );
    assert!(
        set_of(&host).handle_for(&second_id).await.is_err(),
        "no mob stands under the id"
    );
    let after = schedules.get(&earlier).await.expect("the schedule is read");
    assert_eq!(
        after.phase,
        meerkat::SchedulePhase::Active,
        "the schedule is as it was"
    );
    shut_down(&host, &[]).await;
}

/// A runtime the builder makes over a schedule store its embedder hands it
/// has schedules of its own: it says so before its mob is built and hands
/// itself their release once it is up, as a gateway's composition does. A
/// member's create of a mob under an id an earlier mob had then ends the
/// schedules that still name that id in the embedder's store, before the
/// mob exists.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_create_on_a_runtime_the_builder_makes_ends_the_schedules_under_its_id() {
    let state = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn meerkat::ScheduleStore> = Arc::new(meerkat::MemoryScheduleStore::new());
    let host = UnifiedRuntime::builder()
        .definition(definition("host"))
        .persistent_state(state.path())
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .schedule_store(Arc::clone(&store))
        .build()
        .await
        .expect("a runtime the builder makes over a schedule store");
    // The embedder's own view of the store the runtime's schedule tools
    // write.
    let schedules = ScheduleService::new(store);
    let seated = Box::pin(
        host.mob_handle()
            .spawn_spec(SpawnMemberSpec::new("worker", "bob")),
    )
    .await;
    assert!(
        seated.is_ok(),
        "a member of the launch mob: {:?}",
        seated.as_ref().err()
    );
    let creator = host
        .mob_handle()
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("bob"))
        .await
        .expect("the member's session");
    let of_the_mob = definition("second");
    let again = of_the_mob.clone();

    let id = Box::pin(
        set_of(&host).mob_create_definition_with_owner_bridge_session(
            of_the_mob,
            creator.clone(),
            true,
            false,
        ),
    )
    .await
    .expect("the member's mob");
    let earlier = schedule_that_names(&schedules, &id).await;
    let destroyed = Box::pin(set_of(&host).mob_destroy(&id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the mob: {:?}",
        destroyed.as_ref().err()
    );
    let created_again = Box::pin(
        set_of(&host).mob_create_definition_with_owner_bridge_session(again, creator, true, false),
    )
    .await
    .expect("the member's mob under the same id");
    assert_eq!(created_again, id, "the mob is created under the same id");

    let after = schedules.get(&earlier).await.expect("the schedule is read");
    assert_eq!(
        after.phase,
        meerkat::SchedulePhase::Deleted,
        "the member's create ended the schedule the destroyed mob of the id left"
    );
    shut_down(&host, &[&id]).await;
}

// A schedule that names no mob, on a host of two mobs with a member of one
// name in each: whose it is, at a verb that names it by id, at a write and
// at a fire.

/// The person the rows' private `desk` is private to.
const DESKS_OWNER: &str = "console-caller:v1:dd";

/// Which mob's `desk` is private to a person.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PrivateDesk {
    OfTheLaunchMob,
    OfTheSecondMob,
}

/// A host of two mobs with a member `desk` in each, one of them private to
/// a person, and a `clerk` of the launch mob that is private to no one: the
/// agents' schedule tools over a store of the row's, as the clerk's turn
/// calls them. No schedule host fires over that store, so a create passes
/// the tools' judges and is stored; on a host that fires, the schedule
/// host's own probe refuses the create of a target that names an agent and
/// no mob after those judges.
struct TwoDesks {
    host: UnifiedRuntime,
    second_id: MobId,
    schedules: ScheduleService,
    tools: Arc<dyn meerkat_core::AgentToolDispatcher>,
    clerk: meerkat_core::types::SessionId,
}

impl TwoDesks {
    async fn boot(temp: &tempfile::TempDir, private: PrivateDesk) -> Self {
        let (host, _sessions, _machine, ()) = launch_over(temp, definition("host"), |_| ()).await;
        let second_id = second_mob(&host, definition("second")).await;
        let second = set_of(&host)
            .handle_for(&second_id)
            .await
            .expect("the set serves the second mob");
        let desk = |of: PrivateDesk| {
            let spec = SpawnMemberSpec::new("worker", "desk");
            if of == private {
                spec.with_labels(BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    DESKS_OWNER.to_string(),
                )]))
            } else {
                spec
            }
        };
        // Through the mob's handle: a caller's spawn through the runtime may
        // not supply the label that says whom a member is private to.
        Box::pin(
            host.mob_handle()
                .spawn_spec(desk(PrivateDesk::OfTheLaunchMob)),
        )
        .await
        .expect("the launch mob's desk");
        Box::pin(second.spawn_spec(desk(PrivateDesk::OfTheSecondMob)))
            .await
            .expect("the second mob's desk");
        Box::pin(host.spawn(SpawnMemberSpec::new("worker", "clerk")))
            .await
            .expect("the launch mob's clerk");
        let clerk = host
            .mob_handle()
            .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id("clerk"))
            .await
            .expect("the clerk's session");
        let schedules = ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new()));
        let tools = crate::schedule_wiring::agent_schedule_tools_over_runtime_for_test(
            schedules.clone(),
            host.mob_runtime(),
        );
        Self {
            host,
            second_id,
            schedules,
            tools,
            clerk,
        }
    }

    /// One schedule tool call `name` with `args` in a turn of the clerk:
    /// the text it answers, or its refusal.
    async fn clerk_calls(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<String, meerkat_core::ToolError> {
        let args = serde_json::value::RawValue::from_string(args.to_string())
            .expect("the arguments of a schedule tool call");
        let outcome = self
            .tools
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "schedule-tool-call",
                    name,
                    args: &args,
                },
                &meerkat_core::ToolDispatchContext::default()
                    .with_runtime_identity(self.clerk.clone(), None),
            )
            .await?;
        Ok(outcome.result.text_content())
    }

    async fn stop(self) {
        shut_down(&self.host, &[&self.second_id]).await;
    }
}

/// A target that names the agent `desk` and no mob, as a schedule tool call
/// writes it.
fn desk_by_name_alone_as_written() -> serde_json::Value {
    serde_json::json!({
        "target_kind": "identity",
        "type": "resumable_identity",
        "identity": "desk",
        "action": { "type": "prompt", "prompt": "Check in." },
    })
}

/// A tool answer with `schedule_id` taken out, so that the answers a verb
/// gives two ids compare as answers.
fn answer_without_its_id(
    answer: &Result<String, meerkat_core::ToolError>,
    schedule_id: &str,
) -> String {
    match answer {
        Ok(text) => format!("ok: {text}"),
        Err(error) => format!("error: {error:?}"),
    }
    .replace(schedule_id, "<schedule>")
}

/// A schedule that names an agent and no mob is the launch mob's agent's,
/// at every verb that names it by its id. With the launch mob's `desk`
/// private to a person, another agent of the launch mob is answered as for
/// an id no schedule holds, whatever the second mob's `desk` is, and the
/// schedule stays as it was. With the launch mob's `desk` private to no one
/// and the second mob's `desk` private to a person, the same agent reads
/// it, pauses it, resumes it and deletes it.
///
/// The row holds the reading as this tree has it. It does not tell it from
/// a read that walks the host's mobs in the order of their ids and takes
/// the first member of the name: the launch mob's id sorts first here.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_that_names_no_mob_is_answered_by_id_as_the_launch_mobs_members() {
    for (private, hidden) in [
        (PrivateDesk::OfTheLaunchMob, true),
        (PrivateDesk::OfTheSecondMob, false),
    ] {
        let temp = tempfile::tempdir().expect("tempdir");
        let hosted = TwoDesks::boot(&temp, private).await;
        let by_name_alone = hosted
            .schedules
            .create(CreateScheduleRequest {
                name: Some("the desk by name alone".to_string()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
                },
                target: TargetBinding::identity(meerkat::IdentityTargetBinding::resumable(
                    "desk",
                    meerkat::ScheduledSessionAction::Prompt {
                        prompt: meerkat_core::ContentInput::Text("Check in.".into()),
                        system_prompt: None,
                        render_metadata: None,
                        skill_refs: Vec::new(),
                        additional_instructions: Vec::new(),
                    },
                )),
                misfire_policy: meerkat::MisfirePolicy::default(),
                overlap_policy: meerkat::OverlapPolicy::default(),
                missing_target_policy: meerkat::MissingTargetPolicy::default(),
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("a schedule that names no mob")
            .schedule_id;
        let named = by_name_alone.to_string();
        let no_schedule = meerkat::ScheduleId::new().to_string();
        for verb in [
            "meerkat_schedule_get",
            "meerkat_schedule_occurrences",
            "meerkat_schedule_pause",
            "meerkat_schedule_resume",
            "meerkat_schedule_delete",
        ] {
            let of_the_schedule = hosted
                .clerk_calls(verb, serde_json::json!({ "schedule_id": named }))
                .await;
            let of_no_schedule = hosted
                .clerk_calls(verb, serde_json::json!({ "schedule_id": no_schedule }))
                .await;
            assert_eq!(
                answer_without_its_id(&of_the_schedule, &named)
                    == answer_without_its_id(&of_no_schedule, &no_schedule),
                hidden,
                "{private:?}: {verb} answers the schedule that names no mob {} an id no \
                 schedule holds: {of_the_schedule:?}",
                if hidden { "as" } else { "apart from" }
            );
        }
        let after = hosted
            .schedules
            .get(&by_name_alone)
            .await
            .expect("the schedule is read");
        assert_eq!(
            after.phase,
            if hidden {
                meerkat::SchedulePhase::Active
            } else {
                meerkat::SchedulePhase::Deleted
            },
            "{private:?}: the schedule is as the launch mob's member's audience left it"
        );
        hosted.stop().await;
    }
}

/// Whether a schedule tool answered as it answers every target out of the
/// writer's reach.
fn out_of_reach(answer: &Result<String, meerkat_core::ToolError>) -> bool {
    answer
        .as_ref()
        .is_err_and(|error| format!("{error:?}").contains("schedule_target_not_a_peer"))
}

/// A write of a schedule that names an agent and no mob is judged as a
/// write at the launch mob's agent. With the launch mob's `desk` private to
/// a person, another agent's create with such a target, and its retarget of
/// a schedule of its own onto it, are refused as every target out of a
/// writer's reach is (`schedule_target_not_a_peer`), whatever the second
/// mob's `desk` is. With the launch mob's `desk` private to no one and the
/// second mob's `desk` private to a person, both are taken by the tools'
/// judges: the clerk is wired to the launch mob's `desk`, the member such a
/// target names, and the store of the row has no schedule host over it.
///
/// The row holds the reading as this tree has it; like the row before it,
/// it does not tell it from a read that takes the first member of the name
/// in the order of the mobs' ids.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_of_a_schedule_that_names_no_mob_is_judged_as_the_launch_mobs_members() {
    for (private, refused) in [
        (PrivateDesk::OfTheLaunchMob, true),
        (PrivateDesk::OfTheSecondMob, false),
    ] {
        let temp = tempfile::tempdir().expect("tempdir");
        let hosted = TwoDesks::boot(&temp, private).await;
        if !refused {
            // A writer's reach is its mob's wiring: the clerk writes for the
            // launch mob's desk once the two are wired.
            hosted
                .host
                .mob_handle()
                .wire(
                    crate::member_comms_id::mob_member_id("clerk"),
                    crate::member_comms_id::mob_member_id("desk"),
                )
                .await
                .expect("the clerk and the launch mob's desk are wired");
        }
        let trigger = serde_json::json!({
            "type": "interval",
            "start_at_utc": "2026-07-01T05:00:00Z",
            "every_seconds": 86400,
        });

        let created = hosted
            .clerk_calls(
                "meerkat_schedule_create",
                serde_json::json!({
                    "name": "a word for the desk",
                    "trigger": trigger,
                    "target": desk_by_name_alone_as_written(),
                }),
            )
            .await;
        assert_eq!(
            out_of_reach(&created),
            refused,
            "{private:?}: a create aimed at the agent by name alone: {created:?}"
        );
        assert_eq!(created.is_ok(), !refused, "{private:?}: {created:?}");

        // A schedule of the clerk's own, aimed at itself, and its retarget
        // onto the agent by name alone.
        let own = hosted
            .clerk_calls(
                "meerkat_schedule_create",
                serde_json::json!({
                    "name": "a word for myself",
                    "trigger": trigger,
                    "target": {
                        "target_kind": "mob",
                        "type": "member",
                        "mob_id": hosted.host.mob_handle().mob_id().to_string(),
                        "member_id": "clerk",
                        "action": { "type": "send", "content": "Check in." },
                    },
                }),
            )
            .await
            .expect("the clerk's own schedule");
        let own_id = serde_json::from_str::<serde_json::Value>(&own).expect("a created schedule")
            ["schedule_id"]
            .as_str()
            .expect("the created schedule's id")
            .to_string();
        let retargeted = hosted
            .clerk_calls(
                "meerkat_schedule_update",
                serde_json::json!({
                    "schedule_id": own_id,
                    "target": desk_by_name_alone_as_written(),
                }),
            )
            .await;
        assert_eq!(
            out_of_reach(&retargeted),
            refused,
            "{private:?}: a retarget onto the agent by name alone: {retargeted:?}"
        );
        assert_eq!(retargeted.is_ok(), !refused, "{private:?}: {retargeted:?}");
        hosted.stop().await;
    }
}

/// On a host that fires, a schedule whose identity target names an agent
/// and no mob is refused at its create as an invalid schedule, and neither
/// agent of that name gets a word. No schedule host of the launch delivers
/// such a target: the mobs' host takes a target that names a member with
/// its mob, and the session host has no identity seam. So "the launch
/// mob's agent's" is how the follow, the owner read and an agent's schedule
/// tools read such a schedule, not where a fire of it goes: here, with an
/// identity plane in each mob and an agent `desk` in each, no schedule is
/// stored and neither `desk` holds the word.
///
/// A schedule of that form that is in a store by another way (written by
/// an earlier release, or while no firing host judged creates) ends each
/// fire as a missing target. That is read from the source; no row here
/// stores one beside the host's judgement.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_that_names_an_agent_and_no_mob_is_refused_at_its_create_on_a_host_that_fires() {
    const WORD: &str = "the scheduled word for the agent named with no mob";
    let temp = tempfile::tempdir().expect("tempdir");
    let hosted = ScheduledHost::boot(&temp, Desks::Agents).await;
    let created = hosted
        .schedules
        .create(CreateScheduleRequest {
            name: Some("a word for the desk by name alone".to_string()),
            description: None,
            trigger: TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + chrono::Duration::seconds(1),
            },
            target: TargetBinding::identity(meerkat::IdentityTargetBinding::resumable(
                "desk",
                meerkat::ScheduledSessionAction::Prompt {
                    prompt: meerkat_core::ContentInput::Text(WORD.to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            )),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await
        .map(|schedule| schedule.schedule_id.to_string());
    assert!(
        matches!(
            &created,
            Err(meerkat::ScheduleDomainError::InvalidSchedule(detail))
                if detail.contains("not deliverable by any schedule host")
        ),
        "the create of a schedule that names an agent and no mob is refused as an invalid \
         schedule: {created:?}"
    );
    assert!(
        hosted
            .schedules
            .list()
            .await
            .expect("the schedules are listed")
            .is_empty(),
        "no schedule is stored"
    );
    let (in_the_default, in_the_second) = hosted.desk_sessions().await;
    for (whose, session) in [
        ("the launch mob's", in_the_default),
        ("the second mob's", in_the_second),
    ] {
        // An agent that got no delivery yet may hold no session: it holds
        // no word then.
        if let Some(session) = session {
            assert!(
                !transcript(&hosted.sessions, &session).await.contains(WORD),
                "{whose} desk holds no word of the refused schedule"
            );
        }
    }
    hosted.stop().await;
}

// A schedule aimed at a member of a mob of the host that is stored and was
// not loaded since the process started. The launch's record of whose its
// members are has not read that mob, so whose such a member is is not
// known.

/// A host at its second launch over a state directory. Its first launch
/// created a second mob with a `desk` private to a person and a `side`
/// private to no one; this launch knows that mob and has not loaded it.
/// The launch's own mob holds a `clerk` and an `open`, private to no one
/// and wired to each other. The agents' schedule tools are over a store of
/// the row's, and no schedule host fires over it.
struct StoredDesk {
    host: UnifiedRuntime,
    stored_id: MobId,
    schedules: ScheduleService,
    tools: Arc<dyn meerkat_core::AgentToolDispatcher>,
    clerk: meerkat_core::types::SessionId,
    /// A session no member holds: a host's own.
    hosts_own: meerkat_core::types::SessionId,
    /// The session the stored mob's `desk` was seated in by the first
    /// launch.
    desk_session: meerkat_core::types::SessionId,
    /// The session the stored mob's `side`, private to no one, was seated
    /// in by the first launch.
    side_session: meerkat_core::types::SessionId,
    _stored: StoredHost,
}

impl StoredDesk {
    async fn boot() -> Self {
        use crate::mob_announcements::MemberPerson;

        let stored = StoredHost::new();
        let first = stored.launch().await;
        let stored_id = second_mob(&first, definition("stored")).await;
        let second = set_of(&first)
            .handle_for(&stored_id)
            .await
            .expect("the set serves the second mob");
        // Through the mob's handle: a caller's spawn through the runtime may
        // not supply the label that says whom a member is private to.
        Box::pin(
            second.spawn_spec(
                SpawnMemberSpec::new("worker", "desk")
                    .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven)
                    .with_labels(BTreeMap::from([(
                        crate::access::PRIVATE_TO_LABEL.to_string(),
                        DESKS_OWNER.to_string(),
                    )])),
            ),
        )
        .await
        .expect("the second mob's desk");
        Box::pin(
            second.spawn_spec(
                SpawnMemberSpec::new("worker", "side")
                    .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven),
            ),
        )
        .await
        .expect("the second mob's side");
        let desk_session = second
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("desk"))
            .await
            .expect("the desk's session");
        let side_session = second
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("side"))
            .await
            .expect("the side's session");
        drop(second);
        shut_down(&first, &[]).await;
        drop(first);

        let host = stored.launch().await;
        // The first launch's shutdown left the launch's own mob stopped in
        // its store: lift it, as a gateway's activation lifts its mob.
        if matches!(
            host.mob_handle().status().await,
            Ok(meerkat_mob::MobState::Stopped)
        ) {
            host.mob_handle()
                .resume()
                .await
                .expect("the launch's mob runs again");
        }
        for name in ["clerk", "open"] {
            Box::pin(host.spawn(SpawnMemberSpec::new("worker", name)))
                .await
                .expect("a member of the launch's mob");
        }
        host.mob_handle()
            .wire(
                crate::member_comms_id::mob_member_id("clerk"),
                crate::member_comms_id::mob_member_id("open"),
            )
            .await
            .expect("the clerk and the open member are wired");
        let clerk = host
            .mob_handle()
            .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id("clerk"))
            .await
            .expect("the clerk's session");
        // The state the rows read, after everything the fixture did to the
        // launch's own mob.
        let at_start = known(&host).await;
        assert!(
            at_start.not_loaded.contains(&stored_id) && !at_start.loaded.contains(&stored_id),
            "the second launch knows the stored mob and has not loaded it: it manages {:?} and \
             knows as not loaded {:?}",
            at_start.loaded,
            at_start.not_loaded
        );
        let record = host.mob_runtime().private_owners().clone();
        record.first_attempts_settled(Duration::from_secs(30)).await;
        assert_eq!(
            record.person_for_a_reader(stored_id.as_str(), "desk"),
            MemberPerson::NotKnown,
            "the launch's record has not read the mob that is not loaded: whose its member is \
             is not known"
        );
        let schedules = ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new()));
        let tools = crate::schedule_wiring::agent_schedule_tools_over_runtime_for_test(
            schedules.clone(),
            host.mob_runtime(),
        );
        Self {
            host,
            stored_id,
            schedules,
            tools,
            clerk,
            hosts_own: meerkat_core::types::SessionId::new(),
            desk_session,
            side_session,
            _stored: stored,
        }
    }

    /// One schedule tool call `name` with `args` in a turn of the session
    /// `caller`: the text it answers, or its refusal.
    async fn calls(
        &self,
        caller: &meerkat_core::types::SessionId,
        name: &str,
        args: serde_json::Value,
    ) -> Result<String, meerkat_core::ToolError> {
        let args = serde_json::value::RawValue::from_string(args.to_string())
            .expect("the arguments of a schedule tool call");
        let outcome = self
            .tools
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "schedule-tool-call",
                    name,
                    args: &args,
                },
                &meerkat_core::ToolDispatchContext::default()
                    .with_runtime_identity(caller.clone(), None),
            )
            .await?;
        Ok(outcome.result.text_content())
    }

    /// A schedule in the row's store aimed at member `member` of mob
    /// `mob_id`, written by no tool call.
    async fn schedule_for(&self, mob_id: &MobId, member: &str) -> meerkat::ScheduleId {
        self.schedule_aimed(
            &format!("a word for {member}"),
            member_as_written(mob_id, member),
        )
        .await
    }

    /// A schedule in the row's store aimed at `target`, a target as a
    /// schedule tool call writes it, written by no tool call.
    async fn schedule_aimed(&self, name: &str, target: serde_json::Value) -> meerkat::ScheduleId {
        let target = serde_json::from_value::<TargetBinding>(target)
            .expect("a schedule target as a tool call writes it");
        self.schedules
            .create(CreateScheduleRequest {
                name: Some(name.to_string()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
                },
                target,
                misfire_policy: meerkat::MisfirePolicy::default(),
                overlap_policy: meerkat::OverlapPolicy::default(),
                missing_target_policy: meerkat::MissingTargetPolicy::default(),
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("a schedule in the row's store")
            .schedule_id
    }

    /// Require that the schedule `schedule_id` is on none of `caller`'s
    /// schedule tools: it is not listed, and every verb that reads, changes
    /// or ends it by its id answers as for an id no schedule holds.
    async fn assert_on_no_tool_of(
        &self,
        caller: &meerkat_core::types::SessionId,
        what: &str,
        schedule_id: &str,
    ) {
        let listed = self
            .calls(caller, "meerkat_schedule_list", serde_json::json!({}))
            .await
            .expect("the schedules are listed");
        assert!(
            !listed.contains(schedule_id),
            "{what}: the schedule is not listed: {listed}"
        );
        let no_schedule = meerkat::ScheduleId::new().to_string();
        for verb in [
            "meerkat_schedule_get",
            "meerkat_schedule_occurrences",
            "meerkat_schedule_update",
            "meerkat_schedule_pause",
            "meerkat_schedule_resume",
            "meerkat_schedule_delete",
        ] {
            let asked = |id: &str| {
                if verb == "meerkat_schedule_update" {
                    serde_json::json!({ "schedule_id": id, "name": "renamed by its id" })
                } else {
                    serde_json::json!({ "schedule_id": id })
                }
            };
            let of_the_schedule = self.calls(caller, verb, asked(schedule_id)).await;
            let of_no_schedule = self.calls(caller, verb, asked(&no_schedule)).await;
            assert_eq!(
                answer_without_its_id(&of_the_schedule, schedule_id),
                answer_without_its_id(&of_no_schedule, &no_schedule),
                "{what}: {verb} answers the schedule as an id no schedule holds"
            );
        }
    }

    /// Whether the stored mob is still known and not loaded.
    async fn stored_mob_is_not_loaded(&self) -> bool {
        let now = known(&self.host).await;
        now.not_loaded.contains(&self.stored_id) && !now.loaded.contains(&self.stored_id)
    }

    async fn stop(self) {
        shut_down(&self.host, &[]).await;
    }
}

/// A target that names member `member` of mob `mob_id`, as a schedule tool
/// call writes it.
fn member_as_written(mob_id: &MobId, member: &str) -> serde_json::Value {
    serde_json::json!({
        "target_kind": "mob",
        "type": "member",
        "mob_id": mob_id.to_string(),
        "member_id": member,
        "action": { "type": "send", "content": "Check in." },
    })
}

/// A target that names member `member` of mob `mob_id` by the identity a
/// member's own schedule is written with, as a schedule tool call writes it.
fn identity_as_written(mob_id: &MobId, member: &str) -> serde_json::Value {
    serde_json::json!({
        "target_kind": "identity",
        "type": "resumable_identity",
        "identity": meerkat::surface::mob_member_schedule_identity(&meerkat_core::MobMemberBinding {
            mob_id: mob_id.to_string(),
            role: "worker".to_string(),
            member: member.to_string(),
        }),
        "action": { "type": "prompt", "prompt": "Check in." },
    })
}

/// A target that has the host fork a helper off member `source` of mob
/// `mob_id`, as a schedule tool call writes it.
fn fork_helper_as_written(mob_id: &MobId, source: &str) -> serde_json::Value {
    serde_json::json!({
        "target_kind": "mob",
        "type": "fork_helper",
        "mob_id": mob_id.to_string(),
        "source_member_id": source,
        "member_id": "desk-helper",
        "prompt": "Check in.",
    })
}

/// A target that names the session `session` in the exact session form, as
/// a schedule tool call writes it.
fn exact_session_as_written(session: &meerkat_core::types::SessionId) -> serde_json::Value {
    serde_json::json!({
        "target_kind": "session",
        "type": "exact_session",
        "session_id": session.to_string(),
        "action": { "type": "prompt", "prompt": "Check in." },
    })
}

/// A target that names the session `session` in a resumable session form
/// the schedule tools do not rewrite to a member's own (its prompt carries
/// an option only a session target takes), as a schedule tool call writes
/// it.
fn resumable_session_as_written(session: &meerkat_core::types::SessionId) -> serde_json::Value {
    serde_json::json!({
        "target_kind": "session",
        "type": "resumable_session",
        "session_id": session.to_string(),
        "action": {
            "type": "prompt",
            "prompt": "Check in.",
            "system_prompt": "Answer in one line.",
        },
    })
}

/// A schedule aimed at a member whose person is not known is on no agent's
/// schedule tools, in each form a target names a member (a member target,
/// the identity a member's own schedule is written with, the source of a
/// fork helper). The member is of a mob of the host that is stored and
/// not loaded, and is private to a person there. A caller whose turn no
/// member's session makes and an agent of another mob are not listed the
/// schedule, every verb that names it by its id answers each of them as it
/// answers an id no schedule holds, and the schedule stays as it was. A
/// schedule aimed at a member of a loaded mob that is private to no one is
/// listed to both and read by its id. No call loads the mob.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_for_a_member_whose_person_is_not_known_is_on_no_agents_schedule_tools() {
    let hosted = StoredDesk::boot().await;
    let launch_id = hosted.host.mob_handle().mob_id().clone();
    let open = hosted.schedule_for(&launch_id, "open").await.to_string();
    let no_schedule = meerkat::ScheduleId::new().to_string();
    for (form, target) in [
        (
            "a member target",
            member_as_written(&hosted.stored_id, "desk"),
        ),
        (
            "an identity target",
            identity_as_written(&hosted.stored_id, "desk"),
        ),
        (
            "the source of a fork helper",
            fork_helper_as_written(&hosted.stored_id, "desk"),
        ),
    ] {
        let unread_id = hosted.schedule_aimed("a word for the desk", target).await;
        let unread = unread_id.to_string();
        for (who, caller) in [
            (
                "a caller whose turn no member's session makes",
                &hosted.hosts_own,
            ),
            ("an agent of another mob", &hosted.clerk),
        ] {
            let listed = hosted
                .calls(caller, "meerkat_schedule_list", serde_json::json!({}))
                .await
                .expect("the schedules are listed");
            assert!(
                !listed.contains(&unread),
                "{form}: {who} is not listed the schedule aimed at a member whose person is \
                 not known: {listed}"
            );
            for verb in [
                "meerkat_schedule_get",
                "meerkat_schedule_occurrences",
                "meerkat_schedule_pause",
                "meerkat_schedule_resume",
                "meerkat_schedule_delete",
            ] {
                let of_the_schedule = hosted
                    .calls(caller, verb, serde_json::json!({ "schedule_id": unread }))
                    .await;
                let of_no_schedule = hosted
                    .calls(
                        caller,
                        verb,
                        serde_json::json!({ "schedule_id": no_schedule }),
                    )
                    .await;
                assert_eq!(
                    answer_without_its_id(&of_the_schedule, &unread),
                    answer_without_its_id(&of_no_schedule, &no_schedule),
                    "{form}: {verb} answers {who} the schedule aimed at a member whose person \
                     is not known as an id no schedule holds"
                );
            }
        }
        let after = hosted
            .schedules
            .get(&unread_id)
            .await
            .expect("the schedule is read");
        assert_eq!(
            after.phase,
            meerkat::SchedulePhase::Active,
            "{form}: the schedule aimed at a member whose person is not known stays as it was"
        );
    }
    for (who, caller) in [
        (
            "a caller whose turn no member's session makes",
            &hosted.hosts_own,
        ),
        ("an agent of another mob", &hosted.clerk),
    ] {
        let listed = hosted
            .calls(caller, "meerkat_schedule_list", serde_json::json!({}))
            .await
            .expect("the schedules are listed");
        assert!(
            listed.contains(&open),
            "{who} is listed the schedule aimed at a member of a loaded mob that is private to \
             no one: {listed}"
        );
        let read = hosted
            .calls(
                caller,
                "meerkat_schedule_get",
                serde_json::json!({ "schedule_id": open }),
            )
            .await;
        assert!(
            read.as_ref().is_ok_and(|text| text.contains(&open)),
            "{who} reads the schedule aimed at a member of a loaded mob that is private to no \
             one by its id: {read:?}"
        );
    }
    assert!(
        hosted.stored_mob_is_not_loaded().await,
        "no call of an agent's schedule tools loads the mob"
    );
    hosted.stop().await;
}

/// A schedule aimed at the session of a member whose person is not known
/// is on no agent's schedule tools either, in the exact session form and
/// in a resumable form the tools do not rewrite. The sessions are the ones
/// the two members of the stored mob were seated in: no loaded mob's
/// roster holds them, and their stored records name the members.
///
/// While the mob is not loaded, a caller whose turn no member's session
/// makes is listed neither member's schedule, and every verb that reads,
/// changes or ends one by its id answers as for an id no schedule holds;
/// an agent of another mob is listed none either. A schedule aimed at a
/// session no stored record names a member for stays on that caller's
/// tools. No call loads the mob.
///
/// Once the mob is loaded and the launch's record has read it, the
/// schedule aimed at the session of the member private to no one is on
/// that caller's tools, and the one aimed at the session of the member
/// private to a person stays off them. No schedule was changed.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_for_the_session_of_a_member_whose_person_is_not_known_is_on_no_agents_tools() {
    use crate::mob_announcements::MemberPerson;

    const HOSTS_OWN: &str = "a caller whose turn no member's session makes";
    let hosted = StoredDesk::boot().await;
    let mut of_the_desk = Vec::new();
    let mut of_the_side = Vec::new();
    for (form, written) in [
        (
            "the exact session form",
            exact_session_as_written as fn(&meerkat_core::types::SessionId) -> serde_json::Value,
        ),
        (
            "a resumable session form that is not rewritten",
            resumable_session_as_written,
        ),
    ] {
        of_the_desk.push((
            form,
            hosted
                .schedule_aimed(
                    "a word for the desk's session",
                    written(&hosted.desk_session),
                )
                .await,
        ));
        of_the_side.push((
            form,
            hosted
                .schedule_aimed(
                    "a word for the side's session",
                    written(&hosted.side_session),
                )
                .await,
        ));
    }
    let of_no_member = hosted
        .schedule_aimed(
            "a word for a session no member holds",
            exact_session_as_written(&meerkat_core::types::SessionId::new()),
        )
        .await
        .to_string();

    // While the mob is not loaded.
    for (form, schedule_id) in of_the_desk.iter().chain(&of_the_side) {
        hosted
            .assert_on_no_tool_of(
                &hosted.hosts_own,
                &format!("{form}, the mob not loaded, {HOSTS_OWN}"),
                &schedule_id.to_string(),
            )
            .await;
    }
    let listed = hosted
        .calls(
            &hosted.hosts_own,
            "meerkat_schedule_list",
            serde_json::json!({}),
        )
        .await
        .expect("the schedules are listed");
    assert!(
        listed.contains(&of_no_member),
        "{HOSTS_OWN} is listed the schedule aimed at a session no member holds: {listed}"
    );
    let listed_to_the_clerk = hosted
        .calls(
            &hosted.clerk,
            "meerkat_schedule_list",
            serde_json::json!({}),
        )
        .await
        .expect("the schedules are listed");
    for (form, schedule_id) in of_the_desk.iter().chain(&of_the_side) {
        assert!(
            !listed_to_the_clerk.contains(&schedule_id.to_string()),
            "{form}: an agent of another mob is not listed the schedule aimed at the session \
             of a member whose person is not known: {listed_to_the_clerk}"
        );
    }
    assert!(
        hosted.stored_mob_is_not_loaded().await,
        "no call of an agent's schedule tools loads the mob"
    );

    // The mob is loaded by a lookup of it, and the launch's record reads it.
    let loaded = set_of(&hosted.host)
        .handle_for(&hosted.stored_id)
        .await
        .expect("the lookup loads the stored mob");
    drop(loaded);
    let record = hosted.host.mob_runtime().private_owners().clone();
    tokio::time::timeout(
        Duration::from_secs(30),
        record.read_of_the_mob_awaited(hosted.stored_id.as_str(), Duration::from_hours(1)),
    )
    .await
    .expect("the launch's record reads the mob once it is loaded");
    assert_eq!(
        record.person_for_a_reader(hosted.stored_id.as_str(), "side"),
        MemberPerson::Shared,
        "the record has read the mob: its side is private to no one"
    );
    for (form, schedule_id) in &of_the_desk {
        hosted
            .assert_on_no_tool_of(
                &hosted.hosts_own,
                &format!("{form}, the mob loaded, {HOSTS_OWN}"),
                &schedule_id.to_string(),
            )
            .await;
    }
    let listed = hosted
        .calls(
            &hosted.hosts_own,
            "meerkat_schedule_list",
            serde_json::json!({}),
        )
        .await
        .expect("the schedules are listed");
    for (form, schedule_id) in &of_the_side {
        let schedule_id = schedule_id.to_string();
        assert!(
            listed.contains(&schedule_id),
            "{form}: once the mob is loaded, {HOSTS_OWN} is listed the schedule aimed at the \
             session of its member private to no one: {listed}"
        );
        let read = hosted
            .calls(
                &hosted.hosts_own,
                "meerkat_schedule_get",
                serde_json::json!({ "schedule_id": schedule_id }),
            )
            .await;
        assert!(
            read.as_ref().is_ok_and(|text| text.contains(&schedule_id)),
            "{form}: once the mob is loaded, {HOSTS_OWN} reads that schedule by its id: \
             {read:?}"
        );
    }
    for (form, schedule_id) in of_the_desk.iter().chain(&of_the_side) {
        let after = hosted
            .schedules
            .get(schedule_id)
            .await
            .expect("the schedule is read");
        assert_eq!(
            after.phase,
            meerkat::SchedulePhase::Active,
            "{form}: no call changed or ended the schedule"
        );
    }
    hosted.stop().await;
}

/// A schedule write aimed at a member whose person is not known is refused.
/// The member is of a mob of the host that is stored and not loaded. A
/// create with such a target, and a retarget of a schedule of the caller's
/// own onto it, are refused as every target out of a writer's reach is
/// (`schedule_target_not_a_peer`), for a caller whose turn no member's
/// session makes and for an agent of another mob; the retargeted schedule
/// keeps its target. A create aimed at a member of a loaded mob that is
/// private to no one is taken from both. No call loads the mob.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_write_for_a_member_whose_person_is_not_known_is_refused() {
    let hosted = StoredDesk::boot().await;
    let launch_id = hosted.host.mob_handle().mob_id().clone();
    let trigger = serde_json::json!({
        "type": "interval",
        "start_at_utc": "2026-07-01T05:00:00Z",
        "every_seconds": 86400,
    });
    for (who, caller) in [
        (
            "a caller whose turn no member's session makes",
            &hosted.hosts_own,
        ),
        ("an agent of another mob", &hosted.clerk),
    ] {
        let created = hosted
            .calls(
                caller,
                "meerkat_schedule_create",
                serde_json::json!({
                    "name": "a word for the desk",
                    "trigger": trigger,
                    "target": member_as_written(&hosted.stored_id, "desk"),
                }),
            )
            .await;
        assert!(
            out_of_reach(&created),
            "a create by {who} aimed at a member whose person is not known is refused as a \
             target out of reach: {created:?}"
        );

        let by_its_identity = hosted
            .calls(
                caller,
                "meerkat_schedule_create",
                serde_json::json!({
                    "name": "a word for the desk by its identity",
                    "trigger": trigger,
                    "target": identity_as_written(&hosted.stored_id, "desk"),
                }),
            )
            .await;
        assert!(
            out_of_reach(&by_its_identity),
            "a create by {who} aimed, by its identity, at a member whose person is not known is \
             refused as a target out of reach: {by_its_identity:?}"
        );

        // A schedule of the caller's own, aimed at a member of a loaded mob
        // that is private to no one, and its retarget onto the member whose
        // person is not known.
        let own = hosted
            .calls(
                caller,
                "meerkat_schedule_create",
                serde_json::json!({
                    "name": "a word for the open member",
                    "trigger": trigger,
                    "target": member_as_written(&launch_id, "open"),
                }),
            )
            .await
            .expect("a create aimed at a member of a loaded mob that is private to no one");
        let own_id = serde_json::from_str::<serde_json::Value>(&own).expect("a created schedule")
            ["schedule_id"]
            .as_str()
            .expect("the created schedule's id")
            .to_string();
        let retargeted = hosted
            .calls(
                caller,
                "meerkat_schedule_update",
                serde_json::json!({
                    "schedule_id": own_id,
                    "target": member_as_written(&hosted.stored_id, "desk"),
                }),
            )
            .await;
        assert!(
            out_of_reach(&retargeted),
            "a retarget by {who} onto a member whose person is not known is refused as a \
             target out of reach: {retargeted:?}"
        );
        let after = hosted
            .schedules
            .get(&meerkat::ScheduleId::parse(&own_id).expect("the created schedule's id"))
            .await
            .expect("the schedule is read");
        assert!(
            matches!(
                &after.target,
                TargetBinding::Mob(binding) if matches!(
                    binding.as_ref(),
                    MobTargetBinding::Member { mob_id, .. } if mob_id.as_str() == launch_id.as_str()
                )
            ),
            "the schedule {who} tried to retarget keeps its target: {:?}",
            after.target
        );
    }
    assert!(
        hosted.stored_mob_is_not_loaded().await,
        "no call of an agent's schedule tools loads the mob"
    );
    hosted.stop().await;
}

// Three pins of what holds today: a schedule of a destroyed mob and a mob
// created again under its id; a load of a stored mob; an agent's schedule
// write for a member of another loaded mob.

/// A definition with one profile under the id `id`: what a mob is created
/// again from under an earlier mob's id.
fn definition_under(id: &MobId) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n[profiles.worker.tools]\ncomms = true\n"
    ))
    .expect("a definition with one profile")
}

/// A mob is destroyed and a mob is created again under its id, with a
/// member of the same name: no schedule of the earlier mob is live, and no
/// fire reaches the new member. The schedule was aimed at the earlier
/// mob's `desk` and falls due after the new mob's `desk` is seated, on a
/// host that fires. The create ends it before the new mob exists; when its
/// due time has passed, no occurrence of it completed and neither the new
/// `desk` nor the launch mob's `desk` holds its word.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_of_a_destroyed_mob_reaches_no_member_of_a_mob_created_under_its_id() {
    const WORD: &str = "the scheduled word for the desk of the destroyed mob";
    let temp = tempfile::tempdir().expect("tempdir");
    let hosted = ScheduledHost::boot(&temp, Desks::Members).await;
    let desk = crate::member_comms_id::mob_member_id("desk");
    let due = chrono::Utc::now() + chrono::Duration::seconds(10);
    let schedule = hosted
        .schedules
        .create(CreateScheduleRequest {
            name: Some("a word for the desk of the earlier mob".to_string()),
            description: None,
            trigger: TriggerSpec::Once { due_at_utc: due },
            target: TargetBinding::Mob(Box::new(MobTargetBinding::Member {
                mob_id: hosted.second_id.to_string(),
                member_id: desk.to_string(),
                action: ScheduledMobAction::Send {
                    content: meerkat_core::ContentInput::Text(WORD.to_string()),
                    render_metadata: None,
                },
            })),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await
        .expect("the schedule is created")
        .schedule_id;

    let destroyed = Box::pin(set_of(&hosted.host).mob_destroy(&hosted.second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        second_mob(&hosted.host, definition_under(&hosted.second_id)).await,
        hosted.second_id,
        "a mob is created again under the destroyed mob's id"
    );
    let ended = hosted
        .schedules
        .get(&schedule)
        .await
        .expect("the schedule is read");
    assert_eq!(
        ended.phase,
        meerkat::SchedulePhase::Deleted,
        "the create ended the schedule the destroyed mob left under the id"
    );
    let created_again = set_of(&hosted.host)
        .handle_for(&hosted.second_id)
        .await
        .expect("the set serves the mob created again");
    created_again
        .spawn_spec(SpawnMemberSpec::new("worker", "desk"))
        .await
        .expect("the new mob's desk");
    let new_desk = created_again
        .resolve_bridge_session_id(&desk)
        .await
        .expect("the new desk's session");
    let default_desk = hosted
        .host
        .mob_handle()
        .resolve_bridge_session_id(&desk)
        .await
        .expect("the launch mob's desk's session");
    assert!(
        chrono::Utc::now() < due,
        "the new mob's desk is seated before the schedule falls due"
    );

    // Past the due time, by more than a fire takes to its delivery.
    let past_due = due + chrono::Duration::seconds(4);
    while chrono::Utc::now() < past_due {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let completed = hosted
        .schedules
        .list_occurrences(&schedule)
        .await
        .is_ok_and(|occurrences| {
            occurrences
                .iter()
                .any(|occurrence| matches!(occurrence.phase, OccurrencePhase::Completed))
        });
    assert!(!completed, "no occurrence of the ended schedule completed");
    assert!(
        !transcript(&hosted.sessions, &new_desk).await.contains(WORD),
        "the new mob's desk holds no word of the earlier mob's schedule"
    );
    assert!(
        !transcript(&hosted.sessions, &default_desk)
            .await
            .contains(WORD),
        "the launch mob's desk holds no word of it either"
    );
    drop(created_again);
    hosted.stop().await;
}

/// A load of a stored mob ends no schedule. Schedules aimed at a member of
/// a stored mob that is not loaded, in each form a target names a member
/// or its session, stand as they were once a lookup has loaded the mob
/// and the launch's record has read it: a release of a mob's schedules is
/// a create's, never a load's.
#[tokio::test(flavor = "multi_thread")]
async fn a_load_of_a_stored_mob_ends_no_schedule() {
    let hosted = StoredDesk::boot().await;
    let mut aimed = Vec::new();
    for (form, target) in [
        (
            "a member target",
            member_as_written(&hosted.stored_id, "desk"),
        ),
        (
            "an identity target",
            identity_as_written(&hosted.stored_id, "desk"),
        ),
        (
            "the source of a fork helper",
            fork_helper_as_written(&hosted.stored_id, "desk"),
        ),
        (
            "the exact session form",
            exact_session_as_written(&hosted.desk_session),
        ),
        (
            "a resumable session form",
            resumable_session_as_written(&hosted.desk_session),
        ),
    ] {
        aimed.push((
            form,
            hosted.schedule_aimed("a word for the desk", target).await,
        ));
    }
    assert!(
        hosted.stored_mob_is_not_loaded().await,
        "the stored mob is not loaded before the lookup"
    );

    let loaded = set_of(&hosted.host)
        .handle_for(&hosted.stored_id)
        .await
        .expect("the lookup loads the stored mob");
    drop(loaded);
    let record = hosted.host.mob_runtime().private_owners().clone();
    tokio::time::timeout(
        Duration::from_secs(30),
        record.read_of_the_mob_awaited(hosted.stored_id.as_str(), Duration::from_hours(1)),
    )
    .await
    .expect("the launch's record reads the mob once it is loaded");
    assert!(
        !hosted.stored_mob_is_not_loaded().await,
        "the lookup loaded the stored mob"
    );

    for (form, schedule_id) in &aimed {
        let after = hosted
            .schedules
            .get(schedule_id)
            .await
            .expect("the schedule is read");
        assert_eq!(
            after.phase,
            meerkat::SchedulePhase::Active,
            "{form}: the load of the stored mob ended no schedule aimed at its member"
        );
    }
    hosted.stop().await;
}

/// An agent's schedule write does not reach a member of another loaded mob
/// of the host. Both mobs are loaded and the second mob's `desk` is private
/// to no one; nothing wires an agent of the launch mob to it. The launch
/// mob's agent's create aimed at that member, and its retarget of a
/// schedule of its own onto it, are refused as every target out of a
/// writer's reach is (`schedule_target_not_a_peer`); the retargeted
/// schedule keeps its target and no schedule names the second mob.
#[tokio::test(flavor = "multi_thread")]
async fn an_agents_schedule_write_for_a_member_of_another_loaded_mob_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let hosted = TwoDesks::boot(&temp, PrivateDesk::OfTheLaunchMob).await;
    let launch_id = hosted.host.mob_handle().mob_id().clone();
    let trigger = serde_json::json!({
        "type": "interval",
        "start_at_utc": "2026-07-01T05:00:00Z",
        "every_seconds": 86400,
    });

    let created = hosted
        .clerk_calls(
            "meerkat_schedule_create",
            serde_json::json!({
                "name": "a word for the second mob's desk",
                "trigger": trigger,
                "target": member_as_written(&hosted.second_id, "desk"),
            }),
        )
        .await;
    assert!(
        out_of_reach(&created),
        "a create aimed at a member of another loaded mob is refused as a target out of \
         reach: {created:?}"
    );

    let own = hosted
        .clerk_calls(
            "meerkat_schedule_create",
            serde_json::json!({
                "name": "a word for myself",
                "trigger": trigger,
                "target": member_as_written(&launch_id, "clerk"),
            }),
        )
        .await
        .expect("the clerk's own schedule");
    let own_id =
        serde_json::from_str::<serde_json::Value>(&own).expect("a created schedule")["schedule_id"]
            .as_str()
            .expect("the created schedule's id")
            .to_string();
    let retargeted = hosted
        .clerk_calls(
            "meerkat_schedule_update",
            serde_json::json!({
                "schedule_id": own_id,
                "target": member_as_written(&hosted.second_id, "desk"),
            }),
        )
        .await;
    assert!(
        out_of_reach(&retargeted),
        "a retarget onto a member of another loaded mob is refused as a target out of reach: \
         {retargeted:?}"
    );
    let stored = hosted
        .schedules
        .list()
        .await
        .expect("the schedules are read");
    assert_eq!(
        stored.len(),
        1,
        "only the clerk's own schedule is stored: {stored:?}"
    );
    assert!(
        matches!(
            &stored[0].target,
            TargetBinding::Mob(binding) if matches!(
                binding.as_ref(),
                MobTargetBinding::Member { mob_id, .. } if mob_id.as_str() == launch_id.as_str()
            )
        ),
        "the schedule the clerk tried to retarget keeps its target: {:?}",
        stored[0].target
    );
    hosted.stop().await;
}

/// A schedule aimed at the session of a member private to a person stays
/// off the tools of a caller whose turn no member's session makes while
/// the member's mob is not loaded, also when the launch's record has read
/// that mob. The stored mob is loaded by a lookup, the record reads it,
/// and the mob is unloaded again as an idle mob is: no loaded mob's roster
/// holds its members' sessions, and the record knows whose its members
/// are. The schedule aimed at the session of the member private to a
/// person is not listed to that caller, and every verb that reads, changes
/// or ends it by its id answers as for an id no schedule holds; the one
/// aimed at the session of the member private to no one is listed and
/// read by its id. No schedule was changed.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_for_a_private_members_session_stays_hidden_once_its_mob_is_unloaded() {
    use crate::mob_announcements::MemberPerson;

    const HOSTS_OWN: &str = "a caller whose turn no member's session makes";
    let hosted = StoredDesk::boot().await;
    let mut of_the_desk = Vec::new();
    let mut of_the_side = Vec::new();
    for (form, written) in [
        (
            "the exact session form",
            exact_session_as_written as fn(&meerkat_core::types::SessionId) -> serde_json::Value,
        ),
        (
            "a resumable session form that is not rewritten",
            resumable_session_as_written,
        ),
    ] {
        of_the_desk.push((
            form,
            hosted
                .schedule_aimed(
                    "a word for the desk's session",
                    written(&hosted.desk_session),
                )
                .await,
        ));
        of_the_side.push((
            form,
            hosted
                .schedule_aimed(
                    "a word for the side's session",
                    written(&hosted.side_session),
                )
                .await,
        ));
    }

    // Loaded by a lookup, read by the launch's record, and unloaded again.
    let loaded = set_of(&hosted.host)
        .handle_for(&hosted.stored_id)
        .await
        .expect("the lookup loads the stored mob");
    drop(loaded);
    let record = hosted.host.mob_runtime().private_owners().clone();
    tokio::time::timeout(
        Duration::from_secs(30),
        record.read_of_the_mob_awaited(hosted.stored_id.as_str(), Duration::from_hours(1)),
    )
    .await
    .expect("the launch's record reads the mob once it is loaded");
    asleep(&hosted.host, &[&hosted.stored_id]).await;
    assert_eq!(
        record.person_for_a_reader(hosted.stored_id.as_str(), "desk"),
        MemberPerson::Private(DESKS_OWNER.to_string()),
        "the record has read the mob: it knows whose the desk is"
    );
    assert_eq!(
        record.person_for_a_reader(hosted.stored_id.as_str(), "side"),
        MemberPerson::Shared,
        "the record has read the mob: its side is private to no one"
    );

    for (form, schedule_id) in &of_the_desk {
        hosted
            .assert_on_no_tool_of(
                &hosted.hosts_own,
                &format!("{form}, the mob unloaded, {HOSTS_OWN}"),
                &schedule_id.to_string(),
            )
            .await;
    }
    let listed = hosted
        .calls(
            &hosted.hosts_own,
            "meerkat_schedule_list",
            serde_json::json!({}),
        )
        .await
        .expect("the schedules are listed");
    for (form, schedule_id) in &of_the_side {
        let schedule_id = schedule_id.to_string();
        assert!(
            listed.contains(&schedule_id),
            "{form}: {HOSTS_OWN} is listed the schedule aimed at the session of the member \
             private to no one: {listed}"
        );
        let read = hosted
            .calls(
                &hosted.hosts_own,
                "meerkat_schedule_get",
                serde_json::json!({ "schedule_id": schedule_id }),
            )
            .await;
        assert!(
            read.as_ref().is_ok_and(|text| text.contains(&schedule_id)),
            "{form}: {HOSTS_OWN} reads that schedule by its id: {read:?}"
        );
    }
    for (form, schedule_id) in of_the_desk.iter().chain(&of_the_side) {
        let after = hosted
            .schedules
            .get(schedule_id)
            .await
            .expect("the schedule is read");
        assert_eq!(
            after.phase,
            meerkat::SchedulePhase::Active,
            "{form}: no call changed or ended the schedule"
        );
    }
    hosted.stop().await;
}

/// A schedule aimed at a member the launch's record holds only as retired
/// is on no agent's schedule tools, and a write aimed at it is refused.
/// The record holds the name as one whose retirement the memory store
/// recorded as private: it is private to a person, though no record names
/// whom, and no mob's roster holds it. A caller whose turn no member's
/// session makes and an agent of the name's mob are not listed the
/// schedule, every verb that reads, changes or ends it by its id answers
/// as for an id no schedule holds, and a create aimed at the name by a
/// caller whose turn no member's session makes is refused as every target
/// out of a writer's reach is.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_for_a_member_held_only_as_retired_is_on_no_agents_schedule_tools() {
    const RETIRED: &str = "retired-private";
    let hosted = StoredDesk::boot().await;
    let launch_id = hosted.host.mob_handle().mob_id().clone();
    hosted
        .host
        .mob_runtime()
        .private_owners()
        .hold_as_retired_in_tests(RETIRED);
    assert_eq!(
        hosted
            .host
            .mob_runtime()
            .private_owners()
            .person_of(launch_id.as_str(), RETIRED),
        crate::mob_announcements::MemberPerson::Private(
            crate::access::UNKNOWN_PRIVATE_OWNER.to_string()
        ),
        "the record holds the name only as retired: private to a person nobody is"
    );
    let aimed_id = hosted.schedule_for(&launch_id, RETIRED).await;
    let aimed = aimed_id.to_string();

    for (who, caller) in [
        (
            "a caller whose turn no member's session makes",
            &hosted.hosts_own,
        ),
        ("an agent of the name's mob", &hosted.clerk),
    ] {
        hosted.assert_on_no_tool_of(caller, who, &aimed).await;
    }
    let created = hosted
        .calls(
            &hosted.hosts_own,
            "meerkat_schedule_create",
            serde_json::json!({
                "name": "a word for the retired name",
                "trigger": {
                    "type": "interval",
                    "start_at_utc": "2026-07-01T05:00:00Z",
                    "every_seconds": 86400,
                },
                "target": member_as_written(&launch_id, RETIRED),
            }),
        )
        .await;
    assert!(
        out_of_reach(&created),
        "a create aimed at a member held only as retired is refused as a target out of reach: \
         {created:?}"
    );
    let after = hosted
        .schedules
        .get(&aimed_id)
        .await
        .expect("the schedule is read");
    assert_eq!(
        after.phase,
        meerkat::SchedulePhase::Active,
        "no call changed or ended the schedule"
    );
    hosted.stop().await;
}

/// A host over a state directory with a gateway's schedule host over it,
/// and a second mob of the host that is known and not loaded, whose `desk`
/// was seated in a session before: no loaded mob's roster holds the desk's
/// session.
struct SleepingDesk {
    host: UnifiedRuntime,
    sessions: Sessions,
    schedules: ScheduleService,
    schedule_host: Option<meerkat::surface::ScheduleHostHandle>,
    watchdog: tokio::task::JoinHandle<()>,
    second: MobId,
    desk_session: meerkat_core::types::SessionId,
    _stored: StoredHost,
}

/// Why the second mob of a [`SleepingDesk`] is not loaded.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NotLoaded {
    /// The mob set unloaded the idle mob in this launch: the mob runs in
    /// its store.
    Idle,
    /// An earlier launch stopped the mob and ended; this launch has not
    /// loaded it: the mob is stopped in its store.
    Stopped,
}

impl SleepingDesk {
    /// Seat the `desk` of the mob `second` of `host`, private to a person
    /// when `private` says so, and answer its session.
    async fn seat_the_desk(
        host: &UnifiedRuntime,
        second: &MobId,
        private: bool,
    ) -> meerkat_core::types::SessionId {
        let seated_in = set_of(host)
            .handle_for(second)
            .await
            .expect("the second mob");
        // Seated turn-driven: a member that runs a turn of its own at its
        // spawn keeps its mob busy for the idle unload while that turn is
        // under way. Through the mob's handle: a caller's spawn through the
        // runtime may not supply the label that says whom a member is
        // private to.
        let mut desk = SpawnMemberSpec::new("worker", "desk")
            .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven);
        if private {
            desk = desk.with_labels(BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                DESKS_OWNER.to_string(),
            )]));
        }
        Box::pin(seated_in.spawn_spec(desk))
            .await
            .expect("the second mob's desk");
        seated_in
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("desk"))
            .await
            .expect("the desk's session")
    }

    /// The host with the desk's mob not loaded for the reason `why`; the
    /// desk is private to a person when `private` says so.
    async fn boot(private: bool, why: NotLoaded) -> Self {
        let stored = StoredHost::new();
        let schedule_state = stored.state_dir().join("schedule-state");
        std::fs::create_dir_all(&schedule_state).expect("the schedule state directory");
        // A stopped mob is seated and stopped by an earlier launch over the
        // directory, which then ends.
        let earlier = if why == NotLoaded::Stopped {
            let first = stored.launch().await;
            let second = second_mob(&first, definition("session-fire-while-asleep")).await;
            let desk_session = Self::seat_the_desk(&first, &second, private).await;
            set_of(&first)
                .mob_stop(&second)
                .await
                .expect("the second mob stops");
            shut_down(&first, &[]).await;
            drop(first);
            Some((second, desk_session))
        } else {
            None
        };
        let (host, sessions, machine, attached) = stored
            .launch_over(|builder| {
                attach_gateway_schedule_tools(
                    builder,
                    &schedule_state,
                    ScheduleFiringRole::Executor,
                )
                .expect("the schedule tools attach")
            })
            .await;
        let (second, desk_session) = match earlier {
            Some(seated) => {
                // The earlier launch's shutdown left the launch's own mob
                // stopped in its store: lift it, as a gateway's activation
                // lifts its mob.
                if matches!(
                    host.mob_handle().status().await,
                    Ok(meerkat_mob::MobState::Stopped)
                ) {
                    host.mob_handle()
                        .resume()
                        .await
                        .expect("the launch's mob runs again");
                }
                seated
            }
            None => {
                let second = second_mob(&host, definition("session-fire-while-asleep")).await;
                let desk_session = Self::seat_the_desk(&host, &second, private).await;
                (second, desk_session)
            }
        };
        let mob_state =
            adopt_schedule_mob_targets(&host, &attached.service, &attached.mob_target_registry)
                .await;
        let (schedule_host, watchdog) = spawn_gateway_schedule_host(
            &host,
            mob_state,
            GatewayScheduleHostInputs {
                schedule_service: attached.service.clone(),
                session_service: Arc::clone(&sessions),
                runtime_adapter: machine,
                schedule_store_path: schedule_state.join(SCHEDULE_STORE_FILE),
                firing_host_binding: attached.firing_host_binding.clone(),
                runnable_host: None,
                workgraph_service: None,
                owner_id: format!("session-fire-{}", uuid::Uuid::new_v4().simple()),
            },
        )
        .await;
        assert!(
            schedule_host.is_some(),
            "the schedule host spawns over the launch"
        );
        match why {
            NotLoaded::Idle => asleep(&host, &[&second]).await,
            NotLoaded::Stopped => {
                let at_start = known(&host).await;
                assert!(
                    at_start.not_loaded.contains(&second) && !at_start.loaded.contains(&second),
                    "the launch knows the stopped mob and has not loaded it: it manages {:?} \
                     and knows as not loaded {:?}",
                    at_start.loaded,
                    at_start.not_loaded
                );
            }
        }
        Self {
            host,
            sessions,
            schedules: attached.service,
            schedule_host,
            watchdog,
            second,
            desk_session,
            _stored: stored,
        }
    }

    /// A schedule with the trigger `trigger` aimed at the desk's session
    /// (the exact session form) with the prompt `word`, and its first
    /// occurrence's phase once it is terminal or `wait` passed.
    async fn fire_into_the_desks_session(
        &self,
        trigger: serde_json::Value,
        word: &str,
        wait: Duration,
    ) -> Option<String> {
        let target = serde_json::from_value::<TargetBinding>(serde_json::json!({
            "target_kind": "session",
            "type": "exact_session",
            "session_id": self.desk_session.to_string(),
            "action": { "type": "prompt", "prompt": word },
        }))
        .expect("a session target as a tool call writes it");
        let trigger = serde_json::from_value::<TriggerSpec>(trigger)
            .expect("a trigger as a tool call writes it");
        let schedule = self
            .schedules
            .create(CreateScheduleRequest {
                name: Some(
                    "a word for the session of a member of a mob that is not loaded".to_string(),
                ),
                description: None,
                trigger,
                target,
                misfire_policy: meerkat::MisfirePolicy::default(),
                overlap_policy: meerkat::OverlapPolicy::default(),
                missing_target_policy: meerkat::MissingTargetPolicy::default(),
                labels: BTreeMap::new(),
                planning_horizon_days: None,
                planning_horizon_occurrences: None,
            })
            .await
            .expect("the schedule is created");
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let occurrence = self
                .schedules
                .list_occurrences(&schedule.schedule_id)
                .await
                .expect("the occurrences are listed")
                .into_iter()
                .next();
            let terminal = occurrence.as_ref().is_some_and(|occurrence| {
                matches!(
                    occurrence.phase,
                    OccurrencePhase::Completed
                        | OccurrencePhase::Skipped
                        | OccurrencePhase::Misfired
                        | OccurrencePhase::Superseded
                        | OccurrencePhase::DeliveryFailed
                )
            });
            if terminal || tokio::time::Instant::now() >= deadline {
                return occurrence.map(|occurrence| format!("{:?}", occurrence.phase));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn stop(self) {
        if let Some(schedule_host) = self.schedule_host {
            schedule_host.shutdown().await;
        }
        self.watchdog.abort();
        let _ = self.watchdog.await;
        shut_down(&self.host, &[&self.second]).await;
    }
}

/// One fire into the session of the desk of a mob the mob set unloaded:
/// the occurrence completes at its delivery, the mob is loaded after it,
/// and the desk's session holds the word.
async fn a_fire_into_a_sleeping_desks_session_loads_its_mob(
    case: &str,
    private: bool,
    interval: bool,
) {
    let word = format!("the scheduled word for the session of a sleeping desk: {case}");
    let hosted = SleepingDesk::boot(private, NotLoaded::Idle).await;
    let due = chrono::Utc::now() + chrono::Duration::seconds(1);
    let trigger = if interval {
        serde_json::json!({
            "type": "interval",
            "start_at_utc": due.to_rfc3339(),
            "every_seconds": 3600,
        })
    } else {
        serde_json::json!({ "type": "once", "due_at_utc": due.to_rfc3339() })
    };
    let phase = hosted
        .fire_into_the_desks_session(trigger, &word, FIRE_TERMINAL_WAIT)
        .await;
    let got_the_word = phase == completed()
        && transcript_showing(&hosted.sessions, &hosted.desk_session, &word)
            .await
            .contains(&word);
    let loaded = known(&hosted.host).await.loaded.contains(&hosted.second);
    assert!(
        !got_the_word || loaded,
        "{case}: the member's session took a turn while its mob was not loaded: the fire ended \
         {phase:?}"
    );
    assert_eq!(
        (phase, loaded, got_the_word),
        (completed(), true, true),
        "{case}: the fire into the session of a member of a mob that is not loaded completes \
         at its delivery, loads the mob and gives the member the word"
    );
    hosted.stop().await;
}

/// A schedule fire into a member's session gives that session no turn
/// beside its mob: where the member's mob is not loaded, the fire loads
/// the mob and the turn is its member's. The schedule is aimed at the
/// session of a member of a mob of the host that is not loaded (the exact
/// session form), on a host that fires. Its occurrence completes at its
/// delivery, the mob is loaded after it, and the member's session holds
/// the word.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_fire_into_a_member_session_gives_no_turn_beside_its_mob() {
    a_fire_into_a_sleeping_desks_session_loads_its_mob("a one-time schedule", false, false).await;
}

/// As the one-time schedule's row, for the first occurrence of an interval
/// schedule.
#[tokio::test(flavor = "multi_thread")]
async fn an_interval_schedule_fire_into_a_member_session_gives_no_turn_beside_its_mob() {
    a_fire_into_a_sleeping_desks_session_loads_its_mob("an interval schedule", false, true).await;
}

/// As the one-time schedule's row, for a member private to a person: the
/// fire is the host's, and whom the member is private to changes nothing
/// of it.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_fire_into_a_private_members_session_gives_no_turn_beside_its_mob() {
    a_fire_into_a_sleeping_desks_session_loads_its_mob("a member private to a person", true, false)
        .await;
}

/// A schedule fire into the session of a member of a stopped mob that is
/// not loaded fails no delivery and loads nothing. A fire does not start a
/// stopped mob, as work does not: the mob refuses the fire in the form
/// that may be tried again, so the occurrence is held open and is not
/// ended as a failed delivery. While the mob stays stopped the mob is not
/// loaded and the member's session holds no word of it: after the fire the
/// mob set still lists the mob as not loaded and as stopped.
///
/// What becomes of the held occurrence then is the schedule driver's, shown
/// by its own rows: it is tried again at the end of its claim's lease where
/// the schedule's misfire window is still open, and ends misfired where it
/// is not.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_fire_into_a_member_session_of_a_stopped_mob_fails_no_delivery() {
    const WORD: &str = "the scheduled word for the session of a desk of a stopped mob";
    const HELD_OPEN_FOR: Duration = Duration::from_secs(8);
    let hosted = SleepingDesk::boot(false, NotLoaded::Stopped).await;
    let due = chrono::Utc::now() + chrono::Duration::seconds(1);
    let phase = hosted
        .fire_into_the_desks_session(
            serde_json::json!({ "type": "once", "due_at_utc": due.to_rfc3339() }),
            WORD,
            HELD_OPEN_FOR,
        )
        .await;
    let ended = [
        OccurrencePhase::Completed,
        OccurrencePhase::Skipped,
        OccurrencePhase::Misfired,
        OccurrencePhase::Superseded,
        OccurrencePhase::DeliveryFailed,
    ]
    .iter()
    .any(|terminal| phase == Some(format!("{terminal:?}")));
    let got_the_word = transcript(&hosted.sessions, &hosted.desk_session)
        .await
        .contains(WORD);
    // What the set lists after the fire: a mob the set could not load
    // would refuse the fire in the same form, so the row reads that this
    // one is not loaded because it is stopped.
    let listed = set_of(&hosted.host)
        .mob_inventory()
        .await
        .expect("the mob set's inventory");
    let loaded = listed.managed.iter().any(|(id, _)| id == &hosted.second);
    let listed_as_stopped =
        listed.dormant.contains(&hosted.second) && listed.dormant_stopped.contains(&hosted.second);
    let taken_up = phase.is_some() && phase != Some(format!("{:?}", OccurrencePhase::Pending));
    assert_eq!(
        (taken_up, ended, loaded, listed_as_stopped, got_the_word),
        (true, false, false, true, false),
        "the fire into the session of a member of a stopped mob that is not loaded is taken \
         up and held open, loads nothing and gives the member no word: its occurrence is \
         {phase:?}, and the set lists as not loaded {:?} and as stopped {:?}",
        listed.dormant,
        listed.dormant_stopped
    );
    hosted.stop().await;
}
