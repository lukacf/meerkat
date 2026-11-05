//! `/model` across endpoints: the picker, switching, and the context-gauge
//! window.
//!
//! Every endpoint's model list is fetched in parallel (each request with its
//! own timeout) in the background at start-up, unless start-up already
//! fetched them to resolve a bare `--model` id, and cached for the process;
//! `/model` opens the picker from the cache (fetching when it is empty),
//! `/model refresh` refetches. The lists come from the endpoints only, never
//! from the `meerkat-models` catalog. The cache also resolves bare ids (see
//! [`crate::endpoints::parse_model_ref`]).
//!
//! A switch goes through the runtime's session-LLM reconfigure host
//! ([`crate::session::SessionHandle::switch_model`]) between turns, after
//! making sure the engine has a capability profile for the target
//! ([`crate::bootstrap::Runtime::ensure_profile`]); the choice is recorded in
//! the session's sidecar (`<realm>/tui_endpoints/<id>.toml`).
//!
//! The gauge's window is the endpoint's own number (Anthropic and Gemini
//! list it, LiteLLM's `/model/info` reports it); else the engine registry's
//! number for the model (the catalog or a `[models.<id>]` entry); else
//! [`DEFAULT_CONTEXT_WINDOW`], shown as an estimate.

use tokio::task::JoinHandle;

use super::Runner;
use crate::endpoints::registry_context_window;
use crate::endpoints::{EndpointRef, ModelRefError, needs_lookup, parse_model_ref, save_choice};
use crate::models::{FetchResults, ModelCache, ModelGroup, with_current};
use crate::ui::{Msg, NoticeLevel, StatusUpdate};

/// The context window assumed when neither the endpoint nor the engine's
/// model registry knows the model's (shown as `~`, an estimate).
pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;

/// A fetch of every endpoint's list running in the background.
pub(super) type ModelFetch = JoinHandle<FetchResults>;

impl Runner {
    /// The current endpoint and model.
    pub(super) fn current_choice(&self) -> EndpointRef {
        EndpointRef::new(self.current_endpoint.clone(), self.session.model())
    }

    /// Record the current choice next to the session, for `-c`/`/resume`.
    pub(super) fn save_current_choice(&self) {
        let realm = self.runtime.scope.realm_root();
        if let Err(err) = save_choice(
            &realm,
            &self.session.id().to_string(),
            &self.current_choice(),
        ) {
            tracing::warn!(error = %err, "could not record the session's endpoint");
        }
    }

    /// Start the background fetch unless one is running.
    pub(super) fn ensure_fetch(&mut self, announce: bool) {
        if self.model_fetch.is_some() {
            return;
        }
        let endpoints = self
            .runtime
            .endpoints
            .iter()
            .filter(|e| e.has_key())
            .cloned()
            .collect();
        let lister = std::sync::Arc::clone(&self.runtime.lister);
        self.model_fetch = Some(tokio::spawn(async move { lister(endpoints).await }));
        if announce {
            self.notice(NoticeLevel::Info, "fetching the model lists…");
        }
    }

    /// Answer [`crate::ui::Effect::RequestModelPicker`].
    pub(super) fn request_model_picker(&mut self, refresh: bool) {
        if refresh {
            self.models = None;
        }
        if self.models.is_some() || self.runtime.endpoints.is_empty() {
            self.send(Msg::ShowModelPicker(self.picker_groups()));
            return;
        }
        self.picker_waiting = true;
        self.ensure_fetch(true);
    }

    /// The picker: one group per endpoint, the current endpoint first with
    /// the current model first.
    pub(super) fn picker_groups(&self) -> Vec<ModelGroup> {
        let endpoints = &self.runtime.endpoints;
        let current = endpoints.iter().filter(|e| e.name == self.current_endpoint);
        let others = endpoints.iter().filter(|e| e.name != self.current_endpoint);
        current
            .chain(others)
            .map(|endpoint| {
                let listed = self.models.as_ref().and_then(|m| m.get(&endpoint.name));
                let (models, warning) = match listed {
                    Some(Ok(list)) => (list.clone(), None),
                    Some(Err(warning)) => (Vec::new(), Some(warning.clone())),
                    None if !endpoint.has_key() => (
                        Vec::new(),
                        Some("model listing needs an API key the TUI can read".to_string()),
                    ),
                    None => (Vec::new(), None),
                };
                let current = if endpoint.name == self.current_endpoint {
                    self.session.model()
                } else {
                    ""
                };
                ModelGroup {
                    endpoint: endpoint.name.clone(),
                    dialect: endpoint.dialect,
                    base_url: endpoint.base_url.clone(),
                    models: with_current(current, models),
                    warning,
                }
            })
            .collect()
    }

