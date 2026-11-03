import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { Readable, Writable } from "node:stream";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import { PrivateHostClient, HostControlError, HostTransportError, HostObservationTimeout, HostRefusedError } from "../dist/private-host.js";
const artifact = { path: "/private/selected.json", sha256: "1".repeat(64) };
const selection = { deployment_root: "/private/realm", activation: artifact, policy_manifest: artifact, local_policy: artifact };
const event = { route: "telegram/main", original: "request-123", occurrence: "event-456",
  original_authentication: { resource_id: "auth-1", revision: "7", sha256: "2".repeat(64) },
  root_event: { resource_id: "message-1", revision: "1", sha256: "3".repeat(64) },
  contributors: ["earlier-ä", "😀"], content_sha256: "4".repeat(64) };
const original = { route: event.route, original: event.original, event_sha256: "5".repeat(64) };

// These decoder snapshots are not native admission or completion evidence.
function serviceReceipt(caseName) {
  if (caseName === "unresolved") return { state: "admission_unresolved" };
  const observation = { state: "admitted", session_id: "session-b", input_id: "input-b", receipt: null };
  if (caseName === "absent") return observation;
  let report = { status: "pending", input_id: "input-b", phase: "consumed", terminal: { outcome_type: "consumed" },
    last_run_id: "historical-run", attempt_count: 2 };
  if (caseName === "without") { delete report.phase; report.status = "terminal_without_receipt"; }
  else if (["batch", "finalization_failure", "terminated", "numeric_payload"].includes(caseName)) {
    report = { status: "finalized", input_id: "input-b", terminal: { outcome_type: "consumed" }, attempt_count: 2,
      scope: { kind: "run", run_id: "run-b" }, run_id: "run-b", owner_input_id: "input-a",
      recipient_input_ids: ["input-a", "input-b"], outcome: { completion_type: "completed_without_result" } };
    if (caseName === "finalization_failure") report.outcome = { completion_type: "completed_with_finalization_failure", error: { fixture_payload: "preserved" } };
    if (caseName === "terminated") Object.assign(report, { scope: { kind: "runtime_termination" }, run_id: null, outcome: { completion_type: "cancelled" } });
    if (caseName === "numeric_payload") report.outcome = { completion_type: "callback_pending", tool_use_id: "call", tool_name: "read",
      args: { offset: -0.5, values: [1.25, -7], nested: { ratio: 0.125 } } };
  }
  observation.receipt = { source: "durable_store", report };
  return observation;
}

function receiptProtocolFixture(t, replyFor) {
  const commands = [];
  const responses = new Readable({ read() {} });
  const send = (request, result, transform = value => value) => responses.push(Buffer.from(transform(JSON.stringify({ version: 1, id: request.id, result })) + "\n"));
  const writer = new Writable({ write(chunk, _encoding, done) {
    const request = JSON.parse(chunk.toString("utf8")); commands.push(request); done();
    const reply = request.command.kind === "bootstrap" ? { kind: "bootstrap_accepted" } : replyFor(request);
    if (typeof reply === "string") responses.push(Buffer.from(reply + "\n"));
    else if (reply) send(request, reply);
  } });
  const client = new PrivateHostClient(writer, responses, { timeoutMs: 1000 });
  t.after(async () => { await client.close(); assert.equal(writer.destroyed, true); assert.equal(responses.destroyed, true); });
  return { client, commands, send };
}

for (const caseName of ["unresolved", "absent", "pending", "without", "batch", "finalization_failure", "terminated", "numeric_payload"]) {
  test(`service receipt preserves ${caseName} without manufacturing completion`, async t => {
    const expected = serviceReceipt(caseName);
    const { client, commands } = receiptProtocolFixture(t, () => ({ kind: "service_input_receipt", original, observation: expected }));
    await client.bootstrap(selection);
    assert.deepEqual(await client.observeServiceInputReceipt(original), expected);
    assert.deepEqual(commands.map(row => row.command), [{ kind: "bootstrap", selection }, { kind: "observe_service_input_receipt", original }]);
  });
}

for (const mutation of ["original", "input", "owner", "scope", "witness", "status", "extra", "count"]) {
  test(`service receipt rejects ${mutation} mismatch`, async t => {
    const observation = serviceReceipt("batch"), returned = structuredClone(original);
    const report = observation.receipt.report;
    if (mutation === "original") returned.original = "another";
    if (mutation === "input") report.input_id = "another";
    if (mutation === "owner") report.owner_input_id = "input-b";
    if (mutation === "scope") report.scope.run_id = "another";
    if (mutation === "witness") observation.receipt.source = "model_text";
    if (mutation === "status") report.status = "completed";
    if (mutation === "extra") observation.success = true;
    if (mutation === "count") report.attempt_count = true;
    const { client, commands } = receiptProtocolFixture(t, () => ({ kind: "service_input_receipt", original: returned, observation }));
    await client.bootstrap(selection);
    await assert.rejects(client.observeServiceInputReceipt(original), HostControlError);
    assert.equal(commands.length, 2);
  });
}

test("service receipt unavailable preserves a subsequent numeric receipt on the same channel", async t => {
  let first = true;
  const { client, commands } = receiptProtocolFixture(t, () => {
    if (first) { first = false; return { kind: "error", error: "unavailable" }; }
    return { kind: "service_input_receipt", original, observation: serviceReceipt("numeric_payload") };
  });
  await client.bootstrap(selection);
  await assert.rejects(client.observeServiceInputReceipt(original), HostRefusedError);
  assert.deepEqual(await client.observeServiceInputReceipt(original), serviceReceipt("numeric_payload"));
  assert.equal(commands.length, 3);
});

