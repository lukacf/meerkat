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
//! [`CompositionBinding`][cb] discriminant in `meerkat-runtime`. The mob
//! producer now carries the canonical DSL `MobMachineEffect` directly across
//! the composition seam; this module only supplies the dispatcher trait
//! projection that turns schema-declared [`FieldId`] bindings into typed
//! [`FieldValue`]s.
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
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_machine_schema::identity::{
    CompositionId, EffectVariantId, FieldId, MachineId, MachineInstanceId, SignalVariantId,
};
use meerkat_runtime::composition::{
    CatalogCompositionSignalDispatcher, CompositionBinding, CompositionDispatcher, DispatchOutcome,
    DispatchRefusal, EffectPayload, FieldValue, OwnedFieldValue, ProducerEffect, ProducerInstance,
    RouteTable, SignalConsumerSurface,
};
use meerkat_runtime::generated::meerkat_mob_seam as seam_facts;
use meerkat_runtime::meerkat_machine::dsl as meerkat_dsl;
use std::sync::Arc;
use tokio::sync::mpsc;

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
    seam_facts::composition_id()
}

/// Producer instance slug for the mob participant — `mob`.
pub(crate) fn mob_producer_instance_id() -> MachineInstanceId {
    seam_facts::producers::mob_instance_id()
}

/// Machine id for the `mob` participant — `MobMachine`.
pub(crate) fn mob_machine_id() -> MachineId {
    seam_facts::producers::mob_machine_id()
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
/// The variant payload is the canonical DSL effect emitted by
/// `MobMachine`; do not introduce a second producer-effect mirror here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobSeamEffect {
    /// Producer `mob` emitted an effect body from the canonical machine.
    Mob(mob_dsl::MobMachineEffect),
}

impl MobSeamEffect {
    /// Typed [`EffectVariantId`] for this producer body. Matches the
    /// effect-variant slugs declared on `MobMachine`'s schema and the
    /// `meerkat_mob_seam` route declarations.
    pub fn variant_id(&self) -> EffectVariantId {
        match self {
            Self::Mob(mob_dsl::MobMachineEffect::RequestRuntimeBinding { .. }) => {
                seam_facts::effects::mob::request_runtime_binding()
            }
            Self::Mob(mob_dsl::MobMachineEffect::RequestRuntimeIngress { .. }) => {
                seam_facts::effects::mob::request_runtime_ingress()
            }
            Self::Mob(mob_dsl::MobMachineEffect::RequestRuntimeRetire { .. }) => {
                seam_facts::effects::mob::request_runtime_retire()
            }
            Self::Mob(mob_dsl::MobMachineEffect::RequestRuntimeDestroy { .. }) => {
                seam_facts::effects::mob::request_runtime_destroy()
            }
            Self::Mob(other) => unreachable!("non-routed mob effect reached seam: {other:?}"),
        }
    }

    pub fn generated_input_route(&self) -> Option<seam_facts::TypedRoutedInput> {
        seam_facts::route_to_input(&mob_producer_instance_id(), &self.variant_id())
    }

    fn field(&self, id: &FieldId) -> Option<FieldValue<'_>> {
        match self {
            Self::Mob(mob_dsl::MobMachineEffect::RequestRuntimeBinding {
                agent_identity: _,
                agent_runtime_id,
                fence_token,
                generation,
                session_id,
            }) => {
                if id == &seam_facts::fields::agent_runtime_id() {
                    Some(FieldValue::Str(agent_runtime_id.as_str()))
                } else if id == &seam_facts::fields::fence_token() {
                    Some(FieldValue::U64(fence_token.0))
                } else if id == &seam_facts::fields::generation() {
                    Some(FieldValue::U64(generation.0))
                } else if id == &seam_facts::fields::session_id() {
                    Some(FieldValue::Str(session_id.0.as_str()))
                } else {
                    None
                }
            }
            Self::Mob(mob_dsl::MobMachineEffect::RequestRuntimeIngress {
                agent_runtime_id,
                fence_token,
                work_id,
                origin,
            }) => {
                if id == &seam_facts::fields::agent_runtime_id() {
                    Some(FieldValue::Str(agent_runtime_id.as_str()))
                } else if id == &seam_facts::fields::fence_token() {
                    Some(FieldValue::U64(fence_token.0))
                } else if id == &seam_facts::fields::work_id() {
                    Some(FieldValue::Str(work_id.0.as_str()))
                } else if id == &seam_facts::fields::origin() {
                    Some(FieldValue::Opaque(Arc::new(meerkat_work_origin(origin))))
                } else {
                    None
                }
            }
            Self::Mob(mob_dsl::MobMachineEffect::RequestRuntimeRetire { session_id }) => {
                if id == &seam_facts::fields::session_id() {
                    Some(FieldValue::Str(session_id.0.as_str()))
                } else {
                    None
                }
            }
            Self::Mob(mob_dsl::MobMachineEffect::RequestRuntimeDestroy { session_id }) => {
                if id == &seam_facts::fields::session_id() {
                    Some(FieldValue::Str(session_id.0.as_str()))
                } else {
                    None
                }
            }
            Self::Mob(_) => None,
        }
    }
}

