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
    runtime: UnifiedRuntime,
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
        runtime,
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

/// A create of one agent that names `peer` as its peer.
fn create_near(peer: &str, tag: &str) -> Value {
    json!({"request_id": format!("near-{tag}"),
           "agents": [{"name": format!("near-{tag}"), "profile": "worker", "peers": [peer]}]})
}

/// An access rule that denies a person the agents of a profile, or the
/// agents that carry a label, holds for an agent no member was ever seated
/// for: an agent whose first message did not start is committed and owed
/// its seat, and it is decided on its own record, as a seated agent is on
/// its seat. The denied person's get, plan and every change of it answer
/// exactly as for a name no agent holds, it is not in their list, and it
/// is no peer of a create of theirs. A person the rules allow reads it, and
/// the denied person reads an agent owed its seat that no deny names.
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

    let absent = console_get(&app, BOB, "nobody").await;
    let absent = console_refused(&absent).clone();
    assert_eq!(
        absent["data"]["kind"],
        json!("identity_unknown"),
        "{absent:#}"
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

        // The denied person's get answers as for a name no agent holds.
        let got = console_get(&app, BOB, name).await;
        assert_eq!(
            console_refused(&got),
            &absent,
            "{name}: a get of it answers as for a name no agent holds"
        );

        // So do a plan and every change of it.
        for ((method, hidden), (_, unheld)) in changes_of(name, &digest, name)
            .into_iter()
            .zip(changes_of("nobody", &digest, &format!("{name}-absent")))
        {
            let refused = console_rpc(&app, BOB, method, hidden).await;
            let unheld = console_rpc(&app, BOB, method, unheld).await;
            assert_eq!(
                console_refused(&refused),
                console_refused(&unheld),
                "{name}: {method} answers as for a name no agent holds"
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
/// agent a deny names answer exactly as for a name no agent holds, nothing
/// changes it, it is not in their list and it is no peer of a create of
/// theirs. A person the rules allow reads it, and the denied person reads
/// an archived agent no deny names.
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

    let absent = console_get(&app, BOB, "nobody").await;
    let absent = console_refused(&absent).clone();
    assert_eq!(
        absent["data"]["kind"],
        json!("identity_unknown"),
        "{absent:#}"
    );
    for name in ["rev-archived", "sealed-archived"] {
        let digest = digests[name].clone();
        // A person the rules allow reads it.
        let allowed = console_get(&app, ANN, name).await;
        assert_eq!(console_ok(&allowed)["archived"], json!(true), "{allowed:#}");

        // The denied person's get answers as for a name no agent holds.
        let got = console_get(&app, BOB, name).await;
        assert_eq!(
            console_refused(&got),
            &absent,
            "{name}: a get of it answers as for a name no agent holds"
        );

        // So do a plan and every change of it.
        for ((method, hidden), (_, unheld)) in changes_of(name, &digest, name)
            .into_iter()
            .zip(changes_of("nobody", &digest, &format!("{name}-absent")))
        {
            let refused = console_rpc(&app, BOB, method, hidden).await;
            let unheld = console_rpc(&app, BOB, method, unheld).await;
            assert_eq!(
                console_refused(&refused),
                console_refused(&unheld),
                "{name}: {method} answers as for a name no agent holds"
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
