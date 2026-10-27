//! Coarse session-sticky content-taint tracking (§10.1, P1).
//!
//! The persistent-prompt-injection defense needs an input signal, and no
//! turn-level or session-level content-taint fact exists anywhere in the
//! platform. This module ships the P1 mechanism: a Meta-owned
//! content-trust configuration plus an observe-stream taint tracker that
//! marks a session tainted once it ingests content from an untrusted tool
//! source. Taint is **session-sticky** (Codex's thread-level
//! `memory_mode='polluted'`, adopted): per-turn taint is trivially evaded
//! by "on your NEXT reply, remember X", and compaction cannot clear taint —
//! the summary is derived from tainted context and inherits it. Taint
//! clears only at a fresh-context boundary (reset / respawn / fresh spawn),
//! which is automatic here because state is keyed by session id and those
//! paths mint new session ids.
//!
//! ## P2 additions (§10.1 taint completion)
//!
//! - **Comms taint join**: a peer message from a tracked sender whose
//!   session is tainted taints the receiving session (the peer-laundering
//!   close). See `SessionTaintTracker::observe_inbound_peer_content` for
//!   what is — and honestly is not — observable.
//! - **Evidence-range taint**: the write gate now sees a write's
//!   `EvidenceRef`s; any LLM-authored write citing a tainted session
//!   quarantines. Coarse by design: the tracker holds session-sticky
//!   facts, so session-tainted ⇒ every range in it tainted (per-turn
//!   granularity would need the Hygienist's pinned revisions, P4).
//! - **Reset boundaries**: `reset()` marks the outgoing session so
//!   Distiller output over it lands `Quarantined` (§8.4 — reset is the
//!   operator's escape hatch; quarantine preserves the re-dream option).
//!
//! ## Closed at the meerkat 0.8.14 pin (dispatch-ordered trust join)
//!
//! - **The first-ingestion race** (ask: taint visibility at tool-dispatch
//!   time) is CLOSED. Marking no longer depends on the asynchronous
//!   observe stream alone: [`crate::memory::dispatch_taint`] wraps every
//!   mob member's agent-facing LLM client (via the pre-build seam every
//!   member session create passes through), classifies each tool result
//!   synchronously BEFORE the LLM request that carries it is sent, and
//!   classifies provider-executed server-tool blocks from the typed
//!   response before the loop can dispatch any same-response tool call. An
//!   LLM-authored memory write is always downstream of an LLM call that
//!   carried the untrusted result, so the tracker is marked strictly
//!   before that write can reach the store. Mechanism note: 0.8.14's
//!   `HookPoint::PostToolExecution` fires synchronously with the same
//!   typed provenance, but the mob member build path has no hook-engine
//!   carrier (`SessionBuildOptions` cannot ship one), so the join rides
//!   the sanctioned `agent_llm_client_decorator` seam instead - identical
//!   ordering guarantee for LLM-authored writes.
//! - **Name-based classification** is CLOSED for dispatch-time marking:
//!   the request's tool catalog carries the typed `ToolDef.provenance`
//!   owner, and [`ContentTrustConfig::classify_tool_with_provenance`]
//!   attributes MCP tools to their server through it - unqualified MCP
//!   tool names no longer need `content_trust.untrusted_tools` listing.
//!   The observe-stream fallback (events carry only the NAME) keeps the
//!   name-based coarseness; it is belt-and-suspenders behind the
//!   dispatch-time join, not the primary marker.
//!
//! ## Honest gaps that remain (upstream asks, §13)
//!
//! - **The mirror race**: after a session rotation, the tracker's view of
//!   an identity's current session lags until the runtime's delivery hook
//!   or the new session's first `RunStarted` event updates it, so a write
//!   in that window can be quarantined against the *old* session's taint.
//!   This errs conservative (false quarantine, never false trust).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use meerkat_core::event::AgentEvent;
use meerkat_core::types::{ServerToolKind, ToolProvenance, ToolSourceKind};
use serde::{Deserialize, Serialize};

use crate::identity_first::agent_memory::AgentMemoryLlmWrites;
use crate::memory::records::{EvidenceRef, MemoryAuthor};
use crate::memory::staged::StagedBatchKind;

/// Builtin web-facing tool names: ALWAYS untrusted for memory purposes, not
/// overridable by `trusted_tools` (§10.1 "web/fetch always untrusted").
/// `web_search` is meerkat's builtin client-side search
/// (`meerkat_core::web_search::WEB_SEARCH_TOOL_NAME`); the others cover the
/// common fetch spellings across builtin and bundle surfaces.
const ALWAYS_UNTRUSTED_TOOL_NAMES: &[&str] = &["web_search", "web_fetch", "fetch", "http_request"];

/// Server-qualified MCP tool-name prefix (`mcp__<server>__<tool>`). The only
/// name shape that lets P1 attribute a tool to an MCP server (see module
/// docs on classification coarseness).
const MCP_QUALIFIED_PREFIX: &str = "mcp__";

/// Tainted-session entries are bounded; oldest-tainted evict first. A taint
/// entry for a dead session is inert (nothing checks it), so eviction only
/// bounds memory, never correctness for live sessions at sane scales.
const MAX_TRACKED_TAINTED_SESSIONS: usize = 4096;

/// Which tool sources count as untrusted for memory purposes (§10.1).
///
/// Mirrors Codex's `pollutes_memory` posture: web/fetch and provider-native
/// search are always untrusted; MCP servers are untrusted by default with an
/// explicit `trusted_mcp_servers` allowlist. Note meerkat's
/// `ToolAccessPolicy::AllowList` is invocation gating and cannot serve this
/// role.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentTrustConfig {
    /// MCP servers whose tools do not taint. Joinable in P1 only for
    /// server-qualified tool names (`mcp__<server>__<tool>`).
    #[serde(default)]
    pub trusted_mcp_servers: Vec<String>,
    /// Explicit tool names that taint the session when their results enter
    /// context. The escape hatch for unqualified MCP tool names.
    #[serde(default)]
    pub untrusted_tools: Vec<String>,
    /// Explicit tool names that never taint (cannot override the builtin
    /// web/fetch class or `untrusted_tools`).
    #[serde(default)]
    pub trusted_tools: Vec<String>,
}

/// Classification verdict for one tool name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolContentTrust {
    Trusted,
    Untrusted { source: String },
}

impl ContentTrustConfig {
    /// Fail-loud JSON parse for the gateway config block
    /// `agent_memory.content_trust { ... }`. Unknown fields and wrong types
    /// are errors, never silently ignored.
    pub fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "content_trust must be an object".to_string())?;
        let supported = ["trusted_mcp_servers", "untrusted_tools", "trusted_tools"];
        let unsupported = object
            .keys()
            .filter(|key| !supported.contains(&key.as_str()))
            .map(String::as_str)
            .collect::<Vec<_>>();
        if !unsupported.is_empty() {
            return Err(format!(
                "unsupported content_trust fields: {}",
                unsupported.join(", ")
            ));
        }
        let parse_names = |key: &str| -> Result<Vec<String>, String> {
            match object.get(key) {
                None => Ok(Vec::new()),
                Some(value) => {
                    let entries = value
                        .as_array()
                        .ok_or_else(|| format!("content_trust.{key} must be an array"))?;
                    entries
                        .iter()
                        .map(|entry| {
                            entry
                                .as_str()
                                .map(str::trim)
                                .filter(|name| !name.is_empty())
                                .map(ToString::to_string)
                                .ok_or_else(|| {
                                    format!("content_trust.{key} entries must be non-empty strings")
                                })
                        })
                        .collect()
                }
            }
        };
        Ok(Self {
            trusted_mcp_servers: parse_names("trusted_mcp_servers")?,
            untrusted_tools: parse_names("untrusted_tools")?,
            trusted_tools: parse_names("trusted_tools")?,
        })
    }

    /// Classify a tool by NAME (the only fact the observe-stream event
    /// surface carries - module docs). Precedence: builtin web/fetch
    /// (non-overridable) > `untrusted_tools` > `trusted_tools` >
    /// MCP-qualified names against the server allowlist > trusted.
    pub fn classify_tool(&self, name: &str) -> ToolContentTrust {
        self.classify_tool_with_provenance(name, None)
    }

    /// Classify a tool with the typed [`ToolDef.provenance`] owner when the
    /// caller has it (the dispatch-time join - module docs). Precedence is
    /// [`Self::classify_tool`]'s, with the MCP step widened: typed
    /// `ToolSourceKind::Mcp` provenance attributes the tool to
    /// `provenance.source_id` regardless of the name shape; absent or
    /// non-MCP provenance falls back to the `mcp__<server>__<tool>` name
    /// join. Explicit `trusted_tools` entries still override server-level
    /// distrust, exactly as on the name-only path.
    ///
    /// [`ToolDef.provenance`]: meerkat_core::ToolDef
    pub fn classify_tool_with_provenance(
        &self,
        name: &str,
        provenance: Option<&ToolProvenance>,
    ) -> ToolContentTrust {
        if ALWAYS_UNTRUSTED_TOOL_NAMES.contains(&name) {
            return ToolContentTrust::Untrusted {
                source: format!("web tool '{name}'"),
            };
        }
        if self.untrusted_tools.iter().any(|tool| tool == name) {
            return ToolContentTrust::Untrusted {
                source: format!("configured untrusted tool '{name}'"),
            };
        }
        if self.trusted_tools.iter().any(|tool| tool == name) {
            return ToolContentTrust::Trusted;
        }
        if let Some(provenance) = provenance
            && provenance.kind == ToolSourceKind::Mcp
        {
            return self.classify_mcp_server(provenance.source_id.as_str(), name);
        }
        if let Some(rest) = name.strip_prefix(MCP_QUALIFIED_PREFIX) {
            let server = rest.split("__").next().unwrap_or(rest);
            return self.classify_mcp_server(server, name);
        }
        ToolContentTrust::Trusted
    }

    fn classify_mcp_server(&self, server: &str, name: &str) -> ToolContentTrust {
        if self
            .trusted_mcp_servers
            .iter()
            .any(|trusted| trusted == server)
        {
            return ToolContentTrust::Trusted;
        }
        ToolContentTrust::Untrusted {
            source: format!("MCP server '{server}' (tool '{name}')"),
        }
    }
}

