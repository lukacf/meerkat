//! Who the members of a runtime's mob are private to.
//!
//! A member is private to a person when its roster labels carry a non-blank
//! private owner ([`crate::access::PRIVATE_TO_LABEL`]): a console side chat
//! a signed-in person opened, a member such a member's own turn created, and
//! a helper forked from one. Only Meta writes the label on a roster: every
//! caller label path refuses it
//! ([`crate::member_comms_id::RESERVED_AUTHORITY_LABELS`]), the side chat
//! fork writes its creator's, and [`PrivateOwners::spawn_customizer`] gives a
//! member the owner of the member that asked for it.
//!
//! [`PrivateOwners`] knows the owner of every private member of its mob:
//! each member build records its own labels (the base session layer's
//! pre-build hook, [`PrivateOwners::after_hook`]), and the mob's durable
//! event log names every member it spawned with its labels, retired members
//! included, which [`PrivateOwners::seeded`] reads once. The spawn customizer
//! reads it (a spawner was built before its turn spawns), and so do the
//! Steward, which dreams over no private member's memory, and the console's
//! memory panel, which shows a private member's memory to its owner and the
//! access admins only, after the member retired too. A member whose spawn
//! the event log no longer holds (pruned) and that this process never built
//! is not known.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use meerkat_core::service::CreateSessionRequest;
use meerkat_mob::launch::MemberLaunchMode;
use meerkat_mob::{MobError, SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec};

use crate::mob_handle_runtime::PreBuildHook;

/// Events read per page while the mob's event log is read.
const SEED_PAGE: usize = 512;

/// A member, keyed as its mob's roster names it: (mob id, roster member id).
type MemberKey = (String, String);

/// One page of a mob's member spawns: (mob id, roster member id, the labels
/// of the spawn) for each, and the cursor to read on from.
pub(crate) type MemberSpawnPage = (Vec<(String, String, BTreeMap<String, String>)>, u64);

/// Where [`PrivateOwners::seeded`] reads the members a mob spawned, with
/// the labels of each spawn: the mob's durable event log.
#[async_trait::async_trait]
pub(crate) trait MemberSpawnLog: Send + Sync {
    /// The member spawns recorded past `cursor`, reading at most `limit`
    /// events, or `None` once the log holds no event past it.
    async fn spawns_after(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Option<MemberSpawnPage>, String>;
}

#[async_trait::async_trait]
impl MemberSpawnLog for meerkat_mob::MobHandle {
    async fn spawns_after(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Option<MemberSpawnPage>, String> {
        let events = self
            .poll_events(cursor, limit)
            .await
            .map_err(|error| error.to_string())?;
        let Some(last) = events.last().map(|event| event.cursor) else {
            return Ok(None);
        };
        let spawns = events
            .into_iter()
            .filter_map(|event| match event.kind {
                meerkat_mob::MobEventKind::MemberSpawned(spawned) => Some((
                    event.mob_id.to_string(),
                    spawned.agent_identity.to_string(),
                    spawned.labels,
                )),
                _ => None,
            })
            .collect();
        Ok(Some((spawns, last)))
    }
}

/// The owner of each private member of one mob runtime (see the module
/// docs). Cheap to clone; clones share the record.
#[derive(Clone, Default)]
pub(crate) struct PrivateOwners {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// What each member build recorded: the member's owner, or `None` for a
    /// member private to no one. The newest truth about a member.
    built: RwLock<BTreeMap<MemberKey, Option<String>>>,
    /// The owners the mob's event log names, as the last spawn of each
    /// member left them.
    logged: RwLock<BTreeMap<MemberKey, String>>,
    /// Set once the event log has been read.
    seeded: tokio::sync::OnceCell<()>,
}

impl std::fmt::Debug for PrivateOwners {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateOwners")
            .field("seeded", &self.inner.seeded.initialized())
            .finish_non_exhaustive()
    }
}

impl PrivateOwners {
    /// Record what `labels`, the labels member `member` of mob `mob_id` is
    /// built with, say about its private owner.
    pub(crate) fn record(
        &self,
        mob_id: &str,
        member: &str,
        labels: Option<&BTreeMap<String, String>>,
    ) {
        let owner = labels
            .and_then(crate::access::private_owner)
            .map(str::to_string);
        self.inner
            .built
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((mob_id.to_string(), member.to_string()), owner);
    }

    /// The person member `member` of mob `mob_id` is private to, as far as
    /// this runtime knows: what its build recorded, else what the event log
    /// named (once read).
    pub(crate) fn owner_of(&self, mob_id: &str, member: &str) -> Option<String> {
        let key = (mob_id.to_string(), member.to_string());
        if let Some(recorded) = self
            .inner
            .built
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return recorded.clone();
        }
        self.inner
            .logged
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
    }