fn meerkat_work_origin(origin: &mob_dsl::WorkOrigin) -> meerkat_dsl::WorkOrigin {
    match origin {
        mob_dsl::WorkOrigin::External => meerkat_dsl::WorkOrigin::External,
        mob_dsl::WorkOrigin::Internal => meerkat_dsl::WorkOrigin::Internal,
        mob_dsl::WorkOrigin::Ingest => meerkat_dsl::WorkOrigin::Ingest,
    }
}

impl ProducerEffect for MobSeamEffect {
    fn variant_id(&self) -> EffectVariantId {
        MobSeamEffect::variant_id(self)
    }

    fn field(&self, id: &FieldId) -> Option<FieldValue<'_>> {
        MobSeamEffect::field(self, id)
    }
}

/// Lift a routed `MobMachineEffect::Request*` variant into the typed
/// seam-effect sum. Returns `None` for every non-routed variant (persist,
/// notice, topology signal, etc.) — those stay on the in-process
/// effect-drain path and never cross the composition seam.
pub fn lift_routed_effect(effect: &mob_dsl::MobMachineEffect) -> Option<MobSeamEffect> {
    use mob_dsl::MobMachineEffect as DslEffect;
    match effect {
        DslEffect::RequestRuntimeBinding { .. }
        | DslEffect::RequestRuntimeIngress { .. }
        | DslEffect::RequestRuntimeRetire { .. }
        | DslEffect::RequestRuntimeDestroy { .. } => Some(MobSeamEffect::Mob(effect.clone())),
        _ => None,
    }
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

/// Attach the MeerkatMachine -> MobMachine typed signal dispatcher to the
/// shared runtime adapter. This is the reverse direction of
/// [`wired_binding_from_runtime_adapter`]: MeerkatMachine is the producer
/// of RuntimeBound/RuntimeRetired/RuntimeDestroyed lifecycle effects and
/// the mob actor is the signal consumer.
#[cfg(feature = "runtime-adapter")]
pub(super) fn attach_signal_dispatcher_to_runtime_adapter(
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    command_tx: mpsc::Sender<super::state::MobCommand>,
) {
    let schema = meerkat_machine_schema::catalog::meerkat_mob_seam_composition();
    let table = RouteTable::from_schema(&schema)
        .expect("meerkat_mob_seam schema is well-formed by construction");
    let consumer = Arc::new(MobSignalConsumerSurface::new(command_tx));
    let dispatcher: CatalogCompositionSignalDispatcher<
        meerkat_runtime::meerkat_machine::composition::MeerkatSeamSignal,
    > = CatalogCompositionSignalDispatcher::new(schema.name.clone(), table).with_consumer(consumer);
    runtime_adapter.set_composition_signal_dispatcher(Arc::new(dispatcher));
}

#[cfg(feature = "runtime-adapter")]
struct MobSignalConsumerSurface {
    command_tx: mpsc::Sender<super::state::MobCommand>,
    instance_id: MachineInstanceId,
}

#[cfg(feature = "runtime-adapter")]
impl MobSignalConsumerSurface {
    fn new(command_tx: mpsc::Sender<super::state::MobCommand>) -> Self {
        Self {
            command_tx,
            instance_id: mob_producer_instance_id(),
        }
    }
}

#[cfg(feature = "runtime-adapter")]
fn signal_project_str<'a>(
    fields: &'a [(FieldId, OwnedFieldValue)],
    field: &FieldId,
) -> Result<&'a str, String> {
    fields
        .iter()
        .find(|(id, _)| id == field)
        .ok_or_else(|| format!("missing projected signal field `{}`", field.as_str()))
        .and_then(|(_, value)| match value {
            OwnedFieldValue::Str(value) => Ok(value.as_str()),
            other => Err(format!(
                "projected signal field `{}` is not Str: {other:?}",
                field.as_str()
            )),
        })
}