test("service receipt native numeric payload does not relax private counter tokens", async t => {
  const { client, commands } = receiptProtocolFixture(t, request => JSON.stringify({ version: 1, id: request.id,
    result: { kind: "service_input_receipt", original, observation: serviceReceipt("numeric_payload") } })
    .replace('"attempt_count":2', '"attempt_count":2.0'));
  await client.bootstrap(selection);
  await assert.rejects(client.observeServiceInputReceipt(original), HostControlError);
  assert.equal(commands.length, 2);
});

test("service receipt late observation drains without replaying work", async t => {
  let hold = true;
  const { client, commands, send } = receiptProtocolFixture(t, () => hold ? null
    : { kind: "service_input_receipt", original, observation: serviceReceipt("absent") });
  await client.bootstrap(selection);
  await assert.rejects(client.observeServiceInputReceipt(original, { timeoutMs: 20 }), HostObservationTimeout);
  send(commands.at(-1), { kind: "service_input_receipt", original, observation: serviceReceipt("pending") });
  hold = false;
  assert.deepEqual(await client.observeServiceInputReceipt(original), serviceReceipt("absent"));
  assert.equal(commands.length, 3);
  assert.deepEqual(commands.slice(1).map(row => row.command.kind), ["observe_service_input_receipt", "observe_service_input_receipt"]);
});

test("fresh preparation retains exact first request and adopts only its observed result once", async t => {
  const { client, commands } = fixture(t);
  const supplied = { plan: structuredClone(artifact) };
  const attempt = client.beginApplicationPreparation(supplied);
  const beforeObservation = attempt.adopt(selection);
  void beforeObservation.catch(() => {});
  supplied.plan.path = "/changed-after-call";
  assert.throws(() => client.assertReady(), HostControlError);
  await assert.rejects(client.openComponent("frontend"), HostControlError);
  await assert.rejects(client.bootstrap(selection), HostControlError);
  await assert.rejects(beforeObservation, /has not completed/);
  assert.throws(() => client.beginApplicationPreparation({ plan: artifact }), /already requested/);
  assert.equal(attempt.observe(), attempt.observe());
  assert.deepEqual(await attempt.observe(), { path: "/private/completed.json", sha256: "7".repeat(64) });
  assert.ok(Object.isFrozen(attempt.manifest));
  const selected = structuredClone(selection);
  const adopted = attempt.adopt(selected);
  selected.activation.path = "/mutated-after-adopt";
  assert.equal(attempt.adopt(selection), adopted);
  await adopted;
  await assert.rejects(attempt.adopt({ ...selection, deployment_root: "/other" }), /another final selection/);
  client.assertReady();
  assert.deepEqual(attempt.selection, selection);
  await delay(10);
  assert.deepEqual(commands(), [
    { id: 1, command: { kind: "prepare_application", plan: artifact } },
    { id: 2, command: { kind: "bootstrap", selection } },
  ]);
});

test("invalid preparation selections do not consume the first request", async t => {
  const { client, commands } = fixture(t);
  for (const value of [{ plan: { ...artifact, path: "relative" } },
    { plan: { ...artifact, sha256: "bad" } }, { plan: artifact, grants: [] }]) {
    assert.throws(() => client.beginApplicationPreparation(value), HostControlError);
  }
  const attempt = client.beginApplicationPreparation({ plan: artifact });
  await attempt.observe(); await attempt.adopt(selection);
  await delay(10); assert.equal(commands().length, 2);
});

for (const mode of ["prepare-refused", "prepare-malformed"]) {
  test(`${mode} retains the exact failed observation and never permits adoption or replay`, async t => {
    const { client, commands } = fixture(t, mode);
    const attempt = client.beginApplicationPreparation({ plan: artifact }, { timeoutMs: 1000 });
    const failure = await attempt.observe().catch(error => error);
    assert.ok(failure instanceof HostControlError);
    if (mode === "prepare-refused") assert.ok(failure instanceof HostRefusedError);
    await assert.rejects(attempt.observe(), error => error === failure);
    await delay(100);
    assert.equal(attempt.manifest, null);
    await assert.rejects(attempt.adopt(selection), /has not completed/);
    assert.throws(() => client.beginApplicationPreparation({ plan: artifact }), /already requested/);
    assert.equal(commands().length, 1);
  });
}

// An object-mode Buffer and the next iterator request give an explicit
// decoder-consumption boundary, independent of child startup or pipe timing.
test("prepare-late retains the exact failed observation and never permits adoption or replay", async t => {
  const commands = [];
  let consumed;
  const lateConsumed = new Promise(resolve => { consumed = resolve; });
  const lateReply = Buffer.from(JSON.stringify({ version: 1, id: 1,
    result: { kind: "application_prepared", manifest: {
      path: "/private/completed.json", sha256: "7".repeat(64) } } }) + "\n");
  class ObservedResponses extends Readable {
    async *[Symbol.asyncIterator]() {
      for await (const chunk of super[Symbol.asyncIterator]()) {
        yield chunk;
        if (chunk === lateReply) consumed();
      }
    }
  }
  const responses = new ObservedResponses({ objectMode: true, read() {} });
  const writer = new Writable({ write(chunk, _encoding, done) {
    commands.push(JSON.parse(chunk.toString("utf8"))); done();
  } });
  const client = new PrivateHostClient(writer, responses, { timeoutMs: 1000 });
  t.after(async () => {
    await client.close();
    assert.equal(writer.destroyed, true); assert.equal(responses.destroyed, true);
  });
  const attempt = client.beginApplicationPreparation({ plan: artifact }, { timeoutMs: 20 });
  const observation = attempt.observe();
  const failure = await observation.catch(error => error);
  assert.ok(failure instanceof HostObservationTimeout);
  assert.equal(attempt.observe(), observation);
  await assert.rejects(attempt.observe(), error => error === failure);
  responses.push(lateReply);
  let timer;
  try {
    await Promise.race([lateConsumed, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("late reply was not consumed by the decoder")), 1000);
    })]);
  } finally { clearTimeout(timer); }
  assert.equal(attempt.manifest, null);
  await assert.rejects(attempt.observe(), error => error === failure);
  await assert.rejects(attempt.adopt(selection), /has not completed/);
  assert.throws(() => client.beginApplicationPreparation({ plan: artifact }), /already requested/);
  await client.close();
  assert.deepEqual(commands, [{ version: 1, id: 1, command: { kind: "prepare_application", plan: artifact } }]);
});