    /// The owner of every private member this runtime knows, keyed by the
    /// identity its memory scope names it by
    /// ([`crate::member_comms_id::logical_memory_identity`]).
    pub(crate) fn memory_owners(&self) -> BTreeMap<String, String> {
        let built = self
            .inner
            .built
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let logged = self
            .inner
            .logged
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        logged
            .iter()
            .filter(|(key, _)| !built.contains_key(*key))
            .map(|(key, owner)| (key, owner.as_str()))
            .chain(
                built
                    .iter()
                    .filter_map(|(key, owner)| owner.as_deref().map(|owner| (key, owner))),
            )
            .map(|((_, member), owner)| {
                (
                    crate::member_comms_id::logical_memory_identity(member),
                    owner.to_string(),
                )
            })
            .collect()
    }

    /// `self`, once the event log of `log`'s mob has been read: the first
    /// call reads it, every later one returns at once. A read that fails
    /// leaves only what member builds recorded.
    pub(crate) async fn seeded(&self, log: &dyn MemberSpawnLog) -> &Self {
        self.inner
            .seeded
            .get_or_init(|| self.read_event_log(log))
            .await;
        self
    }

    async fn read_event_log(&self, log: &dyn MemberSpawnLog) {
        let mut logged = BTreeMap::new();
        let mut cursor = 0;
        loop {
            let (spawns, next) = match log.spawns_after(cursor, SEED_PAGE).await {
                Ok(Some(page)) => page,
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "the private owners of members this process did not build could not be read from the mob's event log"
                    );
                    break;
                }
            };
            cursor = next;
            for (mob_id, member, labels) in spawns {
                let key = (mob_id, member);
                match crate::access::private_owner(&labels) {
                    Some(owner) => {
                        logged.insert(key, owner.to_string());
                    }
                    None => {
                        logged.remove(&key);
                    }
                }
            }
        }
        *self
            .inner
            .logged
            .write()
            .unwrap_or_else(PoisonError::into_inner) = logged;
    }

    /// Record what the roster of `handle`'s mob holds for `member`, as its
    /// build would: for a member this process may not have built, before a
    /// spawn reads its owner (the source of a prompt-context fork).
    pub(crate) async fn record_roster_member(
        &self,
        handle: &meerkat_mob::MobHandle,
        member: &meerkat_mob::AgentIdentity,
    ) {
        if let Ok(Some(entry)) = handle.get_member(member).await {
            self.record(
                handle.mob_id().as_str(),
                member.as_str(),
                Some(&entry.labels),
            );
        }
    }

    /// `hook`, then record the labels of the member the request builds: the
    /// pre-build hook of the base session layer, which every member create
    /// and rebuild passes.
    pub(crate) fn after_hook(&self, hook: PreBuildHook) -> PreBuildHook {
        let owners = self.clone();
        Arc::new(move |req: &mut CreateSessionRequest| {
            let hook = Arc::clone(&hook);
            let owners = owners.clone();
            Box::pin(async move {
                hook(&mut *req).await?;
                if let Some(binding) = req
                    .build
                    .as_ref()
                    .and_then(|build| build.mob_member_binding.as_ref())
                {
                    owners.record(&binding.mob_id, &binding.member, req.labels.as_ref());
                }
                Ok(())
            })
        })
    }

    /// The spawn customizer that gives each member a private member asks for
    /// (a spawn its own turn makes, or a prompt-context fork of it) that
    /// member's owner ([`Self::apply`]).
    pub(crate) fn spawn_customizer(&self) -> Arc<dyn SpawnMemberCustomizer> {
        Arc::new(PrivateOwnerPropagation {
            owners: self.clone(),
        })
    }

    /// The Steward's source of the members private to a person in
    /// `handle`'s mob: every one this record knows, once the event log has
    /// been read. It holds an observer of the mob, which keeps no mob open.
    pub(crate) fn steward_source(
        &self,
        handle: &meerkat_mob::MobHandle,
    ) -> Arc<dyn crate::memory::steward::PrivateMemberSource> {
        Arc::new(StewardPrivateMembers {
            owners: self.clone(),
            handle: handle.observer(),
        })
    }

    /// The customizer body, factored off the trait so tests can drive it.
    /// A spawn asked for by a member, `spawner` (the member whose turn
    /// spawns), else the source of a prompt-context fork, carries no label
    /// only Meta writes of its own (every caller label path refused them)
    /// and gets that member's private owner, when it has one. A member's
    /// own build, a host's spawn and every other spawn are left as they are.
    fn apply(
        &self,
        mob_id: &str,
        spawner: Option<&meerkat_mob::AgentIdentity>,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        let source = match (spawner, &spec.launch_mode) {
            (Some(spawner), _) => spawner.to_string(),
            (
                None,
                MemberLaunchMode::Fork {
                    source_member_id, ..
                },
            ) => source_member_id.to_string(),
            (None, _) => return Ok(()),
        };
        if let Some(labels) = spec.labels.as_mut() {
            labels.retain(|key, _| {
                !crate::member_comms_id::RESERVED_AUTHORITY_LABELS.contains(&key.as_str())
            });
        }
        if let Some(owner) = self.owner_of(mob_id, &source) {
            spec.labels
                .get_or_insert_with(BTreeMap::new)
                .insert(crate::access::PRIVATE_TO_LABEL.to_string(), owner);
        }
        Ok(())
    }
}

