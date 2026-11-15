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

/// A session target that names `session`.
fn session_target(session: &meerkat_core::types::SessionId) -> TargetBinding {
    TargetBinding::session(meerkat::SessionTargetBinding::ResumableSession {
        session_id: session.clone(),
        action: ScheduledSessionAction::Prompt {
            prompt: meerkat_core::ContentInput::Text("Check in.".into()),
            system_prompt: None,
            render_metadata: None,
            skill_refs: Vec::new(),
            additional_instructions: Vec::new(),
        },
    })
}

/// A target that forks a helper named `helper` from the member `source`.
fn fork_helper_target(source: &str, helper: &str) -> TargetBinding {
    TargetBinding::Mob(Box::new(MobTargetBinding::ForkHelper {
        mob_id: MOB.to_owned(),
        source_member_id: source.to_owned(),
        member_id: helper.to_owned(),
        prompt: "Check in.".to_owned(),
        fork_context: meerkat::ForkContextSpec::default(),
        options: meerkat::HelperOptionsSpec::default(),
    }))
}

/// A target that spawns a helper named `helper`.
fn spawn_helper_target(helper: &str) -> TargetBinding {
    TargetBinding::Mob(Box::new(MobTargetBinding::SpawnHelper {
        mob_id: MOB.to_owned(),
        member_id: helper.to_owned(),
        prompt: "Check in.".to_owned(),
        options: meerkat::HelperOptionsSpec::default(),
    }))
}

/// The end of a member private to a person deletes every schedule that
/// names it, whoever made it and in whichever form: a member target, an
/// identity target, a session target of the session it held, a fork helper
/// whose source it is, and a Console caller's deferred send to it. A
/// schedule that names another member, another session, or only a helper
/// to be seated under the name, stays as it is; so does everything when the
/// same end is handed over again.
#[tokio::test]
async fn the_end_of_a_private_member_deletes_every_schedule_that_names_it() {
    use crate::private_owners::{EndedPrivateMember, PrivateMemberEndFollower};
    use SchedulePhase::{Active, Deleted};

    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
    let following = super::EndedPrivateMemberSchedules::new(service.clone());
    let held = meerkat_core::types::SessionId::new();
    let other = meerkat_core::types::SessionId::new();

    let mut ended = Vec::new();
    for (name, target) in [
        ("member", member_target("side")),
        ("identity", identity_target("side")),
        ("session", session_target(&held)),
        ("fork helper source", fork_helper_target("side", "helper")),
        ("console send", console_target("side")),
    ] {
        ended.push((name, schedule(&service, name, target).await));
    }
    let mut kept = Vec::new();
    for (name, target) in [
        ("another member", member_target("scout")),
        ("another identity", identity_target("scout")),
        ("another session", session_target(&other)),
        (
            "another fork helper source",
            fork_helper_target("scout", "side"),
        ),
        ("a helper under the name", spawn_helper_target("side")),
        ("another console send", console_target("scout")),
    ] {
        kept.push((name, schedule(&service, name, target).await));
    }

    let end = EndedPrivateMember {
        mob_id: MOB.to_owned(),
        member: "side".to_owned(),
        session: Some(held.to_string()),
        ended_at: chrono::Utc::now(),
    };
    for pass in ["the end", "the end handed over again"] {
        following.member_ended(&end).await;
        for (name, id) in &ended {
            assert_eq!(state(&service, id).await.0, Deleted, "{pass}: {name}");
        }
        for (name, id) in &kept {
            assert_eq!(state(&service, id).await, (Active, false), "{pass}: {name}");
        }
    }

    // An identity target that names the member by the name its roster id
    // spells (`desk:front`) is the member's too.
    let by_name = schedule(&service, "by name", identity_target("desk:front")).await;
    let another = schedule(&service, "another name", identity_target("desk:back")).await;
    following
        .member_ended(&EndedPrivateMember {
            mob_id: MOB.to_owned(),
            member: crate::member_comms_id::mob_member_id_str("desk:front").into_owned(),
            session: None,
            ended_at: chrono::Utc::now(),
        })
        .await;
    assert_eq!(state(&service, &by_name).await.0, Deleted, "by name");
    assert_eq!(
        state(&service, &another).await,
        (Active, false),
        "another name"
    );
}

