//! Authenticated MCP Apps operations for the exact originating member/session.
//! UI bodies come from native live observations or retained canonical history.
//! The console frame log contains invocation locators only.

use super::*;
use meerkat_core::{
    ToolApplicationControlRequest, ToolApplicationIngress, ToolApplicationOperation,
    ToolApplicationRequest,
};
use meerkat_mcp::apps::{MCP_APPS_EXTENSION, McpAppInvocation, tool_ui_resource_uri};

#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Request {
    identity: String,
    session_id: String,
    tool_call_id: String,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<Value>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    Resolve,
    ReadResource,
    CallTool,
}

#[derive(Clone)]
pub struct ConsoleToolApplicationIngress {
    state: ConsoleJsonState,
    headers: HeaderMap,
    uri: Uri,
    request: Request,
    public_identity: String,
    registration: Option<crate::console_aggregator::ConsoleApplicationRuntime>,
    operation: Operation,
    principal: Option<String>,
}

type Ingress = ConsoleToolApplicationIngress;

fn apps_enabled(state: &ConsoleJsonState) -> bool {
    state
        .decisions
        .console
        .ui
        .mcp_apps_sandbox_url
        .as_ref()
        .is_some_and(|url| !url.trim().is_empty())
}

impl ConsoleToolApplicationIngress {
    /// Actual authenticated Console principal. `None` denotes a host-protected
    /// open Console; it is never an inferred user or member identity.
    pub fn principal(&self) -> Option<&str> {
        self.principal.as_deref()
    }
    /// Public Console identity independently checked against native ownership.
    pub fn identity(&self) -> &str {
        &self.public_identity
    }
}

impl ToolApplicationIngress for Ingress {
    fn revalidate(&self) -> Result<(), meerkat_core::OperationAuthorizationError> {
        // This is the gateway's effective, realm-resolved Console config.
        // An HTTP caller cannot enable Apps by bypassing the absent UI.
        if !apps_enabled(&self.state) {
            return Err(meerkat_core::OperationAuthorizationError::Unavailable);
        }
        let auth = console_request_auth_context(&self.state, &self.headers, &self.uri)
            .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?;
        let fresh_io = self.operation != Operation::Resolve;
        if self.registration.as_ref().is_some_and(|registration| {
            self.state
                .console_aggregator
                .as_ref()
                .is_none_or(|aggregator| !aggregator.application_runtime_is_current(registration))
        }) || auth.principal != self.principal
            || (fresh_io && self.state.decisions.console.read_only)
            || auth.access_view.as_ref().is_some_and(|view| {
                view.enforced()
                    && (!view.allows_agent(ACTION_AGENT_VIEW, &self.public_identity)
                        || (fresh_io
                            && !view.allows_agent(ACTION_AGENT_SEND, &self.public_identity)))
            })
        {
            return Err(meerkat_core::OperationAuthorizationError::Unavailable);
        }
        Ok(())
    }

    fn revalidate_async(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), meerkat_core::OperationAuthorizationError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            self.revalidate()?;
            authorize_session_in_namespace(
                &self.state,
                &self.request,
                self.operation,
                self.registration
                    .as_ref()
                    .map_or("", |registration| registration.namespace()),
            )
            .await
            .map_err(|()| meerkat_core::OperationAuthorizationError::Unavailable)?;
            self.revalidate()
        })
    }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

/// Identity/session association is read from native current member/continuity
/// ownership. A frame or client-supplied locator cannot manufacture it.
async fn authorize_session(
    state: &ConsoleJsonState,
    request: &Request,
    operation: Operation,
) -> Result<MobRuntime, ()> {
    authorize_session_in_namespace(state, request, operation, "").await
}

