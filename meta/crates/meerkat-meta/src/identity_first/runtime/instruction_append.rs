//! A change of an agent's instructions reaches the agent as one appended
//! system instruction. Nothing ever rewrites what a session already holds.
//!
//! An agent's instructions are a field of its spec. A session's first build
//! assembles them into its first prompt, and a resume authors nothing, so a
//! change made afterwards (an update of a store-owned agent's row, an edit
//! of a configured agent's configuration) would otherwise reach the agent
//! only with its next fresh session.
//!
//! This file owns the one comparison that closes that gap,
//! [`IdentityRuntime::append_changed_instructions`]: an agent's current
//! instructions against what the mob holds for that agent, which is one
//! operator instruction record of Meerkat's mob
//! ([`meerkat_mob::MobHandle::set_operator_instruction`]) in a namespace the
//! platform reserves, keyed by the agent's name and targeted at that agent
//! alone. When they differ it writes the record at the slot's next
//! revision, with the whole current text as its body.
//!
//! Delivery is Meerkat's: the record is durable in the mob's journal, the
//! mob records it in the agent's session (store-only while the agent is not
//! loaded, never waking it), and the session applies it at its next run
//! start as one appended System entry. A run in flight is not touched, the
//! first prompt and every earlier entry stay as they are, and a restart
//! neither loses nor repeats it.
//!
//! The comparison runs where a change of the text is observed with the old
//! and the new text in hand, and once at start:
//!
//! - after the commit of an update of a store-owned agent
//!   ([`IdentityRuntime::after_update`]);
//! - when a registration replaces another with other instructions: a roster
//!   pass that reads an edited configuration, a door or a pass that reads a
//!   changed row again
//!   ([`IdentityRuntime::append_changed_instructions_on_registration`]);
//! - at this process's first roster pass, for every agent the mob holds a
//!   record of ([`IdentityRuntime::append_changed_instructions_at_start`]):
//!   a spec change committed before a stop that the record does not hold
//!   yet is written then.
//!
//! An agent the mob holds no record of takes its first record only from an
//! observed change: without one, nothing says its session was built with
//! another text, and nothing is written. An agent whose session no build
//! created yet needs no record: its first build takes the current text.
//!
//! The removal of an agent revokes its record
//! ([`IdentityRuntime::after_removal`]). An archive keeps it.

use std::collections::BTreeSet;

use meerkat_core::{
    InstructionContentDigest, InstructionEffect, InstructionKey, InstructionLowering,
    InstructionNamespace, InstructionRevision, InstructionSlot, InstructionWriteError,
};
use meerkat_mob::{
    MobHandle, OperatorInstructionError, OperatorInstructionRecordStatus,
    OperatorInstructionStatusQuery, OperatorInstructionTarget, OperatorInstructionWrite,
};

use super::super::types::{AgentIdentity, DurableAgentSpec};
use super::IdentityRuntime;

pub use interface_stand_in::{
    InstructionNextSessionReason, InstructionOutcome, InstructionsChanged,
};

/// The instruction namespaces the platform keeps for itself: a host's
/// operator instruction verbs refuse a namespace that starts with this.
pub const RESERVED_INSTRUCTION_NAMESPACE_PREFIX: &str = "meerkat.identity.";
/// The namespace of the records this file writes: one slot per agent, keyed
/// by the agent's name.
pub const INSTRUCTIONS_NAMESPACE: &str = "meerkat.identity.instructions";

/// Whether `namespace` is one the platform keeps for itself.
#[must_use]
pub fn is_reserved_instruction_namespace(namespace: &str) -> bool {
    namespace.starts_with(RESERVED_INSTRUCTION_NAMESPACE_PREFIX)
}

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

