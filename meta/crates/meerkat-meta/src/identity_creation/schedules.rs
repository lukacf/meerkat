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
//!
//! A schedule is an agent's when it names the agent (a member target, an
//! identity target), names the agent's session, or is a Console caller's
//! schedule for it. No schedule is pointed at an archived agent through
//! Meta's own doors (the agents' schedule tools and the Console refuse a
//! create or an update that would), so nothing targets an archived agent
//! after its archive paused its schedules but a schedule somebody resumed
//! by name.

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

/// The agent's schedules as a change of the agent leaves them, or, in a
/// plan, would leave them: after an archive the schedules that are paused
/// for the agent, after an unarchive the agent's schedules that are active
/// (one somebody resumed by name while the agent was archived included),
/// after a removal the ended ones. A schedule somebody paused by name is
/// on no list of an archive or an unarchive. The list is the schedules as
/// they stand after the change, whichever asking brought each there: a
/// change that is asked again after one that followed only a part answers
/// as a whole one does.
///
/// The list is the host's and an administrator's: a schedule that targets
/// an agent may be anybody's. Every other caller (an agent that asks for
/// the change, a Console caller without `runtime.admin`) is answered
/// `effect` and `count`, and no `schedules` member. The family itself
/// withholds the list only for an agent requester, and Meta's own surfaces
/// withhold it for their other callers: a host that serves any caller
/// other than its operator through `IdentityFamily` withholds the list
/// itself, before it writes the answer for that caller.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SchedulesFollowed {
    /// What was done to them.
    pub effect: ScheduleEffect,
    /// How many.
    pub count: usize,
    /// Which, in the schedule owner's order. Empty, whatever `count` says,
    /// on a value whose list is withheld from its caller.
    pub schedules: Vec<ScheduleRef>,
    /// Whether a schedule could not be read or changed: the list is then
    /// what was done so far, and the same request asked again follows the
    /// rest. Written only when it is so.
    pub incomplete: bool,
    /// Whether the list is withheld from the caller the value is for.
    list_withheld: bool,
}

impl Serialize for SchedulesFollowed {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let members = 2 + usize::from(!self.list_withheld) + usize::from(self.incomplete);
        let mut answer = serializer.serialize_struct("SchedulesFollowed", members)?;
        answer.serialize_field("effect", &self.effect)?;
        answer.serialize_field("count", &self.count)?;
        if !self.list_withheld {
            answer.serialize_field("schedules", &self.schedules)?;
        }
        if self.incomplete {
            answer.serialize_field("incomplete", &true)?;
        }
        answer.end()
    }
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
            list_withheld: false,
        }
    }

    /// The same answer for a caller that is not given the list: what was
    /// done and how many, with no schedule named. It is written without
    /// the `schedules` member.
    #[must_use]
    pub(crate) fn without_list(mut self) -> Self {
        self.schedules = Vec::new();
        self.list_withheld = true;
        self
    }
}

/// The launch's schedules, as the family has them follow an agent. The
/// schedule owner behind it records whose pause a pause is; an
/// implementation keeps nothing of its own.
#[async_trait]
pub trait AgentSchedules: Send + Sync {
    /// Have the schedules that target the agent `agent` of the mob
    /// `mob_id` follow `effect`, and answer the agent's schedules as they
    /// stand after it ([`SchedulesFollowed`]). Each step changes nothing
    /// where it has nothing to do, so the call can be made again, and it
    /// then answers the same. It wakes and builds nothing.
    async fn follow(&self, mob_id: &str, agent: &str, effect: ScheduleEffect) -> SchedulesFollowed;

    /// What [`Self::follow`] would answer for `effect` now. Nothing is
    /// written.
    async fn planned(&self, mob_id: &str, agent: &str, effect: ScheduleEffect)
    -> SchedulesFollowed;
}
