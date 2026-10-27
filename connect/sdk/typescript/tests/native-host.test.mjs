import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, writeFileSync, chmodSync, existsSync, mkdirSync,
  rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
// Required selected peer, deliberately not an optional import or skipped suite.
import { Meta, PersistentTransport, GatewayStartupCleanupError, InitOutcomeUnknownError, StorageResolutionError } from "@rkat/meta-sdk";
import { ToolkitHost, ToolkitApplicationUpdate, ToolkitApplicationPreparation } from "../dist/native-host.js";
import { HostControlError, HostObservationTimeout, HostRefusedError, HostTransportError, PrivateHostClient } from "../dist/private-host.js";

const pin = { path: "/private/original.json", sha256: "1".repeat(64) };
const original = { deployment_root: "/private/original-owner", activation: pin, policy_manifest: pin, local_policy: pin };

function fixture(t, mode = "ready") {
  const directory = mkdtempSync(join(tmpdir(), "toolkit-native-host-"));
  const gateway = join(directory, `${mode}.mjs`);
  writeFileSync(gateway, readFileSync(new URL("./fixtures/native-host-gateway.mjs", import.meta.url)));
  chmodSync(gateway, 0o755);
  const bytes = Buffer.from('{"fixture":"exact update selection bytes, no native authority"}\n');
  const selection = { path: join(directory, "selection.json"), sha256: createHash("sha256").update(bytes).digest("hex") };
  writeFileSync(selection.path, bytes);
  const startup = { selection, prior_tool_policy: { path: join(directory, "prior.json"), sha256: "2".repeat(64) },
    association_directory: join(directory, "associations") };
  mkdirSync(startup.association_directory);
  const runtimes = new Set();
  const rows = () => existsSync(`${gateway}.log`)
    ? readFileSync(`${gateway}.log`, "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line)) : [];
  t.after(async () => {
    try {
      for (const runtime of runtimes) await runtime.shutdown();
      for (const start of rows().filter(row => row.kind === "start")) {
        const exit = rows().filter(row => row.kind === "exit" && row.pid === start.pid);
        assert.equal(exit.length, 1, "native stop must join each inert child");
        assert.equal(exit[0].code, mode === "exit-nonzero" ? 7 : 0);
        assert.ok(rows().some(row => row.pid === start.pid && row.kind === "native-eof"));
      }
    } finally { rmSync(directory, { recursive: true, force: true }); }
  });
  let callbackCount = 0;
  function builder() {
    return Meta.builder().gateway(gateway).gatewayTimeoutMs(2000).initDeadline(2000)
      .persistentState(join(directory, "native-state"))
      .rosterProvider({ async roster(context) {
        assert.deepEqual(context, { mobDefinition: null, previousIdentities: [] });
        callbackCount += 1; return [];
      } });
  }
  return { directory, gateway, bytes, startup, rows, builder, callbackCount: () => callbackCount,
    remember(runtime) { runtimes.add(runtime); return runtime; } };
}

function nativeRequests(f, method) { return f.rows().filter(row => row.kind === "native" && row.request.method === method); }
function privateRequests(f) { return f.rows().filter(row => row.kind === "private"); }

function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}

// The actual selected Meta builder, callback dispatcher, accepted/settled init
// and shutdown owners run against an inert compiler protocol. Its selected
// manifests are mechanical inputs, not native policy/installation evidence.
test("fresh preparation joins same-child callbacks, trusted finalizer, adoption and native Ready", async t => {
  const f = fixture(t, "prepare-ready"); let calls = 0;
  const supplied = { plan: structuredClone(f.startup.selection) };
  const host = new ToolkitApplicationPreparation(supplied, async manifest => {
    calls += 1;
    assert.deepEqual(manifest, { path: "/private/completed-manifest.json", sha256: "4".repeat(64) });
    assert.ok(Object.isFrozen(manifest));
    assert.throws(() => host.hostControl, /bootstrap has not completed/);
    return structuredClone(original);
  });
  supplied.plan.path = "/changed-after-construction";
  const runtime = f.remember(await host.build(f.builder()));
  assert.equal(runtime.isRunning, true); assert.equal(calls, 1); assert.equal(f.callbackCount(), 1);
  assert.deepEqual(privateRequests(f).map(row => row.request), [
    { version: 1, id: 1, command: { kind: "prepare_application", plan: f.startup.selection } },
    { version: 1, id: 2, command: { kind: "bootstrap", selection: original } },
  ]);
  const input = nativeRequests(f, "meta/init")[0].request.params;
  assert.equal(input.init_protocol, "accepted_then_settled");
  assert.deepEqual(input.prepared_application, { bundle_base64: f.bytes.toString("base64"), digest: f.startup.selection.sha256 });
  assert.deepEqual(host.preparationAttempt.selection, original);
  assert.equal(host.settlement.native.value, runtime);
  assert.equal(host.settlement.finalization.status, "fulfilled");
  assert.equal(host.settlement.privateClose.status, "not_requested");
  assert.equal((await host.hostControl.openComponent("frontend")).state, "open");
  await runtime.shutdown();
  await assert.rejects(host.build(f.builder()), /already started/);
  await assert.rejects(runtime.connect(), /already started/);
  assert.equal(f.rows().filter(row => row.kind === "start").length, 1);
  assert.equal(nativeRequests(f, "meta/init").length, 1);
});

