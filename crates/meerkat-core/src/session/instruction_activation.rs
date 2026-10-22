//! Typed chronological instruction activation on the ordered Session transcript.

use super::Session;
use crate::types::{
    InstructionActivationId, InstructionActivationIdentity, InstructionContentDigest,
    InstructionKey, InstructionNamespace, InstructionRevisionRef, Message, MessageTimestamp,
    SessionId, SystemMessage, message_timestamp_now,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const INSTRUCTION_ACTIVATION_RENDER_VERSION_V1: u16 = 1;
pub const MAX_INSTRUCTION_BODY_BYTES: usize = 256 * 1024;
pub const MAX_INSTRUCTION_ACTIVATION_LINEAGE_BYTES: usize = 1024 * 1024;

/// Closed compare-and-set predecessor for an instruction activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "kind", content = "activation_id")]
pub enum InstructionActivationExpectation {
    Absent,
    Effective(InstructionActivationId),
}

/// Explicit request to append one immutable instruction revision transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InstructionActivationRequest {
    pub revision: InstructionRevisionRef,
    pub activation_id: InstructionActivationId,
    pub expectation: InstructionActivationExpectation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<InstructionActivationId>,
    pub body: String,
}

/// Durable transcript record for one instruction activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InstructionActivationRecord {
    pub session_id: SessionId,
    pub identity: InstructionActivationIdentity,
    /// Stable zero-based position among typed activation rows in this
    /// transcript lineage. Compaction preserves activation order, so this
    /// ordinal does not change when conversational rows are summarized.
    pub activation_ordinal: usize,
    /// Current durable transcript projection containing this activation.
    /// Compaction may change this witness without changing the activation.
    pub projection_witness: InstructionActivationProjectionWitness,
}

/// Current transcript projection witness for an activation row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InstructionActivationProjectionWitness {
    pub message_index: usize,
    pub transcript_revision: String,
}

/// Canonical per-key activation state derived from the ordered transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InstructionActivationKeyState {
    pub session_id: SessionId,
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_origin_local: Option<InstructionActivationRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chronological_head: Option<InstructionActivationRecord>,
    pub requires_explicit_child_activation: bool,
    pub next_expectation: InstructionActivationExpectation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_supersedes: Option<InstructionActivationId>,
}

/// Bounded read query for durable activation records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InstructionActivationReadQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<InstructionNamespace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<InstructionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(default = "default_activation_read_limit")]
    pub limit: usize,
}

const fn default_activation_read_limit() -> usize {
    100
}

impl Default for InstructionActivationReadQuery {
    fn default() -> Self {
        Self {
            namespace: None,
            key: None,
            offset: None,
            limit: default_activation_read_limit(),
        }
    }
}

/// One page of durable activation records in transcript order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InstructionActivationReadPage {
    pub session_id: SessionId,
    pub records: Vec<InstructionActivationRecord>,
    /// Present when the query names one exact namespace and key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_state: Option<InstructionActivationKeyState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

/// Result of the domain mutation before physical persistence/publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstructionActivationMutation {
    Appended(InstructionActivationRecord),
    Duplicate(InstructionActivationRecord),
}

/// Public result class for one safe-boundary activation command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InstructionActivationDisposition {
    /// The durable row committed and the materialized session projection was
    /// converged before the command completed.
    Applied,
    /// The exact activation was already the effective durable row.
    Duplicate,
}

/// Reproducible receipt returned by the safe-boundary activation facade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InstructionActivationReceipt {
    pub record: InstructionActivationRecord,
    pub disposition: InstructionActivationDisposition,
}

/// Stable safe-boundary refusal classes exposed by runtime surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InstructionActivationAdmissionErrorCode {
    TargetNotMaterialized,
    UnsupportedCurrentLowering,
    LiveChannelOpen,
    SessionBusy,
    DurabilityUnavailable,
    ExternalWriteFenceConflict,
    ExternalWriteFenceBackoff,
}

/// Stable refusal code shared by core and generated surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InstructionActivationErrorCode {
    InvalidRequest,
    DigestMismatch,
    EffectiveActivationConflict,
    ActivationIdentityConflict,
    ImmutableRevisionConflict,
    RecordedButNotEffective,
    InheritedActivationRequiresExplicitActivation,
    MalformedActivationHistory,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstructionActivationError {
    #[error("instruction activation is unsupported: {0}")]
    Unsupported(String),
    #[error("instruction body exceeds {max_bytes} UTF-8 bytes")]
    BodyTooLarge { max_bytes: usize },
    #[error("instruction activation lineage exceeds {max_bytes} retained UTF-8 body bytes")]
    LineageBodyBudgetExceeded { max_bytes: usize },
    #[error("instruction body digest mismatch: declared {declared}, computed {computed}")]
    DigestMismatch {
        declared: InstructionContentDigest,
        computed: InstructionContentDigest,
    },
    #[error("instruction activation identity conflicts with an existing durable request")]
    ActivationIdentityConflict,
    #[error("instruction activation was recorded but is no longer effective")]
    RecordedButNotEffective,
    #[error("instruction revision is already bound to another body or digest")]
    ImmutableRevisionConflict,
    #[error("forked session must explicitly activate inherited instruction keys: {keys:?}")]
    InheritedActivationRequiresExplicitActivation {
        keys: Vec<(InstructionNamespace, InstructionKey)>,
    },
    #[error("instruction activation expected {expected:?}, but current is {actual:?}")]
    EffectiveActivationConflict {
        expected: InstructionActivationExpectation,
        actual: Option<InstructionActivationId>,
    },
    #[error(
        "instruction supersedes {requested:?}, but current chronological predecessor is {actual:?}"
    )]
    SupersedesConflict {
        requested: Option<InstructionActivationId>,
        actual: Option<InstructionActivationId>,
    },
    #[error("instruction activation history is malformed: {0}")]
    MalformedHistory(String),
    #[error("failed to derive activation transcript revision: {0}")]
    Revision(String),
}

impl InstructionActivationError {
    #[must_use]
    pub const fn code(&self) -> InstructionActivationErrorCode {
        match self {
            Self::Unsupported(_)
            | Self::BodyTooLarge { .. }
            | Self::LineageBodyBudgetExceeded { .. }
            | Self::SupersedesConflict { .. } => InstructionActivationErrorCode::InvalidRequest,
            Self::DigestMismatch { .. } => InstructionActivationErrorCode::DigestMismatch,
            Self::ActivationIdentityConflict => {
                InstructionActivationErrorCode::ActivationIdentityConflict
            }
            Self::RecordedButNotEffective => {
                InstructionActivationErrorCode::RecordedButNotEffective
            }
            Self::ImmutableRevisionConflict => {
                InstructionActivationErrorCode::ImmutableRevisionConflict
            }
            Self::InheritedActivationRequiresExplicitActivation { .. } => {
                InstructionActivationErrorCode::InheritedActivationRequiresExplicitActivation
            }
            Self::EffectiveActivationConflict { .. } => {
                InstructionActivationErrorCode::EffectiveActivationConflict
            }
            Self::MalformedHistory(_) | Self::Revision(_) => {
                InstructionActivationErrorCode::MalformedActivationHistory
            }
        }
    }
}

/// Host policy for the activation rows a durable fork child inherits.
///
/// A fork child holds its source's activation rows as historical context and
/// runs no model turn until each inherited key has an origin-local activation
/// in the child. The policy says who appends that activation. It belongs to
/// the host (a mob builder, or an RPC fork request) and is never a tool
/// argument.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ForkInstructionPolicy {
    /// The host activates each inherited key on the child before its first
    /// turn. A fork that leaves no window for that is refused instead (see
    /// [`ForkInstructionPolicy::effective`]).
    #[default]
    RequireExplicitActivation,
    /// The fork owner re-activates, in the child and before the child is
    /// committed, every key that was effective in the source at the fork
    /// boundary.
    ReactivateInherited,
    /// A fork whose child would inherit any activation row is refused before
    /// anything is committed.
    Refuse,
}

