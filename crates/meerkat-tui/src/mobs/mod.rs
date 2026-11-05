//! Helpers (sub-agents) as Meerkat mob members.
//!
//! The agent delegates with upstream's mob tools (`delegate`: an ephemeral
//! helper in the session's implicit mob, which runs one turn and returns
//! its bounded result; the `mob_*` tools for longer-lived teams). The TUI
//! hosts them with a [`MobMcpState`] over its own runtime-backed service and
//! [`MeerkatMachine`] (mob state durable under the realm), offered to every
//! session build through the builder's default mob-tools slot, and adds
//! what upstream leaves to the host:
//!
//! - **The approval gate.** [`customizer::HelperCustomizer`] (the state's
//!   child spawn-member customizer, asked for every member of every mob the
//!   state creates or restores) puts an [`crate::approval::ApprovalGate`] for
//!   the helper's owner on the spawn spec's `tool_dispatch_admission`: the
//!   helper's calls are judged against the owner's transcript of the user's
//!   words (the helper's task is assistant-authored and never recorded as
//!   user intent), share its allow cache, Stop and the denial counters, and
//!   the escalation dialog names the helper.
//! - **The model.** A `delegate` helper runs on its delegator's model (the
//!   implicit mob's profile); the customizer adds the owner's auth binding,
//!   so a helper of a configured endpoint's session reaches the same
//!   endpoint.
//! - **Tools and instructions.** The TUI's file tools and
//!   [`crate::bootstrap::HELPER_PERSONA`].
//! - **Observation.** [`follow`] subscribes to each helper and forwards its
//!   events, scoped to it, as [`HelperUpdate`]s; the runner sends them to
//!   the UI (nested blocks, the running-helpers count, per-helper usage for
//!   `/cost`).
//! - **Interrupts.** [`Helpers::cancel_for`] force-cancels the running
//!   helpers of a session when its turn is interrupted.

pub mod registry;

#[cfg(feature = "mob")]
pub mod customizer;
#[cfg(feature = "mob")]
mod follow;

use std::sync::Arc;

use meerkat_core::{AuthBindingRef, SessionId};
use parking_lot::Mutex;
use tokio::sync::mpsc;

pub use registry::{HelperRegistry, HelperStatus, HelperTag, HelperUpdate};

#[cfg(feature = "mob")]
use meerkat_mob_mcp::MobMcpState;
#[cfg(feature = "mob")]
use meerkat_runtime::MeerkatMachine;

/// The TUI's helper host.
pub struct Helpers {
    registry: Arc<HelperRegistry>,
    updates: Mutex<Option<mpsc::UnboundedReceiver<HelperUpdate>>>,
    #[cfg(feature = "mob")]
    state: Arc<MobMcpState>,
}

impl std::fmt::Debug for Helpers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Helpers")
            .field("live", &self.registry.live())
            .finish_non_exhaustive()
    }
}

/// What [`Helpers::start`] needs.
#[cfg(feature = "mob")]
pub struct HelperHostParts {
    /// The session service members are created on.
    pub service: Arc<crate::bootstrap::TuiService>,
    /// The runtime members register with.
    pub machine: Arc<MeerkatMachine>,
    /// Realm directory: the mob state lives under it.
    pub realm_root: std::path::PathBuf,
    /// The approval engine whose gate helpers get.
    pub engine: Arc<crate::approval::ApprovalEngine>,
    /// Builds a helper's external tools.
    pub tools: customizer::HelperTools,
    /// Client every member uses instead of resolving credentials (tests).
    pub llm_override: Option<Arc<dyn meerkat::LlmClient>>,
    /// The realm's WorkGraph service: a member build that resolves the
    /// WorkGraph tools on gets its mob's scoped service and namespace grant
    /// from it (the factory refuses WorkGraph tools without a grant).
    pub workgraph: meerkat::WorkGraphService,
}

