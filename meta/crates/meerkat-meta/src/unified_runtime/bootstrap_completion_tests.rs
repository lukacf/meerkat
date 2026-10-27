//! The runtime's one-use bootstrap completion: minted once on each bootstrap
//! success path and on no failure, taken once, and published at most once,
//! serialized against the runtime's close and the mob's admission close.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use meerkat_mob::{CloseIfIdleOutcome, MobAdmissionCloseReason};

use crate::identity_first::{
    AgentIdentity, ContinuityStore, HeldBootstrapStart, IdentityBootstrapMode,
    IdentityFirstRuntimeContext, LocalContinuityStore,
};

use super::held_identity_test_support::{
    Roster, governed_runtime, held_runtime, identity_runtime, member, stage_running_mob_activation,
};
use super::identity_install_phase_tests::{context, runtime};
use super::{BootstrapCompletionRefused, BootstrapCompletionUnavailable, UnifiedRuntime};

/// The setup deadline of a held bootstrap: room for its one member to reach
/// its hold and for the pass to choose `Activate` well before it.
const HOLD: Duration = Duration::from_secs(5);

/// Why the runtime has no completion to hand out; `None` takes one.
fn unavailable(runtime: &UnifiedRuntime) -> Option<BootstrapCompletionUnavailable> {
    runtime.take_bootstrap_completion().err()
}

/// A legacy-constructed runtime activated without an identity context: its
/// completion is minted.
async fn activated(temp: &tempfile::TempDir) -> UnifiedRuntime {
    let mut runtime = runtime(temp).await;
    runtime
        .activate_without_identity_context()
        .await
        .expect("activate without an identity context");
    runtime
}

/// An unheld identity bootstrap mints the completion on `Ok`. It is taken
/// once, then answers `AlreadyTaken`, and its publication runs `f` once and
/// returns its value.
#[tokio::test]
async fn an_unheld_identity_bootstrap_mints_one_completion() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    assert_eq!(
        unavailable(&runtime),
        Some(BootstrapCompletionUnavailable::NotCompleted)
    );
    let context = context(&runtime);
    runtime
        .install_and_bootstrap_identity_first_context(context, &[])
        .await
        .expect("bootstrap the identity context");
    let completion = runtime
        .take_bootstrap_completion()
        .expect("the bootstrap minted the completion");
    assert_eq!(
        unavailable(&runtime),
        Some(BootstrapCompletionUnavailable::AlreadyTaken)
    );
    let runs = AtomicUsize::new(0);
    let published = completion.publish(|| {
        runs.fetch_add(1, Ordering::SeqCst);
        "recorded"
    });
    assert_eq!(published, Ok("recorded"));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;
}

/// A held bootstrap mints on `Activated`. One whose completion the host
/// refuses (`Unsettled`) mints nothing, before or after its failure
/// shutdown. Both runtimes are built by the builder, whose classic
/// activation minted a completion; installing the identity context withdrew
/// it.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_bootstrap_mints_only_when_activated() {
    let dir = tempfile::tempdir().unwrap();
    let identity = AgentIdentity::parse("completion-held").unwrap();
    let roster = vec![member(&identity)];
    let (mut runtime, _continuity) = held_runtime(&dir, "completion-held", &roster).await;
    assert_eq!(
        unavailable(&runtime),
        Some(BootstrapCompletionUnavailable::NotCompleted),
        "the installed identity context withdrew the classic completion"
    );
    let started = runtime
        .bootstrap_installed_identity_first_context_held_until(
            &roster,
            tokio::time::Instant::now() + HOLD,
            Box::new(|_members, _completion| Box::pin(async { Ok(()) })),
        )
        .await
        .expect("the held bootstrap starts");
    assert!(
        matches!(started, HeldBootstrapStart::Activated(_)),
        "{started:?}"
    );
    let _completion = runtime
        .take_bootstrap_completion()
        .expect("the activated held bootstrap minted the completion");
    runtime.shutdown().await;

    let dir = tempfile::tempdir().unwrap();
    let identity = AgentIdentity::parse("completion-held-refused").unwrap();
    let roster = vec![member(&identity)];
    let (mut runtime, _continuity) = held_runtime(&dir, "completion-held-refused", &roster).await;
    let started = runtime
        .bootstrap_installed_identity_first_context_held_until(
            &roster,
            tokio::time::Instant::now() + HOLD,
            Box::new(|_members, _completion| {
                Box::pin(async { Err("the host refuses the application".to_string()) })
            }),
        )
        .await
        .expect("the held bootstrap starts");
    let HeldBootstrapStart::Unsettled(settlement) = started else {
        panic!("a refused completion does not activate: {started:?}");
    };
    assert_eq!(
        unavailable(&runtime),
        Some(BootstrapCompletionUnavailable::NotCompleted)
    );
    let _error = runtime.finish_held_identity_bootstrap(settlement).await;
    assert_eq!(
        unavailable(&runtime),
        Some(BootstrapCompletionUnavailable::NotCompleted)
    );
}

