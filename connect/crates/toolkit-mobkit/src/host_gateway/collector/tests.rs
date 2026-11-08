//! Physical-custody controls over real native MCP routers. These do not fake a
//! GatewayFactoryConstruction, collector account or ServiceCall audit sink.
//! Full public occurrence/provider acceptance also requires selection of
//! Core0768728's native audit owner and a forwarding Meta runtime store.
use super::*;
use futures::FutureExt;
use std::collections::BTreeSet;

const WATCHDOG: Duration = Duration::from_secs(15);
const SERVER: &str = r#"
import json, pathlib, sys, time
mode, entered, calls = sys.argv[1:]
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    if 'id' not in request:
        continue
    if method == 'initialize':
        pathlib.Path(entered).write_text('initialized')
        if mode == 'hold':
            while True:
                time.sleep(60)
        result = {'protocolVersion': request['params']['protocolVersion'],
                  'capabilities': {'tools': {}},
                  'serverInfo': {'name': 'collector-custody-control', 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': [] if mode == 'empty' else [
            {'name': 'get_profile', 'description': 'controlled read',
             'inputSchema': {'type': 'object', 'properties': {}}}]}
    elif method == 'tools/call':
        with open(calls, 'a') as output:
            output.write(request['params']['name'] + '\n')
        result = {'content': [{'type': 'text', 'text': 'controlled native reply'}]}
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

fn adapter() -> Arc<McpRouterAdapter> {
    Arc::new(McpRouterAdapter::new(
        meerkat::mcp::standalone_router_with_stdio_profile(McpStdioLaunchProfile::trusted_host())
            .with_surface_ownership(meerkat_mcp::McpSurfaceOwnership::Owned),
    ))
}

pub(crate) fn configuration(
    directory: &std::path::Path,
    mode: &str,
) -> meerkat_core::McpServerConfig {
    let python = std::path::PathBuf::from(
        std::env::var_os("TOOLKIT_SHARED_TEST_PYTHON")
            .expect("explicit selected Python; these controls use its standard library only"),
    );
    assert!(python.is_absolute() && python.is_file());
    let (mut config, _, _) = crate::private_originals::collector_setup::tests::fixture();
    // The ready-mode server advertises only get_profile. Core refuses a
    // connector that declares any raw operation absent from tools/list.
    config
        .connector
        .as_mut()
        .unwrap()
        .operations
        .retain(|operation, _| operation == "get_profile");
    let McpTransportConfig::Stdio(stdio) = &mut config.transport else {
        unreachable!()
    };
    stdio.command = python
        .to_str()
        .expect("selected Python path is UTF-8")
        .into();
    stdio.args = vec![
        "-u".into(),
        "-c".into(),
        SERVER.into(),
        mode.into(),
        directory.join("entered").to_string_lossy().into_owned(),
        directory.join("calls").to_string_lossy().into_owned(),
    ];
    config
}

async fn wait_entered(directory: &std::path::Path) {
    tokio::time::timeout(WATCHDOG, async {
        while !directory.join("entered").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("real native initialize reached controlled server");
}

fn assert_same_close(first: McpRouterShutdownObservation, second: McpRouterShutdownObservation) {
    let (
        McpRouterShutdownObservation::Reported(first),
        McpRouterShutdownObservation::Reported(second),
    ) = (first, second)
    else {
        panic!("native shutdown must report, not abandon")
    };
    assert!(
        Arc::ptr_eq(&first, &second),
        "repeat observation must retain the same native report"
    );
    assert!(first.is_settled() && first.guards_completed());
    assert_eq!(
        first.servers().len(),
        1,
        "one actual selected connection close"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collector_registration_linearizes_with_stop_before_staging() {
    let registry = Arc::new(CollectorAdapters::default());
    let selected = adapter();
    let (entered, entry) = tokio::sync::oneshot::channel();
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let registry_in = registry.clone();
    let selected_in = selected.clone();
    let release_in = release.clone();
    let registration = tokio::task::spawn_blocking(move || {
        registry_in.register(&selected_in, || {
            entered.send(()).unwrap();
            let (lock, changed) = &*release_in;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = changed.wait(released).unwrap();
            }
            Ok(())
        })
    });
    let mut stopping = None;
    let checked = std::panic::AssertUnwindSafe(async {
        tokio::time::timeout(WATCHDOG, entry)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                registry.0.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            ),
            "the registration owner must retain the same exclusion used by stop"
        );
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let stop_registry = registry.clone();
        stopping = Some(tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            stop_registry.stop();
        }));
        tokio::time::timeout(WATCHDOG, started_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !stopping.as_ref().unwrap().is_finished(),
            "actual stop cannot complete through the held registration exclusion"
        );
    })
    .catch_unwind()
    .await;
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    let registered = registration.await.unwrap();
    if let Some(stopping) = stopping {
        stopping.await.unwrap();
    } else {
        registry.stop();
    }
    assert!(registered.is_ok());
    let late = AdapterLease(adapter());
    assert!(matches!(
        registry.register(&late.0, || panic!("closed registration cannot enter")),
        Err(CollectorOpenRefusal::Closed)
    ));
    assert_eq!(registry.0.lock().unwrap().adapters.len(), 1);
    assert!(
        matches!(
            selected.observe_operation("gmail", "get_profile").await,
            McpOperationObservation::Absent
        ),
        "registration alone launches nothing"
    );
    registry.join().await.unwrap();
    assert!(settled(&late.0.shutdown_with_report().await));
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn cancelled_collector_opening_retains_native_close_until_host_join() {
    let directory = tempfile::tempdir().unwrap();
    let config = configuration(directory.path(), "hold");
    let registry = Arc::new(CollectorAdapters::default());
    let selected = adapter();
    let mut observer = selected.shutdown_observer();
    let registry_in = registry.clone();
    let selected_in = selected.clone();
    let opening = tokio::spawn(async move {
        let lease = AdapterLease(selected_in);
        registry_in.register(&lease.0, || Ok(())).unwrap();
        connect_selected(
            &lease.0,
            &config,
            &BTreeSet::from(["get_profile".into()]),
            WATCHDOG,
        )
        .await
    });
    let checked = std::panic::AssertUnwindSafe(async {
        wait_entered(directory.path()).await;
        assert_eq!(registry.0.lock().unwrap().adapters.len(), 1);
        assert!(
            observer.current().is_none(),
            "the live initialize has not reported close"
        );
        assert!(!directory.path().join("calls").exists());
    })
    .catch_unwind()
    .await;
    // Also runs if initialization/assertion failed: cancel this observer, then
    // join its actual task and every registered native connection before panic.
    opening.abort();
    let ended = opening.await;
    let joined = tokio::time::timeout(WATCHDOG, registry.join()).await;
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
    assert!(matches!(ended, Err(error) if error.is_cancelled()));
    joined.unwrap().unwrap();
    let first = observer.observe().await;
    let second = selected.shutdown_with_report().await;
    assert_same_close(first, second);
    assert!(
        !directory.path().join("calls").exists(),
        "cancelled initialization invokes no provider read"
    );
}

