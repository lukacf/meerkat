import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createRequire } from "node:module";
import { setImmediate as nextTurn } from "node:timers/promises";
import React, { act } from "react";
import { createGuideLoadGate } from "./wizard-state.mjs";

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const state = { id: "owned-stop-guide", version: 12, proposal: { target: "mac" } };
const otherState = { ...state, id: "other-stop-guide" };
const endpoint = `/api/guide/${state.id}/implementation`;
const otherEndpoint = `/api/guide/${otherState.id}/implementation`;
const feedbackKey = `mobkit-implementation-feedback-v1:${state.id}`;
const initialDraft = "  Keep these exact notes\nfor the next application build.  ";
const latestDraft = "  A newer preference\nthat the delayed admission must retain.  ";
const otherDraft = "  Notes for the other guide\nthat must stay with that guide.  ";
const receipt = (buildId, changes = {}) => ({ guide_id: state.id, guide_version: state.version,
  build_id: buildId, current: true, status: "running", ready_to_package: false,
  summary: `Selected source ${buildId}`, ...changes });
const response = (value) => ({ ok: true, json: async () => structuredClone(value) });
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};

async function mounted({ initial = null, onPost, draft = initialDraft, initialStorageFailure = null,
  initialStatusResponse, selectionScope, checkerRequest,
  checker = {status:'ready',can_prepare:false,detail:'Synthetic ready checker'} }, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  dom.window.localStorage.setItem(feedbackKey, draft);
  let feedbackStorageFailure = initialStorageFailure;
  const feedbackStorage = {
    getItem(key) {
      if (key.startsWith('mobkit-implementation-feedback-v1:') && feedbackStorageFailure === 'read-write')
        throw new Error('Synthetic feedback storage access is denied.');
      return dom.window.localStorage.getItem(key);
    },
    setItem(key, value) {
      if (key.startsWith('mobkit-implementation-feedback-v1:') && feedbackStorageFailure)
        throw new Error('Synthetic feedback storage quota is exhausted.');
      return dom.window.localStorage.setItem(key, value);
    },
    removeItem(key) {
      if (key.startsWith('mobkit-implementation-feedback-v1:') && feedbackStorageFailure)
        throw new Error('Synthetic feedback storage access is denied.');
      return dom.window.localStorage.removeItem(key);
    },
  };
  let selected = initial, admittedReceipt, statusReads = 0, timerId = 0, heldStatus, nextStatusFailure, viewState = state;
  const calls = [], reported = [], timers = new Map();
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: feedbackStorage, IS_REACT_ACT_ENVIRONMENT: true,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    fetch: async (target, options = {}) => {
      const method = options.method || "GET";
      const body = options.body ? JSON.parse(options.body) : null;
      calls.push({ target, method, body });
      if (target === '/api/checker-setup') {
        if (checkerRequest) return checkerRequest({ method, body, options });
        if (method === 'GET') return response(checker);
      }
      if (method === "POST") {
        assert.ok(target === endpoint || target === `${endpoint}/stop`, `Unexpected fixture action: ${target}`);
        assert.equal(typeof onPost, "function", "The test did not authorize an action");
        const returned = await onPost({ target, body });
        return { ...returned, json: async () => {
          const next = await returned.json();
          // Decode only when the real request reads the body. Server selection
          // still follows the existing exact selected-build admission fence.
          const matching = target.endsWith("/stop") ? selected?.build_id === body.build_id
            : (selected?.build_id ?? null) === body.previous_build_id;
          if (returned.ok && matching) selected = next;
          return structuredClone(next);
        } };
      }
      if (target === endpoint) {
        statusReads += 1;
        if (statusReads === 1 && initialStatusResponse) return initialStatusResponse;
        if (heldStatus) return heldStatus.promise;
        if (nextStatusFailure) {
          const failed = nextStatusFailure;
          nextStatusFailure = undefined;
          return failed;
        }
        return selected ? response(selected) : { ok: false, status: 404, json: async () => ({ detail: "No saved build." }) };
      }
      if (target === otherEndpoint) return { ok: false, status: 404, json: async () => ({ detail: "No saved build." }) };
      assert.ok([endpoint, otherEndpoint].some((base) => target.startsWith(`${base}/timeline?`)),
        `Unexpected fixture read: ${target}`);
      return response({ build_id: new URL(target, "http://localhost").searchParams.get("build_id"),
        frames: [], has_more: false });
    } };
  const previous = new Map();
  const replace = (key, value) => {
    previous.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
    Object.defineProperty(globalThis, key, { configurable: true, value });
  };
  for (const [key, value] of Object.entries(replacements)) replace(key, value);
  let root;
  try {
    const built = await build({ entryPoints: [fileURLToPath(new URL("implementation-panel.jsx", import.meta.url))],
      bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent",
      loader: { ".css": "empty" }, nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
      alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
      plugins: [{ name: "shared-mounted-react", setup(builder) {
        builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
          path: pathToFileURL(require.resolve(args.path)).href, external: true,
        }));
      } }] });
    const { ImplementationPanel } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
    const { createRoot } = await import("react-dom/client");
    // Advance actual component polls deterministically, without wall-clock waits.
    replace("setTimeout", (callback, delay, ...args) => {
      const id = ++timerId;
      timers.set(id, { callback, delay, args });
      return id;
    });
    replace("clearTimeout", (id) => timers.delete(id));
    root = createRoot(dom.window.document.getElementById("root"));
    const flush = async (operation = () => {}) => act(async () => {
      await operation();
      await nextTurn();
      await nextTurn();
    });
    const onReceipt = (value) => reported.push(value);
    const render = () => flush(() => root.render(React.createElement(ImplementationPanel,
      { state: viewState, plan: { project: { name: "Synthetic stop fixture" } }, Icon: () => null,
        onReceipt, admittedReceipt, selectionScope,
        navigation: viewState.id === state.id && selected?.source_digest ? { guide_id: state.id, guide_version: state.version,
          build_id: selected.build_id, source_digest: selected.source_digest,
          actions: [{ target: "trial", label: "Try saved application" },
            { target: "deployment", label: "Install saved application" },
            { target: "console", label: "Open installed console", url: "http://localhost/console" }] } : null,
        onNavigate: () => assert.fail("Blocked build must not admit a new first-use action") })));
    const button = (label) => [...dom.window.document.querySelectorAll("button")]
      .find((item) => typeof label === "string" ? item.textContent.trim() === label : label.test(item.textContent));
    const edit = (text) => flush(() => {
      const textarea = dom.window.document.querySelector("textarea");
      assert.ok(textarea, "The pending admission must retain the editable feedback composer");
      Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set.call(textarea, text);
      textarea.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
    });
    const poll = async (value) => {
      selected = value;
      const before = statusReads;
      for (let attempt = 0; statusReads === before && attempt < 8; attempt++) {
        const entry = timers.entries().next().value;
        assert.ok(entry, "The mounted component must schedule its next receipt poll");
        const [id, scheduled] = entry;
        timers.delete(id);
        await flush(() => scheduled.callback(...scheduled.args));
      }
      assert.ok(statusReads > before, "The newer receipt must come through the real polling path");
    };
    await render();
    await verify({ dom, calls, reported, button, edit, flush, poll, rerender: render,
      pollChecker: async () => {
        const entry = [...timers.entries()].find(([, scheduled]) => scheduled.delay === 2000);
        assert.ok(entry, "The current checker preparation schedules a read-only status poll.");
        const [id, scheduled] = entry; timers.delete(id);
        await flush(() => scheduled.callback(...scheduled.args));
      },
      current: () => reported.at(-1),
      holdStatusReads: () => {
        assert.equal(heldStatus, undefined);
        const held = deferred();
        heldStatus = held;
        return () => { heldStatus = undefined; held.resolve(response(selected)); };
      },
      admit: async (value) => { selected = value; admittedReceipt = value; await render(); },
      selectOnServer: (value) => { selected = value; },
      failNextRead: () => { nextStatusFailure = { ok: false, status: 503,
        json: async () => ({ detail: "Build status is temporarily unavailable." }) }; },
      navigate: async (guide) => { viewState = guide; admittedReceipt = undefined; await render(); },
      remount: async () => {
        await flush(() => root.unmount());
        root = createRoot(dom.window.document.getElementById("root"));
        await render();
      },
      text: () => dom.window.document.querySelector("textarea")?.value,
      denyFeedbackPersistence: (mode = 'write') => { feedbackStorageFailure = mode; },
      restoreFeedbackPersistence: () => { feedbackStorageFailure = null; },
      savedDraft: (guideId = state.id) => dom.window.localStorage.getItem(`mobkit-implementation-feedback-v1:${guideId}`) });
  } finally {
    if (root) await act(async () => root.unmount());
    for (const [key, descriptor] of [...previous].reverse()) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
    dom.window.close();
  }
}

