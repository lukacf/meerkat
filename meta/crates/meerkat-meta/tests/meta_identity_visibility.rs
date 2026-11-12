#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

//! Who is shown an agent of a mob, and which labels a change of an agent
//! may write.
//!
//! Each row boots the gateway's runtime composition in process, on a
//! persistent state directory, with a roster the row gives and the rows'
//! identity policy. The rows that speak to the Console build its router
//! with sign-in on, under access rules the row gives, and sign in as one of
//! three persons: an access administrator and two others.

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::service::{CreateSessionRequest, SessionError};
use meerkat_core::types::StopReason;
use meerkat_meta::identity_creation::{
    AgentRecord, AgentRef, ChangeRequester, GetAgent, IdentityChangeCall, IdentityChangeSpec,
    IdentityCreationPolicy, IdentityFamily, IdentityFamilyOptions, IdentityOutcome, IdentityRead,
    IdentityReadAnswer, OperationId, Sha256Digest,
};
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, RosterContext, RosterError,
};
use meerkat_meta::{SessionHook, UnifiedRuntime, UnifiedRuntimeBuilder};
use meerkat_mob::{MobDefinition, ProfileName};
use serde_json::{Value, json};

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// The access administrator the Console rows sign in as.
const ROOT: &str = "root@example.org";
/// A person the rows' access rules allow.
const ANN: &str = "ann@example.org";
/// A person the rows' deny rules name.
const BOB: &str = "bob@example.org";
/// The issuer the Console's sign-in fixture trusts.
const ISSUER: &str = "https://trusted.meta.localhost";
/// Bounds a failure only; a passing row never waits for it.
const CALL_BOUND: Duration = Duration::from_secs(90);

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

fn fresh_mob_id() -> String {
    format!(
        "meta-identity-visibility-{}-{}",
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    )
}

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true

[profiles.reviewer]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.reviewer.tools]
comms = true
"#
    ))
    .expect("parse the definition")
}

fn roster_spec(name: &str, profile: &str, labels: &[(&str, &str)]) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(name).unwrap(),
        profile: ProfileName::from(profile),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: labels
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

/// The host's roster: the agents a row lists. It never lists an agent a row
/// creates.
struct Listed(Vec<DurableAgentSpec>);

#[async_trait]
impl RosterProvider for Listed {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(self.0.clone())
    }
}

fn controller() -> meerkat_core::ControllerModelSelection {
    meerkat_core::ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: "gpt-5.5".into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(
            serde_json::json!({"realm": "meta-identity-visibility", "account": "quiet"}),
        )
        .expect("nonsecret identity of the fixture transport"),
        "quiet-profile".into(),
        "quiet-transport".into(),
    )
}

/// The members' model: every request answers.
#[derive(Clone)]
struct QuietClient;

impl meerkat_client::LlmClient for QuietClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        Some(controller())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "noted".to_string(), meta: None });
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

/// A host build hook that fails every session it is asked to create while
/// it is set to: the spawn of a new agent's first message then fails, the
/// agent is owed its seat, and it stays so while the row runs.
#[derive(Default)]
struct FailSessions {
    failing: AtomicBool,
}

#[async_trait]
impl SessionHook for FailSessions {
    async fn before_create(&self, _req: &mut CreateSessionRequest) -> Result<(), SessionError> {
        if self.failing.load(Ordering::SeqCst) {
            return Err(SessionError::Unsupported(
                "the row starts no session".to_owned(),
            ));
        }
        Ok(())
    }
}

