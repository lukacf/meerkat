//! A trust store never holds its own key: `delegate` rows over the
//! production runtime-backed session service.
//!
//! Under network comms (tcp, uds) every session runtime of a realm is built
//! on the realm's key, so a delegate helper has the delegating session's
//! peer id. A trust add of one's descriptor to the other's store would be
//! filed under that store's own peer id.
//!
//! The helper is built beside the delegating session and is not wired to
//! it: neither store takes the other's descriptor, the helper's mob holds
//! no edge to the delegating session, `delegate` reports `wired: false`,
//! and the helper still runs its task and is retired. Under the default
//! inproc comms the helper has a key of its own and is wired as before.
//!
//! The rows read tables and repeat one trust add; no row sends anything to
//! another session. Each observed state is printed on a line starting with
//! `OWNKEY|<row>|`.
#![cfg(not(target_arch = "wasm32"))]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::print_stderr
)]

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    GeneratedCommsTrustAuthoritySourceKind as TrustSource, TrustedPeerDescriptor,
};
use meerkat_core::{CommsRuntimeMode, ContentInput, HandlingMode, Message};
use meerkat_mob::{
    AgentIdentity, MobBackendKind, MobDefinition, MobId, ProfileBinding, ProfileName,
};
use meerkat_mob_mcp::MobMcpState;
use support::{ScriptedCouncilClient, ScriptedTurn, TurnGate, last_user_text, participant_profile};

const MODEL: &str = "claude-haiku-4-5-20251001";
const DELEGATE_TASK: &str = "OWN-KEY-DELEGATE-TASK";
const HELPER_ANSWER: &str = "OWN-KEY-HELPER-ANSWER";

fn helper_id(index: usize) -> String {
    format!("helper-{index}")
}

fn helper_task(index: usize) -> String {
    format!("OWN-KEY-HELPER-TASK-{index}#")
}

fn delegate_call_id(index: usize) -> String {
    format!("call-delegate-{index}")
}

/// One peer row as a runtime reports it: name, peer id, address.
type Row = (String, String, String);

/// What one comms runtime holds at one moment.
#[derive(Debug, Clone)]
struct RuntimeState {
    peer_id: Option<String>,
    comms_name: Option<String>,
    advertised_address: Option<String>,
    /// The peers listing, with each entry's source.
    peers: Vec<(Row, String)>,
    /// The public trust projection, or why it could not be read.
    public_trust: Result<Vec<Row>, String>,
    /// The rows filed under the reciprocal-trust source.
    reciprocal_trust: Result<Vec<Row>, String>,
    /// The rows filed under the external-peer wiring source.
    external_wiring_trust: Result<Vec<Row>, String>,
}

impl RuntimeState {
    fn trust_rows(&self) -> &[Row] {
        self.public_trust
            .as_deref()
            .expect("the public trust projection is readable")
    }

    fn trust_rows_for_peer_id(&self, peer_id: &str) -> Vec<&Row> {
        self.trust_rows()
            .iter()
            .filter(|row| row.1 == peer_id)
            .collect()
    }

    fn lists_peer_id(&self, peer_id: &str) -> bool {
        self.peers.iter().any(|(row, _)| row.1 == peer_id)
    }

    fn peer_id(&self) -> &str {
        self.peer_id.as_deref().expect("the runtime has a peer id")
    }

    fn address(&self) -> &str {
        self.advertised_address
            .as_deref()
            .expect("the runtime advertises an address")
    }

    /// The trust row another runtime holds for this one when it took this
    /// runtime's name, peer id and advertised address.
    fn as_row(&self) -> Row {
        (
            self.comms_name.clone().expect("the runtime has a name"),
            self.peer_id().to_string(),
            self.address().to_string(),
        )
    }