const acceptedRuntimeBuild = () => receipt("runtime-A", { status: "completed", source_digest: "runtime-source",
  worker_stop_confirmed: true, cleanup_pending: false, cleanup_in_progress: false,
  ready_to_package: true, source_verified: true, acceptance_current: true, acceptance_trusted: true,
  acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "runtime-source", status: "pass", tests_run: 3 } });

for (const storageFailure of ['write', 'read-write'])
test(`mounted A -> B -> A builder notes survive ${storageFailure} storage failure without submitting`, async () => {
  const noteA = '  Nasta observation \u00e5\u{1f9ed}\nKeep the latest A instructions.  ';
  const noteB = '  Separate B note \u03bb\nDo not send or mix this with A.  ';
  await mounted({}, async ui => {
    ui.denyFeedbackPersistence(storageFailure);
    await ui.edit(noteA);
    assert.equal(ui.savedDraft(), initialDraft, 'A failed write leaves only the older browser-storage value.');
    await ui.navigate(otherState);
    await ui.edit(noteB);
    await ui.navigate(state);
    assert.equal(ui.text(), noteA, 'The open page must retain the exact newer unsent A note, not reread stale storage.');
    assert.match(ui.dom.window.document.querySelector('.implementation-feedback [role="status"]').textContent,
      /kept in this open page only.*reload/i);
    await ui.remount();
    assert.equal(ui.text(), noteA, 'Returning to the builder panel in this page must retain A even after the panel unmounted.');
    await ui.navigate(otherState);
    assert.equal(ui.text(), noteB, 'B retains its own exact note while A stays separate.');
    await ui.remount();
    assert.equal(ui.text(), noteB, 'The same page retains the separate B draft across its own panel remount.');
    await ui.navigate(state);
    ui.restoreFeedbackPersistence();
    await ui.flush(() => ui.button('Retry saving builder note').click());
    assert.equal(ui.savedDraft(), noteA);
    assert.equal(ui.dom.window.document.querySelector('.implementation-feedback [role="status"]') === null, true);
    await ui.navigate(otherState);
    assert.equal(ui.text(), noteB);
    await ui.flush(() => ui.button('Retry saving builder note').click());
    assert.equal(ui.savedDraft(otherState.id), noteB);
    assert.equal(ui.calls.filter(call => call.method === 'POST').length, 0,
      'Navigation and storage retry never submit a build or native operation.');
  });
});

test('mounted unread builder note retry recovers its older saved text before any edit without erasing it', async () => {
  await mounted({ initialStorageFailure: 'read-write' }, async ui => {
    assert.equal(ui.text(), '');
    assert.equal(ui.savedDraft(), initialDraft);
    assert.ok(ui.button('Retry saving builder note'));
    ui.restoreFeedbackPersistence();
    await ui.flush(() => ui.button('Retry saving builder note').click());
    assert.equal(ui.text(), initialDraft, 'Retry reads the unknown stored note instead of writing the unread empty view.');
    assert.equal(ui.savedDraft(), initialDraft);
    assert.equal(ui.dom.window.document.querySelector('.implementation-feedback [role="status"]') === null, true);
    assert.equal(ui.calls.filter(call => call.method === 'POST').length, 0);
  });
});

test('mounted old build admission cannot clear a newly edited remounted page-only note', async () => {
  const pending = deferred(), note = '  Remounted note \u00f6\u{1f9ed}\nKeep this newer human instruction.  ';
  await mounted({ onPost({ target, body }) {
    assert.equal(target, endpoint);
    assert.deepEqual(body, { version: state.version, request: initialDraft.trim(), previous_build_id: null });
    return pending.promise;
  } }, async ui => {
    ui.denyFeedbackPersistence('read-write');
    await ui.flush(() => ui.button('Build application').click());
    assert.equal(ui.calls.filter(call => call.method === 'POST').length, 1);
    await ui.remount();
    assert.equal(ui.text(), initialDraft, 'A pending build does not consume the unsent page draft during remount.');
    await ui.edit(note);
    await ui.flush(() => pending.resolve(response(receipt('retired-admission', { status: 'queued' }))));
    assert.equal(ui.text(), note, 'The retired panel response must not clear a newly edited remounted draft.');
    assert.equal(ui.savedDraft(), initialDraft, 'Denied storage still retains its older bytes.');
    await ui.remount();
    assert.equal(ui.text(), note, 'The newer unsaved draft remains authoritative after the old response and another remount.');
    assert.match(ui.dom.window.document.querySelector('.implementation-feedback [role="status"]').textContent,
      /kept in this open page only.*reload/i);
    assert.equal(ui.calls.filter(call => call.method === 'POST').length, 1,
      'Neither remount nor response recovery replays the submitted build.');
  });
});

test('missing app checker disables costly build while retaining the exact draft and saved package', async () => {
  await mounted({ initial: acceptedRuntimeBuild(), checker: {status:'needs_preparation',can_prepare:true,detail:'Fixture needs preparation'} },
    async ui => {
      const build=ui.button(/Continue implementation|Build application/);
      assert.ok(build); assert.equal(build.disabled,true);
      assert.ok(ui.button('Prepare app checks'));
      const download = ui.button('Download checked application');
      assert.equal(download.disabled, false);
      assert.equal(build.classList.contains('primary'), false);
      assert.equal(download.classList.contains('primary'), false);
      for (const label of ['Try saved application', 'Install saved application'])
        assert.equal(ui.button(label).classList.contains('primary'), true);
      const consoleLink = [...ui.dom.window.document.querySelectorAll('a')]
        .find(link => link.textContent.trim() === 'Open installed console');
      assert.equal(consoleLink.classList.contains('primary'), false);
      await ui.flush(()=>build.click());
      assert.equal(ui.calls.filter(c=>c.method==='POST').length,0);
      assert.equal(ui.text(),initialDraft);
      assert.equal(ui.savedDraft(),initialDraft);
      const accepted = acceptedRuntimeBuild();
      for (const change of [{ source_verified: false }, { acceptance_trusted: false }, { acceptance_current: false },
        { acceptance: { ...accepted.acceptance, status: 'needs_repair' } }]) {
        await ui.poll({ ...accepted, ...change });
        const continueOrRepair = ui.button(/Continue implementation|Continue repairing application|Review and improve application/);
        assert.ok(continueOrRepair && continueOrRepair.classList.contains('primary'),
          'Unverified source or independent findings keep the existing build action prominent.');
        assert.equal(ui.button('Download checked application').classList.contains('primary'), true);
      }
      await ui.poll(accepted);
      assert.equal(ui.button('Continue implementation').classList.contains('primary'), false);
      ui.failNextRead();
      await ui.poll(accepted);
      assert.equal(ui.button('Refresh build status').classList.contains('primary'), true);
      assert.equal(ui.calls.filter(call => call.method === 'POST').length, 0);
      assert.equal(ui.text(), initialDraft);
    });
});

