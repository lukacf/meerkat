"use strict";
const assert = require("node:assert/strict");
const test = require("node:test");
const { EventEmitter } = require("node:events");
const { _oracles, _rig } = require("./real-images.cjs");
const accepted = { interaction_id: "requested", input_frame_id: "input-requested" };
const done = { kind: "interaction_complete", interaction_id: "requested", payload: { result: "Image operation complete" } };

test("a committed artifact and another interaction's success are not completion", () => {
  assert.equal(_oracles.interactionCompletion([{ kind: "assistant_image", payload: { blob_id: "blob" } }, { ...done, interaction_id: "other" }], accepted, "Image operation complete"), null);
});
test("an image followed by stopped continuation fails even if an old completion exists", () => {
  assert.throws(() => _oracles.interactionCompletion([done, { kind: "text_complete", interaction_id: "requested", payload: { content: "Acceptance scenario stopped: malformed generated result" } }], accepted, "Image operation complete"), /stopped/);
});
test("completion belongs to the accepted interaction and original input", () => {
  assert.throws(() => _oracles.interactionCompletion([{ kind: "user_input", id: accepted.input_frame_id, status: "delivery_failed" }, done], accepted, "Image operation complete"), /delivery_failed/);
});
test("matching instruction and image in different user messages cannot prove ingress", () => {
  const text = { type: "text", text: "inspect" }, image = { type: "image", media_type: "image/png", data: "abc" };
  assert.equal(_oracles.modelImageIngress([{ messages: [{ role: "user", content: [text] }, { role: "user", content: [image] }] }], "inspect", "abc"), null);
  assert(_oracles.modelImageIngress([{ model: "fixture", messages: [{ role: "user", content: [text, image] }] }], "inspect", "abc"));
});
test("tool use id in a call is not a returned image result", () => {
  assert.throws(() => _oracles.imageToolEvidence([{ messages: [{ role: "block_assistant", blocks: [{ block_type: "tool_use", data: { id: "call", name: "generate_image" } }] }] }], "call", "blob"), /result/);
});
test("a successful result for a different blob cannot satisfy image continuation", () => {
  const result = { tool_use_id: "call", is_error: false, content: [{ type: "text", text: JSON.stringify({ terminal: { terminal: "generated" }, images: [{ image_id: "image", blob_ref: { blob_id: "other", media_type: "image/png" } }] }) }] };
  assert.throws(() => _oracles.imageToolEvidence([{ messages: [{ role: "tool_results", results: [result] }] }], "call", "blob"), /blob/);
});
test("three open tasks and two reversed edges cannot satisfy dependency proof", () => {
  const items = ["Review diagram", "Review badge", "Publish report"].map((title, i) => ({ id: String(i), title, status: "open", machine_state: { claim_owner_key: null } }));
  const graph = { items, edges: [{ kind: "blocks", from_id: "2", to_id: "0" }, { kind: "blocks", from_id: "2", to_id: "1" }] };
  assert.throws(() => _oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }), /prerequisite/);
  graph.edges.forEach(edge => { [edge.from_id, edge.to_id] = [edge.to_id, edge.from_id]; });
  assert.deepEqual(_oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }).map(item => item.id), ["0", "1", "2"]);
  items[0].machine_state.claim_owner_key = "some-owner";
  assert.throws(() => _oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }), /unclaimed/);
});

test("unscheduled WorkGraph proof rejects explicit epoch and future scheduling in owner state", () => {
  const items = ["Review diagram", "Review badge", "Publish report"].map((title, i) => ({ id: String(i), title, status: "open", machine_state: { claim_owner_key: null } }));
  const graph = { items, edges: [{ kind: "blocks", from_id: "0", to_id: "2" }, { kind: "blocks", from_id: "1", to_id: "2" }] };
  for (const field of ["due_at", "not_before", "snoozed_until"]) {
    for (const value of ["1970-01-01T00:00:00Z", "9999-12-31T23:59:59Z"]) {
      items[0][field] = value;
      assert.throws(() => _oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }), /unscheduled/, `${field}=${value} is an explicit date, not an absent schedule`);
    }
    delete items[0][field];
    items[0].machine_state[`${field}_utc_ms`] = 0;
    assert.throws(() => _oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }), /unscheduled/, "typed epoch millis must not be treated as falsy absence");
    items[0].machine_state[`${field}_utc_ms`] = null;
  }
  assert.equal(_oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }).length, 3);
});