    /// No trust row of any source, public or by source, and no listed peer.
    fn assert_holds_no_trust_row(&self, context: &str) {
        assert!(self.trust_rows().is_empty(), "{context}: {self:?}");
        assert_eq!(self.reciprocal_trust, Ok(Vec::new()), "{context}: {self:?}");
        assert_eq!(
            self.external_wiring_trust,
            Ok(Vec::new()),
            "{context}: {self:?}"
        );
        assert!(self.peers.is_empty(), "{context}: {self:?}");
    }
}

fn descriptor_rows(
    read: Result<Vec<TrustedPeerDescriptor>, meerkat_core::agent::CommsCapabilityError>,
) -> Result<Vec<Row>, String> {
    read.map(|rows| {
        let mut rows: Vec<Row> = rows
            .into_iter()
            .map(|row| {
                (
                    row.name.to_string(),
                    row.peer_id.to_string(),
                    row.address.to_string(),
                )
            })
            .collect();
        rows.sort();
        rows
    })
    .map_err(|error| format!("{error:?}"))
}

async fn read_runtime(row: &str, label: &str, runtime: &Arc<dyn CoreCommsRuntime>) -> RuntimeState {
    let mut peers: Vec<(Row, String)> = runtime
        .peers()
        .await
        .into_iter()
        .map(|entry| {
            (
                (
                    entry.name.to_string(),
                    entry.peer_id.to_string(),
                    entry.address.to_string(),
                ),
                entry.source.as_str().to_string(),
            )
        })
        .collect();
    peers.sort();
    let state = RuntimeState {
        peer_id: runtime.peer_id().map(|peer_id| peer_id.to_string()),
        comms_name: runtime.comms_name(),
        advertised_address: runtime.advertised_address(),
        peers,
        public_trust: descriptor_rows(runtime.public_trusted_peer_projection_snapshot().await),
        reciprocal_trust: descriptor_rows(
            runtime
                .trusted_peer_projection_snapshot_for_source(
                    TrustSource::MobMachineExternalPeerReciprocalTrust,
                )
                .await,
        ),
        external_wiring_trust: descriptor_rows(
            runtime
                .trusted_peer_projection_snapshot_for_source(
                    TrustSource::MobMachineExternalPeerTrustWiring,
                )
                .await,
        ),
    };
    eprintln!(
        "OWNKEY|{row}|{label}|peer_id={:?} comms_name={:?} advertised_address={:?}",
        state.peer_id, state.comms_name, state.advertised_address
    );
    eprintln!("OWNKEY|{row}|{label}|peers_listing={:?}", state.peers);
    eprintln!("OWNKEY|{row}|{label}|public_trust={:?}", state.public_trust);
    eprintln!(
        "OWNKEY|{row}|{label}|trust_rows_by_source reciprocal={:?} external_wiring={:?}",
        state.reciprocal_trust, state.external_wiring_trust
    );
    state
}

/// One member that mounts comms and the agent mob tools.
fn member_definition(mob_id: &MobId, description: &str) -> MobDefinition {
    let mut profile = participant_profile(description);
    profile.model = MODEL.to_string();
    profile.tools.mob = true;
    // The row drives the member's own turn from outside the mob.
    profile.external_addressable = true;
    let mut definition = MobDefinition::explicit(mob_id.clone());
    definition.profiles.insert(
        ProfileName::from("participant"),
        ProfileBinding::Inline(Box::new(profile)),
    );
    definition
}

/// The tool-result text of each call in `messages`, keyed by call id.
fn tool_results(messages: &[Message]) -> BTreeMap<String, String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .map(|result| (result.tool_use_id.clone(), result.text_content()))
        .collect()
}

