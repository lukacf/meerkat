//! Provider-neutral replay planning and verified adapter lowering.

use crate::{
    AssistantBlock, ContentBlock, Message, ProviderMeta, SystemNoticeBlock, SystemNoticeMessage,
    ToolResult,
};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplayMessageIndex(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplayProjectedMessageIndex(pub usize);
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
    #[error("replay projection has an unexpected message at {projected:?}")]
    UnexpectedProjectedMessage {
        projected: ReplayProjectedMessageIndex,
    },
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
        for (notice_block_index, projected_block) in projected.blocks.iter_mut().enumerate() {
            let projected_content = match projected_block {
                SystemNoticeBlock::Comms { content, .. }
                | SystemNoticeBlock::ExternalEvent { content, .. } => content,
                _ => continue,
            };
            for (content_index, projected) in projected_content.iter_mut().enumerate() {
                let subject = ReplaySubject::SystemNoticeContent {
                    message,
                    notice_block: ReplaySystemNoticeBlockIndex(notice_block_index),
                    content_block: ReplaySystemNoticeContentIndex(content_index),
                };
                match self.consume(subject)? {
                    ReplayDisposition::Preserve => {}
                    ReplayDisposition::LowerToText => {
                        *projected = ContentBlock::Text {
                            text: projected.text_projection().into_owned(),
                        };
                    }
                    disposition => {
                        return Err(ReplayPlanError::ProjectionMismatch {
                            subject,
                            disposition,
                        });
                    }
                }
            }
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
        validate_final_projection(source, projected, self.target)?;
        validate_tool_adjacency(projected)
    }
}

