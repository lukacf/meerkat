//! The actual tool surface of one prepared agent construction, and the
//! decision a trusted host makes on it before the construction is finished.
//!
//! A surface is read from the construction's final composed dispatcher; it is
//! never inferred. Its identities carry provenance exactly as constructed.

use std::sync::{Arc, Mutex};

use crate::agent::AgentToolDispatcher;
use crate::tool_catalog::{ToolCallability, ToolCatalogDeferredEligibility, ToolPlaneClass};
use crate::tool_consequence_policy::ApplicationToolPolicyBinding;
use crate::tool_scope::ToolFilter;
use crate::types::ToolIdentity;

/// The actual tool surface of one prepared construction, read from its final
/// composed dispatcher, with provenance exactly as constructed (absent
/// provenance stays absent; an artifact only where a host verified one).
///
/// `tools` is an UPPER BOUND: every tool the composed dispatcher offers, before
/// the session's tool scope applies. `projected` is that bound narrowed by the
/// construction's static visibility filters (the capability base filter, the
/// access-policy base filter and the initial tool filter); a scope restored
/// from the session (an inherited filter, a deferred-catalog visibility
/// state) can only narrow it further, or make a `Complete` catalog entry
/// visible. When the construction is finished, its actual visible tools are
/// checked to lie within this surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedToolSurface {
    /// The composed dispatcher's provider-facing tools: an upper bound.
    pub tools: Vec<ToolIdentity>,
    /// `tools` narrowed by the construction's static visibility filters.
    pub projected: Vec<ToolIdentity>,
    /// Its catalog, complete only when the dispatcher says so.
    pub catalog: PreparedToolCatalog,
}

/// A prepared construction's tool catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedToolCatalog {
    /// The dispatcher reports an exact catalog: every winning entry,
    /// including deferred ones, with its plane and callability.
    Complete(Vec<PreparedCatalogEntry>),
    /// The dispatcher cannot prove an exact catalog: only `tools` is known,
    /// and it is not a complete deferred surface.
    VisibleOnly,
}

/// One winning catalog entry of a prepared construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCatalogEntry {
    pub identity: ToolIdentity,
    pub plane: ToolPlaneClass,
    pub callability: ToolCallability,
    pub deferred_eligible: bool,
}

impl PreparedToolSurface {
    /// The surface of `tools`, a construction's final composed dispatcher,
    /// projected through its static visibility `filters`.
    pub fn of(tools: &dyn AgentToolDispatcher, filters: &[ToolFilter]) -> Self {
        let identities: Vec<ToolIdentity> =
            tools.tools().iter().map(|tool| tool.identity()).collect();
        // The session's own rule for names, so the projection is the list
        // the session would be offered under these filters.
        let composed = crate::tool_scope::ToolScope::compose(filters);
        let projected = identities
            .iter()
            .filter(|identity| composed.allows(identity.name.as_str()))
            .cloned()
            .collect();
        let catalog = if tools.tool_catalog_capabilities().exact_catalog {
            PreparedToolCatalog::Complete(
                tools
                    .tool_catalog()
                    .iter()
                    .map(|entry| PreparedCatalogEntry {
                        identity: entry.tool.identity(),
                        plane: entry.plane,
                        callability: entry.callability,
                        deferred_eligible: matches!(
                            entry.deferred_eligibility,
                            ToolCatalogDeferredEligibility::DeferredEligible { .. }
                        ),
                    })
                    .collect(),
            )
        } else {
            PreparedToolCatalog::VisibleOnly
        };
        Self {
            tools: identities,
            projected,
            catalog,
        }
    }

    /// Whether `identity` is within this surface: one of its tools, or an
    /// entry of its complete catalog.
    pub fn contains(&self, identity: &ToolIdentity) -> bool {
        self.tools.contains(identity)
            || matches!(&self.catalog, PreparedToolCatalog::Complete(entries)
                if entries.iter().any(|entry| &entry.identity == identity))
    }
}

/// A trusted host's selection for a prepared construction, made from its
/// actual surface: the resolved application tool policy the construction is
/// finished under (a `Provider` binding, bound when it is finished with the
/// construction's own member and registry), and the exact surface the host
/// compiled that policy and its installed-artifact associations against. A
/// selection whose `compiled_against` differs from the construction's surface
/// never binds: the construction refuses to finish. The provider and policy
/// ids are references, not a permit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSurfaceSelection {
    pub binding: ApplicationToolPolicyBinding,
    pub compiled_against: PreparedToolSurface,
}

