#![allow(
    clippy::enum_variant_names,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone,
    clippy::type_complexity,
    clippy::unwrap_used
)]

//! Composition driver / dispatcher contract (C-T §6 #10).
//!
//! The audit calls for a "name/descriptor agreement, duplicate
//! rejection, effect→watcher routing, typed decision-failure error"
//! behavioural test for the composition dispatcher, driven off the
//! real catalog schema (not a hand-written stub). This file is that
//! test — it rides the `CatalogCompositionDispatcher` built directly
//! off `meerkat_mob_seam_composition()`, the canonical schema the
//! runtime wires at startup.
//!
//! R-C coverage (adversarial review): the audit also names "mob actor
//! emitting MemberSessionBindingChanged triggers real MeerkatMachine
//! peer_projection input". Post-C-7 collapse, `MemberSessionBindingChanged`
//! has `disposition external` in the catalog — it does NOT route
//! through the seam dispatcher; it crosses the boundary via the DSL
//! `peer_projection` input path described in `meerkat-runtime/src/
//! meerkat_machine/dsl.rs:2115+` (direct_peer_endpoints ∪
//! mob_overlay_peer_endpoints with peer_projection_epoch). The
//! dispatcher seam carries the 4 Request* effects (binding / ingress
//! / retire / destroy) between mob and meerkat.  This test pins the
//! shape of those 4 routes + the typed refusal arms.
//!
//! See `docs/wave-c-prep/test-coverage-audit.md` §6 #10.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use meerkat_machine_schema::catalog::meerkat_mob_seam_composition;
use meerkat_machine_schema::identity::{
    CompositionId, EffectVariantId, FieldId, InputVariantId, MachineId, MachineInstanceId,
};
use meerkat_runtime::composition::{
    CatalogCompositionDispatcher, CompositionBinding, CompositionDispatcher, ConsumerSurface,
    DispatchRefusal, EffectPayload, FieldValue, OwnedFieldValue, ProducerEffect, ProducerInstance,
    RouteTable,
};

/// Stand-in for the codegen-emitted `MeerkatMobSeamEffect` sum — one
/// arm per producer effect variant the catalog declares (binding /
/// ingress / retire / destroy). This mirrors what
/// `render_composition_driver` would emit; writing it by hand here
/// lets the test ride the real dispatcher without depending on the
/// codegen target landing during C-T.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SeamEffect {
    RequestRuntimeBinding {
        agent_runtime_id: String,
        fence_token: u64,
        generation: u64,
        session_id: String,
    },
    RequestRuntimeIngress {
        agent_runtime_id: String,
        work_id: String,
        origin: String,
    },
    RequestRuntimeRetire {
        session_id: String,
    },
    RequestRuntimeDestroy {
        session_id: String,
    },
}

impl ProducerEffect for SeamEffect {
    fn variant_id(&self) -> EffectVariantId {
        let slug = match self {
            Self::RequestRuntimeBinding { .. } => "RequestRuntimeBinding",
            Self::RequestRuntimeIngress { .. } => "RequestRuntimeIngress",
            Self::RequestRuntimeRetire { .. } => "RequestRuntimeRetire",
            Self::RequestRuntimeDestroy { .. } => "RequestRuntimeDestroy",
        };
        EffectVariantId::parse(slug).expect("variant slug is well-formed")
    }

    fn field(&self, id: &FieldId) -> Option<FieldValue<'_>> {
        match self {
            Self::RequestRuntimeBinding {
                agent_runtime_id,
                fence_token,
                generation,
                session_id,
            } => match id.as_str() {
                "agent_runtime_id" => Some(FieldValue::Str(agent_runtime_id)),
                "fence_token" => Some(FieldValue::U64(*fence_token)),
                "generation" => Some(FieldValue::U64(*generation)),
                "session_id" => Some(FieldValue::Str(session_id)),
                _ => None,
            },
            Self::RequestRuntimeIngress {
                agent_runtime_id,
                work_id,
                origin,
            } => match id.as_str() {
                "agent_runtime_id" => Some(FieldValue::Str(agent_runtime_id)),
                "work_id" => Some(FieldValue::Str(work_id)),
                "origin" => Some(FieldValue::Str(origin)),
                _ => None,
            },
            Self::RequestRuntimeRetire { session_id } => match id.as_str() {
                "session_id" => Some(FieldValue::Str(session_id)),
                _ => None,
            },
            Self::RequestRuntimeDestroy { session_id } => match id.as_str() {
                "session_id" => Some(FieldValue::Str(session_id)),
                _ => None,
            },
        }
    }
}

