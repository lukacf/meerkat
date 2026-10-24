//! The planned context of a prompt-context fork
//! ([`MemberLaunchMode::Fork`](crate::launch::MemberLaunchMode::Fork)).
//!
//! A prompt-context fork reads its source member's history rows and renders
//! them as text into the child's first prompt. What of that history may reach
//! the child is decided here, once, by the rows' types and the host's
//! [`ForkInstructionPolicy`], never by their text:
//!
//! - The source's System rows without an activation identity (its system
//!   prompt, keyed prompt versions and host system-context appends) are never
//!   carried: the child has its own role prompt.
//! - Session-acquired instruction state - instruction activation rows and the
//!   results of the built-in `load_skill` tool - is stripped under
//!   `require_explicit_activation` (the default), refuses the fork under
//!   `refuse`, and is carried under `reactivate_inherited` (activation rows
//!   as each key's effective head only).
//! - Host-injected context rows (typed `injected_context` user rows, the
//!   carrier of per-turn memory recall) are carried only under
//!   `reactivate_inherited`.
//!
//! The rendered rows fit a byte budget, newest first. The block ends with a
//! notice line that says, by name only, what the child was and was not given.

use crate::error::MobError;
use crate::ids::AgentIdentity;
use meerkat_contracts::wire::{
    WireAssistantBlock, WireContentBlock, WireContentInput, WireHistoryRow, WireSessionMessage,
    WireToolResult, WireToolResultContent,
};
use meerkat_core::fork_notice::{
    ForkNoticeKind, ForkNoticeSource, ForkSeed, ForkSeedSourceTools, InstructionKeyRef,
};
use meerkat_core::skills::{LOAD_SKILL_TOOL_NAME, SkillKey, SkillName, SourceUuid};
use meerkat_core::types::{
    CommsNoticeKind, SystemNoticeBlock, SystemNoticeDirection, TranscriptUserRole,
};
use meerkat_core::{
    ForkInstructionPolicy, ForkInstructionRefusal, ForkInstructionRefusalCause,
    InstructionContentDigest, InstructionKey, InstructionNamespace,
};
use std::collections::{BTreeMap, BTreeSet};

/// The most a rendered fork context may take, and the budget for a child
/// whose model the catalog does not know.
pub(crate) const MAX_FORK_CONTEXT_BYTES: usize = 256 * 1024;

const FORK_CONTEXT_END: &str = "[End of forked context]";

/// The byte budget for a child running `model`: half its context window at
/// four bytes per token when the catalog knows it, never more than
/// [`MAX_FORK_CONTEXT_BYTES`].
pub(crate) fn fork_context_budget(model: &str) -> usize {
    meerkat_models::catalog()
        .iter()
        .find(|entry| entry.id == model)
        .and_then(|entry| entry.context_window)
        .map_or(MAX_FORK_CONTEXT_BYTES, |window| {
            usize::try_from(window)
                .unwrap_or(usize::MAX)
                .saturating_mul(2)
                .min(MAX_FORK_CONTEXT_BYTES)
        })
}

/// The skill a `load_skill` call named, from its typed arguments.
#[derive(serde::Deserialize)]
struct LoadSkillArgs {
    source_uuid: String,
    skill_name: String,
}

/// One `load_skill` call: the skill its arguments named, when they parse.
struct LoadSkillCall {
    key: Option<SkillKey>,
}

impl LoadSkillCall {
    fn name(&self) -> String {
        self.key
            .as_ref()
            .map_or_else(|| "unknown skill".to_string(), ToString::to_string)
    }
}

/// The rows a prompt-context fork renders, decided under the host's fork
/// instruction policy and fitted to the budget.
///
/// [`ForkContextPlan::new`] is the only constructor, so nothing renders rows
/// the policy did not decide.
pub(super) struct ForkContextPlan {
    source_member_id: AgentIdentity,
    /// Rendered lines of the carried rows, oldest first.
    lines: Vec<String>,
    carries_session_state: bool,
    system_rows: usize,
    instruction_keys: Vec<(InstructionNamespace, InstructionKey)>,
    /// Skills loaded through `load_skill`, as the notice names them.
    skills: Vec<String>,
    /// The same skills, typed, where their arguments named one.
    skill_keys: Vec<SkillKey>,
    injected_rows: usize,
    omitted_rows: usize,
}

impl ForkContextPlan {
    /// Decide `rows` under `policy` and fit them to `max_bytes`.
    ///
    /// Under [`ForkInstructionPolicy::Refuse`] a fork whose rows hold an
    /// activation row or a `load_skill` result is refused with the typed
    /// refusal a durable fork reports. A fork whose newest carried row alone
    /// does not fit the budget is refused with
    /// [`MobError::ForkContextTooLarge`].
    ///
    /// `source_session` is the session the rows were read from. Only the
    /// activation rows that session wrote itself are its effective
    /// instructions; rows it copied from its own source at a durable fork
    /// are history. When it is not known, every activation row counts.
    pub(super) fn new(
        source_member_id: &AgentIdentity,
        source_session: Option<&meerkat_core::types::SessionId>,
        rows: Vec<WireHistoryRow>,
        policy: ForkInstructionPolicy,
        max_bytes: usize,
    ) -> Result<Self, MobError> {
        let carries_session_state = matches!(policy, ForkInstructionPolicy::ReactivateInherited);
        let load_skill_calls = load_skill_calls(&rows);

        // A fork notice in the source's history belongs to an earlier fork,
        // never to this child: it is not carried. The injected-context row it
        // follows is that fork's planned context, which carries what the
        // notice says it carried, so it is session-acquired state too.
        let mut fork_notice_rows = BTreeSet::new();
        let mut fork_context_rows: BTreeMap<usize, (Vec<InstructionKeyRef>, Vec<SkillKey>)> =
            BTreeMap::new();
        for (index, row) in rows.iter().enumerate() {
            let WireSessionMessage::SystemNotice { blocks, .. } = &row.0 else {
                continue;
            };
            let Some(notice) = blocks.iter().find_map(|block| match block {
                SystemNoticeBlock::ForkBoundary(notice) => Some(notice),
                _ => None,
            }) else {
                continue;
            };
            fork_notice_rows.insert(index);
            // Only a prompt-context fork has a planned row before its
            // notice; the row before a durable fork's notice is ordinary.
            if notice.fork == ForkNoticeKind::PromptContext
                && let Some(previous) = index.checked_sub(1)
                && matches!(
                    &rows[previous].0,
                    WireSessionMessage::User { transcript_role, .. }
                        if *transcript_role == TranscriptUserRole::InjectedContext
                )
            {
                fork_context_rows.insert(
                    previous,
                    (
                        notice.instructions_carried.clone(),
                        notice.skills_carried.clone(),
                    ),
                );
            }
        }

        // Each activation key's effective head: its latest row, unless that
        // row switches the key off (an empty body).
        let empty_body = InstructionContentDigest::for_body("");
        let mut heads: BTreeMap<(InstructionNamespace, InstructionKey), usize> = BTreeMap::new();
        let mut instruction_keys_seen = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            if let WireSessionMessage::System {
                instruction_activation: Some(identity),
                ..
            } = &row.0
            {
                let key = (
                    identity.revision.namespace.clone(),
                    identity.revision.key.clone(),
                );
                if !instruction_keys_seen.contains(&key) {
                    instruction_keys_seen.push(key.clone());
                }
                if source_session.is_none_or(|session| &identity.origin_session_id == session) {
                    heads.insert(key, index);
                }
            }
        }
        let effective_heads: BTreeSet<usize> = heads
            .values()
            .copied()
            .filter(|index| match &rows[*index].0 {
                WireSessionMessage::System {
                    instruction_activation: Some(identity),
                    ..
                } => identity.revision.content_sha256 != empty_body,
                _ => false,
            })
            .collect();
        let mut instruction_keys = instruction_keys_seen
            .iter()
            .filter(|key| {
                heads
                    .get(*key)
                    .is_some_and(|index| effective_heads.contains(index))
            })
            .cloned()
            .collect::<Vec<_>>();

