// Inert protocol fixture. No native runtime, provider, authentication or grant.
import { createReadStream, createWriteStream } from "node:fs";
import { createHash } from "node:crypto";
import { createInterface } from "node:readline";
const mode = process.argv[2];
const commands = createReadStream("", { fd: 3, autoClose: true });
const responses = createWriteStream("", { fd: 4, autoClose: true });
commands.on("error", () => {}); responses.on("error", () => {});
const lines = createInterface({ input: commands });
const handle = { incarnation: "00000000-0000-0000-0000-000000000001", close_id: "00000000-0000-0000-0000-000000000002", scope: { kind: "activation" } };
let pendingJoin;
let pendingSubmission;
let pendingReview;
let reviewForwarded = false;
const timers = new Set();
function send(id, result) { responses.write(JSON.stringify({ version: 1, id, result }) + "\n"); }
function sorted(value) {
  if (Array.isArray(value)) return value.map(sorted);
  if (value && typeof value === "object") return Object.fromEntries(Object.keys(value).sort().map(k => [k, sorted(value[k])]));
  return value;
}
process.stdout.write("fixture-ready\n");
for await (const line of lines) {
  const { id, command } = JSON.parse(line);
  process.stdout.write(JSON.stringify({ id, command }) + "\n");
  if (command.kind === "prepare_application") {
    if (mode === "prepare-refused") send(id, { kind: "error", error: "denied" });
    else {
      const result = { kind: "application_prepared", manifest: {
        path: mode === "prepare-malformed" ? "relative.json" : "/private/completed.json", sha256: "7".repeat(64) } };
      if (mode === "prepare-late") {
        const timer = setTimeout(() => { timers.delete(timer); send(id, result); }, 70); timers.add(timer);
      } else send(id, result);
    }
    continue;
  }
  if (["bootstrap", "replace_application"].includes(command.kind)) {
    if (mode === "bootstrap-refused") send(id, { kind: "error", error: "denied" });
    else if (!["bootstrap-pending", "prepare-adopt-pending"].includes(mode)) send(id, { kind: "bootstrap_accepted" });
    continue;
  }
  if (command.kind === "enroll_console_login") {
    const reply = { kind: "console_login_enrolled", enrollment_id: mode === "console-wrong-enrollment" ? id + 1 : id };
    if (mode === "console-late") { const timer = setTimeout(() => { timers.delete(timer); send(id, reply); }, 70); timers.add(timer); }
    else send(id, reply);
    continue;
  }
  if (command.kind === "retire_console_login") {
    if (mode !== "console-retire-pending") send(id, { kind: "console_login_retired", enrollment_id: command.enrollment_id,
      observation: { fenced: true, reservations_drained: true,
        uncertain: mode === "console-retire-malformed" ? 0 : mode === "console-late" } });
    continue;
  }
  if (["console-full", "console-retire-pending"].includes(mode) && command.kind === "open_component") continue;
  if (mode === "unknown-error") { send(id, { kind: "error", error: "not_a_native_code" }); continue; }
  if (mode === "malformed-error") { send(id, { kind: "error", error: 7 }); continue; }
  if (mode === "duplicate-key") { responses.write(`{"version":1,"id":${id},"result":{"kind":"error","error":"denied","error":"unavailable"}}\n`); continue; }
  if (mode === "numeric-version") { responses.write(`{"version":1.0,"id":${id},"result":{"kind":"error","error":"denied"}}\n`); continue; }
  if (mode === "unknown-id") { send(id + 1, { kind: "error", error: "denied" }); continue; }
  if (mode === "bom") { responses.write(Buffer.concat([Buffer.from([239,187,191]), Buffer.from(JSON.stringify({ version: 1, id, result: { kind: "error", error: "denied" } }) + "\n")])); continue; }
  if (mode === "invalid-utf8") { responses.write(Buffer.from([255, 10])); continue; }
  if (mode === "oversize") { responses.write(" ".repeat(1_048_576) + "\n"); continue; }
  if (mode === "partial-eof") { responses.end('{"version":1'); continue; }
  if (command.kind === "review_remote_controller_parent") {
    pendingReview = { id, route: command.submission.original.route, decision: "approve" };
    continue;
  }
  if (["observe_controller_review", "answer_controller_review", "retire_controller_review"].includes(command.kind)) {
    if (!pendingReview || command.review_id !== pendingReview.id) { send(id, { kind: "error", error: "denied" }); continue; }
    const reviewId = mode === "review-wrong-id" ? command.review_id + 100 : command.review_id;
    let status;
    if (command.kind === "observe_controller_review") {
      status = reviewForwarded ? { state: "answer_forwarded" } : { state: "prompt", prompt: { kind: "approval", approval_id: "approval-1", title: "Review access" } };
    } else if (command.kind === "answer_controller_review") {
      if (command.approval_id !== "approval-1" || reviewForwarded) { send(id, { kind: "error", error: "denied" }); continue; }
      reviewForwarded = true; pendingReview.decision = command.decision;
      status = { state: "answer_forwarded" };
    } else status = { state: "retired" };
    send(id, { kind: "controller_review", review_id: reviewId, status });
    if (command.kind === "retire_controller_review") {
      send(pendingReview.id, { kind: "controller_parent_reviewed", outcome: { state: "unavailable" } });
      pendingReview = undefined;
    }
    continue;
  }
  if (command.kind === "retain_authenticated_event") {
    if (mode === "refused" || mode.startsWith("refused:")) {
      send(id, { kind: "error", error: mode === "refused" ? "source_conflict" : mode.slice("refused:".length) }); continue;
    }
    const event = command.event;
    const original = { route: event.route, original: event.original,
      event_sha256: createHash("sha256").update(JSON.stringify(sorted(event))).digest("hex") };
    if (mode === "wrong-reference") original.original = "another-original";
    const reply = { kind: "authenticated_event_retained", original,
      state: mode === "wrong-retain-state" ? "admitted" : "retained_pending_native_admission" };
    if (mode === "late") { const timer = setTimeout(() => { timers.delete(timer); send(id, reply); }, 70); timers.add(timer); }
    else send(id, reply);
    continue;
  }
  if (command.kind === "submit_authenticated_original") {
    if (mode === "submit-refused") { send(id, { kind: "error", error: "source_conflict" }); continue; }
    const original = command.submission.original;
    const outcome = { state: "admitted", session_id: "native-session", input_id: `input-${original.original}`,
      deduplicated: false, turn_ticket: "00000000-0000-0000-0000-000000000003", source_settlement: "fresh_acceptance_promoted" };
    if (mode === "submit-deduplicated") { outcome.deduplicated = true; outcome.turn_ticket = null; outcome.source_settlement = "retry_withdrawn"; }
    if (mode === "submit-publication-failed") outcome.source_settlement = "post_admission_publication_failed";
    const reply = { kind: "original_submitted", original, outcome };
    if (mode === "submit-unresolved") reply.outcome = { state: "admission_unresolved" };
    if (mode === "submit-unavailable") reply.outcome = { state: "native_outcome_unavailable" };
    if (mode === "submit-wrong-original") reply.original = { ...original, original: "another-original" };
    if (mode === "submit-wrong-digest") reply.original = { ...original, event_sha256: "9".repeat(64) };
    if (mode === "submit-wrong-state") outcome.state = "completed";
    if (mode === "submit-extra-completion") outcome.completed = true;
    if (mode === "submit-wrong-settlement") outcome.source_settlement = "completed";
    if (mode === "submit-wrong-boolean") outcome.deduplicated = 1;
    if (mode === "submit-wrong-ticket") outcome.turn_ticket = "not-a-uuid";
    if (mode === "submit-invalid-session") outcome.session_id = "";
    if (mode === "submit-unresolved-ticket") reply.outcome = { state: "admission_unresolved", turn_ticket: outcome.turn_ticket };
    if (mode === "submit-late") { const timer = setTimeout(() => { timers.delete(timer); send(id, reply); }, 70); timers.add(timer); }
    else if (mode === "submit-concurrent") {
      if (pendingSubmission === undefined) pendingSubmission = { id, reply };
      else { send(id, reply); send(pendingSubmission.id, pendingSubmission.reply); pendingSubmission = undefined; }
    } else send(id, reply);
    continue;
  }
  if (command.kind === "cancel_original_operation") {
    const reply = { kind: "original_operation_cancelled", original: command.original,
      state: mode === "wrong-cancel-state" ? "settled" : "operations_cancelled" };
    if (mode === "legacy-cancel-state") reply.state = "operation_grant_revoked";
    if (mode === "cancel-cleanup-pending") reply.state = "operations_cancelled_cleanup_pending";
    if (mode === "cancel-wrong-original") reply.original = { ...command.original, original: "another-original" };
    if (mode === "cancel-late") { const timer = setTimeout(() => { timers.delete(timer); send(id, reply); }, 70); timers.add(timer); }
    else send(id, reply);
    continue;
  }
  if (command.kind === "open_component") {
    if (pendingReview && reviewForwarded) {
      let outcome = { state: "selected", route: pendingReview.route, purpose: "controller", lineage: [{
        root_authority: { kind: "human", id: "operator", qualification: { kind: "test-fixture" } },
        authority_namespace: "fixture-owner", authority_generation: 1,
        authority_incarnation: "00000000-0000-4000-8000-000000000001", grant_id: "fixture-grant", issued_revision: 1,
      }] };
      if (pendingReview.decision === "deny") outcome = { state: "denied" };
      if (mode === "review-wrong-route") outcome.route = "another/route";
      if (mode === "review-wrong-purpose") outcome.purpose = "operations";
      if (mode === "review-empty-lineage") outcome.lineage = [];
      send(pendingReview.id, { kind: "controller_parent_reviewed", outcome });
      pendingReview = undefined;
    }
    send(id, { kind: "component", observation: { incarnation: handle.incarnation, component: command.component, revision: 3, state: "open" } });
    if (mode === "duplicate-id") send(id, { kind: "error", error: "denied" });
    continue;
  }
  if (command.kind === "join_close" && mode === "concurrent") { pendingJoin = id; continue; }
  const selected = command.handle ?? { ...handle, scope: command.kind === "begin_close_component" ? { kind: "component", component: command.component } : handle.scope };
  send(id, { kind: "close", observation: { handle: selected, state: "closing", failure: null } });
  if (pendingJoin !== undefined) { send(pendingJoin, { kind: "close", observation: { handle, state: "settled", failure: null } }); pendingJoin = undefined; }
}
for (const timer of timers) clearTimeout(timer);
responses.end();
