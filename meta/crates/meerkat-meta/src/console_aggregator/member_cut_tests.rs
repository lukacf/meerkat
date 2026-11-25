//! Rows for one member cut per console read (see `member_cut`): a read
//! decides every identity record and frame exactly as the per-call reads
//! decided them, from one listing of each runtime's members; an identities
//! read and a timeline page list each runtime once; an identities read asks
//! for a session-history backfill of a session only once; and a stream's
//! page that decided an unknown identity refreshes the identity read model.
#![allow(clippy::expect_used)]

use meerkat_core::SessionId;
use meerkat_mob::SpawnMemberSpec;

use super::tests::{
    build_empty_runtime, runtime_entry_for_test, spawn_trusted_identity_projected_member,
    unique_console_mob_id,
};
use super::*;
use crate::identity_first::core_test_bridge::{CoreReadsBridge, register_live};

const RUNTIME: &str = "runtime-a";

/// Hides members labelled `org=secret` and the identity `concealed`, and
/// lists implicit delegate members (the members with the role `delegate` of
/// a mob nested under the primary mob) only when asked to.
struct FixturePolicy {
    include_delegates: bool,
}

impl ConsoleVisibilityPolicy for FixturePolicy {
    fn include_implicit_delegate_members(&self) -> bool {
        self.include_delegates
    }

    fn member_visible(&self, member: &ConsoleMember) -> bool {
        member.labels.get("org").map(String::as_str) != Some("secret")
    }

    fn identity_visible(&self, record: &ConsoleIdentityRecord) -> bool {
        record.identity != "concealed"
    }
}

fn spec(member: &str, labels: &[(&str, &str)]) -> SpawnMemberSpec {
    spec_with_profile("worker", member, labels)
}

/// A member spec under `profile`, which is the member's role.
fn spec_with_profile(profile: &str, member: &str, labels: &[(&str, &str)]) -> SpawnMemberSpec {
    SpawnMemberSpec::from_wire(
        profile.to_string(),
        member.to_string(),
        Some(format!("You are {member}.").into()),
        None,
        None,
    )
    .with_labels(
        labels
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    )
}

fn durable_spec(identity: &str) -> crate::identity_first::DurableAgentSpec {
    crate::identity_first::DurableAgentSpec {
        identity: crate::identity_first::AgentIdentity::parse(identity).expect("identity"),
        profile: meerkat_mob::ProfileName::from("worker"),
        addressability: crate::identity_first::AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

/// A realm with every kind of member a console read decides:
/// - a visible member, and one hidden by its `org=secret` label;
/// - a durable identity's stale incarnation row (`rt:review:singleton:0`)
///   beside its own row, hidden, which shadows it;
/// - a durable identity named by its own row (`builder`);
/// - a nested mob's implicit delegate members (role `delegate`): one plain,
///   and one that projects the identity `builder` from another source;
/// - a durable identity (`relay`) whose stale primary row
///   (`rt:relay:0`) has its own row only as an implicit delegate member:
///   the binding check sees that row only while the policy lists the
///   implicit delegate members;
/// - a durable identity (`keeper`) whose only rows are its own primary
///   row, hidden, and an implicit delegate row projecting another identity:
///   the wrong projection keeps its record visible;
/// - a nested mob's member that is no implicit delegate (`teammate`, role
///   `worker`), listed while the policy lists nested mob members;
/// - durable identities with no live row: one concealed by the policy, one
///   dormant.
struct Realm {
    primary: UnifiedRuntime,
    delegate: UnifiedRuntime,
    entry: RuntimeEntry,
}

impl Realm {
    async fn build(include_delegates: bool) -> Self {
        let primary_id = unique_console_mob_id("member-cut-primary");
        let primary = build_empty_runtime(&primary_id).await;
        let members: [(&str, &[(&str, &str)]); 7] = [
            ("visible", &[]),
            ("secret", &[("org", "secret")]),
            (
                "rt:review:singleton:0",
                &[("agent_identity", "review:singleton")],
            ),
            (
                "review:singleton",
                &[("agent_identity", "review:singleton"), ("org", "secret")],
            ),
            ("builder", &[("agent_identity", "builder")]),
            ("rt:relay:0", &[("agent_identity", "relay")]),
            ("keeper", &[("agent_identity", "keeper"), ("org", "secret")]),
        ];
        for (member, labels) in members {
            spawn_trusted_identity_projected_member(&primary, spec(member, labels)).await;
        }
        let delegate_id = unique_console_mob_id("member-cut-delegate");
        let delegate = build_runtime_with_delegate_profile(&delegate_id).await;
        let delegate_members: [(&str, &[(&str, &str)]); 4] = [
            ("delegate", &[]),
            ("builder", &[("agent_identity", "other")]),
            ("relay", &[("agent_identity", "relay")]),
            ("keeper", &[("agent_identity", "other")]),
        ];
        for (member, labels) in delegate_members {
            spawn_trusted_identity_projected_member(
                &delegate,
                spec_with_profile("delegate", member, labels),
            )
            .await;
        }
        spawn_trusted_identity_projected_member(&delegate, spec("teammate", &[])).await;
        primary
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the primary runtime exposes its mob state")
            .mob_insert_handle(
                meerkat_mob::ids::MobId::from(delegate_id.as_str()),
                delegate.mob_handle().clone(),
            )
            .await;
        // The fixture puts the delegate mob into the gateway with no create
        // admission. The record of who each mob is private to reads it here
        // (a mob a host built, private to no one), as it reads every mob the
        // gateway holds at its start. A listing of a mob that record has not
        // read withholds the mob's members and starts the read in the
        // background; the rows over this realm decide one member twice, one
        // read after the other, and a read that landed between the two would
        // give them two answers for the member.
        primary
            .mob_runtime()
            .private_owners()
            .read_mob(&delegate.mob_handle())
            .await
            .expect("the owner record reads the delegate mob");

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()
                        .expect("continuity store"),
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "member-cut-equivalence".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(core_bridge.clone()),
                default_timeout: None,
            },
        ));
        for identity in [
            "review:singleton",
            "builder",
            "relay",
            "keeper",
            "concealed",
        ] {
            register_live(
                &identity_runtime,
                &core_bridge.core,
                durable_spec(identity),
                0,
                SessionId::new(),
            )
            .await
            .expect("a live durable identity registers");
        }
        identity_runtime
            .register(durable_spec("dormant"))
            .await
            .expect("the identity registers");

        let entry = RuntimeEntry {
            mob: None,
            registration_id: uuid::Uuid::new_v4(),
            runtime_key: RUNTIME.to_string(),
            identity_namespace: String::new(),
            runtime: primary.mob_runtime().clone(),
            identity_runtime: Some(identity_runtime),
            identity_owner: None,
            memory_panel: None,
            console_events: primary.console_events(),
            visibility_policy: Arc::new(FixturePolicy { include_delegates }),
        };
        Self {
            primary,
            delegate,
            entry,
        }
    }

    async fn stop(self) {
        let _ = self.delegate.mob_handle().stop().await;
        let _ = self.primary.mob_handle().stop().await;
    }
}

fn record(
    identity: &str,
    runtime_member_id: &str,
    session_id: Option<&str>,
) -> ConsoleIdentityRecord {
    ConsoleIdentityRecord {
        identity: identity.to_string(),
        display_name: identity.to_string(),
        runtime_key: RUNTIME.to_string(),
        runtime_member_id: runtime_member_id.to_string(),
        session_id: session_id.map(ToString::to_string),
        member_id: None,
        visibility: ConsoleVisibility::Addressable,
        addressable: true,
        health: "ready".to_string(),
        topology_peers: Vec::new(),
        labels: BTreeMap::new(),
    }
}