async fn authorize_session_in_namespace(
    state: &ConsoleJsonState,
    request: &Request,
    operation: Operation,
    namespace: &str,
) -> Result<MobRuntime, ()> {
    let runtime = state.runtime.as_ref().ok_or(())?;
    let live_members = match state
        .access
        .as_ref()
        .filter(|controller| controller.enabled())
    {
        Some(controller) => {
            prime_access_cache_from_runtime_in_namespace(runtime, controller, namespace).await
        }
        None => BTreeSet::new(),
    };
    let handle = runtime.handle();
    // Every resolver refusal denies the session, so a stale runtime id (a
    // typed refusal) is denied exactly as when it resolved to nothing.
    let resolved = resolve_console_identity_control_target(
        "mcp_app_session",
        &handle,
        state
            .identity_runtime
            .as_ref()
            .map(crate::identity_first::PublicIdentityRuntime::runtime),
        state.visibility_policy.as_ref(),
        &request.identity,
    )
    .await
    .map_err(|_| ())?;
    if let Some((identity, registered, alias)) = resolved {
        if registered {
            let owner = state.identity_runtime.as_ref().ok_or(())?;
            let status = owner.status(&identity).await.map_err(|_| ())?;
            if let Some(access) = state.access.as_ref().filter(|access| access.enabled()) {
                // A live member's value was recorded above, as every route
                // records it; recording the identity again from its status
                // would give it a second value.
                if !live_members.contains(&namespace_console_identity(&request.identity, namespace))
                {
                    let mut labels = status.labels.clone();
                    crate::console_spawn::sanitize_unverified_lineage_labels(&mut labels);
                    if let Some(registered) = runtime
                        .console_identity_labels()
                        .await
                        .get(identity.as_str())
                    {
                        crate::console_spawn::merge_registered_labels(&mut labels, registered);
                    }
                    access.record_agent_attributes(namespace_console_attributes(
                        AgentResourceAttributes {
                            identity: request.identity.clone(),
                            agent_id: status.agent_runtime_id.as_ref().map(ToString::to_string),
                            role: status.profile.as_ref().map(ToString::to_string),
                            labels,
                        },
                        namespace,
                    ));
                }
                access.record_current_runtime_aliases([(
                    namespace_console_identity(identity.as_str(), namespace),
                    status
                        .agent_runtime_id
                        .as_ref()
                        .map(|alias| namespace_console_identity(alias.as_str(), namespace)),
                )]);
            }
            if status
                .session_id
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                != Some(request.session_id.as_str())
            {
                if operation != Operation::Resolve {
                    return Err(());
                }
                let session_id =
                    meerkat_core::SessionId::parse(&request.session_id).map_err(|_| ())?;
                let owner = owner
                    .core_session_owner(&session_id)
                    .await
                    .map_err(|_| ())?;
                if owner.as_ref() != Some(&identity) {
                    return Err(());
                }
            }
            if operation == Operation::Resolve {
                return Ok(runtime.clone());
            }
        }
        if let Some(alias) = alias
            && alias.session_id.as_deref() == Some(request.session_id.as_str())
            && runtime_alias_visible_to_console(&handle, state.visibility_policy.as_ref(), &alias)
            && (operation == Operation::Resolve
                || alias.member.status != meerkat_mob::MobMemberStatus::Retiring)
        {
            return Ok(runtime.clone());
        }
    }
    let aliases = lookup_visible_member_alias_candidates_with_session(
        &handle,
        state.visibility_policy.as_ref(),
        &request.identity,
    )
    .await;
    if aliases.len() == 1
        && aliases[0].session_id.as_deref() == Some(request.session_id.as_str())
        && (operation == Operation::Resolve
            || aliases[0].member.status != meerkat_mob::MobMemberStatus::Retiring)
    {
        Ok(runtime.clone())
    } else {
        Err(())
    }
}

async fn route(
    state: ConsoleJsonState,
    request: Request,
    operation: Operation,
) -> Result<
    (
        ConsoleJsonState,
        Request,
        Option<crate::console_aggregator::ConsoleApplicationRuntime>,
    ),
    (),
> {
    if state.runtime.is_some() {
        authorize_session(&state, &request, operation).await?;
        return Ok((state, request, None));
    }
    let aggregator = state.console_aggregator.as_ref().ok_or(())?;
    let mut selected = None;
    for candidate in aggregator.application_runtime_candidates(&request.identity) {
        if !aggregator.application_runtime_is_current(&candidate) {
            continue;
        }
        let mut local_state = state.clone();
        local_state.runtime = Some(candidate.runtime().clone());
        local_state.identity_runtime = candidate.identity_runtime();
        local_state.visibility_policy = candidate.visibility_policy();
        let mut local_request = request.clone();
        local_request.identity = candidate.identity.clone();
        if authorize_session_in_namespace(
            &local_state,
            &local_request,
            operation,
            candidate.namespace(),
        )
        .await
        .is_ok()
            && aggregator.application_runtime_is_current(&candidate)
        {
            if selected.is_some() {
                return Err(());
            }
            selected = Some((local_state, local_request, Some(candidate)));
        }
    }
    selected.ok_or(())
}

