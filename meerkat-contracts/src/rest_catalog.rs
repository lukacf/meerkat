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
    let mut paths = vec![
        RestPathDescriptor::new(
            "/help",
            vec![RestOperationDescriptor::new(
                "post",
                "Ask Meerkat usage help",
            )],
        ),
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
            "/requests/{request_id}/cancel",
            vec![RestOperationDescriptor::new(
                "post",
                "Cancel an uncommitted in-flight request",
            )],
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
            "/capabilities",
            vec![RestOperationDescriptor::new(
                "get",
                "Get runtime capabilities",
            )],
        ),
        RestPathDescriptor::new(
            "/runtime/host_info",
            vec![RestOperationDescriptor::new(
                "get",
                "Get read-only runtime host information",
            )],
        ),
        RestPathDescriptor::new(
            "/runtime/capabilities",
            vec![RestOperationDescriptor::new(
                "get",
                "Get runtime host capability flags",
            )],
        ),
        RestPathDescriptor::new(
            "/runtime/health",
            vec![RestOperationDescriptor::new(
                "get",
                "Get runtime host health",
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
            "/sessions/{id}/status",
            vec![RestOperationDescriptor::new(
                "get",
                "Get a session's current runtime state",
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
            "/mob/{id}/wait-kickoff",
            vec![RestOperationDescriptor::new(
                "post",
                "Wait for autonomous kickoff completion",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/wire-members-batch",
            vec![RestOperationDescriptor::new(
                "post",
                "Wire multiple local mob member edges",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/members/{agent_identity}/status",
            vec![RestOperationDescriptor::with_description(
                "get",
                "Get a mob member execution status snapshot",
                "Returns the current execution/status snapshot for the named mob member.",
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
                RestOperationDescriptor::new(
                    "get",
                    "List realm auth profiles, backend profiles, and bindings",
                ),
                RestOperationDescriptor::new("post", "Store binding-scoped credentials"),
            ],
        ),
        RestPathDescriptor::new(
            "/auth/bindings/{binding_id}",
            vec![
                RestOperationDescriptor::new("get", "Get binding-scoped auth profile"),
                RestOperationDescriptor::new("delete", "Delete binding-scoped credentials"),
            ],
        ),
        RestPathDescriptor::new(
            "/auth/bindings/{binding_id}/test",
            vec![RestOperationDescriptor::new(
                "post",
                "Test a binding resolve path",
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
            "/auth/login/device/complete",
            vec![RestOperationDescriptor::new(
                "post",
                "Complete device-code OAuth login",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/bindings/{binding_id}/status",
            vec![RestOperationDescriptor::new(
                "get",
                "Get binding auth status",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/bindings/{binding_id}/logout",
            vec![RestOperationDescriptor::new("post", "Log out a binding")],
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
    ];
    let workgraph_paths = meerkat_workgraph::workgraph_rest_path_catalog()
        .iter()
        .map(|entry| {
            RestPathDescriptor::new(
                entry.path,
                entry
                    .operations
                    .iter()
                    .map(|operation| {
                        RestOperationDescriptor::new(operation.method, operation.summary)
                    })
                    .collect(),
            )
        })
        .collect::<Vec<_>>();
    let insert_at = paths
        .iter()
        .position(|entry| entry.path == "/comms/send")
        .unwrap_or(paths.len());
    paths.splice(insert_at..insert_at, workgraph_paths);
    paths
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
    fn catalog_keeps_live_config_auth_and_member_routes() {
        let paths = rest_documented_paths();
        for expected in [
            "/config",
            "/schedules",
            "/schedules/{id}/occurrences",
            "/auth/bindings/{binding_id}",
            "/auth/bindings/{binding_id}/test",
            "/auth/login/complete",
            "/auth/login/device/complete",
            "/auth/bindings/{binding_id}/status",
            "/auth/bindings/{binding_id}/logout",
            "/mob/{id}/wait-kickoff",
            "/mob/{id}/wire-members-batch",
            "/mob/{id}/members/{agent_identity}/status",
            "/mob/{id}/members/{agent_identity}/cancel",
            "/mob/{id}/members/{agent_identity}/respawn",
        ] {
            assert!(paths.iter().any(|path| path == &expected));
        }
        for retired in [
            "/realtime/open_info",
            "/realtime/status",
            "/realtime/capabilities",
            "/sessions/{id}/realtime-attachment-status",
            "/mob/{id}/members/{agent_identity}/realtime/attach",
            "/mob/{id}/members/{agent_identity}/realtime/detach",
            "/skills/{id}",
            "/auth/profiles/{id}",
            "/auth/profiles/{id}/test",
            "/auth/status/{id}",
            "/auth/logout/{id}",
            "/sessions/{id}/submit",
            "/sessions/{id}/retire",
            "/sessions/{id}/reset",
            "/sessions/{id}/submissions",
            "/sessions/{session_id}/submissions/{submission_id}",
        ] {
            assert!(
                !paths.iter().any(|path| path == &retired),
                "retired REST route must not be catalogued: {retired}"
            );
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

    #[test]
    #[allow(clippy::expect_used)]
    fn catalog_labels_mob_member_status_as_execution_snapshot() {
        let catalog = rest_path_catalog();
        let member_status = catalog
            .iter()
            .find(|entry| entry.path == "/mob/{id}/members/{agent_identity}/status")
            .expect("mob member status path should remain documented");
        let get = member_status
            .operations
            .iter()
            .find(|operation| operation.method == "get")
            .expect("mob member status GET should remain documented");

        assert_eq!(get.summary, "Get a mob member execution status snapshot");
        assert_eq!(
            get.description,
            Some("Returns the current execution/status snapshot for the named mob member.")
        );
        assert!(
            !get.summary.contains("realtime attachment")
                && get
                    .description
                    .is_none_or(|description| !description.contains("realtime attachment")),
            "mob member status route must not be labelled as realtime attachment status"
        );
    }
}