test("lost adoption acknowledgement retains the chosen selection without another write", async t => {
  const { client, commands } = fixture(t, "prepare-adopt-pending");
  const attempt = client.beginApplicationPreparation({ plan: artifact }, { timeoutMs: 1000 });
  await attempt.observe();
  const first = attempt.adopt(selection);
  await assert.rejects(first, HostObservationTimeout);
  assert.deepEqual(attempt.selection, selection);
  assert.equal(attempt.adopt(selection), first);
  await assert.rejects(attempt.adopt({ ...selection, deployment_root: "/other" }), /another final selection/);
  await delay(10); assert.deepEqual(commands().map(row => row.id), [1, 2]);
});

function fixture(t, mode = "normal", timeoutMs = 1000) {
  const child = spawn(process.execPath, [new URL("./fixtures/private-host-child.mjs", import.meta.url).pathname, mode], {
    stdio: ["ignore", "pipe", "pipe", "pipe", "pipe"], env: { PATH: "/usr/bin:/bin" },
  });
  let stderr = "", forced = false, ready = false, closing;
  const rows = [], waiters = new Set();
  child.stdout.setEncoding("utf8"); child.stderr.setEncoding("utf8");
  const output = createInterface({ input: child.stdout });
  output.on("line", line => {
    if (line === "fixture-ready") ready = true;
    else rows.push(JSON.parse(line));
    for (const check of [...waiters]) check();
  });
  child.stderr.on("data", value => stderr += value);
  const closed = new Promise(resolve => child.once("close", (code, signal) => resolve({ code, signal })));
  child.once("error", error => { throw error; });
  const client = new PrivateHostClient(child.stdio[3], child.stdio[4], { timeoutMs });
  function observe(predicate, label) {
    if (predicate()) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { waiters.delete(check); reject(new Error(label)); }, 2000);
      const check = () => { if (predicate()) { clearTimeout(timer); waiters.delete(check); resolve(); } };
      waiters.add(check); check();
    });
  }
  function close() {
    return closing ??= (async () => {
      const timer = setTimeout(() => { forced = true; child.kill("SIGKILL"); }, 2000);
      await client.close();
      const exit = await closed; clearTimeout(timer);
      assert.equal(forced, false, "forced child cleanup is not successful lifecycle qualification");
      assert.deepEqual(exit, { code: 0, signal: null }); assert.equal(stderr, "");
      t.diagnostic(JSON.stringify({ childPid: child.pid, closed: true, exit, forcedCleanup: forced }));
      assert.equal(child.stdio[3].destroyed, true); assert.equal(child.stdio[4].destroyed, true);
    })();
  }
  t.after(close);
  return { client, close, commands: () => structuredClone(rows),
    ready: () => observe(() => ready, "private fixture did not report readiness"),
    observeCommands: count => observe(() => rows.length >= count, "private fixture command rows were not observed") };

}

// Actual inherited Node endpoints, with inert replies. This does not start Meta
// or prove native descriptor admission, authentication, attachment or grants.
test("private bootstrap, exact source retention, and operation-only cancellation over inherited endpoints", async t => {
  const { client, commands } = fixture(t);
  await client.bootstrap(selection);
  const supplied = structuredClone(event);
  const pending = client.retainAuthenticatedEvent(supplied);
  supplied.original = "mutated-after-call"; supplied.contributors.reverse();
  const retained = await pending;
  assert.deepEqual(retained, { original: { route: event.route, original: event.original,
    event_sha256: "6eeb9c0a26a8ba53abcfbc21b42cd3d0b6b528e74d6ff7985550dc69243b3d7e" }, state: "retained_pending_native_admission" });
  assert.deepEqual(await client.cancelOriginalOperation(retained.original), { original: retained.original, state: "operations_cancelled" });
  await delay(10);
  assert.deepEqual(commands().map(value => value.command.kind), ["bootstrap", "retain_authenticated_event", "cancel_original_operation"]);
  assert.equal(commands()[0].id, 1);
  assert.deepEqual(commands()[0].command.selection, selection);
});

test("pre-bootstrap requests and removed authority fields are refused before writing", async t => {
  const { client, commands } = fixture(t);
  await assert.rejects(client.retainAuthenticatedEvent(event), HostControlError);
  await assert.rejects(client.bootstrap({ ...selection, issuer_id: "legacy" }), HostControlError);
  await client.bootstrap(selection);
  let accessed = false;
  const accessor = { ...event, get content_sha256() { accessed = true; throw new Error("secret"); } };
  await assert.rejects(client.retainAuthenticatedEvent(accessor), HostControlError);
  assert.equal(accessed, false);
  for (const changed of [{ ...event, grants: ["*:"] }, { ...event, route: "../bad" }, { ...event, original: "\ud800" }, { ...event, contributors: Array(65).fill("x") }]) {
    await assert.rejects(client.retainAuthenticatedEvent(changed), HostControlError);
  }
  await delay(10); assert.equal(commands().length, 1);
});

