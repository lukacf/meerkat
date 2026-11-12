#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::too_many_lines
)]
//! Ordinary peer sends between members of two isolated mobs composed in one
//! process and wired with the local-side RPC calls, in the order the Python
//! SDK helper `MobHandle.wire_cross_mob` makes them: `meta/cross_mob/peer_info`
//! on both sides, then `meta/cross_mob/wire_local` on each side, and
//! `meta/cross_mob/unwire_local` on the first side when the second refuses.
//!
//! Every request goes through `handle_unified_rpc_json`, the stdio gateway's
//! dispatcher. No test here writes a route a send needs: those are the ones
//! the wire calls themselves leave behind. (One row puts a stale route into
//! the registry on purpose, as the thing a wire must replace.)
//!
//! The sending member is driven by a scripted model that calls the agent
//! `send_message` tool with the other member's canonical peer id. A delivery
//! is what the receiving mob's model is handed on the receiver's next turn.

use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::stream;
use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_comms::InprocRegistry;
use meerkat_core::types::HandlingMode;
use meerkat_core::{Message, Provider, StopReason};
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, TopologyBootstrapConfig,
    TopologyControlPolicy, UnifiedRuntime, decode_pubkey_b64, handle_unified_rpc_json,
};
use meerkat_mob::{
    MobDefinition, MobRuntimeMode, MobStorage, SpawnMemberSpec, ids::AgentIdentity as MeerkatId,
};
use serde_json::{Value, json};

#[path = "support/llm_usage.rs"]
mod llm_usage;

const SEND_TRIGGER: &str = "SCRIPTED-SEND peer_id=";
const NOT_ROUTABLE: &str = "peer_not_found_or_not_trusted";

/// What a mob's model double was handed for one model call.
struct Observation {
    /// The call follows a tool call of this turn (the `send_message` result).
    after_tool: bool,
    replay: String,
    last: String,
}

/// One model double per mob. A user message `SCRIPTED-SEND peer_id=<id>
/// body=<text>` makes it call `send_message` once; every other call is
/// recorded and answered with plain text.
struct PeerScriptClient {
    /// The bodies already sent, for the whole test process: a member built
    /// again over its state still has its earlier `SCRIPTED-SEND` message
    /// in its history, and must not send it a second time.
    sent: Arc<Mutex<HashSet<String>>>,
    observations: tokio::sync::mpsc::UnboundedSender<Observation>,
}

fn sent_bodies() -> Arc<Mutex<HashSet<String>>> {
    static SENT: std::sync::OnceLock<Arc<Mutex<HashSet<String>>>> = std::sync::OnceLock::new();
    Arc::clone(SENT.get_or_init(|| Arc::new(Mutex::new(HashSet::new()))))
}

fn scripted_send(request: &LlmRequest) -> Option<(String, String)> {
    request.messages.iter().rev().find_map(|message| {
        let Message::User(user) = message else {
            return None;
        };
        let text = user.text_content();
        let rest = text.split_once(SEND_TRIGGER)?.1;
        let (peer_id, rest) = rest.split_once(" body=")?;
        let body = rest.lines().next().unwrap_or_default();
        Some((peer_id.to_string(), body.to_string()))
    })
}

impl LlmClient for PeerScriptClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let provider = LlmClient::provider(self);
        if matches!(
            request.messages.last(),
            Some(Message::User(user)) if user.text_content().contains("You have been spawned as")
        ) {
            let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::EndTurn);
            return Box::pin(stream::iter(vec![Ok(usage), Ok(done)]));
        }
        if let Some((peer_id, body)) = scripted_send(request)
            && self.sent.lock().expect("sent bodies").insert(body.clone())
        {
            let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::ToolUse);
            return Box::pin(stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: format!("scripted-send-{}", body.replace(' ', "-")),
                    name: "send_message".to_string(),
                    args: json!({
                        "peer_id": peer_id,
                        "body": body,
                        "handling_mode": "queue",
                    }),
                    meta: None,
                }),
                Ok(usage),
                Ok(done),
            ]));
        }
        let _ = self.observations.send(Observation {
            after_tool: matches!(request.messages.last(), Some(Message::ToolResults { .. })),
            replay: serde_json::to_string(&request.messages).unwrap_or_default(),
            last: request
                .messages
                .last()
                .map(|message| serde_json::to_string(message).unwrap_or_default())
                .unwrap_or_default(),
        });
        let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::EndTurn);
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "observed".to_string(),
                meta: None,
            }),
            Ok(usage),
            Ok(done),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Other
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// One mob with one member, and what its model double was handed.
struct Mob {
    runtime: UnifiedRuntime,
    mob_id: String,
    member: String,
    received: tokio::sync::mpsc::UnboundedReceiver<Observation>,
    unmatched: Vec<Observation>,
}

/// A member's `meta/cross_mob/peer_info` answer, and the descriptor a caller
/// wires from it.
#[derive(Clone)]
struct PeerInfo {
    comms_name: String,
    peer_id: String,
    /// The address a wire names: the answer's `address` until
    /// [`PeerInfo::by_socket`] replaces it.
    address: String,
    /// The key a wire names (`remote_pubkey_b64`): none until
    /// [`PeerInfo::by_socket`] sets it.
    key: Option<String>,
    /// The answer's `pubkey_b64`.
    pubkey_b64: Option<String>,
    /// The answer's `dialable_address`.
    dialable_address: Option<String>,
    /// The answer as it came.
    answer: Value,
}

impl PeerInfo {
    /// The member by its socket endpoint and key, as a caller in another
    /// process must describe it.
    fn by_socket(&self) -> Self {
        Self {
            address: self
                .dialable_address
                .clone()
                .expect("peer_info reports a dialable address"),
            key: Some(
                self.pubkey_b64
                    .clone()
                    .expect("peer_info reports the member's key"),
            ),
            ..self.clone()
        }
    }
}

/// Member comms with a socket endpoint on a loopback port the kernel picks.
fn socket_member_comms() -> Config {
    let mut config = Config::default();
    config.comms.mode = meerkat_core::CommsRuntimeMode::Tcp;
    config.comms.address = Some("127.0.0.1:0".to_string());
    config
}

impl Mob {
    /// A mob whose member has in-process comms only.
    async fn start(root: &Path, mob_id: &str, member: &str) -> Self {
        Self::start_with(root, mob_id, member, Config::default()).await
    }

    async fn start_with(root: &Path, mob_id: &str, member: &str, config: Config) -> Self {
        let session_path = root.join(format!("{mob_id}-sessions"));
        Self::start_at(root, &session_path, mob_id, member, config).await
    }

    /// A mob whose sessions live under `session_path`. Socket member comms
    /// keep their transport key there, so two mobs started on one path have
    /// members with the same key.
    async fn start_at(
        root: &Path,
        session_path: &Path,
        mob_id: &str,
        member: &str,
        config: Config,
    ) -> Self {
        std::fs::create_dir_all(session_path).expect("session path");
        let factory = AgentFactory::new(session_path).comms(true);
        let session_service = Arc::new(build_ephemeral_service(factory, config, 8));
        let definition = MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
        ))
        .expect("definition");
        let (observations, received) = tokio::sync::mpsc::unbounded_channel();
        let client: Arc<dyn LlmClient> = Arc::new(PeerScriptClient {
            sent: sent_bodies(),
            observations,
        });
        let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
            .with_options(MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(client),
            });
        let runtime = UnifiedRuntime::bootstrap_with_topology(
            mob_spec,
            MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: format!("wire-local-{mob_id}"),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            Duration::from_secs(2),
            TopologyBootstrapConfig {
                policy: TopologyControlPolicy::default(),
                state_path: Some(root.join(format!("{mob_id}-topology.json"))),
            },
        )
        .await
        .expect("bootstrap runtime");
        runtime
            .spawn(
                SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    MeerkatId::from(member).to_string(),
                    None,
                    Some(MobRuntimeMode::TurnDriven),
                    None,
                )
                .with_additional_instructions(vec![format!("You are {member}.")]),
            )
            .await
            .expect("spawn member");
        Self {
            runtime,
            mob_id: mob_id.to_string(),
            member: member.to_string(),
            received,
            unmatched: Vec::new(),
        }
    }

    /// One JSON-RPC request through the stdio gateway's dispatcher: the
    /// result, or the error message.
    async fn rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let response = handle_unified_rpc_json(
            &self.runtime,
            &request.to_string(),
            Duration::from_secs(10),
            None,
            None,
        )
        .await;
        let response: Value = serde_json::from_str(&response).expect("JSON-RPC response");
        match response.get("error") {
            Some(error) if !error.is_null() => Err(error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()),
            _ => Ok(response.get("result").cloned().unwrap_or(Value::Null)),
        }
    }

    async fn peer_info(&self) -> PeerInfo {
        let info = self
            .rpc(
                "meta/cross_mob/peer_info",
                json!({"member_id": self.member}),
            )
            .await
            .expect("peer_info");
        let field = |name: &str| {
            info.get(name)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("peer_info has no {name}: {info}"))
                .to_string()
        };
        let optional = |name: &str| info.get(name).and_then(Value::as_str).map(str::to_string);
        PeerInfo {
            comms_name: field("comms_name"),
            peer_id: field("peer_id"),
            address: field("address"),
            key: None,
            pubkey_b64: optional("pubkey_b64"),
            dialable_address: optional("dialable_address"),
            answer: info.clone(),
        }
    }

    fn wire_params(&self, remote: &PeerInfo) -> Value {
        let mut params = json!({
            "local_member_id": self.member,
            "remote_comms_name": remote.comms_name,
            "remote_peer_id": remote.peer_id,
            "remote_address": remote.address,
        });
        if let Some(key) = &remote.key {
            params["remote_pubkey_b64"] = json!(key);
        }
        params
    }

    async fn wire_local(&self, remote: &PeerInfo) -> Result<Value, String> {
        self.rpc("meta/cross_mob/wire_local", self.wire_params(remote))
            .await
    }

    async fn unwire_local(&self, remote: &PeerInfo) -> Result<Value, String> {
        self.rpc("meta/cross_mob/unwire_local", self.wire_params(remote))
            .await
    }

    fn namespace(&self) -> String {
        meerkat_core::mob_realm_id(&self.mob_id)
            .expect("mob namespace")
            .as_str()
            .to_string()
    }

    /// The member's key as this process registers it.
    fn registered_key(&self, name: &str) -> [u8; 32] {
        *InprocRegistry::global()
            .peers_in_namespace(&self.namespace())
            .into_iter()
            .find(|peer| peer.name == name)
            .unwrap_or_else(|| panic!("{name} is not registered in its own namespace"))
            .pubkey
            .as_bytes()
    }

    /// The peer id this mob's namespace routes `name` to, if it routes it.
    fn route_to(&self, name: &str) -> Option<String> {
        InprocRegistry::global()
            .peers_in_namespace(&self.namespace())
            .into_iter()
            .find(|peer| peer.name == name)
            .map(|peer| peer.pubkey.to_peer_id().to_string())
    }

    /// The comms names the member trusts.
    async fn trusts(&self) -> Vec<String> {
        self.runtime
            .mob_handle()
            .get_member(&MeerkatId::from(self.member.as_str()))
            .await
            .expect("member lookup")
            .expect("member")
            .wired_to
            .iter()
            .map(|peer| peer.as_str().to_string())
            .collect()
    }

    async fn observation(
        &mut self,
        wait: Duration,
        matches: impl Fn(&Observation) -> bool,
    ) -> Option<Observation> {
        if let Some(index) = self.unmatched.iter().position(&matches) {
            return Some(self.unmatched.remove(index));
        }
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match tokio::time::timeout_at(deadline, self.received.recv()).await {
                Ok(Some(observation)) if matches(&observation) => return Some(observation),
                Ok(Some(observation)) => self.unmatched.push(observation),
                Ok(None) => panic!("{}: observation channel closed", self.mob_id),
                Err(_) => return None,
            }
        }
    }

    /// The member calls `send_message` to `peer_id`; the tool result the
    /// member's model was handed.
    async fn send(&mut self, peer_id: &str, body: &str) -> String {
        self.runtime
            .mob_handle()
            .member(&MeerkatId::from(self.member.as_str()))
            .await
            .expect("member handle")
            .send(
                format!("{SEND_TRIGGER}{peer_id} body={body}"),
                HandlingMode::Queue,
            )
            .await
            .expect("trigger the sending turn");
        self.observation(Duration::from_secs(20), |observation| {
            observation.after_tool
        })
        .await
        .unwrap_or_else(|| panic!("{}: no send_message result for {body:?}", self.mob_id))
        .last
    }

    /// Whether the member's model is handed a peer message carrying `body`.
    async fn receives(&mut self, body: &str, wait: Duration) -> bool {
        self.observation(wait, |observation| {
            !observation.after_tool && observation.replay.contains(body)
        })
        .await
        .is_some()
    }

    async fn stop(self) {
        let report = self.runtime.shutdown().await;
        assert!(
            report.mob_stop.is_ok(),
            "{} stop: {:?}",
            self.mob_id,
            report.mob_stop
        );
    }
}

