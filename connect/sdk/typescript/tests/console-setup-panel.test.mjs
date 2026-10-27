// These controls import the exact embedded JavaScript, without a generated build.
// They prove presentation/transport behavior only, not cookies or native auth.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { JSDOM } from "jsdom";
const assets = new URL("../../../crates/meerkat-connect/src/application_gateway/console_setup/assets/", import.meta.url);
const moduleUrl = source => `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`;
const projection = moduleUrl(await readFile(new URL("projection.js", assets), "utf8"));
const view = moduleUrl((await readFile(new URL("view.js", assets), "utf8")).replace('"./projection.js"', JSON.stringify(projection)));
const moduleSource = (await readFile(new URL("panel.js", assets), "utf8"))
  .replace('"./projection.js"', JSON.stringify(projection)).replace('"./view.js"', JSON.stringify(view));
const { mountSourceStatusPanel, mountAccountReview, readSourceStatus, mountSourceStatusPage, mountApplicationAccess, mountOperatorRecovery } = await import(moduleUrl(moduleSource));
function fixture(t) {
  const dom = new JSDOM("<main></main>");
  t.after(() => dom.window.close());
  return dom.window.document.querySelector("main");
}
function snapshot(text = "Pending") {
  return { scope: "viewer", revision: 0, observedAt: "this request", announcement: text,
    tasks: [{ id: "source", title: "Task setup", application: "Fixture", requester: "You", reason: "Reviewed task",
      phase: { text, tone: "attention" }, nextAction: "Your agent reports completion.", roles: [], userActions: [], controls: [] }],
    connections: [], frontends: [], packages: [] };
}
function deferred() { let resolve; let reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
function review(summary = "Connect the selected inbox") {
  return { review_id: 4, can_start: false, prompt: { kind: "approval", approval_id: "actual-prompt", summary,
    allowed_decisions: ["approve", "deny"], body: { phase: "installed_account_access",
      account: { issuer: "https://provider.example", subject: "selected-account" }, software: { server_name: "mail" },
      connector: { resource: "https://api.example", requested_scopes: ["mail.read"] },
      application_selection: { agent: "selected-agent", expires_in_ms: 60000, operations: {
        read_inbox: { required_scopes: ["mail.read"], destination_origin: "https://api.example" } } },
      notice: "Readiness is checked separately." } } };
}
test("mounted Console task status does not claim omitted inventories are empty", async t => {
  const container = fixture(t); const rendered = [];
  let current = snapshot();
  const selectedReview = review();
  const panel = mountSourceStatusPanel(container,
    async () => ({ projection: current, review: selectedReview }),
    { clear() {}, update(value) { rendered.push(value); } });
  t.after(() => panel.dispose());
  for (const empty of [false, true]) {
    current = snapshot(); if (empty) current.tasks = [];
    await panel.refresh();
    assert.deepEqual([...container.querySelectorAll(".toolkit-view h2")].map(heading => heading.textContent),
      ["Task setup status", "Setup requests"]);
    assert.doesNotMatch(container.textContent, /No service connections|No agent frontends|No packages/);
    assert.match(container.textContent, empty ? /No setup requests/ : /Reviewed task/);
    assert.deepEqual([...container.querySelectorAll("button")].map(button => button.textContent), ["Refresh status"]);
  }
  assert.deepEqual(rendered, [selectedReview, selectedReview]);
});

test("same-tab refresh shares the entered read and never starts a competing page request", async t => {
  const container = fixture(t); const held = deferred(); let calls = 0; const rendered = [];
  const account = { clear() { rendered.length = 0; }, update(value) { rendered.push(value.review_id); } };
  const panel = mountSourceStatusPanel(container, () => { calls++; return held.promise; }, account);
  t.after(() => panel.dispose());
  const first = panel.refresh(); const same = panel.refresh();
  assert.equal(first, same); await Promise.resolve(); assert.equal(calls, 1);
  held.resolve({ projection:snapshot("current"), review:{review_id:1} }); await first;
  assert.deepEqual(rendered, [1]); assert.equal(calls, 1);
});
test("read failure and disposal clear the account and suppress held responses", async t => {
  const container = fixture(t); const held = deferred(); const rendered = []; let mode = 0;
  const account = { clear() { rendered.length = 0; }, update(value) { rendered.push(value.review_id); } };
  const panel = mountSourceStatusPanel(container, () => mode === 0 ? Promise.resolve({ projection: snapshot(), review: { review_id: 1 } }) : mode === 1 ? Promise.reject(Error("refused")) : held.promise, account);
  await panel.refresh(); assert.deepEqual(rendered, [1]); mode = 1; await panel.refresh(); assert.deepEqual(rendered, []);
  mode = 2; const pending = panel.first(); await Promise.resolve(); panel.dispose(); held.resolve({ projection: snapshot(), review: { review_id: 3 } }); await pending;
  assert.deepEqual(rendered, []); assert.equal(container.textContent, "");
});
test("the actual approval renderer uses supplied summary and exact fields without interpreting permissions", t => {
  const container = fixture(t); const account = mountAccountReview(container, async () => {}, async () => new Response());
  t.after(() => account.dispose()); const selected = review();
  selected.prompt.body.service_instruction = { proposed_fresh_instruction: "Read this reviewed inbox", live_attachment_effect: "Attach to the original session", configuration_persistence: "Not authorized by this review" };
  selected.prompt.body.application_selection.expires_in_ms = null;
  selected.prompt.body = { access_request: selected.prompt.body, purpose: "operations",
    service_access: { permission: { lifetime: { scope: { kind: "task", task: "retained-task" }, expires_at_ms: 1800000000000 } } } };
  account.update(selected);
  assert.equal(container.querySelector("h2").textContent, "Connect the selected inbox");
  assert.match(container.textContent, /https:\/\/provider.example/); assert.match(container.textContent, /selected-account/);
  assert.match(container.textContent, /mail.read/); assert.match(container.textContent, /read_inbox/);
  assert.match(container.textContent, /This task/); assert.match(container.textContent, /2027-01-15T08:00:00.000Z/);
  assert.doesNotMatch(container.textContent, /Until revoked/);
  assert.match(container.textContent, /Read this reviewed inbox/); assert.match(container.textContent, /Not authorized by this review/);
  assert.equal(container.querySelector("details").open, false);
  assert.deepEqual([...container.querySelectorAll("button")].map(button => button.textContent), ["Approve", "Deny"]);
  account.update(review("")); assert.match(container.textContent, /No additional description was supplied/);
  assert.deepEqual([...container.querySelectorAll("button")].map(button => button.textContent), ["Approve", "Deny"]);
});
// This is the final protected operator body, not the helper's input shape.
// Member permissions and the separate catalog permission own their deadlines.
function batchReview(count = 2) {
  const service_instruction = {
    proposed_fresh_instruction: "Compare the selected public sources for the original question",
    instruction_origin: "setup proposal; approval authorizes this new instruction",
    live_attachment_effect: "hydrate these exact servers into the original session",
    configuration_persistence: "not authorized by this review",
  };
  const member_tool_policy = {
    proposal: { member: "original-agent", lifetime: "until_superseded",
      selection: { tools: Array.from({ length: count }, (_, index) => ({ name: `public_${index}_read`, consequence: "r0" })), catalog: "r0" },
      candidate_canonical_json: '{"members":[{"member_identity":"original-agent","grants":[{"tool_name":"preserved_tool"}]}]}' },
    approval_scope: "the complete successor policy including preserved member permissions",
    lifetime: "until superseded; task closure does not undo this separately reviewed policy",
    catalog_visibility: "eligible tool metadata in the original session; invocation still needs its exact native target grant",
  };
  const members = Array.from({ length: count }, (_, index) => {
    const server = `public-${index}`;
    return { server, parent: { purpose: "operations", connector_selection: { server, connector: null },
      service_access: { permission: { connection: server, operations: [`read_${index}`, `list_${index}`],
        lifetime: { scope: { kind: "task", task: "fresh-original" }, expires_at_ms: 1800000000000 + index * 1000 } } },
      access_request: { phase: "installed_public_service_access", authentication: { kind: "none" },
        software: { profile: `profile-${index}`, server_name: server }, operation_basis: { kind: "selected_manifest_declarations" },
        application_selection: { agent: "original-agent", operations: [`untrusted_display_alias_${index}`], expires_in_ms: null },
        service_instruction } } };
  });
  const request = { phase: "installed_public_service_batch_access", services: members.map(member => member.parent.access_request),
    service_instruction, member_tool_policy, notice: "One decision covers this exact public set and fresh instruction. Readiness is checked separately." };
  return { review_id: 26, can_start: false, prompt: { kind: "approval", approval_id: "actual-batch-prompt",
    summary: "Review this exact set of public services for the task", allowed_decisions: ["approve", "deny"],
    body: { kind: "public_services_v1", members, catalog: { purpose: "operations", access_request: request,
      catalog_access: { session: "original-session", lifetime: { scope: { kind: "task", task: "fresh-original" }, expires_at_ms: 1800000000000 } } },
      access_request: request } } };
}
function ordinaryReviewText(container) {
  const copy = container.cloneNode(true); for (const detail of copy.querySelectorAll("details")) detail.remove();
  return copy.textContent;
}

test("batch review visibly explains the exact members, reviewed task limits and separate persistent policy", t => {
  const container = fixture(t); const account = mountAccountReview(container, async () => {}, async () => new Response());
  t.after(() => account.dispose()); const selected = batchReview();
  const remote = selected.prompt.body.members[1].parent.access_request;
  delete remote.software;
  remote.operation_basis = { kind: "remote_recipe_declarations" };
  remote.remote_export = { export: { declared_remote_service: { mcp_name: "public-1" },
    mcp_configuration: { mcpServers: { "public-1": { url: "https://public-one.example/mcp" },
      unrelated: { url: "https://not-selected.example/mcp" } } } } };
  account.update(selected);
  const text = ordinaryReviewText(container);
  for (const expected of ["2 services", "original-agent", "public-0", "public-1", "read_0, list_0", "read_1, list_1",
    "This task", "2027-01-15T08:00:00.000Z", "2027-01-15T08:00:01.000Z", "Tool catalog", "original-session",
    "Compare the selected public sources for the original question", "not authorized by this review",
    "the complete successor policy including preserved member permissions",
    "until superseded; task closure does not undo this separately reviewed policy",
    "invocation still needs its exact native target grant", "public_0_read", "public_1_read",
    "Selected package declarations", "Researched remote recipe declarations", "https://public-one.example/mcp",
    "No sign-in required by the selected service declaration"])
    assert.ok(text.includes(expected), expected);
  assert.equal(container.querySelectorAll(".toolkit-batch-member").length, 2);
  assert.doesNotMatch(text, /untrusted_display_alias|Until revoked|anonymous execution verified|grant issued|not-selected\.example/i);
  assert.equal(container.querySelector("details").open, false);
  assert.deepEqual(JSON.parse(container.querySelector("details pre").textContent), selected.prompt);
  assert.equal(container.querySelectorAll("input").length, 0);
});

test("warm service review shows the complete two-agent two-session metadata scope without changing protected actions", async t => {
  const container = fixture(t); const sent = deferred(); const calls = [];
  const account = mountAccountReview(container, async () => {}, (path, options) => {
    calls.push({ path, options }); return sent.promise;
  });
  t.after(() => account.dispose()); const selected = batchReview(1);
  selected.prompt.body.kind = "public_services_with_setup_v2";
  selected.prompt.body.access_request.configured_setup = {
    authority: "fresh children for this task from the already configured agent setup policy",
    catalog_scope: "Both named agents may read tool metadata in both named sessions. This is the full two-agent by two-session scope, not paired permissions. It grants no service invocation.",
    processors: [{ namespace: "fixture", id: "original-owner" }, { namespace: "fixture", id: "helper-owner" }],
    sessions: ["original-session", "helper-session"], audience: { kind: "application", id: "exact-audience" },
    expires_at_ms: 1800000000000,
    continuation: "A later acquisition requires its own current capture, access review and fresh grants. Earlier connections must be selected again when needed.",
  };
  account.update(selected);
  const text = ordinaryReviewText(container);
  for (const expected of ["Later integration setup", "original-owner", "helper-owner", "original-session", "helper-session",
    "exact-audience", "full two-agent by two-session scope", "It grants no service invocation", "2027-01-15T08:00:00.000Z",
    "current capture, access review and fresh grants", "Earlier connections must be selected again", "read_0, list_0"])
    assert.ok(text.includes(expected), expected);
  assert.equal(container.querySelectorAll(".toolkit-batch-setup").length, 1);
  assert.equal(container.querySelectorAll(".toolkit-batch-member").length, 1);
  assert.deepEqual(JSON.parse(container.querySelector("details pre").textContent), selected.prompt);
  assert.equal(container.querySelectorAll("input").length, 0);
  const buttons = [...container.querySelectorAll("button")];
  assert.deepEqual(buttons.map(button => button.textContent), ["Approve", "Deny"]);
  buttons[0].click(); assert.equal(calls.length, 1);
  assert.deepEqual(JSON.parse(calls[0].options.body), { review_id: 26, approval_id: "actual-batch-prompt", decision: "approve" });
  assert.ok(buttons.every(button => button.disabled));
  sent.resolve(new Response("{}", { status: 202 })); await new Promise(resolve => setImmediate(resolve));
  account.update(batchReview()); assert.equal(container.querySelector(".toolkit-batch-setup"), null);
});

test("batch review renders every member as text and never turns a missing reviewed limit into a proposed duration", t => {
  const container = fixture(t); const account = mountAccountReview(container, async () => {}, async () => new Response());
  t.after(() => account.dispose()); const selected = batchReview(8);
  const last = selected.prompt.body.members[7]; last.server = '<img src=x onerror="bad()">';
  last.parent.service_access.permission.operations = ['<script>bad()</script>'];
  delete last.parent.service_access.permission.lifetime;
  last.parent.access_request.application_selection.expires_in_ms = 999999;
  account.update(selected);
  const text = ordinaryReviewText(container);
  assert.equal(container.querySelectorAll(".toolkit-batch-member").length, 8);
  assert.match(text, /8 services/); assert.ok(text.includes(last.server));
  assert.ok(text.includes('<script>bad()</script>')); assert.equal(container.querySelector("img,script"), null);
  assert.match(text, /Reviewed access limit unavailable/); assert.doesNotMatch(text, /999999|Until revoked/);
  assert.deepEqual(JSON.parse(container.querySelector("details pre").textContent), selected.prompt);
});

test("batch review keeps the existing protected answer payload, held-action controls and credential refusal", async t => {
  for (const decision of ["approve", "deny"]) {
    const container = fixture(t); const sent = deferred(); const calls = []; let refreshes = 0;
    const account = mountAccountReview(container, async () => { refreshes++; }, (path, options) => {
      calls.push({ path, options }); return sent.promise;
    });
    t.after(() => account.dispose()); account.update(batchReview());
    const buttons = [...container.querySelectorAll("button")];
    assert.deepEqual(buttons.map(button => button.textContent), ["Approve", "Deny"]);
    buttons[decision === "approve" ? 0 : 1].click();
    assert.equal(calls.length, 1); assert.equal(calls[0].path, "/host/toolkit/setup/answer");
    assert.deepEqual(JSON.parse(calls[0].options.body), { review_id: 26, approval_id: "actual-batch-prompt", decision });
    assert.ok(buttons.every(button => button.disabled)); assert.equal(refreshes, 0);
    sent.resolve(new Response("{}", { status: 202 })); await new Promise(resolve => setImmediate(resolve));
    assert.equal(refreshes, 1);
  }
  const container = fixture(t); const account = mountAccountReview(container, async () => {}, async () => new Response());
  t.after(() => account.dispose()); const selected = batchReview();
  selected.credential_input = { kind: "declared_secret_slots", slots: [{ slot: "TOKEN", label: "Token" }] };
  assert.throws(() => account.update(selected), /Invalid credential input/);
  assert.equal(container.querySelectorAll("input,button").length, 0);
});

test("explicit start forwards only the retained review id and waits for a fresh status read", async t => {
  const container = fixture(t); const sent = deferred(); let calls = []; let refreshes = 0;
  const account = mountAccountReview(container, async () => { refreshes++; }, (path, options) => { calls.push({ path, options }); return sent.promise; });
  t.after(() => account.dispose()); account.update({ review_id: 8, can_start: true, prompt: null });
  container.querySelector("button").click(); assert.equal(calls.length, 1);
  assert.equal(calls[0].path, "/host/toolkit/setup/start"); assert.equal(calls[0].options.credentials, "same-origin");
  assert.equal(calls[0].options.redirect, "error"); assert.deepEqual(JSON.parse(calls[0].options.body), { review_id: 8 });
  assert.equal(refreshes, 0); sent.resolve(new Response("{}", { status: 202 }));
  await new Promise(resolve => setImmediate(resolve)); assert.equal(refreshes, 1);
});
test("status transport is a finite GET and does not start account work", async () => {
  const calls = []; const expected = { projection: snapshot(), review: review() };
  const result = await readSourceStatus(async (path, options) => { calls.push({ path, options }); return new Response(JSON.stringify(expected), { headers: { "content-type": "application/json" } }); });
  assert.deepEqual(result, expected); assert.equal(calls.length, 1); assert.equal(calls[0].path, "/host/toolkit/setup/data");
  assert.equal(calls[0].options.method, "GET"); assert.equal(calls[0].options.body, undefined);
});

// Only the status reader's clock is controlled. Poll timers retain the existing
// page fixture, and protected POST promises are never timed out by this clock.
function statusReadClock(t) {
  const originalSet = globalThis.setTimeout; const originalClear = globalThis.clearTimeout;
  const timers = new Map(); const owned = new Set(); const cleared = []; let now = 0;
  t.mock.method(globalThis, "setTimeout", (callback, delay, ...args) => {
    if (delay !== 30000) return originalSet(callback, delay, ...args);
    const id = {}; owned.add(id); timers.set(id, { callback, due: now + delay }); return id;
  });
  t.mock.method(globalThis, "clearTimeout", id => {
    if (!owned.has(id)) return originalClear(id);
    cleared.push(id); timers.delete(id);
  });
  return { timers, cleared, advance(ms) {
    now += ms;
    for (const [id, timer] of [...timers]) if (timer.due <= now) {
      timers.delete(id); timer.callback();
    }
  } };
}
const nextTurn = () => new Promise(resolve => setImmediate(resolve));

test("status read deadline releases a held header response and consumes late rejection", async t => {
  const clock = statusReadClock(t); const headers = deferred(); const calls = []; let settled;
  const read = readSourceStatus((path, options) => { calls.push({ path, options }); return headers.promise; })
    .then(value => { settled = { value }; }, error => { settled = { error }; });
  t.after(async () => { headers.reject(Error("late status transport failure")); await read; await nextTurn(); });
  clock.advance(29999); await nextTurn(); assert.equal(settled, undefined);
  clock.advance(1); await nextTurn();
  assert.ok(settled?.error, "the status observer must stop waiting at its deadline");
  assert.equal(calls.length, 1); assert.equal(calls[0].path, "/host/toolkit/setup/data");
  assert.equal(calls[0].options.method, "GET"); assert.equal(calls[0].options.signal.aborted, true);
  assert.equal(clock.timers.size, 0); assert.equal(clock.cleared.length, 1);
});

test("status read uses one header-and-body deadline and ignores a late body", async t => {
  const clock = statusReadClock(t); const headers = deferred(); const body = deferred(); let signal; let settled; let inspected = 0;
  const late = {}; Object.defineProperty(late, "projection", { get() { inspected++; return snapshot("Late body"); } });
  const read = readSourceStatus((_path, options) => { signal = options.signal; return headers.promise; })
    .then(value => { settled = { value }; }, error => { settled = { error }; });
  t.after(async () => { headers.resolve({ ok: true, headers: new Headers({ "content-type": "application/json" }), json: () => body.promise }); body.resolve(late); await read; await nextTurn(); });
  clock.advance(20000);
  headers.resolve({ ok: true, headers: new Headers({ "content-type": "application/json" }), json: () => body.promise });
  await nextTurn(); assert.equal(settled, undefined);
  clock.advance(9999); await nextTurn(); assert.equal(settled, undefined);
  clock.advance(1); await nextTurn();
  assert.ok(settled?.error, "receiving headers must not restart or remove the response deadline");
  assert.equal(signal.aborted, true); assert.equal(clock.timers.size, 0); assert.equal(clock.cleared.length, 1);
  body.resolve(late); await nextTurn(); assert.equal(inspected, 0, "expired bodies must not be projected or validated later");
});

test("status read clears its deadline on success and each observed refusal", async t => {
  const clock = statusReadClock(t); const signals = [];
  const expected = { projection: snapshot() };
  const cases = [
    () => new Response(JSON.stringify(expected), { headers: { "content-type": "application/json" } }),
    () => new Response("denied", { status: 403 }),
    () => new Response("not json", { headers: { "content-type": "application/json" } }),
    () => { throw Error("transport refused synchronously"); },
  ];
  for (const [index, response] of cases.entries()) {
    const read = readSourceStatus((_path, options) => { signals.push(options.signal); return response(); });
    if (index === 0) assert.deepEqual(await read, expected); else await assert.rejects(read);
    assert.equal(clock.timers.size, 0); assert.equal(clock.cleared.length, index + 1);
  }
  clock.advance(30000); assert.equal(signals.some(signal => signal.aborted), false);
  await assert.rejects(readSourceStatus(() => assert.fail("invalid selection must not fetch"), { page_id: 1, direction: "unknown" }));
  assert.equal(clock.cleared.length, 4, "invalid selection never starts a response timer");
});

test("a timed-out status page requires explicit First page and cannot install a late offer", async t => {
  const fixture = pageFixture(t); const { container, timers } = fixture;
  const clock = statusReadClock(t); const body = deferred(); const calls = [];
  const envelope = (id, title) => ({ projection: snapshot(title), review: review(title),
    page: { page_id: id, first: true, has_next: true } });
  const page = mountSourceStatusPage(container, undefined, (path, options) => {
    calls.push({ path, options });
    if (calls.length === 2) return { ok: true, headers: new Headers({ "content-type": "application/json" }), json: () => body.promise };
    return new Response(JSON.stringify(envelope(calls.length, calls.length === 1 ? "Original review" : "Fresh review")),
      { headers: { "content-type": "application/json" } });
  });
  t.after(async () => { page.dispose(); body.resolve(envelope(99, "Late stale review")); await nextTurn(); });
  const tick = () => { const callback = [...timers.values()][0]; assert.ok(callback); timers.clear(); return callback(); };
  const button = text => [...container.querySelectorAll("button")].find(node => node.textContent === text);
  await nextTurn(); assert.match(container.textContent, /Original review/);
  const polling = tick(); await nextTurn();
  t.after(async () => { body.resolve(envelope(99, "Late stale review")); await polling; });
  assert.equal(calls.length, 2); assert.equal(button("First page").disabled, true);
  assert.equal(calls[1].path, "/host/toolkit/setup/data"); assert.equal(calls[1].options.method, "POST");
  assert.deepEqual(JSON.parse(calls[1].options.body), { page_id: 1, direction: "refresh" });
  clock.advance(30000); await nextTurn();
  assert.match(container.textContent, /Task setup status is unavailable/, "a stalled body must release navigation into explicit recovery");
  assert.doesNotMatch(container.textContent, /Original review|Late stale review/);
  assert.equal(button("First page").disabled, false); assert.equal(button("Refresh status").disabled, true);
  assert.equal(calls[1].options.signal.aborted, true);
  await polling; await tick(); await tick(); assert.equal(calls.length, 2, "polling must not automatically recover or replay the failed page");
  button("First page").click(); await nextTurn();
  assert.equal(calls.length, 3); assert.equal(calls[2].options.method, "GET"); assert.equal(calls[2].options.body, undefined);
  assert.match(container.textContent, /Fresh review/);
  body.resolve(envelope(99, "Late stale review")); await nextTurn();
  assert.match(container.textContent, /Fresh review/); assert.doesNotMatch(container.textContent, /Late stale review/);
  assert.equal(clock.timers.size, 0);
});

test("a retired page's default status read expires without restoring its DOM or polling", async t => {
  const fixture = pageFixture(t); const { container, timers, transition } = fixture;
  const clock = statusReadClock(t); const held = deferred(); let signal; let calls = 0;
  const page = mountSourceStatusPage(container, undefined, (_path, options) => { calls++; signal = options.signal; return held.promise; });
  t.after(async () => { page.dispose(); held.reject(Error("late retired read")); await nextTurn(); });
  await nextTurn(); transition("pagehide"); assert.equal(container.textContent, "");
  clock.advance(30000); await nextTurn();
  assert.equal(signal?.aborted, true, "a retired mount must not leave the default read unbounded");
  assert.equal(container.textContent, ""); assert.equal(timers.size, 0); assert.equal(calls, 1);
  assert.equal(clock.timers.size, 0); assert.equal(clock.cleared.length, 1);
  held.reject(Error("late retired read")); await nextTurn(); assert.equal(container.textContent, "");
});

test("software approval displays the actual checked package and preserves native decisions", t => {
  const container = fixture(t); const account = mountAccountReview(container, async () => {}, async () => new Response());
  t.after(() => account.dispose());
  const selected = { review_id: 9, can_start: false, prompt: { kind: "approval", approval_id: "software-review",
    summary: "Install the exact checked software configuration once", allowed_decisions: ["approve", "deny"],
    body: { kind: "service_software_install", scope: "software_installation_only", check_tool: "setup_selected_check",
      selection: { profile: "python", component: "mcp" }, owner_proposal: { profile: "python", component: "mcp",
        package: { publisher: "Selected publisher", name: "Selected MCP", version: "1.0.0" },
        destination: "/selected/install", mcp_name: "selected-service", entrypoint: { executable: "/selected/python" }, selection_sha256: "a".repeat(64) } } } };
  account.update(selected);
  for (const value of ["Software installation only", "Selected publisher", "Selected MCP", "1.0.0", "/selected/install", "selected-service"])
    assert.match(container.textContent, new RegExp(value));
  assert.deepEqual([...container.querySelectorAll("button")].map(button => button.textContent), ["Approve & Install", "Deny"]);
  selected.prompt.allowed_decisions = ["deny"]; account.update(selected);
  assert.deepEqual([...container.querySelectorAll("button")].map(button => button.textContent), ["Deny"]);
});

function sourceBuildReview() {
  return { review_id: 9, can_start: false, prompt: { kind: "approval", approval_id: "source-build-review",
    summary: "Install the checked source package", allowed_decisions: ["approve", "deny"], body: {
      kind: "service_software_install", scope: "software_installation_only", check_tool: "setup_selected_check",
      owner_proposal: { profile: "node", destination: "/selected/install", build: {
        schema_version: 1, scope: "selected_node_build", profile_sha256: "b".repeat(64),
        target: { node_version: "v24.19.0", node_abi: "137", platform: "linux", arch: "x64",
          libc: { family: "glibc", version: "2.39" } },
        commands: [{ argv: ["/selected/cc", "-c", "src/with space.c", "", "<img src=x>", "-o", "addon.o"] },
          { argv: ["/selected/ld", "-shared", "addon.o", "-o", "addon.node"] }],
        inputs: Array.from({ length: 256 }, (_, index) => ({ path: `headers/input-${index}.h`, sha256: "f".repeat(64) })),
        outputs: [{ destination: "node_modules/fixture/build/addon.node", replaces_sha256: null },
          { destination: "node_modules/fixture/index.cjs", replaces_sha256: "2".repeat(64) }] } } } } };
}

test("source build summary keeps command boundaries and file changes concise", t => {
  const container = fixture(t); const account = mountAccountReview(container, async () => {}, async () => new Response());
  t.after(() => account.dispose()); const selected = sourceBuildReview();
  const original = JSON.stringify(selected); account.update(selected);
  const text = ordinaryReviewText(container);
  for (const expected of ["Build source on this host", "without sandboxing", "v24.19.0", "137", "linux x64 glibc 2.39",
    '"/selected/cc" "-c" "src/with space.c" "" "<img src=x>" "-o" "addon.o"',
    '"/selected/ld" "-shared" "addon.o" "-o" "addon.node"',
    "node_modules/fixture/build/addon.node - Add a new file", "node_modules/fixture/index.cjs - Replace the existing file"])
    assert.ok(text.includes(expected), expected);
  assert.ok(text.indexOf("Command 1 literal arguments") < text.indexOf("Command 2 literal arguments"));
  assert.doesNotMatch(text, /headers\/input-|bbbbbbbb|ffffffff/);
  assert.equal(container.querySelector(".toolkit-build-review").querySelectorAll("dt").length, 7);
  assert.equal(container.querySelector("img,script"), null);
  assert.equal(container.querySelector("details").open, false);
  assert.deepEqual(JSON.parse(container.querySelector("details pre").textContent), selected.prompt);
  assert.equal(JSON.stringify(selected), original);
});

test("build summaries retain unknown change and format limits", t => {
  const container = fixture(t); const account = mountAccountReview(container, async () => {}, async () => new Response());
  t.after(() => account.dispose()); const selected = sourceBuildReview();
  delete selected.prompt.body.owner_proposal.build.outputs[0].replaces_sha256;
  account.update(selected);
  assert.match(ordinaryReviewText(container), /Replacement information unavailable/);
  assert.doesNotMatch(ordinaryReviewText(container), /Add a new file/);
  selected.prompt.body.owner_proposal.build.schema_version = 2; account.update(selected);
  assert.match(ordinaryReviewText(container), /This build format cannot be summarized here/);
  assert.doesNotMatch(ordinaryReviewText(container), /Command 1|Add a new file|Replace the existing file/);
  assert.deepEqual(JSON.parse(container.querySelector("details pre").textContent), selected.prompt);
  delete selected.prompt.body.owner_proposal.build; account.update(selected);
  assert.equal(container.querySelector(".toolkit-build-review"), null);
});

test("refresh layout placeholder survives only the pending read, with no retained review controls", async t => {
  for (const outcome of ["same", "changed", "absent", "invalid", "denied", "unavailable"]) {
    const container = fixture(t); const held = deferred(); let posts = 0;
    container.style.setProperty("min-height", "12px", "important");
    container.getBoundingClientRect = () => ({ height: 1600 });
    let response = { projection: snapshot(), review: review() }; let panel;
    const account = mountAccountReview(container, () => panel.refresh(), async () => { posts++; return new Response(); });
    panel = mountSourceStatusPanel(container, () => response, account);
    t.after(() => { panel.dispose(); account.dispose(); });
    await panel.refresh();
    const oldApprove = [...container.querySelectorAll("button")].find(button => button.textContent === "Approve");
    response = held.promise; const pending = panel.refresh();
    assert.equal(container.style.minHeight, "1600px", outcome);
    assert.equal(container.querySelector(".toolkit-account-review").textContent, "", outcome);
    assert.equal(container.querySelector("details"), null, outcome);
    assert.equal(container.querySelector(".toolkit-account-review button"), null, outcome);
    oldApprove.click(); assert.equal(posts, 0, "removed controls remain fenced during the layout placeholder");
    if (outcome === "denied" || outcome === "unavailable") {
      held.reject(Object.assign(Error("status unavailable"), { status: outcome === "denied" ? 403 : 503 }));
    } else {
      const next = { projection: snapshot(), review: outcome === "absent" ? null : review() };
      if (outcome === "changed") { next.projection.scope = "another-viewer"; next.review.review_id++; }
      if (outcome === "invalid") next.projection = {};
      held.resolve(next);
    }
    await pending;
    assert.equal(container.style.minHeight, "12px", outcome);
    assert.equal(container.style.getPropertyPriority("min-height"), "important", outcome);
    oldApprove.click(); assert.equal(posts, 0, "settlement does not revive a removed review control");
  }
});

test("refresh layout placeholder is released on disposal without clearing a successor's placeholder", async t => {
  const container = fixture(t); const firstRead = deferred(); const nextRead = deferred();
  let height = 1200;
  container.getBoundingClientRect = () => ({ height });
  const first = mountSourceStatusPanel(container, () => firstRead.promise);
  const pending = first.refresh(); await Promise.resolve();
  assert.equal(container.style.minHeight, "1200px");
  first.dispose(); assert.equal(container.style.minHeight, "");
  height = 900;
  const successor = mountSourceStatusPanel(container, () => nextRead.promise);
  t.after(() => successor.dispose());
  const next = successor.refresh(); await Promise.resolve();
  assert.equal(container.style.minHeight, "900px");
  firstRead.resolve({ projection: snapshot("stale") }); await pending;
  assert.equal(container.style.minHeight, "900px", "retired read cannot release the current mount's layout");
  nextRead.resolve({ projection: snapshot("current") }); await next;
  assert.equal(container.style.minHeight, "");
  assert.match(container.textContent, /current/); assert.doesNotMatch(container.textContent, /stale/);
});

test("same-review restoration requests focus without scrolling", async t => {
  const container = fixture(t); let response = { projection: snapshot(), review: review() }; let panel;
  const account = mountAccountReview(container, () => panel.refresh());
  panel = mountSourceStatusPanel(container, () => response, account);
  t.after(() => { panel.dispose(); account.dispose(); });
  await panel.refresh(); container.querySelector("summary").focus();
  const prototype = container.ownerDocument.defaultView.HTMLElement.prototype;
  const originalFocus = prototype.focus; const calls = [];
  prototype.focus = function(options) { calls.push({ node: this, options }); return originalFocus.call(this, options); };
  await panel.refresh();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].node, container.querySelector("summary"));
  assert.deepEqual(calls[0].options, { preventScroll: true });
  response = { projection: { ...snapshot(), scope: "another-viewer" }, review: review() };
  await panel.refresh(); assert.equal(calls.length, 1, "changed viewer cannot inherit focus");
});

