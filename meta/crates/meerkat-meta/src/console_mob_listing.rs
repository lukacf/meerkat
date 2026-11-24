//! Which mobs of a gateway a console lists, and where each sits.
//!
//! A gateway's agent mob state holds its own mob (the primary mob) and every
//! mob created beside it. A console lists a mob beside the primary one only
//! from the mob's creation record ([`meerkat_mob::MobProvenance`]), the fact
//! its create wrote with its first event and that never changes:
//!
//! - a record that names a member (`Member`, `OperatorMember`): the mob is
//!   nested under that member's mob, and that member is its creator, also
//!   after the member was reset or left;
//! - `Host`: a mob of the host's own, served by its own id beside this
//!   one. It is no group of another mob: it is not listed and none of its
//!   members is, and neither is a mob one of its members created (its
//!   parent is not listed);
//! - a record that does not say which member created the mob (`Ambiguous`:
//!   several rosters named the session that created it; or a kind this
//!   release does not know): who the mob is private to cannot be read
//!   ([`crate::private_owners::creation_record_owner_undecidable`]), so the
//!   mob is a top-level mob with no creator named whose members are access
//!   admins' alone;
//! - any other record (`Session`, `OperatorSession`, `Unrecorded`): the mob
//!   is not listed and none of its members is.
//!
//! Whether a mob is a temporary council's is not read here: that is the
//! council's own record, which the console aggregator asks
//! (`console_aggregator::council`), and a participant is served by no read
//! on its answer. (A council creates its mob for a fresh session, so the
//! record of such a mob is `Session` and this listing places it nowhere
//! either.)
//!
//! No label a member or a spawn can write places a mob or a member, and a
//! mob's id says nothing about it, with two exceptions that list nothing: a
//! mob whose id holds the separator of the nested member form, and a mob
//! whose id an identity namespace of its runtime claims
//! ([`namespace_claims_mob`]: the id is one of the namespace's components).
//! Neither is listed, since its members have no console identity of their
//! own: in such a namespace, or for another reader of the runtime under a
//! shorter one, an identity composed for one of the mob's members can be
//! the identity of a member of the runtime's own mob. A mob whose recorded
//! parent is not listed
//! (it was destroyed, it was recreated under the same id, or it is not
//! listed itself) is not listed either.
//!
//! The record is read once per mob incarnation and kept
//! ([`ConsoleMobRecords`]): a read of the mob's store for a mob the gateway
//! already holds. Listing asks no actor, wakes no member and loads no mob.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use meerkat_core::service::MobIncarnationId;
use meerkat_mob::MobHandle;

use crate::mob_handle_runtime::MobRuntime;

/// What a mob's creation record says of who created it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CreationRecord {
    /// A member created it: `creator` (its public name) of the mob
    /// `parent_mob_id` at `parent_incarnation`.
    Member {
        parent_mob_id: String,
        parent_incarnation: MobIncarnationId,
        creator: String,
    },
    /// The host created it.
    Host,
    /// The record does not say which member created it, so who the mob is
    /// private to cannot be read.
    OwnerUndecidable,
    /// The record names neither a member nor the host.
    Unattributed,
}

impl CreationRecord {
    fn of(provenance: &meerkat_mob::MobProvenance) -> Self {
        use meerkat_mob::MobProvenance;
        if crate::private_owners::creation_record_owner_undecidable(provenance) {
            return Self::OwnerUndecidable;
        }
        match provenance {
            MobProvenance::Member {
                parent_mob_id,
                parent_incarnation,
                creator_identity,
                ..
            }
            | MobProvenance::OperatorMember {
                parent_mob_id,
                parent_incarnation,
                creator_identity,
                ..
            } => Self::Member {
                parent_mob_id: parent_mob_id.to_string(),
                parent_incarnation: *parent_incarnation,
                creator: crate::member_comms_id::runtime_alias_str(creator_identity.as_str())
                    .into_owned(),
            },
            MobProvenance::Host => Self::Host,
            _ => Self::Unattributed,
        }
    }
}

