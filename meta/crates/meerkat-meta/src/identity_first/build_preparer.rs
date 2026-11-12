//! Meta's preparation of the member builds Core performs.
//!
//! Core builds a member when it materializes it: on demand (a host delivery,
//! a peer message, a kickoff, an operator warm) and at boot when the member
//! owes work. Before each build Core asks the mob's
//! [`MemberBuildPreparer`], off its actor and before the member's provision,
//! and once the member is live it reports the build's visible tools. Meta's
//! answer is [`IdentityBuildPreparer`]:
//!
//! - a member of a registered roster identity whose host composes an agent
//!   customizer is prepared with the host's `customize_build`, run with the
//!   identity runtime's real context; its tools are published into the
//!   identity's stable customizer tool dispatcher, so the member is built
//!   with them. A member of an identity whose record is its row in the
//!   mob's identity store that Core builds (its first build for its
//!   kickoff, a wake, a restore) is registered from that row, whether or
//!   not a roster pass or a door registered it yet, and is prepared the
//!   same way: Meta's spawn customizer attached the stable dispatcher to
//!   the build, because Core holds the member id as an identity entry;
//! - the seat of an identity whose record is its row is composed from the
//!   host's build step at every build of its member
//!   (`MemberBuildPreparer::prepare_store_owned_seat`): Core seats such an
//!   identity from its row, so the customizer's draft is lowered onto the
//!   seat Core is about to build, through the lowering a configured
//!   identity's seat is composed with, and Core takes the model and the
//!   provider parameters of it at every build and the context, the
//!   instructions and the system prompt at the first build of the member's
//!   session. The customizer runs once for the build: the preparation that
//!   follows in the same build answers what the composition found. Before
//!   the identity runtime is composed there is no host step to run: a fresh
//!   spawn is left as Core lowered it (a refusal would leave the identity
//!   owed its seat until the mob's next start, which asks in the same
//!   state), and a wake or a restore is refused retryable, so the member
//!   stays Dormant and is asked again;
//! - before the identity runtime is composed (its roster registered, its
//!   customizer and runtime services installed), an identity's member is
//!   answered `HoldUntilReleased`: Core builds it and holds its run starts,
//!   and the composition publishes its tools and releases the hold
//!   (`IdentityRuntime::compose_member_builds`). A member of an identity a
//!   host adopted is never held: no roster publication covers it;
//! - every build Core reports is written to the identity's durable tool
//!   record, which `meta/identity/resolved_tools` answers for a member Core
//!   holds Dormant.
//!
//! A member that is not a Meta identity's (a helper, a fork, a member the
//! host made on the mob under its own id) is prepared with nothing to
//! publish. The preparation reads through the request's read handle, and a
//! created identity's row through the runtime's session bridge (the machine
//! state the mob's actor published, and the identity store): it runs inside
//! Core's wake and inside the boot restore, so it never issues a mob command
//! and never waits on an identity's lifecycle lock.
//!
//! A runtime composed with `UnifiedRuntime::bootstrap` over a spec that
//! declares its agent customizer (`MobBootstrapSpec::with_agent_customizer`)
//! gets this preparer as the builder's runtime does. Without a declaration
//! it learns whether it composes a customizer only at its identity
//! bootstrap, so its mob is built with a deferred preparer: inert (Core
//! builds as if the mob had none) until that bootstrap finds a customizer,
//! and inert for good when it finds none.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Weak};

use meerkat_mob::{
    AgentIdentity as MobAgentIdentity, MemberBuildPreparation, MemberBuildPreparationRefusal,
    MemberBuildPrepared, MemberBuildPreparer, MemberBuildReport, MobReadHandle, MobSessionService,
    SpawnMemberSpec,
};

use super::runtime::IdentityRuntime;
use super::types::AgentIdentity;

/// Why an identity's customizer tools wait while the identity runtime is
/// not composed yet.
pub(crate) const PUBLISHED_ONCE_COMPOSED: &str =
    "customizer tools are published once the identity runtime is composed";

/// Why the seat of an identity whose record is its row is not composed for
/// a wake or a restore while the identity runtime is not composed yet.
const SEAT_COMPOSED_ONCE_COMPOSED: &str = "the member's seat is composed from the host's build step once the identity runtime is \
     composed";

