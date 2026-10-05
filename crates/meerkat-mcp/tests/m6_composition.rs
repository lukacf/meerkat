//! M6 successor: trusted configuration selects exposed names in ONE router.
//! Provider operations remain `search`. The two failing predecessor oracles,
//! including the unsupported sharded-owner topology, remain in immutable evidence.
//! These explicit alias-selected expectations are new successor tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "m6_composition_support/mod.rs"]
mod support;

use async_trait::async_trait;
use futures::FutureExt;
use meerkat_core::{
    AgentToolDispatcher, ExternalToolSurfaceSnapshot, McpServerConfig, ResolvedToolExecutionPlan,
    SessionId, SessionRuntimeBindings, ToolCallView, ToolDeadlineChain, ToolDeadlineContributor,
    ToolDeadlineOwner, ToolDef, ToolDispatchContext, ToolError, ToolExecutionResolutionContext,
    ToolExecutionResolutionError, dispatch_tool_execution_plan_fenced,
    resolve_tool_execution_plan_fenced,
};
use meerkat_mcp::{McpRouter, McpRouterAdapter};
use meerkat_runtime::{MeerkatMachine, session_runtime_bindings_have_machine_authority};
use serde_json::{Value, json, value::RawValue};
use std::{
    collections::HashMap,
    panic::AssertUnwindSafe,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use support::{Endpoint, Server};

const LIMIT: Duration = Duration::from_secs(10);

fn provider_tool() -> Arc<ToolDef> {
    Arc::new(ToolDef::new(
        "search",
        "The same provider operation for either selected account",
        json!({"type":"object","properties":{"value":{"type":"string"}}}),
    ))
}

fn config(name: &str, endpoint: &Endpoint, bearer: &str) -> McpServerConfig {
    let mut config = McpServerConfig::streamable_http(
        name,
        &endpoint.url,
        HashMap::from([("authorization".into(), bearer.into())]),
    );
    config.connect_timeout_secs = Some(5);
    config
}

fn named_config(name: &str, endpoint: &Endpoint, bearer: &str, exposed: &str) -> McpServerConfig {
    let mut config = config(name, endpoint, bearer);
    config.tool_names.insert("search".into(), exposed.into());
    config
}

fn adapter(bindings: &SessionRuntimeBindings) -> Arc<McpRouterAdapter> {
    assert!(session_runtime_bindings_have_machine_authority(bindings));
    let adapter = Arc::new(McpRouterAdapter::new(McpRouter::new_with_surface_handle(
        Arc::clone(bindings.external_tool_surface()),
    )));
    adapter.bind_mcp_server_lifecycle_handle(Arc::clone(bindings.mcp_server_lifecycle()));
    adapter
}

fn resolution() -> ToolExecutionResolutionContext {
    ToolExecutionResolutionContext::new(
        ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
            ToolDeadlineOwner::CoreToolDispatch,
            LIMIT,
        )])
        .unwrap(),
    )
}

fn arguments() -> Box<RawValue> {
    RawValue::from_string("{\"value\":\"unchanged-arguments\"}".into()).unwrap()
}

fn resolve(
    dispatcher: &Arc<dyn AgentToolDispatcher>,
    name: &str,
) -> Result<ResolvedToolExecutionPlan, ToolExecutionResolutionError> {
    let args = arguments();
    let plan = resolve_tool_execution_plan_fenced(
        dispatcher,
        ToolCallView {
            id: "m6-call",
            name,
            args: &args,
        },
        &ToolDispatchContext::default(),
        &resolution(),
    )?;
    assert!(
        plan.owner_witnesses()
            .iter()
            .any(|witness| { witness.authority_key().starts_with("mcp-router-adapter:") }),
        "every call must retain the native execution witness"
    );
    Ok(plan)
}

async fn dispatch_plan(
    dispatcher: &Arc<dyn AgentToolDispatcher>,
    name: &str,
    plan: &ResolvedToolExecutionPlan,
) -> Result<Value, ToolError> {
    let args = arguments();
    let result = dispatch_tool_execution_plan_fenced(
        dispatcher,
        ToolCallView {
            id: "m6-call",
            name,
            args: &args,
        },
        &ToolDispatchContext::default(),
        plan,
    )
    .await?;
    assert!(!result.result.is_error);
    Ok(serde_json::from_str(&result.result.text_content()).unwrap())
}

