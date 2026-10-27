//! The dedicated terminal's single staged native setup.
//!
//! The pre-stage reviewed MCP command and the stage's issuance outcomes are
//! separate historical effects. Neither a review nor a saved outcome admits
//! an input; the opened stock owner binds every prompt through PendingOriginal.

use std::{future::Future, io::{self, Write}, path::PathBuf, pin::Pin, sync::Arc};

use futures_util::FutureExt;
use meerkat::{FactoryAgentBuilder, Session, controller_route::ControllerRouteReport,
    governed_realm::{staged::StagedStockRealm, stock::issuance::{
        DEFAULT_STOCK_AGENT, StockGrantOutcome, StockGrantRequest, StockGrantTarget,
    }}};
use meerkat_core::{ApprovalStatus, CreateSessionRequest, McpServerConfig, SessionId};

use super::{ConnectSession, SessionOptions, SessionStartup};
use crate::{ConnectError, approval::{ApprovalPresenter, LocalApprovals, present_approval,
    present_controller_setup_approval, present_supplied_setup_approval},
    service_preparation::NativeSetup, terminal::{TerminalInput, review_text}};

pub(crate) struct StagedSetup {
    pub stage: StagedStockRealm,
    pub session: Session,
    pub server: McpServerConfig,
    pub artifacts: PathBuf,
    // Retain both separately settled pre-stage results. Neither is included in
    // StagedIssuanceEffect and is not rolled back by aborting the stage.
    pub reviewed_mcp: StockGrantOutcome,
    pub reviewed_checked_mcp: StockGrantOutcome,
    // Observe only the actual finished bundle before the existing consumer moves it.
    #[cfg(test)]
    materialization_observer: Option<Box<dyn FnOnce(&meerkat::PersistenceBundle) + Send + Sync>>,
}

/// Non-authority input retained from this invocation's actual reviewed setup.
/// No native stage, grant selection, ticket, actor or prepared client survives
/// here. Reopening checks the original descriptor against the realm's own list.
pub struct StandaloneContinuation {
    storage: meerkat_core::ResolvedStorage,
    session_id: SessionId,
    server: McpServerConfig,
}

/// A fresh ordinary governed opener, used once after the previous runtime and
/// administrative command have settled. It does not repeat startup or issuance.
pub struct ResumedStandalone {
    startup: SessionStartup,
    session: Session,
    server: McpServerConfig,
}

impl StandaloneContinuation {
    pub async fn reopen(&self) -> Result<ResumedStandalone, ConnectError> {
        let runtime = crate::realm::own_config_runtime(&self.storage)?;
        let (servers, _) = meerkat_core::McpRealmPersistTarget::new(&runtime).servers().await
            .map_err(|error| ConnectError::native("Read continuation setup connection", error))?;
        require_same_setup_server(&servers, &self.server)?;
        let (_, persistence) = meerkat::governed_realm::open_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider, &self.storage.locator.state_root,
            self.storage.locator.realm.as_str(), None, Some(meerkat_store::RealmOrigin::Explicit),
            Some(self.storage.layout.clone()), DEFAULT_STOCK_AGENT,
        ).await.map_err(|error| ConnectError::native("Reopen governed setup conversation", error))?;
        if persistence.stock_governance().is_none() {
            return Err(invalid("The continuation has no current governed owner; setup authority was not created or widened"));
        }
        let session = persistence.session_store().load(&self.session_id).await
            .map_err(|error| ConnectError::native("Load committed setup conversation", error))?
            .ok_or_else(|| invalid("The committed setup conversation is unavailable; a replacement session was not created"))?;
        let mut startup = SessionStartup::open(self.storage.clone()).await?;
        // Keep the original supplied path and exact descriptor. The current
        // own-list is checked again after acquiring native runtime custody.
        require_same_setup_server(&startup.config.tools.mcp_servers, &self.server)?;
        let (servers, _) = meerkat_core::McpRealmPersistTarget::new(&runtime).servers().await
            .map_err(|error| ConnectError::native("Recheck continuation setup connection", error))?;
        require_same_setup_server(&servers, &self.server)?;
        startup.persistence = Some(persistence);
        Ok(ResumedStandalone { startup, session, server: self.server.clone() })
    }
}

fn require_same_setup_server(servers: &[McpServerConfig], reviewed: &McpServerConfig) -> Result<(), ConnectError> {
    let mut matching = servers.iter().filter(|entry| entry.name == reviewed.name);
    if matching.next() != Some(reviewed) || matching.next().is_some() {
        return Err(invalid("The originally reviewed setup connection changed or disappeared. No replacement helper or setup grant was created."));
    }
    Ok(())
}

impl ResumedStandalone {
    pub fn startup(&mut self) -> &mut SessionStartup { &mut self.startup }

    pub async fn open_native_setup<I: Future<Output = io::Result<()>> + Send, W: Write>(
        self, options: SessionOptions, setup: Arc<NativeSetup>,
        output: &mut W, mut interrupt: Pin<&mut I>,
    ) -> Result<Option<ConnectSession>, ConnectError> {
        let opened = self.open_inner(options, setup.clone());
        let (result, stopped, notice) = join_entered(opened, output, interrupt.as_mut()).await;
        let Some(result) = result else {
            setup.retire();
            stopped_result(stopped)?;
            return Ok(None);
        };
        let session = match result {
            Ok(session) => session,
            Err(primary) => { setup.retire(); return combine(Err(primary), notice); }
        };
        match combine(stopped_result(stopped), notice) {
            Ok(false) => Ok(Some(session)),
            stopped => {
                setup.retire();
                combine(stopped.map(|_| ()), session.close().await)?;
                Ok(None)
            }
        }
    }

