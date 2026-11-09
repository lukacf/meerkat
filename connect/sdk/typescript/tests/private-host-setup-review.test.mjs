// Inherited-endpoint protocol controls only. The child sends inert native-shaped
// data; these tests cannot establish native authority, admission or completion.
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { PassThrough, Writable } from "node:stream";
import {
  PrivateHostClient, HostControlError, HostTransportError, HostObservationTimeout, HostRefusedError,
} from "../dist/private-host.js";

const artifact = { path: "/private/selected.json", sha256: "1".repeat(64) };
const bootstrap = {
  deployment_root: "/private/realm", activation: artifact,
  policy_manifest: artifact, local_policy: artifact,
};
const original = { route: "telegram/main", original: "request-123", event_sha256: "5".repeat(64) };
const evidence = { resource_id: "transport", revision: "4", sha256: "c".repeat(64) };
const login = { scope_id: "paired-login", expires_at_ms: 1900000000000 };
const content = "Latest messages.\nRäksmörgås";
function selected(mode = "normal") {
  const outcome = {
    state: "setup_request_selected",
    selection: {
      original, session_id: "00000000-0000-4000-8000-000000000002",
      parent: [{
        root_authority: { kind: "human", id: "operator", qualification: { kind: "fixture" } },
        authority_namespace: "fixture-owner", authority_generation: 1,
        authority_incarnation: "00000000-0000-4000-8000-000000000001",
        grant_id: "fixture-child", issued_revision: 1,
      }],
    },
  };
  if (mode !== "normal") {
    let rows = mode === "targets_empty" ? [] : helperTargets();
    if (mode === "targets_all") {
      const [base, mcp] = structuredClone(rows);
      base.target = null; base.parent.grant_id = "fixture-base";
      mcp.target = { kind: "mcp_server", server: "selected-setup" };
      mcp.parent.grant_id = "fixture-mcp";
      rows = [base, mcp, ...rows];
    }
    outcome.selection.helper_targets = rows;
  }
  return outcome;
}

function helperTargets() {
  // Native-shaped observations, not issued grants. Distinct parents and bounds
  // must survive the reply without merging report and account authority.
  return [["accounts", "toolkit_request_service_account"], ["results", "setup_report"]].map(([name, operation]) => {
    const parent = structuredClone(selected().selection.parent[0]);
    parent.grant_id = "fixture-" + name;
    const exact = values => ({ bound: { bound: "exact", values }, unresolved: [] });
    return {
      target: { kind: "registered_tool_bundle_operation", bundle: "meerkat_toolkit.setup_" + name, operation },
      parent,
      restrictions: {
        actions: exact([{ feature: "fixture", action: operation }]),
        resource_domains: exact([{ authority: parent.root_authority, namespace: "original/" + name }]),
        processors: exact([{ processor: "principal", principal: parent.root_authority }]),
        audiences: exact([{ audience: "principal", principal: parent.root_authority }]),
        lifetime: { bound: { bound: "unrestricted" }, unresolved: [] },
        delegation_depth: { bound: { bound: "remaining", edges: 1 }, unresolved: [] },
      },
    };
  });
}

function fixture(t, mode = "normal") {
  const child = spawn(process.execPath, [
    new URL("./fixtures/private-host-setup-review-child.mjs", import.meta.url).pathname, mode,
  ], { stdio: ["ignore", "pipe", "pipe", "pipe", "pipe"], env: { PATH: "/usr/bin:/bin" } });
  let stdout = "", stderr = "", forced = false;
  child.stdout.setEncoding("utf8"); child.stderr.setEncoding("utf8");
  child.stdout.on("data", chunk => { stdout += chunk; });
  child.stderr.on("data", chunk => { stderr += chunk; });
  const closed = new Promise((resolve, reject) => {
    child.once("close", (code, signal) => resolve({ code, signal }));
    child.once("error", reject);
  });
  const client = new PrivateHostClient(child.stdio[3], child.stdio[4], { timeoutMs: 2000 });
  let finishing;
  function finish() {
    return finishing ??= (async () => {
      const timer = setTimeout(() => { forced = true; child.kill("SIGKILL"); }, 2000);
      let exit;
      try { await client.close(); exit = await closed; }
      finally { clearTimeout(timer); }
      assert.equal(forced, false, "forced cleanup is not a passing lifecycle control");
      assert.deepEqual(exit, { code: 0, signal: null });
      assert.equal(stderr, "");
      assert.equal(child.stdio[3].destroyed, true);
      assert.equal(child.stdio[4].destroyed, true);
      t.diagnostic(JSON.stringify({ childPid: child.pid, exit, forcedCleanup: forced }));
      // Child close, not a sleep, establishes the complete observed command log.
      return stdout.trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
    })();
  }
  t.after(finish);
  return { client, finish };
}
function begin(client, options) {
  return client.beginRemoteSetupRequestReview(original, content, evidence, login, options);
}

