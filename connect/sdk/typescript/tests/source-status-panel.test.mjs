import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { JSDOM } from "jsdom";
import { mountSourceStatusPanel, readSourceStatus, SOURCE_STATUS_DATA_PATH } from "../dist/ui/source-status-panel.js";

function fixture(t) {
  const dom = new JSDOM("<main></main>");
  const container = dom.window.document.querySelector("main");
  t.after(() => dom.window.close());
  return container;
}
function snapshot(text = "Setup handoff needs attention", tone = "attention") {
  return {
    scope: "fixture-viewer", revision: 0, observedAt: "this request", announcement: text,
    tasks: [{ id: "task-source", title: "Service setup for your task", application: "Fixture",
      requester: "You", reason: "Approved follow-up instruction: read the inbox",
      phase: { text, tone }, nextAction: "Your agent reports the task result separately.",
      roles: [], userActions: [], controls: [] }], connections: [], frontends: [], packages: [],
  };
}
function deferred() { let resolve; let reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }

test("source status uses the real Toolkit attention renderer without actions", async t => {
  const container = fixture(t);
  const panel = mountSourceStatusPanel(container, async () => ({ projection: snapshot() }));
  t.after(() => panel.dispose());
  await panel.refresh();
  assert.match(container.textContent, /Setup handoff needs attention/);
  assert.deepEqual([...container.querySelectorAll(".toolkit-view h2")].map(heading => heading.textContent),
    ["Task setup status", "Setup requests"]);
  assert.doesNotMatch(container.textContent, /No service connections|No agent frontends|No packages/);
  assert.equal(container.querySelector(".toolkit-view [role=status]").textContent, "Setup handoff needs attention");
  assert.deepEqual([...container.querySelectorAll("button")].map(b => b.textContent), ["Refresh status"]);
});

test("empty task status reports only the supplied task inventory", async t => {
  const container = fixture(t); const current = snapshot(); current.tasks = [];
  const panel = mountSourceStatusPanel(container, async () => ({ projection: current }));
  t.after(() => panel.dispose());
  await panel.refresh();
  assert.match(container.textContent, /No setup requests/);
  assert.deepEqual([...container.querySelectorAll(".toolkit-view h2")].map(heading => heading.textContent),
    ["Task setup status", "Setup requests"]);
  assert.doesNotMatch(container.textContent, /No service connections|No agent frontends|No packages/);
});

test("denied or unavailable reads clear prior data without manufacturing an empty snapshot", async t => {
  const container = fixture(t);
  let denied = false;
  const panel = mountSourceStatusPanel(container, async () => { if (denied) throw new Error("unavailable"); return { projection: snapshot() }; });
  t.after(() => panel.dispose());
  await panel.refresh();
  denied = true;
  await panel.refresh();
  assert.equal(container.querySelector(".toolkit-view"), null);
  assert.doesNotMatch(container.textContent, /read the inbox/);
  assert.match(container.textContent, /status is unavailable/);
});

test("one mount serializes reads and retirement suppresses its held response", async t => {
  const container = fixture(t); const held = deferred(); let calls = 0;
  const panel = mountSourceStatusPanel(container, () => { calls++; return held.promise; });
  t.after(() => panel.dispose());
  const first = panel.refresh(); const same = panel.refresh();
  assert.equal(first, same); await Promise.resolve(); assert.equal(calls, 1);
  panel.dispose(); held.resolve({ projection: snapshot() }); await first;
  assert.equal(container.textContent, ""); assert.equal(calls, 1);
});

test("repeated reads only use the one same-origin GET status path", async () => {
  const calls = [];
  const fetcher = async (path, options) => {
    calls.push({ path, options });
    return new Response(JSON.stringify({ projection: snapshot() }), { headers: { "content-type": "application/json" } });
  };
  for (let i = 0; i < 3; ++i) await readSourceStatus(fetcher);
  assert.equal(calls.length, 3);
  for (const { path, options } of calls) {
    assert.equal(path, SOURCE_STATUS_DATA_PATH);
    assert.equal(options.method, "GET");
    assert.equal(options.credentials, "same-origin");
    assert.equal(options.redirect, "error");
    assert.deepEqual(options.headers, { Accept: "application/json" });
    assert.equal(options.body, undefined);
  }
});

