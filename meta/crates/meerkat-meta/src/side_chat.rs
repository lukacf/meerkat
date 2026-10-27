//! Console side chats: a human's private branch of an agent's conversation.
//!
//! The console's `meta/fork_member` forks an agent's durable transcript into a
//! new mob-plane member the human continues in, while the source agent's
//! session, transcript and queued work stay untouched. The child:
//!
//! - takes its source's role, profile and model overrides and runtime mode,
//!   and (through the durable fork) its application labels, context and tool
//!   overlay, so it runs with the source's tools and its audience is the
//!   source's;
//! - is kept until the human closes it (`meta/retire_member`): the fork sets
//!   an explicit keep-until-retired idle override, so no idle sweep retires
//!   it. The runtime may still unload it while it is idle; that keeps the
//!   member;
//! - is never wired. It has no peer edge, so no other agent can address it
//!   and none lists it as a peer. Meerkat applies the mob definition's wiring
//!   rules when it seats any member, so a source whose role those rules wire
//!   is refused before anything is forked ([`definition_wiring_for_role`]),
//!   and Meta's definition-wiring reconcile leaves side chats out;
//! - names its source in the [`SIDE_CHAT_OF_LABEL`] roster label. The label
//!   counts only where Meerkat's roster agrees ([`verified_side_chat_source`]):
//!   the member's typed `fork_source` names that same source and no member's
//!   own turn created it. The console projects the verified relation as the
//!   `side_chat_of` field of a member row, so a side chat nests under its
//!   source across restarts, and strips the label everywhere else.
//!
//! Instructions: a durable fork copies the source's activated instructions as
//! history only. For each inherited key whose head is an activation the
//! source made itself, the fork activates the same revision and body in the
//! child as the host (`meta/fork_member` is the host's fork), recorded as
//! superseding the source's activation. Any other inherited key stays
//! inactive, and the child refuses its first turn naming it, typed, until a
//! host activates it.

use std::collections::{BTreeMap, BTreeSet};

use meerkat_core::types::{InstructionKey, InstructionNamespace, Message, SessionId};
use meerkat_core::{
    InstructionActivationAdmissionErrorCode, InstructionActivationExpectation,
    InstructionActivationId, InstructionActivationIdentity, InstructionActivationRequest,
    InstructionContentDigest,
};
use meerkat_mob::{MobSessionService as _, SpawnMemberSpec};
use serde::Serialize;

use crate::fork::{ForkChildRetention, ForkMemberError, ForkMemberOutcome};
use crate::mob_handle_runtime::MobRuntime;

/// The roster label a side chat names its source in: the source's public
/// member alias. Projected only where Meerkat's roster agrees
/// ([`verified_side_chat_source`]).
pub const SIDE_CHAT_OF_LABEL: &str = "side_chat_of";

const DISPLAY_NAME_LABEL: &str = "display_name";
/// The most a caller-chosen side chat display name may take.
const MAX_DISPLAY_NAME_CHARS: usize = 80;
/// Prefix of the activation ids a side chat's inherited instructions get.
const INHERITED_ACTIVATION_ID_PREFIX: &str = "console-side-chat:";

/// The source alias a roster member is a side chat of, when Meerkat's roster
/// agrees: the member carries [`SIDE_CHAT_OF_LABEL`], its typed durable
/// `fork_source` names the member the label names, and no member's own turn
/// created it (`spawned_by` is empty: a `fork_off` child of a side chat
/// inherits the label, but names the side chat as its source and spawner).
///
/// Takes the roster entry's fields, not the entry: the roster entry type is
/// not nameable outside meerkat-mob.
pub(crate) fn verified_side_chat_source(
    labels: &BTreeMap<String, String>,
    spawned_by: Option<&meerkat_mob::AgentIdentity>,
    fork_source: Option<&meerkat_core::ForkBuildSource>,
) -> Option<String> {
    let marker = labels.get(SIDE_CHAT_OF_LABEL)?;
    if spawned_by.is_some() {
        return None;
    }
    let source = fork_source?;
    let source_alias = crate::member_comms_id::runtime_alias_str(&source.source_member.member);
    (source_alias.as_ref() == marker.as_str()).then(|| source_alias.into_owned())
}

/// Whether any of `labels` carries the side chat marker, so a projection
/// reads the roster's fork facts only when a side chat may be among its
/// rows.
pub(crate) fn any_side_chat_marker<'a>(
    labels: impl IntoIterator<Item = &'a BTreeMap<String, String>>,
) -> bool {
    labels
        .into_iter()
        .any(|labels| labels.contains_key(SIDE_CHAT_OF_LABEL))
}

