use std::sync::{Arc, OnceLock};

use meerkat::{
    AgentFactory, CreateSessionModelResolutionRequest, FactoryAgentBuilder,
    PersistentSessionService, Session, SystemPromptOverride,
    build_persistent_service_with_runtime_adapter, resolve_create_session_model,
    surface::{
        PersistentRuntimeExecutor, materialize_session_with_reserved_admission_and_actor_slot,
    },
};
use meerkat_core::{
    Config, CreateSessionRequest, ResolvedStorage, SessionBuildOptions, SessionId,
    comms::EventStream,
    service::{DeferredPromptPolicy, InitialTurnPolicy},
};
use meerkat_runtime::{
    AcceptOutcome, Input, MeerkatMachine, PromptInput, RuntimeExecutorAttachmentWitness,
    RuntimeSessionRegistrationWitness,
    completion::CompletionHandle,
    input_admission_custody::{
        NativeInputAdmissionCompletion, NativeInputAdmissionCustody, NativeInputAdmissionSettlement,
    },
};

use crate::{ConnectError, realm::read_config};

const SETUP_INSTRUCTIONS: &str = include_str!("setup-instructions.md");

mod standalone;
pub use standalone::{ResumedStandalone, StandaloneContinuation};

#[derive(Default)]
pub struct SessionOptions {
    pub model: CreateSessionModelResolutionRequest,
    /// Host-supplied native tools, auth selection and policies. This is not a grant.
    pub build: SessionBuildOptions,
}

/// Native startup resources retained while a host presents provider sign-in.
/// Opening these resources creates no session or model client. Ordinary setup
/// consumes the original persistence bundle in `open_lazy_setup`; standalone
/// setup retains a native stage and consumes its ticket and finished bundle in
/// `open_native_setup`. Authentication must use that same native owner's runtime
/// authority. There is no replacement token store.
pub struct SessionStartup {
    pub(crate) storage: ResolvedStorage,
    pub(crate) config: Config,
    pub(crate) factory: AgentFactory,
    pub(crate) persistence: Option<meerkat::PersistenceBundle>,
    pub(crate) connector_auth: Option<Arc<crate::connector_auth::ConnectorAuth>>,
    pub(crate) standalone: Option<standalone::StagedSetup>,
}

impl SessionStartup {
    pub async fn open(storage: ResolvedStorage) -> Result<Self, ConnectError> {
        let config = read_config(&storage).await?;
        Self::with_config(storage, config).await
    }

    async fn with_config(storage: ResolvedStorage, config: Config) -> Result<Self, ConnectError> {
        let factory = Self::realm_factory(&storage);
        Ok(Self {
            storage,
            config,
            factory,
            persistence: None,
            connector_auth: None,
            standalone: None,
        })
    }

    /// Select the existing native credential backend without composing a model,
    /// opening a runtime session or installing the supplied setup helper.
    pub(crate) fn realm_factory(storage: &ResolvedStorage) -> AgentFactory {
        let realm_paths = meerkat_store::realm_paths_in(
            &storage.locator.state_root,
            storage.locator.realm.as_str(),
        );
        let factory = AgentFactory::new(realm_paths.root.clone())
            .context_root(storage.layout.invocation_context())
            .runtime_root(realm_paths.root)
            .builtins(false)
            .shell(false);

        if let Some(root) = storage.layout.user_home_root() {
            factory.user_config_root(root)
        } else {
            factory
        }
    }

    pub(crate) async fn persistence(
        &mut self,
    ) -> Result<&meerkat::PersistenceBundle, ConnectError> {
        if self.standalone.is_some() {
            return Err(ConnectError::InvalidInput(
                "The governed setup stage must supply its own native authority".into(),
            ));
        }
        if self.persistence.is_none() {
            let (_, persistence) = meerkat::storage_provider::open_realm_persistence_with_layout(
                self.storage.layout.clone(),
                self.storage.locator.realm.as_str(),
                None,
                Some(meerkat_store::RealmOrigin::Explicit),
            )
            .await
            .map_err(|e| ConnectError::native("open selected realm persistence", e))?;
            self.persistence = Some(persistence);
        }
        self.persistence.as_ref().ok_or_else(|| {
            ConnectError::InvalidInput(
                "Native startup did not retain its persistence bundle".into(),
            )
        })
    }

    /// Use the native owner already selected for this startup. A standalone
    /// stage forwards its own auth pair; it never opens a second realm owner.
    /// The clone shares custody without retaining the staged machine. Native
    /// Aborting/Ended refusals remain errors and cannot select a fallback.
    pub(crate) async fn provider_auth_runtime_authority(
        &mut self,
    ) -> Result<meerkat_runtime::ProviderAuthRuntimeAuthority, ConnectError> {
        if let Some(staged) = self.standalone.as_ref() {
            return staged
                .stage
                .provider_auth_runtime_authority()
                .map_err(|error| {
                    ConnectError::native("Read staged setup authentication authority", error)
                });
        }
        Ok(self
            .persistence()
            .await?
            .runtime_adapter()
            .provider_auth_runtime_authority())
    }

    pub(crate) async fn configure_connectors(
        &mut self,
        profiles: Vec<meerkat::ConnectorClientAuthProfile>,
    ) -> Result<Arc<crate::connector_auth::ConnectorAuth>, ConnectError> {
        if self.connector_auth.is_some() {
            return Err(ConnectError::InvalidInput(
                "Connector authority is already configured".into(),
            ));
        }
        let runtime = self.provider_auth_runtime_authority().await?;
        let facade = meerkat::HostAuthService::from_factory(&self.factory, runtime)
            .map_err(|error| ConnectError::provider_setup("Open connector authority", error))?
            .with_connector_client_auth_profiles(profiles);
        // Native construction validates every profile. Move this authority
        // once into the live provider; it reads each call's staged native
        // configuration without a Toolkit installation snapshot or registry.
        let auth = crate::connector_auth::ConnectorAuth::from_facade(
            facade,
            self.storage.locator.realm.clone(),
        )?;
        self.connector_auth = Some(auth.clone());
        Ok(auth)
    }

    pub(crate) async fn configure_realm_connectors(
        &mut self,
        profiles: Arc<meerkat::RealmConnectorClients>,
    ) -> Result<Arc<crate::connector_auth::ConnectorAuth>, ConnectError> {
        if self.connector_auth.is_some() {
            return Err(ConnectError::InvalidInput(
                "Connector authority is already configured".into(),
            ));
        }
        let runtime = self.provider_auth_runtime_authority().await?;
        let facade = meerkat::HostAuthService::from_factory(&self.factory, runtime)
            .map_err(|error| ConnectError::provider_setup("Open connector authority", error))?
            .with_connector_client_source(profiles);
        let auth = crate::connector_auth::ConnectorAuth::from_facade(
            facade,
            self.storage.locator.realm.clone(),
        )?;
        self.connector_auth = Some(auth.clone());
        Ok(auth)
    }

