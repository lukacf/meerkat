//! A temporary council participant is never a console member.
//!
//! A participant is a fork of its source member: its session begins with a
//! copy of the source's earlier turns. A console whose visibility policy
//! includes nested members listed it like any other member of a mob the
//! runtime manages, decided on its own name, its council role and the labels
//! it inherits, so a caller the access rules deny the source (by identity or
//! by role) read the source's turns through the participant.
//!
//! These rows run a real council over this module's fixture (the agent
//! `council` tool, a scripted model that holds every participant turn) under
//! a console that includes nested members, with a deny on the source member:
//! the participant is not listed, cannot be reached by name, gets no history
//! backfill, and frames a console stored under its name are not served. The
//! source, other nested members and a console that hides nested members read
//! as before, and none of these reads wakes a dormant member.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::access::{AccessControlConfig, AccessController, AccessEffect, AccessRule};
use crate::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, ConsoleFrame, ConsoleFrameLineageLink,
    ConsoleFrameMemberProvenance, ConsoleFrameSource, ConsoleFrameSourceKind, ConsoleFrameStatus,
    ConsoleIdentityRecord, ConsoleSendError, ConsoleSendRequest, ConsoleTimelineEvent,
    ConsoleTimelineMode, ConsoleTimelineQuery, ConsoleTimelineWindowQuery, ConsoleVisibility,
    ConsoleVisibilityPolicy, HideImplicitDelegateMembersConsoleVisibilityPolicy,
    MetaConsoleAggregator, NewConsoleFrame, TimelineFrameDecider, TimelineFrameFilter,
};
use crate::runtime::ConsoleMember;
use crate::unified_runtime::ConsoleEventStore;

const RUNTIME: &str = "council-console";
const ROOT: &str = "root@example.test";
const CALLER: &str = "caller@example.test";
/// The source member, and the role it is seated under.
const SOURCE: &str = "alice";
const SOURCE_ROLE: &str = "general";
/// The source member's earlier turn: what a caller denied the source must
/// never read.
const SOURCE_TURN: &str = "META-SOURCE-EARLIER-TURN";
/// The seats the agent `council` tool names: the source's fork first.
const PARTICIPANTS: [&str; 2] = ["council-p0", "council-p1"];
/// A member of an ordinary nested mob.
const NESTED_MEMBER: &str = "nested-worker";
/// A member of an ordinary nested mob whose id has a council mob's form.
const LOOKALIKE_MEMBER: &str = "lookalike-worker";
/// A member that takes no part in the council.
const BYSTANDER: &str = "bystander";
/// A member of an ordinary nested mob created under the mob id of a council
/// that settled.
const REUSE_MEMBER: &str = "reuse-worker";

fn everyone_views() -> AccessRule {
    AccessRule {
        id: "everyone-views".to_string(),
        actions: vec!["agent.view".to_string()],
        agents: vec!["*".to_string()],
        ..AccessRule::default()
    }
}

fn deny_source_by_identity() -> AccessRule {
    AccessRule {
        id: "deny-source-identity".to_string(),
        effect: AccessEffect::Deny,
        actions: vec!["agent.view".to_string()],
        agents: vec![SOURCE.to_string()],
        ..AccessRule::default()
    }
}

fn deny_source_by_role() -> AccessRule {
    AccessRule {
        id: "deny-source-role".to_string(),
        effect: AccessEffect::Deny,
        actions: vec!["agent.view".to_string()],
        roles: vec![SOURCE_ROLE.to_string()],
        ..AccessRule::default()
    }
}

fn controller(deny: AccessRule) -> AccessController {
    AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![everyone_views(), deny],
        ..AccessControlConfig::default()
    })
    .expect("controller")
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

/// A console over the fixture's runtime under `policy`.
fn console(
    fixture: &CompletionFixture,
    policy: Arc<dyn ConsoleVisibilityPolicy>,
) -> MetaConsoleAggregator {
    let aggregator = MetaConsoleAggregator::in_memory();
    register(&aggregator, &fixture.runtime, policy);
    aggregator
}

/// Register `runtime` with `aggregator` under `policy`.
fn register(
    aggregator: &MetaConsoleAggregator,
    runtime: &MobRuntime,
    policy: Arc<dyn ConsoleVisibilityPolicy>,
) {
    aggregator.register_runtime_handles_with_policy(
        RUNTIME,
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        policy,
    );
}

fn mob_tools(fixture: &CompletionFixture) -> Arc<meerkat_mob_mcp::MobMcpState> {
    fixture
        .runtime
        .agent_mob_mcp_state()
        .expect("the fixture installs the agent mob tools")
}

/// Run one turn of the primary mob's `member` that carries `text`, to its
/// end.
async fn turn(fixture: &CompletionFixture, member: &str, text: &str) {
    turn_on(fixture, &fixture.runtime.handle(), member, text).await;
}

