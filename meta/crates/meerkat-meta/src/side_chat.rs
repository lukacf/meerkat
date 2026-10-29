//! Console side chats: a human's private branch of an agent's conversation.
//!
//! The console's `meta/fork_member` forks an agent's durable transcript into a
//! new mob-plane member the human continues in, while the source agent's
//! session, transcript and queued work stay untouched. The child:
//!
//! - takes its source's role, profile and model overrides and runtime mode,
//!   and (through the durable fork) its application labels, context and tool
//!   overlay, so it runs with the source's tools;
//! - has exactly its source's audience: every agent check on a side chat is
//!   its source's. A rule that matches the source (by identity, role or
//!   labels) or the source's own lineage covers its side chats for every
//!   action, a deny on any of them hides them too, and a grant that matches
//!   only the side chat itself (its name, role or labels) grants nothing.
//!   Forking one needs `agent.view` and `agent.send` on the source and
//!   `agent.spawn` on the side chat's name, and is refused for a source the
//!   console's visibility policy hides and for a source whose lineage
//!   already fills the access rules' walk (the side chat's would not fit);
//! - is kept until the human closes it (`meta/retire_member`), which, as
//!   every agent check on it, needs `agent.retire` on its source: the fork
//!   sets an explicit keep-until-retired idle override, so a side chat never
//!   idle-retires. The runtime may still unload it while it is idle; that
//!   keeps the member;
//! - is never respawned or reset ([`side_chat_restart_refusal`]), by an
//!   agent's `mob_respawn` tool either, nor adopted as an identity
//!   ([`side_chat_adoption_refusal`]): Meerkat records a fork source only for
//!   the fork itself, so a successor would be no side chat of its source. The
//!   human closes it and forks the source again;
//! - is never wired. It has no peer edge, so no other agent can address it
//!   and none lists it as a peer. The fork seats it exempt from the mob
//!   definition's wiring rules (`meerkat_mob::DefinitionWiring::Exempt`):
//!   they wire it to no member when it is seated, and no member seated,
//!   spawned or rotated later to it, whatever the source's role. Meta's
//!   definition-wiring reconcile and topology leave every exempt member out
//!   too;
//! - names its source in the [`SIDE_CHAT_OF_LABEL`] roster label. The label
//!   counts only where Meerkat's roster agrees ([`verified_side_chat_source`]):
//!   the member's typed `fork_source` names that same source and no member's
//!   own turn created it. The console strips the roster label from every
//!   projection and writes the verified relation into the side chat's console
//!   labels instead ([`project_side_chat_lineage`]): its source's console
//!   identity and the source session it was forked from. The access rules'
//!   lineage walk reads them, and a side chat nests under its source across
//!   restarts.
//!
//! Instructions: a durable fork copies the source's activated instructions as
//! history only. For each inherited key whose head is an activation the
//! source made itself, the fork activates the same revision and body in the
//! child as the host (`meta/fork_member` is the host's fork), recorded as
//! superseding the source's activation. Any other inherited key stays
//! inactive, and the child refuses its first turn naming it, typed, until a
//! host activates it.

use std::collections::BTreeMap;

use meerkat_core::types::{InstructionKey, InstructionNamespace, Message, SessionId};
use meerkat_core::{
    InstructionActivationExpectation, InstructionActivationId, InstructionActivationIdentity,
    InstructionActivationRequest, InstructionContentDigest,
};
use meerkat_mob::{MobSessionService as _, SpawnMemberSpec};
use serde::Serialize;

use crate::fork::{ForkChildRetention, ForkMemberError, ForkMemberOutcome};
use crate::mob_handle_runtime::MobRuntime;

/// The label a side chat names its source in. On Meerkat's roster it is the
/// marker the fork writes: the source's public member alias, which counts
/// only where the roster agrees ([`verified_side_chat_source`]). In the
/// console's labels of a member it is the verified source's console
/// identity, which only [`project_side_chat_lineage`] writes; every
/// projection strips it from roster, spawn and registry labels.
pub const SIDE_CHAT_OF_LABEL: &str = "side_chat_of";

/// The console label a verified side chat names the source session it was
/// forked from in. It binds the side chat's lineage to that incarnation of
/// its source, as `spawned_by_session` binds a spawned member's to its
/// spawner's.
pub const SIDE_CHAT_OF_SESSION_LABEL: &str = "side_chat_of_session";

const DISPLAY_NAME_LABEL: &str = "display_name";
/// Console labels of the source that describe the source itself, not its
/// conversation, with the value a side chat takes instead when its source
/// carries them. A fork copies every application label of its source, and a
/// child's own label wins key by key, so each one present is overridden: a
/// side chat is addressable, closable, not watched, degraded or alerting,
/// has an empty inbox, and is never idle-retired by a label.
const SOURCE_ONLY_LABELS: [(&str, &str); 8] = [
    ("addressable", "true"),
    ("singleton", "false"),
    ("console_watched", "false"),
    ("console_degraded", "false"),
    ("console_degraded_reason", ""),
    ("console_alert_level", "none"),
    ("console_inbox_count", "0"),
    (
        crate::mob_handle_runtime::DELEGATE_IDLE_RETIRE_SECS_LABEL,
        crate::mob_handle_runtime::DELEGATE_IDLE_RETIRE_DISABLED_LABEL,
    ),
];
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

