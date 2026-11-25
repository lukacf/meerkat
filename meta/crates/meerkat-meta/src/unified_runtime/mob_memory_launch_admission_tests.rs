//! Rows of which sessions a launch's own memory engines read.
//!
//! A launch's session store holds the sessions of every mob the launch
//! builds, and its engines are given session ids by a model. A session is
//! the launch's when a journal binds it: the journal of the mob its stored
//! binding names, when that mob has its members in the launch's memory;
//! and, for a session whose stored binding names no mob, the launch mob's
//! own journal. A session no such journal bound is not held: it reads as a
//! session the store does not hold, and whether it is private to a person
//! is not known. No time is compared: a session of an earlier mob of an id
//! is told from the holder's own by the holder's journal, whatever the two
//! were stamped with.
//!
//! The fixture is the launch of the hosted mobs' memory rows; the sessions
//! are stored in a session store of the row's own, under the ids the mobs'
//! journals bound or under ids no journal bound.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_core::types::SessionId;
use meerkat_mob::{AgentIdentity, MobId, SpawnMemberSpec};

use super::mob_entries_tests::{set_of, shut_down};
use super::mob_memory_engines_tests::{ScriptedEngines, StoredEngines, engines_on, titles_in};
use super::mob_memory_firewall_tests::{OWNER, a_mob_created_by};
use super::mob_memory_tests::{DESK, definition_keeping_memory, launch_keeping_memory};
use super::{UnifiedRuntime, UnifiedRuntimeBuilderError, known};
use crate::memory::capabilities::StewardStore as _;
use crate::memory::distiller::TranscriptSource;

/// The name of the member a row seats in a mob a member created.
const HELPER: &str = "helper";

/// The name of a member a row seats private to a person.
const PRIVATE_HELPER: &str = "private-helper";

/// What a transcript source says of one session, word for word: whether a
/// read holds it, and its answer to whether it is private to a person. In
/// the words of a source that does not say, the session's own key is
/// replaced by a fixed word, so what it says of two sessions compares.
async fn said_of(
    source: &Arc<dyn TranscriptSource>,
    session: &str,
) -> (bool, Result<bool, String>) {
    let read = source
        .read(session, 0)
        .await
        .expect("the source reads")
        .is_some();
    let private = source
        .private_to_a_person(session)
        .await
        .map_err(|error| error.to_string().replace(session, "<the session>"));
    (read, private)
}

/// What a row expects of a source for one session: whether a read holds
/// it, and whether it is private to a person (`None`: not known).
type Answer = (bool, Option<bool>);

/// A session the source holds, private to no one.
const ITS_OWN: Answer = (true, Some(false));
/// A session the source does not hold: it reads as one the store does not
/// hold, and whether it is private to a person is not known.
const NOT_ITS_OWN: Answer = (false, None);

/// Ask `source` for every session of `table` (its key, what the row
/// expects, which session it is) and compare the answers whole, so one run
/// shows every line that differs.
///
/// The sessions the row expects not to be the source's own must also be
/// answered in the same words, one and the same value for each, and the
/// same as an id the store holds no session under, which is asked here
/// beside them: a session that is not held is then not told from another,
/// nor from one that does not exist, by what the source says of it.
async fn assert_answers(source: &Arc<dyn TranscriptSource>, table: &[(&str, Answer, &str)]) {
    let mut answered = Vec::new();
    let mut expected = Vec::new();
    let mut said_of_those_not_its_own = std::collections::BTreeSet::new();
    for (session, wished, which) in table {
        let (read, private) = said_of(source, session).await;
        if *wished == NOT_ITS_OWN {
            said_of_those_not_its_own.insert((read, private.clone()));
        }
        answered.push((*which, (read, private.ok())));
        expected.push((*which, *wished));
    }
    if !said_of_those_not_its_own.is_empty() {
        let stored_nowhere = SessionId::new().to_string();
        said_of_those_not_its_own.insert(said_of(source, &stored_nowhere).await);
    }
    let in_the_same_words = said_of_those_not_its_own.len() <= 1;
    assert_eq!(
        (answered, in_the_same_words),
        (expected, true),
        "what the source answers for each session, and whether it says the same of every \
         session that is not its own and of an id its store holds no session under (left), and \
         what it is to answer (right); said of those: {said_of_those_not_its_own:?}"
    );
}

