//! The typed outcome of an identity change: the same on every surface.
//!
//! - `done {operation_id, agents}`: the change committed;
//! - `pending_decision {operation_id, decision_id, deadline_ms}`: nothing is
//!   written but the decision; the same operation returns the same state;
//! - `pending_preparation {operation_id, agents}`: committed, not yet
//!   usable;
//! - `refused {kind, field?, retryable, detail}`: one kind of the closed
//!   vocabulary ([`IdentityRefusal`]), written so a caller can act.
//!
//! An agent's entry says what the change did to it, its spec digest and
//! revision afterwards, for a create whether its seat is owed
//! ([`AgentSeat`]) and, for an update, what became of an instructions change
//! ([`InstructionOutcome`]): an instructions change reaches the agent at its
//! next run; a run that starts within moments of the change may still use
//! the earlier instructions, and earlier instructions are never rewritten.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::digest::Sha256Digest;
use super::operation::OperationId;
use super::refusal::{IdentityRefusal, IdentityRefusalKind};
use super::schedules::SchedulesFollowed;

/// The typed outcome of an identity change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityOutcome {
    /// The change committed.
    Done {
        /// The operation.
        operation_id: OperationId,
        /// The agents it changed, in the order the request names them.
        agents: Vec<AgentOutcome>,
    },
    /// A decision is held for the change: nothing else is written, the same
    /// operation returns this state again, and a keyed notice follows the
    /// decision.
    PendingDecision {
        /// The operation.
        operation_id: OperationId,
        /// The held decision.
        decision_id: String,
        /// When the decision expires, in UNIX milliseconds.
        deadline_ms: u64,
    },
    /// The change committed, and at least one of its agents is not usable
    /// yet: something is still being prepared for it.
    PendingPreparation {
        /// The operation.
        operation_id: OperationId,
        /// The agents it changed.
        agents: Vec<AgentOutcome>,
    },
    /// The change was refused: nothing of it was written.
    Refused(IdentityRefusal),
}

impl IdentityOutcome {
    /// A refusal of `kind`, of the request as a whole.
    pub fn refused(kind: IdentityRefusalKind) -> Self {
        Self::Refused(IdentityRefusal::new(kind))
    }

    /// The outcome's stable token: `done`, `pending_decision`,
    /// `pending_preparation` or `refused`.
    pub fn token(&self) -> &'static str {
        self.listed_token()
    }

    /// The operation, for every outcome but a refusal.
    pub fn operation_id(&self) -> Option<&OperationId> {
        match self {
            Self::Done { operation_id, .. }
            | Self::PendingDecision { operation_id, .. }
            | Self::PendingPreparation { operation_id, .. } => Some(operation_id),
            Self::Refused(_) => None,
        }
    }

    /// The agents a committed change changed; none for a held decision and
    /// a refusal.
    pub fn agents(&self) -> &[AgentOutcome] {
        match self {
            Self::Done { agents, .. } | Self::PendingPreparation { agents, .. } => agents,
            Self::PendingDecision { .. } | Self::Refused(_) => &[],
        }
    }

    /// The refusal, for a refused change.
    pub fn refusal(&self) -> Option<&IdentityRefusal> {
        match self {
            Self::Refused(refusal) => Some(refusal),
            _ => None,
        }
    }

    /// Record what became of the instructions change of the committed
    /// update of the agent `name`: what the instruction step answers after
    /// the commit ([`super::InstructionStep::after_update`]). An outcome
    /// that names no such agent is unchanged.
    ///
    /// [`InstructionsChanged`] is what the step is asked with.
    #[must_use]
    pub fn with_instruction_outcome(mut self, name: &str, outcome: InstructionOutcome) -> Self {
        if let Self::Done { agents, .. } | Self::PendingPreparation { agents, .. } = &mut self
            && let Some(agent) = agents.iter_mut().find(|agent| agent.name == name)
        {
            agent.instructions = Some(outcome);
        }
        self
    }

    /// Record what became of the schedules that target the agent `name`
    /// of a committed archive, unarchive or removal: what the launch's
    /// schedules answer after the commit
    /// ([`super::AgentSchedules::follow`]). An outcome that names no such
    /// agent is unchanged.
    #[must_use]
    pub fn with_schedules(mut self, name: &str, followed: SchedulesFollowed) -> Self {
        if let Self::Done { agents, .. } | Self::PendingPreparation { agents, .. } = &mut self
            && let Some(agent) = agents.iter_mut().find(|agent| agent.name == name)
        {
            agent.schedules = Some(followed);
        }
        self
    }

    /// The outcome for a caller that is told what became of each agent's
    /// schedules and how many they are, and is not given their list
    /// ([`SchedulesFollowed::without_list`]).
    #[must_use]
    pub(crate) fn without_schedule_lists(mut self) -> Self {
        if let Self::Done { agents, .. } | Self::PendingPreparation { agents, .. } = &mut self {
            for agent in agents {
                agent.schedules = agent.schedules.take().map(SchedulesFollowed::without_list);
            }
        }
        self
    }

    /// The outcome as every surface writes it: `outcome` is its token, and
    /// a refusal carries `kind`, `field`, `retryable`, `detail` and the
    /// kind's details ([`IdentityRefusal::to_refused`]).
    pub fn to_json(&self) -> Value {
        match self {
            Self::Done {
                operation_id,
                agents,
            }
            | Self::PendingPreparation {
                operation_id,
                agents,
            } => json!({
                "outcome": self.token(),
                "operation_id": operation_id.as_str(),
                "agents": agents,
            }),
            Self::PendingDecision {
                operation_id,
                decision_id,
                deadline_ms,
            } => json!({
                "outcome": self.token(),
                "operation_id": operation_id.as_str(),
                "decision_id": decision_id,
                "deadline_ms": deadline_ms,
            }),
            Self::Refused(refusal) => {
                let mut refused = refusal.to_refused();
                if let Value::Object(map) = &mut refused {
                    map.insert("outcome".to_owned(), Value::from(self.token()));
                }
                refused
            }
        }
    }
}