test("fresh preparation preserves actual native failure and the finalizer's completed selection", async t => {
  const f = fixture(t, "prepare-native-failed");
  const host = new ToolkitApplicationPreparation({ plan: f.startup.selection }, async () => original);
  let failure;
  await assert.rejects(host.build(f.builder()), error => {
    failure = error; assert.ok(error instanceof StorageResolutionError);
    assert.equal(error.message, "fixture entered preparation failed");
    assert.equal(error.data.durable_effects, "possible"); return true;
  });
  assert.equal(host.settlement.native.reason, failure);
  assert.equal(host.settlement.finalization.status, "fulfilled");
  assert.deepEqual(host.preparationAttempt.selection, original);
  await assert.rejects(host.build(f.builder()), /already started/);
  assert.equal(privateRequests(f).length, 2);
});

test("native Ready cannot finish a build whose adoption acknowledgement was lost", async t => {
  const f = fixture(t, "prepare-adopt-lost");
  const host = new ToolkitApplicationPreparation({ plan: f.startup.selection }, async () => original, { timeoutMs: 1000 });
  await assert.rejects(host.build(f.builder()), HostObservationTimeout);
  assert.equal(host.settlement.native.status, "fulfilled");
  assert.equal(host.settlement.nativeClose.status, "fulfilled");
  assert.equal(host.settlement.native.value.isRunning, false);
  assert.equal(host.settlement.finalization.status, "rejected");
  assert.deepEqual(host.preparationAttempt.selection, original);
  assert.equal(nativeRequests(f, "meta/shutdown").length, 1);
  assert.equal(privateRequests(f).length, 2);
});

test("native Ready plus lost adoption retains the same runtime when cleanup rejects", async t => {
  const f = fixture(t, "prepare-adopt-lost");
  const host = new ToolkitApplicationPreparation({ plan: f.startup.selection }, async () => original, { timeoutMs: 1000 });
  const builder = f.builder(), actualBuild = builder.build.bind(builder);
  let nativeRuntime, shutdownCalls = 0;
  builder.build = () => actualBuild().then(runtime => {
    nativeRuntime = f.remember(runtime);
    const shutdown = runtime.shutdown.bind(runtime);
    runtime.shutdown = () => { shutdownCalls += 1; return shutdown(); };
    return runtime;
  });
  const stop = PersistentTransport.prototype.stop;
  const cleanupFailure = Object.create(null);
  PersistentTransport.prototype.stop = async function() { throw cleanupFailure; };
  let failure;
  try {
    await assert.rejects(host.build(builder), error => {
      if (error instanceof GatewayStartupCleanupError) f.remember(error.cleanup);
      failure = error; assert.ok(error instanceof GatewayStartupCleanupError);
      assert.ok(error.startupError instanceof HostObservationTimeout);
      assert.equal(error.startupError, host.settlement.finalization.reason);
      assert.equal(error.cleanupError, cleanupFailure); return true;
    });
  } finally { PersistentTransport.prototype.stop = stop; }
  assert.equal(host.settlement.native.value, nativeRuntime);
  assert.equal(host.settlement.nativeClose.reason, cleanupFailure);
  assert.equal(host.settlement.privateClose.status, "fulfilled");
  assert.equal(shutdownCalls, 1);
  assert.deepEqual(failure.cleanup.lastTermination(), nativeRuntime.lastGatewayTermination());
  await failure.cleanup.shutdown();
  assert.equal(shutdownCalls, 2, "cleanup reuses the actual native runtime that completed init");
  assert.deepEqual(failure.cleanup.lastTermination(), nativeRuntime.lastGatewayTermination());
  assert.deepEqual(host.preparationAttempt.selection, original);
  await assert.rejects(host.build(f.builder()), /already started/);
  assert.equal(privateRequests(f).length, 2, "cleanup cannot reissue adoption");
  assert.equal(f.rows().filter(row => row.kind === "start").length, 1);
  assert.equal(f.rows().filter(row => row.kind === "exit").length, 1);
});