/// The rows' identity policy: every change of an agent of either profile is
/// allowed to the host and to a person.
fn policy() -> Arc<IdentityCreationPolicy> {
    let changes = json!([
        "create",
        "descriptive",
        "peers",
        "profile",
        "archive",
        "remove",
        "adopt"
    ]);
    let rule = |requester: Value| {
        json!({"change": changes, "requester": requester,
               "target": {"profile": ["worker", "reviewer"]}, "outcome": {"kind": "allow"}})
    };
    Arc::new(
        IdentityCreationPolicy::parse(&json!({
            "enabled": true,
            "peers": {"default": "allow"},
            "approval": {"rules": [rule(json!({"kind": "host"})), rule(json!({"kind": "human"}))]},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled"),
    )
}

/// One booted gateway runtime with the family attached.
struct Rig {
    runtime: Arc<UnifiedRuntime>,
    fail_sessions: Arc<FailSessions>,
}

/// Boot the gateway's runtime composition on `state` over the host's roster
/// `roster`, under the access rules `access` (TOML) when given, and attach
/// the rows' policy to it.
async fn boot(state: &Path, roster: Vec<DurableAgentSpec>, access: Option<&str>) -> Rig {
    let fail_sessions = Arc::new(FailSessions::default());
    let mut builder = UnifiedRuntimeBuilder::default()
        .definition(definition(&fresh_mob_id()))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(Listed(roster)))
        .identity_runtime_instance_id("meta-identity-visibility")
        .session_hook(Arc::clone(&fail_sessions) as Arc<dyn SessionHook>)
        .default_llm_client(Arc::new(QuietClient));
    if let Some(access) = access {
        let config: meerkat_meta::AccessControlConfig =
            toml::from_str(access).expect("access TOML");
        builder = builder
            .access_controller(meerkat_meta::AccessController::new(config).expect("controller"));
    }
    let runtime = Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime");
    assert!(
        runtime.attach_identity_policy(policy(), IdentityFamilyOptions::new()),
        "the runtime serves identities and takes the policy"
    );
    Rig {
        runtime: Arc::new(runtime),
        fail_sessions,
    }
}

/// The decisions of a Console that asks for sign-in and admits the three
/// persons the rows sign in as.
fn signed_console_decisions() -> meerkat_meta::RuntimeDecisionState {
    meerkat_meta::build_runtime_decision_state(meerkat_meta::RuntimeDecisionInputs {
        bigquery: meerkat_meta::BigQueryNaming {
            dataset: "meta_identity_visibility".to_string(),
            table: "events".to_string(),
        },
        trusted_meta_toml: "modules = []\n".to_string(),
        auth: meerkat_meta::AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec![ROOT.to_string(), ANN.to_string(), BOB.to_string()],
        },
        trusted_oidc: meerkat_meta::TrustedOidcRuntimeConfig {
            discovery_json:
                r#"{"issuer":"https://trusted.meta.localhost","jwks_uri":"https://trusted.meta.localhost/.well-known/jwks.json"}"#
                    .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
            shared_account_tokens: false,
        },
        console: meerkat_meta::ConsolePolicy {
            require_app_auth: true,
            read_only: false,
            ..meerkat_meta::ConsolePolicy::default()
        },
        ops: meerkat_meta::RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

/// One Console JSON-RPC call on `app`, signed in as `subject`.
async fn console_rpc(app: &axum::Router, subject: &str, method: &str, params: Value) -> Value {
    use tower::ServiceExt;
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = Some("kid-current".to_string());
    let token = jsonwebtoken::encode(
        &header,
        &json!({
            "iss": ISSUER,
            "aud": "meerkat-console",
            "sub": subject,
            "email": subject,
            "provider": "google_oauth",
            "exp": chrono::Utc::now().timestamp() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(b"phase7-trusted-current-secret"),
    )
    .expect("signed fixture token");
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .body(axum::body::Body::from(
            json!({"jsonrpc": "2.0", "id": "identity", "method": method, "params": params})
                .to_string(),
        ))
        .expect("rpc request");
    let response = tokio::time::timeout(CALL_BOUND, app.clone().oneshot(request))
        .await
        .expect("bounded rpc")
        .expect("rpc response");
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("rpc body");
    serde_json::from_slice(&body).unwrap_or_else(|_| panic!("rpc json for {method}"))
}

/// The result of a Console call that must answer one.
fn console_ok(answer: &Value) -> &Value {
    assert!(
        answer["error"].is_null(),
        "the call answers a result: {answer:#}"
    );
    &answer["result"]
}

/// The error of a Console call that must answer none.
fn console_refused(answer: &Value) -> &Value {
    assert!(
        answer["result"].is_null() && !answer["error"].is_null(),
        "the call is refused: {answer:#}"
    );
    &answer["error"]
}

/// The agent `name` as the Console answers `subject`'s get of it.
async fn console_get(app: &axum::Router, subject: &str, name: &str) -> Value {
    console_rpc(app, subject, "meta/identity/get", json!({"name": name})).await
}

/// The names the Console lists to `subject`, archived agents included, in
/// name order.
async fn console_listed(app: &axum::Router, subject: &str) -> Vec<String> {
    let listed = console_rpc(
        app,
        subject,
        "meta/identity/list",
        json!({"include_archived": true}),
    )
    .await;
    let mut names: Vec<String> = console_ok(&listed)["agents"]
        .as_array()
        .unwrap_or_else(|| panic!("a list answers agents: {listed:#}"))
        .iter()
        .map(|agent| agent["name"].as_str().expect("a name").to_owned())
        .collect();
    names.sort_unstable();
    names
}

fn agent_ref(name: &str, digest: &Sha256Digest) -> AgentRef {
    serde_json::from_value(json!({"name": name, "expected_spec_digest": digest.as_str()}))
        .expect("a change of one agent as a surface writes it")
}

fn update_of(name: &str, digest: &Sha256Digest, set: Value) -> IdentityChangeSpec {
    IdentityChangeSpec::Update(
        serde_json::from_value(
            json!({"name": name, "expected_spec_digest": digest.as_str(), "set": set}),
        )
        .expect("an update as a surface writes it"),
    )
}

fn refusal_of(outcome: &IdentityOutcome) -> (String, Option<String>) {
    let refusal = outcome
        .refusal()
        .unwrap_or_else(|| panic!("the change is refused: {outcome:?}"));
    (refusal.kind_token().to_owned(), refusal.field.clone())
}

impl Rig {
    fn family(&self) -> Arc<dyn IdentityFamily> {
        self.runtime
            .identity_family()
            .expect("the family is attached")
    }

    /// The Console of this runtime, which asks for sign-in.
    fn console(&self) -> axum::Router {
        self.runtime
            .build_console_json_router(signed_console_decisions())
    }

    /// One change of the family, as `requester`'s request `request_id`.
    async fn change(
        &self,
        requester: &ChangeRequester,
        request_id: &str,
        change: IdentityChangeSpec,
    ) -> IdentityOutcome {
        let operation_id =
            OperationId::for_request(&requester.recorded(), request_id).expect("a request id");
        self.family()
            .change(
                &self.runtime.mob_handle(),
                requester,
                IdentityChangeCall::new(operation_id, change),
            )
            .await
    }

    /// The agent `name` in full, as the host reads it.
    async fn agent(&self, name: &str) -> AgentRecord {
        match self
            .family()
            .read(
                &self.runtime.mob_handle(),
                &ChangeRequester::host("ops"),
                IdentityRead::Get(GetAgent::new(name)),
            )
            .await
        {
            Ok(IdentityReadAnswer::Get(record)) => *record,
            other => panic!("the host reads the agent {name}: {other:?}"),
        }
    }

    async fn shutdown(self) {
        let _ = self.runtime.shutdown().await;
    }
}

/// The access rules of the rows about a deny by role or by label: every
/// signed-in person may view, create and change agents; one person is
/// denied the agents of the profile `reviewer`, and the agents whose label
/// `tier` is `sealed`.
const SCOPED_DENIES: &str = r#"
enabled = true
admins = ["root@example.org"]

[[rules]]
id = "people"
actions = ["agent.identity.create", "agent.view", "agent.identity.manage", "agent.identity.remove"]

[[rules]]
id = "no-reviewer"
effect = "deny"
subjects = ["bob@example.org"]
actions = ["agent.view", "agent.identity.manage", "agent.identity.remove"]
roles = ["reviewer"]

[[rules]]
id = "nothing-sealed"
effect = "deny"
subjects = ["bob@example.org"]
actions = ["agent.view", "agent.identity.manage", "agent.identity.remove"]
match_labels = { tier = "sealed" }
"#;

/// The plan and the changes of one agent the rows ask as the denied person:
/// each as its method and its params, under request ids that carry `tag`.
fn changes_of(agent: &str, digest: &Value, tag: &str) -> Vec<(&'static str, Value)> {
    let named = |request: &str| {
        json!({"request_id": format!("{request}-{tag}"), "name": agent,
               "expected_spec_digest": digest})
    };
    let mut update = named("update");
    update["set"] = json!({"display_name": "Other"});
    let mut plan = named("plan");
    plan["change"] = json!("update");
    plan["set"] = json!({"display_name": "Other"});
    vec![
        ("meta/identity/plan", plan),
        ("meta/identity/update", update),
        ("meta/identity/archive", named("archive")),
        ("meta/identity/remove", named("remove")),
    ]
}

/// `answer` with the name `name` spelled `other` wherever it stands.
fn renamed(answer: &Value, name: &str, other: &str) -> Value {
    serde_json::from_str(&answer.to_string().replace(name, other)).expect("an answer is JSON")
}

/// A create of one agent that names `peer` as its peer.
fn create_near(peer: &str, tag: &str) -> Value {
    json!({"request_id": format!("near-{tag}"),
           "agents": [{"name": format!("near-{tag}"), "profile": "worker", "peers": [peer]}]})
}

/// An access rule that denies a person the agents of a profile, or the
/// agents that carry a label, holds for an agent no member was ever seated
/// for: an agent whose first message did not start is committed and owed
/// its seat, and it is decided on its own record, as a seated agent is on
/// its seat. The denied person's get, plan and every change of it are
/// refused as those of a seated agent the deny names are, it is not in
/// their list, and it is no peer of a create of theirs. A person the rules
/// allow reads it, and the denied person reads an agent owed its seat that
/// no deny names.
#[tokio::test(flavor = "multi_thread")]
async fn a_deny_by_role_or_by_label_holds_for_an_agent_owed_its_seat() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), Vec::new(), Some(SCOPED_DENIES)).await;
    let app = rig.console();
    // No session starts in this row: every agent stays owed its seat.
    rig.fail_sessions.failing.store(true, Ordering::SeqCst);
    let made = console_rpc(
        &app,
        ROOT,
        "meta/identity/create",
        json!({"request_id": "make-1", "agents": [
            {"name": "rev-owed", "profile": "reviewer", "first_message": "Begin the watch."},
            {"name": "sealed-owed", "profile": "worker", "labels": {"tier": "sealed"},
             "first_message": "Begin the watch."},
            {"name": "plain-owed", "profile": "worker", "first_message": "Begin the watch."},
        ]}),
    )
    .await;
    let made = console_ok(&made).clone();
    assert_eq!(made["outcome"], json!("done"), "{made:#}");
    for agent in made["agents"].as_array().expect("the created agents") {
        assert_eq!(agent["seat"], json!("owed"), "{made:#}");
    }

    // The twin: the denied person reads an agent owed its seat that no
    // deny names.
    let plain = console_get(&app, BOB, "plain-owed").await;
    assert_eq!(
        console_ok(&plain)["readiness"],
        json!("unseated"),
        "{plain:#}"
    );

    for name in ["rev-owed", "sealed-owed"] {
        // A person the rules allow reads it.
        let allowed = console_get(&app, ANN, name).await;
        let digest = console_ok(&allowed)["spec_digest"].clone();
        assert_eq!(
            console_ok(&allowed)["readiness"],
            json!("unseated"),
            "{allowed:#}"
        );

        // The denied person's get is refused as their get of a seated
        // agent the deny names is.
        let got = console_get(&app, BOB, name).await;
        assert_eq!(
            console_refused(&got)["data"]["kind"],
            json!("access_denied"),
            "{name}: {got:#}"
        );

        // So are a plan and every change of it.
        for (method, params) in changes_of(name, &digest, name) {
            let refused = console_rpc(&app, BOB, method, params).await;
            assert_eq!(
                console_refused(&refused)["data"]["kind"],
                json!("access_denied"),
                "{name}: {method}: {refused:#}"
            );
        }
        let still = console_get(&app, ROOT, name).await;
        assert_eq!(
            (
                &console_ok(&still)["archived"],
                &console_ok(&still)["spec_digest"]
            ),
            (&json!(false), &digest),
            "{name}: nothing changed it"
        );

        // It is no peer of a create of theirs.
        let named = console_rpc(&app, BOB, "meta/identity/create", create_near(name, name)).await;
        let unheld = console_rpc(
            &app,
            BOB,
            "meta/identity/create",
            create_near("nobody", &format!("{name}-absent")),
        )
        .await;
        assert_eq!(
            console_refused(&unheld)["data"]["kind"],
            json!("peer_unknown"),
            "{unheld:#}"
        );
        assert_eq!(
            console_refused(&named),
            console_refused(&unheld),
            "{name}: it is no peer the denied person may name"
        );
    }

    // The lists: the denied person is listed the agent no deny names and
    // none a deny names; a person the rules allow is listed all three.
    let bobs = console_listed(&app, BOB).await;
    assert_eq!(bobs, vec!["plain-owed".to_owned()], "{bobs:?}");
    let anns = console_listed(&app, ANN).await;
    assert_eq!(
        anns,
        vec![
            "plain-owed".to_owned(),
            "rev-owed".to_owned(),
            "sealed-owed".to_owned(),
        ],
        "{anns:?}"
    );
    rig.shutdown().await;
}

/// The same rules hold for an archived agent once its member has left the
/// roster: the denied person's get, plan and every change of an archived
/// agent a deny names are refused as those of a seated agent the deny names
/// are, nothing changes it, it is not in their list and it is no peer of a
/// create of theirs. A person the rules allow reads it, and the denied
/// person reads an archived agent no deny names.
#[tokio::test(flavor = "multi_thread")]
async fn a_deny_by_role_or_by_label_holds_for_an_archived_agent() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), Vec::new(), Some(SCOPED_DENIES)).await;
    let app = rig.console();
    let made = console_rpc(
        &app,
        ROOT,
        "meta/identity/create",
        json!({"request_id": "make-1", "agents": [
            {"name": "rev-seated", "profile": "reviewer"},
            {"name": "rev-archived", "profile": "reviewer"},
            {"name": "sealed-seated", "profile": "worker", "labels": {"tier": "sealed"}},
            {"name": "sealed-archived", "profile": "worker", "labels": {"tier": "sealed"}},
            {"name": "plain-archived", "profile": "worker"},
        ]}),
    )
    .await;
    assert_eq!(console_ok(&made)["outcome"], json!("done"), "{made:#}");
    let mut digests = BTreeMap::new();
    for name in ["rev-archived", "sealed-archived", "plain-archived"] {
        let digest = console_ok(&console_get(&app, ROOT, name).await)["spec_digest"].clone();
        let archived = console_rpc(
            &app,
            ROOT,
            "meta/identity/archive",
            json!({"request_id": format!("archive-{name}"), "name": name,
                   "expected_spec_digest": digest}),
        )
        .await;
        assert_eq!(
            console_ok(&archived)["outcome"],
            json!("done"),
            "{archived:#}"
        );
        let digest = console_ok(&console_get(&app, ROOT, name).await)["spec_digest"].clone();
        digests.insert(name, digest);
    }
    // The archived agents' members retire: the row asks once the roster
    // holds no row of theirs.
    let mob = rig.runtime.mob_handle();
    for name in ["rev-archived", "sealed-archived", "plain-archived"] {
        let member = meerkat_mob::AgentIdentity::from(name);
        tokio::time::timeout(CALL_BOUND, async {
            while mob
                .get_member(&member)
                .await
                .expect("a roster read")
                .is_some()
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the member of {name} leaves the roster"));
    }

    // The twins: a seated agent a deny names is refused to the denied
    // person, and they read an archived agent no deny names.
    for name in ["rev-seated", "sealed-seated"] {
        console_refused(&console_get(&app, BOB, name).await);
    }
    let plain = console_get(&app, BOB, "plain-archived").await;
    assert_eq!(console_ok(&plain)["archived"], json!(true), "{plain:#}");

    for name in ["rev-archived", "sealed-archived"] {
        let digest = digests[name].clone();
        // A person the rules allow reads it.
        let allowed = console_get(&app, ANN, name).await;
        assert_eq!(console_ok(&allowed)["archived"], json!(true), "{allowed:#}");

        // The denied person's get is refused exactly as their get of the
        // seated agent the same deny names.
        let seated = name.replace("archived", "seated");
        let got = console_get(&app, BOB, name).await;
        assert_eq!(
            console_refused(&got)["data"]["kind"],
            json!("access_denied"),
            "{got:#}"
        );
        assert_eq!(
            renamed(&got, name, &seated),
            console_get(&app, BOB, &seated).await,
            "{name}: a get of it answers as the get of a seated agent the deny names"
        );

        // So are a plan and every change of it.
        for (method, params) in changes_of(name, &digest, name) {
            let refused = console_rpc(&app, BOB, method, params).await;
            assert_eq!(
                console_refused(&refused)["data"]["kind"],
                json!("access_denied"),
                "{name}: {method}: {refused:#}"
            );
        }
        let still = console_get(&app, ROOT, name).await;
        assert_eq!(
            (
                &console_ok(&still)["archived"],
                &console_ok(&still)["spec_digest"]
            ),
            (&json!(true), &digest),
            "{name}: nothing changed it"
        );

        // It is no peer of a create of theirs.
        let named = console_rpc(&app, BOB, "meta/identity/create", create_near(name, name)).await;
        let unheld = console_rpc(
            &app,
            BOB,
            "meta/identity/create",
            create_near("nobody", &format!("{name}-absent")),
        )
        .await;
        assert_eq!(
            console_refused(&named),
            console_refused(&unheld),
            "{name}: it is no peer the denied person may name"
        );
    }

    // The lists: the denied person is listed the archived agent no deny
    // names and none a deny names; a person the rules allow is listed all.
    let bobs = console_listed(&app, BOB).await;
    assert_eq!(bobs, vec!["plain-archived".to_owned()], "{bobs:?}");
    let anns = console_listed(&app, ANN).await;
    assert_eq!(
        anns,
        vec![
            "plain-archived".to_owned(),
            "rev-archived".to_owned(),
            "rev-seated".to_owned(),
            "sealed-archived".to_owned(),
            "sealed-seated".to_owned(),
        ],
        "{anns:?}"
    );
    rig.shutdown().await;
}

/// A label only the host's own configuration writes stays on an agent
/// through an update that does not touch it: the update of another label is
/// carried out and the label is as it was. An update that would change or
/// remove such a label is refused `reserved_key`, and so is one that would
/// add it to an agent that does not carry it.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_keeps_a_reserved_label_it_does_not_touch() {
    const GROUP: &str = "console_group";
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(
        temp.path(),
        vec![
            roster_spec("desk", "worker", &[(GROUP, "front"), ("topic", "mail")]),
            roster_spec("plain", "worker", &[("topic", "mail")]),
        ],
        None,
    )
    .await;
    let person = ChangeRequester::human("person-1");
    for name in ["desk", "plain"] {
        let digest = rig.agent(name).await.agent.spec_digest;
        let adopted = rig
            .change(
                &person,
                &format!("adopt-{name}"),
                IdentityChangeSpec::Adopt(agent_ref(name, &digest)),
            )
            .await;
        assert_eq!(adopted.token(), "done", "{adopted:?}");
    }
    let labels = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    };
    assert_eq!(
        rig.agent("desk").await.labels,
        labels(&[(GROUP, "front"), ("topic", "mail")]),
        "the adopted agent carries its configuration's labels"
    );

    // The twin: an agent that carries no such label takes a label update.
    let digest = rig.agent("plain").await.agent.spec_digest;
    let twin = rig
        .change(
            &person,
            "plain-1",
            update_of("plain", &digest, json!({"labels": {"topic": "post"}})),
        )
        .await;
    assert_eq!(twin.token(), "done", "{twin:?}");

    // Another label is set, and then removed: the reserved label stays.
    let digest = rig.agent("desk").await.agent.spec_digest;
    let set = rig
        .change(
            &person,
            "desk-1",
            update_of("desk", &digest, json!({"labels": {"topic": "post"}})),
        )
        .await;
    assert_eq!(set.token(), "done", "{set:?}");
    assert_eq!(
        rig.agent("desk").await.labels,
        labels(&[(GROUP, "front"), ("topic", "post")]),
    );
    let digest = rig.agent("desk").await.agent.spec_digest;
    let removed = rig
        .change(
            &person,
            "desk-2",
            update_of("desk", &digest, json!({"labels_remove": ["topic"]})),
        )
        .await;
    assert_eq!(removed.token(), "done", "{removed:?}");
    assert_eq!(rig.agent("desk").await.labels, labels(&[(GROUP, "front")]));

    // It is not a request's to change, to remove or to add.
    let digest = rig.agent("desk").await.agent.spec_digest;
    for (request, set) in [
        ("desk-3", json!({"labels": {(GROUP): "back"}})),
        ("desk-4", json!({"labels_remove": [GROUP]})),
        (
            "desk-5",
            json!({"labels": {(GROUP): "back", "topic": "post"}}),
        ),
    ] {
        let refused = rig
            .change(&person, request, update_of("desk", &digest, set.clone()))
            .await;
        assert_eq!(
            refusal_of(&refused),
            ("reserved_key".to_owned(), Some("set.labels".to_owned())),
            "{set}: {refused:?}"
        );
    }
    let desk = rig.agent("desk").await;
    assert_eq!(
        desk.labels,
        labels(&[(GROUP, "front")]),
        "nothing changed it"
    );
    assert_eq!(desk.agent.spec_digest, digest);
    let digest = rig.agent("plain").await.agent.spec_digest;
    let added = rig
        .change(
            &person,
            "plain-2",
            update_of("plain", &digest, json!({"labels": {(GROUP): "front"}})),
        )
        .await;
    assert_eq!(
        refusal_of(&added),
        ("reserved_key".to_owned(), Some("set.labels".to_owned())),
        "{added:?}"
    );
    rig.shutdown().await;
}

