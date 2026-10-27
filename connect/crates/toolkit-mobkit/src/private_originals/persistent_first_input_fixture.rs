//! Persistent variant of the existing installed first-input fixture. It keeps
//! the same source, policy, registration, model and cleanup owners; only the
//! concrete native session service and its paired stores differ.
use super::*;

// Test-only transport decoration. It never changes the native runtime, model
// authorization, registration, source or provider request counter.
type AttachmentModelDecorator =
    Box<dyn FnOnce(Arc<dyn LlmClient>) -> Arc<dyn LlmClient> + Send>;

/// Passive capture of the existing native ingress callback. The actual check
/// runs unchanged before an accepted native context is retained for assertions.
pub(crate) type NativeIngressObservations = Arc<std::sync::Mutex<Vec<(
    meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    meerkat_runtime::input_authority::NativeIngressContext,
)>>>;

/// Only durable paths and immutable selected input survive the owner drop.
/// No service, machine, source, registration, pin or live actor is retained.
pub(crate) struct DurableFirstInput {
    directory: Arc<tempfile::TempDir>,
    selection: meerkat_toolkit::host_lifetime::control::HostBootstrapSelection,
    definition: MobDefinition,
    member: DurableAgentSpec,
    session: SessionId,
    submission: PrivateOriginalSubmit,
}

#[path = "custody_recovery_cold_native_tests.rs"]
mod custody_recovery_cold_native_tests;