test("abort before compiler completion fences finalizer entry and joins the same native init", async t => {
  const f = fixture(t, "prepare-pending"); const abort = new AbortController(); let calls = 0;
  const host = new ToolkitApplicationPreparation({ plan: f.startup.selection }, async () => { calls += 1; return original; });
  const send = PersistentTransport.prototype.sendAsync;
  PersistentTransport.prototype.sendAsync = async function(request, options) {
    const result = await send.call(this, request, options);
    if (request.method === "meta/init") abort.abort();
    return result;
  };
  try { await assert.rejects(host.build(f.builder(), { signal: abort.signal }), StorageResolutionError); }
  finally { PersistentTransport.prototype.sendAsync = send; }
  assert.equal(calls, 0); assert.equal(host.settlement.cancelled, true);
  assert.equal(host.preparationAttempt.selection, null);
  assert.equal(host.settlement.native.status, "rejected");
  assert.equal(host.settlement.privateClose.status, "fulfilled");
  assert.equal(privateRequests(f).length, 1);
  assert.equal(f.rows().filter(row => row.kind === "exit").length, 1);
});

test("abort after finalizer entry retains its result and cannot return before it settles", async t => {
  const f = fixture(t, "prepare-ready"); const abort = new AbortController();
  const entered = deferred(), release = deferred(), stopped = deferred();
  const host = new ToolkitApplicationPreparation({ plan: f.startup.selection }, async () => {
    entered.resolve(); await release.promise; return original;
  });
  const stop = PersistentTransport.prototype.stop;
  PersistentTransport.prototype.stop = async function() {
    try { return await stop.call(this); } finally { stopped.resolve(); }
  };
  let finished = false;
  const build = host.build(f.builder(), { signal: abort.signal }).finally(() => { finished = true; });
  void build.catch(() => {});
  try {
    await entered.promise; abort.abort(); await stopped.promise;
    await Promise.resolve(); assert.equal(finished, false, "the entered finalizer is still owned after native stop");
    release.resolve();
    await assert.rejects(build, StorageResolutionError);
  } finally { release.resolve(); PersistentTransport.prototype.stop = stop; }
  assert.equal(host.settlement.cancelled, true);
  assert.deepEqual(host.preparationAttempt.selection, original);
  assert.equal(host.settlement.finalization.status, "rejected", "closed adoption remains distinct from returned publication");
  assert.equal(privateRequests(f).length, 1, "closed channel cannot replay final adoption");
  assert.equal(f.rows().filter(row => row.kind === "exit").length, 1);
});

test("finalizer failure retains the exact value while native failure keeps its public type", async t => {
  const f = fixture(t, "prepare-ready"); const failure = Object.create(null);
  const host = new ToolkitApplicationPreparation({ plan: f.startup.selection }, async () => { throw failure; });
  await assert.rejects(host.build(f.builder()), StorageResolutionError);
  assert.equal(host.settlement.finalization.reason, failure);
  assert.equal(host.preparationAttempt.selection, null);
  assert.equal(privateRequests(f).length, 1);
});