/// Every verified side chat of `handle`'s mob, keyed by the side chat's
/// public alias, with its source's console identity: the source's durable
/// identity label when it is still on the roster, else its public alias.
pub(crate) async fn verified_side_chats(
    handle: &meerkat_mob::MobHandle,
) -> BTreeMap<String, String> {
    let entries = handle.list_all_members().await;
    let console_identity_by_alias: BTreeMap<String, String> = entries
        .iter()
        .map(|entry| {
            let alias = crate::member_comms_id::runtime_alias_str(entry.agent_identity.as_str())
                .into_owned();
            let identity = crate::member_comms_id::durable_identity_label(&entry.labels)
                .map_or_else(|| alias.clone(), str::to_string);
            (alias, identity)
        })
        .collect();
    entries
        .iter()
        .filter_map(|entry| {
            let source_alias = verified_side_chat_source(
                &entry.labels,
                entry.spawned_by.as_ref(),
                entry.fork_source.as_ref(),
            )?;
            let source_identity = console_identity_by_alias
                .get(&source_alias)
                .cloned()
                .unwrap_or(source_alias);
            Some((
                crate::member_comms_id::runtime_alias_str(entry.agent_identity.as_str())
                    .into_owned(),
                source_identity,
            ))
        })
        .collect()
}

/// The aliases of the verified side chats among `members` (public aliases
/// of `handle`'s roster rows). Reads the roster's fork facts only when a row
/// carries the marker.
pub(crate) async fn verified_side_chat_aliases<'a>(
    handle: &meerkat_mob::MobHandle,
    member_labels: impl IntoIterator<Item = &'a BTreeMap<String, String>>,
) -> BTreeSet<String> {
    if !any_side_chat_marker(member_labels) {
        return BTreeSet::new();
    }
    verified_side_chats(handle).await.into_keys().collect()
}

/// A mob definition wiring rule that would wire a member of a role when
/// Meerkat seats it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
pub enum DefinitionWiring {
    /// `auto_wire_orchestrator` wires every member whose role is not the
    /// orchestrator's to the orchestrator.
    AutoWireOrchestrator { orchestrator_role: String },
    /// A `role_wiring` rule names the role.
    RoleWiring { peer_role: String },
}

/// The definition wiring rule that would wire a member of `role` when
/// Meerkat seats it, if any. Mirrors meerkat-mob's spawn wiring plan, and is
/// conservative: a rule that names the role counts even while no member of
/// the peer role is live, since one can be by the time the child is seated.
pub(crate) fn definition_wiring_for_role(
    definition: &meerkat_mob::MobDefinition,
    role: &str,
) -> Option<DefinitionWiring> {
    if definition.wiring.auto_wire_orchestrator
        && let Some(orchestrator) = definition.orchestrator.as_ref()
        && orchestrator.profile.as_str() != role
    {
        return Some(DefinitionWiring::AutoWireOrchestrator {
            orchestrator_role: orchestrator.profile.to_string(),
        });
    }
    definition.wiring.role_wiring.iter().find_map(|rule| {
        if rule.a.as_str() == role {
            Some(DefinitionWiring::RoleWiring {
                peer_role: rule.b.to_string(),
            })
        } else if rule.b.as_str() == role {
            Some(DefinitionWiring::RoleWiring {
                peer_role: rule.a.to_string(),
            })
        } else {
            None
        }
    })
}

/// Why a side chat fork did not complete.
#[derive(Debug)]
pub enum SideChatError {
    /// The request is malformed (an empty or overlong display name).
    InvalidRequest(String),
    /// The durable fork refused or failed, typed (see [`ForkMemberError`]).
    Fork(ForkMemberError),
    /// The source alias names no roster member.
    UnknownSource { source_member_id: String },
    /// The source's roster entry could not be read.
    SourceUnreadable {
        source_member_id: String,
        message: String,
    },
    /// The mob definition wires the source's role, so Meerkat would wire the
    /// child to peers when it seats it. Nothing was forked.
    WiredRole {
        source_member_id: String,
        role: String,
        wiring: DefinitionWiring,
    },
    /// The child was seated with peer edges after all. The caller retires it.
    WiredAtSeat {
        member_alias: String,
        member_id: meerkat_mob::AgentIdentity,
        peers: Vec<String>,
    },
}

impl std::fmt::Display for SideChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(message) => f.write_str(message),
            Self::Fork(error) => write!(f, "{error}"),
            Self::UnknownSource { source_member_id } => {
                write!(f, "unknown fork source: {source_member_id}")
            }
            Self::SourceUnreadable {
                source_member_id,
                message,
            } => write!(
                f,
                "the fork source {source_member_id} could not be read: {message}"
            ),
            Self::WiredRole {
                source_member_id,
                role,
                ..
            } => write!(
                f,
                "a side chat of {source_member_id} cannot stay unwired: the mob definition \
                 wires members of role {role} to peers when they are seated"
            ),
            Self::WiredAtSeat {
                member_alias,
                peers,
                ..
            } => write!(
                f,
                "the side chat {member_alias} was seated with peer edges ({}) and is retired",
                peers.join(", ")
            ),
        }
    }
}

