//! Live doors composed from the builder: console voice and the external
//! `meta/live/*` channel over ONE shared live context.
//!
//! The gateway binaries build their own concrete session service and hand
//! it to the live wiring directly. A library embedder builds through
//! [`UnifiedRuntimeBuilder`](super::builder::UnifiedRuntimeBuilder), whose
//! session service is erased at once, so until this seam existed console
//! voice could only be composed by Meta's own binaries. The builder now
//! records a `LivePlan` and retains the typed inputs; the runtime composes
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

    /// An ordinary `meta/live/open` (no `execution_identity`, the WebSocket
    /// bootstrap) that names `member`, sent through the stdio gateway's
    /// delivery-aware dispatch as the SDKs' plain open sends it: the answer,
    /// how many channels the realtime lane opened, and whether the request
    /// ran under the member's identity authority. With `identity_first` an
    /// identity runtime holds the member; without it the member is whatever
    /// the built runtime's own spawn makes it.
    async fn stdio_ordinary_open(
        id: &str,
        member: &str,
        identity_first: bool,
    ) -> (Value, usize, bool) {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let built = build_embedder_runtime(id, true).await;
        let runtime = &built.runtime;
        runtime
            .compose_live()
            .await
            .expect("live composes")
            .expect("a live door was registered");
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
        let session = runtime
            .mob_handle()
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(member))
            .await
            .expect("the member's session");
        let (identity_ctx, under_identity_authority) = if identity_first {
            let context = crate::rpc::identity_first_context_holding_for_test(
                runtime, id, member, "agent", session,
            )
            .await
            .expect("an identity runtime that holds the member");
            (Some(context), true)
        } else {
            let held = match runtime.identity_runtime_ref() {
                Some(identity_runtime) => identity_runtime
                    .member_alias_lifecycle_target(member)
                    .await
                    .expect("the member's identity authority reads")
                    .is_some(),
                None => false,
            };
            (None, held)
        };
        let external = runtime.live_rpc_handler().expect("external handler");
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/live/open",
            "params": {"identity": member, "model": "gpt-realtime-2"},
        })
        .to_string();
        let mut delivery = crate::rpc::handle_unified_rpc_json_with_live_arc_delivery(
            runtime,
            &request,
            Duration::from_secs(30),
            None,
            identity_ctx.as_ref(),
            Some(&external),
        )
        .await;
        println!(
            "{id} (under identity authority: {under_identity_authority}): {}",
            delivery.response
        );
        let answer: Value = serde_json::from_str(&delivery.response).expect("a JSON-RPC answer");
        delivery
            .settle_delivery(true)
            .await
            .expect("the writer settles");
        let opens = built.realtime.open_count();
        runtime.mob_handle().stop().await.expect("stop");
        (answer, opens, under_identity_authority)
    }

    /// The SDKs' plain open for an identity-first member over the stdio
    /// gateway opens its channel: the ordinary open hands the gateway's
    /// writer no custody, so it needs none on the task the request runs on.
    #[tokio::test]
    async fn a_stdio_ordinary_live_open_for_an_identity_first_member_opens_its_channel() {
        let (answer, opens, under_identity_authority) =
            stdio_ordinary_open("builder-stdio-open-identity", "agent-b", true).await;
        assert!(
            under_identity_authority
                && answer["error"].is_null()
                && answer["result"]["channel_id"].is_string()
                && opens == 1,
            "the ordinary open must open one channel: {answer} opens={opens}"
        );
    }

    /// The same plain open for a member no identity runtime was given for.
    #[tokio::test]
    async fn a_stdio_ordinary_live_open_for_a_spawned_member_opens_its_channel() {
        let (answer, opens, _under_identity_authority) =
            stdio_ordinary_open("builder-stdio-open-spawned", "agent-a", false).await;
        assert!(
            answer["error"].is_null() && answer["result"]["channel_id"].is_string() && opens == 1,
            "the ordinary open must open one channel: {answer} opens={opens}"
        );
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

    // --- A member's disposal while its live channel is open -----------------
    //
    // A live channel is opened and closed on the member's session. When the
    // member is retired, its mob destroyed or the runtime stopped while the
    // channel is open, the runtime that drops the session closes the channel
    // through the host's live composition. The rows open a call on the
    // scripted realtime lane: no request goes to a provider.

    /// The member the rows call.
    const CALLER: &str = "agent-a";

    /// A runtime with one member, `agent-a`.
    async fn runtime_with_the_caller(id: &str) -> Built {
        let built = build_embedder_runtime(id, true).await;
        built
            .runtime
            .compose_live()
            .await
            .expect("live composes")
            .expect("a live door was registered");
        built
            .runtime
            .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                "agent".to_string(),
                CALLER.to_string(),
                None,
                None,
                None,
            ))
            .await
            .expect("member");
        built
    }

    /// One request on the stdio dispatch of `built`'s runtime, with its live
    /// door.
    async fn stdio(built: &Built, method: &str, params: Value) -> Value {
        let live = built.runtime.live_rpc_handler();
        let request =
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
        let answer = crate::rpc::handle_unified_rpc_json_with_live(
            &built.runtime,
            &request,
            Duration::from_mins(1),
            None,
            None,
            live.as_ref(),
        )
        .await;
        serde_json::from_str(&answer).expect("the dispatch answers JSON")
    }

    /// Whether `listed` names the member [`CALLER`].
    async fn caller_is_a_member(built: &Built) -> bool {
        built
            .runtime
            .mob_handle()
            .list_members()
            .await
            .iter()
            .any(|member| member.agent_identity.as_str() == CALLER)
    }

    /// A runtime whose member `agent-a` is in a live call: its channel is
    /// open through the external door.
    struct LiveCall {
        built: Built,
        session: meerkat_core::types::SessionId,
        channel: String,
    }

    impl LiveCall {
        async fn open(id: &str) -> Self {
            let built = runtime_with_the_caller(id).await;
            let session = built
                .runtime
                .mob_handle()
                .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(CALLER))
                .await
                .expect("member session");
            // The member's text model is not realtime-capable; the realtime
            // lane is selected per open.
            let opened = Self::door(
                &built,
                &session,
                "meta/live/open",
                json!({"identity": CALLER, "model": "gpt-realtime-2"}),
            )
            .await;
            assert!(opened.error.is_none(), "{opened:?}");
            let channel = opened
                .result
                .as_ref()
                .and_then(|result| result["channel_id"].as_str())
                .expect("the open answers a channel")
                .to_string();
            let call = Self {
                built,
                session,
                channel,
            };
            assert!(call.channel_is_open().await, "the fixture's call is open");
            assert!(!call.provider_closed(), "the provider's side is open");
            call
        }

        /// One request of the call's own client on the external live door.
        async fn door(
            built: &Built,
            session: &meerkat_core::types::SessionId,
            method: &str,
            params: Value,
        ) -> crate::rpc::JsonRpcResponse {
            built
                .runtime
                .live_rpc_handler()
                .expect("external handler")
                .dispatch(
                    crate::live_wiring::LiveSurfaceAuthority::host_trusted_stdio(),
                    Some(session.clone()),
                    Some(CALLER.to_string()),
                    method.to_string(),
                    params,
                    json!(method),
                )
                .await
        }

        /// Whether the member's session still owns an open live channel, by
        /// the session's own machine state.
        async fn channel_is_open(&self) -> bool {
            let machine = Arc::clone(
                &self
                    .built
                    .runtime
                    .live_plan
                    .as_ref()
                    .expect("the launch's live plan")
                    .inputs
                    .machine,
            );
            machine.session_live_channel_active(&self.session).await
        }

        /// Whether the provider's side of the call was closed.
        fn provider_closed(&self) -> bool {
            use meerkat_core::live_adapter::LiveAdapter as _;
            let adapters = self.built.realtime.adapters();
            assert_eq!(adapters.len(), 1, "one provider session was opened");
            adapters[0].status() == meerkat_core::live_adapter::LiveAdapterStatus::Closed
        }

        /// What the call's client is answered when it asks for its channel.
        async fn caller_asks_for_its_channel(&self) -> crate::rpc::JsonRpcResponse {
            tokio::time::timeout(
                Duration::from_secs(10),
                Self::door(
                    &self.built,
                    &self.session,
                    "meta/live/status",
                    json!({"identity": CALLER, "channel_id": self.channel}),
                ),
            )
            .await
            .expect("the caller's request is answered, not left waiting")
        }
    }

    /// Whether `answer` says the channel is still open to its caller.
    fn says_the_channel_is_open(answer: &crate::rpc::JsonRpcResponse) -> bool {
        answer
            .result
            .as_ref()
            .and_then(|result| result["status"]["status"].as_str())
            .is_some_and(|status| matches!(status, "opening" | "ready" | "active"))
    }

    /// A member retired while its live channel is open is retired, and its
    /// channel is closed on both sides: the member's session owns no channel
    /// and the provider's side is closed.
    #[tokio::test]
    async fn a_member_retired_during_a_live_call_is_retired_and_its_channel_closed() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let call = LiveCall::open("live-disposal-retire").await;

        let retired = stdio(
            &call.built,
            "meta/retire_member",
            json!({"member_id": CALLER}),
        )
        .await;
        assert!(
            retired.get("error").is_none_or(Value::is_null),
            "the retire of a member in a live call is answered: {retired}"
        );
        assert!(
            !caller_is_a_member(&call.built).await,
            "the member is retired"
        );
        assert!(
            !call.channel_is_open().await,
            "the retired member's session owns no live channel"
        );
        assert!(
            call.provider_closed(),
            "the provider's side of the call was closed"
        );
        call.built.runtime.shutdown().await;
    }

    /// The client of a live call whose member is retired is told that its
    /// channel is closed: its next request for the channel is answered at
    /// once, and not as an open channel.
    #[tokio::test]
    async fn a_live_caller_is_told_a_typed_close_when_its_member_is_retired() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let call = LiveCall::open("live-disposal-caller").await;
        let before = call.caller_asks_for_its_channel().await;
        assert!(
            says_the_channel_is_open(&before),
            "the call is open to its caller before the retire: {before:?}"
        );

        let retired = stdio(
            &call.built,
            "meta/retire_member",
            json!({"member_id": CALLER}),
        )
        .await;
        let told = call.caller_asks_for_its_channel().await;
        assert!(
            !says_the_channel_is_open(&told),
            "after the retire ({retired}) the caller is told its channel is closed: {told:?}"
        );
        assert!(
            call.provider_closed(),
            "the provider's side of the call was closed"
        );
        call.built.runtime.shutdown().await;
    }

    /// A mob destroyed while a member's live channel is open is destroyed,
    /// and the channel is closed on both sides.
    #[tokio::test]
    async fn a_mob_destroyed_during_a_members_live_call_closes_the_channel() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let call = LiveCall::open("live-disposal-destroy").await;

        let destroyed = call.built.runtime.mob_handle().destroy().await;
        assert!(
            destroyed.is_ok(),
            "a mob with a member in a live call is destroyed: {:?}",
            destroyed.as_ref().err()
        );
        assert!(
            !call.channel_is_open().await,
            "no session of the destroyed mob owns a live channel"
        );
        assert!(
            call.provider_closed(),
            "the provider's side of the call was closed"
        );
    }

    /// The runtime's own stop while a member's live channel is open closes
    /// the channel and reports a clean teardown of the mob.
    #[tokio::test]
    async fn a_gateway_stop_during_a_live_call_closes_the_channel_and_reports_a_clean_teardown() {
        use crate::unified_runtime::types::MobTerminalShutdownOutcome;

        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let call = LiveCall::open("live-disposal-stop").await;

        let report = call.built.runtime.shutdown().await;
        let clean = matches!(
            &report.mob_terminal_shutdown,
            MobTerminalShutdownOutcome::Completed(mob) if mob.is_clean()
        );
        assert!(
            clean,
            "the mob's terminal teardown is clean with a call open: {:?}",
            report.mob_terminal_shutdown
        );
        assert!(
            call.provider_closed(),
            "the provider's side of the call was closed"
        );
    }

    /// The host's one voice path is free once a disposal closed the channel
    /// of the member that held it.
    #[tokio::test]
    async fn the_voice_path_is_free_after_a_disposal_closed_its_holder() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let call = LiveCall::open("live-disposal-voice-path").await;
        let arbiter = Arc::clone(
            &call
                .built
                .runtime
                .live_context()
                .expect("the live context")
                .arbiter,
        );
        assert!(
            arbiter.holder().await.is_some(),
            "the call holds the voice path"
        );

        let retired = stdio(
            &call.built,
            "meta/retire_member",
            json!({"member_id": CALLER}),
        )
        .await;
        assert_eq!(
            arbiter.holder().await,
            None,
            "after the retire ({retired}) nobody holds the voice path"
        );
        call.built.runtime.shutdown().await;
    }

    /// Guard: a member that is in no live call is retired as before.
    #[tokio::test]
    async fn a_retire_with_no_open_channel_is_unchanged() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let built = runtime_with_the_caller("live-disposal-no-call").await;
        assert!(caller_is_a_member(&built).await);

        let retired = stdio(&built, "meta/retire_member", json!({"member_id": CALLER})).await;
        assert!(
            retired.get("error").is_none_or(Value::is_null),
            "the retire is answered: {retired}"
        );
        assert!(!caller_is_a_member(&built).await, "the member is retired");
        assert!(
            built.realtime.adapters().is_empty(),
            "no provider session was opened or closed"
        );
        built.runtime.shutdown().await;
    }

    /// Guard: the cleanup the live composition installs on the runtime
    /// machine serves a close of the named channel and nothing else. The
    /// five other verbs of the member live plane (an open, an open under an
    /// execution identity, a status read, a control verb and the explicit
    /// close) are answered as a host with no member live plane answers
    /// them: no live transport. The call stays open and no second provider
    /// session is opened.
    ///
    /// The close itself is not asked here: the disposal rows above ask it.
    /// The runtime's supervisor bridge calls that same close for a placed
    /// member; a Meta host takes no placed members.
    #[tokio::test]
    async fn the_disposal_cleanup_serves_no_verb_but_a_close_of_the_named_channel() {
        use meerkat_contracts::{
            BridgeLiveControlVerb, WireLiveExecutionIdentityOverrideV1,
            WireLiveExecutionIdentityVersion,
        };
        use meerkat_runtime::member_live::MemberLiveError;

        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let call = LiveCall::open("live-disposal-mob-plane").await;
        let inputs = &call
            .built
            .runtime
            .live_plan
            .as_ref()
            .expect("the launch's live plan")
            .inputs;
        let host = crate::live_wiring::live_disposal_cleanup_host(
            &call.built.runtime.live_context().expect("the live context"),
            &inputs.service,
            &inputs.machine,
        );
        let unavailable = Err(MemberLiveError::TransportUnavailable);

        assert!(
            matches!(
                host.open(&call.session, None, None).await,
                Err(MemberLiveError::TransportUnavailable)
            ),
            "an open through the mob plane is not served"
        );
        let identity = WireLiveExecutionIdentityOverrideV1 {
            version: WireLiveExecutionIdentityVersion::V1,
            profile_id: "gpt-live-1".to_string(),
        };
        assert!(
            matches!(
                host.open_with_execution_identity(&call.session, &identity, None, None)
                    .await,
                Err(MemberLiveError::TransportUnavailable)
            ),
            "an open under an execution identity through the mob plane is not served"
        );
        assert_eq!(
            host.status(&call.session, Some(call.channel.clone())).await,
            unavailable,
            "a status read through the mob plane is not served"
        );
        assert_eq!(
            host.status(&call.session, None).await,
            unavailable,
            "a status read of the active channel through the mob plane is not served"
        );
        for verb in [
            BridgeLiveControlVerb::CommitInput,
            BridgeLiveControlVerb::Interrupt,
            BridgeLiveControlVerb::Refresh,
        ] {
            assert!(
                matches!(
                    host.control(&call.session, &call.channel, verb).await,
                    Err(MemberLiveError::TransportUnavailable)
                ),
                "a control verb through the mob plane is not served"
            );
        }
        assert!(
            matches!(
                host.close_explicit(&call.session, &call.channel).await,
                Err(MemberLiveError::TransportUnavailable)
            ),
            "the explicit close is not served"
        );

        assert!(
            call.channel_is_open().await,
            "the call is still open after the refused verbs"
        );
        assert!(!call.provider_closed(), "the provider's side is still open");
        call.built.runtime.shutdown().await;
    }

    /// Pin: Meta composes no mob host actor, so no mob on another host
    /// places a member on a Meta host. The disposal cleanup's close is the
    /// one the runtime's supervisor bridge calls for a placed member's
    /// channel, and the cleanup's texts say that on a Meta host the disposal
    /// is that close's only caller. The row reads every production source
    /// of the crate, comment lines left out, and fails when one names the
    /// mob host actor (`MobHostActor`, the `host_actor` module) or its bind
    /// ceremony (`serve_host_bind`, `serve_host_rebind`).
    ///
    /// When it fails: a Meta host that takes placed members lets the
    /// placing mob's supervisor close a member's call through the cleanup.
    /// Decide who may ask that close before the host actor is composed, and
    /// change the cleanup's texts with it.
    #[test]
    fn meta_takes_no_placed_members_so_the_cleanups_close_has_one_caller() {
        // Spelled in two halves, so that this file does not name them.
        let names = [
            concat!("MobHost", "Actor"),
            concat!("host", "_actor"),
            concat!("serve_host", "_bind"),
            concat!("serve_host", "_rebind"),
        ];
        let sources = crate::unified_runtime::default_mob_methods_tests::crate_production_sources();
        assert!(
            sources.iter().any(|(path, _)| path == "live_wiring.rs"),
            "the row reads the crate's production sources"
        );
        for (path, source) in &sources {
            for (number, line) in source.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                for name in names {
                    assert!(
                        !line.contains(name),
                        "{path}:{} names `{name}`: a Meta host that takes placed members \
                         lets the placing mob's supervisor close a member's live call \
                         through the disposal cleanup; decide that close's access first",
                        number + 1
                    );
                }
            }
        }
    }

    /// A mob whose member is in a live call is not idle: its idle close
    /// answers busy, with the member's session counted, and leaves the call
    /// open. Once the call's own client has closed it, the same close finds
    /// the mob idle.
    #[tokio::test]
    async fn an_open_live_call_keeps_its_mob_from_an_idle_close() {
        use meerkat_mob::CloseIfIdleOutcome;

        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let call = LiveCall::open("live-call-idle-close").await;

        let during = call
            .built
            .runtime
            .mob_handle()
            .close_if_idle()
            .await
            .expect("the mob answers its idle close");
        assert!(
            matches!(during, CloseIfIdleOutcome::Busy(busy) if busy.runs == 1),
            "a mob with a member in a live call is busy by that member's session: {during:?}"
        );
        assert!(call.channel_is_open().await, "the call is still open");
        assert!(!call.provider_closed(), "the provider's side is still open");

        let closed = LiveCall::door(
            &call.built,
            &call.session,
            "meta/live/close",
            json!({"identity": CALLER, "channel_id": call.channel}),
        )
        .await;
        assert!(closed.error.is_none(), "{closed:?}");
        assert!(!call.channel_is_open().await, "the client closed its call");
        let after = call
            .built
            .runtime
            .mob_handle()
            .close_if_idle()
            .await
            .expect("the mob answers its idle close");
        assert_eq!(
            after,
            CloseIfIdleOutcome::Closed,
            "with no call open the mob is idle"
        );
        call.built.runtime.shutdown().await;
    }
}
