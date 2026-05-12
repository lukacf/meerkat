#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    unused_imports
)]

use std::collections::BTreeMap;

use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
    dsl_occurrence_lifecycle_machine as occurrence_lifecycle_machine,
    dsl_schedule_lifecycle_machine as schedule_lifecycle_machine,
};
use meerkat_machine_schema::identity::{
    ActorId, CompositionDriverId, CompositionId, EffectVariantId, EnumTypeId, EnumVariantId,
    FieldId, InputVariantId, MachineId, MachineInstanceId, NamedTypeId, PhaseId, ProtocolId,
    RouteId, TransitionId,
};
use meerkat_machine_schema::{
    ActorKind, ActorSchema, CompositionDriver, CompositionDriverRustBinding, CompositionSchema,
    CompositionSchemaError, CompositionStateLimits, CompositionWitness, CompositionWitnessField,
    CompositionWitnessInput, CompositionWitnessState, DriverDispatchRoute, Expr, MachineInstance,
    Route, RouteBindingSource, RouteDelivery, RouteFieldBinding, RouteTarget, RouteTargetKind,
    RouteVariantId, RustTypeAtom, TypePathStructField, TypeRef, WatchedEffect,
    canonical_composition_coverage_manifests, canonical_composition_schemas,
    canonical_machine_coverage_manifests, canonical_machine_schemas, meerkat_mob_seam_composition,
};

#[test]
fn canonical_machine_registry_contains_kernel_and_perimeter_entries() {
    let names = canonical_machine_schemas()
        .into_iter()
        .map(|schema| schema.machine.as_str().to_owned())
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        vec![
            "MeerkatMachine",
            "MobMachine",
            "ScheduleLifecycleMachine",
            "OccurrenceLifecycleMachine",
            // AuthMachine: per-binding auth lease lifecycle. Added
            // after Phase 1.5-rev was refactored from "absorbed into
            // MeerkatMachine" to "standalone perimeter machine"
            // — auth lifecycle is orthogonal to MeerkatMachine's
            // lifecycle and gets its own canonical machine per
            // dogma §1 "one semantic fact, one owner".
            "AuthMachine",
            // WorkGraph: realm-scoped commitment graph lifecycle,
            // readiness, claim state, and topology validation.
            "WorkGraphLifecycleMachine",
        ]
    );

    for absorbed in [
        "SessionTurnAdmissionMachine",
        "SessionToolVisibilityMachine",
        "PeerDirectoryReachabilityMachine",
    ] {
        assert!(
            !names.iter().any(|name| name == absorbed),
            "{absorbed} should be absorbed into canonical kernels, not published separately"
        );
    }
}

#[test]
fn external_tool_surface_failure_cause_fields_are_typed() {
    let schema = meerkat_machine();
    let cause_ty = TypeRef::Enum(
        EnumTypeId::parse("ExternalToolSurfaceFailureCause").expect("valid enum type"),
    );

    let pending_failed = schema
        .inputs
        .variants
        .iter()
        .find(|variant| variant.name.as_str() == "SurfaceMarkPendingFailed")
        .expect("SurfaceMarkPendingFailed input");
    assert_eq!(
        pending_failed
            .field_named("cause")
            .expect("typed failure cause field")
            .ty,
        cause_ty
    );
    assert!(
        pending_failed.field_named("reason").is_err(),
        "pending failure cause must not remain a string-shaped reason field"
    );

    let reject_surface_call = schema
        .effects
        .variants
        .iter()
        .find(|variant| variant.name.as_str() == "RejectSurfaceCall")
        .expect("RejectSurfaceCall effect");
    assert_eq!(
        reject_surface_call
            .field_named("cause")
            .expect("typed rejection cause field")
            .ty,
        cause_ty
    );
    assert!(
        reject_surface_call.field_named("reason").is_err(),
        "surface call rejection cause must not remain a string-shaped reason field"
    );

    let emit_external_tool_delta = schema
        .effects
        .variants
        .iter()
        .find(|variant| variant.name.as_str() == "EmitExternalToolDelta")
        .expect("EmitExternalToolDelta effect");
    assert_eq!(
        emit_external_tool_delta
            .field_named("cause")
            .expect("typed lifecycle failure cause field")
            .ty,
        TypeRef::Option(Box::new(cause_ty))
    );
    assert!(
        emit_external_tool_delta.field_named("reason").is_err(),
        "lifecycle failure cause must not remain a string-shaped reason field"
    );
}

#[test]
fn canonical_composition_registry_contains_kernel_seam_and_schedule_perimeter_entries() {
    let names = canonical_composition_schemas()
        .into_iter()
        .map(|schema| schema.name.as_str().to_owned())
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        vec![
            "meerkat_mob_seam",
            "schedule_bundle",
            "schedule_runtime_bundle",
            "schedule_mob_bundle",
            "auth_lease_bundle",
        ]
    );
}

#[test]
fn canonical_machine_registry_is_individually_valid() {
    for schema in canonical_machine_schemas() {
        assert_eq!(
            schema.validate(),
            Ok(()),
            "machine {} should validate",
            schema.machine
        );
    }
}

#[test]
fn peer_ingress_lifecycle_subject_signal_carries_candidate_not_selected_subject() {
    let schema = meerkat_machine();
    let signal = schema
        .signals
        .variant_named("ClassifyExternalEnvelope")
        .expect("ClassifyExternalEnvelope signal");
    let fields = signal
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();

    assert!(
        fields.contains(&"from_peer"),
        "fallback peer identity should remain a typed input fact"
    );
    assert!(
        fields.contains(&"lifecycle_peer_param"),
        "machine should receive the parsed lifecycle peer parameter candidate"
    );
    assert!(
        !fields.contains(&"lifecycle_peer"),
        "preselected lifecycle subjects must not cross the machine signal seam"
    );
}

#[test]
fn meerkat_deferred_tool_witness_named_type_is_structural_authority() {
    let schema = meerkat_machine();
    let witness_type = NamedTypeId::parse("ToolVisibilityWitness").expect("named type");
    let binding = schema
        .named_type_binding(&witness_type)
        .expect("ToolVisibilityWitness binding");

    assert_eq!(
        binding.rust,
        RustTypeAtom::TypePathFieldPresenceSet {
            path: "crate::catalog::dsl::meerkat_machine::ToolVisibilityWitness".to_string(),
            fields: vec![
                FieldId::parse("stable_owner_key").expect("field"),
                FieldId::parse("last_seen_provenance").expect("field"),
            ],
        },
        "deferred-tool authority must be bound to the typed witness projection and field-presence domain, not String"
    );

    let provenance_type = NamedTypeId::parse("ToolProvenance").expect("named type");
    let provenance_binding = schema
        .named_type_binding(&provenance_type)
        .expect("ToolProvenance binding");

    assert_eq!(
        provenance_binding.rust,
        RustTypeAtom::TypePathStruct {
            path: "crate::catalog::dsl::meerkat_machine::ToolProvenance".to_string(),
            fields: vec![
                TypePathStructField::named("kind", "ToolSourceKind"),
                TypePathStructField::string("source_id"),
            ],
        },
        "nested provenance authority must be a typed structural binding, not a String fallback"
    );
}

#[test]
fn canonical_composition_registry_is_individually_valid() {
    let canonical_machines = canonical_machine_schemas();
    let canonical_machine_refs = canonical_machines.iter().collect::<Vec<_>>();

    for schema in canonical_composition_schemas() {
        assert_eq!(
            schema.validate_against(&canonical_machine_refs),
            Ok(()),
            "composition {} should validate against the canonical machine set",
            schema.name
        );
    }
}

#[test]
fn kernel_seam_rejects_type_mismatched_route_binding() {
    let meerkat = meerkat_machine();
    let mob = mob_machine();
    let mut composition = meerkat_mob_seam_composition();
    let route_idx = composition
        .routes
        .iter()
        .position(|route| route.name.as_str() == "binding_request_reaches_meerkat");
    assert!(route_idx.is_some(), "binding request route");
    let Some(route_idx) = route_idx else {
        return;
    };
    let route = &mut composition.routes[route_idx];
    let generation_binding_idx = route
        .bindings
        .iter()
        .position(|binding| binding.to_field.as_str() == "generation");
    assert!(generation_binding_idx.is_some(), "generation binding");
    let Some(generation_binding_idx) = generation_binding_idx else {
        return;
    };
    let generation_binding = &mut route.bindings[generation_binding_idx];
    generation_binding.source = meerkat_machine_schema::RouteBindingSource::Field {
        from_field: FieldId::parse("fence_token").expect("valid from_field"),
        allow_named_alias: false,
    };

    let result = composition.validate_against(&[&meerkat, &mob]);
    assert!(matches!(
        result,
        Err(CompositionSchemaError::RouteFieldTypeMismatch { .. }
            | CompositionSchemaError::MachineSchema(_))
    ));
}