function pendingCheckerPreparation() {
  const body = deferred();
  const host = { value: { status: 'needs_preparation', can_prepare: true, detail: 'Fixture needs preparation.' },
    bodyStarted: false, settled: false, posts: 0, nextRead: null };
  host.request = ({ method, options }) => {
    if (method === 'POST') {
      assert.equal(++host.posts, 1, 'Status recovery must not send another preparation request.');
      assert.equal(options.signal, undefined, 'The original preparation request is not aborted or given a deadline.');
      return { ok: true, json: async () => {
        host.bodyStarted = true;
        const value = await body.promise;
        host.settled = true;
        if (value instanceof Error) throw value;
        return value;
      } };
    }
    assert.equal(method, 'GET');
    if (host.nextRead) { const next = host.nextRead; host.nextRead = null; return next; }
    return response(host.value);
  };
  host.finish = value => body.resolve(value);
  return host;
}

for (const outcome of ['success', 'error']) test(`checker preparation response recovery enables the real builder and fences late POST ${outcome}`, async () => {
  const host = pendingCheckerPreparation();
  await mounted({ checkerRequest: host.request }, async ui => {
    assert.equal(ui.button('Build application').disabled, true);
    await ui.flush(() => ui.button('Prepare app checks').click());
    assert.equal(host.bodyStarted, true, 'The actual POST JSON body decoder is still pending.');
    const recovery = ui.button('Check preparation status');
    assert.ok(recovery && !recovery.disabled, 'The actual builder exposes read-only preparation recovery.');
    await ui.edit(latestDraft);
    host.value = { status: 'ready', can_prepare: false, detail: 'The owned preparation finished.' };
    await ui.flush(() => recovery.click());
    assert.equal(host.settled, false, 'GET readiness does not cancel or finish the original response promise.');
    assert.equal(ui.button('Build application').disabled, false);
    assert.match(ui.dom.window.document.querySelector('.setup-checker').textContent, /App checks ready/);
    assert.equal(ui.text(), latestDraft); assert.equal(ui.savedDraft(), latestDraft);
    const calls = ui.calls.length;
    await ui.flush(() => host.finish(outcome === 'success'
      ? { status: 'failed', can_prepare: true, detail: 'OLD POST STATUS MUST NOT REPLACE READY' }
      : new Error('OLD POST ERROR MUST NOT REPLACE READY')));
    assert.equal(host.settled, true);
    assert.equal(ui.button('Build application').disabled, false);
    assert.match(ui.dom.window.document.querySelector('.setup-checker').textContent, /App checks ready/);
    assert.doesNotMatch(ui.dom.window.document.body.textContent, /OLD POST|Preparation was not confirmed/);
    assert.equal(ui.dom.window.document.querySelector('.setup-checker [role="alert"]') === null, true);
    assert.equal(ui.text(), latestDraft); assert.equal(ui.savedDraft(), latestDraft);
    assert.equal(ui.calls.length, calls, 'A late POST response starts no refresh, retry or build.');
    assert.equal(host.posts, 1);
    assert.equal(ui.calls.filter(call => call.method === 'POST' && call.target !== '/api/checker-setup').length, 0);
  });
});

test('checker preparation response recovery follows an existing preparation until ready while POST stays pending', async () => {
  const host = pendingCheckerPreparation();
  await mounted({ checkerRequest: host.request }, async ui => {
    await ui.flush(() => ui.button('Prepare app checks').click());
    assert.ok(ui.button('Check preparation status'));
    host.value = { status: 'preparing', can_prepare: false, detail: 'The original task is still preparing.' };
    await ui.flush(() => ui.button('Check preparation status').click());
    assert.equal(host.settled, false); assert.equal(ui.button('Build application').disabled, true);
    assert.equal(ui.button('Prepare app checks') === undefined, true);
    host.value = { status: 'ready', can_prepare: false, detail: 'The same task finished.' };
    await ui.pollChecker();
    assert.equal(host.settled, false); assert.equal(ui.button('Build application').disabled, false);
    assert.equal(host.posts, 1); assert.equal(ui.text(), initialDraft);
    await ui.flush(() => host.finish(new Error('Late response lost after actual readiness.')));
    assert.equal(ui.button('Build application').disabled, false);
    assert.equal(ui.dom.window.document.querySelector('.setup-checker [role="alert"]') === null, true);
    assert.equal(host.posts, 1);
  });
});

test('checker preparation response recovery keeps failed or inconclusive reads from resending preparation', async () => {
  const host = pendingCheckerPreparation();
  await mounted({ checkerRequest: host.request }, async ui => {
    await ui.flush(() => ui.button('Prepare app checks').click());
    assert.ok(ui.button('Check preparation status'));
    host.nextRead = { ok: false, status: 503, json: async () => ({ detail: 'Private transport details are not shown.' }) };
    await ui.flush(() => ui.button('Check preparation status').click());
    assert.match(ui.dom.window.document.querySelector('.setup-checker [role="alert"]').textContent, /could not be read/);
    assert.doesNotMatch(ui.dom.window.document.body.textContent, /Private transport details/);
    assert.equal(ui.button('Build application').disabled, true);
    assert.equal(ui.button('Check preparation status').disabled, false);
    await ui.flush(() => ui.button('Check preparation status').click());
    assert.equal(host.settled, false);
    const prepare = ui.button('Preparing...') || ui.button('Prepare app checks');
    assert.ok(prepare && prepare.disabled, 'An inconclusive GET cannot replay the still-owned POST.');
    await ui.flush(() => prepare.click()); assert.equal(host.posts, 1);
    await ui.flush(() => host.finish(new Error('Late original admission remains uncertain.')));
    assert.equal(ui.button('Prepare app checks').disabled, true, 'An inconclusive read does not erase original response uncertainty.');
    host.value = { status: 'ready', can_prepare: false, detail: 'Fresh explicit readback is ready.' };
    await ui.flush(() => ui.button('Check status').click());
    assert.equal(ui.button('Build application').disabled, false); assert.equal(host.posts, 1);
    assert.equal(ui.text(), initialDraft); assert.equal(ui.savedDraft(), initialDraft);
  });
});

test('checker preparation response recovery keeps visible uncertainty after failed GET and fenced successful POST', async () => {
  const host = pendingCheckerPreparation();
  await mounted({ checkerRequest: host.request }, async ui => {
    await ui.flush(() => ui.button('Prepare app checks').click());
    host.nextRead = { ok: false, status: 503, json: async () => ({ detail: 'Read failed after preparation admission.' }) };
    await ui.flush(() => ui.button('Check preparation status').click());
    const error = ui.dom.window.document.querySelector('.setup-checker [role="alert"]').textContent;
    assert.match(error, /App check status could not be read/);
    await ui.edit(latestDraft);
    const calls = ui.calls.length;
    await ui.flush(() => host.finish({ status: 'ready', can_prepare: false, detail: 'STALE POST READY' }));
    assert.equal(host.settled, true);
    assert.equal(ui.dom.window.document.querySelector('.setup-checker [role="alert"]').textContent, error,
      'The fenced POST does not erase the newer failed-read presentation.');
    assert.equal(ui.button('Build application').disabled, true);
    const prepare = ui.button('Prepare app checks');
    assert.ok(prepare && prepare.disabled, 'Visible preparation admission must reflect the retained uncertainty.');
    assert.equal(ui.button('Check status').disabled, false);
    await ui.flush(() => prepare.click());
    assert.equal(host.posts, 1); assert.equal(ui.calls.length, calls);
    assert.doesNotMatch(ui.dom.window.document.querySelector('.setup-checker').textContent, /App checks ready|STALE POST/);
    host.value = { status: 'ready', can_prepare: false, detail: 'A fresh explicit read verifies current readiness.' };
    await ui.flush(() => ui.button('Check status').click());
    assert.equal(ui.button('Build application').disabled, false);
    assert.equal(ui.dom.window.document.querySelector('.setup-checker [role="alert"]') === null, true);
    assert.equal(ui.text(), latestDraft); assert.equal(ui.savedDraft(), latestDraft);
    assert.equal(host.posts, 1);
    assert.equal(ui.calls.filter(call => call.method === 'POST' && call.target !== '/api/checker-setup').length, 0);
  });
});