for (const mode of ["normal", "targets_empty", "targets_correlated", "targets_all"]) {
  test("setup review writes the exact command and reuses retained review controls: " + mode, async t => {
    const { client, finish } = fixture(t, mode);
    await client.bootstrap(bootstrap);
    const reference = structuredClone(original);
    const review = client.beginRemoteSetupRequestReview(reference, content, evidence, login);
    reference.original = "mutated-after-call";
    let completed = false;
    void review.outcome.then(() => { completed = true; });
    assert.equal(review.reviewId, 2);
    const prompt = await client.observeControllerReview(review.reviewId);
    assert.deepEqual(prompt, {
      state: "prompt",
      prompt: { kind: "approval", approval_id: "approval-1", summary: "Setup request" },
    });
    assert.deepEqual(await client.answerControllerReview(review.reviewId, "approval-1", "approve"), {
      state: "answer_forwarded",
    });
    assert.equal(completed, false, "answer forwarding is not the native selection");
    assert.deepEqual(await client.observeControllerReview(review.reviewId), { state: "answer_forwarded" });
    await client.openComponent("telegram");
    assert.deepEqual(await review.outcome, selected(mode));
    assert.deepEqual(await review.outcome, selected(mode));
    assert.equal((await client.openComponent("telegram")).state, "open");
    const commands = await finish();
    assert.deepEqual(commands.map(v => v.id), [1, 2, 3, 4, 5, 6, 7]);
    assert.deepEqual(commands.map(v => v.command.kind), [
      "bootstrap", "review_remote_setup_request", "observe_controller_review",
      "answer_controller_review", "observe_controller_review", "open_component", "open_component",
    ]);
    assert.deepEqual(commands[1].command, {
      kind: "review_remote_setup_request",
      submission: { original, content, transport_authentication: evidence }, login,
    });
    assert.deepEqual(commands[3].command, {
      kind: "answer_controller_review", review_id: 2, approval_id: "approval-1", decision: "approve",
    });
  });
}

for (const state of ["denied", "unavailable", "pending_reconciliation"]) {
  test("setup review preserves " + state + " without granting or submitting work", async t => {
    const { client, finish } = fixture(t, "state:" + state);
    await client.bootstrap(bootstrap);
    const review = begin(client);
    await client.openComponent("telegram");
    assert.deepEqual(await review.outcome, { state });
    assert.deepEqual(await review.outcome, { state });
    assert.deepEqual((await finish()).map(v => v.command.kind), [
      "bootstrap", "review_remote_setup_request", "open_component",
    ]);
  });
}

const malformed = [
  "outcome_extra", "selection_extra", "missing_parent", "wrong_route", "wrong_original",
  "wrong_digest", "invalid_session", "nonstring_session", "empty_parent", "multiple_parents",
  "parent_object", "parent_extra", "root_array", "invalid_incarnation", "boolean_generation",
  "zero_revision", "unknown_state", "controller_shape", "wrong_reply_kind", "nonselected_extra",
  "targets_null", "targets_object", "targets_limit", "target_row_extra", "target_unknown",
  "target_extra", "target_missing_name", "target_empty_name", "target_nonstring_name",
  "target_duplicate", "target_parent_array", "target_parent_extra", "target_parent_revision",
  "target_restrictions_null", "target_restrictions_extra", "target_dimension_scalar",
];
for (const mode of malformed) {
  test("setup review rejects " + mode + " and poisons the shared transport", async t => {
    const { client, finish } = fixture(t, mode);
    await client.bootstrap(bootstrap);
    const review = begin(client);
    const opening = client.openComponent("telegram");
    const results = await Promise.allSettled([review.outcome, opening]);
    for (const result of results) {
      assert.equal(result.status, "rejected");
      assert.ok(result.reason instanceof HostTransportError);
    }
    assert.throws(() => client.observeControllerReview(review.reviewId), HostTransportError);
    assert.deepEqual((await finish()).map(v => v.command.kind), [
      "bootstrap", "review_remote_setup_request", "open_component",
    ]);
  });
}

