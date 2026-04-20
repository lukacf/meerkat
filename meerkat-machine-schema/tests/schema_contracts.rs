use std::collections::BTreeMap;

use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
    dsl_occurrence_lifecycle_machine as occurrence_lifecycle_machine,
    dsl_schedule_lifecycle_machine as schedule_lifecycle_machine,
};
use meerkat_machine_schema::{
    CompositionSchemaError, canonical_composition_coverage_manifests,
    canonical_composition_schemas, canonical_machine_coverage_manifests, canonical_machine_schemas,
    meerkat_mob_seam_composition,
};

#[test]
fn canonical_machine_registry_contains_only_two_kernel_and_perimeter_entries() {
    let names = canonical_machine_schemas()
        .into_iter()
        .map(|schema| schema.machine)
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
            "AuthMachine"
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
fn canonical_composition_registry_contains_kernel_seam_and_schedule_perimeter_entries() {
    let names = canonical_composition_schemas()
        .into_iter()
        .map(|schema| schema.name)
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        vec![
            "meerkat_mob_seam",
            "schedule_bundle",
            "schedule_runtime_bundle",
            "schedule_mob_bundle",
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
        .position(|route| route.name == "binding_request_reaches_meerkat");
    assert!(route_idx.is_some(), "binding request route");
    let Some(route_idx) = route_idx else {
        return;
    };
    let route = &mut composition.routes[route_idx];
    let generation_binding_idx = route
        .bindings
        .iter()
        .position(|binding| binding.to_field == "generation");
    assert!(generation_binding_idx.is_some(), "generation binding");
    let Some(generation_binding_idx) = generation_binding_idx else {
        return;
    };
    let generation_binding = &mut route.bindings[generation_binding_idx];
    generation_binding.source = meerkat_machine_schema::RouteBindingSource::Field {
        from_field: "fence_token".into(),
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
fn schedule_and_occurrence_machines_stay_in_canonical_coverage_manifests() {
    let machine_names = canonical_machine_schemas()
        .into_iter()
        .map(|schema| schema.machine)
        .collect::<Vec<_>>();
    let coverage_names = canonical_machine_coverage_manifests()
        .into_iter()
        .map(|manifest| manifest.machine)
        .collect::<Vec<_>>();

    for name in [
        schedule_lifecycle_machine().machine,
        occurrence_lifecycle_machine().machine,
    ] {
        assert!(
            machine_names.iter().any(|machine| machine == &name),
            "{name} should remain canonical"
        );
        assert!(
            coverage_names.iter().any(|machine| machine == &name),
            "{name} should retain coverage metadata"
        );
    }
}

#[test]
fn kernel_seam_retains_coverage_metadata() {
    let coverage_names = canonical_composition_coverage_manifests()
        .into_iter()
        .map(|manifest| manifest.composition)
        .collect::<Vec<_>>();

    assert_eq!(
        coverage_names,
        vec![
            "meerkat_mob_seam",
            "schedule_bundle",
            "schedule_runtime_bundle",
            "schedule_mob_bundle",
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
        "InterruptCurrentRun",
        "CancelAfterBoundary",
        "ReconfigureSessionLlmIdentity",
        "StagePersistentFilter",
        "RequestDeferredTools",
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
        "StartConversationRun",
        "StageAdd",
        "StageRemove",
        "StageReload",
        "PendingSucceeded",
        "SnapshotAligned",
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
        "StageAddAttached",
        "StageAddRunning",
        "StageRemoveAttached",
        "StageRemoveRunning",
        "StageReloadAttached",
        "StageReloadRunning",
        "ApplySurfaceBoundaryAttached",
        "ApplySurfaceBoundaryRunning",
        "PendingSucceededAttached",
        "PendingSucceededRunning",
        "FinalizeRemovalCleanAttached",
        "FinalizeRemovalCleanRunning",
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
        "Wire",
        "Unwire",
        "SubmitWork",
        "TaskCreate",
        "TaskUpdate",
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

    for required in ["EmitFlowRunNotice", "NotifyCoordinator", "EmitTaskNotice"] {
        assert!(
            effect_names.iter().any(|name| name == &required),
            "MobMachine should absorb effect {required}"
        );
    }
}

#[test]
fn mob_machine_merges_flow_task_wiring_and_runtime_bridge_state() {
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
        "WireRunning",
        "UnwireRunning",
        "StageSpawnRunning",
        "CompleteSpawnRunning",
        "TaskCreateRunning",
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
        "Retire",
        "Respawn",
        "RetireAll",
        "Wire",
        "Unwire",
        "SubmitWork",
        "CancelAllWork",
        "Stop",
        "Resume",
        "Complete",
        "Reset",
        "Destroy",
        "TaskCreate",
        "TaskUpdate",
        "TaskList",
        "TaskGet",
        "McpServerStates",
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
        .map(|transition| transition.on.variant.as_str())
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
        .map(|transition| transition.on.variant.as_str())
        .collect::<std::collections::BTreeSet<_>>();

    for required in [
        "RunFlow",
        "CancelFlow",
        "Spawn",
        "Retire",
        "Respawn",
        "RetireAll",
        "Wire",
        "Unwire",
        "SubmitWork",
        "CancelAllWork",
        "Stop",
        "Resume",
        "Complete",
        "Reset",
        "Destroy",
        "TaskCreate",
        "TaskUpdate",
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
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let meerkat_transitioned = meerkat
        .transitions
        .iter()
        .map(|transition| transition.on.variant.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for required in [
        "ContainsSession",
        "SessionHasExecutor",
        "SessionHasComms",
        "OpsLifecycleRegistry",
        "InputState",
        "ListActiveInputs",
        "RuntimeState",
        "RuntimeRealtimeAttachmentStatus",
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
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let mob_transitioned = mob
        .transitions
        .iter()
        .map(|transition| transition.on.variant.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for required in [
        "FlowStatus",
        "TaskList",
        "TaskGet",
        "McpServerStates",
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

#[test]
fn every_canonical_input_variant_has_transition_coverage() {
    for schema in canonical_machine_schemas() {
        let surface_only_inputs = schema
            .surface_only_inputs
            .iter()
            .map(String::as_str)
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
            .map(|transition| transition.on.variant.as_str())
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