    /// Consume the startup resources into one exact, locally admitted setup
    /// composition. Provider setup must finish before this single-use claim.
    pub async fn open_lazy_setup(
        self,
        options: SessionOptions,
        lazy: &crate::service_preparation::LazySetup,
    ) -> Result<ConnectSession, ConnectError> {
        lazy.claim()?;
        let result = async {
            validate_lazy_options(&options, lazy)?;
            if let Some(installs) = lazy.installs() {
                installs
                    .bind_realm(&self.storage.locator)
                    .map_err(|e| ConnectError::native("bind selected installation realm", e))?;
            }
            let mut session = Box::pin(ConnectSession::open_composed(
                self.storage.clone(),
                self.config.clone(),
                options,
                lazy.paired_host().research(),
                Some(self),
            ))
            .await?;
            session.service_preparation = Some(lazy.owner());
            Ok(session)
        }
        .await;
        if result.is_err() {
            lazy.owner().shutdown().await;
        }
        result
    }
}

fn validate_lazy_options(
    options: &SessionOptions,
    lazy: &crate::service_preparation::LazySetup,
) -> Result<(), ConnectError> {
    if !crate::setup::is_direct_paired_host_composition(options)
        || !options
            .build
            .external_tools
            .as_ref()
            .is_some_and(|tools| lazy.matches(tools))
        || options.build.tool_dispatch_admission.is_none()
    {
        return Err(ConnectError::InvalidInput(
            "Lazy setup requires its exact closed composition and local dispatch admission".into(),
        ));
    }
    Ok(())
}

/// One native setup conversation. Dropping a waiter does not cancel a native turn.
/// Call `cancel` and await its completion, then `close`, for explicit teardown.
/// Ordinary library opening creates a fresh session. The dedicated CLI can
/// reopen its committed session through the explicit governed continuation path.
pub struct ConnectSession {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
    realm: meerkat_core::RealmLocator,
    registration: RuntimeSessionRegistrationWitness,
    attachment: RuntimeExecutorAttachmentWitness,
    // Clears the optional capture after native close or a dropped session.
    // Like the other fields, Drop alone does not claim native turn cancellation.
    _research: Option<crate::research::ResearchScope>,
    service_preparation: Option<Arc<crate::service_preparation::ServicePreparationOwner>>,
    _connector_auth: Option<Arc<crate::connector_auth::ConnectorAuth>>,
    stock: Option<Arc<meerkat::governed_realm::stock::StockGovernance>>,
    stock_submission: Arc<StockSubmission>,
}

impl ConnectSession {
    /// Open one exact lazy setup composition. No resource location, preparation
    /// or MCP subprocess is needed until the original agent requests it.
    pub async fn open_lazy_setup(
        storage: ResolvedStorage,
        options: SessionOptions,
        lazy: &crate::service_preparation::LazySetup,
    ) -> Result<Self, ConnectError> {
        lazy.claim()?;
        let result = match read_config(&storage).await {
            Ok(config) => Self::open_lazy_claimed(storage, config, options, lazy).await,
            Err(error) => Err(error),
        };
        if result.is_err() {
            lazy.owner().shutdown().await;
        }
        result
    }

    /// The config must be the native effective view of this exact realm. The
    /// supplied composition is single-use, including failed materialization.
    pub async fn open_lazy_setup_with_config(
        storage: ResolvedStorage,
        config: Config,
        options: SessionOptions,
        lazy: &crate::service_preparation::LazySetup,
    ) -> Result<Self, ConnectError> {
        lazy.claim()?;
        let result = Self::open_lazy_claimed(storage, config, options, lazy).await;
        if result.is_err() {
            lazy.owner().shutdown().await;
        }
        result
    }

    async fn open_lazy_claimed(
        storage: ResolvedStorage,
        config: Config,
        options: SessionOptions,
        lazy: &crate::service_preparation::LazySetup,
    ) -> Result<Self, ConnectError> {
        validate_lazy_options(&options, lazy)?;
        if let Some(installs) = lazy.installs() {
            installs
                .bind_realm(&storage.locator)
                .map_err(|e| ConnectError::native("bind selected installation realm", e))?;
        }
        let mut session =
            Self::open_inner(storage, config, options, lazy.paired_host().research()).await?;
        session.service_preparation = Some(lazy.owner());
        Ok(session)
    }

    pub async fn open(
        storage: ResolvedStorage,
        options: SessionOptions,
    ) -> Result<Self, ConnectError> {
        let config = read_config(&storage).await?;
        Self::open_with_config(storage, config, options).await
    }

    /// The supplied config must be the native effective view for this exact realm.
    /// Exposed for embedding hosts that already own config loading.
    pub async fn open_with_config(
        storage: ResolvedStorage,
        config: Config,
        options: SessionOptions,
    ) -> Result<Self, ConnectError> {
        Self::open_inner(storage, config, options, None).await
    }

    /// Open the exact direct frontend composition, optionally binding its
    /// research tool to this session's factory-built provider/auth client.
    /// Reusing a research binding or replacing its dispatcher is refused.
    pub async fn open_paired_host(
        storage: ResolvedStorage,
        options: SessionOptions,
        paired_host: &crate::paired_host_tool::PairedHostFrontend,
    ) -> Result<Self, ConnectError> {
        let config = read_config(&storage).await?;
        Self::open_paired_host_with_config(storage, config, options, paired_host).await
    }

    /// Embedding-host counterpart of open_paired_host. Config must be the native
    /// effective view for this realm, as for open_with_config.
    pub async fn open_paired_host_with_config(
        storage: ResolvedStorage,
        config: Config,
        options: SessionOptions,
        paired_host: &crate::paired_host_tool::PairedHostFrontend,
    ) -> Result<Self, ConnectError> {
        if !(crate::setup::is_direct_paired_host_composition(&options)
            || crate::setup::is_interactive_paired_host_composition(&options))
            || !options
                .build
                .external_tools
                .as_ref()
                .is_some_and(|tools| paired_host.matches(tools))
            || options.build.tool_dispatch_admission.is_none()
        {
            return Err(ConnectError::InvalidInput(
                "PairedHost research requires its exact local composition and dispatch admission"
                    .into(),
            ));
        }
        Self::open_inner(storage, config, options, paired_host.research()).await
    }

    async fn open_inner(
        storage: ResolvedStorage,
        config: Config,
        options: SessionOptions,
        research: Option<crate::research::ResearchBinding>,
    ) -> Result<Self, ConnectError> {
        // The shared native construction retains sizeable factory/config state.
        // Keep it out of every embedding caller's inline async stack frame.
        Box::pin(Self::open_composed(
            storage, config, options, research, None,
        ))
        .await
    }

