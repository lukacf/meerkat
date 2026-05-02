//! §13 InputState — per-input data shell.
//!
//! Canonical lifecycle truth for every input lives in the MeerkatMachine DSL
//! (`input_phases`, `input_run_associations`, `input_boundary_sequences` plus
//! the `QueueAccepted` / `StageForRun` / `RecordBoundarySeq` / etc.
//! transitions). This module owns ONLY the per-input shell metadata that has
//! no DSL home today: a history log, timestamps, policy snapshot, durability
//! class, idempotency key, and the cached payload needed to rebuild queued
//! work after recovery.
//!
//! `InputState` still caches terminal outcome and attempt count for persistence,
//! recovery normalization, and compatibility reads, but the authoritative live
//! copies now live in the DSL's typed terminal/attempt maps.

use chrono::{DateTime, Utc};
use meerkat_core::lifecycle::{InputId, RunId};
use serde::{Deserialize, Serialize, de};

use crate::identifiers::PolicyVersion;
use crate::ingress_types::RuntimeInputSemantics;
use crate::input::Input;
use crate::policy::PolicyDecision;

/// The lifecycle state of an input — mirrors the DSL's `input_phases` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputLifecycleState {
    Accepted,
    Queued,
    Staged,
    Applied,
    AppliedPendingConsumption,
    Consumed,
    Superseded,
    Coalesced,
    Abandoned,
}

impl InputLifecycleState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Consumed | Self::Superseded | Self::Coalesced | Self::Abandoned
        )
    }
}

/// Why an input was abandoned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputAbandonReason {
    Retired,
    Reset,
    Stopped,
    Destroyed,
    Cancelled,
    MaxAttemptsExhausted { attempts: u32 },
}

/// Terminal outcome cache for an input.
///
/// The authoritative live copy is split across the DSL's typed terminal maps.
/// The shell keeps this typed cache for persistence and compatibility reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputTerminalOutcome {
    Consumed,
    Superseded { superseded_by: InputId },
    Coalesced { aggregate_id: InputId },
    Abandoned { reason: InputAbandonReason },
}

/// A single entry in the input's state history (shell bookkeeping).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputStateHistoryEntry {
    pub timestamp: DateTime<Utc>,
    pub from: InputLifecycleState,
    pub to: InputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Snapshot of the policy that was applied to this input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySnapshot {
    pub version: PolicyVersion,
    pub decision: PolicyDecision,
}

/// How a derived input can be reconstructed after crash recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReconstructionSource {
    Projection {
        rule_id: String,
        source_event_id: String,
    },
    Coalescing {
        source_input_ids: Vec<InputId>,
    },
}

/// An event on an input's state (for event sourcing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputStateEvent {
    pub timestamp: DateTime<Utc>,
    pub state: InputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Maximum stage → rollback cycles before the shell chooses Abandon instead
/// of RollbackStaged at the callsite. Shell retry-policy mechanic; not a DSL
/// guard.
pub const MAX_STAGE_ATTEMPTS: u32 = 3;

/// DSL-owned lifecycle projection for an input.
///
/// Carries the fields that are authoritative in the MeerkatMachine DSL
/// (`input_phases`, `input_run_associations`, `input_boundary_sequences`,
/// `input_terminal_kind` + `input_superseded_by` / `input_aggregate_id` /
/// `input_abandon_reason` / `input_abandon_attempt_count`, and
/// `input_attempt_counts`) so they can travel alongside a persisted
/// [`InputState`] at the store boundary, where no live DSL is available to
/// query. Inside a running driver, these values are always read from the DSL
/// directly, never from the seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputStateSeed {
    pub phase: InputLifecycleState,
    pub last_run_id: Option<RunId>,
    pub last_boundary_sequence: Option<u64>,
    pub terminal_outcome: Option<InputTerminalOutcome>,
    pub attempt_count: u32,
}