test("serialized refresh clears private DOM and restores only the fresh same-review presentation", async t => {
  const container = fixture(t); const held = deferred();
  const responses = [{projection:snapshot(),review:review()}, held.promise]; let reads = 0; let panel;
  const account = mountAccountReview(container, () => panel.refresh());
  panel = mountSourceStatusPanel(container, () => { reads++; return responses.shift(); }, account);
  t.after(() => { panel.dispose(); account.dispose(); });
  await panel.refresh(); container.querySelector("details").open = true; container.querySelector("summary").focus();
  const first = panel.refresh(); const same = panel.refresh(); assert.equal(first, same);
  assert.equal(container.querySelector(".toolkit-account-review").textContent, "");
  assert.equal(container.querySelector("details"), null);
  held.resolve({projection:snapshot("Current"),review:review()}); await first;
  assert.equal(reads, 2); assert.equal(container.querySelector("details").open, true);
  assert.equal(container.ownerDocument.activeElement, container.querySelector("summary"));
  [...container.querySelectorAll(".toolkit-account-review button")].find(button => button.textContent === "Deny").focus();
  responses.push({projection:snapshot(),review:review()}); await panel.refresh();
  assert.equal(container.ownerDocument.activeElement.textContent, "Deny");
});

test("changed scope or exact review identity cannot inherit presentation hints", async t => {
  for (const change of ["scope", "review", "approval"]) {
    const container = fixture(t); const held = deferred(); let response = { projection: snapshot(), review: review() };
    let panel;
    const account = mountAccountReview(container, () => panel.refresh());
    panel = mountSourceStatusPanel(container, () => response, account);
    t.after(() => { panel.dispose(); account.dispose(); });
    await panel.refresh(); container.querySelector("details").open = true; container.querySelector("summary").focus();
    response = held.promise; const pending = panel.refresh();
    const next = { projection: snapshot(), review: review() };
    if (change === "scope") next.projection.scope = "another-viewer";
    if (change === "review") next.review.review_id++;
    if (change === "approval") next.review.prompt.approval_id = "another-exact-prompt";
    held.resolve(next); await pending;
    assert.equal(container.querySelector("details").open, false, change);
    assert.notEqual(container.ownerDocument.activeElement, container.querySelector("summary"), change);
  }
});

