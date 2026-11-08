//! Explicit protocol6 transport for the existing finalizer and fresh probe owner.
//! No model/browser parameter supplies required operation or resource authority.
use super::*;
use crate::session::service_probe::{Captured, Execution, Ready};
use crate::terminal_connector::{FinalizedAction, TerminalConnectors};

pub(super) struct Finalized {
    action: Option<FinalizedAction>,
    commissioning: Option<crate::session::service_probe::CommissionObservation>,
    ready: Result<Option<Ready>, ConnectError>,
}
pub(super) async fn finalize(
    captured: Captured,
    closed: crate::session::ClosedSetup,
    mut owner: TerminalConnectors,
    mut cancelled: watch::Receiver<bool>,
    selection: crate::service_probe::ServiceSelection,
    bridge: PromptBridge,
    admission: crate::terminal_connector::private_service::Admission,
) -> Result<Option<Finalized>, ConnectError> {
    let closed = captured.close(closed).await?;
    let reviewed = owner.take_reviewed_action().ok_or_else(ended)?;
    let reviewed_selection = reviewed.probe_selection();
    // The reviewed command retains its exact factory credential authority. All
    // disposed presentation/runtime owners go before administrative realm custody.
    drop(owner);
    // A v7 source is an actual installed tuple. Remote exports/access-only
    // reviews supply no such owner; do not finalize them as a substitute.
    if selection.version() == 7 && reviewed_selection.is_none() {
        return Ok(Some(Finalized {
            action: None,
            ready: Ok(None),
            commissioning: None,
        }));
    }
    let commissioning = if let Some(selected) = &reviewed_selection {
        let outcome = closed
            .commission(selected, &selection, &bridge, &admission)
            .await;
        if let Err(error) = outcome.result {
            return Ok(Some(Finalized {
                action: None,
                ready: Err(error),
                commissioning: Some(outcome.observation),
            }));
        }
        Some(outcome.observation)
    } else {
        None
    };
    let interrupt = async move {
        loop {
            if *cancelled.borrow_and_update() {
                return Ok(());
            }
            if cancelled.changed().await.is_err() {
                return Ok(());
            }
        }
    };
    tokio::pin!(interrupt);
    let action = match reviewed
        .finalize_outcome_unless_cancelled(interrupt.as_mut())
        .await
    {
        Ok(Some(action)) => action,
        Ok(None) => {
            return Ok(Some(Finalized {
                action: None,
                ready: Ok(None),
                commissioning,
            }));
        }
        Err(error) => {
            return Ok(Some(Finalized {
                action: None,
                ready: Err(error),
                commissioning,
            }));
        }
    };
    let ready = reviewed_selection
        .map(|reviewed| closed.finalized(reviewed, &action, selection.host().clone()))
        .transpose();
    Ok(Some(Finalized {
        action: Some(action),
        ready,
        commissioning,
    }))
}
impl Driver {
    pub(super) fn commissioning_current(&self) -> bool {
        self.probe_commissioning
            && self
                .binding
                .as_ref()
                .is_some_and(|binding| matches!(binding.protocol_version, 6 | 7))
            && !self.closing
            && !self.closed
            && !self.failed
            && !*self.plain_cancel.borrow()
            && self
                .selected_host()
                .is_some_and(|host| host.lifetime.current(host.binding.expires_at_ms))
    }
    pub(super) fn probe_initialization_matches(&self, binding: &Initialize) -> bool {
        let selection = self.selected_services();
        if self.probe_selection.is_some() && self.collection_selection.is_some() {
            return false;
        }
        match selection {
            None => !matches!(binding.protocol_version, 6 | 7),
            Some(selected) => {
                let host = &selected.host().binding;
                binding.protocol_version == selected.version()
                    && binding.adapter.is_services()
                    && host.application_id == binding.application.application_id
                    && host.instance_id == binding.application.instance_id
                    && host.origin == binding.application.origin
                    && host.viewer_id == binding.viewer.principal_id
                    && host.login_scope_id == binding.viewer.login_scope_id
                    && host.expires_at_ms == binding.viewer.expires_at_ms
                    && host.realm.realm.as_str() == binding.realm.realm_id
                    && host.realm.state_root == std::path::Path::new(&binding.realm.state_root)
                    && host.context_root == std::path::Path::new(&binding.realm.context_root)
                    && host.user_config_root
                        == std::path::Path::new(&binding.realm.user_config_root)
                    && selected.host().lifetime.current(host.expires_at_ms)
            }
        }
    }
    pub(super) fn probe_available(&self) -> bool {
        self.binding
            .as_ref()
            .is_some_and(|b| b.protocol_version == 6)
            && self.probe_ready.is_some()
            && self.probe_active.is_none()
            && !self.closing
            && !self.closed
            && !self.failed
            && !*self.plain_cancel.borrow()
            && self
                .probe_selection
                .as_ref()
                .is_some_and(|s| s.lifetime.current(s.binding.expires_at_ms))
    }
    pub(super) fn begin_probe(&mut self, id: String, params: Probe) {
        if self.scope.as_deref() != Some(params.scope.as_str())
            || !opaque(&params.job_id)
            || !opaque(&params.profile_id)
        {
            self.bad(&id, "invalid_binding");
            return;
        }
        if !self.probe_available() {
            self.bad(&id, "probe_unavailable");
            return;
        }
        let ready = self
            .probe_ready
            .as_ref()
            .expect("availability retained exact continuation")
            .clone();
        if ready.job_id() != params.job_id {
            self.bad(&id, "invalid_binding");
            return;
        }
        let selected = self
            .probe_selection
            .as_ref()
            .expect("availability retained process selection")
            .clone();
        let Some(profile) = selected
            .profiles
            .iter()
            .find(|profile| profile.id == params.profile_id)
            .cloned()
        else {
            self.bad(&id, "invalid_binding");
            return;
        };
        // Even an unknown or cancelled attempt is never automatically replayed.
        if !self.probed.insert(profile.id.clone()) {
            self.bad(&id, "probe_already_attempted");
            return;
        }
        self.probe_active = Some(id.clone());
        self.last_probe_id = Some(id.clone());
        self.workers += 1;
        self.tasks
            .push(async move { Work::Probed(id, ready.run(selected, profile).await) }.boxed());
        self.state_event();
    }
    pub(super) fn cancel_probe(&mut self, id: &str, params: &Cancel) -> bool {
        if self
            .binding
            .as_ref()
            .is_none_or(|b| b.protocol_version != 6)
            || self.scope.as_deref() != Some(params.scope.as_str())
        {
            return false;
        }
        let current = self.probe_active.as_deref() == Some(params.send_request_id.as_str());
        let completed = self.probe_active.is_none()
            && self.last_probe_id.as_deref() == Some(params.send_request_id.as_str());
        if !current && !completed {
            return false;
        }
        self.fence("cancelled");
        self.output(result(
            id,
            json!({"scope":params.scope,"send_request_id":params.send_request_id,
            "status":if current{"cancellation_requested"}else{"not_current"}}),
        ));
        true
    }
    pub(super) fn probe_finalized(&mut self, result: Result<Option<Finalized>, ConnectError>) {
        self.probe_commissioning = false;
        match result {
            Ok(Some(finalized)) => {
                // Native finalization and probe availability are distinct events.
                if let Some(observation) = finalized.commissioning {
                    self.event(json!({"kind":"service_commissioning","state":"observed","observation":observation.projection(),"capability_verified":false}));
                    self.probe_commissions.push(observation);
                }
                let ready = match (finalized.action, finalized.ready) {
                    (None, Err(error)) => {
                        self.plain_finalized(Err(error));
                        self.state_event();
                        return;
                    }
                    (action, ready) => {
                        self.plain_finalized(Ok(action));
                        ready
                    }
                };
                match ready {
                    Ok(Some(ready))=>{
                        let job_id=ready.job_id().to_owned();
                        let session_id=ready.session_id().clone();
                        self.probe_ready=Some(Arc::new(ready));
                        self.event(json!({"kind":self.service_availability_kind(),
                            "state":if self.probe_available()||self.collection_available(){"available"}else{"retired"},
                            "job_id":job_id,"session_id":session_id,"capability_verified":false}));
                    },
                    Ok(None)=>self.event(json!({"kind":self.service_availability_kind(),"state":"unsupported_source",
                        "capability_verified":false})),
                    Err(_)=>self.event(json!({"kind":self.service_availability_kind(),"state":"finalization_not_selected",
                        "capability_verified":false})),
                }
                self.state_event();
            }
            Ok(None) => self.plain_finalized(Ok(None)),
            Err(error) => self.plain_finalized(Err(error)),
        }
    }
    pub(super) fn probed(&mut self, id: String, mut execution: Execution) {
        self.workers -= 1;
        self.probe_active.take();
        if self
            .probe_selection
            .as_ref()
            .is_none_or(|s| !s.lifetime.current(s.binding.expires_at_ms))
        {
            // Keep native terminal/cleanup facts while refusing expired disclosure
            // of a resource-semantic success to this private viewer.
            execution.report.status = crate::service_probe::ProbeStatus::Cancelled;
        }
        if let Some(error) = execution.error {
            self.failed = true;
            self.close_failure = Some(match self.close_failure.take() {
                Some(prior) => ConnectError::Cleanup {
                    primary: Box::new(prior),
                    cleanup: Box::new(error),
                },
                None => error,
            });
            self.fence("failed");
        }
        self.output(result(
            &id,
            json!({"scope":self.scope,"probe_request_id":id,
            "report":execution.report,"state":self.state()}),
        ));
        self.state_event();
    }
}
