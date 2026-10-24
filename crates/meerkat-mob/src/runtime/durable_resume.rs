//! A host's check before a durable worker's interrupted run resumes.
//!
//! A durable session that stopped inside a run resumes that run from its
//! last committed round when it is opened again. For a durable worker (a
//! member spawned with [`SpawnContinuityIntent::DurableIdentity`]) the work
//! the run was doing may have moved on meanwhile: another worker may hold its
//! work item now. A host that tracks such work installs a
//! [`DurableResumeGate`] with `MobBuilder::with_durable_resume_gate`, and
//! the runtime asks it before it resumes a durable worker's run. Every other
//! member's run resumes without asking.
//!
//! The mob binds the gate on the exact runtime registration of each durable
//! worker it provisions itself (at spawn, cold restore, explicit resume and
//! warm revival), before the worker's actor exists, as the runtime's
//! [`ResumedRunAdmission`] for that registration. The gate is an in-process
//! callback of the mob's own host: a durable worker placed on and
//! materialized by another host resumes its runs there without asking it.

use std::sync::Arc;

use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::service::CreateSessionRequest;
use meerkat_core::types::SessionId;
use meerkat_runtime::{
    MeerkatMachine, ResumedRunAdmission, ResumedRunAdmissionDecision, ResumedRunAdmissionRequest,
    RuntimeSessionRegistrationWitness,
};

use super::handle::SpawnContinuityIntent;
use crate::{AgentIdentity, MobError, MobId, ProfileName};

/// What the runtime asks a [`DurableResumeGate`] before a durable worker's
/// interrupted run resumes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DurableResumeRequest {
    /// The worker's mob.
    pub mob_id: MobId,
    /// The worker's identity in its mob. A host that keys work by owner
    /// lowers it with [`crate::lower_agent_identity_owner_key`].
    pub agent_identity: AgentIdentity,
    /// The profile the worker's session is registered under.
    pub profile: ProfileName,
    /// The worker's session.
    pub session_id: SessionId,
    /// The interrupted run, under its original id.
    pub run_id: RunId,
    /// Every input bound to the run as of its last committed round.
    pub bound_input_ids: Vec<InputId>,
    /// The WorkGraph attention projection the run executes under, when its
    /// inputs carry one and the runtime could read it. `None` otherwise: a
    /// run started without attention, one whose overlay the runtime did not
    /// supply, or one whose projection could not be decoded (then
    /// [`Self::attention_projection_unreadable`] says why).
    pub attention_projection: Option<meerkat::AttentionContextProjection>,
    /// Why the run's attention projection could not be decoded, when its
    /// turn tool overlay carries one that does not decode. A gate that
    /// decides from the projection must not treat such a run as one without
    /// attention: it answers [`DurableResumeVerdict::Unavailable`] (or
    /// `Superseded`) when this is set.
    pub attention_projection_unreadable: Option<String>,
}

impl DurableResumeRequest {
    #[must_use]
    pub fn new(
        mob_id: MobId,
        agent_identity: AgentIdentity,
        profile: ProfileName,
        session_id: SessionId,
        run_id: RunId,
        bound_input_ids: Vec<InputId>,
    ) -> Self {
        Self {
            mob_id,
            agent_identity,
            profile,
            session_id,
            run_id,
            bound_input_ids,
            attention_projection: None,
            attention_projection_unreadable: None,
        }
    }

    /// The same request with the run's attention projection.
    #[must_use]
    pub fn with_attention_projection(
        mut self,
        projection: Option<meerkat::AttentionContextProjection>,
    ) -> Self {
        self.attention_projection = projection;
        self
    }

    /// The same request with why the run's attention projection could not
    /// be decoded.
    #[must_use]
    pub fn with_attention_projection_unreadable(mut self, reason: Option<String>) -> Self {
        self.attention_projection_unreadable = reason;
        self
    }
}

/// A [`DurableResumeGate`]'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DurableResumeVerdict {
    /// Resume the run.
    Continue,
    /// The worker's work moved on. The run is settled at once, the way a
    /// stop settles it: its bound inputs are abandoned as refused resumes,
    /// the run ends failed, and it is never resumed.
    Superseded {
        /// Why, for the run's terminal record.
        reason: String,
    },
    /// The host cannot decide now, for example because the record it reads
    /// is unreachable. The runtime asks again, up to
    /// [`meerkat_runtime::RESUMED_RUN_ADMISSION_ATTEMPTS`] times, and then
    /// settles the run as it settles a superseded one. A gate that cannot
    /// decide never lets the run resume.
    Unavailable {
        /// Why, for the run's terminal record when the attempts run out.
        reason: String,
    },
}