impl std::error::Error for SideChatError {}

/// What a side chat's fork did with one instruction it inherited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SideChatInstruction {
    pub namespace: String,
    pub key: String,
    pub revision_id: String,
    pub content_sha256: String,
    pub state: SideChatInstructionState,
    /// Why an inactive instruction was not inherited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<SideChatInstructionGap>,
}

/// Whether an inherited instruction is in effect in the side chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SideChatInstructionState {
    /// The fork activated the source's revision in the side chat.
    Inherited,
    /// Not in effect: the side chat refuses its first turn naming this key
    /// until a host activates it.
    Inactive,
}

/// Why an inherited instruction stayed inactive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SideChatInstructionGap {
    /// The key's head in the copied transcript is not an activation the
    /// source made itself (it inherited it from an earlier fork and never
    /// activated it), so nothing identical is in effect in the source.
    NotEffectiveInSource,
    /// The copied row's body does not match its declared content digest.
    BodyUnreadable,
    /// Meerkat refused the activation in the side chat.
    ActivationRefused { message: String },
}

/// A seated side chat.
#[derive(Debug)]
pub struct SideChatFork {
    /// The durable fork's outcome, as meerkat answered it.
    pub outcome: ForkMemberOutcome,
    /// The source's console identity (its durable identity label, else its
    /// public alias).
    pub source_identity: String,
    /// The source session the transcript was forked from, when Meerkat's
    /// roster records it.
    pub source_session_id: Option<SessionId>,
    /// The side chat's display name.
    pub display_name: String,
    /// What the fork did with each instruction the side chat inherited.
    pub instructions: Vec<SideChatInstruction>,
}

/// Fork `source_member_alias` into a side chat seated as `child_alias`.
///
/// Refuses, before anything is forked, a source the roster does not hold and
/// a source whose role the mob definition wires; the durable fork refuses a
/// source that is running (`ForkSourceUnavailable`, cause `running`), typed.
pub async fn fork_side_chat(
    mob_runtime: &MobRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    source_member_alias: &str,
    child_alias: &str,
    display_name: Option<&str>,
) -> Result<SideChatFork, SideChatError> {
    let display_name = match display_name.map(str::trim) {
        None | Some("") => None,
        Some(name) if name.chars().count() > MAX_DISPLAY_NAME_CHARS => {
            return Err(SideChatError::InvalidRequest(format!(
                "display_name must hold at most {MAX_DISPLAY_NAME_CHARS} characters"
            )));
        }
        Some(name) if name.chars().any(char::is_control) => {
            return Err(SideChatError::InvalidRequest(
                "display_name must not hold control characters".to_string(),
            ));
        }
        Some(name) => Some(name.to_string()),
    };
    let handle = mob_runtime.handle();
    let source_identity =
        crate::fork::fork_source_roster_identity(identity_runtime, source_member_alias)
            .await
            .map_err(SideChatError::Fork)?;
    let source_alias =
        crate::member_comms_id::runtime_alias_str(source_identity.as_str()).into_owned();
    let source = match handle.get_member(&source_identity).await {
        Ok(Some(source)) => source,
        Ok(None) => {
            return Err(SideChatError::UnknownSource {
                source_member_id: source_alias,
            });
        }
        Err(error) => {
            return Err(SideChatError::SourceUnreadable {
                source_member_id: source_alias,
                message: error.to_string(),
            });
        }
    };
    if let Some(wiring) = definition_wiring_for_role(handle.definition(), source.role.as_str()) {
        return Err(SideChatError::WiredRole {
            source_member_id: source_alias,
            role: source.role.to_string(),
            wiring,
        });
    }
    let source_console_identity = crate::member_comms_id::durable_identity_label(&source.labels)
        .map_or_else(|| source_alias.clone(), str::to_string);
    let display_name = display_name.unwrap_or_else(|| {
        let source_name = source
            .labels
            .get(DISPLAY_NAME_LABEL)
            .map(String::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(source_console_identity.as_str());
        format!("{source_name} (side chat)")
    });
    // The child's own labels win over the ones it inherits from its source,
    // key by key: the marker names this source, and the display name keeps
    // the side chat apart from the agent it branched from.
    let labels = BTreeMap::from([
        (SIDE_CHAT_OF_LABEL.to_string(), source_alias),
        (DISPLAY_NAME_LABEL.to_string(), display_name.clone()),
    ]);
    // The source's role, profile and model overrides and runtime mode, as a
    // `fork_off` child gets them: the side chat runs with the source's tools.
    let mut spec = SpawnMemberSpec::new(source.role, meerkat_mob::AgentIdentity::from(child_alias))
        .with_runtime_mode(source.runtime_mode)
        .with_labels(labels);
    spec.override_profile = source.effective_profile_override;
    spec.model_override = source.effective_model_override;

    let outcome = crate::fork::fork_member_on_runtime(
        mob_runtime,
        identity_runtime,
        source_member_alias,
        spec,
        None,
        ForkChildRetention::UntilRetired,
        None,
    )
    .await
    .map_err(SideChatError::Fork)?;

    let child_id = outcome.result.agent_identity.clone();
    let child = handle.get_member(&child_id).await.ok().flatten();
    if let Some(child) = child.as_ref()
        && !child.wired_to.is_empty()
    {
        return Err(SideChatError::WiredAtSeat {
            member_alias: outcome.member_alias,
            member_id: child_id,
            peers: child
                .wired_to
                .iter()
                .map(|peer| crate::member_comms_id::runtime_alias_str(peer.as_str()).into_owned())
                .collect(),
        });
    }
    let source_session_id = child
        .and_then(|child| child.fork_source)
        .map(|fork_source| fork_source.source_session_id);
    let instructions = inherit_source_instructions(
        mob_runtime,
        &child_id,
        &outcome.result.session_id,
        source_session_id.as_ref(),
    )
    .await;
    Ok(SideChatFork {
        outcome,
        source_identity: source_console_identity,
        source_session_id,
        display_name,
        instructions,
    })
}

/// JSON-RPC code of a typed `meta/fork_member` refusal: nothing was forked.
/// `data.kind` is `fork_source_unavailable` (with `source_member_id` and
/// `cause`: `running`, `no_session`, `remote_read_unavailable` or
/// `authority_unavailable`) or `unwired_seat_unavailable` (with `wiring`).
pub const SIDE_CHAT_FORK_REFUSED_CODE: i64 = -32055;

/// Whether `runtime` can hold a durable fork: its sessions persist, or its
/// composition did not record whether they do (the fork itself then refuses
/// typed). A runtime known to persist no sessions never offers side chats.
pub(crate) fn runtime_may_fork_durably(runtime: &MobRuntime) -> bool {
    runtime
        .resolved_storage()
        .is_none_or(|storage| storage.session_store_incremental.is_some())
}

/// The params of one `meta/fork_member` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForkMemberParams {
    /// The agent whose conversation is forked.
    pub(crate) source_member_id: String,
    /// The side chat's public member alias.
    pub(crate) agent_identity: String,
    /// The side chat's display name; defaults to the source's name with
    /// " (side chat)".
    pub(crate) display_name: Option<String>,
}

