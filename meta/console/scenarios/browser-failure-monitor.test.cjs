"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { EventEmitter } = require("node:events");
const { browserFailureMonitor } = require("./browser-failure-monitor.cjs");

const origin = "http://fixture.test:8080";
function setup(options = {}) {
  const page = new EventEmitter();
  const frame = { page: () => page, url: () => `${origin}/scoped` };
  page.mainFrame = () => frame;
  const context = new EventEmitter();
  context.pages = () => [page];
  const monitor = browserFailureMonitor(context, { origin, ...options });
  const request = (options = {}) => ({
    method: () => options.method || "GET",
    url: () => options.url || `${origin}/console/experience`,
    postData: () => options.body === undefined ? null : JSON.stringify(options.body),
    failure: () => ({ errorText: options.error || "net::ERR_ABORTED" }),
    frame: () => options.frame || (options.page ? { page: () => options.page } : frame),
    isNavigationRequest: () => Boolean(options.navigation),
    redirectedFrom: () => options.redirectedFrom || null,
  });
  const start = item => { context.emit("request", item); return item; };
  const fail = item => context.emit("requestfailed", item);
  const beginNavigation = () => {
    const document = start(request({ navigation: true, url: frame.url() }));
    return { request: () => document, frame: () => frame, url: () => frame.url(), ok: () => true };
  };
  const commit = (response = beginNavigation()) => { page.emit("framenavigated", frame); return response; };
  return { context, page, frame, monitor, request, start, fail, beginNavigation, commit };
}
const seed = { method: "meta/console/query_timeline", params: { mode: "recent", limit: 200 } };

test("unexpected aborted requests and uncaught page errors both fail and retain evidence", () => {
  const s = setup();
  const error = new Error("broken rendering");
  s.context.emit("weberror", { page: () => s.page, error: () => error });
  s.page.emit("pageerror", error);
  s.fail(s.start(s.request()));
  assert.equal(s.monitor.errors.length, 2);
  assert.equal(s.monitor.failures[0].expected, false);
  assert.equal(s.monitor.failures[0].error, "net::ERR_ABORTED");
  assert.throws(() => s.monitor.assertClean(), /unexpected browser/);
});

test("navigation permits only exact requests already in flight on that page", async () => {
  const s = setup();
  const original = s.start(s.request());
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    s.fail(original);
    s.fail(s.start(s.request()));
  });
  assert.equal(s.monitor.expected.length, 1);
  assert.equal(s.monitor.errors.length, 1);
  assert.equal(s.monitor.expected[0].reason, "explicit reload: prior document request");
});

test("navigation does not allow failures in another tab or a different failure code", async () => {
  const s = setup();
  const other = s.start(s.request({ page: new EventEmitter() }));
  const reset = s.start(s.request({ error: "net::ERR_CONNECTION_RESET" }));
  await s.monitor.navigation(s.page, "reload one tab", async () => { s.fail(other); s.fail(reset); });
  assert.equal(s.monitor.expected.length, 0);
  assert.equal(s.monitor.errors.length, 2);
});

test("a committed reload accepts an old-document request started after the action snapshot", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    const font = s.start(s.request({ url: "https://fonts.test/font.woff2" }));
    const response = s.beginNavigation();
    s.fail(font);
    return s.commit(response);
  });
  s.monitor.assertClean();
  assert.equal(s.monitor.expected.length, 1);
  assert.equal(s.monitor.failures[0].expected, true);
  assert.equal(s.monitor.expected[0].reason, "explicit reload: replaced document request");
});

test("replacement cannot forgive a new-document request with the same URL", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    const old = s.start(s.request());
    const response = s.commit();
    s.fail(old);
    s.fail(s.start(s.request()));
    return response;
  });
  assert.equal(s.monitor.expected.length, 1);
  assert.equal(s.monitor.errors.length, 1);
});

test("replacement covers a resource started after navigation begins but before the frame commits", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    const document = s.start(s.request({ navigation: true, url: s.frame.url() }));
    s.fail(s.start(s.request()));
    s.page.emit("framenavigated", s.frame);
    return { request: () => document, frame: () => s.frame, url: () => s.frame.url(), ok: () => true };
  });
  s.monitor.assertClean();
  assert.equal(s.monitor.expected.length, 1);
});

test("a committed redirect chain proves replacement through the exact first navigation request", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    const old = s.start(s.request());
    const first = s.start(s.request({ navigation: true, url: `${origin}/redirect` }));
    s.fail(old);
    const last = s.start(s.request({ navigation: true, url: s.frame.url(), redirectedFrom: first }));
    s.page.emit("framenavigated", s.frame);
    return { request: () => last, frame: () => s.frame, url: () => s.frame.url(), ok: () => true };
  });
  s.monitor.assertClean();
  assert.equal(s.monitor.expected.length, 1);
});

