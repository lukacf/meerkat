//! One diagnostic hold of the real Accepted Original promotion. These values
//! observe existing custody; they cannot admit, publish, or fabricate an input.
use super::*;
use tokio::sync::oneshot;

#[derive(Clone)]
pub struct OriginalPromotionIdentity {
    pub original: PrivateOriginalRef,
    pub session: meerkat_core::SessionId,
    pub input_id: meerkat_core::lifecycle::InputId,
}
pub struct OriginalPromotionWrite {
    pub identity: OriginalPromotionIdentity,
    pub before: StoredRecord,
    pub after: StoredRecord,
}
/// Dropping the test's only permit releases the real worker, including when a
/// timeout or panic drops the diagnostic future. It never aborts that worker.
pub struct OriginalPromotionProbe {
    entered: Option<oneshot::Receiver<Result<OriginalPromotionIdentity, &'static str>>>,
    release: Option<oneshot::Sender<()>>,
    completed: Option<oneshot::Receiver<OriginalPromotionWrite>>,
}
impl OriginalPromotionProbe {
    pub async fn entered(&mut self) -> Result<OriginalPromotionIdentity, &'static str> {
        self.entered
            .take()
            .ok_or("promotion entry already observed")?
            .await
            .map_err(|_| "actual promotion owner ended before entry")?
    }
    pub fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
    pub async fn completed(&mut self) -> Result<OriginalPromotionWrite, &'static str> {
        self.completed
            .take()
            .ok_or("promotion completion already observed")?
            .await
            .map_err(|_| "no actual validated promotion CAS completed")
    }
}
impl Drop for OriginalPromotionProbe {
    fn drop(&mut self) {
        self.release();
    }
}
struct Wait {
    entered: oneshot::Sender<Result<OriginalPromotionIdentity, &'static str>>,
    release: oneshot::Receiver<()>,
    completed: oneshot::Sender<OriginalPromotionWrite>,
}
#[derive(Default)]
pub(super) struct Slot {
    armed: bool,
    wait: Option<Wait>,
    completion: Option<Completion>,
}
pub(super) struct Completion {
    pending: Arc<governed_submit::PendingOriginalAdmission>,
    identity: OriginalPromotionIdentity,
    completed: oneshot::Sender<OriginalPromotionWrite>,
}
impl Completion {
    pub(super) fn complete(self, before: StoredRecord, after: StoredRecord) {
        // Called only after the actual saved attempt validated and writer drop.
        let _ = self.completed.send(OriginalPromotionWrite {
            identity: self.identity,
            before,
            after,
        });
    }
}
impl PrivateOriginals {
    /// A one-shot test observation of the next actual Accepted Original, never
    /// a Helper. The caller must compare the returned real identity with its
    /// own submitted source and native input before releasing the permit.
    pub fn hold_next_original_promotion_for_test(&self) -> OriginalPromotionProbe {
        let (entered, entered_rx) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let (completed, completed_rx) = oneshot::channel();
        let mut slot = self.custody_recovery.promotion_probe.lock().unwrap();
        assert!(
            !slot.armed,
            "one promotion diagnostic per actual source owner"
        );
        slot.armed = true;
        slot.wait = Some(Wait {
            entered,
            release: release_rx,
            completed,
        });
        OriginalPromotionProbe {
            entered: Some(entered_rx),
            release: Some(release),
            completed: Some(completed_rx),
        }
    }
}
pub(super) async fn pause(source: &PrivateOriginals, entry: &Entry) {
    let Entry::Original(pending) = entry else {
        return;
    };
    let wait = source
        .custody_recovery
        .promotion_probe
        .lock()
        .unwrap()
        .wait
        .take();
    let Some(wait) = wait else {
        return;
    };
    // These are existing local custody facts. No store/native/authority query
    // is added, and no lock remains held over the diagnostic wait.
    let identity = match pending.held.upgrade() {
        Some(held) if entry.acknowledged() => OriginalPromotionIdentity {
            original: held.reference.clone(),
            session: pending.session.clone(),
            input_id: pending.input.id().clone(),
        },
        _ => {
            let _ = wait
                .entered
                .send(Err("expected acknowledged real Original"));
            return;
        }
    };
    let _ = wait.entered.send(Ok(identity.clone()));
    let _ = wait.release.await;
    let completion = Completion {
        pending: pending.clone(),
        identity,
        completed: wait.completed,
    };
    assert!(
        source
            .custody_recovery
            .promotion_probe
            .lock()
            .unwrap()
            .completion
            .replace(completion)
            .is_none()
    );
}
pub(super) fn take_completion(
    source: &PrivateOriginals,
    entry: &Entry,
    change: CustodyChange,
) -> Option<Completion> {
    let Entry::Original(pending) = entry else {
        return None;
    };
    if !matches!(change, CustodyChange::Promote) {
        return None;
    }
    let mut slot = source.custody_recovery.promotion_probe.lock().unwrap();
    if slot
        .completion
        .as_ref()
        .is_some_and(|completion| Arc::ptr_eq(&completion.pending, pending))
    {
        slot.completion.take()
    } else {
        None
    }
}