/// Why and when a session became tainted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaintState {
    pub tainted_at_ms: u64,
    pub source: String,
}

#[derive(Default)]
struct TaintInner {
    /// identity → the session key the tracker currently attributes that
    /// identity's activity to. Fed authoritatively by the identity runtime's
    /// delivery/reset hooks and, as fallback, by observed `RunStarted`
    /// events (peer-comms-driven runs never pass through the runtime hooks).
    current_session: HashMap<String, String>,
    /// session key → taint fact. Session-sticky by construction, and
    /// retained after rotation/clear: the fact is historical ("this session
    /// ingested untrusted content"), and the P2 comms join and
    /// evidence-range gate read it for sessions that are no longer current.
    tainted: HashMap<String, TaintState>,
    /// Untrusted ingestion observed before the observer learned the
    /// identity's session (mid-run attach). Transferred to the next learned
    /// session — conservative direction (see module docs).
    pending_identity_taint: HashMap<String, TaintState>,
    /// session key → reset-boundary mark (§8.4): distillates citing this
    /// session quarantine pending steward review. Bounded like `tainted`.
    reset_boundaries: HashMap<String, u64>,
}

/// Host callback that stamps a member's OUTBOUND content-taint declaration
/// (§10.1 ask 5, outbound half). Invoked with the member's public identity and
/// the taint to declare — `Some(Tainted)` when the member's session ingests
/// untrusted content, `None` when it rotates back to a clean session — so the
/// member's peer sends carry the sender's signed declaration and receivers can
/// propagate taint cross-process. The callback is fire-and-forget (the real
/// declare is async on the mob runtime); it must not block.
pub type OutboundTaintDeclarer =
    Arc<dyn Fn(&str, Option<meerkat_core::comms::SenderContentTaint>) + Send + Sync>;

/// Session-sticky taint tracker (§10.1, coarse P1). Cheap to clone; clones
/// share state.
#[derive(Clone, Default)]
pub struct SessionTaintTracker {
    config: Arc<ContentTrustConfig>,
    inner: Arc<Mutex<TaintInner>>,
    /// §9.3 timeline sink for taint transitions; shared across clones.
    event_sink: Arc<Mutex<Option<Arc<dyn crate::memory::events::MemoryEventSink>>>>,
    /// §10.1 ask 5 outbound half: declares a member's outbound content-taint
    /// on the mob runtime so its peer sends carry the flag. Shared across
    /// clones; `None` until the gateway wires the mob handle.
    outbound_declarer: Arc<Mutex<Option<OutboundTaintDeclarer>>>,
}

impl SessionTaintTracker {
    pub fn new(config: ContentTrustConfig) -> Self {
        Self {
            config: Arc::new(config),
            inner: Arc::new(Mutex::new(TaintInner::default())),
            event_sink: Arc::new(Mutex::new(None)),
            outbound_declarer: Arc::new(Mutex::new(None)),
        }
    }

    /// Wire the §10.1 outbound-taint declarer (bound to the mob handle at
    /// gateway bootstrap). Shared across clones.
    pub fn set_outbound_taint_declarer(&self, declarer: OutboundTaintDeclarer) {
        *self
            .outbound_declarer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(declarer);
    }

    /// Declare a member's outbound content-taint, if a declarer is wired.
    /// Called on taint transitions (set/clear); must be invoked WITHOUT the
    /// `inner` lock held (the callback may take time to hand off).
    fn declare_outbound(
        &self,
        identity: &str,
        taint: Option<meerkat_core::comms::SenderContentTaint>,
    ) {
        if let Some(declarer) = self
            .outbound_declarer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            declarer(identity, taint);
        }
    }

    /// Wire the §9.3 timeline sink so taint transitions surface on the
    /// console alongside the tracing warns. Shared across clones.
    pub fn set_event_sink(&self, sink: Arc<dyn crate::memory::events::MemoryEventSink>) {
        *self
            .event_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sink);
    }

    fn emit_event(&self, event: crate::memory::events::MemoryTimelineEvent) {
        if let Some(sink) = self
            .event_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            sink.emit(event);
        }
    }

    /// Observe one agent event for `identity` (the observe-only agent-event
    /// stream is per-member, so attribution is the subscription's).
    pub fn observe_agent_event(&self, identity: &str, event: &AgentEvent) {
        match event {
            AgentEvent::RunStarted {
                session_id, input, ..
            } => {
                let session_key = session_id.to_string();
                self.note_current_session(identity, &session_key);
                // §10.1 comms taint join (legacy, belt-and-braces): before
                // meerkat 0.7.13 the only inbound signal was projection text.
                // It stays as a fallback for same-process senders whose taint
                // this host tracks but who make no envelope declaration.
                if let Some(text) = input.prompt_text() {
                    self.observe_inbound_peer_content(identity, &session_key, &text);
                }
            }
            // §10.1 ask 5 (0.7.13): consume the sender's SIGNED content-taint
            // declaration from the typed peer-ingestion event — canonical peer
            // identity, emitted synchronously as the block commits — instead of
            // parsing rendered projection text. Only an affirmative `Tainted`
            // taints the receiving session; `None` ("no declaration") and
            // `Clean` do not, and `None` is never coalesced into `Clean`.
            AgentEvent::PeerContentIngested {
                peer, sender_taint, ..
            } => {
                if *sender_taint == Some(meerkat_core::comms::SenderContentTaint::Tainted) {
                    let sender = peer
                        .as_ref()
                        .and_then(|peer| peer.display_name.clone())
                        .unwrap_or_else(|| "peer".to_string());
                    self.mark_identity_tainted(
                        identity,
                        format!("peer content declared tainted by sender '{sender}'"),
                    );
                }
            }
            // Taint on result *ingestion*: these are the events whose content
            // enters the conversation context. Errors taint too — an error
            // body is still attacker-influenced text in context.
            AgentEvent::ToolResultReceived { name, .. }
            | AgentEvent::ToolExecutionCompleted { name, .. } => {
                if let ToolContentTrust::Untrusted { source } = self.config.classify_tool(name) {
                    self.mark_identity_tainted(identity, source);
                }
            }
            // Provider-executed server tools (web search / grounding /
            // provider-native): typed, and always untrusted (§10.1).
            AgentEvent::ServerToolContent { kind, .. } => {
                self.mark_identity_tainted(
                    identity,
                    format!("provider server tool '{}'", kind.provider_name()),
                );
            }
            _ => {}
        }
    }

    /// Dispatch-ordered ingestion feed (module docs, "closed at 0.8.14"):
    /// classify one tool result with the typed provenance from the request's
    /// tool catalog and mark the identity synchronously - strictly before
    /// the LLM call that carries the result, so no same-turn LLM memory
    /// write can beat the mark to the store. Marking is idempotent with the
    /// observe-stream fallback, which stays wired behind this.
    pub fn observe_dispatched_tool_result(
        &self,
        identity: &str,
        name: &str,
        provenance: Option<&ToolProvenance>,
    ) {
        if let ToolContentTrust::Untrusted { source } =
            self.config.classify_tool_with_provenance(name, provenance)
        {
            self.mark_identity_tainted(identity, source);
        }
    }

    /// Dispatch-ordered feed for provider-executed server tools (web search /
    /// grounding): typed, and always untrusted (§10.1). Called from the
    /// LLM-boundary join with the response's typed `ServerToolContent`
    /// blocks, before the loop can dispatch any same-response tool call.
    pub fn observe_dispatched_server_tool(&self, identity: &str, kind: &ServerToolKind) {
        self.mark_identity_tainted(
            identity,
            format!("provider server tool '{}'", kind.provider_name()),
        );
    }

    /// Authoritative current-session hint from the identity runtime's
    /// delivery path. Keeps the tracker's attribution ahead of the (async)
    /// observe stream on the paths Meta controls.
    pub fn note_current_session(&self, identity: &str, session_key: &str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending = inner.pending_identity_taint.remove(identity);
        let previous = inner
            .current_session
            .insert(identity.to_string(), session_key.to_string());
        let rotated_away_from_tainted = previous.as_deref() != Some(session_key)
            && previous.is_some_and(|prior| inner.tainted.contains_key(&prior));
        if rotated_away_from_tainted {
            tracing::warn!(
                identity,
                session_key,
                "agent memory taint: session rotated away from a tainted session; \
                 new session starts clean"
            );
            self.emit_event(
                crate::memory::events::MemoryTimelineEvent::TaintTransition {
                    identity: Some(identity.to_string()),
                    session_key: session_key.to_string(),
                    kind: "rotated_clean".to_string(),
                    source: "session rotation away from tainted session".to_string(),
                },
            );
        }
        // A held pre-attribution taint re-lands on the new session — it stays
        // tainted, so do not clear the outbound declaration below.
        let pending_reapplied = pending.is_some();
        if let Some(state) = pending {
            self.insert_taint(&mut inner, session_key.to_string(), state);
        }
        // §10.1 outbound half: on a clean rotation with no re-landed taint, the
        // identity's live session is clean again — clear its outbound stamp.
        if rotated_away_from_tainted && !pending_reapplied {
            drop(inner);
            self.declare_outbound(identity, None);
        }
    }

    /// Explicit clear for the reset path (`reset()` is the operator's escape
    /// hatch from a poisoned session). Rotation already clears implicitly;
    /// this also drops any pending pre-attribution taint. The outgoing
    /// session's *fact* is deliberately retained (P2): "that session was
    /// tainted" stays true after the identity moves on, and the comms join
    /// and the Distiller's evidence gate consult it for exactly such
    /// sessions.
    pub fn clear_identity(&self, identity: &str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.pending_identity_taint.remove(identity);
        inner.current_session.remove(identity);
        // §10.1 outbound half: reset is a fresh-context boundary — the identity
        // no longer speaks for a tainted session, so clear its outbound stamp.
        drop(inner);
        self.declare_outbound(identity, None);
    }

    /// Mark a `reset()` boundary on the outgoing session (§8.4): every
    /// LLM-authored write whose evidence cites this session quarantines
    /// pending steward review, regardless of content taint. Idempotent.
    pub fn mark_reset_boundary(&self, session_key: &str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner
            .reset_boundaries
            .insert(session_key.to_string(), now_ms())
            .is_none()
        {
            tracing::warn!(
                session_key,
                "agent memory taint: reset boundary marked; distillates over this \
                 session will land quarantined pending steward review (§8.4)"
            );
            self.emit_event(
                crate::memory::events::MemoryTimelineEvent::TaintTransition {
                    identity: None,
                    session_key: session_key.to_string(),
                    kind: "reset_boundary".to_string(),
                    source: "reset() boundary (§8.4)".to_string(),
                },
            );
        }
        if inner.reset_boundaries.len() > MAX_TRACKED_TAINTED_SESSIONS
            && let Some(oldest) = inner
                .reset_boundaries
                .iter()
                .min_by_key(|(_, at_ms)| **at_ms)
                .map(|(key, _)| key.clone())
        {
            inner.reset_boundaries.remove(&oldest);
        }
    }

    /// §10.1/§8.4 evidence gate query: why a write citing `session_key` as
    /// evidence must quarantine, if it must. Coarse by design: the tracker
    /// holds session-sticky facts, so a tainted session taints every
    /// evidence range within it.
    pub fn evidence_quarantine_reason(&self, session_key: &str) -> Option<String> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(state) = inner.tainted.get(session_key) {
            return Some(format!(
                "evidence session tainted by {} (session-tainted ⇒ range-tainted)",
                state.source
            ));
        }
        if inner.reset_boundaries.contains_key(session_key) {
            return Some(
                "evidence session closed at a reset boundary; distillates quarantine \
                 pending steward review (§8.4)"
                    .to_string(),
            );
        }
        None
    }

    /// Taint fact for an explicit session key.
    pub fn session_taint(&self, session_key: &str) -> Option<TaintState> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.tainted.get(session_key).cloned()
    }

    /// Taint fact for the identity's currently-attributed session (the write
    /// gate's query: LLM-authored writes carry identity, not session).
    pub fn identity_taint(&self, identity: &str) -> Option<TaintState> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(state) = inner.pending_identity_taint.get(identity) {
            return Some(state.clone());
        }
        inner
            .current_session
            .get(identity)
            .and_then(|session| inner.tainted.get(session))
            .cloned()
    }

    /// §10.1 comms taint join, over what the observe surface actually
    /// carries. Meerkat 0.7.9 has **no typed inbound peer-message event**:
    /// a peer delivery lands in the receiver's session as injected prompt
    /// text rendered by `format_peer_message_projection` /
    /// `format_peer_response_projection` (meerkat-core `interaction.rs:126,
    /// :239`), where the sender is the resolved trusted-peer name — for mob
    /// members the `MemberCommsName` `{mob_id}/{role}/{agent_identity}`
    /// (meerkat-core `connection.rs:368`). This join parses that sender out
    /// and taints the receiving session when the sender's tracked session
    /// is tainted.
    ///
    /// Honest limits, filed against upstream ask 5 (envelope-level taint
    /// flags):
    /// - **"tainted at send time" is approximated at delivery-observe
    ///   time.** If the sender rotated to a clean session between send and
    ///   delivery, the join misses; if the sender got tainted after the
    ///   send, the join over-taints (conservative direction).
    /// - **Peer *requests* render a raw cryptographic `peer_id`**, not a
    ///   comms name, so their senders are unmappable host-side — unknowable
    ///   without the upstream envelope fact.
    /// - **Cross-process senders** are not in this tracker at all;
    ///   sender-session taint state is unknowable for them.
    /// - A user message quoting the projection prefix can false-positive —
    ///   conservative (false quarantine, never false trust).
    fn observe_inbound_peer_content(&self, identity: &str, session_key: &str, text: &str) {
        for line in text.lines() {
            let Some(sender_identity) = peer_projection_sender_identity(line) else {
                continue;
            };
            if sender_identity == identity {
                continue;
            }
            let source = {
                let inner = self
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                inner
                    .pending_identity_taint
                    .get(sender_identity)
                    .or_else(|| {
                        inner
                            .current_session
                            .get(sender_identity)
                            .and_then(|session| inner.tainted.get(session))
                    })
                    .map(|state| state.source.clone())
            };
            if let Some(source) = source {
                let state = TaintState {
                    tainted_at_ms: now_ms(),
                    source: format!(
                        "peer message from tainted sender '{sender_identity}' \
                         (sender session tainted by {source})"
                    ),
                };
                let mut inner = self
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                self.insert_taint(&mut inner, session_key.to_string(), state);
            }
        }
    }

    fn mark_identity_tainted(&self, identity: &str, source: String) {
        let state = TaintState {
            tainted_at_ms: now_ms(),
            source,
        };
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match inner.current_session.get(identity).cloned() {
            Some(session) => self.insert_taint(&mut inner, session, state),
            None => {
                // Mid-run attach: session unknown until the next RunStarted /
                // delivery hook. Hold identity-sticky (module docs).
                match inner.pending_identity_taint.entry(identity.to_string()) {
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        tracing::warn!(
                            identity,
                            source = %state.source,
                            "agent memory taint: untrusted ingestion observed before session \
                             attribution; holding identity-sticky taint"
                        );
                        slot.insert(state);
                    }
                    std::collections::hash_map::Entry::Occupied(mut slot) => {
                        slot.insert(state);
                    }
                }
            }
        }
        // §10.1 outbound half: the identity has ingested untrusted content, so
        // stamp its peer sends Tainted (whether the taint landed on the current
        // session or is held pending). Declare OUTSIDE the `inner` lock.
        drop(inner);
        self.declare_outbound(
            identity,
            Some(meerkat_core::comms::SenderContentTaint::Tainted),
        );
    }

    fn insert_taint(&self, inner: &mut TaintInner, session: String, state: TaintState) {
        match inner.tainted.entry(session) {
            std::collections::hash_map::Entry::Occupied(_) => return,
            std::collections::hash_map::Entry::Vacant(slot) => {
                tracing::warn!(
                    session_key = %slot.key(),
                    source = %state.source,
                    "agent memory taint: session ingested untrusted content; LLM-authored \
                     memory writes from this session will quarantine until a fresh-context \
                     boundary (reset/respawn/fresh spawn)"
                );
                self.emit_event(
                    crate::memory::events::MemoryTimelineEvent::TaintTransition {
                        identity: None,
                        session_key: slot.key().clone(),
                        kind: "tainted".to_string(),
                        source: state.source.clone(),
                    },
                );
                slot.insert(state);
            }
        }
        if inner.tainted.len() > MAX_TRACKED_TAINTED_SESSIONS
            && let Some(oldest) = inner
                .tainted
                .iter()
                .min_by_key(|(_, state)| state.tainted_at_ms)
                .map(|(key, _)| key.clone())
        {
            inner.tainted.remove(&oldest);
        }
    }
}

