import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { deploymentCanConfirmCleanup, deploymentNeedsCleanup, deploymentResumeAction, deploymentAwaitingApplicationSetup, deploymentPhase, deploymentFreshInstallNeedsSetup } from "./deployment-panel.mjs";

const guide = { id: "guide", version: 5 };
const receipt = { guide_id: guide.id, guide_version: 4, deployment_id: "installed-a", build_id: "build-a",
  status: "failed", execution_mode: "interactive", process_live: false, runtime_pid: null,
  package_sha256: "a".repeat(64), cleanup_recovery: { scope: "owned-interactive-cleanup", available: true,
    guide_version: 4, deployment_id: "installed-a", build_id: "build-a", package_sha256: "a".repeat(64), source_digest: "b".repeat(64) } };

const installedProduct = { ...receipt, cleanup_recovery: undefined, status: "stopped", phase: "configure", target: "local",
  health: "unknown", runtime_candidate: { format: "forge-meta-toolkit-runtime-selection-v1",
    assembly: { sha256: "c".repeat(64) }, model_catalog: { sha256: "d".repeat(64) },
    meta: { version: "0.9.0", sha256: "e".repeat(64) }, toolkit: { version: "0.1.0", sha256: "f".repeat(64) } },
  installed_candidate_runtime_proof: { source: "meta-toolkit-product", version: "0.9.0",
    assembly_sha256: "c".repeat(64), catalog_sha256: "d".repeat(64),
    sdk: { meta: { version: "0.9.0", sha256: "e".repeat(64) }, toolkit: { version: "0.1.0", sha256: "f".repeat(64) } } } };

const require = createRequire(import.meta.url);
const bundle = await build({ entryPoints: ["deployment-panel.jsx"], absWorkingDir: fileURLToPath(new URL(".", import.meta.url)),
  bundle: true, format: "esm", platform: "node", write: false, loader: { ".css": "empty" }, logLevel: "silent",
  plugins: [{ name: "shared-renderer-react", setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }] });
