//! Live doors composed from the builder: console voice and the external
//! `meta/live/*` channel over ONE shared live context.
//!
//! The gateway binaries build their own concrete session service and hand
//! it to the live wiring directly. A library embedder builds through
//! [`UnifiedRuntimeBuilder`](super::builder::UnifiedRuntimeBuilder), whose
//! session service is erased at once, so until this seam existed console
//! voice could only be composed by Meta's own binaries. The builder now
//! records a [`LivePlan`] and retains the typed inputs; the runtime composes
//! the doors once, after it is shared, exactly the way `rpc_gateway` does:
//! one `attach_live`, the console controller and the external handler over
//! the same context, and the shared voice-path arbiter between them.

use std::sync::Arc;

use crate::live_wiring::{GatewayLiveContext, LiveRpcHandler};
use crate::mob_handle_runtime::LiveComposeInputs;

use super::UnifiedRuntime;

/// External live channel options for the builder path. Mirrors the gateway's
/// `runtime_options.live` object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveOptions {
    /// Public WebSocket base URL advertised to live clients in open results
    /// (for example `wss://gateway.example.com`). Required: the gateway
    /// binaries substitute their bound address, but a library embedder serves
    /// the router on its own listener, so only it knows the public origin.
    /// `build()` fails closed when this is empty.
    pub public_base_url: String,
    /// Upper bound for the canonical dialogue seed replayed into a fresh
    /// provider session. `None` keeps the wiring default.
    pub seed_max_chars: Option<usize>,
}

impl LiveOptions {
    /// Options advertising `public_base_url` with the default seed bound.
    pub fn new(public_base_url: impl Into<String>) -> Self {
        Self {
            public_base_url: public_base_url.into(),
            seed_max_chars: None,
        }
    }
}

/// What the builder asked for, retained on the runtime until composition.
#[derive(Clone)]
pub(crate) struct LivePlan {
    pub(crate) inputs: LiveComposeInputs,
    pub(crate) live: Option<LiveOptions>,
    #[cfg(feature = "openai-live")]
    pub(crate) console_voice: Option<crate::public_live_config::PublicLiveRegistration>,
    /// Test-only: point the console summarizer at a fixture provider instead
    /// of the realm's configured base URL.
    #[cfg(all(test, feature = "openai-live"))]
    pub(crate) summary_override: Option<(
        meerkat::session_runtime::live_summary::LiveContextSummaryPolicy,
        String,
    )>,
    /// Test-only: replace the realtime session factory on the composed
    /// context so the external door never negotiates with a real provider.
    #[cfg(test)]
    pub(crate) session_factory_override:
        Option<Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>>,
}

/// The composed doors. Cloning shares the same context and handlers.
#[derive(Clone)]
pub struct LiveComposition {
    context: Arc<GatewayLiveContext>,
    external: Option<LiveRpcHandler>,
    #[cfg(feature = "openai-live")]
    console_voice: Option<crate::console_voice::ConsoleVoiceController>,
}

impl LiveComposition {
    /// The one shared live context (adapter host, WebSocket state, voice
    /// path arbiter).
    pub fn context(&self) -> &Arc<GatewayLiveContext> {
        &self.context
    }

    /// The external `meta/live/*` handler, present when the builder
    /// registered `live(...)`.
    pub fn external_handler(&self) -> Option<&LiveRpcHandler> {
        self.external.as_ref()
    }

    /// The console voice controller, present when the builder registered
    /// `console_voice(...)`.
    #[cfg(feature = "openai-live")]
    pub fn console_voice(&self) -> Option<&crate::console_voice::ConsoleVoiceController> {
        self.console_voice.as_ref()
    }
}

