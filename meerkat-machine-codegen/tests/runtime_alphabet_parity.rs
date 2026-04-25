#![allow(clippy::redundant_closure_for_method_calls)]

use meerkat_machine_schema::TriggerKind;
use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
};
use meerkat_mob::canonical_mob_machine_command_manifest;
use meerkat_runtime::canonical_meerkat_machine_command_manifest;
use std::collections::BTreeSet;

fn variant_names<'a>(
    variants: impl IntoIterator<Item = &'a meerkat_machine_schema::VariantSchema>,
) -> BTreeSet<&'a str> {
    variants
        .into_iter()
        .map(|variant| variant.name.as_str())
        .collect()
}

#[test]
fn meerkat_machine_inputs_equal_runtime_manifest_exactly() {
    let schema = meerkat_machine();
    // Realtime-attachment + live-topology DSL inputs are staged directly
    // via `stage_session_dsl_input` from MeerkatMachine public methods
    // (see session_management.rs, llm_reconfigure.rs). They are internal
    // DSL transitions, not dispatched through the MeerkatMachineCommand
    // command channel — so they have no runtime manifest variants by
    // design. Exclude them from the exact-parity assertion.
    const DSL_INTERNAL_INPUTS: &[&str] = &[
        "ProjectRealtimeIntent",
        "BeginRealtimeBinding",
        "ReplaceRealtimeBinding",
        "DetachRealtimeBinding",
        "RequireRealtimeReattach",
        "PublishRealtimeSignal",
        "BeginLiveTopologyReconfigure",
        "MarkLiveTopologyDetached",
        "ApplyLiveTopologyIdentity",
        "ApplyLiveTopologyVisibility",
        "CompleteLiveTopology",
        "AbortLiveTopologyBeforeDetach",
        "FailLiveTopologyAfterDetach",
        // Phase 5G / T5g: MCP server handshake lifecycle is driven via
        // `McpServerLifecycleHandle` trait (meerkat-core/src/handles.rs)
        // staged by the MCP router shell, not through
        // `MeerkatMachineCommand`.
        "McpServerConnectPending",
        "McpServerConnected",
        "McpServerDisconnected",
        "McpServerFailed",
        "McpServerReload",
        // W1-A (issue #264): peer-interaction lifecycle is driven via
        // `PeerInteractionHandle` (meerkat-core/src/handles.rs) from the
        // comms runtime and drain task, not through `MeerkatMachineCommand`.
        "PeerRequestSent",
        "PeerResponseProgressArrived",
        "PeerResponseTerminalArrived",
        "PeerRequestTimedOut",
        "PeerRequestReceived",
        "PeerResponseReplied",
        // W2-E (issue #264): session-context advancement is driven via
        // `SessionContextHandle` (meerkat-core/src/handles.rs) from the
        // session task's `publish_summary` path, not through
        // `MeerkatMachineCommand`.
        "AdvanceSessionContext",
        // U6 (dogma #5): interaction stream lifecycle is driven via
        // `InteractionStreamHandle` (meerkat-core/src/handles.rs) from the
        // comms runtime's `stream()` / `mark_interaction_complete` /
        // `reap_expired_reservations` / `InteractionStream::finish` paths,
        // not through `MeerkatMachineCommand`.
        "InteractionStreamReserved",
        "InteractionStreamAttached",
        "InteractionStreamCompleted",
        "InteractionStreamExpired",
        "InteractionStreamClosedEarly",
        // W2-G (issue #264): peer-ingress transport capability ownership is
        // staged by `stage_peer_ingress_ownership_dsl` from inside the
        // `SetPeerIngressContext` command handler, not as a separately
        // dispatched runtime command. The typed `AttachSessionIngress`,
        // `AttachMobIngress`, and `DetachIngress` inputs are DSL-internal.
        "AttachSessionIngress",
        "AttachMobIngress",
        "DetachIngress",
        // Wave 3 D Row 21: supervisor-bridge authorization is staged by
        // `stage_supervisor_{bind,authorize,revoke}` from the comms drain
        // task, not dispatched through `MeerkatMachineCommand`. The typed
        // `BindSupervisor`, `AuthorizeSupervisor`, and `RevokeSupervisor`
        // inputs are DSL-internal, analogous to the peer-ingress ownership
        // inputs above.
        "BindSupervisor",
        "AuthorizeSupervisor",
        "RevokeSupervisor",
        // Wave-d D-d: supervisor-trust-edge feedback acks for the
        // `supervisor_trust_publish` / `supervisor_trust_revoke` handoff
        // protocols. Staged by `stage_supervisor_trust_{published,
        // publish_failed, revoked, revoke_failed}` from the
        // `try_handle_supervisor_bridge_command` path after the shell
        // calls `Router::{add,remove}_trusted_peer`. DSL-internal; not
        // dispatched through `MeerkatMachineCommand`.
        "SupervisorTrustEdgePublished",
        "SupervisorTrustEdgePublishFailed",
        "SupervisorTrustEdgeRevoked",
        "SupervisorTrustEdgeRevokeFailed",
        // U9 (dogma #4): realtime product-turn lifecycle is driven via
        // `RealtimeProductTurnHandle` (meerkat-core/src/handles.rs) from the
        // realtime-WS dispatch loop in `meerkat-rpc::realtime_ws`, not
        // through `MeerkatMachineCommand`.
        "ProductTurnInFlight",
        "ProductTurnCommitted",
        "ProductOutputStarted",
        "ProductTurnInterrupted",
        "ProductTurnTerminal",
        // Dogma round 2 U-C (dogma #1, #3, #13, #18, #20): realtime
        // projection freshness + reconnect policy are driven via
        // `RealtimeProductTurnHandle` (meerkat-core/src/handles.rs) from
        // the realtime-WS dispatch loop, not through
        // `MeerkatMachineCommand`.
        "RealtimeProjectionAdvanceObserved",
        "RealtimeProjectionRefreshed",
        "RealtimeProjectionReset",
        "ClassifyRealtimeClientInputSubmitted",
        "ClassifyRealtimeMidTurnActivity",
        "ClassifyRealtimeTurnTerminated",
        // Round-5 Track-A (PR #336): ops-barrier satisfaction is
        // driven via `TurnStateHandle::ops_barrier_satisfied`
        // (meerkat-core/src/handles.rs) from the agent's state-loop
        // `WaitingForOps` branch via the
        // `protocol_ops_barrier_satisfaction` handoff helper, not
        // through `MeerkatMachineCommand`.
        "OpsBarrierSatisfied",
        // Surface handoff feedback is delivered through
        // `ExternalToolSurfaceHandle` / generated surface protocol helpers,
        // not through `MeerkatMachineCommand`. Runtime command variants
        // keep the shell-facing `Surface*` names while the canonical DSL
        // owns the protocol feedback names.
        "PendingSucceeded",
        "PendingFailed",
        "SnapshotAligned",
        // Round-5 Track-B (PR #340): peer-projection overlay inputs.
        // `ApplyMobPeerOverlay` is dispatched by the
        // `RecomputeMobPeerOverlay` composition driver (see
        // `meerkat_mob_seam_composition` driver block). The local-
        // endpoint and direct-peer inputs are staged by the peering
        // surface shell paths (not through `MeerkatMachineCommand`),
        // analogous to the peer-ingress ownership inputs above.
        "ApplyMobPeerOverlay",
        "PublishLocalEndpoint",
        "ClearLocalEndpoint",
        "AddDirectPeerEndpoint",
        "RemoveDirectPeerEndpoint",
        "ProjectRealtimeReconnectProgress",
        "ClearRealtimeReconnectProgress",
    ];
    let actual: BTreeSet<&str> = variant_names(&schema.inputs.variants)
        .into_iter()
        .filter(|name| !DSL_INTERNAL_INPUTS.contains(name))
        .collect();
    let expected: BTreeSet<&str> = canonical_meerkat_machine_command_manifest()
        .into_iter()
        .filter(|name| *name != "RuntimeRealtimeChannelStatus")
        .collect();

    assert_eq!(actual, expected);
}