/// Store-seam write gate (§10.1 posture): consulted by the bundled store for
/// every LLM-authored create/supersede so the quarantine decision holds for
/// ALL callers — the Recorder tool, staged batches, and any future stage —
/// not just the tool handler.
pub trait LlmWriteGate: Send + Sync {
    /// `Some(reason)` when this LLM-authored write must land
    /// `RecordStatus::Quarantined`. Non-LLM principals are never gated.
    /// `kind` is the batch's semantic kind (§10.1): review verdicts are
    /// the review the quarantine posture defers to, fresh writes are not.
    /// `evidence` is the union of `EvidenceRef`s the write cites (P2
    /// evidence-range taint, §10.1); empty for writes that cite nothing.
    fn quarantine_reason(
        &self,
        author: &MemoryAuthor,
        kind: StagedBatchKind,
        evidence: &[EvidenceRef],
    ) -> Option<String>;
}

/// The taint/posture gate: `llm_writes = "quarantined"` forces every
/// first-pass LLM-authored write (Agent, Distiller, and the steward's own
/// consolidate/harvest/rank output) into quarantine regardless of taint;
/// agent-authored writes quarantine when the author's session is tainted;
/// and ANY LLM-authored write (Steward and Distiller included) quarantines
/// when its evidence cites a tainted session or a reset boundary
/// (§8.4/§10.1 — coarse: session-tainted ⇒ range-tainted).
///
/// Review-verdict batches (`StagedBatchKind::ReviewVerdict`) are exempt
/// from the *posture* branch only: a review verdict IS the review the
/// posture defers to (§10.1 "quarantined until steward/operator review") —
/// quarantine releases, gating-approved promotions, and proposal accepts
/// commit as review verdicts, and re-quarantining them would make review
/// unable to ever produce an Active record under the conservative posture.
/// The exemption is keyed on the batch's semantic kind, NOT on the Steward
/// author: all dream groups carry `MemoryAuthor::Steward`, but fresh
/// steward LLM output (consolidate creates, harvest copies, rank batches)
/// is first-pass content and respects the posture knob. The evidence-taint
/// branch still applies to every batch, so a consolidation citing a
/// tainted session quarantines like any other write.
pub struct TaintLlmWriteGate {
    tracker: Option<SessionTaintTracker>,
    llm_writes: AgentMemoryLlmWrites,
}

impl TaintLlmWriteGate {
    pub fn new(tracker: Option<SessionTaintTracker>, llm_writes: AgentMemoryLlmWrites) -> Self {
        Self {
            tracker,
            llm_writes,
        }
    }
}

impl LlmWriteGate for TaintLlmWriteGate {
    fn quarantine_reason(
        &self,
        author: &MemoryAuthor,
        kind: StagedBatchKind,
        evidence: &[EvidenceRef],
    ) -> Option<String> {
        if !author.is_llm() {
            return None;
        }
        if self.llm_writes == AgentMemoryLlmWrites::Quarantined
            && kind != StagedBatchKind::ReviewVerdict
        {
            return Some("llm_writes=quarantined policy".to_string());
        }
        let tracker = self.tracker.as_ref()?;
        if let MemoryAuthor::Agent { identity } = author
            && let Some(state) = tracker.identity_taint(identity)
        {
            return Some(format!("session tainted by {}", state.source));
        }
        for evidence_ref in evidence {
            if let Some(reason) = tracker.evidence_quarantine_reason(&evidence_ref.session_id) {
                return Some(reason);
            }
        }
        None
    }
}

