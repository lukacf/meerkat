//! Test fixture: Core's declared apply (`MobHandle::apply_declared_roster`)
//! as an integration-test double of `SessionBridge` emulates it, through the
//! double's own steps ([`apply_through_seats`]), so the double answers a
//! registration, a roster pass and a door's declaration as Core does: it
//! seats, keeps, rotates or moves each identity, and a declaration of an
//! identity's facts only asks for its build where Core would.
//!
//! The crate's own unit doubles share the same emulation
//! (`identity_first::core_test_bridge::apply_through_seats`); this is its
//! port onto the public API, for the test crates.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};

use meerkat_core::types::SessionId;
use meerkat_meta::identity_first::bridge::{DormantSeatRequest, DormantSeatResult};
use meerkat_meta::identity_first::{
    AgentIdentity, BridgeError, DeclaredIdentityAnswer, DeclaredIdentityBuild,
    DeclaredRosterAnswer, DeclaredRosterRequest, DurableAgentSpec, SessionBridge,
    UnlistedIdentityAnswer,
};

/// A double's seat of an identity, Dormant, onto the archived session it
/// names: it builds nothing.
pub type ArchivedSeat<'a> = dyn Fn(&AgentIdentity, &DurableAgentSpec, &SessionId) -> Result<(), BridgeError>
    + Send
    + Sync
    + 'a;

/// Whether a host's identity intent row names `identity` (readable or not):
/// Core's declared apply refuses it `HostAdopted` and its seal leaves it.
async fn intent_named(bridge: &dyn SessionBridge, identity: &AgentIdentity) -> bool {
    matches!(
        bridge.identity_management(identity).await,
        Ok(meerkat_mob::MemberIdentityManagement::IntentManaged
            | meerkat_mob::MemberIdentityManagement::IntentUnreadable)
    )
}

/// The identities a double's Core holds declared: each one a declared apply
/// declared, until a complete roster that does not list it unlists it
/// ([`Self::apply`]).
#[derive(Default)]
pub struct DeclaredSet(Mutex<BTreeSet<AgentIdentity>>);

impl DeclaredSet {
    /// Hold `roster`'s identities declared and, for a complete roster,
    /// unlist each identity held declared that it does not list, as Core's
    /// seal does: `bridge` retires its live member
    /// ([`SessionBridge::retire_member`]; a failed retirement answers
    /// `Failed` and keeps the identity declared, so the next complete roster
    /// retires it again), or the declaration is removed when no member holds
    /// it. An identity a host's intent row names is left as it is.
    pub async fn apply(
        &self,
        bridge: &dyn SessionBridge,
        roster: &DeclaredRosterRequest,
    ) -> Result<BTreeMap<AgentIdentity, UnlistedIdentityAnswer>, BridgeError> {
        let listed: BTreeSet<AgentIdentity> = roster
            .identities
            .iter()
            .map(|declared| declared.spec.identity.clone())
            .collect();
        let candidates: Vec<AgentIdentity> = {
            let mut declared = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            declared.extend(listed.iter().cloned());
            if roster.complete {
                declared.difference(&listed).cloned().collect()
            } else {
                Vec::new()
            }
        };
        let mut ended = BTreeMap::new();
        for identity in candidates {
            if intent_named(bridge, &identity).await {
                continue;
            }
            let outcome = match bridge.live_member(&identity).await? {
                Some(live) => match bridge.retire_member(&live.agent_runtime_id).await {
                    Ok(()) => UnlistedIdentityAnswer::Retired,
                    Err(error) => UnlistedIdentityAnswer::Failed { error },
                },
                None => UnlistedIdentityAnswer::Removed,
            };
            ended.insert(identity, outcome);
        }
        let mut declared = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for (identity, outcome) in &ended {
            if !matches!(outcome, UnlistedIdentityAnswer::Failed { .. }) {
                declared.remove(identity);
            }
        }
        Ok(ended)
    }
}

