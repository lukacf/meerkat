use serde::Serialize;

use crate::RpcRequestLifecycleRule;

#[derive(Debug, Clone, Copy)]
pub struct RpcMethodCatalogOptions {
    pub runtime_available: bool,
    pub mob_enabled: bool,
    pub mcp_enabled: bool,
    pub comms_enabled: bool,
    pub blob_enabled: bool,
    pub session_events_enabled: bool,
    pub session_streams_enabled: bool,
    pub schedule_enabled: bool,
    pub skills_enabled: bool,
}

impl RpcMethodCatalogOptions {
    pub const fn documented_surface() -> Self {
        Self {
            runtime_available: true,
            mob_enabled: true,
            mcp_enabled: true,
            comms_enabled: true,
            blob_enabled: true,
            session_events_enabled: true,
            session_streams_enabled: true,
            schedule_enabled: true,
            skills_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RpcMethodDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_type: Option<&'static str>,
    #[serde(skip_serializing)]
    pub request_lifecycle: RpcRequestLifecycleRule,
}

impl RpcMethodDescriptor {
    const fn basic(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            description,
            params_type: None,
            result_type: None,
            request_lifecycle: RpcRequestLifecycleRule::INLINE_OBSERVATION,
        }
    }

    const fn typed(
        name: &'static str,
        description: &'static str,
        params_type: &'static str,
        result_type: &'static str,
    ) -> Self {
        Self {
            name,
            description,
            params_type: Some(params_type),
            result_type: Some(result_type),
            request_lifecycle: RpcRequestLifecycleRule::INLINE_OBSERVATION,
        }
    }

    const fn with_request_lifecycle(mut self, request_lifecycle: RpcRequestLifecycleRule) -> Self {
        self.request_lifecycle = request_lifecycle;
        self
    }

    const fn result_only(
        name: &'static str,
        description: &'static str,
        result_type: &'static str,
    ) -> Self {
        Self {
            name,
            description,
            params_type: None,
            result_type: Some(result_type),
            request_lifecycle: RpcRequestLifecycleRule::INLINE_OBSERVATION,
        }
    }
}

pub fn rpc_method_catalog(options: RpcMethodCatalogOptions) -> Vec<RpcMethodDescriptor> {
    let mut methods = vec![
        RpcMethodDescriptor::basic("initialize", "Handshake, returns server capabilities"),
        RpcMethodDescriptor::typed(
            "session/create",
            "Create session + run first turn",
            "CreateSessionParams",
            "WireRunResult | DeferredCreateResult",
        )
        .with_request_lifecycle(RpcRequestLifecycleRule::SessionCreateInitialTurn),
        RpcMethodDescriptor::typed(
            "session/list",
            "List active sessions",
            "ListSessionsParams",
            "ListSessionsResult",
        ),
        RpcMethodDescriptor::typed(
            "session/read",
            "Get session state",
            "ReadSessionParams",
            "WireSessionInfo",
        ),
        RpcMethodDescriptor::typed(
            "session/history",
            "Get full session history",
            "ReadSessionHistoryParams",
            "WireSessionHistory",
        ),
        RpcMethodDescriptor::typed(
            "session/fork_at",
            "Fork an idle session at a transcript message index",
            "ForkSessionAtParams",
            "SessionForkResult",
        ),
        RpcMethodDescriptor::typed(
            "session/fork_replace",
            "Fork an idle session and apply a typed transcript replacement",
            "ForkSessionReplaceParams",
            "SessionForkResult",
        ),
        RpcMethodDescriptor::typed(
            "session/archive",
            "Remove session",
            "ArchiveSessionParams",
            "Value",
        ),
        RpcMethodDescriptor::typed(
            "turn/start",
            "Start a new turn on existing session",
            "StartTurnParams",
            "WireRunResult",
        )
        .with_request_lifecycle(RpcRequestLifecycleRule::LONG_RUNNING_PUBLISH_ON_SUCCESS),
        RpcMethodDescriptor::typed(
            "turn/interrupt",
            "Cancel in-flight turn",
            "InterruptParams",
            "Value",
        ),
        RpcMethodDescriptor::basic("config/get", "Read config"),
        RpcMethodDescriptor::basic("config/set", "Replace config"),
        RpcMethodDescriptor::basic("config/patch", "Merge-patch config"),
        RpcMethodDescriptor::basic("capabilities/get", "Get runtime capabilities"),
        RpcMethodDescriptor::result_only(
            "runtime/host_info",
            "Get read-only runtime host information",
            "RuntimeHostInfo",
        ),
        RpcMethodDescriptor::result_only(
            "runtime/capabilities",
            "Get runtime host capability flags",
            "RuntimeHostCapabilities",
        ),
        RpcMethodDescriptor::result_only(
            "runtime/health",
            "Get runtime host health",
            "RuntimeHostHealth",
        ),
        RpcMethodDescriptor::typed(
            "approval/request",
            "Create a durable approval request",
            "ApprovalRequestParams",
            "ApprovalRecord",
        ),
        RpcMethodDescriptor::typed(
            "approval/list",
            "List durable approval requests",
            "ApprovalListParams",
            "ApprovalListResult",
        ),
        RpcMethodDescriptor::typed(
            "approval/get",
            "Get one durable approval request",
            "ApprovalGetParams",
            "ApprovalRecord",
        ),
        RpcMethodDescriptor::typed(
            "approval/decide",
            "Record a durable approval decision",
            "ApprovalDecideParams",
            "ApprovalRecord",
        ),
        RpcMethodDescriptor::result_only(
            "models/catalog",
            "Get the effective model catalog (built-in plus config-backed entries)",
            "ModelsCatalogResponse",
        ),
        // Auth-profile management (Phase 4c — always available).
        RpcMethodDescriptor::typed(
            "auth/profile/list",
            "List configured auth profiles",
            "RealmIdParams",
            "WireAuthProfilesList",
        ),
        RpcMethodDescriptor::typed(
            "auth/profile/get",
            "Get one auth profile by id",
            "BindingIdParams",
            "WireAuthProfileDetail",
        ),
        RpcMethodDescriptor::typed(
            "auth/profile/create",
            "Create an auth profile",
            "CreateProfileParams",
            "WireAuthProfileCreated",
        ),
        RpcMethodDescriptor::typed(
            "auth/profile/delete",
            "Delete an auth profile",
            "BindingIdParams",
            "WireAuthProfileCleared",
        ),
        RpcMethodDescriptor::typed(
            "auth/login/start",
            "Begin an OAuth login; returns authorize URL, state, PKCE verifier",
            "LoginStartParams",
            "WireLoginStart",
        ),
        RpcMethodDescriptor::typed(
            "auth/login/complete",
            "Finish an OAuth login by exchanging an authorization code",
            "LoginCompleteParams",
            "WireLoginReady",
        ),
        RpcMethodDescriptor::typed(
            "auth/login/device_start",
            "Begin a device-code OAuth login; returns user-code + verification URL",
            "DeviceStartParams",
            "WireDeviceStart",
        ),
        RpcMethodDescriptor::typed(
            "auth/login/device_complete",
            "Single-poll completion for a device-code OAuth login; returns pending / slow_down / access_denied / expired / ready",
            "DeviceCompleteParams",
            "WireDeviceCompleteResult",
        ),
        RpcMethodDescriptor::typed(
            "auth/login/provision_api_key",
            "Console-OAuth → API key provisioning (Anthropic oauth_to_api_key): POST access_token to create_api_key endpoint, persist returned api_key",
            "ProvisionApiKeyParams",
            "WireProvisionApiKeyResult",
        ),
        RpcMethodDescriptor::typed(
            "auth/status/get",
            "Get auth status for a profile",
            "BindingIdParams",
            "WireAuthStatusDetail",
        ),
        RpcMethodDescriptor::typed(
            "auth/logout",
            "Revoke + remove persisted credentials",
            "BindingIdParams",
            "WireAuthProfileCleared",
        ),
        RpcMethodDescriptor::result_only(
            "realm/list",
            "List realms with binding summaries",
            "WireRealmList",
        ),
        RpcMethodDescriptor::typed(
            "realm/get",
            "Get one realm's full connection set",
            "RealmIdParams",
            "WireRealmConnectionSet",
        ),
    ];

    if options.runtime_available {
        methods.push(
            RpcMethodDescriptor::typed(
                "help/ask",
                "Ask Meerkat usage help with the embedded platform skill",
                "HelpRequest",
                "HelpResponse",
            )
            .with_request_lifecycle(RpcRequestLifecycleRule::LONG_RUNNING_PUBLISH_ON_SUCCESS),
        );
    }

    if options.blob_enabled {
        methods.extend([
            RpcMethodDescriptor::typed(
                "blob/get",
                "Fetch raw blob payload metadata and bytes by blob id",
                "BlobGetParams",
                "BlobPayload",
            ),
            RpcMethodDescriptor::typed(
                "artifact/list",
                "List stable artifact records",
                "ArtifactListParams",
                "ArtifactListResult",
            ),
            RpcMethodDescriptor::typed(
                "artifact/get",
                "Get one stable artifact record",
                "ArtifactIdParams",
                "ArtifactRecord",
            ),
            RpcMethodDescriptor::typed(
                "artifact/download",
                "Download blob-backed artifact payload bytes",
                "ArtifactDownloadParams",
                "ArtifactDownloadResult",
            ),
        ]);
    }

    if options.session_events_enabled {
        methods.extend([
            RpcMethodDescriptor::typed(
                "session/external_event",
                "Queue a runtime-backed external event",
                "SessionExternalEventEnvelope",
                "RuntimeAcceptResult",
            ),
            RpcMethodDescriptor::typed(
                "session/peer_response_terminal",
                "Admit a correlated terminal peer response through the typed runtime ingress",
                "SessionPeerResponseTerminalParams",
                "RuntimeAcceptResult",
            ),
            RpcMethodDescriptor::typed(
                "session/inject_context",
                "Stage runtime system context for application at the next LLM boundary",
                "InjectSystemContextParams",
                "InjectSystemContextResult",
            ),
            RpcMethodDescriptor::typed(
                "events/latest_cursor",
                "Read the latest replay cursor for an event source",
                "EventsLatestCursorParams",
                "EventsLatestCursorResult",
            ),
            RpcMethodDescriptor::typed(
                "events/list_since",
                "List replayable events after a cursor",
                "EventsListSinceParams",
                "EventsListSinceResult",
            ),
            RpcMethodDescriptor::typed(
                "events/snapshot",
                "Read a point-in-time snapshot anchor for an event source",
                "EventsSnapshotParams",
                "EventsSnapshotResult",
            ),
        ]);
    }

    if options.session_streams_enabled {
        methods.extend([
            RpcMethodDescriptor::typed(
                "session/stream_open",
                "Open a session event stream",
                "SessionStreamOpenParams",
                "SessionStreamOpenResult",
            ),
            RpcMethodDescriptor::typed(
                "session/stream_close",
                "Close a session event stream",
                "SessionStreamCloseParams",
                "SessionStreamCloseResult",
            ),
        ]);
    }

    if options.schedule_enabled {
        methods.extend([
            RpcMethodDescriptor::typed(
                "schedule/create",
                "Create a schedule",
                "CreateScheduleRequest",
                "Schedule",
            ),
            RpcMethodDescriptor::typed(
                "schedule/get",
                "Get one schedule",
                "ScheduleIdParams",
                "Schedule",
            ),
            RpcMethodDescriptor::typed(
                "schedule/list",
                "List schedules",
                "ListSchedulesParams",
                "ScheduleListResult",
            ),
            RpcMethodDescriptor::typed(
                "schedule/update",
                "Update a schedule",
                "UpdateScheduleParams",
                "Schedule",
            ),
            RpcMethodDescriptor::typed(
                "schedule/pause",
                "Pause a schedule",
                "ScheduleIdParams",
                "Schedule",
            ),
            RpcMethodDescriptor::typed(
                "schedule/resume",
                "Resume a schedule",
                "ScheduleIdParams",
                "Schedule",
            ),
            RpcMethodDescriptor::typed(
                "schedule/delete",
                "Delete a schedule",
                "ScheduleIdParams",
                "Schedule",
            ),
            RpcMethodDescriptor::typed(
                "schedule/occurrences",
                "List schedule occurrences",
                "ScheduleOccurrencesParams",
                "ScheduleOccurrencesResult",
            ),
            RpcMethodDescriptor::result_only(
                "schedule/tools",
                "List schedule transport tools",
                "ScheduleToolsResult",
            ),
            RpcMethodDescriptor::typed(
                "schedule/call",
                "Call a schedule transport tool",
                "ScheduleToolCallParams",
                "Value",
            ),
        ]);
    }

    if options.skills_enabled {
        methods.extend([RpcMethodDescriptor::basic(
            "skills/list",
            "List available skills",
        )]);
    }

    if options.runtime_available {
        methods.extend([
            RpcMethodDescriptor::typed(
                "realtime/open_info",
                "Get bootstrap metadata for opening a realtime channel",
                "RealtimeOpenRequest",
                "RealtimeOpenInfo",
            ),
            RpcMethodDescriptor::typed(
                "realtime/status",
                "Get product-layer realtime channel status for a target",
                "RealtimeStatusParams",
                "RealtimeStatusResult",
            ),
            RpcMethodDescriptor::typed(
                "realtime/capabilities",
                "Get product-layer realtime capabilities for a target",
                "RealtimeCapabilitiesParams",
                "RealtimeCapabilitiesResult",
            ),
            RpcMethodDescriptor::typed(
                "session/realtime_attachment_status",
                "Get a session's realtime attachment status",
                "RuntimeRealtimeAttachmentStatusParams",
                "RuntimeRealtimeAttachmentStatusResult",
            ),
        ]);
    }

    if options.mob_enabled {
        methods.extend([
            RpcMethodDescriptor::typed(
                "mob/create",
                "Create a mob from a definition",
                "MobCreateParams",
                "MobCreateResult",
            ),
            RpcMethodDescriptor::result_only("mob/list", "List active mobs", "MobListResult"),
            RpcMethodDescriptor::typed(
                "mob/status",
                "Get mob lifecycle status",
                "MobIdParams",
                "MobStatusResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/lifecycle",
                "Apply a mob lifecycle action",
                "MobLifecycleParams",
                "MobLifecycleResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/spawn",
                "Spawn a new mob member",
                "MobSpawnParams",
                "MobSpawnResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/spawn_many",
                "Spawn multiple new mob members",
                "MobSpawnManyParams",
                "MobSpawnManyResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/ensure_member",
                "Declarative: spawn if absent, otherwise return the existing roster entry",
                "MobEnsureMemberParams",
                "MobEnsureMemberResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/reconcile",
                "Declarative: drive the roster toward a desired set of specs",
                "MobReconcileParams",
                "MobReconcileResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/list_members_matching",
                "List roster members that match a filter",
                "MobListMembersMatchingParams",
                "MobListMembersMatchingResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/retire",
                "Retire a mob member",
                "MobMemberParams",
                "MobRetireResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/respawn",
                "Respawn a mob member with topology restore",
                "MobRespawnParams",
                "MobRespawnResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/wire",
                "Wire a local mob member to a local or external peer",
                "MobWireParams",
                "MobWireResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/unwire",
                "Unwire a local mob member from a local or external peer",
                "MobUnwireParams",
                "MobUnwireResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/members",
                "List members in a mob roster",
                "MobIdParams",
                "MobMembersResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/events",
                "Read mob event history",
                "MobEventsParams",
                "MobEventsResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/member_send",
                "Deliver ordinary content to a specific mob member via the host control plane",
                "MobMemberSendParams",
                "MobMemberSendResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/ingress_interaction",
                "Ensure an ingress member, then deliver user input with a replay cursor receipt",
                "MobIngressInteractionParams",
                "MobIngressInteractionResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/append_system_context",
                "Append system context for a mob member",
                "MobAppendSystemContextParams",
                "MobAppendSystemContextResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/flows",
                "List flows defined for a mob",
                "MobIdParams",
                "MobFlowsResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/flow_run",
                "Start a mob flow run",
                "MobFlowRunParams",
                "MobFlowRunResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/flow_status",
                "Get status for a mob flow run",
                "MobFlowStatusParams",
                "MobFlowStatusResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/flow_cancel",
                "Cancel a mob flow run",
                "MobFlowCancelParams",
                "MobFlowCancelResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/spawn_helper",
                "Spawn a helper member and wait for completion",
                "MobSpawnHelperParams",
                "MobHelperResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/fork_helper",
                "Fork a helper member and wait for completion",
                "MobForkHelperParams",
                "MobHelperResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/force_cancel",
                "Force-cancel a mob member",
                "MobMemberParams",
                "MobForceCancelResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/turn_start",
                "Start a turn on a mob member by identity",
                "MobTurnStartParams",
                "WireRunResult",
            )
            .with_request_lifecycle(RpcRequestLifecycleRule::LONG_RUNNING_PUBLISH_ON_SUCCESS),
            RpcMethodDescriptor::typed(
                "mob/member_status",
                "Get live status for a mob member",
                "MobMemberParams",
                "MobMemberStatusResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/snapshot",
                "Point-in-time aggregate of mob status plus member list",
                "MobIdParams",
                "MobSnapshotResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/destroy",
                "Destroy a mob and surface the structured MobDestroyReport",
                "MobIdParams",
                "MobDestroyResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/rotate_supervisor",
                "Rotate the supervisor bridge for all members of a mob",
                "MobIdParams",
                "MobRotateSupervisorResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/submit_work",
                "Submit a unit of work to a mob member through the work lane",
                "MobSubmitWorkParams",
                "MobSubmitWorkResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/cancel_work",
                "Cancel a previously submitted unit of work",
                "MobCancelWorkParams",
                "MobCancelWorkResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/cancel_all_work",
                "Cancel all in-flight work for a mob member",
                "MobCancelAllWorkParams",
                "MobCancelAllWorkResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/wait_kickoff",
                "Wait for kickoff completion for a member",
                "MobWaitParams",
                "MobWaitMembersResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/wait_ready",
                "Wait for mob startup readiness (members bound but kickoff not required)",
                "MobWaitParams",
                "MobWaitMembersResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/profile/create",
                "Create a realm-scoped mob profile",
                "MobProfileCreateParams",
                "MobProfileLookupResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/profile/get",
                "Read a realm-scoped mob profile",
                "MobProfileNameParams",
                "MobProfileLookupResult",
            ),
            RpcMethodDescriptor::result_only(
                "mob/profile/list",
                "List realm-scoped mob profiles",
                "MobProfileListResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/profile/update",
                "Update a realm-scoped mob profile",
                "MobProfileUpdateParams",
                "MobProfileLookupResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/profile/delete",
                "Delete a realm-scoped mob profile",
                "MobProfileDeleteParams",
                "MobProfileDeleteResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/stream_open",
                "Open a mob event stream",
                "MobStreamOpenParams",
                "MobStreamOpenResult",
            ),
            RpcMethodDescriptor::typed(
                "mob/stream_close",
                "Close a mob event stream",
                "MobStreamCloseParams",
                "MobStreamCloseResult",
            ),
        ]);
    }

    if options.mcp_enabled {
        methods.extend([
            RpcMethodDescriptor::typed(
                "mcp/add",
                "Stage live MCP server add for a session",
                "McpAddParams",
                "McpLiveOpResponse",
            ),
            RpcMethodDescriptor::typed(
                "mcp/remove",
                "Stage live MCP server remove for a session",
                "McpRemoveParams",
                "McpLiveOpResponse",
            ),
            RpcMethodDescriptor::typed(
                "mcp/reload",
                "Optional skeleton for live MCP reload",
                "McpReloadParams",
                "McpLiveOpResponse",
            ),
        ]);
    }

    if options.comms_enabled {
        methods.extend([
            RpcMethodDescriptor::typed(
                "comms/send",
                "Send a comms command from a session",
                "CommsSendParams",
                "CommsSendResult",
            ),
            RpcMethodDescriptor::typed(
                "comms/peers",
                "List peers visible to a session",
                "CommsPeersParams",
                "CommsPeersResult",
            ),
        ]);
    }

    methods
}

pub fn rpc_method_names(options: RpcMethodCatalogOptions) -> Vec<String> {
    rpc_method_catalog(options)
        .into_iter()
        .map(|method| method.name.to_string())
        .collect()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RpcNotificationDescriptor {
    pub name: &'static str,
    pub description: &'static str,
}

impl RpcNotificationDescriptor {
    const fn basic(name: &'static str, description: &'static str) -> Self {
        Self { name, description }
    }
}

pub fn rpc_notification_catalog(
    options: RpcMethodCatalogOptions,
) -> Vec<RpcNotificationDescriptor> {
    let mut notifications = vec![
        RpcNotificationDescriptor::basic(
            "initialized",
            "Client notification acknowledged silently (no response)",
        ),
        RpcNotificationDescriptor::basic(
            "cancel",
            "Cancel an in-flight JSON-RPC request by request id",
        ),
        RpcNotificationDescriptor::basic("session/event", "AgentEvent payload during turns"),
    ];

    if options.session_streams_enabled {
        notifications.extend([
            RpcNotificationDescriptor::basic(
                "session/stream_event",
                "Standalone session stream event notification",
            ),
            RpcNotificationDescriptor::basic(
                "session/stream_end",
                "Standalone session stream terminal notification",
            ),
        ]);
    }

    if options.mob_enabled {
        notifications.extend([
            RpcNotificationDescriptor::basic("mob/stream_event", "Mob stream event notification"),
            RpcNotificationDescriptor::basic("mob/stream_end", "Mob stream terminal notification"),
        ]);
    }

    notifications
}

pub fn rpc_notification_names(options: RpcMethodCatalogOptions) -> Vec<String> {
    rpc_notification_catalog(options)
        .into_iter()
        .map(|notification| notification.name.to_string())
        .collect()
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::{RequestLifecycle, mcp_tool_request_lifecycle, rpc_request_lifecycle};

    #[test]
    fn documented_surface_keeps_live_runtime_and_mob_methods() {
        let methods = rpc_method_names(RpcMethodCatalogOptions::documented_surface());
        assert!(methods.iter().any(|m| m == "help/ask"));
        assert!(methods.iter().any(|m| m == "session/inject_context"));
        assert!(methods.iter().any(|m| m == "runtime/host_info"));
        assert!(methods.iter().any(|m| m == "runtime/capabilities"));
        assert!(methods.iter().any(|m| m == "runtime/health"));
        assert!(methods.iter().any(|m| m == "events/latest_cursor"));
        assert!(methods.iter().any(|m| m == "events/list_since"));
        assert!(methods.iter().any(|m| m == "events/snapshot"));
        assert!(methods.iter().any(|m| m == "approval/request"));
        assert!(methods.iter().any(|m| m == "approval/list"));
        assert!(methods.iter().any(|m| m == "approval/get"));
        assert!(methods.iter().any(|m| m == "approval/decide"));
        assert!(
            methods
                .iter()
                .any(|m| m == "session/peer_response_terminal")
        );
        assert!(methods.iter().any(|m| m == "mob/events"));
        assert!(methods.iter().any(|m| m == "mob/member_send"));
        assert!(methods.iter().any(|m| m == "mob/wait_kickoff"));
        assert!(methods.iter().any(|m| m == "mob/profile/create"));
        assert!(methods.iter().any(|m| m == "schedule/list"));
        assert!(methods.iter().any(|m| m == "schedule/occurrences"));
        assert!(methods.iter().any(|m| m == "schedule/call"));
    }

    #[test]
    fn documented_surface_keeps_live_stream_notifications() {
        let notifications = rpc_notification_names(RpcMethodCatalogOptions::documented_surface());
        assert!(notifications.iter().any(|n| n == "cancel"));
        assert!(notifications.iter().any(|n| n == "session/stream_event"));
        assert!(notifications.iter().any(|n| n == "session/stream_end"));
        assert!(notifications.iter().any(|n| n == "mob/stream_event"));
        assert!(notifications.iter().any(|n| n == "mob/stream_end"));
    }

    #[test]
    fn rpc_catalog_owns_request_lifecycle_rules() {
        let methods = rpc_method_catalog(RpcMethodCatalogOptions::documented_surface());
        let descriptor = |name: &str| {
            methods
                .iter()
                .find(|method| method.name == name)
                .unwrap_or_else(|| panic!("missing descriptor for {name}"))
        };

        assert_eq!(
            descriptor("help/ask").request_lifecycle.resolve(None),
            RequestLifecycle::LongRunningPublishOnSuccess
        );
        assert_eq!(
            descriptor("turn/start").request_lifecycle.resolve(None),
            RequestLifecycle::LongRunningPublishOnSuccess
        );
        assert_eq!(
            descriptor("mob/turn_start").request_lifecycle.resolve(None),
            RequestLifecycle::LongRunningPublishOnSuccess
        );
        assert_eq!(
            descriptor("session/create")
                .request_lifecycle
                .resolve(Some(r#"{"initial_turn":"deferred"}"#)),
            RequestLifecycle::InlineObservation
        );
        assert_eq!(
            rpc_request_lifecycle(
                "session/create",
                Some(r#"{"initial_turn":"run_immediately"}"#),
            ),
            RequestLifecycle::LongRunningPublishOnSuccess
        );
        assert_eq!(
            rpc_request_lifecycle("session/list", None),
            RequestLifecycle::InlineObservation
        );
    }

    #[test]
    fn mcp_tool_catalog_owns_request_lifecycle_rules() {
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_help"),
            RequestLifecycle::LongRunningPublishOnSuccess
        );
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_run"),
            RequestLifecycle::LongRunningPublishOnSuccess
        );
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_resume"),
            RequestLifecycle::LongRunningPublishOnSuccess
        );
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_sessions"),
            RequestLifecycle::LongRunningObservation
        );
    }

    #[test]
    fn documented_surface_excludes_retired_auth_probe() {
        let methods = rpc_method_names(RpcMethodCatalogOptions::documented_surface());
        assert!(
            !methods.iter().any(|m| m == "auth/profile/test"),
            "retired auth/profile/test must not be advertised by public catalogs"
        );
    }

    #[test]
    fn documented_surface_excludes_runtime_session_control_nouns_and_keeps_canonical_ingress() {
        let methods = rpc_method_names(RpcMethodCatalogOptions::documented_surface());
        for supported in [
            "session/external_event",
            "session/peer_response_terminal",
            "session/inject_context",
            "session/realtime_attachment_status",
        ] {
            assert!(
                methods.iter().any(|m| m == supported),
                "canonical runtime-backed session control noun must remain advertised: {supported}"
            );
        }
        for retired in [
            "runtime/session_status",
            "runtime/session_submit",
            "runtime/session_retire",
            "runtime/session_reset",
            "runtime/session_submission",
            "runtime/session_submissions",
        ] {
            assert!(
                !methods.iter().any(|m| m == retired),
                "runtime/session control noun must not be advertised by public catalogs: {retired}"
            );
        }
    }

    #[test]
    fn basic_turn_and_session_methods_advertise_required_params() {
        let methods = rpc_method_catalog(RpcMethodCatalogOptions::documented_surface());
        for name in [
            "session/create",
            "session/read",
            "session/history",
            "session/archive",
            "turn/start",
            "turn/interrupt",
        ] {
            let descriptors: Vec<_> = methods
                .iter()
                .filter(|method| method.name == name)
                .collect();
            assert_eq!(
                descriptors.len(),
                1,
                "missing or duplicated descriptor for {name}"
            );
            for descriptor in descriptors {
                assert!(
                    descriptor.params_type.is_some(),
                    "{name} must advertise its required params type"
                );
            }
        }
    }

    #[test]
    fn auth_response_methods_advertise_concrete_result_types() {
        let methods = rpc_method_catalog(RpcMethodCatalogOptions::documented_surface());
        for (name, expected_result_type) in [
            ("auth/profile/list", "WireAuthProfilesList"),
            ("auth/profile/get", "WireAuthProfileDetail"),
            ("auth/profile/create", "WireAuthProfileCreated"),
            ("auth/profile/delete", "WireAuthProfileCleared"),
            ("auth/login/start", "WireLoginStart"),
            ("auth/login/complete", "WireLoginReady"),
            ("auth/login/device_start", "WireDeviceStart"),
            ("auth/status/get", "WireAuthStatusDetail"),
            ("auth/logout", "WireAuthProfileCleared"),
            ("realm/list", "WireRealmList"),
            ("realm/get", "WireRealmConnectionSet"),
        ] {
            let descriptor = methods
                .iter()
                .find(|method| method.name == name)
                .unwrap_or_else(|| panic!("missing descriptor for {name}"));
            assert_eq!(
                descriptor.result_type,
                Some(expected_result_type),
                "{name} must advertise its concrete result type"
            );
        }
    }

    #[test]
    fn auth_methods_advertise_concrete_request_and_response_contracts() {
        let methods = rpc_method_catalog(RpcMethodCatalogOptions::documented_surface());
        for (name, expected_params_type, expected_result_type) in [
            (
                "auth/profile/list",
                Some("RealmIdParams"),
                Some("WireAuthProfilesList"),
            ),
            (
                "auth/profile/get",
                Some("BindingIdParams"),
                Some("WireAuthProfileDetail"),
            ),
            (
                "auth/profile/create",
                Some("CreateProfileParams"),
                Some("WireAuthProfileCreated"),
            ),
            (
                "auth/profile/delete",
                Some("BindingIdParams"),
                Some("WireAuthProfileCleared"),
            ),
            (
                "auth/login/start",
                Some("LoginStartParams"),
                Some("WireLoginStart"),
            ),
            (
                "auth/login/complete",
                Some("LoginCompleteParams"),
                Some("WireLoginReady"),
            ),
            (
                "auth/login/device_start",
                Some("DeviceStartParams"),
                Some("WireDeviceStart"),
            ),
            (
                "auth/login/device_complete",
                Some("DeviceCompleteParams"),
                Some("WireDeviceCompleteResult"),
            ),
            (
                "auth/login/provision_api_key",
                Some("ProvisionApiKeyParams"),
                Some("WireProvisionApiKeyResult"),
            ),
            (
                "auth/status/get",
                Some("BindingIdParams"),
                Some("WireAuthStatusDetail"),
            ),
            (
                "auth/logout",
                Some("BindingIdParams"),
                Some("WireAuthProfileCleared"),
            ),
            ("realm/list", None, Some("WireRealmList")),
            (
                "realm/get",
                Some("RealmIdParams"),
                Some("WireRealmConnectionSet"),
            ),
        ] {
            let descriptor = methods
                .iter()
                .find(|method| method.name == name)
                .unwrap_or_else(|| panic!("missing descriptor for {name}"));
            assert_eq!(
                descriptor.params_type, expected_params_type,
                "{name} must advertise its concrete params type"
            );
            assert_eq!(
                descriptor.result_type, expected_result_type,
                "{name} must advertise its concrete result type"
            );
        }
    }

    #[test]
    fn comms_and_session_stream_methods_advertise_concrete_contracts() {
        let methods = rpc_method_catalog(RpcMethodCatalogOptions::documented_surface());
        for (name, expected_params_type, expected_result_type) in [
            (
                "session/stream_open",
                "SessionStreamOpenParams",
                "SessionStreamOpenResult",
            ),
            (
                "session/stream_close",
                "SessionStreamCloseParams",
                "SessionStreamCloseResult",
            ),
            ("comms/send", "CommsSendParams", "CommsSendResult"),
            ("comms/peers", "CommsPeersParams", "CommsPeersResult"),
        ] {
            let descriptor = methods
                .iter()
                .find(|method| method.name == name)
                .unwrap_or_else(|| panic!("missing descriptor for {name}"));
            assert_eq!(
                descriptor.params_type,
                Some(expected_params_type),
                "{name} must advertise its concrete params type"
            );
            assert_eq!(
                descriptor.result_type,
                Some(expected_result_type),
                "{name} must advertise its concrete result type"
            );
        }
    }

    #[test]
    fn mob_methods_advertise_only_real_exported_contracts() {
        let methods = rpc_method_catalog(RpcMethodCatalogOptions::documented_surface());
        for (name, expected_params_type, expected_result_type) in [
            (
                "mob/create",
                Some("MobCreateParams"),
                Some("MobCreateResult"),
            ),
            ("mob/list", None, Some("MobListResult")),
            ("mob/status", Some("MobIdParams"), Some("MobStatusResult")),
            (
                "mob/lifecycle",
                Some("MobLifecycleParams"),
                Some("MobLifecycleResult"),
            ),
            ("mob/spawn", Some("MobSpawnParams"), Some("MobSpawnResult")),
            (
                "mob/spawn_many",
                Some("MobSpawnManyParams"),
                Some("MobSpawnManyResult"),
            ),
            (
                "mob/ensure_member",
                Some("MobEnsureMemberParams"),
                Some("MobEnsureMemberResult"),
            ),
            (
                "mob/reconcile",
                Some("MobReconcileParams"),
                Some("MobReconcileResult"),
            ),
            (
                "mob/list_members_matching",
                Some("MobListMembersMatchingParams"),
                Some("MobListMembersMatchingResult"),
            ),
            (
                "mob/retire",
                Some("MobMemberParams"),
                Some("MobRetireResult"),
            ),
            (
                "mob/respawn",
                Some("MobRespawnParams"),
                Some("MobRespawnResult"),
            ),
            ("mob/wire", Some("MobWireParams"), Some("MobWireResult")),
            (
                "mob/unwire",
                Some("MobUnwireParams"),
                Some("MobUnwireResult"),
            ),
            ("mob/members", Some("MobIdParams"), Some("MobMembersResult")),
            (
                "mob/events",
                Some("MobEventsParams"),
                Some("MobEventsResult"),
            ),
            (
                "mob/member_send",
                Some("MobMemberSendParams"),
                Some("MobMemberSendResult"),
            ),
            (
                "mob/ingress_interaction",
                Some("MobIngressInteractionParams"),
                Some("MobIngressInteractionResult"),
            ),
            (
                "mob/append_system_context",
                Some("MobAppendSystemContextParams"),
                Some("MobAppendSystemContextResult"),
            ),
            ("mob/flows", Some("MobIdParams"), Some("MobFlowsResult")),
            (
                "mob/flow_run",
                Some("MobFlowRunParams"),
                Some("MobFlowRunResult"),
            ),
            (
                "mob/flow_status",
                Some("MobFlowStatusParams"),
                Some("MobFlowStatusResult"),
            ),
            (
                "mob/flow_cancel",
                Some("MobFlowCancelParams"),
                Some("MobFlowCancelResult"),
            ),
            (
                "mob/spawn_helper",
                Some("MobSpawnHelperParams"),
                Some("MobHelperResult"),
            ),
            (
                "mob/fork_helper",
                Some("MobForkHelperParams"),
                Some("MobHelperResult"),
            ),
            (
                "mob/force_cancel",
                Some("MobMemberParams"),
                Some("MobForceCancelResult"),
            ),
            (
                "mob/turn_start",
                Some("MobTurnStartParams"),
                Some("WireRunResult"),
            ),
            (
                "mob/member_status",
                Some("MobMemberParams"),
                Some("MobMemberStatusResult"),
            ),
            (
                "mob/snapshot",
                Some("MobIdParams"),
                Some("MobSnapshotResult"),
            ),
            ("mob/destroy", Some("MobIdParams"), Some("MobDestroyResult")),
            (
                "mob/rotate_supervisor",
                Some("MobIdParams"),
                Some("MobRotateSupervisorResult"),
            ),
            (
                "mob/wait_kickoff",
                Some("MobWaitParams"),
                Some("MobWaitMembersResult"),
            ),
            (
                "mob/wait_ready",
                Some("MobWaitParams"),
                Some("MobWaitMembersResult"),
            ),
            (
                "mob/submit_work",
                Some("MobSubmitWorkParams"),
                Some("MobSubmitWorkResult"),
            ),
            (
                "mob/cancel_work",
                Some("MobCancelWorkParams"),
                Some("MobCancelWorkResult"),
            ),
            (
                "mob/cancel_all_work",
                Some("MobCancelAllWorkParams"),
                Some("MobCancelAllWorkResult"),
            ),
            (
                "mob/profile/create",
                Some("MobProfileCreateParams"),
                Some("MobProfileLookupResult"),
            ),
            (
                "mob/profile/get",
                Some("MobProfileNameParams"),
                Some("MobProfileLookupResult"),
            ),
            ("mob/profile/list", None, Some("MobProfileListResult")),
            (
                "mob/profile/update",
                Some("MobProfileUpdateParams"),
                Some("MobProfileLookupResult"),
            ),
            (
                "mob/profile/delete",
                Some("MobProfileDeleteParams"),
                Some("MobProfileDeleteResult"),
            ),
            (
                "mob/stream_open",
                Some("MobStreamOpenParams"),
                Some("MobStreamOpenResult"),
            ),
            (
                "mob/stream_close",
                Some("MobStreamCloseParams"),
                Some("MobStreamCloseResult"),
            ),
        ] {
            let descriptor = methods
                .iter()
                .find(|method| method.name == name)
                .unwrap_or_else(|| panic!("missing descriptor for {name}"));
            assert_eq!(
                descriptor.params_type, expected_params_type,
                "{name} must advertise its concrete params type"
            );
            assert_eq!(
                descriptor.result_type, expected_result_type,
                "{name} must advertise its concrete result type"
            );
        }
    }
}