/// The Python SDK helper `MobHandle.wire_cross_mob`, call for call, from the
/// two `peer_info` answers on: one read of the local member (is it wired to
/// that peer already), `wire_local` on each side, and the undo of the first
/// side when the second refuses, unless the first side was wired before.
async fn sdk_wire_with(
    local: &Mob,
    local_info: &PeerInfo,
    remote: &Mob,
    remote_info: &PeerInfo,
) -> Result<(), String> {
    let wired_before = local.trusts().await.contains(&remote_info.comms_name);
    local.wire_local(remote_info).await?;
    if let Err(error) = remote.wire_local(local_info).await {
        if !wired_before {
            let _ = local.unwire_local(remote_info).await;
        }
        return Err(error);
    }
    Ok(())
}

/// The helper from its start: the two `peer_info` calls, then each side by
/// its socket endpoint and key when BOTH answers report a dialable address,
/// and by the in-process address with no key otherwise.
async fn sdk_wire_cross_mob(local: &Mob, remote: &Mob) -> Result<(), String> {
    let local_info = local.peer_info().await;
    let remote_info = remote.peer_info().await;
    if local_info.dialable_address.is_some() && remote_info.dialable_address.is_some() {
        return sdk_wire_with(
            local,
            &local_info.by_socket(),
            remote,
            &remote_info.by_socket(),
        )
        .await;
    }
    sdk_wire_with(local, &local_info, remote, &remote_info).await
}

const DELIVERY_WAIT: Duration = Duration::from_secs(20);
const NO_DELIVERY_WAIT: Duration = Duration::from_millis(1500);

async fn assert_delivers(sender: &mut Mob, recipient: &mut Mob, body: &str) {
    let recipient_peer_id = recipient.peer_info().await.peer_id;
    let result = sender.send(&recipient_peer_id, body).await;
    assert!(
        !result.contains(NOT_ROUTABLE),
        "{} -> {}: send_message was refused: {result}",
        sender.mob_id,
        recipient.mob_id
    );
    assert!(
        recipient.receives(body, DELIVERY_WAIT).await,
        "{} -> {}: the recipient was never handed {body:?}; send_message answered {result}",
        sender.mob_id,
        recipient.mob_id
    );
}

