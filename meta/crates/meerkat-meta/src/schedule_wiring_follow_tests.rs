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

/// The host's default mob in these rows: [`MOB`].
fn default_mob() -> crate::member_key::DefaultMob {
    crate::member_key::DefaultMob::new(meerkat_mob::MobId::from(MOB))
}

/// The schedules of a launch whose mob, the host's default mob, is [`MOB`].
fn schedules() -> (ScheduleService, MobAgentSchedules) {
    schedules_of(
        ScheduleMobTargetRegistry::default().with_default_mob_for_test(
            crate::member_key::DefaultMob::new(meerkat_mob::MobId::from(MOB)),
        ),
    )
}

/// The schedules of a launch whose targets `registry` resolves.
fn schedules_of(registry: ScheduleMobTargetRegistry) -> (ScheduleService, MobAgentSchedules) {
    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
    let following = MobAgentSchedules::new(service.clone(), registry);
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
    // The followed mob is the host's default mob, so the send names no mob.
    let default_mob = crate::member_key::DefaultMob::new(meerkat_mob::MobId::from(MOB));
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

/// The target of a Console caller's schedule for the agent `agent` of the
/// mob `mob_id`, on a host whose default mob is the followed mob: a send for
/// the followed mob's agent names no mob, a send for another mob's agent
/// names that mob.
fn console_target_in(mob_id: &str, agent: &str) -> TargetBinding {
    let default_mob = crate::member_key::DefaultMob::new(meerkat_mob::MobId::from(MOB));
    ConsoleScheduledSend::new(
        &default_mob,
        &meerkat_mob::MobId::from(mob_id),
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

/// A Console caller's schedule is the schedule of one mob's agent: the agent
/// of the same name in another mob is another agent. The archive, the
/// unarchive and the removal of the followed mob's agent leave the other
/// mob's schedule active and present and do not list it, and a plan for the
/// other mob's agent lists that mob's schedule alone.
#[tokio::test]
async fn a_console_schedule_of_the_agent_of_that_name_in_another_mob_does_not_follow_it() {
    use SchedulePhase::{Active, Deleted, Paused};
    const OTHER_MOB: &str = "second-mob";
    let (service, following) = schedules();
    let own = schedule(&service, "own", console_target_in(MOB, "scout")).await;
    let other = schedule(&service, "other-mob", console_target_in(OTHER_MOB, "scout")).await;
    let own_only = names(&["own"]);

    let planned = following
        .planned(MOB, "scout", ScheduleEffect::Paused)
        .await;
    assert_eq!(listed(&planned), own_only, "a plan of the archive");
    let planned_other = following
        .planned(OTHER_MOB, "scout", ScheduleEffect::Paused)
        .await;
    assert_eq!(
        listed(&planned_other),
        names(&["other-mob"]),
        "a plan of the other mob's agent's archive"
    );

    let paused = following.follow(MOB, "scout", ScheduleEffect::Paused).await;
    assert_eq!(listed(&paused), own_only, "the archive");
    assert_eq!(state(&service, &own).await, (Paused, true));
    assert_eq!(
        state(&service, &other).await,
        (Active, false),
        "another mob's"
    );

    let resumed = following
        .follow(MOB, "scout", ScheduleEffect::Resumed)
        .await;
    assert_eq!(listed(&resumed), own_only, "the unarchive");
    assert_eq!(state(&service, &own).await, (Active, false));
    assert_eq!(
        state(&service, &other).await,
        (Active, false),
        "another mob's"
    );

    let ended = following.follow(MOB, "scout", ScheduleEffect::Ended).await;
    assert_eq!(listed(&ended), own_only, "the removal");
    assert_eq!(state(&service, &own).await.0, Deleted);
    assert_eq!(
        state(&service, &other).await,
        (Active, false),
        "another mob's schedule is active and present after the removal"
    );
}

/// A follow whose registry adopted no mob does not know the host's default
/// mob. It serves a host of one mob: a Console caller's schedule that names
/// no mob is then the followed agent's by its name, and follows the agent's
/// archive and its removal. A Console caller's schedule that names a mob is
/// that mob's agent's there too.
#[tokio::test]
async fn a_console_schedule_that_names_no_mob_follows_by_name_while_the_default_mob_is_not_known() {
    use SchedulePhase::{Active, Deleted, Paused};
    let (service, following) = schedules_of(ScheduleMobTargetRegistry::default());
    let by_console = schedule(&service, "by-console", console_target("scout")).await;
    let other = schedule(
        &service,
        "another-mobs",
        console_target_in(OTHER_MOB, "scout"),
    )
    .await;
    let own_only = names(&["by-console"]);

    let planned = following
        .planned(MOB, "scout", ScheduleEffect::Paused)
        .await;
    assert_eq!(listed(&planned), own_only, "a plan of the archive");
    let paused = following.follow(MOB, "scout", ScheduleEffect::Paused).await;
    assert_eq!(listed(&paused), own_only, "the archive");
    assert_eq!(state(&service, &by_console).await, (Paused, true));
    let ended = following.follow(MOB, "scout", ScheduleEffect::Ended).await;
    assert_eq!(listed(&ended), own_only, "the removal");
    assert_eq!(state(&service, &by_console).await.0, Deleted);
    assert_eq!(
        state(&service, &other).await,
        (Active, false),
        "another mob's schedule is active and present after the removal"
    );
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
    let following = super::EndedPrivateMemberSchedules::new(service.clone(), default_mob());
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
    let following = super::EndedPrivateMemberSchedules::new(service.clone(), default_mob());
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
    let following = super::EndedPrivateMemberSchedules::over(refused.clone(), default_mob());

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

/// A launch's schedules where the delete of one schedule fails every time
/// it is asked, and every other delete is the schedule service's own.
struct OneRefusedDelete {
    service: ScheduleService,
    refused: ScheduleId,
    asked: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl super::EndedMemberSchedules for OneRefusedDelete {
    async fn listed(
        &self,
    ) -> Result<(Vec<meerkat::Schedule>, Vec<String>), meerkat::ScheduleDomainError> {
        Ok((self.service.list().await?, Vec::new()))
    }

    async fn delete(&self, schedule_id: &ScheduleId) -> Result<(), meerkat::ScheduleDomainError> {
        if *schedule_id == self.refused {
            self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return Err(meerkat::ScheduleDomainError::Internal(
                "the store refuses the write".to_owned(),
            ));
        }
        self.service.delete(schedule_id).await.map(|_| ())
    }
}

/// Several schedules target an ended private member, and the delete of the
/// one listed first fails at every attempt. Every other one is deleted all
/// the same; the failing one stays as it is, and one warning names the
/// member they target.
#[tokio::test]
async fn a_schedule_whose_delete_fails_keeps_no_other_schedule_of_the_ended_member() {
    use crate::private_owners::{EndedPrivateMember, PrivateMemberEndFollower};

    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
    for name in ["first", "second", "third"] {
        schedule(&service, name, member_target("side")).await;
    }
    let listed: Vec<ScheduleId> = service
        .list()
        .await
        .expect("the schedules are listed")
        .into_iter()
        .map(|schedule| schedule.schedule_id)
        .collect();
    let (failing, others) = listed.split_first().expect("the first listed schedule");
    assert_eq!(others.len(), 2, "two other schedules target the member");
    let refused = Arc::new(OneRefusedDelete {
        service: service.clone(),
        refused: failing.clone(),
        asked: std::sync::atomic::AtomicUsize::new(0),
    });
    let following = super::EndedPrivateMemberSchedules::over(refused.clone(), default_mob());

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

    for other in others {
        assert_eq!(
            state(&service, other).await.0,
            SchedulePhase::Deleted,
            "a schedule listed after the failing one is deleted"
        );
    }
    assert_eq!(
        state(&service, failing).await,
        (SchedulePhase::Active, false),
        "the schedule whose delete fails stays as it is"
    );
    assert_eq!(
        refused.asked.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "the failing delete is tried at every attempt"
    );
    let log = log.contents();
    let warnings: Vec<&str> = log
        .lines()
        .filter(|line| {
            line.contains(
                "the schedules that target an ended member private to a person could not all \
                 be deleted",
            )
        })
        .collect();
    assert_eq!(warnings.len(), 1, "one warning: {log}");
    let warning = warnings.first().copied().unwrap_or_default();
    assert!(warning.contains("WARN"), "{warning}");
    assert!(warning.contains("target_member=side"), "{warning}");
}

/// Another mob of the host, beside its default mob [`MOB`].
const OTHER_MOB: &str = "follow-other-mob";

/// A member target that names the member `member` of the mob `mob_id`.
fn member_target_in(mob_id: &str, member: &str) -> TargetBinding {
    TargetBinding::Mob(Box::new(MobTargetBinding::Member {
        mob_id: mob_id.to_owned(),
        member_id: member.to_owned(),
        action: ScheduledMobAction::Send {
            content: meerkat_core::ContentInput::Text("Check in.".into()),
            render_metadata: None,
        },
    }))
}

/// An identity target that names the member `member` of the mob `mob_id`
/// by its mob, as a schedule names a member of a mob that is not the
/// default mob.
fn identity_target_in(mob_id: &str, member: &str) -> TargetBinding {
    identity_target(&meerkat::surface::mob_member_schedule_identity(
        &meerkat_core::MobMemberBinding {
            mob_id: mob_id.to_owned(),
            role: "worker".to_owned(),
            member: member.to_owned(),
        },
    ))
}

/// A member is one mob's. The end of a member private to a person in a mob
/// that is not the default mob deletes the schedules that name that member
/// of that mob, and leaves every schedule of the default mob's agent of the
/// same name: a Console caller's send and an identity target that name no
/// mob are for the default mob's agent, and so are the targets that name
/// the default mob.
#[tokio::test]
async fn the_end_of_a_private_member_of_another_mob_leaves_the_default_mobs_agent_of_that_name_its_schedules()
 {
    use crate::private_owners::{EndedPrivateMember, PrivateMemberEndFollower};
    use SchedulePhase::{Active, Deleted};

    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
    let following = super::EndedPrivateMemberSchedules::new(service.clone(), default_mob());

    let mut ended = Vec::new();
    for (name, target) in [
        (
            "the other mob's member",
            member_target_in(OTHER_MOB, "side"),
        ),
        (
            "the other mob's member, by an identity target",
            identity_target_in(OTHER_MOB, "side"),
        ),
        (
            "a console send to the other mob's agent",
            console_target_in(OTHER_MOB, "side"),
        ),
    ] {
        ended.push((name, schedule(&service, name, target).await));
    }
    let mut kept = Vec::new();
    for (name, target) in [
        (
            "a console send to the default mob's agent",
            console_target("side"),
        ),
        (
            "an identity target that names no mob",
            identity_target("side"),
        ),
        ("the default mob's member", member_target("side")),
        (
            "the default mob's member, by an identity target",
            identity_target_in(MOB, "side"),
        ),
    ] {
        kept.push((name, schedule(&service, name, target).await));
    }

    following
        .member_ended(&EndedPrivateMember {
            mob_id: OTHER_MOB.to_owned(),
            member: "side".to_owned(),
            session: None,
            ended_at: chrono::Utc::now(),
        })
        .await;
    for (name, id) in &kept {
        assert_eq!(
            state(&service, id).await,
            (Active, false),
            "left as it is: {name}"
        );
    }
    for (name, id) in &ended {
        assert_eq!(state(&service, id).await.0, Deleted, "deleted: {name}");
    }
}

/// The end of a member private to a person in the default mob deletes the
/// schedules of that member, the ones that name no mob included, and
/// leaves every schedule of another mob's agent of the same name.
#[tokio::test]
async fn the_end_of_a_private_member_of_the_default_mob_leaves_another_mobs_agent_of_that_name_its_schedules()
 {
    use crate::private_owners::{EndedPrivateMember, PrivateMemberEndFollower};
    use SchedulePhase::{Active, Deleted};

    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
    let following = super::EndedPrivateMemberSchedules::new(service.clone(), default_mob());

    let mut ended = Vec::new();
    for (name, target) in [
        (
            "a console send to the default mob's agent",
            console_target("side"),
        ),
        (
            "an identity target that names no mob",
            identity_target("side"),
        ),
        ("the default mob's member", member_target("side")),
        (
            "the default mob's member, by an identity target",
            identity_target_in(MOB, "side"),
        ),
    ] {
        ended.push((name, schedule(&service, name, target).await));
    }
    let mut kept = Vec::new();
    for (name, target) in [
        (
            "a console send to the other mob's agent",
            console_target_in(OTHER_MOB, "side"),
        ),
        (
            "the other mob's member",
            member_target_in(OTHER_MOB, "side"),
        ),
        (
            "the other mob's member, by an identity target",
            identity_target_in(OTHER_MOB, "side"),
        ),
    ] {
        kept.push((name, schedule(&service, name, target).await));
    }

    following
        .member_ended(&EndedPrivateMember {
            mob_id: MOB.to_owned(),
            member: "side".to_owned(),
            session: None,
            ended_at: chrono::Utc::now(),
        })
        .await;
    for (name, id) in &kept {
        assert_eq!(
            state(&service, id).await,
            (Active, false),
            "left as it is: {name}"
        );
    }
    for (name, id) in &ended {
        assert_eq!(state(&service, id).await.0, Deleted, "deleted: {name}");
    }
}

/// An identity target that names no mob is the default mob's agent's: the
/// launch itself delivers it. It does not follow the archive or the
/// removal of another mob's agent of the same name, whose own schedule
/// names its mob; it follows the default mob's agent.
#[tokio::test]
async fn an_identity_schedule_that_names_no_mob_does_not_follow_another_mobs_agent_of_that_name() {
    use SchedulePhase::{Active, Deleted, Paused};
    let (service, following) = schedules();
    let unnamed = schedule(&service, "names-no-mob", identity_target("scout")).await;
    let other = schedule(
        &service,
        "other-mob",
        identity_target_in(OTHER_MOB, "scout"),
    )
    .await;
    let other_only = names(&["other-mob"]);

    let planned = following
        .planned(OTHER_MOB, "scout", ScheduleEffect::Paused)
        .await;
    assert_eq!(
        listed(&planned),
        other_only,
        "a plan of the other mob's agent's archive"
    );
    let paused = following
        .follow(OTHER_MOB, "scout", ScheduleEffect::Paused)
        .await;
    assert_eq!(
        listed(&paused),
        other_only,
        "the other mob's agent's archive"
    );
    assert_eq!(state(&service, &other).await, (Paused, true));
    assert_eq!(
        state(&service, &unnamed).await,
        (Active, false),
        "the default mob's agent's schedule after the other mob's agent's archive"
    );

    let ended = following
        .follow(OTHER_MOB, "scout", ScheduleEffect::Ended)
        .await;
    assert_eq!(
        listed(&ended),
        other_only,
        "the other mob's agent's removal"
    );
    assert_eq!(state(&service, &other).await.0, Deleted);
    assert_eq!(
        state(&service, &unnamed).await,
        (Active, false),
        "the default mob's agent's schedule is active and present after the other mob's \
         agent's removal"
    );

    let paused = following.follow(MOB, "scout", ScheduleEffect::Paused).await;
    assert_eq!(
        listed(&paused),
        names(&["names-no-mob"]),
        "the default mob's agent's archive"
    );
    assert_eq!(state(&service, &unnamed).await, (Paused, true));
}

/// The registry's own answer, which the judge of a private member's
/// schedule target and the follow of an agent share: an identity target
/// that names no mob names the default mob's member of that name, and
/// names no member of another mob; a target that names its mob names that
/// mob's member.
#[tokio::test]
async fn a_target_that_names_no_mob_names_the_default_mobs_member_alone() {
    let registry = ScheduleMobTargetRegistry::default().with_default_mob_for_test(default_mob());
    let unnamed = identity_target("side");
    let of_the_other = identity_target_in(OTHER_MOB, "side");

    assert!(
        registry.target_names_member(&unnamed, MOB, "side").await,
        "a target that names no mob names the default mob's member"
    );
    assert!(
        !registry
            .target_names_member(&unnamed, OTHER_MOB, "side")
            .await,
        "and not the member of that name in another mob"
    );
    assert!(
        registry
            .target_names_member(&of_the_other, OTHER_MOB, "side")
            .await,
        "a target that names its mob names that mob's member"
    );
    assert!(
        !registry
            .target_names_member(&of_the_other, MOB, "side")
            .await,
        "and not the default mob's member of that name"
    );
}

/// A follow whose registry adopted no mob does not know the host's default
/// mob. It serves a host of one mob: an identity target that names no mob
/// is then the followed agent's by its name, and follows the agent's
/// archive.
#[tokio::test]
async fn an_identity_schedule_that_names_no_mob_follows_by_name_while_the_default_mob_is_not_known()
{
    use SchedulePhase::Paused;
    let (service, following) = schedules_of(ScheduleMobTargetRegistry::default());
    let unnamed = schedule(&service, "names-no-mob", identity_target("scout")).await;
    let own_only = names(&["names-no-mob"]);

    let planned = following
        .planned(MOB, "scout", ScheduleEffect::Paused)
        .await;
    assert_eq!(listed(&planned), own_only, "a plan of the archive");
    let paused = following.follow(MOB, "scout", ScheduleEffect::Paused).await;
    assert_eq!(listed(&paused), own_only, "the archive");
    assert_eq!(state(&service, &unnamed).await, (Paused, true));
}

// ---------------------------------------------------------------------------
// The release of a mob's schedules before a mob is created under its id
// ---------------------------------------------------------------------------

/// A mob a member created: the one a mob is created under the id of.
const CREATED: &str = "a-created-mob";
/// Another mob a member created.
const BESIDE: &str = "a-mob-beside-it";

fn mob(mob_id: &str) -> meerkat_mob::MobId {
    meerkat_mob::MobId::from(mob_id)
}

/// A flow or helper target of a mob, from its stored form.
fn mob_target(stored: serde_json::Value) -> TargetBinding {
    TargetBinding::Mob(Box::new(
        serde_json::from_value(stored).expect("a mob target's stored form"),
    ))
}

/// A flow target of the mob `mob_id`.
fn flow_target_in(mob_id: &str) -> TargetBinding {
    mob_target(serde_json::json!({
        "type": "flow",
        "mob_id": mob_id,
        "flow_id": "morning-round",
        "params": {},
    }))
}

/// A helper target that spawns a helper in the mob `mob_id`.
fn spawn_helper_target_in(mob_id: &str) -> TargetBinding {
    mob_target(serde_json::json!({
        "type": "spawn_helper",
        "mob_id": mob_id,
        "member_id": "helper",
        "prompt": "Sweep the inbox.",
    }))
}

/// A helper target that forks the member `source` of the mob `mob_id`.
fn fork_helper_target_in(mob_id: &str, source: &str) -> TargetBinding {
    mob_target(serde_json::json!({
        "type": "fork_helper",
        "mob_id": mob_id,
        "source_member_id": source,
        "member_id": "helper",
        "prompt": "Sweep the inbox.",
    }))
}

/// The runner schedule of an announcement source of the mob `mob_id`, as
/// Meta makes it.
async fn runner_schedule(service: &ScheduleService, name: &str, mob_id: &str) -> ScheduleId {
    let params = meerkat::HostRunnableParams::try_from(serde_json::json!({
        "mob_id": mob_id,
        "source_id": "inbox",
    }))
    .expect("runner params");
    service
        .create(CreateScheduleRequest {
            name: Some(name.to_owned()),
            description: None,
            trigger: TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
            },
            target: TargetBinding::host_runnable(meerkat::HostRunnableTargetBinding {
                runnable: meerkat::HostRunnableName::parse("announcements.collect")
                    .expect("a runnable name"),
                params: Some(params),
            }),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::from([(super::SOURCE_RUNNER_LABEL.to_owned(), "1".to_owned())]),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        })
        .await
        .unwrap_or_else(|error| panic!("the runner schedule {name} is created: {error}"))
        .schedule_id
}

/// One schedule in each form a schedule names the mob `mob_id`, named
/// `<prefix>-<form>`, with their ids.
async fn schedules_that_name(
    service: &ScheduleService,
    prefix: &str,
    mob_id: &str,
) -> Vec<(String, ScheduleId)> {
    let mut made = Vec::new();
    for (form, target) in [
        ("member", member_target_in(mob_id, "scout")),
        ("identity", identity_target_in(mob_id, "scout")),
        ("flow", flow_target_in(mob_id)),
        ("spawn-helper", spawn_helper_target_in(mob_id)),
        ("fork-helper", fork_helper_target_in(mob_id, "scout")),
    ] {
        let name = format!("{prefix}-{form}");
        let id = schedule(service, &name, target).await;
        made.push((name, id));
    }
    let name = format!("{prefix}-runner");
    let id = runner_schedule(service, &name, mob_id).await;
    made.push((name, id));
    made
}

/// The names a release answers, sorted.
fn released_names(released: &[crate::identity_creation::ScheduleRef]) -> Vec<String> {
    let mut names: Vec<String> = released
        .iter()
        .map(|schedule| schedule.name.clone().expect("a name"))
        .collect();
    names.sort_unstable();
    names
}

/// The release of a mob's schedules ends every schedule that names the
/// mob's id, in every form a schedule names a mob, and names each one it
/// ended.
#[tokio::test]
async fn a_release_ends_every_schedule_that_names_the_mob() {
    let (service, following) = schedules();
    let made = schedules_that_name(&service, "named", CREATED).await;
    let mut expected: Vec<String> = made.iter().map(|(name, _)| name.clone()).collect();
    expected.sort_unstable();

    let released = following
        .release_mob(&mob(CREATED))
        .await
        .expect("the release of the mob's schedules");

    assert_eq!(released_names(&released), expected, "the schedules ended");
    for (name, id) in &made {
        assert_eq!(
            state(&service, id).await.0,
            SchedulePhase::Deleted,
            "{name} is ended"
        );
    }
    let live = service.list().await.expect("the live schedules");
    assert!(
        live.is_empty(),
        "no schedule that names the mob is live: {live:?}"
    );
}

/// The release of one mob's schedules leaves every schedule that names
/// another mob, a session, an agent by its name alone, and a Console
/// caller's schedule for an agent of another mob as it is.
#[tokio::test]
async fn a_release_leaves_the_schedules_of_every_other_mob_and_of_sessions() {
    let (service, following) = schedules();
    let mut kept = schedules_that_name(&service, "beside", BESIDE).await;
    kept.extend(schedules_that_name(&service, "launch", MOB).await);
    for (name, target) in [
        ("a-console-callers", console_target("scout")),
        ("an-identity-by-name", identity_target("scout")),
        (
            "a-session",
            session_target(&meerkat_core::types::SessionId::new()),
        ),
    ] {
        let id = schedule(&service, name, target).await;
        kept.push((name.to_owned(), id));
    }

    let released = following
        .release_mob(&mob(CREATED))
        .await
        .expect("the release of another mob's schedules");

    assert!(released.is_empty(), "{released:?}");
    for (name, id) in &kept {
        assert_eq!(
            state(&service, id).await,
            (SchedulePhase::Active, false),
            "{name} is as it was"
        );
    }
}

/// The launch's own mob, the host's default mob, is never created again:
/// asked for it, the release ends nothing, whatever names that mob. A
/// registry knows its own mob from the runtime it adopted, so the launch
/// here is a real one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_asked_for_the_default_mob_ends_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{MOB}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
         runtime_mode = \"turn_driven\"\n[profiles.worker.tools]\ncomms = true\n"
    ))
    .expect("the launch mob's definition");
    let mut spec = crate::mob_handle_runtime::MobBootstrapSpec::ephemeral(
        definition,
        meerkat_mob::MobStorage::in_memory(),
        dir.path().to_path_buf(),
        4,
        None,
    )
    .expect("the launch's bootstrap spec");
    spec.options.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
        meerkat_core::Provider::OpenAI,
    )));
    let runtime = crate::mob_handle_runtime::MobRuntime::bootstrap(spec)
        .await
        .expect("the launch");
    let registry = ScheduleMobTargetRegistry::default();
    registry.adopt_mob_runtime(&runtime, &default_mob());
    let (service, following) = schedules_of(registry);
    let mut kept = schedules_that_name(&service, "default", MOB).await;
    let without = schedule(&service, "console-without-a-mob", console_target("scout")).await;
    kept.push(("console-without-a-mob".to_owned(), without));

    let released = following
        .release_mob(&mob(MOB))
        .await
        .expect("the release asked for the default mob");

    assert!(released.is_empty(), "{released:?}");
    for (name, id) in &kept {
        assert_eq!(
            state(&service, id).await,
            (SchedulePhase::Active, false),
            "{name} is as it was"
        );
    }
    runtime
        .handle()
        .shutdown()
        .await
        .expect("the launch's shutdown");
}

