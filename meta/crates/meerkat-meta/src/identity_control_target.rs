//! Canonical identity-control target resolution.
//!
//! Transport layers own authorization envelopes and operation-specific
//! behavior. This module owns only the identity-to-live-member relation so
//! stdio JSON-RPC and the console cannot disagree about aliases, ambiguity,
//! durable bindings, or stale projected identities.

use std::collections::BTreeSet;

use meerkat_mob::MobHandle;

use crate::identity_first::{AgentIdentity, IdentityRuntime, IdentityStatus};

#[derive(Debug, Clone)]
pub(crate) struct LiveIdentityMember {
    pub(crate) identity: String,
    pub(crate) runtime_member_id: String,
    pub(crate) member: meerkat_mob::runtime::MobMemberListEntry,
    pub(crate) session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct IdentityControlTarget {
    pub(crate) identity: AgentIdentity,
    pub(crate) live: Option<LiveIdentityMember>,
    /// Resolution observed this identity in durable authority.
    ///
    /// A later `UnknownIdentity` means a concurrent delete won. Callers must
    /// not reinterpret the captured member as a legacy live-only identity.
    pub(crate) was_registered: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum IdentityControlResolution {
    Resolved(Box<IdentityControlTarget>),
    /// No durable or visible live target exists. Transports intentionally
    /// project this differently: console uses its unknown-member envelope,
    /// while stdio may carry a valid bare identity into durable operations.
    Unresolved {
        requested_identity: String,
        parsed_identity: Result<AgentIdentity, String>,
        generated_runtime_alias: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdentityControlResolutionError {
    Hidden {
        requested_identity: String,
    },
    Ambiguous {
        requested_identity: String,
        candidates: Vec<String>,
    },
    StaleProjectedBinding {
        identity: String,
        runtime_member_id: String,
        registered_identity: String,
    },
    InvalidProjectedIdentity {
        projected_identity: String,
        detail: String,
    },
    /// Core's member history could not be read, so no registered binding can
    /// be told apart from a live-only member: the control refuses rather
    /// than resolve against the live roster alone.
    HistoryUnavailable {
        requested_identity: String,
        detail: String,
    },
}

impl std::fmt::Display for IdentityControlResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hidden { requested_identity } => {
                write!(formatter, "identity hidden by policy: {requested_identity}")
            }
            Self::Ambiguous {
                requested_identity,
                candidates,
            } => write!(
                formatter,
                // Producer marker: three sites emitted this identical string, so a
                // failure named the candidate SET without naming who built it.
                "ambiguous live identity alias {requested_identity} \
                 [via identity-control-target resolver]: candidates [{}]",
                candidates.join(", ")
            ),
            Self::StaleProjectedBinding {
                identity,
                runtime_member_id,
                registered_identity,
            } => write!(
                formatter,
                "stale live identity alias: live console alias {identity} resolves to {runtime_member_id}, but identity runtime binding belongs to {registered_identity}"
            ),
            Self::InvalidProjectedIdentity {
                projected_identity,
                detail,
            } => write!(
                formatter,
                "invalid projected identity {projected_identity}: {detail}"
            ),
            Self::HistoryUnavailable {
                requested_identity,
                detail,
            } => write!(
                formatter,
                "identity {requested_identity} was not resolved: {detail}; nothing was changed"
            ),
        }
    }
}

impl std::error::Error for IdentityControlResolutionError {}

/// Resolve one control-plane identity against a single durable/live snapshot.
///
/// `member_visible` is supplied by the transport. It is the only policy input
/// and is applied before ambiguity is evaluated, preserving both stdio's
/// implicit-delegate rule and the console's configured member plus identity
/// visibility policy.
pub(crate) async fn resolve_identity_control_target<F>(
    handle: &MobHandle,
    identity_runtime: Option<&IdentityRuntime>,
    requested_identity: &str,
    member_visible: F,
) -> Result<IdentityControlResolution, IdentityControlResolutionError>
where
    F: Fn(&LiveIdentityMember) -> bool,
{
    let requested_identity =
        crate::member_comms_id::runtime_alias_str(requested_identity).into_owned();
    let generated_runtime_alias =
        crate::member_comms_id::is_reserved_generated_alias(&requested_identity);
    let statuses = match identity_runtime {
        Some(identity_runtime) => identity_runtime.statuses().await.map_err(|error| {
            IdentityControlResolutionError::HistoryUnavailable {
                requested_identity: requested_identity.clone(),
                detail: error.to_string(),
            }
        })?,
        None => Vec::new(),
    };
    let members = handle.list_members_including_retiring().await;
    let aliases = live_aliases(handle, members).await;

    let visible = |alias: &&LiveIdentityMember| member_visible(alias);
    let exact_runtime_alias = |runtime_member_id: &str| {
        aliases
            .iter()
            .find(|alias| alias.runtime_member_id == runtime_member_id)
    };
    let candidates = |identity: &str| {
        let matched = aliases
            .iter()
            .filter(|alias| live_alias_matches_request(alias, identity))
            .collect::<Vec<_>>();
        // An EXACT roster-id match wins outright. Since the stable-identity
        // lowering a durable identity usually IS a roster id, so a row that
        // merely carries the same agent_identity label would otherwise sit
        // alongside the identity's own row and make the set read as ambiguous
        // while that row is right there. Label-only matches remain the answer
        // when the request is not itself a roster id.
        if let Some(exact) = matched
            .iter()
            .find(|alias| alias.runtime_member_id == identity)
        {
            return vec![*exact];
        }
        matched
    };
    let visible_candidates = |identity: &str| {
        candidates(identity)
            .into_iter()
            .filter(visible)
            .collect::<Vec<_>>()
    };
    let unique_visible = |identity: &str| {
        let matches = visible_candidates(identity);
        if matches.len() > 1 {
            Err(IdentityControlResolutionError::Ambiguous {
                requested_identity: identity.to_string(),
                candidates: matches
                    .iter()
                    .map(|alias| alias.runtime_member_id.clone())
                    .collect(),
            })
        } else {
            Ok(matches.into_iter().next())
        }
    };
    let hidden_for_request = |identity: &str| {
        candidates(identity)
            .into_iter()
            .any(|alias| !member_visible(alias))
    };

    if generated_runtime_alias {
        if let Some(status) = status_for_runtime_alias(&statuses, &requested_identity) {
            if let Some(live) = exact_runtime_alias(&requested_identity) {
                if !member_visible(live) {
                    return Err(IdentityControlResolutionError::Hidden { requested_identity });
                }
                return Ok(resolved_registered(status.identity.clone(), Some(live)));
            }

            let live = unique_visible(status.identity.as_str())?;
            if live.is_none() && hidden_for_request(status.identity.as_str()) {
                return Err(IdentityControlResolutionError::Hidden { requested_identity });
            }
            return Ok(resolved_registered(status.identity.clone(), live));
        }

        if let Some(live) = unique_visible(&requested_identity)? {
            if let Some(status) = statuses
                .iter()
                .find(|status| status.identity.as_str() == live.identity)
                && !live_alias_matches_status_runtime(live, status)
            {
                return Ok(resolved_registered(status.identity.clone(), Some(live)));
            }
            reject_stale_projected_binding(&statuses, live)?;
            reject_duplicate_projected_identity(&aliases, &member_visible, live)?;
            return resolved_live_only(live)
                .map(Box::new)
                .map(IdentityControlResolution::Resolved);
        }
        if hidden_for_request(&requested_identity) {
            return Err(IdentityControlResolutionError::Hidden { requested_identity });
        }
        return Ok(IdentityControlResolution::Unresolved {
            parsed_identity: AgentIdentity::parse(&requested_identity)
                .map_err(|error| error.to_string()),
            requested_identity,
            generated_runtime_alias: true,
        });
    }

    let parsed_identity =
        AgentIdentity::parse(&requested_identity).map_err(|error| error.to_string());
    if let Ok(identity) = parsed_identity.as_ref()
        && let Some(status) = statuses.iter().find(|status| &status.identity == identity)
    {
        if let Some(runtime_member_id) = status
            .agent_runtime_id
            .as_ref()
            .map(crate::identity_first::AgentRuntimeId::as_str)
            && let Some(live) = exact_runtime_alias(runtime_member_id)
        {
            if !member_visible(live) {
                return Err(IdentityControlResolutionError::Hidden { requested_identity });
            }
            return Ok(resolved_registered(status.identity.clone(), Some(live)));
        }

        if status
            .agent_runtime_id
            .as_ref()
            .map(crate::identity_first::AgentRuntimeId::as_str)
            .is_some_and(|runtime_member_id| {
                exact_runtime_alias(runtime_member_id).is_some_and(|live| !member_visible(live))
            })
            || hidden_for_request(&requested_identity)
                && visible_candidates(&requested_identity).is_empty()
        {
            return Err(IdentityControlResolutionError::Hidden { requested_identity });
        }

        let live = unique_visible(&requested_identity)?;
        return Ok(resolved_registered(status.identity.clone(), live));
    }

    if let Some(status) = status_for_runtime_alias(&statuses, &requested_identity) {
        let exact_live =
            exact_runtime_alias(&requested_identity).filter(|alias| member_visible(alias));
        if exact_live.is_none()
            && exact_runtime_alias(&requested_identity).is_some_and(|alias| !member_visible(alias))
        {
            return Err(IdentityControlResolutionError::Hidden { requested_identity });
        }
        let durable_live = unique_visible(status.identity.as_str())?;
        let live = match (exact_live, durable_live) {
            (Some(exact), Some(durable))
                if exact.runtime_member_id == durable.runtime_member_id =>
            {
                Some(exact)
            }
            (Some(exact), None) => Some(exact),
            (Some(_), Some(durable)) => Some(durable),
            (None, durable) => durable,
        };
        return Ok(resolved_registered(status.identity.clone(), live));
    }

    if let Some(live) = unique_visible(&requested_identity)? {
        reject_stale_projected_binding(&statuses, live)?;
        reject_duplicate_projected_identity(&aliases, &member_visible, live)?;
        return resolved_live_only(live)
            .map(Box::new)
            .map(IdentityControlResolution::Resolved);
    }
    if hidden_for_request(&requested_identity) {
        return Err(IdentityControlResolutionError::Hidden { requested_identity });
    }

    Ok(IdentityControlResolution::Unresolved {
        requested_identity,
        parsed_identity,
        generated_runtime_alias: false,
    })
}

fn resolved_registered(
    identity: AgentIdentity,
    live: Option<&LiveIdentityMember>,
) -> IdentityControlResolution {
    IdentityControlResolution::Resolved(Box::new(IdentityControlTarget {
        identity,
        live: live.cloned(),
        was_registered: true,
    }))
}

fn resolved_live_only(
    live: &LiveIdentityMember,
) -> Result<IdentityControlTarget, IdentityControlResolutionError> {
    let identity = AgentIdentity::parse(&live.identity).map_err(|error| {
        IdentityControlResolutionError::InvalidProjectedIdentity {
            projected_identity: live.identity.clone(),
            detail: error.to_string(),
        }
    })?;
    Ok(IdentityControlTarget {
        identity,
        live: Some(live.clone()),
        was_registered: false,
    })
}

fn status_for_runtime_alias<'a>(
    statuses: &'a [IdentityStatus],
    runtime_member_id: &str,
) -> Option<&'a IdentityStatus> {
    statuses.iter().find(|status| {
        status
            .agent_runtime_id
            .as_ref()
            .is_some_and(|runtime_id| runtime_id.as_str() == runtime_member_id)
    })
}