#[test]
fn kernel_seam_rejects_zero_named_domain_override() {
    let mut composition = meerkat_mob_seam_composition();
    composition.deep_domain_overrides = BTreeMap::from([("WorkIdValues".into(), 0)]);

    let result = composition.validate();
    assert!(matches!(
        result,
        Err(CompositionSchemaError::InvalidNamedDomainCardinality { .. })
    ));
}

#[test]
fn composition_schema_rejects_divergent_named_type_bindings_across_machines() {
    let meerkat = meerkat_machine();
    let mut mob = mob_machine();
    let binding_name = NamedTypeId::parse("AgentRuntimeId").expect("named type");
    let binding = mob
        .named_types
        .iter_mut()
        .find(|binding| binding.name == binding_name)
        .expect("MobMachine AgentRuntimeId binding");
    binding.rust = RustTypeAtom::U64;

    let result = meerkat_mob_seam_composition().validate_against(&[&meerkat, &mob]);

    match result {
        Err(CompositionSchemaError::ConflictingNamedTypeBinding {
            name,
            first_machine,
            first_rust,
            second_machine,
            second_rust,
        }) => {
            assert_eq!(name, "AgentRuntimeId");
            assert_eq!(first_machine, "MeerkatMachine");
            assert_eq!(first_rust, RustTypeAtom::String);
            assert_eq!(second_machine, "MobMachine");
            assert_eq!(second_rust, RustTypeAtom::U64);
        }
        other => panic!(
            "composition validation must reject divergent machine-owned named-type bindings, got {other:?}"
        ),
    }
}

#[test]
fn schedule_and_occurrence_machines_stay_in_canonical_coverage_manifests() {
    let machine_names = canonical_machine_schemas()
        .into_iter()
        .map(|schema| schema.machine.as_str().to_owned())
        .collect::<Vec<_>>();
    let coverage_names = canonical_machine_coverage_manifests()
        .into_iter()
        .map(|manifest| manifest.machine.as_str().to_owned())
        .collect::<Vec<_>>();

    for name in [
        schedule_lifecycle_machine().machine,
        occurrence_lifecycle_machine().machine,
    ] {
        assert!(
            machine_names
                .iter()
                .any(|machine| machine.as_str() == name.as_str()),
            "{name} should remain canonical"
        );
        assert!(
            coverage_names
                .iter()
                .any(|machine| machine.as_str() == name.as_str()),
            "{name} should retain coverage metadata"
        );
    }
}

#[test]
fn kernel_seam_retains_coverage_metadata() {
    let coverage_names = canonical_composition_coverage_manifests()
        .into_iter()
        .map(|manifest| manifest.composition.as_str().to_owned())
        .collect::<Vec<_>>();

    assert_eq!(
        coverage_names,
        vec![
            "meerkat_mob_seam",
            "schedule_bundle",
            "schedule_runtime_bundle",
            "schedule_mob_bundle",
            "auth_lease_bundle",
        ]
    );
}

#[test]
fn meerkat_machine_absorbs_runtime_ingress_turn_tool_and_peer_domains() {
    let schema = meerkat_machine();
    let input_names = schema
        .inputs
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();
    let signal_names = schema
        .signals
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();
    let effect_names = schema
        .effects
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();

    for required in [
        "EnsureSessionWithExecutor",
        "SetSilentIntents",
        "Ingest",
        "PublishEvent",
        "AcceptWithCompletion",
        "AcceptWithoutWake",
        "Prepare",
        "Commit",
        "Fail",
        "StartConversationRun",
        "InterruptCurrentRun",
        "CancelAfterBoundary",
        "ReconfigureSessionLlmIdentity",
        "StagePersistentFilter",
        "RequestDeferredTools",
        "SurfaceStageAdd",
        "SurfaceStageRemove",
        "SurfaceStageReload",
        "SurfaceMarkPendingSucceeded",
        "SurfaceMarkPendingFailed",
        "SurfaceSnapshotAligned",
    ] {
        assert!(
            input_names.iter().any(|name| name == &required),
            "MeerkatMachine should absorb input {required}"
        );
    }

    for required in [
        "EnsureDrainRunning",
        "ClassifyExternalEnvelope",
        "ClassifyPlainEvent",
    ] {
        assert!(
            signal_names.iter().any(|name| name == &required),
            "MeerkatMachine should absorb signal {required}"
        );
    }

    for required in [
        "ResolveAdmission",
        "SubmitAdmittedIngressEffect",
        "SubmitRunPrimitive",
        "PostAdmissionSignal",
        "SubmitOpEvent",
        "EnqueueClassifiedEntry",
        "PeerIngressClassified",
        "SpawnDrainTask",
        "EmitExternalToolDelta",
        "CommittedVisibleSetPublished",
    ] {
        assert!(
            effect_names.iter().any(|name| name == &required),
            "MeerkatMachine should absorb effect {required}"
        );
    }
}

#[test]
fn meerkat_machine_merges_turn_admission_tool_visibility_and_peer_directory_state() {
    let schema = meerkat_machine();
    let field_names = schema
        .state
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    let transition_names = schema
        .transitions
        .iter()
        .map(|transition| transition.name.as_str())
        .collect::<Vec<_>>();
    let effect_names = schema
        .effects
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();

    for required in ["current_run_id", "silent_intent_overrides"] {
        assert!(
            field_names.iter().any(|name| name == &required),
            "MeerkatMachine state should retain absorbed field {required}"
        );
    }

    for required in [
        "InterruptCurrentRun",
        "CancelAfterBoundary",
        "SetPeerIngressContextAttached",
        "SetPeerIngressContextRunning",
        "NotifyDrainExitedAttached",
        "NotifyDrainExitedRunning",
        "EnsureDrainRunningAttached",
        "EnsureDrainRunningRunning",
        "AcceptWithCompletionAttachedQueued",
        "AcceptWithCompletionAttachedImmediate",
        "AcceptWithCompletionRunningQueuedPassive",
        "AcceptWithCompletionRunningInterruptYielding",
        "AcceptWithCompletionRunningImmediate",
        "AcceptWithoutWakeAttached",
        "AcceptWithoutWakeRunning",
        "ClassifyExternalEnvelopeRequestPeerAddedIdle",
        "ClassifyExternalEnvelopeRequestPeerRetiredIdle",
        "ClassifyExternalEnvelopeRequestPeerRetiredRetired",
        "ClassifyExternalEnvelopeRequestPeerRetiredStopped",
        "ClassifyExternalEnvelopeRequestPeerUnwiredIdle",
        "ClassifyExternalEnvelopeRequestPeerUnwiredRetired",
        "ClassifyExternalEnvelopeRequestPeerUnwiredStopped",
        "ClassifyExternalEnvelopeLifecycleAddedIdle",
        "ClassifyExternalEnvelopeLifecycleRetiredIdle",
        "ClassifyExternalEnvelopeLifecycleRetiredRetired",
        "ClassifyExternalEnvelopeLifecycleRetiredStopped",
        "ClassifyExternalEnvelopeLifecycleUnwiredIdle",
        "ClassifyExternalEnvelopeLifecycleUnwiredRetired",
        "ClassifyExternalEnvelopeLifecycleUnwiredStopped",
        "IngestAttached",
        "IngestRunning",
        "PublishEventAttached",
        "PublishEventRunning",
        "ReconfigureSessionLlmIdentityAttached",
        "ReconfigureSessionLlmIdentityRunning",
        "StagePersistentFilterAttached",
        "StagePersistentFilterRunning",
        "RequestDeferredToolsAttached",
        "RequestDeferredToolsRunning",
        "BoundaryAppliedPublish",
        "SurfaceStageAddAttached",
        "SurfaceStageAddRunning",
        "SurfaceStageRemoveAttached",
        "SurfaceStageRemoveRunning",
        "SurfaceStageReloadAttached",
        "SurfaceStageReloadRunning",
        "SurfaceApplyBoundaryAddAttached",
        "SurfaceApplyBoundaryAddRunning",
        "SurfaceApplyBoundaryReloadAttached",
        "SurfaceApplyBoundaryReloadRunning",
        "SurfaceApplyBoundaryRemoveDrainingAttached",
        "SurfaceApplyBoundaryRemoveDrainingRunning",
        "SurfaceApplyBoundaryRemoveNoopAttached",
        "SurfaceApplyBoundaryRemoveNoopRunning",
        "SurfaceMarkPendingSucceededAddAttached",
        "SurfaceMarkPendingSucceededAddRunning",
        "SurfaceMarkPendingSucceededReloadAttached",
        "SurfaceMarkPendingSucceededReloadRunning",
        "SurfaceFinalizeRemovalCleanAttached",
        "SurfaceFinalizeRemovalCleanRunning",
        "PublishCommittedVisibleSetAttached",
        "PublishCommittedVisibleSetRunning",
    ] {
        assert!(
            transition_names.iter().any(|name| name == &required),
            "MeerkatMachine should expose absorbed transition {required}"
        );
    }

    for required in [
        "WakeInterrupt",
        "PostAdmissionSignal",
        "CommittedVisibleSetPublished",
    ] {
        assert!(
            effect_names.iter().any(|name| name == &required),
            "MeerkatMachine should retain absorbed effect {required}"
        );
    }
}