for (const mode of ["controller_setup_state", "controller_setup_purpose"]) {
  test("existing controller decoder rejects " + mode, async t => {
    const { client, finish } = fixture(t, mode);
    await client.bootstrap(bootstrap);
    const review = client.beginRemoteControllerReview(original, content, evidence, login);
    const results = await Promise.allSettled([review.outcome, client.openComponent("telegram")]);
    for (const result of results) {
      assert.equal(result.status, "rejected");
      assert.ok(result.reason instanceof HostTransportError);
    }
    assert.deepEqual((await finish()).map(v => v.command.kind), [
      "bootstrap", "review_remote_controller_parent", "open_component",
    ]);
  });
}

test("setup review rejects another review ID on the shared control reply", async t => {
  const { client, finish } = fixture(t, "wrong_review_id");
  await client.bootstrap(bootstrap);
  const review = begin(client);
  await assert.rejects(client.observeControllerReview(review.reviewId), HostTransportError);
  await assert.rejects(review.outcome, HostTransportError);
  assert.deepEqual((await finish()).map(v => v.command.kind), [
    "bootstrap", "review_remote_setup_request", "observe_controller_review",
  ]);
});

test("retiring setup review uses the original ID and leaves the channel usable", async t => {
  const { client, finish } = fixture(t);
  await client.bootstrap(bootstrap);
  const review = begin(client);
  assert.deepEqual(await client.retireControllerReview(review.reviewId), { state: "retired" });
  assert.deepEqual(await review.outcome, { state: "unavailable" });
  assert.equal((await client.openComponent("telegram")).state, "open");
  const commands = await finish();
  assert.deepEqual(commands.map(v => v.command.kind), [
    "bootstrap", "review_remote_setup_request", "retire_controller_review", "open_component",
  ]);
  assert.deepEqual(commands[2].command, { kind: "retire_controller_review", review_id: review.reviewId });
});

test("setup timeout drains late selected data but never resends or replaces the outcome", async t => {
  const { client, finish } = fixture(t, "late_selected");
  await client.bootstrap(bootstrap);
  const review = begin(client, { timeoutMs: 100 });
  // This reply confirms the child saw the original request before the timeout.
  assert.equal((await client.observeControllerReview(review.reviewId)).state, "prompt");
  let firstError;
  await assert.rejects(review.outcome, error => {
    assert.ok(error instanceof HostObservationTimeout);
    assert.equal(error instanceof HostTransportError, false);
    firstError = error;
    return true;
  });
  await assert.rejects(review.outcome, error => error === firstError);
  assert.deepEqual(await client.retireControllerReview(review.reviewId), { state: "retired" });
  await assert.rejects(review.outcome, error => error === firstError);
  assert.equal((await client.openComponent("telegram")).state, "open");
  const commands = await finish();
  assert.deepEqual(commands.map(v => v.command.kind), [
    "bootstrap", "review_remote_setup_request", "observe_controller_review",
    "retire_controller_review", "open_component",
  ]);
});

test("setup response EOF stays unknown transport loss and never triggers an implicit retry", async t => {
  const { client, finish } = fixture(t, "lost_response");
  await client.bootstrap(bootstrap);
  const review = begin(client);
  let firstError;
  await assert.rejects(review.outcome, error => {
    assert.ok(error instanceof HostTransportError);
    firstError = error;
    return true;
  });
  await assert.rejects(review.outcome, error => error === firstError);
  assert.throws(() => client.retireControllerReview(review.reviewId), HostTransportError);
  assert.deepEqual((await finish()).map(v => v.command.kind), [
    "bootstrap", "review_remote_setup_request",
  ]);
});

test("setup login validation remains local and does not publish a correlation", async t => {
  const { client, finish } = fixture(t);
  await client.bootstrap(bootstrap);
  assert.throws(() => client.beginRemoteSetupRequestReview(original, content, evidence, {
    ...login, operator: "caller-selected",
  }), error => error instanceof HostControlError && !(error instanceof HostTransportError));
  const review = begin(client);
  assert.equal(review.reviewId, 2);
  await client.retireControllerReview(review.reviewId);
  assert.deepEqual(await review.outcome, { state: "unavailable" });
  assert.deepEqual((await finish()).map(v => v.command.kind), [
    "bootstrap", "review_remote_setup_request", "retire_controller_review",
  ]);
});

