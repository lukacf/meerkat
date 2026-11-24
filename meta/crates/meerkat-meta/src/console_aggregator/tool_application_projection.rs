//! Host-only live observations project invocation locators, never UI data.

use super::*;
use meerkat_core::ToolApplicationObservation;
use meerkat_mcp::apps::{MCP_APPS_EXTENSION, McpAppInvocation, tool_ui_resource_uri};

/// Read the tool application observations of the member `identity` of
/// `mob` (the mob whose console events are registered here; none: the
/// default mob) and store their locators. The member is found by its name
/// among the members of that mob alone.
pub(super) fn spawn_for_identity(
    inner: Arc<AggregatorInner>,
    mob: Option<meerkat_mob::MobId>,
    identity: String,
) {
    let inner = inner.projection_owner.as_ref().unwrap_or(&inner).clone();
    let activity = BackfillActivity::begin(&inner);
    tokio::spawn(async move {
        let _activity = activity;
        // This path never acquires the history reader's semaphore or lock.
        // An ephemeral history export can wait behind a model call while the
        // actor's accepted host observations are already available.
        for target in Box::pin(session_backfill_targets_for_identity_of(
            &inner,
            mob.as_ref(),
            &identity,
        ))
        .await
        {
            let key = (
                target.entry.registration_id,
                target.entry.runtime_key.clone(),
                target.session_id.clone(),
            );
            {
                let mut active = inner.tool_application_projections.lock().await;
                match active.entry(key.clone()) {
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        entry.insert(Some(target));
                        continue;
                    }
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(None);
                    }
                }
            }
            let inner = inner.clone();
            let activity = BackfillActivity::begin(&inner);
            tokio::spawn(async move {
                let _activity = activity;
                let mut target = target;
                loop {
                    let _ = append_observations(
                        &inner,
                        &target.entry,
                        &target.record,
                        &target.session_id,
                    )
                    .await;
                    let mut active = inner.tool_application_projections.lock().await;
                    if let Some(pending) = active.get_mut(&key).and_then(Option::take) {
                        target = pending;
                        // A burst retains one trailing read of the native
                        // owner, never one task per streaming fragment.
                        drop(active);
                        continue;
                    }
                    active.remove(&key);
                    break;
                }
            });
        }
    });
}

async fn append_observations(
    inner: &Arc<AggregatorInner>,
    entry: &RuntimeEntry,
    record: &ConsoleIdentityRecord,
    session_id: &str,
) -> ConsoleLogResult<()> {
    if !runtime_entry_is_current(inner, entry) || !entry.visibility_policy.identity_visible(record)
    {
        return Ok(());
    }
    let Ok(observations) = entry
        .runtime
        .read_tool_application_observations(session_id)
        .await
    else {
        // Custom services can omit live display support. A refused native
        // observation grants neither a locator nor a tool action.
        return Ok(());
    };
    // The same registration can rotate a member's session or change its
    // visibility during the native read. Re-resolve that association rather
    // than applying policy to the stale record captured by the trigger. The
    // member is looked for in the registration's own mob.
    let mut current = Box::pin(session_backfill_targets_for_identity_of(
        inner,
        entry.mob.as_ref(),
        &record.identity,
    ))
    .await
    .into_iter()
    .filter(|target| {
        target.entry.registration_id == entry.registration_id
            && target.entry.runtime_key == entry.runtime_key
            && target.record.identity == record.identity
            && target.session_id == session_id
    });
    let Some(current_target) = current.next() else {
        return Ok(());
    };
    if current.next().is_some() || !runtime_entry_is_current(inner, &current_target.entry) {
        return Ok(());
    }
    let entry = &current_target.entry;
    let record = &current_target.record;
    for observation in observations {
        let Some(mut frame) = frame_from_observation(
            &entry.runtime_key,
            &record.identity,
            session_id,
            &observation,
        ) else {
            continue;
        };
        if !runtime_entry_is_current(inner, entry) {
            break;
        }
        frame.source.member_provenance = current_target.provenance.clone();
        // The locator is of the registration's own mob.
        frame.mob_id.clone_from(&entry.mob);
        super::append_and_emit_written_by(
            inner,
            frame,
            entry.visibility_policy.clone(),
            super::FrameWriter::Registration(entry),
        )
        .await?;
    }
    Ok(())
}

