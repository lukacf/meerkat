import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createHash, webcrypto } from "node:crypto";
import { JSDOM } from "../../typescript/node_modules/jsdom/lib/api.js";
import { createWorkbench } from "../static/app.js";

const html = await readFile(new URL("../static/index.html", import.meta.url), "utf8");
const first = "1".repeat(32), second = "2".repeat(32);
const anchor = id => ({ id, sha256: id[0].repeat(64) });
const job = (id = first, status = "running") => ({ id, kind: "install", family: "agno", status,
  recovery_request: anchor(id) });
const state = (current = job()) => ({ families: [{ id: "agno", status: "unprepared", profiles: [] }],
  job: current, scope: "local_installation_only", recovery_manifest: { available: true, max_requests: 256 } });
const response = (value, status = 200) => ({ ok: status === 200, status, json: async () => structuredClone(value) });
const settle = async () => { for (let i = 0; i < 12; i++) await new Promise(resolve => setImmediate(resolve)); };
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const canonical = value => JSON.stringify(value).replace(/[\u007f-\uffff]/g, c => `\\u${c.charCodeAt(0).toString(16).padStart(4, "0")}`) + "\n";
function manifest(ids = [first], workspace = "/private/original-workspace") {
  const text = canonical({ requests: ids.map(anchor), schema_version: 1, workspace });
  return { scope: "retained_setup_requests_only", filename: "toolkit-recovery.json", media_type: "application/json",
    text, bytes: Buffer.byteLength(text), sha256: createHash("sha256").update(text).digest("hex") };
}
function reseal(reply) {
  reply.bytes = Buffer.byteLength(reply.text);
  reply.sha256 = createHash("sha256").update(reply.text).digest("hex");
  return reply;
}
function harness(initial = state(), handler = () => undefined, crypto = webcrypto) {
  const dom = new JSDOM(html, { url: "http://127.0.0.1:49111/#session=PRIVATE-CAPABILITY" });
  const document = dom.window.document, calls = [], downloads = [], created = [], revoked = [], timers = new Map();
  let nextTimer = 0;
  const fixture = { state: structuredClone(initial), reply: manifest() };
  dom.window.URL.createObjectURL = blob => { const url = `blob:http://127.0.0.1:49111/${created.length}`; created.push({ blob, url }); return url; };
  dom.window.URL.revokeObjectURL = url => revoked.push(url);
  dom.window.HTMLAnchorElement.prototype.click = function () { downloads.push({ href: this.href, filename: this.download }); };
  const app = createWorkbench(document, { crypto, fetch: async (path, init) => {
    const call = { path, init, payload: JSON.parse(init.body) }; calls.push(call);
    const handled = await handler(call, fixture);
    if (handled !== undefined) return handled;
    if (path === "/api/state") return response(fixture.state);
    if (path === "/api/recovery_manifest") return response(fixture.reply);
    throw new Error(`Unexpected request ${path}`);
  }, setTimeout: callback => { const id = ++nextTimer; timers.set(id, callback); return id; }, clearTimeout: id => timers.delete(id) });
  const byId = id => document.getElementById(id);
  const click = async id => { assert.ok(byId(id), `Missing ${id}`); byId(id).click(); await settle(); };
  const change = async (id, value) => { byId(id).value = value; byId(id).dispatchEvent(new dom.window.Event("change", { bubbles: true })); await settle(); };
  const close = () => { app.dispose(); dom.window.close(); };
  const blobText = blob => new Promise((resolve, reject) => {
    const reader = new dom.window.FileReader(); reader.onload = () => resolve(reader.result); reader.onerror = reject; reader.readAsText(blob);
  });
  return { app, fixture, byId, click, change, calls, downloads, created, revoked, dom, document, close, blobText };
}