/// Run one turn of `handle`'s `member` that carries `text`, until the
/// member's session holds the reply to it.
async fn turn_on(
    fixture: &CompletionFixture,
    handle: &meerkat_mob::MobHandle,
    member: &str,
    text: &str,
) {
    let identity = AgentIdentity::from(member);
    let session = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("member session");
    handle
        .member(&identity)
        .await
        .expect("member")
        .internal_turn(meerkat_core::ContentInput::Text(text.to_string()))
        .await
        .expect("turn admitted");
    tokio::time::timeout(WAIT, async {
        loop {
            let messages = fixture.messages(&session).await;
            let asked = messages.iter().position(|message| {
                matches!(message, Message::User(user) if user.text_content().contains(text))
            });
            if asked.is_some_and(|asked| {
                messages[asked..]
                    .iter()
                    .any(|message| matches!(message, Message::BlockAssistant(_)))
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the turn's reply is recorded");
}

/// An ordinary nested mob `mob_id` with one member, as an agent's
/// `mob_create` leaves it: managed by the runtime's mob tools `state`.
async fn seat_nested_mob(
    state: &Arc<meerkat_mob_mcp::MobMcpState>,
    mob_id: &str,
    member: &str,
) -> SessionId {
    let mob_id = tokio::time::timeout(WAIT, state.mob_create_definition(definition(mob_id)))
        .await
        .expect("bounded nested mob create")
        .expect("nested mob");
    let handle = state.handle_for(&mob_id).await.expect("nested mob handle");
    let mut spawn = SpawnMemberSpec::new("general", member);
    spawn.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    tokio::time::timeout(WAIT, handle.spawn_spec(spawn))
        .await
        .expect("bounded nested member spawn")
        .expect("nested member");
    handle
        .resolve_bridge_session_id(&AgentIdentity::from(member))
        .await
        .expect("nested member session")
}

/// A council over the source and `bob`, seated and held at its first
/// participant turn.
struct Council {
    id: String,
    job_id: String,
    mob: String,
}

impl Council {
    async fn convene(fixture: &CompletionFixture, gate: &TurnGate) -> Self {
        let surface = fixture.surface("convener", true).await;
        let mob_id = fixture.runtime.handle().mob_id().to_string();
        let started = start_detached(
            &surface,
            "council",
            json!({
                "topic": "Should the console list a council seat?",
                "participants": [
                    {"mob_id": mob_id, "member_id": SOURCE, "role": "analyst"},
                    {"mob_id": mob_id, "member_id": "bob", "role": "critic"},
                ],
                "max_rounds": 1, "timeout_seconds": 120,
            }),
        )
        .await;
        let id = started["council_id"]
            .as_str()
            .expect("council identity")
            .to_string();
        let job_id = started["job_id"]
            .as_str()
            .expect("council job id")
            .to_string();
        fixture
            .wait_for_council_participant(gate, &id, &job_id)
            .await;
        let mob = meerkat_mob::temporary_council::TemporaryCouncilId::new(&id)
            .expect("council identity")
            .temporary_mob_id()
            .to_string();
        Self { id, job_id, mob }
    }

    /// The session the participant forked from the source runs on.
    async fn source_fork_session(&self, fixture: &CompletionFixture) -> String {
        let council_id = meerkat_mob::temporary_council::TemporaryCouncilId::new(&self.id)
            .expect("council identity");
        mob_tools(fixture)
            .temporary_council_store_for_tests()
            .load(&council_id)
            .await
            .expect("read council custody")
            .expect("council record")
            .participants
            .iter()
            .find(|participant| participant.source_identity.as_str() == SOURCE)
            .and_then(|participant| participant.seated_session_id.clone())
            .expect("the source's fork is seated")
            .to_string()
    }

    /// Let the council run to its end: its result reaches the convener and
    /// its temporary mob is destroyed.
    async fn end(&self, fixture: &CompletionFixture, gate: &TurnGate) {
        gate.release();
        fixture
            .assert_delivered("convener", &self.job_id, COUNCIL_SUMMARY)
            .await;
        let state = mob_tools(fixture);
        let mob = meerkat_mob::MobId::from(self.mob.as_str());
        tokio::time::timeout(WAIT, async {
            while state.handle_for(&mob).await.is_ok() {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("the council's temporary mob is destroyed");
    }

    /// Wait for the council's record to settle: its cleanup destroyed the
    /// temporary mob and owes nothing more. The record itself stays.
    async fn settled(&self, fixture: &CompletionFixture) {
        let council_id = meerkat_mob::temporary_council::TemporaryCouncilId::new(&self.id)
            .expect("council identity");
        let store = mob_tools(fixture).temporary_council_store_for_tests();
        tokio::time::timeout(WAIT, async {
            loop {
                let record = store
                    .load(&council_id)
                    .await
                    .expect("read council custody")
                    .expect("council record");
                if !record.is_unfinished() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("the council's record settles");
    }
}

/// The fixture with the source's earlier turn recorded, an ordinary nested
/// mob, and a nested mob whose id has a council mob's form.
struct Realm {
    fixture: CompletionFixture,
    gate: Arc<TurnGate>,
    _release: ReleaseOnDrop,
    nested_mob: String,
    nested_session: SessionId,
    lookalike_mob: String,
    lookalike_session: SessionId,
}

impl Realm {
    async fn start() -> Self {
        let gate = TurnGate::new();
        let release = ReleaseOnDrop(Arc::clone(&gate));
        let fixture =
            CompletionFixture::start(Arc::clone(&gate), &["convener", SOURCE, "bob", BYSTANDER])
                .await;
        turn(&fixture, SOURCE, SOURCE_TURN).await;
        let state = mob_tools(&fixture);
        let nested_mob = unique("nested");
        let nested_session = seat_nested_mob(&state, &nested_mob, NESTED_MEMBER).await;
        let lookalike_mob = format!(
            "{}{}",
            meerkat_mob::temporary_council::TEMPORARY_COUNCIL_MOB_ID_PREFIX,
            unique("lookalike")
        );
        let lookalike_session = seat_nested_mob(&state, &lookalike_mob, LOOKALIKE_MEMBER).await;
        Self {
            fixture,
            gate,
            _release: release,
            nested_mob,
            nested_session,
            lookalike_mob,
            lookalike_session,
        }
    }

    fn names(&self) -> Vec<&'static str> {
        let mut names = vec!["convener", SOURCE, "bob", BYSTANDER];
        names.extend(PARTICIPANTS);
        names.extend([NESTED_MEMBER, LOOKALIKE_MEMBER]);
        names
    }
}

/// Read every member's history into the console.
async fn backfill(aggregator: &MetaConsoleAggregator) {
    tokio::time::timeout(WAIT, aggregator.refresh_session_history())
        .await
        .expect("bounded history backfill")
        .expect("history backfill");
}

/// Every frame the console's store holds.
async fn stored(aggregator: &MetaConsoleAggregator) -> Vec<ConsoleFrame> {
    let mut frames = Vec::new();
    let mut after = None;
    loop {
        let page = aggregator
            .store()
            .query_frames(ConsoleTimelineQuery {
                after: after.clone(),
                limit: 500,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("stored frames");
        let Some(last) = page.frames.last() else {
            return frames;
        };
        after = Some(last.cursor.clone());
        frames.extend(page.frames);
    }
}

fn carries(frame: &ConsoleFrame, text: &str) -> bool {
    serde_json::to_string(frame)
        .expect("frame encodes")
        .contains(text)
}

/// Every frame the console serves `subject` through a timeline or history
/// read: the window (which the RPC page runs too), unscoped and scoped to
/// each of `names`; the stream's snapshot page over the same queries, each
/// stored frame decided again before it is sent; and the live stream's gate
/// over every stored frame, for an unscoped subscriber and for one scoped to
/// the frame's own identity.
async fn served(
    aggregator: &MetaConsoleAggregator,
    controller: &AccessController,
    subject: Option<&str>,
    names: &[&str],
) -> Vec<ConsoleFrame> {
    served_to(aggregator, controller.view_for_subject(subject), names).await
}

/// [`served`] for a caller's own access view.
async fn served_to(
    aggregator: &MetaConsoleAggregator,
    view: crate::access::AccessView,
    names: &[&str],
) -> Vec<ConsoleFrame> {
    let decider = TimelineFrameDecider::new(view);
    let filter: &TimelineFrameFilter<'_> = &|frame: &ConsoleFrame| decider.visible(frame);
    let mut frames = Vec::new();
    for identity in std::iter::once(None).chain(names.iter().copied().map(Some)) {
        let query = ConsoleTimelineWindowQuery {
            identity: identity.map(str::to_string),
            mode: ConsoleTimelineMode::Recent,
            limit: 400,
            ..ConsoleTimelineWindowQuery::default()
        };
        frames.extend(
            aggregator
                .query_timeline_windowed_for(query.clone(), Some(filter))
                .await
                .expect("window")
                .frames,
        );
        for frame in aggregator
            .query_timeline_windowed_stored_for(query, Some(filter))
            .await
            .expect("stored window")
            .frames
        {
            if aggregator
                .timeline_frame_visible_for_query(&frame, identity)
                .await
                && decider.visible(&frame)
            {
                frames.push(frame);
            }
        }
    }
    for frame in stored(aggregator).await {
        if !decider.visible(&frame) {
            continue;
        }
        let event = ConsoleTimelineEvent::ConsoleFrame {
            frame: frame.clone(),
        };
        if aggregator
            .timeline_event_visible_for_subscriber(&event, None)
            .await
            || aggregator
                .timeline_event_visible_for_subscriber(&event, Some(frame.identity.as_str()))
                .await
        {
            frames.push(frame);
        }
    }
    frames
}

/// The source's earlier turn reaches no caller the rules deny the source,
/// and the administrator still reads it under the source's own name.
async fn assert_source_turn_denied(
    aggregator: &MetaConsoleAggregator,
    names: &[&str],
    when: &str,
    wrong: &mut Vec<String>,
) {
    for (rule, deny) in [
        ("identity", deny_source_by_identity()),
        ("role", deny_source_by_role()),
    ] {
        let controller = controller(deny);
        let leaked = served(aggregator, &controller, Some(CALLER), names)
            .await
            .into_iter()
            .filter(|frame| carries(frame, SOURCE_TURN))
            .map(|frame| frame.identity)
            .collect::<BTreeSet<_>>();
        if !leaked.is_empty() {
            wrong.push(format!(
                "{when}: a caller denied the source by {rule} reads the source's earlier turn \
                 in frames of {leaked:?}"
            ));
        }
        let administrator = served(aggregator, &controller, Some(ROOT), names)
            .await
            .into_iter()
            .filter(|frame| carries(frame, SOURCE_TURN))
            .map(|frame| frame.identity)
            .collect::<BTreeSet<_>>();
        assert!(
            administrator.contains(SOURCE),
            "{when}: the administrator reads the source's turn under the source's own name \
             ({rule} rule): {administrator:?}"
        );
    }
}

fn send_request(identity: &str) -> ConsoleSendRequest {
    ConsoleSendRequest {
        identity: identity.to_string(),
        content: json!("a console message for a council seat"),
        origin: "console".to_string(),
        idempotency_key: unique("council-seat-send"),
        handling_mode: None,
        origin_kind: None,
        skill_refs: Vec::new(),
    }
}

/// The runtime member ids an identities read lists.
async fn listed(aggregator: &MetaConsoleAggregator) -> BTreeSet<String> {
    aggregator
        .list_identities_fresh()
        .await
        .expect("identities")
        .into_iter()
        .map(|record| record.runtime_member_id)
        .collect()
}

/// A participant is absent from an identities read and unknown to every
/// member action by its name: inspect, the timeline, blobs, send and retire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_council_participant_is_not_listed_and_unknown_by_name() {
    let realm = Realm::start().await;
    let aggregator = console(&realm.fixture, Arc::new(AllowAllConsoleVisibilityPolicy));
    let council = Council::convene(&realm.fixture, &realm.gate).await;
    backfill(&aggregator).await;
    let mut wrong = Vec::new();

    let identities = aggregator
        .list_identities_fresh()
        .await
        .expect("identities");
    for record in &identities {
        if PARTICIPANTS.contains(&record.runtime_member_id.as_str())
            || record.labels.get("source_mob_id") == Some(&council.mob)
        {
            wrong.push(format!(
                "identities lists the council participant {} of {}",
                record.runtime_member_id, council.mob
            ));
        }
    }
    for participant in PARTICIPANTS {
        if aggregator
            .inspect_identity(participant)
            .await
            .expect("inspect")
            .is_some()
        {
            wrong.push(format!("inspect resolves {participant}"));
        }
        let timeline = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                identity: Some(participant.to_string()),
                mode: ConsoleTimelineMode::Recent,
                limit: 400,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("timeline");
        if !timeline.frames.is_empty() {
            wrong.push(format!(
                "the timeline of {participant} serves {} frames",
                timeline.frames.len()
            ));
        }
        match aggregator.binary_blob_store_for_identity(participant).await {
            Err(ConsoleSendError::UnknownIdentity(_)) => {}
            other => wrong.push(format!(
                "blobs resolve {participant}: {:?}",
                other.map(|store| store.is_some())
            )),
        }
    }
    // The writes last: on a console that resolves the participant they reach
    // the council itself.
    for participant in PARTICIPANTS {
        match aggregator.send(send_request(participant)).await {
            Err(ConsoleSendError::UnknownIdentity(_)) => {}
            other => wrong.push(format!(
                "send resolves {participant}: {:?}",
                other.map(|_| "accepted")
            )),
        }
        match aggregator.retire_identity(participant).await {
            Ok(false) => {}
            other => wrong.push(format!(
                "retire resolves {participant}: {:?}",
                other.map_err(|error| error.to_string())
            )),
        }
    }
    assert!(
        wrong.is_empty(),
        "a council participant is a console member: {wrong:#?}"
    );

    // Nothing the console did reached the council: it runs to its end.
    council.end(&realm.fixture, &realm.gate).await;
    assert!(
        listed(&aggregator)
            .await
            .is_disjoint(&PARTICIPANTS.map(str::to_string).into())
    );
    realm.fixture.stop().await;
}

/// A caller the rules deny the source, by identity or by role, reads the
/// source's earlier turn through no timeline or history read, while the
/// council runs and after it ended; and the console stores no history of a
/// participant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caller_denied_the_source_never_reads_its_turns_through_a_participant() {
    let realm = Realm::start().await;
    let names = realm.names();
    let aggregator = console(&realm.fixture, Arc::new(AllowAllConsoleVisibilityPolicy));
    let council = Council::convene(&realm.fixture, &realm.gate).await;
    let mut wrong = Vec::new();

    backfill(&aggregator).await;
    // An identities read asks for each listed member's history too.
    let _ = listed(&aggregator).await;
    backfill(&aggregator).await;
    assert_source_turn_denied(&aggregator, &names, "while the council runs", &mut wrong).await;

    council.end(&realm.fixture, &realm.gate).await;
    backfill(&aggregator).await;
    assert_source_turn_denied(&aggregator, &names, "after the council ended", &mut wrong).await;

    let backfilled = stored(&aggregator)
        .await
        .into_iter()
        .filter(|frame| {
            PARTICIPANTS.contains(&frame.identity.as_str())
                && frame.source.kind == ConsoleFrameSourceKind::SessionHistory
        })
        .count();
    if backfilled > 0 {
        wrong.push(format!(
            "the console stored {backfilled} history frames of a council participant"
        ));
    }
    assert!(
        wrong.is_empty(),
        "the source's turns are read through its council participant: {wrong:#?}"
    );
    realm.fixture.stop().await;
}

fn captured(
    identity: &str,
    session: &str,
    role: &str,
    primary_mob: &str,
    source_mob: &str,
) -> ConsoleFrameMemberProvenance {
    let record = ConsoleIdentityRecord {
        identity: identity.to_string(),
        display_name: identity.to_string(),
        runtime_key: RUNTIME.to_string(),
        runtime_member_id: identity.to_string(),
        session_id: Some(session.to_string()),
        member_id: None,
        visibility: ConsoleVisibility::Addressable,
        addressable: true,
        health: "ready".to_string(),
        topology_peers: Vec::new(),
        labels: BTreeMap::from([
            ("role".to_string(), role.to_string()),
            ("source_mob_id".to_string(), source_mob.to_string()),
        ]),
    };
    ConsoleFrameMemberProvenance {
        member: ConsoleMember {
            agent_identity: identity.to_string(),
            role: role.to_string(),
            state: "active".to_string(),
            model_capabilities: Default::default(),
            runtime_mode: None,
            session_id: record.session_id.clone(),
            wired_to: Vec::new(),
            labels: record.labels.clone(),
            progress: None,
        },
        primary_mob_id: primary_mob.to_string(),
        source_mob_id: source_mob.to_string(),
        lineage: vec![ConsoleFrameLineageLink {
            identity: identity.to_string(),
            agent_id: Some(identity.to_string()),
            role: Some(role.to_string()),
            labels: Some(record.labels.clone()),
        }],
        identity: record,
    }
}

/// A history frame as a console stored it for a nested member: `key` names
/// it, and its text is `text`. Its time is long past.
fn stored_frame(
    key: &str,
    text: &str,
    provenance: ConsoleFrameMemberProvenance,
) -> NewConsoleFrame {
    stored_frame_at(key, text, provenance, 1)
}

/// The wall clock, as a frame's time.
fn frame_time_now() -> u64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the epoch");
    u64::try_from(since_epoch.as_millis()).expect("a frame time")
}

/// [`stored_frame`] stored at `timestamp_ms`.
fn stored_frame_at(
    key: &str,
    text: &str,
    provenance: ConsoleFrameMemberProvenance,
    timestamp_ms: u64,
) -> NewConsoleFrame {
    let identity = provenance.identity.identity.clone();
    let session = provenance.identity.session_id.clone();
    NewConsoleFrame {
        id: None,
        dedupe_key: key.to_string(),
        timestamp_ms,
        runtime_key: RUNTIME.to_string(),
        identity: identity.clone(),
        conversation_id: Some(identity),
        session_id: session,
        kind: "text_complete".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({ "text": text, "seeded": key }),
        source: ConsoleFrameSource {
            member_provenance: Some(provenance),
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

/// The seeded frames `subject` is served.
async fn served_seeds(
    aggregator: &MetaConsoleAggregator,
    controller: &AccessController,
    subject: &str,
    names: &[&str],
) -> BTreeSet<String> {
    served(aggregator, controller, Some(subject), names)
        .await
        .into_iter()
        .filter_map(|frame| frame.payload.get("seeded")?.as_str().map(str::to_string))
        .collect()
}

/// Frames a console stored under a participant's name before it stopped
/// listing participants are not served, whoever asks: with the council's
/// record present, and with the record lost and the mob gone. A frame of an
/// ordinary nested mob is served as before: one the runtime holds whose id
/// merely has a council mob's form, one it holds under any other id, and one
/// it no longer holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frames_stored_for_a_council_participant_are_not_served() {
    let realm = Realm::start().await;
    let council = Council::convene(&realm.fixture, &realm.gate).await;
    let fork_session = council.source_fork_session(&realm.fixture).await;
    council.end(&realm.fixture, &realm.gate).await;
    council.settled(&realm.fixture).await;
    let primary = realm.fixture.runtime.handle().mob_id().to_string();
    let lost_mob = format!(
        "{}{}",
        meerkat_mob::temporary_council::TEMPORARY_COUNCIL_MOB_ID_PREFIX,
        unique("lost-record")
    );
    let gone_mob = unique("nested-gone");

    // What a console held on disk before the fix, then the runtime
    // registered over it.
    let aggregator = MetaConsoleAggregator::in_memory();
    let seeds = [
        (
            "council-record-present",
            captured(
                PARTICIPANTS[0],
                &fork_session,
                "council_p0",
                &primary,
                &council.mob,
            ),
        ),
        (
            "council-record-lost",
            captured(
                PARTICIPANTS[0],
                "lost-council-session",
                "council_p0",
                &primary,
                &lost_mob,
            ),
        ),
        (
            "lookalike-live",
            captured(
                LOOKALIKE_MEMBER,
                &realm.lookalike_session.to_string(),
                "general",
                &primary,
                &realm.lookalike_mob,
            ),
        ),
        (
            "nested-live",
            captured(
                NESTED_MEMBER,
                &realm.nested_session.to_string(),
                "general",
                &primary,
                &realm.nested_mob,
            ),
        ),
        (
            "nested-gone",
            captured(
                "gone-worker",
                "gone-worker-session",
                "general",
                &primary,
                &gone_mob,
            ),
        ),
    ];
    for (key, provenance) in seeds {
        aggregator
            .store()
            .append_if_absent(stored_frame(key, SOURCE_TURN, provenance))
            .await
            .expect("seeded frame");
    }
    // A frame under the settled council's mob id that no seat wrote and that
    // is newer than the council: the runtime holds no mob under the id, so
    // it is the council's too.
    aggregator
        .store()
        .append_if_absent(stored_frame_at(
            "council-record-present-not-a-seat",
            SOURCE_TURN,
            captured(
                "council-helper",
                "council-helper-session",
                "general",
                &primary,
                &council.mob,
            ),
            frame_time_now(),
        ))
        .await
        .expect("seeded frame");
    register(
        &aggregator,
        &realm.fixture.runtime,
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let _ = listed(&aggregator).await;

    let mut names = realm.names();
    names.extend(["gone-worker", "council-helper"]);
    let ordinary = BTreeSet::from([
        "lookalike-live".to_string(),
        "nested-live".to_string(),
        "nested-gone".to_string(),
    ]);
    for deny in [deny_source_by_identity(), deny_source_by_role()] {
        let by_role = !deny.roles.is_empty();
        let controller = controller(deny);
        assert_eq!(
            served_seeds(&aggregator, &controller, ROOT, &names).await,
            ordinary,
            "the administrator reads the ordinary nested members' frames and no council \
             participant's"
        );
        // The role rule denies every `general` member, the nested ones
        // included: the caller then reads none of the seeded frames.
        let expected = if by_role {
            BTreeSet::new()
        } else {
            ordinary.clone()
        };
        assert_eq!(
            served_seeds(&aggregator, &controller, CALLER, &names).await,
            expected,
            "a caller denied the source reads no council participant's stored frame"
        );
    }
    realm.fixture.stop().await;
}

/// Frames a console stored under a participant's name are not served once no
/// runtime is registered either. A console keeps the frames it stored
/// readable after its last runtime is unregistered (its host shut down);
/// a participant's stay unserved then, whoever asks: with the council's
/// record present before, and with the record lost. With no runtime
/// registered no policy says whether nested members are shown, so the
/// console serves no stored frame of a nested mob's member then, a council's
/// or not; a frame of the runtime's own mob is served, also when that mob's
/// id has a council mob's form.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frames_stored_for_a_council_participant_are_not_served_with_no_runtime_registered() {
    let realm = Realm::start().await;
    let council = Council::convene(&realm.fixture, &realm.gate).await;
    let fork_session = council.source_fork_session(&realm.fixture).await;
    council.end(&realm.fixture, &realm.gate).await;
    let primary = realm.fixture.runtime.handle().mob_id().to_string();
    let gone_mob = unique("nested-gone");
    let form_primary = lost_council_mob();

    let aggregator = MetaConsoleAggregator::in_memory();
    let seeds = [
        (
            "council-record-present",
            captured(
                PARTICIPANTS[0],
                &fork_session,
                "council_p0",
                &primary,
                &council.mob,
            ),
        ),
        (
            "council-record-lost",
            captured(
                PARTICIPANTS[0],
                "lost-council-session",
                "council_p0",
                &primary,
                &lost_council_mob(),
            ),
        ),
        (
            "lookalike-live",
            captured(
                LOOKALIKE_MEMBER,
                &realm.lookalike_session.to_string(),
                "general",
                &primary,
                &realm.lookalike_mob,
            ),
        ),
        (
            "nested-live",
            captured(
                NESTED_MEMBER,
                &realm.nested_session.to_string(),
                "general",
                &primary,
                &realm.nested_mob,
            ),
        ),
        (
            "nested-gone",
            captured(
                "gone-worker",
                "gone-worker-session",
                "general",
                &primary,
                &gone_mob,
            ),
        ),
        // A frame whose runtime's own primary mob had an id of the council
        // form: with no runtime registered a frame of the runtime's own mob
        // is served, whatever the form of its id.
        (
            "primary-with-council-form",
            captured(
                "primary-worker",
                "primary-worker-session",
                "general",
                &form_primary,
                &form_primary,
            ),
        ),
    ];
    for (key, provenance) in seeds {
        aggregator
            .store()
            .append_if_absent(stored_frame(key, SOURCE_TURN, provenance))
            .await
            .expect("seeded frame");
    }
    register(
        &aggregator,
        &realm.fixture.runtime,
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let _ = listed(&aggregator).await;

    let mut names = realm.names();
    names.extend(["gone-worker", "primary-worker"]);
    // While the runtime is registered the ordinary frames are served, so the
    // reads below are of frames a console does serve.
    assert_eq!(
        served_seeds(
            &aggregator,
            &controller(deny_source_by_identity()),
            ROOT,
            &names
        )
        .await,
        BTreeSet::from([
            "lookalike-live".to_string(),
            "nested-live".to_string(),
            "nested-gone".to_string(),
        ]),
        "with the runtime registered the administrator reads the ordinary nested members' frames"
    );

    aggregator.unregister_runtime(RUNTIME);
    // Only the runtime's own mob is served in this state: the two ordinary
    // nested mobs' frames are unserved with the participants' frames.
    let own_mob = BTreeSet::from(["primary-with-council-form".to_string()]);
    for deny in [deny_source_by_identity(), deny_source_by_role()] {
        let by_role = !deny.roles.is_empty();
        let controller = controller(deny);
        assert_eq!(
            served_seeds(&aggregator, &controller, ROOT, &names).await,
            own_mob,
            "with no runtime registered the administrator reads no council participant's stored \
             frame, and no other nested mob's"
        );
        // The role rule denies every `general` member, the one of the
        // runtime's own mob included: the caller then reads none of the
        // seeded frames.
        let expected = if by_role {
            BTreeSet::new()
        } else {
            own_mob.clone()
        };
        assert_eq!(
            served_seeds(&aggregator, &controller, CALLER, &names).await,
            expected,
            "with no runtime registered a caller denied the source reads no council \
             participant's stored frame"
        );
    }
    realm.fixture.stop().await;
}

/// A council's record outlives the council (it answers a replay of the same
/// request), and a mob id is free again once its mob is destroyed. An
/// ordinary nested mob created under the mob id of a council that settled is
/// no council's: its member is listed and resolved by name, its history is
/// read, and its frames are served, a stored one included. The frames a
/// console stored for the settled council's own seats stay unserved, whoever
/// asks: by the seat's fork session and by the seat's name. So does every
/// frame stored under the id before the council settled, whoever wrote it:
/// the mob the runtime holds now did not exist then.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ordinary_mob_under_a_settled_councils_mob_id_is_listed_and_its_frames_served() {
    const REUSE_TURN: &str = "META-REUSE-TURN";
    let realm = Realm::start().await;
    let aggregator = console(&realm.fixture, Arc::new(AllowAllConsoleVisibilityPolicy));
    let council = Council::convene(&realm.fixture, &realm.gate).await;
    let fork_session = council.source_fork_session(&realm.fixture).await;
    // A moment while the council runs.
    let during_council = frame_time_now();
    council.end(&realm.fixture, &realm.gate).await;
    council.settled(&realm.fixture).await;
    let primary = realm.fixture.runtime.handle().mob_id().to_string();

    // The ordinary mob under the settled council's mob id, with one turn.
    let state = mob_tools(&realm.fixture);
    let reuse_session = seat_nested_mob(&state, &council.mob, REUSE_MEMBER).await;
    let reused = state
        .handle_for(&meerkat_mob::MobId::from(council.mob.as_str()))
        .await
        .expect("the ordinary mob under the council's mob id");
    turn_on(&realm.fixture, &reused, REUSE_MEMBER, REUSE_TURN).await;
    // A moment after the ordinary mob exists.
    let after_reuse = frame_time_now();
    assert!(during_council < after_reuse, "the council ended in between");

    // What a console holds under that mob id. Stored while the council ran:
    // a seat's frame, and a frame of a member of the council's mob that was
    // no seat. Stored after the ordinary mob exists: a frame under a seat's
    // name on a session the record never held, a frame on a seat's fork
    // session under a name the record never held, and a frame of the
    // ordinary mob's member.
    let seeds = [
        (
            "settled-council-seat",
            captured(
                PARTICIPANTS[0],
                &fork_session,
                "council_p0",
                &primary,
                &council.mob,
            ),
            during_council,
        ),
        (
            "settled-council-not-a-seat",
            captured(
                "council-helper",
                "council-helper-session",
                "general",
                &primary,
                &council.mob,
            ),
            during_council,
        ),
        (
            "settled-council-seat-by-name",
            captured(
                PARTICIPANTS[1],
                "unrecorded-seat-session",
                "council_p1",
                &primary,
                &council.mob,
            ),
            after_reuse,
        ),
        (
            "settled-council-seat-by-session",
            captured(
                "unrecorded-seat-name",
                &fork_session,
                "council_p0",
                &primary,
                &council.mob,
            ),
            after_reuse,
        ),
        (
            "ordinary-member",
            captured(
                REUSE_MEMBER,
                &reuse_session.to_string(),
                "general",
                &primary,
                &council.mob,
            ),
            after_reuse,
        ),
    ];
    for (key, provenance, stored_at) in seeds {
        aggregator
            .store()
            .append_if_absent(stored_frame_at(key, SOURCE_TURN, provenance, stored_at))
            .await
            .expect("seeded frame");
    }
    backfill(&aggregator).await;

    let mut wrong = Vec::new();
    let identities = listed(&aggregator).await;
    if !identities.contains(REUSE_MEMBER) {
        wrong.push(format!(
            "identities does not list {REUSE_MEMBER} of the ordinary mob {}: {identities:?}",
            council.mob
        ));
    }
    assert!(
        identities.is_disjoint(&PARTICIPANTS.map(str::to_string).into()),
        "the settled council's participants are not listed: {identities:?}"
    );
    if aggregator
        .inspect_identity(REUSE_MEMBER)
        .await
        .expect("inspect")
        .is_none()
    {
        wrong.push(format!("inspect does not resolve {REUSE_MEMBER}"));
    }
    // An identities read asks for each listed member's history too.
    backfill(&aggregator).await;

    let mut names = realm.names();
    names.extend([REUSE_MEMBER, "unrecorded-seat-name", "council-helper"]);
    let ordinary = BTreeSet::from(["ordinary-member".to_string()]);
    let controller = controller(deny_source_by_identity());
    let administrator = served(&aggregator, &controller, Some(ROOT), &names).await;
    if !administrator
        .iter()
        .any(|frame| frame.identity == REUSE_MEMBER && carries(frame, REUSE_TURN))
    {
        wrong.push(format!(
            "the history of {REUSE_MEMBER} is not served to the administrator"
        ));
    }
    for subject in [ROOT, CALLER] {
        let seeded = served_seeds(&aggregator, &controller, subject, &names).await;
        if seeded != ordinary {
            wrong.push(format!(
                "{subject} is served the stored frames {seeded:?}, not {ordinary:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "an ordinary mob under a settled council's mob id is decided as the council's: {wrong:#?}"
    );
    realm.fixture.stop().await;
}

/// A council's record that reads settled while its cleanup receipt does not
/// say its temporary mob is destroyed (no council writes one) still owns its
/// mob id: a mob held under it stays unlisted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_settled_record_without_a_destroyed_receipt_keeps_its_mob_id() {
    let realm = Realm::start().await;
    let aggregator = console(&realm.fixture, Arc::new(AllowAllConsoleVisibilityPolicy));
    let council = Council::convene(&realm.fixture, &realm.gate).await;
    council.end(&realm.fixture, &realm.gate).await;
    council.settled(&realm.fixture).await;

    let state = mob_tools(&realm.fixture);
    let store = state.temporary_council_store_for_tests();
    let council_id = meerkat_mob::temporary_council::TemporaryCouncilId::new(&council.id)
        .expect("council identity");
    let mut record = store
        .load(&council_id)
        .await
        .expect("read council custody")
        .expect("council record");
    assert!(
        record
            .cleanup
            .as_ref()
            .is_some_and(|cleanup| cleanup.temporary_mob_destroyed),
        "a settled council's receipt says its temporary mob is destroyed"
    );
    // The receipt stays, and says the temporary mob is not destroyed.
    record
        .cleanup
        .as_mut()
        .expect("a settled council's receipt")
        .temporary_mob_destroyed = false;
    let record = store.commit(&record).await.expect("rewrite the record");
    assert!(!record.is_unfinished(), "the record still reads settled");

    seat_nested_mob(&state, &council.mob, REUSE_MEMBER).await;
    let identities = listed(&aggregator).await;
    assert!(
        !identities.contains(REUSE_MEMBER),
        "a mob under the id of a record with no destroyed receipt stays unlisted: {identities:?}"
    );
    realm.fixture.stop().await;
}

/// Whether the console decides the mob of `incarnation`, listed under
/// `mob_id`, as a council's.
async fn decided_as_a_councils(
    aggregator: &MetaConsoleAggregator,
    mob_id: &str,
    incarnation: meerkat_core::service::MobIncarnationId,
) -> bool {
    aggregator
        .council_owns_mob_for_test(RUNTIME, mob_id, incarnation)
        .await
        .expect("the runtime's council decision")
}

/// The console lists a nested mob's members before it reads the council
/// record. A council's mob listed while the council ran and decided after
/// its record settled stays the council's, so the seats just listed are not
/// shown as an ordinary mob's members: the runtime no longer holds that
/// incarnation. That holds with no mob under the id and with an ordinary mob
/// created under it since; the ordinary mob's own incarnation is no
/// council's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mob_listed_before_its_council_settled_stays_the_councils() {
    let realm = Realm::start().await;
    let aggregator = console(&realm.fixture, Arc::new(AllowAllConsoleVisibilityPolicy));
    let council = Council::convene(&realm.fixture, &realm.gate).await;
    let state = mob_tools(&realm.fixture);
    let mob = meerkat_mob::MobId::from(council.mob.as_str());
    // What a listing made now keeps of the council's mob. The handle is not
    // kept: a held one would keep the council's destroy from draining.
    let listed = state
        .handle_for(&mob)
        .await
        .expect("the council's temporary mob")
        .incarnation();
    assert!(
        decided_as_a_councils(&aggregator, &council.mob, listed).await,
        "a running council owns its temporary mob"
    );

    council.end(&realm.fixture, &realm.gate).await;
    council.settled(&realm.fixture).await;
    assert!(
        decided_as_a_councils(&aggregator, &council.mob, listed).await,
        "the mob listed before the council's destroy is the council's after its record settled"
    );

    seat_nested_mob(&state, &council.mob, REUSE_MEMBER).await;
    let ordinary = state
        .handle_for(&mob)
        .await
        .expect("the ordinary mob under the council's mob id")
        .incarnation();
    assert!(
        listed != ordinary,
        "a destroy and a create under one id start a new incarnation"
    );
    assert!(
        decided_as_a_councils(&aggregator, &council.mob, listed).await,
        "the mob listed before the council's destroy stays the council's with an ordinary mob \
         under its id"
    );
    assert!(
        !decided_as_a_councils(&aggregator, &council.mob, ordinary).await,
        "the ordinary mob created under the settled council's mob id is no council's"
    );
    realm.fixture.stop().await;
}

/// The person a private member belongs to, and that person's private-owner
/// key.
const OWNER: &str = "owner@example.test";
const OWNER_KEY: &str = "owner-private-key";

/// [`captured`] for a member of the primary mob that is private to the
/// person with `owner_key`.
fn captured_private(
    identity: &str,
    session: &str,
    primary_mob: &str,
    owner_key: &str,
) -> ConsoleFrameMemberProvenance {
    let mut provenance = captured(identity, session, "general", primary_mob, primary_mob);
    let label = crate::access::PRIVATE_TO_LABEL.to_string();
    provenance
        .identity
        .labels
        .insert(label.clone(), owner_key.to_string());
    provenance
        .member
        .labels
        .insert(label.clone(), owner_key.to_string());
    for link in &mut provenance.lineage {
        if let Some(labels) = link.labels.as_mut() {
            labels.insert(label.clone(), owner_key.to_string());
        }
    }
    provenance
}

/// The seeded frames a caller with `view` is served.
async fn seeds_served_to(
    aggregator: &MetaConsoleAggregator,
    view: crate::access::AccessView,
    names: &[&str],
) -> BTreeSet<String> {
    served_to(aggregator, view, names)
        .await
        .into_iter()
        .filter_map(|frame| frame.payload.get("seeded")?.as_str().map(str::to_string))
        .collect()
}

/// With no runtime registered each caller's own access rules still decide
/// every stored frame, as they do with one registered. The configuration
/// has access on, one allow rule (`agent.view` on every agent, for every
/// subject) and one deny rule: effect deny, action `agent.view`, every
/// subject, selector the agent `alice` by exact identity. A frame of a
/// member private to one person that holds its captured member attributes
/// is served to that person and to no other signed-in person, by the
/// captured private-owner rule. The same member's frame without captured
/// attributes is served to neither of them: a console that can identify a
/// person serves a frame stored without member attributes to an
/// administrator only, whatever the rules. A frame of the denied agent
/// itself is served to no non-administrator. What the registered runtime's
/// visibility policy alone hides (here a nested member, on a console that
/// hides nested members) is printed for both states.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stored_frames_with_no_runtime_registered_keep_each_callers_access_rules() {
    stored_frames_keep_access_rules(Some(deny_source_by_identity())).await;
}

/// The same with allow rules only: without a deny rule the private member's
/// frame is still served to its person alone, and its frame without captured
/// member attributes to neither signed-in person.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stored_frames_with_no_runtime_registered_keep_access_rules_under_allow_rules_only() {
    stored_frames_keep_access_rules(None).await;
}

async fn stored_frames_keep_access_rules(deny: Option<AccessRule>) {
    let denies = deny.is_some();
    let realm = Realm::start().await;
    let primary = realm.fixture.runtime.handle().mob_id().to_string();
    let aggregator = MetaConsoleAggregator::in_memory();
    let mut without_capture = stored_frame(
        "private-member-without-capture",
        SOURCE_TURN,
        captured_private("side-chat", "side-chat-session", &primary, OWNER_KEY),
    );
    without_capture.source.member_provenance = None;
    let seeds = [
        stored_frame(
            "private-member",
            SOURCE_TURN,
            captured_private("side-chat", "side-chat-session", &primary, OWNER_KEY),
        ),
        without_capture,
        stored_frame(
            "denied-member",
            SOURCE_TURN,
            captured(SOURCE, "source-session", SOURCE_ROLE, &primary, &primary),
        ),
        stored_frame(
            "shared-member",
            SOURCE_TURN,
            captured("bob", "bob-session", "general", &primary, &primary),
        ),
        stored_frame(
            "nested-member",
            SOURCE_TURN,
            captured(
                NESTED_MEMBER,
                &realm.nested_session.to_string(),
                "general",
                &primary,
                &realm.nested_mob,
            ),
        ),
    ];
    for seed in seeds {
        aggregator
            .store()
            .append_if_absent(seed)
            .await
            .expect("seeded frame");
    }
    register(
        &aggregator,
        &realm.fixture.runtime,
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    );
    let _ = listed(&aggregator).await;

    let mut names = realm.names();
    names.push("side-chat");
    let controller = match deny {
        Some(deny) => controller(deny),
        None => AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec![ROOT.to_string()],
            rules: vec![everyone_views()],
            ..AccessControlConfig::default()
        })
        .expect("controller"),
    };
    let callers = || {
        [
            (
                "owner",
                controller.view_for_caller(Some(OWNER), Some(OWNER_KEY)),
            ),
            (
                "other",
                controller.view_for_caller(Some(CALLER), Some("other-private-key")),
            ),
            ("administrator", controller.view_for_subject(Some(ROOT))),
        ]
    };
    let mut registered = BTreeMap::new();
    for (caller, view) in callers() {
        registered.insert(caller, seeds_served_to(&aggregator, view, &names).await);
    }
    aggregator.unregister_runtime(RUNTIME);
    let mut no_runtime = BTreeMap::new();
    for (caller, view) in callers() {
        no_runtime.insert(caller, seeds_served_to(&aggregator, view, &names).await);
    }
    println!("served with the runtime registered: {registered:#?}");
    println!("served with no runtime registered: {no_runtime:#?}");

    let mut wrong = Vec::new();
    for (state, served) in [("registered", &registered), ("no runtime", &no_runtime)] {
        for seed in [
            "private-member",
            "private-member-without-capture",
            "denied-member",
        ] {
            if seed == "denied-member" && !denies {
                continue;
            }
            if served["other"].contains(seed) {
                wrong.push(format!(
                    "{state}: another signed-in person is served {seed}"
                ));
            }
        }
        // "To neither" signed-in person: the member's own person is not
        // served the frame stored without its member attributes either.
        if served["owner"].contains("private-member-without-capture") {
            wrong.push(format!(
                "{state}: the owner is served private-member-without-capture"
            ));
        }
    }
    if !no_runtime["owner"].contains("private-member") {
        wrong.push("no runtime: the owner is not served the private member's frame".to_string());
    }
    if !no_runtime["other"].contains("shared-member") {
        wrong.push("no runtime: a shared member's frame is not served".to_string());
    }
    // "To an administrator only": somebody is still served that frame.
    if !no_runtime["administrator"].contains("private-member-without-capture") {
        wrong.push(
            "no runtime: an administrator is not served private-member-without-capture".to_string(),
        );
    }
    assert!(
        wrong.is_empty(),
        "a caller's access rules do not decide a stored frame: {wrong:#?}\nregistered: \
         {registered:#?}\nno runtime: {no_runtime:#?}"
    );
    realm.fixture.stop().await;
}

/// What stays as it is on a console that includes nested members: the
/// source, the other members and the members of ordinary nested mobs (one
/// whose id has a council mob's form included) are listed and resolved by
/// name, a deny on the source hides the source's own frames and nothing
/// else, and a nested member's history is read. A console that hides nested
/// members lists none of them and no participant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_source_other_nested_members_and_a_hiding_console_read_as_before() {
    let realm = Realm::start().await;
    turn(&realm.fixture, "bob", "META-BOB-EARLIER-TURN").await;
    let names = realm.names();
    let aggregator = console(&realm.fixture, Arc::new(AllowAllConsoleVisibilityPolicy));
    let hiding = console(
        &realm.fixture,
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    );
    let council = Council::convene(&realm.fixture, &realm.gate).await;
    backfill(&aggregator).await;
    backfill(&hiding).await;

    let including = listed(&aggregator).await;
    for member in [
        "convener",
        SOURCE,
        "bob",
        BYSTANDER,
        NESTED_MEMBER,
        LOOKALIKE_MEMBER,
    ] {
        assert!(
            including.contains(member),
            "an including console lists {member}: {including:?}"
        );
        assert!(
            aggregator
                .inspect_identity(member)
                .await
                .expect("inspect")
                .is_some(),
            "an including console resolves {member}"
        );
    }
    let hidden = listed(&hiding).await;
    assert_eq!(
        hidden,
        BTreeSet::from(["convener", SOURCE, "bob", BYSTANDER].map(str::to_string)),
        "a console that hides nested members lists the primary mob's members only"
    );
    for nested in PARTICIPANTS
        .into_iter()
        .chain([NESTED_MEMBER, LOOKALIKE_MEMBER])
    {
        assert!(
            hiding
                .inspect_identity(nested)
                .await
                .expect("inspect")
                .is_none(),
            "a console that hides nested members resolves no {nested}"
        );
    }

    // A deny on the source hides the source's own frames from the caller,
    // and leaves another member's readable.
    let by_identity = controller(deny_source_by_identity());
    let caller = served(&aggregator, &by_identity, Some(CALLER), &names).await;
    assert!(
        !caller.iter().any(|frame| frame.identity == SOURCE),
        "the deny hides the source's own frames"
    );
    assert!(
        caller
            .iter()
            .any(|frame| frame.identity == "bob" && carries(frame, "META-BOB-EARLIER-TURN")),
        "the caller reads another member's history"
    );
    let administrator = served(&aggregator, &by_identity, Some(ROOT), &names).await;
    assert!(
        administrator
            .iter()
            .any(|frame| frame.identity == SOURCE && carries(frame, SOURCE_TURN)),
        "the administrator reads the source's history"
    );

    // A nested member's history is read and served as before.
    let nested = realm
        .fixture
        .runtime
        .agent_mob_mcp_state()
        .expect("mob tools")
        .handle_for(&meerkat_mob::MobId::from(realm.nested_mob.as_str()))
        .await
        .expect("nested mob");
    turn_on(&realm.fixture, &nested, NESTED_MEMBER, "META-NESTED-TURN").await;
    backfill(&aggregator).await;
    assert!(
        served(&aggregator, &by_identity, Some(CALLER), &names)
            .await
            .iter()
            .any(|frame| frame.identity == NESTED_MEMBER && carries(frame, "META-NESTED-TURN")),
        "an including console serves a nested member's history"
    );
    assert!(
        !served(&hiding, &by_identity, Some(ROOT), &names)
            .await
            .iter()
            .any(|frame| frame.identity == NESTED_MEMBER),
        "a console that hides nested members serves none of their frames"
    );

    council.end(&realm.fixture, &realm.gate).await;
    realm.fixture.stop().await;
}

/// The members `state`'s mobs hold materialized in this process, mob by mob.
async fn materialized(state: &Arc<meerkat_mob_mcp::MobMcpState>) -> BTreeSet<(String, String)> {
    let mut members = BTreeSet::new();
    for (mob_id, handle) in state.mob_handles_snapshot().await.expect("managed mobs") {
        for member in handle.list_members_observation_snapshot().await {
            if !handle.member_unmaterialized(&member.agent_identity) {
                members.insert((mob_id.to_string(), member.agent_identity.to_string()));
            }
        }
    }
    members
}

/// The mobs `state` manages.
async fn managed_mobs(state: &Arc<meerkat_mob_mcp::MobMcpState>) -> BTreeSet<String> {
    state
        .mob_handles_snapshot()
        .await
        .expect("managed mobs")
        .into_iter()
        .map(|(mob_id, _)| mob_id.to_string())
        .collect()
}

/// A participant's frame as a console stored it, under each `source_mobs`
/// entry (its key, the mob the frame's captured provenance names).
async fn seed_participant_frames(
    aggregator: &MetaConsoleAggregator,
    primary: &str,
    source_mobs: &[(&str, String)],
) {
    for (key, source_mob) in source_mobs {
        aggregator
            .store()
            .append_if_absent(stored_frame(
                key,
                SOURCE_TURN,
                captured(
                    PARTICIPANTS[0],
                    "stored-session",
                    "council_p0",
                    primary,
                    source_mob,
                ),
            ))
            .await
            .expect("seeded frame");
    }
}

/// Every read that decides a council participant: an identities read, each
/// member read by a participant's name, and the timeline and history reads
/// of a denied caller and of the administrator.
async fn council_reads(aggregator: &MetaConsoleAggregator, names: &[&str]) {
    let _ = listed(aggregator).await;
    for participant in PARTICIPANTS {
        let _ = aggregator.inspect_identity(participant).await;
        let _ = aggregator.binary_blob_store_for_identity(participant).await;
        let _ = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                identity: Some(participant.to_string()),
                mode: ConsoleTimelineMode::Recent,
                limit: 400,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await;
    }
    let controller = controller(deny_source_by_identity());
    for subject in [CALLER, ROOT] {
        let _ = served(aggregator, &controller, Some(subject), names).await;
    }
}

fn lost_council_mob() -> String {
    format!(
        "{}{}",
        meerkat_mob::temporary_council::TEMPORARY_COUNCIL_MOB_ID_PREFIX,
        unique("lost-record")
    )
}

/// The reads that decide a council participant load nothing while a council
/// runs: across them, with frames stored for a participant of the running
/// council and of one whose record is lost, the members the runtime's mobs
/// hold materialized and the mobs it manages are what they were.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_council_reads_load_no_member_and_no_mob() {
    let realm = Realm::start().await;
    let names = realm.names();
    let state = mob_tools(&realm.fixture);
    let council = Council::convene(&realm.fixture, &realm.gate).await;
    let primary = realm.fixture.runtime.handle().mob_id().to_string();
    let aggregator = MetaConsoleAggregator::in_memory();
    seed_participant_frames(
        &aggregator,
        &primary,
        &[
            ("council-record-present", council.mob.clone()),
            ("council-record-lost", lost_council_mob()),
        ],
    )
    .await;
    register(
        &aggregator,
        &realm.fixture.runtime,
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );

    let mobs_before = managed_mobs(&state).await;
    assert!(
        mobs_before.contains(&council.mob),
        "the running council's mob is managed: {mobs_before:?}"
    );
    let before = materialized(&state).await;
    council_reads(&aggregator, &names).await;
    assert_eq!(
        materialized(&state).await,
        before,
        "the reads materialized no member and released none"
    );
    assert_eq!(
        managed_mobs(&state).await,
        mobs_before,
        "the reads loaded no mob and dropped none"
    );

    council.end(&realm.fixture, &realm.gate).await;
    realm.fixture.stop().await;
}

/// The reads that decide a council participant wake no Dormant member. On a
/// runtime whose sessions persist, a member of the primary mob is released
/// to Dormant; a nested mob whose id has a council mob's form is managed
/// beside it, and the console holds a frame stored for a participant whose
/// council record is lost. Across the reads the member stays Dormant, Core
/// is asked for none of its status (a Dormant member's status read loads its
/// durable session), and the materialized members are what they were.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_council_reads_wake_no_dormant_member() {
    const SLEEPER_TURN: &str = "META-SLEEPER-TURN";
    let gate = TurnGate::new();
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    let log = Arc::new(RequestLog::default());
    let state_dir = tempfile::tempdir().expect("state directory");
    let layout = crate::storage_layout::MetaStorageLayout::with_injected_roots(
        state_dir.path().to_path_buf(),
        None,
    );
    let (storage, provenance) =
        crate::mob_composition_manifest::persistent_mob_storage(layout.event_log_db())
            .map_err(|error| error.to_string())
            .expect("persistent mob storage");
    let mut spec = MobBootstrapSpec::persistent(
        definition(&unique("council-dormant")),
        storage,
        provenance,
        state_dir.path().to_path_buf(),
        4,
    )
    .await
    .map_err(|error| error.to_string())
    .expect("persistent runtime spec");
    spec.options.default_llm_client = Some(Arc::new(CompletionClient {
        gate: Arc::clone(&gate),
        log: Arc::clone(&log),
    }));
    let runtime = tokio::time::timeout(WAIT, Box::pin(MobRuntime::bootstrap(spec)))
        .await
        .expect("bounded persistent bootstrap")
        .map_err(|error| error.to_string())
        .expect("persistent runtime");
    let handle = runtime.handle();
    let state = runtime
        .agent_mob_mcp_state()
        .expect("a persistent runtime installs the agent mob tools");

    let sleeper = AgentIdentity::from(BYSTANDER);
    let mut spawn = SpawnMemberSpec::new("general", BYSTANDER);
    spawn.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    tokio::time::timeout(WAIT, handle.spawn_spec(spawn))
        .await
        .expect("bounded member spawn")
        .expect("seat the member");
    let session = handle
        .resolve_bridge_session_id(&sleeper)
        .await
        .expect("member session");
    handle
        .member(&sleeper)
        .await
        .expect("member")
        .internal_turn(meerkat_core::ContentInput::Text(SLEEPER_TURN.to_string()))
        .await
        .expect("turn admitted");
    log.wait_for_turn_started_by(SLEEPER_TURN).await;
    tokio::time::timeout(WAIT, async {
        loop {
            match handle.release_member(&sleeper).await {
                Ok(()) if handle.member_unmaterialized(&sleeper) => return,
                Ok(()) | Err(meerkat_mob::MobError::LifecycleOperationPending { .. }) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => panic!("release to Dormant: {error}"),
            }
        }
    })
    .await
    .expect("the member is released to Dormant");
    let lookalike_mob = format!(
        "{}{}",
        meerkat_mob::temporary_council::TEMPORARY_COUNCIL_MOB_ID_PREFIX,
        unique("lookalike")
    );
    seat_nested_mob(&state, &lookalike_mob, LOOKALIKE_MEMBER).await;

    let aggregator = MetaConsoleAggregator::in_memory();
    seed_participant_frames(
        &aggregator,
        handle.mob_id().as_str(),
        &[("council-record-lost", lost_council_mob())],
    )
    .await;
    register(
        &aggregator,
        &runtime,
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    // The backfill the registration starts reads no Dormant member either.
    let _ = tokio::time::timeout(
        Duration::from_secs(20),
        aggregator.history_backfill_converged(),
    )
    .await;

    let status_reads = || crate::console_aggregator::core_status_reads::count(&session.to_string());
    let status_reads_before = status_reads();
    let before = materialized(&state).await;
    assert!(
        !before.contains(&(handle.mob_id().to_string(), BYSTANDER.to_string())),
        "the released member is not materialized: {before:?}"
    );
    let mut names = PARTICIPANTS.to_vec();
    names.push(LOOKALIKE_MEMBER);
    council_reads(&aggregator, &names).await;

    assert!(
        handle.member_unmaterialized(&sleeper),
        "the Dormant member stays Dormant across the reads"
    );
    assert_eq!(
        status_reads(),
        status_reads_before,
        "the reads ask Core for no status of the Dormant member"
    );
    assert_eq!(
        materialized(&state).await,
        before,
        "the reads materialized no member"
    );
    assert!(
        listed(&aggregator).await.contains(LOOKALIKE_MEMBER),
        "the nested mob that merely has a council mob's id form is listed"
    );
}
