//! Console projection for agent-tool spawned mob members.
//!
//! Members spawned through the agent-facing mob tools (`mob_spawn_member`,
//! `spawn_member`, `spawn_many_members`, `fork_off`, and `delegate`'s implicit
//! helpers)
//! historically never reached the console: the spawn succeeded, the
//! `initial_message` was delivered to the member, and no frame or identity
//! metadata was ever written under the member's identity. Embedders had to
//! hand-roll kickoff frames server-side to make their workers visible.
//!
//! This module is the spawn-tool-side bridge. After a successful agent-tool
//! spawn the dispatcher hands each spawned member to [`ConsoleSpawnSink`],
//! which:
//!
//! 1. registers the member's runtime-id → identity mapping with the
//!    [`ConsoleEventStore`] so live runtime events aggregate into the same
//!    chat as the kickoff (the identity string must match what
//!    `frame_from_console_event` derives for the member's events);
//! 2. registers spawn labels (`spawned_by`, `via_tool`, plus any labels the
//!    spawn carried) as console identity metadata for sidebar grouping and
//!    ABAC attribute priming;
//! 3. appends the spawn `initial_message` (when present) as a programmatic
//!    `user_input` console event with a deterministic
//!    `spawn-kickoff:{mob_id}:{member_id}:{hash}` event id, so retries and
//!    respawns dedupe to exactly one kickoff frame.
//!
//! A runtime without a console event store simply never installs a sink and
//! the spawn path behaves exactly as before.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::unified_runtime::ConsoleEventStore;

/// Late-binding slot for the console spawn sink. Created when the agent mob
/// tools are installed (no console exists yet at that point) and filled by
/// the unified runtime once its console event store is constructed.
pub(crate) type SharedConsoleSpawnSinkSlot = Arc<std::sync::RwLock<Option<ConsoleSpawnSink>>>;

pub(crate) fn new_console_spawn_sink_slot() -> SharedConsoleSpawnSinkSlot {
    Arc::new(std::sync::RwLock::new(None))
}

/// Console-side label key recording which agent spawned this member.
pub(crate) const SPAWNED_BY_LABEL: &str = "spawned_by";
/// Console-side label key recording which tool performed the spawn.
pub(crate) const VIA_TOOL_LABEL: &str = "via_tool";
/// Console-side label key recording the session of the spawning agent's
/// incarnation, so captured lineage binds the parent incarnation that
/// performed the spawn and not a later member under the same identity.
pub(crate) const SPAWNED_BY_SESSION_LABEL: &str = "spawned_by_session";

/// One spawned member, as seen by the agent-tool spawn path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsoleSpawnSeed {
    /// Target mob, when the spawn path knows it.
    pub(crate) mob_id: Option<String>,
    /// Runtime member id (the mob roster `AgentIdentity`).
    pub(crate) member_id: String,
    /// Console identity for the member's chat. Matches the identity the
    /// console event projection derives for the member's runtime events:
    /// the `agent_identity` label override when the spawn carried one,
    /// otherwise the member id.
    pub(crate) identity: String,
    /// The kickoff content delivered to the member, when present. Either a
    /// JSON string or a content-blocks array, exactly as the spawn carried it.
    pub(crate) initial_message: Option<Value>,
    /// Labels the spawn carried (e.g. `display_name`, `group`, `agent_type`).
    pub(crate) labels: BTreeMap<String, String>,
    /// Identity of the agent that performed the spawn, when known.
    pub(crate) spawned_by: Option<String>,
    /// Session of the spawning agent's incarnation, when known.
    pub(crate) spawned_by_session: Option<String>,
    /// Tool that performed the spawn (`mob_spawn_member`, `delegate`, ...).
    pub(crate) via_tool: String,
}

/// Sink that projects spawned members into the runtime's console event
/// store. Cheap to clone; failure-isolated — console projection must never
/// fail or delay a spawn.
#[derive(Clone)]
pub(crate) struct ConsoleSpawnSink {
    console_events: ConsoleEventStore,
}

impl ConsoleSpawnSink {
    pub(crate) fn new(console_events: ConsoleEventStore) -> Self {
        Self { console_events }
    }

    /// All console identity metadata registered through this sink, keyed by
    /// console identity. Consumed by roster/attribute projections so spawn
    /// labels and lineage survive wholesale cache rebuilds.
    pub(crate) async fn identity_labels_snapshot(
        &self,
    ) -> BTreeMap<String, BTreeMap<String, String>> {
        self.console_events.identity_labels_snapshot().await
    }