test("a denied read or absent review discards hints before the same review returns", async t => {
  for (const failure of ["denied", "absent"]) {
    const container = fixture(t); let mode = 0; let panel;
    const account = mountAccountReview(container, () => panel.refresh());
    panel = mountSourceStatusPanel(container, () => mode === 1
      ? (failure === "denied" ? Promise.reject(Error("unauthorized")) : { projection: snapshot(), review: null })
      : { projection: snapshot(), review: review() }, account);
    t.after(() => { panel.dispose(); account.dispose(); });
    await panel.refresh(); container.querySelector("details").open = true; container.querySelector("summary").focus();
    mode = 1; await panel.refresh();
    assert.equal(container.querySelector(".toolkit-account-review").textContent, "");
    mode = 2; await panel.first();
    assert.equal(container.querySelector("details").open, false, failure);
    assert.notEqual(container.ownerDocument.activeElement, container.querySelector("summary"), failure);
  }
});

test("refresh does not reclaim keyboard focus after the user moved it elsewhere", async t => {
  const container = fixture(t); const held = deferred(); let response = { projection: snapshot(), review: review() };
  let panel;
  const account = mountAccountReview(container, () => panel.refresh());
  panel = mountSourceStatusPanel(container, () => response, account);
  t.after(() => { panel.dispose(); account.dispose(); });
  const outside = container.ownerDocument.createElement("button"); outside.textContent = "Another control"; container.append(outside);
  await panel.refresh(); container.querySelector("details").open = true; container.querySelector("summary").focus();
  response = held.promise; const pending = panel.refresh();
  outside.focus(); outside.blur();
  assert.equal(container.ownerDocument.activeElement, container.ownerDocument.body);
  held.resolve({ projection: snapshot(), review: review() }); await pending;
  assert.equal(container.querySelector("details").open, true);
  assert.notEqual(container.ownerDocument.activeElement, container.querySelector("summary"));
});

