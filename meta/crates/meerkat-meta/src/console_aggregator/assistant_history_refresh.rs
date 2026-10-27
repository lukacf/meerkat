//! Runtime-owned permission to reconcile absent assistant occurrences.

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use meerkat_mob::MemberRunState;
use serde_json::Value;

use super::{ConsoleIdentityRecord, RuntimeEntry};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum AssistantHistoryRefreshReason {
    #[default]
    PositiveOnly,
    Recovery,
    SessionBoundary {
        session_id: String,
    },
}

impl AssistantHistoryRefreshReason {
    pub(super) fn permits_session(&self, session_id: &str) -> bool {
        match self {
            Self::PositiveOnly => false,
            Self::Recovery => !session_id.is_empty(),
            Self::SessionBoundary {
                session_id: expected,
            } => expected == session_id,
        }
    }

    pub(super) fn from_event(kind: &str, session_id: Option<&str>, payload: &Value) -> Self {
        let Some(session_id) = session_id.filter(|value| !value.is_empty()) else {
            return Self::PositiveOnly;
        };
        let run_completed = kind == "run_completed"
            || (kind == "interaction_complete"
                && (payload.get("source_event_type").and_then(Value::as_str)
                    == Some("run_completed")
                    || payload.get("type").and_then(Value::as_str) == Some("run_completed")));
        let final_completion = run_completed
            && matches!(
                payload.get("extraction_required"),
                None | Some(Value::Bool(false))
            );
        if final_completion
            || matches!(
                kind,
                "run_failed"
                    | "interaction_failed"
                    | "stream_truncated"
                    | "extraction_succeeded"
                    | "extraction_failed"
                    | "transcript_rewrite_committed"
                    | "transcript_rewrite_audit_receipt_committed"
                    | "compaction_completed"
            )
        {
            Self::SessionBoundary {
                session_id: session_id.to_string(),
            }
        } else {
            Self::PositiveOnly
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AssistantHistoryRefreshGate {
    PositiveOnly,
    /// Fresh typed observation says the member is not settled for this
    /// session yet (a run is open, input is uncommitted, or another session is
    /// current). The member's own run and commit events re-drive it.
    Pending,
    /// No fresh typed status: the bounded status read did not answer (its
    /// deadline elapsed or it failed) or carried no run state, or the run
    /// state itself is unknown. Treated like `Pending`, but an idle member
    /// emits no event of its own that would re-drive it, so the backfill arms
    /// a re-drive on the next typed change.
    StatusUnknown,
    /// The member is idle on this session but its durable work has not
    /// landed: inputs are still queued, staged or awaiting their boundary
    /// commit (a burst draining between runs, or trailing commits). Not
    /// settled, and a read now would only be read again once the queue
    /// drains, so the backfill skips it and arms a re-drive on the member's
    /// next typed change.
    Draining,
    /// A run is open on this session. Unsettled like `Pending`, so the pass
    /// still restores positive frames (a boot-time turn can run for minutes),
    /// and the backfill also arms a re-drive on the member's next typed
    /// change for the settled refresh instead of leaving it to a tick.
    Running,
    Settled,
}

/// Coalesce a recovery behind an active runtime-wide poll. Ordinary polling
/// cannot erase an explicit recovery or turn a burst into one job per trigger.
pub(super) fn admit_runtime_refresh(
    active: &mut BTreeMap<String, Option<AssistantHistoryRefreshReason>>,
    runtime_key: &str,
    reason: AssistantHistoryRefreshReason,
) -> bool {
    match active.entry(runtime_key.to_string()) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(None);
            true
        }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            if reason == AssistantHistoryRefreshReason::Recovery {
                entry.insert(Some(reason));
            }
            false
        }
    }
}

/// One bounded read of the member's typed status for the refresh gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MemberStatusRead {
    /// Fresh typed status: the member's current session and run state.
    Observed(String, MemberRunState),
    /// No member owns the exact session, or it has no current session: an
    /// observation that the member is not settled for this session.
    NotBound,
    /// The read did not answer (its deadline elapsed or it failed) or carried
    /// no run state: nothing was observed.
    NoAnswer,
}

