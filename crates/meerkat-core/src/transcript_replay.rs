//! Provider-neutral replay planning and verified adapter lowering.

use crate::{
    AssistantBlock, ContentBlock, Message, OpenAiContinuityIssuer, ProviderMeta, SystemNoticeBlock,
    SystemNoticeMessage, ToolResult,
};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// How many responses one generation of a ledger holds. A ledger keeps two
/// generations, so it remembers between this many and twice this many of
/// the most recent responses.
const OPENAI_CONTINUITY_LEDGER_GENERATION: usize = 4_096;

/// How many pieces of material (provider metadata values and hosted-tool
/// items) one response may record. A response that carries more is not
/// recorded and stays of unknown issuer.
const OPENAI_CONTINUITY_LEDGER_RESPONSE_MATERIAL: usize = 64;

/// One piece of continuity material an assistant step carries: an
/// OpenAI-family provider metadata value, or a hosted-tool item, whose
/// provider-issued id lives inside its content.
#[derive(Debug, Clone, Copy)]
pub enum OpenAiContinuityMaterial<'a> {
    Metadata(&'a ProviderMeta),
    HostedToolItem(&'a serde_json::Value),
}

impl<'a> OpenAiContinuityMaterial<'a> {
    /// The material the block `block` carries: its OpenAI-family provider
    /// metadata, and its content when it is a hosted-tool block.
    pub fn of_block(block: &'a AssistantBlock) -> impl Iterator<Item = Self> + 'a {
        let metadata = replay_provider_metadata(block)
            .filter(|metadata| {
                ReplayWireFamily::from_provider_metadata(metadata) == ReplayWireFamily::OpenAi
            })
            .map(Self::Metadata);
        let hosted = match block {
            AssistantBlock::ServerToolContent { content, .. } => {
                Some(Self::HostedToolItem(content))
            }
            _ => None,
        };
        metadata.into_iter().chain(hosted)
    }

    /// The identity of the material: a digest of the whole value as it is
    /// stored. `None` for metadata of another provider family.
    fn key(self) -> Option<[u8; 32]> {
        let (kind, stored) = match self {
            Self::Metadata(metadata) => {
                if ReplayWireFamily::from_provider_metadata(metadata) != ReplayWireFamily::OpenAi {
                    return None;
                }
                (&b"metadata\0"[..], serde_json::to_vec(metadata).ok()?)
            }
            Self::HostedToolItem(content) => (
                &b"hosted-tool-item\0"[..],
                serde_json::to_vec(content).ok()?,
            ),
        };
        let mut hasher = Sha256::new();
        hasher.update(b"openai-continuity-material\0");
        hasher.update(kind);
        hasher.update(&stored);
        Some(hasher.finalize().into())
    }
}

/// What this process knows about which service issued OpenAI-family
/// continuity material (reasoning items, message items, response ids,
/// hosted-tool items).
///
/// A client records each response it receives; replay asks whether an
/// assistant step of a transcript is one this process received from the
/// service it is about to be sent to ([`ReplayStepAdmission`]). The
/// knowledge is of the running process only: it is never serialized, so
/// another process, or this one after a restart, knows nothing about the
/// material in a stored transcript and treats it as of unknown issuer.
///
/// The ledger is keyed by the material itself (a SHA-256 of each provider
/// metadata value and each hosted-tool item exactly as it is stored: id,
/// ciphertext, response id, phase, item content), not by the message that
/// carries it, so a clone, a fork, a rewrite or a serialization round trip
/// of the transcript inside the process does not lose it, and no message
/// type carries the issuer. Material two issuers both returned byte for
/// byte is contested and belongs to neither.
///
/// Bound: digests only, never material. A response records at most 64
/// pieces of material (one that carries more is not recorded). The ledger
/// holds two generations of 4,096 responses each: when the current
/// generation is full the previous one is forgotten as a whole and the
/// current one becomes the previous, so the ledger remembers the most
/// recent 4,096 to 8,192 responses and forgets in batches, never one step
/// per request. An entry is a 32-byte digest and a 4-byte issuer index:
/// about 1.5 MiB for 8,192 typical responses of four pieces each, and at
/// most about 24 MiB when every response carries 64.
///
/// One ledger serves every session of the process, because the issuer is
/// the service and not the session. It holds plain `std::sync` state, takes
/// its lock only inside its own synchronous methods, and recovers a
/// poisoned lock instead of failing other sessions.
pub struct OpenAiContinuityLedger {
    inner: Mutex<OpenAiContinuityLedgerInner>,
    generation: usize,
}

#[derive(Default)]
struct OpenAiContinuityLedgerInner {
    /// Every issuer this ledger has recorded material for; entries hold an
    /// index into it.
    issuers: Vec<OpenAiContinuityIssuer>,
    current: HashMap<[u8; 32], OpenAiContinuityLedgerEntry>,
    previous: HashMap<[u8; 32], OpenAiContinuityLedgerEntry>,
    /// Responses recorded into `current`.
    current_responses: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenAiContinuityLedgerEntry {
    Issuer(u32),
    /// Two issuers returned this exact material: it identifies neither.
    Contested,
}

impl OpenAiContinuityLedgerInner {
    fn entry(&self, key: &[u8; 32]) -> Option<OpenAiContinuityLedgerEntry> {
        self.current
            .get(key)
            .or_else(|| self.previous.get(key))
            .copied()
    }

    fn issuer_index(&mut self, issuer: &OpenAiContinuityIssuer) -> Option<u32> {
        let index = match self.issuers.iter().position(|known| known == issuer) {
            Some(index) => index,
            None => {
                self.issuers.push(issuer.clone());
                self.issuers.len() - 1
            }
        };
        u32::try_from(index).ok()
    }
}

impl Default for OpenAiContinuityLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for OpenAiContinuityLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiContinuityLedger")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl OpenAiContinuityLedger {
    /// An empty ledger: one that has received nothing, as a new process.
    #[must_use]
    pub fn new() -> Self {
        Self::with_generation(OPENAI_CONTINUITY_LEDGER_GENERATION)
    }

    /// An empty ledger whose generations hold `responses` responses each.
    #[must_use]
    pub fn with_generation(responses: usize) -> Self {
        Self {
            inner: Mutex::new(OpenAiContinuityLedgerInner::default()),
            generation: responses.max(1),
        }
    }

    /// The ledger every client of this process shares unless it is built
    /// with its own.
    #[must_use]
    pub fn process() -> Arc<Self> {
        static PROCESS: OnceLock<Arc<OpenAiContinuityLedger>> = OnceLock::new();
        Arc::clone(PROCESS.get_or_init(|| Arc::new(Self::new())))
    }

    /// Record one response this process received from `issuer`: every
    /// piece of material it carried, as a whole.
    pub fn record_response<'a>(
        &self,
        issuer: &OpenAiContinuityIssuer,
        material: impl IntoIterator<Item = OpenAiContinuityMaterial<'a>>,
    ) {
        let mut keys: Vec<[u8; 32]> = Vec::new();
        for key in material
            .into_iter()
            .filter_map(OpenAiContinuityMaterial::key)
        {
            if !keys.contains(&key) {
                keys.push(key);
            }
            if keys.len() > OPENAI_CONTINUITY_LEDGER_RESPONSE_MATERIAL {
                return;
            }
        }
        if keys.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(index) = inner.issuer_index(issuer) else {
            return;
        };
        for key in keys {
            let entry = match inner.entry(&key) {
                None => OpenAiContinuityLedgerEntry::Issuer(index),
                Some(OpenAiContinuityLedgerEntry::Issuer(recorded)) if recorded == index => {
                    OpenAiContinuityLedgerEntry::Issuer(index)
                }
                Some(_) => OpenAiContinuityLedgerEntry::Contested,
            };
            inner.current.insert(key, entry);
        }
        inner.current_responses += 1;
        if inner.current_responses >= self.generation {
            inner.previous = std::mem::take(&mut inner.current);
            inner.current_responses = 0;
        }
    }

    /// Record the assistant step `step` as one response received from
    /// `issuer`.
    pub fn record_step(&self, issuer: &OpenAiContinuityIssuer, step: &[AssistantBlock]) {
        self.record_response(
            issuer,
            step.iter().flat_map(OpenAiContinuityMaterial::of_block),
        );
    }

    /// Whether this process received the assistant step `step` from
    /// `issuer`: the step carries material, and every piece of it is
    /// material `issuer`, and no other issuer, returned. A step is known as
    /// a whole or not at all, and a step that carries no material proves
    /// nothing.
    #[must_use]
    pub fn received_step_from(
        &self,
        issuer: &OpenAiContinuityIssuer,
        step: &[AssistantBlock],
    ) -> bool {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(index) = inner
            .issuers
            .iter()
            .position(|known| known == issuer)
            .and_then(|index| u32::try_from(index).ok())
        else {
            return false;
        };
        let mut material = step
            .iter()
            .flat_map(OpenAiContinuityMaterial::of_block)
            .filter_map(OpenAiContinuityMaterial::key)
            .peekable();
        material.peek().is_some()
            && material
                .all(|key| inner.entry(&key) == Some(OpenAiContinuityLedgerEntry::Issuer(index)))
    }
}

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
///
/// The grammar says which shapes a target parses. It does not say whose
/// opaque continuity material the target can read: every OpenAI-wire service
/// parses a reasoning item, and only the service that issued one can decrypt
/// it. [`ReplayStepAdmission`] decides that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayWireFamily {
    Anthropic,
    OpenAi,
    Gemini,
}

