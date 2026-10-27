use super::*;

fn selection(scope: &str, expires: u64) -> ConsoleLoginSelection {
    ConsoleLoginSelection {
        issuer: "https://console.example/issuer".into(),
        subject: "verified-sub".into(),
        login_scope_id: scope.into(),
        expires_at_ms: expires,
    }
}

fn owner() -> (watch::Sender<bool>, ConsoleLogins) {
    let (send, receive) = watch::channel(false);
    (send, ConsoleLogins::new(PrivateHostClosed(receive), None))
}

#[tokio::test]
async fn retirement_orders_reservations_and_keeps_another_login_usable() {
    let (_connection, logins) = owner();
    let expires = now_ms().unwrap() + 60_000;
    logins.enroll(2, selection("a", expires)).unwrap();
    logins.enroll(3, selection("b", expires)).unwrap();
    let a = logins
        .resolve("https://console.example/issuer", "verified-sub", "a")
        .unwrap();
    let b = logins
        .resolve("https://console.example/issuer", "verified-sub", "b")
        .unwrap();
    let entered = a.reserve().unwrap();
    let waiting = logins.retire(2).unwrap();
    assert_eq!(a.check(), Err(ConsoleLoginRefusal::Retired));
    assert!(matches!(a.reserve(), Err(ConsoleLoginRefusal::Retired)));
    let mut drain = Box::pin(waiting.drain());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut drain)
            .await
            .is_err()
    );
    assert_eq!(a.closed().await, ConsoleLoginRefusal::Retired);
    b.reserve().unwrap().settled();
    entered.settled();
    assert_eq!(
        drain.await.unwrap(),
        ConsoleLoginRetirement {
            fenced: true,
            reservations_drained: true,
            uncertain: false
        }
    );
    assert_eq!(
        logins.retire(2).unwrap().drain().await.unwrap().uncertain,
        false
    );
    assert!(
        logins.enroll(4, selection("a", expires)).is_err(),
        "retired unexpired scope must not retarget a token"
    );
}

#[tokio::test]
async fn dropped_reservation_reports_uncertainty_without_an_input_ledger() {
    let (_connection, logins) = owner();
    logins
        .enroll(2, selection("a", now_ms().unwrap() + 60_000))
        .unwrap();
    let a = logins
        .resolve("https://console.example/issuer", "verified-sub", "a")
        .unwrap();
    let reserved = a.reserve().unwrap();
    let waiting = logins.retire(2).unwrap();
    drop(reserved);
    let observed = waiting.drain().await.unwrap();
    assert!(observed.fenced && observed.reservations_drained && observed.uncertain);
    assert!(logins.retire(2).unwrap().drain().await.unwrap().uncertain);
}

#[tokio::test]
async fn terminal_connection_loss_is_unavailable_even_after_retirement() {
    let (connection, logins) = owner();
    logins
        .enroll(2, selection("a", now_ms().unwrap() + 60_000))
        .unwrap();
    let a = logins
        .resolve("https://console.example/issuer", "verified-sub", "a")
        .unwrap();
    let reserved = a.reserve().unwrap();
    let waiting = logins.retire(2).unwrap();
    connection.send_replace(true);
    assert_eq!(a.closed().await, ConsoleLoginRefusal::Unavailable);
    assert!(matches!(
        waiting.drain().await,
        Err(HostLifetimeError::Unavailable)
    ));
    assert!(matches!(a.reserve(), Err(ConsoleLoginRefusal::Unavailable)));
    reserved.settled();
    let (_new_connection, new_logins) = owner();
    assert!(matches!(
        new_logins.retire(2),
        Err(HostLifetimeError::WrongOwner)
    ));
    assert!(matches!(
        new_logins.resolve("https://console.example/issuer", "verified-sub", "a"),
        Err(ConsoleLoginRefusal::Unknown)
    ));
}

