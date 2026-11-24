//! Rows of the identity plane of a mob a host serves beside its default mob.
//!
//! The fixture is the launch of the entries' rows: a launch that declares
//! its mob to its mob set and gives its default mob an identity context, as
//! the gateways do. What the gateways do for a second mob is said to the
//! runtime once, by a composer; the rows give it one that stands for a
//! launch's: one roster provider for every mob of the host, asked with the
//! definition of the mob it answers for.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use meerkat_mob::{MobId, SpawnMemberSpec};

use super::mob_entries::lazy_start_tests::{StoredHost, asleep, known};
use super::mob_entries_tests::{
    configured_agent, definition, identity_context, launch, launch_composed, launch_keeping,
    launch_over_opened_storage, launch_over_opened_storage_keeping, second_mob, set_of, shut_down,
};
use super::mob_identity_plane::{ComposedMobIdentity, HostMobIdentityComposer};
use super::{IdentityRequestServices, UnifiedRuntime};
use crate::gateway_wiring::HostMobIdentities;
use crate::identity_first::contracts::LeaseProvider;
use crate::identity_first::{
    AgentBuildContext, AgentBuildDraft, AgentCustomizer, AgentRuntimeServices, ContinuityStore,
    CustomizerError, DurabilityPolicy, DurableAgentSpec, IdentityBootstrapMode,
    IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig, IdentityRuntimeError,
    LocalContinuityStore, LocalExternalToolOverlay, LocalLeaseProvider, RosterContext, RosterError,
    RosterProvider, SessionRepairScope,
};
use crate::mob_handle_runtime::MobRuntime;
use crate::runtime::PersistentMetadataStore;

/// The roster provider of a launch: one for every mob of the host. It
/// answers the roster configured for the mob whose definition it is asked
/// with, and keeps the ids of the mobs it was asked for.
#[derive(Default)]
struct LaunchRoster {
    by_mob: Mutex<BTreeMap<MobId, Vec<DurableAgentSpec>>>,
    asked_for: Mutex<Vec<Option<MobId>>>,
}

impl LaunchRoster {
    fn configure(&self, mob_id: &MobId, agents: &[&str]) {
        self.by_mob
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                mob_id.clone(),
                agents.iter().map(|name| configured_agent(name)).collect(),
            );
    }

    fn asked_for(&self) -> Vec<Option<MobId>> {
        self.asked_for
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl RosterProvider for LaunchRoster {
    async fn roster(&self, context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        let mob_id = context
            .mob_definition
            .as_ref()
            .map(|definition| definition.id.clone());
        self.asked_for
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(mob_id.clone());
        Ok(mob_id
            .and_then(|mob_id| {
                self.by_mob
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&mob_id)
                    .cloned()
            })
            .unwrap_or_default())
    }
}

/// What stands for a launch's composer of its other mobs' identity planes:
/// for each mob it is asked for, an identity runtime over that mob's own
/// handle with the launch's one roster provider and that mob's definition.
struct LaunchComposer {
    roster: Arc<LaunchRoster>,
    /// The mobs it was asked for, in order.
    composed_for: Mutex<Vec<MobId>>,
    /// Whether the next ask fails, once.
    fails_next: AtomicBool,
    /// Whether every ask fails, for as long as it is set.
    refuses: AtomicBool,
    /// Whether the launch builds an agent only of a composed mob, as a
    /// launch whose recall failure policy is `fail` does.
    only_of_composed_mobs: AtomicBool,
    /// Whether an ask waits, once it is counted, until the row lets it go
    /// ([`Self::lets_go`]).
    holds: AtomicBool,
    let_go: tokio::sync::Notify,
    /// A composer that tells the identity runtime it composes to keep its
    /// records under this mob id in this store: what a composer written
    /// without care for the mob would do with the launch's own id.
    told_records: Option<(Arc<dyn PersistentMetadataStore>, String)>,
}

impl LaunchComposer {
    fn over(roster: &Arc<LaunchRoster>) -> Arc<Self> {
        Arc::new(Self {
            roster: Arc::clone(roster),
            composed_for: Mutex::default(),
            fails_next: AtomicBool::new(false),
            refuses: AtomicBool::new(false),
            only_of_composed_mobs: AtomicBool::new(false),
            holds: AtomicBool::new(false),
            let_go: tokio::sync::Notify::new(),
            told_records: None,
        })
    }

    fn telling_records(
        roster: &Arc<LaunchRoster>,
        store: Arc<dyn PersistentMetadataStore>,
        mob_id: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            roster: Arc::clone(roster),
            composed_for: Mutex::default(),
            fails_next: AtomicBool::new(false),
            refuses: AtomicBool::new(false),
            only_of_composed_mobs: AtomicBool::new(false),
            holds: AtomicBool::new(false),
            let_go: tokio::sync::Notify::new(),
            told_records: Some((store, mob_id)),
        })
    }

    /// The asks that wait go on, and no later ask waits.
    fn lets_go(&self) {
        self.holds.store(false, Ordering::SeqCst);
        self.let_go.notify_waiters();
    }

    fn composed_for(&self) -> Vec<MobId> {
        self.composed_for
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl HostMobIdentityComposer for LaunchComposer {
    fn builds_agents_only_of_composed_mobs(&self) -> bool {
        self.only_of_composed_mobs.load(Ordering::SeqCst)
    }

    async fn compose(
        &self,
        mob: &MobRuntime,
        _console_events: Option<super::ConsoleEventStore>,
    ) -> Result<ComposedMobIdentity, crate::unified_runtime::MobIdentityNotComposed> {
        self.composed_for
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(mob.mob_id().clone());
        while self.holds.load(Ordering::SeqCst) {
            let let_go = self.let_go.notified();
            if !self.holds.load(Ordering::SeqCst) {
                break;
            }
            let_go.await;
        }
        if self.fails_next.swap(false, Ordering::SeqCst) || self.refuses.load(Ordering::SeqCst) {
            return Err(IdentityRuntimeError::Internal(
                "the launch could not compose this mob's identity plane".to_string(),
            )
            .into());
        }
        let handle = mob.handle().observer();
        let identity_runtime = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    LocalContinuityStore::in_memory().expect("continuity store"),
                ),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: format!("mob-identity-plane-{}", mob.mob_id()),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(crate::gateway_wiring::identity_session_bridge(
                    mob,
                    handle.clone(),
                ))),
                default_timeout: None,
            })
            .with_runtime_services(AgentRuntimeServices::new(handle.clone())),
        );
        if let Some((store, mob_id)) = &self.told_records {
            identity_runtime.set_identity_records(Arc::clone(store), mob_id.clone());
        }
        Ok(ComposedMobIdentity {
            context: Arc::new(IdentityFirstRuntimeContext::new(
                identity_runtime,
                Arc::clone(&self.roster) as Arc<dyn RosterProvider>,
                None,
                None,
                Some(handle.definition().clone()),
            )),
            request_services: IdentityRequestServices::default(),
            served_memory: None,
        })
    }
}

/// A launch whose default mob has its identity context, with the context.
async fn launch_with_identities(
    temp: &tempfile::TempDir,
) -> (UnifiedRuntime, Arc<IdentityFirstRuntimeContext>) {
    let (mut host, _sessions) = launch(temp, definition("host")).await;
    let of_the_default = identity_context(host.mob_runtime(), "default");
    Box::pin(host.install_and_bootstrap_identity_first_context(
        Arc::clone(&of_the_default),
        &[configured_agent("desk"), configured_agent("front")],
    ))
    .await
    .expect("the default mob's identity bootstrap");
    (host, of_the_default)
}

/// The identity runtime of the entry `host` holds for the mob `named`,
/// read from the table without a request: waited for, for twenty seconds.
async fn identity_runtime_of_the_entry(
    host: &UnifiedRuntime,
    named: &MobId,
) -> Option<Arc<IdentityRuntime>> {
    for _ in 0..200 {
        let attached = host
            .mob_entry_in_tests(named)
            .and_then(|entry| entry.identity_runtime_ref().cloned());
        if attached.is_some() {
            return attached;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

/// The public names of the members the mob `mob_id` of `host`'s set holds.
async fn members_of(host: &UnifiedRuntime, mob_id: &MobId) -> std::collections::BTreeSet<String> {
    let handle = set_of(host)
        .handle_for(mob_id)
        .await
        .expect("the set serves the mob");
    handle
        .list_members_including_retiring()
        .await
        .iter()
        .map(|member| {
            crate::member_comms_id::runtime_alias_str(member.agent_identity.as_str()).into_owned()
        })
        .collect()
}

fn identity(name: &str) -> crate::identity_first::AgentIdentity {
    crate::identity_first::AgentIdentity::parse(name).expect("identity")
}

/// A second mob of a launch that composes identities has an identity
/// runtime of its own once it resolves, with no call that attaches one: the
/// launch's composer was asked for it, once, with that mob's own wrapper.
/// It is not the default mob's, the mob's wrapper answers it, a second
/// resolve asks the composer nothing, and the default mob keeps the
/// identity runtime its launch installed.
#[tokio::test]
async fn a_second_mob_of_a_gateway_gets_an_identity_runtime_of_its_own_at_its_build() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let composer = LaunchComposer::over(&roster);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    let second_id = second_mob(&host, definition("second")).await;

    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        let of_the_second = mob.identity_runtime_ref().cloned();
        assert!(
            of_the_second.is_some(),
            "a second mob of a launch that composes identities has an identity runtime when \
             it resolves"
        );
        assert!(
            !of_the_second
                .as_ref()
                .is_some_and(|runtime| Arc::ptr_eq(runtime, &of_the_default.runtime)),
            "and it is not the default mob's"
        );
        assert!(
            mob.mob_runtime()
                .installed_identity_runtime()
                .zip(of_the_second.clone())
                .is_some_and(|(of_the_plane, of_the_entry)| Arc::ptr_eq(
                    &of_the_plane,
                    &of_the_entry
                )),
            "the mob's wrapper answers the same identity runtime"
        );
        let again = host.resolve_mob(Some(&second_id)).await;
        assert!(
            again.is_ok_and(|mob| mob
                .identity_runtime_ref()
                .zip(of_the_second.as_ref())
                .is_some_and(|(now, before)| Arc::ptr_eq(now, before))),
            "a second resolve answers the same identity runtime"
        );
    }
    assert_eq!(
        composer.composed_for(),
        vec![second_id.clone()],
        "the launch's composer was asked once, for the second mob alone"
    );
    assert!(
        host.resolve_mob(None).await.is_ok_and(|mob| mob
            .identity_runtime_ref()
            .is_some_and(|runtime| Arc::ptr_eq(runtime, &of_the_default.runtime))),
        "the default mob keeps the identity runtime its launch installed"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The launch's one roster provider is asked for a second mob with that
/// mob's own definition, and the agents it answers are seated in that mob:
/// members of that mob's roster, known to that mob's identity runtime, and
/// neither in the default mob nor known to its identity runtime.
#[tokio::test]
async fn the_roster_callback_is_asked_for_the_second_mobs_definition_and_its_agents_are_seated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let composer = LaunchComposer::over(&roster);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&host, of_the_second).await;

    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    assert_eq!(
        roster.asked_for(),
        vec![Some(second_id.clone())],
        "the roster provider was asked once, with the second mob's definition"
    );
    let in_the_second = members_of(&host, &second_id).await;
    assert!(
        in_the_second.contains("desk") && in_the_second.contains("back"),
        "the agents the roster answers for the second mob are seated in it: {in_the_second:?}"
    );
    assert!(
        !in_the_second.contains("front"),
        "an agent of the default mob is not seated in the second: {in_the_second:?}"
    );
    let in_the_default = members_of(&host, host.mob_handle().mob_id()).await;
    assert!(
        in_the_default.contains("desk") && !in_the_default.contains("back"),
        "the default mob keeps its own agents alone: {in_the_default:?}"
    );
    if let Ok(mob) = resolved {
        let known = match mob.identity_runtime_ref() {
            Some(runtime) => runtime.registered_spec(&identity("back")).await.is_ok(),
            None => false,
        };
        assert!(known, "the second mob's identity runtime knows its agent");
    }
    assert!(
        matches!(
            of_the_default
                .runtime
                .registered_spec(&identity("back"))
                .await,
            Err(IdentityRuntimeError::UnknownIdentity(_))
        ),
        "the default mob's identity runtime knows no agent of the second mob"
    );
    shut_down(&host, &[&second_id]).await;
}

/// An entry the runtime builds by following its mob set, with no request
/// that names the mob, has its identity plane: the composer was asked and
/// the mob's agents are seated without a resolve.
#[tokio::test]
async fn an_entry_built_by_the_follower_without_a_request_has_its_identity_plane() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let composer = LaunchComposer::over(&roster);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["back"]);
    let second_id = second_mob(&host, of_the_second).await;

    let attached = identity_runtime_of_the_entry(&host, &second_id).await;
    assert!(
        attached.is_some(),
        "the entry of a mob no request named has an identity runtime"
    );
    let mut seated = members_of(&host, &second_id).await;
    for _ in 0..200 {
        if seated.contains("back") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        seated = members_of(&host, &second_id).await;
    }
    assert!(
        seated.contains("back"),
        "and the mob's agents are seated without a request: {seated:?}"
    );
    assert_eq!(composer.composed_for(), vec![second_id.clone()]);
    shut_down(&host, &[&second_id]).await;
}

/// A mob the set built for the host before the launch said how it composes
/// identities (a stored mob the set restored at the start) gets its
/// identity plane when the launch says so, with no request.
#[tokio::test]
async fn a_mob_built_before_the_launch_composes_identities_gets_its_identity_plane_then() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let second_id = second_mob(&host, definition("second")).await;
    for _ in 0..200 {
        if host.mob_entry_in_tests(&second_id).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        host.mob_entry_in_tests(&second_id)
            .is_some_and(|entry| entry.identity_runtime_ref().is_none()),
        "the entry of the second mob is built, with no identity runtime before the launch \
         composes identities"
    );

    let roster = Arc::new(LaunchRoster::default());
    let composer = LaunchComposer::over(&roster);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    assert!(
        identity_runtime_of_the_entry(&host, &second_id)
            .await
            .is_some(),
        "the entry built before has an identity runtime once the launch composes identities"
    );
    assert_eq!(composer.composed_for(), vec![second_id.clone()]);
    shut_down(&host, &[&second_id]).await;
}

/// A composition that fails leaves the mob served, without an identity
/// plane, and is asked again by the next request that names the mob.
#[tokio::test]
async fn a_composition_that_fails_leaves_the_mob_served_and_is_tried_again() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let composer = LaunchComposer::over(&roster);
    composer.fails_next.store(true, Ordering::SeqCst);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    let second_id = second_mob(&host, definition("second")).await;

    // The first ask, the follower's or a request's, fails; every resolve
    // answers the mob.
    let mut composed = None;
    for _ in 0..200 {
        let resolved = host.resolve_mob(Some(&second_id)).await;
        assert!(
            resolved.is_ok(),
            "the second mob stays served: {:?}",
            resolved.as_ref().err()
        );
        composed = resolved
            .ok()
            .and_then(|mob| mob.identity_runtime_ref().cloned());
        if composed.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        composed.is_some(),
        "a later request finds the identity plane composed"
    );
    assert_eq!(
        composer.composed_for(),
        vec![second_id.clone(), second_id.clone()],
        "the composer was asked again after its failure, and no more"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The identity records of a second mob's identity runtime are kept in the
/// host's metadata store under that mob's own id, whatever its composer
/// told the identity runtime: two mobs with an agent of one name each keep
/// their own tool record.
#[tokio::test]
async fn a_second_mobs_identity_records_are_kept_under_its_own_mob_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, of_the_default) = launch_with_identities(&temp).await;
    let default_id = host.mob_id();
    let store = Arc::clone(host.persistent_metadata());
    of_the_default
        .runtime
        .set_identity_records(Arc::clone(&store), default_id.clone());
    let roster = Arc::new(LaunchRoster::default());
    // A composer that tells every identity runtime the launch's mob id.
    let composer = LaunchComposer::telling_records(&roster, Arc::clone(&store), default_id.clone());
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk"]);
    let second_id = second_mob(&host, of_the_second).await;

    let resolved = host.resolve_mob(Some(&second_id)).await;
    let second_runtime = resolved
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "the second mob has an identity runtime"
    );
    let desk = identity("desk");
    let wrote_default = of_the_default
        .runtime
        .write_built_tools(
            &desk,
            &meerkat_core::types::SessionId::new(),
            vec!["of_the_default".to_string()],
        )
        .await;
    assert!(wrote_default.is_ok(), "the default mob's record is written");
    if let Some(second_runtime) = second_runtime {
        let wrote_second = second_runtime
            .write_built_tools(
                &desk,
                &meerkat_core::types::SessionId::new(),
                vec!["of_the_second".to_string()],
            )
            .await;
        assert!(wrote_second.is_ok(), "the second mob's record is written");
    }
    let (of_the_default_desk, of_the_second_desk) = (
        store
            .identity_tool_record(&default_id, "desk")
            .await
            .expect("the store reads"),
        store
            .identity_tool_record(second_id.as_str(), "desk")
            .await
            .expect("the store reads"),
    );
    assert_eq!(
        of_the_default_desk.map(|record| record.tools),
        Some(vec!["of_the_default".to_string()]),
        "the default mob's agent keeps its own record"
    );
    assert_eq!(
        of_the_second_desk.map(|record| record.tools),
        Some(vec!["of_the_second".to_string()]),
        "the second mob's agent's record is kept under the second mob's id"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A mob the mob set came to manage after a private-owner record's seed
/// landed has its private members read: a record that built none of them
/// reads the mob's spawn log when it reads the mobs its set manages again.
#[tokio::test]
async fn a_mob_the_set_manages_after_the_seed_has_its_private_members_read() {
    const OWNER: &str = "console-caller:v1:aa";
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    // A record that builds no member, over the launch's mob set: what a
    // process that did not build the mob's members holds.
    let record = crate::private_owners::PrivateOwners::default();
    record.bind_child_mobs(&set_of(&host));
    let default_handle = host.mob_handle();
    for _ in 0..200 {
        if record.seeded(&default_handle).await.landed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(record.landed(), "the record's seed landed");

    let second_id = second_mob(&host, definition("second")).await;
    // The member is seated through the mob's own handle, with the label
    // Meta writes for a member private to a person: a caller's spawn may
    // not supply it.
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the set serves the second mob");
    let seated = Box::pin(
        second.spawn_spec(
            SpawnMemberSpec::new("worker", "side").with_labels(BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                OWNER.to_string(),
            )])),
        ),
    )
    .await;
    assert!(
        seated.is_ok(),
        "a private member of the second mob: {:?}",
        seated.as_ref().err()
    );
    assert_eq!(
        record.owner_of(second_id.as_str(), "side"),
        None,
        "the record did not build the member, and its seed came before the mob"
    );
    let read = record.read_loaded_mobs().await;
    assert!(read.is_ok(), "the read of the loaded mobs: {read:?}");
    assert_eq!(
        record.owner_of(second_id.as_str(), "side").as_deref(),
        Some(OWNER),
        "a private member of a mob the set manages since the seed is private to its owner"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A mob the mob set came to manage after a record's seed landed, whose
/// durable records the record has not read yet: the record cannot say whose
/// its members are, and the identity gate's reader keeps each of them on
/// the closed side until the read has landed. A mob the record has never
/// read at all is kept there too (the row asks for an id no mob has: the
/// record does not tell such an id from a mob the set knows and has not
/// loaded). Once the mob is read, a member its log names a person for stays
/// on the closed side and a member it names none for does not.
#[tokio::test]
async fn the_members_of_a_mob_the_record_has_not_read_are_on_the_closed_side_of_the_gate() {
    use crate::identity_creation::PrivateMembers;

    const OWNER: &str = "console-caller:v1:aa";
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    // A record that builds no member, over the launch's mob set.
    let record = crate::private_owners::PrivateOwners::default();
    record.bind_child_mobs(&set_of(&host));
    let default_handle = host.mob_handle();
    for _ in 0..200 {
        if record.seeded(&default_handle).await.landed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(record.landed(), "the record's seed landed");

    let second_id = second_mob(&host, definition("second")).await;
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the set serves the second mob");
    for (member, labels) in [
        (
            "side",
            BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                OWNER.to_string(),
            )]),
        ),
        ("open", BTreeMap::new()),
    ] {
        let seated =
            Box::pin(second.spawn_spec(SpawnMemberSpec::new("worker", member).with_labels(labels)))
                .await;
        assert!(
            seated.is_ok(),
            "`{member}` of the second mob: {:?}",
            seated.as_ref().err()
        );
    }

    for member in ["side", "open"] {
        assert!(
            record.is_private(second_id.as_str(), member),
            "`{member}`: the mob's records are not read yet, the gate keeps its member on the \
             closed side"
        );
    }
    assert!(
        record.is_private("a-mob-that-was-never-read", "anyone"),
        "a mob the record has never read is kept on the closed side"
    );

    let read = record.read_loaded_mobs().await;
    assert!(read.is_ok(), "the read of the loaded mobs: {read:?}");
    assert!(
        record.is_private(second_id.as_str(), "side"),
        "the mob's log names a person for the member"
    );
    assert!(
        !record.is_private(second_id.as_str(), "open"),
        "the mob's log names no person for the member"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The launch's own record reads a mob the host creates: the launch's
/// composition marks the mob before the set builds it, and the record reads
/// the mob once the set manages it. From then on a name of that mob the
/// record does not hold is private to no one. A mob the record has never
/// read stays not known, and the launch's own mob is read by the seed.
#[tokio::test]
async fn the_launchs_record_reads_a_mob_the_host_creates() {
    use crate::mob_announcements::MemberPerson;

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let record = host.mob_runtime().private_owners().clone();
    record.first_attempts_settled(Duration::from_secs(30)).await;
    assert!(
        !record.first_read_pending(),
        "the launch's record has read its own mob"
    );

    let second_id = second_mob(&host, definition("second")).await;
    // The read of the mob is owed since the launch's composition was asked
    // for its builder: the wait ends when the record has read the mob.
    tokio::time::timeout(
        Duration::from_secs(30),
        record.read_of_the_mob_awaited(second_id.as_str(), Duration::from_hours(1)),
    )
    .await
    .expect("the launch's record reads the mob the host created");
    assert_eq!(
        record.person_for_a_reader(second_id.as_str(), "nobody"),
        MemberPerson::Shared,
        "a name of a mob that is read, which no record holds, is private to no one"
    );
    assert_eq!(
        record.person_for_a_reader("a-mob-that-was-never-read", "anyone"),
        MemberPerson::NotKnown,
        "a mob the record has never read"
    );
    assert_eq!(
        record.person_for_a_reader(host.mob_handle().mob_id().as_str(), "nobody"),
        MemberPerson::Shared,
        "the launch's own mob is read by the seed"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A read that is owed for a mob the set manages is made, also where the
/// record has read that mob before at the incarnation it has. A mob that
/// was unloaded and loaded again while no reading of the record saw it
/// gone keeps its incarnation: the mark the host set for the second load
/// must be cleared by the next reading, and not stay for ever.
#[tokio::test]
async fn a_mark_for_a_mob_the_record_has_read_before_is_cleared_by_the_next_reading() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let record = host.mob_runtime().private_owners().clone();
    record.first_attempts_settled(Duration::from_secs(30)).await;

    let second_id = second_mob(&host, definition("second")).await;
    tokio::time::timeout(
        Duration::from_secs(30),
        record.read_of_the_mob_awaited(second_id.as_str(), Duration::from_hours(1)),
    )
    .await
    .expect("the launch's record reads the mob the host created");

    // The host says it is about to build the mob again, as at a load after
    // an unload: the set manages the mob at the incarnation the record has
    // read, and nothing of the set changed between.
    record.owe_the_read_of(second_id.as_str());
    let read = record.read_loaded_mobs().await;
    assert!(read.is_ok(), "the reading of the loaded mobs: {read:?}");
    tokio::time::timeout(
        Duration::from_secs(5),
        record.read_of_the_mob_awaited(second_id.as_str(), Duration::from_hours(1)),
    )
    .await
    .expect("the owed read of a mob the set manages is made: its mark does not stay");
    shut_down(&host, &[&second_id]).await;
}

/// A mark for a mob the set does not manage (a load that failed before the
/// mob was built) is not cleared by a reading of the set: it stays until
/// that mob is read, and a wait for that mob ends at its bound. The mark is
/// that mob's own: the record owes no first read for it, a wait for a mob
/// the record has read is not held by it, and the names of the mobs the
/// record has read are answered as before.
#[tokio::test]
async fn a_mark_for_a_mob_the_set_does_not_manage_stays_and_holds_no_other_mob() {
    use crate::mob_announcements::MemberPerson;

    const NEVER_BUILT: &str = "a-mob-whose-load-failed";
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let record = host.mob_runtime().private_owners().clone();
    record.first_attempts_settled(Duration::from_secs(30)).await;

    let second_id = second_mob(&host, definition("second")).await;
    tokio::time::timeout(
        Duration::from_secs(30),
        record.read_of_the_mob_awaited(second_id.as_str(), Duration::from_hours(1)),
    )
    .await
    .expect("the launch's record reads the mob the host created");

    // The host says it is about to build a mob, and the build fails: the
    // set never manages it.
    record.owe_the_read_of(NEVER_BUILT);
    let read = record.read_loaded_mobs().await;
    assert!(read.is_ok(), "the reading of the loaded mobs: {read:?}");
    let bound = Duration::from_millis(300);
    let asked = std::time::Instant::now();
    record.read_of_the_mob_awaited(NEVER_BUILT, bound).await;
    assert!(
        asked.elapsed() + Duration::from_millis(50) >= bound,
        "the mark is not cleared by a reading of the set: a wait for the mob ends at its bound, \
         it ended after {:?}",
        asked.elapsed()
    );
    assert_eq!(
        record.person_for_a_reader(NEVER_BUILT, "anyone"),
        MemberPerson::NotKnown,
        "a name of the marked mob is not known"
    );

    assert!(
        !record.first_read_pending(),
        "the mark is the mob's own: the record owes no first read for it"
    );
    tokio::time::timeout(
        Duration::from_secs(5),
        record.read_of_the_mob_awaited(second_id.as_str(), Duration::from_hours(1)),
    )
    .await
    .expect("a wait for a mob the record has read is not held by another mob's mark");
    assert_eq!(
        record.person_for_a_reader(second_id.as_str(), "nobody"),
        MemberPerson::Shared,
        "a name of a mob that is read is answered as before"
    );
    assert_eq!(
        record.person_for_a_reader(host.mob_handle().mob_id().as_str(), "nobody"),
        MemberPerson::Shared,
        "and so is a name of the launch's own mob"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A host of one mob composes nothing: with a composer given, no mob is
/// asked for while the host has its default mob alone, and the runtime
/// holds no entry.
#[tokio::test]
async fn a_host_of_one_mob_composes_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let composer = LaunchComposer::over(&roster);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(composer.composed_for(), Vec::<MobId>::new());
    assert_eq!(roster.asked_for(), Vec::<Option<MobId>>::new());
    assert_eq!(host.mob_entries_in_tests(), Vec::<MobId>::new());
    assert!(
        host.resolve_mob(None).await.is_ok_and(|mob| mob
            .identity_runtime_ref()
            .is_some_and(|runtime| Arc::ptr_eq(runtime, &of_the_default.runtime))),
        "the default mob keeps the identity runtime its launch installed"
    );
    shut_down(&host, &[]).await;
}

/// A launch that does not say how it composes identities (a launch without
/// an identity plane) gives a second mob none: the mob resolves and has no
/// identity runtime.
#[tokio::test]
async fn a_launch_that_composes_no_identities_gives_a_second_mob_none() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let second_id = second_mob(&host, definition("second")).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok_and(|mob| mob.identity_runtime_ref().is_none()),
        "the second mob resolves without an identity runtime"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A door reads where a resolved mob's identity plane stands without
/// waiting: owed while the launch composes identities and this mob's is
/// not attached (here: its composition fails), attached once it is, and
/// not owed for a mob nothing composes one for (a launch that composes
/// none). The default mob with its identity context reads attached.
#[tokio::test]
async fn a_door_reads_where_a_second_mobs_identity_plane_stands_without_waiting() {
    use super::IdentityPlaneStanding;
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let composer = LaunchComposer::over(&roster);
    composer.refuses.store(true, Ordering::SeqCst);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    let second_id = second_mob(&host, definition("second")).await;
    let door = host.mob_resolver();

    let while_it_fails = door.resolve(Some(&second_id)).await;
    assert!(
        while_it_fails.is_ok(),
        "the second mob resolves while its composition fails: {:?}",
        while_it_fails.as_ref().err()
    );
    if let Ok(mob) = while_it_fails {
        assert_eq!(
            door.identity_plane_standing(&mob),
            IdentityPlaneStanding::Owed,
            "a mob whose composition failed is owed its identity plane"
        );
    }
    composer.refuses.store(false, Ordering::SeqCst);
    let composed = door.resolve(Some(&second_id)).await;
    assert!(
        composed
            .as_ref()
            .is_ok_and(|mob| door.identity_plane_standing(mob) == IdentityPlaneStanding::Attached),
        "a mob whose composition ran has its identity plane attached"
    );
    let default_mob = door.resolve(None).await;
    assert!(
        default_mob
            .as_ref()
            .is_ok_and(|mob| door.identity_plane_standing(mob) == IdentityPlaneStanding::Attached),
        "the default mob with its identity context reads attached"
    );
    shut_down(&host, &[&second_id]).await;

    let other_temp = tempfile::tempdir().expect("tempdir");
    let (plain, _sessions) = launch(&other_temp, definition("plain")).await;
    let third_id = second_mob(&plain, definition("third")).await;
    let plain_door = plain.mob_resolver();
    let resolved = plain_door.resolve(Some(&third_id)).await;
    assert!(
        resolved.as_ref().is_ok_and(
            |mob| plain_door.identity_plane_standing(mob) == IdentityPlaneStanding::NotOwed
        ),
        "a second mob of a launch that composes no identities is owed none"
    );
    shut_down(&plain, &[&third_id]).await;
}

/// A definition with one profile and one flow of one step, under an id of
/// its own.
fn definition_with_a_flow(tag: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"mob-identity-plane-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n[profiles.worker.tools]\ncomms = true\n\n[flows.review]\ndescription = \"review one change\"\n\n[flows.review.steps.review]\nrole = \"worker\"\nmessage = \"review\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition with one profile and one flow")
}

/// What an identity runtime answers a request with once its foreground
/// admission closed.
fn refused_as_shutting_down<T>(answer: &Result<T, IdentityRuntimeError>) -> bool {
    matches!(
        answer,
        Err(IdentityRuntimeError::Internal(text)) if text == "identity runtime is shutting down"
    )
}

/// A flow of a second mob passes that mob's own identity barrier before its
/// run is minted: the barrier that makes every agent the mob's identity
/// runtime registered a member of the mob. Shown where the barrier
/// refuses: once that identity runtime takes no request, a flow of the mob
/// is refused with the barrier's own error and no run starts.
#[tokio::test]
async fn a_flow_of_a_second_mob_passes_that_mobs_identity_barrier() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let composer = LaunchComposer::over(&roster);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    let of_the_second = definition_with_a_flow("second");
    roster.configure(&of_the_second.id, &["back"]);
    let second_id = second_mob(&host, of_the_second).await;
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "the second mob has an identity runtime"
    );
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the set serves the second mob");

    if let Some(second_runtime) = second_runtime {
        second_runtime.close_foreground_operations();
    }
    let run =
        Box::pin(second.run_flow(meerkat_mob::FlowId::from("review"), serde_json::json!({}))).await;
    assert!(
        matches!(
            &run,
            Err(meerkat_mob::MobError::Internal(text))
                if text.contains("identity-first flow membership failed")
        ),
        "a flow of the second mob is refused by the mob's own identity barrier: {run:?}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// Once the runtime shut down, the identity runtime of a second mob takes
/// no request: its foreground admission closed with the runtime's, as the
/// default mob's does, whatever became of the mob's actor.
#[tokio::test]
async fn the_identity_runtime_of_a_second_mob_takes_no_request_after_the_runtime_shut_down() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let composer = LaunchComposer::over(&roster);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["back"]);
    let second_id = second_mob(&host, of_the_second).await;
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "the second mob has an identity runtime"
    );
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the set serves the second mob");

    tokio::time::timeout(Duration::from_secs(30), host.shutdown())
        .await
        .expect("the runtime shuts down");
    if let Some(second_runtime) = second_runtime {
        let late = second_runtime
            // As a door sends: through the identity runtime's admission.
            .send_tracked(
                &identity("back"),
                &meerkat_core::ContentInput::Text("a word after the shutdown".to_string()),
            )
            .await;
        assert!(
            refused_as_shutting_down(&late),
            "a request to the second mob's identity runtime after the runtime shut down is \
             refused as shutting down: {:?}",
            late.as_ref().map(|_| "sent")
        );
    }
    let _ = second.shutdown().await;
}

