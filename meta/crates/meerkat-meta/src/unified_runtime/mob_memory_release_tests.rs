//! Rows of what becomes of the agent memory of a mob a host serves beside
//! its launch mob at the mob set's transitions: the mob's destroy, a create
//! under its id, a restart of the host, the mob's idle unload and its load.
//!
//! The fixture is the stored host of the lazy start rows (a state directory
//! that outlives a launch, so a second launch over it is a restart) whose
//! launches keep agent memory in a real directory and hand their composer
//! over with that memory, as a gateway does. Every mob of the host has an
//! agent of ONE name, `desk`. A row writes and reads an agent's memory
//! through the identity runtime of the mob the agent is in, the door the
//! host's own memory requests are served by.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use meerkat_mob::{MobDefinition, MobId};

use super::mob_entries_tests::{
    configured_agent, definition, identity_context, second_mob, set_of, shut_down,
};
use super::mob_memory_firewall_tests::{
    HELD, NOT_HELD, SIDE, Slots, Stored, a_member_seated_in, a_mob_created_by,
    a_session_stored_under, answered, side_chat, stored, takes_in_outside_content,
};
use super::mob_memory_tests::{
    built_desk, call_memory_tool, config_of_the_launch, definition_keeping_memory,
};
use super::{DestroyedMobMemory, StoredHost, UnifiedRuntime, asleep, known};
use crate::gateway_wiring::{HostMobIdentities, HostMobMemory};
use crate::identity_first::agent_memory::{
    AgentMemoryConfig, AgentMemoryProvider, AgentMemoryRecallFailurePolicy,
    AgentMemoryRecallRequest, AgentMemoryRecord, AgentMemorySelection, NewAgentMemory,
};
use crate::identity_first::{
    AgentCustomizer, AgentIdentity, AgentMemoryCustomizer, DurableAgentSpec,
    IdentityFirstRuntimeContext, IdentityRuntime, RosterContext, RosterError, RosterProvider,
};
use crate::memory::coordinator::{MobScopeResolver, StaticMobBinding};
use crate::memory::sqlite_store::{
    HOSTED_MOBS_DIR, HostedAgentsNotBuilt, HostedMemoryRefusal, SqliteAgentMemoryStore,
};
use crate::mob_handle_runtime::MobBootstrapSpec;

/// The one agent name of the rows: each mob of the host has an agent of it.
const DESK: &str = "desk";

/// The roster of a launch of the rows: the agent `desk`, for whichever mob
/// it is asked. It needs no word per mob, so a launch after a restart seats
/// the stored mobs' agents as the first launch did.
struct DeskInEveryMob;

#[async_trait::async_trait]
impl RosterProvider for DeskInEveryMob {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![configured_agent(DESK)])
    }
}

/// A host over a state directory whose launches keep agent memory in a
/// directory of that state directory, or in the directory another host of
/// the rows keeps its memory in.
struct MemoryHost {
    stored: StoredHost,
    memory_root: PathBuf,
    /// The store value of the latest launch: the launch's ONE value, which
    /// its launch mob's memory, its composer and its release hold.
    of_the_launch: Mutex<Option<SqliteAgentMemoryStore>>,
}

impl MemoryHost {
    fn new() -> Self {
        let stored = StoredHost::of(definition_keeping_memory("stored-host"));
        let memory_root = stored.state_dir().join("agent-memory");
        Self {
            stored,
            memory_root,
            of_the_launch: Mutex::default(),
        }
    }

    /// A host over a state directory of its own, which holds none of
    /// `other`'s mobs, whose launches keep their memory where `other`'s do:
    /// what a state directory is that was given a memory root and not the
    /// stores of the mobs the memory is of.
    fn over_the_memory_of(other: &Self) -> Self {
        Self {
            stored: StoredHost::of(definition_keeping_memory("another-host")),
            memory_root: other.memory_root.clone(),
            of_the_launch: Mutex::default(),
        }
    }

    /// The launch's memory root: the launch mob's realm files are directly
    /// in it.
    fn memory_root(&self) -> &Path {
        &self.memory_root
    }

    /// The store value of the latest launch.
    fn store(&self) -> SqliteAgentMemoryStore {
        self.of_the_launch
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .expect("the host was launched")
    }

    /// The directory below the root that holds the hosted mobs' directories.
    fn hosted_dir(&self) -> PathBuf {
        self.memory_root.join(HOSTED_MOBS_DIR)
    }

    /// The directory of the hosted mob `mob`, by the store's own path
    /// function: the rows never build that path themselves.
    fn dir_of(&self, mob: &MobId) -> PathBuf {
        SqliteAgentMemoryStore::open(&self.memory_root)
            .and_then(|store| store.root_of_hosted_mob(mob))
            .expect("the directory of a mob with an id")
    }

    /// The names of the entries directly in the hosted directory, sorted:
    /// each mob's directory, and whatever else lies there.
    fn hosted_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.hosted_dir())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The names the directories of `mobs` have in the hosted directory,
    /// sorted.
    fn names_of(&self, mobs: &[&MobId]) -> Vec<String> {
        let mut names: Vec<String> = mobs
            .iter()
            .filter_map(|mob| {
                self.dir_of(mob)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect();
        names.sort();
        names
    }

    /// A launch over the state directory, as a gateway that keeps agent
    /// memory makes it, in a gateway's order: the launch's spec declares
    /// the memory of the mobs it hosts; the runtime is built; the launch's
    /// store is opened and its release of a hosted mob's memory is bound;
    /// the launch mob gets its identity context with the agent `desk` and
    /// the launch's memory; and the launch's composer is handed over with
    /// that memory, so each mob the host serves beside the launch mob is
    /// composed with a memory of its own.
    async fn launch(&self) -> UnifiedRuntime {
        let mut host = self.launch_before_its_store_is_open().await;
        self.open_the_store_of(&host);
        self.bring_up(&mut host).await;
        host
    }

    /// The start of [`Self::launch`]: the runtime, built from a spec that
    /// declares the memory of the mobs the launch hosts. The launch's store
    /// is not open yet, so the release of a hosted mob's memory is declared
    /// and not bound.
    async fn launch_before_its_store_is_open(&self) -> UnifiedRuntime {
        self.stored
            .launch_composed(MobBootstrapSpec::with_hosted_mob_memory_declared)
            .await
    }

    /// The launch opens its store and binds the store's release of a
    /// hosted mob's memory to its runtime, as a gateway does as soon as its
    /// store is open. What an earlier launch's releases left is finished
    /// here.
    fn open_the_store_of(&self, host: &UnifiedRuntime) {
        let store =
            SqliteAgentMemoryStore::open(&self.memory_root).expect("the launch's memory store");
        *self
            .of_the_launch
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(store.clone());
        host.mob_runtime()
            .mob_memory_release()
            .bind(Arc::new(store));
    }

    /// The rest of [`Self::launch`]: the launch mob's identity context with
    /// the agent `desk` and the launch's memory, then the hand-over of the
    /// launch's composer with that memory (its ONE store value, its
    /// configuration and the launch mob's id).
    async fn bring_up(&self, host: &mut UnifiedRuntime) {
        self.bring_up_with(host, AgentMemoryConfig::default()).await;
    }

    /// [`Self::bring_up`] with the launch's memory configured as `config`,
    /// for its launch mob and for the mobs it hosts.
    async fn bring_up_with(&self, host: &mut UnifiedRuntime, config: AgentMemoryConfig) {
        let of_the_launch_mob = identity_context(host.mob_runtime(), "default");
        of_the_launch_mob
            .runtime
            .set_agent_memory(Some(crate::AgentMemoryRuntimeInjector::new(
                Arc::new(self.store()),
                config.clone(),
            )))
            .await;
        Box::pin(host.install_and_bootstrap_identity_first_context(
            of_the_launch_mob,
            &[configured_agent(DESK)],
        ))
        .await
        .expect("the launch mob's identity bootstrap");
        HostMobIdentities::new(Arc::new(DeskInEveryMob) as Arc<dyn RosterProvider>, true)
            .with_agent_memory_per_mob(Some(HostMobMemory::new(
                self.store(),
                config,
                host.mob_handle().mob_id().clone(),
            )))
            .hand_over_to(host);
    }

    /// A launch as [`Self::launch`] makes it whose memory is configured to
    /// fail where it cannot be read (the recall failure policy `fail`), for
    /// its launch mob and for the mobs it hosts.
    async fn launch_failing_where_memory_cannot_be_read(&self) -> UnifiedRuntime {
        let mut host = self.launch_before_its_store_is_open().await;
        self.open_the_store_of(&host);
        self.bring_up_with(
            &mut host,
            AgentMemoryConfig {
                recall_failure_policy: AgentMemoryRecallFailurePolicy::Fail,
                ..AgentMemoryConfig::default()
            },
        )
        .await;
        host
    }
}

fn desk() -> AgentIdentity {
    AgentIdentity::parse(DESK).expect("identity")
}

/// A mob of `host` beside its launch mob, created by the host from
/// `definition`, with its identity plane composed and `desk` seated.
async fn hosted_mob(host: &UnifiedRuntime, definition: MobDefinition) -> MobId {
    let id = second_mob(host, definition).await;
    let _ = runtime_of(host, Some(&id)).await;
    id
}

/// The identity runtime of the mob `mob` of `host` (`None`: the launch
/// mob). For a mob that is not loaded this is the request that loads it.
async fn runtime_of(host: &UnifiedRuntime, mob: Option<&MobId>) -> Arc<IdentityRuntime> {
    host.resolve_mob(mob)
        .await
        .ok()
        .and_then(|mob| mob.identity_runtime_ref().cloned())
        .expect("the mob has an identity runtime")
}

/// The memory realm the mob's own identity runtime keys its agents by.
async fn realm_of(runtime: &IdentityRuntime) -> String {
    runtime
        .agent_memory_realm()
        .await
        .expect("the mob keeps agent memory")
}

/// The agent `desk` of the mob `mob` remembers a note titled `title`.
async fn remembers(host: &UnifiedRuntime, mob: Option<&MobId>, title: &str) -> AgentMemoryRecord {
    let runtime = runtime_of(host, mob).await;
    let realm = realm_of(&runtime).await;
    runtime
        .remember_agent_memory(
            &realm,
            &desk(),
            NewAgentMemory {
                title: title.to_string(),
                body: format!("the body of {title}"),
                tags: Vec::new(),
            },
        )
        .await
        .expect("the agent's note is written")
}

/// Every note the agent `desk` of the mob `mob` recalls, whole.
async fn recalled(host: &UnifiedRuntime, mob: Option<&MobId>) -> Vec<AgentMemoryRecord> {
    let runtime = runtime_of(host, mob).await;
    let realm = realm_of(&runtime).await;
    runtime
        .recall_agent_memory(AgentMemoryRecallRequest {
            identity: desk(),
            realm,
            query_text: None,
            query_terms: Vec::new(),
            selection: AgentMemorySelection::Always,
            max_entries: 64,
        })
        .await
        .expect("the agent's notes are read")
}

/// The titles of [`recalled`], sorted.
async fn recalls(host: &UnifiedRuntime, mob: Option<&MobId>) -> Vec<String> {
    let mut titles: Vec<String> = recalled(host, mob)
        .await
        .into_iter()
        .map(|record| record.title)
        .collect();
    titles.sort();
    titles
}

/// Every file below `dir` with its bytes, by its path below `dir`. A
/// directory that is not there holds none.
fn files_below(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, into);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let below = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                into.insert(below, bytes);
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

/// The files below `dir` this process holds open now. Read from the
/// process's own table of open files, so it needs no word of the store: a
/// store value that is alive with a database open shows here whoever keeps
/// it. On a platform without that table it answers none, and the rows'
/// other checks (no entry, the mob not loaded) stand alone.
fn open_below(dir: &Path) -> Vec<PathBuf> {
    let mut open = Vec::new();
    #[cfg(target_os = "linux")]
    {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if let Ok(entries) = std::fs::read_dir("/proc/self/fd") {
            for entry in entries.filter_map(Result::ok) {
                if let Ok(target) = std::fs::read_link(entry.path())
                    && target.starts_with(&dir)
                {
                    open.push(target);
                }
            }
        }
        open.sort();
        open.dedup();
    }
    #[cfg(not(target_os = "linux"))]
    let _ = dir;
    open
}

/// The host stops, as a process that ends does: the runtime's own shutdown
/// stands every mob of the host down, and the runtime is dropped. A launch
/// after it over the same state directory is a restart.
async fn stops(host: UnifiedRuntime) {
    shut_down(&host, &[]).await;
    drop(host);
}

/// Destroy the mob `id` through `host`'s mob set and wait until the
/// runtime holds no entry of it. Nothing here releases what the host keeps
/// under the id: that is the destroy's release, which a row asks or leaves
/// out.
async fn destroyed_by_the_set(host: &UnifiedRuntime, id: &MobId) {
    let destroyed = Box::pin(set_of(host).mob_destroy(id)).await;
    assert!(
        destroyed.is_ok(),
        "the mob set destroys the mob: {:?}",
        destroyed.as_ref().err()
    );
    for _ in 0..200 {
        if !host.mob_entries_in_tests().contains(id) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// The memory of a destroyed mob goes with it, and no other mob's does.
///
/// A host with its launch mob and two mobs beside it; the agent `desk` of
/// each has a note of its own. While the mob set hosts a mob its memory is
/// kept: a release asked too early answers that it is kept and removes
/// nothing. A release that names the launch mob, whose memory is the
/// root's own files, is refused and removes nothing of any mob. After the
/// mob set destroyed the second mob and the host released
/// its memory, the one directory of that mob is gone with every file in
/// it; the third mob's agent and the launch mob's agent recall what they
/// recalled, and the files of both are byte for byte what they were. A
/// second release finds nothing and is no failure.
#[tokio::test]
async fn a_destroy_of_one_mob_removes_its_memory_and_leaves_the_other_mobs() {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let launch_id = host.mob_handle().mob_id().clone();
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    let third = hosted_mob(&host, definition_keeping_memory("third")).await;
    remembers(&host, None, "of the launch mob").await;
    remembers(&host, Some(&second), "of the second mob").await;
    remembers(&host, Some(&third), "of the third mob").await;
    assert_eq!(recalls(&host, Some(&second)).await, ["of the second mob"]);
    assert_eq!(recalls(&host, Some(&third)).await, ["of the third mob"]);
    assert_eq!(recalls(&host, None).await, ["of the launch mob"]);
    let (of_the_second, of_the_third) = (stored.dir_of(&second), stored.dir_of(&third));
    assert!(
        !files_below(&of_the_second).is_empty() && !files_below(&of_the_third).is_empty(),
        "each mob beside the launch mob keeps its memory in a directory of its own"
    );
    assert_ne!(of_the_second, of_the_third);

    // While the mob set hosts the mob, its memory is kept.
    let kept = files_below(&of_the_second);
    let too_early = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&second)
        .await;
    assert!(
        matches!(too_early, Ok(DestroyedMobMemory::KeptForTheMobUnderTheId)),
        "a release of the memory of a mob that is hosted answers that its memory is kept: \
         {too_early:?}"
    );
    assert_eq!(files_below(&of_the_second), kept, "and removes nothing");
    // The launch mob's memory is the root's own files: no release names it.
    let whole = files_below(stored.memory_root());
    let of_the_launch_mob = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&launch_id)
        .await;
    assert!(
        of_the_launch_mob.is_err(),
        "a release that names the launch mob is refused: {of_the_launch_mob:?}"
    );
    assert_eq!(
        files_below(stored.memory_root()),
        whole,
        "and removes nothing of any mob"
    );

    // What the other two keep before the destroy, read after their last
    // memory operation so nothing of theirs is written from here on.
    let third_before = recalled(&host, Some(&third)).await;
    let launch_before = recalled(&host, None).await;
    let files_of_the_third = files_below(&of_the_third);
    let files_of_the_launch_mob: BTreeMap<PathBuf, Vec<u8>> = files_below(stored.memory_root())
        .into_iter()
        .filter(|(path, _)| !path.starts_with(HOSTED_MOBS_DIR))
        .collect();

    destroyed_by_the_set(&host, &second).await;
    let released = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&second)
        .await;
    assert!(
        matches!(released, Ok(DestroyedMobMemory::Done)),
        "the host releases the memory of the destroyed mob: {released:?}"
    );
    assert!(
        !of_the_second.exists(),
        "the directory of the destroyed mob is gone, with every file in it: {:?}",
        files_below(&of_the_second).keys().collect::<Vec<_>>()
    );
    assert_eq!(
        stored.hosted_entries(),
        stored.names_of(&[&third]),
        "nothing of it is left under any name: the third mob's directory alone lies there"
    );
    assert_eq!(
        open_below(&stored.hosted_dir())
            .into_iter()
            .filter(|open| !open.starts_with(&of_the_third))
            .collect::<Vec<_>>(),
        Vec::<PathBuf>::new(),
        "and the process holds no file of it open"
    );
    let again = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&second)
        .await;
    assert!(
        matches!(again, Ok(DestroyedMobMemory::Done)),
        "a second release finds nothing and is no failure: {again:?}"
    );

    assert_eq!(
        files_below(&of_the_third),
        files_of_the_third,
        "the files of the third mob's memory are byte for byte what they were"
    );
    assert_eq!(
        files_below(stored.memory_root())
            .into_iter()
            .filter(|(path, _)| !path.starts_with(HOSTED_MOBS_DIR))
            .collect::<BTreeMap<_, _>>(),
        files_of_the_launch_mob,
        "and so are the launch mob's"
    );
    assert_eq!(
        recalled(&host, Some(&third)).await,
        third_before,
        "the third mob's agent recalls what it recalled"
    );
    assert_eq!(
        recalled(&host, None).await,
        launch_before,
        "and so does the launch mob's"
    );
    shut_down(&host, &[&third]).await;
}