for (const code of ["source_conflict", "invalid_selection", "unavailable", "denied", "wrong_owner", "not_settled"]) {
  test(`typed native ${code} is an exact reply and keeps the private channel available`, async t => {
    const { client, commands } = fixture(t, `refused:${code}`); await client.bootstrap(selection);
    await assert.rejects(client.retainAuthenticatedEvent(event), error => {
      assert.ok(error instanceof HostControlError); assert.ok(error instanceof HostRefusedError);
      assert.equal(error instanceof HostTransportError, false); assert.equal(error.refusal, code);
      assert.equal(error.message, `native host refused: ${code}`); return true;
    });
    assert.equal((await client.openComponent("telegram")).state, "open");
    await delay(10);
    assert.deepEqual(commands().map(value => value.id), [1, 2, 3]);
  });
}

for (const mode of ["wrong-reference", "wrong-retain-state", "wrong-cancel-state"]) {
  test(`${mode} never becomes a successful original operation`, async t => {
    const { client } = fixture(t, mode); await client.bootstrap(selection);
    await assert.rejects(mode === "wrong-cancel-state" ? client.cancelOriginalOperation(original) : client.retainAuthenticatedEvent(event), HostControlError);
    await assert.rejects(client.openComponent("telegram"), HostControlError);
  });
}

test("a pending close join does not block a subsequent disable", async t => {
  const { client } = fixture(t, "concurrent"); await client.bootstrap(selection);
  const close = await client.beginCloseActivation("shutdown");
  assert.equal(close.state, "closing");
  const joined = client.joinClose(close.handle);
  const disabled = await client.beginCloseComponent("telegram", "operator_disable");
  assert.deepEqual(disabled.handle.scope, { kind: "component", component: "telegram" });
  assert.equal((await joined).state, "settled");
});

test("timed-out source observation is not retried; its exact late reply retires correlation", async t => {
  const { client, commands } = fixture(t, "late"); await client.bootstrap(selection);
  await assert.rejects(client.retainAuthenticatedEvent(event, { timeoutMs: 20 }), HostObservationTimeout);
  await delay(100);
  assert.equal((await client.openComponent("telegram")).component, "telegram");
  await delay(10);
  assert.equal(commands().filter(value => value.command.kind === "retain_authenticated_event").length, 1);
});

for (const mode of ["duplicate-key", "numeric-version", "unknown-id", "bom", "invalid-utf8", "oversize", "partial-eof", "unknown-error", "malformed-error"]) {
  test(`${mode} closes both private endpoint owners`, async t => {
    const { client } = fixture(t, mode); await client.bootstrap(selection);
    const failure = await client.openComponent("telegram").catch(error => error);
    assert.ok(failure instanceof HostTransportError); assert.ok(failure instanceof HostControlError);
    assert.equal(failure instanceof HostRefusedError, false);
    await assert.rejects(client.cancelOriginalOperation(original), error => error === failure);
  });
}

test("duplicate completed response IDs fail later observations", async t => {
  const { client } = fixture(t, "duplicate-id"); await client.bootstrap(selection);
  await client.openComponent("telegram"); await delay(10);
  await assert.rejects(client.cancelOriginalOperation(original), HostControlError);
});


const transportAuthentication = { resource_id: "authenticated-transport", revision: "4", sha256: "6".repeat(64) };
const admitted = { state: "admitted", session_id: "native-session", input_id: `input-${original.original}`,
  deduplicated: false, turn_ticket: "00000000-0000-0000-0000-000000000003", source_settlement: "fresh_acceptance_promoted" };

test("original submission writes exact content and frozen evidence, with admission separate from completion", async t => {
  const { client, commands } = fixture(t); await client.bootstrap(selection);
  const reference = structuredClone(original), authentication = structuredClone(transportAuthentication);
  const content = "Read my latest messages.\nRäksmörgås 😀";
  const pending = client.submitAuthenticatedOriginal(reference, content, authentication);
  reference.original = "changed-after-call"; authentication.revision = "changed-after-call";
  assert.deepEqual(await pending, admitted);
  await delay(10);
  assert.deepEqual(commands()[1], { id: 2, command: { kind: "submit_authenticated_original",
    submission: { original, content, transport_authentication: transportAuthentication } } });
  assert.equal(commands().length, 2);
});

for (const [mode, expected] of [
  ["submit-deduplicated", { ...admitted, deduplicated: true, turn_ticket: null, source_settlement: "retry_withdrawn" }],
  ["submit-publication-failed", { ...admitted, source_settlement: "post_admission_publication_failed" }],
  ["submit-unresolved", { state: "admission_unresolved" }],
  ["submit-unavailable", { state: "native_outcome_unavailable" }],
]) {
  test(`${mode} preserves the native observation without resending`, async t => {
    const { client, commands } = fixture(t, mode); await client.bootstrap(selection);
    assert.deepEqual(await client.submitAuthenticatedOriginal(original, "text", transportAuthentication), expected);
    assert.equal((await client.openComponent("telegram")).state, "open");
    await delay(10);
    assert.equal(commands().filter(value => value.command.kind === "submit_authenticated_original").length, 1);
  });
}

test("submission refuses invalid or authority-bearing inputs locally before any write", async t => {
  const { client, commands } = fixture(t);
  await assert.rejects(client.submitAuthenticatedOriginal(original, "text", transportAuthentication), HostControlError);
  await client.bootstrap(selection);
  let accessed = false;
  const getter = { ...transportAuthentication, get sha256() { accessed = true; throw new Error("private"); } };
  for (const [ref, content, auth] of [
    [{ ...original, grants: ["*:"] }, "text", transportAuthentication],
    [original, "text", { ...transportAuthentication, requester: "other" }],
    [original, "\ud800", transportAuthentication],
    [original, "text", getter],
    [original, "text", { ...transportAuthentication, sha256: "not-a-digest" }],
  ]) await assert.rejects(client.submitAuthenticatedOriginal(ref, content, auth), HostControlError);
  assert.equal(accessed, false); await delay(10); assert.equal(commands().length, 1);
});

