//! An ordinary send (a Find) meets its source held at one of the
//! shared binder's two input checks: the retained-source check of its input
//! preparation, and its admitted-custody check.
//!
//! A first send binds its original inside the bind (it issues the controller
//! child grant and saves the binding record) before those checks, so they are
//! past the attempt's first durable effect: the send keeps its shipped
//! outcome there, and the attempt's effect mark names that write. Only an
//! original already bound answers "not now" there, and binds once after the
//! release.
use super::*;
use crate::source_hold_probe::{Hold, Site};
use meerkat_core::protected_section::ProtectedSection;

/// How long the probe holds the source collection.
const HOLD: Duration = Duration::from_millis(300);

/// What the probe saw: each site it was asked at, its key, and the
/// attempt's effect mark there.
#[derive(Default)]
pub(in crate::private_originals) struct Seen(Mutex<Vec<(Site, String, Option<&'static str>)>>);

impl Seen {
    pub(in crate::private_originals) fn at(
        &self,
        site: Site,
    ) -> Vec<(String, Option<&'static str>)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(seen, ..)| *seen == site)
            .map(|(_, key, mark)| (key.clone(), *mark))
            .collect()
    }
}

/// Install a probe that holds the source collection at `site` on its first
/// hit, and records every site it is asked at with the effect mark there.
pub(in crate::private_originals) fn hold_at(site: Site) -> Arc<Seen> {
    hold_where(site, None, false)
}

/// [`hold_at`], holding only at `site`'s first hit for the original `key`.
pub(in crate::private_originals) fn hold_at_key(site: Site, key: &str) -> Arc<Seen> {
    hold_where(site, Some(key.to_owned()), false)
}

/// [`hold_at`], holding the collection inside an owner change on the
/// owner's publication, as another original's first binding write does.
fn hold_in_change_at(site: Site) -> Arc<Seen> {
    hold_where(site, None, true)
}

fn hold_where(site: Site, only: Option<String>, owner_change: bool) -> Arc<Seen> {
    let seen = Arc::new(Seen::default());
    let recorder = seen.clone();
    crate::source_hold_probe::install(Arc::new(move |at, key| {
        let selected = only.as_deref().is_none_or(|only| only == key);
        let first = !recorder
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|(seen, seen_key, _)| {
                *seen == at && only.as_deref().is_none_or(|only| only == seen_key)
            });
        recorder.0.lock().unwrap().push((
            at,
            key.to_owned(),
            meerkat_core::protected_section::effect_recorded(),
        ));
        (at == site && selected && first).then_some(Hold {
            owner_change,
            duration: HOLD,
        })
    }));
    seen
}

pub(in crate::private_originals) async fn clear_probe() {
    tokio::task::spawn_blocking(crate::source_hold_probe::clear)
        .await
        .unwrap();
}

/// A first send whose source is held at its input preparation keeps its
/// shipped outcome (`NativeOutcomeUnavailable`, nothing admitted), never a
/// "not now": its bind had already issued the controller child grant, and
/// the mark at the check names it.
///
/// RED with the first-binding marks dropped: the mark there is unset.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_send_held_at_input_preparation_keeps_its_shipped_refusal() {
    let fixture = FirstFixture::new(true).await;
    let seen = hold_at(Site::OriginalPrepare);
    let result = fixture.submit().await;
    clear_probe().await;
    let inputs = fixture.inputs().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(
        matches!(
            result,
            Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)
        ),
        "the shipped refusal: {}",
        submission_context(&result)
    );
    assert_eq!(inputs, Ok(0), "nothing was admitted");
    assert!(seen.at(Site::OriginalNotNow).is_empty(), "never not now");
    assert_eq!(
        seen.at(Site::OriginalPrepare)
            .first()
            .and_then(|(_, mark)| *mark),
        Some("original.controller_grant"),
        "the check runs after the first send's grant issuance"
    );
}

/// A first send whose source is held at its admitted-custody check waits
/// there as shipped and is admitted once; the mark there names its first
/// binding write, and no "not now" escapes.
///
/// RED with the first-binding marks dropped: the mark there is unset.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_send_held_at_the_admitted_check_waits_as_shipped_and_is_admitted_once() {
    let fixture = FirstFixture::new(true).await;
    let seen = hold_at(Site::OriginalAdmitted);
    let result = fixture.submit().await;
    let completion = fixture.finish(&result).await;
    clear_probe().await;
    let inputs = fixture.inputs().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(
        completion.is_ok(),
        "{completion:?}: {}",
        submission_context(&result)
    );
    assert_eq!(inputs, Ok(1), "admitted once");
    assert!(seen.at(Site::OriginalNotNow).is_empty(), "never not now");
    let admitted = seen.at(Site::OriginalAdmitted);
    assert_eq!(admitted.len(), 1, "one bind: the check waited in place");
    assert_eq!(
        admitted[0].1,
        Some("original.controller_grant"),
        "the check runs after the first send's grant issuance"
    );
}

