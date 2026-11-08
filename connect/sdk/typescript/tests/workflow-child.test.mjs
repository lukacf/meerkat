// Private stream protocol controls only, not native workflow/authority proof.
import test from "node:test";
import assert from "node:assert/strict";
import { Readable, Writable } from "node:stream";
import { PrivateHostClient, HostControlError, HostTransportError, HostObservationTimeout, HostRefusedError } from "../dist/private-host.js";
const pin = { path: "/selected", sha256: "a".repeat(64) };
const bootstrap = { deployment_root: "/application", activation: pin, policy_manifest: pin, local_policy: pin };
const original = name => ({ route: "route", original: name, event_sha256: "a".repeat(64) });
const selected = () => ({ parent: original("P"), planning: original("G"),
  submission: { original: original("A"), content: "actual child request",
    transport_authentication: { resource_id: "event", revision: "1", sha256: "b".repeat(64) } },
  child: { realm_id: "realm", namespace: "work", item_id: "child" }, child_owner: { kind: "agent", id: "actual-child-owner" },
  issuance_id: "child-root", targets: [{ target: { kind: "registered_tool_bundle_operation", bundle: "meerkat_toolkit.setup_accounts",
    operation: "toolkit_request_public_service" }, issuance_id: "child-effect" }], output_audiences: ["actual-audience"] });
const prepared = value => ({ kind: "workflow_child_prepared", original: value.submission.original });
const close = { incarnation: "00000000-0000-0000-0000-000000000001", close_id: "00000000-0000-0000-0000-000000000002", scope: { kind: "activation" } };
const closing = { kind: "close", observation: { handle: close, state: "closing", failure: null } };
function fixture(t, replyFor) {
  const commands = [], responses = new Readable({ read() {} });
  let enteredResolve; const entered = new Promise(resolve => { enteredResolve = resolve; });
  const send = (request, result) => responses.push(Buffer.from(JSON.stringify({ version: 1, id: request.id, result }) + "\n"));
  const writer = new Writable({ write(bytes, _encoding, done) {
    const request = JSON.parse(bytes.toString()); commands.push(request); done();
    if (request.command.kind !== "bootstrap") enteredResolve(request);
    const reply = request.command.kind === "bootstrap" ? { kind: "bootstrap_accepted" } : replyFor(request);
    if (reply) send(request, reply);
  } });
  const client = new PrivateHostClient(writer, responses, { timeoutMs: 1000 });
  t.after(async () => { await client.close(); assert.equal(writer.destroyed, true); assert.equal(responses.destroyed, true); });
  return { client, commands, send, responses, entered };
}

test("exact child selectors snapshot before write and never submit", async t => {
  const value = selected(), expected = structuredClone(value);
  const f = fixture(t, () => null); await f.client.bootstrap(bootstrap);
  const pending = f.client.prepareWorkflowChild(value);
  value.child.item_id = "changed"; value.targets[0].issuance_id = "changed";
  await f.entered;
  assert.deepEqual(f.commands[1].command, { kind: "prepare_workflow_child", request: expected });
  f.send(f.commands[1], prepared(expected));
  assert.deepEqual(await pending, expected.submission.original);
  assert.equal(f.commands.length, 2);
});

for (const mutation of ["original", "digest", "kind", "extra"]) test(`foreign or overstated ${mutation} reply refuses`, async t => {
  const value = selected(), reply = prepared(structuredClone(value));
  if (mutation === "original") reply.original.original = "other";
  else if (mutation === "digest") reply.original.event_sha256 = "c".repeat(64);
  else if (mutation === "kind") reply.kind = "original_submitted";
  else reply.admitted = true;
  const f = fixture(t, () => reply); await f.client.bootstrap(bootstrap);
  await assert.rejects(f.client.prepareWorkflowChild(value), HostControlError);
  await assert.rejects(f.client.prepareWorkflowChild(value), HostTransportError);
  assert.equal(f.commands.length, 2);
});

test("empty selectors are preserved for native refusal on the same usable channel", async t => {
  const value = { ...selected(), targets: [], output_audiences: [] };
  const f = fixture(t, request => request.command.kind === "prepare_workflow_child" ? { kind: "error", error: "denied" } : closing);
  await f.client.bootstrap(bootstrap);
  await assert.rejects(f.client.prepareWorkflowChild(value), error => error instanceof HostRefusedError && error.refusal === "denied");
  assert.deepEqual(f.commands[1].command.request, value);
  assert.equal((await f.client.observeClose(close)).state, "closing");
  assert.equal(f.commands.length, 3);
});

for (const code of ["denied", "unavailable", "observation_changed"]) test(`child ${code} stays exact without replay or submit`, async t => {
  const f = fixture(t, request => request.command.kind === "prepare_workflow_child" ? { kind: "error", error: code } : closing);
  await f.client.bootstrap(bootstrap);
  await assert.rejects(f.client.prepareWorkflowChild(selected()), error => error instanceof HostRefusedError && error.refusal === code);
  assert.equal((await f.client.observeClose(close)).state, "closing");
  assert.deepEqual(f.commands.map(row => row.command.kind), ["bootstrap", "prepare_workflow_child", "observe_close"]);
  assert.deepEqual(f.commands.map(row => row.id), [1, 2, 3]);
});


test("timeout drains late reply without replay or native submission", async t => {
  const value = selected();
  const f = fixture(t, request => request.command.kind === "observe_close" ? closing : null);
  await f.client.bootstrap(bootstrap);
  await assert.rejects(f.client.prepareWorkflowChild(value, { timeoutMs: 10 }), HostObservationTimeout);
  f.send(f.commands[1], prepared(value));
  assert.equal((await f.client.observeClose(close)).state, "closing");
  assert.deepEqual(f.commands.map(row => row.command.kind), ["bootstrap", "prepare_workflow_child", "observe_close"]);
});

test("caller transport close cannot manufacture native cleanup or resend", async t => {
  const f = fixture(t, () => null); await f.client.bootstrap(bootstrap);
  const pending = f.client.prepareWorkflowChild(selected());
  const rejected = assert.rejects(pending, HostTransportError);
  await f.entered;
  await f.client.close(); await rejected;
  assert.deepEqual(f.commands.map(row => row.command.kind), ["bootstrap", "prepare_workflow_child"]);
});

for (const change of ["child_extra", "owner", "grant", "audience"]) test(`invalid ${change} shape writes nothing`, async t => {
  const value = selected(), f = fixture(t, () => null); await f.client.bootstrap(bootstrap);
  if (change === "child_extra") value.child.permission = true;
  else if (change === "owner") value.child_owner.kind = "invented";
  else if (change === "grant") value.targets[0].grant = { authorized: true };
  else value.output_audiences = [true];
  await assert.rejects(f.client.prepareWorkflowChild(value), HostControlError);
  assert.equal(f.commands.length, 1);
});
