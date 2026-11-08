// Real stream/listing ownership with inert native replies, not native acceptance.
import test from "node:test";
import assert from "node:assert/strict";
import { Readable, Writable } from "node:stream";
import { setTimeout as delay } from "node:timers/promises";
import { PrivateHostClient, HostControlError, HostRefusedError } from "../dist/private-host.js";
import { MemberClassObservationTimeout, MemberClassListingUnknown } from "../dist/member-classes.js";
const artifact = { path: "/selected/file", sha256: "a".repeat(64) };
const bootstrap = { deployment_root: "/selected", activation: artifact, policy_manifest: artifact, local_policy: artifact };
const principal = { kind: "actor", id: "owner" };
function selection() {
  const evidence = { resource_id: "event", revision: "1", sha256: "a".repeat(64) };
  return { admission_id: "admit-G", identity: "G", parent: { original: { route: "P-route", original: "P", event_sha256: "b".repeat(64) }, content: "parent", transport_authentication: evidence },
    instance: { class: "helper", member: { member: { mob_id: "mob", member: "G", role: "helper" }, logical_owner: principal, context_generation: 1, audience: { kind: "principal", principal } } },
    event: { route: "G-route", original: "G-original", occurrence: "occurrence", original_authentication: evidence, root_event: evidence, contributors: ["P"], content_sha256: "c".repeat(64) },
    content: "actual G", workflow: "workflow", parent_work: { realm_id: "realm", namespace: "work", item_id: "parent" }, parent_owner: { kind: "agent", id: "owner" }, cycle_label: "cycle",
    not_before_ms: 1, expires_at_ms: 99999, deadline_ms: 88888, max_waiters: 2, hold_timeout_ms: 60000, controller_issuance_id: "controller", planning_root_issuance_id: "root", planning_leaf_issuance_id: "leaf" };
}
function native() { return { state: "settled", native: { state: "result", result: { outcomes: { G: { restore: { stage: "minted" }, detail: -0.5 } }, managed_edges: [] } }, authorization: { state: "succeeded" }, cleanup: { state: "succeeded" } }; }
function outcome(request) { return { state: "prepared", parent: structuredClone(request.parent.original), planning: { route: "G-route", original: "G-original", event_sha256: "d".repeat(64) }, admission_id: request.admission_id, native: native() }; }
function deferred() { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; }
function fixture() {
  const frames = [], waiters = [], reader = new Readable({ read() {} });
  const send = (id, result) => reader.push(Buffer.from(JSON.stringify({ version: 1, id, result }) + "\n"));
  const writer = new Writable({ write(bytes, _, done) { const row = JSON.parse(bytes.toString()); frames.push(row); done();
    if (row.command.kind === "bootstrap") send(row.id, { kind: "bootstrap_accepted" }); waiters.splice(0).forEach(done => done()); } });
  const client = new PrivateHostClient(writer, reader, { timeoutMs: 500 });
  async function next(index = 1) { while (frames.length <= index) await new Promise(done => waiters.push(done)); return frames[index]; }
  return { frames, reader, client, send, next };
}
function listing(f, command, request) { f.send(command.id, { kind: "member_class_listing_required", operation_id: command.id, admission_id: request.admission_id, identity: request.identity, class: request.instance.class }); }
function ack(f, command) { const op = command.command.operation; f.send(command.id, { kind: "member_class_listing_acknowledged", operation_id: op.operation_id, admission_id: op.admission_id, identity: op.identity, class: op.class }); }