/// Meta's preparer of member builds: see the module documentation.
///
/// Created before the mob is built, so it can be installed on it, and bound
/// to the identity runtime once that exists (`bind`).
pub struct IdentityBuildPreparer {
    /// Whether builds are prepared with customizer tools: the composition
    /// has an agent customizer. Unset while a deferred preparer's
    /// composition has not decided: the preparer is then inert.
    publishes_tools: std::sync::OnceLock<bool>,
    /// The identity runtime, once bound. Weak: the runtime owns this
    /// preparer through its composition, never the other way round.
    runtime: std::sync::OnceLock<Weak<IdentityRuntime>>,
    /// The mob's session service, for the live tool read that records a
    /// held member's tools once the composition published them.
    session_service: std::sync::OnceLock<Arc<dyn MobSessionService>>,
    state: std::sync::Mutex<PreparerState>,
}

#[derive(Default)]
struct PreparerState {
    /// The identity runtime declared itself composed: builds of its
    /// registered identities are prepared inline from then on.
    composed: bool,
    /// The members whose build in progress this preparer answered
    /// `HoldUntilReleased`: Core stages the hold with the build, so the
    /// build's report publishes it when the runtime was composed meanwhile
    /// (a composition that read Core's held members before the hold was
    /// staged did not see it).
    held_builds: BTreeSet<MobAgentIdentity>,
    /// Builds reported before the runtime was bound: the identity, the
    /// session and the visible tools of each.
    reports_unbound: Vec<(AgentIdentity, meerkat_core::types::SessionId, Vec<String>)>,
    /// The identity each member being built belongs to, from its
    /// preparation, for the report of that build.
    building: BTreeMap<MobAgentIdentity, AgentIdentity>,
    /// The members whose seat this preparer composed for the build in
    /// progress (`prepare_store_owned_seat`), each with what that build's
    /// preparation answers: the host's customizer ran for the build
    /// already, so the preparation that follows takes the answer and does
    /// not run it again. The next composition of the member's seat replaces
    /// it, and its build's report clears it.
    seat_composed: BTreeMap<MobAgentIdentity, MemberBuildPrepared>,
}

impl IdentityBuildPreparer {
    /// A preparer for a composition that composes an agent customizer
    /// (`publishes_tools`) or not. Without one, builds are prepared with
    /// nothing to publish, and their reports are still recorded.
    pub fn new(publishes_tools: bool) -> Arc<Self> {
        Self::with_publishes_tools(std::sync::OnceLock::from(publishes_tools))
    }

    /// A preparer for a composition that learns whether it composes an
    /// agent customizer only at its identity bootstrap
    /// ([`Self::bind_publishing`]). Until then it is inert: Core builds as
    /// if the mob had no preparer, and no build is held or recorded. A
    /// composition that has no customizer leaves it inert.
    pub(crate) fn deferred() -> Arc<Self> {
        Self::with_publishes_tools(std::sync::OnceLock::new())
    }

