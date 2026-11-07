//! Test-only observation on one actual PrivateOriginals owner and source key.
//! No task-local/global selector and no admission or lifecycle control.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;
use toolkit_store::StoreKey;

pub(crate) struct MetadataRecoveryProbe {
    source: StoreKey,
    pub reads: AtomicUsize,
    pub polls: AtomicUsize,
    pub pages: AtomicUsize,
    pub wraps: AtomicUsize,
    pub drain: AtomicBool,
    pub page_boundary: AtomicBool,
    pause_page: AtomicBool,
    page_released: AtomicBool,
    released: AtomicBool,
    wake: Notify,
    interest: bool,
    pub interest_boundary: AtomicBool,
    interest_released: AtomicBool,
    pub writer_refusals: AtomicUsize,
    pub interest_unavailable: AtomicUsize,
    pub interest_restored: AtomicUsize,
    replay_released: AtomicBool,
    pub ready: AtomicBool,
    pub custody_rounds: AtomicUsize,
    pub custody_idle_rounds: AtomicUsize,
}
impl MetadataRecoveryProbe {
    pub fn new(source: StoreKey) -> Self {
        Self {
            source,
            reads: AtomicUsize::new(0),
            polls: AtomicUsize::new(0),
            pages: AtomicUsize::new(0),
            wraps: AtomicUsize::new(0),
            drain: AtomicBool::new(false),
            page_boundary: AtomicBool::new(false),
            pause_page: AtomicBool::new(false),
            page_released: AtomicBool::new(false),
            released: AtomicBool::new(false),
            wake: Notify::new(),
            interest: false,
            interest_boundary: AtomicBool::new(false),
            interest_released: AtomicBool::new(false),
            writer_refusals: AtomicUsize::new(0),
            interest_unavailable: AtomicUsize::new(0),
            interest_restored: AtomicUsize::new(0),
            replay_released: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            custody_rounds: AtomicUsize::new(0),
            custody_idle_rounds: AtomicUsize::new(0),
        }
    }
    pub async fn after_read(&self, source: &StoreKey) {
        if source != &self.source {
            return;
        }
        self.reads.fetch_add(1, Ordering::SeqCst);
        loop {
            let changed = self.wake.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.released.load(Ordering::SeqCst) {
                return;
            }
            changed.await;
        }
    }
    pub fn polling(&self) {
        if self.reads.load(Ordering::SeqCst) != 0 && !self.released.load(Ordering::SeqCst) {
            self.polls.fetch_add(1, Ordering::SeqCst);
        }
    }
    pub fn paging(&self, at_start: bool) {
        self.pages.fetch_add(1, Ordering::SeqCst);
        if at_start {
            self.wraps.fetch_add(1, Ordering::SeqCst);
        }
    }
    pub fn arm_page_boundary(&self) {
        self.pause_page.store(true, Ordering::SeqCst);
    }
    pub async fn after_poll(&self) {
        if !self.pause_page.load(Ordering::SeqCst) {
            return;
        }
        self.page_boundary.store(true, Ordering::SeqCst);
        loop {
            let changed = self.wake.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.page_released.load(Ordering::SeqCst) {
                return;
            }
            changed.await;
        }
    }
    pub fn release_page_boundary(&self) {
        self.page_released.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }
    pub fn draining(&self) {
        self.drain.store(true, Ordering::SeqCst);
    }
    pub fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.interest_released.store(true, Ordering::SeqCst);
        self.replay_released.store(true, Ordering::SeqCst);
        self.release_page_boundary();
    }
    pub fn for_interest(source: StoreKey) -> Self {
        let mut probe = Self::new(source);
        probe.interest = true;
        probe.released.store(true, Ordering::SeqCst); // Metadata reads are not held in this case.
        probe
    }
    pub async fn wait_for(&self, condition: impl Fn(&Self) -> bool) {
        loop {
            let changed = self.wake.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if condition(self) {
                return;
            }
            changed.await;
        }
    }
    pub async fn before_interest_native(&self, source: &StoreKey) {
        if !self.interest
            || source != &self.source
            || self.interest_boundary.swap(true, Ordering::SeqCst)
        {
            return;
        }
        self.wake.notify_waiters();
        self.wait_for(|probe| probe.interest_released.load(Ordering::SeqCst))
            .await;
    }
    pub fn release_interest(&self) {
        self.interest_released.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }
    pub fn writer_refused(&self, source: &StoreKey) {
        if !self.interest || source != &self.source {
            return;
        }
        self.writer_refusals.fetch_add(1, Ordering::SeqCst);
        self.wake.notify_waiters();
    }
    pub async fn observed_interest(
        &self,
        source: &StoreKey,
        outcome: &Result<
            super::setup_workflow::WorkflowInterestRecovery,
            super::PrivateOriginalError,
        >,
    ) {
        if !self.interest || source != &self.source {
            return;
        }
        match outcome {
            Err(super::PrivateOriginalError::Unavailable) => {
                self.interest_unavailable.fetch_add(1, Ordering::SeqCst);
            }
            Ok(super::setup_workflow::WorkflowInterestRecovery::Restored { memberships }) => {
                assert_eq!(*memberships, 1, "the selected actual worker result");
                let prior = self.interest_restored.fetch_add(1, Ordering::SeqCst);
                self.wake.notify_waiters();
                // Hold only the already-returned first result, outside native
                // custody. The observer captures exact attachment identities
                // before this same worker's normal next sweep.
                if prior == 0 {
                    self.wait_for(|probe| probe.replay_released.load(Ordering::SeqCst))
                        .await;
                }
            }
            _ => (),
        }
        self.wake.notify_waiters();
    }
    pub fn release_replay(&self) {
        self.replay_released.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }
    pub fn custody_round(&self, idle: bool) {
        if self.interest && self.ready.load(Ordering::SeqCst) {
            self.custody_rounds.fetch_add(1, Ordering::SeqCst);
            if idle {
                self.custody_idle_rounds.fetch_add(1, Ordering::SeqCst);
            }
            self.wake.notify_waiters();
        }
    }
}