test("a typed submission refusal remains distinct from unavailable outcome", async t => {
  const { client } = fixture(t, "submit-refused"); await client.bootstrap(selection);
  await assert.rejects(client.submitAuthenticatedOriginal(original, "text", transportAuthentication),
    error => error instanceof HostRefusedError && error.refusal === "source_conflict");
  assert.equal((await client.openComponent("telegram")).state, "open");
});

for (const mode of ["submit-wrong-original", "submit-wrong-digest", "submit-wrong-state", "submit-extra-completion",
  "submit-wrong-settlement", "submit-wrong-boolean", "submit-wrong-ticket", "submit-invalid-session", "submit-unresolved-ticket"]) {
  test(`${mode} poisons correlation instead of reporting an admission`, async t => {
    const { client, commands } = fixture(t, mode); await client.bootstrap(selection);
    await assert.rejects(client.submitAuthenticatedOriginal(original, "text", transportAuthentication), HostControlError);
    await assert.rejects(client.submitAuthenticatedOriginal(original, "text", transportAuthentication), HostControlError);
    await delay(10);
    assert.equal(commands().filter(value => value.command.kind === "submit_authenticated_original").length, 1);
  });
}

test("timed-out submission does not resend and its exact late response retires correlation", async t => {
  const { client, commands } = fixture(t, "submit-late"); await client.bootstrap(selection);
  await assert.rejects(client.submitAuthenticatedOriginal(original, "text", transportAuthentication, { timeoutMs: 20 }), HostObservationTimeout);
  await delay(100);
  assert.equal((await client.openComponent("telegram")).state, "open");
  await delay(10);
  assert.equal(commands().filter(value => value.command.kind === "submit_authenticated_original").length, 1);
});

test("partial EOF during submission leaves the outcome unknown and never resends", async t => {
  const { client, commands } = fixture(t, "partial-eof"); await client.bootstrap(selection);
  await assert.rejects(client.submitAuthenticatedOriginal(original, "text", transportAuthentication),
    error => error instanceof HostControlError && /settlement is unknown/.test(error.message));
  await assert.rejects(client.submitAuthenticatedOriginal(original, "text", transportAuthentication), HostControlError);
  await delay(10);
  assert.equal(commands().filter(value => value.command.kind === "submit_authenticated_original").length, 1);
});

test("concurrent original submissions keep distinct input identities when replies arrive in reverse order", async t => {
  const { client, commands } = fixture(t, "submit-concurrent"); await client.bootstrap(selection);
  const second = { ...original, original: "second-original", event_sha256: "7".repeat(64) };
  const firstReply = client.submitAuthenticatedOriginal(original, "first text", transportAuthentication);
  const secondReply = client.submitAuthenticatedOriginal(second, "second text", transportAuthentication);
  assert.deepEqual(await firstReply, admitted);
  assert.deepEqual(await secondReply, { ...admitted, input_id: "input-second-original" });
  await delay(10);
  assert.deepEqual(commands().map(value => value.id), [1, 2, 3]);
});


for (const mode of ["legacy-cancel-state", "cancel-wrong-original", "cancel-cleanup-pending"]) {
  test(`${mode} cannot become durable original cancellation`, async t => {
    const { client, commands } = fixture(t, mode); await client.bootstrap(selection);
    await assert.rejects(client.cancelOriginalOperation(original), HostControlError);
    await assert.rejects(client.cancelOriginalOperation(original), HostControlError);
    await delay(10);
    assert.equal(commands().filter(value => value.command.kind === "cancel_original_operation").length, 1);
  });
}

test("timed-out durable cancellation consumes its late observation without repeating it", async t => {
  const { client, commands } = fixture(t, "cancel-late"); await client.bootstrap(selection);
  await assert.rejects(client.cancelOriginalOperation(original, { timeoutMs: 20 }), HostObservationTimeout);
  await delay(100);
  assert.equal((await client.openComponent("telegram")).state, "open");
  await delay(10);
  assert.equal(commands().filter(value => value.command.kind === "cancel_original_operation").length, 1);
});

const reviewLogin = { scope_id: "authenticated-login", expires_at_ms: 1900000000000 };

test("remote prompt controls proceed while final native review is pending", async t => {
  const { client, commands } = fixture(t); await client.bootstrap(selection);
  const review = client.beginRemoteControllerReview(original, "Question", event.original_authentication, reviewLogin);
  let completed = false; void review.outcome.then(() => { completed = true; });
  assert.equal(review.reviewId, 2);
  const prompt = await client.observeControllerReview(review.reviewId);
  assert.equal(prompt.state, "prompt"); assert.equal(prompt.prompt.approval_id, "approval-1");
  assert.deepEqual(await client.answerControllerReview(review.reviewId, "approval-1", "approve"), { state: "answer_forwarded" });
  assert.equal(completed, false, "answer forwarding is not the native outcome");
  assert.deepEqual(await client.observeControllerReview(review.reviewId), { state: "answer_forwarded" });
  await client.openComponent("telegram");
  const outcome = await review.outcome;
  assert.equal(outcome.state, "selected"); assert.equal(outcome.purpose, "controller"); assert.equal(outcome.route, original.route);
  await delay(10);
  const seen = commands();
  assert.deepEqual(seen.map(v => v.id), [1, 2, 3, 4, 5, 6]);
  assert.equal(seen.filter(v => v.command.kind === "review_remote_controller_parent").length, 1);
  assert.deepEqual(seen[1].command.login, reviewLogin);
  assert.deepEqual(Object.keys(seen[3].command).sort(), ["approval_id", "decision", "kind", "review_id"]);
});