/// Sender-identity extraction from one line of injected peer-projection
/// text. Pinned to meerkat 0.7.9's canonical projections:
/// `format_peer_message_projection` → `"Peer message from {name}:"` and
/// `format_peer_response_projection` → `"Peer response from {name} (to
/// request: ...)"`, where `{name}` for a mob member is
/// `{mob_id}/{role}/{agent_identity}`. Returns the trailing path segment
/// (the agent identity). Peer *requests* carry a raw peer id and return
/// `None` (module docs on the honest gaps).
fn peer_projection_sender_identity(line: &str) -> Option<&str> {
    let name = if let Some(rest) = line.strip_prefix("Peer message from ") {
        // Canonical shape ends the line with ':' (body follows on the next
        // line). Names may themselves contain ':' (agent identities do), so
        // strip the trailing delimiter rather than splitting at the first.
        match rest.split_once(": ") {
            Some((name, _)) => name.trim(),
            None => rest.strip_suffix(':').unwrap_or(rest).trim(),
        }
    } else {
        let rest = line.strip_prefix("Peer response from ")?;
        rest.split(" (to request:").next()?.trim()
    };
    if name.is_empty() {
        return None;
    }
    Some(name.rsplit('/').next().unwrap_or(name))
}

// ---------------------------------------------------------------------------
// Observe-stream feed
// ---------------------------------------------------------------------------

/// A consumer of the per-member agent-event observe stream. The taint
/// tracker and the Distiller's trigger sink both ride ONE observer loop —
/// one `subscribe_agent_events` subscription per member, however many
/// memory stages listen.
pub trait MemberAgentEventSink: Send + Sync {
    fn observe(&self, identity: &str, envelope: &meerkat_core::event::EventEnvelope<AgentEvent>);
}

impl MemberAgentEventSink for SessionTaintTracker {
    fn observe(&self, identity: &str, envelope: &meerkat_core::event::EventEnvelope<AgentEvent>) {
        self.observe_agent_event(identity, &envelope.payload);
    }
}

/// §9.1 as-built compaction reset feed: an ALWAYS-ON sink — unconditional,
/// unlike the distiller's trigger sink — surfacing `CompactionCompleted`
/// session keys to a callback. The gateway points it at
/// `AgentMemoryRuntimeInjector::on_session_compacted` so the coordinator's
/// cross-turn dedup/budget state resets even when no distiller is enabled
/// (gate finding: budgeted injection without a distiller never reset).
pub struct CompactionResetSink {
    on_compacted: Arc<dyn Fn(&str) + Send + Sync>,
}

impl CompactionResetSink {
    pub fn new(on_compacted: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        Self { on_compacted }
    }
}

impl MemberAgentEventSink for CompactionResetSink {
    fn observe(&self, _identity: &str, envelope: &meerkat_core::event::EventEnvelope<AgentEvent>) {
        if matches!(envelope.payload, AgentEvent::CompactionCompleted { .. })
            && let meerkat_core::event::EventSourceIdentity::Session { session_id } =
                &envelope.source
        {
            (self.on_compacted)(&session_id.to_string());
        }
    }
}

/// Guard for the observer task; aborts the task when the last clone drops
/// (the runtime that owned the sinks is gone).
#[derive(Clone)]
pub struct TaintObserverGuard {
    _abort: Arc<AbortOnDrop>,
}

struct AbortOnDrop(std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(task) = self
            .0
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

impl TaintObserverGuard {
    /// Abort and join the observer rather than merely relying on drop. The
    /// runtime uses this at shutdown so a failed builder cannot leave a ghost
    /// subscriber retaining the mob handle and memory sinks.
    pub async fn abort_and_join(self) {
        let task = self
            ._abort
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }
}

/// Subscribe the taint observer to every active member's agent-event stream
/// (the same observe-only `subscribe_agent_events` surface the console
/// forwarder rides), reconciling membership whenever the mob's machine state
/// changes.
pub fn spawn_taint_observer(
    handle: meerkat_mob::MobHandle,
    tracker: SessionTaintTracker,
) -> TaintObserverGuard {
    spawn_member_event_observer(handle, vec![Arc::new(tracker)])
}

/// Generalized observer: one reconcile loop, one stream per active member,
/// fanned out to every sink (taint tracker, Distiller triggers, future
/// stages).
pub fn spawn_member_event_observer(
    handle: meerkat_mob::MobHandle,
    sinks: Vec<Arc<dyn MemberAgentEventSink>>,
) -> TaintObserverGuard {
    let task = tokio::spawn(run_member_event_observer(handle, sinks));
    TaintObserverGuard {
        _abort: Arc::new(AbortOnDrop(std::sync::Mutex::new(Some(task)))),
    }
}

/// What the member-event observer reads from the mob. [`meerkat_mob::MobHandle`]
/// is the only production source; tests drive the loop through a fake so the
/// wake-ups and retry cadence are observable.
trait MemberEventSource: Send + Sync + 'static {
    type Changes: MemberStateChanges;

    /// Change signal over the mob's machine state: fires when membership,
    /// binding or lifecycle truth may have moved.
    fn state_changes(&self) -> Self::Changes;
    /// Last actor-published lifecycle phase, read without an actor round trip.
    fn phase(&self) -> meerkat_mob::MobState;
    /// Roster ids of the Active members. Only Active members have a live
    /// runtime delta stream; subscribing others fails every pass (the console
    /// forwarder learned this the hard way).
    fn active_members(&self) -> impl Future<Output = Vec<meerkat_mob::AgentIdentity>> + Send;
    /// Open the member's observe-only agent-event stream.
    fn subscribe(
        &self,
        identity: &meerkat_mob::AgentIdentity,
    ) -> impl Future<Output = Result<meerkat_core::comms::EventStream, meerkat_mob::MobError>> + Send;
}

/// A change signal. `Err` means the publisher is gone and the signal will
/// never fire again.
trait MemberStateChanges: Send + 'static {
    fn changed(&mut self) -> impl Future<Output = Result<(), ()>> + Send;
}

impl MemberStateChanges for meerkat_mob::MobMachineStateChanges {
    async fn changed(&mut self) -> Result<(), ()> {
        meerkat_mob::MobMachineStateChanges::changed(self)
            .await
            .map_err(|_closed| ())
    }
}

impl MemberEventSource for meerkat_mob::MobHandle {
    type Changes = meerkat_mob::MobMachineStateChanges;

    fn state_changes(&self) -> Self::Changes {
        self.machine_state_changes()
    }

    fn phase(&self) -> meerkat_mob::MobState {
        self.status_observation_snapshot()
    }

    async fn active_members(&self) -> Vec<meerkat_mob::AgentIdentity> {
        self.list_members_including_retiring()
            .await
            .into_iter()
            .filter(|entry| entry.status == meerkat_mob::MobMemberStatus::Active)
            .map(|entry| entry.agent_identity)
            .collect()
    }

    async fn subscribe(
        &self,
        identity: &meerkat_mob::AgentIdentity,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_mob::MobError> {
        self.subscribe_agent_events(identity).await
    }
}

/// A member stream that closes sooner than this after its subscribe counts
/// as a failed subscribe: a stream that ends as soon as it opens would
/// otherwise be re-subscribed on every close, a spin with no typed signal.
const OBSERVER_MIN_HEALTHY_STREAM: std::time::Duration = crate::fallback_wake::FALLBACK_WAKE_BASE;

/// Per-identity subscribe-failure guard for the member-event observer,
/// keyed by roster id (the handle's namespace).
///
/// Subscriptions are driven by machine-state changes, not a timer. This is
/// the shared last-resort guard ([`crate::fallback_wake`]) around that:
/// every subscribe attempt is an actor command, and during a cold boot the
/// machine state changes many times per second, so a member that cannot be
/// subscribed yet must not be retried on every change. Its deadline is also
/// the fallback wake for a member that failed and whose state then stops
/// changing (a subscribe failure has no typed "now subscribable" signal).
#[derive(Default)]
struct ObserverSubscribeBackoff {
    failures: std::collections::HashMap<String, crate::fallback_wake::FallbackWake>,
}

impl ObserverSubscribeBackoff {
    fn may_attempt(&self, identity: &str, now: tokio::time::Instant) -> bool {
        self.failures
            .get(identity)
            .is_none_or(|wake| wake.is_due(now))
    }

    /// Record a failure; returns whether it is the first in a row.
    fn record_failure(&mut self, identity: &str, now: tokio::time::Instant) -> bool {
        self.failures
            .entry(identity.to_string())
            .or_default()
            .record_failure(now)
    }

    fn record_success(&mut self, identity: &str) {
        self.failures.remove(identity);
    }

    /// Forget members that left the active roster so the map stays bounded.
    fn retain_active(&mut self, active: &std::collections::HashSet<String>) {
        self.failures
            .retain(|identity, _| active.contains(identity));
    }

    /// The earliest pending retry, if any member is backing off.
    fn next_attempt(&self) -> Option<tokio::time::Instant> {
        self.failures
            .values()
            .filter_map(crate::fallback_wake::FallbackWake::next_wake)
            .min()
    }
}

enum ObservedMemberEvent {
    Event(String, Box<meerkat_core::event::EventEnvelope<AgentEvent>>),
    Closed(String),
}

/// Subscription bookkeeping of the member-event observer.
struct MemberEventSubscriptions {
    streams: futures::stream::SelectAll<futures::stream::BoxStream<'static, ObservedMemberEvent>>,
    subscribed: std::collections::HashSet<String>,
    /// When each open stream was subscribed, to recognise one that closes
    /// as soon as it opens, with the member's failure history set aside
    /// while the stream is open (an open stream arms no wake).
    subscribed_at: std::collections::HashMap<
        String,
        (tokio::time::Instant, crate::fallback_wake::FallbackWake),
    >,
    backoff: ObserverSubscribeBackoff,
}

impl MemberEventSubscriptions {
    /// A member's stream ended. One that lived at least
    /// [`OBSERVER_MIN_HEALTHY_STREAM`] proved the subscription healthy and
    /// clears the member's failures; one that closed sooner counts as a
    /// failed subscribe, so the member is not re-subscribed on every close.
    fn stream_closed(&mut self, identity: &str) {
        self.subscribed.remove(identity);
        let now = tokio::time::Instant::now();
        match self.subscribed_at.remove(identity) {
            Some((at, history)) if now.duration_since(at) < OBSERVER_MIN_HEALTHY_STREAM => {
                self.backoff.failures.insert(identity.to_string(), history);
                if self.backoff.record_failure(identity, now) {
                    tracing::warn!(
                        identity = %identity,
                        "agent memory taint observer: member stream closed right after subscribe; \
                         backing off"
                    );
                }
            }
            _ => self.backoff.record_success(identity),
        }
    }