/// A schedule created after a member's end is not the ended member's,
/// though it names the same member: the end deletes the schedules that were
/// there when the member ended, and leaves the later one as it is.
#[tokio::test]
async fn a_schedule_created_after_a_private_members_end_is_not_deleted_with_it() {
    use crate::private_owners::{EndedPrivateMember, PrivateMemberEndFollower};
    use SchedulePhase::{Active, Deleted};

    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
    let following = super::EndedPrivateMemberSchedules::new(service.clone());
    let earlier = schedule(&service, "earlier", member_target("side")).await;
    let ended_at = chrono::Utc::now();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let later = schedule(&service, "later", member_target("side")).await;

    following
        .member_ended(&EndedPrivateMember {
            mob_id: MOB.to_owned(),
            member: "side".to_owned(),
            session: None,
            ended_at,
        })
        .await;
    assert_eq!(state(&service, &earlier).await.0, Deleted, "earlier");
    assert_eq!(state(&service, &later).await, (Active, false), "later");
}

/// A launch's schedules whose every delete fails, as a schedule store that
/// refuses the write does.
struct RefusedDeletes {
    service: ScheduleService,
    asked: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl super::EndedMemberSchedules for RefusedDeletes {
    async fn listed(
        &self,
    ) -> Result<(Vec<meerkat::Schedule>, Vec<String>), meerkat::ScheduleDomainError> {
        Ok((self.service.list().await?, Vec::new()))
    }

    async fn delete(&self, _schedule_id: &ScheduleId) -> Result<(), meerkat::ScheduleDomainError> {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(meerkat::ScheduleDomainError::Internal(
            "the store refuses the write".to_owned(),
        ))
    }
}

/// The log lines a row's own thread writes.
#[derive(Clone, Default)]
struct CapturedLog(Arc<std::sync::Mutex<Vec<u8>>>);

impl CapturedLog {
    fn contents(&self) -> String {
        String::from_utf8_lossy(
            &self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    }
}

impl std::io::Write for CapturedLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLog {
    type Writer = CapturedLog;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// When the schedules that target an ended private member cannot be
/// deleted, the delete is tried three times, the schedules stay as they
/// are, and one warning names the member they target.
#[tokio::test]
async fn schedules_that_cannot_be_deleted_stay_and_the_warning_names_their_member() {
    use crate::private_owners::{EndedPrivateMember, PrivateMemberEndFollower};

    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
    let aimed = schedule(&service, "aimed", member_target("side")).await;
    let refused = Arc::new(RefusedDeletes {
        service: service.clone(),
        asked: std::sync::atomic::AtomicUsize::new(0),
    });
    let following = super::EndedPrivateMemberSchedules::over(refused.clone());

    let log = CapturedLog::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .finish();
    // A second registered Dispatch keeps callsite interest from being
    // cached `never` by threads without a subscriber.
    let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
    let logged = tracing::subscriber::set_default(subscriber);
    following
        .member_ended(&EndedPrivateMember {
            mob_id: MOB.to_owned(),
            member: "side".to_owned(),
            session: None,
            ended_at: chrono::Utc::now(),
        })
        .await;
    drop(logged);

    assert_eq!(
        refused.asked.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "the delete is tried three times"
    );
    assert_eq!(
        state(&service, &aimed).await,
        (SchedulePhase::Active, false),
        "the schedule stays as it is"
    );
    let log = log.contents();
    let warning = log
        .lines()
        .find(|line| {
            line.contains(
                "the schedules that target an ended member private to a person could not all \
                 be deleted",
            )
        })
        .unwrap_or_else(|| panic!("the failure is logged: {log}"));
    assert!(warning.contains("WARN"), "{warning}");
    assert!(warning.contains("target_member=side"), "{warning}");
    assert!(warning.contains("mob_id=follow-mob"), "{warning}");
}
