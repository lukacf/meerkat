//! The approval engine every session's gate shares.
//!
//! [`load_layers`] layers `tui.toml` (global → realm → command line →
//! project; the realm layer is fully applied only when the host trust
//! record says rkat-tui wrote it or the user trusted it, see
//! [`crate::approval::trust`]). [`build`] reads the environment facts,
//! builds the classifier, records escalations through the realm's one
//! approval issuer (the persistence bundle's `approvals.json` owner) and
//! opens the escalation broker, and returns the engine with the channels
//! the runner serves.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, LlmClient};
use meerkat_core::{AuthBindingRef, Config, Provider, SessionLlmIdentity};
use tokio::sync::mpsc;

use crate::approval::classifier::{
    ActionClassifier, Environment, LlmClassifier, UnavailableClassifier, classifier_default_model,
};
use crate::approval::engine::DEFAULT_ESCALATION_TIMEOUT;
use crate::approval::escalation::{EscalationBroker, EscalationRequest};
use crate::approval::ledger::EscalationLedger;
use crate::approval::rules::{self, AutoSettings, RULES_FILE_NAME, RealmTrust};
use crate::approval::trust::HostTrustRecord;
use crate::approval::{ApprovalConfig, ApprovalEngine, ApprovalEvent};
use crate::cli::Args;
use crate::error::TuiError;
use crate::overlay::ConfigOverlay;
use crate::paths::ProjectRoot;
use crate::tools::ReadOnlyProjectTools;

use super::scope::RuntimeScope;

/// Capacity of the escalation channel.
const ESCALATION_CAPACITY: usize = 16;

/// Engine-level overrides for the approval setup (tests and embedders).
#[derive(Clone, Default)]
pub struct ApprovalOverrides {
    /// Review Tier-3 calls with this classifier instead of the LLM one.
    pub classifier: Option<Arc<dyn ActionClassifier>>,
    /// How long the human has to answer an escalation
    /// (default [`DEFAULT_ESCALATION_TIMEOUT`]).
    pub escalation_timeout: Option<Duration>,
}

/// The approval engine plus what the runner serves.
pub struct ApprovalParts {
    /// The engine every session's gate uses.
    pub engine: Arc<ApprovalEngine>,
    /// Escalations for the human (taken by the runner).
    pub escalations: Option<mpsc::Receiver<EscalationRequest>>,
    /// Decisions, pauses and classifier usage (taken by the runner).
    pub events: Option<mpsc::UnboundedReceiver<ApprovalEvent>>,
    /// Observations from loading the rules, shown once.
    pub notices: Vec<String>,
    /// The host trust record of this workspace and realm (`/rules trust`,
    /// and the project MCP config's opt-in).
    pub trust: Arc<parking_lot::Mutex<HostTrustRecord>>,
    /// The realm rules file.
    pub realm_rules: std::path::PathBuf,
}

impl std::fmt::Debug for ApprovalParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalParts")
            .field("engine", &self.engine)
            .field("notices", &self.notices)
            .finish_non_exhaustive()
    }
}

/// What the classifier runs on.
pub(crate) struct ClassifierSource<'a> {
    /// The factory the sessions are built with (its credential resolution
    /// builds the classifier's client).
    pub factory: &'a AgentFactory,
    /// Realm config.
    pub config: &'a Config,
    /// Provider of the start-up model.
    pub provider: Provider,
    /// The auth binding of the start-up endpoint (`None` for a built-in one).
    pub auth_binding: Option<AuthBindingRef>,
    /// Client every session uses instead of resolving credentials, if any.
    pub llm_override: Option<Arc<dyn LlmClient>>,
}

