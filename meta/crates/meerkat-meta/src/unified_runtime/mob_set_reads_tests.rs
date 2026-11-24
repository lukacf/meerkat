//! Rows of the reads that go through the mobs of a host's set, on a host
//! that serves a second mob with a member of its own: each read answers for
//! the launch mob's member, and a member of the second mob is read as that
//! mob's and no other's.
//!
//! The fixture is the declared launch of the entries' rows
//! ([`super::mob_entries_tests`]) with a second mob the set built for the
//! host. Both mobs hold a member `desk`; the launch mob also holds `clerk`,
//! whose schedule tools the rows call. The second mob's id sorts before the
//! launch mob's, so a read that goes through the set mob by mob meets the
//! second mob first.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat::{
    CreateScheduleRequest, IdentityTargetBinding, MemoryScheduleStore, MobTargetBinding,
    ScheduleId, ScheduleService, ScheduledMobAction, ScheduledSessionAction, TargetBinding,
    TriggerSpec,
};
use meerkat_mob::{MobId, SpawnMemberSpec};
use serde_json::json;

use super::UnifiedRuntime;
use super::mob_entries_tests::{definition, launch, second_mob, set_of, shut_down};

/// The person a private `desk` is private to in these rows.
const PERSON: &str = "console-caller:v1:the-person-of-these-rows";

/// Which of the two mobs holds a `desk` that is private to [`PERSON`]; the
/// other mob's `desk` is private to no one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivateDesk {
    /// The launch mob's.
    OfTheLaunchMob,
    /// The second mob's.
    OfTheSecondMob,
}

/// A host of two mobs with a `desk` in each.
pub(crate) struct HostOfTwoMobs {
    pub(crate) host: UnifiedRuntime,
    pub(crate) launch_id: MobId,
    pub(crate) second_id: MobId,
    pub(crate) second: meerkat_mob::MobHandle,
}

impl HostOfTwoMobs {
    /// The host, its second mob, `clerk` and `desk` in the launch mob and
    /// `desk` in the second mob, the `desk` that `private` names private to
    /// [`PERSON`].
    pub(crate) async fn boot(temp: &tempfile::TempDir, private: PrivateDesk) -> Self {
        let (host, _sessions) = launch(temp, definition("host")).await;
        let launch_id = host.mob_handle().mob_id().clone();
        let second_id = second_mob(&host, definition("a-second")).await;
        assert!(
            second_id < launch_id,
            "the second mob's id sorts before the launch mob's: {second_id}, {launch_id}"
        );
        let second = set_of(&host)
            .handle_for(&second_id)
            .await
            .expect("the set serves the second mob");
        let desk = |is_private: bool| {
            let spec = SpawnMemberSpec::new("worker", "desk");
            if is_private {
                spec.with_labels(BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    PERSON.to_string(),
                )]))
            } else {
                spec
            }
        };
        Box::pin(host.spawn(SpawnMemberSpec::new("worker", "clerk")))
            .await
            .expect("the launch mob's clerk");
        // Each `desk` is seated through its mob's own handle, as a host
        // seats a member: the runtime's member creation takes no label that
        // only Meta writes from its caller.
        host.mob_handle()
            .spawn_spec(desk(private == PrivateDesk::OfTheLaunchMob))
            .await
            .expect("the launch mob's desk");
        second
            .spawn_spec(desk(private == PrivateDesk::OfTheSecondMob))
            .await
            .expect("the second mob's desk");
        Self {
            host,
            launch_id,
            second_id,
            second,
        }
    }

    /// The session of the launch mob's `clerk`.
    async fn clerk(&self) -> meerkat_core::types::SessionId {
        self.host
            .mob_handle()
            .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id("clerk"))
            .await
            .expect("the launch mob's clerk holds a session")
    }

    pub(crate) async fn stop(self) {
        shut_down(&self.host, &[&self.second_id]).await;
    }
}