        let mut skills = Vec::new();
        let mut skill_keys: Vec<SkillKey> = Vec::new();
        for (carried_keys, carried_skills) in fork_context_rows.values() {
            for carried in carried_keys {
                let key = (carried.namespace.clone(), carried.key.clone());
                if !instruction_keys_seen.contains(&key) {
                    instruction_keys_seen.push(key.clone());
                }
                if !instruction_keys.contains(&key) {
                    instruction_keys.push(key);
                }
            }
            for skill in carried_skills {
                if !skill_keys.contains(skill) {
                    skill_keys.push(skill.clone());
                    skills.push(skill.to_string());
                }
            }
        }
        let mut unreadable_skill_loads: Vec<String> = Vec::new();
        let mut system_rows = 0usize;
        let mut injected_rows = 0usize;
        let mut row_lines: Vec<Vec<String>> = Vec::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            let lines = match &row.0 {
                WireSessionMessage::System {
                    instruction_activation: Some(_),
                    content,
                    ..
                } => {
                    if carries_session_state && effective_heads.contains(&index) {
                        vec![format!("[system]: {content}")]
                    } else {
                        Vec::new()
                    }
                }
                WireSessionMessage::System { .. } => {
                    system_rows += 1;
                    Vec::new()
                }
                WireSessionMessage::User { content, .. }
                    if fork_context_rows.contains_key(&index) =>
                {
                    if carries_session_state {
                        vec![format!("[user]: {}", content_input_text(content))]
                    } else {
                        Vec::new()
                    }
                }
                WireSessionMessage::User {
                    content,
                    transcript_role,
                    ..
                } if *transcript_role == TranscriptUserRole::InjectedContext => {
                    injected_rows += 1;
                    if carries_session_state {
                        vec![format!("[user]: {}", content_input_text(content))]
                    } else {
                        Vec::new()
                    }
                }
                WireSessionMessage::User { content, .. } => {
                    vec![format!("[user]: {}", content_input_text(content))]
                }
                WireSessionMessage::SystemNotice { .. } if fork_notice_rows.contains(&index) => {
                    Vec::new()
                }
                WireSessionMessage::SystemNotice { body, blocks, .. } => {
                    let mut lines = vec![format!(
                        "[system_notice]: {}",
                        body.clone().unwrap_or_default()
                    )];
                    lines.extend(blocks.iter().filter_map(copied_comms_text));
                    lines
                }
                WireSessionMessage::BlockAssistant { blocks, .. } => {
                    // Both `Text` (display) and `Transcript` (spoken) lanes
                    // project to the rendered text stream; the child sees the
                    // assistant's full visible output regardless of lane.
                    let text: String = blocks
                        .iter()
                        .filter_map(|block| match block {
                            WireAssistantBlock::Text { text, .. }
                            | WireAssistantBlock::Transcript { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![format!("[assistant]: {text}")]
                    }
                }
                WireSessionMessage::ToolResults { results, .. } => results
                    .iter()
                    .filter_map(|result| match load_skill_calls.get(&result.tool_use_id) {
                        // Every load_skill result is skill-bearing, a failed
                        // call or unreadable arguments included: fail closed.
                        Some(call) => {
                            let skill = call.name();
                            if !skills.contains(&skill) {
                                skills.push(skill.clone());
                            }
                            match &call.key {
                                Some(key) if !skill_keys.contains(key) => {
                                    skill_keys.push(key.clone());
                                }
                                Some(_) => {}
                                None => {
                                    if !unreadable_skill_loads.contains(&result.tool_use_id) {
                                        unreadable_skill_loads.push(result.tool_use_id.clone());
                                    }
                                }
                            }
                            if carries_session_state {
                                tool_result_line(result)
                            } else {
                                Some(format!(
                                    "[tool_result({})]: skill {skill} was loaded here; its \
                                     body is not carried into this fork",
                                    result.tool_use_id
                                ))
                            }
                        }
                        None => tool_result_line(result),
                    })
                    .collect(),
            };
            row_lines.push(lines);
        }

        if matches!(policy, ForkInstructionPolicy::Refuse)
            && (!instruction_keys_seen.is_empty() || !skills.is_empty())
        {
            let mut keys = instruction_keys_seen;
            keys.sort();
            skill_keys.sort();
            return Err(MobError::ForkInstructionRefused {
                source_member_id: source_member_id.clone(),
                refusal: ForkInstructionRefusal::new(
                    ForkInstructionRefusalCause::PolicyRefuses,
                    keys,
                )
                .with_skills(skill_keys)
                .with_unreadable_skill_loads(
                    u32::try_from(unreadable_skill_loads.len()).unwrap_or(u32::MAX),
                ),
            });
        }

        let mut plan = Self {
            source_member_id: source_member_id.clone(),
            lines: Vec::new(),
            carries_session_state,
            system_rows,
            instruction_keys,
            skills,
            skill_keys,
            injected_rows,
            omitted_rows: 0,
        };

        // Fit the rows newest first. The header, the notice (with the most an
        // omitted count can add) and the end marker are reserved up front.
        let carried_rows = row_lines.iter().filter(|lines| !lines.is_empty()).count();
        let reserved = plan.header().len()
            + plan.notice_with_omitted(carried_rows).len()
            + FORK_CONTEXT_END.len()
            + 3;
        let mut used = reserved;
        let mut kept: Vec<Vec<String>> = Vec::new();
        let mut omitted = 0usize;
        for lines in row_lines
            .into_iter()
            .rev()
            .filter(|lines| !lines.is_empty())
        {
            let bytes = lines.iter().map(|line| line.len() + 1).sum::<usize>();
            if omitted == 0 && used + bytes <= max_bytes {
                used += bytes;
                kept.push(lines);
            } else {
                if kept.is_empty() {
                    return Err(MobError::ForkContextTooLarge {
                        source_member_id: source_member_id.clone(),
                        max_bytes,
                        required_bytes: reserved + bytes,
                    });
                }
                omitted += 1;
            }
        }
        kept.reverse();
        plan.lines = kept.into_iter().flatten().collect();
        plan.omitted_rows = omitted;
        Ok(plan)
    }