    fn with_publishes_tools(publishes_tools: std::sync::OnceLock<bool>) -> Arc<Self> {
        Arc::new(Self {
            publishes_tools,
            runtime: std::sync::OnceLock::new(),
            session_service: std::sync::OnceLock::new(),
            state: std::sync::Mutex::new(PreparerState::default()),
        })
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, PreparerState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn publishes_tools(&self) -> bool {
        self.publishes_tools.get().copied().unwrap_or(false)
    }

    /// A deferred preparer whose composition has not decided yet.
    fn inert(&self) -> bool {
        self.publishes_tools.get().is_none()
    }

    /// Decide a deferred preparer for a composition that composes an agent
    /// customizer, and bind it to `runtime` ([`Self::bind`]). A preparer
    /// created decided keeps its decision.
    pub(crate) async fn bind_publishing(
        self: &Arc<Self>,
        runtime: &Arc<IdentityRuntime>,
        session_service: Option<Arc<dyn MobSessionService>>,
    ) {
        let _ = self.publishes_tools.set(true);
        self.bind(runtime, session_service).await;
    }

    /// Bind this preparer to `runtime`, once its customizer, customizer tool
    /// registry and identity records are installed: the builds reported
    /// before are written to their tool records. The builds held before are
    /// Core's to name: the composition reads which members Core holds. A
    /// second bind is ignored.
    pub(crate) async fn bind(
        self: &Arc<Self>,
        runtime: &Arc<IdentityRuntime>,
        session_service: Option<Arc<dyn MobSessionService>>,
    ) {
        if let Some(service) = session_service {
            let _ = self.session_service.set(service);
        }
        let reports = {
            let mut state = self.lock_state();
            if self.runtime.set(Arc::downgrade(runtime)).is_err() {
                return;
            }
            runtime.set_member_build_preparer(Arc::clone(self));
            std::mem::take(&mut state.reports_unbound)
        };
        for (identity, session_id, visible_tools) in reports {
            runtime
                .record_built_tools(&identity, &session_id, visible_tools)
                .await;
        }
    }

    /// The identity runtime this preparer is bound to, while it lives.
    /// `Err(())` once it is gone.
    fn bound_runtime(&self) -> Option<Result<Arc<IdentityRuntime>, ()>> {
        self.runtime
            .get()
            .map(|runtime| runtime.upgrade().ok_or(()))
    }

    /// Mark the bound runtime composed: from now on a registered identity's
    /// build is prepared inline. A build held before is published by the
    /// composition (Core names it) or by its report.
    pub(crate) fn compose(&self) {
        self.lock_state().composed = true;
    }

    /// Read the live tools of `identity`'s member on `session_id` and write
    /// them to its tool record: what the member sees once the composition
    /// published its tools. A failed read leaves the record as the build's
    /// report wrote it.
    pub(crate) async fn record_live_tools(
        &self,
        runtime: &IdentityRuntime,
        identity: &AgentIdentity,
        session_id: meerkat_core::types::SessionId,
    ) {
        let service = self.session_service.get();
        match crate::mob_handle_runtime::resolved_tools_for_session(
            service,
            identity.as_str(),
            session_id.clone(),
        )
        .await
        {
            Ok(snapshot) => {
                runtime
                    .record_built_tools(identity, &session_id, snapshot.tools)
                    .await;
            }
            Err(error) => tracing::debug!(
                %identity,
                %error,
                "the live tools of a member whose customizer tools were just published could \
                 not be read; its tool record stays as its build reported it"
            ),
        }
    }

    /// The Meta identity `request`'s member belongs to, if any: the
    /// identity a registered roster identity's member id names, the identity
    /// created at runtime its member id names (registered now from its row
    /// in the mob's identity store, whether or not a roster pass or a door
    /// registered it yet, and prepared as a registered identity's), or the
    /// identity its runtime-authoritative label names. `Ok(None)` for every
    /// other member. It runs inside Core's build of the member, so the
    /// registration of a created identity never waits on its lifecycle lock
    /// ([`IdentityRuntime::register_created_member_for_build`]); a row that
    /// could not be read refuses the build, retryable.
    async fn identity_of(
        &self,
        request: &MemberBuildPreparation,
        runtime: Option<&IdentityRuntime>,
    ) -> Result<Option<AgentIdentity>, MemberBuildPreparationRefusal> {
        if let Some(runtime) = runtime
            && let Ok(identity) =
                AgentIdentity::parse(&crate::member_comms_id::durable_identity_for_roster_member(
                    request.agent_identity.as_str(),
                ))
        {
            if runtime.contains(&identity).await {
                return Ok(Some(identity));
            }
            if runtime
                .register_created_member_for_build(&identity)
                .await
                .map_err(|reason| MemberBuildPreparationRefusal::Retryable { reason })?
            {
                return Ok(Some(identity));
            }
        }
        let entry = request
            .handle
            .get_member(&request.agent_identity)
            .await
            .map_err(|error| MemberBuildPreparationRefusal::Retryable {
                reason: format!("the member's roster entry could not be read: {error}"),
            })?;
        // Core's entry read is the identity runtime's bridge's here: the
        // build's read handle carries none.
        let entries = runtime
            .and_then(IdentityRuntime::bridge)
            .map(|bridge| &**bridge);
        Ok(entry.as_ref().and_then(|entry| {
            crate::member_comms_id::resolve_member_identity(
                &entries,
                &entry.agent_identity,
                &entry.labels,
            )
            .and_then(|identity| AgentIdentity::parse(&identity).ok())
        }))
    }
}

impl IdentityBuildPreparer {
    /// Prepare the build of `member`, the member of Meta identity `identity`:
    /// held before the composition, published inline after it, and never
    /// held when a host adopted the identity. `management` reads the
    /// identity's management in Core, asked for every build that is not a
    /// registered identity's after the composition.
    async fn prepare_identity_member<M>(
        &self,
        member: &MobAgentIdentity,
        identity: AgentIdentity,
        runtime: Option<Arc<IdentityRuntime>>,
        management: M,
    ) -> Result<MemberBuildPrepared, MemberBuildPreparationRefusal>
    where
        M: std::future::Future<
                Output = Result<meerkat_mob::MemberIdentityManagement, meerkat_mob::MobError>,
            > + Send,
    {
        {
            let mut state = self.lock_state();
            state.building.insert(member.clone(), identity.clone());
            // The seat of this build was composed: the host's customizer
            // ran for the build and its tools are published, so the
            // preparation answers what the composition found.
            if let Some(answer) = state.seat_composed.remove(member) {
                return Ok(answer);
            }
            if !self.publishes_tools() {
                return Ok(MemberBuildPrepared::Prepared);
            }
        }
        // After the composition a registered identity's build is prepared
        // inline.
        let composed = self.lock_state().composed;
        if composed
            && let Some(runtime) = runtime.as_ref()
            && runtime.contains(&identity).await
        {
            return runtime.prepare_identity_build(&identity).await;
        }
        // Every other build is held until the roster pass releases it: the
        // composition publishes a registered identity's tools, and the
        // roster pass retires the member of an identity it does not list. A
        // member of an identity a host adopted through an identity intent
        // row is neither, before the composition or after it: no
        // publication or retirement covers it, so it is not Meta's to hold.
        match management.await {
            Ok(
                meerkat_mob::MemberIdentityManagement::IntentManaged
                | meerkat_mob::MemberIdentityManagement::IntentUnreadable,
            ) => return Ok(MemberBuildPrepared::Prepared),
            Ok(_) => {}
            Err(error) => {
                return Err(MemberBuildPreparationRefusal::Retryable {
                    reason: format!("the identity's management could not be read: {error}"),
                });
            }
        }
        // A build answered before the composition is held: Core stages its
        // hold, and the composition (which reads Core's held members) or the
        // build's report publishes it. The check and the mark are one step
        // under the preparer's lock, which the composition takes.
        let runtime = {
            let mut state = self.lock_state();
            match runtime {
                None => {
                    state.held_builds.insert(member.clone());
                    return Ok(MemberBuildPrepared::HoldUntilReleased);
                }
                Some(_) if !state.composed => {
                    state.held_builds.insert(member.clone());
                    return Ok(MemberBuildPrepared::HoldUntilReleased);
                }
                Some(runtime) => runtime,
            }
        };
        // The composition ran while the management was read: a registered
        // identity's build is prepared inline, and a member of an identity
        // the roster does not list is held until the roster pass retires it.
        if runtime.contains(&identity).await {
            return runtime.prepare_identity_build(&identity).await;
        }
        Ok(MemberBuildPrepared::HoldUntilReleased)
    }

