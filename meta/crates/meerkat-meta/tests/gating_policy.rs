#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use meerkat_meta::{
    DiscoverySpec, MetaConfig, ModuleConfig, PreSpawnData, RestartPolicy, handle_meta_rpc_json,
    start_meta_runtime,
};
use serde_json::{Value, json};

const BOUNDARY_ENV_KEY: &str = "META_MODULE_BOUNDARY";
const BOUNDARY_ENV_VALUE_MCP: &str = "mcp";

fn parse_response(line: &str) -> Value {
    serde_json::from_str(line).expect("valid rpc response json")
}

/// A gating owner whose escalations go to `operator-escalation`.
fn runtime_for_gating() -> meerkat_meta::MetaRuntimeHandle {
    let mut runtime = runtime_without_escalation_routes();
    runtime.set_gating_escalation_routes(meerkat_meta::GatingEscalationRoutes {
        default: Some("operator-escalation".to_string()),
        ..meerkat_meta::GatingEscalationRoutes::default()
    });
    runtime
}

fn runtime_without_escalation_routes() -> meerkat_meta::MetaRuntimeHandle {
    start_meta_runtime(
        MetaConfig {
            modules: vec![],
            discovery: DiscoverySpec {
                namespace: "phase12".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        vec![],
        Duration::from_secs(1),
    )
    .expect("runtime starts")
}

fn fixture_binary_path() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_mcp_fixture") {
        return PathBuf::from(path);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let binary_path = workspace_root
        .join("target")
        .join("debug")
        .join("mcp_fixture");
    if binary_path.exists() {
        return binary_path;
    }

    let status = Command::new("cargo")
        .args(["build", "-p", "meerkat-meta", "--bin", "mcp_fixture"])
        .current_dir(workspace_root)
        .status()
        .expect("build mcp_fixture");
    assert!(status.success(), "building mcp_fixture must succeed");
    binary_path
}

fn fixture_module(id: &str, fixture_binary: &std::path::Path) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: fixture_binary.display().to_string(),
        args: vec!["--module".to_string(), id.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

fn mcp_env(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut env = vec![(
        BOUNDARY_ENV_KEY.to_string(),
        BOUNDARY_ENV_VALUE_MCP.to_string(),
    )];
    env.extend(
        extra
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string())),
    );
    env
}

fn runtime_for_gating_with_routing_delivery() -> meerkat_meta::MetaRuntimeHandle {
    let fixture_binary = fixture_binary_path();
    start_meta_runtime(
        MetaConfig {
            modules: vec![
                fixture_module("router", &fixture_binary),
                fixture_module("delivery", &fixture_binary),
            ],
            discovery: DiscoverySpec {
                namespace: "phase12-routing".to_string(),
                modules: vec!["router".to_string(), "delivery".to_string()],
            },
            pre_spawn: vec![
                PreSpawnData {
                    module_id: "router".to_string(),
                    env: mcp_env(&[]),
                },
                PreSpawnData {
                    module_id: "delivery".to_string(),
                    env: mcp_env(&[]),
                },
            ],
        },
        vec![],
        Duration::from_secs(2),
    )
    .expect("runtime starts")
}

fn runtime_for_gating_with_forced_failed_delivery() -> meerkat_meta::MetaRuntimeHandle {
    let fixture_binary = fixture_binary_path();
    start_meta_runtime(
        MetaConfig {
            modules: vec![
                fixture_module("router", &fixture_binary),
                fixture_module("delivery", &fixture_binary),
            ],
            discovery: DiscoverySpec {
                namespace: "phase12-routing-fail".to_string(),
                modules: vec!["router".to_string(), "delivery".to_string()],
            },
            pre_spawn: vec![
                PreSpawnData {
                    module_id: "router".to_string(),
                    env: mcp_env(&[]),
                },
                PreSpawnData {
                    module_id: "delivery".to_string(),
                    env: mcp_env(&[("META_PHASE_C_DELIVERY_FORCE_FAIL", "1")]),
                },
            ],
        },
        vec![],
        Duration::from_secs(2),
    )
    .expect("runtime starts")
}

#[test]
fn gating_resolution_observers_notified_on_decide_escalate_and_timeout() {
    use meerkat_meta::runtime::{
        GatingDecideRequest, GatingDecision, GatingEvaluateRequest, GatingResolutionNotice,
        GatingResolutionObserver, GatingRiskTier,
    };
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Collector {
        notices: Mutex<Vec<GatingResolutionNotice>>,
    }

    impl GatingResolutionObserver for Collector {
        fn on_gating_resolution(&self, notice: &GatingResolutionNotice) {
            self.notices.lock().unwrap().push(notice.clone());
        }
    }

    let mut runtime = runtime_for_gating();
    let collector = Arc::new(Collector::default());
    runtime.register_gating_resolution_observer(collector.clone());

    let evaluate =
        |runtime: &mut meerkat_meta::MetaRuntimeHandle, action: &str, timeout: Option<u64>| {
            let result = runtime.evaluate_gating_action(GatingEvaluateRequest {
                action: action.to_string(),
                actor_id: "memory-steward:family".to_string(),
                risk_tier: GatingRiskTier::R3,
                rationale: None,
                requested_approver: None,
                approval_recipient: None,
                approval_channel: None,
                approval_timeout_ms: timeout,
                entity: None,
                topic: None,
            });
            result.pending_id.expect("R3 mints a pending entry")
        };

    // Approve → approved=true notice.
    let approved_id = evaluate(&mut runtime, "memory.quarantine_promote:one", None);
    runtime
        .decide_gating_action(GatingDecideRequest {
            pending_id: approved_id.clone(),
            approver_id: "operator-1".to_string(),
            decision: GatingDecision::Approve,
            reason: None,
        })
        .expect("approve");

    // Reject → approved=false notice.
    let rejected_id = evaluate(&mut runtime, "memory.quarantine_promote:two", None);
    runtime
        .decide_gating_action(GatingDecideRequest {
            pending_id: rejected_id.clone(),
            approver_id: "operator-1".to_string(),
            decision: GatingDecision::Reject,
            reason: Some("not shareable".to_string()),
        })
        .expect("reject");

    // Escalate → approved=false notice carrying the successor pending id.
    let escalated_id = evaluate(&mut runtime, "memory.quarantine_promote:three", None);
    let escalated = runtime
        .decide_gating_action(GatingDecideRequest {
            pending_id: escalated_id.clone(),
            approver_id: "operator-1".to_string(),
            decision: GatingDecision::Escalate,
            reason: None,
        })
        .expect("escalate");
    let successor = escalated.next_pending_id.clone().expect("successor id");

    // Timeout → approved=false notice with cause timeout_fallback (the
    // sweep runs on the next gating call; 1s is the clamp floor).
    let timed_out_id = evaluate(&mut runtime, "memory.quarantine_promote:four", Some(1_000));
    std::thread::sleep(Duration::from_millis(1_100));
    let _ = runtime.list_gating_pending();

    let notices = collector.notices.lock().unwrap();
    let find = |pending: &str| -> GatingResolutionNotice {
        notices
            .iter()
            .find(|notice| notice.pending_id == pending)
            .unwrap_or_else(|| panic!("no notice for {pending}: {notices:?}"))
            .clone()
    };
    let approve_notice = find(&approved_id);
    assert!(approve_notice.approved);
    assert_eq!(approve_notice.cause, "approval_decided");
    let reject_notice = find(&rejected_id);
    assert!(!reject_notice.approved);
    assert_eq!(reject_notice.cause, "rejection_decided");
    let escalate_notice = find(&escalated_id);
    assert!(!escalate_notice.approved);
    assert_eq!(escalate_notice.cause, "escalation_decided");
    assert_eq!(
        escalate_notice.next_pending_id.as_deref(),
        Some(successor.as_str())
    );
    let timeout_notice = find(&timed_out_id);
    assert!(!timeout_notice.approved);
    assert_eq!(timeout_notice.cause, "timeout_fallback");
    assert_eq!(timeout_notice.next_pending_id, None);
}