/// An OpenAI-wire replay target as a receiver of continuity material: the
/// service it is, and the ledger that knows what this process received from
/// which service.
#[derive(Debug, Clone)]
pub struct OpenAiContinuityTarget {
    issuer: OpenAiContinuityIssuer,
    ledger: Arc<OpenAiContinuityLedger>,
}

impl OpenAiContinuityTarget {
    #[must_use]
    pub fn new(issuer: OpenAiContinuityIssuer, ledger: Arc<OpenAiContinuityLedger>) -> Self {
        Self { issuer, ledger }
    }

    /// The service this target is.
    #[must_use]
    pub fn issuer(&self) -> &OpenAiContinuityIssuer {
        &self.issuer
    }
}

/// The one owner of the question "may this provider continuity metadata be
/// replayed to this target", answered for one assistant step (the output of
/// one provider response).
///
/// Metadata is admitted when it is of the target's wire family and, for the
/// OpenAI family, when the whole step is one this process received from the
/// service the target is. A step is admitted as a whole or not at all: a
/// reasoning item, the items beside it and the ids they carry go together,
/// so no item can keep a provider id whose companion was left out. A step
/// this process did not receive (a transcript from a store, another process,
/// a forgotten response), a step another service returned, and any step for
/// a target that does not say which service it is, are not admitted.
///
/// Anthropic and Gemini metadata records no issuer, so for those families
/// the wire family alone decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayStepAdmission {
    wire_family: ReplayWireFamily,
    openai_step_received: bool,
}