fn required_string_param(params: &serde_json::Value, name: &str) -> Result<String, String> {
    match params.get(name) {
        Some(serde_json::Value::String(value)) if !value.trim().is_empty() => {
            Ok(value.trim().to_string())
        }
        Some(serde_json::Value::String(_)) => Err(format!("{name} must not be empty")),
        Some(serde_json::Value::Null) | None => Err(format!("{name} required")),
        Some(_) => Err(format!("{name} must be a string")),
    }
}

/// Read a `meta/fork_member` request's params. A side chat forks the
/// source's committed transcript end, so a `message_count` (a prefix) is
/// refused rather than ignored.
pub(crate) fn parse_fork_member_params(
    params: &serde_json::Value,
) -> Result<ForkMemberParams, String> {
    let source_member_id = required_string_param(params, "source_member_id")?;
    let agent_identity = required_string_param(params, "agent_identity")?;
    let display_name = match params.get("display_name") {
        Some(serde_json::Value::String(name)) => Some(name.clone()),
        Some(serde_json::Value::Null) | None => None,
        Some(_) => return Err("display_name must be a string".to_string()),
    };
    if params
        .get("message_count")
        .is_some_and(|value| !value.is_null())
    {
        return Err(
            "message_count is not supported: a side chat forks the source's committed \
             transcript end"
                .to_string(),
        );
    }
    Ok(ForkMemberParams {
        source_member_id,
        agent_identity,
        display_name,
    })
}

/// The `meta/fork_member` result for a seated side chat.
pub(crate) fn fork_member_result(fork: &SideChatFork) -> serde_json::Value {
    serde_json::json!({
        "agent_identity": fork.outcome.member_alias,
        "session_id": fork.outcome.result.session_id.to_string(),
        "display_name": fork.display_name,
        "side_chat_of": fork.source_identity,
        "source_member_id": fork.outcome.source_member_alias,
        "source_session_id": fork.source_session_id.as_ref().map(ToString::to_string),
        "retention": "until_closed",
        "instructions": fork.instructions,
        "cache_inheritance": fork.outcome.result.cache_inheritance,
    })
}

fn fork_refusal(message: String, data: serde_json::Value) -> crate::rpc::JsonRpcError {
    crate::rpc::JsonRpcError {
        code: SIDE_CHAT_FORK_REFUSED_CODE,
        message,
        data: Some(data),
    }
}

