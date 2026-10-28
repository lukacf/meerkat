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
//!   with them;
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
//! publish. The preparation reads only through the request's read handle:
//! it runs inside Core's wake and inside the boot restore, so it never
//! issues a mob command.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Weak};

use meerkat_mob::{
    AgentIdentity as MobAgentIdentity, MemberBuildPreparation, MemberBuildPreparationRefusal,
    MemberBuildPrepared, MemberBuildPreparer, MemberBuildReport, MobSessionService,
};

use super::runtime::IdentityRuntime;
use super::types::AgentIdentity;

/// Why an identity's customizer tools wait while the identity runtime is
/// not composed yet.
pub(crate) const PUBLISHED_ONCE_COMPOSED: &str =
    "customizer tools are published once the identity runtime is composed";

/// Meta's preparer of member builds: see the module documentation.
///
/// Created before the mob is built, so it can be installed on it, and bound
/// to the identity runtime once that exists (`bind`).
pub struct IdentityBuildPreparer {
    /// Whether builds are prepared with customizer tools: the composition
    /// has an agent customizer.
    publishes_tools: bool,
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
    /// Identities whose builds were held before the runtime was bound.
    held_unbound: BTreeSet<AgentIdentity>,
    /// Builds reported before the runtime was bound: the identity, the
    /// session and the visible tools of each.
    reports_unbound: Vec<(AgentIdentity, meerkat_core::types::SessionId, Vec<String>)>,
    /// The identity each member being built belongs to, from its
    /// preparation, for the report of that build.
    building: BTreeMap<MobAgentIdentity, AgentIdentity>,
}

impl IdentityBuildPreparer {
    /// A preparer for a composition that composes an agent customizer
    /// (`publishes_tools`) or not. Without one, builds are prepared with
    /// nothing to publish, and their reports are still recorded.
    pub fn new(publishes_tools: bool) -> Arc<Self> {
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
        self.publishes_tools
    }

    /// Bind this preparer to `runtime`, once its customizer, customizer tool
    /// registry and identity records are installed: the identities held
    /// before are recorded on it as pending, and the builds reported before
    /// are written to their tool records. A second bind is ignored.
    pub(crate) async fn bind(
        self: &Arc<Self>,
        runtime: &Arc<IdentityRuntime>,
        session_service: Option<Arc<dyn MobSessionService>>,
    ) {
        if let Some(service) = session_service {
            let _ = self.session_service.set(service);
        }
        // The held identities move to the runtime under the lock a hold
        // takes, so none is missed by a composition that follows.
        let reports = {
            let mut state = self.lock_state();
            if self.runtime.set(Arc::downgrade(runtime)).is_err() {
                return;
            }
            runtime.set_member_build_preparer(Arc::clone(self));
            for identity in std::mem::take(&mut state.held_unbound) {
                runtime.hold_member_build(&identity);
            }
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

    /// Mark the bound runtime composed, with `snapshot` read under the same
    /// lock that a hold before the composition takes: no build held before
    /// the composition is missed by it.
    pub(crate) fn compose<T>(&self, snapshot: impl FnOnce() -> T) -> T {
        let mut state = self.lock_state();
        state.composed = true;
        snapshot()
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
    /// identity a registered roster identity's member id names, or the
    /// identity its runtime-authoritative label names. `Ok(None)` for every
    /// other member.
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
            && runtime.contains(&identity).await
        {
            return Ok(Some(identity));
        }
        let entry = request
            .handle
            .get_member(&request.agent_identity)
            .await
            .map_err(|error| MemberBuildPreparationRefusal::Retryable {
                reason: format!("the member's roster entry could not be read: {error}"),
            })?;
        Ok(entry.as_ref().and_then(|entry| {
            crate::member_comms_id::durable_identity_label(&entry.labels)
                .and_then(|label| AgentIdentity::parse(label).ok())
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
            if !self.publishes_tools {
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
        // member of an identity a host adopted is neither, before the
        // composition or after it: no roster publication or retirement
        // covers it, so it is not Meta's to hold.
        match management.await {
            Ok(meerkat_mob::MemberIdentityManagement::CommandDriven) => {}
            Ok(_) => return Ok(MemberBuildPrepared::Prepared),
            Err(error) => {
                return Err(MemberBuildPreparationRefusal::Retryable {
                    reason: format!("the identity's management could not be read: {error}"),
                });
            }
        }
        // The check and the hold are one step under the preparer's lock,
        // which the composition takes to read what is held, so no hold
        // before the composition is missed by it.
        let runtime = {
            let mut state = self.lock_state();
            match runtime {
                None => {
                    state.held_unbound.insert(identity);
                    return Ok(MemberBuildPrepared::HoldUntilReleased);
                }
                Some(runtime) if !state.composed => {
                    runtime.hold_member_build(&identity);
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
        runtime.hold_member_build(&identity);
        Ok(MemberBuildPrepared::HoldUntilReleased)
    }

    /// Record the visible tools Core reported for the build of `member` on
    /// `session_id`, for the identity its preparation named, or, for a build
    /// Core did not ask this preparer about, the registered identity its
    /// member id names. Any other build is not recorded.
    async fn record_member_build(
        &self,
        member: &MobAgentIdentity,
        session_id: meerkat_core::types::SessionId,
        visible_tools: Vec<String>,
    ) {
        let prepared = {
            let mut state = self.lock_state();
            let prepared = state.building.remove(member);
            if let Some(identity) = &prepared
                && self.runtime.get().is_none()
            {
                state
                    .reports_unbound
                    .push((identity.clone(), session_id, visible_tools));
                return;
            }
            prepared
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
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl MemberBuildPreparer for IdentityBuildPreparer {
    async fn prepare_member_build(
        &self,
        request: MemberBuildPreparation,
    ) -> Result<MemberBuildPrepared, MemberBuildPreparationRefusal> {
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

    async fn member_built(&self, report: MemberBuildReport) {
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
