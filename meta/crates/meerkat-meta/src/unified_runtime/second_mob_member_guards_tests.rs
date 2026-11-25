//! Rows of what a launch puts on the members of a mob it hosts beside its
//! launch mob: one row per guard, each on a real build of a member of a
//! second mob.
//!
//! The fixture is the launch of the entries' rows: a launch that declares
//! its mob to its mob set, as the gateways do. The set builds the second
//! mob through the launch's composition, and each row seats its members
//! through that mob's own handle, as Core's own spawn does: nothing of
//! Meta's doors stands between the row and the guard it shows.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use meerkat_mob::{
    AgentIdentity, ForkContext, MemberLaunchMode, MobHandle, MobId, MobRuntimeMode, SpawnMemberSpec,
};

use super::StoredHost;
use super::UnifiedRuntime;
use super::held_identity_test_support::HostPolicy;
use super::mob_entries_tests::{
    Sessions, definition, launch, launch_composed, second_mob, set_of, shut_down,
};
use crate::mob_handle_runtime::{AfterCreateHook, SessionCreatedContext};
use crate::personal_accounts::{
    McpAccountMode, McpAccountsConfig, McpServerAccounts, PersonalAccountOwnerCheck,
    PersonalAccountRefusal, PersonalMcpAccounts, UnsupportedPersonalServer,
};

/// The person the rows' private members are private to.
const OWNER: &str = "console-caller:v1:aa";

/// The handle the launch's mob set serves the second mob through.
async fn handle_of(host: &UnifiedRuntime, second_id: &MobId) -> MobHandle {
    set_of(host)
        .handle_for(second_id)
        .await
        .expect("the set serves the second mob")
}

/// A member of the fixture's one profile that runs no turn of its own.
fn member(name: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::new("worker", name).with_runtime_mode(MobRuntimeMode::TurnDriven)
}

/// [`member`], seated as Meta seats a member private to [`OWNER`]: with the
/// label only Meta writes.
fn private_member(name: &str) -> SpawnMemberSpec {
    member(name).with_labels(BTreeMap::from([(
        crate::access::PRIVATE_TO_LABEL.to_string(),
        OWNER.to_string(),
    )]))
}

/// The labels the roster row of `name` carries in the mob of `handle`.
async fn labels_of(handle: &MobHandle, name: &str) -> Option<BTreeMap<String, String>> {
    handle
        .get_member(&AgentIdentity::from(name))
        .await
        .ok()
        .flatten()
        .map(|entry| entry.labels)
}

/// A spawn into a second mob refuses a member under an identity of the
/// runtime plane, as a spawn into the launch mob does, and seats nothing.
#[tokio::test]
async fn a_spawn_into_a_second_mob_refuses_a_runtime_plane_identity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = handle_of(&host, &second_id).await;

    for reserved in ["_system", "__console__"] {
        let refused = Box::pin(second.spawn_spec(member(reserved))).await;
        assert!(
            matches!(&refused, Err(error) if error.to_string().contains("is reserved")),
            "a member '{reserved}' of the second mob is refused as reserved: {:?}",
            refused.as_ref().map(|_| "seated")
        );
        assert!(
            labels_of(&second, reserved).await.is_none(),
            "and '{reserved}' is not seated"
        );
    }
    let seated = Box::pin(second.spawn_spec(member("plain"))).await;
    assert!(
        seated.is_ok(),
        "a member under an ordinary name is seated: {:?}",
        seated.as_ref().err()
    );
    shut_down(&host, &[&second_id]).await;
}