/// A schedule that fires once, a day from now, at `target`.
async fn schedule(service: &ScheduleService, name: &str, target: TargetBinding) -> ScheduleId {
    service
        .create(CreateScheduleRequest {
            name: Some(name.to_owned()),
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
        .unwrap_or_else(|error| panic!("the schedule {name} is created: {error}"))
        .schedule_id
}

/// A target that names the agent `desk` and no mob.
fn desk_by_name_alone() -> TargetBinding {
    TargetBinding::identity(IdentityTargetBinding::resumable(
        "desk",
        ScheduledSessionAction::Prompt {
            prompt: meerkat_core::ContentInput::Text("Check in.".into()),
            system_prompt: None,
            render_metadata: None,
            skill_refs: Vec::new(),
            additional_instructions: Vec::new(),
        },
    ))
}

/// A target that names the member `desk` of the mob `mob_id`.
fn desk_of(mob_id: &MobId) -> TargetBinding {
    member_of(mob_id, "desk")
}

/// A target that names the member `member` of the mob `mob_id`.
fn member_of(mob_id: &MobId, member: &str) -> TargetBinding {
    TargetBinding::Mob(Box::new(MobTargetBinding::Member {
        mob_id: mob_id.to_string(),
        member_id: crate::member_comms_id::mob_member_id(member).to_string(),
        action: ScheduledMobAction::Send {
            content: meerkat_core::ContentInput::Text("Check in.".into()),
            render_metadata: None,
        },
    }))
}

/// The schedules the launch mob's `clerk` is listed by its schedule tools.
async fn listed_to_the_clerk(hosted: &HostOfTwoMobs, service: &ScheduleService) -> String {
    let tools = crate::schedule_wiring::agent_schedule_tools_over_runtime_for_test(
        service.clone(),
        hosted.host.mob_runtime(),
    );
    let args = serde_json::value::RawValue::from_string(json!({}).to_string())
        .expect("the arguments of a list");
    let outcome = tools
        .dispatch_with_context(
            meerkat_core::types::ToolCallView {
                id: "schedule-tool-call",
                name: "meerkat_schedule_list",
                args: &args,
            },
            &meerkat_core::ToolDispatchContext::default()
                .with_runtime_identity(hosted.clerk().await, None),
        )
        .await
        .expect("the clerk's schedule tools list");
    outcome.result.text_content()
}

/// A schedule whose target names an agent and no mob is for the launch
/// mob's agent: the launch delivers it there. Who that agent is private to
/// is read from the launch mob's member. Here the launch mob's `desk` is
/// private to no one and the second mob's `desk` is private to a person:
/// another agent of the launch mob is listed the schedule for `desk` by
/// name alone, and is not listed the one for the second mob's `desk`.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_that_names_no_mob_is_listed_whatever_another_mobs_member_of_that_name_is() {
    let temp = tempfile::tempdir().expect("tempdir");
    let hosted = HostOfTwoMobs::boot(&temp, PrivateDesk::OfTheSecondMob).await;
    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
    let by_name_alone = schedule(&service, "the desk by name alone", desk_by_name_alone()).await;
    let of_the_second = schedule(
        &service,
        "the desk of the second mob",
        desk_of(&hosted.second_id),
    )
    .await;
    let listed = listed_to_the_clerk(&hosted, &service).await;
    assert!(
        !listed.contains(&of_the_second.to_string()),
        "a schedule for the second mob's private member is not listed to another agent: {listed}"
    );
    assert!(
        listed.contains(&by_name_alone.to_string()),
        "a schedule that names no mob is for the launch mob's member, private to no one, and \
         is listed: {listed}"
    );
    hosted.stop().await;
}

/// The same read with the two members the other way round: the launch
/// mob's `desk` is private to a person and the second mob's `desk` is
/// private to no one. A schedule for `desk` by name alone is for the launch
/// mob's private member: another agent of the launch mob is not listed it,
/// nor the one that names the launch mob's `desk` with its mob. That agent
/// is listed a schedule for a member of its own mob that is private to no
/// one, and is not listed the one for the second mob's `desk`: an agent's
/// schedule tools show no schedule that names another mob.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_that_names_no_mob_is_read_as_the_launch_mobs_private_members() {
    let temp = tempfile::tempdir().expect("tempdir");
    let hosted = HostOfTwoMobs::boot(&temp, PrivateDesk::OfTheLaunchMob).await;
    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
    let by_name_alone = schedule(&service, "the desk by name alone", desk_by_name_alone()).await;
    let of_the_launch = schedule(
        &service,
        "the desk of the launch mob",
        desk_of(&hosted.launch_id),
    )
    .await;
    let of_the_second = schedule(
        &service,
        "the desk of the second mob",
        desk_of(&hosted.second_id),
    )
    .await;
    let of_the_clerk = schedule(
        &service,
        "the clerk of the launch mob",
        member_of(&hosted.launch_id, "clerk"),
    )
    .await;
    let listed = listed_to_the_clerk(&hosted, &service).await;
    assert!(
        listed.contains(&of_the_clerk.to_string()),
        "a schedule for a member of the launch mob that is private to no one is listed: {listed}"
    );
    assert!(
        !listed.contains(&of_the_second.to_string()),
        "a schedule that names the second mob is not listed to an agent of the launch mob: \
         {listed}"
    );
    assert!(
        !listed.contains(&of_the_launch.to_string()),
        "a schedule for the launch mob's private member is not listed to another agent: {listed}"
    );
    assert!(
        !listed.contains(&by_name_alone.to_string()),
        "a schedule that names no mob is for the launch mob's private member and is not \
         listed to another agent: {listed}"
    );
    hosted.stop().await;
}