function nativeOperations() {
  return { not_before_ms: 1, expires_at_ms: 99999, targets: [
    { issuance_id: "peer-root", target: { kind: "peer", route: "actual-recipient-route", actions: ["message", "request"] } },
    { issuance_id: "directory-root", target: { kind: "peer_directory" } },
    { issuance_id: "memory-root", target: { kind: "recorder_identity", actions: ["read", "write", "delete"] } },
  ] };
}
for (const [replyKind, route] of [
  ["matching", "actual-recipient-route"], ["foreign", "actual-recipient-route"], ["refused", "actual-recipient-route"],
  ["matching", "realm/peer:worker@host_1.2-3"], ["matching", "p".repeat(192)],
]) test(`native operation selection preserves exact original and ${replyKind} for ${route}`, async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap);
  const source = selection().parent, request = nativeOperations();
  request.targets[0].target.route = route;
  const expected = structuredClone(request);
  const pending = f.client.prepareOriginalNativeOperations(source, request);
  const observed = replyKind === "matching" ? pending : assert.rejects(pending,
    replyKind === "refused" ? HostRefusedError : HostControlError);
  request.targets[0].target.route = "mutated-later";
  try {
    const command = await f.next();
    assert.deepEqual(command.command, { kind: "prepare_original_native_operations", submission: source, request: expected });
    if (replyKind === "refused") f.send(command.id, { kind: "error", error: "denied" });
    else { const original = structuredClone(source.original);
      if (replyKind === "foreign") original.original = "other";
      f.send(command.id, { kind: "native_operations_prepared", original }); }
    const result = await observed;
    if (replyKind === "matching") assert.deepEqual(result, source.original);
    assert.equal(f.frames.length, 2, "preparation cannot submit or retry an original");
  } finally { await f.client.close(); }
});
const invalidNativePeerRoutes = {
  route_whitespace: "peer one", route_empty_segment: "peer//two", route_dot_segment: "peer/./two",
  route_parent_segment: "peer/../two", route_non_ascii: "peer/\u00e5", route_too_long: "p".repeat(193),
  route_leading_symbol: "_peer", route_trailing_newline: "peer\n",
  route_line_separator: "peer\u2028", route_paragraph_separator: "peer\u2029",
};
for (const change of ["mob", "audience", "peer_key", "duplicate", "actions", "infinite", "unordered", "boolean", ...Object.keys(invalidNativePeerRoutes)]) test(`native operation ${change} request writes nothing`, async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap); const request = nativeOperations();
  if (change === "mob") request.targets[2].target.kind = "recorder_mob";
  else if (change === "audience") request.targets[0].target.audience = "invented";
  else if (change === "peer_key") request.targets[0].target.peer = "invented";
  else if (change === "duplicate") request.targets[1].issuance_id = "peer-root";
  else if (change === "actions") request.targets[2].target.actions = ["propose"];
  else if (change === "infinite") request.expires_at_ms = Infinity;
  else if (change === "unordered") request.targets[2].target.actions = ["write", "read"];
  else if (Object.hasOwn(invalidNativePeerRoutes, change)) request.targets[0].target.route = invalidNativePeerRoutes[change];
  else request.not_before_ms = true;
  try { await assert.rejects(f.client.prepareOriginalNativeOperations(selection().parent, request), HostControlError);
    assert.equal(f.frames.length, 1); } finally { await f.client.close(); }
});