fn frame(identity: &str, session_id: Option<&str>) -> ConsoleFrame {
    ConsoleFrame {
        mob_id: None,
        id: format!("frame-{identity}-{session_id:?}"),
        cursor: ConsoleCursor::from_seq(1),
        dedupe_key: format!("frame-{identity}-{session_id:?}"),
        timestamp_ms: 1,
        runtime_key: RUNTIME.to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: session_id.map(ToString::to_string),
        kind: "text_complete".to_string(),
        status: ConsoleFrameStatus::Completed,
        frame_version: 1,
        updated_at_ms: None,
        payload: json!({ "text": identity }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::ConsoleEvent,
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

/// Every identity record a read of the realm decides: each durable
/// identity's status record, each listed member's record, durable records
/// spelled by their binding, and records with no live member at all.
async fn records_to_decide(realm: &Realm) -> Vec<ConsoleIdentityRecord> {
    let identity_runtime = realm
        .entry
        .identity_runtime
        .as_ref()
        .expect("identity runtime");
    let mut records: Vec<_> = identity_runtime
        .statuses()
        .await
        .expect("statuses")
        .iter()
        .map(|status| identity_record_for_status(&realm.entry, status))
        .collect();
    for resolved in member_sources_for_entry_including_hidden(&realm.entry).await {
        records.extend(identity_record_for_resolved_member(&resolved).await);
    }
    records.extend([
        record("builder", "rt:builder:0", None),
        record("review:singleton", "rt:review:singleton:0", None),
        record("retired", "retired", Some("retired-session")),
        record("ghost", "ghost", None),
    ]);
    records
}

/// Every frame a read of the realm decides: each record's identity with its
/// session, with none and with another one, under its runtime member id, and
/// frames of identities no member names.
fn frames_to_decide(records: &[ConsoleIdentityRecord]) -> Vec<ConsoleFrame> {
    let mut frames = Vec::new();
    for record in records {
        frames.push(frame(&record.identity, record.session_id.as_deref()));
        frames.push(frame(&record.identity, None));
        frames.push(frame(&record.identity, Some("another-session")));
        frames.push(frame(&record.runtime_member_id, None));
    }
    frames.push(frame("retired", Some("retired-session")));
    frames.push(frame("ghost", None));
    frames
}

/// A record naming `parent` as its spawner, bound (or not) to `session`.
fn child_of(parent: &str, session: Option<&str>) -> ConsoleIdentityRecord {
    let mut child = record("child", "child", Some("child-session"));
    child.labels.insert(
        crate::console_spawn::SPAWNED_BY_LABEL.to_string(),
        parent.to_string(),
    );
    if let Some(session) = session {
        child.labels.insert(
            crate::console_spawn::SPAWNED_BY_SESSION_LABEL.to_string(),
            session.to_string(),
        );
    }
    child
}

/// The per-call decisions a read made before the cut, kept from the
/// integration base (renamed, and following the same rule for a nested
/// mob's member as the cut: it is named by its mob and name, never tied to a
/// primary record by its bare name): each call lists the members itself.
mod per_call {
    use super::*;

    pub(super) async fn console_identity_record_visible(
        entry: &RuntimeEntry,
        record: &ConsoleIdentityRecord,
    ) -> bool {
        if !entry.visibility_policy.identity_visible(record) {
            return false;
        }
        let mut bound_live_member_seen = false;
        let mut wrong_live_projection_seen = false;
        for resolved in Box::pin(member_sources_for_entry_including_hidden(entry)).await {
            if !member_cut::live_member_names_record(&resolved, record) {
                continue;
            }
            bound_live_member_seen = true;
            let Some(live_record) = identity_record_for_resolved_member(&resolved).await else {
                continue;
            };
            if live_record.identity != record.identity {
                wrong_live_projection_seen = true;
                continue;
            }
            if raw_resolved_member_visible(&resolved, &live_record) {
                return true;
            }
        }
        if wrong_live_projection_seen {
            return true;
        }
        !bound_live_member_seen
    }

    async fn live_record_shadowed_by_hidden_durable_binding(
        resolved: &ResolvedConsoleMember,
        record: &ConsoleIdentityRecord,
    ) -> bool {
        let Some(identity_runtime) = resolved.identity_runtime() else {
            return false;
        };
        let Ok(identity) = crate::identity_first::AgentIdentity::parse(&record.identity) else {
            return false;
        };
        if crate::member_comms_id::live_member_is_identity(
            &resolved.runtime_identity,
            identity.as_str(),
        ) {
            return false;
        }
        let status = match identity_runtime.status(&identity).await {
            Ok(status) => status,
            Err(crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => return false,
            Err(_) => return true,
        };
        if status.agent_runtime_id.is_none() {
            return false;
        }
        for candidate in Box::pin(member_sources_for_entry(&resolved.entry)).await {
            if candidate.nested()
                || !crate::member_comms_id::live_member_is_identity(
                    &candidate.runtime_identity,
                    status.identity.as_str(),
                )
            {
                continue;
            }
            let Some(bound_record) = identity_record_for_resolved_member(&candidate).await else {
                continue;
            };
            if bound_record.identity == record.identity
                && !raw_resolved_member_visible(&candidate, &bound_record)
            {
                return true;
            }
        }
        false
    }

    pub(super) async fn resolved_member_visible(
        resolved: &ResolvedConsoleMember,
        record: &ConsoleIdentityRecord,
    ) -> bool {
        if !raw_resolved_member_visible(resolved, record) {
            return false;
        }
        !Box::pin(live_record_shadowed_by_hidden_durable_binding(
            resolved, record,
        ))
        .await
    }

    pub(super) async fn frame_matches_hidden_member(
        entry: &RuntimeEntry,
        frame: &ConsoleFrame,
    ) -> bool {
        let runtime_member_id = strip_namespace(&frame.identity, &entry.identity_namespace)
            .unwrap_or_else(|| frame.identity.clone());
        let frame_session_id = frame.session_id.as_deref();
        let mut saw_hidden = false;
        let mut saw_visible = false;
        let mut named_bare_by_a_nested_member = false;
        for resolved in Box::pin(member_sources_for_entry_including_hidden(entry)).await {
            let Some(record) = identity_record_for_resolved_member(&resolved).await else {
                continue;
            };
            let session_matches =
                frame_session_id.is_some() && record.session_id.as_deref() == frame_session_id;
            let runtime_matches = record.runtime_member_id == runtime_member_id;
            let identity_matches = record.identity == frame.identity;
            if !session_matches && !runtime_matches && !identity_matches {
                if resolved.nested() && resolved.runtime_identity == runtime_member_id {
                    named_bare_by_a_nested_member = true;
                }
                continue;
            }
            if session_matches && !raw_resolved_member_visible(&resolved, &record) {
                return true;
            }
            if session_matches && resolved.nested() && !runtime_matches && !identity_matches {
                return true;
            }
            if Box::pin(resolved_member_visible(&resolved, &record)).await {
                saw_visible = true;
            } else {
                saw_hidden = true;
            }
        }
        (saw_hidden || named_bare_by_a_nested_member) && !saw_visible
    }

    pub(super) async fn capture_member_lineage(
        resolved: &ResolvedConsoleMember,
        record: &ConsoleIdentityRecord,
    ) -> Vec<ConsoleFrameLineageLink> {
        let namespace = resolved.entry.identity_namespace.as_str();
        let mut lineage = vec![lineage_link_for_record(
            record,
            namespace,
            resolved.member.role.as_str(),
            resolved.member.labels.get("role").map(String::as_str),
        )];
        let mut visited = BTreeSet::from([record.identity.clone()]);
        let mut child_labels = record.labels.clone();
        let mut sources: Option<Vec<ResolvedConsoleMember>> = None;
        while lineage.len() < CAPTURED_LINEAGE_DEPTH {
            let Some(parent) = child_labels
                .get(crate::console_spawn::SPAWNED_BY_LABEL)
                .map(|parent| parent.trim().to_string())
                .filter(|parent| !parent.is_empty())
            else {
                break;
            };
            let parent_identity = apply_namespace(&parent, namespace);
            if !visited.insert(parent_identity.clone()) {
                break;
            }
            let bound_session = child_labels
                .get(crate::console_spawn::SPAWNED_BY_SESSION_LABEL)
                .map(|session| session.trim().to_string())
                .filter(|session| !session.is_empty());
            let mut confirmed = None;
            if let Some(bound_session) = bound_session.as_deref() {
                if sources.is_none() {
                    sources = Some(
                        Box::pin(member_sources_for_entry_including_hidden(&resolved.entry)).await,
                    );
                }
                for candidate in sources.iter().flatten() {
                    let roster_id = candidate.member.agent_identity.as_str();
                    let alias_name = crate::member_comms_id::runtime_alias_str(roster_id);
                    let alias: &str = alias_name.as_ref();
                    let durable =
                        crate::member_comms_id::durable_identity_label(&candidate.member.labels)
                            .unwrap_or(alias);
                    if roster_id != parent
                        && alias != parent
                        && durable != parent
                        && candidate.console_name().as_deref() != Some(parent.as_str())
                    {
                        continue;
                    }
                    let Some(candidate_record) =
                        Box::pin(identity_record_for_resolved_member(candidate)).await
                    else {
                        continue;
                    };
                    if candidate_record.session_id.as_deref() == Some(bound_session) {
                        confirmed = Some((
                            candidate_record,
                            candidate.member.role.to_string(),
                            candidate.member.labels.get("role").cloned(),
                        ));
                        break;
                    }
                }
            }
            match confirmed {
                Some((parent_record, role, own_role_label)) => {
                    lineage.push(lineage_link_for_record(
                        &parent_record,
                        namespace,
                        &role,
                        own_role_label.as_deref(),
                    ));
                    child_labels = parent_record.labels;
                }
                None => {
                    lineage.push(ConsoleFrameLineageLink {
                        identity: parent_identity,
                        agent_id: None,
                        role: None,
                        labels: None,
                    });
                    break;
                }
            }
        }
        lineage
    }
}

/// One cut per read decides every record, frame, member and lineage exactly
/// as the per-call reads did, over a realm with visible, hidden, shadowed,
/// delegate, nested, labelled, dormant, retired and unknown identities, with
/// the policy listing implicit delegate members and without. It is an
/// equivalence guard, green on the base by construction: a cut that kept a
/// listing the base would not have read (an implicit delegate member the
/// policy leaves out of the binding check), or reused a decision across
/// members, turns it red.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_cut_decides_every_record_and_frame_as_the_per_call_reads() {
    for include_delegates in [true, false] {
        let realm = Realm::build(include_delegates).await;
        let records = records_to_decide(&realm).await;
        let frames = frames_to_decide(&records);
        let cut = MemberCut::new(&realm.entry);

        for record in &records {
            assert_eq!(
                console_identity_record_visible_in(&cut, record).await,
                per_call::console_identity_record_visible(&realm.entry, record).await,
                "record {} ({}) with delegates {include_delegates}",
                record.identity,
                record.runtime_member_id,
            );
        }
        for frame in &frames {
            assert_eq!(
                frame_matches_hidden_member_in(&cut, frame).await,
                per_call::frame_matches_hidden_member(&realm.entry, frame).await,
                "frame of {} in session {:?} with delegates {include_delegates}",
                frame.identity,
                frame.session_id,
            );
        }
        for member in cut.members().await {
            let resolved = &member.resolved;
            let Some(own) = member.record().await else {
                continue;
            };
            assert_eq!(
                resolved_member_visible_in(&cut, resolved, own).await,
                per_call::resolved_member_visible(resolved, own).await,
                "member {} with delegates {include_delegates}",
                resolved.runtime_identity,
            );
            let session = own.session_id.as_deref();
            for child in [
                child_of(&resolved.runtime_identity, session),
                child_of(&resolved.runtime_identity, Some("an-earlier-parent")),
                child_of(&resolved.runtime_identity, None),
            ] {
                assert_eq!(
                    capture_member_lineage_in(&cut, resolved, &child).await,
                    per_call::capture_member_lineage(resolved, &child).await,
                    "lineage through {} with delegates {include_delegates}",
                    resolved.runtime_identity,
                );
            }
        }
        // The realm is decided both ways: something is hidden and something
        // is visible, so the comparison is not over a uniform answer.
        let mut outcomes = BTreeSet::new();
        for record in &records {
            outcomes.insert(console_identity_record_visible_in(&cut, record).await);
        }
        assert_eq!(outcomes, BTreeSet::from([false, true]));
        // A nested mob's row is tied to no primary record by its bare name:
        // `keeper`, whose own primary row the policy hides, stays hidden
        // whatever a nested row of that name projects.
        assert!(
            !console_identity_record_visible_in(&cut, &record("keeper", "rt:keeper:0", None)).await
        );
        // The outcome that depends on which listing a decision reads, pinned:
        // RED when the binding check ties the stale `rt:relay:0` row to a
        // nested row of that name.
        if !include_delegates {
            let mut relay_stale_row = None;
            for member in cut.members().await {
                if member.resolved.runtime_identity == "rt:relay:0" {
                    let own = member.record().await.expect("the stale row's record");
                    relay_stale_row =
                        Some(resolved_member_visible_in(&cut, &member.resolved, own).await);
                }
            }
            assert_eq!(relay_stale_row, Some(true));
        }
        realm.stop().await;
    }
}

/// Every frame decision of a read through `frame_is_visible_cached`, one
/// read for all frames, equals the frame decided as a read of its own, with
/// the identities a collect of the realm lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_decides_each_frame_as_a_read_of_that_frame_alone() {
    for include_delegates in [true, false] {
        let realm = Realm::build(include_delegates).await;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(RUNTIME.to_string(), realm.entry.clone());
        let identity_records = collect_identity_records(
            &aggregator.inner,
            IdentityCollectionMode::IncludeLiveMembers,
        )
        .await
        .expect("identity records");
        let records = records_to_decide(&realm).await;
        let mut read = FrameVisibilityRead::default();
        for frame in frames_to_decide(&records) {
            for allow_historical_identity in [false, true] {
                assert_eq!(
                    frame_is_visible_cached(
                        &aggregator.inner,
                        &frame,
                        allow_historical_identity,
                        &mut read,
                        &identity_records,
                    )
                    .await
                    .expect("decided in the read"),
                    frame_is_visible(
                        &aggregator.inner,
                        &frame,
                        allow_historical_identity,
                        &identity_records,
                    )
                    .await
                    .expect("decided alone"),
                    "frame of {} in session {:?} (historical {allow_historical_identity}, \
                     delegates {include_delegates})",
                    frame.identity,
                    frame.session_id,
                );
            }
        }
        realm.stop().await;
    }
}

/// An identity-first realm of `count` live durable identities, each named by
/// its own row, under the default policy.
async fn identity_first_realm(count: usize) -> (UnifiedRuntime, RuntimeEntry) {
    let runtime = build_empty_runtime(&unique_console_mob_id("member-cut-listings")).await;
    let core_bridge = Arc::new(CoreReadsBridge::default());
    let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
        crate::identity_first::IdentityRuntimeConfig {
            continuity_store: Arc::new(
                crate::identity_first::LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
            runtime_instance_id: "member-cut-listings".to_string(),
            has_runtime_store: true,
            durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        },
    ));
    for index in 0..count {
        let identity = format!("member-{index}");
        spawn_trusted_identity_projected_member(
            &runtime,
            spec(&identity, &[("agent_identity", identity.as_str())]),
        )
        .await;
        register_live(
            &identity_runtime,
            &core_bridge.core,
            durable_spec(&identity),
            0,
            SessionId::new(),
        )
        .await
        .expect("a live durable identity registers");
    }
    let entry = RuntimeEntry {
        mob: None,
        registration_id: uuid::Uuid::new_v4(),
        runtime_key: RUNTIME.to_string(),
        identity_namespace: String::new(),
        runtime: runtime.mob_runtime().clone(),
        identity_runtime: Some(identity_runtime),
        identity_owner: None,
        memory_panel: None,
        console_events: runtime.console_events(),
        visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
    };
    (runtime, entry)
}

fn aggregator_over(entry: &RuntimeEntry) -> MetaConsoleAggregator {
    let aggregator = MetaConsoleAggregator::new_with_options(
        Arc::new(InMemoryConsoleLogStore::new()),
        ConsoleAggregatorOptions {
            session_history_backfill_enabled: false,
            ..ConsoleAggregatorOptions::default()
        },
    );
    aggregator
        .inner
        .runtimes
        .write()
        .expect("runtime registry")
        .insert(RUNTIME.to_string(), entry.clone());
    aggregator
}

/// An identities read lists each runtime's members once. RED on the base:
/// every durable identity's record listed the members again (N listings)
/// and the live members once more, N + 1 in all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_identities_read_lists_each_runtime_once() {
    const IDENTITIES: usize = 6;
    let (runtime, entry) = identity_first_realm(IDENTITIES).await;
    let aggregator = aggregator_over(&entry);

    let primary = |registration| {
        member_cut::listings::count(registration, member_cut::listings::Listing::Primary)
    };
    let delegates = |registration| {
        member_cut::listings::count(registration, member_cut::listings::Listing::Delegates)
    };
    let (primary_before, delegates_before) = (
        primary(entry.registration_id),
        delegates(entry.registration_id),
    );
    let identities = aggregator.list_identities().await.expect("identities");
    assert_eq!(
        primary(entry.registration_id) - primary_before,
        1,
        "an identities read lists the primary mob's members once"
    );
    assert!(delegates(entry.registration_id) - delegates_before <= 1);
    let listed: BTreeSet<_> = identities
        .iter()
        .map(|record| record.identity.clone())
        .collect();
    assert_eq!(
        listed,
        (0..IDENTITIES)
            .map(|index| format!("member-{index}"))
            .collect::<BTreeSet<_>>(),
    );
    let _ = runtime.mob_handle().stop().await;
}