// Service-parent controls exercise the same private transport and inert child.
// They do not qualify a real administrator, grant mutation or native join.
function serviceGuard(proposalSha256 = "d".repeat(64)) {
  return { proposal_sha256: proposalSha256, authorization_ref: "actual-app-decision-1",
    provenance: { rule_revision: "1" }, released: 0, current: true,
    checkCurrent() { if (!this.current) throw new HostRefusedError("denied"); },
    release() { this.released++; },
  };
}

for (const wireBytes of [12 * 1024, 12 * 1024 + 1]) {
  test("service parent provenance wire limit refuses before transfer or write: " + wireBytes, async t => {
    const { client, finish } = fixture(t, "service_parent");
    await client.bootstrap(bootstrap);
    const request = await client.nextServiceParentAdministration();
    const guard = serviceGuard();
    const overhead = Buffer.byteLength(JSON.stringify({ padding: "" }), "utf8");
    guard.provenance = { padding: "x".repeat(wireBytes - overhead) };
    assert.equal(Buffer.byteLength(JSON.stringify(guard.provenance), "utf8"), wireBytes);
    if (wireBytes > 12 * 1024) {
      assert.throws(() => request.authorize({ kind: "standing_rule_decision", reservation: guard }), HostControlError);
      assert.equal(request.attempt, null);
      assert.equal(guard.released, 0);
      assert.deepEqual(await request.result(), { state: "not_submitted" });
      assert.equal(await client.nextServiceParentAdministration(), request);
      assert.deepEqual((await finish()).map(row => row.command.kind), ["bootstrap", "next_service_parent_administration"]);
      assert.equal(guard.released, 0);
    } else {
      const attempt = request.authorize({ kind: "standing_rule_decision", reservation: guard });
      assert.equal(request.attempt, attempt);
      assert.equal(guard.released, 0);
      assert.deepEqual(await attempt.submit(), { state: "answer_forwarded" });
      assert.equal((await attempt.retire()).custody, "joined");
      assert.equal(guard.released, 1);
      const rows = await finish();
      assert.deepEqual(rows.map(row => row.command.kind), ["bootstrap", "next_service_parent_administration",
        "submit_service_parent_administration", "retire_service_parent_administration"]);
      assert.deepEqual(rows[2].command.decision.provenance, guard.provenance);
    }
  });
}

for (const mode of ["service_parent", "service_parent_uncertain"]) {
  test("service parent retains local authority and separates forwarding from joined outcome: " + mode, async t => {
    const { client, finish } = fixture(t, mode);
    await client.bootstrap(bootstrap);
    const request = await client.nextServiceParentAdministration();
    assert.equal(await client.nextServiceParentAdministration(), request);
    const proposal = request.proposal;
    proposal.rule.routes.length = 0;
    assert.deepEqual(request.proposal.rule.routes, ["member-route"]);
    const guard = serviceGuard();
    const attempt = request.authorize({ kind: "standing_rule_decision", reservation: guard });
    assert.equal(request.attempt, attempt);
    assert.deepEqual(await attempt.result(), { state: "not_submitted" });
    assert.equal(guard.released, 0);
    assert.deepEqual(await attempt.submit(), { state: "answer_forwarded" });
    assert.deepEqual(await attempt.submit(), { state: "answer_forwarded" });
    assert.equal(guard.released, 0);
    const observed = await attempt.result();
    assert.equal(observed.outcome, mode === "service_parent_uncertain" ? "uncertain" : "failed_after_effects");
    assert.equal(observed.custody, "joined");
    assert.equal(observed.parent_issued, true);
    assert.equal(guard.released, 1);
    assert.equal(await client.nextServiceParentAdministration(), request, "Joined alone does not retire the SDK exchange");
    assert.deepEqual(await request.result(), observed);
    assert.deepEqual(await request.retire(), observed);
    assert.deepEqual(await attempt.retire(), observed);
    assert.deepEqual(await request.result(), observed);
    assert.equal(guard.released, 1);
    const rows = await finish();
    assert.deepEqual(rows.map(row => row.command.kind), ["bootstrap", "next_service_parent_administration",
      "submit_service_parent_administration", "result_service_parent_administration", "retire_service_parent_administration"]);
    assert.deepEqual(rows[2].command, { kind: "submit_service_parent_administration", exchange_id: 41,
      proposal_sha256: "d".repeat(64), decision: { kind: "authorized", source: { kind: "standing_rule_decision" },
        authorization_ref: guard.authorization_ref, provenance: guard.provenance } });
  });
}

