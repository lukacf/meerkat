use super::member_effect_lane::{
    MemberEffectAck, MemberEffectCommit, MemberEffectCommitFuture, MemberEffectRequest,
    MemberEffectRetention, MemberEffectSettlement,
};
use super::*;

pub(super) struct SpawnAdmissionIo {
    pub(super) session_comms: Option<Arc<dyn CoreCommsRuntime>>,
    pub(super) provisioner_comms: Option<Arc<dyn CoreCommsRuntime>>,
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
                    Ok(SpawnAdmissionIo {
                        session_comms,
                        provisioner_comms,
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
