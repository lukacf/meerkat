//! Controls of the shared SDK case for a selected check's two checks after
//! its effect (`release_shared_check`). An owner change held open on the
//! host's shared publication at either check is waited out, while the
//! check's entry is still unsettled, and the check's dispatch, its join and
//! its release each run once. A change that outlasts the bound ends that
//! check typed and uncertified, with no replay of the join or the release.
//! A reviewed install's check before its physical dispatch, which holds the
//! install's dispatch guard, answers a change at once: refused, certified
//! not dispatched and typed, and the model's one fresh install goes on. An
//! install that already consumed its checked proposal and meets a change
//! outlasting the bound at its shared entry reservation is refused there,
//! certified, settles as failed and gets its proposal back: its fresh
//! install is reviewed afresh and installs once.
use super::*;
use crate::service_install::shared_check_probe::{self, Step};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND;
use std::time::Instant;

/// B's first selected check: the call the controls act on.
const CALL: &str = "b-helper-check";
/// B's first install: the call the install control acts on.
const INSTALL_CALL: &str = "b-helper-install";
/// How often the check's held entry close looks again whether the check
/// after the join answered, and the longest it is held: the entry stays
/// unsettled while that check runs.
const CLOSE_POLL: Duration = Duration::from_millis(10);
const CLOSE_HOLD_LIMIT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Observed {
    steps: Mutex<Vec<(Step, String, Instant)>>,
    change: Mutex<Option<std::thread::JoinHandle<Instant>>>,
}
impl Observed {
    fn at(&self, step: Step, call: &str) -> Vec<Instant> {
        self.steps
            .lock()
            .unwrap()
            .iter()
            .filter(|(seen, id, _)| *seen == step && id == call)
            .map(|(_, _, at)| *at)
            .collect()
    }
    fn count(&self, step: Step, call: &str) -> usize {
        self.at(step, call).len()
    }
    /// When the held change was published.
    fn change_ended(&self) -> Instant {
        self.change
            .lock()
            .unwrap()
            .take()
            .expect("the control held an owner change at its step")
            .join()
            .unwrap()
    }
}

#[derive(Clone, Copy)]
enum Hold {
    /// Published after this long, or at once when the check refused.
    For(Duration),
    /// Published once the check after the step refused.
    UntilRefused,
}

/// At `at` of B's call `target`, begin an owner change on `publication` on a
/// thread of its own, held per `hold`. A control of B's first check also
/// holds that check's entry close until the check after the join answered.
fn install(
    observed: &Arc<Observed>,
    publication: LocalAuthorizationPublication,
    at: Step,
    target: &'static str,
    hold: Hold,
) {
    let (release, released) = std::sync::mpsc::channel::<()>();
    let release = Mutex::new(Some(release));
    let released = Mutex::new(Some(released));
    let observed = observed.clone();
    shared_check_probe::install(Arc::new(move |step, call| {
        let closing = step == Step::EntryClosing && call == CALL && target == CALL;
        if closing {
            let first = observed.count(Step::EntryClosing, CALL) == 0;
            if first {
                observed
                    .steps
                    .lock()
                    .unwrap()
                    .push((step, call.to_owned(), Instant::now()));
            }
            let since = observed.at(Step::EntryClosing, CALL)[0];
            let answered = observed.count(Step::JoinedChecked, CALL)
                + observed.count(Step::JoinedRefused, CALL)
                > 0;
            return (!answered && since.elapsed() < CLOSE_HOLD_LIMIT).then_some(CLOSE_POLL);
        }
        observed
            .steps
            .lock()
            .unwrap()
            .push((step, call.to_owned(), Instant::now()));
        if call != target {
            return None;
        }
        match step {
            Step::JoinedRefused | Step::ReleasedRefused | Step::InstallInnerRefused => {
                if let Some(release) = release.lock().unwrap().take() {
                    let _ = release.send(());
                }
            }
            _ => {}
        }
        if step == at && observed.change.lock().unwrap().is_none() {
            let publication = publication.clone();
            let released = released.lock().unwrap().take().unwrap();
            let limit = match hold {
                Hold::For(hold) => hold,
                Hold::UntilRefused => OWNER_CHANGE_WAIT_BOUND * 3,
            };
            let (began, started) = std::sync::mpsc::sync_channel(0);
            let change = std::thread::spawn(move || {
                let change = publication.begin_owner_change().unwrap();
                began.send(()).unwrap();
                let _ = released.recv_timeout(limit);
                let ending = Instant::now();
                drop(change);
                ending
            });
            started.recv().unwrap();
            *observed.change.lock().unwrap() = Some(change);
        }
        None
    }));
}

