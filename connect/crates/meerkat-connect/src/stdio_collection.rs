//! Protocol7 projects the retained native collection owner. No wire value can
//! provide a program, raw operation, argument, credential or completion proof.
use super::*;
use crate::service_probe::{ServiceHost, ServiceSelection};
use crate::session::service_probe::CollectionExecution;

impl Driver {
    pub(super) fn selected_services(&self) -> Option<ServiceSelection> {
        match (&self.probe_selection, &self.collection_selection) {
            (Some(selected), None) => Some(ServiceSelection::Probes(selected.clone())),
            (None, Some(selected)) => selected
                .service_selection(self.collection_service_index)
                .map(ServiceSelection::Collections),
            _ => None,
        }
    }
    pub(super) fn selected_host(&self) -> Option<Arc<ServiceHost>> {
        self.selected_services()
            .map(|selection| selection.host().clone())
    }
    pub(super) fn service_availability_kind(&self) -> &'static str {
        if self
            .binding
            .as_ref()
            .is_some_and(|b| b.protocol_version == 7)
        {
            "service_collection_state"
        } else {
            "service_probe_state"
        }
    }
    pub(super) fn collection_state(&self, state: &mut Value) {
        if self
            .binding
            .as_ref()
            .is_some_and(|b| b.protocol_version == 7)
        {
            state["commissioning"] = json!(self.probe_commissioning);
            state["can_collect"] = json!(self.collection_available());
            state["active_collection"] = json!(self.collection_active);
            state["collection_sources"] = json!(
                self.collection_selection
                    .as_ref()
                    .map(|s| s.source_ids().collect::<Vec<_>>())
            );
            state["reusable_source_coverage"] = json!("unqualified_credential_continuity");
            if let Some(selected) = self
                .collection_selection
                .as_ref()
                .filter(|s| s.services.len() > 1)
            {
                state["service_ids"] = json!(selected.service_ids().collect::<Vec<_>>());
                state["service_id"] = json!(self.current_collection_service_id());
                state["can_open_next_service"] = json!(self.can_open_next_collection_service());
            }
        }
    }
    pub(super) fn current_collection_service_id(&self) -> Option<&str> {
        self.collection_selection
            .as_ref()?
            .services
            .get(self.collection_service_index)
            .map(|s| s.id.as_str())
    }
    fn can_open_next_collection_service(&self) -> bool {
        let Some(selected) = self.collection_selection.as_ref() else {
            return false;
        };
        let Some(group) = selected.services.get(self.collection_service_index) else {
            return false;
        };
        let Some(ready) = self.probe_ready.as_ref() else {
            return false;
        };
        self.collection_service_index + 1 < selected.services.len()
            && self.retained_collection_services.len() == self.collection_service_index
            && self.collection_available()
            && self.retired
            && self.plain_close_started
            && self.private_close_started
            && self.collection_open_id.is_none()
            && self.next_service_receivers.is_none()
            && !self.opening
            && self.workers == 0
            && self.tasks.is_empty()
            && self.active.is_none()
            && self.histories == 0
            && !self.connector_busy
            && self.pending.is_none()
            && self.session.is_none()
            && self.plain_owner.is_none()
            && self.plain_closed.is_none()
            && self.probe_capture.is_none()
            && !self.probe_commissioning
            && self.close_failure.is_none()
            && !self.output_failed
            && !self.eof
            && self.close_replies.is_empty()
            && self.retire_replies.is_empty()
            && group.collections.iter().all(|source| {
                self.collected.contains(source.source_id())
                    && self.collection_reports.iter().any(|report| {
                        report.source_id == source.source_id()
                            && report.job_id == ready.job_id()
                            && &report.session_id == ready.session_id()
                    })
            })
    }
    pub(super) fn open_next_collection_service(
        &mut self,
        id: String,
        params: OpenCollectionService,
    ) {
        if self.scope.as_deref() != Some(params.scope.as_str()) || !opaque(&params.service_id) {
            self.bad(&id, "invalid_binding");
            return;
        }
        if !self.can_open_next_collection_service() {
            self.bad(&id, "collection_unavailable");
            return;
        }
        let selected = self
            .collection_selection
            .as_ref()
            .expect("retained selection");
        if selected.services[self.collection_service_index + 1].id != params.service_id {
            self.bad(&id, "invalid_binding");
            return;
        }
        // Preserve the actual prior installation, reports and finalization
        // owners. No Ready, grant, caller or receipt is reconstructed here.
        self.retained_collection_services
            .push(self.probe_ready.take().expect("checked actual Ready"));
        self.collection_service_index += 1;
        self.collection_open_id = Some(id);
        self.approvals.take();
        self.connector.take();
        self.lazy.take();
        self.plain_pending = false;
        self.plain_close_started = false;
        self.private_close_started = false;
        self.last_send_id = None;
        self.last_collection_id = None;
        self.last = None;
        // Only a completed internal setup fence is crossed. The original host
        // lifetime and cancellation channel are never reset. A new presenter
        // channel cannot revive any old watch receiver or old setup worker.
        let (stop, retired) = watch::channel(false);
        self.stop = stop;
        self.bridge.retired = retired;
        let (notices, notice_rx) = mpsc::channel(32);
        self.notices = notices;
        let (prompts, prompt_rx) = mpsc::channel(1);
        self.bridge.prompts = prompts;
        self.next_service_receivers = Some((notice_rx, prompt_rx));
        self.retired = false;
        self.opening = true;
        let binding = self.binding.clone().expect("checked original binding");
        let bridge = self.bridge.clone();
        let retired = self.stop.subscribe();
        let provider_store_root = self.provider_store_root.clone();
        let selection = self.selected_services();
        self.tasks.push(
            async move {
                Work::Opened(Box::new(
                    before_entry(retired, async move {
                        let selection =
                            selection.ok_or_else(|| OpenFailure::from("invalid_binding"))?;
                        open(binding, bridge, provider_store_root, Some(selection)).await
                    })
                    .await,
                ))
            }
            .boxed(),
        );
        self.state_event();
    }
    pub(super) fn collection_available(&self) -> bool {
        self.binding
            .as_ref()
            .is_some_and(|b| b.protocol_version == 7)
            && self.collection_selection.is_some()
            && self.probe_selection.is_none()
            && self.probe_ready.is_some()
            && self.collection_active.is_none()
            && !self.closing
            && !self.closed
            && !self.failed
            && !*self.plain_cancel.borrow()
            && self
                .selected_host()
                .is_some_and(|host| host.lifetime.current(host.binding.expires_at_ms))
    }
    pub(super) fn begin_collection(&mut self, id: String, params: Collect) {
        if self.scope.as_deref() != Some(params.scope.as_str())
            || !opaque(&params.job_id)
            || !opaque(&params.source_id)
        {
            self.bad(&id, "invalid_binding");
            return;
        }
        if !self.collection_available() {
            self.bad(&id, "collection_unavailable");
            return;
        }
        let ready = self
            .probe_ready
            .as_ref()
            .expect("retained actual Ready")
            .clone();
        if ready.job_id() != params.job_id {
            self.bad(&id, "invalid_binding");
            return;
        }
        let Some(source) = self
            .collection_selection
            .as_ref()
            .and_then(|s| s.services.get(self.collection_service_index))
            .and_then(|s| {
                s.collections
                    .iter()
                    .find(|source| source.source_id() == params.source_id)
                    .cloned()
            })
        else {
            self.bad(&id, "invalid_binding");
            return;
        };
        // This is transport admission only. Ready independently retains source
        // attempt admission under its actual native execution mutex.
        if !self.collected.insert(source.source_id().to_owned()) {
            self.bad(&id, "collection_already_attempted");
            return;
        }
        self.collection_active = Some(id.clone());
        self.last_collection_id = Some(id.clone());
        self.workers += 1;
        // No observer select/timeout drops this future. EOF, explicit stop and
        // revocation retire the same host; the driver continues joining tasks.
        self.tasks
            .push(async move { Work::Collected(id, ready.run_collection(source).await) }.boxed());
        self.state_event();
    }
    pub(super) fn cancel_collection(&mut self, id: &str, params: &Cancel) -> bool {
        if self
            .binding
            .as_ref()
            .is_none_or(|b| b.protocol_version != 7)
            || self.scope.as_deref() != Some(params.scope.as_str())
        {
            return false;
        }
        let active = self.collection_active.as_deref() == Some(params.send_request_id.as_str());
        let completed = self.collection_active.is_none()
            && self.last_collection_id.as_deref() == Some(params.send_request_id.as_str());
        if !active && !completed {
            return false;
        }
        self.fence("cancelled");
        self.output(result(
            id,
            json!({"scope":params.scope,"send_request_id":params.send_request_id,
            "status":if active{"cancellation_requested"}else{"not_current"}}),
        ));
        true
    }
    pub(super) fn collection_finished(&mut self, id: String, mut execution: CollectionExecution) {
        self.workers -= 1;
        self.collection_active.take();
        if execution.report.status
            == crate::service_collection::CollectionStatus::HistoricalChecksCompleted
            && self
                .selected_host()
                .is_none_or(|host| !host.lifetime.current(host.binding.expires_at_ms))
        {
            // Preserve all actual historical rows, while refusing an overall
            // completed status to a retired viewer. No current-coverage lease.
            execution.report.status = crate::service_collection::CollectionStatus::Cancelled;
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
            json!({"scope":self.scope,"collection_request_id":id,
            "report":&execution.report,"state":self.state()}),
        ));
        self.collection_reports.push(execution.report);
        self.state_event();
    }
}

/// Preserve actual native cleanup custody and reports across a failed pipe.
/// This wrapper applies only to the explicitly selected v7 invocation.
pub(super) fn retain_failure(
    v7: bool,
    reports: Vec<crate::service_collection::ServiceCollectionReport>,
    outcome: Result<(), ConnectError>,
) -> Result<(), ConnectError> {
    match outcome {
        Err(error) if v7 => Err(ConnectError::native(
            "Retain failed service collection invocation",
            crate::service_collection::ServiceCollectionFailure::new(reports, error),
        )),
        outcome => outcome,
    }
}