test("pending answer and start POSTs stay single-entry across fresh same-review reads", async t => {
  for (const start of [false, true]) {
    const container = fixture(t); const sent = deferred(); const confirmation = deferred(); const calls = [];
    const selected = start ? { review_id: 8, can_start: true, prompt: null } : review();
    let reads = 0; let panel;
    const account = mountAccountReview(container, () => panel.refresh(), (path, options) => {
      calls.push({ path, options }); return sent.promise;
    });
    panel = mountSourceStatusPanel(container, () => ++reads <= 2
      ? { projection: snapshot(), review: selected } : confirmation.promise, account);
    t.after(() => { panel.dispose(); account.dispose(); });
    await panel.refresh();
    const original = container.querySelector(".toolkit-account-review button"); original.click();
    assert.equal(calls.length, 1); assert.equal(original.disabled, true);
    await panel.refresh();
    for (const button of container.querySelectorAll(".toolkit-account-review button")) {
      assert.equal(button.disabled, true);
      button.dispatchEvent(new container.ownerDocument.defaultView.Event("click"));
    }
    original.dispatchEvent(new container.ownerDocument.defaultView.Event("click"));
    assert.equal(calls.length, 1, "neither a new render nor a detached old button resends");
    assert.equal(calls[0].path, start ? "/host/toolkit/setup/start" : "/host/toolkit/setup/answer");
    assert.deepEqual(JSON.parse(calls[0].options.body), start ? { review_id: 8 }
      : { review_id: 4, approval_id: "actual-prompt", decision: "approve" });
    sent.resolve(new Response("{}", { status: 202 }));
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(reads, 3);
    assert.equal(container.querySelector(".toolkit-account-review").textContent, "");
    confirmation.resolve({ projection: snapshot(), review: selected });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(container.querySelector(".toolkit-account-review button").disabled, false);
    original.dispatchEvent(new container.ownerDocument.defaultView.Event("click"));
    assert.equal(calls.length, 1, "confirmation never sends an answer");
  }
});

test("failed post-settlement refresh leaves no actions and disposal suppresses refresh", async t => {
  for (const dispose of [false, true]) {
    const container = fixture(t); const sent = deferred(); let reads = 0; let posts = 0; let panel;
    const account = mountAccountReview(container, () => panel.refresh(), () => { posts++; return sent.promise; });
    panel = mountSourceStatusPanel(container, () => ++reads === 1
      ? { projection: snapshot(), review: review() } : Promise.reject(Error("unauthorized")), account);
    t.after(() => { panel.dispose(); account.dispose(); });
    await panel.refresh(); container.querySelector(".toolkit-account-review button").click();
    if (dispose) { account.dispose(); panel.dispose(); }
    sent.reject(Error("unconfirmed POST"));
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(posts, 1); assert.equal(reads, dispose ? 1 : 2);
    assert.equal(container.querySelector(".toolkit-account-review button"), null);
    if (dispose) assert.equal(container.textContent, "");
    else assert.match(container.textContent, /status is unavailable/);
  }
});

function pageFixture(t) {
  const container = fixture(t);
  const window = container.ownerDocument.defaultView;
  const timers = new Map(); let nextTimer = 0;
  t.mock.method(window, "setTimeout", (callback, delay) => {
    assert.equal(delay, 3000);
    const id = ++nextTimer; timers.set(id, callback); return id;
  });
  t.mock.method(window, "clearTimeout", id => { timers.delete(id); });
  const transition = type => {
    const event = new window.Event(type);
    Object.defineProperty(event, "persisted", { value: true });
    window.dispatchEvent(event);
  };
  return { container, timers, transition };
}

test("browser history restoration fetches fresh setup and ignores an older pending read", async t => {
  const { container, timers, transition } = pageFixture(t);
  const initial = deferred(); const returning = deferred(); let reads = 0;
  const page = mountSourceStatusPage(container, () => ++reads === 1 ? initial.promise : returning.promise);
  t.after(() => page.dispose());
  await Promise.resolve(); assert.equal(reads, 1);
  transition("pagehide"); assert.equal(container.textContent, "");
  transition("pageshow"); transition("pageshow"); await Promise.resolve(); assert.equal(reads, 2);
  const fresh = { projection: snapshot("Returned task"), review: review("Current account review") };
  fresh.projection.scope = "current-viewer";
  returning.resolve(fresh); await new Promise(resolve => setImmediate(resolve));
  assert.match(container.textContent, /Current account review/);
  assert.equal(container.querySelectorAll(".toolkit-account-review").length, 1);
  assert.equal(timers.size, 1);
  initial.resolve({ projection: snapshot("Old task"), review: review("Old private account") });
  await new Promise(resolve => setImmediate(resolve));
  assert.doesNotMatch(container.textContent, /Old task|Old private account/);
  assert.equal(timers.size, 1, "the old read cannot create another polling loop");
  const delayed = [...timers.values()][0];
  transition("pagehide"); assert.equal(timers.size, 0); assert.equal(container.textContent, "");
  await delayed(); assert.equal(reads, 2, "a queued old timer cannot read after pagehide");
  page.dispose(); transition("pageshow"); assert.equal(reads, 2);
});

test("restoration never replays a pending approval or retains a prior viewer's account on denial", async t => {
  const { container, timers, transition } = pageFixture(t);
  const sent = deferred(); const returning = deferred(); let reads = 0; let posts = 0;
  const page = mountSourceStatusPage(container, () => ++reads === 1
    ? Promise.resolve({ projection: snapshot(), review: review("Prior private account") })
    : returning.promise, () => { posts++; return sent.promise; });
  t.after(() => page.dispose());
  await new Promise(resolve => setImmediate(resolve));
  container.querySelector(".toolkit-account-review button").click(); assert.equal(posts, 1);
  transition("pagehide"); transition("pageshow");
  assert.doesNotMatch(container.textContent, /Prior private account|selected-account/);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(reads, 1, "history restoration cannot replace the pending action's offer");
  assert.match(container.textContent, /Waiting for the current setup response/);
  assert.equal(container.querySelector(".toolkit-account-review button"), null);
  sent.resolve(new Response("{}", { status: 202 })); await new Promise(resolve => setImmediate(resolve));
  assert.equal(posts, 1); assert.equal(reads, 1, "the old approval cannot refresh a retired page");
  const tick = [...timers.values()][0]; timers.clear(); const refreshed = tick();
  await Promise.resolve(); returning.reject(Error("current viewer denied")); await refreshed;
  assert.match(container.textContent, /status is unavailable/);
  assert.equal(container.querySelector(".toolkit-account-review button"), null);
  assert.equal(reads, 2, "the restored page reads normally after settlement");
  assert.equal(timers.size, 1);
  page.dispose(); assert.equal(timers.size, 0); assert.equal(container.textContent, "");
});

test("successful history restoration waits for answer and start settlement without replay", async t => {
  for (const start of [false, true]) {
    const { container, transition, timers } = pageFixture(t);
    const sent = deferred(); let reads = 0; let posts = 0;
    const selected = start ? { review_id: 8, can_start: true, prompt: null } : review();
    const page = mountSourceStatusPage(container, async () => {
      reads++; return { projection: snapshot(), review: selected };
    }, () => { posts++; return sent.promise; });
    t.after(() => page.dispose());
    await new Promise(resolve => setImmediate(resolve));
    const original = container.querySelector(".toolkit-account-review button");
    original.click(); assert.equal(posts, 1);
    transition("pagehide"); transition("pageshow");
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(reads, 1);
    assert.equal(container.querySelectorAll(".toolkit-account-review button").length, 0);
    assert.match(container.textContent, /Waiting for the current setup response/);
    original.dispatchEvent(new container.ownerDocument.defaultView.Event("click"));
    assert.equal(posts, 1, "a detached button cannot resend the pending POST");
    sent.resolve(new Response("{}", { status: 202 }));
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(reads, 1, "settlement cannot refresh the retired view");
    assert.equal(posts, 1);
    const tick = [...timers.values()][0]; timers.clear(); await tick();
    assert.equal(reads, 2);
    for (const button of container.querySelectorAll(".toolkit-account-review button"))
      assert.equal(button.disabled, false, "the next current read can present actions after settlement");
    assert.equal(posts, 1, "settlement and refresh never replay the user's response");
    page.dispose();
  }
});


test("public service review shows task access and declaration provenance without an invented account", async t => {
  for (const kind of ["package_published_metadata", "selected_manifest_declarations", "selected_software_declarations",
    "remote_recipe_declarations", "unknown", null, undefined]) {
    const container = fixture(t); const calls = [];
    const account = mountAccountReview(container, async () => {}, async (path, options) => {
      calls.push({ path, body: JSON.parse(options.body) }); return new Response("{}", { status: 202 });
    });
    t.after(() => account.dispose());
    account.update({ review_id: 18, can_start: true, prompt: null });
    assert.equal(container.querySelector("button").textContent, "Start service setup");
    assert.doesNotMatch(container.textContent, /account setup/i);
    const selected = { review_id: 18, can_start: false, prompt: {
      kind: "approval", approval_id: "public-service-review", summary: "Review public service access for this task",
      allowed_decisions: ["approve", "deny"], body: {
        access_request: { phase: "installed_public_service_access", authentication: { kind: "none" },
          software: { profile: "public-feed", server_name: "selected-public-service" }, operation_basis: { kind },
          application_selection: { agent: "original-agent", expires_in_ms: null, operations: ["read_public_feed"] },
          service_instruction: { proposed_fresh_instruction: "Read the selected feed", instruction_origin: "Approved new instruction",
            live_attachment_effect: "Publish tools to the original task", configuration_persistence: "Not authorized by this review" } },
        service_access: { permission: { lifetime: { scope: { kind: "task", task: "retained-task" }, expires_at_ms: 1800000000000 } } },
      } } };
    if (kind === "remote_recipe_declarations") {
      const body = selected.prompt.body.access_request;
      delete body.software;
      body.remote_export = { export: {
        declared_remote_service: { mcp_name: "selected-remote" },
        mcp_configuration: { mcpServers: { "selected-remote": { type: "streamable-http", url: "https://public.example/mcp" } } }
      } };
    }
    account.update(selected);
    for (const text of [kind === "remote_recipe_declarations" ? "Researched service declaration: no sign-in required" : "No account or sign-in required", "original-agent", "read_public_feed", "This task",
      "2027-01-15T08:00:00.000Z", "Publish tools to the original task", "Not authorized by this review"])
      assert.ok(container.textContent.includes(text), text);
    assert.ok(container.textContent.includes(kind === "remote_recipe_declarations" ? "Researched remote recipe declarations"
      : ["selected_manifest_declarations", "selected_software_declarations"].includes(kind) ? "Selected package declarations"
        : kind === "package_published_metadata" ? "Package publisher metadata" : "Operation provenance is unconfirmed"));
    if (kind !== "package_published_metadata") assert.doesNotMatch(container.textContent, /Package publisher metadata/);
    if (kind === "remote_recipe_declarations") {
      assert.ok(container.textContent.includes("https://public.example/mcp"));
      assert.ok(container.textContent.includes("selected-remote"));
      assert.doesNotMatch(container.textContent, /Package publisher metadata|anonymous|verified account/i);
    }
    assert.doesNotMatch(container.textContent, /selected-account|https:\/\/provider.example|Until revoked|account setup/i);
    assert.equal(container.querySelector("a"), null);
    const buttons = [...container.querySelectorAll("button")];
    assert.deepEqual(buttons.map(button => button.textContent), ["Approve", "Deny"]);
    buttons[1].click(); await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(calls, [{ path: "/host/toolkit/setup/answer", body: {
      review_id: 18, approval_id: "public-service-review", decision: "deny" } }]);
  }
});

