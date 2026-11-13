//! Which schedules are an agent's, and what a follow of its archive, its
//! unarchive or its removal answers ([`MobAgentSchedules`]).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat::{
    CreateScheduleRequest, IdentityTargetBinding, MemoryScheduleStore, MobTargetBinding,
    ScheduleId, SchedulePhase, ScheduleService, ScheduledMobAction, ScheduledSessionAction,
    TargetBinding, TriggerSpec,
};

use super::{MobAgentSchedules, ScheduleMobTargetRegistry};
use crate::console_schedules::{ConsoleScheduleSender, ConsoleScheduledSend};
use crate::identity_creation::{AgentSchedules, ScheduleEffect, SchedulesFollowed};

const MOB: &str = "follow-mob";

fn schedules() -> (ScheduleService, MobAgentSchedules) {
    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
    let following = MobAgentSchedules::new(service.clone(), ScheduleMobTargetRegistry::default());
    (service, following)
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

/// An identity target that names `agent`.
fn identity_target(agent: &str) -> TargetBinding {
    TargetBinding::identity(IdentityTargetBinding::resumable(
        agent,
        ScheduledSessionAction::Prompt {
            prompt: meerkat_core::ContentInput::Text("Check in.".into()),
            system_prompt: None,
            render_metadata: None,
            skill_refs: Vec::new(),
            additional_instructions: Vec::new(),
        },
    ))
}

/// A member target that names the member `member` of the mob.
fn member_target(member: &str) -> TargetBinding {
    TargetBinding::Mob(Box::new(MobTargetBinding::Member {
        mob_id: MOB.to_owned(),
        member_id: member.to_owned(),
        action: ScheduledMobAction::Send {
            content: meerkat_core::ContentInput::Text("Check in.".into()),
            render_metadata: None,
        },
    }))
}

/// The target of a Console caller's schedule for `agent`.
fn console_target(agent: &str) -> TargetBinding {
    ConsoleScheduledSend::new(
        agent.to_owned(),
        "Post the morning summary.".to_owned(),
        ConsoleScheduleSender::Anonymous,
        0,
    )
    .target()
    .expect("a console schedule's target")
}

async fn state(service: &ScheduleService, id: &ScheduleId) -> (SchedulePhase, bool) {
    let schedule = service.get(id).await.expect("the schedule is read");
    (schedule.phase, schedule.paused_for_target())
}

/// The names an answer lists, sorted, with its count checked against them.
fn listed(followed: &SchedulesFollowed) -> Vec<String> {
    assert_eq!(followed.count, followed.schedules.len(), "{followed:?}");
    assert!(!followed.incomplete, "{followed:?}");
    let mut names: Vec<String> = followed
        .schedules
        .iter()
        .map(|schedule| schedule.name.clone().expect("a name"))
        .collect();
    names.sort_unstable();
    names
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(ToString::to_string).collect()
}

/// An agent's schedules are the schedules that name it in any form a
/// schedule names an agent: an identity target, a member target, and a
/// Console caller's schedule for it. Each follows the agent's archive, its
/// unarchive and its removal. Another agent's schedule of each form is left
/// alone.
#[tokio::test]
async fn the_console_schedules_of_an_agent_follow_it_as_its_other_schedules_do() {
    use SchedulePhase::{Active, Deleted, Paused};
    let (service, following) = schedules();
    let by_identity = schedule(&service, "by-identity", identity_target("scout")).await;
    let by_member = schedule(&service, "by-member", member_target("scout")).await;
    let by_console = schedule(&service, "by-console", console_target("scout")).await;
    let others = [
        schedule(&service, "other-identity", identity_target("ledger")).await,
        schedule(&service, "other-member", member_target("ledger")).await,
        schedule(&service, "other-console", console_target("ledger")).await,
    ];
    let all = names(&["by-console", "by-identity", "by-member"]);

    let planned = following
        .planned(MOB, "scout", ScheduleEffect::Paused)
        .await;
    assert_eq!(listed(&planned), all, "a plan of the archive");
    let paused = following.follow(MOB, "scout", ScheduleEffect::Paused).await;
    assert_eq!(listed(&paused), all, "the archive");
    for id in [&by_identity, &by_member, &by_console] {
        assert_eq!(state(&service, id).await, (Paused, true));
    }
    for id in &others {
        assert_eq!(state(&service, id).await, (Active, false), "another's");
    }

    let resumed = following
        .follow(MOB, "scout", ScheduleEffect::Resumed)
        .await;
    assert_eq!(listed(&resumed), all, "the unarchive");
    for id in [&by_identity, &by_member, &by_console] {
        assert_eq!(state(&service, id).await, (Active, false));
    }

    let ended = following.follow(MOB, "scout", ScheduleEffect::Ended).await;
    assert_eq!(listed(&ended), all, "the removal");
    for id in [&by_identity, &by_member, &by_console] {
        assert_eq!(state(&service, id).await.0, Deleted);
    }
    for id in &others {
        assert_eq!(state(&service, id).await, (Active, false), "another's");
    }
}

/// An agent whose name no member id spells as it is (`desk:front`) is
/// followed by the schedules that name it by its name, as an agent of a
/// plain name is.
#[tokio::test]
async fn a_schedule_that_names_an_agent_by_a_name_no_member_id_spells_follows_it() {
    use SchedulePhase::{Active, Paused};
    let (service, following) = schedules();
    let named = schedule(&service, "named", identity_target("desk:front")).await;
    let other = schedule(&service, "other", identity_target("desk:back")).await;

    let paused = following
        .follow(MOB, "desk:front", ScheduleEffect::Paused)
        .await;
    assert_eq!(listed(&paused), names(&["named"]));
    assert_eq!(state(&service, &named).await, (Paused, true));
    assert_eq!(state(&service, &other).await, (Active, false));
}

/// A follow answers the agent's schedules as they stand after it, whichever
/// call brought them there: an archive lists the schedules that are paused
/// for the agent, an unarchive the schedules that are active, a removal the
/// ended ones. So a follow that is asked again after one that did only a
/// part answers as a whole one does, and a plan lists what the follow would
/// answer. A schedule somebody paused by name is on no list of an archive or
/// an unarchive.
#[tokio::test]
async fn a_follow_asked_again_after_a_partial_one_answers_as_a_whole_one() {
    use SchedulePhase::{Active, Paused};
    let (service, following) = schedules();
    let first = schedule(&service, "first", identity_target("scout")).await;
    let second = schedule(&service, "second", identity_target("scout")).await;
    let held = schedule(&service, "held", identity_target("scout")).await;
    service.pause(&held).await.expect("a pause by name");
    let both = names(&["first", "second"]);

    // An archive whose first asking paused one schedule and then stopped.
    service
        .pause_for_target(&first)
        .await
        .expect("the part an earlier asking did");
    let planned = following
        .planned(MOB, "scout", ScheduleEffect::Paused)
        .await;
    assert_eq!(listed(&planned), both, "a plan of the archive");
    let paused = following.follow(MOB, "scout", ScheduleEffect::Paused).await;
    assert_eq!(listed(&paused), both, "the archive asked again");
    let again = following.follow(MOB, "scout", ScheduleEffect::Paused).await;
    assert_eq!(listed(&again), both, "and once more");
    assert_eq!(state(&service, &held).await, (Paused, false));

    // An unarchive whose first asking resumed one schedule and then
    // stopped.
    service
        .resume_for_target(&first)
        .await
        .expect("the part an earlier asking did");
    let planned = following
        .planned(MOB, "scout", ScheduleEffect::Resumed)
        .await;
    assert_eq!(listed(&planned), both, "a plan of the unarchive");
    let resumed = following
        .follow(MOB, "scout", ScheduleEffect::Resumed)
        .await;
    assert_eq!(listed(&resumed), both, "the unarchive asked again");
    let again = following
        .follow(MOB, "scout", ScheduleEffect::Resumed)
        .await;
    assert_eq!(listed(&again), both, "and once more");
    for id in [&first, &second] {
        assert_eq!(state(&service, id).await, (Active, false));
    }
    assert_eq!(state(&service, &held).await, (Paused, false));
}
