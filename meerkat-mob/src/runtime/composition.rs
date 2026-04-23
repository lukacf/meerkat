//! Wave-c C-6p — producer side of the `meerkat_mob_seam` composition.
//!
//! The mob kernel (`MobMachine`) emits four routed effects on the
//! `meerkat_mob_seam` composition:
//!
//! * `RequestRuntimeBinding` — producer `mob`, consumer `meerkat.PrepareBindings`
//! * `RequestRuntimeIngress` — producer `mob`, consumer `meerkat.Ingest`
//! * `RequestRuntimeRetire`  — producer `mob`, consumer `meerkat.Retire`
//! * `RequestRuntimeDestroy` — producer `mob`, consumer `meerkat.Destroy`
//!
//! Wave-b B-5 landed the typed [`CompositionDispatcher`][cd] trait +
//! [`CompositionBinding`][cb] discriminant in `meerkat-runtime`. Wave-c
//! C-6p wires the producer end: the actor converts each emitted
//! `MobMachineEffect::Request*` variant into a typed [`MobSeamEffect`]
//! payload and routes it through the dispatcher instead of silently
//! dropping the routed-effect slot on the effect-drain path.
//!
//! `MobSeamEffect` is hand-authored here to match the shape that
//! [`render_composition_driver`][rcd] emits from the driver declaration
//! added to `meerkat_mob_seam_composition()` in this same task. When the
//! workspace-wide codegen cascade re-enables (see wave-c-prep plan:
//! serial spine `C-6p → C-6c → C-6r`), C-6c / C-6r can opt into the
//! generated producer enum at the dispatcher wire-up site without
//! changing this module's interface — the variant/field shape is
//! identical and the `ProducerEffect` impl projects the same
//! [`FieldId`] → [`FieldValue`] bindings the composition route declares.
//!
//! The consumer side (`MeerkatMachine` implementing [`ConsumerSurface`][cs])
//! lands with task `#5` (C-6c). Until then,
//! [`CatalogCompositionDispatcher`][ccd] will resolve the typed route and
//! return [`DispatchRefusal::UnwiredConsumer`][dr]; the dispatch helper
//! in [`dispatch_routed_effect`] propagates that as a typed [`MobError`]
//! rather than silently dropping the effect.
//!
//! [cd]: meerkat_runtime::composition::CompositionDispatcher
//! [cb]: meerkat_runtime::composition::CompositionBinding
//! [cs]: meerkat_runtime::composition::ConsumerSurface
//! [ccd]: meerkat_runtime::composition::CatalogCompositionDispatcher
//! [dr]: meerkat_runtime::composition::DispatchRefusal
//! [rcd]: https://docs.rs/meerkat_machine_codegen (render_composition_driver)

use crate::error::MobError;
use crate::machines::mob_machine as mob_dsl;
use meerkat_machine_schema::identity::{
    CompositionId, EffectVariantId, FieldId, MachineId, MachineInstanceId,
};
use meerkat_runtime::composition::{
    CompositionBinding, CompositionDispatcher, DispatchOutcome, DispatchRefusal, EffectPayload,
    FieldValue, ProducerEffect, ProducerInstance,
};
use std::sync::Arc;

/// Typed handle to a `meerkat_mob_seam` composition dispatcher.
///
/// This is the typed replacement for string-keyed driver declarations.
/// The underlying trait object is parameterised over [`MobSeamEffect`]
/// — the producer's seam-effect sum — so dispatch cannot be invoked
/// with a foreign effect type at compile time.
pub type CompositionDispatcherHandle = Arc<dyn CompositionDispatcher<Effect = MobSeamEffect>>;

/// Typed composition binding attached to the mob actor.
///
/// Monomorphised over [`MobSeamEffect`] so the two constructor halves —
/// [`CompositionBinding::Standalone`] (test / single-machine path) vs
/// [`CompositionBinding::Wired`] (production path with a dispatcher) —
/// stay explicit at every call site inside mob.
pub type MobCompositionBinding = CompositionBinding<MobSeamEffect>;

/// Composition slug — `meerkat_mob_seam`.
pub(crate) fn mob_seam_composition_id() -> CompositionId {
    CompositionId::parse("meerkat_mob_seam").expect("canonical composition slug")
}

/// Producer instance slug for the mob participant — `mob`.
pub(crate) fn mob_producer_instance_id() -> MachineInstanceId {
    MachineInstanceId::parse("mob").expect("canonical instance slug")
}