function applicationAccess(state = "grant_issued") {
  return { scope: "application_issued_access", offer_id: 9, rows: [{ id: "a".repeat(64), service: "mail",
    target: "selected-agent", selected_account: "selected-account", resource: "mail", operations: ["read"],
    lifetime: { scope: { kind: "member", member: "selected-agent" }, expires_at_ms: null },
    state, can_revoke: state === "grant_issued" }] };
}
function accessResponse(state) { return new Response(JSON.stringify({ scope: "application_issued_access", state }),
  { headers: { "content-type": "application/json" } }); }

// No accessible-name package is installed. Resolve this explicit text-only
// aria-labelledby contract against the real DOM; no screen-reader claim.
function managementActionName(button) {
  const ids = button.getAttribute("aria-labelledby")?.split(/\s+/) ?? [];
  assert.ok(ids.length > 1, "management action must reference its visible target facts");
  assert.equal(ids[0], button.id, "the visible action text leads the accessible name");
  const card = button.closest("article");
  return ids.map(id => {
    const node = button.ownerDocument.getElementById(id);
    assert.ok(node && card.contains(node), "each reference belongs to this action's visible card");
    return node.textContent;
  }).join(" ");
}

test("access action names distinguish visible targets, accounts and lifetimes", async t => {
  const container = fixture(t); const calls = [];
  const panel = mountApplicationAccess(container, async () => {}, async (path, options) => {
    calls.push({ path, body: JSON.parse(options.body) }); return accessResponse("revoked");
  });
  t.after(() => panel.dispose());
  const value = applicationAccess();
  value.rows.push({ ...value.rows[0], id: "b".repeat(64), target: "other-agent",
    selected_account: "other-account", resource: "other-inbox", operations: ["read", "list"] });
  value.rows.push({ ...value.rows[0], id: "c".repeat(64),
    lifetime: { scope: { kind: "task", task: "private-task-key" }, expires_at_ms: 1800000000000 } });
  value.rows.push({ ...value.rows[0], id: "d".repeat(64), selected_account: null });
  panel.update(value, "person");
  const buttons = [...container.querySelectorAll("button")];
  assert.deepEqual(buttons.map(managementActionName), [
    "Revoke this access mail Agent selected-agent Operations read Resource mail Selected account selected-account Access scope member Expiry No expiry declared in this scope",
    "Revoke this access mail Agent other-agent Operations read, list Resource other-inbox Selected account other-account Access scope member Expiry No expiry declared in this scope",
    "Revoke this access mail Agent selected-agent Operations read Resource mail Selected account selected-account Access scope task Expiry 2027-01-15T08:00:00.000Z",
    "Revoke this access mail Agent selected-agent Operations read Resource mail Access scope member Expiry No expiry declared in this scope",
  ]);
  assert.ok(buttons.every(button => button.textContent === "Revoke this access"));
  buttons[1].click(); await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(calls, [{ path: "/host/toolkit/setup/access/revoke",
    body: { offer_id: 9, row_id: "b".repeat(64) } }]);
});

test("management label references remain unique across mounts and replacement", t => {
  const container = fixture(t);
  const access = [mountApplicationAccess(container, async () => {}), mountApplicationAccess(container, async () => {})];
  const recovery = [mountOperatorRecovery(container, async () => {}), mountOperatorRecovery(container, async () => {})];
  t.after(() => [...access, ...recovery].forEach(panel => panel.dispose()));
  const seen = new Set();
  for (let replacement = 0; replacement < 2; replacement++) {
    for (const panel of access) panel.update(applicationAccess(), "person");
    for (const panel of recovery) panel.update(recoveryOffer(), "person");
    for (const button of container.querySelectorAll("button")) managementActionName(button);
    for (const node of container.querySelectorAll("[id]")) {
      assert.match(node.id, /^toolkit-setup-label-\d+$/);
      assert.equal(seen.has(node.id), false, "presentation IDs cannot collide or be reused after replacement");
      seen.add(node.id);
    }
  }
});

test("application-issued access is distinct from account status and unknown grants cannot be revoked", t => {
  const container = fixture(t); const panel = mountApplicationAccess(container, async () => {});
  t.after(() => panel.dispose());
  panel.update(undefined, "person");
  assert.match(container.textContent, /has not enabled access management/);
  for (const state of ["grant_issued", "grant_unavailable", "pending_reconciliation", "approval_denied"]) {
    panel.update(applicationAccess(state), "person");
    assert.match(container.textContent, /Selected account/);
    assert.match(container.textContent, /Revocation does not stop work already running/);
    assert.equal(container.querySelectorAll("button").length, state === "grant_issued" ? 1 : 0);
    assert.doesNotMatch(container.textContent, /Account is connected|logged out|credentials removed|was revoked/);
  }
  panel.update({ scope: "application_issued_access", state: "unavailable" }, "person");
  assert.match(container.textContent, /status is unavailable/);
  assert.equal(container.querySelectorAll("button").length, 0);
  for (const bad of [ { ...applicationAccess(), scope: "all_accounts" },
    { ...applicationAccess(), rows: [{ ...applicationAccess().rows[0], state: "grant_unavailable", can_revoke: true }] },
    { ...applicationAccess(), offer_id: 0 } ]) assert.throws(() => panel.update(bad, "person"));
});

test("access POST carries only the exact offer and row and refreshes from real native outcome", async t => {
  const container = fixture(t); const calls = []; const finished = deferred(); let refreshes = 0;
  let panel; panel = mountApplicationAccess(container, async () => {
    ++refreshes; panel.update(applicationAccess("grant_unavailable"), "person"); finished.resolve();
  }, async (url, options) => { calls.push([url, options]); return accessResponse("revoked"); });
  t.after(() => panel.dispose()); panel.update(applicationAccess(), "person");
  container.querySelector("button").click(); await finished.promise; await Promise.resolve();
  assert.equal(calls.length, 1); assert.equal(refreshes, 1);
  assert.equal(calls[0][0], "/host/toolkit/setup/access/revoke");
  assert.equal(calls[0][1].credentials, "same-origin"); assert.equal(calls[0][1].redirect, "error");
  assert.deepEqual(JSON.parse(calls[0][1].body), { offer_id: 9, row_id: "a".repeat(64) });
  assert.match(container.textContent, /This access was revoked/);
  assert.match(container.textContent, /Access is unavailable/);
  assert.equal(container.querySelectorAll("button").length, 0);
});

test("not-current access response requires a fresh view and a new explicit decision", async t => {
  const container = fixture(t); const calls = []; const finished = [deferred(), deferred()]; let refreshes = 0;
  // These are synthetic response values for the UI contract, not a native
  // policy-swap control. Every subsequent action must use the refreshed offer.
  let panel; panel = mountApplicationAccess(container, async () => {
    ++refreshes;
    panel.update({ ...applicationAccess(refreshes === 1 ? "grant_issued" : "grant_unavailable"), offer_id: 9 + refreshes }, "person");
    finished[refreshes - 1].resolve();
  }, async (url, options) => { calls.push([url, options]); return accessResponse(calls.length === 1 ? "not_current" : "revoked"); });
  t.after(() => panel.dispose()); panel.update(applicationAccess(), "person");
  container.querySelector("button").click(); await finished[0].promise; await Promise.resolve();
  assert.equal(calls.length, 1); assert.equal(refreshes, 1);
  assert.match(container.textContent, /This access view is no longer current/);
  assert.match(container.textContent, /This request revoked nothing/);
  assert.match(container.textContent, /Review current status before a new decision/);
  assert.doesNotMatch(container.textContent, /could not be confirmed|This access was revoked/);
  const fresh = container.querySelector("button"); assert.equal(fresh.disabled, false);
  assert.deepEqual(JSON.parse(calls[0][1].body), { offer_id: 9, row_id: "a".repeat(64) });
  await Promise.resolve(); assert.equal(calls.length, 1);
  fresh.click(); await finished[1].promise; await Promise.resolve();
  assert.equal(calls.length, 2); assert.equal(refreshes, 2);
  assert.deepEqual(JSON.parse(calls[1][1].body), { offer_id: 10, row_id: "a".repeat(64) });
  assert.match(container.textContent, /This access was revoked/);
  assert.equal(container.querySelectorAll("button").length, 0);
});

test("lost access reply and overlapping reads never automatically replay a revoke", async t => {
  const container = fixture(t); const post = deferred(); const refreshed = deferred(); const sending = { sending: false };
  let calls = 0; let panel;
  panel = mountApplicationAccess(container, async () => { panel.update(applicationAccess("pending_reconciliation"), "person"); refreshed.resolve(); },
    () => { ++calls; return post.promise; }, sending);
  t.after(() => panel.dispose()); panel.update(applicationAccess(), "person");
  container.querySelector("button").click();
  panel.update(applicationAccess(), "person");
  assert.equal(container.querySelector("button").disabled, true);
  container.querySelector("button").click(); assert.equal(calls, 1);
  post.reject(Error("lost reply")); await refreshed.promise; await Promise.resolve();
  assert.equal(calls, 1); assert.equal(sending.sending, false);
  assert.match(container.textContent, /could not be confirmed/);
  assert.match(container.textContent, /issuance could not be confirmed/);
  assert.equal(container.querySelectorAll("button").length, 0);
});

test("serial page replacement removes the prior access controls and keeps only fresh results", async t => {
  const container = fixture(t); const held = deferred(); let reads = 0;
  const access = mountApplicationAccess(container, async () => {});
  const panel = mountSourceStatusPanel(container, () => ++reads === 1 ? held.promise : Promise.resolve({
    projection:snapshot(),application_access:applicationAccess("grant_unavailable")}), undefined, access);
  t.after(() => { panel.dispose(); access.dispose(); });
  const first = panel.refresh(); assert.equal(panel.refresh(), first);
  held.resolve({projection:snapshot(),application_access:applicationAccess()}); await first;
  assert.equal(reads, 1); assert.equal(container.querySelectorAll(".toolkit-application-access button").length, 1);
  const next = panel.refresh(); assert.equal(container.querySelectorAll(".toolkit-application-access button").length, 0);
  await next; assert.equal(reads, 2); assert.match(container.textContent, /Access is unavailable/);
  panel.dispose(); assert.doesNotMatch(container.textContent, /selected-account/);
});

test("access refresh preserves focus only for the same person's exact row and uses the new offer", async t => {
  const container = fixture(t); const doc = container.ownerDocument; const posts = []; const post = deferred();
  let mode = "same"; let offer = 9; let held;
  const access = mountApplicationAccess(container, async () => {}, (url, options) => {
    posts.push(JSON.parse(options.body)); return post.promise;
  });
  const panel = mountSourceStatusPanel(container, () => held ? held.promise : mode === "failed"
    ? Promise.reject(Error("denied")) : Promise.resolve({ projection: { ...snapshot(), scope: mode === "person" ? "other" : "viewer" },
      application_access: { ...applicationAccess(), offer_id: offer } }), undefined, access);
  t.after(() => { panel.dispose(); access.dispose(); });
  await panel.refresh();
  const old = container.querySelector(".toolkit-application-access button"); old.focus();
  offer = 10; held = deferred(); const read = panel.refresh();
  assert.equal(container.querySelector(".toolkit-application-access button"), null);
  old.dispatchEvent(new doc.defaultView.Event("click")); assert.equal(posts.length, 0);
  held.resolve({ projection: snapshot(), application_access: { ...applicationAccess(), offer_id: offer } }); await read; held = undefined;
  const current = container.querySelector(".toolkit-application-access button");
  assert.notEqual(current, old); assert.equal(doc.activeElement, current);
  current.click(); assert.deepEqual(posts, [{ offer_id: 10, row_id: "a".repeat(64) }]);
  post.resolve(accessResponse("refused")); await new Promise(resolve => setImmediate(resolve));
  for (const change of ["person", "failed"]) {
    mode = "same"; await panel.first(); container.querySelector(".toolkit-application-access button").focus();
    mode = change; await panel.refresh();
    assert.equal(doc.activeElement, doc.body, change);
    if (change === "failed") assert.equal(container.querySelector(".toolkit-application-access button"), null);
  }
  mode = "same"; await panel.first(); container.querySelector(".toolkit-application-access button").focus();
  held = deferred(); const pending = panel.refresh();
  const outside = doc.createElement("button"); outside.textContent = "Another enabled control"; container.append(outside);
  outside.focus(); assert.equal(doc.activeElement, outside);
  held.resolve({ projection: snapshot(), application_access: { ...applicationAccess(), offer_id: 11 } }); await pending;
  assert.equal(doc.activeElement, outside, "a completed read must not steal focus after it moved");
});


