//! Persisted human decision, native defining-realm effects, then task grant.
//! All entered native futures are retained by the existing cancellable driver.

use super::*;
use super::administration::{AdministrationAttempt, CaptureAdministration};
use super::outcome::{CredentialOwnerObservation, CredentialState, PublicationState, WriteState};
use crate::{approval::ApprovalPresenter, stdio::PromptBridge};
use std::{future::Future, sync::atomic::{AtomicBool, Ordering}};
use toolkit_mobkit::private_originals::{InstalledServiceTask, ServiceMcpPersistenceProposal};

impl InstalledAccountRequests {
    pub(crate) async fn present_credentials_console<F: Future<Output = ()>>(
        &mut self, mut request: InstalledCredentialRequest,
        presenter: &mut Option<ApprovalPresenter>, bridge: PromptBridge,
        values: oneshot::Receiver<transport::CredentialValues>,
        refresh: &console::Refresh<'_>, decision: &console::CaptureDecision<'_>,
        administration: &CaptureAdministration<'_>, closed: F,
    ) -> Result<bool, ConnectError> {
        let owner_closed = AtomicBool::new(false);
        let outcome_uncertain = AtomicBool::new(false);
        let result = async {
            if request.response.is_closed() { return Ok(true); }
            let login = request.console_task().and_then(|task| task.console_login()).cloned()
                .ok_or_else(|| refused("The credential request has no retained Console login"))?;
            let reservation = login.reserve().map_err(|_| refused("The Console login refused credential entry"))?;
            let checked_refresh = || -> futures_util::future::BoxFuture<'static, Result<(), ConnectError>> {
                let login = login.clone();
                let current = login.check().map_err(|_| refused("The Console login is unavailable"));
                let refreshed = current.is_ok().then(|| refresh());
                Box::pin(async move {
                    current?;
                    refreshed.expect("a current login selected refresh").await?;
                    login.check().map_err(|_| refused("The Console login is unavailable"))?;
                    Ok(())
                })
            };
            let refresh: &console::Refresh<'_> = &checked_refresh;
            let result = async {
                let closing = async {
                    tokio::select! { biased;
                        _ = closed => { owner_closed.store(true, Ordering::SeqCst); },
                        _ = login.closed() => {},
                    }
                };
                tokio::pin!(closing);
                if !console::checkpoint(&request.current, &mut request.response, &bridge, refresh, closing.as_mut()).await? {
                    return Ok(request.response.is_closed());
                }
                if checked_configuration(&request.selected, &request.selection)?.0 != request.config {
                    return Err(refused("The selected credential installation changed"));
                }
                let custody = request.entry.enter().map_err(|_| refused("The credential request cannot enter"))?;
                let (cancel_review, cancellation) = ReviewCancellation::channel();
                let task = request.task.take().ok_or_else(|| refused("The waiting task is unavailable"))?;
                let reviewed = console::drive_review_cancellable(
                    self.review.review_credentials_with_task(&request.selected, &request.config, &request.slots,
                        &request.selection, task, &request.current, values, refresh, decision,
                        administration, cancellation, &outcome_uncertain),
                    &self.review.approvals, presenter, &bridge, refresh, &mut request.response,
                    closing.as_mut(), Some(&login), Some(cancel_review), Some(&owner_closed),
                ).await?;
                let Some(reply) = reviewed else { return Ok(request.response.is_closed()); };
                if !console::checkpoint(&request.current, &mut request.response, &bridge, refresh, closing.as_mut()).await? {
                    return Ok(request.response.is_closed());
                }
                if checked_configuration(&request.selected, &request.selection)?.0 != request.config {
                    return Err(refused("The credential installation changed after review"));
                }
                let _ = request.response.send(reply);
                drop(custody);
                Ok(true)
            }.await;
            if outcome_uncertain.load(Ordering::SeqCst) { reservation.uncertain(); }
            else if result.is_ok() { reservation.settled(); }
            result
        }.await;
        if owner_closed.load(Ordering::SeqCst) || presenter.is_none() {
            let retired = self.review.retire();
            drop(presenter.take());
            return match result {
                Ok(_) => retired.map(|()| false),
                Err(error) => present::join_cleanup(Err(error), retired).map(|()| false),
            };
        }
        result.map(|_| true)
    }
}

