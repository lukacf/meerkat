//! The approval engine: decides each tool call (see the [module docs](super)).
//!
//! [`ApprovalEngine::decide`] classifies the call into a tier, applies the
//! layered rules, reviews Tier-3 calls with the classifier (allow cache,
//! denial counters, pause), and escalates to the human through the
//! [`EscalationBroker`] when auto mode is paused or the classifier fails.
//! Escalations are recorded in the [`EscalationLedger`] and time out as
//! denials. The engine is shared by every session's
//! [`super::admission::ApprovalGate`].
//!
//! The classifier sees a reasoning-blind transcript per session: the user's
//! own words (recorded by [`crate::session::SessionHandle`] when it submits
//! them, or seeded from the authoritative session on resume) and prior tool
//! calls with their verdicts, never assistant prose or tool output.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use indexmap::IndexSet;
use meerkat_core::{ApprovalDecision, ApprovalId, Session, SessionId};
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tokio::sync::mpsc;

use super::classifier::{ActionClassifier, ClassifierInput, ClassifierVerdict, Environment};
use super::counters::{AutoMode, AutoModeState, DenialCounters, PauseCause};
use super::escalation::{EscalationBroker, EscalationPrompt, HumanDecision};
use super::ledger::{self, EscalationEntry, EscalationLedger};
use super::policy::{
    Action, ActionKind, Decider, PolicyContext, Tier, TierDecision, classify_tier,
};
use super::rules::{self, AutoSettings, Rule, RuleId, RuleOrigin, RuleSet, RulesError};
use super::transcript::ApprovalTranscript;
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
    /// Durable record of escalations; `None` keeps none.
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
}

type CacheKey = (String, u64);

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

/// The approval engine, shared behind an [`Arc`] by every session's gate.
pub struct ApprovalEngine {
    root: ProjectRoot,
    rules: RwLock<RuleSet>,
    settings: RwLock<AutoSettings>,
    transcripts: RwLock<HashMap<SessionId, ApprovalTranscript>>,
    auto: Mutex<AutoMode>,
    allow_cache: Mutex<HashMap<SessionId, HashSet<CacheKey>>>,
    stopped: Mutex<HashSet<SessionId>>,
    classifier: Arc<dyn ActionClassifier>,
    broker: Option<EscalationBroker>,
    ledger: Option<EscalationLedger>,
    escalation_timeout: Duration,
    next_escalation: AtomicU64,
    environment: Environment,
    posix_shell: bool,
    events: mpsc::UnboundedSender<ApprovalEvent>,
    rules_path: Option<PathBuf>,
    /// Project files (display paths) whose writes were allowed this run, in
    /// first-write order: shown to the classifier as agent-authored.
    agent_written: Mutex<IndexSet<String>>,
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
            transcripts: RwLock::new(HashMap::new()),
            auto: Mutex::new(auto),
            allow_cache: Mutex::new(HashMap::new()),
            stopped: Mutex::new(HashSet::new()),
            classifier,
            broker,
            ledger,
            escalation_timeout,
            next_escalation: AtomicU64::new(1),
            environment,
            posix_shell,
            events,
            rules_path,
            agent_written: Mutex::new(IndexSet::new()),
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

    /// Rendered approval transcript for a session (diagnostics and tests).
    #[must_use]
    pub fn transcript(&self, session_id: &SessionId) -> Option<String> {
        self.transcripts
            .read()
            .get(session_id)
            .map(ApprovalTranscript::render)
    }

    /// Seed a session's transcript from its authoritative history (on resume):
    /// user prompts and tool calls, never assistant prose or tool output.
    pub fn seed_transcript(&self, session: &Session) {
        let mut transcript = ApprovalTranscript::default();
        transcript.seed_from_session(session);
        self.transcripts
            .write()
            .insert(session.id().clone(), transcript);
    }

    /// Record words the human submitted to a session (a prompt or a steer).
    ///
    /// Does not clear a previous Stop: only a new run does that
    /// ([`Self::on_run_started`]).
    pub fn record_prompt(&self, session_id: &SessionId, prompt: &str) {
        self.transcripts
            .write()
            .entry(session_id.clone())
            .or_default()
            .push_user(prompt);
    }

    /// Remove the most recent user entry equal to `prompt` (the message never
    /// reached the model). Returns whether one was found.
    pub fn forget_prompt(&self, session_id: &SessionId, prompt: &str) -> bool {
        self.transcripts
            .write()
            .get_mut(session_id)
            .is_some_and(|t| t.forget_user(prompt))
    }

