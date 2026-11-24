//! Who a mob is private to, when the member that created it sits in a mob
//! that is not loaded.
//!
//! A launch over a declared store loads a stored mob when something
//! addresses it. A mob a member created is read from its durable records at
//! its load, and who it is private to is its creating member's owner. That
//! member's mob need not be loaded then: its roster row is read from its
//! store, and the mob stays as it is.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::time::Duration;

use meerkat_mob::{AgentIdentity, MobId, MobRuntimeMode, ProfileName, SpawnMemberSpec};

use super::super::UnifiedRuntime;
use super::super::mob_entries_tests::{definition, second_mob, set_of, shut_down};
use super::lazy_start_tests::{StoredHost, known};

/// The person the launch's member, and so every mob below it, is private to.
const PERSON: &str = "console-caller:v1:creator-store-read";

/// The mobs of one fixture: the mob a member of the launch's mob created
/// (`parent`), its member (`lead`), and the mob that member created
/// (`child`).
struct Family {
    parent: MobId,
    child: MobId,
}

const LEAD: &str = "lead";

/// Build the family on `host`: a member of the launch's mob that is
/// private to [`PERSON`] creates `parent`; `lead`, seated in `parent`,
/// creates `child`. Every mob is then private to that person.
async fn family(host: &UnifiedRuntime) -> Family {
    Box::pin(host.mob_handle().spawn_spec(
        SpawnMemberSpec::new("worker", AgentIdentity::from("owner")).with_labels(BTreeMap::from([
            (
                crate::access::PRIVATE_TO_LABEL.to_string(),
                PERSON.to_string(),
            ),
        ])),
    ))
    .await
    .expect("the owner is seated in the launch's mob");
    let owner = host
        .mob_handle()
        .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id("owner"))
        .await
        .expect("the owner's session");
    let set = set_of(host);
    let parent = definition("parent-of-a-creator");
    let parent_id = parent.id.clone();
    set.mob_create_definition_with_owner_bridge_session(parent, owner, true, false)
        .await
        .expect("the owner's mob is created");
    set.mob_spawn(
        &parent_id,
        ProfileName::from("worker"),
        AgentIdentity::from(LEAD),
        Some(MobRuntimeMode::TurnDriven),
        None,
        None,
    )
    .await
    .expect("the owner's mob seats its lead");
    let parent_handle = set.handle_for(&parent_id).await.expect("the owner's mob");
    let lead = parent_handle
        .get_member(&AgentIdentity::from(LEAD))
        .await
        .expect("the roster of the owner's mob")
        .expect("the lead is seated");
    assert_eq!(
        crate::access::private_owner(&lead.labels),
        Some(PERSON),
        "the lead of a private member's mob is private to that member's person"
    );
    let lead_session = parent_handle
        .resolve_bridge_session_id(&AgentIdentity::from(LEAD))
        .await
        .expect("the lead's session");
    let child = definition("child-of-a-lead");
    let child_id = child.id.clone();
    set.mob_create_definition_with_owner_bridge_session(child, lead_session, true, false)
        .await
        .expect("the lead's mob is created");
    Family {
        parent: parent_id,
        child: child_id,
    }
}