impl Serialize for IdentityOutcome {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_json().serialize(serializer)
    }
}

/// One agent of a committed change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AgentOutcome {
    /// The agent's name.
    pub name: String,
    /// What the change did to it.
    pub state: AgentChangeState,
    /// Its spec digest after the change: Core's digest of the spec in its
    /// row form, the one spec digest every surface shows, and what its
    /// next change names as the spec it read. Absent for a removed agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_digest: Option<Sha256Digest>,
    /// Its spec revision after the change, from 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_revision: Option<u64>,
    /// The session its member is bound to, as the agent stands when the
    /// outcome is answered: a replay, and a read of an earlier operation,
    /// answer today's session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Whether the agent a create committed is seated, as the agent stands
    /// when the outcome is answered. Present on a create's outcome only,
    /// and absent once the agent is neither seated nor owed its seat (it
    /// was archived or removed since).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seat: Option<AgentSeat>,
    /// What became of an update's instructions change. Present on an
    /// update's outcome only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<InstructionOutcome>,
    /// What became of the schedules that target the agent: paused by its
    /// archive, resumed by its unarchive, ended by its removal. Present on
    /// the first answer of those three changes, on a launch whose family
    /// is given its schedules; a replay states none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedules: Option<SchedulesFollowed>,
}

impl AgentOutcome {
    /// The agent `name`, in `state` after its change.
    pub fn new(name: impl Into<String>, state: AgentChangeState) -> Self {
        Self {
            name: name.into(),
            state,
            spec_digest: None,
            spec_revision: None,
            session_id: None,
            seat: None,
            instructions: None,
            schedules: None,
        }
    }
}

/// What a committed change did to one agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentChangeState {
    /// The change committed: the agent exists with the spec the change
    /// gave it. A new agent is dormant: nothing is loaded and nothing wakes
    /// until its first delivery or its first message.
    Committed,
    /// The change committed and the agent is not usable yet.
    PendingPreparation,
    /// The agent's member is stopping; it is archived when that ends.
    Archiving,
    /// The agent is archived: it keeps its name, its record, its
    /// conversation and its memory, and messages to it are refused.
    Archived,
    /// The agent is back from its archive, on its archived session.
    Restored,
    /// The agent's member is stopping; it is removed when that ends.
    Retiring,
    /// The agent is removed: its name is never available again.
    Removed,
}

impl AgentChangeState {
    /// The stable token (`committed`, `archived`, ...).
    pub fn token(self) -> &'static str {
        self.listed_token()
    }
}

closed_tokens!(AgentChangeState {
    Committed => "committed",
    PendingPreparation => "pending_preparation",
    Archiving => "archiving",
    Archived => "archived",
    Restored => "restored",
    Retiring => "retiring",
    Removed => "removed",
});