test("fresh build preserves the native cleanup error identity and same-runtime rejoin capability", async t => {
  const f = fixture(t, "prepare-refused");
  const host = new ToolkitApplicationPreparation({ plan: f.startup.selection }, async () => { throw new Error("must not finalize"); });
  const builder = f.builder(), actualBuild = builder.build.bind(builder);
  let nativeFailure;
  builder.build = () => actualBuild().catch(error => { nativeFailure = error; throw error; });
  const stop = PersistentTransport.prototype.stop;
  const cleanupFailure = Object.create(null);
  PersistentTransport.prototype.stop = async function() { throw cleanupFailure; };
  let failure;
  try {
    await assert.rejects(host.build(builder), error => {
      if (error instanceof GatewayStartupCleanupError) f.remember(error.cleanup);
      failure = error; assert.equal(error, nativeFailure);
      assert.ok(error instanceof GatewayStartupCleanupError);
      assert.equal(error.cleanupError, cleanupFailure); return true;
    });
  } finally { PersistentTransport.prototype.stop = stop; }
  assert.equal(host.settlement.native.reason, failure);
  await failure.cleanup.shutdown();
  await assert.rejects(host.build(f.builder()), /already started/);
  assert.equal(f.rows().filter(row => row.kind === "start").length, 1);
  assert.equal(f.rows().filter(row => row.kind === "exit").length, 1);
});

test("changed native plan carrier is refused at the actual send boundary", async t => {
  const f = fixture(t, "prepare-ready"); let calls = 0;
  const host = new ToolkitApplicationPreparation({ plan: f.startup.selection }, async () => { calls += 1; return original; });
  const builder = f.builder(), start = PersistentTransport.prototype.startAsync;
  PersistentTransport.prototype.startAsync = async function() {
    await start.call(this);
    builder.preparedApplication(Buffer.from("changed carrier"), "8".repeat(64));
  };
  try { await assert.rejects(host.build(builder), InitOutcomeUnknownError); }
  finally { PersistentTransport.prototype.startAsync = start; }
  assert.equal(calls, 0);
  assert.equal(nativeRequests(f, "meta/init").length, 0);
  assert.equal(privateRequests(f).length, 1);
  assert.equal(f.rows().filter(row => row.kind === "exit").length, 1);
});

test("fresh plan local refusals and pre-start abort never launch a native child", async t => {
  const f = fixture(t, "prepare-ready"), finalize = async () => original;
  assert.throws(() => new ToolkitApplicationPreparation({ plan: { ...f.startup.selection, sha256: "9".repeat(64) } }, finalize), /changed/);
  const alias = join(f.directory, "plan-link.json"); symlinkSync(f.startup.selection.path, alias);
  assert.throws(() => new ToolkitApplicationPreparation({ plan: { ...f.startup.selection, path: alias } }, finalize));
  const huge = Buffer.alloc(256 * 1024 + 1), hugePath = join(f.directory, "huge.json"); writeFileSync(hugePath, huge);
  assert.throws(() => new ToolkitApplicationPreparation({ plan: { path: hugePath, sha256: createHash("sha256").update(huge).digest("hex") } }, finalize), /limit/);
  const abort = new AbortController(); abort.abort();
  const host = new ToolkitApplicationPreparation({ plan: f.startup.selection }, finalize);
  await assert.rejects(host.build(f.builder(), { signal: abort.signal }), /before startup/);
  assert.equal(host.preparationAttempt, null); assert.equal(host.settlement, null);
  assert.equal(f.rows().length, 0);
});

function termination(runtime, changes = {}) {
  assert.deepEqual(runtime.lastGatewayTermination(), { state: "exited", exitCode: 0,
    signal: null, escalation: "none", nativeShutdown: "acknowledged", ...changes });
}

// Real selected Meta builder, runtime, dispatcher, transport and init watch with
// an inert child. This does not qualify Rust gateway adoption or member finish.
test("ordinary native builder selects original owner and reconnects only after both owners settle", async t => {
  const f = fixture(t); const supplied = structuredClone(original);
  const host = new ToolkitHost(supplied); supplied.deployment_root = "/mutated";
  const runtime = f.remember(await host.configure(f.builder()).build());
  assert.equal(runtime.isRunning, true); assert.equal(f.callbackCount(), 1);
  const first = host.hostControl;
  assert.equal((await first.openComponent("frontend")).state, "open");
  assert.deepEqual(privateRequests(f)[0].request, { version: 1, id: 1, command: { kind: "bootstrap", selection: original } });
  assert.deepEqual(f.rows()[0].args, ["--persistent", "--host-control-read-fd", "3", "--host-control-write-fd", "4"]);
  const request = nativeRequests(f, "meta/init")[0].request;
  assert.equal(request.params.init_protocol, "accepted_then_settled");
  assert.equal(request.params.prepared_application, undefined);
  assert.equal(request.params.runtime_options.declare_spec_update, undefined);
  await runtime.shutdown();
  termination(runtime);
  assert.throws(() => first.assertReady(), HostTransportError);
  assert.throws(() => host.hostControl, HostControlError);
  await runtime.connect();
  assert.notEqual(host.hostControl, first);
  assert.equal(runtime.lastGatewayTermination(), null, "the new child has no old termination record");
  assert.equal(f.callbackCount(), 2);
  assert.equal(nativeRequests(f, "meta/init").length, 2);
  assert.equal(nativeRequests(f, "meta/shutdown").length, 1);
  assert.ok(nativeRequests(f, "meta/shutdown").every(row => row.privateOpen));
});

