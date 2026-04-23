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
use meerkat_machine_schema::identity::{FieldId, InputVariantId, MachineInstanceId};

use crate::composition::{ConsumerSurface, OwnedFieldValue};
use crate::meerkat_machine::{MeerkatMachine, dsl as mm_dsl};

/// Consumer-side surface for the `meerkat_mob_seam` composition.
///
/// Implements [`ConsumerSurface`] for the `meerkat` target instance. The
/// dispatcher hands the surface one routed input at a time; the surface
/// translates the typed [`InputVariantId`] + projected-field tuple into
/// the matching [`MeerkatMachineInput`][mmi] and applies it against the
/// session's shared DSL authority on the owning [`MeerkatMachine`].
///
/// Session selection: routed effects carry `agent_runtime_id` (for
/// variants that require a target) which matches the `SessionId` string
/// used by `MeerkatMachine::register_session`. `Retire` and `Destroy`
/// do not carry a target in the schema — production wiring is
/// per-member and will install one surface per session so the
/// `agent_runtime_id` of the carrying surface is authoritative; until
/// that per-session wiring lands, a single shared surface returns a
/// typed refusal rather than mis-routing.
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

impl std::fmt::Debug for MeerkatConsumerSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeerkatConsumerSurface")
            .field("pinned_session", &self.pinned_session)
            .finish_non_exhaustive()
    }
}

impl MeerkatConsumerSurface {
    /// Build a consumer surface backed by the given machine. The surface
    /// resolves each routed input's target session from the projected
    /// `agent_runtime_id` field.
    pub fn new(machine: Arc<MeerkatMachine>) -> Self {
        Self {
            machine,
            pinned_session: None,
        }
    }

    /// Build a consumer surface pinned to `session_id`. All routed
    /// inputs are applied against this session; variants that carry
    /// `agent_runtime_id` are additionally checked for agreement and
    /// refused on mismatch.
    pub fn pinned(machine: Arc<MeerkatMachine>, session_id: SessionId) -> Self {
        Self {
            machine,
            pinned_session: Some(session_id),
        }
    }

    fn resolve_session(
        &self,
        projected: &[(FieldId, OwnedFieldValue)],
    ) -> Result<SessionId, String> {
        let projected_runtime_id = projected
            .iter()
            .find(|(id, _)| id.as_str() == "agent_runtime_id")
            .and_then(|(_, v)| match v {
                OwnedFieldValue::Str(s) => Some(s.clone()),
                _ => None,
            });

        match (&self.pinned_session, projected_runtime_id) {
            (Some(pinned), Some(rt)) if rt != pinned.to_string() => Err(format!(
                "routed agent_runtime_id `{rt}` does not match pinned session `{pinned}`"
            )),
            (Some(pinned), _) => Ok(pinned.clone()),
            (None, Some(rt)) => SessionId::parse(&rt).map_err(|e| {
                format!("routed agent_runtime_id `{rt}` is not a valid session UUID: {e}")
            }),
            (None, None) => Err(
                "routed input did not project `agent_runtime_id` and surface is not pinned \
                 to a session — no session can be resolved"
                    .into(),
            ),
        }
    }
}

fn meerkat_instance_id() -> &'static MachineInstanceId {
    static ID: OnceLock<MachineInstanceId> = OnceLock::new();
    ID.get_or_init(|| {
        MachineInstanceId::parse("meerkat").expect("canonical consumer instance slug")
    })
}

fn project_u64(fields: &[(FieldId, OwnedFieldValue)], name: &str) -> Result<u64, String> {
    fields
        .iter()
        .find(|(id, _)| id.as_str() == name)
        .ok_or_else(|| format!("missing projected field `{name}`"))
        .and_then(|(_, v)| match v {
            OwnedFieldValue::U64(n) => Ok(*n),
            other => Err(format!("projected field `{name}` is not U64: {other:?}")),
        })
}