/// A destroy through the host's method whose release of the mob's memory
/// fails is not answered as a whole success. The mob is destroyed and
/// listed no more, and the method answers `-32603` with the destroy's
/// report in its data beside the mob's id and the release that failed, in
/// one fixed sentence that carries none of the release's own words (no
/// path of the memory root). What stood in the release's way is left as it
/// was, and a repeated destroy is answered as one for an id the host does
/// not have. A create under the id is refused while the release cannot be
/// made, in the form of a refused create, and goes through once it can.
///
/// The release fails whoever runs the row: where the directory of the mob's
/// memory would be there is a file that is no directory of a mob's memory,
/// which the release does not remove.
#[tokio::test]
async fn a_destroy_through_the_hosts_method_whose_memory_release_fails_says_so_and_destroys_the_mob()
 {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let launch_id = host.mob_handle().mob_id().clone();
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    let in_the_way = stored.dir_of(&second);
    std::fs::create_dir_all(stored.hosted_dir()).expect("the hosted directory");
    // The mob has remembered nothing. A directory its first use made is
    // taken away, so that the file stands where it would be.
    if in_the_way.is_dir() {
        std::fs::remove_dir_all(&in_the_way).expect("the mob's empty memory directory goes");
    }
    std::fs::write(&in_the_way, b"no directory of a mob's memory").expect("a file in the way");
    let names_the_mob = serde_json::json!({"mob_id": second.to_string()});

    let error =
        crate::rpc::mob_hosting_tests::refusal(&host, "meta/mobs/destroy", names_the_mob.clone())
            .await;

    assert_eq!(error["code"], serde_json::json!(-32603), "{error}");
    let data = &error["data"];
    assert_eq!(
        data["kind"],
        serde_json::json!("mob_destroy_release_failed"),
        "{error}"
    );
    assert_eq!(
        data["mob_id"],
        serde_json::json!(second.to_string()),
        "{error}"
    );
    assert_eq!(data["release"], serde_json::json!("memory"), "{error}");
    assert!(
        data["destroy_report"].is_object(),
        "the data carries the destroy's report: {error}"
    );
    let mut members: Vec<&str> = data
        .as_object()
        .expect("the error's data")
        .keys()
        .map(String::as_str)
        .collect();
    members.sort_unstable();
    assert_eq!(
        members,
        vec!["destroy_report", "kind", "mob_id", "release"],
        "{error}"
    );
    let sentence = error["message"].as_str().expect("the error's sentence");
    assert_eq!(
        sentence,
        format!(
            "the mob '{second}' is destroyed and is listed no more; the release of the memory \
             the host kept for it did not succeed (the host's log has the cause); a mob created \
             under this id starts with none of it: the create releases it first and is refused \
             until that succeeds"
        )
    );
    assert!(
        !sentence.contains(&stored.memory_root().display().to_string())
            && !sentence.contains(HOSTED_MOBS_DIR),
        "the sentence carries none of the release's own words: {sentence}"
    );

    assert!(
        set_of(&host).handle_for(&second).await.is_err(),
        "the mob is destroyed: the set holds no mob under the id"
    );
    assert_eq!(
        std::fs::read(&in_the_way).ok().as_deref(),
        Some(b"no directory of a mob's memory".as_slice()),
        "what stood in the release's way is left as it was"
    );
    let again =
        crate::rpc::mob_hosting_tests::refusal(&host, "meta/mobs/destroy", names_the_mob).await;
    assert!(
        again["code"] == serde_json::json!(-32602)
            && again["data"]["kind"] == serde_json::json!("foreign_mob_target")
            && again["data"]["requested_mob_id"] == serde_json::json!(second.to_string())
            && again["data"]["gateway_mob_id"] == serde_json::json!(launch_id.to_string()),
        "a repeated destroy is answered as one for an id the host does not have: {again}"
    );

    // The sentence's last clause: a create under the id releases that
    // memory first and is refused until the release succeeds.
    let under_the_id = serde_json::json!({
        "mob_config": crate::rpc::mob_hosting_tests::config(second.as_str()),
    });
    let refused =
        crate::rpc::mob_hosting_tests::refusal(&host, "meta/mobs/create", under_the_id.clone())
            .await;
    assert!(
        refused["data"]["kind"] == serde_json::json!("mob_create_refused")
            && refused["data"]["mob_id"] == serde_json::json!(second.to_string())
            && refused["message"].as_str().is_some_and(
                |message| message.contains(crate::private_owners::CREATE_RELEASE_FAILED)
            ),
        "a create under the id is refused while the release cannot be made: {refused}"
    );
    assert!(
        set_of(&host).handle_for(&second).await.is_err(),
        "the refused create made no mob"
    );
    assert_eq!(
        std::fs::read(&in_the_way).ok().as_deref(),
        Some(b"no directory of a mob's memory".as_slice()),
        "what stood in the release's way is still as it was"
    );
    std::fs::remove_file(&in_the_way).expect("what stood in the way goes");
    let created =
        crate::rpc::mob_hosting_tests::result(&host, "meta/mobs/create", under_the_id).await;
    assert_eq!(
        created["mob"]["mob_id"],
        serde_json::json!(second.to_string()),
        "the create goes through once the release can be made: {created}"
    );
    shut_down(&host, &[&second]).await;
}

/// A destroy that could not finish its storage cleanup releases nothing of
/// the mob's memory, and the repeat that finishes the destroy releases it.
/// Here the mob is destroyed while a reader holds its store open: the
/// answer is the destroy's own "incomplete", not the form of a failed
/// release, and the mob's memory is still on disk under its id. Once the
/// reader is gone the repeat answers the destroy's report, nothing of the
/// mob's memory is left under any name, and the launch mob's agent recalls
/// what it recalled.
#[tokio::test]
async fn a_destroy_that_could_not_finish_keeps_the_mobs_memory_until_its_repeat_finishes_it() {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    remembers(&host, None, "of the launch mob").await;
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    remembers(&host, Some(&second), "of the second mob").await;
    let holds_the_note = |stored: &MemoryHost| {
        files_below(&stored.dir_of(&second))
            .values()
            .any(|bytes| contains(bytes, b"of the second mob"))
    };
    assert!(
        holds_the_note(&stored),
        "the second mob's note is on disk under its id before the destroy"
    );
    let store = set_of(&host)
        .persistent_mob_store_path(&second)
        .expect("the second mob is kept in a store file");
    let reader = meerkat_mob::MobStorage::persistent(&store).expect("a reader of the mob's store");
    let names_the_mob = serde_json::json!({"mob_id": second.to_string()});

    let incomplete =
        crate::rpc::mob_hosting_tests::refusal(&host, "meta/mobs/destroy", names_the_mob.clone())
            .await;
    assert_eq!(
        incomplete["code"],
        serde_json::json!(-32603),
        "{incomplete}"
    );
    assert!(
        incomplete["message"]
            .as_str()
            .is_some_and(|message| message.contains("storage cleanup outstanding")),
        "the destroy answers what it still owes: {incomplete}"
    );
    assert!(
        incomplete["data"]["kind"].is_null() && incomplete["data"]["release"].is_null(),
        "and not the form of a failed release: {incomplete}"
    );
    assert!(
        holds_the_note(&stored),
        "a destroy that did not finish releases nothing of the mob's memory"
    );

    drop(reader);
    let repeated =
        crate::rpc::mob_hosting_tests::result(&host, "meta/mobs/destroy", names_the_mob).await;
    assert!(
        repeated["destroy_report"].is_object(),
        "the repeat answers the destroy's report: {repeated}"
    );
    assert_eq!(
        stored.hosted_entries(),
        Vec::<String>::new(),
        "nothing of the mob's memory is left under any name once the repeat finished the destroy"
    );
    assert_eq!(recalls(&host, None).await, ["of the launch mob"]);
    shut_down(&host, &[]).await;
}

/// A create under the id that wins between the mob set's destroy and the
/// host's release of the destroyed mob's memory. The create releases the
/// earlier mob's memory first, so the new mob starts with none, and the
/// release the destroy then asks for is refused: the mob set hosts a mob
/// under the id again, and that mob's memory is kept. The destroy answers
/// the form of a failed memory release (`-32603`,
/// `mob_destroy_release_failed`).
///
/// The row pins that answer as it is today. It is the form a typed answer
/// of the memory release for "a mob stands under the id again" will
/// replace: such a destroy finished and nothing of the destroyed mob's
/// memory is left, so its answer is then the destroy's report. Until then
/// the answer's sentence says "is listed no more" of an id that holds the
/// new mob, and a destroy repeated on this answer would destroy that mob.
#[tokio::test]
async fn a_create_under_the_id_before_the_destroys_memory_release_keeps_its_memory_and_the_destroy_answers_a_failed_release()
 {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let of_the_second = definition_keeping_memory("second");
    let again = of_the_second.clone();
    let id = hosted_mob(&host, of_the_second).await;
    remembers(&host, Some(&id), "of the earlier mob").await;
    let earlier = set_of(&host)
        .handle_for(&id)
        .await
        .expect("the set serves the earlier mob")
        .incarnation()
        .to_string();

    let hold = crate::rpc::mob_hosting::destroy_seam::hold_the_next("release", id.as_str());
    let late = crate::rpc::mob_hosting_tests::refusal(
        &host,
        "meta/mobs/destroy",
        serde_json::json!({"mob_id": id.to_string()}),
    );
    tokio::pin!(late);
    crate::rpc::mob_hosting_tests::until_held(late.as_mut(), &hold).await;

    // The mob set finished the destroy: the id is free, and a new mob takes
    // it before the release is asked. Its create releases what the host
    // kept of the earlier mob's memory.
    assert_eq!(
        hosted_mob(&host, again).await,
        id,
        "a new mob under the freed id"
    );
    assert_eq!(
        recalls(&host, Some(&id)).await,
        Vec::<String>::new(),
        "the new mob recalls nothing of the earlier mob"
    );
    remembers(&host, Some(&id), "of the later mob").await;

    hold.release.notify_one();
    let error = tokio::time::timeout(std::time::Duration::from_mins(1), late)
        .await
        .expect("the destroy answers once its release was asked");
    assert_eq!(error["code"], serde_json::json!(-32603), "{error}");
    assert_eq!(
        error["data"]["kind"],
        serde_json::json!("mob_destroy_release_failed"),
        "{error}"
    );
    assert_eq!(
        error["data"]["mob_id"],
        serde_json::json!(id.to_string()),
        "{error}"
    );
    assert_eq!(
        error["data"]["release"],
        serde_json::json!("memory"),
        "{error}"
    );
    assert!(
        error["data"]["destroy_report"].is_object(),
        "the data carries the report of the destroy that finished: {error}"
    );

    let later = set_of(&host)
        .handle_for(&id)
        .await
        .expect("the set serves the mob created under the id")
        .incarnation()
        .to_string();
    assert_ne!(earlier, later, "the mob under the id is the new one");
    assert_eq!(
        recalls(&host, Some(&id)).await,
        ["of the later mob"],
        "the refused release removed nothing of the new mob's memory"
    );
    shut_down(&host, &[&id]).await;
}