#[derive(Default)]
struct RecordingSurface {
    log: tokio::sync::Mutex<Vec<(InputVariantId, Vec<(FieldId, OwnedFieldValue)>)>>,
}

#[async_trait]
impl ConsumerSurface for RecordingSurface {
    fn instance_id(&self) -> &MachineInstanceId {
        static ID: OnceLock<MachineInstanceId> = OnceLock::new();
        ID.get_or_init(|| MachineInstanceId::parse("meerkat").unwrap())
    }

    async fn apply_routed_input(
        &self,
        variant: InputVariantId,
        projected_fields: Vec<(FieldId, OwnedFieldValue)>,
    ) -> Result<(), String> {
        self.log.lock().await.push((variant, projected_fields));
        Ok(())
    }
}

struct RejectingSurface;

#[async_trait]
impl ConsumerSurface for RejectingSurface {
    fn instance_id(&self) -> &MachineInstanceId {
        static ID: OnceLock<MachineInstanceId> = OnceLock::new();
        ID.get_or_init(|| MachineInstanceId::parse("meerkat").unwrap())
    }

    async fn apply_routed_input(
        &self,
        _variant: InputVariantId,
        _projected_fields: Vec<(FieldId, OwnedFieldValue)>,
    ) -> Result<(), String> {
        Err("consumer is closed".to_string())
    }
}

fn mob_producer() -> ProducerInstance {
    ProducerInstance {
        composition: CompositionId::parse("meerkat_mob_seam").unwrap(),
        instance_id: MachineInstanceId::parse("mob").unwrap(),
        machine: MachineId::parse("MobMachine").unwrap(),
    }
}

fn build_dispatcher_with_consumer(
    consumer: Arc<dyn ConsumerSurface>,
) -> CatalogCompositionDispatcher<SeamEffect> {
    let schema = meerkat_mob_seam_composition();
    let table = RouteTable::from_schema(&schema).expect("catalog seam schema is well-formed");
    CatalogCompositionDispatcher::new(schema.name.clone(), table).with_consumer(consumer)
}

#[tokio::test]
async fn composition_name_matches_catalog_schema() {
    let schema = meerkat_mob_seam_composition();
    let table = RouteTable::from_schema(&schema).unwrap();
    let dispatcher: CatalogCompositionDispatcher<SeamEffect> =
        CatalogCompositionDispatcher::new(schema.name.clone(), table);

    // Name/descriptor agreement: the dispatcher's composition() must
    // equal the schema's `name` field byte-for-byte.
    assert_eq!(
        dispatcher.composition().as_str(),
        "meerkat_mob_seam",
        "dispatcher composition id diverged from catalog schema name",
    );
}