test('checker preparation response recovery respects the real mutable guide selection gate before rerender', async () => {
  const gate = createGuideLoadGate(), selected = gate.begin(); gate.finish(selected);
  const selectionScope = { token: () => gate.token(), capture(guide, expectedToken) {
    const token = gate.token();
    if (guide?.id !== state.id || gate.pending() || expectedToken !== undefined && !gate.current(expectedToken)) return null;
    return () => !gate.pending() && gate.current(token);
  } };
  const host = pendingCheckerPreparation();
  await mounted({ checkerRequest: host.request, selectionScope }, async ui => {
    await ui.flush(() => ui.button('Prepare app checks').click());
    const recovery = ui.button('Check preparation status'); assert.ok(recovery);
    const requests = ui.calls.length, next = gate.begin();
    await ui.flush(() => recovery.click());
    assert.equal(ui.calls.length, requests, 'The old visible recovery button cannot read during pending guide selection.');
    assert.equal(host.settled, false); assert.equal(host.posts, 1);
    gate.finish(next); await ui.rerender();
    host.value = { status: 'ready', can_prepare: false, detail: 'Current host checker is ready.' };
    await ui.flush(() => ui.button('Check preparation status').click());
    assert.equal(ui.button('Build application').disabled, false);
    assert.equal(ui.text(), initialDraft); assert.equal(ui.savedDraft(), initialDraft);
    await ui.flush(() => host.finish(new Error('Old preparation response after navigation.')));
    assert.equal(ui.button('Build application').disabled, false); assert.equal(host.posts, 1);
    assert.equal(ui.dom.window.document.querySelector('.setup-checker [role="alert"]') === null, true);
  });
});

test('checker preparation response recovery cannot publish into a remounted checker lifetime', async () => {
  const host = pendingCheckerPreparation(), freshRead = deferred();
  await mounted({ checkerRequest: host.request }, async ui => {
    await ui.flush(() => ui.button('Prepare app checks').click());
    assert.ok(ui.button('Check preparation status'));
    host.nextRead = { ok: true, json: () => freshRead.promise };
    await ui.remount();
    assert.equal(ui.button('Build application').disabled, true);
    assert.match(ui.dom.window.document.querySelector('.setup-checker').textContent, /Reading app check status/);
    await ui.edit(latestDraft);
    await ui.flush(() => host.finish({ status: 'ready', can_prepare: false, detail: 'RETIRED CHECKER CALLBACK' }));
    assert.equal(ui.button('Build application').disabled, true, 'The old mount cannot provide readiness to the new callback.');
    assert.doesNotMatch(ui.dom.window.document.body.textContent, /RETIRED CHECKER CALLBACK|App checks ready/);
    await ui.flush(() => freshRead.resolve({ status: 'ready', can_prepare: false, detail: 'Fresh owner readback.' }));
    assert.equal(ui.button('Build application').disabled, false);
    assert.equal(ui.text(), latestDraft); assert.equal(ui.savedDraft(), latestDraft);
    assert.equal(host.posts, 1);
  });
});

test("mounted running feedback survives review, reconnect and terminal remount until an explicit next turn", async () => {
  const initial = receipt("long-build"), note = "  Keep the original result.\nExplain the new constraint before changing it.  ";
  await mounted({ initial, onPost({ target, body }) {
    assert.equal(target, endpoint);
    assert.deepEqual(body, { version: state.version, request: note.trim(), previous_build_id: initial.build_id });
    return response(receipt("next-build", { status: "queued" }));
  } }, async (ui) => {
    const textarea = ui.dom.window.document.querySelector("textarea");
    assert.equal(textarea.disabled, false);
    assert.equal(ui.dom.window.document.querySelector(`label[for="${textarea.id}"]`).textContent,
      "Note for the next implementation turn");
    assert.match(ui.dom.window.document.getElementById(textarea.getAttribute("aria-describedby")).textContent,
      /not sent to the running build/);
    await ui.edit(note);
    assert.equal(ui.savedDraft(), note);
    assert.equal(ui.button(/Resume saved build|Build application|Continue implementation/) === undefined, true);
    await ui.poll(receipt(initial.build_id, { phase: "independent_review" }));
    assert.equal(ui.text(), note);
    ui.failNextRead();
    await ui.poll(receipt(initial.build_id, { phase: "automatic_repair" }));
    assert.match(ui.dom.window.document.querySelector('[role="alert"]').textContent,
      /temporarily unavailable/);
    assert.equal(ui.text(), note);
    assert.equal(ui.savedDraft(), note);
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 0);
    await ui.poll(receipt(initial.build_id, { phase: "automatic_repair" }));
    assert.equal(ui.dom.window.document.querySelector('[role="alert"]') === null, true);
    assert.equal(ui.text(), note);
    await ui.remount();
    assert.equal(ui.text(), note);
    assert.equal(ui.savedDraft(), note);
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 0);
    await ui.poll(receipt(initial.build_id, { status: "failed", worker_stop_confirmed: true, cleanup_pending: false }));
    assert.equal(ui.text(), note);
    await ui.remount();
    assert.equal(ui.text(), note);
    assert.equal(ui.savedDraft(), note);
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 0);
    const resume = ui.button("Resume saved build");
    assert.ok(resume && !resume.disabled);
    await ui.flush(() => resume.click());
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(ui.current().build_id, "next-build");
    assert.equal(ui.savedDraft(), null, "Only the explicit accepted next turn consumes the unchanged note");
  });
});

test("an unavailable first build read offers status recovery rather than a new build, preserving the note", async () => {
  const saved = receipt("saved-before-reopen", { status: "failed", worker_stop_confirmed: true });
  await mounted({ initial: saved, initialStatusResponse: { ok: false, status: 503,
    json: async () => ({ detail: "Build status is temporarily unavailable." }) } }, async ui => {
    assert.match(ui.dom.window.document.querySelector("h2").textContent, /Build status is unavailable/);
    assert.equal(ui.button("Build application") === undefined, true);
    const refresh = ui.button("Refresh build status");
    assert.ok(refresh && !refresh.disabled);
    assert.ok(refresh.classList.contains("primary"));
    assert.equal(ui.dom.window.document.querySelectorAll(".implementation-actions button.primary").length, 1);
    await ui.edit(latestDraft);
    await ui.flush(() => refresh.click());
    assert.equal(ui.current().build_id, saved.build_id);
    assert.ok(ui.button("Resume saved build") && !ui.button("Resume saved build").disabled);
    assert.equal(ui.button("Resume saved build").classList.contains("primary"), true);
    assert.equal(ui.text(), latestDraft);
    assert.equal(ui.savedDraft(), latestDraft);
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
  });
});

test("the first pending status read cannot start a build; a checked absence enables the explicit action", async () => {
  const pending = deferred();
  await mounted({ initialStatusResponse: pending.promise }, async ui => {
    assert.equal(ui.button("Build application") === undefined, true);
    const checking = ui.button("Checking build status...");
    assert.ok(checking && checking.disabled);
    await ui.flush(() => checking.click());
    await ui.edit(latestDraft);
    await ui.flush(() => pending.resolve({ ok: false, status: 404,
      json: async () => ({ detail: "No saved build." }) }));
    assert.ok(ui.button("Build application") && !ui.button("Build application").disabled);
    assert.equal(ui.button("Build application").classList.contains("primary"), true);
    assert.equal(ui.text(), latestDraft);
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
  });
});