    /// Register the member's console identity and append its kickoff event.
    /// Idempotent per `(mob_id, member_id, initial_message)`.
    pub(crate) async fn project_spawned_member(&self, seed: &ConsoleSpawnSeed) {
        let identity = seed.identity.trim();
        let member_id = seed.member_id.trim();
        if identity.is_empty() || member_id.is_empty() {
            return;
        }
        // The runtime-plane identities name the console's own events: the
        // console aggregator exempts them from the roster-visibility gate,
        // so a member registered under one would bypass it. Every creation
        // path refuses them; this projection-side backstop covers a member
        // created before they did.
        if crate::console_contracts::is_runtime_plane_identity(identity) {
            crate::console_contracts::warn_runtime_plane_member_drop(
                member_id,
                None,
                identity,
                "spawn projection",
            );
            return;
        }

        // Live runtime events arrive keyed by runtime ids — `{member}:{gen}`
        // (and `rt:{member}:{gen}` in identity-first packs). Register both
        // prefixes so every event form resolves to the kickoff's identity.
        self.console_events
            .register_runtime_identity(member_id, identity)
            .await;
        self.console_events
            .register_runtime_identity(format!("rt:{member_id}"), identity)
            .await;

        let mut labels = seed.labels.clone();
        // Lineage is runtime-derived truth: ABAC inheritance hangs off
        // `spawned_by`, so a caller-supplied label must never spoof (or
        // erase) who actually performed the spawn.
        match seed.spawned_by.as_deref() {
            Some(spawned_by) => {
                labels.insert(SPAWNED_BY_LABEL.to_string(), spawned_by.to_string());
            }
            None => {
                labels.remove(SPAWNED_BY_LABEL);
                // Registrations merge per key, so a respawn with an unknown
                // spawner must also evict any previously registered lineage.
                self.console_events
                    .unregister_identity_label(identity, SPAWNED_BY_LABEL)
                    .await;
            }
        }
        // The spawning incarnation binds the lineage; without one, a stale
        // binding from an earlier spawn must not stay registered.
        match seed
            .spawned_by
            .as_deref()
            .and(seed.spawned_by_session.as_deref())
            .map(str::trim)
            .filter(|session| !session.is_empty())
        {
            Some(session) => {
                labels.insert(SPAWNED_BY_SESSION_LABEL.to_string(), session.to_string());
            }
            None => {
                labels.remove(SPAWNED_BY_SESSION_LABEL);
                self.console_events
                    .unregister_identity_label(identity, SPAWNED_BY_SESSION_LABEL)
                    .await;
            }
        }
        labels.insert(VIA_TOOL_LABEL.to_string(), seed.via_tool.clone());
        self.console_events
            .register_identity_labels(identity, labels)
            .await;

        let Some(initial_message) = seed.initial_message.as_ref() else {
            return;
        };
        let content = match initial_message {
            Value::String(text) => json!([{ "type": "text", "text": text }]),
            other => other.clone(),
        };
        let mut data = json!({
            "content": content,
            "message": { "role": "user", "content": initial_message },
            "source_event_type": "spawn_initial_message",
            "via_tool": seed.via_tool,
            "member_id": member_id,
        });
        if let Some(object) = data.as_object_mut() {
            if let Some(mob_id) = seed.mob_id.as_deref().filter(|id| !id.trim().is_empty()) {
                object.insert("mob_id".to_string(), json!(mob_id));
            }
            if let Some(parent) = seed.spawned_by.as_deref() {
                object.insert("parent_identity".to_string(), json!(parent));
            }
        }
        // `append_envelope` dedupes by event id against the replay window
        // (ALL_EVENTS_REPLAY_CAP), so retries are exactly-once for as long
        // as the original kickoff is still replayable; a respawn issued
        // after the kickoff scrolls out re-appends, and the downstream log
        // store still collapses it via the identical dedupe key.
        self.console_events
            .append_envelope(crate::console_contracts::ConsoleIdentityEventEnvelope {
                event_id: kickoff_event_id(seed.mob_id.as_deref(), member_id, initial_message),
                interaction_id: None,
                identity: identity.to_string(),
                event_type: "user_input".to_string(),
                timestamp_ms: current_time_ms(),
                data,
            })
            .await;
    }
}

/// Tool names whose successful outcomes spawn mob members that should be
/// projected into the console.
///
/// # This vocabulary is meerkat's, and Meta only restates it
///
/// These names are defined upstream on `AgentMobToolSurface`. Meta has no
/// compile-time access to them - they are private consts in
/// `meerkat-mob-mcp` - so this list is a RESTATEMENT, and a restatement of
/// someone else's vocabulary goes stale silently. When meerkat adds a mob tool
/// that spawns members, nothing here fails: the spawn succeeds, the projection
/// is skipped, and the console simply shows no member. An operator reads that
/// as "the tool did not spawn", which sends them upstream instead of to this
/// list.
///
/// That is not hypothetical. `council` landed in meerkat 0.8.31 as a
/// first-class agent mob tool beside `delegate` and `fork_off`; it is absent
/// here deliberately, because its participants live in a temporary mob that is
/// destroyed before the tool returns, and projecting them would write durable
/// console rows for members that no longer exist.
///
/// [`MOB_SPAWN_TOOL_VOCABULARY`] is the single source for this crate, and
/// `console_spawn_vocabulary_is_pinned` forces a human decision whenever it
/// changes. Neither can detect a NEW upstream tool on their own - closing that
/// needs meerkat to export the surface's tool names, which is an upstream ask
/// rather than something Meta can assert from here.
pub(crate) fn is_console_spawn_tool(name: &str) -> bool {
    MOB_SPAWN_TOOL_VOCABULARY.contains(&name)
}

/// Every meerkat mob tool Meta treats as spawning console-visible members.
///
/// Single source of truth for this crate. Consumers that need a NARROWER set
/// must document why and assert their subset against this list, so a name that
/// exists in neither is a typo rather than a silent no-op.
pub(crate) const MOB_SPAWN_TOOL_VOCABULARY: &[&str] = &[
    "mob_spawn_member",
    "spawn_member",
    "spawn_many_members",
    "delegate",
    "fork_off",
];