    /// A run of `session_id` started: a Stop of the previous turn no longer
    /// applies.
    pub fn on_run_started(&self, session_id: &SessionId) {
        self.stopped.lock().remove(session_id);
    }

    /// The human stopped `session_id`'s turn (an interrupt or a Stop answer):
    /// every call of that session that needs review is denied, without
    /// asking anyone, until its next run starts ([`Self::on_run_started`]).
    pub fn stop(&self, session_id: &SessionId) {
        self.stopped.lock().insert(session_id.clone());
    }

    /// Forget a session's per-session state (transcript, allow cache, stop).
    pub fn forget_session(&self, session_id: &SessionId) {
        self.transcripts.write().remove(session_id);
        self.allow_cache.lock().remove(session_id);
        self.stopped.lock().remove(session_id);
    }

    /// Add a rule, persisting it to the realm rules file when configured.
    ///
    /// # Errors
    ///
    /// Returns [`RulesError`] when persistence fails (the rule stays active).
    pub fn add_rule(&self, mut rule: Rule) -> Result<(), RulesError> {
        if self.rules_path.is_some() {
            rule.origin = RuleOrigin::Realm;
        }
        let snapshot = {
            let mut rules = self.rules.write();
            rules.push(rule);
            rules.clone()
        };
        self.persist(&snapshot)
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
            Some(path) => rules::persist_realm_rules(path, rules),
            None => Ok(()),
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

    /// Decide one tool call of `session_id`'s own agent. Records it in the
    /// session's approval transcript and emits an [`ApprovalEvent::Decision`].
    pub async fn decide(
        &self,
        session_id: &SessionId,
        tool_use_id: &str,
        name: &str,
        args: &Value,
    ) -> Verdict {
        self.decide_as(Caller::session(session_id), tool_use_id, name, args)
            .await
    }

    /// Decide one tool call made on behalf of `caller.session_id`: by its own
    /// agent, or by a helper it delegated to ([`Caller::agent`]). A helper's
    /// call is judged exactly like the session's own: against the session's
    /// transcript (the user's words; the helper's task text is
    /// assistant-authored and never recorded as user intent), its allow
    /// cache, its Stop and the shared denial counters. The escalation dialog
    /// and the decision event name the helper.
    pub async fn decide_as(
        &self,
        caller: Caller<'_>,
        tool_use_id: &str,
        name: &str,
        args: &Value,
    ) -> Verdict {
        let Caller { session_id, agent } = caller;
        let verdict = self
            .evaluate(session_id, agent, tool_use_id, name, args)
            .await;
        let (tier, reason) = match &verdict {
            Verdict::Allow { tier, .. } => (*tier, None),
            Verdict::Deny { tier, reason } => (*tier, Some(reason.to_string())),
        };
        if verdict.is_allowed() {
            self.record_agent_writes(&Action::parse(name, args));
        }
        let summary = verdict.summary();
        self.transcripts
            .write()
            .entry(session_id.clone())
            .or_default()
            .push_tool_call(tool_use_id, name, args.clone(), Some(summary));
        tracing::debug!(tool = name, %tier, ?summary, "auto-mode decision");
        self.emit(ApprovalEvent::Decision {
            session_id: session_id.clone(),
            agent: agent.map(str::to_string),
            tool_use_id: tool_use_id.to_string(),
            tool: name.to_string(),
            tier,
            verdict: summary,
            reason,
        });
        verdict
    }

    /// Remember the files an allowed write targets. Recorded at approval
    /// time (before the tool runs): a write that then fails stays listed,
    /// which only makes the classifier more careful.
    fn record_agent_writes(&self, action: &Action<'_>) {
        if matches!(action.kind, ActionKind::FileWrite { .. }) {
            self.agent_written
                .lock()
                .extend(action.subjects(&self.root));
        }
    }

    /// Project files the agent was allowed to write this run, oldest first.
    #[must_use]
    pub fn agent_written(&self) -> Vec<String> {
        self.agent_written.lock().iter().cloned().collect()
    }

    fn tier_of(&self, action: &Action<'_>) -> TierDecision {
        let trust_project_builds = self.settings.read().trust_project_builds;
        let ctx = PolicyContext {
            root: &self.root,
            trust_project_builds,
            posix_shell: self.posix_shell,
        };
        classify_tier(action, &self.rules.read(), &ctx)
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
        name: &str,
        args: &Value,
    ) -> Verdict {
        let action = Action::parse(name, args);
        if let Some(verdict) = self.static_verdict(self.tier_of(&action)) {
            return verdict;
        }

        if self.stopped.lock().contains(session_id) {
            return deny3(DenyReason::Stopped);
        }
        let (disabled, escalating) = {
            let auto = self.auto.lock();
            (auto.disabled(), auto.escalating())
        };
        if disabled {
            return deny3(DenyReason::AutoDisabled);
        }

        let key = cache_key(name, args);
        if !escalating
            && self
                .allow_cache
                .lock()
                .get(session_id)
                .is_some_and(|set| set.contains(&key))
        {
            self.auto.lock().record_allow();
            return allow3(Decider::Classifier, "previously allowed by the classifier");
        }

        let result = self.classify(session_id, &action).await;

        if escalating {
            let hint = match &result {
                Ok(v) => format!("classifier: {v}"),
                Err(e) => format!("classifier error: {e}"),
            };
            return self
                .escalate(session_id, agent, tool_use_id, &action, Some(hint))
                .await;
        }

        match result {
            Ok(ClassifierVerdict::Allow) => {
                self.allow_cache
                    .lock()
                    .entry(session_id.clone())
                    .or_default()
                    .insert(key);
                self.auto.lock().record_allow();
                allow3(Decider::Classifier, "allowed by the classifier")
            }
            Ok(ClassifierVerdict::Block { rule, reason }) => {
                let paused = self.auto.lock().record_classifier_deny();
                if let Some(cause) = paused {
                    self.emit(ApprovalEvent::Paused(cause));
                }
                deny3(DenyReason::Classifier {
                    rule,
                    explanation: reason,
                })
            }
            Err(err) => {
                tracing::warn!(error = %err, tool = name, "classifier failed; escalating");
                if self.broker.is_some() {
                    self.escalate(
                        session_id,
                        agent,
                        tool_use_id,
                        &action,
                        Some(format!("classifier error: {err}")),
                    )
                    .await
                } else {
                    deny3(DenyReason::ClassifierUnavailable(err.to_string()))
                }
            }
        }
    }

    /// Run the classifier on `action` with the session's transcript.
    async fn classify(
        &self,
        session_id: &SessionId,
        action: &Action<'_>,
    ) -> Result<ClassifierVerdict, super::classifier::ClassifierError> {
        let script = self.script_payload(action).await;
        let agent_written = self.agent_written();
        let transcript = self
            .transcripts
            .read()
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        let input = ClassifierInput {
            environment: &self.environment,
            transcript: &transcript,
            tool: action.name,
            args: action.args,
            script_payload: script.as_deref(),
            agent_written: &agent_written,
        };
        self.emit(ApprovalEvent::Classifying(true));
        let result = self.classifier.classify(input).await;
        self.emit(ApprovalEvent::Classifying(false));
        result
    }

    async fn escalate(
        &self,
        session_id: &SessionId,
        agent: Option<&str>,
        tool_use_id: &str,
        action: &Action<'_>,
        hint: Option<String>,
    ) -> Verdict {
        let Some(broker) = &self.broker else {
            return deny3(DenyReason::ClassifierUnavailable(
                "no human is available to review this action".to_string(),
            ));
        };
        let guard = broker.serial().await;
        // Re-check after waiting: a Stop or a new rule may have arrived meanwhile.
        if self.stopped.lock().contains(session_id) {
            return deny3(DenyReason::Stopped);
        }
        if let Some(verdict) = self.static_verdict(self.tier_of(action)) {
            return verdict;
        }
        if self.auto.lock().disabled() {
            return deny3(DenyReason::AutoDisabled);
        }
        let id = self.next_escalation.fetch_add(1, Ordering::Relaxed);
        let record = self.ledger.as_ref().and_then(|ledger| {
            ledger
                .request(EscalationEntry {
                    session_id,
                    tool_use_id,
                    action,
                    timeout: self.escalation_timeout,
                    hint: hint.as_deref(),
                })
                .map_err(|err| tracing::warn!(error = %err, "escalation not recorded"))
                .ok()
        });
        let mut pending = PendingEscalation {
            engine: self,
            id,
            record,
            settled: false,
        };
        let prompt = EscalationPrompt {
            id,
            session_id: session_id.clone(),
            agent: agent.map(str::to_string),
            tool: action.name.to_string(),
            args_preview: args_preview(action),
            subjects: action.subjects(&self.root),
            classifier_hint: hint,
        };
        let answer =
            tokio::time::timeout(self.escalation_timeout, broker.ask_locked(&guard, prompt)).await;
        drop(guard);
        let Ok(decision) = answer else {
            pending.settle_without_human("nobody answered in time");
            return deny3(DenyReason::EscalationTimedOut);
        };
        match decision {
            HumanDecision::AllowOnce => {
                pending.settle(ApprovalDecision::Approve, "allowed once");
                self.human_allowed();
                allow3(Decider::Human, "allowed once by the user")
            }
            HumanDecision::AllowAndAddRule(rule) => {
                pending.settle(ApprovalDecision::Approve, "allowed; allow rule added");
                if let Err(err) = self.add_rule(*rule) {
                    tracing::warn!(error = %err, "failed to persist rule");
                }
                self.human_allowed();
                allow3(Decider::Human, "allowed by the user (rule added)")
            }
            HumanDecision::Deny => {
                pending.settle(ApprovalDecision::Deny, "denied");
                deny3(DenyReason::Human)
            }
            HumanDecision::Stop => {
                pending.settle(ApprovalDecision::Deny, "denied; turn stopped");
                self.stop(session_id);
                deny3(DenyReason::Stopped)
            }
        }
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
    async fn script_payload(&self, action: &Action<'_>) -> Option<String> {
        let ActionKind::Shell {
            command,
            working_dir,
            ..
        } = &action.kind
        else {
            return None;
        };
        let script = self.script_content(command, working_dir.as_deref()).await;
        let build = self.build_context(command).await;
        match (script, build) {
            (None, None) => None,
            (Some(script), None) => Some(script),
            (None, Some(build)) => Some(build),
            (Some(script), Some(build)) => Some(format!("{script}\n{build}")),
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
        let bytes = tokio::fs::read(&resolved.abs).await.ok()?;
        let cut = bytes.len().min(prompts::MAX_SCRIPT_BYTES);
        let slice = bytes.get(..cut)?;
        Some(String::from_utf8_lossy(slice).into_owned())
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
            let Ok(bytes) = tokio::fs::read(&resolved.abs).await else {
                continue;
            };
            let budget = prompts::MAX_BUILD_CONTEXT_BYTES.saturating_sub(out.len());
            if budget < 64 {
                break;
            }
            let text = String::from_utf8_lossy(&bytes);
            let header = format!("--- {file} ---\n");
            let room = budget
                .saturating_sub(header.len() + 16)
                .min(BUILD_FILE_BYTES);
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

/// An escalation put to the human. Dropped unsettled (the tool call was
/// abandoned while waiting), it records a denial and withdraws the dialog.
struct PendingEscalation<'a> {
    engine: &'a ApprovalEngine,
    id: u64,
    record: Option<ApprovalId>,
    settled: bool,
}

impl PendingEscalation<'_> {
    /// The human answered.
    fn settle(&mut self, decision: ApprovalDecision, reason: &str) {
        self.record(decision, ledger::Decider::Human, reason);
    }

    /// No answer will come: deny it and close the dialog.
    fn settle_without_human(&mut self, reason: &str) {
        self.record(ApprovalDecision::Deny, ledger::Decider::Runtime, reason);
        self.engine
            .emit(ApprovalEvent::EscalationWithdrawn(self.id));
    }

    fn record(&mut self, decision: ApprovalDecision, by: ledger::Decider, reason: &str) {
        self.settled = true;
        if let (Some(ledger), Some(record)) = (&self.engine.ledger, &self.record)
            && let Err(err) = ledger.decide(record, decision, by, reason)
        {
            // A timed-out escalation's record has usually expired on its own.
            tracing::debug!(error = %err, "escalation decision not recorded");
        }
    }
}

impl Drop for PendingEscalation<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.settle_without_human("the tool call was abandoned before an answer");
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
        _ => serde_json::to_string_pretty(action.args).unwrap_or_default(),
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

/// Allow-cache key: tool name plus a hash of the key-sorted argument JSON.
fn cache_key(name: &str, args: &Value) -> CacheKey {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash_value(args, &mut hasher);
    (name.to_string(), hasher.finish())
}

fn hash_value(value: &Value, hasher: &mut impl Hasher) {
    match value {
        Value::Null => 0u8.hash(hasher),
        Value::Bool(b) => {
            1u8.hash(hasher);
            b.hash(hasher);
        }
        Value::Number(n) => {
            2u8.hash(hasher);
            n.to_string().hash(hasher);
        }
        Value::String(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        Value::Array(items) => {
            4u8.hash(hasher);
            items.len().hash(hasher);
            for item in items {
                hash_value(item, hasher);
            }
        }
        Value::Object(map) => {
            5u8.hash(hasher);
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            keys.len().hash(hasher);
            for key in keys {
                key.hash(hasher);
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
    use super::super::classifier::ClassifierError;
    use super::super::escalation::EscalationRequest;
    use super::super::policy::VerdictSummary;
    use super::super::rules::RuleKind;
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::ApprovalStatus;
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

    struct Rig {
        _dir: tempfile::TempDir,
        engine: Arc<ApprovalEngine>,
        classifier: Arc<Scripted>,
        escalations: mpsc::Receiver<EscalationRequest>,
        events: mpsc::UnboundedReceiver<ApprovalEvent>,
        session: SessionId,
    }

    fn rig(verdicts: Vec<Result<ClassifierVerdict, ClassifierError>>, timeout: Duration) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let root = ProjectRoot::new(dir.path()).unwrap();
        let classifier = Scripted::new(verdicts);
        let (broker, escalations) = EscalationBroker::new(4);
        let (tx, events) = mpsc::unbounded_channel();
        let engine = ApprovalEngine::new(ApprovalConfig {
            root: root.clone(),
            rules: RuleSet::default(),
            settings: AutoSettings::default(),
            classifier: Arc::clone(&classifier) as Arc<dyn ActionClassifier>,
            broker: Some(broker),
            ledger: Some(EscalationLedger::in_memory()),
            escalation_timeout: timeout,
            environment: Environment::basic(root.as_path(), root.as_path()),
            posix_shell: true,
            events: tx,
            rules_path: None,
        });
        Rig {
            _dir: dir,
            engine,
            classifier,
            escalations,
            events,
            session: SessionId::new(),
        }
    }

    fn push() -> Value {
        json!({"command": "git push --force origin main"})
    }

    async fn decide(rig: &Rig, id: &str, args: &Value) -> Verdict {
        rig.engine.decide(&rig.session, id, "shell", args).await
    }

    fn drain(events: &mut mpsc::UnboundedReceiver<ApprovalEvent>) -> Vec<ApprovalEvent> {
        let mut out = Vec::new();
        while let Ok(event) = events.try_recv() {
            out.push(event);
        }
        out
    }

    #[test]
    fn cache_key_ignores_key_order() {
        let a = json!({"a": 1, "b": [true, null, "x"]});
        let b: Value = serde_json::from_str(r#"{"b": [true, null, "x"], "a": 1}"#).unwrap();
        assert_eq!(cache_key("t", &a), cache_key("t", &b));
        assert_ne!(cache_key("t", &a), cache_key("u", &a));
        assert_ne!(cache_key("t", &a), cache_key("t", &json!({"a": 2})));
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
        let verdict = rig
            .engine
            .decide(
                &rig.session,
                "c1",
                "read_file",
                &json!({"path": "src/lib.rs"}),
            )
            .await;
        assert!(verdict.is_allowed());
        assert_eq!(rig.classifier.calls(), 0);
    }

    #[tokio::test]
    async fn classifier_allows_are_cached_per_session() {
        let mut rig = rig(vec![], DEFAULT_ESCALATION_TIMEOUT);
        let args = push();
        assert!(decide(&rig, "c1", &args).await.is_allowed());
        assert!(decide(&rig, "c2", &args).await.is_allowed());
        assert_eq!(
            rig.classifier.calls(),
            1,
            "second call served from the cache"
        );
        // Another session has its own cache.
        rig.session = SessionId::new();
        assert!(decide(&rig, "c3", &args).await.is_allowed());
        assert_eq!(rig.classifier.calls(), 2);
        let events = drain(&mut rig.events);
        assert!(events.contains(&ApprovalEvent::Classifying(true)));
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
        let engine = Arc::clone(&rig.engine);
        let session = rig.session.clone();
        let task =
            tokio::spawn(async move { engine.decide(&session, "c4", "shell", &push()).await });
        let request = rig.escalations.recv().await.unwrap();
        assert_eq!(request.tool, "shell");
        assert!(request.classifier_hint.unwrap().contains("D4"));
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
        let engine = Arc::clone(&rig.engine);
        let session = rig.session.clone();
        let task =
            tokio::spawn(async move { engine.decide(&session, "c1", "shell", &push()).await });
        let request = rig.escalations.recv().await.unwrap();
        let rule = Rule::literal(
            RuleKind::Allow,
            "shell",
            request.subjects[0].clone(),
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
        assert_eq!(rig.engine.rules().allow.len(), 1);
    }

    #[tokio::test]
    async fn an_unanswered_escalation_times_out_as_a_denial() {
        let mut rig = rig(
            vec![Err(ClassifierError::Timeout)],
            Duration::from_millis(50),
        );
        let engine = Arc::clone(&rig.engine);
        let session = rig.session.clone();
        let task =
            tokio::spawn(async move { engine.decide(&session, "c1", "shell", &push()).await });
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
        assert!(drain(&mut rig.events).contains(&ApprovalEvent::EscalationWithdrawn(request.id)));
        // The record expired with the escalation.
        let records = rig.engine.ledger().unwrap().records().unwrap();
        assert!(matches!(
            records[0].status,
            ApprovalStatus::Expired | ApprovalStatus::Denied
        ));
    }

    #[tokio::test]
    async fn an_abandoned_escalation_is_withdrawn() {
        let mut rig = rig(
            vec![Err(ClassifierError::Timeout)],
            DEFAULT_ESCALATION_TIMEOUT,
        );
        let engine = Arc::clone(&rig.engine);
        let session = rig.session.clone();
        let task =
            tokio::spawn(async move { engine.decide(&session, "c1", "shell", &push()).await });
        let request = rig.escalations.recv().await.unwrap();
        task.abort();
        let _ = task.await;
        assert!(drain(&mut rig.events).contains(&ApprovalEvent::EscalationWithdrawn(request.id)));
        let records = rig.engine.ledger().unwrap().records().unwrap();
        assert_eq!(records[0].status, ApprovalStatus::Denied);
    }

    #[tokio::test]
    async fn a_stopped_session_denies_escalations_until_its_next_run() {
        let mut rig = rig(
            vec![Err(ClassifierError::Timeout)],
            DEFAULT_ESCALATION_TIMEOUT,
        );
        rig.engine.stop(&rig.session);
        let verdict = decide(&rig, "c1", &push()).await;
        assert_eq!(
            verdict,
            Verdict::Deny {
                tier: Tier::Classifier,
                reason: DenyReason::Stopped
            }
        );
        assert!(rig.escalations.try_recv().is_err(), "nobody was asked");
        assert_eq!(rig.classifier.calls(), 0, "nor the classifier");
        // Calls that need no review still run.
        assert!(
            rig.engine
                .decide(&rig.session, "r1", "read_file", &json!({"path": "a"}))
                .await
                .is_allowed()
        );

        // A new run clears the stop.
        rig.engine.on_run_started(&rig.session);
        let engine = Arc::clone(&rig.engine);
        let session = rig.session.clone();
        let task =
            tokio::spawn(async move { engine.decide(&session, "c2", "shell", &push()).await });
        let request = rig.escalations.recv().await.unwrap();
        request.respond.send(HumanDecision::Stop).unwrap();
        assert!(!task.await.unwrap().is_allowed());
        // A Stop answer stops the session again.
        assert_eq!(
            decide(&rig, "c3", &push()).await,
            Verdict::Deny {
                tier: Tier::Classifier,
                reason: DenyReason::Stopped
            }
        );
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
        rig.engine.record_prompt(&rig.session, "USER-ASK deploy it");
        rig.engine.record_prompt(&rig.session, "never mind");
        assert!(rig.engine.forget_prompt(&rig.session, "never mind"));
        let _ = rig
            .engine
            .decide(&rig.session, "c0", "read_file", &json!({"path": "a"}))
            .await;
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
    async fn allowed_writes_are_listed_as_agent_written() {
        let rig = rig(vec![], DEFAULT_ESCALATION_TIMEOUT);
        let args = json!({"path": "src/new.rs", "content": "x"});
        assert!(
            rig.engine
                .decide(&rig.session, "w1", "write_file", &args)
                .await
                .is_allowed()
        );
        assert_eq!(rig.engine.agent_written(), vec!["src/new.rs".to_string()]);
    }
}