test("mounted pages replace task and access rows and send only retained read positions", async t => {
  const container = fixture(t); const calls = []; const posts = []; const held = deferred();
  const empty = snapshot(); empty.tasks = [];
  const replies = [
    {projection:snapshot("First"),review:null,page:{page_id:41,first:true,has_next:true},application_access:applicationAccess()},
    {projection:empty,review:null,page:{page_id:42,first:false,has_next:true},application_access:{...applicationAccess(),rows:[],offer_id:42}},
    {projection:snapshot("Last"),review:null,page:{page_id:43,first:false,has_next:false},application_access:{...applicationAccess(),offer_id:43}},
  ];
  const fetcher = async (path, options) => {
    calls.push({path,options}); return new Response(JSON.stringify(replies.shift()), {headers:{"content-type":"application/json"}});
  };
  const access = mountApplicationAccess(container, async () => {}, (path, options) => { posts.push({path,options}); return held.promise; });
  const panel = mountSourceStatusPanel(container, selection => readSourceStatus(fetcher, selection), undefined, access);
  t.after(() => { panel.dispose(); access.dispose(); });
  await panel.refresh(); const old = container.querySelector(".toolkit-application-access button");
  await panel.next(); old.dispatchEvent(new container.ownerDocument.defaultView.Event("click")); assert.equal(posts.length, 0);
  assert.match(container.textContent, /No matching setup requests on this page/);
  assert.match(container.textContent, /No matching application-issued access records on this page/);
  assert.match(container.textContent, /More history remains to inspect/);
  await panel.refresh(); assert.doesNotMatch(container.textContent, /No matching setup requests/);
  assert.match(container.textContent, /End of the history observed in this read/);
  assert.deepEqual(calls.map(({options}) => options.body && JSON.parse(options.body)),
    [undefined,{page_id:41,direction:"next"},{page_id:42,direction:"refresh"}]);
  assert.equal([...container.querySelectorAll("button")].find(b => b.textContent === "First page").disabled, false);
  assert.equal([...container.querySelectorAll("button")].find(b => b.textContent === "Next page").disabled, true);
  container.querySelector(".toolkit-application-access button").click();
  assert.deepEqual(JSON.parse(posts[0].options.body), {offer_id:43,row_id:"a".repeat(64)});
  held.resolve(accessResponse("refused")); await new Promise(resolve => setImmediate(resolve));
  assert.equal(posts.length, 1);
  for (const call of calls.slice(1)) {
    assert.equal(call.path, "/host/toolkit/setup/data"); assert.equal(call.options.method,"POST");
    assert.equal(call.options.credentials,"same-origin"); assert.equal(call.options.headers["Content-Type"],"application/json");
  }
});

test("same-login tabs pause scheduled polling after replacement while another login keeps its page", async t => {
  // Explicit presentation-only server model; real login owner identity/CAS is
  // exercised by console_lifetime.rs, not inferred from these string keys.
  const current = new Map(); const calls = []; let nextId = 1;
  const read = login => async selection => {
    calls.push({login,selection});
    if (selection && current.get(login) !== selection.page_id) throw Error("stale page");
    const page_id = nextId++; current.set(login,page_id);
    return {projection:snapshot(login),page:{page_id,first:!selection,has_next:true}};
  };
  const a = pageFixture(t), b = pageFixture(t), other = pageFixture(t);
  const settled = () => new Promise(resolve => setImmediate(resolve));
  const tick = async fixture => {
    assert.equal(fixture.timers.size,1); const callback = [...fixture.timers.values()][0];
    fixture.timers.clear(); await callback();
  };
  const taskView = fixture => fixture.container.querySelector(".toolkit-view:not(.toolkit-account-review):not(.toolkit-application-access)");
  const first = mountSourceStatusPage(a.container,read("shared")); await settled();
  const independent = mountSourceStatusPage(other.container,read("other")); await settled();
  const second = mountSourceStatusPage(b.container,read("shared")); await settled();
  t.after(() => { first.dispose(); second.dispose(); independent.dispose(); });
  await tick(a); assert.equal(taskView(a),null);
  assert.match(a.container.textContent,/another tab may have replaced this page/);
  const paused = calls.length; await tick(a); await tick(a); assert.equal(calls.length,paused);
  await tick(other); assert.notEqual(taskView(other),null);
  [...a.container.querySelectorAll("button")].find(button => button.textContent === "First page").click();
  await settled(); assert.equal(calls.at(-1).selection,undefined); assert.notEqual(taskView(a),null);
  await tick(b); assert.equal(taskView(b),null);
  const again = calls.length; await tick(b); assert.equal(calls.length,again);
  await tick(other); assert.notEqual(taskView(other),null);
});

test("invalid page metadata clears the view and does not send a supplied raw cursor", async t => {
  const container = fixture(t); const calls = [];
  const panel = mountSourceStatusPanel(container, async () => ({projection:snapshot(),
    page:{page_id:1,first:true,has_next:true,after:"raw-key"}})); t.after(() => panel.dispose());
  await panel.refresh(); assert.equal(container.querySelector(".toolkit-view"),null);
  await assert.rejects(readSourceStatus((...args) => { calls.push(args); },
    {page_id:1,direction:"next",after:"raw-key"}),/Invalid page selection/);
  assert.equal(calls.length,0);
});

function providerSignIn(reviewId = 31) {
  return { review_id: reviewId, can_start: false, prompt: { kind: "provider_browser",
    authorization_url: "https://provider.example/authorize?state=private-attempt" } };
}
test("provider sign-in cancellation requires an explicit click and forwards only the exact review", async t => {
  const container = fixture(t); const sent = deferred(); const refreshed = deferred();
  const calls = []; let refreshes = 0;
  const account = mountAccountReview(container, () => { refreshes++; return refreshed.promise; },
    (path, options) => { calls.push({ path, options }); return sent.promise; });
  t.after(() => { refreshed.resolve(); account.dispose(); });
  account.update(providerSignIn());
  assert.equal(calls.length, 0);
  const cancel = [...container.querySelectorAll("button")].find(button => button.textContent === "Cancel sign-in");
  assert.ok(cancel, "the retained provider prompt needs an explicit stop control");
  assert.equal(container.querySelector("a").rel, "noopener noreferrer");
  cancel.click(); cancel.click();
  assert.equal(calls.length, 1); assert.equal(cancel.disabled, true); assert.equal(refreshes, 0);
  assert.equal(calls[0].path, "/host/toolkit/setup/cancel-sign-in");
  assert.equal(calls[0].options.method, "POST"); assert.equal(calls[0].options.credentials, "same-origin");
  assert.equal(calls[0].options.redirect, "error");
  assert.deepEqual(JSON.parse(calls[0].options.body), { review_id: 31 });
  assert.doesNotMatch(calls[0].options.body, /private-attempt|authorization_url|approval_id|decision|account/);
  sent.resolve(new Response('{"state":"forwarded"}', { status: 202 }));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(refreshes, 1);
  assert.match(container.querySelector('[role="status"]').textContent, /Response forwarded/);
  assert.doesNotMatch(container.querySelector('[role="status"]').textContent, /cancelled|completed|connected/i);
  refreshed.resolve();
});
test("provider sign-in cancellation rejects detached prompts and view closure causes no request", async t => {
  const container = fixture(t); const calls = [];
  const account = mountAccountReview(container, async () => {}, async (path, options) => {
    calls.push({ path, options }); return new Response("{}", { status: 202 });
  });
  account.update(providerSignIn(31));
  const stale = container.querySelector("button"); assert.ok(stale);
  account.clear(); stale.click(); assert.equal(calls.length, 0);
  account.update(providerSignIn(32)); stale.click(); assert.equal(calls.length, 0);
  const current = container.querySelector("button"); assert.ok(current);
  current.click(); await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(calls.map(call => JSON.parse(call.options.body)), [{ review_id: 32 }]);
  account.update(providerSignIn(33)); const closed = container.querySelector("button");
  account.dispose(); closed.click(); assert.equal(calls.length, 1); assert.equal(container.textContent, "");
});
test("provider sign-in cancellation preserves a replacement projection and never claims completion on POST settlement", async t => {
  for (const status of [202, 409]) {
    const container = fixture(t); const sent = deferred(); const calls = []; let refreshes = 0;
    const account = mountAccountReview(container, async () => { refreshes++; },
      (path, options) => { calls.push({ path, options }); return sent.promise; });
    t.after(() => account.dispose()); account.update(providerSignIn());
    const cancel = container.querySelector("button"); assert.ok(cancel); cancel.click();
    account.update(review("Current native access review"));
    assert.deepEqual([...container.querySelectorAll("button")].map(button => button.textContent), ["Approve", "Deny"]);
    for (const button of container.querySelectorAll("button")) { assert.equal(button.disabled, true); button.click(); }
    sent.resolve(new Response("{}", { status })); await new Promise(resolve => setImmediate(resolve));
    assert.equal(calls.length, 1); assert.equal(refreshes, 1);
    assert.match(container.textContent, /Current native access review/);
    assert.doesNotMatch(container.querySelector('[role="status"]').textContent, /forwarded|cancelled|completed|connected/i);
    account.update(review("Current native access review"));
    const deny = [...container.querySelectorAll("button")].find(button => button.textContent === "Deny");
    deny.click(); await new Promise(resolve => setImmediate(resolve));
    assert.equal(calls[1].path, "/host/toolkit/setup/answer");
    assert.deepEqual(JSON.parse(calls[1].options.body), { review_id: 4, approval_id: "actual-prompt", decision: "deny" });
  }
});

function credentialReview() {
  const selected = review("Save this service and enable the reviewed task");
  selected.prompt.expires_at = new Date(Date.now() + 60000).toISOString();
  selected.prompt.body = {
    phase: "installed_credential_service_access", software: { server_name: "fixture-service", profile: "fixture" },
    application_selection: { agent: "selected-agent", expires_in_ms: 60000, operations: ["read_fixture"] },
    service_instruction: { proposed_fresh_instruction: "Enable this service for the task", configuration_persistence: {
      kind: "requested_defining_realm_persistence", defining_realm: "fixture-owning-realm", expected_configuration_generation: 4 } },
    notice: "Configuration or credentials can remain after a later failure. Provider authentication is not established."
  };
  selected.credential_input = { kind: "declared_secret_slots", slots: [
    { slot: "FIXTURE_API_KEY", label: "api_key" }, { slot: "FIXTURE_TOKEN", label: "token" }
  ] };
  return selected;
}

test("credential form sends one protected response and never projects entered values", async t => {
  const container = fixture(t); const entered = deferred(); const calls = []; let refreshes = 0;
  const account = mountAccountReview(container, async () => { refreshes++; }, (path, options) => {
    calls.push({ path, options }); return entered.promise;
  });
  t.after(() => account.dispose());
  const selected = credentialReview(); account.update(selected, "actual-viewer");
  const fields = [...container.querySelectorAll('input[type="password"]')];
  assert.equal(fields.length, 2);
  assert.match(container.textContent, /fixture-owning-realm/);
  assert.match(container.textContent, /later step fails or setup is cancelled/);
  const approve = [...container.querySelectorAll("button")].find(button => button.textContent === "Approve & Save");
  approve.click(); assert.equal(calls.length, 0, "missing values must not send approval");
  fields[0].value = "private-test-key"; fields[1].value = "private-test-token";
  assert.doesNotMatch(container.textContent, /private-test-key|private-test-token/);
  assert.doesNotMatch(JSON.stringify(selected), /private-test-key|private-test-token/);
  approve.click(); approve.click();
  assert.equal(calls.length, 1); assert.equal(refreshes, 0);
  assert.deepEqual(fields.map(field => field.value), ["", ""], "blank the actual fields before awaiting the POST");
  assert.equal(calls[0].path, "/host/toolkit/setup/credentials");
  assert.equal(calls[0].options.credentials, "same-origin");
  assert.equal(calls[0].options.redirect, "error");
  assert.deepEqual(JSON.parse(calls[0].options.body), { review_id: 4, approval_id: "actual-prompt", decision: "approve",
    credentials: [{ slot: "FIXTURE_API_KEY", value: "private-test-key" }, { slot: "FIXTURE_TOKEN", value: "private-test-token" }] });
  entered.resolve(new Response("{}", { status: 202 }));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(refreshes, 1); assert.equal(calls.length, 1);
  assert.doesNotMatch(container.textContent, /private-test-key|private-test-token|saved successfully/);
});