fn decider(principal: &str, held: &[&str]) -> meerkat_meta::GatingDecider {
    meerkat_meta::GatingDecider {
        principal: principal.to_string(),
        held_approver_ids: held.iter().map(ToString::to_string).collect(),
    }
}

fn r3_for(
    runtime: &mut meerkat_meta::MetaRuntimeHandle,
    actor: &str,
    requested_approver: Option<&str>,
) -> String {
    runtime
        .evaluate_gating_action(
            serde_json::from_value::<meerkat_meta::GatingEvaluateRequest>(json!({
                "action": "Publish release", "actor_id": actor, "risk_tier": "r3",
                "requested_approver": requested_approver,
            }))
            .expect("request"),
        )
        .pending_id
        .expect("r3 pends")
}

/// The request names `person:alice` as its approver id; for an authenticated
/// decider that field is never identity evidence.
fn forged_decision(
    pending_id: &str,
    decision: meerkat_meta::GatingDecision,
) -> meerkat_meta::GatingDecideRequest {
    meerkat_meta::GatingDecideRequest {
        pending_id: pending_id.to_string(),
        approver_id: "person:alice".to_string(),
        decision,
        reason: None,
    }
}

#[test]
fn principal_decides_as_a_held_approver_and_the_audit_names_the_individual() {
    use meerkat_meta::{GatingDecideError, GatingDecision};
    let mut runtime = runtime_for_gating();
    let pending = r3_for(&mut runtime, "agent:writer", Some("person:alice"));
    assert_eq!(
        runtime.decide_gating_action_for(
            forged_decision(&pending, GatingDecision::Approve),
            &decider("bob@example.test", &[])
        ),
        Err(GatingDecideError::ApproverMismatch {
            expected: "person:alice".to_string(),
            provided: "bob@example.test".to_string(),
        })
    );
    assert_eq!(runtime.list_gating_pending().len(), 1);

    let decided = runtime
        .decide_gating_action_for(
            forged_decision(&pending, GatingDecision::Approve),
            &decider("alice@example.test", &["person:alice"]),
        )
        .expect("held approver decides");
    assert_eq!(decided.approver_id, "person:alice");
    let audit = runtime.gating_audit_entries(1).remove(0);
    assert_eq!(audit.detail["approver_id"], json!("person:alice"));
    assert_eq!(
        audit.detail["deciding_principal"],
        json!("alice@example.test")
    );
    assert_eq!(audit.detail["approver_binding"], json!("held"));
}

#[test]
fn shared_role_first_eligible_decision_wins_and_records_the_individual() {
    use meerkat_meta::{GatingDecideError, GatingDecision};
    let mut runtime = runtime_for_gating();
    let pending = r3_for(&mut runtime, "agent:writer", Some("role:release-manager"));
    let carol = decider("carol@example.test", &["role:release-manager"]);
    let dave = decider("dave@example.test", &["role:release-manager"]);
    let decided = runtime
        .decide_gating_action_for(forged_decision(&pending, GatingDecision::Approve), &carol)
        .expect("carol decides");
    assert_eq!(decided.approver_id, "role:release-manager");
    let audit = runtime.gating_audit_entries(1).remove(0);
    assert_eq!(
        audit.detail["deciding_principal"],
        json!("carol@example.test")
    );
    assert_eq!(
        runtime.decide_gating_action_for(forged_decision(&pending, GatingDecision::Reject), &dave),
        Err(GatingDecideError::UnknownPendingId(pending))
    );
}

#[test]
fn self_approval_is_refused_through_the_principal_and_every_held_alias() {
    use meerkat_meta::{GatingDecideError, GatingDecision};
    let mut runtime = runtime_for_gating();
    for (actor, decider_value) in [
        (
            "person:alice",
            decider("alice@example.test", &["person:alice"]),
        ),
        ("alice@example.test", decider("alice@example.test", &[])),
    ] {
        let pending = r3_for(&mut runtime, actor, None);
        assert_eq!(
            runtime.decide_gating_action_for(
                forged_decision(&pending, GatingDecision::Approve),
                &decider_value
            ),
            Err(GatingDecideError::SelfApprovalForbidden),
            "{actor}"
        );
        assert!(
            runtime
                .list_gating_pending()
                .iter()
                .any(|entry| entry.pending_id == pending)
        );
        runtime
            .decide_gating_action_for(
                forged_decision(&pending, GatingDecision::Reject),
                &decider_value,
            )
            .expect("the legacy gate still lets the actor reject");
    }
}