/// Machine id for the `mob` participant — `MobMachine`.
pub(crate) fn mob_machine_id() -> MachineId {
    MachineId::parse("MobMachine").expect("canonical machine id")
}

/// Construct the typed [`ProducerInstance`] handle for the mob side of
/// the seam. Kept as a helper so every dispatch site uses the same slugs.
pub fn mob_producer_instance() -> ProducerInstance {
    ProducerInstance {
        composition: mob_seam_composition_id(),
        instance_id: mob_producer_instance_id(),
        machine: mob_machine_id(),
    }
}

/// Seam-effect sum for the `meerkat_mob_seam` composition, producer side.
///
/// One variant per distinct producer instance participating in the
/// composition. Today that is just the `mob` producer (the composition
/// also declares a `meerkat` producer for signal-kind routes, which the
/// dispatcher excludes — signals are the signal surface's concern).
///
/// Matches the codegen-emitted shape asserted by
/// `meerkat-machine-codegen/tests/routed_effect_module_shape.rs` so the
/// downstream switch-over (C-6c / C-6r) is a literal rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobSeamEffect {
    /// Producer `mob` emitted an effect body.
    Mob(MobProducerEffect),
}

/// Producer-side effect body for the `mob` instance.
///
/// Mirrors the four routed `MobMachineEffect::Request*` variants from
/// the mob DSL at `meerkat-mob/src/machines/mob_machine.rs:1014-1018`.
/// Field names match the composition schema's route bindings verbatim
/// so [`ProducerEffect::field`] can look them up by [`FieldId`] without
/// a translation table. Values live as DSL-bridging types (the flat
/// string/u64 forms) — consumers rehydrate them into domain types via
/// the `ConsumerSurface` impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobProducerEffect {
    /// Route `binding_request_reaches_meerkat` — target
    /// `meerkat.PrepareBindings`.
    RequestRuntimeBinding {
        agent_identity: mob_dsl::AgentIdentity,
        agent_runtime_id: mob_dsl::AgentRuntimeId,
        fence_token: mob_dsl::FenceToken,
        generation: mob_dsl::Generation,
    },
    /// Route `work_request_reaches_meerkat` — target `meerkat.Ingest`.
    /// The route binds producer `runtime_id` to consumer
    /// `agent_runtime_id`, producer `work_id` to consumer `work_id`,
    /// and producer `origin` to consumer `origin`.
    RequestRuntimeIngress {
        agent_runtime_id: mob_dsl::AgentRuntimeId,
        fence_token: mob_dsl::FenceToken,
        work_id: mob_dsl::WorkId,
        origin: mob_dsl::WorkOrigin,
    },
    /// Route `retire_request_reaches_meerkat` — target `meerkat.Retire`.
    /// Carries no fields (the retire target is implied by the currently
    /// bound member; the consumer surface resolves it from its own state).
    RequestRuntimeRetire,
    /// Route `destroy_request_reaches_meerkat` — target `meerkat.Destroy`.
    /// Carries no fields (same implicit-target pattern as Retire).
    RequestRuntimeDestroy,
}

impl MobProducerEffect {
    /// Typed [`EffectVariantId`] for this producer body. Matches the
    /// effect-variant slugs declared on `MobMachine`'s schema and the
    /// `meerkat_mob_seam` route declarations.
    pub fn variant_id(&self) -> EffectVariantId {
        let slug = match self {
            Self::RequestRuntimeBinding { .. } => "RequestRuntimeBinding",
            Self::RequestRuntimeIngress { .. } => "RequestRuntimeIngress",
            Self::RequestRuntimeRetire => "RequestRuntimeRetire",
            Self::RequestRuntimeDestroy => "RequestRuntimeDestroy",
        };
        EffectVariantId::parse(slug).expect("producer effect slug is hand-authored constant")
    }

