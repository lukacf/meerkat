//! Shared gateway construction seams (doctrine D2: converge construction,
//! merge binaries later).
//!
//! The K1/K2 field failures came from the two gateway binaries embedding the
//! same `UnifiedRuntime` with DIVERGENT wiring — `rpc_gateway` built the full
//! identity substrate, `meta_gateway` built none. Every piece both binaries
//! must construct identically belongs here so the drift class is structural
//! history. The full store construction converges with the 0.8 binary merge;
//! v1 shares the identity substrate, whose divergence caused the field
//! failures.
//!
//! The identity session bridge ([`identity_session_bridge`]) is shared by
//! both binaries and by `UnifiedRuntimeBuilder`: three compositions, one
//! construction. The member build wiring (`MemberBuildWiring`: the
//! customizer tool registry, its spawn customizer and the member build
//! preparer) is shared the same way by `UnifiedRuntimeBuilder`, the stdio
//! gateway and `UnifiedRuntime::bootstrap`.

use std::path::Path;
use std::sync::Arc;

use crate::identity_first::{
    AgentCustomizer, ContinuityStore, CustomizerToolRegistry, CustomizerToolsSpawnCustomizer,
    IdentityBuildPreparer, IdentityFirstRuntimeContext, IdentityRuntime, LocalContinuityStore,
    LocalLeaseProvider, contracts::LeaseProvider,
};

/// The durable-identity substrate a gateway builds from its state directory:
/// the continuity store and a lease provider whose fencing counter resumes
/// ABOVE the persisted high-water, so a restart with existing continuity
/// history never presents a stale token.
pub struct GatewayIdentitySubstrate {
    pub continuity_store: Arc<dyn ContinuityStore>,
    pub lease_provider: Arc<dyn LeaseProvider>,
}

/// Open the local identity substrate at `continuity_db`.
///
/// Fails loudly rather than degrading: a 0 fencing floor on an existing
/// store would re-arm the restart-abort class the floor seeding prevents.
pub async fn open_identity_substrate(
    continuity_db: &Path,
) -> Result<GatewayIdentitySubstrate, String> {
    let (store, fencing_floor) =
        LocalContinuityStore::open_with_fencing_floor(continuity_db.to_path_buf())
            .await
            .map_err(|e| {
                format!(
                    "failed to open continuity store and read fencing high-water at {}: {e}",
                    continuity_db.display()
                )
            })?;
    Ok(GatewayIdentitySubstrate {
        continuity_store: Arc::new(store),
        lease_provider: Arc::new(LocalLeaseProvider::with_floor(fencing_floor)),
    })
}

/// The mob handle a gateway gives the identity runtime it installs, for its
/// services and session bridge. The installed identity runtime lives inside
/// the composition the public owner keeps alive, so this is a back-edge and
/// does not own the mob (D67), as in the builder: an owning handle there
/// would keep admission open after the last external owner drops, and the
/// runtime's retained teardown would wait on that close indefinitely.
#[must_use]
pub fn identity_mob_handle(runtime: &crate::UnifiedRuntime) -> meerkat_mob::MobHandle {
    runtime.mob_handle().observer()
}

/// The identity-first session bridge a composition installs: Meta's bridge
/// over `handle`, with every seam `mob_runtime` recorded at its bootstrap.
/// `UnifiedRuntimeBuilder` and both gateway binaries build their identity
/// bridge here, so none of them can leave a seam out. The builder once built
/// its bridge without the controller pin source, and every governed send on
/// its runtime was refused after Meta had woken and built the member.
#[must_use]
pub fn identity_session_bridge(
    mob_runtime: &crate::mob_handle_runtime::MobRuntime,
    handle: meerkat_mob::MobHandle,
) -> crate::identity_first::MobSessionBridge {
    let mut bridge = match mob_runtime.session_service().cloned() {
        Some(service) => {
            crate::identity_first::MobSessionBridge::with_session_service(handle, service)
        }
        None => crate::identity_first::MobSessionBridge::new(handle),
    };
    // Heal seam (2026-07-29 incident): the continuity repair supervisor asks
    // this recoverer to commit the durable head before declaring an identity
    // healed; without it, heal is a cosmetic entry reset that the next
    // materialization re-Breaks.
    if let Some(recoverer) = mob_runtime.committed_boundary_recoverer() {
        bridge = bridge.with_committed_boundary_recoverer(recoverer);
    }
    // Image deliveries are judged with the member builder's host config, so a
    // self-hosted model's `vision` flag counts.
    if let Some(config) = mob_runtime.member_model_config() {
        bridge = bridge.with_member_model_config(config);
    }
    // Governed binding pins the member session actor's actual controller
    // through the concrete session owner, and a live MCP attachment reaches
    // the current member through it.
    if let Some(source) = mob_runtime.controller_pin_source() {
        bridge = bridge.with_controller_pin_source(source);
    }
    bridge
}