/// A step's future, heap-placed: the future of a caller that awaits it holds
/// a pointer, not the step's whole state machine. A roster pass and a door
/// await these inside futures that are deep already.
type BoxedStep<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// What the caller observed of an agent's instructions.
#[derive(Debug, Clone, Copy)]
pub(crate) enum InstructionCheck<'a> {
    /// The text changed from `previous` to `current`. The caller holds no
    /// lifecycle lock.
    Changed {
        previous: &'a [String],
        current: &'a [String],
    },
    /// A registration with `current` replaces one with `previous`. The
    /// caller holds the identity's lifecycle lock.
    Registered {
        previous: &'a [String],
        current: &'a [String],
    },
    /// No change was observed: `current` is the agent's text now. The
    /// caller holds no lifecycle lock.
    Reconcile { current: &'a [String] },
}

impl InstructionCheck<'_> {
    fn current(&self) -> &[String] {
        match self {
            Self::Changed { current, .. }
            | Self::Registered { current, .. }
            | Self::Reconcile { current } => current,
        }
    }

    /// Whether the caller observed the text itself change.
    fn text_changed(&self) -> bool {
        match self {
            Self::Changed { previous, current } | Self::Registered { previous, current } => {
                sections(previous) != sections(current)
            }
            Self::Reconcile { .. } => false,
        }
    }
}

/// The sections of an instructions text that count: the non-empty ones, in
/// order, as a session's build reads them.
fn sections(text: &[String]) -> Vec<&str> {
    text.iter()
        .map(String::as_str)
        .filter(|section| !section.is_empty())
        .collect()
}

/// The preamble of a record that carries a text.
const TEXT_PREAMBLE: &str = "This is the current, complete text of your standing instructions \
for this role. It replaces every earlier version of them in this conversation, including the \
version in your first prompt. The rest of your system prompt is unchanged.";
/// The whole body of a record that carries no text.
const WITHDRAWN_BODY: &str = "Your standing instructions for this role have been withdrawn. \
Every earlier version of them in this conversation, including the version in your first prompt, \
no longer applies. The rest of your system prompt is unchanged.";

/// The body of the record that carries `text`, for a store-owned and a
/// configured agent alike: the fixed preamble, an empty line, and the
/// text's sections in order with an empty line between them. A text with no
/// section is the fixed withdrawal alone. Only the spec's own sections are
/// in it: nothing of the profile's prompt, and nothing a host's customizer
/// adds to a build. The record's digest is this body's.
fn record_body(text: &[String]) -> String {
    let sections = sections(text);
    if sections.is_empty() {
        return WITHDRAWN_BODY.to_string();
    }
    format!("{TEXT_PREAMBLE}\n\n{}", sections.join("\n\n"))
}

/// The slot of `identity`'s record: the platform's namespace, and the
/// agent's name as the key, or the name's digest when Meerkat's identifier
/// bounds refuse the name.
fn instruction_slot(identity: &str) -> Option<InstructionSlot> {
    let namespace = InstructionNamespace::new(INSTRUCTIONS_NAMESPACE).ok()?;
    let key = InstructionKey::new(identity)
        .or_else(|_| InstructionKey::new(InstructionContentDigest::for_body(identity).to_string()))
        .ok()?;
    Some(InstructionSlot { namespace, key })
}

/// The mob's records of `namespace` (one slot's with `key`), read from the
/// mob's committed state. The page it asks for lies past the last member,
/// so no member's session is read.
async fn instruction_records(
    mob: &MobHandle,
    namespace: InstructionNamespace,
    key: Option<InstructionKey>,
) -> Result<Vec<OperatorInstructionRecordStatus>, OperatorInstructionError> {
    mob.operator_instruction_status(OperatorInstructionStatusQuery {
        namespace: Some(namespace),
        key,
        agent_identity: None,
        profile: None,
        offset: usize::MAX,
        limit: Some(1),
    })
    .await
    .map(|page| page.desired)
}