test("live model cannot silently introduce scheduling and clear it before final inspection", () => {
  const create = { payload: { name: "workgraph_create", args: { title: "Review diagram", realm_id: "mob.console-acceptance" } } };
  assert.doesNotThrow(() => _oracles.assertUnscheduledGraphCalls([create]));
  for (const name of ["workgraph_create", "workgraph_update"]) {
    for (const field of ["due_at", "not_before", "snoozed_until"]) {
      assert.throws(() => _oracles.assertUnscheduledGraphCalls([{ payload: { name, args: { [field]: "1970-01-01T00:00:00Z" } } }]), /unscheduled/);
    }
  }
});

// How a scenario of this file opens the console, and what its failure
// monitor forgives. A page that shows "live" has not always made its last
// start: on a gateway that lists mob addressing the console learns that,
// reads the experience for every mob and starts its unscoped subscription
// once more, cancelling the stream it had. The rows drive the scenario's own
// open and reload over a page and a context that only emit what a browser
// would report, in the order given here; nothing waits on a clock.
const origin = "http://fixture.test:8080";
const seed = { method: "meta/console/query_timeline", params: { mode: "recent", limit: 200 } };
const everyMobSeed = { ...seed, params: { ...seed.params, mobs: "*" } };
const turns = () => new Promise(resolve => setImmediate(resolve));
function stock() {
  const page = new EventEmitter(), context = new EventEmitter();
  const frame = { page: () => page, url: () => `${origin}/console` };
  let live = null;
  const request = (options = {}) => ({
    method: () => options.method || "GET",
    url: () => options.url || `${origin}/console/experience`,
    postData: () => options.body === undefined ? null : JSON.stringify(options.body),
    failure: () => ({ errorText: "net::ERR_ABORTED" }),
    frame: () => frame,
    isNavigationRequest: () => Boolean(options.navigation),
    redirectedFrom: () => null,
    response: async () => options.answer === undefined ? null : { ok: () => true, json: async () => options.answer },
  });
  const start = item => { context.emit("request", item); return item; };
  // A browser reports a failed request to the page and to its context.
  const fail = item => { page.emit("requestfailed", item); context.emit("requestfailed", item); };
  const finish = item => { context.emit("requestfinished", item); return item; };
  const answered = item => context.emit("response", { request: () => item, ok: () => true });
  // A navigation: the document request, the new document committed, and a
  // transport status that is not "live" until the row says so.
  const navigate = async () => {
    const document = start(request({ navigation: true, url: frame.url() }));
    let show;
    live = { shown: new Promise(resolve => { show = resolve; }), show };
    page.emit("framenavigated", frame);
    return { request: () => document, frame: () => frame, url: () => frame.url(), ok: () => true };
  };
  page.mainFrame = () => frame;
  page.context = () => context;
  context.pages = () => [page];
  page.goto = navigate;
  page.reload = navigate;
  page.getByTestId = () => ({ count: async () => 1, getByTestId: () => ({ waitFor: async () => {} }) });
  page.locator = () => ({ waitFor: () => live.shown });
  const stream = (query = "") => request({ url: `${origin}/console/timeline/stream${query}` });
  const read = (body = seed) => request({ method: "POST", url: `${origin}/console/rpc`, body });
  const capabilities = (features = ["mob_addressing"]) => request({ method: "POST", url: `${origin}/console/rpc`, body: { method: "meta/capabilities", params: {} },
    answer: { result: { feature_capabilities: features } } });
  const everyMobExperience = () => request({ url: `${origin}/console/experience?mobs=*` });
  // A gateway that lists no mob addressing: the console starts once, and
  // that start is its last. Returns its stream.
  const onlyStartShowsLive = async () => {
    finish(start(capabilities([])));
    finish(start(read()));
    const only = start(stream());
    answered(only);
    live.show();
    await turns();
    return only;
  };
  // The console's first start in a new document, up to the page showing
  // "live": everything the scenario's own open awaits.
  const firstStartShowsLive = async () => {
    const asked = start(capabilities());
    finish(start(read()));
    const first = start(stream("?mobs=*"));
    answered(first);
    live.show();
    await turns();
    return { asked, first };
  };
  // The console's last start: it has its capabilities, reads the experience
  // for every mob, cancels the stream it had and starts again.
  const lastStart = async ({ asked, first }) => {
    finish(asked);
    finish(start(everyMobExperience()));
    await turns();
    fail(first);
    finish(start(read(everyMobSeed)));
    const last = start(stream("?mobs=*"));
    answered(last);
    await turns();
    return last;
  };
  return { page, fixture: { baseUrl: origin }, request, start, fail, finish, answered, stream, firstStartShowsLive, lastStart, onlyStartShowsLive };
}
const told = failure => `${failure.method} ${new URL(failure.url).pathname}${new URL(failure.url).search} ${failure.error}`;