/// Why a trusted host refused a prepared construction's surface. The
/// construction is then abandoned; nothing of it is published.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("tool surface refused: {reason}")]
pub struct ToolSurfaceRefusal {
    pub reason: String,
}

impl ToolSurfaceRefusal {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// A trusted host's decision on a prepared construction's actual surface,
/// made after the construction and before it is finished: its selection, or a
/// refusal. The host may commit what it needs (the selected policy, its
/// installed-artifact facts) to its own owners here; nothing is granted by
/// the surface itself.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait ToolSurfaceGate: Send + Sync {
    async fn decide(
        &self,
        surface: &PreparedToolSurface,
    ) -> Result<ToolSurfaceSelection, ToolSurfaceRefusal>;
}

/// One session registration's tool-surface gate, installed at most once by
/// its trusted host before the session's agent is built. Every registration
/// owns a fresh slot; an empty slot builds the agent in one step as before.
/// When the registration ends its slot is retired, terminally: a retired slot
/// reports no gate to any holder (a binding clone included) and accepts none.
#[derive(Clone, Default)]
pub struct ToolSurfaceGateSlot(Arc<Mutex<GateSlotState>>);

#[derive(Default)]
enum GateSlotState {
    #[default]
    Empty,
    Installed(Arc<dyn ToolSurfaceGate>),
    Retired,
}

impl std::fmt::Debug for ToolSurfaceGateSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolSurfaceGateSlot")
            .field("installed", &self.current().is_some())
            .finish()
    }
}

impl ToolSurfaceGateSlot {
    fn state(&self) -> std::sync::MutexGuard<'_, GateSlotState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Install this registration's gate.
    ///
    /// # Errors
    /// A gate is already installed (a registration has one gate), or the
    /// registration has ended.
    pub fn install(&self, gate: Arc<dyn ToolSurfaceGate>) -> Result<(), ToolSurfaceRefusal> {
        let mut state = self.state();
        match *state {
            GateSlotState::Empty => {
                *state = GateSlotState::Installed(gate);
                Ok(())
            }
            GateSlotState::Installed(_) => Err(ToolSurfaceRefusal::new(
                "the session registration already has a tool-surface gate",
            )),
            GateSlotState::Retired => Err(ToolSurfaceRefusal::new(
                "the session registration has ended",
            )),
        }
    }

    /// The installed gate of a current registration, if any. The returned
    /// gate stays bound to this slot: once the registration ends it refuses
    /// every decision, including one already under way, so a gate exported
    /// before retirement never selects for an ended registration.
    pub fn current(&self) -> Option<Arc<dyn ToolSurfaceGate>> {
        match &*self.state() {
            GateSlotState::Installed(gate) => Some(Arc::new(SlotBoundGate {
                slot: self.clone(),
                gate: Arc::clone(gate),
            })),
            GateSlotState::Empty | GateSlotState::Retired => None,
        }
    }

    /// The registration ended: retire its slot for good.
    pub fn retire(&self) {
        *self.state() = GateSlotState::Retired;
    }

    fn retired(&self) -> bool {
        matches!(*self.state(), GateSlotState::Retired)
    }
}

