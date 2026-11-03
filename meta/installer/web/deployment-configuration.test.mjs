import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { fileURLToPath } from "node:url";
import { deploymentRequiredFields, deploymentFieldId, deploymentRemovableFields, deploymentRemovalKeys,
  deploymentFieldIssues, deploymentFieldConstraintsValid } from "./deployment-configuration.mjs";
import { deploymentCanSubmit, deploymentSettings, createDeploymentPollingGate } from "./deployment-panel.mjs";

const options = { supported: true, current: true, host_provider_keys: ["OPENAI_API_KEY"], fields: [
  { key: "OPENAI_API_KEY", label: "Model provider key", required: true, secret: true },
  { key: "APP_LOGIN_PASSWORD", label: "Application password", description: "Choose a password for your local application.", required: true, secret: true },
  { key: "APP_USER", label: "Application user", required: true, secret: false },
  { key: "APP_PORT", label: "Application port", required: true, secret: false, default: "18089" },
  { key: "SAVED_TOKEN", label: "Existing adapter token", required: true, secret: true, configured: true },
  { key: "SPACE_TOKEN", label: "Generated knowledge token", required: true, secret: true, automatic: true },
  { key: "OPTIONAL_NOTE", label: "Optional note", required: false, secret: false },
] };

test("missing private settings name the package fields and update as the person types", () => {
  const values = {};
  assert.deepEqual(deploymentRequiredFields(options, values), [
    { key: "OPENAI_API_KEY", label: "Model provider key" },
    { key: "APP_LOGIN_PASSWORD", label: "Application password" },
    { key: "APP_USER", label: "Application user" },
  ]);
  assert.equal(deploymentCanSubmit(options, values, false), false);
  values.APP_USER = "test-reader";
  assert.deepEqual(deploymentRequiredFields(options, values, true), [
    { key: "APP_LOGIN_PASSWORD", label: "Application password" },
  ]);
  values.APP_LOGIN_PASSWORD = "synthetic-private-value";
  assert.deepEqual(deploymentRequiredFields(options, values, true), []);
  assert.equal(deploymentCanSubmit(options, values, true), true);
  values.APP_USER = "  ";
  assert.deepEqual(deploymentRequiredFields(options, values, true), [
    { key: "APP_USER", label: "Application user" },
  ]);
  assert.equal(deploymentCanSubmit(options, values, true), false);
});

test("configuration hints expose metadata only and preserve credential admission semantics", () => {
  const values = { APP_USER: "reader", APP_LOGIN_PASSWORD: "private-test-password", OPENAI_API_KEY: "private-test-provider" };
  const before = structuredClone(values);
  assert.deepEqual(deploymentRequiredFields(options, values), []);
  assert.deepEqual(values, before);
  const missing = deploymentRequiredFields(options, { APP_LOGIN_PASSWORD: "private-test-password" });
  assert.equal(JSON.stringify(missing).includes("private-test-password"), false);
  assert.deepEqual(deploymentSettings(options, { ...values, SPACE_TOKEN: "not-submitted", PYTHONPATH: "unrequested" }), values);
  const missingDefault = { ...options, fields: [{ key: "PRIVATE_TOKEN", label: "Private token", secret: true, required: true, default: "untrusted-default" }] };
  assert.deepEqual(deploymentRequiredFields(missingDefault, {}), [{ key: "PRIVATE_TOKEN", label: "Private token" }]);
  assert.equal(deploymentCanSubmit(missingDefault, {}, false), false);
  assert.equal(deploymentCanSubmit({ ...options, current: false }, values, true), false);
  assert.equal(deploymentCanSubmit({ ...options, supported: false }, values, true), false);
});

const constrained = { supported: true, current: true, fields: [
  { key: "APP_PASSWORD", label: "Application password", required: true, secret: true, min_length: 12, max_length: 80 },
  { key: "APP_PORT", label: "Application port", required: false, secret: false, type: "integer", minimum: 1024, maximum: 65535 },
  { key: "APP_NOTE", label: "Application note", required: false, secret: false, nonblank: true, max_length: 4 },
] };

