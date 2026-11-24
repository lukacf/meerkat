//! The release of the agent memory a launch keeps for a mob it hosts beside
//! its launch mob, as the runtime reaches it when its mob set creates a mob
//! under that mob's id and after its mob set destroyed the mob.
//!
//! A mob created under the id of an earlier mob must find nothing of that
//! mob's memory, and a destroyed mob's memory goes with the mob. The
//! runtime does not own a launch's memory: the launch does, and it hands
//! its memory over for the mobs it hosts after the runtime is built and its
//! launch mob's agents are brought up. So the launch carries a slot
//! ([`MobMemorySlot`]) with three states the runtime reads at such a
//! create:
//!
//! - nothing declared: the launch keeps no memory for the mobs it hosts,
//!   and nothing is to be released;
//! - declared and not bound: the launch keeps such memory and has not
//!   handed it over yet, so the create is refused, for the new mob would
//!   later be given what an earlier mob of its id left;
//! - bound: the release is asked.
//!
//! The declaration is made before the runtime is built
//! ([`crate::mob_handle_runtime::MobBootstrapSpec::with_hosted_mob_memory_declared`]),
//! the binding when the launch hands its memory over
//! ([`MobMemorySlot::bind`]).
//!
//! Which mob's memory a release removes is said by the typed id of the mob
//! the runtime's mob set created or destroyed, and by nothing else: no
//! name, label or id an agent supplies reaches it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// Releases the agent memory a launch keeps for the mobs it hosts beside
/// its launch mob.
pub(crate) trait MobMemoryRelease: Send + Sync {
    /// Release everything the launch's memory keeps for the mob `mob_id`:
    /// from here on nothing that served that mob reads or writes it, and a
    /// mob created under the id starts with none of it. Asked again it
    /// changes nothing. An error means something of it is left on disk; its
    /// text is for the host's log, not for a caller.
    fn release_mob(&self, mob_id: &meerkat_mob::MobId) -> Result<(), String>;

    /// Finish the releases an earlier process began and did not end: what
    /// a release took out of its mob's name and could not remove is removed
    /// now. Nothing else is touched: the memory of a mob no release was
    /// asked for stays, whatever mobs the runtime holds.
    fn finish_earlier_releases(&self);
}

/// Where a launch's release of its hosted mobs' memory stands, at one read.
pub(crate) enum MobMemoryStanding {
    /// The launch declared no memory for the mobs it hosts: nothing is to
    /// be released.
    NotDeclared,
    /// The launch declared such memory and its release is not bound yet.
    Owed,
    /// The release is bound.
    Bound(Arc<dyn MobMemoryRelease>),
}

/// The late-bound release of the memory a launch keeps for the mobs it
/// hosts, shared by a launch's spec, its runtime and every clone of either.
#[derive(Clone, Default)]
pub(crate) struct MobMemorySlot {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    declared: AtomicBool,
    release: OnceLock<Arc<dyn MobMemoryRelease>>,
}

impl MobMemorySlot {
    /// The launch keeps memory for the mobs it hosts: from here on a create
    /// that would need its release waits for [`Self::bind`], refused
    /// meanwhile.
    pub(crate) fn declare(&self) {
        self.inner.declared.store(true, Ordering::SeqCst);
    }

    /// Hand the release over. The first word stands: a launch has one
    /// memory. A binding declares the memory too, and it is the launch's
    /// start for the releases an earlier process left unfinished: they are
    /// finished here, before the release stands as bound, so before any
    /// create or destroy can ask it (a create that comes meanwhile meets a
    /// memory that is declared and not bound, and is refused).
    pub(crate) fn bind(&self, release: Arc<dyn MobMemoryRelease>) {
        self.declare();
        if self.inner.release.get().is_some() {
            return;
        }
        release.finish_earlier_releases();
        // Two bindings at once both finish what was left, which finds
        // nothing the second time; the first to be set stands.
        let _ = self.inner.release.set(release);
    }

    /// Where the release stands now.
    pub(crate) fn standing(&self) -> MobMemoryStanding {
        match self.inner.release.get() {
            Some(release) => MobMemoryStanding::Bound(Arc::clone(release)),
            None if self.inner.declared.load(Ordering::SeqCst) => MobMemoryStanding::Owed,
            None => MobMemoryStanding::NotDeclared,
        }
    }
}

/// The bundled store releases a hosted mob's memory by its own path
/// function: the one directory of that mob below the launch's memory root.
///
/// A mob whose id the store has no directory for was never given a store
/// by it (the store refuses to make one for the same id): nothing is kept
/// under that id, so the release answers done and touches nothing.
impl MobMemoryRelease for crate::memory::sqlite_store::SqliteAgentMemoryStore {
    fn release_mob(&self, mob_id: &meerkat_mob::MobId) -> Result<(), String> {
        if self.root_of_hosted_mob(mob_id).is_err() {
            return Ok(());
        }
        self.release_hosted_mob(mob_id)
            .map_err(|error| error.to_string())
    }

    fn finish_earlier_releases(&self) {
        self.finish_releases_of_hosted_mobs();
    }
}