fn fork_source_cause(cause: meerkat_mob::error::ForkSourceUnavailableCause) -> &'static str {
    use meerkat_mob::error::ForkSourceUnavailableCause as Cause;
    match cause {
        Cause::NoSession => "no_session",
        Cause::Running => "running",
        Cause::RemoteReadUnavailable => "remote_read_unavailable",
    }
}

/// The JSON-RPC error a failed `meta/fork_member` answers.
pub(crate) fn fork_member_error(error: &SideChatError) -> crate::rpc::JsonRpcError {
    match error {
        SideChatError::InvalidRequest(message) => crate::rpc::JsonRpcError {
            code: -32602,
            message: message.clone(),
            data: None,
        },
        SideChatError::UnknownSource { .. } => crate::rpc::JsonRpcError {
            code: -32602,
            message: error.to_string(),
            data: None,
        },
        SideChatError::SourceUnreadable { .. } => crate::rpc::JsonRpcError {
            code: -32603,
            message: error.to_string(),
            data: None,
        },
        SideChatError::WiredRole {
            source_member_id,
            role,
            wiring,
        } => fork_refusal(
            error.to_string(),
            serde_json::json!({
                "kind": "unwired_seat_unavailable",
                "source_member_id": source_member_id,
                "role": role,
                "wiring": wiring,
            }),
        ),
        SideChatError::WiredAtSeat {
            member_alias,
            peers,
            ..
        } => fork_refusal(
            error.to_string(),
            serde_json::json!({
                "kind": "unwired_seat_unavailable",
                "agent_identity": member_alias,
                "wiring": { "rule": "seated_with_peers", "peers": peers },
            }),
        ),
        SideChatError::Fork(ForkMemberError::InvalidRequest(message)) => crate::rpc::JsonRpcError {
            code: -32602,
            message: message.clone(),
            data: None,
        },
        SideChatError::Fork(ForkMemberError::SourceAuthority(message)) => fork_refusal(
            error.to_string(),
            serde_json::json!({
                "kind": "fork_source_unavailable",
                "cause": "authority_unavailable",
                "detail": message,
            }),
        ),
        SideChatError::Fork(ForkMemberError::Mob(
            meerkat_mob::MobError::ForkSourceUnavailable {
                source_member_id,
                cause,
                ..
            },
        )) => fork_refusal(
            error.to_string(),
            serde_json::json!({
                "kind": "fork_source_unavailable",
                "source_member_id": source_member_id,
                "cause": fork_source_cause(*cause),
            }),
        ),
        SideChatError::Fork(fork_error @ ForkMemberError::Mob(mob_error)) => {
            crate::rpc::JsonRpcError {
                code: mob_error
                    .wire_error_code()
                    .map_or(-32603, |code| i64::from(code.jsonrpc_code())),
                message: error.to_string(),
                // Meerkat's typed data, verbatim: a committed fork whose seat
                // failed keeps its `fork_session_id` and recovery hint.
                data: fork_error.structured_data(),
            }
        }
    }
}

/// Activate in the side chat, as the host, each inherited instruction whose
/// head the source activated itself, with the same revision and body, and
/// report every inherited key.
async fn inherit_source_instructions(
    mob_runtime: &MobRuntime,
    child: &meerkat_mob::AgentIdentity,
    child_session: &SessionId,
    source_session: Option<&SessionId>,
) -> Vec<SideChatInstruction> {
    let Some(sessions) = mob_runtime.session_service() else {
        return Vec::new();
    };
    let session = match sessions.load_persisted_session(child_session).await {
        Ok(Some(session)) => session,
        Ok(None) => return Vec::new(),
        Err(error) => {
            tracing::warn!(
                session_id = %child_session,
                error = %error,
                "a side chat's transcript could not be read for its inherited instructions; \
                 its first turn names any it must activate"
            );
            return Vec::new();
        }
    };
    let messages = session.messages();
    let requiring = meerkat_core::session::inherited_instruction_keys_requiring_activation(
        child_session,
        messages,
    );
    if requiring.is_empty() {
        return Vec::new();
    }
    let heads = inherited_activation_heads(child_session, messages);
    let handle = mob_runtime.handle();
    let mut instructions = Vec::with_capacity(requiring.len());
    for key in requiring {
        let Some((head, content)) = heads.get(&key) else {
            continue;
        };
        let mut instruction = SideChatInstruction {
            namespace: key.0.as_str().to_string(),
            key: key.1.as_str().to_string(),
            revision_id: head.revision.revision_id.as_str().to_string(),
            content_sha256: head.revision.content_sha256.to_string(),
            state: SideChatInstructionState::Inactive,
            reason: None,
        };
        if source_session != Some(&head.origin_session_id) {
            instruction.reason = Some(SideChatInstructionGap::NotEffectiveInSource);
            instructions.push(instruction);
            continue;
        }
        let Some(body) = activation_body(head, content) else {
            instruction.reason = Some(SideChatInstructionGap::BodyUnreadable);
            instructions.push(instruction);
            continue;
        };
        let request = match inherited_activation_request(child_session, head, body) {
            Ok(request) => request,
            Err(message) => {
                instruction.reason = Some(SideChatInstructionGap::ActivationRefused { message });
                instructions.push(instruction);
                continue;
            }
        };
        match activate_in_child(&handle, child, request).await {
            Ok(()) => instruction.state = SideChatInstructionState::Inherited,
            Err(message) => {
                instruction.reason = Some(SideChatInstructionGap::ActivationRefused { message });
            }
        }
        instructions.push(instruction);
    }
    instructions
}