test("replacement uses exact retained bytes and public declaration, with no replay after shutdown or second build", async t => {
  const f = fixture(t); const selected = structuredClone(f.startup);
  const host = new ToolkitApplicationUpdate(selected);
  selected.prior_tool_policy.sha256 = "3".repeat(64);
  const builder = host.configure(f.builder()).declareSpecUpdate(42);
  assert.throws(() => host.configure(f.builder()), /already configured/);
  const runtime = f.remember(await builder.build());
  const request = nativeRequests(f, "meta/init")[0].request;
  assert.deepEqual(request.params.prepared_application, { bundle_base64: f.bytes.toString("base64"), digest: f.startup.selection.sha256 });
  assert.deepEqual(request.params.runtime_options.declare_spec_update, { expected_revision: 42 });
  assert.deepEqual(request.params.runtime_options.mob_composition, { authority: "authoritative" });
  assert.deepEqual(privateRequests(f)[0].request.command, { kind: "replace_application", startup: f.startup });
  assert.equal(f.callbackCount(), 1);
  await runtime.shutdown();
  termination(runtime);
  await assert.rejects(runtime.connect(), /already started; inspect the original transaction/);
  await assert.rejects(builder.build(), /already started; inspect the original transaction/);
  assert.equal(f.rows().filter(row => row.kind === "start").length, 1);
  assert.equal(nativeRequests(f, "meta/init").length, 1);
});

test("private refusal preserves the original typed failure, joins native stop, and consumes the replacement attempt", async t => {
  const f = fixture(t, "private-refused"); const host = new ToolkitApplicationUpdate(f.startup);
  const builder = host.configure(f.builder()); let failure;
  await assert.rejects(builder.build(), error => {
    failure = error; assert.ok(error instanceof HostRefusedError); assert.equal(error.refusal, "denied"); return true;
  });
  assert.equal(failure.message, "native host refused: denied");
  assert.equal(nativeRequests(f, "meta/init").length, 0);
  assert.equal(f.rows().filter(row => row.kind === "exit").length, 1);
  await assert.rejects(builder.build(), /already started/);
  assert.equal(privateRequests(f).length, 1);
});

test("known private failure after startAsync cannot enter the native init writer", async t => {
  const f = fixture(t); const host = new ToolkitApplicationUpdate(f.startup);
  const builder = host.configure(f.builder());
  const originalOpen = PersistentTransport.prototype.openInitWatch;
  let client, privateFailure, closed;
  // Synchronous native public watch opening occurs after await startAsync.
  // Close records failure synchronously, before the actual sendAsync boundary.
  PersistentTransport.prototype.openInitWatch = function(initId) {
    const watch = originalOpen.call(this, initId);
    client = host.hostControl; closed = client.close();
    assert.throws(() => client.assertReady(), error => { privateFailure = error; return true; });
    return watch;
  };
  try {
    await assert.rejects(builder.build(), error => {
      // Preserve the native runtime's conservative init classification.
      assert.ok(error instanceof InitOutcomeUnknownError);
      assert.ok(error.reason.includes(privateFailure.message)); return true;
    });
  } finally { PersistentTransport.prototype.openInitWatch = originalOpen; await closed; }
  assert.throws(() => client.assertReady(), error => error === privateFailure);
  assert.equal(f.callbackCount(), 1);
  assert.equal(privateRequests(f).length, 1);
  assert.equal(nativeRequests(f, "meta/init").length, 0);
  await assert.rejects(builder.build(), /already started/);
  assert.equal(f.rows().filter(row => row.kind === "start").length, 1);
});

