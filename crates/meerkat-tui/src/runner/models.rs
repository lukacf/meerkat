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
//! ([`crate::bootstrap::Runtime::ensure_profile`]); the session's metadata
//! then records the model and binding the endpoint is derived from.
//!
//! The gauge's (and the compaction policy's) window is the engine's model
//! profile's: the catalog, or the realm's `[models.<id>]` entry, which a
//! fetched listing keeps current (Anthropic and Gemini list windows,
//! LiteLLM's `/model/info` reports them); else [`DEFAULT_CONTEXT_WINDOW`],
//! shown as an estimate. A fetch finishing in the background refreshes the
//! current model's profile and the gauge, but never rebuilds the session:
//! a changed compaction trigger applies at the next switch the user makes.

use std::sync::Arc;

use meerkat_core::Provider;
use tokio::task::JoinHandle;

use super::Runner;
use super::context::RebuildReason;
use crate::bootstrap::Runtime;
use crate::endpoints::registry_context_window;
use crate::endpoints::{EndpointRef, ModelRefError, needs_lookup, parse_model_ref};
use crate::models::{FetchResults, ListModelsError, ModelCache, ModelGroup, fetch_warning, with_current};
use crate::ui::{Msg, NoticeLevel, StatusUpdate};

/// The context window assumed when neither the endpoint nor the engine's
/// model registry knows the model's (shown as `~`, an estimate).
pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;

/// A fetch of every endpoint's list running in the background.
pub(super) type ModelFetch = JoinHandle<FetchResults>;

impl Runner {
    /// The current endpoint and model, when an endpoint serves the session.
    pub(super) fn current_choice(&self) -> Option<EndpointRef> {
        self.current_endpoint
            .as_ref()
            .map(|endpoint| EndpointRef::new(endpoint.as_str(), self.identity.model.clone()))
    }

    /// The current endpoint's name, or `""` when none serves the session.
    fn current_endpoint_name(&self) -> &str {
        self.current_endpoint
            .as_ref()
            .map_or("", crate::endpoints::EndpointName::as_str)
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
        let current_name = self.current_endpoint_name();
        let current = endpoints.iter().filter(|e| e.name == current_name);
        let others = endpoints.iter().filter(|e| e.name != current_name);
        current
            .chain(others)
            .map(|endpoint| {
                let listed = self.models.as_ref().and_then(|m| m.get(&endpoint.name));
                let (models, warning) = match listed {
                    Some(Ok(list)) => (list.clone(), None),
                    Some(Err(err)) => (Vec::new(), Some(fetch_warning(err))),
                    None if !endpoint.has_key() => (
                        Vec::new(),
                        Some("model listing needs an API key the TUI can read".to_string()),
                    ),
                    None => (Vec::new(), None),
                };
                let current = if endpoint.name == current_name {
                    self.identity.model.as_str()
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
    /// refresh the current model's profile from its listing and the gauge,
    /// open the picker and finish a `/model <id>` waiting for them. The
    /// session is not rebuilt here (see the module docs).
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
                    &Arc::new(ListModelsError::Task(err)),
                )
            }
        };
        let listed = cache
            .entry(self.current_endpoint_name(), &self.identity.model)
            .cloned();
        self.models = Some(cache);
        if let Some(listed) = listed
            && let Err(err) = self
                .runtime
                .ensure_profile(self.identity.provider, &self.identity.model, Some(&listed))
                .await
        {
            tracing::warn!(error = %err, "cannot refresh the model profile from its listing");
        }
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
            self.current_endpoint.as_ref().map(crate::endpoints::EndpointName::as_str),
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
            .and_then(|m| m.entry(choice.endpoint.as_str(), &choice.model));
        if let Err(err) = self
            .runtime
            .ensure_profile(choice.provider, &choice.model, listed)
            .await
        {
            self.notice(NoticeLevel::Error, failed(err.to_string()));
            return;
        }
        // The live agent's model registry predates the profile: rebuild the
        // agent from the persisted session first.
        if self.session.profiles() < self.runtime.profiles()
            && let Err(err) = self
                .rebuild_session_workaround(RebuildReason::ModelProfile, self.session.compaction())
                .await
        {
            self.notice(NoticeLevel::Error, failed(err.to_string()));
            return;
        }
        match self.session.switch_model(&choice).await {
            Ok(()) => {
                self.refresh_identity().await;
                // The agent's mob tools name the model `delegate` helpers run
                // on as of its build: rebuild it, with the new model's
                // trigger, so they follow the switch.
                let trigger = if self.compact_now_pending {
                    self.session.compaction()
                } else {
                    self.session_trigger()
                };
                if let Err(err) = self
                    .rebuild_session_workaround(RebuildReason::ModelSwitch, trigger)
                    .await
                {
                    self.notice(
                        NoticeLevel::Warn,
                        format!("helpers keep the previous model until the session is rebuilt: {err}"),
                    );
                }
                self.send(Msg::Status(StatusUpdate::Model {
                    provider: self.current_endpoint_name().to_string(),
                    model: self.identity.model.clone(),
                }));
                self.push_context_window();
                // The new model's window moves an auto-mode trigger.
                self.ensure_trigger().await;
            }
            Err(err) => self.notice(NoticeLevel::Error, failed(err.to_string())),
        }
    }

    /// The current model's context window: known, or
    /// [`DEFAULT_CONTEXT_WINDOW`] as an estimate (`true`).
    pub(super) fn context_window(&self) -> (u64, bool) {
        window_of(&self.runtime, self.identity.provider, &self.identity.model)
    }

    /// Push the current model's context window (see the module docs).
    pub(super) fn push_context_window(&self) {
        let update = match self.context_window() {
            (window, false) => StatusUpdate::ContextWindow(Some(window)),
            (window, true) => StatusUpdate::ApproxContextWindow(window),
        };
        self.send(Msg::Status(update));
    }
}

/// The context window of `model` on `provider`: the engine's model
/// profile's (see the module docs), else [`DEFAULT_CONTEXT_WINDOW`] as an
/// estimate (`true`).
pub(crate) fn window_of(runtime: &Runtime, provider: Provider, model: &str) -> (u64, bool) {
    registry_context_window(&runtime.effective.read(), provider, model)
        .map_or((DEFAULT_CONTEXT_WINDOW, true), |window| (window, false))
}