/// [`PrivateOwners::spawn_customizer`].
struct PrivateOwnerPropagation {
    owners: PrivateOwners,
}

/// [`PrivateOwners::steward_source`].
struct StewardPrivateMembers {
    owners: PrivateOwners,
    handle: meerkat_mob::MobHandle,
}

#[async_trait::async_trait]
impl crate::memory::steward::PrivateMemberSource for StewardPrivateMembers {
    async fn private_identities(&self) -> std::collections::BTreeSet<String> {
        self.owners
            .seeded(&self.handle)
            .await
            .memory_owners()
            .into_keys()
            .collect()
    }
}

impl SpawnMemberCustomizer for PrivateOwnerPropagation {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.owners
            .apply(ctx.mob_id.as_str(), ctx.spawner_identity.as_ref(), spec)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const OWNER: &str = "console-caller:v1:aa";

    fn private_labels() -> BTreeMap<String, String> {
        BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            OWNER.to_string(),
        )])
    }

    /// A spawn a private member asks for gets its owner and keeps no label
    /// only Meta writes of its own; a fork of a private member is private
    /// too; a spawn of a shared member, and a host's spawn, get none.
    #[test]
    fn a_private_members_spawns_and_forks_get_its_owner() {
        let owners = PrivateOwners::default();
        owners.record("mob", "side", Some(&private_labels()));
        owners.record("mob", "shared", Some(&BTreeMap::new()));
        let spawner = meerkat_mob::AgentIdentity::from("side");

        let mut spawned = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h1"))
            .with_labels(BTreeMap::from([
                ("team".to_string(), "blue".to_string()),
                (
                    crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                    "alpha".to_string(),
                ),
                (
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    "console-caller:v1:bb".to_string(),
                ),
            ]));
        owners
            .apply("mob", Some(&spawner), &mut spawned)
            .expect("a spawn of a private member");
        assert_eq!(
            spawned.labels,
            Some(BTreeMap::from([
                ("team".to_string(), "blue".to_string()),
                (
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    OWNER.to_string()
                ),
            ]))
        );

        let mut forked = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h2"));
        forked.launch_mode = MemberLaunchMode::Fork {
            source_member_id: meerkat_mob::AgentIdentity::from("side"),
            fork_context: meerkat_mob::launch::ForkContext::default(),
        };
        owners
            .apply("mob", None, &mut forked)
            .expect("a fork of a private member");
        assert_eq!(forked.labels, Some(private_labels()));

        let mut of_shared = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h3"));
        owners
            .apply(
                "mob",
                Some(&meerkat_mob::AgentIdentity::from("shared")),
                &mut of_shared,
            )
            .expect("a spawn of a shared member");
        assert_eq!(of_shared.labels, None);

        let mut hosts = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h4"))
            .with_labels(private_labels());
        owners
            .apply("mob", None, &mut hosts)
            .expect("a host's spawn");
        assert_eq!(
            hosts.labels,
            Some(private_labels()),
            "a host's own spawn is left as it is"
        );
        assert_eq!(owners.owner_of("other-mob", "side"), None);
    }

    /// The memory owners name each private member by its logical memory
    /// identity, and the newest build of a member wins over the log.
    #[test]
    fn memory_owners_follow_the_newest_record() {
        let owners = PrivateOwners::default();
        owners.inner.logged.write().unwrap().insert(
            ("mob".to_string(), "retired".to_string()),
            OWNER.to_string(),
        );
        owners.inner.logged.write().unwrap().insert(
            ("mob".to_string(), "rebuilt".to_string()),
            OWNER.to_string(),
        );
        owners.record("mob", "rebuilt", Some(&BTreeMap::new()));
        owners.record("mob", "live", Some(&private_labels()));
        assert_eq!(
            owners.memory_owners(),
            BTreeMap::from([
                ("live".to_string(), OWNER.to_string()),
                ("retired".to_string(), OWNER.to_string()),
            ])
        );
    }
    /// A log whose first read fails and whose later reads name one private
    /// member's spawn.
    #[derive(Default)]
    struct FlakyLog {
        reads: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl MemberSpawnLog for FlakyLog {
        async fn spawns_after(
            &self,
            cursor: u64,
            _limit: usize,
        ) -> Result<Option<MemberSpawnPage>, String> {
            if self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return Err("the event store is unavailable".to_string());
            }
            if cursor > 0 {
                return Ok(None);
            }
            Ok(Some((
                vec![("mob".to_string(), "side".to_string(), private_labels())],
                1,
            )))
        }
    }

    /// A seed whose read of the event log failed is read again by the next
    /// caller, so a transient store error does not leave the owners of the
    /// members this process did not build unknown for its life.
    #[tokio::test]
    async fn the_seed_reads_the_log_again_after_a_failed_read() {
        let owners = PrivateOwners::default();
        let log = FlakyLog::default();
        owners.seeded(&log).await;
        owners.seeded(&log).await;
        assert_eq!(
            owners.owner_of("mob", "side").as_deref(),
            Some(OWNER),
            "a later seed reads the log again"
        );
    }

    /// A prompt-context fork of a private member takes the fork source's
    /// owner, whoever asks for it, and a spawner private to another person
    /// may not fork it at all.
    #[test]
    fn a_fork_launch_of_a_private_member_takes_its_owner() {
        let owners = PrivateOwners::default();
        owners.record("mob", "side", Some(&private_labels()));
        owners.record("mob", "shared", Some(&BTreeMap::new()));
        owners.record(
            "mob",
            "bobs",
            Some(&BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                "console-caller:v1:bb".to_string(),
            )])),
        );
        let fork_of_side = |name: &str| {
            let mut spec = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from(name));
            spec.launch_mode = MemberLaunchMode::Fork {
                source_member_id: meerkat_mob::AgentIdentity::from("side"),
                fork_context: meerkat_mob::launch::ForkContext::default(),
            };
            spec
        };
        let mut asked_by_shared = fork_of_side("h5");
        owners
            .apply(
                "mob",
                Some(&meerkat_mob::AgentIdentity::from("shared")),
                &mut asked_by_shared,
            )
            .expect("a shared spawner's fork of a private member");
        assert_eq!(
            asked_by_shared
                .labels
                .as_ref()
                .and_then(|labels| labels.get(crate::access::PRIVATE_TO_LABEL))
                .map(String::as_str),
            Some(OWNER),
            "the fork is private to its source's owner: {:?}",
            asked_by_shared.labels
        );
        let mut asked_by_another = fork_of_side("h6");
        assert!(
            owners
                .apply(
                    "mob",
                    Some(&meerkat_mob::AgentIdentity::from("bobs")),
                    &mut asked_by_another,
                )
                .is_err(),
            "a spawner private to another person may not fork it"
        );
    }

    /// A member private to a person is built without WorkGraph tools: the
    /// base layer's pre-build hook turns them off for its build, and leaves
    /// a shared member's build as it is.
    #[tokio::test]
    async fn a_private_member_is_built_without_workgraph_tools() {
        let owners = PrivateOwners::default();
        let noop: PreBuildHook =
            Arc::new(|_req: &mut CreateSessionRequest| Box::pin(async { Ok(()) }));
        let hook = owners.after_hook(noop);
        for (labels, private) in [(private_labels(), true), (BTreeMap::new(), false)] {
            let mut req = CreateSessionRequest {
                model: "gpt-5.5".to_string(),
                prompt: meerkat_core::types::ContentInput::Text("hello".to_string()),
                system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    mob_member_binding: Some(meerkat_core::MobMemberBinding {
                        mob_id: "mob".to_string(),
                        role: "worker".to_string(),
                        member: "side".to_string(),
                    }),
                    override_workgraph: meerkat_core::ToolCategoryOverride::Enable,
                    ..Default::default()
                }),
                labels: Some(labels),
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
                injected_context: Vec::new(),
            };
            hook(&mut req).await.expect("the hook runs");
            let workgraph = req
                .build
                .as_ref()
                .map(|build| build.override_workgraph)
                .expect("build options");
            assert_eq!(
                workgraph,
                if private {
                    meerkat_core::ToolCategoryOverride::Disable
                } else {
                    meerkat_core::ToolCategoryOverride::Enable
                },
                "private: {private}"
            );
        }
    }
}