/// Identity of the spawning agent derived from its comms name. Mob member
/// comms names are `{mob_id}/{role}/{identity}` (meerkat-mob 0.6); the
/// identity itself may contain `/`, so everything after the second
/// separator belongs to it. A name without separators is already an
/// identity; any other shape is unknown and yields no lineage rather than
/// a guess.
pub(crate) fn spawned_by_from_comms_name(comms_name: &str) -> Option<String> {
    let comms_name = comms_name.trim();
    if comms_name.is_empty() {
        return None;
    }
    if !comms_name.contains('/') {
        return Some(comms_name.to_string());
    }
    let mut parts = comms_name.splitn(3, '/');
    let (_mob, _role) = (parts.next()?, parts.next()?);
    let identity = parts.next()?.trim();
    (!identity.is_empty()).then(|| identity.to_string())
}

/// Strip the runtime-derived lineage keys from labels that did not come
/// from the spawn registry (roster labels, spec labels, wire payloads).
/// ABAC inheritance hangs off `spawned_by`: an unverified claim could mint
/// inherited visibility — or hide a member behind a denied parent — so
/// only the spawn registry may assert these keys.
pub(crate) fn sanitize_unverified_lineage_labels(labels: &mut BTreeMap<String, String>) {
    labels.remove(SPAWNED_BY_LABEL);
    labels.remove(SPAWNED_BY_SESSION_LABEL);
    labels.remove(VIA_TOOL_LABEL);
}