    async fn open_composed(
        storage: ResolvedStorage,
        config: Config,
        mut options: SessionOptions,
        research: Option<crate::research::ResearchBinding>,
        startup: Option<SessionStartup>,
    ) -> Result<Self, ConnectError> {
        if options
            .build
            .realm_id
            .as_ref()
            .is_some_and(|id| id != &storage.locator.realm)
        {
            return Err(ConnectError::InvalidInput(
                "Session options name a different realm".into(),
            ));
        }
        if options.build.resume_session.is_some() {
            return Err(ConnectError::InvalidInput("This setup entry creates a new conversation; native resume needs its own explicit admission".into()));
        }
        if options
            .model
            .provider
            .zip(options.build.provider)
            .is_some_and(|(a, b)| a != b)
            || options
                .model
                .auth_binding
                .as_ref()
                .zip(options.build.auth_binding.as_ref())
                .is_some_and(|(a, b)| a != b)
        {
            return Err(ConnectError::InvalidInput(
                "Model and build options name different provider authentication targets".into(),
            ));
        }
        options.model.provider = options.model.provider.or(options.build.provider);
        options.model.auth_binding = options
            .model
            .auth_binding
            .or(options.build.auth_binding.clone());
        let model = resolve_create_session_model(&config, options.model)
            .map_err(|e| ConnectError::native("resolve setup model", e))?;
        options.build.realm_id = Some(storage.locator.realm.clone());
        options.build.provider = Some(model.provider);
        if let Some(binding) = model.auth_binding {
            options.build.auth_binding = Some(binding);
        }
        let research = research
            .map(|binding| binding.bind(&config, model.provider, &model.model, &mut options.build))
            .transpose()?;
        let mut startup = match startup {
            Some(startup) => startup,
            None => SessionStartup::with_config(storage.clone(), config.clone()).await?,
        };
        startup.persistence().await?;
        let SessionStartup {
            factory,
            persistence,
            connector_auth,
            ..
        } = startup;
        let persistence = persistence.ok_or_else(|| {
            ConnectError::InvalidInput(
                "Native startup did not retain its persistence bundle".into(),
            )
        })?;
        let (service, machine) =
            build_persistent_service_with_runtime_adapter(factory, config, 1, persistence)
                .map_err(|error| ConnectError::native("Build persistent setup service", error))?;
        let service = Arc::new(service);
        let session = Session::new();
        let session_id = session.id().clone();
        let admission = service
            .reserve_create_session_admission()
            .await
            .map_err(|e| ConnectError::native("reserve setup session admission", e))?;
        let request = CreateSessionRequest {
            model: model.model,
            prompt: String::new().into(),
            injected_context: Vec::new(),
            system_prompt: SystemPromptOverride::Set(format!(
                "{SETUP_INSTRUCTIONS}\nThe selected native realm is {}.\n",
                storage.locator.realm.as_str(),
            )),
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(options.build),
            labels: None,
        };
        let executor_service = service.clone();
        let executor_machine = machine.clone();
        let (attachment_tx, attachment_rx) = tokio::sync::oneshot::channel();
        // Keep the native materialization future out of the presenter's stack
        // frame, as in the native runtime-backed service examples.
        let result = Box::pin(materialize_session_with_reserved_admission_and_actor_slot(
            &service,
            &machine,
            session,
            request,
            admission,
            move |id, attachment, actor_slot| {
                let _ = attachment_tx.send(attachment);
                Box::new(
                    PersistentRuntimeExecutor::new(executor_service, executor_machine, id)
                        .with_publication_actor_slot(actor_slot),
                )
            },
        ))
        .await;
        if let Err(error) = result {
            // Materialization owns its admission/attachment rollback. Also close
            // the service we created; do not launch a replacement conversation.
            let cleanup = service.try_shutdown().await;
            if let Err(cleanup_error) = cleanup {
                return Err(ConnectError::Cleanup {
                    primary: Box::new(ConnectError::native(
                        "materialize setup conversation",
                        error,
                    )),
                    cleanup: Box::new(ConnectError::native(
                        "close failed setup materialization",
                        cleanup_error,
                    )),
                });
            }
            return Err(ConnectError::native(
                "materialize setup conversation",
                error,
            ));
        }
        let Some(registration) = machine
            .current_session_registration_witness(&session_id)
            .await
        else {
            return match service.try_shutdown().await {
                Ok(()) => Err(ConnectError::MissingRegistration),
                Err(error) => Err(ConnectError::Cleanup {
                    primary: Box::new(ConnectError::MissingRegistration),
                    cleanup: Box::new(ConnectError::native(
                        "close unregistered setup service",
                        error,
                    )),
                }),
            };
        };
        let attachment = match attachment_rx.await {
            Ok(attachment) => attachment,
            Err(_) => {
                return Err(close_missing_attachment(
                    service,
                    machine,
                    registration,
                    research,
                    connector_auth,
                )
                .await);
            }
        };
        let session = Self {
            service,
            machine,
            session_id,
            realm: storage.locator.clone(),
            registration,
            attachment,
            _research: research,
            service_preparation: None,
            _connector_auth: connector_auth,
            stock: None,
            stock_submission: Arc::new(StockSubmission::default()),
        };
        if session
            ._research
            .as_ref()
            .is_some_and(|scope| !scope.captured())
        {
            let primary = ConnectError::InvalidInput(
                "Native research did not capture exactly one selected client".into(),
            );
            return match session.close().await {
                Ok(()) => Err(primary),
                Err(cleanup) => Err(ConnectError::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(cleanup),
                }),
            };
        }
        Ok(session)
    }

    // Test access to the same construction, actual actor client and attachment.
    // These helpers do not construct authorization or replace the native input.
    #[cfg(test)]
    pub(crate) async fn fixture_open_governed(
        startup: SessionStartup,
        options: SessionOptions,
    ) -> Result<Self, ConnectError> {
        Box::pin(Self::open_composed(
            startup.storage.clone(),
            startup.config.clone(),
            options,
            None,
            Some(startup),
        ))
        .await
    }

    #[cfg(test)]
    pub(crate) async fn fixture_controller(
        &self,
    ) -> Result<meerkat_core::ControllerModelClient, ConnectError> {
        let lease = self
            .service
            .acquire_live_session_actor_turn_boundary_lease(&self.session_id)
            .await
            .map_err(|error| ConnectError::native("pin fixture actor", error))?;
        self.service
            .pin_controller_client_for_actor(&lease)
            .await
            .map_err(|error| ConnectError::native("pin fixture controller", error))
    }

    #[cfg(test)]
    pub(crate) async fn fixture_submit_bound(
        &self,
        input: Input,
    ) -> Result<(AcceptOutcome, CompletionHandle), ConnectError> {
        let (admission, completion) = self
            .machine
            .accept_input_with_completion_for_attachment(&self.attachment, input)
            .await
            .map_err(|error| ConnectError::native("submit bound fixture input", error))?;
        match completion {
            Some(completion) => Ok((admission, completion)),
            None => Err(ConnectError::MissingCompletion {
                admission: Box::new(admission),
            }),
        }
    }

    pub fn id(&self) -> &SessionId {
        &self.session_id
    }

    pub(crate) async fn public_history(
        &self,
        offset: Option<usize>,
    ) -> Result<serde_json::Value, ConnectError> {
        use meerkat_core::service::{SessionHistoryQuery, SessionServiceHistoryExt};
        let mut page = self
            .service
            .read_history(
                &self.session_id,
                SessionHistoryQuery {
                    offset: offset.unwrap_or(0),
                    limit: Some(50),
                },
            )
            .await
            .map_err(|e| ConnectError::native("read setup history", e))?;
        if offset.is_none() && page.message_count > 50 {
            page = self
                .service
                .read_history(
                    &self.session_id,
                    SessionHistoryQuery {
                        offset: page.message_count - 50,
                        limit: Some(50),
                    },
                )
                .await
                .map_err(|e| ConnectError::native("read setup history", e))?;
        }
        let messages: Vec<_> = page
            .messages
            .iter()
            .filter_map(|message| {
                let (role, text) = match message {
                    meerkat_core::Message::User(user) => ("user", user.text_content()),
                    meerkat_core::Message::BlockAssistant(assistant) => {
                        ("assistant", assistant.text_blocks().collect::<String>())
                    }
                    _ => return None,
                };
                Some(serde_json::json!({"role":role,"text":text}))
            })
            .collect();
        Ok(
            serde_json::json!({"messages":messages,"offset":page.offset,"total":page.message_count,"hasMore":page.has_more}),
        )
    }

    pub async fn events(&self) -> Result<EventStream, ConnectError> {
        self.service
            .subscribe_session_events(&self.session_id)
            .await
            .map_err(|e| ConnectError::native("subscribe to setup events", e))
    }

    /// Submit literal human text. Native admission owns ordering and completion.
    pub async fn submit(
        &self,
        message: String,
    ) -> Result<(AcceptOutcome, CompletionHandle), ConnectError> {
        self.submit_input(Input::Prompt(PromptInput::new(message, None)))
            .await
    }

    /// Bind this invocation's actual administrative result once as a system
    /// notice under the native owner's sanctioned local-operator mandate.
    /// The caller retains the typed outcome; it is never human consent. A
    /// failed or uncertain submission is returned, never rebound or retried.
    /// Unconfirmed admission retains the bound input in this process; a later
    /// submission cannot bind while that owner remains unresolved.
    pub async fn submit_finalization(
        &self,
        notice: crate::terminal_connector::FinalizationNotice,
    ) -> Result<(AcceptOutcome, CompletionHandle), ConnectError> {
        if self.stock.is_none() || !notice.belongs_to(&self.realm, &self.session_id) {
            return Err(ConnectError::InvalidInput("Finalization notices require this exact resumed realm, session and current governed owner".into()));
        }
        self.submit_input(notice.into_input()).await
    }

    async fn submit_input(
        &self,
        input: Input,
    ) -> Result<(AcceptOutcome, CompletionHandle), ConnectError> {
        if let Some(stock) = &self.stock {
            // This async-mutex guard reserves the one submission before any
            // actor lookup or bind await. An occupied slot is never rebound.
            let mut reserved = self.stock_submission.reserve()?;
            let witness = self
                .service
                .live_session_actor_witness(&self.session_id)
                .await
                .ok_or_else(|| {
                    ConnectError::InvalidInput("The governed setup actor is unavailable".into())
                })?;
            let lease = self
                .service
                .acquire_live_session_actor_turn_boundary_lease_exact(&witness)
                .await
                .map_err(|e| ConnectError::native("Pin governed setup actor", e))?
                .ok_or_else(|| {
                    ConnectError::InvalidInput("The governed setup actor changed".into())
                })?;
            let controller = self
                .service
                .pin_controller_client_for_actor(&lease)
                .await
                .map_err(|e| ConnectError::native("Pin governed setup controller", e))?;
            // Both stock contracts retain this actual pin. Release the actor
            // boundary before native binding/admission, preserving input kind.
            drop(lease);
            let bound = match input {
                Input::Prompt(prompt) => {
                    stock
                        .bind_prompt(
                            &self.machine,
                            &self.session_id,
                            Input::Prompt(prompt),
                            controller,
                        )
                        .await
                }
                Input::ExternalEvent(event) => {
                    stock
                        .bind_host_outcome(&self.machine, &self.session_id, event, controller)
                        .await
                }
                _ => {
                    return Err(ConnectError::InvalidInput(
                        "Governed setup accepts human prompts or typed host outcomes".into(),
                    ));
                }
            };
            let (input, pending) =
                bound.map_err(|e| ConnectError::native("Bind governed setup original", e))?;
            let observation = Arc::new(OnceLock::new());
            *reserved = Some(BoundStockSubmission {
                input,
                pending: Some(pending),
                observation: observation.clone(),
                _receipt: None,
            });
            // Keep the exact occurrence outside the native wait. The callback
            // records only native's classification and never enters publication.
            let submitted = reserved
                .as_ref()
                .expect("the bound submission was retained")
                .input
                .clone();
            let result = self
                .machine
                .accept_input_with_completion_for_attachment_and_replay_policy_with_custody(
                    &self.attachment,
                    submitted,
                    meerkat_runtime::accept::InputReplayPolicy::KeyOnly,
                    Box::new(StockAdmissionObservation(observation.clone())),
                )
                .await;
            let settlement = reserved
                .as_ref()
                .and_then(|bound| bound.observation.get().copied());
            match result {
                Err(error) => {
                    let primary = ConnectError::native("Submit governed setup message", error);
                    if settlement == Some(NativeInputAdmissionSettlement::NotAdmitted) {
                        reserved.take();
                        return Err(primary);
                    }
                    // Native may report admission before a later error. Promote
                    // only that actual disposition, after the callback returned.
                    let error = if settlement == Some(NativeInputAdmissionSettlement::Admitted) {
                        match reserved.as_mut().expect("retained submission").promote() {
                            Ok(()) => primary,
                            Err(promotion) => ConnectError::Cleanup {
                                primary: Box::new(primary),
                                cleanup: Box::new(promotion),
                            },
                        }
                    } else {
                        primary
                    };
                    return Err(self.stock_submission.retain_error(error));
                }
                Ok((admission, completion)) => {
                    let exact_accepted = matches!(&admission, AcceptOutcome::Accepted { input_id, .. }
                        if input_id == reserved.as_ref().expect("retained submission").input.id());
                    if settlement == Some(NativeInputAdmissionSettlement::Admitted)
                        && exact_accepted
                    {
                        if let Err(primary) =
                            reserved.as_mut().expect("retained submission").promote()
                        {
                            // Consuming promotion failure cannot restore the
                            // original or admit a new bind. Keep its exact input
                            // and receipt even after explicit cancellation joins.
                            reserved.as_mut().expect("retained submission")._receipt =
                                Some((admission.clone(), None));
                            let cancelled = self.cancel(&admission).await.map(|_| ());
                            let settled = match completion {
                                Some(completion) => completion
                                    .try_wait_with_terminal_outcome()
                                    .await
                                    .map(|_| ())
                                    .map_err(|error| {
                                        ConnectError::native(
                                            "Observe refused setup original settlement",
                                            error,
                                        )
                                    }),
                                None => Err(ConnectError::MissingCompletion {
                                    admission: Box::new(admission),
                                }),
                            };
                            let error = match standalone::combine(
                                Err(primary),
                                standalone::combine(cancelled, settled),
                            ) {
                                Err(error) => error,
                                Ok(()) => unreachable!(),
                            };
                            return Err(self.stock_submission.retain_error(error));
                        }
                        reserved.take();
                    } else if settlement == Some(NativeInputAdmissionSettlement::NotAdmitted)
                        && matches!(&admission, AcceptOutcome::Rejected { .. })
                    {
                        reserved.take();
                    } else {
                        // The unchanged KeyOnly contract cannot authenticate a
                        // duplicate payload. Keep the actual receipt/handle and
                        // block another bind rather than promote by enum tag.
                        reserved.as_mut().expect("retained submission")._receipt =
                            Some((admission, completion));
                        return Err(self.stock_submission.retain_error(ConnectError::InvalidInput(
                            "Native admission did not confirm this exact bound input; its outcome remains retained".into(),
                        )));
                    }
                    return match completion {
                        Some(completion) => Ok((admission, completion)),
                        None => Err(ConnectError::MissingCompletion {
                            admission: Box::new(admission),
                        }),
                    };
                }
            }
        }
        let (admission, completion) = self
            .machine
            .accept_input_with_completion_for_attachment(&self.attachment, input)
            .await
            .map_err(|e| ConnectError::native("submit setup message", e))?;
        match completion {
            Some(completion) => Ok((admission, completion)),
            None => Err(ConnectError::MissingCompletion {
                admission: Box::new(admission),
            }),
        }
    }

    /// Cancel only the input named by its native admission receipt. A stale
    /// presenter cannot cancel a newer input that happens to be active now.
    pub async fn cancel(&self, admission: &AcceptOutcome) -> Result<bool, ConnectError> {
        let input_id = match admission {
            AcceptOutcome::Accepted { input_id, .. } => input_id,
            AcceptOutcome::Deduplicated { existing_id, .. } => existing_id,
            _ => {
                return Err(ConnectError::InvalidInput(
                    "Native receipt does not identify an accepted input".into(),
                ));
            }
        };
        self.machine
            .cancel_input_if_present(
                &self.session_id,
                input_id,
                "setup presenter requested cancellation",
            )
            .await
            .map_err(|e| ConnectError::native("cancel setup turn", e))
    }

    /// Ask native custody to close this exact registration. Busy preserves
    /// accepted work. Closed is actual terminal teardown, which can also finish
    /// an ownerless drain. Non-Closed observations return this same owner.
    /// The entered native close is joined; caller cancellation is handled only
    /// after this method returns its actual outcome.
    pub async fn try_close_for_finalization(self) -> FinalizationClose {
        use meerkat_runtime::RuntimeSessionQuiescentClose;
        let observation = self
            .machine
            .unregister_session_registration_until_terminal_if_quiescent(&self.registration)
            .await;
        match observation {
            Ok(RuntimeSessionQuiescentClose::Closed) => {
                // This private transition is available only after native's
                // owned teardown. The returned owner cannot unregister again.
                FinalizationClose::Closed(ClosedSetup {
                    session: Arc::new(self),
                })
            }
            Ok(RuntimeSessionQuiescentClose::Busy) => FinalizationClose::Busy(self),
            other => FinalizationClose::Unconfirmed {
                session: self,
                observation: other
                    .map_err(|error| ConnectError::native("Observe quiescent setup close", error)),
            },
        }
    }

    /// Tear down this exact registration, never a same-id replacement. As with
    /// the session contract, first cancel any active input and await completion.
    /// Retire Toolkit admission before runtime teardown. Legacy lazy setup closes
    /// its retained adapter here; standalone setup leaves adapter shutdown to the
    /// native factory and runtime owner that created it. A returned failure
    /// retains this whole process-local owner. Dropping the final error or
    /// exiting the process is not evidence of joined cleanup.
    pub async fn close(self) -> Result<(), ConnectError> {
        if let Some(owner) = &self.service_preparation {
            // The preparation owner retires admission in both modes and closes
            // an adapter only when Toolkit originally created that adapter.
            owner.shutdown().await;
        }
        let unregister = self
            .machine
            .unregister_session_registration_until_terminal_if_current(&self.registration)
            .await
            .map_err(|e| ConnectError::native("unregister setup runtime", e));
        let shutdown = self
            .service
            .try_shutdown()
            .await
            .map_err(|e| ConnectError::native("close setup service", e));
        let result = match (unregister, shutdown) {
            (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            }),
            (Err(error), _) | (_, Err(error)) => Err(error),
            (Ok(true), Ok(())) => {
                self.stock_submission.release_after_confirmed_close().await;
                Ok(())
            }
            (Ok(false), Ok(())) => Err(ConnectError::InvalidInput(
                "This close did not confirm terminal teardown of the selected registration".into(),
            )),
        };
        match result {
            Ok(()) => Ok(()),
            Err(error) => Err(retain_failed_close(Arc::new(self), error)),
        }
    }
}