/// The identity runtime of a second mob ends with the mob's entry: once the
/// mob set destroyed the mob and its entry went, a request to that identity
/// runtime is refused as shutting down, as after the runtime's own
/// shutdown.
#[tokio::test]
async fn the_identity_runtime_of_a_second_mob_ends_with_its_entry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let composer = LaunchComposer::over(&roster);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["back"]);
    let second_id = second_mob(&host, of_the_second).await;
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "the second mob has an identity runtime"
    );

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    for _ in 0..200 {
        if host.mob_entry_in_tests(&second_id).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        host.mob_entry_in_tests(&second_id).is_none(),
        "the entry of the destroyed mob went"
    );
    if let Some(second_runtime) = second_runtime {
        // The entry leaves the table before its end ran to its close of
        // the identity runtime: the answer is read until it is that close's.
        let mut late = Ok(());
        for _ in 0..50 {
            late = second_runtime
                // As a door sends: through the identity runtime's admission.
                .send_tracked(
                    &identity("back"),
                    &meerkat_core::ContentInput::Text("a word after the destroy".to_string()),
                )
                .await
                .map(|_| ());
            if refused_as_shutting_down(&late) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            refused_as_shutting_down(&late),
            "a request to the identity runtime of a mob whose entry went is refused as \
             shutting down: {late:?}"
        );
    }
    shut_down(&host, &[]).await;
}

/// A customizer of a launch that changes nothing. The rows tell it from
/// another by its address.
struct LaunchCustomizer;

#[async_trait::async_trait]
impl AgentCustomizer for LaunchCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }
}

/// The composer both gateways and the builder hand the runtime, over the
/// launch's one roster provider, with the values of a launch on a durable
/// store that is no candidate.
fn gateways_composer(
    roster: &Arc<LaunchRoster>,
    customizer: Option<Arc<dyn AgentCustomizer>>,
) -> HostMobIdentities {
    HostMobIdentities::new(Arc::clone(roster) as Arc<dyn RosterProvider>, true)
        .with_customizer(customizer)
}

/// The gateways' composer gives a second mob of the host its identity
/// plane over that mob's own handle: an identity runtime of its own with a
/// session bridge, under an instance id that names the mob, with the
/// launch's roster provider asked for that mob's definition and the agents
/// it answers seated in that mob alone.
#[tokio::test]
async fn the_gateways_composer_gives_a_second_mob_its_identity_plane_over_that_mobs_own_handle() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(&roster, None)));
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&host, of_the_second).await;

    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "a second mob of a launch that hands the runtime the gateways' composer has an \
         identity runtime when it resolves"
    );
    if let Some(second_runtime) = second_runtime {
        assert!(
            !Arc::ptr_eq(&second_runtime, &of_the_default.runtime),
            "and it is not the launch mob's"
        );
        assert!(
            second_runtime.has_session_bridge(),
            "it reaches its agents' sessions through a bridge of its own"
        );
        assert!(
            second_runtime
                .runtime_instance_id()
                .ends_with(second_id.as_str())
                && second_runtime.runtime_instance_id()
                    != of_the_default.runtime.runtime_instance_id(),
            "its instance id names the mob: {}",
            second_runtime.runtime_instance_id()
        );
        assert!(
            second_runtime
                .registered_spec(&identity("back"))
                .await
                .is_ok(),
            "it knows the agent the roster answers for its mob"
        );
    }
    assert_eq!(
        roster.asked_for(),
        vec![Some(second_id.clone())],
        "the launch's roster provider was asked once, with the second mob's definition"
    );
    let in_the_second = members_of(&host, &second_id).await;
    assert!(
        in_the_second.contains("desk")
            && in_the_second.contains("back")
            && !in_the_second.contains("front"),
        "the second mob's agents are seated in it, and no agent of the launch mob: \
         {in_the_second:?}"
    );
    let in_the_default = members_of(&host, host.mob_handle().mob_id()).await;
    assert!(
        in_the_default.contains("desk") && !in_the_default.contains("back"),
        "the launch mob keeps its own agents alone: {in_the_default:?}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// Two mobs of one host with an agent of one name keep two sessions: each
/// mob's identity runtime wakes its own agent `desk`, in a session of its
/// own mob, and waking the second mob's does not move the launch mob's.
#[tokio::test]
async fn two_mobs_with_an_agent_of_one_name_keep_separate_sessions() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(&roster, None)));
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk"]);
    let second_id = second_mob(&host, of_the_second).await;
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "the second mob has an identity runtime"
    );

    let desk = identity("desk");
    let in_the_default = Box::pin(of_the_default.runtime.materialize(&desk))
        .await
        .ok()
        .flatten()
        .map(|scope| scope.session_id().clone());
    assert!(
        in_the_default.is_some(),
        "the launch mob's agent `desk` wakes in a session"
    );
    if let Some(second_runtime) = second_runtime {
        let in_the_second = Box::pin(second_runtime.materialize(&desk))
            .await
            .ok()
            .flatten()
            .map(|scope| scope.session_id().clone());
        assert!(
            in_the_second.is_some() && in_the_second != in_the_default,
            "the second mob's agent `desk` wakes in a session of its own: {in_the_default:?}, \
             {in_the_second:?}"
        );
        let in_the_default_again = Box::pin(of_the_default.runtime.materialize(&desk))
            .await
            .ok()
            .flatten()
            .map(|scope| scope.session_id().clone());
        assert_eq!(
            in_the_default_again, in_the_default,
            "the launch mob's agent `desk` keeps its session"
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// A callback bridge that keeps what it is asked: the method and the params
/// of every call, each answered with null.
#[derive(Clone, Default)]
struct AskedCallbacks {
    asked: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
}

impl AskedCallbacks {
    /// What was asked since the last read, in order.
    fn taken(&self) -> Vec<(String, serde_json::Value)> {
        std::mem::take(&mut *self.asked.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[async_trait::async_trait]
impl crate::identity_first::gateway_bridges::CallbackBridge for AskedCallbacks {
    async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method.to_string(), params));
        Ok(serde_json::Value::Null)
    }
}

/// The bootstrap of a hosted mob's identity plane asks the SDK no member
/// callback: neither `after_create` nor `compute_edges`, nor
/// `customize_build`. Every launch hands its roster over as its launch
/// mob's own and hands no topology provider over, so a mob hosted beside
/// the launch mob has no identity agent and its roster pass asks no
/// topology provider.
///
/// The customizer in the row is the launch's own bridge handed over as it
/// is. It holds the LAUNCH mob's runtime services, so it names the launch
/// mob on `after_create` (asked directly below, it does), and it would name
/// the launch mob for a hosted mob's agent too: a gateway therefore makes
/// each hosted mob a bridge of its own (the rows on a hosted mob that has
/// an agent, below). This row keeps the roster's side: a hosted mob gets
/// no agent of the launch's roster and its bootstrap asks the SDK nothing.
#[tokio::test]
async fn a_hosted_mobs_bootstrap_asks_neither_after_create_nor_compute_edges() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let launch_mob = host.mob_handle().mob_id().clone();

    // The launch's customizer bridge as the stdio gateway builds it: with
    // the runtime services of the launch mob's identity runtime.
    let asked = AskedCallbacks::default();
    let bridge = Arc::new(
        crate::identity_first::gateway_bridges::GatewayAgentCustomizer::new(asked.clone())
            .with_runtime_services(AgentRuntimeServices::new(host.mob_handle())),
    );
    // The recorder is live and the bridge is the launch mob's: asked
    // directly, its `after_create` names the launch mob.
    bridge
        .after_create(
            &identity("desk"),
            &meerkat_core::types::SessionId::new(),
            &crate::mob_handle_runtime::SessionCreatedContext {
                model: "gpt-5.5".to_string(),
                labels: BTreeMap::new(),
                system_prompt: None,
            },
        )
        .await
        .expect("the bridge tells after_create");
    let told_directly = asked.taken();
    assert_eq!(
        told_directly
            .iter()
            .map(|(method, params)| (method.as_str(), params["mob_id"].as_str()))
            .collect::<Vec<_>>(),
        vec![(
            "callback/agent_customizer/after_create",
            Some(launch_mob.as_str())
        )],
        "the launch's bridge names the launch mob on after_create"
    );

    // The composer as the gateways hand it over: the launch's roster as its
    // launch mob's own, the launch's one customizer, no topology provider.
    // The launch's roster lists agents under the hosted mob's id too: it is
    // not asked for that mob.
    let roster = Arc::new(LaunchRoster::default());
    let of_the_second = definition("second");
    roster.configure(&launch_mob, &["desk", "front"]);
    roster.configure(&of_the_second.id, &["desk", "back"]);
    host.compose_host_mob_identities(Arc::new(
        gateways_composer(&roster, Some(bridge as Arc<dyn AgentCustomizer>))
            .with_roster_of_the_launch_mob_alone(launch_mob.clone()),
    ));
    let second_id = second_mob(&host, of_the_second).await;

    // A resolve waits for the composition and its roster pass.
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "the hosted mob has an identity plane of its own"
    );
    let in_the_second = members_of(&host, &second_id).await;
    assert!(
        !in_the_second.contains("desk") && !in_the_second.contains("back"),
        "no agent of the launch's roster is seated in the hosted mob: {in_the_second:?}"
    );
    let asked_by_the_bootstrap = asked
        .taken()
        .into_iter()
        .map(|(method, _)| method)
        .collect::<Vec<_>>();
    assert!(
        asked_by_the_bootstrap.is_empty(),
        "the hosted mob's bootstrap asked the SDK a member callback: {asked_by_the_bootstrap:?}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// Why the row above is not empty: a composer whose roster DOES answer for
/// a hosted mob (no launch hands one over in this release) seats that mob's
/// agent, and the same bridge is then asked `customize_build` for it, which
/// names the HOSTED mob: the mob of the build's own context, issued by the
/// hosted mob's identity runtime. Handed over as it is here, the launch's
/// one bridge would name the launch mob on `after_create` of such an agent
/// (the row above), which is why a gateway makes each hosted mob a bridge
/// of its own.
#[tokio::test]
async fn a_hosted_mobs_agent_would_be_built_under_its_own_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let asked = AskedCallbacks::default();
    let bridge = Arc::new(
        crate::identity_first::gateway_bridges::GatewayAgentCustomizer::new(asked.clone())
            .with_runtime_services(AgentRuntimeServices::new(host.mob_handle())),
    );
    let roster = Arc::new(LaunchRoster::default());
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["back"]);
    // No `with_roster_of_the_launch_mob_alone`: the roster answers for the
    // hosted mob.
    host.compose_host_mob_identities(Arc::new(gateways_composer(
        &roster,
        Some(bridge as Arc<dyn AgentCustomizer>),
    )));
    let second_id = second_mob(&host, of_the_second).await;
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "the hosted mob has an identity plane"
    );
    if let Some(second_runtime) = second_runtime {
        // Waking the agent builds its member, if the roster pass did not.
        let _ = Box::pin(second_runtime.materialize(&identity("back"))).await;
    }

    let built_under = asked
        .taken()
        .into_iter()
        .filter(|(method, _)| method == "callback/agent_customizer/customize_build")
        .map(|(_, params)| params["mob_id"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    assert!(
        !built_under.is_empty(),
        "the hosted mob's agent was built through the launch's customizer bridge"
    );
    assert!(
        built_under
            .iter()
            .all(|mob| mob.as_deref() == Some(second_id.as_str())),
        "every customize_build of the hosted mob's agent names the hosted mob: {built_under:?}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The method of the callback that customizes a build.
const CUSTOMIZE_BUILD: &str = "callback/agent_customizer/customize_build";
/// The method of the callback told after an agent's session was created.
const AFTER_CREATE: &str = "callback/agent_customizer/after_create";
/// The method of the callback asked for a mob's edges.
const COMPUTE_EDGES: &str = "callback/topology_provider/compute_edges";
/// The method of the callback that runs a customizer's tool.
const CALL_TOOL: &str = "callback/call_tool";

/// The tool the rows' SDK host gives every agent it customizes.
const HOST_TOOL: &str = "record_lookup";

/// A callback bridge that stands for an SDK host with a customizer. It
/// keeps what it is asked, answers `customize_build` with the draft it was
/// given and one tool of its own, [`HOST_TOOL`], and answers a call of that
/// tool. Every other call is answered with null.
#[derive(Clone, Default)]
struct CustomizingHost {
    asked: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
}

impl CustomizingHost {
    /// What was asked since the last read, in order.
    fn taken(&self) -> Vec<(String, serde_json::Value)> {
        std::mem::take(&mut *self.asked.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[async_trait::async_trait]
impl crate::identity_first::gateway_bridges::CallbackBridge for CustomizingHost {
    async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let answer = match method {
            CUSTOMIZE_BUILD => {
                let mut draft = params["draft"].clone();
                draft["external_tools"] = serde_json::json!([{
                    "name": HOST_TOOL,
                    "description": "Look up a record.",
                    "input_schema": {"type": "object"},
                }]);
                draft
            }
            CALL_TOOL => serde_json::json!({"content": "found"}),
            _ => serde_json::Value::Null,
        };
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method.to_string(), params));
        Ok(answer)
    }
}

/// The mob each call of `method` in `asked` names, in order.
fn mobs_named(asked: &[(String, serde_json::Value)], method: &str) -> Vec<Option<String>> {
    asked
        .iter()
        .filter(|(asked_method, _)| asked_method == method)
        .map(|(_, params)| params["mob_id"].as_str().map(str::to_string))
        .collect()
}

/// The scope of the last `customize_build` in `asked`: the one a call of
/// that build's tools is routed to.
fn last_build_scope(asked: &[(String, serde_json::Value)]) -> Option<String> {
    asked
        .iter()
        .rev()
        .find(|(method, _)| method == CUSTOMIZE_BUILD)
        .and_then(|(_, params)| params["scope_id"].as_str().map(str::to_string))
}

/// The launch's customizer bridge as the stdio gateway builds it: over the
/// SDK host's callback bridge, with the runtime services of the launch
/// mob's identity runtime.
fn launch_bridge_of(
    host: &UnifiedRuntime,
    sdk_host: &CustomizingHost,
) -> Arc<crate::identity_first::gateway_bridges::GatewayAgentCustomizer> {
    Arc::new(
        crate::identity_first::gateway_bridges::GatewayAgentCustomizer::new(sdk_host.clone())
            .with_runtime_services(AgentRuntimeServices::new(host.mob_handle())),
    )
}

/// The gateways' composer as the stdio gateway hands it over with its
/// customizer bridge `launch_bridge`: the launch's composed customizer, and
/// the gateway's own maker of each hosted mob's bridge, here of a gateway
/// without a host composition. This mirrors the gateway's own call: it
/// changes when that call does.
fn gateways_composer_over_a_bridge(
    roster: &Arc<LaunchRoster>,
    launch_bridge: &Arc<crate::identity_first::gateway_bridges::GatewayAgentCustomizer>,
) -> HostMobIdentities {
    gateways_composer(
        roster,
        Some(Arc::clone(launch_bridge) as Arc<dyn AgentCustomizer>),
    )
    .with_customizer_of_each_mob(Some(crate::rpc_gateway::customizer_of_each_hosted_mob(
        Arc::clone(launch_bridge),
        None,
    )))
}

/// A mob the host creates beside its launch mob, with one agent, `desk`,
/// that `roster` answers for it, and that mob's identity runtime. No launch
/// hands over a roster that answers for another mob in this release: the
/// agent stands for one a later release seats in such a mob.
async fn a_hosted_mob_with_a_desk(
    host: &UnifiedRuntime,
    roster: &Arc<LaunchRoster>,
    tag: &str,
) -> (MobId, Arc<IdentityRuntime>) {
    let of_the_mob = definition(tag);
    roster.configure(&of_the_mob.id, &["desk"]);
    let mob_id = second_mob(host, of_the_mob).await;
    // A resolve waits for the composition and its roster pass.
    let runtime = host
        .resolve_mob(Some(&mob_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned())
        .expect("the hosted mob has an identity plane of its own");
    (mob_id, runtime)
}

/// Wake `desk` in the mob `runtime` serves and settle the side effects of
/// its lifecycle steps. Returns the session it woke in.
async fn woken_desk(runtime: &Arc<IdentityRuntime>) -> meerkat_core::types::SessionId {
    let session = Box::pin(runtime.materialize(&identity("desk")))
        .await
        .expect("the mob's agent `desk` wakes")
        .map(|scope| scope.session_id().clone())
        .expect("in a session");
    runtime.settle_lifecycle_projection().await;
    session
}

/// Every customizer callback of a mob hosted beside the launch mob names
/// THAT mob. Two such mobs each have an agent `desk`. The SDK host is told
/// `customize_build` and `after_create` for each, and each call names the
/// mob of the agent it is about: never the launch mob, whose bridge the
/// launch built with its own services, and never the other hosted mob.
///
/// `customize_build` reads its mob from the build's own context.
/// `after_create` carries no such context: it names the mob of the runtime
/// services its bridge was given, so each hosted mob needs a bridge of its
/// own, made from that mob's services.
#[tokio::test]
async fn every_customizer_callback_of_a_hosted_mob_names_that_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let launch_mob = host.mob_handle().mob_id().clone();
    let sdk_host = CustomizingHost::default();
    let launch_bridge = launch_bridge_of(&host, &sdk_host);
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer_over_a_bridge(
        &roster,
        &launch_bridge,
    )));

    let mut hosted = Vec::new();
    let mut told_after_create = Vec::new();
    for tag in ["second", "third"] {
        let (mob_id, runtime) = a_hosted_mob_with_a_desk(&host, &roster, tag).await;
        assert_ne!(mob_id, launch_mob);
        let session = woken_desk(&runtime).await;
        let told = sdk_host.taken();
        // The builds of this mob's agent are over before the next mob
        // exists: every `customize_build` so far is one of them.
        let built_under = mobs_named(&told, CUSTOMIZE_BUILD);
        assert!(
            !built_under.is_empty(),
            "the SDK host was asked to customize the build of {mob_id}'s agent"
        );
        assert!(
            built_under
                .iter()
                .all(|mob| mob.as_deref() == Some(mob_id.as_str())),
            "every customize_build of {mob_id}'s agent names that mob: {built_under:?}"
        );
        told_after_create.extend(
            told.into_iter()
                .filter(|(method, _)| method == AFTER_CREATE),
        );
        hosted.push((mob_id, session));
    }
    // What a lifecycle step told late is read once more.
    told_after_create.extend(
        sdk_host
            .taken()
            .into_iter()
            .filter(|(method, _)| method == AFTER_CREATE),
    );

    // An `after_create` is the agent's whose session it names.
    for (mob_id, session) in &hosted {
        let session = session.to_string();
        let created_under = told_after_create
            .iter()
            .filter(|(_, params)| params["session_id"].as_str() == Some(session.as_str()))
            .map(|(_, params)| params["mob_id"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert!(
            !created_under.is_empty(),
            "the SDK host was told after_create for {mob_id}'s agent"
        );
        assert!(
            created_under
                .iter()
                .all(|mob| mob.as_deref() == Some(mob_id.as_str())),
            "every after_create of {mob_id}'s agent names that mob (the launch mob is \
             {launch_mob}): {created_under:?}"
        );
    }
    let mobs = hosted.iter().map(|(mob_id, _)| mob_id).collect::<Vec<_>>();
    shut_down(&host, &mobs).await;
}

/// Call [`HOST_TOOL`] as the agent `desk` of the mob `runtime` serves, from
/// that mob's own customizer tools, and answer the scope the SDK host was
/// asked to run it under. `None`: the mob's agent has no such tool, or the
/// call failed.
async fn scope_a_desk_tool_call_reaches(
    sdk_host: &CustomizingHost,
    runtime: &Arc<IdentityRuntime>,
) -> Option<String> {
    use meerkat_core::AgentToolDispatcher;
    let tools = runtime
        .installed_customizer_tool_registry()
        .await?
        .for_member(&crate::member_comms_id::mob_member_id("desk"))?;
    if !tools.tools().iter().any(|tool| tool.name == HOST_TOOL) {
        return None;
    }
    let _ = sdk_host.taken();
    let args = serde_json::value::RawValue::from_string("{}".to_string()).ok()?;
    let outcome = tools
        .dispatch(meerkat_core::types::ToolCallView {
            id: "call-1",
            name: HOST_TOOL,
            args: &args,
        })
        .await
        .ok()?;
    if outcome.result.is_error {
        return None;
    }
    sdk_host
        .taken()
        .into_iter()
        .find(|(method, _)| method == CALL_TOOL)
        .and_then(|(_, params)| params["scope_id"].as_str().map(str::to_string))
}

/// A rebuild of one mob's agent leaves the agent of the same name in
/// another mob with its tools. Two mobs hosted beside the launch mob each
/// have an agent `desk`, and the SDK host gives each build a tool under
/// that build's own scope. One mob's `desk` is built again: its tool then
/// runs under the new build's scope, and the other mob's `desk` still has
/// its tool, which still runs under the scope of ITS build.
///
/// The gateway keeps an agent's customizer tools by the agent's name, in
/// the customizer tool registry of the agent's own mob: two mobs that
/// shared one registry would hand one mob's agent the other mob's build.
#[tokio::test]
async fn a_rebuild_of_one_mobs_agent_leaves_another_mobs_agent_of_that_name_its_tools() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let sdk_host = CustomizingHost::default();
    let launch_bridge = launch_bridge_of(&host, &sdk_host);
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer_over_a_bridge(
        &roster,
        &launch_bridge,
    )));

    let (second_id, second_runtime) = a_hosted_mob_with_a_desk(&host, &roster, "second").await;
    woken_desk(&second_runtime).await;
    let second_scope = last_build_scope(&sdk_host.taken());
    let (third_id, third_runtime) = a_hosted_mob_with_a_desk(&host, &roster, "third").await;
    woken_desk(&third_runtime).await;
    let third_scope = last_build_scope(&sdk_host.taken());
    assert!(
        second_scope.is_some() && third_scope.is_some() && second_scope != third_scope,
        "each mob's `desk` was built under a scope of its own: {second_scope:?}, {third_scope:?}"
    );
    assert_eq!(
        scope_a_desk_tool_call_reaches(&sdk_host, &second_runtime).await,
        second_scope,
        "the first mob's `desk` runs its tool under its own build's scope"
    );
    assert_eq!(
        scope_a_desk_tool_call_reaches(&sdk_host, &third_runtime).await,
        third_scope,
        "the other mob's `desk` runs its tool under its own build's scope"
    );

    // The first mob's `desk` is built again.
    let _ = sdk_host.taken();
    let rebuilt = Box::pin(second_runtime.respawn(&identity("desk"))).await;
    assert!(
        rebuilt.is_ok(),
        "the first mob's `desk` is built again: {:?}",
        rebuilt.as_ref().err()
    );
    second_runtime.settle_lifecycle_projection().await;
    let told = sdk_host.taken();
    let rebuilt_scope = last_build_scope(&told);
    assert!(
        rebuilt_scope.is_some() && rebuilt_scope != second_scope && rebuilt_scope != third_scope,
        "the rebuild was customized under a new scope: {rebuilt_scope:?}"
    );
    let rebuilt_under = mobs_named(&told, CUSTOMIZE_BUILD);
    assert!(
        rebuilt_under
            .iter()
            .all(|mob| mob.as_deref() == Some(second_id.as_str())),
        "the rebuild names its own mob: {rebuilt_under:?}"
    );

    assert_eq!(
        scope_a_desk_tool_call_reaches(&sdk_host, &third_runtime).await,
        third_scope,
        "the other mob's `desk` keeps its tool, under the scope of its own build"
    );
    assert_eq!(
        scope_a_desk_tool_call_reaches(&sdk_host, &second_runtime).await,
        rebuilt_scope,
        "the rebuilt `desk` runs its tool under the new build's scope"
    );
    shut_down(&host, &[&second_id, &third_id]).await;
}