/// A mob created under the id an earlier mob had finds nothing of that
/// mob's memory: the create itself releases what the host still keeps of
/// it, before the mob is built.
///
/// The earlier mob is destroyed through the mob set with NO release of its
/// memory (a destroy whose release never ran), so its directory is left.
/// Shown without a restart and across one: the host that left the
/// directory stops, a host starts again over the same state directory, and
/// the create under the id is made there. In both the new mob's agent of
/// the earlier agent's name recalls nothing, what it then remembers is its
/// own alone, and the launch mob's agent and another mob's agent recall
/// what they recalled.
#[tokio::test]
async fn a_mob_created_under_a_freed_id_starts_with_no_memory() {
    for across_a_restart in [false, true] {
        let stored = MemoryHost::new();
        let mut host = stored.launch().await;
        let of_the_second = definition_keeping_memory("second");
        let again = of_the_second.clone();
        let id = hosted_mob(&host, of_the_second).await;
        let third = hosted_mob(&host, definition_keeping_memory("third")).await;
        remembers(&host, None, "of the launch mob").await;
        remembers(&host, Some(&id), "of the earlier mob").await;
        remembers(&host, Some(&third), "of the third mob").await;
        let earlier = set_of(&host)
            .handle_for(&id)
            .await
            .expect("the set serves the earlier mob")
            .incarnation()
            .to_string();

        destroyed_by_the_set(&host, &id).await;
        assert!(
            !files_below(&stored.dir_of(&id)).is_empty(),
            "the destroyed mob's memory is left under its id: nothing released it \
             (across a restart: {across_a_restart})"
        );
        if across_a_restart {
            stops(host).await;
            host = stored.launch().await;
            assert!(
                !files_below(&stored.dir_of(&id)).is_empty(),
                "the memory outlives the host that left it"
            );
        }

        assert_eq!(
            hosted_mob(&host, again).await,
            id,
            "a mob under the same id"
        );
        let later = set_of(&host)
            .handle_for(&id)
            .await
            .expect("the set serves the mob created under the id")
            .incarnation()
            .to_string();
        assert_ne!(earlier, later, "it is another mob");
        assert_eq!(
            recalls(&host, Some(&id)).await,
            Vec::<String>::new(),
            "the mob created under the id recalls nothing of the earlier mob \
             (across a restart: {across_a_restart})"
        );
        remembers(&host, Some(&id), "of the later mob").await;
        assert_eq!(
            recalls(&host, Some(&id)).await,
            ["of the later mob"],
            "what it remembers is its own alone"
        );
        assert!(
            files_below(&stored.dir_of(&id))
                .values()
                .all(|bytes| !contains(bytes, b"of the earlier mob")),
            "no file below the id's directory holds the earlier mob's note"
        );
        assert_eq!(recalls(&host, Some(&third)).await, ["of the third mob"]);
        assert_eq!(recalls(&host, None).await, ["of the launch mob"]);
        shut_down(&host, &[&id, &third]).await;
    }
}

/// Whether `bytes` holds `needle`.
fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

/// A create is refused while what the host keeps of an earlier mob's
/// memory under the id cannot be released: the new mob would read another
/// mob's memory.
///
/// Shown with the store's own release over a real root, with no stand-in:
/// where the directory of the id would be there is something the release
/// does not remove (a file that is no directory of a mob's memory), so the
/// release fails whoever runs the row. The create is refused in the one
/// form every release owed before a create answers in (a refused create
/// with the one sentence of such a release), with nothing of the store's
/// own failure and none of the release's own words in the answer (no path
/// of the memory root); no mob is left under the id; what was there is
/// left as it was;
/// and the launch mob's agent recalls what it recalled. Once what stood in
/// the way is gone the same create goes through, and the mob starts with
/// no memory.
#[tokio::test]
async fn a_create_is_refused_while_an_earlier_mobs_memory_cannot_be_released() {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    remembers(&host, None, "of the launch mob").await;
    let of_the_second = definition_keeping_memory("second");
    let again = of_the_second.clone();
    let id = of_the_second.id.clone();
    let in_the_way = stored.dir_of(&id);
    std::fs::create_dir_all(stored.hosted_dir()).expect("the hosted directory");
    std::fs::write(&in_the_way, b"no directory of a mob's memory").expect("a file in the way");

    let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    let answer = created
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        matches!(
            &created,
            Err(error) if meerkat_mob_mcp::MobMcpState::is_create_refusal(error)
        ) && answer.contains(crate::private_owners::CREATE_RELEASE_FAILED),
        "the create is refused in the one form: {:?}",
        created.as_ref().map(|_| "created").map_err(|_| &answer)
    );
    assert!(
        !answer.contains(&stored.memory_root().display().to_string())
            && !answer.contains(HOSTED_MOBS_DIR)
            && !answer.contains("an earlier mob"),
        "the refusal carries nothing of the store's own failure and none of the release's \
         own words: {answer}"
    );
    assert!(
        set_of(&host).handle_for(&id).await.is_err(),
        "and the set holds no mob under the id"
    );
    assert_eq!(
        std::fs::read(&in_the_way).ok().as_deref(),
        Some(b"no directory of a mob's memory".as_slice()),
        "what stood in the way is left as it was"
    );
    assert_eq!(recalls(&host, None).await, ["of the launch mob"]);

    std::fs::remove_file(&in_the_way).expect("what stood in the way goes");
    assert_eq!(
        hosted_mob(&host, again).await,
        id,
        "the create goes through once the release can be made"
    );
    assert_eq!(recalls(&host, Some(&id)).await, Vec::<String>::new());
    shut_down(&host, &[&id]).await;
}

/// A launch that declared its hosted mobs' memory and has not bound its
/// release yet refuses the create of a mob beside its launch mob: nothing
/// could release what an earlier mob of the id left. The refusal is in the
/// one form every release owed before a create answers in. Once the launch
/// has opened its store, which binds the release, the create goes through.
#[tokio::test]
async fn a_create_is_refused_while_the_launchs_memory_is_declared_and_its_release_not_bound() {
    let stored = MemoryHost::new();
    let mut host = stored.launch_before_its_store_is_open().await;
    let of_the_second = definition_keeping_memory("second");
    let again = of_the_second.clone();
    let id = of_the_second.id.clone();

    let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    assert!(
        matches!(
            &created,
            Err(error) if meerkat_mob_mcp::MobMcpState::is_create_refusal(error)
                && error.to_string().contains(crate::private_owners::CREATE_RELEASE_FAILED)
        ),
        "the create is refused, in the one form, until the release of the launch's memory \
         is bound: {:?}",
        created
            .as_ref()
            .map(|_| "created")
            .map_err(ToString::to_string)
    );
    assert!(
        set_of(&host).handle_for(&id).await.is_err(),
        "and the set holds no mob under the id"
    );
    assert!(
        !stored.hosted_dir().exists(),
        "and nothing was made below the memory root"
    );

    stored.open_the_store_of(&host);
    stored.bring_up(&mut host).await;
    assert_eq!(
        hosted_mob(&host, again).await,
        id,
        "the create goes through once the release is bound"
    );
    shut_down(&host, &[&id]).await;
}

/// A restart of the host keeps each mob's memory, and brings it back only
/// when the mob is loaded.
///
/// A host with its launch mob and two mobs beside it; the agent `desk` of
/// each has a note of its own. The host stops and a host starts again over
/// the same state directory: the mob set restores the stored mobs when a
/// request names them, and nothing before that. At the start no mob beside
/// the launch mob is loaded, the files below the memory root are byte for
/// byte what the first host left, and the start opens no file of a hosted
/// mob. Then each agent recalls its own note, whole (the same record: id,
/// title, body, times), and not another mob's.
#[tokio::test]
async fn each_mobs_memory_is_there_after_a_restart() {
    let stored = MemoryHost::new();
    let first = stored.launch().await;
    let second = hosted_mob(&first, definition_keeping_memory("second")).await;
    let third = hosted_mob(&first, definition_keeping_memory("third")).await;
    remembers(&first, None, "of the launch mob").await;
    remembers(&first, Some(&second), "of the second mob").await;
    remembers(&first, Some(&third), "of the third mob").await;
    let before = [
        recalled(&first, None).await,
        recalled(&first, Some(&second)).await,
        recalled(&first, Some(&third)).await,
    ];
    stops(first).await;
    let left = files_below(stored.memory_root());
    // What this process still holds open of the first host's hosted mobs
    // after that host's shutdown is the first host's, not the start's: the
    // start is asked what it opens beyond that. (A process that ends holds
    // nothing; an unload closes a mob's memory, which its own row shows.)
    let open_before_the_start = open_below(&stored.hosted_dir());
    assert!(
        !files_below(&stored.dir_of(&second)).is_empty()
            && !files_below(&stored.dir_of(&third)).is_empty(),
        "each mob beside the launch mob left its memory in its own directory"
    );

    let host = stored.launch().await;
    let at_the_start = known(&host).await;
    assert!(
        !at_the_start.loaded.contains(&second) && !at_the_start.loaded.contains(&third),
        "a start loads no mob beside the launch mob: {:?}",
        at_the_start.loaded
    );
    assert_eq!(
        open_below(&stored.hosted_dir()),
        open_before_the_start,
        "and opens no file of a mob that is not loaded"
    );
    assert_eq!(
        files_below(&stored.hosted_dir()),
        left.iter()
            .filter_map(|(path, bytes)| {
                path.strip_prefix(HOSTED_MOBS_DIR)
                    .ok()
                    .map(|below| (below.to_path_buf(), bytes.clone()))
            })
            .collect::<BTreeMap<_, _>>(),
        "the hosted mobs' files are byte for byte what the first host left: a start \
         enumerates nothing, releases nothing and moves nothing"
    );

    let after = [
        recalled(&host, None).await,
        recalled(&host, Some(&second)).await,
        recalled(&host, Some(&third)).await,
    ];
    assert_eq!(
        after, before,
        "each mob's agent recalls after the restart the records it recalled before it"
    );
    assert_eq!(recalls(&host, None).await, ["of the launch mob"]);
    assert_eq!(recalls(&host, Some(&second)).await, ["of the second mob"]);
    assert_eq!(recalls(&host, Some(&third)).await, ["of the third mob"]);
    shut_down(&host, &[&second, &third]).await;
}

/// An idle unload and a later load keep a mob's memory, and while the mob
/// is not loaded nothing of its memory is open and nothing opens it.
///
/// The second mob's agent has a note. The mob set unloads the idle mob:
/// its entry goes, and with it the mob's store value, so the process holds
/// no file of that mob's memory open, and the files are all there. Then
/// everything that a list or a status of the host's mobs is, and every
/// memory operation that names another mob, runs: the listing of the
/// host's mobs, the mob set's inventory, the read of a mob "if loaded",
/// the question whether the id is served, a recall and a write of the
/// launch mob's agent and of a third mob's agent, and a release of the
/// not-loaded mob's memory (answered "kept": the set hosts it). After all of
/// them the mob is still not loaded, the runtime holds no entry of it, no
/// file of its memory is open, and its files are byte for byte what the
/// unload left. Then a request that names the mob loads it, and its agent
/// recalls the record it had, whole.
#[tokio::test]
async fn a_hosted_mobs_memory_is_there_after_its_unload_and_its_load_and_nothing_opens_it_in_between()
 {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    let third = hosted_mob(&host, definition_keeping_memory("third")).await;
    remembers(&host, None, "of the launch mob").await;
    remembers(&host, Some(&second), "of the second mob").await;
    remembers(&host, Some(&third), "of the third mob").await;
    let before = recalled(&host, Some(&second)).await;
    let of_the_second = stored.dir_of(&second);
    #[cfg(target_os = "linux")]
    assert!(
        !open_below(&of_the_second).is_empty(),
        "while the mob is loaded and its agent used its memory, its database is open \
         (the reading the row leans on below reads something)"
    );

    // The idle unload takes every idle mob beside the launch mob. A memory
    // operation that names the third mob below loads the third mob, as the
    // request that names a mob does, and never the second.
    asleep(&host, &[&second]).await;
    for _ in 0..200 {
        if !host.mob_entries_in_tests().contains(&second) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        !host.mob_entries_in_tests().contains(&second),
        "the entry of the unloaded mob went: {:?}",
        host.mob_entries_in_tests()
    );
    // The table takes an unloaded mob's entry out first and ends it after,
    // and the entry's end is what closes the memory it served: the files
    // are waited for, for at most ten seconds.
    let mut still_open = open_below(&of_the_second);
    for _ in 0..100 {
        if still_open.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        still_open = open_below(&of_the_second);
    }
    assert_eq!(
        still_open,
        Vec::<PathBuf>::new(),
        "the process holds no file of the unloaded mob's memory open once its entry ended"
    );
    let left = files_below(&of_the_second);
    assert!(!left.is_empty(), "and its files are all there");

    // Lists and statuses of the host's mobs.
    let listed = host.mob_resolver().hosted_mobs().await;
    assert!(
        listed
            .iter()
            .any(|mob| mob.mob_id == second && mob.phase == super::HostedMobPhase::NotLoaded),
        "the listing of the host's mobs names the mob as not loaded"
    );
    assert!(known(&host).await.not_loaded.contains(&second));
    assert!(
        host.mob_resolver().resolve_if_loaded(&second).is_none(),
        "the mob is no mob 'if loaded'"
    );
    assert_eq!(
        host.mob_resolver().served_mob_id(Some(&second)).await.ok(),
        Some(second.clone()),
        "and it is served"
    );
    // Memory operations that name another mob.
    assert_eq!(recalls(&host, None).await, ["of the launch mob"]);
    assert_eq!(recalls(&host, Some(&third)).await, ["of the third mob"]);
    remembers(&host, None, "a later note of the launch mob").await;
    remembers(&host, Some(&third), "a later note of the third mob").await;
    // A release of its memory: it names a path, asks the mob set's records
    // whether the id is hosted, and loads nothing. The mob is hosted.
    let asked = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&second)
        .await;
    assert!(
        matches!(asked, Ok(DestroyedMobMemory::KeptForTheMobUnderTheId)),
        "a release of the memory of a mob that is hosted and not loaded answers that its \
         memory is kept: {asked:?}"
    );

    let known_now = known(&host).await;
    assert!(
        known_now.not_loaded.contains(&second) && !known_now.loaded.contains(&second),
        "none of them loaded the mob: the set manages {:?}",
        known_now.loaded
    );
    assert!(
        !host.mob_entries_in_tests().contains(&second),
        "the runtime holds no entry of it"
    );
    assert_eq!(
        open_below(&of_the_second),
        Vec::<PathBuf>::new(),
        "none of them opened a file of its memory"
    );
    assert_eq!(
        files_below(&of_the_second),
        left,
        "and its files are byte for byte what the unload left"
    );

    // The request that names the mob loads it, over the same directory.
    assert_eq!(
        recalled(&host, Some(&second)).await,
        before,
        "the mob loaded again recalls the record it had"
    );
    assert!(known(&host).await.loaded.contains(&second));
    shut_down(&host, &[&second, &third]).await;
}