/// A host's check before a durable worker's interrupted run resumes,
/// installed with `MobBuilder::with_durable_resume_gate`.
///
/// The runtime asks it before it resumes a durable worker's run, after the
/// worker's session is attached again and before the run is authorized, and
/// never for any other member. It asks once per attempt, up to
/// [`meerkat_runtime::RESUMED_RUN_ADMISSION_ATTEMPTS`] attempts for one
/// resume. An attempt that takes longer than
/// [`meerkat_runtime::RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT`] is cancelled
/// (its future is dropped) and counts as [`DurableResumeVerdict::Unavailable`],
/// so an implementation must be cancellation-safe and idempotent across the
/// attempts of one resume. It answers from its own records and maps its own
/// failures to `Unavailable`, never to `Continue`.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait DurableResumeGate: Send + Sync {
    async fn check(&self, request: &DurableResumeRequest) -> DurableResumeVerdict;
}

/// One durable worker's admission: the mob's gate, asked with the worker's
/// own facts. It is bound on the worker's registration, so every request
/// the runtime makes of it is for the worker's own session.
struct DurableWorkerResumeAdmission {
    gate: Arc<dyn DurableResumeGate>,
    mob_id: MobId,
    agent_identity: AgentIdentity,
    profile: ProfileName,
}