/// Call after capturing the console prefix and before reading current history.
/// Even a terminal trigger checks current runtime progress: another run may
/// have started before its queued backfill acquired the projection lock.
pub(super) async fn observe(
    entry: &RuntimeEntry,
    record: &ConsoleIdentityRecord,
    session_id: &str,
    requested: bool,
) -> AssistantHistoryRefreshGate {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    observe_with(
        requested,
        session_id,
        || {
            Box::pin(read_member_status(
                entry, record, session_id, deadline, false,
            ))
        },
        || async {
            tokio::time::timeout_at(deadline, entry.runtime.session_commit_pending(session_id))
                .await
                .ok()
                .flatten()
        },
        || async {
            tokio::time::timeout_at(
                deadline,
                Box::pin(entry.runtime.session_has_active_inputs(session_id)),
            )
            .await
            .ok()
            .flatten()
        },
    )
    .await
}

/// One bounded read of the member's typed status for `session_id`. With
/// `require_running_mob`, a member whose owning mob is not running (stopped,
/// completed, destroyed) reads `NotBound`: it keeps no live queue there.
async fn read_member_status(
    entry: &RuntimeEntry,
    record: &ConsoleIdentityRecord,
    session_id: &str,
    deadline: tokio::time::Instant,
    require_running_mob: bool,
) -> MemberStatusRead {
    let member =
        crate::member_comms_id::roster_member_id_for_supplied_id(&record.runtime_member_id);
    tokio::time::timeout_at(deadline, async {
        let Ok(expected_session) = meerkat_core::SessionId::parse(session_id) else {
            return MemberStatusRead::NotBound;
        };
        let primary = entry.runtime.handle();
        let (handle, member) = if primary.resolve_bridge_session_id(&member).await
            == Some(expected_session.clone())
        {
            (primary, member)
        } else {
            // Alias and source labels cannot distinguish identical
            // member names in different mobs. Recover the handle from
            // the existing member resolver and its runtime binding.
            let mut owner = None;
            for resolved in Box::pin(super::member_sources_for_entry_including_hidden(entry)).await
            {
                if resolved.member.agent_identity != member
                    && resolved.runtime_identity != record.runtime_member_id
                {
                    continue;
                }
                if resolved
                    .handle
                    .resolve_bridge_session_id(&resolved.member.agent_identity)
                    .await
                    != Some(expected_session.clone())
                {
                    continue;
                }
                if owner.is_some() {
                    return MemberStatusRead::NotBound;
                }
                owner = Some((resolved.handle, resolved.member.agent_identity));
            }
            let Some(owner) = owner else {
                return MemberStatusRead::NotBound;
            };
            owner
        };
        // The binding only routes the observation. Fresh typed status
        // must still prove that this exact session is idle.
        let Ok(status) = crate::member_status_observation::observe_member_status_until(
            &handle, &member, deadline,
        )
        .await
        else {
            return MemberStatusRead::NoAnswer;
        };
        let Some(current_session) = status.current_session_id else {
            return MemberStatusRead::NotBound;
        };
        if require_running_mob
            && !matches!(handle.status().await, Ok(meerkat_mob::MobState::Running))
        {
            return MemberStatusRead::NotBound;
        }
        match status.progress {
            Some(progress) => {
                MemberStatusRead::Observed(current_session.to_string(), progress.run_state)
            }
            None => MemberStatusRead::NoAnswer,
        }
    })
    .await
    .unwrap_or(MemberStatusRead::NoAnswer)
}

async fn observe_with<
    StatusRead,
    StatusFuture,
    CommitRead,
    CommitFuture,
    ActiveRead,
    ActiveFuture,