/// Under a policy that hides implicit delegate members, an identities read
/// of an identity runtime's entry with no status record lists the primary
/// mob's members once and the nested mobs' once, inside the bounded live
/// listing, and lists the nested mob's member that is no implicit delegate.
/// RED when a decision of the read lists a mob again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bounded_live_listing_lists_the_primary_and_the_nested_mobs_once() {
    let primary = build_empty_runtime(&unique_console_mob_id("member-cut-bounded")).await;
    spawn_trusted_identity_projected_member(&primary, spec("worker", &[])).await;
    let delegate_id = unique_console_mob_id("member-cut-bounded-delegate");
    // The nested mob is one the primary mob's `worker` made.
    let delegate = MemberMadeMob::made_by(&primary, "worker", &delegate_id).await;
    delegate.seat(spec("helper", &[])).await;
    primary
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the primary runtime exposes its mob state")
        .mob_insert_handle(
            meerkat_mob::ids::MobId::from(delegate_id.as_str()),
            delegate.mob_handle().clone(),
        )
        .await;
    delegate.read_by_the_owner_record_of(&primary).await;
    let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
        crate::identity_first::IdentityRuntimeConfig {
            continuity_store: Arc::new(
                crate::identity_first::LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
            runtime_instance_id: "member-cut-bounded".to_string(),
            has_runtime_store: true,
            durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(CoreReadsBridge::default())),
            default_timeout: None,
        },
    ));
    let entry = RuntimeEntry {
        mob: None,
        registration_id: uuid::Uuid::new_v4(),
        runtime_key: RUNTIME.to_string(),
        identity_namespace: String::new(),
        runtime: primary.mob_runtime().clone(),
        identity_runtime: Some(identity_runtime),
        identity_owner: None,
        memory_panel: None,
        console_events: primary.console_events(),
        visibility_policy: Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    };
    let aggregator = aggregator_over(&entry);
    let count = |listing| member_cut::listings::count(entry.registration_id, listing);
    let primary_before = count(member_cut::listings::Listing::Primary);
    let delegates_before = count(member_cut::listings::Listing::Delegates);

    let identities = aggregator.list_identities().await.expect("identities");

    assert_eq!(
        count(member_cut::listings::Listing::Primary) - primary_before,
        1,
        "the bounded live listing lists the primary mob once"
    );
    assert_eq!(
        count(member_cut::listings::Listing::Delegates) - delegates_before,
        1,
        "the bounded live listing lists the nested mobs once"
    );
    let listed: BTreeSet<_> = identities
        .iter()
        .map(|record| record.identity.clone())
        .collect();
    assert_eq!(
        listed,
        BTreeSet::from([format!("{delegate_id}/helper"), "worker".to_string()])
    );
    let _ = delegate.mob_handle().stop().await;
    let _ = primary.mob_handle().stop().await;
}