/// The delegating session calls `delegate` once per helper, one after the
/// other, and then answers; each helper's turn waits at its own gate, then
/// answers its task.
fn script(
    delegate_results: Arc<Mutex<BTreeMap<String, String>>>,
    gates: Vec<Arc<TurnGate>>,
) -> impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static {
    move |request| {
        let user = last_user_text(request);
        for (index, gate) in gates.iter().enumerate() {
            if user.contains(&helper_task(index)) {
                return ScriptedTurn::Gated(Arc::clone(gate), HELPER_ANSWER.to_string());
            }
        }
        if !user.contains(DELEGATE_TASK) {
            return ScriptedTurn::Text("ok".to_string());
        }
        let results = tool_results(&request.messages);
        *delegate_results.lock().unwrap() = results.clone();
        for index in 0..gates.len() {
            if results.contains_key(&delegate_call_id(index)) {
                continue;
            }
            return ScriptedTurn::ToolCall {
                id: delegate_call_id(index),
                name: "delegate".to_string(),
                args: serde_json::json!({
                    "task": helper_task(index),
                    "member_id": helper_id(index),
                    "result_label": "helper_result",
                    "max_text_bytes": 4096,
                    "tooling": {
                        "mode": "profile",
                        "source": {
                            "type": "inline",
                            "model": MODEL,
                            "tools": { "comms": true }
                        }
                    },
                }),
            };
        }
        ScriptedTurn::Text("delegated".to_string())
    }
}

/// What a row asks of one run of the delegating turn.
struct Plan {
    row: &'static str,
    config: meerkat::Config,
    /// How many helpers the delegating turn asks for, one after the other.
    helpers: usize,
}

/// What the run showed of one helper.
#[derive(Debug, Default)]
struct HelperObserved {
    /// Whether the helper's own model turn started: it was built and its
    /// work admitted.
    turn_started: bool,
    /// The helper's peer id as its mob's roster holds it, while it is live.
    roster_peer_id: Option<String>,
    /// Whether, while the helper is live, its mob holds an external edge
    /// from the helper to the delegating session.
    external_edge_to_parent_while_live: Option<bool>,
    live: Option<RuntimeState>,
    parent_while_live: Option<RuntimeState>,
    /// The row's own repeat, while the helper is live, of the reciprocal
    /// add the tool made for it (the same mob handle call with the same key
    /// and target runtime, the helper's descriptor rebuilt from its runtime
    /// the same way). `Err` carries the error's debug form.
    repeated_reciprocal_add: Option<Result<(), String>>,
    parent_after_repeat: Option<RuntimeState>,
    /// The `delegate` call's result as the delegating model got it back.
    delegate_result: Option<serde_json::Value>,
    /// Whether the helper is still in its mob's roster after the turn.
    in_roster_after: Option<bool>,
}

impl HelperObserved {
    fn wired(&self) -> Option<bool> {
        self.delegate_result.as_ref()?.get("wired")?.as_bool()
    }

    fn error_message(&self) -> Option<&str> {
        self.delegate_result.as_ref()?.get("message")?.as_str()
    }

    /// The helper was built, ran its task to its answer, `delegate` returned
    /// that answer as a success, and the helper was retired.
    fn assert_ran_and_was_retired(&self, context: &str) {
        assert!(
            self.turn_started,
            "{context}: the helper is built and runs its turn"
        );
        let result = self.delegate_result.as_ref().expect("delegate result");
        assert!(
            result.get("error").is_none(),
            "{context}: delegate returns a success, not a tool error: {result}"
        );
        assert!(
            result.to_string().contains(HELPER_ANSWER),
            "{context}: delegate returns the helper's answer: {result}"
        );
        assert_eq!(
            self.in_roster_after,
            Some(false),
            "{context}: the helper is retired after its turn"
        );
    }
}

/// What one run of the delegating turn showed.
#[derive(Debug, Default)]
struct Observed {
    /// `Err` when the delegating member itself could not be built.
    parent_spawn: Option<Result<(), String>>,
    parent_before: Option<RuntimeState>,
    helpers: Vec<HelperObserved>,
    turn_outcome: Option<Result<(), String>>,
    parent_after: Option<RuntimeState>,
}

