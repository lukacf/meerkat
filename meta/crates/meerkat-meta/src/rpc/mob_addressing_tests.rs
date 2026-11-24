//! Rows for the addressing id of a request: `mob_id`.
//!
//! A request names its mob with a top-level `mob_id`; without one it
//! addresses the default mob. An id the host does not have is refused with
//! `foreign_mob_target` on every addressed method, on the stdio dispatch and
//! on the console RPC, and on the event stream routes. The methods of the
//! announcement family that carry a mob of their own keep the family's
//! answer.
//!
//! The method table below is kept by hand. A dispatcher arm or a family
//! registry entry the table lacks fails
//! `every_method_a_dispatcher_serves_is_in_the_addressing_table`.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tower::ServiceExt;

use crate::UnifiedRuntime;

/// What an id the host does not have answers on a method.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UnknownMob {
    /// The refusal `foreign_mob_target`.
    Foreign,
    /// The method carries a mob of its own and its family answers for it.
    FamilyAnswer,
}

/// Every addressed method that a dispatcher serves today: its name, whether
/// the stdio dispatch and the console RPC have an arm for it, and what an id
/// the host does not have answers there.
const ADDRESSED_METHODS: &[(&str, bool, bool, UnknownMob)] = &[
    ("live/webrtc/answer", true, false, UnknownMob::Foreign),
    ("meta/access/enable", false, true, UnknownMob::Foreign),
    ("meta/access/get", false, true, UnknownMob::Foreign),
    (
        "meta/access/groups/delete",
        false,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/access/groups/set", false, true, UnknownMob::Foreign),
    ("meta/access/preview", false, true, UnknownMob::Foreign),
    ("meta/access/rules/delete", false, true, UnknownMob::Foreign),
    ("meta/access/rules/upsert", false, true, UnknownMob::Foreign),
    ("meta/access/set", false, true, UnknownMob::Foreign),
    ("meta/access/status", false, true, UnknownMob::Foreign),
    (
        "meta/agent_definitions/list",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/agent_memory/forget", true, true, UnknownMob::Foreign),
    (
        "meta/agent_memory/manifest",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/agent_memory/recall", true, true, UnknownMob::Foreign),
    (
        "meta/agent_memory/remember",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/agent_memory/update", true, true, UnknownMob::Foreign),
    (
        "meta/attach_existing_session",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/blob/get", true, true, UnknownMob::Foreign),
    ("meta/blob/upload", false, true, UnknownMob::Foreign),
    (
        "meta/bound_member_transcript",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/call_tool", true, false, UnknownMob::Foreign),
    ("meta/cancel_flow", true, true, UnknownMob::Foreign),
    ("meta/capabilities", true, true, UnknownMob::Foreign),
    ("meta/collect_completed", true, true, UnknownMob::Foreign),
    ("meta/compact_member", true, false, UnknownMob::Foreign),
    ("meta/completion_cursor", true, false, UnknownMob::Foreign),
    ("meta/console/attention", false, true, UnknownMob::Foreign),
    (
        "meta/console/attention/dismiss",
        false,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/console/audit", false, true, UnknownMob::Foreign),
    (
        "meta/console/inspect_identity",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/console/list_identities",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/console/query_timeline",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/console/recover_send",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/console/revoke_sessions",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/console/send", false, true, UnknownMob::Foreign),
    ("meta/console/sign_out", false, true, UnknownMob::Foreign),
    (
        "meta/console/voice/activity",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/console/voice/answer_received",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/console/voice/captions",
        false,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/console/voice/close", false, true, UnknownMob::Foreign),
    (
        "meta/console/voice/context_status",
        false,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/console/voice/open", false, true, UnknownMob::Foreign),
    (
        "meta/console/voice/readiness",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/console/voice/replacement",
        false,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/console/whoami", false, true, UnknownMob::Foreign),
    ("meta/cross_mob/directory", true, true, UnknownMob::Foreign),
    ("meta/cross_mob/peer_info", true, true, UnknownMob::Foreign),
    ("meta/cross_mob/send", true, false, UnknownMob::Foreign),
    ("meta/cross_mob/unwire", true, false, UnknownMob::Foreign),
    (
        "meta/cross_mob/unwire_local",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/cross_mob/wire", true, false, UnknownMob::Foreign),
    ("meta/cross_mob/wire_local", true, true, UnknownMob::Foreign),
    ("meta/decisions/decide", false, true, UnknownMob::Foreign),
    ("meta/decisions/get", false, true, UnknownMob::Foreign),
    ("meta/decisions/list", false, true, UnknownMob::Foreign),
    ("meta/decisions/notice", false, true, UnknownMob::Foreign),
    ("meta/decisions/present", false, true, UnknownMob::Foreign),
    ("meta/decisions/rendering", false, true, UnknownMob::Foreign),
    ("meta/decisions/withdraw", false, true, UnknownMob::Foreign),
    ("meta/delete_identity", true, true, UnknownMob::Foreign),
    ("meta/delivery/history", true, true, UnknownMob::Foreign),
    ("meta/delivery/send", true, false, UnknownMob::Foreign),
    ("meta/dispatch", true, false, UnknownMob::Foreign),
    ("meta/ensure_member", true, true, UnknownMob::Foreign),
    ("meta/events/subscribe", true, false, UnknownMob::Foreign),
    ("meta/find_members", true, true, UnknownMob::Foreign),
    ("meta/flow_status", true, true, UnknownMob::Foreign),
    ("meta/force_cancel_member", true, true, UnknownMob::Foreign),
    ("meta/fork_helper", true, true, UnknownMob::Foreign),
    ("meta/fork_member", false, true, UnknownMob::Foreign),
    ("meta/gating/audit", true, true, UnknownMob::Foreign),
    ("meta/gating/decide", true, true, UnknownMob::Foreign),
    ("meta/gating/evaluate", true, false, UnknownMob::Foreign),
    ("meta/gating/pending", true, true, UnknownMob::Foreign),
    ("meta/gating/questions", true, true, UnknownMob::Foreign),
    ("meta/gating/withdraw", true, false, UnknownMob::Foreign),
    ("meta/get_member", true, true, UnknownMob::Foreign),
    ("meta/identity/adopt", true, true, UnknownMob::Foreign),
    ("meta/identity/archive", true, true, UnknownMob::Foreign),
    (
        "meta/identity/changes_hold",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/identity/create", true, true, UnknownMob::Foreign),
    ("meta/identity/directory", true, true, UnknownMob::Foreign),
    ("meta/identity/get", true, true, UnknownMob::Foreign),
    ("meta/identity/list", true, true, UnknownMob::Foreign),
    ("meta/identity/operation", true, true, UnknownMob::Foreign),
    ("meta/identity/plan", true, true, UnknownMob::Foreign),
    ("meta/identity/remove", true, true, UnknownMob::Foreign),
    (
        "meta/identity/resolved_tools",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/identity/routing_status",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/identity/settle_preparation",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/identity/unarchive", true, true, UnknownMob::Foreign),
    ("meta/identity/update", true, true, UnknownMob::Foreign),
    ("meta/identity/warm", true, true, UnknownMob::Foreign),
    ("meta/identity/withdraw", true, true, UnknownMob::Foreign),
    ("meta/inspect_identity", true, true, UnknownMob::Foreign),
    ("meta/instruction_status", true, false, UnknownMob::Foreign),
    ("meta/interact", true, false, UnknownMob::Foreign),
    ("meta/list_flows", true, true, UnknownMob::Foreign),
    ("meta/list_members", true, true, UnknownMob::Foreign),
    ("meta/list_runs", true, true, UnknownMob::Foreign),
    ("meta/live/close", true, false, UnknownMob::Foreign),
    ("meta/live/commit_input", true, false, UnknownMob::Foreign),
    ("meta/live/interrupt", true, false, UnknownMob::Foreign),
    ("meta/live/open", true, false, UnknownMob::Foreign),
    (
        "meta/live/playback_complete",
        true,
        false,
        UnknownMob::Foreign,
    ),
    (
        "meta/live/playback_owner/register",
        true,
        false,
        UnknownMob::Foreign,
    ),
    (
        "meta/live/playback_owner/revoke",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/live/refresh", true, false, UnknownMob::Foreign),
    (
        "meta/live/replacement_required",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/live/send_input", true, false, UnknownMob::Foreign),
    ("meta/live/status", true, false, UnknownMob::Foreign),
    ("meta/live/superseded", true, false, UnknownMob::Foreign),
    ("meta/live/truncate", true, false, UnknownMob::Foreign),
    (
        "meta/member_activate_instruction",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/member_health", true, true, UnknownMob::Foreign),
    (
        "meta/member_instruction_activations",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/member_status", true, true, UnknownMob::Foreign),
    ("meta/memory/index", true, false, UnknownMob::Foreign),
    (
        "meta/memory/panel/audit_verdicts",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/memory/panel/dream_runs",
        false,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/memory/panel/dreams", false, true, UnknownMob::Foreign),
    (
        "meta/memory/panel/harvests",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/memory/panel/injections",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/memory/panel/overview",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/memory/panel/proposals",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/memory/panel/quarantine",
        false,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/memory/panel/record", false, true, UnknownMob::Foreign),
    (
        "meta/memory/panel/records",
        false,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/memory/quarantine/decide",
        false,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/memory/query", true, false, UnknownMob::Foreign),
    ("meta/memory/stores", true, false, UnknownMob::Foreign),
    (
        "meta/mob_announcements/announce",
        true,
        true,
        UnknownMob::FamilyAnswer,
    ),
    (
        "meta/mob_announcements/audit/list",
        true,
        true,
        UnknownMob::FamilyAnswer,
    ),
    (
        "meta/mob_announcements/declarations/apply",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/declarations/get",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/deliveries/dismiss",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/deliveries/list",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/deliveries/observe",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/deliveries/reconcile",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/deliveries/redeliver",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/escalations/list",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/get",
        true,
        true,
        UnknownMob::FamilyAnswer,
    ),
    (
        "meta/mob_announcements/members/get",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/members/list",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/sources/advance",
        true,
        true,
        UnknownMob::FamilyAnswer,
    ),
    (
        "meta/mob_announcements/sources/end",
        true,
        true,
        UnknownMob::FamilyAnswer,
    ),
    (
        "meta/mob_announcements/sources/get",
        true,
        true,
        UnknownMob::FamilyAnswer,
    ),
    (
        "meta/mob_announcements/sources/list",
        true,
        true,
        UnknownMob::FamilyAnswer,
    ),
    (
        "meta/mob_announcements/sources/pause",
        true,
        true,
        UnknownMob::FamilyAnswer,
    ),
    (
        "meta/mob_announcements/sources/put",
        true,
        true,
        UnknownMob::FamilyAnswer,
    ),
    (
        "meta/mob_announcements/sources/record_failure",
        true,
        true,
        UnknownMob::FamilyAnswer,
    ),
    (
        "meta/mob_announcements/sources/resume",
        true,
        true,
        UnknownMob::FamilyAnswer,
    ),
    (
        "meta/mob_announcements/subscriptions/cancel",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/subscriptions/get",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/subscriptions/list",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/subscriptions/pause",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/subscriptions/put",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/mob_announcements/subscriptions/resume",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/mob_events/query", true, true, UnknownMob::Foreign),
    ("meta/mob_events/subscribe", true, true, UnknownMob::Foreign),
    ("meta/mob_labels/delete", true, true, UnknownMob::Foreign),
    ("meta/mob_labels/get", true, true, UnknownMob::Foreign),
    ("meta/mob_labels/set", true, true, UnknownMob::Foreign),
    (
        "meta/mobpacks/apply_operation",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/mobpacks/catalogs", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/create", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/delete", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/deploy", true, false, UnknownMob::Foreign),
    (
        "meta/mobpacks/deploy_command",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/mobpacks/export", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/get", true, false, UnknownMob::Foreign),
    (
        "meta/mobpacks/graph_projection",
        true,
        false,
        UnknownMob::Foreign,
    ),
    (
        "meta/mobpacks/graph_to_flow",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/mobpacks/import", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/list", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/redo", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/save", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/schema", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/source", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/templates", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/undo", true, false, UnknownMob::Foreign),
    ("meta/mobpacks/validate", true, false, UnknownMob::Foreign),
    ("meta/mobs/create", true, true, UnknownMob::Foreign),
    ("meta/mobs/destroy", true, true, UnknownMob::FamilyAnswer),
    ("meta/mobs/get", true, true, UnknownMob::FamilyAnswer),
    ("meta/mobs/lifecycle", true, true, UnknownMob::FamilyAnswer),
    ("meta/mobs/list", true, true, UnknownMob::Foreign),
    ("meta/mobs/redefine", true, true, UnknownMob::FamilyAnswer),
    ("meta/models/catalog", true, false, UnknownMob::Foreign),
    ("meta/peer_pubkey", true, true, UnknownMob::Foreign),
    (
        "meta/prune_member_system_rows",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/query_events", true, false, UnknownMob::Foreign),
    ("meta/realm_mcp/install", true, false, UnknownMob::Foreign),
    (
        "meta/realm_mcp/remove_secret",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/realm_mcp/servers", true, false, UnknownMob::Foreign),
    (
        "meta/realm_mcp/set_secret",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/reconcile", true, false, UnknownMob::Foreign),
    ("meta/reconcile_edges", true, true, UnknownMob::Foreign),
    ("meta/reconcile_identity", true, false, UnknownMob::Foreign),
    ("meta/recover_delivery", true, false, UnknownMob::Foreign),
    ("meta/rediscover", true, false, UnknownMob::Foreign),
    ("meta/registered_spec", true, false, UnknownMob::Foreign),
    ("meta/reload_member", true, true, UnknownMob::Foreign),
    ("meta/reset", true, true, UnknownMob::Foreign),
    ("meta/reset_all", false, true, UnknownMob::Foreign),
    ("meta/respawn", true, true, UnknownMob::Foreign),
    ("meta/respawn_member", true, true, UnknownMob::Foreign),
    ("meta/retire", true, true, UnknownMob::Foreign),
    ("meta/retire_member", true, true, UnknownMob::Foreign),
    ("meta/revoke_instruction", true, false, UnknownMob::Foreign),
    ("meta/routing/resolve", true, false, UnknownMob::Foreign),
    ("meta/routing/routes/add", true, false, UnknownMob::Foreign),
    (
        "meta/routing/routes/delete",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/routing/routes/list", true, true, UnknownMob::Foreign),
    ("meta/run_flow", true, true, UnknownMob::Foreign),
    ("meta/run_labels/delete", true, true, UnknownMob::Foreign),
    ("meta/run_labels/get", true, true, UnknownMob::Foreign),
    ("meta/run_labels/set", true, true, UnknownMob::Foreign),
    ("meta/schedule/create", false, true, UnknownMob::Foreign),
    ("meta/schedule/delete", false, true, UnknownMob::Foreign),
    ("meta/schedule/list", false, true, UnknownMob::Foreign),
    ("meta/schedule/pause", false, true, UnknownMob::Foreign),
    ("meta/schedule/resume", false, true, UnknownMob::Foreign),
    ("meta/schedule/tools", true, true, UnknownMob::Foreign),
    ("meta/schedule/update", false, true, UnknownMob::Foreign),
    ("meta/send", true, false, UnknownMob::Foreign),
    ("meta/send_message", true, false, UnknownMob::Foreign),
    ("meta/send_status", true, true, UnknownMob::Foreign),
    (
        "meta/session_store/bigquery",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/set_instruction", true, false, UnknownMob::Foreign),
    ("meta/skills/catalog", true, false, UnknownMob::Foreign),
    ("meta/spawn_helper", true, true, UnknownMob::Foreign),
    ("meta/spawn_member", true, false, UnknownMob::Foreign),
    ("meta/status", true, true, UnknownMob::Foreign),
    ("meta/status_identity", true, true, UnknownMob::Foreign),
    (
        "meta/status_identity_bootstrap",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/stop_member_run", true, true, UnknownMob::Foreign),
    ("meta/storage/doctor", true, true, UnknownMob::Foreign),
    ("meta/subscribe", true, false, UnknownMob::Foreign),
    ("meta/tool_grants/audit", true, true, UnknownMob::Foreign),
    ("meta/tool_grants/grant", true, true, UnknownMob::Foreign),
    ("meta/tool_grants/list", true, true, UnknownMob::Foreign),
    ("meta/tool_grants/revoke", true, true, UnknownMob::Foreign),
    ("meta/tools/catalog", true, false, UnknownMob::Foreign),
    ("meta/topology/apply", true, true, UnknownMob::Foreign),
    ("meta/topology/audit/query", true, true, UnknownMob::Foreign),
    (
        "meta/topology/operation/get",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/topology/plan", true, true, UnknownMob::Foreign),
    ("meta/topology/query", true, true, UnknownMob::Foreign),
    ("meta/turn_result", true, false, UnknownMob::Foreign),
    ("meta/wait_for_completion", true, false, UnknownMob::Foreign),
    ("meta/wait_for_turn", true, false, UnknownMob::Foreign),
    (
        "meta/wait_identity_bootstrap",
        true,
        false,
        UnknownMob::Foreign,
    ),
    ("meta/wait_ready", true, true, UnknownMob::Foreign),
    (
        "meta/workgraph/attention/break_glass_reassign",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/workgraph/attention/list",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/workgraph/attention/pause",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/workgraph/attention/prune",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/workgraph/attention/reassign",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/workgraph/attention/resume",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/workgraph/block", true, true, UnknownMob::Foreign),
    ("meta/workgraph/claim", true, true, UnknownMob::Foreign),
    ("meta/workgraph/close", true, true, UnknownMob::Foreign),
    ("meta/workgraph/create", true, true, UnknownMob::Foreign),
    ("meta/workgraph/events", true, true, UnknownMob::Foreign),
    (
        "meta/workgraph/evidence/add",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/workgraph/facts", true, true, UnknownMob::Foreign),
    ("meta/workgraph/get", true, true, UnknownMob::Foreign),
    (
        "meta/workgraph/goal/confirm",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/workgraph/goal/create",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/workgraph/goal/request_close",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "meta/workgraph/goal/status",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/workgraph/link", true, true, UnknownMob::Foreign),
    ("meta/workgraph/list", true, true, UnknownMob::Foreign),
    (
        "meta/workgraph/policy/escalate",
        true,
        true,
        UnknownMob::Foreign,
    ),
    ("meta/workgraph/ready", true, true, UnknownMob::Foreign),
    ("meta/workgraph/release", true, true, UnknownMob::Foreign),
    ("meta/workgraph/snapshot", true, true, UnknownMob::Foreign),
    ("meta/workgraph/update", true, true, UnknownMob::Foreign),
    (
        "mob/adopt_member_identity_declaration",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "mob/apply_member_tool_declaration",
        true,
        true,
        UnknownMob::Foreign,
    ),
    (
        "mob/member_tool_declaration",
        true,
        true,
        UnknownMob::Foreign,
    ),
];

/// An id no mob of the fixture host has.
const ABSENT_MOB: &str = "addressing-rows-absent-mob";

/// Methods whose own parameter `mob_id` is and whose answer differs between
/// no mob and the default mob's id on purpose: the subscription answers the
/// address of its stream, which names the mob, and the three declaration
/// methods require the id and go on to their next parameter with it.
const NAMES_ITS_MOB_IN_ITS_ANSWER: &[&str] = &[
    "meta/mob_events/subscribe",
    "mob/adopt_member_identity_declaration",
    "mob/apply_member_tool_declaration",
    "mob/member_tool_declaration",
];

/// A runtime over one mob, the default mob, with no identity plane.
async fn runtime() -> UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"addressing-rows-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    UnifiedRuntime::builder()
        .definition(definition)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("runtime builds")
}

/// The params of a request that names `mob_id` and nothing else a method
/// could act on. A mobpack authoring request also names a draft store file
/// under `scratch`, so that no request of these rows reaches the host's own
/// draft store.
fn params_naming(method: &str, mob_id: Option<&str>, scratch: &std::path::Path) -> Value {
    let mut params = serde_json::Map::new();
    if let Some(mob_id) = mob_id {
        params.insert("mob_id".to_string(), json!(mob_id));
    }
    if method.starts_with("meta/mobpacks/") {
        params.insert(
            "store_path".to_string(),
            json!(scratch.join("drafts.json").display().to_string()),
        );
    }
    Value::Object(params)
}

/// Whether `answer` is the refusal of a mob the host does not have, naming
/// the requested id. The stdio dispatch answers its host and names the
/// host's default mob beside it (`default_mob` is `Some`); a door that has
/// callers names no other mob (`None`).
fn is_foreign_mob_target(answer: &Value, requested: &str, default_mob: Option<&str>) -> bool {
    answer["error"]["code"] == json!(-32602)
        && answer["error"]["data"]["kind"] == json!("foreign_mob_target")
        && answer["error"]["data"]["requested_mob_id"] == json!(requested)
        && match default_mob {
            Some(default_mob) => answer["error"]["data"]["gateway_mob_id"] == json!(default_mob),
            None => answer["error"]["data"].get("gateway_mob_id").is_none(),
        }
}

/// One request on the stdio dispatch. `None` when it did not answer in time.
async fn stdio(runtime: &UnifiedRuntime, method: &str, params: Value) -> Option<Value> {
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let answer = tokio::time::timeout(
        Duration::from_secs(10),
        crate::rpc::handle_unified_rpc_json(runtime, &request, Duration::from_secs(1), None, None),
    )
    .await
    .ok()?;
    serde_json::from_str(&answer).ok()
}

/// The reference application of `runtime`, as an open console.
fn open_app(runtime: &UnifiedRuntime) -> axum::Router {
    runtime.build_reference_app_router(crate::runtime::RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            ..Default::default()
        },
        None,
    ))
}

/// One request on the console RPC. `None` when it did not answer in time or
/// answered no JSON.
async fn console(app: &axum::Router, method: &str, params: Value) -> Option<Value> {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({"jsonrpc": "2.0", "id": "row", "method": method, "params": params}).to_string(),
        ))
        .expect("console request");
    let response = tokio::time::timeout(Duration::from_secs(10), app.clone().oneshot(request))
        .await
        .ok()?
        .ok()?;
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .ok()?;
    serde_json::from_slice(&body).ok()
}

/// One request on the console's multipart route: a JSON-RPC payload in the
/// part `payload`, with no file. `None` when it answered no JSON.
async fn console_multipart(app: &axum::Router, method: &str, params: Value) -> Option<Value> {
    let payload = json!({"jsonrpc": "2.0", "id": "row", "method": method, "params": params});
    let body = format!(
        "--rows\r\nContent-Disposition: form-data; name=\"payload\"\r\n\r\n{payload}\r\n--rows--\r\n"
    );
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc/multipart")
        .header(
            axum::http::header::CONTENT_TYPE,
            "multipart/form-data; boundary=rows",
        )
        .body(axum::body::Body::from(body))
        .expect("multipart request");
    let response = app.clone().oneshot(request).await.ok()?;
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .ok()?;
    serde_json::from_slice(&body).ok()
}

/// A GET on `uri`: the status and, when the body is JSON, the body. The body
/// of a stream is not read.
async fn get(app: &axum::Router, uri: &str) -> (axum::http::StatusCode, Option<Value>) {
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .expect("stream request");
    let response = app.clone().oneshot(request).await.expect("stream response");
    let status = response.status();
    let json = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if !json {
        return (status, None);
    }
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("refusal body");
    (status, serde_json::from_slice(&body).ok())
}

/// The methods of `ADDRESSED_METHODS` that did not answer as the table says
/// for [`ABSENT_MOB`], each with what it answered.
fn not_refused(
    answers: Vec<(&'static str, UnknownMob, Option<Value>)>,
    default_mob: Option<&str>,
) -> Vec<String> {
    answers
        .into_iter()
        .filter_map(|(method, unknown, answer)| {
            let foreign = answer
                .as_ref()
                .is_some_and(|answer| is_foreign_mob_target(answer, ABSENT_MOB, default_mob));
            let as_the_table_says = match unknown {
                UnknownMob::Foreign => foreign,
                UnknownMob::FamilyAnswer => answer.is_some() && !foreign,
            };
            (!as_the_table_says).then(|| {
                let answered = answer.map_or_else(
                    || "no answer in time".to_string(),
                    |answer| {
                        let text = answer.get("error").unwrap_or(&answer).to_string();
                        text.chars().take(120).collect()
                    },
                );
                format!("{method}: {answered}")
            })
        })
        .collect()
}

/// An id the host does not have is `foreign_mob_target` on every addressed
/// method of the stdio dispatch, whatever else the request holds and
/// whether or not this runtime serves the method's family.
#[tokio::test]
async fn an_id_the_host_does_not_have_is_refused_on_every_addressed_stdio_method() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let mut answers = Vec::new();
    for (method, on_stdio, _, unknown) in ADDRESSED_METHODS {
        if !on_stdio {
            continue;
        }
        let params = params_naming(method, Some(ABSENT_MOB), scratch.path());
        answers.push((*method, *unknown, stdio(&runtime, method, params).await));
    }
    let asked = answers.len();
    let wrong = not_refused(answers, Some(default_mob.as_str()));
    runtime.shutdown().await;
    assert!(
        wrong.is_empty(),
        "{} of {asked} stdio methods did not refuse a mob the host does not have:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// The same on the console RPC, whose refusal names the requested id and
/// no other mob.
#[tokio::test]
async fn an_id_the_host_does_not_have_is_refused_on_every_addressed_console_method() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let runtime = runtime().await;
    let app = open_app(&runtime);
    let mut answers = Vec::new();
    for (method, _, on_console, unknown) in ADDRESSED_METHODS {
        if !on_console {
            continue;
        }
        let params = params_naming(method, Some(ABSENT_MOB), scratch.path());
        answers.push((*method, *unknown, console(&app, method, params).await));
    }
    let asked = answers.len();
    let wrong = not_refused(answers, None);
    drop(app);
    runtime.shutdown().await;
    assert!(
        wrong.is_empty(),
        "{} of {asked} console methods did not refuse a mob the host does not have:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// The default mob's own id is the same request as no mob: every addressed
/// method answers the two alike, on both surfaces. A method that refuses
/// members it does not know must not see `mob_id`.
#[tokio::test]
async fn the_default_mobs_id_is_the_same_request_as_no_mob() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let app = open_app(&runtime);
    let mut differing = Vec::new();
    for (method, on_stdio, on_console, unknown) in ADDRESSED_METHODS {
        // A method that carries a mob of its own may answer with the mob the
        // request named (a stream address, the next missing parameter).
        if *unknown == UnknownMob::FamilyAnswer || NAMES_ITS_MOB_IN_ITS_ANSWER.contains(method) {
            continue;
        }
        let unnamed = params_naming(method, None, scratch.path());
        let named = params_naming(method, Some(&default_mob), scratch.path());
        // A method whose answer is new at every call (a clock, a cursor, a
        // minted id, a write) cannot be compared: it is asked twice with no
        // mob first, and left out when those two answers differ.
        if *on_stdio {
            let plain = stdio(&runtime, method, unnamed.clone()).await;
            if plain == stdio(&runtime, method, unnamed.clone()).await
                && plain != stdio(&runtime, method, named.clone()).await
            {
                differing.push(format!("stdio {method}"));
            }
        }
        if *on_console {
            let plain = console(&app, method, unnamed.clone()).await;
            if plain == console(&app, method, unnamed).await
                && plain != console(&app, method, named).await
            {
                differing.push(format!("console {method}"));
            }
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(
        differing.is_empty(),
        "{} methods answer the default mob's id unlike no mob:\n{}",
        differing.len(),
        differing.join("\n")
    );
}

/// The console's request door says that it judges `mob_id`, as the stdio
/// dispatch does: the feature capabilities of its `meta/capabilities` hold
/// `mob_addressing`, once, for a request that names no mob and for one that
/// names the default mob. `hosted_mobs` is not listed: no method that hosts
/// another mob exists.
#[tokio::test]
async fn the_console_rpc_lists_the_mob_addressing_capability() {
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let app = open_app(&runtime);
    let mut wrong = Vec::new();
    for params in [json!({}), json!({"mob_id": default_mob.as_str()})] {
        let answer = console(&app, "meta/capabilities", params.clone()).await;
        let names: Vec<String> = answer
            .as_ref()
            .and_then(|answer| answer["result"]["feature_capabilities"].as_array())
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let listed = names
            .iter()
            .filter(|name| *name == "mob_addressing")
            .count();
        if listed != 1 {
            wrong.push(format!("{params}: mob_addressing x{listed} in {names:?}"));
        }
        if names.iter().any(|name| name == "hosted_mobs") {
            wrong.push(format!("{params}: lists hosted_mobs"));
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The console's multipart route carries the id in the payload's params and
/// refuses a mob the host does not have like the console RPC does.
#[tokio::test]
async fn the_multipart_route_refuses_a_mob_the_host_does_not_have() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let runtime = runtime().await;
    let app = open_app(&runtime);
    let mut wrong = Vec::new();
    for method in ["meta/console/send", "meta/blob/upload"] {
        let params = params_naming(method, Some(ABSENT_MOB), scratch.path());
        let answer = console_multipart(&app, method, params).await;
        let refused = answer
            .as_ref()
            .is_some_and(|answer| is_foreign_mob_target(answer, ABSENT_MOB, None));
        if !refused {
            wrong.push(format!("{method}: {answer:?}"));
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(wrong.is_empty(), "not refused:\n{}", wrong.join("\n"));
}

/// The two event stream routes refuse a `mob_id` the host does not have
/// before they open a stream, and say which mob was asked for and no
/// other.
#[tokio::test]
async fn the_event_stream_routes_refuse_a_mob_the_host_does_not_have() {
    let runtime = runtime().await;
    let app = open_app(&runtime);
    let mut wrong = Vec::new();
    for uri in [
        format!("/mob/events?mob_id={ABSENT_MOB}"),
        format!("/agents/worker-1/events?mob_id={ABSENT_MOB}"),
    ] {
        let (status, body) = get(&app, &uri).await;
        let refused = status == axum::http::StatusCode::NOT_FOUND
            && body.as_ref().is_some_and(|body| {
                body["error"] == json!("foreign_mob_target")
                    && body["requested_mob_id"] == json!(ABSENT_MOB)
                    && body.get("gateway_mob_id").is_none()
            });
        if !refused {
            wrong.push(format!("{uri}: {status} {body:?}"));
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(wrong.is_empty(), "not refused:\n{}", wrong.join("\n"));
}

/// The mob event stream of the default mob opens the same with no `mob_id`
/// and with the default mob's id.
#[tokio::test]
async fn the_mob_event_stream_takes_the_default_mobs_id_like_no_mob() {
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let app = open_app(&runtime);
    let (plain, _) = get(&app, "/mob/events").await;
    let (by_id, _) = get(&app, &format!("/mob/events?mob_id={default_mob}")).await;
    drop(app);
    runtime.shutdown().await;
    assert_eq!(plain, axum::http::StatusCode::OK);
    assert_eq!(by_id, plain);
}

/// The routers the runtime builds hand their doors the host's mob resolver:
/// a request that names another mob on the console RPC, on its multipart
/// route and on the two event stream routes is answered by the resolver,
/// and no door of the router falls back to the one mob its state holds.
#[tokio::test]
async fn the_runtimes_router_hands_every_door_the_mob_resolver() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let runtime = runtime().await;
    let app = open_app(&runtime);
    let params = params_naming("meta/list_members", Some(ABSENT_MOB), scratch.path());
    let refused = |answer: Option<Value>| {
        answer.is_some_and(|answer| is_foreign_mob_target(&answer, ABSENT_MOB, None))
    };
    assert!(refused(
        console(&app, "meta/list_members", params.clone()).await
    ));
    assert!(refused(
        console_multipart(&app, "meta/console/send", params).await
    ));
    for uri in [
        format!("/mob/events?mob_id={ABSENT_MOB}"),
        format!("/agents/worker-1/events?mob_id={ABSENT_MOB}"),
    ] {
        let (status, _) = get(&app, &uri).await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND, "{uri}");
    }
    drop(app);
    runtime.shutdown().await;
    assert!(
        !crate::rpc::mob_addressing::a_door_answered_without_its_resolver(),
        "a door of the runtime's own router answered without the host's mob resolver"
    );
}

/// A method that only needs to know that the named mob is served (the model
/// catalog, the storage doctor, access, decisions and the other host-scoped
/// methods) answers for a hosted mob that is known and not loaded WITHOUT
/// loading it, on the stdio dispatch and on the console RPC; the eleven
/// reads with a per-mob part (`meta/status`, `meta/capabilities`, the nine
/// catalog reads), named with the same mob, load it and are answered for
/// it.
///
/// Read at the mob set's inventory after every request: the mob is still
/// known and not loaded, or it is managed. The count at the store's own
/// open is Core's to make (its counter is not built into a host's tests);
/// the mob set's answer to "is this mob the host's" may open that one
/// store's creation record once, and loads nothing.
///
/// On the stdio dispatch every method of the host's list is asked, and
/// each is answered for the mob. On the console RPC the host's reads are
/// asked on an open console: each is answered and loads nothing, whatever
/// the door answers a caller it did not identify (a host method that
/// changes the console's own access state would change who the row's next
/// caller is, so those are not asked).
///
/// Not shown here: what the console answers a signed-in caller. The
/// console serves a method of the host for the launch mob alone, so such a
/// request that names another mob is answered the mob id's refusal,
/// whoever asks, and loads nothing
/// (`a_host_scoped_request_of_a_signed_in_caller_for_a_mob_that_is_not_loaded_loads_nothing`,
/// beside the console door's fixture of signed-in callers). That
/// `meta/mobs/list` and `meta/mobs/get` load nothing is a row for the
/// verbs of a host's mobs, where the two methods are.
#[tokio::test]
async fn a_host_scoped_request_that_names_a_dormant_mob_loads_nothing() {
    use super::mob_addressing_guard_tests::HOST_SCOPED_STDIO_METHODS;

    /// The console's host-scoped methods that read and change nothing.
    const HOST_SCOPED_CONSOLE_READS: &[&str] = &[
        "meta/access/get",
        "meta/access/status",
        "meta/console/attention",
        "meta/console/audit",
        "meta/console/whoami",
        "meta/cross_mob/directory",
        "meta/decisions/list",
        "meta/mob_announcements/subscriptions/list",
        "meta/peer_pubkey",
        "meta/schedule/tools",
        "meta/storage/doctor",
    ];
    /// The reads that answer for the host and carry a part of the mob a
    /// request names.
    const READS_WITH_A_PART_OF_THE_NAMED_MOB: &[&str] = &[
        "meta/status",
        "meta/capabilities",
        "meta/mobpacks/schema",
        "meta/mobpacks/catalogs",
        "meta/mobpacks/templates",
        "meta/mobpacks/list",
        "meta/mobpacks/get",
        "meta/mobpacks/apply_operation",
        "meta/tools/catalog",
        "meta/skills/catalog",
        "meta/agent_definitions/list",
    ];

    let scratch = tempfile::tempdir().expect("tempdir");
    let dormant = DormantMob::of_a_stored_host("host-scoped").await;
    let host = &dormant.host;
    let launch_mob = host.mob_id();
    let named = dormant.mob_id.as_str();
    let mut wrong = Vec::new();

    for method in HOST_SCOPED_STDIO_METHODS {
        let answer = stdio(
            host,
            method,
            params_naming(method, Some(named), scratch.path()),
        )
        .await;
        match &answer {
            None => wrong.push(format!("stdio, {method}: no answer in time")),
            Some(answer) if is_foreign_mob_target(answer, named, Some(&launch_mob)) => {
                wrong.push(format!(
                    "stdio, {method}: refused as a mob the host does not have: {answer}"
                ));
            }
            Some(_) => {}
        }
        if !dormant.is_not_loaded().await {
            wrong.push(format!("stdio, {method}: the mob was loaded"));
            dormant.unload().await;
        }
    }

    let app = open_app(host);
    for method in HOST_SCOPED_CONSOLE_READS {
        if !crate::rpc::mob_addressing::console_method_is_host_scoped(method) {
            wrong.push(format!("console RPC, {method}: not a method of the host"));
            continue;
        }
        let answer = console(
            &app,
            method,
            params_naming(method, Some(named), scratch.path()),
        )
        .await;
        if answer.is_none() {
            wrong.push(format!("console RPC, {method}: no answer in time"));
        }
        if !dormant.is_not_loaded().await {
            wrong.push(format!("console RPC, {method}: the mob was loaded"));
            dormant.unload().await;
        }
    }
    drop(app);

    for method in READS_WITH_A_PART_OF_THE_NAMED_MOB {
        if !dormant.is_not_loaded().await {
            dormant.unload().await;
        }
        let answer = stdio(
            host,
            method,
            params_naming(method, Some(named), scratch.path()),
        )
        .await;
        match &answer {
            None => wrong.push(format!("stdio, {method}: no answer in time")),
            Some(answer) if is_foreign_mob_target(answer, named, Some(&launch_mob)) => {
                wrong.push(format!(
                    "stdio, {method}: refused as a mob the host does not have: {answer}"
                ));
            }
            Some(_) => {}
        }
        if dormant.is_not_loaded().await {
            wrong.push(format!(
                "stdio, {method}: a read with a part of the named mob did not load it"
            ));
        }
    }
    for method in ["meta/status", "meta/capabilities"] {
        let answer = stdio(host, method, json!({"mob_id": named})).await;
        if answer
            .as_ref()
            .is_none_or(|answer| answer.get("error").is_some())
        {
            wrong.push(format!(
                "stdio, {method}: not answered for the mob: {answer:?}"
            ));
        }
    }

    host.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A host over a state directory with a second mob the mob set knows and
/// has not loaded, for the rows that ask what a request does to such a mob.
struct DormantMob {
    /// Kept for the state directory it owns.
    _stored: crate::unified_runtime::StoredHost,
    host: UnifiedRuntime,
    mob_id: meerkat_mob::MobId,
}

impl DormantMob {
    /// A launch over a state directory, a second mob of the host created
    /// through the launch's mob set, and that mob unloaded.
    async fn of_a_stored_host(tag: &str) -> Self {
        let stored = crate::unified_runtime::StoredHost::new();
        let host = stored.launch().await;
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"addressing-rows-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
            uuid::Uuid::new_v4().simple()
        ))
        .expect("mob definition");
        let mob_id = Box::pin(
            host.mob_runtime()
                .agent_mob_mcp_state()
                .expect("the launch's mob set")
                .mob_create_definition(definition),
        )
        .await
        .expect("the mob set creates a second mob of the host");
        let dormant = Self {
            _stored: stored,
            host,
            mob_id,
        };
        dormant.unload().await;
        dormant
    }

    /// Whether the mob set knows the mob and has not loaded it.
    async fn is_not_loaded(&self) -> bool {
        let known = crate::unified_runtime::known(&self.host).await;
        known.not_loaded.contains(&self.mob_id) && !known.loaded.contains(&self.mob_id)
    }

    /// Have the mob set unload the mob, asked until the mob is idle for
    /// it: a mob a request just loaded is busy for a moment.
    async fn unload(&self) {
        let set = self
            .host
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the launch's mob set");
        for _ in 0..200 {
            set.unload_idle_mobs_for_tests().await;
            if self.is_not_loaded().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the mob set did not unload the idle mob {}", self.mob_id);
    }
}

/// The text of a dispatcher: `source` from `start` up to `end`.
fn dispatch_body<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let from = source.find(start).expect("the dispatcher's start");
    let body = &source[from..];
    &body[..body.find(end).expect("the dispatcher's end")]
}

/// The stdio dispatch, as [`served_names`] reads it.
fn stdio_dispatch() -> &'static str {
    dispatch_body(
        include_str!("../rpc.rs"),
        "async fn handle_unified_rpc_json_inner(",
        "\nfn build_models_catalog_result(",
    )
}

/// The console RPC's dispatch, as [`served_names`] reads it.
fn console_dispatch() -> &'static str {
    dispatch_body(
        include_str!("../http_console.rs"),
        "async fn handle_console_runtime_rpc_with_visibility(",
        "\nasync fn run_reaches_hidden_member(",
    )
}

/// The family registries a dispatcher serves through: each with the tests
/// of the family a dispatcher's text calls when it serves it. A dispatcher
/// that calls one of them serves every name of the registry.
fn family_registries() -> [(&'static [&'static str], &'static [&'static str]); 10] {
    [
        (
            crate::rpc::identity_methods::IDENTITY_FAMILY_METHODS,
            &["is_identity_family_method("],
        ),
        (
            crate::rpc::mob_hosting::MOB_HOSTING_METHODS,
            &["is_mob_hosting_method(", "host_serves_mob_hosting("],
        ),
        (
            crate::rpc::tool_grant_methods::TOOL_GRANT_METHODS,
            &["is_tool_grant_method("],
        ),
        (
            crate::rpc::mob_methods::MEMBER_DECLARATION_METHODS,
            &["is_member_declaration_method("],
        ),
        (
            crate::rpc::realm_mcp_methods::REALM_MCP_METHODS,
            &["is_realm_mcp_method("],
        ),
        (
            crate::rpc::MOBPACK_AUTHORING_METHODS,
            &["MOBPACK_AUTHORING_METHODS.contains("],
        ),
        (
            crate::mob_announcements::rpc::ANNOUNCEMENT_METHODS,
            &["is_announcement_method("],
        ),
        (
            crate::rpc::workgraph_methods::WORKGRAPH_READ_METHODS,
            &["is_workgraph_method(", "is_workgraph_read_method("],
        ),
        (
            crate::rpc::workgraph_methods::WORKGRAPH_MUTATE_METHODS,
            &["is_workgraph_method(", "is_workgraph_mutating_method("],
        ),
        (
            crate::rpc::workgraph_methods::WORKGRAPH_CONSOLE_MUTATE_METHODS,
            &["is_workgraph_method(", "is_workgraph_mutating_method("],
        ),
    ]
}

/// Every name a dispatcher serves: by an arm of its own, and through each
/// family registry whose test its text calls.
fn served_by(dispatch: &str) -> BTreeSet<String> {
    let mut served = served_names(dispatch);
    for (registry, tests) in family_registries() {
        if tests.iter().any(|test| dispatch.contains(test)) {
            served.extend(registry.iter().map(|name| (*name).to_string()));
        }
    }
    served
}

/// The method name literals that head a line of a dispatcher's text: its
/// arms and the names it lists.
fn served_names(body: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in body.lines() {
        let line = line.trim_start();
        let line = line.strip_prefix("| ").unwrap_or(line);
        let Some(rest) = line.strip_prefix('"') else {
            continue;
        };
        let Some(name) = rest.split('"').next() else {
            continue;
        };
        let method = (name.starts_with("meta/") || name.starts_with("mob/"))
            && name.len() > "meta/".len()
            && !name.ends_with('/');
        if method {
            names.insert(name.to_string());
        }
    }
    names
}

/// Every method a dispatcher serves, by an arm or through a family registry,
/// has a row in the table, so the rows above cover it, and the row says so
/// for each door that serves it, so they ask it on that door.
#[test]
fn every_method_a_dispatcher_serves_is_in_the_addressing_table() {
    let table: BTreeSet<&str> = ADDRESSED_METHODS.iter().map(|row| row.0).collect();
    assert_eq!(
        table.len(),
        ADDRESSED_METHODS.len(),
        "a method is listed twice"
    );
    let mut served = served_names(stdio_dispatch());
    served.extend(served_names(console_dispatch()));
    served.extend(
        family_registries()
            .iter()
            .flat_map(|(registry, _)| registry.iter().map(|name| (*name).to_string())),
    );
    // The launch and its end are not requests to a mob.
    let not_addressed = ["meta/init", "meta/shutdown"];
    let missing: Vec<&String> = served
        .iter()
        .filter(|name| !table.contains(name.as_str()) && !not_addressed.contains(&name.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "served and not in the addressing table: {missing:?}"
    );
    // The door columns: a name a dispatcher serves, by an arm or through a
    // family registry it reads, has that dispatcher's column true. A
    // registry both dispatchers read therefore needs both. What this does
    // not see: a column that is true for a method the dispatcher's text
    // does not name (one served by another door of the same surface, or
    // through a constant); such a row is asked on that surface by the rows
    // above and answers there.
    let on_stdio = served_by(stdio_dispatch());
    let on_the_console = served_by(console_dispatch());
    let mut wrong_door = Vec::new();
    for (method, stdio, console, _) in ADDRESSED_METHODS {
        if on_stdio.contains(*method) && !*stdio {
            wrong_door.push(format!(
                "{method}: the stdio dispatch serves it and the table says it does not"
            ));
        }
        if on_the_console.contains(*method) && !*console {
            wrong_door.push(format!(
                "{method}: the console RPC serves it and the table says it does not"
            ));
        }
    }
    assert!(
        wrong_door.is_empty(),
        "{} door columns of the addressing table are wrong:\n{}",
        wrong_door.len(),
        wrong_door.join("\n")
    );
}

/// A sink for the decision door's notices that takes every line.
struct TakesEveryNotice;

#[async_trait::async_trait]
impl crate::durable_decisions::DecisionNoticeSink for TakesEveryNotice {
    async fn deliver(&self, _line: String) -> bool {
        true
    }
}

/// One request on the stdio decision door. `None` when the door did not
/// take the line.
async fn decision_door(
    connection: &crate::durable_decisions::StdioDecisionConnection,
    runtime: &UnifiedRuntime,
    method: &str,
    params: Value,
) -> Option<Value> {
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let answer = connection.handle_line(runtime, &request).await?;
    serde_json::from_str(&answer).ok()
}

/// The decision methods answer on a door of their own, before the stdio
/// dispatch. That door takes the mob a request names like every other
/// door: an id the host does not have is refused, the default mob's id is
/// the same request as no mob (it is taken off before the method's strict
/// parameters are read), and an id that is not text is invalid.
#[tokio::test]
async fn the_stdio_decision_door_takes_the_mob_a_request_names() {
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let connection =
        crate::durable_decisions::StdioDecisionConnection::new(None, Arc::new(TakesEveryNotice));
    for method in [
        "meta/decisions/list",
        "meta/decisions/get",
        "meta/decisions/rendering",
        "meta/decisions/present",
        "meta/decisions/withdraw",
        "meta/decisions/decide",
    ] {
        let refused =
            decision_door(&connection, &runtime, method, json!({"mob_id": ABSENT_MOB})).await;
        assert!(
            refused.as_ref().is_some_and(|answer| is_foreign_mob_target(
                answer,
                ABSENT_MOB,
                Some(default_mob.as_str())
            )),
            "{method}: {refused:?}"
        );
        let unnamed = decision_door(&connection, &runtime, method, json!({})).await;
        let named = decision_door(
            &connection,
            &runtime,
            method,
            json!({"mob_id": default_mob.as_str()}),
        )
        .await;
        assert!(unnamed.is_some(), "{method}: the door takes the method");
        assert_eq!(named, unnamed, "{method}: the default mob's id is no mob");
        let not_text = decision_door(&connection, &runtime, method, json!({"mob_id": 7})).await;
        assert_eq!(
            not_text.as_ref().map(|answer| &answer["error"]["message"]),
            Some(&json!("Invalid params: mob_id must be a string")),
            "{method}: {not_text:?}"
        );
    }
    runtime.shutdown().await;
}

/// The console methods of the table that serve one mob: the console's door
/// resolves the request's mob for each of them. Sorted, one name per line.
/// The other console methods are the host's
/// (`mob_addressing::HOST_SCOPED_CONSOLE_METHODS`) or are served from the
/// launch mob's own wiring (the list the console's file keeps).
const CONSOLE_METHODS_THAT_SERVE_ONE_MOB: &[&str] = &[
    "meta/agent_memory/forget",
    "meta/agent_memory/manifest",
    "meta/agent_memory/recall",
    "meta/agent_memory/remember",
    "meta/agent_memory/update",
    "meta/attach_existing_session",
    "meta/cancel_flow",
    "meta/capabilities",
    "meta/collect_completed",
    "meta/console/inspect_identity",
    "meta/console/list_identities",
    "meta/console/query_timeline",
    "meta/console/recover_send",
    "meta/console/send",
    "meta/cross_mob/peer_info",
    "meta/cross_mob/unwire_local",
    "meta/cross_mob/wire_local",
    "meta/delete_identity",
    "meta/delivery/history",
    "meta/ensure_member",
    "meta/find_members",
    "meta/flow_status",
    "meta/force_cancel_member",
    "meta/fork_helper",
    "meta/fork_member",
    "meta/gating/audit",
    "meta/gating/decide",
    "meta/gating/pending",
    "meta/gating/questions",
    "meta/get_member",
    "meta/identity/adopt",
    "meta/identity/archive",
    "meta/identity/changes_hold",
    "meta/identity/create",
    "meta/identity/directory",
    "meta/identity/get",
    "meta/identity/list",
    "meta/identity/operation",
    "meta/identity/plan",
    "meta/identity/remove",
    "meta/identity/resolved_tools",
    "meta/identity/routing_status",
    "meta/identity/settle_preparation",
    "meta/identity/unarchive",
    "meta/identity/update",
    "meta/identity/warm",
    "meta/identity/withdraw",
    "meta/inspect_identity",
    "meta/list_flows",
    "meta/list_members",
    "meta/list_runs",
    "meta/member_health",
    "meta/member_status",
    "meta/memory/panel/audit_verdicts",
    "meta/memory/panel/dream_runs",
    "meta/memory/panel/dreams",
    "meta/memory/panel/harvests",
    "meta/memory/panel/injections",
    "meta/memory/panel/overview",
    "meta/memory/panel/proposals",
    "meta/memory/panel/quarantine",
    "meta/memory/panel/record",
    "meta/memory/panel/records",
    "meta/memory/quarantine/decide",
    "meta/mob_announcements/announce",
    "meta/mob_announcements/audit/list",
    "meta/mob_announcements/get",
    "meta/mob_announcements/sources/advance",
    "meta/mob_announcements/sources/end",
    "meta/mob_announcements/sources/get",
    "meta/mob_announcements/sources/list",
    "meta/mob_announcements/sources/pause",
    "meta/mob_announcements/sources/put",
    "meta/mob_announcements/sources/record_failure",
    "meta/mob_announcements/sources/resume",
    "meta/mob_events/query",
    "meta/mob_events/subscribe",
    "meta/mob_labels/delete",
    "meta/mob_labels/get",
    "meta/mob_labels/set",
    "meta/reconcile_edges",
    "meta/reload_member",
    "meta/reset",
    "meta/reset_all",
    "meta/respawn",
    "meta/respawn_member",
    "meta/retire",
    "meta/retire_member",
    "meta/routing/routes/list",
    "meta/run_flow",
    "meta/run_labels/delete",
    "meta/run_labels/get",
    "meta/run_labels/set",
    "meta/schedule/create",
    "meta/schedule/delete",
    "meta/schedule/list",
    "meta/schedule/pause",
    "meta/schedule/resume",
    "meta/schedule/update",
    "meta/send_status",
    "meta/spawn_helper",
    "meta/status",
    "meta/status_identity",
    "meta/stop_member_run",
    "meta/tool_grants/audit",
    "meta/tool_grants/grant",
    "meta/tool_grants/list",
    "meta/tool_grants/revoke",
    "meta/topology/apply",
    "meta/topology/audit/query",
    "meta/topology/operation/get",
    "meta/topology/plan",
    "meta/topology/query",
    "meta/wait_ready",
    "meta/workgraph/attention/break_glass_reassign",
    "meta/workgraph/attention/list",
    "meta/workgraph/attention/pause",
    "meta/workgraph/attention/prune",
    "meta/workgraph/attention/reassign",
    "meta/workgraph/attention/resume",
    "meta/workgraph/block",
    "meta/workgraph/claim",
    "meta/workgraph/close",
    "meta/workgraph/create",
    "meta/workgraph/events",
    "meta/workgraph/evidence/add",
    "meta/workgraph/facts",
    "meta/workgraph/get",
    "meta/workgraph/goal/confirm",
    "meta/workgraph/goal/create",
    "meta/workgraph/goal/request_close",
    "meta/workgraph/goal/status",
    "meta/workgraph/link",
    "meta/workgraph/list",
    "meta/workgraph/policy/escalate",
    "meta/workgraph/ready",
    "meta/workgraph/release",
    "meta/workgraph/snapshot",
    "meta/workgraph/update",
    "mob/adopt_member_identity_declaration",
    "mob/apply_member_tool_declaration",
    "mob/member_tool_declaration",
];

/// The console methods served from the launch mob's own wiring, as the
/// console's file lists them: the string literals of its constant
/// `CONSOLE_METHODS_OF_THE_LAUNCH_MOBS_WIRING`, read from its source. Empty
/// when the file names no such list.
fn console_methods_of_the_launch_mobs_wiring() -> Vec<&'static str> {
    let source = include_str!("../http_console.rs");
    let Some(from) = source.find("const CONSOLE_METHODS_OF_THE_LAUNCH_MOBS_WIRING") else {
        return Vec::new();
    };
    let value = &source[from..];
    let value = &value[..value.find("];").unwrap_or(value.len())];
    value.split('"').skip(1).step_by(2).collect()
}

/// Which side a console method is on is a decision, not a default: every
/// console method of the table is on exactly one of three lists, and no
/// list names anything else. The host's methods read no value of any mob.
/// The methods that serve one mob are answered for the mob the door
/// resolves for the request. The methods served from the launch mob's own
/// wiring are answered before the door resolves a mob, from what its router
/// was built with, so they are the launch mob's alone: the console's file
/// lists them, and its door refuses them for any other mob. A method the
/// console serves that is on no list fails here with its name.
#[test]
fn every_console_method_is_the_hosts_or_serves_one_mob() {
    let of_the_host = crate::rpc::mob_addressing::HOST_SCOPED_CONSOLE_METHODS;
    let of_the_launch_mobs_wiring = console_methods_of_the_launch_mobs_wiring();
    assert!(
        !of_the_launch_mobs_wiring.is_empty(),
        "the console's file names no list of the methods served from the launch mob's wiring"
    );
    assert!(
        of_the_host.windows(2).all(|pair| pair[0] < pair[1]),
        "the host's console methods: sorted, each name once"
    );
    assert!(
        CONSOLE_METHODS_THAT_SERVE_ONE_MOB
            .windows(2)
            .all(|pair| pair[0] < pair[1]),
        "the console methods that serve one mob: sorted, each name once"
    );
    assert!(
        of_the_launch_mobs_wiring
            .windows(2)
            .all(|pair| pair[0] < pair[1]),
        "the console methods of the launch mob's wiring: sorted, each name once"
    );
    let mut found = Vec::new();
    for (method, _, console, _) in ADDRESSED_METHODS {
        if !*console {
            continue;
        }
        let is_the_hosts = of_the_host.contains(method);
        let lists = [
            is_the_hosts,
            CONSOLE_METHODS_THAT_SERVE_ONE_MOB.contains(method),
            of_the_launch_mobs_wiring.contains(method),
        ];
        match lists.iter().filter(|listed| **listed).count() {
            0 => found.push(format!("{method}: on no list")),
            1 => {}
            _ => found.push(format!("{method}: on more than one list")),
        }
        assert_eq!(
            crate::rpc::mob_addressing::console_method_is_host_scoped(method),
            is_the_hosts,
            "{method}"
        );
    }
    for method in of_the_host
        .iter()
        .chain(CONSOLE_METHODS_THAT_SERVE_ONE_MOB)
        .chain(&of_the_launch_mobs_wiring)
    {
        let served = ADDRESSED_METHODS
            .iter()
            .any(|(listed, _, console, _)| listed == method && *console);
        if !served {
            found.push(format!("{method}: not a console method of the table"));
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// `GET /meta/mob_events/stream` names the mob whose ledger it reads with
/// its `mob_id`: an id the host does not have is refused before a stream
/// opens, with the id the request named and no other mob; no id and the
/// default mob's id open the same stream.
#[tokio::test]
async fn the_structural_event_stream_takes_the_mob_a_request_names() {
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let app = open_app(&runtime);
    let path = crate::unified_runtime::mob_events::MOB_EVENTS_STREAM_PATH;
    let (status, body) = get(&app, &format!("{path}?mob_id={ABSENT_MOB}")).await;
    let (plain, _) = get(&app, path).await;
    let (by_id, _) = get(&app, &format!("{path}?mob_id={default_mob}")).await;
    drop(app);
    runtime.shutdown().await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND, "{body:?}");
    assert!(
        body.as_ref().is_some_and(|body| {
            body["error"] == json!("foreign_mob_target")
                && body["requested_mob_id"] == json!(ABSENT_MOB)
                && body.get("gateway_mob_id").is_none()
        }),
        "{body:?}"
    );
    assert_eq!(plain, axum::http::StatusCode::OK);
    assert_eq!(by_id, plain);
}

/// `GET /meta/workgraph/facts/stream` takes `mob_id` like the event stream
/// routes: under the host's resolver an id the host does not have is
/// refused before a stream opens, and no id and the default mob's id open
/// the stream. The route holds no mob of its own, so without a resolver it
/// refuses every id a request names and serves a request that names none.
#[tokio::test]
async fn the_workgraph_fact_stream_takes_the_mob_a_request_names() {
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let path = crate::http_sse::WORKGRAPH_FACTS_STREAM_PATH;
    let routes = || {
        crate::http_sse::workgraph_facts_sse_router(
            crate::workgraph_events::WorkGraphFactHub::new(),
            None,
            None,
        )
    };
    let with_resolver = routes().layer(axum::Extension(runtime.mob_resolver()));
    let (status, body) = get(&with_resolver, &format!("{path}?mob_id={ABSENT_MOB}")).await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND, "{body:?}");
    assert!(
        body.as_ref().is_some_and(|body| {
            body["error"] == json!("foreign_mob_target")
                && body["requested_mob_id"] == json!(ABSENT_MOB)
                && body.get("gateway_mob_id").is_none()
        }),
        "{body:?}"
    );
    let (plain, _) = get(&with_resolver, path).await;
    let (by_id, _) = get(&with_resolver, &format!("{path}?mob_id={default_mob}")).await;
    assert_eq!(plain, axum::http::StatusCode::OK);
    assert_eq!(by_id, plain);

    let without_resolver = routes();
    let (plain, _) = get(&without_resolver, path).await;
    let (named, body) = get(&without_resolver, &format!("{path}?mob_id={default_mob}")).await;
    assert_eq!(plain, axum::http::StatusCode::OK);
    assert_eq!(named, axum::http::StatusCode::NOT_FOUND, "{body:?}");
    runtime.shutdown().await;
}

/// The request shapes of the methods whose own parameter `mob_id` is, each
/// naming `mob_id`: by name and by position, as the parameter types of
/// those methods read both.
fn requests_of_the_methods_that_keep_their_mob(
    mob_id: &str,
) -> Vec<(&'static str, &'static str, Value)> {
    let session = json!({
        "session_id": uuid::Uuid::new_v4().to_string(),
        "lineage_id": "session:addressing-rows",
        "lineage_generation": 0,
        "authority_policy": "require_existing"
    });
    let member = json!({
        "profile_name": "worker",
        "runtime_mode": "turn_driven",
        "execution": { "execution": "controlling_session" }
    });
    let declaration = json!({
        "category_overrides": {},
        "callback_tools": { "kind": "inherit" },
        "execution": { "kind": "inherit" },
        "application_policy": { "kind": "unmanaged" }
    });
    let convergence = json!({ "kind": "drain", "max_wait_ms": 5000 });
    let adopt = crate::rpc::mob_methods::ADOPT_MEMBER_IDENTITY_DECLARATION;
    let apply = crate::rpc::mob_methods::APPLY_MEMBER_TOOL_DECLARATION;
    let read = crate::rpc::mob_methods::MEMBER_TOOL_DECLARATION;
    vec![
        (
            read,
            "by name",
            json!({"mob_id": mob_id, "agent_identity": "worker-1"}),
        ),
        (read, "by position", json!([mob_id, "worker-1"])),
        (
            apply,
            "by name",
            json!({
                "mob_id": mob_id,
                "agent_identity": "worker-1",
                "request_id": "addressing-rows-apply",
                "expected_intent_revision": 1,
                "declaration": declaration,
                "convergence": convergence
            }),
        ),
        (
            apply,
            "by position",
            json!([
                mob_id,
                "worker-1",
                "addressing-rows-apply",
                1,
                declaration,
                convergence
            ]),
        ),
        (
            adopt,
            "by name",
            json!({
                "mob_id": mob_id,
                "agent_identity": "worker-1",
                "request_id": "addressing-rows-adopt",
                "precondition": "expected_absent",
                "declaration_scope": "addressing-rows",
                "declaration_revision": 1,
                "session": session,
                "member": member,
                "owned_wiring": [],
                "convergence": convergence
            }),
        ),
        (
            adopt,
            "by position",
            json!([
                mob_id,
                "worker-1",
                "addressing-rows-adopt",
                "expected_absent",
                "addressing-rows",
                1,
                session,
                member,
                "external_managed",
                [],
                convergence
            ]),
        ),
        (
            "meta/mob_events/query",
            "by name",
            json!({"mob_id": mob_id}),
        ),
        (
            "meta/mob_events/query",
            "by position",
            json!([null, null, null, null, mob_id]),
        ),
        (
            "meta/mob_events/subscribe",
            "by name",
            json!({"mob_id": mob_id}),
        ),
        (
            "meta/mob_events/subscribe",
            "by position",
            json!([null, null, null, null, mob_id]),
        ),
    ]
}

/// Each request shape above reads as its method's parameters with the mob
/// it names, so that the rows over them ask what they say they ask.
#[test]
fn every_request_shape_of_a_method_that_keeps_its_mob_names_the_mob() {
    use meerkat_contracts::wire;
    for (method, shape, params) in requests_of_the_methods_that_keep_their_mob(ABSENT_MOB) {
        let named = if method == crate::rpc::mob_methods::ADOPT_MEMBER_IDENTITY_DECLARATION {
            serde_json::from_value::<wire::MobAdoptMemberIdentityDeclarationParams>(params)
                .map(|params| params.mob_id)
        } else if method == crate::rpc::mob_methods::APPLY_MEMBER_TOOL_DECLARATION {
            serde_json::from_value::<wire::MobApplyMemberToolDeclarationParams>(params)
                .map(|params| params.mob_id)
        } else if method == crate::rpc::mob_methods::MEMBER_TOOL_DECLARATION {
            serde_json::from_value::<wire::MobMemberToolDeclarationParams>(params)
                .map(|params| params.mob_id)
        } else {
            serde_json::from_value::<crate::unified_runtime::EventQuery>(params)
                .map(|query| query.mob_id.unwrap_or_default())
        };
        assert_eq!(
            named.as_deref().map_err(ToString::to_string),
            Ok(ABSENT_MOB),
            "{method} {shape}"
        );
    }
}

/// A door that has callers answers a mob id it does not serve in one form:
/// on the console RPC a member declaration request that names, by name, a
/// mob the host does not have is refused with the id the request named and
/// no other mob. A request whose params are given by position is refused
/// there as malformed params, before the door reads a mob: a request names
/// its mob by a member of its params. No answer of the console RPC or of
/// its multipart route to a request of a method that keeps its mob names a
/// mob the request did not name. The stdio dispatch, whose one caller is
/// the host, reads both shapes and keeps answering with the host's default
/// mob beside the id.
#[tokio::test]
async fn a_door_with_callers_answers_a_mob_id_in_one_form_for_every_shape_of_the_params() {
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let app = open_app(&runtime);
    let mut wrong = Vec::new();
    let mut refusals: std::collections::BTreeMap<&str, Vec<Value>> =
        std::collections::BTreeMap::new();
    for (method, shape, params) in requests_of_the_methods_that_keep_their_mob(ABSENT_MOB) {
        let declaration = crate::rpc::mob_methods::is_member_declaration_method(method);
        let names_another_mob = |answer: &Option<Value>| {
            answer.as_ref().is_none_or(|answer| {
                let text = answer.to_string();
                text.contains(default_mob.as_str()) || text.contains("gateway_mob_id")
            })
        };
        let on_the_console = console(&app, method, params.clone()).await;
        if names_another_mob(&on_the_console) {
            wrong.push(format!("console RPC, {method} {shape}: {on_the_console:?}"));
        }
        if shape == "by position" {
            let malformed = on_the_console.as_ref().is_some_and(|answer| {
                answer["error"] == json!({"code": -32602, "message": "Invalid params"})
            });
            if !malformed {
                wrong.push(format!(
                    "console RPC, {method} {shape}: not refused as malformed params: \
                     {on_the_console:?}"
                ));
            }
        } else if declaration {
            let refused = on_the_console
                .as_ref()
                .is_some_and(|answer| is_foreign_mob_target(answer, ABSENT_MOB, None));
            if !refused {
                wrong.push(format!(
                    "console RPC, {method} {shape}: not the door's refusal: {on_the_console:?}"
                ));
            }
            refusals
                .entry(method)
                .or_default()
                .push(on_the_console.map_or(Value::Null, |answer| answer["error"].clone()));
        }
        let on_the_multipart_route = console_multipart(&app, method, params.clone()).await;
        if names_another_mob(&on_the_multipart_route) {
            wrong.push(format!(
                "console multipart, {method} {shape}: {on_the_multipart_route:?}"
            ));
        }
        if declaration && shape == "by position" {
            let to_the_host = stdio(&runtime, method, params).await;
            let host_form = to_the_host.as_ref().is_some_and(|answer| {
                is_foreign_mob_target(answer, ABSENT_MOB, Some(default_mob.as_str()))
            });
            if !host_form {
                wrong.push(format!(
                    "stdio, {method} {shape}: not the host's refusal: {to_the_host:?}"
                ));
            }
        }
    }
    drop(app);
    runtime.shutdown().await;
    for (method, answers) in refusals {
        if answers.windows(2).any(|pair| pair[0] != pair[1]) {
            wrong.push(format!(
                "console RPC, {method}: the shapes are answered differently: {answers:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The console's read routes, each with a path the fixture serves:
/// `/console/modules` is the second path of the handler of
/// `/console/experience`.
const CONSOLE_READ_ROUTES: &[&str] = &[
    "/console/experience",
    "/console/modules",
    "/console/identities",
    "/console/timeline",
    "/console/timeline/stream",
    "/console/identity/worker-1/stream",
    "/console/gating/stream",
];

/// Whether `status` and `body` are the answer of an HTTP route for a mob
/// id it does not serve to its caller: 404 with the id the request named
/// and no other mob.
fn refuses_the_mob_over_http(
    status: axum::http::StatusCode,
    body: Option<&Value>,
    requested: &str,
) -> bool {
    status == axum::http::StatusCode::NOT_FOUND
        && body.is_some_and(|body| {
            body == &json!({"error": "foreign_mob_target", "requested_mob_id": requested})
        })
}

/// The console's read routes take `mob_id` as a query parameter: an id the
/// host does not have is refused before anything is read, with the id the
/// request named and no other mob; no id and the default mob's id are the
/// same request.
#[tokio::test]
async fn the_console_read_routes_take_the_mob_a_request_names() {
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let app = open_app(&runtime);
    let mut wrong = Vec::new();
    for path in CONSOLE_READ_ROUTES {
        let (status, body) = get(&app, &format!("{path}?mob_id={ABSENT_MOB}")).await;
        if !refuses_the_mob_over_http(status, body.as_ref(), ABSENT_MOB) {
            wrong.push(format!("{path}: an absent mob: {status} {body:?}"));
        }
        let (plain, _) = get(&app, path).await;
        let (by_id, _) = get(&app, &format!("{path}?mob_id={default_mob}")).await;
        if plain != axum::http::StatusCode::OK || by_id != plain {
            wrong.push(format!(
                "{path}: no mob {plain}, the default mob's id {by_id}"
            ));
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The reference application of `runtime` as a console that requires a
/// sign-in.
fn sign_in_app(runtime: &UnifiedRuntime) -> axum::Router {
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": "console-test-signing",
        "email_allowlist": ["rows@example.test"],
    }))
    .expect("console sign-in configuration");
    runtime.build_reference_app_router(decisions)
}

/// No read route of the console reads the mob a request names before it
/// knows the caller: a request without a sign-in is answered as not signed
/// in whatever mob id it names, and the answer says nothing of the id.
#[tokio::test]
async fn no_console_read_route_reads_a_mob_id_before_it_knows_the_caller() {
    let runtime = runtime().await;
    let app = sign_in_app(&runtime);
    let mut wrong = Vec::new();
    for path in CONSOLE_READ_ROUTES {
        for uri in [path.to_string(), format!("{path}?mob_id={ABSENT_MOB}")] {
            let (status, body) = get(&app, &uri).await;
            let names_the_id = body
                .as_ref()
                .is_some_and(|body| body.to_string().contains(ABSENT_MOB));
            if status != axum::http::StatusCode::UNAUTHORIZED || names_the_id {
                wrong.push(format!("{uri}: {status} {body:?}"));
            }
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// One POST of a JSON body on `uri`: the status and, when the answer is
/// JSON, the body.
async fn post(
    app: &axum::Router,
    uri: &str,
    body: &Value,
) -> (axum::http::StatusCode, Option<Value>) {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("the answer");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("the answer's body");
    (status, serde_json::from_slice(&body).ok())
}

/// `body` with the member `mob_id` set to `mob_id`.
fn naming(body: &Value, mob_id: Value) -> Value {
    let mut body = body.clone();
    body["mob_id"] = mob_id;
    body
}

/// The console's two send routes, each with a body that names no mob: a
/// send to a member the fixture does not have, and the recovery of it.
fn console_send_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "/console/send",
            json!({
                "identity": "worker-1",
                "content": "hello",
                "origin": "console",
                "idempotency_key": "addressing-rows-send"
            }),
        ),
        (
            "/console/send/recover",
            json!({
                "identity": "worker-1",
                "origin": "console",
                "idempotency_key": "addressing-rows-send"
            }),
        ),
    ]
}

/// The console's send routes take `mob_id` as a member of the body: an id
/// the host does not have is refused before anything is sent or settled,
/// with the id the request named and no other mob; no id, a `null` id and
/// the default mob's id are the same request; an id that is no string is
/// refused as a request the route cannot read.
#[tokio::test]
async fn the_console_send_routes_take_the_mob_a_request_names() {
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let app = open_app(&runtime);
    let mut wrong = Vec::new();
    for (uri, body) in console_send_requests() {
        let (status, answer) = post(&app, uri, &naming(&body, json!(ABSENT_MOB))).await;
        if !refuses_the_mob_over_http(status, answer.as_ref(), ABSENT_MOB) {
            wrong.push(format!("{uri}: an absent mob: {status} {answer:?}"));
        }
        let plain = post(&app, uri, &body).await;
        if refuses_the_mob_over_http(plain.0, plain.1.as_ref(), ABSENT_MOB)
            || plain.0 == axum::http::StatusCode::UNPROCESSABLE_ENTITY
        {
            wrong.push(format!("{uri}: no mob: {plain:?}"));
        }
        let kind = |answer: &(axum::http::StatusCode, Option<Value>)| {
            (
                answer.0,
                answer.1.as_ref().map(|body| body["error"].clone()),
            )
        };
        for (what, mob_id) in [
            ("the default mob's id", json!(default_mob.as_str())),
            ("a null id", Value::Null),
        ] {
            let named = post(&app, uri, &naming(&body, mob_id)).await;
            if kind(&named) != kind(&plain) {
                wrong.push(format!("{uri}: {what}: {named:?}, no mob: {plain:?}"));
            }
        }
        let (status, answer) = post(&app, uri, &naming(&body, json!(7))).await;
        let unreadable = status == axum::http::StatusCode::BAD_REQUEST
            && answer.as_ref().is_some_and(|answer| {
                answer["error"] == json!("invalid_request")
                    && answer["message"] == json!("mob_id must be a string")
            });
        if !unreadable {
            wrong.push(format!(
                "{uri}: an id that is no string: {status} {answer:?}"
            ));
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Neither send route reads the mob a request names before it knows the
/// caller: a request without a sign-in is answered as not signed in
/// whatever its `mob_id` is, and the answer says nothing of the id.
#[tokio::test]
async fn no_console_send_route_reads_a_mob_id_before_it_knows_the_caller() {
    let runtime = runtime().await;
    let app = sign_in_app(&runtime);
    let mut wrong = Vec::new();
    for (uri, body) in console_send_requests() {
        for body in [
            body.clone(),
            naming(&body, json!(ABSENT_MOB)),
            naming(&body, json!(7)),
        ] {
            let (status, answer) = post(&app, uri, &body).await;
            let names_the_id = answer
                .as_ref()
                .is_some_and(|answer| answer.to_string().contains(ABSENT_MOB));
            if status != axum::http::StatusCode::UNAUTHORIZED || names_the_id {
                wrong.push(format!("{uri} {body}: {status} {answer:?}"));
            }
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The console's three tool application routes, each with a body that
/// names no mob, for a member the fixture does not have.
fn console_app_requests() -> Vec<(&'static str, Value)> {
    let locator = json!({
        "identity": "worker-1",
        "sessionId": uuid::Uuid::new_v4().to_string(),
        "toolCallId": "addressing-rows-call",
    });
    let mut read = locator.clone();
    read["uri"] = json!("ui://addressing-rows/app");
    let mut call = locator.clone();
    call["name"] = json!("addressing-rows-tool");
    call["arguments"] = json!({});
    vec![
        ("/console/mcp-apps/resolve", locator),
        ("/console/mcp-apps/read-resource", read),
        ("/console/mcp-apps/call-tool", call),
    ]
}

/// The console's tool application routes take `mob_id` as a member of the
/// body: an id the host does not have is refused before anything of a
/// member is read, with the id the request named and no other mob; no id,
/// a `null` id and the default mob's id are the same request; an id that
/// is no string is refused as a request the route cannot read.
#[tokio::test]
async fn the_console_app_routes_take_the_mob_a_request_names() {
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let app = open_app(&runtime);
    let mut wrong = Vec::new();
    for (uri, body) in console_app_requests() {
        let (status, answer) = post(&app, uri, &naming(&body, json!(ABSENT_MOB))).await;
        if !refuses_the_mob_over_http(status, answer.as_ref(), ABSENT_MOB) {
            wrong.push(format!("{uri}: an absent mob: {status} {answer:?}"));
        }
        let plain = post(&app, uri, &body).await;
        if plain.0 != axum::http::StatusCode::FORBIDDEN {
            wrong.push(format!("{uri}: no mob: {plain:?}"));
        }
        for (what, mob_id) in [
            ("the default mob's id", json!(default_mob.as_str())),
            ("a null id", Value::Null),
        ] {
            let named = post(&app, uri, &naming(&body, mob_id)).await;
            if named != plain {
                wrong.push(format!("{uri}: {what}: {named:?}, no mob: {plain:?}"));
            }
        }
        let (status, answer) = post(&app, uri, &naming(&body, json!(7))).await;
        let unreadable = status == axum::http::StatusCode::BAD_REQUEST
            && answer.as_ref().is_some_and(|answer| {
                answer["error"] == json!("invalid_request")
                    && answer["message"] == json!("mob_id must be a string")
            });
        if !unreadable {
            wrong.push(format!(
                "{uri}: an id that is no string: {status} {answer:?}"
            ));
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// No tool application route reads the mob a request names before it
/// knows the caller: a request without a sign-in is answered as not signed
/// in whatever its `mob_id` is, and the answer says nothing of the id.
#[tokio::test]
async fn no_console_app_route_reads_a_mob_id_before_it_knows_the_caller() {
    let runtime = runtime().await;
    let app = sign_in_app(&runtime);
    let mut wrong = Vec::new();
    for (uri, body) in console_app_requests() {
        for body in [
            body.clone(),
            naming(&body, json!(ABSENT_MOB)),
            naming(&body, json!(7)),
        ] {
            let (status, answer) = post(&app, uri, &body).await;
            let names_the_id = answer
                .as_ref()
                .is_some_and(|answer| answer.to_string().contains(ABSENT_MOB));
            if status != axum::http::StatusCode::UNAUTHORIZED || names_the_id {
                wrong.push(format!("{uri} {body}: {status} {answer:?}"));
            }
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The delivery methods. Their arms do not resolve the request's mob: the
/// host's delivery service keeps each mob's deliveries apart and is handed
/// the id the dispatch judged, as the routing methods' service is
/// (`a_route_is_added_listed_and_deleted_for_the_mob_a_request_names`).
const DELIVERY_METHODS_KEYED_BY_THE_SERVED_MOB: &[&str] =
    &["meta/delivery/history", "meta/delivery/send"];
/// The gating methods are the host's: the gating state is one for all the
/// host's mobs.
const GATING_METHODS_OF_THE_HOST: &[&str] = &[
    "meta/gating/audit",
    "meta/gating/decide",
    "meta/gating/evaluate",
    "meta/gating/pending",
    "meta/gating/withdraw",
];
/// The agent memory methods: each works with the identity context of the
/// mob a request names.
const AGENT_MEMORY_METHODS: &[&str] = &[
    "meta/agent_memory/forget",
    "meta/agent_memory/manifest",
    "meta/agent_memory/recall",
    "meta/agent_memory/remember",
    "meta/agent_memory/update",
];
const RUNTIME_OPERATIONS_OF_A_RESOLVED_MOB: &[&str] = &["meta/rediscover"];

/// The methods of `methods` that a host serving a second mob does not
/// refuse for that mob with the refusal of a mob id (its kind and the two
/// ids), each with what it answered. The host serves the second mob: an
/// arm that resolves answers for it.
async fn not_refused_for_a_second_served_mob(methods: &[&'static str]) -> Vec<String> {
    let scratch = tempfile::tempdir().expect("tempdir");
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let mut wrong = Vec::new();
    for method in methods {
        let params = params_naming(method, Some(&second_mob), scratch.path());
        let answer = stdio(&host, method, params).await;
        let refused = answer
            .as_ref()
            .is_some_and(|answer| is_foreign_mob_target(answer, &second_mob, Some(&launch_mob)));
        if !refused {
            let answered = answer.map_or_else(
                || "no answer in time".to_string(),
                |answer| {
                    let text = answer.get("error").unwrap_or(&answer).to_string();
                    text.chars().take(120).collect()
                },
            );
            wrong.push(format!("{method}: {answered}"));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    wrong
}

/// A delivery method acts on the host's delivery service for the mob the
/// request names: for a second mob the host serves it is answered by that
/// service, not refused like a mob the host does not have, and not refused
/// as a method that serves the launch mob alone (the rows of the delivery
/// methods for a hosted mob hold what the service answers).
#[tokio::test]
async fn the_delivery_methods_are_answered_for_a_second_served_mob() {
    let answered =
        not_refused_for_a_second_served_mob(DELIVERY_METHODS_KEYED_BY_THE_SERVED_MOB).await;
    assert_eq!(
        answered.len(),
        DELIVERY_METHODS_KEYED_BY_THE_SERVED_MOB.len(),
        "{answered:?}"
    );
    assert!(
        !answered
            .iter()
            .any(|line| line.contains("no answer in time") || line.contains("hosted")),
        "{answered:?}"
    );
}

/// The gating state is the host's, one for all its mobs: the gating
/// methods answer it whatever mob of the host the request names. None of
/// the five is refused for a second served mob, and a request that names
/// the second mob reads the same pending requests and the same audit as a
/// request that names no mob, after a request for the second mob was
/// evaluated. An id the host does not have is still refused (the rows over
/// the addressing table ask that).
#[tokio::test]
async fn the_gating_methods_answer_the_hosts_gating_state_for_any_served_mob() {
    let answered = not_refused_for_a_second_served_mob(GATING_METHODS_OF_THE_HOST).await;
    assert_eq!(
        answered.len(),
        GATING_METHODS_OF_THE_HOST.len(),
        "the gating methods that answered for a second served mob: {answered:?}"
    );
    assert!(
        !answered
            .iter()
            .any(|line| line.contains("no answer in time")),
        "{answered:?}"
    );

    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let second_mob = second.mob_id();
    let evaluate = json!({
        "mob_id": second_mob.as_str(),
        "action": "publish",
        "actor_id": "an-actor",
        "risk_tier": "r3",
        "approval_recipient": "an-approver",
        "approval_channel": "notification",
    });
    let evaluated = stdio(&host, "meta/gating/evaluate", evaluate).await;
    let mut differing = Vec::new();
    for method in ["meta/gating/pending", "meta/gating/audit"] {
        let of_the_host = stdio(&host, method, json!({})).await;
        let named = stdio(&host, method, json!({"mob_id": second_mob.as_str()})).await;
        let answered = of_the_host
            .as_ref()
            .is_some_and(|answer| answer.get("result").is_some());
        if !answered || of_the_host != named {
            differing.push(format!(
                "{method}: {of_the_host:?} for no mob, {named:?} for the second mob"
            ));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(
        differing.is_empty(),
        "after {evaluated:?}:\n{}",
        differing.join("\n")
    );
}

/// The operation of the runtime that was on the list of the launch mob's
/// methods until a resolved mob offered it: `meta/rediscover` resolves the
/// request's mob and is answered for a second served mob, not refused like
/// a mob the host does not have.
#[tokio::test]
async fn the_runtime_operation_a_resolved_mob_offers_is_not_refused_for_a_second_served_mob() {
    let answered = not_refused_for_a_second_served_mob(RUNTIME_OPERATIONS_OF_A_RESOLVED_MOB).await;
    assert_eq!(
        answered.len(),
        RUNTIME_OPERATIONS_OF_A_RESOLVED_MOB.len(),
        "{answered:?}"
    );
    assert!(
        !answered
            .iter()
            .any(|line| line.contains("no answer in time")),
        "{answered:?}"
    );
}

/// The fixture of the rows above serves the second mob: a method whose arm
/// resolves the request's mob answers for it and does not refuse it, and an
/// id no mob has is still refused. So a refusal in those rows is the
/// method's, not the answer of a host that does not have the mob.
#[tokio::test]
async fn a_second_served_mob_is_served_by_an_arm_that_resolves() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let for_the_second = stdio(&host, "meta/list_flows", json!({"mob_id": second_mob})).await;
    let for_no_mob = stdio(&host, "meta/list_flows", json!({"mob_id": ABSENT_MOB})).await;
    host.shutdown().await;
    second.shutdown().await;
    assert!(
        for_the_second
            .as_ref()
            .is_some_and(|answer| answer.get("error").is_none()),
        "{for_the_second:?}"
    );
    assert!(
        for_no_mob
            .as_ref()
            .is_some_and(|answer| is_foreign_mob_target(answer, ABSENT_MOB, Some(&launch_mob))),
        "{for_no_mob:?}"
    );
}

/// The stdio methods whose arms take their identity context from the seam
/// step that never refuses for want of one: for a mob that is not the
/// launch mob it hands the context that mob's own entry holds, or none.
/// They are the two that report on the mob they answer for, with or without
/// an identity context, and the five that wire, unwire and send across
/// mobs, whose handlers take the resolved mob's own identity runtime when
/// they are handed none. Sorted, one name per line; the source row keeps it
/// equal to the arms.
pub(super) const STDIO_METHODS_NEVER_REFUSED_FOR_AN_IDENTITY_CONTEXT: &[&str] = &[
    "meta/capabilities",
    "meta/cross_mob/send",
    "meta/cross_mob/unwire",
    "meta/cross_mob/unwire_local",
    "meta/cross_mob/wire",
    "meta/cross_mob/wire_local",
    "meta/status",
];

/// The stdio methods whose arms work with the identity context of the mob
/// a request names: each takes it from the dispatch's seam step, which
/// hands an arm the launch's context for the launch mob and, for any other
/// mob, the context that mob's own entry holds. Sorted, one name per line;
/// the source rows keep it equal to the arms.
pub(super) const STDIO_METHODS_WORKING_WITH_AN_IDENTITY_CONTEXT: &[&str] = &[
    "meta/agent_memory/forget",
    "meta/agent_memory/manifest",
    "meta/agent_memory/recall",
    "meta/agent_memory/remember",
    "meta/agent_memory/update",
    "meta/attach_existing_session",
    "meta/bound_member_transcript",
    "meta/compact_member",
    "meta/completion_cursor",
    "meta/delete_identity",
    "meta/dispatch",
    "meta/ensure_member",
    "meta/find_members",
    "meta/force_cancel_member",
    "meta/fork_helper",
    "meta/get_member",
    "meta/identity/resolved_tools",
    "meta/identity/routing_status",
    "meta/identity/warm",
    "meta/inspect_identity",
    "meta/instruction_status",
    "meta/interact",
    "meta/list_members",
    "meta/member_activate_instruction",
    "meta/member_health",
    "meta/member_instruction_activations",
    "meta/member_status",
    "meta/prune_member_system_rows",
    "meta/reconcile_identity",
    "meta/recover_delivery",
    "meta/registered_spec",
    "meta/reload_member",
    "meta/reset",
    "meta/respawn",
    "meta/respawn_member",
    "meta/retire",
    "meta/retire_member",
    "meta/revoke_instruction",
    "meta/run_flow",
    "meta/send",
    "meta/send_message",
    "meta/send_status",
    "meta/set_instruction",
    "meta/spawn_helper",
    "meta/spawn_member",
    "meta/status_identity",
    "meta/status_identity_bootstrap",
    "meta/stop_member_run",
    "meta/subscribe",
    "meta/turn_result",
    "meta/wait_for_completion",
    "meta/wait_for_turn",
    "meta/wait_identity_bootstrap",
];

/// One method of each family whose arm works with an identity context and
/// is served for the mob a request names: the member declarations and the
/// tool grants. The family of a mob's long-term agents
/// (`IDENTITY_FAMILY_METHODS`) is each mob's own too, and has a row of its
/// own
/// (`a_request_of_the_long_term_agents_family_for_a_second_mob_is_answered_by_that_mobs_own_family`).
const FAMILY_METHODS_WORKING_WITH_AN_IDENTITY_CONTEXT: &[&str] = &[
    crate::rpc::mob_methods::MEMBER_TOOL_DECLARATION,
    "meta/tool_grants/list",
];

/// The two methods that report on the mob they answer for are served for a
/// second mob: they read whether that mob has an identity context and
/// refuse nothing.
#[tokio::test]
async fn status_and_capabilities_answer_for_a_second_served_mob() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let second_mob = second.mob_id();
    let mut wrong = Vec::new();
    for method in ["meta/status", "meta/capabilities"] {
        let answer = stdio(&host, method, json!({"mob_id": second_mob})).await;
        let served = answer
            .as_ref()
            .is_some_and(|answer| answer.get("error").is_none());
        if !served {
            wrong.push(format!("{method}: {answer:?}"));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A request of a method whose own parameter `mob_id` is names its mob to
/// the stdio dispatch by position as it does by name: for a mob the host
/// does not have, every shape of such a request is refused with the stdio
/// refusal, before the method's handler reads it.
#[tokio::test]
async fn a_by_position_request_for_a_mob_the_host_does_not_have_is_refused_like_one_by_name() {
    let runtime = runtime().await;
    let default_mob = runtime.mob_id();
    let mut wrong = Vec::new();
    for (method, shape, params) in requests_of_the_methods_that_keep_their_mob(ABSENT_MOB) {
        let answer = stdio(&runtime, method, params).await;
        let refused = answer
            .as_ref()
            .is_some_and(|answer| is_foreign_mob_target(answer, ABSENT_MOB, Some(&default_mob)));
        if !refused {
            let answered = answer.map_or_else(
                || "no answer in time".to_string(),
                |answer| answer.to_string().chars().take(160).collect(),
            );
            wrong.push(format!("{method} {shape}: {answered}"));
        }
    }
    runtime.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// On a host that serves a second mob, an event request that names that
/// mob by position is answered as the same request by name: the dispatch
/// resolves the mob the request names, whatever the shape of its params.
#[tokio::test]
async fn a_by_position_event_request_for_a_second_served_mob_is_answered_like_one_by_name() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let mut wrong = Vec::new();
    for method in ["meta/mob_events/query", "meta/mob_events/subscribe"] {
        let by_name = stdio(&host, method, json!({"mob_id": second_mob})).await;
        let by_position = stdio(&host, method, json!([null, null, null, null, second_mob])).await;
        let refused = by_position
            .as_ref()
            .is_none_or(|answer| is_foreign_mob_target(answer, &second_mob, Some(&launch_mob)));
        if refused || by_name != by_position {
            wrong.push(format!(
                "{method}: by name {by_name:?}, by position {by_position:?}"
            ));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The six job callbacks, in the order of the job door's list: the methods
/// of that list that are `meta/*` methods.
const JOB_CALLBACKS: [&str; 6] = [
    "meta/jobs/heartbeat",
    "meta/jobs/progress",
    "meta/jobs/checkpoint",
    "meta/jobs/complete",
    "meta/jobs/fail",
    "meta/jobs/cancel_ack",
];

/// The job door's methods address a job, not a mob: the seam leaves their
/// params as they are and reads no mob from them. The six job callbacks
/// are named here because they are `meta/*` methods, and every other
/// `meta/*` method but the launch and its end takes the id. The table
/// holds no row of the job door: its methods are asked on that door, in
/// the gateway's own rows.
#[test]
fn the_job_doors_methods_take_no_mob_id() {
    let job_door = crate::rpc::mob_addressing::JOB_DOOR_METHODS;
    let callbacks: Vec<&str> = job_door
        .iter()
        .copied()
        .filter(|method| method.starts_with("meta/"))
        .collect();
    assert_eq!(
        callbacks, JOB_CALLBACKS,
        "the job callbacks of the job door"
    );
    let mut wrong = Vec::new();
    for method in job_door {
        let mut params = json!({"mob_id": ABSENT_MOB, "authority": {"job_id": "a-job"}});
        let before = params.clone();
        let named = crate::rpc::mob_addressing::take_named_mob(method, &mut params);
        let untouched = named.as_ref().is_ok_and(|named| named.named().is_none());
        if !untouched || params != before {
            wrong.push(format!(
                "{method}: the seam read a mob from its params and left them as {params}"
            ));
        }
        if ADDRESSED_METHODS.iter().any(|row| row.0 == *method) {
            wrong.push(format!("{method}: a row of the addressing table"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} methods of the job door are addressed to a mob:\n{}",
        wrong.len(),
        job_door.len(),
        wrong.join("\n")
    );
}

/// A door that does not serve the job callbacks answers them as any method
/// it does not know, whether or not the request carries a `mob_id`: the
/// console RPC, and the stdio dispatch asked directly (on the stdio gateway
/// the job door answers them first). A `mob_id` no mob has does not turn
/// that answer into the refusal of a mob.
#[tokio::test]
async fn a_door_that_does_not_serve_the_job_callbacks_answers_them_alike_with_and_without_a_mob() {
    let runtime = runtime().await;
    let app = open_app(&runtime);
    let mut wrong = Vec::new();
    for method in JOB_CALLBACKS {
        let plain = json!({"authority": {"job_id": "a-job", "attempt_id": "a", "fence": 1}});
        let mut named = plain.clone();
        named["mob_id"] = json!(ABSENT_MOB);
        let on_the_console = console(&app, method, plain.clone()).await;
        let on_the_console_with_a_mob = console(&app, method, named.clone()).await;
        if on_the_console.is_none() || on_the_console != on_the_console_with_a_mob {
            wrong.push(format!(
                "console RPC, {method}: {on_the_console:?} without a mob, {on_the_console_with_a_mob:?} with one"
            ));
        }
        let on_stdio = stdio(&runtime, method, plain).await;
        let on_stdio_with_a_mob = stdio(&runtime, method, named).await;
        if on_stdio.is_none() || on_stdio != on_stdio_with_a_mob {
            wrong.push(format!(
                "stdio dispatch, {method}: {on_stdio:?} without a mob, {on_stdio_with_a_mob:?} with one"
            ));
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The routes of a mob are that mob's: a request adds, lists and deletes
/// the route of the mob it names. Two mobs of one host each have a member
/// named `alice` and a route for her under the same route key; each answers
/// its own route, and deleting one mob's route leaves the other's. A route
/// of the launch mob is written without a mob, as it always was; a route of
/// another mob names it.
///
/// What this row does not show: which route a resolve takes. A resolve
/// asks the router module, which this fixture does not load, so here both
/// mobs get the routing service's one answer for that, the same error
/// whole, and neither a refusal for its mob. That a resolve reads the
/// routes of the mob it names is held by the unit row
/// `member_keyed_routes_two_mobs_recipients_of_one_name_keep_their_own_routes`
/// (the match by mob) and by the source row
/// `every_arm_of_the_stdio_dispatch_resolves_its_mob_or_is_on_one_list`
/// (the resolve arm reads the id the dispatch judged).
#[tokio::test]
async fn a_route_is_added_listed_and_deleted_for_the_mob_a_request_names() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let second_mob = second.mob_id();
    let route = |sink: &str| {
        json!({
            "route": {
                "route_key": "roster:alice",
                "recipient": "alice",
                "channel": "notification",
                "sink": sink,
                "target_module": "delivery",
            }
        })
    };
    let of_the_second = |mut params: Value| {
        params["mob_id"] = json!(second_mob.as_str());
        params
    };
    let sinks_listed = |answer: &Option<Value>| -> Option<Vec<String>> {
        let routes = answer.as_ref()?.get("result")?.get("routes")?.as_array()?;
        Some(
            routes
                .iter()
                .filter_map(|route| route.get("sink").and_then(Value::as_str))
                .map(str::to_string)
                .collect(),
        )
    };
    let mut wrong = Vec::new();

    let added = stdio(&host, "meta/routing/routes/add", route("launch-sink")).await;
    let added_route = added
        .as_ref()
        .and_then(|answer| answer.get("result")?.get("route"));
    if added_route.is_none_or(|route| {
        route.get("sink") != Some(&json!("launch-sink")) || route.get("mob_id").is_some()
    }) {
        wrong.push(format!("the launch mob's route as added: {added:?}"));
    }
    let added = stdio(
        &host,
        "meta/routing/routes/add",
        of_the_second(route("second-sink")),
    )
    .await;
    let added_route = added
        .as_ref()
        .and_then(|answer| answer.get("result")?.get("route"));
    if added_route.is_none_or(|route| {
        route.get("sink") != Some(&json!("second-sink"))
            || route.get("mob_id") != Some(&json!(second_mob.as_str()))
    }) {
        wrong.push(format!("the second mob's route as added: {added:?}"));
    }

    let listed = stdio(&host, "meta/routing/routes/list", json!({})).await;
    if sinks_listed(&listed) != Some(vec!["launch-sink".to_string()]) {
        wrong.push(format!("the launch mob's routes: {listed:?}"));
    }
    let listed = stdio(&host, "meta/routing/routes/list", of_the_second(json!({}))).await;
    if sinks_listed(&listed) != Some(vec!["second-sink".to_string()]) {
        wrong.push(format!("the second mob's routes: {listed:?}"));
    }

    // A resolve needs the router and delivery modules, which this fixture
    // does not load: both mobs are answered the routing service's one
    // answer for that, the same error whole, and neither a refusal for its
    // mob.
    let resolve = json!({"recipient": "alice", "channel": "notification"});
    let resolved = stdio(&host, "meta/routing/resolve", resolve.clone()).await;
    let resolved_for_the_second =
        stdio(&host, "meta/routing/resolve", of_the_second(resolve)).await;
    let the_services_answer = json!({
        "code": -32602,
        "message": "Invalid params: router module is not loaded",
    });
    for (mob, answer) in [
        ("the launch mob", &resolved),
        ("the second mob", &resolved_for_the_second),
    ] {
        if answer
            .as_ref()
            .is_none_or(|answer| answer["error"] != the_services_answer)
        {
            wrong.push(format!(
                "{mob}'s resolve is not the routing service's answer without a router module: \
                 {answer:?}"
            ));
        }
    }

    let deleted = stdio(
        &host,
        "meta/routing/routes/delete",
        of_the_second(json!({"route_key": "roster:alice"})),
    )
    .await;
    let deleted_sink = deleted
        .as_ref()
        .and_then(|answer| answer.get("result")?.get("deleted")?.get("sink"));
    if deleted_sink != Some(&json!("second-sink")) {
        wrong.push(format!("the second mob's route as deleted: {deleted:?}"));
    }
    let listed = stdio(&host, "meta/routing/routes/list", json!({})).await;
    if sinks_listed(&listed) != Some(vec!["launch-sink".to_string()]) {
        wrong.push(format!(
            "the launch mob's routes after the second mob's delete: {listed:?}"
        ));
    }
    let listed = stdio(&host, "meta/routing/routes/list", of_the_second(json!({}))).await;
    if sinks_listed(&listed) != Some(Vec::new()) {
        wrong.push(format!(
            "the second mob's routes after its delete: {listed:?}"
        ));
    }

    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A request of a method keyed by the served mob's id (the routing
/// methods, the delivery methods and the two methods that select a member's
/// events), named with a hosted mob that is known and not loaded, is
/// answered for that mob and leaves it not loaded: a route is added, listed
/// and deleted for it, a resolve is answered by the routing service, its
/// delivery history is listed, a send is answered by the delivery service,
/// an event subscription and an event query are answered, and a gating
/// evaluation is answered by the host's gating owner, while the
/// mob set still knows the mob as not loaded after each.
///
/// Read at the mob set's inventory after every request (the count at the
/// store's own open is Core's to make). The source row holds the cause
/// (`every_arm_of_the_stdio_dispatch_resolves_its_mob_or_is_on_one_list`):
/// an arm of such a method reads the id the dispatch judged and never
/// resolves the mob, and resolving is the one step that loads.
#[tokio::test]
async fn a_request_keyed_by_the_served_mobs_id_leaves_a_dormant_mob_not_loaded() {
    use super::mob_addressing_guard_tests::STDIO_METHODS_KEYED_BY_THE_SERVED_MOB;

    let dormant = DormantMob::of_a_stored_host("keyed-by-its-id").await;
    let host = &dormant.host;
    let launch_mob = host.mob_id();
    let named = dormant.mob_id.as_str();
    let of_the_dormant = |mut params: Value| {
        params["mob_id"] = json!(named);
        params
    };
    let sinks_listed = |answer: &Option<Value>| -> Option<Vec<String>> {
        let routes = answer.as_ref()?.get("result")?.get("routes")?.as_array()?;
        Some(
            routes
                .iter()
                .filter_map(|route| route.get("sink").and_then(Value::as_str))
                .map(str::to_string)
                .collect(),
        )
    };
    let mut wrong = Vec::new();
    let mut asked = BTreeSet::new();

    asked.insert("meta/routing/routes/add");
    let added = stdio(
        host,
        "meta/routing/routes/add",
        of_the_dormant(json!({
            "route": {
                "route_key": "roster:alice",
                "recipient": "alice",
                "channel": "notification",
                "sink": "dormant-sink",
                "target_module": "delivery",
            }
        })),
    )
    .await;
    let added_route = added
        .as_ref()
        .and_then(|answer| answer.get("result")?.get("route"));
    if added_route.is_none_or(|route| {
        route.get("sink") != Some(&json!("dormant-sink"))
            || route.get("mob_id") != Some(&json!(named))
    }) {
        wrong.push(format!("the mob's route as added: {added:?}"));
    }
    if !dormant.is_not_loaded().await {
        wrong.push("meta/routing/routes/add loaded the mob".to_string());
    }

    asked.insert("meta/routing/routes/list");
    let listed = stdio(host, "meta/routing/routes/list", of_the_dormant(json!({}))).await;
    if sinks_listed(&listed) != Some(vec!["dormant-sink".to_string()]) {
        wrong.push(format!("the mob's routes: {listed:?}"));
    }
    let of_the_launch = stdio(host, "meta/routing/routes/list", json!({})).await;
    if sinks_listed(&of_the_launch) != Some(Vec::new()) {
        wrong.push(format!("the launch mob's routes: {of_the_launch:?}"));
    }
    if !dormant.is_not_loaded().await {
        wrong.push("meta/routing/routes/list loaded the mob".to_string());
    }

    // A resolve needs the router and delivery modules, which this fixture
    // does not load: the mob is answered the routing service's one answer
    // for that, and no refusal for its mob.
    asked.insert("meta/routing/resolve");
    let resolved = stdio(
        host,
        "meta/routing/resolve",
        of_the_dormant(json!({"recipient": "alice", "channel": "notification"})),
    )
    .await;
    let the_services_answer = json!({
        "code": -32602,
        "message": "Invalid params: router module is not loaded",
    });
    if resolved
        .as_ref()
        .is_none_or(|answer| answer["error"] != the_services_answer)
        || resolved
            .as_ref()
            .is_some_and(|answer| is_foreign_mob_target(answer, named, Some(&launch_mob)))
    {
        wrong.push(format!(
            "the mob's resolve is not the routing service's answer: {resolved:?}"
        ));
    }
    if !dormant.is_not_loaded().await {
        wrong.push("meta/routing/resolve loaded the mob".to_string());
    }

    asked.insert("meta/routing/routes/delete");
    let deleted = stdio(
        host,
        "meta/routing/routes/delete",
        of_the_dormant(json!({"route_key": "roster:alice"})),
    )
    .await;
    let deleted_sink = deleted
        .as_ref()
        .and_then(|answer| answer.get("result")?.get("deleted")?.get("sink"));
    if deleted_sink != Some(&json!("dormant-sink")) {
        wrong.push(format!("the mob's route as deleted: {deleted:?}"));
    }
    if !dormant.is_not_loaded().await {
        wrong.push("meta/routing/routes/delete loaded the mob".to_string());
    }

    // The delivery methods are keyed by the mob too. The mob's history is
    // its own (none yet). A send needs the delivery module, which this
    // fixture does not load: handed a resolution of the mob, it is answered
    // the delivery service's one answer for that, and no refusal for its
    // mob.
    asked.insert("meta/delivery/history");
    let history = stdio(
        host,
        "meta/delivery/history",
        of_the_dormant(json!({"limit": 50})),
    )
    .await;
    let deliveries = history
        .as_ref()
        .and_then(|answer| answer.get("result")?.get("deliveries")?.as_array());
    if deliveries.is_none_or(|deliveries| !deliveries.is_empty()) {
        wrong.push(format!("the mob's delivery history: {history:?}"));
    }
    if !dormant.is_not_loaded().await {
        wrong.push("meta/delivery/history loaded the mob".to_string());
    }

    asked.insert("meta/delivery/send");
    let sent = stdio(
        host,
        "meta/delivery/send",
        of_the_dormant(json!({
            "resolution": {
                "route_id": "route-000001",
                "recipient": "alice",
                "channel": "notification",
                "sink": "webhook",
                "target_module": "delivery",
                "retry_max": 1,
                "backoff_ms": 250,
                "rate_limit_per_minute": 2,
                "mob_id": named,
            },
            "payload": {"message": "hello"},
        })),
    )
    .await;
    let the_delivery_services_answer = json!({
        "code": -32602,
        "message": "Invalid params: delivery module is not loaded",
    });
    if sent
        .as_ref()
        .is_none_or(|answer| answer["error"] != the_delivery_services_answer)
    {
        wrong.push(format!(
            "the mob's send is not the delivery service's answer: {sent:?}"
        ));
    }
    if !dormant.is_not_loaded().await {
        wrong.push("meta/delivery/send loaded the mob".to_string());
    }

    // The two methods that select a member's events are keyed by the mob
    // too: each is answered for the mob from what the host keeps of it,
    // and the mob set manages no mob it did not manage before.
    let managed_before = crate::unified_runtime::known(host).await.loaded;
    asked.insert("meta/events/subscribe");
    let subscribed = stdio(
        host,
        "meta/events/subscribe",
        of_the_dormant(json!({"scope": "mob"})),
    )
    .await;
    let subscription = subscribed
        .as_ref()
        .filter(|answer| answer.get("error").is_none())
        .and_then(|answer| answer.get("result"));
    if subscription.is_none_or(|result| {
        !result.get("events").is_some_and(Value::is_array)
            || !result.get("event_frames").is_some_and(Value::is_array)
    }) {
        wrong.push(format!("the mob's event subscription: {subscribed:?}"));
    }
    if !dormant.is_not_loaded().await {
        wrong.push("meta/events/subscribe loaded the mob".to_string());
    }

    // The gating owner is the host's: an evaluation for the mob names the
    // mob on its rows and loads nothing.
    asked.insert("meta/gating/evaluate");
    let evaluated = stdio(
        host,
        "meta/gating/evaluate",
        of_the_dormant(json!({
            "action": "publish_report",
            "actor_id": "host-job",
            "risk_tier": "r1",
        })),
    )
    .await;
    if evaluated
        .as_ref()
        .and_then(|answer| answer.get("result")?.get("action_id"))
        .is_none()
    {
        wrong.push(format!("the mob's gating evaluation: {evaluated:?}"));
    }
    if !dormant.is_not_loaded().await {
        wrong.push("meta/gating/evaluate loaded the mob".to_string());
    }

    // A host with no event log store answers the query that it has none.
    asked.insert("meta/query_events");
    let queried = stdio(host, "meta/query_events", of_the_dormant(json!({}))).await;
    let rows = queried
        .as_ref()
        .filter(|answer| answer.get("error").is_none())
        .and_then(|answer| answer.get("result"));
    if rows.is_none_or(|result| {
        !result.is_array() && result.get("status") != Some(&json!("no_event_log_configured"))
    }) {
        wrong.push(format!("the mob's event query: {queried:?}"));
    }
    if !dormant.is_not_loaded().await {
        wrong.push("meta/query_events loaded the mob".to_string());
    }
    let managed_after = crate::unified_runtime::known(host).await.loaded;
    if managed_after != managed_before {
        wrong.push(format!(
            "the event methods changed the managed mobs: {managed_before:?} to {managed_after:?}"
        ));
    }

    host.shutdown().await;
    assert_eq!(
        asked,
        STDIO_METHODS_KEYED_BY_THE_SERVED_MOB
            .iter()
            .copied()
            .collect::<BTreeSet<_>>(),
        "every method keyed by the served mob's id is asked"
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A request's send that names a stopped mob of the host is answered the
/// same way whether or not that mob was loaded: the request resolves its
/// mob before its arm runs, which loads a stopped mob that is not loaded,
/// and the answer is the one the request gets for the stopped mob that
/// stayed loaded. The mob the request loaded is still stopped.
#[tokio::test]
async fn a_send_that_names_a_stopped_mob_is_answered_the_same_loaded_or_not() {
    let stored = crate::unified_runtime::StoredHost::new();
    let host = stored.launch().await;
    let set = host
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    let mut mobs = Vec::new();
    for tag in ["stopped-and-loaded", "stopped-and-asleep"] {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"addressing-rows-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
            uuid::Uuid::new_v4().simple()
        ))
        .expect("mob definition");
        let mob_id = Box::pin(set.mob_create_definition(definition))
            .await
            .expect("the mob set creates a mob of the host");
        set.mob_stop(&mob_id).await.expect("the mob is stopped");
        mobs.push(mob_id);
    }
    let (stays_loaded, unloaded) = (&mobs[0], &mobs[1]);
    // One stays loaded behind an owner lease, as a host service's work on
    // it holds it; the other is unloaded as idle.
    let lease = set
        .handle_for(stays_loaded)
        .await
        .expect("the mob that stays loaded")
        .acquire_owner()
        .expect("an owner lease");
    crate::unified_runtime::asleep(&host, &[unloaded]).await;
    assert!(
        crate::unified_runtime::known(&host)
            .await
            .loaded
            .contains(stays_loaded),
        "the leased mob stays loaded"
    );

    let mut answers = Vec::new();
    for mob_id in [stays_loaded, unloaded] {
        let answer = stdio(
            &host,
            "meta/send",
            json!({
                "mob_id": mob_id.as_str(),
                "identity": "desk",
                "content": "a word for the desk of a stopped mob",
            }),
        )
        .await;
        answers.push(
            answer
                .map(|answer| answer.to_string().replace(mob_id.as_str(), "<the mob>"))
                .unwrap_or_else(|| "no answer in time".to_string()),
        );
    }
    let loaded_after = crate::unified_runtime::known(&host).await.loaded;
    let mut still_stopped = Vec::new();
    for mob_id in [stays_loaded, unloaded] {
        still_stopped.push(
            set.handle_for(mob_id)
                .await
                .map(|handle| handle.status_observation_snapshot())
                .ok(),
        );
    }
    drop(lease);
    host.shutdown().await;

    assert_eq!(
        answers[1], answers[0],
        "the send for the stopped mob that was not loaded is answered as the loaded one's"
    );
    assert!(
        answers[0].contains("\"error\""),
        "a send to a member of a stopped mob is refused: {}",
        answers[0]
    );
    assert!(
        loaded_after.contains(unloaded),
        "the request loaded the stopped mob to answer it"
    );
    assert_eq!(
        still_stopped,
        vec![Some(meerkat_mob::MobState::Stopped); 2],
        "both mobs are still stopped"
    );
}

/// The identity plane a row gives a mob: its identity runtime, and the
/// double of Core's member reads behind it.
struct IdentityPlane {
    identity_runtime: Arc<crate::identity_first::IdentityRuntime>,
    core: Arc<crate::identity_first::core_test_bridge::CoreReadsBridge>,
}

/// A runtime like [`runtime`] whose mob has an identity context of its own,
/// installed in the mob's entry as a launch installs it. `instance` names
/// its identity runtime.
async fn runtime_with_an_identity_plane(instance: &str) -> (UnifiedRuntime, IdentityPlane) {
    let mut runtime = runtime().await;
    let core = Arc::new(crate::identity_first::core_test_bridge::CoreReadsBridge::default());
    let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
        crate::identity_first::IdentityRuntimeConfig {
            continuity_store: Arc::new(
                crate::identity_first::LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
            runtime_instance_id: format!("addressing-rows-{instance}"),
            has_runtime_store: true,
            durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core.clone()),
            default_timeout: None,
        },
    ));
    runtime.install_identity_first_context(Arc::new(
        crate::identity_first::IdentityFirstRuntimeContext::new(
            Arc::clone(&identity_runtime),
            Arc::new(crate::unified_runtime::held_identity_test_support::Roster(
                Vec::new(),
            )),
            None,
            None,
            Some(runtime.mob_handle().definition().clone()),
        ),
    ));
    (
        runtime,
        IdentityPlane {
            identity_runtime,
            core,
        },
    )
}

/// The identity context a launch hands the stdio dispatch for its own mob:
/// the identity runtime of `plane`, which is the one installed in
/// `runtime`'s entry.
fn launch_context(runtime: &UnifiedRuntime, plane: &IdentityPlane) -> crate::IdentityFirstContext {
    crate::IdentityFirstContext {
        runtime: crate::identity_first::PublicIdentityRuntime::new(
            Arc::clone(&plane.identity_runtime),
            runtime.mob_runtime().clone(),
        ),
        roster_provider: Arc::new(crate::unified_runtime::held_identity_test_support::Roster(
            Vec::new(),
        )),
        topology_provider: None,
        customizer: None,
        agent_memory_provider: None,
        mob_definition: Some(runtime.mob_handle().definition().clone()),
        transcript_edit_service: None,
        compaction_floors: None,
    }
}

/// Register the identity `name` in `plane` with its member live at
/// `generation`: the runtime id and the session the identity is bound to.
async fn register_member(plane: &IdentityPlane, name: &str, generation: u64) -> (String, String) {
    let identity = crate::identity_first::AgentIdentity::parse(name).expect("identity");
    let session = meerkat_core::types::SessionId::new();
    let runtime_id = crate::identity_first::core_test_bridge::register_live(
        &plane.identity_runtime,
        &plane.core.core,
        crate::unified_runtime::held_identity_test_support::member(&identity),
        generation,
        session.clone(),
    )
    .await
    .expect("the identity registers");
    (runtime_id.as_str().to_string(), session.to_string())
}

/// One request on the stdio dispatch of a launch that hands the dispatch
/// `launch_context`: the row waits for the dispatch's own answer and for
/// nothing else, so no clock of the row decides what it sees. A request
/// that never answers is ended by the test runner's own bound, by name.
/// `None` when the answer is no JSON.
async fn stdio_of_a_launch(
    runtime: &UnifiedRuntime,
    launch_context: Option<&crate::IdentityFirstContext>,
    method: &str,
    params: Value,
) -> Option<Value> {
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let answer = Box::pin(crate::rpc::handle_unified_rpc_json(
        runtime,
        &request,
        Duration::from_secs(1),
        None,
        launch_context,
    ))
    .await;
    serde_json::from_str(&answer).ok()
}

/// The stdio methods that answer a generated runtime id whose identity is
/// bound to another incarnation with the typed refusal that names the
/// registered binding, each with the parameter that carries the id.
const METHODS_REFUSING_A_STALE_RUNTIME_ID: &[(&str, &str)] = &[
    ("meta/completion_cursor", "identity"),
    ("meta/delete_identity", "identity"),
    ("meta/dispatch", "identity"),
    ("meta/get_member", "member_id"),
    ("meta/inspect_identity", "identity"),
    ("meta/instruction_status", "identity"),
    ("meta/interact", "identity"),
    ("meta/member_instruction_activations", "identity"),
    ("meta/recover_delivery", "identity"),
    ("meta/reset", "identity"),
    ("meta/respawn", "identity"),
    ("meta/retire", "identity"),
    ("meta/send", "identity"),
    ("meta/send_status", "identity"),
    ("meta/status_identity", "identity"),
    ("meta/subscribe", "identity"),
    ("meta/turn_result", "identity"),
    ("meta/wait_for_completion", "identity"),
    ("meta/wait_for_turn", "identity"),
];

/// A host with an identity context serves a second mob that has its own.
/// Both mobs have an identity of one name, bound to another incarnation and
/// another session in each, and a request carries a generated runtime id
/// that is stale in both. The typed refusal of an identity method names the
/// binding of the mob the request is for: a request for the second mob is
/// refused by the second mob's own identity runtime, and never asked of the
/// launch's. The same for a one-sided wire, whose handler asks the resolved
/// mob's identity runtime.
#[tokio::test]
async fn a_generated_alias_in_a_request_for_a_second_mob_is_refused_by_that_mobs_own_identity_runtime()
 {
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let (second, second_plane) = runtime_with_an_identity_plane("second").await;
    let of_the_launch = register_member(&host_plane, "ghost", 3).await;
    let of_the_second = register_member(&second_plane, "ghost", 2).await;
    assert_ne!(of_the_launch, of_the_second);
    let launch_context = launch_context(&host, &host_plane);
    let second = host.serve_a_second_mob_in_tests(second);
    let second_mob = second.mob_id();
    let stale = "rt:ghost:1";

    let mut wrong = Vec::new();
    for (method, parameter) in METHODS_REFUSING_A_STALE_RUNTIME_ID {
        for (mob, (binding, session)) in [
            (None, &of_the_launch),
            (Some(second_mob.as_str()), &of_the_second),
        ] {
            let mut params = serde_json::Map::new();
            params.insert((*parameter).to_string(), json!(stale));
            if let Some(mob) = mob {
                params.insert("mob_id".to_string(), json!(mob));
            }
            let answer =
                stdio_of_a_launch(&host, Some(&launch_context), method, Value::Object(params))
                    .await;
            let data = answer.as_ref().map(|answer| &answer["error"]["data"]);
            let refused_by_its_own = data.is_some_and(|data| {
                data["kind"] == json!("stale_identity_runtime_binding")
                    && data["registered_runtime_member_id"] == json!(binding)
                    && data["registered_session_id"] == json!(session)
                    && data["live_runtime_member_id"] == json!(stale)
            });
            if !refused_by_its_own {
                let answered = answer.map_or_else(
                    || "an answer that is no JSON".to_string(),
                    |answer| answer.get("error").unwrap_or(&answer).to_string(),
                );
                wrong.push(format!(
                    "{method} for {}: not refused with the binding {binding}: {answered}",
                    mob.map_or("no mob", |_| "the second mob")
                ));
            }
        }
    }

    // The one-sided wire is handed no identity context for the second mob:
    // its handler asks that mob's own identity runtime.
    let peer_key = [42_u8; 32];
    let wired = stdio_of_a_launch(
        &host,
        Some(&launch_context),
        "meta/cross_mob/wire_local",
        json!({
            "mob_id": second_mob,
            "local_member_id": stale,
            "remote_comms_name": "a-far-mob/worker/front",
            "remote_peer_id": meerkat_core::comms::PeerId::from_ed25519_pubkey(&peer_key).to_string(),
            "remote_address": "inproc://a-far-mob/worker/front",
        }),
    )
    .await;
    let refusal = wired
        .as_ref()
        .and_then(|answer| answer["error"]["message"].as_str())
        .unwrap_or_default();
    if !refusal.contains(&format!("in mob '{second_mob}'"))
        || refusal.contains("require the owning IdentityRuntime")
    {
        wrong.push(format!(
            "meta/cross_mob/wire_local for the second mob: {wired:?}"
        ));
    }

    host.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "{} answers:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// A member name two mobs have reaches its own mob's identity runtime
/// through an identity method: each mob answers the status of its own
/// identity of that name, bound to its own session, and a request that
/// names no mob is the launch mob's.
#[tokio::test]
async fn a_member_name_two_mobs_have_reaches_its_own_mobs_identity_runtime() {
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let (second, second_plane) = runtime_with_an_identity_plane("second").await;
    let of_the_launch = register_member(&host_plane, "desk", 1).await;
    let of_the_second = register_member(&second_plane, "desk", 1).await;
    assert_ne!(of_the_launch.1, of_the_second.1, "each mob's own session");
    let launch_context = launch_context(&host, &host_plane);
    let second = host.serve_a_second_mob_in_tests(second);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();

    let mut answers = Vec::new();
    for mob in [None, Some(launch_mob.as_str()), Some(second_mob.as_str())] {
        let mut params = json!({"identity": "desk"});
        if let Some(mob) = mob {
            params["mob_id"] = json!(mob);
        }
        answers.push(
            stdio_of_a_launch(&host, Some(&launch_context), "meta/status_identity", params).await,
        );
    }
    host.shutdown().await;
    second.shutdown().await;

    let session_of = |answer: &Option<Value>| {
        answer
            .as_ref()
            .and_then(|answer| answer["result"]["session_id"].as_str())
            .map(str::to_string)
    };
    assert_eq!(
        session_of(&answers[0]),
        Some(of_the_launch.1.clone()),
        "a request that names no mob reaches the launch mob's identity: {:?}",
        answers[0]
    );
    assert_eq!(
        session_of(&answers[1]),
        Some(of_the_launch.1.clone()),
        "a request that names the launch mob reaches its identity: {:?}",
        answers[1]
    );
    assert_eq!(
        session_of(&answers[2]),
        Some(of_the_second.1.clone()),
        "a request that names the second mob reaches that mob's own identity: {:?}",
        answers[2]
    );
}

/// The methods whose arms work with an identity context: the listed ones
/// and one of each family.
fn methods_working_with_an_identity_context() -> Vec<&'static str> {
    STDIO_METHODS_WORKING_WITH_AN_IDENTITY_CONTEXT
        .iter()
        .chain(FAMILY_METHODS_WORKING_WITH_AN_IDENTITY_CONTEXT)
        .copied()
        .collect()
}

/// The params of [`params_naming`], bounded where a method would wait for
/// an identity bootstrap no row runs.
fn identity_params_naming(method: &str, mob_id: Option<&str>, scratch: &std::path::Path) -> Value {
    let mut params = params_naming(method, mob_id, scratch);
    if method == "meta/wait_identity_bootstrap" {
        params["timeout_ms"] = json!(50);
    }
    params
}

/// What `host`, whose launch hands the dispatch `launch_context`, answers
/// each method that works with an identity context for the mob `mob_id`:
/// the method and its whole answer (`None` when the answer is no JSON).
async fn identity_answers_for(
    host: &UnifiedRuntime,
    launch_context: Option<&crate::IdentityFirstContext>,
    mob_id: Option<&str>,
) -> Vec<(&'static str, Option<Value>)> {
    let scratch = tempfile::tempdir().expect("tempdir");
    let mut answers = Vec::new();
    for method in methods_working_with_an_identity_context() {
        let params = identity_params_naming(method, mob_id, scratch.path());
        let answer = stdio_of_a_launch(host, launch_context, method, params).await;
        answers.push((method, answer));
    }
    answers
}

/// The whole error the stdio dispatch answers for a mob it hosts whose entry
/// holds no identity context, on a host whose launch composed one: the
/// refusal of a mob id with both ids, typed, which says in its data that
/// the gateway hosts the mob and that the mob has no identity context. Its
/// code is not the one of a method the gateway does not have.
fn no_identity_context_for(mob_id: &str, launch_mob: &str) -> Value {
    json!({
        "code": -32602,
        "message": format!("identity-first runtime not configured for mob '{mob_id}'"),
        "data": {
            "kind": "foreign_mob_target",
            "requested_mob_id": mob_id,
            "gateway_mob_id": launch_mob,
            "hosted": true,
            "identity_context": false,
        },
    })
}

/// Whether `answer` refuses the request for the mob it names: as a mob the
/// host does not have, as a mob the method does not serve, or as a mob
/// without an identity context. Each is the typed refusal of a mob id.
fn refused_for_its_mob(answer: &Value) -> bool {
    answer["error"]["data"]["kind"] == json!("foreign_mob_target")
}

/// SERVED WITHOUT AN IDENTITY PLANE. A host that composed no identity plane
/// serves its launch mob by the path that needs none: the arm is handed no
/// identity context and works on the mob. A second mob of such a host is
/// served the same way: no method that works with an identity context is
/// refused for it because of the mob it names, and each answers the second
/// mob as it answers the launch mob: a result where the launch mob gets a
/// result, and where it gets an error the same error, whole (its code, its
/// text and its data, each mob's own id aside).
#[tokio::test]
async fn a_second_mob_is_served_without_an_identity_plane_on_a_host_that_composed_none() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let for_the_second = identity_answers_for(&host, None, Some(&second_mob)).await;
    let for_the_launch = identity_answers_for(&host, None, Some(&launch_mob)).await;
    host.shutdown().await;
    second.shutdown().await;

    let comparable = |answer: &Option<Value>| match answer {
        None => "no JSON answer".to_string(),
        Some(answer) => match answer.get("error").filter(|error| !error.is_null()) {
            Some(error) => error
                .to_string()
                .replace(second_mob.as_str(), "<mob>")
                .replace(launch_mob.as_str(), "<mob>"),
            None => "a result".to_string(),
        },
    };
    let mut wrong = Vec::new();
    for ((method, answer), (_, launch_answer)) in for_the_second.iter().zip(&for_the_launch) {
        let (of_the_second, of_the_launch) = (comparable(answer), comparable(launch_answer));
        if answer.is_none() || launch_answer.is_none() {
            wrong.push(format!("{method}: an answer that is no JSON"));
        } else if answer.as_ref().is_some_and(refused_for_its_mob) {
            wrong.push(format!(
                "{method}: refused for the mob it names: {of_the_second}"
            ));
        } else if of_the_second != of_the_launch {
            wrong.push(format!(
                "{method}: answers {of_the_second} for the second mob and {of_the_launch} for \
                 the launch mob"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} methods:\n{}",
        wrong.len(),
        for_the_second.len(),
        wrong.join("\n")
    );
}

/// REFUSED WHEN THE HOST HAS AN IDENTITY PLANE AND THE ENTRY HAS NONE. A
/// host whose launch composed an identity context serves a second mob whose
/// entry holds no identity context: a request for that mob on a method that
/// works with an identity context is refused, and the refusal says why: it
/// is the answer of a gateway without an identity plane, with the mob
/// named, and not the refusal of a mob the host does not have: the typed
/// refusal of a mob id whose data says that the gateway hosts the mob and
/// that the mob has no identity context. It is never the bare answer of a
/// method the gateway does not have, which a client reads as an older
/// gateway. The arm never hands its handler the launch's identity runtime
/// for it, and never takes the path that needs none on a host that has
/// one.
#[tokio::test]
async fn an_identity_first_request_is_refused_when_the_host_has_an_identity_plane_and_the_mobs_entry_holds_none()
 {
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let launch_context = launch_context(&host, &host_plane);
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let answers = identity_answers_for(&host, Some(&launch_context), Some(&second_mob)).await;
    host.shutdown().await;
    second.shutdown().await;

    let refusal = no_identity_context_for(&second_mob, &launch_mob);
    let wrong: Vec<String> = answers
        .iter()
        .filter(|(_, answer)| {
            answer
                .as_ref()
                .is_none_or(|answer| answer["error"] != refusal)
        })
        .map(|(method, answer)| {
            format!(
                "{method}: not refused for want of the mob's identity context, answered {}",
                answer.as_ref().map_or_else(
                    || "no JSON".to_string(),
                    |answer| answer.get("error").unwrap_or(answer).to_string()
                )
            )
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "{} of {} methods:\n{}",
        wrong.len(),
        answers.len(),
        wrong.join("\n")
    );
}

/// A second mob that has an identity context of its own is served with it:
/// no method that works with an identity context is refused for it because
/// of the mob it names, whether or not the host's launch composed an
/// identity context for the launch mob.
#[tokio::test]
async fn an_identity_first_request_for_a_second_mob_with_its_own_identity_context_is_served() {
    let mut wrong = Vec::new();
    for host_has_an_identity_plane in [true, false] {
        let (host, launch_context) = if host_has_an_identity_plane {
            let (host, plane) = runtime_with_an_identity_plane("host").await;
            let launch_context = launch_context(&host, &plane);
            (host, Some(launch_context))
        } else {
            (runtime().await, None)
        };
        let (second, _second_plane) = runtime_with_an_identity_plane("second").await;
        let second = host.serve_a_second_mob_in_tests(second);
        let second_mob = second.mob_id();
        let answers = identity_answers_for(&host, launch_context.as_ref(), Some(&second_mob)).await;
        host.shutdown().await;
        second.shutdown().await;
        for (method, answer) in answers {
            let refused = answer.as_ref().is_some_and(refused_for_its_mob);
            if refused || answer.is_none() {
                wrong.push(format!(
                    "{method} on a host {} an identity plane: {}",
                    if host_has_an_identity_plane {
                        "with"
                    } else {
                        "without"
                    },
                    answer.map_or_else(
                        || "an answer that is no JSON".to_string(),
                        |answer| format!("refused for the mob it names: {}", answer["error"])
                    )
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} answers:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// What the stdio dispatch answers a method of the family of a mob's
/// long-term agents (`IDENTITY_FAMILY_METHODS`) asked for a mob it hosts beside the
/// launch mob, by what that mob's entry holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FamilyAnswerForASecondMob {
    /// The mob's own family answers: a result, or the family's own typed
    /// refusal.
    TheMobsOwnFamily,
    /// The typed refusal of a mob without an identity context on a host
    /// whose launch composed one.
    NoIdentityContext,
    /// The answer of a method the gateway does not have.
    MethodNotFound,
}

impl FamilyAnswerForASecondMob {
    /// Whether `answer` is this answer for the mob `mob_id` on a host whose
    /// launch mob is `launch_mob`.
    fn is(self, answer: &Value, mob_id: &str, launch_mob: &str) -> bool {
        match self {
            Self::TheMobsOwnFamily => {
                answer.get("result").is_some_and(|result| !result.is_null())
                    || answer["error"]["code"]
                        == json!(crate::rpc::identity_methods::IDENTITY_REFUSED_CODE)
            }
            Self::NoIdentityContext => {
                answer["error"] == no_identity_context_for(mob_id, launch_mob)
            }
            Self::MethodNotFound => {
                answer["error"]["code"] == json!(-32601)
                    && answer["error"]["message"] == json!("Method not found")
            }
        }
    }
}

/// THE FAMILY OF A MOB'S LONG-TERM AGENTS IS EACH MOB'S OWN. A request of
/// the family (the methods of `IDENTITY_FAMILY_METHODS`; `meta/identity/warm`,
/// `meta/identity/resolved_tools` and `meta/identity/routing_status` are not of
/// it and are served for the mob a request names too) that names another mob
/// the host serves
/// is answered by that mob's own family, for every method of the family: a
/// result, or the family's own typed refusal. It is never refused as a
/// family that is the launch mob's alone. A mob whose entry holds no
/// identity plane on a host whose launch composed one answers the typed
/// refusal for want of an identity context, and a mob whose identity plane
/// holds no family (a mob that stopped) answers as a method the gateway
/// does not have. `meta/capabilities` asked about such a mob lists the
/// family's methods exactly when that mob has a family. The launch mob's
/// own request is answered by the family, and on a host that composed no
/// identity plane by the answer of a method the gateway does not have.
#[tokio::test]
async fn a_request_of_the_long_term_agents_family_for_a_second_mob_is_answered_by_that_mobs_own_family()
 {
    use crate::rpc::identity_methods::IDENTITY_FAMILY_METHODS;

    let lists_the_family = |capabilities: &Option<Value>| {
        capabilities
            .as_ref()
            .and_then(|capabilities| capabilities["result"]["methods"].as_array())
            .map(|methods| {
                IDENTITY_FAMILY_METHODS
                    .iter()
                    .filter(|method| methods.contains(&json!(method)))
                    .count()
            })
    };
    let mut wrong = Vec::new();
    for (host_has_an_identity_plane, the_second_holds) in [
        (true, "no identity plane"),
        (true, "an identity plane with no family"),
        (true, "an identity plane with a family"),
        (false, "no identity plane"),
    ] {
        let (host, launch_context) = if host_has_an_identity_plane {
            let (host, plane) = runtime_with_an_identity_plane("host").await;
            let launch_context = launch_context(&host, &plane);
            (host, Some(launch_context))
        } else {
            (runtime().await, None)
        };
        let second = match the_second_holds {
            "no identity plane" => runtime().await,
            "an identity plane with no family" => {
                // An identity plane as a host composes one for a mob beside
                // its launch mob: no family is attached to it.
                let (second, plane) = runtime_with_an_identity_plane("second").await;
                plane.identity_runtime.detach_identity_family();
                second
            }
            _ => runtime_with_an_identity_plane("second").await.0,
        };
        let second = host.serve_a_second_mob_in_tests(second);
        let launch_mob = host.mob_id();
        let second_mob = second.mob_id();
        let case = format!(
            "a host {} an identity plane, a second mob with {the_second_holds}",
            if host_has_an_identity_plane {
                "with"
            } else {
                "without"
            },
        );
        for method in IDENTITY_FAMILY_METHODS {
            let answer = stdio_of_a_launch(
                &host,
                launch_context.as_ref(),
                method,
                json!({"mob_id": second_mob}),
            )
            .await;
            let expected = match (host_has_an_identity_plane, the_second_holds) {
                (true, "no identity plane") => FamilyAnswerForASecondMob::NoIdentityContext,
                (_, "an identity plane with a family") => {
                    FamilyAnswerForASecondMob::TheMobsOwnFamily
                }
                _ => FamilyAnswerForASecondMob::MethodNotFound,
            };
            if answer
                .as_ref()
                .is_none_or(|answer| !expected.is(answer, &second_mob, &launch_mob))
            {
                wrong.push(format!(
                    "{case}: {method} for the second mob: {} and not {expected:?}",
                    answer.as_ref().map_or_else(
                        || "no JSON".to_string(),
                        |answer| answer.get("error").unwrap_or(answer).to_string()
                    )
                ));
            }
        }
        let capabilities = stdio_of_a_launch(
            &host,
            launch_context.as_ref(),
            "meta/capabilities",
            json!({"mob_id": second_mob}),
        )
        .await;
        let listed_for_the_second = if the_second_holds == "an identity plane with a family" {
            IDENTITY_FAMILY_METHODS.len()
        } else {
            0
        };
        if lists_the_family(&capabilities) != Some(listed_for_the_second) {
            wrong.push(format!(
                "{case}: meta/capabilities for the second mob lists {:?} methods of the family, \
                 not {listed_for_the_second}",
                lists_the_family(&capabilities)
            ));
        }

        // The launch mob's own request, named by its id and by no mob: the
        // family's answer, or on a host that serves no family the answer of
        // a method the gateway does not have.
        for (naming, params) in [
            ("its id", json!({"mob_id": launch_mob})),
            ("no mob", json!({})),
        ] {
            let of_the_launch =
                stdio_of_a_launch(&host, launch_context.as_ref(), "meta/identity/list", params)
                    .await;
            let as_expected = of_the_launch.as_ref().is_some_and(|answer| {
                if host_has_an_identity_plane {
                    answer.get("result").is_some_and(|result| !result.is_null())
                } else {
                    answer["error"]["code"] == json!(-32601)
                        && answer["error"]["message"] == json!("Method not found")
                }
            });
            if !as_expected {
                wrong.push(format!(
                    "{case}: meta/identity/list for the launch mob, named by {naming}: \
                     {of_the_launch:?}"
                ));
            }
        }
        let capabilities = stdio_of_a_launch(
            &host,
            launch_context.as_ref(),
            "meta/capabilities",
            json!({"mob_id": launch_mob}),
        )
        .await;
        let listed_for_the_launch = if host_has_an_identity_plane {
            IDENTITY_FAMILY_METHODS.len()
        } else {
            0
        };
        if lists_the_family(&capabilities) != Some(listed_for_the_launch) {
            wrong.push(format!(
                "{case}: meta/capabilities for the launch mob lists {:?} methods of the family, \
                 not {listed_for_the_launch}",
                lists_the_family(&capabilities)
            ));
        }
        host.shutdown().await;
        second.shutdown().await;
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The family of a mob's long-term agents is served for the mob a request
/// names through the dispatch's one resolving step, and is refused for no
/// mob before it. In the dispatch's text the family is not tested before
/// the arms (the refusal that stood there for another mob is gone), it has
/// one arm, and that arm's first step is the step that resolves the
/// request's mob (`request_mob!()`, the one step that loads a mob that is
/// not loaded): a request of the family that names a mob that is not
/// loaded loads that mob, as a request of every method served per mob
/// does, and nothing of the family is asked of any mob before it
/// (`a_restart_keeps_the_agents_created_in_each_of_two_mobs` shows it on a
/// mob that is known and not loaded). Two things the row of the refusal
/// held still hold: nothing in the dispatch resolves a mob before that
/// step is defined, and the dispatch asks whether a method is of the
/// family in one place.
#[test]
fn the_long_term_agents_family_is_served_through_the_step_that_resolves_the_requests_mob() {
    const THE_REFUSALS_TEST: &str =
        "identity_methods::is_identity_family_method(request.method.as_str())";
    const THE_FAMILYS_TEST: &str = "is_identity_family_method(";
    const THE_ARM: &str = "method if identity_methods::is_identity_family_method(method) => {";
    let dispatch = stdio_dispatch();
    let at = |text: &str| {
        dispatch
            .find(text)
            .unwrap_or_else(|| panic!("the stdio dispatch has no `{text}`"))
    };
    assert_eq!(
        dispatch.matches(THE_REFUSALS_TEST).count(),
        0,
        "the dispatch refuses the family for no mob before its arms"
    );
    assert_eq!(
        dispatch.matches(THE_FAMILYS_TEST).count(),
        1,
        "the dispatch asks whether a method is of the family in one place, its arm: no branch \
         treats the family apart under another spelling"
    );
    assert_eq!(
        dispatch.matches(THE_ARM).count(),
        1,
        "the family has one arm"
    );
    let the_step = at("macro_rules! request_mob");
    assert!(
        !dispatch[..the_step].contains("resolve_mob("),
        "nothing resolves a mob before the step that resolves a request's mob is defined"
    );
    let arm = at(THE_ARM);
    assert!(
        the_step < arm && at("let response = match request.method.as_str() {") < arm,
        "the family's arm is one of the dispatch's arms, after the step that resolves a mob is \
         defined"
    );
    let first_step = dispatch[arm + THE_ARM.len()..].trim_start();
    assert!(
        first_step.starts_with("let mob = request_mob!();"),
        "the family's arm first resolves the request's mob: it begins `{}`",
        first_step.chars().take(60).collect::<String>()
    );
}

/// `meta/status` and `meta/capabilities` report on the mob they answer for.
/// For a second mob that has an identity context of its own the status
/// carries that mob's identity bootstrap and the capabilities list the
/// identity methods, whether or not the launch composed an identity context
/// for the launch mob. For a second mob that has none they report none,
/// also on a host whose launch composed one; neither is ever refused.
#[tokio::test]
async fn status_and_capabilities_report_the_identity_context_of_the_mob_they_answer_for() {
    let mut wrong = Vec::new();
    for (host_has_one, second_has_one) in [(true, true), (false, true), (true, false)] {
        let (host, launch_context) = if host_has_one {
            let (host, plane) = runtime_with_an_identity_plane("host").await;
            let launch_context = launch_context(&host, &plane);
            (host, Some(launch_context))
        } else {
            (runtime().await, None)
        };
        let second = if second_has_one {
            runtime_with_an_identity_plane("second").await.0
        } else {
            runtime().await
        };
        let second = host.serve_a_second_mob_in_tests(second);
        let params = json!({"mob_id": second.mob_id()});
        let status = stdio_of_a_launch(
            &host,
            launch_context.as_ref(),
            "meta/status",
            params.clone(),
        )
        .await;
        let capabilities =
            stdio_of_a_launch(&host, launch_context.as_ref(), "meta/capabilities", params).await;
        host.shutdown().await;
        second.shutdown().await;

        let case = format!(
            "a host {} an identity context, a second mob {} one",
            if host_has_one { "with" } else { "without" },
            if second_has_one { "with" } else { "without" },
        );
        let reports_a_bootstrap = status
            .as_ref()
            .and_then(|status| status["result"].get("identity_bootstrap"))
            .is_some_and(|bootstrap| !bootstrap.is_null());
        let lists_the_identity_methods = capabilities
            .as_ref()
            .and_then(|capabilities| capabilities["result"]["methods"].as_array())
            .is_some_and(|methods| methods.contains(&json!("meta/status_identity")));
        let answered = status
            .as_ref()
            .is_some_and(|status| status.get("error").is_none())
            && capabilities
                .as_ref()
                .is_some_and(|capabilities| capabilities.get("error").is_none());
        if !answered {
            wrong.push(format!("{case}: refused: {status:?} {capabilities:?}"));
        }
        if reports_a_bootstrap != second_has_one {
            wrong.push(format!(
                "{case}: meta/status reports an identity bootstrap: {reports_a_bootstrap}"
            ));
        }
        if lists_the_identity_methods != second_has_one {
            wrong.push(format!(
                "{case}: meta/capabilities lists the identity methods: {lists_the_identity_methods}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Give the identity runtime of `plane` an agent memory store of its own
/// under `dir`: the store's provider.
async fn give_agent_memory(
    plane: &IdentityPlane,
    dir: &std::path::Path,
) -> Arc<dyn crate::identity_first::AgentMemoryProvider> {
    let store = Arc::new(
        crate::memory::SqliteAgentMemoryStore::open(dir.join("agent-memory"))
            .expect("agent memory store"),
    );
    let provider: Arc<dyn crate::identity_first::AgentMemoryProvider> = store;
    plane
        .identity_runtime
        .set_agent_memory(Some(
            crate::identity_first::AgentMemoryRuntimeInjector::new(
                Arc::clone(&provider),
                crate::identity_first::AgentMemoryConfig::default(),
            ),
        ))
        .await;
    provider
}

/// Two mobs with an agent of one name keep their own agent memory: what is
/// remembered for the agent of one mob is recalled for that mob's agent
/// alone, and no agent memory method is refused for the second mob like a
/// mob the host does not have.
#[tokio::test]
async fn two_mobs_keep_their_own_agent_memory() {
    let (dir_host, dir_second) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let (second, second_plane) = runtime_with_an_identity_plane("second").await;
    give_agent_memory(&host_plane, dir_host.path()).await;
    give_agent_memory(&second_plane, dir_second.path()).await;
    register_member(&host_plane, "desk", 1).await;
    register_member(&second_plane, "desk", 1).await;
    let launch_context = launch_context(&host, &host_plane);
    let second = host.serve_a_second_mob_in_tests(second);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let naming = |mut params: Value, mob: Option<&str>| {
        if let Some(mob) = mob {
            params["mob_id"] = json!(mob);
        }
        params
    };
    let remember = |title: &str, mob: Option<&str>| {
        naming(
            json!({
                "identity": "desk",
                "realm": "team",
                "title": title,
                "body": "The desk keeps the standup notes.",
                "tags": ["standup"],
            }),
            mob,
        )
    };
    let recall = |mob: Option<&str>| {
        naming(
            json!({
                "identity": "desk",
                "realm": "team",
                "selection": "contextual",
                "query_terms": ["standup"],
                "max_entries": 8,
            }),
            mob,
        )
    };
    let titles = |answer: &Option<Value>| -> Option<Vec<String>> {
        let records = answer.as_ref()?.get("result")?.get("records")?.as_array()?;
        Some(
            records
                .iter()
                .filter_map(|record| record["title"].as_str())
                .map(str::to_string)
                .collect(),
        )
    };

    let mut wrong = Vec::new();
    for (title, mob) in [
        ("Of the launch mob", None),
        ("Of the second mob", Some(second_mob.as_str())),
    ] {
        let remembered = stdio_of_a_launch(
            &host,
            Some(&launch_context),
            "meta/agent_memory/remember",
            remember(title, mob),
        )
        .await;
        if !remembered
            .as_ref()
            .is_some_and(|answer| answer["result"]["title"] == json!(title))
        {
            wrong.push(format!("remembering {title:?}: {remembered:?}"));
        }
    }
    for (title, mob) in [
        ("Of the launch mob", None),
        ("Of the launch mob", Some(launch_mob.as_str())),
        ("Of the second mob", Some(second_mob.as_str())),
    ] {
        let recalled = stdio_of_a_launch(
            &host,
            Some(&launch_context),
            "meta/agent_memory/recall",
            recall(mob),
        )
        .await;
        if titles(&recalled) != Some(vec![title.to_string()]) {
            wrong.push(format!(
                "the recall for {} finds {:?}, not only {title:?}: {recalled:?}",
                mob.unwrap_or("no mob"),
                titles(&recalled)
            ));
        }
    }
    for method in AGENT_MEMORY_METHODS {
        let answer = stdio_of_a_launch(
            &host,
            Some(&launch_context),
            method,
            json!({"mob_id": second_mob}),
        )
        .await;
        if answer
            .as_ref()
            .is_none_or(|answer| is_foreign_mob_target(answer, &second_mob, Some(&launch_mob)))
        {
            wrong.push(format!("{method} for the second mob: {answer:?}"));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The two identity methods that name no member of the mob's roster answer
/// for the mob a request names: `meta/registered_spec` the row that mob's
/// own identity runtime registered, and `meta/status_identity_bootstrap`
/// that mob's identity bootstrap.
#[tokio::test]
async fn the_identity_methods_that_name_no_member_answer_for_the_mob_a_request_names() {
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let (second, second_plane) = runtime_with_an_identity_plane("second").await;
    let identity = crate::identity_first::AgentIdentity::parse("desk").expect("identity");
    for (plane, label) in [
        (&host_plane, "the-launch-mob"),
        (&second_plane, "the-second-mob"),
    ] {
        let mut spec = crate::unified_runtime::held_identity_test_support::member(&identity);
        spec.labels.insert("of".to_string(), label.to_string());
        crate::identity_first::core_test_bridge::register_live(
            &plane.identity_runtime,
            &plane.core.core,
            spec,
            1,
            meerkat_core::types::SessionId::new(),
        )
        .await
        .expect("the identity registers");
    }
    let launch_context = launch_context(&host, &host_plane);
    let second = host.serve_a_second_mob_in_tests(second);
    let second_mob = second.mob_id();

    let mut wrong = Vec::new();
    for (label, mob) in [
        ("the-launch-mob", None),
        ("the-second-mob", Some(second_mob.as_str())),
    ] {
        let mut params = json!({"identity": "desk"});
        if let Some(mob) = mob {
            params["mob_id"] = json!(mob);
        }
        let registered =
            stdio_of_a_launch(&host, Some(&launch_context), "meta/registered_spec", params).await;
        if !registered
            .as_ref()
            .is_some_and(|answer| answer["result"]["spec"]["labels"]["of"] == json!(label))
        {
            wrong.push(format!(
                "meta/registered_spec for {}: not the row of {label}: {registered:?}",
                mob.unwrap_or("no mob")
            ));
        }
        let mut params = json!({});
        if let Some(mob) = mob {
            params["mob_id"] = json!(mob);
        }
        let bootstrap = stdio_of_a_launch(
            &host,
            Some(&launch_context),
            "meta/status_identity_bootstrap",
            params,
        )
        .await;
        if !bootstrap
            .as_ref()
            .is_some_and(|answer| answer.get("error").is_none() && !answer["result"].is_null())
        {
            wrong.push(format!(
                "meta/status_identity_bootstrap for {}: {bootstrap:?}",
                mob.unwrap_or("no mob")
            ));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A transcript service a host could compose for a mob: a persistent
/// session service over stores of its own under `dir`.
fn a_transcript_service(
    dir: &std::path::Path,
) -> Arc<dyn crate::memory::hygienist::TranscriptEditSessionService> {
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
        meerkat_store::SqliteSessionStore::open(
            crate::MetaStorageLayout::with_injected_roots(dir.to_path_buf(), None)
                .session_db()
                .expect("the layout names the session database")
                .path,
        )
        .expect("session store"),
    );
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(
            crate::MetaStorageLayout::with_injected_roots(dir.to_path_buf(), None).runtime_db(),
        )
        .expect("runtime store"),
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let mut builder = meerkat::FactoryAgentBuilder::new(
        meerkat::AgentFactory::new(dir),
        meerkat::Config::default(),
    );
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ))
}

/// What a request for a second mob needs beside that mob's identity context
/// is what the host composed for that mob and gave its entry: the provider
/// of its agent memory, its transcript service and its compaction floors,
/// the same values and none of the launch's. So a verb that needs one of
/// them reaches the second mob's member: `meta/compact_member` answers for
/// the binding of the second mob's identity, and `meta/capabilities` lists
/// the agent memory methods of the second mob's provider.
#[tokio::test]
async fn the_request_services_of_a_second_mob_are_its_entrys_own() {
    let (dir_host, dir_second) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let (second, second_plane) = runtime_with_an_identity_plane("second").await;
    let launch_provider = give_agent_memory(&host_plane, dir_host.path()).await;
    let provider = give_agent_memory(&second_plane, dir_second.path()).await;
    let transcript = a_transcript_service(dir_second.path());
    let floors = Arc::new(crate::identity_first::CompactionFloorRegistry::default());
    second.set_identity_request_services(crate::unified_runtime::IdentityRequestServices {
        agent_memory_provider: Some(Arc::clone(&provider)),
        transcript_edit_service: Some(Arc::clone(&transcript)),
        compaction_floors: Some(Arc::clone(&floors)),
    });
    let of_the_second = register_member(&second_plane, "ghost", 2).await;
    let mut launch_context = launch_context(&host, &host_plane);
    launch_context.agent_memory_provider = Some(launch_provider);
    let second = host.serve_a_second_mob_in_tests(second);
    let second_id = second.mob_handle().mob_id().clone();
    let second_mob = second.mob_id();

    let mut wrong = Vec::new();
    match host.resolve_mob(Some(&second_id)).await {
        Ok(mob) => match super::identity_context_of_a_served_mob(&mob) {
            Some(context) => {
                if !context
                    .agent_memory_provider
                    .as_ref()
                    .is_some_and(|handed| Arc::ptr_eq(handed, &provider))
                {
                    wrong.push("the agent memory provider is not the entry's".to_string());
                }
                if !context
                    .transcript_edit_service
                    .as_ref()
                    .is_some_and(|handed| Arc::ptr_eq(handed, &transcript))
                {
                    wrong.push("the transcript service is not the entry's".to_string());
                }
                if !context
                    .compaction_floors
                    .as_ref()
                    .is_some_and(|handed| Arc::ptr_eq(handed, &floors))
                {
                    wrong.push("the compaction floors are not the entry's".to_string());
                }
                if !Arc::ptr_eq(context.runtime.runtime(), &second_plane.identity_runtime) {
                    wrong.push("the identity runtime is not the entry's".to_string());
                }
            }
            None => wrong.push("the second mob's entry answers no identity context".to_string()),
        },
        Err(error) => wrong.push(format!("the second mob does not resolve: {error:?}")),
    }

    let compacted = stdio_of_a_launch(
        &host,
        Some(&launch_context),
        "meta/compact_member",
        json!({"mob_id": second_mob, "identity": "rt:ghost:1"}),
    )
    .await;
    if !compacted.as_ref().is_some_and(|answer| {
        answer["error"]["data"]["kind"] == json!("stale_identity_runtime_binding")
            && answer["error"]["data"]["registered_runtime_member_id"] == json!(of_the_second.0)
    }) {
        wrong.push(format!(
            "meta/compact_member does not reach the second mob's identity: {compacted:?}"
        ));
    }
    let capabilities = stdio_of_a_launch(
        &host,
        Some(&launch_context),
        "meta/capabilities",
        json!({"mob_id": second_mob}),
    )
    .await;
    if !capabilities
        .as_ref()
        .and_then(|answer| answer["result"]["methods"].as_array())
        .is_some_and(|methods| methods.contains(&json!("meta/agent_memory/recall")))
    {
        wrong.push(format!(
            "meta/capabilities does not list the second mob's agent memory: {capabilities:?}"
        ));
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The delivery methods are served for the launch mob as before: a request
/// that names no mob, or the launch mob's id, is never answered with the
/// refusal of a mob the host does not have, also on a host that serves a
/// second mob. And the list of the stdio methods that serve the launch mob
/// alone names none of them: it is empty.
#[tokio::test]
async fn the_delivery_methods_are_served_for_the_launch_mob_and_are_on_no_launch_mob_only_list() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let mut wrong = Vec::new();
    for method in DELIVERY_METHODS_KEYED_BY_THE_SERVED_MOB {
        for mob in [None, Some(launch_mob.as_str())] {
            let params = params_naming(method, mob, scratch.path());
            let answer = stdio(&host, method, params).await;
            let refused_for_its_mob = answer.as_ref().is_none_or(|answer| {
                answer["error"]["data"]["kind"] == json!("foreign_mob_target")
            });
            if refused_for_its_mob {
                wrong.push(format!(
                    "{method} for {}: {answer:?}",
                    mob.map_or("no mob", |_| "the launch mob")
                ));
            }
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert_eq!(
        crate::rpc::mob_addressing::STDIO_METHODS_SERVING_THE_LAUNCH_MOB_ONLY,
        &[] as &[&str],
        "a method that serves the launch mob alone again needs its rows here"
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The stdio dispatch reads a mob by position only on a method whose own
/// parameter `mob_id` is. On any other method params that are an array name
/// no mob, whatever they hold: a request shaped like an event query by
/// position, with the id of a mob the host does not have where that query's
/// mob is, is a request for the launch mob and is not refused for its mob.
#[tokio::test]
async fn a_by_position_request_of_a_method_that_does_not_keep_its_mob_names_no_mob() {
    let host = runtime().await;
    let mut wrong = Vec::new();
    for method in ["meta/query_events", "meta/list_members", "meta/status"] {
        let answer = stdio(&host, method, json!([null, null, null, null, ABSENT_MOB])).await;
        let refused_for_its_mob = answer
            .as_ref()
            .is_none_or(|answer| answer["error"]["data"]["kind"] == json!("foreign_mob_target"));
        if refused_for_its_mob {
            wrong.push(format!("{method}: {answer:?}"));
        }
    }
    host.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Two mobs of one host, each with an agent memory store and a memory realm
/// of its own, as a launch composes them: the launch mob's store is the
/// launch's, the second mob's its own directory below it. An agent of one
/// name is in both. A request that carries no `realm` is for the memory
/// realm of the mob it names: what is remembered for the launch mob's agent
/// is not recalled for the second mob's agent of that name, and the other
/// way round. A request that names a realm reads that realm of the store of
/// the mob it names and of no other: the other mob's realm by name, the
/// name of the directory the second mob's store is in, and a path through
/// or out of it, each find nothing. Each store then holds its own mob's
/// note alone.
#[tokio::test]
async fn an_agent_memory_request_without_a_realm_is_for_the_memory_realm_of_the_mob_it_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let (second, second_plane) = runtime_with_an_identity_plane("second").await;
    let second_mob = second.mob_id();
    let realm_of_the_second = format!("mob.{second_mob}");
    let of_the_launch =
        crate::memory::SqliteAgentMemoryStore::open(dir.path().join("agent-memory"))
            .expect("agent memory store");
    let mob_of_the_second = meerkat_mob::MobId::from(second_mob.as_str());
    let of_the_second = of_the_launch
        .of_hosted_mob_in_tests(&mob_of_the_second)
        .expect("the second mob's own store");
    let directory_of_the_second = of_the_launch
        .root_of_hosted_mob(&mob_of_the_second)
        .ok()
        .and_then(|root| {
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .expect("the second mob's directory has a name");
    let through_the_directory = format!(
        "{}/{directory_of_the_second}/{realm_of_the_second}",
        crate::memory::sqlite_store::HOSTED_MOBS_DIR
    );
    for (plane, store, realm) in [
        (&host_plane, of_the_launch.clone(), "default".to_string()),
        (
            &second_plane,
            of_the_second.clone(),
            realm_of_the_second.clone(),
        ),
    ] {
        let provider: Arc<dyn crate::identity_first::AgentMemoryProvider> = Arc::new(store);
        plane
            .identity_runtime
            .set_agent_memory(Some(
                crate::identity_first::AgentMemoryRuntimeInjector::new(
                    provider,
                    crate::identity_first::AgentMemoryConfig {
                        realm,
                        ..Default::default()
                    },
                ),
            ))
            .await;
    }
    register_member(&host_plane, "desk", 1).await;
    register_member(&second_plane, "desk", 1).await;
    let launch_context = launch_context(&host, &host_plane);
    let second = host.serve_a_second_mob_in_tests(second);
    let request = |mut params: Value, mob: Option<&str>, realm: Option<&str>| {
        if let Some(mob) = mob {
            params["mob_id"] = json!(mob);
        }
        if let Some(realm) = realm {
            params["realm"] = json!(realm);
        }
        params
    };
    let remember = |title: &str| {
        json!({
            "identity": "desk",
            "title": title,
            "body": "The desk keeps the standup notes.",
            "tags": ["standup"],
        })
    };
    let recall = || {
        json!({
            "identity": "desk",
            "selection": "contextual",
            "query_terms": ["standup"],
            "max_entries": 8,
        })
    };
    let titles = |answer: &Option<Value>| -> Option<Vec<String>> {
        let records = answer.as_ref()?.get("result")?.get("records")?.as_array()?;
        let mut titles: Vec<String> = records
            .iter()
            .filter_map(|record| record["title"].as_str())
            .map(str::to_string)
            .collect();
        titles.sort();
        Some(titles)
    };

    let mut wrong = Vec::new();
    for (title, mob) in [
        ("Of the launch mob", None),
        ("Of the second mob", Some(second_mob.as_str())),
    ] {
        let remembered = stdio_of_a_launch(
            &host,
            Some(&launch_context),
            "meta/agent_memory/remember",
            request(remember(title), mob, None),
        )
        .await;
        if remembered
            .as_ref()
            .is_none_or(|answer| answer["result"]["title"] != json!(title))
        {
            wrong.push(format!("remembering {title:?}: {remembered:?}"));
        }
    }
    for (what, mob, realm, expected) in [
        ("no mob, no realm", None, None, Some("Of the launch mob")),
        (
            "the second mob, no realm",
            Some(second_mob.as_str()),
            None,
            Some("Of the second mob"),
        ),
        (
            "the second mob, its own realm by name",
            Some(second_mob.as_str()),
            Some(realm_of_the_second.as_str()),
            Some("Of the second mob"),
        ),
        (
            "no mob, the second mob's realm by name",
            None,
            Some(realm_of_the_second.as_str()),
            None,
        ),
        (
            "the second mob, the launch mob's realm by name",
            Some(second_mob.as_str()),
            Some("default"),
            None,
        ),
        (
            "no mob, the name of the directory of the hosted mobs' stores",
            None,
            Some(crate::memory::sqlite_store::HOSTED_MOBS_DIR),
            None,
        ),
        (
            "no mob, a path through the second mob's directory to its realm",
            None,
            Some(through_the_directory.as_str()),
            None,
        ),
        (
            "the second mob, a path out of its directory to the launch mob's realm",
            Some(second_mob.as_str()),
            Some("../../default"),
            None,
        ),
    ] {
        let recalled = stdio_of_a_launch(
            &host,
            Some(&launch_context),
            "meta/agent_memory/recall",
            request(recall(), mob, realm),
        )
        .await;
        let expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        if titles(&recalled) != Some(expected.clone()) {
            wrong.push(format!(
                "the recall for {what} finds {:?}, not exactly {expected:?}",
                titles(&recalled)
            ));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    for (what, store, realm, expected) in [
        (
            "the launch's store, the launch mob's realm",
            &of_the_launch,
            "default",
            Some("Of the launch mob"),
        ),
        (
            "the launch's store, the second mob's realm",
            &of_the_launch,
            realm_of_the_second.as_str(),
            None,
        ),
        (
            "the second mob's store, its own realm",
            &of_the_second,
            realm_of_the_second.as_str(),
            Some("Of the second mob"),
        ),
        (
            "the second mob's store, the launch mob's realm",
            &of_the_second,
            "default",
            None,
        ),
    ] {
        let held = crate::identity_first::AgentMemoryProvider::recall(
            store,
            crate::identity_first::agent_memory::AgentMemoryRecallRequest {
                identity: crate::identity_first::AgentIdentity::parse("desk").expect("identity"),
                realm: realm.to_string(),
                query_text: None,
                query_terms: Vec::new(),
                selection: crate::identity_first::agent_memory::AgentMemorySelection::Always,
                max_entries: 64,
            },
        )
        .await
        .map(|records| {
            records
                .into_iter()
                .map(|record| record.title)
                .collect::<Vec<_>>()
        });
        let expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        if held.as_ref().ok() != Some(&expected) {
            wrong.push(format!("{what} holds {held:?}, not exactly {expected:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A request for the launch mob is unchanged: without a `realm` it takes
/// the wire's default realm, "default", also on a host whose agent memory
/// is configured with another realm. What it remembers is found under
/// "default" by name and not under the configured realm.
#[tokio::test]
async fn an_agent_memory_request_for_the_launch_mob_without_a_realm_takes_the_wires_default_realm()
{
    let dir = tempfile::tempdir().expect("tempdir");
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let store = Arc::new(
        crate::memory::SqliteAgentMemoryStore::open(dir.path().join("agent-memory"))
            .expect("agent memory store"),
    );
    let provider: Arc<dyn crate::identity_first::AgentMemoryProvider> = store;
    host_plane
        .identity_runtime
        .set_agent_memory(Some(
            crate::identity_first::AgentMemoryRuntimeInjector::new(
                provider,
                crate::identity_first::AgentMemoryConfig {
                    realm: "a-configured-realm".to_string(),
                    ..Default::default()
                },
            ),
        ))
        .await;
    register_member(&host_plane, "desk", 1).await;
    let launch_context = launch_context(&host, &host_plane);
    let launch_mob = host.mob_id();

    let mut wrong = Vec::new();
    for (title, mob) in [
        ("Named no mob", None),
        ("Named the launch mob", Some(launch_mob.as_str())),
    ] {
        let mut params = json!({
            "identity": "desk",
            "title": title,
            "body": "The desk keeps the standup notes.",
            "tags": ["standup"],
        });
        if let Some(mob) = mob {
            params["mob_id"] = json!(mob);
        }
        let remembered = stdio_of_a_launch(
            &host,
            Some(&launch_context),
            "meta/agent_memory/remember",
            params,
        )
        .await;
        if remembered
            .as_ref()
            .is_none_or(|answer| answer["result"]["title"] != json!(title))
        {
            wrong.push(format!("remembering {title:?}: {remembered:?}"));
        }
    }
    for (realm, expected) in [
        (None, 2_usize),
        (Some("default"), 2),
        (Some("a-configured-realm"), 0),
    ] {
        let mut params = json!({
            "identity": "desk",
            "selection": "contextual",
            "query_terms": ["standup"],
            "max_entries": 8,
        });
        if let Some(realm) = realm {
            params["realm"] = json!(realm);
        }
        let recalled = stdio_of_a_launch(
            &host,
            Some(&launch_context),
            "meta/agent_memory/recall",
            params,
        )
        .await;
        let found = recalled
            .as_ref()
            .and_then(|answer| answer["result"]["records"].as_array())
            .map(Vec::len);
        if found != Some(expected) {
            wrong.push(format!(
                "the recall under {realm:?} finds {found:?} memories, not {expected}: {recalled:?}"
            ));
        }
    }
    host.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---------------------------------------------------------------------------
// The console events of the mob a request names
// ---------------------------------------------------------------------------

/// What stands behind the `desk` of a row's mob.
#[derive(Clone, Copy)]
enum DeskBridge {
    /// An identity of the mob's identity runtime, over Core's member reads
    /// and its retires, releases and deliveries.
    Delivering,
    /// An identity of the mob's identity runtime, over a Core that commits
    /// a fresh life.
    Committing,
    /// No identity of the mob's identity runtime: a live member of the mob
    /// itself, which an identity method reaches by the member's name.
    LiveMember,
}

/// What a mob's console events hold of the identity `desk`: its pending
/// interactions, its response phase, and its events by type and by the
/// interaction each names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeskConsoleState {
    pending: Vec<String>,
    phase: Option<String>,
    events: Vec<(String, Option<String>)>,
}

async fn desk_console_state(
    events: &crate::unified_runtime::ConsoleEventStore,
) -> DeskConsoleState {
    DeskConsoleState {
        pending: events.pending_interaction_ids("desk").await,
        phase: events.response_phase_for_identity("desk").await,
        events: events
            .replay_all(None)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|event| event.identity == "desk")
            .map(|event| (event.event_type, event.interaction_id))
            .collect(),
    }
}

/// Where Core's facts of a row's `desk` are read.
enum DeskCore {
    Reads(Arc<crate::identity_first::core_test_bridge::CoreReadsBridge>),
    Commits(Arc<super::tests::RpcResetTestBridge>),
    /// The mob's own roster.
    Mob,
}

/// One mob of [`TwoDesks`]: its runtime, its identity runtime, the roster
/// its identity context serves, and where Core's facts of its `desk` are.
struct DeskMob {
    runtime: UnifiedRuntime,
    identity_runtime: Arc<crate::identity_first::IdentityRuntime>,
    roster: Vec<crate::identity_first::DurableAgentSpec>,
    core: DeskCore,
    /// Where the sessions of a mob with a live member are kept.
    dir: Option<tempfile::TempDir>,
}

/// A runtime like [`runtime`] whose mob has an identity context of its own
/// over `bridge`, installed in the mob's entry as a launch installs it, and
/// a `desk`: an identity whose member is live, or a live member of the mob
/// (then the mob's members have comms, as a mob's members must).
async fn a_mob_with_a_desk(instance: &str, bridge: DeskBridge) -> DeskMob {
    use crate::identity_first::core_test_bridge::{CoreReadsBridge, register_live};

    let (mut runtime, dir) = match bridge {
        DeskBridge::LiveMember => {
            let dir = tempfile::tempdir().expect("tempdir");
            let runtime = Box::pin(
                UnifiedRuntime::builder()
                    .mob_spec(super::tests::rpc_test_mob_spec(&dir).expect("mob spec"))
                    .module_config(crate::MetaConfig {
                        modules: Vec::new(),
                        discovery: crate::DiscoverySpec {
                            namespace: format!("console-rows-{instance}"),
                            modules: Vec::new(),
                        },
                        pre_spawn: Vec::new(),
                    })
                    .timeout(Duration::from_secs(5))
                    .build(),
            )
            .await
            .expect("runtime builds");
            (runtime, Some(dir))
        }
        DeskBridge::Delivering | DeskBridge::Committing => (runtime().await, None),
    };
    let desk = crate::identity_first::AgentIdentity::parse("desk").expect("identity");
    let spec = crate::unified_runtime::held_identity_test_support::member(&desk);
    let config = |bridge: Arc<dyn crate::identity_first::SessionBridge>| {
        crate::identity_first::IdentityRuntimeConfig {
            continuity_store: Arc::new(
                crate::identity_first::LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
            runtime_instance_id: format!("console-rows-{instance}"),
            has_runtime_store: true,
            durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        }
    };
    let session = meerkat_core::types::SessionId::new();
    let (identity_runtime, roster, core) = match bridge {
        DeskBridge::Delivering => {
            let core = Arc::new(CoreReadsBridge::with_deliveries());
            let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(config(
                core.clone(),
            )));
            register_live(&identity_runtime, &core.core, spec.clone(), 1, session)
                .await
                .expect("the identity registers");
            (identity_runtime, vec![spec], DeskCore::Reads(core))
        }
        DeskBridge::Committing => {
            let core = Arc::new(super::tests::RpcResetTestBridge::default());
            let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(config(
                core.clone(),
            )));
            register_live(&identity_runtime, &core.core, spec.clone(), 1, session)
                .await
                .expect("the identity registers");
            (identity_runtime, vec![spec], DeskCore::Commits(core))
        }
        DeskBridge::LiveMember => {
            let spawned = Box::pin(runtime.spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "desk".to_string(),
                None,
                None,
                None,
            )))
            .await;
            assert!(
                spawned.is_ok(),
                "the mob's own member desk is spawned: {:?}",
                spawned.as_ref().err()
            );
            let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(config(
                Arc::new(CoreReadsBridge::default()),
            )));
            (identity_runtime, Vec::new(), DeskCore::Mob)
        }
    };
    crate::unified_runtime::held_identity_test_support::install_roster(
        &mut runtime,
        Arc::clone(&identity_runtime),
        &roster,
    );
    DeskMob {
        runtime,
        identity_runtime,
        roster,
        core,
        dir,
    }
}

/// Two mobs of one host, each with an identity plane of its own and a
/// `desk`. Each mob's console events hold one pending interaction of its
/// `desk`, reserved before any request.
struct TwoDesks {
    host: UnifiedRuntime,
    second: &'static UnifiedRuntime,
    launch_context: crate::IdentityFirstContext,
    core_of_the_launch: DeskCore,
    core_of_the_second: DeskCore,
    _dirs: [Option<tempfile::TempDir>; 2],
}

impl TwoDesks {
    async fn new(bridge: DeskBridge) -> Self {
        let launch = a_mob_with_a_desk("host", bridge).await;
        let second = a_mob_with_a_desk("second", bridge).await;
        let launch_context = crate::IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::clone(&launch.identity_runtime),
                launch.runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(crate::unified_runtime::held_identity_test_support::Roster(
                launch.roster.clone(),
            )),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: Some(launch.runtime.mob_handle().definition().clone()),
            transcript_edit_service: None,
            compaction_floors: None,
        };
        let host = launch.runtime;
        let desks = Self {
            second: host.serve_a_second_mob_in_tests(second.runtime),
            host,
            launch_context,
            core_of_the_launch: launch.core,
            core_of_the_second: second.core,
            _dirs: [launch.dir, second.dir],
        };
        for (events, kept) in [
            (desks.events_of_the_launch(), "kept-by-the-launch"),
            (desks.events_of_the_second(), "kept-by-the-second"),
        ] {
            events
                .reserve_interaction_value("desk", None, kept, "a row", json!("an earlier input"))
                .await
                .expect("reserved");
        }
        desks
    }

    /// The console events of the launch mob: the host's store.
    fn events_of_the_launch(&self) -> crate::unified_runtime::ConsoleEventStore {
        self.host.console_events()
    }

    /// The console events of the second mob: its own store of the host's.
    fn events_of_the_second(&self) -> crate::unified_runtime::ConsoleEventStore {
        let second_mob = self.second.mob_handle().mob_id().clone();
        self.host.console_events().of_mob(Some(&second_mob))
    }

    /// The `desk` of one mob as Core holds it: the member's binding, as
    /// text.
    async fn member_of(&self, of_the_second: bool) -> String {
        let (core, runtime) = if of_the_second {
            (&self.core_of_the_second, self.second)
        } else {
            (&self.core_of_the_launch, &self.host)
        };
        let desk = crate::identity_first::AgentIdentity::parse("desk").expect("identity");
        match core {
            DeskCore::Reads(core) => format!("{:?}", core.core.live_member(&desk)),
            DeskCore::Commits(core) => format!("{:?}", core.core.live_member(&desk)),
            DeskCore::Mob => format!(
                "{:?}",
                runtime
                    .mob_handle()
                    .get_member(&meerkat_mob::AgentIdentity::from("desk"))
                    .await
                    .ok()
                    .flatten()
                    .and_then(|entry| entry.bridge_session_id().cloned())
            ),
        }
    }

    async fn shutdown(self) {
        self.host.shutdown().await;
        self.second.shutdown().await;
    }
}

/// What the console events of the mob a request was for hold of `desk`
/// after the request, beside what they held before.
enum DeskOutcome {
    /// A lifecycle record of this type: the pending interaction is failed
    /// for it, then the record is appended.
    Lifecycle(&'static str),
    /// One more pending interaction, the one the answer names, and no event.
    Reserved,
    /// One more pending interaction, and the input as an event of it.
    Sent,
}

/// One request of `method` for the `desk` of each mob in turn, on a host
/// that serves two mobs which each have a `desk`. The request is answered
/// with a result, the console events of the mob it names hold `outcome`,
/// and the other mob's `desk` is as it was: its pending interaction, its
/// response phase, its events, and its member as Core holds it. What is
/// wrong, by line.
async fn a_request_for_one_mobs_desk(
    method: &str,
    bridge: DeskBridge,
    params: Value,
    outcome: DeskOutcome,
) -> Vec<String> {
    let mut wrong = Vec::new();
    for for_the_second in [true, false] {
        let desks = TwoDesks::new(bridge).await;
        let (named, other) = if for_the_second {
            ("the second mob", "the launch mob")
        } else {
            ("the launch mob", "the second mob")
        };
        let (events_of_the_named, events_of_the_other) = if for_the_second {
            (desks.events_of_the_second(), desks.events_of_the_launch())
        } else {
            (desks.events_of_the_launch(), desks.events_of_the_second())
        };
        let before_named = desk_console_state(&events_of_the_named).await;
        let before_other = desk_console_state(&events_of_the_other).await;
        let member_before = desks.member_of(!for_the_second).await;

        let mut params = params.clone();
        if for_the_second {
            params["mob_id"] = json!(desks.second.mob_id());
        }
        let answer =
            stdio_of_a_launch(&desks.host, Some(&desks.launch_context), method, params).await;
        let result = answer
            .as_ref()
            .and_then(|answer| answer.get("result"))
            .filter(|result| !result.is_null());
        if result.is_none() {
            wrong.push(format!(
                "{method} for {named}'s desk: no result: {answer:?}"
            ));
        }

        let after_named = desk_console_state(&events_of_the_named).await;
        let after_other = desk_console_state(&events_of_the_other).await;
        if after_other != before_other {
            wrong.push(format!(
                "{method} for {named}'s desk: the console events of {other}'s desk changed: \
                 {before_other:?} to {after_other:?}"
            ));
        }
        let member_after = desks.member_of(!for_the_second).await;
        if member_after != member_before {
            wrong.push(format!(
                "{method} for {named}'s desk: the member of {other}'s desk changed: \
                 {member_before} to {member_after}"
            ));
        }
        let kept = before_named.pending.clone();
        let expected = match outcome {
            DeskOutcome::Lifecycle(record) => DeskConsoleState {
                pending: Vec::new(),
                phase: None,
                events: before_named
                    .events
                    .iter()
                    .cloned()
                    .chain([
                        ("interaction_failed".to_string(), kept.first().cloned()),
                        (record.to_string(), None),
                    ])
                    .collect(),
            },
            DeskOutcome::Reserved => {
                let reserved = result
                    .and_then(|result| result["interaction_id"].as_str())
                    .unwrap_or("no interaction id in the answer")
                    .to_string();
                DeskConsoleState {
                    pending: kept.iter().cloned().chain([reserved]).collect(),
                    ..before_named.clone()
                }
            }
            DeskOutcome::Sent => {
                let sent = after_named.pending.get(1).cloned();
                DeskConsoleState {
                    pending: kept.iter().cloned().chain(sent.clone()).collect(),
                    phase: before_named.phase.clone(),
                    events: before_named
                        .events
                        .iter()
                        .cloned()
                        .chain([("user_input".to_string(), sent)])
                        .collect(),
                }
            }
        };
        if after_named != expected
            || matches!(outcome, DeskOutcome::Sent) && after_named.pending.len() != 2
        {
            wrong.push(format!(
                "{method} for {named}'s desk: its console events hold {after_named:?}, not \
                 {expected:?}"
            ));
        }
        if method == "meta/interact" {
            // The stream the answer names is the launch mob's console
            // stream of that name: an answer for another mob names none.
            let stream = result.and_then(|result| result.get("stream"));
            let names_the_launch_mobs_stream = stream.is_some_and(|stream| {
                stream["route"] == json!("/console/identity/desk/stream")
                    && stream["identity"] == json!("desk")
            });
            if for_the_second && stream.is_some()
                || !for_the_second && !names_the_launch_mobs_stream
            {
                wrong.push(format!(
                    "{method} for {named}'s desk: the stream its answer names: {stream:?}"
                ));
            }
        }
        desks.shutdown().await;
    }
    wrong
}

/// `meta/interact` for the `desk` of one mob reserves its interaction in
/// that mob's console events and leaves the other mob's `desk` as it was;
/// an answer for a mob that is not the launch mob names no console stream.
#[tokio::test]
async fn meta_interact_acts_on_the_console_events_of_the_mob_it_names() {
    let wrong = a_request_for_one_mobs_desk(
        "meta/interact",
        DeskBridge::Delivering,
        json!({"identity": "desk", "content": "hello"}),
        DeskOutcome::Reserved,
    )
    .await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `meta/send_message` for the `desk` of one mob reserves its interaction
/// and records the input in that mob's console events, and leaves the
/// other mob's `desk` as it was.
#[tokio::test]
async fn meta_send_message_acts_on_the_console_events_of_the_mob_it_names() {
    let wrong = a_request_for_one_mobs_desk(
        "meta/send_message",
        DeskBridge::Delivering,
        json!({"member_id": "desk", "message": "hello"}),
        DeskOutcome::Sent,
    )
    .await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `meta/respawn` for the `desk` of one mob records the respawn in that
/// mob's console events, where it fails that `desk`'s pending interaction,
/// and leaves the other mob's `desk` as it was. Each `desk` is a live
/// member of its mob, which the method respawns by its name.
#[tokio::test]
async fn meta_respawn_acts_on_the_console_events_of_the_mob_it_names() {
    let wrong = a_request_for_one_mobs_desk(
        "meta/respawn",
        DeskBridge::LiveMember,
        json!({"identity": "desk"}),
        DeskOutcome::Lifecycle("identity_respawned"),
    )
    .await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `meta/retire` for the `desk` of one mob records the retirement in that
/// mob's console events, where it fails that `desk`'s pending interaction,
/// and leaves the other mob's `desk` as it was.
#[tokio::test]
async fn meta_retire_acts_on_the_console_events_of_the_mob_it_names() {
    let wrong = a_request_for_one_mobs_desk(
        "meta/retire",
        DeskBridge::Delivering,
        json!({"identity": "desk"}),
        DeskOutcome::Lifecycle("identity_retired"),
    )
    .await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `meta/reset` for the `desk` of one mob records the reset in that mob's
/// console events, where it fails that `desk`'s pending interaction, and
/// leaves the other mob's `desk` as it was.
#[tokio::test]
async fn meta_reset_acts_on_the_console_events_of_the_mob_it_names() {
    let wrong = a_request_for_one_mobs_desk(
        "meta/reset",
        DeskBridge::Committing,
        json!({"identity": "desk"}),
        DeskOutcome::Lifecycle("identity_reset"),
    )
    .await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `meta/delete_identity` for the `desk` of one mob records the deletion
/// in that mob's console events, where it fails that `desk`'s pending
/// interaction, and leaves the other mob's `desk` as it was.
#[tokio::test]
async fn meta_delete_identity_acts_on_the_console_events_of_the_mob_it_names() {
    let wrong = a_request_for_one_mobs_desk(
        "meta/delete_identity",
        DeskBridge::Committing,
        json!({"identity": "desk"}),
        DeskOutcome::Lifecycle("identity_deleted"),
    )
    .await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---------------------------------------------------------------------------

// The memory realm of an agent memory request, arm by arm
// ---------------------------------------------------------------------------

/// Two mobs of one host on ONE agent memory store, each with a memory realm
/// of its own and an agent `desk`, and one memory remembered for each
/// `desk` under its mob's realm BY NAME (so that nothing here depends on
/// what an arm does with a request that names no realm).
struct TwoMemories {
    host: UnifiedRuntime,
    second: &'static UnifiedRuntime,
    launch_context: crate::IdentityFirstContext,
    second_mob: String,
    realm_of_the_second: String,
    /// The id of the memory of the launch mob's `desk`.
    of_the_launch: String,
    /// The id of the memory of the second mob's `desk`.
    of_the_second: String,
}

impl TwoMemories {
    async fn new(dir: &std::path::Path) -> Self {
        let (host, host_plane) = runtime_with_an_identity_plane("host").await;
        let (second, second_plane) = runtime_with_an_identity_plane("second").await;
        let second_mob = second.mob_id();
        let realm_of_the_second = format!("mob.{second_mob}");
        let store = Arc::new(
            crate::memory::SqliteAgentMemoryStore::open(dir.join("agent-memory"))
                .expect("agent memory store"),
        );
        let provider: Arc<dyn crate::identity_first::AgentMemoryProvider> = store;
        for (plane, realm) in [
            (&host_plane, "default".to_string()),
            (&second_plane, realm_of_the_second.clone()),
        ] {
            plane
                .identity_runtime
                .set_agent_memory(Some(
                    crate::identity_first::AgentMemoryRuntimeInjector::new(
                        Arc::clone(&provider),
                        crate::identity_first::AgentMemoryConfig {
                            realm,
                            ..Default::default()
                        },
                    ),
                ))
                .await;
        }
        register_member(&host_plane, "desk", 1).await;
        register_member(&second_plane, "desk", 1).await;
        let launch_context = launch_context(&host, &host_plane);
        let second = host.serve_a_second_mob_in_tests(second);
        let mut memories = Self {
            host,
            second,
            launch_context,
            second_mob,
            realm_of_the_second,
            of_the_launch: String::new(),
            of_the_second: String::new(),
        };
        memories.of_the_launch = memories
            .remembered("Of the launch mob", None, "default")
            .await;
        let realm = memories.realm_of_the_second.clone();
        let second_mob = memories.second_mob.clone();
        memories.of_the_second = memories
            .remembered("Of the second mob", Some(&second_mob), &realm)
            .await;
        memories
    }

    async fn ask(&self, method: &str, params: Value) -> Option<Value> {
        stdio_of_a_launch(&self.host, Some(&self.launch_context), method, params).await
    }

    /// Remember `title` for the `desk` of `mob` under `realm` by name: the
    /// id of the memory.
    async fn remembered(&self, title: &str, mob: Option<&str>, realm: &str) -> String {
        let mut params = json!({
            "identity": "desk",
            "realm": realm,
            "title": title,
            "body": "The desk keeps the standup notes.",
            "tags": ["standup"],
        });
        if let Some(mob) = mob {
            params["mob_id"] = json!(mob);
        }
        let answer = self.ask("meta/agent_memory/remember", params).await;
        answer
            .as_ref()
            .and_then(|answer| answer["result"]["memory_id"].as_str())
            .unwrap_or_else(|| panic!("remembering {title:?}: {answer:?}"))
            .to_string()
    }

    /// The titles the `desk` of `mob` recalls under `realm` by name, sorted.
    async fn titles(&self, mob: Option<&str>, realm: &str) -> Vec<String> {
        let mut params = json!({
            "identity": "desk",
            "realm": realm,
            "selection": "contextual",
            "query_terms": ["standup"],
            "max_entries": 8,
        });
        if let Some(mob) = mob {
            params["mob_id"] = json!(mob);
        }
        let answer = self.ask("meta/agent_memory/recall", params).await;
        let mut titles: Vec<String> = answer
            .as_ref()
            .and_then(|answer| answer["result"]["records"].as_array())
            .unwrap_or_else(|| panic!("recalling under {realm}: {answer:?}"))
            .iter()
            .filter_map(|record| record["title"].as_str())
            .map(str::to_string)
            .collect();
        titles.sort();
        titles
    }

    /// What each mob's `desk` remembers under its own realm by name: the
    /// launch mob's titles, then the second mob's.
    async fn both(&self) -> (Vec<String>, Vec<String>) {
        (
            self.titles(None, "default").await,
            self.titles(Some(&self.second_mob), &self.realm_of_the_second)
                .await,
        )
    }

    async fn shutdown(self) {
        self.host.shutdown().await;
        self.second.shutdown().await;
    }
}

/// `meta/agent_memory/forget` without a `realm` is for the memory realm of
/// the mob it names. For the second mob's `desk` it forgets that agent's
/// memory, and with the id of the launch mob's memory it forgets nothing:
/// the launch mob's agent of the same name keeps what it remembers.
#[tokio::test]
async fn an_agent_memory_forget_without_a_realm_is_for_the_memory_realm_of_the_mob_it_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let memories = TwoMemories::new(dir.path()).await;
    let mut wrong = Vec::new();

    let reaching_across = memories
        .ask(
            "meta/agent_memory/forget",
            json!({
                "mob_id": memories.second_mob,
                "identity": "desk",
                "memory_id": memories.of_the_launch,
            }),
        )
        .await;
    if reaching_across
        .as_ref()
        .is_some_and(|answer| answer["result"]["deleted"] == json!(true))
    {
        wrong.push(format!(
            "a forget for the second mob deleted the launch mob's memory: {reaching_across:?}"
        ));
    }
    let kept = memories.both().await;
    if kept
        != (
            vec!["Of the launch mob".to_string()],
            vec!["Of the second mob".to_string()],
        )
    {
        wrong.push(format!(
            "after a forget for the second mob with the launch mob's memory id: {kept:?}"
        ));
    }

    let forgotten = memories
        .ask(
            "meta/agent_memory/forget",
            json!({
                "mob_id": memories.second_mob,
                "identity": "desk",
                "memory_id": memories.of_the_second,
            }),
        )
        .await;
    if forgotten
        .as_ref()
        .is_none_or(|answer| answer["result"]["deleted"] != json!(true))
    {
        wrong.push(format!(
            "the second mob's own memory is not forgotten: {forgotten:?}"
        ));
    }
    let left = memories.both().await;
    if left != (vec!["Of the launch mob".to_string()], Vec::new()) {
        wrong.push(format!("after the second mob's forget: {left:?}"));
    }
    memories.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `meta/agent_memory/update` without a `realm` is for the memory realm of
/// the mob it names. For the second mob's `desk` it rewrites that agent's
/// memory, and with the id of the launch mob's memory it rewrites nothing:
/// the launch mob's agent of the same name keeps its memory as it was.
#[tokio::test]
async fn an_agent_memory_update_without_a_realm_is_for_the_memory_realm_of_the_mob_it_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let memories = TwoMemories::new(dir.path()).await;
    let update = |memory_id: &str, title: &str| {
        json!({
            "mob_id": memories.second_mob,
            "identity": "desk",
            "memory_id": memory_id,
            "title": title,
            "body": "The desk keeps the standup notes.",
            "tags": ["standup"],
        })
    };
    let mut wrong = Vec::new();

    let reaching_across = memories
        .ask(
            "meta/agent_memory/update",
            update(&memories.of_the_launch, "Rewritten across mobs"),
        )
        .await;
    if reaching_across
        .as_ref()
        .is_some_and(|answer| answer.get("error").is_none())
    {
        wrong.push(format!(
            "an update for the second mob took the launch mob's memory: {reaching_across:?}"
        ));
    }
    let kept = memories.both().await;
    if kept
        != (
            vec!["Of the launch mob".to_string()],
            vec!["Of the second mob".to_string()],
        )
    {
        wrong.push(format!(
            "after an update for the second mob with the launch mob's memory id: {kept:?}"
        ));
    }

    let updated = memories
        .ask(
            "meta/agent_memory/update",
            update(&memories.of_the_second, "Of the second mob, rewritten"),
        )
        .await;
    if updated
        .as_ref()
        .is_none_or(|answer| answer["result"]["supersedes"] != json!(memories.of_the_second))
    {
        wrong.push(format!(
            "the second mob's own memory is not rewritten: {updated:?}"
        ));
    }
    let after = memories.both().await;
    if after
        != (
            vec!["Of the launch mob".to_string()],
            vec!["Of the second mob, rewritten".to_string()],
        )
    {
        wrong.push(format!("after the second mob's update: {after:?}"));
    }
    memories.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `meta/agent_memory/manifest` without a `realm` is for the memory realm
/// of the mob it names: for the second mob's `desk` it lists that agent's
/// memory and not the launch mob's agent's of the same name, and for a
/// request that names no mob the launch mob's.
#[tokio::test]
async fn an_agent_memory_manifest_without_a_realm_is_for_the_memory_realm_of_the_mob_it_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let memories = TwoMemories::new(dir.path()).await;
    let mut wrong = Vec::new();
    for (mob, expected) in [
        (None, "Of the launch mob"),
        (Some(memories.second_mob.as_str()), "Of the second mob"),
    ] {
        let mut params = json!({"identity": "desk", "tier": "full"});
        if let Some(mob) = mob {
            params["mob_id"] = json!(mob);
        }
        let listed = memories.ask("meta/agent_memory/manifest", params).await;
        let titles: Option<Vec<&str>> = listed
            .as_ref()
            .and_then(|answer| answer["result"]["records"].as_array())
            .map(|records| {
                records
                    .iter()
                    .filter_map(|record| record["title"].as_str())
                    .collect()
            });
        if titles != Some(vec![expected]) {
            wrong.push(format!(
                "the manifest for {} lists {titles:?}, not only {expected:?}: {listed:?}",
                mob.unwrap_or("no mob")
            ));
        }
    }
    memories.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---------------------------------------------------------------------------
// The refusals of a mob on the stdio dispatch, told apart
// ---------------------------------------------------------------------------

/// The whole error of the stdio dispatch for a method that serves the
/// launch mob alone, asked for another mob the host serves: the refusal of
/// a mob id with both ids, the member `hosted`, and a text that says the
/// gateway hosts the mob.
fn serves_the_launch_mob_only(method: &str, requested: &str, launch_mob: &str) -> Value {
    json!({
        "code": -32602,
        "message": format!(
            "mob '{requested}' is hosted on this gateway, but {method} serves the launch mob \
             '{launch_mob}' only"
        ),
        "data": {
            "kind": "foreign_mob_target",
            "requested_mob_id": requested,
            "gateway_mob_id": launch_mob,
            "hosted": true,
        },
    })
}

/// A caller of the stdio dispatch tells the refusals of a mob apart. An id
/// the host does not have: the refusal of a mob id with both ids and
/// nothing else. A mob the host serves, on a method that serves the launch
/// mob alone: the same kind and ids with `hosted` and its own text (the
/// list of such methods is empty, so this row holds no case of it: the
/// module spawn and the live family have theirs in
/// `a_module_spawn_and_a_live_request_for_another_mob_are_refused`). A mob
/// the host serves whose entry holds no identity context, on a host whose
/// launch composed one: the same kind and ids with `hosted` and
/// `identity_context`. Each is told from the others by its data, not by
/// its text, and none has the code of a method the gateway does not have.
/// Each is compared whole.
#[tokio::test]
async fn the_refusals_of_a_mob_on_the_stdio_dispatch_are_told_apart() {
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let launch_context = launch_context(&host, &host_plane);
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let not_hosted = |method: &str| {
        (
            method.to_string(),
            ABSENT_MOB.to_string(),
            json!({
                "code": -32602,
                "message": format!(
                    "request targets mob '{ABSENT_MOB}' but this gateway serves '{launch_mob}'"
                ),
                "data": {
                    "kind": "foreign_mob_target",
                    "requested_mob_id": ABSENT_MOB,
                    "gateway_mob_id": launch_mob,
                },
            }),
        )
    };
    let mut cases = vec![
        not_hosted("meta/delivery/send"),
        not_hosted("meta/list_members"),
        (
            "meta/list_members".to_string(),
            second_mob.clone(),
            no_identity_context_for(&second_mob, &launch_mob),
        ),
    ];
    for method in crate::rpc::mob_addressing::STDIO_METHODS_SERVING_THE_LAUNCH_MOB_ONLY {
        cases.push((
            (*method).to_string(),
            second_mob.clone(),
            serves_the_launch_mob_only(method, &second_mob, &launch_mob),
        ));
    }
    let mut wrong = Vec::new();
    for (method, mob, expected) in cases {
        let answer = stdio_of_a_launch(
            &host,
            Some(&launch_context),
            &method,
            json!({"mob_id": mob}),
        )
        .await;
        if answer
            .as_ref()
            .is_none_or(|answer| answer["error"] != expected)
        {
            wrong.push(format!(
                "{method} for {mob}: {} and not {expected}",
                answer.as_ref().map_or_else(
                    || "no JSON".to_string(),
                    |answer| answer.get("error").unwrap_or(answer).to_string()
                )
            ));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `meta/capabilities` asked for a mob lists the methods the dispatch
/// serves for that mob: for a mob that is not the launch mob it lists no
/// method that serves the launch mob alone, and for the launch mob it lists
/// them as before.
#[tokio::test]
async fn the_capabilities_of_another_mob_list_no_method_that_serves_the_launch_mob_alone() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let mut wrong = Vec::new();
    for (mob, listed) in [
        (None, true),
        (Some(launch_mob.as_str()), true),
        (Some(second_mob.as_str()), false),
    ] {
        let mut params = json!({});
        if let Some(mob) = mob {
            params["mob_id"] = json!(mob);
        }
        let capabilities = stdio(&host, "meta/capabilities", params).await;
        let methods = capabilities
            .as_ref()
            .and_then(|answer| answer["result"]["methods"].as_array());
        for method in crate::rpc::mob_addressing::STDIO_METHODS_SERVING_THE_LAUNCH_MOB_ONLY {
            if methods.map(|methods| methods.contains(&json!(method))) != Some(listed) {
                wrong.push(format!(
                    "meta/capabilities for {}: lists {method}: not {listed}",
                    mob.unwrap_or("no mob")
                ));
            }
        }
        // What the dispatch serves for another mob stays listed: the
        // routing methods and the delivery methods.
        for method in [
            "meta/routing/resolve",
            "meta/delivery/send",
            "meta/delivery/history",
        ] {
            if !methods.is_some_and(|methods| methods.contains(&json!(method))) {
                wrong.push(format!(
                    "meta/capabilities for {}: does not list {method}: {capabilities:?}",
                    mob.unwrap_or("no mob")
                ));
            }
        }
        // The module form of `meta/spawn_member` serves the launch mob
        // alone: another mob is offered the member form only.
        let modes = capabilities.as_ref().map(|answer| {
            answer["result"]["runtime_capabilities"]["available_spawn_modes"].clone()
        });
        let offered = if listed {
            json!(["module", "profile"])
        } else {
            json!(["profile"])
        };
        if modes.as_ref() != Some(&offered) {
            wrong.push(format!(
                "meta/capabilities for {}: the spawn modes are {modes:?}, not {offered}",
                mob.unwrap_or("no mob")
            ));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The host has one module runtime and one live transport, each composed
/// for the launch mob. A module spawn (`meta/spawn_member` with
/// `module_id`) and a live method that name another mob the host serves
/// are refused as a method that serves the launch mob alone, before either
/// is reached; for the launch mob each is answered by its own service.
#[tokio::test]
async fn a_module_spawn_and_a_live_request_for_another_mob_are_refused() {
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let launch_context = launch_context(&host, &host_plane);
    let (second, _second_plane) = runtime_with_an_identity_plane("second").await;
    let second = host.serve_a_second_mob_in_tests(second);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let mut wrong = Vec::new();
    for (method, what, params) in [
        (
            "meta/spawn_member",
            "a module spawn (meta/spawn_member with module_id)",
            json!({"module_id": "a-module"}),
        ),
        (
            "meta/live/open",
            "meta/live/open",
            json!({"identity": "desk"}),
        ),
        ("meta/live/status", "meta/live/status", json!({})),
        ("meta/live/close", "meta/live/close", json!({})),
    ] {
        let mut for_the_second = params.clone();
        for_the_second["mob_id"] = json!(second_mob);
        let answer = stdio_of_a_launch(&host, Some(&launch_context), method, for_the_second).await;
        let expected = serves_the_launch_mob_only(what, &second_mob, &launch_mob);
        if answer
            .as_ref()
            .is_none_or(|answer| answer["error"] != expected)
        {
            wrong.push(format!(
                "{method} for the second mob: {} and not {expected}",
                answer.as_ref().map_or_else(
                    || "no JSON".to_string(),
                    |answer| answer.get("error").unwrap_or(answer).to_string()
                )
            ));
        }
        let answer = stdio_of_a_launch(&host, Some(&launch_context), method, params).await;
        if answer.as_ref().is_none_or(|answer| {
            answer["error"]["data"]["kind"] == json!("foreign_mob_target")
                || answer.get("error").is_none()
        }) {
            // Neither service is composed in this fixture: each answers
            // that for the launch mob, and never a refusal of its mob.
            wrong.push(format!("{method} for the launch mob: {answer:?}"));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A module spawn that names another mob is refused as a module spawn
/// whatever that mob holds: also for a mob whose entry holds no identity
/// context on a host whose launch composed one, which an identity method
/// refuses for want of that context. The module spawn is judged before the
/// mob is resolved and before its identity context is asked for.
#[tokio::test]
async fn a_module_spawn_for_a_mob_without_an_identity_context_is_refused_as_a_module_spawn() {
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let launch_context = launch_context(&host, &host_plane);
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let mut wrong = Vec::new();
    for (params, expected) in [
        (
            json!({"mob_id": second_mob, "module_id": "a-module"}),
            serves_the_launch_mob_only(
                "a module spawn (meta/spawn_member with module_id)",
                &second_mob,
                &launch_mob,
            ),
        ),
        // The member form of the same method works with the mob's identity
        // context and is refused for want of it.
        (
            json!({"mob_id": second_mob, "profile": "worker", "meerkat_id": "desk"}),
            no_identity_context_for(&second_mob, &launch_mob),
        ),
    ] {
        let answer = stdio_of_a_launch(
            &host,
            Some(&launch_context),
            "meta/spawn_member",
            params.clone(),
        )
        .await;
        if answer
            .as_ref()
            .is_none_or(|answer| answer["error"] != expected)
        {
            wrong.push(format!(
                "meta/spawn_member {params}: {} and not {expected}",
                answer.as_ref().map_or_else(
                    || "no JSON".to_string(),
                    |answer| answer.get("error").unwrap_or(answer).to_string()
                )
            ));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `meta/send_message` takes the console events of the mob it names once
/// its params are judged: a request for another mob that is refused for its
/// params makes no console event store for that mob.
#[tokio::test]
async fn a_send_message_refused_for_its_params_makes_no_console_events_for_its_mob() {
    let (host, host_plane) = runtime_with_an_identity_plane("host").await;
    let launch_context = launch_context(&host, &host_plane);
    let (second, _second_plane) = runtime_with_an_identity_plane("second").await;
    let second = host.serve_a_second_mob_in_tests(second);
    let second_mob = second.mob_id();
    let mut refused = Vec::new();
    for params in [
        json!({"mob_id": second_mob}),
        json!({"mob_id": second_mob, "member_id": "desk"}),
        json!({"mob_id": second_mob, "member_id": "desk", "message": "hello", "handling_mode": 7}),
    ] {
        let answer =
            stdio_of_a_launch(&host, Some(&launch_context), "meta/send_message", params).await;
        refused.push(
            answer
                .as_ref()
                .is_some_and(|answer| answer["error"]["code"] == json!(-32602)),
        );
    }
    let stores = host.console_events().mobs_with_a_store();
    host.shutdown().await;
    second.shutdown().await;
    assert_eq!(
        refused,
        [true, true, true],
        "each request is refused for its params"
    );
    assert!(
        stores.is_empty(),
        "a request refused for its params made a console event store: {stores:?}"
    );
}

/// The console events the dispatch's step answers for the launch mob are
/// the host's own store, the very object, whether the request names no mob
/// or the launch mob. For another mob they are that mob's own store of the
/// host's, the same one each time, and not the host's. The host's store
/// knows its launch mob: asked for it by its id, it answers itself and
/// makes no second store for it.
#[tokio::test]
async fn the_console_events_of_the_launch_mob_are_the_hosts_own_store() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_id = host.mob_handle().mob_id().clone();
    let second_id = second.mob_handle().mob_id().clone();
    let of_the_host = host.console_events();
    assert!(
        of_the_host
            .of_mob(Some(&launch_id))
            .is_the_store(&of_the_host),
        "asked for the launch mob by its id, the host's store answers itself"
    );
    assert!(
        of_the_host.mobs_with_a_store().is_empty(),
        "the launch mob has no second store: {:?}",
        of_the_host.mobs_with_a_store()
    );
    for named in [None, Some(&launch_id)] {
        let mob = host.resolve_mob(named).await.expect("the launch mob");
        assert!(
            super::console_events_of_a_served_mob(&mob).is_the_store(&of_the_host),
            "the console events of the launch mob, named {named:?}, are the host's store itself"
        );
    }
    {
        let mob = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob");
        let of_the_second = super::console_events_of_a_served_mob(&mob);
        assert!(
            !of_the_second.is_the_store(&of_the_host),
            "the console events of another mob are not the host's store"
        );
        assert!(
            of_the_second.is_the_store(&of_the_host.of_mob(Some(&second_id))),
            "the console events of another mob are its own store of the host's"
        );
    }
    host.shutdown().await;
    second.shutdown().await;
}

/// A GATING REQUEST NAMES THE MOB IT WAS MADE FOR. The gating owner is the
/// host's, one for every mob it serves, and a request the stdio door
/// evaluates has no origin. One made for a mob beside the launch mob names
/// that mob on every audit row its evaluation writes and on every later row
/// of its action (here the host's withdrawal), under `request_mob_id` in
/// the row's detail; one made for the launch mob, by no mob id or by the
/// launch mob's own, names none.
#[tokio::test]
async fn a_gating_request_names_the_mob_it_was_made_for_on_every_row_of_its_action() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    // A request for the mob `mob_id`, then the host's withdrawal of it: the
    // mob each audit row of its action names, in the order they were
    // written.
    async fn rows_of_a_request_for(
        host: &UnifiedRuntime,
        mob_id: Option<&str>,
    ) -> Vec<Option<String>> {
        let mut params = json!({
            "action": "publish_report",
            "actor_id": "host-job",
            "risk_tier": "r3",
            "approval_timeout_ms": 600_000,
        });
        if let Some(mob_id) = mob_id {
            params["mob_id"] = json!(mob_id);
        }
        let evaluated = stdio(host, "meta/gating/evaluate", params)
            .await
            .expect("the evaluation answers");
        let action_id = evaluated["result"]["action_id"]
            .as_str()
            .unwrap_or_else(|| panic!("an evaluated action: {evaluated}"))
            .to_string();
        let pending_id = evaluated["result"]["pending_id"]
            .as_str()
            .unwrap_or_else(|| panic!("the action waits for an approval: {evaluated}"))
            .to_string();
        host.withdraw_gating_action(crate::runtime::GatingWithdrawRequest {
            pending_id,
            reason: None,
        })
        .await
        .expect("the host withdraws its request");
        host.gating_audit_entries(usize::MAX)
            .await
            .into_iter()
            .filter(|entry| entry.action_id == action_id)
            .map(|entry| {
                entry
                    .detail
                    .get("request_mob_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect()
    }

    let of_the_second_mob = rows_of_a_request_for(&host, Some(second_mob.as_str())).await;
    let of_no_mob = rows_of_a_request_for(&host, None).await;
    let of_the_launch_mob_by_its_id = rows_of_a_request_for(&host, Some(launch_mob.as_str())).await;
    host.shutdown().await;
    second.shutdown().await;

    assert!(
        of_the_second_mob.len() >= 2,
        "an evaluation that waits and its withdrawal write a row each at least: \
         {of_the_second_mob:?}"
    );
    assert_eq!(
        of_the_second_mob,
        vec![Some(second_mob.clone()); of_the_second_mob.len()],
        "every row of a request made for the second mob names it"
    );
    for (rows, case) in [
        (&of_no_mob, "no mob"),
        (&of_the_launch_mob_by_its_id, "the launch mob by its id"),
    ] {
        assert!(rows.len() >= 2, "{case}: {rows:?}");
        assert_eq!(
            rows,
            &vec![None; rows.len()],
            "no row of a request made for {case} names a mob"
        );
    }
}

/// WHETHER A HOST SERVES A MOB BESIDE ITS LAUNCH MOB HAS THREE ANSWERS. A
/// door that reads a record of no known mob as the launch mob's asks the
/// host's resolver first, and "not known" is never read as "none". A host
/// that has no mob beside its launch mob serves its launch mob alone; a
/// resolver that serves a second mob says so; a runtime that shut down
/// cannot tell.
#[tokio::test]
async fn whether_a_host_serves_another_mob_has_three_answers() {
    use crate::unified_runtime::OtherMobsServed;

    let host = runtime().await;
    let second = runtime().await;
    let resolver = host.mob_resolver();
    assert_eq!(
        resolver.other_mobs_served().await,
        OtherMobsServed::LaunchMobAlone,
        "a host that has no mob beside its launch mob serves its launch mob alone"
    );
    assert_eq!(
        resolver
            .clone()
            .serving_a_second_mob_in_tests(&second.mob_resolver())
            .other_mobs_served()
            .await,
        OtherMobsServed::AnotherMob
    );
    host.shutdown().await;
    assert_eq!(
        resolver.other_mobs_served().await,
        OtherMobsServed::NotKnown,
        "a runtime that shut down cannot tell which mobs its host has"
    );
    second.shutdown().await;
}
