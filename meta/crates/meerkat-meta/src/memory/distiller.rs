//! Distiller — extraction from evidence
//! (docs/design/agent-memory-architecture.md §8.4).
//!
//! Runs **off-turn**: a bounded one-shot structured LLM call over a window
//! of session evidence, producing `remember`/`update` proposals that land
//! through the authored-write seam with `MemoryAuthor::Distiller` — never
//! above `AgentObserved`, quarantined when the evidence window is tainted
//! (§10.1) or when the window closed at a `reset()` boundary (§8.4).
//!
//! ## Harness: detached now, fork later (deliberate, not a shortcut)
//!
//! §8.4 prefers a `Session::fork` harness so the extraction call shares the
//! parent's prompt cache. The primitive exists in meerkat 0.7.9
//! (`Session::fork` / `fork_at`, meerkat-core `session.rs:3644/3764`), but
//! Meta can only reach it through the mob layer's fork launch mode
//! (`meerkat_mob::launch::MemberLaunchMode::Fork` + `ForkContext`,
//! `launch.rs:23/52`; one-shot `MobRuntimeHandle::fork_helper`,
//! `handle.rs:5946`) — which spawns a **live member carrying the parent's
//! full tool surface**. §8.4's fork containment ("every call gated to
//! read-only + propose/remember") needs an authorization/capability layer
//! that does not exist yet, and a fork-helper run gives back free text, not
//! this stage's validated structured ops. Shipping the fork path now would
//! mean an uncontained extractor with live tools. So P2 ships the
//! **detached bounded re-read** first-class — a one-shot client obtained
//! through the same `AgentFactory::build_llm_client_for_identity` seam the
//! Selector uses (§8.1), over a transcript slice read from the persistent
//! session store — with cost bounded by the §8.1 guards.
//!
//! TODO(§8.4 fork harness): when a capability-gated tool-authorization
//! layer lands, add a fork-based path via
//! `SpawnMemberSpec.launch_mode = MemberLaunchMode::Fork { source_member_id,
//! fork_context: ForkContext::FullHistory }` (the O(1) CoW
//! `Session::fork`), keeping the parent's tool list byte-identical for
//! prompt-cache sharing and moving containment to the authorization layer.
//! The detached path stays as the cross-process / cache-expired fallback.
//!
//! ## Triggers
//!
//! (a) **Completed interactions** — the observe-only agent-event stream
//! (same surface as the taint tracker), coalesced per session and
//! throttled (≥ `min_interactions` runs completed AND ≥
//! `MIN_SECONDS_BETWEEN_RUNS` since the last extraction). (b) **Session
//! rotation** — the identity runtime's respawn/retire/delete paths call
//! [`DistillerEngine::distill_now`] before rotation (bounded by
//! [`PRE_ROTATION_TIMEOUT`]; rotation never hangs on distillation), and
//! reset/resume-fallback spawn it detached off the critical path.
//! (c) **Compaction** — `AgentEvent::CompactionCompleted` is observable on
//! the agent-event stream; the discarded content survives only in
//! meerkat's session semantic memory, so the post-compaction run reads the
//! discard range host-side via `MemoryStore::enumerate_scoped` over that
//! session's scope (ask 8: exact, provenance-ordered, paged — read-only
//! [`HnswDiscardSource`]; opened lazily, drained, dropped — D3's re-index
//! cost is paid once per harvest, never held).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;

use meerkat_client::{BlockAssembler, LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::event::AgentEvent;
use meerkat_core::{Message, Provider, UserMessage};

use crate::identity_first::agent_memory::{
    AgentMemoryError, AgentMemoryProvider, MEMORY_TOOL_NAME, compact_whitespace,
    truncate_utf8_boundary,
};
use crate::memory::completion::{CompletionStop, UNTERMINATED_STREAM, completion_stop};
use crate::memory::factory_handle::FactoryModelClientHandle;
use crate::memory::guards::{BackgroundBudget, BackgroundBudgetConfig};
use crate::memory::records::{
    EvidenceRef, ManifestTier, MemoryAuthor, MemoryKind, MemoryScope, NewMemoryRecord, RecordMeta,
};
use crate::memory::taint::{MemberAgentEventSink, SessionTaintTracker};

/// Embedded prompt bundle (crate-local copy of
/// `tests/memory-evals/prompts/distiller-v0.md`; a unit test enforces byte
/// equality so the calibration artifact and the shipped default cannot
/// drift — same pattern as the Selector).
pub const EMBEDDED_PROMPT_V0: &str = include_str!("distiller_prompt_v0.md");

const MANIFEST_PLACEHOLDER: &str = "{{existing_manifest}}";
const TOMBSTONES_PLACEHOLDER: &str = "{{recent_tombstones}}";
const TRANSCRIPT_PLACEHOLDER: &str = "{{transcript}}";

/// Pre-rotation distillation budget: respawn/retire/delete wait at most
/// this long before proceeding with rotation (§8.4 — rotation must never
/// hang on distillation; a timed-out run is a loud skip).
pub const PRE_ROTATION_TIMEOUT: Duration = Duration::from_secs(15);
/// Per-identity coalescing floor between interaction-triggered runs.
pub const MIN_SECONDS_BETWEEN_RUNS: u64 = 120;
/// Tombstone window rendered into the prompt's "never re-create" list.
const TOMBSTONE_LOOKBACK_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// Row cap for one compaction-discard harvest query.
const COMPACTION_HARVEST_LIMIT: usize = 64;
/// Per-message and total byte bounds on the rendered transcript window.
const MAX_TRANSCRIPT_MESSAGE_BYTES: usize = 4 * 1024;
const MAX_TRANSCRIPT_TOTAL_BYTES: usize = 48 * 1024;
/// Window-state entries are bounded; least-recently-active evict first.
const MAX_TRACKED_WINDOWS: usize = 4096;
/// Output budget for the structured op list, overridable per deployment via
/// [`DistillerConfig::max_output_tokens`].
///
/// Why this is 16_384 and not the 2048 it used to be: the provider spends
/// the provider spends ONE budget on reasoning tokens AND the visible answer,
/// so a ceiling sized for a non-reasoning model is consumed before the op
/// list begins, and the truncation is silent rather than an error. The
/// steward's sibling constant at the old 4096 is why a production fleet
/// committed ZERO ops across twelve consecutive runs over four days; this
/// stage had the same defect one power of two lower.
///
/// 16_384 is the largest value that cannot be a hard provider rejection on
/// any text model in meerkat's catalog (the smallest cataloged ceiling is
/// `gpt-5.4-mini`'s 16_384), and an unreached ceiling costs nothing.
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 16_384;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum DistillerError {
    Profile(String),
    Auth(String),
    Client(String),
    /// The model stopped because it hit the output ceiling. Carried as its
    /// own variant because the alternative - a truncated body returned as an
    /// ordinary success - is how a production fleet ran this stage for four
    /// days committing zero ops while the error blamed the model's JSON.
    Truncated {
        max_output_tokens: u32,
    },
    /// The model stopped before a complete answer for a reason other than
    /// the output ceiling: a content filter, a cancelled response, a tool
    /// call the stage never offers, or a stop reason Meerkat adds later.
    /// Nothing is distilled from the partial body, however well it parses.
    Incomplete {
        stop_reason: meerkat_core::StopReason,
    },
    Parse(String),
    Store(String),
    Transcript(String),
}

impl std::fmt::Display for DistillerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Profile(msg) => write!(f, "distiller profile error: {msg}"),
            Self::Auth(msg) => write!(f, "distiller auth error: {msg}"),
            Self::Client(msg) => write!(f, "distiller client error: {msg}"),
            Self::Truncated { max_output_tokens } => write!(
                f,
                "distiller response hit the output ceiling of {max_output_tokens} tokens and was \
                 truncated; raise it via the distiller config's max_output_tokens (models spend this \
                 same budget on thinking/reasoning tokens as well as the answer)"
            ),
            Self::Incomplete { stop_reason } => write!(
                f,
                "distiller response stopped with {stop_reason:?} before a complete answer; \
                 nothing is distilled from it"
            ),
            Self::Parse(msg) => write!(f, "distiller parse error: {msg}"),
            Self::Store(msg) => write!(f, "distiller store error: {msg}"),
            Self::Transcript(msg) => write!(f, "distiller transcript error: {msg}"),
        }
    }
}

impl std::error::Error for DistillerError {}

// ---------------------------------------------------------------------------
// Calibration profile (§11)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct DistillerParams {
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    /// Manifest rows rendered into the prompt (newest-first when truncating).
    #[serde(default = "default_max_manifest_records")]
    pub max_manifest_records: usize,
    #[serde(default = "default_max_tombstones")]
    pub max_tombstones: usize,
}

fn default_temperature() -> f32 {
    0.0
}
fn default_max_output_tokens() -> u32 {
    DEFAULT_MAX_OUTPUT_TOKENS
}
fn default_max_manifest_records() -> usize {
    200
}
fn default_max_tombstones() -> usize {
    32
}

impl Default for DistillerParams {
    fn default() -> Self {
        Self {
            temperature: default_temperature(),
            max_output_tokens: default_max_output_tokens(),
            max_manifest_records: default_max_manifest_records(),
            max_tombstones: default_max_tombstones(),
        }
    }
}

/// A loaded distiller calibration profile (§11), prompt template resolved.
#[derive(Debug, Clone)]
pub struct DistillerProfile {
    pub stage: String,
    pub version: String,
    pub model: String,
    pub provider: Provider,
    pub prompt_bundle: String,
    pub prompt_template: String,
    pub params: DistillerParams,
}

#[derive(Debug, Deserialize)]
struct RawProfile {
    stage: String,
    version: String,
    model: String,
    #[serde(default)]
    provider: Option<String>,
    prompt_bundle: String,
    #[serde(default)]
    params: Option<DistillerParams>,
}

impl DistillerProfile {
    /// The embedded default: `tests/memory-evals/profiles/distiller-v0.toml` with
    /// the prompt compiled in. The model tier is a calibration decision
    /// (§11/§16); the config's `distiller.model` override adjusts it
    /// per-deployment without a new profile file.
    pub fn embedded_default() -> Self {
        Self {
            stage: "distiller".to_string(),
            version: "0".to_string(),
            model: "claude-haiku-4-5".to_string(),
            provider: Provider::Anthropic,
            prompt_bundle: "prompts/distiller-v0.md".to_string(),
            prompt_template: EMBEDDED_PROMPT_V0.to_string(),
            params: DistillerParams::default(),
        }
    }

    /// Replace the profile's model (the config-block override). Fail-loud:
    /// the model must resolve in the catalog unless a provider is already
    /// pinned by the profile.
    pub fn with_model_override(mut self, model: &str) -> Result<Self, DistillerError> {
        let model = model.trim();
        if model.is_empty() {
            return Err(DistillerError::Profile(
                "distiller model override must not be empty".to_string(),
            ));
        }
        self.provider = meerkat_models::infer_provider(model).ok_or_else(|| {
            DistillerError::Profile(format!(
                "distiller model override '{model}' is not in the model catalog"
            ))
        })?;
        self.model = model.to_string();
        Ok(self)
    }

    /// Replace the profile's output-token ceiling (the config-block
    /// override). Fail-loud in the same shape as [`Self::with_model_override`]:
    /// a zero budget cannot produce an answer, so it is a typed error rather
    /// than an extraction that silently proposes nothing.
    ///
    /// Deliberately no upper bound: the real ceiling is the model's, it is
    /// context-dependent, and the provider rejects an over-large request
    /// loudly - unlike the under-large one this override exists to fix.
    pub fn with_max_output_tokens(
        mut self,
        max_output_tokens: u32,
    ) -> Result<Self, DistillerError> {
        if max_output_tokens == 0 {
            return Err(DistillerError::Profile(
                "distiller max_output_tokens override must be greater than zero".to_string(),
            ));
        }
        self.params.max_output_tokens = max_output_tokens;
        Ok(self)
    }