    fn header(&self) -> String {
        format!(
            "[Forked conversation context from member '{}']",
            self.source_member_id
        )
    }

    fn notice_with_omitted(&self, omitted_rows: usize) -> String {
        let carried = |carried: bool| if carried { "carried" } else { "not carried" };
        let mut sentences = vec![format!(
            "You are a fork of member '{}'.",
            self.source_member_id
        )];
        if self.system_rows > 0 {
            sentences.push(
                "The source's system prompt and host system context were not carried.".to_string(),
            );
        }
        if !self.instruction_keys.is_empty() {
            let keys = self
                .instruction_keys
                .iter()
                .map(|(namespace, key)| format!("{namespace}/{key}"))
                .collect::<Vec<_>>()
                .join(", ");
            sentences.push(format!(
                "Activated instructions {}: {keys}.",
                carried(self.carries_session_state)
            ));
        }
        if !self.skills.is_empty() {
            sentences.push(format!(
                "Skills loaded by the source {}: {}.",
                carried(self.carries_session_state),
                self.skills.join(", ")
            ));
        }
        if self.injected_rows > 0 {
            sentences.push(format!(
                "Host-injected context rows {}: {}.",
                carried(self.carries_session_state),
                self.injected_rows
            ));
        }
        if omitted_rows > 0 {
            sentences.push(format!("Earlier rows omitted: {omitted_rows}."));
        }
        format!("[fork_notice]: {}", sentences.join(" "))
    }

    /// Render the planned rows as the fork-context text block. ONE renderer
    /// over the ONE page row vocabulary (`WireSessionMessage`, DEC-P6E-20):
    /// the local fork branch projects its domain page through
    /// `WireMemberHistoryPageBody::try_from_history_page` and the remote
    /// branch decodes the bridge page, so local and remote fork context are
    /// byte-identical by construction.
    pub(super) fn render(&self) -> String {
        let mut lines = Vec::with_capacity(self.lines.len() + 3);
        lines.push(self.header());
        lines.extend(self.lines.iter().cloned());
        lines.push(self.notice_with_omitted(self.omitted_rows));
        lines.push(FORK_CONTEXT_END.to_string());
        lines.join("\n")
    }

    /// The fork seed of a child whose create build records it: the planned
    /// rows without the notice line, which Core turns into the child's typed
    /// fork notice ([`meerkat_core::fork_notice`]) at its first run.
    pub(super) fn seed(
        &self,
        source: ForkNoticeSource,
        source_tools: ForkSeedSourceTools,
    ) -> ForkSeed {
        let mut lines = Vec::with_capacity(self.lines.len() + 2);
        lines.push(self.header());
        lines.extend(self.lines.iter().cloned());
        lines.push(FORK_CONTEXT_END.to_string());
        let source_instructions = self
            .instruction_keys
            .iter()
            .map(|(namespace, key)| InstructionKeyRef::new(namespace.clone(), key.clone()))
            .collect::<Vec<_>>();
        let count = |rows: usize| u32::try_from(rows).unwrap_or(u32::MAX);
        let (carried_skills, removed_skills) = if self.carries_session_state {
            (self.skill_keys.clone(), Vec::new())
        } else {
            (Vec::new(), self.skill_keys.clone())
        };
        let (injected_carried, injected_removed) = if self.carries_session_state {
            (self.injected_rows, 0)
        } else {
            (0, self.injected_rows)
        };
        ForkSeed {
            fork: ForkNoticeKind::PromptContext,
            source,
            planned_context: Some(lines.join("\n")),
            instructions_carried: if self.carries_session_state {
                source_instructions.clone()
            } else {
                Vec::new()
            },
            source_instructions,
            skills_removed: removed_skills,
            skills_carried: carried_skills,
            injected_rows_removed: count(injected_removed),
            injected_rows_carried: count(injected_carried),
            earlier_rows_omitted: count(self.omitted_rows),
            source_tools,
        }
    }
}

/// The `load_skill` calls in `rows`, by tool-use id, with the skill each
/// call's typed arguments named.
fn load_skill_calls(rows: &[WireHistoryRow]) -> BTreeMap<String, LoadSkillCall> {
    let mut calls = BTreeMap::new();
    for row in rows {
        let WireSessionMessage::BlockAssistant { blocks, .. } = &row.0 else {
            continue;
        };
        for block in blocks {
            if let WireAssistantBlock::ToolUse { id, name, args, .. } = block
                && name == LOAD_SKILL_TOOL_NAME
            {
                let key = serde_json::from_str::<LoadSkillArgs>(args.get())
                    .ok()
                    .and_then(|args| {
                        Some(SkillKey::new(
                            SourceUuid::parse(&args.source_uuid).ok()?,
                            SkillName::parse(&args.skill_name).ok()?,
                        ))
                    });
                calls.insert(id.clone(), LoadSkillCall { key });
            }
        }
    }
    calls
}

fn content_input_text(content: &WireContentInput) -> String {
    match content {
        WireContentInput::Text(text) => text.clone(),
        WireContentInput::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                WireContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
    }
}

/// One tool result as a preview line, or nothing for an empty result.
fn tool_result_line(result: &WireToolResult) -> Option<String> {
    let text: String = match &result.content {
        WireToolResultContent::Text(text) => text.clone(),
        WireToolResultContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                WireContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
    };
    if text.is_empty() {
        return None;
    }
    Some(format!(
        "[tool_result({})]: {}",
        result.tool_use_id,
        preview(text)
    ))
}

/// `text` cut to about 200 bytes, as a tool result is shown.
fn preview(text: String) -> String {
    if text.len() > 200 {
        // Find a valid UTF-8 char boundary at or before byte 200
        let end = text
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= 200)
            .last()
            .unwrap_or(0);
        format!("{}...", &text[..end])
    } else {
        text
    }
}