/// Each inherited key's head in `messages` (its last copied activation row,
/// the head at the fork boundary) with the row's stored content.
fn inherited_activation_heads(
    child_session: &SessionId,
    messages: &[Message],
) -> BTreeMap<(InstructionNamespace, InstructionKey), (InstructionActivationIdentity, String)> {
    let mut heads = BTreeMap::new();
    for message in messages {
        let Message::System(system) = message else {
            continue;
        };
        let Some(identity) = system.instruction_activation.as_ref() else {
            continue;
        };
        if &identity.origin_session_id == child_session {
            continue;
        }
        heads.insert(
            (
                identity.revision.namespace.clone(),
                identity.revision.key.clone(),
            ),
            (identity.clone(), system.content.clone()),
        );
    }
    heads
}

/// The body of an activation row: its envelope's text after the first blank
/// line, accepted only when it matches the row's declared content digest.
/// Meerkat checks the digest and the revision's binding again when the
/// activation is appended.
fn activation_body<'a>(
    identity: &InstructionActivationIdentity,
    content: &'a str,
) -> Option<&'a str> {
    let (_, body) = content.split_once("\n\n")?;
    (InstructionContentDigest::for_body(body) == identity.revision.content_sha256).then_some(body)
}

/// The host activation that inherits `head` in the side chat: the same
/// revision and body, superseding the source's activation the child copied.
/// Its id is a function of the side chat's session and the head, so the same
/// fork names the same activation.
fn inherited_activation_request(
    child_session: &SessionId,
    head: &InstructionActivationIdentity,
    body: &str,
) -> Result<InstructionActivationRequest, String> {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(child_session.to_string().as_bytes());
    hasher.update([0u8]);
    hasher.update(head.activation_id.as_str().as_bytes());
    let digest = hasher.finalize();
    let mut id = String::from(INHERITED_ACTIVATION_ID_PREFIX);
    for byte in digest.iter().take(16) {
        id.push_str(&format!("{byte:02x}"));
    }
    let activation_id = InstructionActivationId::new(id).map_err(|error| error.to_string())?;
    Ok(InstructionActivationRequest {
        revision: head.revision.clone(),
        activation_id,
        expectation: InstructionActivationExpectation::Absent,
        supersedes: Some(head.activation_id.clone()),
        body: body.to_string(),
    })
}