    async fn open_inner(self, mut options: SessionOptions, setup: Arc<NativeSetup>) -> Result<ConnectSession, ConnectError> {
        let Self { mut startup, session, server } = self;
        if setup.session_id() != session.id() || startup.standalone.is_some() {
            return Err(invalid("The continuation composition does not name the committed setup session"));
        }
        options = setup.session_options(options, &server)?;
        if options.build.realm_id.as_ref().is_some_and(|realm| realm != &startup.storage.locator.realm)
            || options.model.provider.zip(options.build.provider).is_some_and(|(a,b)| a != b)
            || options.model.auth_binding.as_ref().zip(options.build.auth_binding.as_ref()).is_some_and(|(a,b)| a != b)
        { return Err(invalid("The continuation model and build must select the same realm and authentication")); }
        options.model.provider = options.model.provider.or(options.build.provider);
        options.model.auth_binding = options.model.auth_binding.or(options.build.auth_binding.clone());
        let model = meerkat::resolve_create_session_model(&startup.config, options.model)
            .map_err(|error| ConnectError::native("Resolve continuation model", error))?;
        options.build.realm_id = Some(startup.storage.locator.realm.clone());
        options.build.provider = Some(model.provider);
        options.build.auth_binding = model.auth_binding;
        options.build.resume_existing_session(session.clone());
        let research = setup.research().map(|binding|
            binding.bind(&startup.config, model.provider, &model.model, &mut options.build)
        ).transpose()?;
        let mut factory = setup.configure_factory(startup.factory.clone());
        if let Some(auth) = &startup.connector_auth {
            factory = factory.mcp_call_context_provider(auth.context_provider.clone());
        }
        let mut builder = FactoryAgentBuilder::new(factory, startup.config.clone());
        // Retain the live round guard until this builder's native MCP
        // connections settle and the original helper handshake is observed.
        builder.wait_for_mcp = true;
        let request = CreateSessionRequest {
            model: model.model, prompt: String::new().into(), injected_context: Vec::new(),
            // The committed conversation already owns its setup instructions.
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: None, event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(options.build), labels: None,
        };
        let persistence = startup.persistence.take().ok_or_else(|| invalid("The fresh governed continuation owner is unavailable"))?;
        let stock = persistence.stock_governance().cloned().ok_or_else(|| invalid("Current setup grants are unavailable"))?;
        ConnectSession::materialize_native_setup(&startup.storage, builder, persistence, session, request,
            stock, research, startup.connector_auth.take(), setup).await
    }
}

impl SessionStartup {
    /// Capture only the descriptor actually reviewed by this invocation.
    pub fn standalone_continuation(&self) -> Result<StandaloneContinuation, ConnectError> {
        let staged = self.standalone.as_ref().ok_or_else(|| invalid("The initial reviewed setup stage is unavailable"))?;
        Ok(StandaloneContinuation { storage: self.storage.clone(), session_id: staged.session.id().clone(),
            server: staged.server.clone() })
    }
    /// Review the supplied setup server and its access before entering native
    /// grant issuance. The exact MCP command is settled before stage custody.
    /// A stopped observer never causes a replay or an inferred rollback.
    pub async fn prepare_standalone<I: Future<Output = io::Result<()>>, W: Write>(
        storage: meerkat_core::ResolvedStorage, session: Session,
        prepared: crate::resources::PreparedResources,
        approvals: &LocalApprovals, presenter: &mut ApprovalPresenter,
        input: &mut TerminalInput, output: &mut W, interrupt: Pin<&mut I>,
    ) -> Result<Option<Self>, ConnectError> {
        let runtime = crate::realm::own_config_runtime(&storage)?;
        Self::prepare_standalone_with_config(storage, session, prepared, runtime,
            approvals, presenter, input, output, interrupt).await
    }