test("explicit anchored running job selection downloads only the verified recovery file", async () => {
  const h = harness();
  try {
    await h.app.start();
    assert.ok(h.byId("recovery-panel"), "The recovery selection UI must exist");
    assert.equal(h.byId("recovery-panel").hidden, false);
    assert.equal(h.byId("recovery-add").textContent, "Add to recovery file");
    assert.equal(h.byId("recovery-save").disabled, true);
    await h.click("recovery-add");
    assert.equal(h.byId("recovery-selection").children.length, 1);
    await h.click("recovery-save");
    assert.deepEqual(h.calls.map(c => c.path), ["/api/state", "/api/recovery_manifest"]);
    assert.deepEqual(h.calls[1].payload, { job_ids: [first] });
    assert.equal(h.calls[1].init.headers["X-Toolkit-Session"], "PRIVATE-CAPABILITY");
    assert.deepEqual(h.downloads, [{ href: h.created[0].url, filename: "toolkit-recovery.json" }]);
    assert.equal(h.created[0].blob.type, "application/json");
    assert.equal(await h.blobText(h.created[0].blob), h.fixture.reply.text);
    assert.equal(h.document.body.textContent.includes("PRIVATE-CAPABILITY"), false);
    assert.match(h.byId("recovery-status").textContent, /download requested/i);
    assert.match(h.byId("recovery-status").textContent, new RegExp(h.fixture.reply.sha256));
    h.app.dispose();
    assert.deepEqual(h.revoked, [h.created[0].url]);
  } finally { h.close(); }
});

test("old hosts hide recovery selection and jobs without valid anchors cannot be added", async () => {
  const old = state(); delete old.recovery_manifest;
  const h = harness(old);
  try { await h.app.start(); assert.equal(h.byId("recovery-panel").hidden, true); await h.click("recovery-add");
    assert.equal(h.byId("recovery-selection").children.length, 0); } finally { h.close(); }
  const variants = [null, { ...job(), kind: "prepare" }, { ...job(), recovery_request: undefined },
    { ...job(), recovery_request: anchor(second) }, { ...job(), recovery_request: { ...anchor(first), sha256: "a".repeat(64) + "\n" } },
    { ...job(), recovery_request: { ...anchor(first), extra: true } }, { ...job(), id: first + "\n" }];
  for (const variant of variants) {
    const h = harness(state(variant));
    try { await h.app.start(); assert.equal(h.byId("recovery-add").disabled, true); await h.click("recovery-add");
      assert.equal(h.byId("recovery-save").disabled, true); assert.equal(h.calls.length, 1); } finally { h.close(); }
  }
});

function historyHarness(recovered = false) {
  const jobs = [job(second, "unconfirmed"), job(first, "unconfirmed")];
  if (recovered) for (const item of jobs) Object.assign(item, { recovered: true, error: "historical_outcome_unknown" });
  const initial = state(jobs[0]);
  initial.job_history = { available: true, count: jobs.length, ...(recovered ? { recovery_selection: true } : {}) };
  if (recovered) { initial.mode = "recovery"; initial.families[0].status = "unconfirmed"; }
  return harness(initial, call => {
    if (call.path === "/api/jobs") return response({ jobs: jobs.map(item => ({ ...item, profile: null })), next_before: null });
    if (call.path === "/api/job") return response({ job: jobs.find(item => item.id === call.payload.job_id),
      reconciliation: { available: true, observation: null } });
  });
}

test("selection follows explicit order across full history views and supports removing and re-adding", async () => {
  const h = historyHarness();
  try {
    await h.app.start(); await h.click("recovery-add");
    await h.change("job-choice", first); await h.click("recovery-add");
    await h.click("recovery-add");
    assert.equal(h.byId("recovery-selection").children.length, 2);
    h.fixture.reply = manifest([second, first]); await h.click("recovery-save");
    assert.deepEqual(h.calls.filter(c => c.path === "/api/recovery_manifest").at(-1).payload, { job_ids: [second, first] });
    assert.equal(h.downloads.length, 1);
    const remove = h.byId("recovery-selection").querySelector("button");
    assert.match(remove.getAttribute("aria-label"), new RegExp(second)); remove.click();
    assert.equal(h.byId("recovery-selection").children.length, 1);
    await h.change("job-choice", ""); await h.click("recovery-add");
    h.fixture.reply = manifest([first, second]); await h.click("recovery-save");
    assert.deepEqual(h.calls.filter(c => c.path === "/api/recovery_manifest").at(-1).payload, { job_ids: [first, second] });
    assert.equal(h.downloads.length, 2); assert.deepEqual(h.revoked, [h.created[0].url]);
    assert.ok(h.calls.every(c => ["/api/state", "/api/jobs", "/api/job", "/api/recovery_manifest"].includes(c.path)));
  } finally { h.close(); }
});

