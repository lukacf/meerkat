import test from "node:test";
import assert from "node:assert/strict";
import { Readable, Writable } from "node:stream";
import { setTimeout as delay } from "node:timers/promises";
import { PrivateHostClient, HostControlError, HostRefusedError, HostTransportError } from "../dist/private-host.js";
import { MemberMcpApplication, MemberClassObservationTimeout, MemberClassListingUnknown } from "../dist/member-classes.js";
const artifact = { path: "/selected", sha256: "1".repeat(64) };
const bootstrap = { deployment_root: "/realm", activation: artifact, policy_manifest: artifact, local_policy: artifact };
const member = { mob_id: "application", member: "project_one", role: "project" };
const principal = { kind: "actor", id: "application-owner" };
const selected = { admission_id: "add-project-one", identity: "project-one", class: "project" };
function request() { return { id: selected.admission_id,
  instance: { class: "project", member: { member, logical_owner: principal, context_generation: 1, audience: { kind: "principal", principal } } },
  spec: { identity: selected.identity, profile: "project", addressability: "advertised", displayName: null, labels: {}, context: null, additionalInstructions: [], runtimeModeOverride: "turn_driven" },
  route: { id: "project-console", origin: { kind: "local_console", member, requester: principal, ingress_actor: principal }, logical_executor: principal,
    represented_subject: null, source_authority: principal, authentication_domain: "console", event_domain: "console" },
  controller: { target: { kind: "original_route", route: "project-console" }, issuance_id: "issue-project" } }; }
function settled() { return { state: "settled", native: { state: "result", result: { outcomes: { "project-one": { stage: "created", payload: { offset: -0.5 } } }, managed_edges: [] } }, authorization: { state: "succeeded" }, cleanup: { state: "succeeded" } }; }
function deferred() { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; }
function fixture(options = {}) {
  const frames = [], waiters = [];
  const reader = new Readable({ read() {} });
  const send = (id, result) => reader.push(Buffer.from(JSON.stringify({ version: 1, id, result }) + "\n"));
  const writer = new Writable({ write(bytes, _, done) {
    const value = JSON.parse(bytes.toString()); frames.push(value); done();
    if (value.command.kind === "bootstrap") send(value.id, { kind: "bootstrap_accepted" });
    waiters.splice(0).forEach(resolve => resolve());
  } });
  const client = new PrivateHostClient(writer, reader, { timeoutMs: 500, ...options });
  async function next(after = 1) { while (frames.length <= after) await new Promise(done => waiters.push(done)); return frames[after]; }
  return { client, frames, send, reader, next };
}
function listing(f, add) { f.send(add.id, { kind: "member_class_listing_required", operation_id: add.id, ...selected }); }
function ackListing(f, ack) { f.send(ack.id, { kind: "member_class_listing_acknowledged", operation_id: ack.command.operation.operation_id, ...selected }); }
function mcp(f, exchange = 1, bindings = null, selection = selected) { f.send(0, { kind: "member_class_mcp_required", exchange_id: exchange, selection, bindings }); }
function ackMcp(f, ack) { f.send(ack.id, { kind: "member_class_mcp_acknowledged", exchange_id: ack.command.operation.exchange_id }); }

