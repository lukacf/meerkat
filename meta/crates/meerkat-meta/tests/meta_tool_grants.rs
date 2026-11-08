#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Scoped tool grants of the identities a Meta gateway serves
//! (`meta/tool_grants/{grant, revoke, list, audit}`), on the dispatcher the
//! stdio gateway serves its host with.
//!
//! A member's profile gates a tool (`[profiles.*.tools] grant_gated`); Core's
//! lowering mounts it closed on every build of the member, and an identity
//! grant opens it. The grant names the identity the gateway's identity
//! runtime serves: Meta resolves it to Core's member, seats a registered
//! identity Core does not hold through the declared apply's one-identity
//! door (which builds nothing), and keeps no grant state of its own. Every
//! refusal is the typed `tool_grant_request_refused` umbrella.
//!
//! Each row boots the gateway's runtime composition on a temporary state
//! directory (persistent stores, the state directory's identity substrate, a
//! roster provider) with a model that calls the gated tool once per probe
//! turn and reports the tool's result.

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::StopReason;
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, MutableRosterProvider, RosterContext,
    RosterError,
};
use meerkat_mob::machines::mob_machine::MemberMaterializationPhase;
use meerkat_mob::{MobDefinition, ProfileName};
use serde_json::{Value, json};
use tokio::sync::mpsc;

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// The identity every row grants.
const IDENTITY: &str = "agent-a";
/// The tool the identity's profile gates: a built-in that has no side effect.
const GATED_TOOL: &str = "datetime";
/// The marker of a probe turn: such a turn calls the gated tool once.
const PROBE: &str = "TOOL-GRANT-PROBE";
/// Bounds a failure only; a passing row never waits for it.
const PROBE_BOUND: Duration = Duration::from_secs(90);
/// A grant window that outlives every row.
const TEN_MINUTES_MS: u64 = 600_000;

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

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
builtins = true
grant_gated = {{ tools = ["{GATED_TOOL}"] }}
"#
    ))
    .expect("parse the gated definition")
}

fn fresh_mob_id() -> String {
    format!(
        "meta-tool-grants-{}-{}",
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    )
}

fn spec() -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(IDENTITY).unwrap(),
        profile: ProfileName::from("worker"),
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

struct OneIdentityRoster;

#[async_trait]
impl RosterProvider for OneIdentityRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![spec()])
    }
}

/// The members' model: a request whose last message names [`PROBE`] calls
/// the gated tool once, under a fresh call id; a request whose last message
/// carries tool results sends each result's text on the row's channel and
/// answers; any other request answers.
#[derive(Clone)]
struct ProbeClient {
    next_call: Arc<AtomicUsize>,
    results: mpsc::UnboundedSender<String>,
}

impl meerkat_client::LlmClient for ProbeClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let call = match request.messages.last() {
            Some(Message::ToolResults { results, .. }) => {
                for result in results {
                    let _ = self.results.send(result.text_content());
                }
                None
            }
            Some(last)
                if serde_json::to_string(last).is_ok_and(|message| message.contains(PROBE)) =>
            {
                Some(format!(
                    "probe-call-{}",
                    self.next_call.fetch_add(1, Ordering::SeqCst)
                ))
            }
            _ => None,
        };
        let stop = if call.is_some() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        let [usage, done] = llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, stop);
        Box::pin(async_stream::stream! {
            match call {
                Some(id) => {
                    yield Ok(LlmEvent::ToolCallComplete {
                        id,
                        name: GATED_TOOL.to_string(),
                        args: json!({}),
                        meta: None,
                    });
                }
                None => {
                    yield Ok(LlmEvent::TextDelta { delta: "noted".to_string(), meta: None });
                }
            }
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

/// The grant refusal code a probed call's result carries, if any.
fn grant_refusal(text: &str) -> Option<&'static str> {
    [
        meerkat_core::tool_grant::TOOL_GRANT_REQUIRED_CODE,
        meerkat_core::tool_grant::TOOL_GRANT_EXPIRED_CODE,
        meerkat_core::tool_grant::TOOL_GRANT_REVOKED_CODE,
        meerkat_core::tool_grant::TOOL_GRANT_UNAVAILABLE_CODE,
    ]
    .into_iter()
    .find(|code| text.contains(code))
}

/// One booted gateway runtime and the channel its model reports probed
/// results on.
struct Rig {
    runtime: meerkat_meta::UnifiedRuntime,
    probes: mpsc::UnboundedReceiver<String>,
    probe_turns: usize,
}

/// Boot the gateway's runtime composition on `state` for the mob `mob_id`.
async fn boot(state: &Path, mob_id: &str) -> Rig {
    let (results, probes) = mpsc::unbounded_channel();
    let client = ProbeClient {
        next_call: Arc::new(AtomicUsize::new(0)),
        results,
    };
    let builder = UnifiedRuntimeBuilder::default()
        .definition(definition(mob_id))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(OneIdentityRoster))
        .identity_runtime_instance_id("meta-tool-grants")
        .default_llm_client(Arc::new(client));
    let runtime = Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime");
    Rig {
        runtime,
        probes,
        probe_turns: 0,
    }
}