    /// Load an external calibration profile (fail-loud), same layout rules
    /// as the Selector's loader.
    pub fn load(path: &Path) -> Result<Self, DistillerError> {
        let text = std::fs::read_to_string(path).map_err(|err| {
            DistillerError::Profile(format!("cannot read profile '{}': {err}", path.display()))
        })?;
        let raw: RawProfile = toml::from_str(&text).map_err(|err| {
            DistillerError::Profile(format!("invalid profile '{}': {err}", path.display()))
        })?;
        if raw.stage != "distiller" {
            return Err(DistillerError::Profile(format!(
                "profile '{}' is for stage '{}', not 'distiller'",
                path.display(),
                raw.stage
            )));
        }
        if raw.model.trim().is_empty() || raw.model == "PLACEHOLDER" {
            return Err(DistillerError::Profile(format!(
                "profile '{}' does not name a model",
                path.display()
            )));
        }
        let provider = match raw.provider.as_deref() {
            Some(name) => Provider::parse_strict(name).ok_or_else(|| {
                DistillerError::Profile(format!(
                    "profile '{}': unknown provider '{name}'",
                    path.display()
                ))
            })?,
            None => meerkat_models::infer_provider(&raw.model).ok_or_else(|| {
                DistillerError::Profile(format!(
                    "profile '{}': model '{}' is not in the catalog; set `provider` explicitly",
                    path.display(),
                    raw.model
                ))
            })?,
        };
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let candidates = [
            base.join(&raw.prompt_bundle),
            base.parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&raw.prompt_bundle),
        ];
        let bundle_path = candidates.iter().find(|p| p.is_file()).ok_or_else(|| {
            DistillerError::Profile(format!(
                "profile '{}': prompt_bundle '{}' does not resolve",
                path.display(),
                raw.prompt_bundle
            ))
        })?;
        let prompt_template = std::fs::read_to_string(bundle_path).map_err(|err| {
            DistillerError::Profile(format!(
                "cannot read prompt bundle '{}': {err}",
                bundle_path.display()
            ))
        })?;
        let profile = Self {
            stage: raw.stage,
            version: raw.version,
            model: raw.model,
            provider,
            prompt_bundle: raw.prompt_bundle,
            prompt_template,
            params: raw.params.unwrap_or_default(),
        };
        profile.validate()?;
        Ok(profile)
    }

    fn validate(&self) -> Result<(), DistillerError> {
        for placeholder in [
            MANIFEST_PLACEHOLDER,
            TOMBSTONES_PLACEHOLDER,
            TRANSCRIPT_PLACEHOLDER,
        ] {
            if !self.prompt_template.contains(placeholder) {
                return Err(DistillerError::Profile(format!(
                    "prompt bundle '{}' is missing placeholder `{placeholder}`",
                    self.prompt_bundle
                )));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Config (`agent_memory.distiller { ... }`)
// ---------------------------------------------------------------------------

/// Distiller config block. `enabled` defaults **off** for this landing:
/// flipping the default is a calibration-scorecard decision (§11), like the
/// Selector's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistillerConfig {
    pub enabled: bool,
    /// §8.1 hard per-window cap on distillation runs, per realm.
    pub runs_per_hour: u32,
    /// Interaction-trigger threshold: completed runs per session before an
    /// extraction is considered.
    pub min_interactions: u32,
    /// Optional model override applied to the embedded default profile.
    pub model: Option<String>,
    /// Output-token ceiling for the extraction call. `None` keeps the
    /// embedded profile's default.
    ///
    /// Exposed because the provider spends this single budget on
    /// reasoning tokens AND the visible answer, so a ceiling sized for a
    /// non-reasoning model truncates the op list to nothing without raising
    /// an error. The defect was not that the default was wrong but that it
    /// was a private constant a deployment could neither see nor change;
    /// [`crate::memory::steward::StewardConfig::max_output_tokens`] carries
    /// the production incident that made it visible, and
    /// [`crate::memory::hygienist::HygienistConfig`] has the same knob.
    pub max_output_tokens: Option<u32>,
}

impl Default for DistillerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            runs_per_hour: crate::memory::guards::DEFAULT_RUNS_PER_HOUR,
            min_interactions: 3,
            model: None,
            max_output_tokens: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Structured output: proposed ops
// ---------------------------------------------------------------------------

/// Epistemic attribution (§8.4 doctrine rule 5). Mechanically mirrored into
/// an `epistemic:*` tag on the landed record, same convention as the
/// Recorder tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Epistemic {
    OperatorSaid,
    Observed,
}

impl Epistemic {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "operator_said" => Some(Self::OperatorSaid),
            "observed" => Some(Self::Observed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposedAction {
    Remember,
    Update { target_id: String },
}

/// One validated distiller proposal.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedOp {
    pub action: ProposedAction,
    pub kind: MemoryKind,
    pub title: String,
    pub description: String,
    pub body: String,
    pub tags: Vec<String>,
    pub epistemic: Epistemic,
    pub evidence_range: Option<(u64, u64)>,
}

#[derive(Debug, Deserialize)]
struct RawOp {
    action: String,
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    title: String,
    #[serde(default)]
    description: String,
    body: String,
    #[serde(default)]
    tags: Vec<String>,
    epistemic: String,
    #[serde(default)]
    evidence_range: Option<(u64, u64)>,
}

/// Strict parse of the model's op list: exactly one JSON array (fenced or
/// prefixed output tolerated by extracting the outermost `[..]`).
pub fn parse_ops(reply: &str) -> Result<Vec<RawParsedOp>, String> {
    let trimmed = reply.trim();
    let raw: Vec<RawOp> = match serde_json::from_str(trimmed) {
        Ok(raw) => raw,
        Err(first_err) => {
            let (Some(start), Some(end)) = (trimmed.find('['), trimmed.rfind(']')) else {
                return Err(format!("no JSON array in reply: {first_err}"));
            };
            if start >= end {
                return Err(format!("no JSON array in reply: {first_err}"));
            }
            serde_json::from_str(&trimmed[start..=end]).map_err(|err| err.to_string())?
        }
    };
    Ok(raw.into_iter().map(RawParsedOp).collect())
}

/// A syntactically-parsed op awaiting semantic validation. Opaque on
/// purpose: callers go through [`validate_op`].
#[derive(Debug)]
pub struct RawParsedOp(RawOp);

/// Semantic validation of one parsed op against the manifest the model was
/// shown. Invalid ops are per-op skips (warned by the caller), not run
/// failures — one bad op must not discard the window's good ones.
pub fn validate_op(op: RawParsedOp, manifest_ids: &[String]) -> Result<ProposedOp, String> {
    let raw = op.0;
    let action = match raw.action.as_str() {
        "remember" => ProposedAction::Remember,
        "update" => {
            let target = raw
                .target_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "update op is missing target_id".to_string())?;
            if !manifest_ids.iter().any(|id| id == target) {
                return Err(format!(
                    "update op targets '{target}', which is not in the manifest"
                ));
            }
            ProposedAction::Update {
                target_id: target.to_string(),
            }
        }
        other => return Err(format!("unknown action '{other}'")),
    };
    let kind = match raw.kind.as_deref() {
        None => MemoryKind::Fact,
        Some(kind) => {
            MemoryKind::parse(kind).ok_or_else(|| format!("unknown record kind '{kind}'"))?
        }
    };
    let epistemic = Epistemic::parse(&raw.epistemic)
        .ok_or_else(|| format!("unknown epistemic status '{}'", raw.epistemic))?;
    let title = compact_whitespace(&raw.title);
    if title.is_empty() {
        return Err("op has an empty title".to_string());
    }
    let body = raw.body.trim().to_string();
    if body.is_empty() {
        return Err("op has an empty body".to_string());
    }
    if let Some((start, end)) = raw.evidence_range
        && start > end
    {
        return Err(format!("evidence_range [{start}, {end}] is inverted"));
    }
    Ok(ProposedOp {
        action,
        kind,
        title,
        description: compact_whitespace(&raw.description),
        body,
        tags: raw.tags,
        epistemic,
        evidence_range: raw.evidence_range,
    })
}

// ---------------------------------------------------------------------------
// Evidence sources
// ---------------------------------------------------------------------------

/// One transcript message in the evidence window, with its **absolute**
/// position in the persisted session (the `[N]` index the prompt shows and
/// `evidence_range` cites).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMessage {
    pub index: u64,
    pub role: &'static str,
    pub text: String,
}

/// The evidence window: messages `[start_index, end_index)` of the session
/// transcript. `end_index` is the cursor the engine advances to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSlice {
    pub session_key: String,
    pub start_index: u64,
    pub end_index: u64,
    pub messages: Vec<TranscriptMessage>,
    /// Ask 4 refinement (0.7.12): the transcript head revision id at the
    /// instant this slice was read, so distilled records can pin the exact
    /// revision their evidence was extracted from (§7.1). `None` when the
    /// source cannot report a revision.
    pub head_revision: Option<String>,
}

/// How the engine reads persisted transcripts. The real implementation is
/// [`SessionStoreTranscriptSource`]; tests supply scripted slices.
#[async_trait]
pub trait TranscriptSource: Send + Sync {
    /// Messages from `from_index` to the current end of the persisted
    /// transcript; `None` when the session does not exist in the store.
    async fn read(
        &self,
        session_key: &str,
        from_index: u64,
    ) -> Result<Option<TranscriptSlice>, DistillerError>;
}

/// Reads the persistent session store (`SessionStore::load`, the same
/// surface the console history uses). The transcript is durable and
/// positionally indexed, and survives member teardown/retire/reset — which
/// is what makes the detached reset-boundary distillation possible.
pub struct SessionStoreTranscriptSource {
    store: Arc<dyn meerkat::SessionStore>,
}