/// On a gateway whose only memory is its launch mob's, a create, a destroy
/// and every release leave the memory root byte for byte what it was.
///
/// The launch mob's agent has a note and has recalled it; from there its
/// memory is not asked again until the end, so every byte below the memory
/// root is at rest. Then the host creates a mob beside its launch mob
/// whose agents never use memory, the mob set destroys it, the host asks
/// the release of that mob's memory, twice, and of the launch mob's (which
/// is refused), and creates a mob under the freed id again. After all of
/// it: no directory of hosted mobs was made below the root, every file
/// below the root has the bytes it had, and the launch mob's agent recalls
/// the very record it recalled (id, title, body, times).
#[tokio::test]
async fn a_host_whose_only_memory_is_its_launch_mobs_keeps_its_memory_root_byte_for_byte() {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let launch_id = host.mob_handle().mob_id().clone();
    remembers(&host, None, "of the launch mob").await;
    let before = recalled(&host, None).await;
    assert_eq!(before.len(), 1, "the launch mob's agent has its one note");
    let at_rest = files_below(stored.memory_root());
    assert!(
        !at_rest.is_empty(),
        "the launch mob's memory is in the root"
    );
    assert!(
        !stored.hosted_dir().exists(),
        "a host with no memory but its launch mob's has no directory of hosted mobs"
    );

    // A mob beside the launch mob whose agents never use memory.
    let of_the_second = definition("second");
    let again = of_the_second.clone();
    let id = hosted_mob(&host, of_the_second).await;
    destroyed_by_the_set(&host, &id).await;
    for _ in 0..2 {
        let released = host
            .mob_resolver()
            .release_memory_of_a_destroyed_mob(&id)
            .await;
        assert!(
            matches!(released, Ok(DestroyedMobMemory::Done)),
            "a release that finds nothing is no failure: {released:?}"
        );
    }
    let of_the_launch_mob = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&launch_id)
        .await;
    assert!(
        of_the_launch_mob.is_err(),
        "a release that names the launch mob is refused: {of_the_launch_mob:?}"
    );
    assert_eq!(
        hosted_mob(&host, again).await,
        id,
        "a mob under the freed id"
    );

    assert!(
        !stored.hosted_dir().exists(),
        "no directory of hosted mobs was made below the memory root"
    );
    assert_eq!(
        files_below(stored.memory_root()),
        at_rest,
        "every file below the memory root has the bytes it had"
    );
    assert_eq!(
        recalled(&host, None).await,
        before,
        "the launch mob's agent recalls the very record it recalled"
    );
    shut_down(&host, &[&id]).await;
}

/// A note of the rows titled `title`.
fn note(title: &str) -> NewAgentMemory {
    NewAgentMemory {
        title: title.to_string(),
        body: format!("the body of {title}"),
        tags: Vec::new(),
    }
}

/// Nothing that served an earlier mob writes into the memory of a mob
/// created under its id, or reads it.
///
/// Two things that served the earlier mob are kept past its end, as a call
/// in flight or a run of an engine would keep them: the memory provider its
/// requests were served by, and a copy of its store value. The mob set
/// destroys the mob and a mob is created under its id; the new mob's agent
/// writes a note. A write and a read through either kept thing are refused
/// then, for good; they make no file; the new mob's agent recalls its own
/// note alone, and no file below the id's directory holds a word of the
/// earlier mob or of the late writes.
#[tokio::test]
async fn nothing_that_served_an_earlier_mob_writes_into_the_memory_of_a_mob_created_under_its_id() {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let of_the_second = definition_keeping_memory("second");
    let again = of_the_second.clone();
    let id = hosted_mob(&host, of_the_second).await;
    remembers(&host, Some(&id), "of the earlier mob").await;
    let earlier_runtime = runtime_of(&host, Some(&id)).await;
    let realm = realm_of(&earlier_runtime).await;
    drop(earlier_runtime);
    let kept_provider: Arc<dyn AgentMemoryProvider> = host
        .resolve_mob(Some(&id))
        .await
        .ok()
        .and_then(|mob| mob.identity_request_services())
        .and_then(|services| services.agent_memory_provider)
        .expect("the provider the earlier mob's memory requests are served by");
    let kept_copy = stored
        .store()
        .of_hosted_mob_in_tests(&id)
        .expect("a copy of the earlier mob's store value");
    let all_of_desk = || AgentMemoryRecallRequest {
        identity: desk(),
        realm: realm.clone(),
        query_text: None,
        query_terms: Vec::new(),
        selection: AgentMemorySelection::Always,
        max_entries: 64,
    };
    assert!(
        kept_provider.recall(all_of_desk()).await.is_ok()
            && kept_copy.recall(all_of_desk()).await.is_ok(),
        "while the earlier mob stands, both read its memory"
    );

    destroyed_by_the_set(&host, &id).await;
    assert_eq!(
        hosted_mob(&host, again).await,
        id,
        "a mob under the same id"
    );
    remembers(&host, Some(&id), "of the later mob").await;

    let late_through_the_provider = kept_provider
        .remember(&realm, &desk(), note("late, through the provider"))
        .await;
    let late_through_the_copy = kept_copy
        .remember(&realm, &desk(), note("late, through the copy"))
        .await;
    assert!(
        late_through_the_provider.is_err() && late_through_the_copy.is_err(),
        "a write through what served the earlier mob is refused: {:?}, {:?}",
        late_through_the_provider.map(|record| record.title),
        late_through_the_copy.map(|record| record.title)
    );
    let read_through_the_provider = kept_provider.recall(all_of_desk()).await;
    let read_through_the_copy = kept_copy.recall(all_of_desk()).await;
    assert!(
        read_through_the_provider.is_err() && read_through_the_copy.is_err(),
        "and so is a read: {:?}, {:?}",
        read_through_the_provider.map(|records| records.len()),
        read_through_the_copy.map(|records| records.len())
    );
    assert_eq!(
        recalls(&host, Some(&id)).await,
        ["of the later mob"],
        "the mob created under the id recalls its own note alone"
    );
    assert_eq!(
        stored.hosted_entries(),
        stored.names_of(&[&id]),
        "the refused calls made nothing beside the new mob's directory"
    );
    for (path, bytes) in files_below(&stored.dir_of(&id)) {
        assert!(
            !contains(&bytes, b"of the earlier mob") && !contains(&bytes, b"late, through"),
            "no file of the new mob's memory holds a word of the earlier mob or of a late \
             write: {}",
            path.display()
        );
    }
    shut_down(&host, &[&id]).await;
}

/// A release whose removal did not finish is finished at the next start,
/// and no other mob's memory is touched.
///
/// A release first takes the mob's directory out of the mob's name, then
/// removes it. Here the removal fails after that: the release answers its
/// failure, the mob's own path is free already (a mob created under the id
/// could read nothing of it), and what is left lies in the hosted
/// directory under a name that is no mob's. The host stops. At the next
/// start, before any request, what was left is gone, the third mob's
/// directory is byte for byte what it was, and the third mob's agent
/// recalls its note.
#[tokio::test]
async fn a_release_whose_removal_did_not_finish_is_finished_at_the_next_start() {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    let third = hosted_mob(&host, definition_keeping_memory("third")).await;
    remembers(&host, Some(&second), "of the second mob").await;
    remembers(&host, Some(&third), "of the third mob").await;
    assert_eq!(stored.hosted_entries(), stored.names_of(&[&second, &third]));

    destroyed_by_the_set(&host, &second).await;
    stored.store().fail_removals_in_tests(true);
    let released = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&second)
        .await;
    assert!(
        released.is_err(),
        "a release whose removal fails answers its failure: {released:?}"
    );
    assert!(
        !stored.dir_of(&second).exists(),
        "the destroyed mob's own path is free all the same"
    );
    let left: Vec<String> = stored
        .hosted_entries()
        .into_iter()
        .filter(|name| !stored.names_of(&[&third]).contains(name))
        .collect();
    assert_eq!(
        left.len(),
        1,
        "what the release could not remove lies under one name that is no mob's: {left:?}"
    );
    stops(host).await;
    let of_the_third = files_below(&stored.dir_of(&third));
    assert!(
        stored.hosted_entries().contains(&left[0]),
        "it outlives the host that left it"
    );

    let host = stored.launch().await;
    assert_eq!(
        stored.hosted_entries(),
        stored.names_of(&[&third]),
        "the start finished the release: nothing is left but the third mob's directory"
    );
    assert_eq!(
        files_below(&stored.dir_of(&third)),
        of_the_third,
        "which is byte for byte what it was"
    );
    assert_eq!(recalls(&host, Some(&third)).await, ["of the third mob"]);
    shut_down(&host, &[&third]).await;
}

/// A start touches no directory that no release took out of its mob's
/// name, whatever the mobs its mob set holds.
///
/// A host with two mobs beside its launch mob, each with a note, stops.
/// Another host starts on a state directory that holds NONE of those mobs'
/// stores and keeps its memory in the first host's memory root: what a
/// state directory is that was given a memory root without the mobs it is
/// of. Its mob set holds neither mob. Its start removes nothing: every file
/// below the memory root is byte for byte what the first host left. The
/// first host starts again on its own state directory, and each mob's
/// agent recalls its note.
#[tokio::test]
async fn a_start_touches_no_directory_that_no_release_took_out_of_its_mobs_name() {
    let stored = MemoryHost::new();
    let first = stored.launch().await;
    let second = hosted_mob(&first, definition_keeping_memory("second")).await;
    let third = hosted_mob(&first, definition_keeping_memory("third")).await;
    remembers(&first, Some(&second), "of the second mob").await;
    remembers(&first, Some(&third), "of the third mob").await;
    stops(first).await;
    let left = files_below(&stored.hosted_dir());
    assert!(!left.is_empty());

    let elsewhere = MemoryHost::over_the_memory_of(&stored);
    let other = elsewhere.launch().await;
    let known_there = known(&other).await;
    assert!(
        !known_there.loaded.contains(&second)
            && !known_there.not_loaded.contains(&second)
            && !known_there.loaded.contains(&third)
            && !known_there.not_loaded.contains(&third),
        "the other host's mob set holds neither mob"
    );
    assert_eq!(
        files_below(&stored.hosted_dir()),
        left,
        "its start removed nothing: no release took any of these directories out of its \
         mob's name"
    );
    stops(other).await;
    assert_eq!(files_below(&stored.hosted_dir()), left);

    let host = stored.launch().await;
    assert_eq!(recalls(&host, Some(&second)).await, ["of the second mob"]);
    assert_eq!(recalls(&host, Some(&third)).await, ["of the third mob"]);
    shut_down(&host, &[&second, &third]).await;
}

