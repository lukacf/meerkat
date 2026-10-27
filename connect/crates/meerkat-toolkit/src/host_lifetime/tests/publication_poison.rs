//! Actual host callback custody composed with the selected native publication.
//! These controls use no reconstructed authorization receipt or private API.

use super::*;
use meerkat_authorization::{
    clock::HostAuthorizationClock,
    grants::{LocalGrantAuthority, LocalGrantConfiguration},
    publication::PublicationError,
};
use std::panic::{AssertUnwindSafe, catch_unwind};

fn bound_fixture() -> (Fixture, LocalAuthorizationPublication) {
    let mut fixture = Fixture::new();
    let publication = LocalAuthorizationPublication::new();
    Arc::get_mut(&mut fixture.owner)
        .unwrap()
        .bind_native_authorization_publication(publication.clone())
        .unwrap();
    fixture.open();
    (fixture, publication)
}

#[derive(Clone, Copy, Debug)]
enum CallbackEntry {
    Setup,
    Administration,
    GatewayReady,
    Component,
}

fn enter(fixture: &Fixture, entry: CallbackEntry, panic_inside: bool) -> Result<()> {
    let mut operation = || -> Result<()> {
        if panic_inside {
            panic!("bounded host callback poison");
        }
        Ok(())
    };
    match entry {
        CallbackEntry::Setup => fixture.owner.with_current_setup(operation),
        CallbackEntry::Administration => fixture.owner.with_native_administration_entry(operation)?,
        CallbackEntry::GatewayReady => fixture.owner.publish_gateway_ready(&mut operation),
        CallbackEntry::Component => fixture.owner.with_component_open(
            &Id::new("slack-extension").unwrap(),
            None,
            None,
            operation,
        )?,
    }
}

#[test]
fn callback_entry_variants_poison_retained_native_stamps() {
    for entry in [
        CallbackEntry::Setup,
        CallbackEntry::Administration,
        CallbackEntry::GatewayReady,
        CallbackEntry::Component,
    ] {
        let (fixture, publication) = bound_fixture();
        let component = Id::new("slack-extension").unwrap();
        let (observed, stamp) = publication
            .observe(|| fixture.owner.validate_native_entry(Some(&component)))
            .unwrap();
        observed.unwrap();
        // Normal sentinel disposal is inert; a healthy callback is not a
        // publication mutation and must not invalidate this real observation.
        enter(&fixture, entry, false).unwrap();
        assert_eq!(stamp.check_current(), Ok(()), "{entry:?}");

        let caught = catch_unwind(AssertUnwindSafe(|| enter(&fixture, entry, true)));
        // The catch is outside the actual callback entry/sentinel scope.
        assert!(caught.is_err(), "{entry:?}");
        assert!(fixture.owner.activation.entry.is_poisoned(), "{entry:?}");
        assert!(!fixture.owner.activation.machine.is_poisoned());
        assert_eq!(stamp.check_current(), Err(PublicationError::Unavailable));
        assert!(matches!(
            publication.observe(|| fixture.owner.validate_native_entry(Some(&component))),
            Err(PublicationError::Unavailable)
        ));
        assert_eq!(
            fixture.owner.validate_native_entry(None),
            Err(HostLifetimeError::Unavailable)
        );
        assert_eq!(
            fixture.owner.current_setup_policy(),
            Err(HostLifetimeError::Unavailable)
        );
        assert_eq!(enter(&fixture, entry, false), Err(HostLifetimeError::Unavailable));
        if matches!(entry, CallbackEntry::Component) {
            assert!(fixture.owner.component(&component).unwrap().entry.is_poisoned());
        }
        // Poison cannot manufacture an actual close or settlement receipt.
        assert!(matches!(
            fixture.owner.begin_close_activation(HostCloseReason::Shutdown),
            Err(HostLifetimeError::Unavailable)
        ));
    }
}

#[test]
fn callback_panic_before_outer_observation_finishes_refuses_its_stamp() {
    let (fixture, publication) = bound_fixture();
    let mut phase = None;
    let mut caught = None;
    let observed = publication.observe(|| {
        // The pure host observation has succeeded and released its finite
        // phase read, but the enclosing native observation has not completed.
        phase = Some(fixture.owner.validate_native_entry(None));
        caught = Some(catch_unwind(AssertUnwindSafe(|| {
            enter(&fixture, CallbackEntry::Setup, true)
        })));
        "copied phase facts"
    });
    assert_eq!(phase, Some(Ok(())));
    assert!(caught.unwrap().is_err());
    assert!(matches!(observed, Err(PublicationError::Unavailable)));
    assert!(fixture.owner.activation.entry.is_poisoned());
    assert!(!fixture.owner.activation.machine.is_poisoned());
}

#[test]
fn caught_callback_panic_under_native_reservation_keeps_poison_after_return() {
    let (fixture, publication) = bound_fixture();
    let grants = LocalGrantAuthority::new(
        LocalGrantConfiguration {
            root: principal(PrincipalKind::ServiceAccount, "host-grant-owner"),
            namespace: id("host-poison-native-reservation"),
            generation: 1,
        },
        publication.clone(),
        Arc::new(HostAuthorizationClock),
    )
    .unwrap();
    let (observed, stamp) = publication
        .observe(|| fixture.owner.validate_native_entry(None))
        .unwrap();
    observed.unwrap();
    let mut reserved = false;
    let mut caught = None;
    let returned = grants.try_with_owner_changes_excluded(|| {
        // This is the actual native unpublished reservation, not a synthetic
        // mutex or a published writer whose own drop would already poison it.
        reserved = matches!(
            publication.try_begin_owner_change(),
            Err(PublicationError::Unavailable)
        ) && stamp.check_current().is_ok();
        caught = Some(catch_unwind(AssertUnwindSafe(|| {
            enter(&fixture, CallbackEntry::Administration, true)
        })));
        "already entered owner callback completed"
    });
    assert!(reserved);
    assert!(caught.unwrap().is_err());
    // The already-entered native operation keeps its return. Poison affects
    // subsequent native observations/entries, not this prior completion.
    assert_eq!(returned, Ok("already entered owner callback completed"));
    assert_eq!(stamp.check_current(), Err(PublicationError::Unavailable));
    assert!(fixture.owner.activation.entry.is_poisoned());
    let mut entered_again = false;
    assert_eq!(
        grants.try_with_owner_changes_excluded(|| entered_again = true),
        Err(PublicationError::Unavailable)
    );
    assert!(!entered_again);
    assert!(matches!(
        publication.begin_owner_change(),
        Err(PublicationError::Unavailable)
    ));
    assert!(matches!(publication.observe(|| ()), Err(PublicationError::Unavailable)));
}