#[tokio::test]
async fn collector_ready_requires_actual_operation_and_joins_all_owned_connections() {
    let good_dir = tempfile::tempdir().unwrap();
    let empty_dir = tempfile::tempdir().unwrap();
    let registry = CollectorAdapters::default();
    let good = AdapterLease(adapter());
    let empty = AdapterLease(adapter());
    registry.register(&good.0, || Ok(())).unwrap();
    registry.register(&empty.0, || Ok(())).unwrap();
    let operations = BTreeSet::from(["get_profile".into()]);
    let checked = std::panic::AssertUnwindSafe(async {
        connect_selected(
            &good.0,
            &configuration(good_dir.path(), "ready"),
            &operations,
            WATCHDOG,
        )
        .await
        .unwrap();
        let mut empty_config = configuration(empty_dir.path(), "empty");
        // Establish an empty catalog, then refuse the requested operation.
        // A mismatched connector declaration instead closes during discovery,
        // before the host join whose physical custody this control checks.
        empty_config.connector.as_mut().unwrap().operations.clear();
        assert!(
            matches!(
                connect_selected(&empty.0, &empty_config, &operations, WATCHDOG).await,
                Err(CollectorOpenRefusal::NotReady)
            ),
            "empty pending queue cannot stand in for the actual selected operation"
        );
        assert_eq!(
            empty.0.active_server_names().await,
            vec!["gmail".to_string()],
            "the empty catalog must remain a real active connection until host join"
        );
        assert!(matches!(
            empty.0.observe_operation("gmail", "get_profile").await,
            McpOperationObservation::Absent
        ));
        assert!(matches!(
            good.0.observe_operation("gmail", "get_profile").await,
            McpOperationObservation::Callable {
                connector: Some(_),
                ..
            }
        ));
    })
    .catch_unwind()
    .await;
    let joined = tokio::time::timeout(WATCHDOG, registry.join()).await;
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
    joined.unwrap().unwrap();
    assert_same_close(
        good.0.shutdown_with_report().await,
        good.0.shutdown_with_report().await,
    );
    assert_same_close(
        empty.0.shutdown_with_report().await,
        empty.0.shutdown_with_report().await,
    );
    assert!(!good_dir.path().join("calls").exists() && !empty_dir.path().join("calls").exists());
}

#[test]
fn collector_launch_refuses_unowned_secrets_and_nonfinite_observation_budget() {
    let (mut config, _, _) = crate::private_originals::collector_setup::tests::fixture();
    assert!(check_launch(&config, WATCHDOG).is_ok());
    for timeout in [Duration::ZERO, Duration::MAX] {
        assert!(matches!(
            check_launch(&config, timeout),
            Err(CollectorOpenRefusal::UnsupportedConfiguration)
        ));
    }
    let McpTransportConfig::Stdio(stdio) = &mut config.transport else {
        unreachable!()
    };
    stdio.secret_env.insert("PROVIDER_SECRET".into());
    assert!(matches!(
        check_launch(&config, WATCHDOG),
        Err(CollectorOpenRefusal::UnsupportedConfiguration)
    ));
}