fn validate_final_projection(
    source: &[Message],
    projected: &[Message],
    target: ReplayTarget,
) -> Result<(), ReplayPlanError> {
    let mut projected_index = 0;
    for (message_index, source_message) in source.iter().enumerate() {
        let message_index = ReplayMessageIndex(message_index);
        if let Message::BlockAssistant(source_assistant) = source_message {
            let projected_assistant =
                projected
                    .get(projected_index)
                    .and_then(|message| match message {
                        Message::BlockAssistant(assistant) => Some(assistant),
                        _ => None,
                    });
            let consumed = validate_final_assistant_projection(
                message_index,
                source_assistant,
                projected_assistant,
                target,
            )?;
            projected_index += usize::from(consumed);
            continue;
        }

        let Some(projected_message) = projected.get(projected_index) else {
            return Err(message_projection_mismatch(message_index));
        };
        let valid = match (source_message, projected_message) {
            (Message::System(source), Message::System(actual)) => source == actual,
            (Message::User(source), Message::User(actual)) => {
                let expected_content = source
                    .content
                    .iter()
                    .enumerate()
                    .map(|(block_index, block)| {
                        project_content_block(
                            block,
                            target,
                            false,
                            ReplaySubject::UserContent {
                                message: message_index,
                                block: ReplayUserContentIndex(block_index),
                            },
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                source.render_metadata == actual.render_metadata
                    && source.identity == actual.identity
                    && source.transcript_role == actual.transcript_role
                    && source.created_at == actual.created_at
                    && expected_content == actual.content
            }
            (
                Message::ToolResults {
                    results: source,
                    created_at: source_created_at,
                },
                Message::ToolResults {
                    results: actual,
                    created_at: actual_created_at,
                },
            ) => {
                let expected = source
                    .iter()
                    .enumerate()
                    .map(|(result_index, result)| {
                        project_tool_result_for_target(
                            message_index,
                            ReplayToolResultIndex(result_index),
                            result,
                            target,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                source_created_at == actual_created_at && expected == *actual
            }
            (Message::SystemNotice(source), Message::SystemNotice(actual)) => {
                project_system_notice_for_target(message_index, source, target)? == *actual
            }
            _ => false,
        };
        if !valid {
            return Err(match source_message {
                Message::System(_) => ReplayPlanError::PreservedSystemMessageMismatch,
                _ => message_projection_mismatch(message_index),
            });
        }
        projected_index += 1;
    }
    if projected_index != projected.len() {
        return Err(ReplayPlanError::UnexpectedProjectedMessage {
            projected: ReplayProjectedMessageIndex(projected_index),
        });
    }
    Ok(())
}

fn validate_final_assistant_projection(
    message: ReplayMessageIndex,
    source: &crate::BlockAssistantMessage,
    projected: Option<&crate::BlockAssistantMessage>,
    target: ReplayTarget,
) -> Result<bool, ReplayPlanError> {
    let requires_message = source.blocks.iter().any(|block| {
        matches!(
            assistant_disposition(block, target),
            ReplayDisposition::Preserve | ReplayDisposition::LowerToText
        )
    });
    let Some(projected) = projected else {
        return if requires_message {
            Err(message_projection_mismatch(message))
        } else {
            Ok(false)
        };
    };
    if !requires_message
        && projected.blocks.first().is_some_and(|first| {
            !source
                .blocks
                .iter()
                .any(|block| assistant_block_matches_target(block, first, target))
        })
    {
        return Ok(false);
    }
    if source.stop_reason != projected.stop_reason
        || source.identity != projected.identity
        || source.created_at != projected.created_at
    {
        return Err(message_projection_mismatch(message));
    }

    let mut projected_block = 0;
    for (block_index, source_block) in source.blocks.iter().enumerate() {
        let subject = ReplaySubject::AssistantBlock {
            message,
            block: ReplayAssistantBlockIndex(block_index),
        };
        let disposition = assistant_disposition(source_block, target);
        let actual = projected.blocks.get(projected_block);
        let consumes = match disposition {
            ReplayDisposition::Preserve => {
                validate_final_assistant_block(subject, source_block, actual, target)?;
                true
            }
            ReplayDisposition::LowerToText => {
                let valid = actual.is_some_and(|block| {
                    matches!(block, AssistantBlock::Text { text, meta: None } if *text == source_text_projection(source_block))
                });
                if !valid {
                    return Err(projection_mismatch(subject, disposition));
                }
                true
            }
            ReplayDisposition::Omit => false,
            ReplayDisposition::ProviderNative => {
                if actual.is_some_and(|block| {
                    assistant_block_matches_target(source_block, block, target)
                }) {
                    validate_final_assistant_block(subject, source_block, actual, target)?;
                    true
                } else {
                    false
                }
            }
            ReplayDisposition::CollapseToText => {
                return Err(projection_mismatch(subject, disposition));
            }
        };
        projected_block += usize::from(consumes);
    }
    if projected_block != projected.blocks.len() {
        return Err(message_projection_mismatch(message));
    }
    Ok(true)
}

fn validate_final_assistant_block(
    subject: ReplaySubject,
    source: &AssistantBlock,
    projected: Option<&AssistantBlock>,
    target: ReplayTarget,
) -> Result<(), ReplayPlanError> {
    if !assistant_payload_eq(source, projected) {
        return Err(projection_mismatch(subject, ReplayDisposition::Preserve));
    }
    let source_meta = assistant_meta(source);
    let projected_meta = projected.and_then(assistant_meta);
    let expected_meta = source_meta.filter(|metadata| {
        ReplayWireFamily::from_provider_metadata(metadata) == target.wire_family
    });
    if projected_meta != expected_meta {
        return Err(projection_mismatch(
            subject,
            ReplayDisposition::ProviderNative,
        ));
    }
    Ok(())
}

fn assistant_block_matches_target(
    source: &AssistantBlock,
    projected: &AssistantBlock,
    target: ReplayTarget,
) -> bool {
    if !assistant_payload_eq(source, Some(projected)) {
        return false;
    }
    let expected_meta = assistant_meta(source).filter(|metadata| {
        ReplayWireFamily::from_provider_metadata(metadata) == target.wire_family
    });
    assistant_meta(projected) == expected_meta
}

fn project_content_block(
    block: &ContentBlock,
    target: ReplayTarget,
    tool_result: bool,
    subject: ReplaySubject,
) -> Result<ContentBlock, ReplayPlanError> {
    match content_disposition(block, target, tool_result) {
        ReplayDisposition::Preserve => Ok(block.clone()),
        ReplayDisposition::LowerToText => Ok(ContentBlock::Text {
            text: block.text_projection().into_owned(),
        }),
        disposition => Err(projection_mismatch(subject, disposition)),
    }
}

fn project_tool_result_for_target(
    message: ReplayMessageIndex,
    result: ReplayToolResultIndex,
    source: &ToolResult,
    target: ReplayTarget,
) -> Result<ToolResult, ReplayPlanError> {
    for (block_index, block) in source.content.iter().enumerate() {
        if matches!(block, ContentBlock::Video { .. }) {
            return Err(ReplayPlanError::UnsupportedCapability {
                subject: ReplaySubject::ToolResultContent {
                    message,
                    result,
                    block: ReplayToolResultContentIndex(block_index),
                },
                capability: ReplayCapability::ToolResultVideo,
            });
        }
    }
    let projected = source
        .content
        .iter()
        .enumerate()
        .map(|(block_index, block)| {
            project_content_block(
                block,
                target,
                true,
                ReplaySubject::ToolResultContent {
                    message,
                    result,
                    block: ReplayToolResultContentIndex(block_index),
                },
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let content = match target.tool_result_projection {
        ReplayToolResultProjection::PreserveBlocks => projected,
        ReplayToolResultProjection::CollapseToText => {
            ContentBlock::text_vec(crate::types::text_content(&projected))
        }
    };
    Ok(ToolResult::with_blocks(
        source.tool_use_id.clone(),
        content,
        source.is_error,
    ))
}

fn project_system_notice_for_target(
    message: ReplayMessageIndex,
    source: &SystemNoticeMessage,
    target: ReplayTarget,
) -> Result<SystemNoticeMessage, ReplayPlanError> {
    let mut projected = source.clone();
    for (notice_block_index, block) in projected.blocks.iter_mut().enumerate() {
        match block {
            SystemNoticeBlock::Comms { content, .. }
            | SystemNoticeBlock::ExternalEvent { content, .. } => {
                *content = content
                    .iter()
                    .enumerate()
                    .map(|(content_block_index, block)| {
                        project_content_block(
                            block,
                            target,
                            false,
                            ReplaySubject::SystemNoticeContent {
                                message,
                                notice_block: ReplaySystemNoticeBlockIndex(notice_block_index),
                                content_block: ReplaySystemNoticeContentIndex(content_block_index),
                            },
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
            }
            _ => {}
        }
    }
    Ok(projected)
}

fn message_projection_mismatch(message: ReplayMessageIndex) -> ReplayPlanError {
    projection_mismatch(
        ReplaySubject::Message(message),
        ReplayDisposition::ProviderNative,
    )
}

fn projection_mismatch(subject: ReplaySubject, disposition: ReplayDisposition) -> ReplayPlanError {
    ReplayPlanError::ProjectionMismatch {
        subject,
        disposition,
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

    #[test]
    fn final_projection_validation_rejects_output_changed_after_recording() {
        let source_block = ContentBlock::Structured {
            data: serde_json::value::RawValue::from_string(r#"{"key":"value"}"#.to_string())
                .unwrap_or_else(|error| panic!("test structured content: {error}")),
        };
        let source = [Message::User(UserMessage::with_blocks(vec![
            source_block.clone(),
        ]))];
        let plan = ReplayPlan::build(
            &source,
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
                &source[0],
                None,
            )
            .unwrap_or_else(|error| panic!("test message: {error}"));
        let projected_block = ContentBlock::Text {
            text: source_block.text_projection().into_owned(),
        };
        application
            .record_content(
                ReplaySubject::UserContent {
                    message: ReplayMessageIndex(0),
                    block: ReplayUserContentIndex(0),
                },
                &source_block,
                &projected_block,
            )
            .unwrap_or_else(|error| panic!("test content: {error}"));

        assert!(matches!(
            plan.validate_projected(&source, &source, application),
            Err(ReplayPlanError::ProjectionMismatch {
                subject: ReplaySubject::Message(ReplayMessageIndex(0)),
                ..
            })
        ));
    }

    #[test]
    fn final_projection_aligns_identical_provider_native_payloads_by_metadata() {
        let foreign = AssistantBlock::Reasoning {
            text: "same reasoning".to_string(),
            meta: Some(Box::new(ProviderMeta::Gemini {
                thought_signature: "foreign".to_string(),
            })),
        };
        let native = AssistantBlock::Reasoning {
            text: "same reasoning".to_string(),
            meta: Some(Box::new(ProviderMeta::Anthropic {
                signature: "native".to_string(),
            })),
        };
        let source_assistant =
            BlockAssistantMessage::new(vec![foreign.clone(), native.clone()], StopReason::EndTurn);
        let mut projected_assistant = source_assistant.clone();
        projected_assistant.blocks = vec![native.clone()];
        let source = [Message::BlockAssistant(source_assistant)];
        let projected = [Message::BlockAssistant(projected_assistant)];
        let plan = ReplayPlan::build(
            &source,
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
                &source[0],
                None,
            )
            .unwrap_or_else(|error| panic!("test message: {error}"));
        application
            .record_assistant(
                ReplaySubject::AssistantBlock {
                    message: ReplayMessageIndex(0),
                    block: ReplayAssistantBlockIndex(0),
                },
                &foreign,
                None,
            )
            .unwrap_or_else(|error| panic!("foreign assistant block: {error}"));
        application
            .record_provider_metadata(
                ReplaySubject::ProviderMetadata {
                    message: ReplayMessageIndex(0),
                    block: ReplayAssistantBlockIndex(0),
                },
                assistant_meta(&foreign).unwrap_or_else(|| panic!("foreign metadata")),
                &foreign,
                None,
            )
            .unwrap_or_else(|error| panic!("foreign metadata: {error}"));
        application
            .record_assistant(
                ReplaySubject::AssistantBlock {
                    message: ReplayMessageIndex(0),
                    block: ReplayAssistantBlockIndex(1),
                },
                &native,
                Some(&native),
            )
            .unwrap_or_else(|error| panic!("native assistant block: {error}"));
        application
            .record_provider_metadata(
                ReplaySubject::ProviderMetadata {
                    message: ReplayMessageIndex(0),
                    block: ReplayAssistantBlockIndex(1),
                },
                assistant_meta(&native).unwrap_or_else(|| panic!("native metadata")),
                &native,
                Some(&native),
            )
            .unwrap_or_else(|error| panic!("native metadata: {error}"));

        plan.validate_projected(&source, &projected, application)
            .unwrap_or_else(|error| panic!("valid projection: {error}"));
    }
}