test("public constraints reject invalid supplied values without normalizing or exposing secrets", () => {
  const values = { APP_PASSWORD: "short-test", APP_PORT: "70000", APP_NOTE: "   " }, before = structuredClone(values);
  assert.deepEqual(deploymentFieldIssues(constrained, values), [
    { key: "APP_PASSWORD", label: "Application password", detail: "Use at least 12 characters." },
    { key: "APP_PORT", label: "Application port", detail: "Enter a whole number between 1024 and 65535, without spaces." },
    { key: "APP_NOTE", label: "Application note", detail: "Enter a value that is not only whitespace." },
  ]);
  assert.equal(deploymentCanSubmit(constrained, values, false), false);
  assert.ok(!JSON.stringify(deploymentFieldIssues(constrained, values)).includes(values.APP_PASSWORD));
  assert.deepEqual(values, before);
  const valid = { APP_PASSWORD: "  exact-secret-with-spaces  ", APP_PORT: "+18089", APP_NOTE: "" };
  assert.equal(deploymentCanSubmit(constrained, valid, false), true);
  assert.deepEqual(deploymentSettings(constrained, valid), { APP_PASSWORD: valid.APP_PASSWORD, APP_PORT: valid.APP_PORT });
  assert.deepEqual(deploymentFieldIssues(constrained, { APP_PASSWORD: "😀".repeat(12), APP_NOTE: "😀".repeat(4) }), []);
  assert.equal(deploymentFieldIssues(constrained, { APP_NOTE: "😀".repeat(5) })[0].detail, "Use at most 4 characters.");
  for (const APP_PORT of [" 18089", "18089 ", "18089\n", "18089\r\n", "1.8e4", "18089.0", "９９９９", "9007199254740992", "9".repeat(400)])
    assert.equal(deploymentFieldIssues(constrained, { APP_PORT }).length, 1);
  assert.deepEqual(deploymentFieldIssues(constrained, { APP_PORT: "70000" }, ["APP_PORT"]), []);
  assert.deepEqual(deploymentFieldIssues({ fields: [{ key: "SAVED", label: "Saved", required: true, secret: true,
    configured: true, min_length: 12 }] }, { SAVED: "" }), []);
  for (const APP_NOTE of ["\u001c", "\u0085", "\u3000"])
    assert.equal(deploymentFieldIssues(constrained, { APP_NOTE }).length, 1);
  assert.deepEqual(deploymentFieldIssues(constrained, { APP_NOTE: "\ufeff" }), []);
});

const authenticatedConsole = { supported: true, current: true, console_scope: { authentication: "local-password" }, fields: [
  { key: "CONSOLE_USER_ID", label: "Console user", required: true, secret: false },
  { key: "CONSOLE_LOGIN_PASSWORD", label: "Console password", required: true, secret: true },
] };

test("declared Console credentials obey runtime rules without inferring them from field names", () => {
  const values = { CONSOLE_USER_ID: "test reader", CONSOLE_LOGIN_PASSWORD: "short" }, before = structuredClone(values);
  const issues = deploymentFieldIssues(authenticatedConsole, values);
  assert.deepEqual(issues, [
    { key: "CONSOLE_USER_ID", label: "Console user", detail: "Use 1-200 letters, numbers, dots, underscores, @, + or hyphens." },
    { key: "CONSOLE_LOGIN_PASSWORD", label: "Console password", detail: "Use 12-4096 characters for the Console password." },
  ]);
  assert.deepEqual(values, before);
  assert.equal(deploymentCanSubmit(authenticatedConsole, values), false);
  for (const CONSOLE_USER_ID of ["test-reader\n", "reader/one", "😀", "a".repeat(201)])
    assert.equal(deploymentFieldIssues(authenticatedConsole, { CONSOLE_USER_ID }).length, 1);
  for (const CONSOLE_LOGIN_PASSWORD of ["x".repeat(4097), "x".repeat(12) + "\ud800", "x".repeat(12) + "\udfff"])
    assert.equal(deploymentFieldIssues(authenticatedConsole, { CONSOLE_LOGIN_PASSWORD }).length, 1);
  for (const CONSOLE_LOGIN_PASSWORD of ["  exact secret  ", "😀".repeat(4096)]) {
    const valid = { CONSOLE_USER_ID: "test-reader+one@example.test", CONSOLE_LOGIN_PASSWORD };
    assert.deepEqual(deploymentFieldIssues(authenticatedConsole, valid), []);
    assert.deepEqual(deploymentSettings(authenticatedConsole, valid), valid);
  }
  for (const console_scope of [undefined, { authentication: "loopback-only" }, { authentication: "other" }])
    assert.deepEqual(deploymentFieldIssues({ ...authenticatedConsole, console_scope }, values), []);
  assert.ok(!JSON.stringify(issues).includes('"short"'));
  assert.deepEqual(deploymentFieldIssues(authenticatedConsole, { CONSOLE_LOGIN_PASSWORD: "" }), []);
});