/// A limit of this release: a mob hosted beside the launch mob is not asked
/// for its edges. The application's edges callback is the launch mob's
/// alone: a launch hands its topology provider to no other mob, so the
/// roster pass of a hosted mob's composition, which asks whatever topology
/// provider it was given, asks none, even for a mob that has an agent. The
/// launch's own topology bridge is live and names the launch mob (asked
/// directly below, it does).
#[tokio::test]
async fn a_hosted_mob_with_an_agent_is_not_asked_for_its_edges() {
    use crate::identity_first::contracts::TopologyProvider;
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let launch_mob = host.mob_handle().mob_id().clone();
    let sdk_host = CustomizingHost::default();
    let launch_bridge = launch_bridge_of(&host, &sdk_host);
    let launch_topology =
        crate::identity_first::gateway_bridges::GatewayTopologyProvider::new(sdk_host.clone())
            .with_runtime_services(AgentRuntimeServices::new(host.mob_handle()));
    // Null is no list of edges: the answer is refused, and the ask is kept.
    let _ = launch_topology
        .compute_edges(
            &[identity("desk")],
            &crate::identity_first::TopologyContext { roster: Vec::new() },
        )
        .await;
    assert_eq!(
        mobs_named(&sdk_host.taken(), COMPUTE_EDGES),
        vec![Some(launch_mob.to_string())],
        "the launch's topology bridge asks for the launch mob's edges"
    );

    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer_over_a_bridge(
        &roster,
        &launch_bridge,
    )));
    let (second_id, second_runtime) = a_hosted_mob_with_a_desk(&host, &roster, "second").await;
    woken_desk(&second_runtime).await;
    let told = sdk_host.taken();
    assert!(
        !mobs_named(&told, CUSTOMIZE_BUILD).is_empty(),
        "the hosted mob has an agent the SDK host customized"
    );
    assert!(
        mobs_named(&told, COMPUTE_EDGES).is_empty(),
        "a hosted mob is not asked for its edges: {:?}",
        told.iter().map(|(method, _)| method).collect::<Vec<_>>()
    );
    shut_down(&host, &[&second_id]).await;
}

/// A host composition whose customizer hook keeps how often it was asked
/// and answers the customizer it was given, or none while it is told to
/// drop it.
#[derive(Default)]
struct CountingHook {
    asked: std::sync::atomic::AtomicUsize,
    drops: AtomicBool,
}

impl CountingHook {
    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl crate::rpc_gateway::GatewayHostComposition for CountingHook {
    async fn prepare_bootstrap(
        &self,
        spec: crate::mob_handle_runtime::MobBootstrapSpec,
    ) -> Result<crate::mob_handle_runtime::MobBootstrapSpec, crate::rpc_gateway::GatewayHostError>
    {
        Ok(spec)
    }
    fn native_grant_work_configuration(
        &self,
    ) -> Result<
        Option<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration>,
        crate::rpc_gateway::GatewayHostError,
    > {
        Ok(None)
    }
    async fn bind_runtime(
        &self,
        _runtime: &mut UnifiedRuntime,
    ) -> Result<(), crate::rpc_gateway::GatewayHostError> {
        Ok(())
    }
    fn publish_bootstrap_ready(
        &self,
        publication: crate::rpc_gateway::GatewayBootstrapReady<'_>,
    ) -> Result<(), crate::rpc_gateway::GatewayHostError> {
        publication.publish();
        Ok(())
    }
    fn begin_close(
        &self,
        _reason: crate::rpc_gateway::GatewayHostCloseReason,
    ) -> Result<(), crate::rpc_gateway::GatewayHostError> {
        Ok(())
    }
    async fn join_close(&self) -> Result<(), crate::rpc_gateway::GatewayHostError> {
        Ok(())
    }
    async fn wait_closed(&self) {
        std::future::pending::<()>().await;
    }
    fn compose_agent_customizer(
        &self,
        customizer: Option<Arc<dyn AgentCustomizer>>,
    ) -> Result<Option<Arc<dyn AgentCustomizer>>, crate::rpc_gateway::GatewayHostError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if self.drops.load(Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(customizer)
    }
}

/// A host's customizer hook is asked once for the launch and once more
/// when a mob hosted beside the launch mob is loaded: never for a mob that
/// is listed and not loaded. A gateway starts over a state directory that
/// holds a second mob. The launch composes its own customizer through the
/// hook: asked once. Handing the composer over asks nothing, and the
/// stored mob, which the mob set lists as not loaded, asks nothing either.
/// The first request that names the mob loads it and composes its identity
/// plane with a bridge of its own, which passes through the hook: asked
/// twice. A second request for the loaded mob asks nothing.
#[tokio::test]
async fn the_hosts_customizer_hook_is_asked_for_the_launch_and_for_each_hosted_mob_it_loads() {
    let stored = StoredHost::new();
    let first = stored.launch().await;
    let second_id = second_mob(&first, definition("second")).await;
    shut_down(&first, &[]).await;
    drop(first);

    let host = stored.launch().await;
    let hook = Arc::new(CountingHook::default());
    let of_the_gateway: Arc<dyn crate::rpc_gateway::GatewayHostComposition> = hook.clone();
    let sdk_host = CustomizingHost::default();
    let launch_bridge = launch_bridge_of(&host, &sdk_host);

    // The launch's own customizer, composed by the host as the gateway
    // composes it at its init.
    let of_the_launch = crate::rpc_gateway::host_composed_agent_customizer(
        Some(&of_the_gateway),
        Some(Arc::clone(&launch_bridge) as Arc<dyn AgentCustomizer>),
    );
    assert!(
        of_the_launch.as_ref().is_ok_and(Option::is_some),
        "the host composes the launch's customizer: {:?}",
        of_the_launch.as_ref().err()
    );
    assert_eq!(hook.asked(), 1, "the launch alone asks the hook once");

    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(
        gateways_composer(&roster, of_the_launch.ok().flatten())
            .with_roster_of_the_launch_mob_alone(host.mob_handle().mob_id().clone())
            .with_customizer_of_each_mob(Some(crate::rpc_gateway::customizer_of_each_hosted_mob(
                Arc::clone(&launch_bridge),
                Some(of_the_gateway),
            ))),
    ));
    let before_a_load = known(&host).await;
    assert!(
        before_a_load.not_loaded.contains(&second_id) && !before_a_load.loaded.contains(&second_id),
        "the stored mob is listed and not loaded: loaded {:?}, not loaded {:?}",
        before_a_load.loaded,
        before_a_load.not_loaded
    );
    assert_eq!(
        hook.asked(),
        1,
        "a mob that is listed and not loaded asks the hook nothing"
    );

    // A request that names the mob loads it; the resolve waits for its
    // identity plane.
    let loaded = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        loaded.is_some(),
        "the loaded mob has an identity plane of its own"
    );
    assert_eq!(
        hook.asked(),
        2,
        "the load of a hosted mob asks the hook once more"
    );
    let again = host.resolve_mob(Some(&second_id)).await;
    assert!(again.is_ok(), "the loaded mob resolves again");
    drop(again);
    assert_eq!(hook.asked(), 2, "a loaded mob asks the hook nothing more");

    // Unloaded, the mob is listed and not loaded once more. The next
    // request loads it with a new entry and a bridge made anew: the hook is
    // asked again.
    drop(loaded);
    asleep(&host, &[&second_id]).await;
    assert_eq!(hook.asked(), 2, "an unload asks the hook nothing");
    let loaded_again = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        loaded_again.is_some(),
        "the mob loaded again has an identity plane of its own"
    );
    assert_eq!(
        hook.asked(),
        3,
        "a mob that is unloaded and loaded again asks the hook again"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A hook that drops a hosted mob's customizer leaves that mob without an
/// identity plane, and the gateway keeps serving: the mob resolves, the
/// launch mob resolves, and the next request that resolves the mob asks
/// the hook again. Once the hook answers the customizer it is given, that
/// request composes the mob's identity plane.
#[tokio::test]
async fn a_hook_that_drops_a_hosted_mobs_customizer_leaves_it_without_an_identity_plane() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let hook = Arc::new(CountingHook::default());
    let of_the_gateway: Arc<dyn crate::rpc_gateway::GatewayHostComposition> = hook.clone();
    let sdk_host = CustomizingHost::default();
    let launch_bridge = launch_bridge_of(&host, &sdk_host);
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(
        gateways_composer(
            &roster,
            Some(Arc::clone(&launch_bridge) as Arc<dyn AgentCustomizer>),
        )
        .with_customizer_of_each_mob(Some(
            crate::rpc_gateway::customizer_of_each_hosted_mob(
                Arc::clone(&launch_bridge),
                Some(of_the_gateway),
            ),
        )),
    ));

    hook.drops.store(true, Ordering::SeqCst);
    let second_id = second_mob(&host, definition("second")).await;
    let served = host.resolve_mob(Some(&second_id)).await;
    assert!(
        served
            .as_ref()
            .is_ok_and(|mob| mob.identity_runtime_ref().is_none()),
        "the mob is served without an identity plane"
    );
    drop(served);
    let asked_so_far = hook.asked();
    assert!(asked_so_far >= 1, "the hook was asked for the hosted mob");
    let served_again = host.resolve_mob(Some(&second_id)).await;
    assert!(
        served_again
            .as_ref()
            .is_ok_and(|mob| mob.identity_runtime_ref().is_none()),
        "the mob is still served without an identity plane"
    );
    drop(served_again);
    assert!(
        hook.asked() > asked_so_far,
        "the next request that resolves the mob asks the hook again"
    );
    assert!(
        host.resolve_mob(None).await.is_ok(),
        "the launch mob is served all along"
    );

    hook.drops.store(false, Ordering::SeqCst);
    let composed = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        composed.is_some(),
        "once the hook answers the customizer it is given, the mob gets its identity plane"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The gateways' composer hands a second mob's agents no agent memory: the
/// services it composes for the mob's requests carry no memory provider,
/// and the customizer of the mob's identity context is the one the launch
/// gave the composer, which a launch composes without the memory it binds
/// to its launch mob. So no memory event of a second mob's agent exists
/// to be written under an agent of the launch mob.
#[tokio::test]
async fn the_gateways_composer_hands_a_second_mobs_agents_no_agent_memory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let of_the_launch: Arc<dyn AgentCustomizer> = Arc::new(LaunchCustomizer);
    host.compose_host_mob_identities(Arc::new(gateways_composer(
        &roster,
        Some(Arc::clone(&of_the_launch)),
    )));
    let second_id = second_mob(&host, definition("second")).await;

    let services = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_request_services());
    assert!(
        services.is_some(),
        "the second mob has the services of its identity plane's requests"
    );
    if let Some(services) = services {
        assert!(
            services.agent_memory_provider.is_none(),
            "they carry no agent memory provider"
        );
        assert!(
            services.compaction_floors.is_some(),
            "they carry the compaction floors of the mob's own bridge"
        );
    }
    let customizer = host
        .mob_entry_in_tests(&second_id)
        .and_then(|entry| entry.identity_first_context.get().cloned())
        .and_then(|context| context.customizer.clone());
    assert!(
        customizer.is_some_and(|customizer| Arc::ptr_eq(&customizer, &of_the_launch)),
        "the customizer of the second mob's agents is the one the launch gave the composer"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The gateways' composer carries the launch's values to a second mob's
/// identity runtime: whether the launch has a durable runtime store,
/// whether it is a candidate launch, where an operator repairs a session,
/// and how the roster pass seats the mob's agents.
#[tokio::test]
async fn the_gateways_composer_carries_the_launchs_values_to_a_second_mobs_identity_runtime() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let repair_scope = SessionRepairScope {
        state_root: Some(temp.path().to_path_buf()),
        realm: Some("a-realm".to_string()),
        runtime_store: None,
    };
    host.compose_host_mob_identities(Arc::new(HostMobIdentities {
        has_runtime_store: false,
        bootstrap_mode: IdentityBootstrapMode::LazyMaterialize,
        candidate_launch: true,
        session_repair_scope: Some(repair_scope.clone()),
        ..gateways_composer(&roster, None)
    }));
    let second_id = second_mob(&host, definition("second")).await;

    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "the second mob has an identity runtime"
    );
    if let Some(second_runtime) = second_runtime {
        assert!(
            !second_runtime.has_runtime_store(),
            "a launch without a durable runtime store composes none for the second mob"
        );
        assert!(
            second_runtime.is_candidate_launch(),
            "a candidate launch's second mob is a candidate's too"
        );
        assert_eq!(
            second_runtime.session_repair_scope(),
            Some(repair_scope),
            "the second mob's identity runtime names the launch's repair scope"
        );
    }
    let bootstrap_mode = host
        .mob_entry_in_tests(&second_id)
        .and_then(|entry| entry.identity_first_context.get().cloned())
        .map(|context| context.bootstrap_mode().clone());
    assert_eq!(
        bootstrap_mode,
        Some(IdentityBootstrapMode::LazyMaterialize),
        "the second mob's roster pass runs in the launch's bootstrap mode"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A launch of the gateways' composer that is no candidate marks no second
/// mob's identity runtime as one.
#[tokio::test]
async fn a_launch_that_is_no_candidate_marks_no_second_mob_as_one() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_default) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(&roster, None)));
    let second_id = second_mob(&host, definition("second")).await;

    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some_and(|runtime| {
            !runtime.is_candidate_launch()
                && runtime.has_runtime_store()
                && runtime.session_repair_scope().is_none()
        }),
        "the second mob has an identity runtime of a launch on a durable store that is no \
         candidate"
    );
    shut_down(&host, &[&second_id]).await;
}

/// An identity runtime keeps neither the continuity store nor the lease
/// provider it is given: an agent's continuity is in its mob's own store.
/// So the pair a composer gives a second mob's identity runtime holds
/// nothing of the mob, and no file of it is owed.
#[tokio::test]
async fn an_identity_runtime_keeps_neither_the_continuity_store_nor_the_lease_provider_it_is_given()
{
    let continuity_store: Arc<dyn ContinuityStore> =
        Arc::new(LocalContinuityStore::in_memory().expect("continuity store"));
    let lease_provider: Arc<dyn LeaseProvider> = Arc::new(LocalLeaseProvider::new());
    // Kept alive to the end of the row: the counts are read while it lives.
    let _identity_runtime = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::clone(&continuity_store),
        lease_provider: Arc::clone(&lease_provider),
        runtime_instance_id: "mob-identity-plane-pin".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: None,
        default_timeout: None,
    });
    assert_eq!(
        Arc::strong_count(&continuity_store),
        1,
        "the identity runtime holds no reference to the continuity store it was given"
    );
    assert_eq!(
        Arc::strong_count(&lease_provider),
        1,
        "nor to the lease provider"
    );
}

/// The tool the rows' launch customizer gives each agent it builds.
const LAUNCH_TOOL: &str = "workspace_lookup";

/// The launch's tools for one build: [`LAUNCH_TOOL`].
struct LaunchTools;

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for LaunchTools {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        vec![Arc::new(meerkat_core::types::ToolDef {
            audience: Default::default(),
            name: LAUNCH_TOOL.into(),
            description: "Look up the workspace record.".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
        })]
        .into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(meerkat_core::ToolResult {
            host_metadata: Default::default(),
            tool_use_id: call.id.to_string(),
            content: vec![],
            is_error: false,
            settlement_failures: Vec::new(),
        }
        .into())
    }
}

/// A launch customizer that gives every agent it builds [`LAUNCH_TOOL`],
/// and keeps the names of the agents it was asked to build.
#[derive(Default)]
struct ToolCustomizer {
    built: Mutex<Vec<String>>,
}

