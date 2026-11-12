//! Where the identity runtime keeps the family of changes and reads of its
//! mob's long-term agents (`identity_creation::IdentityFamily`).
//!
//! A launch attaches the family when its identity runtime is installed.
//! Every surface reads it here at each call: the stdio gateway's and the
//! Console's `meta/identity/*` methods, and the agents' identity tools. A
//! launch whose identity policy is present and enabled serves the family's
//! changes ([`IdentityRuntime::identity_changes_enabled`]): the agents'
//! identity tools are mounted and the Console offers changes exactly then.
//! A launch without one serves the family's reads and refuses every change
//! `change_denied`. The family is detached when the mob stops, with the
//! identity authority.

use std::sync::Arc;

use super::IdentityRuntime;
use crate::identity_creation::{IdentityFamily, PeerVisibility};
use crate::identity_first::AgentIdentity;

impl IdentityRuntime {
    /// Attach `family`, of a launch whose identity policy is present and
    /// enabled: every surface of the mob calls it from now on, and it
    /// serves changes.
    pub fn attach_identity_family(&self, family: Arc<dyn IdentityFamily>) {
        self.identity_family.attach(family);
    }

    /// Attach `family`, of a launch without an enabled identity policy:
    /// every surface of the mob reads through it, and it refuses every
    /// change.
    pub fn attach_identity_reads(&self, family: Arc<dyn IdentityFamily>) {
        self.identity_family.attach_reads(family);
    }

    /// Attach the launch's own family, which serves changes when
    /// `changes_enabled`.
    pub(crate) fn attach_identity_gate(
        &self,
        gate: Arc<crate::identity_creation::IdentityGate>,
        changes_enabled: bool,
    ) {
        self.identity_family.attach_gate(gate, changes_enabled);
    }

    /// The mob this runtime serves, as the handle its composition gave
    /// its services: one that does not keep the mob owned. The family's
    /// background work (a change carried out after a person approved it)
    /// acts through it.
    pub(crate) fn family_mob_handle(&self) -> Option<meerkat_mob::MobHandle> {
        self.runtime_services.mob_handle()
    }

    /// Whether the host's roster lists `identity` while the agent is served
    /// from its record, so the roster's entry is not applied.
    ///
    /// STAND-IN, to be deleted when the roster pass is merged: the roster
    /// pass owns this fact and brings a method of this name and signature,
    /// which the family's reads then call unchanged. Until then no roster
    /// pass says so, and the answer is `false` for every agent.
    #[allow(
        clippy::unused_async,
        reason = "the signature of the roster pass's own method, which awaits"
    )]
    pub(crate) async fn roster_listing_superseded(&self, _identity: &AgentIdentity) -> bool {
        false
    }

    /// Whether the record of the agent `name` is its row in the mob's
    /// identity store: an agent created at run time, or adopted. `false`
    /// for an agent its host configures, for a name no agent holds, and for
    /// a record this runtime cannot read: a run-time change of such an
    /// agent is refused, so a surface offers none.
    pub(crate) async fn record_is_store_owned(&self, name: &str) -> bool {
        let Ok(identity) = AgentIdentity::parse(name) else {
            return false;
        };
        matches!(
            self.spec_record(&identity).await,
            Ok(super::created::SpecRecord::Store)
        )
    }

    /// The attached family as a caller that sees the agents `seen` admits
    /// calls it: a peer that caller may not see is answered as a name no
    /// agent holds.
    pub fn identity_family_seen_by(
        &self,
        seen: Arc<dyn PeerVisibility>,
    ) -> Option<Arc<dyn IdentityFamily>> {
        self.identity_family.seen_by(seen)
    }

    /// The attached family: `None` before the launch attached one, and once
    /// the mob stopped.
    pub fn identity_family(&self) -> Option<Arc<dyn IdentityFamily>> {
        self.identity_family.get()
    }

    /// Whether this launch serves changes of its agents: true exactly when
    /// its identity policy is present and enabled. The agents' identity
    /// tools are mounted, the Console offers changes and a decision is
    /// driven exactly when it is true; the family's reads are served either
    /// way.
    pub fn identity_changes_enabled(&self) -> bool {
        self.identity_family.changes_enabled()
    }

    /// Detach the family: the mob stopped.
    pub(crate) fn detach_identity_family(&self) {
        self.identity_family.clear();
    }

    /// A change of the family wrote the record of the agent `name` (a
    /// create, an update, an archive, an unarchive, an adoption, a
    /// removal): serve the agent from its record from now on. Its
    /// registration is read again from its record, made when this runtime
    /// had none, and forgotten when the record is gone. It reads one
    /// record, sends the mob no command and wakes nothing; a read that
    /// fails leaves the registration to the agent's next door, which reads
    /// the record again.
    pub(crate) async fn note_agent_record_changed(&self, name: &str) {
        let Ok(identity) = AgentIdentity::parse(name) else {
            return;
        };
        if let Err(error) = self.register_from_row(&identity).await {
            tracing::debug!(
                identity = %identity,
                %error,
                "the registration of a changed agent is left to its next door"
            );
        }
    }
}
