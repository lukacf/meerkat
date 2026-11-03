//! Public tool-result and original-custody controls using inert local resources.
//! These cover result shapes, not installed root transport or native readiness.
use super::*;
use crate::{
    protected_parent::selected_input::{SelectedInput, digest, inert_test_input},
    resources::LocalRoundOwner,
};
use meerkat_core::{McpServerConfig, SessionId, mcp_config::McpTransportConfig};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

fn composition(
    eager: bool,
) -> (
    LazySetup,
    ServicePreparationPresenter,
    Arc<private_attempt::PrivateCustody>,
) {
    let (router, custody) = private_attempt::PrivateCustody::new(
        McpRouter::new_with_stdio_profile(McpStdioLaunchProfile::trusted_host()),
        SessionId::new(),
    );
    let custody = Arc::new(custody);
    let (paired_host, hosts) = PairedHostFrontend::deferred();
    drop(hosts);
    // Use the same composition entry and eager-state argument as PrivateNativeSetup.
    let (setup, presenter) = LazySetup::compose_with_adapter(
        paired_host,
        false,
        true,
        false,
        false,
        true,
        Arc::new(McpRouterAdapter::new(router)),
        None,
        None,
        None,
        eager,
    );
    assert!(setup.owner.private_custody.set(custody.clone()).is_ok());
    (setup, presenter, custody)
}

fn connection(server: &McpServerConfig) -> PreparedSetupConnection {
    let McpTransportConfig::Stdio(transport) = &server.transport else {
        panic!("stdio fixture")
    };
    PreparedSetupConnection::from_preparer_output(
        &serde_json::to_vec(&json!({
            "mcpServers":{"toolkit-setup":{"command":transport.command,"args":transport.args}}
        }))
        .unwrap(),
    )
    .unwrap()
}

fn profile_path(server: &McpServerConfig) -> PathBuf {
    let McpTransportConfig::Stdio(transport) = &server.transport else {
        panic!("stdio fixture")
    };
    let flag = transport
        .args
        .iter()
        .position(|v| v == "--configuration")
        .unwrap();
    PathBuf::from(&transport.args[flag + 1])
        .parent()
        .unwrap()
        .join("selected-install.json")
}

async fn call(setup: &LazySetup) -> Result<Value, ToolError> {
    let args = serde_json::value::to_raw_value(&json!({})).unwrap();
    let result = setup
        .dispatcher()
        .dispatch(ToolCallView {
            id: "selected-input-result",
            name: TOOLKIT_PREPARE_SERVICES,
            args: &args,
        })
        .await?;
    Ok(serde_json::from_str(&result.result.text_content()).unwrap())
}