impl ToolCustomizer {
    fn built(&self) -> Vec<String> {
        self.built
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl AgentCustomizer for ToolCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        self.built
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(spec.identity.to_string());
        draft.local_external_tools = LocalExternalToolOverlay::new(Arc::new(LaunchTools));
        Ok(())
    }
}

/// An agent of a second mob is built with the tools the launch's customizer
/// gives it: the mob's own member wiring prepares the build (the customizer
/// is asked for that agent) and attaches the tools the customizer put on
/// the build, so the agent's session offers them.
#[tokio::test]
async fn an_agent_of_a_second_mob_is_built_with_the_tools_the_launchs_customizer_gives_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_launch_mob) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let customizer = Arc::new(ToolCustomizer::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(
        &roster,
        Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>),
    )));
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["back"]);
    let second_id = second_mob(&host, of_the_second).await;
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "the second mob has an identity runtime"
    );

    let session = match second_runtime {
        Some(second_runtime) => Box::pin(second_runtime.materialize(&identity("back")))
            .await
            .ok()
            .flatten()
            .map(|scope| scope.session_id().clone()),
        None => None,
    };
    assert!(
        session.is_some(),
        "the second mob's agent wakes in a session"
    );
    assert!(
        customizer.built().iter().any(|name| name == "back"),
        "the launch's customizer was asked to build the second mob's agent: {:?}",
        customizer.built()
    );
    if let Some(session) = session {
        let offered = crate::mob_handle_runtime::resolved_tools_for_session(
            host.mob_runtime().session_service(),
            "back",
            session,
        )
        .await
        .map(|snapshot| snapshot.tools);
        assert!(
            offered
                .as_ref()
                .is_ok_and(|tools| tools.iter().any(|tool| tool == LAUNCH_TOOL)),
            "the agent's session offers the tool the customizer gave its build: {:?}",
            offered.as_ref().map_err(ToString::to_string)
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// The incarnation of the cursor the lifecycle projection of the mob `id`
/// stored in `store`, read until it is `wanted` or twenty seconds passed.
async fn stored_cursor_incarnation(
    store: &Arc<dyn PersistentMetadataStore>,
    id: &MobId,
    wanted: &str,
) -> Option<String> {
    let mut stored = None;
    for _ in 0..200 {
        stored = store
            .lifecycle_projection_cursor(id.as_str())
            .await
            .ok()
            .flatten()
            .map(|cursor| cursor.incarnation);
        if stored.as_deref() == Some(wanted) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    stored
}

/// A second mob of a launch with the gateways' composer, destroyed and
/// created again under the same id with nothing released in between: the
/// host, its metadata store, the mob's id, and the incarnations of the
/// destroyed mob and of the one that stands. Before the destroy the old
/// mob's lifecycle projection had stored its cursor, and an intent of the
/// old incarnation was left in the store, as a step the destroyed mob still
/// owed would leave one.
async fn a_second_mob_created_again_under_its_id(
    temp: &tempfile::TempDir,
) -> (
    UnifiedRuntime,
    Arc<dyn PersistentMetadataStore>,
    MobId,
    String,
    String,
) {
    let (host, _of_the_launch_mob) = launch_with_identities(temp).await;
    let store = Arc::clone(host.persistent_metadata());
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(&roster, None)));
    let of_the_second = definition("second");
    let again = of_the_second.clone();
    let id = of_the_second.id.clone();
    roster.configure(&id, &["back"]);
    second_mob(&host, of_the_second).await;
    let old = set_of(&host)
        .handle_for(&id)
        .await
        .expect("the set serves the second mob")
        .incarnation()
        .to_string();
    assert!(
        host.resolve_mob(Some(&id))
            .await
            .is_ok_and(|mob| mob.identity_runtime_ref().is_some()),
        "the second mob has an identity runtime"
    );
    assert_eq!(
        stored_cursor_incarnation(&store, &id, &old)
            .await
            .as_deref(),
        Some(old.as_str()),
        "the first mob's lifecycle projection stored its cursor"
    );
    store
        .set_lifecycle_effect_intent(&crate::runtime::LifecycleEffectIntent::new(
            id.as_str(),
            old.as_str(),
            "back",
            1,
            crate::runtime::LifecycleEffectVerb::Reset,
            Some(meerkat_core::types::SessionId::new()),
            1_700_000_000_000,
        ))
        .await
        .expect("an intent of the first mob");

    let destroyed = Box::pin(set_of(&host).mob_destroy(&id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    for _ in 0..200 {
        if host.mob_entry_in_tests(&id).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        second_mob(&host, again).await,
        id,
        "a mob under the same id"
    );
    let new = set_of(&host)
        .handle_for(&id)
        .await
        .expect("the set serves the mob created again")
        .incarnation()
        .to_string();
    assert_ne!(new, old, "the mob created again is another incarnation");
    assert!(
        host.resolve_mob(Some(&id))
            .await
            .is_ok_and(|mob| mob.identity_runtime_ref().is_some()),
        "the mob created again has an identity runtime"
    );
    (host, store, id, old, new)
}

/// A second mob created again under a destroyed mob's id starts its
/// lifecycle projection from no cursor of the destroyed mob: the cursor the
/// store holds under that id is of the new incarnation, though nothing
/// released the destroyed mob's records.
#[tokio::test]
async fn a_mob_created_again_under_a_destroyed_mobs_id_starts_from_no_cursor_of_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, store, id, old, new) = a_second_mob_created_again_under_its_id(&temp).await;

    let stored = stored_cursor_incarnation(&store, &id, &new).await;
    assert_eq!(
        stored.as_deref(),
        Some(new.as_str()),
        "the stored cursor is of the mob that stands, not of the destroyed one ({old})"
    );
    shut_down(&host, &[&id]).await;
}

/// A second mob created again under a destroyed mob's id carries out none
/// of what the destroyed mob still owed: the start of its lifecycle
/// projection removes the intents of the destroyed incarnation.
#[tokio::test]
async fn a_mob_created_again_under_a_destroyed_mobs_id_drops_its_intents() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, store, id, old, new) = a_second_mob_created_again_under_its_id(&temp).await;

    // The projection of the mob that stands has started once its cursor is
    // stored.
    assert_eq!(
        stored_cursor_incarnation(&store, &id, &new)
            .await
            .as_deref(),
        Some(new.as_str()),
        "the projection of the mob created again started"
    );
    let mut of_the_destroyed = usize::MAX;
    for _ in 0..200 {
        of_the_destroyed = store
            .lifecycle_effect_intents(id.as_str())
            .await
            .map(|intents| {
                intents
                    .iter()
                    .filter(|intent| intent.incarnation == old)
                    .count()
            })
            .unwrap_or(usize::MAX);
        if of_the_destroyed == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        of_the_destroyed, 0,
        "no intent of the destroyed incarnation is left under the mob's id"
    );
    shut_down(&host, &[&id]).await;
}

/// A cursor no ledger of the rows reaches: a stored structural-events
/// cursor with this value was left by an earlier mob.
const LEFT_CURSOR: u64 = 4_242_424_242;

/// The run of an earlier mob whose labels were left in the host's memory.
const LEFT_RUN: &str = "run-of-the-destroyed";

/// [`launch_with_identities`], of the launch mob `of_the_launch_mob`,
/// keeping the host's records in `metadata`.
async fn launch_with_identities_keeping(
    temp: &tempfile::TempDir,
    of_the_launch_mob: meerkat_mob::MobDefinition,
    metadata: Arc<dyn PersistentMetadataStore>,
) -> UnifiedRuntime {
    let (mut host, _sessions, _machine, ()) = launch_keeping(
        temp,
        of_the_launch_mob,
        meerkat::Config::default(),
        metadata,
        |_| (),
        |spec| spec,
    )
    .await;
    let context = identity_context(host.mob_runtime(), "default");
    Box::pin(host.install_and_bootstrap_identity_first_context(
        context,
        &[configured_agent("desk"), configured_agent("front")],
    ))
    .await
    .expect("the launch mob's identity bootstrap");
    host
}

/// What `store` keeps under the mob id `id` of the four kinds of record a
/// destroyed mob can leave: whether the agent `desk` has a tool record, how
/// many lifecycle failures, the structural-events cursor, and how many
/// idle-retirement opt-ins.
async fn left_under(
    store: &Arc<dyn PersistentMetadataStore>,
    id: &MobId,
) -> (bool, usize, Option<u64>, usize) {
    (
        store
            .identity_tool_record(id.as_str(), "desk")
            .await
            .expect("the tool record")
            .is_some(),
        store
            .lifecycle_effect_failures(id.as_str(), 0, 16)
            .await
            .expect("the lifecycle failures")
            .len(),
        store
            .get_subscription_cursor(id.as_str())
            .await
            .expect("the cursor"),
        store
            .load_member_idle_retire_overrides()
            .await
            .expect("the opt-ins")
            .iter()
            .filter(|record| record.mob_id == id.as_str())
            .count(),
    )
}

/// A second mob of `host` with the agent `desk`, destroyed through the mob
/// set with NO release of its records: the definition to create it again
/// from. When it answers, `store` holds under the mob's id one record of
/// each of the four kinds: the tool record its agent's build wrote, a
/// lifecycle failure of its incarnation, a structural-events cursor
/// ([`LEFT_CURSOR`], written after the destroy as a late write would be)
/// and an idle-retirement opt-in of one of its members. The host's memory
/// holds a fifth kind under the id: a label of the mob and a label of one
/// of its runs ([`LEFT_RUN`]).
async fn a_destroyed_second_mob_with_its_records_left(
    host: &UnifiedRuntime,
    store: &Arc<dyn PersistentMetadataStore>,
) -> meerkat_mob::MobDefinition {
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(&roster, None)));
    let of_the_second = definition("second");
    let again = of_the_second.clone();
    let id = of_the_second.id.clone();
    roster.configure(&id, &["desk"]);
    second_mob(host, of_the_second).await;
    let incarnation = set_of(host)
        .handle_for(&id)
        .await
        .expect("the set serves the second mob")
        .incarnation()
        .to_string();
    let second_runtime = host
        .resolve_mob(Some(&id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned())
        .expect("the second mob has an identity runtime");
    second_runtime
        .write_built_tools(
            &identity("desk"),
            &meerkat_core::types::SessionId::new(),
            vec!["of_the_destroyed".to_string()],
        )
        .await
        .expect("the agent's tool record");
    store
        .record_lifecycle_effect_failure(&crate::runtime::LifecycleEffectFailure::new(
            id.as_str(),
            incarnation.as_str(),
            12,
            "desk",
            3,
            crate::runtime::LifecycleEffectKind::RetirementHook,
            3,
            "timed out",
            1_700_000_000_001,
        ))
        .await
        .expect("a lifecycle failure of the mob");
    store
        .set_member_idle_retire_override(&crate::runtime::MemberIdleRetireOverrideRecord {
            mob_id: id.to_string(),
            member_id: "helper".to_string(),
            session_id: meerkat_core::types::SessionId::new(),
            policy: crate::mob_handle_runtime::DelegateIdleRetireOverride::Disabled,
            recorded_at: chrono::Utc::now(),
        })
        .await
        .expect("an idle-retirement opt-in of the mob");
    // The fifth kind is kept in the host's memory, under the mob's id: the
    // labels a caller gave the mob and one of its runs.
    for (method, params) in [
        (
            "meta/mob_labels/set",
            serde_json::json!({"labels": {"owner": "of-the-destroyed"}}),
        ),
        (
            "meta/run_labels/set",
            serde_json::json!({"run_id": LEFT_RUN, "labels": {"trace": "of-the-destroyed"}}),
        ),
    ] {
        assert_eq!(
            crate::runtime::dispatch_label_method(
                host.metadata_table(),
                id.as_str(),
                method,
                &params
            )
            .await,
            Some(crate::runtime::LabelRpcResult::Accepted),
            "{method} for the second mob"
        );
    }

    let destroyed = Box::pin(set_of(host).mob_destroy(&id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    for _ in 0..200 {
        if host.mob_entry_in_tests(&id).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    store
        .set_subscription_cursor(id.as_str(), LEFT_CURSOR)
        .await
        .expect("a cursor written after the destroy");
    // The destroyed mob's own subscriber may write once more as it ends:
    // a cursor is left either way.
    let (tool_record, failures, cursor, opt_ins) = left_under(store, &id).await;
    assert!(
        tool_record && failures == 1 && cursor.is_some() && opt_ins == 1,
        "the destroyed mob's records are left under its id: nothing released them"
    );
    again
}

/// What a mob created under a destroyed mob's id must find of that mob's
/// records: none. Its own structural-events cursor may be stored by then;
/// the destroyed mob's is not.
async fn assert_nothing_of_the_destroyed_mob_shows(
    store: &Arc<dyn PersistentMetadataStore>,
    id: &MobId,
) {
    let (tool_record, failures, cursor, opt_ins) = left_under(store, id).await;
    assert!(
        !tool_record,
        "the destroyed mob's agent's tool record is gone"
    );
    assert_eq!(
        failures, 0,
        "the destroyed mob's lifecycle failures are gone"
    );
    assert_ne!(
        cursor,
        Some(LEFT_CURSOR),
        "the destroyed mob's structural-events cursor is gone"
    );
    assert_eq!(
        opt_ins, 0,
        "the destroyed mob's idle-retirement opt-ins are gone"
    );
}

/// What a mob created under a destroyed mob's id answers of that mob's
/// labels, and carries of them on its events: none.
async fn assert_no_label_of_the_destroyed_mob_shows(host: &UnifiedRuntime, id: &MobId) {
    for (method, params) in [
        ("meta/mob_labels/get", serde_json::json!({})),
        (
            "meta/run_labels/get",
            serde_json::json!({"run_id": LEFT_RUN}),
        ),
    ] {
        assert_eq!(
            crate::runtime::dispatch_label_method(
                host.metadata_table(),
                id.as_str(),
                method,
                &params
            )
            .await,
            Some(crate::runtime::LabelRpcResult::Labels(BTreeMap::new())),
            "{method} of the mob created under the id answers no label of the destroyed mob"
        );
    }
    let mob = host
        .resolve_mob(Some(id))
        .await
        .expect("the mob created under the id is served");
    let events = mob
        .query_mob_events(&super::EventQuery::default())
        .await
        .expect("the events of the mob created under the id");
    assert!(
        !events.is_empty(),
        "the mob created under the id has events"
    );
    let labelled: Vec<&str> = events
        .iter()
        .filter(|event| !event.mob_labels.is_empty() || !event.run_labels.is_empty())
        .map(|event| event.kind.as_str())
        .collect();
    assert!(
        labelled.is_empty(),
        "no event of the mob created under the id carries a label of the destroyed mob: \
         {labelled:?}"
    );
}

/// A mob created under the id of a destroyed mob whose records were never
/// released reads none of them: the create itself releases what the host
/// still keeps under that id, before the mob is built.
#[tokio::test]
async fn a_mob_created_under_a_destroyed_mobs_id_reads_none_of_its_records() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    let host = launch_with_identities_keeping(&temp, definition("host"), Arc::clone(&store)).await;
    let again = a_destroyed_second_mob_with_its_records_left(&host, &store).await;

    let id = second_mob(&host, again).await;
    assert_nothing_of_the_destroyed_mob_shows(&store, &id).await;
    // The labels are kept in the host's memory, so they are shown without a
    // restart only: a host that starts again holds none.
    assert_no_label_of_the_destroyed_mob_shows(&host, &id).await;
    shut_down(&host, &[&id]).await;
}

/// The same across a restart of the host: the host stopped after the
/// destroy and before any release, starts again over the same records, and
/// a mob created under the destroyed mob's id reads none of them.
#[tokio::test]
async fn a_mob_created_under_a_destroyed_mobs_id_after_a_restart_reads_none_of_its_records() {
    let (first_temp, second_temp) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let store: Arc<dyn PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    let of_the_launch_mob = definition("host");
    let again = {
        let host = launch_with_identities_keeping(
            &first_temp,
            of_the_launch_mob.clone(),
            Arc::clone(&store),
        )
        .await;
        let again = a_destroyed_second_mob_with_its_records_left(&host, &store).await;
        shut_down(&host, &[]).await;
        again
    };
    let id = again.id.clone();
    let (tool_record, failures, cursor, opt_ins) = left_under(&store, &id).await;
    assert!(
        tool_record && failures == 1 && cursor.is_some() && opt_ins == 1,
        "the records outlive the host that left them"
    );

    let host =
        launch_with_identities_keeping(&second_temp, of_the_launch_mob, Arc::clone(&store)).await;
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(&roster, None)));
    assert_eq!(
        second_mob(&host, again).await,
        id,
        "a mob under the same id"
    );
    assert_nothing_of_the_destroyed_mob_shows(&store, &id).await;
    shut_down(&host, &[&id]).await;
}

/// A create is refused when the records an earlier mob of the id left
/// cannot be released: the mob would read another mob's records. Shown
/// with a store whose release fails (its table is gone) and with a store
/// that has no release at all; neither create leaves a mob.
#[tokio::test]
async fn a_create_is_refused_when_an_earlier_mobs_records_cannot_be_released() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("meta.sqlite");
    let failing: Arc<dyn PersistentMetadataStore> = Arc::new(
        crate::runtime::SqliteMetadataStore::open(&path).expect("a metadata store in a file"),
    );
    let without: Arc<dyn PersistentMetadataStore> =
        Arc::new(crate::console_audit::test_support::ListingStore::default());
    for (store, case) in [
        (failing, "a store whose release fails"),
        (without, "a store that has no release"),
    ] {
        let launch_temp = tempfile::tempdir().expect("tempdir");
        let (host, _sessions, _machine, ()) = launch_keeping(
            &launch_temp,
            definition("host"),
            meerkat::Config::default(),
            store,
            |_| (),
            |spec| spec,
        )
        .await;
        if case == "a store whose release fails" {
            // The store's table goes: every statement on it fails from here.
            rusqlite::Connection::open(&path)
                .and_then(|connection| connection.execute("DROP TABLE meta_metadata", []))
                .expect("the table is dropped");
        }
        let of_the_second = definition("second");
        let id = of_the_second.id.clone();
        let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
        assert!(
            matches!(
                &created,
                Err(error) if error.to_string().contains(crate::private_owners::CREATE_RELEASE_FAILED)
            ),
            "the create is refused with the one sentence of a failed release ({case}): {:?}",
            created.as_ref().map(|_| "created")
        );
        // The refusal says what could not be done, and nothing of the
        // store's own failure: no table, path or key of it.
        let answer = created
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            !answer.contains("meta_metadata") && !answer.contains("metadata store io"),
            "the refusal carries nothing of the store's own failure: {answer}"
        );
        assert!(
            set_of(&host).handle_for(&id).await.is_err(),
            "and the set holds no mob under the id"
        );
        shut_down(&host, &[]).await;
    }
}

/// A member built in a mob created under a destroyed mob's id, under the
/// name an earlier private member of that id had, is private to nobody:
/// its own build says so in the launch's record. The create under the id
/// had forgotten what the record held of the earlier mob's member under
/// that name (the row of a create that forgets the members the record
/// holds under its id shows that).
#[tokio::test]
async fn a_member_built_under_a_destroyed_mobs_private_members_name_is_private_to_nobody() {
    const OWNER: &str = "console-caller:v1:aa";
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let of_the_second = definition("second");
    let again = of_the_second.clone();
    let id = second_mob(&host, of_the_second).await;
    let record = host.mob_runtime().private_owners().clone();
    let seated = Box::pin(
        set_of(&host)
            .handle_for(&id)
            .await
            .expect("the set serves the second mob")
            .spawn_spec(
                SpawnMemberSpec::new("worker", "side").with_labels(BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    OWNER.to_string(),
                )])),
            ),
    )
    .await;
    assert!(
        seated.is_ok(),
        "a private member of the second mob: {:?}",
        seated.as_ref().err()
    );
    assert_eq!(
        record.owner_of(id.as_str(), "side").as_deref(),
        Some(OWNER),
        "the launch's record knows the member's owner"
    );

    let destroyed = Box::pin(set_of(&host).mob_destroy(&id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        second_mob(&host, again).await,
        id,
        "a mob under the same id"
    );
    let seated_in_the_new_mob = Box::pin(
        set_of(&host)
            .handle_for(&id)
            .await
            .expect("the set serves the new mob")
            .spawn_spec(SpawnMemberSpec::new("worker", "side")),
    )
    .await;
    assert!(
        seated_in_the_new_mob.is_ok(),
        "a member of the new mob under the earlier private member's name: {:?}",
        seated_in_the_new_mob.as_ref().err()
    );
    assert_eq!(
        record.owner_of(id.as_str(), "side"),
        None,
        "the new mob's member under that name is private to nobody"
    );
    shut_down(&host, &[&id]).await;
}