test("remote retirement leaves the shared private connection usable", async t => {
  const { client } = fixture(t); await client.bootstrap(selection);
  const review = client.beginRemoteControllerReview(original, "Question", event.original_authentication, reviewLogin);
  assert.deepEqual(await client.retireControllerReview(review.reviewId), { state: "retired" });
  assert.deepEqual(await review.outcome, { state: "unavailable" });
  assert.equal((await client.openComponent("telegram")).state, "open");
});

test("remote timeout never repeats setup and still drains its late native outcome", async t => {
  const { client, commands } = fixture(t); await client.bootstrap(selection);
  const review = client.beginRemoteControllerReview(original, "Question", event.original_authentication, reviewLogin, { timeoutMs: 20 });
  await assert.rejects(review.outcome, error => {
    assert.ok(error instanceof HostObservationTimeout); assert.ok(error instanceof HostControlError);
    assert.equal(error instanceof HostTransportError, false); assert.equal(error instanceof HostRefusedError, false); return true;
  });
  await client.retireControllerReview(review.reviewId);
  assert.equal((await client.openComponent("telegram")).state, "open");
  await delay(10);
  assert.equal(commands().filter(v => v.command.kind === "review_remote_controller_parent").length, 1);
});

for (const mode of ["review-wrong-route", "review-wrong-purpose", "review-empty-lineage"]) {
  test(`${mode} cannot become a selected controller parent`, async t => {
    const { client } = fixture(t, mode); await client.bootstrap(selection);
    const review = client.beginRemoteControllerReview(original, "Question", event.original_authentication, reviewLogin);
    await client.observeControllerReview(review.reviewId);
    await client.answerControllerReview(review.reviewId, "approval-1", "approve");
    const opening = client.openComponent("telegram");
    await assert.rejects(review.outcome, HostControlError);
    await assert.rejects(opening, HostControlError);
  });
}

test("remote response correlation mismatch closes both endpoints", async t => {
  const { client } = fixture(t, "review-wrong-id"); await client.bootstrap(selection);
  const review = client.beginRemoteControllerReview(original, "Question", event.original_authentication, reviewLogin);
  await assert.rejects(client.observeControllerReview(review.reviewId), HostControlError);
  await assert.rejects(review.outcome, HostControlError);
});

test("remote login cannot carry caller-selected authority", async t => {
  const { client, commands } = fixture(t); await client.bootstrap(selection);
  for (const login of [{ ...reviewLogin, operator: "forged" }, { ...reviewLogin, expires_at_ms: Number.MAX_SAFE_INTEGER + 1 }]) {
    assert.throws(() => client.beginRemoteControllerReview(original, "Question", event.original_authentication, login), HostControlError);
  }
  await delay(10); assert.equal(commands().length, 1);
});


test("written human review outlives the control timeout without closing shared transport", async t => {
  const { client, commands } = fixture(t, "normal", 200); await client.bootstrap(selection);
  const review = client.beginRemoteControllerReview(original, "Question", event.original_authentication, reviewLogin, { timeoutMs: 2000 });
  await delay(350);
  assert.deepEqual(await client.retireControllerReview(review.reviewId), { state: "retired" });
  assert.deepEqual(await review.outcome, { state: "unavailable" });
  assert.deepEqual(await review.outcome, { state: "unavailable" });
  assert.equal((await client.openComponent("telegram")).state, "open");
  assert.equal(commands().filter(value => value.command.kind === "review_remote_controller_parent").length, 1);
});


function localFailure(error) {
  assert.ok(error instanceof HostControlError);
  assert.equal(error instanceof HostTransportError, false);
  assert.equal(error instanceof HostRefusedError, false);
  assert.equal(error instanceof HostObservationTimeout, false);
  return true;
}

test("local invalid or oversized commands preserve the pending review and next correlation", async t => {
  const { client, commands } = fixture(t); await client.bootstrap(selection);
  const review = client.beginRemoteControllerReview(original, "Question", event.original_authentication, reviewLogin);
  await assert.rejects(client.submitAuthenticatedOriginal(original, "x".repeat(1_048_576), transportAuthentication), localFailure);
  await assert.rejects(client.submitAuthenticatedOriginal(original, "text", { ...transportAuthentication, requester: "forged" }), localFailure);
  assert.equal((await client.observeControllerReview(review.reviewId)).state, "prompt");
  await client.retireControllerReview(review.reviewId);
  assert.deepEqual(await review.outcome, { state: "unavailable" });
  assert.equal((await client.openComponent("telegram")).state, "open");
  await delay(10);
  assert.deepEqual(commands().map(value => value.id), [1, 2, 3, 4, 5]);
  assert.equal(commands().filter(value => value.command.kind === "submit_authenticated_original").length, 0);
});

test("oversized unwritten bootstrap remains local and a valid bootstrap uses the first correlation", async t => {
  const { client, commands } = fixture(t);
  await assert.rejects(client.bootstrap({ ...selection, deployment_root: "x".repeat(1_048_576) }), localFailure);
  await client.bootstrap(selection);
  assert.equal((await client.openComponent("telegram")).state, "open");
  await delay(10);
  assert.deepEqual(commands().map(value => value.id), [1, 2]);
  assert.deepEqual(commands()[0].command.selection, selection);
});

for (const mode of ["bootstrap-refused", "bootstrap-pending"]) {
  test(`${mode} preserves the exact first error but leaves no reusable bootstrap channel`, async t => {
    const { client, commands } = fixture(t, mode, 500);
    await assert.rejects(client.bootstrap(selection), error => {
      assert.ok(error instanceof HostControlError);
      assert.equal(error instanceof HostTransportError, false);
      if (mode === "bootstrap-refused") { assert.ok(error instanceof HostRefusedError); assert.equal(error.refusal, "denied"); }
      else assert.ok(error instanceof HostObservationTimeout);
      return true;
    });
    await assert.rejects(client.openComponent("telegram"), HostTransportError);
    await client.close(); await delay(10);
    assert.equal(commands().filter(value => value.command.kind === "bootstrap").length, 1);
    assert.equal(commands().length, 1);
  });
}

