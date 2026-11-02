//! Host selection of a member's scoped destination at the exact native
//! registration boundary.
//!
//! A configured host only selects. The provisioner installs the selected
//! destination itself, on the machine that minted the member's exact
//! registration, so a host can neither skip the install nor bind another
//! runtime owner's session. The host never receives the machine.

use crate::{MemberRegistrationHookCall, MobError};
use meerkat_authorization_contracts::constraints::AudienceRef;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::NativeWorkTarget;
use meerkat_core::auth::PrincipalRef;
use meerkat_core::{MobMemberBinding, RealmId};
use meerkat_runtime::meerkat_machine::SessionAgentMandate;
use meerkat_runtime::{LogicalRuntimeId, MeerkatMachine, RuntimeSessionRegistrationWitness};
use std::collections::HashMap;
use std::sync::Arc;

/// The hook call that refused one member's registration, recorded by that
/// call itself when the host returns an error. A host may return any
/// [`MobError`], so a refusal is never read back from an error's type or
/// text. The first refusal is kept.
#[derive(Clone, Default)]
pub(super) struct HostRefusalSlot(Arc<std::sync::Mutex<Option<MemberRegistrationHookCall>>>);

impl HostRefusalSlot {
    fn record(&self, call: MemberRegistrationHookCall) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert(call);
    }

    /// The call that refused, if a host refused.
    pub(super) fn refused(&self) -> Option<MemberRegistrationHookCall> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The destination a host selects for one member: everything of the
/// member's scoped target except its runtime, which only the member's exact
/// registration names. A host can therefore never select another runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberDestinationSelection {
    pub logical_owner: PrincipalRef,
    pub context: EvidenceId,
    /// Must advance past any generation already bound on the member's
    /// registration (see [`MemberRuntimeRegistrationHook::select_destination`]).
    pub context_generation: u64,
    pub audience: AudienceRef,
}

impl MemberDestinationSelection {
    /// The full scoped target for the session `registration` registered. The
    /// context must be that session's own document: a host cannot relabel a
    /// member's history under another context.
    fn target_for(
        &self,
        registration: &RuntimeSessionRegistrationWitness,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<NativeWorkTarget, MobError> {
        if self.context != identity.session_document {
            return Err(MobError::WiringError(
                "the selected context is not the member's own session document".into(),
            ));
        }
        let runtime = LogicalRuntimeId::for_session(registration.session_id());
        let logical_runtime = EvidenceId::new(runtime.to_string()).map_err(|error| {
            MobError::Internal(format!("member runtime is not a valid target id: {error}"))
        })?;
        Ok(NativeWorkTarget {
            logical_owner: self.logical_owner.clone(),
            logical_runtime,
            context: self.context.clone(),
            context_generation: self.context_generation,
            audience: self.audience.clone(),
        })
    }
}

/// The canonical context of a registered member: its own session document.
fn session_document(
    registration: &RuntimeSessionRegistrationWitness,
) -> Result<EvidenceId, MobError> {
    EvidenceId::new(registration.session_id().to_string()).map_err(|error| {
        MobError::Internal(format!("member session is not a valid context id: {error}"))
    })
}

/// Immutable native build identity of the receiving member. Application
/// labels, prompts and session contents are deliberately absent: hosts
/// choose their destination from these typed facts only.
#[derive(Clone, Debug)]
pub struct MemberRuntimeBuildIdentity {
    realm_id: RealmId,
    member: MobMemberBinding,
    session_document: EvidenceId,
    registration: RuntimeSessionRegistrationWitness,
}

impl MemberRuntimeBuildIdentity {
    pub fn realm_id(&self) -> &RealmId {
        &self.realm_id
    }

    pub fn member(&self) -> &MobMemberBinding {
        &self.member
    }

    /// The member's own session document, the only context a selection may
    /// name.
    pub fn session_document(&self) -> &EvidenceId {
        &self.session_document
    }

    /// The member's exact runtime registration this call is made for: the
    /// one being prepared for a new member, or the current one of a member
    /// whose actor is recovered. A later registration of the same session is
    /// a different registration, so a host binds what it decides here to
    /// this registration, not to an identity it derives from the member's
    /// session id or labels.
    pub fn registration(&self) -> &RuntimeSessionRegistrationWitness {
        &self.registration
    }