/// A mob a member creates under the id of an earlier mob is not read by
/// what the launch's record read of that earlier mob: until the new mob's
/// own records are read, a name the record does not hold in it is not
/// known, where a name of a mob the record has read is private to no one.
/// The create's admission forgets that a mob of the id was read. The row
/// keeps every later reading away from the new mob: before the second
/// create it turns the record to the mob set of another launch, so what it
/// sees afterwards is what the create itself left.
#[tokio::test]
async fn a_mob_a_member_creates_under_an_earlier_mobs_id_is_not_read_until_its_own_records_are() {
    use crate::mob_announcements::MemberPerson;
    const NO_MEMBER: &str = "no-member-of-the-mob";

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let elsewhere_temp = tempfile::tempdir().expect("tempdir");
    let (elsewhere, _its_sessions) = launch(&elsewhere_temp, definition("elsewhere")).await;
    let record = host.mob_runtime().private_owners().clone();
    record.first_attempts_settled(Duration::from_secs(30)).await;
    let creator = a_member_that_creates_mobs(&host, "bob", None).await;
    let of_the_second = definition("second");
    let again = of_the_second.clone();
    let id = of_the_second.id.clone();

    let earlier = created_by_a_member(&host, of_the_second, &creator).await;
    // The record reads a mob the set came to manage in the background.
    let mut read_the_earlier_mob = false;
    for _ in 0..300 {
        if record.person_for_a_reader(id.as_str(), NO_MEMBER) == MemberPerson::Shared {
            read_the_earlier_mob = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let destroyed = Box::pin(set_of(&host).mob_destroy(&id)).await;
    // A reading the destroy started ends before the record is turned away.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after_the_destroy = record.person_for_a_reader(id.as_str(), NO_MEMBER);
    record.bind_child_mobs(&set_of(&elsewhere));

    let anew = created_by_a_member(&host, again, &creator).await;
    let after_the_create = record.person_for_a_reader(id.as_str(), NO_MEMBER);

    shut_down(&host, &[&id]).await;
    shut_down(&elsewhere, &[]).await;
    assert!(
        earlier.is_ok() && destroyed.is_ok() && anew.is_ok(),
        "a member creates a mob, the set destroys it, the member creates one under its id: \
         {:?}, {:?}, {:?}",
        earlier.as_ref().err().map(ToString::to_string),
        destroyed.as_ref().err().map(ToString::to_string),
        anew.as_ref().err().map(ToString::to_string)
    );
    assert!(
        read_the_earlier_mob,
        "the record read the earlier mob: a name it does not hold there is private to no one"
    );
    assert_eq!(
        after_the_destroy,
        MemberPerson::Shared,
        "what the record read stands for the id until a mob is created under it"
    );
    assert_eq!(
        after_the_create,
        MemberPerson::NotKnown,
        "the new mob is not read: a name the record does not hold in it is not known"
    );
}

/// A create that comes before the follower's next pass finds the destroyed
/// mob's entry still in the table, with what served that mob still running:
/// its events subscriber, which stores a cursor under the id, and its
/// identity runtime, whose lifecycle projection stores a cursor and
/// failures under it. The create ends that entry before it releases what
/// the host keeps under the id, so nothing of the destroyed mob writes
/// after the release, and the entry's end no longer comes after the new mob
/// was given its console event store.
#[tokio::test]
async fn a_create_before_the_followers_next_pass_ends_the_destroyed_mobs_entry_first() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_launch_mob) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    host.compose_host_mob_identities(Arc::new(gateways_composer(&roster, None)));
    let of_the_second = definition("second");
    let again = of_the_second.clone();
    let id = of_the_second.id.clone();
    roster.configure(&id, &["back"]);
    second_mob(&host, of_the_second).await;
    let of_the_destroyed = host
        .resolve_mob(Some(&id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned())
        .expect("the second mob has an identity runtime");
    let its_entry = host
        .mob_entry_in_tests(&id)
        .expect("the second mob has its entry");
    // No pass comes on its own from here: the follower's next pass is the
    // one a request asks for below, and the destroy asks for none (a
    // destroy asks for a pass of its own, on a task; the row is of the
    // create that comes before that pass ran).
    host.hold_the_follower_of_host_mobs_in_tests().await;
    host.destroys_of_host_mobs_ask_for_no_pass_in_tests();

    let destroyed = Box::pin(set_of(&host).mob_destroy(&id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert!(
        host.mob_entry_in_tests(&id)
            .is_some_and(|entry| Arc::ptr_eq(&entry, &its_entry)),
        "the destroyed mob's entry stands until a pass"
    );
    // The create, through the set alone: no request resolves a mob, so no
    // pass runs before it answered.
    assert_eq!(
        second_mob(&host, again).await,
        id,
        "a mob under the same id"
    );

    let entry_left = host.mob_entry_in_tests(&id).is_none();
    let late = tokio::time::timeout(
        Duration::from_secs(20),
        of_the_destroyed.send_tracked(
            &identity("back"),
            &meerkat_core::ContentInput::Text("a word after the create".to_string()),
        ),
    )
    .await
    .map(|answer| answer.map(|_| ()));
    let ended = matches!(&late, Ok(answer) if refused_as_shutting_down(answer));
    // The new mob's console event store, named before the next pass.
    let named = host.console_events().of_mob(Some(&id));
    assert!(
        host.resolve_mob(Some(&id)).await.is_ok(),
        "the mob created under the id is served"
    );
    let kept = host.console_events().of_mob(Some(&id)).is_the_store(&named);
    assert!(
        entry_left && ended && kept,
        "when the create answered, the destroyed mob's entry had left the table \
         ({entry_left}) and what served that mob had ended, its identity runtime refusing \
         as shutting down ({ended}: {late:?}); and the new mob keeps the console event \
         store it had before the next pass ({kept})"
    );
    shut_down(&host, &[&id]).await;
}

/// A metadata store that keeps a release waiting when a row asks it to:
/// the create that asked for it stands inside the composition, after the
/// mob set found its id free, until the row lets it go.
#[derive(Default)]
struct HeldRelease {
    inner: crate::runtime::InMemoryMetadataStore,
    hold_the_next: AtomicBool,
    asked: tokio::sync::Notify,
    go: tokio::sync::Notify,
    /// The mob ids whose records' release was asked, in order.
    releases: Mutex<Vec<String>>,
}

impl HeldRelease {
    /// The mob ids whose records' release was asked, in order.
    fn releases(&self) -> Vec<String> {
        self.releases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The next release waits for [`Self::let_go`].
    fn hold_the_next_release(&self) {
        self.hold_the_next.store(true, Ordering::SeqCst);
    }

    /// Answers once the release that waits was asked.
    async fn asked_for_the_release(&self) {
        self.asked.notified().await;
    }

    /// The release that waits, or the next one to wait, goes on.
    fn let_go(&self) {
        self.go.notify_one();
    }
}

#[async_trait::async_trait]
impl PersistentMetadataStore for HeldRelease {
    async fn get_subscription_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<u64>, crate::MetadataStoreError> {
        self.inner.get_subscription_cursor(mob_id).await
    }

    async fn set_subscription_cursor(
        &self,
        mob_id: &str,
        cursor: u64,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner.set_subscription_cursor(mob_id, cursor).await
    }

    async fn load_member_idle_retire_overrides(
        &self,
    ) -> Result<Vec<crate::MemberIdleRetireOverrideRecord>, crate::MetadataStoreError> {
        self.inner.load_member_idle_retire_overrides().await
    }

    async fn set_member_idle_retire_override(
        &self,
        record: &crate::MemberIdleRetireOverrideRecord,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner.set_member_idle_retire_override(record).await
    }

    async fn clear_member_idle_retire_override(
        &self,
        record: &crate::MemberIdleRetireOverrideRecord,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner.clear_member_idle_retire_override(record).await
    }

    async fn identity_tool_record(
        &self,
        mob_id: &str,
        identity: &str,
    ) -> Result<Option<crate::IdentityToolRecord>, crate::MetadataStoreError> {
        self.inner.identity_tool_record(mob_id, identity).await
    }

    async fn set_identity_tool_record(
        &self,
        record: &crate::IdentityToolRecord,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner.set_identity_tool_record(record).await
    }

    async fn load_identity_rotation_intents(
        &self,
        mob_id: &str,
    ) -> Result<Vec<crate::IdentityRotationIntent>, crate::MetadataStoreError> {
        self.inner.load_identity_rotation_intents(mob_id).await
    }

    async fn set_identity_rotation_intent(
        &self,
        intent: &crate::IdentityRotationIntent,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner.set_identity_rotation_intent(intent).await
    }

    async fn clear_identity_rotation_intent(
        &self,
        mob_id: &str,
        identity: &str,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner
            .clear_identity_rotation_intent(mob_id, identity)
            .await
    }

    async fn lifecycle_projection_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<crate::LifecycleProjectionCursor>, crate::MetadataStoreError> {
        self.inner.lifecycle_projection_cursor(mob_id).await
    }

    async fn set_lifecycle_projection_cursor(
        &self,
        cursor: &crate::LifecycleProjectionCursor,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner.set_lifecycle_projection_cursor(cursor).await
    }

    async fn lifecycle_effect_intent(
        &self,
        mob_id: &str,
        key: &crate::LifecycleIntentKey,
    ) -> Result<Option<crate::LifecycleEffectIntent>, crate::MetadataStoreError> {
        self.inner.lifecycle_effect_intent(mob_id, key).await
    }

    async fn set_lifecycle_effect_intent(
        &self,
        intent: &crate::LifecycleEffectIntent,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner.set_lifecycle_effect_intent(intent).await
    }

    async fn clear_lifecycle_effect_intent(
        &self,
        mob_id: &str,
        key: &crate::LifecycleIntentKey,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner.clear_lifecycle_effect_intent(mob_id, key).await
    }

    async fn lifecycle_effect_intents(
        &self,
        mob_id: &str,
    ) -> Result<Vec<crate::LifecycleEffectIntent>, crate::MetadataStoreError> {
        self.inner.lifecycle_effect_intents(mob_id).await
    }

    async fn record_lifecycle_effect_failure(
        &self,
        failure: &crate::LifecycleEffectFailure,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner.record_lifecycle_effect_failure(failure).await
    }

    async fn lifecycle_effect_failures(
        &self,
        mob_id: &str,
        skip: usize,
        limit: usize,
    ) -> Result<Vec<crate::LifecycleEffectFailure>, crate::MetadataStoreError> {
        self.inner
            .lifecycle_effect_failures(mob_id, skip, limit)
            .await
    }

    async fn prune_lifecycle_effect_failures(
        &self,
        mob_id: &str,
        keep: usize,
    ) -> Result<usize, crate::MetadataStoreError> {
        self.inner
            .prune_lifecycle_effect_failures(mob_id, keep)
            .await
    }

    async fn clear_lifecycle_effect_failure(
        &self,
        mob_id: &str,
        generation: u64,
        effect: crate::runtime::LifecycleEffectKind,
        identity: &str,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner
            .clear_lifecycle_effect_failure(mob_id, generation, effect, identity)
            .await
    }

    async fn release_mob_records(
        &self,
        mob_id: &str,
    ) -> Result<crate::MobRecordsRelease, crate::MetadataStoreError> {
        if self.hold_the_next.swap(false, Ordering::SeqCst) {
            self.asked.notify_one();
            self.go.notified().await;
        }
        self.releases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(mob_id.to_string());
        self.inner.release_mob_records(mob_id).await
    }

    async fn settle_detached_writes(&self) {
        self.inner.settle_detached_writes().await;
    }

    fn realm_document_retention(&self) -> Option<crate::RecordRetention> {
        self.inner.realm_document_retention()
    }

    async fn realm_document(&self, key: &str) -> Result<Option<String>, crate::MetadataStoreError> {
        self.inner.realm_document(key).await
    }

    async fn set_realm_document(
        &self,
        key: &str,
        document: Option<&str>,
    ) -> Result<(), crate::MetadataStoreError> {
        self.inner.set_realm_document(key, document).await
    }

    async fn realm_document_keys(
        &self,
        prefix: &str,
    ) -> Result<Vec<String>, crate::MetadataStoreError> {
        self.inner.realm_document_keys(prefix).await
    }

    async fn bind_decision_event(
        &self,
        mob_id: &str,
        ingress: &str,
        event_ref: &str,
        decision_id: &str,
    ) -> Result<String, crate::MetadataStoreError> {
        self.inner
            .bind_decision_event(mob_id, ingress, event_ref, decision_id)
            .await
    }
}

/// The mob that got the id writes what a mob keeps with its host: a
/// lifecycle failure of its incarnation and an idle-retirement opt-in in
/// the host's metadata store, and a private member in the launch's record.
async fn the_mob_that_got_the_id_writes_its_records(
    host: &UnifiedRuntime,
    store: &Arc<dyn PersistentMetadataStore>,
    id: &MobId,
    owner: &str,
) {
    let handle = set_of(host)
        .handle_for(id)
        .await
        .expect("the set serves the mob that got the id");
    store
        .record_lifecycle_effect_failure(&crate::runtime::LifecycleEffectFailure::new(
            id.as_str(),
            handle.incarnation().to_string(),
            12,
            "desk",
            3,
            crate::runtime::LifecycleEffectKind::RetirementHook,
            3,
            "timed out",
            1_700_000_000_001,
        ))
        .await
        .expect("a lifecycle failure of the mob");
    store
        .set_member_idle_retire_override(&crate::runtime::MemberIdleRetireOverrideRecord {
            mob_id: id.to_string(),
            member_id: "helper".to_string(),
            session_id: meerkat_core::types::SessionId::new(),
            policy: crate::mob_handle_runtime::DelegateIdleRetireOverride::Disabled,
            recorded_at: chrono::Utc::now(),
        })
        .await
        .expect("an idle-retirement opt-in of the mob");
    let seated = Box::pin(
        handle.spawn_spec(
            SpawnMemberSpec::new("worker", "side").with_labels(BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                owner.to_string(),
            )])),
        ),
    )
    .await;
    assert!(
        seated.is_ok(),
        "a private member of the mob that got the id: {:?}",
        seated.as_ref().err()
    );
}

/// Two creates under one id: one gets the id, and what its mob keeps with
/// the host stays whole. The first create stands inside the composition,
/// after the mob set found the id free, while the second runs; whichever
/// of the two gets the id, the other's create takes none of the records
/// the winner's mob wrote and the launch's record still knows the winner's
/// private member.
///
/// The mob set reserves a create's id before it asks the composition, so
/// the create that loses the id releases nothing the winner's mob wrote.
#[tokio::test]
async fn two_creates_under_one_id_leave_the_winners_records_whole() {
    const OWNER: &str = "console-caller:v1:aa";
    let temp = tempfile::tempdir().expect("tempdir");
    let held = Arc::new(HeldRelease::default());
    let store: Arc<dyn PersistentMetadataStore> = Arc::clone(&held) as _;
    let (host, _sessions, _machine, ()) = launch_keeping(
        &temp,
        definition("host"),
        meerkat::Config::default(),
        Arc::clone(&store),
        |_| (),
        |spec| spec,
    )
    .await;
    let of_the_second = definition("second");
    let id = of_the_second.id.clone();
    let create = |definition: meerkat_mob::MobDefinition| {
        let set = set_of(&host);
        tokio::spawn(async move {
            Box::pin(set.mob_create_definition(definition))
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    };

    held.hold_the_next_release();
    let first = create(of_the_second.clone());
    tokio::time::timeout(Duration::from_secs(30), held.asked_for_the_release())
        .await
        .expect("the first create stands inside the composition");
    // The second create runs to its answer while the first stands; a set
    // that makes it wait for the first is given twenty seconds to say so.
    let mut second = create(of_the_second);
    let mut second_answer = tokio::time::timeout(Duration::from_secs(20), &mut second)
        .await
        .ok()
        .map(|joined| joined.expect("the second create ran"));
    let mut written = false;
    if matches!(second_answer, Some(Ok(()))) {
        the_mob_that_got_the_id_writes_its_records(&host, &store, &id, OWNER).await;
        written = true;
    }
    held.let_go();
    let first_answer = tokio::time::timeout(Duration::from_secs(30), first)
        .await
        .expect("the first create answers once it is let go")
        .expect("the first create ran");
    let second_answer = match second_answer.take() {
        Some(answer) => answer,
        None => tokio::time::timeout(Duration::from_secs(30), second)
            .await
            .expect("the second create answers after the first")
            .expect("the second create ran"),
    };
    assert!(
        first_answer.is_ok() != second_answer.is_ok(),
        "one of the two creates gets the id: the first {first_answer:?}, the second \
         {second_answer:?}"
    );
    if !written {
        the_mob_that_got_the_id_writes_its_records(&host, &store, &id, OWNER).await;
    }

    let failures = store
        .lifecycle_effect_failures(id.as_str(), 0, 16)
        .await
        .expect("the lifecycle failures")
        .len();
    let opt_ins = store
        .load_member_idle_retire_overrides()
        .await
        .expect("the opt-ins")
        .iter()
        .filter(|record| record.mob_id == id.as_str())
        .count();
    let owner = host
        .mob_runtime()
        .private_owners()
        .owner_of(id.as_str(), "side");
    assert!(
        failures == 1 && opt_ins == 1 && owner.as_deref() == Some(OWNER),
        "the records of the mob that got the id are whole after the other create answered: \
         lifecycle failures {failures} (1), idle-retirement opt-ins {opt_ins} (1), the owner \
         of its private member {owner:?}"
    );
    shut_down(&host, &[&id]).await;
}

/// The owner of the rows' private members.
const SCHEDULES_OWNER: &str = "console-caller:v1:aa";

/// A release of a mob's schedules of the rows' own, in the place of the
/// one a launch's schedule service binds: it keeps each mob it was asked
/// for, with what the launch's record of private members said then of the
/// owner of that mob's member `side`, and it fails when told to.
struct SchedulesOfTheRows {
    record: crate::private_owners::PrivateOwners,
    asked: Mutex<Vec<(String, Option<String>)>>,
    fails: AtomicBool,
}

impl SchedulesOfTheRows {
    fn of(host: &UnifiedRuntime) -> Arc<Self> {
        Arc::new(Self {
            record: host.mob_runtime().private_owners().clone(),
            asked: Mutex::new(Vec::new()),
            fails: AtomicBool::new(false),
        })
    }

    fn asked(&self) -> Vec<(String, Option<String>)> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl crate::mob_schedules_release::MobSchedulesRelease for SchedulesOfTheRows {
    async fn release_mob(&self, mob_id: &MobId) -> Result<usize, String> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((
                mob_id.to_string(),
                self.record.owner_of(mob_id.as_str(), "side"),
            ));
        if self.fails.load(Ordering::SeqCst) {
            Err("the schedule store of the rows is closed".to_string())
        } else {
            Ok(1)
        }
    }
}

/// A launch that declared schedules before its runtime was built.
async fn launch_with_schedules_declared(temp: &tempfile::TempDir) -> UnifiedRuntime {
    let (host, _sessions, _machine, ()) = launch_composed(
        temp,
        definition("host"),
        meerkat::Config::default(),
        |_| (),
        crate::mob_handle_runtime::MobBootstrapSpec::with_mob_schedules_declared,
    )
    .await;
    host
}

/// A second mob of `host` with the private member `side`, destroyed
/// through the mob set: its id and the definition to create it again from.
/// The launch's record still names the member's owner.
async fn a_destroyed_second_mob_with_a_private_member(
    host: &UnifiedRuntime,
) -> (MobId, meerkat_mob::MobDefinition) {
    let of_the_second = definition("second");
    let again = of_the_second.clone();
    let id = second_mob(host, of_the_second).await;
    let seated = Box::pin(
        set_of(host)
            .handle_for(&id)
            .await
            .expect("the set serves the second mob")
            .spawn_spec(
                SpawnMemberSpec::new("worker", "side").with_labels(BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    SCHEDULES_OWNER.to_string(),
                )])),
            ),
    )
    .await;
    assert!(
        seated.is_ok(),
        "a private member of the second mob: {:?}",
        seated.as_ref().err()
    );
    let destroyed = Box::pin(set_of(host).mob_destroy(&id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        host.mob_runtime()
            .private_owners()
            .owner_of(id.as_str(), "side")
            .as_deref(),
        Some(SCHEDULES_OWNER),
        "the launch's record still names the destroyed mob's private member's owner"
    );
    (id, again)
}

/// A create releases the schedules the launch keeps under the mob's id
/// while the launch's record still knows the earlier mob's private
/// members: that record is what keeps a private member's schedule from
/// other agents until the schedule is ended. Once its releases passed, the
/// create forgets the members the record holds under the id, a host's
/// create as a member's: the mob created under the id is told nothing of
/// them, and a member built in it is what its own build says.
#[tokio::test]
async fn a_create_releases_the_schedules_under_its_id_and_then_forgets_the_members_the_record_holds_under_it()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_with_schedules_declared(&temp).await;
    let schedules = SchedulesOfTheRows::of(&host);
    host.mob_runtime()
        .mob_schedules()
        .bind(Arc::clone(&schedules) as _);
    let (id, again) = a_destroyed_second_mob_with_a_private_member(&host).await;

    assert_eq!(
        second_mob(&host, again).await,
        id,
        "a mob under the same id"
    );
    assert_eq!(
        schedules.asked(),
        vec![
            (id.to_string(), None),
            (id.to_string(), Some(SCHEDULES_OWNER.to_string())),
        ],
        "each create asked for the release of the schedules under the id, the second while \
         the launch's record still named the earlier mob's private member's owner"
    );
    assert_eq!(
        host.mob_runtime()
            .private_owners()
            .owner_of(id.as_str(), "side")
            .as_deref(),
        None,
        "and the host's create then forgot what the record held of the earlier mob's member"
    );
    shut_down(&host, &[&id]).await;
}

/// A member's create asks for the release of the schedules under its id
/// once: the launch's schedules are one of the releases bound to the
/// launch's record, and nothing else of the create releases them.
#[tokio::test]
async fn a_members_create_asks_for_the_release_of_the_schedules_under_its_id_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_with_schedules_declared(&temp).await;
    let schedules = SchedulesOfTheRows::of(&host);
    host.mob_runtime()
        .mob_schedules()
        .bind(Arc::clone(&schedules) as _);
    let creator = a_member_that_creates_mobs(&host, "bob", None).await;
    let of_the_mob = definition("second");
    let id = of_the_mob.id.clone();

    let created = created_by_a_member(&host, of_the_mob, &creator).await;
    let asked = schedules.asked();
    shut_down(&host, &[&id]).await;
    assert!(
        created.is_ok(),
        "the member's create goes through: {:?}",
        created.as_ref().map_err(ToString::to_string)
    );
    assert_eq!(
        asked,
        vec![(id.to_string(), None)],
        "the release of the schedules under the id was asked for once"
    );
}

/// A create is refused when the schedules kept under its id cannot be
/// released: the new mob would find an earlier mob's schedules. The
/// launch's record of private members is left as it was, so what it hides
/// stays hidden, the refusal carries nothing of the release's own failure,
/// and no mob is left under the id.
#[tokio::test]
async fn a_create_is_refused_when_the_schedules_under_its_id_cannot_be_released() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_with_schedules_declared(&temp).await;
    let schedules = SchedulesOfTheRows::of(&host);
    host.mob_runtime()
        .mob_schedules()
        .bind(Arc::clone(&schedules) as _);
    let (id, again) = a_destroyed_second_mob_with_a_private_member(&host).await;

    schedules.fails.store(true, Ordering::SeqCst);
    let created = Box::pin(set_of(&host).mob_create_definition(again)).await;
    let answer = created
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        created.is_err()
            && answer.contains(crate::private_owners::CREATE_RELEASE_FAILED)
            && !answer.contains("of the rows"),
        "the create is refused with the one sentence of a failed release, with nothing of \
         the release's own failure: {:?}",
        created.as_ref().map(|_| "created")
    );
    assert_eq!(
        host.mob_runtime()
            .private_owners()
            .owner_of(id.as_str(), "side")
            .as_deref(),
        Some(SCHEDULES_OWNER),
        "the launch's record of private members is as it was"
    );
    assert!(
        set_of(&host).handle_for(&id).await.is_err(),
        "and the set holds no mob under the id"
    );
    shut_down(&host, &[]).await;
}

/// A launch that declared schedules whose release is not bound yet refuses
/// the create of a second mob: nothing could release what an earlier mob
/// of the id left. Once the release is bound the create goes through.
#[tokio::test]
async fn a_create_is_refused_while_the_launchs_schedules_are_declared_and_not_bound() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_with_schedules_declared(&temp).await;
    let of_the_second = definition("second");
    let again = of_the_second.clone();
    let id = of_the_second.id.clone();

    let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    assert!(
        matches!(
            &created,
            Err(error) if error.to_string().contains(crate::private_owners::CREATE_RELEASE_FAILED)
        ),
        "the create is refused until the launch's schedules are bound: {:?}",
        created.as_ref().map(|_| "created")
    );
    assert!(
        set_of(&host).handle_for(&id).await.is_err(),
        "and the set holds no mob under the id"
    );

    host.mob_runtime()
        .mob_schedules()
        .bind(SchedulesOfTheRows::of(&host) as _);
    assert_eq!(
        second_mob(&host, again).await,
        id,
        "the create goes through once the release is bound"
    );
    shut_down(&host, &[&id]).await;
}

/// The person the rows' private member is private to.
const CREATORS_OWNER: &str = "console-caller:v1:bb";

/// A member of `host`'s launch mob under `name`, private to `owner` when
/// one is given: the session it creates mobs with, as its mob tools do.
async fn a_member_that_creates_mobs(
    host: &UnifiedRuntime,
    name: &str,
    owner: Option<&str>,
) -> meerkat_core::types::SessionId {
    let mut spec = SpawnMemberSpec::new("worker", name);
    if let Some(owner) = owner {
        spec = spec.with_labels(BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            owner.to_string(),
        )]));
    }
    let seated = Box::pin(host.mob_handle().spawn_spec(spec)).await;
    assert!(
        seated.is_ok(),
        "a member of the launch mob: {:?}",
        seated.as_ref().err()
    );
    host.mob_handle()
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(name))
        .await
        .expect("the member's session")
}

/// The create of a mob from `definition` by the member whose session is
/// `creator`, as that member's mob tools make it.
async fn created_by_a_member(
    host: &UnifiedRuntime,
    definition: meerkat_mob::MobDefinition,
    creator: &meerkat_core::types::SessionId,
) -> Result<MobId, meerkat_mob::MobError> {
    Box::pin(
        set_of(host).mob_create_definition_with_owner_bridge_session(
            definition,
            creator.clone(),
            true,
            false,
        ),
    )
    .await
}

/// The person a member seated now in the mob `id` is private to: the rows
/// seat one under `name` and read the owner its spawn was given.
async fn owner_of_a_member_seated_in(
    host: &UnifiedRuntime,
    id: &MobId,
    name: &str,
) -> Option<String> {
    let handle = set_of(host)
        .handle_for(id)
        .await
        .expect("the set serves the mob");
    let seated = Box::pin(handle.spawn_spec(SpawnMemberSpec::new("worker", name))).await;
    assert!(
        seated.is_ok(),
        "a member of the mob: {:?}",
        seated.as_ref().err()
    );
    handle
        .get_member(&meerkat_mob::AgentIdentity::from(name))
        .await
        .ok()
        .flatten()
        .and_then(|entry| crate::access::private_owner(&entry.labels).map(str::to_string))
}