impl ReplayStepAdmission {
    /// Decide for the assistant step `step` of a transcript replayed to a
    /// target of `wire_family` that, on the OpenAI wire, is the service
    /// `openai` names. This is the only reader of the continuity ledger.
    #[must_use]
    pub fn of(
        wire_family: ReplayWireFamily,
        openai: Option<&OpenAiContinuityTarget>,
        step: &[AssistantBlock],
    ) -> Self {
        let openai_step_received = wire_family == ReplayWireFamily::OpenAi
            && openai.is_some_and(|target| target.ledger.received_step_from(&target.issuer, step));
        Self {
            wire_family,
            openai_step_received,
        }
    }

    /// Whether `metadata`, carried by a block of the step, may be replayed
    /// to the target.
    #[must_use]
    pub fn admits(&self, metadata: &ProviderMeta) -> bool {
        if ReplayWireFamily::from_provider_metadata(metadata) != self.wire_family {
            return false;
        }
        match self.wire_family {
            ReplayWireFamily::OpenAi => self.openai_step_received,
            ReplayWireFamily::Anthropic | ReplayWireFamily::Gemini => true,
        }
    }

    /// Whether the step's provider-issued items that carry their identity
    /// in their own content (hosted-tool items, which hold a provider item
    /// id and are paired with the step's reasoning item) may be replayed to
    /// the target. On the OpenAI wire they follow their step; the other
    /// wire families decide by their own content rules, as before.
    #[must_use]
    pub fn admits_provider_items(&self) -> bool {
        match self.wire_family {
            ReplayWireFamily::OpenAi => self.openai_step_received,
            ReplayWireFamily::Anthropic | ReplayWireFamily::Gemini => true,
        }
    }