async fn member_runtime(
    state: &MobMcpState,
    mob_id: &MobId,
    member: &str,
) -> Option<Arc<dyn CoreCommsRuntime>> {
    let session = state
        .handle_for(mob_id)
        .await
        .ok()?
        .resolve_bridge_session_id(&AgentIdentity::from(member))
        .await?;
    state.session_service().comms_runtime(&session).await
}

/// The handle of the delegating session's implicit delegation mob.
async fn implicit_mob_handle(
    state: &MobMcpState,
    mob_id: &MobId,
) -> Option<(MobId, meerkat_mob::MobHandle)> {
    let parent_session = state
        .handle_for(mob_id)
        .await
        .ok()?
        .resolve_bridge_session_id(&AgentIdentity::from("parent"))
        .await?;
    let implicit = state
        .find_implicit_mob_for_bridge_session(&parent_session.to_string())
        .await?;
    let handle = state.handle_for(&implicit).await.ok()?;
    Some((implicit, handle))
}

/// What the helper's mob holds of it at one moment.
struct HelperRosterRead {
    in_roster: bool,
    roster_peer_id: Option<String>,
    /// Whether the roster entry holds an external edge named `parent_name`.
    external_edge_to_parent: bool,
    runtime: Option<Arc<dyn CoreCommsRuntime>>,
}

async fn helper_reads(
    row: &str,
    state: &MobMcpState,
    mob_id: &MobId,
    index: usize,
    parent_name: &str,
) -> Option<HelperRosterRead> {
    let label = helper_id(index);
    let Some((implicit, handle)) = implicit_mob_handle(state, mob_id).await else {
        eprintln!("OWNKEY|{row}|{label}|no implicit delegation mob found");
        return None;
    };
    let identity = AgentIdentity::from(helper_id(index));
    let roster = handle.roster().await;
    let entry = roster.get_by_identity(&identity);
    let in_roster = entry.is_some();
    let roster_peer_id = entry
        .and_then(|entry| entry.peer_id())
        .map(|peer_id| peer_id.to_string());
    let external_edge_to_parent =
        entry.is_some_and(|entry| entry.external_peer_descriptor(parent_name).is_some());
    eprintln!(
        "OWNKEY|{row}|{label}|implicit_mob={implicit} in_roster={in_roster} roster_peer_id={roster_peer_id:?} external_edge_to_parent={external_edge_to_parent}",
    );
    drop(roster);
    let runtime = match handle.resolve_bridge_session_id(&identity).await {
        Some(session) => state.session_service().comms_runtime(&session).await,
        None => None,
    };
    Some(HelperRosterRead {
        in_roster,
        roster_peer_id,
        external_edge_to_parent,
        runtime,
    })
}

/// Repeats, while the helper is live, the reciprocal add `delegate` made
/// for it: the same mob handle call with the same key and target runtime,
/// and the helper's descriptor built from its runtime the same way (its
/// public key and its advertised address). A trust add only: nothing is
/// sent.
async fn repeat_reciprocal_add(
    row: &str,
    state: &MobMcpState,
    mob_id: &MobId,
    index: usize,
    parent: &Arc<dyn CoreCommsRuntime>,
    helper: &Arc<dyn CoreCommsRuntime>,
) -> Option<Result<(), String>> {
    let label = helper_id(index);
    let (_, handle) = implicit_mob_handle(state, mob_id).await?;
    let descriptor = TrustedPeerDescriptor::unsigned_with_pubkey(
        helper.comms_name()?,
        helper.peer_id()?.to_string(),
        helper.public_key_bytes()?,
        helper.advertised_address()?,
    )
    .expect("the helper's descriptor");
    let result = handle
        .apply_external_peer_reciprocal_trust(
            &AgentIdentity::from(helper_id(index)),
            &parent.comms_name()?,
            Arc::clone(parent),
            descriptor,
        )
        .await;
    eprintln!("OWNKEY|{row}|{label}|repeated_reciprocal_add={result:?}");
    Some(result.map_err(|error| format!("{error:?}")))
}