fn assert_projection(value: &Value, changed: bool, input: &SelectedInput) {
    assert_eq!(value["state"], "requested");
    assert_eq!(value["changed"], changed);
    let selected = value["selected_input"]
        .as_object()
        .expect("completed original copy");
    let fields: BTreeSet<_> = selected.keys().map(String::as_str).collect();
    assert_eq!(
        fields,
        BTreeSet::from([
            "service_id",
            "staged_id",
            "profile",
            "component",
            "package_sha256",
            "selection_sha256",
            "bundle_sha256"
        ])
    );
    assert_eq!(selected["service_id"], "repository");
    assert_eq!(selected["profile"], "python");
    assert_eq!(selected["component"], "selected-component");
    assert_eq!(selected["package_sha256"], digest(b"{}"));
    assert_eq!(selected["selection_sha256"], digest(b"{}"));
    assert_eq!(selected["bundle_sha256"], input.bundle().sha256);
    let id = selected["staged_id"].as_str().unwrap();
    assert!(id.starts_with("input-") && id.len() == 38);
    assert!(id[6..].bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(
        selected.values().all(|v| v
            .as_str()
            .is_some_and(|s| !s.contains('/') && !s.contains('\\'))),
        "public result must not carry a filesystem path"
    );
}

async fn close_fixture(setup: &LazySetup, custody: &private_attempt::PrivateCustody) {
    // The inert command is intentionally not an installed MCP server. A native
    // refusal is permissible, but its original close must complete and fence.
    let _outcome =
        tokio::time::timeout(Duration::from_secs(30), custody.close(&setup.owner.adapter))
            .await
            .expect("same fixture custody must finish closing");
    assert!(custody.staged_input().is_none());
}

#[tokio::test]
async fn lazy_prepare_results_project_only_completed_selected_input() {
    let (_files, owner, server) = LocalRoundOwner::selected_input_test_preparation();
    let (setup, mut presenter, custody) = composition(false);
    let input = inert_test_input("repository");
    let prepared = connection(&server);
    let mut result = Box::pin(call(&setup));
    assert!(futures_util::poll!(result.as_mut()).is_pending());
    let mut request = presenter.next().await.unwrap();
    assert!(custody.staged_input().is_none());
    custody.retain(server, owner).unwrap();
    custody
        .stage_selected(input.clone(), std::future::pending())
        .await
        .unwrap();
    // Copy completion alone does not finish the pending tool request.
    assert!(futures_util::poll!(result.as_mut()).is_pending());
    let (first, handoff) = tokio::join!(result, request.handoff(prepared));
    handoff.unwrap();
    let first = first.unwrap();
    assert_projection(&first, true, &input);
    let repeated = call(&setup).await.unwrap();
    assert_projection(&repeated, false, &input);
    assert_eq!(first["selected_input"], repeated["selected_input"]);
    assert!(
        presenter.requests.try_recv().is_err(),
        "repeat request must not prepare again"
    );
    close_fixture(&setup, &custody).await;
    let fenced = call(&setup).await.unwrap();
    assert_eq!(fenced["changed"], false);
    assert!(fenced.get("selected_input").is_none());
}

#[test]
fn eager_prepare_result_projects_only_after_copy_and_never_after_fence() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_files, owner, server) = LocalRoundOwner::selected_input_test_preparation();
        let (setup, mut presenter, custody) = composition(true);
        let before = call(&setup).await.unwrap();
        assert_eq!(before["changed"], false);
        assert!(before.get("selected_input").is_none());
        let input = inert_test_input("repository");
        custody.retain(server, owner).unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx.await.unwrap();
        let mut copy = Box::pin(custody.stage_selected(input.clone(), std::future::pending()));
        let pending = futures_util::poll!(copy.as_mut()).is_pending();
        let during = call(&setup).await;
        // Release before assertions so a failed assertion cannot leave the
        // test runtime waiting forever for its inert blocker.
        release_tx.send(()).unwrap();
        blocker.await.unwrap();
        assert!(pending);
        let during = during.unwrap();
        assert_eq!(during["changed"], false);
        assert!(during.get("selected_input").is_none());
        copy.await.unwrap();
        assert_projection(&call(&setup).await.unwrap(), false, &input);
        assert!(
            presenter.requests.try_recv().is_err(),
            "eager result must not prepare again"
        );
        close_fixture(&setup, &custody).await;
        let fenced = call(&setup).await.unwrap();
        assert_eq!(fenced["changed"], false);
        assert!(fenced.get("selected_input").is_none());
    });
}

#[tokio::test]
async fn prepare_result_never_projects_failed_or_cancelled_copy() {
    for cancel in [false, true] {
        let (_files, owner, server) = LocalRoundOwner::selected_input_test_preparation();
        let (setup, _presenter, custody) = composition(true);
        if !cancel {
            std::fs::write(profile_path(&server), b"{}").unwrap();
        }
        custody.retain(server, owner).unwrap();
        let input = inert_test_input("repository");
        let interrupted = async {
            if cancel {
                Ok(())
            } else {
                std::future::pending().await
            }
        };
        assert!(
            custody
                .stage_selected(input.clone(), interrupted)
                .await
                .is_err()
        );
        assert!(input.failed_for_test());
        assert!(input.begin().is_err());
        let result = call(&setup).await.unwrap();
        assert_eq!(result["changed"], false);
        assert!(result.get("selected_input").is_none());
        close_fixture(&setup, &custody).await;
        assert!(call(&setup).await.unwrap().get("selected_input").is_none());
    }
}

#[tokio::test]
async fn lazy_prepare_failure_or_cancellation_never_returns_a_selected_result() {
    for cancel in [false, true] {
        let (_files, owner, server) = LocalRoundOwner::selected_input_test_preparation();
        let (setup, mut presenter, custody) = composition(false);
        let mut result = Box::pin(call(&setup));
        assert!(futures_util::poll!(result.as_mut()).is_pending());
        let request = presenter.next().await.unwrap();
        if !cancel {
            std::fs::write(profile_path(&server), b"{}").unwrap();
        }
        custody.retain(server, owner).unwrap();
        let input = inert_test_input("repository");
        let interrupted = async {
            if cancel {
                Ok(())
            } else {
                std::future::pending().await
            }
        };
        assert!(
            custody
                .stage_selected(input.clone(), interrupted)
                .await
                .is_err()
        );
        drop(request);
        assert!(result.await.is_err());
        assert!(input.failed_for_test());
        assert!(custody.staged_input().is_none());
        close_fixture(&setup, &custody).await;
    }
}