async fn original_invocation(
    runtime: &MobRuntime,
    request: &Request,
) -> Result<McpAppInvocation, ()> {
    // The native owner projects accepted live results without blocking behind
    // the next model request and uses committed history for retained replay.
    // A browser frame never supplies the original result or its host carrier.
    let observations = runtime
        .read_tool_application_observations(&request.session_id)
        .await
        .map_err(|_| ())?;
    let mut found = None;
    for observation in observations {
        if observation.tool_call_id == request.tool_call_id {
            let invocation = observation
                .host_metadata
                .get(MCP_APPS_EXTENSION)
                .ok_or(())?;
            let invocation: McpAppInvocation =
                serde_json::from_value(invocation.clone()).map_err(|_| ())?;
            if tool_ui_resource_uri(&invocation.tool).is_none()
                || found.replace(invocation).is_some()
            {
                return Err(());
            }
        }
    }
    found.ok_or(())
}

fn cached_renderer_resource(
    original: &McpAppInvocation,
    uri: Option<&str>,
) -> Result<Option<Value>, ()> {
    if uri.is_none() || uri != tool_ui_resource_uri(&original.tool) {
        return Ok(None);
    }
    original
        .resource
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|_| ())
}

fn unavailable() -> axum::response::Response {
    let mut response = console_json_error(
        StatusCode::FORBIDDEN,
        "mcp_app_unavailable",
        "This tool app is unavailable for the current viewer or member",
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn execute(
    runtime: &MobRuntime,
    request: &Request,
    operation: ToolApplicationOperation,
    ingress: Arc<Ingress>,
) -> Result<Value, ()> {
    let session_id = meerkat_core::SessionId::parse(&request.session_id).map_err(|_| ())?;
    let control = ToolApplicationControlRequest::from_trusted_ingress(
        session_id,
        ToolApplicationRequest {
            tool_call_id: request.tool_call_id.clone(),
            extension: MCP_APPS_EXTENSION.into(),
            operation,
        },
        ingress,
    )
    .map_err(|_| ())?;
    let service = runtime.session_service().ok_or(())?;
    Arc::clone(service)
        .tool_application(control)
        .await
        .map_err(|error| {
            tracing::debug!(error = %error, "MCP app operation refused");
        })
}

async fn handle(
    state: ConsoleJsonState,
    headers: HeaderMap,
    uri: Uri,
    request: Request,
    operation: Operation,
) -> axum::response::Response {
    let Some(auth) = console_request_auth_context(&state, &headers, &uri) else {
        return console_json_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Console app access requires authorization",
        );
    };
    // A candidate launch calls no member's tool: 409 `candidate_launch`, the
    // REST form of the RPC surfaces' `-32047` refusal. Resolving and reading
    // an app's resource stay open.
    if operation == Operation::CallTool
        && state
            .runtime
            .as_ref()
            .is_some_and(|runtime| !runtime.speaks_for_composition())
    {
        return console_json_error(
            StatusCode::CONFLICT,
            "candidate_launch",
            "this gateway is a candidate launch and calls no member's tool; its members' work \
             belongs to the authoritative launch",
        );
    }
    if !apps_enabled(&state)
        || request.identity.trim().is_empty()
        || request.identity.len() > 1024
        || request.tool_call_id.trim().is_empty()
        || request.tool_call_id.len() > 1024
        || meerkat_core::SessionId::parse(&request.session_id).is_err()
        || crate::member_comms_id::validate_public_member_alias("identity", &request.identity)
            .is_err()
    {
        return unavailable();
    }
    let public_identity = request.identity.clone();
    // A retained renderer is history, including after session rotation. A
    // cache miss is upgraded to fresh resource admission before any native IO.
    let lookup_operation = if operation == Operation::ReadResource {
        Operation::Resolve
    } else {
        operation
    };
    let Ok((state, request, registration)) = route(state, request, lookup_operation).await else {
        return unavailable();
    };
    let mut ingress = Arc::new(Ingress {
        state: state.clone(),
        headers,
        uri,
        request: request.clone(),
        public_identity,
        registration,
        operation: lookup_operation,
        principal: auth.principal,
    });
    let Some(runtime) = state.runtime.as_ref() else {
        return unavailable();
    };
    if ingress.revalidate().is_err() {
        return unavailable();
    }
    let Ok(original) = original_invocation(runtime, &request).await else {
        return unavailable();
    };
    if ingress.revalidate_async().await.is_err() {
        return unavailable();
    }
    let cached_resource = if operation == Operation::ReadResource {
        match cached_renderer_resource(&original, request.uri.as_deref()) {
            Ok(resource) => resource,
            Err(()) => return unavailable(),
        }
    } else {
        None
    };
    if operation == Operation::ReadResource && cached_resource.is_none() {
        let mut fresh_ingress = (*ingress).clone();
        fresh_ingress.operation = Operation::ReadResource;
        ingress = Arc::new(fresh_ingress);
        if ingress.revalidate_async().await.is_err() {
            return unavailable();
        }
    }
    let mut response = if let Some(resource) = cached_resource {
        resource
    } else if operation == Operation::Resolve {
        // This advertises the host's action surface for the current member.
        // Exact tool permission and physical connection custody are checked
        // when an action enters native dispatch. A native capability probe here
        // would wait behind the running model turn and delay cached rendering.
        let can_call = !state.decisions.console.read_only
            && authorize_session_in_namespace(
                &state,
                &request,
                Operation::CallTool,
                ingress
                    .registration
                    .as_ref()
                    .map_or("", |registration| registration.namespace()),
            )
            .await
            .is_ok()
            && console_request_auth_context(&state, &ingress.headers, &ingress.uri).is_some_and(
                |auth| {
                    auth.access_view.as_ref().is_none_or(|view| {
                        !view.enforced()
                            || view.allows_agent(ACTION_AGENT_SEND, &ingress.public_identity)
                    })
                },
            );
        json!({ "tool": original.tool, "arguments": original.arguments, "result": original.result,
            "resource": original.resource, "canCallTools": can_call })
    } else {
        let operation = match operation {
            Operation::ReadResource => match request.uri.clone() {
                Some(uri) => ToolApplicationOperation::ReadResource { uri },
                None => return unavailable(),
            },
            Operation::CallTool => match (request.name.clone(), request.arguments.clone()) {
                (Some(name), Some(arguments)) => {
                    ToolApplicationOperation::CallTool { name, arguments }
                }
                _ => return unavailable(),
            },
            Operation::Resolve => unreachable!(),
        };
        match execute(runtime, &request, operation, ingress.clone()).await {
            Ok(value) => value,
            Err(()) => return unavailable(),
        }
    };
    if ingress.revalidate_async().await.is_err() {
        return unavailable();
    }
    if operation == Operation::Resolve
        && response.get("canCallTools").and_then(Value::as_bool) == Some(true)
    {
        let mut action_ingress = (*ingress).clone();
        action_ingress.operation = Operation::CallTool;
        response["canCallTools"] = Value::Bool(action_ingress.revalidate_async().await.is_ok());
        if ingress.revalidate().is_err() {
            return unavailable();
        }
    }
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(response),
    )
        .into_response()
}