/// Runs the delegating turn under `plan` and reads every state.
async fn observe(plan: Plan) -> Observed {
    let Plan {
        row,
        config,
        helpers,
    } = plan;
    eprintln!(
        "OWNKEY|{row}|config|mode={:?} address={:?} helpers={helpers}",
        config.comms.mode, config.comms.address
    );
    let mut observed = Observed::default();
    observed
        .helpers
        .resize_with(helpers, HelperObserved::default);
    let temp = tempfile::tempdir().expect("temp dir");
    let delegate_results = Arc::new(Mutex::new(BTreeMap::new()));
    let gates: Vec<Arc<TurnGate>> = (0..helpers).map(|_| TurnGate::new()).collect();
    let client = Arc::new(ScriptedCouncilClient::new(script(
        Arc::clone(&delegate_results),
        gates.clone(),
    )));
    let state =
        support::agent_mob_tools_state_with_config(temp.path(), client, config, |state| state);
    let mob_id = MobId::from(format!("own-key-{}", uuid::Uuid::new_v4().simple()));
    state
        .mob_create_definition(member_definition(&mob_id, "delegating parent"))
        .await
        .expect("create the mob");

    let spawn = state
        .mob_spawn(
            &mob_id,
            ProfileName::from("participant"),
            AgentIdentity::from("parent"),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .map(|_| ())
        .map_err(|error| format!("{error:?}"));
    eprintln!("OWNKEY|{row}|parent|spawn={spawn:?}");
    observed.parent_spawn = Some(spawn);
    if matches!(observed.parent_spawn, Some(Err(_))) {
        let _ = state.mob_destroy(&mob_id).await;
        return observed;
    }
    let parent = member_runtime(&state, &mob_id, "parent")
        .await
        .expect("the delegating member's comms runtime");
    let parent_before = read_runtime(row, "parent_before_delegate", &parent).await;
    let parent_name = parent_before
        .comms_name
        .clone()
        .expect("the delegating member has a comms name");
    observed.parent_before = Some(parent_before);

    let turn = state
        .handle_for(&mob_id)
        .await
        .expect("mob handle")
        .member(&AgentIdentity::from("parent"))
        .await
        .expect("member handle")
        .start_turn(
            ContentInput::Text(DELEGATE_TASK.to_string()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("the delegating turn is admitted");
    let mut turn_wait = Box::pin(turn.wait());
    let mut turn_result = None;
    let mut handled = vec![false; helpers];
    let deadline = tokio::time::Instant::now() + Duration::from_mins(3);
    while turn_result.is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the delegating turn neither ended nor reached a helper gate"
        );
        let waiting = (0..helpers).find(|index| !handled[*index] && gates[*index].entered() > 0);
        let Some(index) = waiting else {
            tokio::select! {
                result = &mut turn_wait => turn_result = Some(result),
                () = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
            continue;
        };
        // This helper's own model turn is waiting at its gate: it was
        // built, wired (or not) and given its work.
        handled[index] = true;
        let label = helper_id(index);
        let helper = &mut observed.helpers[index];
        helper.turn_started = true;
        let read = helper_reads(row, &state, &mob_id, index, &parent_name).await;
        let mut helper_runtime = None;
        if let Some(read) = read {
            helper.roster_peer_id = read.roster_peer_id;
            helper.external_edge_to_parent_while_live = Some(read.external_edge_to_parent);
            match &read.runtime {
                Some(runtime) => {
                    helper.live = Some(read_runtime(row, &format!("{label}_live"), runtime).await);
                }
                None => eprintln!("OWNKEY|{row}|{label}_live|no comms runtime"),
            }
            helper_runtime = read.runtime;
        }
        helper.parent_while_live =
            Some(read_runtime(row, &format!("parent_while_{label}_live"), &parent).await);
        if let Some(runtime) = &helper_runtime {
            helper.repeated_reciprocal_add =
                repeat_reciprocal_add(row, &state, &mob_id, index, &parent, runtime).await;
            helper.parent_after_repeat = Some(
                read_runtime(
                    row,
                    &format!("parent_after_repeated_add_for_{label}"),
                    &parent,
                )
                .await,
            );
        }
        gates[index].open();
    }
    let turn_result = turn_result
        .expect("the delegating turn ended")
        .map(|_| ())
        .map_err(|error| format!("{error:?}"));
    eprintln!("OWNKEY|{row}|parent|turn_outcome={turn_result:?}");
    observed.turn_outcome = Some(turn_result);
    let results = delegate_results.lock().unwrap().clone();
    for (index, helper) in observed.helpers.iter_mut().enumerate() {
        let text = results.get(&delegate_call_id(index)).cloned();
        eprintln!(
            "OWNKEY|{row}|{}|turn_started={} delegate_result={text:?}",
            helper_id(index),
            helper.turn_started
        );
        helper.delegate_result =
            text.map(|text| serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)));
        helper.in_roster_after = helper_reads(row, &state, &mob_id, index, &parent_name)
            .await
            .map(|read| read.in_roster);
    }
    observed.parent_after = Some(read_runtime(row, "parent_after_delegate", &parent).await);

    let _ = state.mob_destroy(&mob_id).await;
    observed
}

