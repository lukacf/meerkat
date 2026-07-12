//! Canonical REST surface catalog.
//!
//! Single owner of the documented REST path/method surface **and** the typed
//! request/response contract per operation. The OpenAPI emission
//! (`emit.rs`) is a pure projection of this catalog plus `schema_for!` of
//! the referenced wire structs — there is no second hand-maintained
//! (path, method) → schema map anywhere.

use serde::Serialize;

/// Marker component for declared-untyped JSON pass-through bodies.
pub const REST_SCHEMA_JSON_VALUE: &str = "JsonValue";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RestOperationDescriptor {
    pub method: &'static str,
    pub summary: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'static str>,
    /// Component schema name of the JSON request body, when one is accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_schema: Option<&'static str>,
    /// Whether the request body is mandatory.
    pub request_required: bool,
    /// Component schema name of the success response body.
    pub response_schema: &'static str,
    /// Content type of the success response.
    pub response_content_type: &'static str,
}

impl RestOperationDescriptor {
    const fn json(
        method: &'static str,
        summary: &'static str,
        response_schema: &'static str,
    ) -> Self {
        Self {
            method,
            summary,
            description: None,
            request_schema: None,
            request_required: false,
            response_schema,
            response_content_type: "application/json",
        }
    }

    const fn with_json_request(
        method: &'static str,
        summary: &'static str,
        request_schema: &'static str,
        response_schema: &'static str,
    ) -> Self {
        Self {
            method,
            summary,
            description: None,
            request_schema: Some(request_schema),
            request_required: true,
            response_schema,
            response_content_type: "application/json",
        }
    }

    const fn with_optional_json_request(
        method: &'static str,
        summary: &'static str,
        request_schema: &'static str,
        response_schema: &'static str,
    ) -> Self {
        Self {
            method,
            summary,
            description: None,
            request_schema: Some(request_schema),
            request_required: false,
            response_schema,
            response_content_type: "application/json",
        }
    }

    const fn text(
        method: &'static str,
        summary: &'static str,
        response_schema: &'static str,
    ) -> Self {
        Self {
            method,
            summary,
            description: None,
            request_schema: None,
            request_required: false,
            response_schema,
            response_content_type: "text/plain",
        }
    }

    const fn event_stream(
        method: &'static str,
        summary: &'static str,
        response_schema: &'static str,
    ) -> Self {
        Self {
            method,
            summary,
            description: None,
            request_schema: None,
            request_required: false,
            response_schema,
            response_content_type: "text/event-stream",
        }
    }

