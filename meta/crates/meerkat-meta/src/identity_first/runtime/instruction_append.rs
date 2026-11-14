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
//! platform reserves, keyed by the agent's name and written for that agent
//! and its forks (a member target with `include_forks`: Meerkat's mob
//! selects the agent and every fork of its fork family, and owes a seated
//! fork the entry as it owes the agent). When they differ it writes the
//! record at the slot's next revision, with the whole current text as its
//! body.
//!
//! Delivery is Meerkat's: the record is durable in the mob's journal, the
//! mob records it in the agent's session (store-only while the agent is not
//! loaded, never waking it), and the session applies it at its next run
//! start as one appended System entry. A run in flight is not touched, the
//! first prompt and every earlier entry stay as they are, and a restart
//! neither loses nor repeats it.
//!
//! The comparison runs where a change of the text is observed with the old
//! and the new text in hand, and once per agent in a process:
//!
//! - after the commit of an update of a store-owned agent
//!   ([`IdentityRuntime::after_update`]);
//! - when a registration replaces another with other instructions: a roster
//!   pass that reads an edited configuration, a door or a pass that reads a
//!   changed row again, a spec the runtime's own update replaces
//!   ([`IdentityRuntime::append_changed_instructions_on_registration`]);
//! - when an agent is registered and no registration is replaced. A
//!   configured agent: at its first registration in this process, by
//!   whichever roster pass makes it, or its first since its host's
//!   configuration listed it again. An agent whose record is its row:
//!   before its first delivery or after its first change in this process,
//!   whichever registered it before (a roster pass, a door, Core's build
//!   of its member)
//!   ([`IdentityRuntime::append_owed_instructions_before_delivery`]). A
//!   start reads the rows it serves in pages and compares none of them:
//!   rows can be many, and a start reads no session on behalf of one. A
//!   read compares nothing either. A
//!   text that changed before a stop, or while the gateway was down,
//!   without its record written is recorded then. One read of the
//!   mob's records per process serves all of them
//!   ([`InstructionRecordsAtStart`]): an agent whose record already holds
//!   its text, or whose conversation was built with it, costs no
//!   comparison and no lock.
//!
//! An agent the mob holds no record of takes its first record from an
//! observed change, or, at its first registration, when the sections its
//! conversation's build state records are not the current ones (the
//! session service's `recorded_build_instructions`: committed metadata, no
//! transcript row). A session whose build state does not say (one built or
//! resumed through 0.9.14) gets nothing written then: its text changes with
//! the next observed change or a fresh session, as before. An agent whose
//! session no build created yet needs no record: its first build takes the
//! current text. An agent with a conversation and no seated member
//! (archived, or dropped from its host's configuration) is compared against
//! the session its retirement kept, and its record is delivered when its
//! member is seated again.
//!
//! The removal of an agent revokes its record
//! ([`IdentityRuntime::after_removal`]): the agent has no instructions any
//! more, and a fork that holds the entry is told so by the same revoke. An
//! archive keeps the record. A fresh life (a reset, a delete) keeps it too,
//! brought up to the current text when it holds another one: the agent
//! goes on with the same instructions, and a revoke would take them from
//! every fork that still runs on them. The fresh session's first prompt is
//! built from the current text, and the kept record reaches it as one
//! standing entry with that same text
//! ([`IdentityRuntime::keep_instruction_record_for_fresh_life`]). A removal
//! whose step never ran is completed by the next process: its one read of
//! the mob's records revokes the record of every agent the mob holds as
//! removed.

use std::collections::{BTreeMap, BTreeSet};