/// Why the live doors could not be composed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveComposeError {
    /// The builder registered a live door without persistent sessions.
    /// Live channels need the durable session owner; call
    /// `persistent_state(...)` (or compose a `storage_provider` realm) on the
    /// builder.
    LiveRequiresPersistentSessions,
    /// The console voice controller refused the registration.
    ConsoleVoice(String),
    /// `live(...)` was registered with an empty `public_base_url`. The
    /// builder path cannot learn the bound address, so an empty value would
    /// advertise a loopback placeholder to live clients.
    LiveRequiresPublicBaseUrl,
    /// Console voice is composed but the decision state leaves the console
    /// open (`console.require_app_auth = false`). The controller serves only
    /// authenticated principals, so the router refuses to build rather than
    /// ship a console whose voice button can never work.
    ConsoleVoiceRequiresAppAuth,
}

impl std::fmt::Display for LiveComposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LiveRequiresPersistentSessions => write!(
                f,
                "console_voice and live require persistent sessions: configure \
                 UnifiedRuntimeBuilder::persistent_state (or a storage_provider realm) \
                 before registering a live door"
            ),
            Self::ConsoleVoice(error) => write!(f, "console_voice composition failed: {error}"),
            Self::LiveRequiresPublicBaseUrl => write!(
                f,
                "live requires a public_base_url: the builder path serves the router on the \
                 embedder's listener and cannot learn the public WebSocket origin itself"
            ),
            Self::ConsoleVoiceRequiresAppAuth => write!(
                f,
                "console_voice requires an authenticated console: build the router with a \
                 decision state whose console.require_app_auth is true (for example \
                 parse_console_auth_config with provider \"oidc\" or \"jwt\")"
            ),
        }
    }
}

impl std::error::Error for LiveComposeError {}

impl UnifiedRuntime {
    pub(crate) fn set_live_plan(&mut self, plan: Option<LivePlan>) {
        self.live_plan = plan;
    }

    /// Test-only: install fixture overrides on the plan before composition.
    #[cfg(all(test, feature = "openai-live"))]
    pub(crate) fn test_override_live_plan(
        &mut self,
        summary: Option<(
            meerkat::session_runtime::live_summary::LiveContextSummaryPolicy,
            String,
        )>,
        session_factory: Option<Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>>,
    ) {
        if let Some(plan) = self.live_plan.as_mut() {
            plan.summary_override = summary;
            plan.session_factory_override = session_factory;
        }
    }

    /// Compose the live doors the builder registered, once.
    ///
    /// Takes `&Arc<Self>` because the console voice controller binds the
    /// shared runtime (its mob handle and identity runtime). A successful
    /// composition is bound once and returned by every later call; errors
    /// are not cached, so a failing registration is retried (including a
    /// fresh `attach_live`) on the next call. Returns `Ok(None)` when the
    /// builder registered no live door.
    pub async fn compose_live(
        self: &Arc<Self>,
    ) -> Result<Option<LiveComposition>, LiveComposeError> {
        let Some(plan) = self.live_plan.clone() else {
            return Ok(None);
        };
        let composition = self
            .live_composition
            .get_or_try_init(|| async { compose(self, plan) })
            .await?;
        Ok(Some(composition.clone()))
    }

    /// The live composition, if [`Self::compose_live`] has bound it.
    pub fn live_composition(&self) -> Option<&LiveComposition> {
        self.live_composition.get()
    }

    /// The shared live context, once composed.
    pub fn live_context(&self) -> Option<Arc<GatewayLiveContext>> {
        self.live_composition
            .get()
            .map(|composition| Arc::clone(&composition.context))
    }

    /// The external `meta/live/*` handler, once composed and registered.
    pub fn live_rpc_handler(&self) -> Option<LiveRpcHandler> {
        self.live_composition
            .get()
            .and_then(|composition| composition.external.clone())
    }

    /// The console voice controller, once composed and registered.
    #[cfg(feature = "openai-live")]
    pub fn console_voice_controller(&self) -> Option<crate::console_voice::ConsoleVoiceController> {
        self.live_composition
            .get()
            .and_then(|composition| composition.console_voice.clone())
    }
}

