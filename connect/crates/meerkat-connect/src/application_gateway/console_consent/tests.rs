//! Helpers used only by the actual installed factory/SDK control. They invoke
//! the production event adapter with a real resolver and native-issued row;
//! they do not construct a human, candidate, scope or native authorization.
use super::*;
pub(crate) async fn evidence(
    route: &Arc<ConsoleConsentRoute>,
    headers: &HeaderMap,
    record: &ApprovalRecord,
) -> Result<Arc<dyn ConsentEligibility>, HostLifetimeError> {
    let (_, proof) = route
        .observe(
            headers,
            &Uri::from_static("/toolkit/consent/decide"),
            record,
        )
        .await?;
    Ok(proof)
}
pub(crate) fn no_retained_events(route: &ConsoleConsentRoute) -> bool {
    route
        .events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_empty()
}
pub(crate) fn issuer_gone(route: &ConsoleConsentRoute) -> bool {
    route.owner.strong_count() == 0
}
pub(crate) async fn poisoned_event_cleanup(
    route: &Arc<ConsoleConsentRoute>,
    headers: &HeaderMap,
    record: &ApprovalRecord,
) {
    let (approver, proof) = route
        .observe(
            headers,
            &Uri::from_static("/toolkit/consent/decide"),
            record,
        )
        .await
        .unwrap();
    let key = approver.authentication_scope().unwrap().as_str().to_owned();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _cleanup = EventGuard {
            route: route.clone(),
            key: key.clone(),
        };
        let mut events = route.events.lock().unwrap();
        events.insert(
            key,
            AcceptedEvent {
                approver,
                approval: record.clone(),
                proof: proof.clone(),
            },
        );
        panic!("controlled callback unwind after actual event observation");
    }));
    assert!(result.is_err());
    assert!(proof.check_current().is_err());
    assert!(no_retained_events(route));
}
pub(crate) async fn historical_target(
    route: &ConsoleConsentRoute,
    record: &ApprovalRecord,
) -> Result<AgentIdentity, HostLifetimeError> {
    route
        .binding
        .consent_session_owner(record.consent.as_ref().unwrap().scope.as_ref().unwrap())
        .await
}