#[tokio::test]
async fn expiry_wakes_an_idle_handle_without_new_frames() {
    let (_connection, logins) = owner();
    logins
        .enroll(2, selection("a", now_ms().unwrap() + 100))
        .unwrap();
    let a = logins
        .resolve("https://console.example/issuer", "verified-sub", "a")
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), a.closed())
            .await
            .unwrap(),
        ConsoleLoginRefusal::Expired
    );
    assert!(matches!(a.reserve(), Err(ConsoleLoginRefusal::Expired)));
}

#[test]
fn exact_tuple_is_not_an_email_alias_or_a_different_owner() {
    let (_connection, logins) = owner();
    logins
        .enroll(2, selection("a", now_ms().unwrap() + 60_000))
        .unwrap();
    for (issuer, subject, scope) in [
        ("https://other.example", "verified-sub", "a"),
        (
            "https://console.example/issuer",
            "same-email@example.org",
            "a",
        ),
        ("https://console.example/issuer", "verified-sub", "b"),
    ] {
        assert!(matches!(
            logins.resolve(issuer, subject, scope),
            Err(ConsoleLoginRefusal::Unknown)
        ));
    }
}


#[tokio::test]
async fn native_entry_fence_uses_the_same_retirement_reservation() {
    let (_connection, logins) = owner();
    logins.enroll(2, selection("native", now_ms().unwrap() + 60_000)).unwrap();
    let login = logins.resolve("https://console.example/issuer", "verified-sub", "native").unwrap();
    let fence: Arc<dyn GovernedEntryFence> = Arc::new(login.clone());
    let permit = fence.enter().unwrap();
    let mut retirement = Box::pin(logins.retire(2).unwrap().drain());
    assert!(matches!(fence.enter(), Err(GovernedEntryRefusal::Retired)));
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut retirement).await.is_err());
    permit.finish(GovernedEntryOutcome::Entered);
    let observed = retirement.await.unwrap();
    assert!(observed.fenced && observed.reservations_drained && !observed.uncertain);
}

#[tokio::test]
async fn native_unentered_is_definite_but_uncertain_or_dropped_permit_stays_unknown() {
    for (scope, outcome) in [("refused", Some(GovernedEntryOutcome::Unentered)),
        ("unknown", Some(GovernedEntryOutcome::Uncertain)), ("dropped", None)] {
        let (_connection, logins) = owner();
        logins.enroll(2, selection(scope, now_ms().unwrap() + 60_000)).unwrap();
        let login = logins.resolve("https://console.example/issuer", "verified-sub", scope).unwrap();
        let permit = GovernedEntryFence::enter(&login).unwrap();
        let waiting = logins.retire(2).unwrap();
        if let Some(outcome) = outcome { permit.finish(outcome); } else { drop(permit); }
        let observed = waiting.drain().await.unwrap();
        assert_eq!(observed.uncertain, scope != "refused");
    }
}

#[tokio::test]
async fn native_fence_does_not_retarget_another_login_or_clear_pipe_loss() {
    let (connection, logins) = owner();
    let expires = now_ms().unwrap() + 60_000;
    logins.enroll(2, selection("a", expires)).unwrap();
    logins.enroll(3, selection("b", expires)).unwrap();
    let a = logins.resolve("https://console.example/issuer", "verified-sub", "a").unwrap();
    let a_again = logins.resolve("https://console.example/issuer", "verified-sub", "a").unwrap();
    let b = logins.resolve("https://console.example/issuer", "verified-sub", "b").unwrap();
    assert!(a.same_owner(&a_again));
    assert!(!a.same_owner(&b));
    logins.retire(2).unwrap().drain().await.unwrap();
    assert!(matches!(GovernedEntryFence::enter(&a), Err(GovernedEntryRefusal::Retired)));
    GovernedEntryFence::enter(&b).unwrap().finish(GovernedEntryOutcome::Unentered);
    connection.send_replace(true);
    assert!(matches!(GovernedEntryFence::enter(&b), Err(GovernedEntryRefusal::Unavailable(_))));
}