fn compose(
    runtime: &Arc<UnifiedRuntime>,
    plan: LivePlan,
) -> Result<LiveComposition, LiveComposeError> {
    #[cfg(not(feature = "openai-live"))]
    let _ = runtime;
    let LiveComposeInputs {
        service,
        machine,
        factory,
        config,
    } = plan.inputs;
    // Console voice alone negotiates its transport over WebRTC and never
    // advertises a WebSocket origin, so an empty base URL is correct there;
    // `live(...)` is validated non-empty at build.
    let ws_base_url = plan
        .live
        .as_ref()
        .map(|options| options.public_base_url.trim_end_matches('/').to_string())
        .unwrap_or_default();
    let seed_max_chars = plan
        .live
        .as_ref()
        .and_then(|options| options.seed_max_chars);
    #[allow(unused_mut)]
    let mut context = crate::live_wiring::attach_live(
        Arc::clone(&service),
        Arc::clone(&machine),
        &factory,
        config,
        ws_base_url,
        seed_max_chars,
    );
    #[cfg(test)]
    if let Some(factory) = plan.session_factory_override.clone() {
        context.session_factory = factory;
    }
    let context = Arc::new(context);
    #[cfg(feature = "openai-live")]
    let console_voice = match plan.console_voice {
        Some(registration) => {
            #[cfg(test)]
            let controller = crate::console_voice::ConsoleVoiceController::compose_live_host(
                runtime,
                Arc::clone(&context),
                Arc::clone(&service),
                Arc::clone(&machine),
                factory,
                registration,
                plan.summary_override,
            );
            #[cfg(not(test))]
            let controller = crate::console_voice::ConsoleVoiceController::with_shared_live_context(
                runtime,
                Arc::clone(&context),
                Arc::clone(&service),
                Arc::clone(&machine),
                factory,
                registration,
            );
            Some(controller.map_err(LiveComposeError::ConsoleVoice)?)
        }
        None => None,
    };
    let external = plan
        .live
        .as_ref()
        .map(|_| crate::live_wiring::live_rpc_handler(Arc::clone(&context), service, machine));
    Ok(LiveComposition {
        context,
        external,
        #[cfg(feature = "openai-live")]
        console_voice,
    })
}