/// Each launch that hands its memory over for the mobs it hosts binds the
/// release of a hosted mob's memory, and no create can meet a launch that
/// keeps that memory and cannot release it.
///
/// Read from the two launches' sources: the stdio gateway and the builder
/// each hand their memory over (`with_agent_memory_per_mob`), so each binds
/// its store's release. The gateway, whose runtime serves while the launch
/// is still being composed, declares it on its spec
/// (`with_hosted_mob_memory_declared`) and binds it on its runtime once its
/// store is open (`mob_memory_release().bind(`), so a create that came
/// before the store is open is refused and not let through without a
/// release. The builder has its store open before its runtime is built,
/// so it binds on its spec (`mob_memory_release.bind(`) BEFORE the
/// bootstrap (`bootstrap_for_the_builder(`): a member the bootstrap
/// restores is built before the builder's next line, and a create it asks
/// must find the release bound. Without the binding a mob created under a
/// used id would be composed over the memory an earlier mob of the id
/// left.
#[test]
fn each_launch_that_hands_its_memory_over_binds_the_release_of_a_hosted_mobs_memory() {
    let source_of = |file: &str| -> String {
        let path = format!("{}/{file}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path)
            .expect("the launch's source")
            .split_whitespace()
            .collect()
    };

    let gateway = source_of("src/rpc_gateway.rs");
    assert!(
        gateway.contains(".with_agent_memory_per_mob("),
        "the gateway hands its memory over for the mobs it hosts"
    );
    assert!(
        gateway.contains(".with_hosted_mob_memory_declared()"),
        "the gateway declares the memory of the mobs it hosts on its spec"
    );
    assert!(
        gateway.contains(".mob_memory_release().bind("),
        "the gateway binds the release of a hosted mob's memory to its runtime"
    );

    let builder = source_of("src/unified_runtime/builder.rs");
    assert!(
        builder.contains(".with_agent_memory_per_mob("),
        "the builder hands its memory over for the mobs it hosts"
    );
    let bound = builder.find("mob_spec.mob_memory_release.bind(");
    let built = builder.find("UnifiedRuntime::bootstrap_for_the_builder(");
    assert!(
        matches!((bound, built), (Some(bound), Some(built)) if bound < built),
        "the builder binds the release on its spec before its runtime is built: bound at \
         {bound:?}, built at {built:?}"
    );
    assert!(
        !builder.contains(".mob_memory_release().bind("),
        "and binds it nowhere after the runtime is built"
    );
}

/// What a launch composed as a gateway composes it holds beside its
/// runtime: the launch's two memory slots, filled; the launch mob's
/// identity context; and the tracker the launch store's write gate reads.
struct OfAGateway {
    slots: Slots,
    launch_context: Arc<IdentityFirstRuntimeContext>,
    taint_of_the_launch: crate::memory::taint::SessionTaintTracker,
    config: AgentMemoryConfig,
}

impl MemoryHost {
    /// A launch over the state directory as [`Self::launch`] makes it,
    /// whose memory is composed as a gateway composes it: the launch's
    /// store with its firewall (the memory stack over the bundled store,
    /// its engines off), the launch mob's customizer and injector over
    /// that stack with the stack's tracker, the hand-over of the launch's
    /// composer with the launch's ONE store value, and the two memory
    /// slots of the launch's spec filled from the stack.
    async fn launch_as_a_gateway(&self) -> (UnifiedRuntime, OfAGateway) {
        let mut host = self.launch_before_its_store_is_open().await;
        self.open_the_store_of(&host);
        let config = config_of_the_launch();
        let launch_mob = host.mob_handle().mob_id().clone();
        let stack = crate::memory_wiring::attach_memory_engines(
            Arc::new(self.store()),
            &config,
            &crate::memory_wiring::MemoryEnginesConfig::default(),
            crate::memory_wiring::MemoryStackSeams {
                persistent_state: Some(self.stored.state_dir().to_path_buf()),
                transcript_store: Some(Arc::new(meerkat_store::MemoryStore::new())),
                event_sink: Some(host.memory_event_sink()),
                ..Default::default()
            },
        )
        .expect("the launch's memory stack");
        let resolver: Arc<dyn MobScopeResolver> = Arc::new(StaticMobBinding {
            realm: config.realm.clone(),
            mob: launch_mob.to_string(),
        });
        let customizer: Arc<dyn AgentCustomizer> = Arc::new(
            AgentMemoryCustomizer::wrap(None, stack.provider.clone(), config.clone())
                .with_mob_resolver(Some(Arc::clone(&resolver))),
        );
        let injector =
            crate::AgentMemoryRuntimeInjector::new(stack.provider.clone(), config.clone())
                .with_taint_tracker(stack.taint.clone())
                .with_mob_resolver(Some(resolver));
        let composed = identity_context(host.mob_runtime(), "default");
        composed.runtime.set_agent_memory(Some(injector)).await;
        composed
            .runtime
            .set_agent_customizer(Some(Arc::clone(&customizer)))
            .await;
        let launch_context = Arc::new(IdentityFirstRuntimeContext::new(
            Arc::clone(&composed.runtime),
            Arc::new(DeskInEveryMob) as Arc<dyn RosterProvider>,
            None,
            Some(customizer),
            Some(host.mob_runtime().handle().observer().definition().clone()),
        ));
        Box::pin(host.install_and_bootstrap_identity_first_context(
            Arc::clone(&launch_context),
            &[configured_agent(DESK)],
        ))
        .await
        .expect("the launch mob's identity bootstrap");
        HostMobIdentities::new(Arc::new(DeskInEveryMob) as Arc<dyn RosterProvider>, true)
            .with_agent_memory_per_mob(Some(HostMobMemory::new(
                self.store(),
                config.clone(),
                launch_mob,
            )))
            .hand_over_to(&host);
        let slots = Slots::of(&host);
        slots.taint.fill(stack.taint.clone());
        slots
            .side_chat
            .fill(Arc::clone(&stack.provider), config.clone());
        (
            host,
            OfAGateway {
                slots,
                launch_context,
                taint_of_the_launch: stack.taint,
                config,
            },
        )
    }
}

/// One `remember` of an agent's `memory` tool, titled `title`.
fn remember_args(title: &str) -> serde_json::Value {
    serde_json::json!({
        "action": "remember",
        "title": title,
        "body": format!("{title}, in full."),
    })
}

/// After a restart, the memory of a stored second mob is its own again
/// from the first call: for a side chat of its agent and for what its agent
/// takes in.
///
/// A host composed as a gateway composes it, with a second mob; a side
/// chat of the second mob's agent stores a note. The host stops and a host
/// starts again over the same state directory. The second mob is loaded,
/// and before its agent takes any turn a side chat of that agent is built:
/// its first `memory` call recalls the note from before the restart, a new
/// note of it joins that one in the second mob's own store under the side
/// chat's own name, and the launch's store holds neither. Then the second
/// mob's agent takes in content from outside the host: nobody is marked in
/// the launch's memory, that agent's next write is set aside in its own
/// mob's store, and the launch mob's agent of the same name writes as
/// before.
#[tokio::test]
async fn after_a_restart_a_stored_second_mobs_side_chat_and_its_agents_marks_are_in_that_mobs_own_memory()
 {
    let stored_host = MemoryHost::new();
    let (first, gateway) = stored_host.launch_as_a_gateway().await;
    let second = hosted_mob(&first, definition_keeping_memory("second")).await;
    let before = side_chat(&gateway.slots, &second, SIDE, DESK, None);
    assert_eq!(
        before.remembers("A note from before the restart").await,
        Ok(false),
        "the second mob's side chat stores its note"
    );
    stops(first).await;

    let (host, gateway) = stored_host.launch_as_a_gateway().await;
    assert!(
        !known(&host).await.loaded.contains(&second),
        "a start loads no mob beside the launch mob"
    );
    // The mob is loaded by a request that names it, which composes its
    // identity plane and its memory. Its agent takes no turn.
    let runtime_of_the_second = runtime_of(&host, Some(&second)).await;
    let realm = realm_of(&runtime_of_the_second).await;
    let of_the_second = stored_host
        .store()
        .of_hosted_mob_in_tests(&second)
        .expect("a store value over the second mob's own directory");

    let side = side_chat(&gateway.slots, &second, SIDE, DESK, None);
    assert!(
        side.is_offered_memory(),
        "a side chat of the stored mob's agent is offered a `memory` tool after the restart"
    );
    let recalled = side.recalls().await;
    assert!(
        recalled
            .as_ref()
            .is_ok_and(|answer| answer.contains("A note from before the restart")),
        "its first `memory` call recalls the note from before the restart: {recalled:?}"
    );
    assert_eq!(
        side.remembers("A note after the restart").await,
        Ok(false),
        "and a new note of it is stored"
    );
    assert_eq!(
        stored(&of_the_second, &realm).await,
        [
            (
                SIDE.to_string(),
                "A note after the restart".to_string(),
                "active"
            ),
            (
                SIDE.to_string(),
                "A note from before the restart".to_string(),
                "active"
            ),
        ],
        "both notes are in the second mob's own store, under the side chat's own name"
    );
    assert_eq!(
        stored(&stored_host.store(), &gateway.config.realm).await,
        Stored::new(),
        "the launch's store holds neither"
    );
    assert_eq!(side.reached_the_carried_tool(), 0);

    takes_in_outside_content(&gateway.slots, &second, DESK).await;
    assert!(
        gateway.taint_of_the_launch.identity_taint(DESK).is_none(),
        "what the second mob's agent took in marks nobody in the launch's memory"
    );
    let second_context = host
        .resolve_mob(Some(&second))
        .await
        .ok()
        .and_then(|mob| mob.identity_first_context())
        .expect("the second mob has an identity plane after the restart");
    let second_handle = set_of(&host)
        .handle_for(&second)
        .await
        .expect("the set serves the second mob");
    let draft = built_desk(&second_context, second_handle).await;
    let (answer, failed) =
        call_memory_tool(&draft, remember_args("The second desk heard a rumour")).await;
    assert!(
        !failed && answer.contains("QUARANTINED"),
        "the second mob's agent's write after what it took in is set aside: {answer}"
    );
    assert!(
        stored(&of_the_second, &realm).await.contains(&(
            DESK.to_string(),
            "The second desk heard a rumour".to_string(),
            "quarantined"
        )),
        "in its own mob's store, under the agent's name"
    );
    let launch_draft = built_desk(
        &gateway.launch_context,
        host.mob_runtime().handle().observer(),
    )
    .await;
    let (answer, failed) = call_memory_tool(
        &launch_draft,
        remember_args("The launch desk opens at eight"),
    )
    .await;
    assert!(
        !failed && !answer.contains("QUARANTINED"),
        "the launch mob's agent of the same name writes as before: {answer}"
    );
    assert_eq!(
        stored(&stored_host.store(), &gateway.config.realm).await,
        [(
            DESK.to_string(),
            "The launch desk opens at eight".to_string(),
            "active"
        )],
        "the launch's store holds the launch mob's agent's note and nothing of the second mob"
    );
    shut_down(&host, &[&second]).await;
}

/// A session of a destroyed mob is not a session of the mob created under
/// its id: the launch's memory tells the two apart by the journal of the
/// mob that holds the id now.
///
/// A member of the launch mob creates a mob, that mob's journal binds its
/// member to a session, and the session is stored while the mob holds the
/// id: it is a session of the launch's memory. The mob set destroys the
/// mob (a real destroy, which clears the mob's journal), and a member of
/// the launch mob creates a mob under the same id (a real create, through
/// the create's admission). A session the new mob's journal bound is a
/// session of the launch's memory, and the earlier mob's session, which
/// names its mob by the same id, is not: asked of the source a launch
/// assembles its memory engines with, it reads as a session the store
/// does not hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_of_a_destroyed_mob_is_not_a_session_of_the_mob_created_under_its_id() {
    let stored_host = MemoryHost::new();
    let host = stored_host.launch().await;
    let launch_mob = host.mob_handle().mob_id().clone();
    let owners = host.mob_runtime().private_owners().clone();
    let sessions = Arc::new(meerkat::MemoryStore::new());
    let read_by_the_source: Arc<meerkat::MemoryStore> = Arc::clone(&sessions);
    let source = crate::memory_wiring::launch_transcript_source(
        read_by_the_source,
        owners.clone(),
        host.mob_handle(),
    );
    // One definition for both creates: each call of the fixture mints an id.
    let of_the_mob = definition_keeping_memory("created");
    let id = of_the_mob.id.clone();

    let earlier = a_mob_created_by(&host, &launch_mob, "maker", of_the_mob.clone()).await;
    assert_eq!(earlier, id);
    let read = owners.read_loaded_mobs().await;
    assert!(read.is_ok(), "the record reads the created mob: {read:?}");
    let of_the_earlier_mob = a_session_stored_under(
        &sessions,
        a_member_seated_in(&host, &id, "helper").await,
        Some((&id, "helper")),
    )
    .await;
    assert_eq!(
        answered(&source, &of_the_earlier_mob).await,
        HELD,
        "while the earlier mob holds the id, its member's session is a session of the launch's \
         memory"
    );

    destroyed_by_the_set(&host, &id).await;
    assert!(
        set_of(&host).handle_for(&id).await.is_err(),
        "the set holds no mob under the id after the destroy"
    );
    let later = a_mob_created_by(&host, &launch_mob, "second-maker", of_the_mob).await;
    assert_eq!(
        later, id,
        "a mob is created under the id of the destroyed one"
    );
    let read = owners.read_loaded_mobs().await;
    assert!(
        read.is_ok(),
        "the record reads the mob created since: {read:?}"
    );
    assert!(
        owners.first_event_of(id.as_str()).is_some(),
        "the record dated the mob that holds the id now"
    );
    let of_the_new_mob = a_session_stored_under(
        &sessions,
        a_member_seated_in(&host, &id, "helper").await,
        Some((&id, "helper")),
    )
    .await;

    assert_eq!(
        answered(&source, &of_the_earlier_mob).await,
        NOT_HELD,
        "the mob that holds the id now never bound the earlier mob's session: it is no \
         session of the launch's memory"
    );
    assert_eq!(
        answered(&source, &of_the_new_mob).await,
        HELD,
        "a session of the mob that holds the id now is"
    );
    shut_down(&host, &[&id]).await;
}

/// The release of a destroyed mob's memory answers "done" only where it
/// knows that nothing is left, and an error wherever it cannot know.
///
/// Done: on a launch that declared no memory for the mobs it hosts, where
/// nothing can be stored for one. An error, and nothing removed: on a
/// launch that declared that memory and has not bound its release yet;
/// from a table of entries that no runtime is bound to; and through a
/// resolver whose runtime is gone, for a mob the set destroyed and whose
/// memory was not released: its directory stands. Through the runtime's
/// own resolver the same mob's memory is then released, and a second call
/// finds nothing.
#[tokio::test]
async fn the_release_of_a_destroyed_mobs_memory_answers_done_only_where_it_knows() {
    let never_hosted = MobId::from("never-hosted");

    // A launch that keeps no memory for the mobs it hosts.
    let plain = StoredHost::of(definition("plain-host"));
    let host = plain.launch_composed(|spec| spec).await;
    let nothing_declared = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&never_hosted)
        .await;
    assert!(
        matches!(nothing_declared, Ok(DestroyedMobMemory::Done)),
        "a launch that keeps no memory for the mobs it hosts has none to release: \
         {nothing_declared:?}"
    );
    stops(host).await;

    // A table of entries no runtime is bound to: what the launch declared
    // cannot be read there.
    let no_runtime = super::mob_entries::MobEntries::new()
        .release_memory(&never_hosted)
        .await;
    assert!(
        no_runtime.is_err(),
        "a table no runtime is bound to cannot say that nothing is kept: {no_runtime:?}"
    );

    // Declared, and the release not bound yet.
    let stored = MemoryHost::new();
    let mut host = stored.launch_before_its_store_is_open().await;
    let owed = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&never_hosted)
        .await;
    assert!(
        owed.is_err(),
        "a launch that declared the memory and has not bound its release released nothing: \
         {owed:?}"
    );

    // A mob the set destroyed, whose memory was not released yet.
    stored.open_the_store_of(&host);
    stored.bring_up(&mut host).await;
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    remembers(&host, Some(&second), "of the second mob").await;
    destroyed_by_the_set(&host, &second).await;
    let of_the_second = stored.dir_of(&second);
    assert!(
        of_the_second.is_dir(),
        "the destroyed mob's memory is on disk until it is released"
    );

    // Through a resolver whose runtime is gone nothing can be asked: it
    // released nothing, and does not say it did.
    let gone = host
        .mob_resolver()
        .whose_runtime_is_gone_in_tests()
        .release_memory_of_a_destroyed_mob(&second)
        .await;
    assert!(
        gone.is_err(),
        "a resolver whose runtime is gone released nothing and does not say it did: {gone:?}"
    );
    assert!(
        of_the_second.is_dir(),
        "and the mob's memory stands where it stood"
    );

    // Through the runtime's own resolver the memory is released.
    for asked in ["the release", "a second release"] {
        let released = host
            .mob_resolver()
            .release_memory_of_a_destroyed_mob(&second)
            .await;
        assert!(
            matches!(released, Ok(DestroyedMobMemory::Done)),
            "({asked}) the memory of the destroyed mob is released: {released:?}"
        );
        assert!(!of_the_second.exists(), "({asked}) its directory is gone");
    }
    shut_down(&host, &[]).await;
}

