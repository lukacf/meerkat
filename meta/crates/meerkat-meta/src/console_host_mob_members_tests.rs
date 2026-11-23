//! The console's walks of the mobs of a host's set, on a host that serves a
//! second mob with a member of its own: the launch mob's console takes a
//! mob as a member's group only when a member's session owns it, so a mob
//! the host created is no group of the launch mob and its members are not
//! the launch mob's.

#![allow(clippy::expect_used)]

use std::collections::BTreeSet;

use super::*;
use crate::unified_runtime::mob_set_reads_tests::{HostOfTwoMobs, PrivateDesk};

/// The snapshot walk of the launch mob and the walk its access priming
/// makes both list the launch mob's own members and no group for the second
/// mob, which holds a member of its own under a name the launch mob has
/// too.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_snapshot_and_priming_take_no_host_created_mob_as_a_members_group() {
    let temp = tempfile::tempdir().expect("tempdir");
    let hosted = HostOfTwoMobs::boot(&temp, PrivateDesk::OfTheSecondMob).await;
    assert_eq!(
        hosted
            .second
            .list_members_observation_snapshot()
            .await
            .len(),
        1,
        "the second mob holds its own member"
    );
    let runtime = hosted.host.mob_runtime();

    let walked = Box::pin(walk_console_snapshot_read_model(runtime, None)).await;
    let own: Vec<&str> = walked
        .primary_members
        .iter()
        .map(|member| member.agent_identity.as_str())
        .collect();
    assert_eq!(
        (own.len(), own.iter().copied().collect::<BTreeSet<_>>()),
        (2, BTreeSet::from(["clerk", "desk"])),
        "the snapshot walk lists the launch mob's two members, each once"
    );
    assert!(
        walked.mob_groups.is_empty(),
        "the snapshot walk makes no group of a mob the host created: {} group(s)",
        walked.mob_groups.len()
    );

    let groups: Vec<String> = delegate_mobs_with_hosts(runtime)
        .await
        .into_iter()
        .map(|(mob_id, _, host_member)| format!("{mob_id} of {host_member}"))
        .collect();
    assert!(
        groups.is_empty(),
        "the access priming records no member of a mob the host created: {groups:?}"
    );
    hosted.stop().await;
}