test("refresh replacement and retirement blank even retained old password nodes", t => {
  for (const action of ["suspend", "clear", "update", "dispose"]) {
    const container = fixture(t); let calls = 0;
    const account = mountAccountReview(container, async () => {}, async () => { calls++; return new Response(); });
    t.after(() => account.dispose()); account.update(credentialReview(), "actual-viewer");
    const field = container.querySelector('input[type="password"]'); field.value = "private-old-node-value";
    const oldApprove = [...container.querySelectorAll("button")].find(button => button.textContent === "Approve & Save");
    if (action === "update") account.update(credentialReview(), "replacement-viewer"); else account[action]();
    assert.equal(field.value, "", action); assert.equal(field.isConnected, false, action);
    oldApprove.click(); assert.equal(calls, 0, "retired event references cannot submit a replacement review");
    assert.doesNotMatch(container.textContent, /private-old-node-value/);
  }
});

test("automatic polling preserves focused credential input while explicit refresh and pagehide clear it", async t => {
  const { container, timers, transition } = pageFixture(t); let reads = 0;
  const page = mountSourceStatusPage(container, async () => { reads++; return { projection: snapshot(), review: credentialReview() }; });
  t.after(() => page.dispose());
  const settled = () => new Promise(resolve => setImmediate(resolve));
  const tick = async () => { assert.equal(timers.size, 1); const callback = [...timers.values()][0]; timers.clear(); await callback(); };
  await settled(); assert.equal(reads, 1);
  const field = container.querySelector('input[type="password"]'); field.focus();
  await tick(); assert.equal(reads, 1, "an empty focused field must not disappear before typing");
  field.value = "private-poll-value"; field.blur();
  await tick(); assert.equal(reads, 1); assert.equal(field.value, "private-poll-value");
  [...container.querySelectorAll("button")].find(button => button.textContent === "Refresh status").click();
  assert.equal(field.value, "", "the detached old field is blanked synchronously");
  await settled(); assert.equal(reads, 2);
  const replacement = container.querySelector('input[type="password"]'); replacement.value = "private-navigation-value";
  transition("pagehide"); assert.equal(replacement.value, ""); assert.equal(replacement.isConnected, false);
  assert.equal(container.textContent, ""); assert.equal(timers.size, 0);
});


test("native review expiry resumes the existing poll and clears a focused credential form", async t => {
  let now = Date.now(); t.mock.method(Date, "now", () => now);
  const selected = credentialReview();
  const { container, timers } = pageFixture(t); let reads = 0;
  const page = mountSourceStatusPage(container, async () => { reads++; return { projection: snapshot(), review: selected }; });
  t.after(() => page.dispose()); await new Promise(resolve => setImmediate(resolve));
  const field = container.querySelector('input[type="password"]'); field.value = "private-expired-value"; field.focus();
  now += 60001;
  assert.equal(timers.size, 1); const tick = [...timers.values()][0]; timers.clear(); await tick();
  assert.equal(reads, 2); assert.equal(field.value, ""); assert.equal(field.isConnected, false);
  assert.equal(container.querySelector('input[type="password"]'), null);
  assert.match(container.textContent, /review has expired/);
  assert.equal(container.querySelector(".toolkit-account-review button"), null);
  // The retained native expiry controls only stale-form presentation here.
  // Actual HTTP/native refusal and fresh review ownership remain separate.
});

test("malformed or foreign credential projections clear actions without sending input", async t => {
  for (const mutate of [
    selected => { selected.credential_input.slots[1].slot = "FIXTURE_API_KEY"; },
    selected => { selected.credential_input.slots[0].slot = "foreign.value"; },
    selected => { selected.credential_input.slots[0].value = "forbidden-projection-value"; },
    selected => { selected.credential_input.kind = "untrusted_form"; },
    selected => { selected.prompt.body.phase = "installed_public_service_access"; },
  ]) {
    const container = fixture(t); let calls = 0; let panel;
    const selected = credentialReview(); mutate(selected);
    const account = mountAccountReview(container, () => panel.refresh(), async () => { calls++; return new Response(); });
    panel = mountSourceStatusPanel(container, async () => ({ projection: snapshot(), review: selected }), account);
    t.after(() => { panel.dispose(); account.dispose(); });
    await panel.refresh();
    assert.equal(container.querySelector('input[type="password"]'), null);
    assert.equal(container.querySelector(".toolkit-account-review button"), null);
    assert.doesNotMatch(container.textContent, /forbidden-projection-value/);
    assert.match(container.textContent, /status is unavailable/);
    assert.equal(calls, 0);
  }
});


test("pending protected action keeps its offer through automatic and manual status reads", async t => {
  for (const action of ["revoke", "answer", "start"]) {
    const { container, timers } = pageFixture(t);
    const entered = deferred(); let reads = 0; const calls = []; let currentOffer;
    const selectedReview = action === "start" ? { review_id: 8, can_start: true, prompt: null }
      : action === "answer" ? review() : undefined;
    const page = mountSourceStatusPage(container, async () => {
      const page_id = ++reads; currentOffer = page_id;
      return { projection: snapshot(), review: selectedReview,
        page: { page_id, first: true, has_next: true },
        application_access: { ...applicationAccess(), offer_id: page_id } };
    }, (url, options) => { calls.push({ url, body: JSON.parse(options.body) }); return entered.promise; });
    t.after(() => { entered.resolve(new Response("{}", { status: 202 })); page.dispose(); });
    const settled = () => new Promise(resolve => setImmediate(resolve));
    await settled(); assert.equal(reads, 1);
    const selector = action === "revoke" ? ".toolkit-application-access button" : ".toolkit-account-review button";
    const clicked = container.querySelector(selector); clicked.click();
    assert.equal(calls.length, 1); assert.equal(clicked.disabled, true);
    const tick = [...timers.values()][0]; timers.clear(); await tick();
    for (const label of ["Refresh status", "First page", "Next page"]) {
      [...container.querySelectorAll("button")].find(button => button.textContent === label)
        .dispatchEvent(new container.ownerDocument.defaultView.Event("click"));
      await settled();
    }
    assert.equal(reads, 1, `${action}: no status request may retire the offer before POST settlement`);
    assert.equal(currentOffer, 1);
    clicked.dispatchEvent(new container.ownerDocument.defaultView.Event("click"));
    assert.equal(calls.length, 1, "blocked reads never repeat the protected action");
    if (action === "revoke") {
      assert.equal(calls[0].body.offer_id, currentOffer, "the delayed POST still reaches its actual selected offer");
      entered.resolve(accessResponse("revoked"));
    } else entered.resolve(new Response("{}", { status: 202 }));
    await settled();
    assert.equal(reads, 2, "settlement performs the normal current status read");
    assert.equal(calls.length, 1);
    page.dispose();
  }
});

test("stale access offer 409 is definite nonentry while a lost reply remains uncertain", async t => {
  for (const stale of [true, false]) {
    const container = fixture(t); let posts = 0; let refreshes = 0; let panel;
    panel = mountApplicationAccess(container, async () => {
      ++refreshes; panel.update({ ...applicationAccess(), offer_id: 10 }, "person");
    }, async () => {
      ++posts;
      if (!stale) throw Error("lost reply");
      return new Response(JSON.stringify({ error: "Protected setup is unavailable" }), {
        status: 409, headers: { "Content-Type": "application/json" } });
    });
    t.after(() => panel.dispose()); panel.update(applicationAccess(), "person");
    container.querySelector("button").click(); await new Promise(resolve => setImmediate(resolve));
    assert.equal(posts, 1); assert.equal(refreshes, 1);
    assert.match(container.textContent, stale ? /This request revoked nothing/ : /could not be confirmed/);
    assert.doesNotMatch(container.textContent, stale ? /could not be confirmed/ : /This request revoked nothing/);
    assert.equal(container.querySelector("button").disabled, false, "a fresh explicit decision is available");
    await Promise.resolve(); assert.equal(posts, 1, "neither failure class automatically retries");
  }
});

function recoveryOffer(overrides = {}) {
  return { scope:"operator_service_recovery", offer_id:31, rows:[{ id:"b".repeat(64), service:"mail", target:"worker",
    operations:["read"], resource:"inbox", state:"publication_unresolved", parent:"issued", child:"issued", policy:"pending", can_recover:true, ...overrides }] };
}
test("operator recovery is absent without selection and never offers cleanup for a published or unobserved parent", t => {
  const container = fixture(t); const recovery = mountOperatorRecovery(container, async () => {}, async () => { throw Error("no mutation"); });
  t.after(() => recovery.dispose()); recovery.update(undefined, "viewer"); assert.equal(container.textContent, "");
  assert.equal(container.querySelector(".toolkit-view"), null, "absent selection adds no panel");
  recovery.update(recoveryOffer({ parent:"not_observed", child:"not_observed", can_recover:false }), "viewer");
  assert.match(container.textContent, /worker may still enter/); assert.equal(container.querySelectorAll("button").length, 0);
  recovery.update(recoveryOffer({ state:"publication_present", can_recover:false }), "viewer");
  assert.equal(container.querySelectorAll("button").length, 0); assert.match(container.textContent, /matching task source is retained/);
  recovery.clear(); assert.equal(container.textContent, "");
  assert.equal(container.querySelector(".toolkit-view"), null, "retirement removes the recovery panel");
});
test("recovery action names identify each visible service target without changing dispatch", async t => {
  const container = fixture(t); const calls = [];
  const recovery = mountOperatorRecovery(container, async () => {}, async (path, options) => {
    calls.push({ path, body: JSON.parse(options.body) });
    return new Response("{}", { status: 503 });
  });
  t.after(() => recovery.dispose());
  const value = recoveryOffer();
  value.rows.push({ ...value.rows[0], id: "c".repeat(64), target: "other-agent",
    resource: "other-inbox", operations: ["read", "list"] });
  recovery.update(value, "person");
  const buttons = [...container.querySelectorAll("button")];
  assert.deepEqual(buttons.map(managementActionName), value.rows.map(row =>
    `Revoke this unresolved approval ${row.service} Agent ${row.target} Operations ${row.operations.join(", ")} Resource ${row.resource}`));
  assert.ok(buttons.every(button => button.textContent === "Revoke this unresolved approval"));
  buttons[1].click(); await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(calls, [{ path: "/host/toolkit/setup/recovery/revoke",
    body: { offer_id: 31, row_id: "c".repeat(64) } }]);
});
test("operator cleanup posts only the opaque offer once and reports partial native outcomes without claiming policy rollback", async t => {
  const container = fixture(t); const held = deferred(); const calls = []; let refreshes = 0; const submission = { sending:false };
  const recovery = mountOperatorRecovery(container, async () => { refreshes++; recovery.update(recoveryOffer({ parent:"unavailable", child:"unavailable" }), "viewer"); },
    (path, options) => { calls.push({ path, options }); return held.promise; }, submission);
  t.after(() => recovery.dispose()); recovery.update(recoveryOffer(), "viewer");
  const button = container.querySelector("button"); button.click(); button.click();
  assert.equal(calls.length, 1); assert.equal(submission.sending, true);
  assert.equal(calls[0].path, "/host/toolkit/setup/recovery/revoke");
  assert.equal(calls[0].options.credentials, "same-origin"); assert.equal(calls[0].options.redirect, "error");
  assert.deepEqual(JSON.parse(calls[0].options.body), { offer_id:31, row_id:"b".repeat(64) });
  held.resolve(new Response(JSON.stringify({ scope:"operator_service_recovery", parent:"revoked", child:"unresolved", uncertain:true,
    policy_observed_at_offer:"pending", policy_changed:false }), { headers:{"content-type":"application/json"} }));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(refreshes, 1); assert.equal(submission.sending, false);
  assert.match(container.textContent, /parent was revoked/); assert.match(container.textContent, /ownership needs reconciliation/);
  assert.match(container.textContent, /Tool policy was not changed/); assert.match(container.textContent, /Some effects remain uncertain/);
});
test("operator cleanup response lost across retirement is not replayed or applied to another viewer", async t => {
  const container = fixture(t); const held = deferred(); let calls = 0; let refreshes = 0;
  const recovery = mountOperatorRecovery(container, async () => { refreshes++; }, () => { calls++; return held.promise; });
  t.after(() => recovery.dispose()); recovery.update(recoveryOffer(), "old-login"); container.querySelector("button").click();
  recovery.update(undefined, "new-login"); held.reject(Error("connection lost"));
  await new Promise(resolve => setImmediate(resolve)); assert.equal(calls, 1); assert.equal(refreshes, 1); assert.equal(container.textContent, "");
});