for (const witness of ["none", "hash", "unobserved", "wrong-frame", "http-error", "multiple"]) {
  test(`late cancellation stays unexpected with ${witness} replacement evidence`, async () => {
    const s = setup();
    await s.monitor.navigation(s.page, "explicit reload", async () => {
      const old = s.start(s.request());
      const response = s.beginNavigation();
      s.fail(old);
      if (witness === "none") return;
      if (witness === "hash") { s.page.emit("framenavigated", s.frame); return null; }
      s.commit(response);
      if (witness === "unobserved") response.request = () => s.request({ navigation: true, url: s.frame.url() });
      if (witness === "wrong-frame") response.frame = () => ({});
      if (witness === "http-error") response.ok = () => false;
      if (witness === "multiple") s.page.emit("framenavigated", s.frame);
      return response;
    });
    assert.equal(s.monitor.expected.length, 0);
    assert.equal(s.monitor.errors.length, 1);
  });
}

test("replacement retains wrong-error, other-tab, completed-request and navigation-request failures", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    s.fail(s.start(s.request({ error: "net::ERR_CONNECTION_RESET" })));
    s.fail(s.start(s.request({ page: new EventEmitter() })));
    const completed = s.start(s.request());
    s.context.emit("requestfinished", completed);
    s.fail(completed);
    s.fail(s.start(s.request({ navigation: true })));
    return s.commit();
  });
  assert.equal(s.monitor.expected.length, 0);
  assert.equal(s.monitor.errors.length, 4);
});

test("a failed navigation cannot validate a buffered old-document cancellation", async () => {
  const s = setup();
  await assert.rejects(s.monitor.navigation(s.page, "explicit reload", async () => {
    const old = s.start(s.request());
    const response = s.beginNavigation();
    s.fail(old);
    s.commit(response);
    throw new Error("readiness failed");
  }), /readiness failed/);
  assert.equal(s.monitor.errors.length, 1);
  assert.equal(s.page.listenerCount("framenavigated"), 0);
});

test("an abort before navigation starts cannot be forgiven by a later successful reload", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    s.fail(s.start(s.request()));
    return s.commit();
  });
  assert.equal(s.monitor.expected.length, 0);
  assert.equal(s.monitor.errors.length, 1);
});

test("assertClean rejects deferred failures until document replacement is proven", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    const old = s.start(s.request());
    const response = s.beginNavigation();
    s.fail(old);
    assert.throws(() => s.monitor.assertClean(), /still pending/);
    return s.commit(response);
  });
  s.monitor.assertClean();
});

test("replacement cannot override an explicit expected-error mismatch", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    const old = s.start(s.request());
    s.monitor.expectFailure(old, "drop response", "net::ERR_FAILED");
    const response = s.beginNavigation();
    s.fail(old);
    return s.commit(response);
  });
  assert.equal(s.monitor.expected.length, 0);
  assert.equal(s.monitor.errors.length, 1);
});

test("an exact explicit failure during replacement is counted once", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    const old = s.start(s.request());
    s.monitor.expectFailure(old, "deliberate abort", "net::ERR_ABORTED");
    const response = s.beginNavigation();
    s.fail(old);
    return s.commit(response);
  });
  s.monitor.assertClean();
  assert.equal(s.monitor.expected.length, 1);
  assert.equal(s.monitor.expected[0].reason, "deliberate abort");
});

test("unavailable response ownership fails closed and releases the action scope", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    const old = s.start(s.request());
    const response = s.beginNavigation();
    s.fail(old);
    s.commit(response);
    response.frame = () => { throw new Error("Frame is not available"); };
    return response;
  });
  assert.equal(s.monitor.errors.length, 1);
  assert.equal(s.page.listenerCount("framenavigated"), 0);
  const prior = s.start(s.request());
  await s.monitor.navigation(s.page, "next action", async () => s.fail(prior));
  assert.equal(s.monitor.expected.length, 1);
  assert.equal(s.monitor.errors.length, 1);
});

test("replacement proof and candidates end with the action", async () => {
  const s = setup(); let pending;
  await s.monitor.navigation(s.page, "explicit reload", async () => {
    pending = s.start(s.request());
    return s.commit();
  });
  s.fail(pending);
  assert.equal(s.monitor.errors.length, 1);
  assert.equal(s.page.listenerCount("framenavigated"), 0);
});