const { DeploymentCleanupRecovery, DeploymentResumeAction, DeploymentPrivateSettingsInstructions, DeploymentInstallDescription } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].contents).toString("base64")}`);

test("failed installed source requires explicit cleanup independently of newer candidate acceptance", () => {
  const before = structuredClone(receipt), actions = [];
  assert.equal(deploymentNeedsCleanup(receipt), true);
  assert.equal(deploymentCanConfirmCleanup(receipt, guide), true);
  const element = DeploymentCleanupRecovery({ receipt, state: guide, disabled: false, onAction: (value) => actions.push(value) });
  const html = renderToStaticMarkup(element);
  assert.match(html, /Stop and verify cleanup/);
  assert.match(html, /starting again is a separate action/);
  assert.doesNotMatch(html, /Retry|private-api|ready to use/);
  assert.deepEqual(actions, []);
  element.props.children[0].props.onClick();
  assert.deepEqual(actions, ["stop"]);
  assert.equal(DeploymentCleanupRecovery({ receipt, state: guide, disabled: true }).props.children[0].props.disabled, true);
  assert.deepEqual(receipt, before);
});

test("unconfirmed or differently bound cleanup never unlocks a retry or settings action", () => {
  for (const change of [{ available: false }, { scope: "another-scope" }, { deployment_id: "other" },
    { build_id: "other" }, { guide_version: 5 }, { guide_version: true }, { package_sha256: "c".repeat(64) },
    { source_digest: null }]) {
    const value = { ...receipt, cleanup_recovery: { ...receipt.cleanup_recovery, ...change } };
    assert.equal(deploymentCanConfirmCleanup(value, guide), false);
    const html = renderToStaticMarkup(React.createElement(DeploymentCleanupRecovery, { receipt: value, state: guide }));
    assert.match(html, /cleanup is unconfirmed/);
    assert.doesNotMatch(html, /<button/);
  }
  const unknown = { ...receipt, cleanup_recovery: { available: false, reason: "Unknown cleanup <script>private</script>" } };
  const html = renderToStaticMarkup(React.createElement(DeploymentCleanupRecovery, { receipt: unknown, state: guide }));
  assert.match(html, /&lt;script&gt;/);
  assert.doesNotMatch(html, /<script/);
});

test("live, background, foreign and transaction recovery scopes use their existing lifecycle controls", () => {
  for (const change of [{ execution_mode: "background" }, { service: { manager: "foreign" } },
    { status: "running" }, { status: "stopped" }, { update: { status: "restored-stopped" } },
    { update: { status: "candidate-restored-stopped" } }]) {
    const value = { ...receipt, ...change };
    assert.equal(deploymentNeedsCleanup(value), false);
    assert.equal(deploymentCanConfirmCleanup(value, guide), false);
    assert.equal(DeploymentCleanupRecovery({ receipt: value, state: guide }), null);
  }
  for (const change of [{ guide_id: "foreign" }, { process_live: true }, { runtime_pid: 123 }, { source_digest: "c".repeat(64) }])
    assert.equal(deploymentCanConfirmCleanup({ ...receipt, ...change }, guide), false);
  assert.equal(DeploymentCleanupRecovery({ receipt, state: { ...guide, id: "foreign" } }), null);
});

test("positive no-worker cancellation offers explicit installation retry rather than ordinary Start", () => {
  const stopped = { ...receipt, status: "stopped", destination: "/owned/partial-install", blueprint_hash: "c".repeat(64),
    startup_attempt_id: null, interactive_cleanup_attempt_id: null,
    interactive_lifecycle_id: "5c1f4b32-b1c9-4bd0-ab1a-332806ec52a8" };
  stopped.interactive_cleanup = { confirmed: true, worker_exit_confirmed: true, boundary: "worker-not-admitted", return_code: 0, supervisor_pid: 123,
    lifecycle_id: stopped.interactive_lifecycle_id, startup_attempt_id: null, source_digest: "d".repeat(64), owner_sha256: "e".repeat(64),
    ...Object.fromEntries(["deployment_id", "guide_id", "build_id", "guide_version", "blueprint_hash", "destination", "package_sha256"].map((key) => [key, stopped[key]])) };
  const before = structuredClone(stopped), actions = [];
  assert.equal(deploymentResumeAction(stopped, guide).action, "retry");
  const element = DeploymentResumeAction({ receipt: stopped, state: guide, onAction: (value) => actions.push(value) });
  const html = renderToStaticMarkup(element);
  assert.match(html, /Retry installation/);
  assert.match(html, /No application worker started/);
  assert.doesNotMatch(html, />Start installed application</);
  assert.deepEqual(actions, []);
  element.props.children[0].props.onClick();
  assert.deepEqual(actions, ["retry"]);
  assert.deepEqual(stopped, before);
  for (const change of [{ confirmed: false }, { lifecycle_id: "old" }, { package_sha256: "f".repeat(64) }, { return_code: -9 }])
    assert.equal(deploymentResumeAction({ ...stopped, interactive_cleanup: { ...stopped.interactive_cleanup, ...change } }, guide), null);
  assert.equal(deploymentResumeAction({ ...stopped, guide_id: "other" }, guide), null);
  assert.equal(deploymentResumeAction({ ...stopped, status: "running" }, guide), null);
  const product = { ...stopped, phase: "configure", runtime_candidate: { format: "forge-meta-toolkit-runtime-selection-v1" } };
  assert.equal(deploymentResumeAction(product, guide).action, "retry");
  assert.equal(deploymentAwaitingApplicationSetup(product, guide), false);
  assert.notEqual(deploymentPhase(product), "Your application is installed");
  const productHtml = renderToStaticMarkup(DeploymentResumeAction({ receipt: product, state: guide, onAction: () => {} }));
  assert.match(productHtml, /Retry installation/);
  assert.doesNotMatch(productHtml, /Installation complete|Application setup is next/);
});

test("selected product setup instructions do not point to a removed ordinary Start control", () => {
  const product = { ...receipt, runtime_candidate: { format: "forge-meta-toolkit-runtime-selection-v1" } };
  const before = structuredClone(product);
  const html = renderToStaticMarkup(DeploymentPrivateSettingsInstructions({ receipt: product }));
  assert.match(html, /keep their installed values/);
  assert.match(html, /stopped/);
  assert.match(html, /Complete application setup before starting/);
  assert.doesNotMatch(html, /Start installed application|<button/);
  assert.deepEqual(product, before);
  assert.match(renderToStaticMarkup(DeploymentPrivateSettingsInstructions({ receipt })), /Start installed application/);
});

test("a failed product installer with exact confirmed teardown offers guarded retry using saved settings", () => {
  const partial = { ...receipt, cleanup_recovery: undefined, status: "stopped", phase: "stop", target: "local",
    destination: "/owned/partial-install", blueprint_hash: "c".repeat(64), startup_attempt_id: null,
    interactive_cleanup_attempt_id: null, interactive_lifecycle_id: "5c1f4b32-b1c9-4bd0-ab1a-332806ec52a8",
    runtime_candidate: { format: "forge-meta-toolkit-runtime-selection-v1" } };
  partial.interactive_cleanup = { confirmed: true, worker_exit_confirmed: true,
    boundary: "owned-process-groups-absent", return_code: 1, supervisor_pid: 123,
    lifecycle_id: partial.interactive_lifecycle_id, startup_attempt_id: null,
    source_digest: "d".repeat(64), owner_sha256: "e".repeat(64),
    ...Object.fromEntries(["deployment_id", "guide_id", "build_id", "guide_version", "blueprint_hash", "destination", "package_sha256"].map(key => [key, partial[key]])) };
  const before = structuredClone(partial), actions = [];
  assert.equal(deploymentResumeAction(partial, guide)?.action, "retry");
  const element = DeploymentResumeAction({ receipt: partial, state: guide, onAction: value => actions.push(value) });
  const html = renderToStaticMarkup(element);
  assert.match(html, /Retry installation/);
  assert.match(html, /saved private settings/);
  assert.doesNotMatch(html, /No application worker started|>Start installed application<|Installation complete/);
  assert.deepEqual(actions, []);
  element.props.children[0].props.onClick();
  assert.deepEqual(actions, ["retry"]);
  assert.deepEqual(partial, before);
  for (const change of [{ confirmed: false }, { worker_exit_confirmed: false }, { return_code: 0 },
    { lifecycle_id: "old" }, { startup_attempt_id: "attempt" }, { source_digest: "bad" },
    { package_sha256: "f".repeat(64) }, { owner_sha256: null }, { supervisor_pid: 0 }, { guide_version: 5 }])
    assert.equal(deploymentResumeAction({ ...partial, interactive_cleanup: { ...partial.interactive_cleanup, ...change } }, guide), null);
  for (const change of [{ process_live: true }, { runtime_pid: 123 }, { startup_attempt_id: "attempt" },
    { interactive_cleanup_attempt_id: "attempt" }, { service: {} }, { execution_mode: "background" },
    { update: { status: "completed" } }, { guide_id: "other" }, { target: "gcp" },
    { installed_candidate_runtime_proof: {} }, { installed_candidate_runtime_proof: installedProduct.installed_candidate_runtime_proof }])
    assert.equal(deploymentResumeAction({ ...partial, ...change }, guide), null);
});

test("guarded product retry stays closed for a foreign boundary, a different source digest or a non-UUID lifecycle", () => {
  // The cleanup above, with a lifecycle id and the receipt's own source digest chosen per case.
  const failed = (lifecycle = "5c1f4b32-b1c9-4bd0-ab1a-332806ec52a8", receiptChange = {}, cleanupChange = {}) => {
    const partial = { ...receipt, cleanup_recovery: undefined, status: "stopped", phase: "stop", target: "local",
      destination: "/owned/partial-install", blueprint_hash: "c".repeat(64), startup_attempt_id: null,
      interactive_cleanup_attempt_id: null, interactive_lifecycle_id: lifecycle,
      runtime_candidate: { format: "forge-meta-toolkit-runtime-selection-v1" }, ...receiptChange };
    partial.interactive_cleanup = { confirmed: true, worker_exit_confirmed: true,
      boundary: "owned-process-groups-absent", return_code: 1, supervisor_pid: 123,
      lifecycle_id: lifecycle, startup_attempt_id: null, source_digest: "d".repeat(64), owner_sha256: "e".repeat(64),
      ...Object.fromEntries(["deployment_id", "guide_id", "build_id", "guide_version", "blueprint_hash", "destination", "package_sha256"].map(key => [key, partial[key]])),
      ...cleanupChange };
    return partial;
  };
  assert.equal(deploymentResumeAction(failed(), guide)?.action, "retry");
  assert.equal(deploymentResumeAction(failed(undefined, { source_digest: "d".repeat(64) }), guide)?.action, "retry",
    "The receipt's own source digest may name the cleaned source.");
  for (const boundary of ["foreign-boundary", "worker-not-admitted", "owned-process-groups-absent-unverified"])
    assert.equal(deploymentResumeAction(failed(undefined, {}, { boundary }), guide), null, boundary);
  assert.equal(deploymentResumeAction(failed(undefined, { source_digest: "f".repeat(64) }), guide), null,
    "A cleanup of another source cannot retry this installation.");
  for (const lifecycle of ["lifecycle-1", "5C1F4B32-B1C9-4BD0-AB1A-332806EC52A8", "5c1f4b32-b1c9-4bd0-ab1a-332806ec52a8-extra", ""])
    assert.equal(deploymentResumeAction(failed(lifecycle), guide), null, lifecycle);
});

test("initial install explains the host-selected Configure step without claiming agents have started", () => {
  const options = { source_kind: "application", fresh_install: { available: true, next_step: "configure" } };
  const before = structuredClone(options);
  assert.equal(deploymentFreshInstallNeedsSetup(options), true);
  const html = renderToStaticMarkup(DeploymentInstallDescription({ nativeTeam: false, options }));
  assert.match(html, /Application setup follows/);
  assert.match(html, /agents do not start yet/);
  assert.doesNotMatch(html, /starts the proposed agents|<button/);
  assert.deepEqual(options, before);
  for (const value of [null, {}, { fresh_install: { available: false, next_step: "configure" } },
    { fresh_install: { available: true, next_step: "start" } }, { source_kind: "native-team", fresh_install: options.fresh_install },
    { source_kind: "unknown", fresh_install: options.fresh_install }]) {
    assert.equal(deploymentFreshInstallNeedsSetup(value), false);
  }
  assert.match(renderToStaticMarkup(DeploymentInstallDescription({ nativeTeam: false, options: {} })), /starts the proposed agents/);
  assert.match(renderToStaticMarkup(DeploymentInstallDescription({ nativeTeam: true, options: {} })), /accepted plan/);
});

test("installed selected product explains setup without offering ordinary startup", () => {
  const installed = structuredClone(installedProduct);
  const before = structuredClone(installed), actions = [];
  assert.equal(deploymentAwaitingApplicationSetup(installed, guide), true);
  assert.equal(deploymentResumeAction(installed, guide), null);
  assert.equal(deploymentPhase(installed), "Your application is installed");
  const html = renderToStaticMarkup(DeploymentResumeAction({ receipt: installed, state: guide,
    confirmed: true, onAction: (value) => actions.push(value) }));
  assert.match(html, /Application setup is next/);
  assert.match(html, /Installation<\/dt><dd>Checked/);
  assert.match(html, /Checking the installation and its saved settings/);
  assert.match(html, /Nothing is running yet/);
  assert.match(html, /<button[^>]*disabled[^>]*>Checking setup/);
  assert.doesNotMatch(html, /Set up and start application|Start installed|ready to use|SDK|policy|runtime-selection/);
  assert.deepEqual(actions, []);
  assert.deepEqual(installed, before);
  for (const change of [{ guide_id: "other" }, { status: "running" }, { phase: "stop" }, { runtime_pid: 123 },
    { process_live: true }, { process_live: undefined }, { deployment_id: "" }, { build_id: undefined },
    { package_sha256: "unknown" }, { execution_mode: "background" }, { target: "gcp" }, { source_kind: "native-team" }, { source_kind: "unknown" },
    { update: { status: "restored-stopped" } }, { runtime_candidate: { format: "forge-runtime-candidate-selection-v1" } }])
    assert.equal(deploymentAwaitingApplicationSetup({ ...installed, ...change }, guide), false);
});

test("a settings write failure or later health reset cannot claim an unfinished runtime installation completed", () => {
  // Same-install settings failure retains stopped/Configure after source materialization;
  // a later settings save resets health without installing the selected runtime bytes.
  for (const health of ["failed", "unknown"]) {
    const incomplete = { ...installedProduct, health, installed_candidate_runtime_proof: undefined,
      interactive_cleanup: { confirmed: true, worker_exit_confirmed: true, boundary: "owned-process-groups-absent" } };
    const before = structuredClone(incomplete), actions = [];
    assert.equal(deploymentAwaitingApplicationSetup(incomplete, guide), false);
    assert.notEqual(deploymentPhase(incomplete), "Your application is installed");
    assert.equal(DeploymentResumeAction({ receipt: incomplete, state: guide, onAction: (value) => actions.push(value) }), null);
    assert.deepEqual(actions, []);
    assert.deepEqual(incomplete, before);
  }
});

test("installation completion belongs to the selected assembly, catalogue and both SDKs", () => {
  for (const change of [{ source: "signed-candidate" }, { version: "other" }, { assembly_sha256: "0".repeat(64) },
    { catalog_sha256: "0".repeat(64) }, { sdk: undefined },
    { sdk: { ...installedProduct.installed_candidate_runtime_proof.sdk, meta: { version: "0.9.0", sha256: "0".repeat(64) } } },
    { sdk: { ...installedProduct.installed_candidate_runtime_proof.sdk, toolkit: { version: "other", sha256: "f".repeat(64) } } }]) {
    const changed = { ...installedProduct, installed_candidate_runtime_proof: { ...installedProduct.installed_candidate_runtime_proof, ...change } };
    assert.equal(deploymentAwaitingApplicationSetup(changed, guide), false);
    assert.equal(DeploymentResumeAction({ receipt: changed, state: guide }), null);
  }
});

test("product restart affordances stay blocked after stop or source recovery while legacy startup stays available", () => {
  const selected = { ...receipt, cleanup_recovery: undefined, status: "stopped", phase: "stop",
    runtime_candidate: { format: "forge-meta-toolkit-runtime-selection-v1" } };
  for (const update of [undefined, { status: "restored-stopped" }, { status: "candidate-restored-stopped" }])
    assert.equal(deploymentResumeAction({ ...selected, update }, guide), null);
  assert.equal(deploymentResumeAction({ ...selected, runtime_candidate: { format: "forge-runtime-candidate-selection-v1" } }, guide).action, "restart");
  assert.equal(deploymentResumeAction({ ...selected, runtime_candidate: undefined }, guide).action, "restart");
  assert.equal(deploymentResumeAction({ ...selected, status: "failed" }, guide), null);
});


function restoredBackgroundApplication(manager = "systemd-user") {
  const deploymentId = "11111111-1111-4111-8111-111111111111";
  const directory = `/synthetic-private/deployments/${guide.id}/${deploymentId}`;
  return { ...structuredClone(installedProduct), deployment_id: deploymentId, execution_mode: "background", phase: "stop",
    service_supervisor_pid: 123, service_cleanup_confirmed: true,
    service: { manager, label: `ai.mobkit.launch.${deploymentId}`, receipt_directory: directory,
      unit_sha256: "9".repeat(64), bootstrap_selection: { path: `${directory}/bootstrap-admission-original.json`, sha256: "8".repeat(64) },
      public_summary: { manager, definition_verified: true, state: "stopped", pid: null } },
    service_observation: { owned: true, loaded: false, state: "stopped", pid: null, control_pid: null, control_group: null } };
}

test("selected background Restore restart renders the existing explicit action for both managers", () => {
  for (const manager of ["launchd", "systemd-user"]) {
    const installed = restoredBackgroundApplication(manager), before = structuredClone(installed), actions = [];
    assert.equal(deploymentAwaitingApplicationSetup(installed, guide), false);
    assert.equal(deploymentResumeAction(installed, guide).action, "restart");
    const element = DeploymentResumeAction({ receipt: installed, state: guide, disabled: false,
      onAction: action => actions.push(action) });
    const html = renderToStaticMarkup(element);
    assert.match(html, />Start installed application</);
    assert.doesNotMatch(html, /Prepare application setup|Set up and start|Run in background|bootstrap-admission|11111111/);
    assert.deepEqual(actions, [], "Rendering never admits or restarts the application.");
    element.props.children[0].props.onClick();
    assert.deepEqual(actions, ["restart"], "The explicit action delegates to the original canonical command owner.");
    const paused = DeploymentResumeAction({ receipt: installed, state: guide, disabled: true, onAction: action => actions.push(action) });
    assert.equal(paused.props.children[0].props.disabled, true, "Parent reconnect, pending and source guards still pause the action.");
    assert.match(renderToStaticMarkup(paused), /<button[^>]*disabled[^>]*>Start installed application/);
    assert.deepEqual(installed, before);
  }
});

test("selected background Restore restart rejects non-Restore and uncertain service projections", () => {
  const original = restoredBackgroundApplication();
  const cases = [
    r => { delete r.service.bootstrap_selection; },
    r => { r.service.bootstrap_selection.sha256 = "unknown"; },
    r => { r.service.bootstrap_selection.path = "/other-install/bootstrap-admission-original.json"; },
    r => { r.service.label = "ai.mobkit.launch.foreign"; },
    r => { r.service.public_summary.definition_verified = false; },
    r => { r.service_observation.owned = false; },
    r => { r.service_observation.state = r.service.public_summary.state = "unknown"; },
    r => { r.service_observation.control_group = "/synthetic-live-group"; },
    r => { r.service_cleanup_confirmed = false; },
    r => { r.process_live = true; },
    r => { delete r.installed_candidate_runtime_proof; },
    r => { r.installed_candidate_runtime_proof.catalog_sha256 = "0".repeat(64); },
    r => { r.guide_id = "another-guide"; },
    r => { r.target = "gcp"; },
  ];
  for (const change of cases) {
    const installed = structuredClone(original), actions = [];
    change(installed);
    const before = structuredClone(installed);
    assert.equal(deploymentResumeAction(installed, guide), null);
    assert.equal(DeploymentResumeAction({ receipt: installed, state: guide, onAction: action => actions.push(action) }), null);
    assert.deepEqual(actions, []);
    assert.deepEqual(installed, before);
  }
  const interactive = { ...original, execution_mode: "interactive", service: undefined };
  assert.equal(deploymentResumeAction(interactive, guide), null);
  const html = renderToStaticMarkup(DeploymentResumeAction({ receipt: interactive, state: guide, confirmed: true, onAction: () => {} }));
  assert.match(html, /Application setup is next/);
  assert.doesNotMatch(html, />Start installed application</);
  const ordinary = { ...original, runtime_candidate: undefined, installed_candidate_runtime_proof: undefined };
  assert.equal(deploymentResumeAction(ordinary, guide).action, "restart", "The existing ordinary lifecycle remains unchanged.");
});