/// The Console's methods that name one member by `identity` and read it.
const MEMBER_READS: [&str; 3] = [
    "meta/console/inspect_identity",
    "meta/identity/resolved_tools",
    "meta/identity/routing_status",
];

/// One of the Console's member reads of the agent `name`, asked by
/// `subject`.
async fn member_read(app: &axum::Router, subject: &str, method: &str, name: &str) -> Value {
    console_rpc(app, subject, method, json!({ "identity": name })).await
}

/// Every member read of the agent `name` refuses `subject` `access_denied`,
/// as it refuses them a seated agent a deny names.
async fn assert_member_reads_denied(app: &axum::Router, subject: &str, name: &str) {
    for method in MEMBER_READS {
        let answer = member_read(app, subject, method, name).await;
        assert_eq!(
            console_refused(&answer)["data"]["kind"],
            json!("access_denied"),
            "{method} of {name} by {subject}: {answer:#}"
        );
    }
}

/// Every member read of the agent `name` answers `subject` exactly as it
/// answers them for a name no agent holds.
async fn assert_member_reads_as_absent(app: &axum::Router, subject: &str, name: &str) {
    for method in MEMBER_READS {
        let answer = member_read(app, subject, method, name).await;
        let absent = member_read(app, subject, method, "nobody").await;
        assert_eq!(
            renamed(&answer, name, "nobody"),
            absent,
            "{method} of {name} by {subject} answers as for a name no agent holds"
        );
    }
}