    // The public entry fixes this dependency to the selected realm's own
    // native runtime. The rest of the flow uses that one concrete owner.
    async fn prepare_standalone_with_config<I: Future<Output = io::Result<()>>, W: Write>(
        storage: meerkat_core::ResolvedStorage, session: Session,
        prepared: crate::resources::PreparedResources, runtime: meerkat_core::ConfigRuntime,
        approvals: &LocalApprovals, presenter: &mut ApprovalPresenter,
        input: &mut TerminalInput, output: &mut W, mut interrupt: Pin<&mut I>,
    ) -> Result<Option<Self>, ConnectError> {
        use meerkat::governed_realm::{staged::stage_stock_realm_persistence_as,
            stock::{StockPrincipals, govern_stock_realm, issuance::{McpOperations,
                ReviewedMcpInstallation, grant_stock_agent_for_reviewed_installation}}};
        let mut server = prepared.connection.into_server();
        let operations: std::collections::BTreeSet<String> = crate::setup::PREPARATION_TOOLS.iter()
            .chain(crate::setup::ACQUISITION_TOOLS.iter()).map(|name| (*name).to_owned()).collect();
        server.access = Some(meerkat_core::McpServerAccess::allowing(operations.iter().cloned()));
        meerkat_core::mcp_config::reject_realm_server_env_references(std::slice::from_ref(&server))
            .map_err(|e| ConnectError::native("Validate supplied setup server", e))?;
        server.validate_access().map_err(|e| ConnectError::native("Validate setup operation ceiling", e))?;
        approvals.check_session(session.id())?;
        let (servers, generation) = meerkat_core::McpRealmPersistTarget::new(&runtime).servers().await
            .map_err(|e| ConnectError::native("Read selected realm setup configuration", e))?;
        let matches: Vec<_> = servers.iter().enumerate().filter(|(_, entry)| entry.name == server.name).collect();
        if matches.len() > 1 { return Err(invalid("The selected realm has conflicting setup server entries")); }
        let previous = matches.first().map(|(_, entry)| (*entry).clone());
        let changed = previous.as_ref() != Some(&server);
        let mut candidate = servers.clone();
        match matches.first() {
            Some((index, _)) => candidate[*index] = server.clone(),
            None => candidate.push(server.clone()),
        }
        let delta = meerkat_core::ConfigDelta(serde_json::json!({"tools":{"mcp_servers": candidate}}));
        runtime.patch_preview(&delta).await
            .map_err(|e| ConnectError::native("Preview setup configuration", e))?
            .validate_for_write(meerkat_models::canonical(), &servers)
            .map_err(|e| ConnectError::native("Validate setup configuration change", e))?;
        let posture = meerkat_store::read_realm_posture_in(&storage.locator.state_root, storage.locator.realm.as_str()).await
            .map_err(|e| ConnectError::native("Read selected realm governance", e))?;
        let request = StockGrantRequest { agent: DEFAULT_STOCK_AGENT.to_owned(),
            target: StockGrantTarget::Mcp { server: server.name.clone(), operations: McpOperations::Exact(operations.clone()) },
            expires_in_ms: None };
        let checked_operations = crate::service_install::checked_install_operations();
        let checked_request = StockGrantRequest { agent: DEFAULT_STOCK_AGENT.to_owned(),
            target: StockGrantTarget::CheckedMcpRoute {
                route: crate::service_install::CHECKED_INSTALL_ROUTE.to_owned(),
                server: server.name.clone(), operations: checked_operations.clone(),
            }, expires_in_ms: None };
        let body = serde_json::json!({
            "realm": storage.locator.realm.as_str(), "agent": DEFAULT_STOCK_AGENT,
            "purpose": "Prepare and inspect the service or frontend you choose. Installation and account access still require their own reviews.",
            "realm_change": if posture == meerkat_store::RealmPosture::Governed { "Keep this governed realm and its existing grant owner" } else { "Create or convert this realm to native governed operation; this persists if later setup stops" },
            "setup_change": if changed { "Save this exact supplied setup connection, replacing only the shown prior setup connection when present" } else { "Reuse this exact saved setup connection" },
            "previous_setup": previous, "supplied_setup": server,
            "reviewed_generation": generation, "operations": operations,
            "checked_installation": {
                "route": crate::service_install::CHECKED_INSTALL_ROUTE,
                "server": server.name, "operations": checked_operations,
                "scope": "Available only through a live checked setup session. Each installation still requires its exact retained check and separate human review; raw MCP access does not include these operations.",
            },
            "lifetime": "Until revoked. A later failure or cancellation does not revoke an issued grant or undo saved realm changes.",
        });
        if !review(approvals, presenter, session.id(),
            "Allow the supplied setup helper to prepare choices in this realm?", body.clone(),
            SetupReviewPresentation::SuppliedHelper,
            input, output, interrupt.as_mut()).await? { return Ok(None); }
        record(&prepared.artifact_directory, "setup-review", &serde_json::to_string_pretty(&body)
            .map_err(|e| ConnectError::native("Retain exact setup review", e))?)?;
        if posture == meerkat_store::RealmPosture::NotMaterialized {
            let materializing = async {
                let (_, bundle) = meerkat::storage_provider::open_realm_persistence_with_layout(
                    storage.layout.clone(), storage.locator.realm.as_str(), None,
                    Some(meerkat_store::RealmOrigin::Explicit),
                ).await?;
                drop(bundle);
                Ok::<_, meerkat::PersistenceError>(())
            };
            let (outcome, stopped, notice) = join_entered(materializing, output, interrupt.as_mut()).await;
            let Some(outcome) = outcome else { stopped_result(stopped)?; return Ok(None); };
            let retained = record(&prepared.artifact_directory, "realm-materialization", &format!("{outcome:#?}"));
            let shown = report(output, if outcome.is_ok() {
                "The selected realm was created. It remains saved if this setup stops."
            } else {
                "Realm creation failed and may have left saved state. No setup access was requested."
            });
            combine(outcome.map_err(|e| ConnectError::native("Create selected native realm", e)),
                combine(notice, combine(retained, shown)))?;
            if stopped_result(stopped)? { return Ok(None); }
        }
        let principals = StockPrincipals::for_local_operator(&storage.locator.realm)
            .map_err(|e| ConnectError::native("Identify selected realm operator", e))?;
        let (outcome, stopped, notice) = join_entered(govern_stock_realm(
            &storage.locator.state_root, storage.locator.realm.as_str(), &principals,
        ), output, interrupt.as_mut()).await;
        let Some(outcome) = outcome else { stopped_result(stopped)?; return Ok(None); };
        let retained = record(&prepared.artifact_directory, "realm-governance", &format!("{outcome:#?}"));
        let shown = report(output, match (&outcome, posture) {
            (Ok(_), meerkat_store::RealmPosture::Governed) => "The existing governed realm and its grant owner were retained.",
            (Ok(_), _) => "The realm is now governed. This saved change is not undone if setup stops.",
            (Err(_), _) => "Opening governed realm authority failed; saved realm changes may remain. No setup access was requested.",
        });
        combine(outcome.map_err(|e| ConnectError::native("Open selected governed realm authority", e)),
            combine(notice, combine(retained, shown)))?;
        if stopped_result(stopped)? { return Ok(None); }
        // Recheck the exact own-list used by the review. The native patch
        // below owns writer exclusion and the expected-generation decision.
        let current = meerkat_core::McpRealmPersistTarget::new(&runtime).servers().await
            .map_err(|e| ConnectError::native("Recheck reviewed setup configuration", e))?;
        if current != (servers, generation) { return Err(invalid("The reviewed realm configuration changed. No setup grant was requested.")); }
        let generation = if changed {
            let (outcome, stopped, notice) = join_entered(runtime.patch(delta, Some(generation)), output, interrupt.as_mut()).await;
            let Some(outcome) = outcome else { stopped_result(stopped)?; return Ok(None); };
            let observed = match &outcome {
                Ok(snapshot) => format!("Saved reviewed setup descriptor at native generation {}", snapshot.generation),
                Err(error) => format!("Native configuration write failed and may be partial: {error:?}"),
            };
            let retained = record(&prepared.artifact_directory, "setup-configuration", &observed);
            let shown = report(output, if outcome.is_ok() {
                "The reviewed setup connection was saved. Other configured connections were preserved."
            } else {
                "Saving the setup connection failed and may be partial. No setup grant or retry was started."
            });
            let snapshot = combine(outcome.map_err(|e| ConnectError::native("Save reviewed setup configuration; outcome may be partial", e)),
                combine(notice, combine(retained, shown)))?;
            if stopped_result(stopped)? { return Ok(None); }
            snapshot.generation
        } else {
            report(output, "The exact reviewed setup connection was already saved and was reused.")?;
            generation
        };
        let installation = ReviewedMcpInstallation { server: server.clone(), generation };
        record(&prepared.artifact_directory, "mcp-grant-request", &format!("{request:#?}\n{installation:#?}"))?;
        let (outcome, stopped, notice) = join_entered(grant_stock_agent_for_reviewed_installation(
            &storage, &principals, &request, &installation,
        ), output, interrupt.as_mut()).await;
        let Some(outcome) = outcome else { stopped_result(stopped)?; return Ok(None); };
        let retained = record(&prepared.artifact_directory, "mcp-grant-outcome", &format!("{outcome:#?}"));
        let shown = report_grant(output, "Setup helper access", outcome.as_ref());
        let reviewed_mcp = combine(outcome.map_err(|e| ConnectError::native("Issue reviewed setup access", e)),
            combine(notice, combine(retained, shown)))?;
        if stopped_result(stopped)? { return Ok(None); }
        record(&prepared.artifact_directory, "checked-mcp-grant-request", &format!("{checked_request:#?}\n{installation:#?}"))?;
        let (outcome, stopped, notice) = join_entered(grant_stock_agent_for_reviewed_installation(
            &storage, &principals, &checked_request, &installation,
        ), output, interrupt.as_mut()).await;
        let Some(outcome) = outcome else { stopped_result(stopped)?; return Ok(None); };
        let retained = record(&prepared.artifact_directory, "checked-mcp-grant-outcome", &format!("{outcome:#?}"));
        let shown = report_grant(output, "Checked installation route access", outcome.as_ref());
        let reviewed_checked_mcp = combine(outcome.map_err(|e| ConnectError::native("Issue reviewed checked installation access", e)),
            combine(notice, combine(retained, shown)))?;
        if stopped_result(stopped)? { return Ok(None); }
        // Both pre-stage commands are now fully settled. No ordinary runtime or
        // administrative bundle is retained when acquiring stage custody.
        let mut startup = SessionStartup::open(storage.clone()).await?;
        let (stage, stopped, notice) = join_entered(stage_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider, &storage.locator.state_root,
            storage.locator.realm.as_str(), None, Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout.clone()), DEFAULT_STOCK_AGENT,
        ), output, interrupt.as_mut()).await;
        let Some(stage) = stage else { stopped_result(stopped)?; return Ok(None); };
        let stage = match stage {
            Ok(stage) => stage,
            Err(error) => return combine(Err(ConnectError::native("Prepare governed setup ownership", error)), notice),
        };
        startup.standalone = Some(StagedSetup { stage, session, server,
            artifacts: prepared.artifact_directory, reviewed_mcp, reviewed_checked_mcp,
            #[cfg(test)]
            materialization_observer: None,
        });
        match combine(stopped_result(stopped), notice) {
            Ok(false) => {},
            stopped => {
                let primary = stopped.map(|_| ());
                let cleanup = startup.abort_standalone(output).await;
                combine(primary, cleanup)?;
                return Ok(None);
            }
        }
        Ok(Some(startup))
    }

    /// Close an unfinished stage after its entered presenters have settled.
    /// Other native handles may retain custody after abort; this does not
    /// identify or join work held by those handles.
    pub async fn abort_standalone<W: Write>(&mut self, output: &mut W) -> Result<(), ConnectError> {
        let Some(staged) = self.standalone.as_mut() else { return Ok(()); };
        // Release this startup's idle facade. Escaped handles and entered auth
        // work keep their own native lifetime; abort reports their custody.
        self.connector_auth = None;
        let outcome = staged.stage.abort().await;
        let retained = record(&staged.artifacts, "stage-abort", &format!("{outcome:#?}"));
        // Terminal errors still expose every settled, previously unobserved
        // command from the same stage. No command is repeated here.
        let unobserved = staged.stage.take_unobserved_grants();
        let remainder = record(&staged.artifacts, "stage-unobserved", &format!("{unobserved:#?}"));
        let shown = match &outcome {
            Ok(aborted) => {
                let effects = report_stage(output, aborted.issuance);
                let custody = report(output, match aborted.custody {
                    meerkat_runtime::StagedCustodyRelease::Released => "Setup stopped and this realm owner was released. Saved configuration and granted access remain as reported.",
                    meerkat_runtime::StagedCustodyRelease::Retained => "Setup stopped, but a native handle still retains this realm owner. It cannot be reopened until that handle releases it. This does not confirm that credential work has finished; saved configuration and granted access remain as reported.",
                });
                combine(effects, custody)
            }
            Err(_) => report(output, "Setup ownership cleanup did not confirm release. Saved changes and the exact cleanup outcome were retained; no replacement owner was opened."),
        };
        combine(combine(outcome.map(|_| ()).map_err(|e| ConnectError::native("Abort governed setup stage", e)), retained), combine(remainder, shown))
    }

    /// Prepare, review and grant the actual controller route, then consume its
    /// native ticket in this exact session's one construction. The terminal
    /// interrupt is retained through all entered work and is never replayed.
    pub async fn open_native_setup<I: Future<Output = io::Result<()>> + Send, W: Write>(
        mut self, options: SessionOptions, setup: Arc<NativeSetup>,
        approvals: &LocalApprovals, presenter: &mut ApprovalPresenter,
        input: &mut TerminalInput, output: &mut W, mut interrupt: Pin<&mut I>,
    ) -> Result<Option<ConnectSession>, ConnectError> {
        let result = self.open_native_setup_inner(options, setup.clone(), approvals,
            presenter, input, output, interrupt.as_mut()).await;
        if matches!(&result, Ok(Some(_))) { return result; }
        setup.retire();
        let cleanup = self.abort_standalone(output).await;
        match (result, cleanup) {
            (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup {
                primary: Box::new(primary), cleanup: Box::new(cleanup),
            }),
            (Err(error), _) | (_, Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        }
    }

    async fn open_native_setup_inner<I: Future<Output = io::Result<()>> + Send, W: Write>(
        &mut self, mut options: SessionOptions, setup: Arc<NativeSetup>,
        approvals: &LocalApprovals, presenter: &mut ApprovalPresenter,
        input: &mut TerminalInput, output: &mut W, mut interrupt: Pin<&mut I>,
    ) -> Result<Option<ConnectSession>, ConnectError> {
        let staged = self.standalone.as_ref().ok_or_else(|| invalid("The exact governed setup stage is required"))?;
        if staged.session.id() != setup.session_id() || self.persistence.is_some() {
            return Err(invalid("The setup stage and composition name different owners"));
        }
        approvals.check_session(staged.session.id())?;
        options = setup.session_options(options, &staged.server)?;
        if options.build.realm_id.as_ref().is_some_and(|realm| realm != &self.storage.locator.realm)
            || options.model.provider.zip(options.build.provider).is_some_and(|(a, b)| a != b)
            || options.model.auth_binding.as_ref().zip(options.build.auth_binding.as_ref())
                .is_some_and(|(a, b)| a != b)
        { return Err(invalid("The setup model and build must select this same realm and authentication")); }
        options.model.provider = options.model.provider.or(options.build.provider);
        options.model.auth_binding = options.model.auth_binding.or(options.build.auth_binding.clone());
        let model = meerkat::resolve_create_session_model(&self.config, options.model)
            .map_err(|e| ConnectError::native("Resolve governed setup model", e))?;
        options.build.realm_id = Some(self.storage.locator.realm.clone());
        options.build.provider = Some(model.provider);
        options.build.auth_binding = model.auth_binding;
        let research = setup.research().map(|binding|
            binding.bind(&self.config, model.provider, &model.model, &mut options.build)
        ).transpose()?;
        let mut factory = setup.configure_factory(self.factory.clone());
        if let Some(auth) = &self.connector_auth {
            factory = factory.mcp_call_context_provider(auth.context_provider.clone());
        }
        let builder = FactoryAgentBuilder::new(factory, self.config.clone());
        let mut request = CreateSessionRequest {
            model: model.model, prompt: String::new().into(), injected_context: Vec::new(),
            system_prompt: meerkat::SystemPromptOverride::Set(format!(
                "{}\nThe selected native realm is {}.\n", super::SETUP_INSTRUCTIONS,
                self.storage.locator.realm.as_str(),
            )),
            max_tokens: None, event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(options.build), labels: None,
        };
        let staged = self.standalone.as_mut().ok_or_else(|| invalid("The setup stage is unavailable"))?;
        let (offer, stopped, notice) = join_entered(staged.stage.prepare_controller(
            &builder, &request, staged.session.id().clone(),
        ), output, interrupt.as_mut()).await;
        let Some(offer) = offer else { stopped_result(stopped)?; return Ok(None); };
        let offer = combine(offer.map_err(|e| ConnectError::native("Prepare actual setup controller", e)), notice)?;
        if stopped_result(stopped)? { return Ok(None); }
        let requests = controller_requests(&offer.report, setup.bundle_operations());
        let body = controller_review_body(self.storage.locator.realm.as_str(), &offer.report, &requests);
        let summary = controller_setup_summary(&offer.report, &requests, &body);
        if !review(approvals, presenter, staged.session.id(),
            "Allow this setup assistant to use the selected model and tools?", body,
            SetupReviewPresentation::Controller(summary),
            input, output, interrupt.as_mut()).await? { return Ok(None); }
        let total = requests.len();
        let mut display = StagedGrantDisplay::default();
        // Report completed grants even when a later command, retention write or
        // cancellation stops this loop. Native failures retain their exact message.
        let settled = 'issue: {
            for (index, request) in requests.into_iter().enumerate() {
                if let Err(error) = record(&staged.artifacts, &format!("stage-grant-{index}-request"), &format!("{request:#?}")) {
                    break 'issue Err(error);
                }
                let (outcome, stopped, notice) = join_entered(staged.stage.grant_stock_agent(request), output, interrupt.as_mut()).await;
                let Some(outcome) = outcome else { break 'issue stopped_result(stopped).map(|_| false); };
                let retained = record(&staged.artifacts, &format!("stage-grant-{index}-outcome"), &format!("{outcome:#?}"));
                let shown = display.observe(output, &outcome);
                if let Err(error) = combine(outcome.map_err(|e| ConnectError::native("Issue reviewed setup grant", e)),
                    combine(notice, combine(retained, shown))) { break 'issue Err(error); }
                match stopped_result(stopped) {
                    Ok(false) => {},
                    Ok(true) => break 'issue Ok(false),
                    Err(error) => break 'issue Err(error),
                }
            }
            Ok(true)
        };
        let shown = display.finish(output, total);
        if !combine(settled, shown)? { return Ok(None); }
        offer.ticket.seat_build(request.build.as_mut().ok_or_else(|| invalid("The exact setup build is required"))?);
        let (opened, stopped, notice) = join_entered(staged.stage.finish(), output, interrupt.as_mut()).await;
        let Some(opened) = opened else { stopped_result(stopped)?; return Ok(None); };
        let retained = record(&staged.artifacts, "stage-finish", &format!("{opened:#?}"));
        let opened = match opened {
            Ok(opened) => opened,
            Err(error) => return combine(Err(ConnectError::native("Finish governed setup stage", error)), combine(notice, retained)),
        };
        let staged = self.standalone.take().ok_or_else(|| invalid("The finished setup stage is unavailable"))?;
        combine(notice, combine(retained, report_stage(output, opened.issuance)))?;
        let stock = staged.stage.stock_governance().clone();
        #[cfg(test)]
        if let Some(observe) = staged.materialization_observer { observe(&opened.bundle); }
        // Finished bundles retain the same auth owner. Even cancellation after
        // finish closes this exact bundle/session instead of reopening a realm.
        let interrupted = stopped_result(stopped)?;
        if interrupted { drop(opened); return Ok(None); }
        let session = ConnectSession::materialize_native_setup(
            &self.storage, builder, opened.bundle, staged.session, request, stock,
            research, self.connector_auth.clone(), setup.clone(),
        );
        let (session, stopped, notice) = join_entered(session, output, interrupt.as_mut()).await;
        let Some(session) = session else { stopped_result(stopped)?; return Ok(None); };
        let session = match session {
            Ok(session) => session,
            Err(error) => return combine(Err(error), notice),
        };
        match combine(stopped_result(stopped), notice) {
            Ok(false) => {},
            stopped => {
                let primary = stopped.map(|_| ());
                let cleanup = session.close().await;
                combine(primary, cleanup)?;
                return Ok(None);
            }
        }
        Ok(Some(session))
    }
}

fn controller_requests(report: &ControllerRouteReport, operations: &std::collections::BTreeSet<String>) -> Vec<StockGrantRequest> {
    std::iter::once(StockGrantTarget::Controller { routes: [report.key.clone()].into_iter().collect() })
        .chain(std::iter::once(StockGrantTarget::Catalog))
        .chain(operations.iter().map(|operation| StockGrantTarget::ToolBundleOperation {
            bundle: crate::service_preparation::native::STANDALONE_BUNDLE.to_owned(), operation: operation.clone(),
        }))
        .map(|target| StockGrantRequest { agent: DEFAULT_STOCK_AGENT.to_owned(), target, expires_in_ms: None })
        .collect()
}

fn controller_review_body(realm: &str, report: &ControllerRouteReport,
    requests: &[StockGrantRequest],
) -> serde_json::Value {
    serde_json::json!({
        "realm": realm,
        "agent": DEFAULT_STOCK_AGENT,
        "controller": {"key": report.key.to_string(), "provider": report.provider,
            "model": report.model, "wire_model": report.wire_model,
            "endpoint": report.endpoint, "credential_owner": report.credential_owner},
        "grants": requests.iter().map(|request| format!("{request:?}")).collect::<Vec<_>>(),
        "lifetime": "Until revoked. Each named tool operation has an independent native grant.",
        "prior_effect": "The separately reviewed setup MCP grant is already settled. Refusal here does not roll it back.",
    })
}

/// A display of this exact stock request family, not a second grant model.
/// Different targets, lifetimes or review shapes retain the full technical view.
fn controller_setup_summary(report: &ControllerRouteReport, requests: &[StockGrantRequest],
    body: &serde_json::Value,
) -> Option<String> {
    let (controller, rest) = requests.split_first()?;
    let (catalog, tools) = rest.split_first()?;
    let agent = body.get("agent")?.as_str()?;
    if !matches!(&controller.target, StockGrantTarget::Controller { routes }
        if routes.len() == 1 && routes.contains(&report.key))
        || !matches!(&catalog.target, StockGrantTarget::Catalog)
        || requests.iter().any(|request| request.agent != agent || request.expires_in_ms.is_some())
    { return None; }
    let mut operations = std::collections::BTreeSet::new();
    for request in tools {
        match &request.target {
            StockGrantTarget::ToolBundleOperation { bundle, operation }
                if bundle == crate::service_preparation::native::STANDALONE_BUNDLE
                    && !operation.is_empty() && operations.insert(operation) => {},
            _ => return None,
        }
    }
    let fields = ["realm", "agent", "controller", "grants", "lifetime", "prior_effect"];
    let selected = body.get("controller")?;
    let route_fields = ["key", "provider", "model", "wire_model", "endpoint", "credential_owner"];
    if body.as_object()?.len() != fields.len() || fields.iter().any(|key| body.get(key).is_none())
        || selected.as_object()?.len() != route_fields.len() || route_fields.iter().any(|key| selected.get(key).is_none())
        || selected.get("key")?.as_str()? != report.key.to_string()
        || body.get("grants")? != &serde_json::json!(requests.iter().map(|request| format!("{request:?}")).collect::<Vec<_>>())
    { return None; }
    let field = |value: &serde_json::Value, name: &str| value.get(name)?.as_str()
        .filter(|text| !text.is_empty()).map(review_text);
    Some(format!(
        "Model: {} ({})\nProvider model: {}\nEndpoint: {}\nCredential owner: {}\nScope: realm {}, agent {}\nTools: tool catalog and {} named setup operations (see details).\nLifetime: {}\nEarlier review: {}",
        field(selected, "model")?, field(selected, "provider")?, field(selected, "wire_model")?,
        field(selected, "endpoint")?, field(selected, "credential_owner")?, field(body, "realm")?, review_text(agent),
        operations.len(), field(body, "lifetime")?, field(body, "prior_effect")?,
    ))
}

impl ConnectSession {
    async fn materialize_native_setup(
        storage: &meerkat_core::ResolvedStorage, builder: FactoryAgentBuilder,
        persistence: meerkat::PersistenceBundle, session: Session, request: CreateSessionRequest,
        stock: Arc<meerkat::governed_realm::stock::StockGovernance>,
        research: Option<crate::research::ResearchScope>,
        connector_auth: Option<Arc<crate::connector_auth::ConnectorAuth>>, setup: Arc<NativeSetup>,
    ) -> Result<Self, ConnectError> {
        let config_path = crate::realm::config_source(storage)?.config_doc_path(&storage.locator.realm);
        // This builder holds a snapshot. Resolve declared service secrets from
        // the selected realm's own documents, including after continuation.
        let builder = meerkat::surface::install_default_mcp_auth_resolver_for_realm(
            builder, &persistence.runtime_adapter(), storage,
        );
        let (service, machine) = meerkat::surface::build_runtime_backed_service_with_default_reconfigure_host(
            builder, 1, persistence, config_path.with_file_name("config_state.json"),
        ).map_err(|error| ConnectError::native("Compose governed setup runtime", error))?;
        let id = session.id().clone();
        let mut registration = None;
        let result = async {
            let admission = service.reserve_create_session_admission().await
                .map_err(|e| ConnectError::native("Reserve governed setup admission", e))?;
            let mut prepared = machine.prepare_session_materialization(id.clone()).await
                .map_err(|e| ConnectError::native("Prepare exact setup registration", e))?;
            registration = machine.current_session_registration_witness(&id).await;
            let materialized = async {
                if registration.is_none() { return Err(invalid("The prepared setup registration is unavailable")); }
                // Native configured-MCP attachment requires this exact current
                // session mandate before its actor is constructed.
                stock.bind_session_agent(&machine, &id, [crate::setup::SERVER_NAME]).await
                    .map_err(|e| ConnectError::native("Bind governed setup tool mandate", e))?;
                let (_, actor_slot) = Box::pin(meerkat::surface::materialize_prepared_session_actor_unattached_with_actor_slot(
                    &service, &machine, &mut prepared, session, request, admission,
                )).await.map_err(|e| ConnectError::native("Build governed setup actor", e))?;
                machine.ensure_session_with_executor(id.clone(), Box::new(
                    meerkat::surface::PersistentRuntimeExecutor::new(service.clone(), machine.clone(), id.clone())
                        .with_publication_actor_slot(actor_slot),
                )).await.map_err(|e| ConnectError::native("Attach exact setup actor", e))?;
                machine.current_executor_attachment_witness(&id).await
                    .ok_or_else(|| invalid("The setup actor has no current executor attachment"))
            }.await;
            if materialized.is_err() {
                let rollback = prepared.rollback_now().await
                    .map(|_| ()).map_err(|e| ConnectError::native("Roll back failed setup registration", e));
                return match (materialized, rollback) {
                    (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup { primary: Box::new(primary), cleanup: Box::new(cleanup) }),
                    (Err(error), _) | (_, Err(error)) => Err(error),
                    (Ok(value), Ok(())) => Ok(value),
                };
            }
            materialized
        }.await;
        let attachment = match result {
            Ok(value) => value,
            Err(primary) => {
                // Preserve every actual construction owner and the original
                // failure. A missing or no-longer-current exact registration
                // cannot be replaced or treated as a successful terminal join.
                return Err(super::close_failed_construction(primary, service, machine,
                    registration, research, connector_auth, Some(stock), Some(setup)).await);
            }
        };
        let registration = registration.ok_or_else(|| invalid("The setup registration was not retained"))?;
        // The exact actor and attachment already exist. Retain their complete
        // owner before checking whether the tool composer published its part.
        let mut opened = Self { service, machine, session_id: id, realm: storage.locator.clone(), registration, attachment,
            _research: research, service_preparation: None, _connector_auth: connector_auth,
            stock: Some(stock), stock_submission: Arc::new(super::StockSubmission::default()) };
        match setup.owner() {
            Ok(owner) => opened.service_preparation = Some(owner),
            Err(primary) => return combine(Err(primary), opened.close().await).map(|()| unreachable!()),
        }
        if opened._research.as_ref().is_some_and(|scope| !scope.captured()) {
            let primary = invalid("Native research did not retain the one prepared controller");
            return combine(Err(primary), opened.close().await).map(|()| unreachable!());
        }
        Ok(opened)
    }
}

enum SetupReviewPresentation { SuppliedHelper, Controller(Option<String>), Full }

async fn review<I: Future<Output = io::Result<()>>, W: Write>(
    approvals: &LocalApprovals, presenter: &mut ApprovalPresenter, session: &SessionId,
    summary: &str, body: serde_json::Value, presentation: SetupReviewPresentation,
    input: &mut TerminalInput, output: &mut W,
    mut interrupt: Pin<&mut I>,
) -> Result<bool, ConnectError> {
    let expected_body = body.clone();
    let mut request = crate::terminal_connector::review_request(session, summary, body)?;
    request.requester = meerkat_core::ApprovalPrincipalId::new("connect-local-setup")
        .map_err(|e| ConnectError::native("Name local setup review requester", e))?;
    request.resource.id = meerkat_core::ApprovalResourceId::new("selected-realm-setup");
    let decision = approvals.review_access(request);
    tokio::pin!(decision);
    loop {
        tokio::select! { biased;
            signal = interrupt.as_mut() => { signal.map_err(|e| ConnectError::native("Listen for setup cancellation", e))?; return Ok(false); },
            result = decision.as_mut() => return result.map(|record| record.status == ApprovalStatus::Approved),
            prompt = presenter.next() => {
                let prompt = prompt.ok_or_else(|| invalid("The protected setup presenter ended"))?;
                let continued = match &presentation {
                    SetupReviewPresentation::SuppliedHelper =>
                        present_supplied_setup_approval(prompt, input, output, interrupt.as_mut()).await?,
                    SetupReviewPresentation::Controller(summary) =>
                        present_controller_setup_approval(prompt, &expected_body, summary.as_deref(),
                            input, output, interrupt.as_mut()).await?,
                    SetupReviewPresentation::Full =>
                        present_approval(prompt, input, output, interrupt.as_mut()).await?,
                };
                if !continued { return Ok(false); }
            }
        }
    }
}

/// Poll an entered operation to settlement even when its observer stops. A
/// signal already ready at entry leaves the operation unpolled. Every caller
/// retains the returned native result before handling cancellation. A notice
/// failure is returned separately only after the same entered work settles.
async fn join_entered<F: Future, I: Future<Output = io::Result<()>>, W: Write>(
    operation: F, output: &mut W, mut interrupt: Pin<&mut I>,
) -> (Option<F::Output>, Option<io::Result<()>>, Result<(), ConnectError>) {
    if let Some(signal) = interrupt.as_mut().now_or_never() { return (None, Some(signal), Ok(())); }
    tokio::pin!(operation);
    let (outcome, signal, notice) = if let Some(outcome) = operation.as_mut().now_or_never() {
        (outcome, None, Ok(()))
    } else {
        tokio::select! { biased;
            signal = interrupt.as_mut() => {
                let notice = report(output, "Waiting for native setup startup to settle");
                (operation.await, Some(signal), notice)
            },
            outcome = operation.as_mut() => (outcome, None, Ok(())),
        }
    };
    let signal = signal.or_else(|| interrupt.as_mut().now_or_never());
    (Some(outcome), signal, notice)
}

fn stopped_result(signal: Option<io::Result<()>>) -> Result<bool, ConnectError> {
    signal.map(|result| result.map(|()| true).map_err(|e| ConnectError::native("Listen for setup cancellation", e)))
        .unwrap_or(Ok(false))
}

fn report<W: Write>(output: &mut W, message: &str) -> Result<(), ConnectError> {
    writeln!(output, "{message}").and_then(|()| output.flush())
        .map_err(|e| ConnectError::native("Show setup outcome", e))
}

/// Render only the native outcome classification. Exact grant identities and
/// cleanup attempts stay in the private outcome record, not in ordinary prompts.
fn report_grant<W: Write>(output: &mut W, capability: &str,
    outcome: Result<&StockGrantOutcome, &meerkat::governed_realm::stock::issuance::StockGrantError>,
) -> Result<(), ConnectError> {
    use meerkat::governed_realm::stock::issuance::{StockGrantError, RevocationOutcome};
    let result = match outcome {
        Ok(grant) => {
            if grant.cleanup.iter().any(|attempt| attempt.outcome != RevocationOutcome::Revoked) {
                "was granted and selected. Some prior grant cleanup is deferred or unconfirmed; the exact outcomes were retained."
            } else {
                "was granted and selected. This access remains until revoked, even if setup stops."
            }
        }
        Err(StockGrantError::Refused(_)) => "was refused before grant effects. No access was issued, selected or revoked by this command.",
        Err(StockGrantError::IssuanceUncertain { .. }) => "has an uncertain issuance outcome. Its selection was unchanged; no retry was started.",
        Err(StockGrantError::NotSelected { .. }) => "was issued but was not selected. The previous selection remains; no retry was started.",
        Err(StockGrantError::SelectionUncertain { .. }) => "was issued and selected, but that selection may not survive a crash. No cleanup or retry was started.",
        Err(StockGrantError::CommandUncertain) => "has an uncertain command outcome and may have changed grants or their selection. No retry was started.",
    };
    report(output, &format!("{capability} {result}"))
}

/// Invocation-local display only. The native result and each private record
/// remain with the caller; a count is not a stage or persistence receipt.
#[derive(Default)]
struct StagedGrantDisplay { selected: usize, deferred_cleanup: usize }

impl StagedGrantDisplay {
    fn observe<W: Write>(&mut self, output: &mut W,
        outcome: &Result<StockGrantOutcome, meerkat::governed_realm::staged::StagedRealmError>,
    ) -> Result<(), ConnectError> {
        match outcome {
            Ok(grant) => {
                self.selected += 1;
                if grant.cleanup.iter().any(|attempt| attempt.outcome != meerkat::governed_realm::stock::issuance::RevocationOutcome::Revoked) {
                    self.deferred_cleanup += 1;
                }
                Ok(())
            }
            Err(_) => report_staged_grant(output, outcome),
        }
    }

    fn finish<W: Write>(&self, output: &mut W, total: usize) -> Result<(), ConnectError> {
        let cleanup = if self.deferred_cleanup == 0 { String::new() } else {
            format!(" Requests with deferred or unconfirmed prior grant cleanup: {}.", self.deferred_cleanup)
        };
        report(output, &format!(
            "Setup permissions: {} of {total} confirmed granted and selected.{cleanup}", self.selected,
        ))
    }
}

fn report_staged_grant<W: Write>(output: &mut W,
    outcome: &Result<StockGrantOutcome, meerkat::governed_realm::staged::StagedRealmError>,
) -> Result<(), ConnectError> {
    use meerkat::governed_realm::staged::StagedRealmError;
    match outcome {
        Ok(grant) => report_grant(output, "Reviewed setup capability", Ok(grant)),
        Err(StagedRealmError::Grant(error)) => report_grant(output, "Reviewed setup capability", Err(error)),
        Err(_) => report(output, "The setup step was refused before this grant command entered. Earlier grant and configuration outcomes remain as reported."),
    }
}

fn report_stage<W: Write>(output: &mut W,
    effect: meerkat::governed_realm::staged::StagedIssuanceEffect,
) -> Result<(), ConnectError> {
    use meerkat::governed_realm::staged::StagedIssuanceEffect;
    report(output, match effect {
        StagedIssuanceEffect::NotEntered => "No controller or additional setup grant changed the realm in this setup stage. The earlier helper access outcome is separate.",
        StagedIssuanceEffect::Committed => "Controller and additional setup grant effects settled with known outcomes. They remain saved independently of whether this conversation opens.",
        StagedIssuanceEffect::Uncertain => "At least one controller or additional setup grant has an uncertain outcome. Its exact result is retained and no replay was started.",
    })
}

fn record(directory: &std::path::Path, name: &str, value: &str) -> Result<(), ConnectError> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = directory.join(format!("standalone-{name}.txt"));
    let retained = (|| -> io::Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        file.write_all(value.as_bytes())?;
        file.sync_all()
    })();
    retained.map_err(|error| invalid(&format!(
        "The setup outcome could not be fully retained at {}: {error}. Exact observed outcome: {value}. No automatic retry was started.", path.display(),
    )))
}