/// Append `request` to the side chat's session at its safe boundary,
/// materializing the member once when the runtime has not (or no longer)
/// holds its session live.
async fn activate_in_child(
    handle: &meerkat_mob::MobHandle,
    child: &meerkat_mob::AgentIdentity,
    request: InstructionActivationRequest,
) -> Result<(), String> {
    match handle
        .activate_member_instruction(child, request.clone())
        .await
    {
        Ok(_) => Ok(()),
        Err(error)
            if error.admission_code()
                == Some(InstructionActivationAdmissionErrorCode::TargetNotMaterialized) =>
        {
            handle
                .ensure_member_live(child)
                .await
                .map_err(|error| error.to_string())?;
            handle
                .activate_member_instruction(child, request)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn fork_source(member: &str) -> meerkat_core::ForkBuildSource {
        meerkat_core::ForkBuildSource::new(
            meerkat_core::MobMemberBinding {
                mob_id: "mob".to_string(),
                role: "worker".to_string(),
                member: member.to_string(),
            },
            SessionId::new(),
        )
    }

    fn marked(source: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(SIDE_CHAT_OF_LABEL.to_string(), source.to_string())])
    }

    #[test]
    fn a_side_chat_counts_only_where_the_roster_agrees() {
        let source = fork_source("alice");
        assert_eq!(
            verified_side_chat_source(&marked("alice"), None, Some(&source)),
            Some("alice".to_string()),
            "the marker names the member the durable fork came from"
        );
        assert_eq!(
            verified_side_chat_source(&marked("bob"), None, Some(&source)),
            None,
            "a marker naming another member is not a side chat"
        );
        assert_eq!(
            verified_side_chat_source(&marked("alice"), None, None),
            None,
            "a caller-supplied marker on a member that was not forked counts for nothing"
        );
        let spawner = meerkat_mob::AgentIdentity::from("alice");
        assert_eq!(
            verified_side_chat_source(&marked("alice"), Some(&spawner), Some(&source)),
            None,
            "a fork a member's own turn made (fork_off) is not a human's side chat"
        );
        assert_eq!(
            verified_side_chat_source(&BTreeMap::new(), None, Some(&source)),
            None,
            "a plain durable fork without the marker is not a side chat"
        );
    }

    #[test]
    fn an_encoded_source_member_id_compares_as_its_public_alias() {
        let encoded = crate::member_comms_id::mob_member_id("review:alpha");
        let source = fork_source(encoded.as_str());
        assert_eq!(
            verified_side_chat_source(&marked("review:alpha"), None, Some(&source)),
            Some("review:alpha".to_string())
        );
    }

    fn definition(toml: &str) -> meerkat_mob::MobDefinition {
        meerkat_mob::MobDefinition::from_toml(toml).expect("definition parses")
    }

    const PROFILES: &str = r#"
[profiles.lead]
model = "gpt-5.5"

[profiles.worker]
model = "gpt-5.5"

[profiles.analyst]
model = "gpt-5.5"
"#;

    #[test]
    fn definition_wiring_names_the_rule_that_would_wire_a_role() {
        let unwired = definition(&format!("[mob]\nid = \"m\"\n{PROFILES}"));
        assert_eq!(definition_wiring_for_role(&unwired, "worker"), None);

        let orchestrated = definition(&format!(
            "[mob]\nid = \"m\"\norchestrator = \"lead\"\n{PROFILES}\n[wiring]\nauto_wire_orchestrator = true\n"
        ));
        assert_eq!(
            definition_wiring_for_role(&orchestrated, "worker"),
            Some(DefinitionWiring::AutoWireOrchestrator {
                orchestrator_role: "lead".to_string()
            })
        );
        assert_eq!(
            definition_wiring_for_role(&orchestrated, "lead"),
            None,
            "the orchestrator itself is not wired to itself"
        );

        let role_wired = definition(&format!(
            "[mob]\nid = \"m\"\n{PROFILES}\n[[wiring.role_wiring]]\na = \"worker\"\nb = \"analyst\"\n"
        ));
        assert_eq!(
            definition_wiring_for_role(&role_wired, "analyst"),
            Some(DefinitionWiring::RoleWiring {
                peer_role: "worker".to_string()
            })
        );
        assert_eq!(definition_wiring_for_role(&role_wired, "lead"), None);
    }

    fn activation_identity(
        session: &SessionId,
        activation: &str,
        body: &str,
    ) -> InstructionActivationIdentity {
        InstructionActivationIdentity {
            activation_id: InstructionActivationId::new(activation).unwrap(),
            revision: meerkat_core::types::InstructionRevisionRef {
                namespace: InstructionNamespace::new("app.skills").unwrap(),
                key: InstructionKey::new("calendar").unwrap(),
                revision_id: meerkat_core::types::InstructionRevisionId::new("r1").unwrap(),
                content_sha256: InstructionContentDigest::for_body(body),
            },
            supersedes: None,
            origin_session_id: session.clone(),
            render_version: meerkat_core::INSTRUCTION_ACTIVATION_RENDER_VERSION_V1,
        }
    }

    #[test]
    fn an_activation_body_is_read_only_when_it_matches_its_digest() {
        let session = SessionId::new();
        let identity = activation_identity(&session, "a1", "be brief");
        assert_eq!(
            activation_body(&identity, "[header]\nkey=\"calendar\"\n\nbe brief"),
            Some("be brief")
        );
        assert_eq!(
            activation_body(&identity, "[header]\n\nsomething else"),
            None,
            "a body that does not match the declared digest is not inherited"
        );
        assert_eq!(activation_body(&identity, "no delimiter"), None);
    }

    #[test]
    fn an_inherited_activation_supersedes_the_source_head_with_a_stable_id() {
        let source_session = SessionId::new();
        let child_session = SessionId::new();
        let head = activation_identity(&source_session, "a1", "be brief");
        let request = inherited_activation_request(&child_session, &head, "be brief")
            .expect("the request builds");
        assert_eq!(request.revision, head.revision);
        assert_eq!(request.supersedes, Some(head.activation_id.clone()));
        assert_eq!(
            request.expectation,
            InstructionActivationExpectation::Absent
        );
        assert_eq!(request.body, "be brief");
        assert!(
            request
                .activation_id
                .as_str()
                .starts_with(INHERITED_ACTIVATION_ID_PREFIX)
        );
        assert_eq!(
            request.activation_id.as_str().len(),
            INHERITED_ACTIVATION_ID_PREFIX.len() + 32
        );
        let again = inherited_activation_request(&child_session, &head, "be brief").unwrap();
        assert_eq!(
            again.activation_id, request.activation_id,
            "the same fork names the same activation"
        );
        let other_child =
            inherited_activation_request(&SessionId::new(), &head, "be brief").unwrap();
        assert_ne!(other_child.activation_id, request.activation_id);
    }

    #[test]
    fn fork_member_params_name_both_members_and_take_no_prefix() {
        let params = parse_fork_member_params(&serde_json::json!({
            "source_member_id": " alpha ",
            "agent_identity": "alpha-side",
            "display_name": "Draft",
        }))
        .expect("params parse");
        assert_eq!(
            params,
            ForkMemberParams {
                source_member_id: "alpha".to_string(),
                agent_identity: "alpha-side".to_string(),
                display_name: Some("Draft".to_string()),
            }
        );
        assert!(
            parse_fork_member_params(&serde_json::json!({ "agent_identity": "alpha-side" }))
                .is_err()
        );
        assert!(
            parse_fork_member_params(&serde_json::json!({
                "source_member_id": "alpha",
                "agent_identity": "",
            }))
            .is_err()
        );
        assert!(
            parse_fork_member_params(&serde_json::json!({
                "source_member_id": "alpha",
                "agent_identity": "alpha-side",
                "message_count": 4,
            }))
            .is_err(),
            "a prefix is refused, not ignored"
        );
        assert!(
            parse_fork_member_params(&serde_json::json!({
                "source_member_id": "alpha",
                "agent_identity": "alpha-side",
                "message_count": null,
            }))
            .is_ok()
        );
    }

    #[test]
    fn a_running_source_is_refused_typed() {
        let error = fork_member_error(&SideChatError::Fork(ForkMemberError::Mob(
            meerkat_mob::MobError::ForkSourceUnavailable {
                source_member_id: "alpha".to_string(),
                cause: meerkat_mob::error::ForkSourceUnavailableCause::Running,
            },
        )));
        assert_eq!(error.code, SIDE_CHAT_FORK_REFUSED_CODE);
        assert_eq!(
            error.data,
            Some(serde_json::json!({
                "kind": "fork_source_unavailable",
                "source_member_id": "alpha",
                "cause": "running",
            }))
        );
    }

    #[test]
    fn a_wired_role_and_a_committed_fork_keep_their_typed_data() {
        let wired = fork_member_error(&SideChatError::WiredRole {
            source_member_id: "beta".to_string(),
            role: "worker".to_string(),
            wiring: DefinitionWiring::RoleWiring {
                peer_role: "analyst".to_string(),
            },
        });
        assert_eq!(wired.code, SIDE_CHAT_FORK_REFUSED_CODE);
        assert_eq!(
            wired.data,
            Some(serde_json::json!({
                "kind": "unwired_seat_unavailable",
                "source_member_id": "beta",
                "role": "worker",
                "wiring": { "rule": "role_wiring", "peer_role": "analyst" },
            }))
        );

        let fork_session_id = SessionId::new();
        let committed = fork_member_error(&SideChatError::Fork(ForkMemberError::Mob(
            meerkat_mob::MobError::ForkMemberProvisionFailed {
                member_id: meerkat_mob::AgentIdentity::from("beta-side"),
                fork_session_id: fork_session_id.clone(),
                reason: "resume provisioning failed".to_string(),
            },
        )));
        let data = committed
            .data
            .expect("a committed fork keeps its recovery data");
        assert_eq!(
            data["kind"],
            serde_json::json!("fork_member_provision_failed")
        );
        assert_eq!(
            data["fork_session_id"],
            serde_json::json!(fork_session_id.to_string())
        );
        assert_eq!(
            data["recovery"],
            serde_json::json!("resume_committed_fork_session")
        );
    }

    #[test]
    fn inherited_heads_skip_the_child_own_rows_and_keep_the_latest_copy() {
        let source_session = SessionId::new();
        let child_session = SessionId::new();
        let system = |identity: InstructionActivationIdentity, content: &str| {
            let mut message = meerkat_core::types::SystemMessage::new(content.to_string());
            message.instruction_activation = Some(identity);
            Message::System(message)
        };
        let first = activation_identity(&source_session, "a1", "one");
        let second = activation_identity(&source_session, "a2", "two");
        let own = activation_identity(&child_session, "a3", "three");
        let heads = inherited_activation_heads(
            &child_session,
            &[
                system(first, "h\n\none"),
                system(second.clone(), "h\n\ntwo"),
                system(own, "h\n\nthree"),
            ],
        );
        let key = (
            InstructionNamespace::new("app.skills").unwrap(),
            InstructionKey::new("calendar").unwrap(),
        );
        let (head, content) = heads.get(&key).expect("the key has an inherited head");
        assert_eq!(head.activation_id, second.activation_id);
        assert_eq!(content, "h\n\ntwo");
    }
}
