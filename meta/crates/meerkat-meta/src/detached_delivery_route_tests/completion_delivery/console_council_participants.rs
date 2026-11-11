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
    let view = controller.view_for_subject(subject);
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
/// it, and its text is `text`.
fn stored_frame(
    key: &str,
    text: &str,
    provenance: ConsoleFrameMemberProvenance,
) -> NewConsoleFrame {
    let identity = provenance.identity.identity.clone();
    let session = provenance.identity.session_id.clone();
    NewConsoleFrame {
        id: None,
        dedupe_key: key.to_string(),
        timestamp_ms: 1,
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
    register(
        &aggregator,
        &realm.fixture.runtime,
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let _ = listed(&aggregator).await;

    let mut names = realm.names();
    names.push("gone-worker");
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