// Real Node stream framing with inert native replies, not native execution.
test("held Add writes once, waits for actual listing commit, and preserves native result", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap); let writes = 0; const release = deferred();
  const attempt = f.client.memberClasses.beginAdd(request(), async () => { writes++; await release.promise; });
  const add = await f.next(); assert.equal(add.command.operation.spec.runtime_mode_override, "turn_driven");
  assert.equal(writes, 0); listing(f, add); await delay(0); assert.equal(writes, 1); assert.equal(f.frames.length, 2);
  release.resolve(); const ack = await f.next(2); assert.equal(ack.command.operation.accepted, true); ackListing(f, ack);
  await attempt.joinListing(); f.send(add.id, { kind: "member_class", observation: settled() });
  assert.deepEqual(await attempt.result(), settled()); assert.equal(writes, 1); await f.client.close();
});
test("timeout retains exact Add and late unsettled can be joined without replay", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap); let writes = 0;
  const attempt = f.client.memberClasses.beginAdd(request(), async () => { writes++; }, { timeoutMs: 10 });
  const add = await f.next(); await assert.rejects(attempt.result(), error => error instanceof MemberClassObservationTimeout && error.attempt === attempt);
  f.send(add.id, { kind: "member_class", observation: { state: "unsettled", settlement_id: add.id } }); await delay(0);
  const joined = f.client.memberClasses.join(add.id); const join = await f.next(2);
  f.send(join.id, { kind: "member_class", observation: settled() }); await joined; await attempt.joinListing();
  assert.equal(writes, 0); assert.equal(f.frames.filter(row => row.command.operation?.kind === "add").length, 1); await f.client.close();
});
for (const mutation of ["operation", "identity", "duplicate", "unknown-zero"]) test(`invalid ${mutation} callback closes pipe before an unrelated action`, async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap); let writes = 0; const release = deferred();
  const attempt = f.client.memberClasses.beginAdd(request(), async () => { writes++; await release.promise; }); const add = await f.next();
  if (mutation === "unknown-zero") f.send(0, { kind: "bootstrap_accepted" });
  else {
    const row = { kind: "member_class_listing_required", operation_id: add.id, ...selected };
    if (mutation === "operation") row.operation_id++;
    if (mutation === "identity") row.identity = "other";
    f.send(add.id, row); if (mutation === "duplicate") f.send(add.id, row);
  }
  await assert.rejects(attempt.result()); assert.equal(writes, mutation === "duplicate" ? 1 : 0);
  release.resolve(); await assert.rejects(attempt.joinListing(), MemberClassListingUnknown);
  await f.client.close().catch(() => {});
});
test("observe keeps retirement distinct from authorization and exact predecessor", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap);
  const pending = f.client.memberClasses.observe(selected.admission_id, member); const command = await f.next();
  const value = { state: "observed", record: { id: selected.admission_id, instance: request().instance, phase: { kind: "retired" } }, checkpoint: { admission: selected.admission_id, revision: 0 }, authorization: { state: "refused", error: "denied" } };
  f.send(command.id, { kind: "member_class", observation: value }); assert.deepEqual(await pending, value); await f.client.close();
});
test("operator MCP intent preserves native lifetime shape and refuses flattened authority before wire", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap);
  const lifetime = { bound: { bound: "window", window: { not_before_ms: 10, expires_at_ms: 20 } }, unresolved: [] };
  const services = [{ server: "own-server", operations: Array.from({ length: 256 }, (_, i) => `operation-${i}`), lifetime }];
  const mcp = { origin: "operator_self", console_enrollment_id: 7, services };
  const attempt = f.client.memberClasses.beginAdd({ ...request(), mcp }, async () => {});
  const add = await f.next(); assert.deepEqual(add.command.operation.mcp, mcp);
  f.send(add.id, { kind: "member_class", observation: { state: "reservation_not_accepted", error: "denied" } });
  await attempt.result(); await attempt.joinListing();
  for (const invalidLifetime of [
    { not_before_ms: 10, expires_at_ms: 20 },
    { bound: { bound: "unrestricted", window: { not_before_ms: 10, expires_at_ms: 20 } }, unresolved: [] },
    { bound: { bound: "window", window: { not_before_ms: 0, expires_at_ms: Number.MAX_SAFE_INTEGER + 1 } }, unresolved: [] },
    { bound: { bound: "empty" }, unresolved: {} },
  ]) assert.throws(() => f.client.memberClasses.beginAdd({ ...request(), mcp: { ...mcp, services: [{ ...services[0], lifetime: invalidLifetime }] } }, async () => {}), HostControlError);
  await delay(0); assert.equal(f.frames.length, 2); await f.client.close();
});
for (const bindings of [null, []]) test(`MCP ${bindings === null ? "lookup" : "explicit empty retain"} ACK uses exact durable callback`, async () => {
  const calls = [], gate = deferred(); const app = new MemberMcpApplication({
    async retain(value) { calls.push(["retain", value]); await gate.promise; },
    async lookup(value) { calls.push(["lookup", value]); await gate.promise; return { selection: value, bindings: ["binding-1"] }; },
  });
  const f = fixture({ memberMcp: app }); await f.client.bootstrap(bootstrap); mcp(f, 1, bindings); await delay(0);
  assert.equal(f.frames.length, 1); assert.equal(calls.length, 1); gate.resolve(); const ack = await f.next();
  assert.deepEqual(ack.command.operation.retained, { selection: selected, bindings: bindings ?? ["binding-1"] });
  ackMcp(f, ack); await delay(0); await f.client.close();
});
test("MCP native ACK refusal preserves pipe; application uncertainty remains joined", async () => {
  const app = new MemberMcpApplication({ async retain() {}, async lookup(selection) { return { selection, bindings: [] }; } });
  const f = fixture({ memberMcp: app }); await f.client.bootstrap(bootstrap); mcp(f); const ack = await f.next();
  f.send(ack.id, { kind: "error", error: "denied" }); await delay(0); f.client.assertAvailable();
  const observed = f.client.memberClasses.retire("admission", member); const command = await f.next(2);
  f.send(command.id, { kind: "member_class", observation: { state: "retired" } }); await observed;
  await assert.rejects(f.client.close(), AggregateError);
});
for (const change of ["selection", "absent", "replay", "out-of-order"]) test(`MCP refuses ${change} without fabricating an empty binding`, async () => {
  let calls = 0;
  const app = new MemberMcpApplication({ async retain() { calls++; }, async lookup(value) { calls++; if (change === "absent") throw new Error("absent"); return { selection: change === "selection" ? { ...value, identity: "other" } : value, bindings: [] }; } });
  const f = fixture({ memberMcp: app }); await f.client.bootstrap(bootstrap); mcp(f, 2); const ack = await f.next();
  if (change === "selection" || change === "absent") assert.equal(ack.command.operation.retained, null);
  ackMcp(f, ack); await delay(0);
  if (change === "replay" || change === "out-of-order") { mcp(f, change === "replay" ? 2 : 1); await delay(0); assert.throws(() => f.client.assertAvailable(), HostTransportError); }
  assert.equal(calls, 1); await f.client.close().catch(() => {});
});
test("close joins entered application write, retains lost ACK uncertainty, and fences later entry", async () => {
  let calls = 0; const entered = deferred(), release = deferred();
  const app = new MemberMcpApplication({ async retain() { calls++; entered.resolve(); await release.promise; }, async lookup() { throw new Error("unused"); } });
  const f = fixture({ memberMcp: app }); await f.client.bootstrap(bootstrap); mcp(f, 1, []); await entered.promise;
  let done = false; const closed = f.client.close().finally(() => { done = true; }); void closed.catch(() => {});
  await delay(0); assert.equal(done, false); release.resolve(); await assert.rejects(closed, AggregateError); assert.equal(calls, 1);
  assert.equal(f.frames.length, 1, "no ACK is invented after pipe loss");
});
test("close before queued callback entry performs no application write", async () => {
  let calls = 0; const app = new MemberMcpApplication({ async retain() { calls++; }, async lookup(value) { calls++; return { selection: value, bindings: [] }; } });
  const f = fixture({ memberMcp: app }); await f.client.bootstrap(bootstrap); mcp(f); await f.client.close(); assert.equal(calls, 0);
});
test("invalid request and model-supplied authority fields are refused before wire", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap);
  for (const change of [value => { value.instance.member.setup_capability = {}; }, value => { value.route.application_parent = {}; }, value => { value.mcp = { origin: "agent", console_enrollment_id: 1, services: [] }; }]) {
    const value = request(); change(value); assert.throws(() => f.client.memberClasses.beginAdd(value, async () => {}), HostControlError);
  }
  assert.equal(f.frames.length, 1); await f.client.close();
});
test("Join retains original Add identity rather than accepting another member outcome", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap);
  const attempt = f.client.memberClasses.beginAdd(request(), async () => {}); const add = await f.next();
  f.send(add.id, { kind: "member_class", observation: { state: "unsettled", settlement_id: add.id } }); await attempt.result();
  const joined = f.client.memberClasses.join(add.id); const command = await f.next(2); const value = settled();
  value.native.result.outcomes = { other: { stage: "created" } }; f.send(command.id, { kind: "member_class", observation: value });
  await assert.rejects(joined, HostTransportError); await assert.rejects(attempt.joinListing(), MemberClassListingUnknown); await f.client.close();
});
for (const failedAck of ["denied", "unsendable"]) test(`listing ${failedAck} ACK preserves the correct transport boundary`, async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap);
  const attempt = f.client.memberClasses.beginAdd(request(), async () => {}); const add = await f.next();
  if (failedAck === "unsendable") {
    const call = f.client._memberCall.bind(f.client);
    f.client._memberCall = (operation, ...rest) => {
      if (operation.kind === "listing_result") throw new HostControlError("fixture prewrite refusal");
      return call(operation, ...rest);
    };
  }
  listing(f, add);
  if (failedAck === "denied") { const ack = await f.next(2); f.send(ack.id, { kind: "error", error: "denied" }); }
  await assert.rejects(attempt.joinListing(), MemberClassListingUnknown);
  if (failedAck === "denied") {
    f.client.assertAvailable(); f.send(add.id, { kind: "member_class", observation: settled() }); await attempt.result();
  } else { assert.throws(() => f.client.assertAvailable(), HostTransportError); await assert.rejects(attempt.result()); }
  await assert.rejects(f.client.close(), AggregateError);
});
test("explicit participant join before store close joins failed entered write and preserves the same failure on runtime close", async () => {
  const marker = new Error("fixture retention uncertain"), entered = deferred(), release = deferred(); let calls = 0;
  const app = new MemberMcpApplication({ async retain() { calls++; entered.resolve(); await release.promise; throw marker; }, async lookup() { throw new Error("unused"); } });
  const f = fixture({ memberMcp: app }); await f.client.bootstrap(bootstrap); mcp(f, 1, []); await entered.promise;
  const joined = app.join(); void joined.catch(() => {}); let completed = false; void joined.finally(() => { completed = true; }).catch(() => {});
  await delay(0); assert.equal(completed, false); release.resolve(); const ack = await f.next(); assert.equal(ack.command.operation.retained, null); ackMcp(f, ack);
  let first; await assert.rejects(joined, error => { first = error; return error.errors.includes(marker); });
  await assert.rejects(app.join(), error => error === first); assert.equal(calls, 1);
  await assert.rejects(f.client.close(), error => error.errors.includes(first));
});
test("close joins every entered listing even when another listing has already failed", async () => {
  const f = fixture(); await f.client.bootstrap(bootstrap);
  const firstEntered = deferred(), firstRelease = deferred(), secondEntered = deferred(), secondRelease = deferred();
  const first = f.client.memberClasses.beginAdd(request(), async () => { firstEntered.resolve(); await firstRelease.promise; throw new Error("first listing uncertain"); });
  const firstCommand = await f.next(); listing(f, firstCommand); await firstEntered.promise;
  const secondRequest = request(); secondRequest.id = "add-second"; secondRequest.spec.identity = "project-second";
  const second = f.client.memberClasses.beginAdd(secondRequest, async () => { secondEntered.resolve(); await secondRelease.promise; });
  const secondCommand = await f.next(2);
  f.send(secondCommand.id, { kind: "member_class_listing_required", operation_id: secondCommand.id,
    admission_id: secondRequest.id, identity: secondRequest.spec.identity, class: "project" });
  await secondEntered.promise;
  let done = false; const closed = f.client.close().finally(() => { done = true; }); void closed.catch(() => {});
  firstRelease.resolve(); await assert.rejects(first.joinListing(), MemberClassListingUnknown);
  await delay(0); assert.equal(done, false, "another entered write still owns its store");
  secondRelease.resolve(); await assert.rejects(second.joinListing(), MemberClassListingUnknown);
  await assert.rejects(closed, AggregateError); await assert.rejects(first.result()); await assert.rejects(second.result());
});