// One in-process holder for the native candidate. It neither serializes the
// input nor creates a retry authorization; PendingOriginal remains native.
#[derive(Default)]
struct StockSubmission {
    candidate: tokio::sync::Mutex<Option<BoundStockSubmission>>,
}

struct BoundStockSubmission {
    input: Input,
    pending: Option<meerkat::governed_realm::stock::originals::PendingOriginal>,
    observation: Arc<OnceLock<NativeInputAdmissionSettlement>>,
    _receipt: Option<(AcceptOutcome, Option<CompletionHandle>)>,
}

impl BoundStockSubmission {
    fn promote(&mut self) -> Result<(), ConnectError> {
        self.pending
            .take()
            .expect("one promotion of the retained candidate")
            .admit()
            .map_err(|error| ConnectError::native("Retain accepted setup original", error))
    }
}

struct StockAdmissionObservation(Arc<OnceLock<NativeInputAdmissionSettlement>>);

impl NativeInputAdmissionCustody for StockAdmissionObservation {
    fn settle(
        self: Box<Self>,
        settlement: NativeInputAdmissionSettlement,
    ) -> Option<NativeInputAdmissionCompletion> {
        // Native owns the classification. This callback must stay local and
        // infallible: no pending.admit, publication, I/O, await or restoration.
        let _ = self.0.set(settlement);
        None
    }
}

