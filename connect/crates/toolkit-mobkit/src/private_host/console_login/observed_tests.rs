//! Real mechanical login-owner controls. These do not construct a Console
//! human, approve a native request, or establish Meta authentication currentness.

use super::*;

fn owner() -> (
    super::super::PrivateHostClosePublisher,
    ConsoleLogins,
    LocalAuthorizationPublication,
) {
    let (send, receive) = watch::channel(false);
    let publication = LocalAuthorizationPublication::new();
    (
        super::super::PrivateHostClosePublisher {
            closed: send,
            publication: Some(publication.clone()),
        },
        ConsoleLogins::new(PrivateHostClosed(receive), Some(publication.clone())),
        publication,
    )
}

fn selection(scope: &str, expires_at_ms: u64) -> ConsoleLoginSelection {
    ConsoleLoginSelection {
        issuer: "https://mechanical.example".into(),
        subject: "retained-subject".into(),
        login_scope_id: scope.into(),
        expires_at_ms,
    }
}

#[test]
fn observed_login_requires_its_exact_owner_and_preserves_ordinary_entry() {
    let (_pipe, logins, _) = owner();
    let expires = now_ms().unwrap() + 60_000;
    let a = logins.fixture_enroll(1, selection("a", expires)).unwrap();
    let b = logins.fixture_enroll(2, selection("b", expires)).unwrap();
    let (_foreign_pipe, foreign, _) = owner();
    let equal_labels = foreign.fixture_enroll(1, selection("a", expires)).unwrap();
    assert!(!a.same_owner(&equal_labels));
    assert!(matches!(
        foreign.observe_login(&a),
        Err(ConsoleLoginRefusal::Unknown)
    ));
    assert!(matches!(
        logins.observe_login(&equal_labels),
        Err(ConsoleLoginRefusal::Unknown)
    ));
    let observation = logins.clone().observe_login(&a).unwrap();
    assert_eq!(observation.check_current(&a.clone()), Ok(()));
    assert_eq!(
        observation.check_current(&b),
        Err(ConsoleLoginRefusal::Unknown)
    );
    assert_eq!(
        observation.check_current(&equal_labels),
        Err(ConsoleLoginRefusal::Unknown)
    );
    // Observation does not enter or reserve. The final check uses neither lock.
    {
        let _rows = logins.rows.lock().unwrap();
        let state = a.0.state.lock().unwrap();
        assert_eq!(state.reservations, 0);
        assert_eq!(observation.check_current(&a), Ok(()));
    }
    GovernedEntryFence::enter(&a)
        .unwrap()
        .finish(GovernedEntryOutcome::Unentered);
    assert_eq!(a.0.state.lock().unwrap().reservations, 0);

    let (_legacy_pipe, legacy) = ConsoleLogins::fixture_owner();
    let legacy_login = legacy.fixture_enroll(1, selection("a", expires)).unwrap();
    assert!(matches!(
        legacy.observe_login(&legacy_login),
        Err(ConsoleLoginRefusal::Unavailable)
    ));
    // No joined publication means no new proof, but ordinary behavior remains.
    assert_eq!(legacy_login.check(), Ok(()));
    GovernedEntryFence::enter(&legacy_login)
        .unwrap()
        .finish(GovernedEntryOutcome::Unentered);
}

#[tokio::test]
async fn observed_login_refuses_real_retirement_and_synchronous_pipe_close() {
    let (pipe, logins, _) = owner();
    let expires = now_ms().unwrap() + 60_000;
    let a = logins.fixture_enroll(1, selection("a", expires)).unwrap();
    let b = logins.fixture_enroll(2, selection("b", expires)).unwrap();
    let observation = logins.observe_login(&a).unwrap();
    let reservation = a.reserve().unwrap();
    let waiting = logins.retire(1).unwrap();
    // Retirement invalidates before the outstanding real entry has drained.
    assert_eq!(
        observation.check_current(&a),
        Err(ConsoleLoginRefusal::Unavailable)
    );
    assert!(matches!(
        logins.observe_login(&a),
        Err(ConsoleLoginRefusal::Retired)
    ));
    assert!(matches!(
        GovernedEntryFence::enter(&a),
        Err(GovernedEntryRefusal::Retired)
    ));
    reservation.settled();
    let retired = waiting.drain().await.unwrap();
    assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);

    let observation = logins.observe_login(&b).unwrap();
    assert_eq!(observation.check_current(&b), Ok(()));
    pipe.close().unwrap();
    assert_eq!(
        observation.check_current(&b),
        Err(ConsoleLoginRefusal::Unavailable)
    );
    assert!(matches!(
        logins.observe_login(&b),
        Err(ConsoleLoginRefusal::Unavailable)
    ));
    assert!(matches!(
        GovernedEntryFence::enter(&b),
        Err(GovernedEntryRefusal::Unavailable(_))
    ));
}