    /// Compose `seat`, the seat Core lowered for a build of the member of
    /// an identity whose record is its row
    /// (`MemberBuildPreparer::prepare_store_owned_seat`): once the identity
    /// runtime is composed, the identity is registered from its row if no
    /// roster pass or door registered it yet, the host's customizer runs
    /// once for the build, its tools are published, and the rest of its
    /// draft is lowered onto the seat
    /// ([`IdentityRuntime::compose_store_owned_seat`]). The build's
    /// preparation, which Core asks next, then answers what the composition
    /// found and does not run the customizer again.
    async fn compose_store_owned_seat(
        &self,
        seat: &mut SpawnMemberSpec,
    ) -> Result<(), MemberBuildPreparationRefusal> {
        // An inert preparer, and a composition with no agent customizer,
        // have no host build step: the seat stays as Core lowered it.
        if !self.publishes_tools() {
            return Ok(());
        }
        // Nothing is kept of an earlier composition of this member's seat.
        self.lock_state().seat_composed.remove(&seat.identity);
        let composed = self.lock_state().composed;
        let runtime = match self.bound_runtime() {
            Some(Ok(runtime)) if composed => runtime,
            Some(Err(())) => {
                return Err(MemberBuildPreparationRefusal::Retryable {
                    reason: "the identity runtime is shut down".to_string(),
                });
            }
            // The identity runtime is not composed yet, so its customizer
            // cannot run. A fresh spawn is left as Core lowered it: refused,
            // the identity would stay owed its seat until the mob's next
            // start, which asks in this same state. A wake or a restore is
            // refused for now: the member stays Dormant, and Core asks
            // again once the retry is due.
            Some(Ok(_)) | None => {
                if matches!(
                    seat.launch_mode,
                    meerkat_mob::launch::MemberLaunchMode::Fresh
                ) {
                    return Ok(());
                }
                return Err(MemberBuildPreparationRefusal::Retryable {
                    reason: SEAT_COMPOSED_ONCE_COMPOSED.to_string(),
                });
            }
        };
        let Ok(identity) = AgentIdentity::parse(
            &crate::member_comms_id::durable_identity_for_roster_member(seat.identity.as_str()),
        ) else {
            return Ok(());
        };
        // An identity no roster pass or door registered yet is registered
        // from its row, as its build's preparation registers it; a row that
        // could not be read refuses the build, retryable.
        if !runtime.contains(&identity).await
            && !runtime
                .register_created_member_for_build(&identity)
                .await
                .map_err(|reason| MemberBuildPreparationRefusal::Retryable { reason })?
        {
            return Ok(());
        }
        // Held behind a box: the customizer's future and the lowering stay
        // out of the frame of Core's build.
        if let Some(answer) = Box::pin(runtime.compose_store_owned_seat(&identity, seat)).await? {
            self.lock_state()
                .seat_composed
                .insert(seat.identity.clone(), answer);
        }
        Ok(())
    }

