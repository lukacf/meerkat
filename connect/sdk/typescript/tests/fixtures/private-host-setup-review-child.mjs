import { createHash } from "node:crypto";
// Inert replies on inherited endpoints. No native authentication or grant owner.
import { createReadStream, createWriteStream } from "node:fs";
import { createInterface } from "node:readline";

const mode = process.argv[2] ?? "normal";
const commands = createReadStream("", { fd: 3, autoClose: true });
const responses = createWriteStream("", { fd: 4, autoClose: true });
commands.on("error", () => {});
responses.on("error", () => {});
const lines = createInterface({ input: commands });
const session = "00000000-0000-4000-8000-000000000002";
let pending;

function send(id, result) {
  responses.write(JSON.stringify({ version: 1, id, result }) + "\n");
}
function parent() {
  return [{
    root_authority: { kind: "human", id: "operator", qualification: { kind: "fixture" } },
    authority_namespace: "fixture-owner", authority_generation: 1,
    authority_incarnation: "00000000-0000-4000-8000-000000000001",
    grant_id: "fixture-child", issued_revision: 1,
  }];
}
function helperTargets() {
  return [["accounts", "toolkit_request_service_account"], ["results", "setup_report"]].map(([name, operation]) => {
    const lineage = parent()[0];
    lineage.grant_id = "fixture-" + name;
    const exact = values => ({ bound: { bound: "exact", values }, unresolved: [] });
    return {
      target: { kind: "registered_tool_bundle_operation", bundle: "meerkat_toolkit.setup_" + name, operation },
      parent: lineage,
      restrictions: {
        actions: exact([{ feature: "fixture", action: operation }]),
        resource_domains: exact([{ authority: lineage.root_authority, namespace: "original/" + name }]),
        processors: exact([{ processor: "principal", principal: lineage.root_authority }]),
        audiences: exact([{ audience: "principal", principal: lineage.root_authority }]),
        lifetime: { bound: { bound: "unrestricted" }, unresolved: [] },
        delegation_depth: { bound: { bound: "remaining", edges: 1 }, unresolved: [] },
      },
    };
  });
}
function targetSelection(chosen) {
  let rows = helperTargets();
  const row = rows[0];
  if (mode === "targets_empty") rows = [];
  else if (mode === "targets_all") {
    const [base, mcp] = structuredClone(rows);
    base.target = null; base.parent.grant_id = "fixture-base";
    mcp.target = { kind: "mcp_server", server: "selected-setup" };
    mcp.parent.grant_id = "fixture-mcp";
    rows = [base, mcp, ...rows];
  } else if (mode === "targets_null") rows = null;
  else if (mode === "targets_object") rows = row;
  else if (mode === "targets_limit") {
    // Stay within the frame limit so the target-count boundary is exercised.
    row.restrictions = Object.fromEntries(Object.keys(row.restrictions).map(key => [key, {}]));
    rows = Array.from({ length: 64 }, (_, i) => ({
      ...structuredClone(row), target: { kind: "mcp_server", server: "fixture-" + String(i).padStart(2, "0") },
    }));
  } else if (mode === "target_row_extra") row.permission = "all";
  else if (mode === "target_unknown") row.target = { kind: "future_target", operation: "setup_report" };
  else if (mode === "target_extra") row.target.fallback = true;
  else if (mode === "target_missing_name") delete row.target.operation;
  else if (mode === "target_empty_name") row.target.bundle = "";
  else if (mode === "target_nonstring_name") row.target.operation = 1;
  else if (mode === "target_duplicate") rows.push(structuredClone(row));
  else if (mode === "target_parent_array") row.parent = [row.parent];
  else if (mode === "target_parent_extra") row.parent.permission = "all";
  else if (mode === "target_parent_revision") row.parent.issued_revision = 0;
  else if (mode === "target_restrictions_null") row.restrictions = null;
  else if (mode === "target_restrictions_extra") row.restrictions.permission = "all";
  else if (mode === "target_dimension_scalar") row.restrictions.actions = true;
  chosen.helper_targets = rows;
}
function finalReply(review) {
  const controller = {
    state: "selected", route: review.original.route, purpose: "controller", lineage: parent(),
  };
  let outcome = {
    state: "setup_request_selected",
    selection: { original: { ...review.original }, session_id: session, parent: parent() },
  };
  const chosen = outcome.selection;
  if (mode.startsWith("state:")) outcome = { state: mode.slice("state:".length) };
  else if (mode.startsWith("target_") || mode.startsWith("targets_")) targetSelection(chosen);
  else if (mode === "outcome_extra") outcome.admitted = true;
  else if (mode === "selection_extra") chosen.turn_ticket = session;
  else if (mode === "missing_parent") delete chosen.parent;
  else if (mode === "wrong_route") chosen.original.route = "another-route";
  else if (mode === "wrong_original") chosen.original.original = "another-original";
  else if (mode === "wrong_digest") chosen.original.event_sha256 = "9".repeat(64);
  else if (mode === "invalid_session") chosen.session_id = "not-a-uuid";
  else if (mode === "nonstring_session") chosen.session_id = 7;
  else if (mode === "empty_parent") chosen.parent = [];
  else if (mode === "multiple_parents") chosen.parent.push(structuredClone(chosen.parent[0]));
  else if (mode === "parent_object") chosen.parent = chosen.parent[0];
  else if (mode === "parent_extra") chosen.parent[0].permission = "all";
  else if (mode === "root_array") chosen.parent[0].root_authority = [];
  else if (mode === "invalid_incarnation") chosen.parent[0].authority_incarnation = "bad";
  else if (mode === "boolean_generation") chosen.parent[0].authority_generation = true;
  else if (mode === "zero_revision") chosen.parent[0].issued_revision = 0;
  else if (mode === "unknown_state") outcome.state = "admitted";
  else if (mode === "controller_shape") outcome = controller;
  else if (mode === "nonselected_extra") outcome = { state: "denied", selection: chosen };
  if (!review.setup && mode !== "controller_setup_state") {
    outcome = controller;
    if (mode === "controller_setup_purpose") outcome.purpose = "setup_request";
  }
  return {
    kind: review.setup && mode !== "wrong_reply_kind"
      ? "setup_request_reviewed" : "controller_parent_reviewed",
    outcome,
  };
}
// Exact new service-parent envelope on this same inert child connection.
let serviceExchange = null;
let serviceNumber = 40;
let serviceNextRefused = false, serviceNextPending;
let serviceSubmit;
let serviceRetire;
function serviceProposal() {
  const proposal = {
    operator: { kind: "fixture" }, rule: { kind: "application_policy_tasks", routes: ["member-route"] },
    issuance_id: "parent-1", deployment: "deployment-1", source_policy_sha256: "a".repeat(64),
    original: { route: "member-route", original: "original-1", event_sha256: "5".repeat(64) },
    acquisition: { realm: "realm-1", owner: "owner-1", id: "acquisition-1" },
    instruction_sha256: "b".repeat(64), expires_in_ms: 60000,
    application: { kind: "fixture" }, represented_subject: null,
    decision: { route: "member-route", decision: "decision-1" },
    connector_selection: { server: "fixture", connector: null }, restrictions: {},
  };
  if (mode.startsWith("service_tool_policy")) {
    const canonical = '{"fixture":"' + (mode === "service_tool_policy_large" ? "x".repeat(100_000) : "complete policy including preserved permissions") + '"}\n';
    proposal.tool_policy = { provider_id: "application", policy_id: "tools", member: "assistant:main",
      selection: { tools: [{ name: "get_top_hackernews_stories", consequence: "r0" }], catalog: "r0" },
      catalog_session: session, prior: { revision: 1, digest: "sha256:" + "a".repeat(64) },
      candidate: { revision: 2, digest: "sha256:" + createHash("sha256").update(canonical).digest("hex") },
      candidate_canonical_json: canonical, lifetime: "until_superseded" };
    if (mode === "service_tool_policy_bad_digest") proposal.tool_policy.candidate.digest = "sha256:" + "0".repeat(64);
    if (mode === "service_tool_policy_bare_digest") proposal.tool_policy.candidate.digest = proposal.tool_policy.candidate.digest.slice(7);
    if (mode === "service_tool_policy_revision_negative") proposal.tool_policy.candidate.revision = -1;
    if (mode === "service_tool_policy_revision_string") proposal.tool_policy.candidate.revision = "2";
    if (mode === "service_tool_policy_revision_unsafe") proposal.tool_policy.candidate.revision = 2 ** 53;
  }
  return proposal;
}
function serviceObservation() {
  const refused = !serviceExchange?.decision || !["authorized", "authorized_with_tool_policy"].includes(serviceExchange.decision.kind);
  const reason = serviceExchange?.decision?.kind === "requires_human" ? "requires_human"
    : serviceExchange?.decision?.kind === "unavailable" ? "unavailable" : "denied";
  const value = { outcome: refused ? "refused_before_entry" : mode === "service_parent_uncertain" ? "uncertain" : "failed_after_effects",
    custody: "joined", reason: refused ? reason : "not_settled", parent_issuance_id: "parent-1",
    parent_issued: !refused, child: null, source: null };
  if (mode === "service_tool_policy" || mode === "service_tool_policy_large") {
    value.tool_policy = { proposal_sha256: serviceExchange.decision.tool_policy_sha256,
      candidate: serviceProposal().tool_policy.candidate, native: "accepted", retained_as_current: false };
  }
  if (mode === "service_parent_running") value.custody = "running";
  if (mode === "service_parent_wrong_parent") value.parent_issuance_id = "another-parent";
  return value;
}
function serviceReply(id, suffix, observation) {
  send(id, { kind: "service_parent_administration_" + suffix, exchange_id: serviceExchange.id,
    ...(observation ? { observation } : {}) });
}
function serviceCommand(id, command) {
  if (command.kind === "next_service_parent_administration") {
    if (mode === "service_parent_next_unavailable" && !serviceNextRefused) {
      serviceNextRefused = true;
      send(id, { kind: "error", error: "unavailable" });
      return true;
    }
    if (mode === "service_parent_next_pending") {
      if (serviceNextPending !== undefined) throw new Error("Next timeout sent a second query");
      serviceNextPending = id;
      return true;
    }
    serviceExchange ??= { id: ++serviceNumber };
    const result = { kind: "service_parent_administration_request", exchange_id: serviceExchange.id,
      proposal_sha256: "d".repeat(64), proposal: serviceProposal() };
    if (mode === "service_tool_policy_large") {
      const bytes = Buffer.byteLength(JSON.stringify({ version: 1, id, result })) + 1;
      if (!(65_536 < bytes && bytes < 1_048_576)) throw new Error("large complete proposal is outside fixture frame bounds");
    }
    send(id, result);
  } else if (command.kind === "submit_service_parent_administration") {
    if (command.exchange_id !== serviceExchange?.id || serviceExchange.decision) throw new Error("duplicate or wrong service answer");
    serviceExchange.decision = command.decision;
    if (mode === "service_parent_eof") responses.end();
    else if (mode === "service_parent_lost_ack") serviceSubmit = id;
    else serviceReply(id, "submitted");
  } else if (command.kind === "result_service_parent_administration") {
    if (command.exchange_id !== serviceExchange?.id) throw new Error("wrong service result");
    if (serviceSubmit) { serviceReply(serviceSubmit, "submitted"); serviceSubmit = undefined; }
    serviceReply(id, "result", serviceObservation());
  } else if (command.kind === "retire_service_parent_administration") {
    if (command.exchange_id !== serviceExchange?.id) throw new Error("wrong service retirement");
    if (mode === "service_parent_retire_pending") serviceRetire = id;
    else { serviceReply(id, "retired", serviceObservation()); serviceExchange = null; }
  } else return false;
  return true;
}