impl StockSubmission {
    fn reserve(
        self: &Arc<Self>,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<BoundStockSubmission>>, ConnectError> {
        let guard = self.candidate.try_lock().map_err(|_| self.retain_error(ConnectError::InvalidInput(
            "Another governed submission is being bound or admitted; no new occurrence was created".into(),
        )))?;
        if guard.is_some() {
            return Err(self.retain_error(ConnectError::InvalidInput(
                "The prior governed submission remains retained; no new occurrence was created"
                    .into(),
            )));
        }
        Ok(guard)
    }

    fn retain_error(self: &Arc<Self>, source: ConnectError) -> ConnectError {
        ConnectError::native(
            "Governed submission ownership",
            StockSubmissionError {
                source: Box::new(source),
                _custody: self.clone(),
            },
        )
    }

    async fn release_after_confirmed_close(&self) {
        self.candidate.lock().await.take();
    }
}

// Share the same holder with returned failures. A consuming close error must
// not accidentally withdraw the candidate by dropping the session. Native
// error identity remains reachable through the standard source chain.
struct StockSubmissionError {
    source: Box<ConnectError>,
    _custody: Arc<StockSubmission>,
}

impl std::fmt::Debug for StockSubmissionError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("StockSubmissionError")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for StockSubmissionError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(
            "the original submission owner and error are retained; do not retry automatically",
        )
    }
}

