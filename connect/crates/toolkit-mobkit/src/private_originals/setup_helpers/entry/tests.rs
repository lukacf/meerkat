//! Reservation controls use the existing mechanical enrollment owner. They do
//! not fabricate a verified Human or prove native helper admission by themselves.
use super::*;
use crate::private_host::{ConsoleLoginRetirement, ConsoleLoginSelection, ConsoleLogins};
use std::{
    task::Poll,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn owners() -> (
    tokio::sync::watch::Sender<bool>,
    ConsoleLogins,
    [ConsoleLoginHandle; 2],
) {
    let (closed, source) = ConsoleLogins::fixture_owner();
    let expires_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    let handles = [1, 2].map(|id| {
        source
            .fixture_enroll(
                id,
                ConsoleLoginSelection {
                    issuer: "fixture-issuer".into(),
                    subject: "same-human".into(),
                    login_scope_id: format!("scope-{id}"),
                    expires_at_ms,
                },
            )
            .unwrap()
    });
    (closed, source, handles)
}

#[test]
fn local_operator_without_contributing_login_keeps_unfenced_path() {
    assert!(for_logins(vec![]).is_none());
}

#[tokio::test]
async fn retired_later_contributor_refuses_and_releases_prior_reservation() {
    let (_closed, source, handles) = owners();
    let fence = for_logins(handles.to_vec()).unwrap();
    let retired = source.fixture_retire(2).unwrap().await.unwrap();
    assert_eq!(
        retired,
        ConsoleLoginRetirement {
            fenced: true,
            reservations_drained: true,
            uncertain: false
        }
    );
    // Capture preceded retirement. Final enter must consult the same owner,
    // and a refused second login must release the first as known unentered.
    assert!(matches!(fence.enter(), Err(GovernedEntryRefusal::Retired)));
    let first = tokio::time::timeout(Duration::from_secs(2), source.fixture_retire(1).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first,
        ConsoleLoginRetirement {
            fenced: true,
            reservations_drained: true,
            uncertain: false
        }
    );
}

#[tokio::test]
async fn winning_reservations_join_native_decision_for_every_contributor() {
    for outcome in [
        GovernedEntryOutcome::Entered,
        GovernedEntryOutcome::Unentered,
    ] {
        let (_closed, source, handles) = owners();
        let fence = for_logins(handles.to_vec()).unwrap();
        let permit = fence.enter().unwrap();
        let mut first = source.fixture_retire(1).unwrap();
        let mut second = source.fixture_retire(2).unwrap();
        assert!(matches!(futures::poll!(first.as_mut()), Poll::Pending));
        assert!(matches!(futures::poll!(second.as_mut()), Poll::Pending));
        assert!(matches!(fence.enter(), Err(GovernedEntryRefusal::Retired)));
        permit.finish(outcome);
        for wait in [first, second] {
            let observed = tokio::time::timeout(Duration::from_secs(2), wait)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                observed,
                ConsoleLoginRetirement {
                    fenced: true,
                    reservations_drained: true,
                    uncertain: false
                }
            );
        }
    }
}

#[tokio::test]
async fn unknown_or_dropped_native_decision_drains_with_uncertainty() {
    for dropped in [false, true] {
        let (_closed, source, handles) = owners();
        let permit = for_logins(handles.to_vec()).unwrap().enter().unwrap();
        let mut first = source.fixture_retire(1).unwrap();
        let mut second = source.fixture_retire(2).unwrap();
        assert!(matches!(futures::poll!(first.as_mut()), Poll::Pending));
        assert!(matches!(futures::poll!(second.as_mut()), Poll::Pending));
        if dropped {
            drop(permit);
        } else {
            permit.finish(GovernedEntryOutcome::Uncertain);
        }
        for wait in [first, second] {
            let observed = tokio::time::timeout(Duration::from_secs(2), wait)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                observed,
                ConsoleLoginRetirement {
                    fenced: true,
                    reservations_drained: true,
                    uncertain: true
                }
            );
        }
    }
}

#[tokio::test]
async fn pipe_loss_refuses_without_claiming_known_retirement() {
    let (closed, _source, handles) = owners();
    let fence = for_logins(handles.to_vec()).unwrap();
    closed.send_replace(true);
    assert!(matches!(
        fence.enter(),
        Err(GovernedEntryRefusal::Unavailable(_))
    ));
}