#[cfg(feature = "runtime-adapter")]
fn signal_project_u64(
    fields: &[(FieldId, OwnedFieldValue)],
    field: &FieldId,
) -> Result<u64, String> {
    fields
        .iter()
        .find(|(id, _)| id == field)
        .ok_or_else(|| format!("missing projected signal field `{}`", field.as_str()))
        .and_then(|(_, value)| match value {
            OwnedFieldValue::U64(value) => Ok(*value),
            other => Err(format!(
                "projected signal field `{}` is not U64: {other:?}",
                field.as_str()
            )),
        })
}

#[cfg(feature = "runtime-adapter")]
fn build_mob_signal(
    variant: &SignalVariantId,
    projected: &[(FieldId, OwnedFieldValue)],
) -> Result<mob_dsl::MobMachineSignal, String> {
    let runtime_id = mob_dsl::AgentRuntimeId::from(
        signal_project_str(projected, &seam_facts::fields::agent_runtime_id())?.to_string(),
    );
    let fence_token = mob_dsl::FenceToken(signal_project_u64(
        projected,
        &seam_facts::fields::fence_token(),
    )?);
    if variant == &seam_facts::signals::observe_runtime_ready() {
        Ok(mob_dsl::MobMachineSignal::ObserveRuntimeReady {
            agent_runtime_id: runtime_id,
            fence_token,
        })
    } else if variant == &seam_facts::signals::observe_runtime_retired() {
        Ok(mob_dsl::MobMachineSignal::ObserveRuntimeRetired {
            agent_runtime_id: runtime_id,
            fence_token,
        })
    } else if variant == &seam_facts::signals::observe_runtime_destroyed() {
        Ok(mob_dsl::MobMachineSignal::ObserveRuntimeDestroyed {
            agent_runtime_id: runtime_id,
            fence_token,
        })
    } else {
        Err(format!(
            "mob signal consumer surface does not accept routed signal `{other}`; \
             only ObserveRuntimeReady/ObserveRuntimeRetired/ObserveRuntimeDestroyed are declared",
            other = variant.as_str()
        ))
    }
}

#[cfg(feature = "runtime-adapter")]
#[async_trait::async_trait]
impl SignalConsumerSurface for MobSignalConsumerSurface {
    fn instance_id(&self) -> &MachineInstanceId {
        &self.instance_id
    }

