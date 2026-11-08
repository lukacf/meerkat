//! Parser/admission controls. Hand-authored transcript values here test only
//! rejection/correlation logic; they are never a native acceptance receipt.
use super::*;
use meerkat_core::types::{AssistantBlock, BlockAssistantMessage};

fn probe_catalog() -> meerkat_core::ToolCatalogView {
    use meerkat_core::types::{ToolProvenance, ToolSourceKind};
    use meerkat_core::{
        ToolCallability, ToolCatalogDeferredEligibility, ToolCatalogMode, ToolCatalogView,
        ToolCatalogViewEntry, ToolCatalogVisibility, ToolPlaneClass,
    };
    ToolCatalogView {
        mode: ToolCatalogMode::Deferred,
        exact_catalog: true,
        pending_sources: vec![],
        staged_changes_pending: false,
        entries: ["selected", "other"]
            .into_iter()
            .map(|name| ToolCatalogViewEntry {
                name: name.into(),
                plane: ToolPlaneClass::Session,
                callability: ToolCallability::Callable,
                visibility: ToolCatalogVisibility::Deferred,
                deferred_eligibility: ToolCatalogDeferredEligibility::DeferredEligible {
                    provenance: ToolProvenance {
                        kind: ToolSourceKind::Mcp,
                        source_id: "current-selected-server".into(),
                        artifact: None,
                    },
                },
            })
            .collect(),
    }
}

#[test]
fn probe_turn_overlay_carries_only_the_selected_catalog_authority() {
    use meerkat_core::service::TURN_DEFERRED_TOOL_AUTHORITIES_CONTEXT_KEY;
    let catalog = probe_catalog();
    let arguments = json!({"resource":"exact"});
    let input = probe_prompt("selected", &arguments, &catalog).unwrap();
    let metadata = input.turn_metadata.as_ref().unwrap();
    let overlay = metadata.turn_tool_overlay.as_ref().unwrap();
    assert_eq!(
        overlay
            .allowed_tools
            .as_ref()
            .unwrap()
            .iter()
            .map(|name| name.as_str())
            .collect::<Vec<_>>(),
        vec!["selected"]
    );
    assert!(overlay.blocked_tools.is_none() && overlay.tool_choice_plan.is_empty());
    assert_eq!(overlay.dispatch_context.len(), 1);
    let authorities: Vec<meerkat_core::DeferredToolLoadAuthority> = serde_json::from_value(
        overlay.dispatch_context[TURN_DEFERRED_TOOL_AUTHORITIES_CONTEXT_KEY].clone(),
    )
    .unwrap();
    let meerkat_core::ToolCatalogDeferredEligibility::DeferredEligible { provenance } =
        &catalog.entries[0].deferred_eligibility
    else {
        panic!("fixture provenance");
    };
    assert_eq!(
        authorities,
        vec![meerkat_core::DeferredToolLoadAuthority::new(
            "selected",
            meerkat_core::ToolVisibilityWitness {
                last_seen_provenance: Some(provenance.clone()),
            }
        )]
    );
    let meerkat_core::ContentInput::Text(text) = input.content else {
        panic!("fresh prompt");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({"instruction":
        "Perform only this exact required-resource check once, then stop. Do not substitute a resource, call another tool, retry, or infer capability from prose.",
        "tool":"selected","arguments":arguments})
    );
}

#[test]
fn probe_turn_overlay_refuses_missing_ambiguous_or_unavailable_catalog_identity() {
    use meerkat_core::{
        ToolCallability, ToolCatalogDeferredEligibility, ToolCatalogVisibility, ToolPlaneClass,
    };
    for variant in 0..8 {
        let mut catalog = probe_catalog();
        match variant {
            0 => catalog.exact_catalog = false,
            1 => catalog.pending_sources.push("pending".into()),
            2 => {
                catalog.entries.remove(0);
            }
            3 => catalog.entries.push(catalog.entries[0].clone()),
            4 => {
                catalog.entries[0].deferred_eligibility = ToolCatalogDeferredEligibility::InlineOnly
            }
            5 => catalog.entries[0].callability = ToolCallability::from_bool(false),
            6 => catalog.entries[0].visibility = ToolCatalogVisibility::Hidden,
            _ => catalog.entries[0].plane = ToolPlaneClass::Control,
        }
        assert!(
            probe_prompt("selected", &json!({"resource":"exact"}), &catalog).is_err(),
            "variant {variant}"
        );
    }
}

