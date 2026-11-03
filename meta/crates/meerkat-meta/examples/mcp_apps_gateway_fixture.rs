//! Native MCP Apps fixture on the gateway's own member composition.
//!
//! The bundled `rpc_gateway` builds every member through its gateway member
//! factories, whose builder installs the meerkat facade's MCP credential
//! source and connector owner, and with it the stock connector call-context
//! provider on every member MCP router. `console_acceptance_fixture` builds
//! its runtime with `UnifiedRuntime::builder()`, which never installs that
//! provider, so an MCP Apps regression behind it went unseen there. This
//! fixture runs the persistent gateway itself
//! (`rpc_gateway::run_persistent_with_host`) and adds only what a browser
//! test needs:
//!
//! - the acceptance fixture's scripted model as the mob's default LLM
//!   client, set in `prepare_bootstrap`;
//! - that model's controls under `/host/__fixture` on the gateway listener.
//!
//! It leaves every member factory as the gateway constructs it. Drive it as
//! an SDK drives `rpc_gateway --persistent`: one `meta/init` request on
//! stdin, then JSON-RPC requests, one per line
//! (`console/mcp-apps-native-e2e.cjs`).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use axum::routing::{get, post};
use axum::{Json, Router};
use meerkat_meta::rpc_gateway::{
    GatewayBootstrapReady, GatewayHostCloseReason, GatewayHostComposition, GatewayHostError,
};
use meerkat_meta::{MobBootstrapSpec, UnifiedRuntime};
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;
use serde_json::{Value, json};

// The acceptance fixture's scripted model; this fixture uses its MCP Apps
// scenarios only.
#[allow(dead_code)]
#[path = "console_acceptance_support/scenario_client.rs"]
mod scenario_client;
use scenario_client::{ModelBarrier, ModelPlan, RecordingClient};

struct ScriptedModelHost {
    plan: Arc<Mutex<ModelPlan>>,
    requests: Arc<Mutex<Vec<Value>>>,
    barrier: Arc<ModelBarrier>,
}

#[async_trait::async_trait]
impl GatewayHostComposition for ScriptedModelHost {
    async fn prepare_bootstrap(
        &self,
        mut spec: MobBootstrapSpec,
    ) -> Result<MobBootstrapSpec, GatewayHostError> {
        spec.options.default_llm_client = Some(Arc::new(RecordingClient::new(
            Arc::clone(&self.plan),
            Arc::clone(&self.requests),
            Arc::clone(&self.barrier),
        )));
        Ok(spec)
    }

    fn native_grant_work_configuration(
        &self,
    ) -> Result<Option<NativeGrantWorkConfiguration>, GatewayHostError> {
        Ok(None)
    }

    async fn bind_runtime(&self, _runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        Ok(())
    }

    fn publish_bootstrap_ready(
        &self,
        publication: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
        publication.publish();
        Ok(())
    }

    fn begin_close(&self, _reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn join_close(&self) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn wait_closed(&self) {
        std::future::pending::<()>().await;
    }

    fn host_app_routes(
        &self,
        _viewer: meerkat_meta::http_console::ConsoleViewerResolver,
    ) -> Result<Option<Router>, GatewayHostError> {
        let plan = Arc::clone(&self.plan);
        let barrier = Arc::clone(&self.barrier);
        let requests = Arc::clone(&self.requests);
        Ok(Some(
            Router::new()
                .route(
                    "/__fixture/model",
                    post(move |Json(next): Json<ModelPlan>| async move {
                        *plan
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
                        Json(json!({"ok": true}))
                    }),
                )
                .route(
                    "/__fixture/model-barrier",
                    post(move |Json(command): Json<Value>| async move {
                        match barrier.control(&command) {
                            Ok(status) => (axum::http::StatusCode::OK, Json(status)),
                            Err(error) => (
                                axum::http::StatusCode::BAD_REQUEST,
                                Json(json!({"error": error})),
                            ),
                        }
                    }),
                )
                .route(
                    "/__fixture/requests",
                    get(move || async move {
                        Json(
                            requests
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .clone(),
                        )
                    }),
                ),
        ))
    }
}

fn main() {
    let runtime =
        meerkat_meta::gateway_composition::gateway_tokio_runtime().expect("gateway tokio runtime");
    let host = Arc::new(ScriptedModelHost {
        plan: Arc::new(Mutex::new(ModelPlan {
            source: "Ready.".into(),
            reasoning_blocks: Vec::new(),
            delay_ms: 0,
            chunk_chars: 64,
            scenario: None,
        })),
        requests: Arc::new(Mutex::new(Vec::new())),
        barrier: Arc::new(ModelBarrier::default()),
    });
    let result = runtime.block_on(meerkat_meta::rpc_gateway::run_persistent_with_host(
        None,
        host as Arc<dyn GatewayHostComposition>,
    ));
    runtime.shutdown_background();
    if let Err(error) = result {
        eprintln!("mcp apps gateway fixture: {error}");
        std::process::exit(1);
    }
}
