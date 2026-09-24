//! Wave-c C-6c — consumer side of the `meerkat_mob_seam` composition.
//!
//! Wave-c C-6p landed the producer side: the mob actor converts each
//! emitted `MobMachineEffect::Request*` variant into a typed
//! [`MobSeamEffect`][mse] and routes it through a
//! [`CompositionDispatcher`][cd]. Until this module lands, that
//! dispatcher resolves the typed route but fails with
//! [`DispatchRefusal::UnwiredConsumer`][dr] because no
//! [`ConsumerSurface`][cs] is registered for the `meerkat` target
//! instance.
//!
//! C-6c closes that seam: [`MeerkatConsumerSurface`] is the typed
//! consumer surface. The dispatcher invokes
//! [`ConsumerSurface::apply_routed_input`] with the typed
//! [`InputVariantId`] + projected field bindings declared by the
//! `meerkat_mob_seam` composition schema
//! (`meerkat-machine-schema/src/catalog/compositions.rs::meerkat_mob_seam_composition`).
//! This surface translates each of the four routed variants —
//! `PrepareBindings`, `Ingest`, `Retire`, `Destroy` — into the
//! corresponding `MeerkatMachineInput` and applies it against the
//! session's shared DSL authority.
//!
//! The route bindings declared in the schema are the sole source of
//! truth for field projection shape. If a route binding references a
//! producer field the effect body did not populate, the dispatcher
//! returns [`DispatchRefusal::MissingProducerField`][dr] before this
//! surface is reached, so [`apply_routed_input`][cs_apply] can assume
//! the declared bindings are present.
//!
//! [mse]: meerkat_mob::runtime::composition::MobSeamEffect
//! [cd]: crate::composition::CompositionDispatcher
//! [cs]: crate::composition::ConsumerSurface
//! [cs_apply]: crate::composition::ConsumerSurface::apply_routed_input
//! [dr]: crate::composition::DispatchRefusal

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use meerkat_core::types::SessionId;
use meerkat_machine_schema::identity::{
    EffectVariantId, FieldId, InputVariantId, MachineInstanceId,
};

use crate::composition::{
    CompositionSignalDispatcher, FieldValue, ProducerInstance, ProducerSignal, SignalPayload,
};
use crate::composition::{ConsumerSurface, OwnedFieldValue, SignalDispatchOutcome};
use crate::generated::meerkat_mob_seam as seam_facts;
use crate::meerkat_machine::{MeerkatMachine, dsl as mm_dsl};

/// Consumer-side surface for the `meerkat_mob_seam` composition.
///
/// Implements [`ConsumerSurface`] for the `meerkat` target instance. The
/// dispatcher hands the surface one routed input at a time; the surface
/// translates the typed [`InputVariantId`] + projected-field tuple into
/// the matching [`MeerkatMachineInput`][mmi] and applies it against the
/// session's shared DSL authority on the owning [`MeerkatMachine`].
///
/// Session selection: every route the schema declares into this surface
/// binds `session_id`, so the projected value is the only session source.
/// An unpinned surface refuses a routed input that projects none instead of
/// inferring the session from another projected field.
///
/// Runtime epoch: the seam projects MobMachine-owned facts only, and the
/// entry runtime epoch is a meerkat-owned REGISTRATION fact, so no routed
/// variant carries it and MobMachine must never learn it. The variants that
/// declare the field are built here with `None` and resolved against the
/// target session entry inside
/// `MeerkatMachine::apply_routed_session_dsl_input`, under the same guard
/// that resolved that entry. Resolving it in this surface instead would be
/// either a peek that goes stale before the apply, or a self-deadlock on the
/// non-reentrant per-session mutation gate the routed dispatch already
/// holds.
///
/// What that means for stale writers on THIS lane: because the epoch is
/// self-supplied from the entry, the machine's exact-epoch guard compares the
/// entry against itself and cannot fence a stale mob-originated placement. The
/// fence here is the placement triple plus the fence token - `session_id`,
/// `agent_runtime_id`, `generation` and `fence_token` all come from the
/// producer and are all matched by the machine. The epoch guard fences
/// callers that STATE an epoch (every direct, non-routed producer does), which
/// is why `resolve_routed_entry_runtime_epoch` fills only an absent value and
/// a stated-but-stale epoch is still refused.
///
/// [mmi]: crate::meerkat_machine::dsl::MeerkatMachineInput
pub struct MeerkatConsumerSurface {
    machine: Arc<MeerkatMachine>,
    /// Optional pinned session id. `None` means every routed input must
    /// project its own `session_id`; `Some(id)` means every routed input is
    /// applied against that session and the surface refuses variants whose
    /// projected `session_id` disagrees.
    pinned_session: Option<SessionId>,
}

/// Producer-side signal source sum for MeerkatMachine lifecycle effects
/// routed through the `meerkat_mob_seam` signal surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeerkatSeamSignal {
    RuntimeBound {
        agent_runtime_id: mm_dsl::AgentRuntimeId,
        fence_token: mm_dsl::FenceToken,
    },
    RuntimeRetired {
        agent_runtime_id: mm_dsl::AgentRuntimeId,
        fence_token: mm_dsl::FenceToken,
    },
    RuntimeDestroyed {
        agent_runtime_id: mm_dsl::AgentRuntimeId,
        fence_token: mm_dsl::FenceToken,
    },
}

impl MeerkatSeamSignal {
    pub fn variant_id(&self) -> EffectVariantId {
        match self {
            Self::RuntimeBound { .. } => seam_facts::effects::meerkat::runtime_bound(),
            Self::RuntimeRetired { .. } => seam_facts::effects::meerkat::runtime_retired(),
            Self::RuntimeDestroyed { .. } => seam_facts::effects::meerkat::runtime_destroyed(),
        }
    }

    pub fn generated_signal_route(&self) -> Option<seam_facts::TypedRoutedSignal> {
        seam_facts::route_to_signal(
            &seam_facts::producers::meerkat_instance_id(),
            &self.variant_id(),
        )
    }

    fn field(&self, id: &FieldId) -> Option<FieldValue<'_>> {
        let (agent_runtime_id, fence_token) = match self {
            Self::RuntimeBound {
                agent_runtime_id,
                fence_token,
            }
            | Self::RuntimeRetired {
                agent_runtime_id,
                fence_token,
            }
            | Self::RuntimeDestroyed {
                agent_runtime_id,
                fence_token,
            } => (agent_runtime_id, fence_token),
        };
        if id == &seam_facts::fields::agent_runtime_id() {
            Some(FieldValue::Str(agent_runtime_id.0.as_str()))
        } else if id == &seam_facts::fields::fence_token() {
            Some(FieldValue::U64(fence_token.0))
        } else {
            None
        }
    }
}

impl ProducerSignal for MeerkatSeamSignal {
    fn variant_id(&self) -> EffectVariantId {
        self.variant_id()
    }

    fn field(&self, id: &FieldId) -> Option<FieldValue<'_>> {
        self.field(id)
    }
}

pub type MeerkatCompositionSignalDispatcher =
    Arc<dyn CompositionSignalDispatcher<Signal = MeerkatSeamSignal>>;

pub fn meerkat_producer_instance() -> ProducerInstance {
    let producer = seam_facts::producers::meerkat();
    ProducerInstance {
        composition: seam_facts::composition_id(),
        instance_id: producer.instance_id,
        machine: producer.machine,
    }
}

pub fn lift_routed_signal(effect: &mm_dsl::MeerkatMachineEffect) -> Option<MeerkatSeamSignal> {
    match effect {
        mm_dsl::MeerkatMachineEffect::RuntimeBound {
            agent_runtime_id,
            fence_token,
        } => Some(MeerkatSeamSignal::RuntimeBound {
            agent_runtime_id: agent_runtime_id.clone(),
            fence_token: *fence_token,
        }),
        mm_dsl::MeerkatMachineEffect::RuntimeRetired {
            agent_runtime_id,
            fence_token,
        } => Some(MeerkatSeamSignal::RuntimeRetired {
            agent_runtime_id: agent_runtime_id.clone(),
            fence_token: *fence_token,
        }),
        mm_dsl::MeerkatMachineEffect::RuntimeDestroyed {
            agent_runtime_id,
            fence_token,
        } => Some(MeerkatSeamSignal::RuntimeDestroyed {
            agent_runtime_id: agent_runtime_id.clone(),
            fence_token: *fence_token,
        }),
        _ => None,
    }
}

pub async fn dispatch_routed_signal(
    dispatcher: &MeerkatCompositionSignalDispatcher,
    signal: MeerkatSeamSignal,
) -> Result<SignalDispatchOutcome, String> {
    let variant = signal.variant_id();
    dispatcher
        .dispatch_signal(
            meerkat_producer_instance(),
            SignalPayload::Emitted {
                variant,
                body: signal,
            },
        )
        .await
        .map_err(|refusal| refusal.to_string())
}

impl std::fmt::Debug for MeerkatConsumerSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeerkatConsumerSurface")
            .field("pinned_session", &self.pinned_session)
            .finish_non_exhaustive()
    }
}

impl MeerkatConsumerSurface {
    /// Build a consumer surface backed by the given machine. The surface
    /// resolves each routed input's target session from projected fields.
    pub fn new(machine: Arc<MeerkatMachine>) -> Self {
        Self {
            machine,
            pinned_session: None,
        }
    }