const consoleLogin = { issuer: "issuer", subject: "verified-sub", login_scope_id: "scope-a", expires_at_ms: 9_000_000_000_000 };

test("Console enrollment is exact and a timed-out outcome never becomes success after its late reply", async t => {
  const { client, commands } = fixture(t, "console-late"); await client.bootstrap(selection);
  const supplied = structuredClone(consoleLogin);
  const login = client.beginConsoleLogin(supplied, { timeoutMs: 20 });
  supplied.subject = "display-email";
  await assert.rejects(login.outcome, HostObservationTimeout);
  await delay(100);
  await assert.rejects(login.outcome, HostObservationTimeout);
  const retiring = login.retire();
  assert.equal(login.retire(), retiring);
  assert.deepEqual(await retiring, { fenced: true, reservations_drained: true, uncertain: true });
  await delay(10);
  const observed = commands();
  assert.deepEqual(observed.map(row => row.command.kind), ["bootstrap", "enroll_console_login", "retire_console_login"]);
  assert.deepEqual(observed[1].command.login, consoleLogin);
  assert.equal(observed[2].command.enrollment_id, observed[1].id);
});

test("Console retirement bypasses all 32 ordinary pending calls on the actual writer", async t => {
  const { client, commands } = fixture(t, "console-full"); await client.bootstrap(selection);
  const login = client.beginConsoleLogin(consoleLogin); await login.outcome;
  const pending = Array.from({ length: 32 }, () => client.openComponent("pending").catch(error => error));
  await assert.rejects(client.openComponent("full"), HostControlError);
  assert.deepEqual(await login.retire(), { fenced: true, reservations_drained: true, uncertain: false });
  await delay(10);
  assert.equal(commands().filter(row => row.command.kind === "open_component").length, 32);
  assert.equal(commands().filter(row => row.command.kind === "retire_console_login").length, 1);
  await client.close();
  assert.ok((await Promise.all(pending)).every(error => error instanceof HostTransportError));
});

test("exhausted Console control slot terminally closes instead of silently refusing retirement", async t => {
  const { client, commands } = fixture(t, "console-retire-pending"); await client.bootstrap(selection);
  const a = client.beginConsoleLogin(consoleLogin);
  const b = client.beginConsoleLogin({ ...consoleLogin, login_scope_id: "scope-b" });
  await Promise.all([a.outcome, b.outcome]);
  const pending = Array.from({ length: 32 }, () => client.openComponent("pending").catch(error => error));
  const first = a.retire().catch(error => error);
  // Let the same serialized writer deliver the reserved control before testing
  // capacity exhaustion; this is protocol transport, not native settlement.
  await delay(20);
  await assert.rejects(b.retire(), HostTransportError);
  assert.ok(await first instanceof HostTransportError);
  assert.ok((await Promise.all(pending)).every(error => error instanceof HostTransportError));
  await assert.rejects(client.openComponent("after-loss"), HostTransportError);
  assert.equal(commands().filter(row => row.command.kind === "retire_console_login").length, 1);
});

for (const mode of ["console-wrong-enrollment", "console-retire-malformed"]) {
  test(`${mode} refuses the exact response and closes the pipe`, async t => {
    const { client } = fixture(t, mode); await client.bootstrap(selection);
    const login = client.beginConsoleLogin(consoleLogin);
    if (mode === "console-wrong-enrollment") await assert.rejects(login.outcome, HostTransportError);
    else { await login.outcome; await assert.rejects(login.retire(), HostTransportError); }
    await assert.rejects(client.openComponent("after-refusal"), HostTransportError);
  });
}

test("Console labels refuse extra authority and invalid identity before writing", async t => {
  const { client, commands } = fixture(t); await client.bootstrap(selection);
  for (const changed of [{ ...consoleLogin, bearer: "secret" }, { ...consoleLogin, subject: "" },
    { ...consoleLogin, issuer: "\n" }, { ...consoleLogin, expires_at_ms: 2 ** 53 }]) {
    assert.throws(() => client.beginConsoleLogin(changed), HostControlError);
  }
  await delay(10); assert.equal(commands().length, 1);
});

// These use the real trusted-launcher endpoint client and an inert sendInit
// callback. They do not qualify native construction, callbacks or readiness.
test("replacement startup joins exact selected bytes to one same-child native init", async t => {
  const { createHash } = await import("node:crypto");
  const { client, commands } = fixture(t);
  const bytes = Buffer.from('{"update_id":"selected-transaction"}\n');
  const startup = { selection: { path: "/private/update.json", sha256: createHash("sha256").update(bytes).digest("hex") },
    prior_tool_policy: artifact, association_directory: "/private/update-associations" };
  let calls = 0;
  const params = { runtime_options: { declare_spec_update: { expected_revision: 11 } } };
  const settled = { native: "actual-init-result" };
  assert.equal(await client.startApplicationUpdate(startup, bytes, params, async actual => {
    calls++;
    assert.deepEqual(actual, { ...params, prepared_application: {
      bundle_base64: bytes.toString("base64"), digest: startup.selection.sha256 } });
    return settled;
  }), settled);
  assert.equal(calls, 1);
  await client.openComponent("after-init");
  await assert.rejects(client.startApplicationUpdate(startup, bytes, params, async () => { calls++; }), HostControlError);
  assert.equal(calls, 1);
  await delay(10);
  assert.deepEqual(commands().map(row => row.command.kind), ["replace_application", "open_component"]);
  assert.deepEqual(commands()[0].command.startup, startup);
});