/// Load the layered `tui.toml` (global, realm, project, command line) of
/// `scope`: rules, `[auto]` settings, endpoints and price overrides, plus
/// the notices loading produced.
///
/// # Errors
///
/// [`TuiError::Overlay`] for invalid command-line rule flags,
/// [`TuiError::Approval`] when a rules file cannot be read.
pub(crate) fn load_layers(
    scope: &RuntimeScope,
    args: &Args,
) -> Result<(rules::Layered, Vec<String>), TuiError> {
    let cli_layer = ConfigOverlay::from_args(args)?.rules_file()?;
    let realm_dir = scope.realm_root();
    let realm_rules = realm_dir.join(RULES_FILE_NAME);
    let mut notices = Vec::new();
    let realm_trust = RealmTrust::from_status(trust_record(scope).check(&realm_rules));
    let global_rules = scope.global.as_ref().map(super::GlobalDirs::rules_file);
    let layered = rules::load_layered_with(
        &realm_dir,
        global_rules.as_deref(),
        &scope.context_root.join(".rkat"),
        realm_trust,
        cli_layer.as_ref(),
    )
    .map_err(crate::approval::ApprovalError::from)?;
    notices.extend(layered.notices.iter().map(ToString::to_string));
    if let Some(notice) = rules::legacy_global_rules_notice(
        scope.user_config_root.as_deref(),
        global_rules.as_deref(),
    ) {
        notices.push(notice.to_string());
    }
    Ok((layered, notices))
}

/// The host trust record of `scope`'s workspace and realm (in memory when
/// there is no user config directory: nothing is trusted across runs).
#[must_use]
pub fn trust_record(scope: &RuntimeScope) -> HostTrustRecord {
    match &scope.global {
        Some(global) => HostTrustRecord::open(&global.config, &scope.context_root, &scope.realm_id),
        None => HostTrustRecord::in_memory(&scope.context_root),
    }
}

/// Build the approval engine for `scope` from the loaded `layered` rules.
/// Escalations are recorded through `issuer`, the realm's one approval
/// issuer (`PersistenceBundle::approval_issuer`).
///
/// # Errors
///
/// [`TuiError::Io`] when the current directory cannot be read.
pub(crate) async fn build(
    scope: &RuntimeScope,
    layered: rules::Layered,
    mut notices: Vec<String>,
    source: ClassifierSource<'_>,
    overrides: &ApprovalOverrides,
    issuer: meerkat_core::ApprovalService,
) -> Result<ApprovalParts, TuiError> {
    let realm_dir = scope.realm_root();
    let realm_rules = realm_dir.join(RULES_FILE_NAME);
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (broker, escalations) = EscalationBroker::new(ESCALATION_CAPACITY);
    let cwd = std::env::current_dir().map_err(|e| TuiError::io("read current directory", e))?;
    let environment = Environment::detect(scope.project_root.as_path(), &cwd).await;
    let classifier = match &overrides.classifier {
        Some(classifier) => Arc::clone(classifier),
        None => {
            let (classifier, notice) = make_classifier(
                &layered.settings,
                source,
                scope.project_root.clone(),
                events_tx.clone(),
            )
            .await;
            notices.extend(notice);
            classifier
        }
    };
    let ledger = EscalationLedger::new(issuer);
    let trust = Arc::new(parking_lot::Mutex::new(trust_record(scope)));
    let engine = ApprovalEngine::new(ApprovalConfig {
        root: scope.project_root.clone(),
        rules: layered.rules,
        settings: layered.settings,
        classifier,
        broker: Some(broker),
        ledger: Some(ledger),
        escalation_timeout: overrides
            .escalation_timeout
            .unwrap_or(DEFAULT_ESCALATION_TIMEOUT),
        environment,
        posix_shell: shell_is_posix(),
        events: events_tx,
        rules_path: Some(realm_rules.clone()),
        rules_trust: Some(Arc::clone(&trust)),
    });
    Ok(ApprovalParts {
        engine,
        escalations: Some(escalations),
        events: Some(events_rx),
        notices,
        trust,
        realm_rules,
    })
}