for (const [method, reason] of [["requiresHuman", "requires_human"], ["deny", "denied"], ["unavailable", "unavailable"]]) {
  test("service parent negative answer retains result and explicit retirement: " + method, async t => {
    const { client, finish } = fixture(t, "service_parent");
    await client.bootstrap(bootstrap);
    const request = await client.nextServiceParentAdministration();
    assert.deepEqual(await request[method](), { state: "answer_forwarded" });
    assert.equal(request.attempt, null);
    const observed = await request.result();
    assert.equal(observed.outcome, "refused_before_entry");
    assert.equal(observed.reason, reason);
    assert.deepEqual(await request.retire(), observed);
    const rows = await finish();
    assert.deepEqual(rows[2].command.decision, { kind: reason });
    assert.deepEqual(rows.map(row => row.command.kind), ["bootstrap", "next_service_parent_administration",
      "submit_service_parent_administration", "result_service_parent_administration", "retire_service_parent_administration"]);
  });
}

test("service parent lost ACK leaves the same attempt and original reply observable", async t => {
  const { client, finish } = fixture(t, "service_parent_lost_ack");
  await client.bootstrap(bootstrap);
  const request = await client.nextServiceParentAdministration();
  const guard = serviceGuard();
  const attempt = request.authorize({ kind: "standing_rule_decision", reservation: guard });
  await assert.rejects(attempt.submit({ timeoutMs: 20 }), HostObservationTimeout);
  assert.equal(request.attempt, attempt);
  assert.equal(guard.released, 0);
  const observed = await request.result();
  assert.equal(observed.outcome, "failed_after_effects");
  assert.deepEqual(await attempt.submit(), { state: "answer_forwarded" });
  assert.deepEqual(await attempt.retire(), observed);
  const rows = await finish();
  assert.equal(rows.filter(row => row.command.kind === "submit_service_parent_administration").length, 1);
});

test("service parent Next Unavailable releases only the unselected query on the same channel", async t => {
  const { client, finish } = fixture(t, "service_parent_next_unavailable");
  await client.bootstrap(bootstrap);
  await assert.rejects(client.nextServiceParentAdministration(), HostRefusedError);
  const request = await client.nextServiceParentAdministration();
  assert.equal(request.exchangeId, 41);
  assert.equal(request.attempt, null);
  await request.retire();
  const rows = await finish();
  const next = rows.filter(row => row.command.kind === "next_service_parent_administration");
  assert.equal(next.length, 2);
  assert.ok(next[1].id > next[0].id);
  assert.equal(rows.filter(row => row.command.kind === "submit_service_parent_administration").length, 0);
});

test("service parent Next timeout retains the original unanswered query", async t => {
  const { client, finish } = fixture(t, "service_parent_next_pending");
  await client.bootstrap(bootstrap);
  await assert.rejects(client.nextServiceParentAdministration({ timeoutMs: 20 }), HostObservationTimeout);
  const pending = client.nextServiceParentAdministration();
  // This existing fixture command releases the held
  // first reply. It neither authorizes nor submits a service-parent answer.
  await client.openComponent("fixture");
  const request = await pending;
  assert.equal(request.exchangeId, 41);
  assert.equal(request.attempt, null);
  await request.retire();
  const rows = await finish();
  assert.equal(rows.filter(row => row.command.kind === "next_service_parent_administration").length, 1);
  assert.equal(rows.filter(row => row.command.kind === "submit_service_parent_administration").length, 0);
});