    pub(super) fn from_request(
        req: &meerkat_core::service::CreateSessionRequest,
        registration: &RuntimeSessionRegistrationWitness,
    ) -> Result<Self, MobError> {
        let build = req.build.as_ref().ok_or_else(|| {
            MobError::Internal("registration hook requires native member build identity".into())
        })?;
        let member = build.mob_member_binding.clone().ok_or_else(|| {
            MobError::Internal("registration hook requires native MobMemberBinding".into())
        })?;
        let realm_id = build.realm_id.clone().ok_or_else(|| {
            MobError::Internal("registration hook requires native member realm".into())
        })?;
        let expected_realm = meerkat_core::mob_realm_id(&member.mob_id)
            .map_err(|error| MobError::Internal(error.to_string()))?;
        let comms_name = member
            .comms_name()
            .map_err(|error| MobError::Internal(error.to_string()))?;
        if realm_id != expected_realm
            || build.comms_name.as_deref() != Some(comms_name.to_string().as_str())
        {
            return Err(MobError::Internal(
                "registration hook member identity disagrees with native realm or comms binding"
                    .into(),
            ));
        }
        Ok(Self {
            realm_id,
            member,
            session_document: session_document(registration)?,
            registration: registration.clone(),
        })
    }
}

/// Optional host composition, never queue admission or permission. Both
/// methods are required: a configured host must fail closed when it cannot
/// select. Absence preserves the existing unscoped runtime behavior.
///
/// Every method runs on the runtime of the task that asks for the member
/// (the mob actor's, which is the host's), never on Core's private
/// process-wide runtimes: the provisioner keeps the member's transaction in
/// its own owned task and awaits each call as a task on the host's runtime.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait MemberRuntimeRegistrationHook: Send + Sync {
    /// Select the destination a newly registered member's session document is
    /// bound to. Called while the member's prepared registration transaction
    /// owns cleanup, before actor creation; a refusal rolls that transaction
    /// back exactly. The provisioner installs the returned target.
    ///
    /// For a restored member whose registration this transaction did not
    /// insert, a refusal removes that registration exactly through the
    /// canonical unregister: the session's durable history records it, every
    /// input the session still holds is abandoned as Stopped, and their
    /// completions resolve as runtime-terminated. Any other failure keeps the
    /// restored registration for an explicit repair to reuse. A member that
    /// `MobBuilder::resume` restores at boot is refused the same way: the
    /// boot restore's own rollback removes the registration it inserted, and
    /// the actor's startup registers nothing for the refused member. A member
    /// refused either way can still be retired, respawned or destroyed: its
    /// retirement archives the durable session without routing a retire to
    /// the runtime the refusal released. A member the boot recreates on a
    /// fresh session (as it does when the session service keeps no durable
    /// sessions) is refused the same way: the transaction's own rollback
    /// removes the registration it inserted, and only that member stays
    /// Broken with the typed refusal while the rest of the mob comes up. A
    /// spawn that adopts a session registered before it (a resumed or
    /// pre-registered session) keeps that registration on a refusal; only
    /// the transaction is rolled back.
    ///
    /// A failed revival's destination stays bound on the restored
    /// registration it keeps, so a host's re-selection for that registration
    /// must advance `context_generation`: a selection of the bound generation
    /// is refused before anything is built, with
    /// [`MobError::MemberDestinationNotInstalled`] carrying
    /// `RuntimeDriverError::ScopedContextGenerationNotAdvanced`, and the
    /// registration stays for the repair, which selects again with an
    /// advanced `context_generation`. The recovery of an attached
    /// member's actor (its registration still attached) asks
    /// [`Self::expected_destination`] instead, and a refusal during that
    /// recovery keeps the registration and answers the host's error.
    async fn select_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError>;

    /// The destination the host still expects for an already attached member.
    /// Read-only: never a cold setter and never physical work. The machine
    /// checks it against the member's current binding inside the same
    /// preparation that recovers the member's actor.
    async fn expected_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError>;

    /// Select the agent a NEW member's registration acts as: its qualified
    /// executor, the represented subject and the exact grant lineage selected
    /// for each configured server, from the host's own trusted installation
    /// facts (the identity's labels authorize nothing). Called after the
    /// member's destination is installed and before its agent or any of its
    /// tools is built; the provisioner binds the returned mandate on the
    /// member's exact registration. A refusal rolls the registration back
    /// exactly. A restored member's refusal removes its registration the same
    /// way (see [`Self::select_destination`]). The default selects none: the
    /// registration stays unbound, so its configured servers are refused as
    /// unavailable.
    async fn select_agent_mandate(
        &self,
        _identity: &MemberRuntimeBuildIdentity,
    ) -> Result<Option<SessionAgentMandate>, MobError> {
        Ok(None)
    }