/// Run the whole shared SDK case with the change held at `at` of B's first
/// check, and answer what every selected check went through. With a change
/// that is published, the case's own settled feedback proof runs as well.
fn control(at: Step, hold: Hold) -> (Arc<Observed>, Arc<Script>) {
    control_at(at, CALL, hold)
}

/// [`control`] with the change held at `at` of B's call `target`.
fn control_at(at: Step, target: &'static str, hold: Hold) -> (Arc<Observed>, Arc<Script>) {
    let (pipes, parent) = SdkParent::pair();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let observed = Arc::new(Observed::default());
    let seen = observed.clone();
    let script = runtime.block_on(async move {
        let mut case = Case::new(parent.enter());
        let result = std::panic::AssertUnwindSafe(async {
            case.construct(pipes).await;
            let publication = case
                .host
                .as_ref()
                .unwrap()
                .authorization_publication_for_test()
                .expect("the host's private originals share one publication");
            install(&seen, publication, at, target, hold);
            case.run().await;
        })
        .catch_unwind()
        .await;
        shared_check_probe::clear();
        if result.is_err() {
            case.retain_failed_audits().await;
        }
        let cleanup = case.cleanup_completed_runtime().await;
        assert!(
            cleanup.is_empty(),
            "all SDK/native/physical owners must settle: {cleanup:?}"
        );
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
        if matches!(hold, Hold::For(_)) {
            case.assert_check_feedback().await;
        }
        case.script.clone()
    });
    (observed, script)
}

/// The steps B's first check went through: dispatched once, joined once,
/// released once, each check after its effect answered once and current.
fn assert_ran_once(observed: &Observed) {
    for step in [
        Step::Dispatched,
        Step::Joined,
        Step::JoinedChecked,
        Step::Released,
        Step::ReleasedChecked,
    ] {
        assert_eq!(observed.count(step, CALL), 1, "{step:?} runs once");
    }
    for step in [Step::JoinedRefused, Step::ReleasedRefused] {
        assert_eq!(observed.count(step, CALL), 0, "{step:?} never happens");
    }
}

#[test]
fn owner_change_at_the_check_after_the_join_is_waited_out_once() {
    let hold = Duration::from_millis(120);
    let (observed, script) = control(Step::Joined, Hold::For(hold));
    assert_ran_once(&observed);
    let ended = observed.change_ended();
    let checked = observed.at(Step::JoinedChecked, CALL)[0];
    assert!(
        checked >= ended,
        "the check after the join answered only after the change was published"
    );
    assert!(
        observed.at(Step::EntrySettled, CALL)[0] > checked,
        "the check's wait ended while its entry was still unsettled"
    );
    assert!(
        !script
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(key, _, _)| key == &format!("{CALL}-fresh")),
        "the waited check answered; the model never checked again"
    );
}

#[test]
fn owner_change_at_the_check_after_the_release_is_waited_out_once() {
    let hold = Duration::from_millis(120);
    let (observed, script) = control(Step::Released, Hold::For(hold));
    assert_ran_once(&observed);
    let ended = observed.change_ended();
    assert!(
        observed.at(Step::ReleasedChecked, CALL)[0] >= ended,
        "the check after the release answered only after the change was published"
    );
    assert!(
        !script
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(key, _, _)| key == &format!("{CALL}-fresh")),
        "the waited check answered; the model never checked again"
    );
}

