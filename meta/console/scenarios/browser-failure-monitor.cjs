"use strict";
const assert = require("node:assert/strict");

const pageMonitors = new WeakMap();
const aborted = "net::ERR_ABORTED";

// While a document initializes the console cancels its own unscoped
// subscription (a recent-page read, then the stream) and starts it again:
// when the experience names its scope and, on a gateway whose capabilities
// list mob addressing, once more after it has read the experience for every
// mob. A cancelled read or stream is the console's own only as a replaced
// start: a read or stream of the same subscription that began later than it,
// in the same action. The browser reports that later start before or after
// the cancellation; either way it must be there by the end of the action.
// Two starts may be replaced in one action and no third, and the second only
// by a start that asks for every mob: a gateway that lists no mob addressing
// replaces one. A cancellation that no later start follows is a failed
// replacement.
//
// An initializing action ends on the console's own last initializing start,
// as the browser's requests show it, whatever the action itself awaited: the
// answers to the capabilities reads the console made in it and, where one
// lists mob addressing, the answer to the experience read for every mob and
// then the start that follows it having gone on unreplaced: an unscoped read
// that began after that answer has finished, and the stream begun after it
// has answered before any newer unscoped read began (a newer read takes its
// place). The storage-scope start and the capability's start send the same
// read, so that last step rests on the order in which the browser runs the
// console's tasks, not on the read's shape. An action whose scenario holds
// that read on purpose says so (`lastStartHeld`) and ends once it has begun.
// A start replaced before the witness is inside the action and judged by
// the rule above; a cancellation after it is outside and unexpected. An
// action that saw no capabilities read under a prefix waits for nothing
// there. The wait is bounded: an action whose witness does not come fails
// and names what is missing.
const replacedStarts = 2;
const mobAddressing = "mob_addressing";
// The unscoped stream: no query, or the one that asks for every mob.
const unscopedStreamQueries = ["", "?mobs=*"];