test("native admitted-B source fixture projects through the real attention renderer", {
  skip: !process.env.TOOLKIT_SOURCE_STATUS_NATIVE_FIXTURE,
}, async t => {
  // Written only by the actual Rust source-owner fixture after native B
  // admission and its source publication fault. No synthetic fallback here.
  const projection = JSON.parse(await readFile(process.env.TOOLKIT_SOURCE_STATUS_NATIVE_FIXTURE, "utf8"));
  const container = fixture(t);
  const panel = mountSourceStatusPanel(container, async () => ({ projection }));
  t.after(() => panel.dispose());
  await panel.refresh();
  assert.match(container.textContent, /Setup handoff needs attention/);
  assert.match(container.textContent, /follow-up was accepted/);
  assert.match(container.textContent, /will not send the same input again/);
  assert.equal(container.querySelector(".toolkit-view [role=status]").textContent, projection.announcement);
});

test("read-only helper decodes the host envelope and page selection without exposing review actions", async t => {
  const calls = []; const projection = snapshot(); projection.tasks = [];
  const fetcher = async (path, options) => {
    calls.push({ path, options }); return new Response(JSON.stringify({ projection,
      review: { review_id: 7, can_start: true }, application_access: { state: "available" },
      page: { page_id: 10 + calls.length, first: calls.length === 1, has_next: calls.length < 3 } }),
      { headers: { "content-type": "application/json" } });
  };
  const container = fixture(t); const panel = mountSourceStatusPanel(container, selection => readSourceStatus(fetcher, selection));
  t.after(() => panel.dispose());
  await panel.refresh(); assert.match(container.textContent, /No matching setup requests on this page/);
  assert.match(container.textContent, /More history remains to inspect/);
  assert.doesNotMatch(container.textContent, /Start service setup|Revoke access|No service connections/);
  await panel.next(); await panel.refresh();
  assert.deepEqual(calls.map(({options}) => options.body && JSON.parse(options.body)),
    [undefined, { page_id:11, direction:"next" }, { page_id:12, direction:"refresh" }]);
  assert.match(container.textContent, /End of the history observed in this read/);
  assert.equal([...container.querySelectorAll("button")].find(b => b.textContent === "First page").disabled, false);
  assert.equal([...container.querySelectorAll("button")].find(b => b.textContent === "Next page").disabled, true);
  for (const call of calls.slice(1)) {
    assert.equal(call.path, SOURCE_STATUS_DATA_PATH); assert.equal(call.options.method, "POST");
    assert.equal(call.options.credentials, "same-origin"); assert.equal(call.options.headers["Content-Type"], "application/json");
  }
  const selected = await readSourceStatus(fetcher);
  assert.deepEqual(Object.keys(selected).sort(), ["page", "projection"]);
  await assert.rejects(readSourceStatus(fetcher, { page_id:11, direction:"next", principal:"other" }), /Invalid page selection/);
  await assert.rejects(readSourceStatus(async () => new Response(JSON.stringify(projection),
    { headers: { "content-type":"application/json" } })), /Invalid status envelope/);
});

test("a lost read pauses automatic refresh until an explicit First without replaying Next", async t => {
  const container = fixture(t); const calls = []; let fail = false;
  const panel = mountSourceStatusPanel(container, async selection => {
    calls.push(selection); if (fail) throw Error("lost page response");
    return { projection:snapshot(), page:{page_id:17,first:true,has_next:true} };
  }); t.after(() => panel.dispose());
  await panel.refresh(); fail = true; await panel.next();
  assert.equal(container.querySelector(".toolkit-view"), null);
  await panel.refresh(); await panel.next(); assert.equal(calls.length, 2);
  fail = false; await panel.first(); assert.deepEqual(calls, [undefined, {page_id:17,direction:"next"}, undefined]);
});