test("a failed later build read preserves saved work and blocks a new admission until recovery", async () => {
  const saved = receipt("saved-before-read-failure", { status: "failed", worker_stop_confirmed: true });
  await mounted({ initial: saved }, async ui => {
    assert.ok(ui.button("Resume saved build") && !ui.button("Resume saved build").disabled);
    ui.failNextRead();
    await ui.poll(saved);
    assert.equal(ui.current().build_id, saved.build_id);
    const retainedResume = ui.button("Resume saved build");
    assert.ok(retainedResume && retainedResume.disabled);
    assert.equal(retainedResume.classList.contains("primary"), true);
    await ui.flush(() => retainedResume.click());
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
    assert.equal(ui.button("Rebuild with updated runtime").disabled, true);
    assert.match(ui.dom.window.document.querySelector(".implementation-next").textContent, /Refresh build status/);
    await ui.flush(() => ui.button("Refresh build status").click());
    assert.ok(ui.button("Resume saved build") && !ui.button("Resume saved build").disabled);
    assert.equal(ui.text(), initialDraft);
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
  });
});

test("a delayed failed first read cannot replace the new selected guide's checked absence", async () => {
  const pending = deferred();
  await mounted({ initialStatusResponse: pending.promise }, async ui => {
    await ui.navigate(otherState);
    assert.ok(ui.button("Build application") && !ui.button("Build application").disabled);
    await ui.edit(otherDraft);
    await ui.flush(() => pending.resolve({ ok: false, status: 503,
      json: async () => ({ detail: "Old guide is unavailable." }) }));
    assert.ok(ui.button("Build application") && !ui.button("Build application").disabled);
    assert.equal(ui.button("Refresh build status") === undefined, true);
    assert.equal(ui.text(), otherDraft);
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
  });
});

test("mounted runtime refresh accepts a blank draft once and binds the selected checked build", async () => {
  const pending = deferred(), initial = acceptedRuntimeBuild();
  await mounted({ initial, draft: "", onPost({ target, body }) {
    assert.equal(target, endpoint);
    assert.equal(body.version, state.version);
    assert.equal(body.previous_build_id, initial.build_id);
    assert.equal(body.refresh_framework, true);
    assert.match(body.request, /Preserve its requested behavior/);
    assert.match(body.request, /Refresh the runtime in build runtime-A\./);
    return pending.promise;
  } }, async (ui) => {
    const ordinary = ui.button("Continue implementation");
    assert.ok(ordinary && ordinary.disabled, "An accepted build still needs feedback for an ordinary continuation");
    const update = ui.button("Rebuild with updated runtime");
    assert.ok(update && !update.disabled);
    await ui.flush(() => { update.click(); update.click(); });
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
    await ui.edit(latestDraft);
    await ui.flush(() => pending.resolve(response(receipt("runtime-B", { status: "queued" }))));
    assert.equal(ui.current().build_id, "runtime-B");
    assert.equal(ui.savedDraft(), latestDraft);
  });
});

for (const status of ["failed", "interrupted"]) test(`mounted ${status} host refresh binds the same current build after confirmed cleanup`, async () => {
  const pending = deferred(), initial = receipt(`host-gap-${status}`, { status,
    worker_stop_confirmed: true, cleanup_pending: false, cleanup_in_progress: false,
    detail: "The generated application host needs to be refreshed before this application can pass review." });
  await mounted({ initial, draft: "", onPost({ target, body }) {
    assert.equal(target, endpoint);
    assert.equal(body.version, state.version);
    assert.equal(body.previous_build_id, initial.build_id);
    assert.equal(body.refresh_framework, true);
    assert.match(body.request, /Preserve its requested behavior, agent design and authored application code/);
    assert.ok(body.request.endsWith(`Refresh the runtime in build ${initial.build_id}.`));
    return pending.promise;
  } }, async (ui) => {
    assert.ok(ui.dom.window.document.body.textContent.includes(initial.detail));
    assert.match(ui.dom.window.document.querySelector(".implementation-runtime-update").textContent,
      /runtime and generated application host/);
    const update = ui.button("Rebuild with updated runtime");
    assert.ok(update && !update.disabled);
    await ui.flush(() => { update.click(); update.click(); });
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
    await ui.edit(latestDraft);
    await ui.flush(() => pending.resolve(response(receipt(`refreshed-${status}`, { status: "queued" }))));
    assert.equal(ui.current().build_id, `refreshed-${status}`);
    assert.equal(ui.text(), latestDraft);
    assert.equal(ui.savedDraft(), latestDraft);
  });
});

const candidateDigest = "c".repeat(64);
const acceptedCandidateBuild = (changes = {}) => {
  const accepted = acceptedRuntimeBuild();
  return { ...accepted, source_snapshot_verified: true, source_digest: candidateDigest,
    acceptance: { ...accepted.acceptance, source_digest: candidateDigest },
    framework: { runtime_candidate: { format: "forge-meta-toolkit-runtime-selection-v1" } }, ...changes };
};

for (const current of [true, false]) for (const [sourceVerified, snapshotVerified] of [[true, true], [false, true], [false, false], [false, undefined]]) {
  test(`fresh build binds the incomplete source once and preserves the unsent note: current=${current}, publication=${sourceVerified}, check=${snapshotVerified}`, async () => {
    const initial = acceptedCandidateBuild({ status: sourceVerified ? "interrupted" : "failed", ready_to_package: false,
      source_verified: sourceVerified, source_snapshot_verified: snapshotVerified,
      current, current_guide_version: current ? state.version : null,
      guide_version: current ? state.version : state.version - 1 });
    const bodies = [];
    await mounted({ initial, onPost({ body }) {
      bodies.push(body);
      return response(receipt("fresh-B", { status: "queued" }));
    } }, async ui => {
      const fresh = ui.button("Start a fresh build");
      assert.ok(fresh && !fresh.disabled);
      if (!current) assert.equal(ui.button("Rebuild with updated runtime") === undefined, true);
      await ui.flush(() => { fresh.click(); fresh.click(); });
      assert.equal(ui.current().build_id, "fresh-B");
      assert.equal(ui.text(), initialDraft);
      assert.equal(ui.savedDraft(), initialDraft);
    });
    assert.deepEqual(bodies, [{ version: state.version, request: "", previous_build_id: initial.build_id,
      start_fresh: true, previous_source_digest: candidateDigest }]);
  });
}

for (const changes of [{ source_digest: undefined }, { source_digest: "invalid" }, { worker_stop_confirmed: false },
  { cleanup_pending: true }, { cleanup_in_progress: true }, { cleanup_custody_lost: true },
  { source_digest: "" }, { status: "completed" }, { guide_id: otherState.id }]) {
  test(`fresh build is absent for unavailable source or cleanup: ${JSON.stringify(changes)}`, async () => {
    await mounted({ initial: acceptedCandidateBuild({ status: "interrupted", ready_to_package: false,
      current: false, current_guide_version: null, guide_version: state.version - 1, ...changes }) }, async ui => {
      assert.equal(ui.button("Start a fresh build") === undefined, true);
      assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
    });
  });
}

test("mounted candidate runtime refresh sends the host-owned preserve-design request and keeps the unsent draft", async () => {
  const initial = acceptedCandidateBuild(), bodies = [];
  await mounted({ initial, onPost({ target, body }) {
    assert.equal(target, endpoint);
    bodies.push(body);
    return response(receipt("runtime-B", { status: "queued" }));
  } }, async (ui) => {
    const update = ui.button("Rebuild with updated runtime");
    assert.ok(update && !update.disabled);
    await ui.flush(() => { update.click(); update.click(); });
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(ui.current().build_id, "runtime-B");
    assert.equal(ui.text(), initialDraft, "The builder note was not sent, so it stays on the page");
    assert.equal(ui.savedDraft(), initialDraft);
  });
  assert.deepEqual(bodies, [{ version: state.version, request: "", previous_build_id: initial.build_id,
    refresh_framework: true, preserve_design: true }]);
});

