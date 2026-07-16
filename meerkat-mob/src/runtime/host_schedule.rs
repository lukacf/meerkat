//! Schedule-target projection for the remote member host.
//!
//! The host actor remains the sole residency authority. This adapter only
//! resolves stable mob-member schedule identities through the actor's watch
//! projection; it never searches durable sessions or performs recovery.

use async_trait::async_trait;
use tokio::sync::watch;

use meerkat::surface::{
    NoopScheduleMobHost, SurfaceScheduleMobHost, parse_mob_member_schedule_identity,
};
use meerkat::{
    DeliveryDispatch, IdentityTargetBinding, MobTargetBinding, Occurrence, ScheduleDomainError,
    TargetProbeOutcome,
};
use meerkat_core::types::SessionId;

use super::host_observation::{HostObservationProjection, SessionObservationFacts};

enum ProjectedMobMember {
    Unrelated,
    Missing { mob_id: String, member: String },
    Ready(SessionId),
}

/// Schedule-mob adapter backed only by the host actor's residency projection.
///
/// Ordinary mob targets remain unsupported on a member host. Stable
/// `mob_member:` identities are recognized here so the shared schedule adapter
/// can deliver to the exact actor-published session through its ordinary
/// session-delivery path.
pub struct HostObservationScheduleMobHost {
    projection_rx: watch::Receiver<HostObservationProjection>,
    unsupported_mob_host: NoopScheduleMobHost,
}

impl HostObservationScheduleMobHost {
    pub fn new(
        projection_rx: watch::Receiver<HostObservationProjection>,
        unsupported_mob_detail: impl Into<String>,
    ) -> Self {
        Self {
            projection_rx,
            unsupported_mob_host: NoopScheduleMobHost::new(unsupported_mob_detail),
        }
    }

    fn project_identity(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<ProjectedMobMember, String> {
        let Some(identity) = parse_mob_member_schedule_identity(binding.identity()) else {
            return Ok(ProjectedMobMember::Unrelated);
        };

        let projection = self.projection_rx.borrow();
        let mut resolved = None;
        for (session_key, facts) in &projection.sessions {
            let facts_match =
                facts.mob_id == identity.mob_id && facts.agent_identity == identity.member;
            let incarnation_matches = facts.incarnation.mob_id == identity.mob_id
                && facts.incarnation.agent_identity == identity.member;
            if !facts_match && !incarnation_matches {
                continue;
            }

            validate_projection_row(session_key, facts).map_err(|detail| {
                format!(
                    "host observation projection for mob member '{}/{}' is corrupt: {detail}",
                    identity.mob_id, identity.member
                )
            })?;
            let session_id = SessionId::parse(session_key).map_err(|error| {
                format!(
                    "host observation projection for mob member '{}/{}' has invalid session id '{}': {error}",
                    identity.mob_id, identity.member, session_key
                )
            })?;
            if session_id.to_string() != session_key.as_str() {
                return Err(format!(
                    "host observation projection for mob member '{}/{}' has non-canonical session id '{}'",
                    identity.mob_id, identity.member, session_key
                ));
            }
            if let Some(previous) = resolved {
                return Err(format!(
                    "host observation projection contains duplicate resident sessions for mob member '{}/{}': '{}' and '{}'",
                    identity.mob_id, identity.member, previous, session_id
                ));
            }
            resolved = Some(session_id);
        }

        Ok(match resolved {
            Some(session_id) => ProjectedMobMember::Ready(session_id),
            None => ProjectedMobMember::Missing {
                mob_id: identity.mob_id,
                member: identity.member,
            },
        })
    }
}

fn validate_projection_row(
    session_key: &str,
    facts: &SessionObservationFacts,
) -> Result<(), String> {
    let incarnation = &facts.incarnation;
    if session_key != incarnation.member_session_id {
        return Err(format!(
            "session map key '{}' does not match incarnation session '{}'",
            session_key, incarnation.member_session_id
        ));
    }
    if facts.mob_id != incarnation.mob_id
        || facts.agent_identity != incarnation.agent_identity
        || facts.generation != incarnation.generation
        || facts.fence_token != incarnation.fence_token
    {
        return Err("row facts do not match the exact published incarnation".to_string());
    }
    Ok(())
}

#[async_trait]
impl SurfaceScheduleMobHost for HostObservationScheduleMobHost {
    async fn probe_mob_target(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        self.unsupported_mob_host.probe_mob_target(binding).await
    }