/// Without an identity context the completion exists only once
/// `activate_without_identity_context` returned `Ok`: a legacy constructor
/// returns an active runtime with none. It mints with nothing pending and
/// with a staged activation alike, and the builder passes through it.
#[tokio::test]
async fn activation_without_an_identity_context_mints_the_completion() {
    for staged in [false, true] {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = runtime(&temp).await;
        if staged {
            stage_running_mob_activation(&mut runtime);
        }
        assert_eq!(
            unavailable(&runtime),
            Some(BootstrapCompletionUnavailable::NotCompleted),
            "staged={staged}"
        );
        runtime
            .activate_without_identity_context()
            .await
            .expect("activate without an identity context");
        assert_eq!(unavailable(&runtime), None, "staged={staged}");
        runtime.shutdown().await;
    }

    let dir = tempfile::tempdir().unwrap();
    let runtime = governed_runtime(&dir, "completion-builder").await;
    assert_eq!(unavailable(&runtime), None, "the builder's classic path");
    runtime.shutdown().await;
}

/// A failed identity bootstrap mints nothing: it closes the runtime instead.
#[tokio::test]
async fn a_failed_identity_bootstrap_mints_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    let continuity: Arc<dyn ContinuityStore> =
        Arc::new(LocalContinuityStore::in_memory().expect("continuity store"));
    let context = Arc::new(IdentityFirstRuntimeContext::new_with_bootstrap_mode(
        identity_runtime(&runtime, continuity, "completion-failed"),
        Arc::new(Roster(Vec::new())),
        None,
        None,
        Some(runtime.mob_handle().definition().clone()),
        // Refused by the roster pass's own validation.
        IdentityBootstrapMode::LazyWithBackgroundWarm { concurrency: 0 },
    ));
    let Err(error) = runtime
        .install_and_bootstrap_identity_first_context(context, &[])
        .await
    else {
        panic!("an invalid bootstrap mode fails the bootstrap");
    };
    assert!(
        error
            .to_string()
            .contains("concurrency must be greater than 0"),
        "{error}"
    );
    assert_eq!(
        unavailable(&runtime),
        Some(BootstrapCompletionUnavailable::NotCompleted)
    );
}

/// A publication after the runtime's close began is refused and runs
/// nothing: right as the close begins, while the mob still admits work, and
/// after a whole shutdown.
#[tokio::test]
async fn a_publication_after_the_close_began_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = activated(&temp).await;
    let completion = runtime.take_bootstrap_completion().expect("minted");
    // The first step of every shutdown.
    runtime.close_gate.close();
    assert!(
        runtime.mob_handle().admission_closed().borrow().is_none(),
        "the mob still admits work"
    );
    let ran = AtomicBool::new(false);
    assert_eq!(
        completion.publish(|| ran.store(true, Ordering::SeqCst)),
        Err(BootstrapCompletionRefused::RuntimeClosing)
    );
    assert!(!ran.load(Ordering::SeqCst), "f ran after the close began");
    runtime.shutdown().await;

    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = activated(&temp).await;
    let completion = runtime.take_bootstrap_completion().expect("minted");
    runtime.shutdown().await;
    assert_eq!(
        completion.publish(|| ran.store(true, Ordering::SeqCst)),
        Err(BootstrapCompletionRefused::RuntimeClosing)
    );
    assert!(!ran.load(Ordering::SeqCst), "f ran after the shutdown");
}

