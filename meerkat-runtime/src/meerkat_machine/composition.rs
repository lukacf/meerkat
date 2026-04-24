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
        variant: &str,
        projected: &[(FieldId, OwnedFieldValue)],
    ) -> Result<SessionId, String> {
        // Typed session_id is the canonical source (Shape 4 — producer DSL
        // emits `session_id: SessionId` alongside `agent_runtime_id`; see
        // `MobMachineEffect::RequestRuntimeBinding` in
        // `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs:168`).
        let projected_session_id = projected
            .iter()
            .find(|(id, _)| id.as_str() == "session_id")
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
            (None, None) if variant == "Ingest" => {
                let runtime_id = project_str(projected, "runtime_id")?;
                self.machine
                    .resolve_registered_session_for_runtime_id(&mm_dsl::AgentRuntimeId::from(
                        runtime_id.to_string(),
                    ))
                    .await
            }
            (None, None) => Err(
                "routed input did not project `session_id` and surface is not pinned \
                 to a session — no session can be resolved"
                    .into(),
            ),
        }
    }
}

impl MeerkatMachine {
    async fn resolve_registered_session_for_runtime_id(
        &self,
        runtime_id: &mm_dsl::AgentRuntimeId,
    ) -> Result<SessionId, String> {
        let sessions = self.sessions.read().await;
        let mut matches = Vec::new();

        for (session_id, entry) in sessions.iter() {
            let authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if authority.state.active_runtime_id.as_ref() == Some(runtime_id) {
                matches.push(session_id.clone());
            }
        }

        match matches.len() {
            0 => Err(format!(
                "routed Ingest runtime_id `{}` did not match any registered session active_runtime_id",
                runtime_id.0
            )),
            1 => Ok(matches.remove(0)),
            count => Err(format!(
                "routed Ingest runtime_id `{}` matched {count} registered sessions; refusing ambiguous delivery",
                runtime_id.0
            )),
        }
    }
}

#[allow(clippy::panic)]
fn meerkat_instance_id() -> &'static MachineInstanceId {
    static ID: OnceLock<MachineInstanceId> = OnceLock::new();
    ID.get_or_init(|| match MachineInstanceId::parse("meerkat") {
        Ok(id) => id,
        Err(err) => unreachable!("canonical consumer instance slug rejected: {err}"),
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
        let variant_slug = variant.as_str();
        let session_id = self.resolve_session(variant_slug, &projected).await?;
        let input = match variant_slug {
            "PrepareBindings" => {
                let rt = project_str(&projected, "agent_runtime_id")?;
                let fence = project_u64(&projected, "fence_token")?;
                let gen_ = project_u64(&projected, "generation")?;
                let sid = project_str(&projected, "session_id")?;
                mm_dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: mm_dsl::AgentRuntimeId::from(rt.to_string()),
                    fence_token: mm_dsl::FenceToken(fence),
                    generation: mm_dsl::Generation(gen_),
                    session_id: mm_dsl::SessionId::from(sid.to_string()),
                }
            }
            "Ingest" => {
                // Route binding `work_request_reaches_meerkat` delivers
                // producer `agent_runtime_id` into the consumer's canonical
                // `runtime_id` field; producer `work_id` → consumer
                // `work_id`; producer `origin` → consumer `origin`.
                let rt = project_str(&projected, "runtime_id")?;
                let work_id = project_str(&projected, "work_id")?;
                let origin_slug = project_str(&projected, "origin")?;
                let origin = parse_work_origin(origin_slug)?;
                mm_dsl::MeerkatMachineInput::Ingest {
                    runtime_id: mm_dsl::AgentRuntimeId::from(rt.to_string()),
                    work_id: mm_dsl::WorkId::from(work_id.to_string()),
                    origin,
                }
            }
            "Retire" => {
                let sid = project_str(&projected, "session_id")?;
                mm_dsl::MeerkatMachineInput::Retire {
                    session_id: mm_dsl::SessionId::from(sid.to_string()),
                }
            }
            "Destroy" => {
                let sid = project_str(&projected, "session_id")?;
                mm_dsl::MeerkatMachineInput::Destroy {
                    session_id: mm_dsl::SessionId::from(sid.to_string()),
                }
            }
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
        assert!(err.contains("fence_token"), "{err}");
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
        assert!(err.contains("Recycle"), "{err}");
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
        assert!(err.contains("session_id"), "{err}");
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
        assert!(err.contains("pinned"), "{err}");
    }

    #[tokio::test]
    async fn ingest_prefers_projected_session_id() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        let session_id = sid("00000000-0000-0000-0000-000000000001");
        machine.register_session(session_id.clone()).await;

        surface
            .apply_routed_input(
                iv("Ingest"),
                vec![
                    (fld("runtime_id"), OwnedFieldValue::Str("rt-other".into())),
                    (fld("work_id"), OwnedFieldValue::Str("work-1".into())),
                    (fld("origin"), OwnedFieldValue::Str("Ingest".into())),
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
    async fn ingest_resolves_session_from_runtime_id_when_session_id_absent() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        let session_id = sid("00000000-0000-0000-0000-000000000001");
        machine.register_session(session_id.clone()).await;
        bind_runtime(&surface, &session_id, "rt-match").await;

        surface
            .apply_routed_input(
                iv("Ingest"),
                vec![
                    (fld("runtime_id"), OwnedFieldValue::Str("rt-match".into())),
                    (fld("work_id"), OwnedFieldValue::Str("work-1".into())),
                    (fld("origin"), OwnedFieldValue::Str("Ingest".into())),
                ],
            )
            .await
            .expect("runtime_id resolves to the registered session");
    }

    #[tokio::test]
    async fn ingest_without_matching_runtime_id_is_refused() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        machine
            .register_session(sid("00000000-0000-0000-0000-000000000001"))
            .await;

        let err = surface
            .apply_routed_input(
                iv("Ingest"),
                vec![
                    (fld("runtime_id"), OwnedFieldValue::Str("rt-missing".into())),
                    (fld("work_id"), OwnedFieldValue::Str("work-1".into())),
                    (fld("origin"), OwnedFieldValue::Str("Ingest".into())),
                ],
            )
            .await
            .expect_err("runtime_id has no registered owner");
        assert!(
            err.contains("did not match any registered session"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn ingest_with_ambiguous_runtime_id_is_refused() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let surface = MeerkatConsumerSurface::new(Arc::clone(&machine));
        let first = sid("00000000-0000-0000-0000-000000000001");
        let second = sid("00000000-0000-0000-0000-000000000002");
        machine.register_session(first.clone()).await;
        machine.register_session(second.clone()).await;
        bind_runtime(&surface, &first, "rt-shared").await;
        bind_runtime(&surface, &second, "rt-shared").await;

        let err = surface
            .apply_routed_input(
                iv("Ingest"),
                vec![
                    (fld("runtime_id"), OwnedFieldValue::Str("rt-shared".into())),
                    (fld("work_id"), OwnedFieldValue::Str("work-1".into())),
                    (fld("origin"), OwnedFieldValue::Str("Ingest".into())),
                ],
            )
            .await
            .expect_err("runtime_id is ambiguous");
        assert!(err.contains("ambiguous delivery"), "{err}");
    }
}