for (const mode of ["init-refused", "settled-failed", "init-unknown"]) {
  test(`native ${mode} retains its actual error and never triggers replacement replay`, async t => {
    const f = fixture(t, mode); const host = new ToolkitApplicationUpdate(f.startup);
    const builder = host.configure(f.builder());
    await assert.rejects(builder.build(), error => {
      if (mode === "init-unknown") assert.ok(error instanceof InitOutcomeUnknownError);
      else {
        assert.ok(error instanceof StorageResolutionError); assert.equal(error.code, -32014);
        assert.equal(error.message, mode === "init-refused" ? "fixture selected revision refused" : "fixture entered init failed");
        assert.deepEqual(error.data, mode === "init-refused" ? { kind: "fixture_conflict", marker: 42 }
          : { marker: 43, durable_effects: "possible" });
      }
      return true;
    });
    assert.equal(nativeRequests(f, "meta/init").length, 1);
    assert.equal(f.rows().filter(row => row.kind === "exit").length, 1);
    if (mode === "settled-failed") assert.equal(nativeRequests(f, "meta/shutdown")[0].privateOpen, true);
    await assert.rejects(builder.build(), /already started/);
    assert.equal(f.rows().filter(row => row.kind === "start").length, 1);
  });
}

test("native shutdown error is retained after joining the child and closing the private channel", async t => {
  const f = fixture(t, "shutdown-failed"); const host = new ToolkitApplicationUpdate(f.startup);
  const runtime = f.remember(await host.configure(f.builder()).build()); const client = host.hostControl;
  await assert.rejects(runtime.shutdown(), /fixture cleanup failed/);
  termination(runtime, { nativeShutdown: "refused" });
  assert.equal(nativeRequests(f, "meta/shutdown")[0].privateOpen, true);
  assert.equal(f.rows().filter(row => row.kind === "exit").length, 1);
  assert.throws(() => client.assertReady(), HostTransportError);
  assert.throws(() => host.hostControl, HostControlError);
  await runtime.shutdown();
  termination(runtime, { nativeShutdown: "refused" });
  await assert.rejects(runtime.connect(), /already started/);
});

test("failed native stop keeps the private channel and rejoins through the returned runtime", async t => {
  const f = fixture(t); const host = new ToolkitHost(original);
  const originalStop = PersistentTransport.prototype.stop;
  const runtime = f.remember(await host.configure(f.builder()).build());
  const client = host.hostControl; const failure = new Error("injected native reap failure");
  PersistentTransport.prototype.stop = async function() { throw failure; };
  try {
    await assert.rejects(runtime.shutdown(), error => error === failure);
    assert.equal(runtime.lastGatewayTermination(), null);
    assert.equal(host.hostControl, client);
    assert.equal((await client.openComponent("still-owned")).state, "open");
    await assert.rejects(runtime.connect(), error => error === failure);
    assert.equal(f.rows().filter(row => row.kind === "start").length, 1);
  } finally {
    PersistentTransport.prototype.stop = originalStop;
    // Public runtime custody survives the failure. No captured transport,
    // private field or independent process reaper is needed to settle it.
    await runtime.shutdown();
  }
  termination(runtime);
  assert.throws(() => client.assertReady(), HostTransportError);
  await runtime.connect();
  assert.equal(f.rows().filter(row => row.kind === "start").length, 2);
});

test("failed build exposes the same native cleanup owner and does not replay a replacement", async t => {
  const f = fixture(t, "private-refused"); const host = new ToolkitApplicationUpdate(f.startup);
  const builder = host.configure(f.builder());
  const originalStop = PersistentTransport.prototype.stop;
  const failure = new Error("injected native stop before exit");
  let rejected;
  PersistentTransport.prototype.stop = async function() { throw failure; };
  try {
    await assert.rejects(builder.build(), error => {
      assert.ok(error instanceof GatewayStartupCleanupError);
      assert.ok(error.startupError instanceof HostRefusedError);
      assert.equal(error.cause, error.startupError);
      assert.equal(error.cleanupError, failure);
      assert.equal(error.cleanup.lastTermination(), null);
      rejected = error; f.remember(error.cleanup); return true;
    });
    await assert.rejects(rejected.cleanup.shutdown(), error => error === failure);
    assert.equal(f.rows().filter(row => row.kind === "start").length, 1);
  } finally {
    PersistentTransport.prototype.stop = originalStop;
    if (rejected) await rejected.cleanup.shutdown();
  }
  assert.deepEqual(rejected.cleanup.lastTermination(), { state: "exited", exitCode: 0,
    signal: null, escalation: "none", nativeShutdown: "not_negotiated" });
  await assert.rejects(builder.build(), /already started/);
  assert.equal(privateRequests(f).length, 1);
});