/// A timeline page lists each runtime's members once, however many frames
/// and identities it decides. RED on the base: every frame of a visible or
/// unknown identity listed the members again, and every identity once more.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timeline_page_lists_each_runtime_once() {
    const IDENTITIES: usize = 4;
    const FRAMES_EACH: usize = 3;
    let (runtime, entry) = identity_first_realm(IDENTITIES).await;
    let aggregator = aggregator_over(&entry);
    aggregator
        .list_identities()
        .await
        .expect("the identity read model is primed");
    let mut sequence = 0_u64;
    let mut append = |identity: String| {
        sequence += 1;
        NewConsoleFrame {
            mob_id: None,
            id: None,
            dedupe_key: format!("listing-frame-{sequence}"),
            timestamp_ms: sequence,
            runtime_key: RUNTIME.to_string(),
            identity: identity.clone(),
            conversation_id: Some(identity),
            session_id: None,
            kind: "text_complete".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({ "text": sequence }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::ConsoleEvent,
                source_cursor: None,
            },
            source_event_id: None,
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        }
    };
    let mut frames = Vec::new();
    for _ in 0..FRAMES_EACH {
        for index in 0..IDENTITIES {
            frames.push(append(format!("member-{index}")));
        }
        frames.push(append("retired".to_string()));
    }
    for frame in frames {
        aggregator
            .store()
            .append_if_absent(frame)
            .await
            .expect("append");
    }

    let primary = |registration| {
        member_cut::listings::count(registration, member_cut::listings::Listing::Primary)
    };
    let delegates = |registration| {
        member_cut::listings::count(registration, member_cut::listings::Listing::Delegates)
    };
    let (primary_before, delegates_before) = (
        primary(entry.registration_id),
        delegates(entry.registration_id),
    );
    let page = aggregator
        .query_timeline(ConsoleTimelineQuery {
            limit: 1_000,
            ..ConsoleTimelineQuery::default()
        })
        .await
        .expect("timeline page");
    assert_eq!(
        primary(entry.registration_id) - primary_before,
        1,
        "a timeline page lists the primary mob's members once"
    );
    assert!(delegates(entry.registration_id) - delegates_before <= 1);
    // Every live identity's frames, none of the retired identity's.
    assert_eq!(page.frames.len(), IDENTITIES * FRAMES_EACH);
    assert!(page.frames.iter().all(|frame| frame.identity != "retired"));
    let _ = runtime.mob_handle().stop().await;
}

/// A runtime whose durable store answers bounded boundary reads, with live
/// members whose sessions each hold one committed boundary there.
struct CommittedSessions {
    _temp: tempfile::TempDir,
    runtime: Arc<UnifiedRuntime>,
    service: super::tests::DelayedHistorySessionService,
    durable: Arc<dyn meerkat_runtime::RuntimeStore>,
    entry: RuntimeEntry,
    sessions: Vec<String>,
}

/// [`CommittedSessions`] of `members` live members.
async fn runtime_with_committed_sessions(members: usize) -> CommittedSessions {
    let durable: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let (temp, runtime, service) = super::tests::build_stress_runtime_with_durable_runtime_store(
        members,
        Duration::ZERO,
        Some(durable.clone()),
    )
    .await;
    let entry = runtime_entry_for_test(RUNTIME, runtime.as_ref());
    let mut sessions = Vec::new();
    for resolved in member_sources_for_entry(&entry).await {
        let record = identity_record_for_resolved_member(&resolved)
            .await
            .expect("a live member's record");
        let session = record.session_id.expect("a live member has a session");
        super::tests::commit_fixture_session_boundary(durable.as_ref(), &session, "first").await;
        sessions.push(session);
    }
    assert_eq!(sessions.len(), members);
    CommittedSessions {
        _temp: temp,
        runtime,
        service,
        durable,
        entry,
        sessions,
    }
}

/// A second identities read asks for no pass of a session whose history did
/// not move since its last pass read it (the same committed boundary): no
/// pass would read it. RED on the base: every identities read asked for
/// every listed live session again (four more asks).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_identities_read_asks_for_no_pass_of_a_session_whose_history_did_not_move() {
    const MEMBERS: usize = 4;
    let CommittedSessions {
        _temp,
        runtime,
        service,
        entry,
        ..
    } = runtime_with_committed_sessions(MEMBERS).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator
        .inner
        .runtimes
        .write()
        .expect("runtime registry")
        .insert(RUNTIME.to_string(), entry.clone());

    aggregator.list_identities().await.expect("identities");
    aggregator.history_backfill_converged().await;
    assert_eq!(
        identity_read_asks::count(entry.registration_id),
        MEMBERS,
        "the first read asks for each live session"
    );
    let reads = service.read_calls();

    aggregator.list_identities().await.expect("identities");
    aggregator.history_backfill_converged().await;
    assert_eq!(
        identity_read_asks::count(entry.registration_id),
        MEMBERS,
        "the second read asks for no session whose history did not move"
    );
    assert_eq!(
        service.read_calls(),
        reads,
        "no session history is read again"
    );
    let _ = runtime.mob_handle().stop().await;
}

/// An identities read asks again for a session whose history moved after
/// the first ask, once its last pass's watermark is no longer fresh (as the
/// pass itself gates its reads), and the pass reads it; a read after that,
/// with nothing moved, asks for nothing. The session's committed boundary
/// moves with a commit, and its watermark is aged in place of waiting out
/// its freshness. RED at the first form of the read-path rule (a session
/// asked for at most once per process): the second read asks for nothing
/// and the moved history is never read. RED for a rule that asks whenever
/// the watermark is stale: the third read asks again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_identities_read_asks_again_for_a_session_whose_history_moved() {
    let CommittedSessions {
        _temp,
        runtime,
        service,
        durable,
        entry,
        sessions,
    } = runtime_with_committed_sessions(1).await;
    let session = sessions.first().expect("one session").clone();
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator
        .inner
        .runtimes
        .write()
        .expect("runtime registry")
        .insert(RUNTIME.to_string(), entry.clone());
    let watermark_key = session_history_watermark_runtime_key(RUNTIME, &session);
    let age_the_watermark = || async {
        let watermark = aggregator
            .store()
            .source_watermark(&watermark_key, ConsoleFrameSourceKind::SessionHistory)
            .await
            .expect("watermark read")
            .expect("the last pass recorded a watermark");
        let offset = parse_session_history_watermark(&watermark, &session).unwrap_or(0);
        aggregator
            .store()
            .record_source_watermark(
                &watermark_key,
                ConsoleFrameSourceKind::SessionHistory,
                &format_session_history_watermark(
                    &session,
                    offset,
                    current_time_ms().saturating_sub(60_000),
                ),
            )
            .await
            .expect("aged watermark");
    };

    aggregator.list_identities().await.expect("identities");
    aggregator.history_backfill_converged().await;
    assert_eq!(identity_read_asks::count(entry.registration_id), 1);
    let first_reads = service.read_calls();
    assert!(
        first_reads >= 1,
        "the first pass reads the session's history"
    );

    super::tests::commit_fixture_session_boundary(durable.as_ref(), &session, "second").await;
    age_the_watermark().await;
    aggregator.list_identities().await.expect("identities");
    aggregator.history_backfill_converged().await;
    assert_eq!(
        identity_read_asks::count(entry.registration_id),
        2,
        "the read asks again for the session whose history moved"
    );
    assert!(
        service.read_calls() > first_reads,
        "the pass reads the moved history"
    );
    let second_reads = service.read_calls();

    age_the_watermark().await;
    aggregator.list_identities().await.expect("identities");
    aggregator.history_backfill_converged().await;
    assert_eq!(
        identity_read_asks::count(entry.registration_id),
        2,
        "nothing moved since the last pass: nothing is asked for"
    );
    assert_eq!(service.read_calls(), second_reads);
    let _ = runtime.mob_handle().stop().await;
}

/// Hides the member named `0` from every console view.
struct HideMember(&'static str);

impl ConsoleVisibilityPolicy for HideMember {
    fn member_visible(&self, member: &ConsoleMember) -> bool {
        member.agent_identity != self.0
    }
}

/// A read whose runtime registers again between two frames decides the
/// second frame against the new registration: it lists the new
/// registration's members once and decides as a read of that frame alone.
/// RED when `FrameVisibilityRead::cut` ignores the registration id: the
/// second frame reuses the first registration's members and its policy, so
/// the hidden member's frame shows and the new registration is never listed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_takes_a_new_cut_when_its_runtime_registers_again() {
    let runtime = build_empty_runtime(&unique_console_mob_id("member-cut-reregister")).await;
    spawn_trusted_identity_projected_member(&runtime, spec("agent-a", &[])).await;
    let first = runtime_entry_for_test(RUNTIME, &runtime);
    let second = RuntimeEntry {
        mob: None,
        registration_id: uuid::Uuid::new_v4(),
        visibility_policy: Arc::new(HideMember("agent-a")),
        ..first.clone()
    };
    let identity_records: Vec<_> = {
        let mut records = Vec::new();
        for resolved in member_sources_for_entry_including_hidden(&first).await {
            records.extend(identity_record_for_resolved_member(&resolved).await);
        }
        records
    };
    let member_frame = frame("test/agent-a", None);
    let aggregator = MetaConsoleAggregator::in_memory();
    let register = |entry: &RuntimeEntry| {
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(RUNTIME.to_string(), entry.clone());
    };

    register(&first);
    let mut read = FrameVisibilityRead::default();
    frame_is_visible_cached(
        &aggregator.inner,
        &frame("test/ghost", None),
        false,
        &mut read,
        &identity_records,
    )
    .await
    .expect("decided under the first registration");
    register(&second);
    let in_the_read = frame_is_visible_cached(
        &aggregator.inner,
        &member_frame,
        false,
        &mut read,
        &identity_records,
    )
    .await
    .expect("decided under the second registration");

    assert_eq!(
        member_cut::listings::count(
            second.registration_id,
            member_cut::listings::Listing::Primary
        ),
        1,
        "the second registration is listed once"
    );
    let alone = frame_is_visible(&aggregator.inner, &member_frame, false, &identity_records)
        .await
        .expect("decided alone");
    assert_eq!(in_the_read, alone);
    assert!(!in_the_read, "the second registration hides the member");
    let _ = runtime.mob_handle().stop().await;
}