pub(super) fn combine<T>(primary: Result<T, ConnectError>, cleanup: Result<(), ConnectError>) -> Result<T, ConnectError> {
    match (primary, cleanup) {
        (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup { primary: Box::new(primary), cleanup: Box::new(cleanup) }),
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}

fn invalid(message: &str) -> ConnectError { ConnectError::InvalidInput(message.to_owned()) }

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod config_tests;

#[cfg(test)]
mod stock_submission_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod presentation_tests {
    use super::*;
    use meerkat::governed_realm::{staged::StagedRealmError, stock::issuance::{
        CleanupAttempt, RevocationOutcome, StockGrantError, stock_agent_principal,
    }};

    #[test]
    fn actual_controller_request_shape_has_compact_review_with_full_fallback() {
        // Actual public report type and production request/body construction.
        // These are presentation inputs, not a pinned client or issued grants.
        let operations = [crate::forge::FORGE_PREPARE.to_owned(),
            crate::service_preparation::TOOLKIT_PREPARE_SERVICES.to_owned()].into_iter().collect();
        for credential_owner in ["binding fixture-realm/model", "account fixture-realm/operator"] {
            let report = ControllerRouteReport {
                key: meerkat_core::ControllerRouteKey::parse(&format!("model-route/v1/{}", "a".repeat(64))).unwrap(),
                provider: "self_hosted".into(), model: "fixture-model".into(),
                wire_model: "fixture-wire-model".into(), endpoint: "https://models.invalid/v1".into(),
                credential_owner: credential_owner.into(),
            };
            let requests = controller_requests(&report, &operations);
            assert_eq!(requests.len(), operations.len() + 2);
            let body = controller_review_body("fixture-realm", &report, &requests);
            let summary = controller_setup_summary(&report, &requests, &body).expect("actual report shape is compact");
            for value in [&report.provider, &report.model, &report.wire_model, &report.endpoint, &report.credential_owner] {
                assert!(summary.contains(value.as_str()));
            }
            for value in ["realm fixture-realm", DEFAULT_STOCK_AGENT, "2 named setup operations", "Until revoked", "does not roll it back"] {
                assert!(summary.contains(value));
            }
            assert!(!summary.contains("StockGrantRequest"));
            let exact = serde_json::to_string(&body).unwrap();
            assert_eq!(exact.matches("StockGrantRequest").count(), requests.len());
            for operation in &operations { assert!(exact.contains(operation.as_str())); }
            for field in ["endpoint", "credential_owner", "wire_model"] {
                let mut changed = body.clone(); changed["controller"][field] = serde_json::Value::Null;
                assert!(controller_setup_summary(&report, &requests, &changed).is_none());
            }
            let mut changed = body.clone(); changed["future_scope"] = serde_json::json!("requires full review");
            assert!(controller_setup_summary(&report, &requests, &changed).is_none());
            let mut changed = body.clone(); changed["grants"] = serde_json::json!([]);
            assert!(controller_setup_summary(&report, &requests, &changed).is_none());
            let mut timed = requests.clone(); timed[2].expires_in_ms = Some(60_000);
            let timed_body = controller_review_body("fixture-realm", &report, &timed);
            assert!(controller_setup_summary(&report, &timed, &timed_body).is_none(), "never claim unlimited lifetime for a timed request");
            let mut unsupported = requests.clone(); unsupported[2].target = StockGrantTarget::CheckedMcpRoute {
                route: "fixture-route".into(), server: "fixture-server".into(), operations: ["read".into()].into(),
            };
            let unsupported_body = controller_review_body("fixture-realm", &report, &unsupported);
            assert!(controller_setup_summary(&report, &unsupported, &unsupported_body).is_none());
        }
    }

    #[test]
    fn staged_grant_summary_preserves_partial_native_outcomes() {
        let realm = meerkat_core::connection::RealmId::parse("fixture-realm").unwrap();
        let principal = stock_agent_principal(&realm, "fixture").unwrap();
        // Typed native references are formatter fixtures only, never submitted.
        let reference = || serde_json::from_value(serde_json::json!({
            "root_authority": &principal, "authority_namespace": "fixture-authority",
            "authority_generation": 1, "authority_incarnation": "6f75f457-4232-4e12-8426-508d49ece197",
            "grant_id": "fixture-grant", "issued_revision": 1,
        })).unwrap();
        for (error, message) in [
            (StockGrantError::NotSelected { issued: reference(), reason: "PRIVATE_REASON".into() }, "was issued but was not selected"),
            (StockGrantError::SelectionUncertain { issued: reference(), previous: None, reason: "PRIVATE_REASON".into() }, "may not survive a crash"),
            (StockGrantError::IssuanceUncertain { id: serde_json::from_value(serde_json::json!("fixture-grant")).unwrap() }, "uncertain issuance outcome"),
            (StockGrantError::CommandUncertain, "uncertain command outcome"),
        ] {
            let mut display = StagedGrantDisplay::default();
            let mut output = Vec::new();
            for deferred in [false, true] {
                let cleanup = if deferred { vec![CleanupAttempt { agent: "fixture".into(), grant: reference(), outcome: RevocationOutcome::Uncertain }] } else { vec![] };
                display.observe(&mut output, &Ok(StockGrantOutcome { issued: reference(), superseded: None, cleanup })).unwrap();
            }
            assert!(output.is_empty(), "successful grants accumulate without repeated lines");
            let outcome = Err(StagedRealmError::Grant(error));
            let shown = display.observe(&mut output, &outcome);
            let primary = combine(outcome.map_err(|error| ConnectError::native("Issue reviewed setup grant", error)), shown);
            let result = combine(primary, display.finish(&mut output, 4));
            assert!(matches!(result, Err(ConnectError::Native { operation: "Issue reviewed setup grant", .. })));
            let text = String::from_utf8(output).unwrap();
            assert!(text.contains(message));
            let lower = text.to_ascii_lowercase();
            assert!(lower.contains("no retry was started") || lower.contains("no cleanup or retry was started"));
            assert!(text.contains("2 of 4 confirmed granted and selected"));
            assert!(text.contains("deferred or unconfirmed prior grant cleanup: 1"));
            assert!(text.find(message).unwrap() < text.find("2 of 4").unwrap());
            assert!(!text.contains("PRIVATE_REASON") && !text.contains("4 of 4"));
        }
        struct RefusedOutput;
        impl Write for RefusedOutput {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> { Err(io::Error::other("display refused")) }
            fn flush(&mut self) -> io::Result<()> { Err(io::Error::other("display refused")) }
        }
        let display = StagedGrantDisplay::default();
        let primary: Result<(), ConnectError> = Err(ConnectError::native("Issue reviewed setup grant",
            StagedRealmError::Grant(StockGrantError::CommandUncertain)));
        let result = combine(primary, display.finish(&mut RefusedOutput, 4));
        let ConnectError::Cleanup { primary, cleanup } = result.unwrap_err() else { panic!("both failures must survive"); };
        let ConnectError::Native { operation: "Issue reviewed setup grant", source } = *primary else { panic!("native error remains primary"); };
        assert!(matches!(source.downcast_ref::<StagedRealmError>(), Some(StagedRealmError::Grant(StockGrantError::CommandUncertain))));
        assert!(matches!(*cleanup, ConnectError::Native { operation: "Show setup outcome", .. }));
    }
}