#[tokio::test]
async fn observed_login_refuses_retirement_writer_in_progress_without_waiting() {
    let (_pipe, logins, publication) = owner();
    let login = logins
        .fixture_enroll(1, selection("a", now_ms().unwrap() + 60_000))
        .unwrap();
    let observation = logins.observe_login(&login).unwrap();
    let (_, before) = publication.observe(|| ()).unwrap();
    let held_state = login.0.state.lock().unwrap();
    assert!(matches!(
        logins.observe_login(&login),
        Err(ConsoleLoginRefusal::Unavailable)
    ));
    let retiring = logins.clone();
    let worker = std::thread::spawn(move || retiring.retire(1));
    // The actual retirement writer publishes before waiting for this state
    // guard. Observe that publication, not an elapsed-time claim of contention.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while before.check_current().is_ok() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    let writer_published = before.check_current().is_err();
    let during = logins.observe_login(&login);
    let held_proof = observation.check_current(&login);
    drop(held_state);
    let waiting = worker.join().unwrap().unwrap();
    waiting.drain().await.unwrap();
    assert!(
        writer_published,
        "actual retirement reached publication before release"
    );
    assert!(matches!(during, Err(ConsoleLoginRefusal::Unavailable)));
    assert_eq!(held_proof, Err(ConsoleLoginRefusal::Unavailable));
    assert_eq!(
        observation.check_current(&login),
        Err(ConsoleLoginRefusal::Unavailable)
    );
    assert!(matches!(
        logins.observe_login(&login),
        Err(ConsoleLoginRefusal::Retired)
    ));
}

#[test]
fn observed_login_checks_exact_deadline_and_unavailable_time_without_publication() {
    let (_pipe, logins, publication) = owner();
    let expires = now_ms().unwrap() + 60_000;
    let login = logins.fixture_enroll(1, selection("a", expires)).unwrap();
    let observation = logins.observe_login(&login).unwrap();
    let (_, unchanged) = publication.observe(|| ()).unwrap();
    assert_eq!(observation.check_current(&login), Ok(()));
    // The private time argument exercises the very same final check, with no
    // test clock callback, mutable expiry, timer or native authentication claim.
    let before = UNIX_EPOCH + Duration::from_millis(expires - 1);
    let exact = UNIX_EPOCH + Duration::from_millis(expires);
    let after = UNIX_EPOCH + Duration::from_millis(expires + 1);
    assert_eq!(observation.check_current_at(&login, before), Ok(()));
    assert_eq!(
        observation.check_current_at(&login, exact),
        Err(ConsoleLoginRefusal::Expired)
    );
    assert_eq!(
        observation.check_current_at(&login, after),
        Err(ConsoleLoginRefusal::Expired)
    );
    let unavailable = UNIX_EPOCH.checked_sub(Duration::from_millis(1)).unwrap();
    assert_eq!(
        observation.check_current_at(&login, unavailable),
        Err(ConsoleLoginRefusal::Unavailable)
    );
    assert!(
        unchanged.check_current().is_ok(),
        "time checks do not publish owner changes"
    );
    assert_eq!(login.0.selection.expires_at_ms, expires);
    assert_eq!(login.0.state.lock().unwrap().reservations, 0);
}

#[test]
fn observed_login_preserves_real_revoke_callback_poison_without_publication() {
    let (_pipe, logins, publication) = owner();
    let login = logins
        .fixture_enroll(1, selection("a", now_ms().unwrap() + 60_000))
        .unwrap();
    let observation = logins.observe_login(&login).unwrap();
    let (_, unchanged) = publication.observe(|| ()).unwrap();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = login
            .with_current_revoke(|| panic!("actual revoke callback failed before native mutation"));
    }));
    assert!(panicked.is_err());
    assert!(login.0.state.is_poisoned());
    assert!(
        unchanged.check_current().is_ok(),
        "callback did not publish a native mutation"
    );
    assert_eq!(login.check(), Err(ConsoleLoginRefusal::Unavailable));
    assert_eq!(login.try_check(), Err(ConsoleLoginRefusal::Unavailable));
    assert_eq!(
        observation.check_current(&login),
        Err(ConsoleLoginRefusal::Unavailable)
    );
    assert!(matches!(
        logins.observe_login(&login),
        Err(ConsoleLoginRefusal::Unavailable)
    ));
    assert!(matches!(
        GovernedEntryFence::enter(&login),
        Err(GovernedEntryRefusal::Unavailable(_))
    ));
}

#[test]
fn observed_login_refuses_actual_last_publisher_loss_without_publication() {
    let (pipe, logins, publication) = owner();
    let login = logins
        .fixture_enroll(1, selection("a", now_ms().unwrap() + 60_000))
        .unwrap();
    let observation = logins.observe_login(&login).unwrap();
    let (_, unchanged) = publication.observe(|| ()).unwrap();
    let last_publisher = pipe.clone();
    drop(pipe);
    assert!(unchanged.check_current().is_ok());
    assert_eq!(
        login.check(),
        Ok(()),
        "another real close publisher still owns the channel"
    );
    assert_eq!(observation.check_current(&login), Ok(()));
    // Exercise the real publisher's last drop during unwind, without calling
    // close or relying on any host callback returning to publish that close.
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _last = last_publisher;
        panic!("host cleanup failed before explicit close");
    }));
    assert!(panicked.is_err());
    assert!(
        unchanged.check_current().is_ok(),
        "last sender loss did not publish explicit close"
    );
    assert!(
        !login.0.state.is_poisoned(),
        "this is channel loss, not callback state poison"
    );
    assert_eq!(login.check(), Err(ConsoleLoginRefusal::Unavailable));
    assert_eq!(login.try_check(), Err(ConsoleLoginRefusal::Unavailable));
    assert_eq!(
        observation.check_current(&login),
        Err(ConsoleLoginRefusal::Unavailable)
    );
    assert!(matches!(
        logins.observe_login(&login),
        Err(ConsoleLoginRefusal::Unavailable)
    ));
    assert!(matches!(
        GovernedEntryFence::enter(&login),
        Err(GovernedEntryRefusal::Unavailable(_))
    ));
}