/// Make the record of the agent `name` one no build can read, in the mob's
/// identity store under `state`, beside the running mob.
fn spoil_record(state: &Path, name: &str) {
    let member = meerkat_meta::member_comms_id::roster_member_id_for_identity(name);
    let mut spoiled = 0;
    let mut pending = vec![state.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("read the state directory") {
            let path = entry.expect("an entry of the state directory").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let Ok(connection) = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            ) else {
                continue;
            };
            if connection.busy_timeout(Duration::from_secs(10)).is_err() {
                continue;
            }
            // A file that is no store of identity records refuses the
            // statement and is passed over.
            if let Ok(rows) = connection.execute(
                "UPDATE mob_identity_intents SET record_json = ?1 WHERE agent_identity = ?2",
                rusqlite::params![br#"{"schema_version":2}"#.to_vec(), member.as_str()],
            ) {
                spoiled += rows;
            }
        }
    }
    assert_eq!(spoiled, 1, "the one record of {name} is spoiled");
}

/// The Console's member reads (the inspection of an agent, its resolved
/// tools, its routing status) decide an agent owed its seat on its own
/// record, as they decide a seated agent on its seat: a person a deny by
/// role or by label names is refused, a person the rules allow and an
/// access administrator are answered, and the denied person is answered for
/// an agent no deny names. An agent whose record cannot be read is answered
/// to an access administrator only: every other person is answered as for a
/// name no agent holds.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_s_member_reads_decide_an_agent_owed_its_seat_on_its_record() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), Vec::new(), Some(SCOPED_DENIES)).await;
    let app = rig.console();
    // No session starts in this row: every agent stays owed its seat.
    rig.fail_sessions.failing.store(true, Ordering::SeqCst);
    let made = console_rpc(
        &app,
        ROOT,
        "meta/identity/create",
        json!({"request_id": "make-1", "agents": [
            {"name": "rev-owed", "profile": "reviewer", "first_message": "Begin the watch."},
            {"name": "sealed-owed", "profile": "worker", "labels": {"tier": "sealed"},
             "first_message": "Begin the watch."},
            {"name": "plain-owed", "profile": "worker", "first_message": "Begin the watch."},
            {"name": "dim-owed", "profile": "worker", "first_message": "Begin the watch."},
        ]}),
    )
    .await;
    let made = console_ok(&made).clone();
    assert_eq!(made["outcome"], json!("done"), "{made:#}");
    for agent in made["agents"].as_array().expect("the created agents") {
        assert_eq!(agent["seat"], json!("owed"), "{made:#}");
    }
    // The record of one agent cannot be read from here on.
    spoil_record(temp.path(), "dim-owed");

    for name in ["rev-owed", "sealed-owed"] {
        assert_member_reads_denied(&app, BOB, name).await;
        // A person the rules allow and an access administrator are
        // answered the inspection, and one answer for the other reads.
        for subject in [ANN, ROOT] {
            let inspected = member_read(&app, subject, MEMBER_READS[0], name).await;
            assert_eq!(
                console_ok(&inspected)["identity"]["identity"],
                json!(name),
                "{subject}: {inspected:#}"
            );
        }
        for method in &MEMBER_READS[1..] {
            let allowed = member_read(&app, ANN, method, name).await;
            assert_ne!(
                allowed["error"]["data"]["kind"],
                json!("access_denied"),
                "{method} of {name}: {allowed:#}"
            );
            assert_eq!(
                allowed,
                member_read(&app, ROOT, method, name).await,
                "{method} of {name} answers a person the rules allow as an administrator"
            );
        }
    }

    // The twin: the denied person is answered for an agent no deny names.
    let plain = member_read(&app, BOB, MEMBER_READS[0], "plain-owed").await;
    assert_eq!(
        console_ok(&plain)["identity"]["identity"],
        json!("plain-owed"),
        "{plain:#}"
    );

    // The agent whose record cannot be read.
    for subject in [BOB, ANN] {
        assert_member_reads_as_absent(&app, subject, "dim-owed").await;
    }
    let administered = member_read(&app, ROOT, MEMBER_READS[0], "dim-owed").await;
    assert_ne!(
        renamed(&administered, "dim-owed", "nobody"),
        member_read(&app, ROOT, MEMBER_READS[0], "nobody").await,
        "an access administrator is not answered as for a name no agent holds"
    );
    rig.shutdown().await;
}