/// The two-stage LLM classifier on the session's provider (model from
/// `[auto] classifier_model`, else [`classifier_default_model`]), built with
/// the factory's own credential resolution. Without a model or a client
/// every Tier-3 call is escalated to the human; the notice says why.
async fn make_classifier(
    settings: &AutoSettings,
    source: ClassifierSource<'_>,
    root: ProjectRoot,
    events: mpsc::UnboundedSender<ApprovalEvent>,
) -> (Arc<dyn ActionClassifier>, Option<String>) {
    let model = settings
        .classifier_model
        .clone()
        .or_else(|| classifier_default_model(source.provider).map(str::to_string));
    let Some(model) = model else {
        return (
            Arc::new(UnavailableClassifier),
            Some(format!(
                "no classifier model for provider {}: set [auto] classifier_model; until then every reviewed action asks you",
                source.provider.as_str()
            )),
        );
    };
    let client = match source.llm_override {
        Some(client) => client,
        None => {
            let identity = SessionLlmIdentity {
                model: model.clone(),
                provider: source.provider,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: source.auth_binding.clone(),
            };
            match source
                .factory
                .build_llm_client_for_identity(source.config, &identity)
                .await
            {
                Ok(client) => client,
                Err(err) => {
                    return (
                        Arc::new(UnavailableClassifier),
                        Some(format!(
                            "the safety classifier ({model}) is unavailable: {err}; every reviewed action asks you"
                        )),
                    );
                }
            }
        }
    };
    let mut classifier =
        LlmClassifier::new(client, source.provider, model).with_usage_events(events);
    // Stage 2 may read project files (read-only, bounded) to judge what a
    // command will actually execute.
    if settings.classifier_tools {
        classifier = classifier.with_project_tools(Arc::new(ReadOnlyProjectTools::new(root)));
    }
    (Arc::new(classifier), None)
}

/// Shells whose syntax the Tier-1 shell analysis understands.
const POSIX_SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "ash"];

/// The program the shell tool runs, as the tool resolves it (its default
/// shell, else, when that is `nu` and missing, `$SHELL`, bash, zsh or sh).
#[must_use]
pub fn resolved_shell_program() -> Option<std::path::PathBuf> {
    use meerkat_tools::builtin::shell::ShellConfig;

    let default = ShellConfig::default();
    if let Ok(path) = default.resolve_shell_path() {
        return Some(path);
    }
    if default.shell != "nu" {
        return None;
    }
    let shell_env = std::env::var("SHELL").ok();
    let mut candidates: Vec<String> = Vec::new();
    if let Some(env) = shell_env.as_deref().map(str::trim)
        && !env.is_empty()
        && env != default.shell
    {
        candidates.push(env.to_string());
    }
    for name in ["bash", "zsh", "sh"] {
        if !candidates.iter().any(|c| c == name) {
            candidates.push(name.to_string());
        }
    }
    candidates.into_iter().find_map(|shell| {
        ShellConfig {
            shell,
            ..ShellConfig::default()
        }
        .resolve_shell_path()
        .ok()
    })
}

/// Whether `program` is a POSIX shell.
#[must_use]
pub fn is_posix_shell(program: &Path) -> bool {
    program
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| POSIX_SHELLS.contains(&n))
}

/// Whether the shell tool will run a POSIX shell. Unknown or non-POSIX
/// programs (the engine default is `nu`) disable Tier-1 shell analysis.
#[must_use]
pub fn shell_is_posix() -> bool {
    cfg!(unix) && resolved_shell_program().is_some_and(|p| is_posix_shell(&p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_shell_names() {
        assert!(is_posix_shell(Path::new("/bin/bash")));
        assert!(is_posix_shell(Path::new("/usr/bin/sh")));
        assert!(!is_posix_shell(Path::new("/usr/bin/nu")));
        assert!(!is_posix_shell(Path::new("/usr/bin/fish")));
    }
}