impl InputStateSeed {
    /// Freshly-accepted input: no run association, no boundary sequence,
    /// no terminal outcome, zero attempts.
    pub fn new_accepted() -> Self {
        Self {
            phase: InputLifecycleState::Accepted,
            last_run_id: None,
            last_boundary_sequence: None,
            terminal_outcome: None,
            attempt_count: 0,
        }
    }
}

/// Persisted bundle: shell [`InputState`] plus its [`InputStateSeed`].
///
/// Used at the store boundary so the DSL-owned fields survive persistence
/// without being re-shadowed onto `InputState` itself. Recovery treats the
/// seed as a durable witness and re-enters the recovered facts through typed
/// machine inputs; it does not hydrate DSL state directly from this bundle.
#[derive(Debug, Clone)]
pub struct StoredInputState {
    pub state: InputState,
    pub seed: InputStateSeed,
}

impl StoredInputState {
    /// Convenience: freshly-accepted bundle.
    pub fn new_accepted(input_id: InputId) -> Self {
        Self {
            state: InputState::new_accepted(input_id),
            seed: InputStateSeed::new_accepted(),
        }
    }
}

/// Per-input shell data. Plain fields, no hidden state machine.
///
/// All DSL-owned lifecycle fields (`phase`, `last_run_id`,
/// `last_boundary_sequence`, `terminal_outcome`, `attempt_count`) are
/// authoritative in the DSL. Live code reads them via
/// `EphemeralRuntimeDriver::input_phase` / `input_last_run_id` /
/// `input_last_boundary_sequence` / `input_terminal_outcome` /
/// `input_attempt_count`. Persistence callsites serialize them via
/// [`InputStateSeed`] bundled on [`StoredInputState`].
#[derive(Debug, Clone)]
pub struct InputState {
    pub input_id: InputId,
    /// Compatibility cache of the DSL-owned terminal outcome metadata.
    pub terminal_outcome: Option<InputTerminalOutcome>,
    /// Compatibility cache of the DSL-owned attempt count.
    pub attempt_count: u32,
    pub history: Vec<InputStateHistoryEntry>,
    pub updated_at: DateTime<Utc>,
    pub policy: Option<PolicySnapshot>,
    /// Runtime-stamped run semantics captured at admission and persisted so
    /// recovery does not reclassify execution kind from payload shape.
    pub runtime_semantics: Option<RuntimeInputSemantics>,
    pub durability: Option<crate::input::InputDurability>,
    pub idempotency_key: Option<crate::identifiers::IdempotencyKey>,
    pub recovery_count: u32,
    pub reconstruction_source: Option<ReconstructionSource>,
    pub persisted_input: Option<Input>,
    pub created_at: DateTime<Utc>,
}