impl std::error::Error for StockSubmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// Custody returned by one automatic administrative close attempt. A native
/// Closed result can still have failed host cleanup and cannot then finalize.
pub enum FinalizationClose {
    Closed(ClosedSetup),
    Busy(ConnectSession),
    Unconfirmed {
        session: ConnectSession,
        // Preserve the actual native variant or typed error source. Neither
        // AlreadyClosing nor NotCurrent is a joined cleanup receipt.
        observation: Result<meerkat_runtime::RuntimeSessionQuiescentClose, ConnectError>,
    },
}

/// A native Closed receipt with host cleanup still owned by this process.
/// Only the successful cleanup result permits the administrative handoff.
pub struct ClosedSetup {
    session: Arc<ConnectSession>,
}

impl ClosedSetup {
    /// Finish only host cleanup. On failure retain this exact cleanup owner;
    /// an explicit retry never repeats native registration teardown.
    pub async fn finish_host_cleanup(self) -> Result<(), (Self, ConnectError)> {
        if let Some(owner) = &self.session.service_preparation {
            owner.shutdown().await;
        }
        match self.session.service.try_shutdown().await {
            Ok(()) => {
                self.session
                    .stock_submission
                    .release_after_confirmed_close()
                    .await;
                Ok(())
            }
            Err(error) => {
                let error = retain_failed_close(
                    self.session.clone(),
                    ConnectError::native(
                        "Close quiescent setup service",
                        HostCleanupError(Box::new(error)),
                    ),
                );
                Err((self, error))
            }
        }
    }
}

// A failed consuming close retains the complete existing owner, including the
// exact registration, actor service and any unconfirmed bound input. Keeping
// this error is process-local custody, not a durable recovery or retry handle.
fn retain_failed_close(session: Arc<ConnectSession>, source: ConnectError) -> ConnectError {
    ConnectError::native(
        "Setup teardown remains unconfirmed",
        RetainedSetupCloseError {
            source: Box::new(source),
            _session: session,
        },
    )
}

struct RetainedSetupCloseError {
    source: Box<ConnectError>,
    _session: Arc<ConnectSession>,
}

impl std::fmt::Debug for RetainedSetupCloseError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("RetainedSetupCloseError")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for RetainedSetupCloseError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("the original setup owner and cleanup error are retained")
    }
}

impl std::error::Error for RetainedSetupCloseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

// A missing attachment cannot be converted into a complete ConnectSession.
// Close only its actual construction handles and retain them on unconfirmed
// cleanup. No replacement attachment or registration is selected here.
async fn close_missing_attachment(
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    machine: Arc<MeerkatMachine>,
    registration: RuntimeSessionRegistrationWitness,
    research: Option<crate::research::ResearchScope>,
    connector_auth: Option<Arc<crate::connector_auth::ConnectorAuth>>,
) -> ConnectError {
    close_failed_construction(
        ConnectError::MissingAttachment,
        service,
        machine,
        Some(registration),
        research,
        connector_auth,
        None,
        None,
    )
    .await
}

// Preserve the original failure and exact partial construction owners. Missing
// registration custody cannot be recovered by looking up a logical SessionId.
async fn close_failed_construction(
    primary: ConnectError,
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    machine: Arc<MeerkatMachine>,
    registration: Option<RuntimeSessionRegistrationWitness>,
    research: Option<crate::research::ResearchScope>,
    connector_auth: Option<Arc<crate::connector_auth::ConnectorAuth>>,
    stock: Option<Arc<meerkat::governed_realm::stock::StockGovernance>>,
    setup: Option<Arc<crate::service_preparation::NativeSetup>>,
) -> ConnectError {
    let unregister = match &registration {
        Some(registration) => machine
            .unregister_session_registration_until_terminal_if_current(registration)
            .await
            .map_err(|error| ConnectError::native("close failed setup construction", error)),
        None => Err(ConnectError::MissingRegistration),
    };
    let observed_unregister = unregister.as_ref().ok().copied();
    // Attempt service cleanup even when exact registration teardown refused.
    // Service shutdown alone cannot upgrade false or an error into a join.
    let shutdown = service
        .try_shutdown()
        .await
        .map_err(|error| ConnectError::native("close failed setup service", error));
    let unregister = unregister.and_then(|joined| if joined { Ok(()) } else {
        Err(ConnectError::InvalidInput("Setup construction cleanup did not confirm terminal teardown of the selected registration".into()))
    });
    match standalone::combine(unregister, shutdown) {
        Ok(()) => primary,
        Err(source) => ConnectError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(ConnectError::native(
                "Setup construction cleanup remains unconfirmed",
                RetainedSetupConstructionError {
                    source: Box::new(source),
                    observed_unregister,
                    _service: service,
                    _machine: machine,
                    _registration: registration,
                    _research: research,
                    _connector_auth: connector_auth,
                    _stock: stock,
                    _setup: setup,
                },
            )),
        },
    }
}

// Same process-local error custody as RetainedSetupCloseError, before an exact
// attachment exists. Retaining this error retains the real construction owner;
// dropping its final copy or exiting the process does not prove joined cleanup.
struct RetainedSetupConstructionError {
    source: Box<ConnectError>,
    // Some is the exact returned bool. None retains the native error or
    // MissingRegistration in source; neither is a selected-registration join.
    observed_unregister: Option<bool>,
    _service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    _machine: Arc<MeerkatMachine>,
    _registration: Option<RuntimeSessionRegistrationWitness>,
    _research: Option<crate::research::ResearchScope>,
    _connector_auth: Option<Arc<crate::connector_auth::ConnectorAuth>>,
    _stock: Option<Arc<meerkat::governed_realm::stock::StockGovernance>>,
    _setup: Option<Arc<crate::service_preparation::NativeSetup>>,
}
impl std::fmt::Debug for RetainedSetupConstructionError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("RetainedSetupConstructionError")
            .field("observed_unregister", &self.observed_unregister)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for RetainedSetupConstructionError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("the original setup construction owner and cleanup error are retained")
    }
}
impl std::error::Error for RetainedSetupConstructionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

// Keep the actual native error in the source chain without rendering arbitrary
// native payloads while the terminal reports this bounded cleanup class.
#[derive(Debug)]
struct HostCleanupError(Box<dyn std::error::Error + Send + Sync>);

impl std::fmt::Display for HostCleanupError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("host cleanup was not confirmed; native details are retained")
    }
}

impl std::error::Error for HostCleanupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[cfg(test)]
mod quiescent_close_tests {
    use super::*;