    /// The agent mandate the host still expects an already attached member's
    /// registration to be bound to (`None`: unbound). Read-only: recovery
    /// never binds or rebinds the agent. The machine checks it against the
    /// registration's binding inside the same preparation that recovers the
    /// member's actor; a mismatch refuses the recovery and leaves the binding
    /// as it is (a surface gate the recovery installed before the check
    /// stays). The default expects none.
    async fn expected_agent_mandate(
        &self,
        _identity: &MemberRuntimeBuildIdentity,
    ) -> Result<Option<SessionAgentMandate>, MobError> {
        Ok(None)
    }

    /// Check the member's ACTUAL visible tool surface, as its built session
    /// owns it, before the member is published (a new member before its
    /// first turn; a recovered actor before it is committed). Tool
    /// provenance is attribution only: the host compares this surface with
    /// its own committed activation, which owns the association between an
    /// installed artifact and its tools. A refusal discards the member
    /// exactly; effects its construction already started are not undone. A
    /// restored member's refusal removes its registration the same way (see
    /// [`Self::select_destination`]), except during the recovery of an
    /// attached member's actor, which keeps the registration and answers the
    /// host's error.
    async fn verify_member_surface(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &[meerkat_core::ToolDef],
    ) -> Result<(), MobError>;

    /// Select the policy a NEW member's construction is finished under, from
    /// its prepared tool surface: the actual winning tool identities of its
    /// one construction, after it is composed and before it is finished. The
    /// host compiles and commits what it needs (its application tool policy,
    /// its installed-artifact associations) to its own owners here and
    /// returns the resolved `Provider` binding with the exact surface it
    /// compiled against; the construction binds it, with its own member and
    /// registry, when it is finished. A refusal abandons the construction and
    /// discards the member exactly. A restored member's refusal removes its
    /// registration the same way (see [`Self::select_destination`]), except
    /// during the recovery of an attached member's actor, which keeps the
    /// registration and answers the host's error. A host that selects nothing
    /// cannot complete a member it registers: the default refuses.
    async fn prepare_member_surface(
        &self,
        _identity: &MemberRuntimeBuildIdentity,
        _surface: &meerkat_core::PreparedToolSurface,
    ) -> Result<meerkat_core::ToolSurfaceSelection, MobError> {
        Err(MobError::WiringError(
            "the member's host selected no application tool policy for its prepared surface".into(),
        ))
    }
}

/// Where a member's provision records the selection its host made for the
/// member's prepared surface. The construction is finished only under that
/// selection, so a recorded selection of a finished member is the one its
/// agent was finished under.
#[derive(Clone, Default)]
pub(super) struct MemberSurfaceSelectionSlot(
    Arc<std::sync::Mutex<Option<meerkat_core::ToolSurfaceSelection>>>,
);

impl MemberSurfaceSelectionSlot {
    fn record(&self, selection: &meerkat_core::ToolSurfaceSelection) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(selection.clone());
    }

    pub(super) fn selection(&self) -> Option<meerkat_core::ToolSurfaceSelection> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// The slots one transaction's surface decision is recorded into.
#[derive(Clone, Default)]
struct MemberSurfaceGateSlots {
    selected: Option<MemberSurfaceSelectionSlot>,
    refusals: Option<HostRefusalSlot>,
}

/// Where a member registration's surface gate records its host's decision:
/// the slots of the transaction that prepares the registration now. A
/// registration keeps the gate it was given, so a later transaction on it
/// points the route at its own slots before its build asks the gate (see
/// [`install_member_surface_gate`]). The transactions of one registration
/// are serialized by its turn-finalization boundary.
struct MemberSurfaceGateRoute(std::sync::Mutex<MemberSurfaceGateSlots>);

impl MemberSurfaceGateRoute {
    fn slots(&self) -> MemberSurfaceGateSlots {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn point_at(&self, slots: MemberSurfaceGateSlots) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = slots;
    }
}

/// An installed gate's route, with the exact registration it was installed
/// on.
struct InstalledGateRoute {
    registration: RuntimeSessionRegistrationWitness,
    route: std::sync::Weak<MemberSurfaceGateRoute>,
}

/// The routes of the surface gates one owner installed, by session. Every
/// clone of the owner shares them; a route lives no longer than its gate.
#[derive(Clone, Default)]
pub(super) struct MemberSurfaceGateRoutes(
    Arc<std::sync::Mutex<HashMap<meerkat_core::types::SessionId, InstalledGateRoute>>>,
);