fn live_alias_matches_status_runtime(live: &LiveIdentityMember, status: &IdentityStatus) -> bool {
    let session_matches = match (
        status.session_id.as_ref().map(ToString::to_string),
        live.session_id.as_deref(),
    ) {
        (Some(status_session), Some(live_session)) => status_session == live_session,
        _ => true,
    };
    status
        .agent_runtime_id
        .as_ref()
        .is_some_and(|runtime_id| runtime_id.as_str() == live.runtime_member_id)
        && status.identity.as_str() == live.identity
        && session_matches
}

fn live_alias_matches_request(alias: &LiveIdentityMember, requested_identity: &str) -> bool {
    alias.runtime_member_id == requested_identity || alias.identity == requested_identity
}

fn reject_stale_projected_binding(
    statuses: &[IdentityStatus],
    live: &LiveIdentityMember,
) -> Result<(), IdentityControlResolutionError> {
    if let Some(status) = status_for_runtime_alias(statuses, &live.runtime_member_id)
        && status.identity.as_str() != live.identity
    {
        return Err(IdentityControlResolutionError::StaleProjectedBinding {
            identity: live.identity.clone(),
            runtime_member_id: live.runtime_member_id.clone(),
            registered_identity: status.identity.as_str().to_string(),
        });
    }
    Ok(())
}