    /// Build a consumer surface pinned to `session_id`. All routed
    /// inputs are applied against this session; variants that carry a
    /// `session_id` are additionally checked for agreement and refused on
    /// mismatch.
    pub fn pinned(machine: Arc<MeerkatMachine>, session_id: SessionId) -> Self {
        Self {
            machine,
            pinned_session: Some(session_id),
        }
    }

    async fn resolve_session(
        &self,
        _variant: &InputVariantId,
        projected: &[(FieldId, OwnedFieldValue)],
    ) -> Result<SessionId, String> {
        // Typed session_id is the canonical source (Shape 4 — producer DSL
        // emits `session_id: SessionId` alongside `agent_runtime_id`; see
        // `MobMachineEffect::RequestRuntimeBinding` in
        // `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs:168`).
        let projected_session_id = projected
            .iter()
            .find(|(id, _)| id == &seam_facts::fields::session_id())
            .and_then(|(_, v)| match v {
                OwnedFieldValue::Str(s) => Some(s.clone()),
                _ => None,
            });

        match (&self.pinned_session, projected_session_id) {
            (Some(pinned), Some(sid)) if sid != pinned.to_string() => Err(format!(
                "routed session_id `{sid}` does not match pinned session `{pinned}`"
            )),
            (Some(pinned), _) => Ok(pinned.clone()),
            (None, Some(sid)) => SessionId::parse(&sid)
                .map_err(|e| format!("routed session_id `{sid}` is not a valid UUID: {e}")),
            (None, None) => Err(
                "routed input did not project `session_id` and surface is not pinned \
                 to a session — no session can be resolved"
                    .into(),
            ),
        }
    }
}

#[allow(clippy::panic)]
fn meerkat_instance_id() -> &'static MachineInstanceId {
    static ID: OnceLock<MachineInstanceId> = OnceLock::new();
    ID.get_or_init(seam_facts::producers::meerkat_instance_id)
}

fn project_u64(fields: &[(FieldId, OwnedFieldValue)], field: &FieldId) -> Result<u64, String> {
    fields
        .iter()
        .find(|(id, _)| id == field)
        .ok_or_else(|| format!("missing projected field `{}`", field.as_str()))
        .and_then(|(_, v)| match v {
            OwnedFieldValue::U64(n) => Ok(*n),
            other => Err(format!(
                "projected field `{}` is not U64: {other:?}",
                field.as_str()
            )),
        })
}

fn project_optional_u64(
    fields: &[(FieldId, OwnedFieldValue)],
    field: &FieldId,
) -> Result<Option<u64>, String> {
    match fields.iter().find(|(id, _)| id == field) {
        None => Ok(None),
        Some((_, OwnedFieldValue::U64(n))) => Ok(Some(*n)),
        Some((_, other)) => Err(format!(
            "projected field `{}` is not U64: {other:?}",
            field.as_str()
        )),
    }
}

fn project_str<'a>(
    fields: &'a [(FieldId, OwnedFieldValue)],
    field: &FieldId,
) -> Result<&'a str, String> {
    fields
        .iter()
        .find(|(id, _)| id == field)
        .ok_or_else(|| format!("missing projected field `{}`", field.as_str()))
        .and_then(|(_, v)| match v {
            OwnedFieldValue::Str(s) => Ok(s.as_str()),
            other => Err(format!(
                "projected field `{}` is not Str: {other:?}",
                field.as_str()
            )),
        })
}