#[tokio::test]
async fn probe_turn_overlay_native_owner_rejects_missing_or_changed_provenance() {
    use meerkat_core::service::TURN_DEFERRED_TOOL_AUTHORITIES_CONTEXT_KEY;
    use meerkat_core::{
        DeferredToolLoadAuthority, ToolCatalogDeferredEligibility, ToolScope, ToolScopeStageError,
    };
    // Component controls use the real generated visibility owner. No service
    // call or model result is manufactured; native-seven covers submission.
    for deferred in [true, false] {
        let machine = MeerkatMachine::ephemeral();
        let bindings = machine.prepare_bindings(SessionId::new()).await.unwrap();
        let mut catalog = probe_catalog();
        if !deferred {
            catalog.mode = meerkat_core::ToolCatalogMode::Inline;
            for entry in &mut catalog.entries {
                entry.visibility = meerkat_core::ToolCatalogVisibility::Visible;
            }
        }
        let tools = catalog
            .entries
            .iter()
            .map(|entry| {
                let ToolCatalogDeferredEligibility::DeferredEligible { provenance } =
                    &entry.deferred_eligibility
                else {
                    panic!("fixture provenance");
                };
                Arc::new(
                    meerkat_core::ToolDef::new(
                        entry.name.clone(),
                        "Probe control",
                        json!({"type":"object"}),
                    )
                    .with_provenance(provenance.clone()),
                )
            })
            .collect::<Vec<_>>();
        let scope = ToolScope::new_with_visibility_owner(
            tools.into(),
            Default::default(),
            if deferred {
                catalog
                    .entries
                    .iter()
                    .map(|entry| entry.name.clone())
                    .collect()
            } else {
                Default::default()
            },
            bindings.tool_visibility_owner().clone(),
        )
        .unwrap();
        let initial = scope.visible_tool_names().unwrap();
        let overlay = probe_prompt("selected", &json!({}), &catalog)
            .unwrap()
            .turn_metadata
            .unwrap()
            .turn_tool_overlay
            .unwrap();
        let authorities: Vec<DeferredToolLoadAuthority> = serde_json::from_value(
            overlay.dispatch_context[TURN_DEFERRED_TOOL_AUTHORITIES_CONTEXT_KEY].clone(),
        )
        .unwrap();
        for missing in [true, false] {
            let mut changed = authorities.clone();
            if missing {
                changed[0].witness.last_seen_provenance = None;
            } else {
                changed[0]
                    .witness
                    .last_seen_provenance
                    .as_mut()
                    .unwrap()
                    .source_id = "another-server".into();
            }
            let error = scope
                .handle()
                .set_turn_overlay_with_deferred_authorities(
                    Some(overlay.allowed_tools.clone().unwrap().into_iter().collect()),
                    Default::default(),
                    &changed,
                )
                .unwrap_err();
            assert!(matches!(
                error,
                ToolScopeStageError::InvalidWitnesses { .. }
            ));
            assert_eq!(
                scope.visible_tool_names().unwrap(),
                initial,
                "refused authority must not change visibility"
            );
        }
        scope
            .handle()
            .set_turn_overlay_with_deferred_authorities(
                Some(overlay.allowed_tools.unwrap().into_iter().collect()),
                Default::default(),
                &authorities,
            )
            .unwrap();
        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["selected".into()].into_iter().collect()
        );
        scope.handle().clear_turn_overlay().unwrap();
        assert_eq!(
            scope.visible_tool_names().unwrap(),
            initial,
            "turn load must not persist"
        );
    }
}