    /// Subscribe every Active member that has no stream and is not backing
    /// off. A no-op while the mob is not Running: subscribing goes through
    /// the mob actor, and while the mob is still Creating (cold boot,
    /// restore) or already stopping, members cannot be subscribed and every
    /// attempt only adds actor load.
    async fn reconcile<S: MemberEventSource>(&mut self, source: &S) {
        use futures::StreamExt;

        if source.phase() != meerkat_mob::MobState::Running {
            return;
        }
        let now = tokio::time::Instant::now();
        let mut active = std::collections::HashSet::new();
        for member in source.active_members().await {
            let identity = member.to_string();
            active.insert(identity.clone());
            if self.subscribed.contains(&identity) || !self.backoff.may_attempt(&identity, now) {
                continue;
            }
            // Sinks receive the LOGICAL identity (task #53): the roster id is
            // the comms-safe encoding of the member's alias, and
            // identity-first internal members now roster under the encoded
            // DURABLE identity (a generated rt:{identity}:{generation} alias
            // is incarnation detail, not a roster spelling) - both
            // decode/strip to the durable identity the memory scopes, the
            // write gate, and the SDK surface key on. Keying sinks by the
            // roster id is what split distiller scopes per incarnation
            // (downstream app activation smoke). Subscription bookkeeping
            // (`subscribed`/`Closed`/backoff) stays keyed by the roster id -
            // that is the handle's namespace.
            let sink_identity = crate::member_comms_id::logical_memory_identity(&identity);
            match source.subscribe(&member).await {
                Ok(stream) => {
                    self.subscribed.insert(identity.clone());
                    let history = self.backoff.failures.remove(&identity).unwrap_or_default();
                    self.subscribed_at.insert(identity.clone(), (now, history));
                    let close_key = identity.clone();
                    self.streams.push(
                        stream
                            .map(move |envelope| {
                                ObservedMemberEvent::Event(
                                    sink_identity.clone(),
                                    Box::new(envelope),
                                )
                            })
                            .chain(futures::stream::once(async move {
                                ObservedMemberEvent::Closed(close_key)
                            }))
                            .boxed(),
                    );
                }
                Err(error) => {
                    // Usually a short-lived spawn race. Warn on the first
                    // failure in a row, then debug.
                    if self.backoff.record_failure(&identity, now) {
                        tracing::warn!(
                            identity = %identity,
                            error = %error,
                            "agent memory taint observer: failed to subscribe; will retry on the next state change, with backoff"
                        );
                    } else {
                        tracing::debug!(
                            identity = %identity,
                            error = %error,
                            "agent memory taint observer: subscribe still failing; backing off"
                        );
                    }
                }
            }
        }
        self.backoff.retain_active(&active);
    }
}

async fn run_member_event_observer<S: MemberEventSource>(
    source: S,
    sinks: Vec<Arc<dyn MemberAgentEventSink>>,
) {
    use futures::StreamExt;

    // Bind the change signal before the first pass so a change landing
    // during it still wakes the next wait.
    let mut changes = source.state_changes();
    let mut changes_open = true;
    let mut subscriptions = MemberEventSubscriptions {
        streams: futures::stream::SelectAll::new(),
        subscribed: std::collections::HashSet::new(),
        subscribed_at: std::collections::HashMap::new(),
        backoff: ObserverSubscribeBackoff::default(),
    };
    subscriptions.reconcile(&source).await;

    loop {
        let next_retry = subscriptions.backoff.next_attempt();
        tokio::select! {
            Some(observed) = subscriptions.streams.next() => match observed {
                ObservedMemberEvent::Event(identity, envelope) => {
                    for sink in &sinks {
                        sink.observe(&identity, &envelope);
                    }
                }
                ObservedMemberEvent::Closed(identity) => {
                    subscriptions.stream_closed(&identity);
                    // A closure is itself a re-subscribe trigger: the member
                    // may still be live (stream lag or teardown race), and no
                    // machine transition is guaranteed to follow.
                    subscriptions.reconcile(&source).await;
                }
            },
            changed = changes.changed(), if changes_open => match changed {
                Ok(()) => subscriptions.reconcile(&source).await,
                // The mob actor is gone: nothing new can become live. Drain
                // what is still open, then end.
                Err(()) => changes_open = false,
            },
            () = crate::fallback_wake::sleep_until(next_retry), if next_retry.is_some() => {
                subscriptions.reconcile(&source).await;
            }
            else => break,
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone,
    clippy::unwrap_used
)]
mod tests {
    use super::*;
    use meerkat_core::types::{ContentBlock, ServerToolKind, SessionId};
    use serde_json::json;

    fn run_started(session: &SessionId) -> AgentEvent {
        AgentEvent::RunStarted {
            identity: Default::default(),
            session_id: session.clone(),
            input: meerkat_core::types::RunInput::Content {
                content: meerkat_core::ContentInput::Text("hi".to_string()),
            },
            request_reasoning: None,
        }
    }

    fn tool_result(name: &str) -> AgentEvent {
        AgentEvent::ToolResultReceived {
            id: "tool-1".to_string(),
            name: name.to_string(),
            content: vec![ContentBlock::Text {
                text: "ok".to_string(),
            }],
            is_error: false,
        }
    }

    fn peer_ingested(taint: Option<meerkat_core::comms::SenderContentTaint>) -> AgentEvent {
        AgentEvent::PeerContentIngested {
            kind: meerkat_core::types::CommsNoticeKind::Message,
            peer: None,
            request_id: None,
            sender_taint: taint,
            delivery_id: None,
        }
    }

    // Ask 5 (0.7.13): the receiver taints on a sender's SIGNED `Tainted`
    // declaration, from the typed event — but `None` ("no declaration") and
    // `Clean` must never taint, and `None` is never coalesced into `Clean`.
    #[test]
    fn declared_peer_taint_taints_receiver_but_none_or_clean_does_not() {
        use meerkat_core::comms::SenderContentTaint;
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        tracker.note_current_session("identity:b", "sess-b");

        tracker.observe_agent_event("identity:b", &peer_ingested(None));
        assert!(
            tracker.session_taint("sess-b").is_none(),
            "no declaration must not taint"
        );

        tracker.observe_agent_event(
            "identity:b",
            &peer_ingested(Some(SenderContentTaint::Clean)),
        );
        assert!(
            tracker.session_taint("sess-b").is_none(),
            "an affirmative Clean declaration must not taint"
        );

        tracker.observe_agent_event(
            "identity:b",
            &peer_ingested(Some(SenderContentTaint::Tainted)),
        );
        assert!(
            tracker.session_taint("sess-b").is_some(),
            "a declared-tainted peer delivery taints the receiving session"
        );
    }

    #[test]
    fn taint_transitions_emit_timeline_events_when_sink_wired() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let sink = std::sync::Arc::new(crate::memory::events::CollectingEventSink::new());
        tracker.set_event_sink(sink.clone());

        tracker.note_current_session("identity:a", "sess-1");
        tracker.observe_agent_event("identity:a", &tool_result("web_fetch"));
        tracker.mark_reset_boundary("sess-1");
        // Idempotent boundary: no duplicate event.
        tracker.mark_reset_boundary("sess-1");
        tracker.note_current_session("identity:a", "sess-2");

        let types = sink.types();
        assert_eq!(
            types,
            vec![
                "memory.taint.transition", // sess-1 tainted
                "memory.taint.transition", // reset boundary
                "memory.taint.transition", // rotated clean
            ]
        );
        let events = sink.events.lock().unwrap();
        let kinds: Vec<String> = events
            .iter()
            .map(|event| match event {
                crate::memory::events::MemoryTimelineEvent::TaintTransition { kind, .. } => {
                    kind.clone()
                }
                other => panic!("unexpected event {other:?}"),
            })
            .collect();
        assert_eq!(kinds, vec!["tainted", "reset_boundary", "rotated_clean"]);
    }

    #[test]
    fn content_trust_parse_rejects_unknown_fields_and_bad_types() {
        let err = ContentTrustConfig::from_json_value(&json!({"servers": []}))
            .expect_err("unknown field must fail loud");
        assert!(err.contains("unsupported content_trust fields"), "{err}");
        let err = ContentTrustConfig::from_json_value(&json!({"trusted_mcp_servers": "kg"}))
            .expect_err("non-array must fail loud");
        assert!(err.contains("must be an array"), "{err}");
        let err = ContentTrustConfig::from_json_value(&json!({"untrusted_tools": [1]}))
            .expect_err("non-string entry must fail loud");
        assert!(err.contains("non-empty strings"), "{err}");
        let err =
            ContentTrustConfig::from_json_value(&json!([])).expect_err("non-object must fail loud");
        assert!(err.contains("must be an object"), "{err}");
    }

    #[test]
    fn content_trust_parse_accepts_full_block() {
        let config = ContentTrustConfig::from_json_value(&json!({
            "trusted_mcp_servers": ["knowledge_graph"],
            "untrusted_tools": ["scrape_page"],
            "trusted_tools": ["mcp__scanner__lint"],
        }))
        .expect("valid block parses");
        assert_eq!(config.trusted_mcp_servers, vec!["knowledge_graph"]);
        assert_eq!(config.untrusted_tools, vec!["scrape_page"]);
        assert_eq!(config.trusted_tools, vec!["mcp__scanner__lint"]);
    }

    #[test]
    fn classification_precedence_holds() {
        let config = ContentTrustConfig {
            trusted_mcp_servers: vec!["kg".to_string()],
            untrusted_tools: vec!["scrape_page".to_string()],
            // Web builtins are never overridable.
            trusted_tools: vec!["web_search".to_string(), "mcp__evil__probe".to_string()],
        };
        assert!(matches!(
            config.classify_tool("web_search"),
            ToolContentTrust::Untrusted { .. }
        ));
        assert!(matches!(
            config.classify_tool("scrape_page"),
            ToolContentTrust::Untrusted { .. }
        ));
        // Explicit per-tool trust overrides server-level distrust.
        assert_eq!(
            config.classify_tool("mcp__evil__probe"),
            ToolContentTrust::Trusted
        );
        // MCP untrusted by default; allowlisted server trusted.
        assert!(matches!(
            config.classify_tool("mcp__other__search"),
            ToolContentTrust::Untrusted { .. }
        ));
        assert_eq!(
            config.classify_tool("mcp__kg__query"),
            ToolContentTrust::Trusted
        );
        // Unknown plain names are trusted in P1 (documented coarseness).
        assert_eq!(config.classify_tool("shell"), ToolContentTrust::Trusted);
    }

    // (b) The dispatch-time join: typed MCP provenance attributes a tool to
    // its server even when the NAME is not server-qualified - the exact gap
    // the name-based path could not close.
    #[test]
    fn typed_mcp_provenance_attributes_unqualified_tool_names() {
        use meerkat_core::types::{ToolProvenance, ToolSourceKind};
        let config = ContentTrustConfig {
            trusted_mcp_servers: vec!["kg".to_string()],
            ..ContentTrustConfig::default()
        };
        let kg = ToolProvenance {
            kind: ToolSourceKind::Mcp,
            source_id: "kg".into(),
            artifact: None,
        };
        let scraper = ToolProvenance {
            kind: ToolSourceKind::Mcp,
            source_id: "scraper".into(),
            artifact: None,
        };
        // Plain name, untrusted server: provenance closes the attribution.
        let verdict = config.classify_tool_with_provenance("scrape_page", Some(&scraper));
        match verdict {
            ToolContentTrust::Untrusted { source } => {
                assert!(source.contains("MCP server 'scraper'"), "{source}");
                assert!(source.contains("scrape_page"), "{source}");
            }
            ToolContentTrust::Trusted => panic!("untrusted-server MCP tool must taint"),
        }
        // Plain name, allowlisted server: trusted through the same join.
        assert_eq!(
            config.classify_tool_with_provenance("query", Some(&kg)),
            ToolContentTrust::Trusted
        );
        // Typed provenance wins over a misleading name shape: the server in
        // the provenance is the owner, not the name's `mcp__` segment.
        assert_eq!(
            config.classify_tool_with_provenance("mcp__evil__query", Some(&kg)),
            ToolContentTrust::Trusted
        );
        // Precedence is preserved around the widened MCP step: web builtins
        // and explicit per-tool lists still outrank provenance.
        assert!(matches!(
            config.classify_tool_with_provenance("web_search", Some(&kg)),
            ToolContentTrust::Untrusted { .. }
        ));
        let listed = ContentTrustConfig {
            trusted_tools: vec!["scrape_page".to_string()],
            ..ContentTrustConfig::default()
        };
        assert_eq!(
            listed.classify_tool_with_provenance("scrape_page", Some(&scraper)),
            ToolContentTrust::Trusted,
            "explicit trusted_tools overrides server-level distrust, as on the name path"
        );
    }

    // (c) Absence fallback: no provenance (and non-MCP provenance) must
    // reproduce the name-based classification exactly.
    #[test]
    fn absent_or_non_mcp_provenance_falls_back_to_name_classification() {
        use meerkat_core::types::{ToolProvenance, ToolSourceKind};
        let config = ContentTrustConfig {
            trusted_mcp_servers: vec!["kg".to_string()],
            untrusted_tools: vec!["scrape_page".to_string()],
            trusted_tools: vec!["mcp__evil__probe".to_string()],
        };
        for name in [
            "web_search",
            "scrape_page",
            "mcp__evil__probe",
            "mcp__other__search",
            "mcp__kg__query",
            "shell",
        ] {
            assert_eq!(
                config.classify_tool_with_provenance(name, None),
                config.classify_tool(name),
                "provenance-absent classification must match the name path for '{name}'"
            );
        }
        // Non-MCP provenance kinds keep the name-shape semantics too.
        let builtin = ToolProvenance {
            kind: ToolSourceKind::Builtin,
            source_id: "builtin".into(),
            artifact: None,
        };
        assert_eq!(
            config.classify_tool_with_provenance("shell", Some(&builtin)),
            ToolContentTrust::Trusted
        );
        assert_eq!(
            config.classify_tool_with_provenance("mcp__other__search", Some(&builtin)),
            config.classify_tool("mcp__other__search")
        );
    }

    #[test]
    fn dispatched_tool_result_marks_identity_before_any_session_attribution() {
        use meerkat_core::types::{ToolProvenance, ToolSourceKind};
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let provenance = ToolProvenance {
            kind: ToolSourceKind::Mcp,
            source_id: "scraper".into(),
            artifact: None,
        };
        // No RunStarted, no delivery hook: the dispatch feed must still land
        // (identity-sticky pending), so the same-turn write gate sees it.
        tracker.observe_dispatched_tool_result("identity:a", "scrape_page", Some(&provenance));
        let taint = tracker
            .identity_taint("identity:a")
            .expect("dispatch-time mark must be visible to the gate immediately");
        assert!(
            taint.source.contains("MCP server 'scraper'"),
            "{}",
            taint.source
        );

        // Trusted results do not mark.
        tracker.observe_dispatched_tool_result("identity:b", "shell", None);
        assert!(tracker.identity_taint("identity:b").is_none());

        // Server tools are always untrusted.
        tracker.observe_dispatched_server_tool("identity:c", &ServerToolKind::WebSearch);
        let taint = tracker
            .identity_taint("identity:c")
            .expect("server tool marks");
        assert!(taint.source.contains("web_search"), "{}", taint.source);
    }

    #[test]
    fn tracker_taints_on_untrusted_tool_and_clears_on_rotation() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let session = SessionId::new();
        tracker.observe_agent_event("identity:a", &run_started(&session));
        assert!(tracker.identity_taint("identity:a").is_none());

        tracker.observe_agent_event("identity:a", &tool_result("shell"));
        assert!(tracker.identity_taint("identity:a").is_none());

        tracker.observe_agent_event("identity:a", &tool_result("web_search"));
        let taint = tracker
            .identity_taint("identity:a")
            .expect("web tool result taints the session");
        assert!(taint.source.contains("web_search"), "{}", taint.source);
        assert!(tracker.session_taint(&session.to_string()).is_some());

        // Session-sticky: a later benign event does not clear.
        tracker.observe_agent_event("identity:a", &tool_result("shell"));
        assert!(tracker.identity_taint("identity:a").is_some());

        // Rotation (reset/respawn/fresh spawn mint a new session id) clears.
        let fresh = SessionId::new();
        tracker.observe_agent_event("identity:a", &run_started(&fresh));
        assert!(tracker.identity_taint("identity:a").is_none());
        // The old session's fact remains recorded (P2 comms joins read it).
        assert!(tracker.session_taint(&session.to_string()).is_some());
    }