/// A loopback port nothing listens on, reserved by binding port 0 and
/// releasing it.
fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("reserve a loopback port")
        .local_addr()
        .expect("reserved listener local addr")
        .port()
}

fn config_with(mode: CommsRuntimeMode, address: Option<String>) -> meerkat::Config {
    let mut config = meerkat::Config::default();
    config.comms.mode = mode;
    config.comms.address = address;
    config
}

/// The delegate row under network comms: every helper is built on the
/// realm's key, so its peer id is the delegating session's own. The
/// helper's descriptor is not added to the delegating session's store:
/// `delegate` reports `wired: false`, the delegating session's store holds
/// no row under its own peer id (nor any row) while the helper is live,
/// after the row's own repeat of the add, and after the helper is retired,
/// and each helper still runs its task and is retired.
///
/// The helper's own store is asserted by
/// `assert_the_helpers_own_store_holds_no_row_and_its_mob_no_edge`.
fn assert_same_key_helpers_are_not_wired(row: &str, observed: &Observed) {
    assert_eq!(observed.parent_spawn, Some(Ok(())), "{row}");
    assert_eq!(observed.turn_outcome, Some(Ok(())), "{row}");
    let before = observed.parent_before.as_ref().expect("parent state");
    let own_peer_id = before.peer_id();
    before.assert_holds_no_trust_row(&format!("{row}: before delegate"));

    for (index, helper) in observed.helpers.iter().enumerate() {
        let context = format!("{row}: {}", helper_id(index));
        let live = helper.live.as_ref().expect("the helper was live");
        assert_eq!(
            helper.roster_peer_id.as_deref(),
            Some(own_peer_id),
            "{context}"
        );
        assert_eq!(
            live.peer_id(),
            own_peer_id,
            "{context}: the helper's runtime is built on the realm's key, as its parent's is"
        );
        assert_eq!(
            helper.wired(),
            Some(false),
            "{context}: delegate reports the helper not wired: {:?}",
            helper.delegate_result
        );
        // The delegating session's store does not hold the helper's
        // descriptor, which carries the delegating session's own peer id.
        let while_live = helper.parent_while_live.as_ref().expect("parent state");
        assert!(
            while_live.trust_rows_for_peer_id(own_peer_id).is_empty(),
            "{context}: the delegating session's store holds no row under its own peer id: {while_live:?}"
        );
        while_live.assert_holds_no_trust_row(&format!("{context}: the delegating session's store"));
        // The same add again, by the mob handle call the tool would make:
        // refused, and nothing is stored. (The add's own typed refusal is
        // the authority's, shown by its unit rows; a helper that was not
        // wired has no edge for the mob to authorize the add from.)
        assert!(
            helper
                .repeated_reciprocal_add
                .as_ref()
                .is_some_and(Result::is_err),
            "{context}: the reciprocal add is refused, got {:?}",
            helper.repeated_reciprocal_add
        );
        let after_repeat = helper.parent_after_repeat.as_ref().expect("parent state");
        after_repeat.assert_holds_no_trust_row(&format!(
            "{context}: the delegating session's store after the refused add"
        ));
        helper.assert_ran_and_was_retired(&context);
    }

    let after = observed.parent_after.as_ref().expect("parent state");
    assert!(
        after.trust_rows_for_peer_id(own_peer_id).is_empty(),
        "{row}: after the helpers are retired the delegating session's store holds no row under its own peer id: {after:?}"
    );
    after.assert_holds_no_trust_row(&format!("{row}: after delegate"));
}

