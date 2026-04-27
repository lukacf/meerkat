//! DSL-generated machine schemas.
//!
//! These modules contain `machine!` invocations that generate the same
//! `MachineSchema` values as the hand-written catalog entries. They use
//! `rust: "self"` so the generated `schema()` function references
//! `crate::MachineSchema` instead of `meerkat_machine_schema::MachineSchema`.
#![allow(
    dead_code,
    unused_variables,
    unreachable_code,
    clippy::cmp_owned,
    clippy::assign_op_pattern
)]

/// Extension trait providing `.get()` on Option to support the `option_value`
/// schema pattern (`Expr::MapGet { map: Field(...), key: String("value") }`).
/// In the runtime dispatch code, `.get("value")` extracts the inner value.
/// This is only used by the generated dispatch code in this crate (which is
/// dead code — only the `schema()` function is called).
trait OptionValueExt<T: Clone> {
    fn get(&self, _key: &str) -> T;
}
impl<T: Clone + Default> OptionValueExt<T> for Option<T> {
    fn get(&self, _key: &str) -> T {
        self.clone().unwrap_or_default()
    }
}

pub mod auth_machine;
pub mod meerkat_machine;
pub mod mob_machine;
pub mod occurrence_lifecycle;
pub mod schedule_lifecycle;

use crate::identity::InputVariantId;
use crate::{MachineSchema, NamedTypeBinding};

trait RuntimeInternalInputVariant: Copy {
    fn input_variant_id(self) -> InputVariantId;
}

macro_rules! runtime_internal_inputs {
    ($type_name:ident, $const_name:ident, [$($variant:ident),+ $(,)?]) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum $type_name {
            $($variant),+
        }

        const $const_name: &[$type_name] = &[
            $($type_name::$variant),+
        ];

        impl RuntimeInternalInputVariant for $type_name {
            fn input_variant_id(self) -> InputVariantId {
                InputVariantId::from_trusted_catalog_literal(match self {
                    $(Self::$variant => stringify!($variant),)+
                })
            }
        }
    };
}

/// Attach authoritative [`NamedTypeBinding`]s to a DSL-generated schema.
///
/// The `machine!` macro does not yet emit `named_types` entries, so the
/// catalog wrappers supply them here. This is the single source of truth
/// for how DSL-declared named types lower to Rust atoms — the codegen
/// consults these bindings rather than matching on type names.
fn with_named_types(mut schema: MachineSchema, bindings: Vec<NamedTypeBinding>) -> MachineSchema {
    schema.named_types = bindings;
    schema
}

fn input_variant_ids<T: RuntimeInternalInputVariant>(
    variants: &'static [T],
) -> Vec<InputVariantId> {
    variants
        .iter()
        .copied()
        .map(RuntimeInternalInputVariant::input_variant_id)
        .collect()
}

fn with_runtime_internal_inputs(
    mut schema: MachineSchema,
    inputs: Vec<InputVariantId>,
) -> MachineSchema {
    schema.runtime_internal_inputs = inputs;
    schema
}

pub fn dsl_auth_machine() -> MachineSchema {
    with_named_types(
        auth_machine::AuthMachineState::schema(),
        vec![NamedTypeBinding::string("AuthLifecyclePhase")],
    )
}

pub fn dsl_meerkat_machine() -> MachineSchema {
    let schema = with_named_types(
        meerkat_machine::MeerkatMachineState::schema(),
        vec![
            NamedTypeBinding::u64("BoundarySequence"),
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::u64("Generation"),
            NamedTypeBinding::string("AgentRuntimeId"),
            NamedTypeBinding::string("CommsRuntimeId"),
            NamedTypeBinding::string("InputId"),
            NamedTypeBinding::string("McpServerId"),
            NamedTypeBinding::string("MeerkatPhase"),
            NamedTypeBinding::string("MobId"),
            NamedTypeBinding::string("OperationId"),
            NamedTypeBinding::string("OperationKind"),
            NamedTypeBinding::string("PeerCorrelationId"),
            NamedTypeBinding::string("RunId"),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("SessionLlmCapabilitySurface"),
            NamedTypeBinding::string("SessionLlmCapabilitySurfaceStatus"),
            NamedTypeBinding::string("SessionLlmIdentity"),
            NamedTypeBinding::string("SessionToolVisibilityDelta"),
            NamedTypeBinding::string("SessionToolVisibilityState"),
            NamedTypeBinding::string("SurfaceId"),
            NamedTypeBinding::string("ToolFilter"),
            NamedTypeBinding::string("ToolVisibilityWitness"),
            NamedTypeBinding::u64("TurnNumber"),
            NamedTypeBinding::string("WaitRequestId"),
            NamedTypeBinding::string("WorkId"),
            // Wave-c C-6r: typed PeerEndpoint twin.
            NamedTypeBinding::type_path(
                "PeerEndpoint",
                "crate::catalog::dsl::meerkat_machine::PeerEndpoint",
            ),
            NamedTypeBinding::type_path(
                "PeerName",
                "crate::catalog::dsl::meerkat_machine::PeerName",
            ),
            NamedTypeBinding::type_path("PeerId", "crate::catalog::dsl::meerkat_machine::PeerId"),
            NamedTypeBinding::type_path(
                "PeerAddress",
                "crate::catalog::dsl::meerkat_machine::PeerAddress",
            ),
        ],
    );
    with_runtime_internal_inputs(
        schema,
        input_variant_ids(MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS),
    )
}

