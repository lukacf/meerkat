//! Hygienist — context curation at boundaries (§8.6).
//!
//! The user-visible half of "dreaming": an off-turn LLM pass that keeps a
//! long-lived embodiment's context semantically pristine by pruning dead
//! tool results and collapsing repeated scaffolding, while preserving
//! decisions and their rationale. Everything applies through meerkat's
//! audited same-session transcript revisions (`SessionServiceTranscriptEditExt::
//! rewrite_session_transcript` — session identity unchanged, originals
//! restorable, every commit audited), reached host-side through the
//! [`TranscriptRevisionSeam`]. Mid-turn sessions are refused by meerkat's
//! own `TranscriptEditRunningBehavior::Reject` default — fail-closed.
//!
//! The judgment (what is dead, what is scaffolding, what is a decision) is
//! the LLM's; everything structural is deterministic validator law here:
//! range bounds and role legality, the §8.6 quarantine hard-block (spans
//! referenced by `Quarantined` records are untouchable until steward review
//! completes — an attacker must not steer the Hygienist into pruning the
//! tool output documenting the attack), active-record span flags as audit
//! events, and the §8.4 ordering invariant (distillation for the affected
//! window must have run first — post-compaction runs are sequenced behind
//! the distiller's harvest through its follow-up hook; on-demand runs
//! consult the distiller's window cursor).
//!
//! Curation vocabulary is deliberately narrower than a free-form rewrite:
//! `prune_tool_results` stubs the payload of tool-result messages in place
//! (the tool-call/result pairing the provider APIs require survives), and
//! `collapse` replaces a contiguous run of non-tool messages with one typed
//! system notice. Deleting arbitrary messages is not expressible — the
//! validator, not the prompt, is what makes tool-pairing breakage
//! impossible. The system prompt and a detached job's completion entry
//! (`HygieneRole::BackgroundJobResult`) are untouchable by the same law.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;

use meerkat_client::{BlockAssembler, LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::event::AgentEvent;
use meerkat_core::{Message, Provider, SystemNoticeKind, SystemNoticeMessage, UserMessage};

use crate::identity_first::agent_memory::{compact_whitespace, truncate_utf8_boundary};
use crate::memory::capabilities::StewardStore;
use crate::memory::completion::{CompletionStop, UNTERMINATED_STREAM, completion_stop};
use crate::memory::distiller::{CompactionFollowUp, DistillOutcome, DistillerEngine};
use crate::memory::events::{MemoryEventSink, MemoryTimelineEvent};
use crate::memory::factory_handle::FactoryModelClientHandle;
use crate::memory::guards::{BackgroundBudget, BackgroundBudgetConfig};
use crate::memory::records::{ManifestTier, MemoryScope, RecordStatus};
use crate::memory::taint::MemberAgentEventSink;

/// Embedded prompt bundle (crate-local copy of
/// `tests/memory-evals/prompts/hygienist-v0.md`; a unit test enforces byte
/// equality so the calibration artifact and the shipped default cannot
/// drift — same pattern as the other stages).
pub const EMBEDDED_PROMPT_V0: &str = include_str!("hygienist_prompt_v0.md");

const TRANSCRIPT_PLACEHOLDER: &str = "{{transcript}}";
const PROTECTED_RANGES_PLACEHOLDER: &str = "{{protected_ranges}}";

/// Default hygiene runs per realm per day (§8.6 boundary cadence; the
/// concrete number is a §16-class open question, this is the conservative
/// starting point).
pub const DEFAULT_RUNS_PER_DAY: u32 = 2;
/// Per-message and total byte bounds on the rendered transcript.
const MAX_TRANSCRIPT_MESSAGE_BYTES: usize = 2 * 1024;
const MAX_TRANSCRIPT_TOTAL_BYTES: usize = 48 * 1024;
/// Output budget for the structured op list, overridable per deployment via
/// [`HygienistConfig::max_output_tokens`].
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
/// Quarantine rows consulted per pass for the §8.6 hard-block.
const SPAN_QUARANTINE_LIMIT: usize = 256;
/// Cap on the collapse replacement note.
const MAX_COLLAPSE_REPLACEMENT_BYTES: usize = 512;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum HygienistError {
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
    /// No revision is applied from the partial body, however well it parses.
    Incomplete {
        stop_reason: meerkat_core::StopReason,
    },
    Parse(String),
    Seam(String),
    Spans(String),
}

impl std::fmt::Display for HygienistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Profile(msg) => write!(f, "hygienist profile error: {msg}"),
            Self::Auth(msg) => write!(f, "hygienist auth error: {msg}"),
            Self::Client(msg) => write!(f, "hygienist client error: {msg}"),
            Self::Truncated { max_output_tokens } => write!(
                f,
                "hygienist response hit the output ceiling of {max_output_tokens} tokens and was \
                 truncated; raise it via the hygienist config's max_output_tokens (models spend this \
                 same budget on thinking/reasoning tokens as well as the answer)"
            ),
            Self::Incomplete { stop_reason } => write!(
                f,
                "hygienist response stopped with {stop_reason:?} before a complete answer; \
                 no revision is applied from it"
            ),
            Self::Parse(msg) => write!(f, "hygienist parse error: {msg}"),
            Self::Seam(msg) => write!(f, "hygienist revision seam error: {msg}"),
            Self::Spans(msg) => write!(f, "hygienist span source error: {msg}"),
        }
    }
}

impl std::error::Error for HygienistError {}

// ---------------------------------------------------------------------------
// Calibration profile (§11)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct HygienistParams {
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
}

fn default_temperature() -> f32 {
    0.0
}
fn default_max_output_tokens() -> u32 {
    DEFAULT_MAX_OUTPUT_TOKENS
}

impl Default for HygienistParams {
    fn default() -> Self {
        Self {
            temperature: default_temperature(),
            max_output_tokens: default_max_output_tokens(),
        }
    }
}

/// A loaded hygienist calibration profile (§11), prompt template resolved.
#[derive(Debug, Clone)]
pub struct HygienistProfile {
    pub stage: String,
    pub version: String,
    pub model: String,
    pub provider: Provider,
    pub prompt_bundle: String,
    pub prompt_template: String,
    pub params: HygienistParams,
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
    params: Option<HygienistParams>,
}

impl HygienistProfile {
    /// The embedded default: `tests/memory-evals/profiles/hygienist-v0.toml` with
    /// the prompt compiled in. The model tier is a calibration decision
    /// (§11); `hygienist.model` overrides per-deployment.
    pub fn embedded_default() -> Self {
        Self {
            stage: "hygienist".to_string(),
            version: "0".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            provider: Provider::Anthropic,
            prompt_bundle: "prompts/hygienist-v0.md".to_string(),
            prompt_template: EMBEDDED_PROMPT_V0.to_string(),
            params: HygienistParams::default(),
        }
    }

    /// Replace the profile's model (the config-block override). Fail-loud:
    /// the model must resolve in the catalog.
    pub fn with_model_override(mut self, model: &str) -> Result<Self, HygienistError> {
        let model = model.trim();
        if model.is_empty() {
            return Err(HygienistError::Profile(
                "hygienist model override must not be empty".to_string(),
            ));
        }
        self.provider = meerkat_models::infer_provider(model).ok_or_else(|| {
            HygienistError::Profile(format!(
                "hygienist model override '{model}' is not in the model catalog"
            ))
        })?;
        self.model = model.to_string();
        Ok(self)
    }

    /// Replace the profile's output-token ceiling (the config-block
    /// override). Fail-loud in the same shape as [`Self::with_model_override`]:
    /// a zero budget cannot produce an answer, so it is a typed error rather
    /// than a pass that silently revises nothing.
    ///
    /// Deliberately no upper bound: the real ceiling is the model's, it is
    /// context-dependent, and the provider rejects an over-large request
    /// loudly - unlike the under-large one this override exists to fix.
    pub fn with_max_output_tokens(
        mut self,
        max_output_tokens: u32,
    ) -> Result<Self, HygienistError> {
        if max_output_tokens == 0 {
            return Err(HygienistError::Profile(
                "hygienist max_output_tokens override must be greater than zero".to_string(),
            ));
        }
        self.params.max_output_tokens = max_output_tokens;
        Ok(self)
    }