/// A release that cannot read the schedules is an error, so that the create
/// it stands in front of is refused.
#[tokio::test]
async fn a_release_that_cannot_list_the_schedules_is_an_error() {
    let service = ScheduleService::new(Arc::new(meerkat::DisabledScheduleStore));
    let following = MobAgentSchedules::new(service, ScheduleMobTargetRegistry::default());

    let released = following.release_mob(&mob(CREATED)).await;

    assert!(
        released.is_err(),
        "a release over a store that answers nothing is an error: {released:?}"
    );
}

/// A pin of what the release does today in a state no composition reaches:
/// the registry adopted no runtime, so the host's default mob is not known.
/// The release ends every schedule that spells the mob's id and names each
/// one it ended. A Console caller's schedule whose send names no mob is for
/// the default mob, which is not known then: it names no mob and is as it
/// was.
#[tokio::test]
async fn a_release_while_the_default_mob_is_not_known_ends_what_spells_the_mob() {
    let (service, following) = schedules_of(ScheduleMobTargetRegistry::default());
    let made = schedules_that_name(&service, "named", CREATED).await;
    let mut expected: Vec<String> = made.iter().map(|(name, _)| name.clone()).collect();
    expected.sort_unstable();
    let without = schedule(&service, "console-without-a-mob", console_target("scout")).await;

    let released = following
        .release_mob(&mob(CREATED))
        .await
        .expect("the release while the default mob is not known");

    assert_eq!(
        released_names(&released),
        expected,
        "the schedules that spell the mob are ended and named"
    );
    for (name, id) in &made {
        assert_eq!(
            state(&service, id).await.0,
            SchedulePhase::Deleted,
            "{name} is ended"
        );
    }
    assert_eq!(
        state(&service, &without).await,
        (SchedulePhase::Active, false),
        "the Console caller's schedule that names no mob is as it was"
    );
}