    /// Remove from `block`, a block of the step, continuity metadata the
    /// target does not admit.
    #[must_use]
    pub fn strip_foreign_metadata(&self, mut block: AssistantBlock) -> AssistantBlock {
        if replay_provider_metadata(&block).is_some_and(|metadata| !self.admits(metadata)) {
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

impl ReplayWireFamily {
    #[must_use]
    pub const fn from_provider_metadata(metadata: &ProviderMeta) -> Self {
        match metadata {
            ProviderMeta::Anthropic { .. }
            | ProviderMeta::AnthropicRedacted { .. }
            | ProviderMeta::AnthropicCompaction { .. } => Self::Anthropic,
            ProviderMeta::Gemini { .. } | ProviderMeta::GeminiFunctionCall { .. } => Self::Gemini,
            ProviderMeta::OpenAi { .. }
            | ProviderMeta::OpenAiResponse { .. }
            | ProviderMeta::OpenAiAssistantMessage { .. } => Self::OpenAi,
        }
    }

    /// Remove continuity metadata a target of this wire family that names
    /// no OpenAI service does not admit ([`ReplayStepAdmission`]): metadata
    /// of another wire family and, on the OpenAI wire, all of it. An
    /// OpenAI-wire target that knows which service it is asks its plan
    /// ([`ReplayPlan::step_admission`]).
    #[must_use]
    pub fn strip_foreign_metadata(self, block: AssistantBlock) -> AssistantBlock {
        ReplayStepAdmission::of(self, None, &[]).strip_foreign_metadata(block)
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
    /// The admission of each source message's step, by message index;
    /// decided once when the plan is built. A message that is not an
    /// assistant step holds the admission of an empty step.
    admissions: Vec<ReplayStepAdmission>,
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
                | SystemNoticeBlock::ExternalEvent { content, .. }
                | SystemNoticeBlock::RuntimeNotice { content, .. } => content,
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
    /// Plan the replay of `messages` to `target`. An OpenAI-wire target
    /// planned this way names no service, so no OpenAI-family continuity
    /// metadata is admitted
    /// ([`Self::build_with_openai_continuity`] names one).
    pub fn build(messages: &[Message], target: ReplayTarget) -> Result<Self, ReplayPlanError> {
        Self::build_with_openai_continuity(messages, target, None)
    }

    /// Plan the replay of `messages` to `target`, which on the OpenAI wire
    /// is the service `openai` names: a step is admitted only when this
    /// process received it from that service ([`ReplayStepAdmission`]).
    pub fn build_with_openai_continuity(
        messages: &[Message],
        target: ReplayTarget,
        openai: Option<&OpenAiContinuityTarget>,
    ) -> Result<Self, ReplayPlanError> {
        let mut admissions = Vec::with_capacity(messages.len());
        let mut entries = Vec::new();
        for (message_index, message) in messages.iter().enumerate() {
            let index = ReplayMessageIndex(message_index);
            let step: &[AssistantBlock] = match message {
                Message::BlockAssistant(assistant) => &assistant.blocks,
                _ => &[],
            };
            let admission = ReplayStepAdmission::of(target.wire_family, openai, step);
            admissions.push(admission);
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
                                disposition: if admission.admits(meta) {
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
                            | SystemNoticeBlock::ExternalEvent { content, .. }
                            | SystemNoticeBlock::RuntimeNotice { content, .. } => content,
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
        Ok(Self {
            target,
            entries,
            admissions,
        })
    }

    /// The admission of the assistant step at `message` of the planned
    /// transcript: which of its continuity metadata the target is sent.
    #[must_use]
    pub fn step_admission(&self, message: ReplayMessageIndex) -> ReplayStepAdmission {
        self.admissions
            .get(message.0)
            .copied()
            .unwrap_or_else(|| ReplayStepAdmission::of(self.target.wire_family, None, &[]))
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
        validate_final_projection(source, projected, self)?;
        validate_tool_adjacency(projected)
    }
}

fn validate_final_projection(
    source: &[Message],
    projected: &[Message],
    plan: &ReplayPlan,
) -> Result<(), ReplayPlanError> {
    let target = plan.target;
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
                plan.step_admission(message_index),
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
    admission: ReplayStepAdmission,
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
                .any(|block| assistant_block_matches_target(block, first, admission))
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
                validate_final_assistant_block(subject, source_block, actual, admission)?;
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
                    assistant_block_matches_target(source_block, block, admission)
                }) {
                    validate_final_assistant_block(subject, source_block, actual, admission)?;
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
    admission: ReplayStepAdmission,
) -> Result<(), ReplayPlanError> {
    if !assistant_payload_eq(source, projected) {
        return Err(projection_mismatch(subject, ReplayDisposition::Preserve));
    }
    let source_meta = assistant_meta(source);
    let projected_meta = projected.and_then(assistant_meta);
    let expected_meta = source_meta.filter(|metadata| admission.admits(metadata));
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
    admission: ReplayStepAdmission,
) -> bool {
    if !assistant_payload_eq(source, Some(projected)) {
        return false;
    }
    let expected_meta = assistant_meta(source).filter(|metadata| admission.admits(metadata));
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
            | SystemNoticeBlock::ExternalEvent { content, .. }
            | SystemNoticeBlock::RuntimeNotice { content, .. } => {
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
        AssistantBlock::Text {
            text,
            meta: Some(meta),
        } if text.is_empty()
            && matches!(meta.as_ref(), ProviderMeta::OpenAiAssistantMessage { .. }) =>
        {
            ReplayDisposition::ProviderNative
        }
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
        AssistantBlock::Transcript { text, source, .. } => source.text_for_model(text).into_owned(),
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
            AssistantBlock::Reasoning { text: left, .. },
            AssistantBlock::Reasoning { text: right, .. },
        ) => left == right,
        (
            AssistantBlock::Transcript {
                text: left,
                source: left_source,
                ..
            },
            AssistantBlock::Transcript {
                text: right,
                source: right_source,
                ..
            },
        ) => left == right && left_source == right_source,
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
    fn unmeasured_transcript_lowering_cannot_drop_the_source_qualifier() {
        let source = AssistantBlock::Transcript {
            text: "voice-only discussion".into(),
            source: crate::TranscriptSource::SpokenUnmeasured,
            meta: None,
        };
        let messages = [Message::BlockAssistant(BlockAssistantMessage::new(
            vec![source.clone()],
            StopReason::EndTurn,
        ))];
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
        .unwrap_or_else(|error| panic!("replay plan: {error}"));
        for (text, valid) in [
            ("voice-only discussion".to_string(), false),
            (
                crate::TranscriptSource::SpokenUnmeasured
                    .text_for_model("voice-only discussion")
                    .into_owned(),
                true,
            ),
        ] {
            let projected = AssistantBlock::Text { text, meta: None };
            let mut application = plan.application();
            application
                .record_message(
                    ReplaySubject::Message(ReplayMessageIndex(0)),
                    &messages[0],
                    Some(&messages[0]),
                )
                .unwrap_or_else(|error| panic!("message replay: {error}"));
            assert_eq!(
                application
                    .record_assistant(
                        ReplaySubject::AssistantBlock {
                            message: ReplayMessageIndex(0),
                            block: ReplayAssistantBlockIndex(0)
                        },
                        &source,
                        Some(&projected),
                    )
                    .is_ok(),
                valid
            );
        }
        let relabelled = AssistantBlock::Transcript {
            text: "voice-only discussion".into(),
            source: crate::TranscriptSource::Spoken,
            meta: None,
        };
        assert!(!assistant_payload_eq(&source, Some(&relabelled)));
    }

    #[test]
    fn empty_text_native_replay_is_limited_to_openai_assistant_items() {
        let target = ReplayTarget::new(
            ReplayWireFamily::OpenAi,
            false,
            false,
            false,
            ReplayToolResultProjection::CollapseToText,
            ReplayReasoningProjection::ProviderNative,
        );
        for meta in [
            ProviderMeta::Anthropic {
                signature: "signature".to_string(),
            },
            ProviderMeta::Gemini {
                thought_signature: "signature".to_string(),
            },
            ProviderMeta::OpenAiResponse {
                response_id: "resp_1".to_string(),
            },
            ProviderMeta::OpenAi {
                id: "rs_1".to_string(),
                encrypted_content: None,
                phase: None,
                response_id: None,
            },
        ] {
            let block = AssistantBlock::Text {
                text: String::new(),
                meta: Some(Box::new(meta)),
            };
            assert_eq!(
                assistant_disposition(&block, target),
                ReplayDisposition::Omit
            );
        }
        let block = AssistantBlock::Text {
            text: String::new(),
            meta: Some(Box::new(ProviderMeta::OpenAiAssistantMessage {
                id: "msg_1".to_string(),
                phase: Some(crate::types::OpenAiAssistantPhase::Commentary),
                response_id: None,
            })),
        };
        assert_eq!(
            assistant_disposition(&block, target),
            ReplayDisposition::ProviderNative
        );
    }

    #[test]
    fn gemini_native_call_metadata_is_preserved_only_for_gemini_replay() {
        let block = AssistantBlock::ToolUse {
            id: "fc_0".into(),
            name: "lookup".into(),
            args: serde_json::value::to_raw_value(&serde_json::json!({})).unwrap(),
            meta: Some(Box::new(ProviderMeta::GeminiFunctionCall {
                provider_id: "provider-call".into(),
                thought_signature: Some("opaque-signature".into()),
            })),
        };
        assert_eq!(
            ReplayWireFamily::Gemini.strip_foreign_metadata(block.clone()),
            block
        );
        for family in [ReplayWireFamily::Anthropic, ReplayWireFamily::OpenAi] {
            assert!(matches!(family.strip_foreign_metadata(block.clone()),
                AssistantBlock::ToolUse { id, meta: None, .. } if id == "fc_0"));
        }
    }

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

#[cfg(test)]
mod continuity_issuer_tests;