impl InstalledServiceReview {
    async fn review_credentials_with_task(
        &self, selected: &InstalledConnectorSelection, config: &McpServerConfig,
        slots: &[ServiceCredentialSlot], selection: &CredentialSelection,
        task: InstalledServiceTask, current: &CurrentRequest,
        values: oneshot::Receiver<transport::CredentialValues>,
        refresh: &console::Refresh<'_>, capture_decision: &console::CaptureDecision<'_>,
        capture_administration: &CaptureAdministration<'_>, cancellation: ReviewCancellation,
        outcome_uncertain: &AtomicBool,
    ) -> Result<CredentialReply, ConnectError> {
        let check = || -> Result<(), ConnectError> {
            self.approvals.check_session(&self.session)?;
            cancellation.check()?;
            if checked_configuration(selected, selection)?.0 != *config {
                return Err(refused("The reviewed credential installation changed"));
            }
            Ok(())
        };
        check()?;
        refresh().await?;
        if !current.check().await { return Err(refused("The credential request is no longer current")); }
        check()?;
        let installation = self.realm_installation.as_ref()
            .ok_or_else(|| refused("The defining realm installation owner is unavailable"))?;
        let (_, generation) = installation.servers().await
            .map_err(|_| refused("The defining realm configuration is unavailable"))?;
        check()?;
        let observed = CredentialOwnerObservation { defining_realm: installation.realm().clone(),
            configuration_generation: generation, configuration: config.clone() };
        let mut reply = CredentialReply::pending(&observed,
            slots.iter().map(|slot| slot.environment_variable().to_owned()));
        let request = credential_review_request(selected, config, selection)?;
        let operations = selection.operations.iter().cloned().map(toolkit_package::Id::new)
            .collect::<Result<BTreeSet<_>, _>>().map_err(|_| refused("The credential operations are invalid"))?;
        let task = task.prepare_no_connector_persistence_review(config, operations, selection.expires_in_ms,
            request, selection.tool_policy.clone(),
            ServiceMcpPersistenceProposal::new(observed.defining_realm.clone(), generation))
            .await.map_err(|_| refused("The waiting task cannot be offered for credential approval"))?;
        check()?;
        // This existing owner persists and validates the complete decision
        // before returning. Neither protected forwarding nor the private value
        // delivery below is a decision or permission to mutate native state.
        let record = self.approvals.review_access_cancellable(task.request().clone(),
            cancellation.cancelled(), |id| cancellation.approval_created(id)).await?;
        let decision_current = capture_decision()?;
        check()?;
        reply.access_reviewed = true;
        if record.status == ApprovalStatus::Denied {
            reply.state = CredentialState::Denied;
            return Ok(reply);
        }
        if record.status != ApprovalStatus::Approved {
            return Err(refused("The credential review has no matching decision"));
        }
        let administration = capture_administration()?;
        // No native mutation has entered yet. A retired/cancelled review must
        // not park waiting for a protected sender that can no longer answer.
        let mut values = tokio::select! { biased;
            _ = cancellation.cancelled() => return Err(ConnectError::ReviewCancelled),
            values = values => values.map_err(|_| refused("The protected credential input is unavailable"))?.values,
        };
        if values.keys().cloned().collect::<BTreeSet<_>>() != slots.iter()
            .map(|slot| slot.environment_variable().to_owned()).collect::<BTreeSet<_>>() {
            return Err(refused("The protected credential declarations changed"));
        }
        if !current.check().await { return Ok(reply); }
        refresh().await?;
        check()?;
        if !decision_current() { return Ok(reply); }
        let attempt = AdministrationAttempt::new(administration.clone(), cancellation.clone());
        // Never race an entered native future against cancellation. The outer
        // driver sets cancellation and joins this same future to settlement.
        let installed = installation.install(config.clone(), generation, attempt.clone()).await;
        let state = match installed {
            Ok(generation) => {
                reply.persistence.install.generation = Some(generation);
                WriteState::Installed
            }
            Err(error) => install_failure(error),
        };
        attempt.settled(state == WriteState::OutcomeUnknown);
        outcome_uncertain.fetch_or(state == WriteState::OutcomeUnknown, Ordering::SeqCst);
        reply.persistence.install.state = state;
        if state != WriteState::Installed { return Ok(reply); }
        for slot in &mut reply.persistence.secrets {
            // Failure after a known install is a partial known outcome. It
            // does not roll back, retry, enter another slot or publish a grant.
            if check().is_err() || !current.check().await || refresh().await.is_err()
                || check().is_err() || !decision_current() { return Ok(reply); }
            let value = values.remove(&slot.slot)
                .ok_or_else(|| refused("The protected credential input is unavailable"))?;
            let value = meerkat_core::auth::McpServiceSecretValue::new(value.expose().to_owned());
            let attempt = AdministrationAttempt::new(administration.clone(), cancellation.clone());
            let stored = installation.set_secret(config, &slot.slot, value, attempt.clone()).await;
            let state = match stored { Ok(()) => WriteState::Stored, Err(error) => secret_failure(error) };
            attempt.settled(state == WriteState::OutcomeUnknown);
            outcome_uncertain.fetch_or(state == WriteState::OutcomeUnknown, Ordering::SeqCst);
            slot.state = state;
            if state != WriteState::Stored { return Ok(reply); }
        }
        if check().is_err() || !current.check().await || refresh().await.is_err()
            || check().is_err() || !decision_current() { return Ok(reply); }
        let publication_selection = selected.clone();
        let publication_config = config.clone();
        let publication_operations = selection.operations.clone();
        let publication_approvals = self.approvals.clone();
        let publication_session = self.session.clone();
        let publication_cancellation = cancellation.clone();
        let retained = task.approve(record, move || {
            publication_approvals.check_session(&publication_session).is_ok()
                && publication_cancellation.check().is_ok()
                && publication_selection.secret_configuration(&publication_operations).ok().as_ref()
                    == Some(&publication_config)
                && decision_current()
        }).await;
        match retained {
            Ok(retained) => {
                reply.task_publication = if retained { PublicationState::Retained } else { PublicationState::NotRetained };
                reply.grant_issued = retained;
                reply.fresh_instruction_retained = retained;
                if retained { reply.state = CredentialState::Reviewed; }
            }
            Err(_) => {
                outcome_uncertain.store(true, Ordering::SeqCst);
                reply.task_publication = PublicationState::OutcomeUnknown;
            }
        }
        Ok(reply)
    }
}