fn frame_from_observation(
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    observation: &ToolApplicationObservation,
) -> Option<NewConsoleFrame> {
    if observation.tool_call_id.is_empty() || observation.tool_name.is_empty() {
        return None;
    }
    let invocation: McpAppInvocation =
        serde_json::from_value(observation.host_metadata.get(MCP_APPS_EXTENSION)?.clone()).ok()?;
    tool_ui_resource_uri(&invocation.tool)?;
    Some(NewConsoleFrame {
        mob_id: None,
        id: None,
        dedupe_key: format!(
            "tool-application:{}",
            json!([runtime_key, identity, session_id, observation.tool_call_id])
        ),
        timestamp_ms: current_time_ms(),
        runtime_key: runtime_key.to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: Some(session_id.to_string()),
        kind: "mcp_app".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({
            "session_id": session_id,
            "tool_call_id": observation.tool_call_id,
        }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::ToolApplication,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_tool_application_refresh_does_not_wait_for_history_readers() {
        let (_directory, runtime, service) =
            super::super::tests::build_stress_runtime(1, Duration::ZERO).await;
        let aggregator = MetaConsoleAggregator::new_with_options(
            Arc::new(SqliteConsoleLogStore::in_memory().unwrap()),
            ConsoleAggregatorOptions {
                session_history_backfill_enabled: false,
                ..Default::default()
            },
        );
        let entry = super::super::tests::runtime_entry_for_test("runtime-app", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .unwrap()
            .insert(entry.runtime_key.clone(), entry);
        let identity = "test/agent-0";
        let target = session_backfill_target_for_identity(&aggregator.inner, identity)
            .await
            .expect("native member target");
        let history_lock = aggregator
            .inner
            .session_history_projection_locks
            .lock()
            .unwrap()
            .entry(session_history_watermark_runtime_key(
                "runtime-app",
                &target.session_id,
            ))
            .or_default()
            .clone();
        let _history_guard = history_lock.lock().await;
        let _history_permits = aggregator
            .inner
            .session_backfill_permits
            .clone()
            .acquire_many_owned(
                aggregator
                    .inner
                    .session_backfill_permits
                    .available_permits() as u32,
            )
            .await
            .unwrap();
        spawn_for_identity(aggregator.inner.clone(), None, identity.to_string());
        tokio::time::timeout(Duration::from_secs(5), async {
            while service.application_read_calls() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("native app observations must not wait for a history lock or permit");
        tokio::time::timeout(
            Duration::from_secs(5),
            aggregator.history_backfill_converged(),
        )
        .await
        .expect("app refresh must settle while the history reader stays blocked");
        assert_eq!(
            service.read_calls(),
            0,
            "app refresh must not request history"
        );
        runtime.shutdown().await;
    }

    struct SwitchedMemberVisibility(AtomicBool);

    impl ConsoleVisibilityPolicy for SwitchedMemberVisibility {
        fn member_visible(&self, _member: &ConsoleMember) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_tool_application_refresh_rechecks_member_visibility_after_read() {
        let (_directory, runtime, service) =
            super::super::tests::build_stress_runtime(1, Duration::ZERO).await;
        let aggregator = MetaConsoleAggregator::in_memory_with_options(ConsoleAggregatorOptions {
            session_history_backfill_enabled: false,
            ..Default::default()
        });
        let visibility = Arc::new(SwitchedMemberVisibility(AtomicBool::new(true)));
        let mut entry = super::super::tests::runtime_entry_for_test("runtime-app", &runtime);
        entry.visibility_policy = visibility.clone();
        aggregator
            .inner
            .runtimes
            .write()
            .unwrap()
            .insert(entry.runtime_key.clone(), entry);
        let gate = super::super::tests::HistoryReadGate::new();
        service.script_application_observations(super::super::tests::ScriptedApplicationRead {
            observations: vec![observation_for_test()],
            gate: gate.clone(),
        });
        spawn_for_identity(aggregator.inner.clone(), None, "test/agent-0".into());
        gate.wait_until_entered().await;
        visibility.0.store(false, Ordering::SeqCst);
        gate.release();
        tokio::time::timeout(
            Duration::from_secs(5),
            aggregator.history_backfill_converged(),
        )
        .await
        .expect("the native observation refresh should settle");
        let stored = aggregator
            .store()
            .query_frames(ConsoleTimelineQuery::default())
            .await
            .unwrap();
        assert!(
            stored.frames.is_empty(),
            "hidden member locator must not be appended"
        );
        assert_eq!(service.read_calls(), 0);
        runtime.shutdown().await;
    }

    /// The live display of a tool's application for an agent of a mob the
    /// host serves beside its default mob is stored under that mob. The
    /// console events of such a mob are registered; a turn starts in its
    /// agent. The start asks for that agent's observations, found among the
    /// mob's own agents, and the locator frame names the mob and the key of
    /// the mob's registration.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_mobs_agents_tool_application_is_stored_under_that_mob() {
        let (_directory, runtime, service) =
            super::super::tests::build_stress_runtime(1, Duration::ZERO).await;
        let aggregator = MetaConsoleAggregator::in_memory_with_options(ConsoleAggregatorOptions {
            session_history_backfill_enabled: false,
            ..Default::default()
        });
        aggregator.tell_default_mob(crate::member_key::DefaultMob::new(
            meerkat_mob::MobId::from("tool-rows-launch"),
        ));
        let mob = runtime.mob_runtime().handle().mob_id().clone();
        aggregator
            .register_mob_console_events(
                &mob,
                runtime.mob_runtime().clone(),
                runtime.console_events(),
                Arc::new(AllowAllConsoleVisibilityPolicy),
            )
            .expect("the second mob's console events register");
        let gate = super::super::tests::HistoryReadGate::new();
        service.script_application_observations(super::super::tests::ScriptedApplicationRead {
            observations: vec![observation_for_test()],
            gate: gate.clone(),
        });
        // A turn starts in the mob's agent.
        runtime
            .console_events()
            .record_lifecycle("agent-0", "turn_started", json!({}))
            .await;
        let asked = tokio::time::timeout(Duration::from_secs(5), async {
            while service.application_read_calls() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        gate.release();
        let settled = tokio::time::timeout(
            Duration::from_secs(5),
            aggregator.history_backfill_converged(),
        )
        .await
        .is_ok();
        // The frames of the mob, by the store's read of one mob's frames: a
        // read that names no mob answers the default mob's frames alone.
        let locators: Vec<ConsoleFrame> = aggregator
            .store()
            .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
                mob_id: Some(mob.clone()),
                ..Default::default()
            })
            .await
            .unwrap()
            .frames
            .into_iter()
            .filter(|frame| frame.source.kind == ConsoleFrameSourceKind::ToolApplication)
            .collect();
        runtime.shutdown().await;
        assert!(
            asked,
            "a turn that starts in a second mob's agent asks for that agent's tool applications"
        );
        assert!(settled, "the refresh settles");
        assert_eq!(
            locators
                .iter()
                .map(|frame| (
                    frame.mob_id.clone(),
                    frame.runtime_key.clone(),
                    frame.identity.clone()
                ))
                .collect::<Vec<_>>(),
            [(
                Some(mob.clone()),
                mob_runtime_key(&mob),
                "agent-0".to_string()
            )],
            "one locator frame, under the mob and the key of its registration"
        );
    }

    fn observation_for_test() -> ToolApplicationObservation {
        ToolApplicationObservation {
            tool_call_id: "call-chart".into(),
            tool_name: "show_chart".into(),
            host_metadata: BTreeMap::from([(
                MCP_APPS_EXTENSION.into(),
                json!({
                    "registration": { "server": "charts", "connection": "physical-private" },
                    "tool": { "name": "show_chart", "inputSchema": { "type": "object" },
                        "_meta": { "ui": { "resourceUri": "ui://charts/view" } } },
                    "arguments": { "query": "private-input" },
                    "result": { "content": [{ "type": "text", "text": "Chart ready" }],
                        "_meta": { "details": "private-result" } },
                    "resource": { "contents": [{ "uri": "ui://charts/view",
                        "text": "private-html", "mimeType": "text/html;profile=mcp-app" }] }
                }),
            )]),
        }
    }

    #[test]
    fn live_tool_application_frames_contain_only_native_locators() {
        let observation = observation_for_test();
        let frame =
            frame_from_observation("runtime-a", "agent-a", "session-a", &observation).unwrap();
        assert_eq!(frame.source.kind, ConsoleFrameSourceKind::ToolApplication);
        assert_eq!(frame.identity, "agent-a");
        assert_eq!(frame.session_id.as_deref(), Some("session-a"));
        assert_eq!(
            frame.payload,
            json!({ "session_id": "session-a", "tool_call_id": "call-chart" })
        );
        let serialized = serde_json::to_string(&frame).unwrap();
        for hidden in [
            "private-input",
            "private-result",
            "private-html",
            "physical-private",
        ] {
            assert!(
                !serialized.contains(hidden),
                "host-only data escaped: {hidden}"
            );
        }
        assert_ne!(
            frame.dedupe_key,
            frame_from_observation("runtime-a", "agent-a", "session-b", &observation)
                .unwrap()
                .dedupe_key,
        );
        let mut missing = observation;
        missing.host_metadata.clear();
        assert!(frame_from_observation("runtime-a", "agent-a", "session-a", &missing).is_none());
    }
}
