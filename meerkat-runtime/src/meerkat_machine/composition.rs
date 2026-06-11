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
/// Session selection: routed effects prefer a projected `session_id`.
/// `Ingest` may arrive without one, so the shared surface resolves its
/// projected `runtime_id` through each registered session's DSL-owned
/// `active_runtime_id`. Zero or multiple matches are refused rather than
/// guessing.
///
/// [mmi]: crate::meerkat_machine::dsl::MeerkatMachineInput
pub struct MeerkatConsumerSurface {
    machine: Arc<MeerkatMachine>,
    /// Optional pinned session id. `None` means the surface infers the
    /// session from the `agent_runtime_id` field of each routed input;
    /// `Some(id)` means every routed input is applied against that
    /// session and the surface refuses variants whose projected
    /// `agent_runtime_id` disagrees.
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

#[async_trait]
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
            .apply_routed_meerkat_input(
                &session_id,
                mm_dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: mm_dsl::AgentRuntimeId("rt-authoritative".into()),
                    fence_token: mm_dsl::FenceToken(13),
                    generation: Some(mm_dsl::Generation(0)),
                    runtime_epoch_id: None,
                    session_id: mm_dsl::SessionId(session_id.to_string()),
                },
            )
            .await
            .expect("authoritative binding still applies after local resource prep");

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