test("recovered job selection retains the controller's original workspace and exact Unicode encoding", async () => {
  const h = historyHarness(true);
  try {
    await h.app.start(); await h.change("job-choice", first); await h.click("recovery-add");
    h.fixture.reply = manifest([first], "/private/original-caf\u00e9/\ud83c\udf33");
    await h.click("recovery-save");
    assert.equal(h.downloads.length, 1);
    const saved = await h.blobText(h.created[0].blob);
    assert.equal(saved, h.fixture.reply.text);
    assert.equal(JSON.parse(saved).workspace, "/private/original-caf\u00e9/\ud83c\udf33");
    assert.equal(h.fixture.state.job.status, "unconfirmed");
    assert.equal(h.calls.filter(c => c.path.includes("reconcile")).length, 0);
  } finally { h.close(); }
});

test("anchor field order does not change the owner's canonical manifest", async () => {
  const initial = state(); initial.job.recovery_request = { sha256: "1".repeat(64), id: first };
  const h = harness(initial);
  try {
    await h.app.start(); await h.click("recovery-add"); await h.click("recovery-save");
    assert.equal(h.downloads.length, 1); assert.equal(await h.blobText(h.created[0].blob), manifest().text);
  } finally { h.close(); }
});

test("acquisition and remote export anchors can join an explicit mixed selection", async () => {
  const h = harness();
  try {
    await h.app.start(); await h.click("recovery-add");
    h.fixture.state.job = { ...job(second), kind: "acquire", family: "selected-mcpb", request_id: "a".repeat(32) };
    h.fixture.state.mcpb_acquisition = { available: true, scope: "trusted_public_mcpb_acquisition",
      staging_root: "/private/staged", host_profile_sha256: "b".repeat(64) };
    await h.click("refresh"); await h.click("recovery-add");
    const third = "3".repeat(32);
    h.fixture.state.job = { ...job(third), kind: "export", family: "remote-recipe", request_sha256: "c".repeat(64) };
    h.fixture.state.local_recipes = { export_available: true };
    await h.click("refresh"); await h.click("recovery-add");
    h.fixture.reply = manifest([first, second, third]); await h.click("recovery-save");
    assert.deepEqual(h.calls.at(-1).payload, { job_ids: [first, second, third] });
    assert.equal(h.downloads.length, 1);
    assert.ok(h.calls.every(c => ["/api/state", "/api/recovery_manifest"].includes(c.path)));
  } finally { h.close(); }
});

test("malformed, partial, foreign, noncanonical and incorrectly hashed replies never download", async t => {
  const cases = {
    empty: () => ({}),
    wrong_scope: r => ({ ...r, scope: "installed" }),
    wrong_filename: r => ({ ...r, filename: "../../launcher.html" }),
    wrong_media: r => ({ ...r, media_type: "text/html" }),
    navigation: r => ({ ...r, url: "https://example.invalid/PRIVATE-CAPABILITY" }),
    wrong_bytes: r => ({ ...r, bytes: r.bytes + 1 }),
    wrong_digest: r => ({ ...r, sha256: "0".repeat(64) }),
    digest_newline: r => ({ ...r, sha256: r.sha256 + "\n" }),
    fractional_length: r => ({ ...r, bytes: r.bytes + 0.5 }),
    invalid_json: r => reseal({ ...r, text: "{" }),
    no_requests: r => reseal({ ...r, text: canonical({ requests: [], schema_version: 1, workspace: "/old" }) }),
    foreign_job: () => manifest([second]),
    foreign_hash: r => reseal({ ...r, text: r.text.replace('"sha256":"' + "1".repeat(64), '"sha256":"' + "2".repeat(64)) }),
    duplicate_request: () => manifest([first, first]),
    extra_request_key: r => reseal({ ...r, text: r.text.replace('"id":', '"extra":true,"id":') }),
    extra_manifest_key: r => reseal({ ...r, text: r.text.replace('"requests":', '"extra":true,"requests":') }),
    relative_root: () => manifest([first], "relative/workspace"),
    null_in_root: () => manifest([first], "/old\0workspace"),
    wrong_version: r => reseal({ ...r, text: r.text.replace('"schema_version":1', '"schema_version":2') }),
    duplicate_key: r => reseal({ ...r, text: r.text.replace('"schema_version":1', '"schema_version":2,"schema_version":1') }),
    no_newline: r => reseal({ ...r, text: r.text.trimEnd() }),
    extra_whitespace: r => reseal({ ...r, text: " " + r.text }),
    oversized: r => reseal({ ...r, text: " ".repeat(65537) }),
  };
  for (const [name, alter] of Object.entries(cases)) await t.test(name, async () => {
    const h = harness();
    try {
      await h.app.start(); await h.click("recovery-add"); h.fixture.reply = alter(h.fixture.reply);
      await h.click("recovery-save");
      assert.equal(h.downloads.length, 0); assert.equal(h.created.length, 0);
      assert.equal(h.byId("recovery-selection").children.length, 1);
      assert.equal(h.byId("recovery-save").disabled, false);
      assert.match(h.byId("recovery-status").textContent, /Could not verify/);
      assert.equal(h.byId("recovery-status").textContent.includes("PRIVATE-CAPABILITY"), false);
    } finally { h.close(); }
  });
});