test("actual commissioning command snapshots request and retains listing before native result", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap); const request = selection(), gate = deferred(); let writes = 0;
  const attempt = f.client.beginWorkflowCommission(request, async () => { writes++; await gate.promise; });
  request.instance.class = "changed-after-call";
  const command = await f.next(); assert.equal(command.command.kind, "commission_workflow"); assert.equal(command.command.request.instance.class, "helper");
  listing(f, command, selection()); await delay(0); assert.equal(writes, 1); assert.equal(f.frames.length, 2);
  gate.resolve(); const acknowledgement = await f.next(2); assert.equal(acknowledgement.command.operation.accepted, true); ack(f, acknowledgement);
  await attempt.joinListing(); f.send(command.id, { kind: "workflow_commissioned", outcome: outcome(request) });
  assert.deepEqual(await attempt.result(), outcome(request)); assert.deepEqual(await attempt.result(), outcome(request));
  assert.equal(writes, 1); await f.client.close();
});
for (const state of ["native", "created_without_planning", "add_not_accepted", "preparation_refused"]) test(`preserves distinct ${state} outcome`, async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap); const request = selection(); let writes = 0;
  const attempt = f.client.beginWorkflowCommission(request, async () => { writes++; }); const command = await f.next();
  const value = {
    native: { state, outcome: { state: "preflight_refused", error: "denied", cleanup: { state: "succeeded" } } },
    created_without_planning: { ...outcome(request), state, error: "unavailable" },
    add_not_accepted: { state, native: { state: "reservation_not_accepted", error: "denied" }, retirement: { state: "refused", error: "not_settled" } },
    preparation_refused: { state, error: "unavailable", retirement: { state: "succeeded" }, physical_cleanup: null },
  }[state];
  f.send(command.id, { kind: "workflow_commissioned", outcome: value }); assert.deepEqual(await attempt.result(), value);
  await attempt.joinListing(); assert.equal(writes, 0); await f.client.close();
});
for (const mutation of ["parent", "planning", "admission", "native_identity", "cleanup", "extra"]) test(`rejects ${mutation} result mismatch without success`, async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap); const request = selection();
  const attempt = f.client.beginWorkflowCommission(request, async () => {}); const command = await f.next(), value = outcome(request);
  if (mutation === "parent") value.parent.original = "other";
  else if (mutation === "planning") value.planning.route = "other";
  else if (mutation === "admission") value.admission_id = "other";
  else if (mutation === "native_identity") value.native.native.result.outcomes = { other: {} };
  else if (mutation === "cleanup") value.native.cleanup = { state: "refused", error: "not_settled" };
  else value.completed = true;
  f.send(command.id, { kind: "workflow_commissioned", outcome: value }); await assert.rejects(attempt.result());
  assert.throws(() => f.client.beginWorkflowCommission(request, async () => {})); await f.client.close().catch(() => {});
});
test("timeout retains entered publication and drains late result without another command", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap); const request = selection(), gate = deferred(); let writes = 0;
  const attempt = f.client.beginWorkflowCommission(request, async () => { writes++; await gate.promise; }, { timeoutMs: 15 });
  const command = await f.next(); listing(f, command, request);
  await assert.rejects(attempt.result(), error => error instanceof MemberClassObservationTimeout && error.attempt === attempt);
  gate.resolve(); const acknowledgement = await f.next(2); ack(f, acknowledgement); await attempt.joinListing();
  f.send(command.id, { kind: "workflow_commissioned", outcome: outcome(request) }); await delay(0);
  assert.equal(f.frames.filter(row => row.command.kind === "commission_workflow").length, 1); assert.equal(writes, 1); await f.client.close();
});
test("typed refusal is local and an explicit same-id second call remains a separate command", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap);
  for (let i = 0; i < 2; i++) {
    const attempt = f.client.beginWorkflowCommission(selection(), async () => assert.fail("unentered")); const command = await f.next(i + 1);
    f.send(command.id, { kind: "error", error: "denied" }); await assert.rejects(attempt.result(), HostRefusedError); await attempt.joinListing();
  }
  assert.equal(f.frames.length, 3); await f.client.close();
});
test("pipe loss does not cancel entered write and close joins it before unknown ACK", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap); const request = selection(), gate = deferred(); let entered = false, finished = false;
  const attempt = f.client.beginWorkflowCommission(request, async () => { entered = true; await gate.promise; finished = true; });
  const command = await f.next(); listing(f, command, request); await delay(0); assert.equal(entered, true);
  f.reader.push(null); await assert.rejects(attempt.result()); let closed = false;
  const closing = f.client.close().catch(() => {}).then(() => { closed = true; }); await delay(0); assert.equal(closed, false);
  gate.resolve(); await assert.rejects(attempt.joinListing(), MemberClassListingUnknown); await closing; assert.equal(finished, true);
});
for (const mutation of ["authority", "number", "instance_extra"]) test(`invalid ${mutation} request writes nothing`, async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap); const request = selection();
  if (mutation === "authority") request.grant = "invented";
  else if (mutation === "number") request.max_waiters = true;
  else request.instance.grant = "invented";
  assert.throws(() => f.client.beginWorkflowCommission(request, async () => {}), HostControlError);
  await delay(0); assert.equal(f.frames.length, 1); await f.client.close();
});
test("late typed refusal releases unentered listing after observation timeout", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap);
  const attempt = f.client.beginWorkflowCommission(selection(), async () => assert.fail("unentered"), { timeoutMs: 10 });
  const command = await f.next(); await assert.rejects(attempt.result(), MemberClassObservationTimeout);
  f.send(command.id, { kind: "error", error: "denied" });
  await Promise.race([attempt.joinListing(), delay(500).then(() => assert.fail("listing not released"))]);
  await f.client.close();
});
test("async convenience preserves native refusal without replay", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap);
  const result = f.client.commissionWorkflow(selection(), async () => {}); const command = await f.next();
  f.send(command.id, { kind: "error", error: "denied" }); await assert.rejects(result, HostRefusedError);
  assert.equal(f.frames.length, 2); await f.client.close();
});

for (const replyKind of ["matching", "wrong_parent", "refused"]) test(`parent preparation preserves exact selection and ${replyKind}`, async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap); const request = selection();
  const task = f.client.prepareWorkflowParent(request, "p-root", "p-leaf");
  const observed = replyKind === "matching" ? task : assert.rejects(task,
    replyKind === "refused" ? HostRefusedError : HostControlError);
  try {
    const command = await f.next();
    assert.deepEqual(command.command, { kind: "prepare_workflow_parent", request: {
      commission: request, parent_root_issuance_id: "p-root", parent_leaf_issuance_id: "p-leaf" } });
    if (replyKind === "refused") f.send(command.id, { kind: "error", error: "denied" });
    else {
      const parent = structuredClone(request.parent.original);
      if (replyKind === "wrong_parent") parent.original = "other";
      f.send(command.id, { kind: "workflow_parent_prepared", original: parent });
    }
    const result = await observed;
    if (replyKind === "matching") assert.deepEqual(result, request.parent.original);
  } finally { await f.client.close(); }
});

test("parent preparation refuses aliased IDs before transport", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap);
  await assert.rejects(f.client.prepareWorkflowParent(selection(), "root", "p-leaf"), HostControlError);
  assert.equal(f.frames.length, 1); await f.client.close();
});