use meerkat_core::types::SessionId;
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
use crate::identity_creation::{
    InstructionNextSessionReason, InstructionOutcome, InstructionStep, InstructionsChanged,
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

/// A step's future, heap-placed: the future of a caller that awaits it holds
/// a pointer, not the step's whole state machine. A roster pass and a door
/// await these inside futures that are deep already.
type BoxedStep<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// What the mob held in the platform's namespace when this process first
/// compared an agent no change was observed for: the digest of each active
/// record, by its slot's key, and the agents that read has served. It is
/// read once per process and kept for the process's life as a reason to
/// skip: an agent whose record already held its text needs no comparison,
/// the first time it is registered. It is not a copy of the mob's truth:
/// every write reads the record again from the mob, under the identity's
/// lifecycle lock, and an agent registered a second time is compared with
/// the mob's record as it is then.
#[derive(Default)]
pub(super) struct InstructionRecordsAtStart {
    held: tokio::sync::OnceCell<BTreeMap<InstructionKey, Option<InstructionContentDigest>>>,
    compared: std::sync::Mutex<BTreeSet<AgentIdentity>>,
}

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
    /// No change was observed: `current` is the agent's text now, compared
    /// with the mob's record, else with the sections the agent's
    /// conversation was built with. `locked` says whether the caller holds
    /// the identity's lifecycle lock.
    Reconcile { current: &'a [String], locked: bool },
    /// The agent's fresh life (a reset, a delete) is about to be seated on
    /// `current`. No change was observed and none is looked for: a record
    /// the mob holds is kept, and brought up to `current` when it holds
    /// another text; an agent with no record gets none. The caller holds
    /// the identity's lifecycle lock.
    FreshLife { current: &'a [String] },
}

impl InstructionCheck<'_> {
    fn current(&self) -> &[String] {
        match self {
            Self::Changed { current, .. }
            | Self::Registered { current, .. }
            | Self::Reconcile { current, .. }
            | Self::FreshLife { current } => current,
        }
    }

    /// Whether the caller holds the identity's lifecycle lock.
    fn caller_holds_the_lock(&self) -> bool {
        matches!(
            self,
            Self::Registered { .. } | Self::Reconcile { locked: true, .. } | Self::FreshLife { .. }
        )
    }

    /// Whether the caller observed the text itself change.
    fn text_changed(&self) -> bool {
        match self {
            Self::Changed { previous, current } | Self::Registered { previous, current } => {
                sections(previous) != sections(current)
            }
            Self::Reconcile { .. } | Self::FreshLife { .. } => false,
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

/// The mob's record of `slot`, read from the mob's committed state. The
/// page it asks for lies past the last member, so no member's session is
/// read.
async fn instruction_record(
    mob: &MobHandle,
    slot: &InstructionSlot,
) -> Result<Option<OperatorInstructionRecordStatus>, OperatorInstructionError> {
    mob.operator_instruction_status(OperatorInstructionStatusQuery {
        namespace: Some(slot.namespace.clone()),
        key: Some(slot.key.clone()),
        agent_identity: None,
        profile: None,
        offset: usize::MAX,
        limit: Some(1),
    })
    .await
    .map(|page| page.desired.into_iter().next())
}

/// The session that holds `member`'s conversation, if a build created one:
/// the session its seat is bound to, once a build created it; or, while no
/// member holds the agent (it is archived, or its host's configuration
/// dropped it), the session its retirement archived, which its next seat
/// comes back on. `None` for an agent no build has created a session for:
/// its first build takes the current text. Read from the mob's published
/// state; nothing is woken.
async fn conversation_of(
    mob: &MobHandle,
    member: &meerkat_mob::AgentIdentity,
) -> Option<SessionId> {
    if let Some(session_id) = mob.resolve_bridge_session_id(member).await {
        return (!mob.session_unborn(&session_id)).then_some(session_id);
    }
    match mob.identity_entry(member) {
        Ok(meerkat_mob::IdentityEntryRead::Entry(entry)) => entry.archived_session,
        _ => None,
    }
}

/// Whether `recorded`, the sections a session's build state records, are
/// other sections than `current`: they do not start with them (a build
/// records the spec's sections, then whatever its host's customizer and the
/// agent's memory added).
fn other_sections(recorded: &[String], current: &[String]) -> bool {
    !sections(recorded).starts_with(&sections(current))
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

/// When a recorded change reaches an agent that has a conversation, by how
/// its model takes an appended system instruction: `lowering`, as the
/// agent's session service read it. Only a model read as taking one after
/// conversation rows is answered the next run. A lowering that could not be
/// read (`None`: no session service, a read that failed, a service that
/// cannot tell) is never answered so: it is answered as a model that takes
/// one only ahead of the conversation is, the cautious one of the answers
/// there are. The instruction status read is the authority for whether the
/// change is in effect.
fn applies_with_lowering(lowering: Option<InstructionLowering>) -> InstructionOutcome {
    match lowering {
        Some(InstructionLowering::MidConversation) => InstructionOutcome::AppliesAtNextRun,
        Some(InstructionLowering::Unavailable) => InstructionOutcome::AppliesAtNextSession {
            reason: InstructionNextSessionReason::LoweringUnavailable,
        },
        Some(InstructionLowering::LeadingPrefixOnly) | None => {
            InstructionOutcome::AppliesAfterCompaction
        }
    }
}

impl IdentityRuntime {
    /// The one comparison of an agent's instructions: `identity`'s current
    /// text against the record `mob` holds for it. When they differ, the
    /// record is written at its slot's next revision, under the identity's
    /// lifecycle lock, with the whole current text as its body; equal text
    /// writes nothing.
    ///
    /// A record the mob holds is always kept current. An agent with no
    /// record takes its first one from a change the caller observed, or,
    /// where the caller observed none ([`InstructionCheck::Reconcile`]),
    /// when its session was built with other sections; and only when a
    /// build created its session: before that, its first build takes the
    /// current text.
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
        let _lifecycle_guard = if check.caller_holds_the_lock() {
            None
        } else {
            Some(lifecycle_lock.lock().await)
        };

        // What the mob holds for the agent.
        let record = match instruction_record(mob, &slot).await {
            Ok(record) => record,
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
        // The session that holds the agent's conversation, if a build
        // created one; an agent with no seated member (archived, or dropped
        // from its host's configuration) has the one its retirement kept.
        let conversation = conversation_of(mob, &member).await;
        match held {
            // The mob holds the current text: nothing is written. A caller
            // that observed the change (an update's step, also when a
            // delivery door recorded the change first, and a replay of the
            // step) is still told when it applies.
            Some(held) if held.content_sha256.as_ref() == Some(&digest) => {
                return if matches!(check, InstructionCheck::Changed { .. }) && check.text_changed()
                {
                    self.applies_when(conversation.as_ref()).await
                } else {
                    InstructionOutcome::Unchanged
                };
            }
            // The mob holds another text: the record is brought up to date.
            Some(_) => {}
            // No record yet: the first one comes from a change of an agent
            // that has a conversation. The caller observed the change, or,
            // where it observed none, the conversation was built with other
            // sections than the current ones. The record of an agent with
            // no seated member is delivered when its member is seated.
            None => {
                let changed = match check {
                    InstructionCheck::Changed { .. } | InstructionCheck::Registered { .. } => {
                        check.text_changed()
                    }
                    InstructionCheck::Reconcile { current, .. } => match conversation.as_ref() {
                        Some(session_id) => {
                            self.built_with_other_sections(session_id, identity, current)
                                .await
                        }
                        None => false,
                    },
                    // A fresh life writes no first record: its first
                    // prompt is built from the current text.
                    InstructionCheck::FreshLife { .. } => false,
                };
                if !changed {
                    return InstructionOutcome::Unchanged;
                }
                if conversation.is_none() {
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
            // The record is the agent's and its forks': a live fork of the
            // agent is owed the entry at its next run, and a fork taken
            // later has the record in its snapshot. The mob selects the
            // forks; this file reads no fork.
            target: OperatorInstructionTarget::Member {
                agent_identity: member,
                include_forks: true,
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
        self.applies_when(conversation.as_ref()).await
    }

    /// When a recorded change reaches the agent whose conversation is the
    /// session `conversation`: its model's lowering decides, as the member
    /// session service reads it ([`applies_with_lowering`]). An agent with
    /// no conversation takes the current text with its first build, which
    /// is its next run.
    async fn applies_when(&self, conversation: Option<&SessionId>) -> InstructionOutcome {
        let Some(session_id) = conversation else {
            return InstructionOutcome::AppliesAtNextRun;
        };
        let lowering = match self
            .bridge
            .as_ref()
            .and_then(|bridge| bridge.member_session_service())
        {
            Some(sessions) => sessions
                .read_member_instructions(std::slice::from_ref(session_id))
                .await
                .into_iter()
                .next()
                .and_then(Result::ok)
                .and_then(|read| read.lowering),
            None => None,
        };
        applies_with_lowering(lowering)
    }

    /// Whether the session `session_id`, the conversation of an agent the
    /// mob holds no record of, was built with other sections than `current`
    /// ([`other_sections`]), read from the session's committed build state
    /// (its metadata; no transcript row). A session whose record does not
    /// say, as one built or resumed through 0.9.14 does not, was built with
    /// nobody knows what: it answers `false`, and nothing is written for it
    /// until a change is observed.
    async fn built_with_other_sections(
        &self,
        session_id: &SessionId,
        identity: &AgentIdentity,
        current: &[String],
    ) -> bool {
        let Some(sessions) = self
            .bridge
            .as_ref()
            .and_then(|bridge| bridge.member_session_service())
        else {
            return false;
        };
        match sessions.recorded_build_instructions(session_id).await {
            Ok(Some(recorded)) => other_sections(&recorded, current),
            Ok(None) => false,
            Err(error) => {
                tracing::warn!(
                    %identity,
                    %error,
                    "the sections an agent's session was built with could not be read; its \
                     instructions are compared again at the next start"
                );
                false
            }
        }
    }

    /// After the store commit of an update of a store-owned agent that
    /// changed its instructions: the comparison, with both texts in hand.
    /// A repeat finds the record current and writes nothing.
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
        let Ok(identity) = AgentIdentity::parse(identity) else {
            return;
        };
        let lifecycle_lock = self.lifecycle_lock_for(&identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        self.revoke_record_locked(mob, &identity, "a removed agent")
            .await;
    }

    /// `identity`'s fresh life (a reset, a delete) is about to be seated on
    /// `current`, its instructions now, under the identity's lifecycle
    /// lock. The agent goes on with the same instructions, so its record is
    /// KEPT: the record is the agent's and its forks', and a fork that
    /// keeps its conversation must go on running on the agent's current
    /// text (a revoke would reach every fork and leave it with the earlier
    /// text of the first prompt it copied). A record that holds another
    /// text than `current` (a change nothing recorded before the fresh
    /// life) is first written at its next revision with the current text,
    /// so a kept record never states a superseded text. An agent with no
    /// record, or whose record is revoked, gets none: its first prompt is
    /// built from the current text.
    ///
    /// The fresh conversation is selected by the kept record like any
    /// conversation of the agent: beside its first prompt, which is built
    /// from the current text, it holds that text once more, as the one
    /// standing entry of the record. A later change is the record's next
    /// revision.
    ///
    /// A record that could not be brought up to the current text is
    /// revoked, as a removal revokes it: nothing may present a superseded
    /// text to the fresh conversation as the current one.
    pub(super) fn keep_instruction_record_for_fresh_life<'a>(
        &'a self,
        identity: &'a AgentIdentity,
        current: &'a [String],
    ) -> BoxedStep<'a, ()> {
        Box::pin(async move {
            if self.is_candidate_launch() {
                return;
            }
            let Some(mob) = self.runtime_services.mob_handle() else {
                return;
            };
            let kept = self
                .append_changed_instructions(
                    &mob,
                    identity,
                    InstructionCheck::FreshLife { current },
                )
                .await;
            if !matches!(kept, InstructionOutcome::AppliesAtNextSession { .. }) {
                return;
            }
            // The write was refused, or the record could not be written:
            // whether the record still holds another text is read again.
            let stale = match instruction_slot(identity.as_str()) {
                Some(slot) => instruction_record(&mob, &slot)
                    .await
                    .ok()
                    .flatten()
                    .filter(|record| record.effect == InstructionEffect::Active)
                    .is_some_and(|record| {
                        record.content_sha256
                            != Some(InstructionContentDigest::for_body(&record_body(current)))
                    }),
                None => false,
            };
            if stale {
                self.revoke_record_locked(
                    &mob,
                    identity,
                    "an agent starting a fresh conversation whose record holds an earlier text",
                )
                .await;
            }
        })
    }

    /// Revoke `identity`'s record at its slot's next revision, while the
    /// caller holds the identity's lifecycle lock. An agent with no record,
    /// or whose record is revoked already, is left as it is.
    async fn revoke_record_locked(&self, mob: &MobHandle, identity: &AgentIdentity, whose: &str) {
        let Some(slot) = instruction_slot(identity.as_str()) else {
            return;
        };
        let record = match instruction_record(mob, &slot).await {
            Ok(record) => record,
            Err(error) => {
                tracing::warn!(
                    %identity,
                    %error,
                    whose,
                    "the mob's instruction record could not be read; it is not revoked"
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
                whose,
                "the mob's instruction record was not revoked"
            );
        }
    }

    /// `spec` is about to become `spec.identity`'s registration, under the
    /// identity's lifecycle lock. When it replaces a registration that
    /// holds other instructions, the comparison runs with both texts. When
    /// it replaces none (the agent's first registration in this process,
    /// or its first since its host's configuration listed it again), no
    /// change was observed: the comparison runs only for an agent whose
    /// text may be one the mob has no record of
    /// ([`Self::unrecorded_change`]).
    pub(super) fn append_changed_instructions_on_registration<'a>(
        &'a self,
        spec: &'a DurableAgentSpec,
    ) -> BoxedStep<'a, ()> {
        Box::pin(async move {
            let current = spec.additional_instructions.as_slice();
            let previous = self
                .entries
                .read()
                .await
                .get(&spec.identity)
                .map(|entry| entry.spec.additional_instructions.clone());
            if previous
                .as_ref()
                .is_some_and(|previous| sections(previous) == sections(current))
            {
                return;
            }
            let Some(mob) = self.runtime_services.mob_handle() else {
                return;
            };
            let check = match previous.as_deref() {
                Some(previous) => InstructionCheck::Registered { previous, current },
                None => {
                    if !self.unrecorded_change(&mob, &spec.identity, current).await {
                        return;
                    }
                    InstructionCheck::Reconcile {
                        current,
                        locked: true,
                    }
                }
            };
            self.append_changed_instructions(&mob, &spec.identity, check)
                .await;
        })
    }

    /// Whether this process has not compared `name` yet, so the comparison
    /// at its first registration is still owed. An agent whose record is
    /// its row is registered by a roster pass, a door or a build of its
    /// member without being compared. Derived from the set the comparison
    /// keeps of the agents it has served: nothing is stored for it, and it
    /// reads nothing.
    fn first_registration_comparison_owed(&self, name: &AgentIdentity) -> bool {
        !self
            .instruction_records_at_start
            .compared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(name)
    }

    /// Before a delivery to `name` is submitted, and after a change of
    /// `name` committed: an agent this process has not compared yet is
    /// compared now, once ([`Self::first_registration_comparison_owed`]).
    /// This is where an agent whose record is its row is first compared in
    /// a process: a start registers such rows in pages and reads no session
    /// for any of them, and a build of a member compares nothing. A
    /// configured agent was compared when its roster pass registered it, so
    /// this costs it one lookup. A read is not a use: no read calls this.
    ///
    /// The caller holds no lifecycle lock and is not inside a build of the
    /// agent's member: the comparison takes the identity's lifecycle lock
    /// itself.
    pub(super) fn append_owed_instructions_before_delivery<'a>(
        &'a self,
        name: &'a AgentIdentity,
    ) -> BoxedStep<'a, ()> {
        Box::pin(async move {
            if self.first_registration_comparison_owed(name) {
                self.append_changed_instructions_at_first_registration(name)
                    .await;
            }
        })
    }

    /// The comparison owed to `name` at its first registration in this
    /// process ([`Self::append_owed_instructions_before_delivery`]): the
    /// agent is compared as at any first registration, and the comparison
    /// takes the identity's lifecycle lock itself. The caller holds none.
    fn append_changed_instructions_at_first_registration<'a>(
        &'a self,
        name: &'a AgentIdentity,
    ) -> BoxedStep<'a, ()> {
        Box::pin(async move {
            let Some(current) = self
                .entries
                .read()
                .await
                .get(name)
                .map(|entry| entry.spec.additional_instructions.clone())
            else {
                return;
            };
            let Some(mob) = self.runtime_services.mob_handle() else {
                return;
            };
            if !self.unrecorded_change(&mob, name, &current).await {
                return;
            }
            self.append_changed_instructions(
                &mob,
                name,
                InstructionCheck::Reconcile {
                    current: &current,
                    locked: false,
                },
            )
            .await;
        })
    }

    /// Whether `identity`, for which no change was observed, may hold a
    /// text the mob has no record of: its record, as the mob held it when
    /// this process first looked ([`InstructionRecordsAtStart`]), is active
    /// with another text; or it had no active record and its conversation
    /// was built with other sections than `current`.
    ///
    /// It takes no lifecycle lock, sends the mob no command and copies none
    /// of the mob's state per agent: the records are read once per process,
    /// the agent's session comes from the mob's roster (or, with no seated
    /// member, from the mob's entry of the agent), and its build sections
    /// from the session's committed metadata. `true` is a reason to run the
    /// comparison, which reads the record again under the identity's
    /// lifecycle lock; it is never the base of a write.
    async fn unrecorded_change(
        &self,
        mob: &MobHandle,
        identity: &AgentIdentity,
        current: &[String],
    ) -> bool {
        if self.is_candidate_launch() {
            return false;
        }
        let Some(slot) = instruction_slot(identity.as_str()) else {
            return false;
        };
        // The read at start serves each agent once: an agent registered
        // again later in this process (its host's configuration dropped it
        // and listed it again) is compared with the mob's record as it is
        // now.
        let first = self
            .instruction_records_at_start
            .compared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(identity.clone());
        if !first {
            return true;
        }
        let Some(records) = self.instruction_records_at_start(mob).await else {
            // The records did not read: the comparison reads the agent's.
            return true;
        };
        if let Some(held) = records.get(&slot.key) {
            return held.as_ref()
                != Some(&InstructionContentDigest::for_body(&record_body(current)));
        }
        let member = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let conversation = match mob.resolve_bridge_session_id_observation(&member).await {
            Some(session_id) => Some(session_id),
            None => match mob.identity_entry(&member) {
                Ok(meerkat_mob::IdentityEntryRead::Entry(entry)) => entry.archived_session,
                _ => None,
            },
        };
        match conversation {
            Some(session_id) => {
                self.built_with_other_sections(&session_id, identity, current)
                    .await
            }
            None => false,
        }
    }

    /// The mob's active records of the platform's namespace, as this
    /// process first read them: one read of the mob's records for the
    /// process's life, which reads no session. `None` when the read failed;
    /// the next caller reads again.
    ///
    /// That first read also completes removals: every record whose agent
    /// the mob holds as removed is revoked, as [`Self::after_removal`]
    /// revokes it. So a removal that committed while the step after it
    /// never ran (the process stopped in between; a replayed removal is
    /// answered from the mob's receipt and does not repeat the step) still
    /// ends with its record revoked. An archived agent is not removed: its
    /// record stays. A record revoked already is left as it is, so a later
    /// start writes nothing.
    async fn instruction_records_at_start(
        &self,
        mob: &MobHandle,
    ) -> Option<&BTreeMap<InstructionKey, Option<InstructionContentDigest>>> {
        self.instruction_records_at_start
            .held
            .get_or_try_init(|| async {
                let namespace =
                    InstructionNamespace::new(INSTRUCTIONS_NAMESPACE).map_err(|_| ())?;
                let records = mob
                    .operator_instruction_status(OperatorInstructionStatusQuery {
                        namespace: Some(namespace),
                        key: None,
                        agent_identity: None,
                        profile: None,
                        offset: usize::MAX,
                        limit: Some(1),
                    })
                    .await
                    .map_err(|error| {
                        tracing::warn!(
                            %error,
                            "the mob's instruction records could not be read; the next                              registration reads them again"
                        );
                    })?
                    .desired;
                let mut held = BTreeMap::new();
                for record in records {
                    if record.effect != InstructionEffect::Active {
                        continue;
                    }
                    if let OperatorInstructionTarget::Member {
                        agent_identity: member,
                        ..
                    } = &record.target
                        && matches!(
                            mob.identity_entry(member),
                            Ok(meerkat_mob::IdentityEntryRead::Removed)
                        )
                    {
                        let identity = crate::member_comms_id::durable_identity_for_roster_member(
                            member.as_str(),
                        );
                        self.after_removal(mob, &identity).await;
                        continue;
                    }
                    held.insert(record.slot.key, record.content_sha256);
                }
                Ok::<_, ()>(held)
            })
            .await
            .ok()
    }

    /// This runtime as the instruction step of its own launch
    /// ([`InstructionStep`]): what the launch's executor calls after an
    /// update that changed an agent's instructions and after a removal,
    /// unless the launch gives a step of its own.
    pub(crate) fn instruction_step(
        self: &std::sync::Arc<Self>,
    ) -> std::sync::Arc<dyn InstructionStep> {
        std::sync::Arc::new(OwnInstructionStep(std::sync::Arc::downgrade(self)))
    }
}

/// The step the identity change interface calls after its commits: the
/// comparison after an update, the revoke after a removal.
#[async_trait::async_trait]
impl InstructionStep for IdentityRuntime {
    async fn after_update(
        &self,
        mob: &MobHandle,
        change: &InstructionsChanged,
    ) -> InstructionOutcome {
        IdentityRuntime::after_update(self, mob, change).await
    }

    async fn after_removal(&self, mob: &MobHandle, identity: &str) {
        IdentityRuntime::after_removal(self, mob, identity).await;
    }
}

/// The identity runtime as the instruction step of its own launch's
/// executor. The runtime keeps the family that commits through that
/// executor, so the step holds the runtime weakly. A runtime that is gone
/// has stopped serving its mob: the change is committed, and the next
/// process records it at the agent's first delivery or change.
struct OwnInstructionStep(std::sync::Weak<IdentityRuntime>);

#[async_trait::async_trait]
impl InstructionStep for OwnInstructionStep {
    async fn after_update(
        &self,
        mob: &MobHandle,
        change: &InstructionsChanged,
    ) -> InstructionOutcome {
        match self.0.upgrade() {
            Some(identities) => IdentityRuntime::after_update(&identities, mob, change).await,
            None => InstructionOutcome::AppliesAtNextSession {
                reason: InstructionNextSessionReason::RecordUnavailable,
            },
        }
    }

    async fn after_removal(&self, mob: &MobHandle, identity: &str) {
        if let Some(identities) = self.0.upgrade() {
            IdentityRuntime::after_removal(&identities, mob, identity).await;
        }
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
        assert!(
            !InstructionCheck::Reconcile {
                current: &other,
                locked: false
            }
            .text_changed()
        );
    }

    /// The baseline for an agent with no record: the sections a build
    /// recorded are other sections when they do not start with the current
    /// ones. What a host's customizer or the agent's memory added after
    /// them is no change, and empty sections do not count. The documented
    /// limit follows from it: a text that only drops sections from the end
    /// reads as no change.
    #[test]
    fn recorded_sections_that_start_with_the_current_ones_are_no_change() {
        let recorded = text(&["a", "b", "added by the host"]);
        assert!(!other_sections(&recorded, &text(&["a", "b"])));
        assert!(!other_sections(&recorded, &text(&["a", "", "b"])));
        assert!(other_sections(&recorded, &text(&["a", "c"])));
        assert!(other_sections(&recorded, &text(&["b"])));
        assert!(!other_sections(&recorded, &text(&["a"])));
        assert!(!other_sections(&recorded, &text(&[])));
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

    /// When a recorded change reaches an agent that has a conversation:
    /// only a model read as taking a system instruction after conversation
    /// rows is answered the next run. A lowering that could not be read is
    /// answered as a model that takes one only ahead of the conversation
    /// is, never as the next run.
    #[test]
    fn an_unreadable_lowering_never_answers_the_next_run() {
        assert_eq!(
            applies_with_lowering(Some(InstructionLowering::MidConversation)),
            InstructionOutcome::AppliesAtNextRun
        );
        assert_eq!(
            applies_with_lowering(Some(InstructionLowering::LeadingPrefixOnly)),
            InstructionOutcome::AppliesAfterCompaction
        );
        assert_eq!(
            applies_with_lowering(None),
            InstructionOutcome::AppliesAfterCompaction
        );
        assert_eq!(
            applies_with_lowering(Some(InstructionLowering::Unavailable)),
            InstructionOutcome::AppliesAtNextSession {
                reason: InstructionNextSessionReason::LoweringUnavailable
            }
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