/// Escalation goes to a configured successor that keeps the exact candidate
/// and expiry; without a route it is refused and the entry stays pending.
#[test]
fn escalation_routes_to_a_configured_successor_or_is_refused() {
    use meerkat_meta::{GatingDecideError, GatingDecision, GatingEscalationRoutes};
    let escalate = |pending_id: &str| meerkat_meta::GatingDecideRequest {
        pending_id: pending_id.to_string(),
        approver_id: "person:alice".to_string(),
        decision: GatingDecision::Escalate,
        reason: None,
    };

    let mut unrouted = runtime_without_escalation_routes();
    let pending = r3_for(&mut unrouted, "agent:writer", Some("person:alice"));
    let before = unrouted.gating_audit_entries(512);
    assert_eq!(
        unrouted.decide_gating_action(escalate(&pending)),
        Err(GatingDecideError::NoEscalationRoute {
            requested_approver: Some("person:alice".to_string()),
        })
    );
    assert_eq!(unrouted.list_gating_pending().len(), 1);
    assert_eq!(unrouted.gating_audit_entries(512), before);
    let rpc = parse_response(&handle_meta_rpc_json(
        &mut unrouted,
        &json!({
            "jsonrpc": "2.0", "id": "escalate", "method": "meta/gating/decide",
            "params": {"pending_id": pending, "approver_id": "person:alice", "decision": "escalate"},
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    assert_eq!(rpc["error"]["code"], json!(-32602));
    assert_eq!(
        rpc["error"]["message"],
        json!("Invalid params: no escalation route is configured from 'person:alice'")
    );

    let mut routed = runtime_without_escalation_routes();
    routed.set_gating_escalation_routes(GatingEscalationRoutes {
        default: Some("role:on-call".to_string()),
        routes: [(
            "person:alice".to_string(),
            "role:release-manager".to_string(),
        )]
        .into(),
    });
    let specific = r3_for(&mut routed, "agent:writer", Some("person:alice"));
    let original = routed.list_gating_pending().remove(0);
    let decided = routed
        .decide_gating_action(escalate(&specific))
        .expect("routed");
    let successor = routed.list_gating_pending().remove(0);
    assert_eq!(
        Some(&successor.pending_id),
        decided.next_pending_id.as_ref()
    );
    assert_eq!(
        successor.requested_approver.as_deref(),
        Some("role:release-manager")
    );
    assert_eq!(successor.action_id, original.action_id);
    assert_eq!(successor.action, original.action);
    assert_eq!(successor.deadline_at_ms, original.deadline_at_ms);

    let unnamed = r3_for(&mut routed, "agent:writer", None);
    let decided = routed
        .decide_gating_action(meerkat_meta::GatingDecideRequest {
            approver_id: "operator".to_string(),
            ..escalate(&unnamed)
        })
        .expect("default route");
    let next = decided.next_pending_id.expect("successor");
    assert_eq!(
        routed
            .list_gating_pending()
            .into_iter()
            .find(|entry| entry.pending_id == next)
            .and_then(|entry| entry.requested_approver),
        Some("role:on-call".to_string())
    );
}

/// The audit cursor resumes after a retained entry and reports a gap for a
/// cursor the owner does not hold: evicted, unknown, or from an earlier
/// owner incarnation.
/// An audit cursor read that expires an entry delivers its timeout notice.
#[test]
fn gating_audit_cursor_read_delivers_timeout_notices() {
    use meerkat_meta::runtime::{GatingResolutionNotice, GatingResolutionObserver};
    #[derive(Default)]
    struct Causes(std::sync::Mutex<Vec<String>>);
    impl GatingResolutionObserver for Causes {
        fn on_gating_resolution(&self, notice: &GatingResolutionNotice) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(notice.cause.clone());
        }
    }
    let mut runtime = runtime_for_gating();
    let causes = std::sync::Arc::new(Causes::default());
    runtime.register_gating_resolution_observer(causes.clone());
    runtime.evaluate_gating_action(
        serde_json::from_value::<meerkat_meta::GatingEvaluateRequest>(json!({
            "action": "Deploy", "actor_id": "worker", "risk_tier": "r3",
            "approval_timeout_ms": 1,
        }))
        .expect("request"),
    );
    std::thread::sleep(Duration::from_millis(1_100));
    let page = runtime.gating_audit_after(None);
    assert!(
        page.entries
            .iter()
            .any(|entry| entry.event_type == "timeout_fallback")
    );
    assert_eq!(
        *causes
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec!["timeout_fallback".to_string()]
    );
}

#[test]
fn gating_audit_cursor_resumes_or_reports_a_gap() {
    let mut runtime = runtime_for_gating();
    r3_for(&mut runtime, "agent:writer", None);
    let first = runtime.gating_audit_after(None);
    assert!(!first.gap);
    let cursor = first.entries.last().expect("entry").audit_id.clone();
    let caught_up = runtime.gating_audit_after(Some(&cursor));
    assert!(!caught_up.gap);
    assert!(caught_up.entries.is_empty());
    r3_for(&mut runtime, "agent:writer", None);
    let next = runtime.gating_audit_after(Some(&cursor));
    assert!(!next.gap);
    assert_eq!(next.entries.len(), 1);
    assert_ne!(next.entries[0].audit_id, cursor);

    let unknown = runtime.gating_audit_after(Some("gate-audit-000001"));
    assert!(unknown.gap);
    assert_eq!(
        unknown.entries.len(),
        2,
        "a gap restarts from the oldest retained entry"
    );
    let mut restarted = runtime_for_gating();
    assert!(restarted.gating_audit_after(Some(&cursor)).gap);

    let rpc = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0", "id": "audit", "method": "meta/gating/audit",
            "params": {"after_audit_id": cursor, "limit": 10},
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    assert_eq!(rpc["result"]["gap"], json!(false));
    assert_eq!(rpc["result"]["entries"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        rpc["result"]["next_cursor"],
        rpc["result"]["entries"][0]["audit_id"]
    );
    let lost = parse_response(&handle_meta_rpc_json(
        &mut restarted,
        &json!({
            "jsonrpc": "2.0", "id": "audit", "method": "meta/gating/audit",
            "params": {"after_audit_id": cursor},
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    assert_eq!(lost["result"]["gap"], json!(true));
    assert_eq!(
        lost["result"]["next_cursor"],
        Value::Null,
        "a gap with nothing returned does not echo the stale cursor"
    );
}

/// Eviction from the bounded audit is a gap; a restored audit keeps its
/// entries' ids, so their cursors keep working in the restoring runtime.
#[test]
fn gating_audit_cursor_gap_on_eviction_and_continuity_after_restore() {
    use meerkat_meta::runtime::GatingEvaluateRequest;
    let r0 = || {
        serde_json::from_value::<GatingEvaluateRequest>(json!({
            "action": "Read", "actor_id": "worker", "risk_tier": "r0",
        }))
        .expect("request")
    };
    let mut runtime = runtime_for_gating();
    runtime.evaluate_gating_action(r0());
    let first = runtime.gating_audit_after(None).entries[0].audit_id.clone();
    for _ in 0..512 {
        runtime.evaluate_gating_action(r0());
    }
    let evicted = runtime.gating_audit_after(Some(&first));
    assert!(evicted.gap, "an evicted cursor is a gap");
    assert_eq!(evicted.entries.len(), 512);

    let retained = evicted.entries[100].audit_id.clone();
    let mut restored = runtime_for_gating();
    restored
        .restore_gating_state(runtime.gating_state_snapshot())
        .expect("restore");
    let page = restored.gating_audit_after(Some(&retained));
    assert!(!page.gap, "a restored entry's cursor still resolves");
    assert_eq!(page.entries.len(), 411);
}

/// A refused decision leaves the pending entries, their order, the ids and
/// the audit exactly as they were.
#[test]
fn refused_decisions_leave_the_owner_state_unchanged() {
    use meerkat_meta::{GatingDecideError, GatingDecision};
    let mut runtime = runtime_without_escalation_routes();
    let first = r3_for(&mut runtime, "agent:writer", Some("person:alice"));
    r3_for(&mut runtime, "agent:writer", None);
    r3_for(&mut runtime, "agent:writer", None);
    let before = runtime.gating_state_snapshot();
    for (decision, decider_value, expected) in [
        (
            GatingDecision::Approve,
            decider("bob@example.test", &[]),
            GatingDecideError::ApproverMismatch {
                expected: "person:alice".to_string(),
                provided: "bob@example.test".to_string(),
            },
        ),
        (
            GatingDecision::Approve,
            decider("agent:writer", &["person:alice"]),
            GatingDecideError::SelfApprovalForbidden,
        ),
        (
            GatingDecision::Escalate,
            decider("alice@example.test", &["person:alice"]),
            GatingDecideError::NoEscalationRoute {
                requested_approver: Some("person:alice".to_string()),
            },
        ),
    ] {
        assert_eq!(
            runtime.decide_gating_action_for(forged_decision(&first, decision), &decider_value),
            Err(expected)
        );
        assert_eq!(runtime.gating_state_snapshot(), before);
    }
}

/// Without router and delivery modules the notification is refused before
/// entry; the pending entry is still committed and the refusal is reported
/// on the same `approval_notification` stream.
#[test]
fn r3_notification_refused_before_entry_is_reported_as_not_sent() {
    let mut runtime = runtime_for_gating();
    runtime.evaluate_gating_action(
        serde_json::from_value::<meerkat_meta::GatingEvaluateRequest>(json!({
            "action": "publish_release", "actor_id": "alice", "risk_tier": "r3",
            "approval_recipient": "approvals@example.com",
            "approval_channel": "transactional",
        }))
        .expect("request"),
    );
    assert_eq!(runtime.list_gating_pending().len(), 1);
    let audit = runtime.gating_audit_entries(10);
    let created = audit
        .iter()
        .position(|entry| entry.event_type == "pending_created")
        .expect("pending_created");
    let notified = audit
        .iter()
        .position(|entry| entry.event_type == "approval_notification")
        .expect("approval_notification");
    assert!(created < notified);
    assert_eq!(audit[notified].detail["disposition"], json!("not_sent"));
    assert!(
        audit[notified].detail["approval_notification_error"]
            .as_str()
            .is_some_and(|error| error.starts_with("notification_modules_unavailable:"))
    );
}

#[test]
#[ignore]
fn r3_approval_notification_follows_the_committed_pending_entry() {
    let mut runtime = runtime_for_gating_with_routing_delivery();
    let evaluated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0", "id": "notify", "method": "meta/gating/evaluate",
            "params": {
                "action": "publish_release", "actor_id": "alice", "risk_tier": "r3",
                "requested_approver": "bob",
                "approval_recipient": "approvals@example.com",
                "approval_channel": "transactional",
            },
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending_id = evaluated["result"]["pending_id"].clone();
    let pending = runtime.list_gating_pending().remove(0);
    let audit = runtime.gating_audit_entries(10);
    runtime.shutdown();

    let created = audit
        .iter()
        .position(|entry| entry.event_type == "pending_created")
        .expect("pending_created");
    let notified = audit
        .iter()
        .position(|entry| entry.event_type == "approval_notification")
        .expect("approval_notification");
    assert!(
        created < notified,
        "the pending entry and its audit are committed first"
    );
    assert_eq!(audit[created].detail["approval_delivery_id"], Value::Null);
    assert_eq!(
        audit[notified].pending_id,
        pending_id.as_str().map(ToString::to_string)
    );
    assert_eq!(audit[notified].detail["disposition"], json!("acknowledged"));
    assert_eq!(
        audit[notified].detail["approval_delivery_id"],
        json!(pending.approval_delivery_id)
    );
    assert!(pending.approval_delivery_id.is_some());
}

#[test]
#[ignore]
fn r3_failed_notification_keeps_the_committed_pending_entry() {
    let mut runtime = runtime_for_gating_with_forced_failed_delivery();
    runtime.evaluate_gating_action(
        serde_json::from_value::<meerkat_meta::GatingEvaluateRequest>(json!({
            "action": "publish_release", "actor_id": "alice", "risk_tier": "r3",
            "approval_recipient": "approvals@example.com",
            "approval_channel": "transactional",
        }))
        .expect("request"),
    );
    let pending = runtime.list_gating_pending();
    let audit = runtime.gating_audit_entries(10);
    runtime.shutdown();

    assert_eq!(
        pending.len(),
        1,
        "a failed notification leaves one pending action"
    );
    assert_eq!(pending[0].approval_delivery_id, None);
    let notified = audit
        .iter()
        .find(|entry| entry.event_type == "approval_notification")
        .expect("approval_notification");
    assert_eq!(notified.detail["disposition"], json!("not_delivered"));
    assert!(notified.detail["approval_delivery_id"].is_string());
}

#[test]
fn phase0_contract_008_gating_escalate_returns_successor_pending_entry() {
    let mut runtime = runtime_for_gating();
    let evaluated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase0-gating-evaluate",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"publish_release",
                "actor_id":"alice",
                "risk_tier":"r3",
                "requested_approver":"bob",
                "approval_timeout_ms":60_000
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let escalated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase0-gating-escalate",
            "method":"meta/gating/decide",
            "params":{
                "pending_id": evaluated["result"]["pending_id"].clone(),
                "approver_id":"bob",
                "decision":"escalate",
                "reason":"needs higher approval"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-gating-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    runtime.shutdown();

    let next_pending_id = escalated["result"]["next_pending_id"]
        .as_str()
        .expect("escalate should return next_pending_id");
    assert_eq!(escalated["result"]["decision"], json!("escalate"));
    assert_eq!(escalated["result"]["outcome"], json!("pending_approval"));
    assert_ne!(
        next_pending_id,
        evaluated["result"]["pending_id"]
            .as_str()
            .expect("evaluate pending id")
    );
    assert_eq!(
        pending["result"]["pending"][0]["pending_id"],
        json!(next_pending_id)
    );
    assert_eq!(
        pending["result"]["pending"][0]["requested_approver"],
        json!("operator-escalation")
    );
}

#[test]
#[ignore]
fn phase12_r3_approval_flow_enforces_approver_constraints_and_audits() {
    let mut runtime = runtime_for_gating_with_routing_delivery();
    let evaluated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase12-r3-evaluate",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"publish_release",
                "actor_id":"alice",
                "risk_tier":"r3",
                "requested_approver":"bob",
                "approval_recipient":"approvals@example.com",
                "approval_channel":"transactional",
                "approval_timeout_ms":60_000
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending_before_decision = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-r3-pending-before","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    let self_approve = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase12-r3-self-approve",
            "method":"meta/gating/decide",
            "params":{
                "pending_id": evaluated["result"]["pending_id"].clone(),
                "approver_id":"alice",
                "decision":"approve"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let wrong_approver = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase12-r3-wrong-approver",
            "method":"meta/gating/decide",
            "params":{
                "pending_id": evaluated["result"]["pending_id"].clone(),
                "approver_id":"carol",
                "decision":"approve"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending_after_invalid = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-r3-pending-after-invalid","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    let approved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase12-r3-approve",
            "method":"meta/gating/decide",
            "params":{
                "pending_id": evaluated["result"]["pending_id"].clone(),
                "approver_id":"bob",
                "decision":"approve"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-r3-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    let audit = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-r3-audit","method":"meta/gating/audit","params":{"limit":20}}"#,
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-r3-history","method":"meta/delivery/history","params":{"recipient":"approvals@example.com","limit":5}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    let entries = audit["result"]["entries"]
        .as_array()
        .expect("audit entries");
    let events = entries
        .iter()
        .filter_map(|entry| entry.get("event_type").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let pending_created = entries
        .iter()
        .find(|entry| entry.get("event_type") == Some(&json!("pending_created")))
        .expect("pending_created entry");
    let approval_decided = entries
        .iter()
        .find(|entry| entry.get("event_type") == Some(&json!("approval_decided")))
        .expect("approval_decided entry");
    let approval_notification = entries
        .iter()
        .find(|entry| entry.get("event_type") == Some(&json!("approval_notification")))
        .expect("approval_notification entry");
    assert_eq!(evaluated["result"]["outcome"], json!("pending_approval"));
    assert_eq!(
        self_approve["error"]["message"],
        json!("Invalid params: approver_id cannot self-approve the action actor")
    );
    assert_eq!(
        wrong_approver["error"]["message"],
        json!("Invalid params: approver_id 'carol' does not match requested_approver 'bob'")
    );
    assert_eq!(approved["result"]["outcome"], json!("allowed"));
    assert_eq!(pending["result"]["pending"], json!([]));
    assert!(events.contains(&"pending_created"));
    assert!(events.contains(&"approval_decided"));
    assert_eq!(
        pending_after_invalid["result"]["pending"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        pending_after_invalid["result"]["pending"][0]["pending_id"],
        pending_before_decision["result"]["pending"][0]["pending_id"]
    );
    assert_eq!(
        pending_created["detail"]["approval_route_id"],
        pending_before_decision["result"]["pending"][0]["approval_route_id"]
    );
    // The pending entry is committed before the notification is sent, so
    // its creation audit has no delivery id; the notification audit does.
    assert_eq!(
        pending_created["detail"]["approval_delivery_id"],
        Value::Null
    );
    assert_eq!(
        approval_notification["detail"]["approval_delivery_id"],
        pending_before_decision["result"]["pending"][0]["approval_delivery_id"]
    );
    assert_eq!(
        approval_notification["detail"]["disposition"],
        json!("acknowledged")
    );
    assert_eq!(
        approval_decided["detail"]["approval_route_id"],
        pending_before_decision["result"]["pending"][0]["approval_route_id"]
    );
    assert_eq!(
        approval_decided["detail"]["approval_delivery_id"],
        pending_before_decision["result"]["pending"][0]["approval_delivery_id"]
    );
    assert_eq!(
        history["result"]["deliveries"][0]["route_id"],
        pending_before_decision["result"]["pending"][0]["approval_route_id"]
    );
    assert_eq!(
        history["result"]["deliveries"][0]["delivery_id"],
        pending_before_decision["result"]["pending"][0]["approval_delivery_id"]
    );
}

#[test]
#[ignore]
fn phase12_risk_tiers_and_timeout_fallback_are_wired_with_audit() {
    let mut runtime = runtime_for_gating();
    let r0 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase12-r0",
            "method":"meta/gating/evaluate",
            "params":{"action":"read_status","actor_id":"agent-r0","risk_tier":"r0"}
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let r1 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase12-r1",
            "method":"meta/gating/evaluate",
            "params":{"action":"edit_doc","actor_id":"agent-r1","risk_tier":"r1"}
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let r2 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase12-r2",
            "method":"meta/gating/evaluate",
            "params":{"action":"notify_users","actor_id":"agent-r2","risk_tier":"r2"}
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    // `approval_timeout_ms` is clamped to a 1s floor (a value of 0 would
    // otherwise mint an already-expired pending entry; see
    // GATING_APPROVAL_TIMEOUT_MIN_MS). Request the minimum, then sleep just past
    // it so the timeout-fallback path genuinely fires on the next gating RPC.
    let _r3_timeout = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase12-r3",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"delete_prod_data",
                "actor_id":"agent-r3",
                "risk_tier":"r3",
                "approval_timeout_ms":1
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    std::thread::sleep(Duration::from_millis(1_100));
    let pending = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    let audit = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-audit","method":"meta/gating/audit","params":{"limit":30}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    let entries = audit["result"]["entries"]
        .as_array()
        .expect("audit entries");
    let has_timeout_fallback = entries.iter().any(|entry| {
        entry.get("event_type") == Some(&json!("timeout_fallback"))
            && entry.get("outcome") == Some(&json!("safe_draft"))
    });
    let has_r2_consequence = entries.iter().any(|entry| {
        entry.get("outcome") == Some(&json!("allowed_with_audit"))
            && entry.get("detail").and_then(|detail| detail.get("policy"))
                == Some(&json!("consequence_mode_allow_with_audit_v0_1"))
    });

    assert_eq!(
        (
            r0["result"]["outcome"].clone(),
            r1["result"]["outcome"].clone(),
            r2["result"]["outcome"].clone(),
            pending["result"]["pending"].clone(),
            has_timeout_fallback,
            has_r2_consequence,
        ),
        (
            json!("allowed"),
            json!("allowed"),
            json!("allowed_with_audit"),
            json!([]),
            true,
            true,
        )
    );
}

/// Regression: a caller-supplied R3 approval timeout must be clamped so the
/// deadline can't saturate past `u64::MAX` and become a never-expiring pending
/// approval. With a huge `approval_timeout_ms` the pending entry's
/// `deadline_at_ms` must stay finite (≈ now + 7d), keeping the
/// timeout-to-safe-draft fallback reachable.
#[test]
fn r3_approval_timeout_is_clamped_so_deadline_never_saturates() {
    let mut runtime = runtime_for_gating();
    let _evaluated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"clamp-r3",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"delete_prod_data",
                "actor_id":"agent-clamp",
                "risk_tier":"r3",
                "approval_timeout_ms": u64::MAX
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"clamp-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    runtime.shutdown();

    let entry = pending["result"]["pending"]
        .as_array()
        .and_then(|entries| entries.first())
        .expect("one pending approval");
    let created = entry["created_at_ms"].as_u64().expect("created_at_ms");
    let deadline = entry["deadline_at_ms"].as_u64().expect("deadline_at_ms");
    assert!(
        deadline < u64::MAX,
        "deadline must not saturate: {deadline}"
    );
    // Clamped to the 7-day ceiling, not the u64::MAX request.
    let seven_days_ms = 7 * 24 * 60 * 60 * 1_000;
    assert_eq!(deadline, created.saturating_add(seven_days_ms));
}

/// Regression: a caller-supplied R3 approval timeout of `0` (or any tiny
/// value) must be clamped UP to a 1s floor. Without the clamp the pending
/// entry's `deadline_at_ms` equals `created_at_ms`, so `refresh_gating_timeouts`
/// treats it as already-expired on the very next gating RPC and replaces it
/// with a `timeout_fallback` SafeDraft before any approver can act — making R3
/// approval impossible to complete.
#[test]
fn r3_approval_timeout_zero_is_clamped_up_so_pending_stays_approvable() {
    let mut runtime = runtime_for_gating();
    let _evaluated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"floor-r3",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"delete_prod_data",
                "actor_id":"agent-floor",
                "risk_tier":"r3",
                "approval_timeout_ms": 0
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    // The very next gating RPC runs refresh_gating_timeouts. The pending entry
    // must survive it (not be immediately expired) because the deadline was
    // floored to created + 1s.
    let pending = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"floor-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    runtime.shutdown();

    let entries = pending["result"]["pending"]
        .as_array()
        .expect("pending entries");
    assert_eq!(
        entries.len(),
        1,
        "pending approval must not be auto-expired"
    );
    let entry = &entries[0];
    let created = entry["created_at_ms"].as_u64().expect("created_at_ms");
    let deadline = entry["deadline_at_ms"].as_u64().expect("deadline_at_ms");
    assert!(
        deadline >= created.saturating_add(1_000),
        "deadline must be floored to >= created + 1s: created={created} deadline={deadline}"
    );
}

#[test]
#[ignore]
fn phase12_r3_notification_records_error_when_delivery_status_not_sent() {
    let mut runtime = runtime_for_gating_with_forced_failed_delivery();
    let evaluated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase12-r3-delivery-failed",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"publish_release",
                "actor_id":"alice",
                "risk_tier":"r3",
                "requested_approver":"bob",
                "approval_recipient":"approvals@example.com",
                "approval_channel":"transactional",
                "approval_timeout_ms":60_000
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-r3-delivery-failed-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    let audit = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-r3-delivery-failed-audit","method":"meta/gating/audit","params":{"limit":10}}"#,
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-r3-delivery-failed-history","method":"meta/delivery/history","params":{"recipient":"approvals@example.com","limit":5}}"#,
        Duration::from_secs(1),
    ));
    runtime.shutdown();

    let entries = audit["result"]["entries"]
        .as_array()
        .expect("audit entries");
    let pending_created = entries
        .iter()
        .find(|entry| entry.get("event_type") == Some(&json!("pending_created")))
        .expect("pending_created entry");
    let approval_notification = entries
        .iter()
        .find(|entry| entry.get("event_type") == Some(&json!("approval_notification")))
        .expect("approval_notification entry");
    assert_eq!(
        approval_notification["detail"]["disposition"],
        json!("not_delivered")
    );
    let notification_error = approval_notification["detail"]["approval_notification_error"]
        .as_str()
        .expect("delivery failure should be explicit");
    let failed_delivery_id = history["result"]["deliveries"][0]["delivery_id"]
        .as_str()
        .expect("history delivery id");

    assert_eq!(evaluated["result"]["outcome"], json!("pending_approval"));
    assert_eq!(
        pending["result"]["pending"][0]["approval_delivery_id"],
        Value::Null
    );
    assert_eq!(
        pending_created["detail"]["approval_delivery_id"],
        Value::Null
    );
    assert!(notification_error.starts_with("delivery_status:failed:"));
    assert!(
        notification_error.ends_with(failed_delivery_id),
        "notification error should include failed delivery id"
    );
    assert_eq!(
        history["result"]["deliveries"][0]["status"],
        json!("failed")
    );
}

#[test]
#[ignore]
fn phase12_r3_notification_records_error_when_modules_unavailable() {
    let mut runtime = runtime_for_gating();
    let evaluated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase12-r3-notification-modules-missing",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"publish_release",
                "actor_id":"alice",
                "risk_tier":"r3",
                "requested_approver":"bob",
                "approval_recipient":"approvals@example.com",
                "approval_channel":"transactional",
                "approval_timeout_ms":60_000
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-r3-notification-modules-missing-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    let audit = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase12-r3-notification-modules-missing-audit","method":"meta/gating/audit","params":{"limit":10}}"#,
        Duration::from_secs(1),
    ));
    runtime.shutdown();

    let entries = audit["result"]["entries"]
        .as_array()
        .expect("audit entries");
    let pending_created = entries
        .iter()
        .find(|entry| entry.get("event_type") == Some(&json!("pending_created")))
        .expect("pending_created entry");

    assert_eq!(evaluated["result"]["outcome"], json!("pending_approval"));
    assert_eq!(
        pending["result"]["pending"][0]["approval_route_id"],
        Value::Null
    );
    assert_eq!(
        pending["result"]["pending"][0]["approval_delivery_id"],
        Value::Null
    );
    assert_eq!(
        pending_created["detail"]["approval_notification_error"],
        json!("notification_modules_unavailable:router,delivery")
    );
}

#[test]
fn gating_origin_is_opt_in_and_preserved_across_serialization_and_escalation() {
    use meerkat_meta::runtime::{
        GatingDecideRequest, GatingDecision, GatingEvaluateRequest, GatingOrigin,
        GatingPendingEntry,
    };
    let mut runtime = runtime_for_gating();
    let request: GatingEvaluateRequest = serde_json::from_value(json!({
        "action": "Deploy the candidate", "actor_id": "worker", "risk_tier": "r3",
        "rationale": "Release approved changes",
    }))
    .expect("request");
    let origin = GatingOrigin {
        identity: "identity:release".to_string(),
        conversation_id: Some("conversation:release".to_string()),
        interaction_id: Some("interaction:123".to_string()),
    };
    let evaluated = runtime.evaluate_gating_action_with_origin(request, Some(origin.clone()));
    let pending_id = evaluated.pending_id.expect("R3 pending");
    let entry = runtime.list_gating_pending().remove(0);
    assert_eq!(entry.origin.as_ref(), Some(&origin));
    assert_eq!(entry.rationale.as_deref(), Some("Release approved changes"));
    let persisted = serde_json::to_vec(&entry).expect("serialize pending");
    let restored: GatingPendingEntry = serde_json::from_slice(&persisted).expect("restore pending");
    assert_eq!(entry, restored);
    let result = runtime
        .decide_gating_action(GatingDecideRequest {
            pending_id,
            approver_id: "operator".to_string(),
            decision: GatingDecision::Escalate,
            reason: None,
        })
        .expect("escalation");
    let successor = runtime.list_gating_pending().remove(0);
    assert_eq!(Some(successor.pending_id), result.next_pending_id);
    assert_eq!(successor.origin.as_ref(), Some(&origin));
    let audit = runtime.gating_audit_entries(20);
    assert!(
        audit
            .iter()
            .filter(|entry| entry.event_type == "pending_created")
            .all(|entry| {
                entry.detail.get("origin") == Some(&serde_json::to_value(&origin).expect("origin"))
            })
    );
}

#[test]
fn legacy_gating_records_and_rpc_actor_claims_do_not_mint_origin() {
    use meerkat_meta::runtime::GatingPendingEntry;
    let legacy: GatingPendingEntry = serde_json::from_value(json!({
        "pending_id": "old", "action_id": "action", "action": "Legacy request",
        "actor_id": "identity:release", "risk_tier": "r3", "created_at_ms": 1,
        "deadline_at_ms": 2,
    }))
    .expect("legacy record remains readable");
    assert!(legacy.origin.is_none());
    assert!(legacy.rationale.is_none());
    let mut runtime = runtime_for_gating();
    let request = json!({
        "jsonrpc": "2.0", "id": "forged", "method": "meta/gating/evaluate",
        "params": {
            "action": "Deploy", "actor_id": "identity:release", "risk_tier": "r3",
            "origin": { "identity": "identity:release", "conversation_id": "c", "interaction_id": "i" },
        },
    });
    let result = handle_meta_rpc_json(&mut runtime, &request.to_string(), Duration::from_secs(1));
    assert!(parse_response(&result).get("result").is_some());
    assert!(runtime.list_gating_pending()[0].origin.is_none());
}

#[test]
fn gating_owner_snapshot_restores_origin_and_sequence_without_replaying_decisions() {
    use meerkat_meta::runtime::{
        GatingDecideRequest, GatingDecision, GatingEvaluateRequest, GatingOrigin,
        GatingStateSnapshot,
    };
    let mut source = runtime_for_gating();
    let request = || {
        serde_json::from_value::<GatingEvaluateRequest>(json!({
            "action": "Publish release", "actor_id": "worker", "risk_tier": "r3",
            "rationale": "All required checks passed",
        }))
        .expect("request")
    };
    let origin = GatingOrigin {
        identity: "identity:release".to_string(),
        conversation_id: Some("conversation:release".to_string()),
        interaction_id: Some("interaction:release".to_string()),
    };
    let original_id = source
        .evaluate_gating_action_with_origin(request(), Some(origin.clone()))
        .pending_id
        .expect("pending");
    let persisted = serde_json::to_vec(&source.gating_state_snapshot()).expect("serialize owner");
    let snapshot: GatingStateSnapshot =
        serde_json::from_slice(&persisted).expect("restore owner bytes");
    let mut restored = runtime_for_gating();
    restored
        .restore_gating_state(snapshot.clone())
        .expect("pristine restore");
    let restored_snapshot = restored.gating_state_snapshot();
    assert_eq!(restored_snapshot.pending, snapshot.pending);
    assert_eq!(restored_snapshot.audit, snapshot.audit);
    assert_ne!(restored_snapshot.owner_epoch, snapshot.owner_epoch);
    let decision = restored
        .decide_gating_action(GatingDecideRequest {
            pending_id: original_id.clone(),
            approver_id: "operator".to_string(),
            decision: GatingDecision::Escalate,
            reason: None,
        })
        .expect("escalate after restart");
    let successor = restored.list_gating_pending().remove(0);
    assert_ne!(successor.pending_id, original_id);
    assert_eq!(
        Some(&successor.pending_id),
        decision.next_pending_id.as_ref()
    );
    assert_eq!(successor.origin, Some(origin));

    let mut restarted = runtime_for_gating();
    restarted
        .restore_gating_state(restored.gating_state_snapshot())
        .expect("successor restore");
    assert!(
        restarted
            .decide_gating_action(GatingDecideRequest {
                pending_id: original_id,
                approver_id: "operator".to_string(),
                decision: GatingDecision::Approve,
                reason: None,
            })
            .is_err()
    );
    let later_id = restarted
        .evaluate_gating_action(request())
        .pending_id
        .expect("later request");
    assert_ne!(later_id, successor.pending_id);
}

/// Gating IDs key durable state outside the owner (staged memory
/// promotions). An owner restarted without restored gating state must not
/// reissue any ID an earlier process minted, nor a legacy sequential ID.
#[test]
fn restarted_gating_owner_never_reissues_ids() {
    use meerkat_meta::runtime::GatingEvaluateRequest;
    let request = || {
        serde_json::from_value::<GatingEvaluateRequest>(json!({
            "action": "Deploy", "actor_id": "worker", "risk_tier": "r3",
        }))
        .expect("request")
    };
    let ids = |runtime: &mut meerkat_meta::MetaRuntimeHandle| {
        let result = runtime.evaluate_gating_action(request());
        let pending_id = result.pending_id.expect("r3 pends");
        let audit_id = runtime.gating_audit_entries(1).remove(0).audit_id;
        vec![result.action_id, pending_id, audit_id]
    };
    let before_restart = ids(&mut runtime_for_gating());
    let after_restart = ids(&mut runtime_for_gating());
    for id in &after_restart {
        assert!(
            !before_restart.contains(id),
            "restarted owner reissued {id} (before restart: {before_restart:?})"
        );
        for legacy in [
            "gate-action-000000",
            "gate-pending-000001",
            "gate-audit-000002",
        ] {
            assert_ne!(id, legacy, "fresh owner minted a legacy sequential id");
        }
    }
}

fn gating_snapshot_ids(snapshot: &meerkat_meta::runtime::GatingStateSnapshot) -> Vec<String> {
    let mut ids = Vec::new();
    for entry in &snapshot.pending {
        ids.push(entry.pending_id.clone());
        ids.push(entry.action_id.clone());
    }
    for entry in &snapshot.audit {
        ids.push(entry.audit_id.clone());
        ids.push(entry.action_id.clone());
        ids.extend(entry.pending_id.clone());
    }
    ids
}

/// Restored IDs stay exactly as they were; the restoring owner mints under its
/// own fresh epoch, so new IDs cannot alias restored ones.
#[test]
fn restored_owner_keeps_restored_ids_and_mints_under_its_own_epoch() {
    use meerkat_meta::runtime::GatingEvaluateRequest;
    let request = || {
        serde_json::from_value::<GatingEvaluateRequest>(json!({
            "action": "Deploy", "actor_id": "worker", "risk_tier": "r3",
        }))
        .expect("request")
    };
    let mut source = runtime_for_gating();
    source.evaluate_gating_action(request());
    let snapshot = source.gating_state_snapshot();
    let source_epoch = snapshot.owner_epoch.clone().expect("source epoch");

    let mut restored = runtime_for_gating();
    restored
        .restore_gating_state(snapshot.clone())
        .expect("restore");
    let restored_ids = gating_snapshot_ids(&snapshot);
    let result = restored.evaluate_gating_action(request());
    let pending_id = result.pending_id.expect("new gate pends");
    let audit_id = restored.gating_audit_entries(1).remove(0).audit_id;
    let own_epoch = restored
        .gating_state_snapshot()
        .owner_epoch
        .expect("own epoch");
    assert_ne!(own_epoch, source_epoch);
    for id in [&result.action_id, &pending_id, &audit_id] {
        assert!(!restored_ids.contains(id), "{id} aliases a restored id");
        assert!(
            id.contains(&own_epoch) && !id.contains(&source_epoch),
            "{id}"
        );
    }

    // The mixed history validates again in a third owner.
    let mut third = runtime_for_gating();
    third
        .restore_gating_state(restored.gating_state_snapshot())
        .expect("mixed epochs restore");
}

/// Version 1 snapshots (sequential IDs) still restore. Version 2 snapshots may
/// mix legacy IDs, earlier owners' epoch IDs and the exporting owner's IDs.
/// Anything else is refused.
#[test]
fn gating_snapshot_accepts_legacy_and_epoch_ids_and_rejects_malformed_ones() {
    use meerkat_meta::runtime::{
        GatingEvaluateRequest, GatingStateRestoreError, GatingStateSnapshot,
    };
    let mut source = runtime_for_gating();
    source.evaluate_gating_action(
        serde_json::from_value::<GatingEvaluateRequest>(json!({
            "action": "Deploy", "actor_id": "worker", "risk_tier": "r3",
        }))
        .expect("request"),
    );
    let current = source.gating_state_snapshot();
    assert_eq!(current.version, 2);
    let epoch = current.owner_epoch.clone().expect("owner epoch");

    let mut v1_json = serde_json::to_value(&current).expect("snapshot json");
    v1_json["version"] = json!(1);
    v1_json
        .as_object_mut()
        .expect("object")
        .remove("owner_epoch");
    v1_json["next_sequence"] = json!(3);
    v1_json["pending"][0]["action_id"] = json!("gate-action-000000");
    v1_json["pending"][0]["pending_id"] = json!("gate-pending-000001");
    v1_json["audit"][0]["action_id"] = json!("gate-action-000000");
    v1_json["audit"][0]["pending_id"] = json!("gate-pending-000001");
    v1_json["audit"][0]["audit_id"] = json!("gate-audit-000002");
    let v1: GatingStateSnapshot = serde_json::from_value(v1_json.clone()).expect("v1 parses");
    runtime_for_gating()
        .restore_gating_state(v1.clone())
        .expect("released version 1 snapshot restores");

    let mut mixed = current.clone();
    mixed.pending.push(v1.pending[0].clone());
    mixed.audit[0].audit_id = format!("gate-audit-v2-{}-000007", "0".repeat(32));
    runtime_for_gating()
        .restore_gating_state(mixed)
        .expect("legacy, earlier-owner and own ids restore together");

    let mut invalid = Vec::new();
    let mut v1_with_epoch_id = v1_json.clone();
    v1_with_epoch_id["pending"][0]["pending_id"] = json!(current.pending[0].pending_id);
    invalid.push(serde_json::from_value::<GatingStateSnapshot>(v1_with_epoch_id).expect("parse"));
    let mut v1_with_owner = v1.clone();
    v1_with_owner.owner_epoch = Some(epoch.clone());
    invalid.push(v1_with_owner);
    for pending_id in [
        format!("gate-pending-v2-{epoch}-{:06}", current.next_sequence),
        format!("gate-pending-v3-{epoch}-000001"),
        "gate-pending-v2-XYZ-000001".to_string(),
        format!("gate-pending-v2-{}-000001", epoch.to_uppercase()),
        format!("gate-pending-v2-{}-000001", &epoch[..31]),
        format!("gate-pending-v2-{epoch}-1"),
        format!("gate-action-v2-{epoch}-000001"),
    ] {
        let mut snapshot = current.clone();
        snapshot.pending[0].pending_id = pending_id;
        invalid.push(snapshot);
    }
    let mut bad_owner = current.clone();
    bad_owner.owner_epoch = Some("not-hex".to_string());
    invalid.push(bad_owner);
    for snapshot in invalid {
        assert!(
            matches!(
                runtime_for_gating().restore_gating_state(snapshot.clone()),
                Err(GatingStateRestoreError::InvalidSnapshot(_))
            ),
            "accepted {snapshot:?}"
        );
    }
    let mut future = current;
    future.version = 3;
    assert_eq!(
        runtime_for_gating().restore_gating_state(future),
        Err(GatingStateRestoreError::UnsupportedVersion(3))
    );
}

#[test]
fn gating_snapshot_validation_is_atomic_and_cannot_overwrite_a_live_owner() {
    use meerkat_meta::runtime::{GatingEvaluateRequest, GatingStateRestoreError};
    let mut source = runtime_for_gating();
    source.evaluate_gating_action(
        serde_json::from_value::<GatingEvaluateRequest>(json!({
            "action": "Deploy", "actor_id": "worker", "risk_tier": "r3",
        }))
        .expect("request"),
    );
    let valid = source.gating_state_snapshot();
    assert!(matches!(
        source.restore_gating_state(valid.clone()),
        Err(GatingStateRestoreError::RuntimeNotPristine)
    ));
    let mut corruptions = Vec::new();
    let mut unsupported = valid.clone();
    unsupported.version = 99;
    corruptions.push(unsupported);
    let mut duplicate = valid.clone();
    duplicate.pending.push(duplicate.pending[0].clone());
    corruptions.push(duplicate);
    let mut frontier = valid.clone();
    frontier.next_sequence = 1;
    corruptions.push(frontier);
    let mut origin = valid.clone();
    origin.pending[0].origin = Some(meerkat_meta::runtime::GatingOrigin {
        identity: "".to_string(),
        conversation_id: None,
        interaction_id: None,
    });
    corruptions.push(origin);
    let mut audit = valid.clone();
    audit.audit[0].detail["origin"] = json!({ "identity": 12 });
    corruptions.push(audit);
    let mut deadline = valid.clone();
    deadline.pending[0].deadline_at_ms = 0;
    corruptions.push(deadline);
    for invalid in corruptions {
        let mut target = runtime_for_gating();
        let before = target.gating_state_snapshot();
        assert!(target.restore_gating_state(invalid).is_err());
        assert_eq!(target.gating_state_snapshot(), before);
        target
            .restore_gating_state(valid.clone())
            .expect("failed validation leaves owner pristine");
    }
}

#[test]
fn gating_restore_retains_legacy_records_and_applies_owner_expiry() {
    use meerkat_meta::runtime::{GatingEvaluateRequest, GatingStateSnapshot};
    let mut source = runtime_for_gating();
    source.evaluate_gating_action(
        serde_json::from_value::<GatingEvaluateRequest>(json!({
            "action": "Legacy deploy", "actor_id": "worker", "risk_tier": "r3",
        }))
        .expect("request"),
    );
    let mut json = serde_json::to_value(source.gating_state_snapshot()).expect("snapshot");
    let pending = json["pending"][0].as_object_mut().expect("pending record");
    pending.remove("origin");
    pending.remove("rationale");
    pending.insert("created_at_ms".to_string(), json!(1));
    pending.insert("deadline_at_ms".to_string(), json!(2));
    let snapshot: GatingStateSnapshot =
        serde_json::from_value(json).expect("legacy optional fields");
    let mut restored = runtime_for_gating();
    restored
        .restore_gating_state(snapshot)
        .expect("legacy restore");
    assert!(restored.list_gating_pending().is_empty());
    let audit = restored.gating_audit_entries(20);
    let timeout = audit
        .iter()
        .find(|entry| entry.event_type == "timeout_fallback")
        .expect("canonical timeout");
    assert_eq!(
        timeout.outcome,
        meerkat_meta::runtime::GatingOutcome::SafeDraft
    );
    assert_eq!(timeout.detail["reason"], json!("approval_timeout"));
}

#[test]
fn gating_empty_optional_origin_fields_preserve_identity_authority() {
    use meerkat_meta::runtime::{GatingEvaluateRequest, GatingOrigin};
    for (conversation_id, interaction_id) in [
        (Some("  ".to_string()), Some("private-turn".to_string())),
        (
            Some("private-conversation".to_string()),
            Some("\t".to_string()),
        ),
        (Some(String::new()), Some(String::new())),
    ] {
        let mut runtime = runtime_for_gating();
        let request: GatingEvaluateRequest = serde_json::from_value(json!({
            "action": "Private release", "actor_id": "worker", "risk_tier": "r3",
            "rationale": "Private context",
        }))
        .expect("request");
        let expected = GatingOrigin {
            identity: "hidden-reviewer".to_string(),
            conversation_id: conversation_id
                .clone()
                .filter(|value| !value.trim().is_empty()),
            interaction_id: interaction_id
                .clone()
                .filter(|value| !value.trim().is_empty()),
        };
        let evaluated = runtime.evaluate_gating_action_with_origin(
            request,
            Some(GatingOrigin {
                identity: expected.identity.clone(),
                conversation_id,
                interaction_id,
            }),
        );
        assert!(evaluated.pending_id.is_some());
        assert_eq!(
            runtime.list_gating_pending()[0].origin,
            Some(expected.clone())
        );
        assert_eq!(
            runtime.gating_audit_entries(20)[0].detail["origin"],
            json!(expected)
        );
        let snapshot = runtime.gating_state_snapshot();
        let mut restored = runtime_for_gating();
        restored
            .restore_gating_state(snapshot.clone())
            .expect("normalized snapshot");
        let restored_snapshot = restored.gating_state_snapshot();
        assert_eq!(restored_snapshot.pending, snapshot.pending);
        assert_eq!(restored_snapshot.audit, snapshot.audit);
    }
}

#[test]
fn gating_invalid_supplied_identity_cannot_become_unattributed() {
    use meerkat_meta::runtime::{GatingEvaluateRequest, GatingOrigin, GatingOutcome};
    for identity in ["", "  ", "\t"] {
        let mut runtime = runtime_for_gating();
        let request: GatingEvaluateRequest = serde_json::from_value(json!({
            "action": "Private release", "actor_id": "worker", "risk_tier": "r3",
            "rationale": "Private context",
        }))
        .expect("request");
        let evaluated = runtime.evaluate_gating_action_with_origin(
            request,
            Some(GatingOrigin {
                identity: identity.to_string(),
                conversation_id: None,
                interaction_id: Some("private-turn".to_string()),
            }),
        );
        assert_eq!(evaluated.outcome, GatingOutcome::SafeDraft);
        assert_eq!(
            evaluated.fallback_reason.as_deref(),
            Some("invalid_gating_origin")
        );
        assert!(evaluated.pending_id.is_none());
        assert!(runtime.list_gating_pending().is_empty());
        assert!(runtime.gating_audit_entries(20).is_empty());
    }
}