    #[test]
    fn tracker_taints_on_server_tool_content() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let session = SessionId::new();
        tracker.note_current_session("identity:a", &session.to_string());
        tracker.observe_agent_event(
            "identity:a",
            &AgentEvent::ServerToolContent {
                assistant_message_id: None,
                id: None,
                kind: ServerToolKind::WebSearch,
                content: json!({"results": []}),
            },
        );
        let taint = tracker.identity_taint("identity:a").expect("taints");
        assert!(taint.source.contains("web_search"), "{}", taint.source);
    }

    #[test]
    fn pre_attribution_taint_holds_identity_sticky_then_transfers() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        // Tool event before any RunStarted (mid-run attach).
        tracker.observe_agent_event("identity:a", &tool_result("fetch"));
        assert!(tracker.identity_taint("identity:a").is_some());

        // The next attributed session inherits the pending taint.
        let session = SessionId::new();
        tracker.observe_agent_event("identity:a", &run_started(&session));
        assert!(tracker.session_taint(&session.to_string()).is_some());
        assert!(tracker.identity_taint("identity:a").is_some());
    }

    #[test]
    fn clear_identity_drops_attribution_but_keeps_the_session_fact() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let session = SessionId::new();
        tracker.observe_agent_event("identity:a", &run_started(&session));
        tracker.observe_agent_event("identity:a", &tool_result("web_fetch"));
        assert!(tracker.identity_taint("identity:a").is_some());
        tracker.clear_identity("identity:a");
        // The identity moves on clean...
        assert!(tracker.identity_taint("identity:a").is_none());
        // ...but the historical fact survives: the comms join and the
        // evidence-range gate consult exactly such sessions (P2).
        assert!(tracker.session_taint(&session.to_string()).is_some());
        assert!(
            tracker
                .evidence_quarantine_reason(&session.to_string())
                .is_some()
        );
    }

    #[test]
    fn outbound_declarer_stamps_tainted_on_ingestion_and_clears_on_boundaries() {
        use std::sync::{Arc, Mutex};
        type Recorded = Vec<(String, Option<meerkat_core::comms::SenderContentTaint>)>;
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let calls: Arc<Mutex<Recorded>> = Arc::new(Mutex::new(Vec::new()));
        let sink = calls.clone();
        tracker.set_outbound_taint_declarer(Arc::new(move |identity, taint| {
            sink.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((identity.to_string(), taint));
        }));

        let session = SessionId::new();
        tracker.observe_agent_event("identity:a", &run_started(&session));
        // A benign tool makes no declaration.
        tracker.observe_agent_event("identity:a", &tool_result("shell"));
        assert!(calls.lock().unwrap().is_empty());

        // Untrusted ingestion stamps the member Tainted for outbound peer sends.
        tracker.observe_agent_event("identity:a", &tool_result("web_search"));
        {
            let recorded = calls.lock().unwrap();
            assert_eq!(recorded.len(), 1, "one declaration expected: {recorded:?}");
            assert_eq!(recorded[0].0, "identity:a");
            assert_eq!(
                recorded[0].1,
                Some(meerkat_core::comms::SenderContentTaint::Tainted)
            );
        }

        // Rotation to a fresh clean session clears the declaration (None, not
        // Clean — an absent declaration, never coalesced into clean).
        let fresh = SessionId::new();
        tracker.note_current_session("identity:a", &fresh.to_string());
        {
            let recorded = calls.lock().unwrap();
            assert_eq!(recorded.len(), 2, "rotation clears: {recorded:?}");
            assert_eq!(recorded[1].1, None);
        }

        // Reset (the operator escape hatch) is also a fresh-context boundary.
        tracker.observe_agent_event("identity:a", &tool_result("web_fetch"));
        tracker.clear_identity("identity:a");
        {
            let recorded = calls.lock().unwrap();
            assert_eq!(
                recorded.last().expect("a clear declaration").1,
                None,
                "reset clears the outbound declaration: {recorded:?}"
            );
        }
    }

    #[test]
    fn gate_quarantines_tainted_agents_and_quarantined_policy() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let session = SessionId::new();
        tracker.observe_agent_event("identity:a", &run_started(&session));

        let gate = TaintLlmWriteGate::new(Some(tracker.clone()), AgentMemoryLlmWrites::Observed);
        let agent = MemoryAuthor::Agent {
            identity: "identity:a".to_string(),
        };
        assert!(
            gate.quarantine_reason(&agent, StagedBatchKind::FreshWrite, &[])
                .is_none()
        );
        assert!(
            gate.quarantine_reason(&MemoryAuthor::Application, StagedBatchKind::FreshWrite, &[])
                .is_none()
        );

        tracker.observe_agent_event("identity:a", &tool_result("web_search"));
        let reason = gate
            .quarantine_reason(&agent, StagedBatchKind::FreshWrite, &[])
            .expect("tainted quarantines");
        assert!(reason.contains("session tainted"), "{reason}");
        // Non-LLM principals are never gated, tainted or not.
        assert!(
            gate.quarantine_reason(&MemoryAuthor::Application, StagedBatchKind::FreshWrite, &[])
                .is_none()
        );
        assert!(
            gate.quarantine_reason(&MemoryAuthor::Operator, StagedBatchKind::FreshWrite, &[])
                .is_none()
        );

        // llm_writes=quarantined forces quarantine with no taint at all —
        // for every first-pass LLM write (Agent, Distiller, and the
        // steward's own fresh consolidate/harvest/rank output).
        let strict = TaintLlmWriteGate::new(None, AgentMemoryLlmWrites::Quarantined);
        let reason = strict
            .quarantine_reason(
                &MemoryAuthor::Agent {
                    identity: "identity:clean".to_string(),
                },
                StagedBatchKind::FreshWrite,
                &[],
            )
            .expect("policy quarantines untainted writes");
        assert!(reason.contains("llm_writes=quarantined"), "{reason}");
        assert!(
            strict
                .quarantine_reason(
                    &MemoryAuthor::Distiller {
                        run_id: "run-1".to_string()
                    },
                    StagedBatchKind::FreshWrite,
                    &[]
                )
                .is_some()
        );
        let steward = MemoryAuthor::Steward {
            run_id: "run-1".to_string(),
        };
        assert!(
            strict
                .quarantine_reason(&steward, StagedBatchKind::FreshWrite, &[])
                .is_some(),
            "fresh steward LLM output (consolidate/harvest/rank) respects the posture"
        );
        // A review verdict IS the review the posture defers to: the posture
        // branch must not re-quarantine releases, approved promotions, and
        // proposal accepts (they would otherwise never produce an Active
        // record). Keyed on the batch kind, not the Steward author.
        assert!(
            strict
                .quarantine_reason(&steward, StagedBatchKind::ReviewVerdict, &[])
                .is_none()
        );
        assert!(
            strict
                .quarantine_reason(&MemoryAuthor::Operator, StagedBatchKind::FreshWrite, &[])
                .is_none()
        );
    }

    #[test]
    fn quarantined_posture_still_gates_review_verdicts_on_tainted_evidence() {
        // The review-verdict exemption is posture-branch-only: a review
        // batch citing a tainted session still quarantines through the
        // evidence-range branch (§10.1).
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let session = SessionId::new();
        tracker.observe_agent_event("identity:a", &run_started(&session));
        tracker.observe_agent_event("identity:a", &tool_result("web_search"));

        let gate = TaintLlmWriteGate::new(Some(tracker), AgentMemoryLlmWrites::Quarantined);
        let steward = MemoryAuthor::Steward {
            run_id: "run-1".to_string(),
        };
        assert!(
            gate.quarantine_reason(&steward, StagedBatchKind::ReviewVerdict, &[])
                .is_none()
        );
        let reason = gate
            .quarantine_reason(
                &steward,
                StagedBatchKind::ReviewVerdict,
                &evidence_for(&session),
            )
            .expect("tainted evidence still quarantines review verdicts");
        assert!(reason.contains("evidence session tainted"), "{reason}");
    }

    fn evidence_for(session: &SessionId) -> Vec<EvidenceRef> {
        vec![EvidenceRef {
            session_id: session.to_string(),
            generation: 0,
            revision: None,
            range: Some((0, 4)),
        }]
    }

    #[test]
    fn gate_quarantines_llm_writes_citing_tainted_evidence() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let session = SessionId::new();
        tracker.observe_agent_event("identity:a", &run_started(&session));
        tracker.observe_agent_event("identity:a", &tool_result("web_search"));
        // Identity rotates away: the identity is clean, the session fact
        // remains — the distiller's evidence must still quarantine.
        let fresh = SessionId::new();
        tracker.observe_agent_event("identity:a", &run_started(&fresh));

        let gate = TaintLlmWriteGate::new(Some(tracker), AgentMemoryLlmWrites::Observed);
        let distiller = MemoryAuthor::Distiller {
            run_id: "run-1".to_string(),
        };
        let reason = gate
            .quarantine_reason(
                &distiller,
                StagedBatchKind::FreshWrite,
                &evidence_for(&session),
            )
            .expect("tainted evidence range quarantines (session-tainted ⇒ range-tainted)");
        assert!(reason.contains("evidence session tainted"), "{reason}");
        // Clean evidence does not.
        assert!(
            gate.quarantine_reason(
                &distiller,
                StagedBatchKind::FreshWrite,
                &evidence_for(&fresh)
            )
            .is_none()
        );
        // Non-LLM authors are never evidence-gated.
        assert!(
            gate.quarantine_reason(
                &MemoryAuthor::Operator,
                StagedBatchKind::FreshWrite,
                &evidence_for(&session)
            )
            .is_none()
        );
    }

    #[test]
    fn reset_boundary_quarantines_evidence_without_content_taint() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let session = SessionId::new();
        tracker.observe_agent_event("identity:a", &run_started(&session));
        assert!(
            tracker
                .evidence_quarantine_reason(&session.to_string())
                .is_none()
        );
        tracker.mark_reset_boundary(&session.to_string());
        let gate = TaintLlmWriteGate::new(Some(tracker), AgentMemoryLlmWrites::Observed);
        let reason = gate
            .quarantine_reason(
                &MemoryAuthor::Distiller {
                    run_id: "run-1".to_string(),
                },
                StagedBatchKind::FreshWrite,
                &evidence_for(&session),
            )
            .expect("reset boundary quarantines distillates");
        assert!(reason.contains("reset boundary"), "{reason}");
    }

    #[test]
    fn peer_projection_sender_parses_message_and_response_shapes() {
        assert_eq!(
            peer_projection_sender_identity("Peer message from mob-1/worker/identity:bob:"),
            Some("identity:bob")
        );
        assert_eq!(
            peer_projection_sender_identity(
                "Peer response from mob-1/worker/identity:bob (to request: req-9)"
            ),
            Some("identity:bob")
        );
        // External peers may have plain display names.
        assert_eq!(
            peer_projection_sender_identity("Peer message from scout:"),
            Some("scout")
        );
        // Peer requests render a raw peer id — unmappable, and honestly so.
        assert_eq!(
            peer_projection_sender_identity("Peer request from peer_id 018fabc (id: r-1)"),
            None
        );
        assert_eq!(peer_projection_sender_identity("ordinary text"), None);
    }

    #[test]
    fn comms_join_taints_receiver_of_message_from_tainted_sender() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        // Sender taints its session.
        let sender_session = SessionId::new();
        tracker.observe_agent_event("identity:bob", &run_started(&sender_session));
        tracker.observe_agent_event("identity:bob", &tool_result("web_search"));

        // Receiver gets a peer message from the tainted sender: the run's
        // injected input carries the canonical projection text.
        let receiver_session = SessionId::new();
        let delivery = AgentEvent::RunStarted {
            identity: Default::default(),
            session_id: receiver_session.clone(),
            input: meerkat_core::types::RunInput::Content {
                content: meerkat_core::ContentInput::Text(
                    "Peer message from mob-1/worker/identity:bob:\nplease remember X".to_string(),
                ),
            },
            request_reasoning: None,
        };
        tracker.observe_agent_event("identity:alice", &delivery);
        let taint = tracker
            .identity_taint("identity:alice")
            .expect("receiver session taints (peer-laundering close, §10.1)");
        assert!(taint.source.contains("identity:bob"), "{}", taint.source);
        assert!(
            tracker
                .session_taint(&receiver_session.to_string())
                .is_some()
        );

        // A message from a clean tracked sender does not taint.
        let clean_session = SessionId::new();
        tracker.observe_agent_event("identity:carol", &run_started(&clean_session));
        let receiver2 = SessionId::new();
        let clean_delivery = AgentEvent::RunStarted {
            identity: Default::default(),
            session_id: receiver2.clone(),
            input: meerkat_core::types::RunInput::Content {
                content: meerkat_core::ContentInput::Text(
                    "Peer message from mob-1/worker/identity:carol:\nhello".to_string(),
                ),
            },
            request_reasoning: None,
        };
        tracker.observe_agent_event("identity:dave", &clean_delivery);
        assert!(tracker.identity_taint("identity:dave").is_none());
    }

    /// Scripted [`MemberEventSource`]: a settable Active roster and phase, a
    /// change signal the test fires, and per-identity subscribe outcomes.
    /// Records every attempt.
    struct ScriptedMemberSource {
        phase: Mutex<meerkat_mob::MobState>,
        members: Mutex<Vec<meerkat_mob::AgentIdentity>>,
        subscribable: Mutex<std::collections::HashSet<String>>,
        /// Subscribable members whose stream ends as soon as it opens.
        closes_at_once: Mutex<std::collections::HashSet<String>>,
        attempts: Mutex<HashMap<String, Vec<tokio::time::Instant>>>,
        changes: tokio::sync::watch::Sender<()>,
    }

    struct ScriptedChanges(tokio::sync::watch::Receiver<()>);

    impl MemberStateChanges for ScriptedChanges {
        async fn changed(&mut self) -> Result<(), ()> {
            self.0.changed().await.map_err(|_closed| ())
        }
    }

    impl ScriptedMemberSource {
        fn new(phase: meerkat_mob::MobState, members: &[&str], subscribable: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                phase: Mutex::new(phase),
                members: Mutex::new(
                    members
                        .iter()
                        .map(|member| meerkat_mob::AgentIdentity::from(*member))
                        .collect(),
                ),
                subscribable: Mutex::new(subscribable.iter().map(ToString::to_string).collect()),
                closes_at_once: Mutex::new(std::collections::HashSet::new()),
                attempts: Mutex::new(HashMap::new()),
                changes: tokio::sync::watch::channel(()).0,
            })
        }

        /// Publish a machine-state change, as the mob actor does on every
        /// applied machine input.
        fn change(&self) {
            self.changes.send_replace(());
        }

        fn attempts(&self, identity: &str) -> Vec<tokio::time::Instant> {
            self.attempts
                .lock()
                .unwrap()
                .get(identity)
                .cloned()
                .unwrap_or_default()
        }
    }

    impl MemberEventSource for Arc<ScriptedMemberSource> {
        type Changes = ScriptedChanges;

        fn state_changes(&self) -> ScriptedChanges {
            ScriptedChanges(self.changes.subscribe())
        }

        fn phase(&self) -> meerkat_mob::MobState {
            *self.phase.lock().unwrap()
        }

        async fn active_members(&self) -> Vec<meerkat_mob::AgentIdentity> {
            self.members.lock().unwrap().clone()
        }

        async fn subscribe(
            &self,
            identity: &meerkat_mob::AgentIdentity,
        ) -> Result<meerkat_core::comms::EventStream, meerkat_mob::MobError> {
            let identity = identity.to_string();
            self.attempts
                .lock()
                .unwrap()
                .entry(identity.clone())
                .or_default()
                .push(tokio::time::Instant::now());
            if self.closes_at_once.lock().unwrap().contains(&identity) {
                Ok(Box::pin(futures::stream::empty()))
            } else if self.subscribable.lock().unwrap().contains(&identity) {
                Ok(Box::pin(futures::stream::pending()))
            } else {
                Err(meerkat_mob::MobError::Internal(format!(
                    "no event injector for '{identity}' yet"
                )))
            }
        }
    }

    /// Let the observer task run everything that is ready, without moving
    /// the (paused) clock.
    async fn settle() {
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }

    /// A member is subscribed on the machine-state change that makes it
    /// live, with no timer involved: the observer sleeps until the state
    /// changes.
    #[tokio::test(start_paused = true)]
    async fn member_event_observer_subscribes_on_the_state_change_that_makes_a_member_live() {
        let source = ScriptedMemberSource::new(meerkat_mob::MobState::Running, &[], &["worker"]);
        let started = tokio::time::Instant::now();
        let task = tokio::spawn(run_member_event_observer(source.clone(), Vec::new()));
        settle().await;

        // An hour without any state change: no pass, no subscribe.
        tokio::time::sleep(std::time::Duration::from_hours(1)).await;
        assert!(source.attempts("worker").is_empty());

        source
            .members
            .lock()
            .unwrap()
            .push(meerkat_mob::AgentIdentity::from("worker"));
        source.change();
        settle().await;
        let attempts = source.attempts("worker");
        assert_eq!(attempts.len(), 1, "subscribed on the change itself");
        assert_eq!(
            attempts[0].duration_since(started),
            std::time::Duration::from_hours(1),
            "no clock time passed between the change and the subscribe"
        );

        // Later changes do not re-subscribe an open stream.
        source.change();
        settle().await;
        assert_eq!(source.attempts("worker").len(), 1);
        task.abort();
    }

    /// The observer never subscribes while the mob is not Running: during
    /// cold boot each attempt is an actor command competing with the restore,
    /// however often the machine state changes.
    #[tokio::test(start_paused = true)]
    async fn member_event_observer_waits_for_a_running_mob_before_subscribing() {
        let source =
            ScriptedMemberSource::new(meerkat_mob::MobState::Creating, &["worker"], &["worker"]);
        let task = tokio::spawn(run_member_event_observer(source.clone(), Vec::new()));
        settle().await;
        for _ in 0..50 {
            source.change();
            settle().await;
        }
        assert!(
            source.attempts("worker").is_empty(),
            "no subscribe attempt while the mob is Creating"
        );

        *source.phase.lock().unwrap() = meerkat_mob::MobState::Running;
        source.change();
        settle().await;
        assert_eq!(
            source.attempts("worker").len(),
            1,
            "the change into Running subscribes, and the open stream is not re-subscribed"
        );
        task.abort();
    }

    /// A member whose subscribe keeps failing is not retried on every state
    /// change: a storm of changes (cold boot) costs one attempt per backoff
    /// window, and a healthy member beside it is unaffected.
    #[tokio::test(start_paused = true)]
    async fn member_event_observer_backs_off_a_failing_member_through_a_change_storm() {
        let source = ScriptedMemberSource::new(
            meerkat_mob::MobState::Running,
            &["stuck", "healthy"],
            &["healthy"],
        );
        let task = tokio::spawn(run_member_event_observer(source.clone(), Vec::new()));
        settle().await;
        // 18 changes per second for 10 s.
        for _ in 0..180 {
            source.change();
            settle().await;
            tokio::time::sleep(std::time::Duration::from_millis(55)).await;
        }
        let stuck = source.attempts("stuck").len();
        assert!(
            (4..=6).contains(&stuck),
            "backoff bounds a failing member to ~1 attempt per window, saw {stuck}"
        );
        assert_eq!(source.attempts("healthy").len(), 1);
        task.abort();
    }

    /// With no further state change, a failed member is still retried at its
    /// backoff deadlines (the fallback guard), and once it becomes
    /// subscribable the next due attempt succeeds and retries stop.
    #[tokio::test(start_paused = true)]
    async fn member_event_observer_retries_a_failed_member_at_its_backoff_deadline() {
        let source = ScriptedMemberSource::new(meerkat_mob::MobState::Running, &["stuck"], &[]);
        let started = tokio::time::Instant::now();
        let task = tokio::spawn(run_member_event_observer(source.clone(), Vec::new()));

        tokio::time::sleep(std::time::Duration::from_mins(2)).await;
        let stuck = source.attempts("stuck");
        let offsets: Vec<u64> = stuck
            .iter()
            .map(|at| at.duration_since(started).as_secs())
            .collect();
        // 1 s, 2 s, 4 s, 8 s, 16 s, then the 30 s cap.
        assert_eq!(offsets, [0, 1, 3, 7, 15, 31, 61, 91]);

        source
            .subscribable
            .lock()
            .unwrap()
            .insert("stuck".to_string());
        tokio::time::sleep(std::time::Duration::from_mins(2)).await;
        assert_eq!(source.attempts("stuck").len(), stuck.len() + 1);
        task.abort();
    }

    /// A member whose stream closes as soon as it opens is not re-subscribed
    /// on every close (a spin with no typed signal): each instant close
    /// counts as a failed subscribe and backs the member off.
    #[tokio::test(start_paused = true)]
    async fn member_event_observer_backs_off_a_stream_that_closes_at_once() {
        let source =
            ScriptedMemberSource::new(meerkat_mob::MobState::Running, &["flaky"], &["flaky"]);
        source
            .closes_at_once
            .lock()
            .unwrap()
            .insert("flaky".to_string());
        let started = tokio::time::Instant::now();
        let task = tokio::spawn(run_member_event_observer(source.clone(), Vec::new()));
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let offsets: Vec<u64> = source
            .attempts("flaky")
            .iter()
            .map(|at| at.duration_since(started).as_secs())
            .collect();
        assert_eq!(offsets, [0, 1, 3, 7], "one attempt per backoff window");
        task.abort();
    }

    /// When the mob actor is gone and no stream is open, the observer ends.
    #[tokio::test(start_paused = true)]
    async fn member_event_observer_ends_when_the_mob_actor_is_gone() {
        struct GoneSource;
        impl MemberEventSource for GoneSource {
            type Changes = ScriptedChanges;
            fn state_changes(&self) -> ScriptedChanges {
                ScriptedChanges(tokio::sync::watch::channel(()).1)
            }
            fn phase(&self) -> meerkat_mob::MobState {
                meerkat_mob::MobState::Stopped
            }
            async fn active_members(&self) -> Vec<meerkat_mob::AgentIdentity> {
                Vec::new()
            }
            async fn subscribe(
                &self,
                _identity: &meerkat_mob::AgentIdentity,
            ) -> Result<meerkat_core::comms::EventStream, meerkat_mob::MobError> {
                Err(meerkat_mob::MobError::Internal("gone".to_string()))
            }
        }
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            run_member_event_observer(GoneSource, Vec::new()),
        )
        .await
        .expect("the observer ends once its change signal is closed");
    }

    #[test]
    fn observer_subscribe_backoff_resets_on_success_and_forgets_departed_members() {
        let now = tokio::time::Instant::now();
        let mut backoff = ObserverSubscribeBackoff::default();
        assert!(backoff.record_failure("a", now), "first failure in a row");
        assert!(!backoff.record_failure("a", now), "second failure in a row");
        assert!(!backoff.may_attempt("a", now));
        assert!(backoff.may_attempt("b", now), "backoff is per identity");
        backoff.record_success("a");
        assert!(backoff.may_attempt("a", now));
        assert!(backoff.record_failure("a", now), "success resets the run");
        backoff.retain_active(&std::collections::HashSet::new());
        assert!(
            backoff.may_attempt("a", now),
            "departed members are forgotten"
        );
    }
}
