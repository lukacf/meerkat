//! The stock host's scoped destination for one of its own sessions.
//!
//! The stock host owns the session it created, so it supplies the exact
//! [`NativeWorkTarget`] its document serves and binds it at registration
//! ([`MeerkatMachine::bind_scoped_context`]). The target is never read back
//! from the machine, a claim or the wire: the same value is the original's
//! target, the association's target and the canonical MCP destination
//! ([`scoped_destination_target`]).

use std::sync::atomic::{AtomicU64, Ordering};

use meerkat_authorization_contracts::constraints::AudienceRef;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::NativeWorkTarget;
use meerkat_core::authorization::OwnerQualifiedTarget;
use meerkat_core::{PrincipalRef, SessionId};
use meerkat_runtime::meerkat_machine::scoped_destination_target;
use meerkat_runtime::{LogicalRuntimeId, MeerkatMachine, RuntimeDriverError};

/// The last generation this process bound. A bind proposes the larger of its
/// successor and the wall clock in milliseconds: best effort, so that a
/// replacement registration after a restart usually advances past the
/// generation an earlier process bound. It does not read the retained
/// generation, so a same-millisecond restart, an earlier allocation ahead of
/// wall time or a clock set back can propose a value that does not advance.
/// The machine's `generation_advances` guard is authoritative: such a bind
/// fails typed and the prompt is refused locally, never run ungoverned.
static LAST_GENERATION: AtomicU64 = AtomicU64::new(0);

fn next_generation() -> u64 {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    let mut last = LAST_GENERATION.load(Ordering::SeqCst);
    loop {
        let next = now_ms.max(last.saturating_add(1)).max(1);
        match LAST_GENERATION.compare_exchange(last, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return next,
            Err(current) => last = current,
        }
    }
}

fn invalid(reason: &str) -> RuntimeDriverError {
    RuntimeDriverError::ValidationFailed {
        reason: format!("stock session destination: {reason}"),
    }
}

/// One stock session's exact destination: owned by the operator, served by
/// the session's own runtime and context, disclosed to that session.
#[derive(Clone, PartialEq, Eq)]
pub struct StockSessionDestination {
    target: NativeWorkTarget,
}

impl std::fmt::Debug for StockSessionDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StockSessionDestination")
            .finish_non_exhaustive()
    }
}

impl StockSessionDestination {
    /// The destination of `session_id` under `operator` at `generation`.
    /// The scope (owner, runtime, context, audience) depends only on the
    /// operator and the session, so every generation names the same scope.
    pub fn for_session(
        operator: &PrincipalRef,
        session_id: &SessionId,
        generation: u64,
    ) -> Result<Self, RuntimeDriverError> {
        operator
            .validate_qualified()
            .map_err(|_| invalid("the operator is not a qualified principal"))?;
        if generation == 0 {
            return Err(invalid("a bound generation starts at 1"));
        }
        let runtime = EvidenceId::new(LogicalRuntimeId::for_session(session_id).to_string())
            .map_err(|_| invalid("the session runtime is not a bounded identifier"))?;
        let context = EvidenceId::new(session_id.to_string())
            .map_err(|_| invalid("the session id is not a bounded identifier"))?;
        Ok(Self {
            target: NativeWorkTarget {
                logical_owner: operator.clone(),
                logical_runtime: runtime,
                context,
                context_generation: generation,
                audience: AudienceRef::Destination {
                    authority: operator.clone(),
                    destination_id: session_id.to_string(),
                },
            },
        })
    }

    /// Whether `target` is a stock destination of `operator` in any
    /// generation: its owner and audience are the operator's and its context
    /// is the session its runtime serves. Currentness of the generation is the
    /// machine's binding, not this check.
    pub fn is_stock_target(operator: &PrincipalRef, target: &NativeWorkTarget) -> bool {
        Self::stock_session(operator, target).is_some()
    }

    /// The session `target` is a stock destination of under `operator`, in
    /// any generation ([`Self::is_stock_target`]), or `None`.
    pub fn stock_session(operator: &PrincipalRef, target: &NativeWorkTarget) -> Option<SessionId> {
        let session_id = SessionId::parse(target.context.as_str()).ok()?;
        Self::for_session(operator, &session_id, target.context_generation.max(1))
            .is_ok_and(|expected| {
                expected.target.logical_owner == target.logical_owner
                    && expected.target.logical_runtime == target.logical_runtime
                    && expected.target.context == target.context
                    && expected.target.audience == target.audience
                    && target.context_generation > 0
            })
            .then_some(session_id)
    }