#[tokio::test]
async fn nonwaiting_check_keeps_contention_and_terminal_login_refusals_distinct() {
    let (connection, logins) = owner();
    logins.enroll(2, selection("cold", now_ms().unwrap() + 60_000)).unwrap();
    let login = logins.resolve("https://console.example/issuer", "verified-sub", "cold").unwrap();
    let locked = login.0.state.lock().unwrap();
    assert_eq!(login.try_check(), Err(ConsoleLoginRefusal::Unavailable));
    drop(locked);
    assert_eq!(login.try_check(), Ok(()));
    logins.retire(2).unwrap().drain().await.unwrap();
    assert_eq!(login.try_check(), Err(ConsoleLoginRefusal::Retired));
    connection.send_replace(true);
    assert_eq!(login.try_check(), Err(ConsoleLoginRefusal::Unavailable));
}


fn published_owner() -> (super::super::PrivateHostClosePublisher, ConsoleLogins,
    LocalAuthorizationPublication)
{
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

#[tokio::test]
async fn native_publication_tracks_login_enrollment_retirement_and_pipe_close() {
    let (pipe, logins, publication) = published_owner();
    let expires = now_ms().unwrap() + 60_000;
    let (_, before_enrollment) = publication.observe(|| ()).unwrap();
    let a = logins.fixture_enroll(2, selection("a", expires)).unwrap();
    assert!(before_enrollment.check_current().is_err());
    let (observed, captured) = publication.observe(|| a.try_check()).unwrap();
    assert_eq!(observed, Ok(()));
    assert!(captured.check_current().is_ok());
    {
        // Stamp currentness does not replace the real login check or its fence.
        let _held = a.0.state.lock().unwrap();
        assert!(captured.check_current().is_ok());
        assert_eq!(a.try_check(), Err(ConsoleLoginRefusal::Unavailable));
    }
    logins.retire(2).unwrap().drain().await.unwrap();
    assert!(captured.check_current().is_err());
    assert_eq!(a.try_check(), Err(ConsoleLoginRefusal::Retired));
    assert!(matches!(GovernedEntryFence::enter(&a), Err(GovernedEntryRefusal::Retired)));
    assert!(logins.enroll(3, selection("a", expires)).is_err());

    let b = logins.fixture_enroll(4, selection("b", expires)).unwrap();
    assert!(!a.same_owner(&b));
    assert!(matches!(GovernedEntryFence::enter(&a), Err(GovernedEntryRefusal::Retired)));
    let (observed, current) = publication.observe(|| b.try_check()).unwrap();
    assert_eq!(observed, Ok(()));
    drop(publication.begin_owner_change().unwrap());
    assert!(current.check_current().is_err());
    assert_eq!(b.try_check(), Ok(()));
    GovernedEntryFence::enter(&b).unwrap().finish(GovernedEntryOutcome::Unentered);
    let (observed, current) = publication.observe(|| b.try_check()).unwrap();
    assert_eq!(observed, Ok(()));
    pipe.close().unwrap();
    assert!(current.check_current().is_err());
    assert_eq!(b.try_check(), Err(ConsoleLoginRefusal::Unavailable));
    assert!(matches!(GovernedEntryFence::enter(&b), Err(GovernedEntryRefusal::Unavailable(_))));
}

#[tokio::test]
async fn actual_login_expiry_refuses_native_entry_while_publication_is_unchanged() {
    let (_pipe, logins, publication) = published_owner();
    let expires = now_ms().unwrap() + 200;
    let old = logins.fixture_enroll(2, selection("same", expires)).unwrap();
    let (observed, captured) = publication.observe(|| old.try_check()).unwrap();
    assert_eq!(observed, Ok(()));
    // Observe the actual wall-clock deadline, without modifying this login.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while now_ms().unwrap() < expires && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(now_ms().unwrap() >= expires, "actual login deadline was reached");
    assert!(captured.check_current().is_ok());
    assert_eq!(old.try_check(), Err(ConsoleLoginRefusal::Expired));
    assert!(matches!(GovernedEntryFence::enter(&old), Err(GovernedEntryRefusal::Expired)));
    let replacement = logins.fixture_enroll(3, selection("same", now_ms().unwrap() + 60_000)).unwrap();
    assert!(!old.same_owner(&replacement));
    assert!(captured.check_current().is_err());
    assert_eq!(old.try_check(), Err(ConsoleLoginRefusal::Expired));
    assert!(matches!(GovernedEntryFence::enter(&old), Err(GovernedEntryRefusal::Expired)));
    assert_eq!(replacement.try_check(), Ok(()));
    GovernedEntryFence::enter(&replacement).unwrap().finish(GovernedEntryOutcome::Unentered);
}

#[tokio::test]
async fn native_publication_refuses_owner_mutation_during_login_observation() {
    let (_pipe, logins, publication) = published_owner();
    let login = logins.fixture_enroll(2, selection("a", now_ms().unwrap() + 60_000)).unwrap();
    let mut waiting = None;
    let changed = publication.observe(|| {
        let observed = login.try_check();
        assert_eq!(observed, Ok(()));
        waiting = Some(logins.retire(2).unwrap());
        observed
    });
    assert!(changed.is_err());
    waiting.unwrap().drain().await.unwrap();
    assert_eq!(login.try_check(), Err(ConsoleLoginRefusal::Retired));
    assert!(matches!(GovernedEntryFence::enter(&login), Err(GovernedEntryRefusal::Retired)));

    let (_legacy_pipe, legacy) = owner();
    let legacy_login = legacy.fixture_enroll(2, selection("a", now_ms().unwrap() + 60_000)).unwrap();
    assert_eq!(legacy_login.check(), Ok(()));
    assert!(legacy.publication.is_none(), "an unbound owner has no native publication to observe");
}

#[tokio::test]
async fn revoke_entry_excludes_actual_retirement_without_waiting_on_publication() {
    let (_pipe, logins, publication) = published_owner();
    let login = logins.fixture_enroll(2, selection("revoke", now_ms().unwrap() + 60_000)).unwrap();
    let (_, before) = publication.observe(|| ()).unwrap();
    let mut worker = None;
    let outcome = login.with_current_revoke(|| {
        let retire = logins.clone();
        worker = Some(std::thread::spawn(move || retire.retire(2)));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while before.check_current().is_ok() && std::time::Instant::now() < deadline { std::thread::yield_now(); }
        // The actual retire owner has acquired publication and is blocked on
        // this entry's state mutex. The public nonblocking writer must refuse.
        (before.check_current().is_err(), publication.try_begin_owner_change().is_err())
    });
    let waiting = worker.unwrap().join().unwrap().unwrap();
    waiting.drain().await.unwrap();
    assert_eq!(outcome, Ok((true, true)));
    let entered = std::sync::atomic::AtomicBool::new(false);
    assert_eq!(login.with_current_revoke(|| entered.store(true, std::sync::atomic::Ordering::SeqCst)),
        Err(ConsoleLoginRefusal::Retired));
    assert!(!entered.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn revoke_entry_excludes_actual_pipe_close_and_releases_on_native_contention() {
    let (pipe, logins, publication) = published_owner();
    let login = logins.fixture_enroll(2, selection("pipe", now_ms().unwrap() + 60_000)).unwrap();
    let (_, before) = publication.observe(|| ()).unwrap();
    let mut worker = None;
    let outcome = login.with_current_revoke(|| {
        worker = Some(std::thread::spawn(move || pipe.close()));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while before.check_current().is_ok() && std::time::Instant::now() < deadline { std::thread::yield_now(); }
        (before.check_current().is_err(), publication.try_begin_owner_change().is_err())
    });
    worker.unwrap().join().unwrap().unwrap();
    assert_eq!(outcome, Ok((true, true)));
    assert_eq!(login.with_current_revoke(|| ()), Err(ConsoleLoginRefusal::Unavailable));
}


#[tokio::test]
async fn access_result_classification_retains_entered_native_uncertainty_on_the_actual_login() {
    let (_pipe, logins) = owner();
    let login = logins.fixture_enroll(2, selection("native-result", now_ms().unwrap() + 60_000)).unwrap();
    // Explicit result-classification seam only. This typed value does not
    // pretend the fixture executed a native store failure or revoke call.
    let result = crate::host_gateway::ConsoleSetupBinding::finish_access_revoke(login.reserve().unwrap(),
        Ok(Err(meerkat_authorization::grants::ExpectedRevokeRefusal::Grant(
            meerkat_authorization::grants::GrantRefusal::Unavailable)))).unwrap();
    // A later definite refusal settles only its own reservation. It cannot
    // erase the earlier entered mutation's uncertain result on this login.
    let later = crate::host_gateway::ConsoleSetupBinding::finish_access_revoke(login.reserve().unwrap(),
        Ok(Err(meerkat_authorization::grants::ExpectedRevokeRefusal::NotCurrent))).unwrap();
    let retired = logins.fixture_retire(2).unwrap().await.unwrap();
    assert_eq!(result["state"], "unavailable");
    assert_eq!(later["state"], "not_current");
    assert!(retired.reservations_drained); assert!(retired.uncertain);
}

#[tokio::test]
async fn access_not_current_classification_settles_only_its_actual_login_reservation() {
    let (_pipe, logins) = owner();
    let login = logins.fixture_enroll(2, selection("not-current", now_ms().unwrap() + 60_000)).unwrap();
    // Classification only: this is an actual login reservation, not an
    // executed native mutation or a joined AccessController policy swap.
    let result = crate::host_gateway::ConsoleSetupBinding::finish_access_revoke(login.reserve().unwrap(),
        Ok(Err(meerkat_authorization::grants::ExpectedRevokeRefusal::NotCurrent))).unwrap();
    let retired = logins.fixture_retire(2).unwrap().await.unwrap();
    assert_eq!(result["scope"], "application_issued_access");
    assert_eq!(result["state"], "not_current");
    assert!(retired.reservations_drained); assert!(!retired.uncertain);
}


#[tokio::test]
async fn mutation_try_reserve_refuses_contention_and_retains_the_same_login_until_settlement() {
    let (_connection, logins) = owner();
    logins.enroll(71, selection("mutation", now_ms().unwrap() + 60_000)).unwrap();
    let login = logins.resolve("https://console.example/issuer", "verified-sub", "mutation").unwrap();
    let held_state = login.0.state.lock().unwrap();
    let worker_login = login.clone();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let result = worker_login.try_reserve();
        let refused = matches!(&result, Err(ConsoleLoginRefusal::Unavailable));
        if let Ok(reservation) = result { reservation.settled(); }
        let _ = send.send(refused);
    });
    // The result must arrive before releasing the real state lock. A blocking
    // reserve regression times out, but cleanup releases and joins it first.
    let while_held = receive.recv_timeout(Duration::from_secs(2));
    drop(held_state);
    let joined = worker.join();
    assert!(joined.is_ok());
    assert_eq!(while_held, Ok(true));
    assert_eq!(login.0.state.lock().unwrap().reservations, 0);
    assert!(!login.0.state.lock().unwrap().uncertain);

    let reservation = login.try_reserve().unwrap();
    let retained = Arc::new(Mutex::new(Some(reservation)));
    let permit = meerkat_core::EnteredAdministration::holding(retained.clone());
    let waiting = logins.retire(71).unwrap();
    assert!(matches!(login.try_reserve(), Err(ConsoleLoginRefusal::Retired)));
    let mut drain = Box::pin(waiting.drain());
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut drain).await.is_err());
    // The native contract drops its opaque hold after the actual result; the
    // caller then classifies that result on the existing reservation. This is
    // a custody control, not execution of a native persistence operation.
    drop(permit);
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut drain).await.is_err());
    retained.lock().unwrap().take().unwrap().settled();
    let observed = drain.await.unwrap();
    assert!(observed.fenced && observed.reservations_drained && !observed.uncertain);
}