#[test]
fn mob_machine_absorbs_flow_orchestrator_runtime_bridge_and_public_command_domains() {
    let schema = mob_machine();
    let input_names = schema
        .inputs
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();
    let signal_names = schema
        .signals
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();
    let effect_names = schema
        .effects
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();

    for required in [
        "RunFlow",
        "CancelFlow",
        "CancelWork",
        "CancelAllWork",
        // D-wire-handler (#26): DSL inputs are pair-valued
        // (`WireMembers`/`UnwireMembers` taking a `WiringEdge`), not
        // per-member. The `MobMachineCommand::Wire`/`Unwire` shell
        // commands forward to these inputs after normalizing
        // `(local, target)` into an edge.
        "WireMembers",
        "UnwireMembers",
        "SubmitWork",
        "SubscribeMobEvents",
    ] {
        assert!(
            input_names.iter().any(|name| name == &required),
            "MobMachine should absorb input {required}"
        );
    }

    for required in ["StageSpawn", "CreateRun"] {
        assert!(
            signal_names.iter().any(|name| name == &required),
            "MobMachine should absorb signal {required}"
        );
    }

    for required in ["EmitFlowRunNotice", "NotifyCoordinator"] {
        assert!(
            effect_names.iter().any(|name| name == &required),
            "MobMachine should absorb effect {required}"
        );
    }
}

#[test]
fn mob_machine_merges_flow_wiring_and_runtime_bridge_state() {
    let schema = mob_machine();
    let field_names = schema
        .state
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    let transition_names = schema
        .transitions
        .iter()
        .map(|transition| transition.name.as_str())
        .collect::<Vec<_>>();

    for required in ["pending_spawn_count", "coordinator_bound"] {
        assert!(
            field_names.iter().any(|name| name == &required),
            "MobMachine state should retain absorbed field {required}"
        );
    }

    for required in [
        // W3-H-1: RetireRunning is split into RetireRunningReleasing /
        // RetireRunningNoBinding; we anchor on the NoBinding variant to
        // keep this contract assertion aligned with the post-split
        // topology (the Releasing variant exists conditionally on
        // prior realtime binding state).
        "RetireRunningNoBinding",
        "RetireAllRunning",
        // Renamed in wave-a track-b `450464072` ("MobMachine
        // session-binding generalization + topology_epoch") to
        // disambiguate member-wiring from realtime-transport wiring;
        // the `Members` infix stayed through the absorption sweep.
        "WireMembersRunning",
        "UnwireMembersRunning",
        "StageSpawnRunning",
        "CompleteSpawnRunning",
        "BindCoordinatorRunning",
        "RunFlowRunning",
        "StartFlowRunning",
        "CreateRunRunning",
        "StartRunRunning",
        "CompleteFlowRunning",
        "FinishRunRunning",
        "ObserveRuntimeRetired",
        "DestroyMob",
        "ObserveRuntimeDestroyed",
        "SubscribeMobEventsRunning",
    ] {
        assert!(
            transition_names.iter().any(|name| name == &required),
            "MobMachine should expose absorbed transition {required}"
        );
    }
}

#[test]
fn meerkat_runtime_command_surface_is_fully_accounted_for_by_canonical_schema_inputs() {
    let schema = meerkat_machine();
    let input_names = schema
        .inputs
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();

    for required in [
        "RegisterSession",
        "UnregisterSession",
        "EnsureSessionWithExecutor",
        "SetSilentIntents",
        "InterruptCurrentRun",
        "CancelAfterBoundary",
        "StopRuntimeExecutor",
        "ContainsSession",
        "SessionHasExecutor",
        "SessionHasComms",
        "OpsLifecycleRegistry",
        "ReconfigureSessionLlmIdentity",
        "PrepareBindings",
        "InputState",
        "ListActiveInputs",
        "PublishCommittedVisibleSet",
        "SetPeerIngressContext",
        "NotifyDrainExited",
        "AbortAll",
        "Abort",
        "Wait",
        "Ingest",
        "PublishEvent",
        "RuntimeState",
        "LoadBoundaryReceipt",
        "AcceptWithCompletion",
        "AcceptWithoutWake",
        "ReconfigureSessionLlmIdentity",
        "StagePersistentFilter",
        "RequestDeferredTools",
        "Prepare",
        "Commit",
        "Fail",
        "Retire",
        "Recycle",
        "Reset",
        "Recover",
        "Destroy",
    ] {
        assert!(
            input_names.iter().any(|name| name == &required),
            "MeerkatMachine canonical schema should account for runtime command/input {required}"
        );
    }
}

#[test]
fn mob_runtime_command_surface_is_fully_accounted_for_by_canonical_schema_inputs() {
    let schema = mob_machine();
    let input_names = schema
        .inputs
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();

    for required in [
        "RunFlow",
        "CancelFlow",
        "FlowStatus",
        "Spawn",
        "EnsureMember",
        "Reconcile",
        "Retire",
        "Respawn",
        "RetireAll",
        // D-wire-handler (#26): see note in `mob_machine_absorbs_flow_orchestrator...`.
        "WireMembers",
        "UnwireMembers",
        "SubmitWork",
        "CancelAllWork",
        "Stop",
        "Resume",
        "Complete",
        "Reset",
        "Destroy",
        "RosterSnapshot",
        "ListMembers",
        "ListMembersIncludingRetiring",
        "ListAllMembers",
        "MemberStatus",
        "SubscribeAgentEvents",
        "SubscribeAllAgentEvents",
        "SubscribeMobEvents",
        "PollEvents",
        "ReplayAllEvents",
        "RecordOperatorActionProvenance",
        "GetMember",
        "SetSpawnPolicy",
        "Shutdown",
        "ForceCancel",
    ] {
        assert!(
            input_names.iter().any(|name| name == &required),
            "MobMachine canonical schema should account for runtime command/input {required}"
        );
    }

    for intentionally_test_only in ["FlowTrackerCounts", "OrchestratorSnapshot"] {
        assert!(
            !input_names
                .iter()
                .any(|name| name == &intentionally_test_only),
            "MobMachine canonical schema should not publish test-only diagnostic input {intentionally_test_only}"
        );
    }
}

#[test]
fn every_mutating_meerkat_runtime_command_has_transition_coverage() {
    let schema = meerkat_machine();
    let transitioned_inputs = schema
        .transitions
        .iter()
        .map(|transition| transition.on.variant_str())
        .collect::<std::collections::BTreeSet<_>>();

    for required in [
        "RegisterSession",
        "UnregisterSession",
        "EnsureSessionWithExecutor",
        "SetSilentIntents",
        "InterruptCurrentRun",
        "CancelAfterBoundary",
        "StopRuntimeExecutor",
        "ReconfigureSessionLlmIdentity",
        "PrepareBindings",
        "PublishCommittedVisibleSet",
        "SetPeerIngressContext",
        "NotifyDrainExited",
        "StagePersistentFilter",
        "RequestDeferredTools",
        "AbortAll",
        "Abort",
        "Ingest",
        "PublishEvent",
        "Retire",
        "Recycle",
        "Reset",
        "Destroy",
        "AcceptWithCompletion",
        "AcceptWithoutWake",
        "Prepare",
        "Commit",
        "Fail",
    ] {
        assert!(
            transitioned_inputs.contains(required),
            "MeerkatMachine should model mutating runtime command {required} with at least one transition",
        );
    }
}

#[test]
fn every_mutating_mob_runtime_command_has_transition_coverage() {
    let schema = mob_machine();
    let transitioned_inputs = schema
        .transitions
        .iter()
        .map(|transition| transition.on.variant_str())
        .collect::<std::collections::BTreeSet<_>>();

    for required in [
        "RunFlow",
        "CancelFlow",
        "Spawn",
        "EnsureMember",
        "Reconcile",
        "Retire",
        "Respawn",
        "RetireAll",
        // D-wire-handler (#26): DSL transitions cover `WireMembersRunning`
        // and `UnwireMembersRunning` keyed by `WiringEdge`.
        "WireMembers",
        "UnwireMembers",
        "SubmitWork",
        "CancelAllWork",
        "Stop",
        "Resume",
        "Complete",
        "Reset",
        "Destroy",
        "SubscribeAgentEvents",
        "SubscribeAllAgentEvents",
        "SubscribeMobEvents",
        "RecordOperatorActionProvenance",
        "SetSpawnPolicy",
        "Shutdown",
        "ForceCancel",
    ] {
        assert!(
            transitioned_inputs.contains(required),
            "MobMachine should model mutating runtime command {required} with at least one transition",
        );
    }
}