test("opening the console ends on the console's last start: a stream it replaces after the page shows live is inside the action", async () => {
  const s = stock();
  const monitor = _rig.failureMonitor(s.page, s.fixture);
  const opening = _rig.openStock(s.page, s.fixture, monitor);
  await turns();
  await s.lastStart(await s.firstStartShowsLive());
  await opening;
  assert.deepEqual(monitor.errors, [], "the console's own replaced start is not an unexpected failure");
  assert.deepEqual(monitor.expected.map(told), ["GET /console/timeline/stream?mobs=* net::ERR_ABORTED"]);
});

test("the explicit reload ends on the console's last start too, after the replaced document's stream", async () => {
  const s = stock();
  const monitor = _rig.failureMonitor(s.page, s.fixture);
  // The document that the reload replaces holds a live stream.
  const old = s.start(s.stream("?mobs=*"));
  s.answered(old);
  const reloading = _rig.openStock(s.page, s.fixture, monitor, true);
  await turns();
  s.fail(old);
  await s.lastStart(await s.firstStartShowsLive());
  await reloading;
  assert.deepEqual(monitor.errors, [], "the console's own replaced start is not an unexpected failure");
  assert.deepEqual(monitor.expected.map(told), [
    "GET /console/timeline/stream?mobs=* net::ERR_ABORTED",
    "GET /console/timeline/stream?mobs=* net::ERR_ABORTED",
  ]);
});

test("the two actions forgive nothing else: a stream cancelled after the last start, and another request cancelled inside an action", async () => {
  // On a gateway that lists no mob addressing the console's one start is its
  // last, so nothing is replaced and the action ends with the page "live".
  const after = stock();
  const afterMonitor = _rig.failureMonitor(after.page, after.fixture);
  const opening = _rig.openStock(after.page, after.fixture, afterMonitor);
  await turns();
  const only = await after.onlyStartShowsLive();
  await opening;
  assert.deepEqual(afterMonitor.errors, []);
  assert.deepEqual(afterMonitor.expected, []);
  // The action is over: a cancelled stream has no excuse.
  after.fail(only);
  assert.equal(afterMonitor.errors.length, 1, "a cancellation after the action is unexpected");

  const inside = stock();
  const insideMonitor = _rig.failureMonitor(inside.page, inside.fixture);
  const open = _rig.openStock(inside.page, inside.fixture, insideMonitor);
  await turns();
  // A request of the new document that is not the unscoped subscription.
  inside.fail(inside.start(inside.request()));
  await inside.onlyStartShowsLive();
  await open;
  assert.equal(insideMonitor.errors.length, 1, "inside the action only a replaced start of the subscription is the console's own");
  assert.deepEqual(insideMonitor.expected, []);
});