/// Spawn `name` into the mob `mob_id` as the host does, once the record of
/// who each mob is private to answers for the mob: a refusal that asks for
/// a retry (the record is still reading the mob) is made again.
async fn spawned_once_the_mob_is_read(
    host: &UnifiedRuntime,
    mob_id: &MobId,
    name: &str,
) -> Result<(), String> {
    let set = set_of(host);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let spawned = set
            .mob_spawn(
                mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from(name),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await;
        match spawned {
            Ok(_) => return Ok(()),
            Err(error) => {
                let refusal = error.to_string();
                if !refusal.contains("retry") || tokio::time::Instant::now() >= deadline {
                    return Err(refusal);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// A mob created by a member of a mob that is not loaded is settled from
/// that mob's store. After a restart only the child mob is addressed: it
/// is loaded and read, its creating member's roster row is read from the
/// store of the member's mob, and a spawn into the child mob is seated as
/// that member's person's. The member's mob is not loaded for it.
#[tokio::test]
async fn a_mob_whose_creators_mob_is_not_loaded_is_settled_from_that_mobs_store() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let Family { parent, child } = family(&host).await;
    shut_down(&host, &[]).await;
    drop(host);

    let host = stored.launch().await;
    let set = set_of(&host);
    let loaded = set
        .handle_for(&child)
        .await
        .expect("the lookup loads the child mob");
    let after_the_load = known(&host).await;
    assert!(
        after_the_load.not_loaded.contains(&parent) && !after_the_load.loaded.contains(&parent),
        "the lookup of the child mob loads its creating member's mob no more than any other: \
         loaded {:?}, not loaded {:?}",
        after_the_load.loaded,
        after_the_load.not_loaded
    );

    let spawned = spawned_once_the_mob_is_read(&host, &child, "late").await;
    assert_eq!(
        spawned,
        Ok(()),
        "a spawn into a mob whose creating member's mob is not loaded is seated"
    );
    let late = loaded
        .get_member(&AgentIdentity::from("late"))
        .await
        .expect("the child mob's roster")
        .expect("the spawned member is seated");
    assert_eq!(
        crate::access::private_owner(&late.labels),
        Some(PERSON),
        "it is private to the person its mob's creating member is private to"
    );
    let after_the_spawn = known(&host).await;
    assert!(
        after_the_spawn.not_loaded.contains(&parent) && !after_the_spawn.loaded.contains(&parent),
        "the creating member's mob was read from its store and not loaded: loaded {:?}, not \
         loaded {:?}",
        after_the_spawn.loaded,
        after_the_spawn.not_loaded
    );
    drop(loaded);
    shut_down(&host, &[]).await;
}

/// The store of the creating member's mob cannot be read: nothing says who
/// that member is private to, so the mob it created is not taken to be
/// private to no one. A spawn into it is refused, and the refusal says what
/// the mob waits for.
#[tokio::test]
async fn a_mob_whose_creators_mob_has_a_store_that_cannot_be_read_stays_unsettled() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let Family { parent, child } = family(&host).await;
    shut_down(&host, &[]).await;
    drop(host);
    let file = crate::child_mob_storage::child_mob_databases(stored.state_dir())
        .into_iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(parent.as_str()))
        })
        .expect("the stored file of the creating member's mob");
    std::fs::write(&file, b"not a sqlite database").expect("damage the stored mob's file");

    let host = stored.launch().await;
    let set = set_of(&host);
    let loaded = set
        .handle_for(&child)
        .await
        .expect("the lookup loads the child mob");
    let refusal = spawned_once_the_mob_is_read(&host, &child, "late")
        .await
        .expect_err("a spawn into a mob whose creating member nothing says the owner of");
    assert!(
        refusal.contains(child.as_str())
            && refusal.contains("is not known")
            && !refusal.contains("retry"),
        "the refusal names the mob and says what it waits for: {refusal}"
    );
    assert!(
        loaded
            .get_member(&AgentIdentity::from("late"))
            .await
            .expect("the child mob's roster")
            .is_none(),
        "nothing was seated"
    );
    drop(loaded);
    shut_down(&host, &[]).await;
}

/// The first person a row's member is private to, and the second.
const FIRST: &str = "console-caller:v1:creator-store-read-first";
const SECOND: &str = "console-caller:v1:creator-store-read-second";

/// The seat of one member of a mob, as its roster held it: the mob, the
/// incarnation that mob runs at, the member and its generation, and the
/// session the member holds.
struct Seat {
    member: crate::private_owners::Convener,
    session: meerkat_core::types::SessionId,
}

/// Seat `name` in the mob `mob` of `host`, private to `person`, and answer
/// its seat.
async fn seated_private_to(host: &UnifiedRuntime, mob: &MobId, name: &str, person: &str) -> Seat {
    let handle = set_of(host)
        .handle_for(mob)
        .await
        .expect("the host's second mob");
    Box::pin(
        handle.spawn_spec(
            SpawnMemberSpec::new("worker", AgentIdentity::from(name))
                .with_labels(BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    person.to_string(),
                )]))
                .with_runtime_mode(MobRuntimeMode::TurnDriven),
        ),
    )
    .await
    .expect("the member is seated");
    let row = handle
        .get_member(&AgentIdentity::from(name))
        .await
        .expect("the mob's roster")
        .expect("the member is seated");
    assert_eq!(
        crate::access::private_owner(&row.labels),
        Some(person),
        "its roster row names its person"
    );
    Seat {
        member: (
            mob.clone(),
            handle.incarnation(),
            AgentIdentity::from(name),
            row.generation,
        ),
        session: row
            .bridge_session_id()
            .cloned()
            .expect("the member holds a session"),
    }
}