/// A session stored in `sessions` with one message `said`, private to no
/// one, whose stored member binding names the member `bound_to` (or names
/// none): its key. With `id` it is stored under that id, the one a mob's
/// journal bound; without, under an id no journal bound.
async fn a_session_under(
    sessions: &Arc<meerkat::MemoryStore>,
    id: Option<SessionId>,
    bound_to: Option<(&MobId, &str)>,
    said: &str,
) -> String {
    a_session_stored(sessions, id, bound_to, None, said).await
}

/// [`a_session_under`] for a session whose stored labels say it is the
/// session of a member private to the person `private_to` (or to no one).
async fn a_session_stored(
    sessions: &Arc<meerkat::MemoryStore>,
    id: Option<SessionId>,
    bound_to: Option<(&MobId, &str)>,
    private_to: Option<&str>,
    said: &str,
) -> String {
    let mut session = id.map_or_else(meerkat_core::Session::new, meerkat_core::Session::with_id);
    let stored = session.set_session_metadata(meerkat_core::SessionMetadata {
        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
        model: "gpt-5.5".to_string(),
        model_fallback: None,
        max_tokens: 1024,
        structured_output_retries: meerkat_core::config::default_structured_output_retries(),
        provider: meerkat_core::Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        tooling: meerkat_core::SessionTooling::default(),
        keep_alive: false,
        comms_name: None,
        peer_meta: private_to.map(|person| {
            meerkat_core::PeerMeta::default().with_label(crate::access::PRIVATE_TO_LABEL, person)
        }),
        realm_id: None,
        instance_id: None,
        backend: None,
        config_generation: None,
        auth_binding: None,
        mob_member_binding: bound_to.map(|(mob, member)| meerkat_core::MobMemberBinding {
            mob_id: mob.to_string(),
            role: "worker".to_string(),
            member: member.to_string(),
        }),
    });
    assert!(
        stored.is_ok(),
        "the session's metadata is recorded: {stored:?}"
    );
    session.push(meerkat_core::Message::User(
        meerkat_core::UserMessage::text(said),
    ));
    let saved = meerkat::SessionStore::save(&**sessions, &session).await;
    assert!(saved.is_ok(), "the session is stored: {:?}", saved.err());
    session.id().to_string()
}

/// The session the mob of `handle` holds its member `member` bound to, as
/// the mob's own journal says it.
async fn session_in_the_journal_of(
    handle: &meerkat_mob::MobHandle,
    member: &AgentIdentity,
) -> SessionId {
    handle
        .last_member_binding(member)
        .await
        .expect("the mob's journal reads")
        .and_then(|binding| binding.session_id().cloned())
        .expect("the mob's journal bound the member to a session")
}

/// Seat the member [`HELPER`] in the mob `mob` the set serves: the session
/// that mob's journal bound it to.
async fn a_helper_seated_in(host: &UnifiedRuntime, mob: &MobId) -> SessionId {
    let handle = set_of(host)
        .handle_for(mob)
        .await
        .expect("the set serves the mob");
    let seated = Box::pin(handle.spawn_spec(SpawnMemberSpec::new("worker", HELPER))).await;
    assert!(
        seated.is_ok(),
        "the mob's member: {:?}",
        seated.as_ref().err()
    );
    session_in_the_journal_of(&handle, &AgentIdentity::from(HELPER)).await
}

