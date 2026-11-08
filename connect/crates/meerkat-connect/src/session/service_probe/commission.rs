//! Post-close protected commissioning of the real probe controller/catalog.
//! The native stage is an administrative owner only. No input is submitted
//! under its local operator and no ticket is transplanted into the host opener.
use super::*;
use crate::{
    approval::{ApprovalPresenter, LocalApprovals},
    stdio::{PromptAnswer, PromptBridge},
};
use meerkat::governed_realm::{
    staged::{
        StagedIssuanceEffect, StagedRealmError, StagedStockRealm, stage_stock_realm_persistence_as,
    },
    stock::{
        govern_stock_realm,
        issuance::{StockGrantError, StockGrantRequest},
    },
};
use meerkat_core::{AdministrationEntry, ApprovalDecision, ApprovalPrincipalId, ApprovalStatus};
use std::{
    io::Write,
    path::Path,
    time::{Duration, SystemTime},
};

/// Actual native results remain retained with this invocation; the wire is a
/// redacted projection. Neither projection nor file is an access authority.
pub(crate) struct CommissionObservation {
    projection: Value,
    _grants:
        Vec<Result<meerkat::governed_realm::stock::issuance::StockGrantOutcome, StagedRealmError>>,
}
impl CommissionObservation {
    pub(crate) fn projection(&self) -> Value {
        self.projection.clone()
    }
}
pub(crate) struct CommissionResult {
    pub(crate) observation: CommissionObservation,
    pub(crate) result: Result<(), ConnectError>,
}