/// The Console's member reads decide an archived agent whose member has
/// left the roster on its own record, the first time the Console is asked:
/// a person a deny by role or by label names is refused; an archived agent
/// private to a person is answered to that person and to an access
/// administrator, and every other person is answered exactly as for a name
/// no agent holds; the denied person is answered for an archived agent no
/// rule names, until its record gets a label a deny names: a change of the
/// record decides the next read.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_s_member_reads_decide_an_archived_agent_on_its_record() {
    let temp = tempfile::tempdir().unwrap();
    let owner = meerkat_meta::private_owner_key(ISSUER, ANN);
    let archived = [
        "confidant",
        "rev-archived",
        "sealed-archived",
        "plain-archived",
    ];
    let rig = boot(
        temp.path(),
        vec![
            roster_spec(
                "confidant",
                "worker",
                &[("console.private_to", owner.as_str())],
            ),
            roster_spec("rev-archived", "reviewer", &[]),
            roster_spec("sealed-archived", "worker", &[("tier", "sealed")]),
            roster_spec("plain-archived", "worker", &[]),
        ],
        Some(SCOPED_DENIES),
    )
    .await;
    let app = rig.console();
    // The host adopts and archives each agent before the Console is asked
    // anything.
    let host = ChangeRequester::host_operator("rpc");
    for name in archived {
        let digest = rig.agent(name).await.agent.spec_digest;
        let adopted = rig
            .change(
                &host,
                &format!("adopt-{name}"),
                IdentityChangeSpec::Adopt(agent_ref(name, &digest)),
            )
            .await;
        assert_eq!(adopted.token(), "done", "{adopted:?}");
        let digest = rig.agent(name).await.agent.spec_digest;
        let archive = rig
            .change(
                &host,
                &format!("archive-{name}"),
                IdentityChangeSpec::Archive(agent_ref(name, &digest)),
            )
            .await;
        assert_eq!(archive.token(), "done", "{archive:?}");
    }
    // The archived agents' members retire: the row asks once the roster
    // holds no row of theirs.
    let mob = rig.runtime.mob_handle();
    for name in archived {
        let member = meerkat_mob::AgentIdentity::from(
            meerkat_meta::member_comms_id::roster_member_id_for_identity(name).as_str(),
        );
        tokio::time::timeout(CALL_BOUND, async {
            while mob
                .get_member(&member)
                .await
                .expect("a roster read")
                .is_some()
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the member of {name} leaves the roster"));
    }

    // A deny by role or by label.
    for name in ["rev-archived", "sealed-archived"] {
        assert_member_reads_denied(&app, BOB, name).await;
    }
    // The agent private to a person: every other person is answered as for
    // a name no agent holds.
    assert_member_reads_as_absent(&app, BOB, "confidant").await;
    // Its person and an access administrator are answered it.
    for subject in [ANN, ROOT] {
        let inspected = member_read(&app, subject, MEMBER_READS[0], "confidant").await;
        assert_eq!(
            console_ok(&inspected)["identity"]["identity"],
            json!("confidant"),
            "{subject}: {inspected:#}"
        );
        let tools = member_read(&app, subject, MEMBER_READS[1], "confidant").await;
        assert_eq!(
            console_ok(&tools)["identity"],
            json!("confidant"),
            "{subject}: {tools:#}"
        );
    }
    // The twin: the denied person is answered for an archived agent no rule
    // names.
    let plain = member_read(&app, BOB, MEMBER_READS[0], "plain-archived").await;
    assert_eq!(
        console_ok(&plain)["identity"]["identity"],
        json!("plain-archived"),
        "{plain:#}"
    );
    // Its record gets a label a deny names: the next read is decided on
    // the record as it is now.
    let digest = rig.agent("plain-archived").await.agent.spec_digest;
    let sealed = rig
        .change(
            &host,
            "seal-plain-archived",
            update_of(
                "plain-archived",
                &digest,
                json!({"labels": {"tier": "sealed"}}),
            ),
        )
        .await;
    assert_eq!(sealed.token(), "done", "{sealed:?}");
    assert_member_reads_denied(&app, BOB, "plain-archived").await;
    rig.shutdown().await;
}

/// [`boot`], with `compose` applied to the builder before it builds: what
/// one row's composition sets beside the rows' own.
async fn boot_with(
    state: &Path,
    roster: Vec<DurableAgentSpec>,
    access: Option<&str>,
    compose: impl FnOnce(UnifiedRuntimeBuilder) -> UnifiedRuntimeBuilder,
) -> Rig {
    let fail_sessions = Arc::new(FailSessions::default());
    let mut builder = UnifiedRuntimeBuilder::default()
        .definition(definition(&fresh_mob_id()))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(Listed(roster)))
        .identity_runtime_instance_id("meta-identity-visibility")
        .session_hook(Arc::clone(&fail_sessions) as Arc<dyn SessionHook>)
        .default_llm_client(Arc::new(QuietClient));
    if let Some(access) = access {
        let config: meerkat_meta::AccessControlConfig =
            toml::from_str(access).expect("access TOML");
        builder = builder
            .access_controller(meerkat_meta::AccessController::new(config).expect("controller"));
    }
    let runtime = Box::pin(compose(builder).build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime");
    assert!(
        runtime.attach_identity_policy(policy(), IdentityFamilyOptions::new()),
        "the runtime serves identities and takes the policy"
    );
    Rig {
        runtime: Arc::new(runtime),
        fail_sessions,
    }
}

/// `spec`, placed on a member host that no process of the row serves: that
/// host creates the agent's member at the agent's first use.
fn placed(mut spec: DurableAgentSpec) -> DurableAgentSpec {
    spec.placement = Some(meerkat_contracts::WireHostRef("member-host-1".to_owned()));
    spec
}

/// The agent `name` stands as the row needs it: no member holds it (the
/// roster holds no row of its name), the mob holds its name for its host's
/// first use of it, the identity runtime has registered it and binds it to
/// no session, and the host reads it with no seat.
async fn assert_registered_with_no_member(rig: &Rig, name: &str) {
    let member = meerkat_meta::member_comms_id::roster_member_id_for_identity(name);
    let mob = rig.runtime.mob_handle();
    assert!(
        mob.get_member(&member)
            .await
            .expect("a roster read")
            .is_none(),
        "no member holds {name}"
    );
    assert!(
        mob.list_all_members()
            .await
            .iter()
            .all(|row| row.agent_identity != member),
        "the roster holds no row of {name}"
    );
    let hosted_on_first_use =
        meerkat_mob::EntryStanding::Unseated(meerkat_mob::UnseatedEntryCause::HostedOnFirstUse);
    match mob.identity_entry(&member).expect("the mob's entry") {
        meerkat_mob::IdentityEntryRead::Entry(entry) => {
            assert_eq!(entry.standing, hosted_on_first_use, "{name}: {entry:?}");
        }
        other => panic!("the mob holds {name} as an entry: {other:?}"),
    }
    let status = rig
        .runtime
        .identity_runtime()
        .expect("the identity runtime")
        .status(&AgentIdentity::parse(name).unwrap())
        .await
        .unwrap_or_else(|error| panic!("{name} is not registered: {error}"));
    assert_eq!(
        status.state,
        meerkat_meta::identity_first::IdentityLifecycleState::Dormant,
        "{name}: {status:?}"
    );
    assert!(
        status.session_id.is_none() && status.member_id.is_none(),
        "{name} is bound to no session: {status:?}"
    );
    assert_eq!(
        rig.agent(name).await.agent.readiness,
        meerkat_meta::identity_creation::AgentReadiness::Unseated,
        "the host reads {name} with no seat"
    );
}

/// The Console's member reads decide an agent its host configures, for
/// which no member was ever created, on what the host configures for it, as
/// they decide a seated agent on its seat. The row's agents are placed on a
/// member host, which creates the member of each at the agent's first use,
/// and the start leaves them for it: the identity runtime has registered
/// each, the mob holds its name with no member, and no read of the row uses
/// one. Such an agent that is private to a person is answered to that
/// person and to an access administrator, and every other person is
/// answered exactly as for a name no agent holds. A person a deny by role
/// names is refused such an agent of that role, which a person the rules
/// allow is answered, and the denied person is answered such an agent no
/// rule names.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_s_member_reads_decide_a_placed_agent_no_member_was_created_for() {
    const PRIVATE: &str = "confidant-placed";
    const REVIEWER: &str = "rev-placed";
    const PLAIN: &str = "plain-placed";
    let temp = tempfile::tempdir().unwrap();
    let owner = meerkat_meta::private_owner_key(ISSUER, ANN);
    let private_to_ann = [("console.private_to", owner.as_str())];
    let rig = boot_with(
        temp.path(),
        vec![
            placed(roster_spec(PRIVATE, "worker", &private_to_ann)),
            placed(roster_spec(REVIEWER, "reviewer", &[])),
            placed(roster_spec(PLAIN, "worker", &[])),
        ],
        Some(SCOPED_DENIES),
        // A placed agent the mob holds no member of is left for its first
        // use: the start creates no member of it.
        |builder| {
            builder.identity_bootstrap_mode(meerkat_meta::IdentityBootstrapMode::LazyMaterialize)
        },
    )
    .await;
    let app = rig.console();

    // The state the row is about, before the Console is asked anything.
    for name in [PRIVATE, REVIEWER, PLAIN] {
        assert_registered_with_no_member(&rig, name).await;
    }
    assert_eq!(
        rig.agent(PRIVATE).await.labels.get("console.private_to"),
        Some(&owner),
        "the registered agent carries its person"
    );

    // The agent private to a person: every other person is answered as for
    // a name no agent holds.
    assert_member_reads_as_absent(&app, BOB, PRIVATE).await;
    // Its person and an access administrator are answered it.
    for subject in [ANN, ROOT] {
        let inspected = member_read(&app, subject, MEMBER_READS[0], PRIVATE).await;
        assert_eq!(
            console_ok(&inspected)["identity"]["identity"],
            json!(PRIVATE),
            "{subject}: {inspected:#}"
        );
    }

    // A deny by role: the person it names is refused, and a person the
    // rules allow is answered.
    assert_member_reads_denied(&app, BOB, REVIEWER).await;
    let allowed = member_read(&app, ANN, MEMBER_READS[0], REVIEWER).await;
    assert_eq!(
        console_ok(&allowed)["identity"]["identity"],
        json!(REVIEWER),
        "{allowed:#}"
    );

    // The twin: the denied person is answered for an agent no rule names.
    let plain = member_read(&app, BOB, MEMBER_READS[0], PLAIN).await;
    assert_eq!(
        console_ok(&plain)["identity"]["identity"],
        json!(PLAIN),
        "{plain:#}"
    );

    // No read used an agent: each stands as it stood.
    for name in [PRIVATE, REVIEWER, PLAIN] {
        assert_registered_with_no_member(&rig, name).await;
    }
    rig.shutdown().await;
}

