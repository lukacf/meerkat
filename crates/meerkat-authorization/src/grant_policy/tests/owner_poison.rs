//! A poisoned owner publication refuses prepared and exclusive operation
//! entry, while an operation that already entered keeps its outcome.

use super::*;
use crate::work::{FurtherOriginals, OriginalBasisSource};
use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
use meerkat_core::OperationAuthorizationError;

/// Every original is active and none is further retained: the work's own
/// association is its only basis.
struct ActiveOriginals;
impl OriginalBasisSource for ActiveOriginals {
    fn require_active(
        &self,
        _namespace: &QualifiedIngressNamespace,
        _original: &OriginalWorkRef,
    ) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }

    fn active_retained_basis(
        &self,
        _namespace: &QualifiedIngressNamespace,
        _original: &OriginalWorkRef,
    ) -> Result<Option<WorkAuthorityBasis>, OperationAuthorizationError> {
        Ok(None)
    }
}

fn no_further_originals() -> FurtherOriginals {
    FurtherOriginals {
        originals: Vec::new().into(),
        source: Arc::new(ActiveOriginals),
    }
}

/// A host owner sharing `publication` panics inside its own entry lock while
/// holding the sentinel, and catches its own panic.
#[allow(clippy::panic)] // Deliberately poison the host owner's own lock.
fn caught_host_poison(publication: &LocalAuthorizationPublication, host: &Mutex<()>) {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _entry = host.lock().expect("host entry");
        let _sentinel = publication.poison_on_unwind();
        panic!("simulated host owner panic");
    }));
    assert!(caught.is_err());
    assert!(host.is_poisoned());
}

#[test]
fn a_prepared_check_does_not_enter_after_its_owner_is_poisoned() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let context = fixture.context(&policy, &association);
    let binding = source(SourceAuthorizationUse::Read, "public");
    let prepared = context
        .authorization()
        .prepare(&binding)
        .expect("prepared before the poison");
    assert!(prepared.check_current(&binding).is_ok());

    fixture.publication.poison();
    assert!(matches!(
        prepared.check_current(&binding),
        Err(OperationAuthorizationError::Unavailable)
    ));
    assert!(matches!(
        context.authorization().prepare(&binding),
        Err(OperationAuthorizationError::Unavailable)
    ));
}

#[test]
fn a_poison_caught_inside_an_exclusive_operation_keeps_its_outcome_and_stops_the_next_entry() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let context = fixture.context(&policy, &association);
    let binding = source(SourceAuthorizationUse::Read, "public");
    let prepared = context
        .authorization()
        .prepare(&binding)
        .expect("prepared before the poison");
    let validity = prepared.validity();
    let further = no_further_originals();
    let host = Mutex::new(());

    let mut completed = false;
    let entered = context.authorization().run_if_current_within(
        &binding,
        validity.as_ref(),
        &further,
        &mut || {
            caught_host_poison(&fixture.publication, &host);
            completed = true;
        },
    );
    assert!(
        entered.is_ok(),
        "the entered operation keeps its outcome: {entered:?}"
    );
    assert!(completed, "the entered operation ran to completion");
    assert!(matches!(
        prepared.check_current(&binding),
        Err(OperationAuthorizationError::Unavailable)
    ));

    let mut ran = false;
    let next = context.authorization().run_if_current_within(
        &binding,
        validity.as_ref(),
        &further,
        &mut || ran = true,
    );
    assert!(
        matches!(next, Err(OperationAuthorizationError::Unavailable)),
        "{next:?}"
    );
    assert!(!ran, "no operation enters after the poison");
}