impl Closed {
    pub(crate) async fn commission(
        &self,
        selected: &ProbeReviewedSelection,
        selection: &crate::service_probe::ServiceSelection,
        bridge: &PromptBridge,
        admission: &crate::terminal_connector::private_service::Admission,
    ) -> CommissionResult {
        let mut observed = CommissionObservation {
            projection: json!({"realm":self.0.storage.locator.realm.as_str(),
            "session_id":self.0.session,"job_id":selected.installed.job_id,"agent":selected.request.agent,
            "state":"incomplete","governance":"not_entered","grants":[],"controller_disposal":null,"issuance":null,
            "custody_released":false,"capability_verified":false}),
            _grants: Vec::new(),
        };
        let result = self
            .commission_inner(selected, selection, bridge, admission, &mut observed)
            .await;
        observed.projection["state"] = json!(if result.is_ok() {
            "completed"
        } else {
            "incomplete"
        });
        CommissionResult {
            observation: observed,
            result,
        }
    }
    async fn commission_inner(
        &self,
        selected: &ProbeReviewedSelection,
        selection: &crate::service_probe::ServiceSelection,
        bridge: &PromptBridge,
        admission: &crate::terminal_connector::private_service::Admission,
        observed: &mut CommissionObservation,
    ) -> Result<(), ConnectError> {
        let host = selection.host();
        let current = || {
            admission
                .enter()
                .map(|_| ())
                .map_err(|_| contract::unavailable())
        };
        current()?;
        if host.binding.realm != self.0.storage.locator
            || selected.installed.realm != self.0.storage.locator
            || selected.installed.session != self.0.session
        {
            return Err(contract::unavailable());
        }
        let StockGrantTarget::Mcp {
            server,
            operations: McpOperations::Exact(operations),
        } = &selected.request.target
        else {
            return Err(contract::unavailable());
        };
        // V7 does not rewrite a reviewed grant or accept a larger grant as a
        // collection ceiling. The human review must select the exact union.
        if server != &selected.config.name
            || !selection.permits_review(
                &selected.installed.family,
                &selected.installed.profile,
                &selected.request.agent,
                operations,
                &selected.config,
            )
        {
            return Err(contract::unavailable());
        }
        selected.recheck().await?;
        current()?;
        // No service/runtime/credential facade from the original setup may be
        // retained here. Closed was produced only after the actual joined close.
        let artifacts = crate::resources::reserve_setup_artifacts(&self.0.storage)?;
        let actor = ApprovalPrincipalId::new(host.binding.viewer_id.clone())
            .map_err(|_| contract::unavailable())?;
        let (approvals, mut presenter) = LocalApprovals::create_browser(
            &artifacts,
            actor,
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(host.binding.expires_at_ms))
                .ok_or_else(contract::unavailable)?,
        )?;
        approvals.bind_session(&self.0.session)?;
        let body=async {
            let posture=meerkat_store::read_realm_posture_in(&self.0.storage.locator.state_root,
                self.0.storage.locator.realm.as_str()).await.map_err(|_|contract::unavailable())?;
            // The real setup already persisted its conversation. Do not create
            // a replacement realm to repair an absent predecessor.
            if posture==meerkat_store::RealmPosture::NotMaterialized {return Err(contract::unavailable());}
            if posture!=meerkat_store::RealmPosture::Governed {
                let body=json!({"realm":self.0.storage.locator.realm.as_str(),"session_id":self.0.session,
                    "job_id":selected.installed.job_id,"agent":selected.request.agent,
                    "change":"govern_existing_realm","consequence":"This saved governance change remains if later review or setup stops. No service, model or resource permission is issued by this step."});
                protected_review(&approvals,&mut presenter,bridge,admission,&self.0.session,
                    "Make this selected realm governed for the required service checks?",body).await?;
                selected.recheck().await?; current()?;
                let principals=StockPrincipals::for_local_operator(&self.0.storage.locator.realm)
                    .map_err(|_|contract::unavailable())?;
                // The whole exact administrative command is admitted here.
                // Cancellation afterwards joins it; there is no after-lock veto.
                observed.projection["governance"]=json!("entered");
                let result=govern_stock_realm(&self.0.storage.locator.state_root,
                    self.0.storage.locator.realm.as_str(),&principals).await;
                observed.projection["governance"]=json!(if result.is_ok(){"committed"}else{"outcome_unknown"});
                super::super::standalone::combine(
                    result.map_err(|error|ConnectError::native("Commission selected realm governance",error)),
                    record(&artifacts,"governance",&observed.projection))?;
                current()?;
            } else {observed.projection["governance"]=json!("already_governed");}
            selected.recheck().await?;current()?;
            let mut stage=stage_stock_realm_persistence_as(&meerkat::DiskStorageProvider,
                &self.0.storage.locator.state_root,self.0.storage.locator.realm.as_str(),None,
                Some(meerkat_store::RealmOrigin::Explicit),Some(self.0.storage.layout.clone()),
                &selected.request.agent).await.map_err(|error|ConnectError::native("Stage selected probe controller",error))?;
            // From creation through disposal this retained future owns the exact
            // stage. No select drops an entered preparation, grant or abort.
            let outcome=self.commission_stage(&mut stage,selected,bridge,admission,
                &approvals,&mut presenter,&artifacts,observed).await;
            let disposal=stage.abort().await;
            match disposal {
                Ok(disposal)=>{
                    observed.projection["controller_disposal"]=json!(match disposal.controller {
                        meerkat::PreparedControllerDisposal::NotPrepared=>"not_prepared",
                        meerkat::PreparedControllerDisposal::Drained=>"drained",
                        meerkat::PreparedControllerDisposal::Taken=>"taken",
                        meerkat::PreparedControllerDisposal::AlreadyRetired=>"already_retired",
                    });
                    observed.projection["custody_released"]=json!(disposal.custody==meerkat_runtime::StagedCustodyRelease::Released);
                    observed.projection["issuance"]=json!(match disposal.issuance {
                        StagedIssuanceEffect::NotEntered=>"not_entered",
                        StagedIssuanceEffect::Committed=>"committed",
                        StagedIssuanceEffect::Uncertain=>"uncertain",
                    });
                    // No command waiter was discarded, so unexpected unobserved
                    // outcomes refuse rather than becoming a replacement grant.
                    let released=disposal.custody==meerkat_runtime::StagedCustodyRelease::Released
                        && matches!(disposal.controller,meerkat::PreparedControllerDisposal::NotPrepared
                            |meerkat::PreparedControllerDisposal::Drained)
                        && disposal.unobserved_grants.is_empty() && disposal.issuance!=StagedIssuanceEffect::Uncertain;
                    let retained=record(&artifacts,"stage",&observed.projection);
                    let cleanup=if released {retained}else {super::super::standalone::combine(Err(contract::unavailable()),retained)};
                    super::super::standalone::combine(outcome,cleanup)
                },
                Err(error)=>{
                    // Preserve the actual failed stage, not merely its string.
                    let cleanup=ConnectError::native("Retain unconfirmed commissioning stage",
                        RetainedStage {stage:std::sync::Mutex::new(Some(stage)),error});
                    super::super::standalone::combine(outcome,Err(cleanup))
                },
            }
        }.await;
        approvals.retire();
        body
    }
    async fn commission_stage(
        &self,
        stage: &mut StagedStockRealm,
        selected: &ProbeReviewedSelection,
        bridge: &PromptBridge,
        admission: &crate::terminal_connector::private_service::Admission,
        approvals: &LocalApprovals,
        presenter: &mut ApprovalPresenter,
        artifacts: &Path,
        observed: &mut CommissionObservation,
    ) -> Result<(), ConnectError> {
        admission.enter().map_err(|_| contract::unavailable())?;
        let mut config = crate::realm::read_config(&self.0.storage).await?;
        config.tools.mcp_servers = vec![selected.config.clone()];
        let model = meerkat::resolve_create_session_model(&config, self.0.model.clone())
            .map_err(|_| contract::unavailable())?;
        let build = meerkat_core::SessionBuildOptions {
            realm_id: Some(self.0.storage.locator.realm.clone()),
            provider: Some(model.provider),
            auth_binding: model.auth_binding,
            ..Default::default()
        };
        let builder = FactoryAgentBuilder::new(self.0.factory.clone(), config);
        let request = CreateSessionRequest {
            model: model.model,
            prompt: String::new().into(),
            injected_context: Vec::new(),
            system_prompt: SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        };
        admission.enter().map_err(|_| contract::unavailable())?;
        let offer = stage
            .prepare_controller(&builder, &request, self.0.session.clone())
            .await
            .map_err(|error| ConnectError::native("Prepare actual probe model route", error))?;
        // No input or model computation is run. The ticket is deliberately
        // disposed by stage.abort, never seated in a different host owner.
        let requests = vec![
            StockGrantRequest {
                agent: selected.request.agent.clone(),
                expires_in_ms: None,
                target: StockGrantTarget::Controller {
                    routes: [offer.report.key.clone()].into_iter().collect(),
                },
            },
            StockGrantRequest {
                agent: selected.request.agent.clone(),
                expires_in_ms: None,
                target: StockGrantTarget::Catalog,
            },
        ];
        let body = json!({"realm":self.0.storage.locator.realm.as_str(),"session_id":self.0.session,
            "job_id":selected.installed.job_id,"agent":selected.request.agent,
            "controller":{"key":offer.report.key.to_string(),"provider":offer.report.provider,
                "model":offer.report.model,"wire_model":offer.report.wire_model,"endpoint":offer.report.endpoint,
                "credential_owner":offer.report.credential_owner},
            "permissions":["exact_controller_route","native_tool_catalog"],
            "lifetime":"Until explicitly revoked. Native controller grants require unrestricted lifetime; logout fences this private caller but does not revoke these saved grants.",
            "separate":"The checked service grant, actual required operation and resource result still need their independent owners and observations."});
        protected_review(
            approvals,
            presenter,
            bridge,
            admission,
            &self.0.session,
            "Allow this exact model route and catalog for the selected service-check agent?",
            body,
        )
        .await?;
        selected.recheck().await?;
        for (index, request) in requests.into_iter().enumerate() {
            admission.enter().map_err(|_| contract::unavailable())?;
            let result = stage.grant_stock_agent(request).await;
            let passed = result.is_ok();
            observed.projection["grants"]
                .as_array_mut()
                .ok_or_else(contract::unavailable)?
                .push(grant_observation(&result));
            observed._grants.push(result);
            record(artifacts, &format!("grant-{index}"), &observed.projection)?;
            if !passed {
                return Err(contract::unavailable());
            }
        }
        admission.enter().map_err(|_| contract::unavailable())?;
        drop(offer);
        Ok(())
    }
}