#[test]
fn every_query_runtime_command_has_expected_surface_coverage() {
    let meerkat = meerkat_machine();
    let meerkat_surface_only_inputs = meerkat
        .surface_only_inputs
        .iter()
        .map(AsRef::as_ref)
        .collect::<std::collections::BTreeSet<_>>();
    let meerkat_transitioned = meerkat
        .transitions
        .iter()
        .map(|transition| transition.on.variant_str())
        .collect::<std::collections::BTreeSet<_>>();
    for required in [
        "ContainsSession",
        "SessionHasExecutor",
        "SessionHasComms",
        "OpsLifecycleRegistry",
        "InputState",
        "ListActiveInputs",
        "RuntimeState",
        "LoadBoundaryReceipt",
    ] {
        assert!(
            meerkat_surface_only_inputs.contains(required),
            "MeerkatMachine query command {required} should stay surfaced even without transitions"
        );
        assert!(
            !meerkat_transitioned.contains(required),
            "MeerkatMachine query command {required} should no longer require transition coverage"
        );
    }
    let required = "Wait";
    assert!(
        meerkat_transitioned.contains(required),
        "MeerkatMachine helper command {required} should still have transition coverage"
    );

    let mob = mob_machine();
    let mob_surface_only_inputs = mob
        .surface_only_inputs
        .iter()
        .map(AsRef::as_ref)
        .collect::<std::collections::BTreeSet<_>>();
    let mob_transitioned = mob
        .transitions
        .iter()
        .map(|transition| transition.on.variant_str())
        .collect::<std::collections::BTreeSet<_>>();
    for required in [
        "FlowStatus",
        "RosterSnapshot",
        "ListMembers",
        "ListMembersIncludingRetiring",
        "ListAllMembers",
        "MemberStatus",
        "CancelWork",
        "PollEvents",
        "ReplayAllEvents",
        "GetMember",
    ] {
        assert!(
            mob_surface_only_inputs.contains(required),
            "MobMachine query command {required} should stay surfaced even without transitions"
        );
        assert!(
            !mob_transitioned.contains(required),
            "MobMachine query command {required} should no longer require transition coverage"
        );
    }
}

// --------------------------------------------------------------------------
// Composition driver execution framework (Track-B, R5).
//
// The declarative `CompositionDriver` descriptor replaces the hand-crafted
// `flow_frame_loop` template specialization. It is the schema-side seam that
// the runtime dispatcher (meerkat-runtime::composition_dispatch) consumes to
// know which effects to watch and where to route driver decisions.
//
// These tests pin the framework contract: the schema must accept a fully
// declared driver, must reject a driver that references unknown machines or
// variants, and must accept `driver: None` on compositions that do not need
// one.
// --------------------------------------------------------------------------

fn sample_driver_rust_binding() -> CompositionDriverRustBinding {
    CompositionDriverRustBinding {
        module_path: "meerkat-runtime/src/generated/noop_driver.rs".into(),
        driver_type: "NoopDriver".into(),
        store_plan_type: "NoopStorePlan".into(),
        work_type: "NoopWork".into(),
        decision_type: "NoopDecision".into(),
        required_imports: vec![],
    }
}

fn noop_driver_on_meerkat_mob_seam() -> CompositionDriver {
    // Watch a single effect that already exists on MobMachine in the
    // canonical schema (`RuntimeBound` round-trip flow). This keeps the
    // test decoupled from the Track-B MobMachine extensions landing in
    // the next commit — the framework here only cares that the declared
    // watched variant exists on the producer machine.
    CompositionDriver {
        name: CompositionDriverId::parse("noop_driver").expect("valid driver name"),
        rust: sample_driver_rust_binding(),
        watched_effects: vec![WatchedEffect {
            producer_instance: MachineInstanceId::parse("mob").expect("valid producer_instance"),
            effect_variant: EffectVariantId::parse("RequestRuntimeBinding")
                .expect("valid effect_variant"),
        }],
        dispatch_routes: vec![DriverDispatchRoute {
            name: RouteId::parse("noop_dispatch").expect("valid route slug"),
            target_instance: MachineInstanceId::parse("meerkat").expect("valid MachineInstanceId"),
            target_kind: RouteTargetKind::Input,
            input_variant: RouteVariantId::Input(
                InputVariantId::parse("PrepareBindings").expect("valid input-variant slug"),
            ),
        }],
    }
}

#[test]
fn composition_driver_with_declared_watches_and_routes_validates_against_canonical_machines() {
    let meerkat = meerkat_machine();
    let mob = mob_machine();
    let mut composition = meerkat_mob_seam_composition();
    composition.driver = Some(noop_driver_on_meerkat_mob_seam());

    assert_eq!(
        composition.validate_against(&[&meerkat, &mob]),
        Ok(()),
        "declarative composition driver should validate against canonical machines",
    );
}

#[test]
fn composition_driver_rejects_watched_effect_on_unknown_producer_instance() {
    let meerkat = meerkat_machine();
    let mob = mob_machine();
    let mut composition = meerkat_mob_seam_composition();
    let mut driver = noop_driver_on_meerkat_mob_seam();
    driver.watched_effects[0].producer_instance =
        MachineInstanceId::parse("ghost_machine").expect("valid MachineInstanceId");
    composition.driver = Some(driver);

    let result = composition.validate_against(&[&meerkat, &mob]);
    assert!(
        matches!(
            result,
            Err(CompositionSchemaError::UnknownCompositionDriverWatchedMachine { .. })
        ),
        "expected UnknownCompositionDriverWatchedMachine, got {result:?}",
    );
}

#[test]
fn composition_driver_rejects_watched_variant_missing_on_producer_effects() {
    let meerkat = meerkat_machine();
    let mob = mob_machine();
    let mut composition = meerkat_mob_seam_composition();
    let mut driver = noop_driver_on_meerkat_mob_seam();
    driver.watched_effects[0].effect_variant =
        EffectVariantId::parse("NoSuchEffect").expect("valid EffectVariantId");
    composition.driver = Some(driver);

    let result = composition.validate_against(&[&meerkat, &mob]);
    assert!(
        matches!(
            result,
            Err(CompositionSchemaError::UnknownCompositionDriverWatchedEffect { .. })
        ),
        "expected UnknownCompositionDriverWatchedEffect, got {result:?}",
    );
}

#[test]
fn composition_driver_rejects_dispatch_route_to_unknown_target_instance() {
    let meerkat = meerkat_machine();
    let mob = mob_machine();
    let mut composition = meerkat_mob_seam_composition();
    let mut driver = noop_driver_on_meerkat_mob_seam();
    driver.dispatch_routes[0].target_instance =
        MachineInstanceId::parse("ghost_target").expect("valid MachineInstanceId");
    composition.driver = Some(driver);

    let result = composition.validate_against(&[&meerkat, &mob]);
    assert!(
        matches!(
            result,
            Err(CompositionSchemaError::UnknownCompositionDriverDispatchMachine { .. })
        ),
        "expected UnknownCompositionDriverDispatchMachine, got {result:?}",
    );
}

#[test]
fn composition_driver_rejects_dispatch_route_input_variant_missing_on_target() {
    let meerkat = meerkat_machine();
    let mob = mob_machine();
    let mut composition = meerkat_mob_seam_composition();
    let mut driver = noop_driver_on_meerkat_mob_seam();
    driver.dispatch_routes[0].input_variant = RouteVariantId::Input(
        InputVariantId::parse("NoSuchInput").expect("valid input-variant slug"),
    );
    composition.driver = Some(driver);

    let result = composition.validate_against(&[&meerkat, &mob]);
    assert!(
        matches!(
            result,
            Err(CompositionSchemaError::UnknownCompositionDriverDispatchVariant { .. })
        ),
        "expected UnknownCompositionDriverDispatchVariant, got {result:?}",
    );
}

#[test]
fn composition_driver_rejects_duplicate_watched_effect_declarations() {
    let meerkat = meerkat_machine();
    let mob = mob_machine();
    let mut composition = meerkat_mob_seam_composition();
    let mut driver = noop_driver_on_meerkat_mob_seam();
    driver
        .watched_effects
        .push(driver.watched_effects[0].clone());
    composition.driver = Some(driver);

    let result = composition.validate_against(&[&meerkat, &mob]);
    assert!(
        matches!(result, Err(CompositionSchemaError::DuplicateName { .. })),
        "expected DuplicateName, got {result:?}",
    );
}

