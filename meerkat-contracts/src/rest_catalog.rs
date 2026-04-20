use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RestOperationDescriptor {
    pub method: &'static str,
    pub summary: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'static str>,
}

impl RestOperationDescriptor {
    const fn new(method: &'static str, summary: &'static str) -> Self {
        Self {
            method,
            summary,
            description: None,
        }
    }

    const fn with_description(
        method: &'static str,
        summary: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            method,
            summary,
            description: Some(description),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RestPathDescriptor {
    pub path: &'static str,
    pub operations: Vec<RestOperationDescriptor>,
}

impl RestPathDescriptor {
    fn new(path: &'static str, operations: Vec<RestOperationDescriptor>) -> Self {
        Self { path, operations }
    }
}

pub fn rest_path_catalog() -> Vec<RestPathDescriptor> {
    vec![
        RestPathDescriptor::new(
            "/sessions",
            vec![
                RestOperationDescriptor::new("get", "List sessions"),
                RestOperationDescriptor::new("post", "Create and run a new session"),
            ],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}",
            vec![
                RestOperationDescriptor::new("get", "Get session details"),
                RestOperationDescriptor::new("delete", "Archive a session"),
            ],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/history",
            vec![RestOperationDescriptor::new(
                "get",
                "Get full session history",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/interrupt",
            vec![RestOperationDescriptor::new(
                "post",
                "Interrupt a running session",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/system_context",
            vec![RestOperationDescriptor::new(
                "post",
                "Append system context to a session",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/messages",
            vec![RestOperationDescriptor::new(
                "post",
                "Continue session with new message",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/external-events",
            vec![RestOperationDescriptor::new(
                "post",
                "Queue a runtime-backed external event",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/peer-response-terminal",
            vec![RestOperationDescriptor::new(
                "post",
                "Admit a correlated terminal peer response through the typed runtime ingress",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/events",
            vec![RestOperationDescriptor::new("get", "SSE event stream")],
        ),
        RestPathDescriptor::new(
            "/schedule/tools",
            vec![RestOperationDescriptor::new("get", "List schedule tools")],
        ),
        RestPathDescriptor::new(
            "/schedule/call",
            vec![RestOperationDescriptor::new(
                "post",
                "Invoke a schedule tool",
            )],
        ),
        RestPathDescriptor::new(
            "/schedules",
            vec![
                RestOperationDescriptor::new("get", "List schedules"),
                RestOperationDescriptor::new("post", "Create schedule"),
            ],
        ),
        RestPathDescriptor::new(
            "/schedules/{id}",
            vec![
                RestOperationDescriptor::new("get", "Get schedule"),
                RestOperationDescriptor::new("patch", "Update schedule"),
                RestOperationDescriptor::new("delete", "Delete schedule"),
            ],
        ),
        RestPathDescriptor::new(
            "/schedules/{id}/pause",
            vec![RestOperationDescriptor::new("post", "Pause schedule")],
        ),
        RestPathDescriptor::new(
            "/schedules/{id}/resume",
            vec![RestOperationDescriptor::new("post", "Resume schedule")],
        ),
        RestPathDescriptor::new(
            "/schedules/{id}/occurrences",
            vec![RestOperationDescriptor::new(
                "get",
                "List schedule occurrences",
            )],
        ),
        RestPathDescriptor::new(
            "/comms/send",
            vec![RestOperationDescriptor::new("post", "Send a comms message")],
        ),
        RestPathDescriptor::new(
            "/comms/peers",
            vec![RestOperationDescriptor::new(
                "get",
                "List resolved comms peers",
            )],
        ),
        RestPathDescriptor::new(
            "/config",
            vec![
                RestOperationDescriptor::new("get", "Read config"),
                RestOperationDescriptor::new("put", "Replace config"),
                RestOperationDescriptor::new("patch", "Merge-patch config"),
            ],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/mcp/add",
            vec![RestOperationDescriptor::with_description(
                "post",
                "Stage live MCP server addition",
                "Requires mcp_live capability. Check GET /capabilities.",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/mcp/remove",
            vec![RestOperationDescriptor::with_description(
                "post",
                "Stage live MCP server removal",
                "Requires mcp_live capability. Check GET /capabilities.",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/mcp/reload",
            vec![RestOperationDescriptor::with_description(
                "post",
                "Stage live MCP server reload",
                "Requires mcp_live capability. Check GET /capabilities.",
            )],
        ),
        RestPathDescriptor::new(
            "/skills",
            vec![RestOperationDescriptor::new("get", "List available skills")],
        ),
        RestPathDescriptor::new(
            "/skills/{id}",
            vec![RestOperationDescriptor::new("get", "Inspect a skill")],
        ),
        RestPathDescriptor::new(
            "/capabilities",
            vec![RestOperationDescriptor::new(
                "get",
                "Get runtime capabilities",
            )],
        ),
        RestPathDescriptor::new(
            "/models/catalog",
            vec![RestOperationDescriptor::new(
                "get",
                "Get the compiled-in model catalog",
            )],
        ),
        RestPathDescriptor::new(
            "/realtime/open_info",
            vec![RestOperationDescriptor::new(
                "post",
                "Get bootstrap metadata for opening a realtime channel",
            )],
        ),
        RestPathDescriptor::new(
            "/realtime/status",
            vec![RestOperationDescriptor::new(
                "post",
                "Get product-layer realtime channel status for a target",
            )],
        ),
        RestPathDescriptor::new(
            "/realtime/capabilities",
            vec![RestOperationDescriptor::new(
                "post",
                "Get product-layer realtime capabilities for a target",
            )],
        ),
        RestPathDescriptor::new(
            "/runtime/{id}/state",
            vec![RestOperationDescriptor::new("get", "Get runtime state")],
        ),
        RestPathDescriptor::new(
            "/runtime/{id}/realtime_attachment_status",
            vec![RestOperationDescriptor::new(
                "get",
                "Get runtime-owned realtime attachment status for a session",
            )],
        ),
        RestPathDescriptor::new(
            "/runtime/{id}/accept",
            vec![RestOperationDescriptor::new(
                "post",
                "Accept a runtime input",
            )],
        ),
        RestPathDescriptor::new(
            "/runtime/{id}/retire",
            vec![RestOperationDescriptor::new("post", "Retire a runtime")],
        ),
        RestPathDescriptor::new(
            "/runtime/{id}/reset",
            vec![RestOperationDescriptor::new("post", "Reset a runtime")],
        ),
        RestPathDescriptor::new(
            "/input/{id}/list",
            vec![RestOperationDescriptor::new(
                "get",
                "List runtime inputs for a session",
            )],
        ),
        RestPathDescriptor::new(
            "/input/{session_id}/{input_id}",
            vec![RestOperationDescriptor::new(
                "get",
                "Get a runtime input state",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/events",
            vec![RestOperationDescriptor::new("get", "SSE mob event stream")],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/spawn-helper",
            vec![RestOperationDescriptor::new(
                "post",
                "Spawn a helper member in a mob",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/fork-helper",
            vec![RestOperationDescriptor::new(
                "post",
                "Fork a helper member in a mob",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/members/{agent_identity}/status",
            vec![RestOperationDescriptor::new(
                "get",
                "Get realtime attachment status for a mob member",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/members/{agent_identity}/realtime/attach",
            vec![RestOperationDescriptor::new(
                "post",
                "Attach realtime attachment intent to a mob member",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/members/{agent_identity}/realtime/detach",
            vec![RestOperationDescriptor::new(
                "post",
                "Detach realtime attachment intent from a mob member",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/members/{agent_identity}/cancel",
            vec![RestOperationDescriptor::new(
                "post",
                "Force-cancel a mob member",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/members/{agent_identity}/respawn",
            vec![RestOperationDescriptor::new(
                "post",
                "Respawn a mob member with topology restore",
            )],
        ),
        RestPathDescriptor::new(
            "/health",
            vec![RestOperationDescriptor::new("get", "Health check")],
        ),
        // Phase 4c — auth + realm endpoints.
        RestPathDescriptor::new(
            "/auth/profiles",
            vec![
                RestOperationDescriptor::new("get", "List auth profiles"),
                RestOperationDescriptor::new("post", "Create an auth profile"),
            ],
        ),
        RestPathDescriptor::new(
            "/auth/profiles/{id}",
            vec![
                RestOperationDescriptor::new("get", "Get one auth profile"),
                RestOperationDescriptor::new("delete", "Delete an auth profile"),
            ],
        ),
        RestPathDescriptor::new(
            "/auth/profiles/{id}/test",
            vec![RestOperationDescriptor::new(
                "post",
                "Dry-run the profile's resolve path",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/login/start",
            vec![RestOperationDescriptor::new(
                "post",
                "Begin OAuth login (loopback flow)",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/login/complete",
            vec![RestOperationDescriptor::new(
                "post",
                "Finish OAuth login with an authorization code",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/login/device/start",
            vec![RestOperationDescriptor::new(
                "post",
                "Begin device-code OAuth login",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/status/{id}",
            vec![RestOperationDescriptor::new("get", "Get auth status")],
        ),
        RestPathDescriptor::new(
            "/auth/logout/{id}",
            vec![RestOperationDescriptor::new(
                "post",
                "Revoke + remove persisted credentials",
            )],
        ),
        RestPathDescriptor::new(
            "/realms",
            vec![RestOperationDescriptor::new("get", "List realm summaries")],
        ),
        RestPathDescriptor::new(
            "/realms/{id}",
            vec![RestOperationDescriptor::new(
                "get",
                "Get a realm's connection set",
            )],
        ),
    ]
}

pub fn rest_documented_paths() -> Vec<&'static str> {
    rest_path_catalog()
        .into_iter()
        .map(|entry| entry.path)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keeps_live_config_and_member_routes() {
        let paths = rest_documented_paths();
        for expected in [
            "/config",
            "/schedules",
            "/schedules/{id}/occurrences",
            "/runtime/{id}/realtime_attachment_status",
            "/mob/{id}/members/{agent_identity}/status",
            "/mob/{id}/members/{agent_identity}/realtime/attach",
            "/mob/{id}/members/{agent_identity}/realtime/detach",
            "/mob/{id}/members/{agent_identity}/cancel",
            "/mob/{id}/members/{agent_identity}/respawn",
        ] {
            assert!(paths.iter().any(|path| path == &expected));
        }
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn catalog_keeps_live_mcp_route_descriptions() {
        let catalog = rest_path_catalog();
        let mcp_add = catalog
            .iter()
            .find(|entry| entry.path == "/sessions/{id}/mcp/add")
            .expect("mcp/add path should remain documented");
        let post = mcp_add
            .operations
            .iter()
            .find(|operation| operation.method == "post")
            .expect("mcp/add POST should remain documented");
        assert_eq!(
            post.description,
            Some("Requires mcp_live capability. Check GET /capabilities.")
        );
    }
}