/// An original already bound by a first send that ended without admission
/// is sent again while its source is held at the admitted-custody check: the
/// check answers "not now" (the source is held), with nothing written by
/// this attempt, the send waits outside the identity's lock, binds again and
/// is admitted once.
///
/// RED on the base: the check waits in place, inside the bind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bound_original_sent_again_while_its_source_is_held_is_not_now_then_admitted_once() {
    let fixture = FirstFixture::new(true).await;
    let first_seen = hold_at(Site::OriginalPrepare);
    let first = fixture.submit().await;
    clear_probe().await;
    let seen = hold_at(Site::OriginalAdmitted);
    let second = fixture.submit().await;
    let completion = fixture.finish(&second).await;
    clear_probe().await;
    let inputs = fixture.inputs().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(
        matches!(
            first,
            Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)
        ),
        "the first send ended without admission: {}",
        submission_context(&first)
    );
    assert!(first_seen.at(Site::OriginalNotNow).is_empty());
    assert_eq!(
        seen.at(Site::OriginalNotNow),
        vec![("source_held".to_string(), None)],
        "the bound original's check answered not now, once"
    );
    let admitted = seen.at(Site::OriginalAdmitted);
    assert_eq!(
        admitted.len(),
        2,
        "bound again after the wait: {admitted:?}"
    );
    assert_eq!(admitted[0].1, None, "no durable effect before the not now");
    assert!(
        completion.is_ok(),
        "{completion:?}: {}",
        submission_context(&second)
    );
    assert_eq!(inputs, Ok(1), "admitted once");
}

/// A first send whose controller child was already issued (an earlier bind's
/// issuance committed, its binding record did not) issues nothing, saves
/// the binding, and keeps the shipped wait at its admitted-custody check:
/// the only mark there is the binding write's.
///
/// RED with the first-binding marks dropped: the mark there is unset.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_send_whose_grant_was_issued_marks_only_its_binding_write() {
    let fixture = FirstFixture::new(true).await;
    let issued = fixture.preissue_controller(false);
    let seen = hold_at(Site::OriginalAdmitted);
    let result = if issued.is_ok() {
        fixture.submit().await
    } else {
        Err("fixture issuance failed".into())
    };
    let completion = fixture.finish(&result).await;
    clear_probe().await;
    let inputs = fixture.inputs().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(issued.is_ok(), "fixture native issuance failed");
    assert!(
        completion.is_ok(),
        "{completion:?}: {}",
        submission_context(&result)
    );
    assert_eq!(inputs, Ok(1), "admitted once");
    assert!(seen.at(Site::OriginalNotNow).is_empty(), "never not now");
    assert_eq!(
        seen.at(Site::OriginalAdmitted)
            .first()
            .and_then(|(_, mark)| *mark),
        Some("original.first_binding"),
        "the grant was found, not issued: only the binding write marks"
    );
}

/// The write witness's decision point is placed where the shared binder
/// passes its admitted-custody check, and nowhere earlier: on a Find's first
/// send every store write of the bind attempt before that check sees no
/// decision (the first binding write among them, marked by the grant before
/// it), and every write after it sees exactly the check's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_send_records_its_decision_at_the_admitted_check_and_nowhere_earlier() {
    let fixture = FirstFixture::new(true).await;
    let _ = crate::write_witness::take();
    let result = fixture.submit().await;
    let completion = fixture.finish(&result).await;
    let writes: Vec<_> = crate::write_witness::take()
        .into_iter()
        .filter(|write| write.section == ProtectedSection::GovernedBind)
        .collect();
    let inputs = fixture.inputs().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(
        completion.is_ok(),
        "{completion:?}: {}",
        submission_context(&result)
    );
    assert_eq!(inputs, Ok(1), "admitted once");
    let decided = writes
        .iter()
        .position(|write| write.decided.is_some())
        .unwrap_or(writes.len());
    let (before, after) = writes.split_at(decided);
    assert!(
        before
            .iter()
            .any(|write| write.write == "toolkit_store.compare_and_swap"
                && write.effect == Some("original.controller_grant")),
        "the first binding write precedes the decision point: {writes:?}"
    );
    assert!(
        !after.is_empty()
            && after
                .iter()
                .all(|write| write.decided == Some("original.admitted")),
        "every write after the admitted-custody check sees that decision: {writes:?}"
    );
}

/// A first send whose source is held inside an owner change at its
/// admitted-custody check waits there as shipped, on the check's slow path
/// (it waits out the owner change, then reads the collection again), and is
/// admitted once; no "not now" escapes. Those two waits are baseline sites
/// inside the bind (`original_ingress.admitted_guard.change` and
/// `original_ingress.admitted_guard.recheck`, owner B).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_send_held_inside_an_owner_change_at_the_admitted_check_waits_as_shipped() {
    let fixture = FirstFixture::new(true).await;
    let seen = hold_in_change_at(Site::OriginalAdmitted);
    let result = fixture.submit().await;
    let completion = fixture.finish(&result).await;
    clear_probe().await;
    let inputs = fixture.inputs().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(
        completion.is_ok(),
        "{completion:?}: {}",
        submission_context(&result)
    );
    assert_eq!(inputs, Ok(1), "admitted once");
    assert!(seen.at(Site::OriginalNotNow).is_empty(), "never not now");
    let admitted = seen.at(Site::OriginalAdmitted);
    assert_eq!(admitted.len(), 1, "one bind: the check waited in place");
    assert_eq!(
        admitted[0].1,
        Some("original.controller_grant"),
        "the check runs after the first send's grant issuance"
    );
}