/// The name of the file in a hosted mob's memory directory that says which
/// mob, and which incarnation of it, the directory was made for. A stored
/// contract: the rows name it themselves.
const MARK: &str = "mob-incarnation";

impl MemoryHost {
    /// A launch over the state directory that keeps NO agent memory: its
    /// spec declares none, no store is opened and no release of a hosted
    /// mob's memory is bound; the launch mob gets its identity context with
    /// the agent `desk`, and the launch's composer is handed over with no
    /// memory, so each mob the host serves beside the launch mob is
    /// composed without one.
    async fn launch_keeping_no_memory(&self) -> UnifiedRuntime {
        let mut host = self.stored.launch().await;
        let of_the_launch_mob = identity_context(host.mob_runtime(), "default");
        Box::pin(host.install_and_bootstrap_identity_first_context(
            of_the_launch_mob,
            &[configured_agent(DESK)],
        ))
        .await
        .expect("the launch mob's identity bootstrap");
        HostMobIdentities::new(Arc::new(DeskInEveryMob) as Arc<dyn RosterProvider>, true)
            .hand_over_to(&host);
        host
    }

    /// The names of the entries directly in the launch's memory root,
    /// sorted: the launch mob's realm files and whatever else lies there.
    fn root_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.memory_root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

/// Whether the mob `mob`, which `host` serves, has an identity plane after
/// a request that names it: the request that loads a stored mob and asks
/// for its composition.
async fn has_an_identity_plane(host: &UnifiedRuntime, mob: &MobId) -> bool {
    let resolved = host.resolve_mob(Some(mob)).await.ok();
    assert!(resolved.is_some(), "the host serves the mob {mob}");
    resolved.is_some_and(|resolved| resolved.identity_runtime_ref().is_some())
}

/// The incarnation of the mob `mob` as `host`'s mob set holds it. For a
/// stored mob this loads it.
async fn incarnation_of(host: &UnifiedRuntime, mob: &MobId) -> String {
    set_of(host)
        .handle_for(mob)
        .await
        .expect("the set serves the mob")
        .incarnation()
        .to_string()
}

/// The kind of each refusal the compositions of the mob `mob` answered for
/// its memory directory, in order: one for each ask. For a directory marked
/// for another mob the kind says which incarnation the mark names.
fn refusals_of(host: &UnifiedRuntime, mob: &MobId) -> Vec<String> {
    let mut kinds: Vec<String> = whole_refusals_of(host, mob).iter().map(kind_of).collect();
    // A mob is asked for more than once (the request that loads it, and
    // whatever asks for its plane beside that request): each ask is
    // answered, and the rows ask that every answer is of one kind.
    kinds.dedup();
    kinds
}

/// The kind of a refusal, in the rows' words.
fn kind_of(refusal: &HostedMemoryRefusal) -> String {
    match refusal {
        HostedMemoryRefusal::WrittenForAnotherMob {
            marked_incarnation, ..
        } => format!("marked for another mob, at {marked_incarnation}"),
        HostedMemoryRefusal::NotMarked { .. } => "not marked".to_string(),
        HostedMemoryRefusal::MarkUnreadable { .. } => "mark unreadable".to_string(),
        HostedMemoryRefusal::NotADirectory { .. } => "no directory".to_string(),
        HostedMemoryRefusal::Gone { .. } => "gone".to_string(),
    }
}

/// The kind of each refusal the host's log carried for the mob `mob`, in
/// order: one for each line written.
fn logged_refusals_of(host: &UnifiedRuntime, mob: &MobId) -> Vec<String> {
    host.mob_entry_in_tests(mob)
        .map(|entry| entry.memory_refusal_lines_in_tests())
        .unwrap_or_default()
        .iter()
        .map(kind_of)
        .collect()
}

/// Every refusal the compositions of the mob `mob` answered for its memory
/// directory, in order: one for each ask.
fn whole_refusals_of(host: &UnifiedRuntime, mob: &MobId) -> Vec<HostedMemoryRefusal> {
    host.mob_entry_in_tests(mob)
        .map(|entry| entry.memory_refusals_in_tests())
        .unwrap_or_default()
}

/// The directory of `mob` loses what says whose it is, where it has that.
fn loses_its_mark(dir: &Path) {
    let removed = std::fs::remove_file(dir.join(MARK));
    assert!(
        removed.as_ref().map_or_else(
            |error| error.kind() == std::io::ErrorKind::NotFound,
            |()| true
        ),
        "the mark could not be removed: {removed:?}"
    );
}

/// The same mob keeps its memory across a restart, and its directory says
/// whose it is.
///
/// A mob the host serves beside its launch mob remembers a note. Its
/// directory then holds a file that names the mob and the incarnation the
/// directory was made for. After a restart the mob is the same mob (the
/// same incarnation), its agent recalls what it wrote before, and the file
/// is byte for byte what it was.
#[tokio::test]
async fn a_mob_keeps_its_memory_across_a_restart_and_its_directory_names_the_mob_and_its_incarnation()
 {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    remembers(&host, Some(&second), "of the second mob").await;
    let incarnation = incarnation_of(&host, &second).await;
    let mark = stored.dir_of(&second).join(MARK);
    let written = std::fs::read_to_string(&mark).unwrap_or_default();
    assert_eq!(
        written,
        format!("{{\"mob_id\":\"{second}\",\"incarnation\":\"{incarnation}\"}}"),
        "the directory of a mob that wrote names the mob and its incarnation, in the mark's one \
         form"
    );
    stops(host).await;

    let host = stored.launch().await;
    assert_eq!(
        incarnation_of(&host, &second).await,
        incarnation,
        "after the restart it is the same mob"
    );
    assert_eq!(
        recalls(&host, Some(&second)).await,
        ["of the second mob"],
        "the same mob recalls what it wrote before the restart"
    );
    assert_eq!(
        std::fs::read_to_string(&mark).unwrap_or_default(),
        written,
        "and what its directory says is as it was"
    );
    shut_down(&host, &[&second]).await;
}

/// A directory that stands under a mob's name with nothing in it, and
/// nothing that says whose it is, is served to no mob.
///
/// The host serves a mob beside its launch mob and stops. Whatever the
/// mob's own first use made under its name goes, and a directory with
/// nothing in it is put there by something that is no start of the host;
/// the host starts again. The request that loads the mob gives it
/// no identity plane, nothing is made in the directory, and the launch
/// mob's agent recalls what it recalled.
#[tokio::test]
async fn an_existing_directory_that_holds_nothing_and_no_mark_is_served_to_no_mob() {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    remembers(&host, None, "of the launch mob").await;
    let dir = stored.dir_of(&second);
    stops(host).await;
    // What the mob's own first use made goes, and a directory with nothing
    // in it stands under the mob's name: one that no start of the host made.
    let removed = std::fs::remove_dir_all(&dir);
    assert!(
        removed.as_ref().map_or_else(
            |error| error.kind() == std::io::ErrorKind::NotFound,
            |()| true
        ),
        "the mob's directory could not be removed: {removed:?}"
    );
    std::fs::create_dir_all(&dir).expect("a directory under the mob's name");
    let beside_it = stored.hosted_entries();

    let host = stored.launch().await;
    assert!(
        !has_an_identity_plane(&host, &second).await,
        "a mob whose directory nothing proves its own is given no identity plane, though the \
         directory holds nothing"
    );
    assert_eq!(
        refusals_of(&host, &second),
        ["not marked"],
        "every ask of the mob is refused, for this one reason"
    );
    assert_eq!(
        stored.hosted_entries(),
        beside_it,
        "nothing was made beside the directory"
    );
    assert_eq!(
        std::fs::read_dir(&dir).map(Iterator::count).ok(),
        Some(0),
        "nothing was made in the directory"
    );
    assert_eq!(recalls(&host, None).await, ["of the launch mob"]);
    shut_down(&host, &[&second]).await;
}

/// A directory that stands under a mob's name with memory in it, and
/// nothing that says whose it is, is served to no mob.
///
/// A mob remembers a note; the host stops; its directory loses what says
/// whose it is; the host starts again. The request that loads the mob gives
/// it no identity plane, every file below the directory is byte for byte
/// what it was, and the launch mob's agent recalls what it recalled.
#[tokio::test]
async fn an_existing_directory_that_holds_memory_and_no_mark_is_served_to_no_mob() {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    remembers(&host, None, "of the launch mob").await;
    remembers(&host, Some(&second), "of the second mob").await;
    let incarnation = incarnation_of(&host, &second).await;
    stops(host).await;
    let dir = stored.dir_of(&second);
    loses_its_mark(&dir);
    let before = files_below(&dir);
    assert!(!before.is_empty(), "the mob's memory is in its directory");
    let beside_it = stored.hosted_entries();

    let host = stored.launch().await;
    assert!(
        !has_an_identity_plane(&host, &second).await,
        "a mob whose directory nothing proves its own is given no identity plane"
    );
    assert_eq!(
        refusals_of(&host, &second),
        ["not marked"],
        "every ask of the mob is refused, for this one reason"
    );
    assert_eq!(
        whole_refusals_of(&host, &second)
            .first()
            .map(ToString::to_string),
        Some(format!(
            "the agent memory directory {} is not proved to be the mob '{second}''s \
             (incarnation {incarnation}): a directory stands there with no mark",
            dir.display()
        )),
        "the host's log line is told which directory, which mob at which incarnation, and why"
    );
    assert_eq!(
        files_below(&dir),
        before,
        "nothing in the directory was changed"
    );
    assert_eq!(
        stored.hosted_entries(),
        beside_it,
        "nothing was made beside the directory"
    );
    assert_eq!(recalls(&host, None).await, ["of the launch mob"]);
    shut_down(&host, &[&second]).await;
}

/// A mob created under a used id while the launch kept no memory is not
/// served the earlier mob's memory when the launch keeps memory again.
///
/// A launch that keeps memory: a mob remembers a note and is destroyed
/// through the mob set with no release of its memory (a destroy whose
/// release never ran), so its directory stays. A launch that keeps none: a
/// mob is created under its id; no release is asked, so the earlier mob's
/// directory still stays, read by nothing. A
/// launch that keeps memory again: the request that loads the new mob gives
/// it no identity plane, every file below the directory is byte for byte
/// what the earlier mob left, and the launch mob's agent recalls what it
/// recalled.
#[tokio::test]
async fn a_mob_created_under_a_used_id_while_the_launch_kept_no_memory_is_not_served_the_earlier_mobs_memory()
 {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let of_the_second = definition_keeping_memory("second");
    let again = of_the_second.clone();
    let second = hosted_mob(&host, of_the_second).await;
    remembers(&host, None, "of the launch mob").await;
    remembers(&host, Some(&second), "of the earlier mob").await;
    let earlier = incarnation_of(&host, &second).await;
    destroyed_by_the_set(&host, &second).await;
    stops(host).await;

    let host = stored.launch_keeping_no_memory().await;
    assert_eq!(
        second_mob(&host, again).await,
        second,
        "a mob under the same id"
    );
    let later = incarnation_of(&host, &second).await;
    assert_ne!(earlier, later, "it is another mob");
    stops(host).await;
    let dir = stored.dir_of(&second);
    let left = files_below(&dir);
    assert!(
        left.values()
            .any(|bytes| contains(bytes, b"of the earlier mob")),
        "the earlier mob's memory is still under the id: a launch that keeps no memory asked \
         no release of it"
    );

    // Read before the launch: a mob's composition is asked as soon as the mob is loaded.
    let beside_it = stored.hosted_entries();
    let host = stored.launch().await;
    assert_eq!(
        incarnation_of(&host, &second).await,
        later,
        "the launch that keeps memory again serves the mob created under the id"
    );
    assert!(
        !has_an_identity_plane(&host, &second).await,
        "the mob created under the id is given no identity plane over the earlier mob's \
         directory, and so none of its memory"
    );
    assert_eq!(
        refusals_of(&host, &second),
        [format!("marked for another mob, at {earlier}")],
        "every ask of the mob is refused: the directory is marked for the earlier mob"
    );
    assert_eq!(
        files_below(&dir),
        left,
        "nothing in the earlier mob's directory was changed"
    );
    assert_eq!(
        stored.hosted_entries(),
        beside_it,
        "nothing was made beside the directory"
    );
    assert_eq!(recalls(&host, None).await, ["of the launch mob"]);
    shut_down(&host, &[&second]).await;
}

/// Two launches over one memory root serve neither the other's mob of the
/// same id.
///
/// Two hosts, each over a state directory of its own, keep their memory in
/// ONE root. The other host has its mob of the id from a launch that kept
/// no memory. The first host creates its own mob under the same id, which
/// remembers a note. When the other host then starts with memory, the
/// request that loads its mob gives that mob no identity plane, and every
/// file below the directory is byte for byte what the first host's mob
/// left. The first host's mob recalls its note after a restart.
#[tokio::test]
async fn two_launches_over_one_memory_root_serve_neither_the_others_mob_of_the_same_id() {
    let first = MemoryHost::new();
    let other = MemoryHost::over_the_memory_of(&first);
    let of_the_second = definition_keeping_memory("second");
    let host = other.launch_keeping_no_memory().await;
    let second = second_mob(&host, of_the_second.clone()).await;
    let of_the_other = incarnation_of(&host, &second).await;
    stops(host).await;

    let host = first.launch().await;
    assert_eq!(
        hosted_mob(&host, of_the_second).await,
        second,
        "the first host's mob has the same id"
    );
    remembers(&host, Some(&second), "of the first host's mob").await;
    let of_the_first = incarnation_of(&host, &second).await;
    assert_ne!(
        of_the_first, of_the_other,
        "it is another mob than the other host's"
    );
    stops(host).await;
    let dir = first.dir_of(&second);
    let kept = files_below(&dir);
    assert!(
        !kept.is_empty(),
        "the first host's mob has its memory there"
    );

    // Read before the launch: a mob's composition is asked as soon as the mob is loaded.
    let beside_it = first.hosted_entries();
    let host = other.launch().await;
    assert_eq!(
        incarnation_of(&host, &second).await,
        of_the_other,
        "the other host serves its own mob of the id"
    );
    assert!(
        !has_an_identity_plane(&host, &second).await,
        "the other host's mob is given no identity plane over the first host's mob's directory"
    );
    assert_eq!(
        refusals_of(&host, &second),
        [format!("marked for another mob, at {of_the_first}")],
        "every ask of the mob is refused: the directory is marked for the first host's mob"
    );
    assert_eq!(
        files_below(&dir),
        kept,
        "nothing in the first host's mob's directory was changed"
    );
    assert_eq!(
        first.hosted_entries(),
        beside_it,
        "nothing was made beside the directory"
    );
    shut_down(&host, &[&second]).await;
    drop(host);

    let host = first.launch().await;
    assert_eq!(
        recalls(&host, Some(&second)).await,
        ["of the first host's mob"],
        "the first host's mob recalls its note"
    );
    shut_down(&host, &[&second]).await;
}

/// A mob whose directory is refused is given no store: not at the request
/// that loads it, not for a member a door builds for it, not for its stored
/// agent that a wake makes live, and nothing of the launch's own memory.
///
/// The launch mob's agent remembers a note and the host stops. The host
/// is started keeping no memory, a mob is created beside the launch mob
/// and composed there without memory, which seats its agent (a wake of the
/// agent is answered there), and the host stops. A note is written into
/// the mob's directory through a store value made for the mob at its
/// incarnation, and the directory loses what says whose it is. The host
/// starts again, composed as a gateway composes its memory. The request
/// that loads the mob gives it no identity plane. A side chat of the mob's
/// agent, a member a door builds, is offered no `memory` tool. A wake of
/// the mob's stored agent is asked of the mob set. After all three no file
/// below the directory is open in the process and none was changed,
/// nothing was made beside it, the launch's memory root holds what it
/// held, and the launch mob's agent recalls its own note alone.
///
/// What the wake is answered is asked last, as it is built: the mob's own
/// runtime makes its stored agent live, as it does for a mob that has no
/// identity plane, and nothing holds the agent's run starts. The agent is
/// given no agent memory by that.
///
/// The mob's note is written by the row's own store value, which the row
/// drops, and by no launch: the row's witness is the process's table of
/// open files, which must hold no file of the directory before the launch
/// that is asked. In this process a launch that served a hosted mob's
/// memory and stopped still holds that mob's realm files open.
#[tokio::test]
async fn a_mob_whose_directory_is_refused_is_given_no_store_on_a_wake_a_lazy_resolve_or_a_request()
{
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    remembers(&host, None, "of the launch mob").await;
    stops(host).await;
    let host = stored.launch_keeping_no_memory().await;
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    let of_the_set = set_of(&host)
        .handle_for(&second)
        .await
        .expect("the set serves the mob");
    let incarnation = of_the_set.incarnation();
    let seated = of_the_set
        .ensure_member_live(&crate::member_comms_id::mob_member_id(DESK))
        .await;
    assert!(
        seated.is_ok(),
        "the mob's agent is seated by the composition that keeps no memory: a wake of it \
         is answered there: {seated:?}"
    );
    drop(of_the_set);
    stops(host).await;
    let dir = stored.dir_of(&second);
    assert!(
        !dir.exists(),
        "a launch that keeps no memory makes no directory for the mob"
    );
    {
        let of_the_root =
            SqliteAgentMemoryStore::open(stored.memory_root()).expect("the memory root's store");
        let of_the_mob = of_the_root
            .of_hosted_mob(&second, incarnation)
            .expect("a store value made for the mob at its incarnation");
        let written = of_the_mob
            .remember(&format!("mob.{second}"), &desk(), note("of the second mob"))
            .await;
        assert!(
            written.is_ok(),
            "the note is written into the mob's directory: {:?}",
            written.map(|record| record.title)
        );
    }
    loses_its_mark(&dir);
    let before = files_below(&dir);
    assert!(
        before
            .values()
            .any(|bytes| contains(bytes, b"of the second mob")),
        "the mob's note is in its directory"
    );
    let beside_it = stored.hosted_entries();
    assert_eq!(
        open_below(&dir),
        Vec::<PathBuf>::new(),
        "no file below the directory is open in the process before the launch"
    );

    let (host, gateway) = stored.launch_as_a_gateway().await;
    let in_the_root = stored.root_entries();
    assert!(
        !has_an_identity_plane(&host, &second).await,
        "the request that loads the mob gives it no identity plane"
    );
    assert_eq!(
        refusals_of(&host, &second),
        ["not marked"],
        "every ask of the mob is refused, for this one reason"
    );
    assert!(
        !side_chat(&gateway.slots, &second, SIDE, DESK, None).is_offered_memory(),
        "a side chat of the mob's agent is offered no `memory` tool: no store is the mob's, \
         and the launch's is not given in its place"
    );
    assert_eq!(
        open_below(&dir),
        Vec::<PathBuf>::new(),
        "no file below the directory is open in the process before a wake"
    );
    // A wake of the mob's stored agent, asked of the mob set as a delivery
    // asks it. Whatever it is answered, it gives the agent no store.
    let of_the_set = set_of(&host)
        .handle_for(&second)
        .await
        .expect("the set serves the mob");
    let woken = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        of_the_set.ensure_member_live(&crate::member_comms_id::mob_member_id(DESK)),
    )
    .await;
    assert_eq!(
        open_below(&dir),
        Vec::<PathBuf>::new(),
        "no file below the directory is open in the process (the wake was answered {woken:?})"
    );
    assert_eq!(
        files_below(&dir),
        before,
        "nothing in the directory was changed (the wake was answered {woken:?})"
    );
    assert_eq!(
        stored.hosted_entries(),
        beside_it,
        "nothing was made beside the directory"
    );
    assert_eq!(
        stored.root_entries(),
        in_the_root,
        "nothing was made in the launch's memory root for the mob"
    );
    assert_eq!(
        recalls(&host, None).await,
        ["of the launch mob"],
        "the launch mob's agent recalls its own note alone"
    );
    // What the wake was answered, and what holds the agent it made live.
    let woken_as = match &woken {
        Ok(Ok(())) => "live".to_string(),
        Ok(Err(refused)) => format!("refused: {refused}"),
        Err(_) => "not answered in twenty seconds".to_string(),
    };
    assert_eq!(
        woken_as, "live",
        "the mob's own runtime makes its stored agent live, as for a mob without an \
         identity plane"
    );
    let held = of_the_set.member_holds_run_starts(
        &crate::member_comms_id::mob_member_id(DESK),
        meerkat_mob::HostRunStartHoldReason::ToolsNotPublished,
    );
    assert!(
        matches!(held, Ok(false)),
        "nothing holds the run starts of the agent the wake made live: {held:?}"
    );
    drop(of_the_set);
    shut_down(&host, &[&second]).await;
}