test("replacement conflicts and digest mismatch fail before private selection", async t => {
  const { createHash } = await import("node:crypto");
  const { client, commands } = fixture(t);
  const bytes = Buffer.from("selected");
  const startup = { selection: { ...artifact, sha256: createHash("sha256").update(bytes).digest("hex") },
    prior_tool_policy: artifact, association_directory: "/private/output" };
  let called = false;
  for (const params of [{ mob_config: "other" }, { application_tool_policies: [] }, { prepared_application: {} }]) {
    await assert.rejects(client.startApplicationUpdate(startup, bytes, params, async () => { called = true; }), HostControlError);
  }
  await assert.rejects(client.startApplicationUpdate(startup, Buffer.from("changed"), {}, async () => { called = true; }), HostControlError);
  assert.equal(called, false); assert.deepEqual(commands(), []);
  await client.bootstrap(selection);
});

for (const mode of ["bootstrap-refused", "bootstrap-pending"]) {
  test(`replacement ${mode} never sends native init or retries`, async t => {
    const { createHash } = await import("node:crypto");
    const { client, commands, ready, observeCommands, close } = fixture(t, mode);
    await ready();
    const bytes = Buffer.from("selected");
    const startup = { selection: { ...artifact, sha256: createHash("sha256").update(bytes).digest("hex") },
      prior_tool_policy: artifact, association_directory: "/private/output" };
    let called = false;
    await assert.rejects(client.startApplicationUpdate(startup, bytes, {}, async () => { called = true; }), error => {
      if (mode === "bootstrap-refused") {
        assert.ok(error instanceof HostRefusedError); assert.equal(error.refusal, "denied");
      } else assert.ok(error instanceof HostObservationTimeout);
      return true;
    });
    await assert.rejects(client.replaceApplication(startup), HostControlError);
    assert.equal(called, false);
    await observeCommands(1); await close();
    assert.deepEqual(commands(), [{ id: 1, command: { kind: "replace_application", startup } }]);
  });
}

test("entered native init failure stays owned and never replays replacement", async t => {
  const { createHash } = await import("node:crypto");
  const { client, commands, ready, observeCommands, close } = fixture(t);
  await ready();
  const bytes = Buffer.from("selected");
  const startup = { selection: { ...artifact, sha256: createHash("sha256").update(bytes).digest("hex") },
    prior_tool_policy: artifact, association_directory: "/private/output" };
  const failure = new Error("native init outcome unavailable");
  let calls = 0;
  await assert.rejects(client.startApplicationUpdate(startup, bytes, {}, async () => { calls++; throw failure; }), error => error === failure);
  assert.equal(calls, 1);
  // Keep the same endpoint usable for the launcher's actual cleanup.
  await client.beginCloseActivation("shutdown");
  await assert.rejects(client.replaceApplication(startup), HostControlError);
  await observeCommands(2); await close();
  assert.equal(calls, 1);
  assert.deepEqual(commands().map(row => row.command.kind), ["replace_application", "begin_close_activation"]);
});

// One explicit object-mode Buffer reaches the real private decoder as one
// iteration. Pipe writes alone do not guarantee this scheduling boundary.
// These are inert component endpoints, not a native child or finish receipt.
function sameChunkBootstrapFixture(t) {
  const commands = [];
  const responses = new Readable({ objectMode: true, read() {} });
  const writer = new Writable({ write(chunk, _encoding, done) {
    const request = JSON.parse(chunk.toString("utf8"));
    commands.push(request);
    responses.push(Buffer.from(JSON.stringify({ version: 1, id: request.id,
      result: { kind: "bootstrap_accepted" } }) + "\n" + '{"version":1,\n'));
    done();
  } });
  const client = new PrivateHostClient(writer, responses, { timeoutMs: 1000 });
  t.after(async () => {
    await client.close();
    assert.equal(writer.destroyed, true); assert.equal(responses.destroyed, true);
  });
  return { client, commands };
}

for (const operation of ["bootstrap", "replaceApplication", "startApplicationUpdate"]) {
  test(`${operation} refuses accepted selection followed by malformed data in the same chunk`, async t => {
    const { createHash } = await import("node:crypto");
    const { client, commands } = sameChunkBootstrapFixture(t);
    const bytes = Buffer.from("selected");
    const startup = { selection: { ...artifact, sha256: createHash("sha256").update(bytes).digest("hex") },
      prior_tool_policy: artifact, association_directory: "/private/output" };
    let initCalls = 0, failure;
    const sendInit = async () => { initCalls++; return { native: "must-not-enter" }; };
    const begin = () => operation === "bootstrap" ? client.bootstrap(selection)
      : operation === "replaceApplication" ? client.replaceApplication(startup)
      : client.startApplicationUpdate(startup, bytes, {}, sendInit);
    await assert.rejects(begin(), error => {
      assert.ok(error instanceof HostTransportError);
      assert.equal(error.message, "native host response stream failed; settlement is unknown");
      failure = error; return true;
    });
    // The original reader failure remains the owner, including after close.
    await assert.rejects(client.openComponent("after-failure"), error => error === failure);
    await assert.rejects(client.bootstrap(selection), HostControlError);
    await assert.rejects(client.replaceApplication(startup), HostControlError);
    await assert.rejects(client.startApplicationUpdate(startup, bytes, {}, sendInit), HostControlError);
    await client.close();
    await assert.rejects(client.openComponent("after-close"), error => error === failure);
    assert.equal(initCalls, 0);
    assert.deepEqual(commands, [{ version: 1, id: 1, command: operation === "bootstrap"
      ? { kind: "bootstrap", selection } : { kind: "replace_application", startup } }]);
  });
}