impl MemberSurfaceGateRoutes {
    fn routes(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<meerkat_core::types::SessionId, InstalledGateRoute>>
    {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The route of the gate this owner installed on exactly
    /// `registration`, while that gate lives.
    fn route(
        &self,
        registration: &RuntimeSessionRegistrationWitness,
    ) -> Option<Arc<MemberSurfaceGateRoute>> {
        self.routes()
            .get(registration.session_id())
            .filter(|installed| &installed.registration == registration)
            .and_then(|installed| installed.route.upgrade())
    }

    fn record(
        &self,
        registration: &RuntimeSessionRegistrationWitness,
        route: &Arc<MemberSurfaceGateRoute>,
    ) {
        let mut routes = self.routes();
        routes.retain(|_, installed| installed.route.strong_count() > 0);
        routes.insert(
            registration.session_id().clone(),
            InstalledGateRoute {
                registration: registration.clone(),
                route: Arc::downgrade(route),
            },
        );
    }
}

/// A new member registration's tool-surface gate: its host's
/// [`MemberRuntimeRegistrationHook::prepare_member_surface`] for that member,
/// recorded into the slots its route points at.
struct MemberSurfaceGate {
    hook: Arc<dyn MemberRuntimeRegistrationHook>,
    identity: MemberRuntimeBuildIdentity,
    route: Arc<MemberSurfaceGateRoute>,
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl meerkat_core::ToolSurfaceGate for MemberSurfaceGate {
    async fn decide(
        &self,
        surface: &meerkat_core::PreparedToolSurface,
    ) -> Result<meerkat_core::ToolSurfaceSelection, meerkat_core::ToolSurfaceRefusal> {
        // The transaction whose build asks: the one the route points at.
        let slots = self.route.slots();
        let selection = self
            .hook
            .prepare_member_surface(&self.identity, surface)
            .await
            .map_err(|error| {
                if let Some(refusals) = &slots.refusals {
                    refusals.record(MemberRegistrationHookCall::PrepareMemberSurface);
                }
                meerkat_core::ToolSurfaceRefusal::new(error.to_string())
            })?;
        if let Some(selected) = &slots.selected {
            selected.record(&selection);
        }
        Ok(selection)
    }
}

/// Install the host's surface selection on the member's exact registration,
/// before its agent is built: a new member's, or a recovered member's whose
/// registration carries none yet (a registration keeps the gate it was
/// given; a replaced registration starts without one). The gate records into
/// this transaction's `selected` and `refusals`: a gate this owner installed
/// for an earlier transaction on the same registration (`routes`) is pointed
/// at them, so its decision reaches the transaction that asks. A gate another
/// owner installed records into that owner's slots.
pub(super) async fn install_member_surface_gate(
    machine: &MeerkatMachine,
    hook: Arc<dyn MemberRuntimeRegistrationHook>,
    registration: &RuntimeSessionRegistrationWitness,
    identity: &MemberRuntimeBuildIdentity,
    routes: &MemberSurfaceGateRoutes,
    selected: Option<MemberSurfaceSelectionSlot>,
    refusals: Option<HostRefusalSlot>,
) -> Result<(), MobError> {
    let slots = MemberSurfaceGateSlots { selected, refusals };
    if machine
        .session_tool_surface_gate(registration.session_id())
        .await
        .is_some()
    {
        if let Some(route) = routes.route(registration) {
            route.point_at(slots);
        }
        return Ok(());
    }
    let route = Arc::new(MemberSurfaceGateRoute(std::sync::Mutex::new(slots)));
    machine
        .bind_session_tool_surface_gate(
            registration,
            Arc::new(MemberSurfaceGate {
                hook,
                identity: identity.clone(),
                route: Arc::clone(&route),
            }),
        )
        .await
        .map_err(|error| {
            MobError::WiringError(format!(
                "member tool-surface decision could not be installed on its registration: {error}"
            ))
        })?;
    routes.record(registration, &route);
    Ok(())
}

/// Bind the host's selected agent mandate on the member's exact registration,
/// before its agent is built. Nothing is bound when the host selects none.
/// The host's hook is called on `host`, its runtime.
pub(super) async fn install_selected_agent_mandate(
    host: &super::provisioner::MemberHostRuntime,
    machine: &MeerkatMachine,
    hook: &Arc<dyn MemberRuntimeRegistrationHook>,
    registration: &RuntimeSessionRegistrationWitness,
    identity: &MemberRuntimeBuildIdentity,
    refusals: &HostRefusalSlot,
) -> Result<(), MobError> {
    let selected = host
        .run_hook({
            let hook = Arc::clone(hook);
            let identity = identity.clone();
            async move { hook.select_agent_mandate(&identity).await }
        })
        .await?;
    let Some(mandate) = selected
        .inspect_err(|_| refusals.record(MemberRegistrationHookCall::SelectAgentMandate))?
    else {
        return Ok(());
    };
    machine
        .bind_session_agent_mandate(registration, mandate)
        .await
        .map_err(|error| {
            MobError::WiringError(format!(
                "member agent mandate could not be bound on its registration: {error}"
            ))
        })
}

/// The runtime owner and host selected for one backend, with the routes of
/// the surface gates that backend installed; present only when the owner and
/// the host are configured.
pub(super) type MemberRegistrationComposition = Option<(
    Arc<MeerkatMachine>,
    Arc<dyn MemberRuntimeRegistrationHook>,
    MemberSurfaceGateRoutes,
)>;

/// Select and install a newly registered member's destination on the exact
/// registration's own machine. The host's hook is called on `host`, its
/// runtime.
pub(super) async fn install_selected_destination(
    host: &super::provisioner::MemberHostRuntime,
    machine: &MeerkatMachine,
    hook: &Arc<dyn MemberRuntimeRegistrationHook>,
    registration: &RuntimeSessionRegistrationWitness,
    req: &meerkat_core::service::CreateSessionRequest,
    refusals: &HostRefusalSlot,
) -> Result<MemberRuntimeBuildIdentity, MobError> {
    require_deferred_first_turn(req)?;
    let identity = MemberRuntimeBuildIdentity::from_request(req, registration)?;
    let selected = host
        .run_hook({
            let hook = Arc::clone(hook);
            let identity = identity.clone();
            async move { hook.select_destination(&identity).await }
        })
        .await?;
    let target = selected
        .inspect_err(|_| refusals.record(MemberRegistrationHookCall::SelectDestination))?
        .target_for(registration, &identity)?;
    machine
        .bind_scoped_context(registration, &target)
        .await
        .map_err(|error| MobError::MemberDestinationNotInstalled {
            session_id: registration.session_id().clone(),
            error,
        })?;
    Ok(identity)
}

/// The member's surface is verified before its first turn, so a hooked
/// member, created or recovered, is built on the deferred route only; an
/// eager request is refused before the host is asked.
fn require_deferred_first_turn(
    req: &meerkat_core::service::CreateSessionRequest,
) -> Result<(), MobError> {
    if req.initial_turn == meerkat_core::service::InitialTurnPolicy::RunImmediately {
        return Err(MobError::WiringError(
            "a member registered through a host hook is created with a deferred first turn".into(),
        ));
    }
    Ok(())
}

/// Hand the built member's actual visible tool surface to the host before
/// the member is published. The host's hook is called on `host`, its runtime.
pub(super) async fn verify_member_surface(
    host: &super::provisioner::MemberHostRuntime,
    hook: &Arc<dyn MemberRuntimeRegistrationHook>,
    session_service: &dyn super::MobSessionService,
    session_id: &meerkat_core::types::SessionId,
    identity: &MemberRuntimeBuildIdentity,
    refusals: Option<&HostRefusalSlot>,
) -> Result<(), MobError> {
    let surface = session_service
        .live_visible_tool_defs(session_id)
        .await
        .map_err(MobError::SessionError)?
        .ok_or_else(|| {
            MobError::WiringError(
                "the member's session service cannot report its actual tool surface".into(),
            )
        })?;
    host.run_hook({
        let hook = Arc::clone(hook);
        let identity = identity.clone();
        async move { hook.verify_member_surface(&identity, &surface).await }
    })
    .await?
    .inspect_err(|_| {
        if let Some(refusals) = refusals {
            refusals.record(MemberRegistrationHookCall::VerifyMemberSurface);
        }
    })
}

/// The destination the host still expects for an attached member, as the
/// full target its current registration must be bound to.
pub(super) async fn expected_target(
    hook: &dyn MemberRuntimeRegistrationHook,
    registration: &RuntimeSessionRegistrationWitness,
    req: &meerkat_core::service::CreateSessionRequest,
) -> Result<(NativeWorkTarget, MemberRuntimeBuildIdentity), MobError> {
    require_deferred_first_turn(req)?;
    let identity = MemberRuntimeBuildIdentity::from_request(req, registration)?;
    let target = hook
        .expected_destination(&identity)
        .await?
        .target_for(registration, &identity)?;
    Ok((target, identity))
}