    fn field(&self, id: &FieldId) -> Option<FieldValue<'_>> {
        match self {
            Self::RequestRuntimeBinding {
                agent_identity: _,
                agent_runtime_id,
                fence_token,
                generation,
            } => match id.as_str() {
                "agent_runtime_id" => Some(FieldValue::Str(agent_runtime_id.as_str())),
                "fence_token" => Some(FieldValue::U64(fence_token.0)),
                "generation" => Some(FieldValue::U64(generation.0)),
                _ => None,
            },
            Self::RequestRuntimeIngress {
                agent_runtime_id,
                fence_token,
                work_id,
                origin,
            } => match id.as_str() {
                // The composition's route binding for
                // `work_request_reaches_meerkat` projects producer field
                // `runtime_id` → consumer field `agent_runtime_id`. Surface
                // both slugs against the same backing runtime-id so the
                // dispatcher's field-binding walk resolves whether the
                // caller asks by route-producer name (`runtime_id`) or by
                // consumer-field name (`agent_runtime_id`).
                "agent_runtime_id" | "runtime_id" => {
                    Some(FieldValue::Str(agent_runtime_id.as_str()))
                }
                "fence_token" => Some(FieldValue::U64(fence_token.0)),
                "work_id" => Some(FieldValue::Str(work_id.0.as_str())),
                "origin" => Some(FieldValue::Str(work_origin_slug(origin))),
                _ => None,
            },
            Self::RequestRuntimeRetire | Self::RequestRuntimeDestroy => None,
        }
    }
}

/// Stable slug for a DSL [`mob_dsl::WorkOrigin`] used as the routed-effect
/// field value. The DSL enum carries an `Ingest` variant the mob side
/// never produces (it models the meerkat-side admission-kind), so emit a
/// distinct slug for it — the consumer surface rejects it explicitly if
/// it ever appears here, which would be a DSL bug rather than a silent
/// drop.
fn work_origin_slug(origin: &mob_dsl::WorkOrigin) -> &'static str {
    match origin {
        mob_dsl::WorkOrigin::External => "External",
        mob_dsl::WorkOrigin::Internal => "Internal",
        mob_dsl::WorkOrigin::Ingest => "Ingest",
    }
}

impl ProducerEffect for MobSeamEffect {
    fn variant_id(&self) -> EffectVariantId {
        match self {
            Self::Mob(body) => body.variant_id(),
        }
    }

    fn field(&self, id: &FieldId) -> Option<FieldValue<'_>> {
        match self {
            Self::Mob(body) => body.field(id),
        }
    }
}

/// Lift a routed `MobMachineEffect::Request*` variant into the typed
/// seam-effect sum. Returns `None` for every non-routed variant (persist,
/// notice, topology signal, etc.) — those stay on the in-process
/// effect-drain path and never cross the composition seam.
pub fn lift_routed_effect(effect: &mob_dsl::MobMachineEffect) -> Option<MobSeamEffect> {
    use mob_dsl::MobMachineEffect as DslEffect;
    let body = match effect {
        DslEffect::RequestRuntimeBinding {
            agent_identity,
            agent_runtime_id,
            fence_token,
            generation,
        } => MobProducerEffect::RequestRuntimeBinding {
            agent_identity: agent_identity.clone(),
            agent_runtime_id: agent_runtime_id.clone(),
            fence_token: *fence_token,
            generation: *generation,
        },
        DslEffect::RequestRuntimeIngress {
            agent_runtime_id,
            fence_token,
            work_id,
            origin,
        } => MobProducerEffect::RequestRuntimeIngress {
            agent_runtime_id: agent_runtime_id.clone(),
            fence_token: *fence_token,
            work_id: work_id.clone(),
            origin: *origin,
        },
        DslEffect::RequestRuntimeRetire => MobProducerEffect::RequestRuntimeRetire,
        DslEffect::RequestRuntimeDestroy => MobProducerEffect::RequestRuntimeDestroy,
        _ => return None,
    };
    Some(MobSeamEffect::Mob(body))
}

/// Wave-c C-6c — build a production [`MobCompositionBinding`] that
/// routes mob-emitted routed effects into the meerkat consumer surface
/// installed on `runtime_adapter`.
///
/// This is the single constructor site that flips mob assembly from
/// `CompositionBinding::Standalone` (the default that returned
/// [`DispatchRefusal::UnwiredConsumer`] on every dispatch during
/// wave-c's intermediate state) to `CompositionBinding::Wired(_)` with
/// a [`meerkat_runtime::composition::CatalogCompositionDispatcher`]
/// carrying the typed `meerkat_mob_seam`
/// [`meerkat_runtime::composition::RouteTable`] and the runtime-side
/// consumer surface. The builder helper below is feature-gated on
/// `runtime-adapter` and returns `None` on the no-adapter build so
/// callers can fall through to `CompositionBinding::Standalone`.
#[cfg(feature = "runtime-adapter")]
pub fn wired_binding_from_runtime_adapter(
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
) -> MobCompositionBinding {
    use meerkat_runtime::composition::{
        CatalogCompositionDispatcher, CompositionBinding, RouteTable,
    };
    let schema = meerkat_machine_schema::catalog::meerkat_mob_seam_composition();
    // The schema is hand-authored and compile-time-fixed; a failure to
    // build the route table is a schema bug, not a runtime condition.
    // `expect` mirrors the pattern used by the dispatcher's own test
    // helpers in `meerkat-runtime/src/composition/route_table.rs`.
    let table = RouteTable::from_schema(&schema)
        .expect("meerkat_mob_seam schema is well-formed by construction");
    let consumer = Arc::new(
        meerkat_runtime::meerkat_machine::composition::MeerkatConsumerSurface::new(Arc::clone(
            runtime_adapter,
        )),
    );
    let dispatcher: CatalogCompositionDispatcher<MobSeamEffect> =
        CatalogCompositionDispatcher::new(schema.name.clone(), table).with_consumer(consumer);
    CompositionBinding::Wired(Arc::new(dispatcher))
}