test("service parent retirement wins before submission and waits for exact joined custody", async t => {
  const { client, finish } = fixture(t, "service_parent_retire_pending");
  await client.bootstrap(bootstrap);
  const request = await client.nextServiceParentAdministration();
  const guard = serviceGuard();
  const attempt = request.authorize({ kind: "standing_rule_decision", reservation: guard });
  await assert.rejects(attempt.retire({ timeoutMs: 20 }), HostObservationTimeout);
  await assert.rejects(attempt.submit(), /retired before submission/);
  assert.equal(guard.released, 0);
  assert.equal(await client.nextServiceParentAdministration(), request);
  await client.openComponent("fixture");
  const observed = await attempt.retire();
  assert.equal(observed.outcome, "refused_before_entry");
  assert.equal(guard.released, 1);
  const next = await client.nextServiceParentAdministration();
  assert.notEqual(next, request);
  assert.equal(next.exchangeId, 42);
  const retiring = next.retire();
  await client.openComponent("fixture");
  await retiring;
  const rows = await finish();
  assert.equal(rows.filter(row => row.command.kind === "submit_service_parent_administration").length, 0);
  assert.equal(rows.filter(row => row.command.kind === "retire_service_parent_administration").length, 2);
});

test("service parent interrupted caller recovers the synchronous request attempt alias", async t => {
  const { client, finish } = fixture(t, "service_parent");
  await client.bootstrap(bootstrap);
  const request = await client.nextServiceParentAdministration();
  const guard = serviceGuard();
  assert.throws(() => {
    request.authorize({ kind: "standing_rule_decision", reservation: guard });
    throw new Error("caller discarded its returned handle");
  }, /discarded/);
  assert.ok(request.attempt);
  await request.attempt.retire();
  assert.equal(guard.released, 1);
  assert.equal((await finish()).filter(row => row.command.kind === "submit_service_parent_administration").length, 0);
});

for (const mode of ["service_parent_eof", "service_parent_running", "service_parent_wrong_parent"]) {
  test("service parent response-map cleanup or malformed join cannot release the guard: " + mode, async t => {
    const { client, finish } = fixture(t, mode);
    await client.bootstrap(bootstrap);
    const request = await client.nextServiceParentAdministration();
    const guard = serviceGuard();
    const attempt = request.authorize({ kind: "standing_rule_decision", reservation: guard });
    if (mode === "service_parent_eof") await assert.rejects(attempt.submit(), HostTransportError);
    else { await attempt.submit(); await assert.rejects(attempt.result(), HostTransportError); }
    assert.equal(request.attempt, attempt);
    assert.equal(guard.released, 0);
    await assert.rejects(attempt.retire(), HostTransportError);
    await client.close();
    assert.equal(guard.released, 0);
    const rows = await finish();
    assert.equal(rows.filter(row => row.command.kind === "submit_service_parent_administration").length, 1);
  });
}

test("service parent login decision requires actual same-client enrollment and guard binding", async t => {
  const first = fixture(t, "service_parent");
  const second = fixture(t, "service_parent");
  await first.client.bootstrap(bootstrap); await second.client.bootstrap(bootstrap);
  const request = await first.client.nextServiceParentAdministration();
  const guard = serviceGuard();
  const otherLogin = second.client.beginConsoleLogin({ issuer: "issuer", subject: "sub", login_scope_id: "scope", expires_at_ms: 9000000000000 });
  await otherLogin.outcome;
  assert.throws(() => request.authorize({ kind: "current_login_decision", reservation: guard, login: otherLogin }), HostControlError);
  assert.throws(() => request.authorize({ kind: "standing_rule_decision", reservation: serviceGuard("e".repeat(64)) }), HostControlError);
  assert.equal(request.attempt, null);
  const actualLogin = first.client.beginConsoleLogin({ issuer: "issuer", subject: "sub", login_scope_id: "scope", expires_at_ms: 9000000000000 });
  await actualLogin.outcome;
  const attempt = request.authorize({ kind: "current_login_decision", reservation: guard, login: actualLogin });
  await attempt.submit();
  await attempt.result();
  await attempt.retire();
  const rows = await first.finish();
  assert.deepEqual(rows.find(row => row.command.kind === "submit_service_parent_administration").command.decision.source,
    { kind: "current_login_decision", enrollment_id: actualLogin.enrollmentId });
  assert.equal(guard.released, 1);
  await second.finish();
});