test("operator refresh preserves keyboard focus only within the same scope and current row", t => {
  const container = fixture(t); const recovery = mountOperatorRecovery(container, async () => {});
  t.after(() => recovery.dispose()); recovery.update(recoveryOffer(), "viewer"); container.querySelector("button").focus();
  recovery.suspend(); recovery.update(recoveryOffer(), "viewer"); assert.equal(container.ownerDocument.activeElement, container.querySelector("button"));
  recovery.suspend(); recovery.update(recoveryOffer(), "another-viewer"); assert.equal(container.ownerDocument.activeElement, container.ownerDocument.body);
});

for (const kind of ["access", "recovery"]) {
  const selector = kind === "access" ? ".toolkit-application-access" : ".toolkit-operator-recovery";
  const key = kind === "access" ? "application_access" : "operator_recovery";
  const offer = () => kind === "access" ? applicationAccess() : recoveryOffer();
  const refused = () => kind === "access" ? accessResponse("controller_in_use")
    : new Response(JSON.stringify({ scope: "operator_service_recovery", parent: "controller_in_use",
      child: "not_entered", uncertain: false, policy_changed: false }), { headers: { "content-type": "application/json" } });
  const refusal = kind === "access" ? /required by unfinished controller work/ : /Unfinished controller work still requires/;
  const historical = kind === "access" ? /Last revocation attempt:/ : /Last approval cleanup attempt:/;
  const settled = () => new Promise(resolve => setImmediate(resolve));
  const tick = async timers => {
    assert.equal(timers.size, 1); const callback = [...timers.values()][0]; timers.clear(); await callback();
  };

  test(`${kind} management feedback survives ordinary polling as history while owner status stays current`, async t => {
    const { container, timers } = pageFixture(t); const calls = []; const nextAction = deferred();
    let value = offer(); let reads = 0;
    const page = mountSourceStatusPage(container, async () => {
      reads++; return { projection: snapshot(), [key]: { ...value, offer_id: reads } };
    }, async (path, options) => {
      calls.push({ path, body: JSON.parse(options.body) });
      if (calls.length === 1) return refused();
      return nextAction.promise;
    });
    t.after(() => { nextAction.resolve(refused()); page.dispose(); }); await settled();
    container.querySelector(`${selector} button`).click(); await settled();
    assert.match(container.querySelector(selector).textContent, refusal);
    await tick(timers);
    assert.match(container.querySelector(selector).textContent, refusal, "the next ordinary poll must retain the explanation");
    assert.match(container.querySelector(selector).textContent, historical);
    assert.equal(calls.length, 1, "polling must not replay the action");
    const nextOffer = reads;
    container.querySelector(`${selector} button`).click();
    assert.doesNotMatch(container.querySelector(selector).textContent, refusal, "a new action replaces the old explanation");
    nextAction.reject(Error("reply lost")); await settled(); await tick(timers);
    assert.match(container.querySelector(selector).textContent, /result could not be confirmed/);
    assert.match(container.querySelector(selector).textContent, historical);
    assert.deepEqual(calls.map(call => call.body), [
      { offer_id: 1, row_id: value.rows[0].id }, { offer_id: nextOffer, row_id: value.rows[0].id },
    ]);
    assert.ok(calls.every(call => call.path === `/host/toolkit/setup/${kind}/revoke`));
    value = kind === "access" ? applicationAccess("grant_unavailable")
      : recoveryOffer({ state: "publication_present", can_recover: false });
    await tick(timers);
    assert.match(container.querySelector(selector).textContent, /result could not be confirmed/);
    assert.equal(container.querySelector(`${selector} button`), null, "historical feedback grants no current action");
    assert.match(container.querySelector(selector).textContent,
      kind === "access" ? /Access is unavailable/ : /cleanup is unavailable here/);
  });

  test(`${kind} management feedback clears on target or viewer loss, retirement and failed observations`, async t => {
    for (const change of ["target", "viewer", "pagehide", "unavailable", "503"]) {
      const { container, timers, transition } = pageFixture(t); let changed = false; let calls = 0;
      const page = mountSourceStatusPage(container, async () => {
        if (changed && change === "503") { const error = Error("temporarily unavailable"); error.status = 503; throw error; }
        const value = offer();
        if (changed && change === "target") value.rows = [];
        if (changed && change === "unavailable") value.state = "unavailable";
        return { projection: { ...snapshot(), scope: changed && change === "viewer" ? "another-viewer" : "viewer" }, [key]: value };
      }, async () => { calls++; return refused(); });
      t.after(() => page.dispose()); await settled();
      container.querySelector(`${selector} button`).click(); await settled(); await tick(timers);
      assert.match(container.querySelector(selector).textContent, historical, change);
      changed = true;
      if (change === "pagehide") transition("pagehide"); else await tick(timers);
      assert.doesNotMatch(container.textContent, historical, change);
      changed = false;
      if (change === "pagehide") { transition("pageshow"); await settled(); } else await tick(timers);
      assert.doesNotMatch(container.textContent, historical, `${change}: a returning row must not revive retired feedback`);
      assert.equal(calls, 1, change); page.dispose();
    }
  });

  test(`${kind} management feedback cannot return with a late response after its viewer or target retired`, async t => {
    for (const change of ["target", "viewer", "clear"]) {
      const container = fixture(t); const held = deferred(); let calls = 0;
      const mount = kind === "access" ? mountApplicationAccess : mountOperatorRecovery;
      const panel = mount(container, async () => {}, () => { calls++; return held.promise; });
      t.after(() => { held.resolve(refused()); panel.dispose(); }); panel.update(offer(), "viewer");
      container.querySelector("button").click();
      if (change === "clear") panel.clear();
      else panel.update(change === "target" ? { ...offer(), rows: [] } : offer(), change === "viewer" ? "other-viewer" : "viewer");
      panel.update(offer(), "viewer");
      held.resolve(refused()); await settled();
      assert.doesNotMatch(container.textContent, refusal, change);
      assert.doesNotMatch(container.textContent, historical, change);
      assert.equal(calls, 1, change);
    }
  });

  test(`${kind} management feedback stays inside its exact target card after multiple rows reorder`, async t => {
    const { container, timers } = pageFixture(t); const value = offer(); const calls = [];
    const selected = { ...value.rows[0], id: "c".repeat(64), service: "selected-service",
      target: "selected-worker", resource: "selected-resource" };
    value.rows.push(selected);
    const page = mountSourceStatusPage(container, async () => ({ projection: snapshot(), [key]: value }),
      async (path, options) => { calls.push({ path, body: JSON.parse(options.body) }); return refused(); });
    t.after(() => page.dispose()); await settled();
    const cards = () => [...container.querySelectorAll(`${selector} article`)];
    cards()[1].querySelector("button").click(); await settled();
    const checkTarget = () => {
      const target = cards().find(card => card.querySelector("h3").textContent === selected.service);
      const other = cards().find(card => card !== target);
      assert.match(target.textContent, refusal, "the explanation must be inside the selected target's card");
      assert.match(target.querySelector('[role="status"]').textContent, historical);
      assert.doesNotMatch(other.textContent, historical);
      assert.equal(container.querySelectorAll(`${selector} [role="status"]`).length, 1, "reuse the existing status node");
    };
    checkTarget();
    cards()[0].querySelector("button").focus();
    value.rows.reverse(); await tick(timers);
    assert.equal(cards()[0].querySelector("h3").textContent, selected.service);
    checkTarget();
    assert.deepEqual(calls, [{ path: `/host/toolkit/setup/${kind}/revoke`,
      body: { offer_id: value.offer_id, row_id: selected.id } }]);
  });
}


test("503 clears stale setup actions and the existing poll refreshes the retained page; 403 still pauses", async t => {
  for (const status of [503, 403]) {
    const { container, timers } = pageFixture(t); const calls = []; let reads = 0;
    const fetcher = async (path, options) => {
      calls.push({path, options});
      if (++reads === 2) return new Response('{"error":"Protected setup is unavailable"}',
        {status, headers:{"content-type":"application/json"}});
      return new Response(JSON.stringify({projection:snapshot(), review:review("Current review"),
        page:{page_id:17 + reads, first:reads === 1, has_next:true}}),
        {headers:{"content-type":"application/json"}});
    };
    const page = mountSourceStatusPage(container, undefined, fetcher); t.after(() => page.dispose());
    const tick = async () => { assert.equal(timers.size, 1); const callback = [...timers.values()][0]; timers.clear(); await callback(); };
    await new Promise(resolve => setImmediate(resolve));
    assert.ok(container.querySelector(".toolkit-account-review button"));
    await tick();
    assert.equal(container.querySelector(".toolkit-view .toolkit-task"), null);
    assert.equal(container.querySelector(".toolkit-account-review button"), null);
    assert.equal(container.querySelector(".toolkit-application-access button"), null);
    assert.equal(container.querySelector(".toolkit-operator-recovery button"), null);
    assert.doesNotMatch(container.textContent, /Current review|Reviewed task/);
    assert.equal(calls.length, 2, "a failed read cannot enter an immediate retry");
    await tick();
    assert.equal(calls.length, status === 503 ? 3 : 2);
    if (status === 503) {
      assert.deepEqual(calls.map(call => call.options.body && JSON.parse(call.options.body)),
        [undefined, {page_id:18,direction:"refresh"}, {page_id:18,direction:"refresh"}]);
      assert.ok(container.querySelector(".toolkit-account-review button"));
    } else {
      assert.match(container.textContent, /Use First page/);
      assert.equal([...container.querySelectorAll("button")].find(button => button.textContent === "Refresh status").disabled, true);
    }
    assert.ok(calls.every(call => call.path === "/host/toolkit/setup/data"), "no protected action or login is replayed");
    assert.ok(calls.every(call => call.options.credentials === "same-origin"));
    page.dispose(); assert.equal(timers.size, 0);
  }
});

test("an initial 503 leaves only the existing next status poll eligible", async t => {
  const { container, timers } = pageFixture(t); const calls = [];
  const page = mountSourceStatusPage(container, undefined, async (path, options) => {
    calls.push({path, options});
    return calls.length === 1 ? new Response("unavailable", {status:503})
      : new Response(JSON.stringify({projection:snapshot(), page:{page_id:4,first:true,has_next:false}}),
        {headers:{"content-type":"application/json"}});
  }); t.after(() => page.dispose());
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(calls.length, 1); assert.match(container.textContent, /temporarily unavailable/);
  const tick = [...timers.values()][0]; assert.ok(tick); timers.clear(); await tick();
  assert.equal(calls.length, 2); assert.ok(calls.every(call => call.options.method === "GET"));
  assert.match(container.textContent, /Reviewed task/);
});

test("503 recovery never overlaps or replays a protected submission", async t => {
  const { container, timers } = pageFixture(t); const held = deferred(); let reads = 0; let posts = 0;
  const page = mountSourceStatusPage(container, undefined, async (path) => {
    if (path !== "/host/toolkit/setup/data") { assert.equal(path, "/host/toolkit/setup/answer"); posts++; return held.promise; }
    if (++reads === 2) return new Response("unavailable", {status:503});
    return new Response(JSON.stringify({projection:snapshot(), review:reads === 1 ? review() : undefined,
      page:{page_id:reads,first:true,has_next:false}}), {headers:{"content-type":"application/json"}});
  }); t.after(() => page.dispose());
  const tick = async () => { const callback = [...timers.values()][0]; assert.ok(callback); timers.clear(); await callback(); };
  await new Promise(resolve => setImmediate(resolve));
  container.querySelector(".toolkit-account-review button").click();
  assert.equal(posts, 1); await tick(); assert.equal(reads, 1);
  held.resolve(new Response("{}", {status:202})); await new Promise(resolve => setImmediate(resolve));
  assert.equal(posts, 1); assert.equal(reads, 2); assert.match(container.textContent, /temporarily unavailable/);
  await tick(); assert.equal(reads, 3); assert.equal(posts, 1);
});


test("an explicit First after denial can receive 503 and resume the existing read loop", async t => {
  const { container, timers } = pageFixture(t); let reads = 0;
  const page = mountSourceStatusPage(container, undefined, async () => {
    reads++;
    return reads < 3 ? new Response("unavailable", {status:reads === 1 ? 403 : 503})
      : new Response(JSON.stringify({projection:snapshot()}), {headers:{"content-type":"application/json"}});
  }); t.after(() => page.dispose());
  await new Promise(resolve => setImmediate(resolve));
  [...container.querySelectorAll("button")].find(button => button.textContent === "First page").click();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(reads, 2); assert.match(container.textContent, /temporarily unavailable/);
  const tick = [...timers.values()][0]; assert.ok(tick); timers.clear(); await tick();
  assert.equal(reads, 3); assert.match(container.textContent, /Reviewed task/);
});