/// Retire `name` from the mob `mob` of `host` and wait until its roster
/// holds it no more.
async fn left(host: &UnifiedRuntime, mob: &MobId, name: &str) {
    let handle = set_of(host)
        .handle_for(mob)
        .await
        .expect("the host's second mob");
    handle
        .retire(AgentIdentity::from(name))
        .await
        .expect("the member is retired");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while handle
        .get_member(&AgentIdentity::from(name))
        .await
        .expect("the mob's roster")
        .is_some()
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the retired member left its roster"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// What a record that has read nothing says of a mob whose creating member
/// is `member`, seated on `session`: who a spawn into that mob is private
/// to, or the spawn's refusal. The mob is recorded as the read of a
/// council's mob records it for one convener, which asks the same
/// judgement as the read of any mob a member created.
async fn a_fresh_record_says(
    host: &UnifiedRuntime,
    member: &crate::private_owners::Convener,
    session: &meerkat_core::types::SessionId,
) -> Result<Option<String>, String> {
    const CREATED: &str = "a-mob-that-member-created";
    let set = set_of(host);
    let owners = crate::private_owners::PrivateOwners::default();
    owners.bind_child_mobs(&set);
    owners
        .record_council_mob_convened_by(&set, CREATED, std::slice::from_ref(member), session)
        .await;
    let mut spec = SpawnMemberSpec::new("worker", AgentIdentity::from("late"));
    owners
        .apply_child(CREATED, None, &mut spec)
        .map(|()| {
            spec.labels
                .and_then(|mut labels| labels.remove(crate::access::PRIVATE_TO_LABEL))
        })
        .map_err(|error| error.to_string())
}

/// Whether the mob `mob` is known to `host` and not loaded.
async fn is_not_loaded(host: &UnifiedRuntime, mob: &MobId) -> bool {
    let mobs = known(host).await;
    mobs.not_loaded.contains(mob) && !mobs.loaded.contains(mob)
}

/// A creating member that LEFT a mob that is not loaded is answered by its
/// own binding in that mob's stored member history: the mob it created is
/// its person's. The member's mob is read from its store and stays not
/// loaded.
#[tokio::test]
async fn a_member_that_left_a_mob_that_is_not_loaded_is_answered_by_its_own_binding() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let parent = second_mob(&host, definition("mob-of-a-member-that-left")).await;
    let seat = seated_private_to(&host, &parent, LEAD, FIRST).await;
    left(&host, &parent, LEAD).await;
    shut_down(&host, &[]).await;
    drop(host);

    let host = stored.launch().await;
    assert!(
        is_not_loaded(&host, &parent).await,
        "the member's mob is not loaded"
    );
    assert_eq!(
        a_fresh_record_says(&host, &seat.member, &seat.session).await,
        Ok(Some(FIRST.to_string())),
        "a creating member that left a mob that is not loaded: its own binding says its person"
    );
    assert!(
        is_not_loaded(&host, &parent).await,
        "the member's mob was read from its store and not loaded"
    );
    shut_down(&host, &[]).await;
}

/// A creating member whose name a LATER member holds in a mob that is not
/// loaded is answered by its own binding, not by the later member: the mob
/// it created is its person's, and a mob the later member created is the
/// later member's person's.
#[tokio::test]
async fn a_member_whose_name_a_later_member_holds_in_a_mob_that_is_not_loaded_keeps_its_own_person()
{
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let parent = second_mob(&host, definition("mob-of-a-name-held-twice")).await;
    let first = seated_private_to(&host, &parent, LEAD, FIRST).await;
    left(&host, &parent, LEAD).await;
    let second = seated_private_to(&host, &parent, LEAD, SECOND).await;
    assert_ne!(
        first.member.3, second.member.3,
        "the later member is another member under the name"
    );
    shut_down(&host, &[]).await;
    drop(host);

    let host = stored.launch().await;
    assert!(
        is_not_loaded(&host, &parent).await,
        "the members' mob is not loaded"
    );
    assert_eq!(
        a_fresh_record_says(&host, &first.member, &first.session).await,
        Ok(Some(FIRST.to_string())),
        "the first member's mob is its person's, whoever holds its name in the stored roster"
    );
    assert_eq!(
        a_fresh_record_says(&host, &second.member, &second.session).await,
        Ok(Some(SECOND.to_string())),
        "the later member's mob is the later member's person's"
    );
    assert!(
        is_not_loaded(&host, &parent).await,
        "the members' mob was read from its store and not loaded"
    );
    shut_down(&host, &[]).await;
}