/// One argument record from a spawn-tool call: top-level args or one entry
/// of a `specs`/`members` array.
struct SpawnArgRecord {
    member_id: Option<String>,
    mob_id: Option<String>,
    initial_message: Option<Value>,
    labels: BTreeMap<String, String>,
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn member_id_field(value: &Value) -> Option<String> {
    text_field(value, "member_id")
        .or_else(|| text_field(value, "agent_identity"))
        .or_else(|| text_field(value, "identity"))
}

fn labels_field(value: &Value) -> BTreeMap<String, String> {
    value
        .get("labels")
        .and_then(Value::as_object)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|text| (key.clone(), text.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn spawn_arg_record(value: &Value, default_mob_id: Option<&str>) -> Option<SpawnArgRecord> {
    if !value.is_object() {
        return None;
    }
    let member_id = member_id_field(value);
    let initial_message = value
        .get("initial_message")
        .or_else(|| value.get("task"))
        .filter(|message| !message.is_null())
        .cloned();
    if member_id.is_none() && initial_message.is_none() {
        return None;
    }
    let mut labels = labels_field(value);
    if let Some(display_name) = text_field(value, "display_name") {
        labels
            .entry("display_name".to_string())
            .or_insert(display_name);
    }
    Some(SpawnArgRecord {
        member_id,
        mob_id: text_field(value, "mob_id").or_else(|| default_mob_id.map(ToString::to_string)),
        initial_message,
        labels,
    })
}

fn spawn_arg_records(args: &Value) -> Vec<SpawnArgRecord> {
    let default_mob_id = text_field(args, "mob_id");
    let mut records = Vec::new();
    for key in ["specs", "members"] {
        let Some(values) = args.get(key).and_then(Value::as_array) else {
            continue;
        };
        for value in values {
            if let Some(record) = spawn_arg_record(value, default_mob_id.as_deref()) {
                records.push(record);
            }
        }
    }
    if records.is_empty()
        && let Some(record) = spawn_arg_record(args, default_mob_id.as_deref())
    {
        records.push(record);
    }
    records
}

/// The members a typed batch outcome reports spawned, in its order: Core's
/// `{"results": [...]}`, one entry per spec, where a spawned entry names its
/// member and a failed entry spawned nothing. `None` when the outcome is not
/// that shape.
fn typed_batch_spawned_members(outcome: &Value) -> Option<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct TypedBatchOutcome {
        results: Vec<meerkat_contracts::MobSpawnManyResultEntry>,
    }
    let typed = <TypedBatchOutcome as serde::Deserialize>::deserialize(outcome).ok()?;
    Some(
        typed
            .results
            .into_iter()
            .filter_map(|entry| match entry.result {
                meerkat_contracts::MobSpawnManyResultPayload::Spawned(spawned) => {
                    Some(spawned.agent_identity)
                }
                meerkat_contracts::MobSpawnManyResultPayload::Failed(_) => None,
            })
            .collect(),
    )
}

/// One member reported spawned by the tool outcome.
struct SpawnOutcomeTarget {
    member_id: String,
    mob_id: Option<String>,
}

fn collect_outcome_targets(
    value: &Value,
    default_mob_id: Option<&str>,
    targets: &mut Vec<SpawnOutcomeTarget>,
) {
    if let Some(member_id) = member_id_field(value)
        && !targets.iter().any(|target| target.member_id == member_id)
    {
        targets.push(SpawnOutcomeTarget {
            member_id,
            mob_id: text_field(value, "mob_id").or_else(|| default_mob_id.map(ToString::to_string)),
        });
    }
    for key in ["members", "specs", "spawned", "results"] {
        let Some(values) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        let nested_default = text_field(value, "mob_id");
        for nested in values {
            collect_outcome_targets(
                nested,
                nested_default.as_deref().or(default_mob_id),
                targets,
            );
        }
    }
}

/// The console identity a spawned member's projection registers: its usable
/// `agent_identity` label (the console identity override), otherwise its
/// member id, trimmed as [`ConsoleSpawnSink::project_spawned_member`]
/// registers it. The runtime-plane member checks evaluate this same value.
pub(crate) fn spawn_seed_console_identity(
    member_id: &str,
    labels: &BTreeMap<String, String>,
) -> String {
    crate::member_comms_id::durable_identity_label(labels)
        .unwrap_or(member_id)
        .trim()
        .to_string()
}

/// Extract the spawned members from a successful spawn-tool dispatch.
///
/// `args` is the tool-call argument object (single-record shape or
/// `specs`/`members` arrays); `outcome_text` is the tool result payload.
/// Members reported by the outcome are authoritative (they actually
/// spawned); argument records enrich them with `initial_message`/`task`,
/// labels, and identity overrides. A typed batch outcome (`spawn_many_members`)
/// projects only its spawned entries: a failed entry's argument record never
/// stands in for it. Otherwise, when the outcome carries no parsable member,
/// argument records with explicit member ids are used as fallback.
pub(crate) fn console_spawn_seeds(
    tool_name: &str,
    args: &Value,
    outcome_text: &str,
    spawner_comms_name: Option<&str>,
) -> Vec<ConsoleSpawnSeed> {
    if !is_console_spawn_tool(tool_name) {
        return Vec::new();
    }
    let spawned_by = spawner_comms_name.and_then(spawned_by_from_comms_name);
    let arg_records = spawn_arg_records(args);

    let outcome = serde_json::from_str::<Value>(outcome_text).ok();
    let typed_spawned = outcome.as_ref().and_then(typed_batch_spawned_members);
    let mut outcome_targets = Vec::new();
    if let Some(spawned) = &typed_spawned {
        outcome_targets.extend(spawned.iter().map(|member_id| SpawnOutcomeTarget {
            member_id: member_id.clone(),
            mob_id: None,
        }));
    } else if let Some(outcome) = &outcome {
        collect_outcome_targets(outcome, None, &mut outcome_targets);
    }

    let seed_for = |member_id: String,
                    mob_id: Option<String>,
                    record: Option<&SpawnArgRecord>|
     -> ConsoleSpawnSeed {
        let labels = record
            .map(|record| record.labels.clone())
            .unwrap_or_default();
        let identity = spawn_seed_console_identity(&member_id, &labels);
        ConsoleSpawnSeed {
            mob_id: mob_id
                .or_else(|| record.and_then(|record| record.mob_id.clone()))
                .or_else(|| text_field(args, "mob_id")),
            member_id,
            identity,
            initial_message: record.and_then(|record| record.initial_message.clone()),
            labels,
            spawned_by: spawned_by.clone(),
            spawned_by_session: None,
            via_tool: tool_name.to_string(),
        }
    };

    if typed_spawned.is_some() || !outcome_targets.is_empty() {
        return outcome_targets
            .into_iter()
            .map(|target| {
                let record = arg_records
                    .iter()
                    .find(|record| record.member_id.as_deref() == Some(target.member_id.as_str()))
                    // A single argument record with a tool-generated member id
                    // (delegate) pairs with the single spawned target.
                    .or_else(|| match arg_records.as_slice() {
                        [only] if only.member_id.is_none() => Some(only),
                        _ => None,
                    });
                seed_for(target.member_id, target.mob_id, record)
            })
            .collect();
    }

    arg_records
        .iter()
        .filter_map(|record| {
            let member_id = record.member_id.clone()?;
            Some(seed_for(member_id, record.mob_id.clone(), Some(record)))
        })
        .collect()
}

/// Stable short hash for kickoff dedupe keys.
fn hash_short(input: &str) -> String {
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    digest[..8]
        .iter()
        .fold(String::with_capacity(16), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Merge console-registered spawn labels into a member's projected labels.
/// Roster/member labels win on conflict — except the runtime-derived lineage
/// keys (`spawned_by`, `via_tool`), where the spawn registry is the source
/// of truth: ABAC inheritance hangs off `spawned_by`, so a spec-supplied
/// label must not displace who actually performed the spawn.
pub(crate) fn merge_registered_labels(
    labels: &mut BTreeMap<String, String>,
    registered: &BTreeMap<String, String>,
) {
    for (key, value) in registered {
        if key == SPAWNED_BY_LABEL || key == SPAWNED_BY_SESSION_LABEL || key == VIA_TOOL_LABEL {
            labels.insert(key.clone(), value.clone());
        } else {
            labels.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
}

fn kickoff_event_id(mob_id: Option<&str>, member_id: &str, initial_message: &Value) -> String {
    format!(
        "spawn-kickoff:{}:{}:{}",
        mob_id.unwrap_or("-"),
        member_id,
        hash_short(&initial_message.to_string())
    )
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::{EventEnvelope, UnifiedEvent};

    fn seed(member_id: &str, initial_message: Option<Value>) -> ConsoleSpawnSeed {
        ConsoleSpawnSeed {
            mob_id: Some("ops".to_string()),
            member_id: member_id.to_string(),
            identity: member_id.to_string(),
            initial_message,
            labels: BTreeMap::from([("group".to_string(), "workers".to_string())]),
            spawned_by: Some("ops-lead".to_string()),
            spawned_by_session: None,
            via_tool: "mob_spawn_member".to_string(),
        }
    }

    #[tokio::test]
    async fn projects_kickoff_event_under_member_identity() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());

        sink.project_spawned_member(&seed("worker-3", Some(json!("Find the person"))))
            .await;

        let replay = store.replay_all(None).await.expect("replay");
        let kickoff = replay
            .iter()
            .find(|event| event.event_type == "user_input")
            .expect("kickoff user_input event projected");
        assert_eq!(kickoff.identity, "worker-3");
        assert!(
            kickoff.event_id.starts_with("spawn-kickoff:ops:worker-3:"),
            "deterministic kickoff id, got {}",
            kickoff.event_id
        );
        assert_eq!(kickoff.data["content"][0]["type"], "text");
        assert_eq!(kickoff.data["content"][0]["text"], "Find the person");
        assert_eq!(kickoff.data["message"]["role"], "user");
        assert_eq!(kickoff.data["source_event_type"], "spawn_initial_message");
        assert_eq!(kickoff.data["via_tool"], "mob_spawn_member");
        assert_eq!(kickoff.data["parent_identity"], "ops-lead");
    }

    /// Backstop for spawn paths that skip rpc validation: a seed carrying
    /// the reserved runtime-plane identity must project nothing — no
    /// runtime-id mapping, no kickoff frame — or the member's frames would
    /// ride the aggregator's `_system` visibility/namespacing exemptions.
    #[tokio::test]
    async fn reserved_system_identity_seed_projects_nothing() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        let mut spoofed = seed("worker-3", Some(json!("Find the person")));
        spoofed.identity = crate::console_contracts::SYSTEM_EVENT_IDENTITY.to_string();

        sink.project_spawned_member(&spoofed).await;

        let replay = store.replay_all(None).await.expect("replay");
        assert!(
            !replay.iter().any(|event| event.event_type == "user_input"),
            "no kickoff may project under the reserved identity: {replay:#?}"
        );
        assert!(
            !sink
                .identity_labels_snapshot()
                .await
                .contains_key(crate::console_contracts::SYSTEM_EVENT_IDENTITY),
            "no identity metadata may register under the reserved identity"
        );
    }

    /// The backstop covers both runtime-plane identities: a seed carrying
    /// `__console__` projects nothing either.
    #[tokio::test]
    async fn reserved_console_identity_seed_projects_nothing() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        let mut spoofed = seed("worker-4", Some(json!("Find the person")));
        spoofed.identity = "__console__".to_string();

        sink.project_spawned_member(&spoofed).await;

        let replay = store.replay_all(None).await.expect("replay");
        assert!(
            !replay.iter().any(|event| event.event_type == "user_input"),
            "no kickoff may project under the reserved identity: {replay:#?}"
        );
        assert!(
            !sink
                .identity_labels_snapshot()
                .await
                .contains_key("__console__"),
            "no identity metadata may register under the reserved identity"
        );
    }

    #[tokio::test]
    async fn double_projection_dedupes_to_one_kickoff() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        let seed = seed("worker-3", Some(json!("Find the person")));

        sink.project_spawned_member(&seed).await;
        sink.project_spawned_member(&seed).await;

        let replay = store.replay_all(None).await.expect("replay");
        let kickoffs = replay
            .iter()
            .filter(|event| event.event_type == "user_input")
            .count();
        assert_eq!(kickoffs, 1, "spawn retries must not duplicate the kickoff");
    }

    #[tokio::test]
    async fn no_initial_message_registers_identity_without_chat_frames() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());