fn project_work_origin(
    fields: &[(FieldId, OwnedFieldValue)],
    field: &FieldId,
) -> Result<mm_dsl::WorkOrigin, String> {
    fields
        .iter()
        .find(|(id, _)| id == field)
        .ok_or_else(|| format!("missing projected field `{}`", field.as_str()))
        .and_then(|(_, v)| match v {
            OwnedFieldValue::Opaque(value) => value
                .downcast_ref::<mm_dsl::WorkOrigin>()
                .copied()
                .ok_or_else(|| format!("projected field `{}` is not WorkOrigin", field.as_str())),
            other => Err(format!(
                "projected field `{}` is not WorkOrigin: {other:?}",
                field.as_str()
            )),
        })
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl ConsumerSurface for MeerkatConsumerSurface {
    fn instance_id(&self) -> &MachineInstanceId {
        meerkat_instance_id()
    }

    async fn apply_routed_input(
        &self,
        variant: InputVariantId,
        projected: Vec<(FieldId, OwnedFieldValue)>,
    ) -> Result<(), crate::composition::ConsumerError> {
        let session_id = self.resolve_session(&variant, &projected).await?;
        let input = if variant == seam_facts::inputs::prepare_bindings() {
            let rt = project_str(&projected, &seam_facts::fields::agent_runtime_id())?;
            let fence = project_u64(&projected, &seam_facts::fields::fence_token())?;
            let generation = project_u64(&projected, &seam_facts::fields::generation())?;
            let sid = project_str(&projected, &seam_facts::fields::session_id())?;
            mm_dsl::MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: mm_dsl::AgentRuntimeId::from(rt.to_string()),
                fence_token: mm_dsl::FenceToken(fence),
                generation: Some(mm_dsl::Generation(generation)),
                // No seam field projects the entry epoch (see the type doc):
                // `None` asks the routed apply to resolve the registered
                // value this placement must assert. It never means "unbind
                // the epoch".
                runtime_epoch_id: None,
                session_id: mm_dsl::SessionId::from(sid.to_string()),
            }
        } else if variant == seam_facts::inputs::ingest() {
            // Route binding `work_request_reaches_meerkat` delivers
            // producer `agent_runtime_id` into the consumer's canonical
            // `runtime_id` field and also carries the MobMachine-owned
            // binding facts that MeerkatMachine validates before admission.
            let sid = project_str(&projected, &seam_facts::fields::session_id())?;
            let rt = project_str(&projected, &seam_facts::fields::runtime_id())?;
            let fence = project_u64(&projected, &seam_facts::fields::fence_token())?;
            let generation = project_optional_u64(&projected, &seam_facts::fields::generation())?;
            let work_id = project_str(&projected, &seam_facts::fields::work_id())?;
            let origin = project_work_origin(&projected, &seam_facts::fields::origin())?;
            mm_dsl::MeerkatMachineInput::Ingest {
                session_id: mm_dsl::SessionId::from(sid.to_string()),
                runtime_id: mm_dsl::AgentRuntimeId::from(rt.to_string()),
                fence_token: mm_dsl::FenceToken(fence),
                generation: generation.map(mm_dsl::Generation),
                // Same as PrepareBindings above: resolved from the entry by
                // the routed apply, so the admission guard's exact epoch
                // match compares two truthful values.
                runtime_epoch_id: None,
                work_id: mm_dsl::WorkId::from(work_id.to_string()),
                origin,
            }
        } else if variant == seam_facts::inputs::retire() {
            let sid = project_str(&projected, &seam_facts::fields::session_id())?;
            mm_dsl::MeerkatMachineInput::Retire {
                session_id: mm_dsl::SessionId::from(sid.to_string()),
            }
        } else if variant == seam_facts::inputs::destroy() {
            let sid = project_str(&projected, &seam_facts::fields::session_id())?;
            mm_dsl::MeerkatMachineInput::Destroy {
                session_id: mm_dsl::SessionId::from(sid.to_string()),
            }
        } else {
            return Err(crate::composition::ConsumerError::new(
                "meerkat_consumer_surface_unsupported_input",
                format!(
                    "meerkat consumer surface does not accept routed input `{}`; \
                     only PrepareBindings/Ingest/Retire/Destroy are declared in the \
                     `meerkat_mob_seam` schema",
                    variant.as_str()
                ),
            ));
        };

        // Kernel→consumer leg stays typed: the per-variant stable
        // discriminant minted by the generated-machine refusal (e.g.
        // `dsl_guard_rejected`) crosses the seam verbatim instead of being
        // collapsed under one generic projection code.
        self.machine
            .apply_routed_meerkat_input(&session_id, input)
            .await
            .map_err(|refusal| {
                crate::composition::ConsumerError::new(refusal.error_code, refusal.message)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composition::{
        CatalogCompositionSignalDispatcher, OwnedFieldValue, RouteTable, SignalConsumerSurface,
    };
    use crate::meerkat_machine::{
        EnsureRuntimeExecutorAttachment, LocalSessionMaterializationMode,
        PreparedSessionMaterialization, RuntimeLoopAttachmentSlot,
    };
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
    use meerkat_machine_schema::identity::SignalVariantId;

    fn fld(slug: &str) -> FieldId {
        FieldId::parse(slug).expect("field slug")
    }

    fn iv(slug: &str) -> InputVariantId {
        InputVariantId::parse(slug).expect("input variant slug")
    }

    fn sid(slug: &str) -> SessionId {
        SessionId::parse(slug).expect("session id")
    }

    struct NoopExecutor;

    #[async_trait]
    impl CoreExecutor for NoopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    struct StartupGatedNoopExecutor {
        reconciliation_started: Arc<tokio::sync::Notify>,
        allow_reconciliation: Arc<tokio::sync::Notify>,
        gate_first_reconciliation: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl CoreExecutor for StartupGatedNoopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn reconcile_committed_compaction_projections(
            &mut self,
            _intents: &[meerkat_core::CompactionProjectionIntent],
        ) -> Result<(), CoreExecutorError> {
            if self
                .gate_first_reconciliation
                .swap(false, std::sync::atomic::Ordering::AcqRel)
            {
                self.reconciliation_started.notify_one();
                self.allow_reconciliation.notified().await;
            }
            Ok(())
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    async fn begin_startup_gated_first_live_attachment(
        machine: &Arc<MeerkatMachine>,
        session_id: &SessionId,
        mut prepared: PreparedSessionMaterialization,
    ) -> (tokio::task::JoinHandle<()>, Arc<tokio::sync::Notify>) {
        let reconciliation_started = Arc::new(tokio::sync::Notify::new());
        let allow_reconciliation = Arc::new(tokio::sync::Notify::new());
        let registration = tokio::spawn({
            let reconciliation_started = Arc::clone(&reconciliation_started);
            let allow_reconciliation = Arc::clone(&allow_reconciliation);
            async move {
                let attachment = prepared
                    .ensure_executor_attachment(move |_| {
                        Box::new(StartupGatedNoopExecutor {
                            reconciliation_started,
                            allow_reconciliation,
                            gate_first_reconciliation: std::sync::atomic::AtomicBool::new(true),
                        })
                    })
                    .await
                    .expect("prepared first live attachment must reach startup reconciliation");
                let EnsureRuntimeExecutorAttachment::Pending(attachment) = attachment else {
                    panic!("missing-live materialization unexpectedly found a serving attachment");
                };
                attachment
                    .commit()
                    .await
                    .expect("first live attachment must finish after startup release");
            }
        });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reconciliation_started.notified(),
        )
        .await
        .expect("first live attachment must reach startup reconciliation");

        let attached = machine
            .session_dsl_state(session_id)
            .await
            .expect("pending first attachment authority exists");
        assert_eq!(attached.lifecycle_phase, mm_dsl::MeerkatPhase::Attached);
        assert_eq!(
            attached.registration_phase,
            mm_dsl::RegistrationPhase::Active
        );
        {
            let sessions = machine.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .expect("pending first attachment remains registered");
            assert!(
                matches!(
                    &entry.attachment_slot,
                    RuntimeLoopAttachmentSlot::Pending(_)
                ),
                "startup reconciliation must retain a pending mechanical attachment"
            );
            assert!(
                !entry.has_live_attachment(),
                "startup reconciliation must not publish a serving attachment before commit"
            );
        }
        assert!(
            machine
                .current_executor_attachment_witness(session_id)
                .await
                .is_none(),
            "a pending startup attachment must not expose a serving witness"
        );

        (registration, allow_reconciliation)
    }

    #[allow(clippy::too_many_arguments)]
    async fn route_successor_binding_and_finish_gated_attachment(
        machine: &Arc<MeerkatMachine>,
        session_id: &SessionId,
        registration: tokio::task::JoinHandle<()>,
        allow_reconciliation: Arc<tokio::sync::Notify>,
        completion: crate::completion::CompletionHandle,
        replacement_runtime_id: &'static str,
        replacement_fence_token: u64,
        replacement_generation: u64,
    ) {
        // Queue the routed input while executor startup still owns the session
        // mutation gate. Tokio's FIFO mutex then gives this already-waiting
        // route the exact Attached seam before the runtime loop can dequeue
        // the preserved backlog.
        let route_started = Arc::new(tokio::sync::Notify::new());
        let routed = tokio::spawn({
            let machine = Arc::clone(machine);
            let session_id = session_id.clone();
            let route_started = Arc::clone(&route_started);
            async move {
                route_started.notify_one();
                MeerkatConsumerSurface::pinned(Arc::clone(&machine), session_id.clone())
                    .apply_routed_input(
                        iv("PrepareBindings"),
                        vec![
                            (
                                fld("agent_runtime_id"),
                                OwnedFieldValue::Str(replacement_runtime_id.into()),
                            ),
                            (
                                fld("fence_token"),
                                OwnedFieldValue::U64(replacement_fence_token),
                            ),
                            (
                                fld("generation"),
                                OwnedFieldValue::U64(replacement_generation),
                            ),
                            (
                                fld("session_id"),
                                OwnedFieldValue::Str(session_id.to_string()),
                            ),
                        ],
                    )
                    .await
                    .expect("routed successor binding is admitted at Attached");
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), route_started.notified())
            .await
            .expect("successor binding route must start");
        tokio::task::yield_now().await;
        assert!(
            !routed.is_finished(),
            "the routed input must wait behind executor startup's mutation-gate ownership"
        );

        allow_reconciliation.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), registration)
            .await
            .expect("first live attachment registration must finish")
            .expect("first live attachment registration task must not panic");
        tokio::time::timeout(std::time::Duration::from_secs(2), routed)
            .await
            .expect("successor binding route must finish")
            .expect("successor binding route task must not panic");

        let completion_outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            completion.wait_authorized(),
        )
        .await;
        let completion_outcome = match completion_outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let snapshot = machine
                    .meerkat_machine_spine_snapshot(session_id)
                    .await
                    .expect("runtime spine exists after completion timeout");
                panic!(
                    "unassigned queued input must complete on the first live attachment after ownerless recovery: {error}; snapshot={snapshot:#?}"
                );
            }
        };
        match completion_outcome {
            crate::completion::CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("expected preserved input completion, got {other:?}"),
        }

        let rebound = machine
            .session_dsl_state(session_id)
            .await
            .expect("rebound session authority exists");
        assert_eq!(rebound.lifecycle_phase, mm_dsl::MeerkatPhase::Attached);
        assert_eq!(
            rebound.registration_phase,
            mm_dsl::RegistrationPhase::Active
        );
        assert!(
            matches!(&rebound.active_runtime_id, Some(value) if value.0 == replacement_runtime_id)
        );
        assert_eq!(
            rebound.active_fence_token,
            Some(mm_dsl::FenceToken(replacement_fence_token))
        );
        assert_eq!(
            rebound.active_runtime_generation,
            Some(mm_dsl::Generation(replacement_generation))
        );
        let entry_epoch_id = {
            let sessions = machine.sessions.read().await;
            sessions
                .get(session_id)
                .expect("rebound entry remains registered")
                .epoch_id
                .clone()
        };
        assert_eq!(
            rebound.active_runtime_epoch_id,
            Some(mm_dsl::RuntimeEpochId::from_domain(&entry_epoch_id)),
            "the composition route projects no epoch, so the routed placement \
             asserts the entry epoch the registration installed - a rebind must \
             never leave a live entry epochless (that is the state in which the \
             compaction projection coordinator can never authorize a handoff)"
        );
    }

    fn assert_input_spine_preserved(
        before: &crate::meerkat_machine_types::MeerkatInputsSnapshot,
        after: &crate::meerkat_machine_types::MeerkatInputsSnapshot,
    ) {
        assert_eq!(after.queue, before.queue);
        assert_eq!(after.steer_queue, before.steer_queue);
        assert_eq!(after.current_run_id, before.current_run_id);
        assert_eq!(
            after.current_run_contributors,
            before.current_run_contributors
        );
        assert_eq!(after.post_admission_signal, before.post_admission_signal);
        assert_eq!(
            after.silent_intent_overrides,
            before.silent_intent_overrides
        );
        assert_eq!(after.admission_order.len(), before.admission_order.len());
        for (after_input, before_input) in after.admission_order.iter().zip(&before.admission_order)
        {
            assert_eq!(after_input.input_id, before_input.input_id);
            assert_eq!(after_input.content_shape, before_input.content_shape);
            assert_eq!(after_input.request_id, before_input.request_id);
            assert_eq!(after_input.reservation_key, before_input.reservation_key);
            assert_eq!(after_input.handling_mode, before_input.handling_mode);
            assert_eq!(
                after_input.live_interrupt_required,
                before_input.live_interrupt_required
            );
            assert_eq!(after_input.lifecycle, before_input.lifecycle);
            assert_eq!(after_input.terminal_outcome, before_input.terminal_outcome);
            assert_eq!(after_input.last_run_id, before_input.last_run_id);
            assert_eq!(
                after_input.last_boundary_sequence,
                before_input.last_boundary_sequence
            );
            assert_eq!(after_input.is_prompt, before_input.is_prompt);
        }
    }

    /// The entry's registration-owned runtime epoch, as the DSL sees it.
    async fn entry_runtime_epoch_id(
        machine: &MeerkatMachine,
        session_id: &SessionId,
    ) -> mm_dsl::RuntimeEpochId {
        let sessions = machine.sessions.read().await;
        mm_dsl::RuntimeEpochId::from_domain(
            &sessions
                .get(session_id)
                .expect("session entry exists")
                .epoch_id,
        )
    }

    async fn bind_runtime(
        surface: &MeerkatConsumerSurface,
        session_id: &SessionId,
        runtime_id: &str,
    ) {
        surface
            .apply_routed_input(
                iv("PrepareBindings"),
                vec![
                    (
                        fld("agent_runtime_id"),
                        OwnedFieldValue::Str(runtime_id.into()),
                    ),
                    (fld("fence_token"), OwnedFieldValue::U64(1)),
                    (fld("generation"), OwnedFieldValue::U64(0)),
                    (
                        fld("session_id"),
                        OwnedFieldValue::Str(session_id.to_string()),
                    ),
                ],
            )
            .await
            .expect("bind runtime");
    }

    /// Regression proof for the 0.8.23 compaction-persistence wedge: a
    /// seam-routed member must end up with the entry's REAL runtime epoch on its
    /// authority, because that is the epoch the compaction projection
    /// coordinator holds as `expected`.
    ///
    /// The seam route projects only MobMachine-owned facts (runtime id, fence,
    /// generation, session id) - by design it owns no epoch. Before this change
    /// the consumer hardcoded `None` into routed `PrepareBindings`/`Ingest`, and
    /// `PrepareBindings` wrote that `None` over the entry epoch: the coordinator
    /// then compared a minted epoch against `None` forever ("runtime epoch
    /// changed (expected ..., current None)"), warn-only, so a member whose
    /// strand was already over-window became permanently unusable in silence.
    /// The epoch now belongs to registration and routed inputs resolve it from
    /// the entry, so both the routed placement and routed work admission agree
    /// with it.
    #[tokio::test]
    async fn routed_seam_binding_and_ingest_preserve_the_entry_runtime_epoch() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        let session_id = sid("00000000-0000-0000-0000-000000000042");
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let entry_epoch_id = {
            let sessions = machine.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("registered entry exists")
                .epoch_id
                .clone()
        };
        let registered_epoch = mm_dsl::RuntimeEpochId::from_domain(&entry_epoch_id);
        assert_eq!(
            machine
                .session_dsl_state(&session_id)
                .await
                .expect("registered authority exists")
                .active_runtime_epoch_id,
            Some(registered_epoch.clone()),
            "registration installs the entry epoch"
        );

        bind_runtime(&surface, &session_id, "mob-runtime").await;
        let bound = machine
            .session_dsl_state(&session_id)
            .await
            .expect("bound authority exists");
        assert!(matches!(&bound.active_runtime_id, Some(value) if value.0 == "mob-runtime"));
        assert_eq!(
            bound.active_runtime_epoch_id,
            Some(registered_epoch.clone()),
            "the routed placement must leave the registered entry epoch intact"
        );

        // Work admission takes the same lane: the routed Ingest carries no
        // epoch either, and its exact-match guard now sees the truthful value.
        surface
            .apply_routed_input(
                iv("Ingest"),
                vec![
                    (
                        fld("runtime_id"),
                        OwnedFieldValue::Str("mob-runtime".into()),
                    ),
                    (fld("fence_token"), OwnedFieldValue::U64(1)),
                    (fld("generation"), OwnedFieldValue::U64(0)),
                    (fld("work_id"), OwnedFieldValue::Str("work-1".into())),
                    (
                        fld("origin"),
                        OwnedFieldValue::Opaque(Arc::new(mm_dsl::WorkOrigin::Ingest)),
                    ),
                    (
                        fld("session_id"),
                        OwnedFieldValue::Str(session_id.to_string()),
                    ),
                ],
            )
            .await
            .expect(
                "routed work admission must be admitted for an epoch-registered \
                 seam member: epoch-Some sessions refusing routed Ingest is the \
                 other half of the mutually exclusive trap",
            );
        assert_eq!(
            machine
                .session_dsl_state(&session_id)
                .await
                .expect("authority exists after admission")
                .active_runtime_epoch_id,
            Some(registered_epoch),
            "admission must not disturb the entry epoch"
        );
    }

    /// End of the wedge, at the seam the production defect ran through: a
    /// seam-routed member with locally prepared session resources (the mob
    /// provisioning shape) authorizes its durable compaction projection once the
    /// routed placement lands. This is the acceptance half that regresses if the
    /// routed epoch resolution breaks - the coordinator holds the entry epoch as
    /// `expected`, and before the fix the routed placement wrote `None` over it,
    /// so this authorization could never succeed for any mob member with
    /// durable semantic memory.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn routed_seam_member_authorizes_its_durable_compaction_projection() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let machine = Arc::new(MeerkatMachine::persistent_without_blobs(
            store as Arc<dyn crate::store::RuntimeStore>,
        ));
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        let session_id = sid("00000000-0000-0000-0000-000000000043");
        let bindings = machine
            .prepare_local_session_bindings(session_id.clone())
            .await
            .expect("mob provisioning prepares local session resources");
        let projection: meerkat_core::CompactionProjectionId =
            serde_json::from_value(serde_json::json!({
                "session_id": session_id,
                "parent_revision": "parent",
                "revision": "revision",
                "commit_fingerprint": "sha256:routed-seam-member-fixture",
            }))
            .expect("persisted compaction projection fixture");

        assert!(
            bindings
                .compaction_commit_coordinator()
                .authorize_projection(&projection)
                .is_err(),
            "before the routed placement there is no authoritative binding to commit against"
        );

        bind_runtime(&surface, &session_id, "mob-runtime").await;

        bindings
            .compaction_commit_coordinator()
            .authorize_projection(&projection)
            .expect(
                "a seam-routed member must be able to persist compaction: the routed placement \
                 asserts the entry epoch the coordinator holds",
            );
    }

    /// The epoch resolution above fills only an ABSENT epoch. A routed input
    /// that states a stale epoch is still refused - the guard's original design
    /// intent (fencing stale-epoch writers) survives, and the fill is not a
    /// tautology against the machine's own state.
    #[tokio::test]
    async fn routed_input_stating_a_stale_runtime_epoch_is_still_refused() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let refusal = machine
            .apply_routed_meerkat_input(
                &session_id,
                mm_dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: mm_dsl::AgentRuntimeId("mob-runtime".into()),
                    fence_token: mm_dsl::FenceToken(1),
                    generation: Some(mm_dsl::Generation(0)),
                    runtime_epoch_id: Some(mm_dsl::RuntimeEpochId("stale-epoch".into())),
                    session_id: mm_dsl::SessionId(session_id.to_string()),
                },
            )
            .await
            .expect_err("a stated stale epoch must not be silently replaced");
        assert_eq!(refusal.error_code, "dsl_guard_rejected", "{refusal}");

        let state = machine
            .session_dsl_state(&session_id)
            .await
            .expect("authority remains available");
        assert_eq!(
            state.active_runtime_id, None,
            "a refused placement must not bind"
        );
        assert!(
            state.active_runtime_epoch_id.is_some(),
            "the registered entry epoch survives a refused placement"
        );
    }

    /// Same fencing on the work-admission half of the lane: `Ingest`'s exact
    /// epoch match is what keeps a writer from a dead runtime generation out,
    /// and filling only an absent epoch leaves that intact.
    #[tokio::test]
    async fn routed_ingest_stating_a_stale_runtime_epoch_is_still_refused() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");
        bind_runtime(&surface, &session_id, "mob-runtime").await;

        let refusal = machine
            .apply_routed_meerkat_input(
                &session_id,
                mm_dsl::MeerkatMachineInput::Ingest {
                    session_id: mm_dsl::SessionId(session_id.to_string()),
                    runtime_id: mm_dsl::AgentRuntimeId("mob-runtime".into()),
                    fence_token: mm_dsl::FenceToken(1),
                    generation: Some(mm_dsl::Generation(0)),
                    runtime_epoch_id: Some(mm_dsl::RuntimeEpochId("stale-epoch".into())),
                    work_id: mm_dsl::WorkId("work-1".into()),
                    origin: mm_dsl::WorkOrigin::Ingest,
                },
            )
            .await
            .expect_err("work stated against a stale epoch must not be admitted");
        assert_eq!(refusal.error_code, "dsl_guard_rejected", "{refusal}");
    }

    #[tokio::test]
    async fn prepare_bindings_requires_all_three_fields() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        let err = surface
            .apply_routed_input(
                iv("PrepareBindings"),
                vec![
                    (fld("agent_runtime_id"), OwnedFieldValue::Str("rt-1".into())),
                    // fence_token missing on purpose.
                    (fld("generation"), OwnedFieldValue::U64(3)),
                    (
                        fld("session_id"),
                        OwnedFieldValue::Str("00000000-0000-0000-0000-000000000001".into()),
                    ),
                ],
            )
            .await
            .expect_err("missing fence_token");
        assert!(err.message().contains("fence_token"), "{err}");
    }

    #[tokio::test]
    async fn unknown_variant_is_refused_typed() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        // Pin the surface so resolve_session doesn't fail earlier on
        // missing session_id — this test focuses on variant-rejection,
        // not session resolution.
        let pinned =
            SessionId::parse("00000000-0000-0000-0000-000000000001").expect("uuid literal");
        let surface = MeerkatConsumerSurface::pinned(Arc::clone(&machine), pinned);
        let err = surface
            .apply_routed_input(iv("Recycle"), vec![])
            .await
            .expect_err("Recycle is not a routed variant");
        assert!(err.message().contains("Recycle"), "{err}");
    }

    #[tokio::test]
    async fn machine_rejection_keeps_per_variant_typed_code_across_consumer_seam() {
        // Row #14 gate (kernel→consumer leg): a generated-machine rejection of
        // a routed input must cross the consumer seam under its per-variant
        // stable discriminant, not be collapsed into one generic
        // `consumer_projection_failed` code.
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = sid("00000000-0000-0000-0000-000000000001");
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let surface = MeerkatConsumerSurface::pinned(Arc::clone(&machine), session_id.clone());

        // Ingest before any runtime binding: the generated machine rejects
        // the transition; the typed discriminant must survive verbatim.
        let err = surface
            .apply_routed_input(
                iv("Ingest"),
                vec![
                    (fld("runtime_id"), OwnedFieldValue::Str("rt-none".into())),
                    (fld("fence_token"), OwnedFieldValue::U64(1)),
                    (fld("work_id"), OwnedFieldValue::Str("work-1".into())),
                    (
                        fld("origin"),
                        OwnedFieldValue::Opaque(Arc::new(mm_dsl::WorkOrigin::Ingest)),
                    ),
                    (
                        fld("session_id"),
                        OwnedFieldValue::Str(session_id.to_string()),
                    ),
                ],
            )
            .await
            .expect_err("ingest on an unbound session must be machine-rejected");
        assert!(
            err.error_code().starts_with("dsl_"),
            "machine rejection must keep its per-variant typed code, got `{}`: {err}",
            err.error_code()
        );
        assert_ne!(
            err.error_code(),
            "consumer_projection_failed",
            "kernel rejections must not be collapsed under the generic projection code"
        );
    }

    #[tokio::test]
    async fn unpinned_surface_requires_projected_session_id_for_retire() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        // Retire has no fields in the schema; an unpinned surface
        // therefore cannot resolve a session and must refuse rather
        // than pick arbitrarily.
        let err = surface
            .apply_routed_input(iv("Retire"), vec![])
            .await
            .expect_err("Retire without target");
        assert!(err.message().contains("session_id"), "{err}");
    }

    #[tokio::test]
    async fn pinned_surface_rejects_mismatched_session_id() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let pinned =
            SessionId::parse("00000000-0000-0000-0000-000000000001").expect("uuid literal");
        let surface = MeerkatConsumerSurface::pinned(Arc::clone(&machine), pinned);
        let err = surface
            .apply_routed_input(
                iv("PrepareBindings"),
                vec![
                    (
                        fld("agent_runtime_id"),
                        OwnedFieldValue::Str("rt-other".into()),
                    ),
                    (fld("fence_token"), OwnedFieldValue::U64(1)),
                    (fld("generation"), OwnedFieldValue::U64(0)),
                    (
                        fld("session_id"),
                        OwnedFieldValue::Str("00000000-0000-0000-0000-000000000002".into()),
                    ),
                ],
            )
            .await
            .expect_err("session_id disagrees with pinned session");
        assert!(err.message().contains("pinned"), "{err}");
    }

    #[tokio::test]
    async fn ingest_prefers_projected_session_id() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        let session_id = sid("00000000-0000-0000-0000-000000000001");
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");
        bind_runtime(&surface, &session_id, "rt-other").await;

        surface
            .apply_routed_input(
                iv("Ingest"),
                vec![
                    (fld("runtime_id"), OwnedFieldValue::Str("rt-other".into())),
                    (fld("fence_token"), OwnedFieldValue::U64(1)),
                    (fld("generation"), OwnedFieldValue::U64(0)),
                    (fld("work_id"), OwnedFieldValue::Str("work-1".into())),
                    (
                        fld("origin"),
                        OwnedFieldValue::Opaque(Arc::new(mm_dsl::WorkOrigin::Ingest)),
                    ),
                    (
                        fld("session_id"),
                        OwnedFieldValue::Str(session_id.to_string()),
                    ),
                ],
            )
            .await
            .expect("session_id targets the routed input");
    }

    #[tokio::test]
    async fn ingest_requires_projected_session_id() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        let session_id = sid("00000000-0000-0000-0000-000000000001");
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");
        bind_runtime(&surface, &session_id, "rt-match").await;

        let err = surface
            .apply_routed_input(
                iv("Ingest"),
                vec![
                    (fld("runtime_id"), OwnedFieldValue::Str("rt-match".into())),
                    (fld("fence_token"), OwnedFieldValue::U64(1)),
                    (fld("generation"), OwnedFieldValue::U64(0)),
                    (fld("work_id"), OwnedFieldValue::Str("work-1".into())),
                    (
                        fld("origin"),
                        OwnedFieldValue::Opaque(Arc::new(mm_dsl::WorkOrigin::Ingest)),
                    ),
                ],
            )
            .await
            .expect_err("session_id is required for routed ingest");
        assert!(err.message().contains("session_id"), "{err}");
    }

    #[tokio::test]
    async fn ingest_without_matching_generated_binding_is_refused() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        let session_id = sid("00000000-0000-0000-0000-000000000001");
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");
        bind_runtime(&surface, &session_id, "rt-current").await;

        let err = surface
            .apply_routed_input(
                iv("Ingest"),
                vec![
                    (fld("runtime_id"), OwnedFieldValue::Str("rt-missing".into())),
                    (fld("fence_token"), OwnedFieldValue::U64(1)),
                    (fld("generation"), OwnedFieldValue::U64(0)),
                    (fld("work_id"), OwnedFieldValue::Str("work-1".into())),
                    (
                        fld("origin"),
                        OwnedFieldValue::Opaque(Arc::new(mm_dsl::WorkOrigin::Ingest)),
                    ),
                    (
                        fld("session_id"),
                        OwnedFieldValue::Str(session_id.to_string()),
                    ),
                ],
            )
            .await
            .expect_err("generated binding guard rejects the routed input");
        assert!(err.message().contains("Ingest"), "{err}");
    }

    #[derive(Default)]
    struct RecordingSignalSurface {
        log: tokio::sync::Mutex<Vec<(SignalVariantId, Vec<(FieldId, OwnedFieldValue)>)>>,
    }

    #[async_trait]
    impl SignalConsumerSurface for RecordingSignalSurface {
        fn instance_id(&self) -> &MachineInstanceId {
            static ID: OnceLock<MachineInstanceId> = OnceLock::new();
            ID.get_or_init(|| MachineInstanceId::parse("mob").expect("canonical instance id"))
        }

        async fn receive_signal(
            &self,
            variant: SignalVariantId,
            projected_fields: Vec<(FieldId, OwnedFieldValue)>,
        ) -> Result<(), crate::composition::ConsumerError> {
            self.log.lock().await.push((variant, projected_fields));
            Ok(())
        }
    }

    #[tokio::test]
    async fn routed_prepare_bindings_dispatches_runtime_bound_signal() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let signal_surface = Arc::new(RecordingSignalSurface::default());
        let schema = meerkat_machine_schema::catalog::meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).expect("catalog routes");
        let dispatcher: CatalogCompositionSignalDispatcher<MeerkatSeamSignal> =
            CatalogCompositionSignalDispatcher::new(schema.name.clone(), table)
                .with_consumer(signal_surface.clone());
        machine.set_composition_signal_dispatcher(Arc::new(dispatcher));

        machine
            .apply_routed_meerkat_input(
                &session_id,
                mm_dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: mm_dsl::AgentRuntimeId("rt-1".into()),
                    fence_token: mm_dsl::FenceToken(11),
                    generation: Some(mm_dsl::Generation(0)),
                    runtime_epoch_id: None,
                    session_id: mm_dsl::SessionId(session_id.to_string()),
                },
            )
            .await
            .expect("routed input applies and emits signal");

        let log = signal_surface.log.lock().await;
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].0.as_str(), "ObserveRuntimeReady");
        assert_eq!(log[0].1[0].0.as_str(), "agent_runtime_id");
        assert!(matches!(&log[0].1[0].1, OwnedFieldValue::Str(value) if value == "rt-1"));
        assert_eq!(log[0].1[1].0.as_str(), "fence_token");
        assert!(matches!(log[0].1[1].1, OwnedFieldValue::U64(11)));
    }

    /// Schema-enumerated completeness gate (mirror of the mob seam's
    /// `lift_covers_every_schema_declared_mob_effect_route`): every
    /// schema-declared meerkat-producer route must have a lift arm in
    /// `lift_routed_signal`, and the lift must not invent undeclared routes.
    #[test]
    fn lift_covers_every_schema_declared_meerkat_signal_route() {
        use std::collections::BTreeSet;

        let schema = meerkat_machine_schema::catalog::meerkat_mob_seam_composition();
        let declared: BTreeSet<String> = schema
            .routes
            .iter()
            .filter(|route| &route.from_machine == meerkat_instance_id())
            .map(|route| route.effect_variant.as_str().to_string())
            .collect();

        let liftable_bodies = [
            mm_dsl::MeerkatMachineEffect::RuntimeBound {
                agent_runtime_id: mm_dsl::AgentRuntimeId("rt-1".into()),
                fence_token: mm_dsl::FenceToken(1),
            },
            mm_dsl::MeerkatMachineEffect::RuntimeRetired {
                agent_runtime_id: mm_dsl::AgentRuntimeId("rt-1".into()),
                fence_token: mm_dsl::FenceToken(1),
            },
            mm_dsl::MeerkatMachineEffect::RuntimeDestroyed {
                agent_runtime_id: mm_dsl::AgentRuntimeId("rt-1".into()),
                fence_token: mm_dsl::FenceToken(1),
            },
        ];
        let liftable: BTreeSet<String> = liftable_bodies
            .iter()
            .map(|effect| {
                lift_routed_signal(effect)
                    .expect("declared routed effect must lift")
                    .variant_id()
                    .as_str()
                    .to_string()
            })
            .collect();

        assert_eq!(
            declared, liftable,
            "every schema-declared meerkat signal route must have a lift arm in \
             lift_routed_signal (and vice versa); update the lift AND this gate \
             together when the composition changes"
        );
    }

    #[test]
    fn routed_meerkat_signal_projection_tracks_generated_route_facts() {
        use crate::generated::meerkat_mob_seam as seam_facts;

        let cases = vec![
            (
                MeerkatSeamSignal::RuntimeBound {
                    agent_runtime_id: mm_dsl::AgentRuntimeId("rt-bound".into()),
                    fence_token: mm_dsl::FenceToken(11),
                },
                seam_facts::route_runtime_bound_reaches_mob(),
            ),
            (
                MeerkatSeamSignal::RuntimeRetired {
                    agent_runtime_id: mm_dsl::AgentRuntimeId("rt-retired".into()),
                    fence_token: mm_dsl::FenceToken(12),
                },
                seam_facts::route_runtime_retired_reaches_mob(),
            ),
            (
                MeerkatSeamSignal::RuntimeDestroyed {
                    agent_runtime_id: mm_dsl::AgentRuntimeId("rt-destroyed".into()),
                    fence_token: mm_dsl::FenceToken(13),
                },
                seam_facts::route_runtime_destroyed_reaches_mob(),
            ),
        ];

        for (signal, expected_route) in cases {
            let route = signal.generated_signal_route().expect("generated route");
            assert_eq!(route, expected_route);
            for (producer_field, _) in &route.bindings {
                assert!(
                    signal.field(producer_field).is_some(),
                    "generated route `{}` requires producer field `{}`",
                    route.route_id.as_str(),
                    producer_field.as_str()
                );
            }
        }
    }

    #[tokio::test]
    async fn local_session_bindings_do_not_dispatch_runtime_bound_signal() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();

        let signal_surface = Arc::new(RecordingSignalSurface::default());
        let schema = meerkat_machine_schema::catalog::meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).expect("catalog routes");
        let dispatcher: CatalogCompositionSignalDispatcher<MeerkatSeamSignal> =
            CatalogCompositionSignalDispatcher::new(schema.name.clone(), table)
                .with_consumer(signal_surface.clone());
        machine.set_composition_signal_dispatcher(Arc::new(dispatcher));

        let bindings = machine
            .prepare_local_session_bindings(session_id.clone())
            .await
            .expect("local bindings prepare");

        assert_eq!(bindings.session_id(), &session_id);
        let expected_epoch = {
            let sessions = machine.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("session registered")
                .epoch_id
                .to_string()
        };
        assert!(
            signal_surface.log.lock().await.is_empty(),
            "local resource preparation must not publish cross-machine runtime readiness"
        );
        {
            let sessions = machine.sessions.read().await;
            let entry = sessions.get(&session_id).expect("session registered");
            let authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(
                authority.state().active_runtime_id.is_none(),
                "local resource preparation must leave binding identity unclaimed"
            );
            assert!(
                authority.state().active_fence_token.is_none(),
                "local resource preparation must leave binding fence unclaimed"
            );
        }

        machine
            .prepare_runtime_placement_binding(
                session_id.clone(),
                crate::identifiers::LogicalRuntimeId::new("rt-authoritative"),
                13,
                7,
            )
            .await
            .expect("authoritative binding still applies after local resource prep");
        machine
            .prepare_runtime_placement_binding(
                session_id.clone(),
                crate::identifiers::LogicalRuntimeId::new("rt-authoritative"),
                13,
                7,
            )
            .await
            .expect("exact placement replay is idempotent");

        let log = signal_surface.log.lock().await;
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].0.as_str(), "ObserveRuntimeReady");
        assert!(
            matches!(&log[0].1[0].1, OwnedFieldValue::Str(value) if value == "rt-authoritative")
        );
        {
            let sessions = machine.sessions.read().await;
            let entry = sessions.get(&session_id).expect("session registered");
            let authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(
                matches!(&authority.state().active_runtime_id, Some(value) if value.0 == "rt-authoritative")
            );
            assert!(matches!(
                authority.state().active_fence_token,
                Some(mm_dsl::FenceToken(13))
            ));
            assert!(matches!(
                authority.state().active_runtime_generation,
                Some(mm_dsl::Generation(7))
            ));
            assert_eq!(
                authority
                    .state()
                    .active_runtime_epoch_id
                    .as_ref()
                    .map(|id| id.0.as_str()),
                Some(expected_epoch.as_str()),
                "exact member placement must still bind the current registration epoch"
            );
        }
    }

    #[tokio::test]
    async fn current_idle_stale_binding_is_normalized_before_routed_revival_rebinds() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let runtime_store: Arc<dyn crate::store::RuntimeStore> = store.clone();
        let session_id = sid("00000000-0000-0000-0000-000000000034");
        let runtime_id = MeerkatMachine::logical_runtime_id(&session_id);

        crate::store::RuntimeStore::commit_machine_lifecycle(
            store.as_ref(),
            &runtime_id,
            crate::store::MachineLifecycleCommit::new_with_binding(
                crate::RuntimeState::Idle,
                crate::store::MachineLifecycleBindingFacts::new(
                    Some("stale-mob-runtime:7".into()),
                    Some(41),
                    Some(7),
                    Some("stale-runtime-epoch".into()),
                ),
                crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
            ),
            &[],
        )
        .await
        .expect("seed idle lifecycle with the interrupted epoch's binding tuple");

        let encoded =
            crate::store::RuntimeStore::load_machine_lifecycle_record(store.as_ref(), &runtime_id)
                .await
                .expect("load seeded lifecycle record")
                .expect("seeded lifecycle record exists");
        let encoded: serde_json::Value =
            serde_json::from_slice(&encoded).expect("lifecycle record is JSON");
        assert_eq!(
            encoded["record_version"],
            crate::store::MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            "the regression fixture must exercise the current durable lifecycle shape"
        );

        let machine = Arc::new(MeerkatMachine::persistent(
            runtime_store,
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ));
        let signal_surface = Arc::new(RecordingSignalSurface::default());
        let schema = meerkat_machine_schema::catalog::meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).expect("catalog routes");
        let dispatcher: CatalogCompositionSignalDispatcher<MeerkatSeamSignal> =
            CatalogCompositionSignalDispatcher::new(schema.name.clone(), table)
                .with_consumer(signal_surface.clone());
        machine.set_composition_signal_dispatcher(Arc::new(dispatcher));

        machine
            .prepare_local_session_bindings(session_id.clone())
            .await
            .expect("delivery-time local preparation recovers the idle runtime");

        let recovered = machine
            .session_dsl_state(&session_id)
            .await
            .expect("recovered session authority exists");
        assert_eq!(recovered.lifecycle_phase, mm_dsl::MeerkatPhase::Idle);
        assert_eq!(
            recovered.registration_phase,
            mm_dsl::RegistrationPhase::Queuing,
            "the lifecycle row does not persist a live executor claim, so cold recovery must queue a replacement"
        );
        assert_eq!(
            recovered.active_runtime_id, None,
            "cold registration must release the stale runtime owner"
        );
        assert_eq!(
            recovered.active_fence_token, None,
            "cold registration must release the stale fence"
        );
        assert_eq!(
            recovered.active_runtime_generation, None,
            "cold registration must release the stale generation"
        );
        let entry_epoch = entry_runtime_epoch_id(&machine, &session_id).await;
        assert_ne!(
            recovered.active_runtime_epoch_id,
            Some(mm_dsl::RuntimeEpochId("stale-runtime-epoch".into())),
            "cold registration must release the DEAD process's runtime epoch"
        );
        assert_eq!(
            recovered.active_runtime_epoch_id,
            Some(entry_epoch.clone()),
            "cold registration installs the epoch of the entry it just created: \
             the epoch is a registration fact, and a live registered entry that \
             carries none can never authorize a compaction projection"
        );
        assert!(
            signal_surface.log.lock().await.is_empty(),
            "local resource recovery must not publish stale runtime readiness"
        );

        // No physical attachment exists yet: this request belongs to the
        // machine's durable unassigned queue, not to an attachment A that a
        // later B could wrongly inherit.
        assert!(
            machine
                .current_executor_attachment_witness(&session_id)
                .await
                .is_none(),
            "cold Queuing recovery must not mint an attachment witness"
        );
        let queued_input = crate::input::Input::Prompt(crate::input::PromptInput::new(
            "durable work queued before missing-live materialization",
            None,
        ));
        let queued_input_id = queued_input.id().clone();
        let (outcome, completion) = machine
            .accept_input_with_completion(&session_id, queued_input)
            .await
            .expect("queue durable work before missing-live materialization");
        assert!(outcome.is_accepted());
        let completion = completion.expect("queued durable work installs a completion waiter");
        let durable_before =
            crate::store::RuntimeStore::load_input_states_strict(store.as_ref(), &runtime_id)
                .await
                .expect("load queued durable work before missing-live preparation");
        assert_eq!(durable_before.len(), 1);
        assert_eq!(durable_before[0].state.input_id, queued_input_id);
        assert_eq!(
            durable_before[0].seed.phase,
            crate::input_state::InputLifecycleState::Queued
        );
        let spine_before_revival = machine
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("runtime spine exists before missing-live materialization");
        assert_eq!(
            spine_before_revival.inputs.queue,
            vec![queued_input_id.clone()]
        );

        let prepared = machine
            .prepare_local_session_materialization_with_mode(
                session_id.clone(),
                LocalSessionMaterializationMode::MissingLiveRevival,
            )
            .await
            .expect("typed missing-live materialization claims the cold-normalized session");

        let normalized = machine
            .session_dsl_state(&session_id)
            .await
            .expect("normalized revival authority exists");
        assert_eq!(normalized.lifecycle_phase, mm_dsl::MeerkatPhase::Idle);
        assert_eq!(
            normalized.registration_phase,
            mm_dsl::RegistrationPhase::Queuing
        );
        assert_eq!(
            normalized.active_runtime_id, None,
            "missing-live preparation releases the stale runtime owner"
        );
        assert_eq!(
            normalized.active_fence_token, None,
            "missing-live preparation releases the stale fence"
        );
        assert_eq!(
            normalized.active_runtime_generation, None,
            "missing-live preparation releases the stale generation"
        );
        assert_eq!(
            normalized.active_runtime_epoch_id,
            Some(entry_epoch.clone()),
            "missing-live preparation releases the stale PLACEMENT and retains the \
             registered entry epoch"
        );
        let after_revival = machine
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("runtime spine remains available after missing-live preparation");
        assert_input_spine_preserved(&spine_before_revival.inputs, &after_revival.inputs);
        assert_eq!(
            after_revival.completion_waiters, spine_before_revival.completion_waiters,
            "typed exit/readmit must leave completion waiters verbatim"
        );
        let durable_after_revival =
            crate::store::RuntimeStore::load_input_states_strict(store.as_ref(), &runtime_id)
                .await
                .expect("load queued durable work after missing-live preparation");
        assert_eq!(durable_after_revival.len(), 1);
        assert_eq!(durable_after_revival[0].state.input_id, queued_input_id);
        assert_eq!(
            durable_after_revival[0].seed.phase,
            crate::input_state::InputLifecycleState::Queued,
            "missing-live preparation must not terminalize or consume unassigned durable work"
        );
        assert!(
            signal_surface.log.lock().await.is_empty(),
            "missing-live preparation must not publish runtime readiness for the discarded stale tuple"
        );
        assert!(
            machine
                .current_executor_attachment_witness(&session_id)
                .await
                .is_none(),
            "missing-live preparation must leave the durable queue unassigned until its first live attachment"
        );
        crate::begin_session_runtime_actor_materialization(prepared.bindings())
            .expect("claim the normalized materialization for actor construction")
            .commit()
            .expect("record the normalized actor materialization");

        let (registration, allow_reconciliation) =
            begin_startup_gated_first_live_attachment(&machine, &session_id, prepared).await;
        let attached = machine
            .session_dsl_state(&session_id)
            .await
            .expect("first live attachment startup authority exists");
        assert_eq!(attached.active_runtime_id, None, "stale owner released");
        assert_eq!(attached.active_fence_token, None, "stale fence released");
        assert_eq!(
            attached.active_runtime_generation, None,
            "stale generation released"
        );
        assert_eq!(
            attached.active_runtime_epoch_id,
            Some(entry_epoch.clone()),
            "stale placement released, registered entry epoch retained"
        );
        assert!(
            signal_surface.log.lock().await.is_empty(),
            "executor attachment alone must not publish runtime readiness"
        );

        let startup_persisted = crate::store::load_machine_lifecycle(store.as_ref(), &runtime_id)
            .await
            .expect("load startup-persisted healed lifecycle")
            .expect("startup-persisted lifecycle exists");
        assert_eq!(
            startup_persisted.runtime_state(),
            crate::RuntimeState::Idle,
            "replacement startup may retain the repaired Idle lifecycle projection; the V3 binding tuple below is the required healed fact"
        );
        assert_eq!(startup_persisted.binding().agent_runtime_id(), None);
        assert_eq!(startup_persisted.binding().fence_token(), None);
        assert_eq!(startup_persisted.binding().runtime_generation(), None);
        // Startup commits the HEALED (fully cleared) tuple rather than a
        // projection of the live authority, so this row carries no epoch even
        // though the in-memory entry is registered under one. That is the
        // Converged fixed point on the next cold recovery.
        assert_eq!(startup_persisted.binding().runtime_epoch_id(), None);

        route_successor_binding_and_finish_gated_attachment(
            &machine,
            &session_id,
            registration,
            allow_reconciliation,
            completion,
            "replacement-mob-runtime:8",
            42,
            8,
        )
        .await;

        let healed_durable = crate::store::load_machine_lifecycle(store.as_ref(), &runtime_id)
            .await
            .expect("load healed lifecycle after replacement completion")
            .expect("healed lifecycle exists");
        assert_eq!(
            healed_durable.runtime_state(),
            crate::RuntimeState::Idle,
            "generated durability classification projects process-local Attached to recoverable Idle"
        );
        assert_eq!(
            healed_durable.binding().agent_runtime_id(),
            Some("replacement-mob-runtime:8")
        );
        assert_eq!(healed_durable.binding().fence_token(), Some(42));
        assert_eq!(healed_durable.binding().runtime_generation(), Some(8));
        assert_eq!(
            healed_durable.binding().runtime_epoch_id(),
            Some(entry_epoch.0.as_str()),
            "the replacement placement asserts, and therefore preserves, the \
             registered entry epoch"
        );

        let log = signal_surface.log.lock().await;
        assert_eq!(log.len(), 1, "successor binding publishes readiness once");
        assert_eq!(log[0].0.as_str(), "ObserveRuntimeReady");
        assert_eq!(log[0].1[0].0.as_str(), "agent_runtime_id");
        assert!(
            matches!(&log[0].1[0].1, OwnedFieldValue::Str(value) if value == "replacement-mob-runtime:8")
        );
        assert_eq!(log[0].1[1].0.as_str(), "fence_token");
        assert!(matches!(log[0].1[1].1, OwnedFieldValue::U64(42)));
    }

    #[tokio::test]
    async fn ownerless_attached_claim_is_normalized_by_missing_live_materialization() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = sid("00000000-0000-0000-0000-000000000035");
        let signal_surface = Arc::new(RecordingSignalSurface::default());
        let schema = meerkat_machine_schema::catalog::meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).expect("catalog routes");
        let dispatcher: CatalogCompositionSignalDispatcher<MeerkatSeamSignal> =
            CatalogCompositionSignalDispatcher::new(schema.name.clone(), table)
                .with_consumer(signal_surface.clone());
        machine.set_composition_signal_dispatcher(Arc::new(dispatcher));

        machine
            .prepare_local_session_bindings(session_id.clone())
            .await
            .expect("prepare the original in-process session entry");
        // This input is accepted before any physical executor attachment
        // exists, so it remains machine-owned and unassigned across the
        // ownerless-claim repair below.
        assert!(
            machine
                .current_executor_attachment_witness(&session_id)
                .await
                .is_none(),
            "local preparation must not mint an attachment witness"
        );
        let queued_input = crate::input::Input::Prompt(crate::input::PromptInput::new(
            "queued work survives an ownerless in-process executor claim",
            None,
        ));
        let queued_input_id = queued_input.id().clone();
        let (outcome, completion) = machine
            .accept_input_with_completion(&session_id, queued_input)
            .await
            .expect("queue work before manufacturing the ownerless claim");
        assert!(outcome.is_accepted());
        let completion = completion.expect("queued work installs a completion waiter");

        let (driver_before, authority_before, epoch_before) = {
            let sessions = machine.sessions.read().await;
            let entry = sessions.get(&session_id).expect("session entry exists");
            (
                entry.driver.clone(),
                Arc::clone(&entry.dsl_authority),
                entry.epoch_id.clone(),
            )
        };

        // Manufacture the exact limbo shape on this live entry: generated
        // Attached/Active authority with all stale binding facts, but no
        // runtime-loop attachment or teardown owner. No recovery or entry
        // reconstruction participates in this fixture.
        machine
            .stage_session_dsl_input(
                &session_id,
                mm_dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: mm_dsl::AgentRuntimeId(
                        "ownerless-stale-mob-runtime:9".into(),
                    ),
                    fence_token: mm_dsl::FenceToken(51),
                    generation: Some(mm_dsl::Generation(9)),
                    // The stale facts under test are the PLACEMENT triple; the
                    // epoch is registration-owned, so the fixture must state the
                    // registered one (a placement naming any other epoch is a
                    // typed rejection).
                    runtime_epoch_id: Some(mm_dsl::RuntimeEpochId::from_domain(&epoch_before)),
                    session_id: mm_dsl::SessionId(session_id.to_string()),
                },
                "OwnerlessAttachedRegressionBinding",
            )
            .await
            .expect("stage the stale binding tuple on the same session entry");
        machine
            .stage_session_dsl_input(
                &session_id,
                mm_dsl::MeerkatMachineInput::EnsureSessionWithExecutor {
                    session_id: mm_dsl::SessionId(session_id.to_string()),
                },
                "OwnerlessAttachedRegressionClaim",
            )
            .await
            .expect("stage the ownerless active executor claim");
        {
            let mut driver = driver_before.lock().await;
            driver.sync_control_projection_from_dsl_authority();
        }

        let ownerless = machine
            .session_dsl_state(&session_id)
            .await
            .expect("ownerless authority exists");
        assert_eq!(ownerless.lifecycle_phase, mm_dsl::MeerkatPhase::Attached);
        assert_eq!(
            ownerless.registration_phase,
            mm_dsl::RegistrationPhase::Active
        );
        assert!(
            matches!(&ownerless.active_runtime_id, Some(value) if value.0 == "ownerless-stale-mob-runtime:9")
        );
        assert_eq!(ownerless.active_fence_token, Some(mm_dsl::FenceToken(51)));
        assert_eq!(
            ownerless.active_runtime_generation,
            Some(mm_dsl::Generation(9))
        );
        assert!(
            ownerless.active_runtime_epoch_id
                == Some(mm_dsl::RuntimeEpochId::from_domain(&epoch_before))
        );
        {
            let sessions = machine.sessions.read().await;
            let entry = sessions.get(&session_id).expect("ownerless entry exists");
            assert!(
                !entry.has_live_attachment(),
                "an ownerless generated claim must not imply a serving attachment"
            );
            assert!(entry.runtime_loop_teardown.is_none());
        }

        let before_revival = machine
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("ownerless runtime spine exists before missing-live preparation");
        assert_eq!(before_revival.inputs.queue, vec![queued_input_id.clone()]);
        let prepared = machine
            .prepare_local_session_materialization_with_mode(
                session_id.clone(),
                LocalSessionMaterializationMode::MissingLiveRevival,
            )
            .await
            .expect("typed missing-live materialization repairs the ownerless active claim");

        let normalized = machine
            .session_dsl_state(&session_id)
            .await
            .expect("normalized ownerless authority exists");
        assert_eq!(normalized.lifecycle_phase, mm_dsl::MeerkatPhase::Idle);
        assert_eq!(
            normalized.registration_phase,
            mm_dsl::RegistrationPhase::Queuing
        );
        assert_eq!(normalized.active_runtime_id, None);
        assert_eq!(normalized.active_fence_token, None);
        assert_eq!(normalized.active_runtime_generation, None);
        assert_eq!(
            normalized.active_runtime_epoch_id,
            Some(mm_dsl::RuntimeEpochId::from_domain(&epoch_before)),
            "normalization clears the ownerless placement and keeps the registered \
             entry epoch"
        );
        let after_revival = machine
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("ownerless runtime spine survives missing-live preparation");
        assert_input_spine_preserved(&before_revival.inputs, &after_revival.inputs);
        assert_eq!(
            after_revival.completion_waiters, before_revival.completion_waiters,
            "same-instance exit/readmit must leave completion waiters verbatim"
        );
        {
            let sessions = machine.sessions.read().await;
            let entry = sessions
                .get(&session_id)
                .expect("same entry survives missing-live preparation");
            assert!(Arc::ptr_eq(&driver_before, &entry.driver));
            assert!(Arc::ptr_eq(&authority_before, &entry.dsl_authority));
            assert_eq!(entry.epoch_id, epoch_before);
            assert!(
                !entry.has_live_attachment(),
                "missing-live normalization must leave no serving attachment"
            );
            assert!(entry.runtime_loop_teardown.is_none());
        }
        assert!(
            signal_surface.log.lock().await.is_empty(),
            "fixture staging and missing-live preparation must not publish stale runtime readiness"
        );
        assert!(
            machine
                .current_executor_attachment_witness(&session_id)
                .await
                .is_none(),
            "missing-live preparation must leave the queue unassigned until its first live attachment"
        );
        crate::begin_session_runtime_actor_materialization(prepared.bindings())
            .expect("claim the normalized materialization for actor construction")
            .commit()
            .expect("record the normalized actor materialization");

        let (registration, allow_reconciliation) =
            begin_startup_gated_first_live_attachment(&machine, &session_id, prepared).await;
        route_successor_binding_and_finish_gated_attachment(
            &machine,
            &session_id,
            registration,
            allow_reconciliation,
            completion,
            "ownerless-replacement-mob-runtime:10",
            52,
            10,
        )
        .await;

        let log = signal_surface.log.lock().await;
        assert_eq!(log.len(), 1, "successor binding publishes readiness once");
        assert_eq!(log[0].0.as_str(), "ObserveRuntimeReady");
        assert!(
            matches!(&log[0].1[0].1, OwnedFieldValue::Str(value) if value == "ownerless-replacement-mob-runtime:10")
        );
        assert!(matches!(log[0].1[1].1, OwnedFieldValue::U64(52)));
    }

    #[tokio::test]
    async fn completed_stop_cleanup_is_retired_by_idle_preparation() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = sid("00000000-0000-0000-0000-000000000036");
        machine
            .prepare_local_session_bindings(session_id.clone())
            .await
            .expect("prepare the original session entry");
        machine
            .ensure_session_with_executor(session_id.clone(), Box::new(NoopExecutor))
            .await
            .expect("attach the exact executor that will own stop cleanup");
        machine
            .stop_runtime_executor(&session_id, "completed cleanup regression")
            .await
            .expect("exact executor stop cleanup completes");

        let stopped = machine
            .session_dsl_state(&session_id)
            .await
            .expect("stopped authority remains registered");
        assert_eq!(stopped.lifecycle_phase, mm_dsl::MeerkatPhase::Stopped);
        {
            let sessions = machine.sessions.read().await;
            let entry = sessions.get(&session_id).expect("stopped entry remains");
            let coordinator = entry
                .runtime_stop_cleanup_coordinator
                .as_ref()
                .expect("completed exact cleanup receipt remains attached to the epoch");
            assert!(matches!(
                coordinator.result_rx.borrow().clone(),
                Some(Ok(()))
            ));
            assert!(
                !entry.has_live_attachment(),
                "completed exact stop cleanup must leave no live executor"
            );
            assert!(
                matches!(entry.attachment_slot, RuntimeLoopAttachmentSlot::Empty),
                "completed exact stop cleanup must retire its attachment slot"
            );
        }

        // Re-admit first, as the delivery path can do before its later local
        // bindings preparation. The subsequent idempotent preparation must
        // not turn the completed cleanup receipt into an in-progress owner.
        let entry_epoch_id = {
            let sessions = machine.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("stopped entry remains")
                .epoch_id
                .clone()
        };
        machine
            .stage_session_dsl_input(
                &session_id,
                mm_dsl::MeerkatMachineInput::RegisterSession {
                    session_id: mm_dsl::SessionId(session_id.to_string()),
                    // Re-admission restates the entry's registered epoch.
                    runtime_epoch_id: Some(mm_dsl::RuntimeEpochId::from_domain(&entry_epoch_id)),
                },
                "CompletedStopRegressionReadmit",
            )
            .await
            .expect("re-admit the stopped session without recreating its entry");
        machine
            .prepare_local_session_bindings(session_id.clone())
            .await
            .expect("local preparation preserves the re-admitted session");

        let prepared = machine
            .session_dsl_state(&session_id)
            .await
            .expect("prepared re-admitted authority exists");
        assert_eq!(prepared.lifecycle_phase, mm_dsl::MeerkatPhase::Idle);
        assert_eq!(
            prepared.registration_phase,
            mm_dsl::RegistrationPhase::Queuing
        );
        {
            let sessions = machine.sessions.read().await;
            let entry = sessions
                .get(&session_id)
                .expect("prepared entry remains registered");
            assert!(
                entry.runtime_stop_cleanup_coordinator.is_none(),
                "local preparation must retire the completed exact cleanup receipt"
            );
            assert!(entry.runtime_loop_teardown.is_none());
        }
    }

    #[tokio::test]
    async fn session_owned_prepare_bindings_is_idempotent_without_reemitting_runtime_bound() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();

        let signal_surface = Arc::new(RecordingSignalSurface::default());
        let schema = meerkat_machine_schema::catalog::meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).expect("catalog routes");
        let dispatcher: CatalogCompositionSignalDispatcher<MeerkatSeamSignal> =
            CatalogCompositionSignalDispatcher::new(schema.name.clone(), table)
                .with_consumer(signal_surface.clone());
        machine.set_composition_signal_dispatcher(Arc::new(dispatcher));

        machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("initial session-owned binding prepares");
        machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("duplicate session-owned binding returns existing handles");

        let log = signal_surface.log.lock().await;
        assert_eq!(
            log.len(),
            1,
            "duplicate handle resolution must not publish a second RuntimeBound signal"
        );
        assert_eq!(log[0].0.as_str(), "ObserveRuntimeReady");
        assert!(
            matches!(&log[0].1[0].1, OwnedFieldValue::Str(value) if value.starts_with("rt:session:"))
        );
    }

    #[tokio::test]
    async fn session_owned_prepare_bindings_rejects_conflicting_authoritative_runtime() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let signal_surface = Arc::new(RecordingSignalSurface::default());
        let schema = meerkat_machine_schema::catalog::meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).expect("catalog routes");
        let dispatcher: CatalogCompositionSignalDispatcher<MeerkatSeamSignal> =
            CatalogCompositionSignalDispatcher::new(schema.name.clone(), table)
                .with_consumer(signal_surface.clone());
        machine.set_composition_signal_dispatcher(Arc::new(dispatcher));

        machine
            .apply_routed_meerkat_input(
                &session_id,
                mm_dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: mm_dsl::AgentRuntimeId("operator-rt:0".into()),
                    fence_token: mm_dsl::FenceToken(17),
                    generation: Some(mm_dsl::Generation(0)),
                    runtime_epoch_id: None,
                    session_id: mm_dsl::SessionId(session_id.to_string()),
                },
            )
            .await
            .expect("mob-owned authoritative binding applies");

        let err = machine
            .prepare_bindings(session_id.clone())
            .await
            .expect_err("session-owned binding must not overwrite mob-owned authority");
        assert!(
            err.to_string().contains("DSL authority (PrepareBindings)"),
            "{err}"
        );

        let state = machine
            .session_dsl_state(&session_id)
            .await
            .expect("session state remains available");
        assert!(
            matches!(&state.active_runtime_id, Some(value) if value.0 == "operator-rt:0"),
            "conflicting prepare_bindings must not rewrite active_runtime_id: {:?}",
            state.active_runtime_id
        );
        assert!(matches!(
            state.active_fence_token,
            Some(mm_dsl::FenceToken(17))
        ));

        let log = signal_surface.log.lock().await;
        assert_eq!(
            log.len(),
            1,
            "rejected session-owned binding must not publish a shadow RuntimeBound signal"
        );
        assert!(matches!(&log[0].1[0].1, OwnedFieldValue::Str(value) if value == "operator-rt:0"));
    }
}