#[test]
fn owner_change_outlasting_the_check_after_the_join_ends_typed_without_replay() {
    let (observed, script) = control(Step::Joined, Hold::UntilRefused);
    let joined = observed.at(Step::Joined, CALL);
    let refused = observed.at(Step::JoinedRefused, CALL);
    assert_eq!(observed.count(Step::Dispatched, CALL), 1);
    assert_eq!(joined.len(), 1, "the selection was joined once");
    assert_eq!(refused.len(), 1, "the check after the join refused once");
    for step in [Step::JoinedChecked, Step::Released, Step::ReleasedChecked] {
        assert_eq!(
            observed.count(step, CALL),
            0,
            "{step:?}: nothing after the refused check ran"
        );
    }
    assert!(
        refused[0].duration_since(joined[0]) >= Duration::from_secs(1),
        "the check waited for the change before it refused: {:?}",
        refused[0].duration_since(joined[0])
    );
    assert!(
        observed.change_ended() >= refused[0],
        "the change was held until the check refused"
    );
    let seen = script.seen_results.lock().unwrap();
    let result = seen.get(CALL).expect("the refused check's result");
    assert!(
        find_feedback::canonical_check_refusal(result)
            && !find_feedback::certified_reprepare(result),
        "after its effect the refusal stays typed and uncertified"
    );
    // The model's one fresh check is another call with its own join and
    // release; the refused call's are never replayed.
    let fresh = format!("{CALL}-fresh");
    for step in [Step::Dispatched, Step::Joined, Step::Released] {
        assert_eq!(
            observed.count(step, &fresh),
            1,
            "{step:?} of the fresh check"
        );
    }
}

// B's first install holds its dispatch guard, which owns the physical
// dispatch, when its check before that dispatch meets an owner change held
// for 120 ms. The check answers at once, refused, certified not dispatched
// and typed; the physical install never ran for it. The model's one fresh
// install runs its own review, guard and check, and succeeds.
#[test]
fn owner_change_at_the_install_check_is_refused_certified_and_installed_fresh() {
    let hold = Duration::from_millis(120);
    let (observed, script) = control_at(Step::InstallGuarded, INSTALL_CALL, Hold::For(hold));
    assert_eq!(
        observed.count(Step::InstallGuarded, INSTALL_CALL),
        1,
        "the refused install's check ran once"
    );
    let fresh = format!("{INSTALL_CALL}-fresh");
    assert_eq!(
        observed.count(Step::InstallGuarded, &fresh),
        1,
        "the fresh install held its own guard once"
    );
    let seen = script.seen_results.lock().unwrap();
    let refused = seen
        .get(INSTALL_CALL)
        .expect("the refused install's result");
    assert!(
        refused.is_error && find_feedback::certified_reprepare(refused),
        "refused at once before the physical dispatch, typed and certified not dispatched: {}",
        refused.text_content()
    );
    assert!(
        !seen
            .get(&fresh)
            .expect("the fresh install's result")
            .is_error,
        "the one fresh install succeeded"
    );
}

// B's first install consumed its checked proposal when its inner call meets
// an owner change held past the bound at the shared entry reservation. The
// reservation waits the bound out and refuses, certified not dispatched:
// nothing of the install was sent. The install then settles as failed, not
// uncertain, and its proposal is restored, so the model's one fresh install
// is reviewed afresh, consumes the same checked proposal and installs once.
#[test]
fn owner_change_outlasting_the_install_reservation_is_refused_certified_and_installed_fresh() {
    let (observed, script) = control_at(Step::InstallConsumed, INSTALL_CALL, Hold::UntilRefused);
    let consumed = observed.at(Step::InstallConsumed, INSTALL_CALL);
    let refused = observed.at(Step::InstallInnerRefused, INSTALL_CALL);
    assert_eq!(
        consumed.len(),
        1,
        "the refused install consumed its proposal once"
    );
    assert_eq!(refused.len(), 1, "its inner call refused once");
    assert!(
        refused[0].duration_since(consumed[0]) >= Duration::from_secs(1),
        "the reservation waited for the change before it refused: {:?}",
        refused[0].duration_since(consumed[0])
    );
    assert!(
        observed.change_ended() >= refused[0],
        "the change was held until the reservation refused"
    );
    let fresh = format!("{INSTALL_CALL}-fresh");
    assert_eq!(
        observed.count(Step::InstallConsumed, &fresh),
        1,
        "the fresh install consumed the restored proposal once"
    );
    assert_eq!(observed.count(Step::InstallInnerRefused, &fresh), 0);
    let seen = script.seen_results.lock().unwrap();
    let first = seen
        .get(INSTALL_CALL)
        .expect("the refused install's result");
    assert!(
        first.is_error && find_feedback::certified_reprepare(first),
        "refused after the bound, typed and certified not dispatched: {}",
        first.text_content()
    );
    assert!(
        !seen
            .get(&fresh)
            .expect("the fresh install's result")
            .is_error,
        "the one fresh install was reviewed afresh and installed"
    );
}