/// The mob set destroys the mob `id`, and the runtime lets go of its entry.
async fn destroyed_by_the_set(host: &UnifiedRuntime, id: &MobId) {
    let destroyed = Box::pin(set_of(host).mob_destroy(id)).await;
    assert!(
        destroyed.is_ok(),
        "the mob set destroys the mob: {:?}",
        destroyed.as_ref().err()
    );
    let mut let_go = false;
    for _ in 0..200 {
        if !host.mob_entries_in_tests().contains(id) {
            let_go = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(let_go, "the runtime let go of the destroyed mob's entry");
}

/// The source a launch assembles its own memory engines with, over the
/// sessions of `sessions`, once the launch's record has read its mobs: a
/// row that asks it stands at the state after the record's read of the
/// mobs that are loaded.
async fn source_of_the_launch(
    host: &UnifiedRuntime,
    sessions: &Arc<meerkat::MemoryStore>,
) -> Arc<dyn TranscriptSource> {
    let owners = host.mob_runtime().private_owners().clone();
    let read = owners.read_loaded_mobs().await;
    assert!(read.is_ok(), "the record reads the loaded mobs: {read:?}");
    crate::memory_wiring::launch_transcript_source(
        Arc::clone(sessions) as Arc<dyn meerkat::SessionStore>,
        owners,
        host.mob_handle(),
    )
}

/// A session of the launch mob is the launch's when the launch mob's own
/// journal bound it. Asked of the source the launch's engines read
/// through: the session the journal holds the launch mob's agent bound to
/// is held; a session whose stored binding names the launch mob and that
/// the launch mob's journal never bound reads as a session the store does
/// not hold, and whether it is private to a person is not known.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_launchs_memory_engines_read_a_session_of_the_launch_mob_that_its_journal_bound_and_no_other()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    let sessions = Arc::new(meerkat::MemoryStore::new());

    let bound = a_session_under(
        &sessions,
        Some(
            session_in_the_journal_of(
                &kept.host.mob_handle(),
                &crate::member_comms_id::mob_member_id(DESK),
            )
            .await,
        ),
        Some((&launch_mob, DESK)),
        "said to the launch mob's desk",
    )
    .await;
    let never_bound = a_session_under(
        &sessions,
        None,
        Some((&launch_mob, DESK)),
        "said to a desk the launch mob's journal never bound",
    )
    .await;

    let source = source_of_the_launch(&kept.host, &sessions).await;
    assert_answers(
        &source,
        &[
            (
                bound.as_str(),
                ITS_OWN,
                "the launch mob's own, by its journal",
            ),
            (
                never_bound.as_str(),
                NOT_ITS_OWN,
                "one that names the launch mob and that its journal never bound",
            ),
        ],
    )
    .await;
    kept.shut_down().await;
}

/// A session with no stored binding is the launch's only when the launch
/// mob's own journal bound it. A session stored with no binding under the
/// id the launch mob's journal holds a member bound to is held. A session
/// with no stored binding that no journal bound is not held, and neither
/// is one that another mob's journal bound: the launch mob's journal alone
/// is asked for a session that names no mob, and whether such a session is
/// private to a person is not known. An id the store holds no session
/// under answers the same, so a session that is not held is not told from
/// one that does not exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_with_no_stored_binding_is_the_launchs_only_when_the_launch_mobs_journal_bound_it()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    let sessions = Arc::new(meerkat::MemoryStore::new());

    let of_a_launch_member = a_helper_seated_in(&kept.host, &launch_mob).await;
    let bound_by_the_launch_mob = a_session_under(
        &sessions,
        Some(of_a_launch_member),
        None,
        "said to a member the launch mob's journal bound",
    )
    .await;
    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of the launch has an identity plane"
    );
    let of_the_second_desk = session_in_the_journal_of(
        &kept.second_handle().await,
        &crate::member_comms_id::mob_member_id(DESK),
    )
    .await;
    let bound_by_another_mob = a_session_under(
        &sessions,
        Some(of_the_second_desk),
        None,
        "said to a member another mob's journal bound",
    )
    .await;
    let bound_by_no_mob = a_session_under(&sessions, None, None, "said to no member").await;
    let stored_nowhere = SessionId::new().to_string();

    let source = source_of_the_launch(&kept.host, &sessions).await;
    assert_answers(
        &source,
        &[
            (
                bound_by_the_launch_mob.as_str(),
                ITS_OWN,
                "no stored binding, bound by the launch mob's journal",
            ),
            (
                bound_by_another_mob.as_str(),
                NOT_ITS_OWN,
                "no stored binding, bound by another mob's journal",
            ),
            (
                bound_by_no_mob.as_str(),
                NOT_ITS_OWN,
                "no stored binding, bound by no journal",
            ),
            (
                stored_nowhere.as_str(),
                NOT_ITS_OWN,
                "an id the store holds no session under",
            ),
        ],
    )
    .await;
    kept.shut_down().await;
}