// These are the entered install path's typed outcomes. A known no-write
// failure leaves any earlier installation or credential state unchanged.
fn install_failure(error: meerkat_mobkit::rpc_gateway::GatewayRealmMcpError) -> WriteState {
    use meerkat_core::McpRealmPersistError;
    use meerkat_mobkit::rpc_gateway::GatewayRealmMcpError;
    match error {
        GatewayRealmMcpError::NoDocument => WriteState::NotWritten,
        GatewayRealmMcpError::NotAdmitted(_)
        | GatewayRealmMcpError::Persist(McpRealmPersistError::NotAdmitted(_)) => {
            WriteState::NotAdmitted
        }
        GatewayRealmMcpError::OutcomeUnknown(_)
        | GatewayRealmMcpError::Persist(McpRealmPersistError::OutcomeUnknown(_)) => {
            WriteState::OutcomeUnknown
        }
        GatewayRealmMcpError::Persist(
            McpRealmPersistError::ServerExists(_)
            | McpRealmPersistError::ServerNotFound(_)
            | McpRealmPersistError::EnvReference(_)
            | McpRealmPersistError::Access(_)
            | McpRealmPersistError::Config(_),
        ) => WriteState::NotWritten,
        _ => WriteState::OutcomeUnknown,
    }
}

fn secret_failure(error: meerkat::McpServiceSecretUpdateError) -> WriteState {
    use meerkat::McpServiceSecretUpdateError;
    match error {
        McpServiceSecretUpdateError::NotAdmitted(_) => WriteState::NotAdmitted,
        McpServiceSecretUpdateError::OutcomeUnknown(_) => WriteState::OutcomeUnknown,
        McpServiceSecretUpdateError::Invalid(_)
        | McpServiceSecretUpdateError::DefinitionChanged(_)
        | McpServiceSecretUpdateError::StillCurrent(_)
        | McpServiceSecretUpdateError::ConfigurationUnavailable(_)
        | McpServiceSecretUpdateError::Ownership(_) => WriteState::NotWritten,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{AdministrationEntryRefusal, ConfigRuntimeError, McpRealmPersistError};
    use meerkat_mobkit::rpc_gateway::GatewayRealmMcpError;

    const PAYLOAD: &str = "private-error-payload-must-not-be-projected";

    fn io_failure() -> ConfigRuntimeError {
        ConfigRuntimeError::Io(std::io::Error::other(PAYLOAD))
    }

    fn pending_reply() -> CredentialReply {
        let observed = CredentialOwnerObservation {
            defining_realm: meerkat_core::RealmId::parse("credential-projection").unwrap(),
            configuration_generation: 7,
            configuration: McpServerConfig::stdio(
                "fixture", "fixture", Vec::new(), Default::default(),
            ),
        };
        CredentialReply::pending(
            &observed,
            ["FIRST", "SECOND", "THIRD"].into_iter().map(str::to_owned),
        )
    }

    fn assert_incomplete_projection(reply: &CredentialReply) -> Value {
        let value = serde_json::to_value(reply).unwrap();
        assert_eq!(value["state"], "incomplete");
        assert_eq!(value["task_publication"], "not_entered");
        assert_eq!(value["grant_issued"], false);
        assert_eq!(value["fresh_instruction_retained"], false);
        assert_eq!(value["ready"], false);
        assert!(!value.to_string().contains(PAYLOAD));
        value
    }

    #[test]
    fn native_install_error_projection_distinguishes_refusal_from_unknown_without_payloads() {
        // Exercise both the actual native conversion and defensive direct
        // wrapping. No native write or provider call runs in this control.
        let cases = [
            (GatewayRealmMcpError::NoDocument, "not_written"),
            (
                GatewayRealmMcpError::from(McpRealmPersistError::ServerExists(PAYLOAD.into())),
                "not_written",
            ),
            (GatewayRealmMcpError::from(McpRealmPersistError::Config(io_failure())), "not_written"),
            (
                GatewayRealmMcpError::NotAdmitted(AdministrationEntryRefusal::Unavailable(
                    PAYLOAD.into(),
                )),
                "not_admitted",
            ),
            (
                GatewayRealmMcpError::Persist(McpRealmPersistError::NotAdmitted(
                    AdministrationEntryRefusal::NotAdministrator,
                )),
                "not_admitted",
            ),
            (GatewayRealmMcpError::OutcomeUnknown(PAYLOAD.into()), "outcome_unknown"),
            (
                GatewayRealmMcpError::from(McpRealmPersistError::OutcomeUnknown(io_failure())),
                "outcome_unknown",
            ),
            (
                GatewayRealmMcpError::Persist(McpRealmPersistError::OutcomeUnknown(io_failure())),
                "outcome_unknown",
            ),
        ];
        for (error, expected) in cases {
            let mut reply = pending_reply();
            reply.persistence.install.state = install_failure(error);
            let value = assert_incomplete_projection(&reply);
            assert_eq!(value["persistence"]["install"], json!({"state": expected}));
            assert!(
                value["persistence"]["secrets"].as_array().unwrap().iter()
                    .all(|slot| slot["state"] == "not_entered")
            );
        }
    }

    #[test]
    fn native_secret_error_projection_preserves_known_partial_saves_without_payloads() {
        use meerkat::McpServiceSecretUpdateError;
        let cases = [
            (McpServiceSecretUpdateError::Invalid(PAYLOAD.into()), "not_written"),
            (McpServiceSecretUpdateError::DefinitionChanged(PAYLOAD.into()), "not_written"),
            (McpServiceSecretUpdateError::StillCurrent(PAYLOAD.into()), "not_written"),
            (McpServiceSecretUpdateError::ConfigurationUnavailable(PAYLOAD.into()), "not_written"),
            (McpServiceSecretUpdateError::Ownership(PAYLOAD.into()), "not_written"),
            (
                McpServiceSecretUpdateError::NotAdmitted(AdministrationEntryRefusal::Unavailable(
                    PAYLOAD.into(),
                )),
                "not_admitted",
            ),
            (McpServiceSecretUpdateError::OutcomeUnknown(PAYLOAD.into()), "outcome_unknown"),
        ];
        for (error, expected) in cases {
            let mut reply = pending_reply();
            // Model a known earlier install and slot save. This is a projection
            // control, not evidence of executing those native effects.
            reply.access_reviewed = true;
            reply.persistence.install.state = WriteState::Installed;
            reply.persistence.install.generation = Some(8);
            reply.persistence.secrets[0].state = WriteState::Stored;
            reply.persistence.secrets[1].state = secret_failure(error);
            let value = assert_incomplete_projection(&reply);
            assert_eq!(
                value["persistence"]["install"],
                json!({"state":"installed", "generation":8}),
            );
            assert_eq!(
                value["persistence"]["secrets"],
                json!([
                    {"slot":"FIRST", "state":"stored"},
                    {"slot":"SECOND", "state":expected},
                    {"slot":"THIRD", "state":"not_entered"},
                ]),
            );
        }
    }
}