#[test]
fn mob_machine_inputs_equal_runtime_manifest_exactly() {
    let schema = mob_machine();
    // Phase 5G declarative API additions (T4a/T4b/T5d) introduced
    // `EnsureMember`, `Reconcile`, and `ListMembersMatching` as
    // runtime-command composition helpers that dispatch through the
    // MobHandle without their own DSL transition entries — they layer on
    // top of `Spawn`/`Retire`/`ListMembers` rather than adding new
    // semantic facts. Exclude them from the exact-parity assertion until
    // the DSL schema is extended to model their composition seams (if
    // ever; they may stay shell-level by design).
    const RUNTIME_COMPOSITION_ONLY_COMMANDS: &[&str] =
        &["EnsureMember", "Reconcile", "ListMembersMatching"];
    const DSL_ONLY_INPUTS: &[&str] = &[
        "WireMembers",
        "UnwireMembers",
        "BindMemberSession",
        "RotateMemberSession",
        "ReleaseMemberSession",
        "SessionIngressDetachedForMobDestroy",
        "SessionIngressDetachFailedForMobDestroy",
    ];
    let actual: BTreeSet<&str> = variant_names(&schema.inputs.variants)
        .into_iter()
        .filter(|name| !DSL_ONLY_INPUTS.contains(name))
        .collect();
    let expected: BTreeSet<&str> = canonical_mob_machine_command_manifest()
        .into_iter()
        .filter(|name| {
            !RUNTIME_COMPOSITION_ONLY_COMMANDS.contains(name) && !["Wire", "Unwire"].contains(name)
        })
        .collect();

    assert_eq!(actual, expected);
}

#[test]
fn every_canonical_input_has_transition_coverage() {
    for schema in [meerkat_machine(), mob_machine()] {
        let surface_only_inputs: BTreeSet<&str> = schema
            .surface_only_inputs
            .iter()
            .map(|v| v.as_str())
            .collect();
        let covered: BTreeSet<&str> = schema
            .transitions
            .iter()
            .filter(|transition| transition.on.kind() == TriggerKind::Input)
            .map(|transition| transition.on.variant_str())
            .collect();

        for input in &schema.inputs.variants {
            if surface_only_inputs.contains(input.name.as_str()) {
                continue;
            }
            assert!(
                covered.contains(input.name.as_str()),
                "{} input `{}` has no transition coverage",
                schema.machine,
                input.name
            );
        }
    }
}

#[test]
fn every_canonical_signal_has_transition_coverage() {
    for schema in [meerkat_machine(), mob_machine()] {
        let covered: BTreeSet<&str> = schema
            .transitions
            .iter()
            .filter(|transition| transition.on.kind() == TriggerKind::Signal)
            .map(|transition| transition.on.variant_str())
            .collect();

        for signal in &schema.signals.variants {
            assert!(
                covered.contains(signal.name.as_str()),
                "{} signal `{}` has no transition coverage",
                schema.machine,
                signal.name
            );
        }
    }
}