/// Dispatch a single routed seam effect through the mob's composition
/// binding.
///
/// * [`CompositionBinding::Wired`] — delegates to
///   [`CompositionDispatcher::dispatch`]. A [`DispatchRefusal`] is lifted
///   to [`MobError::Internal`] with the typed refusal preserved in the
///   message; `DispatchRefusal::UnwiredConsumer` is the expected
///   intermediate-state shape while C-6c has not yet installed the
///   [`meerkat_runtime::composition::ConsumerSurface`] on
///   `MeerkatMachine`, and is reported loudly here — no silent drop.
/// * [`CompositionBinding::Standalone`] — test / single-machine path.
///   The effect has no consumer to route to by construction; this helper
///   returns `Ok(None)` so the caller can log-and-continue. (Production
///   surfaces never construct `Standalone`; the builder default for
///   real mob assembly is `Wired`.)
pub async fn dispatch_routed_effect(
    binding: &MobCompositionBinding,
    effect: MobSeamEffect,
) -> Result<Option<DispatchOutcome>, MobError> {
    let Some(dispatcher) = binding.wired() else {
        return Ok(None);
    };
    let variant = effect.variant_id();
    let payload = EffectPayload::Emitted {
        variant,
        body: effect,
    };
    dispatcher
        .dispatch(mob_producer_instance(), payload)
        .await
        .map(Some)
        .map_err(dispatch_refusal_to_mob_error)
}

