//! Component controls with real native config persistence and retained futures.
//! No provider, MCP operation or whole installed setup is executed here.
use super::*;
use meerkat_core::{ConfigDelta, ConfigStore, FileConfigStore, McpRealmPersistTarget};
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn original_deadline_and_lost_login_refuse_with_sender_still_retained() {
    let (retire, receive) = watch::channel(false);
    let admission = Admission::new(now_ms() + 20, receive);
    assert!(admission.check().is_ok());
    tokio::time::timeout(Duration::from_secs(1), admission.closed())
        .await
        .unwrap();
    assert!(!*retire.borrow());
    assert!(admission.check().is_err());
    let current = Admission::new(now_ms() + 60_000, retire.subscribe());
    retire.send_replace(true);
    assert!(current.check().is_err());
    let (sender, receiver) = watch::channel(false);
    let lost = Admission::new(now_ms() + 60_000, receiver);
    drop(sender);
    assert!(lost.check().is_err());
}

#[tokio::test]
async fn actual_native_writer_rechecks_original_private_login_after_lock_wait() {
    let root = tempfile::tempdir().unwrap();
    let storage = crate::RealmOptions {
        context_root: root.path().to_owned(),
        state_root: Some(root.path().join("state")),
        user_config_root: Some(root.path().join("home")),
    }
    .resolve("private-entry")
    .unwrap();
    let path = crate::realm::config_source(&storage)
        .unwrap()
        .config_doc_path(&storage.locator.realm);
    FileConfigStore::new(path.clone(), meerkat_models::canonical())
        .patch(ConfigDelta(json!({"tools":{"mcp_servers":[]}})))
        .await
        .unwrap();
    let runtime = crate::realm::own_config_runtime(&storage).unwrap();
    let target = McpRealmPersistTarget::new(&runtime);
    let (_, generation) = target.servers().await.unwrap();
    let before = std::fs::read(&path).unwrap();
    let held = meerkat_core::config_runtime::ConfigWriterLock::try_hold(
        &path.with_file_name("config_state.lock"),
    )
    .unwrap()
    .unwrap();
    let (retire, receive) = watch::channel(false);
    let admission = Admission::new(now_ms() + 60_000, receive);
    let server = McpServerConfig::stdio("private-entry", "/bin/true", vec![], Default::default());
    let write = target.add_entered(server, Some(generation), &admission);
    tokio::pin!(write);
    let early = tokio::select! {
        result = write.as_mut() => Some(result),
        _ = tokio::time::sleep(Duration::from_millis(100)) => None,
    };
    retire.send_replace(true);
    drop(held);
    let result = match early {
        Some(result) => result,
        None => write.await,
    };
    assert!(matches!(
        result,
        Err(meerkat_core::mcp_config::McpRealmPersistError::NotAdmitted(
            _
        ))
    ));
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(target.servers().await.unwrap(), (vec![], generation));
}