/// A session of a mob a launch mob's member created is the launch's when
/// that mob's own journal bound it. The session the created mob's journal
/// holds its member bound to is held; a session whose stored binding names
/// the created mob and that the mob's journal never bound is not, however
/// new it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_launchs_memory_engines_read_a_session_of_a_mob_its_member_created_that_the_mobs_journal_bound_and_no_other()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    let sessions = Arc::new(meerkat::MemoryStore::new());

    let created = a_mob_created_by(
        &kept.host,
        &launch_mob,
        "maker",
        definition_keeping_memory("created"),
    )
    .await;
    let of_the_helper = a_helper_seated_in(&kept.host, &created).await;
    let bound = a_session_under(
        &sessions,
        Some(of_the_helper),
        Some((&created, HELPER)),
        "said to the created mob's helper",
    )
    .await;
    let never_bound = a_session_under(
        &sessions,
        None,
        Some((&created, HELPER)),
        "said to a helper the created mob's journal never bound",
    )
    .await;

    let source = source_of_the_launch(&kept.host, &sessions).await;
    assert_answers(
        &source,
        &[
            (
                bound.as_str(),
                ITS_OWN,
                "the created mob's own, by its journal",
            ),
            (
                never_bound.as_str(),
                NOT_ITS_OWN,
                "one that names the created mob and that its journal never bound",
            ),
        ],
    )
    .await;
    shut_down(&kept.host, &[&created, &kept.second_id]).await;
}

/// A session of an earlier mob of an id is not the mob's that holds the id
/// now, whatever time it carries. A launch mob's member creates a mob, the
/// mob's journal binds its member to a session, the mob is destroyed and a
/// launch mob's member creates a mob under the same id. The earlier mob's
/// session, stored with a creation time that is not older than the later
/// mob (as on a host whose clock was set back in between), is not held:
/// the later mob's journal never bound it. The later mob reads none of the
/// earlier mob's journal: destroying a mob clears its journal, and a mob
/// created again under the id starts a new history (the mob crate's member
/// binding history says so of itself).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_of_an_earlier_mob_of_an_id_is_not_read_for_the_mob_that_holds_the_id_now_whatever_its_time()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    let sessions = Arc::new(meerkat::MemoryStore::new());

    let definition = definition_keeping_memory("held-twice");
    let id = definition.id.clone();
    let earlier =
        a_mob_created_by(&kept.host, &launch_mob, "first-maker", definition.clone()).await;
    assert_eq!(earlier, id, "the earlier mob holds the id");
    let of_the_earlier_helper = a_helper_seated_in(&kept.host, &earlier).await;
    destroyed_by_the_set(&kept.host, &earlier).await;
    let later = a_mob_created_by(&kept.host, &launch_mob, "second-maker", definition).await;
    assert_eq!(later, id, "the later mob holds the same id");

    // Stored now, after the later mob was created: its creation time is not
    // older than the mob that holds the id.
    let of_the_earlier_mob = a_session_under(
        &sessions,
        Some(of_the_earlier_helper),
        Some((&id, HELPER)),
        "said to the helper of the earlier mob of the id",
    )
    .await;

    let stored_nowhere = SessionId::new().to_string();

    let source = source_of_the_launch(&kept.host, &sessions).await;
    assert_answers(
        &source,
        &[
            (
                of_the_earlier_mob.as_str(),
                NOT_ITS_OWN,
                "bound by the earlier mob's journal, asked of the mob that holds the id now",
            ),
            (
                stored_nowhere.as_str(),
                NOT_ITS_OWN,
                "an id the store holds no session under",
            ),
        ],
    )
    .await;
    shut_down(&kept.host, &[&later, &kept.second_id]).await;
}

/// Whose memory a session's mob is in is asked before any journal. A
/// session whose stored binding names a mob whose members are not in the
/// launch's memory is not the launch's, although that mob's own journal
/// bound it: it reads as a session the store does not hold, and whether it
/// is private to a person is not known.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_whose_stored_binding_names_a_mob_outside_the_launchs_memory_is_not_the_launchs_whoever_bound_it()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    let sessions = Arc::new(meerkat::MemoryStore::new());

    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of the launch has an identity plane"
    );
    let of_the_second_desk = session_in_the_journal_of(
        &kept.second_handle().await,
        &crate::member_comms_id::mob_member_id(DESK),
    )
    .await;
    let named_and_bound_by_the_second = a_session_under(
        &sessions,
        Some(of_the_second_desk),
        Some((&kept.second_id, DESK)),
        "said to the second mob's desk",
    )
    .await;

    let stored_nowhere = SessionId::new().to_string();

    let source = source_of_the_launch(&kept.host, &sessions).await;
    assert_answers(
        &source,
        &[
            (
                named_and_bound_by_the_second.as_str(),
                NOT_ITS_OWN,
                "one that names the second mob and that the second mob's journal bound",
            ),
            (
                stored_nowhere.as_str(),
                NOT_ITS_OWN,
                "an id the store holds no session under",
            ),
        ],
    )
    .await;
    kept.shut_down().await;
}