test("finished requests and allowances from completed actions cannot hide later failures", async () => {
  const s = setup();
  const done = s.start(s.request()); s.context.emit("requestfinished", done);
  const delayed = s.start(s.request());
  await s.monitor.navigation(s.page, "reload", async () => { s.fail(done); });
  s.fail(delayed);
  assert.equal(s.monitor.errors.length, 2);
});

test("initialization allows one exact same-origin unscoped seed and stream request", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "initial host", async () => {
    s.fail(s.start(s.request({ method: "POST", url: `${origin}/console/rpc`, body: seed })));
    s.fail(s.start(s.request({ url: `${origin}/console/timeline/stream` })));
  });
  s.monitor.assertClean();
  assert.equal(s.monitor.expected.length, 2);
  assert(s.monitor.expected.every(item => item.reason === "initial host: authority initialization"));
});

test("initialization cannot forgive duplicate seeds, scoped reads, writes, foreign origins, or wrong failures", async () => {
  const s = setup();
  await s.monitor.navigation(s.page, "initial host", async () => {
    const options = { method: "POST", url: `${origin}/console/rpc`, body: seed };
    s.fail(s.start(s.request(options)));
    s.fail(s.start(s.request(options)));
    s.fail(s.start(s.request({ ...options, body: { ...seed, params: { ...seed.params, identity: "router:main" } } })));
    s.fail(s.start(s.request({ ...options, body: { method: "meta/console/send", params: { content: "do not hide" } } })));
    s.fail(s.start(s.request({ ...options, url: "http://foreign.test/console/rpc" })));
    s.fail(s.start(s.request({ url: `${origin}/console/timeline/stream`, error: "net::ERR_FAILED" })));
  });
  assert.equal(s.monitor.expected.length, 1);
  assert.equal(s.monitor.errors.length, 5);
});

test("initialization allowance ends with its action even for the same request", async () => {
  const s = setup(); let late;
  await s.monitor.navigation(s.page, "initial host", async () => {
    late = s.start(s.request({ url: `${origin}/console/timeline/stream` }));
  });
  s.fail(late);
  assert.equal(s.monitor.errors.length, 1);
});

test("a deliberately dropped response permits only the exact request and error once", () => {
  const s = setup();
  const options = { method: "POST", url: `${origin}/console/rpc`, body: { method: "meta/console/send" }, error: "net::ERR_FAILED" };
  const chosen = s.start(s.request(options));
  s.monitor.expectFailure(chosen, "drop completed owner response", "net::ERR_FAILED");
  s.fail(chosen);
  s.fail(s.start(s.request(options)));
  s.fail(chosen);
  assert.equal(s.monitor.expected.length, 1);
  assert.equal(s.monitor.errors.length, 2);
  assert.equal(s.monitor.failures[0].postData, JSON.stringify(options.body));
});

test("a missing deliberate failure and a wrong error cannot be reported as clean", () => {
  const s = setup();
  const wrong = s.start(s.request());
  s.monitor.expectFailure(wrong, "drop response", "net::ERR_FAILED");
  assert.throws(() => s.monitor.assertClean(), /expected request failure was not observed/);
  s.fail(wrong);
  assert.equal(s.monitor.errors.length, 1);
});

test("a deliberate failure which instead completes remains an unmet expectation", () => {
  const s = setup(); const completed = s.start(s.request());
  s.monitor.expectFailure(completed, "drop response", "net::ERR_FAILED");
  s.context.emit("requestfinished", completed);
  assert.throws(() => s.monitor.assertClean(), /expected request failure was not observed/);
});

test("scope switch snapshots only exact prior requests selected by its authority predicate", async () => {
  const s = setup();
  const old = s.start(s.request({ url: `${origin}/scope-a/console/rpc` }));
  const current = s.start(s.request({ url: `${origin}/scope-b/console/rpc` }));
  await s.monitor.during(s.page, "switch host", async () => {
    s.fail(old); s.fail(current);
    s.fail(s.start(s.request({ url: `${origin}/scope-a/console/rpc` })));
  }, { priorRequests: request => new URL(request.url()).pathname.startsWith("/scope-a/"), initialize: false });
  assert.equal(s.monitor.expected.length, 1);
  assert.equal(s.monitor.errors.length, 2);
});

test("new popup initialization is bounded to one page and records that page's errors", async () => {
  const s = setup(); const popup = new EventEmitter();
  const returned = await s.monitor.popup("explicit clone", async () => {
    s.context.emit("page", popup);
    s.fail(s.start(s.request({ page: popup, url: `${origin}/console/timeline/stream` })));
    return popup;
  });
  assert.equal(returned, popup);
  assert.equal(s.monitor.expected.length, 1);
  const error = new Error("popup broke");
  s.context.emit("weberror", { page: () => popup, error: () => error });
  popup.emit("pageerror", error);
  assert.equal(s.monitor.errors.length, 1);
});