    async fn deliver_mob_target(
        &self,
        occurrence: &Occurrence,
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        self.unsupported_mob_host
            .deliver_mob_target(occurrence, binding)
            .await
    }

    async fn probe_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<TargetProbeOutcome>, ScheduleDomainError> {
        match self
            .project_identity(binding)
            .map_err(ScheduleDomainError::ProbeFailed)?
        {
            ProjectedMobMember::Unrelated => Ok(None),
            ProjectedMobMember::Ready(_) => Ok(Some(TargetProbeOutcome::Ready)),
            ProjectedMobMember::Missing { mob_id, member } => {
                Ok(Some(TargetProbeOutcome::Missing {
                    detail: Some(format!(
                        "mob member '{member}' in mob '{mob_id}' is not resident on this host"
                    )),
                }))
            }
        }
    }

    async fn resolve_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<SessionId>, ScheduleDomainError> {
        match self
            .project_identity(binding)
            .map_err(ScheduleDomainError::Internal)?
        {
            ProjectedMobMember::Unrelated => Ok(None),
            ProjectedMobMember::Ready(session_id) => Ok(Some(session_id)),
            ProjectedMobMember::Missing { mob_id, member } => {
                Err(ScheduleDomainError::Internal(format!(
                    "mob member '{member}' in mob '{mob_id}' is no longer resident on this host"
                )))
            }
        }
    }

