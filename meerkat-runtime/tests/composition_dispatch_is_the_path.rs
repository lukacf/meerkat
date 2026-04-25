#![allow(
    clippy::expect_used,
    clippy::explicit_iter_loop,
    clippy::panic,
    clippy::redundant_clone,
    clippy::type_complexity,
    clippy::unwrap_used
)]

//! `composition_dispatch_is_the_path` — wave-b V2 / Task B-5.
//!
//! End-to-end assertion that the [`meerkat_runtime::composition::
//! CompositionDispatcher`] trait is *the* typed execution path for routed
//! effects. Three levels of verification:
//!
//! 1. **Behavioural.** Produce a `MobMachine` routed effect, wire it
//!    through the catalog-backed dispatcher, assert the typed consumer
//!    surface on the `meerkat` instance receives the typed input with
//!    bindings resolved by [`FieldId`].
//! 2. **Typed refusal.** Unresolved routes, composition mismatches,
//!    signal-kind targets, missing producer fields, and unwired
//!    consumers all surface as typed [`DispatchRefusal`] variants — the
//!    dispatcher never silently drops a routed effect.
//! 3. **Grep-level invariant.** Scan every file under
//!    `meerkat-runtime/src/meerkat_machine/` and
//!    `meerkat-runtime/src/mob_adapter.rs` and fail if any of them
//!    re-introduces a legacy routed-effect helper — e.g. the deleted
//!    `composition_dispatch`, `recompute_mob_peer_overlay`, or
//!    `comms_trust_reconcile` names. These are RMAT-adjacent canaries
//!    that wave-b Task B-10 will turn into a semantic audit; this test
//!    is the minimum-viable gate.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// Hand-rolled `MeerkatMobSeamEffect` stand-in covering the Mob producer
/// variants the composition schema routes today. Once the codegen wires
/// `render_composition_driver` into the runtime tree, this type is
/// replaced by the emitted `MeerkatMobSeamEffect` verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SeamEffect {
    Mob(MobEffect),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MobEffect {
    RequestRuntimeBinding {
        agent_runtime_id: String,
        fence_token: u64,
        generation: u64,
        session_id: String,
    },
    RequestRuntimeRetire {
        session_id: String,
    },
}

impl ProducerEffect for SeamEffect {
    fn variant_id(&self) -> EffectVariantId {
        match self {
            Self::Mob(MobEffect::RequestRuntimeBinding { .. }) => {
                EffectVariantId::parse("RequestRuntimeBinding").unwrap()
            }
            Self::Mob(MobEffect::RequestRuntimeRetire { .. }) => {
                EffectVariantId::parse("RequestRuntimeRetire").unwrap()
            }
        }
    }

    fn field(&self, id: &FieldId) -> Option<FieldValue<'_>> {
        match self {
            Self::Mob(MobEffect::RequestRuntimeBinding {
                agent_runtime_id,
                fence_token,
                generation,
                session_id,
            }) => match id.as_str() {
                "agent_runtime_id" => Some(FieldValue::Str(agent_runtime_id)),
                "fence_token" => Some(FieldValue::U64(*fence_token)),
                "generation" => Some(FieldValue::U64(*generation)),
                "session_id" => Some(FieldValue::Str(session_id)),
                _ => None,
            },
            Self::Mob(MobEffect::RequestRuntimeRetire { session_id }) => match id.as_str() {
                "session_id" => Some(FieldValue::Str(session_id)),
                _ => None,
            },
        }
    }
}

struct RecordingSurface {
    id: MachineInstanceId,
    log: tokio::sync::Mutex<Vec<(InputVariantId, Vec<(FieldId, OwnedFieldValue)>)>>,
}