/// A signed-in person's Console token.
fn console_token(subject: &str) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = Some("kid-current".to_string());
    jsonwebtoken::encode(
        &header,
        &json!({
            "iss": ISSUER,
            "aud": "meerkat-console",
            "sub": subject,
            "email": subject,
            "provider": "google_oauth",
            "exp": chrono::Utc::now().timestamp() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(b"phase7-trusted-current-secret"),
    )
    .expect("signed fixture token")
}

/// The answer of a Console GET of `path` as `subject`, which succeeds.
async fn console_read(app: &axum::Router, subject: &str, path: &str) -> Value {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(path)
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", console_token(subject)),
        )
        .body(axum::body::Body::empty())
        .expect("get request");
    let response = tokio::time::timeout(CALL_BOUND, app.clone().oneshot(request))
        .await
        .expect("bounded get")
        .expect("get response");
    assert_eq!(response.status(), 200, "{path} as {subject}");
    let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("get body");
    serde_json::from_slice(&body).unwrap_or_else(|_| panic!("json of {path}"))
}

/// The rows' agents among `names`, without the name of the runtime a
/// Console that serves several puts before each, in name order.
fn of_the_row(names: impl Iterator<Item = String>, agents: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = names
        .map(|name| name.rsplit('/').next().unwrap_or_default().to_owned())
        .filter(|name| agents.contains(&name.as_str()))
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// The agents of `agents` whose record each list of the Console `app` names
/// to `subject`: the identity list as a route and as a call, and with
/// `sidebar` the agent rows of the experience.
async fn listed_records(
    app: &axum::Router,
    subject: &str,
    agents: &[&str],
    sidebar: bool,
) -> Vec<(&'static str, Vec<String>)> {
    let names_of = |listed: &Value, key: &str| {
        of_the_row(
            listed
                .as_array()
                .unwrap_or_else(|| panic!("a list of records: {listed:#}"))
                .iter()
                .map(|record| {
                    record[key]
                        .as_str()
                        .or(record["agent_identity"].as_str())
                        .unwrap_or_default()
                        .to_owned()
                }),
            agents,
        )
    };
    let route = console_read(app, subject, "/console/identities").await;
    let call = console_rpc(app, subject, "meta/console/list_identities", json!({})).await;
    let mut lists = vec![
        (
            "GET /console/identities",
            names_of(&route["identities"], "identity"),
        ),
        (
            "meta/console/list_identities",
            names_of(&console_ok(&call)["identities"], "identity"),
        ),
    ];
    if sidebar {
        let experience = console_read(app, subject, "/console/experience").await;
        lists.push((
            "the agent rows of /console/experience",
            names_of(
                &experience["agent_sidebar"]["live_snapshot"]["agents"],
                "identity",
            ),
        ));
    }
    lists
}

/// A Console that serves `rig`'s runtime without holding it as its own
/// runtime, under the access rules `access` (TOML): it names each agent
/// with the runtime's name `served` before it.
fn serving_console(rig: &Rig, access: &str) -> axum::Router {
    let aggregator = meerkat_meta::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime(meerkat_meta::ConsoleRuntimeRegistration {
        runtime_key: "served".to_owned(),
        identity_namespace: "served".to_owned(),
        runtime: Arc::clone(&rig.runtime),
        visibility_policy: Arc::new(meerkat_meta::AllowAllConsoleVisibilityPolicy),
    });
    let config: meerkat_meta::AccessControlConfig = toml::from_str(access).expect("access TOML");
    meerkat_meta::console_json_router_with_aggregator_and_access(
        signed_console_decisions(),
        aggregator,
        Some(meerkat_meta::AccessController::new(config).expect("controller")),
    )
}

/// A deny by role holds for the list record of an agent that has no live
/// member, as it holds for the record of a live one. The Console lists an
/// agent with no live member from what the identity runtime registered of
/// it; the record carries the agent's profile as its role, so a person a
/// deny by role names is not listed such an agent of that role: an agent its
/// host configures that no member was created for, and an agent of the
/// identity store that is owed its seat. A person the rules allow is listed
/// every agent, and the denied person is listed the agents of a role no deny
/// names, with a live member or without. It holds on the runtime's own
/// Console and on a Console that serves the runtime beside others.
#[tokio::test(flavor = "multi_thread")]
async fn a_deny_by_role_holds_for_the_list_record_of_an_agent_with_no_live_member() {
    const AGENTS: [&str; 6] = [
        "plain-live",
        "plain-owed",
        "plain-placed",
        "rev-live",
        "rev-owed",
        "rev-placed",
    ];
    let temp = tempfile::tempdir().unwrap();
    let rig = boot_with(
        temp.path(),
        vec![
            placed(roster_spec("rev-placed", "reviewer", &[])),
            placed(roster_spec("plain-placed", "worker", &[])),
            roster_spec("rev-live", "reviewer", &[]),
            roster_spec("plain-live", "worker", &[]),
        ],
        Some(SCOPED_DENIES),
        |builder| {
            builder.identity_bootstrap_mode(meerkat_meta::IdentityBootstrapMode::LazyMaterialize)
        },
    )
    .await;
    let app = rig.console();
    // The placed agents have no member; the others have a live one.
    for name in ["rev-placed", "plain-placed"] {
        assert_registered_with_no_member(&rig, name).await;
    }
    let roster: Vec<String> = rig
        .runtime
        .mob_handle()
        .list_all_members()
        .await
        .iter()
        .map(|row| row.agent_identity.to_string())
        .collect();
    assert_eq!(
        of_the_row(roster.into_iter(), &AGENTS),
        ["plain-live", "rev-live"],
        "the roster holds the two live members"
    );
    // Two agents of the identity store that stay owed their seat.
    rig.fail_sessions.failing.store(true, Ordering::SeqCst);
    let made = console_rpc(
        &app,
        ROOT,
        "meta/identity/create",
        json!({"request_id": "make-1", "agents": [
            {"name": "rev-owed", "profile": "reviewer", "first_message": "Begin the watch."},
            {"name": "plain-owed", "profile": "worker", "first_message": "Begin the watch."},
        ]}),
    )
    .await;
    for agent in console_ok(&made)["agents"]
        .as_array()
        .expect("the created agents")
    {
        assert_eq!(agent["seat"], json!("owed"), "{made:#}");
    }

    let serving = serving_console(&rig, SCOPED_DENIES);
    for (console, app, sidebar) in [("own", &app, false), ("serving", &serving, true)] {
        // The denied person: no agent of the denied role, live or not.
        for (list, names) in listed_records(app, BOB, &AGENTS, sidebar).await {
            assert_eq!(
                names,
                ["plain-live", "plain-owed", "plain-placed"],
                "{console} Console, {list}"
            );
        }
        // A person the rules allow: every agent.
        for (list, names) in listed_records(app, ANN, &AGENTS, sidebar).await {
            assert_eq!(names, AGENTS, "{console} Console, {list}");
        }
    }
    // The record of an agent with no live member carries its profile as
    // its role, as a live member's record carries its role.
    let listed = console_read(&app, ROOT, "/console/identities").await;
    for (name, role) in [
        ("rev-placed", "reviewer"),
        ("rev-owed", "reviewer"),
        ("plain-placed", "worker"),
        ("rev-live", "reviewer"),
    ] {
        let record = listed["identities"]
            .as_array()
            .expect("the records")
            .iter()
            .find(|record| record["identity"] == json!(name))
            .unwrap_or_else(|| panic!("the record of {name}: {listed:#}"));
        assert_eq!(record["labels"]["role"], json!(role), "{name}: {record:#}");
    }
    rig.shutdown().await;
}
