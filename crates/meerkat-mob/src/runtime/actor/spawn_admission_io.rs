use super::member_effect_lane::{
    MemberEffectAck, MemberEffectCommit, MemberEffectCommitFuture, MemberEffectRequest,
    MemberEffectRetention, MemberEffectSettlement,
};
use super::*;

pub(super) struct SpawnAdmissionIo {
    pub(super) session_comms: Option<Arc<dyn CoreCommsRuntime>>,
    pub(super) provisioner_comms: Option<Arc<dyn CoreCommsRuntime>>,
    /// The supervisor private-trust install, realized off the actor with the
    /// rest of the endpoint observation so a slow member runtime or comms
    /// never parks the actor's command loop. `finalize_spawn_admit` consumes
    /// it at the point where it used to install.
    pub(super) supervisor_trust: SpawnSupervisorTrust,
}

/// Outcome of the off-actor supervisor private-trust install for one spawn.
#[derive(Default)]
pub(super) enum SpawnSupervisorTrust {
    /// Nothing to install: a run-scoped flow member, a remote member, or a
    /// member without a local session and comms runtime.
    #[default]
    NotApplicable,
    Installed {
        session_id: SessionId,
        comms: Arc<dyn CoreCommsRuntime>,
        install: SupervisorPrivateTrustInstall,
    },
    Failed(SupervisorPrivateTrustInstallError),
}

struct SpawnAdmissionCommit {
    ctx: Box<SpawnFinalizeCtx>,
    provision: PendingProvision,
    route: spawn_activation::SpawnActivationRoute,
    observed: Result<SpawnAdmissionIo, MobError>,
}

impl MemberEffectCommit for SpawnAdmissionCommit {
    fn commit(
        self: Box<Self>,
        actor: &mut MobActor,
        settlement: MemberEffectSettlement,
    ) -> MemberEffectCommitFuture<'_> {
        Box::pin(async move {
            if !settlement.is_current() || actor.durable_uncertainty_fail_stop {
                actor.durable_uncertainty_fail_stop = true;
                return MemberEffectAck::Retained(MemberEffectRetention::resumable(
                    MobError::Internal("spawn admission observation lost its exact owner".into()),
                    self,
                ));
            }
            let Self {
                ctx,
                provision,
                route,
                observed,
            } = *self;
            boxed_arm_future(|| actor.finish_spawn_admission(ctx, provision, route, observed))
                .await;
            MemberEffectAck::Settled
        })
    }
}

impl MobActor {
    pub(super) async fn observe_spawn_admission(
        &mut self,
        ctx: Box<SpawnFinalizeCtx>,
        provision: PendingProvision,
        route: spawn_activation::SpawnActivationRoute,
    ) {
        let members = vec![self.member_fence_or_absent(&ctx.agent_identity).await];
        let service = Arc::clone(&self.session_service);
        let provisioner = Arc::clone(&self.provisioner);
        let trust_installer = self.supervisor_trust_installer();
        self.dispatch_member_effect(MemberEffectRequest {
            context: "spawn_admission_endpoint_observation",
            members,
            effects: Box::pin(async move {
                let observed = std::panic::AssertUnwindSafe(async {
                    let member = provision.member_ref()?.clone();
                    let session_comms = match member.bridge_session_id() {
                        Some(session) if ctx.remote.is_none() => {
                            service.comms_runtime(session).await
                        }
                        _ => None,
                    };
                    let provisioner_comms = if ctx.remote.is_none() {
                        provisioner.comms_runtime(&member).await
                    } else {
                        None
                    };
                    let supervisor_trust =
                        match (member.bridge_session_id(), provisioner_comms.as_ref()) {
                            (Some(session_id), Some(comms))
                                if !ctx.agent_identity.is_flow_member_namespace() =>
                            {
                                tracing::debug!(
                                    agent_identity = %ctx.agent_identity,
                                    session_id = %session_id,
                                    "spawn admission installing supervisor private trust"
                                );
                                match trust_installer
                                    .install_supervisor_private_trust_for_session(
                                        session_id, comms, None,
                                    )
                                    .await
                                {
                                    Ok(install) => {
                                        tracing::debug!(
                                            agent_identity = %ctx.agent_identity,
                                            session_id = %session_id,
                                            "spawn admission installed supervisor private trust"
                                        );
                                        SpawnSupervisorTrust::Installed {
                                            session_id: session_id.clone(),
                                            comms: Arc::clone(comms),
                                            install,
                                        }
                                    }
                                    Err(error) => SpawnSupervisorTrust::Failed(error),
                                }
                            }
                            _ => SpawnSupervisorTrust::NotApplicable,
                        };
                    Ok(SpawnAdmissionIo {
                        session_comms,
                        provisioner_comms,
                        supervisor_trust,
                    })
                })
                .catch_unwind()
                .await
                .unwrap_or_else(|payload| {
                    Err(MobError::Internal(format!(
                        "spawn endpoint observation panicked: {}",
                        super::super::panic_capture::panic_payload_detail(payload.as_ref()),
                    )))
                });
                Box::new(SpawnAdmissionCommit {
                    ctx,
                    provision,
                    route,
                    observed,
                }) as Box<dyn MemberEffectCommit>
            }),
            unsettled_commit: Box::new(SpawnAdmissionOwnerLost),
        });
    }
}

struct SpawnAdmissionOwnerLost;

impl MemberEffectCommit for SpawnAdmissionOwnerLost {
    fn commit(
        self: Box<Self>,
        actor: &mut MobActor,
        _settlement: MemberEffectSettlement,
    ) -> MemberEffectCommitFuture<'_> {
        Box::pin(async move {
            actor.durable_uncertainty_fail_stop = true;
            MemberEffectAck::Retained(MemberEffectRetention::unresumable(MobError::Internal(
                "spawn admission observation owner disappeared".into(),
            )))
        })
    }
}