fn call(run: &meerkat_core::RunId, id: &str, name: &str, args: &str) -> Message {
    let mut assistant = BlockAssistantMessage::snapshot(vec![AssistantBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        args: serde_json::value::RawValue::from_string(args.into()).unwrap(),
        meta: None,
    }]);
    assistant.identity.run_id = Some(run.clone());
    Message::BlockAssistant(assistant)
}
fn result(id: &str) -> Message {
    Message::tool_results(vec![ToolResult::new(
        id.into(),
        "resource-result".into(),
        false,
    )])
}

#[test]
fn correlation_requires_exact_unique_call_run_arguments_and_later_result() {
    let run = meerkat_core::RunId::new();
    let other = meerkat_core::RunId::new();
    let args = json!({"repository":"one"});
    let exact = call(&run, "call", "read", &args.to_string());
    let answer = result("call");
    let selected = correlated_result(
        &[exact.clone(), answer.clone()],
        &run,
        "call",
        "read",
        &args,
    )
    .unwrap();
    assert_eq!(selected.text_content(), "resource-result");
    for messages in [
        vec![answer.clone(), exact.clone()],
        vec![exact.clone()],
        vec![exact.clone(), answer.clone(), answer.clone()],
        vec![exact.clone(), exact.clone(), answer.clone()],
        vec![
            call(&other, "call", "read", &args.to_string()),
            answer.clone(),
        ],
        vec![
            call(&run, "call", "write", &args.to_string()),
            answer.clone(),
        ],
        vec![
            call(&run, "call", "read", r#"{"repository":"other"}"#),
            answer.clone(),
        ],
        vec![
            call(
                &run,
                "call",
                "read",
                r#"{"repository":"other","repository":"one"}"#,
            ),
            answer,
        ],
    ] {
        assert!(correlated_result(&messages, &run, "call", "read", &args).is_none());
    }
}

#[test]
fn native_selected_connection_requires_unique_exact_descriptor() {
    let selected: meerkat_core::McpServerConfig = serde_json::from_value(json!({"name":"server",
        "command":"/bin/false","args":[]}))
    .unwrap();
    assert!(require_selected(std::slice::from_ref(&selected), &selected).is_ok());
    assert!(require_selected(&[], &selected).is_err());
    assert!(require_selected(&[selected.clone(), selected.clone()], &selected).is_err());
    let mut changed = selected.clone();
    let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut changed.transport else {
        panic!("stdio fixture");
    };
    stdio.command = "/bin/true".into();
    assert!(require_selected(&[changed], &selected).is_err());
}

#[test]
fn a_model_success_sentence_is_not_a_native_tool_result() {
    let run = meerkat_core::RunId::new();
    let text = Message::BlockAssistant(BlockAssistantMessage::snapshot(vec![
        AssistantBlock::Text {
            text: "The required repository was verified".into(),
            meta: None,
        },
    ]));
    assert!(correlated_result(&[text], &run, "call", "read", &json!({})).is_none());
}

#[test]
fn native_unknown_dispatch_is_not_relabelled_as_known_tool_failure_or_success() {
    let successful = ToolResult::new("call".into(), "resource found".into(), false);
    let failed = ToolResult::new("call".into(), "operation failed".into(), true);
    for result in [&successful, &failed] {
        assert!(matches!(
            dispatch_refusal(LiveBridgeEffectOutcome::Unknown, result),
            Some(ProbeStatus::OutcomeUnknown)
        ));
        assert!(matches!(
            dispatch_refusal(LiveBridgeEffectOutcome::Failed, result),
            Some(ProbeStatus::ToolFailed)
        ));
    }
    assert!(matches!(
        dispatch_refusal(LiveBridgeEffectOutcome::Committed, &failed),
        Some(ProbeStatus::ToolFailed)
    ));
    assert!(dispatch_refusal(LiveBridgeEffectOutcome::Committed, &successful).is_none());
}

#[test]
fn cancelled_probe_join_keeps_each_original_error_owner_until_returned_error_drops() {
    // These are actual owned error values, not native completion receipts.
    // The regression is the previously discarded terminal-join failure.
    #[derive(Debug)]
    struct Owner(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl std::fmt::Display for Owner {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("retained test owner")
        }
    }
    impl std::error::Error for Owner {}
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    for (cancel_fails, join_fails) in [(false, true), (true, false), (true, true), (false, false)] {
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let joined = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let error = |owner: std::sync::Arc<std::sync::atomic::AtomicBool>| {
            ConnectError::native("test-owned failure", Owner(owner))
        };
        let outcome = joined_probe_stop(
            if cancel_fails {
                Err(error(cancelled.clone()))
            } else {
                Ok(())
            },
            if join_fails {
                Err(error(joined.clone()))
            } else {
                Ok(())
            },
        );
        assert_eq!(outcome.is_err(), cancel_fails || join_fails);
        assert!(!cancelled.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!joined.load(std::sync::atomic::Ordering::SeqCst));
        let retains =
            |error: &ConnectError, expected: &std::sync::Arc<std::sync::atomic::AtomicBool>| {
                let ConnectError::Native { source, .. } = error else {
                    panic!("original native error was replaced");
                };
                let actual = source
                    .downcast_ref::<Owner>()
                    .expect("original owner remains typed");
                assert!(std::sync::Arc::ptr_eq(&actual.0, expected));
            };
        match (cancel_fails, join_fails, outcome.as_ref()) {
            (true, true, Err(ConnectError::Cleanup { primary, cleanup })) => {
                retains(primary, &cancelled);
                retains(cleanup, &joined);
            }
            (true, false, Err(error)) => retains(error, &cancelled),
            (false, true, Err(error)) => retains(error, &joined),
            (false, false, Ok(())) => (),
            _ => panic!("cancellation/join ownership shape changed"),
        }
        drop(outcome);
        assert_eq!(
            cancelled.load(std::sync::atomic::Ordering::SeqCst),
            cancel_fails
        );
        assert_eq!(joined.load(std::sync::atomic::Ordering::SeqCst), join_fails);
    }
}

#[test]
fn partial_progress_reads_gate_admission_and_outcome_without_promoting_cleanup() {
    // Gate/progress component control, not a native accepted-input receipt.
    let input = meerkat_core::InputId::new();
    let run = meerkat_core::RunId::new();
    let attempt = std::sync::Arc::new(Mutex::new(None));
    let mut progress = PartialCall {
        input: input.clone(),
        run: None,
        call: None,
        outcome: None,
    };
    progress.observe_dispatch(&attempt);
    assert_eq!(progress.input, input);
    assert!(progress.run.is_none() && progress.call.is_none() && progress.outcome.is_none());
    *attempt.lock().unwrap() = Some(Attempt {
        run: run.clone(),
        call: "entered-call".into(),
        outcome: None,
    });
    progress.observe_dispatch(&attempt);
    assert_eq!(progress.run, Some(run.clone()));
    assert_eq!(progress.call.as_deref(), Some("entered-call"));
    assert!(progress.outcome.is_none());
    attempt.lock().unwrap().as_mut().unwrap().outcome = Some(LiveBridgeEffectOutcome::Unknown);
    progress.observe_dispatch(&attempt);
    assert_eq!(progress.outcome, Some(LiveBridgeEffectOutcome::Unknown));
    let poison = attempt.clone();
    assert!(
        std::thread::spawn(move || {
            let _held = poison.lock().unwrap();
            panic!("fixture poison");
        })
        .join()
        .is_err()
    );
    progress.observe_dispatch(&attempt);
    assert_eq!(progress.input, input);
    assert_eq!(progress.run, Some(run));
    assert_eq!(progress.call.as_deref(), Some("entered-call"));
    assert_eq!(progress.outcome, Some(LiveBridgeEffectOutcome::Unknown));
    let mut unread = PartialCall {
        input: meerkat_core::InputId::new(),
        run: None,
        call: None,
        outcome: None,
    };
    unread.observe_dispatch(&attempt);
    assert!(unread.run.is_none() && unread.call.is_none() && unread.outcome.is_none());
}