/// The member build wiring a composition installs on its mob and binds to
/// its identity runtime: the customizer tool registry, the spawn customizer
/// that attaches each registered identity's stable customizer tool
/// dispatcher to every build of its member, and Meta's member build
/// preparer, which publishes the member's tools right before Core builds
/// it. `UnifiedRuntimeBuilder`, the stdio gateway and
/// `UnifiedRuntime::bootstrap` all wire their mob here. A runtime the
/// bootstrap composed once had no preparer, and a member it restored was
/// built without its customizer tools.
pub(crate) struct MemberBuildWiring {
    /// The registry the spawn customizer attaches from, when the composition
    /// decided at construction that it composes an agent customizer.
    registry: Option<Arc<CustomizerToolRegistry>>,
    spawn_customizer: Option<Arc<CustomizerToolsSpawnCustomizer>>,
    preparer: Option<Arc<IdentityBuildPreparer>>,
    /// The agent customizer the composition's spec declares
    /// (`MobBootstrapSpec::with_agent_customizer`), which the identity
    /// bootstrap installs.
    declared_customizer: Option<Arc<dyn AgentCustomizer>>,
}

impl MemberBuildWiring {
    /// The wiring of a composition that knows when it builds its mob whether
    /// it composes an agent customizer (`customizer_tools`) and whether an
    /// identity runtime prepares the mob's member builds (`prepares`).
    pub(crate) fn new(customizer_tools: bool, prepares: bool) -> Self {
        let registry = customizer_tools.then(CustomizerToolRegistry::new);
        Self {
            spawn_customizer: registry
                .clone()
                .map(|registry| Arc::new(CustomizerToolsSpawnCustomizer::new(registry))),
            preparer: prepares.then(|| IdentityBuildPreparer::new(registry.is_some())),
            registry,
            declared_customizer: None,
        }
    }

    /// The wiring of a `UnifiedRuntime::bootstrap` composition whose spec
    /// declares its agent customizer: armed when the mob is built, as
    /// [`Self::new`] arms a composition with a customizer, so a build that
    /// comes before the identity bootstrap is held until the roster pass
    /// publishes its tools. The identity bootstrap installs `customizer` and
    /// binds the preparer ([`Self::bind_at_identity_bootstrap`]).
    pub(crate) fn declared(customizer: Arc<dyn AgentCustomizer>) -> Self {
        Self {
            declared_customizer: Some(customizer),
            ..Self::new(true, true)
        }
    }

    /// The wiring of a `UnifiedRuntime::bootstrap` composition whose spec
    /// declares no agent customizer: it learns only at its identity
    /// bootstrap whether it composes one, so its spawn customizer attaches
    /// nothing and its preparer is inert until
    /// [`Self::bind_at_identity_bootstrap`] finds one.
    pub(crate) fn deferred() -> Self {
        Self {
            registry: None,
            spawn_customizer: Some(Arc::new(CustomizerToolsSpawnCustomizer::deferred())),
            preparer: Some(IdentityBuildPreparer::deferred()),
            declared_customizer: None,
        }
    }

    /// Install the spawn customizer, composed after those `spec` carries,
    /// and the preparer on `spec`.
    pub(crate) fn install(
        &self,
        spec: crate::mob_handle_runtime::MobBootstrapSpec,
    ) -> crate::mob_handle_runtime::MobBootstrapSpec {
        let spec = match self.spawn_customizer.clone() {
            Some(customizer) => spec.with_spawn_member_customizer(customizer),
            None => spec,
        };
        match self.preparer.clone() {
            Some(preparer) => spec.with_member_build_preparer(preparer),
            None => spec,
        }
    }