test("a reordered two-job response is refused, even with a correct response hash", async () => {
  const h = historyHarness();
  try {
    await h.app.start(); await h.click("recovery-add"); await h.change("job-choice", first); await h.click("recovery-add");
    h.fixture.reply = manifest([first, second]); await h.click("recovery-save");
    assert.equal(h.downloads.length, 0); assert.equal(h.byId("recovery-selection").children.length, 2);
  } finally { h.close(); }
});

test("HTTP and crypto failures retain anchors and expose no response secrets", async () => {
  for (const failure of ["http", "json", "crypto"]) {
    const h = harness(state(), call => {
      if (call.path !== "/api/recovery_manifest") return;
      if (failure === "http") return response({ error: "PRIVATE-SERVER-SECRET" }, 409);
      if (failure === "json") return { ok: true, json: async () => { throw new Error("PRIVATE-SERVER-SECRET"); } };
    }, failure === "crypto" ? { subtle: { digest: async () => { throw new Error("PRIVATE-CRYPTO-SECRET"); } } } : webcrypto);
    try {
      await h.app.start(); await h.click("recovery-add"); await h.click("recovery-save");
      assert.equal(h.downloads.length, 0); assert.equal(h.byId("recovery-save").disabled, false);
      assert.equal(h.byId("recovery-selection").children.length, 1);
      assert.doesNotMatch(h.document.body.textContent, /PRIVATE-(SERVER|CRYPTO)-SECRET/);
    } finally { h.close(); }
  }
});

test("selection is locked until the response and digest finish, independent of current job changes", async () => {
  const pending = deferred(), digest = deferred(); let digestInput;
  const h = harness(state(), call => call.path === "/api/recovery_manifest" ? pending.promise : undefined,
    { subtle: { digest: (algorithm, bytes) => { digestInput = { algorithm, bytes }; return digest.promise; } } });
  try {
    await h.app.start(); await h.click("recovery-add"); await h.click("recovery-save");
    assert.equal(h.byId("recovery-save").disabled, true);
    assert.equal(h.byId("recovery-selection").querySelector("button").disabled, true);
    await h.click("recovery-save");
    h.fixture.state.job = job(second); await h.click("refresh"); await h.click("recovery-add");
    h.byId("recovery-selection").querySelector("button").dispatchEvent(new h.dom.window.Event("click", { bubbles: true }));
    assert.equal(h.byId("recovery-selection").children.length, 1);
    pending.resolve(response(manifest())); await settle();
    assert.equal(h.byId("recovery-save").disabled, true); assert.equal(h.downloads.length, 0);
    digest.resolve(await webcrypto.subtle.digest(digestInput.algorithm, digestInput.bytes)); await settle();
    assert.equal(h.downloads.length, 1);
    assert.equal(h.calls.filter(c => c.path === "/api/recovery_manifest").length, 1);
    assert.equal(h.byId("recovery-add").disabled, false);
  } finally { h.close(); }
});