/// Core's declared apply through the double's own steps:
/// - an identity a host's identity intent row names is refused
///   `HostAdopted`: Core's declared apply does not drive it;
/// - an identity whose member Core holds keeps it, and a first-boot `Resume`
///   request of the session the member is bound to is carried out with it; a
///   member the roster moves to another role is rotated onto a fresh session
///   when its sessions live in memory only
///   ([`SessionBridge::session_outlives_member`] answers `false`), is
///   restamped onto its own session when the activation declared that
///   migration ([`SessionBridge::resume_session`]), and is held under its
///   previous role otherwise, its materialization released;
/// - a first-boot `Fresh` request starts the identity's fresh life
///   ([`SessionBridge::rotate_member`]);
/// - an identity Core holds no member of awaits its held creation when its
///   host holds it, is hosted on its first use when it is placed or bound to
///   an external runtime, and is otherwise seated Dormant, all of them in one
///   seat call ([`SessionBridge::seat_dormant_members`]): on its first-boot
///   request's session, else on its last binding's session, else on a fresh
///   one. A seat the double answers archived is revived as Core's declared
///   apply revives it, Dormant on that session with nothing built (`revive`,
///   the double's own seat onto an archived session); a double with no
///   revival answers it `Failed`;
/// - a declaration of facts only whose verdict seats or starts a member
///   answers `BuildRequired` and takes no step.
///
/// Unlike Core, it answers `Kept` for any member Core holds live under the
/// spec's role, with none of Core's mark, label or custody checks: only the
/// rows over a real Core mob pin the legacy adoption.
///
/// Nothing is unlisted ([`DeclaredSet::apply`] does that). Each identity's
/// answer is its own: a refused seat, rotation or revival is that identity's
/// `Failed`.
pub async fn apply_through_seats(
    bridge: &dyn SessionBridge,
    roster: &DeclaredRosterRequest,
    revive: Option<&ArchivedSeat<'_>>,
) -> Result<DeclaredRosterAnswer, BridgeError> {
    let mut answer = DeclaredRosterAnswer::default();
    let mut seats = Vec::new();
    for declared in &roster.identities {
        let spec = &declared.spec;
        let identity = &spec.identity;
        let request = roster.first_boot_requests.get(identity);
        let draft = match &declared.build {
            DeclaredIdentityBuild::Composed(draft) => Some(draft),
            _ => None,
        };
        let live = bridge.live_member(identity).await?;
        // A host's intent row names the identity: Core's declared apply does
        // not drive it.
        if intent_named(bridge, identity).await {
            answer.identities.insert(
                identity.clone(),
                DeclaredIdentityAnswer::Refused {
                    refusal: meerkat_mob::DeclaredEntryRefusal::HostAdopted,
                },
            );
            continue;
        }
        let outcome = match (live, request) {
            (_, Some(meerkat_mob::FirstBootRequest::Fresh)) => match draft {
                None => DeclaredIdentityAnswer::BuildRequired,
                Some(draft) => match bridge.rotate_member(identity, spec, draft).await {
                    Ok(session_id) => {
                        answer
                            .first_boot_requests_carried_out
                            .insert(identity.clone());
                        DeclaredIdentityAnswer::FreshLife { session_id }
                    }
                    Err(error) => DeclaredIdentityAnswer::Failed { error },
                },
            },
            (Some(live), request) => {
                let role = bridge
                    .last_member_binding(identity)
                    .await
                    .ok()
                    .flatten()
                    .map(|binding| binding.role().clone());
                match role {
                    Some(role) if role != spec.profile => {
                        let memory_only = match live.session_id.as_ref() {
                            Some(session) => {
                                matches!(bridge.session_outlives_member(session).await, Ok(false))
                            }
                            None => false,
                        };
                        if memory_only {
                            match draft {
                                None => DeclaredIdentityAnswer::BuildRequired,
                                Some(draft) => {
                                    match bridge.rotate_member(identity, spec, draft).await {
                                        Ok(session_id) => {
                                            DeclaredIdentityAnswer::RoleRotated { session_id }
                                        }
                                        Err(error) => DeclaredIdentityAnswer::Failed { error },
                                    }
                                }
                            }
                        } else if bridge.declared_role_migration(identity).as_ref() != Some(&role) {
                            let _ = bridge.release_member(identity).await;
                            DeclaredIdentityAnswer::RoleMigrationHeld {
                                from: role,
                                to: spec.profile.clone(),
                            }
                        } else {
                            match (draft, live.session_id.as_ref()) {
                                (None, _) => DeclaredIdentityAnswer::BuildRequired,
                                (Some(_), None) => DeclaredIdentityAnswer::Failed {
                                    error: BridgeError::Mob(format!(
                                        "{identity}'s member is bound to no session to migrate on"
                                    )),
                                },
                                (Some(draft), Some(session)) => match bridge
                                    .resume_session(identity, spec, draft, session, Some(&role))
                                    .await
                                {
                                    Ok(_) => {
                                        let _ = bridge.release_member(identity).await;
                                        let generation = bridge
                                            .last_member_binding(identity)
                                            .await
                                            .ok()
                                            .flatten()
                                            .map_or(0, |binding| binding.generation().get());
                                        DeclaredIdentityAnswer::RoleMigrated { generation }
                                    }
                                    Err(error) => DeclaredIdentityAnswer::Failed { error },
                                },
                            }
                        }
                    }
                    _ => {
                        if let Some(meerkat_mob::FirstBootRequest::Resume { session_id }) = request
                            && live.session_id.as_ref() == Some(session_id)
                        {
                            answer
                                .first_boot_requests_carried_out
                                .insert(identity.clone());
                        }
                        DeclaredIdentityAnswer::Kept
                    }
                }
            }
            (None, _) if declared.held_creation => DeclaredIdentityAnswer::AwaitingHeldCreation,
            (None, _) if spec.placement.is_some() || spec.binding.is_some() => {
                DeclaredIdentityAnswer::HostedOnFirstUse
            }
            (None, request) => match draft {
                None => DeclaredIdentityAnswer::BuildRequired,
                Some(draft) => {
                    let session = match request {
                        Some(meerkat_mob::FirstBootRequest::Resume { session_id }) => {
                            Some(session_id.clone())
                        }
                        _ => bridge
                            .last_member_binding(identity)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|binding| binding.session_id().cloned()),
                    };
                    seats.push(DormantSeatRequest {
                        identity: identity.clone(),
                        spec: spec.clone(),
                        draft: draft.clone(),
                        session,
                        resume_from_role: None,
                    });
                    continue;
                }
            },
        };
        answer.identities.insert(identity.clone(), outcome);
    }
    if seats.is_empty() {
        return Ok(answer);
    }
    let requested: std::collections::BTreeMap<AgentIdentity, DormantSeatRequest> = seats
        .iter()
        .map(|request| (request.identity.clone(), request.clone()))
        .collect();
    for (identity, result) in bridge.seat_dormant_members(seats).await? {
        let Some(request) = requested.get(&identity) else {
            continue;
        };
        let first_boot = matches!(
            roster.first_boot_requests.get(&identity),
            Some(meerkat_mob::FirstBootRequest::Resume { .. })
        );
        let outcome = match result {
            DormantSeatResult::Seated { session_id } => DeclaredIdentityAnswer::Seated {
                session_id,
                history_unavailable: None,
            },
            DormantSeatResult::AlreadySeated { .. } => DeclaredIdentityAnswer::Kept,
            DormantSeatResult::SessionArchived { session_id } => {
                match revive.map(|revive| revive(&identity, &request.spec, &session_id)) {
                    Some(Ok(())) => DeclaredIdentityAnswer::Seated {
                        session_id,
                        history_unavailable: None,
                    },
                    Some(Err(error)) => DeclaredIdentityAnswer::Failed { error },
                    None => DeclaredIdentityAnswer::Failed {
                        error: BridgeError::Unsupported {
                            operation: "revive an archived session",
                        },
                    },
                }
            }
            DormantSeatResult::Embodied { session_id } => DeclaredIdentityAnswer::Embodied {
                session_id,
                history_unavailable: None,
            },
            DormantSeatResult::Refused(error) => DeclaredIdentityAnswer::Failed { error },
            _ => DeclaredIdentityAnswer::Failed {
                error: BridgeError::Mob(format!(
                    "{identity}: a seat answer this double does not know"
                )),
            },
        };
        if first_boot && outcome.bound_session().is_some() {
            answer
                .first_boot_requests_carried_out
                .insert(identity.clone());
        }
        answer.identities.insert(identity, outcome);
    }
    Ok(answer)
}