    /// The registry the composition installs on its identity runtime, the
    /// one its spawn customizer attaches from.
    pub(crate) fn customizer_tool_registry(&self) -> Option<Arc<CustomizerToolRegistry>> {
        self.registry.clone()
    }

    /// Bind the preparer to `runtime` once its customizer, customizer tool
    /// registry and identity records are installed: the builds it held
    /// before are pending on the runtime, and the roster pass publishes them.
    pub(crate) async fn bind(
        &self,
        runtime: &Arc<IdentityRuntime>,
        session_service: Option<Arc<dyn meerkat_mob::MobSessionService>>,
    ) {
        if let Some(preparer) = self.preparer.as_ref() {
            preparer.bind(runtime, session_service).await;
        }
    }

    /// Bind a `UnifiedRuntime::bootstrap` wiring to the identity runtime of
    /// `context`, before its identity bootstrap lifts the mob, composing the
    /// runtime as `UnifiedRuntimeBuilder` composes it.
    ///
    /// The customizer is the one the spec declared; without a declaration,
    /// the one installed on the runtime, else the one the context carries.
    /// A declared customizer allows the runtime and the context to carry the
    /// same customizer or none: any other one is refused, as is a runtime
    /// whose customizer tool registry is not the one the mob was built with.
    /// Without any customizer nothing changes: a deferred wiring stays inert.
    /// With one, it becomes the runtime's own when none is installed, the
    /// runtime gets the composition's registry when it has none, the spawn
    /// customizer attaches from the runtime's registry, and the preparer
    /// publishes tools and is bound. A refusal changes nothing.
    pub(crate) async fn bind_at_identity_bootstrap(
        &self,
        context: &IdentityFirstRuntimeContext,
        session_service: Option<Arc<dyn meerkat_mob::MobSessionService>>,
    ) -> Result<(), String> {
        let runtime = &context.runtime;
        let installed = runtime.installed_agent_customizer().await;
        let customizer = match self.declared_customizer.as_ref() {
            Some(declared) => {
                let other = |carried: &Arc<dyn AgentCustomizer>| !Arc::ptr_eq(carried, declared);
                if context.customizer.as_ref().is_some_and(other) {
                    return Err(
                        "conflicting agent customizers: the mob's spec declares one \
                         (MobBootstrapSpec::with_agent_customizer) and the identity context \
                         carries another; the context carries the declared customizer or none"
                            .to_string(),
                    );
                }
                if installed.as_ref().is_some_and(other) {
                    return Err(
                        "conflicting agent customizers: the mob's spec declares one \
                         (MobBootstrapSpec::with_agent_customizer) and the identity runtime has \
                         another installed; the runtime installs the declared customizer or none"
                            .to_string(),
                    );
                }
                Arc::clone(declared)
            }
            None => match installed.clone().or_else(|| context.customizer.clone()) {
                Some(customizer) => customizer,
                None => return Ok(()),
            },
        };
        let registry = match (
            runtime.installed_customizer_tool_registry().await,
            self.registry.as_ref(),
        ) {
            (Some(runtime_registry), Some(composed))
                if !Arc::ptr_eq(&runtime_registry, composed) =>
            {
                return Err(
                    "conflicting customizer tool registries: the mob was built with its own \
                     and the identity runtime has another installed; the runtime installs none"
                        .to_string(),
                );
            }
            (Some(runtime_registry), _) => runtime_registry,
            (None, composed) => {
                let registry = composed
                    .cloned()
                    .unwrap_or_else(CustomizerToolRegistry::new);
                runtime
                    .set_customizer_tool_registry(Some(Arc::clone(&registry)))
                    .await;
                registry
            }
        };
        if installed.is_none() {
            runtime.set_agent_customizer(Some(customizer)).await;
        }
        if let Some(customizer) = self.spawn_customizer.as_ref() {
            customizer.arm(registry);
        }
        if let Some(preparer) = self.preparer.as_ref() {
            preparer.bind_publishing(runtime, session_service).await;
        }
        Ok(())
    }
}