/// A stream's page (frames read as stored) that decides a frame of an
/// identity the identity read model does not know asks the model to
/// refresh, as the stream's live frames do: the member then lists in it.
/// RED on the base, and with the refresh call removed: the model stays as
/// it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stream_page_with_an_unknown_identity_refreshes_the_identity_read_model() {
    let runtime = build_empty_runtime(&unique_console_mob_id("member-cut-refresh")).await;
    spawn_trusted_identity_projected_member(&runtime, spec("agent-a", &[])).await;
    let entry = runtime_entry_for_test(RUNTIME, &runtime);
    let aggregator = aggregator_over(&entry);
    aggregator
        .store()
        .append_if_absent(NewConsoleFrame {
            mob_id: None,
            id: None,
            dedupe_key: "unknown-identity-frame".to_string(),
            timestamp_ms: 1,
            runtime_key: RUNTIME.to_string(),
            identity: "test/agent-a".to_string(),
            conversation_id: Some("test/agent-a".to_string()),
            session_id: None,
            kind: "text_complete".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({ "text": "hello" }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::ConsoleEvent,
                source_cursor: None,
            },
            source_event_id: None,
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        })
        .await
        .expect("append");
    assert!(
        aggregator
            .inner
            .identity_read_model
            .current()
            .await
            .is_empty(),
        "the read model does not know the member yet"
    );

    let page = aggregator
        .query_timeline_windowed_stored_for(
            ConsoleTimelineWindowQuery {
                after: Some(ConsoleCursor::from_seq(0)),
                limit: 1_000,
                ..ConsoleTimelineWindowQuery::default()
            },
            None,
        )
        .await
        .expect("a stream page");
    assert!(
        page.frames.is_empty(),
        "an unknown identity's frame is hidden"
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let known = aggregator
            .inner
            .identity_read_model
            .current()
            .await
            .iter()
            .any(|record| record.identity == "test/agent-a");
        if known {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the page did not refresh the identity read model"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let _ = runtime.mob_handle().stop().await;
}

/// A mob a seated member of a primary runtime's mob made, as a console's
/// listing reads it: built over the primary's session service, with a
/// creation record that names that member as it is seated there (at its
/// generation, on its session). The primary's gateway holds it once a row
/// puts it there.
pub(super) struct MemberMadeMob {
    handle: MobHandle,
}

impl MemberMadeMob {
    /// The mob `mob_id`, offering the profiles `worker` and `delegate`,
    /// made by `creator`, a seated member of `primary`'s mob.
    pub(super) async fn made_by(primary: &UnifiedRuntime, creator: &str, mob_id: &str) -> Self {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true

[profiles.delegate]
model = "gpt-5.5"
external_addressable = true

[profiles.delegate.tools]
comms = true
"#
        ))
        .expect("definition parses");
        let primary_handle = primary.mob_handle();
        let creator = crate::member_comms_id::mob_member_id(creator);
        let seated = primary_handle
            .get_member(&creator)
            .await
            .expect("the roster reads")
            .expect("the creating member is seated");
        let state = primary
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the primary runtime exposes its mob state");
        let handle = meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
            .with_session_service(state.session_service())
            .allow_ephemeral_sessions(true)
            .with_default_llm_client(Arc::new(meerkat_client::TestClient::for_provider(
                meerkat_core::Provider::OpenAI,
            )))
            .with_provenance(meerkat_mob::MobProvenance::OperatorMember {
                parent_mob_id: primary_handle.mob_id().clone(),
                parent_incarnation: primary_handle.incarnation(),
                creator_identity: creator,
                creator_generation: seated.generation,
                owner_session: seated
                    .bridge_session_id()
                    .cloned()
                    .expect("the creating member has a session"),
            })
            .create()
            .await
            .expect("the member's mob is built");
        Self { handle }
    }

    /// The mob's handle.
    pub(super) fn mob_handle(&self) -> MobHandle {
        self.handle.clone()
    }

    /// Seat a member as [`spawn_trusted_identity_projected_member`] seats one
    /// in a runtime's own mob.
    pub(super) async fn seat(&self, mut spec: SpawnMemberSpec) {
        let alias = spec.identity.to_string();
        spec.identity = crate::member_comms_id::mob_member_id(&alias);
        self.handle
            .spawn_spec(spec)
            .await
            .expect("trusted identity-projected member spawns");
    }

    /// Have the record of who each mob is private to read this mob, which
    /// the row put into `primary`'s gateway with no create admission: a
    /// listing of a mob that record has not read withholds the mob's
    /// members and starts the read in the background.
    pub(super) async fn read_by_the_owner_record_of(&self, primary: &UnifiedRuntime) {
        primary
            .mob_runtime()
            .private_owners()
            .read_mob(&self.handle)
            .await
            .expect("the owner record reads the member's mob");
    }
}

/// A runtime whose mob offers the profile `worker` and `delegate`, the
/// profile the helpers of the `delegate` tool run under.
pub(super) async fn build_runtime_with_delegate_profile(mob_id: &str) -> UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true

[profiles.delegate]
model = "gpt-5.5"
external_addressable = true

[profiles.delegate.tools]
comms = true
"#
    ))
    .expect("definition parses");
    UnifiedRuntime::builder()
        .definition(definition)
        .default_llm_client(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )))
        .build()
        .await
        .expect("runtime builds")
}

/// A policy that does not include implicit delegate members hides, in a mob
/// nested under the primary mob, the members with the role `delegate` and no
/// other member: every read lists the nested mob's other members and decides
/// them as it decides the primary mob's. Such a policy hides no other
/// member of a nested mob.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_policy_hiding_implicit_delegates_hides_only_them_in_a_nested_mob() {
    let primary = build_empty_runtime(&unique_console_mob_id("member-cut-nested")).await;
    spawn_trusted_identity_projected_member(&primary, spec("worker", &[])).await;
    let nested_id = unique_console_mob_id("member-cut-nested-team");
    // The nested mob is one the primary mob's `worker` made.
    let nested = MemberMadeMob::made_by(&primary, "worker", &nested_id).await;
    nested.seat(spec("helper", &[])).await;
    nested
        .seat(SpawnMemberSpec::from_wire(
            "delegate".to_string(),
            "aide".to_string(),
            Some("You are aide.".into()),
            None,
            None,
        ))
        .await;
    primary
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the primary runtime exposes its mob state")
        .mob_insert_handle(
            meerkat_mob::ids::MobId::from(nested_id.as_str()),
            nested.mob_handle().clone(),
        )
        .await;
    nested.read_by_the_owner_record_of(&primary).await;
    let entry = RuntimeEntry {
        mob: None,
        registration_id: uuid::Uuid::new_v4(),
        runtime_key: RUNTIME.to_string(),
        identity_namespace: String::new(),
        runtime: primary.mob_runtime().clone(),
        identity_runtime: None,
        identity_owner: None,
        memory_panel: None,
        console_events: primary.console_events(),
        visibility_policy: Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    };
    let names = |members: Vec<ResolvedConsoleMember>| -> BTreeSet<String> {
        members
            .into_iter()
            .map(|member| member.runtime_identity)
            .collect()
    };
    let owned = |names: &[&str]| -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    };

    assert_eq!(
        names(member_sources_for_entry(&entry).await),
        owned(&["helper", "worker"]),
        "the member listing of sends, transcripts and streams"
    );
    assert_eq!(
        names(member_sources_for_entry_including_hidden(&entry).await),
        owned(&["aide", "helper", "worker"]),
        "the listing with the hidden members"
    );

    let cut = MemberCut::new(&entry);
    let mut decided = BTreeMap::new();
    for member in cut.members().await {
        let own = member.record().await.expect("a listed member's record");
        decided.insert(
            member.resolved.runtime_identity.clone(),
            resolved_member_visible_in(&cut, &member.resolved, own).await,
        );
    }
    assert_eq!(
        decided,
        BTreeMap::from([
            ("aide".to_string(), false),
            ("helper".to_string(), true),
            ("worker".to_string(), true),
        ]),
        "each member as a read decides it"
    );
    assert!(
        !frame_matches_hidden_member_in(&cut, &frame(&format!("{nested_id}/helper"), None)).await,
        "a frame of the nested mob's member is not a hidden member's"
    );
    assert!(
        frame_matches_hidden_member_in(&cut, &frame(&format!("{nested_id}/aide"), None)).await,
        "a frame of the implicit delegate member is a hidden member's"
    );
    for bare in ["helper", "aide"] {
        assert!(
            frame_matches_hidden_member_in(&cut, &frame(bare, None)).await,
            "a frame under the bare name {bare} of a nested member is not served"
        );
    }

    let aggregator = aggregator_over(&entry);
    let identities = aggregator.list_identities().await.expect("identities");
    let listed: BTreeSet<_> = identities
        .iter()
        .map(|record| record.identity.clone())
        .collect();
    assert_eq!(
        listed,
        BTreeSet::from([format!("{nested_id}/helper"), "worker".to_string()])
    );
    let _ = nested.mob_handle().stop().await;
    let _ = primary.mob_handle().stop().await;
}

/// A primary mob with `planner` and `researcher`, and a mob its `planner`
/// made, which the same gateway holds beside it, with a `researcher` and a
/// `helper` (both workers) and an `aide` (an implicit delegate member),
/// read by a console with `policy`.
struct Namesakes {
    primary: UnifiedRuntime,
    nested: MemberMadeMob,
    nested_id: String,
    entry: RuntimeEntry,
}