for (const [defect, changes] of [
  ["failed", { status: "failed", ready_to_package: false }],
  ["interrupted", { status: "interrupted", ready_to_package: false }],
  ["untrusted acceptance", { acceptance_trusted: false }],
  ["stale acceptance", { acceptance_current: false }],
  ["unverified source", { source_verified: false }],
  ["not ready to package", { ready_to_package: false }],
]) test(`mounted candidate runtime refresh is offered only for the exact accepted build: ${defect}`, async () => {
  await mounted({ initial: acceptedCandidateBuild(changes), draft: "" }, async (ui) => {
    const update = ui.button("Rebuild with updated runtime");
    assert.ok(update && update.disabled);
    assert.match(ui.dom.window.document.querySelector(".implementation-runtime-update").textContent,
      /starts from this application's independently accepted build/);
    await ui.flush(() => update.click());
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 0);
  });
});

test("mounted rejected runtime refresh preserves the exact feedback draft", async () => {
  await mounted({ initial: acceptedRuntimeBuild(), onPost({ body }) {
    assert.equal(body.refresh_framework, true);
    assert.ok(body.request.startsWith(initialDraft.trim()));
    return { ok: false, status: 409, json: async () => ({ detail: "The selected build changed." }) };
  } }, async (ui) => {
    await ui.flush(() => ui.button("Rebuild with updated runtime").click());
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(ui.text(), initialDraft);
    assert.equal(ui.savedDraft(), initialDraft);
    assert.match(ui.dom.window.document.querySelector('[role="alert"]').textContent, /selected build changed/);
  });
});

test("mounted successive blank runtime refreshes target different completed builds", async () => {
  const initial = acceptedRuntimeBuild(), bodies = [];
  await mounted({ initial, draft: "", onPost({ body }) {
    bodies.push(body);
    return response({ ...initial, build_id: bodies.length === 1 ? "runtime-B" : "runtime-C" });
  } }, async (ui) => {
    await ui.flush(() => ui.button("Rebuild with updated runtime").click());
    assert.equal(ui.current().build_id, "runtime-B");
    assert.equal(ui.text(), "");
    await ui.flush(() => ui.button("Rebuild with updated runtime").click());
    assert.equal(ui.current().build_id, "runtime-C");
    assert.equal(bodies.length, 2);
    assert.equal(bodies[0].previous_build_id, "runtime-A");
    assert.equal(bodies[1].previous_build_id, "runtime-B");
    assert.ok(bodies.every((body) => body.refresh_framework === true));
    assert.notEqual(bodies[0].request, bodies[1].request, "An explicit later update must not deduplicate into the earlier completed update");
    assert.equal(ui.savedDraft(), null);
  });
});

test("mounted delayed runtime refresh cannot replace a guide after navigation", async () => {
  const pending = deferred();
  await mounted({ initial: acceptedRuntimeBuild(), onPost: () => pending.promise }, async (ui) => {
    await ui.flush(() => ui.button("Rebuild with updated runtime").click());
    await ui.navigate(otherState);
    await ui.edit(otherDraft);
    await ui.flush(() => pending.resolve(response(receipt("late-runtime", { status: "queued" }))));
    assert.equal(ui.current(), null);
    assert.equal(ui.savedDraft(), initialDraft);
    assert.equal(ui.savedDraft(otherState.id), otherDraft);
    assert.equal(ui.text(), otherDraft);
  });
});

for (const changes of [{ current: false }, { guide_id: otherState.id }, { guide_version: state.version - 1 },
  { build_id: "" }, { status: "running" }, { status: "failed", cleanup_pending: true },
  { status: "interrupted", cleanup_pending: true }, { status: "interrupted", worker_stop_confirmed: false },
  { status: "interrupted", worker_stop_confirmed: undefined }, { status: "failed", cleanup_in_progress: true },
  { status: "failed", cleanup_custody_lost: true }]) {
  test(`mounted runtime refresh is absent for an unavailable checked build ${JSON.stringify(changes)}`, async () => {
    await mounted({ initial: { ...acceptedRuntimeBuild(), ...changes }, onPost: () => assert.fail("Unavailable runtime refresh") }, async (ui) => {
      assert.equal(ui.button("Rebuild with updated runtime") === undefined, true);
      assert.equal(ui.calls.filter((call) => call.method === "POST").length, 0);
    });
  });
}

test("mounted Stop sends the exact selected version/build once on double activation and preserves feedback", async () => {
  const pending = deferred(), initial = receipt("stop-A");
  await mounted({ initial, onPost({ target, body }) {
    assert.equal(target, `${endpoint}/stop`);
    assert.deepEqual(body, { version: state.version, build_id: initial.build_id });
    return pending.promise;
  } }, async (ui) => {
    const stop = ui.button("Stop build");
    assert.ok(stop && !stop.disabled);
    await ui.flush(() => { stop.click(); stop.click(); });
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
    assert.ok(ui.button(/Stopping/).disabled);
    assert.equal(ui.button(/Resume saved build|Build application|Continue implementation/) === undefined, true);
    assert.equal(ui.savedDraft(), initialDraft);
    await ui.flush(() => pending.resolve(response(receipt(initial.build_id,
      { status: "interrupted", worker_stop_confirmed: true, cleanup_pending: false }))));
    assert.equal(ui.current().build_id, initial.build_id);
    assert.equal(ui.text(), initialDraft);
    assert.equal(ui.savedDraft(), initialDraft);
    assert.ok(ui.button("Resume saved build") && !ui.button("Resume saved build").disabled);
  });
});

for (const status of ["interrupted", "completed"]) test(`mounted terminal ${status} cleanup blocks new actions and offers Retry cleanup`, async () => {
  const initial = receipt(`cleanup-${status}`, { status, cleanup_pending: true, cleanup_in_progress: false,
    stop_requested: true, worker_stop_confirmed: false, ready_to_package: true,
    source_verified: true, source_digest: "synthetic-checked-source", acceptance_current: true, acceptance_trusted: true,
    acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "synthetic-checked-source", status: "pass", tests_run: 2 } });
  await mounted({ initial, onPost({ target, body }) {
    assert.equal(target, `${endpoint}/stop`);
    assert.deepEqual(body, { version: state.version, build_id: initial.build_id });
    return response(receipt(initial.build_id, { status: "interrupted", worker_stop_confirmed: true,
      cleanup_pending: false, cleanup_in_progress: false }));
  } }, async (ui) => {
    const retry = ui.button("Retry cleanup");
    assert.ok(retry && !retry.disabled, "A terminal pending cleanup needs a usable retry, not a permanently disabled Stop");
    assert.equal(ui.button(/Resume saved build|Build application|Continue implementation|Review and improve|Download checked|Try saved|Install saved/) === undefined, true);
    assert.equal([...ui.dom.window.document.querySelectorAll("a")].some((item) => item.textContent.includes("Open installed console")), false);
    assert.equal(ui.savedDraft(), initialDraft);
    await ui.flush(() => retry.click());
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(ui.current().worker_stop_confirmed, true);
    assert.equal(ui.text(), initialDraft);
    assert.ok(ui.button("Resume saved build") && !ui.button("Resume saved build").disabled);
  });
});

