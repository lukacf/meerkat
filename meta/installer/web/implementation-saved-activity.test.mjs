import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { renderToStaticMarkup } from "react-dom/server";

async function load(path, mounted = false) {
  const require = createRequire(import.meta.url);
  const built = await build({ entryPoints: [fileURLToPath(new URL(path, import.meta.url))], bundle: true,
    write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
    nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
    alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
    plugins: mounted ? [{ name: "same-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
        path: pathToFileURL(require.resolve(args.path)).href, external: true,
      }));
    } }] : [] });
  return import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
}
const { implementationSavedActivity, implementationCanDownload } = await load("implementation-panel.mjs");
const { ImplementationSavedActivity } = await load("implementation-panel.jsx", true);
const guide = { id: "activity-guide", version: 4 };
const stamp = (second) => `2026-10-03T10:00:${String(second).padStart(2, "0")}Z`;
const event = (message, second) => ({ message, at: stamp(second) });
const receipt = (change = {}) => ({ guide_id: guide.id, guide_version: guide.version,
  build_id: "owned-build", current: true, status: "running", phase: "implementation", ...change });
const recovery = { at: stamp(5), reason: "completed_without_current_publication", retry_requested: true,
  builder_phase: "initial_build", source_digest: "a".repeat(64),
  interaction_id: "private-interaction-canary", native_completion: { cursor: "private-cursor-canary" } };

test("latest saved source and check summaries omit file paths and every raw extra field", () => {
  const saved = receipt({ progress: [event("Writing application/private-path-canary.py", 1),
    event("Editing application/last-path-canary.py", 2),
    { ...event("Application checks passed.", 3), arguments: { question: "private-question-canary" }, detail: "private-detail-canary" }],
    request: "private-request-canary", files: [{ path: "private-file-canary" }], detail: "private-receipt-canary" });
  const original = structuredClone(saved), view = implementationSavedActivity(saved, guide);
  assert.equal(view.label, "Latest saved build activity");
  assert.deepEqual(view.rows.map((row) => [row.kind, row.at]), [["source", stamp(2)], ["check", stamp(3)]]);
  assert.match(view.rows[1].message, /Last source check passed/);
  assert.match(view.rows[1].message, /Independent review and installation are separate/);
  assert.doesNotMatch(JSON.stringify(view), /private-|path-canary|arguments|question|detail/);
  assert.deepEqual(saved, original);
  assert.equal(implementationCanDownload(saved, guide), false);
});

test("a later source edit makes retained passing checks explicitly out of date", () => {
  const view = implementationSavedActivity(receipt({ progress: [event("Application checks passed.", 1),
    event("Editing application/changed.py", 2)], receipt: { status: "pass" }, source_verified: false }), guide);
  assert.match(view.rows.find((row) => row.kind === "check").message, /out of date after the saved change/);
  assert.doesNotMatch(JSON.stringify(view), /Last source check passed/);
});

test("the exact bounded host batch event is a saved source change and retires earlier checks", () => {
  for (const count of [1, 9, 10, 19, 20, 29, 30, 31, 32]) {
    const saved = receipt({ progress: [event("Application checks passed.", 1),
      { ...event(`Updated ${count} source files in one checked batch.`, 2),
        args: { path: "private-batch-path", content: "private-batch-content" }, detail: "private-batch-detail" }],
      request: "private-batch-request", source_verified: false, ready_to_package: false });
    const before = structuredClone(saved), view = implementationSavedActivity(saved, guide);
    assert.deepEqual(view.rows.find((row) => row.kind === "source"),
      { kind: "source", at: stamp(2), message: "Application source change saved." });
    assert.match(view.rows.find((row) => row.kind === "check").message, /out of date after the saved change/);
    assert.doesNotMatch(JSON.stringify(view), /private-|args|path|content|Updated|Last source check passed/);
    assert.equal(implementationCanDownload(saved, guide), false);
    assert.deepEqual(saved, before);
  }
});

test("batch-like arbitrary text and out-of-bound counts cannot fabricate a saved source event", () => {
  const messages = ["0", "33", "999999999999999999999", "01", "1.0", "+1", "-1", "1e0", "one"]
    .map((count) => `Updated ${count} source files in one checked batch.`);
  messages.push("Updated 2 source files in one checked batch. private-extra", "Updated 2 source files in one checked batch.\n",
    "private-prefix Updated 2 source files in one checked batch.", "Updated 2 source files in one checked batch",
    "Updated 2 source files in one checked batch.\r", "updated 2 source files in one checked batch.");
  for (const message of messages)
    assert.equal(implementationSavedActivity(receipt({ progress: [event(message, 1)] }), guide), null);
  assert.equal(implementationSavedActivity(receipt({ progress: [{ ...event("Unrecognized host event", 1),
    detail: "Updated 2 source files in one checked batch.", args: { message: "Updated 2 source files in one checked batch." } }],
    request: "Updated 2 source files in one checked batch." }), guide), null);
});

test("a recorded failed check remains attention rather than package or acceptance success", () => {
  const saved = receipt({ progress: [event("Running isolated syntax and application tests.", 1),
    event("Application checks need attention.", 2)], status: "failed", source_verified: false, ready_to_package: false });
  assert.match(implementationSavedActivity(saved, guide).rows[0].message, /found work to do/);
  assert.equal(implementationCanDownload(saved, guide), false);
});