impl Namesakes {
    async fn build(tag: &str, policy: Arc<dyn ConsoleVisibilityPolicy>) -> Self {
        let primary = build_empty_runtime(&unique_console_mob_id(tag)).await;
        for member in ["planner", "researcher"] {
            spawn_trusted_identity_projected_member(&primary, spec(member, &[])).await;
        }
        let nested_id = unique_console_mob_id(&format!("{tag}-team"));
        let nested = MemberMadeMob::made_by(&primary, "planner", &nested_id).await;
        for member in ["researcher", "helper"] {
            nested.seat(spec(member, &[])).await;
        }
        nested
            .seat(spec_with_profile("delegate", "aide", &[]))
            .await;
        primary
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the primary runtime exposes its mob state")
            .mob_insert_handle(
                meerkat_mob::ids::MobId::from(nested_id.as_str()),
                nested.mob_handle().clone(),
            )
            .await;
        nested.read_by_the_owner_record_of(&primary).await;
        let entry = RuntimeEntry {
            mob: None,
            registration_id: uuid::Uuid::new_v4(),
            runtime_key: RUNTIME.to_string(),
            identity_namespace: String::new(),
            runtime: primary.mob_runtime().clone(),
            identity_runtime: None,
            identity_owner: None,
            memory_panel: None,
            console_events: primary.console_events(),
            visibility_policy: policy,
        };
        Self {
            primary,
            nested,
            nested_id,
            entry,
        }
    }

    /// The nested mob's member `name` as the console names it.
    fn nested_identity(&self, name: &str) -> String {
        format!("{}/{name}", self.nested_id)
    }

    /// Whether `handle`'s roster holds `name` as an active member.
    async fn active(handle: &MobHandle, name: &str) -> bool {
        handle
            .list_members_observation_snapshot()
            .await
            .iter()
            .any(|member| {
                member.agent_identity.as_str() == name
                    && member.status == meerkat_mob::MobMemberStatus::Active
            })
    }

    async fn stop(self) {
        let _ = self.nested.mob_handle().stop().await;
        let _ = self.primary.mob_handle().stop().await;
    }
}
/// A policy written for the release before this change that hides implicit
/// delegate members (it answers that switch and no method added since) keeps
/// hiding every member
/// of every nested mob on every read: the member listing of sends,
/// transcripts and streams, each member as a read decides it, frames written
/// by a nested member, the identities read, and the actions that name one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_policy_written_before_this_change_hides_every_nested_member_on_every_read() {
    let policy: Arc<dyn ConsoleVisibilityPolicy> = Arc::new(FixturePolicy {
        include_delegates: false,
    });
    let realm = Namesakes::build("old-policy", policy.clone()).await;
    let primary_mob_id = realm.primary.mob_handle().mob_id().to_string();

    let listed: BTreeSet<String> = member_sources_for_entry(&realm.entry)
        .await
        .into_iter()
        .map(|member| (member.source_mob_id, member.runtime_identity))
        .map(|(mob, name)| format!("{mob}/{name}"))
        .collect();
    assert_eq!(
        listed,
        BTreeSet::from([
            format!("{primary_mob_id}/planner"),
            format!("{primary_mob_id}/researcher"),
        ]),
        "the member listing of sends, transcripts and streams"
    );

    let cut = MemberCut::new(&realm.entry);
    for member in cut.members().await {
        if member.resolved.source_mob_id == primary_mob_id {
            continue;
        }
        let own = member.record().await.expect("a listed member's record");
        assert!(
            !resolved_member_visible_in(&cut, &member.resolved, own).await,
            "{} is hidden as a read decides it",
            member.resolved.runtime_identity
        );
        assert!(
            !retained_member_visible(
                policy.as_ref(),
                &ConsoleFrameMemberProvenance {
                    identity: own.clone(),
                    member: console_member_for_resolved_member(&member.resolved, own),
                    primary_mob_id: primary_mob_id.clone(),
                    source_mob_id: member.resolved.source_mob_id.clone(),
                    lineage: Vec::new(),
                },
            ),
            "a frame {} wrote, earlier ones included, stays hidden",
            member.resolved.runtime_identity
        );
        assert!(
            frame_matches_hidden_member_in(&cut, &frame(&own.identity, None)).await,
            "a frame under {} is a hidden member's",
            own.identity
        );
    }

    let aggregator = aggregator_over(&realm.entry);
    let identities: BTreeSet<String> = aggregator
        .list_identities()
        .await
        .expect("identities")
        .into_iter()
        .map(|record| record.identity)
        .collect();
    assert_eq!(
        identities,
        BTreeSet::from(["planner".to_string(), "researcher".to_string()])
    );
    for identity in [
        "helper".to_string(),
        realm.nested_identity("helper"),
        realm.nested_identity("researcher"),
    ] {
        assert!(
            aggregator
                .inspect_identity(&identity)
                .await
                .expect("inspect answers")
                .is_none(),
            "inspect of {identity}"
        );
        assert!(
            aggregator
                .resolve_send_member(&identity)
                .await
                .expect("a send resolves")
                .is_none(),
            "a send to {identity}"
        );
        assert!(
            !aggregator
                .retire_identity(&identity)
                .await
                .expect("retire answers"),
            "retire of {identity}"
        );
    }
    for member in ["researcher", "helper", "aide"] {
        assert!(
            Namesakes::active(&realm.nested.mob_handle(), member).await,
            "{member} is left alone"
        );
    }
    realm.stop().await;
}

/// On the stock console a bare member name names the primary mob's member,
/// as in the release before this change, whatever a nested mob holds under
/// the same name: inspect, the
/// member a send resolves and retire all reach the primary member, and none
/// answers "ambiguous".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bare_name_names_the_primary_mobs_member_whatever_a_nested_mob_holds() {
    let realm = Namesakes::build(
        "bare-name",
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    )
    .await;
    let aggregator = aggregator_over(&realm.entry);
    let primary_mob_id = realm.primary.mob_handle().mob_id().to_string();

    let inspected = aggregator
        .inspect_identity("researcher")
        .await
        .expect("inspect answers, not ambiguous")
        .expect("the primary member is found");
    assert_eq!(inspected.identity.identity, "researcher");
    assert!(
        !inspected.identity.labels.contains_key("source_mob_id"),
        "the primary mob's member: {:?}",
        inspected.identity.labels
    );
    let target = aggregator
        .resolve_send_member("researcher")
        .await
        .expect("a send resolves, not ambiguous")
        .expect("a send finds the primary member");
    assert_eq!(target.source_mob_id, primary_mob_id);
    assert!(
        aggregator
            .retire_identity("researcher")
            .await
            .expect("retire answers, not ambiguous"),
        "retire reaches the primary member"
    );
    assert!(
        !Namesakes::active(&realm.primary.mob_handle(), "researcher").await,
        "the primary member is retired"
    );
    assert!(
        Namesakes::active(&realm.nested.mob_handle(), "researcher").await,
        "the nested namesake is left alone"
    );
    realm.stop().await;
}

/// A nested mob's member is addressed by its mob and name, the identity the
/// console lists it under: inspect, the member a send resolves and retire
/// reach that member and no namesake.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_member_is_addressed_by_its_mob_and_name() {
    let realm = Namesakes::build(
        "mob-and-name",
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    )
    .await;
    let aggregator = aggregator_over(&realm.entry);
    let namesake = realm.nested_identity("researcher");

    let listed: BTreeSet<String> = aggregator
        .list_identities()
        .await
        .expect("identities")
        .into_iter()
        .map(|record| record.identity)
        .collect();
    assert_eq!(
        listed,
        BTreeSet::from([
            "planner".to_string(),
            "researcher".to_string(),
            namesake.clone(),
            realm.nested_identity("helper"),
        ]),
        "each member once, the nested ones under their mob; the implicit delegate hidden"
    );
    let inspected = aggregator
        .inspect_identity(&namesake)
        .await
        .expect("inspect answers")
        .expect("the nested member is found");
    assert_eq!(inspected.identity.identity, namesake);
    assert_eq!(
        inspected.identity.labels.get("source_mob_id"),
        Some(&realm.nested_id)
    );
    let target = aggregator
        .resolve_send_member(&namesake)
        .await
        .expect("a send resolves")
        .expect("a send finds the nested member");
    assert_eq!(target.source_mob_id, realm.nested_id);
    assert!(
        aggregator
            .retire_identity(&namesake)
            .await
            .expect("retire answers"),
        "retire reaches the nested member"
    );
    assert!(
        !Namesakes::active(&realm.nested.mob_handle(), "researcher").await,
        "the nested member is retired"
    );
    assert!(
        Namesakes::active(&realm.primary.mob_handle(), "researcher").await,
        "the primary namesake is left alone"
    );
    realm.stop().await;
}

/// A nested mob's member named by its bare name is unknown: a bare name
/// names a primary member only, so no action reaches a nested member through
/// one, also when no primary member has the name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_member_named_bare_is_unknown() {
    let realm = Namesakes::build(
        "bare-nested",
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    )
    .await;
    let aggregator = aggregator_over(&realm.entry);

    assert!(
        aggregator
            .inspect_identity("helper")
            .await
            .expect("inspect answers")
            .is_none(),
        "inspect finds no member"
    );
    assert!(
        aggregator
            .resolve_send_member("helper")
            .await
            .expect("a send resolves")
            .is_none(),
        "a send finds no member"
    );
    assert!(
        !aggregator
            .retire_identity("helper")
            .await
            .expect("retire answers"),
        "retire finds no member"
    );
    assert!(
        Namesakes::active(&realm.nested.mob_handle(), "helper").await,
        "the nested member is left alone"
    );
    realm.stop().await;
}

