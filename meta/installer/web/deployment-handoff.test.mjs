import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { fileURLToPath } from "node:url";

const bundle = await build({ entryPoints: ["deployment-panel.jsx"], absWorkingDir: fileURLToPath(new URL(".", import.meta.url)),
  bundle: true, format: "esm", platform: "node", write: false, loader: { ".css": "empty" }, logLevel: "silent" });
const { DeploymentHandoff, DeploymentUpdatePreparation, DeploymentUpdateActions, DeploymentRuntimeDetails } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].contents).toString("base64")}`);
const render = (handoff) => renderToStaticMarkup(React.createElement(DeploymentHandoff, { handoff }));

test("manual native Console access remains optional and only renders the supplied available runtime link", () => {
  const props = { consoleUrl: "http://127.0.0.1:4110/console", scope: { authentication: "local-password",
    requested_interfaces: "Chat and a custom UI" }, Icon: () => null };
  const html = renderToStaticMarkup(React.createElement(DeploymentRuntimeDetails, props));
  assert.match(html, /^<details class="deployment-evidence deployment-interface-scope"><summary>Runtime and native Console details \(optional\)<\/summary>/);
  assert.doesNotMatch(html, /<details[^>]*\bopen\b|setup-action primary/);
  assert.match(html, /href="http:\/\/127\.0\.0\.1:4110\/console"/);
  assert.match(html, /Sign in with the Console password you chose for this installation/);
  const unavailable = renderToStaticMarkup(React.createElement(DeploymentRuntimeDetails, { ...props, consoleUrl: null }));
  assert.doesNotMatch(unavailable, /<a |href=/);
  assert.equal(renderToStaticMarkup(React.createElement(DeploymentRuntimeDetails, { ...props, consoleUrl: null, scope: null })), "");
  const older = renderToStaticMarkup(React.createElement(DeploymentRuntimeDetails, { ...props, scope: null }));
  assert.match(older, /Open native MobKit Console directly/);
  assert.doesNotMatch(older, /Chat and a custom UI|declares one local Console account/);
});

test("production Run instructions distinguish installation, local tunnel and optional service actions", () => {
  const handoff = { mode: "remote-package", status: "not-connected", steps: [
    { title: "Install and enter private configuration", where: "destination computer", description: "The generated setup asks for private fields.", commands: ["bash install.sh"] },
    { title: "Verify, then start the background service", where: "destination computer", description: "Requires the user systemd manager.", commands: ["bash start.sh --doctor", "bash deploy/install-service.sh start"] },
    { title: "If using another computer, open a private SSH tunnel", where: "your browser computer, not the destination SSH session", description: "Replace USER@HOST and keep this terminal open.", commands: ["ssh -N -L 18088:127.0.0.1:18088 USER@HOST"] },
  ], operations: [
    { title: "Check service status", commands: ["bash deploy/install-service.sh status"] },
    { title: "Stop the application", commands: ["bash deploy/install-service.sh stop"] },
    { title: "Use a terminal instead", description: "Keep the terminal open and stop with Control-C.", commands: ["bash start.sh"] },
  ], limitations: ["Remote health has not been verified."] };
  const before = structuredClone(handoff), html = render(handoff);
  const orderedSteps = html.slice(html.indexOf("<ol>"), html.indexOf("</ol>") + 5);
  assert.match(orderedSteps, /bash deploy\/install-service\.sh start/);
  assert.match(orderedSteps, /your browser computer, not the destination SSH session/);
  assert.match(orderedSteps, /ssh -N -L 18088:127\.0\.0\.1:18088 USER@HOST/);
  assert.doesNotMatch(orderedSteps, /install-service\.sh (?:stop|status)/);
  const management = html.match(/<details class="deployment-evidence"><summary>Manage[^]*?<\/details>/)?.[0];
  assert.ok(management);
  assert.match(management, /reference actions, not additional installation steps/);
  assert.match(management, /bash deploy\/install-service\.sh stop/);
  assert.match(management, /Control-C/);
  assert.doesNotMatch(management, /\bopen=/);
  assert.match(html, /Remote deployment is not connected/);
  assert.match(html, /Remote health has not been verified/);
  assert.deepEqual(handoff, before);
});

test("packages without service support do not display invented management and missing scripts remain visible", () => {
  const foreground = render({ steps: [{ title: "Run in this terminal", where: "destination computer",
    description: "Keep the terminal open; stop with Control-C.", commands: ["bash start.sh"] }], limitations: [] });
  assert.match(foreground, /Control-C/);
  assert.doesNotMatch(foreground, /Manage the installed application|install-service/);
  const missing = render({ steps: [], operations: [], limitations: ["Required install.sh is missing. Continue implementation."] });
  assert.match(missing, /<details class="deployment-evidence" open="">/);
  assert.match(missing, /Required install\.sh is missing/);
  assert.doesNotMatch(missing, /bash install\.sh|service status|live-ready/);
});

test("the production background preview explains activation limits before its explicit comparison action", () => {
  const state = { id: "guide", version: 4, proposal: { target: "mac" } };
  const build = { guide_id: state.id, guide_version: 4, current: true, status: "completed", build_id: "candidate",
    ready_to_package: true, acceptance_trusted: true, acceptance_current: true, acceptance: { status: "pass" } };
  const receipt = { guide_id: state.id, deployment_id: "owned", build_id: "installed", status: "running", execution_mode: "background" };
  const options = { build_id: build.build_id, guide_version: 4, current: true, target: "mac", host_target: "mac", supported: true };
  const actions = [], props = { state, build, receipt, options, disabled: false, onStage: () => actions.push("compare") };
  const html = renderToStaticMarkup(React.createElement(DeploymentUpdatePreparation, props));
  assert.ok(html.indexOf("This background service can only preview update compatibility") < html.indexOf("<button"));
  assert.match(html, /Preview update compatibility/);
  assert.doesNotMatch(html, /Stage accepted update|Apply and verify update|Restore previous application/);
  assert.deepEqual(actions, []);
  const element = DeploymentUpdatePreparation(props);
  element.props.children[1].props.onClick();
  assert.deepEqual(actions, ["compare"]);
  const disabled = DeploymentUpdatePreparation({ ...props, disabled: true });
  assert.equal(disabled.props.children[1].props.disabled, true);
  const foreground = renderToStaticMarkup(React.createElement(DeploymentUpdatePreparation,
    { ...props, receipt: { ...receipt, execution_mode: "interactive" } }));
  assert.match(foreground, /Stage accepted update/);
  assert.doesNotMatch(foreground, /Background service updates|Preview update compatibility/);
});

test("the supported background UI explains Stop first and dispatches only explicit exact eligible actions", () => {
  const state = { id: "guide", version: 4, proposal: { target: "mac" } };
  const build = { guide_id: state.id, guide_version: 4, current: true, status: "completed", build_id: "candidate",
    ready_to_package: true, acceptance_trusted: true, acceptance_current: true, acceptance: { status: "pass" },
    source_digest: "d".repeat(64) };
  const receipt = { guide_id: state.id, deployment_id: "owned", build_id: "installed", package_sha256: "a".repeat(64),
    execution_mode: "background", status: "stopped", process_live: false, runtime_pid: null,
    service_supervisor_pid: 1234, service_cleanup_confirmed: true,
    service: { manager: "launchd", unit_sha256: "b".repeat(64), public_summary: { definition_verified: true } },
    service_observation: { owned: true, loaded: false, state: "stopped", pid: null },
    update_capabilities: { background_activation_supported: true, requires_stopped: true,
      deployment_id: "owned", build_id: "installed", package_sha256: "a".repeat(64), manager: "launchd",
      unit_sha256: "b".repeat(64), host_manifest_sha256: "c".repeat(64) },
    update: { update_id: "exact-stage", status: "staged", to_build_id: build.build_id, guide_version: 4,
      source_digest: build.source_digest, blockers: [], rollback_available: false } };
  const actions = [], props = { receipt, state, build, disabled: false, onAction: (action) => actions.push(action) };
  const html = renderToStaticMarkup(React.createElement(DeploymentUpdateActions, props));
  assert.match(html, /Apply and verify update/);
  assert.doesNotMatch(html, /Stop and confirm cleanup|Restore previous application|ready for your real jobs/);
  assert.deepEqual(actions, []);
  DeploymentUpdateActions(props).props.children[1].props.children[1].props.onClick();
  assert.deepEqual(actions, ["apply-update"]);
  assert.equal(DeploymentUpdateActions({ ...props, disabled: true }).props.children[1].props.children[1].props.disabled, true);

  const running = { ...receipt, status: "running", process_live: true, runtime_pid: 2345, service_cleanup_confirmed: false };
  const waiting = renderToStaticMarkup(React.createElement(DeploymentUpdateActions, { ...props, receipt: running }));
  assert.match(waiting, /Stop this background service and wait for confirmed native cleanup/);
  assert.doesNotMatch(waiting, /<button|Apply and verify update/);
  const unknownCleanup = { ...receipt, status: "interrupted", service_cleanup_confirmed: undefined };
  const cleanup = renderToStaticMarkup(React.createElement(DeploymentUpdateActions, { ...props, receipt: unknownCleanup }));
  assert.match(cleanup, /missing service PID alone does not confirm cleanup/);
  assert.match(cleanup, /Stop and confirm cleanup/);
  assert.doesNotMatch(cleanup, /Apply and verify update/);
  DeploymentUpdateActions({ ...props, receipt: unknownCleanup }).props.children[1].props.children[0].props.onClick();
  assert.deepEqual(actions, ["apply-update", "stop"]);

  const recovery = { ...receipt, update: { ...receipt.update, status: "recovery-needed", rollback_available: true } };
  const recoveryElement = DeploymentUpdateActions({ ...props, receipt: recovery });
  assert.match(renderToStaticMarkup(recoveryElement), /Retry current application/);
  recoveryElement.props.children[1].props.children[2].props.onClick();
  assert.deepEqual(actions, ["apply-update", "stop", "recover-current"]);
  const completed = { ...receipt, update: { ...receipt.update, status: "completed", rollback_available: true } };
  const restore = DeploymentUpdateActions({ ...props, receipt: completed });
  assert.match(renderToStaticMarkup(restore), /Restore previous application/);
  restore.props.children[1].props.children[3].props.onClick();
  assert.deepEqual(actions, ["apply-update", "stop", "recover-current", "rollback"]);
  const unverified = renderToStaticMarkup(React.createElement(DeploymentUpdateActions,
    { ...props, receipt: { ...receipt, update_capabilities: undefined } }));
  assert.doesNotMatch(unverified, /<button|Apply and verify update|Retry current application|Restore previous application/);
});