    const fn with_description(mut self, description: &'static str) -> Self {
        self.description = Some(description);
        self
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
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Ask Meerkat usage help",
                "HelpRequest",
                "HelpResponse",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions",
            vec![
                RestOperationDescriptor::json("get", "List sessions", "ListSessionsResult"),
                RestOperationDescriptor::with_json_request(
                    "post",
                    "Create and run a new session",
                    "RestCreateSessionRequest",
                    "WireRunResult",
                ),
            ],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}",
            vec![
                RestOperationDescriptor::json(
                    "get",
                    "Get session details",
                    "RestSessionDetailsResponse",
                ),
                RestOperationDescriptor::json("delete", "Archive a session", "StatusResponse"),
            ],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/history",
            vec![RestOperationDescriptor::json(
                "get",
                "Get full session history",
                "WireSessionHistory",
            )],
        ),
        // NOTE: the transcript-edit family (`transcript-revisions/{revision}`,
        // `rewrite-transcript`, `restore-transcript-revision`) is deliberately
        // NOT served by REST — it is exposed only via JSON-RPC
        // (`session/rewrite_transcript` etc.) and SessionService. These paths
        // were briefly catalogued (#739) then dropped from the axum router in
        // the v0.6.27 rebase while the catalog entries lingered, advertising
        // phantom 404 endpoints. Re-adding them here without a matching
        // `router()` arm will fail `verify-rest-surface-alignment`.
        RestPathDescriptor::new(
            "/sessions/{id}/interrupt",
            vec![RestOperationDescriptor::json(
                "post",
                "Interrupt a running session",
                "StatusResponse",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/system_context",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Append system context to a session",
                "RestAppendSystemContextRequest",
                "StatusResponse",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/messages",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Continue session with new message",
                "RestContinueSessionRequest",
                "WireRunResult",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/external-events",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Queue a runtime-backed external event",
                REST_SCHEMA_JSON_VALUE,
                "StatusResponse",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/peer-response-terminal",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Admit a correlated terminal peer response through the typed runtime ingress",
                "RestPeerResponseTerminalRequest",
                "StatusResponse",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/events",
            vec![RestOperationDescriptor::event_stream(
                "get",
                "SSE event stream",
                "SseEventStream",
            )],
        ),
        RestPathDescriptor::new(
            "/requests/{request_id}/cancel",
            vec![RestOperationDescriptor::json(
                "post",
                "Cancel an uncommitted in-flight request",
                REST_SCHEMA_JSON_VALUE,
            )],
        ),
        RestPathDescriptor::new(
            "/schedule/tools",
            vec![RestOperationDescriptor::json(
                "get",
                "List schedule tools",
                "ScheduleToolsResult",
            )],
        ),
        RestPathDescriptor::new(
            "/schedule/call",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Invoke a schedule tool",
                "ScheduleToolCallParams",
                REST_SCHEMA_JSON_VALUE,
            )],
        ),
        RestPathDescriptor::new(
            "/schedules",
            vec![
                RestOperationDescriptor::json("get", "List schedules", "ScheduleListResult"),
                RestOperationDescriptor::with_json_request(
                    "post",
                    "Create schedule",
                    "CreateScheduleRequest",
                    "Schedule",
                ),
            ],
        ),
        RestPathDescriptor::new(
            "/schedules/{id}",
            vec![
                RestOperationDescriptor::json("get", "Get schedule", "Schedule"),
                RestOperationDescriptor::with_json_request(
                    "patch",
                    "Update schedule",
                    "UpdateScheduleRequest",
                    "Schedule",
                ),
                RestOperationDescriptor::json("delete", "Delete schedule", "Schedule"),
            ],
        ),
        RestPathDescriptor::new(
            "/schedules/{id}/pause",
            vec![RestOperationDescriptor::json(
                "post",
                "Pause schedule",
                "Schedule",
            )],
        ),
        RestPathDescriptor::new(
            "/schedules/{id}/resume",
            vec![RestOperationDescriptor::json(
                "post",
                "Resume schedule",
                "Schedule",
            )],
        ),
        RestPathDescriptor::new(
            "/schedules/{id}/occurrences",
            vec![RestOperationDescriptor::json(
                "get",
                "List schedule occurrences",
                "ScheduleOccurrencesResult",
            )],
        ),
        RestPathDescriptor::new(
            "/comms/send",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Send a comms message",
                "CommsSendParams",
                "CommsSendResult",
            )],
        ),
        RestPathDescriptor::new(
            "/comms/peers",
            vec![RestOperationDescriptor::json(
                "get",
                "List resolved comms peers",
                "CommsPeersResult",
            )],
        ),
        RestPathDescriptor::new(
            "/config",
            vec![
                RestOperationDescriptor::json("get", "Read config", "ConfigEnvelope"),
                RestOperationDescriptor::with_json_request(
                    "put",
                    "Replace config",
                    "RestSetConfigRequest",
                    "ConfigEnvelope",
                ),
                RestOperationDescriptor::with_json_request(
                    "patch",
                    "Merge-patch config",
                    "RestPatchConfigRequest",
                    "ConfigEnvelope",
                ),
            ],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/mcp/add",
            vec![
                RestOperationDescriptor::with_json_request(
                    "post",
                    "Stage live MCP server addition",
                    "McpAddParams",
                    "McpLiveOpResponse",
                )
                .with_description("Requires mcp_live capability. Check GET /capabilities."),
            ],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/mcp/remove",
            vec![
                RestOperationDescriptor::with_json_request(
                    "post",
                    "Stage live MCP server removal",
                    "McpRemoveParams",
                    "McpLiveOpResponse",
                )
                .with_description("Requires mcp_live capability. Check GET /capabilities."),
            ],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/mcp/reload",
            vec![
                RestOperationDescriptor::with_json_request(
                    "post",
                    "Stage live MCP server reload",
                    "McpReloadParams",
                    "McpLiveOpResponse",
                )
                .with_description("Requires mcp_live capability. Check GET /capabilities."),
            ],
        ),
        RestPathDescriptor::new(
            "/skills",
            vec![RestOperationDescriptor::json(
                "get",
                "List available skills",
                "SkillListResponse",
            )],
        ),
        RestPathDescriptor::new(
            "/capabilities",
            vec![RestOperationDescriptor::json(
                "get",
                "Get runtime capabilities",
                "CapabilitiesResponse",
            )],
        ),
        RestPathDescriptor::new(
            "/runtime/host_info",
            vec![RestOperationDescriptor::json(
                "get",
                "Get read-only runtime host information",
                "RuntimeHostInfo",
            )],
        ),
        RestPathDescriptor::new(
            "/runtime/capabilities",
            vec![RestOperationDescriptor::json(
                "get",
                "Get runtime host capability flags",
                "RuntimeHostCapabilities",
            )],
        ),
        RestPathDescriptor::new(
            "/runtime/health",
            vec![RestOperationDescriptor::json(
                "get",
                "Get runtime host health",
                "RuntimeHostHealth",
            )],
        ),
        RestPathDescriptor::new(
            "/models/catalog",
            vec![RestOperationDescriptor::json(
                "get",
                "Get the compiled-in model catalog",
                "ModelsCatalogResponse",
            )],
        ),
        RestPathDescriptor::new(
            "/sessions/{id}/status",
            vec![RestOperationDescriptor::json(
                "get",
                "Get a session's current runtime state",
                "RuntimeStateResult",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/events",
            vec![RestOperationDescriptor::event_stream(
                "get",
                "SSE mob event stream",
                "SseEventStream",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/spawn-helper",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Spawn a helper member in a mob",
                "RestMobHelperRequest",
                REST_SCHEMA_JSON_VALUE,
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/fork-helper",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Fork a helper member in a mob",
                "RestMobForkHelperRequest",
                REST_SCHEMA_JSON_VALUE,
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/wait-kickoff",
            vec![RestOperationDescriptor::with_optional_json_request(
                "post",
                "Wait for autonomous kickoff completion",
                "RestMobWaitRequest",
                REST_SCHEMA_JSON_VALUE,
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/wire-members-batch",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Wire multiple local mob member edges",
                "RestMobWireMembersBatchRequest",
                "MobWireMembersBatchResult",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/members/{agent_identity}/status",
            vec![
                RestOperationDescriptor::json(
                    "get",
                    "Get a mob member execution status snapshot",
                    REST_SCHEMA_JSON_VALUE,
                )
                .with_description(
                    "Returns the current execution/status snapshot for the named mob member.",
                ),
            ],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/members/{agent_identity}/cancel",
            vec![RestOperationDescriptor::json(
                "post",
                "Force-cancel a mob member",
                REST_SCHEMA_JSON_VALUE,
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/members/{agent_identity}/respawn",
            vec![RestOperationDescriptor::json(
                "post",
                "Respawn a mob member with topology restore",
                REST_SCHEMA_JSON_VALUE,
            )],
        ),
        // NOTE (SD-2, phase 7): REST serves multi-host OBSERVATION only —
        // the three GETs below. Host bind/revoke, hard-cancel, the member
        // live family, and grant administration are deliberately NOT served
        // by REST; they are exposed via JSON-RPC (`mob/bind_host` etc.) and
        // the CLI. Re-adding an admin path here without a matching axum
        // `router()` arm will fail `verify-rest-surface-alignment`
        // (phantom-404 precedent above).
        RestPathDescriptor::new(
            "/mob/{id}/members/{agent_identity}/history",
            vec![RestOperationDescriptor::json(
                "get",
                "Read a mob member transcript page by identity",
                "MobMemberHistoryResult",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/hosts",
            vec![RestOperationDescriptor::json(
                "get",
                "List tracked member hosts with bind phase and declared capabilities",
                "MobHostsResult",
            )],
        ),
        RestPathDescriptor::new(
            "/mob/{id}/route-installs",
            vec![RestOperationDescriptor::json(
                "get",
                "Outstanding cross-host route-install obligations",
                "MobRouteInstallsResult",
            )],
        ),
        RestPathDescriptor::new(
            "/health",
            vec![RestOperationDescriptor::text(
                "get",
                "Health check",
                "PlainTextResponse",
            )],
        ),
        // Phase 4c — auth + realm endpoints.
        RestPathDescriptor::new(
            "/auth/profiles",
            vec![
                RestOperationDescriptor::json(
                    "get",
                    "List realm auth profiles, backend profiles, and bindings",
                    "WireAuthProfilesList",
                ),
                RestOperationDescriptor::with_json_request(
                    "post",
                    "Store binding-scoped credentials",
                    "RestAuthProfileCreateRequest",
                    "WireAuthProfileCreated",
                ),
            ],
        ),
        RestPathDescriptor::new(
            "/auth/bindings/{binding_id}",
            vec![
                RestOperationDescriptor::json(
                    "get",
                    "Get binding-scoped auth profile",
                    "WireAuthProfileDetail",
                ),
                RestOperationDescriptor::json(
                    "delete",
                    "Delete binding-scoped credentials",
                    "WireAuthProfileCleared",
                ),
            ],
        ),
        RestPathDescriptor::new(
            "/auth/bindings/{binding_id}/test",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Test a binding resolve path",
                "RestAuthBindingTestRequest",
                "WireAuthStatusDetail",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/login/start",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Begin OAuth login (loopback flow)",
                "LoginStartParams",
                "WireLoginStart",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/login/complete",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Finish OAuth login with an authorization code",
                "LoginCompleteParams",
                "WireLoginReady",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/login/device/start",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Begin device-code OAuth login",
                "DeviceStartParams",
                "WireDeviceStart",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/login/device/complete",
            vec![RestOperationDescriptor::with_json_request(
                "post",
                "Complete device-code OAuth login",
                "DeviceCompleteParams",
                "WireDeviceCompleteResult",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/bindings/{binding_id}/status",
            vec![RestOperationDescriptor::json(
                "get",
                "Get binding auth status",
                "WireAuthStatusDetail",
            )],
        ),
        RestPathDescriptor::new(
            "/auth/bindings/{binding_id}/logout",
            vec![RestOperationDescriptor::json(
                "post",
                "Log out a binding",
                "WireAuthProfileCleared",
            )],
        ),
        RestPathDescriptor::new(
            "/realms",
            vec![RestOperationDescriptor::json(
                "get",
                "List realm summaries",
                "WireRealmList",
            )],
        ),
        RestPathDescriptor::new(
            "/realms/{id}",
            vec![RestOperationDescriptor::json(
                "get",
                "Get a realm's connection set",
                "WireRealmConnectionSet",
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
                        // WorkGraph owns its REST contract map; the catalog
                        // projects it rather than re-declaring it.
                        match meerkat_workgraph::workgraph_rest_request_response_schema(
                            entry.path,
                            operation.method,
                        ) {
                            Some((Some(request_schema), response_schema)) => {
                                RestOperationDescriptor::with_json_request(
                                    operation.method,
                                    operation.summary,
                                    request_schema,
                                    response_schema,
                                )
                            }
                            Some((None, response_schema)) => RestOperationDescriptor::json(
                                operation.method,
                                operation.summary,
                                response_schema,
                            ),
                            None => RestOperationDescriptor::json(
                                operation.method,
                                operation.summary,
                                REST_SCHEMA_JSON_VALUE,
                            ),
                        }
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
            "/mob/{id}/members/{agent_identity}/history",
            "/mob/{id}/hosts",
            "/mob/{id}/route-installs",
        ] {
            assert!(paths.iter().any(|path| path == &expected));
        }
        for retired in [
            // Transcript-edit family is RPC-only; never re-add to the REST
            // catalog without a matching axum router() arm + handler.
            "/sessions/{id}/transcript-revisions/{revision}",
            "/sessions/{id}/rewrite-transcript",
            "/sessions/{id}/restore-transcript-revision",
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

    /// Catalog totality: every documented operation carries an explicit
    /// response contract, and `JsonValue` pass-throughs are the only
    /// declared-untyped refs (no empty/implicit contracts can exist —
    /// the descriptor type has no schema-less constructor).
    #[test]
    fn every_operation_declares_a_response_contract() {
        for path in rest_path_catalog() {
            for operation in &path.operations {
                assert!(
                    !operation.response_schema.is_empty(),
                    "{} {} has no response contract",
                    operation.method,
                    path.path
                );
            }
        }
    }

    /// The create/continue session contracts are the real wire structs —
    /// the catalog refs must match the types emitted via `schema_for!` so
    /// drift like the historical missing `enable_web_search` is structural
    /// rather than editorial.
    #[test]
    fn session_bodies_reference_contract_wire_structs() {
        let catalog = rest_path_catalog();
        let sessions = catalog
            .iter()
            .find(|entry| entry.path == "/sessions")
            .and_then(|entry| {
                entry
                    .operations
                    .iter()
                    .find(|operation| operation.method == "post")
            });
        assert_eq!(
            sessions.and_then(|op| op.request_schema),
            Some("RestCreateSessionRequest")
        );
        let messages = catalog
            .iter()
            .find(|entry| entry.path == "/sessions/{id}/messages")
            .and_then(|entry| {
                entry
                    .operations
                    .iter()
                    .find(|operation| operation.method == "post")
            });
        assert_eq!(
            messages.and_then(|op| op.request_schema),
            Some("RestContinueSessionRequest")
        );
    }
}