/// Under one key `delegate` does not wire the helper to the delegating
/// session at all: the helper's own store holds no row (its parent's
/// descriptor would carry the helper's own peer id), and the helper's mob
/// holds no edge to the delegating session.
fn assert_the_helpers_own_store_holds_no_row_and_its_mob_no_edge(row: &str, observed: &Observed) {
    let before = observed.parent_before.as_ref().expect("parent state");
    let own_peer_id = before.peer_id();
    for (index, helper) in observed.helpers.iter().enumerate() {
        let context = format!("{row}: {}", helper_id(index));
        let live = helper.live.as_ref().expect("the helper was live");
        assert_eq!(live.peer_id(), own_peer_id, "{context}: one key");
        assert!(
            live.trust_rows_for_peer_id(own_peer_id).is_empty(),
            "{context}: the helper's store holds no row under its own peer id: {live:?}"
        );
        live.assert_holds_no_trust_row(&format!("{context}: the helper's own store"));
        assert_eq!(
            helper.external_edge_to_parent_while_live,
            Some(false),
            "{context}: the helper's mob holds no edge to the delegating session"
        );
    }
}

/// Tcp comms on port 0: each listener takes its own port, so helpers are
/// built beside the delegating session, each on the realm's key. `delegate`
/// does not wire two runtimes on one key: neither store takes the other's
/// descriptor, which would carry its own peer id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_port_zero_delegate_helper_on_the_same_key_is_not_wired() {
    let observed = observe(Plan {
        row: "tcp_port_zero",
        config: config_with(CommsRuntimeMode::Tcp, Some("127.0.0.1:0".to_string())),
        helpers: 2,
    })
    .await;
    eprintln!("OWNKEY|tcp_port_zero|summary|{observed:#?}");

    let before = observed.parent_before.as_ref().expect("parent state");
    assert!(before.address().starts_with("tcp://127.0.0.1:"));
    for helper in &observed.helpers {
        let live = helper.live.as_ref().expect("the helper was live");
        assert_ne!(live.address(), before.address(), "its own port");
    }
    assert_same_key_helpers_are_not_wired("tcp_port_zero", &observed);
    assert_the_helpers_own_store_holds_no_row_and_its_mob_no_edge("tcp_port_zero", &observed);
}

/// Uds comms on one configured socket path: helpers are built beside the
/// delegating session on the realm's key and at the same advertised
/// address. `delegate` does not wire two runtimes on one key: neither store
/// takes the other's descriptor, which would carry its own peer id.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uds_delegate_helper_on_the_same_key_is_not_wired() {
    let socket_dir = tempfile::tempdir().expect("socket dir");
    let address = socket_dir.path().join("comms.sock");
    let observed = observe(Plan {
        row: "uds",
        config: config_with(
            CommsRuntimeMode::Uds,
            Some(address.to_string_lossy().into_owned()),
        ),
        helpers: 2,
    })
    .await;
    eprintln!("OWNKEY|uds|summary|{observed:#?}");

    let before = observed.parent_before.as_ref().expect("parent state");
    assert!(before.address().starts_with("uds://"));
    for helper in &observed.helpers {
        let live = helper.live.as_ref().expect("the helper was live");
        assert_eq!(live.address(), before.address(), "the one socket path");
    }
    assert_same_key_helpers_are_not_wired("uds", &observed);
    assert_the_helpers_own_store_holds_no_row_and_its_mob_no_edge("uds", &observed);
}