test("failed Toolkit build retains private cleanup after exit through the returned native handle", async t => {
  const f = fixture(t, "private-refused"); const host = new ToolkitApplicationUpdate(f.startup);
  const builder = f.builder(); const gatewayTransport = builder.gatewayTransport;
  let factoryCalls = 0;
  builder.gatewayTransport = function(factory) {
    return gatewayTransport.call(this, context => { factoryCalls += 1; return factory(context); });
  };
  host.configure(builder); builder.gatewayTransport = gatewayTransport;
  const originalSelect = PrivateHostClient.prototype.replaceApplication;
  const originalClose = PrivateHostClient.prototype.close;
  const privateFailure = new Error("injected private close remains unsettled after exit");
  const attempts = [];
  let client, startupFailure, rejected;
  PrivateHostClient.prototype.replaceApplication = async function(startup) {
    client = this;
    try { return await originalSelect.call(this, startup); }
    catch (error) { startupFailure = error; throw error; }
  };
  PrivateHostClient.prototype.close = async function() {
    attempts.push({ client: this, exits: f.rows().filter(row => row.kind === "exit") });
    // startAsync first attempts close before native stop. Keep that failed
    // join pending through the later post-exit close, without masking refusal.
    if (attempts.length <= 2) throw privateFailure;
    return await originalClose.call(this);
  };
  try {
    await assert.rejects(builder.build().then(runtime => f.remember(runtime)), error => {
      if (error instanceof GatewayStartupCleanupError) { rejected = error; f.remember(error.cleanup); }
      assert.ok(error instanceof GatewayStartupCleanupError);
      assert.ok(startupFailure instanceof HostRefusedError);
      assert.equal(startupFailure.refusal, "denied");
      assert.equal(error.startupError, startupFailure);
      assert.equal(error.cause, startupFailure);
      assert.equal(error.cleanupError, privateFailure); return true;
    });
    // Check outside startAsync's swallowed cleanup catch, so a wrong early
    // phase cannot disappear as another ignored private-close exception.
    assert.equal(attempts.length, 2);
    assert.equal(attempts[0].exits.length, 0);
    assert.equal(attempts[1].exits.length, 1);
    assert.equal(attempts[1].exits[0].code, 0);
    assert.ok(attempts.every(attempt => attempt.client === client));
    const record = structuredClone(rejected.cleanup.lastTermination());
    assert.deepEqual(record, { state: "exited", exitCode: 0, signal: null,
      escalation: "none", nativeShutdown: "not_negotiated" });
    assert.equal(factoryCalls, 1);
    await rejected.cleanup.shutdown();
    assert.equal(attempts.length, 3, "returned cleanup must rejoin the retained private client");
    assert.equal(attempts[2].client, client);
    assert.deepEqual(attempts[2].exits, attempts[1].exits);
    assert.deepEqual(rejected.cleanup.lastTermination(), record);
    await rejected.cleanup.shutdown();
    assert.equal(attempts.length, 3, "settled cleanup does not repeat private close");
    assert.equal(factoryCalls, 1, "cleanup never invokes a replacement factory");
    assert.deepEqual(rejected.cleanup.lastTermination(), record);
    await assert.rejects(builder.build().then(runtime => f.remember(runtime)), /already started/);
    assert.equal(factoryCalls, 2, "explicit rebuild reaches the existing replacement guard");
    assert.equal(f.rows().filter(row => row.kind === "start").length, 1);
    assert.equal(privateRequests(f).length, 1);
    assert.equal(nativeRequests(f, "meta/init").length, 0);
  } finally {
    PrivateHostClient.prototype.replaceApplication = originalSelect;
    PrivateHostClient.prototype.close = originalClose;
    // Assertion cleanup uses only a capability actually returned by build.
    // The existing fixture also remembers any unexpected successful runtime.
    if (rejected) await rejected.cleanup.shutdown();
  }
});