#[tokio::test]
async fn all_four_catalog_routes_dispatch_to_meerkat_consumer() {
    // Pin the full route set in one test: each of the four mob→meerkat
    // effects must land on the meerkat consumer with the declared
    // `input_variant` and the typed field projection.
    let consumer = Arc::new(RecordingSurface::default());
    let dispatcher = build_dispatcher_with_consumer(Arc::clone(&consumer) as _);

    // Route 1: RequestRuntimeBinding → PrepareBindings
    let outcome = dispatcher
        .dispatch(
            mob_producer(),
            EffectPayload::Emitted {
                variant: EffectVariantId::parse("RequestRuntimeBinding").unwrap(),
                body: SeamEffect::RequestRuntimeBinding {
                    agent_runtime_id: "rt-1".into(),
                    fence_token: 11,
                    generation: 3,
                    session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".into(),
                },
            },
        )
        .await
        .expect("binding route dispatches");
    assert_eq!(outcome.applied_input.as_str(), "PrepareBindings");
    assert_eq!(
        outcome.route.route_id.as_str(),
        "binding_request_reaches_meerkat"
    );
    assert_eq!(outcome.consumer.as_str(), "meerkat");

    // Route 2: RequestRuntimeIngress → Ingest
    let outcome = dispatcher
        .dispatch(
            mob_producer(),
            EffectPayload::Emitted {
                variant: EffectVariantId::parse("RequestRuntimeIngress").unwrap(),
                body: SeamEffect::RequestRuntimeIngress {
                    agent_runtime_id: "rt-1".into(),
                    work_id: "w-42".into(),
                    origin: "test".into(),
                },
            },
        )
        .await
        .expect("ingress route dispatches");
    assert_eq!(outcome.applied_input.as_str(), "Ingest");
    assert_eq!(
        outcome.route.route_id.as_str(),
        "work_request_reaches_meerkat"
    );

    // Route 3: RequestRuntimeRetire → Retire
    let outcome = dispatcher
        .dispatch(
            mob_producer(),
            EffectPayload::Emitted {
                variant: EffectVariantId::parse("RequestRuntimeRetire").unwrap(),
                body: SeamEffect::RequestRuntimeRetire {
                    session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".to_string(),
                },
            },
        )
        .await
        .expect("retire route dispatches");
    assert_eq!(outcome.applied_input.as_str(), "Retire");

    // Route 4: RequestRuntimeDestroy → Destroy
    let outcome = dispatcher
        .dispatch(
            mob_producer(),
            EffectPayload::Emitted {
                variant: EffectVariantId::parse("RequestRuntimeDestroy").unwrap(),
                body: SeamEffect::RequestRuntimeDestroy {
                    session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".to_string(),
                },
            },
        )
        .await
        .expect("destroy route dispatches");
    assert_eq!(outcome.applied_input.as_str(), "Destroy");

    // Post-condition: the consumer saw all four inputs in order, with
    // the correct field projections for the two field-carrying effects.
    let log = consumer.log.lock().await;
    assert_eq!(log.len(), 4, "consumer must see exactly 4 routed inputs");
    let variants: Vec<&str> = log.iter().map(|(v, _)| v.as_str()).collect();
    assert_eq!(
        variants,
        vec!["PrepareBindings", "Ingest", "Retire", "Destroy"],
    );
    // PrepareBindings field projection: 4 fields, typed correctly.
    // session_id added by A1 Shape 4 (9e3b31ec4) as the fourth binding
    // on `binding_request_reaches_meerkat`.
    let (_, binding_fields) = &log[0];
    assert_eq!(binding_fields.len(), 4);
    assert_eq!(binding_fields[0].0.as_str(), "agent_runtime_id");
    match &binding_fields[0].1 {
        OwnedFieldValue::Str(s) => assert_eq!(s, "rt-1"),
        other => panic!("agent_runtime_id not Str: {other:?}"),
    }
    match &binding_fields[1].1 {
        OwnedFieldValue::U64(v) => assert_eq!(*v, 11),
        other => panic!("fence_token not U64: {other:?}"),
    }
    match &binding_fields[2].1 {
        OwnedFieldValue::U64(v) => assert_eq!(*v, 3),
        other => panic!("generation not U64: {other:?}"),
    }
    assert_eq!(binding_fields[3].0.as_str(), "session_id");
    match &binding_fields[3].1 {
        OwnedFieldValue::Str(s) => assert_eq!(s, "019dbd3d-d7ad-75a1-96d0-8013927e78f8"),
        other => panic!("session_id not Str: {other:?}"),
    }
}