    async fn receive_signal(
        &self,
        variant: SignalVariantId,
        projected_fields: Vec<(FieldId, OwnedFieldValue)>,
    ) -> Result<(), String> {
        let signal = build_mob_signal(&variant, &projected_fields)?;
        self.command_tx
            .try_send(super::state::MobCommand::ProjectMachineSignal { signal })
            .map_err(|error| format!("mob actor refused signal enqueue: {error}"))
    }
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
        let body = mob_dsl::MobMachineEffect::RequestRuntimeBinding {
            agent_identity: mob_dsl::AgentIdentity::from("agent"),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from("rt-1"),
            fence_token: mob_dsl::FenceToken(7),
            generation: mob_dsl::Generation(3),
            session_id: mob_dsl::SessionId::from("session-1"),
        };
        assert_eq!(
            MobSeamEffect::Mob(body).variant_id(),
            ev("RequestRuntimeBinding"),
        );
    }

    #[test]
    fn request_runtime_binding_projects_all_route_field_bindings() {
        let body = mob_dsl::MobMachineEffect::RequestRuntimeBinding {
            agent_identity: mob_dsl::AgentIdentity::from("agent"),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from("rt-1"),
            fence_token: mob_dsl::FenceToken(7),
            generation: mob_dsl::Generation(3),
            session_id: mob_dsl::SessionId::from("session-1"),
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
    fn routed_mob_effect_projection_tracks_generated_route_facts() {
        use meerkat_runtime::generated::meerkat_mob_seam as seam_facts;

        let cases = vec![
            (
                MobSeamEffect::Mob(mob_dsl::MobMachineEffect::RequestRuntimeBinding {
                    agent_identity: mob_dsl::AgentIdentity::from("agent"),
                    agent_runtime_id: mob_dsl::AgentRuntimeId::from("rt-1"),
                    fence_token: mob_dsl::FenceToken(7),
                    generation: mob_dsl::Generation(3),
                    session_id: mob_dsl::SessionId::from("session-1"),
                }),
                seam_facts::route_binding_request_reaches_meerkat(),
            ),
            (
                MobSeamEffect::Mob(mob_dsl::MobMachineEffect::RequestRuntimeIngress {
                    agent_runtime_id: mob_dsl::AgentRuntimeId::from("rt-1"),
                    fence_token: mob_dsl::FenceToken(7),
                    work_id: mob_dsl::WorkId::from("work-1"),
                    origin: mob_dsl::WorkOrigin::External,
                }),
                seam_facts::route_work_request_reaches_meerkat(),
            ),
            (
                MobSeamEffect::Mob(mob_dsl::MobMachineEffect::RequestRuntimeRetire {
                    session_id: mob_dsl::SessionId::from("session-1"),
                }),
                seam_facts::route_retire_request_reaches_meerkat(),
            ),
            (
                MobSeamEffect::Mob(mob_dsl::MobMachineEffect::RequestRuntimeDestroy {
                    session_id: mob_dsl::SessionId::from("session-1"),
                }),
                seam_facts::route_destroy_request_reaches_meerkat(),
            ),
        ];

        for (effect, expected_route) in cases {
            let route = effect.generated_input_route().expect("generated route");
            assert_eq!(route, expected_route);
            for (producer_field, _) in &route.bindings {
                assert!(
                    effect.field(producer_field).is_some(),
                    "generated route `{}` requires producer field `{}`",
                    route.route_id.as_str(),
                    producer_field.as_str()
                );
            }
        }
    }

    #[test]
    fn retire_and_destroy_have_no_fields() {
        let retire = MobSeamEffect::Mob(mob_dsl::MobMachineEffect::RequestRuntimeRetire {
            session_id: mob_dsl::SessionId::from("019dbd3d-d7ad-75a1-96d0-8013927e78f8"),
        });
        let destroy = MobSeamEffect::Mob(mob_dsl::MobMachineEffect::RequestRuntimeDestroy {
            session_id: mob_dsl::SessionId::from("019dbd3d-d7ad-75a1-96d0-8013927e78f8"),
        });
        assert_eq!(retire.variant_id(), ev("RequestRuntimeRetire"));
        assert_eq!(destroy.variant_id(), ev("RequestRuntimeDestroy"));
        assert!(retire.field(&fid("agent_runtime_id")).is_none());
        assert!(destroy.field(&fid("agent_runtime_id")).is_none());
    }

    #[test]
    fn ingress_exposes_schema_declared_producer_fields() {
        let body = mob_dsl::MobMachineEffect::RequestRuntimeIngress {
            agent_runtime_id: mob_dsl::AgentRuntimeId::from("rt-x"),
            fence_token: mob_dsl::FenceToken(1),
            work_id: mob_dsl::WorkId::from("w-1"),
            origin: mob_dsl::WorkOrigin::External,
        };
        let effect = MobSeamEffect::Mob(body);

        assert!(matches!(
            effect
                .field(&fid("agent_runtime_id"))
                .expect("agent_runtime_id"),
            FieldValue::Str("rt-x"),
        ));
        assert!(effect.field(&fid("runtime_id")).is_none());
        match effect.field(&fid("origin")).expect("origin") {
            FieldValue::Opaque(value) => assert!(matches!(
                value.downcast_ref::<meerkat_dsl::WorkOrigin>(),
                Some(meerkat_dsl::WorkOrigin::External)
            )),
            other => panic!("origin should stay typed, got {other:?}"),
        }
    }

    #[test]
    fn lift_routes_only_routed_request_variants() {
        use mob_dsl::MobMachineEffect as DslEffect;

        let binding_in = DslEffect::RequestRuntimeBinding {
            agent_identity: mob_dsl::AgentIdentity::from("a"),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from("rt"),
            fence_token: mob_dsl::FenceToken(1),
            generation: mob_dsl::Generation(0),
            session_id: mob_dsl::SessionId::from("session-1"),
        };
        assert!(matches!(
            lift_routed_effect(&binding_in),
            Some(MobSeamEffect::Mob(
                mob_dsl::MobMachineEffect::RequestRuntimeBinding { .. }
            )),
        ));

        let retire_in = DslEffect::RequestRuntimeRetire {
            session_id: mob_dsl::SessionId::from("019dbd3d-d7ad-75a1-96d0-8013927e78f8"),
        };
        assert!(matches!(
            lift_routed_effect(&retire_in),
            Some(MobSeamEffect::Mob(
                mob_dsl::MobMachineEffect::RequestRuntimeRetire { .. }
            )),
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
        let effect = MobSeamEffect::Mob(mob_dsl::MobMachineEffect::RequestRuntimeRetire {
            session_id: mob_dsl::SessionId::from("019dbd3d-d7ad-75a1-96d0-8013927e78f8"),
        });
        let outcome = dispatch_routed_effect(&binding, effect)
            .await
            .expect("standalone is not an error");
        assert!(
            outcome.is_none(),
            "standalone dispatcher performs no routing"
        );
    }
}