/// Whether a fork leaves the host a window to activate inherited keys on the
/// child before the child's first turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ActivationWindow {
    /// The host drives the child's first turn and can activate before it.
    HostDriven,
    /// The fork admits the child's first turn itself, or commits a child the
    /// host cannot activate before that turn.
    None,
}

impl ForkInstructionPolicy {
    /// The policy a fork applies on a path with `window`.
    ///
    /// [`Self::RequireExplicitActivation`] without a window would commit a
    /// child whose first turn must fail, so it refuses before commit instead.
    #[must_use]
    pub const fn effective(self, window: ActivationWindow) -> Self {
        match (self, window) {
            (Self::RequireExplicitActivation, ActivationWindow::None) => Self::Refuse,
            (policy, _) => policy,
        }
    }
}

/// Why a fork that would inherit activation rows was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum ForkInstructionRefusalCause {
    /// The effective policy refuses a child that inherits activation rows.
    /// `reactivate_inherited` carries the source's effective activations into
    /// the child instead.
    PolicyRefuses,
    /// Re-activation was asked for, but the last copied row of a key was not
    /// effective in the source: the source never ran a turn under it.
    NotEffectiveInSource,
    /// Re-activating the effective bodies would exceed the retained
    /// activation-body budget of the lineage.
    LineageBodyBudgetExceeded { max_bytes: usize },
    /// The activation rules refused a re-activation row.
    Activation {
        code: InstructionActivationErrorCode,
    },
    /// The member host that holds a placed source predates the policy and
    /// refused the request that carried it, so nothing was forked there.
    MemberHostUnsupported,
}

/// A durable fork refused before commit because of inherited activation rows.
/// No child session exists when this is returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[error("{}", fork_instruction_refusal_message(.cause, .keys))]
#[non_exhaustive]
pub struct ForkInstructionRefusal {
    pub cause: ForkInstructionRefusalCause,
    /// The inherited keys the refusal concerns, in (namespace, key) order.
    pub keys: Vec<(InstructionNamespace, InstructionKey)>,
}

impl ForkInstructionRefusal {
    #[must_use]
    pub fn new(
        cause: ForkInstructionRefusalCause,
        keys: Vec<(InstructionNamespace, InstructionKey)>,
    ) -> Self {
        Self { cause, keys }
    }
}

fn fork_instruction_refusal_message(
    cause: &ForkInstructionRefusalCause,
    keys: &[(InstructionNamespace, InstructionKey)],
) -> String {
    let keys = keys
        .iter()
        .map(|(namespace, key)| format!("{namespace}/{key}"))
        .collect::<Vec<_>>()
        .join(", ");
    match cause {
        ForkInstructionRefusalCause::PolicyRefuses => format!(
            "fork refused: the child would inherit activated instruction keys [{keys}] and the \
             fork instruction policy refuses them; to carry them into the child, set the host's \
             fork_instruction_policy (inherited_instructions on a session fork call) to \
             reactivate_inherited"
        ),
        ForkInstructionRefusalCause::NotEffectiveInSource => format!(
            "fork refused: instruction keys [{keys}] were inherited but never activated in the \
             source, so there is no effective activation to carry into the child"
        ),
        ForkInstructionRefusalCause::LineageBodyBudgetExceeded { max_bytes } => format!(
            "fork refused: re-activating instruction keys [{keys}] would exceed the {max_bytes} \
             byte retained activation-body budget"
        ),
        ForkInstructionRefusalCause::Activation { code } => {
            format!("fork refused: re-activating instruction keys [{keys}] was refused ({code:?})")
        }
        ForkInstructionRefusalCause::MemberHostUnsupported => "fork refused: the member host \
             that holds the source does not support the fork instruction policy, so \
             reactivate_inherited cannot be carried to it"
            .to_string(),
    }
}

/// Text before the digest of a derived fork re-activation id. Operator
/// presentation only: no code branches on it. A re-activation row is
/// recognized by reproducing it ([`Session::fork_reactivation_suffix_len`]).
const FORK_REACTIVATION_ID_PREFIX: &str = "fork-reactivation:";

/// The activation id of the row that re-activates `head` in the fork child
/// `child`: `fork-reactivation:` and the first 32 hex digits of
/// `sha256(child || 0x00 || head)`. It depends on the child and the head
/// only, so a planned child's bytes are a function of its prefix and its id,
/// and it hashes the child id, so it cannot collide with an id of the lineage
/// it re-activates.
fn fork_reactivation_activation_id(
    child: &SessionId,
    head: &InstructionActivationId,
) -> Result<InstructionActivationId, InstructionActivationError> {
    use sha2::{Digest as _, Sha256};
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hasher = Sha256::new();
    hasher.update(child.to_string().as_bytes());
    hasher.update([0u8]);
    hasher.update(head.as_str().as_bytes());
    let digest = hasher.finalize();
    let mut id = String::from(FORK_REACTIVATION_ID_PREFIX);
    for byte in digest.iter().take(16) {
        id.push(char::from(HEX[usize::from(byte >> 4)]));
        id.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    InstructionActivationId::new(id).map_err(|error| {
        InstructionActivationError::MalformedHistory(format!(
            "derived fork re-activation id is invalid: {error}"
        ))
    })
}

fn message_created_at(message: &Message) -> MessageTimestamp {
    match message {
        Message::System(system) => system.created_at,
        Message::SystemNotice(notice) => notice.created_at,
        Message::User(user) => user.created_at,
        Message::BlockAssistant(assistant) => assistant.created_at,
        Message::ToolResults { created_at, .. } => *created_at,
    }
}

/// Each key a fork child inherited, with its head row at the fork boundary
/// (the key's last copied row) and that row's body, in (namespace, key)
/// order.
type InheritedActivationHeads =
    BTreeMap<(InstructionNamespace, InstructionKey), (InstructionActivationIdentity, String)>;

fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_owned()).to_string()
}

fn render_origin_v1(identity: &InstructionActivationIdentity, body: &str) -> String {
    let supersedes = identity
        .supersedes
        .as_ref()
        .map(|value| json_string(value.as_str()))
        .unwrap_or_else(|| "null".to_string());
    format!(
        "[meerkat-instruction-activation-v1]\nnamespace={}\nkey={}\nrevision={}\nactivation={}\nsupersedes={}\norigin_session_id={}\ncontent_sha256={}\nbody_bytes={}\n\n{}",
        json_string(identity.revision.namespace.as_str()),
        json_string(identity.revision.key.as_str()),
        json_string(identity.revision.revision_id.as_str()),
        json_string(identity.activation_id.as_str()),
        supersedes,
        json_string(&identity.origin_session_id.to_string()),
        identity.revision.content_sha256,
        body.len(),
        body,
    )
}

fn render_inherited_v1(
    identity: &InstructionActivationIdentity,
    child_session_id: &SessionId,
    body: &str,
) -> String {
    let supersedes = identity
        .supersedes
        .as_ref()
        .map(|value| json_string(value.as_str()))
        .unwrap_or_else(|| "null".to_string());
    format!(
        "[meerkat-inherited-instruction-v1]\nnamespace={}\nkey={}\nrevision={}\nactivation={}\nsupersedes={}\norigin_session_id={}\nchild_session_id={}\ncontent_sha256={}\nstatus=historical_only_not_active_in_child\nbody_bytes={}\n\n{}",
        json_string(identity.revision.namespace.as_str()),
        json_string(identity.revision.key.as_str()),
        json_string(identity.revision.revision_id.as_str()),
        json_string(identity.activation_id.as_str()),
        supersedes,
        json_string(&identity.origin_session_id.to_string()),
        json_string(&child_session_id.to_string()),
        identity.revision.content_sha256,
        body.len(),
        body,
    )
}