#[test]
fn composition_driver_rejects_duplicate_dispatch_route_names() {
    let meerkat = meerkat_machine();
    let mob = mob_machine();
    let mut composition = meerkat_mob_seam_composition();
    let mut driver = noop_driver_on_meerkat_mob_seam();
    driver
        .dispatch_routes
        .push(driver.dispatch_routes[0].clone());
    composition.driver = Some(driver);

    let result = composition.validate_against(&[&meerkat, &mob]);
    assert!(
        matches!(result, Err(CompositionSchemaError::DuplicateName { .. })),
        "expected DuplicateName, got {result:?}",
    );
}

#[test]
fn composition_with_driver_none_still_validates_as_before() {
    // Post-Commit-4 the `meerkat_mob_seam` composition itself
    // declares a `RecomputeMobPeerOverlay` driver — so this test
    // exercises a constructed driverless variant of the seam to pin
    // that `driver: None` is still a valid shape. The 3
    // `schedule_*_bundle` compositions continue to carry `driver:
    // None` and validate transitively via the
    // `canonical_composition_registry_is_individually_valid` test.
    let meerkat = meerkat_machine();
    let mob = mob_machine();
    let mut composition = meerkat_mob_seam_composition();
    composition.driver = None;

    assert_eq!(
        composition.validate_against(&[&meerkat, &mob]),
        Ok(()),
        "composition without a driver must validate",
    );
}

fn literal_probe_composition(
    routes: Vec<Route>,
    witnesses: Vec<CompositionWitness>,
) -> CompositionSchema {
    CompositionSchema {
        name: CompositionId::parse("literal_probe").expect("composition slug"),
        machines: vec![MachineInstance {
            instance_id: MachineInstanceId::parse("meerkat").expect("machine instance"),
            machine_name: MachineId::parse("MeerkatMachine").expect("machine slug"),
            actor: ActorId::parse("meerkat_actor").expect("actor slug"),
        }],
        actors: vec![ActorSchema {
            name: ActorId::parse("meerkat_actor").expect("actor slug"),
            kind: ActorKind::Machine,
        }],
        handoff_protocols: vec![],
        entry_inputs: vec![],
        routes,
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses,
        deep_domain_cardinality: 2,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(CompositionStateLimits::ci_defaults()),
        closed_world: true,
    }
}

fn literal_probe_witness(
    expected_routes: Vec<RouteId>,
    preload_inputs: Vec<CompositionWitnessInput>,
    expected_states: Vec<CompositionWitnessState>,
) -> CompositionWitness {
    CompositionWitness {
        name: meerkat_machine_schema::identity::CompositionWitnessId::parse("literal_probe")
            .expect("witness slug"),
        preload_inputs,
        expected_routes,
        expected_scheduler_rules: vec![],
        expected_states,
        expected_transitions: vec![],
        expected_transition_order: vec![],
        state_limits: CompositionStateLimits::ci_defaults(),
    }
}

fn route_literal_to_meerkat_input(
    route_name: &str,
    input_variant: &str,
    field: &str,
    expr: Expr,
) -> Route {
    Route {
        name: RouteId::parse(route_name).expect("route slug"),
        from_machine: MachineInstanceId::parse("meerkat").expect("machine instance"),
        effect_variant: EffectVariantId::parse("CommittedVisibleSetPublished")
            .expect("effect slug"),
        to: RouteTarget::new(
            MachineInstanceId::parse("meerkat").expect("machine instance"),
            RouteVariantId::Input(InputVariantId::parse(input_variant).expect("input slug")),
        ),
        bindings: vec![RouteFieldBinding {
            to_field: FieldId::parse(field).expect("field slug"),
            source: RouteBindingSource::Literal(expr),
        }],
        delivery: RouteDelivery::Immediate,
    }
}

#[test]
fn composition_route_literal_rejects_closed_tool_filter_placeholder_string() {
    let meerkat = meerkat_machine();
    let route = route_literal_to_meerkat_input(
        "bad_tool_filter_route",
        "StageVisibilityFilter",
        "filter",
        Expr::String("toolfilter_2".into()),
    );
    let composition = literal_probe_composition(
        vec![route],
        vec![literal_probe_witness(
            vec![RouteId::parse("bad_tool_filter_route").expect("route slug")],
            vec![],
            vec![],
        )],
    );

    let result = composition.validate_against(&[&meerkat]);
    assert!(
        matches!(
            &result,
            Err(CompositionSchemaError::RouteLiteralTypeMismatch { route, to_field, .. })
                if route == "bad_tool_filter_route" && to_field == "filter"
        ),
        "expected route literal type mismatch for closed ToolFilter domain, got {result:?}",
    );
}

#[test]
fn composition_witness_input_rejects_unknown_enum_named_variant_literal() {
    let meerkat = meerkat_machine();
    let composition = literal_probe_composition(
        vec![],
        vec![literal_probe_witness(
            vec![],
            vec![CompositionWitnessInput {
                machine: MachineInstanceId::parse("meerkat").expect("machine instance"),
                input_variant: InputVariantId::parse("RegisterOp").expect("input slug"),
                fields: vec![
                    CompositionWitnessField {
                        field: FieldId::parse("operation_id").expect("field slug"),
                        expr: Expr::String("op-1".into()),
                    },
                    CompositionWitnessField {
                        field: FieldId::parse("kind").expect("field slug"),
                        expr: Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("OperationKind").expect("enum type"),
                            variant: EnumVariantId::parse("Bogus").expect("variant slug"),
                        },
                    },
                ],
            }],
            vec![],
        )],
    );

    let result = composition.validate_against(&[&meerkat]);
    assert!(
        matches!(
            &result,
            Err(CompositionSchemaError::WitnessLiteralTypeMismatch { field, .. })
                if field == "kind"
        ),
        "expected witness literal type mismatch for unknown OperationKind, got {result:?}",
    );
}

#[test]
fn composition_witness_state_rejects_closed_tool_filter_placeholder_string() {
    let meerkat = meerkat_machine();
    let composition = literal_probe_composition(
        vec![],
        vec![literal_probe_witness(
            vec![],
            vec![],
            vec![CompositionWitnessState {
                machine: MachineInstanceId::parse("meerkat").expect("machine instance"),
                phase: None,
                fields: vec![CompositionWitnessField {
                    field: FieldId::parse("active_filter").expect("field slug"),
                    expr: Expr::String("toolfilter_2".into()),
                }],
            }],
        )],
    );

    let result = composition.validate_against(&[&meerkat]);
    assert!(
        matches!(
            &result,
            Err(CompositionSchemaError::WitnessStateLiteralTypeMismatch { field, .. })
                if field == "active_filter"
        ),
        "expected witness state literal type mismatch for closed ToolFilter domain, got {result:?}",
    );
}

#[test]
fn every_canonical_input_variant_has_transition_coverage() {
    for schema in canonical_machine_schemas() {
        let surface_only_inputs = schema
            .surface_only_inputs
            .iter()
            .map(AsRef::as_ref)
            .collect::<std::collections::BTreeSet<_>>();
        let input_names = schema
            .inputs
            .variants
            .iter()
            .map(|variant| variant.name.as_str())
            .filter(|input| !surface_only_inputs.contains(input))
            .collect::<Vec<_>>();
        let transitioned_inputs = schema
            .transitions
            .iter()
            .map(|transition| transition.on.variant_str())
            .collect::<std::collections::BTreeSet<_>>();

        let missing = input_names
            .into_iter()
            .filter(|input| !transitioned_inputs.contains(input))
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "machine {} should model every input with at least one transition; missing: {:?}",
            schema.machine,
            missing
        );
    }
}

// ---------------------------------------------------------------------------
// Handoff protocol binding contracts — HandleBridge + additional_modes
// ---------------------------------------------------------------------------

#[allow(clippy::expect_used, clippy::panic)]
mod handoff_binding {
    use meerkat_machine_schema::identity::{
        ActorId, CompositionId, EffectVariantId, FieldId, InputVariantId, MachineId,
        MachineInstanceId, ProtocolId,
    };
    use meerkat_machine_schema::{
        ActorKind, ActorSchema, ClosurePolicy, CompositionSchema, CompositionSchemaError,
        CompositionStateLimits, EffectHandoffProtocol, FeedbackFieldBinding, FeedbackFieldSource,
        FeedbackInputRef, HandleBridgeFeedbackBinding, MachineInstance, ProtocolGenerationMode,
        ProtocolHelperReturnShape, ProtocolRustBinding, canonical_composition_schemas,
        canonical_machine_schemas, compat_composition_schemas,
    };

    fn ok_handle_binding() -> ProtocolRustBinding {
        ProtocolRustBinding {
            module_path: "crate-x/src/generated/proto.rs".into(),
            generation_mode: ProtocolGenerationMode::HandleBridge,
            required_imports: vec![],
            authority_type_path: None,
            mutator_trait_path: None,
            input_enum_path: None,
            effect_enum_path: None,
            transition_type_path: None,
            error_type_path: None,
            executor_trigger_input_variant: None,
            bridge_source_type_path: None,
            helper_return_shape: ProtocolHelperReturnShape::Effects,
            handle_trait_path: Some("crate::SomeHandle".into()),
            handle_feedback_bindings: vec![HandleBridgeFeedbackBinding {
                input_variant: InputVariantId::parse("Ack").expect("valid input_variant"),
                method_name: "acknowledge".into(),
                arg_accessors: Default::default(),
                forwarded_fields: None,
            }],
            input_payload_module_path: None,
            additional_modes: vec![],
        }
    }

