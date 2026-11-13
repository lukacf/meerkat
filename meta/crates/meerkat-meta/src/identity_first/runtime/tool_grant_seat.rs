//! The seat a tool grant takes for an identity Core holds no member of.
//!
//! An identity grant applies at the member's builds: the member's gate and
//! its identity's grants are Core's. An identity the runtime registered but
//! Core holds no member of (its member was retired, or Core deferred its
//! seat) is declared again here as a door declares it: one partial declared
//! apply of the identity alone, under its lifecycle lock. Core's verdict
//! seats it Dormant, on its archived session or a fresh one, and builds
//! nothing; its first delivery builds it, and that build reads the grant.
//!
//! Unlike a delivery's door, a grant never creates a member: an identity Core
//! leaves to its host's first use (placed on a member host, or bound to an
//! external runtime) stays as it is, and the grant finds no member. Any
//! other answer that leaves no member is the door's typed refusal.

use std::sync::Arc;

use super::super::bridge::DeclaredIdentityAnswer;
use super::super::types::AgentIdentity;
use super::declare::DeclarationLocks;
use super::{IdentityRuntime, IdentityRuntimeError};

impl IdentityRuntime {
    /// Seat `identity` in Core's mob for a tool grant when Core holds no
    /// member of it: a door's one-identity declaration, under the identity's
    /// lifecycle lock, that builds and creates nothing. An identity Core
    /// holds takes no Core step. A seat settles the identity's views as a
    /// door's seat does, and reconciles its managed edges, bounded.
    ///
    /// # Errors
    /// [`IdentityRuntimeError::UnknownIdentity`] for an identity the runtime
    /// has not registered, and the door's typed refusal of an answer that
    /// leaves no member (a held role move, a held creation, a pending
    /// preparation, a deferral, a refusal, a failure).
    pub(crate) async fn seat_for_tool_grant(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), IdentityRuntimeError> {
        if self.core_live_member(identity).await?.is_some() {
            return Ok(());
        }
        let seated = {
            let lifecycle_lock = self.lifecycle_lock_for(identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            let spec = self
                .entries
                .read()
                .await
                .get(identity)
                .map(|entry| entry.spec.clone())
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
            let bridge = Arc::clone(self.core_bridge(identity)?);
            let mut declaration = self
                .declare_roster(
                    &bridge,
                    std::slice::from_ref(&spec),
                    false,
                    DeclarationLocks::Held,
                    false,
                    None,
                )
                .await;
            let answer = declaration.outcomes.remove(identity).unwrap_or_else(|| {
                Err(IdentityRuntimeError::DeclarationNotApplied {
                    retryable: false,
                    detail: format!("Core answered no declaration of {identity}"),
                })
            })?;
            match answer {
                answer if answer.bound_session().is_some() => true,
                DeclaredIdentityAnswer::Kept
                | DeclaredIdentityAnswer::Restamped { .. }
                | DeclaredIdentityAnswer::RoleMigrated { .. }
                | DeclaredIdentityAnswer::Noted { .. }
                | DeclaredIdentityAnswer::RecordedOnly
                | DeclaredIdentityAnswer::HostedOnFirstUse => false,
                answer => {
                    let history = self.core_view_history().await.ok().flatten();
                    return Err(self
                        .declared_answer_error(identity, answer, history.as_ref())
                        .unwrap_or_else(|| {
                            IdentityRuntimeError::Internal(format!(
                                "Core's answer for {identity} was mapped as a refusal"
                            ))
                        }));
                }
            }
        };
        if seated {
            self.settle_successful_wake(identity).await;
            if let Err(error) = self.reconcile_target_topology_for_wake(identity).await {
                tracing::warn!(
                    identity = %identity,
                    %error,
                    "the identity a grant seated had its managed edges left unreconciled; the \
                     next reconcile retries them"
                );
            }
        } else if self.core_live_member(identity).await?.is_none()
            && let Some(refusal) = self.unseated_refusal(identity).await
        {
            // The door seated no member, and Core's entry of the name says
            // why (archived, or awaiting its host's preparation, its held
            // creation or its seat): the grant finds no member, typed.
            return Err(refusal);
        }
        Ok(())
    }
}