impl Rig {
    /// One JSON-RPC call through the dispatcher the stdio gateway serves.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let ctx = meerkat_meta::rpc::IdentityFirstContext {
            runtime: self
                .runtime
                .identity_runtime()
                .expect("identity runtime installed"),
            roster_provider: Arc::new(MutableRosterProvider::new(vec![spec()])),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };
        let raw = meerkat_meta::handle_unified_rpc_json(
            &self.runtime,
            &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string(),
            Duration::from_mins(1),
            None,
            Some(&ctx),
        )
        .await;
        serde_json::from_str(&raw).expect("json-rpc response")
    }

    /// A grant's answer, which must be a success.
    async fn grant(&self, params: Value) -> Value {
        let response = self.rpc("meta/tool_grants/grant", params).await;
        assert!(
            response["error"].is_null(),
            "the grant is issued: {response:#?}"
        );
        response["result"].clone()
    }

    /// Send the identity a probe turn and answer the gated tool's result
    /// text from that turn.
    async fn probe(&mut self) -> String {
        self.probe_turns += 1;
        let sent = self
            .rpc(
                "meta/send",
                json!({"identity": IDENTITY, "content": format!("{PROBE} {}", self.probe_turns)}),
            )
            .await;
        assert!(sent["error"].is_null(), "the probe is admitted: {sent:#?}");
        tokio::time::timeout(PROBE_BOUND, self.probes.recv())
            .await
            .expect("the probe turn calls the gated tool")
            .expect("the probe channel stays open")
    }

    fn materialization(&self) -> Option<MemberMaterializationPhase> {
        let member = meerkat_meta::member_comms_id::roster_member_id_for_identity(IDENTITY);
        self.runtime
            .mob_handle()
            .member_materialization_for_test(&member)
    }

    async fn shutdown(self) {
        let _ = self.runtime.shutdown().await;
    }
}

fn tool_target() -> Value {
    json!({"kind": "tool", "name": GATED_TOOL})
}

/// The typed grant request refusal of `response`: invalid params with the
/// `tool_grant_request_refused` umbrella; answers the nested kind.
fn refusal_kind(response: &Value) -> String {
    assert_eq!(
        response["error"]["code"],
        json!(-32602),
        "a grant request refusal is invalid params: {response:#?}"
    );
    let data = &response["error"]["data"];
    assert_eq!(
        data["kind"],
        json!("tool_grant_request_refused"),
        "the refusal is the typed umbrella: {response:#?}"
    );
    data["tool_grant_request_refusal"]["kind"]
        .as_str()
        .unwrap_or_else(|| panic!("the umbrella names its kind: {response:#?}"))
        .to_string()
}

