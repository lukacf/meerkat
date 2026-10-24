//! Deterministic faults for the startup restore's tuple checks.
//!
//! Compiled only for crate tests or the explicit `test-support` feature.
//! Production restores never consult this module: an unarmed restore reads
//! every roster entry as it is.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::error::MemberCustodyRestore;
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, Generation, MobId};
use crate::roster::RosterEntry;

/// The roster field a restore fault perturbs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreTupleFault {
    /// The entry's fence token, read as one higher.
    FenceToken,
    /// The entry's generation, read as one higher.
    Generation,
    /// The entry's runtime id, read as its next generation's.
    RuntimeId,
}

/// The armed faults, keyed by mob, member and startup restore: tests in one
/// process (in-process `cargo test`) arm faults for their own mobs without
/// seeing each other's, and a fault armed for one restore is never consumed
/// by the other (the cleanup delivery restore reads every roster entry before
/// the operation binding restore runs).
static RESTORE_TUPLE_FAULTS: Mutex<
    BTreeMap<(MobId, AgentIdentity, &'static str), RestoreTupleFault>,
> = Mutex::new(BTreeMap::new());

/// Arm a one-shot fault for `member` of `mob_id` at the startup `restore`:
/// the next time that restore reads the member's roster entry it sees `field`
/// perturbed, so its tuple check finds the entry diverged from the generated
/// authority. The fault is removed when it applies.
#[doc(hidden)]
pub fn fault_next_restore_tuple_for_test(
    mob_id: &MobId,
    member: &AgentIdentity,
    restore: MemberCustodyRestore,
    field: RestoreTupleFault,
) {
    if let Ok(mut armed) = RESTORE_TUPLE_FAULTS.lock() {
        armed.insert((mob_id.clone(), member.clone(), restore.as_str()), field);
    }
}

/// Apply the fault armed for `entry`'s member of `mob_id` at `restore`, if
/// any, and disarm it. Without an armed fault the entry is left as it is.
pub(crate) fn apply_restore_tuple_fault_for_test(
    mob_id: &MobId,
    restore: MemberCustodyRestore,
    entry: &mut RosterEntry,
) {
    let fault = RESTORE_TUPLE_FAULTS.lock().ok().and_then(|mut armed| {
        armed.remove(&(
            mob_id.clone(),
            entry.agent_identity.clone(),
            restore.as_str(),
        ))
    });
    match fault {
        Some(RestoreTupleFault::FenceToken) => {
            entry.fence_token = FenceToken::new(entry.fence_token.get().wrapping_add(1));
        }
        Some(RestoreTupleFault::Generation) => {
            entry.generation = Generation::new(entry.generation.get().wrapping_add(1));
        }
        Some(RestoreTupleFault::RuntimeId) => {
            entry.agent_runtime_id = AgentRuntimeId::new(
                entry.agent_runtime_id.identity.clone(),
                Generation::new(entry.agent_runtime_id.generation.get().wrapping_add(1)),
            );
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::event::MemberRef;
    use crate::ids::ProfileName;
    use crate::runtime_mode::MobRuntimeMode;

    fn entry(identity: &str) -> RosterEntry {
        RosterEntry {
            agent_identity: AgentIdentity::from(identity),
            generation: Generation::new(2),
            fence_token: FenceToken::new(7),
            agent_runtime_id: AgentRuntimeId::new(
                AgentIdentity::from(identity),
                Generation::new(2),
            ),
            role: ProfileName::from("worker"),
            member_ref: MemberRef::from_bridge_session_id(meerkat_core::types::SessionId::new()),
            runtime_mode: MobRuntimeMode::TurnDriven,
            peer_id: None,
            transport_public_key: None,
            wired_to: BTreeSet::new(),
            external_peer_specs: BTreeMap::new(),
            labels: BTreeMap::new(),
            kickoff: None,
            effective_profile_override: None,
            effective_model_override: None,
            spawned_by: None,
            fork_job: None,
            fork_source: None,
            fork_overlay: Default::default(),
            direct_member_fence: None,
        }
    }

    #[test]
    fn an_unarmed_restore_reads_the_entry_as_it_is() {
        let mob = MobId::from("restore-fault-unarmed");
        let original = entry("w-1");
        let mut read = original.clone();
        apply_restore_tuple_fault_for_test(&mob, MemberCustodyRestore::CleanupDelivery, &mut read);
        assert_eq!(read, original);
        apply_restore_tuple_fault_for_test(&mob, MemberCustodyRestore::StartupBinding, &mut read);
        assert_eq!(read, original);
    }

    #[test]
    fn an_armed_fault_perturbs_its_one_field_once() {
        let mob = MobId::from("restore-fault-fields");
        let original = entry("w-1");
        for field in [
            RestoreTupleFault::FenceToken,
            RestoreTupleFault::Generation,
            RestoreTupleFault::RuntimeId,
        ] {
            fault_next_restore_tuple_for_test(
                &mob,
                &original.agent_identity,
                MemberCustodyRestore::CleanupDelivery,
                field,
            );
            let mut read = original.clone();
            apply_restore_tuple_fault_for_test(
                &mob,
                MemberCustodyRestore::CleanupDelivery,
                &mut read,
            );
            let mut expected = original.clone();
            match field {
                RestoreTupleFault::FenceToken => expected.fence_token = FenceToken::new(8),
                RestoreTupleFault::Generation => expected.generation = Generation::new(3),
                RestoreTupleFault::RuntimeId => {
                    expected.agent_runtime_id =
                        AgentRuntimeId::new(AgentIdentity::from("w-1"), Generation::new(3));
                }
            }
            assert_eq!(read, expected, "{field:?}");
            // One-shot: the next restore reads the entry as it is.
            let mut again = original.clone();
            apply_restore_tuple_fault_for_test(
                &mob,
                MemberCustodyRestore::CleanupDelivery,
                &mut again,
            );
            assert_eq!(again, original, "{field:?} applies once");
        }
    }

    #[test]
    fn a_fault_applies_only_to_its_mob_and_member() {
        let mob = MobId::from("restore-fault-keyed");
        let other_mob = MobId::from("restore-fault-keyed-other");
        let member = entry("w-1");
        let other_member = entry("w-2");
        let site = MemberCustodyRestore::CleanupDelivery;
        fault_next_restore_tuple_for_test(
            &mob,
            &member.agent_identity,
            site,
            RestoreTupleFault::FenceToken,
        );

        let mut read = other_member.clone();
        apply_restore_tuple_fault_for_test(&mob, site, &mut read);
        assert_eq!(read, other_member, "another member of the mob is untouched");
        let mut read = member.clone();
        apply_restore_tuple_fault_for_test(&other_mob, site, &mut read);
        assert_eq!(read, member, "the same member of another mob is untouched");

        let mut read = member.clone();
        apply_restore_tuple_fault_for_test(&mob, site, &mut read);
        assert_eq!(
            read.fence_token,
            FenceToken::new(8),
            "the armed member still sees it"
        );
    }

    /// The cleanup delivery restore reads every roster entry before the
    /// operation binding restore runs: a fault armed for one restore is left
    /// for that restore, not consumed by the other.
    #[test]
    fn a_fault_applies_only_at_its_restore() {
        let mob = MobId::from("restore-fault-site");
        let member = entry("w-1");
        fault_next_restore_tuple_for_test(
            &mob,
            &member.agent_identity,
            MemberCustodyRestore::StartupBinding,
            RestoreTupleFault::RuntimeId,
        );

        let mut read = member.clone();
        apply_restore_tuple_fault_for_test(&mob, MemberCustodyRestore::CleanupDelivery, &mut read);
        assert_eq!(
            read, member,
            "the cleanup delivery restore reads the entry as it is"
        );

        let mut read = member.clone();
        apply_restore_tuple_fault_for_test(&mob, MemberCustodyRestore::StartupBinding, &mut read);
        assert_eq!(
            read.agent_runtime_id,
            AgentRuntimeId::new(AgentIdentity::from("w-1"), Generation::new(3)),
            "the operation binding restore sees the fault armed for it"
        );
    }
}