/// A release asked again ends what an earlier asking left and then answers
/// that nothing is left.
#[tokio::test]
async fn a_release_asked_again_ends_what_is_left_and_then_nothing() {
    let (service, following) = schedules();
    let made = schedules_that_name(&service, "named", CREATED).await;
    // The part an earlier asking did before it stopped.
    let (first_name, first_id) = &made[0];
    service
        .delete(first_id)
        .await
        .expect("the part an earlier asking did");
    let mut left: Vec<String> = made
        .iter()
        .map(|(name, _)| name.clone())
        .filter(|name| name != first_name)
        .collect();
    left.sort_unstable();

    let released = following
        .release_mob(&mob(CREATED))
        .await
        .expect("the release asked again");
    assert_eq!(released_names(&released), left, "what was left is ended");
    let again = following
        .release_mob(&mob(CREATED))
        .await
        .expect("the release asked once more");
    assert!(again.is_empty(), "nothing is left: {again:?}");
}

/// A release that meets a schedule naming a session reads what the stored
/// session names. When it cannot (here the mob state the sessions are read
/// from is gone), it is an error, so that the create it stands in front of
/// is refused, and the schedule is as it was.
#[tokio::test]
async fn a_release_that_cannot_read_a_named_session_is_an_error() {
    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
    let registry = ScheduleMobTargetRegistry::default();
    let dir = tempfile::tempdir().expect("tempdir");
    let mob_state = Arc::new(
        meerkat_mob_mcp::MobMcpState::new(
            Arc::new(meerkat_session::EphemeralSessionService::new(
                meerkat::FactoryAgentBuilder::new(
                    meerkat::AgentFactory::new(dir.path()),
                    meerkat::Config::default(),
                ),
                4,
            )),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("mob state"),
    );
    registry.set_mob_state(Some(Arc::clone(&mob_state)));
    drop(mob_state);
    let following = MobAgentSchedules::new(service.clone(), registry);
    let named = schedule(
        &service,
        "a-session",
        session_target(&meerkat_core::types::SessionId::new()),
    )
    .await;

    let released = following.release_mob(&mob(CREATED)).await;

    assert!(
        released.is_err(),
        "a release that cannot read the session a schedule names is an error: {released:?}"
    );
    assert_eq!(
        state(&service, &named).await,
        (SchedulePhase::Active, false),
        "the schedule is as it was"
    );
}

/// A Console caller's schedule is for an agent of the mob its send names.
/// The release of a mob's schedules ends a Console caller's schedule for an
/// agent of that mob, and names it. It leaves the schedule for the agent of
/// the same name in the mob beside it, and the one whose send names no mob:
/// that one is for the host's default mob, whose schedules are never
/// released.
#[tokio::test]
async fn a_release_ends_a_console_callers_schedule_for_an_agent_of_the_mob() {
    let (service, following) = schedules();
    let of_the_mob = schedule(
        &service,
        "a-console-callers-of-the-mob",
        console_target_in(CREATED, "scout"),
    )
    .await;
    let beside = schedule(
        &service,
        "a-console-callers-beside",
        console_target_in(BESIDE, "scout"),
    )
    .await;
    let of_the_default_mob = schedule(
        &service,
        "a-console-callers-of-the-default-mob",
        console_target("scout"),
    )
    .await;

    let released = following
        .release_mob(&mob(CREATED))
        .await
        .expect("the release of the mob's schedules");

    assert_eq!(
        released_names(&released),
        names(&["a-console-callers-of-the-mob"]),
        "the Console caller's schedule for the mob's agent is ended and named"
    );
    assert_eq!(
        state(&service, &of_the_mob).await.0,
        SchedulePhase::Deleted,
        "the Console caller's schedule for the mob's agent is ended"
    );
    for (whose, id) in [
        ("the mob beside it", &beside),
        ("the default mob", &of_the_default_mob),
    ] {
        assert_eq!(
            state(&service, id).await,
            (SchedulePhase::Active, false),
            "the Console caller's schedule for the agent of {whose} is as it was"
        );
    }
}