/// What a copied comms block gives the child: whom the source heard from
/// (or wrote to), the text of a peer message, a status and a payload preview
/// when the block has them, and the sender's taint declaration.
///
/// Only a message's content is the sender's own words. The content of a
/// request, a response or a lifecycle notice is the runtime's rendering of
/// the notice for the source's model: the request id, a ready
/// `send_response` call, the uncapped result or a note on whether to answer.
/// Rows written by earlier releases have the same shape, so that content is
/// left out here; the title names the intent or kind, and the payload
/// preview carries the params or the result. The child is not the
/// addressee: no request or delivery id or reply instruction reaches it, so
/// it has no means to answer a peer on its source's behalf.
fn copied_comms_text(block: &SystemNoticeBlock) -> Option<String> {
    let SystemNoticeBlock::Comms {
        kind,
        direction,
        peer,
        sender_taint,
        status,
        payload,
        content,
        ..
    } = block
    else {
        return None;
    };
    let peer = peer.as_ref().map_or_else(
        || "a peer".to_string(),
        |peer| match peer.display_name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => peer.id.to_string(),
        },
    );
    let mut lines = vec![match direction {
        SystemNoticeDirection::Outgoing => format!("to {peer}:"),
        SystemNoticeDirection::Incoming | SystemNoticeDirection::Internal => {
            format!("from {peer}:")
        }
    }];
    if matches!(kind, CommsNoticeKind::Message) {
        lines.extend(
            content
                .iter()
                .map(|block| block.text_projection().into_owned())
                .filter(|text| !text.trim().is_empty()),
        );
    }
    if let Some(status) = status {
        lines.push(format!("status: {status}"));
    }
    if let Some(payload) = payload {
        lines.push(format!("payload: {}", preview(payload.to_string())));
    }
    if matches!(
        sender_taint,
        Some(meerkat_core::comms::SenderContentTaint::Tainted)
    ) {
        lines.push(meerkat_core::interaction::SENDER_TAINT_MARKER.to_string());
    }
    Some(lines.join("\n"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::comms::{PeerId, SenderContentTaint};
    use meerkat_core::interaction::{
        InteractionId, ResponseStatus, format_peer_lifecycle_projection,
        format_peer_message_projection, format_peer_request_projection,
        format_peer_response_projection,
    };
    use meerkat_core::types::{
        CommsNoticeKind, ContentBlock, InstructionActivationId, InstructionActivationIdentity,
        InstructionRevisionId, InstructionRevisionRef, SessionId, SystemNoticeDirection,
        SystemNoticeKind, SystemNoticePeer,
    };
    use serde_json::json;

    const AT: &str = "2026-10-22T00:00:00Z";
    const SKILL_SOURCE: &str = "6f1d2c3b-4a5e-4f60-8a7b-9c0d1e2f3a4b";

    fn system(content: &str) -> WireHistoryRow {
        WireHistoryRow(WireSessionMessage::System {
            content: content.to_string(),
            created_at: AT.to_string(),
            identity: None,
            prompt_version: None,
            instruction_activation: None,
        })
    }

    fn activation(key: &str, activation: &str, body: &str) -> WireHistoryRow {
        let identity = InstructionActivationIdentity {
            activation_id: InstructionActivationId::new(activation).unwrap(),
            revision: InstructionRevisionRef {
                namespace: InstructionNamespace::new("host.standing").unwrap(),
                key: InstructionKey::new(key).unwrap(),
                revision_id: InstructionRevisionId::new(format!("rev-{activation}")).unwrap(),
                content_sha256: InstructionContentDigest::for_body(body),
            },
            supersedes: None,
            origin_session_id: SessionId::new(),
            render_version: 1,
        };
        WireHistoryRow(WireSessionMessage::System {
            content: format!("[meerkat-instruction-activation-v1]\nkey={key}\n\n{body}"),
            created_at: AT.to_string(),
            identity: None,
            prompt_version: None,
            instruction_activation: Some(identity),
        })
    }

    fn user_row(text: &str, transcript_role: TranscriptUserRole) -> WireHistoryRow {
        WireHistoryRow(WireSessionMessage::User {
            content: WireContentInput::Text(text.to_string()),
            transcript_role,
            interaction_id: None,
            run_id: None,
            created_at: AT.to_string(),
        })
    }

    fn user(text: &str) -> WireHistoryRow {
        user_row(text, TranscriptUserRole::Conversational)
    }

    fn assistant(blocks: Vec<WireAssistantBlock>) -> WireHistoryRow {
        WireHistoryRow(WireSessionMessage::BlockAssistant {
            blocks,
            stop_reason: None,
            interaction_id: None,
            run_id: None,
            assistant_message_id: None,
            realtime_origin: None,
            created_at: AT.to_string(),
        })
    }

    fn load_skill_call(id: &str, args: serde_json::Value) -> WireHistoryRow {
        assistant(vec![WireAssistantBlock::ToolUse {
            id: id.to_string(),
            name: LOAD_SKILL_TOOL_NAME.to_string(),
            args: serde_json::value::to_raw_value(&args).unwrap(),
            meta: None,
        }])
    }

    fn tool_result(id: &str, text: &str) -> WireHistoryRow {
        WireHistoryRow(WireSessionMessage::ToolResults {
            results: vec![WireToolResult {
                tool_use_id: id.to_string(),
                content: WireToolResultContent::Text(text.to_string()),
                is_error: false,
                settlement_failures: Vec::new(),
            }],
            created_at: AT.to_string(),
        })
    }

    fn comms_notice(body: &str) -> WireHistoryRow {
        WireHistoryRow(WireSessionMessage::SystemNotice {
            kind: SystemNoticeKind::Comms,
            runtime_origin: None,
            body: Some(body.to_string()),
            blocks: Vec::new(),
            created_at: AT.to_string(),
        })
    }

    /// A comms notice as the source's runtime records an incoming peer
    /// input: the notice's title as its body and the typed block that
    /// carries who sent it and what it said.
    fn peer_notice(title: &str, block: SystemNoticeBlock) -> WireHistoryRow {
        WireHistoryRow(WireSessionMessage::SystemNotice {
            kind: SystemNoticeKind::Comms,
            runtime_origin: None,
            body: Some(title.to_string()),
            blocks: vec![block],
            created_at: AT.to_string(),
        })
    }

    /// The typed block of an incoming peer input as the source's runtime
    /// records it. An envelope without blocks of its own is stored with the
    /// runtime's rendered admission text as its content (the comms bridge's
    /// `peer_rendered_body`), so `admitted_text` is that text. Only a message
    /// names its delivery.
    fn incoming_peer_block(
        kind: CommsNoticeKind,
        peer_id: PeerId,
        admitted_text: String,
    ) -> SystemNoticeBlock {
        let delivery_id = matches!(kind, CommsNoticeKind::Message).then(InteractionId::new);
        SystemNoticeBlock::Comms {
            kind,
            direction: SystemNoticeDirection::Incoming,
            peer: Some(SystemNoticePeer {
                id: peer_id,
                display_name: Some("reviewer".to_string()),
            }),
            sender_taint: None,
            request_id: None,
            intent: None,
            status: None,
            summary: None,
            payload: None,
            content: vec![ContentBlock::Text {
                text: admitted_text,
            }],
            delivery_id,
            in_reply_to: None,
        }
    }

    fn plan(
        rows: Vec<WireHistoryRow>,
        policy: ForkInstructionPolicy,
    ) -> Result<ForkContextPlan, MobError> {
        ForkContextPlan::new(
            &AgentIdentity::from("source"),
            None,
            rows,
            policy,
            MAX_FORK_CONTEXT_BYTES,
        )
    }

    fn rendered(rows: Vec<WireHistoryRow>, policy: ForkInstructionPolicy) -> String {
        plan(rows, policy).expect("the fork is planned").render()
    }

    const POLICIES: [ForkInstructionPolicy; 3] = [
        ForkInstructionPolicy::RequireExplicitActivation,
        ForkInstructionPolicy::Refuse,
        ForkInstructionPolicy::ReactivateInherited,
    ];

    #[test]
    fn the_source_system_prompt_and_host_context_are_never_carried() {
        for policy in POLICIES {
            let text = rendered(
                vec![system("ROLE-PROMPT"), system("HOST-APPEND"), user("hello")],
                policy,
            );
            assert!(
                !text.contains("ROLE-PROMPT") && !text.contains("HOST-APPEND"),
                "{text}"
            );
            assert!(text.contains("[user]: hello"), "{text}");
            assert!(
                text.contains("system prompt and host system context were not carried"),
                "{text}"
            );
        }
    }

    #[test]
    fn activated_instructions_are_stripped_by_default_and_named() {
        let text = rendered(
            vec![
                system("ROLE-PROMPT"),
                activation("member", "a1", "BODY-1"),
                user("hi"),
            ],
            ForkInstructionPolicy::RequireExplicitActivation,
        );
        assert!(!text.contains("BODY-1"), "{text}");
        assert!(
            text.contains("Activated instructions not carried: host.standing/member."),
            "{text}"
        );
    }

    #[test]
    fn refuse_refuses_a_fork_whose_rows_hold_an_activation_row() {
        let refused = plan(
            vec![activation("member", "a1", "BODY-1"), user("hi")],
            ForkInstructionPolicy::Refuse,
        );
        match refused {
            Err(MobError::ForkInstructionRefused { refusal, .. }) => {
                assert_eq!(refusal.cause, ForkInstructionRefusalCause::PolicyRefuses);
                assert_eq!(refusal.keys.len(), 1);
                assert!(refusal.skills.is_empty());
            }
            other => panic!(
                "expected a typed refusal, got {:?}",
                other.map(|p| p.render())
            ),
        }
    }

    #[test]
    fn reactivate_inherited_carries_each_key_effective_head_only() {
        let text = rendered(
            vec![
                activation("member", "a1", "OLD-BODY"),
                activation("member", "a2", "NEW-BODY"),
                user("hi"),
            ],
            ForkInstructionPolicy::ReactivateInherited,
        );
        assert!(
            text.contains("NEW-BODY") && !text.contains("OLD-BODY"),
            "{text}"
        );
        assert!(
            text.contains("Activated instructions carried: host.standing/member."),
            "{text}"
        );
    }

    #[test]
    fn a_switched_off_key_carries_no_body_at_all() {
        let text = rendered(
            vec![
                activation("member", "a1", "OLD-BODY"),
                activation("member", "a2", ""),
                user("hi"),
            ],
            ForkInstructionPolicy::ReactivateInherited,
        );
        assert!(!text.contains("OLD-BODY"), "{text}");
        assert!(!text.contains("host.standing/member"), "{text}");
    }

    #[test]
    fn a_load_skill_result_follows_the_three_way_policy() {
        let rows = || {
            vec![
                load_skill_call(
                    "call-1",
                    json!({"source_uuid": SKILL_SOURCE, "skill_name": "pdf-processing"}),
                ),
                tool_result("call-1", "{\"body\":\"SKILL-BODY\"}"),
                user("hi"),
            ]
        };
        let stripped = rendered(rows(), ForkInstructionPolicy::RequireExplicitActivation);
        assert!(!stripped.contains("SKILL-BODY"), "{stripped}");
        assert!(
            stripped.contains(&format!(
                "skill {SKILL_SOURCE}/pdf-processing was loaded here"
            )) && stripped.contains(&format!(
                "Skills loaded by the source not carried: {SKILL_SOURCE}/pdf-processing."
            )),
            "{stripped}"
        );
        match plan(rows(), ForkInstructionPolicy::Refuse) {
            Err(MobError::ForkInstructionRefused { refusal, .. }) => {
                assert!(refusal.keys.is_empty());
                assert_eq!(refusal.skills.len(), 1);
                assert_eq!(
                    refusal.skills[0].to_string(),
                    format!("{SKILL_SOURCE}/pdf-processing")
                );
            }
            other => panic!(
                "expected a typed refusal, got {:?}",
                other.map(|p| p.render())
            ),
        }
        let kept = rendered(rows(), ForkInstructionPolicy::ReactivateInherited);
        assert!(kept.contains("SKILL-BODY"), "{kept}");
    }

    #[test]
    fn unreadable_load_skill_arguments_fail_closed() {
        let rows = || {
            vec![
                load_skill_call("call-1", json!({"bogus": 1})),
                tool_result("call-1", "SKILL-BODY"),
            ]
        };
        let stripped = rendered(rows(), ForkInstructionPolicy::RequireExplicitActivation);
        assert!(!stripped.contains("SKILL-BODY"), "{stripped}");
        assert!(stripped.contains("unknown skill"), "{stripped}");
        assert!(matches!(
            plan(rows(), ForkInstructionPolicy::Refuse),
            Err(MobError::ForkInstructionRefused { .. })
        ));
    }

    #[test]
    fn host_injected_context_is_carried_only_under_reactivate_inherited() {
        let rows = || {
            vec![
                user_row("RECALL-ROW", TranscriptUserRole::InjectedContext),
                user("hi"),
            ]
        };
        for policy in [
            ForkInstructionPolicy::RequireExplicitActivation,
            ForkInstructionPolicy::Refuse,
        ] {
            let text = rendered(rows(), policy);
            assert!(!text.contains("RECALL-ROW"), "{text}");
            assert!(
                text.contains("Host-injected context rows not carried: 1."),
                "{text}"
            );
        }
        let kept = rendered(rows(), ForkInstructionPolicy::ReactivateInherited);
        assert!(kept.contains("RECALL-ROW"), "{kept}");
        assert!(
            kept.contains("Host-injected context rows carried: 1."),
            "{kept}"
        );
    }

    #[test]
    fn system_notices_and_conversation_rows_are_kept() {
        let text = rendered(
            vec![
                comms_notice("PEER-SAID-HI"),
                user("hi"),
                assistant(vec![WireAssistantBlock::Text {
                    text: "ANSWER".to_string(),
                    meta: None,
                }]),
            ],
            ForkInstructionPolicy::RequireExplicitActivation,
        );
        assert!(
            text.contains("[system_notice]: PEER-SAID-HI")
                && text.contains("[user]: hi")
                && text.contains("[assistant]: ANSWER"),
            "{text}"
        );
    }

    /// A copied peer message gives the child who sent it and what it said,
    /// not only the notice's title.
    #[test]
    fn a_copied_peer_message_carries_its_sender_and_text() {
        let message = incoming_peer_block(
            CommsNoticeKind::Message,
            PeerId::new(),
            format_peer_message_projection("reviewer", "PEER-ASKED-FOR-THE-NUMBERS"),
        );
        let text = rendered(
            vec![peer_notice("Peer message", message)],
            ForkInstructionPolicy::RequireExplicitActivation,
        );
        assert!(
            text.contains("PEER-ASKED-FOR-THE-NUMBERS") && text.contains("from reviewer:"),
            "the copied peer message names its sender and carries its text: {text}"
        );
    }

    /// A copied peer request carries what was asked and by whom, but not
    /// the source's means to answer it. A live request's content is the
    /// runtime's rendering for the source's model: its id and a ready
    /// `send_response` call. The child is not the addressee, so neither
    /// reaches it.
    #[test]
    fn a_copied_peer_request_carries_no_reply_affordance() {
        let peer_id = PeerId::new();
        let params = json!({"draft": "DRAFT-PARAM"});
        let mut request = incoming_peer_block(
            CommsNoticeKind::Request,
            peer_id,
            format_peer_request_projection(
                peer_id,
                Some("reviewer"),
                "REQUEST-ID-7",
                "review",
                &params,
            ),
        );
        let SystemNoticeBlock::Comms {
            request_id,
            intent,
            payload,
            ..
        } = &mut request
        else {
            unreachable!("the helper builds a comms block");
        };
        *request_id = Some("REQUEST-ID-7".to_string());
        *intent = Some("review".to_string());
        *payload = Some(params);
        let text = rendered(
            vec![peer_notice("Peer request: review", request)],
            ForkInstructionPolicy::RequireExplicitActivation,
        );
        assert!(
            text.contains("Peer request: review")
                && text.contains("from reviewer:")
                && text.contains("DRAFT-PARAM"),
            "the copied request carries what was asked and by whom: {text}"
        );
        assert!(
            !text.contains("REQUEST-ID-7")
                && !text.contains("send_response")
                && !text.contains("in_reply_to")
                && !text.contains("Delivery ID"),
            "the copied request carries no means to answer it: {text}"
        );
    }

    /// A copied peer response carries its sender, its status and a preview
    /// of its result. The request it answers is the source's own, so its id
    /// is not carried, and the result is cut like a tool result.
    #[test]
    fn a_copied_peer_response_carries_no_request_id_and_a_capped_result() {
        let tail = format!("{}RESULT-TAIL", "x".repeat(400));
        let result = json!({"summary": "RESULT-HEAD", "tail": tail});
        let mut response = incoming_peer_block(
            CommsNoticeKind::ResponseTerminal,
            PeerId::new(),
            format_peer_response_projection(
                "reviewer",
                "REQUEST-ID-9",
                ResponseStatus::Completed,
                &result,
            ),
        );
        let SystemNoticeBlock::Comms {
            request_id,
            status,
            payload,
            ..
        } = &mut response
        else {
            unreachable!("the helper builds a comms block");
        };
        *request_id = Some("REQUEST-ID-9".to_string());
        *status = Some("completed".to_string());
        *payload = Some(result);
        let text = rendered(
            vec![peer_notice("Peer response terminal", response)],
            ForkInstructionPolicy::RequireExplicitActivation,
        );
        assert!(
            text.contains("from reviewer:")
                && text.contains("status: completed")
                && text.contains("RESULT-HEAD"),
            "the copied response carries its sender, status and result preview: {text}"
        );
        assert!(
            !text.contains("REQUEST-ID-9") && !text.contains("RESULT-TAIL"),
            "the copied response carries no request id and only a capped result: {text}"
        );
    }

    /// A copied lifecycle notice gives the child its kind (in the title), its
    /// sender and its params, not the runtime's note to the source's model on
    /// whether to answer it.
    #[test]
    fn a_copied_peer_lifecycle_notice_carries_its_params_not_the_runtime_note() {
        let peer_id = PeerId::new();
        let params = json!({"phase": "KICKOFF-PARAM"});
        let mut notice = incoming_peer_block(
            CommsNoticeKind::Lifecycle,
            peer_id,
            format_peer_lifecycle_projection(
                Some(peer_id),
                Some("reviewer"),
                "mob.kickoff_started",
                &params,
            ),
        );
        let SystemNoticeBlock::Comms {
            intent, payload, ..
        } = &mut notice
        else {
            unreachable!("the helper builds a comms block");
        };
        *intent = Some("mob.kickoff_started".to_string());
        *payload = Some(params);
        let text = rendered(
            vec![peer_notice("Peer lifecycle: mob.kickoff_started", notice)],
            ForkInstructionPolicy::RequireExplicitActivation,
        );
        assert!(
            text.contains("Peer lifecycle: mob.kickoff_started")
                && text.contains("from reviewer:")
                && text.contains("KICKOFF-PARAM"),
            "the copied lifecycle notice carries its kind, sender and params: {text}"
        );
        assert!(
            !text.contains("send_response") && !text.contains("Peer lifecycle notice from"),
            "the runtime's note to the source's model is not copied: {text}"
        );
    }

    /// A copied peer message whose sender declared it tainted keeps the
    /// marker, so the child reads it with the same caution.
    #[test]
    fn a_copied_tainted_peer_message_keeps_its_taint_marker() {
        let mut tainted = incoming_peer_block(
            CommsNoticeKind::Message,
            PeerId::new(),
            format_peer_message_projection("reviewer", "PEER-PASTED-A-WEB-PAGE"),
        );
        let SystemNoticeBlock::Comms { sender_taint, .. } = &mut tainted else {
            unreachable!("the helper builds a comms block");
        };
        *sender_taint = Some(SenderContentTaint::Tainted);
        let text = rendered(
            vec![peer_notice("Peer message", tainted)],
            ForkInstructionPolicy::RequireExplicitActivation,
        );
        assert!(
            text.contains("PEER-PASTED-A-WEB-PAGE")
                && text.contains("[sender declared this content tainted]"),
            "the copied tainted message keeps its taint marker: {text}"
        );
    }

    #[test]
    fn the_budget_keeps_the_newest_rows_and_counts_the_rest() {
        let rows = (0..10)
            .map(|index| user(&format!("ROW-{index}-{}", "x".repeat(1_000))))
            .collect::<Vec<_>>();
        let plan = ForkContextPlan::new(
            &AgentIdentity::from("source"),
            None,
            rows,
            ForkInstructionPolicy::RequireExplicitActivation,
            4_000,
        )
        .expect("the newest rows fit");
        let text = plan.render();
        assert!(text.len() <= 4_000, "{} bytes", text.len());
        assert!(
            text.contains("ROW-9-") && !text.contains("ROW-0-"),
            "{text}"
        );
        assert!(text.contains("Earlier rows omitted: "), "{text}");
    }

    #[test]
    fn a_newest_row_larger_than_the_budget_is_refused_typed() {
        let refused = ForkContextPlan::new(
            &AgentIdentity::from("source"),
            None,
            vec![user(&"x".repeat(10_000))],
            ForkInstructionPolicy::RequireExplicitActivation,
            4_000,
        );
        match refused {
            Err(MobError::ForkContextTooLarge {
                max_bytes,
                required_bytes,
                ..
            }) => {
                assert_eq!(max_bytes, 4_000);
                assert!(required_bytes > 10_000);
            }
            other => panic!(
                "expected ForkContextTooLarge, got {:?}",
                other.map(|p| p.render())
            ),
        }
    }

    /// The refusal a `refuse` policy gives for `rows`, as wire JSON and as
    /// its message.
    fn refusal_of(rows: Vec<WireHistoryRow>) -> (serde_json::Value, String) {
        match plan(rows, ForkInstructionPolicy::Refuse) {
            Err(MobError::ForkInstructionRefused { refusal, .. }) => (
                serde_json::to_value(&refusal).expect("refusal wire json"),
                refusal.to_string(),
            ),
            other => panic!(
                "expected a typed refusal, got {:?}",
                other.map(|p| p.render())
            ),
        }
    }

    fn readable_load_skill() -> Vec<WireHistoryRow> {
        vec![
            load_skill_call(
                "call-1",
                json!({"source_uuid": SKILL_SOURCE, "skill_name": "pdf-processing"}),
            ),
            tool_result("call-1", "SKILL-BODY"),
        ]
    }

    #[test]
    fn a_refusal_of_keys_only_names_the_keys_only() {
        let (wire, message) = refusal_of(vec![activation("member", "a1", "BODY-1"), user("hi")]);
        assert_eq!(wire["keys"], json!([["host.standing", "member"]]), "{wire}");
        assert!(wire.get("skills").is_none(), "{wire}");
        assert!(wire.get("unreadable_skill_loads").is_none(), "{wire}");
        assert!(
            message.contains("would inherit activated instruction keys [host.standing/member] and the fork instruction policy refuses them"),
            "{message}"
        );
        assert!(!message.contains("skill"), "{message}");
    }

    #[test]
    fn a_refusal_of_loaded_skills_only_names_the_skills_only() {
        let (wire, message) = refusal_of(readable_load_skill());
        assert_eq!(wire["keys"], json!([]), "{wire}");
        assert_eq!(wire["skills"].as_array().map(Vec::len), Some(1), "{wire}");
        assert!(wire.get("unreadable_skill_loads").is_none(), "{wire}");
        assert!(
            message.contains(&format!(
                "would inherit skills loaded by the source [{SKILL_SOURCE}/pdf-processing] and the fork instruction policy refuses them"
            )),
            "{message}"
        );
        assert!(!message.contains("instruction keys"), "{message}");
    }

    #[test]
    fn a_refusal_of_unreadable_load_skill_calls_counts_the_calls() {
        let (wire, message) = refusal_of(vec![
            load_skill_call("call-1", json!({"bogus": 1})),
            tool_result("call-1", "SKILL-BODY"),
        ]);
        assert_eq!(wire["keys"], json!([]), "{wire}");
        assert!(wire.get("skills").is_none(), "{wire}");
        assert_eq!(wire["unreadable_skill_loads"], json!(1), "{wire}");
        assert!(
            message.contains("would inherit 1 load_skill call whose skill cannot be read and the fork instruction policy refuses them"),
            "{message}"
        );
        assert!(
            !message.contains("instruction keys") && !message.contains("skills loaded"),
            "{message}"
        );
    }

    #[test]
    fn a_refusal_of_keys_and_skills_names_both() {
        let mut rows = vec![activation("member", "a1", "BODY-1")];
        rows.extend(readable_load_skill());
        let (wire, message) = refusal_of(rows);
        assert_eq!(wire["keys"], json!([["host.standing", "member"]]), "{wire}");
        assert_eq!(wire["skills"].as_array().map(Vec::len), Some(1), "{wire}");
        assert!(wire.get("unreadable_skill_loads").is_none(), "{wire}");
        assert!(
            message.contains(&format!(
                "would inherit activated instruction keys [host.standing/member] and skills loaded by the source [{SKILL_SOURCE}/pdf-processing] and the fork instruction policy refuses them"
            )),
            "{message}"
        );
    }

    /// A key the source switched off is still a key it activated: `refuse`
    /// refuses the fork, as it does a durable fork of the same source.
    #[test]
    fn refuse_refuses_a_source_whose_only_key_is_switched_off() {
        let (wire, message) = refusal_of(vec![
            activation("member", "a1", "BODY-1"),
            activation("member", "a2", ""),
            user("hi"),
        ]);
        assert_eq!(wire["keys"], json!([["host.standing", "member"]]), "{wire}");
        assert!(message.contains("host.standing/member"), "{message}");
    }

    fn seed_of(rows: Vec<WireHistoryRow>, policy: ForkInstructionPolicy) -> ForkSeed {
        plan(rows, policy).expect("the fork is planned").seed(
            ForkNoticeSource {
                member: None,
                session_id: None,
            },
            ForkSeedSourceTools::NotRead {
                reason: meerkat_core::fork_notice::ForkToolsNotComparedReason::SourceNotLive,
            },
        )
    }

    /// An earlier fork's typed rows as a child's history holds them: its
    /// planned context and the notice that says what that context carried.
    fn earlier_fork_rows(carried: Vec<InstructionKeyRef>) -> Vec<WireHistoryRow> {
        let seed = ForkSeed {
            fork: ForkNoticeKind::PromptContext,
            source: ForkNoticeSource {
                member: None,
                session_id: None,
            },
            planned_context: None,
            source_instructions: carried.clone(),
            instructions_carried: carried,
            skills_removed: Vec::new(),
            skills_carried: Vec::new(),
            injected_rows_removed: 0,
            injected_rows_carried: 0,
            earlier_rows_omitted: 0,
            source_tools: ForkSeedSourceTools::NotRead {
                reason: meerkat_core::fork_notice::ForkToolsNotComparedReason::SourceNotLive,
            },
        };
        let notice = seed.notice(SessionId::new(), &BTreeSet::new(), &BTreeSet::new());
        vec![
            user_row(
                "[Forked conversation context from member 'grandparent']\nEARLIER-CONTEXT",
                TranscriptUserRole::InjectedContext,
            ),
            WireHistoryRow(WireSessionMessage::SystemNotice {
                kind: SystemNoticeKind::Generic,
                runtime_origin: None,
                body: None,
                blocks: vec![SystemNoticeBlock::ForkBoundary(Box::new(notice))],
                created_at: AT.to_string(),
            }),
        ]
    }

    fn standing_ref() -> InstructionKeyRef {
        InstructionKeyRef::new(
            InstructionNamespace::new("host.standing").unwrap(),
            InstructionKey::new("member").unwrap(),
        )
    }

    #[test]
    fn the_seed_carries_the_planned_rows_without_the_notice_line() {
        let rows = vec![
            system("You are the source."),
            activation("member", "standing-1", "STANDING-BODY"),
            user_row("RECALL", TranscriptUserRole::InjectedContext),
            load_skill_call(
                "call-1",
                json!({"source_uuid": SKILL_SOURCE, "skill_name": "pdf-processing"}),
            ),
            tool_result("call-1", "SKILL-BODY"),
            user("the conversation"),
        ];
        let seed = seed_of(
            rows.clone(),
            ForkInstructionPolicy::RequireExplicitActivation,
        );
        let context = seed.planned_context.clone().expect("a prompt-context seed");
        assert!(context.contains("[user]: the conversation"), "{context}");
        assert!(!context.contains("[fork_notice]"), "{context}");
        for leaked in [
            "You are the source.",
            "STANDING-BODY",
            "RECALL",
            "SKILL-BODY",
        ] {
            assert!(!context.contains(leaked), "{leaked} leaked: {context}");
        }
        assert_eq!(seed.source_instructions, vec![standing_ref()]);
        assert!(seed.instructions_carried.is_empty());
        assert_eq!(seed.skills_removed.len(), 1);
        assert!(seed.skills_carried.is_empty());
        assert_eq!(seed.injected_rows_removed, 1);

        let kept = seed_of(rows, ForkInstructionPolicy::ReactivateInherited);
        assert_eq!(kept.instructions_carried, vec![standing_ref()]);
        assert_eq!(kept.skills_carried.len(), 1);
        assert!(kept.skills_removed.is_empty());
        assert_eq!(kept.injected_rows_carried, 1);
        let notice = kept.notice(SessionId::new(), &BTreeSet::new(), &BTreeSet::new());
        assert!(
            notice.instructions_removed.is_empty(),
            "carried keys are not removed: {notice:?}"
        );
    }

    #[test]
    fn an_earlier_forks_notice_is_not_carried_and_its_context_is_session_state() {
        let mut rows = vec![system("You are the child.")];
        rows.extend(earlier_fork_rows(vec![standing_ref()]));
        rows.push(user("the child's task"));

        let text = rendered(
            rows.clone(),
            ForkInstructionPolicy::RequireExplicitActivation,
        );
        assert!(!text.contains("EARLIER-CONTEXT"), "{text}");
        assert!(!text.contains("[system_notice]"), "{text}");
        assert!(text.contains("[user]: the child's task"), "{text}");
        let seed = seed_of(
            rows.clone(),
            ForkInstructionPolicy::RequireExplicitActivation,
        );
        assert_eq!(
            seed.source_instructions,
            vec![standing_ref()],
            "the keys the earlier context carried are the source's"
        );
        assert_eq!(
            seed.injected_rows_removed, 0,
            "it is not a host-injected row"
        );

        match plan(rows.clone(), ForkInstructionPolicy::Refuse) {
            Err(MobError::ForkInstructionRefused { refusal, .. }) => {
                assert_eq!(refusal.keys.len(), 1, "{refusal:?}");
            }
            other => panic!(
                "refuse refuses the carried keys, got {:?}",
                other.map(|p| p.render())
            ),
        }
        let kept = rendered(rows, ForkInstructionPolicy::ReactivateInherited);
        assert!(kept.contains("EARLIER-CONTEXT"), "{kept}");
        assert!(
            !kept.contains("[system_notice]"),
            "the copied notice never is: {kept}"
        );
    }

    /// A durable fork's notice has no planned row before it: the
    /// host-injected row that precedes it is the source's own.
    #[test]
    fn a_transcript_notice_does_not_claim_the_injected_row_before_it() {
        let transcript = ForkSeed {
            fork: ForkNoticeKind::Transcript,
            source: ForkNoticeSource {
                member: None,
                session_id: None,
            },
            planned_context: None,
            source_instructions: Vec::new(),
            instructions_carried: Vec::new(),
            skills_removed: Vec::new(),
            skills_carried: Vec::new(),
            injected_rows_removed: 0,
            injected_rows_carried: 0,
            earlier_rows_omitted: 0,
            source_tools: ForkSeedSourceTools::NotRead {
                reason: meerkat_core::fork_notice::ForkToolsNotComparedReason::SourceNotLive,
            },
        };
        let notice = transcript.notice(SessionId::new(), &BTreeSet::new(), &BTreeSet::new());
        let rows = vec![
            user_row("RECALL-ROW", TranscriptUserRole::InjectedContext),
            WireHistoryRow(WireSessionMessage::SystemNotice {
                kind: SystemNoticeKind::Generic,
                runtime_origin: None,
                body: None,
                blocks: vec![SystemNoticeBlock::ForkBoundary(Box::new(notice))],
                created_at: AT.to_string(),
            }),
            user("hi"),
        ];
        let seed = seed_of(rows, ForkInstructionPolicy::RequireExplicitActivation);
        assert_eq!(
            seed.injected_rows_removed, 1,
            "the recall row is the source's host-injected context"
        );
    }

    #[test]
    fn the_budget_follows_the_child_model_window() {
        assert_eq!(fork_context_budget("no-such-model"), MAX_FORK_CONTEXT_BYTES);
        for entry in meerkat_models::catalog() {
            let budget = fork_context_budget(entry.id);
            assert!(budget <= MAX_FORK_CONTEXT_BYTES, "{}", entry.id);
            if let Some(window) = entry.context_window {
                assert!(
                    budget <= usize::try_from(window).unwrap() * 2,
                    "{}",
                    entry.id
                );
            }
        }
    }
}