/// A session is private to a person when the journal says so of the member
/// it bound the session to, or when the stored session says so of itself:
/// the stricter of the two. The journal answers for the member that held a
/// session last, and a stored session answers for the build that wrote it,
/// so neither alone carries the question.
///
/// The launch mob's journal binds a session to a member that is private to
/// a person, and the session is stored as a shared member's, with no
/// private owner: private. The journal binds another session to a shared
/// member, and its stored labels name a person: private. That second line
/// holds before the journal is asked at all (the stored session answers
/// it): it is the guard that the stored answer is kept, not a line that
/// waits for a change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_is_private_to_a_person_when_its_stored_labels_or_the_journals_member_say_so() {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    let sessions = Arc::new(meerkat::MemoryStore::new());

    let handle = set_of(&kept.host)
        .handle_for(&launch_mob)
        .await
        .expect("the set serves the launch mob");
    let seated = Box::pin(handle.spawn_spec(
        SpawnMemberSpec::new("worker", PRIVATE_HELPER).with_labels(BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            OWNER.to_string(),
        )])),
    ))
    .await;
    assert!(
        seated.is_ok(),
        "the member private to a person: {:?}",
        seated.as_ref().err()
    );
    let of_the_private_member =
        session_in_the_journal_of(&handle, &AgentIdentity::from(PRIVATE_HELPER)).await;
    let private_by_the_journal = a_session_under(
        &sessions,
        Some(of_the_private_member),
        Some((&launch_mob, DESK)),
        "said to a member private to a person",
    )
    .await;
    let of_the_shared_desk = session_in_the_journal_of(
        &kept.host.mob_handle(),
        &crate::member_comms_id::mob_member_id(DESK),
    )
    .await;
    let private_by_its_stored_labels = a_session_stored(
        &sessions,
        Some(of_the_shared_desk),
        Some((&launch_mob, DESK)),
        Some(OWNER),
        "said to a member the stored session says is private to a person",
    )
    .await;

    let source = source_of_the_launch(&kept.host, &sessions).await;
    assert_answers(
        &source,
        &[
            (
                private_by_the_journal.as_str(),
                (true, Some(true)),
                "bound by the journal to a member private to a person, stored as a shared member's",
            ),
            (
                private_by_its_stored_labels.as_str(),
                (true, Some(true)),
                "bound by the journal to a shared member, stored with labels that name a person",
            ),
        ],
    )
    .await;
    kept.shut_down().await;
}

/// The same for the memory of a mob the launch hosts beside its launch mob,
/// asked of the source that mob's engines read through. The second mob's
/// journal binds a session to a member that is private to a person, and
/// the session is stored as a shared member's: private. The journal binds
/// another session to the shared desk, and its stored labels name a
/// person: private (the guard, as for the launch).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hosted_mobs_session_is_private_to_a_person_when_its_stored_labels_or_the_journals_member_say_so()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    let sessions = Arc::new(meerkat::MemoryStore::new());

    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of the launch has an identity plane"
    );
    let second = set_of(&kept.host)
        .handle_for(&kept.second_id)
        .await
        .expect("the set serves the second mob");
    let seated = Box::pin(second.spawn_spec(
        SpawnMemberSpec::new("worker", PRIVATE_HELPER).with_labels(BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            OWNER.to_string(),
        )])),
    ))
    .await;
    assert!(
        seated.is_ok(),
        "the second mob's member private to a person: {:?}",
        seated.as_ref().err()
    );
    let of_the_private_member =
        session_in_the_journal_of(&second, &AgentIdentity::from(PRIVATE_HELPER)).await;
    let private_by_the_journal = a_session_under(
        &sessions,
        Some(of_the_private_member),
        Some((&kept.second_id, DESK)),
        "said to a member of the second mob private to a person",
    )
    .await;
    let of_the_shared_desk =
        session_in_the_journal_of(&second, &crate::member_comms_id::mob_member_id(DESK)).await;
    let private_by_its_stored_labels = a_session_stored(
        &sessions,
        Some(of_the_shared_desk),
        Some((&kept.second_id, DESK)),
        Some(OWNER),
        "said to a member of the second mob the stored session says is private to a person",
    )
    .await;

    let source = crate::memory_wiring::hosted_mob_transcript_source(
        Arc::clone(&sessions) as Arc<dyn meerkat::SessionStore>,
        kept.second_handle().await,
    );
    assert_answers(
        &source,
        &[
            (
                private_by_the_journal.as_str(),
                (true, Some(true)),
                "bound by the journal to a member private to a person, stored as a shared member's",
            ),
            (
                private_by_its_stored_labels.as_str(),
                (true, Some(true)),
                "bound by the journal to a shared member, stored with labels that name a person",
            ),
        ],
    )
    .await;
    kept.shut_down().await;
}