runtime_internal_inputs!(
    MeerkatMachineRuntimeInternalInput,
    MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS,
    [
        AbortLiveTopologyBeforeDetach,
        AddDirectPeerEndpoint,
        AdvanceSessionContext,
        ApplyLiveTopologyIdentity,
        ApplyLiveTopologyVisibility,
        ApplyMobPeerOverlay,
        AttachMobIngress,
        AttachSessionIngress,
        AuthorizeSupervisor,
        BeginLiveTopologyReconfigure,
        BeginRealtimeBinding,
        BindSupervisor,
        ClassifyRealtimeClientInputSubmitted,
        ClassifyRealtimeMidTurnActivity,
        ClassifyRealtimeTurnTerminated,
        ClearLocalEndpoint,
        ClearRealtimeReconnectProgress,
        CompleteUntilChangedSwitchTurnReconfigure,
        CompleteLiveTopology,
        DetachIngress,
        DetachRealtimeBinding,
        FailLiveTopologyAfterDetach,
        InteractionStreamAttached,
        InteractionStreamClosedEarly,
        InteractionStreamCompleted,
        InteractionStreamExpired,
        InteractionStreamReserved,
        MarkLiveTopologyDetached,
        McpServerConnectPending,
        McpServerConnected,
        McpServerDisconnected,
        McpServerFailed,
        McpServerReload,
        ModelRoutingStatus,
        OpsBarrierSatisfied,
        PeerRequestReceived,
        PeerRequestSent,
        PeerRequestTimedOut,
        PeerResponseProgressArrived,
        PeerResponseReplied,
        PeerResponseTerminalArrived,
        PendingFailed,
        PendingSucceeded,
        ProductOutputStarted,
        ProductTurnCommitted,
        ProductTurnInFlight,
        ProductTurnInterrupted,
        ProductTurnTerminal,
        ProjectRealtimeIntent,
        ProjectRealtimeReconnectProgress,
        PublishLocalEndpoint,
        PublishRealtimeSignal,
        RealtimeProjectionAdvanceObserved,
        RealtimeProjectionRefreshed,
        RealtimeProjectionReset,
        RecoverInputLifecycle,
        RemoveDirectPeerEndpoint,
        ReplaceRealtimeBinding,
        RequestFiniteSwitchTurn,
        RequestUntilChangedSwitchTurn,
        RequireRealtimeReattach,
        RequireRealtimeReattachForAuthority,
        RevokeSupervisor,
        SetModelRoutingBaseline,
        SnapshotAligned,
        SupervisorTrustEdgePublishFailed,
        SupervisorTrustEdgePublished,
        SupervisorTrustEdgeRevokeFailed,
        SupervisorTrustEdgeRevoked,
    ]
);

pub fn dsl_mob_machine() -> MachineSchema {
    let schema = with_named_types(
        mob_machine::MobMachineState::schema(),
        vec![
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::u64("Generation"),
            NamedTypeBinding::string("AgentIdentity"),
            NamedTypeBinding::string("AgentRuntimeId"),
            NamedTypeBinding::type_path(
                "ExternalPeerEdge",
                "crate::catalog::dsl::mob_machine::ExternalPeerEdge",
            ),
            NamedTypeBinding::type_path(
                "ExternalPeerEndpoint",
                "crate::catalog::dsl::mob_machine::ExternalPeerEndpoint",
            ),
            NamedTypeBinding::string("MobId"),
            NamedTypeBinding::string("MobMemberState"),
            NamedTypeBinding::string("MobPhase"),
            NamedTypeBinding::string("MobTask"),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("TaskId"),
            NamedTypeBinding::string("TaskStatus"),
            NamedTypeBinding::string("WiringEdge"),
            NamedTypeBinding::string("WorkId"),
            NamedTypeBinding::type_path(
                "PeerAddress",
                "crate::catalog::dsl::mob_machine::PeerAddress",
            ),
            NamedTypeBinding::type_path("PeerId", "crate::catalog::dsl::mob_machine::PeerId"),
            NamedTypeBinding::type_path("PeerName", "crate::catalog::dsl::mob_machine::PeerName"),
            NamedTypeBinding::type_path(
                "PeerSigningKey",
                "crate::catalog::dsl::mob_machine::PeerSigningKey",
            ),
        ],
    );
    with_runtime_internal_inputs(
        schema,
        input_variant_ids(MOB_MACHINE_RUNTIME_INTERNAL_INPUTS),
    )
}

runtime_internal_inputs!(
    MobMachineRuntimeInternalInput,
    MOB_MACHINE_RUNTIME_INTERNAL_INPUTS,
    [
        BindMemberSession,
        ReleaseMemberSession,
        RotateMemberSession,
        SessionIngressDetachFailedForMobDestroy,
        SessionIngressDetachedForMobDestroy,
        UnwireExternalPeer,
        UnwireMembers,
        WireExternalPeer,
        WireMembers,
    ]
);

pub fn dsl_schedule_lifecycle_machine() -> MachineSchema {
    with_named_types(
        schedule_lifecycle::ScheduleLifecycleMachineState::schema(),
        // Schedule-side reciprocal ack tracks `Set<OccurrenceId>` and
        // receives `ConfirmOccurrencesSuperseded { occurrence_id }` from
        // the occurrence authority; both sides must agree on the atom.
        vec![
            NamedTypeBinding::string("OccurrenceId"),
            NamedTypeBinding::string("ScheduleLifecycleState"),
        ],
    )
}

pub fn dsl_occurrence_lifecycle_machine() -> MachineSchema {
    with_named_types(
        occurrence_lifecycle::OccurrenceLifecycleMachineState::schema(),
        vec![
            NamedTypeBinding::string("ClaimToken"),
            NamedTypeBinding::string("DeliveryReceipt"),
            NamedTypeBinding::string("OccurrenceId"),
            NamedTypeBinding::string("OccurrenceLifecycleState"),
            NamedTypeBinding::string("ScheduleId"),
        ],
    )
}