test("private close failure after exit stays on the same runtime until its actual join", async t => {
  const f = fixture(t); const host = new ToolkitHost(original);
  const runtime = f.remember(await host.configure(f.builder()).build());
  const client = host.hostControl;
  const originalClose = PrivateHostClient.prototype.close;
  const failure = new Error("injected private join rejection");
  let attempts = 0;
  PrivateHostClient.prototype.close = async function() {
    assert.equal(this, client); attempts += 1; throw failure;
  };
  try {
    await assert.rejects(runtime.shutdown(), error => error === failure);
    termination(runtime);
    await assert.rejects(runtime.connect(), error => error === failure);
    assert.equal(attempts, 2, "reconnect rejoins the original private client before any factory");
    assert.equal(f.rows().filter(row => row.kind === "start").length, 1);
  } finally {
    PrivateHostClient.prototype.close = originalClose;
    await runtime.shutdown();
  }
  await runtime.connect();
  assert.equal(f.rows().filter(row => row.kind === "start").length, 2);
});

test("native and private stop failures preserve both original errors on the same owner", async t => {
  const f = fixture(t, "shutdown-failed"); const host = new ToolkitApplicationUpdate(f.startup);
  const runtime = f.remember(await host.configure(f.builder()).build());
  const originalClose = PrivateHostClient.prototype.close;
  const privateFailure = new Error("injected private join rejection");
  let nativeFailure;
  PrivateHostClient.prototype.close = async function() { throw privateFailure; };
  try {
    await assert.rejects(runtime.shutdown(), error => {
      assert.ok(error instanceof AggregateError);
      nativeFailure = error.errors[0];
      assert.match(nativeFailure.message, /fixture cleanup failed/);
      assert.equal(error.cause, nativeFailure);
      assert.equal(error.errors[1], privateFailure); return true;
    });
    termination(runtime, { nativeShutdown: "refused" });
  } finally {
    PrivateHostClient.prototype.close = originalClose;
    // The private join now completes, while the retained native refusal is
    // still reported. A further rejoin releases only settled ownership.
    await assert.rejects(runtime.shutdown(), /did not confirm clean shutdown/);
    await runtime.shutdown();
  }
  termination(runtime, { nativeShutdown: "refused" });
});

for (const mode of ["exit-nonzero", "term-zero"]) {
  test(`${mode} is physical settlement without a clean Toolkit shutdown claim`, async t => {
    const f = fixture(t, mode); const host = new ToolkitHost(original);
    const runtime = f.remember(await host.configure(f.builder()).build());
    const client = host.hostControl;
    await assert.rejects(runtime.shutdown(), /did not confirm clean shutdown/);
    termination(runtime, mode === "exit-nonzero" ? { exitCode: 7 } : { escalation: "sigterm" });
    assert.throws(() => client.assertReady(), HostTransportError);
    // Rejoin acknowledges the already reported terminal outcome. The native
    // record remains abnormal; it is never rewritten as clean or Ready.
    await runtime.shutdown();
    termination(runtime, mode === "exit-nonzero" ? { exitCode: 7 } : { escalation: "sigterm" });
    assert.equal(f.rows().filter(row => row.kind === "start").length, 1);
  });
}

test("bad startup pins and unsupported builder combinations refuse before any process", async t => {
  const f = fixture(t);
  assert.throws(() => new ToolkitApplicationUpdate({ ...f.startup, selection: { ...f.startup.selection, sha256: "f".repeat(64) } }), /selection changed/);
  assert.throws(() => new ToolkitApplicationUpdate({ ...f.startup, association_directory: "/bad\0path" }), HostControlError);
  const link = join(f.directory, "linked.json"); symlinkSync(f.startup.selection.path, link);
  assert.throws(() => new ToolkitApplicationUpdate({ ...f.startup, selection: { ...f.startup.selection, path: link } }));
  const large = Buffer.alloc(16 * 1024 * 1024 + 1, 32); const largePath = join(f.directory, "large.json");
  writeFileSync(largePath, large);
  assert.throws(() => new ToolkitApplicationUpdate({ ...f.startup, selection: {
    path: largePath, sha256: createHash("sha256").update(large).digest("hex") } }), /exceeds its limit/);
  await assert.rejects(new ToolkitApplicationUpdate(f.startup).configure(Meta.builder()).build(), /requires gateway/);
  await assert.rejects(new ToolkitApplicationUpdate(f.startup).configure(f.builder()).mob("not-read.toml").build(), /mutually exclusive/);
  assert.equal(f.rows().length, 0);
});