fn body_from_origin_envelope<'a>(
    identity: &InstructionActivationIdentity,
    content: &'a str,
) -> Result<&'a str, String> {
    if identity.render_version != INSTRUCTION_ACTIVATION_RENDER_VERSION_V1 {
        return Err(format!(
            "unsupported instruction activation render version {}",
            identity.render_version
        ));
    }
    let (_, body) = content
        .split_once("\n\n")
        .ok_or_else(|| "instruction activation envelope has no body delimiter".to_string())?;
    let computed = InstructionContentDigest::for_body(body);
    if computed != identity.revision.content_sha256 {
        return Err(format!(
            "instruction activation body digest mismatch: declared {}, computed {}",
            identity.revision.content_sha256, computed
        ));
    }
    if render_origin_v1(identity, body) != content {
        return Err(
            "instruction activation envelope bytes do not match typed identity".to_string(),
        );
    }
    Ok(body)
}

/// Validate activation envelopes, predecessor chronology, uniqueness, revision
/// immutability, and the bounded retained-body budget.
pub fn validate_instruction_activation_messages(messages: &[Message]) -> Result<(), String> {
    let mut activations = BTreeSet::new();
    let mut revisions = BTreeMap::new();
    let mut predecessors = BTreeMap::new();
    let mut retained_body_bytes = 0usize;
    for message in messages {
        let Message::System(system) = message else {
            continue;
        };
        let semantic_count = usize::from(system.identity.is_some())
            + usize::from(system.prompt_version.is_some())
            + usize::from(system.instruction_activation.is_some());
        if semantic_count > 1 {
            return Err(
                "System message semantic identities must be mutually exclusive".to_string(),
            );
        }
        let Some(identity) = system.instruction_activation.as_ref() else {
            continue;
        };
        let body = body_from_origin_envelope(identity, &system.content)?;
        retained_body_bytes = retained_body_bytes.checked_add(body.len()).ok_or_else(|| {
            "instruction activation retained-body byte count overflowed".to_string()
        })?;
        if retained_body_bytes > MAX_INSTRUCTION_ACTIVATION_LINEAGE_BYTES {
            return Err(format!(
                "instruction activation lineage exceeds {MAX_INSTRUCTION_ACTIVATION_LINEAGE_BYTES} retained UTF-8 body bytes"
            ));
        }
        let lineage_key = (
            identity.revision.namespace.clone(),
            identity.revision.key.clone(),
        );
        let predecessor = predecessors.get(&lineage_key).cloned();
        if identity.supersedes != predecessor {
            return Err(format!(
                "instruction activation {} supersedes {:?}, but chronological predecessor is {:?}",
                identity.activation_id, identity.supersedes, predecessor
            ));
        }
        predecessors.insert(lineage_key, identity.activation_id.clone());
        let activation_key = (
            identity.revision.namespace.clone(),
            identity.revision.key.clone(),
            identity.activation_id.clone(),
        );
        if !activations.insert(activation_key) {
            return Err("instruction activation identity appears more than once".to_string());
        }
        let revision_key = (
            identity.revision.namespace.clone(),
            identity.revision.key.clone(),
            identity.revision.revision_id.clone(),
        );
        let value = (identity.revision.content_sha256.clone(), body.to_string());
        if let Some(existing) = revisions.insert(revision_key, value.clone())
            && existing != value
        {
            return Err("instruction revision is bound to multiple bodies or digests".to_string());
        }
    }
    Ok(())
}

/// Project inherited rows as historical-only envelopes while preserving order.
pub fn materialize_instruction_activation_messages(
    session_id: &SessionId,
    messages: &[Message],
) -> Result<Vec<Message>, String> {
    messages
        .iter()
        .map(|message| {
            let Message::System(system) = message else {
                return Ok(message.clone());
            };
            let Some(identity) = system.instruction_activation.as_ref() else {
                return Ok(message.clone());
            };
            let body = body_from_origin_envelope(identity, &system.content)?;
            if &identity.origin_session_id == session_id {
                return Ok(message.clone());
            }
            let mut inherited = system.clone();
            inherited.content = render_inherited_v1(identity, session_id, body);
            Ok(Message::System(inherited))
        })
        .collect()
}

/// Keys inherited by a fork that lack an explicit child-origin activation.
pub fn inherited_instruction_keys_requiring_activation(
    session_id: &SessionId,
    messages: &[Message],
) -> BTreeSet<(InstructionNamespace, InstructionKey)> {
    let mut inherited = BTreeSet::new();
    let mut local = BTreeSet::new();
    for message in messages {
        let Message::System(system) = message else {
            continue;
        };
        let Some(identity) = system.instruction_activation.as_ref() else {
            continue;
        };
        let key = (
            identity.revision.namespace.clone(),
            identity.revision.key.clone(),
        );
        if &identity.origin_session_id == session_id {
            local.insert(key);
        } else {
            inherited.insert(key);
        }
    }
    inherited.retain(|key| !local.contains(key));
    inherited
}

impl Session {
    /// Apply the host's fork instruction policy to this fork child, before its
    /// single save, and return how many re-activation rows it appended.
    ///
    /// The child holds the activation rows it copied with its prefix as
    /// history only. [`ForkInstructionPolicy::Refuse`] refuses a child that
    /// inherits any. [`ForkInstructionPolicy::RequireExplicitActivation`]
    /// leaves them for the host to activate; callers resolve it first for
    /// forks that leave no window for that ([`ForkInstructionPolicy::effective`]).
    /// [`ForkInstructionPolicy::ReactivateInherited`] appends, through the
    /// ordinary activation rules, one origin-local row per inherited key that
    /// re-activates the key's head at the fork boundary, provided that head
    /// was effective in `source_session_id`. The rows follow the prefix in
    /// (namespace, key) order, carry the last copied message's timestamp and
    /// derive their ids from the child id and the head, so the child is a
    /// function of its prefix and its id. A refusal leaves the child unchanged.
    pub fn apply_fork_instruction_policy(
        &mut self,
        source_session_id: &SessionId,
        policy: ForkInstructionPolicy,
    ) -> Result<usize, ForkInstructionRefusal> {
        let heads = self.inherited_activation_heads().map_err(|error| {
            ForkInstructionRefusal::new(
                ForkInstructionRefusalCause::Activation { code: error.code() },
                Vec::new(),
            )
        })?;
        if heads.is_empty() {
            return Ok(0);
        }
        let keys = heads.keys().cloned().collect::<Vec<_>>();
        match policy {
            ForkInstructionPolicy::RequireExplicitActivation => Ok(0),
            ForkInstructionPolicy::Refuse => Err(ForkInstructionRefusal::new(
                ForkInstructionRefusalCause::PolicyRefuses,
                keys,
            )),
            ForkInstructionPolicy::ReactivateInherited => {
                let not_effective = heads
                    .iter()
                    .filter(|(_, (head, _))| head.origin_session_id != *source_session_id)
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                if !not_effective.is_empty() {
                    return Err(ForkInstructionRefusal::new(
                        ForkInstructionRefusalCause::NotEffectiveInSource,
                        not_effective,
                    ));
                }
                let mut child = self.clone();
                let appended = child.append_fork_reactivations(&heads).map_err(|error| {
                    let cause = match error {
                        InstructionActivationError::LineageBodyBudgetExceeded { max_bytes } => {
                            ForkInstructionRefusalCause::LineageBodyBudgetExceeded { max_bytes }
                        }
                        error => ForkInstructionRefusalCause::Activation { code: error.code() },
                    };
                    ForkInstructionRefusal::new(cause, keys)
                })?;
                *self = child;
                Ok(appended)
            }
        }
    }