/// The set destroys the mob `id`.
async fn destroyed(host: &UnifiedRuntime, id: &MobId) {
    let destroyed = Box::pin(set_of(host).mob_destroy(id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the mob: {:?}",
        destroyed.as_ref().err()
    );
}

/// A mob a member creates under the id of a destroyed mob another member
/// created takes nothing of the earlier mob's creator: its members are
/// private to the person its own creator is private to, here nobody.
#[tokio::test]
async fn a_members_mob_under_a_destroyed_members_mobs_id_is_private_to_its_own_creators_person() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let private = a_member_that_creates_mobs(&host, "ann", Some(CREATORS_OWNER)).await;
    let shared = a_member_that_creates_mobs(&host, "bob", None).await;
    let of_the_mob = definition("second");
    let again = of_the_mob.clone();

    let id = created_by_a_member(&host, of_the_mob, &private)
        .await
        .expect("the private member's mob");
    assert_eq!(
        owner_of_a_member_seated_in(&host, &id, "kid")
            .await
            .as_deref(),
        Some(CREATORS_OWNER),
        "a member of a private member's mob is private to its creator's person"
    );
    destroyed(&host, &id).await;

    assert_eq!(
        created_by_a_member(&host, again, &shared)
            .await
            .expect("the shared member's mob under the same id"),
        id
    );
    assert_eq!(
        owner_of_a_member_seated_in(&host, &id, "kid").await,
        None,
        "a member of the shared member's mob is private to nobody: the earlier mob's creator \
         is not its creator"
    );
    shut_down(&host, &[&id]).await;
}

/// A mob a private member creates under the id of a destroyed mob the host
/// created is private to its creator's person.
#[tokio::test]
async fn a_private_members_mob_under_a_destroyed_host_mobs_id_is_private_to_its_creators_person() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let private = a_member_that_creates_mobs(&host, "ann", Some(CREATORS_OWNER)).await;
    let of_the_mob = definition("second");
    let again = of_the_mob.clone();

    let id = second_mob(&host, of_the_mob).await;
    destroyed(&host, &id).await;

    assert_eq!(
        created_by_a_member(&host, again, &private)
            .await
            .expect("the private member's mob under the same id"),
        id
    );
    assert_eq!(
        owner_of_a_member_seated_in(&host, &id, "kid")
            .await
            .as_deref(),
        Some(CREATORS_OWNER),
        "a member of the private member's mob is private to its creator's person"
    );
    shut_down(&host, &[&id]).await;
}

/// How many idle-retirement opt-ins `store` keeps under the mob id `id`.
async fn opt_ins_under(store: &Arc<dyn PersistentMetadataStore>, id: &MobId) -> usize {
    store
        .load_member_idle_retire_overrides()
        .await
        .expect("the opt-ins")
        .iter()
        .filter(|record| record.mob_id == id.as_str())
        .count()
}

/// An idle-retirement opt-in of a member of the mob `id`, left in `store`.
async fn an_opt_in_left_under(store: &Arc<dyn PersistentMetadataStore>, id: &MobId) {
    store
        .set_member_idle_retire_override(&crate::runtime::MemberIdleRetireOverrideRecord {
            mob_id: id.to_string(),
            member_id: "helper".to_string(),
            session_id: meerkat_core::types::SessionId::new(),
            policy: crate::mob_handle_runtime::DelegateIdleRetireOverride::Disabled,
            recorded_at: chrono::Utc::now(),
        })
        .await
        .expect("an idle-retirement opt-in of the mob");
}

/// A mob a member creates under the id of a destroyed mob reads none of
/// the records the host still keeps under that id: the create releases
/// them, as a host's create does.
#[tokio::test]
async fn a_mob_a_member_creates_under_a_destroyed_mobs_id_reads_none_of_its_records() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    let (host, _sessions, _machine, ()) = launch_keeping(
        &temp,
        definition("host"),
        meerkat::Config::default(),
        Arc::clone(&store),
        |_| (),
        |spec| spec,
    )
    .await;
    let creator = a_member_that_creates_mobs(&host, "bob", None).await;
    let of_the_mob = definition("second");
    let again = of_the_mob.clone();

    let id = created_by_a_member(&host, of_the_mob, &creator)
        .await
        .expect("the member's mob");
    an_opt_in_left_under(&store, &id).await;
    destroyed(&host, &id).await;
    assert_eq!(
        opt_ins_under(&store, &id).await,
        1,
        "the destroyed mob's record is left under its id"
    );

    assert_eq!(
        created_by_a_member(&host, again, &creator)
            .await
            .expect("the member's mob under the same id"),
        id
    );
    assert_eq!(
        opt_ins_under(&store, &id).await,
        0,
        "the mob created under the id finds none of the destroyed mob's records"
    );
    shut_down(&host, &[&id]).await;
}

/// A member's create is refused when what the host keeps under the id
/// cannot be released: it is answered as a refused create, and no mob is
/// left under the id.
#[tokio::test]
async fn a_members_create_is_refused_when_what_is_kept_under_the_id_cannot_be_released() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("meta.sqlite");
    let store: Arc<dyn PersistentMetadataStore> = Arc::new(
        crate::runtime::SqliteMetadataStore::open(&path).expect("a metadata store in a file"),
    );
    let launch_temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions, _machine, ()) = launch_keeping(
        &launch_temp,
        definition("host"),
        meerkat::Config::default(),
        store,
        |_| (),
        |spec| spec,
    )
    .await;
    let creator = a_member_that_creates_mobs(&host, "bob", None).await;
    // The store's table goes: every statement on it fails from here.
    rusqlite::Connection::open(&path)
        .and_then(|connection| connection.execute("DROP TABLE meta_metadata", []))
        .expect("the table is dropped");
    let of_the_mob = definition("second");
    let id = of_the_mob.id.clone();

    let created = created_by_a_member(&host, of_the_mob, &creator).await;
    assert!(
        matches!(
            &created,
            Err(error) if meerkat_mob_mcp::MobMcpState::is_create_refusal(error)
                && error.to_string().contains(crate::private_owners::CREATE_RELEASE_FAILED)
        ),
        "the member's create is refused: {:?}",
        created
            .as_ref()
            .map(|_| "created")
            .map_err(ToString::to_string)
    );
    assert!(
        set_of(&host).handle_for(&id).await.is_err(),
        "and the set holds no mob under the id"
    );
    shut_down(&host, &[]).await;
}

/// A member's create is refused on a host whose metadata store cannot
/// release a mob's records: nothing could remove what an earlier mob of
/// the id left there. It is answered as a refused create, and no mob is
/// left under the id.
#[tokio::test]
async fn a_members_create_is_refused_on_a_store_that_cannot_release_a_mobs_records() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn PersistentMetadataStore> =
        Arc::new(crate::console_audit::test_support::ListingStore::default());
    let (host, _sessions, _machine, ()) = launch_keeping(
        &temp,
        definition("host"),
        meerkat::Config::default(),
        store,
        |_| (),
        |spec| spec,
    )
    .await;
    let creator = a_member_that_creates_mobs(&host, "bob", None).await;
    let of_the_mob = definition("second");
    let id = of_the_mob.id.clone();

    let created = created_by_a_member(&host, of_the_mob, &creator).await;
    assert!(
        matches!(
            &created,
            Err(error) if meerkat_mob_mcp::MobMcpState::is_create_refusal(error)
                && error.to_string().contains(crate::private_owners::CREATE_RELEASE_FAILED)
        ),
        "the member's create is refused: {:?}",
        created
            .as_ref()
            .map(|_| "created")
            .map_err(ToString::to_string)
    );
    assert!(
        set_of(&host).handle_for(&id).await.is_err(),
        "and the set holds no mob under the id"
    );
    shut_down(&host, &[]).await;
}

/// A launch that does not declare its mob to its mob set (here a runtime
/// the builder makes from a definition, which opens its mob's storage
/// itself) hands its mob set no composition. A create its host makes
/// through the set is admitted there, as it was before a launch could host
/// mobs: the mob is one of the mob set's own, built with the set's own
/// configuration, and no request of the runtime resolves it. A member
/// seated in it takes a turn; the launch's record holds the mob as private
/// to nobody; and the next launch over the same state serves the mob again
/// through the set.
#[tokio::test]
async fn a_hosts_create_on_a_launch_without_a_declared_store_makes_a_mob_of_the_mob_sets_own() {
    use futures::StreamExt as _;

    const THE_TASK: &str = "one short task for alice of the host's second mob";
    let state = tempfile::tempdir().expect("tempdir");
    let of_the_host = definition("host");
    let launch = || async {
        UnifiedRuntime::builder()
            .definition(of_the_host.clone())
            .persistent_state(state.path())
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build()
            .await
            .expect("a runtime the builder makes from a definition")
    };
    let host = launch().await;
    let of_the_second = definition("second");
    let id = of_the_second.id.clone();
    let alice = meerkat_mob::AgentIdentity::from("alice");

    let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    // Some(true): the turn that started with the task ran to its end.
    let mut the_turn_ended = None;
    if let Ok(handle) = set_of(&host).handle_for(&id).await {
        let seated = Box::pin(handle.spawn_spec(SpawnMemberSpec::new("worker", "alice"))).await;
        let events = handle.subscribe_agent_events(&alice).await;
        let member = handle.member(&alice).await;
        if let (Ok(_), Ok(mut events), Ok(member)) = (seated, events, member)
            && member.internal_turn(THE_TASK).await.is_ok()
        {
            the_turn_ended = tokio::time::timeout(Duration::from_secs(30), async {
                let mut started = false;
                while let Some(event) = events.next().await {
                    match &event.payload {
                        meerkat_core::AgentEvent::RunStarted { .. } => {
                            started = started
                                || serde_json::to_string(&event.payload)
                                    .is_ok_and(|run| run.contains(THE_TASK));
                        }
                        meerkat_core::AgentEvent::RunCompleted { .. } if started => {
                            return Some(true);
                        }
                        meerkat_core::AgentEvent::RunFailed { .. } if started => {
                            return Some(false);
                        }
                        _ => {}
                    }
                }
                None
            })
            .await
            .ok()
            .flatten();
        }
    }
    let recorded = host
        .mob_runtime()
        .private_owners()
        .recorded_mob_owner(id.as_str());
    let resolved_by_a_request = host.resolve_mob(Some(&id)).await.is_ok();
    // Shut down before a launch over the same state in this process: the
    // shutdown stands the durable root of the mob set's mobs down within
    // its terminal window, which a loaded host can outrun, and a launch
    // over a root still claimed is refused. Stand down again, bounded,
    // until the root is released.
    let released = |outcome: &crate::ChildMobStandDownOutcome| {
        matches!(
            outcome,
            crate::ChildMobStandDownOutcome::Released { .. }
                | crate::ChildMobStandDownOutcome::NotConfigured
        )
    };
    let mut stood_down = tokio::time::timeout(Duration::from_secs(30), host.shutdown())
        .await
        .expect("the runtime shuts down")
        .child_mob_stand_down;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !released(&stood_down) && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        stood_down = host.mob_runtime().stand_down_child_mobs(deadline).await;
    }
    let root_released = released(&stood_down);
    drop(host);

    let mut served_again = false;
    if root_released {
        let host = launch().await;
        served_again = set_of(&host).handle_for(&id).await.is_ok();
        shut_down(&host, &[&id]).await;
    }

    assert!(
        created.is_ok(),
        "the host's create is admitted: {:?}",
        created
            .as_ref()
            .map(|_| "created")
            .map_err(ToString::to_string)
    );
    assert_eq!(
        the_turn_ended,
        Some(true),
        "a member seated in the mob was given a turn that started with its task and ran to \
         its end"
    );
    assert_eq!(
        recorded,
        Some(None),
        "the launch's record holds the mob as private to nobody"
    );
    assert!(
        !resolved_by_a_request,
        "no request of the runtime resolves the mob: it is the mob set's own"
    );
    assert!(
        root_released,
        "the durable root of the mob set's mobs is released before the next launch"
    );
    assert!(
        served_again,
        "the next launch over the same state serves the mob again through the set"
    );
}

/// On a launch that hosts no mob beside its launch mob a member's create
/// still releases what the host keeps under the id, as on every launch:
/// the release hangs on the create, not on how the launch holds its store.
#[tokio::test]
async fn a_members_create_releases_what_is_kept_under_the_id_on_a_launch_that_hosts_no_second_mob()
{
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_over_opened_storage(&temp, definition("host")).await;
    let store = Arc::clone(host.persistent_metadata());
    let creator = a_member_that_creates_mobs(&host, "bob", None).await;
    let of_the_mob = definition("second");
    let again = of_the_mob.clone();

    let id = created_by_a_member(&host, of_the_mob, &creator)
        .await
        .expect("the member's mob");
    an_opt_in_left_under(&store, &id).await;
    destroyed(&host, &id).await;

    assert_eq!(
        created_by_a_member(&host, again, &creator)
            .await
            .expect("the member's mob under the same id"),
        id
    );
    assert_eq!(
        opt_ins_under(&store, &id).await,
        0,
        "the mob created under the id finds none of the destroyed mob's records"
    );
    shut_down(&host, &[&id]).await;
}

/// On a launch over storage it opened itself (declared to no mob set) a
/// host's create and a member's create both go through, and each asks the
/// releases bound to the launch's record, once.
#[tokio::test]
async fn a_hosts_create_on_a_launch_over_opened_storage_asks_the_bound_releases() {
    /// A release bound to the record that counts the creates it was asked
    /// for.
    #[derive(Default)]
    struct Counted(Mutex<Vec<String>>);
    #[async_trait::async_trait]
    impl crate::private_owners::MobCreateRelease for Counted {
        fn what(&self) -> &'static str {
            "what the row counts"
        }
        async fn release_before_create(&self, mob_id: &MobId) -> Result<(), String> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(mob_id.to_string());
            Ok(())
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_over_opened_storage(&temp, definition("host")).await;
    let counted = Arc::new(Counted::default());
    host.mob_runtime()
        .private_owners()
        .bind_create_release(Arc::clone(&counted) as _);
    let of_the_second = definition("second");
    let second = of_the_second.id.clone();

    let by_the_host = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    let creator = a_member_that_creates_mobs(&host, "bob", None).await;
    let of_the_member = definition("third");
    let third = of_the_member.id.clone();
    let by_a_member = created_by_a_member(&host, of_the_member, &creator).await;
    let asked = counted
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    shut_down(&host, &[&second, &third]).await;

    assert!(
        by_the_host.is_ok(),
        "the host's create goes through: {:?}",
        by_the_host
            .as_ref()
            .map(|_| "created")
            .map_err(ToString::to_string)
    );
    assert!(
        by_a_member.is_ok(),
        "a member's create goes through as before: {:?}",
        by_a_member.as_ref().map_err(ToString::to_string)
    );
    assert_eq!(
        asked,
        vec![second.to_string(), third.to_string()],
        "each create asked the release bound to the record, once"
    );
}

/// A member that a private member of a mob the host created asks for
/// there is private to that person, as in a mob a member created: the mob
/// set's own spawn customizer reads the launch's record. No model turn
/// spawns here: the row drives the customizer's body for the asking member
/// and seats what it returns.
#[tokio::test]
async fn a_private_members_spawn_in_a_mob_the_host_created_is_that_persons() {
    const OWNER: &str = "console-caller:v1:cc";
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_over_opened_storage(&temp, definition("host")).await;
    let of_the_second = definition("second");
    let id = of_the_second.id.clone();
    let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    let record = host.mob_runtime().private_owners().clone();
    let mut owner_of_the_spawn = None;
    if let Ok(handle) = set_of(&host).handle_for(&id).await {
        let side = Box::pin(
            handle.spawn_spec(
                SpawnMemberSpec::new("worker", "side").with_labels(BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    OWNER.to_string(),
                )])),
            ),
        )
        .await;
        let mut helper = SpawnMemberSpec::new("worker", "helper");
        let asked_by_the_private_member = record.apply_child(
            id.as_str(),
            Some(&meerkat_mob::AgentIdentity::from("side")),
            &mut helper,
        );
        if side.is_ok()
            && asked_by_the_private_member.is_ok()
            && Box::pin(handle.spawn_spec(helper)).await.is_ok()
        {
            owner_of_the_spawn = record.owner_of(id.as_str(), "helper");
        }
    }
    shut_down(&host, &[&id]).await;
    assert!(
        created.is_ok(),
        "the host's create goes through: {:?}",
        created
            .as_ref()
            .map(|_| "created")
            .map_err(ToString::to_string)
    );
    assert_eq!(
        owner_of_the_spawn.as_deref(),
        Some(OWNER),
        "the member the private member asked for is private to that person"
    );
}

/// A create asks the store for the release of the records kept under its
/// id once, a host's create as a member's: the runtime's table of entries
/// is one of the releases bound to the launch's record, and nothing else
/// of the create asks it.
#[tokio::test]
async fn a_create_asks_the_store_for_the_release_of_its_records_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let held = Arc::new(HeldRelease::default());
    let store: Arc<dyn PersistentMetadataStore> = Arc::clone(&held) as _;
    let (host, _sessions, _machine, ()) = launch_keeping(
        &temp,
        definition("host"),
        meerkat::Config::default(),
        store,
        |_| (),
        |spec| spec,
    )
    .await;
    let of_the_second = definition("second");
    let second = of_the_second.id.clone();
    let by_the_host = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    let creator = a_member_that_creates_mobs(&host, "bob", None).await;
    let of_the_member = definition("third");
    let third = of_the_member.id.clone();
    let by_a_member = created_by_a_member(&host, of_the_member, &creator).await;
    let asked = held.releases();
    shut_down(&host, &[&second, &third]).await;
    assert!(
        by_the_host.is_ok() && by_a_member.is_ok(),
        "both creates go through: {:?} {:?}",
        by_the_host
            .as_ref()
            .map(|_| "created")
            .map_err(ToString::to_string),
        by_a_member
            .as_ref()
            .map(|_| "created")
            .map_err(ToString::to_string)
    );
    assert_eq!(
        asked,
        vec![second.to_string(), third.to_string()],
        "each create asked the store for the release of its records, once"
    );
}

/// A host over a state directory that outlives its launches: its mob's
/// store in a file, and the mobs of its mob set kept durably beside it.
/// One launch opens the mob's store itself and declares it to no mob set;
/// another declares the same file to its mob set.
struct StoredHostOfTwoLaunches {
    dir: tempfile::TempDir,
    definition: meerkat_mob::MobDefinition,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    blobs: Arc<dyn meerkat_core::BlobStore>,
    sessions: Arc<dyn meerkat::SessionStore>,
    jobs: Arc<dyn meerkat::DetachedJobStore>,
    metadata: Arc<crate::runtime::InMemoryMetadataStore>,
}

impl StoredHostOfTwoLaunches {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("a state directory"),
            definition: definition("stored-host"),
            runtime_store: Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            blobs: Arc::new(crate::blob_store::Base64BlobStoreAdapter::new(Arc::new(
                crate::blob_store::ObjectStoreBlobStore::memory(),
            ))),
            sessions: Arc::new(meerkat_store::MemoryStore::new()),
            jobs: Arc::new(meerkat::MemoryDetachedJobStore::new()),
            metadata: Arc::new(crate::runtime::InMemoryMetadataStore::new()),
        }
    }

    /// A launch over the state directory: `declared` says whether it
    /// declares its mob's store to its mob set or opens it itself.
    async fn launch(&self, declared: bool) -> UnifiedRuntime {
        let machine = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(
                Arc::clone(&self.runtime_store),
                Arc::clone(&self.blobs),
            )
            .expect("the runtime machine"),
        );
        let mut builder = meerkat::FactoryAgentBuilder::new(
            meerkat::AgentFactory::new(self.dir.path())
                .builtins(false)
                .comms(true),
            meerkat::Config::default(),
        );
        builder.default_detached_job_store = Some(Arc::clone(&self.jobs));
        let service = Arc::new(meerkat_session::PersistentSessionService::new(
            builder,
            8,
            Arc::clone(&self.sessions),
            Arc::clone(&self.runtime_store),
            Arc::clone(&self.blobs),
        ));
        // The launch mob's store is where the layout over the state
        // directory names it, for both launches.
        let store_file =
            crate::MetaStorageLayout::with_injected_roots(self.dir.path().to_path_buf(), None)
                .event_log_db();
        let base = if declared {
            crate::mob_handle_runtime::MobBootstrapSpec::declared(
                self.definition.clone(),
                crate::mob_composition_manifest::declared_mob_store(store_file),
                service,
            )
        } else {
            // The launch opens the store itself and says that it is a
            // file: the mobs of its mob set are kept durably only beside
            // a store declared persistent.
            let (storage, provenance) =
                crate::mob_composition_manifest::persistent_mob_storage(store_file)
                    .expect("the launch opens its mob's store");
            crate::mob_handle_runtime::MobBootstrapSpec::new(
                self.definition.clone(),
                storage,
                service,
            )
            .with_mob_storage_provenance(provenance)
        };
        let spec = base
            .with_session_runtime_adapter(machine)
            .expect("the launch adopts the runtime machine")
            .with_runtime_delivery(crate::MobRuntimeDelivery::new(
                Arc::clone(&self.runtime_store),
                Arc::clone(&self.jobs),
            ))
            .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
            .expect("the agent mob tools")
            .with_child_mob_storage(crate::ChildMobStorage::Durable(
                crate::MetaStorageLayout::with_injected_roots(self.dir.path().to_path_buf(), None),
            ))
            .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
                allow_ephemeral_sessions: false,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
            });
        Box::pin(UnifiedRuntime::bootstrap_with_options(
            spec,
            crate::MetaConfig {
                modules: Vec::new(),
                discovery: crate::DiscoverySpec {
                    namespace: self.definition.id.to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            },
            Vec::new(),
            Duration::from_secs(10),
            crate::runtime::RuntimeOptions::default(),
            Arc::clone(&self.metadata) as Arc<dyn PersistentMetadataStore>,
        ))
        .await
        .expect("a launch over the state directory")
    }
}

/// Shut `host` down before a launch over the same state in this process,
/// and say whether the durable root of the mob set's mobs was released.
/// The shutdown stands that root down within its terminal window, which a
/// loaded host can outrun, and a launch over a root still claimed is
/// refused: stand down again, bounded, until the root is released.
async fn shut_down_for_a_launch_over_the_same_state(host: &UnifiedRuntime) -> bool {
    let released = |outcome: &crate::ChildMobStandDownOutcome| {
        matches!(
            outcome,
            crate::ChildMobStandDownOutcome::Released { .. }
                | crate::ChildMobStandDownOutcome::NotConfigured
        )
    };
    let mut stood_down = tokio::time::timeout(Duration::from_secs(30), host.shutdown())
        .await
        .expect("the runtime shuts down")
        .child_mob_stand_down;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !released(&stood_down) && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        stood_down = host.mob_runtime().stand_down_child_mobs(deadline).await;
    }
    released(&stood_down)
}

