//! The release of the schedules a launch keeps under a mob's id, as the
//! runtime reaches it when its mob set creates a mob under that id.
//!
//! A mob created under the id of an earlier mob must find none of that
//! mob's schedules. The runtime does not own a launch's schedule service:
//! the composition that builds the service does. So the launch carries a
//! slot ([`MobSchedulesSlot`]) with three states the runtime reads at such
//! a create:
//!
//! - nothing declared: the launch has no schedules, and nothing is to be
//!   released;
//! - declared and not bound: the launch has schedules and the composition
//!   has not yet handed their release over, so the create is refused, for
//!   the new mob would find what an earlier mob of its id left;
//! - bound: the release is asked.
//!
//! The declaration is made before the runtime is built
//! ([`crate::mob_handle_runtime::MobBootstrapSpec::with_mob_schedules_declared`]),
//! the binding once the composition's schedule service knows the runtime's
//! mobs ([`MobSchedulesSlot::bind`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;

/// Ends the schedules a launch keeps under a mob's id.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub(crate) trait MobSchedulesRelease: Send + Sync {
    /// End every schedule kept under the mob `mob_id`: the answer is how
    /// many ended. Asked again it ends what is left. An error means some
    /// may be left; its text is for the host's log, not for a caller.
    async fn release_mob(&self, mob_id: &meerkat_mob::MobId) -> Result<usize, String>;
}

/// Where a launch's release of a mob's schedules stands, at one read.
pub(crate) enum MobSchedulesStanding {
    /// The launch declared no schedules: nothing is to be released.
    NotDeclared,
    /// The launch declared schedules and their release is not bound yet.
    Owed,
    /// The release is bound.
    Bound(Arc<dyn MobSchedulesRelease>),
}

/// The late-bound release of a mob's schedules, shared by a launch's spec,
/// its runtime and every clone of either.
#[derive(Clone, Default)]
pub(crate) struct MobSchedulesSlot {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    declared: AtomicBool,
    release: OnceLock<Arc<dyn MobSchedulesRelease>>,
}

impl MobSchedulesSlot {
    /// The launch has schedules: from here on a create that would need
    /// their release waits for [`Self::bind`], refused meanwhile.
    pub(crate) fn declare(&self) {
        self.inner.declared.store(true, Ordering::SeqCst);
    }

    /// Hand the release over. The first word stands: a launch has one
    /// schedule service. A binding declares the schedules too.
    pub(crate) fn bind(&self, release: Arc<dyn MobSchedulesRelease>) {
        self.declare();
        let _ = self.inner.release.set(release);
    }

    /// Where the release stands now.
    pub(crate) fn standing(&self) -> MobSchedulesStanding {
        match self.inner.release.get() {
            Some(release) => MobSchedulesStanding::Bound(Arc::clone(release)),
            None if self.inner.declared.load(Ordering::SeqCst) => MobSchedulesStanding::Owed,
            None => MobSchedulesStanding::NotDeclared,
        }
    }
}