async fn call(dispatcher: &Arc<dyn AgentToolDispatcher>, name: &str) -> Value {
    dispatch_plan(dispatcher, name, &resolve(dispatcher, name).unwrap())
        .await
        .unwrap()
}

fn assert_receipt(value: &Value, account: &str) {
    assert_eq!(value["account"], account);
    assert_eq!(value["arguments"], json!({"value":"unchanged-arguments"}));
}

async fn add(adapter: &McpRouterAdapter, config: McpServerConfig) {
    adapter.stage_add(config).await.unwrap();
    let applied = adapter.apply_staged().await.unwrap();
    assert!(applied.delta.rejected_boundaries.is_empty());
    adapter.wait_until_ready(LIMIT).await.unwrap();
}

async fn await_deletes(endpoint: &Endpoint, expected: usize) {
    tokio::time::timeout(LIMIT, async {
        while endpoint.deletes() < expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("native close must reach the owned HTTP fixture");
}

fn aligned(snapshot: &ExternalToolSurfaceSnapshot) -> bool {
    snapshot.snapshot_epoch == snapshot.snapshot_aligned_epoch
}

fn name_for(dispatcher: &dyn AgentToolDispatcher, server: &str) -> Option<String> {
    let matching: Vec<_> = dispatcher
        .tool_catalog()
        .iter()
        .filter_map(|entry| {
            entry
                .tool
                .provenance
                .as_ref()
                .filter(|source| source.source_id.as_str() == server)
                .map(|_| entry.tool.name.to_string())
        })
        .collect();
    (matching.len() == 1).then(|| matching[0].clone())
}

#[derive(Default)]
struct Owned {
    adapters: Vec<Arc<McpRouterAdapter>>,
    endpoints: Vec<Endpoint>,
}

impl Owned {
    // Same custody pattern as the existing downstream realization tests. Every
    // fixture task is retained and each join is attempted even after a panic.
    async fn finish(mut self, body: std::thread::Result<()>) {
        let mut errors = vec![];
        let mut joins = vec![];
        for endpoint in &self.endpoints {
            endpoint.release_gates();
        }
        for adapter in &self.adapters {
            if AssertUnwindSafe(adapter.shutdown())
                .catch_unwind()
                .await
                .is_err()
            {
                errors.push("native shutdown panicked".to_string());
            }
        }
        for endpoint in &mut self.endpoints {
            let expected = endpoint.expected_joins();
            match AssertUnwindSafe(endpoint.shutdown()).catch_unwind().await {
                Ok((initialized, joined, failures)) => {
                    joins.push(json!({
                        "initialized":initialized,"expected":expected,"observed":joined,
                        "errors":failures
                    }));
                    if !initialized || joined != expected {
                        errors.push(format!(
                            "fixture initialization/join mismatch: {initialized} {joined:?}"
                        ));
                    }
                    errors.extend(failures);
                }
                Err(_) => errors.push("endpoint shutdown panicked".into()),
            }
        }
        // The expected acceptance panic is not an excuse to lose cleanup
        // evidence. Root must reject a counterexample unless this is true.
        eprintln!(
            "M6_CLEANUP {}",
            json!({
                "fixture_cleanup_passed":errors.is_empty(),
                "primary_body_passed":body.is_ok(),"endpoint_joins":joins,"errors":errors,
                "native_private_close_tasks_joined":false
            })
        );
        if let Err(primary) = body {
            for error in errors {
                eprintln!("M6 cleanup: {error}");
            }
            std::panic::resume_unwind(primary);
        }
        assert!(errors.is_empty(), "M6 cleanup failures: {errors:?}");
    }
}

#[tokio::test]
async fn explicit_names_keep_both_raw_identical_operations_on_one_aligned_router() {
    let mut owned = Owned::default();
    let body = AssertUnwindSafe(async {
        owned.endpoints.push(
            Endpoint::start_accounts(vec![
                (
                    Server::new("a", vec![provider_tool()]),
                    "Bearer m6-account-a",
                ),
                (
                    Server::new("b", vec![provider_tool()]),
                    "Bearer m6-account-b",
                ),
            ])
            .await
            .unwrap(),
        );
        let machine = MeerkatMachine::ephemeral();
        let session = SessionId::new();
        let bindings = machine.prepare_bindings(session.clone()).await.unwrap();
        assert_eq!(bindings.session_id(), &session);
        let native = adapter(&bindings);
        owned.adapters.push(Arc::clone(&native));
        let dispatcher: Arc<dyn AgentToolDispatcher> = native.clone();

        add(
            &native,
            named_config(
                "account-a",
                &owned.endpoints[0],
                "Bearer m6-account-a",
                "home_search",
            ),
        )
        .await;
        assert_receipt(&call(&dispatcher, "home_search").await, "a");
        let old_a = resolve(&dispatcher, "home_search").unwrap();
        add(
            &native,
            named_config(
                "account-b",
                &owned.endpoints[0],
                "Bearer m6-account-b",
                "work_search",
            ),
        )
        .await;

        assert!(native.tool_name_collisions().await.unwrap().is_empty());
        let simultaneous = native.external_tool_surface_snapshot().unwrap();
        assert_eq!(
            simultaneous
                .entries
                .iter()
                .filter(|entry| entry.visible)
                .count(),
            2
        );
        let a_name = name_for(dispatcher.as_ref(), "account-a");
        let b_name = name_for(dispatcher.as_ref(), "account-b");
        assert_eq!(a_name.as_deref(), Some("home_search"));
        assert_eq!(b_name.as_deref(), Some("work_search"));
        assert!(resolve(&dispatcher, "search").is_err());
        let independent = match (&a_name, &b_name) {
            (Some(a), Some(b)) if a != b => {
                assert_receipt(&call(&dispatcher, a).await, "a");
                assert_receipt(&call(&dispatcher, b).await, "b");
                true
            }
            _ => false,
        };
        // Every native boundary retains its execution-plan fencing obligation.
        assert!(
            dispatch_plan(&dispatcher, "home_search", &old_a)
                .await
                .is_err()
        );

        native.stage_remove("account-a").await.unwrap();
        let removed = native.apply_staged().await.unwrap();
        assert_eq!(removed.delta.removed_servers, ["account-a"]);
        assert!(removed.delta.rejected_boundaries.is_empty());
        assert!(removed.delta.degraded_removals.is_empty());
        assert_receipt(&call(&dispatcher, "work_search").await, "b");
        assert!(
            dispatch_plan(&dispatcher, "home_search", &old_a)
                .await
                .is_err()
        );
        assert_eq!(
            name_for(dispatcher.as_ref(), "account-b").as_deref(),
            Some("work_search")
        );
        assert!(resolve(&dispatcher, "search").is_err());
        let old_b = resolve(&dispatcher, "work_search").unwrap();
        let reloaded_server = Server::new("b", vec![provider_tool()]);
        let reloaded_calls = Arc::clone(&reloaded_server.calls);
        owned.endpoints.push(
            Endpoint::start(reloaded_server, Some("Bearer m6-account-b"), None)
                .await
                .unwrap(),
        );
        native
            .stage_reload(named_config(
                "account-b",
                &owned.endpoints[1],
                "Bearer m6-account-b",
                "work_search",
            ))
            .await
            .unwrap();
        let reloaded = native.apply_staged().await.unwrap();
        assert!(reloaded.delta.rejected_boundaries.is_empty());
        native.wait_until_ready(LIMIT).await.unwrap();
        assert!(
            dispatch_plan(&dispatcher, "work_search", &old_b)
                .await
                .is_err()
        );
        assert_eq!(reloaded_calls.load(Ordering::SeqCst), 0);
        assert_receipt(&call(&dispatcher, "work_search").await, "b");
        assert_eq!(reloaded_calls.load(Ordering::SeqCst), 1);
        assert!(aligned(&native.external_tool_surface_snapshot().unwrap()));
        await_deletes(&owned.endpoints[0], 2).await;
        assert!(owned.endpoints[0].requests() > 0);
        eprintln!(
            "M6_EXPLICIT_SINGLE_ROUTER {}",
            json!({
                "reached_acceptance_oracle":true,
                "a_name":a_name,"b_name":b_name,"independent_dispatch":independent,
                "two_active_snapshot_aligned":aligned(&simultaneous),
                "remove_survivor_dispatch":true,"reload_stale_plan_refused":true
            })
        );
        // An excluded route or unsettled shared snapshot is not acceptance.
        assert!(
            independent && aligned(&simultaneous),
            "M6 requires both raw-identical account operations in one aligned native session"
        );
    })
    .catch_unwind()
    .await;
    owned.finish(body).await;
}

#[tokio::test]
async fn unmapped_name_is_unchanged_and_explicit_rename_fences_old_plans() {
    let mut owned = Owned::default();
    let body = AssertUnwindSafe(async {
        let server = Server::new("a", vec![provider_tool()]);
        let calls = Arc::clone(&server.calls);
        owned
            .endpoints
            .push(Endpoint::start(server, None, None).await.unwrap());
        let machine = MeerkatMachine::ephemeral();
        let bindings = machine.prepare_bindings(SessionId::new()).await.unwrap();
        let native = adapter(&bindings);
        owned.adapters.push(Arc::clone(&native));
        let dispatcher: Arc<dyn AgentToolDispatcher> = native.clone();
        // Legacy single-account configuration is byte-for-byte raw at exposure.
        add(
            &native,
            McpServerConfig::streamable_http("stable", &owned.endpoints[0].url, HashMap::new()),
        )
        .await;
        assert_eq!(
            name_for(dispatcher.as_ref(), "stable").as_deref(),
            Some("search")
        );
        assert_receipt(&call(&dispatcher, "search").await, "a");
        let raw_plan = resolve(&dispatcher, "search").unwrap();
        let mut selected =
            McpServerConfig::streamable_http("stable", &owned.endpoints[0].url, HashMap::new());
        selected
            .tool_names
            .insert("search".into(), "home_search".into());
        native.stage_reload(selected.clone()).await.unwrap();
        native.apply_staged().await.unwrap();
        native.wait_until_ready(LIMIT).await.unwrap();
        assert!(resolve(&dispatcher, "search").is_err());
        assert!(
            dispatch_plan(&dispatcher, "search", &raw_plan)
                .await
                .is_err()
        );
        let named_plan = resolve(&dispatcher, "home_search").unwrap();
        assert!(
            named_plan
                .owner_witnesses()
                .iter()
                .any(|w| w.owner_key() == "home_search")
        );
        assert_receipt(&call(&dispatcher, "home_search").await, "a");

        // Named reload reuses the exact map; explicit config reload can rename it.
        native.stage_reload("stable").await.unwrap();
        native.apply_staged().await.unwrap();
        native.wait_until_ready(LIMIT).await.unwrap();
        assert_eq!(
            name_for(dispatcher.as_ref(), "stable").as_deref(),
            Some("home_search")
        );
        assert!(
            dispatch_plan(&dispatcher, "home_search", &named_plan)
                .await
                .is_err()
        );
        let before_rename = resolve(&dispatcher, "home_search").unwrap();
        selected
            .tool_names
            .insert("search".into(), "family_search".into());
        native.stage_reload(selected).await.unwrap();
        native.apply_staged().await.unwrap();
        native.wait_until_ready(LIMIT).await.unwrap();
        let before_refused_call = calls.load(Ordering::SeqCst);
        assert!(resolve(&dispatcher, "home_search").is_err());
        assert!(
            dispatch_plan(&dispatcher, "home_search", &before_rename)
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), before_refused_call);
        assert_receipt(&call(&dispatcher, "family_search").await, "a");
        assert_eq!(calls.load(Ordering::SeqCst), before_refused_call + 1);
        assert!(aligned(&native.external_tool_surface_snapshot().unwrap()));
        await_deletes(&owned.endpoints[0], 3).await;
    })
    .catch_unwind()
    .await;
    owned.finish(body).await;
}

/// A peer dispatcher with builtin provenance exercises the existing gateway's
/// collision/precedence owner. It is not a second MCP routing owner.
struct BuiltinPeer;

#[async_trait]
impl AgentToolDispatcher for BuiltinPeer {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut tool = ToolDef::new("local_status", "builtin peer", json!({"type":"object"}));
        tool.provenance = Some(meerkat_core::ToolProvenance {
            kind: meerkat_core::ToolSourceKind::Builtin,
            source_id: "builtin-fixture".into(),
        });
        Arc::from([Arc::new(tool)])
    }
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        Ok(meerkat_core::ToolResult::new(call.id.into(), "builtin-owned".into(), false).into())
    }
}