    /// Record the visible tools Core reported for the build of `member` on
    /// `session_id`, for the identity its preparation named, or, for a build
    /// Core did not ask this preparer about, the registered identity its
    /// member id names. Any other build is not recorded. A build this
    /// preparer held before the composition, reported once the runtime is
    /// composed, is published now
    /// ([`IdentityRuntime::publish_held_member_build`]).
    async fn record_member_build(
        &self,
        member: &MobAgentIdentity,
        session_id: meerkat_core::types::SessionId,
        visible_tools: Vec<String>,
    ) {
        let (prepared, held_and_composed) = {
            let mut state = self.lock_state();
            let prepared = state.building.remove(member);
            let held = state.held_builds.remove(member);
            state.seat_composed.remove(member);
            if let Some(identity) = &prepared
                && self.runtime.get().is_none()
            {
                state
                    .reports_unbound
                    .push((identity.clone(), session_id, visible_tools));
                return;
            }
            (prepared, held && state.composed)
        };
        let Some(Ok(runtime)) = self.bound_runtime() else {
            return;
        };
        let identity = match prepared {
            Some(identity) => identity,
            // A build Core reports without asking this preparer first (a
            // spawn Core commits live: an embodied seat, a held create) is
            // recorded for the registered identity its member id names.
            None => {
                let Ok(identity) = AgentIdentity::parse(
                    &crate::member_comms_id::durable_identity_for_roster_member(member.as_str()),
                ) else {
                    return;
                };
                if !runtime.contains(&identity).await {
                    return;
                }
                identity
            }
        };
        runtime
            .record_built_tools(&identity, &session_id, visible_tools)
            .await;
        if held_and_composed {
            runtime
                .publish_held_member_build(self, &identity, session_id)
                .await;
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl MemberBuildPreparer for IdentityBuildPreparer {
    async fn prepare_member_build(
        &self,
        request: MemberBuildPreparation,
    ) -> Result<MemberBuildPrepared, MemberBuildPreparationRefusal> {
        // An inert preparer answers as if the mob had none: it reads nothing.
        if self.inert() {
            return Ok(MemberBuildPrepared::Prepared);
        }
        let runtime = match self.bound_runtime() {
            Some(Ok(runtime)) => Some(runtime),
            Some(Err(())) => {
                return Err(MemberBuildPreparationRefusal::Retryable {
                    reason: "the identity runtime is shut down".to_string(),
                });
            }
            None => None,
        };
        let Some(identity) = self.identity_of(&request, runtime.as_deref()).await? else {
            return Ok(MemberBuildPrepared::Prepared);
        };
        let handle = request.handle.clone();
        let member = request.agent_identity.clone();
        self.prepare_identity_member(&request.agent_identity, identity, runtime, async move {
            handle.identity_management(&member).await
        })
        .await
    }

    async fn prepare_store_owned_seat(
        &self,
        _handle: MobReadHandle,
        seat: &mut SpawnMemberSpec,
    ) -> Result<(), MemberBuildPreparationRefusal> {
        // The composition reads through the identity runtime's session
        // bridge, as the build's preparation reads a row: it needs nothing
        // of the build's read handle.
        self.compose_store_owned_seat(seat).await
    }

    async fn member_built(&self, report: MemberBuildReport) {
        if self.inert() {
            return;
        }
        self.record_member_build(
            &report.agent_identity,
            report.session_id,
            report.visible_tools,
        )
        .await;
    }
}

#[cfg(test)]
#[path = "build_preparer_tests.rs"]
mod tests;