/// What a refused write means for the change.
fn outcome_of_refusal(error: &OperatorInstructionError) -> InstructionOutcome {
    use InstructionNextSessionReason as Reason;
    let reason = match error {
        OperatorInstructionError::Refused(InstructionWriteError::PreparedAuthorizationRequired) => {
            Reason::PreparedAuthorizationRequired
        }
        OperatorInstructionError::Refused(InstructionWriteError::SlotLimitExceeded) => {
            Reason::SlotLimitExceeded
        }
        OperatorInstructionError::Refused(InstructionWriteError::ActiveSetBudgetExceeded {
            ..
        }) => Reason::ActiveSetBudgetExceeded,
        OperatorInstructionError::Refused(InstructionWriteError::BodyTooLarge { .. }) => {
            Reason::BodyTooLarge
        }
        _ => Reason::RecordUnavailable,
    };
    InstructionOutcome::AppliesAtNextSession { reason }
}

impl IdentityRuntime {
    /// The one comparison of an agent's instructions: `identity`'s current
    /// text against the record `mob` holds for it. When they differ, the
    /// record is written at its slot's next revision, under the identity's
    /// lifecycle lock, with the whole current text as its body; equal text
    /// writes nothing.
    ///
    /// A record the mob holds is always kept current. An agent with no
    /// record takes its first one only from a change the caller observed
    /// (see [`InstructionCheck`]), and only when a build created its
    /// session: before that, its first build takes the current text.
    ///
    /// The answer says when the current text reaches the agent. A write the
    /// mob refuses, or cannot take, changes nothing and is answered with
    /// its typed reason; the spec change the caller observed stands.
    pub(crate) async fn append_changed_instructions(
        &self,
        mob: &MobHandle,
        identity: &AgentIdentity,
        check: InstructionCheck<'_>,
    ) -> InstructionOutcome {
        use InstructionNextSessionReason as Reason;
        // A launch that is not the mob's authoritative one writes nothing.
        if self.is_candidate_launch() {
            return InstructionOutcome::Unchanged;
        }
        let Some(slot) = instruction_slot(identity.as_str()) else {
            return InstructionOutcome::Unchanged;
        };
        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = match check {
            InstructionCheck::Registered { .. } => None,
            InstructionCheck::Changed { .. } | InstructionCheck::Reconcile { .. } => {
                Some(lifecycle_lock.lock().await)
            }
        };

        // What the mob holds for the agent.
        let record = match instruction_records(mob, slot.namespace.clone(), Some(slot.key.clone()))
            .await
        {
            Ok(records) => records.into_iter().next(),
            Err(error) => {
                tracing::warn!(
                    %identity,
                    %error,
                    "the mob's instruction record of an agent could not be read; the next start \
                     compares its instructions again"
                );
                return if check.text_changed() {
                    InstructionOutcome::AppliesAtNextSession {
                        reason: Reason::RecordUnavailable,
                    }
                } else {
                    InstructionOutcome::Unchanged
                };
            }
        };
        let body = record_body(check.current());
        let digest = InstructionContentDigest::for_body(&body);
        let held = record
            .as_ref()
            .filter(|record| record.effect == InstructionEffect::Active);
        let member = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let session_id = mob.resolve_bridge_session_id(&member).await;
        match held {
            // The mob holds the current text.
            Some(held) if held.content_sha256.as_ref() == Some(&digest) => {
                return InstructionOutcome::Unchanged;
            }
            // The mob holds another text: the record is brought up to date.
            Some(_) => {}
            // No record yet: the first one comes from an observed change of
            // an agent whose session a build created.
            None => {
                if !check.text_changed() {
                    return InstructionOutcome::Unchanged;
                }
                let built = session_id
                    .as_ref()
                    .is_some_and(|session_id| !mob.session_unborn(session_id));
                if !built {
                    return InstructionOutcome::AppliesAtNextRun;
                }
            }
        }

        let next = record
            .as_ref()
            .map_or(1, |record| record.revision.get().saturating_add(1));
        let Ok(revision) = InstructionRevision::new(next) else {
            return InstructionOutcome::AppliesAtNextSession {
                reason: Reason::RecordUnavailable,
            };
        };
        let write = OperatorInstructionWrite {
            slot: slot.clone(),
            revision,
            body,
            content_sha256: Some(digest),
            target: OperatorInstructionTarget::Member {
                agent_identity: member,
            },
        };
        match mob.set_operator_instruction(write).await {
            Ok(receipt) => {
                tracing::info!(
                    %identity,
                    key = %slot.key,
                    revision = %revision,
                    disposition = ?receipt.disposition,
                    generation = receipt.generation,
                    "an agent's changed instructions were recorded for its next run"
                );
            }
            Err(error) => {
                tracing::warn!(
                    %identity,
                    key = %slot.key,
                    revision = %revision,
                    code = ?error.code(),
                    %error,
                    "an agent's changed instructions were not recorded; they apply from its \
                     next fresh session"
                );
                return outcome_of_refusal(&error);
            }
        }

        // Recorded. When it applies is the agent's model's to say: one that
        // takes a system instruction after conversation rows applies it at
        // its next run start.
        let lowering = match (self.bridge.as_ref(), session_id.as_ref()) {
            (Some(bridge), Some(session_id)) if !mob.session_unborn(session_id) => {
                match bridge.member_session_service() {
                    Some(sessions) => sessions
                        .read_member_instructions(std::slice::from_ref(session_id))
                        .await
                        .into_iter()
                        .next()
                        .and_then(Result::ok)
                        .and_then(|read| read.lowering),
                    None => None,
                }
            }
            _ => None,
        };
        match lowering {
            Some(InstructionLowering::LeadingPrefixOnly) => {
                InstructionOutcome::AppliesAfterCompaction
            }
            Some(InstructionLowering::Unavailable) => InstructionOutcome::AppliesAtNextSession {
                reason: Reason::LoweringUnavailable,
            },
            Some(InstructionLowering::MidConversation) | None => {
                InstructionOutcome::AppliesAtNextRun
            }
        }
    }