/// A verified side chat's lineage, as Meerkat's durable roster records its
/// fork.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SideChatLineage {
    /// The source's console identity: its durable identity label while it is
    /// on the roster, else the durable identity its roster id names.
    pub(crate) source_identity: String,
    /// The source session the side chat's transcript was forked from.
    pub(crate) source_session_id: String,
}

/// The roster facts of one member that decide whether it is a side chat. The
/// roster entry type is not nameable outside meerkat-mob, so callers lend its
/// fields.
pub(crate) struct SideChatRosterRow<'a> {
    /// The member's public alias.
    pub(crate) alias: String,
    pub(crate) labels: &'a BTreeMap<String, String>,
    pub(crate) spawned_by: Option<&'a meerkat_mob::AgentIdentity>,
    pub(crate) fork_source: Option<&'a meerkat_core::ForkBuildSource>,
}

/// Every verified side chat among `rows`, one mob's roster, keyed by the side
/// chat's public alias, with its lineage. A caller builds the rows only when
/// one carries the marker ([`any_side_chat_marker`]): no other row is one.
pub(crate) fn side_chat_lineages(
    rows: &[SideChatRosterRow<'_>],
) -> BTreeMap<String, SideChatLineage> {
    let console_identity_by_alias: BTreeMap<&str, &str> = rows
        .iter()
        .map(|row| {
            (
                row.alias.as_str(),
                crate::member_comms_id::durable_identity_label(row.labels)
                    .unwrap_or(row.alias.as_str()),
            )
        })
        .collect();
    rows.iter()
        .filter_map(|row| {
            let source_alias =
                verified_side_chat_source(row.labels, row.spawned_by, row.fork_source)?;
            let fork_source = row.fork_source?;
            let source_session_id = fork_source.source_session_id.to_string();
            let source_identity = match console_identity_by_alias.get(source_alias.as_str()) {
                Some(identity) => String::from(*identity),
                // A source that left the roster: the durable identity its
                // roster id names (a legacy `rt:{identity}:{generation}` id
                // names `{identity}`).
                None => crate::member_comms_id::durable_identity_for_roster_member(
                    &fork_source.source_member.member,
                ),
            };
            Some((
                row.alias.clone(),
                SideChatLineage {
                    source_identity,
                    source_session_id,
                },
            ))
        })
        .collect()
}

/// Every verified side chat of `handle`'s mob, keyed by the side chat's
/// public alias, with its lineage ([`side_chat_lineages`]). One read of the
/// roster's fork facts: a console read that has the member's runtime reads
/// them through its [`SideChatLineageCache`].
pub(crate) async fn verified_side_chats(
    handle: &meerkat_mob::MobHandle,
) -> BTreeMap<String, SideChatLineage> {
    read_verified_side_chats(handle).await.0
}

/// [`verified_side_chats`], with each marked incarnation the roster lists:
/// its runtime id, fence token and lineage (`None` for a marked member that
/// is not a verified side chat).
async fn read_verified_side_chats(
    handle: &meerkat_mob::MobHandle,
) -> (BTreeMap<String, SideChatLineage>, Vec<ListedSideChatMarker>) {
    let entries = handle.list_all_members().await;
    if !any_side_chat_marker(entries.iter().map(|entry| &entry.labels)) {
        return (BTreeMap::new(), Vec::new());
    }
    let rows = entries
        .iter()
        .map(|entry| SideChatRosterRow {
            alias: crate::member_comms_id::runtime_alias_str(entry.agent_identity.as_str())
                .into_owned(),
            labels: &entry.labels,
            spawned_by: entry.spawned_by.as_ref(),
            fork_source: entry.fork_source.as_ref(),
        })
        .collect::<Vec<_>>();
    let lineages = side_chat_lineages(&rows);
    let marked = entries
        .iter()
        .zip(&rows)
        .filter(|(_, row)| row.labels.contains_key(SIDE_CHAT_OF_LABEL))
        .map(|(entry, row)| ListedSideChatMarker {
            runtime_id: entry.agent_runtime_id.clone(),
            fence_token: entry.fence_token,
            lineage: lineages.get(&row.alias).cloned(),
        })
        .collect();
    (lineages, marked)
}

/// One incarnation a roster read listed with the side chat marker.
struct ListedSideChatMarker {
    runtime_id: meerkat_mob::AgentRuntimeId,
    fence_token: meerkat_mob::FenceToken,
    lineage: Option<SideChatLineage>,
}

/// One member incarnation of one mob, as [`SideChatLineageCache`] keys it:
/// the mob, the incarnation's runtime id and its fence token.
type SideChatIncarnation = (String, meerkat_mob::AgentRuntimeId, meerkat_mob::FenceToken);

/// The most incarnations a [`SideChatLineageCache`] keeps; a full cache
/// starts over.
const SIDE_CHAT_LINEAGE_CACHE_LIMIT: usize = 4096;

/// The verified side chat lineage of each member incarnation one runtime's
/// console has read.
///
/// A member's fork facts (its side chat marker, its typed `fork_source` and
/// its `spawned_by`) never change for an incarnation, so its lineage is read
/// from Meerkat's roster once ([`meerkat_mob::MobHandle::list_all_members`],
/// which clones the mob's machine state) and then kept: the events of a
/// side chat's stream and the identity records of every console read take it
/// from here. A read keeps the lineage of every marked incarnation the
/// roster lists. An incarnation the roster no longer lists is kept without
/// one: a later incarnation replaced it, and none of its fork facts can
/// change.
#[derive(Default)]
pub(crate) struct SideChatLineageCache {
    lineages: std::sync::Mutex<BTreeMap<SideChatIncarnation, Option<SideChatLineage>>>,
    /// How often the roster's fork facts were read.
    reads: std::sync::atomic::AtomicUsize,
}

impl SideChatLineageCache {
    /// Every verified side chat of `handle`'s mob ([`verified_side_chats`]),
    /// read now, keeping the lineage of each marked incarnation the roster
    /// lists.
    pub(crate) async fn verified(
        &self,
        handle: &meerkat_mob::MobHandle,
    ) -> BTreeMap<String, SideChatLineage> {
        let (lineages, marked) = read_verified_side_chats(handle).await;
        self.reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.keep(&handle.mob_id().to_string(), marked);
        lineages
    }

    /// The lineage of one incarnation of a member of `handle`'s mob (its
    /// runtime id and fence token) whose roster labels are `labels`, when it
    /// is a verified side chat. A member without the side chat marker has
    /// none, and nothing is read; a marked incarnation's lineage is read from
    /// the roster the first time it is asked for, then kept.
    pub(crate) async fn lineage(
        &self,
        handle: &meerkat_mob::MobHandle,
        runtime_id: &meerkat_mob::AgentRuntimeId,
        fence_token: meerkat_mob::FenceToken,
        labels: &BTreeMap<String, String>,
    ) -> Option<SideChatLineage> {
        if !labels.contains_key(SIDE_CHAT_OF_LABEL) {
            return None;
        }
        let key = (handle.mob_id().to_string(), runtime_id.clone(), fence_token);
        let kept = self.locked().get(&key).cloned();
        if let Some(kept) = kept {
            return kept;
        }
        self.verified(handle).await;
        self.kept_or_none(key)
    }

    /// Keep the lineages a caller read from `mob_id`'s roster itself, one
    /// per marked incarnation it listed (its runtime id, fence token and
    /// lineage, `None` for a marked member that is not a verified side chat):
    /// a route's access priming lists the roster anyway, so a later read of
    /// those incarnations reads nothing.
    pub(crate) fn keep_listed(
        &self,
        mob_id: &str,
        listed: impl IntoIterator<
            Item = (
                meerkat_mob::AgentRuntimeId,
                meerkat_mob::FenceToken,
                Option<SideChatLineage>,
            ),
        >,
    ) {
        self.keep(
            mob_id,
            listed
                .into_iter()
                .map(|(runtime_id, fence_token, lineage)| ListedSideChatMarker {
                    runtime_id,
                    fence_token,
                    lineage,
                })
                .collect(),
        );
    }

    /// How often the roster's fork facts were read.
    #[cfg(test)]
    pub(crate) fn reads(&self) -> usize {
        self.reads.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn locked(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<SideChatIncarnation, Option<SideChatLineage>>> {
        self.lineages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The kept lineage of `key`, keeping none for an incarnation the last
    /// read did not list.
    fn kept_or_none(&self, key: SideChatIncarnation) -> Option<SideChatLineage> {
        self.locked().entry(key).or_insert(None).clone()
    }

    fn keep(&self, mob_id: &str, marked: Vec<ListedSideChatMarker>) {
        let mut lineages = self.locked();
        if lineages.len() >= SIDE_CHAT_LINEAGE_CACHE_LIMIT {
            lineages.clear();
        }
        for listed in marked {
            lineages.insert(
                (mob_id.to_string(), listed.runtime_id, listed.fence_token),
                listed.lineage,
            );
        }
    }
}

/// The lineage of `member_id`, a member of `handle`'s mob whose roster labels
/// are `labels`, when it is a verified side chat. Reads the roster's fork
/// facts only when `labels` carry the marker, and keeps nothing: a caller
/// with the member's runtime and incarnation asks its
/// [`SideChatLineageCache`].
pub(crate) async fn verified_side_chat_lineage(
    handle: &meerkat_mob::MobHandle,
    member_id: &meerkat_mob::AgentIdentity,
    labels: &BTreeMap<String, String>,
) -> Option<SideChatLineage> {
    if !labels.contains_key(SIDE_CHAT_OF_LABEL) {
        return None;
    }
    let alias = crate::member_comms_id::runtime_alias_str(member_id.as_str()).into_owned();
    verified_side_chats(handle).await.remove(&alias)
}

/// Write a verified side chat's lineage into its console labels: its
/// source's console identity ([`SIDE_CHAT_OF_LABEL`]) and the source session
/// it was forked from ([`SIDE_CHAT_OF_SESSION_LABEL`]). Applied after the
/// roster labels lost every unverified lineage claim and took the spawn
/// registry's gaps. The access rules follow the label to the source, so a
/// side chat's audience is its source's. Meerkat's roster says no agent
/// spawned a side chat, so a spawn lineage the registry holds under its
/// identity names an earlier member and is dropped.
pub(crate) fn project_side_chat_lineage(
    labels: &mut BTreeMap<String, String>,
    lineage: Option<&SideChatLineage>,
) {
    let Some(lineage) = lineage else {
        return;
    };
    crate::console_spawn::sanitize_unverified_lineage_labels(labels);
    labels.insert(
        SIDE_CHAT_OF_LABEL.to_string(),
        lineage.source_identity.clone(),
    );
    labels.insert(
        SIDE_CHAT_OF_SESSION_LABEL.to_string(),
        lineage.source_session_id.clone(),
    );
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
    /// Under the launch's member auth binding, the source's session records
    /// another binding or none: Meerkat would seat the side chat from the
    /// source's records, off the launch binding. The member route refusal
    /// (`member_auth_binding_uncarried`), as the console answers it. Nothing
    /// was forked.
    BindingRefused(crate::rpc::JsonRpcError),
    /// The child was seated with peer edges after all. The caller retires it.
    WiredAtSeat {
        member_alias: String,
        member_id: meerkat_mob::AgentIdentity,
        peers: Vec<String>,
    },
    /// The fork read another session of the source than the one its checks
    /// read: the source moved to another incarnation in between (a reset or
    /// a respawn ran), so the child is no fork the checks covered. The caller
    /// retires it.
    SourceChanged {
        member_alias: String,
        member_id: meerkat_mob::AgentIdentity,
        source_member_id: String,
    },
}

impl SideChatError {
    /// The seated child the caller retires again: a side chat seated with
    /// peer edges, or forked from a session its checks did not read.
    pub(crate) fn seated_child_to_retire(&self) -> Option<&meerkat_mob::AgentIdentity> {
        match self {
            Self::WiredAtSeat { member_id, .. } | Self::SourceChanged { member_id, .. } => {
                Some(member_id)
            }
            _ => None,
        }
    }
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
            Self::BindingRefused(error) => f.write_str(&error.message),
            Self::WiredAtSeat {
                member_alias,
                peers,
                ..
            } => write!(
                f,
                "the side chat {member_alias} was seated with peer edges ({}) and is retired",
                peers.join(", ")
            ),
            Self::SourceChanged {
                member_alias,
                source_member_id,
                ..
            } => write!(
                f,
                "the fork source {source_member_id} moved to another session while the side \
                 chat {member_alias} was forked; the side chat is retired, fork again"
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
/// a source whose session was never born (seated and never woken: there is
/// no conversation to fork, and the fork does not wake it), typed as a
/// source without a session (`ForkSourceUnavailable`, cause `no_session`),
/// and, under the launch's member auth binding, a source whose session
/// records another binding or none (`member_auth_binding_uncarried`). The
/// durable fork refuses a source that is running (cause `running`), typed. A
/// dormant source forks from its stored session without waking.
///
/// The fork is bound to the source session those checks read: when the
/// seated child records another one as its fork source (the source was
/// reset or respawned in between), the child is answered
/// [`SideChatError::SourceChanged`], for the caller to retire.
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
    // The session the fork reads, as the roster names it now; the child's
    // durable fork source names the exact one once it is seated.
    let source_session_before_fork = source.bridge_session_id().cloned();
    // A seat that was never woken has no session in any store: nothing to
    // fork, and waking it would run its first build (and any kickoff) only
    // to fork an empty conversation.
    if source_session_before_fork
        .as_ref()
        .is_some_and(|session| handle.session_unborn(session))
    {
        return Err(source_without_session(&source_alias));
    }
    // Meerkat seats a fork child from its source's records and asks no spawn
    // customizer, so under the launch's member auth binding a source whose
    // session records another binding, or none, would seat a side chat the
    // member build guard refuses only after the fork committed its session.
    if let Some(session) = source_session_before_fork.as_ref()
        && let Some(refusal) = mob_runtime
            .fork_binding_refusal("meta/fork_member", session)
            .await
    {
        return Err(SideChatError::BindingRefused(refusal.json_rpc_error()));
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
    // key by key: the marker names this source, the display name keeps the
    // side chat apart from the agent it branched from, and the source's own
    // console flags do not follow it.
    let mut labels = BTreeMap::from([
        (SIDE_CHAT_OF_LABEL.to_string(), source_alias.clone()),
        (DISPLAY_NAME_LABEL.to_string(), display_name.clone()),
    ]);
    for (key, value) in SOURCE_ONLY_LABELS {
        if source.labels.contains_key(key) {
            labels.insert(key.to_string(), value.to_string());
        }
    }
    // The source's role, profile and model overrides and runtime mode, as a
    // `fork_off` child gets them: the side chat runs with the source's tools.
    // It is exempt from the mob definition's wiring rules, so it is seated
    // with no peer edge, and no member seated after it is wired to it.
    let mut spec = SpawnMemberSpec::new(source.role, meerkat_mob::AgentIdentity::from(child_alias))
        .with_runtime_mode(source.runtime_mode)
        .with_labels(labels)
        .with_definition_wiring(meerkat_mob::DefinitionWiring::Exempt);
    spec.override_profile = source.effective_profile_override;
    spec.model_override = source.effective_model_override;

    let outcome = match crate::fork::fork_member_on_runtime(
        mob_runtime,
        identity_runtime,
        source_member_alias,
        spec,
        None,
        ForkChildRetention::UntilRetired,
        None,
    )
    .await
    {
        Ok(outcome) => outcome,
        // The store holds no session the roster names for the source (it
        // was never born after all): the same refusal, nothing was forked.
        Err(ForkMemberError::Mob(meerkat_mob::MobError::SessionError(
            meerkat_core::service::SessionError::NotFound { id },
        ))) if source_session_before_fork.as_ref() == Some(&id) => {
            return Err(source_without_session(&source_alias));
        }
        Err(error) => return Err(SideChatError::Fork(error)),
    };

    let child_id = outcome.result.agent_identity.clone();
    // The seated child as the roster holds it. The fork has committed, so a
    // failed read does not undo it: the side chat is answered, and only what
    // the read would have added (the exact source session) falls back.
    let child = match handle.get_member(&child_id).await {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(
                member_id = %child_id,
                error = %error,
                "a seated side chat's roster entry could not be read"
            );
            None
        }
    };
    // The checks above read one session of the source; the side chat must
    // be a fork of that one. A reset or a respawn of the source between the
    // read and the fork seats it from another session none of them covered.
    if forked_another_session(
        source_session_before_fork.as_ref(),
        child
            .as_ref()
            .and_then(|child| child.fork_source.as_ref())
            .map(|fork_source| &fork_source.source_session_id),
    ) {
        return Err(SideChatError::SourceChanged {
            member_alias: outcome.member_alias,
            member_id: child_id,
            source_member_id: source_alias,
        });
    }
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
        .map(|fork_source| fork_source.source_session_id)
        .or(source_session_before_fork);
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

/// Whether a seated side chat was forked from another session of its source
/// (`forked_from`, its durable fork source) than `read`, the session the
/// fork's checks read. A side unknown on either side is no evidence of a
/// change.
fn forked_another_session(read: Option<&SessionId>, forked_from: Option<&SessionId>) -> bool {
    matches!((read, forked_from), (Some(read), Some(forked_from)) if read != forked_from)
}

/// The typed refusal of a source with no session to fork: Meerkat's own
/// `ForkSourceUnavailable` with cause `no_session`, so the console answers it
/// as it answers that refusal.
fn source_without_session(source_member_id: &str) -> SideChatError {
    SideChatError::Fork(ForkMemberError::Mob(
        meerkat_mob::MobError::ForkSourceUnavailable {
            source_member_id: source_member_id.to_string(),
            cause: meerkat_mob::error::ForkSourceUnavailableCause::NoSession,
        },
    ))
}

/// JSON-RPC code of a typed `meta/fork_member` refusal. `data.kind` is
/// `fork_source_unavailable` (with `source_member_id` and `cause`: `running`,
/// `no_session`, `remote_read_unavailable`, or `authority_unavailable` with
/// the identity authority's refusal in `detail`), and nothing was forked;
/// `fork_source_changed` (with `agent_identity` and `source_member_id`), a
/// child forked from another session of the source than the fork's checks
/// read; or `unwired_seat_unavailable` (`wiring.rule` `seated_with_peers`),
/// a child seated with peer edges after all. For the last two, `retired`
/// says whether the child was retired again.
pub const SIDE_CHAT_FORK_REFUSED_CODE: i64 = -32055;

/// JSON-RPC code of a refused respawn or reset of a side chat
/// ([`side_chat_restart_refusal`]): `data.kind` `side_chat_restart_refused`.
pub const SIDE_CHAT_RESTART_REFUSED_CODE: i64 = -32602;

/// The verified source of `member_id`, a member of `handle`'s mob, when it
/// is a side chat ([`verified_side_chat_source`]): `Ok(None)` for any other
/// member and for one the roster does not hold. A roster entry that cannot be
/// read answers `-32603` naming `operation`: whether the member is a side
/// chat is not known, so the caller refuses.
async fn side_chat_source_of(
    handle: &meerkat_mob::MobHandle,
    member_id: &meerkat_mob::AgentIdentity,
    operation: &str,
) -> Result<Option<String>, crate::rpc::JsonRpcError> {
    match handle.get_member(member_id).await {
        Ok(Some(entry)) => Ok(verified_side_chat_source(
            &entry.labels,
            entry.spawned_by.as_ref(),
            entry.fork_source.as_ref(),
        )),
        Ok(None) => Ok(None),
        Err(error) => Err(crate::rpc::JsonRpcError {
            code: -32603,
            message: format!("{operation} failed: the member could not be read: {error}"),
            data: None,
        }),
    }
}

/// The refusal of `operation` (`respawn` or `reset`) on `member_id`, a
/// member of `handle`'s mob, when it is a verified side chat
/// ([`verified_side_chat_source`]). Either would seat a successor no fork
/// made: Meerkat records a fork source only for the fork itself, so the
/// successor would no longer be decided as the agent it branched from and
/// would take the edges the definition declares. A side chat is closed and
/// forked again instead. A member whose roster entry cannot be read is
/// refused too, since whether it is a side chat is not known. `None` for
/// any other member and for one the roster does not hold, which the
/// operation answers itself.
pub(crate) async fn side_chat_restart_refusal(
    handle: &meerkat_mob::MobHandle,
    member_id: &meerkat_mob::AgentIdentity,
    operation: &str,
) -> Option<crate::rpc::JsonRpcError> {
    let source = match side_chat_source_of(handle, member_id, operation).await {
        Ok(source) => source?,
        Err(refusal) => return Some(refusal),
    };
    let alias = crate::member_comms_id::runtime_alias_str(member_id.as_str()).into_owned();
    let done = if operation == "reset" {
        "reset"
    } else {
        "respawned"
    };
    Some(crate::rpc::JsonRpcError {
        code: SIDE_CHAT_RESTART_REFUSED_CODE,
        message: format!(
            "{alias} is a side chat of {source} and cannot be {done}: close it and fork \
             {source} again"
        ),
        data: Some(serde_json::json!({
            "kind": "side_chat_restart_refused",
            "operation": operation,
            "agent_identity": alias,
            "source_member_id": source,
        })),
    })
}

/// JSON-RPC code of a refused adoption of a side chat as an identity
/// ([`side_chat_adoption_refusal`]): `data.kind`
/// `side_chat_adoption_refused`.
pub const SIDE_CHAT_ADOPTION_REFUSED_CODE: i64 = -32602;

/// The refusal of adopting `member_id`, a member of `handle`'s mob, as an
/// identity (`mob/adopt_member_identity_declaration`) when it is a verified
/// side chat. An adopted identity is rebuilt from its declaration, which no
/// fork made: its next incarnation would be no side chat of its agent, and
/// the identity verbs would respawn and reset it. A member whose roster
/// entry cannot be read is refused too. `None` for any other member and for
/// one the roster does not hold, which the adoption answers itself.
pub(crate) async fn side_chat_adoption_refusal(
    handle: &meerkat_mob::MobHandle,
    member_id: &meerkat_mob::AgentIdentity,
) -> Option<crate::rpc::JsonRpcError> {
    let source = match side_chat_source_of(handle, member_id, "adoption").await {
        Ok(source) => source?,
        Err(refusal) => return Some(refusal),
    };
    let alias = crate::member_comms_id::runtime_alias_str(member_id.as_str()).into_owned();
    Some(crate::rpc::JsonRpcError {
        code: SIDE_CHAT_ADOPTION_REFUSED_CODE,
        message: format!(
            "{alias} is a side chat of {source} and cannot be adopted as an identity: an \
             identity is rebuilt from its declaration, which would be no fork of {source}"
        ),
        data: Some(serde_json::json!({
            "kind": "side_chat_adoption_refused",
            "agent_identity": alias,
            "source_member_id": source,
        })),
    })
}

/// The typed tool error an agent's mob tool answers a side chat refusal
/// with: a policy denial carrying the refusal's kind
/// (`side_chat_restart_refused`), as an agent meets the tools' other typed
/// refusals. A refusal without a kind (the member could not be read) is a
/// plain tool failure.
pub(crate) fn side_chat_refusal_tool_error(
    refusal: crate::rpc::JsonRpcError,
) -> meerkat_core::ToolError {
    match refusal
        .data
        .as_ref()
        .and_then(|data| data.get("kind"))
        .and_then(serde_json::Value::as_str)
    {
        Some(kind) => meerkat_core::ToolError::policy_denied(
            meerkat_core::ToolConsequenceDenial::new(kind, refusal.message),
        ),
        None => meerkat_core::ToolError::other(refusal.message),
    }
}

/// Whether `runtime` can hold a durable fork, and so offers side chats in its
/// `meta/capabilities`: its sessions persist, or its composition did not
/// record whether they do (the method is offered then, and the fork itself
/// refuses typed where the sessions do not persist). A runtime known to
/// persist no sessions never offers side chats.
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
        SideChatError::BindingRefused(refusal) => refusal.clone(),
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
        SideChatError::SourceChanged {
            member_alias,
            source_member_id,
            ..
        } => fork_refusal(
            error.to_string(),
            serde_json::json!({
                "kind": "fork_source_changed",
                "agent_identity": member_alias,
                "source_member_id": source_member_id,
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
    let heads = inherited_activation_heads(child_session, messages);
    if heads.is_empty() {
        return Vec::new();
    }
    let requiring = meerkat_core::session::inherited_instruction_keys_requiring_activation(
        child_session,
        messages,
    );
    let handle = mob_runtime.handle();
    let mut instructions = Vec::with_capacity(heads.len());
    for (key, (head, content)) in &heads {
        let mut instruction = SideChatInstruction {
            namespace: key.0.as_str().to_string(),
            key: key.1.as_str().to_string(),
            revision_id: head.revision.revision_id.as_str().to_string(),
            content_sha256: head.revision.content_sha256.to_string(),
            state: SideChatInstructionState::Inactive,
            reason: None,
        };
        // A key the child already activated itself (the gateway's
        // `reactivate_inherited` fork policy wrote it) is in effect.
        if !requiring.contains(key) {
            instruction.state = SideChatInstructionState::Inherited;
            instructions.push(instruction);
            continue;
        }
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
    if identity.render_version != meerkat_core::INSTRUCTION_ACTIVATION_RENDER_VERSION_V1 {
        return None;
    }
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

/// Append `request` to the side chat's session at its safe boundary. Meerkat
/// wakes a side chat that is not materialized before it appends.
async fn activate_in_child(
    handle: &meerkat_mob::MobHandle,
    child: &meerkat_mob::AgentIdentity,
    request: InstructionActivationRequest,
) -> Result<(), String> {
    handle
        .activate_member_instruction(child, request)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
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

    /// The lineage of each verified side chat among one roster's rows: its
    /// source's console identity (the source's durable identity label while
    /// it is on the roster, else its alias) and the forked session. No other
    /// row has one.
    #[test]
    fn side_chat_lineages_name_the_source_identity_and_the_forked_session() {
        let source = fork_source("alpha");
        let session = source.source_session_id.to_string();
        let spawner = meerkat_mob::AgentIdentity::from("alpha");
        let source_labels =
            BTreeMap::from([("agent_identity".to_string(), "domain:alpha".to_string())]);
        let side_labels = marked("alpha");
        let rows = [
            SideChatRosterRow {
                alias: "alpha".to_string(),
                labels: &source_labels,
                spawned_by: None,
                fork_source: None,
            },
            SideChatRosterRow {
                alias: "alpha-side".to_string(),
                labels: &side_labels,
                spawned_by: None,
                fork_source: Some(&source),
            },
            SideChatRosterRow {
                alias: "alpha-fork".to_string(),
                labels: &side_labels,
                spawned_by: Some(&spawner),
                fork_source: Some(&source),
            },
        ];
        assert_eq!(
            side_chat_lineages(&rows),
            BTreeMap::from([(
                "alpha-side".to_string(),
                SideChatLineage {
                    source_identity: "domain:alpha".to_string(),
                    source_session_id: session,
                },
            )]),
            "only the side chat has a lineage, and it names the source's console identity"
        );
        assert_eq!(
            side_chat_lineages(&rows[1..])
                .get("alpha-side")
                .map(|lineage| lineage.source_identity.as_str()),
            Some("alpha"),
            "a source that left the roster is named by the identity its roster id names"
        );
        let legacy_source = fork_source("rt:alpha:0");
        let legacy_labels = marked("rt:alpha:0");
        assert_eq!(
            side_chat_lineages(&[SideChatRosterRow {
                alias: "alpha-legacy-side".to_string(),
                labels: &legacy_labels,
                spawned_by: None,
                fork_source: Some(&legacy_source),
            }])
            .get("alpha-legacy-side")
            .map(|lineage| lineage.source_identity.as_str()),
            Some("alpha"),
            "a legacy runtime-alias roster id names its durable identity, not the alias"
        );
        let unmarked = BTreeMap::new();
        assert!(
            side_chat_lineages(&[SideChatRosterRow {
                alias: "beta".to_string(),
                labels: &unmarked,
                spawned_by: None,
                fork_source: Some(&source),
            }])
            .is_empty(),
            "a durable fork without the marker has no side chat lineage"
        );
    }

    /// A verified side chat's console labels name its source and the forked
    /// session, and lose any spawn lineage the registry held under its
    /// identity; without a lineage nothing changes.
    #[test]
    fn the_projection_writes_only_the_verified_lineage() {
        let lineage = SideChatLineage {
            source_identity: "alpha".to_string(),
            source_session_id: "alpha-session".to_string(),
        };
        let mut labels = BTreeMap::from([
            ("team".to_string(), "review".to_string()),
            (
                crate::console_spawn::SPAWNED_BY_LABEL.to_string(),
                "earlier-parent".to_string(),
            ),
            (
                crate::console_spawn::SPAWNED_BY_SESSION_LABEL.to_string(),
                "earlier-session".to_string(),
            ),
        ]);
        let unchanged = labels.clone();
        project_side_chat_lineage(&mut labels, None);
        assert_eq!(labels, unchanged);
        project_side_chat_lineage(&mut labels, Some(&lineage));
        assert_eq!(
            labels,
            BTreeMap::from([
                ("team".to_string(), "review".to_string()),
                (SIDE_CHAT_OF_LABEL.to_string(), "alpha".to_string()),
                (
                    SIDE_CHAT_OF_SESSION_LABEL.to_string(),
                    "alpha-session".to_string()
                ),
            ])
        );
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

    /// A source refused under the launch's member auth binding answers the
    /// member route refusal as it is.
    #[test]
    fn a_binding_refusal_keeps_the_member_route_refusal() {
        let refusal = crate::rpc::JsonRpcError {
            code: -32602,
            message: "meta/fork_member cannot fork".to_string(),
            data: Some(serde_json::json!({ "kind": "member_auth_binding_uncarried" })),
        };
        let error = fork_member_error(&SideChatError::BindingRefused(refusal.clone()));
        assert_eq!(error, refusal);
        assert_eq!(
            SideChatError::BindingRefused(refusal).to_string(),
            "meta/fork_member cannot fork"
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

    /// A source with no session to fork is refused as Meerkat refuses it.
    #[test]
    fn a_source_without_a_session_is_refused_typed() {
        let error = fork_member_error(&source_without_session("alpha"));
        assert_eq!(error.code, SIDE_CHAT_FORK_REFUSED_CODE);
        assert_eq!(
            error.data,
            Some(serde_json::json!({
                "kind": "fork_source_unavailable",
                "source_member_id": "alpha",
                "cause": "no_session",
            }))
        );
    }

    #[test]
    fn a_wired_seat_and_a_committed_fork_keep_their_typed_data() {
        let wired = fork_member_error(&SideChatError::WiredAtSeat {
            member_alias: "beta-side".to_string(),
            member_id: meerkat_mob::AgentIdentity::from("beta-side"),
            peers: vec!["lead".to_string()],
        });
        assert_eq!(wired.code, SIDE_CHAT_FORK_REFUSED_CODE);
        assert_eq!(
            wired.data,
            Some(serde_json::json!({
                "kind": "unwired_seat_unavailable",
                "agent_identity": "beta-side",
                "wiring": { "rule": "seated_with_peers", "peers": ["lead"] },
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

    /// A side chat must be a fork of the source session its checks read: a
    /// child forked from another session (the source was reset or respawned
    /// between the read and the fork) is refused typed, and the caller
    /// retires it; an unknown side is no evidence of a change.
    #[test]
    fn a_fork_of_another_source_session_is_refused_for_the_caller_to_retire() {
        let read = SessionId::new();
        let other = SessionId::new();
        assert!(!forked_another_session(Some(&read), Some(&read)));
        assert!(forked_another_session(Some(&read), Some(&other)));
        assert!(!forked_another_session(None, Some(&other)));
        assert!(!forked_another_session(Some(&read), None));

        let changed = SideChatError::SourceChanged {
            member_alias: "alpha-side".to_string(),
            member_id: meerkat_mob::AgentIdentity::from("alpha-side"),
            source_member_id: "alpha".to_string(),
        };
        assert_eq!(
            changed.seated_child_to_retire(),
            Some(&meerkat_mob::AgentIdentity::from("alpha-side"))
        );
        let error = fork_member_error(&changed);
        assert_eq!(error.code, SIDE_CHAT_FORK_REFUSED_CODE);
        assert_eq!(
            error.data,
            Some(serde_json::json!({
                "kind": "fork_source_changed",
                "agent_identity": "alpha-side",
                "source_member_id": "alpha",
            }))
        );
        assert_eq!(
            SideChatError::UnknownSource {
                source_member_id: "alpha".to_string(),
            }
            .seated_child_to_retire(),
            None,
            "nothing was seated to retire"
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
