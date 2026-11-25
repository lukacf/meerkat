//! The release of what a launch's announcements keep under a mob's id, as
//! the runtime reaches it when its mob set creates a mob under that id.
//!
//! A mob created under the id of an earlier mob must find nothing of that
//! mob with the announcements: no source, no announcement, no
//! subscription that names it. The runtime does not own a launch's
//! announcements: the composition that attaches their owner does, and it
//! attaches the owner after the runtime is built and its members are
//! brought up. So the launch carries a slot ([`MobAnnouncementsSlot`])
//! with three states the runtime reads at such a create:
//!
//! - nothing declared: the launch composes no announcements, and nothing
//!   is to be released;
//! - declared and not bound: the launch composes announcements and their
//!   owner is not attached yet, so the create is refused, for the new mob
//!   would find what an earlier mob of its id left;
//! - bound: the release is asked.
//!
//! The declaration is made before the runtime is built
//! ([`crate::mob_handle_runtime::MobBootstrapSpec::with_mob_announcements_declared`]),
//! the binding when the announcements' owner is attached
//! ([`MobAnnouncementsSlot::bind`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;

/// Releases what a launch's announcements keep under a mob's id.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub(crate) trait MobAnnouncementsRelease: Send + Sync {
    /// Release everything the announcements keep under the mob `mob_id`.
    /// Asked again it changes nothing. An error means something may be
    /// left; its text is for the host's log, not for a caller.
    async fn release_mob(&self, mob_id: &meerkat_mob::MobId) -> Result<(), String>;
}

/// Where a launch's release of a mob's announcements stands, at one read.
pub(crate) enum MobAnnouncementsStanding {
    /// The launch declared no announcements: nothing is to be released.
    NotDeclared,
    /// The launch declared announcements and their release is not bound
    /// yet.
    Owed,
    /// The release is bound.
    Bound(Arc<dyn MobAnnouncementsRelease>),
}

/// The late-bound release of what the announcements keep under a mob's
/// id, shared by a launch's spec, its runtime and every clone of either.
#[derive(Clone, Default)]
pub(crate) struct MobAnnouncementsSlot {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    declared: AtomicBool,
    release: OnceLock<Arc<dyn MobAnnouncementsRelease>>,
}

impl MobAnnouncementsSlot {
    /// The launch composes announcements: from here on a create that
    /// would need their release waits for [`Self::bind`], refused
    /// meanwhile.
    pub(crate) fn declare(&self) {
        self.inner.declared.store(true, Ordering::SeqCst);
    }

    /// Hand the release over. The first word stands: a runtime has one
    /// announcements' owner. A binding declares the announcements too.
    pub(crate) fn bind(&self, release: Arc<dyn MobAnnouncementsRelease>) {
        self.declare();
        let _ = self.inner.release.set(release);
    }

    /// Where the release stands now.
    pub(crate) fn standing(&self) -> MobAnnouncementsStanding {
        match self.inner.release.get() {
            Some(release) => MobAnnouncementsStanding::Bound(Arc::clone(release)),
            None if self.inner.declared.load(Ordering::SeqCst) => MobAnnouncementsStanding::Owed,
            None => MobAnnouncementsStanding::NotDeclared,
        }
    }
}