test("mounted admitted cleanup in progress prevents duplicate cleanup and Resume", async () => {
  await mounted({ initial: receipt("closing-A", { status: "interrupted", cleanup_pending: true,
    cleanup_in_progress: true, stop_requested: true }), onPost: () => assert.fail("Closing cleanup cannot be re-admitted") }, async (ui) => {
    assert.equal(ui.button("Retry cleanup") === undefined, true);
    assert.ok(ui.button(/Stopping|Closing|Cleaning/).disabled);
    assert.equal(ui.button(/Resume saved build|Build application|Continue implementation/) === undefined, true);
    assert.equal(ui.savedDraft(), initialDraft);
  });
});

test("mounted delayed Stop response cannot replace a newer selected build", async () => {
  const pending = deferred(), newer = receipt("stop-B");
  await mounted({ initial: receipt("stop-A"), onPost: () => pending.promise }, async (ui) => {
    await ui.flush(() => ui.button("Stop build").click());
    await ui.admit(newer);
    assert.equal(ui.current().build_id, newer.build_id);
    const releaseStatus = ui.holdStatusReads();
    const sinceNewer = ui.reported.length;
    await ui.flush(() => pending.resolve(response(receipt("stop-A", { status: "interrupted", worker_stop_confirmed: true }))));
    assert.equal(ui.current().build_id, newer.build_id);
    assert.ok(ui.reported.slice(sinceNewer).every((value) => !value || value.build_id === newer.build_id));
    assert.ok(ui.button("Stop build") && !ui.button("Stop build").disabled);
    assert.equal(ui.savedDraft(), initialDraft);
    await ui.flush(releaseStatus);
  });
});

for (const via of ["admission", "poll"]) test(`mounted delayed Resume admission preserves a newer build from ${via} and the latest draft`, async () => {
  const pending = deferred(), saved = receipt("saved-A", { status: "interrupted", worker_stop_confirmed: true });
  const newer = receipt("newer-B");
  await mounted({ initial: saved, onPost({ target, body }) {
    assert.equal(target, endpoint);
    assert.deepEqual(body, { version: state.version, request: initialDraft.trim(), previous_build_id: saved.build_id });
    return pending.promise;
  } }, async (ui) => {
    await ui.flush(() => ui.button("Resume saved build").click());
    await ui.edit(latestDraft);
    if (via === "admission") await ui.admit(newer);
    else await ui.poll(newer);
    assert.equal(ui.current().build_id, newer.build_id);
    const releaseStatus = ui.holdStatusReads();
    const sinceNewer = ui.reported.length;
    await ui.flush(() => pending.resolve(response(receipt("resumed-A", { status: "queued" }))));
    assert.equal(ui.current().build_id, newer.build_id);
    assert.ok(ui.reported.slice(sinceNewer).every((value) => !value || value.build_id === newer.build_id));
    assert.equal(ui.savedDraft(), latestDraft);
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
    await ui.flush(releaseStatus);
    await ui.poll(receipt(newer.build_id, { status: "interrupted", worker_stop_confirmed: true }));
    assert.equal(ui.text(), latestDraft);
  });
});

test("mounted initial Build explicitly binds no previously selected build", async () => {
  await mounted({ onPost({ target, body }) {
    assert.equal(target, endpoint);
    assert.deepEqual(body, { version: state.version, request: initialDraft.trim(), previous_build_id: null });
    return response(receipt("first-build", { status: "queued" }));
  } }, async (ui) => {
    await ui.flush(() => ui.button("Build application").click());
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(ui.current().build_id, "first-build");
    assert.equal(ui.savedDraft(), null, "Only an accepted unchanged draft is cleared");
  });
});

for (const outcome of ["admission", "error"]) test(`saved-build response recovery preserves writing and fences late ${outcome}`, async () => {
  const pendingBody = deferred(), saved = receipt("recovery-before", { status: "failed", worker_stop_confirmed: true });
  const newer = { ...acceptedRuntimeBuild(), build_id: `recovery-after-${outcome}` };
  const oldError = "The detached original response failed.";
  let decodingBody = false;
  await mounted({ initial: saved, onPost({ target, body }) {
    assert.equal(target, endpoint);
    assert.deepEqual(body, { version: state.version, request: initialDraft.trim(), previous_build_id: saved.build_id });
    return { ok: outcome === "admission", status: outcome === "admission" ? 202 : 409,
      json: async () => { decodingBody = true; return pendingBody.promise; } };
  } }, async ui => {
    await ui.flush(() => ui.button("Resume saved build").click());
    assert.equal(decodingBody, true, "Actual request decoding must have entered the still-pending response body.");
    await ui.poll(newer);
    await ui.rerender();
    await ui.edit(latestDraft);
    const show = ui.button("Show saved build");
    assert.ok(show && !show.disabled, "A known current newer terminal build offers explicit recovery from the pending response.");
    assert.equal(ui.button("Try saved application") === undefined, true, "The original pending presentation still gates follow-on navigation.");
    await ui.flush(() => show.click());
    assert.equal(ui.current().build_id, newer.build_id);
    assert.equal(ui.text(), latestDraft);
    assert.equal(ui.savedDraft(), latestDraft);
    assert.equal(ui.button("Show saved build") === undefined, true);
    assert.equal(ui.button("Starting builder...") === undefined, true);
    assert.ok(ui.button("Try saved application") && !ui.button("Try saved application").disabled);
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 1, "Showing a saved build sends no retry, stop or new build.");
    const finalNote = latestDraft + "\n  Edited after showing the saved build.  ";
    await ui.edit(finalNote);
    const callbacks = ui.reported.length, requests = ui.calls.length;
    await ui.flush(() => pendingBody.resolve(outcome === "admission"
      ? receipt("detached-old-build", { status: "queued" }) : { detail: oldError }));
    assert.equal(ui.current().build_id, newer.build_id);
    assert.equal(ui.text(), finalNote);
    assert.equal(ui.savedDraft(), finalNote);
    assert.equal(ui.reported.length, callbacks, "The detached original result invokes no parent receipt callback.");
    assert.equal(ui.calls.length, requests, "A detached result must not refresh or submit any request.");
    assert.equal(ui.dom.window.document.querySelector('[role="alert"]') === null, true);
    assert.equal(ui.dom.window.document.body.textContent.includes(oldError), false);
    assert.ok(ui.button("Try saved application") && !ui.button("Try saved application").disabled);
  });
});

test("saved-build response recovery excludes active, predecessor, stale, unconfirmed and uncleaned receipts", async () => {
  const saved = receipt("recovery-predecessor", { status: "failed", worker_stop_confirmed: true });
  const terminal = { ...acceptedRuntimeBuild(), build_id: "recovery-selected" };
  const variants = [
    { name: "queued", value: { ...terminal, status: "queued" } },
    { name: "running", value: { ...terminal, status: "running" } },
    { name: "checking", value: { ...terminal, status: "checking" } },
    { name: "predecessor", value: saved },
    { name: "stale source", value: { ...terminal, current: false } },
    { name: "another guide", value: { ...terminal, guide_id: otherState.id } },
    { name: "earlier version", value: { ...terminal, guide_version: state.version - 1 } },
    { name: "pending cleanup", value: { ...terminal, cleanup_pending: true } },
    { name: "entered cleanup", value: { ...terminal, cleanup_in_progress: true } },
    { name: "lost cleanup owner", value: { ...terminal, cleanup_custody_lost: true } },
    { name: "unconfirmed stop", value: { ...terminal, worker_stop_confirmed: false } },
    { name: "unconfirmed read", value: terminal, readFailure: true },
  ];
  for (const variant of variants) {
    const pendingBody = deferred();
    await mounted({ initial: saved, onPost: () => ({ ok: true, json: () => pendingBody.promise }) }, async ui => {
      await ui.flush(() => ui.button("Resume saved build").click());
      if (variant.readFailure) ui.failNextRead();
      await ui.poll(variant.value);
      assert.equal(ui.button("Show saved build") === undefined, true, `${variant.name} must not release an original pending admission.`);
      assert.equal(ui.text(), initialDraft);
      assert.equal(ui.savedDraft(), initialDraft);
      assert.equal(ui.calls.filter(call => call.method === "POST").length, 1);
      await ui.flush(() => pendingBody.resolve(receipt("finished-old-response", { status: "queued" })));
    });
  }
});