#[tokio::test]
async fn composition_mismatch_is_typed_refusal() {
    let consumer: Arc<dyn ConsumerSurface> = Arc::new(RecordingSurface::default());
    let dispatcher = build_dispatcher_with_consumer(consumer);

    let mut wrong = mob_producer();
    wrong.composition = CompositionId::parse("some_other_seam").unwrap();

    let err = dispatcher
        .dispatch(
            wrong,
            EffectPayload::Emitted {
                variant: EffectVariantId::parse("RequestRuntimeRetire").unwrap(),
                body: SeamEffect::RequestRuntimeRetire {
                    session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".to_string(),
                },
            },
        )
        .await
        .expect_err("must refuse mismatched composition");

    match err {
        DispatchRefusal::CompositionMismatch { expected, actual } => {
            assert_eq!(expected.as_str(), "meerkat_mob_seam");
            assert_eq!(actual.as_str(), "some_other_seam");
        }
        other => panic!("expected CompositionMismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn unresolved_route_is_typed_refusal() {
    // The catalog has no route for an `UnknownEffect` variant.
    let consumer: Arc<dyn ConsumerSurface> = Arc::new(RecordingSurface::default());
    let dispatcher = build_dispatcher_with_consumer(consumer);

    let err = dispatcher
        .dispatch(
            mob_producer(),
            EffectPayload::Emitted {
                variant: EffectVariantId::parse("UnknownEffect").unwrap(),
                body: SeamEffect::RequestRuntimeRetire {
                    session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".to_string(),
                },
            },
        )
        .await
        .expect_err("must refuse unrouted effect");

    match err {
        DispatchRefusal::UnresolvedRoute {
            composition,
            instance,
            variant,
        } => {
            assert_eq!(composition.as_str(), "meerkat_mob_seam");
            assert_eq!(instance.as_str(), "mob");
            assert_eq!(variant.as_str(), "UnknownEffect");
        }
        other => panic!("expected UnresolvedRoute, got {other:?}"),
    }
}

#[tokio::test]
async fn unwired_consumer_is_typed_refusal() {
    // Build a dispatcher without registering any consumer surface.
    let schema = meerkat_mob_seam_composition();
    let table = RouteTable::from_schema(&schema).unwrap();
    let dispatcher: CatalogCompositionDispatcher<SeamEffect> =
        CatalogCompositionDispatcher::new(schema.name.clone(), table);

    let err = dispatcher
        .dispatch(
            mob_producer(),
            EffectPayload::Emitted {
                variant: EffectVariantId::parse("RequestRuntimeRetire").unwrap(),
                body: SeamEffect::RequestRuntimeRetire {
                    session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".to_string(),
                },
            },
        )
        .await
        .expect_err("must refuse delivery with no wired consumer");

    match err {
        DispatchRefusal::UnwiredConsumer {
            composition,
            instance,
        } => {
            assert_eq!(composition.as_str(), "meerkat_mob_seam");
            assert_eq!(instance.as_str(), "meerkat");
        }
        other => panic!("expected UnwiredConsumer, got {other:?}"),
    }
}

#[tokio::test]
async fn consumer_refusal_is_typed_refusal() {
    // Register a consumer that always returns Err — the dispatcher must
    // lift its message into ConsumerRefused, not swallow it.
    let dispatcher = build_dispatcher_with_consumer(Arc::new(RejectingSurface) as _);

    let err = dispatcher
        .dispatch(
            mob_producer(),
            EffectPayload::Emitted {
                variant: EffectVariantId::parse("RequestRuntimeRetire").unwrap(),
                body: SeamEffect::RequestRuntimeRetire {
                    session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".to_string(),
                },
            },
        )
        .await
        .expect_err("consumer rejection surfaces as typed refusal");

    match err {
        DispatchRefusal::ConsumerRefused {
            instance,
            variant,
            reason,
        } => {
            assert_eq!(instance.as_str(), "meerkat");
            assert_eq!(variant.as_str(), "Retire");
            assert_eq!(reason, "consumer is closed");
        }
        other => panic!("expected ConsumerRefused, got {other:?}"),
    }
}

#[tokio::test]
async fn duplicate_consumer_registration_replaces_prior_entry() {
    // with_consumer() documents that duplicate registration for the
    // same instance_id replaces — this is what keeps wiring-site bugs
    // visible (a second registration isn't silently dropped, but it
    // also doesn't panic). Prove the contract: the second surface wins.
    let schema = meerkat_mob_seam_composition();
    let table = RouteTable::from_schema(&schema).unwrap();

    let first = Arc::new(RecordingSurface::default());
    let second = Arc::new(RecordingSurface::default());

    let dispatcher: CatalogCompositionDispatcher<SeamEffect> =
        CatalogCompositionDispatcher::new(schema.name.clone(), table)
            .with_consumer(Arc::clone(&first) as _)
            .with_consumer(Arc::clone(&second) as _);

    dispatcher
        .dispatch(
            mob_producer(),
            EffectPayload::Emitted {
                variant: EffectVariantId::parse("RequestRuntimeRetire").unwrap(),
                body: SeamEffect::RequestRuntimeRetire {
                    session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".to_string(),
                },
            },
        )
        .await
        .expect("dispatch succeeds");

    assert_eq!(
        first.log.lock().await.len(),
        0,
        "first registration must be replaced — it must not see the input",
    );
    assert_eq!(
        second.log.lock().await.len(),
        1,
        "second (winning) registration receives the input",
    );
}

#[test]
fn standalone_binding_has_no_dispatcher() {
    // CompositionBinding discriminates at compile time between
    // standalone / wired / owner-provided — proving Standalone yields
    // no dispatcher keeps the "no silent fallback" invariant pinned.
    let binding: CompositionBinding<SeamEffect> = CompositionBinding::standalone();
    assert!(binding.is_standalone());
    assert!(binding.wired().is_none());
}
