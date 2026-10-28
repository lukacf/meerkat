//! The identity runtime's side of Meta's member build preparation
//! ([`super::super::build_preparer::IdentityBuildPreparer`]): the inline
//! preparation of a registered identity's build, the hold of a build that
//! comes before the runtime is composed, and the composition that publishes
//! the held members' tools and releases their holds.

use std::sync::Arc;

use futures::stream::{self, StreamExt};
use meerkat_mob::{MemberBuildPreparationRefusal, MemberBuildPrepared};

use super::super::bridge::{BridgeAdmissionError, BridgeError};
use super::super::build_preparer::{IdentityBuildPreparer, PUBLISHED_ONCE_COMPOSED};
use super::super::types::{AgentBuildContext, AgentIdentity, CustomizerToolsPending};
use super::{IdentityRuntime, IdentityRuntimeError};

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

    /// Record that Core built `identity`'s member held: its customizer tools
    /// are pending until a publication releases the hold. A reason already
    /// recorded (an earlier publication that failed) is kept.
    pub(crate) fn hold_member_build(&self, identity: &AgentIdentity) {
        self.customizer_tools_pending
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(identity.clone())
            .or_insert_with(|| CustomizerToolsPending {
                reason: PUBLISHED_ONCE_COMPOSED.to_string(),
                artifact_refusal: None,
            });
    }

    /// Prepare the build of registered `identity`'s member Core is about to
    /// perform: run the host's `customize_build` with the runtime's real
    /// context and publish its tools into the identity's stable dispatcher,
    /// so the member is built with them. Nothing is released: the build
    /// starts with no hold.
    ///
    /// A failed build is refused retryable: the member stays Dormant, the
    /// identity's customizer tools are recorded pending with the reason
    /// (typed when the host's installed artifacts refused them), and a later
    /// delivery tries again. A candidate launch publishes and holds the
    /// member: its runs belong to the authoritative launch.
    pub(crate) async fn prepare_identity_build(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberBuildPrepared, MemberBuildPreparationRefusal> {
        let Some(customizer) = self.customizer.read().await.clone() else {
            return Ok(MemberBuildPrepared::Prepared);
        };
        let Some(spec) = self
            .entries
            .read()
            .await
            .get(identity)
            .map(|entry| entry.spec.clone())
        else {
            // The identity left the roster since the preparer read it: the
            // roster pass retires its member, which holds no run until then.
            self.hold_member_build(identity);
            return Ok(MemberBuildPrepared::HoldUntilReleased);
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
                if self.is_candidate_launch() {
                    return Ok(MemberBuildPrepared::HoldUntilReleased);
                }
                Ok(MemberBuildPrepared::Prepared)
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
                     published, so it stays dormant until a later delivery publishes them"
                );
                self.refused_build_preparations
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        identity.clone(),
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
    /// registered identity whose tools are still pending (Core built its
    /// member held before the composition, or a publication failed) is
    /// published now, which releases its hold, and the member's live tools
    /// are written to its tool record. Run after each roster registration,
    /// so an identity the roster lists again, or one whose publication failed
    /// before, is published by the next one.
    ///
    /// The publications run with no lifecycle lock held, at most
    /// [`super::super::orchestrator::identity_restore_concurrency`] at once.
    /// A failed one keeps the identity pending with its reason: its member
    /// stays held until a later composition or delivery publishes.
    pub(crate) async fn compose_member_builds(&self) {
        let Some(preparer) = self.member_build_preparer() else {
            return;
        };
        let pending = preparer.compose(|| {
            self.customizer_tools_pending
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        });
        if !preparer.publishes_tools() || pending.is_empty() {
            return;
        }
        let mut registered = Vec::with_capacity(pending.len());
        {
            let entries = self.entries.read().await;
            for identity in pending {
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
                    match self
                        .publish_pending_customizer_tools(
                            &identity,
                            session_id.as_ref(),
                            "composition",
                        )
                        .await
                    {
                        Ok(()) => {
                            if let Some(session_id) = session_id {
                                preparer
                                    .record_live_tools(self, &identity, session_id)
                                    .await;
                            }
                        }
                        Err(error) => tracing::warn!(
                            %identity,
                            %error,
                            "the customizer tools of a member Core built before the \
                             composition were not published; it stays held until a later \
                             composition or delivery publishes them"
                        ),
                    }
                }
            })
            .buffer_unordered(super::super::orchestrator::identity_restore_concurrency())
            .collect::<Vec<_>>()
            .await;
    }

    /// The error a delivery to `identity` reports for its admission's bridge
    /// error: a build Core refused because its preparation failed reports
    /// the recorded reason, typed ([`Self::pending_customizer_refusal`]).
    /// Every other error maps as the admission phase maps it.
    pub(crate) fn admission_error(
        &self,
        identity: &AgentIdentity,
        error: BridgeAdmissionError,
    ) -> IdentityRuntimeError {
        if let BridgeAdmissionError::MemberWakeRefused {
            refusal: meerkat_mob::MemberWakeRefusal::HostPreparationPending,
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
            refusal: meerkat_mob::MemberWakeRefusal::HostPreparationPending,
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
            refusal: meerkat_mob::MemberWakeRefusal::HostPreparationPending,
            ..
        } = &error
            && let Some(refusal) = self.pending_customizer_refusal(identity)
        {
            return refusal;
        }
        super::classify_wake_failure(identity, session_id, error)
    }

    /// What a delivery reports when Core refused to build `identity`'s
    /// member because its preparation failed: the recorded reason, typed
    /// when the host's installed artifacts refused the tools. `None` when no
    /// failed preparation is recorded for the identity.
    pub(crate) fn pending_customizer_refusal(
        &self,
        identity: &AgentIdentity,
    ) -> Option<IdentityRuntimeError> {
        let pending = self.customizer_tools_pending_for(identity)?;
        if pending.reason == PUBLISHED_ONCE_COMPOSED {
            return None;
        }
        Some(match pending.artifact_refusal {
            Some(refusal) => IdentityRuntimeError::ToolArtifactRefused(Box::new(refusal)),
            None => IdentityRuntimeError::IdentityWakeRetryable {
                identity: identity.clone(),
                session_id: None,
                detail: format!(
                    "the member's customizer tools are not published ({}); it is not built \
                     until they are, retry",
                    pending.reason
                ),
            },
        })
    }
}