#[tokio::test]
async fn accepted_command_retains_exact_effect_after_cancel_and_refuses_before_acceptance() {
    let (retire, receive) = watch::channel(false);
    let admission = Admission::new(now_ms() + 60_000, receive);
    let calls = AtomicUsize::new(0);
    let (entered, entered_rx) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let stop = admission.closed();
    let interrupt = async {
        stop.await;
        Ok(())
    };
    tokio::pin!(interrupt);
    let mut stopped = None;
    let exact = (
        "realm",
        "installed-job",
        "server",
        "agent",
        ["read"],
        Some(60_000_u64),
    );
    let (result, stayed_owned) = {
        let accepted = accepted_effect(interrupt.as_mut(), &mut stopped, &admission, || async {
            calls.fetch_add(1, Ordering::SeqCst);
            let retained = exact;
            let _ = entered.send(());
            released.await.unwrap();
            retained
        });
        tokio::pin!(accepted);
        let early = tokio::select! {
            result = accepted.as_mut() => Some(result),
            _ = entered_rx => None,
        };
        retire.send_replace(true);
        let after_cancel = if early.is_none() {
            accepted.as_mut().now_or_never()
        } else {
            None
        };
        let stayed_owned = early.is_none() && after_cancel.is_none();
        let _ = release.send(());
        let result = match early.or(after_cancel) {
            Some(result) => result,
            None => accepted.await,
        };
        (result, stayed_owned)
    };
    assert!(stayed_owned);
    assert_eq!(result.unwrap().unwrap(), exact);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(stopped, Some(secrets::Stop::Cancelled)));
    let mut stopped = None;
    let pending = std::future::pending::<io::Result<()>>();
    tokio::pin!(pending);
    let refused = accepted_effect(pending.as_mut(), &mut stopped, &admission, || async {
        calls.fetch_add(1, Ordering::SeqCst);
    })
    .await;
    assert!(matches!(refused, Some(Err(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn probe_host_retirement_refuses_actual_waiting_config_write_before_driver_fence() {
    use crate::service_probe::{
        ServiceProbeSelection,
        tests::{binding, selected},
    };
    let root = tempfile::tempdir().unwrap();
    let host =
        Arc::new(ServiceProbeSelection::new(binding(root.path()), vec![selected("one")]).unwrap());
    let storage = crate::RealmOptions {
        context_root: root.path().into(),
        state_root: Some(root.path().join("state")),
        user_config_root: Some(root.path().join("home")),
    }
    .resolve("private-probe")
    .unwrap();
    let path = crate::realm::config_source(&storage)
        .unwrap()
        .config_doc_path(&storage.locator.realm);
    FileConfigStore::new(path.clone(), meerkat_models::canonical())
        .patch(ConfigDelta(json!({"tools":{"mcp_servers":[]}})))
        .await
        .unwrap();
    let runtime = crate::realm::own_config_runtime(&storage).unwrap();
    let target = McpRealmPersistTarget::new(&runtime);
    let (_, generation) = target.servers().await.unwrap();
    let before = std::fs::read(&path).unwrap();
    let held = meerkat_core::config_runtime::ConfigWriterLock::try_hold(
        &path.with_file_name("config_state.lock"),
    )
    .unwrap()
    .unwrap();
    let (pipe, receive) = watch::channel(false);
    let admission = Admission::new(host.binding.expires_at_ms, receive).with_probe(Some(&host));
    let write = target.add_entered(
        McpServerConfig::stdio("exact", "/bin/true", vec![], Default::default()),
        Some(generation),
        &admission,
    );
    tokio::pin!(write);
    let early = tokio::select! {value=write.as_mut()=>Some(value),_=tokio::time::sleep(Duration::from_millis(100))=>None};
    host.control().retire();
    drop(held);
    let result = match early {
        Some(value) => value,
        None => write.await,
    };
    assert!(
        !*pipe.borrow(),
        "no driver or pipe retirement supplied this refusal"
    );
    assert!(matches!(
        result,
        Err(meerkat_core::mcp_config::McpRealmPersistError::NotAdmitted(
            _
        ))
    ));
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(target.servers().await.unwrap(), (vec![], generation));
}

#[tokio::test]
async fn probe_host_retirement_joins_the_exact_already_accepted_administrative_command() {
    use crate::service_probe::{
        ServiceProbeSelection,
        tests::{binding, selected},
    };
    let root = tempfile::tempdir().unwrap();
    let host =
        Arc::new(ServiceProbeSelection::new(binding(root.path()), vec![selected("one")]).unwrap());
    let (pipe, receive) = watch::channel(false);
    let admission = Admission::new(host.binding.expires_at_ms, receive).with_probe(Some(&host));
    let (entered, entry) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let interrupt = async {
        admission.closed().await;
        Ok(())
    };
    tokio::pin!(interrupt);
    let mut stopped = None;
    let calls = AtomicUsize::new(0);
    let command = accepted_effect(interrupt.as_mut(), &mut stopped, &admission, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        let _ = entered.send(());
        let _ = released.await;
        ("exact-realm", "exact-agent", "exact-installation")
    });
    tokio::pin!(command);
    let early = tokio::select! {value=command.as_mut()=>Some(value),_=entry=>None};
    host.control().retire();
    let after = if early.is_none() {
        command.as_mut().now_or_never()
    } else {
        None
    };
    let retained = early.is_none() && after.is_none();
    let _ = release.send(());
    let result = match early.or(after) {
        Some(value) => value,
        None => command.await,
    };
    assert!(retained);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!*pipe.borrow());
    assert_eq!(
        result.unwrap().unwrap(),
        ("exact-realm", "exact-agent", "exact-installation")
    );
    assert!(admission.enter().is_err());
}