    /// The background fetch finished: cache the lists and their prices,
    /// update the gauge, open the picker and finish a `/model <id>` waiting
    /// for them.
    pub(super) async fn model_fetch_finished(
        &mut self,
        joined: Result<FetchResults, tokio::task::JoinError>,
    ) {
        self.model_fetch = None;
        let cache = match joined {
            Ok(results) => {
                let (cache, prices) = ModelCache::from_results(results);
                self.set_endpoint_prices(prices);
                cache
            }
            Err(err) => {
                tracing::warn!(error = %err, "list-models task failed");
                ModelCache::failed(
                    self.runtime.endpoints.names(),
                    &format!("list-models task failed: {err}"),
                )
            }
        };
        self.models = Some(cache);
        self.push_context_window();
        if std::mem::take(&mut self.picker_waiting) {
            self.send(Msg::ShowModelPicker(self.picker_groups()));
        }
        if let Some(reference) = self.pending_set.take() {
            if self.turn.is_active() {
                self.notice(
                    NoticeLevel::Warn,
                    format!("/model {reference} dropped: a turn started meanwhile"),
                );
            } else {
                self.set_model(reference).await;
            }
        }
    }

    /// Resolve `/model <reference>` against the cached lists.
    fn resolve_ref(&self, reference: &str) -> Result<EndpointRef, ModelRefError> {
        let index = self
            .models
            .as_ref()
            .map(ModelCache::index)
            .unwrap_or_default();
        parse_model_ref(
            reference,
            self.runtime.endpoints.as_slice(),
            &index,
            Some(&self.current_endpoint),
        )
    }

    /// `/model <reference>` or a picker selection: switch the live session's
    /// model between turns.
    ///
    /// A bare id is looked up in the cached lists (fetched first when the
    /// cache is empty and several endpoints could serve it).
    pub(super) async fn set_model(&mut self, reference: String) {
        if self.turn.is_active() {
            self.notice(
                NoticeLevel::Warn,
                "a turn is running; switch models once it ends (or interrupt it)",
            );
            return;
        }
        if self.models.is_none() && needs_lookup(&reference, self.runtime.endpoints.as_slice()) {
            self.pending_set = Some(reference);
            self.ensure_fetch(true);
            return;
        }
        let failed = |err: String| format!("model not changed: {err}");
        let target = match self.resolve_ref(&reference) {
            Ok(target) => target,
            Err(err) => {
                self.notice(NoticeLevel::Error, failed(err.to_string()));
                return;
            }
        };
        let choice = match self.runtime.choice_for(&target) {
            Ok(choice) => choice,
            Err(err) => {
                self.notice(NoticeLevel::Error, failed(err.to_string()));
                return;
            }
        };
        let listed = self
            .models
            .as_ref()
            .and_then(|m| m.entry(&choice.endpoint, &choice.model));
        if let Err(err) = self.runtime.ensure_profile(&choice, listed).await {
            self.notice(NoticeLevel::Error, failed(err.to_string()));
            return;
        }
        // The live agent's model registry predates the profile: rebuild the
        // agent from the persisted session first.
        if self.session.profiles() < self.runtime.profiles()
            && let Err(err) = self.rebuild_session().await
        {
            self.notice(NoticeLevel::Error, failed(err.to_string()));
            return;
        }
        match self.session.switch_model(&choice).await {
            Ok(()) => {
                self.current_endpoint.clone_from(&choice.endpoint);
                self.send(Msg::Status(StatusUpdate::Model {
                    provider: self.current_endpoint.clone(),
                    model: choice.model.clone(),
                }));
                self.push_context_window();
                self.save_current_choice();
            }
            Err(err) => self.notice(NoticeLevel::Error, failed(err.to_string())),
        }
    }

    /// Take the session off the runtime and make it live again, so its agent
    /// is rebuilt from the current config (between turns only).
    async fn rebuild_session(&mut self) -> Result<(), crate::error::TuiError> {
        let id = self.session.id().clone();
        self.session.retire().await?;
        self.session =
            crate::session::SessionHandle::resume(&self.runtime, &id, self.template.options(None))
                .await?;
        self.follow_events().await;
        Ok(())
    }

    /// Push the current model's context window (see the module docs).
    pub(super) fn push_context_window(&self) {
        let model = self.session.model();
        let listed = self
            .models
            .as_ref()
            .and_then(|cache| cache.entry(&self.current_endpoint, model))
            .and_then(|entry| entry.context_window);
        let update = match listed.or_else(|| {
            registry_context_window(&self.runtime.config, self.session.provider(), model)
        }) {
            Some(window) => StatusUpdate::ContextWindow(Some(window)),
            None => StatusUpdate::ApproxContextWindow(DEFAULT_CONTEXT_WINDOW),
        };
        self.send(Msg::Status(update));
    }
}