    /// The length of the fork re-activation rows that end this fork child:
    /// zero, or the trailing origin-local activation rows, verified to be
    /// exactly the rows [`ForkInstructionPolicy::ReactivateInherited`] derives
    /// from the messages before them and this child's id. The suffix is
    /// recognized only by reproducing it, never by the shape of its ids.
    pub fn fork_reactivation_suffix_len(&self) -> Result<usize, InstructionActivationError> {
        let suffix = self
            .messages()
            .iter()
            .rev()
            .take_while(|message| {
                matches!(
                    message,
                    Message::System(system)
                        if system
                            .instruction_activation
                            .as_ref()
                            .is_some_and(|identity| identity.origin_session_id == *self.id())
                )
            })
            .count();
        if suffix == 0 {
            return Ok(0);
        }
        let prefix_len = self.messages().len().saturating_sub(suffix);
        let mut probe = self.clone();
        probe.messages =
            super::TranscriptMessages::from_vec(self.messages()[..prefix_len].to_vec());
        let heads = probe.inherited_activation_heads()?;
        probe.append_fork_reactivations(&heads)?;
        if probe.messages() != self.messages() {
            return Err(InstructionActivationError::MalformedHistory(
                "the trailing origin-local activation rows are not the fork re-activation of \
                 the messages before them"
                    .to_string(),
            ));
        }
        Ok(suffix)
    }

    fn inherited_activation_heads(
        &self,
    ) -> Result<InheritedActivationHeads, InstructionActivationError> {
        validate_instruction_activation_messages(self.messages())
            .map_err(InstructionActivationError::MalformedHistory)?;
        let mut heads = InheritedActivationHeads::new();
        for message in self.messages() {
            let Message::System(system) = message else {
                continue;
            };
            let Some(identity) = system.instruction_activation.as_ref() else {
                continue;
            };
            if identity.origin_session_id == *self.id() {
                continue;
            }
            let body = body_from_origin_envelope(identity, &system.content)
                .map_err(InstructionActivationError::MalformedHistory)?;
            heads.insert(
                (
                    identity.revision.namespace.clone(),
                    identity.revision.key.clone(),
                ),
                (identity.clone(), body.to_string()),
            );
        }
        Ok(heads)
    }

    /// Append the re-activation row of each head, in key order, through the
    /// ordinary activation rules.
    fn append_fork_reactivations(
        &mut self,
        heads: &InheritedActivationHeads,
    ) -> Result<usize, InstructionActivationError> {
        let Some(created_at) = self.messages().last().map(message_created_at) else {
            return Ok(0);
        };
        for (head, body) in heads.values() {
            let request = InstructionActivationRequest {
                revision: head.revision.clone(),
                activation_id: fork_reactivation_activation_id(self.id(), &head.activation_id)?,
                expectation: InstructionActivationExpectation::Absent,
                supersedes: Some(head.activation_id.clone()),
                body: body.clone(),
            };
            self.activate_instruction_at(request, created_at)?;
        }
        Ok(heads.len())
    }

    /// Refuse a model request while a fork has inherited historical
    /// instruction rows without an explicit origin-local activation.
    pub fn validate_instruction_activation_turn_readiness(
        &self,
    ) -> Result<(), InstructionActivationError> {
        let keys = inherited_instruction_keys_requiring_activation(self.id(), self.messages())
            .into_iter()
            .collect::<Vec<_>>();
        if keys.is_empty() {
            Ok(())
        } else {
            Err(InstructionActivationError::InheritedActivationRequiresExplicitActivation { keys })
        }
    }

    /// Read current durable activation rows in transcript order.
    pub fn instruction_activation_records(
        &self,
        query: InstructionActivationReadQuery,
    ) -> Result<InstructionActivationReadPage, InstructionActivationError> {
        validate_instruction_activation_messages(self.messages())
            .map_err(InstructionActivationError::MalformedHistory)?;
        if !(1..=200).contains(&query.limit) {
            return Err(InstructionActivationError::MalformedHistory(
                "instruction activation read limit must be between 1 and 200".to_string(),
            ));
        }
        let mut all = Vec::new();
        let mut activation_ordinal = 0usize;
        for (message_index, message) in self.messages().iter().enumerate() {
            let Message::System(system) = message else {
                continue;
            };
            let Some(identity) = system.instruction_activation.as_ref() else {
                continue;
            };
            let record = InstructionActivationRecord {
                session_id: self.id().clone(),
                identity: identity.clone(),
                activation_ordinal,
                projection_witness: InstructionActivationProjectionWitness {
                    message_index,
                    transcript_revision: super::transcript_messages_digest(
                        &self.messages()[..=message_index],
                    )
                    .map_err(|error| InstructionActivationError::Revision(error.to_string()))?,
                },
            };
            activation_ordinal = activation_ordinal.saturating_add(1);
            if query
                .namespace
                .as_ref()
                .is_none_or(|namespace| namespace == &identity.revision.namespace)
                && query
                    .key
                    .as_ref()
                    .is_none_or(|key| key == &identity.revision.key)
            {
                all.push(record);
            }
        }
        let key_state = match (query.namespace.clone(), query.key.clone()) {
            (Some(namespace), Some(key)) => {
                let chronological_head = all.last().cloned();
                let effective_origin_local = all
                    .iter()
                    .rev()
                    .find(|record| record.identity.origin_session_id == *self.id())
                    .cloned();
                let requires_explicit_child_activation =
                    chronological_head.is_some() && effective_origin_local.is_none();
                let next_expectation = effective_origin_local
                    .as_ref()
                    .map(|record| {
                        InstructionActivationExpectation::Effective(
                            record.identity.activation_id.clone(),
                        )
                    })
                    .unwrap_or(InstructionActivationExpectation::Absent);
                let next_supersedes = chronological_head
                    .as_ref()
                    .map(|record| record.identity.activation_id.clone());
                Some(InstructionActivationKeyState {
                    session_id: self.id().clone(),
                    namespace,
                    key,
                    effective_origin_local,
                    chronological_head,
                    requires_explicit_child_activation,
                    next_expectation,
                    next_supersedes,
                })
            }
            _ => None,
        };
        let offset = query.offset.unwrap_or(0).min(all.len());
        let end = offset.saturating_add(query.limit).min(all.len());
        Ok(InstructionActivationReadPage {
            session_id: self.id().clone(),
            records: all[offset..end].to_vec(),
            key_state,
            next_offset: (end < all.len()).then_some(end),
        })
    }

    /// Append one typed instruction activation to this session document.
    pub fn activate_instruction(
        &mut self,
        request: InstructionActivationRequest,
    ) -> Result<InstructionActivationMutation, InstructionActivationError> {
        self.activate_instruction_at(request, message_timestamp_now())
    }