async fn assert_refused(sender: &mut Mob, recipient: &mut Mob, body: &str) {
    let recipient_peer_id = recipient.peer_info().await.peer_id;
    let result = sender.send(&recipient_peer_id, body).await;
    assert!(
        result.contains(NOT_ROUTABLE),
        "{} -> {}: an unwired member's send_message must be refused: {result}",
        sender.mob_id,
        recipient.mob_id
    );
    assert!(
        !recipient.receives(body, NO_DELIVERY_WAIT).await,
        "{} -> {}: an unwired member's message was delivered",
        sender.mob_id,
        recipient.mob_id
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_wire_sequence_delivers_peer_sends_both_ways() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-both-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-both-b", "bob").await;

    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");

    assert_delivers(&mut left, &mut right, "both-ways a-to-b").await;
    assert_delivers(&mut right, &mut left, "both-ways b-to-a").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_local_with_the_members_key_supplied_delivers_both_ways() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-keyed-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-keyed-b", "bob").await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;

    // The library call under the RPC, with each member's own key supplied.
    left.runtime
        .wire_local(
            "alice",
            &bob.comms_name,
            &bob.peer_id,
            &bob.address,
            Some(right.registered_key(&bob.comms_name)),
        )
        .await
        .expect("wire alice to bob");
    right
        .runtime
        .wire_local(
            "bob",
            &alice.comms_name,
            &alice.peer_id,
            &alice.address,
            Some(left.registered_key(&alice.comms_name)),
        )
        .await
        .expect("wire bob to alice");

    assert_delivers(&mut left, &mut right, "keyed a-to-b").await;
    assert_delivers(&mut right, &mut left, "keyed b-to-a").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_wire_sequence_keeps_an_unwired_third_mob_out() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-third-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-third-b", "bob").await;
    let mut third = Mob::start(root.path(), "wire-third-c", "carol").await;

    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");
    // The wired pair delivers, so the refusals below are not a dead edge.
    assert_delivers(&mut left, &mut right, "third a-to-b").await;

    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    let carol = third.peer_info().await;
    assert_refused(&mut third, &mut right, "third c-to-b").await;
    assert_refused(&mut third, &mut left, "third c-to-a").await;
    assert_refused(&mut left, &mut third, "third a-to-c").await;
    assert_refused(&mut right, &mut third, "third b-to-c").await;
    for name in [&alice.comms_name, &bob.comms_name] {
        assert_eq!(
            third.route_to(name),
            None,
            "the unwired mob's namespace gained a route to {name}"
        );
    }
    assert_eq!(left.route_to(&carol.comms_name), None);
    assert_eq!(right.route_to(&carol.comms_name), None);
    assert!(third.trusts().await.is_empty());

    left.stop().await;
    right.stop().await;
    third.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_local_refuses_a_peer_id_the_named_live_member_does_not_have() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-bound-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-bound-b", "bob").await;
    let bob = right.peer_info().await;
    // Same comms name, another peer id: the name is a live member of this
    // process, and it is not that peer.
    let impostor = PeerInfo {
        peer_id: "00000000-0000-4000-8000-0000000000aa".to_string(),
        ..bob.clone()
    };

    // As a FIRST wire: refused before anything is installed.
    let refusal = left
        .wire_local(&impostor)
        .await
        .expect_err("a peer id the named live member does not have must be refused");
    assert!(
        refusal.contains("under another peer id or key"),
        "the refusal must come from the key binding check: {refusal}"
    );
    assert_eq!(
        left.route_to(&bob.comms_name),
        None,
        "a refused first wire left a route"
    );
    assert!(left.trusts().await.is_empty());

    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");

    // And over an existing wiring.
    let refusal = left
        .wire_local(&impostor)
        .await
        .expect_err("a peer id the named live member does not have must be refused");
    assert!(
        refusal.contains("cross_mob/wire_local failed"),
        "unexpected refusal: {refusal}"
    );

    // Nothing was displaced: the route and the trust are the first wiring's.
    assert_eq!(left.route_to(&bob.comms_name), Some(bob.peer_id.clone()));
    assert_eq!(left.trusts().await, vec![bob.comms_name.clone()]);
    assert_delivers(&mut left, &mut right, "bound a-to-b").await;
    assert_delivers(&mut right, &mut left, "bound b-to-a").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_wire_rollback_leaves_no_trust_and_no_route() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-undo-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-undo-b", "bob").await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;

    // The first side alone: trust and route arrive together.
    left.wire_local(&bob).await.expect("first side");
    assert_eq!(left.trusts().await, vec![bob.comms_name.clone()]);
    assert_eq!(
        left.route_to(&bob.comms_name),
        Some(bob.peer_id.clone()),
        "wire_local left trust without a route"
    );
    left.unwire_local(&bob).await.expect("undo the first side");
    assert!(left.trusts().await.is_empty());
    assert_eq!(left.route_to(&bob.comms_name), None);

    // The helper's sequence with a second side that refuses: the first
    // member's peer id is no longer the one its `peer_info` answered.
    let stale_alice = PeerInfo {
        peer_id: "00000000-0000-4000-8000-0000000000bb".to_string(),
        ..alice.clone()
    };
    sdk_wire_with(&left, &stale_alice, &right, &bob)
        .await
        .expect_err("the second side must refuse");
    assert!(left.trusts().await.is_empty(), "trust without route");
    assert!(right.trusts().await.is_empty(), "trust without route");
    assert_eq!(left.route_to(&bob.comms_name), None, "route without trust");
    assert_eq!(
        right.route_to(&alice.comms_name),
        None,
        "route without trust"
    );

    // Nothing is left in the way of a clean wiring.
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence after a rollback");
    assert_delivers(&mut left, &mut right, "undo a-to-b").await;
    assert_delivers(&mut right, &mut left, "undo b-to-a").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_info_answers_the_members_key_and_omits_a_dialable_address_it_does_not_have() {
    let root = tempfile::tempdir().expect("root");
    let plain = Mob::start(root.path(), "wire-info-a", "alice").await;
    let socket = Mob::start_with(root.path(), "wire-info-b", "bob", socket_member_comms()).await;

    let alice = plain.peer_info().await;
    let key = alice.pubkey_b64.clone().expect("the member's key");
    assert_eq!(
        decode_pubkey_b64(&key).expect("standard base64 of 32 bytes"),
        plain.registered_key(&alice.comms_name),
        "pubkey_b64 is the member's transport key"
    );
    // A member with in-process comms only has no dialable address: the
    // field is left out, never null.
    assert!(
        alice.answer.get("dialable_address").is_none(),
        "peer_info: {}",
        alice.answer
    );
    assert!(alice.address.starts_with("inproc://"));

    let bob = socket.peer_info().await;
    let dialable = bob.dialable_address.clone().expect("a socket endpoint");
    assert!(
        dialable.starts_with("tcp://127.0.0.1:") && !dialable.ends_with(":0"),
        "the bound endpoint, not the configured port 0: {dialable}"
    );
    assert!(bob.pubkey_b64.is_some());

    plain.stop().await;
    socket.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_wire_sequence_over_member_socket_endpoints_delivers_both_ways() {
    let root = tempfile::tempdir().expect("root");
    let mut left =
        Mob::start_with(root.path(), "wire-socket-a", "alice", socket_member_comms()).await;
    let mut right =
        Mob::start_with(root.path(), "wire-socket-b", "bob", socket_member_comms()).await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;

    // What two gateways in two processes do: each side names the other by
    // the socket endpoint and key its peer_info reports.
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");
    // No in-process route was installed: the sends below cross the members'
    // socket endpoints.
    assert_eq!(left.route_to(&bob.comms_name), None);
    assert_eq!(right.route_to(&alice.comms_name), None);

    assert_delivers(&mut left, &mut right, "socket a-to-b").await;
    assert_delivers(&mut right, &mut left, "socket b-to-a").await;

    left.unwire_local(&bob.by_socket())
        .await
        .expect("unwire alice from bob");
    right
        .unwire_local(&alice.by_socket())
        .await
        .expect("unwire bob from alice");
    assert!(left.trusts().await.is_empty());
    assert!(right.trusts().await.is_empty());

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyless_in_process_wire_to_a_member_no_local_mob_has_is_refused_in_plain_words() {
    let root = tempfile::tempdir().expect("root");
    let left = Mob::start(root.path(), "wire-absent-a", "alice").await;
    // A member of a mob another process hosts, as that process's peer_info
    // describes it to a caller that passes no key.
    let absent_name = "wire-absent-elsewhere/worker/zed".to_string();
    let absent = PeerInfo {
        comms_name: absent_name.clone(),
        peer_id: meerkat_core::comms::PeerId::from_ed25519_pubkey(&[42_u8; 32]).to_string(),
        address: format!("inproc://{absent_name}"),
        key: None,
        pubkey_b64: None,
        dialable_address: None,
        answer: Value::Null,
    };

    let refusal = left
        .wire_local(&absent)
        .await
        .expect_err("an in-process address reaches no other process");
    assert!(
        refusal.contains("is not a live member of a mob in this process")
            && refusal.contains("member_comms_address"),
        "the refusal must say what is wrong and what to do: {refusal}"
    );
    assert!(
        !refusal.contains("must be non-zero"),
        "the raw descriptor text must not reach the caller: {refusal}"
    );
    assert!(left.trusts().await.is_empty());

    // With the member's key supplied the same wire is accepted as before.
    let keyed = PeerInfo {
        // Standard base64 of the 32-byte key [42; 32] the peer id derives from.
        key: Some("KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=".to_string()),
        ..absent.clone()
    };
    left.wire_local(&keyed)
        .await
        .expect("a keyed in-process wire");
    assert_eq!(left.trusts().await, vec![absent_name]);

    left.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_wire_local_takes_back_the_route_it_installed() {
    let root = tempfile::tempdir().expect("root");
    let left = Mob::start(root.path(), "wire-failed-a", "alice").await;
    let right = Mob::start(root.path(), "wire-failed-b", "bob").await;
    let bob = right.peer_info().await;

    // The route goes in before the trust, and this mob has no such member
    // to give the trust to.
    let mut params = left.wire_params(&bob);
    params["local_member_id"] = json!("nobody");
    left.rpc("meta/cross_mob/wire_local", params)
        .await
        .expect_err("a member this mob does not have");
    assert_eq!(
        left.route_to(&bob.comms_name),
        None,
        "a failed wire left its route behind"
    );
    assert!(left.trusts().await.is_empty());

    left.stop().await;
    right.stop().await;
}

// ---------------------------------------------------------------------------
// A wire is never replaced, and a wire that is already there is left alone.
// ---------------------------------------------------------------------------

impl Mob {
    /// A second member of this mob, on the same profile.
    async fn spawn_second_member(&self, member: &str) {
        self.runtime
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                MeerkatId::from(member).to_string(),
                None,
                Some(MobRuntimeMode::TurnDriven),
                None,
            ))
            .await
            .expect("spawn a second local member");
    }

    /// The wire parameters of `member`, a member of this mob other than the
    /// one the fixture drives.
    fn wire_params_of(&self, member: &str, remote: &PeerInfo) -> Value {
        let mut params = self.wire_params(remote);
        params["local_member_id"] = json!(member);
        params
    }

    /// The comms names `member` trusts.
    async fn trusts_of(&self, member: &str) -> Vec<String> {
        self.runtime
            .mob_handle()
            .get_member(&MeerkatId::from(member))
            .await
            .expect("member lookup")
            .expect("member")
            .wired_to
            .iter()
            .map(|peer| peer.as_str().to_string())
            .collect()
    }
}

/// The refusal of a wire that would replace an edge: plain words that name
/// the remedy, never the mob machine's text.
fn assert_refused_as_already_wired(refusal: &str) {
    assert!(
        refusal.contains("already wired") && refusal.contains("unwire"),
        "the refusal must say the pair is wired with another endpoint and to unwire first: {refusal}"
    );
    assert!(
        !refusal.contains("Running -> Running") && !refusal.contains("invalid state transition"),
        "the mob machine's text must not reach the caller: {refusal}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_wire_of_the_same_edge_changes_nothing() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-twice-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-twice-b", "bob").await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");
    let route_to_bob = left.route_to(&bob.comms_name);
    let route_to_alice = right.route_to(&alice.comms_name);

    // The helper again, and one side alone again: accepted, nothing added.
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the identical edge is accepted again");
    left.wire_local(&bob).await.expect("one side again");
    assert_eq!(left.trusts().await, vec![bob.comms_name.clone()]);
    assert_eq!(right.trusts().await, vec![alice.comms_name.clone()]);
    assert_eq!(left.route_to(&bob.comms_name), route_to_bob);
    assert_eq!(right.route_to(&alice.comms_name), route_to_alice);
    assert_delivers(&mut left, &mut right, "twice a-to-b").await;
    assert_delivers(&mut right, &mut left, "twice b-to-a").await;

    // One edge was stored, not three: one unwire removes trust and route.
    left.unwire_local(&bob)
        .await
        .expect("unwire alice from bob");
    assert!(left.trusts().await.is_empty());
    assert_eq!(left.route_to(&bob.comms_name), None);

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wire_under_another_endpoint_is_refused_in_plain_words_and_changes_nothing() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-moved-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-moved-b", "bob").await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");
    let route_to_bob = left.route_to(&bob.comms_name);

    // The same member at another address: what a peer that came back on a
    // new port presents.
    let moved = PeerInfo {
        address: "tcp://127.0.0.1:45999".to_string(),
        key: bob.pubkey_b64.clone(),
        ..bob.clone()
    };
    let refusal = left
        .wire_local(&moved)
        .await
        .expect_err("a wire is never replaced");
    assert_refused_as_already_wired(&refusal);
    // The refusal names the endpoint the member is wired to now.
    assert!(
        refusal.contains(&bob.address),
        "the refusal names the wired endpoint: {refusal}"
    );

    // Another peer id under the name (a key the caller made up).
    let other_key = PeerInfo {
        peer_id: meerkat_core::comms::PeerId::from_ed25519_pubkey(&[42_u8; 32]).to_string(),
        // Standard base64 of the 32-byte key [42; 32] that peer id derives from.
        key: Some("KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=".to_string()),
        ..bob.clone()
    };
    let refusal = left
        .wire_local(&other_key)
        .await
        .expect_err("a wire is never replaced");
    assert_refused_as_already_wired(&refusal);

    // Nothing changed on either side, and the edge still works.
    assert_eq!(left.trusts().await, vec![bob.comms_name.clone()]);
    assert_eq!(right.trusts().await, vec![alice.comms_name.clone()]);
    assert_eq!(left.route_to(&bob.comms_name), route_to_bob);
    assert_delivers(&mut left, &mut right, "moved a-to-b").await;
    assert_delivers(&mut right, &mut left, "moved b-to-a").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_helper_keeps_a_side_that_was_wired_before_it_ran() {
    let root = tempfile::tempdir().expect("root");
    let left = Mob::start(root.path(), "wire-kept-a", "alice").await;
    let right = Mob::start(root.path(), "wire-kept-b", "bob").await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;

    // The first side is wired before the helper runs.
    left.wire_local(&bob).await.expect("first side");
    // The helper's sequence with a second side that refuses.
    let stale_alice = PeerInfo {
        peer_id: "00000000-0000-4000-8000-0000000000cc".to_string(),
        ..alice.clone()
    };
    sdk_wire_with(&left, &stale_alice, &right, &bob)
        .await
        .expect_err("the second side must refuse");

    // The helper undoes only a wire it made: the earlier edge and its route
    // are still there.
    assert_eq!(left.trusts().await, vec![bob.comms_name.clone()]);
    assert_eq!(left.route_to(&bob.comms_name), Some(bob.peer_id.clone()));
    assert!(right.trusts().await.is_empty());

    left.stop().await;
    right.stop().await;
}

// ---------------------------------------------------------------------------
// Properties the route relies on.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_local_members_wired_to_one_peer_share_the_route_until_the_last_unwire() {
    let root = tempfile::tempdir().expect("root");
    let left = Mob::start(root.path(), "wire-shared-a", "alice").await;
    let right = Mob::start(root.path(), "wire-shared-b", "bob").await;
    left.spawn_second_member("alma").await;
    let bob = right.peer_info().await;
    let route = Some(bob.peer_id.clone());

    left.wire_local(&bob).await.expect("alice to bob");
    left.rpc(
        "meta/cross_mob/wire_local",
        left.wire_params_of("alma", &bob),
    )
    .await
    .expect("alma to bob");
    assert_eq!(left.route_to(&bob.comms_name), route);

    // A failed wire of a third caller leaves the route the others use.
    left.rpc(
        "meta/cross_mob/wire_local",
        left.wire_params_of("nobody", &bob),
    )
    .await
    .expect_err("a member this mob does not have");
    assert_eq!(left.route_to(&bob.comms_name), route);

    // The first unwire keeps the route for the member still wired.
    left.unwire_local(&bob).await.expect("unwire alice");
    assert!(left.trusts().await.is_empty());
    assert_eq!(left.trusts_of("alma").await, vec![bob.comms_name.clone()]);
    assert_eq!(
        left.route_to(&bob.comms_name),
        route,
        "the route went with the first of two unwires"
    );

    // The last unwire removes it.
    left.rpc(
        "meta/cross_mob/unwire_local",
        left.wire_params_of("alma", &bob),
    )
    .await
    .expect("unwire alma");
    assert!(left.trusts_of("alma").await.is_empty());
    assert_eq!(left.route_to(&bob.comms_name), None);

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_after_a_one_sided_wire_is_dropped_by_the_receiver() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-one-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-one-b", "bob").await;
    let bob = right.peer_info().await;

    // Only alice's side: alice trusts bob and her mob routes to him. Bob
    // has not wired alice.
    left.wire_local(&bob).await.expect("first side");
    assert_eq!(left.route_to(&bob.comms_name), Some(bob.peer_id.clone()));
    assert!(right.trusts().await.is_empty());

    let result = left.send(&bob.peer_id, "one-sided a-to-b").await;
    assert!(
        result.contains("untrusted_sender"),
        "a route alone must not be heard: the receiver drops a sender it has not wired: {result}"
    );
    assert!(
        !right.receives("one-sided a-to-b", NO_DELIVERY_WAIT).await,
        "a member was handed a message from a peer it never wired"
    );

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keyless_wire_to_a_member_of_the_same_mob_is_refused_in_plain_words() {
    let root = tempfile::tempdir().expect("root");
    let left = Mob::start(root.path(), "wire-same-a", "alice").await;
    left.spawn_second_member("alma").await;
    let alma = left
        .rpc("meta/cross_mob/peer_info", json!({"member_id": "alma"}))
        .await
        .expect("peer_info of the second member");

    let refusal = left
        .rpc(
            "meta/cross_mob/wire_local",
            json!({
                "local_member_id": "alice",
                "remote_comms_name": alma["comms_name"],
                "remote_peer_id": alma["peer_id"],
                "remote_address": alma["address"],
            }),
        )
        .await
        .expect_err("wire_local wires a member to a peer of another mob");
    assert!(
        refusal.contains("is a member of this mob"),
        "the refusal must say what is wrong: {refusal}"
    );
    assert!(
        !refusal.contains("must be non-zero"),
        "the raw descriptor text must not reach the caller: {refusal}"
    );
    assert!(left.trusts().await.is_empty());

    left.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_respawned_peer_is_wired_again_after_both_sides_unwire() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-respawn-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-respawn-b", "bob").await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");

    right
        .rpc("meta/respawn_member", json!({"member_id": "bob"}))
        .await
        .expect("respawn bob");
    let new_bob = right.peer_info().await;
    assert_ne!(
        new_bob.peer_id, bob.peer_id,
        "fixture: a respawned member has a new key"
    );

    // The helper again: alice is wired to bob's old key, and a wire is
    // never replaced. The refusal leaves trust and routes exactly as they
    // were, on both sides.
    let refusal = sdk_wire_cross_mob(&left, &right)
        .await
        .expect_err("a wire is never replaced");
    assert_refused_as_already_wired(&refusal);
    assert_eq!(left.trusts().await, vec![bob.comms_name.clone()]);
    assert_eq!(
        left.route_to(&bob.comms_name),
        Some(bob.peer_id.clone()),
        "a refused wire changed the route"
    );
    assert_eq!(right.trusts().await, vec![alice.comms_name.clone()]);
    assert_eq!(
        right.route_to(&alice.comms_name),
        Some(alice.peer_id.clone())
    );

    // The way back: unwire both sides, each with the other member's CURRENT
    // peer info, then wire again.
    left.unwire_local(&new_bob).await.expect("unwire alice");
    right.unwire_local(&alice).await.expect("unwire bob");
    assert!(left.trusts().await.is_empty());
    assert!(right.trusts().await.is_empty());
    assert_eq!(
        left.route_to(&bob.comms_name),
        None,
        "the unwire left the route to the old key"
    );
    assert_eq!(right.route_to(&alice.comms_name), None);

    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence after both sides unwired");
    assert_eq!(
        left.route_to(&bob.comms_name),
        Some(new_bob.peer_id.clone())
    );
    assert_delivers(&mut left, &mut right, "respawn a-to-b").await;
    assert_delivers(&mut right, &mut left, "respawn b-to-a").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_route_under_the_members_name_is_replaced_by_the_route_to_its_current_key() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-stale-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-stale-b", "bob").await;
    let third = Mob::start(root.path(), "wire-stale-c", "carol").await;
    let bob = right.peer_info().await;
    let carol = third.peer_info().await;

    // What an earlier incarnation of bob leaves behind when nobody unwires:
    // an entry under bob's name in alice's namespace, under a key bob no
    // longer has. Only a route to that member can sit under that name (a
    // member's name carries its mob), and only code inside the process can
    // put one there; here another inbox stands in for the old one.
    let registry = InprocRegistry::global();
    let old_key = meerkat_comms::PubKey::new(third.registered_key(&carol.comms_name));
    let old_inbox = registry
        .get_by_pubkey_in_namespace(&third.namespace(), &old_key)
        .expect("an inbox to stand in for the old incarnation");
    let stale = registry.register_with_meta_in_namespace(
        &left.namespace(),
        bob.comms_name.clone(),
        old_key,
        old_inbox,
        meerkat_core::PeerMeta::default(),
    );
    assert!(!stale.is_rejected(), "fixture: {stale:?}");
    assert_eq!(left.route_to(&bob.comms_name), Some(carol.peer_id.clone()));

    // The wire replaces it with the route to bob's current key.
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence over a stale route");
    assert_eq!(left.route_to(&bob.comms_name), Some(bob.peer_id.clone()));
    assert_delivers(&mut left, &mut right, "stale a-to-b").await;
    assert_delivers(&mut right, &mut left, "stale b-to-a").await;

    left.stop().await;
    right.stop().await;
    third.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wire_is_refused_when_the_local_mob_already_holds_that_key() {
    let root = tempfile::tempdir().expect("root");
    // Two mobs whose members have one transport key.
    let shared = root.path().join("shared-sessions");
    let left = Mob::start_at(
        root.path(),
        &shared,
        "wire-key-a",
        "alice",
        socket_member_comms(),
    )
    .await;
    let right = Mob::start_at(
        root.path(),
        &shared,
        "wire-key-b",
        "bob",
        socket_member_comms(),
    )
    .await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    assert_eq!(alice.peer_id, bob.peer_id, "fixture: one key for both");

    let refusal = left
        .wire_local(&bob)
        .await
        .expect_err("the local mob already holds that key");
    assert!(
        refusal.contains("already holds that key"),
        "the refusal must say what is wrong: {refusal}"
    );
    // Nothing changed: the local member keeps its own registration, no
    // route to the peer appeared, no trust was stored.
    assert_eq!(
        left.route_to(&alice.comms_name),
        Some(alice.peer_id.clone()),
        "the local member lost its own registration"
    );
    assert_eq!(left.route_to(&bob.comms_name), None);
    assert!(left.trusts().await.is_empty());

    left.stop().await;
    right.stop().await;
}

// ---------------------------------------------------------------------------
// Persistent state: a runtime is stopped and built again over its state.
// ---------------------------------------------------------------------------

impl Mob {
    /// A mob over persistent state under `root`. The first call for a mob
    /// spawns its member (`spawn`); a later call over the same `root` finds
    /// the member in the state, with the key it had.
    async fn start_persistent(root: &Path, mob_id: &str, member: &str, spawn: bool) -> Self {
        let definition = MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
        ))
        .expect("definition");
        let (observations, received) = tokio::sync::mpsc::unbounded_channel();
        let client: Arc<dyn LlmClient> = Arc::new(PeerScriptClient {
            sent: sent_bodies(),
            observations,
        });
        let runtime = meerkat_meta::UnifiedRuntimeBuilder::default()
            .definition(definition)
            .persistent_state(root.join(format!("{mob_id}-state")))
            .session_identity_config_root(root.join("keys"))
            .comms(true)
            .default_llm_client(client)
            .build()
            .await
            .expect("build a runtime over persistent state");
        if spawn {
            runtime
                .spawn(SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    MeerkatId::from(member).to_string(),
                    None,
                    Some(MobRuntimeMode::TurnDriven),
                    None,
                ))
                .await
                .expect("spawn member");
        }
        Self {
            runtime,
            mob_id: mob_id.to_string(),
            member: member.to_string(),
            received,
            unmatched: Vec::new(),
        }
    }

    /// A host message to the member, which builds it when it is not built
    /// yet; returns once its model has answered.
    async fn wake(&mut self, body: &str) {
        self.runtime
            .mob_handle()
            .member(&MeerkatId::from(self.member.as_str()))
            .await
            .expect("member handle")
            .send(body.to_string(), HandlingMode::Queue)
            .await
            .expect("host message");
        assert!(
            self.receives(body, DELIVERY_WAIT).await,
            "{}: the member did not run a turn on the host message",
            self.mob_id
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_identical_wire_again_reaches_a_member_that_was_built_again() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wire-rebuilt-a", "alice", true).await;
    let mut right = Mob::start_persistent(root.path(), "wire-rebuilt-b", "bob", true).await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");
    assert_delivers(&mut left, &mut right, "rebuilt before a-to-b").await;

    // Bob's runtime is stopped and built again over its state; bob is built
    // again by a host message. Same name, same key, a new inbox.
    right.stop().await;
    let mut right = Mob::start_persistent(root.path(), "wire-rebuilt-b", "bob", false).await;
    right.wake("rebuilt host message").await;
    assert_eq!(right.peer_info().await.peer_id, bob.peer_id);

    // The identical wire again is accepted, and its route is the member's
    // inbox of now, not the one that is gone.
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the identical edge is accepted again");
    assert_delivers(&mut left, &mut right, "rebuilt after a-to-b").await;
    assert_delivers(&mut right, &mut left, "rebuilt after b-to-a").await;

    left.stop().await;
    right.stop().await;
}

// ---------------------------------------------------------------------------
// The Console's wire_local / unwire_local (its own handler over the same
// route calls).
// ---------------------------------------------------------------------------

fn console_decisions() -> meerkat_meta::RuntimeDecisionState {
    meerkat_meta::build_runtime_decision_state(meerkat_meta::RuntimeDecisionInputs {
        bigquery: meerkat_meta::BigQueryNaming {
            dataset: "wire_local_dataset".to_string(),
            table: "wire_local_table".to_string(),
        },
        trusted_meta_toml: "[[modules]]\nid = \"router\"\ncommand = \"router-bin\"\nargs = []\nrestart_policy = \"always\"\n"
            .to_string(),
        auth: meerkat_meta::AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec!["root@example.test".to_string()],
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
            require_app_auth: false,
            ..meerkat_meta::ConsolePolicy::default()
        },
        ops: meerkat_meta::RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state")
}

/// A signed-in person's console token.
fn console_token(subject: &str) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = Some("kid-current".to_string());
    jsonwebtoken::encode(
        &header,
        &json!({
            "iss": "https://trusted.meta.localhost",
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

/// One Console RPC: the result, or the error message.
async fn console_rpc(
    app: &axum::Router,
    bearer: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    use tower::ServiceExt;
    let payload = json!({"jsonrpc": "2.0", "id": "test", "method": method, "params": params});
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(axum::http::header::CONTENT_TYPE, "application/json");
    if let Some(bearer) = bearer {
        request = request.header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {bearer}"),
        );
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(axum::body::Body::from(payload.to_string()))
                .expect("rpc request"),
        )
        .await
        .expect("rpc response");
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("rpc body");
    let response: Value = serde_json::from_slice(&body).expect("rpc json");
    match response.get("error") {
        Some(error) if !error.is_null() => Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()),
        _ => Ok(response.get("result").cloned().unwrap_or(Value::Null)),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_wire_local_installs_the_route_and_its_unwire_removes_it() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-console-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-console-b", "bob").await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    // The caller is the Console's access admin: the one caller whose wire
    // may look at the other mobs of the process.
    left.runtime.set_access_controller(
        meerkat_meta::AccessController::new(meerkat_meta::AccessControlConfig {
            approvers: Default::default(),
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            groups: Default::default(),
            rules: Vec::new(),
        })
        .expect("controller"),
    );
    let console = left.runtime.build_reference_app_router(console_decisions());
    let admin = console_token("root@example.test");
    let admin = Some(admin.as_str());

    console_rpc(
        &console,
        admin,
        "meta/cross_mob/wire_local",
        left.wire_params(&bob),
    )
    .await
    .expect("the Console wires alice to bob");
    assert_eq!(left.trusts().await, vec![bob.comms_name.clone()]);
    assert_eq!(
        left.route_to(&bob.comms_name),
        Some(bob.peer_id.clone()),
        "the Console's wire_local left trust without a route"
    );
    right.wire_local(&alice).await.expect("bob's side");
    assert_delivers(&mut left, &mut right, "console a-to-b").await;
    assert_delivers(&mut right, &mut left, "console b-to-a").await;

    // A wire under another endpoint is refused there in the same plain words.
    let moved = PeerInfo {
        address: "tcp://127.0.0.1:45999".to_string(),
        key: bob.pubkey_b64.clone(),
        ..bob.clone()
    };
    let refusal = console_rpc(
        &console,
        admin,
        "meta/cross_mob/wire_local",
        left.wire_params(&moved),
    )
    .await
    .expect_err("a wire is never replaced");
    assert_refused_as_already_wired(&refusal);
    assert_eq!(left.route_to(&bob.comms_name), Some(bob.peer_id.clone()));

    console_rpc(
        &console,
        admin,
        "meta/cross_mob/unwire_local",
        left.wire_params(&bob),
    )
    .await
    .expect("the Console unwires alice from bob");
    assert!(left.trusts().await.is_empty());
    assert_eq!(left.route_to(&bob.comms_name), None);

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_wire_local_tells_a_caller_who_is_no_admin_nothing_about_another_mob() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-oracle-a", "alice").await;
    let right = Mob::start(root.path(), "wire-oracle-b", "bob").await;
    let bob = right.peer_info().await;
    // Access is enforced: one named admin, and everyone (signed in or not)
    // may operate the runtime's wiring and see its members.
    left.runtime.set_access_controller(
        meerkat_meta::AccessController::new(meerkat_meta::AccessControlConfig {
            approvers: Default::default(),
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            groups: Default::default(),
            rules: vec![meerkat_meta::AccessRule {
                id: "everyone-operates-wiring".to_string(),
                actions: vec!["runtime.admin".to_string(), "agent.view".to_string()],
                ..meerkat_meta::AccessRule::default()
            }],
        })
        .expect("controller"),
    );
    let console = left.runtime.build_reference_app_router(console_decisions());

    // A name no mob of this process has, shaped like bob's.
    let absent_name = "wire-oracle-b/worker/nobody".to_string();
    let absent = PeerInfo {
        comms_name: absent_name.clone(),
        address: format!("inproc://{absent_name}"),
        ..bob.clone()
    };
    let keyed = |info: &PeerInfo| PeerInfo {
        key: bob.pubkey_b64.clone(),
        ..info.clone()
    };

    // No key: the answer for the live member of the other mob is the answer
    // for the name no mob has, word for word, and nothing is installed.
    let for_live = console_rpc(
        &console,
        None,
        "meta/cross_mob/wire_local",
        left.wire_params(&bob),
    )
    .await
    .expect_err("a caller who is no admin learns nothing about another mob");
    let for_absent = console_rpc(
        &console,
        None,
        "meta/cross_mob/wire_local",
        left.wire_params(&absent),
    )
    .await
    .expect_err("a name no mob has");
    assert_eq!(
        for_live.replace(&bob.comms_name, "<name>"),
        for_absent.replace(&absent_name, "<name>"),
        "the two answers differ"
    );
    assert_eq!(left.route_to(&bob.comms_name), None);
    assert!(left.trusts().await.is_empty());

    // With the member's key: trust alone, as for the name no mob has. No
    // route into the other mob appears.
    for info in [keyed(&bob), keyed(&absent)] {
        console_rpc(
            &console,
            None,
            "meta/cross_mob/wire_local",
            left.wire_params(&info),
        )
        .await
        .expect("a keyed in-process wire is trust alone");
        assert_eq!(
            left.route_to(&info.comms_name),
            None,
            "a caller who is no admin installed a route into another mob"
        );
        console_rpc(
            &console,
            None,
            "meta/cross_mob/unwire_local",
            left.wire_params(&info),
        )
        .await
        .expect("unwire");
    }
    assert!(left.trusts().await.is_empty());

    // The admin's wire is the operator's: the route is installed.
    let admin = console_token("root@example.test");
    console_rpc(
        &console,
        Some(&admin),
        "meta/cross_mob/wire_local",
        left.wire_params(&bob),
    )
    .await
    .expect("the admin wires alice to bob");
    assert_eq!(left.route_to(&bob.comms_name), Some(bob.peer_id.clone()));

    left.stop().await;
    right.stop().await;
}

// ---------------------------------------------------------------------------
// A peer message wakes a member of the other mob that is not built: after a
// runtime was stopped and built again, with no second wire and no host
// message to the recipient. A wake is forwarded only for a link both mobs
// hold.
// ---------------------------------------------------------------------------

impl Mob {
    /// Stop the runtime and build it again over its state. Nobody is built.
    async fn built_again(self, root: &Path) -> Self {
        let (mob_id, member) = (self.mob_id.clone(), self.member.clone());
        self.stop().await;
        Self::start_persistent(root, &mob_id, &member, false).await
    }

    /// The names this mob's namespace routes, its own members included.
    fn routed_names(&self) -> Vec<String> {
        let mut names: Vec<String> = InprocRegistry::global()
            .peers_in_namespace(&self.namespace())
            .into_iter()
            .map(|peer| peer.name)
            .collect();
        names.sort();
        names
    }
}

/// The member by its in-process address and its key: the descriptor of a
/// member that may not be built.
fn keyed(info: &PeerInfo) -> PeerInfo {
    PeerInfo {
        key: Some(
            info.pubkey_b64
                .clone()
                .expect("peer_info reports the member's key"),
        ),
        ..info.clone()
    }
}

/// The sender's `send_message` is refused as a peer that is not found, the
/// recipient's model is handed nothing, and the recipient is not built.
async fn assert_wakes_nobody(sender: &mut Mob, recipient: &mut Mob, to: &PeerInfo, body: &str) {
    let result = sender.send(&to.peer_id, body).await;
    assert!(
        result.contains(NOT_ROUTABLE),
        "{} -> {}: the send must be refused as a peer that is not found: {result}",
        sender.mob_id,
        recipient.mob_id
    );
    assert!(
        !recipient.receives(body, NO_DELIVERY_WAIT).await,
        "{} -> {}: the message was delivered",
        sender.mob_id,
        recipient.mob_id
    );
    assert_eq!(
        recipient.route_to(&to.comms_name),
        None,
        "{}: the member was built by a send that must wake nobody",
        recipient.mob_id
    );
    assert_eq!(
        sender.route_to(&to.comms_name),
        None,
        "{}: a refused send left a route to the member",
        sender.mob_id
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_mobs_built_again_deliver_each_way_with_no_second_wire_and_no_host_wake() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-both-a", "alice", true).await;
    let mut right = Mob::start_persistent(root.path(), "wake-both-b", "bob", true).await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");
    assert_delivers(&mut left, &mut right, "wake-both before a-to-b").await;
    assert_delivers(&mut right, &mut left, "wake-both before b-to-a").await;

    // Both runtimes are stopped and built again over their state. A stopped
    // runtime leaves no route behind: none of its own, none to its members.
    let left = left.built_again(root.path()).await;
    let right = right.built_again(root.path()).await;
    let (mut left, mut right) = (left, right);
    assert_eq!(left.routed_names(), Vec::<String>::new());
    assert_eq!(right.routed_names(), Vec::<String>::new());

    // Nobody wires again. Alice's turn is started by the host (a member
    // sends from a turn); bob gets no host message: alice's peer message
    // builds him, and he is handed it.
    assert_delivers(&mut left, &mut right, "wake-both after a-to-b").await;
    assert_eq!(
        right.route_to(&bob.comms_name),
        Some(bob.peer_id.clone()),
        "the peer message built bob"
    );
    assert_delivers(&mut right, &mut left, "wake-both after b-to-a").await;
    assert_eq!(left.route_to(&alice.comms_name), Some(alice.peer_id));

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_mob_built_again_is_reached_by_its_peer_with_no_second_wire_and_no_host_wake() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-one-a", "alice", true).await;
    let right = Mob::start_persistent(root.path(), "wake-one-b", "bob", true).await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");

    // Bob's runtime alone is stopped and built again. Alice's mob keeps
    // running, and its route to the bob that is gone went with his runtime.
    let mut right = right.built_again(root.path()).await;
    assert_eq!(left.route_to(&bob.comms_name), None);
    assert_eq!(right.routed_names(), Vec::<String>::new());

    // Alice's peer message builds bob; his answer reaches alice.
    assert_delivers(&mut left, &mut right, "wake-one after a-to-b").await;
    assert_eq!(right.route_to(&bob.comms_name), Some(bob.peer_id.clone()));
    assert_delivers(&mut right, &mut left, "wake-one after b-to-a").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_only_one_mob_holds_wakes_nobody() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-half-a", "alice", true).await;
    let right = Mob::start_persistent(root.path(), "wake-half-b", "bob", true).await;
    let bob = right.peer_info().await;

    // Only alice's side is wired: her mob holds the link, bob's does not.
    left.wire_local(&bob).await.expect("alice's side alone");
    assert!(right.trusts().await.is_empty());

    // Bob's runtime is built again: bob is not built.
    let mut right = right.built_again(root.path()).await;
    assert_wakes_nobody(&mut left, &mut right, &bob, "wake-half a-to-b").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_an_unwire_on_one_side_nothing_is_forwarded() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-unwired-a", "alice", true).await;
    let mut right = Mob::start_persistent(root.path(), "wake-unwired-b", "bob", true).await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");
    assert_delivers(&mut left, &mut right, "wake-unwired before a-to-b").await;

    let left = left.built_again(root.path()).await;
    let right = right.built_again(root.path()).await;
    let (mut left, mut right) = (left, right);

    // Bob's side is unwired while bob is not built; the unwire builds
    // nobody. Alice's mob still holds its half of the link.
    right
        .unwire_local(&keyed(&alice))
        .await
        .expect("unwire bob from alice");
    assert_eq!(right.routed_names(), Vec::<String>::new());
    assert_wakes_nobody(&mut left, &mut right, &bob, "wake-unwired a-to-b").await;

    // Wired again on bob's side, the link is whole and a peer message
    // builds bob.
    right
        .wire_local(&keyed(&alice))
        .await
        .expect("wire bob to alice again");
    assert_delivers(&mut left, &mut right, "wake-unwired again a-to-b").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wire_to_a_member_that_is_not_built_with_its_key_delivers() {
    let root = tempfile::tempdir().expect("root");
    let left = Mob::start_persistent(root.path(), "wake-keyed-a", "alice", true).await;
    let right = Mob::start_persistent(root.path(), "wake-keyed-b", "bob", true).await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;

    // Both runtimes are built again before anything is wired: neither
    // member is built.
    let left = left.built_again(root.path()).await;
    let right = right.built_again(root.path()).await;
    let (mut left, mut right) = (left, right);

    // The in-process form with the member's key wires a member that is not
    // built. The wire builds nobody and installs no route.
    left.wire_local(&keyed(&bob))
        .await
        .expect("wire alice to a bob that is not built");
    right
        .wire_local(&keyed(&alice))
        .await
        .expect("wire bob to an alice that is not built");
    assert_eq!(left.routed_names(), Vec::<String>::new());
    assert_eq!(right.routed_names(), Vec::<String>::new());
    assert_eq!(left.trusts().await, vec![bob.comms_name.clone()]);
    assert_eq!(right.trusts().await, vec![alice.comms_name.clone()]);

    assert_delivers(&mut left, &mut right, "wake-keyed a-to-b").await;
    assert_delivers(&mut right, &mut left, "wake-keyed b-to-a").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_mob_refuses_the_wake_and_builds_nobody() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-stopped-a", "alice", true).await;
    let mut right = Mob::start_persistent(root.path(), "wake-stopped-b", "bob", true).await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");
    assert_delivers(&mut left, &mut right, "wake-stopped before a-to-b").await;

    let left = left.built_again(root.path()).await;
    let right = right.built_again(root.path()).await;
    let (mut left, mut right) = (left, right);

    // Bob's mob is stopped while its runtime lives on. The wake is asked of
    // bob's own mob, which refuses it: alice's send is refused with that
    // typed answer, and bob is not built.
    right
        .runtime
        .mob_handle()
        .stop()
        .await
        .expect("stop bob's mob");
    let result = left.send(&bob.peer_id, "wake-stopped a-to-b").await;
    assert!(
        result.contains("peer_wake_refused"),
        "a stopped mob refuses the wake with its own typed answer: {result}"
    );
    assert!(
        !right
            .receives("wake-stopped a-to-b", NO_DELIVERY_WAIT)
            .await,
        "a member of a stopped mob was handed a peer message"
    );
    assert_eq!(right.route_to(&bob.comms_name), None, "bob was built");
    assert_eq!(left.route_to(&bob.comms_name), None);

    left.stop().await;
    right.stop().await;
}

// ---------------------------------------------------------------------------
// A mob's own sends beside another mob's wake route; a destroyed mob; and
// the Console caller who is no access admin.
// ---------------------------------------------------------------------------

impl Mob {
    /// `member` of this mob calls `send_message` to `peer_id`; the tool
    /// result its model was handed.
    async fn send_from(&mut self, member: &str, peer_id: &str, body: &str) -> String {
        self.runtime
            .mob_handle()
            .member(&MeerkatId::from(member))
            .await
            .expect("member handle")
            .send(
                format!("{SEND_TRIGGER}{peer_id} body={body}"),
                HandlingMode::Queue,
            )
            .await
            .expect("trigger the sending turn");
        self.observation(Duration::from_secs(20), |observation| {
            observation.after_tool
        })
        .await
        .unwrap_or_else(|| panic!("{}: no send_message result for {body:?}", self.mob_id))
        .last
    }

    /// A second member, wired to the fixture's member inside this mob.
    async fn spawn_wired_second_member(&self, member: &str) {
        self.spawn_second_member(member).await;
        self.runtime
            .mob_handle()
            .wire(
                MeerkatId::from(member),
                meerkat_mob::PeerTarget::Local(MeerkatId::from(self.member.as_str())),
            )
            .await
            .expect("wire two members of one mob");
    }

    fn has_wake_route_for(&self, peer: &PeerInfo) -> bool {
        use base64::Engine;
        let key: [u8; 32] = base64::engine::general_purpose::STANDARD
            .decode(
                peer.pubkey_b64
                    .as_deref()
                    .expect("peer_info reports the member's key"),
            )
            .expect("base64 key")
            .try_into()
            .expect("a 32-byte key");
        InprocRegistry::global()
            .has_wake_route_in_namespace(&self.namespace(), &meerkat_comms::PubKey::new(key))
    }
}

/// Each mob's namespace holds a wake route under the OTHER mob's member's
/// key (the forwarder's), beside that member's own mob's route in its own
/// namespace. A mob's own sends to its own member are not touched by it: a
/// member's peer message inside its mob, and a host message, still build
/// the member and reach it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mobs_own_sends_still_build_its_member_beside_another_mobs_wake_route() {
    let root = tempfile::tempdir().expect("root");
    let left = Mob::start_persistent(root.path(), "wake-own-a", "alice", true).await;
    let right = Mob::start_persistent(root.path(), "wake-own-b", "bob", true).await;
    left.spawn_wired_second_member("dave").await;
    right.spawn_wired_second_member("carol").await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");

    let left = left.built_again(root.path()).await;
    let right = right.built_again(root.path()).await;
    let (mut left, mut right) = (left, right);
    assert!(left.has_wake_route_for(&bob), "mob A forwards for bob");
    assert!(right.has_wake_route_for(&alice), "mob B forwards for alice");
    assert_eq!(right.routed_names(), Vec::<String>::new());

    // Mob B's own member carol sends to bob, who is not built: bob's own
    // mob builds him, and he is handed carol's message.
    let result = right
        .send_from("carol", &bob.peer_id, "own carol-to-bob")
        .await;
    assert!(!result.contains(NOT_ROUTABLE), "{result}");
    assert!(
        right.receives("own carol-to-bob", DELIVERY_WAIT).await,
        "bob was not handed his own mob's peer message: {result}"
    );
    assert_eq!(right.route_to(&bob.comms_name), Some(bob.peer_id.clone()));

    // Mob A's host sends alice a message: it builds her.
    assert_eq!(left.route_to(&alice.comms_name), None, "alice is not built");
    left.wake("own host-to-alice").await;
    assert_eq!(
        left.route_to(&alice.comms_name),
        Some(alice.peer_id.clone())
    );

    // And the link between the two mobs still delivers both ways.
    assert_delivers(&mut left, &mut right, "own a-to-b").await;
    assert_delivers(&mut right, &mut left, "own b-to-a").await;

    left.stop().await;
    right.stop().await;
}

/// A mob the runtime stopped for its teardown holds no link any more: the
/// other mob's send is refused as a peer that is not found, and nothing is
/// asked of the stopped mob.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mob_stopped_for_teardown_holds_no_link() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-teardown-a", "alice", true).await;
    let right = Mob::start_persistent(root.path(), "wake-teardown-b", "bob", true).await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");

    let mut right = right.built_again(root.path()).await;
    assert!(matches!(
        right.runtime.stop_mob_for_teardown().await,
        meerkat_meta::MobStopOutcome::Stopped
    ));
    assert_wakes_nobody(&mut left, &mut right, &bob, "wake-teardown a-to-b").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_destroyed_mob_is_refused_and_builds_nobody() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-destroyed-a", "alice", true).await;
    let mut right = Mob::start_persistent(root.path(), "wake-destroyed-b", "bob", true).await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");
    assert_delivers(&mut left, &mut right, "wake-destroyed before a-to-b").await;

    let left = left.built_again(root.path()).await;
    let right = right.built_again(root.path()).await;
    let (mut left, mut right) = (left, right);

    // Bob's mob is destroyed while its runtime lives on.
    right
        .runtime
        .mob_handle()
        .destroy()
        .await
        .expect("destroy bob's mob");
    let result = left.send(&bob.peer_id, "wake-destroyed a-to-b").await;
    assert!(
        result.contains("peer_wake_refused") || result.contains(NOT_ROUTABLE),
        "a destroyed mob wakes nobody, and the send is refused: {result}"
    );
    assert!(
        !right
            .receives("wake-destroyed a-to-b", NO_DELIVERY_WAIT)
            .await,
        "a member of a destroyed mob was handed a peer message"
    );
    assert_eq!(right.route_to(&bob.comms_name), None, "bob was built");
    assert_eq!(left.route_to(&bob.comms_name), None);

    left.stop().await;
    let _ = right.runtime.shutdown().await;
}

/// A Console over `mob` on which everyone may operate the wiring and one
/// named subject is the access admin.
fn console_for_everyone(mob: &mut Mob) -> axum::Router {
    mob.runtime.set_access_controller(
        meerkat_meta::AccessController::new(meerkat_meta::AccessControlConfig {
            approvers: Default::default(),
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            groups: Default::default(),
            rules: vec![meerkat_meta::AccessRule {
                id: "everyone-operates-wiring".to_string(),
                actions: vec!["runtime.admin".to_string(), "agent.view".to_string()],
                ..meerkat_meta::AccessRule::default()
            }],
        })
        .expect("controller"),
    );
    mob.runtime.build_reference_app_router(console_decisions())
}

/// A descriptor shaped like a member of `mob_id` under a key nobody holds.
fn nobody_of(mob_id: &str) -> PeerInfo {
    use base64::Engine;
    let key = meerkat_comms::Keypair::generate().public_key();
    let comms_name = format!("{mob_id}/worker/nobody");
    let pubkey_b64 = base64::engine::general_purpose::STANDARD.encode(key.as_bytes());
    PeerInfo {
        address: format!("inproc://{comms_name}"),
        comms_name,
        peer_id: key.to_peer_id().to_string(),
        key: Some(pubkey_b64.clone()),
        pubkey_b64: Some(pubkey_b64),
        dialable_address: None,
        answer: Value::Null,
    }
}

/// A Console caller who is no access admin wires alice, with the key, to a
/// member of another mob that exists and to a key nobody holds. While the
/// other mob does not hold the edge, the two are not told apart: the same
/// wire answer, no route, and the same refusal of alice's send, which wakes
/// nobody.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keyed_wire_by_a_caller_who_is_no_admin_tells_an_existing_member_from_nobody_in_nothing()
{
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-oracle-a", "alice", true).await;
    let right = Mob::start_persistent(root.path(), "wake-oracle-b", "bob", true).await;
    let bob = keyed(&right.peer_info().await);
    let nobody = nobody_of("wake-oracle-b");
    // Bob's runtime is built again: bob exists and is not built.
    let mut right = right.built_again(root.path()).await;
    let console = console_for_everyone(&mut left);

    let mut wire_answers = Vec::new();
    let mut send_answers = Vec::new();
    for (info, body) in [(&bob, "oracle to-bob"), (&nobody, "oracle to-nobody")] {
        let wired = console_rpc(
            &console,
            None,
            "meta/cross_mob/wire_local",
            left.wire_params(info),
        )
        .await
        .expect("a keyed in-process wire is accepted");
        wire_answers.push(wired.to_string().replace(&info.comms_name, "<name>"));
        assert_eq!(
            left.route_to(&info.comms_name),
            None,
            "the wire installed a route"
        );

        let result = left.send(&info.peer_id, body).await;
        assert!(result.contains(NOT_ROUTABLE), "{result}");
        send_answers.push(
            result
                .replace(&info.peer_id, "<peer>")
                .replace(body, "<body>"),
        );
        assert_eq!(
            left.route_to(&info.comms_name),
            None,
            "the send installed a route"
        );
        assert!(!right.receives(body, NO_DELIVERY_WAIT).await);
        assert_eq!(
            right.routed_names(),
            Vec::<String>::new(),
            "somebody was built"
        );

        console_rpc(
            &console,
            None,
            "meta/cross_mob/unwire_local",
            left.wire_params(info),
        )
        .await
        .expect("unwire");
    }
    assert_eq!(wire_answers[0], wire_answers[1], "the wire answers differ");
    let stamp = |answer: &str| -> String {
        // The tool result carries a time and the tool call's id.
        answer
            .split("\"content\":")
            .nth(1)
            .and_then(|rest| rest.split("\"is_error\"").next())
            .unwrap_or(answer)
            .to_string()
    };
    assert_eq!(
        stamp(&send_answers[0]),
        stamp(&send_answers[1]),
        "the send answers differ"
    );

    left.stop().await;
    right.stop().await;
}

/// The edge a caller who is no access admin wired is an edge of the mob's
/// state like any other, before and after a restart: alone it wakes nobody,
/// and once the other mob holds it too a peer message builds the member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edge_wired_by_a_caller_who_is_no_admin_behaves_the_same_after_a_restart() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-same-a", "alice", true).await;
    let right = Mob::start_persistent(root.path(), "wake-same-b", "bob", true).await;
    let alice = keyed(&left.peer_info().await);
    let bob = keyed(&right.peer_info().await);
    let mut right = right.built_again(root.path()).await;
    let console = console_for_everyone(&mut left);
    console_rpc(
        &console,
        None,
        "meta/cross_mob/wire_local",
        left.wire_params(&bob),
    )
    .await
    .expect("a keyed in-process wire is accepted");

    // The Console app holds its runtime's state: it goes before the runtime
    // is built again.
    drop(console);

    // Only alice's mob holds the edge: nobody is woken, before a restart
    // and after one.
    assert_wakes_nobody(&mut left, &mut right, &bob, "same alone before").await;
    let left = left.built_again(root.path()).await;
    let right = right.built_again(root.path()).await;
    let (mut left, mut right) = (left, right);
    assert_wakes_nobody(&mut left, &mut right, &bob, "same alone after").await;

    // Bob's mob wires the edge back: a peer message builds bob, before a
    // restart and after one.
    right.wire_local(&alice).await.expect("wire bob to alice");
    assert_delivers(&mut left, &mut right, "same whole before").await;
    let left = left.built_again(root.path()).await;
    let right = right.built_again(root.path()).await;
    let (mut left, mut right) = (left, right);
    assert_eq!(right.routed_names(), Vec::<String>::new());
    assert_delivers(&mut left, &mut right, "same whole after").await;

    left.stop().await;
    right.stop().await;
}

// ---------------------------------------------------------------------------
// The bilateral same-process wire (`wire_cross_mob`, the topology
// coordinator's path) writes the same rows as `wire_local`: its edges are
// links from the moment they are committed, and stop being links the moment
// they are removed.
// ---------------------------------------------------------------------------

/// Each runtime knows the other mob as an in-process contact, as a host that
/// composes two mobs in one process sets them up for the bilateral wire.
async fn know_each_other(left: &mut Mob, right: &mut Mob) {
    let directory = meerkat_meta::contact_directory::ContactDirectory::from_toml(&format!(
        "[mobs]\n\"{}\" = \"inproc\"\n\"{}\" = \"inproc\"",
        left.mob_id, right.mob_id
    ))
    .expect("contact directory");
    left.runtime.set_contact_directory(directory.clone());
    right.runtime.set_contact_directory(directory);
    left.runtime.register_peer_runtime(&right.runtime).await;
    right.runtime.register_peer_runtime(&left.runtime).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edge_of_the_bilateral_wire_wakes_a_member_that_is_not_built() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-bilateral-a", "alice", true).await;
    let mut right = Mob::start_persistent(root.path(), "wake-bilateral-b", "bob", true).await;
    let bob = right.peer_info().await;
    know_each_other(&mut left, &mut right).await;
    Box::pin(
        left.runtime
            .wire_cross_mob("alice", "bob", "wake-bilateral-b"),
    )
    .await
    .expect("the bilateral wire");
    assert_delivers(&mut left, &mut right, "wake-bilateral before a-to-b").await;

    // Bob's runtime alone is stopped and built again: bob is not built.
    // Alice's runtime was never restarted, so what it holds for the edge is
    // what the bilateral wire itself recorded.
    let mut right = right.built_again(root.path()).await;
    assert_eq!(right.routed_names(), Vec::<String>::new());
    assert_delivers(&mut left, &mut right, "wake-bilateral after a-to-b").await;
    assert_eq!(right.route_to(&bob.comms_name), Some(bob.peer_id.clone()));
    assert_delivers(&mut right, &mut left, "wake-bilateral after b-to-a").await;

    left.stop().await;
    right.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_bilateral_unwire_a_one_sided_wire_wakes_nobody() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-unlinked-a", "alice", true).await;
    let mut right = Mob::start_persistent(root.path(), "wake-unlinked-b", "bob", true).await;
    let bob = keyed(&right.peer_info().await);
    know_each_other(&mut left, &mut right).await;
    Box::pin(
        left.runtime
            .wire_cross_mob("alice", "bob", "wake-unlinked-b"),
    )
    .await
    .expect("the bilateral wire");
    assert_eq!(left.trusts().await, vec![bob.comms_name.clone()]);
    assert_eq!(right.trusts().await.len(), 1);

    // Both runtimes are built again: each reads the edge from its rows.
    let left = left.built_again(root.path()).await;
    let right = right.built_again(root.path()).await;
    let (mut left, mut right) = (left, right);
    know_each_other(&mut left, &mut right).await;

    // The bilateral unwire removes the edge from both mobs' rows.
    Box::pin(
        left.runtime
            .unwire_cross_mob("alice", "bob", "wake-unlinked-b"),
    )
    .await
    .expect("the bilateral unwire");
    assert!(left.trusts().await.is_empty());
    assert!(right.trusts().await.is_empty());
    assert_eq!(
        right.routed_names(),
        Vec::<String>::new(),
        "the unwire built nobody"
    );

    // Alice's side alone is wired again. Bob's mob holds no edge to alice:
    // nobody is woken, and alice's send is refused as a peer that is not
    // found.
    left.wire_local(&bob).await.expect("alice's side alone");
    assert_wakes_nobody(&mut left, &mut right, &bob, "wake-unlinked a-to-b").await;

    left.stop().await;
    right.stop().await;
}

/// A third runtime that starts and stops in the process leaves the route
/// between two others where it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_third_runtime_starting_and_stopping_leaves_the_route_between_two_others() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-third-a", "alice", true).await;
    let mut right = Mob::start_persistent(root.path(), "wake-third-b", "bob", true).await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");
    assert_eq!(left.route_to(&bob.comms_name), Some(bob.peer_id.clone()));
    assert_eq!(
        right.route_to(&alice.comms_name),
        Some(alice.peer_id.clone())
    );

    let third = Mob::start_persistent(root.path(), "wake-third-c", "carol", true).await;
    assert_eq!(left.route_to(&bob.comms_name), Some(bob.peer_id.clone()));
    assert_eq!(
        right.route_to(&alice.comms_name),
        Some(alice.peer_id.clone())
    );
    assert_delivers(&mut left, &mut right, "wake-third a-to-b").await;
    third.stop().await;
    assert_eq!(left.route_to(&bob.comms_name), Some(bob.peer_id.clone()));
    assert_eq!(
        right.route_to(&alice.comms_name),
        Some(alice.peer_id.clone())
    );
    assert_delivers(&mut right, &mut left, "wake-third b-to-a").await;

    left.stop().await;
    right.stop().await;
}

/// A route that is already there may lead to an inbox that is gone. The
/// identical wire again points it at the member's inbox of now.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_identical_wire_again_re_points_a_route_that_led_to_an_inbox_that_is_gone() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start(root.path(), "wire-repoint-a", "alice").await;
    let mut right = Mob::start(root.path(), "wire-repoint-b", "bob").await;
    let bob = right.peer_info().await;
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the helper's call sequence");
    assert_delivers(&mut left, &mut right, "repoint before a-to-b").await;

    // Alice's mob still routes bob's name and key, to an inbox that takes
    // nothing (as the inbox of an earlier build of bob would be).
    let key = meerkat_comms::PubKey::new(right.registered_key(&bob.comms_name));
    let registry = InprocRegistry::global();
    assert!(registry.unregister_in_namespace(&left.namespace(), &key));
    let (_gone, gone_sender) = meerkat_comms::Inbox::new_transport_only();
    assert!(
        !registry
            .register_with_meta_in_namespace(
                &left.namespace(),
                bob.comms_name.clone(),
                key,
                gone_sender,
                meerkat_comms::PeerMeta::default(),
            )
            .is_rejected()
    );
    let dropped = left.send(&bob.peer_id, "repoint gone a-to-b").await;
    assert!(
        dropped.contains("classification_rejected"),
        "the planted route leads nowhere: {dropped}"
    );
    assert!(
        !right
            .receives("repoint gone a-to-b", NO_DELIVERY_WAIT)
            .await
    );

    // The identical wire again is accepted and re-points the route.
    sdk_wire_cross_mob(&left, &right)
        .await
        .expect("the identical edge is accepted again");
    assert_delivers(&mut left, &mut right, "repoint after a-to-b").await;

    left.stop().await;
    right.stop().await;
}

// ---------------------------------------------------------------------------
// A route goes with its last link. The bilateral unwire removes the edge
// from both mobs' rows, and the route a forwarded wake installed for that
// edge goes with it: a later one-sided wire finds nothing to deliver
// through, whether the other member is built or not.
// ---------------------------------------------------------------------------

/// The bilateral wire of alice and bob, one delivery each way (each mob's
/// namespace then routes the other mob's member), and the bilateral unwire.
async fn wired_delivered_and_unwired(left: &mut Mob, right: &mut Mob, tag: &str) {
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    know_each_other(left, right).await;
    Box::pin(
        left.runtime
            .wire_cross_mob(&left.member, &right.member, &right.mob_id),
    )
    .await
    .expect("the bilateral wire");
    assert_delivers(left, right, &format!("{tag} before a-to-b")).await;
    assert_delivers(right, left, &format!("{tag} before b-to-a")).await;
    assert_eq!(left.route_to(&bob.comms_name), Some(bob.peer_id.clone()));
    assert_eq!(right.route_to(&alice.comms_name), Some(alice.peer_id));
    Box::pin(
        left.runtime
            .unwire_cross_mob(&left.member, &right.member, &right.mob_id),
    )
    .await
    .expect("the bilateral unwire");
    assert!(left.trusts().await.is_empty());
    assert!(right.trusts().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bilateral_unwire_takes_the_routes_its_edge_delivered_through() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-leftover-a", "alice", true).await;
    let mut right = Mob::start_persistent(root.path(), "wake-leftover-b", "bob", true).await;
    let alice = keyed(&left.peer_info().await);
    let bob = keyed(&right.peer_info().await);
    wired_delivered_and_unwired(&mut left, &mut right, "leftover").await;

    // Neither mob routes the other mob's member any more, and neither
    // holds a wake route for it.
    assert_eq!(
        left.route_to(&bob.comms_name),
        None,
        "alice's mob still routes bob after the unwire"
    );
    assert_eq!(
        right.route_to(&alice.comms_name),
        None,
        "bob's mob still routes alice after the unwire"
    );
    assert!(!left.has_wake_route_for(&bob));
    assert!(!right.has_wake_route_for(&alice));

    // Bob's runtime is built again: bob exists and is not built. Alice's
    // side alone is wired again, with bob's key: her send is refused as a
    // peer that is not found, and nobody is built.
    let mut right = right.built_again(root.path()).await;
    left.wire_local(&bob).await.expect("alice's side alone");
    assert_wakes_nobody(&mut left, &mut right, &bob, "leftover alone a-to-b").await;

    left.stop().await;
    right.stop().await;
}

/// After the bilateral unwire, a Console caller who is no access admin
/// wires alice's side alone, with bob's key, while bob is still built. Her
/// send is refused as a peer that is not found, as for a key nobody holds:
/// it does not reach bob's inbox, so the caller cannot tell a built member
/// from nobody.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_bilateral_unwire_a_keyed_wire_by_a_caller_who_is_no_admin_reaches_nobody() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-unrouted-a", "alice", true).await;
    let mut right = Mob::start_persistent(root.path(), "wake-unrouted-b", "bob", true).await;
    let bob = keyed(&right.peer_info().await);
    wired_delivered_and_unwired(&mut left, &mut right, "unrouted").await;

    let console = console_for_everyone(&mut left);
    console_rpc(
        &console,
        None,
        "meta/cross_mob/wire_local",
        left.wire_params(&bob),
    )
    .await
    .expect("a keyed in-process wire is accepted");
    let result = left.send(&bob.peer_id, "unrouted alone a-to-b").await;
    assert!(
        result.contains(NOT_ROUTABLE),
        "bob's mob holds no edge to alice: the send must be refused as a peer that is not \
         found, built or not: {result}"
    );
    assert!(
        !right
            .receives("unrouted alone a-to-b", NO_DELIVERY_WAIT)
            .await
    );
    assert_eq!(
        left.route_to(&bob.comms_name),
        None,
        "alice's mob routes bob"
    );
    assert_eq!(
        right.routed_names(),
        vec![bob.comms_name.clone()],
        "bob's mob routes more than bob himself"
    );

    drop(console);
    left.stop().await;
    right.stop().await;
}

/// The owner of a wake route that is never asked: it stands for the owner of
/// a member that is seated under a key.
struct SeatedOwner;

impl meerkat_comms::InprocPeerWaker for SeatedOwner {
    fn wake(&self, _request: meerkat_comms::PeerWakeRequest) -> meerkat_comms::PeerWakeFuture {
        Box::pin(std::future::ready(Err(
            meerkat_comms::PeerWakeDenial::NotAMember,
        )))
    }

    fn member_is_waking(&self, _member: &str) -> bool {
        false
    }
}

fn transport_key(peer: &PeerInfo) -> meerkat_comms::PubKey {
    use base64::Engine;
    let key: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(
            peer.pubkey_b64
                .as_deref()
                .expect("peer_info reports the member's key"),
        )
        .expect("base64 key")
        .try_into()
        .expect("a 32-byte key");
    meerkat_comms::PubKey::new(key)
}

/// The bilateral wire refuses a key a member of either wiring mob is seated
/// under, as `wire_local` does, before anything is recorded: that key's wake
/// route in the mob's namespace is its own member's and stays, so the edge
/// could hold no link.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bilateral_wire_refuses_a_key_a_member_of_a_wiring_mob_is_seated_under() {
    let root = tempfile::tempdir().expect("root");
    let mut left = Mob::start_persistent(root.path(), "wake-seated-a", "alice", true).await;
    let mut right = Mob::start_persistent(root.path(), "wake-seated-b", "bob", true).await;
    let alice = left.peer_info().await;
    let bob = right.peer_info().await;
    know_each_other(&mut left, &mut right).await;
    let registry = InprocRegistry::global();
    let seated_owner: Arc<dyn meerkat_comms::InprocPeerWaker> = Arc::new(SeatedOwner);
    let seated = Arc::downgrade(&seated_owner);

    // A member of the mob on one side is seated under the key of the member
    // on the other side: refused in plain words, and neither row changed.
    for (namespace, key, holder) in [
        (
            left.namespace(),
            transport_key(&bob),
            "this mob".to_string(),
        ),
        (
            right.namespace(),
            transport_key(&alice),
            format!("mob {:?}", right.mob_id),
        ),
    ] {
        assert!(registry.publish_wake_route(&namespace, key, "dave", seated.clone()));
        let refusal = Box::pin(left.runtime.wire_cross_mob("alice", "bob", "wake-seated-b"))
            .await
            .expect_err("a member of a wiring mob is seated under that key");
        assert!(
            refusal
                .to_string()
                .contains(&format!("{holder} already holds that key")),
            "the refusal must name the mob that holds the key ({holder}): {refusal}"
        );
        assert!(left.trusts().await.is_empty(), "alice's row changed");
        assert!(right.trusts().await.is_empty(), "bob's row changed");
        assert!(registry.retract_wake_route(&namespace, &key, &seated));
    }

    // With no such member the same wire is accepted and delivers.
    Box::pin(left.runtime.wire_cross_mob("alice", "bob", "wake-seated-b"))
        .await
        .expect("the bilateral wire");
    assert_delivers(&mut left, &mut right, "seated after a-to-b").await;

    left.stop().await;
    right.stop().await;
}