/// A store that holds a mob its host created on a launch that declared its
/// mob to no mob set is opened later by a launch that declares its mob.
/// The mob's creation record names the host, so the mob set builds it
/// through that launch's composition, and the launch serves it as one of
/// its hosted mobs: a request that names it resolves it there, which no
/// request did on the launch that created it.
#[tokio::test]
async fn a_mob_the_host_created_on_an_undeclared_launch_is_hosted_by_a_later_declared_launch() {
    let stored = StoredHostOfTwoLaunches::new();
    let first = stored.launch(false).await;
    let of_the_second = definition("second");
    let id = of_the_second.id.clone();
    let created = Box::pin(set_of(&first).mob_create_definition(of_the_second)).await;
    let resolved_on_the_first = first.resolve_mob(Some(&id)).await.is_ok();
    let root_released = shut_down_for_a_launch_over_the_same_state(&first).await;
    drop(first);

    let mut resolved_on_the_second = false;
    if root_released {
        let second = stored.launch(true).await;
        // A request right after a start can come before the mob set has
        // read which of its stored mobs are the host's: asked again,
        // bounded.
        for _ in 0..100 {
            if second.resolve_mob(Some(&id)).await.is_ok() {
                resolved_on_the_second = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        shut_down(&second, &[]).await;
    }
    assert!(
        created.is_ok(),
        "the host's create on the launch that declared no store: {:?}",
        created
            .as_ref()
            .map(|_| "created")
            .map_err(ToString::to_string)
    );
    assert!(
        !resolved_on_the_first,
        "no request of the launch that created it resolves the mob"
    );
    assert!(
        root_released,
        "the durable root of the mob set's mobs is released before the next launch"
    );
    assert!(
        resolved_on_the_second,
        "the launch that declares its mob serves the stored mob as one of its hosted mobs"
    );
}

/// What a host sees of a create it made through the mob set, and what the
/// create left under its id: whether the answer is a refused create, the
/// refusal's kind, reason and retryability as its data carries them, the
/// error's own sentence, and whether the mob set or its inventory hold
/// anything under the id.
async fn what_a_create_answered_and_left(
    set: &meerkat_mob_mcp::MobMcpState,
    id: &MobId,
    created: &Result<MobId, meerkat_mob::MobError>,
) -> serde_json::Value {
    let refusal = created.as_ref().err();
    let data = refusal
        .and_then(meerkat_mob::MobError::structured_data)
        .unwrap_or(serde_json::Value::Null);
    let in_the_inventory = match set.mob_inventory().await {
        Ok(inventory) => format!("{inventory:?}").contains(id.as_str()),
        Err(_) => true,
    };
    serde_json::json!({
        "a refused create": refusal.is_some_and(meerkat_mob_mcp::MobMcpState::is_create_refusal),
        "kind": data["kind"],
        "reason": data["reason"],
        "retryable": data["retryable"],
        "sentence": refusal.map(ToString::to_string),
        "a mob under the id in the set": set.handle_for(id).await.is_ok(),
        "the id in the set's inventory": in_the_inventory,
    })
}

/// What [`what_a_create_answered_and_left`] answers for a create that a
/// release owed before it refused: a refused create with the one sentence
/// of a failed release, not retryable, and nothing left under the id.
fn refused_by_a_release_and_nothing_left(id: &MobId) -> serde_json::Value {
    serde_json::json!({
        "a refused create": true,
        "kind": meerkat_mob_mcp::MOB_CREATE_REFUSED_KIND,
        "reason": crate::private_owners::CREATE_RELEASE_FAILED,
        "retryable": false,
        "sentence": format!(
            "session error: mob create refused for mob id '{id}': {}",
            crate::private_owners::CREATE_RELEASE_FAILED
        ),
        "a mob under the id in the set": false,
        "the id in the set's inventory": false,
    })
}

/// On a launch that declares its mob to no mob set, a host's create is
/// refused while the launch's announcements are declared and their owner
/// is not attached: nothing could release what an earlier mob of the id
/// left with them. The host is answered a refused create with the one
/// sentence of a failed release, not retryable, and no mob is left: not
/// in the mob set, not in its inventory, not in the runtime's table.
#[tokio::test]
async fn a_hosts_create_on_an_undeclared_launch_is_refused_while_its_announcements_are_not_attached()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_over_opened_storage_keeping(
        &temp,
        definition("host"),
        Arc::new(crate::runtime::InMemoryMetadataStore::new()),
        crate::mob_handle_runtime::MobBootstrapSpec::with_mob_announcements_declared,
    )
    .await;
    let of_the_second = definition("second");
    let id = of_the_second.id.clone();
    let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    let answered = what_a_create_answered_and_left(&set_of(&host), &id, &created).await;
    let in_the_table = host.mob_entry_in_tests(&id).is_some();
    shut_down(&host, &[]).await;
    assert_eq!(answered, refused_by_a_release_and_nothing_left(&id));
    assert!(
        !in_the_table,
        "the runtime's table holds no entry of the mob"
    );
}

/// On a launch that declares its mob to no mob set, a host's create is
/// refused when the records an earlier mob of the id left cannot be
/// released: shown with a store whose release fails (its table is gone)
/// and with a host's own store that has no release at all. The host is
/// answered a refused create with the one sentence of a failed release,
/// not retryable, with nothing of the store's own failure, and no mob is
/// left: not in the mob set, not in its inventory, not in the runtime's
/// table.
#[tokio::test]
async fn a_hosts_create_on_an_undeclared_launch_is_refused_when_the_stores_records_cannot_be_released()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("meta.sqlite");
    let failing: Arc<dyn PersistentMetadataStore> = Arc::new(
        crate::runtime::SqliteMetadataStore::open(&path).expect("a metadata store in a file"),
    );
    let without: Arc<dyn PersistentMetadataStore> =
        Arc::new(crate::console_audit::test_support::ListingStore::default());
    for (store, case) in [
        (failing, "a store whose release fails"),
        (without, "a store that has no release"),
    ] {
        let launch_temp = tempfile::tempdir().expect("tempdir");
        let host =
            launch_over_opened_storage_keeping(&launch_temp, definition("host"), store, |spec| {
                spec
            })
            .await;
        if case == "a store whose release fails" {
            // The store's table goes: every statement on it fails from here.
            rusqlite::Connection::open(&path)
                .and_then(|connection| connection.execute("DROP TABLE meta_metadata", []))
                .expect("the table is dropped");
        }
        let of_the_second = definition("second");
        let id = of_the_second.id.clone();
        let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
        let answered = what_a_create_answered_and_left(&set_of(&host), &id, &created).await;
        let in_the_table = host.mob_entry_in_tests(&id).is_some();
        shut_down(&host, &[]).await;
        assert_eq!(
            answered,
            refused_by_a_release_and_nothing_left(&id),
            "{case}"
        );
        assert!(
            !in_the_table,
            "{case}: the runtime's table holds no entry of the mob"
        );
    }
}

/// A host's create that comes before the runtime that serves the launch
/// is bound to its table of entries is refused: the table can release
/// nothing yet. A host that keeps the launch's mob set from its own slot
/// can ask it while the runtime is still being composed; the row stands in
/// that moment by preparing the launch's mob with a table no runtime was
/// bound to, over a store the launch declares to no mob set. The host is
/// answered a refused create with the one sentence of a failed release,
/// not retryable, and no mob is left: not in the mob set, not in its
/// inventory, not in the table.
#[tokio::test]
async fn a_hosts_create_on_an_undeclared_launch_is_refused_before_its_runtime_is_bound() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blobs: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(crate::blob_store::Base64BlobStoreAdapter::new(Arc::new(
            crate::blob_store::ObjectStoreBlobStore::memory(),
        )));
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(Arc::clone(&store), Arc::clone(&blobs))
            .expect("the runtime machine"),
    );
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    let mut builder = meerkat::FactoryAgentBuilder::new(
        meerkat::AgentFactory::new(temp.path())
            .builtins(false)
            .comms(true),
        meerkat::Config::default(),
    );
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        8,
        Arc::new(meerkat_store::MemoryStore::new()),
        Arc::clone(&store),
        blobs,
    ));
    let table = super::mob_entries::MobEntries::new();
    let spec = crate::mob_handle_runtime::MobBootstrapSpec::new(
        definition("host"),
        meerkat_mob::MobStorage::in_memory(),
        service,
    )
    .with_session_runtime_adapter(machine)
    .expect("the launch adopts the runtime machine")
    .with_runtime_delivery(crate::MobRuntimeDelivery::new(store, jobs))
    .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
    .expect("the agent mob tools")
    .with_child_mob_storage(crate::ChildMobStorage::ProcessBound)
    .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
        allow_ephemeral_sessions: false,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
    })
    .with_host_mob_entry_wiring(
        Arc::clone(&table) as Arc<dyn crate::mob_handle_runtime::HostMobEntryWiring>
    );
    let launch_mob = MobRuntime::bootstrap(spec)
        .await
        .expect("the launch's mob is prepared and started");
    let set = launch_mob
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    let of_the_second = definition("second");
    let id = of_the_second.id.clone();
    let created = Box::pin(set.mob_create_definition(of_the_second)).await;
    let answered = what_a_create_answered_and_left(&set, &id, &created).await;
    let in_the_table = table.entry_in_tests(&id).is_some();
    let _ = launch_mob.handle().shutdown().await;
    let _ = launch_mob
        .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(30))
        .await;
    assert_eq!(answered, refused_by_a_release_and_nothing_left(&id));
    assert!(!in_the_table, "the table holds no entry of the mob");
}

/// Two members' creates under one id: one gets the id, and what its mob
/// keeps with the host stays whole. The first create stands inside the
/// create admission's release while the second runs; whichever of the two
/// gets the id, the other's create takes none of the records the winner's
/// mob wrote.
///
/// The mob set reserves a create's id before it asks the create admission,
/// so the create that loses the id releases nothing the winner's mob wrote.
#[tokio::test]
async fn two_members_creates_under_one_id_leave_the_winners_records_whole() {
    let temp = tempfile::tempdir().expect("tempdir");
    let held = Arc::new(HeldRelease::default());
    let store: Arc<dyn PersistentMetadataStore> = Arc::clone(&held) as _;
    let (host, _sessions, _machine, ()) = launch_keeping(
        &temp,
        definition("host"),
        meerkat::Config::default(),
        Arc::clone(&store),
        |_| (),
        |spec| spec,
    )
    .await;
    let first_creator = a_member_that_creates_mobs(&host, "ann", None).await;
    let second_creator = a_member_that_creates_mobs(&host, "bob", None).await;
    let of_the_mob = definition("second");
    let id = of_the_mob.id.clone();
    let create =
        |definition: meerkat_mob::MobDefinition, creator: meerkat_core::types::SessionId| {
            let set = set_of(&host);
            tokio::spawn(async move {
                Box::pin(set.mob_create_definition_with_owner_bridge_session(
                    definition, creator, true, false,
                ))
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
            })
        };

    held.hold_the_next_release();
    let first = create(of_the_mob.clone(), first_creator);
    tokio::time::timeout(Duration::from_secs(30), held.asked_for_the_release())
        .await
        .expect("the first create stands inside the create admission's release");
    // The second create runs to its answer while the first stands; a set
    // that makes it wait for the first is given twenty seconds to say so.
    let mut second = create(of_the_mob, second_creator);
    let mut second_answer = tokio::time::timeout(Duration::from_secs(20), &mut second)
        .await
        .ok()
        .map(|joined| joined.expect("the second create ran"));
    let mut written = false;
    if matches!(second_answer, Some(Ok(()))) {
        an_opt_in_left_under(&store, &id).await;
        written = true;
    }
    held.let_go();
    let first_answer = tokio::time::timeout(Duration::from_secs(30), first)
        .await
        .expect("the first create answers once it is let go")
        .expect("the first create ran");
    let second_answer = match second_answer.take() {
        Some(answer) => answer,
        None => tokio::time::timeout(Duration::from_secs(30), second)
            .await
            .expect("the second create answers after the first")
            .expect("the second create ran"),
    };
    assert!(
        first_answer.is_ok() != second_answer.is_ok(),
        "one of the two creates gets the id: the first {first_answer:?}, the second \
         {second_answer:?}"
    );
    if !written {
        an_opt_in_left_under(&store, &id).await;
    }

    assert_eq!(
        opt_ins_under(&store, &id).await,
        1,
        "the record of the mob that got the id is whole after the other create answered"
    );
    shut_down(&host, &[&id]).await;
}

/// A release of what the announcements keep under a mob's id of the rows'
/// own, in the place of the one the announcements' owner binds when it is
/// attached: it keeps each mob it was asked for, and it fails when told
/// to.
#[derive(Default)]
struct AnnouncementsOfTheRows {
    asked: Mutex<Vec<String>>,
    fails: AtomicBool,
}

impl AnnouncementsOfTheRows {
    fn asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl crate::mob_announcements_release::MobAnnouncementsRelease for AnnouncementsOfTheRows {
    async fn release_mob(&self, mob_id: &MobId) -> Result<(), String> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(mob_id.to_string());
        if self.fails.load(Ordering::SeqCst) {
            Err("the announcement store of the rows is closed".to_string())
        } else {
            Ok(())
        }
    }
}

/// A launch that declared announcements before its runtime was built.
async fn launch_with_announcements_declared(temp: &tempfile::TempDir) -> UnifiedRuntime {
    let (host, _sessions, _machine, ()) = launch_composed(
        temp,
        definition("host"),
        meerkat::Config::default(),
        |_| (),
        crate::mob_handle_runtime::MobBootstrapSpec::with_mob_announcements_declared,
    )
    .await;
    host
}

/// A create releases what the launch's announcements keep under the mob's
/// id: a host's create and a member's create each ask for it, by the id of
/// the mob they create.
#[tokio::test]
async fn a_create_releases_what_the_announcements_keep_under_its_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_with_announcements_declared(&temp).await;
    let announcements = Arc::new(AnnouncementsOfTheRows::default());
    host.mob_runtime()
        .mob_announcements_release()
        .bind(Arc::clone(&announcements) as _);
    let of_the_second = definition("second");
    let again = of_the_second.clone();

    let id = second_mob(&host, of_the_second).await;
    destroyed(&host, &id).await;
    let creator = a_member_that_creates_mobs(&host, "bob", None).await;
    let created = created_by_a_member(&host, again, &creator).await;
    assert!(
        matches!(&created, Ok(created) if *created == id),
        "a member creates a mob under the same id: {:?}",
        created.as_ref().map_err(ToString::to_string)
    );

    assert_eq!(
        announcements.asked(),
        vec![id.to_string(), id.to_string()],
        "the host's create and the member's create each asked for the release of what the \
         announcements keep under the id"
    );
    shut_down(&host, &[&id]).await;
}

/// A create is refused when what the announcements keep under its id
/// cannot be released: the new mob would find what an earlier mob of the
/// id left there. The refusal carries nothing of the release's own
/// failure, and no mob is left under the id.
#[tokio::test]
async fn a_create_is_refused_when_what_the_announcements_keep_under_its_id_cannot_be_released() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_with_announcements_declared(&temp).await;
    let announcements = Arc::new(AnnouncementsOfTheRows::default());
    host.mob_runtime()
        .mob_announcements_release()
        .bind(Arc::clone(&announcements) as _);
    let of_the_second = definition("second");
    let id = of_the_second.id.clone();

    announcements.fails.store(true, Ordering::SeqCst);
    let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    let answer = created
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        created.is_err()
            && answer.contains(crate::private_owners::CREATE_RELEASE_FAILED)
            && !answer.contains("of the rows"),
        "the create is refused with the one sentence of a failed release, with nothing of \
         the release's own failure: {:?}",
        created.as_ref().map(|_| "created")
    );
    assert!(
        set_of(&host).handle_for(&id).await.is_err(),
        "and the set holds no mob under the id"
    );
    shut_down(&host, &[]).await;
}

/// A launch that declared announcements whose owner is not attached yet
/// refuses the create of a mob, a member's create as a host's: nothing
/// could release what an earlier mob of the id left with the
/// announcements. Once the release is bound the create goes through.
#[tokio::test]
async fn a_create_is_refused_while_the_launchs_announcements_are_declared_and_not_bound() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_with_announcements_declared(&temp).await;
    let creator = a_member_that_creates_mobs(&host, "bob", None).await;
    let of_the_second = definition("second");
    let (for_the_member, again) = (of_the_second.clone(), of_the_second.clone());
    let id = of_the_second.id.clone();

    let by_the_host = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    assert!(
        matches!(
            &by_the_host,
            Err(error) if meerkat_mob_mcp::MobMcpState::is_create_refusal(error)
                && error.to_string().contains(crate::private_owners::CREATE_RELEASE_FAILED)
        ),
        "the host's create is refused until the launch's announcements are bound: {:?}",
        by_the_host.as_ref().map(|_| "created")
    );
    let by_a_member = created_by_a_member(&host, for_the_member, &creator).await;
    assert!(
        matches!(
            &by_a_member,
            Err(error) if meerkat_mob_mcp::MobMcpState::is_create_refusal(error)
                && error.to_string().contains(crate::private_owners::CREATE_RELEASE_FAILED)
        ),
        "a member's create is refused too, as a refused create: {:?}",
        by_a_member
            .as_ref()
            .map(|_| "created")
            .map_err(ToString::to_string)
    );
    assert!(
        set_of(&host).handle_for(&id).await.is_err(),
        "and the set holds no mob under the id"
    );

    host.mob_runtime()
        .mob_announcements_release()
        .bind(Arc::new(AnnouncementsOfTheRows::default()) as _);
    assert_eq!(
        second_mob(&host, again).await,
        id,
        "the create goes through once the release is bound"
    );
    shut_down(&host, &[&id]).await;
}

/// A launch that declared no announcements is asked for nothing of them:
/// the release's slot stands undeclared before and after a create, and
/// the create of a mob goes through.
#[tokio::test]
async fn a_launch_that_declared_no_announcements_creates_a_mob_and_owes_no_release_of_them() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let undeclared = |host: &UnifiedRuntime| {
        matches!(
            host.mob_runtime().mob_announcements_release().standing(),
            crate::mob_announcements_release::MobAnnouncementsStanding::NotDeclared
        )
    };
    assert!(undeclared(&host), "the launch declared no announcements");

    let of_the_second = definition("second");
    let id = of_the_second.id.clone();
    assert_eq!(
        second_mob(&host, of_the_second).await,
        id,
        "the create of a mob goes through"
    );
    assert!(undeclared(&host), "and the create declared nothing");
    shut_down(&host, &[&id]).await;
}

/// Whether a schedule whose identity target names the member `side` and no
/// mob is hidden from a turn of a member that is private to nobody, on a
/// host of two mobs that each hold a member `side`: the launch mob's is
/// private to `in_the_launch_mob`, the second mob's to `in_the_other_mob`
/// (nobody for `None`). The second mob's id sorts before the launch mob's,
/// so a walk of the host's mobs meets it first. The schedule's targets are
/// read as a gateway's are: over a registry that adopted the launch's
/// runtime.
async fn a_schedule_that_names_no_mob_is_hidden_from_a_shared_agent(
    in_the_launch_mob: Option<&str>,
    in_the_other_mob: Option<&str>,
) -> bool {
    use crate::console_schedules::AgentScheduleAudience as _;

    let private_to = |owner: Option<&str>| {
        let spec = SpawnMemberSpec::new("worker", "side");
        match owner {
            Some(owner) => spec.with_labels(BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                owner.to_string(),
            )])),
            None => spec,
        }
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let other = second_mob(&host, definition("an-earlier-id")).await;
    assert!(
        other < *host.mob_handle().mob_id(),
        "the second mob's id sorts before the launch mob's"
    );
    let seated = Box::pin(host.mob_handle().spawn_spec(private_to(in_the_launch_mob))).await;
    assert!(
        seated.is_ok(),
        "the launch mob's member: {:?}",
        seated.as_ref().err()
    );
    let seated = Box::pin(
        set_of(&host)
            .handle_for(&other)
            .await
            .expect("the set serves the second mob")
            .spawn_spec(private_to(in_the_other_mob)),
    )
    .await;
    assert!(
        seated.is_ok(),
        "the second mob's member of the same name: {:?}",
        seated.as_ref().err()
    );
    let reader = a_member_that_creates_mobs(&host, "reader", None).await;

    let registry = crate::schedule_wiring::ScheduleMobTargetRegistry::default();
    registry.adopt_mob_runtime(
        host.mob_runtime(),
        &crate::member_key::DefaultMob::new(host.mob_handle().mob_id().clone()),
    );
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new()));
    let schedule = service
        .create(meerkat::CreateScheduleRequest {
            name: Some("names no mob".to_owned()),
            description: None,
            trigger: meerkat::TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
            },
            target: meerkat::TargetBinding::identity(meerkat::IdentityTargetBinding::resumable(
                "side",
                meerkat::ScheduledSessionAction::Prompt {
                    prompt: meerkat_core::ContentInput::Text("Check in.".into()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            )),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        })
        .await
        .expect("a schedule that names no mob");
    let hidden = registry
        .hides(
            &schedule,
            &meerkat_core::ToolDispatchContext::default().with_runtime_identity(reader, None),
        )
        .await;
    shut_down(&host, &[&other]).await;
    hidden
}

/// A schedule whose identity target names no mob is delivered to the
/// default mob's member, so who it is hidden from follows that member
/// alone. The default mob's member is private to a person: the schedule is
/// hidden from an agent that is private to nobody, also when another mob
/// holds a member of the same name that is private to nobody.
#[tokio::test]
async fn a_schedule_that_names_no_mob_is_hidden_as_the_default_mobs_private_member_is() {
    assert!(
        a_schedule_that_names_no_mob_is_hidden_from_a_shared_agent(Some(SCHEDULES_OWNER), None)
            .await,
        "the default mob's private member's schedule is hidden from a shared agent"
    );
}

/// The default mob's member is private to nobody: its schedule that names
/// no mob is hidden from no agent, also when another mob holds a member of
/// the same name that is private to a person.
#[tokio::test]
async fn a_schedule_that_names_no_mob_is_not_hidden_for_another_mobs_private_member() {
    assert!(
        !a_schedule_that_names_no_mob_is_hidden_from_a_shared_agent(None, Some(SCHEDULES_OWNER))
            .await,
        "the default mob's shared member's schedule is hidden from no agent"
    );
}

/// A create that is refused because the schedules under its id cannot be
/// released leaves the labels the host keeps in its memory under the id:
/// they are forgotten only once every release that can refuse is done, so
/// a refused create has dropped nothing a later create could not release.
#[tokio::test]
async fn a_create_refused_for_its_schedules_leaves_the_labels_kept_under_its_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch_with_schedules_declared(&temp).await;
    let schedules = SchedulesOfTheRows::of(&host);
    host.mob_runtime()
        .mob_schedules()
        .bind(Arc::clone(&schedules) as _);
    let of_the_second = definition("second");
    let again = of_the_second.clone();
    let id = second_mob(&host, of_the_second).await;
    assert_eq!(
        crate::runtime::dispatch_label_method(
            host.metadata_table(),
            id.as_str(),
            "meta/mob_labels/set",
            &serde_json::json!({"labels": {"owner": "of-the-destroyed"}}),
        )
        .await,
        Some(crate::runtime::LabelRpcResult::Accepted),
        "a label of the second mob"
    );
    destroyed(&host, &id).await;

    schedules.fails.store(true, Ordering::SeqCst);
    let created = Box::pin(set_of(&host).mob_create_definition(again)).await;
    assert!(
        created.is_err(),
        "the create is refused: {:?}",
        created.as_ref().map(|_| "created")
    );
    assert_eq!(
        crate::runtime::dispatch_label_method(
            host.metadata_table(),
            id.as_str(),
            "meta/mob_labels/get",
            &serde_json::json!({}),
        )
        .await,
        Some(crate::runtime::LabelRpcResult::Labels(BTreeMap::from([(
            "owner".to_string(),
            "of-the-destroyed".to_string(),
        )]))),
        "the labels kept under the id are as they were"
    );
    shut_down(&host, &[]).await;
}