    #[test]
    fn cleanup_error_preserves_typed_source_without_rendering_its_payload() {
        let error = HostCleanupError(Box::new(std::io::Error::other(
            "private native fixture detail",
        )));
        assert!(!error.to_string().contains("private native fixture detail"));
        let source = std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap();
        assert_eq!(source.to_string(), "private native fixture detail");
    }

    #[tokio::test]
    async fn native_custody_observation_distinguishes_accepted_key_only_duplicate_and_nonentry() {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let temp = tempfile::tempdir().unwrap();
            let storage = crate::RealmOptions {
                context_root: temp.path().to_owned(),
                state_root: Some(temp.path().join("realms")),
                user_config_root: Some(temp.path().join("home")),
            }.resolve("custody-observation").unwrap();
            let session = ConnectSession::open_with_config(storage, Config::default(), SessionOptions {
                model: CreateSessionModelResolutionRequest { model: Some("gpt-5.4".into()), ..Default::default() },
                build: SessionBuildOptions {
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(Arc::new(
                        meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI)
                    ))),
                    ..Default::default()
                },
            }).await.unwrap();
            let machine = session.machine.clone();
            let attachment = session.attachment.clone();
            let mut prompt = PromptInput::new("Actual native custody observation", None);
            prompt.header.idempotency_key = Some(meerkat_runtime::identifiers::IdempotencyKey::new("custody-observation"));
            let input = Input::Prompt(prompt);
            let first_observation = Arc::new(OnceLock::new());
            let first = machine.accept_input_with_completion_for_attachment_and_replay_policy_with_custody(
                &attachment, input.clone(), meerkat_runtime::accept::InputReplayPolicy::KeyOnly,
                Box::new(StockAdmissionObservation(first_observation.clone())),
            ).await;
            let duplicate_observation = Arc::new(OnceLock::new());
            let duplicate = machine.accept_input_with_completion_for_attachment_and_replay_policy_with_custody(
                &attachment, input.clone(), meerkat_runtime::accept::InputReplayPolicy::KeyOnly,
                Box::new(StockAdmissionObservation(duplicate_observation.clone())),
            ).await;
            let not_polled = Arc::new(OnceLock::new());
            drop(machine.accept_input_with_completion_for_attachment_and_replay_policy_with_custody(
                &attachment, input.clone(), meerkat_runtime::accept::InputReplayPolicy::KeyOnly,
                Box::new(StockAdmissionObservation(not_polled.clone())),
            ));
            // Join actual fixture teardown and both returned handles before
            // assertions. This control observes native classification, not a
            // stock promotion or a fabricated post-commit failure.
            let cleanup = session.close().await;
            let mut receipts = Vec::new();
            let mut completions_joined = true;
            for result in [first, duplicate] {
                match result {
                    Ok((admission, completion)) => {
                        receipts.push(admission);
                        if let Some(completion) = completion {
                            completions_joined &= completion.try_wait_with_terminal_outcome().await.is_ok();
                        }
                    }
                    Err(_) => completions_joined = false,
                }
            }
            let stale_observation = Arc::new(OnceLock::new());
            let stale = if cleanup.is_ok() {
                Some(machine.accept_input_with_completion_for_attachment_and_replay_policy_with_custody(
                    &attachment, input.clone(), meerkat_runtime::accept::InputReplayPolicy::KeyOnly,
                    Box::new(StockAdmissionObservation(stale_observation.clone())),
                ).await)
            } else { None };
            assert!(cleanup.is_ok() && completions_joined);
            assert_eq!(first_observation.get(), Some(&NativeInputAdmissionSettlement::Admitted));
            assert_eq!(duplicate_observation.get(), Some(&NativeInputAdmissionSettlement::Uncertain));
            assert_eq!(not_polled.get(), Some(&NativeInputAdmissionSettlement::NotAdmitted));
            assert_eq!(stale_observation.get(), Some(&NativeInputAdmissionSettlement::NotAdmitted));
            assert!(matches!(stale, Some(Err(_))));
            assert!(matches!(receipts.first(), Some(AcceptOutcome::Accepted { input_id, .. }) if input_id == input.id()));
            assert!(matches!(receipts.get(1), Some(AcceptOutcome::Deduplicated { existing_id, .. }) if existing_id == input.id()),
                "matching input identity alone does not upgrade KeyOnly custody");
        }).await.expect("bounded actual native custody observation and fixture cleanup");
    }

    #[tokio::test]
    async fn missing_attachment_cleanup_preserves_primary_and_exact_unconfirmed_owner() {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            for already_removed in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let storage = crate::RealmOptions {
                    context_root: temp.path().to_owned(),
                    state_root: Some(temp.path().join("realms")),
                    user_config_root: Some(temp.path().join("home")),
                }.resolve("construction-cleanup").unwrap();
                let session = ConnectSession::open_with_config(storage, Config::default(), SessionOptions {
                    model: CreateSessionModelResolutionRequest { model: Some("gpt-5.4".into()), ..Default::default() },
                    build: SessionBuildOptions {
                        llm_client_override: Some(meerkat::encode_llm_client_override_for_service(Arc::new(
                            meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI)
                        ))),
                        ..Default::default()
                    },
                }).await.unwrap();
                let session_id = session.session_id.clone();
                let epoch = session.registration.epoch_id().clone();
                // Exercise the actual cleanup helper with the real construction
                // handles. This does not fabricate an attachment-channel fault.
                let ConnectSession { service, machine, registration, _research, _connector_auth, .. } = session;
                let retained_service = service.clone();
                let retained_machine = machine.clone();
                let retained_registration = registration.clone();
                let first_close = if already_removed {
                    Some(machine.unregister_session_registration_until_terminal_if_current(&registration).await)
                } else { None };
                let error = close_missing_attachment(service, machine, registration, _research, _connector_auth).await;
                let absent_after_cleanup = retained_machine.current_session_registration_witness(&session_id).await.is_none();
                let expected = if already_removed {
                    match &error {
                        ConnectError::Cleanup { primary, cleanup } if matches!(primary.as_ref(), ConnectError::MissingAttachment) => {
                            match cleanup.as_ref() {
                                ConnectError::Native { source, .. } => source.downcast_ref::<RetainedSetupConstructionError>()
                                    .is_some_and(|owner| owner.observed_unregister == Some(false)
                                        && Arc::ptr_eq(&retained_service, &owner._service)
                                        && Arc::ptr_eq(&retained_machine, &owner._machine)
                                        && owner._registration.as_ref().is_some_and(|registration|
                                            registration.session_id() == &session_id
                                                && registration.epoch_id() == &epoch)),
                                _ => false,
                            }
                        }
                        _ => false,
                    }
                } else { matches!(&error, ConnectError::MissingAttachment) };
                // Complete fixture cleanup before assertions, including on an
                // unexpected result. This later call does not upgrade the
                // earlier false observation or prove shutdown was attempted by
                // the branch under test.
                let final_unregister = retained_machine.unregister_session_registration_until_terminal_if_current(&retained_registration).await;
                let final_shutdown = retained_service.try_shutdown().await;
                assert!(final_unregister.is_ok() && final_shutdown.is_ok());
                if already_removed {
                    assert!(matches!(first_close, Some(Ok(true))), "the fixture's first exact true result proves its join");
                }
                assert!(absent_after_cleanup && expected,
                    "primary failure and exact false-observation custody must survive construction cleanup");
            }
        }).await.expect("bounded real construction cleanup and fixture teardown");
    }

    #[tokio::test]
    async fn failed_construction_cleanup_retains_primary_and_missing_witness_uncertainty() {
        #[derive(Debug)]
        struct Primary(Arc<()>);
        impl std::fmt::Display for Primary {
            fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                output.write_str("original construction failure")
            }
        }
        impl std::error::Error for Primary {}
        let exact_primary = |error: &ConnectError, marker: &Arc<()>| match error {
            ConnectError::Native { operation, source } if *operation == "fixture construction" => {
                source
                    .downcast_ref::<Primary>()
                    .is_some_and(|source| Arc::ptr_eq(&source.0, marker))
            }
            _ => false,
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            for witness in ["current", "removed", "missing"] {
                let temp = tempfile::tempdir().unwrap();
                let storage = crate::RealmOptions {
                    context_root: temp.path().to_owned(),
                    state_root: Some(temp.path().join("realms")),
                    user_config_root: Some(temp.path().join("home")),
                }.resolve("failed-construction").unwrap();
                let session = ConnectSession::open_with_config(storage, Config::default(), SessionOptions {
                    model: CreateSessionModelResolutionRequest { model: Some("gpt-5.4".into()), ..Default::default() },
                    build: SessionBuildOptions {
                        llm_client_override: Some(meerkat::encode_llm_client_override_for_service(Arc::new(
                            meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI)
                        ))),
                        ..Default::default()
                    },
                }).await.unwrap();
                let ConnectSession { service, machine, registration, _research, _connector_auth, stock, .. } = session;
                let retained_service = service.clone();
                let retained_machine = machine.clone();
                let retained_registration = registration.clone();
                // Both negative cases use an actual successful prior close.
                // The missing-witness case then passes None with an empty
                // service; that is still no join proof available to the helper.
                let first_close = if witness != "current" {
                    Some(machine.unregister_session_registration_until_terminal_if_current(&registration).await)
                } else { None };
                let marker = Arc::new(());
                let primary = ConnectError::native("fixture construction", Primary(marker.clone()));
                let selected = if witness == "missing" { None } else { Some(registration) };
                let error = close_failed_construction(primary, service, machine, selected,
                    _research, _connector_auth, stock, None).await;
                let expected = match &error {
                    ConnectError::Cleanup { primary, cleanup } if witness != "current" && exact_primary(primary, &marker) => {
                        match cleanup.as_ref() {
                            ConnectError::Native { source, .. } => source.downcast_ref::<RetainedSetupConstructionError>()
                                .is_some_and(|owner| Arc::ptr_eq(&retained_service, &owner._service)
                                    && Arc::ptr_eq(&retained_machine, &owner._machine)
                                    && if witness == "missing" {
                                        owner.observed_unregister.is_none() && owner._registration.is_none()
                                            && matches!(owner.source.as_ref(), ConnectError::MissingRegistration)
                                    } else {
                                        owner.observed_unregister == Some(false)
                                            && owner._registration.as_ref().is_some_and(|selected|
                                                selected.session_id() == retained_registration.session_id()
                                                    && selected.epoch_id() == retained_registration.epoch_id())
                                    }),
                            _ => false,
                        }
                    }
                    primary if witness == "current" => exact_primary(primary, &marker),
                    _ => false,
                };
                // This uses only the original fixture witness. In the
                // negative cases, first_close supplies the real cleanup proof;
                // its result was never given to the helper or used to upgrade
                // the helper's earlier false/missing observation.
                let final_unregister = retained_machine.unregister_session_registration_until_terminal_if_current(&retained_registration).await;
                let final_shutdown = retained_service.try_shutdown().await;
                assert!(final_shutdown.is_ok() && matches!(final_unregister, Ok(false)));
                if witness != "current" { assert!(matches!(first_close, Some(Ok(true)))); }
                assert!(expected, "retain the exact primary and distinguish missing/false custody from a joined registration");
            }
        }).await.expect("bounded actual construction owners and fixture cleanup");
    }

    #[tokio::test]
    async fn absent_native_registration_returns_exact_unconfirmed_owner() {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let temp = tempfile::tempdir().unwrap();
            let storage = crate::RealmOptions {
                context_root: temp.path().to_owned(),
                state_root: Some(temp.path().join("realms")),
                user_config_root: Some(temp.path().join("home")),
            }
            .resolve("absent-registration")
            .unwrap();
            let session = ConnectSession::open_with_config(
                storage,
                Config::default(),
                SessionOptions {
                    model: CreateSessionModelResolutionRequest {
                        model: Some("gpt-5.4".into()),
                        ..Default::default()
                    },
                    build: SessionBuildOptions {
                        llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                            Arc::new(meerkat_client::TestClient::for_provider(
                                meerkat_core::Provider::OpenAI,
                            )),
                        )),
                        ..Default::default()
                    },
                },
            )
            .await
            .unwrap();
            let machine = session.machine.clone();
            let session_id = session.session_id.clone();
            let epoch = session.registration.epoch_id().clone();
            let first_close = machine
                .unregister_session_registration_until_terminal_if_current(&session.registration)
                .await;
            let preserved = match session.try_close_for_finalization().await {
                FinalizationClose::Unconfirmed {
                    session,
                    observation,
                } => {
                    let preserved = matches!(
                        observation,
                        Ok(meerkat_runtime::RuntimeSessionQuiescentClose::NotCurrent)
                    ) && Arc::ptr_eq(&machine, &session.machine)
                        && session.session_id == session_id
                        && session.registration.epoch_id() == &epoch;
                    // Even with successful host shutdown, a second ordinary
                    // close cannot upgrade NotCurrent into a joined receipt.
                    let cleanup = session
                        .close()
                        .await
                        .expect_err("absent registration remains unconfirmed");
                    let retained = match &cleanup {
                        ConnectError::Native { source, .. } => {
                            source.downcast_ref::<RetainedSetupCloseError>()
                        }
                        _ => None,
                    }
                    .expect("failed consuming close retains its complete original owner");
                    let same_owner = Arc::ptr_eq(&machine, &retained._session.machine)
                        && retained._session.session_id == session_id
                        && retained._session.registration.epoch_id() == &epoch;
                    // The first fixture result, asserted after this cleanup,
                    // must prove the join. Dropping this error is not proof.
                    preserved && same_owner
                }
                FinalizationClose::Busy(session) => {
                    session.close().await.unwrap();
                    false
                }
                FinalizationClose::Closed(closed) => {
                    assert!(closed.finish_host_cleanup().await.is_ok());
                    false
                }
            };
            assert!(
                matches!(first_close, Ok(true)),
                "only the first exact true result proves fixture teardown"
            );
            assert!(
                preserved,
                "NotCurrent must retain the original owner and native observation"
            );
        })
        .await
        .expect("bounded exact-registration control");
    }
}