#[tokio::test]
async fn aliases_obey_existing_gateway_collision_and_builtin_precedence() {
    use meerkat_core::gateway::{DynamicToolComposite, ToolGateway};
    let mut owned = Owned::default();
    let body = AssertUnwindSafe(async {
        let server = Server::new("a", vec![provider_tool()]);
        let calls = Arc::clone(&server.calls);
        owned
            .endpoints
            .push(Endpoint::start(server, None, None).await.unwrap());
        let machine = MeerkatMachine::ephemeral();
        let bindings = machine.prepare_bindings(SessionId::new()).await.unwrap();
        let native = adapter(&bindings);
        owned.adapters.push(Arc::clone(&native));
        let builtin: Arc<dyn AgentToolDispatcher> = Arc::new(BuiltinPeer);
        let mcp: Arc<dyn AgentToolDispatcher> = native.clone();
        // Build-known ownership is the existing gateway's decision.
        let gateway = ToolGateway::new(Arc::clone(&builtin), Some(Arc::clone(&mcp))).unwrap();
        let mut selected =
            McpServerConfig::streamable_http("mapped", &owned.endpoints[0].url, HashMap::new());
        selected
            .tool_names
            .insert("search".into(), "local_status".into());
        add(&native, selected).await;
        let provenance = native.tools()[0].provenance.clone().unwrap();
        assert_eq!(provenance.kind, meerkat_core::ToolSourceKind::Mcp);
        assert_eq!(provenance.source_id.as_str(), "mapped");
        assert!(ToolGateway::new(Arc::clone(&builtin), Some(Arc::clone(&mcp))).is_err());
        let args = arguments();
        let result = gateway
            .dispatch(ToolCallView {
                id: "builtin-call",
                name: "local_status",
                args: &args,
            })
            .await
            .unwrap();
        assert_eq!(result.result.text_content(), "builtin-owned");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let composite: Arc<dyn AgentToolDispatcher> =
            Arc::new(DynamicToolComposite::new(vec![builtin, mcp]));
        assert!(composite.tools().is_empty());
        assert!(resolve(&composite, "local_status").is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    })
    .catch_unwind()
    .await;
    owned.finish(body).await;
}

#[tokio::test]
async fn unconfigured_collision_reports_raw_pairs_until_explicit_reload_resolves_it() {
    use meerkat_mcp::{McpToolNameCollision, McpToolRoute};
    let mut owned = Owned::default();
    let body = AssertUnwindSafe(async {
        owned.endpoints.push(
            Endpoint::start_accounts(vec![
                (
                    Server::new("a", vec![provider_tool()]),
                    "Bearer m6-account-a",
                ),
                (
                    Server::new("b", vec![provider_tool()]),
                    "Bearer m6-account-b",
                ),
            ])
            .await
            .unwrap(),
        );
        let machine = MeerkatMachine::ephemeral();
        let bindings = machine.prepare_bindings(SessionId::new()).await.unwrap();
        let native = adapter(&bindings);
        owned.adapters.push(Arc::clone(&native));
        let dispatcher: Arc<dyn AgentToolDispatcher> = native.clone();
        add(
            &native,
            config("account-a", &owned.endpoints[0], "Bearer m6-account-a"),
        )
        .await;
        add(
            &native,
            config("account-b", &owned.endpoints[0], "Bearer m6-account-b"),
        )
        .await;
        let collisions = native.tool_name_collisions().await.unwrap();
        assert_eq!(
            collisions.as_ref(),
            &[McpToolNameCollision {
                exposed_name: "search".into(),
                routes: vec![
                    McpToolRoute {
                        server_name: "account-a".into(),
                        raw_operation: "search".into()
                    },
                    McpToolRoute {
                        server_name: "account-b".into(),
                        raw_operation: "search".into()
                    },
                ],
            }]
        );
        assert!(collisions[0].to_string().contains("tool_names"));
        assert!(resolve(&dispatcher, "search").is_err());
        assert!(!aligned(&native.external_tool_surface_snapshot().unwrap()));
        native
            .stage_reload(named_config(
                "account-a",
                &owned.endpoints[0],
                "Bearer m6-account-a",
                "home_search",
            ))
            .await
            .unwrap();
        native.apply_staged().await.unwrap();
        native.wait_until_ready(LIMIT).await.unwrap();
        assert!(native.tool_name_collisions().await.unwrap().is_empty());
        assert!(aligned(&native.external_tool_surface_snapshot().unwrap()));
        assert_receipt(&call(&dispatcher, "home_search").await, "a");
        assert_receipt(&call(&dispatcher, "search").await, "b");
        // Historical diagnostic remains immutable while the live projection changes.
        assert_eq!(collisions[0].routes.len(), 2);
        await_deletes(&owned.endpoints[0], 1).await;
    })
    .catch_unwind()
    .await;
    owned.finish(body).await;
}
