//! The schedules that target an agent follow its archive, its unarchive and
//! its removal.
//!
//! A schedule that targets an archived agent would end each fire as a final
//! delivery failure, and one that targets a removed agent as a missing
//! target, for as long as the schedule exists. So a change that stops an
//! agent takes its schedules with it:
//!
//! - an archive pauses the Active schedules that target the agent, for
//!   their target;
//! - an unarchive resumes exactly the schedules that are paused for their
//!   target, and leaves a schedule somebody paused by name as it is;
//! - a removal ends every schedule that targets the agent.
//!
//! Whose pause a schedule's pause is, the schedule's own lifecycle records
//! (`ScheduleService::pause_for_target`): nothing here remembers it. The
//! family asks the launch's schedule owner through [`AgentSchedules`] after
//! the change committed, and says what it did in the change's answer
//! ([`SchedulesFollowed`]); a plan says what a change would do. A launch
//! that gives the family no schedules ([`super::IdentityFamilyOptions`])
//! changes none, and its answers carry no such member.
//!
//! Asked again under the same request, a change follows its schedules
//! again: each step changes nothing where it has nothing to do, so a
//! caller that got no answer asks again and the schedules converge. A
//! pause or a resume somebody asks for by name between the two takes the
//! schedule over, and the second asking follows the target again.

use async_trait::async_trait;
use serde::Serialize;

use super::request::RequestAction;

/// What a change of an agent does to the schedules that target it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScheduleEffect {
    /// An archive: the Active schedules are paused for their target.
    Paused,
    /// An unarchive: the schedules paused for their target are resumed.
    Resumed,
    /// A removal: every schedule is ended.
    Ended,
}

impl ScheduleEffect {
    /// What the change `action` does to the schedules that target its
    /// agent; `None` for a change that leaves them alone (a create, an
    /// update, an adoption).
    #[must_use]
    pub fn of_action(action: &RequestAction) -> Option<(Self, &str)> {
        match action {
            RequestAction::Archive { identity, .. } => Some((Self::Paused, identity)),
            RequestAction::Unarchive { identity, .. } => Some((Self::Resumed, identity)),
            RequestAction::Remove { identity, .. } => Some((Self::Ended, identity)),
            _ => None,
        }
    }
}

/// One schedule, as an answer names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ScheduleRef {
    /// Its id.
    pub schedule_id: String,
    /// Its name, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ScheduleRef {
    /// The schedule `schedule_id`, named `name` when it has a name.
    pub fn new(schedule_id: impl Into<String>, name: Option<String>) -> Self {
        Self {
            schedule_id: schedule_id.into(),
            name,
        }
    }
}

/// The schedules a change of an agent took with it, or, in a plan, would
/// take with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct SchedulesFollowed {
    /// What was done to them.
    pub effect: ScheduleEffect,
    /// How many.
    pub count: usize,
    /// Which, in the schedule owner's order.
    pub schedules: Vec<ScheduleRef>,
    /// Whether a schedule could not be read or changed: the list is then
    /// what was done so far, and the same request asked again follows the
    /// rest. Written only when it is so.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub incomplete: bool,
}

impl SchedulesFollowed {
    /// `schedules`, to which `effect` was done, all of them when not
    /// `incomplete`.
    pub fn new(effect: ScheduleEffect, schedules: Vec<ScheduleRef>, incomplete: bool) -> Self {
        Self {
            effect,
            count: schedules.len(),
            schedules,
            incomplete,
        }
    }
}

/// The launch's schedules, as the family has them follow an agent. The
/// schedule owner behind it records whose pause a pause is; an
/// implementation keeps nothing of its own.
#[async_trait]
pub trait AgentSchedules: Send + Sync {
    /// Have the schedules that target the agent `agent` of the mob
    /// `mob_id` follow `effect`, and answer the ones it was done to. Each
    /// step changes nothing where it has nothing to do, so the call can be
    /// made again. It wakes and builds nothing.
    async fn follow(&self, mob_id: &str, agent: &str, effect: ScheduleEffect) -> SchedulesFollowed;

    /// The schedules [`Self::follow`] would do `effect` to now. Nothing is
    /// written.
    async fn planned(&self, mob_id: &str, agent: &str, effect: ScheduleEffect)
    -> SchedulesFollowed;
}