/// A mob a launch mob's member created that is not loaded is not asked: a
/// session its journal bound is not held while the mob is not loaded, and
/// whether it is private to a person is not known. The mob is not loaded
/// for the question. Loaded again, the mob's journal answers and the same
/// session is held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_of_a_member_created_mob_that_is_not_loaded_is_not_held_and_nothing_is_loaded() {
    let stored = StoredEngines::new();
    let launched = stored.launch().await;
    let launch_mob = launched.host.mob_handle().mob_id().clone();
    let sessions = Arc::new(meerkat::MemoryStore::new());

    let created = a_mob_created_by(
        &launched.host,
        &launch_mob,
        "maker",
        definition_keeping_memory("created"),
    )
    .await;
    let of_the_helper = a_helper_seated_in(&launched.host, &created).await;
    let session = a_session_under(
        &sessions,
        Some(of_the_helper),
        Some((&created, HELPER)),
        "said to the created mob's helper",
    )
    .await;
    let source = source_of_the_launch(&launched.host, &sessions).await;
    assert_answers(
        &source,
        &[(session.as_str(), ITS_OWN, "while its mob is loaded")],
    )
    .await;

    launched.unloads(&[&created]).await;
    assert_answers(
        &source,
        &[(session.as_str(), NOT_ITS_OWN, "while its mob is not loaded")],
    )
    .await;
    let known_now = known(&launched.host).await;
    assert!(
        known_now.not_loaded.contains(&created) && !known_now.loaded.contains(&created),
        "the question loaded no mob: the set manages {:?}",
        known_now.loaded
    );

    let loaded_again = set_of(&launched.host).handle_for(&created).await;
    assert!(
        loaded_again.is_ok(),
        "a request loads the mob: {:?}",
        loaded_again.as_ref().err()
    );
    assert_answers(
        &source,
        &[(session.as_str(), ITS_OWN, "once its mob is loaded again")],
    )
    .await;
    shut_down(&launched.host, &[&created]).await;
}

const HANDED_TO_THE_LAUNCH_DESK: &str = "Returns of the launch desk close on Fridays";