fn reject_duplicate_projected_identity<F>(
    aliases: &[LiveIdentityMember],
    member_visible: &F,
    live: &LiveIdentityMember,
) -> Result<(), IdentityControlResolutionError>
where
    F: Fn(&LiveIdentityMember) -> bool,
{
    let candidates = aliases
        .iter()
        .filter(|alias| live_alias_matches_request(alias, &live.identity) && member_visible(alias))
        .collect::<Vec<_>>();
    if candidates.len() > 1 {
        return Err(IdentityControlResolutionError::Ambiguous {
            requested_identity: live.identity.clone(),
            candidates: candidates
                .iter()
                .map(|alias| alias.runtime_member_id.clone())
                .collect(),
        });
    }
    Ok(())
}

async fn live_aliases(
    handle: &MobHandle,
    members: Vec<meerkat_mob::runtime::MobMemberListEntry>,
) -> Vec<LiveIdentityMember> {
    let mut seen_member_ids = BTreeSet::new();
    let mut aliases = Vec::with_capacity(members.len());
    for member in members {
        let runtime_member_id =
            crate::member_comms_id::runtime_alias_str(member.agent_identity.as_str()).into_owned();
        if !seen_member_ids.insert(runtime_member_id.clone()) {
            continue;
        }
        let identity = crate::member_comms_id::durable_identity_label(&member.labels)
            .map(str::to_owned)
            .unwrap_or_else(|| runtime_member_id.clone());
        let session_id = handle
            .resolve_bridge_session_id_observation(&member.agent_identity)
            .await
            .map(|session_id| session_id.to_string());
        aliases.push(LiveIdentityMember {
            identity,
            runtime_member_id,
            member,
            session_id,
        });
    }
    aliases
}