impl SessionStoreTranscriptSource {
    pub fn new(store: Arc<dyn meerkat::SessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl TranscriptSource for SessionStoreTranscriptSource {
    async fn read(
        &self,
        session_key: &str,
        from_index: u64,
    ) -> Result<Option<TranscriptSlice>, DistillerError> {
        let session_id = meerkat_core::types::SessionId::parse(session_key).map_err(|err| {
            DistillerError::Transcript(format!("invalid session key '{session_key}': {err}"))
        })?;
        let session = self
            .store
            .load(&session_id)
            .await
            .map_err(|err| DistillerError::Transcript(err.to_string()))?;
        let Some(session) = session else {
            return Ok(None);
        };
        // Ask 4 refinement: pin the head revision the extractor is about to
        // read. `transcript_revision()` is fallible serialization; a failure
        // degrades to None ("head at capture time") rather than blocking the
        // distillation.
        let head_revision = session.transcript_revision().ok();
        let all = session.messages();
        let end_index = all.len() as u64;
        let start_index = from_index.min(end_index);
        let messages = all[start_index as usize..]
            .iter()
            .enumerate()
            .filter_map(|(offset, message)| {
                let index = start_index + offset as u64;
                project_message(message).map(|(role, text)| TranscriptMessage {
                    index,
                    role,
                    text: truncate_utf8_boundary(
                        &compact_whitespace(&text),
                        MAX_TRANSCRIPT_MESSAGE_BYTES,
                    ),
                })
            })
            .collect();
        Ok(Some(TranscriptSlice {
            session_key: session_key.to_string(),
            start_index,
            end_index,
            messages,
            head_revision,
        }))
    }
}

/// Text projection of one transcript message. The system prompt is not
/// evidence (it is configuration, and it would dwarf the window); tool
/// results are evidence (operator-visible ground truth) but rendered
/// tersely.
fn project_message(message: &Message) -> Option<(&'static str, String)> {
    match message {
        Message::System(_) => None,
        Message::SystemNotice(notice) => notice
            .body
            .as_deref()
            .map(|body| ("system notice", body.to_string())),
        Message::User(user) => Some(("user", user.text_content())),
        Message::BlockAssistant(assistant) => Some((
            "assistant",
            assistant.text_blocks().collect::<Vec<_>>().join("\n"),
        )),
        Message::ToolResults { results, .. } => {
            let text = results
                .iter()
                .map(|result| meerkat_core::types::text_content(&result.content))
                .collect::<Vec<_>>()
                .join("\n");
            Some(("tool results", text))
        }
    }
}

/// One compaction-discard row harvested from meerkat's session semantic
/// memory (§8.4 trigger (c)).
#[derive(Debug, Clone, PartialEq)]
pub struct DiscardEntry {
    pub content: String,
    /// Source message offsets in the **pre-compaction** transcript
    /// (`MemorySource::Compaction.source_range`).
    pub range: Option<(u64, u64)>,
}

/// Host-side read over a session's compaction discards.
#[async_trait]
pub trait CompactionDiscardSource: Send + Sync {
    async fn read_discards(
        &self,
        session_key: &str,
        limit: usize,
    ) -> Result<Vec<DiscardEntry>, DistillerError>;

    /// Ask 2 (landed in meerkat 0.7.12): reclaim a permanently-orphaned
    /// session's semantic-memory rows via `MemoryStore::drop_scope`. Session
    /// ids rotate under respawn/reset and vanish under delete, stranding rows
    /// that nothing will ever search yet that every future agent build in the
    /// realm re-embeds (defect D3, storage half). Callers MUST run this only
    /// AFTER distillation has preserved what mattered, and ONLY for causes
    /// that permanently abandon the id. Returns the number of rows dropped
    /// (0 when unsupported — the default is a no-op for stores/test doubles
    /// without a drop facility).
    async fn drop_scope(&self, session_key: &str) -> Result<usize, DistillerError> {
        let _ = session_key;
        Ok(0)
    }
}

/// Reads meerkat's own session semantic memory store at
/// `<persistent_state>/memory` (the path `AgentFactory` opens it at,
/// meerkat `factory.rs:5416`). **Read-only host-side access to meerkat's
/// store** — sanctioned by §8.4; this is not a Meta retrieval index and
/// stays outside the §12 bright line, which governs the bundled store.
///
/// Cost note (D3): `HnswMemoryStore::open` rebuilds its in-RAM index from
/// the SQLite rows on every open, so the store is opened lazily per
/// harvest and dropped afterwards.
///
/// Ask 8 (landed in meerkat 0.7.12): the harvest now reads the discard range
/// via `MemoryStore::enumerate_scoped` — exact, provenance-ordered (durable
/// id), paged — instead of the old generous-limit `search` approximation.
/// Enumeration is read-only and additive; it does not touch indexing/search
/// semantics. The per-page `limit` is the total safety ceiling on rows read;
/// if a scope exceeds it the harvest stops and logs (never silently), but at
/// per-session compaction-row counts the full scope fits well within it.
pub struct HnswDiscardSource {
    dir: PathBuf,
}

/// Raw scope rows fetched per `enumerate_scoped` page.
const HARVEST_PAGE_ROWS: usize = 256;

impl HnswDiscardSource {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

#[async_trait]
impl CompactionDiscardSource for HnswDiscardSource {
    async fn read_discards(
        &self,
        session_key: &str,
        limit: usize,
    ) -> Result<Vec<DiscardEntry>, DistillerError> {
        use meerkat_core::memory::{MemoryEnumerationRequest, MemorySearchScope, MemoryStore};

        if !self.dir.is_dir() {
            // No session semantic memory in this deployment: nothing was
            // preserved at compaction, so there is nothing to harvest.
            return Ok(Vec::new());
        }
        let session_id = meerkat_core::types::SessionId::parse(session_key).map_err(|err| {
            DistillerError::Transcript(format!("invalid session key '{session_key}': {err}"))
        })?;
        let dir = self.dir.clone();
        let store = tokio::task::spawn_blocking(move || meerkat_memory::HnswMemoryStore::open(dir))
            .await
            .map_err(|err| DistillerError::Store(err.to_string()))?
            .map_err(|err| DistillerError::Store(err.to_string()))?;
        let scope = MemorySearchScope::for_session(session_id);

        // Ask 8: page the scope exactly, in durable-id order, until it is
        // exhausted or the safety ceiling is hit — no relevance ranking, no
        // silent truncation.
        let mut entries = Vec::new();
        let mut offset = 0usize;
        loop {
            let remaining = limit.saturating_sub(entries.len());
            if remaining == 0 {
                break;
            }
            let page = store
                .enumerate_scoped(
                    &scope,
                    MemoryEnumerationRequest {
                        limit: HARVEST_PAGE_ROWS.min(remaining),
                        offset,
                        source_overlap: None,
                        indexed_after: None,
                    },
                )
                .await
                .map_err(|err| DistillerError::Store(err.to_string()))?;
            for record in page.records {
                entries.push(DiscardEntry {
                    range: record
                        .metadata
                        .source
                        .source_range()
                        .map(|range| (range.start(), range.end())),
                    content: record.content,
                });
            }
            match page.next_offset {
                Some(next) if entries.len() < limit => offset = next,
                Some(_) => {
                    tracing::warn!(
                        session_key,
                        limit,
                        "compaction discard harvest hit its row ceiling; \
                         scope has more rows than were read"
                    );
                    break;
                }
                None => break,
            }
        }
        Ok(entries)
    }

    async fn drop_scope(&self, session_key: &str) -> Result<usize, DistillerError> {
        use meerkat_core::memory::{MemoryOwner, MemoryStore};

        if !self.dir.is_dir() {
            // No session semantic memory in this deployment: nothing to reap.
            return Ok(0);
        }
        let session_id = meerkat_core::types::SessionId::parse(session_key).map_err(|err| {
            DistillerError::Transcript(format!("invalid session key '{session_key}': {err}"))
        })?;
        let dir = self.dir.clone();
        let store = tokio::task::spawn_blocking(move || meerkat_memory::HnswMemoryStore::open(dir))
            .await
            .map_err(|err| DistillerError::Store(err.to_string()))?
            .map_err(|err| DistillerError::Store(err.to_string()))?;
        let receipt = store
            .drop_scope(&MemoryOwner::canonical_session(session_id))
            .await
            .map_err(|err| DistillerError::Store(err.to_string()))?;
        Ok(receipt.dropped_entries)
    }
}

// ---------------------------------------------------------------------------
// Tombstones (prompt guard) — the mechanical backstop is the staged
// validator's tombstone-recreation rejection; this list closes the
// paraphrase gap (§8.4 "never re-create these").
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TombstoneMeta {
    pub title: String,
    pub kind: MemoryKind,
    pub tombstoned_at_ms: u64,
}

#[async_trait]
pub trait TombstoneSource: Send + Sync {
    async fn recent_tombstones(
        &self,
        scope: &MemoryScope,
        since_ms: u64,
        limit: usize,
    ) -> Result<Vec<TombstoneMeta>, AgentMemoryError>;
}

// ---------------------------------------------------------------------------
// Client acquisition (§8.1 — same factory seam as the Selector)
// ---------------------------------------------------------------------------

#[async_trait]
pub trait DistillerClientHandle: Send + Sync {
    async fn client(&self) -> Result<Arc<dyn LlmClient>, DistillerError>;
    fn invalidate(&self);
    /// The credential authority of a factory-backed handle.
    #[cfg(test)]
    fn provider_auth_authority(&self) -> Option<meerkat_core::auth::ProviderAuthPersistenceId> {
        None
    }
}

/// Thin wrapper over the Selector's factory handle: one client-acquisition
/// path for every judgment stage (§8.1 dogma rule 7).
pub struct FactoryDistillerHandle {
    inner: FactoryModelClientHandle,
}

impl FactoryDistillerHandle {
    pub fn new(
        store_path: impl Into<PathBuf>,
        config: meerkat::Config,
        realm: impl Into<String>,
        profile: &DistillerProfile,
    ) -> Self {
        Self {
            inner: FactoryModelClientHandle::for_model(
                store_path,
                config,
                realm,
                &profile.model,
                profile.provider,
            ),
        }
    }

    /// Resolve and persist the Distiller's provider credentials in `persistence`
    /// instead of the process default store (see
    /// [`FactoryModelClientHandle::with_provider_auth_persistence`]).
    #[must_use]
    pub fn with_provider_auth_persistence(
        self,
        persistence: meerkat::ProviderAuthPersistence,
    ) -> Self {
        Self {
            inner: self.inner.with_provider_auth_persistence(persistence),
        }
    }
}

#[async_trait]
impl DistillerClientHandle for FactoryDistillerHandle {
    async fn client(&self) -> Result<Arc<dyn LlmClient>, DistillerError> {
        use crate::memory::factory_handle::{ModelClientError, ModelClientHandle};
        self.inner.client().await.map_err(|err| match err {
            ModelClientError::Auth(msg) => DistillerError::Auth(msg),
            other => DistillerError::Client(other.to_string()),
        })
    }

    fn invalidate(&self) {
        use crate::memory::factory_handle::ModelClientHandle;
        self.inner.invalidate();
    }

    #[cfg(test)]
    fn provider_auth_authority(&self) -> Option<meerkat_core::auth::ProviderAuthPersistenceId> {
        self.inner.provider_auth_authority()
    }
}

// ---------------------------------------------------------------------------
// Prompt rendering + the extraction call
// ---------------------------------------------------------------------------

pub fn render_prompt(
    profile: &DistillerProfile,
    manifest: &[RecordMeta],
    tombstones: &[TombstoneMeta],
    transcript_text: &str,
) -> String {
    let manifest_text = if manifest.is_empty() {
        "(no records)".to_string()
    } else {
        manifest
            .iter()
            .take(profile.params.max_manifest_records)
            .map(crate::memory::factory_handle::render_manifest_row)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let tombstones_text = if tombstones.is_empty() {
        "(none)".to_string()
    } else {
        tombstones
            .iter()
            .take(profile.params.max_tombstones)
            .map(|tombstone| {
                format!(
                    "- [{}] {}",
                    tombstone.kind.as_str(),
                    compact_whitespace(&tombstone.title)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    profile
        .prompt_template
        .replace(MANIFEST_PLACEHOLDER, &manifest_text)
        .replace(TOMBSTONES_PLACEHOLDER, &tombstones_text)
        .replace(TRANSCRIPT_PLACEHOLDER, transcript_text)
}

/// Render the evidence window with `[N]` indices, bounded by the total
/// transcript byte budget (oldest messages drop first — the window's tail
/// is the freshest evidence).
pub fn render_transcript(slice: &TranscriptSlice) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut total = 0usize;
    for message in slice.messages.iter().rev() {
        let line = format!("[{}] {}: {}", message.index, message.role, message.text);
        if total + line.len() + 1 > MAX_TRANSCRIPT_TOTAL_BYTES && !lines.is_empty() {
            lines.push("(earlier messages omitted for budget)".to_string());
            break;
        }
        total += line.len() + 1;
        lines.push(line);
    }
    lines.reverse();
    lines.join("\n")
}

fn render_discards(entries: &[DiscardEntry]) -> String {
    let mut lines: Vec<String> = vec![
        "(compaction-discarded content recovered from session semantic memory; \
         [N-M] are pre-compaction message offsets)"
            .to_string(),
    ];
    let mut total = 0usize;
    for entry in entries {
        let prefix = match entry.range {
            Some((start, end)) => format!("[{start}-{end}]"),
            None => "[?]".to_string(),
        };
        let line = format!(
            "{prefix} {}",
            truncate_utf8_boundary(
                &compact_whitespace(&entry.content),
                MAX_TRANSCRIPT_MESSAGE_BYTES
            )
        );
        if total + line.len() + 1 > MAX_TRANSCRIPT_TOTAL_BYTES && lines.len() > 1 {
            lines.push("(further discards omitted for budget)".to_string());
            break;
        }
        total += line.len() + 1;
        lines.push(line);
    }
    lines.join("\n")
}

/// One bounded completion against the profile's model/params.
///
/// `temperature` is set unconditionally on purpose: whether it reaches the
/// wire is the provider client's decision, which already consults the model's
/// catalog row. See [`crate::memory::steward::complete_text`] for the full
/// reasoning.
async fn complete_text(
    client: &dyn LlmClient,
    profile: &DistillerProfile,
    prompt: String,
) -> Result<String, DistillerError> {
    let request = LlmRequest::new(
        &profile.model,
        vec![Message::User(UserMessage::text(prompt))],
    )
    .with_max_tokens(profile.params.max_output_tokens)
    .with_temperature(profile.params.temperature);
    let mut stream = client.stream(&request);
    let mut assembler = BlockAssembler::new();
    let mut answered = false;
    while let Some(event) = stream.next().await {
        match event.map_err(classify_llm_error)? {
            LlmEvent::AssistantOutput { blocks } => {
                assembler = BlockAssembler::from_final_blocks(blocks);
            }
            LlmEvent::TextDelta { delta, meta } => assembler.on_text_delta(&delta, meta),
            LlmEvent::Done { outcome } => match outcome {
                LlmDoneOutcome::Success { stop_reason, .. } => match completion_stop(stop_reason) {
                    CompletionStop::Answered => {
                        answered = true;
                        break;
                    }
                    // Do NOT discard stop_reason. A MaxTokens stop arrives with a
                    // partial or empty body; returning it as a short answer sends a
                    // truncation into a strict parse, which then blames the model.
                    CompletionStop::Truncated => {
                        return Err(DistillerError::Truncated {
                            max_output_tokens: profile.params.max_output_tokens,
                        });
                    }
                    // Nor is any other unfinished body an answer, however well
                    // it parses: nothing is distilled from a filtered reply.
                    CompletionStop::Incomplete(stop_reason) => {
                        return Err(DistillerError::Incomplete { stop_reason });
                    }
                },
                LlmDoneOutcome::Error { error } => return Err(classify_llm_error(error)),
            },
            LlmEvent::OperationObservationFailed {
                operation_id,
                phase,
            } => {
                // Observation failure does not change the provider's physical outcome.
                tracing::warn!(%operation_id, ?phase, "LLM operation observation failed");
            }
            LlmEvent::ReasoningDelta { .. }
            | LlmEvent::ReasoningComplete { .. }
            | LlmEvent::ToolCallDelta { .. }
            | LlmEvent::ToolCallComplete { .. }
            | LlmEvent::ServerToolContent { .. }
            | LlmEvent::UsageUpdate { .. }
            | LlmEvent::WireLiveness => {}
        }
    }
    // A stream that ends before its terminal event has no answer either.
    if !answered {
        return Err(DistillerError::Client(UNTERMINATED_STREAM.to_string()));
    }
    Ok(meerkat_core::types::BlockAssistantMessage::new(
        assembler.finalize(),
        meerkat_core::StopReason::EndTurn,
    )
    .to_string())
}

fn classify_llm_error(error: LlmError) -> DistillerError {
    match error {
        LlmError::AuthenticationFailed { .. } | LlmError::InvalidApiKey => {
            DistillerError::Auth(error.to_string())
        }
        // Policy stops must not enter the auth-refresh or parse-repair retry paths.
        LlmError::PolicyStop { .. } => DistillerError::Client(error.to_string()),
        other => DistillerError::Client(other.to_string()),
    }
}

/// One extraction call: render, call, strict-parse with exactly one repair
/// round-trip (the Selector's containment shape).
pub async fn extract(
    profile: &DistillerProfile,
    client: &dyn LlmClient,
    manifest: &[RecordMeta],
    tombstones: &[TombstoneMeta],
    transcript_text: &str,
) -> Result<Vec<RawParsedOp>, DistillerError> {
    let prompt = render_prompt(profile, manifest, tombstones, transcript_text);
    let reply = complete_text(client, profile, prompt).await?;
    match parse_ops(&reply) {
        Ok(ops) => Ok(ops),
        Err(first_err) => {
            let repair_prompt = format!(
                "The following reply was supposed to be exactly one JSON array of memory ops \
                 (each {{\"action\": \"remember\" | \"update\", \"target_id\"?, \"kind\", \
                 \"title\", \"description\", \"body\", \"tags\", \"epistemic\", \
                 \"evidence_range\"?}}) but did not parse ({first_err}). Reply with ONLY the \
                 corrected JSON array, no other text.\n\n{reply}"
            );
            let repaired = complete_text(client, profile, repair_prompt).await?;
            parse_ops(&repaired).map_err(DistillerError::Parse)
        }
    }
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

/// Why an extraction window is closing. `Reset` is the only cause whose
/// distillates land wholesale-quarantined (§8.4 — reset is the operator's
/// escape hatch; quarantine preserves the re-dream option where "off" would
/// destroy evidence once session GC lands upstream).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistillCause {
    Interactions,
    Respawn,
    Retire,
    Delete,
    Reset,
    ResumeFallback,
    Compaction,
}

impl DistillCause {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Interactions => "interactions",
            Self::Respawn => "respawn",
            Self::Retire => "retire",
            Self::Delete => "delete",
            Self::Reset => "reset",
            Self::ResumeFallback => "resume_fallback",
            Self::Compaction => "compaction",
        }
    }

    /// §8.4: reset-boundary distillates land `Quarantined` pending steward
    /// review; respawn/retire distill normally (recovery/continuity paths).
    pub fn quarantines_output(&self) -> bool {
        matches!(self, Self::Reset)
    }
}

/// Per-(identity, session) window state: the cursor adaptation of CC's
/// interaction-id mutual-exclusion trick — transcript position instead of
/// interaction ids, because persisted meerkat transcripts are positionally
/// indexed and carry no interaction ids (meerkat-core `session_store.rs`).
#[derive(Default, Clone)]
struct WindowState {
    cursor: u64,
    completed_runs: u32,
    /// The agent's own `memory` tool wrote during this window: skip
    /// extraction for the window and advance the cursor (§8.4 mutual
    /// exclusion — applies to the interaction trigger only; rotation
    /// windows are final and distill regardless, with the pre-injected
    /// manifest as the duplication guard).
    recorder_wrote: bool,
    generation: u64,
    last_run_at: Option<Instant>,
    last_activity_at: Option<Instant>,
    in_flight: bool,
}

/// Outcome of one `distill_now` call, for logs and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistillOutcome {
    Skipped {
        reason: String,
    },
    Completed {
        run_id: String,
        written: usize,
        quarantined: usize,
    },
}

/// Compaction-skip reasons that mean "there was nothing to harvest", not
/// "the harvest failed" — shared with the §8.6 ordering check so the
/// strings cannot drift apart.
pub(crate) const SKIP_NO_DISCARD_SOURCE: &str = "no compaction discard source wired";
pub(crate) const SKIP_NO_DISCARDS: &str = "no compaction discards to harvest";

impl DistillOutcome {
    /// §8.6 ordering invariant input: whether a compaction-cause run left
    /// the boundary's evidence harvested (completed) or provably empty
    /// (nothing to harvest). Budget denials, read failures, and extraction
    /// failures are NOT satisfied — hygiene must not discard material the
    /// distiller never saw.
    pub fn compaction_harvest_satisfied(&self) -> bool {
        match self {
            Self::Completed { .. } => true,
            Self::Skipped { reason } => {
                reason == SKIP_NO_DISCARDS || reason == SKIP_NO_DISCARD_SOURCE
            }
        }
    }
}

/// Post-compaction follow-up hook (§8.6 trigger sequencing): invoked with
/// `(identity, session_key, outcome)` after every compaction-cause
/// distillation attempt, on the same detached task — so a wired Hygienist
/// runs strictly AFTER the distiller's harvest for that boundary.
pub type CompactionFollowUp = Arc<dyn Fn(&str, &str, &DistillOutcome) + Send + Sync>;

/// Compaction-observation hook: invoked synchronously with
/// `(identity, session_key)` the moment a `CompactionCompleted` event is
/// observed — before, and regardless of, the harvest attempt (skipped and
/// budget-denied harvests included). For harvest/hygiene sequencing
/// observers only. Deliberately NOT the recall budget-reset path: distiller
/// sinks register only when the distiller is enabled, so the coordinator's
/// `on_session_compacted` reset is driven by the gateway's always-on
/// member-event sink instead — do not couple it back here.
pub type CompactionObserved = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub struct DistillerEngine {
    profile: DistillerProfile,
    config: DistillerConfig,
    handle: Arc<dyn DistillerClientHandle>,
    provider: Arc<dyn AgentMemoryProvider>,
    tombstones: Arc<dyn TombstoneSource>,
    transcripts: Arc<dyn TranscriptSource>,
    compaction: Option<Arc<dyn CompactionDiscardSource>>,
    tracker: Option<SessionTaintTracker>,
    budget: BackgroundBudget,
    realm: String,
    /// §9.3 timeline sink (optional; tracing stays the fallback surface).
    events: Mutex<Option<Arc<dyn crate::memory::events::MemoryEventSink>>>,
    /// (identity, session_key) → window state.
    windows: Mutex<HashMap<(String, String), WindowState>>,
    /// Test-only override for the pre-rotation timeout.
    pre_rotation_timeout: Duration,
    run_counter: std::sync::atomic::AtomicU64,
    /// §8.6 trigger sequencing: the Hygienist's post-compaction pass runs
    /// through this hook, strictly after the harvest attempt.
    compaction_follow_up: Mutex<Option<CompactionFollowUp>>,
    /// Synchronous compaction-observation hook (see [`CompactionObserved`]).
    compaction_observed: Mutex<Option<CompactionObserved>>,
}

impl DistillerEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        profile: DistillerProfile,
        config: DistillerConfig,
        handle: Arc<dyn DistillerClientHandle>,
        provider: Arc<dyn AgentMemoryProvider>,
        tombstones: Arc<dyn TombstoneSource>,
        transcripts: Arc<dyn TranscriptSource>,
        compaction: Option<Arc<dyn CompactionDiscardSource>>,
        tracker: Option<SessionTaintTracker>,
        realm: impl Into<String>,
    ) -> Self {
        let budget = BackgroundBudget::new(BackgroundBudgetConfig {
            runs_per_window: config.runs_per_hour,
            window: Duration::from_hours(1),
            max_concurrent: crate::memory::guards::DEFAULT_MAX_CONCURRENT,
        });
        Self {
            profile,
            config,
            handle,
            provider,
            tombstones,
            transcripts,
            compaction,
            tracker,
            budget,
            realm: realm.into(),
            events: Mutex::new(None),
            windows: Mutex::new(HashMap::new()),
            pre_rotation_timeout: PRE_ROTATION_TIMEOUT,
            run_counter: std::sync::atomic::AtomicU64::new(0),
            compaction_follow_up: Mutex::new(None),
            compaction_observed: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn with_pre_rotation_timeout(mut self, timeout: Duration) -> Self {
        self.pre_rotation_timeout = timeout;
        self
    }

    /// The credential authority the engine's model-client handle resolves
    /// with.
    #[cfg(test)]
    pub(crate) fn client_provider_auth_authority(
        &self,
    ) -> Option<meerkat_core::auth::ProviderAuthPersistenceId> {
        self.handle.provider_auth_authority()
    }

    /// Wire the §9.3 timeline sink (skipped pre-rotation distillations,
    /// budget denials). Also threads it into the engine's budget guard.
    pub fn set_event_sink(&self, sink: Arc<dyn crate::memory::events::MemoryEventSink>) {
        self.budget.set_event_sink(sink.clone());
        *self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sink);
    }

    pub fn pre_rotation_timeout(&self) -> Duration {
        self.pre_rotation_timeout
    }

    /// Wire the §8.6 post-compaction follow-up (the Hygienist). Invoked
    /// after every compaction-cause distillation attempt with its outcome.
    pub fn set_compaction_follow_up(&self, hook: CompactionFollowUp) {
        *self
            .compaction_follow_up
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(hook);
    }

    /// Wire the synchronous compaction-observation hook (see
    /// [`CompactionObserved`]).
    pub fn set_compaction_observed(&self, hook: CompactionObserved) {
        *self
            .compaction_observed
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(hook);
    }

    /// Fire the compaction-observation hook. Called by the trigger sink at
    /// the earliest session-attributed observation of a compaction, before
    /// the harvest is even spawned.
    fn note_compaction_observed(&self, identity: &str, session_key: &str) {
        let hook = self
            .compaction_observed
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        if let Some(hook) = hook {
            hook(identity, session_key);
        }
    }

    /// Transcript index up to which interaction/rotation distillation has
    /// run for `(identity, session_key)` (§8.4 window cursor). 0 when the
    /// window was never distilled. Read-only: never creates window state.
    pub fn distilled_cursor(&self, identity: &str, session_key: &str) -> u64 {
        self.windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(identity.to_string(), session_key.to_string()))
            .map(|state| state.cursor)
            .unwrap_or(0)
    }

    fn with_window<T>(
        &self,
        identity: &str,
        session_key: &str,
        f: impl FnOnce(&mut WindowState) -> T,
    ) -> T {
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if windows.len() >= MAX_TRACKED_WINDOWS
            && !windows.contains_key(&(identity.to_string(), session_key.to_string()))
            && let Some(oldest) = windows
                .iter()
                .min_by_key(|(_, state)| state.last_activity_at)
                .map(|(key, _)| key.clone())
        {
            windows.remove(&oldest);
        }
        let state = windows
            .entry((identity.to_string(), session_key.to_string()))
            .or_default();
        state.last_activity_at = Some(Instant::now());
        f(state)
    }

    /// Session-context hint from the identity runtime (delivery and
    /// lifecycle paths): binds the continuity generation the session's
    /// `EvidenceRef`s carry. Sessions only ever observed (never delivered
    /// to through the runtime) keep generation 0 — documented coarseness,
    /// resolved when an upstream generation fact exists on the stream.
    pub fn note_session_generation(&self, identity: &str, session_key: &str, generation: u64) {
        self.with_window(identity, session_key, |state| {
            state.generation = generation;
        });
    }

    fn note_recorder_write(&self, identity: &str, session_key: &str) {
        self.with_window(identity, session_key, |state| {
            state.recorder_wrote = true;
        });
    }

    /// Interaction-trigger bookkeeping: returns true when thresholds say an
    /// extraction should run now (the caller spawns it).
    fn note_run_completed(&self, identity: &str, session_key: &str) -> bool {
        let min_interactions = self.config.min_interactions;
        self.with_window(identity, session_key, |state| {
            state.completed_runs += 1;
            if state.in_flight {
                return false;
            }
            if state.completed_runs < min_interactions {
                return false;
            }
            if let Some(last) = state.last_run_at
                && last.elapsed() < Duration::from_secs(MIN_SECONDS_BETWEEN_RUNS)
            {
                return false;
            }
            state.in_flight = true;
            true
        })
    }

    fn identity_scope(&self, identity: &str) -> MemoryScope {
        MemoryScope::Identity {
            realm: self.realm.clone(),
            identity: identity.to_string(),
        }
    }

    fn mint_run_id(&self) -> String {
        let seq = self
            .run_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("distill-{}-{seq}", now_ms())
    }

    /// One extraction run over the window `[cursor, end)` of
    /// `session_key`'s transcript (or the compaction-discard harvest for
    /// [`DistillCause::Compaction`]). Budget-gated, mutually exclusive with
    /// Recorder writes, evidence-taint-aware through the store's write gate.
    pub async fn distill_now(
        self: &Arc<Self>,
        identity: &str,
        session_key: &str,
        cause: DistillCause,
    ) -> DistillOutcome {
        let outcome = self.distill_inner(identity, session_key, cause).await;
        self.with_window(identity, session_key, |state| {
            state.in_flight = false;
        });
        if cause == DistillCause::Compaction {
            let follow_up = self
                .compaction_follow_up
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .clone();
            if let Some(follow_up) = follow_up {
                follow_up(identity, session_key, &outcome);
            }
        }
        match &outcome {
            DistillOutcome::Skipped { reason } => {
                tracing::debug!(
                    identity,
                    session_key,
                    cause = cause.as_str(),
                    reason,
                    "agent memory distiller: run skipped"
                );
            }
            DistillOutcome::Completed {
                run_id,
                written,
                quarantined,
            } => {
                tracing::info!(
                    identity,
                    session_key,
                    cause = cause.as_str(),
                    run_id,
                    written,
                    quarantined,
                    "agent memory distiller: run completed"
                );
            }
        }
        outcome
    }

    async fn distill_inner(
        self: &Arc<Self>,
        identity: &str,
        session_key: &str,
        cause: DistillCause,
    ) -> DistillOutcome {
        let (cursor, recorder_wrote, generation) =
            self.with_window(identity, session_key, |state| {
                (state.cursor, state.recorder_wrote, state.generation)
            });

        // Evidence read comes before the budget gate: an empty window must
        // not burn a budgeted run.
        let (evidence_text, evidence_range, window_end, head_revision) = match cause {
            DistillCause::Compaction => {
                let Some(compaction) = self.compaction.as_ref() else {
                    return DistillOutcome::Skipped {
                        reason: SKIP_NO_DISCARD_SOURCE.to_string(),
                    };
                };
                let entries = match compaction
                    .read_discards(session_key, COMPACTION_HARVEST_LIMIT)
                    .await
                {
                    Ok(entries) => entries,
                    Err(err) => {
                        tracing::warn!(
                            identity,
                            session_key,
                            error = %err,
                            "agent memory distiller: compaction harvest failed"
                        );
                        return DistillOutcome::Skipped {
                            reason: format!("compaction harvest failed: {err}"),
                        };
                    }
                };
                if entries.is_empty() {
                    return DistillOutcome::Skipped {
                        reason: SKIP_NO_DISCARDS.to_string(),
                    };
                }
                let range = discard_evidence_range(&entries);
                // Discards predate the current transcript head (they were
                // evicted at compaction), so there is no head revision to pin.
                (render_discards(&entries), range, None, None)
            }
            _ => {
                let slice = match self.transcripts.read(session_key, cursor).await {
                    Ok(Some(slice)) => slice,
                    Ok(None) => {
                        return DistillOutcome::Skipped {
                            reason: "session not found in the session store".to_string(),
                        };
                    }
                    Err(err) => {
                        tracing::warn!(
                            identity,
                            session_key,
                            error = %err,
                            "agent memory distiller: transcript read failed"
                        );
                        return DistillOutcome::Skipped {
                            reason: format!("transcript read failed: {err}"),
                        };
                    }
                };
                if slice.messages.is_empty() {
                    return DistillOutcome::Skipped {
                        reason: "empty evidence window".to_string(),
                    };
                }
                if cause == DistillCause::Interactions && recorder_wrote {
                    // CC's mutual-exclusion trick: the agent already curated
                    // this window itself; skip and advance the cursor.
                    self.with_window(identity, session_key, |state| {
                        state.cursor = slice.end_index;
                        state.recorder_wrote = false;
                        state.completed_runs = 0;
                    });
                    return DistillOutcome::Skipped {
                        reason: "recorder wrote in window (mutual exclusion)".to_string(),
                    };
                }
                let range = Some((slice.start_index, slice.end_index.saturating_sub(1)));
                let head_revision = slice.head_revision.clone();
                (
                    render_transcript(&slice),
                    range,
                    Some(slice.end_index),
                    head_revision,
                )
            }
        };

        // §8.1 resource guard: consulted before every run, loud on deny.
        let _permit = match self.budget.try_acquire(&self.realm, "distiller") {
            Ok(permit) => permit,
            Err(denied) => {
                return DistillOutcome::Skipped {
                    reason: format!("budget denied: {denied}"),
                };
            }
        };

        // §8.4 reset quarantine: mark the boundary in the tracker so the
        // store's write gate quarantines every record citing this session —
        // the write law stays at the store seam, not in this caller.
        if cause.quarantines_output()
            && let Some(tracker) = self.tracker.as_ref()
        {
            tracker.mark_reset_boundary(session_key);
        }

        let scope = self.identity_scope(identity);
        let manifest = match self
            .provider
            .manifest(std::slice::from_ref(&scope), ManifestTier::Full)
            .await
        {
            Ok(manifest) => manifest,
            Err(err) => {
                tracing::warn!(identity, error = %err, "agent memory distiller: manifest read failed");
                return DistillOutcome::Skipped {
                    reason: format!("manifest read failed: {err}"),
                };
            }
        };
        let tombstones = match self
            .tombstones
            .recent_tombstones(
                &scope,
                now_ms().saturating_sub(TOMBSTONE_LOOKBACK_MS),
                self.profile.params.max_tombstones,
            )
            .await
        {
            Ok(tombstones) => tombstones,
            Err(err) => {
                tracing::warn!(identity, error = %err, "agent memory distiller: tombstone read failed");
                return DistillOutcome::Skipped {
                    reason: format!("tombstone read failed: {err}"),
                };
            }
        };

        let client = match self.handle.client().await {
            Ok(client) => client,
            Err(err) => {
                tracing::warn!(identity, error = %err, "agent memory distiller: client acquisition failed");
                return DistillOutcome::Skipped {
                    reason: format!("client acquisition failed: {err}"),
                };
            }
        };
        let raw_ops = match extract(
            &self.profile,
            &*client,
            &manifest,
            &tombstones,
            &evidence_text,
        )
        .await
        {
            Ok(ops) => ops,
            Err(DistillerError::Auth(message)) => {
                // One re-resolve, mirroring the Selector's auth containment.
                tracing::warn!(error = %message, "distiller auth failure; re-resolving client");
                self.handle.invalidate();
                let retried = match self.handle.client().await {
                    Ok(client) => {
                        extract(
                            &self.profile,
                            &*client,
                            &manifest,
                            &tombstones,
                            &evidence_text,
                        )
                        .await
                    }
                    Err(err) => Err(err),
                };
                match retried {
                    Ok(ops) => ops,
                    Err(err) => {
                        tracing::warn!(identity, error = %err, "agent memory distiller: extraction failed");
                        return DistillOutcome::Skipped {
                            reason: format!("extraction failed: {err}"),
                        };
                    }
                }
            }
            Err(err) => {
                tracing::warn!(identity, error = %err, "agent memory distiller: extraction failed");
                return DistillOutcome::Skipped {
                    reason: format!("extraction failed: {err}"),
                };
            }
        };

        let manifest_ids: Vec<String> = manifest.iter().map(|meta| meta.id.clone()).collect();
        let run_id = self.mint_run_id();
        let author = MemoryAuthor::Distiller {
            run_id: run_id.clone(),
        };
        let mut written = 0usize;
        let mut quarantined = 0usize;
        for raw in raw_ops {
            let op = match validate_op(raw, &manifest_ids) {
                Ok(op) => op,
                Err(reason) => {
                    tracing::warn!(run_id, reason, "agent memory distiller: op dropped");
                    continue;
                }
            };
            let record = self.build_record(
                &op,
                session_key,
                generation,
                evidence_range,
                head_revision.as_deref(),
            );
            let result = match &op.action {
                ProposedAction::Remember => {
                    self.provider
                        .remember_authored(&scope, record, author.clone())
                        .await
                }
                ProposedAction::Update { target_id } => {
                    self.provider
                        .supersede_authored(&scope, target_id, record, author.clone())
                        .await
                }
            };
            match result {
                Ok(receipt) => {
                    written += 1;
                    if matches!(
                        receipt.status,
                        crate::memory::records::RecordStatus::Quarantined { .. }
                    ) {
                        quarantined += 1;
                    }
                }
                Err(err) => {
                    // Includes the staged validator's tombstone-recreation
                    // reject — the mechanical backstop behind the prompt
                    // guard.
                    tracing::warn!(run_id, error = %err, "agent memory distiller: write rejected");
                }
            }
        }

        self.with_window(identity, session_key, |state| {
            // Interaction-window bookkeeping is consumed only by runs that
            // actually covered the window [cursor, end). A compaction-cause
            // harvest reads the discard ledger (window_end=None) — clearing
            // `recorder_wrote` there would defeat the §8.4 mutual exclusion
            // for a window the agent already curated, and zeroing
            // `completed_runs` would silently defer the interaction trigger.
            if let Some(end) = window_end {
                state.cursor = end;
                state.recorder_wrote = false;
                state.completed_runs = 0;
            }
            state.last_run_at = Some(Instant::now());
        });
        DistillOutcome::Completed {
            run_id,
            written,
            quarantined,
        }
    }

    fn build_record(
        &self,
        op: &ProposedOp,
        session_key: &str,
        generation: u64,
        window_range: Option<(u64, u64)>,
        revision: Option<&str>,
    ) -> NewMemoryRecord {
        let mut tags = op.tags.clone();
        if op.epistemic == Epistemic::OperatorSaid
            && !tags.iter().any(|tag| tag == "epistemic:operator_said")
        {
            // Same convention as the Recorder tool: attribution rides as a
            // tag so recall and the steward see the claim's nature.
            tags.push("epistemic:operator_said".to_string());
        }
        // Model-cited range wins when it stays inside the window; a range
        // outside the evidence the model was shown is a hallucinated
        // citation and falls back to the window bounds.
        let range = match (op.evidence_range, window_range) {
            (Some((start, end)), Some((window_start, window_end)))
                if start >= window_start && end <= window_end =>
            {
                Some((start, end))
            }
            (_, window) => window,
        };
        NewMemoryRecord {
            kind: op.kind,
            title: op.title.clone(),
            description: op.description.clone(),
            body: op.body.clone(),
            tags,
            evidence: vec![EvidenceRef {
                session_id: session_key.to_string(),
                generation,
                // Ask 4 refinement: pin the transcript head revision the
                // evidence was read at (None on the compaction path, whose
                // discards predate the current head).
                revision: revision.map(str::to_string),
                range,
            }],
            verification: None,
        }
    }

    /// Pre-rotation hook body (respawn/retire/delete): bounded, best-effort
    /// — rotation proceeds on timeout with a loud skip.
    pub async fn distill_before_rotation(
        self: &Arc<Self>,
        identity: &str,
        session_key: &str,
        cause: DistillCause,
    ) {
        let timeout = self.pre_rotation_timeout;
        match tokio::time::timeout(timeout, self.distill_now(identity, session_key, cause)).await {
            Ok(_) => {}
            Err(_) => {
                tracing::warn!(
                    identity,
                    session_key,
                    cause = cause.as_str(),
                    timeout_ms = timeout.as_millis() as u64,
                    "agent memory distiller: pre-rotation distillation timed out; \
                     rotation proceeds without it"
                );
                if let Some(sink) = self
                    .events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                {
                    sink.emit(
                        crate::memory::events::MemoryTimelineEvent::DistillationTimedOut {
                            identity: identity.to_string(),
                            session_key: session_key.to_string(),
                            cause: cause.as_str().to_string(),
                        },
                    );
                }
            }
        }
    }

    /// Ask 2 GC: reclaim a permanently-orphaned session's semantic-memory
    /// rows after its knowledge has been distilled. Best-effort and bounded —
    /// a rotation must never fail on cleanup. Caller MUST have already run
    /// `distill_before_rotation` for this session and MUST only invoke this
    /// for causes that permanently abandon the session id (respawn/reset mint
    /// a fresh id; delete discards it) — never for resumable retires.
    pub async fn drop_orphaned_session_scope(
        self: &Arc<Self>,
        session_key: &str,
        cause: DistillCause,
    ) {
        let Some(compaction) = self.compaction.as_ref() else {
            return;
        };
        match compaction.drop_scope(session_key).await {
            Ok(0) => {}
            Ok(dropped) => tracing::debug!(
                session_key,
                cause = cause.as_str(),
                dropped,
                "agent memory GC: reclaimed orphaned session semantic-memory rows"
            ),
            Err(err) => tracing::warn!(
                session_key,
                cause = cause.as_str(),
                error = %err,
                "agent memory GC: dropping orphaned session scope failed; \
                 rows remain (re-embed tax persists), rotation proceeds"
            ),
        }
    }

    /// Detached distillation (reset / resume-fallback / compaction): never
    /// on any critical path. The session store outlives the session, so the
    /// read stays valid after teardown.
    pub fn spawn_detached(
        self: &Arc<Self>,
        identity: &str,
        session_key: &str,
        cause: DistillCause,
    ) {
        let engine = self.clone();
        let identity = identity.to_string();
        let session_key = session_key.to_string();
        tokio::spawn(async move {
            engine.distill_now(&identity, &session_key, cause).await;
        });
    }
}

fn discard_evidence_range(entries: &[DiscardEntry]) -> Option<(u64, u64)> {
    let mut bounds: Option<(u64, u64)> = None;
    for entry in entries {
        if let Some((start, end)) = entry.range {
            bounds = Some(match bounds {
                None => (start, end),
                Some((lo, hi)) => (lo.min(start), hi.max(end)),
            });
        }
    }
    bounds
}

// ---------------------------------------------------------------------------
// Observe-stream trigger sink (rides the same member-event observer as the
// taint tracker)
// ---------------------------------------------------------------------------

pub struct DistillerTriggers {
    engine: Arc<DistillerEngine>,
}

impl DistillerTriggers {
    pub fn new(engine: Arc<DistillerEngine>) -> Self {
        Self { engine }
    }
}

impl MemberAgentEventSink for DistillerTriggers {
    fn observe(&self, identity: &str, envelope: &meerkat_core::event::EventEnvelope<AgentEvent>) {
        // Scope keys are LOGICAL identities (task #53). The observer already
        // normalizes its fan-out; this re-normalization is a cheap fixed
        // point that keeps any direct sink caller from re-splitting
        // distiller scopes per incarnation (the downstream app's activation smoke:
        // extraction landed under mk--rt_c... roster ids, invisible to
        // injection and SDK reads).
        let identity = crate::member_comms_id::logical_memory_identity(identity);
        let identity = identity.as_str();
        match &envelope.payload {
            AgentEvent::ToolCallRequested { name, args, .. } if name == MEMORY_TOOL_NAME => {
                let is_write = args
                    .as_value()
                    .get("action")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|action| {
                        matches!(action, "remember" | "update" | "forget" | "propose_to_mob")
                    });
                if is_write && let Some(session) = current_session_of(envelope) {
                    self.engine.note_recorder_write(identity, &session);
                }
            }
            AgentEvent::RunCompleted { session_id, .. } => {
                let session = session_id.to_string();
                if self.engine.note_run_completed(identity, &session) {
                    self.engine
                        .spawn_detached(identity, &session, DistillCause::Interactions);
                }
            }
            AgentEvent::CompactionCompleted { .. } => {
                if let Some(session) = current_session_of(envelope) {
                    // Sequencing observers hear about the boundary here —
                    // synchronously, before the detached harvest, and even
                    // when that harvest is later skipped or budget-denied.
                    self.engine.note_compaction_observed(identity, &session);
                    self.engine
                        .spawn_detached(identity, &session, DistillCause::Compaction);
                } else {
                    tracing::warn!(
                        identity,
                        "agent memory distiller: compaction event without session \
                         attribution; harvest skipped"
                    );
                }
            }
            _ => {}
        }
    }
}

/// Session attribution for non-run-scoped events: the envelope's source
/// identity when it names a session.
fn current_session_of(envelope: &meerkat_core::event::EventEnvelope<AgentEvent>) -> Option<String> {
    match &envelope.source {
        meerkat_core::event::EventSourceIdentity::Session { session_id } => {
            Some(session_id.to_string())
        }
        _ => None,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::redundant_clone, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::identity_first::agent_memory::{
        AgentMemoryRecallRequest, AgentMemoryRecord, AuthoredWriteReceipt,
    };
    use crate::memory::records::RecordStatus;
    use futures::stream;
    use std::sync::Mutex as StdMutex;

    #[tokio::test]
    async fn complete_text_uses_final_assistant_output() {
        use meerkat_core::AssistantBlock;

        for provisional in [None, Some("discarded provisional text")] {
            for blocks in [
                Vec::new(),
                vec![
                    AssistantBlock::Text {
                        text: "[".to_string(),
                        meta: None,
                    },
                    AssistantBlock::Reasoning {
                        text: "not answer text".to_string(),
                        meta: None,
                    },
                    AssistantBlock::Text {
                        text: "]".to_string(),
                        meta: None,
                    },
                ],
            ] {
                let expected = if blocks.is_empty() { "" } else { "[]" };
                let mut events = Vec::new();
                if let Some(delta) = provisional {
                    events.push(LlmEvent::TextDelta {
                        delta: delta.to_string(),
                        meta: None,
                    });
                }
                events.push(LlmEvent::OperationObservationFailed {
                    operation_id: meerkat_core::OperationId::new(),
                    phase: meerkat_core::authorization::OperationObservationPhase::Outcome,
                });
                events.push(LlmEvent::AssistantOutput { blocks });
                events.push(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                });
                let client = meerkat_client::TestClient::new(events);
                assert_eq!(
                    complete_text(
                        &client,
                        &DistillerProfile::embedded_default(),
                        "test".into()
                    )
                    .await
                    .expect("completion"),
                    expected
                );
            }
        }
    }

    #[tokio::test]
    async fn observation_failure_preserves_physical_completion_without_retry() {
        use crate::test_diagnostics::{
            CompletionCase, ObservationClient, PHYSICAL_TEXT, capture_warnings,
        };

        let profile = DistillerProfile::embedded_default();
        for case in CompletionCase::ALL {
            let client = ObservationClient::new(case);
            let (result, log) =
                capture_warnings(complete_text(&client, &profile, "test".into())).await;
            match case {
                CompletionCase::Success => {
                    assert_eq!(result.expect("physical success"), PHYSICAL_TEXT);
                }
                CompletionCase::TerminalError | CompletionCase::StreamError => assert!(
                    matches!(result, Err(DistillerError::Client(message))
                        if message == LlmError::ConnectionReset.to_string()),
                    "partial text must not replace the terminal provider failure",
                ),
                CompletionCase::Truncated => assert!(matches!(
                    result,
                    Err(DistillerError::Truncated { max_output_tokens })
                        if max_output_tokens == profile.params.max_output_tokens
                )),
                CompletionCase::Unterminated => assert!(
                    matches!(result, Err(DistillerError::Client(message))
                        if message == UNTERMINATED_STREAM),
                    "a stream with no terminal event is not an answer",
                ),
            }
            client.assert_observed_once_without_retry(&log);
        }
    }

    #[tokio::test]
    async fn complete_text_final_output_preserves_truncation() {
        let client = meerkat_client::TestClient::new(vec![
            LlmEvent::AssistantOutput { blocks: Vec::new() },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::MaxTokens),
            },
        ]);
        let profile = DistillerProfile::embedded_default();
        assert!(matches!(
            complete_text(&client, &profile, "test".into()).await,
            Err(DistillerError::Truncated { max_output_tokens })
                if max_output_tokens == profile.params.max_output_tokens
        ));
    }

    /// A body the model did not finish is not an answer, however well it
    /// parses: every stop before a complete answer is typed.
    #[tokio::test]
    async fn complete_text_refuses_an_incomplete_stop() {
        use crate::test_diagnostics::{INCOMPLETE_STOPS, stopping_client};

        let profile = DistillerProfile::embedded_default();
        for stop in INCOMPLETE_STOPS {
            let client = stopping_client(ONE_OP_REPLY, stop);
            let result = complete_text(&client, &profile, "test".into()).await;
            assert!(
                matches!(
                    result,
                    Err(DistillerError::Incomplete { stop_reason }) if stop_reason == stop
                ),
                "{stop:?}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn policy_stop_is_not_auth_or_parse_repair() {
        let stop = LlmError::PolicyStop {
            code: "misalignment_policy_violation".to_string(),
            message: "operator review required".to_string(),
        };
        assert!(!stop.is_retryable());
        assert!(matches!(
            classify_llm_error(stop.clone()),
            DistillerError::Client(message)
                if message.contains("misalignment_policy_violation")
        ));
        let client = meerkat_client::TestClient::new(vec![
            LlmEvent::TextDelta {
                delta: "invalid JSON must not be repaired after a policy stop".to_string(),
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error: stop },
            },
        ]);
        assert!(matches!(
            extract(
                &DistillerProfile::embedded_default(),
                &client,
                &[],
                &[],
                "test"
            )
            .await,
            Err(DistillerError::Client(message))
                if message.contains("misalignment_policy_violation")
        ));
    }

    // Ask 2+8 (meerkat 0.7.12): the compaction-discard harvest reads its
    // session's semantic-memory scope EXACTLY via `enumerate_scoped`, and a
    // permanently-orphaned scope is reclaimed via `drop_scope`. This exercises
    // both against a real `HnswMemoryStore` seeded with compaction-discard
    // rows (proving the source_range mapping and the round-trip drop).
    #[tokio::test]
    async fn hnsw_discard_source_enumerates_then_drops_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        use meerkat_core::memory::{
            MemoryIndexRequest, MemoryIndexScope, MemoryMetadata, MemorySource, MemoryStore,
            MessageRange,
        };
        use meerkat_core::types::{MemoryIndexableContent, SessionId};

        let dir = tempfile::tempdir()?;
        let session = SessionId::new();

        // Seed two discarded ranges into meerkat's session semantic memory.
        {
            let store = meerkat_memory::HnswMemoryStore::open(dir.path())?;
            let scope = MemoryIndexScope::for_session(session.clone());
            for (start, end, text) in [
                (0u64, 2u64, "operator prefers terse status updates"),
                (2u64, 5u64, "root cause was an expired upstream TLS cert"),
            ] {
                let request = MemoryIndexRequest::new(
                    scope.clone(),
                    MemoryIndexableContent::Indexable(text.to_string()),
                    MemoryMetadata {
                        session_id: session.clone(),
                        source: MemorySource::Compaction {
                            source_range: MessageRange::new(start, end)?,
                        },
                        indexed_at: meerkat_core::time_compat::SystemTime::now(),
                    },
                )?;
                store.index_scoped(request).await?;
            }
        }

        let source = HnswDiscardSource::new(dir.path());

        // Exact enumeration: both rows, with their source ranges intact.
        let discards = source
            .read_discards(&session.to_string(), COMPACTION_HARVEST_LIMIT)
            .await?;
        assert_eq!(discards.len(), 2, "enumeration must return every scope row");
        assert!(discards.iter().any(|d| d.range == Some((0, 2))));
        assert!(
            discards
                .iter()
                .any(|d| d.content.contains("expired upstream TLS cert"))
        );

        // GC: dropping the scope reclaims exactly those rows; a re-read is empty.
        let dropped = source.drop_scope(&session.to_string()).await?;
        assert_eq!(dropped, 2, "drop_scope must reclaim every seeded row");
        let after = source
            .read_discards(&session.to_string(), COMPACTION_HARVEST_LIMIT)
            .await?;
        assert!(after.is_empty(), "dropped scope must enumerate empty");
        Ok(())
    }

    // -- scripted LLM -------------------------------------------------------

    struct ScriptedLlm {
        replies: StdMutex<Vec<String>>,
        prompts: StdMutex<Vec<String>>,
        stop_reason: meerkat_core::StopReason,
    }

    impl ScriptedLlm {
        fn new(replies: Vec<&str>) -> Self {
            Self {
                replies: StdMutex::new(replies.into_iter().map(str::to_string).collect()),
                prompts: StdMutex::new(Vec::new()),
                stop_reason: meerkat_core::StopReason::EndTurn,
            }
        }

        /// Every reply ends with `stop_reason` instead of an ended turn.
        fn stopping_with(mut self, stop_reason: meerkat_core::StopReason) -> Self {
            self.stop_reason = stop_reason;
            self
        }

        fn prompts(&self) -> Vec<String> {
            self.prompts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait]
    impl LlmClient for ScriptedLlm {
        fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
            let prompt = request
                .messages
                .iter()
                .map(|message| match message {
                    Message::User(user) => user.text_content(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            self.prompts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(prompt);
            let reply = {
                let mut replies = self
                    .replies
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if replies.is_empty() {
                    String::new()
                } else {
                    replies.remove(0)
                }
            };
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: reply,
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(self.stop_reason),
                }),
            ]))
        }

        fn provider(&self) -> Provider {
            Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct ScriptedHandle {
        client: Arc<ScriptedLlm>,
    }

    #[async_trait]
    impl DistillerClientHandle for ScriptedHandle {
        async fn client(&self) -> Result<Arc<dyn LlmClient>, DistillerError> {
            Ok(self.client.clone())
        }
        fn invalidate(&self) {}
    }

    /// Handle whose client acquisition never resolves — the pre-rotation
    /// timeout test's hang.
    struct HangingHandle;

    #[async_trait]
    impl DistillerClientHandle for HangingHandle {
        async fn client(&self) -> Result<Arc<dyn LlmClient>, DistillerError> {
            futures::future::pending().await
        }
        fn invalidate(&self) {}
    }

    // -- scripted provider / sources ---------------------------------------

    #[derive(Default)]
    struct CapturingProvider {
        remembers: StdMutex<Vec<(MemoryScope, NewMemoryRecord, MemoryAuthor)>>,
        supersedes: StdMutex<Vec<(MemoryScope, String, NewMemoryRecord, MemoryAuthor)>>,
        manifest: StdMutex<Vec<RecordMeta>>,
        quarantine_all: bool,
    }

    #[async_trait]
    impl AgentMemoryProvider for CapturingProvider {
        async fn recall(
            &self,
            _request: AgentMemoryRecallRequest,
        ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
            Ok(Vec::new())
        }

        async fn manifest(
            &self,
            _scopes: &[MemoryScope],
            _tier: ManifestTier,
        ) -> Result<Vec<RecordMeta>, AgentMemoryError> {
            Ok(self
                .manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        }

        fn supports_manifest(&self) -> bool {
            true
        }

        async fn remember_authored(
            &self,
            scope: &MemoryScope,
            record: NewMemoryRecord,
            author: MemoryAuthor,
        ) -> Result<AuthoredWriteReceipt, AgentMemoryError> {
            self.remembers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((scope.clone(), record, author));
            Ok(AuthoredWriteReceipt {
                memory_id: "mem-new".to_string(),
                status: if self.quarantine_all {
                    RecordStatus::Quarantined {
                        reason: "test".to_string(),
                    }
                } else {
                    RecordStatus::Active
                },
            })
        }

        async fn supersede_authored(
            &self,
            scope: &MemoryScope,
            prior: &str,
            record: NewMemoryRecord,
            author: MemoryAuthor,
        ) -> Result<AuthoredWriteReceipt, AgentMemoryError> {
            self.supersedes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((scope.clone(), prior.to_string(), record, author));
            Ok(AuthoredWriteReceipt {
                memory_id: "mem-updated".to_string(),
                status: RecordStatus::Active,
            })
        }

        fn supports_authored_writes(&self) -> bool {
            true
        }
    }

    struct StaticTombstones(Vec<TombstoneMeta>);

    #[async_trait]
    impl TombstoneSource for StaticTombstones {
        async fn recent_tombstones(
            &self,
            _scope: &MemoryScope,
            _since_ms: u64,
            _limit: usize,
        ) -> Result<Vec<TombstoneMeta>, AgentMemoryError> {
            Ok(self.0.clone())
        }
    }

    struct StaticTranscript(Option<TranscriptSlice>);

    #[async_trait]
    impl TranscriptSource for StaticTranscript {
        async fn read(
            &self,
            _session_key: &str,
            from_index: u64,
        ) -> Result<Option<TranscriptSlice>, DistillerError> {
            Ok(self.0.clone().map(|mut slice| {
                slice.messages.retain(|message| message.index >= from_index);
                slice.start_index = from_index.max(slice.start_index);
                slice
            }))
        }
    }

    fn slice(messages: &[(&'static str, &str)]) -> TranscriptSlice {
        TranscriptSlice {
            session_key: "sess-1".to_string(),
            start_index: 0,
            end_index: messages.len() as u64,
            messages: messages
                .iter()
                .enumerate()
                .map(|(index, (role, text))| TranscriptMessage {
                    index: index as u64,
                    role,
                    text: text.to_string(),
                })
                .collect(),
            head_revision: Some("rev-head".to_string()),
        }
    }

    fn engine_with(
        replies: Vec<&str>,
        provider: Arc<CapturingProvider>,
        transcript: Option<TranscriptSlice>,
        tombstones: Vec<TombstoneMeta>,
        tracker: Option<SessionTaintTracker>,
        config: DistillerConfig,
    ) -> (Arc<DistillerEngine>, Arc<ScriptedLlm>) {
        engine_with_llm(
            ScriptedLlm::new(replies),
            provider,
            transcript,
            tombstones,
            tracker,
            config,
        )
    }

    fn engine_with_llm(
        llm: ScriptedLlm,
        provider: Arc<CapturingProvider>,
        transcript: Option<TranscriptSlice>,
        tombstones: Vec<TombstoneMeta>,
        tracker: Option<SessionTaintTracker>,
        config: DistillerConfig,
    ) -> (Arc<DistillerEngine>, Arc<ScriptedLlm>) {
        let client = Arc::new(llm);
        let engine = Arc::new(DistillerEngine::new(
            DistillerProfile::embedded_default(),
            config,
            Arc::new(ScriptedHandle {
                client: client.clone(),
            }),
            provider,
            Arc::new(StaticTombstones(tombstones)),
            Arc::new(StaticTranscript(transcript)),
            None,
            tracker,
            "team",
        ));
        (engine, client)
    }

    fn enabled_config() -> DistillerConfig {
        DistillerConfig {
            enabled: true,
            ..DistillerConfig::default()
        }
    }

    const NOOP_REPLY: &str = "[]";
    const ONE_OP_REPLY: &str = r#"[{"action": "remember", "kind": "gotcha",
        "title": "Cargo goes through the wrapper",
        "description": "When running cargo commands in this repo",
        "body": "Operator said: \"always use ./scripts/repo-cargo, never raw cargo\".",
        "tags": [], "epistemic": "operator_said", "evidence_range": [1, 2]}]"#;

    // -- prompt rendering ---------------------------------------------------

    #[test]
    fn prompt_renders_manifest_tombstones_and_transcript() {
        let profile = DistillerProfile::embedded_default();
        let manifest = vec![RecordMeta {
            id: "mem-1".to_string(),
            kind: MemoryKind::Gotcha,
            title: "Wrapper cargo".to_string(),
            description: "When building".to_string(),
            age_days: 2,
            rank: Some(1),
        }];
        let tombstones = vec![TombstoneMeta {
            title: "Operator phone number".to_string(),
            kind: MemoryKind::Fact,
            tombstoned_at_ms: 5,
        }];
        let transcript = render_transcript(&slice(&[
            ("user", "please run the build"),
            ("assistant", "running it now"),
        ]));
        let prompt = render_prompt(&profile, &manifest, &tombstones, &transcript);
        assert!(prompt.contains("- mem-1 [gotcha, saved 2 days ago, rank 1] Wrapper cargo"));
        assert!(prompt.contains("- [fact] Operator phone number"));
        assert!(prompt.contains("[0] user: please run the build"));
        assert!(prompt.contains("[1] assistant: running it now"));
        assert!(!prompt.contains("{{existing_manifest}}"));
        assert!(!prompt.contains("{{recent_tombstones}}"));
        assert!(!prompt.contains("{{transcript}}"));
    }

    #[test]
    fn prompt_renders_empty_sections_honestly() {
        let profile = DistillerProfile::embedded_default();
        let prompt = render_prompt(&profile, &[], &[], "(empty)");
        assert!(prompt.contains("(no records)"));
        assert!(prompt.contains("(none)"));
    }

    // -- parse + repair -----------------------------------------------------

    #[tokio::test]
    async fn extract_parses_ops_and_tolerates_fences() -> Result<(), Box<dyn std::error::Error>> {
        let fenced = format!("```json\n{ONE_OP_REPLY}\n```");
        let client = ScriptedLlm::new(vec![&fenced]);
        let profile = DistillerProfile::embedded_default();
        let ops = extract(&profile, &client, &[], &[], "[0] user: hi").await?;
        assert_eq!(ops.len(), 1);
        assert_eq!(client.prompts().len(), 1, "fenced JSON needs no repair");
        let op = validate_op(ops.into_iter().next().unwrap(), &[]).expect("valid op");
        assert_eq!(op.kind, MemoryKind::Gotcha);
        assert_eq!(op.epistemic, Epistemic::OperatorSaid);
        assert_eq!(op.evidence_range, Some((1, 2)));
        Ok(())
    }

    #[tokio::test]
    async fn extract_repairs_malformed_output_once() -> Result<(), Box<dyn std::error::Error>> {
        let client = ScriptedLlm::new(vec!["Here are my thoughts, no JSON", NOOP_REPLY]);
        let profile = DistillerProfile::embedded_default();
        let ops = extract(&profile, &client, &[], &[], "[0] user: hi").await?;
        assert!(ops.is_empty());
        let prompts = client.prompts();
        assert_eq!(prompts.len(), 2, "exactly one repair round-trip");
        assert!(prompts[1].contains("ONLY the corrected JSON array"));
        Ok(())
    }

    #[tokio::test]
    async fn extract_errors_after_failed_repair() {
        let client = ScriptedLlm::new(vec!["nope", "still nope"]);
        let profile = DistillerProfile::embedded_default();
        let result = extract(&profile, &client, &[], &[], "[0] user: hi").await;
        assert!(
            matches!(result, Err(DistillerError::Parse(_))),
            "{result:?}"
        );
    }

    #[test]
    fn validate_op_enforces_action_kind_epistemic_and_targets() {
        let parse_one = |json: &str| -> Result<ProposedOp, String> {
            let ops = parse_ops(json).expect("parses");
            validate_op(
                ops.into_iter().next().expect("one op"),
                &["mem-1".to_string()],
            )
        };
        // Unknown epistemic status is a per-op reject.
        let err =
            parse_one(r#"[{"action":"remember","title":"t","body":"b","epistemic":"vibes"}]"#)
                .expect_err("unknown epistemic");
        assert!(err.contains("epistemic"), "{err}");
        // Updates must target a manifest record.
        let err = parse_one(
            r#"[{"action":"update","target_id":"mem-9","title":"t","body":"b","epistemic":"observed"}]"#,
        )
        .expect_err("unknown target");
        assert!(err.contains("not in the manifest"), "{err}");
        let ok = parse_one(
            r#"[{"action":"update","target_id":"mem-1","title":"t","body":"b","epistemic":"observed"}]"#,
        )
        .expect("valid update");
        assert_eq!(
            ok.action,
            ProposedAction::Update {
                target_id: "mem-1".to_string()
            }
        );
        // Inverted evidence ranges are hallucinated citations.
        let err = parse_one(
            r#"[{"action":"remember","title":"t","body":"b","epistemic":"observed","evidence_range":[9,2]}]"#,
        )
        .expect_err("inverted range");
        assert!(err.contains("inverted"), "{err}");
    }

    // -- engine: writes, evidence, author -----------------------------------

    #[tokio::test]
    async fn distill_writes_through_authored_seam_with_distiller_author_and_evidence() {
        let provider = Arc::new(CapturingProvider::default());
        let (engine, _client) = engine_with(
            vec![ONE_OP_REPLY],
            provider.clone(),
            Some(slice(&[
                ("user", "use the wrapper"),
                ("assistant", "noted"),
                ("user", "always ./scripts/repo-cargo, never raw cargo"),
            ])),
            Vec::new(),
            None,
            enabled_config(),
        );
        engine.note_session_generation("identity:a", "sess-1", 3);
        let outcome = engine
            .distill_now("identity:a", "sess-1", DistillCause::Retire)
            .await;
        assert!(
            matches!(outcome, DistillOutcome::Completed { written: 1, .. }),
            "{outcome:?}"
        );
        let remembers = provider.remembers.lock().unwrap();
        assert_eq!(remembers.len(), 1);
        let (scope, record, author) = &remembers[0];
        assert_eq!(
            *scope,
            MemoryScope::Identity {
                realm: "team".to_string(),
                identity: "identity:a".to_string()
            }
        );
        assert!(matches!(author, MemoryAuthor::Distiller { .. }));
        assert!(author.is_llm(), "Distiller must classify as an LLM author");
        assert_eq!(record.evidence.len(), 1);
        let evidence = &record.evidence[0];
        assert_eq!(evidence.session_id, "sess-1");
        assert_eq!(evidence.generation, 3);
        // Ask 4 refinement: transcript-path evidence pins the head revision
        // the slice was read at (the StaticTranscript reports "rev-head").
        assert_eq!(evidence.revision.as_deref(), Some("rev-head"));
        assert_eq!(
            evidence.range,
            Some((1, 2)),
            "model-cited range within window"
        );
        assert!(record.tags.contains(&"epistemic:operator_said".to_string()));
    }

    /// A reply the model did not finish writes nothing, even one that parses
    /// into an op: the run is skipped, and its reason and warn line name the
    /// stop.
    #[tokio::test]
    async fn distill_writes_nothing_from_an_incomplete_reply() {
        use crate::test_diagnostics::{INCOMPLETE_STOPS, capture_warnings};

        for stop in INCOMPLETE_STOPS {
            let provider = Arc::new(CapturingProvider::default());
            let (engine, _client) = engine_with_llm(
                ScriptedLlm::new(vec![ONE_OP_REPLY]).stopping_with(stop),
                provider.clone(),
                Some(slice(&[
                    ("user", "use the wrapper"),
                    ("assistant", "noted"),
                    ("user", "always ./scripts/repo-cargo, never raw cargo"),
                ])),
                Vec::new(),
                None,
                enabled_config(),
            );
            engine.note_session_generation("identity:a", "sess-1", 3);
            let (outcome, log) =
                capture_warnings(engine.distill_now("identity:a", "sess-1", DistillCause::Retire))
                    .await;
            let named = format!("{stop:?}");
            assert!(
                matches!(&outcome, DistillOutcome::Skipped { reason } if reason.contains(&named)),
                "{stop:?}: {outcome:?}"
            );
            assert!(
                provider.remembers.lock().unwrap().is_empty(),
                "{stop:?}: nothing is remembered"
            );
            assert!(log.contains(&named), "{stop:?}: {log}");
        }
    }

    #[tokio::test]
    async fn hallucinated_evidence_range_falls_back_to_window_bounds() {
        let reply = r#"[{"action": "remember", "kind": "fact", "title": "t",
            "description": "", "body": "b", "tags": [],
            "epistemic": "observed", "evidence_range": [90, 95]}]"#;
        let provider = Arc::new(CapturingProvider::default());
        let (engine, _client) = engine_with(
            vec![reply],
            provider.clone(),
            Some(slice(&[("user", "a"), ("assistant", "b")])),
            Vec::new(),
            None,
            enabled_config(),
        );
        engine
            .distill_now("identity:a", "sess-1", DistillCause::Retire)
            .await;
        let remembers = provider.remembers.lock().unwrap();
        assert_eq!(remembers[0].1.evidence[0].range, Some((0, 1)));
    }

    #[tokio::test]
    async fn update_ops_supersede_the_target_record() {
        let reply = r#"[{"action": "update", "target_id": "mem-1", "kind": "gotcha",
            "title": "t", "description": "", "body": "b", "tags": [],
            "epistemic": "observed"}]"#;
        let provider = Arc::new(CapturingProvider::default());
        provider.manifest.lock().unwrap().push(RecordMeta {
            id: "mem-1".to_string(),
            kind: MemoryKind::Gotcha,
            title: "old".to_string(),
            description: String::new(),
            age_days: 1,
            rank: None,
        });
        let (engine, _client) = engine_with(
            vec![reply],
            provider.clone(),
            Some(slice(&[("user", "a")])),
            Vec::new(),
            None,
            enabled_config(),
        );
        engine
            .distill_now("identity:a", "sess-1", DistillCause::Retire)
            .await;
        let supersedes = provider.supersedes.lock().unwrap();
        assert_eq!(supersedes.len(), 1);
        assert_eq!(supersedes[0].1, "mem-1");
        assert!(provider.remembers.lock().unwrap().is_empty());
    }

    // -- cursor mutual exclusion -------------------------------------------

    #[tokio::test]
    async fn recorder_write_in_window_skips_extraction_and_advances_cursor() {
        let provider = Arc::new(CapturingProvider::default());
        let (engine, client) = engine_with(
            vec![ONE_OP_REPLY, ONE_OP_REPLY],
            provider.clone(),
            Some(slice(&[("user", "a"), ("assistant", "b")])),
            Vec::new(),
            None,
            enabled_config(),
        );
        engine.note_recorder_write("identity:a", "sess-1");
        let outcome = engine
            .distill_now("identity:a", "sess-1", DistillCause::Interactions)
            .await;
        assert!(
            matches!(&outcome, DistillOutcome::Skipped { reason } if reason.contains("mutual exclusion")),
            "{outcome:?}"
        );
        assert!(
            client.prompts().is_empty(),
            "no LLM call in a skipped window"
        );
        assert!(provider.remembers.lock().unwrap().is_empty());

        // The cursor advanced past the curated window: the next run sees an
        // empty window, not the same messages again.
        let outcome = engine
            .distill_now("identity:a", "sess-1", DistillCause::Interactions)
            .await;
        assert!(
            matches!(&outcome, DistillOutcome::Skipped { reason } if reason.contains("empty")),
            "{outcome:?}"
        );

        // Rotation causes are NOT excluded: retire distills the window even
        // after a recorder write (window is final; manifest guards dupes).
        let (engine, _client) = engine_with(
            vec![ONE_OP_REPLY],
            provider.clone(),
            Some(slice(&[("user", "a")])),
            Vec::new(),
            None,
            enabled_config(),
        );
        engine.note_recorder_write("identity:a", "sess-1");
        let outcome = engine
            .distill_now("identity:a", "sess-1", DistillCause::Retire)
            .await;
        assert!(
            matches!(outcome, DistillOutcome::Completed { .. }),
            "{outcome:?}"
        );
    }

    // -- trigger throttling + budget guard -----------------------------------

    #[test]
    fn interaction_trigger_respects_min_interactions_and_in_flight() {
        let provider = Arc::new(CapturingProvider::default());
        let (engine, _client) = engine_with(
            vec![],
            provider,
            None,
            Vec::new(),
            None,
            DistillerConfig {
                enabled: true,
                min_interactions: 3,
                ..DistillerConfig::default()
            },
        );
        assert!(!engine.note_run_completed("identity:a", "sess-1"));
        assert!(!engine.note_run_completed("identity:a", "sess-1"));
        assert!(
            engine.note_run_completed("identity:a", "sess-1"),
            "third run trips"
        );
        // in_flight set by the trip: further completions coalesce.
        assert!(!engine.note_run_completed("identity:a", "sess-1"));
    }

    #[tokio::test]
    async fn budget_guard_skips_runs_beyond_the_window_cap() {
        let provider = Arc::new(CapturingProvider::default());
        let (engine, _client) = engine_with(
            vec![NOOP_REPLY, NOOP_REPLY],
            provider,
            Some(slice(&[("user", "a")])),
            Vec::new(),
            None,
            DistillerConfig {
                enabled: true,
                runs_per_hour: 1,
                ..DistillerConfig::default()
            },
        );
        let first = engine
            .distill_now("identity:a", "sess-1", DistillCause::Retire)
            .await;
        assert!(
            matches!(first, DistillOutcome::Completed { .. }),
            "{first:?}"
        );
        // Fresh window content so the second attempt reaches the guard.
        engine.with_window("identity:a", "sess-1", |state| state.cursor = 0);
        let second = engine
            .distill_now("identity:a", "sess-1", DistillCause::Retire)
            .await;
        assert!(
            matches!(&second, DistillOutcome::Skipped { reason } if reason.contains("budget denied")),
            "{second:?}"
        );
    }

    // -- trigger sink over the observe stream --------------------------------

    fn envelope(
        session: &meerkat_core::types::SessionId,
        payload: AgentEvent,
    ) -> meerkat_core::event::EventEnvelope<AgentEvent> {
        meerkat_core::event::EventEnvelope {
            event_id: Default::default(),
            source: meerkat_core::event::EventSourceIdentity::Session {
                session_id: session.clone(),
            },
            seq: 0,
            mob_id: None,
            timestamp_ms: 0,
            payload,
        }
    }

    #[tokio::test]
    async fn trigger_sink_marks_memory_tool_writes_but_not_reads() {
        let provider = Arc::new(CapturingProvider::default());
        let session = meerkat_core::types::SessionId::new();
        let session_key = session.to_string();
        let make = |transcript: TranscriptSlice| {
            engine_with(
                vec![NOOP_REPLY],
                provider.clone(),
                Some(transcript),
                Vec::new(),
                None,
                enabled_config(),
            )
        };
        let tool_call = |action: &str| AgentEvent::ToolCallRequested {
            id: "t-1".to_string(),
            name: MEMORY_TOOL_NAME.to_string(),
            args: meerkat_core::event::ToolCallArguments::from_value(serde_json::json!({
                "action": action, "title": "t", "body": "b"
            }))
            .expect("object args"),
        };

        // A memory WRITE trips the mutual-exclusion flag...
        let mut transcript = slice(&[("user", "a")]);
        transcript.session_key = session_key.clone();
        let (engine, _client) = make(transcript.clone());
        let sink = DistillerTriggers::new(engine.clone());
        sink.observe("identity:a", &envelope(&session, tool_call("remember")));
        let outcome = engine
            .distill_now("identity:a", &session_key, DistillCause::Interactions)
            .await;
        assert!(
            matches!(&outcome, DistillOutcome::Skipped { reason } if reason.contains("mutual exclusion")),
            "{outcome:?}"
        );

        // ...a memory READ (recall) does not.
        let (engine, _client) = make(transcript);
        let sink = DistillerTriggers::new(engine.clone());
        sink.observe("identity:a", &envelope(&session, tool_call("recall")));
        let outcome = engine
            .distill_now("identity:a", &session_key, DistillCause::Interactions)
            .await;
        assert!(
            matches!(outcome, DistillOutcome::Completed { .. }),
            "{outcome:?}"
        );
    }

    // -- reset quarantine -----------------------------------------------------

    #[tokio::test]
    async fn reset_cause_marks_boundary_so_evidence_quarantines() {
        let tracker = SessionTaintTracker::new(Default::default());
        let provider = Arc::new(CapturingProvider::default());
        let (engine, _client) = engine_with(
            vec![ONE_OP_REPLY],
            provider,
            Some(slice(&[("user", "a"), ("assistant", "b"), ("user", "c")])),
            Vec::new(),
            Some(tracker.clone()),
            enabled_config(),
        );
        assert!(tracker.evidence_quarantine_reason("sess-1").is_none());
        engine
            .distill_now("identity:a", "sess-1", DistillCause::Reset)
            .await;
        let reason = tracker
            .evidence_quarantine_reason("sess-1")
            .expect("reset boundary marked before writes");
        assert!(reason.contains("reset"), "{reason}");
    }

    // -- pre-rotation timeout --------------------------------------------------

    #[tokio::test]
    async fn pre_rotation_distillation_never_blocks_rotation() {
        let provider = Arc::new(CapturingProvider::default());
        let engine = Arc::new(
            DistillerEngine::new(
                DistillerProfile::embedded_default(),
                enabled_config(),
                Arc::new(HangingHandle),
                provider,
                Arc::new(StaticTombstones(Vec::new())),
                Arc::new(StaticTranscript(Some(slice(&[("user", "a")])))),
                None,
                None,
                "team",
            )
            .with_pre_rotation_timeout(Duration::from_millis(50)),
        );
        let started = Instant::now();
        engine
            .distill_before_rotation("identity:a", "sess-1", DistillCause::Respawn)
            .await;
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "pre-rotation hook must return at the timeout, not hang"
        );
    }

    // -- profile ----------------------------------------------------------------

    #[test]
    fn embedded_prompt_matches_calibration_bundle() -> Result<(), Box<dyn std::error::Error>> {
        let bundle = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/memory-evals/prompts/distiller-v0.md");
        if !bundle.is_file() {
            return Ok(());
        }
        let text = std::fs::read_to_string(bundle)?;
        assert_eq!(
            text, EMBEDDED_PROMPT_V0,
            "tests/memory-evals/prompts/distiller-v0.md and \
             src/memory/distiller_prompt_v0.md have drifted"
        );
        Ok(())
    }

    #[test]
    fn embedded_default_profile_validates_and_names_a_catalog_model() {
        let profile = DistillerProfile::embedded_default();
        profile.validate().expect("embedded profile must validate");
        assert_eq!(
            meerkat_models::infer_provider(&profile.model),
            Some(profile.provider),
            "embedded default model must resolve in the catalog"
        );
    }

    #[test]
    fn external_profile_loads_from_evals_layout() -> Result<(), Box<dyn std::error::Error>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/memory-evals/profiles/distiller-v0.toml");
        if !path.is_file() {
            return Ok(());
        }
        let profile = DistillerProfile::load(&path)?;
        assert_eq!(profile.stage, "distiller");
        assert_eq!(profile.prompt_template, EMBEDDED_PROMPT_V0);
        Ok(())
    }

    #[test]
    fn model_override_is_fail_loud() {
        let profile = DistillerProfile::embedded_default();
        assert!(profile.clone().with_model_override("not-a-model").is_err());
        assert!(profile.clone().with_model_override("  ").is_err());
        let overridden = profile
            .with_model_override("claude-haiku-4-5")
            .expect("catalog model accepted");
        assert_eq!(overridden.model, "claude-haiku-4-5");
    }

    /// The budget override is the reachable half of the output-ceiling fix: a
    /// zero budget is a typed error, not an extraction that silently proposes
    /// nothing.
    #[test]
    fn max_output_tokens_override_is_fail_loud() {
        let profile = DistillerProfile::embedded_default();
        assert!(profile.clone().with_max_output_tokens(0).is_err());
        let raised = profile
            .with_max_output_tokens(32_768)
            .expect("nonzero budget accepted");
        assert_eq!(raised.params.max_output_tokens, 32_768);
    }

    // -- §8.6 seams: harvest classification, follow-up hook, cursor ----------

    #[test]
    fn compaction_harvest_satisfaction_classifies_skip_reasons() {
        assert!(
            DistillOutcome::Completed {
                run_id: "r".to_string(),
                written: 0,
                quarantined: 0,
            }
            .compaction_harvest_satisfied()
        );
        assert!(
            DistillOutcome::Skipped {
                reason: SKIP_NO_DISCARDS.to_string(),
            }
            .compaction_harvest_satisfied()
        );
        assert!(
            DistillOutcome::Skipped {
                reason: SKIP_NO_DISCARD_SOURCE.to_string(),
            }
            .compaction_harvest_satisfied()
        );
        for reason in [
            "budget denied: window budget exhausted (2/2 runs)",
            "compaction harvest failed: io",
            "extraction failed: parse",
        ] {
            assert!(
                !DistillOutcome::Skipped {
                    reason: reason.to_string(),
                }
                .compaction_harvest_satisfied(),
                "{reason}"
            );
        }
    }

    #[tokio::test]
    async fn compaction_follow_up_fires_with_outcome_and_cursor_is_readable() {
        let provider = Arc::new(CapturingProvider::default());
        // No compaction source wired ⇒ the run skips with the benign
        // "nothing to harvest" reason, and the follow-up still fires.
        let (engine, _client) = engine_with(
            vec![NOOP_REPLY],
            provider,
            Some(slice(&[("user", "hello")])),
            Vec::new(),
            None,
            enabled_config(),
        );
        let seen: Arc<StdMutex<Vec<(String, String, bool)>>> = Arc::new(StdMutex::new(Vec::new()));
        let sink = seen.clone();
        engine.set_compaction_follow_up(Arc::new(move |identity, session, outcome| {
            sink.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    identity.to_string(),
                    session.to_string(),
                    outcome.compaction_harvest_satisfied(),
                ));
        }));
        engine
            .distill_now("identity:a", "sess-1", DistillCause::Compaction)
            .await;
        // Non-compaction causes never fire the hook.
        engine
            .distill_now("identity:a", "sess-1", DistillCause::Interactions)
            .await;
        let seen = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            seen,
            vec![("identity:a".to_string(), "sess-1".to_string(), true)]
        );
        // The §8.4 window cursor is readable without creating state.
        assert_eq!(engine.distilled_cursor("identity:never", "sess-x"), 0);
        engine.with_window("identity:a", "sess-1", |state| state.cursor = 7);
        assert_eq!(engine.distilled_cursor("identity:a", "sess-1"), 7);
    }

    struct StaticDiscards(Vec<DiscardEntry>);

    #[async_trait]
    impl CompactionDiscardSource for StaticDiscards {
        async fn read_discards(
            &self,
            _session_key: &str,
            _limit: usize,
        ) -> Result<Vec<DiscardEntry>, DistillerError> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn compaction_harvest_preserves_interaction_window_state() {
        let provider = Arc::new(CapturingProvider::default());
        let client = Arc::new(ScriptedLlm::new(vec![ONE_OP_REPLY]));
        let engine = Arc::new(DistillerEngine::new(
            DistillerProfile::embedded_default(),
            enabled_config(),
            Arc::new(ScriptedHandle { client }),
            provider,
            Arc::new(StaticTombstones(Vec::new())),
            Arc::new(StaticTranscript(Some(slice(&[
                ("user", "use the wrapper"),
                ("assistant", "noted"),
            ])))),
            Some(Arc::new(StaticDiscards(vec![DiscardEntry {
                content: "discarded: wrapper reminder".to_string(),
                range: Some((0, 5)),
            }]))),
            None,
            "team",
        ));
        // The agent's Recorder wrote in the interaction window, and one run
        // completed toward the interaction trigger.
        engine.note_recorder_write("identity:a", "sess-1");
        assert!(!engine.note_run_completed("identity:a", "sess-1"));

        // A compaction-cause harvest COMPLETES over the discard ledger…
        let outcome = engine
            .distill_now("identity:a", "sess-1", DistillCause::Compaction)
            .await;
        assert!(
            matches!(outcome, DistillOutcome::Completed { written: 1, .. }),
            "{outcome:?}"
        );

        // …without consuming the interaction window it never covered: the
        // cursor stays put, the §8.4 mutual-exclusion flag survives, and the
        // interaction counter is not silently deferred.
        let (cursor, recorder_wrote, completed_runs) =
            engine.with_window("identity:a", "sess-1", |state| {
                (state.cursor, state.recorder_wrote, state.completed_runs)
            });
        assert_eq!(cursor, 0, "compaction never advances the transcript cursor");
        assert!(
            recorder_wrote,
            "compaction-cause harvests must not clear the recorder \
             mutual-exclusion flag for a window they did not distill"
        );
        assert_eq!(
            completed_runs, 1,
            "compaction-cause harvests must not zero the interaction counter"
        );

        // The next interactions-cause run therefore still skips the window
        // the agent already curated itself.
        let outcome = engine
            .distill_now("identity:a", "sess-1", DistillCause::Interactions)
            .await;
        assert!(
            matches!(&outcome, DistillOutcome::Skipped { reason } if reason.contains("mutual exclusion")),
            "{outcome:?}"
        );
    }

    // Task #53 regression: the trigger sink keys the distiller by the
    // LOGICAL identity, not the mob-plane id it is observed under.
    //
    // Identity-first members now roster under the encoded DURABLE identity, so
    // the roster id itself no longer varies per generation. The normalization
    // this test guards still matters, because generated runtime aliases
    // (mk--rt_c..., one per incarnation) remain a real observed shape in
    // BINDING and EVENT detail - and that is what the seeds below feed. Before
    // the fix every generation's extraction landed in its own scope, disjoint
    // from the scope the SDK, injection, and the recorder key on.
    #[tokio::test]
    async fn trigger_sink_normalizes_roster_ids_to_the_logical_identity_across_generations() {
        let provider = Arc::new(CapturingProvider::default());
        let (engine, _client) = engine_with(
            vec![NOOP_REPLY],
            provider,
            Some(slice(&[("user", "hello")])),
            Vec::new(),
            None,
            enabled_config(),
        );
        let seen: Arc<StdMutex<Vec<(String, String)>>> = Arc::new(StdMutex::new(Vec::new()));
        let observed = seen.clone();
        engine.set_compaction_observed(Arc::new(move |identity, session| {
            observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((identity.to_string(), session.to_string()));
        }));

        let session = meerkat_core::types::SessionId::new();
        let sink = DistillerTriggers::new(engine.clone());
        // Two respawn generations of the SAME durable identity, observed
        // under their encoded roster ids.
        for generation in ["rt:identity:lead-1:0", "rt:identity:lead-1:1"] {
            let roster_id = crate::member_comms_id::mob_member_id_str(generation).into_owned();
            assert!(roster_id.starts_with("mk--"), "{roster_id}");
            sink.observe(
                &roster_id,
                &envelope(
                    &session,
                    AgentEvent::CompactionCompleted {
                        summary_tokens: 10,
                        messages_before: 20,
                        messages_after: 2,
                        summary_source: None,
                        estimated_history_tokens_after: None,
                    },
                ),
            );
        }
        let identities: Vec<String> = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(identity, _)| identity.clone())
            .collect();
        assert_eq!(
            identities,
            vec!["identity:lead-1".to_string(), "identity:lead-1".to_string()],
            "both generations must key the ONE logical scope"
        );
    }

    #[tokio::test]
    async fn compaction_observed_hook_fires_at_observation_even_without_discards() {
        let provider = Arc::new(CapturingProvider::default());
        // No discard source wired: the harvest itself will skip — the
        // observation hook must fire regardless (sequencing observers must
        // not depend on the harvest being viable).
        let (engine, _client) = engine_with(
            vec![NOOP_REPLY],
            provider,
            Some(slice(&[("user", "hello")])),
            Vec::new(),
            None,
            enabled_config(),
        );
        let seen: Arc<StdMutex<Vec<(String, String)>>> = Arc::new(StdMutex::new(Vec::new()));
        let observed = seen.clone();
        engine.set_compaction_observed(Arc::new(move |identity, session| {
            observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((identity.to_string(), session.to_string()));
        }));

        let session = meerkat_core::types::SessionId::new();
        let sink = DistillerTriggers::new(engine.clone());
        sink.observe(
            "identity:a",
            &envelope(
                &session,
                AgentEvent::CompactionCompleted {
                    summary_tokens: 10,
                    messages_before: 20,
                    messages_after: 2,
                    summary_source: None,
                    estimated_history_tokens_after: None,
                },
            ),
        );
        // Fired synchronously at observation time — no detached harvest to
        // await.
        assert_eq!(
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            vec![("identity:a".to_string(), session.to_string())]
        );

        // Non-compaction events never fire it.
        sink.observe(
            "identity:a",
            &envelope(
                &session,
                AgentEvent::RunCompleted {
                    assistant_message_id: None,
                    identity: Default::default(),
                    session_id: session.clone(),
                    result: "done".to_string(),
                    structured_output: None,
                    extraction_required: false,
                    usage: Default::default(),
                    terminal_cause_kind: None,
                },
            ),
        );
        assert_eq!(
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1
        );
    }
}