    /// [`Self::activate_instruction`] with the appended row's timestamp given.
    fn activate_instruction_at(
        &mut self,
        request: InstructionActivationRequest,
        created_at: MessageTimestamp,
    ) -> Result<InstructionActivationMutation, InstructionActivationError> {
        validate_instruction_activation_messages(self.messages())
            .map_err(InstructionActivationError::MalformedHistory)?;
        if request.body.len() > MAX_INSTRUCTION_BODY_BYTES {
            return Err(InstructionActivationError::BodyTooLarge {
                max_bytes: MAX_INSTRUCTION_BODY_BYTES,
            });
        }
        let retained_body_bytes = self.messages().iter().try_fold(0usize, |total, message| {
            let Message::System(system) = message else {
                return Ok(total);
            };
            let Some(identity) = system.instruction_activation.as_ref() else {
                return Ok(total);
            };
            let body = body_from_origin_envelope(identity, &system.content)
                .map_err(InstructionActivationError::MalformedHistory)?;
            total.checked_add(body.len()).ok_or_else(|| {
                InstructionActivationError::MalformedHistory(
                    "instruction activation retained-body byte count overflowed".to_string(),
                )
            })
        })?;
        let computed = InstructionContentDigest::for_body(&request.body);
        if computed != request.revision.content_sha256 {
            return Err(InstructionActivationError::DigestMismatch {
                declared: request.revision.content_sha256,
                computed,
            });
        }

        let same_key = |identity: &InstructionActivationIdentity| {
            identity.revision.namespace == request.revision.namespace
                && identity.revision.key == request.revision.key
        };
        let rows = self
            .messages()
            .iter()
            .enumerate()
            .filter_map(|(index, message)| {
                let Message::System(system) = message else {
                    return None;
                };
                let identity = system.instruction_activation.as_ref()?;
                same_key(identity).then_some((index, system, identity))
            })
            .collect::<Vec<_>>();
        let current_any = rows.last().map(|(_, _, identity)| *identity);
        let current_local = rows
            .iter()
            .rev()
            .find(|(_, _, identity)| identity.origin_session_id == *self.id())
            .map(|(_, _, identity)| *identity);

        if let Some((index, system, identity)) = rows
            .iter()
            .find(|(_, _, identity)| identity.activation_id == request.activation_id)
        {
            if identity.origin_session_id != *self.id() {
                return Err(InstructionActivationError::ActivationIdentityConflict);
            }
            let requested_identity = InstructionActivationIdentity {
                activation_id: request.activation_id.clone(),
                revision: request.revision.clone(),
                supersedes: request.supersedes.clone(),
                origin_session_id: self.id().clone(),
                render_version: INSTRUCTION_ACTIVATION_RENDER_VERSION_V1,
            };
            let exact = **identity == requested_identity
                && system.content == render_origin_v1(&requested_identity, &request.body);
            if !exact {
                return Err(InstructionActivationError::ActivationIdentityConflict);
            }
            if current_local.is_some_and(|current| current.activation_id == request.activation_id) {
                let transcript_revision =
                    super::transcript_messages_digest(&self.messages()[..=*index])
                        .map_err(|error| InstructionActivationError::Revision(error.to_string()))?;
                return Ok(InstructionActivationMutation::Duplicate(
                    InstructionActivationRecord {
                        session_id: self.id().clone(),
                        identity: requested_identity,
                        activation_ordinal: self.messages()[..=*index]
                            .iter()
                            .filter(|message| {
                                matches!(message, Message::System(system) if system.instruction_activation.is_some())
                            })
                            .count()
                            .saturating_sub(1),
                        projection_witness: InstructionActivationProjectionWitness {
                            message_index: *index,
                            transcript_revision,
                        },
                    },
                ));
            }
            return Err(InstructionActivationError::RecordedButNotEffective);
        }

        for (_, system, identity) in &rows {
            if identity.revision.revision_id == request.revision.revision_id
                && (identity.revision.content_sha256 != request.revision.content_sha256
                    || body_from_origin_envelope(identity, &system.content)
                        .is_ok_and(|body| body != request.body))
            {
                return Err(InstructionActivationError::ImmutableRevisionConflict);
            }
        }

        let actual = current_local.map(|identity| identity.activation_id.clone());
        let expectation_matches = match &request.expectation {
            InstructionActivationExpectation::Absent => actual.is_none(),
            InstructionActivationExpectation::Effective(expected) => {
                actual.as_ref() == Some(expected)
            }
        };
        if !expectation_matches {
            return Err(InstructionActivationError::EffectiveActivationConflict {
                expected: request.expectation,
                actual,
            });
        }
        let chronological_predecessor = current_any.map(|identity| identity.activation_id.clone());
        if request.supersedes != chronological_predecessor {
            return Err(InstructionActivationError::SupersedesConflict {
                requested: request.supersedes,
                actual: chronological_predecessor,
            });
        }
        if retained_body_bytes.saturating_add(request.body.len())
            > MAX_INSTRUCTION_ACTIVATION_LINEAGE_BYTES
        {
            return Err(InstructionActivationError::LineageBodyBudgetExceeded {
                max_bytes: MAX_INSTRUCTION_ACTIVATION_LINEAGE_BYTES,
            });
        }

        let identity = InstructionActivationIdentity {
            activation_id: request.activation_id,
            revision: request.revision,
            supersedes: request.supersedes,
            origin_session_id: self.id().clone(),
            render_version: INSTRUCTION_ACTIVATION_RENDER_VERSION_V1,
        };
        let content = render_origin_v1(&identity, &request.body);
        let message_index = self.messages().len();
        let activation_ordinal = self
            .messages()
            .iter()
            .filter(|message| {
                matches!(message, Message::System(system) if system.instruction_activation.is_some())
            })
            .count();
        self.messages.push(Message::System(SystemMessage {
            content,
            created_at,
            identity: None,
            prompt_version: None,
            instruction_activation: Some(identity.clone()),
        }));
        self.mark_content_mutated(crate::time_compat::SystemTime::now());
        let transcript_revision = self
            .transcript_revision()
            .map_err(|error| InstructionActivationError::Revision(error.to_string()))?;
        Ok(InstructionActivationMutation::Appended(
            InstructionActivationRecord {
                session_id: self.id().clone(),
                identity,
                activation_ordinal,
                projection_witness: InstructionActivationProjectionWitness {
                    message_index,
                    transcript_revision,
                },
            },
        ))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::types::{InstructionRevisionId, SystemPromptKey, SystemPromptVersion, UserMessage};
    use crate::{
        SystemPromptUpdateRequest, TranscriptEditError, TranscriptRewriteReason,
        TranscriptRewriteSelection,
    };

    fn request(
        body: &str,
        activation: &str,
        expectation: InstructionActivationExpectation,
        supersedes: Option<&str>,
    ) -> InstructionActivationRequest {
        InstructionActivationRequest {
            revision: InstructionRevisionRef {
                namespace: InstructionNamespace::new("app.example").unwrap(),
                key: InstructionKey::new("primary").unwrap(),
                revision_id: InstructionRevisionId::new(format!("rev-{activation}")).unwrap(),
                content_sha256: InstructionContentDigest::for_body(body),
            },
            activation_id: InstructionActivationId::new(activation).unwrap(),
            expectation,
            supersedes: supersedes.map(|value| InstructionActivationId::new(value).unwrap()),
            body: body.to_string(),
        }
    }

    #[test]
    fn activation_is_chronological_idempotent_and_cas_protected() {
        let mut session = Session::new();
        session.append_system_message("base");
        let first = request(
            "policy one",
            "act-1",
            InstructionActivationExpectation::Absent,
            None,
        );
        let appended = match session.activate_instruction(first.clone()).unwrap() {
            InstructionActivationMutation::Appended(record) => record,
            InstructionActivationMutation::Duplicate(_) => panic!("first activation was duplicate"),
        };
        session.push(Message::User(UserMessage::text("later turn")));
        let duplicate = match session.activate_instruction(first).unwrap() {
            InstructionActivationMutation::Duplicate(record) => record,
            InstructionActivationMutation::Appended(_) => {
                panic!("exact retry appended another row")
            }
        };
        assert_eq!(duplicate.activation_ordinal, appended.activation_ordinal);
        assert_eq!(duplicate.projection_witness, appended.projection_witness);
        let stale = request(
            "policy two",
            "act-2",
            InstructionActivationExpectation::Absent,
            Some("act-1"),
        );
        assert!(matches!(
            session.activate_instruction(stale),
            Err(InstructionActivationError::EffectiveActivationConflict { .. })
        ));
    }

    #[test]
    fn rollback_is_a_later_explicit_activation() {
        let mut session = Session::new();
        session
            .activate_instruction(request(
                "policy one",
                "act-1",
                InstructionActivationExpectation::Absent,
                None,
            ))
            .unwrap();
        session
            .activate_instruction(request(
                "policy two",
                "act-2",
                InstructionActivationExpectation::Effective(
                    InstructionActivationId::new("act-1").unwrap(),
                ),
                Some("act-1"),
            ))
            .unwrap();
        let mut rollback = request(
            "policy one",
            "act-3",
            InstructionActivationExpectation::Effective(
                InstructionActivationId::new("act-2").unwrap(),
            ),
            Some("act-2"),
        );
        rollback.revision.revision_id = InstructionRevisionId::new("rev-act-1").unwrap();
        session.activate_instruction(rollback).unwrap();
        assert_eq!(session.messages().len(), 3);
    }

    #[test]
    fn fork_projection_is_historical_only() {
        let mut parent = Session::new();
        parent
            .activate_instruction(request(
                "policy one",
                "act-1",
                InstructionActivationExpectation::Absent,
                None,
            ))
            .unwrap();
        let child = parent.fork();
        let projected =
            materialize_instruction_activation_messages(child.id(), child.messages()).unwrap();
        assert!(matches!(
            &projected[0],
            Message::System(system)
                if system.content.starts_with("[meerkat-inherited-instruction-v1]")
        ));
        assert_eq!(
            inherited_instruction_keys_requiring_activation(child.id(), child.messages()).len(),
            1
        );
        assert!(matches!(
            child.validate_instruction_activation_turn_readiness(),
            Err(InstructionActivationError::InheritedActivationRequiresExplicitActivation { .. })
        ));

        let mut child = child;
        let key_query = InstructionActivationReadQuery {
            namespace: Some(InstructionNamespace::new("app.example").unwrap()),
            key: Some(InstructionKey::new("primary").unwrap()),
            ..InstructionActivationReadQuery::default()
        };
        let inherited_state = child
            .instruction_activation_records(key_query.clone())
            .unwrap()
            .key_state
            .expect("exact inherited key state");
        assert!(inherited_state.effective_origin_local.is_none());
        assert_eq!(
            inherited_state
                .chronological_head
                .as_ref()
                .map(|record| record.identity.activation_id.as_str()),
            Some("act-1")
        );
        assert!(inherited_state.requires_explicit_child_activation);
        assert_eq!(
            inherited_state.next_expectation,
            InstructionActivationExpectation::Absent
        );
        assert_eq!(
            inherited_state
                .next_supersedes
                .as_ref()
                .map(InstructionActivationId::as_str),
            Some("act-1")
        );
        child
            .activate_instruction(request(
                "policy one",
                "child-act-1",
                InstructionActivationExpectation::Absent,
                Some("act-1"),
            ))
            .unwrap();
        child
            .validate_instruction_activation_turn_readiness()
            .unwrap();
        let local_state = child
            .instruction_activation_records(key_query)
            .unwrap()
            .key_state
            .expect("exact child-local key state");
        assert_eq!(
            local_state
                .effective_origin_local
                .as_ref()
                .map(|record| record.identity.activation_id.as_str()),
            Some("child-act-1")
        );
        assert_eq!(
            local_state
                .chronological_head
                .as_ref()
                .map(|record| record.identity.activation_id.as_str()),
            Some("child-act-1")
        );
        assert!(!local_state.requires_explicit_child_activation);
        assert_eq!(
            local_state.next_expectation,
            InstructionActivationExpectation::Effective(
                InstructionActivationId::new("child-act-1").unwrap()
            )
        );
        assert_eq!(
            local_state
                .next_supersedes
                .as_ref()
                .map(InstructionActivationId::as_str),
            Some("child-act-1")
        );
    }

    #[test]
    fn the_effective_policy_refuses_only_without_an_activation_window() {
        use ForkInstructionPolicy::{ReactivateInherited, Refuse, RequireExplicitActivation};
        for (policy, window, effective) in [
            (
                RequireExplicitActivation,
                ActivationWindow::HostDriven,
                RequireExplicitActivation,
            ),
            (RequireExplicitActivation, ActivationWindow::None, Refuse),
            (
                ReactivateInherited,
                ActivationWindow::HostDriven,
                ReactivateInherited,
            ),
            (
                ReactivateInherited,
                ActivationWindow::None,
                ReactivateInherited,
            ),
            (Refuse, ActivationWindow::HostDriven, Refuse),
            (Refuse, ActivationWindow::None, Refuse),
        ] {
            assert_eq!(
                policy.effective(window),
                effective,
                "{policy:?} with {window:?}"
            );
        }
    }

    fn keyed_request(key: &str, body: &str, activation: &str) -> InstructionActivationRequest {
        let mut request = request(
            body,
            activation,
            InstructionActivationExpectation::Absent,
            None,
        );
        request.revision.key = InstructionKey::new(key).unwrap();
        request
    }

    /// A parent that activated `zeta`, then `alpha`, then `zeta` again, and a
    /// fork child of it.
    fn fork_child_of_two_keys() -> (Session, Session) {
        let mut parent = Session::new();
        parent
            .activate_instruction(keyed_request("zeta", "policy z", "z-1"))
            .unwrap();
        parent.push(Message::User(UserMessage::text("a turn")));
        parent
            .activate_instruction(keyed_request("alpha", "policy a", "a-1"))
            .unwrap();
        let mut zeta_again = keyed_request("zeta", "policy z two", "z-2");
        zeta_again.expectation = InstructionActivationExpectation::Effective(
            InstructionActivationId::new("z-1").unwrap(),
        );
        zeta_again.supersedes = Some(InstructionActivationId::new("z-1").unwrap());
        parent.activate_instruction(zeta_again).unwrap();
        parent.push(Message::User(UserMessage::text("another turn")));
        let child = parent.fork();
        (parent, child)
    }

    #[test]
    fn reactivation_rows_are_a_function_of_the_prefix_and_the_child() {
        let (parent, child) = fork_child_of_two_keys();
        let prefix = child.messages().len();
        let mut first = child.clone();
        let mut second = child.clone();
        assert_eq!(
            first.apply_fork_instruction_policy(
                parent.id(),
                ForkInstructionPolicy::ReactivateInherited
            ),
            Ok(2)
        );
        assert_eq!(
            second.apply_fork_instruction_policy(
                parent.id(),
                ForkInstructionPolicy::ReactivateInherited
            ),
            Ok(2)
        );
        assert_eq!(
            first.messages(),
            second.messages(),
            "the same prefix and child give byte-identical rows"
        );
        let rows = first.messages()[prefix..]
            .iter()
            .map(|message| match message {
                Message::System(system) => system
                    .instruction_activation
                    .clone()
                    .expect("an activation row"),
                other => panic!("expected a re-activation row, got {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows.iter()
                .map(|identity| identity.revision.key.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"],
            "rows in (namespace, key) order, not activation order"
        );
        assert!(
            rows.iter()
                .all(|identity| identity.origin_session_id == *first.id())
        );
        assert_eq!(
            rows.iter()
                .map(|identity| identity
                    .supersedes
                    .as_ref()
                    .map(InstructionActivationId::as_str))
                .collect::<Vec<_>>(),
            vec![Some("a-1"), Some("z-2")],
            "each row re-activates its key's head at the boundary"
        );
        assert_eq!(rows[1].revision.revision_id.as_str(), "rev-z-2");
        first
            .validate_instruction_activation_turn_readiness()
            .unwrap();
        assert_eq!(first.fork_reactivation_suffix_len(), Ok(2));
        assert_eq!(
            child.fork_reactivation_suffix_len(),
            Ok(0),
            "a child without re-activation has no suffix"
        );
    }

    #[test]
    fn refusals_leave_the_child_unchanged() {
        let (parent, child) = fork_child_of_two_keys();
        let mut refused = child.clone();
        assert_eq!(
            refused
                .apply_fork_instruction_policy(parent.id(), ForkInstructionPolicy::Refuse)
                .unwrap_err()
                .cause,
            ForkInstructionRefusalCause::PolicyRefuses
        );
        assert_eq!(refused.messages(), child.messages());
        let mut explicit = child.clone();
        assert_eq!(
            explicit.apply_fork_instruction_policy(
                parent.id(),
                ForkInstructionPolicy::RequireExplicitActivation
            ),
            Ok(0)
        );
        assert_eq!(explicit.messages(), child.messages());
        // A grandchild of an unactivated child: the heads it inherits were
        // never effective in its source.
        let mut grandchild = child.fork();
        let refusal = grandchild
            .apply_fork_instruction_policy(child.id(), ForkInstructionPolicy::ReactivateInherited)
            .unwrap_err();
        assert_eq!(
            refusal.cause,
            ForkInstructionRefusalCause::NotEffectiveInSource
        );
        assert_eq!(refusal.keys.len(), 2);
        assert_eq!(grandchild.messages(), child.messages());
    }

    #[test]
    fn a_suffix_the_fork_did_not_derive_is_refused() {
        let (parent, child) = fork_child_of_two_keys();
        let mut genuine = child.clone();
        genuine
            .apply_fork_instruction_policy(parent.id(), ForkInstructionPolicy::ReactivateInherited)
            .unwrap();
        // The same rows, except one byte of the second row's id: still valid
        // origin-local activations, but not the ones the fork derives.
        let heads = child.inherited_activation_heads().unwrap();
        let created_at = message_created_at(child.messages().last().unwrap());
        let mut forged = child.clone();
        for (index, (head, body)) in heads.values().enumerate() {
            let mut activation_id =
                fork_reactivation_activation_id(child.id(), &head.activation_id)
                    .unwrap()
                    .as_str()
                    .to_string();
            if index == 1 {
                let last = activation_id.pop().unwrap();
                activation_id.push(if last == '0' { '1' } else { '0' });
            }
            forged
                .activate_instruction_at(
                    InstructionActivationRequest {
                        revision: head.revision.clone(),
                        activation_id: InstructionActivationId::new(activation_id).unwrap(),
                        expectation: InstructionActivationExpectation::Absent,
                        supersedes: Some(head.activation_id.clone()),
                        body: body.clone(),
                    },
                    created_at,
                )
                .unwrap();
        }
        assert_eq!(genuine.fork_reactivation_suffix_len(), Ok(2));
        assert!(
            forged.fork_reactivation_suffix_len().is_err(),
            "a suffix the fork did not derive is refused"
        );
    }

    #[test]
    fn fork_rejects_reuse_of_an_inherited_activation_id() {
        let mut parent = Session::new();
        parent
            .activate_instruction(request(
                "policy one",
                "act-1",
                InstructionActivationExpectation::Absent,
                None,
            ))
            .unwrap();
        let mut child = parent.fork();
        let before = child.messages().to_vec();

        assert_eq!(
            child.activate_instruction(request(
                "policy one",
                "act-1",
                InstructionActivationExpectation::Absent,
                Some("act-1"),
            )),
            Err(InstructionActivationError::ActivationIdentityConflict)
        );
        assert_eq!(child.messages(), before);
    }

    #[test]
    fn digest_mismatch_is_rejected_without_mutation() {
        let mut session = Session::new();
        let mut request = request(
            "approved body",
            "act-1",
            InstructionActivationExpectation::Absent,
            None,
        );
        request.revision.content_sha256 = InstructionContentDigest::for_body("other body");

        assert!(matches!(
            session.activate_instruction(request),
            Err(InstructionActivationError::DigestMismatch { .. })
        ));
        assert!(session.messages().is_empty());
    }

    #[test]
    fn activation_id_is_immutable_and_old_exact_retry_is_not_effective() {
        let mut session = Session::new();
        let first = request(
            "policy one",
            "act-1",
            InstructionActivationExpectation::Absent,
            None,
        );
        session.activate_instruction(first.clone()).unwrap();
        session
            .activate_instruction(request(
                "policy two",
                "act-2",
                InstructionActivationExpectation::Effective(
                    InstructionActivationId::new("act-1").unwrap(),
                ),
                Some("act-1"),
            ))
            .unwrap();

        assert_eq!(
            session.activate_instruction(first),
            Err(InstructionActivationError::RecordedButNotEffective)
        );

        let mut reused = request(
            "different body",
            "act-1",
            InstructionActivationExpectation::Effective(
                InstructionActivationId::new("act-2").unwrap(),
            ),
            Some("act-2"),
        );
        reused.revision.revision_id = InstructionRevisionId::new("different-revision").unwrap();
        assert_eq!(
            session.activate_instruction(reused),
            Err(InstructionActivationError::ActivationIdentityConflict)
        );
    }

    #[test]
    fn revision_id_cannot_be_rebound_to_another_body() {
        let mut session = Session::new();
        session
            .activate_instruction(request(
                "policy one",
                "act-1",
                InstructionActivationExpectation::Absent,
                None,
            ))
            .unwrap();
        let mut rebound = request(
            "policy changed",
            "act-2",
            InstructionActivationExpectation::Effective(
                InstructionActivationId::new("act-1").unwrap(),
            ),
            Some("act-1"),
        );
        rebound.revision.revision_id = InstructionRevisionId::new("rev-act-1").unwrap();

        assert_eq!(
            session.activate_instruction(rebound),
            Err(InstructionActivationError::ImmutableRevisionConflict)
        );
    }

    #[test]
    fn inherited_revision_id_cannot_be_rebound_in_a_fork() {
        let mut parent = Session::new();
        parent
            .activate_instruction(request(
                "policy one",
                "act-1",
                InstructionActivationExpectation::Absent,
                None,
            ))
            .unwrap();
        let mut child = parent.fork();
        let mut rebound = request(
            "policy changed",
            "child-act-1",
            InstructionActivationExpectation::Absent,
            Some("act-1"),
        );
        rebound.revision.revision_id = InstructionRevisionId::new("rev-act-1").unwrap();

        assert_eq!(
            child.activate_instruction(rebound),
            Err(InstructionActivationError::ImmutableRevisionConflict)
        );
    }

    #[test]
    fn generic_rewrite_cannot_erase_an_activation() {
        let mut session = Session::new();
        session
            .activate_instruction(request(
                "policy one",
                "act-1",
                InstructionActivationExpectation::Absent,
                None,
            ))
            .unwrap();
        let before = session.messages().to_vec();

        assert!(matches!(
            session.commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                Vec::new(),
                TranscriptRewriteReason::new("attempt activation erasure"),
                Some("unit-test".to_string()),
                None,
            ),
            Err(TranscriptEditError::InvalidTranscriptShape(message))
                if message.contains("cannot mint, alter, move, erase, or restore")
        ));
        assert_eq!(session.messages(), before);
    }

    #[test]
    fn generic_rewrite_cannot_move_an_activation_across_conversation() {
        let mut session = Session::new();
        session
            .activate_instruction(request(
                "policy one",
                "act-1",
                InstructionActivationExpectation::Absent,
                None,
            ))
            .unwrap();
        session.push(Message::User(UserMessage::text("later turn")));
        let before = session.messages().to_vec();

        assert!(matches!(
            session.commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![before[1].clone(), before[0].clone()],
                TranscriptRewriteReason::new("attempt activation move"),
                Some("unit-test".to_string()),
                None,
            ),
            Err(TranscriptEditError::InvalidTranscriptShape(message))
                if message.contains("cannot mint, alter, move, erase, or restore")
        ));
        assert_eq!(session.messages(), before);
    }

    #[test]
    fn persisted_session_rejects_a_broken_supersedes_chain() {
        let mut session = Session::new();
        session
            .activate_instruction(request(
                "policy one",
                "act-1",
                InstructionActivationExpectation::Absent,
                None,
            ))
            .unwrap();
        session
            .activate_instruction(request(
                "policy two",
                "act-2",
                InstructionActivationExpectation::Effective(
                    InstructionActivationId::new("act-1").unwrap(),
                ),
                Some("act-1"),
            ))
            .unwrap();
        let bytes = session.to_persisted_bytes().unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["messages"][1]["instruction_activation"]["supersedes"] = serde_json::Value::Null;
        let malformed = serde_json::to_vec(&value).unwrap();
        assert!(Session::from_persisted_bytes(&malformed).is_err());
    }

    #[test]
    fn retained_body_budget_refuses_growth_but_allows_exact_retry() {
        let mut session = Session::new();
        let body = "x".repeat(MAX_INSTRUCTION_BODY_BYTES);
        let mut previous: Option<InstructionActivationId> = None;
        let mut current_request = None;
        for index in 0..4 {
            let activation = format!("act-{index}");
            let expectation = previous
                .as_ref()
                .map_or(InstructionActivationExpectation::Absent, |previous| {
                    InstructionActivationExpectation::Effective(previous.clone())
                });
            let next = request(
                &body,
                &activation,
                expectation,
                previous.as_ref().map(InstructionActivationId::as_str),
            );
            session.activate_instruction(next.clone()).unwrap();
            previous = Some(InstructionActivationId::new(activation).unwrap());
            current_request = Some(next);
        }

        assert!(matches!(
            session.activate_instruction(current_request.unwrap()),
            Ok(InstructionActivationMutation::Duplicate(_))
        ));
        assert_eq!(
            session.activate_instruction(request(
                "one more byte",
                "act-over-budget",
                InstructionActivationExpectation::Effective(previous.clone().unwrap()),
                previous.as_ref().map(InstructionActivationId::as_str),
            )),
            Err(InstructionActivationError::LineageBodyBudgetExceeded {
                max_bytes: MAX_INSTRUCTION_ACTIVATION_LINEAGE_BYTES,
            })
        );
    }

    #[test]
    fn prompt_replacement_preserves_the_chronological_activation_row() {
        let mut session = Session::new();
        session.append_system_message("base prompt");
        session
            .activate_instruction(request(
                "policy one",
                "act-1",
                InstructionActivationExpectation::Absent,
                None,
            ))
            .unwrap();

        session
            .update_system_prompt(SystemPromptUpdateRequest {
                key: SystemPromptKey::new("primary").unwrap(),
                expected_version: None,
                target_message_index: Some(0),
                content: "replacement prompt".to_string(),
                actor: Some("unit-test".to_string()),
                expected_parent_revision: None,
            })
            .unwrap();

        let activation_rows = session
            .messages()
            .iter()
            .filter(|message| {
                matches!(
                    message,
                    Message::System(system) if system.instruction_activation.is_some()
                )
            })
            .count();
        assert_eq!(activation_rows, 1);
        let projected = session.messages_for_model_boundary();
        assert!(matches!(
            &projected[0],
            Message::System(system)
                if system.prompt_version.as_ref().is_some_and(|identity| {
                    identity.version == SystemPromptVersion::INITIAL
                }) && system.content == "replacement prompt"
        ));
        assert!(matches!(
            &projected[1],
            Message::System(system)
                if system.instruction_activation.as_ref().is_some_and(|identity| {
                    identity.activation_id.as_str() == "act-1"
                })
        ));
    }

    #[test]
    fn activation_record_reads_are_filtered_and_bounded() {
        let mut session = Session::new();
        session
            .activate_instruction(request(
                "policy one",
                "act-1",
                InstructionActivationExpectation::Absent,
                None,
            ))
            .unwrap();
        session
            .activate_instruction(request(
                "policy two",
                "act-2",
                InstructionActivationExpectation::Effective(
                    InstructionActivationId::new("act-1").unwrap(),
                ),
                Some("act-1"),
            ))
            .unwrap();

        let page = session
            .instruction_activation_records(InstructionActivationReadQuery {
                limit: 1,
                ..InstructionActivationReadQuery::default()
            })
            .unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.next_offset, Some(1));
        let second = session
            .instruction_activation_records(InstructionActivationReadQuery {
                offset: page.next_offset,
                limit: 1,
                ..InstructionActivationReadQuery::default()
            })
            .unwrap();
        assert_eq!(second.records[0].identity.activation_id.as_str(), "act-2");
        assert_eq!(second.next_offset, None);
    }

    #[test]
    fn compaction_preserves_activation_ordinal_and_updates_projection_witness() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("first turn")));
        session.push(Message::User(UserMessage::text("second turn")));
        let activation = request(
            "policy one",
            "act-1",
            InstructionActivationExpectation::Absent,
            None,
        );
        let appended = match session.activate_instruction(activation.clone()).unwrap() {
            InstructionActivationMutation::Appended(record) => record,
            InstructionActivationMutation::Duplicate(_) => panic!("first activation was duplicate"),
        };
        let activation_row = session.messages()[2].clone();
        session
            .stage_validated_compaction_for_test(
                vec![
                    Message::User(UserMessage::compaction_summary("summary")),
                    activation_row.clone(),
                ],
                1,
            )
            .unwrap();
        assert_eq!(session.messages()[1], activation_row);

        let page = session
            .instruction_activation_records(InstructionActivationReadQuery {
                namespace: Some(InstructionNamespace::new("app.example").unwrap()),
                key: Some(InstructionKey::new("primary").unwrap()),
                ..InstructionActivationReadQuery::default()
            })
            .unwrap();
        let read = &page.records[0];
        assert_eq!(read.identity, appended.identity);
        assert_eq!(read.activation_ordinal, appended.activation_ordinal);
        assert_ne!(read.projection_witness, appended.projection_witness);
        assert_eq!(read.projection_witness.message_index, 1);
        let state = page.key_state.expect("exact key query returns state");
        assert_eq!(state.effective_origin_local.as_ref(), Some(read));
        assert_eq!(state.chronological_head.as_ref(), Some(read));
        assert!(!state.requires_explicit_child_activation);
        assert_eq!(
            state.next_expectation,
            InstructionActivationExpectation::Effective(
                InstructionActivationId::new("act-1").unwrap()
            )
        );

        let duplicate = match session.activate_instruction(activation).unwrap() {
            InstructionActivationMutation::Duplicate(record) => record,
            InstructionActivationMutation::Appended(_) => panic!("exact retry appended"),
        };
        assert_eq!(duplicate.identity, appended.identity);
        assert_eq!(duplicate.activation_ordinal, appended.activation_ordinal);
        assert_eq!(duplicate.projection_witness, read.projection_witness);
    }
}