    /// After the store commit of an update of a store-owned agent that
    /// changed its instructions, and on a replay of that update: the
    /// comparison, with both texts in hand. A repeat finds the record
    /// current and writes nothing.
    pub async fn after_update(
        &self,
        mob: &MobHandle,
        change: &InstructionsChanged,
    ) -> InstructionOutcome {
        let Ok(identity) = AgentIdentity::parse(&change.identity) else {
            return InstructionOutcome::Unchanged;
        };
        let comparison: BoxedStep<'_, InstructionOutcome> =
            Box::pin(self.append_changed_instructions(
                mob,
                &identity,
                InstructionCheck::Changed {
                    previous: &change.previous,
                    current: &change.current,
                },
            ));
        comparison.await
    }

    /// After the committed removal of an agent, and on a replay of it: the
    /// agent's record is revoked at its slot's next revision. An agent with
    /// no record, or whose record is revoked already, is left as it is. An
    /// archive keeps the record: the agent comes back on its session, with
    /// its instructions as they were.
    pub async fn after_removal(&self, mob: &MobHandle, identity: &str) {
        if self.is_candidate_launch() {
            return;
        }
        let (Some(slot), Ok(identity)) =
            (instruction_slot(identity), AgentIdentity::parse(identity))
        else {
            return;
        };
        let lifecycle_lock = self.lifecycle_lock_for(&identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        let record =
            match instruction_records(mob, slot.namespace.clone(), Some(slot.key.clone())).await {
                Ok(records) => records.into_iter().next(),
                Err(error) => {
                    tracing::warn!(
                        %identity,
                        %error,
                        "the mob's instruction record of a removed agent could not be read; it is \
                         not revoked"
                    );
                    return;
                }
            };
        let Some(record) = record.filter(|record| record.effect == InstructionEffect::Active)
        else {
            return;
        };
        let Ok(revision) = InstructionRevision::new(record.revision.get().saturating_add(1)) else {
            return;
        };
        if let Err(error) = mob
            .revoke_operator_instruction(slot.clone(), revision)
            .await
        {
            tracing::warn!(
                %identity,
                key = %slot.key,
                revision = %revision,
                code = ?error.code(),
                %error,
                "the mob's instruction record of a removed agent was not revoked"
            );
        }
    }

    /// A registration of `spec` is about to replace `spec.identity`'s
    /// registration, under the identity's lifecycle lock: when the one it
    /// replaces holds other instructions, the comparison runs with both
    /// texts. A first registration observes no change.
    pub(super) fn append_changed_instructions_on_registration<'a>(
        &'a self,
        spec: &'a DurableAgentSpec,
    ) -> BoxedStep<'a, ()> {
        Box::pin(async move {
            let Some(previous) = self
                .entries
                .read()
                .await
                .get(&spec.identity)
                .map(|entry| entry.spec.additional_instructions.clone())
            else {
                return;
            };
            if sections(&previous) == sections(&spec.additional_instructions) {
                return;
            }
            let Some(mob) = self.runtime_services.mob_handle() else {
                return;
            };
            self.append_changed_instructions(
                &mob,
                &spec.identity,
                InstructionCheck::Registered {
                    previous: &previous,
                    current: &spec.additional_instructions,
                },
            )
            .await;
        })
    }

    /// The start check, after this process's first roster pass
    /// (`generation` 1): every registered agent the mob holds a record of
    /// is compared, so a text that changed before the stop without its
    /// record written is recorded now. One read of the mob's records, and
    /// no session is read.
    pub(crate) fn append_changed_instructions_at_start(
        &self,
        generation: u64,
    ) -> BoxedStep<'_, ()> {
        Box::pin(async move {
            if generation != 1 || self.is_candidate_launch() {
                return;
            }
            let Some(mob) = self.runtime_services.mob_handle() else {
                return;
            };
            let Ok(namespace) = InstructionNamespace::new(INSTRUCTIONS_NAMESPACE) else {
                return;
            };
            let recorded = match instruction_records(&mob, namespace, None).await {
                Ok(records) => records
                    .into_iter()
                    .map(|record| record.slot.key)
                    .collect::<BTreeSet<_>>(),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "the mob's instruction records could not be read at start; an agent's next \
                         observed change of instructions compares them"
                    );
                    return;
                }
            };
            if recorded.is_empty() {
                return;
            }
            let registered = self
                .entries
                .read()
                .await
                .iter()
                .map(|(identity, entry)| {
                    (identity.clone(), entry.spec.additional_instructions.clone())
                })
                .collect::<Vec<_>>();
            for (identity, current) in registered {
                let holds_record = instruction_slot(identity.as_str())
                    .is_some_and(|slot| recorded.contains(&slot.key));
                if !holds_record {
                    continue;
                }
                self.append_changed_instructions(
                    &mob,
                    &identity,
                    InstructionCheck::Reconcile { current: &current },
                )
                .await;
            }
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn text(sections: &[&str]) -> Vec<String> {
        sections.iter().map(ToString::to_string).collect()
    }

    /// The two texts a model reads, byte for byte, and the body's form: the
    /// preamble, an empty line and the sections in order, an empty line
    /// between them; empty sections do not count. A change of any of these
    /// bytes changes the digest of every record, so each agent that has a
    /// record gets it rewritten once (a new revision and one more entry in
    /// its conversation) at the first start of the changed build: change
    /// them knowingly.
    #[test]
    fn the_record_body_is_the_fixed_preamble_and_the_sections_in_order() {
        assert_eq!(
            record_body(&text(&["Answer briefly.", "", "Cite sources."])),
            "This is the current, complete text of your standing instructions for this role. \
             It replaces every earlier version of them in this conversation, including the \
             version in your first prompt. The rest of your system prompt is unchanged.\n\n\
             Answer briefly.\n\nCite sources."
        );
        assert_eq!(
            record_body(&text(&["", ""])),
            "Your standing instructions for this role have been withdrawn. Every earlier \
             version of them in this conversation, including the version in your first prompt, \
             no longer applies. The rest of your system prompt is unchanged."
        );
        assert_eq!(record_body(&text(&[])), record_body(&text(&[""])));
    }

    /// The slot is the platform's namespace and the agent's name; a name
    /// Meerkat's identifier bounds refuse is keyed by its digest.
    #[test]
    fn the_slot_is_keyed_by_the_agents_name_or_its_digest() {
        let slot = instruction_slot("keeper").expect("a slot");
        assert_eq!(slot.namespace.as_str(), "meerkat.identity.instructions");
        assert_eq!(slot.key.as_str(), "keeper");
        assert!(is_reserved_instruction_namespace(slot.namespace.as_str()));

        let long = "n".repeat(200);
        let slot = instruction_slot(&long).expect("a slot for a long name");
        assert_eq!(
            slot.key.as_str(),
            InstructionContentDigest::for_body(&long).as_str()
        );
        assert_eq!(slot.key.as_str().len(), "sha256:".len() + 64);
    }

    /// Only the platform's prefix is reserved.
    #[test]
    fn only_the_platforms_prefix_is_reserved() {
        assert!(is_reserved_instruction_namespace(
            "meerkat.identity.instructions"
        ));
        assert!(is_reserved_instruction_namespace(
            "meerkat.identity.anything"
        ));
        assert!(!is_reserved_instruction_namespace("meerkat.identity"));
        assert!(!is_reserved_instruction_namespace("app.example"));
    }

    /// A change is one of the sections that count: an added empty section
    /// is none.
    #[test]
    fn a_change_is_a_change_of_the_sections_that_count() {
        let (one, padded, other) = (text(&["a"]), text(&["a", ""]), text(&["b"]));
        assert!(
            !InstructionCheck::Changed {
                previous: &one,
                current: &padded
            }
            .text_changed()
        );
        assert!(
            InstructionCheck::Registered {
                previous: &one,
                current: &other
            }
            .text_changed()
        );
        assert!(!InstructionCheck::Reconcile { current: &other }.text_changed());
    }

    /// Each refusal of the mob has its own reason; anything else is the
    /// record not being available now.
    #[test]
    fn a_refused_write_keeps_its_typed_reason() {
        use InstructionNextSessionReason as Reason;
        let refused = |error| outcome_of_refusal(&OperatorInstructionError::Refused(error));
        let at_next_session = |reason| InstructionOutcome::AppliesAtNextSession { reason };
        assert_eq!(
            refused(InstructionWriteError::PreparedAuthorizationRequired),
            at_next_session(Reason::PreparedAuthorizationRequired)
        );
        assert_eq!(
            refused(InstructionWriteError::SlotLimitExceeded),
            at_next_session(Reason::SlotLimitExceeded)
        );
        assert_eq!(
            refused(InstructionWriteError::ActiveSetBudgetExceeded { max_bytes: 1 }),
            at_next_session(Reason::ActiveSetBudgetExceeded)
        );
        assert_eq!(
            refused(InstructionWriteError::BodyTooLarge { max_bytes: 1 }),
            at_next_session(Reason::BodyTooLarge)
        );
        assert_eq!(
            outcome_of_refusal(&OperatorInstructionError::MobTerminal),
            at_next_session(Reason::RecordUnavailable)
        );
    }

    /// The outcome's wire form: a `kind`, and the reason beside it.
    #[test]
    fn the_outcome_serializes_as_a_kind_and_a_reason() {
        let json = |outcome| serde_json::to_value::<InstructionOutcome>(outcome).unwrap();
        assert_eq!(
            json(InstructionOutcome::AppliesAtNextRun),
            serde_json::json!({"kind": "applies_at_next_run"})
        );
        assert_eq!(
            json(InstructionOutcome::AppliesAfterCompaction),
            serde_json::json!({"kind": "applies_after_compaction"})
        );
        assert_eq!(
            json(InstructionOutcome::Unchanged),
            serde_json::json!({"kind": "unchanged"})
        );
        assert_eq!(
            json(InstructionOutcome::AppliesAtNextSession {
                reason: InstructionNextSessionReason::SlotLimitExceeded
            }),
            serde_json::json!({"kind": "applies_at_next_session", "reason": "slot_limit_exceeded"})
        );
    }
}