/// A launch hands its runtime how the identity planes of its other mobs
/// are composed, with the values it gave its launch mob. From then on an
/// identity agent of a second mob of the host is built: the launch's
/// customizer is asked for that agent, and the tool it gave the build
/// reaches the agent's session through that mob's own member wiring.
#[tokio::test]
async fn an_identity_agent_of_a_second_mob_is_built_in_a_launch_that_hands_its_composer_over() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_launch_mob) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let customizer = Arc::new(ToolCustomizer::default());
    HostMobIdentities::new(Arc::clone(&roster) as Arc<dyn RosterProvider>, true)
        .with_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .hand_over_to(&host);
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["back"]);
    let second_id = second_mob(&host, of_the_second).await;
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "the second mob has an identity runtime"
    );

    let session = match second_runtime {
        Some(second_runtime) => Box::pin(second_runtime.materialize(&identity("back")))
            .await
            .ok()
            .flatten()
            .map(|scope| scope.session_id().clone()),
        None => None,
    };
    assert!(
        session.is_some(),
        "the second mob's agent wakes in a session"
    );
    assert!(
        customizer.built().iter().any(|name| name == "back"),
        "the launch's customizer was asked to build the second mob's agent: {:?}",
        customizer.built()
    );
    if let Some(session) = session {
        let offered = crate::mob_handle_runtime::resolved_tools_for_session(
            host.mob_runtime().session_service(),
            "back",
            session,
        )
        .await
        .map(|snapshot| snapshot.tools);
        assert!(
            offered
                .as_ref()
                .is_ok_and(|tools| tools.iter().any(|tool| tool == LAUNCH_TOOL)),
            "the agent's session offers the tool the customizer gave its build: {:?}",
            offered.as_ref().map_err(ToString::to_string)
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// A turn of an identity agent the launch's composer built in a second mob
/// shows under that mob, and not under the launch mob's agent of the same
/// name. The launch mob has an identity agent `desk`; the launch hands its
/// composer over, and the second mob's roster names a `desk` too, which
/// that mob's own identity runtime wakes. Each `desk` runs a turn. A run's
/// frames name the session of the agent that ran.
///
/// (a) The console's administrator reads `desk`'s timeline with no mob, on
///     `GET /console/timeline`: the launch mob's agent's own run, no frame
///     that names the second mob's agent's session, and nothing of that
///     agent's task.
/// (b) The administrator's read with the second mob's id holds the run:
///     `meta/console/query_timeline` on the Console's request door names
///     the second mob and its `desk`.
/// (c) The launch mob's console event store holds no event of the run.
///
/// The fixture is a runtime launched in this process; no gateway process
/// is started.
#[tokio::test]
async fn a_turn_of_a_second_mobs_composed_agent_shows_under_its_mob_and_not_the_launch_mobs() {
    use super::mob_console_log_tests::{
        FEED_ROW_ADMIN, FEED_ROW_SIGNING_SECRET, drained_until_a_frame_names, run_a_turn_of_desk,
        timeline_read_as, timeline_request_as,
    };
    use crate::access::{AccessControlConfig, AccessController};
    /// The task of the second mob's agent: a text no other agent is given.
    const TASK: &str = "a-task-only-the-composed-desk-of-the-second-mob-was-given";

    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, of_the_default) = launch_with_identities(&temp).await;
    host.initialize_console_projection_with_policy(Arc::new(
        crate::console_aggregator::HideImplicitDelegateMembersConsoleVisibilityPolicy,
    ))
    .expect("the runtime's console projection");
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![FEED_ROW_ADMIN.to_string()],
        ..AccessControlConfig::default()
    })
    .expect("access configuration");
    host.set_access_controller(controller);

    let roster = Arc::new(LaunchRoster::default());
    let customizer = Arc::new(ToolCustomizer::default());
    HostMobIdentities::new(Arc::clone(&roster) as Arc<dyn RosterProvider>, true)
        .with_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .hand_over_to(&host);
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk"]);
    let second_id = second_mob(&host, of_the_second).await;
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned())
        .expect("the second mob has an identity runtime");

    let desk = identity("desk");
    let woke_in_the_launch_mob = Box::pin(of_the_default.runtime.materialize(&desk))
        .await
        .ok()
        .flatten()
        .is_some();
    let woke_in_the_second_mob = Box::pin(second_runtime.materialize(&desk))
        .await
        .ok()
        .flatten()
        .is_some();
    assert!(
        woke_in_the_launch_mob && woke_in_the_second_mob,
        "each mob's agent `desk` wakes in a session: {woke_in_the_launch_mob}, \
         {woke_in_the_second_mob}"
    );
    assert!(
        customizer.built().iter().any(|name| name == "desk"),
        "the launch's customizer was asked to build the second mob's agent: {:?}",
        customizer.built()
    );

    let decisions = crate::console_auth_config::parse_console_auth_config(&serde_json::json!({
        "shared_secret": FEED_ROW_SIGNING_SECRET,
        "email_allowlist": [FEED_ROW_ADMIN],
    }))
    .expect("console sign-in configuration");
    let app = host.build_console_json_router(decisions);

    let member = meerkat_mob::AgentIdentity::from("desk");
    let launch = host.mob_runtime().handle();
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the mob set serves the second mob");
    let launch_session = launch
        .resolve_bridge_session_id(&member)
        .await
        .expect("the launch mob's agent's session")
        .to_string();
    let second_session = second
        .resolve_bridge_session_id(&member)
        .await
        .expect("the second mob's agent's session")
        .to_string();
    assert_ne!(launch_session, second_session, "two agents, two sessions");

    let mut wrong = Vec::new();
    run_a_turn_of_desk(&launch, "a-task-of-the-launch-mobs-desk").await;
    if !drained_until_a_frame_names(&host, None, &launch_session).await {
        wrong.push(
            "the launch mob's agent's own run is not stored under the launch mob".to_string(),
        );
    }
    // The run the row is about.
    run_a_turn_of_desk(&second, TASK).await;
    let stored_under_the_second =
        drained_until_a_frame_names(&host, Some(&second_id), &second_session).await;
    if !stored_under_the_second {
        wrong.push(
            "the composed agent's run is not stored under the second mob in the console log"
                .to_string(),
        );
    }

    match timeline_read_as(&app, "desk", FEED_ROW_ADMIN).await {
        Ok((frames, body)) => {
            let names = |session: &str| {
                frames
                    .iter()
                    .any(|frame| frame["session_id"].as_str() == Some(session))
            };
            if !names(&launch_session) {
                wrong.push(format!(
                    "(a) the administrator does not read the launch mob's agent's own run in \
                     `desk`'s timeline with no mob: {} frames",
                    frames.len()
                ));
            }
            if names(&second_session) || body.contains(TASK) || body.contains(&second_session) {
                wrong.push(
                    "(a) the administrator reads the second mob's agent's run in `desk`'s \
                     timeline with no mob"
                        .to_string(),
                );
            }
        }
        Err(answer) => wrong.push(format!(
            "(a) the read of `desk`'s timeline with no mob is not answered: {answer}"
        )),
    }

    match timeline_request_as(&app, "desk", &second_id, FEED_ROW_ADMIN).await {
        Ok((frames, _body)) => {
            if !frames
                .iter()
                .any(|frame| frame["session_id"].as_str() == Some(second_session.as_str()))
            {
                wrong.push(format!(
                    "(b) the administrator's read with the second mob's id, on the request \
                     door, does not hold the run of its agent (stored under the second mob: \
                     {stored_under_the_second}): {:?}",
                    frames
                        .iter()
                        .filter_map(|frame| frame["kind"].as_str())
                        .collect::<Vec<_>>()
                ));
            }
        }
        Err(answer) => wrong.push(format!(
            "(b) the administrator's read with the second mob's id, on the request door, is \
             not answered: {answer}"
        )),
    }

    let of_the_run: Vec<String> = host
        .console_events()
        .replay_all(None)
        .await
        .expect("the launch mob's console events")
        .into_iter()
        .filter(|event| {
            let data = event.data.to_string();
            data.contains(&second_session) || data.contains(TASK)
        })
        .map(|event| format!("{} {}", event.identity, event.event_type))
        .collect();
    if !of_the_run.is_empty() {
        wrong.push(format!(
            "(c) the launch mob's console event store holds events of the second mob's agent's \
             run: {of_the_run:?}"
        ));
    }

    drop(app);
    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "a turn of a second mob's composed agent does not show under its mob alone:\n{}",
        wrong.join("\n")
    );
}

/// A launch whose roster is its launch mob's own (it answers the same
/// agents whichever mob it is asked for) says so when it hands its
/// composer over: a second mob of the host gets its identity plane, and
/// none of the launch mob's agents is seated in it.
#[tokio::test]
async fn a_launch_whose_roster_is_its_launch_mobs_seats_none_of_it_in_a_second_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_launch_mob) = launch_with_identities(&temp).await;
    let of_the_launch_mob = Arc::new(crate::identity_first::MutableRosterProvider::new(vec![
        configured_agent("desk"),
    ]));
    HostMobIdentities::new(of_the_launch_mob as Arc<dyn RosterProvider>, true)
        .with_roster_of_the_launch_mob_alone(host.mob_handle().mob_id().clone())
        .hand_over_to(&host);
    let second_id = second_mob(&host, definition("second")).await;
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned());
    assert!(
        second_runtime.is_some(),
        "the second mob has an identity runtime"
    );
    assert!(
        !members_of(&host, &second_id).await.contains("desk"),
        "no agent of the launch mob's roster is seated in the second mob: {:?}",
        members_of(&host, &second_id).await
    );
    shut_down(&host, &[&second_id]).await;
}

/// Each launch hands its composer over itself: the two gateways and the
/// runtime builder each build what their launch gave its launch mob and
/// hand it to their runtime, once, after their launch mob's identity
/// bootstrap. A launch that left it out would give the mobs it hosts no
/// identity plane, and no row of a fixture launch would show it.
///
/// What each launch must say when it hands over is held here as well. A
/// launch that can keep agent memory says whether that memory is its
/// launch mob's alone (it then hands nothing over) and hands over the
/// memory it keeps per mob; a launch that can be a candidate launch says
/// whether it is; and every launch says that its roster is its launch
/// mob's own, so no agent of that roster is seated in another mob. The
/// console gateway keeps no agent memory and is no candidate launch: its
/// file names neither. And no launch hands a topology provider over: like
/// the roster it is the launch mob's alone.
#[test]
fn each_launch_hands_over_how_its_other_mobs_identity_planes_are_composed() {
    const BUILDS: &str = "HostMobIdentities::new(";
    const HANDS_OVER: &str = ".hand_over_to(";
    const AGENT_MEMORY: &str = ".with_agent_memory_of_the_launch_mob(";
    const AGENT_MEMORY_PER_MOB: &str = ".with_agent_memory_per_mob(";
    const CANDIDATE: &str = ".with_candidate_launch(";
    const ROSTER_OF_THE_LAUNCH_MOB: &str = ".with_roster_of_the_launch_mob_alone(";
    const TOPOLOGY: &str = ".with_topology(";
    let mut missed = Vec::new();
    for (launch, source, bootstrap, says, says_nothing_of) in [
        (
            "the stdio gateway",
            include_str!("../rpc_gateway.rs"),
            "bootstrap_installed_identity_first_context(&roster_specs)",
            &[
                AGENT_MEMORY,
                AGENT_MEMORY_PER_MOB,
                CANDIDATE,
                ROSTER_OF_THE_LAUNCH_MOB,
            ][..],
            &[][..],
        ),
        (
            "the console gateway",
            include_str!("../bin/meta_gateway.rs"),
            ".install_and_bootstrap_identity_first_context(",
            &[ROSTER_OF_THE_LAUNCH_MOB][..],
            &["agent_memory", "mark_candidate_launch"][..],
        ),
        (
            "the runtime builder",
            include_str!("builder.rs"),
            ".install_and_bootstrap_identity_first_context(",
            &[
                AGENT_MEMORY,
                AGENT_MEMORY_PER_MOB,
                CANDIDATE,
                ROSTER_OF_THE_LAUNCH_MOB,
            ][..],
            &[][..],
        ),
    ] {
        for said in [BUILDS, HANDS_OVER].iter().chain(says) {
            let times = source.matches(said).count();
            if times != 1 {
                missed.push(format!("{launch} has `{said}` {times} times, not once"));
            }
        }
        if !matches!(
            (source.find(bootstrap), source.find(HANDS_OVER)),
            (Some(bootstrapped), Some(handed_over)) if bootstrapped < handed_over
        ) {
            missed.push(format!(
                "{launch} does not hand over after its launch mob's identity bootstrap"
            ));
        }
        for unsaid in says_nothing_of {
            if source.contains(unsaid) {
                missed.push(format!(
                    "{launch} names `{unsaid}`: its hand-over must then say so"
                ));
            }
        }
        if source.contains(TOPOLOGY) {
            missed.push(format!(
                "{launch} hands a topology provider over: it is the launch mob's alone"
            ));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}

/// The topology provider, like the roster, is the launch mob's alone. A
/// mob hosted beside the launch mob is composed as the launches hand over,
/// with no topology provider: its identity context holds none, so its
/// identity bootstrap asks no provider for edges.
#[tokio::test]
async fn a_mob_hosted_beside_the_launch_mob_is_composed_with_no_topology_provider() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_launch_mob) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    HostMobIdentities::new(Arc::clone(&roster) as Arc<dyn RosterProvider>, true)
        .hand_over_to(&host);
    let second_id = second_mob(&host, definition("second")).await;
    let context = host
        .resolve_mob(Some(&second_id))
        .await
        .ok()
        .and_then(|mob| mob.identity_first_context());
    assert!(context.is_some(), "the second mob has an identity context");
    assert!(
        context.is_some_and(|context| context.topology_provider.is_none()),
        "and it holds no topology provider"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A launch whose agent memory is its launch mob's alone (a host's own
/// memory provider, or a native host that composes the launch's customizer
/// itself) cannot keep it per mob, and its customizer carries it. Such a
/// launch hands nothing over: a second mob of the host resolves without an
/// identity runtime, the roster provider is not asked for it, and the
/// launch's customizer is asked to build no agent of it. (A launch that
/// keeps its agent memory in the bundled store hands it over per mob: the
/// rows of `mob_memory_tests`.)
#[tokio::test]
async fn a_launch_whose_agent_memory_is_its_launch_mobs_alone_composes_no_identity_plane_for_a_second_mob()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _of_the_launch_mob) = launch_with_identities(&temp).await;
    let roster = Arc::new(LaunchRoster::default());
    let customizer = Arc::new(ToolCustomizer::default());
    HostMobIdentities::new(Arc::clone(&roster) as Arc<dyn RosterProvider>, true)
        .with_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .with_agent_memory_of_the_launch_mob(true)
        .hand_over_to(&host);
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["back"]);
    let second_id = second_mob(&host, of_the_second).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok_and(|mob| mob.identity_runtime_ref().is_none()),
        "the second mob resolves without an identity runtime"
    );
    assert!(
        roster
            .asked_for()
            .iter()
            .all(|asked| asked.as_ref() != Some(&second_id)),
        "the roster provider was not asked for the second mob: {:?}",
        roster.asked_for()
    );
    assert!(
        customizer.built().is_empty(),
        "the launch's customizer was asked to build no agent of the second mob: {:?}",
        customizer.built()
    );
    shut_down(&host, &[&second_id]).await;
}

/// What a wake of the member `name` of the mob `mob_id` is answered, asked
/// of the host's mob set as a delivery asks it: "live", or the typed
/// refusal of the wake.
async fn wake_of(host: &UnifiedRuntime, mob_id: &MobId, name: &str) -> String {
    let of_the_set = set_of(host)
        .handle_for(mob_id)
        .await
        .expect("the set serves the mob");
    let woken = tokio::time::timeout(
        Duration::from_secs(20),
        of_the_set.ensure_member_live(&crate::member_comms_id::mob_member_id(name)),
    )
    .await;
    match woken {
        Ok(Ok(())) => "live".to_string(),
        Ok(Err(meerkat_mob::MobError::MemberWakeRefused { refusal, .. })) => {
            format!("refused: {refusal:?}")
        }
        Ok(Err(other)) => format!("failed: {other}"),
        Err(_) => "not answered in twenty seconds".to_string(),
    }
}

/// A launch over `stored`'s state whose default mob has its identity
/// context: a restart of the host when `stored` was launched before.
async fn launch_of_stored_with_identities(stored: &super::StoredHost) -> UnifiedRuntime {
    let mut host = stored.launch().await;
    let of_the_default = identity_context(host.mob_runtime(), "default");
    Box::pin(
        host.install_and_bootstrap_identity_first_context(
            of_the_default,
            &[configured_agent("desk")],
        ),
    )
    .await
    .expect("the default mob's identity bootstrap");
    host
}

/// A stored host with a second mob whose agent `desk` was seated by a
/// first life's composition, stopped: the host, the roster and the mob.
async fn a_stored_second_mob_with_its_agent_seated() -> (super::StoredHost, Arc<LaunchRoster>, MobId)
{
    let stored = super::StoredHost::new();
    let roster = Arc::new(LaunchRoster::default());
    let host = launch_of_stored_with_identities(&stored).await;
    host.compose_host_mob_identities(
        LaunchComposer::over(&roster) as Arc<dyn HostMobIdentityComposer>
    );
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk"]);
    let second_id = second_mob(&host, of_the_second).await;
    // A request that names the mob waits for its composition, roster pass
    // included: the agent is seated when it is answered.
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved
            .as_ref()
            .is_ok_and(|mob| mob.identity_runtime_ref().is_some()),
        "the first life composes the second mob"
    );
    drop(resolved);
    let seated = members_of(&host, &second_id).await;
    assert!(
        seated.contains("desk"),
        "and its roster's agent is seated in it: {seated:?}"
    );
    shut_down(&host, &[]).await;
    drop(host);
    (stored, roster, second_id)
}

/// The typed reason no agent of the mob `mob_id` is built for, waited for
/// until it is `wanted`, for at most twenty seconds.
async fn agents_not_built_comes_to(
    host: &UnifiedRuntime,
    mob_id: &MobId,
    wanted: Option<crate::memory::sqlite_store::HostedAgentsNotBuilt>,
) -> Option<crate::memory::sqlite_store::HostedAgentsNotBuilt> {
    let read = || {
        host.mob_entry_in_tests(mob_id)
            .and_then(|entry| entry.agents_not_built_in_tests())
    };
    for _ in 0..200 {
        if read() == wanted {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    read()
}

/// A launch that builds an agent only of a composed mob builds none of a
/// mob's agents from the mob's build until the mob's first composition
/// succeeds, and that composition builds them again.
///
/// A stored mob's agent was seated by an earlier life. The host starts as
/// a launch that builds an agent only of a composed mob, and its composer
/// is held inside its first ask. The request that names the mob builds
/// the mob and waits for that composition. BEFORE THE COMPOSITION ANSWERS
/// no agent of the mob is built, for the reason that its memory is not
/// composed yet, and a wake of its stored agent is refused as a build its
/// host asked to retry: what a member the mob materializes at its start
/// is answered. Then the composer is let go and answers: the request
/// finds the mob composed, nothing is withheld any more, the composer was
/// asked once, and the FIRST wake of the agent after it is answered.
#[tokio::test]
async fn a_launch_that_builds_agents_only_of_composed_mobs_builds_none_before_a_mobs_first_composition_answers()
 {
    let (stored, roster, second_id) = a_stored_second_mob_with_its_agent_seated().await;
    let host = launch_of_stored_with_identities(&stored).await;
    let composer = LaunchComposer::over(&roster);
    composer.only_of_composed_mobs.store(true, Ordering::SeqCst);
    composer.holds.store(true, Ordering::SeqCst);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);

    let asked = async {
        host.resolve_mob(Some(&second_id))
            .await
            .is_ok_and(|mob| mob.identity_runtime_ref().is_some())
    };
    let before_it_answers = async {
        for _ in 0..200 {
            if composer.composed_for().contains(&second_id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(
            composer.composed_for(),
            vec![second_id.clone()],
            "the mob's first composition is asked, and held"
        );
        let not_built = host
            .mob_entry_in_tests(&second_id)
            .and_then(|entry| entry.agents_not_built_in_tests());
        let woken = wake_of(&host, &second_id, "desk").await;
        composer.lets_go();
        (not_built, woken)
    };
    let (composed, (not_built, woken)) = tokio::join!(asked, before_it_answers);
    assert_eq!(
        not_built,
        Some(crate::memory::sqlite_store::HostedAgentsNotBuilt::MemoryNotComposedYet),
        "before its first composition answers none of the mob's agents is built: its memory \
         is not composed yet"
    );
    assert_eq!(
        woken, "refused: BuildPreparationRetry",
        "a wake of the mob's stored agent before the composition answers is refused, to be \
         retried"
    );
    assert!(composed, "the request finds the mob composed");
    assert_eq!(
        agents_not_built_comes_to(&host, &second_id, None).await,
        None,
        "nothing is withheld from the composed mob"
    );
    assert_eq!(
        composer.composed_for(),
        vec![second_id.clone()],
        "the composer was asked once"
    );
    assert_eq!(
        wake_of(&host, &second_id, "desk").await,
        "live",
        "the first wake of the agent after the composition is answered"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A mob built before the launch said how it composes is withheld from the
/// answer of its first composition that does not succeed, and a
/// composition that succeeds builds its agents again.
///
/// A stored mob's agent was seated by an earlier life. The host starts and
/// a request builds the mob before the launch says how it composes:
/// nothing is withheld. The launch then says it builds an agent only of a
/// composed mob, with a composer whose every ask fails: once that
/// composition has failed no agent of the mob is built, for the reason
/// that its memory is not composed yet, and a wake of its stored agent is
/// refused. Then the composer answers: the next request composes the mob,
/// nothing is withheld, and the first wake of the agent is answered.
#[tokio::test]
async fn a_mob_built_before_the_launch_said_how_it_composes_is_withheld_from_its_first_failed_composition()
 {
    let (stored, roster, second_id) = a_stored_second_mob_with_its_agent_seated().await;
    let host = launch_of_stored_with_identities(&stored).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved
            .as_ref()
            .is_ok_and(|mob| mob.identity_runtime_ref().is_none()),
        "the mob is built and served before the launch says how it composes"
    );
    drop(resolved);
    assert_eq!(
        host.mob_entry_in_tests(&second_id)
            .and_then(|entry| entry.agents_not_built_in_tests()),
        None,
        "and nothing is withheld from it"
    );

    let composer = LaunchComposer::over(&roster);
    composer.refuses.store(true, Ordering::SeqCst);
    composer.only_of_composed_mobs.store(true, Ordering::SeqCst);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);
    assert_eq!(
        agents_not_built_comes_to(
            &host,
            &second_id,
            Some(crate::memory::sqlite_store::HostedAgentsNotBuilt::MemoryNotComposedYet),
        )
        .await,
        Some(crate::memory::sqlite_store::HostedAgentsNotBuilt::MemoryNotComposedYet),
        "once its first composition has failed none of the mob's agents is built: its \
         memory is not composed yet"
    );
    assert_eq!(
        wake_of(&host, &second_id, "desk").await,
        "refused: BuildPreparationRetry",
        "a wake of the mob's stored agent is refused, to be retried"
    );

    composer.refuses.store(false, Ordering::SeqCst);
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved
            .as_ref()
            .is_ok_and(|mob| mob.identity_runtime_ref().is_some()),
        "the next request composes the mob"
    );
    drop(resolved);
    assert_eq!(
        host.mob_entry_in_tests(&second_id)
            .and_then(|entry| entry.agents_not_built_in_tests()),
        None,
        "nothing is withheld from the composed mob"
    );
    assert_eq!(
        wake_of(&host, &second_id, "desk").await,
        "live",
        "the first wake of the agent after the composition is answered"
    );
    shut_down(&host, &[&second_id]).await;
}

/// Every other launch builds a mob's members as the mob's own runtime
/// lowers them, composed or not: nothing is withheld from a mob of a
/// launch that does not say it builds an agent only of a composed mob.
///
/// The same stored mob, and a launch whose composer fails at every ask and
/// says nothing of when agents are built (the default): nothing is
/// withheld from the mob, and a wake of its stored agent makes it live.
#[tokio::test]
async fn a_launch_that_does_not_say_so_builds_a_mobs_stored_agent_before_its_composition_succeeds()
{
    let (stored, roster, second_id) = a_stored_second_mob_with_its_agent_seated().await;
    let host = launch_of_stored_with_identities(&stored).await;
    let composer = LaunchComposer::over(&roster);
    composer.refuses.store(true, Ordering::SeqCst);
    host.compose_host_mob_identities(Arc::clone(&composer) as Arc<dyn HostMobIdentityComposer>);

    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved
            .as_ref()
            .is_ok_and(|mob| mob.identity_runtime_ref().is_none()),
        "the mob is served, without an identity plane: its composition failed"
    );
    drop(resolved);
    assert_eq!(
        host.mob_entry_in_tests(&second_id)
            .and_then(|entry| entry.agents_not_built_in_tests()),
        None,
        "nothing is withheld from the mob"
    );
    assert_eq!(
        wake_of(&host, &second_id, "desk").await,
        "live",
        "a wake of the mob's stored agent makes it live, as before"
    );
    shut_down(&host, &[&second_id]).await;
}