/// A slot's installed gate as handed out by [`ToolSurfaceGateSlot::current`].
struct SlotBoundGate {
    slot: ToolSurfaceGateSlot,
    gate: Arc<dyn ToolSurfaceGate>,
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl ToolSurfaceGate for SlotBoundGate {
    async fn decide(
        &self,
        surface: &PreparedToolSurface,
    ) -> Result<ToolSurfaceSelection, ToolSurfaceRefusal> {
        let ended = || ToolSurfaceRefusal::new("the session registration has ended");
        if self.slot.retired() {
            return Err(ended());
        }
        let selection = self.gate.decide(surface).await?;
        if self.slot.retired() {
            return Err(ended());
        }
        Ok(selection)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    struct Refuses;

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl ToolSurfaceGate for Refuses {
        async fn decide(
            &self,
            _surface: &PreparedToolSurface,
        ) -> Result<ToolSurfaceSelection, ToolSurfaceRefusal> {
            Err(ToolSurfaceRefusal::new("refuses"))
        }
    }

    #[test]
    fn a_slot_takes_one_gate_and_retires_for_every_clone() {
        let slot = ToolSurfaceGateSlot::default();
        let clone = slot.clone();
        assert!(slot.current().is_none());
        slot.install(Arc::new(Refuses)).unwrap();
        assert!(clone.current().is_some(), "clones share the slot");
        assert!(slot.install(Arc::new(Refuses)).is_err(), "one gate");
        clone.retire();
        assert!(slot.current().is_none(), "retired for every holder");
        assert!(slot.install(Arc::new(Refuses)).is_err(), "terminal");
    }

    /// Selects, but ends its registration while deciding.
    struct RetiresWhileDeciding(ToolSurfaceGateSlot);

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl ToolSurfaceGate for RetiresWhileDeciding {
        async fn decide(
            &self,
            surface: &PreparedToolSurface,
        ) -> Result<ToolSurfaceSelection, ToolSurfaceRefusal> {
            self.0.retire();
            Ok(ToolSurfaceSelection {
                binding: ApplicationToolPolicyBinding::Unmanaged,
                compiled_against: surface.clone(),
            })
        }
    }

    #[tokio::test]
    async fn an_exported_gate_refuses_once_its_registration_ends() {
        let surface = PreparedToolSurface {
            tools: Vec::new(),
            projected: Vec::new(),
            catalog: PreparedToolCatalog::VisibleOnly,
        };
        let slot = ToolSurfaceGateSlot::default();
        slot.install(Arc::new(RetiresWhileDeciding(slot.clone())))
            .unwrap();
        let during = slot.current().unwrap();
        assert!(
            during.decide(&surface).await.is_err(),
            "a selection made as the registration ended does not stand"
        );

        let slot = ToolSurfaceGateSlot::default();
        slot.install(Arc::new(Refuses)).unwrap();
        let exported = slot.current().unwrap();
        slot.retire();
        let refusal = exported.decide(&surface).await.unwrap_err();
        assert_eq!(refusal.reason, "the session registration has ended");
    }

    #[test]
    fn the_projection_narrows_by_every_static_filter() {
        struct Two;
        #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
        impl AgentToolDispatcher for Two {
            fn tools(&self) -> Arc<[Arc<crate::types::ToolDef>]> {
                vec![
                    Arc::new(crate::types::ToolDef::new("a", "", serde_json::json!({}))),
                    Arc::new(crate::types::ToolDef::new("b", "", serde_json::json!({}))),
                ]
                .into()
            }

            async fn dispatch(
                &self,
                _call: crate::types::ToolCallView<'_>,
            ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
                unreachable!("never dispatched")
            }
        }
        let surface = PreparedToolSurface::of(
            &Two,
            &[ToolFilter::Deny(
                [crate::types::ToolName::from("b")].into_iter().collect(),
            )],
        );
        assert_eq!(surface.tools.len(), 2, "the upper bound keeps both");
        assert_eq!(surface.projected.len(), 1);
        assert_eq!(surface.projected[0].name.as_str(), "a");
        assert_eq!(surface.catalog, PreparedToolCatalog::VisibleOnly);
    }

    /// The projection follows the session's own rule for names: a filter
    /// that closes `shell` leaves no `monitor_start` in it, as in the
    /// session's real list.
    #[test]
    fn the_projection_leaves_out_a_monitor_start_whose_shell_is_closed() {
        struct ShellFamily;
        #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
        impl AgentToolDispatcher for ShellFamily {
            fn tools(&self) -> Arc<[Arc<crate::types::ToolDef>]> {
                ["shell", "shell_jobs", "monitor_start"]
                    .into_iter()
                    .map(|name| {
                        Arc::new(crate::types::ToolDef::new(name, "", serde_json::json!({})))
                    })
                    .collect()
            }

            async fn dispatch(
                &self,
                _call: crate::types::ToolCallView<'_>,
            ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
                unreachable!("never dispatched")
            }
        }
        let projected = |filter: ToolFilter| {
            PreparedToolSurface::of(&ShellFamily, &[filter])
                .projected
                .iter()
                .map(|identity| identity.name.to_string())
                .collect::<Vec<_>>()
        };
        let names = |names: &[&str]| {
            names
                .iter()
                .map(|name| crate::types::ToolName::from(*name))
                .collect()
        };
        assert_eq!(
            projected(ToolFilter::Deny(names(&["shell"]))),
            ["shell_jobs"],
            "a deny of shell closes the monitor"
        );
        assert_eq!(
            projected(ToolFilter::Allow(names(&["monitor_start", "shell_jobs"]))),
            ["shell_jobs"],
            "an allow list without shell does not admit the monitor"
        );
        assert_eq!(
            projected(ToolFilter::All),
            ["shell", "shell_jobs", "monitor_start"]
        );
    }
}