    /// Load an external calibration profile (fail-loud), same layout rules
    /// as the other stages' loaders.
    pub fn load(path: &Path) -> Result<Self, HygienistError> {
        let text = std::fs::read_to_string(path).map_err(|err| {
            HygienistError::Profile(format!("cannot read profile '{}': {err}", path.display()))
        })?;
        let raw: RawProfile = toml::from_str(&text).map_err(|err| {
            HygienistError::Profile(format!("invalid profile '{}': {err}", path.display()))
        })?;
        if raw.stage != "hygienist" {
            return Err(HygienistError::Profile(format!(
                "profile '{}' is for stage '{}', not 'hygienist'",
                path.display(),
                raw.stage
            )));
        }
        if raw.model.trim().is_empty() || raw.model == "PLACEHOLDER" {
            return Err(HygienistError::Profile(format!(
                "profile '{}' does not name a model",
                path.display()
            )));
        }
        let provider = match raw.provider.as_deref() {
            Some(name) => Provider::parse_strict(name).ok_or_else(|| {
                HygienistError::Profile(format!(
                    "profile '{}': unknown provider '{name}'",
                    path.display()
                ))
            })?,
            None => meerkat_models::infer_provider(&raw.model).ok_or_else(|| {
                HygienistError::Profile(format!(
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
            HygienistError::Profile(format!(
                "profile '{}': prompt_bundle '{}' does not resolve",
                path.display(),
                raw.prompt_bundle
            ))
        })?;
        let prompt_template = std::fs::read_to_string(bundle_path).map_err(|err| {
            HygienistError::Profile(format!(
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

    fn validate(&self) -> Result<(), HygienistError> {
        for placeholder in [TRANSCRIPT_PLACEHOLDER, PROTECTED_RANGES_PLACEHOLDER] {
            if !self.prompt_template.contains(placeholder) {
                return Err(HygienistError::Profile(format!(
                    "prompt bundle '{}' is missing placeholder `{placeholder}`",
                    self.prompt_bundle
                )));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Config (`agent_memory.hygienist { ... }`)
// ---------------------------------------------------------------------------

/// Hygienist config block. `enabled` defaults **off**: §15 ships this stage
/// last because it is the highest-risk stage, and flipping the default is a
/// calibration-scorecard decision (§11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HygienistConfig {
    pub enabled: bool,
    /// Hard per-realm window cap (window = 24 h, concurrency = 1).
    pub runs_per_day: u32,
    /// Model override for the embedded profile.
    pub model: Option<String>,
    /// Output-token ceiling for the curation call. `None` keeps the embedded
    /// profile's default.
    ///
    /// Exposed because the provider spends this single budget on
    /// reasoning tokens AND the visible answer, so a ceiling sized for a
    /// non-reasoning model truncates the op list to nothing without raising
    /// an error. The defect was not that the default was wrong but that it
    /// was a private constant a deployment could neither see nor change;
    /// [`crate::memory::steward::StewardConfig::max_output_tokens`] carries
    /// the production incident that made it visible, and
    /// [`crate::memory::distiller::DistillerConfig`] has the same knob.
    ///
    /// NOT YET HONORED BY ANY SHIPPED WIRING. Unlike its distiller and
    /// steward siblings there is no `memory_wiring` site for this stage: the
    /// module scope note in [`crate::memory_wiring`] keeps the Hygienist
    /// gateway-wired because the transcript-revision seam is not
    /// builder-owned, and the gateway's only `HygienistProfile` construction
    /// applies `model` alone. Setting this today changes nothing until that
    /// construction also calls [`HygienistProfile::with_max_output_tokens`].
    /// Deliberately not worked around from here: giving `HygienistEngine::new`
    /// a second, stage-specific place to reconcile profile against config
    /// would either swallow the zero-budget error or break every caller's
    /// signature, and inventing a `MemoryEnginesConfig.hygienist` field would
    /// be inventing architecture rather than wiring.
    pub max_output_tokens: Option<u32>,
}

impl Default for HygienistConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            runs_per_day: DEFAULT_RUNS_PER_DAY,
            model: None,
            max_output_tokens: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Client handle
// ---------------------------------------------------------------------------

/// One bounded model-client acquisition per pass (§8.1 invocation seam).
#[async_trait]
pub trait HygienistClientHandle: Send + Sync {
    async fn client(&self) -> Result<Arc<dyn LlmClient>, HygienistError>;
    fn invalidate(&self) {}
}

/// The production handle: meerkat's factory seam with auth-lease refresh,
/// shared implementation with the Selector/Distiller.
pub struct FactoryHygienistHandle {
    inner: FactoryModelClientHandle,
}

impl FactoryHygienistHandle {
    pub fn new(
        store_path: PathBuf,
        config: meerkat::Config,
        realm: impl Into<String>,
        profile: &HygienistProfile,
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
}

#[async_trait]
impl HygienistClientHandle for FactoryHygienistHandle {
    async fn client(&self) -> Result<Arc<dyn LlmClient>, HygienistError> {
        use crate::memory::factory_handle::{ModelClientError, ModelClientHandle};
        self.inner.client().await.map_err(|err| match err {
            ModelClientError::Auth(msg) => HygienistError::Auth(msg),
            other => HygienistError::Client(other.to_string()),
        })
    }

    fn invalidate(&self) {
        use crate::memory::factory_handle::ModelClientHandle;
        self.inner.invalidate();
    }
}

// ---------------------------------------------------------------------------
// The transcript-revision seam (meerkat 0.7.9 apply surface)
// ---------------------------------------------------------------------------

/// Receipt of one committed transcript revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedRevision {
    pub parent_revision: String,
    pub revision: String,
    pub message_count: usize,
}

/// Host-side reach into meerkat's audited same-session transcript revisions
/// (§8.6 mechanism). The production implementation wraps the CONCRETE
/// `PersistentSessionService` (which implements meerkat-core's
/// `SessionServiceTranscriptEditExt`) — the erased `Arc<dyn
/// MobSessionService>` the mob layer consumes does not carry the edit
/// extension, so the gateway threads a typed handle to here at bootstrap.
#[async_trait]
pub trait TranscriptRevisionSeam: Send + Sync {
    /// The session's current transcript together with its head revision id at
    /// read time (ask 4 refinement: `list_transcript_revisions` now exposes
    /// the head, so the caller can pin what it read and compare-and-swap on
    /// the rewrite). The head is `None` when the source cannot report one.
    /// `Ok(None)` when the session does not exist.
    async fn read_messages(
        &self,
        session_key: &str,
    ) -> Result<Option<(Vec<Message>, Option<String>)>, String>;

    /// Commit one audited rewrite replacing `[start, end)` with
    /// `replacement`. Implementations must refuse mid-turn sessions
    /// (meerkat's `TranscriptEditRunningBehavior::Reject` default).
    /// `expected_parent_revision` (the head observed by the matching
    /// `read_messages`) is a compare-and-swap guard: the rewrite is rejected
    /// if the head advanced since, so hygiene never commits against a
    /// transcript that changed under it.
    async fn rewrite(
        &self,
        session_key: &str,
        start: usize,
        end: usize,
        replacement: Vec<Message>,
        note: &str,
        expected_parent_revision: Option<String>,
    ) -> Result<AppliedRevision, String>;
}

/// The concrete session-service surface the seam needs: history reads plus
/// typed transcript edits. Blanket-implemented, so any service implementing
/// both extension traits (meerkat-session's `PersistentSessionService`
/// does) coerces to `Arc<dyn TranscriptEditSessionService>`.
pub trait TranscriptEditSessionService:
    meerkat_core::service::SessionServiceHistoryExt
    + meerkat_core::service::SessionServiceTranscriptEditExt
{
}

impl<T> TranscriptEditSessionService for T where
    T: meerkat_core::service::SessionServiceHistoryExt
        + meerkat_core::service::SessionServiceTranscriptEditExt
        + ?Sized
{
}

/// Production seam over the concrete session service (meerkat-session's
/// `PersistentSessionService` implements both extension traits; reads go
/// through `read_history`).
pub struct SessionServiceRevisionSeam {
    service: Arc<dyn TranscriptEditSessionService>,
}

impl SessionServiceRevisionSeam {
    pub fn new(service: Arc<dyn TranscriptEditSessionService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl TranscriptRevisionSeam for SessionServiceRevisionSeam {
    async fn read_messages(
        &self,
        session_key: &str,
    ) -> Result<Option<(Vec<Message>, Option<String>)>, String> {
        let session_id = meerkat_core::types::SessionId::parse(session_key)
            .map_err(|err| format!("invalid session key '{session_key}': {err}"))?;
        let messages = match self
            .service
            .read_history(
                &session_id,
                meerkat_core::service::SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
        {
            Ok(page) => page.messages,
            Err(meerkat_core::SessionError::NotFound { .. }) => return Ok(None),
            Err(err) => return Err(err.to_string()),
        };
        // Ask 4 refinement: capture the head revision for the rewrite CAS.
        // `limit: Some(0)` fetches the head without the commit log. A store
        // that does not support revision listing degrades to `None` (no CAS,
        // same as before) rather than failing the hygiene read.
        let head_revision = match self
            .service
            .list_transcript_revisions(
                &session_id,
                meerkat_core::service::SessionTranscriptRevisionListQuery {
                    limit: Some(0),
                    offset: None,
                },
            )
            .await
        {
            Ok(list) => Some(list.head_revision),
            Err(meerkat_core::SessionError::Unsupported(_)) => None,
            Err(err) => return Err(err.to_string()),
        };
        Ok(Some((messages, head_revision)))
    }

    async fn rewrite(
        &self,
        session_key: &str,
        start: usize,
        end: usize,
        replacement: Vec<Message>,
        note: &str,
        expected_parent_revision: Option<String>,
    ) -> Result<AppliedRevision, String> {
        let session_id = meerkat_core::types::SessionId::parse(session_key)
            .map_err(|err| format!("invalid session key '{session_key}': {err}"))?;
        let mut reason = meerkat_core::TranscriptRewriteReason::new("hygiene");
        reason.note = Some(note.to_string());
        let result = self
            .service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::service::SessionTranscriptRewriteRequest {
                    selection: meerkat_core::TranscriptRewriteSelection::MessageRange {
                        start,
                        end,
                    },
                    replacement,
                    reason,
                    actor: Some("meta-hygienist".to_string()),
                    // Ask 4 refinement: compare-and-swap against the head the
                    // matching read observed (via list_transcript_revisions).
                    // If the head advanced since, meerkat rejects the rewrite,
                    // so hygiene never commits against a changed transcript.
                    // Belt-and-braces with the mutation guard + Reject default.
                    expected_parent_revision,
                    running_behavior: meerkat_core::TranscriptEditRunningBehavior::default(),
                },
            )
            .await
            .map_err(|err| err.to_string())?;
        Ok(AppliedRevision {
            parent_revision: result.parent_revision,
            revision: result.revision,
            message_count: result.message_count,
        })
    }
}

// ---------------------------------------------------------------------------
// Span references (§8.6 validator inputs)
// ---------------------------------------------------------------------------

/// One memory record whose provenance cites the session under curation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanReference {
    pub record_id: String,
    /// Quarantined records hard-block (§8.6: until steward review
    /// completes); active records only flag.
    pub quarantined: bool,
    /// Cited message range within the session. `None` means the record
    /// cites the session without a range — conservatively treated as
    /// spanning the whole transcript.
    pub range: Option<(u64, u64)>,
}

/// Where the validator learns which spans are referenced by records.
#[async_trait]
pub trait SpanReferenceSource: Send + Sync {
    async fn span_references(
        &self,
        identity: &str,
        session_key: &str,
    ) -> Result<Vec<SpanReference>, String>;
}

/// Production source over any steward-capable store's existing readers: the
/// realm's quarantine queue (hard-block set) plus the identity/realm-scope
/// active manifests resolved to full records (audit-flag set). Mob-scope
/// records do not carry session evidence (promotion copies drop evidence
/// refs), so identity + realm scopes are the complete evidence-citing
/// population.
pub struct StoreSpanReferenceSource {
    store: Arc<dyn StewardStore>,
    realm: String,
}

impl StoreSpanReferenceSource {
    pub fn new(store: Arc<dyn StewardStore>, realm: impl Into<String>) -> Self {
        Self {
            store,
            realm: realm.into(),
        }
    }
}

#[async_trait]
impl SpanReferenceSource for StoreSpanReferenceSource {
    async fn span_references(
        &self,
        identity: &str,
        session_key: &str,
    ) -> Result<Vec<SpanReference>, String> {
        let mut references = Vec::new();
        let quarantined = self
            .store
            .quarantined_records(&self.realm, SPAN_QUARANTINE_LIMIT)
            .await
            .map_err(|err| err.to_string())?;
        for record in &quarantined {
            for evidence in &record.provenance.evidence {
                if evidence.session_id == session_key {
                    references.push(SpanReference {
                        record_id: record.id.clone(),
                        quarantined: true,
                        range: evidence.range,
                    });
                }
            }
        }
        let scopes = vec![
            MemoryScope::Identity {
                realm: self.realm.clone(),
                identity: identity.to_string(),
            },
            MemoryScope::Realm {
                realm: self.realm.clone(),
            },
        ];
        let manifest = self
            .store
            .manifest(&scopes, ManifestTier::Full)
            .await
            .map_err(|err| err.to_string())?;
        let ids: Vec<String> = manifest.into_iter().map(|meta| meta.id).collect();
        let records = self
            .store
            .records_by_ids(&self.realm, &ids)
            .await
            .map_err(|err| err.to_string())?;
        for record in &records {
            if record.status != RecordStatus::Active {
                continue;
            }
            for evidence in &record.provenance.evidence {
                if evidence.session_id == session_key {
                    references.push(SpanReference {
                        record_id: record.id.clone(),
                        quarantined: false,
                        range: evidence.range,
                    });
                }
            }
        }
        Ok(references)
    }
}

// ---------------------------------------------------------------------------
// Ordering gate (§8.4 invariant)
// ---------------------------------------------------------------------------

/// Where the §8.4 ordering check learns how far distillation has run.
pub trait DistillationGate: Send + Sync {
    /// Transcript index up to which distillation has covered
    /// `(identity, session_key)`.
    fn distilled_through(&self, identity: &str, session_key: &str) -> u64;
}

impl DistillationGate for DistillerEngine {
    fn distilled_through(&self, identity: &str, session_key: &str) -> u64 {
        self.distilled_cursor(identity, session_key)
    }
}

// ---------------------------------------------------------------------------
// Proposal model + parse
// ---------------------------------------------------------------------------

/// What one hygiene op does to its range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionAction {
    /// Stub the payload of the tool-result messages in the range; the
    /// messages (and their tool_use pairing) survive.
    PruneToolResults,
    /// Replace the messages in the range with one typed system notice
    /// carrying this note.
    Collapse { replacement: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionOp {
    pub action: RevisionAction,
    /// `[start, end)` message indices in the transcript being revised.
    pub start: usize,
    pub end: usize,
    pub rationale: String,
}

/// A parsed (not yet validated) revision proposal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RevisionProposal {
    pub ops: Vec<RevisionOp>,
}

#[derive(Deserialize)]
struct RawReply {
    #[serde(default)]
    ops: Vec<RawOp>,
}

#[derive(Deserialize)]
struct RawOp {
    op: String,
    range: (usize, usize),
    #[serde(default)]
    replacement: Option<String>,
    #[serde(default)]
    rationale: String,
}

/// Parse the model's reply into a proposal. Tolerates surrounding prose by
/// slicing the outermost JSON object (same tolerance as the other stages);
/// unknown op names are a parse error, not a silent drop — the reply is one
/// semantic unit.
pub fn parse_revision_reply(reply: &str) -> Result<RevisionProposal, String> {
    let start = reply
        .find('{')
        .ok_or_else(|| "reply contains no JSON object".to_string())?;
    let end = reply
        .rfind('}')
        .ok_or_else(|| "reply contains no closing brace".to_string())?;
    if end < start {
        return Err("reply braces are unbalanced".to_string());
    }
    let raw: RawReply = serde_json::from_str(&reply[start..=end])
        .map_err(|err| format!("reply did not parse: {err}"))?;
    let mut ops = Vec::new();
    for raw_op in raw.ops {
        let (start, end) = raw_op.range;
        let action = match raw_op.op.as_str() {
            "prune_tool_results" => RevisionAction::PruneToolResults,
            "collapse" => {
                let replacement = raw_op
                    .replacement
                    .as_deref()
                    .map(compact_whitespace)
                    .unwrap_or_default();
                if replacement.is_empty() {
                    return Err("collapse op without a replacement note".to_string());
                }
                RevisionAction::Collapse {
                    replacement: truncate_utf8_boundary(
                        &replacement,
                        MAX_COLLAPSE_REPLACEMENT_BYTES,
                    ),
                }
            }
            other => return Err(format!("unknown op '{other}'")),
        };
        ops.push(RevisionOp {
            action,
            start,
            end,
            rationale: compact_whitespace(&raw_op.rationale),
        });
    }
    Ok(RevisionProposal { ops })
}

// ---------------------------------------------------------------------------
// Deterministic validator (§8.6)
// ---------------------------------------------------------------------------

/// Role projection used for validator law and prompt rendering. Total over
/// the raw message list so op indices always address the real transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HygieneRole {
    System,
    SystemNotice,
    User,
    /// Assistant message without tool calls.
    Assistant,
    /// Assistant message that issues tool calls — collapsing it would
    /// orphan the paired tool results, so it is untouchable.
    AssistantToolUse,
    ToolResults,
    /// A detached `fork_off`/council job's durable completion entry. It
    /// records an outcome the agent refers back to on later turns, not
    /// scaffolding, so it is untouchable: collapsing it would erase that
    /// outcome from the working view.
    BackgroundJobResult,
}

impl HygieneRole {
    pub fn of(message: &Message) -> Self {
        if crate::detached_completion::is_detached_completion_entry(message) {
            return Self::BackgroundJobResult;
        }
        match message {
            Message::System(_) => Self::System,
            Message::SystemNotice(_) => Self::SystemNotice,
            Message::User(_) => Self::User,
            Message::BlockAssistant(assistant) => {
                if assistant.has_tool_calls() {
                    Self::AssistantToolUse
                } else {
                    Self::Assistant
                }
            }
            Message::ToolResults { .. } => Self::ToolResults,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::SystemNotice => "system notice",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::AssistantToolUse => "assistant (tool call)",
            Self::ToolResults => "tool results",
            Self::BackgroundJobResult => "background job result",
        }
    }
}

/// Why the validator refused a proposal wholesale. A refused proposal
/// applies nothing — the revision is one semantic unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionReject {
    /// Malformed ranges: out of bounds, empty, overlapping.
    InvalidRange { detail: String },
    /// Role law: prune targets non-tool messages, collapse touches tool
    /// activity, the system prompt or a background job result.
    IllegalRole { detail: String },
    /// §8.6 hard-block: the revision touches a span referenced by a
    /// quarantined record whose steward review has not completed.
    QuarantineReferenced { record_id: String },
    /// §8.4 ordering invariant: distillation has not covered the affected
    /// window.
    OrderingUnmet { cursor: u64, needed: u64 },
}

impl std::fmt::Display for RevisionReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRange { detail } => write!(f, "invalid range: {detail}"),
            Self::IllegalRole { detail } => write!(f, "illegal role: {detail}"),
            Self::QuarantineReferenced { record_id } => write!(
                f,
                "range referenced by quarantined record '{record_id}' (review incomplete)"
            ),
            Self::OrderingUnmet { cursor, needed } => write!(
                f,
                "distillation cursor {cursor} has not covered the affected window (needs {needed})"
            ),
        }
    }
}

/// A validated proposal plus its §8.6 audit flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRevision {
    pub ops: Vec<RevisionOp>,
    /// Active records whose evidence spans the revision touches — allowed,
    /// audited (§8.6).
    pub flagged_active_records: Vec<String>,
}

/// How the §8.4 ordering invariant is discharged for this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderingContext {
    /// The pass was sequenced behind the distiller's compaction harvest
    /// (the follow-up hook) — the invariant holds by construction.
    SequencedAfterHarvest,
    /// On-demand: check the distiller's window cursor. `None` means no
    /// distiller is deployed — nothing to order against (revisions stay
    /// restorable regardless; §8.4's "where enabled" clause).
    Cursor(Option<u64>),
}

/// The §8.6 deterministic validator. Judgment picked the ranges; this is
/// the law they pass through.
pub fn validate_revision(
    proposal: &RevisionProposal,
    roles: &[HygieneRole],
    spans: &[SpanReference],
    ordering: OrderingContext,
) -> Result<ValidatedRevision, RevisionReject> {
    let len = roles.len();
    let mut sorted: Vec<&RevisionOp> = proposal.ops.iter().collect();
    sorted.sort_by_key(|op| op.start);
    let mut previous_end = 0usize;
    for op in &sorted {
        if op.start >= op.end {
            return Err(RevisionReject::InvalidRange {
                detail: format!("empty range [{}, {})", op.start, op.end),
            });
        }
        if op.end > len {
            return Err(RevisionReject::InvalidRange {
                detail: format!(
                    "range [{}, {}) exceeds transcript length {len}",
                    op.start, op.end
                ),
            });
        }
        if op.start < previous_end {
            return Err(RevisionReject::InvalidRange {
                detail: format!("range [{}, {}) overlaps an earlier op", op.start, op.end),
            });
        }
        previous_end = op.end;
        for (index, &role) in roles.iter().enumerate().take(op.end).skip(op.start) {
            match op.action {
                RevisionAction::PruneToolResults => {
                    if role != HygieneRole::ToolResults {
                        return Err(RevisionReject::IllegalRole {
                            detail: format!(
                                "prune_tool_results range [{}, {}) covers a {} message at [{index}]",
                                op.start,
                                op.end,
                                role.as_str()
                            ),
                        });
                    }
                }
                RevisionAction::Collapse { .. } => {
                    if !matches!(
                        role,
                        HygieneRole::User | HygieneRole::SystemNotice | HygieneRole::Assistant
                    ) {
                        return Err(RevisionReject::IllegalRole {
                            detail: format!(
                                "collapse range [{}, {}) covers a {} message at [{index}]",
                                op.start,
                                op.end,
                                role.as_str()
                            ),
                        });
                    }
                }
            }
        }
    }
    // §8.6 quarantine hard-block, then active-record audit flags.
    let mut flagged: Vec<String> = Vec::new();
    for span in spans {
        let (span_start, span_end) = match span.range {
            Some((start, end)) => (start as usize, (end as usize).saturating_add(1)),
            // A record citing the session without a range conservatively
            // spans everything.
            None => (0, len.max(1)),
        };
        let touched = sorted
            .iter()
            .any(|op| op.start < span_end && span_start < op.end);
        if !touched {
            continue;
        }
        if span.quarantined {
            return Err(RevisionReject::QuarantineReferenced {
                record_id: span.record_id.clone(),
            });
        }
        if !flagged.contains(&span.record_id) {
            flagged.push(span.record_id.clone());
        }
    }
    // §8.4 ordering invariant.
    if let OrderingContext::Cursor(Some(cursor)) = ordering {
        let needed = sorted.iter().map(|op| op.end as u64).max().unwrap_or(0);
        if needed > cursor {
            return Err(RevisionReject::OrderingUnmet { cursor, needed });
        }
    }
    Ok(ValidatedRevision {
        ops: sorted.into_iter().cloned().collect(),
        flagged_active_records: flagged,
    })
}

// ---------------------------------------------------------------------------
// Prompt rendering
// ---------------------------------------------------------------------------

fn message_text(message: &Message) -> String {
    match message {
        Message::System(_) => "(system prompt — untouchable)".to_string(),
        Message::SystemNotice(notice) => notice.body.clone().unwrap_or_default(),
        Message::User(user) => user.text_content(),
        Message::BlockAssistant(assistant) => {
            assistant.text_blocks().collect::<Vec<_>>().join("\n")
        }
        Message::ToolResults { results, .. } => results
            .iter()
            .map(|result| meerkat_core::types::text_content(&result.content))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Render the transcript with `[N]` raw indices and roles, bounded by the
/// total byte budget (oldest messages drop first).
pub fn render_transcript(messages: &[Message]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut total = 0usize;
    for (index, message) in messages.iter().enumerate().rev() {
        let role = HygieneRole::of(message);
        let text = truncate_utf8_boundary(
            &compact_whitespace(&message_text(message)),
            MAX_TRANSCRIPT_MESSAGE_BYTES,
        );
        let line = format!("[{index}] {}: {text}", role.as_str());
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

fn render_protected_ranges(spans: &[SpanReference]) -> String {
    if spans.is_empty() {
        return "(none)".to_string();
    }
    spans
        .iter()
        .map(|span| {
            let range = match span.range {
                Some((start, end)) => format!("[{start}-{end}]"),
                None => "[whole session]".to_string(),
            };
            format!(
                "- {} {} referenced by record '{}'",
                if span.quarantined {
                    "QUARANTINED"
                } else {
                    "active"
                },
                range,
                span.record_id
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_prompt(
    profile: &HygienistProfile,
    messages: &[Message],
    spans: &[SpanReference],
) -> String {
    profile
        .prompt_template
        .replace(TRANSCRIPT_PLACEHOLDER, &render_transcript(messages))
        .replace(
            PROTECTED_RANGES_PLACEHOLDER,
            &render_protected_ranges(spans),
        )
}

// ---------------------------------------------------------------------------
// Replacement construction
// ---------------------------------------------------------------------------

/// Build the replacement for the hull `[hull_start, hull_end)` of the
/// validated ops: untouched messages pass through, pruned tool results keep
/// their `tool_use_id` pairing with stubbed payloads, collapsed runs become
/// one typed system notice.
pub fn build_replacement(
    messages: &[Message],
    ops: &[RevisionOp],
) -> Option<(usize, usize, Vec<Message>)> {
    let hull_start = ops.iter().map(|op| op.start).min()?;
    let hull_end = ops.iter().map(|op| op.end).max()?;
    let mut replacement = Vec::new();
    let mut index = hull_start;
    while index < hull_end {
        if let Some(op) = ops.iter().find(|op| op.start == index) {
            match &op.action {
                RevisionAction::PruneToolResults => {
                    for pruned in &messages[op.start..op.end] {
                        if let Message::ToolResults {
                            results,
                            created_at,
                        } = pruned
                        {
                            let stubbed = results
                                .iter()
                                .map(|result| {
                                    let mut stubbed = result.clone();
                                    stubbed.set_text_content(format!(
                                        "[pruned by hygienist: {}]",
                                        op.rationale
                                    ));
                                    stubbed
                                })
                                .collect();
                            replacement.push(Message::ToolResults {
                                results: stubbed,
                                created_at: *created_at,
                            });
                        }
                    }
                }
                RevisionAction::Collapse { replacement: note } => {
                    replacement.push(Message::SystemNotice(SystemNoticeMessage::new(
                        SystemNoticeKind::Generic,
                        format!(
                            "[hygienist] collapsed {} messages: {note}",
                            op.end - op.start
                        ),
                    )));
                }
            }
            index = op.end;
        } else {
            replacement.push(messages[index].clone());
            index += 1;
        }
    }
    Some((hull_start, hull_end, replacement))
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// What triggered a hygiene pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HygieneCause {
    /// Sequenced behind the distiller's compaction harvest (or directly
    /// off the compaction event when no distiller is deployed).
    PostCompaction,
    OnDemand,
}

impl HygieneCause {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PostCompaction => "post_compaction",
            Self::OnDemand => "on_demand",
        }
    }
}

/// Outcome of one `hygiene_now` call, for logs and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HygieneOutcome {
    Skipped {
        reason: String,
    },
    Blocked {
        reason: String,
    },
    Applied {
        run_id: String,
        revision: AppliedRevision,
        ops: usize,
        flagged_active_records: Vec<String>,
    },
}

pub struct HygienistEngine {
    profile: HygienistProfile,
    config: HygienistConfig,
    handle: Arc<dyn HygienistClientHandle>,
    seam: Arc<dyn TranscriptRevisionSeam>,
    spans: Arc<dyn SpanReferenceSource>,
    gate: Option<Arc<dyn DistillationGate>>,
    budget: BackgroundBudget,
    realm: String,
    events: Mutex<Option<Arc<dyn MemoryEventSink>>>,
    run_counter: std::sync::atomic::AtomicU64,
}

impl HygienistEngine {
    pub fn new(
        profile: HygienistProfile,
        config: HygienistConfig,
        handle: Arc<dyn HygienistClientHandle>,
        seam: Arc<dyn TranscriptRevisionSeam>,
        spans: Arc<dyn SpanReferenceSource>,
        gate: Option<Arc<dyn DistillationGate>>,
        realm: impl Into<String>,
    ) -> Self {
        // Curation concurrency is 1 per realm; runs/day is the window cap.
        let budget = BackgroundBudget::new(BackgroundBudgetConfig {
            runs_per_window: config.runs_per_day,
            // `Duration::from_days` is unstable (duration_constructors);
            // clippy 1.96 suggests it, so allow the units lint here.
            #[allow(clippy::duration_suboptimal_units)]
            window: Duration::from_secs(24 * 60 * 60),
            max_concurrent: 1,
        });
        Self {
            profile,
            config,
            handle,
            seam,
            spans,
            gate,
            budget,
            realm: realm.into(),
            events: Mutex::new(None),
            run_counter: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn config(&self) -> &HygienistConfig {
        &self.config
    }

    /// Wire the §9.3 timeline sink; also threads it into the budget guard.
    pub fn set_event_sink(&self, sink: Arc<dyn MemoryEventSink>) {
        self.budget.set_event_sink(sink.clone());
        *self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sink);
    }

    fn emit(&self, event: MemoryTimelineEvent) {
        if let Some(sink) = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            sink.emit(event);
        }
    }

    fn mint_run_id(&self) -> String {
        let seq = self
            .run_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("hygiene-{}-{seq}", now_ms())
    }

    /// One curation pass over `session_key`'s live transcript. Never on a
    /// delivery path; a mid-turn session is refused by the seam.
    pub async fn hygiene_now(
        self: &Arc<Self>,
        identity: &str,
        session_key: &str,
        cause: HygieneCause,
    ) -> HygieneOutcome {
        let outcome = self.hygiene_inner(identity, session_key, cause).await;
        match &outcome {
            HygieneOutcome::Skipped { reason } => {
                tracing::debug!(
                    identity,
                    session_key,
                    cause = cause.as_str(),
                    reason,
                    "agent memory hygienist: pass skipped"
                );
                self.emit(MemoryTimelineEvent::HygieneSkipped {
                    identity: identity.to_string(),
                    session_key: session_key.to_string(),
                    cause: cause.as_str().to_string(),
                    reason: reason.clone(),
                });
            }
            HygieneOutcome::Blocked { reason } => {
                tracing::warn!(
                    identity,
                    session_key,
                    cause = cause.as_str(),
                    reason,
                    "agent memory hygienist: revision blocked"
                );
                self.emit(MemoryTimelineEvent::HygieneBlocked {
                    identity: identity.to_string(),
                    session_key: session_key.to_string(),
                    cause: cause.as_str().to_string(),
                    reason: reason.clone(),
                });
            }
            HygieneOutcome::Applied {
                run_id,
                revision,
                ops,
                flagged_active_records,
            } => {
                tracing::info!(
                    identity,
                    session_key,
                    cause = cause.as_str(),
                    run_id,
                    ops,
                    revision = %revision.revision,
                    "agent memory hygienist: revision applied"
                );
                self.emit(MemoryTimelineEvent::HygieneApplied {
                    identity: identity.to_string(),
                    session_key: session_key.to_string(),
                    cause: cause.as_str().to_string(),
                    parent_revision: revision.parent_revision.clone(),
                    revision: revision.revision.clone(),
                    ops: *ops,
                    flagged_active_records: flagged_active_records.clone(),
                });
            }
        }
        outcome
    }

    async fn hygiene_inner(
        self: &Arc<Self>,
        identity: &str,
        session_key: &str,
        cause: HygieneCause,
    ) -> HygieneOutcome {
        // Reads come before the budget gate: an empty transcript must not
        // burn a budgeted run.
        let (messages, head_revision) = match self.seam.read_messages(session_key).await {
            Ok(Some(read)) => read,
            Ok(None) => {
                return HygieneOutcome::Skipped {
                    reason: "session not found".to_string(),
                };
            }
            Err(err) => {
                return HygieneOutcome::Skipped {
                    reason: format!("transcript read failed: {err}"),
                };
            }
        };
        if messages.is_empty() {
            return HygieneOutcome::Skipped {
                reason: "empty transcript".to_string(),
            };
        }
        let spans = match self.spans.span_references(identity, session_key).await {
            Ok(spans) => spans,
            Err(err) => {
                // Fail closed: without the span facts the §8.6 hard-block
                // cannot be checked, so no revision happens.
                return HygieneOutcome::Blocked {
                    reason: format!("span references unavailable: {err}"),
                };
            }
        };

        let _permit = match self.budget.try_acquire(&self.realm, "hygienist") {
            Ok(permit) => permit,
            Err(denied) => {
                return HygieneOutcome::Skipped {
                    reason: format!("budget denied: {denied}"),
                };
            }
        };

        let client = match self.handle.client().await {
            Ok(client) => client,
            Err(err) => {
                return HygieneOutcome::Skipped {
                    reason: format!("client acquisition failed: {err}"),
                };
            }
        };
        let prompt = render_prompt(&self.profile, &messages, &spans);
        let reply = match complete_text(&*client, &self.profile, prompt.clone()).await {
            Ok(reply) => reply,
            Err(HygienistError::Auth(message)) => {
                // One re-resolve, mirroring the other stages' auth containment.
                tracing::warn!(error = %message, "hygienist auth failure; re-resolving client");
                self.handle.invalidate();
                let retried = match self.handle.client().await {
                    Ok(client) => complete_text(&*client, &self.profile, prompt).await,
                    Err(err) => Err(err),
                };
                match retried {
                    Ok(reply) => reply,
                    Err(err) => {
                        return HygieneOutcome::Skipped {
                            reason: format!("completion failed: {err}"),
                        };
                    }
                }
            }
            Err(err) => {
                return HygieneOutcome::Skipped {
                    reason: format!("completion failed: {err}"),
                };
            }
        };
        let proposal = match parse_revision_reply(&reply) {
            Ok(proposal) => proposal,
            Err(err) => {
                return HygieneOutcome::Skipped {
                    reason: format!("reply did not parse: {err}"),
                };
            }
        };
        if proposal.ops.is_empty() {
            return HygieneOutcome::Skipped {
                reason: "no-op judgment (preferred output)".to_string(),
            };
        }
        let roles: Vec<HygieneRole> = messages.iter().map(HygieneRole::of).collect();
        let ordering = match cause {
            HygieneCause::PostCompaction => OrderingContext::SequencedAfterHarvest,
            HygieneCause::OnDemand => OrderingContext::Cursor(
                self.gate
                    .as_ref()
                    .map(|gate| gate.distilled_through(identity, session_key)),
            ),
        };
        let validated = match validate_revision(&proposal, &roles, &spans, ordering) {
            Ok(validated) => validated,
            Err(reject) => {
                return HygieneOutcome::Blocked {
                    reason: reject.to_string(),
                };
            }
        };
        let run_id = self.mint_run_id();
        self.emit(MemoryTimelineEvent::HygieneProposed {
            identity: identity.to_string(),
            session_key: session_key.to_string(),
            cause: cause.as_str().to_string(),
            ops: validated.ops.len(),
            flagged_active_records: validated.flagged_active_records.clone(),
        });
        let Some((hull_start, hull_end, replacement)) =
            build_replacement(&messages, &validated.ops)
        else {
            return HygieneOutcome::Skipped {
                reason: "validated proposal had no ops".to_string(),
            };
        };
        let rationales = validated
            .ops
            .iter()
            .map(|op| op.rationale.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let note = truncate_utf8_boundary(&format!("{run_id}: {rationales}"), 512);
        match self
            .seam
            .rewrite(
                session_key,
                hull_start,
                hull_end,
                replacement,
                &note,
                head_revision,
            )
            .await
        {
            Ok(revision) => HygieneOutcome::Applied {
                run_id,
                revision,
                ops: validated.ops.len(),
                flagged_active_records: validated.flagged_active_records,
            },
            Err(err) => HygieneOutcome::Skipped {
                reason: format!("revision apply refused: {err}"),
            },
        }
    }

    /// Detached pass (trigger paths). Never on any critical path.
    pub fn spawn_detached(
        self: &Arc<Self>,
        identity: &str,
        session_key: &str,
        cause: HygieneCause,
    ) {
        let engine = self.clone();
        let identity = identity.to_string();
        let session_key = session_key.to_string();
        tokio::spawn(async move {
            engine.hygiene_now(&identity, &session_key, cause).await;
        });
    }
}

/// The §8.6 trigger-sequencing glue: a [`crate::memory::distiller::CompactionFollowUp`]
/// that runs hygiene strictly AFTER the distiller's compaction harvest for
/// the boundary, and only when the harvest left nothing unharvested
/// (`DistillOutcome::compaction_harvest_satisfied` — budget denials and
/// read/extraction failures block hygiene loudly instead).
pub fn distiller_follow_up(engine: Arc<HygienistEngine>) -> CompactionFollowUp {
    Arc::new(
        move |identity: &str, session_key: &str, outcome: &DistillOutcome| {
            if outcome.compaction_harvest_satisfied() {
                engine.spawn_detached(identity, session_key, HygieneCause::PostCompaction);
            } else {
                let reason = match outcome {
                    DistillOutcome::Skipped { reason } => reason.clone(),
                    DistillOutcome::Completed { .. } => unreachable!("completed harvests satisfy"),
                };
                tracing::warn!(
                    identity,
                    session_key,
                    reason,
                    "agent memory hygienist: post-compaction pass withheld (harvest incomplete)"
                );
                engine.emit(MemoryTimelineEvent::HygieneSkipped {
                    identity: identity.to_string(),
                    session_key: session_key.to_string(),
                    cause: HygieneCause::PostCompaction.as_str().to_string(),
                    reason: format!("distiller harvest incomplete: {reason}"),
                });
            }
        },
    )
}

// ---------------------------------------------------------------------------
// Observe-stream trigger (deployments without a distiller)
// ---------------------------------------------------------------------------

/// Compaction trigger for deployments where no distiller is enabled: rides
/// the same member-event observer. When a distiller IS enabled, use
/// [`distiller_follow_up`] instead — registering both would race the §8.4
/// ordering this exists to preserve.
pub struct HygienistTriggers {
    engine: Arc<HygienistEngine>,
}

impl HygienistTriggers {
    pub fn new(engine: Arc<HygienistEngine>) -> Self {
        Self { engine }
    }
}

impl MemberAgentEventSink for HygienistTriggers {
    fn observe(&self, identity: &str, envelope: &meerkat_core::event::EventEnvelope<AgentEvent>) {
        // Scope keys are LOGICAL identities (task #53) - same fixed-point
        // re-normalization as the distiller sink.
        let identity = crate::member_comms_id::logical_memory_identity(identity);
        let identity = identity.as_str();
        if let AgentEvent::CompactionCompleted { .. } = &envelope.payload {
            match &envelope.source {
                meerkat_core::event::EventSourceIdentity::Session { session_id } => {
                    self.engine.spawn_detached(
                        identity,
                        &session_id.to_string(),
                        HygieneCause::PostCompaction,
                    );
                }
                _ => {
                    tracing::warn!(
                        identity,
                        "agent memory hygienist: compaction event without session \
                         attribution; pass skipped"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// LLM call
// ---------------------------------------------------------------------------

/// One bounded completion against the profile's model/params.
///
/// `temperature` is set unconditionally on purpose: whether it reaches the
/// wire is the provider client's decision, which already consults the model's
/// catalog row. See [`crate::memory::steward::complete_text`] for the full
/// reasoning.
pub async fn complete_text(
    client: &dyn LlmClient,
    profile: &HygienistProfile,
    prompt: String,
) -> Result<String, HygienistError> {
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
                        return Err(HygienistError::Truncated {
                            max_output_tokens: profile.params.max_output_tokens,
                        });
                    }
                    // Nor is any other unfinished body an answer, however well
                    // it parses: no revision is applied from a filtered reply.
                    CompletionStop::Incomplete(stop_reason) => {
                        return Err(HygienistError::Incomplete { stop_reason });
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
        return Err(HygienistError::Client(UNTERMINATED_STREAM.to_string()));
    }
    Ok(meerkat_core::types::BlockAssistantMessage::new(
        assembler.finalize(),
        meerkat_core::StopReason::EndTurn,
    )
    .to_string())
}

fn classify_llm_error(error: LlmError) -> HygienistError {
    match error {
        LlmError::AuthenticationFailed { .. } | LlmError::InvalidApiKey => {
            HygienistError::Auth(error.to_string())
        }
        // Policy stops must not enter the auth-refresh retry path.
        LlmError::PolicyStop { .. } => HygienistError::Client(error.to_string()),
        other => HygienistError::Client(other.to_string()),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::StopReason;
    use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, ToolResult};
    use std::sync::Mutex as StdMutex;

    #[tokio::test]
    async fn complete_text_uses_final_assistant_output() {
        for provisional in [None, Some("discarded provisional text")] {
            for blocks in [
                Vec::new(),
                vec![
                    AssistantBlock::Text {
                        text: "{".to_string(),
                        meta: None,
                    },
                    AssistantBlock::Reasoning {
                        text: "not answer text".to_string(),
                        meta: None,
                    },
                    AssistantBlock::Text {
                        text: "}".to_string(),
                        meta: None,
                    },
                ],
            ] {
                let expected = if blocks.is_empty() { "" } else { "{}" };
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
                    outcome: LlmDoneOutcome::success(StopReason::EndTurn),
                });
                let client = meerkat_client::TestClient::new(events);
                assert_eq!(
                    complete_text(
                        &client,
                        &HygienistProfile::embedded_default(),
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

        let profile = HygienistProfile::embedded_default();
        for case in CompletionCase::ALL {
            let client = ObservationClient::new(case);
            let (result, log) =
                capture_warnings(complete_text(&client, &profile, "test".into())).await;
            match case {
                CompletionCase::Success => {
                    assert_eq!(result.expect("physical success"), PHYSICAL_TEXT);
                }
                CompletionCase::TerminalError | CompletionCase::StreamError => assert!(
                    matches!(result, Err(HygienistError::Client(message))
                        if message == LlmError::ConnectionReset.to_string()),
                    "partial text must not replace the terminal provider failure",
                ),
                CompletionCase::Truncated => assert!(matches!(
                    result,
                    Err(HygienistError::Truncated { max_output_tokens })
                        if max_output_tokens == profile.params.max_output_tokens
                )),
                CompletionCase::Unterminated => assert!(
                    matches!(result, Err(HygienistError::Client(message))
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
                outcome: LlmDoneOutcome::success(StopReason::MaxTokens),
            },
        ]);
        let profile = HygienistProfile::embedded_default();
        assert!(matches!(
            complete_text(&client, &profile, "test".into()).await,
            Err(HygienistError::Truncated { max_output_tokens })
                if max_output_tokens == profile.params.max_output_tokens
        ));
    }

    /// A body the model did not finish is not an answer, however well it
    /// parses: every stop before a complete answer is typed.
    #[tokio::test]
    async fn complete_text_refuses_an_incomplete_stop() {
        use crate::test_diagnostics::{INCOMPLETE_STOPS, stopping_client};

        let profile = HygienistProfile::embedded_default();
        for stop in INCOMPLETE_STOPS {
            let client = stopping_client(PRUNE_REPLY, stop);
            let result = complete_text(&client, &profile, "test".into()).await;
            assert!(
                matches!(
                    result,
                    Err(HygienistError::Incomplete { stop_reason }) if stop_reason == stop
                ),
                "{stop:?}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn policy_stop_is_not_auth_or_successful_text() {
        let stop = LlmError::PolicyStop {
            code: "misalignment_policy_violation".to_string(),
            message: "operator review required".to_string(),
        };
        assert!(!stop.is_retryable());
        assert!(matches!(
            classify_llm_error(stop.clone()),
            HygienistError::Client(message)
                if message.contains("misalignment_policy_violation")
        ));
        let client = meerkat_client::TestClient::new(vec![
            LlmEvent::TextDelta {
                delta: "invalid JSON must not be accepted after a policy stop".to_string(),
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error: stop },
            },
        ]);
        assert!(matches!(
            complete_text(&client, &HygienistProfile::embedded_default(), "test".into()).await,
            Err(HygienistError::Client(message))
                if message.contains("misalignment_policy_violation")
        ));
    }

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    fn assistant(text: &str) -> Message {
        Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: text.to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        ))
    }

    fn assistant_tool_call(id: &str) -> Message {
        let args = serde_json::value::RawValue::from_string(r#"{"cmd":"ls"}"#.to_string())
            .expect("raw args");
        Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::ToolUse {
                id: id.to_string(),
                name: "shell".to_string(),
                args,
                meta: None,
            }],
            StopReason::ToolUse,
        ))
    }

    fn tool_results(id: &str, text: &str) -> Message {
        Message::tool_results(vec![ToolResult::new(
            id.to_string(),
            text.to_string(),
            false,
        )])
    }

    fn transcript() -> Vec<Message> {
        vec![
            user("please check the logs"),               // 0
            assistant_tool_call("call-1"),               // 1
            tool_results("call-1", "3000 lines of log"), // 2
            assistant("logs are clean; decision: ship"), // 3
            user("scaffold notice"),                     // 4
            user("scaffold notice"),                     // 5
            user("scaffold notice"),                     // 6
            assistant("done"),                           // 7
        ]
    }

    fn roles(messages: &[Message]) -> Vec<HygieneRole> {
        messages.iter().map(HygieneRole::of).collect()
    }

    /// A detached `fork_off` job's durable completion entry, exactly as
    /// meerkat's detached delivery builds it.
    fn completion_entry() -> Message {
        Message::SystemNotice(
            meerkat_mob_mcp::detached_delivery::detached_completion_notice(
                "fork_off",
                "job-a",
                meerkat_core::event::BackgroundJobTerminalStatus::Completed,
                &serde_json::json!({ "text": "done" }),
            )
            .expect("meerkat builds the completion notice"),
        )
    }

    fn prune(start: usize, end: usize) -> RevisionOp {
        RevisionOp {
            action: RevisionAction::PruneToolResults,
            start,
            end,
            rationale: "dead output".to_string(),
        }
    }

    fn collapse(start: usize, end: usize) -> RevisionOp {
        RevisionOp {
            action: RevisionAction::Collapse {
                replacement: "repeated scaffolding".to_string(),
            },
            start,
            end,
            rationale: "scaffolding".to_string(),
        }
    }

    // -- parse ---------------------------------------------------------------

    #[test]
    fn parse_accepts_ops_and_rejects_unknown() {
        let proposal = parse_revision_reply(
            r#"{"ops": [
                {"op": "prune_tool_results", "range": [2, 3], "rationale": "dead"},
                {"op": "collapse", "range": [4, 7], "replacement": "notices", "rationale": "dup"}
            ]}"#,
        )
        .expect("parses");
        assert_eq!(proposal.ops.len(), 2);
        assert_eq!(proposal.ops[0].action, RevisionAction::PruneToolResults);
        assert!(matches!(
            proposal.ops[1].action,
            RevisionAction::Collapse { .. }
        ));

        assert!(parse_revision_reply(r#"{"ops": [{"op": "delete", "range": [0, 1]}]}"#).is_err());
        assert!(
            parse_revision_reply(r#"{"ops": [{"op": "collapse", "range": [0, 1]}]}"#).is_err(),
            "collapse without replacement must not parse"
        );
        assert_eq!(
            parse_revision_reply(r#"{"ops": []}"#).expect("noop parses"),
            RevisionProposal::default()
        );
        // Prose tolerance: the JSON object is sliced out.
        assert!(parse_revision_reply("Sure! {\"ops\": []} Done.").is_ok());
    }

    // -- validator matrix ------------------------------------------------------

    #[test]
    fn validator_accepts_legal_prune_and_collapse() {
        let messages = transcript();
        let proposal = RevisionProposal {
            ops: vec![prune(2, 3), collapse(4, 7)],
        };
        let validated = validate_revision(
            &proposal,
            &roles(&messages),
            &[],
            OrderingContext::SequencedAfterHarvest,
        )
        .expect("legal ops validate");
        assert_eq!(validated.ops.len(), 2);
        assert!(validated.flagged_active_records.is_empty());
    }

    #[test]
    fn validator_rejects_malformed_ranges() {
        let messages = transcript();
        let roles = roles(&messages);
        for (proposal, name) in [
            (
                RevisionProposal {
                    ops: vec![prune(3, 3)],
                },
                "empty",
            ),
            (
                RevisionProposal {
                    ops: vec![prune(2, 99)],
                },
                "out of bounds",
            ),
            (
                RevisionProposal {
                    ops: vec![collapse(4, 7), collapse(5, 8)],
                },
                "overlap",
            ),
        ] {
            let result = validate_revision(
                &proposal,
                &roles,
                &[],
                OrderingContext::SequencedAfterHarvest,
            );
            assert!(
                matches!(result, Err(RevisionReject::InvalidRange { .. })),
                "{name}: {result:?}"
            );
        }
    }

    #[test]
    fn validator_enforces_role_law() {
        let messages = transcript();
        let roles = roles(&messages);
        // Prune over a non-tool message.
        let result = validate_revision(
            &RevisionProposal {
                ops: vec![prune(2, 4)],
            },
            &roles,
            &[],
            OrderingContext::SequencedAfterHarvest,
        );
        assert!(matches!(result, Err(RevisionReject::IllegalRole { .. })));
        // Collapse over an assistant tool call (would orphan the pairing).
        let result = validate_revision(
            &RevisionProposal {
                ops: vec![collapse(1, 3)],
            },
            &roles,
            &[],
            OrderingContext::SequencedAfterHarvest,
        );
        assert!(matches!(result, Err(RevisionReject::IllegalRole { .. })));
    }

    /// A background job result is an outcome the agent refers back to, not
    /// scaffolding: a collapse covering it is refused wholesale, while the
    /// scaffolding beside it still collapses.
    #[test]
    fn validator_never_collapses_a_background_job_result() {
        let messages = vec![
            user("scaffold notice"), // 0
            completion_entry(),      // 1
            user("scaffold notice"), // 2
            user("scaffold notice"), // 3
        ];
        let roles = roles(&messages);
        assert_eq!(roles[1], HygieneRole::BackgroundJobResult);
        let result = validate_revision(
            &RevisionProposal {
                ops: vec![collapse(0, 3)],
            },
            &roles,
            &[],
            OrderingContext::SequencedAfterHarvest,
        );
        assert!(
            matches!(result, Err(RevisionReject::IllegalRole { .. })),
            "{result:?}"
        );
        let result = validate_revision(
            &RevisionProposal {
                ops: vec![collapse(2, 4)],
            },
            &roles,
            &[],
            OrderingContext::SequencedAfterHarvest,
        );
        assert!(result.is_ok(), "{result:?}");
        assert!(
            render_transcript(&messages).contains("[1] background job result: "),
            "the hygienist sees the entry under its own role"
        );
    }

    #[test]
    fn validator_never_collapses_a_mixed_background_job_notice() {
        let Message::SystemNotice(mut mixed) = completion_entry() else {
            panic!("completion fixture must be a typed notice");
        };
        let refresh_block = meerkat_core::types::SystemNoticeBlock::BackgroundJob {
            job_id: "shell-refresh".to_string(),
            display_name: Some("shell".to_string()),
            status: meerkat_core::event::BackgroundJobTerminalStatus::Completed,
            detail: None,
            persisted: false,
        };
        mixed.blocks.push(refresh_block.clone());
        let messages = vec![
            user("older scaffold"),
            Message::SystemNotice(mixed),
            Message::SystemNotice(SystemNoticeMessage::with_blocks(
                SystemNoticeKind::BackgroundJob,
                None,
                vec![refresh_block],
            )),
            Message::SystemNotice(SystemNoticeMessage::new(
                SystemNoticeKind::Generic,
                "Background fork_off job job-a finished (completed)",
            )),
            user("newer scaffold"),
        ];
        let roles = roles(&messages);
        assert_eq!(roles[1], HygieneRole::BackgroundJobResult);
        assert_eq!(roles[2], HygieneRole::SystemNotice);
        assert_eq!(roles[3], HygieneRole::SystemNotice);
        let rejected = validate_revision(
            &RevisionProposal {
                ops: vec![collapse(0, 2)],
            },
            &roles,
            &[],
            OrderingContext::SequencedAfterHarvest,
        );
        assert!(
            matches!(rejected, Err(RevisionReject::IllegalRole { .. })),
            "{rejected:?}"
        );
        let ordinary = validate_revision(
            &RevisionProposal {
                ops: vec![collapse(2, 5)],
            },
            &roles,
            &[],
            OrderingContext::SequencedAfterHarvest,
        );
        assert!(ordinary.is_ok(), "{ordinary:?}");
        assert!(render_transcript(&messages).contains("[1] background job result: "));
    }

    #[test]
    fn validator_hard_blocks_quarantine_referenced_spans() {
        let messages = transcript();
        let spans = vec![SpanReference {
            record_id: "mem-q".to_string(),
            quarantined: true,
            range: Some((2, 2)),
        }];
        let result = validate_revision(
            &RevisionProposal {
                ops: vec![prune(2, 3)],
            },
            &roles(&messages),
            &spans,
            OrderingContext::SequencedAfterHarvest,
        );
        assert!(
            matches!(
                result,
                Err(RevisionReject::QuarantineReferenced { ref record_id }) if record_id == "mem-q"
            ),
            "{result:?}"
        );
        // A rangeless quarantined citation blocks the whole session.
        let spans = vec![SpanReference {
            record_id: "mem-q2".to_string(),
            quarantined: true,
            range: None,
        }];
        let result = validate_revision(
            &RevisionProposal {
                ops: vec![collapse(4, 7)],
            },
            &roles(&messages),
            &spans,
            OrderingContext::SequencedAfterHarvest,
        );
        assert!(matches!(
            result,
            Err(RevisionReject::QuarantineReferenced { .. })
        ));
    }

    #[test]
    fn validator_flags_active_spans_without_blocking() {
        let messages = transcript();
        let spans = vec![
            SpanReference {
                record_id: "mem-a".to_string(),
                quarantined: false,
                range: Some((2, 2)),
            },
            SpanReference {
                record_id: "mem-elsewhere".to_string(),
                quarantined: false,
                range: Some((7, 7)),
            },
        ];
        let validated = validate_revision(
            &RevisionProposal {
                ops: vec![prune(2, 3)],
            },
            &roles(&messages),
            &spans,
            OrderingContext::SequencedAfterHarvest,
        )
        .expect("active spans flag, not block");
        assert_eq!(validated.flagged_active_records, vec!["mem-a".to_string()]);
    }

    #[test]
    fn validator_refuses_when_ordering_invariant_unmet() {
        let messages = transcript();
        let result = validate_revision(
            &RevisionProposal {
                ops: vec![prune(2, 3)],
            },
            &roles(&messages),
            &[],
            OrderingContext::Cursor(Some(1)),
        );
        assert!(
            matches!(
                result,
                Err(RevisionReject::OrderingUnmet {
                    cursor: 1,
                    needed: 3
                })
            ),
            "{result:?}"
        );
        // Cursor beyond the hull passes; no gate (no distiller) passes.
        assert!(
            validate_revision(
                &RevisionProposal {
                    ops: vec![prune(2, 3)],
                },
                &roles(&messages),
                &[],
                OrderingContext::Cursor(Some(3)),
            )
            .is_ok()
        );
        assert!(
            validate_revision(
                &RevisionProposal {
                    ops: vec![prune(2, 3)],
                },
                &roles(&messages),
                &[],
                OrderingContext::Cursor(None),
            )
            .is_ok()
        );
    }

    // -- replacement construction ----------------------------------------------

    #[test]
    fn replacement_preserves_pairing_and_collapses_runs() {
        for is_error in [false, true] {
            let mut messages = transcript();
            let failures = vec![crate::test_diagnostics::settlement_failure(is_error)];
            let host_metadata = std::collections::BTreeMap::from([(
                "test:application".into(),
                serde_json::json!({"result": {"_meta": {"private": "original"}}}),
            )]);
            let original_timestamp = match &mut messages[2] {
                Message::ToolResults {
                    results,
                    created_at,
                } => {
                    results[0].is_error = is_error;
                    results[0].settlement_failures = failures.clone();
                    results[0].host_metadata = host_metadata.clone();
                    *created_at
                }
                other => panic!("expected source tool results, got {other:?}"),
            };
            let original = messages.clone();
            let ops = vec![prune(2, 3), collapse(4, 7)];
            let (start, end, replacement) =
                build_replacement(&messages, &ops).expect("ops produce a hull");
            assert_eq!((start, end), (2, 7));
            // [2] pruned tool results, [3] untouched assistant, [4..7) one notice.
            assert_eq!(replacement.len(), 3);
            match &replacement[0] {
                Message::ToolResults {
                    results,
                    created_at,
                } => {
                    assert_eq!(results[0].tool_use_id, "call-1");
                    assert_eq!(results[0].is_error, is_error);
                    assert_eq!(results[0].settlement_failures, failures);
                    assert_eq!(results[0].host_metadata, host_metadata);
                    assert_eq!(*created_at, original_timestamp);
                    let text = meerkat_core::types::text_content(&results[0].content);
                    assert!(text.contains("[pruned by hygienist"), "{text}");
                }
                other => panic!("expected tool results, got {other:?}"),
            }
            match &replacement[1] {
                Message::BlockAssistant(assistant) => {
                    assert!(assistant.text_blocks().any(|text| text.contains("ship")));
                }
                other => panic!("expected assistant, got {other:?}"),
            }
            match &replacement[2] {
                Message::SystemNotice(notice) => {
                    let body = notice.body.as_deref().unwrap_or_default();
                    assert!(body.contains("collapsed 3 messages"), "{body}");
                    assert!(body.contains("repeated scaffolding"), "{body}");
                }
                other => panic!("expected system notice, got {other:?}"),
            }
            assert_eq!(messages, original, "source transcript is not rewritten");
        }
    }

    // -- engine flow -------------------------------------------------------------

    struct ScriptedSeam {
        messages: Vec<Message>,
        rewrites: StdMutex<Vec<(usize, usize, usize)>>,
        refuse: bool,
        /// CAS value the engine forwarded from read_messages to rewrite. The
        /// outer `Option` records whether rewrite was called at all; the inner
        /// is the forwarded `expected_parent_revision` — two distinct facts.
        #[allow(clippy::option_option)]
        last_expected_parent: StdMutex<Option<Option<String>>>,
    }

    // The head this scripted seam reports at read time — the CAS value the
    // engine must forward verbatim to the rewrite.
    const SCRIPTED_HEAD_REVISION: &str = "rev-head-at-read";

    #[async_trait]
    impl TranscriptRevisionSeam for ScriptedSeam {
        async fn read_messages(
            &self,
            _session_key: &str,
        ) -> Result<Option<(Vec<Message>, Option<String>)>, String> {
            Ok(Some((
                self.messages.clone(),
                Some(SCRIPTED_HEAD_REVISION.to_string()),
            )))
        }

        async fn rewrite(
            &self,
            _session_key: &str,
            start: usize,
            end: usize,
            replacement: Vec<Message>,
            _note: &str,
            expected_parent_revision: Option<String>,
        ) -> Result<AppliedRevision, String> {
            *self
                .last_expected_parent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(expected_parent_revision);
            if self.refuse {
                return Err("session is running".to_string());
            }
            self.rewrites
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((start, end, replacement.len()));
            Ok(AppliedRevision {
                parent_revision: "rev-parent".to_string(),
                revision: "rev-new".to_string(),
                message_count: self.messages.len() - (end - start) + replacement.len(),
            })
        }
    }

    struct ScriptedSpans(Vec<SpanReference>);

    #[async_trait]
    impl SpanReferenceSource for ScriptedSpans {
        async fn span_references(
            &self,
            _identity: &str,
            _session_key: &str,
        ) -> Result<Vec<SpanReference>, String> {
            Ok(self.0.clone())
        }
    }

    struct ScriptedLlm {
        reply: String,
        stop_reason: StopReason,
    }

    #[async_trait]
    impl LlmClient for ScriptedLlm {
        fn stream<'a>(&'a self, _request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
            let reply = self.reply.clone();
            Box::pin(futures::stream::iter(vec![
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
        reply: String,
        stop_reason: StopReason,
    }

    #[async_trait]
    impl HygienistClientHandle for ScriptedHandle {
        async fn client(&self) -> Result<Arc<dyn LlmClient>, HygienistError> {
            Ok(Arc::new(ScriptedLlm {
                reply: self.reply.clone(),
                stop_reason: self.stop_reason,
            }))
        }
    }

    struct FixedGate(u64);

    impl DistillationGate for FixedGate {
        fn distilled_through(&self, _identity: &str, _session_key: &str) -> u64 {
            self.0
        }
    }

    fn engine_with(
        reply: &str,
        seam: Arc<ScriptedSeam>,
        spans: Vec<SpanReference>,
        gate: Option<Arc<dyn DistillationGate>>,
        runs_per_day: u32,
    ) -> Arc<HygienistEngine> {
        engine_stopping_with(reply, StopReason::EndTurn, seam, spans, gate, runs_per_day)
    }

    /// An engine whose every completion streams `reply` and then stops with
    /// `stop_reason`.
    fn engine_stopping_with(
        reply: &str,
        stop_reason: StopReason,
        seam: Arc<ScriptedSeam>,
        spans: Vec<SpanReference>,
        gate: Option<Arc<dyn DistillationGate>>,
        runs_per_day: u32,
    ) -> Arc<HygienistEngine> {
        Arc::new(HygienistEngine::new(
            HygienistProfile::embedded_default(),
            HygienistConfig {
                enabled: true,
                runs_per_day,
                ..HygienistConfig::default()
            },
            Arc::new(ScriptedHandle {
                reply: reply.to_string(),
                stop_reason,
            }),
            seam,
            Arc::new(ScriptedSpans(spans)),
            gate,
            "team",
        ))
    }

    fn seam() -> Arc<ScriptedSeam> {
        Arc::new(ScriptedSeam {
            messages: transcript(),
            rewrites: StdMutex::new(Vec::new()),
            refuse: false,
            last_expected_parent: StdMutex::new(None),
        })
    }

    const PRUNE_REPLY: &str =
        r#"{"ops": [{"op": "prune_tool_results", "range": [2, 3], "rationale": "dead"}]}"#;

    #[tokio::test]
    async fn engine_applies_validated_revision_and_emits_events() {
        let scripted = seam();
        let engine = engine_with(PRUNE_REPLY, scripted.clone(), Vec::new(), None, 2);
        let sink = Arc::new(crate::memory::events::CollectingEventSink::new());
        engine.set_event_sink(sink.clone());
        let outcome = engine
            .hygiene_now("identity:luka", "sess-1", HygieneCause::PostCompaction)
            .await;
        match outcome {
            HygieneOutcome::Applied { revision, ops, .. } => {
                assert_eq!(revision.revision, "rev-new");
                assert_eq!(ops, 1);
            }
            other => panic!("expected Applied, got {other:?}"),
        }
        assert_eq!(
            scripted
                .rewrites
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[(2, 3, 1)]
        );
        // Ask 4 refinement: the head observed at read time is forwarded to the
        // rewrite as the compare-and-swap parent (no longer None).
        assert_eq!(
            scripted
                .last_expected_parent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            Some(Some(SCRIPTED_HEAD_REVISION.to_string())),
            "hygiene must CAS against the head it read"
        );
        assert_eq!(
            sink.types(),
            vec!["memory.hygiene.proposed", "memory.hygiene.applied"]
        );
    }

    #[tokio::test]
    async fn engine_blocks_quarantine_referenced_revision() {
        let scripted = seam();
        let spans = vec![SpanReference {
            record_id: "mem-q".to_string(),
            quarantined: true,
            range: Some((2, 2)),
        }];
        let engine = engine_with(PRUNE_REPLY, scripted.clone(), spans, None, 2);
        let sink = Arc::new(crate::memory::events::CollectingEventSink::new());
        engine.set_event_sink(sink.clone());
        let outcome = engine
            .hygiene_now("identity:luka", "sess-1", HygieneCause::PostCompaction)
            .await;
        assert!(
            matches!(outcome, HygieneOutcome::Blocked { .. }),
            "{outcome:?}"
        );
        assert!(
            scripted
                .rewrites
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "blocked revision must not reach the seam"
        );
        assert_eq!(sink.types(), vec!["memory.hygiene.blocked"]);
    }

    /// A reply the model did not finish applies no revision, even one that
    /// validates: the pass is skipped, and its reason and timeline event name
    /// the stop.
    #[tokio::test]
    async fn engine_applies_nothing_from_an_incomplete_reply() {
        use crate::test_diagnostics::INCOMPLETE_STOPS;

        for stop in INCOMPLETE_STOPS {
            let scripted = seam();
            let engine =
                engine_stopping_with(PRUNE_REPLY, stop, scripted.clone(), Vec::new(), None, 2);
            let sink = Arc::new(crate::memory::events::CollectingEventSink::new());
            engine.set_event_sink(sink.clone());
            let outcome = engine
                .hygiene_now("identity:luka", "sess-1", HygieneCause::PostCompaction)
                .await;
            let named = format!("{stop:?}");
            assert!(
                matches!(&outcome, HygieneOutcome::Skipped { reason } if reason.contains(&named)),
                "{stop:?}: {outcome:?}"
            );
            assert!(
                scripted
                    .rewrites
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_empty(),
                "{stop:?}: an unfinished reply must not reach the seam"
            );
            let events = sink
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(
                matches!(
                    events.as_slice(),
                    [MemoryTimelineEvent::HygieneSkipped { reason, .. }] if reason.contains(&named)
                ),
                "{stop:?}: {events:?}"
            );
        }
    }

    #[tokio::test]
    async fn engine_refuses_on_demand_beyond_distiller_cursor() {
        let scripted = seam();
        let engine = engine_with(
            PRUNE_REPLY,
            scripted.clone(),
            Vec::new(),
            Some(Arc::new(FixedGate(1))),
            2,
        );
        let outcome = engine
            .hygiene_now("identity:luka", "sess-1", HygieneCause::OnDemand)
            .await;
        assert!(
            matches!(outcome, HygieneOutcome::Blocked { .. }),
            "{outcome:?}"
        );
        // The same pass sequenced behind the harvest is fine.
        let outcome = engine
            .hygiene_now("identity:luka", "sess-1", HygieneCause::PostCompaction)
            .await;
        assert!(
            matches!(outcome, HygieneOutcome::Applied { .. }),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn engine_skips_noop_and_respects_budget() {
        let scripted = seam();
        let engine = engine_with(r#"{"ops": []}"#, scripted.clone(), Vec::new(), None, 1);
        let outcome = engine
            .hygiene_now("identity:luka", "sess-1", HygieneCause::PostCompaction)
            .await;
        assert!(
            matches!(&outcome, HygieneOutcome::Skipped { reason } if reason.contains("no-op")),
            "{outcome:?}"
        );
        // The no-op burned the single budgeted run; the next pass is denied.
        let outcome = engine
            .hygiene_now("identity:luka", "sess-1", HygieneCause::PostCompaction)
            .await;
        assert!(
            matches!(&outcome, HygieneOutcome::Skipped { reason } if reason.contains("budget denied")),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn engine_reports_seam_refusal_as_skip() {
        let scripted = Arc::new(ScriptedSeam {
            messages: transcript(),
            rewrites: StdMutex::new(Vec::new()),
            refuse: true,
            last_expected_parent: StdMutex::new(None),
        });
        let engine = engine_with(PRUNE_REPLY, scripted, Vec::new(), None, 2);
        let outcome = engine
            .hygiene_now("identity:luka", "sess-1", HygieneCause::PostCompaction)
            .await;
        assert!(
            matches!(&outcome, HygieneOutcome::Skipped { reason } if reason.contains("apply refused")),
            "{outcome:?}"
        );
    }

    // -- trigger sequencing -------------------------------------------------------

    #[tokio::test]
    async fn follow_up_runs_after_satisfied_harvest_and_withholds_otherwise() {
        let scripted = seam();
        let engine = engine_with(PRUNE_REPLY, scripted.clone(), Vec::new(), None, 4);
        let sink = Arc::new(crate::memory::events::CollectingEventSink::new());
        engine.set_event_sink(sink.clone());
        let follow_up = distiller_follow_up(engine);

        follow_up(
            "identity:luka",
            "sess-1",
            &DistillOutcome::Skipped {
                reason: "budget denied: window budget exhausted (2/2 runs)".to_string(),
            },
        );
        // The withheld pass emits synchronously; nothing was spawned.
        assert_eq!(sink.types(), vec!["memory.hygiene.skipped"]);

        follow_up(
            "identity:luka",
            "sess-1",
            &DistillOutcome::Completed {
                run_id: "distill-1".to_string(),
                written: 0,
                quarantined: 0,
            },
        );
        // The satisfied harvest spawns a detached pass; wait for it.
        for _ in 0..100 {
            if scripted
                .rewrites
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len()
                == 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            scripted
                .rewrites
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1,
            "satisfied harvest must trigger the pass"
        );
    }

    // -- profile ----------------------------------------------------------------

    #[test]
    fn embedded_prompt_matches_calibration_bundle() -> Result<(), Box<dyn std::error::Error>> {
        let bundle = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/memory-evals/prompts/hygienist-v0.md");
        if !bundle.is_file() {
            return Ok(());
        }
        let text = std::fs::read_to_string(bundle)?;
        assert_eq!(
            text, EMBEDDED_PROMPT_V0,
            "tests/memory-evals/prompts/hygienist-v0.md and src/memory/hygienist_prompt_v0.md have drifted"
        );
        Ok(())
    }

    #[test]
    fn model_override_is_fail_loud() {
        let profile = HygienistProfile::embedded_default();
        assert!(profile.clone().with_model_override("").is_err());
        assert!(
            profile
                .clone()
                .with_model_override("not-a-real-model-xyz")
                .is_err()
        );
        let overridden = profile
            .with_model_override("claude-haiku-4-5")
            .expect("catalog model accepted");
        assert_eq!(overridden.model, "claude-haiku-4-5");
    }

    /// The budget override is the reachable half of the output-ceiling fix: a
    /// zero budget is a typed error, not a pass that silently revises nothing.
    #[test]
    fn max_output_tokens_override_is_fail_loud() {
        let profile = HygienistProfile::embedded_default();
        assert!(profile.clone().with_max_output_tokens(0).is_err());
        let raised = profile
            .with_max_output_tokens(32_768)
            .expect("nonzero budget accepted");
        assert_eq!(raised.params.max_output_tokens, 32_768);
    }
}