/// A running publication finishes before the runtime's close begins: a
/// shutdown that reaches the gate meanwhile waits there, and begins only
/// once `f` returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_publication_finishes_before_the_close_begins() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = Arc::new(activated(&temp).await);
    let completion = runtime.take_bootstrap_completion().expect("minted");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let publication = tokio::task::spawn_blocking(move || {
        completion.publish(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            "published"
        })
    });
    entered_rx.await.expect("the publication runs f");
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    runtime.close_gate.tell_close_reached(reached_tx);
    let closing = Arc::clone(&runtime);
    let shutdown = tokio::spawn(async move { closing.shutdown().await });
    tokio::time::timeout(Duration::from_secs(30), reached_rx)
        .await
        .expect("the shutdown reaches the gate")
        .expect("told");
    for _ in 0..20 {
        assert!(
            !runtime.shutting_down.load(Ordering::SeqCst),
            "the close began while a publication ran"
        );
        assert!(!shutdown.is_finished(), "the shutdown ran past the gate");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    release_tx.send(()).unwrap();
    assert_eq!(publication.await.unwrap(), Ok("published"));
    tokio::time::timeout(Duration::from_mins(2), shutdown)
        .await
        .expect("the shutdown completes once f returned")
        .expect("the shutdown task");
    assert!(runtime.shutting_down.load(Ordering::SeqCst));
}

/// A publication after the mob's admission closed (with no runtime
/// shutdown) is refused and runs nothing.
#[tokio::test]
async fn a_publication_after_the_admission_closed_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = activated(&temp).await;
    let completion = runtime.take_bootstrap_completion().expect("minted");
    runtime
        .mob_handle()
        .close_admission(MobAdmissionCloseReason::StandDown)
        .await
        .expect("close the mob's admission");
    let ran = AtomicBool::new(false);
    assert_eq!(
        completion.publish(|| ran.store(true, Ordering::SeqCst)),
        Err(BootstrapCompletionRefused::AdmissionClosed)
    );
    assert!(
        !ran.load(Ordering::SeqCst),
        "f ran after the admission closed"
    );
    runtime.shutdown().await;
}

/// While `f` runs, its owner lease keeps the mob from an idle close: the
/// close answers busy with that one foreground owner, and closes once `f`
/// returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_publication_keeps_the_mob_from_an_idle_close() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = activated(&temp).await;
    let completion = runtime.take_bootstrap_completion().expect("minted");
    let observer = runtime.mob_handle().observer();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let publication = tokio::task::spawn_blocking(move || {
        completion.publish(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        })
    });
    entered_rx.await.expect("the publication runs f");
    match observer.close_if_idle().await.expect("close if idle") {
        CloseIfIdleOutcome::Busy(busy) => assert_eq!(busy.foreground_owners, 1, "{busy:?}"),
        other => panic!("an idle close landed while f ran: {other:?}"),
    }
    release_tx.send(()).unwrap();
    assert_eq!(publication.await.unwrap(), Ok(()));
    assert_eq!(
        observer.close_if_idle().await.expect("close if idle"),
        CloseIfIdleOutcome::Closed
    );
    runtime.shutdown().await;
}

/// Attaching an identity context leaves a classic completion as it is: this
/// runtime does not bootstrap an attached roster, so nothing would mint
/// again.
#[tokio::test]
async fn attaching_an_identity_context_keeps_the_completion() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = activated(&temp).await;
    let context = context(&runtime);
    runtime.attach_identity_first_context(context);
    assert_eq!(
        unavailable(&runtime),
        None,
        "the classic completion survives an attach"
    );
    runtime.shutdown().await;
}

/// A publication whose `f` panics poisons the gate's lock. The panic
/// propagates, the runtime still closes, and a later publication is still
/// refused: both read through the poison.
#[tokio::test]
async fn a_panicking_publication_leaves_the_gate_working() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = activated(&temp).await;
    let completion = runtime.take_bootstrap_completion().expect("minted");
    let later = completion.sibling_for_test();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        completion.publish(|| panic!("f panics inside the gate"))
    }));
    assert!(panicked.is_err(), "the panic propagates out of publish");
    tokio::time::timeout(runtime.shutdown_horizon(), runtime.shutdown())
        .await
        .expect("the shutdown passes the poisoned gate");
    let ran = AtomicBool::new(false);
    assert_eq!(
        later.publish(|| ran.store(true, Ordering::SeqCst)),
        Err(BootstrapCompletionRefused::RuntimeClosing)
    );
    assert!(!ran.load(Ordering::SeqCst), "f ran after the close");
}

#[test]
fn the_completion_crosses_threads() {
    fn send<T: Send>() {}
    send::<super::RuntimeBootstrapCompleted>();
}