test("service parent partial command failure retains authority after pending callbacks are cleared", async t => {
  const replies = new PassThrough();
  const rows = [];
  let partial;
  const commands = new Writable({ write(chunk, _encoding, callback) {
    const row = JSON.parse(chunk.toString("utf8"));
    rows.push(row);
    if (row.command.kind === "submit_service_parent_administration") {
      partial = Buffer.from(chunk).subarray(0, 7);
      callback(new Error("controlled partial write failure"));
      return;
    }
    const proposal = { operator: {}, rule: { kind: "application_policy_tasks", routes: ["member-route"] },
      issuance_id: "parent-1", deployment: "deployment-1", source_policy_sha256: "a".repeat(64),
      original: { route: "member-route", original: "original-1", event_sha256: "5".repeat(64) },
      acquisition: { realm: "realm-1", owner: "owner-1", id: "acquisition-1" },
      instruction_sha256: "b".repeat(64), expires_in_ms: 60000, application: {}, represented_subject: null,
      decision: {}, connector_selection: {}, restrictions: {} };
    const result = row.command.kind === "bootstrap" ? { kind: "bootstrap_accepted" }
      : { kind: "service_parent_administration_request", exchange_id: 41, proposal_sha256: "d".repeat(64), proposal };
    replies.write(JSON.stringify({ version: 1, id: row.id, result }) + "\n");
    callback();
  } });
  const client = new PrivateHostClient(commands, replies, { timeoutMs: 1000 });
  t.after(() => client.close());
  await client.bootstrap(bootstrap);
  const request = await client.nextServiceParentAdministration();
  const guard = serviceGuard();
  const attempt = request.authorize({ kind: "standing_rule_decision", reservation: guard });
  await assert.rejects(attempt.submit(), HostTransportError);
  assert.equal(partial.length, 7);
  assert.equal(request.attempt, attempt);
  assert.equal(guard.released, 0);
  await assert.rejects(attempt.submit(), HostTransportError);
  await assert.rejects(attempt.retire(), HostTransportError);
  await client.close();
  assert.equal(guard.released, 0);
  assert.deepEqual(rows.map(row => row.command.kind), ["bootstrap", "next_service_parent_administration", "submit_service_parent_administration"]);
});


for (const large of [false, true]) {
test("service tool policy requires exact complete-candidate acknowledgment and preserves partial outcome" + (large ? " above 64 KiB" : ""), async t => {
  const { client, finish } = fixture(t, large ? "service_tool_policy_large" : "service_tool_policy");
  await client.bootstrap(bootstrap);
  const request = await client.nextServiceParentAdministration();
  const guard = serviceGuard();
  assert.match(request.toolPolicySha256, /^[0-9a-f]{64}$/);
  assert.equal(request.proposal.tool_policy.lifetime, "until_superseded");
  assert.ok(request.proposal.tool_policy.candidate_canonical_json.endsWith("\n"));
  assert.throws(() => request.authorize({ kind: "standing_rule_decision", reservation: guard }), HostControlError);
  assert.throws(() => request.authorizeToolPolicy({ kind: "standing_rule_decision", reservation: guard }, "0".repeat(64)), HostControlError);
  assert.equal(request.attempt, null);
  assert.equal(guard.released, 0);
  const digest = request.toolPolicySha256;
  const attempt = request.authorizeToolPolicy({ kind: "standing_rule_decision", reservation: guard }, digest);
  await attempt.submit();
  assert.equal(guard.released, 0);
  const observed = await attempt.result();
  assert.equal(observed.outcome, "failed_after_effects");
  assert.deepEqual(observed.tool_policy, { proposal_sha256: digest, candidate: request.proposal.tool_policy.candidate,
    native: "accepted", retained_as_current: false });
  assert.equal(guard.released, 1, "Joined, not policy acceptance, releases retained guard");
  assert.deepEqual(await attempt.retire(), observed);
  const rows = await finish();
  const submitted = rows.filter(row => row.command.kind === "submit_service_parent_administration");
  assert.equal(submitted.length, 1);
  assert.equal(submitted[0].command.decision.kind, "authorized_with_tool_policy");
  assert.equal(submitted[0].command.decision.tool_policy_sha256, digest);
});
}

for (const drift of ["bad_digest", "bare_digest", "revision_negative", "revision_string", "revision_unsafe"]) {
test(`service tool policy ${drift} refuses before local authorization`, async t => {
  const { client, finish } = fixture(t, "service_tool_policy_" + drift);
  await client.bootstrap(bootstrap);
  await assert.rejects(client.nextServiceParentAdministration(), HostTransportError);
  const rows = await finish();
  assert.equal(rows.some(row => row.command.kind === "submit_service_parent_administration"), false);
});
}
