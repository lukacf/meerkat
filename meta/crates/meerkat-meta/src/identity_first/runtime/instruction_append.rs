//! A change of an agent's instructions reaches the agent as one appended
//! system instruction.
//!
//! This file will own the one comparison of an agent's current instructions
//! against what the mob holds for that agent. For now it holds the surface
//! its rows are written against: the outcome of a change, and the two steps
//! an identity change calls after its commit. Neither step does anything
//! yet.

use meerkat_mob::MobHandle;

use super::IdentityRuntime;

pub use interface_stand_in::{
    InstructionNextSessionReason, InstructionOutcome, InstructionsChanged,
};

/// STAND-IN for the identity change interface's types
/// (`identity_creation::{InstructionOutcome, InstructionNextSessionReason}`
/// and `identity_creation::executor::InstructionsChanged`). This module is
/// deleted, and its names imported from there, when that interface lands.
mod interface_stand_in {
    /// What an agent's changed instructions mean for the agent. Closed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum InstructionOutcome {
        /// Recorded: the agent applies the current text at its next run. A
        /// run that starts within moments of the change may still use the
        /// earlier instructions.
        AppliesAtNextRun,
        /// Recorded, and the agent's model takes a system instruction only
        /// ahead of the conversation: the current text applies once a
        /// compaction at a run's first request carries it, or with a fresh
        /// session.
        AppliesAfterCompaction,
        /// The current text applies from the agent's next fresh session,
        /// for `reason`.
        AppliesAtNextSession {
            reason: InstructionNextSessionReason,
        },
        /// The mob already holds the current text for the agent.
        Unchanged,
    }

    /// Why an agent's changed instructions apply only from its next fresh
    /// session. Closed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum InstructionNextSessionReason {
        /// The runtime is governed, and no authorization exists for an
        /// instruction record there.
        PreparedAuthorizationRequired,
        /// The mob holds as many instruction records as it can.
        SlotLimitExceeded,
        /// The mob's instruction bodies hold as many bytes as they can.
        ActiveSetBudgetExceeded,
        /// The agent's model takes no system instruction besides its
        /// prompt.
        LoweringUnavailable,
        /// The text is larger than one instruction record holds.
        BodyTooLarge,
        /// The mob could not take the record now. The change is committed;
        /// the next start writes the record, and the text then reaches the
        /// agent at its next run.
        RecordUnavailable,
    }

    /// An agent's instructions changed: both texts, as lists of sections.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct InstructionsChanged {
        pub identity: String,
        pub previous: Vec<String>,
        pub current: Vec<String>,
    }
}

impl IdentityRuntime {
    /// After the store commit of an update of a store-owned agent that
    /// changed its instructions.
    pub async fn after_update(
        &self,
        _mob: &MobHandle,
        _change: &InstructionsChanged,
    ) -> InstructionOutcome {
        InstructionOutcome::Unchanged
    }

    /// After the committed removal of an agent.
    pub async fn after_removal(&self, _mob: &MobHandle, _identity: &str) {}
}