/// A temporary council's participant is reached by no read, on a console
/// whose policy includes nested members too: the council seats it in a mob
/// it creates for a fresh session, so the mob's creation record names no
/// member and the mob is not listed. The participant is absent from the
/// member listing and the identities read, is not found by inspect, by a
/// send or by retire under any name, gets no history read, and a frame it
/// wrote before (stored under its bare name, with or without its session) is
/// not served. The member it was forked from is reached as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_council_participant_is_reached_by_no_read_on_a_console_that_lists_nested_members() {
    let primary = build_empty_runtime(&unique_console_mob_id("council-source")).await;
    spawn_trusted_identity_projected_member(&primary, spec("source", &[])).await;
    let state = primary
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the primary runtime exposes its mob state");
    let council_id = unique_console_mob_id("council-seats");
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{council_id}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#
    ))
    .expect("definition parses");
    // As the council coordinator creates its mob: for a fresh session no
    // roster names.
    let created = Box::pin(state.mob_create_definition_with_owner_bridge_session(
        definition,
        SessionId::new(),
        false,
        false,
    ))
    .await
    .expect("the council's mob is created");
    let council = Box::pin(state.handle_for(&created))
        .await
        .expect("the council's mob is held");
    let mut participant = SpawnMemberSpec::new(
        meerkat_mob::ProfileName::from("worker"),
        meerkat_mob::AgentIdentity::from("panelist"),
    );
    participant.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    council
        .ensure_member(participant)
        .await
        .expect("the participant is seated");
    let policy: Arc<dyn ConsoleVisibilityPolicy> = Arc::new(AllowAllConsoleVisibilityPolicy);
    let entry = RuntimeEntry {
        mob: None,
        registration_id: uuid::Uuid::new_v4(),
        runtime_key: RUNTIME.to_string(),
        identity_namespace: String::new(),
        runtime: primary.mob_runtime().clone(),
        identity_runtime: None,
        identity_owner: None,
        memory_panel: None,
        console_events: primary.console_events(),
        visibility_policy: policy.clone(),
    };
    let primary_mob_id = primary.mob_handle().mob_id().to_string();
    let qualified = format!("{council_id}/panelist");

    assert!(
        member_sources_for_entry(&entry)
            .await
            .iter()
            .all(|member| member.source_mob_id == primary_mob_id),
        "the member listing of sends, transcripts and history reads"
    );
    let aggregator = aggregator_over(&entry);
    let identities: BTreeSet<String> = aggregator
        .list_identities()
        .await
        .expect("identities")
        .into_iter()
        .map(|record| record.identity)
        .collect();
    assert_eq!(identities, BTreeSet::from(["source".to_string()]));
    for name in ["panelist", qualified.as_str()] {
        assert!(
            aggregator
                .inspect_identity(name)
                .await
                .expect("inspect answers")
                .is_none(),
            "inspect of {name}"
        );
        assert!(
            aggregator
                .resolve_send_member(name)
                .await
                .expect("a send resolves")
                .is_none(),
            "a send to {name}"
        );
        assert!(
            !aggregator
                .retire_identity(name)
                .await
                .expect("retire answers"),
            "retire of {name}"
        );
        assert!(
            session_backfill_targets_for_identity_of(&aggregator.inner, None, name)
                .await
                .is_empty(),
            "no history read of {name}"
        );
    }

    // Frames a console that listed the participant stored before: under its
    // bare name, captured with its mob.
    let cut = MemberCut::new(&entry);
    let mut participant_session = None;
    for member in cut.members().await {
        if member.resolved.source_mob_id != council_id {
            continue;
        }
        let own = member.record().await.expect("the participant's record");
        participant_session = own.session_id.clone();
        let mut captured_before = own.clone();
        captured_before.identity = "panelist".to_string();
        captured_before.runtime_member_id = "panelist".to_string();
        assert!(
            !retained_member_visible(
                policy.as_ref(),
                &ConsoleFrameMemberProvenance {
                    member: console_member_for_resolved_member(&member.resolved, &captured_before),
                    identity: captured_before,
                    primary_mob_id: primary_mob_id.clone(),
                    source_mob_id: council_id.clone(),
                    lineage: Vec::new(),
                },
            ),
            "a stored frame captured with the participant is not served"
        );
    }
    assert!(
        participant_session.is_some(),
        "the participant has a session"
    );
    for stored in [
        frame("panelist", None),
        frame("panelist", participant_session.as_deref()),
        frame(&qualified, participant_session.as_deref()),
    ] {
        assert!(
            frame_matches_hidden_member_in(&cut, &stored).await,
            "a stored frame of {} (session {:?}) is a hidden member's",
            stored.identity,
            stored.session_id
        );
    }

    // The member it was forked from is unaffected.
    assert!(
        aggregator
            .inspect_identity("source")
            .await
            .expect("inspect answers")
            .is_some()
    );
    assert!(
        aggregator
            .resolve_send_member("source")
            .await
            .expect("a send resolves")
            .is_some()
    );
    assert!(!frame_matches_hidden_member_in(&cut, &frame("source", None)).await);
    let _ = primary.mob_handle().stop().await;
}

/// A mob whose id is the identity namespace its runtime is served under is
/// not listed: its members' identities (`<mob>/<name>`) would be the
/// namespaced identities of the runtime's own members. Every read lists the
/// runtime's own members alone, each once and as itself, no request reaches
/// a member of that mob, and a frame such a member wrote is no frame of the
/// runtime's own member of the same name. Under the stock empty namespace
/// the same mob is listed as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mob_whose_id_is_its_runtimes_namespace_is_not_listed() {
    let policy: Arc<dyn ConsoleVisibilityPolicy> =
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy);
    let mut realm = Namesakes::build("namespace-mob", policy.clone()).await;
    let namespace = realm.nested_id.clone();
    realm.entry.identity_namespace.clone_from(&namespace);
    let primary_mob_id = realm.primary.mob_handle().mob_id().to_string();
    let namespaced = |name: &str| format!("{namespace}/{name}");

    assert!(
        member_sources_for_entry(&realm.entry)
            .await
            .iter()
            .all(|member| member.source_mob_id == primary_mob_id),
        "the member listing of sends, transcripts and streams holds the runtime's own members \
         alone"
    );
    let aggregator = aggregator_over(&realm.entry);
    let records = aggregator.list_identities().await.expect("identities");
    let mut listed: Vec<String> = records
        .iter()
        .map(|record| record.identity.clone())
        .collect();
    listed.sort();
    assert_eq!(
        listed,
        vec![namespaced("planner"), namespaced("researcher")],
        "each member of the runtime's own mob once, and no member of the mob named as the \
         namespace"
    );
    assert!(
        records
            .iter()
            .all(|record| !record.labels.contains_key("source_mob_id")),
        "{records:#?}"
    );
    let inspected = aggregator
        .inspect_identity(&namespaced("researcher"))
        .await
        .expect("inspect answers")
        .expect("the runtime's own member is found");
    assert!(
        !inspected.identity.labels.contains_key("source_mob_id"),
        "the runtime's own member: {:?}",
        inspected.identity.labels
    );
    assert!(
        aggregator
            .inspect_identity(&namespaced("helper"))
            .await
            .expect("inspect answers")
            .is_none(),
        "a member of the mob named as the namespace is not found"
    );
    let target = aggregator
        .resolve_send_member(&namespaced("researcher"))
        .await
        .expect("a send resolves")
        .expect("a send finds the runtime's own member");
    assert_eq!(target.source_mob_id, primary_mob_id);

    // Frames under the one identity the two namesakes would share.
    let cut = MemberCut::new(&realm.entry);
    let mut sessions = BTreeMap::new();
    for member in cut.members().await {
        if member.resolved.runtime_identity == "researcher" {
            let own = member.record().await.expect("a member's record");
            sessions.insert(member.resolved.nested(), own.session_id.clone());
        }
    }
    let own_session = sessions
        .get(&false)
        .cloned()
        .flatten()
        .expect("the runtime's own member has a session");
    let nested_session = sessions
        .get(&true)
        .cloned()
        .flatten()
        .expect("the nested namesake has a session");
    assert!(
        !frame_matches_hidden_member_in(
            &cut,
            &frame(&namespaced("researcher"), Some(&own_session))
        )
        .await,
        "a frame of the runtime's own member is served"
    );
    assert!(
        frame_matches_hidden_member_in(
            &cut,
            &frame(&namespaced("researcher"), Some(&nested_session))
        )
        .await,
        "a frame the nested namesake wrote is no frame of the runtime's own member"
    );
    realm.stop().await;

    // The stock empty namespace: the same shape of mobs is listed as before.
    let realm = Namesakes::build("namespace-mob-stock", policy).await;
    let listed: BTreeSet<String> = aggregator_over(&realm.entry)
        .list_identities()
        .await
        .expect("identities")
        .into_iter()
        .map(|record| record.identity)
        .collect();
    assert_eq!(
        listed,
        BTreeSet::from([
            "planner".to_string(),
            "researcher".to_string(),
            realm.nested_identity("researcher"),
            realm.nested_identity("helper"),
        ])
    );
    realm.stop().await;
}