/// A member whose profile gates a tool has it closed on Meta: a call is
/// refused `tool_grant_required`. An identity grant opens it at the next
/// turn, with the host's `decision_ref` recorded in its issuer; list and
/// audit show the grant; the revoke closes it again at once.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_grant_opens_a_gated_tool_of_a_meta_member_and_its_revoke_closes_it() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;

    let closed = rig.probe().await;
    assert_eq!(
        grant_refusal(&closed),
        Some("tool_grant_required"),
        "the gated tool is closed before a grant: {closed}"
    );

    let granted = rig
        .grant(json!({
            "identity": IDENTITY,
            "target": tool_target(),
            "duration_ms": TEN_MINUTES_MS,
            "reason": "the host approved it",
            "decision_ref": "approval-7",
        }))
        .await;
    let grant = &granted["grant"];
    assert_eq!(grant["status"], json!("active"), "{granted:#?}");
    assert_eq!(grant["subject"]["kind"], json!("identity"), "{granted:#?}");
    assert_eq!(
        grant["subject"]["identity"],
        json!(IDENTITY),
        "{granted:#?}"
    );
    assert_eq!(
        grant["issuer"],
        json!({"kind": "operator", "decision_ref": "approval-7"}),
        "the stdio host's grant names the host's decision: {granted:#?}"
    );
    let grant_id = grant["grant_id"].as_str().expect("grant id").to_string();

    let open = rig.probe().await;
    assert_eq!(
        grant_refusal(&open),
        None,
        "the grant opens the tool at the next turn: {open}"
    );

    let listed = rig
        .rpc("meta/tool_grants/list", json!({"identity": IDENTITY}))
        .await;
    assert!(listed["error"].is_null(), "{listed:#?}");
    let grants = listed["result"]["grants"].as_array().expect("grants");
    assert_eq!(grants.len(), 1, "{listed:#?}");
    assert_eq!(grants[0]["grant_id"], json!(grant_id), "{listed:#?}");

    let audit = rig
        .rpc("meta/tool_grants/audit", json!({"identity": IDENTITY}))
        .await;
    assert!(audit["error"].is_null(), "{audit:#?}");
    let granted_row = audit["result"]["rows"]
        .as_array()
        .expect("audit rows")
        .iter()
        .find(|row| row["event"]["kind"] == json!("granted"))
        .unwrap_or_else(|| panic!("the audit has the granted row: {audit:#?}"))
        .clone();
    assert_eq!(
        granted_row["event"]["issuer"]["decision_ref"],
        json!("approval-7"),
        "{audit:#?}"
    );

    let revoked = rig
        .rpc(
            "meta/tool_grants/revoke",
            json!({"grant_id": grant_id, "reason": "done"}),
        )
        .await;
    assert!(revoked["error"].is_null(), "{revoked:#?}");
    assert_eq!(revoked["result"]["changed"], json!(true), "{revoked:#?}");
    assert_eq!(
        revoked["result"]["grant"]["status"],
        json!("revoked"),
        "{revoked:#?}"
    );

    let closed_again = rig.probe().await;
    assert_eq!(
        grant_refusal(&closed_again),
        Some("tool_grant_revoked"),
        "the revoke closes the tool: {closed_again}"
    );
    let ended = rig
        .rpc(
            "meta/tool_grants/list",
            json!({"identity": IDENTITY, "include_ended": true}),
        )
        .await;
    assert_eq!(
        ended["result"]["grants"][0]["status"],
        json!("revoked"),
        "{ended:#?}"
    );
    rig.shutdown().await;
}

/// A registered identity Core does not hold (its member was retired) is
/// granted: the grant seats it through the declared apply's one-identity
/// door, Dormant, and wakes nothing; the member's first build after that
/// opens the granted tool.
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_to_an_identity_seated_by_the_door_applies_at_its_first_build() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;

    let retired = rig.rpc("meta/retire", json!({"identity": IDENTITY})).await;
    assert!(retired["error"].is_null(), "{retired:#?}");
    assert_eq!(
        rig.materialization(),
        None,
        "Core holds no member of the retired identity"
    );

    let granted = rig
        .grant(json!({
            "identity": IDENTITY,
            "target": tool_target(),
            "duration_ms": TEN_MINUTES_MS,
        }))
        .await;
    assert_eq!(granted["grant"]["status"], json!("active"), "{granted:#?}");
    assert_eq!(
        rig.materialization(),
        Some(MemberMaterializationPhase::Dormant),
        "the grant seated the identity Dormant and woke nothing"
    );

    let open = rig.probe().await;
    assert_eq!(
        grant_refusal(&open),
        None,
        "the member's first build after the seat opens the granted tool: {open}"
    );
    rig.shutdown().await;
}

/// An identity grant lives in the runtime's store, keyed by the mob's
/// incarnation: it survives a restart of the gateway over the same state
/// directory, and the resumed member's build opens it.
#[tokio::test(flavor = "multi_thread")]
async fn identity_grants_survive_a_gateway_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let rig = boot(temp.path(), &mob_id).await;
    let granted = rig
        .grant(json!({
            "identity": IDENTITY,
            "target": tool_target(),
            "duration_ms": TEN_MINUTES_MS,
        }))
        .await;
    let grant_id = granted["grant"]["grant_id"].clone();
    rig.shutdown().await;

    let mut rig = boot(temp.path(), &mob_id).await;
    let listed = rig
        .rpc("meta/tool_grants/list", json!({"identity": IDENTITY}))
        .await;
    assert!(listed["error"].is_null(), "{listed:#?}");
    assert_eq!(
        listed["result"]["grants"][0]["grant_id"], grant_id,
        "the grant survives the restart: {listed:#?}"
    );
    let open = rig.probe().await;
    assert_eq!(
        grant_refusal(&open),
        None,
        "the resumed member's build opens the grant: {open}"
    );
    rig.shutdown().await;
}