for await (const line of lines) {
  const { id, command } = JSON.parse(line);
  process.stdout.write(JSON.stringify({ id, command }) + "\n");
  if (serviceCommand(id, command)) continue;
  if (command.kind === "enroll_console_login") {
    send(id, { kind: "console_login_enrolled", enrollment_id: id });
  } else if (command.kind === "retire_console_login") {
    send(id, { kind: "console_login_retired", enrollment_id: command.enrollment_id,
      observation: { fenced: true, reservations_drained: true, uncertain: false } });
  } else if (command.kind === "bootstrap") {
    send(id, { kind: "bootstrap_accepted" });
  } else if (["review_remote_setup_request", "review_remote_controller_parent"].includes(command.kind)) {
    pending = {
      id, original: command.submission.original,
      setup: command.kind === "review_remote_setup_request", answered: false,
    };
    if (mode === "lost_response") responses.end();
  } else if (["observe_controller_review", "answer_controller_review", "retire_controller_review"].includes(command.kind)) {
    if (!pending || command.review_id !== pending.id) {
      send(id, { kind: "error", error: "wrong_owner" });
      continue;
    }
    const reviewId = mode === "wrong_review_id" ? command.review_id + 1 : command.review_id;
    if (command.kind === "retire_controller_review") {
      // The timeout case gets its selected reply only after the caller retires.
      send(pending.id, mode === "late_selected" ? finalReply(pending) : {
        kind: pending.setup ? "setup_request_reviewed" : "controller_parent_reviewed",
        outcome: { state: "unavailable" },
      });
      pending = undefined;
      send(id, { kind: "controller_review", review_id: reviewId, status: { state: "retired" } });
    } else {
      if (command.kind === "answer_controller_review") pending.answered = true;
      const status = pending.answered ? { state: "answer_forwarded" } : {
        state: "prompt", prompt: { kind: "approval", approval_id: "approval-1", summary: "Setup request" },
      };
      send(id, { kind: "controller_review", review_id: reviewId, status });
    }
  } else if (command.kind === "open_component") {
    if (serviceNextPending !== undefined) {
      serviceExchange = { id: ++serviceNumber };
      send(serviceNextPending, { kind: "service_parent_administration_request", exchange_id: serviceExchange.id,
        proposal_sha256: "d".repeat(64), proposal: serviceProposal() });
      serviceNextPending = undefined;
    }
    if (serviceRetire) { serviceReply(serviceRetire, "retired", serviceObservation()); serviceRetire = undefined; serviceExchange = null; }
    if (pending && mode !== "late_selected") {
      send(pending.id, finalReply(pending));
      pending = undefined;
    }
    send(id, { kind: "component", observation: {
      incarnation: session, component: command.component, revision: 1, state: "open",
    } });
  } else {
    send(id, { kind: "error", error: "unavailable" });
  }
}
responses.end();