    async fn deliver_identity_target(
        &self,
        _occurrence: &Occurrence,
        _binding: &IdentityTargetBinding,
    ) -> Result<Option<DeliveryDispatch>, ScheduleDomainError> {
        // Resolution above binds the exact session. The shared adapter then
        // uses the ordinary session host so there is only one delivery seam.
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use meerkat::surface::{SurfaceScheduleMobHost as _, mob_member_schedule_identity};
    use meerkat::{
        IdentityTargetBinding, MobTargetBinding, ScheduledMobAction, ScheduledSessionAction,
        TargetProbeOutcome,
    };
    use meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation;
    use meerkat_core::{ContentInput, MobMemberBinding, SessionId};
    use tokio::sync::watch;

    use super::{HostObservationProjection, HostObservationScheduleMobHost};
    use crate::runtime::host_observation::SessionObservationFacts;

    const UNSUPPORTED_DETAIL: &str = "ordinary mob targets are controller-host only";

    fn identity_binding(mob_id: &str, member: &str) -> IdentityTargetBinding {
        let identity = mob_member_schedule_identity(&MobMemberBinding {
            mob_id: mob_id.to_string(),
            role: "worker".to_string(),
            member: member.to_string(),
        });
        IdentityTargetBinding::resumable(
            identity,
            ScheduledSessionAction::Prompt {
                prompt: ContentInput::Text("scheduled work".to_string()),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        )
    }

    fn observation_row(
        session_id: &SessionId,
        mob_id: &str,
        member: &str,
    ) -> SessionObservationFacts {
        SessionObservationFacts {
            incarnation: BridgeMemberIncarnation {
                mob_id: mob_id.to_string(),
                agent_identity: member.to_string(),
                host_id: "host-1".to_string(),
                binding_generation: 7,
                member_session_id: session_id.to_string(),
                generation: 3,
                fence_token: 11,
            },
            mob_id: mob_id.to_string(),
            agent_identity: member.to_string(),
            generation: 3,
            fence_token: 11,
            generation_start_seq: 1,
            pending_turns: Vec::new(),
            turn_outcomes: Vec::new(),
        }
    }

    fn host(projection: HostObservationProjection) -> HostObservationScheduleMobHost {
        let (_projection_tx, projection_rx) = watch::channel(projection);
        HostObservationScheduleMobHost::new(projection_rx, UNSUPPORTED_DETAIL)
    }

    #[tokio::test]
    async fn resolves_exact_actor_published_session() {
        let session_id = SessionId::new();
        let adapter = host(HostObservationProjection {
            sessions: BTreeMap::from([(
                session_id.to_string(),
                observation_row(&session_id, "mob-1", "worker-1"),
            )]),
        });
        let binding = identity_binding("mob-1", "worker-1");

        assert!(matches!(
            adapter.probe_identity_target(&binding).await,
            Ok(Some(TargetProbeOutcome::Ready))
        ));
        assert_eq!(
            adapter
                .resolve_identity_target(&binding)
                .await
                .expect("projection resolution"),
            Some(session_id)
        );
    }

    #[tokio::test]
    async fn recognized_missing_member_is_typed_missing_and_never_delegated() {
        let adapter = host(HostObservationProjection::default());
        let binding = identity_binding("mob-1", "missing-worker");

        assert!(matches!(
            adapter.probe_identity_target(&binding).await,
            Ok(Some(TargetProbeOutcome::Missing { .. }))
        ));
        assert!(adapter.resolve_identity_target(&binding).await.is_err());
    }

    #[tokio::test]
    async fn unrelated_identity_is_left_to_the_shared_session_adapter() {
        let adapter = host(HostObservationProjection::default());
        let binding = IdentityTargetBinding::resumable(
            "external:user-1",
            ScheduledSessionAction::Event {
                event_type: "tick".to_string(),
                payload: serde_json::json!({}),
                render_metadata: None,
            },
        );

        assert!(matches!(
            adapter.probe_identity_target(&binding).await,
            Ok(None)
        ));
        assert_eq!(
            adapter
                .resolve_identity_target(&binding)
                .await
                .expect("unrelated identity delegation"),
            None
        );
    }

    #[tokio::test]
    async fn ordinary_mob_targets_preserve_noop_rejection() {
        let adapter = host(HostObservationProjection::default());
        let binding = MobTargetBinding::Member {
            mob_id: "mob-1".to_string(),
            member_id: "worker-1".to_string(),
            action: ScheduledMobAction::Send {
                content: ContentInput::Text("work".to_string()),
                render_metadata: None,
            },
        };

        let probe = adapter
            .probe_mob_target(&binding)
            .await
            .expect("noop mob probe");
        let TargetProbeOutcome::Missing { detail } = probe else {
            panic!("expected delegated no-op rejection, got {probe:?}");
        };
        assert_eq!(detail.as_deref(), Some(UNSUPPORTED_DETAIL));
    }

    #[tokio::test]
    async fn duplicate_or_inconsistent_matching_rows_fail_closed() {
        let first = SessionId::new();
        let second = SessionId::new();
        let duplicate = host(HostObservationProjection {
            sessions: BTreeMap::from([
                (
                    first.to_string(),
                    observation_row(&first, "mob-1", "worker-1"),
                ),
                (
                    second.to_string(),
                    observation_row(&second, "mob-1", "worker-1"),
                ),
            ]),
        });
        let binding = identity_binding("mob-1", "worker-1");
        assert!(duplicate.resolve_identity_target(&binding).await.is_err());

        let mut corrupt_row = observation_row(&first, "mob-1", "worker-1");
        corrupt_row.incarnation.member_session_id = second.to_string();
        let corrupt = host(HostObservationProjection {
            sessions: BTreeMap::from([(first.to_string(), corrupt_row)]),
        });
        assert!(corrupt.probe_identity_target(&binding).await.is_err());
    }
}