/// Every refusal of the grant methods is invalid params carrying the typed
/// `tool_grant_request_refused` umbrella.
#[tokio::test(flavor = "multi_thread")]
async fn meta_tool_grant_refusals_are_typed() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;

    let unknown = rig
        .rpc(
            "meta/tool_grants/grant",
            json!({"identity": "nobody", "target": tool_target(), "duration_ms": TEN_MINUTES_MS}),
        )
        .await;
    assert_eq!(refusal_kind(&unknown), "subject_unknown");

    let not_gated = rig
        .rpc(
            "meta/tool_grants/grant",
            json!({
                "identity": IDENTITY,
                "target": {"kind": "tool", "name": "view_image"},
                "duration_ms": TEN_MINUTES_MS,
            }),
        )
        .await;
    assert_eq!(refusal_kind(&not_gated), "target_not_gated");

    let family_not_gated = rig
        .rpc(
            "meta/tool_grants/grant",
            json!({
                "identity": IDENTITY,
                "target": {"kind": "family", "family": "shell"},
                "duration_ms": TEN_MINUTES_MS,
            }),
        )
        .await;
    assert_eq!(refusal_kind(&family_not_gated), "target_not_gated");

    let empty = rig
        .rpc(
            "meta/tool_grants/grant",
            json!({
                "identity": IDENTITY,
                "target": {"kind": "tool", "name": ""},
                "duration_ms": TEN_MINUTES_MS,
            }),
        )
        .await;
    assert_eq!(refusal_kind(&empty), "empty_target");

    for window in [
        json!({}),
        json!({"duration_ms": TEN_MINUTES_MS, "expires_at_ms": 4_102_444_800_000_u64}),
        // Longer than the default maximum lifetime (24 h).
        json!({"duration_ms": 25 * 3_600_000_u64}),
    ] {
        let mut params = json!({"identity": IDENTITY, "target": tool_target()});
        for (key, value) in window.as_object().unwrap() {
            params[key] = value.clone();
        }
        let refused = rig.rpc("meta/tool_grants/grant", params).await;
        assert_eq!(refusal_kind(&refused), "invalid_window", "{window}");
    }

    for decision_ref in ["", "approval\u{7}7"] {
        let refused = rig
            .rpc(
                "meta/tool_grants/grant",
                json!({
                    "identity": IDENTITY,
                    "target": tool_target(),
                    "duration_ms": TEN_MINUTES_MS,
                    "decision_ref": decision_ref,
                }),
            )
            .await;
        assert_eq!(refusal_kind(&refused), "invalid_decision_ref");
    }

    for grant_id in ["not-a-grant-id", "00000000-0000-4000-8000-000000000000"] {
        let refused = rig
            .rpc("meta/tool_grants/revoke", json!({"grant_id": grant_id}))
            .await;
        assert_eq!(refusal_kind(&refused), "not_found", "{grant_id}");
        assert_eq!(
            refused["error"]["data"]["tool_grant_request_refusal"]["grant_id"],
            json!(grant_id)
        );
    }

    let page = rig
        .rpc(
            "meta/tool_grants/audit",
            json!({"identity": IDENTITY, "limit": 0}),
        )
        .await;
    assert_eq!(refusal_kind(&page), "invalid_page_limit");
    rig.shutdown().await;
}

/// An identity created at runtime is store-owned (its record is its row in
/// the mob's identity store) and Meta's verbs do not act on it yet: a grant
/// to it is `subject_unknown`, although Core's roster holds its member.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_owned_identity_is_subject_unknown_until_store_owned_identities_are_grantable() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let handle = rig.runtime.mob_handle();
    let created = handle
        .create_store_owned_identities(
            meerkat_mob::StoreOwnedIdentityCreate::new(
                meerkat_mob::IdentityOperation::new(
                    "create-keeper",
                    meerkat_core::approval::decision::Sha256Digest::of(b"create-keeper").as_str(),
                ),
                meerkat_mob::IdentityChangeRequesterInput::host("console"),
                vec![meerkat_mob::CreatedIdentityRequest::new(
                    meerkat_mob::CreatedIdentitySpec::new("keeper", "worker"),
                )],
            ),
            meerkat_mob::IdentityChangeConsentInput::Policy(
                meerkat_mob::IdentityPolicyOutcome::Allow,
            ),
        )
        .await
        .map(|created| created.change);
    assert!(
        matches!(
            created,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "{created:?}"
    );
    let keeper = meerkat_mob::AgentIdentity::from("keeper");
    match handle.identity_entry(&keeper) {
        Ok(meerkat_mob::IdentityEntryRead::Entry(entry)) => assert_eq!(
            entry.origin,
            meerkat_mob::IdentityEntryOrigin::Created,
            "the created identity is store-owned"
        ),
        other => panic!("the created identity is an entry: {other:?}"),
    }

    let refused = rig
        .rpc(
            "meta/tool_grants/grant",
            json!({"identity": "keeper", "target": tool_target(), "duration_ms": TEN_MINUTES_MS}),
        )
        .await;
    assert_eq!(refusal_kind(&refused), "subject_unknown");
    rig.shutdown().await;
}