/// A compaction harvest for a session no journal of the launch's memory
/// bound distils nothing. A distiller that reads through the launch's
/// source is handed the discards of a compaction for two such sessions:
/// one whose stored binding names the launch mob, one of no member.
/// Whether such a session is private to a person is unknown, so neither is
/// distilled: the launch mob's agent gains no note, and nothing is set
/// aside for it.
///
/// A guard, not a road a launch takes: a compaction's discards are handed
/// to a launch's distiller by its identity runtime, for its own agents'
/// sessions, which the launch mob's journal bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_compaction_harvest_for_a_session_no_journal_of_the_launchs_memory_bound_distils_nothing()
{
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    let sessions = Arc::new(meerkat::MemoryStore::new());
    let naming_the_launch_mob = a_session_under(
        &sessions,
        None,
        Some((&launch_mob, DESK)),
        "said to a desk the launch mob's journal never bound",
    )
    .await;
    let of_no_member = a_session_under(&sessions, None, None, "said to no member").await;

    let scripted = ScriptedEngines::new();
    scripted.answers_arrays_with(&serde_json::json!([{
        "action": "remember",
        "kind": "fact",
        "title": HANDED_TO_THE_LAUNCH_DESK,
        "description": "When the launch desk's returns close.",
        "body": "The returns of the launch desk close on Fridays.",
        "tags": [],
        "epistemic": "observed",
    }]));
    let stack = crate::memory_wiring::assemble_memory_stack(
        Arc::new(kept.of_the_launch.clone()),
        &kept.config,
        &engines_on(),
        crate::memory_wiring::MemoryStackSeams {
            persistent_state: Some(temp.path().to_path_buf()),
            transcript_store: Some(Arc::clone(&sessions) as Arc<dyn meerkat::SessionStore>),
            event_sink: Some(kept.host.memory_event_sink()),
            ..Default::default()
        },
        None,
        crate::memory_wiring::MemoryStackOver {
            transcripts: Some(source_of_the_launch(&kept.host, &sessions).await),
            clients: Some(scripted.clients()),
            ..Default::default()
        },
    )
    .expect("a memory stack over the launch's source");
    let distiller = stack.distiller.clone().expect("the stack's distiller");
    let memory = crate::identity_first::agent_memory::AgentMemoryRuntimeInjector::new(
        stack.provider.clone(),
        kept.config.clone(),
    )
    .with_distiller(distiller);

    for session in [&naming_the_launch_mob, &of_no_member] {
        let harvested = memory
            .harvest_compaction_within_bound(
                &super::mob_memory_tests::desk(),
                session,
                Some(vec![crate::memory::distiller::DiscardEntry {
                    content: "The returns of the launch desk close on Fridays.".to_string(),
                    range: None,
                }]),
                false,
                None,
            )
            .await;
        assert!(
            matches!(
                &harvested,
                Err(crate::identity_first::agent_memory::DistillerRunError::Failed(why))
                    if why.contains("is not one a journal of the launch's memory bound")
            ),
            "the harvest answers that it harvested nothing, and why: {harvested:?}"
        );
    }

    assert!(
        titles_in(
            &kept.of_the_launch,
            &crate::memory::records::MemoryScope::Identity {
                realm: kept.config.realm.clone(),
                identity: DESK.to_string(),
            }
        )
        .await
        .is_empty(),
        "the launch mob's agent gains no note from a session no journal bound"
    );
    assert!(
        kept.of_the_launch
            .quarantined_records(&kept.config.realm, 16)
            .await
            .expect("the launch's notes set aside")
            .is_empty(),
        "and nothing is set aside for it"
    );
    kept.shut_down().await;
}

/// A launch whose own mob storage keeps no journal of its members' sessions
/// cannot say which sessions are its mobs' own, so it is refused its memory
/// engines when it starts, as a configuration that conflicts: a host built
/// with persistent state, the whole memory stack and a mob storage that
/// has no such journal answers a refusal that names both, and builds
/// nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launch_whose_mob_storage_keeps_no_journal_is_refused_its_memory_engines_at_its_start() {
    let temp = tempfile::tempdir().expect("tempdir");
    let engines = crate::memory_wiring::MemoryEnginesConfig {
        steward: crate::memory::steward::StewardConfig {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let built = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition_keeping_memory("no-journal"))
            .persistent_state(temp.path().join("state"))
            .mob_storage(
                meerkat_mob::MobStorage::with_events(Arc::new(
                    meerkat_mob::InMemoryMobEventStore::new(),
                )),
                crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            )
            .persistent_agent_memory_stack(
                crate::identity_first::AgentMemoryConfig::default(),
                engines,
            )
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await;
    let answered = match built {
        Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(message)) => {
            format!("refused as a conflicting configuration: {message}")
        }
        Err(other) => format!("refused otherwise: {other}"),
        Ok(runtime) => {
            let _stopped =
                tokio::time::timeout(std::time::Duration::from_secs(30), runtime.shutdown()).await;
            "built".to_string()
        }
    };
    assert!(
        answered.starts_with("refused as a conflicting configuration: ")
            && answered.contains("mob_storage()")
            && answered.contains("persistent_agent_memory_stack()"),
        "the start refuses the composition and names the mob storage and the memory stack: \
         {answered}"
    );
}
