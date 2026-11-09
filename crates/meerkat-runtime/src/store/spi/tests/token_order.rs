//! The token order a host store keeps: every grant answers a token above
//! every earlier grant of the realm, in either mode and to any caller, a
//! takeover of the writer's own recorded lease included. A store that grants
//! a token at or below one already granted lets two writers carry one token,
//! or a later writer a lower one, so a stopped writer's re-acquire checks its
//! grant against the token it holds, and against a grant of its own whose
//! listing failed, and loses the realm, naming the store's broken order,
//! when the grant is not above both.

use super::*;

// Unrun until the candidate's validation. A renew stop at 1 s; at 2 s the
// re-acquire's exclusive acquire meets the writer's own lease and takes it
// over, and the store answers the takeover with the token it superseded
// (token 1, the writer's own), which holds the fence again: it broke its
// token order. The realm is lost, its cause names the store's broken token
// order and not another writer, and nothing is written again. Before, the
// writer installed token 1 again, renewed it and resumed.
#[tokio::test(start_paused = true)]
async fn a_grant_at_or_below_the_installed_token_loses_the_realm() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let (realm, store) = open(&host).await;
    assert_eq!(realm.fence().get(), 1);
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1;
        faults.grant_reuses_token = 1;
    }
    let landed = host.commit_count();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        host.token(),
        1,
        "the store answered the takeover with token 1"
    );
    let cause = realm
        .realm_poisoned()
        .expect("a grant at or below the installed token loses the realm");
    assert!(cause.contains("token order"), "{cause}");
    assert!(!cause.contains("another writer"), "{cause}");
    assert!(!realm.lease_is_live());
    let refused = store
        .persist_input_state(
            &LogicalRuntimeId::new("after"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await;
    assert!(
        matches!(refused, Err(RuntimeStoreError::RealmRefused { .. })),
        "{refused:?}"
    );
    assert_eq!(
        host.commit_count(),
        landed,
        "nothing was written under the reused token"
    );
}