>(
    requested: bool,
    session_id: &str,
    read_status: StatusRead,
    read_commit: CommitRead,
    read_active: ActiveRead,
) -> AssistantHistoryRefreshGate
where
    StatusRead: FnOnce() -> StatusFuture,
    StatusFuture: Future<Output = MemberStatusRead>,
    CommitRead: FnOnce() -> CommitFuture,
    CommitFuture: Future<Output = Option<bool>>,
    ActiveRead: FnOnce() -> ActiveFuture,
    ActiveFuture: Future<Output = Option<bool>>,
{
    if !requested {
        return AssistantHistoryRefreshGate::PositiveOnly;
    }
    let current_session = match read_status().await {
        MemberStatusRead::Observed(current_session, MemberRunState::Idle) => current_session,
        MemberStatusRead::Observed(current_session, MemberRunState::RunOpen) => {
            return if current_session == session_id {
                AssistantHistoryRefreshGate::Running
            } else {
                AssistantHistoryRefreshGate::Pending
            };
        }
        MemberStatusRead::NotBound => return AssistantHistoryRefreshGate::Pending,
        MemberStatusRead::Observed(_, MemberRunState::Unknown) | MemberStatusRead::NoAnswer => {
            return AssistantHistoryRefreshGate::StatusUnknown;
        }
    };
    if current_session != session_id {
        return AssistantHistoryRefreshGate::Pending;
    }
    // Idle between runs is not settled while durable work is still landing:
    // a queued burst drains run by run, and each run's commits trail it.
    match (read_commit().await, read_active().await) {
        (Some(false), Some(false)) => AssistantHistoryRefreshGate::Settled,
        (Some(true), _) | (_, Some(true)) => AssistantHistoryRefreshGate::Draining,
        _ => AssistantHistoryRefreshGate::Pending,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::future::ready;

    use super::*;

    #[test]
    fn eligibility_requires_an_explicit_same_session_boundary() {
        let recovery = AssistantHistoryRefreshReason::Recovery;
        let boundary = AssistantHistoryRefreshReason::SessionBoundary {
            session_id: "session-a".into(),
        };
        assert!(recovery.permits_session("session-a"));
        assert!(boundary.permits_session("session-a"));
        assert!(!boundary.permits_session("session-b"));
        assert!(!AssistantHistoryRefreshReason::PositiveOnly.permits_session("session-a"));
    }

    #[test]
    fn runtime_recovery_is_queued_once_when_a_poll_already_owns_the_worker() {
        let mut active = std::collections::BTreeMap::new();
        assert!(admit_runtime_refresh(
            &mut active,
            "runtime",
            AssistantHistoryRefreshReason::PositiveOnly
        ));
        assert!(!admit_runtime_refresh(
            &mut active,
            "runtime",
            AssistantHistoryRefreshReason::Recovery
        ));
        for _ in 0..32 {
            assert!(!admit_runtime_refresh(
                &mut active,
                "runtime",
                AssistantHistoryRefreshReason::PositiveOnly
            ));
        }
        assert_eq!(active.len(), 1);
        assert_eq!(
            active.get_mut("runtime").and_then(Option::take),
            Some(AssistantHistoryRefreshReason::Recovery)
        );
        assert_eq!(active.get_mut("runtime").and_then(Option::take), None);
    }

    #[test]
    fn completion_requires_final_extraction_and_cannot_be_inferred_from_text_events() {
        let reason = |kind, payload| {
            AssistantHistoryRefreshReason::from_event(kind, Some("session-a"), &payload)
        };
        assert!(reason("interaction_complete", serde_json::json!({"source_event_type":"run_completed", "extraction_required":false})).permits_session("session-a"));
        assert!(
            !reason(
                "interaction_complete",
                serde_json::json!({"source_event_type":"run_completed", "extraction_required":true})
            )
            .permits_session("session-a")
        );
        assert!(!reason("interaction_complete", serde_json::json!({"source_event_type":"run_completed", "extraction_required":"false"})).permits_session("session-a"));
        for kind in [
            "extraction_succeeded",
            "extraction_failed",
            "run_failed",
            "stream_truncated",
            "transcript_rewrite_committed",
            "compaction_completed",
        ] {
            assert!(
                reason(kind, serde_json::json!({})).permits_session("session-a"),
                "{kind}"
            );
        }
        for kind in [
            "text_complete",
            "turn_completed",
            "turn_started",
            "text_delta",
        ] {
            assert!(
                !reason(kind, serde_json::json!({})).permits_session("session-a"),
                "{kind}"
            );
        }
        assert!(
            !AssistantHistoryRefreshReason::from_event("run_failed", None, &serde_json::json!({}))
                .permits_session("session-a")
        );
    }

    #[tokio::test]
    async fn opportunistic_reads_do_not_probe_or_authorize_absence() {
        let calls = RefCell::new(Vec::new());
        let gate = observe_with(
            false,
            "session-a",
            || {
                calls.borrow_mut().push("status");
                ready(MemberStatusRead::Observed(
                    "session-a".into(),
                    MemberRunState::Idle,
                ))
            },
            || {
                calls.borrow_mut().push("commit");
                ready(Some(false))
            },
            || {
                calls.borrow_mut().push("active");
                ready(Some(false))
            },
        )
        .await;
        assert_eq!(gate, AssistantHistoryRefreshGate::PositiveOnly);
        assert!(calls.borrow().is_empty());
    }

    /// An idle member whose queued burst is still draining (or whose commits
    /// still trail) is not settled: the gate reads `Draining` until both its
    /// active inputs and its pending commit have landed, and only an
    /// inconclusive read is `Pending`.
    #[tokio::test]
    async fn an_idle_member_with_queued_inputs_is_draining_not_settled() {
        let gate = |commit: Option<bool>, active: Option<bool>| {
            observe_with(
                true,
                "session-a",
                || {
                    ready(MemberStatusRead::Observed(
                        "session-a".into(),
                        MemberRunState::Idle,
                    ))
                },
                move || ready(commit),
                move || ready(active),
            )
        };
        assert_eq!(
            gate(Some(false), Some(true)).await,
            AssistantHistoryRefreshGate::Draining
        );
        assert_eq!(
            gate(Some(true), Some(false)).await,
            AssistantHistoryRefreshGate::Draining
        );
        assert_eq!(
            gate(Some(true), Some(true)).await,
            AssistantHistoryRefreshGate::Draining
        );
        assert_eq!(
            gate(Some(false), None).await,
            AssistantHistoryRefreshGate::Pending
        );
        assert_eq!(
            gate(None, Some(false)).await,
            AssistantHistoryRefreshGate::Pending
        );
        assert_eq!(
            gate(Some(false), Some(false)).await,
            AssistantHistoryRefreshGate::Settled
        );
    }

    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn secondary_mob_refresh_uses_the_owner_of_the_exact_session() {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "refresh-primary-{}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
mob = true
"#,
            uuid::Uuid::new_v4()
        ))
        .expect("primary definition parses");
        // The convenience builder composes the runtime-backed session service
        // and installs agent mob tools over that same authority.
        let runtime = crate::unified_runtime::UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(std::sync::Arc::new(
                meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            ))
            .build()
            .await
            .expect("runtime with agent mob control builds");
        runtime
            .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                "worker".into(),
                "agent-0".into(),
                Some("Reply that the primary is ready.".into()),
                None,
                None,
            ))
            .await
            .expect("primary member spawns");
        let entry = super::super::tests::runtime_entry_for_test("refresh-owner", &runtime);
        let primary = entry.runtime.handle();
        let state = entry
            .runtime
            .agent_mob_mcp_state()
            .expect("runtime exposes shared mob control");
        let service = entry
            .runtime
            .session_service()
            .expect("runtime exposes its session service");
        assert!(
            std::sync::Arc::ptr_eq(
                &service
                    .acquire_runtime_adapter(None)
                    .expect("acquire the parent runtime machine")
                    .expect("parent runtime machine"),
                &state
                    .session_service()
                    .acquire_runtime_adapter(None)
                    .expect("acquire the child runtime machine")
                    .expect("child runtime machine"),
            ),
            "agent-created mobs must share the parent's real runtime authority"
        );
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "refresh-child-{}"