/// A member that a nested mob's member spawned carries its spawner as a
/// confirmed ancestor: the spawn registry names the spawner by its mob and
/// name, and the incarnation that spawned it is found under that name. Under
/// an access configuration with a role deny that applies to the caller, the
/// caller who is listed the member's row is served the member's frames too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_a_nested_member_spawned_carries_its_confirmed_spawner() {
    let realm = Namesakes::build(
        "nested-spawner",
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    )
    .await;
    // The fixture puts the nested mob into the gateway with no create
    // admission: the record of who each mob is private to reads it here (a
    // mob the `planner` made, private to no one), as it reads every mob the
    // gateway holds at its start.
    realm
        .primary
        .mob_runtime()
        .private_owners()
        .read_mob(&realm.nested.mob_handle())
        .await
        .expect("the owner record reads the nested mob");
    let primary_mob_id = realm.primary.mob_handle().mob_id().to_string();
    let spawner = realm.nested_identity("researcher");
    let child = realm.nested_identity("helper");
    let mut spawner_record = None;
    {
        let cut = MemberCut::new(&realm.entry);
        for member in cut.members().await {
            if member.resolved.console_name().as_deref() == Some(spawner.as_str()) {
                spawner_record = member.record().await.cloned();
            }
        }
    }
    let spawner_record = spawner_record.expect("the nested spawner is listed");
    let spawner_session = spawner_record
        .session_id
        .clone()
        .expect("the nested spawner has a session");
    // What the spawn registry holds for the child: its spawner, by its mob
    // and name, and the session of the incarnation that spawned it.
    realm
        .entry
        .console_events
        .register_identity_labels(
            &realm.nested_id,
            &child,
            "helper",
            BTreeMap::from([
                (
                    crate::console_spawn::SPAWNED_BY_LABEL.to_string(),
                    spawner.clone(),
                ),
                (
                    crate::console_spawn::SPAWNED_BY_SESSION_LABEL.to_string(),
                    spawner_session,
                ),
            ]),
        )
        .await;

    let cut = MemberCut::new(&realm.entry);
    let mut captured = None;
    for member in cut.members().await {
        if member.resolved.console_name().as_deref() == Some(child.as_str()) {
            let own = member.record().await.expect("the child's record");
            let lineage = capture_member_lineage_in(&cut, &member.resolved, own).await;
            captured = Some((member.resolved.clone(), own.clone(), lineage));
        }
    }
    let (resolved, own, lineage) = captured.expect("the child is listed");
    assert_eq!(lineage.len(), 2, "{lineage:#?}");
    assert_eq!(lineage[1].identity, spawner);
    assert_eq!(
        lineage[1].role.as_deref(),
        Some("worker"),
        "the spawner is captured with its attributes: {lineage:#?}"
    );
    assert_eq!(
        lineage[1]
            .labels
            .as_ref()
            .and_then(|labels| labels.get("source_mob_id")),
        Some(&realm.nested_id),
        "{lineage:#?}"
    );

    // Everyone may view every agent, and no one an agent with the role
    // `auditor`: a deny the bare identity of an ancestor cannot decide.
    let access = crate::access::AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![
            crate::access::AccessRule {
                id: "everyone-views".to_string(),
                subjects: vec!["*".to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                agents: vec!["*".to_string()],
                ..Default::default()
            },
            crate::access::AccessRule {
                id: "no-one-views-an-auditor".to_string(),
                effect: crate::access::AccessEffect::Deny,
                subjects: vec!["*".to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                roles: vec!["auditor".to_string()],
                ..Default::default()
            },
        ],
        ..Default::default()
    })
    .expect("access controller");
    // The roster priming's record of the two members.
    for (record, role) in [(&spawner_record, "worker"), (&own, "worker")] {
        access.record_agent_attributes(crate::access::AgentResourceAttributes {
            identity: record.identity.clone(),
            agent_id: Some(record.runtime_member_id.clone()),
            role: Some(role.to_string()),
            labels: record.labels.clone(),
        });
    }
    let caller = access.view_for_caller(Some("carol@example.test"), Some("person:carol"));
    assert!(
        caller.can_view_agent(&child),
        "the caller is listed the member's row"
    );
    let mut written = frame(&child, own.session_id.as_deref());
    written.source.member_provenance = Some(ConsoleFrameMemberProvenance {
        member: console_member_for_resolved_member(&resolved, &own),
        identity: own,
        primary_mob_id,
        source_mob_id: realm.nested_id.clone(),
        lineage,
    });
    assert!(
        timeline_frame_visible_to(&caller, &written),
        "the caller who is listed the member's row is served its frames"
    );
    realm.stop().await;
}

/// Which mob a member sits in and who created that mob are the console's
/// own facts for an agent with no live member too: a `source_mob_id` or a
/// `delegate_host_identity` its declaration carries is not in its record,
/// so a rule that selects a nested mob's members by the mob label does not
/// match it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_with_no_live_member_keeps_no_console_mob_label() {
    let (runtime, entry) = identity_first_realm(0).await;
    let identity_runtime = entry.identity_runtime.as_ref().expect("identity runtime");
    let mut declared = durable_spec("dormant");
    declared.labels = BTreeMap::from([
        ("source_mob_id".to_string(), "vault".to_string()),
        ("delegate_host_identity".to_string(), "planner".to_string()),
        ("tier".to_string(), "public".to_string()),
    ]);
    identity_runtime
        .register(declared)
        .await
        .expect("the identity registers");
    let statuses = identity_runtime.statuses().await.expect("statuses");
    let status = statuses
        .iter()
        .find(|status| status.identity.as_str() == "dormant")
        .expect("the dormant identity's status");
    let record = identity_record_for_status(&entry, status);
    assert_eq!(
        record.labels.get("tier").map(String::as_str),
        Some("public"),
        "a declared label of its own stays: {:?}",
        record.labels
    );
    for key in crate::console_spawn::CONSOLE_MOB_LABELS {
        assert!(
            !record.labels.contains_key(key),
            "the record carries no `{key}`: {:?}",
            record.labels
        );
    }
    let _ = runtime.mob_handle().stop().await;
}

/// The [`Namesakes`] mobs read under a namespace of two components whose
/// first is the nested mob's id, with one more member of that mob whose
/// name starts with the namespace's second component (`east/x`). That
/// member's console name is then the namespaced identity of a member `x` of
/// the runtime's own mob, and a request can spell it a second way: the
/// namespace, then its mob and name. Answers the realm and that spelling.
async fn namesakes_under_a_namespace_that_starts_with_the_mob(tag: &str) -> (Namesakes, String) {
    let mut realm = Namesakes::build(
        tag,
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    )
    .await;
    realm.nested.seat(spec("east/x", &[])).await;
    let namespace = format!("{}/east", realm.nested_id);
    let spelled_through_the_namespace = format!("{namespace}/{}/east/x", realm.nested_id);
    realm.entry.identity_namespace = namespace;
    (realm, spelled_through_the_namespace)
}

/// A mob whose id is the first component of the namespace its runtime is
/// served under is not listed, as a mob whose id is the whole namespace is
/// not: no spelling of a member of it is found by an inspection, and the
/// identities read names none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_inspection_finds_a_member_of_a_mob_named_as_the_namespaces_first_component() {
    let (realm, spelled) =
        namesakes_under_a_namespace_that_starts_with_the_mob("namespace-head-inspect").await;
    let aggregator = aggregator_over(&realm.entry);
    for identity in [
        spelled.clone(),
        format!("{}/east/x", realm.nested_id),
        format!(
            "{}/{}",
            realm.entry.identity_namespace,
            realm.nested_identity("helper")
        ),
    ] {
        assert!(
            aggregator
                .inspect_identity(&identity)
                .await
                .expect("inspect answers")
                .is_none(),
            "inspect of {identity}"
        );
    }
    let records = aggregator.list_identities().await.expect("identities");
    assert!(
        records
            .iter()
            .all(|record| !record.labels.contains_key("source_mob_id")),
        "the identities read lists the runtime's own members alone: {records:#?}"
    );
    realm.stop().await;
}

/// The same mob is reached by no send: a send to a member of it, spelled
/// through the namespace or by its mob and name, resolves no member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_send_reaches_a_member_of_a_mob_named_as_the_namespaces_first_component() {
    let (realm, spelled) =
        namesakes_under_a_namespace_that_starts_with_the_mob("namespace-head-send").await;
    let aggregator = aggregator_over(&realm.entry);
    for identity in [
        spelled.clone(),
        format!("{}/east/x", realm.nested_id),
        format!(
            "{}/{}",
            realm.entry.identity_namespace,
            realm.nested_identity("helper")
        ),
    ] {
        assert!(
            aggregator
                .resolve_send_member(&identity)
                .await
                .expect("a send resolves")
                .is_none(),
            "a send to {identity}"
        );
    }
    assert!(
        Namesakes::active(&realm.nested.mob_handle(), "helper").await,
        "the nested member is left alone"
    );
    realm.stop().await;
}

/// The labels a frame captures of its member are the member's own: a
/// label named `role` that the member carries is captured as it is, beside
/// the member's role in its mob, which the frame carries apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_captures_its_members_own_label_named_role() {
    let (runtime, entry) = identity_first_realm(0).await;
    spawn_trusted_identity_projected_member(
        &runtime,
        spec(
            "claimant",
            &[("agent_identity", "claimant"), ("role", "auditor")],
        ),
    )
    .await;
    let cut = MemberCut::new(&entry);
    let mut captured = None;
    for member in cut.members().await {
        let Some(own) = member.record().await else {
            continue;
        };
        if own.identity == "claimant" {
            captured = Some(capture_member_lineage_in(&cut, &member.resolved, own).await);
        }
    }
    let lineage = captured.expect("the member is listed");
    let own = lineage.first().expect("the member's own link");
    assert_eq!(own.role.as_deref(), Some("worker"), "{own:#?}");
    assert_eq!(
        own.labels
            .as_ref()
            .and_then(|labels| labels.get("role"))
            .map(String::as_str),
        Some("auditor"),
        "the captured labels are the member's own: {own:#?}"
    );
    let _ = runtime.mob_handle().stop().await;
}