test("remote metadata constraints require bounded declarations", () => {
  for (const field of constrained.fields) assert.equal(deploymentFieldConstraintsValid(field), true);
  for (const field of [{ min_length: true }, { max_length: 32769 }, { min_length: 4, max_length: 3 },
    { nonblank: "true" }, { type: "number", minimum: 1, maximum: 9 }, { minimum: 1 },
    { type: "integer", minimum: 4, maximum: 3 }, { type: "integer", minimum: 0, maximum: 2 ** 53 },
    { type: "integer", minimum: 0 }, { min_length: null }, { nonblank: null }, { type: null },
    { minimum: null }, { required: true, max_length: 0 },
    { type: "integer", minimum: 100000, maximum: 999999, max_length: 3 }])
    assert.equal(deploymentFieldConstraintsValid(field), false);
  assert.equal(deploymentFieldConstraintsValid({ required: true, secret: true }), true);
});

// Render the actual production field component. This verifies accessible form
// wiring without claiming a browser, provider, deployment or private admission.
const bundle = await build({ entryPoints: ["deployment-panel.jsx"], absWorkingDir: fileURLToPath(new URL(".", import.meta.url)),
  bundle: true, format: "esm", platform: "node", write: false, loader: { ".css": "empty" }, logLevel: "silent" });