[profiles.worker]
realm_profile = "worker"
"#,
            uuid::Uuid::new_v4()
        ))
        .expect("child definition parses");
        let mob_id = Box::pin(state.mob_create_definition(definition))
            .await
            .expect("child shares the runtime session service");
        let member = crate::member_comms_id::roster_member_id_for_identity("agent-0");
        Box::pin(state.mob_spawn_spec(
            &mob_id,
            meerkat_mob::SpawnMemberSpec::from_wire(
                "worker".into(),
                "agent-0".into(),
                Some("Reply that the child is ready.".into()),
                None,
                None,
            ),
        ))
        .await
        .expect("child member spawns with the primary member's alias");
        let child = state
            .handle_for(&mob_id)
            .await
            .expect("child handle exists");
        let row = child
            .list_members_observation_snapshot()
            .await
            .into_iter()
            .find(|row| row.agent_identity == member)
            .expect("child member is in its owner's roster");
        let mut record = super::super::identity_record_for_member(&entry, &child, &row)
            .await
            .expect("child identity projects");
        let session_id = record.session_id.clone().expect("child session exists");
        let typed_session = meerkat_core::SessionId::parse(&session_id)
            .expect("child session is a typed runtime session");
        let primary_session = primary
            .resolve_bridge_session_id_observation(&member)
            .await
            .expect("primary member with the same alias has its own session");
        assert_ne!(
            primary_session, typed_session,
            "the same alias in the primary mob must not select the child session"
        );
        // Source labels are display metadata, not authority to select a mob.
        record
            .labels
            .insert("source_mob_id".into(), primary.mob_id().to_string());

        // Spawn only acknowledges admission. Require a terminal run witness
        // and the real shared commit authority before asserting settlement.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        tokio::time::timeout_at(deadline, async {
            loop {
                let execution = service
                    .execution_snapshot(&typed_session)
                    .await
                    .expect("shared service observes the child session");
                let status = crate::member_status_observation::observe_member_status_until(
                    &child, &member, deadline,
                )
                .await
                .expect("child status is observable");
                if execution.as_ref().is_some_and(|snapshot| {
                    snapshot.turn_terminal && snapshot.terminal_run_id.is_some()
                }) && status.current_session_id.as_ref() == Some(&typed_session)
                    && status.progress.as_ref().is_some_and(|progress| {
                        progress.run_state == MemberRunState::Idle && progress.in_flight_work == 0
                    })
                    && entry.runtime.session_commit_pending(&session_id).await == Some(false)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("child's initial run commits before refreshing its history");

        assert_eq!(
            observe(&entry, &record, &session_id, true).await,
            AssistantHistoryRefreshGate::Settled,
            "a settled secondary member must authorize its own history refresh"
        );
        state
            .mob_destroy(&mob_id)
            .await
            .expect("child is destroyed");
        assert_eq!(
            observe(&entry, &record, &session_id, true).await,
            AssistantHistoryRefreshGate::Pending,
            "the surviving primary alias cannot authorize the removed child's session"
        );
        primary.stop().await.expect("primary fixture stops");
    }

    #[tokio::test]
    async fn active_successor_unknown_or_replaced_session_never_authorizes_absence() {
        // No fresh typed observation is `StatusUnknown` (re-driven on the
        // next typed change); an observed non-settled member is `Pending`.
        for (status, expected) in [
            (
                MemberStatusRead::NoAnswer,
                AssistantHistoryRefreshGate::StatusUnknown,
            ),
            (
                MemberStatusRead::NotBound,
                AssistantHistoryRefreshGate::Pending,
            ),
            (
                MemberStatusRead::Observed("session-a".into(), MemberRunState::RunOpen),
                AssistantHistoryRefreshGate::Running,
            ),
            (
                MemberStatusRead::Observed("session-b".into(), MemberRunState::RunOpen),
                AssistantHistoryRefreshGate::Pending,
            ),
            (
                MemberStatusRead::Observed("session-a".into(), MemberRunState::Unknown),
                AssistantHistoryRefreshGate::StatusUnknown,
            ),
            (
                MemberStatusRead::Observed("session-b".into(), MemberRunState::Idle),
                AssistantHistoryRefreshGate::Pending,
            ),
        ] {
            let calls = RefCell::new(Vec::new());
            let gate = observe_with(
                true,
                "session-a",
                || {
                    calls.borrow_mut().push("status");
                    ready(status)
                },
                || {
                    calls.borrow_mut().push("commit");
                    ready(Some(false))
                },
                || {
                    calls.borrow_mut().push("active");
                    ready(Some(false))
                },
            )
            .await;
            assert_eq!(gate, expected);
            assert_eq!(*calls.borrow(), ["status"]);
        }
    }

    #[tokio::test]
    async fn idle_status_precedes_commit_check_and_pending_observations_remain_retryable() {
        for pending in [Some(true), None, Some(false)] {
            let calls = RefCell::new(Vec::new());
            let gate = observe_with(
                true,
                "session-a",
                || {
                    calls.borrow_mut().push("status");
                    ready(MemberStatusRead::Observed(
                        "session-a".into(),
                        MemberRunState::Idle,
                    ))
                },
                || {
                    calls.borrow_mut().push("commit");
                    ready(pending)
                },
                || {
                    calls.borrow_mut().push("active");
                    ready(Some(false))
                },
            )
            .await;
            assert_eq!(*calls.borrow(), ["status", "commit", "active"]);
            // A pending commit is durable work still landing: draining, not
            // settled. Only an inconclusive read stays plainly pending.
            assert_eq!(
                gate,
                match pending {
                    Some(false) => AssistantHistoryRefreshGate::Settled,
                    Some(true) => AssistantHistoryRefreshGate::Draining,
                    None => AssistantHistoryRefreshGate::Pending,
                }
            );
        }
    }
}
