//! The identity runtime's side of Meta's member build preparation
//! ([`super::super::build_preparer::IdentityBuildPreparer`]): the inline
//! preparation of a registered identity's build, and the composition that
//! publishes the tools of the members Core holds for unpublished tools and
//! releases their holds. Which members Core holds is Core's read
//! ([`super::super::bridge::SessionBridge::members_holding_run_starts_for_published_tools`]),
//! and a build preparation the host asked to retry is Core's typed cause
//! (`MemberWakeRefusal::BuildPreparationRetry`); Meta keeps only its
//! customizer's last failure text, for the status and the typed answer to a
//! refused delivery. The door's one retry of a wake Core refused for a cause
//! the host ends ([`IdentityRuntime::door_retries`]) lives here too.

use std::sync::Arc;

use futures::stream::{self, StreamExt};
use meerkat_mob::{MemberBuildPreparationRefusal, MemberBuildPrepared};

use super::super::bridge::{BridgeAdmissionError, BridgeError};
use super::super::build_preparer::IdentityBuildPreparer;
use super::super::types::{
    AgentBuildContext, AgentBuildDraft, AgentIdentity, CustomizerToolsPending, DurableAgentSpec,
};
use super::{IdentityRuntime, IdentityRuntimeError};

/// How far the host's build step of one build got
/// ([`IdentityRuntime::compose_identity_build`]).
enum ComposedIdentityBuild {
    /// The runtime composes no customizer: there is no host build step.
    NoCustomizer,
    /// The identity is not registered (it left the roster since the
    /// preparer read it).
    Unregistered,
    /// The customizer ran and its tools are published: the identity's
    /// registered spec and the draft the customizer composed from it.
    Composed(Box<(DurableAgentSpec, AgentBuildDraft)>),
}

impl IdentityRuntime {
    /// Install the preparer the mob was built with: the composition
    /// ([`Self::compose_member_builds`]) goes through it.
    pub(crate) fn set_member_build_preparer(&self, preparer: Arc<IdentityBuildPreparer>) {
        *self
            .member_build_preparer
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(preparer);
    }

