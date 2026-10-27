//! Head-canonical resume regression across process restarts.
//!
//! Sessions live in Core's own session store, which shares `runtime.sqlite`
//! with Core's runtime store under the head-canonical profile; Meta keeps no
//! copy of its own. The harness boots the gateway-shaped composition
//! (`UnifiedRuntimeBuilder::continuity_from_state_dir` opens the identity
//! substrate the binaries use) and reads the durable session back from Core's
//! store after each boot, so a resume that served a stale or re-created
//! session fails here instead of passing.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, RosterContext, RosterError,
};
use meerkat_mob::{MobDefinition, ProfileName};
use tokio::time::sleep;

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy under meerkat 0.8.22. See the module docs.
#[path = "support/llm_usage.rs"]
mod llm_usage;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const MOB_TOML: &str = r#"
[mob]
id = "head-canonical-resume"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
"#;

const MEMBER: &str = "personal:alice";
const RUNTIME_INSTANCE: &str = "head-canonical-resume";

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("parse identity")
}

fn definition() -> MobDefinition {
    MobDefinition::from_toml(MOB_TOML).expect("parse head-canonical mob definition")
}

fn durable_spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: id(name),
        profile: ProfileName::from("personal"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

struct OneMemberRoster;

#[async_trait]
impl RosterProvider for OneMemberRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![durable_spec(MEMBER)])
    }
}

/// Records the serialized LLM request each turn and answers "ok" so turns
/// complete without a real provider.
#[derive(Clone, Default)]
struct CaptureClient {
    requests: Arc<std::sync::Mutex<Vec<String>>>,
}

impl CaptureClient {
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn last(&self) -> Option<String> {
        self.requests.lock().unwrap().last().cloned()
    }
}

impl meerkat_client::LlmClient for CaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_string(request).unwrap_or_default());
        // meerkat 0.8.22 rejects a turn whose stream carried no normalized
        // provider accounting, so the terminal `Done` never travels alone.
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }
    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }
    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

// ---------------------------------------------------------------------------
// Gateway-shaped harness
// ---------------------------------------------------------------------------

/// Build the gateway-shaped runtime, as `rpc_gateway` composes it: persistent
/// state, the state directory's identity substrate (opened through
/// `gateway_wiring::open_identity_substrate`) and a roster provider. Sessions
/// live in Core's session store on `runtime.sqlite`.
async fn boot(state: &Path, capture: CaptureClient) -> meerkat_meta::UnifiedRuntime {
    Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition())
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(OneMemberRoster))
            .identity_runtime_instance_id(RUNTIME_INSTANCE)
            .comms(true)
            .default_llm_client(Arc::new(capture))
            .build(),
    )
    .await
    .expect("build the gateway-shaped UnifiedRuntime")
}

// ---------------------------------------------------------------------------
// Precondition probe
// ---------------------------------------------------------------------------

/// The message count of `session_id` in Core's session store, which shares
/// `runtime.sqlite` with the runtime store; `None` when Core holds no such
/// session.
async fn core_session_message_count(state: &Path, session_id: &str) -> Option<i64> {
    use meerkat::SessionStore as _;
    let store = meerkat_store::SqliteSessionStore::open(state.join("runtime.sqlite"))
        .expect("open Core's session store");
    let session_id = meerkat_core::types::SessionId::parse(session_id).expect("a session id");
    store
        .load(&session_id)
        .await
        .expect("read Core's session store")
        .map(|session| i64::try_from(session.messages().len()).expect("a message count"))
}

async fn wait_for_turn(capture: &CaptureClient, want: usize, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while capture.count() < want {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what} to reach the LLM"
        );
        sleep(Duration::from_millis(100)).await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The regression this harness was built for (advisory:
/// "head-canonical sessions become unresumable after reboot"): with the
/// head-canonical document as the ONLY session authority, four
/// boot/turn/reboot cycles must keep the identity ACTIVE, keep resuming the
/// same session, keep replaying the transcript, and keep GROWING the durable
/// head.
///
/// The field shape degrades across restarts rather than failing on the first
/// one (2 ACTIVE -> 1 ACTIVE/1 BROKEN -> 2 BROKEN), because each turn advances
/// the durable head while the stale document the reader is handed stays
/// pinned. One restart is therefore not enough coverage; this walks four.
#[tokio::test(flavor = "multi_thread")]
async fn core_session_survives_repeated_reboots() {
    const TOKEN: &str = "MARKER-HEAD-CANONICAL-7-ZULU";
    const CYCLES: usize = 4;
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");

    let mut session_id: Option<String> = None;
    let mut message_count: i64 = 0;

    for cycle in 1..=CYCLES {
        let capture = CaptureClient::default();
        let runtime = boot(&state, capture.clone()).await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime")
            .clone();

        // Form 2 of the advisory surfaces here: a resume that cannot read its
        // own durable head leaves the identity Broken instead of Active.
        let status = identity_runtime
            .status(&id(MEMBER))
            .await
            .unwrap_or_else(|e| panic!("boot {cycle}: identity status: {e}"));
        assert_eq!(
            status.state,
            meerkat_meta::identity_first::IdentityLifecycleState::Active,
            "boot {cycle}: identity must be Active after restore, got {:?} (wake refusal: {:?})",
            status.state,
            status.wake_refusal,
        );
        if let (Some(previous), Some(current)) = (session_id.as_ref(), status.session_id.as_ref()) {
            assert_eq!(
                &current.to_string(),
                previous,
                "boot {cycle}: restore must reuse the durable session, not create a new one"
            );
        }

        let prompt = if cycle == 1 {
            format!("Please note this token: {TOKEN}")
        } else {
            format!("Reboot {cycle}: which token did I give you?")
        };
        identity_runtime
            .send(&id(MEMBER), &meerkat_core::ContentInput::Text(prompt))
            .await
            .unwrap_or_else(|e| panic!("boot {cycle}: send turn: {e}"));
        wait_for_turn(&capture, 1, &format!("boot {cycle}'s turn")).await;

        if cycle > 1 {
            // Form 1 of the advisory surfaces here: the reader is handed a
            // stale document, so the replayed transcript is short or empty.
            let replayed = capture
                .last()
                .unwrap_or_else(|| panic!("boot {cycle}: no request captured"));
            assert!(
                replayed.contains(TOKEN),
                "boot {cycle}: the post-restart request must replay the head-canonical \
                 transcript (token {TOKEN}); resume served a stale or empty document"
            );
        }

        sleep(Duration::from_millis(500)).await;
        runtime.shutdown().await;

        let head_session = status
            .session_id
            .as_ref()
            .unwrap_or_else(|| panic!("boot {cycle}: an Active identity is bound to a session"))
            .to_string();
        let head_messages = core_session_message_count(&state, &head_session)
            .await
            .unwrap_or_else(|| {
                panic!("boot {cycle}: Core's session store holds no session {head_session}")
            });
        match session_id.as_ref() {
            None => session_id = Some(head_session),
            Some(previous) => assert_eq!(
                &head_session, previous,
                "boot {cycle}: the durable session must stay the same"
            ),
        }
        assert!(
            head_messages > message_count,
            "boot {cycle}: the turn must EXTEND the durable session (message_count \
             {message_count} -> {head_messages}); a rejected save or a re-created session \
             would not grow it"
        );
        message_count = head_messages;
    }

    assert!(
        message_count >= (CYCLES * 2) as i64,
        "after {CYCLES} boot/turn cycles the durable head should hold at least one \
         user+assistant pair per cycle, got {message_count}"
    );
}