// Observe the context so popup requests are included from their first request.
// Expected failures name exact Playwright Request objects, never URL wildcards.
function browserFailureMonitor(context, { origin, initializationPrefixes = [""], initializationTimeoutMs = 20_000 }) {
  const authority = new URL(origin).origin;
  assert(initializationPrefixes.length && initializationPrefixes.every(prefix => prefix === "" || /^\/[a-zA-Z0-9_-]+(?:\/[a-zA-Z0-9_-]+)*$/.test(prefix)), "initialization prefixes must be explicit paths");
  const configuredPrefixes = new Set(initializationPrefixes);
  function actionPrefixes(initialize) {
    const selected = initialize === true ? initializationPrefixes : initialize || [];
    assert(Array.isArray(selected) && selected.every(prefix => configuredPrefixes.has(prefix)), "action must select a configured initialization prefix");
    return selected;
  }
  const errors = [], expected = [], failures = [];
  const active = new Map(), allowances = new Map(), actions = new Map(), pages = new Set();
  // The console's capabilities reads and every-mob experience reads of an
  // initializing action, by request, and the order in which they answered.
  const lifecycle = new Map();
  let popupAction = null, order = 0;
  const requestPage = request => { try { return request.frame().page(); } catch { return null; } };
  // The prefix whose unscoped subscription the request belongs to: its
  // recent-page read or its stream, for one mob or for every mob.
  function subscriptionPrefix(request, prefixes) {
    const url = new URL(request.url());
    if (url.origin !== authority) return undefined;
    const prefix = prefixes.find(item => url.pathname === `${item}/console/timeline/stream` || url.pathname === `${item}/console/rpc`);
    if (prefix === undefined) return undefined;
    if (request.method() === "GET" && url.pathname === `${prefix}/console/timeline/stream`) {
      return unscopedStreamQueries.includes(url.search) ? prefix : undefined;
    }
    if (request.method() !== "POST" || url.pathname !== `${prefix}/console/rpc`) return undefined;
    try {
      const body = JSON.parse(request.postData() || "{}");
      return body.method === "meta/console/query_timeline" && body.params?.mode === "recent"
        && body.params?.limit === 200 && !Object.hasOwn(body.params, "identity")
        && !Object.hasOwn(body.params, "mob_id") && (!Object.hasOwn(body.params, "mobs") || body.params.mobs === "*")
        && !Object.hasOwn(body.params, "before") && !Object.hasOwn(body.params, "after") ? prefix : undefined;
    } catch { return undefined; }
  }
  const asksEveryMob = request => {
    if (request.method() === "GET") return new URL(request.url()).search === "?mobs=*";
    try { return JSON.parse(request.postData() || "{}").params?.mobs === "*"; } catch { return false; }
  };
  // The console's capabilities read or its experience read for every mob, under a selected prefix.
  function lifecycleRequest(request, prefixes) {
    const url = new URL(request.url());
    if (url.origin !== authority) return undefined;
    if (request.method() === "GET") {
      const prefix = prefixes.find(item => url.pathname === `${item}/console/experience`);
      return prefix !== undefined && url.search === "?mobs=*" ? { prefix, kind: "experience" } : undefined;
    }
    const prefix = prefixes.find(item => url.pathname === `${item}/console/rpc`);
    if (prefix === undefined || request.method() !== "POST") return undefined;
    try {
      return JSON.parse(request.postData() || "{}").method === "meta/capabilities" ? { prefix, kind: "capabilities" } : undefined;
    } catch { return undefined; }
  }
  function lifeOf(action, prefix) {
    if (!action.life.has(prefix)) action.life.set(prefix, { capabilities: [], experiences: [], reads: [], streams: [] });
    return action.life.get(prefix);
  }
  // What the action has not yet seen of the console's last initializing start under the prefix.
  function missingWitness(action, prefix) {
    const life = action.life.get(prefix);
    if (!life || !life.capabilities.length) return null;
    if (life.capabilities.some(entry => entry.lists === undefined)) return "the answer to its capabilities read";
    if (!life.capabilities.some(entry => entry.lists)) return null;
    const answered = life.experiences.map(entry => entry.answered).filter(Number.isFinite);
    if (!answered.length) return "the answer to its experience read for every mob";
    const first = Math.min(...answered);
    const later = life.reads.filter(read => read.began > first);
    if (!later.length) return "the unscoped read that follows its experience read for every mob";
    if (action.lastStartHeld) return null;
    // The start that read opens has gone on unreplaced.
    const unreplaced = later.some(read => Number.isFinite(read.finished) && life.streams.some(stream =>
      stream.began > read.began && Number.isFinite(stream.answered)
      && !life.reads.some(newer => newer.began > read.began && newer.began < stream.answered)));
    return unreplaced ? null : "the answer to the stream of that read with no newer unscoped read before it";
  }
  function settleLifecycle(request, finished) {
    const item = lifecycle.get(request);
    if (!item) return;
    lifecycle.delete(request);
    if (item.kind === "stream") return;
    if (item.kind === "read") {
      if (finished) { item.entry.finished = ++order; item.action.settle?.(); }
      return;
    }
    if (item.kind === "experience") { item.entry.answered = ++order; item.action.settle?.(); return; }
    const conclude = lists => { item.entry.lists = lists; item.action.settle?.(); };
    if (!finished) { conclude(false); return; }
    Promise.resolve().then(() => request.response()).then(response => response?.ok() ? response.json() : null).then(
      body => conclude(Array.isArray(body?.result?.feature_capabilities) && body.result.feature_capabilities.includes(mobAddressing)),
      () => conclude(false),
    );
  }
  // The unscoped stream of an initializing action has answered.
  function observeResponse(response) {
    let item;
    try { item = lifecycle.get(response.request()); } catch { return; }
    if (item?.kind !== "stream" || Number.isFinite(item.entry.answered) || !response.ok()) return;
    item.entry.answered = ++order;
    item.action.settle?.();
  }
  async function initialized(action) {
    const missing = () => action.prefixes.map(prefix => [prefix, missingWitness(action, prefix)]).filter(([, what]) => what);
    if (!missing().length) return;
    let timer;
    try {
      await new Promise((resolve, reject) => {
        action.settle = () => { if (!missing().length) resolve(); };
        timer = setTimeout(() => {
          const [prefix, what] = missing()[0] || [];
          if (!what) resolve();
          else reject(new Error(`${action.reason}: the console's last initializing start was not seen${prefix ? ` under ${prefix}` : ""}: missing ${what}`));
        }, initializationTimeoutMs);
      });
    } finally { clearTimeout(timer); action.settle = null; }
  }
  // `subscription` names the prefix whose unscoped subscription the request
  // belongs to and its place among that subscription's requests in the
  // action: its cancellation counts only as a replaced start.
  function bind(request, reason, error, action, required = false, subscription = undefined) {
    assert(!allowances.has(request), "a request cannot have overlapping failure allowances");
    allowances.set(request, { reason, error, action, required, subscription });
  }
  function accept(record, reason) {
    record.expected = true;
    record.reason = reason;
    const { expected: ignored, ...evidence } = record;
    expected.push(evidence);
  }
  function observeRequest(request) {
    const page = requestPage(request);
    // Popup subresource requests can precede the context's page event. Bind
    // only a real Request-owned page; frame-unavailable requests stay unknown.
    if (page) attachPage(page);
    active.set(request, page);
    const action = actions.get(page);
    const document = action?.document;
    if (document && request.frame() === document.frame) {
      if (request.isNavigationRequest()) {
        document.request ||= request;
      } else if (!document.fenced) document.candidates.add(request);
    }
    if (!action?.prefixes.length) return;
    // In a navigation only the new document's requests tell of its initialization.
    const fresh = !document || document.fenced;
    const lifecycleOf = fresh ? lifecycleRequest(request, action.prefixes) : undefined;
    if (lifecycleOf) {
      const entry = lifecycleOf.kind === "experience" ? { answered: undefined } : { lists: undefined };
      lifeOf(action, lifecycleOf.prefix)[lifecycleOf.kind === "experience" ? "experiences" : "capabilities"].push(entry);
      lifecycle.set(request, { action, kind: lifecycleOf.kind, entry });
      return;
    }
    const prefix = subscriptionPrefix(request, action.prefixes);
    if (prefix === undefined) return;
    if (!action.starts.has(prefix)) action.starts.set(prefix, { replaced: 0, begun: 0, everyMob: 0, cancelled: [] });
    const start = action.starts.get(prefix);
    const everyMob = asksEveryMob(request);
    start.begun += 1;
    if (everyMob) start.everyMob = start.begun;
    // This request is the successor of the starts cancelled before it; of the
    // second replaced start only when it asks for every mob.
    start.cancelled = start.cancelled.filter(item => {
      if (item.second && !everyMob) return true;
      accept(item.record, `${action.reason}: authority initialization`);
      return false;
    });
    bind(request, `${action.reason}: authority initialization`, aborted, action, false, { prefix, order: start.begun });
    if (fresh) {
      const read = request.method() === "POST";
      const entry = read ? { began: ++order, finished: undefined } : { began: ++order, answered: undefined };
      lifeOf(action, prefix)[read ? "reads" : "streams"].push(entry);
      lifecycle.set(request, { action, kind: read ? "read" : "stream", entry });
      action.settle?.();
    }
  }
  function observeFinished(request) {
    settleLifecycle(request, true);
    active.delete(request);
    for (const action of actions.values()) action.document?.candidates.delete(request);
    if (!allowances.get(request)?.required) allowances.delete(request);
  }
  const errorText = failure => `${failure.method} ${failure.url}: ${failure.error} ${failure.postData || ""}`;
  function observeFailure(request) {
    settleLifecycle(request, false);
    const allowance = allowances.get(request);
    const failure = { method: request.method(), url: request.url(), postData: request.postData(), error: request.failure()?.errorText };
    const matches = Boolean(allowance && failure.error === allowance.error);
    // A cancelled start waits for its successor; an action replaces no third.
    const start = matches && allowance.subscription !== undefined ? allowance.action.starts.get(allowance.subscription.prefix) : undefined;
    const accepted = matches && !start;
    const record = { ...failure, expected: accepted, ...(accepted ? { reason: allowance.reason } : {}) };
    failures.push(record);
    const document = actions.get(active.get(request))?.document;
    if (accepted) expected.push({ ...failure, reason: allowance.reason });
    else if (start && start.replaced < replacedStarts) {
      start.replaced += 1;
      // Its successor has begun already, or the cancellation waits for it.
      // The second replaced start is the capability's: its successor asks for every mob.
      const second = start.replaced === replacedStarts;
      if ((second ? start.everyMob : start.begun) > allowance.subscription.order) accept(record, allowance.reason);
      else start.cancelled.push({ record, second });
    }
    else if (!allowance && failure.error === aborted && document?.request && document.candidates.has(request)) document.failures.push(record);
    else errors.push(errorText(failure));
    document?.candidates.delete(request);
    active.delete(request); allowances.delete(request);
  }
  function attachPage(page) {
    if (pages.has(page)) return;
    pages.add(page); pageMonitors.set(page, monitor);
    if (popupAction && !popupAction.page) {
      popupAction.page = page; actions.set(page, popupAction);
    }
  }
  // Context errors include exceptions before a popup page event. Listening to
  // this one owner event also avoids counting the mirrored pageerror twice.
  const observeWebError = webError => errors.push(webError.error().message);
  function replacedDocument(document, response) {
    try {
      if (!response?.ok?.() || document.boundaries.length !== 1 || response.frame() !== document.frame
        || response.url() !== document.boundaries[0]) return false;
      const request = response.request();
      if (!request.isNavigationRequest() || request.frame() !== document.frame) return false;
      const seen = new Set();
      for (let current = request; current && !seen.has(current); current = current.redirectedFrom?.()) {
        if (current === document.request) return true;
        seen.add(current);
      }
    } catch { /* Unavailable frame ownership cannot prove replacement. */ }
    return false;
  }
  function finishAction(action, response) {
    if (action.document) {
      const document = action.document;
      document.page.off("framenavigated", document.onNavigation);
      const replaced = replacedDocument(document, response);
      for (const failure of document.failures) {
        if (replaced) {
          failure.expected = true;
          failure.reason = `${action.reason}: replaced document request`;
          const { expected: ignored, ...evidence } = failure;
          expected.push(evidence);
        } else errors.push(errorText(failure));
      }
    }
    // No successor began in the action: a failed replacement.
    for (const start of action.starts.values()) for (const item of start.cancelled.splice(0)) errors.push(errorText(item.record));
    for (const [request, item] of lifecycle) if (item.action === action) lifecycle.delete(request);
    for (const [request, allowance] of allowances) if (allowance.action === action) allowances.delete(request);
    for (const [page, value] of actions) if (value === action) actions.delete(page);
  }
  const monitor = {
    errors, expected, failures,
    expectFailure(request, reason, error) {
      assert(reason && error, "deliberate request failure requires an action reason and exact error");
      assert(active.has(request), "deliberate failure must identify an observed in-flight request");
      bind(request, reason, error, null, true);
    },
    async during(page, reason, action, { priorRequests = () => true, initialize = false, documentNavigation = false, lastStartHeld = false } = {}) {
      assert(!actions.has(page), "failure action scopes must not overlap on a page");
      const scope = { reason, prefixes: actionPrefixes(initialize), starts: new Map(), life: new Map(), settle: null, lastStartHeld };
      if (documentNavigation) {
        // Requests can begin after the action snapshot while the old document
        // is still running, even after navigation begins. Stop collecting at
        // the frame event, and require the actual committed Response as proof.
        const document = { page, frame: page.mainFrame(), candidates: new Set(), failures: [], boundaries: [], fenced: false, request: null };
        document.onNavigation = frame => {
          if (frame !== document.frame) return;
          document.fenced = true;
          document.boundaries.push(frame.url());
        };
        scope.document = document;
        page.on("framenavigated", document.onNavigation);
      }
      actions.set(page, scope);
      let response;
      try {
        for (const [request, owner] of active) if (owner === page && priorRequests(request)) {
          bind(request, `${reason}: prior document request`, aborted, scope);
        }
        response = await action();
        await initialized(scope);
        return response;
      } finally { finishAction(scope, response); }
    },
    navigation(page, reason, action) {
      return monitor.during(page, reason, action, { initialize: true, documentNavigation: true });
    },
    async popup(reason, action) {
      assert.equal(popupAction, null, "popup action scopes must not overlap");
      const scope = { reason, prefixes: actionPrefixes(true), starts: new Map(), life: new Map(), settle: null, page: null };
      popupAction = scope;
      try {
        const opened = await action();
        await initialized(scope);
        return opened;
      } finally { popupAction = null; finishAction(scope); }
    },
    assertClean() {
      assert([...actions.values()].every(action => !action.document?.failures.length), "navigation cancellation evidence is still pending");
      assert([...actions.values(), ...(popupAction ? [popupAction] : [])].every(action => ![...action.starts.values()].some(start => start.cancelled.length)),
        "initialization replacement evidence is still pending");
      assert.deepEqual(errors, [], "no unexpected browser exceptions or failed requests");
      const missing = [...allowances].filter(([, allowance]) => allowance.required).map(([request, allowance]) => ({
        method: request.method(), url: request.url(), reason: allowance.reason,
      }));
      assert.deepEqual(missing, [], "expected request failure was not observed");
    },
    stop() {
      context.off("request", observeRequest); context.off("requestfinished", observeFinished);
      context.off("requestfailed", observeFailure); context.off("page", attachPage);
      context.off("response", observeResponse);
      context.off("weberror", observeWebError);
      for (const page of pages) pageMonitors.delete(page);
      pages.clear();
    },
  };
  for (const page of context.pages()) attachPage(page);
  context.on("page", attachPage); context.on("request", observeRequest);
  context.on("weberror", observeWebError);
  context.on("requestfinished", observeFinished); context.on("requestfailed", observeFailure);
  context.on("response", observeResponse);
  return monitor;
}

function navigation(page, reason, action) {
  const monitor = pageMonitors.get(page);
  assert(monitor, "navigation must have an attached browser failure monitor");
  return monitor.navigation(page, reason, action);
}

module.exports = { browserFailureMonitor, navigation };
