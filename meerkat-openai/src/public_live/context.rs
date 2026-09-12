//! Content-only context append plans. Encoding is not a durable attempt claim
//! and never authorizes sending, retrying, or marking a chunk injected.

use meerkat_core::live_execution::LiveContextIntent;
use meerkat_core::live_execution::profile::LiveProfileDefinition;
use meerkat_core::live_execution::request::LiveSourceIdentity;
use oai_rt_rs::live::{ClientEvent, Command, Nullable};
use sha2::{Digest, Sha256};

pub const LIVE_CONTEXT_APPEND_MAX_BYTES: usize = 400;
pub const LIVE_PROFILE_INSTRUCTIONS_MAX_BYTES: usize = 8192;
pub const LIVE_DISCLOSED_CONTENT_MAX_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveAppendChunkOrdinal(u32);

impl LiveAppendChunkOrdinal {
    pub const fn get(self) -> u32 {
        self.0
    }
}

pub struct LiveAppendPlan<'a> {
    intent: LiveContextIntent,
    content: &'a str,
    client_delegation: Option<&'a str>,
}

impl std::fmt::Debug for LiveAppendPlan<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveAppendPlan")
            .field("intent", &self.intent)
            .field("content_bytes", &self.content.len())
            .field("client_correlated", &self.client_delegation.is_some())
            .finish_non_exhaustive()
    }
}

impl<'a> LiveAppendPlan<'a> {
    /// Steering is taken only from the voice profile slot, never by relabeling
    /// executor or application text. The caller still needs a trusted binding.
    pub fn profile_instructions(
        profile: &'a LiveProfileDefinition,
    ) -> Result<Option<Self>, LiveAppendPlanError> {
        let Some(content) = profile.instructions.as_deref() else {
            return Ok(None);
        };
        Self::new(
            LiveContextIntent::Instructions,
            content,
            None,
            LIVE_PROFILE_INSTRUCTIONS_MAX_BYTES,
        )
        .map(Some)
    }

    pub fn thinking(
        disclosed_content: &'a str,
        source: &'a LiveSourceIdentity,
    ) -> Result<Self, LiveAppendPlanError> {
        Self::new(
            LiveContextIntent::Thinking,
            disclosed_content,
            client_delegation(source),
            LIVE_DISCLOSED_CONTENT_MAX_BYTES,
        )
    }

    pub fn commentary(
        disclosed_result: &'a str,
        source: &'a LiveSourceIdentity,
    ) -> Result<Self, LiveAppendPlanError> {
        Self::new(
            LiveContextIntent::Commentary,
            disclosed_result,
            client_delegation(source),
            LIVE_DISCLOSED_CONTENT_MAX_BYTES,
        )
    }

    fn new(
        intent: LiveContextIntent,
        content: &'a str,
        client_delegation: Option<&'a str>,
        limit: usize,
    ) -> Result<Self, LiveAppendPlanError> {
        if content.is_empty() {
            return Err(LiveAppendPlanError::EmptyContent);
        }
        if content.len() > limit {
            return Err(LiveAppendPlanError::ContentTooLarge);
        }
        Ok(Self {
            intent,
            content,
            client_delegation,
        })
    }

    pub fn chunks(&self) -> impl Iterator<Item = LiveAppendChunk<'a>> {
        let mut remaining = self.content;
        let mut ordinal = 0;
        let intent = self.intent;
        let client_delegation = self.client_delegation;
        std::iter::from_fn(move || {
            if remaining.is_empty() {
                return None;
            }
            let mut end = remaining.len().min(LIVE_CONTEXT_APPEND_MAX_BYTES);
            while !remaining.is_char_boundary(end) {
                end -= 1;
            }
            let (content, rest) = remaining.split_at(end);
            remaining = rest;
            let chunk = LiveAppendChunk {
                ordinal: LiveAppendChunkOrdinal(ordinal),
                intent,
                content,
                client_delegation,
            };
            ordinal += 1;
            Some(chunk)
        })
    }
}

fn client_delegation(source: &LiveSourceIdentity) -> Option<&str> {
    match source {
        LiveSourceIdentity::ClientDelegation { delegation } => Some(delegation.as_str()),
        LiveSourceIdentity::FunctionCall { .. } | LiveSourceIdentity::ApplicationRequest { .. } => {
            None
        }
    }
}

pub struct LiveAppendChunk<'a> {
    ordinal: LiveAppendChunkOrdinal,
    intent: LiveContextIntent,
    content: &'a str,
    client_delegation: Option<&'a str>,
}

impl LiveAppendChunk<'_> {
    pub const fn ordinal(&self) -> LiveAppendChunkOrdinal {
        self.ordinal
    }
    pub const fn intent(&self) -> LiveContextIntent {
        self.intent
    }
    pub fn content(&self) -> &str {
        self.content
    }

    /// Content-command identity, including intent and nullable delegation.
    /// A durable attempt must bind its own event ID and this digest separately.
    pub fn digest(&self) -> Result<[u8; 32], LiveAppendPlanError> {
        let bytes = serde_json::to_vec(&self.event().command)
            .map_err(|_| LiveAppendPlanError::EncodingFailed)?;
        let mut hash = Sha256::new();
        hash.update(b"meerkat.public-live-context-chunk.v1\0");
        hash.update(bytes);
        Ok(hash.finalize().into())
    }

    pub fn event(&self) -> ClientEvent {
        let content = self.content.to_owned();
        let delegation_id = Nullable(self.client_delegation.map(str::to_owned));
        ClientEvent::new(match self.intent {
            LiveContextIntent::Instructions => Command::InstructionsAppend {
                content,
                delegation_id,
            },
            LiveContextIntent::Thinking => Command::ThinkingAppend {
                content,
                delegation_id,
            },
            LiveContextIntent::Commentary => Command::CommentaryAppend {
                content,
                delegation_id,
            },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveAppendPlanError {
    #[error("public Live context content is empty")]
    EmptyContent,
    #[error("public Live context content exceeds its local UTF-8 byte bound")]
    ContentTooLarge,
    #[error("public Live context chunk encoding failed")]
    EncodingFailed,
}