fn project_str<'a>(
    fields: &'a [(FieldId, OwnedFieldValue)],
    name: &str,
) -> Result<&'a str, String> {
    fields
        .iter()
        .find(|(id, _)| id.as_str() == name)
        .ok_or_else(|| format!("missing projected field `{name}`"))
        .and_then(|(_, v)| match v {
            OwnedFieldValue::Str(s) => Ok(s.as_str()),
            other => Err(format!("projected field `{name}` is not Str: {other:?}")),
        })
}

fn parse_work_origin(slug: &str) -> Result<mm_dsl::WorkOrigin, String> {
    match slug {
        "External" => Ok(mm_dsl::WorkOrigin::External),
        "Internal" => Ok(mm_dsl::WorkOrigin::Internal),
        "Ingest" => Ok(mm_dsl::WorkOrigin::Ingest),
        other => Err(format!("unknown WorkOrigin slug `{other}`")),
    }
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
    ) -> Result<(), String> {
        let session_id = self.resolve_session(&projected)?;
        let input = match variant.as_str() {
            "PrepareBindings" => {
                let rt = project_str(&projected, "agent_runtime_id")?;
                let fence = project_u64(&projected, "fence_token")?;
                let gen_ = project_u64(&projected, "generation")?;
                mm_dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: mm_dsl::AgentRuntimeId::from(rt.to_string()),
                    fence_token: mm_dsl::FenceToken(fence),
                    generation: mm_dsl::Generation(gen_),
                }
            }
            "Ingest" => {
                // Route binding `work_request_reaches_meerkat` delivers
                // producer `runtime_id` → consumer `agent_runtime_id`;
                // producer `work_id` → consumer `work_id`; producer
                // `origin` → consumer `origin`. The consumer DSL input
                // names the runtime field `runtime_id`, matching the
                // schema's consumer-field slug.
                let rt = project_str(&projected, "agent_runtime_id")?;
                let work_id = project_str(&projected, "work_id")?;
                let origin_slug = project_str(&projected, "origin")?;
                let origin = parse_work_origin(origin_slug)?;
                mm_dsl::MeerkatMachineInput::Ingest {
                    runtime_id: mm_dsl::AgentRuntimeId::from(rt.to_string()),
                    work_id: mm_dsl::WorkId::from(work_id.to_string()),
                    origin,
                }
            }
            "Retire" => mm_dsl::MeerkatMachineInput::Retire,
            "Destroy" => mm_dsl::MeerkatMachineInput::Destroy,
            other => {
                return Err(format!(
                    "meerkat consumer surface does not accept routed input `{other}`; \
                     only PrepareBindings/Ingest/Retire/Destroy are declared in the \
                     `meerkat_mob_seam` schema",
                ));
            }
        };

        self.machine
            .apply_routed_meerkat_input(&session_id, input)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fld(slug: &str) -> FieldId {
        FieldId::parse(slug).expect("field slug")
    }

    fn iv(slug: &str) -> InputVariantId {
        InputVariantId::parse(slug).expect("input variant slug")
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
                ],
            )
            .await
            .expect_err("missing fence_token");
        assert!(err.contains("fence_token"), "{err}");
    }

    #[tokio::test]
    async fn unknown_variant_is_refused_typed() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        let err = surface
            .apply_routed_input(iv("Recycle"), vec![])
            .await
            .expect_err("Recycle is not a routed variant");
        assert!(err.contains("Recycle"), "{err}");
    }

    #[tokio::test]
    async fn unpinned_surface_requires_projected_runtime_id_for_retire() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        // Retire has no fields in the schema; an unpinned surface
        // therefore cannot resolve a session and must refuse rather
        // than pick arbitrarily.
        let err = surface
            .apply_routed_input(iv("Retire"), vec![])
            .await
            .expect_err("Retire without target");
        assert!(err.contains("agent_runtime_id"), "{err}");
    }

    #[tokio::test]
    async fn pinned_surface_rejects_mismatched_runtime_id() {
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
                ],
            )
            .await
            .expect_err("runtime id disagrees with pinned session");
        assert!(err.contains("pinned"), "{err}");
    }
}