/// For a mob that is not loaded only the member's own binding answers: no
/// roster row is read from a store. A creation record that names a session
/// the stored journal never bound says nothing of the member seated under
/// the name, also at the record's generation number: the mob is not taken
/// to be that member's person's, and a spawn into it is refused, not as
/// retryable.
#[tokio::test]
async fn a_session_the_stored_journal_never_bound_says_nothing_of_a_member_of_a_mob_not_loaded() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let parent = second_mob(&host, definition("mob-of-a-session-never-bound")).await;
    let seat = seated_private_to(&host, &parent, LEAD, FIRST).await;
    shut_down(&host, &[]).await;
    drop(host);

    let host = stored.launch().await;
    assert!(
        is_not_loaded(&host, &parent).await,
        "the member's mob is not loaded"
    );
    let never_bound = meerkat_core::types::SessionId::new();
    let refusal = a_fresh_record_says(&host, &seat.member, &never_bound)
        .await
        .expect_err("a session the journal never bound names no creating member");
    assert!(
        refusal.contains("is not known") && !refusal.contains("retry"),
        "the refusal is the one of a creating member nothing says for certain: {refusal}"
    );
    assert_eq!(
        a_fresh_record_says(&host, &seat.member, &seat.session).await,
        Ok(Some(FIRST.to_string())),
        "the member's own session is answered by its binding"
    );
    shut_down(&host, &[]).await;
}

/// The name of the member of the mob that a host's mob's member makes in
/// the rows below.
const MADE_MEMBER: &str = "desk";

/// On `host`: a mob the host creates beside the launch mob, and a mob its
/// member [`LEAD`] makes, with the member [`MADE_MEMBER`]. The host's mob
/// and the made mob.
async fn a_mob_made_under_a_hosts_mob(host: &UnifiedRuntime) -> (MobId, MobId) {
    let hosted = second_mob(host, definition("hosts-mob-of-a-maker")).await;
    let set = set_of(host);
    set.mob_spawn(
        &hosted,
        ProfileName::from("worker"),
        AgentIdentity::from(LEAD),
        Some(MobRuntimeMode::TurnDriven),
        None,
        None,
    )
    .await
    .expect("the host's mob seats its lead");
    let lead_session = set
        .handle_for(&hosted)
        .await
        .expect("the host's mob")
        .resolve_bridge_session_id(&AgentIdentity::from(LEAD))
        .await
        .expect("the lead's session");
    let made = definition("made-under-a-hosts-mob");
    let made_id = made.id.clone();
    set.mob_create_definition_with_owner_bridge_session(made, lead_session, true, false)
        .await
        .expect("the lead's mob is created");
    set.mob_spawn(
        &made_id,
        ProfileName::from("worker"),
        AgentIdentity::from(MADE_MEMBER),
        Some(MobRuntimeMode::TurnDriven),
        None,
        None,
    )
    .await
    .expect("the made mob seats its member");
    (hosted, made_id)
}

/// The members the record of who each member is private to answers under
/// the mob `primary` of `host`, each with its mob.
fn answered_under(host: &UnifiedRuntime, primary: &str) -> Vec<(String, String)> {
    host.mob_runtime()
        .private_owners()
        .nested_members(primary)
        .into_iter()
        .map(|(mob, name, _)| (mob, name))
        .collect()
}

/// Wait, at most thirty seconds, until the record of who each member is
/// private to has read the mob `mob_id` of `host`: the pass that follows
/// the mob's load reads it. Whether it has.
async fn the_record_has_read(host: &UnifiedRuntime, mob_id: &MobId) -> bool {
    let owners = host.mob_runtime().private_owners().clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !owners.has_read_the_mob(mob_id.as_str()) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    owners.has_read_the_mob(mob_id.as_str())
}