for (const change of [{ status: "failed" }, { status: "interrupted" }, { stop_requested: true },
  { cleanup_in_progress: true }, { status: "completed" }, { status: "running", phase: "implementation" }]) test(`an ended or stopping check is not still presented as running: ${JSON.stringify(change)}`, () => {
  const view = implementationSavedActivity(receipt({ ...change,
    progress: [event("Running isolated syntax and application tests.", 1)] }), guide);
  assert.match(view.rows[0].message, /result was not recorded/);
  assert.doesNotMatch(view.rows[0].message, /Checking the saved/);
});

test("exact host recovery records disclose one requested turn without inferring its success", () => {
  for (const status of ["running", "failed", "completed"]) {
    const view = implementationSavedActivity(receipt({ status, builder_publication_retries: 1,
      builder_publication_failures: [recovery], ready_to_package: false }), guide);
    assert.deepEqual(view.rows, [{ kind: "recovery", at: stamp(5),
      message: "One additional builder turn was requested to finish checked source publication." }]);
    assert.doesNotMatch(JSON.stringify(view), /private|passed|accepted|ready|complete/);
  }
  for (const change of [{ builder_publication_retries: 2 }, { builder_publication_retries: true },
    { builder_publication_failures: [{ ...recovery, retry_requested: "true" }] },
    { builder_publication_failures: [{ ...recovery, source_digest: "private-canary" }] },
    { builder_publication_failures: [{ ...recovery, reason: "private-canary" }] }])
    assert.equal(implementationSavedActivity(receipt({ builder_publication_retries: 1,
      builder_publication_failures: [recovery], ...change }), guide), null);
});

test("earlier guide revision is labelled historical and another guide cannot receive its activity", () => {
  const earlier = receipt({ current: false, progress: [event("Writing application/old.py", 1)] });
  const view = implementationSavedActivity(earlier, { ...guide, version: 5 });
  assert.equal(view.current, false);
  assert.equal(view.label, "Saved activity from an earlier design");
  assert.equal(implementationSavedActivity(earlier, { id: "other-guide", version: 4 }), null);
  assert.equal(implementationSavedActivity(receipt({ status: "unknown" }), guide), null);
});

test("unrecognized metadata and invalid timestamps do not fabricate saved activity", () => {
  assert.equal(implementationSavedActivity(receipt({ progress: [
    event("Model is 80 percent done; private-canary", 1),
    { message: "Writing application/x.py", at: "not-a-host-timestamp" },
    { message: "Application checks passed.", at: true }, null],
    timing: { tool: "write_project_file", elapsed_ms: 8000 } }), guide), null);
});

test("the rendered saved activity has a polite atomic status and stays independent of native transcript", () => {
  const markup = renderToStaticMarkup(React.createElement(ImplementationSavedActivity,
    { receipt: receipt({ progress: [event("Writing application/path-canary.py", 1)] }), state: guide }));
  assert.match(markup, /role="status"/);
  assert.match(markup, /aria-live="polite"/);
  assert.match(markup, /aria-atomic="true"/);
  assert.match(markup, /datetime="2026-10-03T10:00:01Z"/i);
  assert.doesNotMatch(markup, /path-canary|implementation-native|percentage|ETA/);
});

test("mounted receipt updates replace saved/check/recovery facts and preserve honest ended/stale scope", async () => {
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/" });
  const replacements = { window: dom.window, document: dom.window.document,
    IS_REACT_ACT_ENVIRONMENT: true, fetch: () => assert.fail("Saved activity must not admit or fetch anything.") };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.window.document.getElementById("root"));
  try {
    const render = async (saved, state = guide) => act(async () => root.render(
      React.createElement(ImplementationSavedActivity, { receipt: saved, state })));
    let saved = receipt({ progress: [event("Writing application/private-path-canary.py", 1)] });
    await render(saved);
    const notice = dom.window.document.querySelector('[role="status"]');
    assert.match(notice.textContent, /source change saved/);
    saved = { ...saved, status: "checking", progress: [...saved.progress, event("Running isolated syntax and application tests.", 2)] };
    await render(saved);
    assert.equal(dom.window.document.querySelector('[role="status"]') === notice, true);
    assert.match(notice.textContent, /Checking the saved source/);
    saved = { ...saved, status: "running", progress: [...saved.progress, event("Application checks passed.", 3), event("Editing application/private-new-path.py", 4)] };
    await render(saved);
    assert.match(notice.textContent, /out of date/);
    saved = { ...saved, builder_publication_retries: 1, builder_publication_failures: [recovery] };
    await render(saved);
    assert.match(notice.textContent, /One additional builder turn was requested/);
    await render({ ...saved, status: "failed", current: false }, { ...guide, version: 5 });
    assert.match(notice.textContent, /earlier design/);
    assert.doesNotMatch(notice.textContent, /private-|passed|Checking the saved source/);
    await render(saved, { id: "different-guide", version: 4 });
    assert.equal(dom.window.document.querySelector('[role="status"]') === null, true);
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
});