test("capability loss and changed selected anchors invalidate in-flight download without clearing selection", async () => {
  for (const mutation of ["capability", "anchor", "unavailable"]) {
    const pending = deferred(); let failState = false;
    const h = harness(state(), call => call.path === "/api/recovery_manifest" ? pending.promise :
      call.path === "/api/state" && failState ? response({}, 503) : undefined);
    try {
      await h.app.start(); await h.click("recovery-add"); await h.click("recovery-save");
      if (mutation === "capability") delete h.fixture.state.recovery_manifest;
      if (mutation === "anchor") h.fixture.state.job.recovery_request.sha256 = "f".repeat(64);
      if (mutation === "unavailable") failState = true;
      await h.click("refresh"); pending.resolve(response(manifest())); await settle();
      assert.equal(h.downloads.length, 0); assert.equal(h.byId("recovery-selection").children.length, 1);
      if (mutation === "anchor") {
        assert.equal(h.byId("recovery-save").disabled, true);
        assert.match(h.byId("recovery-selection").textContent, /request changed/);
        h.byId("recovery-selection").querySelector("button").click();
        await h.click("recovery-add"); assert.equal(h.byId("recovery-save").disabled, false);
      }
    } finally { h.close(); }
  }
});

test("disposal during request or digest never creates a download", async () => {
  for (const during of ["request", "digest"]) {
    const pending = deferred(); let digestInput;
    const h = harness(state(), call => call.path === "/api/recovery_manifest" && during === "request" ? pending.promise : undefined,
      during === "digest" ? { subtle: { digest: (algorithm, bytes) => { digestInput = { algorithm, bytes }; return pending.promise; } } } : webcrypto);
    try {
      await h.app.start(); await h.click("recovery-add"); await h.click("recovery-save"); h.app.dispose();
      pending.resolve(during === "request" ? response(manifest()) : await webcrypto.subtle.digest(digestInput.algorithm, digestInput.bytes));
      await settle(); assert.equal(h.created.length, 0); assert.equal(h.downloads.length, 0);
    } finally { h.close(); }
  }
});

test("an old response cannot complete or overwrite a newer save after host status recovers", async () => {
  const old = deferred(); let attempts = 0;
  const h = harness(state(), call => {
    if (call.path === "/api/recovery_manifest" && attempts++ === 0) return old.promise;
  });
  try {
    await h.app.start(); await h.click("recovery-add"); await h.click("recovery-save");
    delete h.fixture.state.recovery_manifest; await h.click("refresh");
    h.fixture.state.recovery_manifest = { available: true, max_requests: 256 }; await h.click("refresh");
    await h.click("recovery-save"); assert.equal(h.downloads.length, 1);
    const status = h.byId("recovery-status").textContent;
    old.resolve(response(manifest([second]))); await settle();
    assert.equal(h.downloads.length, 1); assert.equal(h.byId("recovery-status").textContent, status);
  } finally { h.close(); }
});

test("a failed download dispatch removes its link and revokes its object URL", async () => {
  const h = harness();
  try {
    h.dom.window.HTMLAnchorElement.prototype.click = () => { throw new Error("PRIVATE-DOWNLOAD-ERROR"); };
    await h.app.start(); await h.click("recovery-add"); await h.click("recovery-save");
    assert.deepEqual(h.revoked, [h.created[0].url]);
    assert.equal(h.document.querySelectorAll("a[download]").length, 0);
    assert.equal(h.byId("recovery-selection").children.length, 1);
    assert.doesNotMatch(h.byId("recovery-status").textContent, /PRIVATE-DOWNLOAD-ERROR/);
  } finally { h.close(); }
});

test("maximum selection is bounded to 256 and selecting never creates host work", async () => {
  const h = harness();
  try {
    await h.app.start();
    for (let i = 1; i <= 256; i++) {
      const id = i.toString(16).padStart(32, "0"); h.fixture.state.job = { ...job(id), recovery_request: { id, sha256: "a".repeat(64) } };
      await h.click("refresh"); await h.click("recovery-add");
    }
    assert.equal(h.byId("recovery-selection").children.length, 256);
    h.fixture.state.job = job(second); await h.click("refresh");
    assert.equal(h.byId("recovery-add").disabled, true); await h.click("recovery-add");
    assert.equal(h.byId("recovery-selection").children.length, 256);
    assert.ok(h.calls.every(c => c.path === "/api/state"));
  } finally { h.close(); }
});