pub(crate) async fn protected_review(
    approvals: &LocalApprovals,
    presenter: &mut ApprovalPresenter,
    bridge: &PromptBridge,
    admission: &crate::terminal_connector::private_service::Admission,
    session: &SessionId,
    summary: &str,
    body: Value,
) -> Result<(), ConnectError> {
    admission.enter().map_err(|_| contract::unavailable())?;
    let request = crate::terminal_connector::review_request(session, summary, body)?;
    let decision = approvals.review_access(request);
    tokio::pin!(decision);
    loop {
        tokio::select! { biased;
            _=bridge.closed()=>return Err(contract::unavailable()),
            result=decision.as_mut()=>{
                let result=result?;
                admission.enter().map_err(|_|contract::unavailable())?;
                return if result.status==ApprovalStatus::Approved {Ok(())}else{Err(contract::unavailable())};
            },
            prompt=presenter.next()=>{
                let mut prompt=prompt.ok_or_else(contract::unavailable)?;
                let record=prompt.record();
                // Keep polling the native decision deadline while the private
                // presenter waits. Dropping this exact bridge waiter closes
                // its prompt through the existing driver response channel.
                let answer=tokio::select! { biased;
                    _=bridge.closed()=>return Err(contract::unavailable()),
                    result=decision.as_mut()=>{
                        let result=result?;
                        admission.enter().map_err(|_|contract::unavailable())?;
                        return if result.status==ApprovalStatus::Approved {Ok(())}else{Err(contract::unavailable())};
                    },
                    answer=bridge.ask(json!({"kind":"approval","approval_id":record.approval_id,
                        "summary":record.proposed_action.summary,"body":record.proposed_action.body,
                        "allowed_decisions":["approve","deny"],"expires_at":record.expires_at}))=>answer?,
                };
                admission.enter().map_err(|_|contract::unavailable())?;
                let decision=match answer {
                    PromptAnswer::Approval {decision:crate::stdio::wire::Decision::Approve}=>ApprovalDecision::Approve,
                    PromptAnswer::Approval {decision:crate::stdio::wire::Decision::Deny}=>ApprovalDecision::Deny,
                    _=>return Err(contract::unavailable()),
                };
                prompt.decide(decision)?;
            },
        }
    }
}
fn grant_observation(
    result: &Result<meerkat::governed_realm::stock::issuance::StockGrantOutcome, StagedRealmError>,
) -> Value {
    use meerkat::governed_realm::stock::issuance::RevocationOutcome;
    let mut value = json!({"state":"outcome_unknown","issued":null,"candidate_grant_id":null,"superseded":null,"cleanup":[]});
    match result {
        Ok(outcome) => {
            value["state"] = json!("committed");
            value["issued"] = json!(outcome.issued.grant_id.as_str());
            value["superseded"] = json!(
                outcome
                    .superseded
                    .as_ref()
                    .map(|value| value.grant_id.as_str())
            );
            value["cleanup"] = json!(
                outcome
                    .cleanup
                    .iter()
                    .map(|attempt| match attempt.outcome {
                        RevocationOutcome::Revoked => "revoked",
                        RevocationOutcome::ControllerInUse => "controller_in_use",
                        RevocationOutcome::Deferred => "deferred",
                        RevocationOutcome::ReferencesUnknown => "references_unknown",
                        RevocationOutcome::CustodyUnavailable => "custody_unavailable",
                        RevocationOutcome::Refused => "refused",
                        RevocationOutcome::Uncertain => "uncertain",
                    })
                    .collect::<Vec<_>>()
            );
        }
        Err(StagedRealmError::Grant(StockGrantError::Refused(_))) => {
            value["state"] = json!("refused")
        }
        Err(StagedRealmError::Grant(StockGrantError::NotSelected { issued, .. })) => {
            value["state"] = json!("issued_not_selected");
            value["issued"] = json!(issued.grant_id.as_str());
        }
        Err(StagedRealmError::Grant(StockGrantError::IssuanceUncertain { id })) => {
            value["state"] = json!("issuance_unknown");
            value["candidate_grant_id"] = json!(id.as_str());
        }
        Err(StagedRealmError::Grant(StockGrantError::SelectionUncertain {
            issued,
            previous,
            ..
        })) => {
            value["state"] = json!("selection_unknown");
            value["issued"] = json!(issued.grant_id.as_str());
            value["superseded"] = json!(previous.as_ref().map(|value| value.grant_id.as_str()));
        }
        Err(_) => (),
    }
    value
}
pub(crate) fn record(directory: &Path, name: &str, value: &Value) -> Result<(), ConnectError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let bytes = serde_json::to_vec(value).map_err(|_| contract::unavailable())?;
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(directory.join(format!("service-commission-{name}.json")))?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::File::open(directory)?.sync_all()
        })();
        result.map_err(|error| {
            ConnectError::native("Retain commissioning observation without retry", error)
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (directory, name, value);
        Err(contract::unavailable())
    }
}
struct RetainedStage {
    stage: std::sync::Mutex<Option<StagedStockRealm>>,
    error: StagedRealmError,
}
impl std::fmt::Debug for RetainedStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = (&self.stage, &self.error);
        f.write_str("RetainedCommissioningStage")
    }
}
impl std::fmt::Display for RetainedStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The actual commissioning stage did not confirm cleanup; it remains retained")
    }
}
impl std::error::Error for RetainedStage {}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use tokio::sync::watch;

    #[tokio::test]
    async fn actual_commission_review_retains_native_decision_and_refuses_retired_viewer() {
        for retire_before_answer in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let session = SessionId::new();
            let (approvals, mut presenter) = LocalApprovals::create_browser(
                root.path(),
                ApprovalPrincipalId::new("viewer").unwrap(),
                SystemTime::now() + Duration::from_secs(60),
            )
            .unwrap();
            approvals.bind_session(&session).unwrap();
            let (bridge, mut prompts, retire_bridge) = PromptBridge::channel();
            let (retire, receive) = watch::channel(false);
            let admission = crate::terminal_connector::private_service::Admission::new(
                contract::now() + 60_000,
                receive,
            );
            let exact = json!({"realm":"one","session_id":session,"job_id":"installed-job","agent":"selected-agent",
                "permissions":["exact_controller_route","native_tool_catalog"]});
            let owner = protected_review(
                &approvals,
                &mut presenter,
                &bridge,
                &admission,
                &session,
                "Exact native review",
                exact.clone(),
            );
            let human = async {
                let observed = std::panic::AssertUnwindSafe(async {
                    let prompt = tokio::time::timeout(Duration::from_secs(3), prompts.recv())
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(prompt.prompt["body"], exact);
                    assert_eq!(prompt.prompt["kind"], "approval");
                    assert!(prompt.prompt["approval_id"].as_str().is_some());
                    if retire_before_answer {
                        retire.send_replace(true);
                    }
                    let _ = prompt.response.send(Ok(PromptAnswer::Approval {
                        decision: crate::stdio::wire::Decision::Approve,
                    }));
                })
                .catch_unwind()
                .await;
                if observed.is_err() {
                    retire.send_replace(true);
                    retire_bridge.send_replace(true);
                }
                observed
            };
            let (outcome, human) =
                tokio::join!(std::panic::AssertUnwindSafe(owner).catch_unwind(), human);
            approvals.retire();
            retire_bridge.send_replace(true);
            retire.send_replace(true);
            if let Err(panic) = human {
                std::panic::resume_unwind(panic);
            }
            match outcome {
                Ok(result) => assert_eq!(result.is_ok(), !retire_before_answer),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
    }

    #[tokio::test]
    async fn protected_review_joins_its_native_deadline_and_pending_cancellation() {
        for cancel in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let session = SessionId::new();
            // Exercise the real decision timer with a test-only short bound.
            // Actor attribution and the admission remain valid for 60 s. A
            // short actor expiry alone does not shorten the 120 s decision
            // timer, and this isolated channel has no host-lifetime watcher.
            let decision_lifetime = if cancel {
                Duration::from_secs(60)
            } else {
                Duration::from_millis(500)
            };
            let (approvals, mut presenter) =
                LocalApprovals::create_browser_with_test_decision_lifetime(
                    root.path(),
                    ApprovalPrincipalId::new("viewer").unwrap(),
                    SystemTime::now() + Duration::from_secs(60),
                    decision_lifetime,
                )
                .unwrap();
            approvals.bind_session(&session).unwrap();
            let (bridge, mut prompts, retire_bridge) = PromptBridge::channel();
            let (retire, receive) = watch::channel(false);
            let admission = crate::terminal_connector::private_service::Admission::new(
                contract::now() + 60_000,
                receive,
            );
            let owner = protected_review(
                &approvals,
                &mut presenter,
                &bridge,
                &admission,
                &session,
                "Bounded review",
                json!({"session_id":session,"exact":"same review"}),
            );
            let observer = async {
                let mut prompt = prompts.recv().await.unwrap();
                assert_eq!(prompt.prompt["kind"], "approval");
                if cancel {
                    retire_bridge.send_replace(true);
                }
                // No synthetic answer and no replacement presenter. The exact
                // sender closes when the original native review settles.
                prompt.response.closed().await;
            };
            let result = tokio::time::timeout(Duration::from_secs(3), async {
                tokio::join!(owner, observer)
            })
            .await;
            approvals.retire();
            retire.send_replace(true);
            retire_bridge.send_replace(true);
            assert!(
                result
                    .expect("unanswered review must settle at its original bound")
                    .0
                    .is_err()
            );
        }
    }

    #[test]
    fn uncertain_native_command_projection_never_exposes_error_text_or_known_success() {
        let result: Result<
            meerkat::governed_realm::stock::issuance::StockGrantOutcome,
            StagedRealmError,
        > = Err(StagedRealmError::Grant(StockGrantError::CommandUncertain));
        let value = grant_observation(&result);
        assert_eq!(
            value,
            json!({"state":"outcome_unknown","issued":null,"candidate_grant_id":null,
            "superseded":null,"cleanup":[]})
        );
        assert!(!value.to_string().contains("reason"));
    }
}