/// A fork of a member of a second mob that is private to a person is
/// private to the same person: the launch gives it its source's owner, from
/// the record it keeps of the members it built, the second mob's included.
#[tokio::test]
async fn a_fork_of_a_private_member_of_a_second_mob_is_private_to_the_same_person() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = handle_of(&host, &second_id).await;
    let seated = Box::pin(second.spawn_spec(private_member("side"))).await;
    assert!(
        seated.is_ok(),
        "a private member of the second mob: {:?}",
        seated.as_ref().err()
    );

    let forked = Box::pin(second.spawn_spec(member("side-fork").with_launch_mode(
        MemberLaunchMode::Fork {
            source_member_id: AgentIdentity::from("side"),
            fork_context: ForkContext::default(),
        },
    )))
    .await;
    assert!(
        forked.is_ok(),
        "a fork of the private member: {:?}",
        forked.as_ref().err()
    );
    let owner_of_the_fork = labels_of(&second, "side-fork")
        .await
        .and_then(|labels| crate::access::private_owner(&labels).map(str::to_string));
    assert_eq!(
        owner_of_the_fork.as_deref(),
        Some(OWNER),
        "the fork is private to its source's owner"
    );
    let shared = Box::pin(second.spawn_spec(member("plain"))).await;
    assert!(
        shared.is_ok(),
        "a member nobody forked: {:?}",
        shared.as_ref().err()
    );
    assert!(
        labels_of(&second, "plain")
            .await
            .is_some_and(|labels| crate::access::private_owner(&labels).is_none()),
        "a member that forks nobody is private to nobody"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A member of a second mob that is private to a person is hidden from the
/// operator tools of a member of that mob that is not private to that
/// person, and from a call no member of the mob makes; it is shown to
/// itself, and a shared member is shown to it.
#[tokio::test]
async fn a_private_member_of_a_second_mob_is_hidden_from_another_members_operator_tools() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = handle_of(&host, &second_id).await;
    for spec in [private_member("side"), member("plain")] {
        let seated = Box::pin(second.spawn_spec(spec)).await;
        assert!(
            seated.is_ok(),
            "the members of the second mob: {:?}",
            seated.as_ref().err()
        );
    }
    let (side, plain) = (AgentIdentity::from("side"), AgentIdentity::from("plain"));
    let both = [side.clone(), plain.clone()];

    assert_eq!(
        second.members_hidden_from(Some(&plain), &both).await,
        [side.clone()].into(),
        "the private member is hidden from the shared member's tools, the shared one is not"
    );
    assert_eq!(
        second.members_hidden_from(None, &both).await,
        [side.clone()].into(),
        "and from a call no member of the mob makes"
    );
    assert!(
        second
            .members_hidden_from(Some(&side), &both)
            .await
            .is_empty(),
        "the private member is shown itself and the shared member"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The create of a member of a second mob passes the launch's own chain of
/// hooks before it is built: the launch's record of whom its members are
/// private to, which that chain writes at each member's build, knows the
/// member of the second mob once it is seated.
#[tokio::test]
async fn the_create_of_a_second_mobs_member_passes_the_launchs_hooks_before_its_build() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = handle_of(&host, &second_id).await;
    let record = host.mob_runtime().private_owners().clone();
    assert_eq!(
        record.owner_of(second_id.as_str(), "side"),
        None,
        "the launch's record knows no member of the second mob before one is built"
    );

    let seated = Box::pin(second.spawn_spec(private_member("side"))).await;
    assert!(
        seated.is_ok(),
        "a private member of the second mob: {:?}",
        seated.as_ref().err()
    );
    assert_eq!(
        record.owner_of(second_id.as_str(), "side").as_deref(),
        Some(OWNER),
        "the launch's hooks recorded the member at its build, under the second mob"
    );
    assert_eq!(
        record.owner_of(host.mob_handle().mob_id().as_str(), "side"),
        None,
        "and not under the launch mob"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The sessions a launch's after-create hook was told of, with their
/// labels.
#[derive(Default)]
struct Created(Mutex<Vec<BTreeMap<String, String>>>);

impl Created {
    fn hook(self: &Arc<Self>) -> AfterCreateHook {
        let created = Arc::clone(self);
        Arc::new(move |_session_id, context: SessionCreatedContext| {
            created
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(context.labels);
            Box::pin(async {})
        })
    }

    /// How many creates carried the label `key` with the value `value`.
    fn with_label(&self, key: &str, value: &str) -> usize {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|labels| labels.get(key).is_some_and(|found| found == value))
            .count()
    }
}

/// The create of a member of a second mob runs on the launch's own session
/// service: the hook the launch's spec puts after every create is told of
/// it, once, as it is told of a create in the launch mob.
#[tokio::test]
async fn the_create_of_a_second_mobs_member_is_told_to_the_launchs_after_create_hook() {
    const SEAT: &str = "seat";
    let temp = tempfile::tempdir().expect("tempdir");
    let created = Arc::new(Created::default());
    let hook = created.hook();
    let (host, _sessions, _machine, ()) = launch_composed(
        &temp,
        definition("host"),
        meerkat::Config::default(),
        |_| (),
        move |spec| spec.with_after_create_hook(hook),
    )
    .await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = handle_of(&host, &second_id).await;

    let in_the_second = Box::pin(second.spawn_spec(
        member("worker-b").with_labels(BTreeMap::from([(SEAT.to_string(), "second".to_string())])),
    ))
    .await;
    assert!(
        in_the_second.is_ok(),
        "a member of the second mob: {:?}",
        in_the_second.as_ref().err()
    );
    let in_the_launch_mob = Box::pin(host.mob_handle().spawn_spec(
        member("worker-a").with_labels(BTreeMap::from([(SEAT.to_string(), "launch".to_string())])),
    ))
    .await;
    assert!(
        in_the_launch_mob.is_ok(),
        "a member of the launch mob: {:?}",
        in_the_launch_mob.as_ref().err()
    );
    assert_eq!(
        (
            created.with_label(SEAT, "second"),
            created.with_label(SEAT, "launch")
        ),
        (1, 1),
        "the launch's after-create hook was told of each member's create once"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The personal server of the rows' compositions.
const NOTES: &str = "notes";

/// A definition under an id of its own with two profiles: `assistant`,
/// which declares the personal server [`NOTES`] in its personal shape, and
/// `plain`, which declares none.
fn definition_with_a_personal_server(tag: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"second-mob-guards-{tag}-{}\"\n\n[profiles.assistant]\nmodel = \"gpt-5.5\"\n\n[[profiles.assistant.tools.mcp_servers]]\nname = \"{NOTES}\"\nurl = \"https://notes.example.test/mcp\"\noauth_account_selection = \"discover\"\n\n[profiles.plain]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition with a personal server")
}

/// The owner check of a composition whose server [`NOTES`] is personal,
/// installed on a member factory as a launch installs it.
fn personal_accounts_check() -> PersonalAccountOwnerCheck {
    let check = PersonalMcpAccounts::new(McpAccountsConfig {
        servers: BTreeMap::from([(
            NOTES.to_string(),
            McpServerAccounts {
                accounts: McpAccountMode::Personal,
            },
        )]),
    })
    .owner_check();
    let _factory = check.install(meerkat::AgentFactory::minimal(), None);
    check
}

/// A member of a second mob that would run on another host is refused a
/// personal MCP server, as a member of the launch mob is: the spawn ends at
/// the check of the profile it resolved, typed, and seats nothing. A placed
/// member whose profile declares no personal server does not end there.
#[tokio::test]
async fn a_member_of_a_second_mob_placed_on_another_host_is_refused_a_personal_server() {
    let placed = |profile: &str, name: &str| {
        SpawnMemberSpec::new(profile, name).with_placement(
            meerkat_mob::machines::mob_machine::HostId::from("12D3KooWRemoteHost"),
        )
    };
    let hosted_elsewhere = PersonalAccountRefusal::Unsupported {
        server: NOTES.to_string(),
        reason: UnsupportedPersonalServer::HostedElsewhere,
    }
    .to_string();
    let temp = tempfile::tempdir().expect("tempdir");
    let check = personal_accounts_check();
    let (host, _sessions, _machine, ()) = launch_composed(
        &temp,
        definition("host"),
        meerkat::Config::default(),
        |_| (),
        move |spec| spec.with_personal_mcp_accounts(check),
    )
    .await;
    let second_id = second_mob(&host, definition_with_a_personal_server("second")).await;
    let second = handle_of(&host, &second_id).await;

    let refused = Box::pin(second.spawn_spec(placed("assistant", "member-1"))).await;
    assert!(
        matches!(
            &refused,
            Err(meerkat_mob::MobError::WiringError(message)) if *message == hosted_elsewhere
        ),
        "a placed member of the second mob with a personal server is refused \
         `{hosted_elsewhere}`: {:?}",
        refused.as_ref().map(|_| "seated")
    );
    let unrelated = Box::pin(second.spawn_spec(placed("plain", "member-2"))).await;
    assert!(
        !matches!(
            &unrelated,
            Err(meerkat_mob::MobError::WiringError(message)) if *message == hosted_elsewhere
        ),
        "a placed member without a personal server does not end at that check: {:?}",
        unrelated.as_ref().map(|_| "seated")
    );
    assert!(
        labels_of(&second, "member-1").await.is_none(),
        "the refused member is not seated"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The members' configuration of a launch that names a member auth binding:
/// a realm whose default binding is `a`, with the binding `b` the launch
/// names. The rows' model answers every turn, so no key is resolved.
const BOUND_REALM: &str = r#"
[realm.native-team]
default_binding = "a"

[realm.native-team.backend.openai_main]
provider = "openai"
backend_kind = "openai_api"

[realm.native-team.auth.key_a]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "SECOND_MOB_GUARDS_TEST_KEY_A" }

[realm.native-team.auth.key_b]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "SECOND_MOB_GUARDS_TEST_KEY_B" }

[realm.native-team.binding.a]
backend_profile = "openai_main"
auth_profile = "key_a"

[realm.native-team.binding.b]
backend_profile = "openai_main"
auth_profile = "key_b"
"#;

/// The binding `name` of the rows' realm, as a launch names it.
fn binding(name: &str) -> meerkat_core::AuthBindingRef {
    meerkat_core::AuthBindingRef {
        realm: meerkat_core::RealmId::parse("native-team").expect("realm id"),
        binding: meerkat_core::BindingId::parse(name).expect("binding id"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

/// A launch whose members all build with the binding `b` of
/// [`BOUND_REALM`], with its session service.
async fn launch_under_a_member_auth_binding(
    temp: &tempfile::TempDir,
) -> (UnifiedRuntime, Sessions) {
    let config: meerkat::Config = toml::from_str(BOUND_REALM).expect("the members' configuration");
    let of_the_members = Arc::new(config.clone());
    let (host, sessions, _machine, ()) = launch_composed(
        temp,
        definition("host"),
        config,
        |_| (),
        move |spec| {
            spec.with_member_model_config(of_the_members)
                .with_member_auth_binding(binding("b"))
        },
    )
    .await;
    (host, sessions)
}

/// The auth binding the session of the member `name` of `handle`'s mob
/// records: the one the member was built with.
async fn recorded_binding(
    sessions: &Sessions,
    handle: &MobHandle,
    name: &str,
) -> Option<meerkat_core::AuthBindingRef> {
    let session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(name))
        .await?;
    sessions
        .export_live_session(&session)
        .await
        .ok()?
        .try_session_metadata()
        .ok()
        .flatten()?
        .auth_binding
}

/// On a launch that names a member auth binding, a member of a second mob
/// is built with exactly that binding, as a member of the launch mob is: a
/// spawn that names no binding is given the launch's, and its session
/// records it.
#[tokio::test]
async fn a_member_of_a_second_mob_is_built_with_the_launchs_member_auth_binding() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, sessions) = launch_under_a_member_auth_binding(&temp).await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = handle_of(&host, &second_id).await;

    let in_the_launch_mob = Box::pin(host.mob_handle().spawn_spec(member("worker-a"))).await;
    assert!(
        in_the_launch_mob.is_ok(),
        "a member of the launch mob that names no binding: {:?}",
        in_the_launch_mob.as_ref().err()
    );
    assert_eq!(
        recorded_binding(&sessions, &host.mob_handle(), "worker-a").await,
        Some(binding("b")),
        "the launch mob's member is built with the launch's binding"
    );
    let in_the_second = Box::pin(second.spawn_spec(member("worker-b"))).await;
    assert!(
        in_the_second.is_ok(),
        "a member of the second mob that names no binding: {:?}",
        in_the_second.as_ref().err()
    );
    assert_eq!(
        recorded_binding(&sessions, &second, "worker-b").await,
        Some(binding("b")),
        "the second mob's member is built with the launch's binding"
    );
    shut_down(&host, &[&second_id]).await;
}

/// On a launch that names a member auth binding, a member of a second mob
/// that names another binding is refused, and nothing is built for it.
#[tokio::test]
async fn a_member_of_a_second_mob_that_names_another_auth_binding_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch_under_a_member_auth_binding(&temp).await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = handle_of(&host, &second_id).await;

    let refused =
        Box::pin(second.spawn_spec(member("rogue").with_auth_binding(binding("a")))).await;
    assert!(
        refused.is_err(),
        "a member of the second mob that names another binding is refused"
    );
    assert!(
        labels_of(&second, "rogue").await.is_none(),
        "and it is not seated"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The policy provider of the rows' launch.
const POLICY_PROVIDER: &str = "host";

/// A member whose tools the launch's policy provider governs: the spawn
/// names a policy of that provider, as a host that governs its members'
/// tools does.
fn governed_member(name: &str) -> SpawnMemberSpec {
    let mut spec = member(name);
    spec.application_tool_policy = Some(meerkat_core::ApplicationToolPolicyBinding::Provider {
        provider_id: meerkat_core::PolicyProviderId::new(POLICY_PROVIDER).expect("provider id"),
        policy_id: meerkat_core::PolicyId::new("member-tools").expect("policy id"),
    });
    spec
}

/// A member of a second mob is built under a policy of the launch's tool
/// consequence policy registry: a spawn that names a policy of the launch's
/// provider builds, in the second mob as in the launch mob. On a launch
/// without that registry the same spawn builds in neither.
#[tokio::test]
async fn a_member_of_a_second_mob_is_built_under_a_policy_of_the_launchs_registry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(
        meerkat_core::ToolConsequencePolicyRegistry::new(
            vec![Arc::new(HostPolicy(
                meerkat_core::PolicyProviderId::new(POLICY_PROVIDER).expect("provider id"),
            ))],
            meerkat_core::PolicyEvaluationSupervisorConfig::default(),
            None,
        )
        .expect("the launch's policy registry"),
    );
    let (host, _sessions, _machine, ()) = launch_composed(
        &temp,
        definition("host"),
        meerkat::Config::default(),
        |_| (),
        move |spec| spec.with_tool_consequence_policy_registry(registry),
    )
    .await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = handle_of(&host, &second_id).await;
    let in_the_launch_mob =
        Box::pin(host.mob_handle().spawn_spec(governed_member("worker-a"))).await;
    assert!(
        in_the_launch_mob.is_ok(),
        "a governed member of the launch mob builds: {:?}",
        in_the_launch_mob.as_ref().err()
    );
    let in_the_second = Box::pin(second.spawn_spec(governed_member("worker-b"))).await;
    assert!(
        in_the_second.is_ok(),
        "a governed member of the second mob builds: {:?}",
        in_the_second.as_ref().err()
    );
    shut_down(&host, &[&second_id]).await;

    let other_temp = tempfile::tempdir().expect("tempdir");
    let (ungoverned, _sessions) = launch(&other_temp, definition("host")).await;
    let other_id = second_mob(&ungoverned, definition("second")).await;
    let other = handle_of(&ungoverned, &other_id).await;
    let (in_the_launch_mob, in_the_second) = (
        Box::pin(
            ungoverned
                .mob_handle()
                .spawn_spec(governed_member("worker-a")),
        )
        .await,
        Box::pin(other.spawn_spec(governed_member("worker-b"))).await,
    );
    assert!(
        in_the_launch_mob.is_err() && in_the_second.is_err(),
        "without a registry a member that names a provider's policy builds in neither mob: \
         {:?}, {:?}",
        in_the_launch_mob.as_ref().map(|_| "built"),
        in_the_second.as_ref().map(|_| "built")
    );
    shut_down(&ungoverned, &[&other_id]).await;
}

/// A member of a second mob that is private to a person does not make an
/// agent of the launch mob with its name private to the launch's memory:
/// the launch's record names the second mob's member private where its mob
/// is asked, and its views for the memory, the one a dream reads included,
/// do not know that member at all.
#[tokio::test]
async fn a_private_member_of_a_second_mob_is_no_member_of_the_launchs_memory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = handle_of(&host, &second_id).await;
    let record = host.mob_runtime().private_owners().clone();
    let in_the_launch_mob = Box::pin(host.mob_handle().spawn_spec(member("desk"))).await;
    assert!(
        in_the_launch_mob.is_ok(),
        "the launch mob's agent, private to nobody: {:?}",
        in_the_launch_mob.as_ref().err()
    );
    let in_the_second = Box::pin(second.spawn_spec(private_member("desk"))).await;
    assert!(
        in_the_second.is_ok(),
        "a member of the second mob with the same name, private to a person: {:?}",
        in_the_second.as_ref().err()
    );
    assert_eq!(
        record.owner_of(second_id.as_str(), "desk").as_deref(),
        Some(OWNER),
        "the record knows the second mob's member as private, under its own mob"
    );

    assert_eq!(
        record.memory_members().get("desk"),
        Some(&None),
        "to the launch's memory `desk` is the launch mob's agent, private to nobody"
    );
    assert!(
        !record.memory_owners().contains_key("desk"),
        "and no owner is named for it"
    );
    for _ in 0..200 {
        if record.landed() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let left_out_of_a_dream = record
        .steward_source(&host.mob_handle())
        .private_identities()
        .await;
    assert!(
        left_out_of_a_dream
            .as_ref()
            .is_ok_and(|private| !private.contains("desk")),
        "a dream does not leave the launch mob's agent out: {left_out_of_a_dream:?}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The private identities `source` names, once it has read who the members
/// are private to: asked again, twenty seconds at most, while it answers
/// that it has not.
async fn private_identities_of(
    source: &dyn crate::memory::steward::PrivateMemberSource,
) -> Result<std::collections::BTreeSet<String>, String> {
    let mut answer = source.private_identities().await;
    for _ in 0..200 {
        if answer.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        answer = source.private_identities().await;
    }
    answer
}

/// The Steward's source of a second mob names that mob's private members,
/// and the launch mob's names the launch mob's. With a member of one name in
/// each mob, private to a person in the second mob and to nobody in the
/// launch mob, a dream of the second mob leaves that name's scope out and a
/// dream of the launch mob covers it; a member private to a person in the
/// launch mob alone is left out of the launch mob's dream and is nothing to
/// the second mob's.
#[tokio::test]
async fn the_scope_of_a_hosted_mobs_private_member_is_private_in_that_mobs_steward_source() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = handle_of(&host, &second_id).await;
    for seated in [
        Box::pin(host.mob_handle().spawn_spec(member("desk"))).await,
        Box::pin(host.mob_handle().spawn_spec(private_member("side"))).await,
        Box::pin(second.spawn_spec(private_member("desk"))).await,
    ] {
        assert!(seated.is_ok(), "a member is seated: {:?}", seated.err());
    }

    let of_the_launch = host.mob_runtime().private_member_source();
    let of_the_second = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob resolves")
        .mob_runtime()
        .private_member_source();
    assert_eq!(
        private_identities_of(of_the_second.as_ref()).await,
        Ok(std::collections::BTreeSet::from(["desk".to_string()])),
        "a dream of the second mob leaves its own private member out, and no member of the \
         launch mob"
    );
    assert_eq!(
        private_identities_of(of_the_launch.as_ref()).await,
        Ok(std::collections::BTreeSet::from(["side".to_string()])),
        "a dream of the launch mob leaves its own private member out, and covers its agent \
         of the second mob's member's name"
    );
    drop(second);
    shut_down(&host, &[&second_id]).await;
}

/// What a row of the retirements reads of `host`'s record, once the record
/// has read the launch mob and the second mob and both mobs' memory.
struct RetirementsRead {
    of_the_launch: BTreeMap<String, Option<String>>,
    of_the_second: BTreeMap<String, Option<String>>,
    /// The record's answer for `clerk`, a name only the second mob's memory
    /// names, asked for the launch mob.
    clerk_in_the_launch_mob: crate::mob_announcements::MemberPerson,
    /// The record's answer for `ledger`, a name only the launch mob's
    /// memory names, asked for the second mob.
    ledger_in_the_second_mob: crate::mob_announcements::MemberPerson,
}

/// Bind the memory of each mob of `host` as its composition does (the
/// launch mob's through the record, the second mob's through that mob's own
/// runtime), wait for the record's reads, and read its views.
async fn retirements_read(
    host: &UnifiedRuntime,
    second_id: &MobId,
    of_the_launch: &Arc<crate::memory::sqlite_store::SqliteAgentMemoryStore>,
    of_the_second: &Arc<crate::memory::sqlite_store::SqliteAgentMemoryStore>,
    second_realm: &str,
) -> RetirementsRead {
    use crate::mob_announcements::MemberPerson;

    let record = host.mob_runtime().private_owners().clone();
    let launch_id = host.mob_handle().mob_id().clone();
    record.bind_private_retirements(of_the_launch.clone(), "default".to_string());
    host.resolve_mob(Some(second_id))
        .await
        .expect("the second mob resolves")
        .mob_runtime()
        .bind_private_retirements(of_the_second.clone(), second_realm.to_string());
    let private_to_someone =
        || MemberPerson::Private(crate::access::UNKNOWN_PRIVATE_OWNER.to_string());
    for _ in 0..200 {
        if record.landed()
            && record.person_of(launch_id.as_str(), "ledger") == private_to_someone()
            && record.person_of(second_id.as_str(), "clerk") == private_to_someone()
            && record.owner_of(second_id.as_str(), "desk").is_some()
            && record.memory_members().contains_key("desk")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    RetirementsRead {
        of_the_launch: record.memory_members(),
        of_the_second: record.memory_members_of(second_id.as_str()),
        clerk_in_the_launch_mob: record.person_of(launch_id.as_str(), "clerk"),
        ledger_in_the_second_mob: record.person_of(second_id.as_str(), "ledger"),
    }
}

/// A private retirement that one mob's memory recorded is read for that mob
/// alone, and so it is after a restart. The launch mob has an agent `desk`
/// private to nobody, and its memory recorded the retirement of a private
/// member `ledger`. The second mob has a member `desk` private to a person,
/// and its memory recorded the retirement of an earlier private member
/// `desk` and of a private member `clerk`. The launch mob's views hold its
/// agent as private to nobody and `ledger` as private to a person, and
/// nothing of `clerk`; the second mob's views hold `desk` and `clerk` as
/// private and nothing of `ledger`; and neither mob's answer for a member
/// the record does not hold names the other mob's retirement.
#[tokio::test]
async fn a_private_retirement_one_mobs_memory_recorded_is_read_for_that_mob_alone() {
    use crate::memory::capabilities::StewardStore as _;

    let stored = StoredHost::new();
    let memory = tempfile::tempdir().expect("a directory for the two memory stores");
    let store_in = |name: &str| {
        Arc::new(
            crate::memory::sqlite_store::SqliteAgentMemoryStore::open(memory.path().join(name))
                .expect("a memory store"),
        )
    };
    let (of_the_launch, of_the_second) = (store_in("launch"), store_in("second"));

    let host = stored.launch().await;
    let second_id = second_mob(&host, definition("second")).await;
    let second_realm = meerkat_core::connection::mob_realm_id(second_id.as_str())
        .expect("the second mob has a realm of its own")
        .as_str()
        .to_string();
    let second = handle_of(&host, &second_id).await;
    for seated in [
        Box::pin(host.mob_handle().spawn_spec(member("desk"))).await,
        Box::pin(second.spawn_spec(private_member("desk"))).await,
    ] {
        assert!(seated.is_ok(), "a member is seated: {:?}", seated.err());
    }
    drop(second);
    of_the_launch
        .record_private_retirement("default", "ledger", None, "retired", 5)
        .await
        .expect("the launch mob's memory records a private retirement");
    for name in ["desk", "clerk"] {
        of_the_second
            .record_private_retirement(&second_realm, name, None, "retired", 5)
            .await
            .expect("the second mob's memory records a private retirement");
    }

    let read = retirements_read(
        &host,
        &second_id,
        &of_the_launch,
        &of_the_second,
        &second_realm,
    )
    .await;
    each_mobs_views_are_its_own(&read, "in the launch that seated the members");
    shut_down(&host, &[]).await;
    drop(host);

    let host = stored.launch().await;
    let read = retirements_read(
        &host,
        &second_id,
        &of_the_launch,
        &of_the_second,
        &second_realm,
    )
    .await;
    each_mobs_views_are_its_own(&read, "after a restart");
    shut_down(&host, &[]).await;
}

/// What [`a_private_retirement_one_mobs_memory_recorded_is_read_for_that_mob_alone`]
/// holds of `read`, `when` it was read.
fn each_mobs_views_are_its_own(read: &RetirementsRead, when: &str) {
    use crate::mob_announcements::MemberPerson;

    let someone = || Some(crate::access::UNKNOWN_PRIVATE_OWNER.to_string());
    assert_eq!(
        read.of_the_launch,
        BTreeMap::from([
            ("desk".to_string(), None),
            ("ledger".to_string(), someone()),
        ]),
        "the launch mob's views, {when}"
    );
    assert_eq!(
        read.of_the_second,
        BTreeMap::from([
            ("clerk".to_string(), someone()),
            ("desk".to_string(), Some(OWNER.to_string())),
        ]),
        "the second mob's views, {when}"
    );
    assert_eq!(
        read.clerk_in_the_launch_mob,
        MemberPerson::NotKnown,
        "a retirement the second mob's memory recorded names no member of the launch mob, \
         {when}"
    );
    assert_eq!(
        read.ledger_in_the_second_mob,
        MemberPerson::NotKnown,
        "a retirement the launch mob's memory recorded names no member of the second mob, \
         {when}"
    );
}

/// The create request of a member `name` of the mob `mob_id`, as the mob
/// hands it to the launch's session service.
fn member_create(mob_id: &str, name: &str) -> meerkat_core::service::CreateSessionRequest {
    meerkat_core::service::CreateSessionRequest {
        model: "gpt-5.5".to_string(),
        prompt: meerkat_core::ContentInput::Text("hi".to_string()),
        injected_context: Vec::new(),
        system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
        build: Some(meerkat_core::service::SessionBuildOptions {
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: mob_id.to_string(),
                role: "worker".to_string(),
                member: name.to_string(),
            }),
            ..meerkat_core::service::SessionBuildOptions::default()
        }),
        labels: None,
    }
}

/// The build of a second mob's member takes no part in what the launch's
/// memory observes of a member's model calls. Every member build carries
/// the launch's observing client, and which memory it marks is asked at
/// each model call for the mob the build is bound to: a model call of a
/// second mob's member that takes in content from outside marks nobody in
/// the launch's memory, and the same call of the launch mob's member of
/// the same name marks that member there.
#[tokio::test]
async fn the_build_of_a_second_mobs_member_takes_no_part_in_what_the_launchs_memory_observes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let slot = Arc::new(Mutex::new(None));
    let kept = Arc::clone(&slot);
    let (host, _sessions, _machine, ()) = launch_composed(
        &temp,
        definition("host"),
        meerkat::Config::default(),
        |_| (),
        move |spec| {
            *kept.lock().unwrap_or_else(PoisonError::into_inner) = Some(spec.dispatch_taint_slot());
            spec
        },
    )
    .await;
    let slot = slot
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .expect("the launch's slot");
    let of_the_launch = crate::SessionTaintTracker::new(crate::ContentTrustConfig::default());
    slot.fill(of_the_launch.clone());
    let launch_id = host.mob_handle().mob_id().clone();
    // The launch composes the second mob here, through its mob set.
    let second_id = second_mob(&host, definition("second")).await;

    let (mut of_the_launch_mob, mut of_the_second) = (
        member_create(launch_id.as_str(), "desk"),
        member_create(second_id.as_str(), "desk"),
    );
    crate::memory::dispatch_taint::attach_member_taint_decorator(&mut of_the_launch_mob, &slot);
    crate::memory::dispatch_taint::attach_member_taint_decorator(&mut of_the_second, &slot);
    let observed = |request: &meerkat_core::service::CreateSessionRequest| {
        request
            .build
            .as_ref()
            .is_some_and(|build| build.agent_llm_client_decorator.is_some())
    };
    assert!(
        observed(&of_the_launch_mob) && observed(&of_the_second),
        "every member build gets the launch's observing client"
    );
    super::mob_memory_firewall_tests::a_model_call_that_takes_in_outside_content(of_the_second)
        .await;
    assert!(
        of_the_launch.identity_taint("desk").is_none(),
        "what a member of the second mob took in marks nobody in the launch's memory"
    );
    super::mob_memory_firewall_tests::a_model_call_that_takes_in_outside_content(of_the_launch_mob)
        .await;
    assert!(
        of_the_launch.identity_taint("desk").is_some(),
        "what the launch mob's member of the same name took in marks it in the launch's memory"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The tool the rows' model asks for; its result is content from outside
/// the host.
const TAKES_IN: &str = "fetch";

/// A member's own tool whose result is content from outside the host.
struct TakesIn;

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for TakesIn {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        vec![Arc::new(meerkat_core::types::ToolDef::new(
            TAKES_IN,
            "Takes in content from outside the host.",
            serde_json::json!({"type": "object", "properties": {}}),
        ))]
        .into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(meerkat_core::ToolDispatchOutcome::sync_result(
            meerkat_core::types::ToolResult::new(
                call.id.to_string(),
                "a page from outside".to_string(),
                false,
            ),
        ))
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities::default()
    }
}

/// A model that asks for [`TAKES_IN`] on a conversation's first call and
/// answers on every call that carries a tool's result. It counts its
/// answers, so a row can wait for the end of a turn that took the tool's
/// result in.
#[derive(Default)]
struct TakesInThenAnswers {
    answers: std::sync::atomic::AtomicUsize,
}

impl TakesInThenAnswers {
    /// Whether the model has answered `turns` times, given thirty seconds.
    async fn answered(&self, turns: usize) -> bool {
        for _ in 0..300 {
            if self.answers.load(std::sync::atomic::Ordering::SeqCst) >= turns {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        false
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for TakesInThenAnswers {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let carries_a_result = request
            .messages
            .iter()
            .any(|message| matches!(message, meerkat_core::Message::ToolResults { .. }));
        let (lead, stop_reason) = if carries_a_result {
            self.answers
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            (
                meerkat_client::LlmEvent::TextDelta {
                    delta: "noted".to_string(),
                    meta: None,
                },
                meerkat_core::StopReason::EndTurn,
            )
        } else {
            (
                meerkat_client::LlmEvent::ToolCallComplete {
                    id: "call-takes-in".to_string(),
                    name: TAKES_IN.to_string(),
                    args: serde_json::json!({}),
                    meta: None,
                },
                meerkat_core::StopReason::ToolUse,
            )
        };
        let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
            request,
            meerkat_core::Provider::OpenAI,
            stop_reason,
        );
        Box::pin(futures::stream::iter(vec![Ok(lead), Ok(usage), Ok(done)]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// A definition under an id of its own with one profile, `worker`, whose
/// members run their own turns and take what a host sends them.
fn definition_of_members_a_host_sends_to(tag: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"second-mob-guards-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\nruntime_mode = \"autonomous_host\"\nexternal_addressable = true\n\n[profiles.worker.tools]\ncomms = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition of members a host sends to")
}

/// A member `name` of the profile `worker` that holds [`TakesIn`].
fn member_that_takes_in(name: &str) -> SpawnMemberSpec {
    let mut member = SpawnMemberSpec::new("worker", name);
    member.external_tools = Some(Arc::new(TakesIn));
    member
}

/// The launch's memory is the launch mob's, end to end. A member of a
/// second mob, under the name of a member of the launch mob, runs a turn
/// that takes in a tool's result from outside: the launch's memory holds
/// no mark under that name afterwards. The same turn of the launch mob's
/// own member of that name is marked, so the fixture's model, tool and
/// memory do mark a member.
#[tokio::test]
async fn a_turn_of_a_second_mobs_member_marks_nobody_in_the_launchs_memory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tracker = crate::SessionTaintTracker::new(crate::ContentTrustConfig::default());
    let model = Arc::new(TakesInThenAnswers::default());
    let (of_the_launch, of_the_members) = (tracker.clone(), Arc::clone(&model));
    let (host, _sessions, _machine, ()) = launch_composed(
        &temp,
        definition_of_members_a_host_sends_to("host"),
        meerkat::Config::default(),
        |_| (),
        move |spec| {
            spec.dispatch_taint_slot().fill(of_the_launch);
            spec.with_options(crate::mob_handle_runtime::MobBootstrapOptions {
                allow_ephemeral_sessions: false,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(of_the_members),
            })
        },
    )
    .await;
    let second_id = second_mob(&host, definition_of_members_a_host_sends_to("second")).await;
    let second = handle_of(&host, &second_id).await;

    let in_the_second = Box::pin(second.spawn_spec(member_that_takes_in("helper"))).await;
    assert!(
        in_the_second.is_ok(),
        "a member of the second mob: {:?}",
        in_the_second.as_ref().err()
    );
    let sent = crate::send_message_on_mob(
        &second,
        "helper",
        "fetch the page and say what it holds".to_string(),
    )
    .await;
    assert!(
        sent.is_ok(),
        "the second mob's member is sent its turn: {:?}",
        sent.as_ref().err()
    );
    assert!(
        model.answered(1).await,
        "the second mob's member took the tool's result in and answered"
    );
    assert!(
        tracker.identity_taint("helper").is_none(),
        "what a member of a second mob took in marks nobody in the launch's memory"
    );

    let in_the_launch_mob =
        Box::pin(host.mob_handle().spawn_spec(member_that_takes_in("helper"))).await;
    assert!(
        in_the_launch_mob.is_ok(),
        "the launch mob's member of the same name: {:?}",
        in_the_launch_mob.as_ref().err()
    );
    let sent = crate::send_message_on_mob(
        &host.mob_handle(),
        "helper",
        "fetch the page and say what it holds".to_string(),
    )
    .await;
    assert!(
        sent.is_ok(),
        "the launch mob's member is sent its turn: {:?}",
        sent.as_ref().err()
    );
    // The mark is waited for itself. The model's count of answers is both
    // members': a later call of the second mob's member would be counted
    // as the launch mob's member's answer.
    let mut marked = false;
    for _ in 0..300 {
        if tracker.identity_taint("helper").is_some() {
            marked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        marked,
        "what the launch mob's own member took in marks it in the launch's memory"
    );
    shut_down(&host, &[&second_id]).await;
}