/// In process, over the launch a gateway composes: the same mob keeps its
/// memory, and a mob created under a used id is refused the earlier mob's,
/// across a destroy, a launch that keeps no memory, a create and a launch
/// that keeps memory again, with a restart between the steps.
///
/// Two mobs beside the launch mob remember a note each; one of them is
/// destroyed through the mob set with no release of its memory. The host
/// is started keeping no memory: a mob is created under the destroyed
/// mob's id; the other mob is left as it is. The host is started keeping
/// memory again: the
/// mob that was never created anew is the same mob and recalls its note;
/// the mob created under the used id is given no identity plane, a side
/// chat of its agent is offered no `memory` tool, and every file below the
/// earlier mob's directory is byte for byte what that mob left. One more
/// restart finds both as they were.
///
/// The destroy is made in the first launch: in this process a destroy in a
/// launch that follows another launch over the same state directory is
/// left with its storage cleanup outstanding, by what the earlier launch's
/// runtime still holds of the mob's store after it stopped.
#[tokio::test]
async fn the_same_mob_keeps_its_memory_and_a_successor_is_refused_across_destroy_off_create_and_on()
{
    let stored = MemoryHost::new();
    let (host, of_the_first_launch) = stored.launch_as_a_gateway().await;
    let of_the_second = definition_keeping_memory("second");
    let again = of_the_second.clone();
    let second = hosted_mob(&host, of_the_second).await;
    let kept = hosted_mob(&host, definition_keeping_memory("kept")).await;
    remembers(&host, None, "of the launch mob").await;
    remembers(&host, Some(&second), "of the earlier mob").await;
    remembers(&host, Some(&kept), "of the mob that stays").await;
    let earlier = incarnation_of(&host, &second).await;
    let staying = incarnation_of(&host, &kept).await;
    destroyed_by_the_set(&host, &second).await;
    stops(host).await;
    drop(of_the_first_launch);

    let host = stored.launch_keeping_no_memory().await;
    assert_eq!(
        second_mob(&host, again).await,
        second,
        "a mob under the same id"
    );
    let later = incarnation_of(&host, &second).await;
    assert_ne!(earlier, later, "it is another mob");
    stops(host).await;
    let dir = stored.dir_of(&second);
    let left = files_below(&dir);
    assert!(
        left.values()
            .any(|bytes| contains(bytes, b"of the earlier mob")),
        "the earlier mob's memory is still under the id"
    );

    for start in [
        "the first start that keeps memory again",
        "the start after it",
    ] {
        // Read before the launch: a mob's composition is asked as soon as the mob is loaded.
        let beside_it = stored.hosted_entries();
        let (host, gateway) = stored.launch_as_a_gateway().await;
        assert_eq!(
            incarnation_of(&host, &kept).await,
            staying,
            "({start}) the mob that was never created anew is the same mob"
        );
        assert_eq!(
            recalls(&host, Some(&kept)).await,
            ["of the mob that stays"],
            "({start}) and it recalls what it wrote before"
        );
        assert_eq!(incarnation_of(&host, &second).await, later);
        assert!(
            !has_an_identity_plane(&host, &second).await,
            "({start}) the mob created under the used id is given no identity plane"
        );
        assert_eq!(
            refusals_of(&host, &second),
            [format!("marked for another mob, at {earlier}")],
            "({start}) every ask of the mob is refused: the directory is marked for the earlier mob"
        );
        assert!(
            !side_chat(&gateway.slots, &second, SIDE, DESK, None).is_offered_memory(),
            "({start}) a side chat of its agent is offered no `memory` tool"
        );
        assert_eq!(
            files_below(&dir),
            left,
            "({start}) nothing in the earlier mob's directory was changed"
        );
        assert_eq!(
            stored.hosted_entries(),
            beside_it,
            "({start}) nothing was made beside the directory"
        );
        assert_eq!(recalls(&host, None).await, ["of the launch mob"]);
        stops(host).await;
        drop(gateway);
    }
}