impl FirstFixture<meerkat::PersistentSessionService<FactoryAgentBuilder>> {
    pub(crate) async fn for_live_attachment() -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>) {
        let (fixture, store, _, _) = Self::construct_persistent(false, None, None, false, None).await;
        (fixture, store)
    }

    pub(crate) async fn for_live_attachment_with_model(
        decorate: impl FnOnce(Arc<dyn LlmClient>) -> Arc<dyn LlmClient> + Send + 'static,
    ) -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>) {
        let (fixture, store, _, _) =
            Self::construct_persistent(false, None, Some(Box::new(decorate)), false, None).await;
        (fixture, store)
    }

    pub(crate) async fn for_live_attachment_with_model_and_human_policy(
        decorate: impl FnOnce(Arc<dyn LlmClient>) -> Arc<dyn LlmClient> + Send + 'static,
    ) -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>) {
        let (fixture, store, _, _) =
            Self::construct_persistent(false, None, Some(Box::new(decorate)), true, None).await;
        (fixture, store)
    }

    pub(crate) async fn for_console_service_batch(
        human: meerkat_core::PrincipalRef,
        decorate: impl FnOnce(Arc<dyn LlmClient>) -> Arc<dyn LlmClient> + Send + 'static,
    ) -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>) {
        let (fixture, store, _, _) = Self::construct_persistent(
            false, None, Some(Box::new(decorate)), true, Some(human)).await;
        (fixture, store)
    }

    pub(crate) async fn for_console_service_batch_with_ingress(
        human: meerkat_core::PrincipalRef,
        decorate: impl FnOnce(Arc<dyn LlmClient>) -> Arc<dyn LlmClient> + Send + 'static,
        observations: NativeIngressObservations,
    ) -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>) {
        let (fixture, store, _, _) = Self::construct_persistent_observed(
            false, None, Some(Box::new(decorate)), true, Some(human), Some(observations)).await;
        (fixture, store)
    }

    pub(crate) async fn for_durable_continuation()
        -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>, DurableFirstInput,
            Arc<crate::installed_host::acquisition::InstalledNativeContinuation>)
    {
        let (fixture, store, retained, continuation) = Self::construct_persistent(true, None, None, false, None).await;
        (fixture, store, retained.unwrap(), continuation.unwrap())
    }

    pub(crate) async fn reopen_durable_continuation(retained: DurableFirstInput)
        -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>,
            Arc<crate::installed_host::acquisition::InstalledNativeContinuation>)
    {
        let (fixture, store, _, continuation) = Self::construct_persistent(true, Some(retained), None, false, None).await;
        (fixture, store, continuation.unwrap())
    }

    pub(crate) async fn reopen_durable_continuation_with_source(
        retained: DurableFirstInput,
        decorate: crate::installed_host::controller_first_input_fixture::PrepareOriginalSource,
    ) -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>,
        Arc<crate::installed_host::acquisition::InstalledNativeContinuation>)
    {
        let (fixture, store, _, continuation) = Self::construct_persistent_observed_source(
            true, Some(retained), None, false, None, None, Some(decorate)).await;
        (fixture, store, continuation.unwrap())
    }

    /// Reuse the real persistent session and its ordinary close/restore path.
    /// The new service parent is selected only while every old owner is closed
    /// and dropped. This is not native Console authentication or provisioning
    /// an attachment grant for an unknown future session.
    pub(crate) async fn for_application_service(
        source_kind: crate::installed_host::controller_first_input_fixture::ApplicationServiceSource,
        configuration: impl FnOnce(&str) -> meerkat_core::McpServerConfig,
    ) -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>, meerkat_core::McpServerConfig,
        Arc<crate::installed_host::acquisition::InstalledNativeContinuation>)
    {
        let (first, history, mut retained, continuation) = Self::for_durable_continuation().await;
        let config = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
            configuration(&first.scope.realm)));
        let session = first.session.clone();
        let closed = first.close_durable_owners().await;
        drop(continuation);
        drop(history);
        drop(first);
        assert!(closed.is_ok(), "initial persistent owners must settle: {closed:?}");
        let config = match config {
            Ok(config) => config,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        crate::installed_host::controller_first_input_fixture::configure_service_parent_before_restore(
            &mut retained.selection, &session, &config, source_kind);
        let (mut fixture, history, continuation) = Self::reopen_durable_continuation(retained).await;
        let retained_source = std::panic::AssertUnwindSafe(async {
            assert_eq!(fixture.session, session, "same actual durable session after restore");
            // The unsubmitted pre-close source is not relabelled after changing
            // a selected requester. Retain a fresh occurrence from this host.
            let mut event = private_event("application-service-original");
            event.route = Id::new("console").unwrap();
            event.content_sha256 = digest(PROMPT.as_bytes());
            let authentication = event.original_authentication.clone();
            let original = fixture.source.retain_event(event).await.unwrap();
            fixture.submission = PrivateOriginalSubmit { original, content: PROMPT.into(), transport_authentication: authentication };
        }).catch_unwind().await;
        if let Err(payload) = retained_source {
            let closed = fixture.close_durable_owners().await;
            drop(continuation);
            drop(history);
            drop(fixture);
            assert!(closed.is_ok(), "restored persistent owners must settle: {closed:?}");
            std::panic::resume_unwind(payload);
        }
        (fixture, history, config, continuation)
    }

    pub(crate) fn continuation_native(&self) -> &meerkat_mobkit::mob_handle_runtime::MobRuntime {
        self.native.mob_runtime()
    }

    pub(crate) async fn close_durable_owners(&self) -> Result<(), String> {
        use meerkat_toolkit::host_lifetime::control::{HostCloseReason, HostCloseState, PrivateHostLifetimeControl};
        let close = self._lifetime.begin_close_activation(HostCloseReason::Shutdown)
            .map_err(|error| error.to_string());
        let native = self.shutdown().await;
        let local = match close {
            Ok(close) => self._lifetime.join_close(&close.handle).await.map_err(|error| error.to_string()),
            Err(error) => Err(error),
        };
        native?;
        let local = local?;
        if local.state != HostCloseState::Settled { return Err(format!("host close: {local:?}")); }
        Ok(())
    }

    pub(super) async fn publish_fixture_controller(service: &meerkat::PersistentSessionService<FactoryAgentBuilder>,
        machine: &MeerkatMachine, session: &SessionId)
    {
        let lease = service.acquire_live_session_actor_turn_boundary_lease(session).await.unwrap();
        let pin = service.pin_controller_client_for_actor(&lease).await.unwrap();
        drop(lease);
        assert_eq!(pin.selection(), &selection());
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &machine.generated_auth_lease_handle(), pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-controller-only-fixture"),
            tokio::time::Instant::now() + WAIT,
        ).await.expect("selected fixture credential publishes within its setup deadline");
    }

    async fn construct_persistent(durable: bool, retained: Option<DurableFirstInput>,
        decorate: Option<AttachmentModelDecorator>, human_policy: bool, console_human: Option<meerkat_core::PrincipalRef>)
        -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>, Option<DurableFirstInput>,
            Option<Arc<crate::installed_host::acquisition::InstalledNativeContinuation>>)
    {
        Self::construct_persistent_observed(durable, retained, decorate, human_policy, console_human, None).await
    }

    async fn construct_persistent_observed(durable: bool, retained: Option<DurableFirstInput>,
        decorate: Option<AttachmentModelDecorator>, human_policy: bool,
        console_human: Option<meerkat_core::PrincipalRef>, observations: Option<NativeIngressObservations>)
        -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>, Option<DurableFirstInput>,
            Option<Arc<crate::installed_host::acquisition::InstalledNativeContinuation>>)
    {
        Self::construct_persistent_observed_source(durable, retained, decorate,
            human_policy, console_human, observations, None).await
    }

    async fn construct_persistent_observed_source(durable: bool, retained: Option<DurableFirstInput>,
        decorate: Option<AttachmentModelDecorator>, human_policy: bool,
        console_human: Option<meerkat_core::PrincipalRef>, observations: Option<NativeIngressObservations>,
        source_store: Option<crate::installed_host::controller_first_input_fixture::PrepareOriginalSource>,
    ) -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>, Option<DurableFirstInput>,
        Option<Arc<crate::installed_host::acquisition::InstalledNativeContinuation>>)
    {
        let mut opened = None;
        let mut owners = None;
        let built = std::panic::AssertUnwindSafe(tokio::time::timeout(
            WAIT,
            Box::pin(Self::build_persistent(&mut opened, &mut owners, durable, retained, decorate, human_policy, console_human, observations, source_store)),
        ))
        .catch_unwind()
        .await;
        match built {
            Ok(Ok(fixture)) => fixture,
            failed => {
                use meerkat_toolkit::host_lifetime::control::{HostCloseReason, HostCloseState, PrivateHostLifetimeControl};
                let close = owners.as_ref().map(|(_, lifetime)|
                    lifetime.begin_close_activation(HostCloseReason::Shutdown));
                let cleanup = match opened {
                    Some(runtime) => shutdown_fixture(&runtime).await,
                    None => Ok(()),
                };
                if let (Some((_, lifetime)), Some(Ok(close))) = (&owners, &close) {
                    assert_eq!(lifetime.join_close(&close.handle).await.unwrap().state, HostCloseState::Settled);
                }
                assert!(close.as_ref().is_none_or(|close| close.is_ok()), "host close entry refused: {close:?}");
                drop(owners);
                assert!(cleanup.is_ok(), "{cleanup:?}");
                match failed {
                    Err(payload) => std::panic::resume_unwind(payload),
                    _ => panic!("persistent first-input fixture construction timed out"),
                }
            }
        }
    }

    async fn build_persistent(
        opened: &mut Option<UnifiedRuntime>,
        owners: &mut Option<(Arc<tempfile::TempDir>, Arc<NativeHostLifetime>)>,
        durable: bool, retained: Option<DurableFirstInput>,
        decorate: Option<AttachmentModelDecorator>, human_policy: bool, console_human: Option<meerkat_core::PrincipalRef>,
        observations: Option<NativeIngressObservations>,
        source_store: Option<crate::installed_host::controller_first_input_fixture::PrepareOriginalSource>,
    ) -> (Self, Arc<dyn meerkat_runtime::RuntimeStore>, Option<DurableFirstInput>,
            Option<Arc<crate::installed_host::acquisition::InstalledNativeContinuation>>) {
        assert!(!human_policy || (!durable && retained.is_none()), "human policy fixture is selected only for this fresh public attachment");
        let facts =
            meerkat_core::ControllerModelFacts::new(selection(), ENDPOINT.into(), MODEL.into());
        use crate::installed_host::controller_first_input_fixture as installed;
        let console_owned_identity = console_human.is_some();
        assert!(source_store.is_none() || durable, "source decorator only selects an actual durable pair");
        let mut host = match &retained {
            Some(retained) => match source_store {
                Some(source_store) => installed::reopen_for_continuation_with_source(
                    retained.directory.clone(), retained.selection.clone(), source_store).await,
                None => installed::reopen_for_continuation(retained.directory.clone(), retained.selection.clone()).await,
            },
            None if durable => match source_store {
                Some(source_store) => installed::open_with_configured_setup_probe_source(&facts, source_store).await,
                None => installed::open_with_configured_setup_probe(&facts).await,
            },
            None if console_owned_identity => installed::open_for_console_service_batch(&facts, console_human.unwrap()).await,
            None if human_policy => installed::open_for_service_input_with_human_policy(&facts).await,
            None => installed::open_for_service_input(&facts).await,
        };
        if let Some(observations) = observations {
            let actual = host.configuration.ingress.clone();
            host.configuration.ingress = Arc::new(move |runtime, input, ingress, association| {
                let result = actual(runtime, input, ingress, association);
                if result.is_ok() && matches!(ingress.replay_lookup(),
                    Some(meerkat_runtime::input_authority::NativeReplayLookup::FirstAuthentication)) {
                    observations.lock().unwrap().push((association.clone(), ingress.clone()));
                }
                result
            });
        }
        *owners = Some((host.directory.clone(), host.lifetime.clone()));
        let continuation = durable.then(||
            crate::installed_host::acquisition::InstalledNativeContinuation::for_service_continuation_test(
                host.store.clone(), host.source_scope.clone(), host.originals.clone()));
        let provider = Arc::new(ScriptedProvider::default());
        let mut builder =
            FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
        let probe = Some(Arc::new(if human_policy {
            crate::installed_host::acquisition::ContextProbe::with_exact_catalog()
        } else {
            crate::installed_host::acquisition::ContextProbe::default()
        }));
        let client: Arc<dyn LlmClient> = match &probe {
            Some(_) => Arc::new(crate::installed_host::acquisition::ProbeProvider::new(
                provider.clone(),
            )),
            None => provider.clone(),
        };
        let client = match decorate {
            Some(decorate) => decorate(client),
            None => client,
        };
        builder.default_llm_client = Some(client);
        let mob_tools = builder.default_mob_tools.clone();
        *builder.default_agent_llm_client_decorator.write().unwrap() =
            Some(fixture_controller_decorator());
        // Select the governed persistent machine before publishing any adapter.
        // The facade wires the SAME stores into the factory and concrete service.
        let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
            Arc::new(
                meerkat::SqliteSessionStore::open(
                    host.directory.path().join("native-sessions.sqlite3"),
                )
                .unwrap(),
            ),
            if durable {
                Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(
                    host.directory.path().join("native-sessions.sqlite3")).unwrap())
                    as Arc<dyn meerkat_runtime::RuntimeStore>
            } else { Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()) },
            Arc::new(meerkat::MemoryBlobStore::new()),
            host.configuration,
        )
        .unwrap();
        // Observe the SAME canonical runtime store that the facade installs.
        // Persistent terminal archival removes inputs from the live driver
        // ledger, so a spine count is not this fixture's admission history.
        let runtime_store = persistence.runtime_store();
        let jobs = persistence.job_store();
        let (service, machine) =
            meerkat::surface::build_runtime_backed_service(builder, 4, persistence)
                .expect("compose persistent first-input runtime");
        let service = Arc::new(service);
        let mut definition = definition();
        definition.id = MobId::from("factory");
        let ProfileBinding::Inline(profile) = definition
            .profiles
            .get_mut(&ProfileName::from("worker"))
            .unwrap()
        else {
            panic!("inline fixture")
        };
        profile.tools = ToolConfig {
            comms: true,
            ..Default::default()
        };
        if let Some(retained) = &retained { definition = retained.definition.clone(); }
        let namespace = definition.id.to_string();
        let (mob_store, mob_provenance) = if durable {
            let (storage, provenance) = meerkat_mobkit::mob_composition_manifest::persistent_mob_storage(
                host.directory.path().join("native-mob.sqlite3")).unwrap();
            (storage, Some(provenance))
        } else { (MobStorage::in_memory(), None) };
        let mut spec = MobBootstrapSpec::new(definition, mob_store, service.clone())
            .with_session_runtime_adapter(machine.clone())
            .unwrap()
            .with_member_runtime_registration_hook(host.registration)
            .with_tool_consequence_policy_registry(host.tool_policy_registry);
        spec = match mob_provenance {
            Some(provenance) => {
                // This fixture creates no agent child mobs and reopens root/session
                // owners in-process. Their durable stores and provenance stay intact.
                spec.with_mob_storage_provenance(provenance)
                    .with_child_mob_storage(meerkat_mobkit::ChildMobStorage::ProcessBound)
            }
            None => spec.with_declared_ephemeral_mob_storage(),
        };
        if let Some(continuation) = &continuation {
            spec = spec.with_runtime_delivery(meerkat_mobkit::mob_handle_runtime::MobRuntimeDelivery::new(
                runtime_store.clone(), jobs))
                .with_agent_mob_tools(mob_tools).unwrap()
                .with_host_job_source(meerkat_mob_mcp::HostJobSource::new(
                    [continuation.namespace().to_owned()], continuation.clone()).unwrap()).unwrap();
        }
        if let Some(probe) = &probe {
            let probe = probe.clone();
            spec = spec.with_default_external_tools_provider(Arc::new(move || {
                Some(probe.clone() as Arc<dyn meerkat_core::AgentToolDispatcher>)
            }));
        }
        *opened = Some(bootstrap_fixture(spec, namespace).await);
        let native = opened.as_mut().unwrap();
        if let Some(continuation) = &continuation {
            continuation.bind_native(native.mob_runtime()).unwrap();
        }
        let handle = native.mob_handle();
        let live_session = if durable || console_owned_identity {
            None
        } else {
            let member = MemberIdentity::from(MEMBER);
            handle.spawn_spec(SpawnMemberSpec::new("worker", member.clone())).await.unwrap();
            let session = handle.get_member(&member).await.unwrap().unwrap()
                .bridge_session_id().cloned().unwrap();
            Self::publish_fixture_controller(&service, &machine, &session).await;
            Some(session)
        };
        let lease_provider = Arc::new(LocalLeaseProvider::new());
        let runtime_instance = "toolkit-first-input";
        let bridge = if durable || console_owned_identity {
            MobSessionBridge::with_session_service(
                handle.clone(), native.mob_runtime().session_service().cloned()
                    .expect("persistent runtime retains its actual session service"),
            )
        } else {
            MobSessionBridge::new(handle.observer())
        }.with_controller_pin_source(service.clone());
        let bridge: Arc<dyn meerkat_mobkit::identity_first::SessionBridge> = Arc::new(bridge);
        let runtime = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(if durable {
                    LocalContinuityStore::open(host.directory.path().join("native-continuity.sqlite3")).unwrap()
                } else { LocalContinuityStore::in_memory().unwrap() }),
                lease_provider: lease_provider.clone(),
                runtime_instance_id: runtime_instance.into(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(bridge),
                default_timeout: None,
            })
            .with_runtime_services(
                meerkat_mobkit::identity_first::AgentRuntimeServices::new(handle.clone()),
            ),
        );
        let identity = AgentIdentity::parse(MEMBER).unwrap();
        let mut member_spec = DurableAgentSpec {
            identity: identity.clone(),
            profile: ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: vec![],
            initial_message: None,
            runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        };
        if let Some(retained) = &retained { member_spec = retained.member.clone(); }
        if let Some(session) = &live_session {
        // Core already owns this spawned member and session; registration adds its roster spec.
        runtime.register(member_spec.clone()).await;
        let status = runtime.status(&identity).await.unwrap();
        assert_eq!(status.state, IdentityLifecycleState::Active);
        assert_eq!(status.session_id, Some(session.clone()));
        assert!(status.agent_runtime_id.is_some(), "spawned Core member has a runtime identity");
        assert_eq!(runtime.core_session_owner(session).await.unwrap(), Some(identity.clone()));
        }
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            runtime,
            Arc::new(MutableRosterProvider::new(vec![member_spec.clone()])),
            None,
            None,
            Some(handle.definition().clone()),
        ));
        let session = if durable || console_owned_identity {
            // Let the native identity owner create and restore its own member
            // and continuity from the first boot; a raw alias is not a durable identity.
            let restored = native.install_and_bootstrap_identity_first_context(
                context, std::slice::from_ref(&member_spec)).await.unwrap();
            let outcome = restored.outcomes.get(&identity).expect("durable identity outcome");
            if retained.is_some() {
                assert!(matches!(outcome, meerkat_mobkit::identity_first::RestoreOutcome::Resumed { .. }),
                    "the durable identity must resume: {outcome:?}");
            } else {
                assert!(matches!(outcome, meerkat_mobkit::identity_first::RestoreOutcome::Created { .. }),
                    "the durable identity must be created: {outcome:?}");
            }
            let session = native.identity_runtime().unwrap().status(&identity).await.unwrap()
                .session_id.expect("durable identity has its actual session");
            if let Some(retained) = &retained {
                assert_eq!(session, retained.session, "same actual durable session after restore");
            }
            // Each activation publishes the current actor's actual pin. No
            // controller witness is retained across the joined close/reopen.
            Self::publish_fixture_controller(&service, &machine, &session).await;
            session
        } else {
            native.attach_identity_first_context(context);
            live_session.unwrap()
        };
        let runtime = native.identity_runtime().unwrap();
        let source = host.originals;
        source.bind_machine(machine.clone()).unwrap();
        source.bind_identity_runtime(&runtime).unwrap();
        if durable || console_owned_identity { source.bind_controller_pin_source(service.clone()).unwrap(); }
        let submission = if let Some(retained) = retained { retained.submission } else {
        let mut event = private_event("first-conversation");
        event.route = Id::new("console").unwrap();
        event.content_sha256 = digest(PROMPT.as_bytes());
        let authentication = event.original_authentication.clone();
        let original = source.retain_event(event).await.unwrap();
        PrivateOriginalSubmit { original, content: PROMPT.into(), transport_authentication: authentication }
        };
        let retained = durable.then(|| DurableFirstInput {
            directory: host.directory.clone(), selection: host.selection.clone(),
            definition: handle.definition().clone(), member: member_spec, session: session.clone(),
            submission: submission.clone(),
        });
        (
            Self {
                store: host.store,
                source_path: host.source_path,
                scope: host.source_scope,
                service,
                probe,
                _directory: host.directory,
                _lifetime: host.lifetime,
                source,
                machine,
                native: opened.take().unwrap(),
                runtime,
                identity,
                session,
                provider,
                submission,
            },
            runtime_store,
            retained,
            continuation,
        )
    }
}

#[path = "custody_recovery_snapshot_tests.rs"]
mod custody_recovery_snapshot_tests;
