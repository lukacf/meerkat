import assert from "node:assert/strict";
import { after, test } from "node:test";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash, webcrypto } from "node:crypto";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { build } from "esbuild";
import { installedPackageBinding } from "./installed-package.mjs";

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const directory = await mkdtemp(join(tmpdir(), "forge-installed-package-"));
const modulePath = join(directory, "installed-package.mjs");
after(() => rm(directory, { recursive: true, force: true }));
const bundled = await build({ entryPoints: [fileURLToPath(new URL("installed-package.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", outfile: modulePath,
  sourcemap: "linked", sourcesContent: true, logLevel: "silent",
  nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
  plugins: [{ name: "shared-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
for (const output of bundled.outputFiles) await writeFile(output.path, output.contents);
const { InstalledPackageDownload } = await import(pathToFileURL(modulePath).href);
const guide = { id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", version: 8,
  implementation: { build_id: "failed-newer-build", status: "failed" } };
const payload = new Blob(["synthetic retained package bytes"]);
const hash = createHash("sha256").update(Buffer.from(await payload.arrayBuffer())).digest("hex");
const receipt = { guide_id: guide.id, guide_version: 5,
  deployment_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb", build_id: "original-accepted-build",
  source_kind: "application", source_digest: "c".repeat(64), package_sha256: hash,
  status: "running", target: "local" };
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };

// Synthetic transport only: the production read-only component never contacts
// Forge, an installed runtime, private settings or an operating-system service.
async function mounted(verify, initial = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const saved = new Map(), calls = [], downloads = [], revoked = [], timers = new Map();
  const realSetTimeout = globalThis.setTimeout, realClearTimeout = globalThis.clearTimeout;
  let transport = async () => ({ ok: true, blob: async () => payload });
  const replace = (name, value) => {
    saved.set(name, Object.getOwnPropertyDescriptor(globalThis, name));
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
  };
  for (const [name, value] of Object.entries({ window: dom.window, document: dom.window.document,
    navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true, crypto: webcrypto,
    fetch: async (url, options = {}) => { calls.push({ url, options }); return transport(url, options); },
    setTimeout(callback, delay, ...args) {
      if (delay !== 60000 && delay !== 1000) return realSetTimeout(callback, delay, ...args);
      const token = { delay }; timers.set(token, () => callback(...args)); return token;
    },
    clearTimeout(token) { if (!timers.delete(token)) realClearTimeout(token); },
  })) replace(name, value);
  const oldCreate = URL.createObjectURL, oldRevoke = URL.revokeObjectURL;
  URL.createObjectURL = () => `blob:checked-${downloads.length}`;
  URL.revokeObjectURL = url => revoked.push(url);
  dom.window.HTMLAnchorElement.prototype.click = function () { downloads.push({ href: this.href, filename: this.download }); };
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.window.document.getElementById("root"));
  let props = { receipt: structuredClone(receipt), state: structuredClone(guide), ...initial };
  let unmounted = false;
  async function render(next = {}) { props = { ...props, ...next }; await act(async () => root.render(React.createElement(InstalledPackageDownload, props))); }
  const settle = async () => act(async () => { await new Promise(resolve => realSetTimeout(resolve, 15)); });
  const click = async () => act(async () => { dom.window.document.querySelector("button")?.click(); });
  try {
    await render();
    await verify({ document: dom.window.document, calls, downloads, revoked, render, click, settle,
      setTransport(value) { transport = value; },
      async expire() { await act(async () => { for (const [token, callback] of timers) if (token.delay === 60000) callback(); }); },
      async unmount() { await act(async () => root.unmount()); unmounted = true; },
    });
  } finally {
    if (!unmounted) await act(async () => root.unmount());
    URL.createObjectURL = oldCreate; URL.revokeObjectURL = oldRevoke;
    for (const [name, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor); else delete globalThis[name];
    }
    dom.window.close();
  }
}

test("installed source stays exportable when a newer proposal's build failed", async () => mounted(async ui => {
  await ui.click(); await ui.settle();
  assert.equal(ui.calls.length, 1);
  const url = new URL(ui.calls[0].url, "http://localhost");
  assert.equal(url.pathname, `/api/guide/${guide.id}/deployment/installed-package`);
  assert.equal(url.searchParams.get("build_id"), receipt.build_id);
  assert.equal(url.searchParams.get("deployment_id"), receipt.deployment_id);
  assert.equal(url.searchParams.get("source_digest"), receipt.source_digest);
  assert.equal(url.searchParams.get("package_sha256"), hash);
  assert.equal(ui.calls[0].options.method, undefined);
  assert.equal(ui.downloads.length, 1);
  assert.match(ui.downloads[0].filename, /^forge-installed-application-/);
}));

test("a tampered download cannot be saved and an explicit retry remains available", async () => mounted(async ui => {
  ui.setTransport(async () => ({ ok: true, blob: async () => new Blob(["changed bytes"]) }));
  await ui.click(); await ui.settle();
  assert.equal(ui.downloads.length, 0);
  assert.match(ui.document.querySelector('[role="alert"]').textContent, /differs from this installation/);
  assert.equal(ui.document.querySelector("button").disabled, false);
  ui.setTransport(async () => ({ ok: true, blob: async () => payload }));
  await ui.click(); await ui.settle();
  assert.equal(ui.calls.length, 2); assert.equal(ui.downloads.length, 1);
}));

test("duplicate clicks send one read and a stalled body releases the download", async () => mounted(async ui => {
  const held = deferred();
  ui.setTransport(async () => ({ ok: true, blob: () => held.promise }));
  await ui.click(); await ui.click();
  assert.equal(ui.calls.length, 1);
  assert.equal(ui.document.querySelector("button").disabled, true);
  await ui.expire(); await ui.settle();
  assert.match(ui.document.querySelector('[role="alert"]').textContent, /taking too long/);
  assert.equal(ui.document.querySelector("button").disabled, false);
  assert.equal(ui.calls[0].options.signal.aborted, true);
  held.resolve(payload); await ui.settle();
  assert.equal(ui.downloads.length, 0);
}));

test("a source switch and return retires the original read even if its body arrives later", async () => mounted(async ui => {
  const held = deferred();
  ui.setTransport(async () => ({ ok: true, blob: () => held.promise }));
  await ui.click();
  await ui.render({ receipt: { ...receipt, source_digest: "d".repeat(64) } });
  await ui.render({ receipt });
  held.resolve(payload); await ui.settle();
  assert.equal(ui.downloads.length, 0);
  assert.equal(ui.calls[0].options.signal.aborted, true);
  assert.equal(ui.document.querySelector('[role="alert"]') === null, true);
  ui.setTransport(async () => ({ ok: true, blob: async () => payload }));
  await ui.click(); await ui.settle(); assert.equal(ui.downloads.length, 1);
}));

test("selection retirement before a rerender cannot download for the departed setup", async () => {
  let selected = true;
  const scope = { token: () => "original-guide-load", capture: () => () => selected };
  await mounted(async ui => {
    const held = deferred();
    ui.setTransport(async () => ({ ok: true, blob: () => held.promise }));
    await ui.click(); selected = false; held.resolve(payload); await ui.settle();
    assert.equal(ui.downloads.length, 0);
  }, { selectionScope: scope });
});

test("unmounting retires the download without any package or lifecycle action", async () => mounted(async ui => {
  const held = deferred();
  ui.setTransport(async () => ({ ok: true, blob: () => held.promise }));
  await ui.click(); await ui.unmount();
  held.resolve(payload); await ui.settle();
  assert.equal(ui.downloads.length, 0);
  assert.equal(ui.calls.length, 1);
  assert.equal(ui.calls[0].options.signal.aborted, true);
}));

test("native teams, remote destinations and foreign receipts do not offer this local application export", () => {
  assert.equal(installedPackageBinding({ ...receipt, source_kind: "native-team", build_id: null }, guide), null);
  assert.equal(installedPackageBinding({ ...receipt, target: "gcp" }, guide), null);
  assert.equal(installedPackageBinding({ ...receipt, guide_id: "another-guide" }, guide), null);
  assert.equal(installedPackageBinding({ ...receipt, package_sha256: "unconfirmed" }, guide), null);
});