pub(super) async fn resolve(
    State(state): State<ConsoleJsonState>,
    headers: HeaderMap,
    uri: Uri,
    Json(request): Json<Request>,
) -> axum::response::Response {
    handle(state, headers, uri, request, Operation::Resolve).await
}
pub(super) async fn read_resource(
    State(state): State<ConsoleJsonState>,
    headers: HeaderMap,
    uri: Uri,
    Json(request): Json<Request>,
) -> axum::response::Response {
    handle(state, headers, uri, request, Operation::ReadResource).await
}
pub(super) async fn call_tool(
    State(state): State<ConsoleJsonState>,
    headers: HeaderMap,
    uri: Uri,
    Json(request): Json<Request>,
) -> axum::response::Response {
    handle(state, headers, uri, request, Operation::CallTool).await
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn state(runtime: Option<MobRuntime>) -> ConsoleJsonState {
        ConsoleJsonState {
            session_fences: crate::auth::session_fence::ConsoleSessionFences::detached(),
            decisions: RuntimeDecisionState::local_console(
                crate::ConsolePolicy {
                    require_app_auth: false,
                    ui: crate::ConsoleUiConfig {
                        mcp_apps_sandbox_url: Some("https://apps.example.test/sandbox.html".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            ),
            runtime,
            module_runtime: None,
            contact_directory: None,
            event_log: None,
            gateway_peer_keys: None,
            identity_runtime: None,
            console_events: None,
            console_aggregator: None,
            mob_events: None,
            metadata_table: None,
            visibility_policy: Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
            snapshot_read_model: Default::default(),
            access: None,
            memory_panel: None,
            operator_resolver: None,
            identity_roster: None,
            workgraph: None,
            topology: None,
            job_health_projection: None,
            governed_ingress: Default::default(),
            mob_announcements: Default::default(),
        }
    }

    fn request(session_id: &meerkat_core::SessionId) -> Request {
        Request {
            identity: "review:apps".into(),
            session_id: session_id.to_string(),
            tool_call_id: "call-1".into(),
            uri: None,
            name: None,
            arguments: None,
        }
    }

    fn ingress(state: ConsoleJsonState, request: Request, operation: Operation) -> Ingress {
        Ingress {
            state,
            public_identity: request.identity.clone(),
            registration: None,
            request,
            operation,
            headers: HeaderMap::new(),
            uri: Uri::from_static("/console/mcp-apps/call-tool"),
            principal: None,
        }
    }

    #[test]
    fn request_cannot_supply_a_result_or_runtime_authority() {
        let value = json!({ "identity": "review:apps", "sessionId": meerkat_core::SessionId::new(),
            "toolCallId": "call-1", "result": { "_meta": { "ui": {} } } });
        assert!(serde_json::from_value::<Request>(value).is_err());
    }

    #[test]
    fn effective_realm_config_gates_every_app_operation() -> Result<(), Box<dyn std::error::Error>>
    {
        let config = r#"
mcp_apps_sandbox_url = "https://apps.example.test/sandbox.html"
[realms.disabled]
mcp_apps_sandbox_url = " "
"#;
        let request = request(&meerkat_core::SessionId::new());
        for (realm, enabled) in [(None, true), (Some("disabled"), false)] {
            let mut state = state(None);
            state.decisions.console.ui =
                crate::console_config::load_console_ui_config_from_toml_for_realm(config, realm)?;
            for operation in [
                Operation::Resolve,
                Operation::ReadResource,
                Operation::CallTool,
            ] {
                assert_eq!(
                    ingress(state.clone(), request.clone(), operation)
                        .revalidate()
                        .is_ok(),
                    enabled,
                    "effective realm config must gate every HTTP app operation"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn view_only_grants_do_not_authorize_fresh_member_resource_reads()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = request(&meerkat_core::SessionId::new());
        for allow_send in [false, true] {
            let mut state = state(None);
            let mut actions = vec![ACTION_AGENT_VIEW.to_string()];
            if allow_send {
                actions.push(ACTION_AGENT_SEND.to_string());
            }
            state.access = Some(AccessController::new(crate::access::AccessControlConfig {
                enabled: true,
                admins: vec!["admin@example.test".into()],
                // This explicitly grants an anonymous caller on an open Console.
                rules: vec![crate::access::AccessRule {
                    id: "open-console-viewer".into(),
                    actions,
                    ..Default::default()
                }],
                ..Default::default()
            })?);
            assert!(
                ingress(state.clone(), request.clone(), Operation::Resolve)
                    .revalidate()
                    .is_ok()
            );
            for operation in [Operation::ReadResource, Operation::CallTool] {
                assert_eq!(
                    ingress(state.clone(), request.clone(), operation)
                        .revalidate()
                        .is_ok(),
                    allow_send
                );
                let mut read_only = state.clone();
                read_only.decisions.console.read_only = true;
                assert!(
                    ingress(read_only, request.clone(), operation)
                        .revalidate()
                        .is_err()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn cached_reads_select_only_the_original_declared_renderer()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut invocation: McpAppInvocation = serde_json::from_value(json!({
            "registration": {"server": "fixture", "connection": "original"},
            "tool": {"name": "view", "inputSchema": {},
                "_meta": {"ui": {"resourceUri": "ui://fixture/view"}}},
            "arguments": {}, "result": {"content": []},
            "resource": {"contents": [{"uri": "ui://fixture/view",
                "mimeType": "text/html;profile=mcp-app", "text": "<p>Original view</p>"}]}
        }))?;
        let original = serde_json::to_value(invocation.resource.as_ref())?;
        assert_eq!(
            cached_renderer_resource(&invocation, Some("ui://fixture/view")),
            Ok(Some(original))
        );
        for uri in [
            None,
            Some("ui://fixture/other"),
            Some("private://member/secret"),
        ] {
            assert_eq!(cached_renderer_resource(&invocation, uri), Ok(None));
        }
        invocation.resource = None;
        assert_eq!(
            cached_renderer_resource(&invocation, Some("ui://fixture/view")),
            Ok(None),
            "a missing retained resource requires fresh IO admission"
        );
        Ok(())
    }

    #[test]
    fn read_only_and_current_access_policy_reject_actions() {
        let mut state = state(None);
        state.decisions.console.read_only = true;
        let request = request(&meerkat_core::SessionId::new());
        assert!(
            ingress(state.clone(), request.clone(), Operation::CallTool)
                .revalidate()
                .is_err()
        );
        assert!(
            ingress(state.clone(), request.clone(), Operation::Resolve)
                .revalidate()
                .is_ok()
        );
        state.access = Some(
            AccessController::new(crate::access::AccessControlConfig {
                enabled: true,
                admins: vec!["admin@example.test".to_string()],
                ..Default::default()
            })
            .expect("valid test access controller"),
        );
        assert!(
            ingress(state, request, Operation::Resolve)
                .revalidate()
                .is_err()
        );
    }

    #[tokio::test]
    async fn native_session_association_is_rechecked_after_admission()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use super::super::tests::{
            build_empty_console_test_runtime, empty_identity_control_test_runtime,
            register_identity_control_test_binding, spawn_identity_control_test_member,
        };
        let (_directory, runtime) = build_empty_console_test_runtime("mcp-app-custody").await?;
        let member = "rt:review:apps:0";
        spawn_identity_control_test_member(&runtime, member, "review:apps").await?;
        let session = runtime
            .handle()
            .resolve_bridge_session_id_observation(&crate::member_comms_id::mob_member_id(member))
            .await
            .expect("native member session");
        let owner = empty_identity_control_test_runtime("mcp-app-custody")?;
        register_identity_control_test_binding(&owner, "review:apps", member, session.clone())
            .await?;
        let mut state = state(Some(runtime.clone()));
        state.identity_runtime = Some(crate::identity_first::PublicIdentityRuntime::new(
            owner.clone(),
            runtime,
        ));
        let receipt = ingress(state.clone(), request(&session), Operation::CallTool);
        assert!(receipt.revalidate_async().await.is_ok());
        assert!(
            authorize_session(
                &state,
                &request(&meerkat_core::SessionId::new()),
                Operation::Resolve
            )
            .await
            .is_err()
        );
        // The actor must refuse a receipt whose identity has moved to another
        // session, even while its old runtime member remains observable.
        register_identity_control_test_binding(
            &owner,
            "review:apps",
            member,
            meerkat_core::SessionId::new(),
        )
        .await?;
        assert!(receipt.revalidate_async().await.is_err());
        // Retained canonical ownership (Core's history still binds the old
        // session to the identity) permits historical display after the
        // identity rotates, while the old actor cannot authorize new IO.
        assert!(
            authorize_session(&state, &request(&session), Operation::Resolve)
                .await
                .is_ok()
        );
        assert!(
            authorize_session(&state, &request(&session), Operation::ReadResource)
                .await
                .is_err()
        );
        Ok(())
    }

    /// An identity-first member's app reads authorize on the session Core
    /// binds it to, over the mob's own bridge rather than a scripted double:
    /// the live session reads and calls tools, a session Core never bound is
    /// refused, and after a reset the old session still resolves through
    /// Core's history while only the successor's session reads.
    #[tokio::test]
    async fn identity_first_app_reads_authorize_on_cores_session_over_the_mob_bridge()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use super::super::tests::build_empty_console_test_runtime;
        let (_directory, runtime) = build_empty_console_test_runtime("mcp-app-core").await?;
        let owner = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "mcp-app-core".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(crate::identity_first::MobSessionBridge::new(
                    runtime.handle(),
                ))),
                default_timeout: None,
            },
        ));
        let identity = crate::identity_first::AgentIdentity::parse("review:apps")?;
        owner
            .register(crate::identity_first::DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: std::collections::BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;
        let scope = owner
            .materialize(&identity)
            .await?
            .ok_or("Core holds review:apps live after the wake")?;
        let live = scope.session_id().clone();
        let mut state = state(Some(runtime.clone()));
        state.identity_runtime = Some(crate::identity_first::PublicIdentityRuntime::new(
            owner.clone(),
            runtime.clone(),
        ));
        for (operation, name) in [
            (Operation::ReadResource, "read_resource"),
            (Operation::CallTool, "call_tool"),
        ] {
            assert!(
                authorize_session(&state, &request(&live), operation)
                    .await
                    .is_ok(),
                "Core's live session must authorize {name}"
            );
        }
        let unbound = meerkat_core::SessionId::new();
        for (operation, name) in [
            (Operation::Resolve, "resolve"),
            (Operation::ReadResource, "read_resource"),
        ] {
            assert!(
                authorize_session(&state, &request(&unbound), operation)
                    .await
                    .is_err(),
                "a session Core never bound must refuse {name}"
            );
        }

        let successor = owner.reset(&identity).await?;
        let successor = successor
            .session_id()
            .cloned()
            .ok_or("a reset binds the identity to a fresh session")?;
        assert_ne!(successor, live);
        assert!(
            authorize_session(&state, &request(&live), Operation::ReadResource)
                .await
                .is_err(),
            "the session the reset retired must not read"
        );
        assert!(
            authorize_session(&state, &request(&live), Operation::Resolve)
                .await
                .is_ok(),
            "Core's history still owns the retired session for display"
        );
        assert!(
            authorize_session(&state, &request(&successor), Operation::ReadResource)
                .await
                .is_ok(),
            "the successor's session must read"
        );
        Ok(())
    }

    /// Authorizing an app call for a live identity-first member records the
    /// member's access value once, as the roster priming records it, so a
    /// repeated authorization publishes no access change.
    #[tokio::test]
    async fn app_authorization_records_a_live_member_once()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use super::super::tests::build_empty_console_test_runtime;
        let (_directory, runtime) = build_empty_console_test_runtime("mcp-app-priming").await?;
        let owner = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "mcp-app-priming".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(crate::identity_first::MobSessionBridge::new(
                    runtime.handle(),
                ))),
                default_timeout: None,
            },
        ));
        let identity = crate::identity_first::AgentIdentity::parse("review:apps")?;
        owner
            .register(crate::identity_first::DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: std::collections::BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;
        let scope = owner
            .materialize(&identity)
            .await?
            .ok_or("Core holds review:apps live after the wake")?;
        let live = scope.session_id().clone();
        let mut state = state(Some(runtime.clone()));
        state.identity_runtime = Some(crate::identity_first::PublicIdentityRuntime::new(
            owner.clone(),
            runtime.clone(),
        ));
        let controller = AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec!["admin@example.test".into()],
            ..Default::default()
        })?;
        let publication = meerkat_authorization::publication::LocalAuthorizationPublication::new();
        controller
            .join_authorization_publication(publication.clone())
            .map_err(|_| "a fresh controller joins its first publication")?;
        state.access = Some(controller);
        let _first = authorize_session(&state, &request(&live), Operation::ReadResource).await;
        let quiet = publication
            .observe(|| ())
            .map_err(|_| "a quiet publication")?
            .1;
        for _ in 0..2 {
            let _again = authorize_session(&state, &request(&live), Operation::ReadResource).await;
        }
        assert!(
            quiet.check_current().is_ok(),
            "repeated app authorizations of a live member publish no access change"
        );
        Ok(())
    }

    #[tokio::test]
    async fn aggregate_route_uses_native_session_and_rejects_replaced_registration()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use super::super::tests::{
            build_empty_console_test_runtime, spawn_identity_control_test_member,
        };
        let (_directory, runtime) = build_empty_console_test_runtime("mcp-app-aggregate").await?;
        let member = "rt:review:apps:0";
        spawn_identity_control_test_member(&runtime, member, "review:apps").await?;
        let session = runtime
            .handle()
            .resolve_bridge_session_id_observation(&crate::member_comms_id::mob_member_id(member))
            .await
            .expect("native member session");
        let aggregator = MetaConsoleAggregator::in_memory();
        let register = || {
            aggregator.register_runtime_handles_with_policy(
                "runtime",
                "project",
                runtime.clone(),
                None,
                ConsoleEventStore::new(),
                Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
            );
        };
        register();
        let mut state = state(None);
        state.console_aggregator = Some(aggregator);
        let mut request = request(&session);
        request.identity = "project/review:apps".into();
        let public_identity = request.identity.clone();
        let (state, request, registration) = route(state, request, Operation::CallTool)
            .await
            .map_err(|()| "route refused")?;
        assert_eq!(request.identity, "review:apps");
        let mut receipt = ingress(state, request, Operation::CallTool);
        receipt.public_identity = public_identity;
        receipt.registration = registration;
        assert!(receipt.revalidate_async().await.is_ok());
        receipt
            .state
            .console_aggregator
            .as_ref()
            .expect("registered console aggregator")
            .register_runtime_handles_with_policy(
                "runtime",
                "project",
                runtime,
                None,
                ConsoleEventStore::new(),
                Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
            );
        assert!(receipt.revalidate_async().await.is_err());
        Ok(())
    }

    /// A candidate launch calls no member's tool: its call-tool answers 409
    /// `candidate_launch` before anything is resolved, while resolving an app
    /// stays open. An authoritative launch's call-tool is not refused so: it
    /// answers as its admission decides (here, unavailable).
    #[tokio::test]
    async fn a_candidate_launch_calls_no_app_tool()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use super::super::tests::build_empty_console_test_runtime_as;
        use crate::mob_composition_manifest::CompositionAuthority;

        let mut answers = Vec::new();
        for (label, authority) in [
            ("candidate", CompositionAuthority::NonAuthoritative),
            ("authoritative", CompositionAuthority::Authoritative),
        ] {
            let (_directory, runtime) =
                build_empty_console_test_runtime_as(&format!("mcp-app-{label}"), authority).await?;
            let request = request(&meerkat_core::SessionId::new());
            let call = handle(
                state(Some(runtime.clone())),
                HeaderMap::new(),
                Uri::from_static("/console/mcp-apps/call-tool"),
                request.clone(),
                Operation::CallTool,
            )
            .await;
            let call_status = call.status();
            let call_body: Value =
                serde_json::from_slice(&axum::body::to_bytes(call.into_body(), usize::MAX).await?)?;
            let resolve = handle(
                state(Some(runtime)),
                HeaderMap::new(),
                Uri::from_static("/console/mcp-apps/resolve"),
                request,
                Operation::Resolve,
            )
            .await;
            answers.push((call_status, call_body["error"].clone(), resolve.status()));
        }
        assert_eq!(
            answers,
            vec![
                (
                    StatusCode::CONFLICT,
                    json!("candidate_launch"),
                    StatusCode::FORBIDDEN
                ),
                (
                    StatusCode::FORBIDDEN,
                    json!("mcp_app_unavailable"),
                    StatusCode::FORBIDDEN
                ),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn unavailable_responses_are_not_cacheable() {
        let response = handle(
            state(None),
            HeaderMap::new(),
            Uri::from_static("/console/mcp-apps/resolve"),
            request(&meerkat_core::SessionId::new()),
            Operation::Resolve,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .expect("cache-control header"),
            "no-store"
        );
    }
}