test("a popup request before the page event binds only that first known page", async () => {
  const s = setup(); const popup = new EventEmitter(); const unrelated = new EventEmitter();
  await s.monitor.popup("explicit clone", async () => {
    s.fail(s.start(s.request({ page: popup, url: `${origin}/console/timeline/stream` })));
    s.context.emit("page", popup);
    s.fail(s.start(s.request({ page: popup, method: "POST", url: `${origin}/console/rpc`, body: seed })));
    s.fail(s.start(s.request({ page: unrelated, url: `${origin}/console/timeline/stream` })));
  });
  assert.equal(s.monitor.expected.length, 2);
  assert.equal(s.monitor.errors.length, 1);
  assert(s.monitor.expected.every(item => item.reason === "explicit clone: authority initialization"));
});

test("a frame-unavailable popup request remains unexpected", async () => {
  const s = setup(); const popup = new EventEmitter();
  await s.monitor.popup("explicit clone", async () => {
    const unknown = s.request({ url: `${origin}/console/timeline/stream` });
    unknown.frame = () => { throw new Error("Frame is not available yet"); };
    s.fail(s.start(unknown));
    s.context.emit("page", popup);
    s.fail(s.start(s.request({ page: popup, url: `${origin}/console/timeline/stream` })));
  });
  assert.equal(s.monitor.errors.length, 1);
  assert.equal(s.monitor.expected.length, 1);
});

test("context errors before a popup page event are caught once", async () => {
  const s = setup(); const popup = new EventEmitter(); const error = new Error("early popup broke");
  await s.monitor.popup("explicit clone", async () => {
    s.context.emit("weberror", { page: () => popup, error: () => error });
    popup.emit("pageerror", error);
    s.context.emit("page", popup);
  });
  assert.deepEqual(s.monitor.errors, ["early popup broke"]);
  assert.throws(() => s.monitor.assertClean(), /unexpected browser/);
});

test("context exceptions without a known page fail closed", () => {
  const s = setup();
  s.context.emit("weberror", { page: () => null, error: () => new Error("unknown page broke") });
  assert.deepEqual(s.monitor.errors, ["unknown page broke"]);
  assert.throws(() => s.monitor.assertClean(), /unexpected browser/);
});

test("closing an action with an exception clears its cancellation allowance", async () => {
  const s = setup(); const pending = s.start(s.request());
  await assert.rejects(s.monitor.navigation(s.page, "reload", async () => { throw new Error("navigation failed"); }), /navigation failed/);
  s.fail(pending);
  assert.equal(s.monitor.errors.length, 1);
});

test("stop removes listeners before explicit browser teardown", () => {
  const s = setup();
  s.monitor.stop();
  s.fail(s.request()); s.page.emit("pageerror", new Error("teardown"));
  s.context.emit("weberror", { page: () => s.page, error: () => new Error("teardown") });
  assert.equal(s.monitor.errors.length, 0);
  assert.equal(s.context.listenerCount("requestfailed"), 0);
  assert.equal(s.context.listenerCount("weberror"), 0);
});

test("prefixed initialization only allows the explicitly selected next authority", async () => {
  const s = setup({ initializationPrefixes: ["/scope-a", "/scope-b"] });
  await s.monitor.during(s.page, "switch to B", async () => {
    s.fail(s.start(s.request({ url: `${origin}/scope-b/console/timeline/stream` })));
    s.fail(s.start(s.request({ method: "POST", url: `${origin}/scope-b/console/rpc`, body: seed })));
    s.fail(s.start(s.request({ url: `${origin}/scope-a/console/timeline/stream` })));
    s.fail(s.start(s.request({ url: `${origin}/scope-b-extra/console/timeline/stream` })));
  }, { priorRequests: () => false, initialize: ["/scope-b"] });
  assert.equal(s.monitor.expected.length, 2);
  assert.equal(s.monitor.errors.length, 2);
});

test("unconfigured initialization authority cannot open an allowance", async () => {
  const s = setup();
  await assert.rejects(s.monitor.during(s.page, "invent prefix", async () => {}, { initialize: ["/unknown"] }), /configured initialization prefix/);
  const item = s.start(s.request());
  await s.monitor.navigation(s.page, "later valid action", async () => s.fail(item));
  s.monitor.assertClean();
});
