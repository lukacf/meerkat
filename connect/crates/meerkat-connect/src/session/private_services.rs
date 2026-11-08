//! Reviewed setup grants precede the single host-authenticated runtime open.
//! The administrative stage is aborted; its ticket and auth pair never leave it.
use super::*;
use crate::{
    approval::{ApprovalPresenter, LocalApprovals},
    service_preparation::private_native::{BUNDLE, PrivateNativeSetup},
    stdio::PromptBridge,
    terminal_connector::private_service::Admission,
};
use meerkat::governed_realm::{
    staged::{
        StagedIssuanceEffect, StagedRealmError, StagedStockRealm, stage_stock_realm_persistence_as,
    },
    stock::{
        StockPrincipals,
        issuance::{
            DEFAULT_STOCK_AGENT, McpOperations, ReviewedMcpInstallation, StockGrantRequest,
            StockGrantTarget, grant_stock_agent_for_reviewed_installation,
        },
    },
};
use meerkat_core::{AdministrationEntry, McpServerConfig};
use serde_json::json;
use std::path::Path;

impl SessionStartup {
    pub(crate) async fn review_private_helper(
        &self,
        model: &CreateSessionModelResolutionRequest,
        session: &SessionId,
        helper: &McpServerConfig,
        setup: &PrivateNativeSetup,
        approvals: &LocalApprovals,
        presenter: &mut ApprovalPresenter,
        bridge: &PromptBridge,
        admission: &Admission,
        artifacts: &Path,
    ) -> Result<(), ConnectError> {
        current(admission)?;
        if self.persistence.is_some()
            || self.standalone.is_some()
            || self.connector_auth.is_some()
            || meerkat_store::read_realm_posture_in(
                &self.storage.locator.state_root,
                self.storage.locator.realm.as_str(),
            )
            .await
            .map_err(|error| ConnectError::native("Read next-service governance", error))?
                != meerkat_store::RealmPosture::Governed
        {
            return Err(unavailable());
        }
        approvals.check_session(session)?;
        let runtime = crate::realm::own_config_runtime(&self.storage)?;
        let (servers, generation) = meerkat_core::McpRealmPersistTarget::new(&runtime)
            .servers()
            .await
            .map_err(|error| ConnectError::native("Read private helper installation", error))?;
        let matches: Vec<_> = servers
            .iter()
            .enumerate()
            .filter(|(_, row)| row.name == helper.name)
            .collect();
        if matches.len() > 1 {
            return Err(unavailable());
        }
        let previous = matches.first().map(|(_, row)| (*row).clone());
        let mut candidate = servers.clone();
        match matches.first() {
            Some((index, _)) => candidate[*index] = helper.clone(),
            None => candidate.push(helper.clone()),
        }
        let delta = meerkat_core::ConfigDelta(json!({"tools":{"mcp_servers":candidate}}));
        runtime
            .patch_preview(&delta)
            .await
            .map_err(|error| ConnectError::native("Preview private helper installation", error))?
            .validate_for_write(meerkat_models::canonical(), &servers)
            .map_err(|error| ConnectError::native("Validate private helper installation", error))?;
        let helper_operations = crate::setup::PREPARATION_TOOLS
            .iter()
            .chain(crate::setup::ACQUISITION_TOOLS.iter())
            .map(|name| (*name).to_owned())
            .collect();
        let helper_requests = [
            StockGrantRequest {
                agent: DEFAULT_STOCK_AGENT.into(),
                expires_in_ms: None,
                target: StockGrantTarget::Mcp {
                    server: helper.name.clone(),
                    operations: McpOperations::Exact(helper_operations),
                },
            },
            StockGrantRequest {
                agent: DEFAULT_STOCK_AGENT.into(),
                expires_in_ms: None,
                target: StockGrantTarget::CheckedMcpRoute {
                    route: crate::service_install::CHECKED_INSTALL_ROUTE.into(),
                    server: helper.name.clone(),
                    operations: crate::service_install::checked_install_operations(),
                },
            },
        ];
        current(admission)?;
        let mut stage = stage_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider,
            &self.storage.locator.state_root,
            self.storage.locator.realm.as_str(),
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(self.storage.layout.clone()),
            DEFAULT_STOCK_AGENT,
        )
        .await
        .map_err(|error| ConnectError::native("Stage next-service controller review", error))?;
        let outcome = async {
            current(admission)?;
            let model = resolve_create_session_model(&self.config, model.clone())
                .map_err(|error| ConnectError::native("Resolve next-service controller", error))?;
            let build = SessionBuildOptions {
                realm_id: Some(self.storage.locator.realm.clone()),
                provider: Some(model.provider),
                auth_binding: model.auth_binding,
                ..Default::default()
            };
            let request = CreateSessionRequest { model:model.model, prompt:String::new().into(),
                injected_context:Vec::new(), system_prompt:SystemPromptOverride::Inherit, max_tokens:None,
                event_tx:None, initial_turn:InitialTurnPolicy::Defer,
                deferred_prompt_policy:DeferredPromptPolicy::Discard, build:Some(build), labels:None };
            let builder = FactoryAgentBuilder::new(self.factory.clone(), self.config.clone());
            let offer = stage.prepare_controller(&builder, &request, session.clone()).await
                .map_err(|error| ConnectError::native("Prepare next-service controller route", error))?;
            let requests: Vec<_> = std::iter::once(StockGrantTarget::Controller {
                    routes: [offer.report.key.clone()].into_iter().collect() })
                .chain(std::iter::once(StockGrantTarget::Catalog))
                .chain(setup.operations().iter().map(|operation| StockGrantTarget::ToolBundleOperation {
                    bundle:BUNDLE.into(), operation:operation.clone() }))
                .map(|target| StockGrantRequest {agent:DEFAULT_STOCK_AGENT.into(), expires_in_ms:None, target}).collect();
            let body = json!({"change":"review_next_service_setup","realm":self.storage.locator.realm.as_str(),
                "session_id":session,"agent":DEFAULT_STOCK_AGENT,"previous_setup":previous,
                "supplied_setup":helper,"reviewed_generation":generation,
                "controller":{"key":offer.report.key.to_string(),"provider":offer.report.provider,
                    "model":offer.report.model,"wire_model":offer.report.wire_model,
                    "endpoint":offer.report.endpoint,"credential_owner":offer.report.credential_owner},
                "helper_grants":helper_requests.iter().map(|r|format!("{r:?}")).collect::<Vec<_>>(),
                "setup_grants":requests.iter().map(|r|format!("{r:?}")).collect::<Vec<_>>(),
                "lifetime":"Until revoked. Logout fences this private caller but does not revoke saved grants.",
                "consequence":"Approved configuration and grants remain saved if a later step fails. Package installation and service access still require their separate reviews."});
            super::service_probe::commission::protected_review(approvals,presenter,bridge,admission,session,
                "Allow this prepared helper and exact model route for the next service setup?",body).await?;
            current(admission)?;
            let current_config = meerkat_core::McpRealmPersistTarget::new(&runtime).servers().await
                .map_err(|error| ConnectError::native("Recheck reviewed helper configuration",error))?;
            if current_config != (servers.clone(),generation) { return Err(unavailable()); }
            for (index, request) in requests.into_iter().enumerate() {
                current(admission)?;
                let result = stage.grant_stock_agent(request).await;
                let recorded = record(artifacts,&format!("setup-grant-{index}"),&json!({"outcome":format!("{result:?}")}));
                super::standalone::combine(result.map(|_| ()).map_err(|error|
                    ConnectError::native("Issue reviewed private setup grant",error)),recorded)?;
            }
            drop(offer);
            Ok(())
        }.await;
        // The entire entered grant and disposal are joined, including on deny,
        // logout or output loss. No controller ticket is seated in another owner.
        let cleanup = match stage.abort().await {
            Ok(disposal) => {
                let settled = disposal.custody == meerkat_runtime::StagedCustodyRelease::Released
                    && matches!(
                        disposal.controller,
                        meerkat::PreparedControllerDisposal::NotPrepared
                            | meerkat::PreparedControllerDisposal::Drained
                    )
                    && disposal.unobserved_grants.is_empty()
                    && disposal.issuance != StagedIssuanceEffect::Uncertain;
                let recorded = record(
                    artifacts,
                    "setup-stage-close",
                    &json!({"settled":settled,"disposal":format!("{disposal:?}")}),
                );
                let cleanup = if settled {
                    Ok(())
                } else {
                    Err(ConnectError::native(
                        "Retain unconfirmed next-service review stage",
                        RetainedStage {
                            _stage: std::sync::Mutex::new(Some(stage)),
                            _error: None,
                            _disposal: Some(disposal),
                        },
                    ))
                };
                super::standalone::combine(cleanup, recorded)
            }
            Err(error) => Err(ConnectError::native(
                "Retain next-service review stage",
                RetainedStage {
                    _stage: std::sync::Mutex::new(Some(stage)),
                    _error: Some(error),
                    _disposal: None,
                },
            )),
        };
        super::standalone::combine(outcome, cleanup)?;
        current(admission)?;
        let observed = meerkat_core::McpRealmPersistTarget::new(&runtime)
            .servers()
            .await
            .map_err(|error| ConnectError::native("Recheck private helper before save", error))?;
        if observed != (servers, generation) {
            return Err(unavailable());
        }
        let generation = if previous.as_ref() != Some(helper) {
            let outcome = runtime.patch(delta, Some(generation)).await;
            let recorded = record(
                artifacts,
                "setup-configuration",
                &json!({"outcome":format!("{outcome:?}")}),
            );
            super::standalone::combine(
                outcome
                    .map_err(|error| ConnectError::native("Save reviewed private helper", error)),
                recorded,
            )?
            .generation
        } else {
            generation
        };
        let reviewed = ReviewedMcpInstallation {
            server: helper.clone(),
            generation,
        };
        let principals = StockPrincipals::for_local_operator(&self.storage.locator.realm)
            .map_err(|error| ConnectError::native("Identify private setup grant root", error))?;
        for (index, request) in helper_requests.iter().enumerate() {
            current(admission)?;
            let outcome = grant_stock_agent_for_reviewed_installation(
                &self.storage,
                &principals,
                request,
                &reviewed,
            )
            .await;
            let recorded = record(
                artifacts,
                &format!("helper-grant-{index}"),
                &json!({"outcome":format!("{outcome:?}")}),
            );
            super::standalone::combine(
                outcome.map(|_| ()).map_err(|error| {
                    ConnectError::native("Issue reviewed private helper access", error)
                }),
                recorded,
            )?;
        }
        current(admission)
    }

    pub(crate) async fn materialize_private_services(
        mut self,
        mut options: SessionOptions,
        session: Session,
        setup: &Arc<PrivateNativeSetup>,
        helper: &McpServerConfig,
    ) -> Result<ConnectSession, ConnectError> {
        self.config = crate::realm::read_config(&self.storage).await?;
        self.config.tools.mcp_servers = vec![helper.clone()];
        let model = resolve_create_session_model(&self.config, options.model)
            .map_err(|error| ConnectError::native("Resolve governed private setup model", error))?;
        options.build.realm_id = Some(self.storage.locator.realm.clone());
        options.build.provider = Some(model.provider);
        options.build.auth_binding = model.auth_binding;
        let research = setup
            .research()
            .map(|binding| {
                binding.bind(
                    &self.config,
                    model.provider,
                    &model.model,
                    &mut options.build,
                )
            })
            .transpose()?;
        let persistence = self.persistence.take().ok_or_else(unavailable)?;
        let stock = persistence
            .stock_governance()
            .cloned()
            .ok_or_else(unavailable)?;
        let mut builder = FactoryAgentBuilder::new(self.factory, self.config);
        builder.wait_for_mcp = true;
        let request = CreateSessionRequest {
            model: model.model,
            prompt: String::new().into(),
            injected_context: Vec::new(),
            system_prompt: SystemPromptOverride::Set(format!(
                "{SETUP_INSTRUCTIONS}\nThe selected native realm is {}.\nThe supplied service preparation is already retained. toolkit_prepare_services is idempotent.\n",
                self.storage.locator.realm.as_str()
            )),
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(options.build),
            labels: None,
        };
        let mut opened = ConnectSession::materialize_governed(
            &self.storage,
            builder,
            persistence,
            session,
            request,
            stock,
            research,
            self.connector_auth.take(),
            None,
            std::slice::from_ref(&helper.name),
        )
        .await?;
        match setup.owner() {
            Ok(owner) => opened.service_preparation = Some(owner),
            Err(primary) => return super::standalone::combine(Err(primary), opened.close().await),
        }
        Ok(opened)
    }
}

fn current(admission: &Admission) -> Result<(), ConnectError> {
    admission.enter().map(|_| ()).map_err(|_| unavailable())
}
fn unavailable() -> ConnectError {
    ConnectError::InvalidInput("The reviewed governed private setup is unavailable".into())
}
fn record(path: &Path, name: &str, value: &serde_json::Value) -> Result<(), ConnectError> {
    super::service_probe::commission::record(path, name, value)
}
struct RetainedStage {
    _stage: std::sync::Mutex<Option<StagedStockRealm>>,
    _error: Option<StagedRealmError>,
    _disposal: Option<meerkat::governed_realm::staged::StagedAbortOutcome>,
}
impl std::fmt::Debug for RetainedStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetainedPrivateReviewStage")
    }
}
impl std::fmt::Display for RetainedStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Private setup review stage cleanup is unconfirmed; its owner remains retained")
    }
}
impl std::error::Error for RetainedStage {}
