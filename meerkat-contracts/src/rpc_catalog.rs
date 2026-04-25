use serde::Serialize;

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

    pub const fn mini_surface() -> Self {
        Self {
            runtime_available: false,
            mob_enabled: false,
            mcp_enabled: false,
            comms_enabled: false,
            blob_enabled: false,
            session_events_enabled: false,
            session_streams_enabled: false,
            schedule_enabled: false,
            skills_enabled: false,
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
}

impl RpcMethodDescriptor {
    const fn basic(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            description,
            params_type: None,
            result_type: None,
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
        }
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
        }
    }
}

pub fn rpc_method_catalog(options: RpcMethodCatalogOptions) -> Vec<RpcMethodDescriptor> {
    let mut methods = vec![
        RpcMethodDescriptor::basic("initialize", "Handshake, returns server capabilities"),
        RpcMethodDescriptor::basic("session/create", "Create session + run first turn"),
        RpcMethodDescriptor::basic("session/list", "List active sessions"),
        RpcMethodDescriptor::basic("session/read", "Get session state"),
        RpcMethodDescriptor::basic("session/history", "Get full session history"),
        RpcMethodDescriptor::basic("session/archive", "Remove session"),
        RpcMethodDescriptor::basic("turn/start", "Start a new turn on existing session"),
        RpcMethodDescriptor::basic("turn/interrupt", "Cancel in-flight turn"),
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
        RpcMethodDescriptor::basic("auth/profile/list", "List configured auth profiles"),
        RpcMethodDescriptor::basic("auth/profile/get", "Get one auth profile by id"),
        RpcMethodDescriptor::basic("auth/profile/create", "Create an auth profile"),
        RpcMethodDescriptor::basic("auth/profile/delete", "Delete an auth profile"),
        RpcMethodDescriptor::basic(
            "auth/login/start",
            "Begin an OAuth login; returns authorize URL, state, PKCE verifier",
        ),
        RpcMethodDescriptor::basic(
            "auth/login/complete",
            "Finish an OAuth login by exchanging an authorization code",
        ),
        RpcMethodDescriptor::basic(
            "auth/login/device_start",
            "Begin a device-code OAuth login; returns user-code + verification URL",
        ),
        RpcMethodDescriptor::basic(
            "auth/login/device_complete",
            "Single-poll completion for a device-code OAuth login; returns pending / slow_down / access_denied / expired / ready",
        ),
        RpcMethodDescriptor::basic(
            "auth/login/provision_api_key",
            "Console-OAuth → API key provisioning (Anthropic oauth_to_api_key): POST access_token to create_api_key endpoint, persist returned api_key",
        ),
        RpcMethodDescriptor::basic("auth/status/get", "Get auth status for a profile"),
        RpcMethodDescriptor::basic("auth/logout", "Revoke + remove persisted credentials"),
        RpcMethodDescriptor::basic("realm/list", "List realms with binding summaries"),
        RpcMethodDescriptor::basic("realm/get", "Get one realm's full connection set"),
    ];

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
            RpcMethodDescriptor::basic(
                "session/inject_context",
                "Stage runtime system context for application at the next LLM boundary",
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
            RpcMethodDescriptor::basic("session/stream_open", "Open a session event stream"),
            RpcMethodDescriptor::basic("session/stream_close", "Close a session event stream"),
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
        methods.extend([
            RpcMethodDescriptor::basic("skills/list", "List available skills"),
            RpcMethodDescriptor::basic("skills/inspect", "Inspect one skill"),
        ]);
    }

    if options.runtime_available {
        methods.extend([
            RpcMethodDescriptor::basic("session/status", "Get a session's current runtime state"),
            RpcMethodDescriptor::basic("session/submit", "Accept a runtime input for a session"),
            RpcMethodDescriptor::basic("session/retire", "Retire a session runtime"),
            RpcMethodDescriptor::basic("session/reset", "Reset a session runtime"),
            RpcMethodDescriptor::basic("session/submission", "Get a runtime input state"),
            RpcMethodDescriptor::basic("session/submissions", "List active runtime inputs"),
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
            RpcMethodDescriptor::basic("mob/list", "List active mobs"),
            RpcMethodDescriptor::basic("mob/status", "Get mob lifecycle status"),
            RpcMethodDescriptor::basic("mob/lifecycle", "Apply a mob lifecycle action"),
            RpcMethodDescriptor::basic("mob/spawn", "Spawn a new mob member"),
            RpcMethodDescriptor::basic("mob/spawn_many", "Spawn multiple new mob members"),
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
            RpcMethodDescriptor::basic("mob/retire", "Retire a mob member"),
            RpcMethodDescriptor::basic("mob/respawn", "Respawn a mob member with topology restore"),
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
            RpcMethodDescriptor::basic("mob/members", "List members in a mob roster"),
            RpcMethodDescriptor::basic("mob/events", "Read mob event history"),
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
            RpcMethodDescriptor::basic(
                "mob/append_system_context",
                "Append system context for a mob member",
            ),
            RpcMethodDescriptor::basic("mob/flows", "List flows defined for a mob"),
            RpcMethodDescriptor::basic("mob/flow_run", "Start a mob flow run"),
            RpcMethodDescriptor::basic("mob/flow_status", "Get status for a mob flow run"),
            RpcMethodDescriptor::basic("mob/flow_cancel", "Cancel a mob flow run"),
            RpcMethodDescriptor::basic(
                "mob/spawn_helper",
                "Spawn a helper member and wait for completion",
            ),
            RpcMethodDescriptor::basic(
                "mob/fork_helper",
                "Fork a helper member and wait for completion",
            ),
            RpcMethodDescriptor::basic("mob/force_cancel", "Force-cancel a mob member"),
            RpcMethodDescriptor::basic(
                "mob/turn_start",
                "Start a turn on a mob member by identity",
            ),
            RpcMethodDescriptor::basic("mob/member_status", "Get live status for a mob member"),
            RpcMethodDescriptor::basic(
                "mob/snapshot",
                "Point-in-time aggregate of mob status plus member list",
            ),
            RpcMethodDescriptor::basic(
                "mob/destroy",
                "Destroy a mob and surface the structured MobDestroyReport",
            ),
            RpcMethodDescriptor::basic(
                "mob/rotate_supervisor",
                "Rotate the supervisor bridge for all members of a mob",
            ),
            RpcMethodDescriptor::basic(
                "mob/submit_work",
                "Submit a unit of work to a mob member through the work lane",
            ),
            RpcMethodDescriptor::basic(
                "mob/cancel_work",
                "Cancel a previously submitted unit of work",
            ),
            RpcMethodDescriptor::basic(
                "mob/cancel_all_work",
                "Cancel all in-flight work for a mob member",
            ),
            RpcMethodDescriptor::basic(
                "mob/wait_kickoff",
                "Wait for kickoff completion for a member",
            ),
            RpcMethodDescriptor::basic(
                "mob/wait_ready",
                "Wait for mob startup readiness (members bound but kickoff not required)",
            ),
            RpcMethodDescriptor::basic("mob/profile/create", "Create a realm-scoped mob profile"),
            RpcMethodDescriptor::basic("mob/profile/get", "Read a realm-scoped mob profile"),
            RpcMethodDescriptor::basic("mob/profile/list", "List realm-scoped mob profiles"),
            RpcMethodDescriptor::basic("mob/profile/update", "Update a realm-scoped mob profile"),
            RpcMethodDescriptor::basic("mob/profile/delete", "Delete a realm-scoped mob profile"),
            RpcMethodDescriptor::basic("mob/stream_open", "Open a mob event stream"),
            RpcMethodDescriptor::basic("mob/stream_close", "Close a mob event stream"),
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
            RpcMethodDescriptor::basic("comms/send", "Send a comms command from a session"),
            RpcMethodDescriptor::basic("comms/peers", "List peers visible to a session"),
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
mod tests {
    use super::*;

    #[test]
    fn documented_surface_keeps_live_runtime_and_mob_methods() {
        let methods = rpc_method_names(RpcMethodCatalogOptions::documented_surface());
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
    fn documented_surface_excludes_retired_auth_probe() {
        let methods = rpc_method_names(RpcMethodCatalogOptions::documented_surface());
        assert!(
            !methods.iter().any(|m| m == "auth/profile/test"),
            "retired auth/profile/test must not be advertised by public catalogs"
        );
    }
}
