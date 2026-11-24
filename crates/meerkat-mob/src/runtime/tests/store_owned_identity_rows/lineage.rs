//! Where the lineage of a created identity's member goes: its identity
//! record's requester and consent decide it, never its seat (which is the
//! host's whoever asked).

use super::*;
use crate::runtime::{LineageClosed, MemberLineageStep, MobLineageCause};

async fn step(mob: &StoreOwnedMob, name: &str) -> MemberLineageStep {
    mob.handle.member_lineage_step(&id(name)).await
}

fn create_as(
    operation_id: &str,
    requester: IdentityChangeRequesterInput,
    name: &str,
) -> StoreOwnedIdentityCreate {
    StoreOwnedIdentityCreate::new(
        operation(operation_id),
        requester,
        vec![CreatedIdentityRequest::new(spec(name, "worker"))],
    )
}

/// An identity the host declared is its mob's own.
#[tokio::test]
async fn a_declared_entry_is_its_mobs_own() {
    let mob = StoreOwnedMob::new("lineage-declared").await;
    let mut entry = SpawnMemberSpec::new("worker", id("declared"));
    entry.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    mob.handle
        .spawn_spec(entry.with_entry_custody(true))
        .await
        .expect("the host declares an entry");
    assert_eq!(
        step(&mob, "declared").await,
        MemberLineageStep::Mob(MobLineageCause::DeclaredEntry)
    );
}

/// An identity created at a person's request, or on a host surface other
/// than the Console, is its mob's own.
#[tokio::test]
async fn an_entry_a_person_or_the_host_asked_for_is_its_mobs_own() {
    let mob = StoreOwnedMob::new("lineage-asked").await;
    committed(
        mob.create(
            create_as(
                "create-by-person",
                IdentityChangeRequesterInput::Human {
                    principal: "person:pat".to_string(),
                },
                "by-person",
            ),
            ALLOW,
        )
        .await,
    );
    committed(
        mob.create(
            create_as(
                "create-by-host",
                IdentityChangeRequesterInput::host("host"),
                "by-host",
            ),
            ALLOW,
        )
        .await,
    );
    for name in ["by-person", "by-host"] {
        assert_eq!(
            step(&mob, name).await,
            MemberLineageStep::Mob(MobLineageCause::AuthorizedEntry),
            "{name}"
        );
    }
}

/// An identity the host is recorded to have asked for on the Console
/// surface names no verified requester: its lineage closes.
#[tokio::test]
async fn an_entry_asked_for_on_the_console_surface_is_closed() {
    let mob = StoreOwnedMob::new("lineage-console").await;
    committed(
        mob.create(host_create("create-console", &["alpha"]), ALLOW)
            .await,
    );
    assert_eq!(
        step(&mob, "alpha").await,
        MemberLineageStep::Closed(LineageClosed::UnverifiedRequester)
    );
}

/// An identity an agent asked for with a person's recorded decision is its
/// mob's own; under the host's allow rule alone its lineage closes: the
/// record names the agent that asked by name only, and a name is never
/// followed.
#[tokio::test]
async fn an_entry_an_agent_asked_for_follows_its_consent() {
    let mob = StoreOwnedMob::new("lineage-agent").await;
    let session = mob.spawn_requester("requester").await;

    let decided = spec("decided", "worker");
    let grant = mob
        .admit(&session, "create-decided", std::slice::from_ref(&decided))
        .await
        .expect("the spawn grant admits the worker profile");
    let now = now_ms();
    let permit = decided_permit(&operation("create-decided"), now, now + 600_000);
    committed(
        mob.create(
            StoreOwnedMob::agent_create("create-decided", grant, vec![decided]),
            IdentityChangeConsentInput::Decided(&permit),
        )
        .await,
    );
    assert_eq!(
        step(&mob, "decided").await,
        MemberLineageStep::Mob(MobLineageCause::AuthorizedEntry),
        "a person decided the create"
    );

    let allowed = spec("allowed", "worker");
    let grant = mob
        .admit(&session, "create-allowed", std::slice::from_ref(&allowed))
        .await
        .expect("the spawn grant admits the worker profile");
    committed(
        mob.create(
            StoreOwnedMob::agent_create("create-allowed", grant, vec![allowed]),
            ALLOW,
        )
        .await,
    );
    assert_eq!(
        step(&mob, "allowed").await,
        MemberLineageStep::Closed(LineageClosed::CreatorNotProven),
        "the allow rule alone let the create through, for a plain member"
    );
}

/// The identity `name` created under the host's allow rule alone at the
/// request of the agent on `session`.
async fn allowed_create(mob: &StoreOwnedMob, session: &SessionId, operation_id: &str, name: &str) {
    let asked = spec(name, "worker");
    let grant = mob
        .admit(session, operation_id, std::slice::from_ref(&asked))
        .await
        .expect("the spawn grant admits the worker profile");
    committed(
        mob.create(
            StoreOwnedMob::agent_create(operation_id, grant, vec![asked]),
            ALLOW,
        )
        .await,
    );
}

/// A plain member asks for an identity under the host's allow rule and then
/// leaves; an identity is later created under the name it held. The first
/// identity's lineage reads closed: it is not the later identity's.
#[tokio::test]
async fn an_identity_later_created_under_an_askers_name_gives_what_it_asked_for_nothing() {
    let mob = StoreOwnedMob::new("lineage-asker-name").await;
    let session = mob.spawn_requester("asker").await;
    allowed_create(&mob, &session, "create-asked", "asked").await;
    assert_eq!(
        step(&mob, "asked").await,
        MemberLineageStep::Closed(LineageClosed::CreatorNotProven)
    );

    mob.handle
        .retire(id("asker"))
        .await
        .expect("the asker retires");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_mins(1);
    while mob.is_member("asker").await {
        assert!(tokio::time::Instant::now() < deadline, "the asker left");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    committed(
        mob.create(
            create_as(
                "create-under-the-name",
                IdentityChangeRequesterInput::host("host"),
                "asker",
            ),
            ALLOW,
        )
        .await,
    );
    assert_eq!(
        step(&mob, "asker").await,
        MemberLineageStep::Mob(MobLineageCause::AuthorizedEntry),
        "the later identity's own lineage is its own record's"
    );
    assert_eq!(
        step(&mob, "asked").await,
        MemberLineageStep::Closed(LineageClosed::CreatorNotProven),
        "the first identity's lineage is not the later identity's"
    );
}

/// An identity an identity entry asked for under the host's allow rule
/// alone reads closed too: who asked is never followed by its name.
#[tokio::test]
async fn an_identity_an_identity_entry_asked_for_under_the_allow_rule_reads_closed() {
    let mob = StoreOwnedMob::new("lineage-entry-asker").await;
    mob.handle
        .spawn_spec(SpawnMemberSpec::new("worker", id("lead-entry")).with_entry_custody(true))
        .await
        .expect("the host declares an entry that can ask");
    let session = mob.session("lead-entry").await;
    allowed_create(&mob, &session, "create-by-entry", "by-entry").await;
    assert_eq!(
        step(&mob, "by-entry").await,
        MemberLineageStep::Closed(LineageClosed::CreatorNotProven)
    );
}