/// The private retirements a hosted mob's memory recorded are read as that
/// mob's where the mob is composed, after a restart. A first launch creates
/// the mob, and the mob's memory records the retirement of a private
/// member, through the store made for the mob at its incarnation; the host
/// stops. A second launch over the same state directory composes the mob
/// over that memory: after the composition named the mob, so the
/// retirements name members of that mob and none of the launch mob, and
/// before the record says it has read the mob, so no reader takes an ended
/// private member of that mob for a member private to no one meanwhile.
///
/// This is the pin of where the private record reads a hosted mob's
/// recorded retirements, in the form a restart gives it. Its earlier form
/// wrote the retirement under the mob's id before any mob held the id.
/// That premise fell with the mark of a hosted mob's directory: a
/// directory is the mob's only where it was made for the mob at its
/// incarnation, so a memory that holds a mob's retirements before the
/// mob is composed exists only after an earlier life of the host.
#[tokio::test]
async fn after_a_restart_the_retirements_a_hosted_mobs_memory_recorded_are_read_as_that_mobs_where_it_is_composed()
 {
    use crate::memory::capabilities::StewardStore as _;
    use crate::mob_announcements::MemberPerson;

    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    let incarnation = set_of(&host)
        .handle_for(&second)
        .await
        .expect("the set serves the second mob")
        .incarnation();
    let recorded = stored
        .store()
        .of_hosted_mob(&second, incarnation)
        .expect("the second mob's own store")
        .record_private_retirement(&format!("mob.{second}"), "ghost", None, "retired", 5)
        .await;
    assert!(recorded.is_ok(), "the retirement is recorded: {recorded:?}");
    stops(host).await;

    // The second life: the request below names the mob, and the mob's
    // identity plane is composed over a memory that holds the retirement.
    let host = stored.launch().await;
    let launch_mob = host.mob_handle().mob_id().clone();
    let _ = runtime_of(&host, Some(&second)).await;
    let owners = host.mob_runtime().private_owners().clone();
    let mut read = false;
    for _ in 0..200 {
        if owners.has_read_the_mob(second.as_str()) {
            read = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(read, "the record comes to say it has read the second mob");
    let private_to_someone =
        MemberPerson::Private(crate::access::UNKNOWN_PRIVATE_OWNER.to_string());
    assert_eq!(
        owners.person_of(second.as_str(), "ghost"),
        private_to_someone,
        "once the record says it has read the mob, it holds what the mob's memory recorded"
    );
    assert_ne!(
        owners.person_of(launch_mob.as_str(), "ghost"),
        private_to_someone,
        "the second mob's memory names no member of the launch mob"
    );
    stops(host).await;
}

/// The host's log carries a mob's refused memory directory once while the
/// mob stays loaded, and again when the refusal changes. A logging fact
/// only: every ask is refused whether or not it is logged, and nothing is
/// decided from what was logged. What was logged is kept with the loaded
/// mob: a mob that is unloaded and loaded again is logged again (no row
/// here unloads it).
///
/// A mob's directory has lost what says whose it is. Three requests name
/// the mob: each ask of its composition is refused, and the host's log has
/// ONE line for all of them. Then a mark that is no mark is put in the
/// directory: the next request's ask is refused for that reason and the
/// log has a second line; one more request is refused and adds none.
#[tokio::test]
async fn the_hosts_log_carries_a_refused_directory_once_while_its_mob_stays_loaded_and_again_when_the_refusal_changes()
 {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    remembers(&host, Some(&second), "of the second mob").await;
    stops(host).await;
    let dir = stored.dir_of(&second);
    loses_its_mark(&dir);

    let host = stored.launch().await;
    for _ in 0..3 {
        assert!(
            !has_an_identity_plane(&host, &second).await,
            "each request finds the mob without an identity plane"
        );
    }
    let asked = whole_refusals_of(&host, &second).len();
    assert!(
        asked >= 3,
        "each request asked the mob's composition and was refused: {asked}"
    );
    assert_eq!(refusals_of(&host, &second), ["not marked"]);
    assert_eq!(
        logged_refusals_of(&host, &second),
        ["not marked"],
        "the host's log has one line for all of them"
    );
    let words = host
        .mob_entry_in_tests(&second)
        .map(|entry| entry.memory_refusal_line_words_in_tests())
        .unwrap_or_default();
    assert!(
        words.iter().all(|line| {
            line.contains("can still be woken, and then runs without agent memory")
                && !line.contains("none of its agents is built")
        }),
        "under the default policy the line says an agent of the mob can still be woken and \
         then runs without agent memory: {words:?}"
    );
    assert_eq!(
        host.mob_entry_in_tests(&second)
            .and_then(|entry| entry.agents_not_built_in_tests()),
        None,
        "and nothing is withheld from the mob under the default policy"
    );

    std::fs::write(dir.join(MARK), b"this is no mark").expect("a mark that is no mark");
    assert!(!has_an_identity_plane(&host, &second).await);
    assert_eq!(
        logged_refusals_of(&host, &second),
        ["not marked", "mark unreadable"],
        "a refusal for another reason is a second line"
    );
    assert!(!has_an_identity_plane(&host, &second).await);
    assert_eq!(
        logged_refusals_of(&host, &second),
        ["not marked", "mark unreadable"],
        "and one more ask refused for that reason adds none"
    );
    assert_eq!(
        refusals_of(&host, &second),
        ["not marked", "mark unreadable"],
        "every ask was refused, each for the reason that stood"
    );
    assert!(
        whole_refusals_of(&host, &second).len() >= asked + 2,
        "the two later requests were each asked and refused"
    );
    shut_down(&host, &[&second]).await;
}

/// What a wake of the stored agent `desk` of the mob `mob` is answered,
/// asked of `host`'s mob set as a delivery asks it: "live", or the typed
/// refusal of the wake.
async fn wake_of_desk(host: &UnifiedRuntime, mob: &MobId) -> String {
    let of_the_set = set_of(host)
        .handle_for(mob)
        .await
        .expect("the set serves the mob");
    let woken = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        of_the_set.ensure_member_live(&crate::member_comms_id::mob_member_id(DESK)),
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

/// Under the recall failure policy `fail`, a stored agent of a mob whose
/// memory directory is refused is not built: a wake of it is refused,
/// typed and to be retried, until the directory is the mob's again.
///
/// A launch that keeps no memory creates a mob and seats its agent. A note
/// is written into the mob's directory through a store value made for the
/// mob at its incarnation, and the directory loses what says whose it is.
/// The host starts with its memory configured to fail where it cannot be
/// read. The request that loads the mob gives it no identity plane, and a
/// wake of its stored agent is refused as a build its host asked to retry,
/// the first time and again. Then the mark the directory lost is put back,
/// byte for byte: the next request composes the mob, the FIRST wake of its
/// agent after that is answered (the composition retries the builds that
/// were refused; the row waits for no retry timer), and the agent recalls
/// the note the directory held.
///
/// Under the default policy the same wake makes the agent live without
/// agent memory, which the host's log says: the row of a refused mob that
/// is given no store pins that.
#[tokio::test]
async fn under_the_fail_policy_a_refused_mobs_stored_agent_is_not_built_until_the_directory_is_the_mobs_again()
 {
    let stored = MemoryHost::new();
    let host = stored.launch_keeping_no_memory().await;
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    let incarnation = set_of(&host)
        .handle_for(&second)
        .await
        .expect("the set serves the mob")
        .incarnation();
    assert_eq!(
        wake_of_desk(&host, &second).await,
        "live",
        "the mob's agent is seated by the composition that keeps no memory"
    );
    stops(host).await;
    let dir = stored.dir_of(&second);
    {
        let of_the_root =
            SqliteAgentMemoryStore::open(stored.memory_root()).expect("the memory root's store");
        let of_the_mob = of_the_root
            .of_hosted_mob(&second, incarnation)
            .expect("a store value made for the mob at its incarnation");
        let written = of_the_mob
            .remember(&format!("mob.{second}"), &desk(), note("of the second mob"))
            .await;
        assert!(
            written.is_ok(),
            "the note is written into the mob's directory: {:?}",
            written.map(|record| record.title)
        );
    }
    let mark = std::fs::read(dir.join(MARK)).expect("the directory's mark");
    loses_its_mark(&dir);

    let host = stored.launch_failing_where_memory_cannot_be_read().await;
    assert!(
        !has_an_identity_plane(&host, &second).await,
        "the request that loads the mob gives it no identity plane"
    );
    assert_eq!(refusals_of(&host, &second), ["not marked"]);
    let not_built = host
        .mob_entry_in_tests(&second)
        .and_then(|entry| entry.agents_not_built_in_tests());
    assert!(
        matches!(
            not_built,
            Some(HostedAgentsNotBuilt::MemoryRefused(
                HostedMemoryRefusal::NotMarked { .. }
            ))
        ),
        "none of the mob's agents is built, for the mob's refusal: {not_built:?}"
    );
    let told = not_built.map(|why| why.to_string()).unwrap_or_default();
    assert!(
        told.contains("a directory stands there with no mark")
            && told.contains("recall failure policy is `fail`")
            && !told.contains(&dir.display().to_string()),
        "a refused build is told why in words that name no path: {told:?}"
    );
    let words = host
        .mob_entry_in_tests(&second)
        .map(|entry| entry.memory_refusal_line_words_in_tests())
        .unwrap_or_default();
    assert!(
        words.len() == 1
            && words.iter().all(|line| {
                line.contains("none of its agents is built") && !line.contains("can still be woken")
            }),
        "the host's log line says, once, that none of the mob's agents is built, and does \
         not say that one can still be woken: {words:?}"
    );
    for ask in ["the first wake", "a second wake"] {
        assert_eq!(
            wake_of_desk(&host, &second).await,
            "refused: BuildPreparationRetry",
            "({ask}) under the fail policy the refused mob's stored agent is not built: its \
             wake is refused, to be retried"
        );
    }

    // The directory is the mob's again: the mark it lost is put back.
    std::fs::write(dir.join(MARK), &mark).expect("the mark is put back");
    assert!(
        has_an_identity_plane(&host, &second).await,
        "the next request composes the mob over its own directory"
    );
    assert_eq!(
        host.mob_entry_in_tests(&second)
            .and_then(|entry| entry.agents_not_built_in_tests()),
        None,
        "its agents are built again: nothing is withheld"
    );
    // The composition that succeeds ends the refusal: it asks the mob to
    // retry the builds it refused meanwhile, so the first wake after it is
    // answered, with no wait for the mob's own retry.
    assert_eq!(
        wake_of_desk(&host, &second).await,
        "live",
        "the first wake of the agent after the composition is answered"
    );
    assert_eq!(
        recalls(&host, Some(&second)).await,
        ["of the second mob"],
        "and the agent recalls what the directory held"
    );
    shut_down(&host, &[&second]).await;
}

/// A release of a destroyed mob's memory that is asked while a mob stands
/// under the id is no failure of the release: the mob's memory is kept,
/// the call released nothing, and its answer says exactly that, apart from
/// "done".
///
/// Three ways a mob stands under the id when the release is asked: the mob
/// set hosts the mob and it is loaded; the mob is hosted and not loaded; a
/// new mob was created under the id of a mob the set destroyed, before the
/// release of the destroyed mob's memory was asked (the create released
/// the earlier mob's memory first, so what lies under the id is the new
/// mob's own). In each the release answers that the mob's memory is kept,
/// and the files under the id are byte for byte what they were. The mob
/// that was not loaded is not loaded by it, and each mob recalls
/// afterwards what it had. What is a failure stays one: a release that
/// names the launch mob.
#[tokio::test]
async fn a_release_asked_while_a_mob_stands_under_the_id_is_answered_without_a_failure_and_removes_nothing()
 {
    let stored = MemoryHost::new();
    let host = stored.launch().await;
    let launch_id = host.mob_handle().mob_id().clone();
    let second = hosted_mob(&host, definition_keeping_memory("second")).await;
    let of_the_third = definition_keeping_memory("third");
    let again = of_the_third.clone();
    let third = hosted_mob(&host, of_the_third).await;
    remembers(&host, Some(&second), "of the second mob").await;
    remembers(&host, Some(&third), "of the earlier mob").await;
    // Each reading: the way the mob stands, whether the release answered
    // that the mob's memory is kept, whether the files under the id are
    // what they were.
    let mut read: Vec<(&str, bool, bool)> = Vec::new();

    // The mob set hosts the mob, and it is loaded.
    let of_the_second = stored.dir_of(&second);
    let kept = files_below(&of_the_second);
    assert!(!kept.is_empty(), "the loaded mob keeps its memory on disk");
    let answer = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&second)
        .await;
    read.push((
        "hosted and loaded",
        matches!(answer, Ok(DestroyedMobMemory::KeptForTheMobUnderTheId)),
        files_below(&of_the_second) == kept,
    ));

    // The mob is hosted and not loaded. Its entry's end closes the memory
    // it served: the files are read once none of them is open.
    asleep(&host, &[&second]).await;
    for _ in 0..200 {
        if !host.mob_entries_in_tests().contains(&second) && open_below(&of_the_second).is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        known(&host).await.not_loaded.contains(&second)
            && !host.mob_entries_in_tests().contains(&second),
        "the unloaded mob is hosted and not loaded: {:?}",
        host.mob_entries_in_tests()
    );
    let left = files_below(&of_the_second);
    assert!(!left.is_empty(), "the unloaded mob's files are all there");
    let answer = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&second)
        .await;
    read.push((
        "hosted and not loaded",
        matches!(answer, Ok(DestroyedMobMemory::KeptForTheMobUnderTheId)),
        files_below(&of_the_second) == left,
    ));
    let known_now = known(&host).await;
    assert!(
        known_now.not_loaded.contains(&second)
            && !known_now.loaded.contains(&second)
            && !host.mob_entries_in_tests().contains(&second),
        "the release loaded no mob: the set manages {:?}",
        known_now.loaded
    );

    // A new mob under the id of a mob the set destroyed, created before the
    // release of the destroyed mob's memory is asked.
    destroyed_by_the_set(&host, &third).await;
    assert_eq!(
        hosted_mob(&host, again).await,
        third,
        "a new mob under the freed id"
    );
    assert_eq!(
        recalls(&host, Some(&third)).await,
        Vec::<String>::new(),
        "its create released the earlier mob's memory: the new mob recalls nothing of it"
    );
    remembers(&host, Some(&third), "of the later mob").await;
    let of_the_third = stored.dir_of(&third);
    let kept = files_below(&of_the_third);
    assert!(!kept.is_empty(), "the new mob keeps its memory on disk");
    let answer = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&third)
        .await;
    read.push((
        "created under a destroyed mob's id",
        matches!(answer, Ok(DestroyedMobMemory::KeptForTheMobUnderTheId)),
        files_below(&of_the_third) == kept,
    ));

    assert_eq!(
        read,
        [
            ("hosted and loaded", true, true),
            ("hosted and not loaded", true, true),
            ("created under a destroyed mob's id", true, true),
        ],
        "each release asked while a mob stands under the id answers that the mob's memory \
         is kept and removes nothing"
    );
    assert_eq!(
        recalls(&host, Some(&third)).await,
        ["of the later mob"],
        "the mob created under the id recalls its own note"
    );
    assert_eq!(
        recalls(&host, Some(&second)).await,
        ["of the second mob"],
        "and the mob that was not loaded recalls its note once a request loads it"
    );

    // What is a failure stays one.
    let of_the_launch_mob = host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&launch_id)
        .await;
    assert!(
        of_the_launch_mob.is_err(),
        "a release that names the launch mob is refused: {of_the_launch_mob:?}"
    );
    shut_down(&host, &[&second, &third]).await;
}

/// The fixture of these rows for the rows of another door: a host over a
/// state directory whose launches keep agent memory, with one mob beside
/// its launch mob whose agent `desk` has a note. A row of a door reads
/// what the host keeps of that mob beside the mob set's own records
/// through it, and starts the host again over the same state.
pub(crate) struct HostOfARememberingMob {
    stored: MemoryHost,
    /// The mob beside the launch mob.
    pub(crate) mob: MobId,
}

impl HostOfARememberingMob {
    /// The fixture and its first launch: the mob is created by the host,
    /// loaded, and its agent has written its note.
    pub(crate) async fn launched() -> (Self, UnifiedRuntime) {
        let stored = MemoryHost::new();
        let host = stored.launch().await;
        let mob = hosted_mob(&host, definition_keeping_memory("second")).await;
        remembers(&host, Some(&mob), "of the second mob").await;
        (Self { stored, mob }, host)
    }

    /// `host` stops as a process ends, and a launch over the same state
    /// starts: the mob beside the launch mob is known and not loaded.
    pub(crate) async fn started_again(&self, host: UnifiedRuntime) -> UnifiedRuntime {
        stops(host).await;
        self.stored.launch().await
    }

    /// [`Self::started_again`], with `between` run once `host` has stopped
    /// and before the next launch starts: a row changes what is on disk
    /// there.
    pub(crate) async fn started_again_after(
        &self,
        host: UnifiedRuntime,
        between: impl FnOnce(),
    ) -> UnifiedRuntime {
        stops(host).await;
        between();
        self.stored.launch().await
    }

    /// Every file of the mob's own memory directory, with its bytes.
    pub(crate) fn memory_files_of_the_mob(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        files_below(&self.stored.dir_of(&self.mob))
    }

    /// The files of the mob's memory this process holds open now.
    pub(crate) fn memory_files_open_of_the_mob(&self) -> Vec<PathBuf> {
        open_below(&self.stored.dir_of(&self.mob))
    }

    /// The host's state directory.
    pub(crate) fn state_dir(&self) -> PathBuf {
        self.stored.stored.state_dir().to_path_buf()
    }

    /// The directory of the mob's own memory, by the store's path function.
    pub(crate) fn memory_dir_of_the_mob(&self) -> PathBuf {
        self.stored.dir_of(&self.mob)
    }

    /// Every file below the host's state directory whose path names the
    /// mob (its store is among them), with its bytes.
    pub(crate) fn state_files_of_the_mob(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        files_below(self.stored.stored.state_dir())
            .into_iter()
            .filter(|(path, _)| path.to_string_lossy().contains(self.mob.as_str()))
            .collect()
    }

    /// The host stops, the mob beside its launch mob with it.
    pub(crate) async fn ends(&self, host: UnifiedRuntime) {
        shut_down(&host, &[&self.mob]).await;
        drop(host);
    }
}