/// Whether the agent a create committed is seated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentSeat {
    /// The agent is seated: dormant, or live on its first message.
    Seated,
    /// The agent is committed and its seat is owed: its first message did
    /// not start. It exists, its name is taken and its record is durable;
    /// it is seated without a restart at its next delivery or when the
    /// seat is driven again, and its first message runs once then.
    Owed,
}

impl AgentSeat {
    /// The stable token: `seated` or `owed`.
    pub fn token(self) -> &'static str {
        self.listed_token()
    }
}

closed_tokens!(AgentSeat {
    Seated => "seated",
    Owed => "owed",
});

closed_tokens!(IdentityOutcome {
    Done => "done",
    PendingDecision => "pending_decision",
    PendingPreparation => "pending_preparation",
    Refused => "refused",
});

/// What became of an update's instructions change. The new text is in the
/// agent's record at once, whatever the outcome: it reaches the agent at
/// its next run, a run that starts within moments of the change may still
/// use the earlier instructions, and earlier instructions are never
/// rewritten.
///
/// This is the one definition of the outcome: the field of an update's
/// result is this type and nothing else names its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InstructionOutcome {
    /// The change reaches the agent at its next run.
    AppliesAtNextRun,
    /// The agent's model takes new instructions only at the head of what
    /// it reads: the change reaches the agent after its next compaction.
    AppliesAfterCompaction,
    /// The change does not reach the agent as it runs now, for `reason`: it
    /// applies when the agent next starts afresh.
    AppliesAtNextSession {
        /// Why.
        reason: InstructionNextSessionReason,
    },
    /// The agent holds the same instructions already, or the update did
    /// not change them.
    Unchanged,
}

impl InstructionOutcome {
    /// The stable token: `applies_at_next_run`, `applies_after_compaction`,
    /// `applies_at_next_session` or `unchanged`.
    pub fn token(self) -> &'static str {
        self.listed_token()
    }
}

closed_tokens!(InstructionOutcome {
    AppliesAtNextRun => "applies_at_next_run",
    AppliesAfterCompaction => "applies_after_compaction",
    AppliesAtNextSession => "applies_at_next_session",
    Unchanged => "unchanged",
});

/// Why an instructions change does not reach an agent as it runs now
/// ([`InstructionOutcome::AppliesAtNextSession`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InstructionNextSessionReason {
    /// The mob's runtime is governed: the change needs an authorization
    /// its host prepares.
    PreparedAuthorizationRequired,
    /// The mob holds as many instruction records as it can.
    SlotLimitExceeded,
    /// The mob's instruction records are as large together as they can be.
    ActiveSetBudgetExceeded,
    /// The agent's model cannot be given the change as it runs.
    LoweringUnavailable,
    /// The instructions text is larger than one instruction record holds.
    BodyTooLarge,
    /// The mob could not take the instruction record now: the change is
    /// committed, and the record is written at the mob's next start.
    RecordUnavailable,
}

impl InstructionNextSessionReason {
    /// The stable token (`prepared_authorization_required`, ...).
    pub fn token(self) -> &'static str {
        self.listed_token()
    }
}

closed_tokens!(InstructionNextSessionReason {
    PreparedAuthorizationRequired => "prepared_authorization_required",
    SlotLimitExceeded => "slot_limit_exceeded",
    ActiveSetBudgetExceeded => "active_set_budget_exceeded",
    LoweringUnavailable => "lowering_unavailable",
    BodyTooLarge => "body_too_large",
    RecordUnavailable => "record_unavailable",
});

/// The fact that a committed update changed an agent's instructions text:
/// the agent, the text its record held before the update and the text it
/// holds now, each whole, taken from the commit itself. The instruction
/// step is asked with it ([`super::InstructionStep::after_update`]); it
/// never reaches a surface.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InstructionsChanged {
    /// The agent.
    pub identity: String,
    /// The instructions its record held before the update.
    pub previous: Vec<String>,
    /// The instructions its record holds now.
    pub current: Vec<String>,
}

impl InstructionsChanged {
    /// The change of `identity`'s instructions from `previous` to
    /// `current`; `None` when the two are the same text.
    pub fn between(
        identity: impl Into<String>,
        previous: Vec<String>,
        current: Vec<String>,
    ) -> Option<Self> {
        (previous != current).then(|| Self {
            identity: identity.into(),
            previous,
            current,
        })
    }
}