    fn composition_with_protocol(protocol: EffectHandoffProtocol) -> CompositionSchema {
        CompositionSchema {
            name: CompositionId::parse("test_bundle").expect("valid composition slug"),
            machines: vec![MachineInstance {
                instance_id: MachineInstanceId::parse("meerkat").expect("valid instance_id"),
                machine_name: MachineId::parse("MeerkatMachine").expect("valid machine_name"),
                actor: ActorId::parse("meerkat_authority").expect("valid actor"),
            }],
            actors: vec![
                ActorSchema {
                    name: ActorId::parse("meerkat_authority").expect("valid actor slug"),
                    kind: ActorKind::Machine,
                },
                ActorSchema {
                    name: ActorId::parse("surface_owner").expect("valid actor slug"),
                    kind: ActorKind::Owner,
                },
            ],
            handoff_protocols: vec![protocol],
            entry_inputs: vec![],
            routes: vec![],
            route_target_selectors: vec![],
            driver: None,
            transaction_plans: vec![],
            actor_priorities: vec![],
            scheduler_rules: vec![],
            invariants: vec![],
            witnesses: vec![],
            deep_domain_cardinality: 2,
            deep_domain_overrides: std::collections::BTreeMap::new(),
            witness_domain_cardinality: 2,
            ci_limits: Some(CompositionStateLimits {
                step_limit: 4,
                pending_input_limit: 4,
                pending_route_limit: 4,
                delivered_route_limit: 0,
                emitted_effect_limit: 0,
                seq_limit: 0,
                set_limit: 0,
                map_limit: 0,
            }),
            closed_world: true,
        }
    }

    fn handle_bridge_protocol(rust: ProtocolRustBinding) -> EffectHandoffProtocol {
        EffectHandoffProtocol {
            name: ProtocolId::parse("test_handoff").expect("valid protocol slug"),
            producer_instance: MachineInstanceId::parse("meerkat")
                .expect("valid producer_instance"),
            effect_variant: EffectVariantId::parse("RefreshVisibleSurfaceSet")
                .expect("valid effect_variant"),
            realizing_actor: ActorId::parse("surface_owner").expect("valid realizing_actor"),
            correlation_fields: vec![],
            obligation_fields: vec![],
            allowed_feedback_inputs: vec![FeedbackInputRef {
                machine_instance: MachineInstanceId::parse("meerkat")
                    .expect("valid machine_instance"),
                input_variant: InputVariantId::parse("Ack").expect("valid input_variant"),
                field_bindings: vec![FeedbackFieldBinding {
                    input_field: FieldId::parse("epoch").expect("valid input_field"),
                    source: FeedbackFieldSource::OwnerContext("epoch".into()),
                }],
            }],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: None,
            rust,
        }
    }

    #[test]
    fn handle_bridge_requires_handle_trait_path() {
        let mut binding = ok_handle_binding();
        binding.handle_trait_path = None;
        let composition = composition_with_protocol(handle_bridge_protocol(binding));

        let err = composition
            .validate()
            .expect_err("must reject missing handle_trait_path");
        match err {
            CompositionSchemaError::InvalidHandoffRustBinding { protocol, detail } => {
                assert_eq!(protocol, "test_handoff");
                assert!(
                    detail.contains("handle_trait_path"),
                    "error detail should mention handle_trait_path, got {detail}"
                );
            }
            other => panic!("expected InvalidHandoffRustBinding, got {other:?}"),
        }
    }

    #[test]
    fn handle_bridge_requires_method_for_every_feedback_input() {
        let mut binding = ok_handle_binding();
        binding.handle_feedback_bindings = vec![];
        let composition = composition_with_protocol(handle_bridge_protocol(binding));

        let err = composition
            .validate()
            .expect_err("must reject missing method map");
        match err {
            CompositionSchemaError::InvalidHandoffRustBinding { protocol, detail } => {
                assert_eq!(protocol, "test_handoff");
                assert!(
                    detail.contains("handle_feedback_bindings"),
                    "error detail should mention handle_feedback_bindings, got {detail}"
                );
                assert!(
                    detail.contains("Ack"),
                    "error detail should name the missing feedback input, got {detail}"
                );
            }
            other => panic!("expected InvalidHandoffRustBinding, got {other:?}"),
        }
    }

    #[test]
    fn handle_bridge_minimum_binding_validates_clean() {
        let binding = ok_handle_binding();
        let composition = composition_with_protocol(handle_bridge_protocol(binding));
        composition
            .validate()
            .expect("minimum handle-bridge binding should validate");
    }

    #[test]
    fn validate_against_rejects_protocol_without_machine_owned_disposition() {
        let binding = ProtocolRustBinding {
            module_path: "crate-x/src/generated/proto.rs".into(),
            generation_mode: ProtocolGenerationMode::EffectExtractor,
            required_imports: vec![],
            authority_type_path: None,
            mutator_trait_path: None,
            input_enum_path: None,
            effect_enum_path: Some("crate::Effect".into()),
            transition_type_path: None,
            error_type_path: None,
            executor_trigger_input_variant: None,
            bridge_source_type_path: None,
            helper_return_shape: ProtocolHelperReturnShape::Obligations,
            handle_trait_path: None,
            handle_feedback_bindings: vec![],
            input_payload_module_path: None,
            additional_modes: vec![],
        };
        let protocol = EffectHandoffProtocol {
            name: ProtocolId::parse("floating_protocol").expect("valid protocol slug"),
            producer_instance: MachineInstanceId::parse("meerkat")
                .expect("valid producer_instance"),
            effect_variant: EffectVariantId::parse("RefreshVisibleSurfaceSet")
                .expect("valid effect_variant"),
            realizing_actor: ActorId::parse("surface_owner").expect("valid realizing_actor"),
            correlation_fields: vec![],
            obligation_fields: vec![],
            allowed_feedback_inputs: vec![],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: None,
            rust: binding,
        };
        let composition = composition_with_protocol(protocol);
        let machines = canonical_machine_schemas();
        let machine_refs: Vec<_> = machines.iter().collect();

        let err = composition
            .validate_against(&machine_refs)
            .expect_err("protocol must be owned by producer disposition");
        match err {
            CompositionSchemaError::HandoffProtocolMismatch {
                protocol,
                effect_variant,
                expected_protocol,
            } => {
                assert_eq!(protocol, "floating_protocol");
                assert_eq!(effect_variant, "RefreshVisibleSurfaceSet");
                assert_eq!(expected_protocol, "floating_protocol");
            }
            other => panic!("expected HandoffProtocolMismatch, got {other:?}"),
        }
    }

    #[test]
    fn additional_modes_rejects_duplicating_primary_mode() {
        let mut binding = ok_handle_binding();
        binding.additional_modes = vec![ProtocolGenerationMode::HandleBridge];
        let composition = composition_with_protocol(handle_bridge_protocol(binding));

        let err = composition
            .validate()
            .expect_err("must reject duplicate primary mode");
        match err {
            CompositionSchemaError::InvalidHandoffRustBinding { protocol, detail } => {
                assert_eq!(protocol, "test_handoff");
                assert!(
                    detail.contains("duplicates the primary"),
                    "error detail should mention duplication, got {detail}"
                );
            }
            other => panic!("expected InvalidHandoffRustBinding, got {other:?}"),
        }
    }

    #[test]
    fn additional_modes_rejects_repeat_within_list() {
        let mut binding = ok_handle_binding();
        // Switch primary to EffectExtractor so HandleBridge can appear in
        // additional_modes; then make it appear twice.
        binding.generation_mode = ProtocolGenerationMode::EffectExtractor;
        // Fill required EffectExtractor fields.
        binding.authority_type_path = Some("crate::A".into());
        binding.mutator_trait_path = Some("crate::AM".into());
        binding.input_enum_path = Some("crate::AI".into());
        binding.effect_enum_path = Some("crate::AE".into());
        binding.transition_type_path = Some("crate::AT".into());
        binding.error_type_path = Some("crate::AErr".into());
        binding.additional_modes = vec![
            ProtocolGenerationMode::HandleBridge,
            ProtocolGenerationMode::HandleBridge,
        ];
        let composition = composition_with_protocol(handle_bridge_protocol(binding));

        let err = composition
            .validate()
            .expect_err("must reject repeat within additional_modes");
        match err {
            CompositionSchemaError::InvalidHandoffRustBinding { protocol, detail } => {
                assert_eq!(protocol, "test_handoff");
                assert!(
                    detail.contains("more than once"),
                    "error detail should mention repetition, got {detail}"
                );
            }
            other => panic!("expected InvalidHandoffRustBinding, got {other:?}"),
        }
    }