test("saved-build response recovery cannot transfer to another guide or a revised guide lifetime", async () => {
  for (const next of [otherState, { ...state, version: state.version + 1 }]) {
    const pendingBody = deferred(), saved = receipt("drift-before", { status: "failed", worker_stop_confirmed: true });
    const newer = { ...acceptedRuntimeBuild(), build_id: "drift-after" };
    await mounted({ initial: saved, onPost: () => ({ ok: true, json: () => pendingBody.promise }) }, async ui => {
      await ui.flush(() => ui.button("Resume saved build").click());
      await ui.poll(newer);
      assert.ok(ui.button("Show saved build"));
      await ui.navigate(next);
      await ui.edit(otherDraft);
      assert.equal(ui.button("Show saved build") === undefined, true);
      await ui.flush(() => pendingBody.resolve(receipt("drift-old-response", { status: "queued" })));
      assert.equal(ui.text(), otherDraft);
      assert.equal(ui.savedDraft(next.id), otherDraft);
      assert.equal(ui.calls.filter(call => call.method === "POST").length, 1);
      assert.equal(ui.dom.window.document.querySelector('[role="alert"]') === null, true);
    });
  }
});

test("saved-build response recovery checks the original selection owner even before rerender", async () => {
  const gate = createGuideLoadGate(), selected = gate.begin(); gate.finish(selected);
  const selectionScope = {
    token: () => gate.token(),
    capture(guide, expectedToken) {
      const token = gate.token();
      if (guide?.id !== state.id || gate.pending() || expectedToken !== undefined && !gate.current(expectedToken)) return null;
      return () => !gate.pending() && gate.current(token);
    },
  };
  const pendingBody = deferred(), saved = receipt("selection-before", { status: "failed", worker_stop_confirmed: true });
  const newer = { ...acceptedRuntimeBuild(), build_id: "selection-after" };
  await mounted({ initial: saved, selectionScope,
    onPost: () => ({ ok: true, json: () => pendingBody.promise }) }, async ui => {
    await ui.flush(() => ui.button("Resume saved build").click());
    await ui.poll(newer);
    const staleShow = ui.button("Show saved build");
    assert.ok(staleShow && !staleShow.disabled);
    const nextSelection = gate.begin();
    await ui.flush(() => staleShow.click());
    assert.ok(ui.button("Starting builder..."), "A mutable pending selection gate must refuse the old visible recovery action.");
    assert.equal(ui.text(), initialDraft);
    gate.finish(nextSelection);
    await ui.rerender();
    assert.equal(ui.button("Show saved build") === undefined, true, "A later selection token cannot acquire the original response owner.");
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 1);
    await ui.flush(() => pendingBody.resolve(receipt("selection-old-response", { status: "queued" })));
    assert.equal(ui.current().build_id, newer.build_id);
    assert.equal(ui.text(), initialDraft);
  });
});

for (const outcome of ["admission", "error"]) test(`mounted delayed Build ${outcome} cannot survive leaving and returning to the same guide/version`, async () => {
  const pending = deferred(), newer = receipt(`navigation-B-${outcome}`);
  const oldError = "The old navigation admission failed.";
  await mounted({ onPost({ target, body }) {
    assert.equal(target, endpoint);
    assert.deepEqual(body, { version: state.version, request: initialDraft.trim(), previous_build_id: null });
    return pending.promise;
  } }, async (ui) => {
    await ui.flush(() => ui.button("Build application").click());
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
    await ui.navigate(otherState);
    await ui.edit(otherDraft);
    assert.equal(ui.savedDraft(otherState.id), otherDraft);
    assert.equal(ui.savedDraft(), initialDraft);

    // B is selected by the server but has not reached the returning panel.
    // Holding that GET prevents a poll from hiding an erroneous adoption of A.
    ui.selectOnServer(newer);
    const releaseStatus = ui.holdStatusReads();
    const beforeReturn = ui.calls.filter((call) => call.method === "GET" && call.target === endpoint).length;
    await ui.navigate(state);
    assert.equal(ui.current(), null);
    assert.equal(ui.text(), initialDraft);
    await ui.edit(latestDraft);
    const returnedReads = ui.calls.filter((call) => call.method === "GET" && call.target === endpoint).length;
    assert.ok(returnedReads > beforeReturn, "Returning must request the current server selection");
    const sinceReturn = ui.reported.length;
    await ui.flush(() => pending.resolve(outcome === "admission"
      ? response(receipt("navigation-A", { status: "queued" }))
      : { ok: false, status: 409, json: async () => ({ detail: oldError }) }));

    assert.equal(ui.current(), null, "A reply from the old navigation lifetime must not adopt a build");
    assert.ok(ui.reported.slice(sinceReturn).every((value) => !value || value.build_id === newer.build_id));
    assert.equal(ui.text(), latestDraft);
    assert.equal(ui.savedDraft(), latestDraft);
    assert.equal(ui.savedDraft(otherState.id), otherDraft);
    assert.equal(ui.dom.window.document.querySelector('[role="alert"]') === null, true,
      "The old action's error must not appear in the returned guide");
    assert.equal(ui.dom.window.document.body.textContent.includes(oldError), false);

    await ui.flush(releaseStatus);
    await ui.poll(newer);
    assert.equal(ui.current().build_id, newer.build_id);
    assert.ok(ui.calls.filter((call) => call.method === "GET" && call.target === endpoint).length > returnedReads,
      "A fresh status poll must establish the current build after the ignored action");
    assert.ok(ui.reported.slice(sinceReturn).every((value) => !value || value.build_id === newer.build_id));
    assert.equal(ui.savedDraft(), latestDraft);
    assert.equal(ui.savedDraft(otherState.id), otherDraft);
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("mounted cleanup custody loss blocks retry and new actions while retaining source diagnostics", async () => {
  const initial = receipt("custody-lost", { status: "completed", cleanup_pending: true,
    cleanup_in_progress: false, cleanup_custody_lost: true, worker_stop_confirmed: false,
    cleanup_detail: "The saved worker cannot be owned by this service. Confirm its shutdown separately.",
    ready_to_package: true, source_verified: true, source_digest: "synthetic-checked-source",
    acceptance_current: true, acceptance_trusted: true,
    acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "synthetic-checked-source",
      status: "pass", tests_run: 2 },
    files: [{ path: "project/domain.py", authored: true, size: 19 }],
    detail: "Saved source review completed before custody was lost." });
  await mounted({ initial, onPost: () => assert.fail("Lost custody cannot authorize a build or cleanup") }, async (ui) => {
    const unavailable = ui.button("Cleanup unavailable");
    assert.ok(unavailable && unavailable.disabled);
    assert.equal(ui.button("Retry cleanup") === undefined, true);
    assert.equal(ui.button(/Resume saved build|Build application|Continue implementation|Review and improve|Download checked|Try saved|Install saved/) === undefined, true);
    assert.equal([...ui.dom.window.document.querySelectorAll("a")].some((item) =>
      item.textContent.includes("Open installed console")), false);
    assert.ok(ui.button("Inspect 1 source files"), "Lost cleanup custody must retain inspection of saved source");
    const details = ui.dom.window.document.body.textContent;
    assert.ok(details.includes(initial.summary));
    assert.ok(details.includes(initial.cleanup_detail));
    assert.ok(details.includes(initial.detail));
    assert.equal(details.includes("Continue implementation to resolve its checks"), false);
    assert.equal(ui.savedDraft(), initialDraft);
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 0);
  });
});