    /// The exact target: the original's target, the association's target and
    /// the destination the session document is bound to.
    pub fn target(&self) -> &NativeWorkTarget {
        &self.target
    }

    /// The canonical MCP attachment destination of this target.
    pub fn mcp_destination(&self) -> OwnerQualifiedTarget {
        scoped_destination_target(&self.target)
    }
}

/// Bind the stock session `session_id` to its destination under `operator`,
/// at the next generation, on its exact current registration. Call it once the
/// session's executor is registered and before any governed input or MCP
/// entry, and again for every replacement registration (resume, restart): the
/// rebind keeps the same scope and advances the generation.
pub async fn bind_stock_session(
    machine: &MeerkatMachine,
    operator: &PrincipalRef,
    session_id: &SessionId,
) -> Result<StockSessionDestination, RuntimeDriverError> {
    bind_stock_session_within(machine, operator, session_id, |commit| {
        commit();
        Ok(())
    })
    .await
}

/// [`bind_stock_session`], committed only within the binder's final
/// decision `within`, which runs at the binding's commit with the session's
/// machine authority held ([`MeerkatMachine::bind_scoped_context_within`]):
/// unless it runs the commit, the session's binding is unchanged.
pub async fn bind_stock_session_within(
    machine: &MeerkatMachine,
    operator: &PrincipalRef,
    session_id: &SessionId,
    within: impl FnOnce(&mut dyn FnMut()) -> Result<(), RuntimeDriverError> + Send,
) -> Result<StockSessionDestination, RuntimeDriverError> {
    let registration = machine
        .current_session_registration_witness(session_id)
        .await
        .ok_or_else(|| invalid("the session has no current registration"))?;
    let destination =
        StockSessionDestination::for_session(operator, session_id, next_generation())?;
    machine
        .bind_scoped_context_within(&registration, destination.target(), within)
        .await?;
    Ok(destination)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::{PrincipalKind, TrustDomainId};

    fn operator(id: &str) -> PrincipalRef {
        PrincipalRef::in_domain(
            PrincipalKind::Human,
            id,
            TrustDomainId::new("stock-test").expect("domain"),
        )
        .expect("qualified operator")
    }

    #[test]
    fn every_generation_names_the_same_session_scope() {
        let session = SessionId::new();
        let first = StockSessionDestination::for_session(&operator("op"), &session, 1).unwrap();
        let later = StockSessionDestination::for_session(&operator("op"), &session, 9).unwrap();
        assert_eq!(first.target().context_generation, 1);
        assert_eq!(later.target().context_generation, 9);
        assert_eq!(first.target().logical_owner, later.target().logical_owner);
        assert_eq!(
            first.target().logical_runtime,
            later.target().logical_runtime
        );
        assert_eq!(first.target().context, later.target().context);
        assert_eq!(first.target().audience, later.target().audience);
        assert_eq!(
            first.target().logical_runtime.as_str(),
            LogicalRuntimeId::for_session(&session).to_string()
        );
        let mcp = later.mcp_destination();
        assert_eq!(mcp.authority, operator("op"));
        assert_eq!(&*mcp.namespace, "session");
        assert_eq!(&*mcp.id, session.to_string());
    }

    #[test]
    fn only_the_operators_own_session_target_is_a_stock_target() {
        let session = SessionId::new();
        let own = StockSessionDestination::for_session(&operator("op"), &session, 3).unwrap();
        assert!(StockSessionDestination::is_stock_target(
            &operator("op"),
            own.target()
        ));
        assert!(!StockSessionDestination::is_stock_target(
            &operator("other"),
            own.target()
        ));
        let mut moved = own.target().clone();
        moved.logical_runtime =
            EvidenceId::new(LogicalRuntimeId::for_session(&SessionId::new()).to_string()).unwrap();
        assert!(!StockSessionDestination::is_stock_target(
            &operator("op"),
            &moved
        ));
        let mut relabeled = own.target().clone();
        relabeled.audience = AudienceRef::Principal {
            principal: operator("op"),
        };
        assert!(!StockSessionDestination::is_stock_target(
            &operator("op"),
            &relabeled
        ));
        let mut unbound = own.target().clone();
        unbound.context_generation = 0;
        assert!(!StockSessionDestination::is_stock_target(
            &operator("op"),
            &unbound
        ));
    }

    #[test]
    fn generations_strictly_advance_and_never_start_at_zero() {
        let first = next_generation();
        let second = next_generation();
        assert!(first >= 1);
        assert!(second > first);
        assert!(
            StockSessionDestination::for_session(&operator("op"), &SessionId::new(), 0).is_err()
        );
    }
}
