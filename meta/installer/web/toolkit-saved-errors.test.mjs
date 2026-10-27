import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";
import React, { act } from "react";
import { savedToolkitErrorMessage, SAVED_TOOLKIT_ERROR_FALLBACK } from "./toolkit-saved-errors.mjs";

test("known refusals explain the next action and preserve uncertain preparation", () => {
  for (const [code, expected] of [
    ["saved_settings_host_changed", /host.*changed.*approve it again/],
    ["saved_settings_request_changed", /request changed.*check the full request/],
    ["saved_settings_host_unavailable", /not enabled.*host administrator/],
    ["saved_settings_profile_unavailable", /profile is unavailable.*replacement/],
    ["saved_settings_manifest_refused", /could not be verified.*saved workspace/],
    ["saved_settings_review_interrupted", /interrupted.*no preparation was automatically repeated/],
    ["saved_settings_preparation_unconfirmed", /could not be confirmed.*retained attempt/],
    ["saved_settings_audience_approval_required", /full request.*contains no secrets/],
    ["saved_settings_review_in_progress", /already being prepared.*Wait/],
  ]) {
    const message = savedToolkitErrorMessage(code);
    assert.match(message, expected);
    assert.equal(message.includes(code), false);
    assert.equal(savedToolkitErrorMessage(new Error(code)), message);
  }
});

test("unknown errors and private diagnostics always use a fixed generic message", () => {
  for (const value of ["saved_settings_future_code", "PRIVATE_TOKEN=secret /private/workspace/request.json",
    new Error("PRIVATE_TOKEN=secret"), { detail: "private diagnostic" }, "__proto__", "constructor", null]) {
    assert.equal(savedToolkitErrorMessage(value), SAVED_TOOLKIT_ERROR_FALLBACK);
  }
});

test("the panel renders friendly retained refusals and hides raw action diagnostics", async () => {
  const require = createRequire(import.meta.url);
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const bundled = await build({ entryPoints: [fileURLToPath(new URL("toolkit-saved-setup.jsx", import.meta.url))],
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", jsx: "automatic",
    loader: { ".css": "empty" }, plugins: [{ name: "shared-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
    } }] });
  const { ToolkitSavedSetup } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString("base64")}`);
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/" });
  const row = { request_id: "1".repeat(32), request_sha256: "a".repeat(64), family: "services", profile: "gmail" };
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true, fetch: async (url, options = {}) => {
      if (options.method) throw new Error("PRIVATE_TOKEN=secret /private/workspace/request.json");
      return { ok: true, json: async () => ({ available: true, status: "unconfirmed",
        manifest_sha256: "b".repeat(64), requests: [row], error: "saved_settings_preparation_unconfirmed" }) };
    } };
  const descriptors = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root"));
  const pause = () => new Promise(resolve => setTimeout(resolve, 5));
  try {
    await act(async () => { root.render(React.createElement(ToolkitSavedSetup,
      { guideId: "guide", version: 1, disabled: false, onClose() {} })); await pause(); });
    assert.match(document.querySelector('[role="alert"]').textContent, /Preparation could not be confirmed/);
    assert.doesNotMatch(document.body.textContent, /saved_settings_/);
    const button = [...document.querySelectorAll("button")].find(node => node.textContent.startsWith("gmail"));
    await act(async () => { button.click(); await pause(); });
    assert.match(document.body.textContent, /Your conversation and draft are kept/);
    assert.doesNotMatch(document.body.textContent, /PRIVATE_TOKEN|secret|\/private\/workspace/);
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of descriptors) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
});
