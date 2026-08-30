//! Provider-neutral replay planning and verified adapter lowering.

use crate::{
    AssistantBlock, ContentBlock, Message, ProviderMeta, SystemNoticeBlock, SystemNoticeMessage,
    ToolResult,
};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplayMessageIndex(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplayAssistantBlockIndex(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplayUserContentIndex(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplayToolResultIndex(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplayToolResultContentIndex(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplaySystemNoticeBlockIndex(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplaySystemNoticeContentIndex(pub usize);

/// The provider wire grammar that receives replay, independently of account
/// provider identity. Self-hosted and Copilot OpenAI-compatible endpoints use
/// [`Self::OpenAi`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayWireFamily {
    Anthropic,
    OpenAi,
    Gemini,
}

impl ReplayWireFamily {
    #[must_use]
    pub const fn from_provider_metadata(metadata: &ProviderMeta) -> Self {
        match metadata {
            ProviderMeta::Anthropic { .. }
            | ProviderMeta::AnthropicRedacted { .. }
            | ProviderMeta::AnthropicCompaction { .. } => Self::Anthropic,
            ProviderMeta::Gemini { .. } => Self::Gemini,
            ProviderMeta::OpenAi { .. } | ProviderMeta::OpenAiResponse { .. } => Self::OpenAi,
        }
    }

    /// Remove continuity metadata that was issued by a different wire family.
    #[must_use]
    pub fn strip_foreign_metadata(self, mut block: AssistantBlock) -> AssistantBlock {
        if replay_provider_metadata(&block)
            .is_some_and(|metadata| Self::from_provider_metadata(metadata) != self)
        {
            match &mut block {
                AssistantBlock::Text { meta, .. }
                | AssistantBlock::Transcript { meta, .. }
                | AssistantBlock::Reasoning { meta, .. }
                | AssistantBlock::ToolUse { meta, .. }
                | AssistantBlock::ServerToolContent { meta, .. } => *meta = None,
                AssistantBlock::Image { .. } => {}
            }
        }
        block
    }
}

/// Read provider continuity metadata from an assistant block.
#[must_use]
pub fn replay_provider_metadata(block: &AssistantBlock) -> Option<&ProviderMeta> {
    match block {
        AssistantBlock::Text { meta, .. }
        | AssistantBlock::Transcript { meta, .. }
        | AssistantBlock::Reasoning { meta, .. }
        | AssistantBlock::ToolUse { meta, .. }
        | AssistantBlock::ServerToolContent { meta, .. } => meta.as_deref(),
        AssistantBlock::Image { .. } => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReplaySubject {
    Message(ReplayMessageIndex),
    AssistantBlock {
        message: ReplayMessageIndex,
        block: ReplayAssistantBlockIndex,
    },
    UserContent {
        message: ReplayMessageIndex,
        block: ReplayUserContentIndex,
    },
    ToolResultContent {
        message: ReplayMessageIndex,
        result: ReplayToolResultIndex,
        block: ReplayToolResultContentIndex,
    },
    ToolResult {
        message: ReplayMessageIndex,
        result: ReplayToolResultIndex,
    },
    SystemNoticeContent {
        message: ReplayMessageIndex,
        notice_block: ReplaySystemNoticeBlockIndex,
        content_block: ReplaySystemNoticeContentIndex,
    },
    ProviderMetadata {
        message: ReplayMessageIndex,
        block: ReplayAssistantBlockIndex,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayCapability {
    ToolResultVideo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayDisposition {
    Preserve,
    LowerToText,
    CollapseToText,
    Omit,
    /// The provider adapter owns wire-specific replay legality.
    ProviderNative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayPlanEntry {
    pub subject: ReplaySubject,
    pub disposition: ReplayDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayTarget {
    pub wire_family: ReplayWireFamily,
    pub image_input: bool,
    pub inline_video: bool,
    pub image_tool_results: bool,
    pub tool_result_projection: ReplayToolResultProjection,
    pub reasoning_projection: ReplayReasoningProjection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayToolResultProjection {
    PreserveBlocks,
    CollapseToText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayReasoningProjection {
    ProviderNative,
    LowerToText,
    Omit,
}

impl ReplayTarget {
    #[must_use]
    pub const fn new(
        wire_family: ReplayWireFamily,
        image_input: bool,
        inline_video: bool,
        image_tool_results: bool,
        tool_result_projection: ReplayToolResultProjection,
        reasoning_projection: ReplayReasoningProjection,
    ) -> Self {
        Self {
            wire_family,
            image_input,
            inline_video,
            image_tool_results,
            tool_result_projection,
            reasoning_projection,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplayPlanError {
    #[error("replay application expected {expected:?}, received {actual:?}")]
    UnexpectedSubject {
        expected: Option<ReplaySubject>,
        actual: ReplaySubject,
    },
    #[error("replay projection does not satisfy {disposition:?} for {subject:?}")]
    ProjectionMismatch {
        subject: ReplaySubject,
        disposition: ReplayDisposition,
    },
    #[error("replay projection changed or omitted an ordered system message")]
    PreservedSystemMessageMismatch,
    #[error(
        "replay application ended before its complete plan was consumed; next subject is {next:?}"
    )]
    IncompleteApplication { next: ReplaySubject },
    #[error("tool results at message {message:?} have no immediately preceding tool use")]
    ToolResultsWithoutToolUse { message: ReplayMessageIndex },
    #[error("tool uses at message {message:?} have no immediately adjacent tool results")]
    ToolUseWithoutToolResults { message: ReplayMessageIndex },
    #[error("tool results at message {message:?} do not exactly match the preceding tool uses")]
    ToolResultMismatch { message: ReplayMessageIndex },
    #[error("tool uses at message {message:?} reuse a tool-use ID")]
    DuplicateToolUseId { message: ReplayMessageIndex },
    #[error("tool results at message {message:?} reuse a tool-result ID")]
    DuplicateToolResultId { message: ReplayMessageIndex },
    #[error("replay subject {subject:?} requires unsupported capability {capability:?}")]
    UnsupportedCapability {
        subject: ReplaySubject,
        capability: ReplayCapability,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayPlan {
    target: ReplayTarget,
    entries: Vec<ReplayPlanEntry>,
}

/// O(1) ordered replay-plan verifier. Adapters pass the source and their
/// concrete projected output to the typed record methods; labels cannot be
/// self-attested.
pub struct ReplayApplication<'a> {
    plan: &'a ReplayPlan,
    cursor: usize,
}

impl ReplayApplication<'_> {
    pub fn next(&self, subject: ReplaySubject) -> Result<ReplayDisposition, ReplayPlanError> {
        let expected = self.plan.entries.get(self.cursor);
        if expected.map(|entry| entry.subject) != Some(subject) {
            return Err(ReplayPlanError::UnexpectedSubject {
                expected: expected.map(|entry| entry.subject),
                actual: subject,
            });
        }
        Ok(expected
            .map(|entry| entry.disposition)
            .unwrap_or(ReplayDisposition::Omit))
    }

    fn consume(&mut self, subject: ReplaySubject) -> Result<ReplayDisposition, ReplayPlanError> {
        let disposition = self.next(subject)?;
        self.cursor += 1;
        Ok(disposition)
    }

    pub fn record_message(
        &mut self,
        subject: ReplaySubject,
        source: &Message,
        projected: Option<&Message>,
    ) -> Result<(), ReplayPlanError> {
        let disposition = self.consume(subject)?;
        if matches!(disposition, ReplayDisposition::Preserve) && projected != Some(source) {
            return Err(ReplayPlanError::ProjectionMismatch {
                subject,
                disposition,
            });
        }
        Ok(())
    }

    pub fn record_content(
        &mut self,
        subject: ReplaySubject,
        source: &ContentBlock,
        projected: &ContentBlock,
    ) -> Result<(), ReplayPlanError> {
        let disposition = self.consume(subject)?;
        let valid = match disposition {
            ReplayDisposition::Preserve => projected == source,
            ReplayDisposition::LowerToText => is_text_projection(source, projected),
            ReplayDisposition::CollapseToText
            | ReplayDisposition::ProviderNative
            | ReplayDisposition::Omit => false,
        };
        if valid {
            Ok(())
        } else {
            Err(ReplayPlanError::ProjectionMismatch {
                subject,
                disposition,
            })
        }
    }

    /// Build and validate one complete tool result, including provider-required
    /// collapse of multiple canonical blocks into one text payload.
    pub fn project_tool_result(
        &mut self,
        message: ReplayMessageIndex,
        result_index: ReplayToolResultIndex,
        source: &ToolResult,
    ) -> Result<ToolResult, ReplayPlanError> {
        let mut projected_blocks = Vec::with_capacity(source.content.len());
        for (block_index, block) in source.content.iter().enumerate() {
            let subject = ReplaySubject::ToolResultContent {
                message,
                result: result_index,
                block: ReplayToolResultContentIndex(block_index),
            };
            let projected = match self.next(subject)? {
                ReplayDisposition::Preserve => block.clone(),
                ReplayDisposition::LowerToText => ContentBlock::Text {
                    text: block.text_projection().into_owned(),
                },
                disposition => {
                    return Err(ReplayPlanError::ProjectionMismatch {
                        subject,
                        disposition,
                    });
                }
            };
            self.record_content(subject, block, &projected)?;
            projected_blocks.push(projected);
        }

        let subject = ReplaySubject::ToolResult {
            message,
            result: result_index,
        };
        let disposition = self.consume(subject)?;
        let content = match disposition {
            ReplayDisposition::ProviderNative => projected_blocks,
            ReplayDisposition::CollapseToText => {
                ContentBlock::text_vec(crate::types::text_content(&projected_blocks))
            }
            _ => {
                return Err(ReplayPlanError::ProjectionMismatch {
                    subject,
                    disposition,
                });
            }
        };
        Ok(ToolResult::with_blocks(
            source.tool_use_id.clone(),
            content,
            source.is_error,
        ))
    }

    pub fn project_system_notice(
        &mut self,
        message: ReplayMessageIndex,
        notice: &SystemNoticeMessage,
    ) -> Result<SystemNoticeMessage, ReplayPlanError> {
        self.consume(ReplaySubject::Message(message))?;
        let mut projected = notice.clone();
        for (notice_block_index, (source_block, projected_block)) in notice
            .blocks
            .iter()
            .zip(projected.blocks.iter_mut())
            .enumerate()
        {
            let (source_content, projected_content) = match (source_block, projected_block) {
                (
                    SystemNoticeBlock::Comms {
                        content: source, ..
                    },
                    SystemNoticeBlock::Comms {
                        content: projected, ..
                    },
                )
                | (
                    SystemNoticeBlock::ExternalEvent {
                        content: source, ..
                    },
                    SystemNoticeBlock::ExternalEvent {
                        content: projected, ..
                    },
                ) => (source, projected),
                _ => continue,
            };
            let mut next_content = Vec::with_capacity(source_content.len());
            for (content_index, source) in source_content.iter().enumerate() {
                let subject = ReplaySubject::SystemNoticeContent {
                    message,
                    notice_block: ReplaySystemNoticeBlockIndex(notice_block_index),
                    content_block: ReplaySystemNoticeContentIndex(content_index),
                };
                let next = match self.next(subject)? {
                    ReplayDisposition::Preserve => source.clone(),
                    ReplayDisposition::LowerToText => ContentBlock::Text {
                        text: source.text_projection().into_owned(),
                    },
                    disposition => {
                        return Err(ReplayPlanError::ProjectionMismatch {
                            subject,
                            disposition,
                        });
                    }
                };
                self.record_content(subject, source, &next)?;
                next_content.push(next);
            }
            *projected_content = next_content;
        }
        Ok(projected)
    }

    pub fn record_assistant(
        &mut self,
        subject: ReplaySubject,
        source: &AssistantBlock,
        projected: Option<&AssistantBlock>,
    ) -> Result<(), ReplayPlanError> {
        let disposition = self.consume(subject)?;
        if assistant_meta(source).is_none() && projected.and_then(assistant_meta).is_some() {
            return Err(ReplayPlanError::ProjectionMismatch {
                subject,
                disposition,
            });
        }
        let valid = match disposition {
            ReplayDisposition::Preserve => assistant_payload_eq(source, projected),
            ReplayDisposition::LowerToText => projected.is_some_and(|block| {
                matches!(block, AssistantBlock::Text { text, meta: None } if *text == source_text_projection(source))
            }),
            ReplayDisposition::CollapseToText => false,
            ReplayDisposition::Omit => projected.is_none(),
            ReplayDisposition::ProviderNative => native_assistant_outcome(source, projected),
        };
        if valid {
            Ok(())
        } else {
            Err(ReplayPlanError::ProjectionMismatch {
                subject,
                disposition,
            })
        }
    }

    pub fn record_provider_metadata(
        &mut self,
        subject: ReplaySubject,
        source: &ProviderMeta,
        source_block: &AssistantBlock,
        projected: Option<&AssistantBlock>,
    ) -> Result<(), ReplayPlanError> {
        let disposition = self.consume(subject)?;
        let valid = match disposition {
            ReplayDisposition::Omit => projected.and_then(assistant_meta).is_none(),
            ReplayDisposition::ProviderNative => {
                if assistant_payload_eq(source_block, projected) {
                    projected.and_then(assistant_meta) == Some(source)
                } else {
                    projected.and_then(assistant_meta).is_none()
                }
            }
            ReplayDisposition::Preserve
            | ReplayDisposition::LowerToText
            | ReplayDisposition::CollapseToText => false,
        };
        if valid {
            Ok(())
        } else {
            Err(ReplayPlanError::ProjectionMismatch {
                subject,
                disposition,
            })
        }
    }

    pub fn finish(self) -> Result<(), ReplayPlanError> {
        self.plan
            .entries
            .get(self.cursor)
            .map(|entry| ReplayPlanError::IncompleteApplication {
                next: entry.subject,
            })
            .map_or(Ok(()), Err)
    }
}

impl ReplayPlan {
    pub fn build(messages: &[Message], target: ReplayTarget) -> Result<Self, ReplayPlanError> {
        let mut entries = Vec::new();
        for (message_index, message) in messages.iter().enumerate() {
            let index = ReplayMessageIndex(message_index);
            entries.push(ReplayPlanEntry {
                subject: ReplaySubject::Message(index),
                disposition: match message {
                    Message::System(_) => ReplayDisposition::Preserve,
                    Message::SystemNotice(_) => ReplayDisposition::ProviderNative,
                    _ => ReplayDisposition::ProviderNative,
                },
            });
            match message {
                Message::User(user) => {
                    for (block, content) in user.content.iter().enumerate() {
                        entries.push(ReplayPlanEntry {
                            subject: ReplaySubject::UserContent {
                                message: index,
                                block: ReplayUserContentIndex(block),
                            },
                            disposition: content_disposition(content, target, false),
                        });
                    }
                }
                Message::BlockAssistant(assistant) => {
                    for (block, assistant) in assistant.blocks.iter().enumerate() {
                        let block_index = ReplayAssistantBlockIndex(block);
                        entries.push(ReplayPlanEntry {
                            subject: ReplaySubject::AssistantBlock {
                                message: index,
                                block: block_index,
                            },
                            disposition: assistant_disposition(assistant, target),
                        });
                        if let Some(meta) = assistant_meta(assistant) {
                            entries.push(ReplayPlanEntry {
                                subject: ReplaySubject::ProviderMetadata {
                                    message: index,
                                    block: block_index,
                                },
                                disposition: if ReplayWireFamily::from_provider_metadata(meta)
                                    == target.wire_family
                                {
                                    ReplayDisposition::ProviderNative
                                } else {
                                    ReplayDisposition::Omit
                                },
                            });
                        }
                    }
                }
                Message::ToolResults { results, .. } => {
                    for (result, tool_result) in results.iter().enumerate() {
                        for (block, content) in tool_result.content.iter().enumerate() {
                            let subject = ReplaySubject::ToolResultContent {
                                message: index,
                                result: ReplayToolResultIndex(result),
                                block: ReplayToolResultContentIndex(block),
                            };
                            if matches!(content, ContentBlock::Video { .. }) {
                                return Err(ReplayPlanError::UnsupportedCapability {
                                    subject,
                                    capability: ReplayCapability::ToolResultVideo,
                                });
                            }
                            entries.push(ReplayPlanEntry {
                                subject,
                                disposition: content_disposition(content, target, true),
                            });
                        }
                        entries.push(ReplayPlanEntry {
                            subject: ReplaySubject::ToolResult {
                                message: index,
                                result: ReplayToolResultIndex(result),
                            },
                            disposition: match target.tool_result_projection {
                                ReplayToolResultProjection::PreserveBlocks => {
                                    ReplayDisposition::ProviderNative
                                }
                                ReplayToolResultProjection::CollapseToText => {
                                    ReplayDisposition::CollapseToText
                                }
                            },
                        });
                    }
                }
                Message::SystemNotice(notice) => {
                    for (notice_block, block) in notice.blocks.iter().enumerate() {
                        let content = match block {
                            SystemNoticeBlock::Comms { content, .. }
                            | SystemNoticeBlock::ExternalEvent { content, .. } => content,
                            _ => continue,
                        };
                        for (content_block, content) in content.iter().enumerate() {
                            entries.push(ReplayPlanEntry {
                                subject: ReplaySubject::SystemNoticeContent {
                                    message: index,
                                    notice_block: ReplaySystemNoticeBlockIndex(notice_block),
                                    content_block: ReplaySystemNoticeContentIndex(content_block),
                                },
                                disposition: content_disposition(content, target, false),
                            });
                        }
                    }
                }
                Message::System(_) => {}
            }
        }
        Ok(Self { target, entries })
    }

    #[must_use]
    pub const fn target(&self) -> ReplayTarget {
        self.target
    }

    pub fn entries(&self) -> impl ExactSizeIterator<Item = &ReplayPlanEntry> {
        self.entries.iter()
    }

    #[must_use]
    pub const fn application(&self) -> ReplayApplication<'_> {
        ReplayApplication {
            plan: self,
            cursor: 0,
        }
    }

    pub fn validate_projected(
        &self,
        source: &[Message],
        projected: &[Message],
        application: ReplayApplication<'_>,
    ) -> Result<(), ReplayPlanError> {
        application.finish()?;
        let source_system = source
            .iter()
            .filter(|message| matches!(message, Message::System(_)));
        let projected_system = projected
            .iter()
            .filter(|message| matches!(message, Message::System(_)));
        if !source_system.eq(projected_system) {
            return Err(ReplayPlanError::PreservedSystemMessageMismatch);
        }
        validate_tool_adjacency(projected)
    }
}

fn assistant_disposition(block: &AssistantBlock, target: ReplayTarget) -> ReplayDisposition {
    match block {
        AssistantBlock::Text { text, .. } if text.is_empty() => ReplayDisposition::Omit,
        AssistantBlock::Text { .. } | AssistantBlock::ToolUse { .. } => ReplayDisposition::Preserve,
        AssistantBlock::Transcript { text, .. } if text.is_empty() => ReplayDisposition::Omit,
        AssistantBlock::Transcript { .. } => ReplayDisposition::LowerToText,
        AssistantBlock::Reasoning { text, .. }
            if text.is_empty()
                && matches!(
                    target.reasoning_projection,
                    ReplayReasoningProjection::LowerToText
                ) =>
        {
            ReplayDisposition::Omit
        }
        AssistantBlock::Reasoning { .. } => match target.reasoning_projection {
            ReplayReasoningProjection::ProviderNative => ReplayDisposition::ProviderNative,
            ReplayReasoningProjection::LowerToText => ReplayDisposition::LowerToText,
            ReplayReasoningProjection::Omit => ReplayDisposition::Omit,
        },
        AssistantBlock::ServerToolContent { .. } => ReplayDisposition::ProviderNative,
        AssistantBlock::Image { .. } => ReplayDisposition::Omit,
    }
}

fn content_disposition(
    block: &ContentBlock,
    target: ReplayTarget,
    tool_result: bool,
) -> ReplayDisposition {
    match block {
        ContentBlock::Text { .. } => ReplayDisposition::Preserve,
        ContentBlock::Structured { .. } | ContentBlock::SkillContext { .. } => {
            ReplayDisposition::LowerToText
        }
        ContentBlock::Image { data, .. } => match data {
            crate::ImageData::Blob { .. } => ReplayDisposition::LowerToText,
            crate::ImageData::Inline { .. } if tool_result && !target.image_tool_results => {
                ReplayDisposition::LowerToText
            }
            crate::ImageData::Inline { .. } if !tool_result && !target.image_input => {
                ReplayDisposition::LowerToText
            }
            crate::ImageData::Inline { .. } => ReplayDisposition::Preserve,
        },
        ContentBlock::Video { .. } if target.inline_video => ReplayDisposition::Preserve,
        ContentBlock::Video { .. } => ReplayDisposition::LowerToText,
    }
}

fn assistant_meta(block: &AssistantBlock) -> Option<&ProviderMeta> {
    replay_provider_metadata(block)
}

fn is_text_projection(source: &ContentBlock, projected: &ContentBlock) -> bool {
    matches!(projected, ContentBlock::Text { text } if *text == source.text_projection())
}

fn source_text_projection(source: &AssistantBlock) -> String {
    match source {
        AssistantBlock::Transcript { text, .. } => text.clone(),
        AssistantBlock::Reasoning { text, .. } => format!("[Reasoning: {text}]"),
        _ => String::new(),
    }
}

fn assistant_payload_eq(source: &AssistantBlock, projected: Option<&AssistantBlock>) -> bool {
    let Some(projected) = projected else {
        return false;
    };
    match (source, projected) {
        (AssistantBlock::Text { text: left, .. }, AssistantBlock::Text { text: right, .. })
        | (
            AssistantBlock::Transcript { text: left, .. },
            AssistantBlock::Transcript { text: right, .. },
        )
        | (
            AssistantBlock::Reasoning { text: left, .. },
            AssistantBlock::Reasoning { text: right, .. },
        ) => left == right,
        (
            AssistantBlock::ToolUse {
                id: left_id,
                name: left_name,
                args: left_args,
                ..
            },
            AssistantBlock::ToolUse {
                id: right_id,
                name: right_name,
                args: right_args,
                ..
            },
        ) => left_id == right_id && left_name == right_name && left_args.get() == right_args.get(),
        (
            AssistantBlock::ServerToolContent {
                id: left_id,
                kind: left_kind,
                content: left_content,
                ..
            },
            AssistantBlock::ServerToolContent {
                id: right_id,
                kind: right_kind,
                content: right_content,
                ..
            },
        ) => left_id == right_id && left_kind == right_kind && left_content == right_content,
        (AssistantBlock::Image { .. }, AssistantBlock::Image { .. }) => source == projected,
        _ => false,
    }
}

fn native_assistant_outcome(source: &AssistantBlock, projected: Option<&AssistantBlock>) -> bool {
    projected.is_none() || assistant_payload_eq(source, projected)
}

fn tool_use_ids(message: &Message) -> Vec<&str> {
    match message {
        Message::BlockAssistant(assistant) => assistant
            .blocks
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn has_duplicate_ids(ids: &[&str]) -> bool {
    let mut seen = HashSet::with_capacity(ids.len());
    ids.iter().any(|id| !seen.insert(*id))
}

fn validate_tool_adjacency(messages: &[Message]) -> Result<(), ReplayPlanError> {
    let mut pending: Option<(ReplayMessageIndex, Vec<&str>)> = None;
    for (index, message) in messages.iter().enumerate() {
        let message_index = ReplayMessageIndex(index);
        if let Message::ToolResults { results, .. } = message {
            let Some((_, expected)) = pending.take() else {
                return Err(ReplayPlanError::ToolResultsWithoutToolUse {
                    message: message_index,
                });
            };
            let actual = results
                .iter()
                .map(|result| result.tool_use_id.as_str())
                .collect::<Vec<_>>();
            if has_duplicate_ids(&actual) {
                return Err(ReplayPlanError::DuplicateToolResultId {
                    message: message_index,
                });
            }
            let expected = expected.into_iter().collect::<HashSet<_>>();
            let actual = actual.into_iter().collect::<HashSet<_>>();
            if actual != expected {
                return Err(ReplayPlanError::ToolResultMismatch {
                    message: message_index,
                });
            }
            continue;
        }
        if let Some((message, _)) = pending {
            return Err(ReplayPlanError::ToolUseWithoutToolResults { message });
        }
        let ids = tool_use_ids(message);
        if has_duplicate_ids(&ids) {
            return Err(ReplayPlanError::DuplicateToolUseId {
                message: message_index,
            });
        }
        if !ids.is_empty() {
            pending = Some((message_index, ids));
        }
    }
    pending
        .map(|(message, _)| Err(ReplayPlanError::ToolUseWithoutToolResults { message }))
        .unwrap_or(Ok(()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{BlockAssistantMessage, StopReason, UserMessage};

    #[test]
    fn rejects_wrong_lowering_from_actual_projected_content() {
        let source = ContentBlock::Structured {
            data: serde_json::value::RawValue::from_string(r#"{"key":"value"}"#.to_string())
                .unwrap_or_else(|error| panic!("test structured content: {error}")),
        };
        let messages = [Message::User(UserMessage::with_blocks(vec![
            source.clone(),
        ]))];
        let plan = ReplayPlan::build(
            &messages,
            ReplayTarget::new(
                ReplayWireFamily::OpenAi,
                false,
                false,
                false,
                ReplayToolResultProjection::CollapseToText,
                ReplayReasoningProjection::Omit,
            ),
        )
        .unwrap_or_else(|error| panic!("test replay plan: {error}"));
        let mut application = plan.application();
        application
            .record_message(
                ReplaySubject::Message(ReplayMessageIndex(0)),
                &messages[0],
                None,
            )
            .unwrap_or_else(|error| panic!("test message: {error}"));
        assert!(matches!(
            application.record_content(
                ReplaySubject::UserContent {
                    message: ReplayMessageIndex(0),
                    block: ReplayUserContentIndex(0),
                },
                &source,
                &source,
            ),
            Err(ReplayPlanError::ProjectionMismatch {
                disposition: ReplayDisposition::LowerToText,
                ..
            })
        ));
    }

    #[test]
    fn rejects_leaked_foreign_provider_metadata() {
        let metadata = ProviderMeta::OpenAiResponse {
            response_id: "response".to_string(),
        };
        let block = AssistantBlock::Reasoning {
            text: "thought".to_string(),
            meta: Some(Box::new(metadata.clone())),
        };
        let messages = [Message::BlockAssistant(BlockAssistantMessage::new(
            vec![block.clone()],
            StopReason::EndTurn,
        ))];
        let plan = ReplayPlan::build(
            &messages,
            ReplayTarget::new(
                ReplayWireFamily::Anthropic,
                true,
                false,
                true,
                ReplayToolResultProjection::PreserveBlocks,
                ReplayReasoningProjection::ProviderNative,
            ),
        )
        .unwrap_or_else(|error| panic!("test replay plan: {error}"));
        let mut application = plan.application();
        application
            .record_message(
                ReplaySubject::Message(ReplayMessageIndex(0)),
                &messages[0],
                None,
            )
            .unwrap_or_else(|error| panic!("test message: {error}"));
        application
            .record_assistant(
                ReplaySubject::AssistantBlock {
                    message: ReplayMessageIndex(0),
                    block: ReplayAssistantBlockIndex(0),
                },
                &block,
                None,
            )
            .unwrap_or_else(|error| panic!("test assistant: {error}"));
        assert!(matches!(
            application.record_provider_metadata(
                ReplaySubject::ProviderMetadata {
                    message: ReplayMessageIndex(0),
                    block: ReplayAssistantBlockIndex(0),
                },
                &metadata,
                &block,
                Some(&block),
            ),
            Err(ReplayPlanError::ProjectionMismatch {
                disposition: ReplayDisposition::Omit,
                ..
            })
        ));
    }

    #[test]
    fn empty_reasoning_is_omitted_when_target_lowers_reasoning_to_text() {
        let messages = [Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Reasoning {
                text: String::new(),
                meta: Some(Box::new(ProviderMeta::AnthropicRedacted {
                    data: "encrypted".to_string(),
                })),
            }],
            StopReason::EndTurn,
        ))];
        let plan = ReplayPlan::build(
            &messages,
            ReplayTarget::new(
                ReplayWireFamily::Gemini,
                true,
                true,
                true,
                ReplayToolResultProjection::PreserveBlocks,
                ReplayReasoningProjection::LowerToText,
            ),
        )
        .unwrap_or_else(|error| panic!("test replay plan: {error}"));
        assert_eq!(
            plan.entries()
                .find(|entry| matches!(entry.subject, ReplaySubject::AssistantBlock { .. }))
                .map(|entry| entry.disposition),
            Some(ReplayDisposition::Omit)
        );
    }
}