/// The record of who members are private to keeps a member of a mob the
/// host created under that mob: the second mob is recorded as created by no
/// member (private to no one), its private `desk` is private to its person
/// under the second mob's id, and the launch mob's `desk` is private to no
/// one.
#[tokio::test(flavor = "multi_thread")]
async fn the_private_owner_record_keeps_a_host_created_mobs_member_under_its_own_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let hosted = HostOfTwoMobs::boot(&temp, PrivateDesk::OfTheSecondMob).await;
    let owners = hosted.host.mob_runtime().private_owners().clone();
    owners
        .first_attempts_settled(crate::private_owners::FIRST_READ_WAIT)
        .await;
    let desk = crate::member_comms_id::mob_member_id("desk").to_string();
    assert_eq!(
        owners.recorded_mob_owner(hosted.second_id.as_str()),
        Some(None),
        "a mob the host created is recorded as private to no one"
    );
    let label_of = |handle: meerkat_mob::MobHandle| {
        let desk = crate::member_comms_id::mob_member_id("desk");
        async move {
            handle
                .get_member(&desk)
                .await
                .expect("the roster is read")
                .and_then(|entry| crate::access::private_owner(&entry.labels).map(str::to_string))
        }
    };
    assert_eq!(
        label_of(hosted.second.clone()).await.as_deref(),
        Some(PERSON),
        "the second mob's desk carries its person"
    );
    assert_eq!(
        label_of(hosted.host.mob_handle()).await,
        None,
        "the launch mob's desk carries no person"
    );
    assert_eq!(
        owners.owner_of(hosted.launch_id.as_str(), &desk),
        None,
        "the record names no person for the launch mob's desk"
    );
    hosted.stop().await;
}

/// The record of whom members are private to answers the person of a
/// private member of either mob when it is asked with that member's mob,
/// and answers no person for the other mob's member of the same name nor
/// for a name the asked mob does not hold. Read once with the private
/// member in the second mob and once with it in the launch mob.
#[tokio::test(flavor = "multi_thread")]
async fn the_private_owner_record_answers_the_person_of_a_second_mobs_private_member() {
    let desk = crate::member_comms_id::mob_member_id("desk").to_string();
    let clerk = crate::member_comms_id::mob_member_id("clerk").to_string();
    for private in [PrivateDesk::OfTheSecondMob, PrivateDesk::OfTheLaunchMob] {
        let temp = tempfile::tempdir().expect("tempdir");
        let hosted = HostOfTwoMobs::boot(&temp, private).await;
        let owners = hosted.host.mob_runtime().private_owners().clone();
        owners
            .first_attempts_settled(crate::private_owners::FIRST_READ_WAIT)
            .await;
        let (of_the_private, of_the_other) = match private {
            PrivateDesk::OfTheSecondMob => (&hosted.second_id, &hosted.launch_id),
            PrivateDesk::OfTheLaunchMob => (&hosted.launch_id, &hosted.second_id),
        };
        assert_eq!(
            owners.owner_of(of_the_private.as_str(), &desk).as_deref(),
            Some(PERSON),
            "asked with the mob of the private desk ({of_the_private}), the record answers \
             its person"
        );
        assert_eq!(
            owners.owner_of(of_the_other.as_str(), &desk),
            None,
            "asked with the other mob ({of_the_other}), the record answers no person for \
             its desk"
        );
        assert_eq!(
            owners.owner_of(hosted.second_id.as_str(), &clerk),
            None,
            "the record answers no person for a name the second mob does not hold"
        );
        hosted.stop().await;
    }
}