/// The typed `stale_identity_runtime_binding` refusal: the requested
/// `live_runtime_member_id` (with `live_session_id` when a live member
/// carries it) is not `identity`'s registered binding (`status`; `None` when
/// the identity runtime does not know the identity). Every surface builds it
/// here, so the six-field data object is the same everywhere; each keeps its
/// own `message_prefix` (for example `"reset failed: "`).
pub(crate) fn stale_identity_runtime_binding_error(
    message_prefix: &str,
    identity: &AgentIdentity,
    status: Option<&IdentityStatus>,
    live_runtime_member_id: &str,
    live_session_id: Option<&str>,
) -> crate::rpc::JsonRpcError {
    let registered_runtime_member_id = status
        .and_then(|status| status.agent_runtime_id.as_ref())
        .map(crate::identity_first::AgentRuntimeId::as_str);
    crate::rpc::JsonRpcError {
        code: -32000,
        message: format!(
            "{message_prefix}identity runtime binding for {} points at {}, but requested live member is {live_runtime_member_id}",
            identity.as_str(),
            registered_runtime_member_id.unwrap_or("<none>"),
        ),
        data: Some(serde_json::json!({
            "kind": "stale_identity_runtime_binding",
            "identity": identity.as_str(),
            "registered_runtime_member_id": registered_runtime_member_id,
            "live_runtime_member_id": live_runtime_member_id,
            "registered_session_id": status
                .and_then(|status| status.session_id.as_ref())
                .map(ToString::to_string),
            "live_session_id": live_session_id,
        })),
    }
}