#[cfg(test)]
#[cfg(feature = "openai-live")]
#[allow(clippy::expect_used)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::{LiveComposeError, LiveOptions};
    use crate::console_voice::live_host::tests::{
        ProviderFixture, SHARED_HOST_TEST_LOCK, Summary, SummaryScenario, config, rpc,
    };
    use crate::public_live_config::PublicLiveRegistration;
    use crate::unified_runtime::UnifiedRuntime;
    use crate::unified_runtime::types::UnifiedRuntimeBuilderError;

    fn definition(id: &str) -> meerkat_mob::MobDefinition {
        meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{id}"
[profiles.agent]
model = "gpt-5.5"
external_addressable = true
[profiles.agent.tools]
comms = true
"#
        ))
        .expect("definition")
    }

    fn registration() -> PublicLiveRegistration {
        PublicLiveRegistration::parse(&json!({
            "principal":"voice@example.com","realm":"voice",
            "auth_binding":{"realm":"voice","binding":"openai"},"voice":"marin"
        }))
        .expect("registration")
    }

    fn console_token() -> String {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &json!({
                "iss":"http://127.0.0.1/meta-gateway","aud":"persistent-gateway",
                "sub":"voice@example.com","email":"voice@example.com",
                "exp":chrono::Utc::now().timestamp()+300
            }),
            &jsonwebtoken::EncodingKey::from_secret(b"console-test-signing"),
        )
        .expect("token")
    }

    fn authenticated_decisions() -> crate::RuntimeDecisionState {
        crate::console_auth_config::parse_console_auth_config(&json!({
            "shared_secret":"console-test-signing", "email_allowlist":["voice@example.com"]
        }))
        .expect("auth")
    }

    struct Built {
        _directory: tempfile::TempDir,
        provider: ProviderFixture,
        runtime: Arc<UnifiedRuntime>,
        realtime: Arc<meerkat::test_fixtures::realtime::ScriptedRealtimeSessionFactory>,
    }

    /// The production shape: a library embedder builds through the builder with
    /// persistent state, registers console voice and the external live
    /// channel, and serves the router on its own listener.
    async fn build_embedder_runtime(id: &str, with_live: bool) -> Built {
        let provider = ProviderFixture::start().await;
        std::fs::create_dir_all(".rct").expect("test state parent");
        let directory = tempfile::Builder::new()
            .prefix("builder-voice-")
            .tempdir_in(".rct")
            .expect("test state");
        let client = Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        ));
        let mut builder = UnifiedRuntime::builder()
            .definition(definition(id))
            // Sessions persist in Core's pair on runtime.sqlite under the
            // state directory, which is what a live door needs.
            .persistent_state(directory.path())
            .default_llm_client(client)
            .meerkat_config(config())
            .console_voice(registration());
        if with_live {
            builder = builder.live(LiveOptions::new(format!("ws://127.0.0.1/{id}")));
        }
        let mut runtime = Box::pin(builder.build()).await.expect("runtime builds");
        // Point the console summarizer and the external realtime lane at
        // fixtures before the doors compose, exactly as the gateway-path test
        // does over its hand-built context.
        let release = Arc::new(tokio::sync::Notify::new());
        release.notify_one();
        let policy = meerkat::session_runtime::live_summary::LiveContextSummaryPolicy::new(
            Arc::new(Summary {
                calls: Arc::new(AtomicUsize::new(0)),
                release,
                cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                scenario: SummaryScenario::Success,
            }),
            4 * 1024 * 1024,
            4096,
            Duration::from_secs(30),
        )
        .expect("summary policy");
        let realtime =
            Arc::new(meerkat::test_fixtures::realtime::ScriptedRealtimeSessionFactory::new());
        runtime.test_override_live_plan(
            Some((policy, provider.url.clone())),
            Some(Arc::clone(&realtime)
                as Arc<
                    dyn meerkat_client::realtime_session::RealtimeSessionFactory,
                >),
        );
        Built {
            _directory: directory,
            provider,
            runtime: Arc::new(runtime),
            realtime,
        }
    }

    #[tokio::test]
    async fn console_voice_without_a_persistent_store_fails_the_build_closed() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let Err(error) = Box::pin(
            UnifiedRuntime::builder()
                .definition(definition("builder-voice-ephemeral"))
                .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
                .console_voice(registration())
                .build(),
        )
        .await
        else {
            unreachable!("ephemeral sessions cannot host a live door");
        };
        assert!(matches!(
            error,
            UnifiedRuntimeBuilderError::LiveCompose(
                LiveComposeError::LiveRequiresPersistentSessions
            )
        ));
        assert!(error.to_string().contains("persistent_state"));
    }

    #[tokio::test]
    async fn the_live_router_refuses_an_open_console_when_voice_is_composed() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let built = build_embedder_runtime("builder-voice-open-console", false).await;
        let mut open = authenticated_decisions();
        open.console.require_app_auth = false;
        let Err(refused) = built
            .runtime
            .build_reference_app_router_with_live(open)
            .await
        else {
            unreachable!("an open console cannot serve voice");
        };
        assert_eq!(refused, LiveComposeError::ConsoleVoiceRequiresAppAuth);
        // The plain router still builds; it simply carries no voice door.
        let _plain = built
            .runtime
            .build_reference_app_router(authenticated_decisions());
        built.runtime.mob_handle().stop().await.expect("stop");
    }

    #[tokio::test]
    async fn builder_console_voice_and_live_share_one_owner_through_the_built_router() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let built = build_embedder_runtime("builder-voice-shared", true).await;
        let runtime = &built.runtime;
        let realtime = Arc::clone(&built.realtime);
        let _provider = &built.provider;
        runtime
            .compose_live()
            .await
            .expect("live composes")
            .expect("a live door was registered");
        assert!(runtime.console_voice_controller().is_some());
        assert!(runtime.live_rpc_handler().is_some());
        assert!(runtime.live_context().is_some());
        let app = runtime
            .build_reference_app_router_with_live(authenticated_decisions())
            .await
            .expect("live router");
        let token = console_token();
        for member in ["agent-a", "agent-b"] {
            runtime
                .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                    "agent".to_string(),
                    member.to_string(),
                    None,
                    None,
                    None,
                ))
                .await
                .expect("member");
        }
        let ready = rpc(
            &app,
            &token,
            "meta/console/voice/readiness",
            json!({"identity":"agent-a"}),
        )
        .await;
        assert_eq!(ready["result"]["available"], json!(true), "{ready}");
        // Phase 1: the console engages the voice path through the built router.
        let opened = rpc(
            &app,
            &token,
            "meta/console/voice/open",
            json!({"identity":"agent-a","request_id":"voice-first"}),
        )
        .await;
        assert!(opened["error"].is_null(), "{opened}");
        let console_channel = opened["result"]["channel_id"]
            .as_str()
            .expect("console channel")
            .to_string();
        // Phase 2: the external door, reached through the builder accessor,
        // preempts the console call and names what it superseded.
        let session_b = runtime
            .mob_handle()
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("agent-b"))
            .await
            .expect("external member session");
        let external = runtime.live_rpc_handler().expect("external handler");
        let external_open = external
            .dispatch(
                crate::live_wiring::LiveSurfaceAuthority::host_trusted_stdio(),
                Some(session_b.clone()),
                Some("agent-b".to_string()),
                "meta/live/open".to_string(),
                json!({"identity":"agent-b", "model":"gpt-realtime-2"}),
                json!("external-first"),
            )
            .await;
        assert!(external_open.error.is_none(), "{external_open:?}");
        let result: Value = external_open.result.expect("external open result");
        assert_eq!(
            result["superseded"],
            json!({"owner":"console_voice","identity":"agent-a","channel_id":console_channel}),
        );
        assert_eq!(
            realtime.open_count(),
            1,
            "the external open used the fixture lane"
        );
        let replacement = rpc(
            &app,
            &token,
            "meta/console/voice/replacement",
            json!({"identity":"agent-a","request_id":"voice-first"}),
        )
        .await;
        assert_eq!(
            replacement["error"]["data"]["kind"], "voice_superseded",
            "{replacement}"
        );
        assert_eq!(
            replacement["error"]["data"]["reason"],
            "superseded_by_external_live"
        );
        let busy = rpc(
            &app,
            &token,
            "meta/console/voice/readiness",
            json!({"identity":"agent-a"}),
        )
        .await;
        assert_eq!(busy["result"]["available"], json!(false), "{busy}");
        assert_eq!(busy["result"]["reason"], "external_live_active");
        assert_eq!(busy["result"]["holder"]["identity"], "agent-b");
        runtime.mob_handle().stop().await.expect("stop");
    }

    #[tokio::test]
    async fn live_without_a_public_base_url_fails_the_build_closed() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let directory = tempfile::Builder::new()
            .prefix("builder-live-url-")
            .tempdir_in(".rct")
            .expect("test state");
        let Err(error) = Box::pin(
            UnifiedRuntime::builder()
                .definition(definition("builder-live-url"))
                .persistent_state(directory.path())
                .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
                .live(LiveOptions::new("   "))
                .build(),
        )
        .await
        else {
            unreachable!("an empty public_base_url cannot be advertised");
        };
        assert!(matches!(
            error,
            UnifiedRuntimeBuilderError::LiveCompose(LiveComposeError::LiveRequiresPublicBaseUrl)
        ));
    }
}