impl DurableWorkerResumeAdmission {
    /// The gate's request for the runtime's `request`.
    fn gate_request(&self, request: &ResumedRunAdmissionRequest) -> DurableResumeRequest {
        // A projection that does not decode reaches the host as unreadable,
        // with the reason, never as absent: the host tells a corrupt record
        // from a run without attention.
        let (attention_projection, attention_projection_unreadable) =
            match meerkat::workgraph_attention_projection_from_overlay(
                request.turn_tool_overlay.as_ref(),
            ) {
                Ok(projection) => (projection, None),
                Err(error) => {
                    tracing::warn!(
                        session_id = %request.session_id,
                        run_id = %request.run_id,
                        %error,
                        "a durable worker's resumed run carries an unreadable WorkGraph attention projection"
                    );
                    (None, Some(error.to_string()))
                }
            };
        DurableResumeRequest::new(
            self.mob_id.clone(),
            self.agent_identity.clone(),
            self.profile.clone(),
            request.session_id.clone(),
            request.run_id.clone(),
            request.bound_input_ids.clone(),
        )
        .with_attention_projection(attention_projection)
        .with_attention_projection_unreadable(attention_projection_unreadable)
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl ResumedRunAdmission for DurableWorkerResumeAdmission {
    async fn admit_resumed_run(
        &self,
        request: &ResumedRunAdmissionRequest,
    ) -> ResumedRunAdmissionDecision {
        match self.gate.check(&self.gate_request(request)).await {
            DurableResumeVerdict::Continue => ResumedRunAdmissionDecision::Continue,
            DurableResumeVerdict::Superseded { reason } => {
                ResumedRunAdmissionDecision::Superseded { detail: reason }
            }
            DurableResumeVerdict::Unavailable { reason } => {
                ResumedRunAdmissionDecision::Unavailable { detail: reason }
            }
        }
    }
}

/// A mob's durable resume gate, as its provisioning binds it: one admission
/// per durable worker, on the worker's exact registration.
#[derive(Clone)]
pub(super) struct DurableResumeBinding {
    gate: Arc<dyn DurableResumeGate>,
}

impl DurableResumeBinding {
    pub(super) fn new(gate: Arc<dyn DurableResumeGate>) -> Self {
        Self { gate }
    }

    /// The bind one provision owes its member: the gate's admission for a
    /// member whose incarnation is a durable identity, to bind on the
    /// member's registration with `machine`. `None` for every other member.
    /// A durable worker's create request without its member binding is
    /// refused: the worker is never built without its gate.
    pub(super) fn pending_bind(
        &self,
        machine: &Arc<MeerkatMachine>,
        continuity_intent: &SpawnContinuityIntent,
        create_session: &CreateSessionRequest,
    ) -> Result<Option<PendingDurableResumeBind>, MobError> {
        if !matches!(
            continuity_intent,
            SpawnContinuityIntent::DurableIdentity { .. }
        ) {
            return Ok(None);
        }
        let member = create_session
            .build
            .as_ref()
            .and_then(|build| build.mob_member_binding.as_ref())
            .ok_or_else(|| {
                MobError::Internal(
                    "a durable worker's provision carries no member binding for its resume gate"
                        .into(),
                )
            })?;
        let admission: Arc<dyn ResumedRunAdmission> = Arc::new(DurableWorkerResumeAdmission {
            gate: Arc::clone(&self.gate),
            mob_id: MobId::from(member.mob_id.as_str()),
            agent_identity: AgentIdentity::from(member.member.as_str()),
            profile: ProfileName::from(member.role.as_str()),
        });
        Ok(Some(PendingDurableResumeBind {
            machine: Arc::clone(machine),
            admission,
        }))
    }
}

/// One durable worker's admission, to bind on its registration before its
/// actor is created.
pub(super) struct PendingDurableResumeBind {
    machine: Arc<MeerkatMachine>,
    admission: Arc<dyn ResumedRunAdmission>,
}

impl PendingDurableResumeBind {
    /// Bind the worker's admission on `registration`, the worker's exact
    /// registration. A registration holds one admission for its lifetime,
    /// so one bound there already stays: a later provision on the same
    /// registration (a revival that keeps it, or a restore of the mob in
    /// this process that adopts it) bound it for the same worker. The
    /// transactions of one registration are serialized, so no other
    /// provision binds on it meanwhile.
    pub(super) async fn bind_on(
        self,
        registration: &RuntimeSessionRegistrationWitness,
    ) -> Result<(), MobError> {
        let unbindable = |error| {
            MobError::WiringError(format!(
                "the durable worker's resume gate could not be bound on its registration: {error}"
            ))
        };
        if self
            .machine
            .resumed_run_admission_bound(registration)
            .await
            .map_err(unbindable)?
        {
            return Ok(());
        }
        self.machine
            .bind_resumed_run_admission(registration, self.admission)
            .await
            .map_err(unbindable)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Answers with a fixed verdict and records every request it was asked.
    struct RecordingGate {
        verdict: DurableResumeVerdict,
        asked: std::sync::Mutex<Vec<DurableResumeRequest>>,
    }

    impl RecordingGate {
        fn new(verdict: DurableResumeVerdict) -> Arc<Self> {
            Arc::new(Self {
                verdict,
                asked: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn asked(&self) -> Vec<DurableResumeRequest> {
            self.asked.lock().unwrap().clone()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl DurableResumeGate for RecordingGate {
        async fn check(&self, request: &DurableResumeRequest) -> DurableResumeVerdict {
            self.asked.lock().unwrap().push(request.clone());
            self.verdict.clone()
        }
    }

    fn admission(gate: Arc<RecordingGate>) -> DurableWorkerResumeAdmission {
        DurableWorkerResumeAdmission {
            gate,
            mob_id: MobId::from("durable-mob"),
            agent_identity: AgentIdentity::from("worker-1"),
            profile: ProfileName::from("worker"),
        }
    }

    fn runtime_request(session_id: &SessionId) -> ResumedRunAdmissionRequest {
        ResumedRunAdmissionRequest::new(
            meerkat_runtime::LogicalRuntimeId::for_session(session_id),
            session_id.clone(),
            RunId::new(),
            vec![InputId::new()],
        )
    }

    fn projection() -> meerkat::AttentionContextProjection {
        meerkat::AttentionContextProjection {
            binding_id: meerkat::WorkAttentionBindingId::new("binding-1").expect("binding id"),
            work_ref: meerkat::WorkItemRef {
                realm_id: "realm-a".to_string(),
                namespace: meerkat::WorkNamespace::new("ns-1").expect("namespace"),
                item_id: meerkat::WorkItemId::new("work-1").expect("work item id"),
            },
            mode: meerkat::WorkAttentionMode::Pursue,
            binding_revision: 3,
            item_revision: 4,
            parent_refs: Vec::new(),
            parent_context: Vec::new(),
            evidence_refs: Vec::new(),
            authority: meerkat::ProjectedAttentionAuthority {
                can_get: true,
                can_add_evidence: true,
                can_release: true,
                can_update: true,
                can_block: false,
                can_create: false,
                can_link: false,
                can_link_parent: false,
                can_link_related: false,
                can_link_derived_from: false,
                can_close_own_review_item: false,
                can_close_if_policy_allows: false,
            },
            text: meerkat::AttentionProjectionText {
                title: "Work".to_string(),
                rendered: "Work".to_string(),
                truncated: false,
            },
        }
    }

    /// Each verdict maps onto the runtime's decision of the same meaning,
    /// and the gate is asked with the worker's own facts and the run's.
    #[tokio::test]
    async fn the_gate_verdict_maps_onto_the_runtime_decision() {
        let session_id = SessionId::new();
        for (verdict, expected) in [
            (
                DurableResumeVerdict::Continue,
                ResumedRunAdmissionDecision::Continue,
            ),
            (
                DurableResumeVerdict::Superseded {
                    reason: "the item moved to worker-2".to_string(),
                },
                ResumedRunAdmissionDecision::Superseded {
                    detail: "the item moved to worker-2".to_string(),
                },
            ),
            (
                DurableResumeVerdict::Unavailable {
                    reason: "the work graph is unreachable".to_string(),
                },
                ResumedRunAdmissionDecision::Unavailable {
                    detail: "the work graph is unreachable".to_string(),
                },
            ),
        ] {
            let gate = RecordingGate::new(verdict);
            let request = runtime_request(&session_id);
            let decision = admission(Arc::clone(&gate))
                .admit_resumed_run(&request)
                .await;
            assert_eq!(decision, expected);
            let asked = gate.asked();
            assert_eq!(asked.len(), 1);
            let asked = &asked[0];
            assert_eq!(asked.mob_id, MobId::from("durable-mob"));
            assert_eq!(asked.agent_identity, AgentIdentity::from("worker-1"));
            assert_eq!(asked.profile, ProfileName::from("worker"));
            assert_eq!(asked.session_id, request.session_id);
            assert_eq!(asked.run_id, request.run_id);
            assert_eq!(asked.bound_input_ids, request.bound_input_ids);
            assert_eq!(asked.attention_projection, None);
            assert_eq!(asked.attention_projection_unreadable, None);
        }
    }

    /// The run's attention projection reaches the gate when the runtime
    /// supplies the overlay that carries it; an undecodable one reaches it
    /// as unreadable, with the reason, not as absent.
    #[tokio::test]
    async fn the_gate_sees_the_attention_projection_the_run_carries() {
        let session_id = SessionId::new();
        let gate = RecordingGate::new(DurableResumeVerdict::Continue);
        let overlay =
            meerkat::WorkGraphToolSurface::turn_overlay_for_attention_projection(&projection())
                .expect("attention overlay");
        let request = runtime_request(&session_id).with_turn_tool_overlay(Some(overlay));
        admission(Arc::clone(&gate))
            .admit_resumed_run(&request)
            .await;
        assert_eq!(gate.asked()[0].attention_projection, Some(projection()));
        assert_eq!(gate.asked()[0].attention_projection_unreadable, None);

        let gate = RecordingGate::new(DurableResumeVerdict::Continue);
        let mut malformed = meerkat_core::TurnToolOverlay::default();
        malformed.dispatch_context.insert(
            meerkat::WORKGRAPH_ATTENTION_DISPATCH_CONTEXT_KEY.to_string(),
            serde_json::json!({ "not": "a projection" }),
        );
        let request = runtime_request(&session_id).with_turn_tool_overlay(Some(malformed));
        let decision = admission(Arc::clone(&gate))
            .admit_resumed_run(&request)
            .await;
        assert_eq!(decision, ResumedRunAdmissionDecision::Continue);
        let asked = &gate.asked()[0];
        assert_eq!(asked.attention_projection, None);
        assert!(
            asked
                .attention_projection_unreadable
                .as_deref()
                .is_some_and(|reason| reason.contains("attention projection")),
            "{:?}",
            asked.attention_projection_unreadable
        );
    }

    fn create_session_request(
        member: Option<meerkat_core::MobMemberBinding>,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "mock-model".to_string(),
            prompt: meerkat_core::types::ContentInput::Text("work".to_string()),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions {
                mob_member_binding: member,
                ..Default::default()
            }),
            labels: None,
        }
    }

    fn member_binding() -> meerkat_core::MobMemberBinding {
        meerkat_core::MobMemberBinding {
            mob_id: "durable-mob".to_string(),
            role: "worker".to_string(),
            member: "worker-1".to_string(),
        }
    }

    /// Only a durable identity owes a bind, with the facts of its member
    /// binding; a durable worker without one is refused.
    #[tokio::test]
    async fn only_a_durable_identity_owes_a_resume_gate_bind() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let binding = DurableResumeBinding::new(RecordingGate::new(DurableResumeVerdict::Continue));
        let durable = SpawnContinuityIntent::DurableIdentity {
            continuity_key: "worker-1".to_string(),
        };

        assert!(
            binding
                .pending_bind(
                    &machine,
                    &SpawnContinuityIntent::Ephemeral,
                    &create_session_request(Some(member_binding())),
                )
                .expect("an ordinary member owes nothing")
                .is_none()
        );
        assert!(
            binding
                .pending_bind(
                    &machine,
                    &durable,
                    &create_session_request(Some(member_binding()))
                )
                .expect("a durable worker owes a bind")
                .is_some()
        );
        let error = binding
            .pending_bind(&machine, &durable, &create_session_request(None))
            .err()
            .expect("a durable worker without its member binding is refused");
        assert!(matches!(error, MobError::Internal(_)), "{error:?}");
    }
}