    #[test]
    fn dual_mode_effect_extractor_plus_handle_bridge_validates() {
        // Minimum valid stacked binding: EffectExtractor primary +
        // HandleBridge secondary, with all required fields for both.
        let binding = ProtocolRustBinding {
            module_path: "crate-x/src/generated/proto.rs".into(),
            generation_mode: ProtocolGenerationMode::EffectExtractor,
            required_imports: vec![],
            authority_type_path: Some("crate::A".into()),
            mutator_trait_path: Some("crate::AM".into()),
            input_enum_path: Some("crate::AI".into()),
            effect_enum_path: Some("crate::AE".into()),
            transition_type_path: Some("crate::AT".into()),
            error_type_path: Some("crate::AErr".into()),
            executor_trigger_input_variant: None,
            bridge_source_type_path: None,
            helper_return_shape: ProtocolHelperReturnShape::Effects,
            handle_trait_path: Some("crate::SomeHandle".into()),
            handle_feedback_bindings: vec![HandleBridgeFeedbackBinding {
                input_variant: InputVariantId::parse("Ack").expect("valid input_variant"),
                method_name: "acknowledge".into(),
                arg_accessors: Default::default(),
                forwarded_fields: None,
            }],
            input_payload_module_path: None,
            additional_modes: vec![ProtocolGenerationMode::HandleBridge],
        };
        let composition = composition_with_protocol(handle_bridge_protocol(binding));
        composition
            .validate()
            .expect("dual-mode binding should validate");
    }

    /// Wave-d D-c: `auth_lease_bundle` composition validates in isolation
    /// and carries the structural seam closure for the AuthMachine
    /// lifecycle-event publication. Asserts:
    /// - the composition is registered in `canonical_composition_schemas()`;
    /// - it validates against the canonical machine registry alone;
    /// - the handoff protocol `auth_lease_lifecycle_publication` is
    ///   declared directly on `AuthMachine::EmitLifecycleEvent`.
    ///
    /// This is the red-test anchor for the orphan-closure contract: if
    /// any of the composition components are removed or renamed, this
    /// fails before the broader drift tests do, naming the specific
    /// structural loss.
    #[test]
    fn auth_lease_bundle_composition_closes_auth_machine_orphan() {
        let comp = compat_composition_schemas()
            .into_iter()
            .chain(canonical_composition_schemas())
            .find(|c| c.name.as_str() == "auth_lease_bundle")
            .expect("auth_lease_bundle must be registered as a canonical composition");

        let machines = canonical_machine_schemas();
        let refs: Vec<_> = machines.iter().collect();
        comp.validate_against(&refs).unwrap_or_else(|err| {
            panic!("auth_lease_bundle must validate against canonical machines: {err:?}")
        });

        assert!(
            comp.routes.is_empty(),
            "auth lifecycle publication must not pass through a bridge-only route"
        );

        let protocol = comp
            .handoff_protocols
            .iter()
            .find(|p| p.name.as_str() == "auth_lease_lifecycle_publication")
            .expect("handoff protocol auth_lease_lifecycle_publication must be present");
        assert_eq!(protocol.producer_instance.as_str(), "auth_machine");
        assert_eq!(protocol.effect_variant.as_str(), "EmitLifecycleEvent");
        assert_eq!(protocol.realizing_actor.as_str(), "auth_lease_owner");
    }

    #[test]
    fn compat_composition_schemas_is_accessible_and_validates_each_returned_entry() {
        // `compat_composition_schemas()` is invoked by the codegen iteration
        // alongside canonical. After bridge demolition, every entry it returns
        // must validate against the canonical machine registry alone.
        let compositions = compat_composition_schemas();
        let machines = canonical_machine_schemas();
        let machine_refs: Vec<_> = machines.iter().collect();
        for composition in &compositions {
            composition
                .validate_against(&machine_refs)
                .unwrap_or_else(|err| {
                    panic!(
                        "compat composition `{}` failed validation: {err:?}",
                        composition.name
                    )
                });
        }
    }

