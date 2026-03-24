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
            "/sessions/{id}/events",
            vec![RestOperationDescriptor::new("get", "SSE event stream")],
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
            "/runtime/{id}/state",
            vec![RestOperationDescriptor::new("get", "Get runtime state")],
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
            "/mob/prefabs",
            vec![RestOperationDescriptor::new("get", "List mob prefabs")],
        ),
        RestPathDescriptor::new(
            "/mob/tools",
            vec![RestOperationDescriptor::new("get", "List mob tools")],
        ),
        RestPathDescriptor::new(
            "/mob/call",
            vec![RestOperationDescriptor::new(
                "post",
                "Invoke a mob tool call",
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
            "/mob/{id}/members/{meerkat_id}/status",
            vec![RestOperationDescriptor::new(
                "get",
                "Get live status for a mob member",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/members/{meerkat_id}/cancel",
            vec![RestOperationDescriptor::new(
                "post",
                "Force-cancel a mob member",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/members/{meerkat_id}/respawn",
            vec![RestOperationDescriptor::new(
                "post",
                "Respawn a mob member with topology restore",
            )],
        ),
        RestPathDescriptor::new(
            "/health",
            vec![RestOperationDescriptor::new("get", "Health check")],
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
            "/mob/{id}/members/{meerkat_id}/status",
            "/mob/{id}/members/{meerkat_id}/cancel",
            "/mob/{id}/members/{meerkat_id}/respawn",
        ] {
            assert!(paths.iter().any(|path| path == &expected));
        }
    }

    #[test]
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
