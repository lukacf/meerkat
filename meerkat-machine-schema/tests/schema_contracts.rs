use std::collections::BTreeMap;

use meerkat_machine_schema::{
    CompositionSchemaError, canonical_composition_coverage_manifests,
    canonical_composition_schemas, canonical_machine_coverage_manifests, canonical_machine_schemas,
    meerkat_machine, meerkat_mob_seam_composition, mob_machine, occurrence_lifecycle_machine,
    schedule_lifecycle_machine,
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
            "OccurrenceLifecycleMachine"
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
        "RegisterOperation",
        "StartConversationRun",
        "StageAdd",
        "StageRemove",
        "StageReload",
        "PendingSucceeded",
        "SnapshotAligned",
        "ReconcileResolvedDirectory",
        "StagePersistentFilter",
        "RequestDeferredTools",
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

    for required in [
        "interrupt_pending",
        "shutdown_pending",
        "inherited_base_filter",
        "active_filter",
        "staged_filter",
        "active_requested_deferred_names",
        "staged_requested_deferred_names",
        "requested_witnesses",
        "filter_witnesses",
        "active_visibility_revision",
        "staged_visibility_revision",
        "committed_visibility_revision",
        "resolved_peer_keys",
        "peer_reachability",
        "peer_last_reason",
    ] {
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
        "AcceptWithCompletionAttached",
        "AcceptWithCompletionRunning",
        "AcceptWithoutWakeAttached",
        "AcceptWithoutWakeRunning",
        "IngestAttached",
        "IngestRunning",
        "PublishEventAttached",
        "PublishEventRunning",
        "StagePersistentFilterAttached",
        "StagePersistentFilterRunning",
        "RequestDeferredToolsAttached",
        "RequestDeferredToolsRunning",
        "BoundaryAppliedPromote",
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
        "ReconcileResolvedDirectoryAttached",
        "ReconcileResolvedDirectoryRunning",
        "RecordSendSucceededAttached",
        "RecordSendSucceededRunning",
        "RecordSendFailedAttached",
        "RecordSendFailedRunning",
    ] {
        assert!(
            transition_names.iter().any(|name| name == &required),
            "MeerkatMachine should expose absorbed transition {required}"
        );
    }

    for required in ["WakeInterrupt", "CommittedVisibleSetPublished"] {
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
        "ExternalTurn",
        "InternalTurn",
        "TaskCreate",
        "TaskUpdate",
        "SubscribeMobEvents",
    ] {
        assert!(
            input_names.iter().any(|name| name == &required),
            "MobMachine should absorb input {required}"
        );
    }

    for required in [
        "StageSpawn",
        "KickoffStarted",
        "RuntimeRunSubmitted",
        "CreateRun",
        "RegisterTargets",
        "StartRootFrame",
        "StartLoop",
        "BodyFrameCompleted",
        "UntilConditionFailed",
    ] {
        assert!(
            signal_names.iter().any(|name| name == &required),
            "MobMachine should absorb signal {required}"
        );
    }

    for required in [
        "EmitFlowRunNotice",
        "PersistStepOutput",
        "AdmitStepWork",
        "NotifyCoordinator",
        "AdmitKickoffTurn",
        "RequestBodyFrameStart",
        "LoopCompleted",
        "EmitTaskNotice",
    ] {
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

    for required in [
        "pending_spawn_count",
        "retiring_member_count",
        "wiring_edge_count",
        "task_count",
        "event_subscription_count",
        "active_frame_count",
        "active_loop_count",
        "coordinator_bound",
        "kickoff_pending",
    ] {
        assert!(
            field_names.iter().any(|name| name == &required),
            "MobMachine state should retain absorbed field {required}"
        );
    }

    for required in [
        "RetireRunning",
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
        "StartRootFrameRunning",
        "FrameTerminatedRunning",
        "StartLoopRunning",
        "BodyFrameCompletedRunning",
        "BodyFrameFailedRunning",
        "BodyFrameCanceledRunning",
        "UntilConditionFailedRunning",
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
        "ExternalTurn",
        "InternalTurn",
        "SubmitWork",
        "CancelWork",
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
        "PrepareBindings",
        "PublishCommittedVisibleSet",
        "SetPeerIngressContext",
        "NotifyDrainExited",
        "AbortAll",
        "Abort",
        "Ingest",
        "PublishEvent",
        "Retire",
        "Recycle",
        "Reset",
        "Recover",
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
        "ExternalTurn",
        "InternalTurn",
        "SubmitWork",
        "CancelWork",
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
fn every_query_runtime_command_has_transition_coverage() {
    let meerkat = meerkat_machine();
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
        "LoadBoundaryReceipt",
        "Wait",
    ] {
        assert!(
            meerkat_transitioned.contains(required),
            "MeerkatMachine query command {required} should have transition coverage"
        );
    }

    let mob = mob_machine();
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
        "PollEvents",
        "ReplayAllEvents",
        "GetMember",
    ] {
        assert!(
            mob_transitioned.contains(required),
            "MobMachine query command {required} should have transition coverage"
        );
    }
}

#[test]
fn every_canonical_input_variant_has_transition_coverage() {
    for schema in canonical_machine_schemas() {
        let input_names = schema
            .inputs
            .variants
            .iter()
            .map(|variant| variant.name.as_str())
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