    pub(crate) fn member_build_preparer(&self) -> Option<Arc<IdentityBuildPreparer>> {
        self.member_build_preparer
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Prepare the build of registered `identity`'s member Core is about to
    /// perform: run the host's `customize_build` with the runtime's real
    /// context and publish its tools into the identity's stable dispatcher,
    /// so the member is built with them. Nothing is released: the build
    /// starts with no hold.
    ///
    /// A failed build is refused retryable: the member stays Dormant, Core
    /// records the build preparation's retry (its wakes are refused
    /// `BuildPreparationRetry` until the host retries it, or Core's retry
    /// timer does), and the customizer's failure is recorded with its reason
    /// (typed when the host's installed artifacts refused the tools). A
    /// candidate launch publishes and holds the member: its runs belong to
    /// the authoritative launch.
    pub(crate) async fn prepare_identity_build(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberBuildPrepared, MemberBuildPreparationRefusal> {
        Ok(match self.compose_identity_build(identity, None).await? {
            ComposedIdentityBuild::NoCustomizer => MemberBuildPrepared::Prepared,
            // The identity left the roster since the preparer read it: the
            // roster pass retires its member, which holds no run until then.
            ComposedIdentityBuild::Unregistered => MemberBuildPrepared::HoldUntilReleased,
            ComposedIdentityBuild::Composed(_) => self.composed_build_answer(),
        })
    }

    /// What Core is answered for a build whose customizer tools were just
    /// published: it may run, except on a candidate launch, which holds the
    /// member (its runs belong to the authoritative launch).
    fn composed_build_answer(&self) -> MemberBuildPrepared {
        if self.is_candidate_launch() {
            MemberBuildPrepared::HoldUntilReleased
        } else {
            MemberBuildPrepared::Prepared
        }
    }

    /// Compose `seat`, the seat Core lowered for a build of registered
    /// `identity`'s member, an identity whose record is its row: run the
    /// host's `customize_build` once for this build, publish its tools into
    /// the identity's stable dispatcher, and lower the rest of the draft
    /// onto the seat through the lowering a configured identity's seat is
    /// composed with
    /// ([`super::super::bridge::SessionBridge::lower_store_owned_seat_build`]).
    /// Core decides what the build takes of it: the model and the provider
    /// parameters at every build, the context, the instructions and the
    /// system prompt at the first build of the member's session.
    ///
    /// `row` is the spec the identity's row holds now, when it reads: the
    /// customizer composes from it, so the draft follows a row Core changed
    /// since the identity was registered. Without it the registered spec
    /// stands in.
    ///
    /// The draft's labels must be exactly the seat's. What decides on a
    /// member's labels reads its seat, and the seat of such an identity
    /// carries its row's labels and no others, so a label the customizer
    /// sets that the seat lacks cannot be honoured: the composition is
    /// refused for good
    /// (`MemberBuildPreparationRefusal::store_owned_seat_labels`, naming
    /// the label keys and never a value), the member is not built, and the
    /// reason is recorded for the identity's status. Core refuses such an
    /// answer itself: the draft's labels are lowered onto the seat, so the
    /// refusal does not rest on this check.
    ///
    /// `Ok(Some(answer))` is what this build's preparation answers, the
    /// customizer having run for it already; `Ok(None)` when nothing was
    /// composed (the runtime composes no customizer, or the identity left
    /// the registration) and the seat is as Core lowered it. A failed
    /// customizer, and a draft that does not lower, refuse the build
    /// retryable, with the failure recorded for the identity's status and
    /// the typed answer to a refused delivery.
    pub(crate) async fn compose_store_owned_seat(
        &self,
        identity: &AgentIdentity,
        row: Option<DurableAgentSpec>,
        seat: &mut meerkat_mob::SpawnMemberSpec,
    ) -> Result<Option<MemberBuildPrepared>, MemberBuildPreparationRefusal> {
        let Some(bridge) = self.bridge() else {
            return Ok(None);
        };
        let ComposedIdentityBuild::Composed(composed) =
            self.compose_identity_build(identity, row).await?
        else {
            return Ok(None);
        };
        let (spec, draft) = *composed;
        if let Some(refusal) =
            MemberBuildPreparationRefusal::store_owned_seat_labels(seat, &draft.labels)
        {
            tracing::warn!(
                %identity,
                reason = refusal.reason(),
                "the member's build was refused: its host's customizer answers labels its seat                  does not carry"
            );
            self.record_customizer_failure(
                identity,
                CustomizerToolsPending {
                    reason: refusal.reason().to_string(),
                    artifact_refusal: None,
                },
            );
            return Err(refusal);
        }
        if let Err(error) = bridge.lower_store_owned_seat_build(&spec, &draft, seat) {
            let reason = format!("the host's build did not lower onto the member's seat: {error}");
            tracing::warn!(
                %identity,
                %reason,
                "the member's build was refused: it stays dormant until its build preparation \
                 is retried"
            );
            self.record_customizer_failure(
                identity,
                CustomizerToolsPending {
                    reason: reason.clone(),
                    artifact_refusal: None,
                },
            );
            return Err(MemberBuildPreparationRefusal::Retryable { reason });
        }
        Ok(Some(self.composed_build_answer()))
    }

    /// Run the host's `customize_build` for the build of registered
    /// `identity`'s member Core is about to perform, with the runtime's
    /// real context, and publish its tools into the identity's stable
    /// dispatcher, unreleased: the one host build step of a build, whose
    /// draft the caller lowers further or drops. The customizer composes
    /// from `row`, the spec the identity's row holds now, when the caller
    /// read one, else from the registered spec. A failed customizer is
    /// refused retryable, its failure recorded.
    async fn compose_identity_build(
        &self,
        identity: &AgentIdentity,
        row: Option<DurableAgentSpec>,
    ) -> Result<ComposedIdentityBuild, MemberBuildPreparationRefusal> {
        let Some(customizer) = self.customizer.read().await.clone() else {
            return Ok(ComposedIdentityBuild::NoCustomizer);
        };
        let registered = match row {
            Some(row) => Some(row),
            None => self
                .entries
                .read()
                .await
                .get(identity)
                .map(|entry| entry.spec.clone()),
        };
        let Some(spec) = registered else {
            return Ok(ComposedIdentityBuild::Unregistered);
        };
        let context = AgentBuildContext {
            identity: identity.clone(),
            active_peers: self.entries.read().await.keys().cloned().collect(),
            managed_edges: self.desired_peer_edges.read().await.clone(),
            runtime_services: self.runtime_services(),
        };
        let mut draft = Self::base_build_draft(&spec);
        let shutting_down = || MemberBuildPreparationRefusal::Retryable {
            reason: "the identity runtime is shutting down".to_string(),
        };
        let mut shutdown = self.foreground_cancel.subscribe();
        if *shutdown.borrow() {
            return Err(shutting_down());
        }
        let result = {
            let customize = customizer.customize_build(&context, &spec, &mut draft);
            tokio::pin!(customize);
            tokio::select! {
                result = &mut customize => result,
                changed = shutdown.changed() => {
                    if changed.is_ok() && *shutdown.borrow() {
                        return Err(shutting_down());
                    }
                    customize.await
                }
            }
        };
        match result {
            Ok(()) => {
                self.publish_customizer_tools_unreleased(identity, &mut draft)
                    .await;
                Ok(ComposedIdentityBuild::Composed(Box::new((spec, draft))))
            }
            Err(error) => {
                let reason = format!("customize_build failed: {error}");
                let artifact_refusal = match &error {
                    super::super::types::CustomizerError::ToolArtifactRefused(refusal) => {
                        Some(refusal.as_ref().clone())
                    }
                    _ => None,
                };
                tracing::warn!(
                    %identity,
                    %reason,
                    "the member's build was refused: its customizer tools could not be \
                     published, so it stays dormant until its build preparation is retried"
                );
                self.record_customizer_failure(
                    identity,
                    CustomizerToolsPending {
                        reason: reason.clone(),
                        artifact_refusal,
                    },
                );
                Err(MemberBuildPreparationRefusal::Retryable { reason })
            }
        }
    }

    /// The runtime is composed: its roster is registered, and its customizer,
    /// customizer tool registry and services are installed. From now on the
    /// preparer prepares each registered identity's build inline. Every
    /// registered identity whose member Core holds for unpublished tools
    /// (Core built it held before the composition, or a publication failed)
    /// is published now, which releases its hold, and the member's live tools
    /// are written to its tool record. Run after each roster registration,
    /// so an identity the roster lists again, or one whose publication failed
    /// before, is published by the next one. A build Core held before the
    /// composition and reports after it is published at its report
    /// ([`Self::publish_held_member_build`]).
    ///
    /// The publications run with no lifecycle lock held, at most
    /// [`super::super::orchestrator::identity_restore_concurrency`] at once.
    /// A failed one records its reason: its member stays held until a later
    /// composition or delivery publishes.
    pub(crate) async fn compose_member_builds(&self) {
        let Some(preparer) = self.member_build_preparer() else {
            return;
        };
        preparer.compose();
        if !preparer.publishes_tools() {
            return;
        }
        let held = match self.members_core_holds_for_unpublished_tools() {
            Ok(held) => held,
            // Nothing is published, so every member Core holds stays held:
            // a later composition, or the member's wake or delivery, reads
            // its hold again.
            Err(error) => {
                tracing::warn!(
                    %error,
                    "Core's members held for unpublished customizer tools did not read; \
                     they stay held until a later composition or delivery publishes them"
                );
                return;
            }
        };
        if held.is_empty() {
            return;
        }
        let mut registered = Vec::with_capacity(held.len());
        {
            let entries = self.entries.read().await;
            for identity in held {
                if entries.contains_key(&identity) {
                    registered.push(identity);
                }
            }
        }
        if registered.is_empty() {
            return;
        }
        tracing::info!(
            member_count = registered.len(),
            "publishing the customizer tools of members Core built before the identity runtime \
             was composed"
        );
        stream::iter(registered)
            .map(|identity| {
                let preparer = Arc::clone(&preparer);
                async move {
                    let session_id = match self.core_live_member(&identity).await {
                        Ok(Some(live)) => live.session_id,
                        _ => None,
                    };
                    self.publish_held_member(&preparer, &identity, session_id, "composition")
                        .await;
                }
            })
            .buffer_unordered(super::super::orchestrator::identity_restore_concurrency())
            .collect::<Vec<_>>()
            .await;
    }

    /// The report of a build the preparer held (`HoldUntilReleased`) for
    /// registered `identity`, on `session_id`, once the runtime is composed:
    /// Core staged the hold with the build, so a composition that read
    /// Core's held members before that missed it. Published here, as the
    /// composition publishes it, when Core still holds it.
    pub(crate) async fn publish_held_member_build(
        &self,
        preparer: &IdentityBuildPreparer,
        identity: &AgentIdentity,
        session_id: meerkat_core::types::SessionId,
    ) {
        if !preparer.publishes_tools() || !self.entries.read().await.contains_key(identity) {
            return;
        }
        self.publish_held_member(preparer, identity, Some(session_id), "build report")
            .await;
    }

    /// Publish the tools of `identity`'s member Core holds for unpublished
    /// tools, which releases the hold, and write its live tools to its tool
    /// record. A failed publication is logged; its reason is recorded.
    async fn publish_held_member(
        &self,
        preparer: &IdentityBuildPreparer,
        identity: &AgentIdentity,
        session_id: Option<meerkat_core::types::SessionId>,
        operation: &'static str,
    ) {
        match self
            .publish_pending_customizer_tools(identity, session_id.as_ref(), operation)
            .await
        {
            Ok(()) => {
                if let Some(session_id) = session_id {
                    preparer.record_live_tools(self, identity, session_id).await;
                }
            }
            Err(error) => tracing::warn!(
                %identity,
                %error,
                "the customizer tools of a member Core holds for unpublished tools were not \
                 published; it stays held until a later composition or delivery publishes them"
            ),
        }
    }

    /// The door's one retry of a wake (a delivery's, a materialization's)
    /// that Core refused for a cause the host ends itself; `refused` names
    /// the member and Core's refusal:
    /// - a build preparation the host asked to retry
    ///   (`BuildPreparationRetry`): the host retries the preparation
    ///   (`resolve_member_build_preparation`), so Core's next wake of the
    ///   member asks the preparer again;
    /// - a host preparation Core keeps for the identity
    ///   (`HostPreparationPending`): the identity is declared again
    ///   ([`Self::declare_identity_locked`]; the caller holds its lifecycle
    ///   lock), the host's answer to that preparation. The door declares
    ///   first an identity whose row Meta's own first-boot records hold, so
    ///   this is a preparation those records do not name.
    ///
    /// `Ok(true)` when the door took its step: the caller wakes, or delivers,
    /// once more, and a second refusal surfaces typed (`retried` is then
    /// set). Any other refusal is not the door's: `Ok(false)`. A healthy wake
    /// asks Core for nothing.
    pub(crate) async fn door_retries(
        &self,
        identity: &AgentIdentity,
        retried: &mut bool,
        refused: Option<(&meerkat_mob::AgentIdentity, meerkat_mob::MemberWakeRefusal)>,
    ) -> Result<bool, IdentityRuntimeError> {
        let Some((member, refusal)) = refused else {
            return Ok(false);
        };
        if *retried {
            return Ok(false);
        }
        match refusal {
            meerkat_mob::MemberWakeRefusal::BuildPreparationRetry => {
                *retried = true;
                self.core_bridge(identity)?
                    .resolve_member_build_preparation(member)
                    .await
                    .map_err(|error| IdentityRuntimeError::IdentityWakeRetryable {
                        identity: identity.clone(),
                        session_id: None,
                        detail: format!(
                            "Core refused the member's wake for the build preparation its host \
                             asked to retry, and the retry was not asked: {error}"
                        ),
                    })?;
                Ok(true)
            }
            meerkat_mob::MemberWakeRefusal::HostPreparationPending => {
                *retried = true;
                let spec = self
                    .entries
                    .read()
                    .await
                    .get(identity)
                    .map(|entry| entry.spec.clone())
                    .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
                self.declare_identity_locked(&spec, None).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// The error a delivery to `identity` reports for its admission's bridge
    /// error: a wake Core refused for the build preparation the host asked
    /// to retry reports the customizer's recorded failure, typed
    /// ([`Self::pending_customizer_refusal`]). Every other error maps as the
    /// admission phase maps it.
    pub(crate) fn admission_error(
        &self,
        identity: &AgentIdentity,
        error: BridgeAdmissionError,
    ) -> IdentityRuntimeError {
        if let BridgeAdmissionError::MemberWakeRefused {
            refusal: meerkat_mob::MemberWakeRefusal::BuildPreparationRetry,
            ..
        } = &error
            && let Some(refusal) = self.pending_customizer_refusal(identity)
        {
            return refusal;
        }
        super::admission_phase_error(identity, error)
    }

    /// [`Self::admission_error`] for the ingress-only lane.
    pub(crate) fn ingress_error(
        &self,
        identity: &AgentIdentity,
        error: BridgeError,
    ) -> IdentityRuntimeError {
        if let BridgeError::MemberWakeRefused {
            refusal: meerkat_mob::MemberWakeRefusal::BuildPreparationRetry,
            ..
        } = &error
            && let Some(refusal) = self.pending_customizer_refusal(identity)
        {
            return refusal;
        }
        super::ingress_phase_error(identity, error)
    }

    /// [`Self::admission_error`] for a wake Meta asked Core for
    /// (`ensure_member_live`), classified as a wake failure otherwise.
    pub(crate) fn wake_error(
        &self,
        identity: &AgentIdentity,
        session_id: Option<&meerkat_core::types::SessionId>,
        error: BridgeError,
    ) -> IdentityRuntimeError {
        if let BridgeError::MemberWakeRefused {
            refusal: meerkat_mob::MemberWakeRefusal::BuildPreparationRetry,
            ..
        } = &error
            && let Some(refusal) = self.pending_customizer_refusal(identity)
        {
            return refusal;
        }
        super::classify_wake_failure(identity, session_id, error)
    }

    /// What a delivery reports when Core refused to build `identity`'s
    /// member for the build preparation the host asked to retry: the reason
    /// the customizer failed with, typed when the host's installed artifacts
    /// refused the tools. `None` when no failure is recorded for the
    /// identity (the delivery then reports Core's refusal).
    pub(crate) fn pending_customizer_refusal(
        &self,
        identity: &AgentIdentity,
    ) -> Option<IdentityRuntimeError> {
        let failure = self.customizer_failure(identity)?;
        Some(match failure.artifact_refusal {
            Some(refusal) => IdentityRuntimeError::ToolArtifactRefused(Box::new(refusal)),
            None => IdentityRuntimeError::IdentityWakeRetryable {
                identity: identity.clone(),
                session_id: None,
                detail: format!(
                    "the member's customizer tools are not published ({}); it is not built \
                     until they are, retry",
                    failure.reason
                ),
            },
        })
    }
}

/// The member and Core's refusal when Core refused a wake for a cause the
/// door can take a step for ([`IdentityRuntime::door_retries`]): a build
/// preparation the host asked to retry (`BuildPreparationRetry`), or the
/// host preparation Core keeps (`HostPreparationPending`). `None` for any
/// other error.
pub(crate) fn door_refusal_of(
    error: &BridgeError,
) -> Option<(&meerkat_mob::AgentIdentity, meerkat_mob::MemberWakeRefusal)> {
    match error {
        BridgeError::MemberWakeRefused {
            identity,
            refusal:
                refusal @ (meerkat_mob::MemberWakeRefusal::BuildPreparationRetry
                | meerkat_mob::MemberWakeRefusal::HostPreparationPending),
            ..
        } => Some((identity, *refusal)),
        _ => None,
    }
}

/// [`door_refusal_of`] for an admission's error.
pub(crate) fn admission_door_refusal_of(
    error: &BridgeAdmissionError,
) -> Option<(&meerkat_mob::AgentIdentity, meerkat_mob::MemberWakeRefusal)> {
    match error {
        BridgeAdmissionError::MemberWakeRefused {
            identity,
            refusal:
                refusal @ (meerkat_mob::MemberWakeRefusal::BuildPreparationRetry
                | meerkat_mob::MemberWakeRefusal::HostPreparationPending),
            ..
        } => Some((identity, *refusal)),
        _ => None,
    }
}