    /// Negative: EffectExtractor with no `authority_type_path` and no
    /// stacked `HandleBridge` must fail validation. The handoff binding
    /// contract relies on this gate to prevent accidental
    /// extract-obligations-only bindings whose feedback surface has
    /// no home.
    #[test]
    fn effect_extractor_without_authority_must_stack_handle_bridge() {
        use meerkat_machine_schema::{
            ActorKind, ActorSchema, ClosurePolicy, CompositionSchema, CompositionSchemaError,
            CompositionStateLimits, EffectHandoffProtocol, FeedbackFieldBinding,
            FeedbackFieldSource, FeedbackInputRef, MachineInstance, ProtocolGenerationMode,
            ProtocolHelperReturnShape, ProtocolRustBinding,
        };

        let composition = CompositionSchema {
            name: CompositionId::parse("test_effect_extractor_without_authority")
                .expect("valid composition slug"),
            machines: vec![MachineInstance {
                instance_id: MachineInstanceId::parse("meerkat").expect("valid instance_id"),
                machine_name: MachineId::parse("MeerkatMachine").expect("valid machine_name"),
                actor: ActorId::parse("meerkat_authority").expect("valid actor"),
            }],
            actors: vec![
                ActorSchema {
                    name: ActorId::parse("meerkat_authority").expect("valid actor slug"),
                    kind: ActorKind::Machine,
                },
                ActorSchema {
                    name: ActorId::parse("owner").expect("valid actor slug"),
                    kind: ActorKind::Owner,
                },
            ],
            handoff_protocols: vec![EffectHandoffProtocol {
                name: ProtocolId::parse("no_authority_no_handle").expect("valid protocol slug"),
                producer_instance: MachineInstanceId::parse("meerkat")
                    .expect("valid producer_instance"),
                effect_variant: EffectVariantId::parse("RefreshVisibleSurfaceSet")
                    .expect("valid effect_variant"),
                realizing_actor: ActorId::parse("owner").expect("valid realizing_actor"),
                correlation_fields: vec![
                    FieldId::parse("snapshot_epoch").expect("valid field slug"),
                ],
                obligation_fields: vec![
                    FieldId::parse("snapshot_epoch").expect("valid field slug"),
                ],
                allowed_feedback_inputs: vec![FeedbackInputRef {
                    machine_instance: MachineInstanceId::parse("meerkat")
                        .expect("valid machine_instance"),
                    input_variant: InputVariantId::parse("SurfaceSnapshotAligned")
                        .expect("valid input_variant"),
                    field_bindings: vec![FeedbackFieldBinding {
                        input_field: FieldId::parse("epoch").expect("valid input_field"),
                        source: FeedbackFieldSource::ObligationField(
                            FieldId::parse("snapshot_epoch").expect("valid field slug"),
                        ),
                    }],
                }],
                closure_policy: ClosurePolicy::AckRequired,
                liveness_annotation: None,
                rust: ProtocolRustBinding {
                    module_path: "meerkat-mcp/src/generated/test_protocol.rs".into(),
                    generation_mode: ProtocolGenerationMode::EffectExtractor,
                    required_imports: vec![],
                    authority_type_path: None,
                    mutator_trait_path: None,
                    input_enum_path: None,
                    effect_enum_path: Some(
                        "crate::external_tool_surface_authority::ExternalToolSurfaceEffect".into(),
                    ),
                    transition_type_path: None,
                    error_type_path: None,
                    executor_trigger_input_variant: None,
                    bridge_source_type_path: None,
                    helper_return_shape: ProtocolHelperReturnShape::Obligations,
                    handle_trait_path: None,
                    handle_feedback_bindings: vec![],
                    input_payload_module_path: None,
                    additional_modes: vec![],
                },
            }],
            entry_inputs: vec![],
            routes: vec![],
            route_target_selectors: vec![],
            driver: None,
            transaction_plans: vec![],
            actor_priorities: vec![],
            scheduler_rules: vec![],
            invariants: vec![],
            witnesses: vec![],
            deep_domain_cardinality: 2,
            deep_domain_overrides: std::collections::BTreeMap::new(),
            witness_domain_cardinality: 2,
            ci_limits: Some(CompositionStateLimits {
                step_limit: 4,
                pending_input_limit: 4,
                pending_route_limit: 4,
                delivered_route_limit: 0,
                emitted_effect_limit: 0,
                seq_limit: 0,
                set_limit: 0,
                map_limit: 0,
            }),
            closed_world: true,
        };

        let err = composition
            .validate()
            .expect_err("must reject EffectExtractor without authority or stacked HandleBridge");
        match err {
            CompositionSchemaError::InvalidHandoffRustBinding { protocol, detail } => {
                assert_eq!(protocol, "no_authority_no_handle");
                assert!(
                    detail.contains("HandleBridge"),
                    "error detail should mention HandleBridge requirement, got {detail}"
                );
            }
            other => panic!("expected InvalidHandoffRustBinding, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical handoff parity — guard against silent drift between the
// canonical producer effects and the runtime structs/enums they feed.
// ---------------------------------------------------------------------------

#[allow(clippy::expect_used, clippy::panic)]
mod canonical_handoff_parity {
    use meerkat_machine_schema::identity::NamedTypeId;
    use meerkat_machine_schema::{TypeRef, catalog::dsl::dsl_meerkat_machine};

    #[test]
    fn meerkat_wait_all_satisfied_mirrors_runtime_struct() {
        // The canonical `WaitAllSatisfied` effect must name the two
        // fields the runtime's hand-written `WaitAllSatisfied` struct
        // in `meerkat-core/src/ops_lifecycle.rs` exposes:
        //   pub wait_request_id: WaitRequestId,
        //   pub operation_ids: Vec<OperationId>,
        // Drift in either direction silently desyncs the canonical
        // `ops_barrier_satisfaction` handoff obligation.
        let schema = dsl_meerkat_machine();
        let effect = schema
            .effects
            .variants
            .iter()
            .find(|v| v.name.as_str() == "WaitAllSatisfied")
            .expect("MeerkatMachine must declare WaitAllSatisfied effect");
        let field_names: std::collections::BTreeSet<&str> =
            effect.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(
            field_names.contains("wait_request_id"),
            "canonical effect lost `wait_request_id` field — runtime struct has it"
        );
        assert!(
            field_names.contains("operation_ids"),
            "canonical effect lost `operation_ids` field — runtime struct has it"
        );
        assert_eq!(
            field_names.len(),
            2,
            "canonical effect gained extra fields not present on runtime struct — audit both"
        );
        // Type-shape parity: operation_ids must render as a sequence
        // of OperationId, wait_request_id as the typed newtype.
        let wait_request = effect.field_named("wait_request_id").expect("field");
        assert_eq!(
            wait_request.ty,
            TypeRef::Named(NamedTypeId::parse("WaitRequestId").expect("valid NamedTypeId")),
            "canonical wait_request_id must be `WaitRequestId` typed"
        );
        let operation_ids = effect.field_named("operation_ids").expect("field");
        assert!(
            matches!(&operation_ids.ty, TypeRef::Set(inner) if matches!(inner.as_ref(), TypeRef::Named(name) if name.as_str() == "OperationId")),
            "canonical operation_ids must be Set<OperationId>, got {:?}",
            operation_ids.ty
        );
    }

    #[test]
    fn meerkat_refresh_visible_surface_set_mirrors_runtime_struct() {
        let schema = dsl_meerkat_machine();
        let effect = schema
            .effects
            .variants
            .iter()
            .find(|v| v.name.as_str() == "RefreshVisibleSurfaceSet")
            .expect("MeerkatMachine must declare RefreshVisibleSurfaceSet effect");
        assert_eq!(
            effect.fields.len(),
            1,
            "RefreshVisibleSurfaceSet has exactly `snapshot_epoch` — runtime parity"
        );
        assert_eq!(effect.fields[0].name.as_str(), "snapshot_epoch");
        assert_eq!(effect.fields[0].ty, TypeRef::U64);
    }

    #[test]
    fn meerkat_schedule_surface_completion_mirrors_runtime_struct() {
        let schema = dsl_meerkat_machine();
        let effect = schema
            .effects
            .variants
            .iter()
            .find(|v| v.name.as_str() == "ScheduleSurfaceCompletion")
            .expect("MeerkatMachine must declare ScheduleSurfaceCompletion effect");
        let field_names: std::collections::BTreeSet<&str> =
            effect.fields.iter().map(|f| f.name.as_str()).collect();
        // The runtime struct in `meerkat-mcp/src/external_tool_surface_authority.rs`
        // exposes these five fields. Any deletion/addition on either
        // side must sync here or fail the parity gate.
        for required in [
            "surface_id",
            "operation",
            "pending_task_sequence",
            "staged_intent_sequence",
            "applied_at_turn",
        ] {
            assert!(
                field_names.contains(required),
                "canonical ScheduleSurfaceCompletion lost `{required}` — runtime parity violation"
            );
        }
        assert_eq!(
            field_names.len(),
            5,
            "canonical ScheduleSurfaceCompletion gained extra fields not on runtime — audit both"
        );
    }
}

// ---------------------------------------------------------------------------
// D-f: Schedule ↔ Occurrence supersede reciprocal ack route.
//
// The SupersedePendingOccurrences route is one-way (Schedule → Occurrence).
// Per wave-d d.0 #43, the occurrence side must emit a confirmation effect
// that flows back as a Schedule input so the schedule machine observes which
// occurrences it actually superseded.
//
// These tests pin the shape of the reciprocal round-trip:
//   1. OccurrenceLifecycleMachine defines `OccurrencesSuperseded` effect.
//   2. The Supersede transition emits it alongside `Superseded`.
//   3. ScheduleLifecycleMachine defines `ConfirmOccurrencesSuperseded` input.
//   4. A schedule transition consumes the ack and records the outcome.
//   5. `schedule_bundle_composition` registers the reciprocal route wiring
//      the occurrence effect to the schedule input.
// ---------------------------------------------------------------------------

#[test]
fn occurrence_lifecycle_machine_defines_occurrences_superseded_effect() {
    let schema = occurrence_lifecycle_machine();
    let effect_names = schema
        .effects
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        effect_names.contains(&"OccurrencesSuperseded"),
        "OccurrenceLifecycleMachine must define reciprocal-ack effect OccurrencesSuperseded; found {effect_names:?}",
    );
}

#[test]
fn occurrence_supersede_transition_emits_occurrences_superseded() {
    let schema = occurrence_lifecycle_machine();
    let supersede_transitions = schema
        .transitions
        .iter()
        .filter(|transition| transition.on.variant_str() == "Supersede")
        .collect::<Vec<_>>();
    assert!(
        !supersede_transitions.is_empty(),
        "OccurrenceLifecycleMachine must retain at least one Supersede transition"
    );

    for transition in supersede_transitions {
        let emits = transition
            .emit
            .iter()
            .map(|emit| emit.variant.as_str())
            .collect::<Vec<_>>();
        let transition_name = &transition.name;
        assert!(
            emits.contains(&"OccurrencesSuperseded"),
            "Supersede transition {transition_name} must emit OccurrencesSuperseded (found emits: {emits:?})",
        );
    }
}

#[test]
fn schedule_lifecycle_machine_defines_confirm_occurrences_superseded_input() {
    let schema = schedule_lifecycle_machine();
    let input_names = schema
        .inputs
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        input_names.contains(&"ConfirmOccurrencesSuperseded"),
        "ScheduleLifecycleMachine must define reciprocal-ack input ConfirmOccurrencesSuperseded; found {input_names:?}",
    );
}

#[test]
fn schedule_has_transition_consuming_confirm_occurrences_superseded() {
    let schema = schedule_lifecycle_machine();
    let transitioned = schema
        .transitions
        .iter()
        .map(|transition| transition.on.variant_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        transitioned.contains("ConfirmOccurrencesSuperseded"),
        "ScheduleLifecycleMachine must model ConfirmOccurrencesSuperseded with at least one transition"
    );
}

#[test]
fn schedule_bundle_registers_occurrences_superseded_reciprocal_route() {
    let composition = meerkat_machine_schema::catalog::schedule_bundle_composition();
    let reciprocal = composition
        .routes
        .iter()
        .find(|route| {
            route.from_machine.as_str() == "occurrence"
                && route.effect_variant.as_str() == "OccurrencesSuperseded"
        })
        .expect(
            "schedule_bundle must register a reciprocal route from occurrence.OccurrencesSuperseded",
        );

    assert_eq!(
        reciprocal.to.machine.as_str(),
        "schedule",
        "reciprocal ack must target the schedule instance"
    );
    match &reciprocal.to.input_variant {
        RouteVariantId::Input(input) => assert_eq!(
            input.as_str(),
            "ConfirmOccurrencesSuperseded",
            "reciprocal ack must target ConfirmOccurrencesSuperseded input"
        ),
        other => panic!("reciprocal ack route must target an Input variant, got {other:?}"),
    }
}

#[test]
fn schedule_bundle_validates_with_reciprocal_ack_route() {
    let composition = meerkat_machine_schema::catalog::schedule_bundle_composition();
    let canonical_machines = canonical_machine_schemas();
    let canonical_machine_refs = canonical_machines.iter().collect::<Vec<_>>();
    assert_eq!(
        composition.validate_against(&canonical_machine_refs),
        Ok(()),
        "schedule_bundle composition must validate after the reciprocal ack is wired"
    );
}