impl RecordingSurface {
    fn meerkat() -> Self {
        Self {
            id: MachineInstanceId::parse("meerkat").unwrap(),
            log: tokio::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ConsumerSurface for RecordingSurface {
    fn instance_id(&self) -> &MachineInstanceId {
        &self.id
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

fn mob_producer() -> ProducerInstance {
    ProducerInstance {
        composition: CompositionId::parse("meerkat_mob_seam").unwrap(),
        instance_id: MachineInstanceId::parse("mob").unwrap(),
        machine: MachineId::parse("MobMachine").unwrap(),
    }
}

fn build_dispatcher(consumer: Arc<RecordingSurface>) -> CatalogCompositionDispatcher<SeamEffect> {
    let schema = meerkat_mob_seam_composition();
    let table = RouteTable::from_schema(&schema).expect("seam schema routes are well-formed");
    CatalogCompositionDispatcher::new(schema.name.clone(), table).with_consumer(consumer)
}

// ---------------------------------------------------------------------
// 1. Behavioural assertion: the dispatcher is the only path that applies
//    a routed effect as a typed consumer input.
// ---------------------------------------------------------------------

#[tokio::test]
async fn dispatcher_is_the_path_for_mob_request_runtime_binding() {
    let consumer = Arc::new(RecordingSurface::meerkat());
    let dispatcher = build_dispatcher(Arc::clone(&consumer));

    let payload = EffectPayload::Emitted {
        variant: EffectVariantId::parse("RequestRuntimeBinding").unwrap(),
        body: SeamEffect::Mob(MobEffect::RequestRuntimeBinding {
            agent_runtime_id: "rt-alpha".into(),
            fence_token: 11,
            generation: 2,
            session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".into(),
        }),
    };

    let outcome = dispatcher
        .dispatch(mob_producer(), payload)
        .await
        .expect("dispatcher resolved the typed route");

    assert_eq!(outcome.consumer.as_str(), "meerkat");
    assert_eq!(outcome.applied_input.as_str(), "PrepareBindings");
    assert_eq!(
        outcome.route.route_id.as_str(),
        "binding_request_reaches_meerkat"
    );

    let log = consumer.log.lock().await;
    assert_eq!(log.len(), 1, "dispatcher invoked the consumer exactly once");
    let (variant, fields) = &log[0];
    assert_eq!(variant.as_str(), "PrepareBindings");

    // Field-binding order matches the composition schema declaration.
    let field_ids: Vec<&str> = fields.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        field_ids,
        vec![
            "agent_runtime_id",
            "fence_token",
            "generation",
            "session_id"
        ]
    );
    match &fields[0].1 {
        OwnedFieldValue::Str(s) => assert_eq!(s, "rt-alpha"),
        other => panic!("expected Str for agent_runtime_id, got {other:?}"),
    }
    match &fields[1].1 {
        OwnedFieldValue::U64(v) => assert_eq!(*v, 11),
        other => panic!("expected U64 for fence_token, got {other:?}"),
    }
    match &fields[2].1 {
        OwnedFieldValue::U64(v) => assert_eq!(*v, 2),
        other => panic!("expected U64 for generation, got {other:?}"),
    }
    match &fields[3].1 {
        OwnedFieldValue::Str(s) => assert_eq!(s, "019dbd3d-d7ad-75a1-96d0-8013927e78f8"),
        other => panic!("expected Str for session_id, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatcher_handles_binding_free_route() {
    // `RequestRuntimeRetire` has no field bindings in the schema. Verify
    // the dispatcher handles empty-binding routes without projection
    // errors.
    let consumer = Arc::new(RecordingSurface::meerkat());
    let dispatcher = build_dispatcher(Arc::clone(&consumer));

    let payload = EffectPayload::Emitted {
        variant: EffectVariantId::parse("RequestRuntimeRetire").unwrap(),
        body: SeamEffect::Mob(MobEffect::RequestRuntimeRetire {
            session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".to_string(),
        }),
    };

    let outcome = dispatcher
        .dispatch(mob_producer(), payload)
        .await
        .expect("retire route resolves");

    assert_eq!(outcome.applied_input.as_str(), "Retire");

    let log = consumer.log.lock().await;
    assert_eq!(log.len(), 1);
    // Post-Shape-4 extension: retire route now carries `session_id` so the
    // consumer surface can resolve the target session (54ce31148).
    assert_eq!(log[0].1.len(), 1);
    assert_eq!(log[0].1[0].0.as_str(), "session_id");
}

// ---------------------------------------------------------------------
// 2. Typed refusal: every failure mode surfaces as a typed
//    `DispatchRefusal`, not a silent drop.
// ---------------------------------------------------------------------

#[tokio::test]
async fn refuses_unresolved_route_typed() {
    let consumer = Arc::new(RecordingSurface::meerkat());
    let dispatcher = build_dispatcher(consumer);

    // Producer variant that the schema does not route.
    let payload = EffectPayload::Emitted {
        variant: EffectVariantId::parse("UnknownEffect").unwrap(),
        body: SeamEffect::Mob(MobEffect::RequestRuntimeRetire {
            session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".to_string(),
        }),
    };

    let err = dispatcher
        .dispatch(mob_producer(), payload)
        .await
        .expect_err("unrouted variant is typed refusal");
    assert!(
        matches!(err, DispatchRefusal::UnresolvedRoute { .. }),
        "expected UnresolvedRoute, got {err:?}"
    );
}

#[tokio::test]
async fn refuses_unwired_consumer_typed() {
    // Build a dispatcher with NO consumer registered for the `meerkat`
    // target instance; the route resolves but delivery surfaces typed.
    let schema = meerkat_mob_seam_composition();
    let table = RouteTable::from_schema(&schema).unwrap();
    let dispatcher: CatalogCompositionDispatcher<SeamEffect> =
        CatalogCompositionDispatcher::new(schema.name.clone(), table);

    let payload = EffectPayload::Emitted {
        variant: EffectVariantId::parse("RequestRuntimeBinding").unwrap(),
        body: SeamEffect::Mob(MobEffect::RequestRuntimeBinding {
            agent_runtime_id: "rt".into(),
            fence_token: 0,
            generation: 0,
            session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".into(),
        }),
    };

    let err = dispatcher
        .dispatch(mob_producer(), payload)
        .await
        .expect_err("missing consumer is typed refusal");
    assert!(
        matches!(err, DispatchRefusal::UnwiredConsumer { .. }),
        "expected UnwiredConsumer, got {err:?}"
    );
}

#[tokio::test]
async fn refuses_composition_mismatch_typed() {
    let consumer = Arc::new(RecordingSurface::meerkat());
    let dispatcher = build_dispatcher(consumer);

    let mut wrong = mob_producer();
    wrong.composition = CompositionId::parse("wrong_composition").unwrap();

    let payload = EffectPayload::Emitted {
        variant: EffectVariantId::parse("RequestRuntimeBinding").unwrap(),
        body: SeamEffect::Mob(MobEffect::RequestRuntimeBinding {
            agent_runtime_id: "rt".into(),
            fence_token: 0,
            generation: 0,
            session_id: "019dbd3d-d7ad-75a1-96d0-8013927e78f8".into(),
        }),
    };

    let err = dispatcher
        .dispatch(wrong, payload)
        .await
        .expect_err("composition mismatch");
    assert!(matches!(err, DispatchRefusal::CompositionMismatch { .. }));
}

// ---------------------------------------------------------------------
// 3. Two-constructor witness: `CompositionBinding` is not
//    `Option<Arc<dyn CompositionDispatcher>>`. Both arms exist at the
//    type level.
// ---------------------------------------------------------------------

#[tokio::test]
async fn composition_binding_discriminates_standalone_vs_wired() {
    let standalone: CompositionBinding<SeamEffect> = CompositionBinding::Standalone;
    assert!(standalone.is_standalone());
    assert!(standalone.wired().is_none());

    let consumer = Arc::new(RecordingSurface::meerkat());
    let dispatcher: Arc<dyn CompositionDispatcher<Effect = SeamEffect>> =
        Arc::new(build_dispatcher(consumer));
    let wired: CompositionBinding<SeamEffect> = CompositionBinding::Wired(dispatcher);
    assert!(!wired.is_standalone());
    assert!(wired.wired().is_some());
}

// ---------------------------------------------------------------------
// 4. Grep-level RMAT canary: the deleted stringly-typed helpers from
//    wave-a must not reappear in `meerkat-runtime/src/meerkat_machine/`
//    or `meerkat-runtime/src/mob_adapter.rs`. B-10 will upgrade this to
//    a semantic audit; for now the byte-pattern match is the minimum-
//    viable invariant that the dispatcher is THE path.
// ---------------------------------------------------------------------

#[test]
fn no_legacy_composition_helpers_in_routed_effect_call_sites() {
    // The forbidden identifiers are the deleted wave-a helpers that
    // used to live alongside the dispatch call sites. Their re-entry
    // would mean a parallel, non-typed path to the one this trait
    // defines. We scan the source files themselves (not the crate's
    // compile output) so the test is cheap and deterministic.
    // Helper-shape substrings: we ban the stringly-typed *helper-call*
    // patterns that wave-a deleted, not the mere presence of the name —
    // wave-c C-T restored `comms_trust_reconcile` as a proper typed
    // module (`crate::comms_trust_reconcile::CommsTrustReconciler`), so
    // the bare name is no longer a signal for a parallel non-typed path.
    // Match the helper-function shape (`fn <name>`) instead.
    const BANNED: &[&str] = &[
        "composition_dispatch", // deleted wave-a helper
        "recompute_mob_peer_overlay",
        "fn comms_trust_reconcile", // helper fn, not the typed module
    ];

    let crate_src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let roots: [&Path; 2] = [
        &crate_src.join("meerkat_machine"),
        &crate_src.join("mob_adapter.rs"),
    ];

    let mut files: Vec<PathBuf> = Vec::new();
    for root in roots.iter() {
        if root.is_file() {
            files.push(root.to_path_buf());
        } else if root.is_dir() {
            collect_rs_files(root, &mut files);
        }
    }
    assert!(
        !files.is_empty(),
        "expected at least one routed-effect call-site file to scan; check allowlist roots"
    );

    for file in &files {
        let text = std::fs::read_to_string(file)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", file.display()));
        for banned in BANNED {
            assert!(
                !text.contains(banned),
                "forbidden legacy helper `{banned}` reappeared in {}; \
                 routed-effect dispatch must go through \
                 meerkat_runtime::composition::CompositionDispatcher",
                file.display()
            );
        }
    }
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}