const { DeploymentConfigurationFields, DeploymentInstallationArea, DeploymentSettingsGuidance, revealDeploymentConfiguration } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].contents).toString("base64")}`);
const render = (values, useHost = false) => renderToStaticMarkup(React.createElement(DeploymentConfigurationFields, {
  options, values, useHost, guideId: "owned-guide", busy: false, onChange() {},
}));

test("production fields show accessible correction hints while keeping private text in its input", () => {
  const value = "short-test";
  const html = renderToStaticMarkup(React.createElement(DeploymentConfigurationFields, {
    options: constrained, values: { APP_PASSWORD: value, APP_PORT: "70000" },
    guideId: "owned-guide", busy: false, onChange() {},
  }));
  const id = deploymentFieldId("owned-guide", "APP_PASSWORD");
  assert.match(html, /Check these settings before continuing:/);
  assert.ok(html.includes(`href="#${id}"`));
  assert.ok(html.includes(`aria-describedby="${id}-constraint"`));
  assert.ok(html.includes('aria-invalid="true"'));
  assert.match(html, /Use at least 12 characters\./);
  assert.match(html, /Enter a whole number between 1024 and 65535, without spaces\./);
  assert.equal((html.match(/short-test/g) || []).length, 1);
  assert.ok(!html.includes('minlength="12"'), "UTF-16 browser length must not disagree with Unicode field constraints");
  const valid = renderToStaticMarkup(React.createElement(DeploymentConfigurationFields, {
    options: constrained, values: { APP_PASSWORD: "  exact-secret-with-spaces  ", APP_PORT: "18089" },
    guideId: "owned-guide", busy: false, onChange() {},
  }));
  assert.doesNotMatch(valid, /Check these settings|aria-invalid="true"/);
});

test("production Console fields expose correction hints even without generic scalar metadata", () => {
  const html = renderToStaticMarkup(React.createElement(DeploymentConfigurationFields, {
    options: authenticatedConsole, values: { CONSOLE_USER_ID: "test reader", CONSOLE_LOGIN_PASSWORD: "short" },
    guideId: "synthetic-console-guide", busy: false, onChange() {},
  }));
  assert.match(html, /Use 12-4096 characters for the Console password\./);
  assert.match(html, /Use 1-200 letters, numbers/);
  const id = deploymentFieldId("synthetic-console-guide", "CONSOLE_LOGIN_PASSWORD");
  assert.ok(html.includes(`aria-describedby="${id}-constraint"`));
  assert.ok(html.includes('aria-invalid="true"'));
  assert.equal((html.match(/value="short"/g) || []).length, 1);
  assert.ok(!html.replace(/<input[^>]*>/g, "").includes("short"));
});

test("production fields connect missing-setting links, required hints and accessible descriptions", () => {
  const html = render({}, true), id = deploymentFieldId("owned-guide", "APP_LOGIN_PASSWORD");
  assert.match(html, /Complete the required settings to continue:/);
  assert.match(html, /role="status" aria-live="polite"/);
  assert.ok(html.includes(`href="#${id}"`));
  assert.ok(html.includes(`for="${id}"`));
  const passwordInput = html.match(new RegExp(`<input\\b[^>]*\\bid="${id}"[^>]*>`))?.[0];
  assert.ok(passwordInput);
  assert.ok(passwordInput.includes('name="APP_LOGIN_PASSWORD"'));
  assert.ok(passwordInput.includes('type="password"'));
  assert.ok(passwordInput.includes(`aria-describedby="${id}-description ${id}-required"`));
  assert.ok(html.includes(`id="${id}-required"`));
  assert.match(html, /Enter Application password to continue/);
  assert.doesNotMatch(html, /name="SPACE_TOKEN"/);
  const inheritedId = deploymentFieldId("owned-guide", "OPENAI_API_KEY");
  assert.ok(!html.includes(`href="#${inheritedId}"`));
  assert.ok(!html.includes(`id="${inheritedId}-required"`));
});

test("production form guidance disappears for satisfied defaults, existing settings and private input", () => {
  const html = render({ APP_USER: "reader", APP_LOGIN_PASSWORD: "synthetic-only-password" }, true);
  assert.doesNotMatch(html, /Complete the required settings|Enter .* to continue|aria-required="true"/);
  assert.match(html, /Already configured; leave blank to keep/);
  // The password remains solely in its native password input. It is never a
  // visible hint/status message or an emitted missing-fields projection.
  assert.equal((html.match(/synthetic-only-password/g) || []).length, 1);
  assert.deepEqual(deploymentRequiredFields(options, { APP_USER: "reader", APP_LOGIN_PASSWORD: "synthetic-only-password" }, true), []);
});

test("private setup keeps required accounts visible and optional customization behind a disclosure", () => {
  const html = render({}, false), optionalStart = html.indexOf('<details class="deployment-optional-settings">');
  assert.ok(optionalStart > 0);
  assert.ok(html.indexOf('name="APP_LOGIN_PASSWORD"') < optionalStart);
  assert.ok(html.indexOf('name="APP_USER"') < optionalStart);
  assert.ok(html.indexOf('name="OPTIONAL_NOTE"') > optionalStart);
  assert.match(html, /Optional settings \(1\)/);
  assert.doesNotMatch(html.slice(optionalStart, html.indexOf(">", optionalStart) + 1), /\bopen\b/);
  const requiredOnly = renderToStaticMarkup(React.createElement(DeploymentConfigurationFields, {
    options: { fields: [{ key: "ACCOUNT_PASSWORD", label: "Account password", required: true, secret: true }] },
    values: {}, guideId: "owned-guide", busy: false, onChange() {},
  }));
  assert.doesNotMatch(requiredOnly, /<details|Optional settings/);
  assert.match(requiredOnly, /name="ACCOUNT_PASSWORD"/);
});

const removalOptions = { ...options, fields: [...options.fields,
  { key: "SAVED_OPTIONAL_TOKEN", label: "Saved adapter token", required: false, secret: true, configured: true, removable: true },
  { key: "OPTIONAL_DEFAULT", label: "Optional preference", required: false, secret: false, configured: true, removable: true, default: "source-default" },
  { key: "OPTIONAL_UNSAVED", label: "Unsaved adapter", required: false, secret: true, configured: false, removable: true },
  { key: "HOST_AUTOMATIC", label: "Host managed setting", required: false, configured: true, removable: true, automatic: true },
  { key: "NOT_REMOVABLE", label: "Unverified removal", required: false, configured: true, removable: false },
] };

test("only saved optional declared overrides can be selected for removal", () => {
  const before = structuredClone(removalOptions);
  assert.deepEqual(deploymentRemovableFields(removalOptions).map((field) => field.key), ["SAVED_OPTIONAL_TOKEN", "OPTIONAL_DEFAULT"]);
  const selected = ["SAVED_OPTIONAL_TOKEN"];
  const keys = deploymentRemovalKeys(removalOptions, selected);
  assert.deepEqual(keys, selected);
  assert.notEqual(keys, selected);
  for (const keys of [["SAVED_TOKEN"], ["HOST_AUTOMATIC"], ["OPTIONAL_UNSAVED"], ["NOT_REMOVABLE"], ["UNDECLARED"],
    ["SAVED_OPTIONAL_TOKEN", "SAVED_OPTIONAL_TOKEN"], [null], "SAVED_OPTIONAL_TOKEN", null])
    assert.equal(deploymentRemovalKeys(removalOptions, keys), null);
  assert.deepEqual(deploymentRemovalKeys({ fields: [] }, []), []);
  assert.deepEqual(removalOptions, before);
});

function elementNodes(value) {
  if (Array.isArray(value)) return value.flatMap(elementNodes);
  if (!value || typeof value !== "object" || !value.props) return [];
  return [value, ...elementNodes(value.props.children)];
}

test("production removal controls retain blank-keeps semantics and emit explicit field intent only", () => {
  const before = structuredClone(removalOptions), intents = [];
  const props = { options: removalOptions, values: {}, useHost: true, guideId: "owned-guide", busy: false,
    onChange() {}, onRemove(key, checked) { intents.push({ key, checked }); }, removed: [] };
  const tree = DeploymentConfigurationFields(props);
  const nodes = elementNodes(tree);
  const controls = nodes.filter((node) => node.type === "input" && node.props.type === "checkbox");
  assert.deepEqual(controls.map((node) => node.props.id), [
    `${deploymentFieldId("owned-guide", "SAVED_OPTIONAL_TOKEN")}-remove`,
    `${deploymentFieldId("owned-guide", "OPTIONAL_DEFAULT")}-remove`,
  ]);
  assert.equal(controls.every((node) => node.props.checked === false), true);
  controls[0].props.onChange({ target: { checked: true } });
  controls[0].props.onChange({ target: { checked: false } });
  assert.deepEqual(intents, [{ key: "SAVED_OPTIONAL_TOKEN", checked: true }, { key: "SAVED_OPTIONAL_TOKEN", checked: false }]);
  const html = renderToStaticMarkup(React.createElement(DeploymentConfigurationFields, props));
  assert.match(html, /Remove the saved Saved adapter token value when saving\. Application defaults may still apply\./);
  assert.match(html, /Already configured; leave blank to keep/);
  assert.doesNotMatch(html, /private_settings|source_digest|package_sha256|type="checkbox"[^>]*checked=""/);
  const removed = renderToStaticMarkup(React.createElement(DeploymentConfigurationFields, { ...props, removed: ["SAVED_OPTIONAL_TOKEN"] }));
  const id = deploymentFieldId("owned-guide", "SAVED_OPTIONAL_TOKEN");
  const input = removed.match(new RegExp(`<input\\b[^>]*\\bid="${id}"[^>]*>`))?.[0];
  assert.ok(input?.includes('type="password"'));
  assert.ok(input?.includes('value=""'));
  assert.ok(input?.includes('disabled=""'));
  assert.ok(input?.includes('placeholder="Saved value marked for removal"'));
  assert.ok(removed.includes(`id="${id}-remove"`));
  assert.deepEqual(deploymentRequiredFields(removalOptions, { APP_USER: "reader", APP_LOGIN_PASSWORD: "synthetic-only" }, true), []);
  assert.deepEqual(removalOptions, before);
  const newInstall = renderToStaticMarkup(React.createElement(DeploymentConfigurationFields, { ...props, onRemove: undefined }));
  assert.doesNotMatch(newInstall, /Remove the saved|type="checkbox"/);
  const busy = elementNodes(DeploymentConfigurationFields({ ...props, busy: true }));
  assert.equal(busy.filter((node) => node.type === "input").every((node) => node.props.disabled), true);
});

test("production placement shows private setup immediately after its action and before later installation panels", () => {
  const actions = [];
  const configuration = React.createElement("form", { className: "deployment-form", onSubmit: () => actions.push("save") },
    React.createElement("h3", { tabIndex: -1, "data-configuration-heading": true }, "Change this installation's private settings"),
    React.createElement(DeploymentConfigurationFields, { options: removalOptions, values: {}, useHost: true,
      guideId: "owned-guide", busy: false, onChange() {}, onRemove() {} }));
  const html = renderToStaticMarkup(React.createElement("section", null,
    React.createElement(DeploymentInstallationArea, { configuration },
      React.createElement("button", { onClick: () => actions.push("open") }, "Change private settings")),
    React.createElement("article", null, "Use your installed application"),
    React.createElement("details", null, React.createElement("summary", null, "Runtime details"))));
  assert.ok(html.indexOf("</button></div><form") > 0);
  assert.ok(html.indexOf("</form>") < html.indexOf("Use your installed application"));
  assert.ok(html.indexOf("</form>") < html.indexOf("Runtime details"));
  assert.match(html, /tabindex="-1" data-configuration-heading="true"/);
  assert.deepEqual(actions, []);
  const closed = renderToStaticMarkup(React.createElement(DeploymentInstallationArea, { configuration: false },
    React.createElement("button", null, "Change private settings")));
  assert.doesNotMatch(closed, /<form|name="APP_LOGIN_PASSWORD"/);
});

test("an admitted installation hides the private form even if its HTTP response was lost", async () => {
  const guideId = "owned-guide", gate = createDeploymentPollingGate();
  gate.open(guideId);
  const postResponse = Promise.reject(new Error("The connection closed before its response arrived."));
  await assert.rejects(postResponse, /connection closed/);
  const configuration = React.createElement("form", { className: "deployment-form" },
    React.createElement(DeploymentConfigurationFields, { options, values: {}, guideId, busy: false, onChange() {} }),
    React.createElement("button", { type: "submit" }, "Install and start"));
  for (const status of ["queued", "installing", "checking", "starting", "running", "stopping"]) {
    const observed = await gate.observe(guideId, async () => ({ guide_id: guideId, deployment_id: "actual-admission", status }));
    assert.ok(gate.current(observed.token, guideId));
    const html = renderToStaticMarkup(React.createElement(DeploymentInstallationArea,
      { configuration, receipt: observed.receipt }, React.createElement("button", null, "Stop application")));
    assert.match(html, /Stop application/);
    assert.doesNotMatch(html, /<form|APP_LOGIN_PASSWORD|Install and start/);
  }
  const unadmitted = renderToStaticMarkup(React.createElement(DeploymentInstallationArea, { configuration, receipt: null }));
  assert.match(unadmitted, /<form|Install and start/);
});

test("a running application explains private setup without bypassing stopped-source checks", () => {
  const running = renderToStaticMarkup(React.createElement(DeploymentSettingsGuidance, { receipt: { status: "running" } }));
  assert.match(running, /stop the application first/);
  assert.match(running, /checks its saved source and cleanup/);
  assert.doesNotMatch(running, /<button|<form/);
  for (const status of ["queued", "installing", "starting", "stopping", "stopped", "failed", "interrupted"])
    assert.equal(renderToStaticMarkup(React.createElement(DeploymentSettingsGuidance, { receipt: { status } })), "");
  assert.equal(renderToStaticMarkup(React.createElement(DeploymentSettingsGuidance, { receipt: null })), "");
});

test("verified setup reveal focuses only the exact connected form and scrolls it without submitting", () => {
  const key = JSON.stringify(["guide", 4, "failed-B", "settings", "exact-installed-A", "checked-source"]);
  const calls = [], heading = { focus: (options) => calls.push({ focus: options }) };
  const form = { isConnected: true, dataset: { configurationKey: key },
    querySelector(selector) { assert.equal(selector, "[data-configuration-heading]"); return heading; },
    scrollIntoView: (options) => calls.push({ scroll: options }) };
  assert.equal(revealDeploymentConfiguration(form, key), true);
  assert.deepEqual(calls, [{ focus: { preventScroll: true } }, { scroll: { block: "start", behavior: "auto" } }]);
  calls.length = 0;
  for (const stale of [null, { ...form, isConnected: false }, { ...form, dataset: { configurationKey: "different-installation" } },
    { ...form, querySelector: () => null }]) assert.equal(revealDeploymentConfiguration(stale, key), false);
  assert.equal(revealDeploymentConfiguration(form, "old-guide-read"), false);
  assert.equal(revealDeploymentConfiguration(form, ""), false);
  assert.deepEqual(calls, []);
});