/// Control: under the default inproc comms each helper has a key of its
/// own. It is built and wired, the delegating session trusts and lists it
/// while it is live, and it runs its task and is retired.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inproc_control_delegate_helper_with_its_own_key_is_wired() {
    let observed = observe(Plan {
        row: "inproc",
        config: config_with(CommsRuntimeMode::Inproc, None),
        helpers: 1,
    })
    .await;
    eprintln!("OWNKEY|inproc|summary|{observed:#?}");

    assert_eq!(observed.parent_spawn, Some(Ok(())));
    assert_eq!(observed.turn_outcome, Some(Ok(())));
    let before = observed.parent_before.as_ref().expect("parent state");
    let parent = observed.parent_after.as_ref().expect("parent state");
    let helper = &observed.helpers[0];
    let live = helper.live.as_ref().expect("the helper was live");
    assert_ne!(
        live.peer_id(),
        parent.peer_id(),
        "under inproc comms the helper has a key of its own"
    );
    assert_eq!(
        helper.wired(),
        Some(true),
        "delegate reports the helper wired: {:?}",
        helper.delegate_result
    );
    assert_eq!(helper.external_edge_to_parent_while_live, Some(true));
    assert_eq!(
        helper.repeated_reciprocal_add,
        Some(Ok(())),
        "the reciprocal add of a helper on its own key is admitted"
    );
    // While the helper is live it trusts its parent, and the delegating
    // session trusts and lists the helper.
    assert_eq!(live.trust_rows_for_peer_id(before.peer_id()).len(), 1);
    let while_live = helper.parent_while_live.as_ref().expect("parent state");
    assert_eq!(while_live.trust_rows(), [live.as_row()]);
    assert!(while_live.lists_peer_id(live.peer_id()));
    helper.assert_ran_and_was_retired("inproc");
}

/// Control: tcp comms on one configured loopback port. The delegating
/// session's runtime listens on it, so no second session runtime of the
/// realm can be built: `delegate` returns a tool error naming the listener
/// failure, the helper never exists, and the delegating session's store and
/// peers listing stay empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_fixed_port_control_delegate_helper_is_never_built() {
    let address = format!("127.0.0.1:{}", free_loopback_port());
    let observed = observe(Plan {
        row: "tcp_fixed_port",
        config: config_with(CommsRuntimeMode::Tcp, Some(address)),
        helpers: 1,
    })
    .await;
    eprintln!("OWNKEY|tcp_fixed_port|summary|{observed:#?}");

    const LISTENER_FAILURE: &str = "Comms runtime failed: Failed to start comms listeners: \
                                    Listener error: Address already in use";
    assert_eq!(observed.parent_spawn, Some(Ok(())));
    assert_eq!(observed.turn_outcome, Some(Ok(())));
    let helper = &observed.helpers[0];
    assert!(!helper.turn_started, "the helper is never built");
    assert!(helper.live.is_none());
    let result = helper.delegate_result.as_ref().expect("delegate result");
    assert_eq!(result["error"], "execution_failed", "{result}");
    assert!(
        helper
            .error_message()
            .is_some_and(|message| message.contains(LISTENER_FAILURE)),
        "{result}"
    );
    let parent = observed.parent_after.as_ref().expect("parent state");
    parent.assert_holds_no_trust_row("tcp_fixed_port: after delegate");
}