/// The creation record of each mob a runtime's consoles have read, kept per
/// mob incarnation and shared by every clone of the runtime, and the
/// identity namespaces console aggregators serve the runtime under.
///
/// The namespaces are noted on these records, which the clones of one
/// runtime share and no other runtime does: a host that builds two runtimes
/// around one mob holds two sets of records, and a namespace one of them is
/// served under is not known to the consoles of the other.
#[derive(Default)]
pub(crate) struct ConsoleMobRecords {
    read: Mutex<BTreeMap<String, (MobIncarnationId, CreationRecord)>>,
    /// Each namespace an aggregator serves the runtime under, with how many
    /// registrations name it. A mob one of them claims is listed by no
    /// reader of the runtime ([`Self::claims_mob`]).
    served_namespaces: Mutex<BTreeMap<String, usize>>,
    #[cfg(test)]
    store_reads: std::sync::atomic::AtomicUsize,
}

impl std::fmt::Debug for ConsoleMobRecords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleMobRecords").finish_non_exhaustive()
    }
}

impl ConsoleMobRecords {
    /// The record of `handle`'s mob: the kept one for its incarnation, else
    /// read from its store and kept. A record that cannot be read is
    /// `Unattributed` for this listing and read again by the next.
    async fn record(&self, mob_id: &str, handle: &MobHandle) -> CreationRecord {
        let incarnation = handle.incarnation();
        if let Some((kept_for, record)) = self
            .read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(mob_id)
            && *kept_for == incarnation
        {
            return record.clone();
        }
        #[cfg(test)]
        self.store_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match handle.provenance().await {
            Ok(provenance) => {
                let record = CreationRecord::of(&provenance);
                self.read
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(mob_id.to_string(), (incarnation, record.clone()));
                record
            }
            Err(error) => {
                tracing::debug!(mob_id, %error, "console mob listing: creation record unreadable");
                CreationRecord::Unattributed
            }
        }
    }