impl InputState {
    /// Create a fresh InputState. Paired DSL state starts in the `Accepted`
    /// phase via [`InputStateSeed::new_accepted`]; callers that need the
    /// bundle use [`StoredInputState::new_accepted`].
    pub fn new_accepted(input_id: InputId) -> Self {
        let now = Utc::now();
        Self {
            input_id,
            terminal_outcome: None,
            attempt_count: 0,
            history: Vec::new(),
            updated_at: now,
            policy: None,
            runtime_semantics: None,
            durability: None,
            idempotency_key: None,
            recovery_count: 0,
            reconstruction_source: None,
            persisted_input: None,
            created_at: now,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.terminal_outcome.is_some()
    }

    pub fn terminal_outcome(&self) -> Option<&InputTerminalOutcome> {
        self.terminal_outcome.as_ref()
    }

    pub fn history(&self) -> &[InputStateHistoryEntry] {
        &self.history
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    pub fn attempt_count(&self) -> u32 {
        self.attempt_count
    }
}

// ---------------------------------------------------------------------------
// Custom Serialize / Deserialize — preserves the on-disk wire format
// ---------------------------------------------------------------------------
//
// `InputStateSerde` is the on-disk contract exercised by
// `recovery_contract`, `recovery_replay`, and `driver_persistent` tests.
// Field names, types, defaults, and `skip_serializing_if` markers are kept
// verbatim from the pre-5G/1 release. Since `InputState` no longer owns the
// three DSL-authoritative fields, serialization flows through
// [`StoredInputState`] where shell + seed can be bundled into the wire
// struct.

fn deserialize_persisted_input<'de, D>(deserializer: D) -> Result<Option<Input>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(input) = Option::<serde_json::Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let input = migrate_legacy_persisted_input(input);
    serde_json::from_value(input)
        .map(Some)
        .map_err(de::Error::custom)
}

fn migrate_legacy_persisted_input(mut input: serde_json::Value) -> serde_json::Value {
    if let Some(metadata) = input
        .as_object_mut()
        .and_then(|object| object.get_mut("turn_metadata"))
    {
        migrate_legacy_persisted_turn_metadata(metadata);
    }
    input
}

fn migrate_legacy_persisted_turn_metadata(metadata: &mut serde_json::Value) {
    let Some(object) = metadata.as_object_mut() else {
        return;
    };

    migrate_legacy_clear_flag(object, "clear_provider_params", "provider_params");
    migrate_legacy_clear_flag(object, "clear_connection_ref", "connection_ref");

    let provider = object
        .get("provider")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    if let Some(provider_params) = object.get_mut("provider_params") {
        migrate_legacy_provider_params(provider_params, &provider);
    }
    if let Some(connection_ref) = object.get_mut("connection_ref") {
        wrap_legacy_object_override(connection_ref);
    }
    if let Some(keep_alive) = object.get_mut("keep_alive") {
        migrate_legacy_keep_alive_override(keep_alive);
    }
}

fn migrate_legacy_clear_flag(
    metadata: &mut serde_json::Map<String, serde_json::Value>,
    flag: &'static str,
    target: &'static str,
) {
    let Some(value) = metadata.remove(flag) else {
        return;
    };
    if value.as_bool() == Some(true) && !metadata.contains_key(target) {
        metadata.insert(target.to_string(), serde_json::json!({ "action": "clear" }));
    }
}

fn migrate_legacy_provider_params(params: &mut serde_json::Value, provider: &str) {
    let Some(object) = params.as_object() else {
        return;
    };
    if object.contains_key("action") {
        return;
    }

    let projected =
        meerkat_core::lifecycle::run_primitive::ProviderParamsOverride::from_legacy_provider_value(
            provider, params,
        );
    let wire = meerkat_contracts::wire::runtime::WireProviderParamsOverride::from(projected);
    if let Ok(value) = serde_json::to_value(wire) {
        *params = serde_json::json!({
            "action": "set",
            "value": value,
        });
    }
}

fn wrap_legacy_object_override(value: &mut serde_json::Value) {
    let Some(object) = value.as_object() else {
        return;
    };
    if object.contains_key("action") {
        return;
    }

    let legacy = std::mem::take(value);
    *value = serde_json::json!({
        "action": "set",
        "value": legacy,
    });
}

fn migrate_legacy_keep_alive_override(keep_alive: &mut serde_json::Value) {
    if let Some(requests_keep_alive) = keep_alive.as_bool() {
        *keep_alive = if requests_keep_alive {
            serde_json::json!({
                "action": "set",
                "value": {
                    "ttl_secs": 30,
                    "policy": "pinned",
                },
            })
        } else {
            serde_json::json!({ "action": "clear" })
        };
        return;
    }

    let Some(object) = keep_alive.as_object_mut() else {
        return;
    };
    if object.contains_key("action") {
        if object.get("action").and_then(serde_json::Value::as_str) == Some("set")
            && let Some(value) = object.get_mut("value")
        {
            migrate_legacy_keep_alive_policy(value);
        }
        return;
    }

    migrate_legacy_keep_alive_policy(keep_alive);
    let legacy = std::mem::take(keep_alive);
    *keep_alive = serde_json::json!({
        "action": "set",
        "value": legacy,
    });
}

fn migrate_legacy_keep_alive_policy(policy: &mut serde_json::Value) {
    let Some(object) = policy.as_object_mut() else {
        return;
    };
    if let Some(ttl) = object.remove("ttl") {
        object.entry("ttl_secs").or_insert(ttl);
    }
}

#[derive(Serialize, Deserialize)]
struct InputStateSerde {
    input_id: InputId,
    current_state: InputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<PolicySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_semantics: Option<RuntimeInputSemantics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_outcome: Option<InputTerminalOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    durability: Option<crate::input::InputDurability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<crate::identifiers::IdempotencyKey>,
    #[serde(default)]
    attempt_count: u32,
    #[serde(default)]
    recovery_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<InputStateHistoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reconstruction_source: Option<ReconstructionSource>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_persisted_input"
    )]
    persisted_input: Option<Input>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_boundary_sequence: Option<u64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl Serialize for StoredInputState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let helper = InputStateSerde {
            input_id: self.state.input_id.clone(),
            current_state: self.seed.phase,
            policy: self.state.policy.clone(),
            runtime_semantics: self.state.runtime_semantics,
            terminal_outcome: self.seed.terminal_outcome.clone(),
            durability: self.state.durability,
            idempotency_key: self.state.idempotency_key.clone(),
            attempt_count: self.seed.attempt_count,
            recovery_count: self.state.recovery_count,
            history: self.state.history.clone(),
            reconstruction_source: self.state.reconstruction_source.clone(),
            persisted_input: self.state.persisted_input.clone(),
            last_run_id: self.seed.last_run_id.clone(),
            last_boundary_sequence: self.seed.last_boundary_sequence,
            created_at: self.state.created_at,
            updated_at: self.state.updated_at,
        };
        helper.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StoredInputState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let helper = InputStateSerde::deserialize(deserializer)?;
        let state = InputState {
            input_id: helper.input_id,
            terminal_outcome: helper.terminal_outcome.clone(),
            attempt_count: helper.attempt_count,
            history: helper.history,
            updated_at: helper.updated_at,
            policy: helper.policy,
            runtime_semantics: helper.runtime_semantics,
            durability: helper.durability,
            idempotency_key: helper.idempotency_key,
            recovery_count: helper.recovery_count,
            reconstruction_source: helper.reconstruction_source,
            persisted_input: helper.persisted_input,
            created_at: helper.created_at,
        };
        let seed = InputStateSeed {
            phase: helper.current_state,
            last_run_id: helper.last_run_id,
            last_boundary_sequence: helper.last_boundary_sequence,
            terminal_outcome: helper.terminal_outcome,
            attempt_count: helper.attempt_count,
        };
        Ok(StoredInputState { state, seed })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::policy::{
        ApplyMode, ConsumePoint, DrainPolicy, QueueMode, RoutingDisposition, WakeMode,
    };
    use meerkat_core::ops::{OpEvent, OperationId};

    #[test]
    fn new_accepted_starts_with_no_shell_history() {
        let id = InputId::new();
        let state = InputState::new_accepted(id.clone());
        assert_eq!(state.input_id, id);
        assert!(state.history.is_empty());
    }

    #[test]
    fn seed_new_accepted_defaults_match_queue_lifecycle() {
        let seed = InputStateSeed::new_accepted();
        assert_eq!(seed.phase, InputLifecycleState::Accepted);
        assert!(seed.last_run_id.is_none());
        assert!(seed.last_boundary_sequence.is_none());
        assert!(seed.terminal_outcome.is_none());
        assert_eq!(seed.attempt_count, 0);
    }

    #[test]
    fn lifecycle_state_terminal() {
        assert!(InputLifecycleState::Consumed.is_terminal());
        assert!(InputLifecycleState::Superseded.is_terminal());
        assert!(InputLifecycleState::Coalesced.is_terminal());
        assert!(InputLifecycleState::Abandoned.is_terminal());

        assert!(!InputLifecycleState::Accepted.is_terminal());
        assert!(!InputLifecycleState::Queued.is_terminal());
        assert!(!InputLifecycleState::Staged.is_terminal());
        assert!(!InputLifecycleState::Applied.is_terminal());
        assert!(!InputLifecycleState::AppliedPendingConsumption.is_terminal());
    }

    #[test]
    fn lifecycle_state_serde() {
        for state in [
            InputLifecycleState::Accepted,
            InputLifecycleState::Queued,
            InputLifecycleState::Staged,
            InputLifecycleState::Applied,
            InputLifecycleState::AppliedPendingConsumption,
            InputLifecycleState::Consumed,
            InputLifecycleState::Superseded,
            InputLifecycleState::Coalesced,
            InputLifecycleState::Abandoned,
        ] {
            let json = serde_json::to_value(state).unwrap();
            let parsed: InputLifecycleState = serde_json::from_value(json).unwrap();
            assert_eq!(state, parsed);
        }
    }

    #[test]
    fn stored_input_state_serde_roundtrip_preserves_fields() {
        let mut state = InputState::new_accepted(InputId::new());
        let policy = PolicyDecision {
            apply_mode: ApplyMode::StageRunStart,
            wake_mode: WakeMode::WakeIfIdle,
            queue_mode: QueueMode::Fifo,
            consume_point: ConsumePoint::OnRunComplete,
            drain_policy: DrainPolicy::QueueNextTurn,
            routing_disposition: RoutingDisposition::Queue,
            record_transcript: true,
            emit_operator_content: true,
            policy_version: PolicyVersion(1),
        };
        state.policy = Some(PolicySnapshot {
            version: PolicyVersion(1),
            decision: policy.clone(),
        });
        state.runtime_semantics = Some(RuntimeInputSemantics::from_policy_and_kind(
            &policy,
            crate::identifiers::InputKind::Prompt,
        ));
        state.history.push(InputStateHistoryEntry {
            timestamp: state.updated_at,
            from: InputLifecycleState::Accepted,
            to: InputLifecycleState::Queued,
            reason: Some("QueueAccepted".into()),
        });
        let bundle = StoredInputState {
            state,
            seed: InputStateSeed {
                phase: InputLifecycleState::Queued,
                last_run_id: None,
                last_boundary_sequence: None,
                terminal_outcome: None,
                attempt_count: 0,
            },
        };

        let json = serde_json::to_value(&bundle).unwrap();
        let parsed: StoredInputState = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.state.input_id, bundle.state.input_id);
        assert_eq!(parsed.seed.phase, bundle.seed.phase);
        assert_eq!(
            parsed.state.runtime_semantics,
            bundle.state.runtime_semantics
        );
        assert_eq!(parsed.state.history.len(), 1);
    }

    #[test]
    fn stored_input_state_deserializes_legacy_persisted_input_tags() {
        let continuation_bundle = StoredInputState {
            state: InputState {
                persisted_input: Some(Input::Continuation(
                    crate::input::ContinuationInput::detached_background_op_completed(),
                )),
                ..InputState::new_accepted(InputId::new())
            },
            seed: InputStateSeed::new_accepted(),
        };
        let mut continuation_json = serde_json::to_value(&continuation_bundle).unwrap();
        continuation_json["persisted_input"]["input_type"] =
            serde_json::Value::String("system_generated".into());
        let parsed: StoredInputState = serde_json::from_value(continuation_json).unwrap();
        assert!(matches!(
            parsed.state.persisted_input,
            Some(Input::Continuation(_))
        ));

        let operation_bundle = StoredInputState {
            state: InputState {
                persisted_input: Some(Input::Operation(crate::input::OperationInput {
                    header: crate::input::InputHeader {
                        id: InputId::new(),
                        timestamp: Utc::now(),
                        source: crate::input::InputOrigin::System,
                        durability: crate::input::InputDurability::Derived,
                        visibility: crate::input::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    operation_id: OperationId::new(),
                    event: OpEvent::Cancelled {
                        id: OperationId::new(),
                    },
                })),
                ..InputState::new_accepted(InputId::new())
            },
            seed: InputStateSeed::new_accepted(),
        };
        let mut operation_json = serde_json::to_value(&operation_bundle).unwrap();
        operation_json["persisted_input"]["input_type"] =
            serde_json::Value::String("projected".into());
        let parsed: StoredInputState = serde_json::from_value(operation_json).unwrap();
        assert!(matches!(
            parsed.state.persisted_input,
            Some(Input::Operation(_))
        ));
    }

    #[test]
    fn stored_input_state_deserializes_legacy_persisted_turn_metadata() {
        use meerkat_core::lifecycle::run_primitive::{KeepAliveMode, TurnMetadataOverride};

        let bundle = StoredInputState {
            state: InputState {
                persisted_input: Some(Input::Prompt(crate::input::PromptInput {
                    header: crate::input::InputHeader {
                        id: InputId::new(),
                        timestamp: Utc::now(),
                        source: crate::input::InputOrigin::Operator,
                        durability: crate::input::InputDurability::Durable,
                        visibility: crate::input::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    text: "recover me".into(),
                    blocks: None,
                    turn_metadata: None,
                    build_only_overrides: None,
                })),
                ..InputState::new_accepted(InputId::new())
            },
            seed: InputStateSeed::new_accepted(),
        };
        let mut json = serde_json::to_value(&bundle).unwrap();
        json["persisted_input"]["turn_metadata"] = serde_json::json!({
            "model": "gpt-legacy",
            "provider": "openai",
            "provider_params": {
                "temperature": 0.2
            },
            "connection_ref": {
                "realm": "dev",
                "binding": "default_openai"
            },
            "keep_alive": {
                "ttl": 90,
                "policy": "policy_driven"
            }
        });

        let parsed: StoredInputState = serde_json::from_value(json)
            .expect("pre-PR persisted turn metadata must load during recovery");
        let metadata = match parsed.state.persisted_input {
            Some(Input::Prompt(input)) => input.turn_metadata.expect("turn metadata"),
            other => panic!("unexpected persisted input: {other:?}"),
        };

        assert_eq!(
            metadata.model.as_ref().map(|model| model.as_str()),
            Some("gpt-legacy")
        );
        assert_eq!(metadata.provider, Some(meerkat_core::Provider::OpenAI));
        assert!(matches!(
            metadata.provider_params.as_ref(),
            Some(TurnMetadataOverride::Set(params)) if params.temperature == Some(0.2)
        ));
        assert!(matches!(
            metadata.connection_ref.as_ref(),
            Some(TurnMetadataOverride::Set(connection_ref))
                if connection_ref.realm.as_str() == "dev"
                    && connection_ref.binding.as_str() == "default_openai"
        ));
        assert!(matches!(
            metadata.keep_alive.as_ref(),
            Some(TurnMetadataOverride::Set(policy))
                if policy.ttl == std::time::Duration::from_secs(90)
                    && policy.policy == KeepAliveMode::PolicyDriven
        ));

        let round_trip = serde_json::to_value(&StoredInputState {
            state: InputState {
                persisted_input: Some(Input::Prompt(crate::input::PromptInput {
                    header: crate::input::InputHeader {
                        id: InputId::new(),
                        timestamp: Utc::now(),
                        source: crate::input::InputOrigin::Operator,
                        durability: crate::input::InputDurability::Durable,
                        visibility: crate::input::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    text: "recover me".into(),
                    blocks: None,
                    turn_metadata: Some(metadata),
                    build_only_overrides: None,
                })),
                ..InputState::new_accepted(InputId::new())
            },
            seed: InputStateSeed::new_accepted(),
        })
        .unwrap();
        assert_eq!(
            round_trip["persisted_input"]["turn_metadata"]["keep_alive"]["value"]["ttl_secs"],
            90
        );
        assert!(
            round_trip["persisted_input"]["turn_metadata"]["keep_alive"]["value"]
                .get("ttl")
                .is_none(),
            "new persisted input serialization must stay on the canonical wire shape: {round_trip}"
        );
    }

    #[test]
    fn stored_input_state_deserializes_legacy_persisted_turn_metadata_clears() {
        use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;

        let bundle = StoredInputState {
            state: InputState {
                persisted_input: Some(Input::Prompt(crate::input::PromptInput {
                    header: crate::input::InputHeader {
                        id: InputId::new(),
                        timestamp: Utc::now(),
                        source: crate::input::InputOrigin::Operator,
                        durability: crate::input::InputDurability::Durable,
                        visibility: crate::input::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    text: "recover me".into(),
                    blocks: None,
                    turn_metadata: None,
                    build_only_overrides: None,
                })),
                ..InputState::new_accepted(InputId::new())
            },
            seed: InputStateSeed::new_accepted(),
        };
        let mut json = serde_json::to_value(&bundle).unwrap();
        json["persisted_input"]["turn_metadata"] = serde_json::json!({
            "clear_provider_params": true,
            "clear_connection_ref": true,
            "keep_alive": false
        });

        let parsed: StoredInputState = serde_json::from_value(json)
            .expect("pre-PR persisted clear metadata must load during recovery");
        let metadata = match parsed.state.persisted_input {
            Some(Input::Prompt(input)) => input.turn_metadata.expect("turn metadata"),
            other => panic!("unexpected persisted input: {other:?}"),
        };

        assert!(matches!(
            metadata.provider_params,
            Some(TurnMetadataOverride::Clear)
        ));
        assert!(matches!(
            metadata.connection_ref,
            Some(TurnMetadataOverride::Clear)
        ));
        assert!(matches!(
            metadata.keep_alive,
            Some(TurnMetadataOverride::Clear)
        ));
    }

    #[test]
    fn abandon_reason_serde() {
        for reason in [
            InputAbandonReason::Retired,
            InputAbandonReason::Reset,
            InputAbandonReason::Destroyed,
            InputAbandonReason::Cancelled,
        ] {
            let json = serde_json::to_value(&reason).unwrap();
            let parsed: InputAbandonReason = serde_json::from_value(json).unwrap();
            assert_eq!(reason, parsed);
        }
    }

    #[test]
    fn terminal_outcome_consumed_serde() {
        let outcome = InputTerminalOutcome::Consumed;
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["outcome_type"], "consumed");
        let parsed: InputTerminalOutcome = serde_json::from_value(json).unwrap();
        assert_eq!(outcome, parsed);
    }

    #[test]
    fn terminal_outcome_superseded_serde() {
        let outcome = InputTerminalOutcome::Superseded {
            superseded_by: InputId::new(),
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["outcome_type"], "superseded");
        let parsed: InputTerminalOutcome = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, InputTerminalOutcome::Superseded { .. }));
    }

    #[test]
    fn terminal_outcome_abandoned_serde() {
        let outcome = InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Retired,
        };
        let json = serde_json::to_value(&outcome).unwrap();
        let parsed: InputTerminalOutcome = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Retired,
            }
        ));
    }

    #[test]
    fn reconstruction_source_serde() {
        let sources = vec![
            ReconstructionSource::Projection {
                rule_id: "rule-1".into(),
                source_event_id: "evt-1".into(),
            },
            ReconstructionSource::Coalescing {
                source_input_ids: vec![InputId::new(), InputId::new()],
            },
        ];
        for source in sources {
            let json = serde_json::to_value(&source).unwrap();
            assert!(json["source_type"].is_string());
            let parsed: ReconstructionSource = serde_json::from_value(json).unwrap();
            let _ = parsed;
        }
    }

    #[test]
    fn input_state_event_serde() {
        let event = InputStateEvent {
            timestamp: Utc::now(),
            state: InputLifecycleState::Queued,
            detail: Some("queued for processing".into()),
        };
        let json = serde_json::to_value(&event).unwrap();
        let parsed: InputStateEvent = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.state, InputLifecycleState::Queued);
    }
}
