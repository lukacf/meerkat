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
            "models/catalog",
            "Get the effective model catalog (built-in plus config-backed entries)",
            "ModelsCatalogResponse",
        ),
    ];

    if options.blob_enabled {
        methods.push(RpcMethodDescriptor::typed(
            "blob/get",
            "Fetch raw blob payload metadata and bytes by blob id",
            "BlobGetParams",
            "BlobPayload",
        ));
    }

    if options.session_events_enabled {
        methods.extend([
            RpcMethodDescriptor::basic(
                "session/external_event",
                "Queue a runtime-backed external event",
            ),
            RpcMethodDescriptor::basic(
                "session/inject_context",
                "Stage runtime system context for application at the next LLM boundary",
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
            RpcMethodDescriptor::typed(
                "runtime/state",
                "Get a session runtime's current state",
                "RuntimeStateParams",
                "RuntimeStateResult",
            ),
            RpcMethodDescriptor::typed(
                "runtime/accept",
                "Accept a runtime input for a session",
                "RuntimeAcceptParams",
                "RuntimeAcceptResult",
            ),
            RpcMethodDescriptor::typed(
                "runtime/retire",
                "Retire a session runtime",
                "RuntimeRetireParams",
                "RuntimeRetireResult",
            ),
            RpcMethodDescriptor::typed(
                "runtime/reset",
                "Reset a session runtime",
                "RuntimeResetParams",
                "RuntimeResetResult",
            ),
            RpcMethodDescriptor::typed(
                "input/state",
                "Get the state of a specific runtime input",
                "InputStateParams",
                "InputStateResult",
            ),
            RpcMethodDescriptor::typed(
                "input/list",
                "List active runtime inputs for a session",
                "InputListParams",
                "InputListResult",
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
                "mob/wait_kickoff",
                "Wait for kickoff completion for a member",
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
}