    /// Drop the records of mobs the gateway no longer holds.
    fn keep_only(&self, held: &[(String, MobHandle)]) {
        self.read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|mob_id, _| held.iter().any(|(held_id, _)| held_id == mob_id));
    }

    /// Whether the kept record of mob `mob_id` leaves its owner undecided.
    /// `false` for a mob no listing has read.
    pub(crate) fn owner_undecidable(&self, mob_id: &str) -> bool {
        self.read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(mob_id)
            .is_some_and(|(_, record)| *record == CreationRecord::OwnerUndecidable)
    }

    /// Note that a console aggregator serves the runtime under `namespace`
    /// from now on. The empty namespace is noted nowhere: it claims no mob.
    pub(crate) fn serve_under(&self, namespace: &str) {
        let namespace = namespace.trim().trim_matches('/');
        if namespace.is_empty() {
            return;
        }
        *self
            .served_namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(namespace.to_string())
            .or_default() += 1;
    }

    /// Note that one registration under `namespace` ended.
    pub(crate) fn stop_serving_under(&self, namespace: &str) {
        let namespace = namespace.trim().trim_matches('/');
        let mut served = self
            .served_namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(registrations) = served.get_mut(namespace) {
            *registrations = registrations.saturating_sub(1);
            if *registrations == 0 {
                served.remove(namespace);
            }
        }
    }

    /// Whether the mob `mob_id` has no console identity for a reader that
    /// names the runtime's members under `reader_namespace`: that namespace
    /// claims it ([`namespace_claims_mob`]), or one an aggregator serves
    /// the runtime under does. The second holds for every reader, the
    /// runtime's own console included, so that one identity never names a
    /// nested member for one reader and a member of the runtime's own mob
    /// for another.
    pub(crate) fn claims_mob(&self, reader_namespace: &str, mob_id: &str) -> bool {
        namespace_claims_mob(reader_namespace, mob_id)
            || self
                .served_namespaces
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .keys()
                .any(|namespace| namespace_claims_mob(namespace, mob_id))
    }

    /// How many creation records were read from a mob's store.
    #[cfg(test)]
    pub(crate) fn store_reads(&self) -> usize {
        self.store_reads.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// One mob a console lists beside the primary mob.
#[derive(Clone)]
pub(crate) struct ListedMob {
    pub(crate) mob_id: String,
    pub(crate) handle: MobHandle,
    /// The mob it is nested under; `None` for a top-level mob.
    pub(crate) parent_mob_id: Option<String>,
    /// The console identity of the member its creation record names (a bare
    /// name in the primary mob, the nested form in any other); `None` for a
    /// top-level mob.
    pub(crate) creator_identity: Option<String>,
    /// Whether its creation record leaves its owner undecided: its members
    /// are then listed to access admins alone.
    pub(crate) owner_undecidable: bool,
}

/// The mobs of a runtime's agent mob state beside its primary mob, as a
/// console reads them.
#[derive(Default)]
pub(crate) struct ConsoleMobListing {
    /// The mobs a console lists, each after the mob it is nested under.
    pub(crate) listed: Vec<ListedMob>,
    /// The mobs a console does not list: none of their members is listed,
    /// resolved or read, and a frame one of them wrote is a hidden member's.
    pub(crate) unlisted: Vec<(String, MobHandle)>,
}

/// Whether the identity namespace `namespace` claims the mob id `mob_id`:
/// the id is one of the namespace's components (the whole namespace, for one
/// of a single component). Under a namespace that starts with the mob's id,
/// an identity that starts with that id can be the namespaced identity of a
/// member of the runtime's own mob (`<namespace>/<name>`). Where the id is a
/// later component, a reader of the same runtime under the shorter namespace
/// before it would compose the identities of the mob's members
/// (`<shorter>/<mob>/<name>`) where a reader under this one composes those
/// of the runtime's own. So the mob's members have no console identity of
/// their own there. The namespace is read with no blank around it, and an
/// empty component, which a leading or trailing `/` leaves, names no mob.
/// The empty namespace claims no mob.
pub(crate) fn namespace_claims_mob(namespace: &str, mob_id: &str) -> bool {
    namespace
        .trim()
        .split('/')
        .any(|component| !component.is_empty() && component == mob_id)
}

/// Read which mobs `runtime`'s consoles list (see the module docs), for a
/// reader that names the runtime's members under `identity_namespace` (empty
/// for a console of the runtime's own). Reads the managed set as
/// `mob_handles_snapshot` hands it out and each mob's kept creation record.
pub(crate) async fn console_mob_listing(
    runtime: &MobRuntime,
    identity_namespace: &str,
) -> ConsoleMobListing {
    let Some(state) = runtime.agent_mob_mcp_state() else {
        return ConsoleMobListing::default();
    };
    let primary = runtime.handle();
    let primary_mob_id = primary.mob_id().to_string();
    let held: Vec<(String, MobHandle)> = Box::pin(state.mob_handles_snapshot())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(mob_id, handle)| (mob_id.to_string(), handle))
        .filter(|(mob_id, _)| *mob_id != primary_mob_id)
        .collect();
    let records = runtime.console_mob_records();
    records.keep_only(&held);
    let mut pending = Vec::with_capacity(held.len());
    for (mob_id, handle) in held {
        // No member of a mob whose id holds the separator of the nested
        // member form has a console identity, and none of a mob that the
        // reader's namespace, or a namespace the runtime is served under,
        // claims has one of its own.
        let record = if crate::console_member_ref::nested_member_identity(&mob_id, "member")
            .is_none()
            || records.claims_mob(identity_namespace, &mob_id)
        {
            CreationRecord::Unattributed
        } else {
            records.record(&mob_id, &handle).await
        };
        pending.push((mob_id, handle, record));
    }
    // A mob whose record leaves its creator undecided is attributed to no
    // mob the launch composes beside its launch mob: it is listed once, by
    // the launch mob's console, and by no console of such a mob.
    let beside_the_launch_mob = runtime
        .private_owners()
        .hosted_mobs()
        .holds(&primary_mob_id);
    // A mob is placed once its parent is: the primary mob, or a mob placed
    // before it at the incarnation its record names.
    let mut placed: BTreeMap<String, MobIncarnationId> =
        BTreeMap::from([(primary_mob_id.clone(), primary.incarnation())]);
    let mut listing = ConsoleMobListing::default();
    loop {
        let mut progressed = false;
        let mut rest = Vec::with_capacity(pending.len());
        for (mob_id, handle, record) in pending {
            let placement = match &record {
                // A mob whose record does not say who created it is listed at
                // the top level, for access admins alone.
                CreationRecord::OwnerUndecidable if !beside_the_launch_mob => Some((None, None)),
                CreationRecord::Member {
                    parent_mob_id,
                    parent_incarnation,
                    creator,
                } if placed.get(parent_mob_id) == Some(parent_incarnation) => {
                    let creator_identity = if *parent_mob_id == primary_mob_id {
                        Some(creator.clone())
                    } else {
                        crate::console_member_ref::nested_member_identity(parent_mob_id, creator)
                    };
                    Some((Some(parent_mob_id.clone()), creator_identity))
                }
                // A mob the host created is a mob of its own, served by its
                // own id: it is no group of this mob. A mob nested under it
                // names a parent that is never placed. A mob of an undecided
                // creator is no group of a mob composed beside the launch
                // mob.
                CreationRecord::Host
                | CreationRecord::Member { .. }
                | CreationRecord::OwnerUndecidable
                | CreationRecord::Unattributed => None,
            };
            match placement {
                Some((parent_mob_id, creator_identity)) => {
                    placed.insert(mob_id.clone(), handle.incarnation());
                    listing.listed.push(ListedMob {
                        mob_id,
                        handle,
                        parent_mob_id,
                        creator_identity,
                        owner_undecidable: record == CreationRecord::OwnerUndecidable,
                    });
                    progressed = true;
                }
                None => rest.push((mob_id, handle, record)),
            }
        }
        pending = rest;
        if !progressed {
            break;
        }
    }
    listing.unlisted = pending
        .into_iter()
        .map(|(mob_id, handle, _)| (mob_id, handle))
        .collect();
    listing
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A namespace is read as a console composes identities under it: with
    /// no blank and no `/` around it. However it is written, it claims the
    /// mob it claims and no other, and a namespace that is empty once read
    /// so claims no mob.
    #[test]
    fn a_namespace_claims_a_mob_however_the_namespace_is_written() {
        for namespace in [
            "ops",
            "ops/",
            "/ops",
            "/ops/",
            " ops ",
            " /ops/ ",
            "ops/east",
            " /ops/east/ ",
        ] {
            assert!(
                namespace_claims_mob(namespace, "ops"),
                "{namespace:?} claims the mob `ops`"
            );
            for other in ["op", "opsx", "ops/", " ops"] {
                assert!(
                    !namespace_claims_mob(namespace, other),
                    "{namespace:?} does not claim the mob {other:?}"
                );
            }
        }
        for namespace in ["", " ", "/", " / ", "//"] {
            assert!(
                !namespace_claims_mob(namespace, "ops"),
                "{namespace:?} claims no mob"
            );
        }
    }

    /// A namespace of several components claims the mob named as any of
    /// them: a second reader of the runtime, under a shorter namespace that
    /// this one starts with, would compose the identities of that mob's
    /// members where this one composes those of the runtime's own.
    #[test]
    fn a_namespace_claims_a_mob_named_as_any_of_its_components() {
        for mob_id in ["ops", "east", "desk"] {
            assert!(
                namespace_claims_mob(" /ops/east/desk/ ", mob_id),
                "the mob {mob_id:?} is claimed"
            );
        }
        for mob_id in ["eas", "east/desk", "ops/east", "north"] {
            assert!(
                !namespace_claims_mob("ops/east/desk", mob_id),
                "the mob {mob_id:?} is not claimed"
            );
        }
    }

    /// The registrations of a runtime are counted under the namespace as a
    /// console reads it, however each registration writes it: the mob the
    /// namespace claims stays claimed until the last of them ended, and no
    /// longer.
    #[test]
    fn a_registration_is_counted_under_its_namespace_however_it_is_written() {
        let records = ConsoleMobRecords::default();
        assert!(!records.claims_mob("", "ops"));
        records.serve_under(" /ops/ ");
        records.serve_under("ops");
        assert!(records.claims_mob("", "ops"));
        records.stop_serving_under("ops/");
        assert!(
            records.claims_mob("", "ops"),
            "one registration under the namespace is left"
        );
        records.stop_serving_under(" ops");
        assert!(
            !records.claims_mob("", "ops"),
            "no registration under the namespace is left"
        );
    }
}