        sink.project_spawned_member(&seed("worker-9", None)).await;

        let replay = store.replay_all(None).await.expect("replay");
        assert!(
            replay.iter().all(|event| event.identity != "worker-9"),
            "no initial_message means no chat frames until first activity"
        );
        let labels = store
            .identity_labels("worker-9")
            .await
            .expect("identity labels registered");
        assert_eq!(labels.get("group").map(String::as_str), Some("workers"));
        assert_eq!(
            labels.get(SPAWNED_BY_LABEL).map(String::as_str),
            Some("ops-lead")
        );
        assert_eq!(
            labels.get(VIA_TOOL_LABEL).map(String::as_str),
            Some("mob_spawn_member")
        );
    }

    #[tokio::test]
    async fn registers_runtime_identity_so_live_events_join_the_same_chat() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());

        sink.project_spawned_member(&seed("worker-3", Some(json!("go"))))
            .await;

        // A child runtime id (`{member}:{gen}:{child}`) only resolves through
        // the explicit registration — the `{identity}:{N}` fallback would
        // mis-derive `worker-3:0` as the identity.
        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-live-1".to_string(),
                source: "test".to_string(),
                timestamp_ms: 7,
                event: UnifiedEvent::Agent {
                    agent_id: "worker-3:0:1".to_string(),
                    event_type: "text_delta".to_string(),
                    payload: Some(json!({ "delta": "on it" })),
                },
            })
            .await;

        let replay = store.replay_all(None).await.expect("replay");
        let live = replay
            .iter()
            .find(|event| event.event_id == "evt-live-1")
            .expect("live event projected");
        assert_eq!(
            live.identity, "worker-3",
            "live runtime events must land in the same chat as the kickoff"
        );
    }

    /// The spawn registry binds lineage to the spawning incarnation: the
    /// session that performed the spawn is registered next to `spawned_by`,
    /// a caller-supplied value never is, and a later spawn of the same
    /// identity without a known spawning session evicts the earlier binding.
    #[tokio::test]
    async fn the_spawning_session_is_registered_only_by_the_spawn_registry() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        let mut bound = ConsoleSpawnSeed {
            spawned_by_session: Some("lead-session-1".to_string()),
            ..seed("worker-3", None)
        };
        bound.labels.insert(
            SPAWNED_BY_SESSION_LABEL.to_string(),
            "claimed-session".to_string(),
        );
        sink.project_spawned_member(&bound).await;
        let labels = store
            .identity_labels("worker-3")
            .await
            .expect("identity labels registered");
        assert_eq!(
            labels.get(SPAWNED_BY_SESSION_LABEL).map(String::as_str),
            Some("lead-session-1")
        );

        // Respawned by a spawner whose session is unknown: no stale binding.
        sink.project_spawned_member(&seed("worker-3", None)).await;
        let labels = store
            .identity_labels("worker-3")
            .await
            .expect("identity labels registered");
        assert_eq!(labels.get(SPAWNED_BY_SESSION_LABEL), None);

        // Unverified lineage labels lose the binding along with `spawned_by`.
        let mut roster_labels = BTreeMap::from([
            (SPAWNED_BY_LABEL.to_string(), "victim-agent".to_string()),
            (
                SPAWNED_BY_SESSION_LABEL.to_string(),
                "claimed-session".to_string(),
            ),
        ]);
        sanitize_unverified_lineage_labels(&mut roster_labels);
        assert!(roster_labels.is_empty());
    }

    #[tokio::test]
    async fn caller_labels_cannot_spoof_runtime_derived_lineage() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        let mut spoofed = seed("worker-3", None);
        spoofed
            .labels
            .insert(SPAWNED_BY_LABEL.to_string(), "victim-agent".to_string());
        spoofed
            .labels
            .insert(VIA_TOOL_LABEL.to_string(), "console".to_string());

        sink.project_spawned_member(&spoofed).await;

        let labels = store
            .identity_labels("worker-3")
            .await
            .expect("identity labels registered");
        assert_eq!(
            labels.get(SPAWNED_BY_LABEL).map(String::as_str),
            Some("ops-lead"),
            "ABAC lineage must record the actual spawner, not a caller-supplied label"
        );
        assert_eq!(
            labels.get(VIA_TOOL_LABEL).map(String::as_str),
            Some("mob_spawn_member")
        );

        let unknown_parent = ConsoleSpawnSeed {
            spawned_by: None,
            spawned_by_session: None,
            labels: BTreeMap::from([(SPAWNED_BY_LABEL.to_string(), "victim-agent".to_string())]),
            ..seed("worker-9", None)
        };
        sink.project_spawned_member(&unknown_parent).await;
        let labels = store
            .identity_labels("worker-9")
            .await
            .expect("identity labels registered");
        assert!(
            !labels.contains_key(SPAWNED_BY_LABEL),
            "unknown spawner must not fall back to a caller-supplied lineage claim"
        );
    }

    /// Pins the vocabulary so a change is a DECISION, not a diff nobody reads.
    ///
    /// This cannot detect a new tool added UPSTREAM - meerkat's tool names are
    /// private consts and Meta cannot see them. What it does is force whoever
    /// edits this list to state, in a failing test, that they considered
    /// whether the new tool spawns console-visible members. `council` is the
    /// worked example: it IS a spawn tool upstream and is deliberately absent
    /// here, and that absence should stay a decision rather than becoming an
    /// oversight the next time someone reads the list.
    #[test]
    fn console_spawn_vocabulary_is_pinned() {
        assert_eq!(
            MOB_SPAWN_TOOL_VOCABULARY,
            [
                "mob_spawn_member",
                "spawn_member",
                "spawn_many_members",
                "delegate",
                "fork_off",
            ],
            "the console spawn vocabulary changed. If you ADDED a tool, confirm \
             it spawns members that should be projected AND that its argument \
             shape is handled. If you REMOVED one, confirm nothing still \
             projects through it. `council` is intentionally absent: its \
             participants are destroyed before the tool returns.",
        );
    }

    #[test]
    fn spawn_tools_are_recognized() {
        for tool in [
            "mob_spawn_member",
            "spawn_member",
            "spawn_many_members",
            "delegate",
            "fork_off",
        ] {
            assert!(is_console_spawn_tool(tool), "{tool} spawns members");
        }
        assert!(!is_console_spawn_tool("mob_retire_member"));
        assert!(!is_console_spawn_tool("send_message"));
    }

    #[test]
    fn spawned_by_uses_comms_name_identity_segment() {
        assert_eq!(
            spawned_by_from_comms_name("ops/orchestrator/ops-lead").as_deref(),
            Some("ops-lead")
        );
        assert_eq!(
            spawned_by_from_comms_name("ops-lead").as_deref(),
            Some("ops-lead")
        );
        // Identities may contain `/` (e.g. `people/finder`): the comms-name
        // shape is `{mob}/{role}/{identity}`, so everything after the second
        // separator belongs to the identity.
        assert_eq!(
            spawned_by_from_comms_name("ops/orchestrator/people/finder").as_deref(),
            Some("people/finder")
        );
        // One separator is not the canonical shape — don't guess lineage.
        assert_eq!(spawned_by_from_comms_name("mob/role"), None);
        assert_eq!(spawned_by_from_comms_name(""), None);
        assert_eq!(spawned_by_from_comms_name("mob/role/"), None);
    }

    #[tokio::test]
    async fn respawn_with_unknown_spawner_clears_stale_lineage() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());

        sink.project_spawned_member(&seed("worker-3", None)).await;
        assert_eq!(
            store
                .identity_labels("worker-3")
                .await
                .and_then(|labels| labels.get(SPAWNED_BY_LABEL).cloned())
                .as_deref(),
            Some("ops-lead")
        );

        let unknown_spawner = ConsoleSpawnSeed {
            spawned_by: None,
            spawned_by_session: None,
            via_tool: "spawn_member".to_string(),
            ..seed("worker-3", None)
        };
        sink.project_spawned_member(&unknown_spawner).await;

        let labels = store
            .identity_labels("worker-3")
            .await
            .expect("identity labels registered");
        assert!(
            !labels.contains_key(SPAWNED_BY_LABEL),
            "a respawn with an unknown spawner must not keep stale lineage alive"
        );
        assert_eq!(
            labels.get(VIA_TOOL_LABEL).map(String::as_str),
            Some("spawn_member")
        );
    }

    #[test]
    fn seeds_from_single_spawn_args_and_outcome() {
        let args = json!({
            "mob_id": "ops",
            "profile": "person-worker",
            "member_id": "worker-3",
            "initial_message": "Find the person",
            "labels": { "group": "workers", "display_name": "Worker 3" }
        });
        let outcome = json!({
            "agent_identity": "worker-3",
            "member_ref": "opaque-ref"
        });

        let seeds = console_spawn_seeds(
            "mob_spawn_member",
            &args,
            &outcome.to_string(),
            Some("ops/orchestrator/ops-lead"),
        );

        assert_eq!(seeds.len(), 1);
        let seed = &seeds[0];
        assert_eq!(seed.mob_id.as_deref(), Some("ops"));
        assert_eq!(seed.member_id, "worker-3");
        assert_eq!(seed.identity, "worker-3");
        assert_eq!(seed.initial_message, Some(json!("Find the person")));
        assert_eq!(
            seed.labels.get("group").map(String::as_str),
            Some("workers")
        );
        assert_eq!(
            seed.labels.get("display_name").map(String::as_str),
            Some("Worker 3")
        );
        assert_eq!(seed.spawned_by.as_deref(), Some("ops-lead"));
        assert_eq!(seed.via_tool, "mob_spawn_member");
    }

    #[tokio::test]
    async fn fork_off_projects_retained_child_from_canonical_result() {
        let args = json!({
            "member_id": "durable-investigator",
            "task": "Inspect the ledger"
        });
        let outcome = json!({
            "output": "The ledger is consistent.",
            "mob_id": "ops",
            "source_member_id": "ops-lead",
            "agent_identity": "durable-investigator",
            "member_ref": "opaque-ref",
            "fork_session_id": "fork-session",
            "turn_session_id": "fork-session",
            "bounded_result": {
                "label": "delegation",
                "text": "The ledger is consistent.",
                "status": "completed",
                "truncated": false
            }
        });

        let seeds = console_spawn_seeds(
            "fork_off",
            &args,
            &outcome.to_string(),
            Some("ops/orchestrator/ops-lead"),
        );
        assert_eq!(seeds.len(), 1);
        let seed = &seeds[0];
        assert_eq!(seed.mob_id.as_deref(), Some("ops"));
        assert_eq!(seed.member_id, "durable-investigator");
        assert_eq!(seed.initial_message, Some(json!("Inspect the ledger")));
        assert_eq!(seed.spawned_by.as_deref(), Some("ops-lead"));
        assert_eq!(seed.via_tool, "fork_off");

        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        sink.project_spawned_member(seed).await;
        let labels = store
            .identity_labels("durable-investigator")
            .await
            .expect("retained child is registered in the normal console identity snapshot");
        assert_eq!(
            labels.get(SPAWNED_BY_LABEL).map(String::as_str),
            Some("ops-lead")
        );
        assert_eq!(
            labels.get(VIA_TOOL_LABEL).map(String::as_str),
            Some("fork_off")
        );
        let replay = store.replay_all(None).await.expect("replay");
        assert!(replay.iter().any(|event| {
            event.identity == "durable-investigator"
                && event.data["message"]["content"] == "Inspect the ledger"
        }));
    }

    #[test]
    fn seeds_honor_agent_identity_label_override() {
        let args = json!({
            "mob_id": "ops",
            "member_id": "worker-3",
            "initial_message": "go",
            "labels": { "agent_identity": "people/finder" }
        });
        let outcome = json!({ "agent_identity": "worker-3" });

        let seeds = console_spawn_seeds("mob_spawn_member", &args, &outcome.to_string(), None);

        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].member_id, "worker-3");
        assert_eq!(
            seeds[0].identity, "people/finder",
            "kickoff must land under the same identity the event projection derives"
        );
    }

    /// The outcome `spawn_many_members` returns: Core's typed results, one
    /// entry per spec, in spec order.
    fn spawn_many_outcome(results: Vec<meerkat_contracts::MobSpawnManyResultEntry>) -> String {
        json!({ "results": results }).to_string()
    }

    fn spawned_row(member_id: &str) -> meerkat_contracts::MobSpawnManyResultEntry {
        meerkat_contracts::MobSpawnManyResultEntry::spawned(
            member_id,
            meerkat_contracts::WireMemberRef::encode("ops", member_id),
        )
    }

    fn failed_row() -> meerkat_contracts::MobSpawnManyResultEntry {
        meerkat_contracts::MobSpawnManyResultEntry::failed(
            meerkat_contracts::MobSpawnManyFailureCause::ProfileNotFound,
            "profile person-worker-x is not defined",
        )
    }

    /// A failed row spawned nothing: only the members Core reports spawned
    /// are projected, never a failed row's spec in its place.
    #[test]
    fn spawn_many_projects_only_the_rows_that_spawned() {
        let args = json!({
            "mob_id": "ops",
            "specs": [
                { "profile": "person-worker", "member_id": "worker-a", "initial_message": "task a" },
                { "profile": "person-worker-x", "member_id": "worker-b", "initial_message": "task b" }
            ]
        });
        let outcome = spawn_many_outcome(vec![spawned_row("worker-a"), failed_row()]);

        let seeds = console_spawn_seeds("spawn_many_members", &args, &outcome, None);

        let projected = seeds
            .iter()
            .map(|seed| {
                (
                    seed.member_id.as_str(),
                    seed.mob_id.as_deref(),
                    seed.initial_message.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            projected,
            vec![("worker-a", Some("ops"), Some(json!("task a")))]
        );
    }

    /// A batch whose every row failed projects no member at all.
    #[test]
    fn spawn_many_with_every_row_failed_projects_nothing() {
        let args = json!({
            "mob_id": "ops",
            "specs": [
                { "profile": "person-worker-x", "member_id": "worker-a", "initial_message": "task a" }
            ]
        });
        let outcome = spawn_many_outcome(vec![failed_row()]);

        let seeds = console_spawn_seeds("spawn_many_members", &args, &outcome, None);

        let projected = seeds
            .iter()
            .map(|seed| seed.member_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(projected, Vec::<&str>::new());
    }

    #[test]
    fn seeds_from_specs_array_enrich_each_member() {
        let args = json!({
            "mob_id": "ops",
            "specs": [
                {
                    "profile": "person-worker",
                    "member_id": "worker-a",
                    "initial_message": "task a",
                    "labels": { "group": "workers" }
                },
                {
                    "profile": "person-worker",
                    "member_id": "worker-b",
                    "initial_message": "task b"
                }
            ]
        });
        let outcome = spawn_many_outcome(vec![spawned_row("worker-a"), spawned_row("worker-b")]);

        let seeds = console_spawn_seeds("spawn_many_members", &args, &outcome, None);

        assert_eq!(seeds.len(), 2);
        let worker_a = seeds
            .iter()
            .find(|seed| seed.member_id == "worker-a")
            .unwrap();
        assert_eq!(worker_a.initial_message, Some(json!("task a")));
        assert_eq!(
            worker_a.labels.get("group").map(String::as_str),
            Some("workers")
        );
        let worker_b = seeds
            .iter()
            .find(|seed| seed.member_id == "worker-b")
            .unwrap();
        assert_eq!(worker_b.initial_message, Some(json!("task b")));
    }

    #[test]
    fn delegate_seed_pairs_generated_member_with_task() {
        let args = json!({ "task": "Review the diff" });
        let outcome = json!({
            "agent_identity": "helper-3f2a",
            "member_ref": "opaque",
            "mob_id": "implicit-1",
            "wired": true
        });

        let seeds = console_spawn_seeds(
            "delegate",
            &args,
            &outcome.to_string(),
            Some("ops/orchestrator/ops-lead"),
        );

        assert_eq!(seeds.len(), 1);
        let seed = &seeds[0];
        assert_eq!(seed.member_id, "helper-3f2a");
        assert_eq!(seed.mob_id.as_deref(), Some("implicit-1"));
        assert_eq!(seed.initial_message, Some(json!("Review the diff")));
        assert_eq!(seed.spawned_by.as_deref(), Some("ops-lead"));
        assert_eq!(seed.via_tool, "delegate");
    }

    #[test]
    fn unparsable_outcome_falls_back_to_args_members() {
        let args = json!({
            "mob_id": "ops",
            "member_id": "worker-3",
            "initial_message": "go"
        });

        let seeds = console_spawn_seeds("mob_spawn_member", &args, "spawned ok", None);

        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].member_id, "worker-3");
    }

    #[test]
    fn merge_registered_labels_fills_gaps_without_overriding() {
        let mut labels = BTreeMap::from([
            ("group".to_string(), "roster-group".to_string()),
            ("role".to_string(), "worker".to_string()),
        ]);
        let registered = BTreeMap::from([
            ("group".to_string(), "registry-group".to_string()),
            (SPAWNED_BY_LABEL.to_string(), "ops-lead".to_string()),
        ]);

        merge_registered_labels(&mut labels, &registered);

        assert_eq!(
            labels.get("group").map(String::as_str),
            Some("roster-group")
        );
        assert_eq!(
            labels.get(SPAWNED_BY_LABEL).map(String::as_str),
            Some("ops-lead")
        );
        assert_eq!(labels.get("role").map(String::as_str), Some("worker"));
    }

    #[test]
    fn merge_registered_labels_keeps_lineage_keys_runtime_derived() {
        let mut labels = BTreeMap::from([
            (SPAWNED_BY_LABEL.to_string(), "victim-agent".to_string()),
            (VIA_TOOL_LABEL.to_string(), "console".to_string()),
        ]);
        let registered = BTreeMap::from([
            (SPAWNED_BY_LABEL.to_string(), "ops-lead".to_string()),
            (VIA_TOOL_LABEL.to_string(), "mob_spawn_member".to_string()),
        ]);

        merge_registered_labels(&mut labels, &registered);

        assert_eq!(
            labels.get(SPAWNED_BY_LABEL).map(String::as_str),
            Some("ops-lead"),
            "registry lineage must displace spec-supplied spawned_by claims"
        );
        assert_eq!(
            labels.get(VIA_TOOL_LABEL).map(String::as_str),
            Some("mob_spawn_member")
        );
    }
}
