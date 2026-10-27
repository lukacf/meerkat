//! Retain the existing service preparation owner on a private-pipe binding.
//!
//! The host must authorize and construct the service composition separately.
//! This worker adds no tools, prompt kinds, install permission or readiness
//! state. Native handoff acknowledgement remains the request owner's result.

use futures_util::FutureExt;
use meerkat_core::ResolvedStorage;

use crate::{
    ConnectError,
    resources::InstalledResources,
    service_preparation::ServicePreparationPresenter,
};

use super::PromptBridge;

fn unconfirmed() -> ConnectError {
    // Resource errors can contain private paths. The private-pipe driver needs
    // only a failure disposition; the original resource owner retains evidence.
    ConnectError::InvalidInput(
        "The service preparation handoff is unconfirmed; close this setup session without retrying"
            .into(),
    )
}

/// Drive this future through completion. Do not select it away on retirement.
/// The driver's existing worker count must include it until it returns, and a
/// failure remains a failure even when the binding has already been retired.
pub(super) async fn run(
    mut presenter: ServicePreparationPresenter,
    storage: ResolvedStorage,
    bridge: PromptBridge,
) -> Result<(), ConnectError> {
    loop {
        // A queued request must not enter resource preparation after the
        // binding has retired, including before this worker's first poll.
        if bridge.retired() {
            return Ok(());
        }
        let request = tokio::select! {
            biased;
            _ = bridge.closed() => return Ok(()),
            request = presenter.next() => request,
        };
        let Some(mut request) = request else {
            return Err(unconfirmed());
        };
        if bridge.retired() {
            return Ok(());
        }
        // A native tool waiter can disappear while the request is queued.
        // Observe its existing lease before locating resources or starting work.
        if request.closed().now_or_never().is_some() {
            return Err(unconfirmed());
        }
        // File verification is synchronous. Retain and join it off the pipe's
        // reader task so controls stay responsive without abandoning that work.
        let resources = tokio::task::spawn_blocking(InstalledResources::locate)
            .await
            .map_err(|_| unconfirmed())?
            .map_err(|_| unconfirmed())?;
        if bridge.retired() {
            return Ok(());
        }
        if request.closed().now_or_never().is_some() {
            return Err(unconfirmed());
        }
        let interrupted = async {
            bridge.closed().await;
            Ok(())
        };
        // This is the original preparer, not a reconstructed command or an
        // arbitrary host response. It retains its entered child and joins it
        // when either retirement or the exact native waiter closes. Its native
        // handoff must also acknowledge before the worker continues.
        let result = request.prepare_installed(resources, &storage, interrupted).await;
        match result {
            Ok(true) => {}
            Ok(false) if bridge.retired() => return Ok(()),
            Ok(false) | Err(_) => return Err(unconfirmed()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        RealmOptions,
        paired_host_tool::PairedHostFrontend,
        service_preparation::{LazySetup, TOOLKIT_PREPARE_SERVICES},
    };
    use meerkat_core::{AgentToolDispatcher, types::ToolCallView};
    use std::{sync::Arc, time::Duration};
    use tokio::sync::{mpsc, watch};

    fn bridge() -> (PromptBridge, watch::Sender<bool>) {
        let (prompts, _receiver) = mpsc::channel(1);
        let (stop, retired) = watch::channel(false);
        (PromptBridge { prompts, retired }, stop)
    }

    fn storage(root: &std::path::Path) -> ResolvedStorage {
        RealmOptions {
            context_root: root.into(),
            state_root: Some(root.join("state")),
            user_config_root: Some(root.join("user")),
        }
        .resolve("service-stdio-test")
        .unwrap()
    }

    async fn request(dispatcher: Arc<dyn AgentToolDispatcher>) {
        let args = serde_json::value::to_raw_value(&serde_json::json!({})).unwrap();
        assert!(dispatcher
            .dispatch(ToolCallView {
                id: "prepare",
                name: TOOLKIT_PREPARE_SERVICES,
                args: &args,
            })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn retired_before_first_poll_drops_queued_request_without_preparing() {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path());
        let artifacts = storage.locator.state_root.join(".meerkat-connect");
        let (paired_host, _hosts) = PairedHostFrontend::deferred();
        let (lazy, presenter) = LazySetup::new(paired_host);
        let mut pending = Box::pin(request(lazy.dispatcher()));
        assert!(pending.as_mut().now_or_never().is_none());
        let (bridge, stop) = bridge();
        stop.send_replace(true);
        run(presenter, storage, bridge).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), pending).await.unwrap();
        assert!(!lazy.owner().accepted());
        assert!(!artifacts.exists());
    }

    #[tokio::test]
    async fn closed_native_waiter_refuses_queued_preparation_without_resources() {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path());
        let artifacts = storage.locator.state_root.join(".meerkat-connect");
        let (paired_host, _hosts) = PairedHostFrontend::deferred();
        let (lazy, presenter) = LazySetup::new(paired_host);
        let mut pending = Box::pin(request(lazy.dispatcher()));
        assert!(pending.as_mut().now_or_never().is_none());
        drop(pending);
        let (bridge, _stop) = bridge();
        let result = tokio::time::timeout(Duration::from_secs(1), run(presenter, storage, bridge))
            .await.unwrap();
        assert!(result.is_err());
        assert!(!lazy.owner().accepted());
        assert!(!artifacts.exists());
    }

    #[tokio::test]
    async fn idle_worker_joins_on_retirement_without_preparing() {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path());
        let artifacts = storage.locator.state_root.join(".meerkat-connect");
        let (paired_host, _hosts) = PairedHostFrontend::deferred();
        let (_lazy, presenter) = LazySetup::new(paired_host);
        let (bridge, stop) = bridge();
        let mut worker = Box::pin(run(presenter, storage, bridge));
        assert!(worker.as_mut().now_or_never().is_none());
        stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), worker).await.unwrap().unwrap();
        assert!(!artifacts.exists());
    }
}