/// A mob made under a mob the host created beside the launch mob is that
/// mob's, also while that mob is stored and not loaded. A member of such a
/// host's mob created a mob with a member of its own. After a restart only
/// the made mob is addressed: it is loaded, and the record of who each
/// member is private to reads it in the pass that follows its load. The row
/// waits for that pass alone and makes no reading of its own. The mob set
/// reads who created a mob it has not loaded on a sweep of its own, which
/// runs ahead of that pass or behind it: the row does not place the order
/// of the two, and in either order the record answers the launch mob no
/// member of the made mob. The host's mob is read as not loaded after the
/// answer. Once the host's mob is loaded the record answers that mob the
/// made mob's member, and the launch mob still none.
#[tokio::test]
async fn a_mob_made_under_a_hosts_mob_that_is_not_loaded_is_not_the_launch_mobs() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let launch_mob = host.mob_id();
    let (hosted, made_id) = a_mob_made_under_a_hosts_mob(&host).await;
    shut_down(&host, &[]).await;
    drop(host);

    let host = stored.launch().await;
    let set = set_of(&host);
    let loaded = set
        .handle_for(&made_id)
        .await
        .expect("the lookup loads the made mob");
    let the_made_mobs_member = (made_id.to_string(), MADE_MEMBER.to_string());
    let the_record_read_the_made_mob = the_record_has_read(&host, &made_id).await;
    let under_the_launch_mob = answered_under(&host, &launch_mob);
    // The scene, read after the answer: the host's mob is still not loaded.
    let after_the_answer = known(&host).await;
    let the_hosts_mob_is_not_loaded =
        after_the_answer.not_loaded.contains(&hosted) && !after_the_answer.loaded.contains(&hosted);

    // The host's mob is loaded, as a request that names it loads it.
    let hosted_handle = set.handle_for(&hosted).await.expect("the host's mob loads");
    let under_the_hosts_mob = answered_under(&host, hosted.as_str());
    let under_the_launch_mob_then = answered_under(&host, &launch_mob);
    drop(hosted_handle);
    drop(loaded);
    drop(set);
    shut_down(&host, &[]).await;

    assert!(
        the_record_read_the_made_mob,
        "the fixture: the record read the made mob"
    );
    assert!(
        the_hosts_mob_is_not_loaded,
        "the fixture: after the answer the host's mob is not loaded: loaded {:?}, not loaded {:?}",
        after_the_answer.loaded, after_the_answer.not_loaded
    );
    assert!(
        !under_the_launch_mob.contains(&the_made_mobs_member),
        "while the host's mob is not loaded the launch mob is answered the member of a mob \
         made under it: {under_the_launch_mob:?} (after the answer the mob set knows as created \
         by the host: {:?})",
        after_the_answer.created_by_the_host
    );
    assert_eq!(
        under_the_hosts_mob,
        vec![the_made_mobs_member.clone()],
        "once it is loaded the host's mob is answered the member of the mob made under it"
    );
    assert!(
        !under_the_launch_mob_then.contains(&the_made_mobs_member),
        "once the host's mob is loaded the launch mob is answered the member of a mob made \
         under it: {under_the_launch_mob_then:?}"
    );
}

/// A creating mob of which no stored creation can be read stays
/// unclassified: nothing takes it for a mob the host created. The store of
/// the host's mob is damaged while the gateway is down. After the restart
/// the made mob is loaded and read. The mob set answers nothing of who
/// created the host's mob, the record holds no creation of it, and the
/// made mob's member is answered under the launch mob, as the member of
/// any mob whose line of creating mobs ends at a mob no record names.
#[tokio::test]
async fn a_mob_made_under_a_mob_whose_creation_cannot_be_read_stays_the_launch_mobs() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let launch_mob = host.mob_id();
    let (hosted, made_id) = a_mob_made_under_a_hosts_mob(&host).await;
    shut_down(&host, &[]).await;
    drop(host);
    let file = crate::child_mob_storage::child_mob_databases(stored.state_dir())
        .into_iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(hosted.as_str()))
        })
        .expect("the stored file of the host's mob");
    std::fs::write(&file, b"not a sqlite database").expect("damage the stored mob's file");

    let host = stored.launch().await;
    let set = set_of(&host);
    let loaded = set
        .handle_for(&made_id)
        .await
        .expect("the lookup loads the made mob");
    let the_made_mobs_member = (made_id.to_string(), MADE_MEMBER.to_string());
    let the_record_read_the_made_mob = the_record_has_read(&host, &made_id).await;
    let under_the_launch_mob = answered_under(&host, &launch_mob);
    let after_the_answer = known(&host).await;
    drop(loaded);
    drop(set);
    shut_down(&host, &[]).await;

    assert!(
        the_record_read_the_made_mob,
        "the fixture: the record read the made mob"
    );
    assert!(
        !after_the_answer.created_by_the_host.contains(&hosted)
            && !after_the_answer.loaded.contains(&hosted),
        "the fixture: the mob set reads no creation from a store that cannot be read and loads \
         no mob from it: loaded {:?}, created by the host {:?}",
        after_the_answer.loaded,
        after_the_answer.created_by_the_host
    );
    assert!(
        under_the_launch_mob.contains(&the_made_mobs_member),
        "a mob whose creating mob has no creation that can be read is not answered under the \
         launch mob: {under_the_launch_mob:?}"
    );
}