fn dispatch_refusal_to_mob_error(refusal: DispatchRefusal) -> MobError {
    match refusal {
        // UnwiredConsumer is the expected intermediate-state shape during
        // wave-c spine (C-6p landed, C-6c pending). Surface as the explicit
        // WiringError variant — this is a construction-time wiring bug
        // once the spine fully lands.
        DispatchRefusal::UnwiredConsumer {
            composition,
            instance,
        } => MobError::WiringError(format!(
            "composition `{composition}` has no consumer surface registered for instance `{instance}` \
             — C-6c (meerkat_runtime::composition::ConsumerSurface on MeerkatMachine) pending"
        )),
        DispatchRefusal::UnresolvedRoute {
            composition,
            instance,
            variant,
        } => MobError::WiringError(format!(
            "composition `{composition}` declares no input route for producer \
             `{instance}` effect variant `{variant}`"
        )),
        DispatchRefusal::MissingProducerField {
            route,
            variant,
            field,
        } => MobError::WiringError(format!(
            "route `{route}` requires producer field `{field}` on variant `{variant}`; \
             producer did not provide it"
        )),
        DispatchRefusal::CompositionMismatch { expected, actual } => {
            MobError::WiringError(format!(
                "dispatcher composition `{expected}` does not match producer composition `{actual}`"
            ))
        }
        DispatchRefusal::ConsumerRefused {
            instance,
            variant,
            reason,
        } => MobError::Internal(format!(
            "consumer `{instance}` refused routed input `{variant}`: {reason}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(slug: &str) -> EffectVariantId {
        EffectVariantId::parse(slug).expect("slug")
    }

    fn fid(slug: &str) -> FieldId {
        FieldId::parse(slug).expect("slug")
    }

    #[test]
    fn request_runtime_binding_variant_id_matches_schema_slug() {
        let body = MobProducerEffect::RequestRuntimeBinding {
            agent_identity: mob_dsl::AgentIdentity::from("agent"),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from("rt-1"),
            fence_token: mob_dsl::FenceToken(7),
            generation: mob_dsl::Generation(3),
        };
        assert_eq!(body.variant_id(), ev("RequestRuntimeBinding"));
        assert_eq!(
            MobSeamEffect::Mob(body).variant_id(),
            ev("RequestRuntimeBinding"),
        );
    }

    #[test]
    fn request_runtime_binding_projects_all_route_field_bindings() {
        let body = MobProducerEffect::RequestRuntimeBinding {
            agent_identity: mob_dsl::AgentIdentity::from("agent"),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from("rt-1"),
            fence_token: mob_dsl::FenceToken(7),
            generation: mob_dsl::Generation(3),
        };
        let effect = MobSeamEffect::Mob(body);

        assert!(matches!(
            effect.field(&fid("agent_runtime_id")).expect("present"),
            FieldValue::Str("rt-1"),
        ));
        assert!(matches!(
            effect.field(&fid("fence_token")).expect("present"),
            FieldValue::U64(7),
        ));
        assert!(matches!(
            effect.field(&fid("generation")).expect("present"),
            FieldValue::U64(3),
        ));
        assert!(effect.field(&fid("unknown_field")).is_none());
    }

    #[test]
    fn retire_and_destroy_have_no_fields() {
        let retire = MobSeamEffect::Mob(MobProducerEffect::RequestRuntimeRetire);
        let destroy = MobSeamEffect::Mob(MobProducerEffect::RequestRuntimeDestroy);
        assert_eq!(retire.variant_id(), ev("RequestRuntimeRetire"));
        assert_eq!(destroy.variant_id(), ev("RequestRuntimeDestroy"));
        assert!(retire.field(&fid("agent_runtime_id")).is_none());
        assert!(destroy.field(&fid("agent_runtime_id")).is_none());
    }

    #[test]
    fn ingress_exposes_both_runtime_id_and_agent_runtime_id_aliases() {
        // Route bindings carry `(to_field, from_field)` pairs; the
        // `work_request_reaches_meerkat` route translates producer field
        // `runtime_id` into consumer field `agent_runtime_id`, so the
        // producer surfaces under BOTH slugs that the route-binding walk
        // may probe.
        let body = MobProducerEffect::RequestRuntimeIngress {
            agent_runtime_id: mob_dsl::AgentRuntimeId::from("rt-x"),
            fence_token: mob_dsl::FenceToken(1),
            work_id: mob_dsl::WorkId::from("w-1"),
            origin: mob_dsl::WorkOrigin::External,
        };
        let effect = MobSeamEffect::Mob(body);

        assert!(matches!(
            effect.field(&fid("runtime_id")).expect("runtime_id alias"),
            FieldValue::Str("rt-x"),
        ));
        assert!(matches!(
            effect
                .field(&fid("agent_runtime_id"))
                .expect("agent_runtime_id alias"),
            FieldValue::Str("rt-x"),
        ));
        assert!(matches!(
            effect.field(&fid("origin")).expect("origin"),
            FieldValue::Str("External"),
        ));
    }

    #[test]
    fn lift_routes_only_routed_request_variants() {
        use mob_dsl::MobMachineEffect as DslEffect;

        let binding_in = DslEffect::RequestRuntimeBinding {
            agent_identity: mob_dsl::AgentIdentity::from("a"),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from("rt"),
            fence_token: mob_dsl::FenceToken(1),
            generation: mob_dsl::Generation(0),
        };
        assert!(matches!(
            lift_routed_effect(&binding_in),
            Some(MobSeamEffect::Mob(
                MobProducerEffect::RequestRuntimeBinding { .. }
            )),
        ));

        let retire_in = DslEffect::RequestRuntimeRetire;
        assert!(matches!(
            lift_routed_effect(&retire_in),
            Some(MobSeamEffect::Mob(MobProducerEffect::RequestRuntimeRetire)),
        ));

        // Non-routed variant: `PersistKickoffUpdate` stays on the local
        // effect-drain path.
        let local_only = DslEffect::PersistKickoffUpdate {
            member_id: "m".into(),
            phase: mob_dsl::KickoffPhase::Pending,
        };
        assert!(lift_routed_effect(&local_only).is_none());
    }

    #[tokio::test]
    async fn standalone_binding_skips_dispatch_without_error() {
        let binding: MobCompositionBinding = CompositionBinding::Standalone;
        let effect = MobSeamEffect::Mob(MobProducerEffect::RequestRuntimeRetire);
        let outcome = dispatch_routed_effect(&binding, effect)
            .await
            .expect("standalone is not an error");
        assert!(
            outcome.is_none(),
            "standalone dispatcher performs no routing"
        );
    }
}