#[cfg(feature = "mob")]
impl Helpers {
    /// Host helpers on `parts.service`; returns the host and the mob-tools
    /// factory every session build gets.
    ///
    /// # Errors
    ///
    /// [`crate::TuiError::Runtime`] when the mob state cannot attach to the
    /// runtime.
    pub fn start(
        parts: HelperHostParts,
    ) -> Result<(Arc<Self>, Arc<dyn meerkat_core::service::MobToolsFactory>), crate::TuiError> {
        let HelperHostParts {
            service,
            machine,
            realm_root,
            engine,
            tools,
            llm_override,
            workgraph,
        } = parts;
        let (registry, updates) = HelperRegistry::new();
        let registry = Arc::new(registry);
        let (spawning_tx, spawning_rx) = mpsc::unbounded_channel();
        let customizer =
            customizer::HelperCustomizer::new(Arc::clone(&registry), engine, tools, spawning_tx);
        let state = MobMcpState::new_with_runtime_adapter(
            service as Arc<dyn meerkat_mob::MobSessionService>,
            Some(machine),
            meerkat_mob::MobControlPrincipal::Owner,
        )?
        .with_persistent_storage_root(Some(realm_root))
        .with_workgraph_service(Some(workgraph))
        .with_default_llm_client(llm_override)
        .with_child_spawn_member_customizer(Arc::new(customizer))
        .into_shared();
        tokio::spawn(follow::watch(
            Arc::downgrade(&state),
            Arc::clone(&registry),
            spawning_rx,
        ));
        let factory = Arc::new(meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(
            Arc::clone(&state),
        ));
        Ok((
            Arc::new(Self {
                registry,
                updates: Mutex::new(Some(updates)),
                state,
            }),
            factory,
        ))
    }

    /// The mob host (tests and diagnostics).
    #[must_use]
    pub fn state(&self) -> &Arc<MobMcpState> {
        &self.state
    }

    /// Stop every running helper that works for `owner` (its turn was
    /// interrupted): force-cancel its run, then retire it. The `delegate`
    /// call that would have retired it is abandoned with the owner's run, so
    /// without the retire the helper would stay seated. Failures are logged:
    /// the gate already denies the helpers' reviewed calls once the owner is
    /// stopped.
    pub async fn cancel_for(&self, owner: &SessionId) {
        for (mob_id, identity) in self.registry.live_for(owner) {
            let mob = meerkat_mob::MobId::from(mob_id);
            let handle = match self.state.handle_for(&mob).await {
                Ok(handle) => handle,
                Err(err) => {
                    tracing::warn!(helper = %identity, mob = %mob, error = %err, "cannot reach helper's mob");
                    continue;
                }
            };
            let member = meerkat_mob::AgentIdentity::from(identity.clone());
            if let Err(err) = handle.force_cancel_member(member.clone()).await {
                tracing::debug!(helper = %identity, error = %err, "helper run not cancelled");
            }
            if let Err(err) = handle.retire(member).await {
                tracing::warn!(helper = %identity, mob = %mob, error = %err, "cannot retire helper");
            }
        }
    }
}

impl Helpers {
    /// The bookkeeping shared with the customizer and the followers.
    #[must_use]
    pub fn registry(&self) -> &Arc<HelperRegistry> {
        &self.registry
    }

    /// `session` runs on `model` through `auth_binding`: its helpers inherit
    /// both.
    pub fn register_session(
        &self,
        session: &SessionId,
        model: &str,
        auth_binding: Option<&AuthBindingRef>,
    ) {
        self.registry.register_session(session, model, auth_binding);
    }

    /// The helper updates for the UI (taken once, by the runner).
    pub fn take_updates(&self) -> Option<mpsc::UnboundedReceiver<HelperUpdate>> {
        self.updates.lock().take()
    }

    /// Running helpers.
    #[must_use]
    pub fn live(&self) -> Vec<HelperStatus> {
        self.registry.live()
    }
}

#[cfg(not(feature = "mob"))]
impl Helpers {
    /// Without the `mob` feature there are no helpers to cancel.
    #[allow(clippy::unused_async)]
    pub async fn cancel_for(&self, _owner: &SessionId) {}
}
