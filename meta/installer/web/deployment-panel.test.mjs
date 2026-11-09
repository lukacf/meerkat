import assert from "node:assert/strict";
import test from "node:test";
import { deploymentCanPrepare, deploymentCanInstall, deploymentCanFreshInstall, deploymentFreshInstallRestriction, deploymentCanSubmit, deploymentConsole, deploymentIsActive,
  deploymentPhase, deploymentDiagnostic, deploymentSettings, deploymentHandoff, deploymentOptionsCurrent,
  deploymentExperienceKey, deploymentExperiences, createDeploymentPollingGate,
  deploymentCanConfigure, deploymentCanStopFailedBackground, deploymentConfigurationForm, deploymentConfigurationHints,
  deploymentInstalledSettingsCurrent, deploymentReconfiguration, deploymentSettingsReadKey,
  deploymentSettingsReadCurrent, deploymentSettingsResponseCurrent, readDeploymentSettings,
  deploymentConfigurationRevealCurrent, nativeTeamCanInstall, deploymentInstalledSourceKind, deploymentSourceKey, deploymentInstallSelection } from "./deployment-panel.mjs";
import { deploymentUpdatePreparation, deploymentBackgroundUpdate, deploymentUpdateAction, deploymentCanStageUpdate,
  deploymentUpdateOwnerStart } from "./deployment-panel.mjs";
import * as panel from "./deployment-panel.mjs";

const guide = { id: "guide", version: 4, proposal: { target: "mac" } };
const accepted = { build_id: "build", guide_id: guide.id, guide_version: 4, status: "completed", ready_to_package: true,
  current: true, acceptance_current: true, acceptance_trusted: true, acceptance: { status: "pass" } };

const nativeGuide = { id: guide.id, version: 4, status: "ready", blueprint: { target: "mac" },
  proposal: null, question: null, readiness: { ready: true }, brief: { purpose: "Synthetic study" } };
const nativeOptions = { source_kind: "native-team", source_id: "c".repeat(64), source_digest: "d".repeat(64), build_id: null,
  guide_version: 4, current_guide_version: 4, current: true, target: "mac", host_target: "mac", supported: true,
  default_destination: "/owned/native-team", console_port: 18088, fields: [],
  execution_modes: [{ id: "interactive", available: true }] };

test("native installation needs the accepted ready guide and authentic source options, without application acceptance", () => {
  assert.equal(nativeTeamCanInstall(nativeGuide), true);
  assert.equal(deploymentCanPrepare(null, nativeGuide, "native-team"), true);
  assert.equal(deploymentCanInstall(null, nativeGuide, nativeOptions), true);
  assert.deepEqual(deploymentInstallSelection(nativeOptions, null, nativeGuide), {
    source_kind: "native-team", source_id: nativeOptions.source_id, source_digest: nativeOptions.source_digest });
  assert.equal(deploymentCanPrepare(null, nativeGuide), false);
  for (const change of [{ status: "review" }, { proposal: nativeGuide.blueprint }, { question: { id: "pending" } },
    { readiness: { ready: false } }, { operation: { status: "running" } }, { blueprint: null }, { blueprint: { target: "gcp" } }])
    assert.equal(deploymentCanInstall(accepted, { ...nativeGuide, ...change }, nativeOptions), false);
  for (const change of [{ build_id: accepted.build_id }, { source_id: "bad" }, { source_digest: "bad" },
    { guide_version: 3 }, { current_guide_version: 5 }, { current: false }, { target: "linux" }])
    assert.equal(deploymentCanInstall(accepted, nativeGuide, { ...nativeOptions, ...change }), false);
  assert.notEqual(deploymentSourceKey(accepted, nativeGuide, "native-team"), deploymentSourceKey(accepted, nativeGuide));
  assert.notEqual(deploymentSourceKey(null, nativeGuide, "native-team"),
    deploymentSourceKey(null, { ...nativeGuide, brief: { purpose: "Changed" } }, "native-team"));
});

test("an installed source choice belongs to its matching receipt and does not imply readiness", () => {
  const application = { guide_id: guide.id, deployment_id: "owned", build_id: "installed", status: "stopped" };
  assert.equal(deploymentInstalledSourceKind(application, guide.id), "application");
  assert.equal(deploymentInstalledSourceKind(null, null), null);
  assert.equal(deploymentConsole(application), null);
  assert.equal(deploymentInstalledSourceKind({ ...application, source_kind: "application" }, guide.id), "application");
  assert.equal(deploymentInstalledSourceKind({ ...application, guide_id: "other" }, guide.id), null);
  assert.equal(deploymentInstalledSourceKind({ ...application, deployment_id: null }, guide.id), null);
  assert.equal(deploymentInstalledSourceKind({ ...application, source_kind: "unknown" }, guide.id), null);
  assert.equal(deploymentInstalledSourceKind({ ...application, build_id: null }, guide.id), null);
  const native = { ...application, ...nativeOptions };
  assert.equal(deploymentInstalledSourceKind(native, guide.id), "native-team");
  assert.equal(deploymentInstalledSourceKind({ ...native, source_id: "bad" }, guide.id), null);
});

test("native installed settings stay receipt-owned across changed guide selection while application updates remain unavailable", () => {
  const receipt = { guide_id: guide.id, guide_version: 4, deployment_id: "native-install", ...nativeOptions,
    package_sha256: "a".repeat(64), destination: "/owned/native-team", execution_mode: "interactive", status: "stopped", process_live: false };
  const laterGuide = { ...nativeGuide, version: 5, status: "refining", proposal: { target: "linux" }, readiness: { ready: false } };
  const options = { ...nativeOptions, scope: "verified-installed-private-configuration", version: 5,
    deployment_id: receipt.deployment_id, package_sha256: receipt.package_sha256, source_verified: true,
    requires_stopped: true, destination: receipt.destination, execution_mode: "interactive" };
  assert.equal(deploymentCanConfigure(receipt, null, laterGuide, options), true);
  assert.equal(deploymentCanConfigure(receipt, null, laterGuide, { ...options, source_id: "f".repeat(64) }), false);
  assert.equal(deploymentConfigurationForm(options, receipt, null, laterGuide, "settings").destination, receipt.destination);
  assert.equal(deploymentCanStageUpdate(receipt, accepted, guide, privateOptions), false);
  assert.equal(deploymentUpdateAction({ ...receipt, update: { update_id: "fake", rollback_available: true } }, accepted, guide, "rollback"), null);
  assert.equal(deploymentExperienceKey(receipt, guide.id), "");
});

const privateOptions = { build_id: accepted.build_id, guide_version: accepted.guide_version, current: true,
  target: "mac", host_target: "mac", supported: true, default_destination: "/owned/agents", console_port: 18088,
  configured_deployment_id: "owned-installation", configured_destination: "/owned/agents",
  fields: [{ key: "OPENAI_API_KEY", label: "Provider key", required: true, secret: true, configured: true },
    { key: "APP_USER", label: "Application user", required: true, secret: false, configured: true }],
  execution_modes: [{ id: "interactive", available: true }, { id: "background", available: true }] };
const configurable = { ...accepted, deployment_id: "owned-installation", destination: "/owned/agents",
  status: "stopped", execution_mode: "background", package_sha256: "a".repeat(64), process_live: false,
  service_observation: { owned: true, loaded: false, state: "stopped" } };
const installedOptions = { ...privateOptions, scope: "verified-installed-private-configuration", version: guide.version,
  deployment_id: configurable.deployment_id, package_sha256: configurable.package_sha256,
  source_digest: "b".repeat(64), source_verified: true, requires_stopped: true,
  destination: configurable.destination, execution_mode: configurable.execution_mode };

test("failed background Stop is independent of proposal currency and application update protocol", () => {
  const receipt = { ...configurable, status: "failed", current: false, guide_version: 3,
    update_capabilities: { background_activation_supported: false },
    service_observation: { owned: true, loaded: true, state: "waiting", pid: null },
    service: { manager: "launchd", public_summary: {
      manager: "launchd", definition_verified: true, state: "waiting", pid: null } } };
  const native = { ...receipt, source_kind: "native-team", build_id: null,
    source_id: "c".repeat(64), source_digest: "d".repeat(64) };
  for (const source of [receipt, native]) {
    assert.equal(deploymentCanStopFailedBackground(source, guide), true);
    assert.equal(deploymentCanStopFailedBackground({ ...source, status: "interrupted" }, guide), true);
    assert.equal(deploymentCanConfigure(source, null, guide), false, "Stop eligibility does not prove cleanup.");
    const linux = { ...source, service_observation: { owned: true, loaded: true, state: "failed", pid: null },
      service: { manager: "systemd-user", public_summary: {
        manager: "systemd-user", definition_verified: true, state: "failed", pid: null } } };
    assert.equal(deploymentCanStopFailedBackground(linux, guide), true);
  }
});

test("a background service whose own files are in a protected folder keeps its status note", () => {
  // Compare strings only: the note is plain projected text.
  const note = "macOS does not let a background login item read Desktop, Documents, Downloads, iCloud Drive or " +
    "cloud storage folders. This service runs only while this computer lets background login items read them, " +
    "for example with Full Disk Access.";
  const receipt = { ...configurable, status: "running", protected_folder: note,
    service: { manager: "launchd", public_summary: {
      manager: "launchd", definition_verified: true, state: "running", pid: 4567 } } };
  const read = value => panel.deploymentProtectedFolderNote?.(value) ?? null;
  assert.equal(read(receipt), note);
  assert.equal(read({ ...receipt, protected_folder: undefined }), null);
  assert.equal(read({ ...receipt, protected_folder: "x".repeat(501) }), null);
  assert.equal(read({ ...receipt, execution_mode: "interactive" }), null);
  assert.equal(read({ ...receipt, service: { ...receipt.service, manager: "systemd-user" } }), null);
});

test("failed background Stop refuses foreign, unknown or inconsistent ownership without borrowing update support", () => {
  const receipt = { ...configurable, status: "failed",
    service_observation: { owned: true, loaded: true, state: "waiting", pid: null },
    service: { manager: "launchd", public_summary: {
      manager: "launchd", definition_verified: true, state: "waiting", pid: null } } };
  for (const changed of [
    { ...receipt, guide_id: "other-guide" }, { ...receipt, execution_mode: "interactive" },
    { ...receipt, target: "gcp" }, { ...receipt, service: null },
    { ...receipt, service_observation: { ...receipt.service_observation, owned: false } },
    { ...receipt, service_observation: { owned: true, loaded: true, state: "unknown", pid: null },
      service: { manager: "launchd", public_summary: {
        manager: "launchd", definition_verified: true, state: "unknown", pid: null } } },
    { ...receipt, service: { ...receipt.service, public_summary: {
      ...receipt.service.public_summary, definition_verified: false } } },
    { ...receipt, service: { ...receipt.service, public_summary: {
      ...receipt.service.public_summary, manager: "systemd-user" } } },
    { ...receipt, service: { ...receipt.service, public_summary: {
      ...receipt.service.public_summary, state: "running" } } },
    { ...receipt, service: { ...receipt.service, public_summary: {
      ...receipt.service.public_summary, pid: 456 } } },
    { ...receipt, source_kind: "native-team", build_id: null, source_id: "unbound" },
  ]) assert.equal(deploymentCanStopFailedBackground(changed, guide), false);
});

test("failed background cleanup with update history defers only to a verified alternate update Stop", () => {
  const receipt = { ...configurable, status: "failed",
    service_observation: { owned: true, loaded: true, state: "waiting", pid: null },
    service: { manager: "launchd", unit_sha256: "f".repeat(64), public_summary: {
      manager: "launchd", definition_verified: true, state: "waiting", pid: null } } };
  const supported = { background_activation_supported: true, requires_stopped: true,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id, package_sha256: receipt.package_sha256,
    manager: "launchd", unit_sha256: receipt.service.unit_sha256, host_manifest_sha256: "c".repeat(64) };
  for (const status of ["completed", "staged"]) {
    const updated = { ...receipt, update: { status } };
    assert.equal(deploymentCanStopFailedBackground(updated, guide), true, "Unverified update history supplies no other Stop.");
    assert.equal(deploymentCanStopFailedBackground({ ...updated,
      update_capabilities: { ...supported, background_activation_supported: false } }, guide), true);
    assert.equal(deploymentCanStopFailedBackground({ ...updated, update_capabilities: supported }, guide), false,
      "The verified update route already supplies its single explicit Stop.");
  }
  assert.equal(deploymentCanStopFailedBackground({ ...receipt, update_capabilities: supported }, guide), true,
    "Update support without an update does not itself render another Stop.");
});

test("changing private settings keeps the exact owned folder and lifecycle without reading private values", () => {
  const receipt = { ...configurable, logs: [{ text: "redacted historical output" }], private_settings: { APP_USER: "must-not-copy" } };
  const before = structuredClone({ options: installedOptions, receipt });
  assert.equal(deploymentCanConfigure(receipt, accepted, guide, installedOptions), true);
  const form = deploymentConfigurationForm(installedOptions, receipt, accepted, guide, "settings");
  assert.equal(form.intent, "settings");
  assert.equal(form.destination, receipt.destination);
  assert.equal(form.executionMode, "background");
  assert.deepEqual(form.values, {});
  assert.ok(deploymentReconfiguration(form.options, receipt, accepted, guide, form.values));
  assert.deepEqual(deploymentSettings(form.options, form.values), {});
  assert.deepEqual(deploymentSettings(form.options, { APP_USER: "replacement", UNDECLARED_SECRET: "not-requested" }), { APP_USER: "replacement" });
  assert.equal(JSON.stringify(form).includes("must-not-copy"), false);
  assert.equal(JSON.stringify(form).includes("redacted historical output"), false);
  assert.deepEqual({ options: installedOptions, receipt }, before);
});

test("another-folder setup starts separately and requires its own private settings", () => {
  const form = deploymentConfigurationForm(privateOptions, configurable, accepted, guide, "new");
  assert.equal(form.destination, "/owned/agents-new");
  assert.equal(form.executionMode, "interactive");
  assert.deepEqual(form.values, { MOBKIT_PORT: "18088" });
  assert.equal(form.options.fields.every((field) => field.configured === false), true);
  assert.equal(deploymentCanSubmit(form.options, form.values, false), false);
  assert.deepEqual(deploymentSettings(form.options, { ...form.values, OPENAI_API_KEY: "synthetic-private-key", APP_USER: "new-user" }),
    { OPENAI_API_KEY: "synthetic-private-key", APP_USER: "new-user", MOBKIT_PORT: "18088" });
  assert.equal(deploymentCanSubmit(form.options, { ...form.values, OPENAI_API_KEY: "synthetic-private-key", APP_USER: "new-user" }, false), true);
  assert.equal(privateOptions.fields.every((field) => field.configured === true), true);
  assert.equal(deploymentConfigurationForm({ ...privateOptions, default_destination: "/owned/fresh" }, configurable, accepted, guide, "new").destination, "/owned/fresh");
});

test("reconfiguration requires exact stopped installed source and cannot change its lifecycle", () => {
  for (const change of [{ guide_id: "other" }, { build_id: "other-installed-source" }, { guide_version: 3 },
    { deployment_id: null }, { destination: null }, { update: { status: "recovery-needed" } }]) {
    const receipt = { ...configurable, ...change };
    assert.equal(deploymentCanConfigure(receipt, accepted, guide, installedOptions), false);
    assert.equal(deploymentConfigurationForm(installedOptions, receipt, accepted, guide, "settings"), null);
  }
  assert.equal(deploymentConfigurationForm(installedOptions, { ...configurable, status: "running" }, accepted, guide, "settings"), null);
  assert.equal(deploymentConfigurationForm({ ...installedOptions, execution_mode: "interactive" }, configurable, accepted, guide, "settings"), null);
  assert.equal(deploymentConfigurationForm(privateOptions, configurable, accepted, guide, "settings"), null);
  assert.equal(deploymentConfigurationForm(privateOptions, configurable, accepted, guide, "unknown"), null);
  const retained = { ...configurable, guide_version: 3, current: false };
  const retainedBuild = { build_id: "new-unaccepted-build", status: "failed" };
  const retainedOptions = { ...installedOptions, guide_version: 3 };
  assert.equal(deploymentConfigurationForm(retainedOptions, retained, retainedBuild, guide, "settings").destination, configurable.destination);
  for (const change of [{ process_live: true }, { runtime_pid: 123 }, { service_observation: { owned: false } },
    { service_observation: { owned: true, loaded: true, state: "running", pid: 123 } },
    { service_supervisor_pid: 123, service_cleanup_confirmed: false }])
    assert.equal(deploymentConfigurationForm(installedOptions, { ...configurable, ...change }, accepted, guide, "settings"), null);
});

test("another tab's same-build private hints cannot attach to this tab's selected installation", () => {
  const second = { ...configurable, deployment_id: "second-installation", destination: "/owned/second-folder" };
  const secondOptions = { ...installedOptions, deployment_id: second.deployment_id, destination: second.destination };
  const before = structuredClone({ options: secondOptions, receipt: configurable });
  const mismatched = deploymentConfigurationForm(secondOptions, configurable, accepted, guide, "settings");
  assert.equal(mismatched, null);
  const matching = deploymentConfigurationForm(secondOptions, second, accepted, guide, "settings");
  assert.equal(matching.options.fields.every((field) => field.configured === true), true);
  assert.ok(deploymentReconfiguration(matching.options, second, accepted, guide, {}));
  assert.deepEqual({ options: secondOptions, receipt: configurable }, before);
});

test("open-form hints lose their binding when polling selects another installation or destination", () => {
  const form = deploymentConfigurationForm(installedOptions, configurable, accepted, guide, "settings");
  assert.equal(deploymentConfigurationHints(form.options, configurable, form.destination, true).fields.every((field) => field.configured), true);
  for (const receipt of [{ ...configurable, deployment_id: "new-selected-installation" },
    { ...configurable, destination: "/owned/another-folder" }, { ...configurable, build_id: "other-source" },
    { ...configurable, guide_version: 3 }]) {
    const hints = deploymentConfigurationHints(form.options, receipt, form.destination, true);
    assert.equal(hints.fields.every((field) => field.configured === false), true);
    assert.equal(deploymentCanSubmit(hints, {}, false, form.executionMode), false);
  }
  assert.equal(deploymentConfigurationHints(form.options, configurable, "/owned/another-folder", true).fields.every((field) => field.configured === false), true);
  assert.equal(deploymentConfigurationHints(form.options, configurable, form.destination, false).fields.every((field) => field.configured === false), true);
});

test("legacy or malformed configured-presence provenance cannot skip required private fields", () => {
  for (const change of [{ configured_deployment_id: undefined }, { configured_destination: undefined },
    { configured_deployment_id: null }, { configured_destination: "" }, { configured_deployment_id: 1 },
    { configured_destination: "/owned/other" }, { guide_version: true }, { build_id: null }]) {
    const hints = deploymentConfigurationHints({ ...privateOptions, ...change }, configurable, configurable.destination, true);
    assert.equal(hints.fields.every((field) => field.configured === false), true);
    assert.equal(deploymentCanSubmit(hints, {}, false, "background"), false);
  }
  assert.equal(deploymentConfigurationHints(null, configurable, configurable.destination, true), null);
});

test("a failed newer candidate cannot redirect installed private settings or borrow its acceptance", () => {
  const build = { build_id: "new-failed-candidate", guide_id: guide.id, status: "failed", current: true,
    acceptance_current: false, acceptance_trusted: false, ready_to_package: false };
  const receipt = { ...configurable, current: false };
  const before = structuredClone({ build, receipt, installedOptions });
  const form = deploymentConfigurationForm(installedOptions, receipt, build, guide, "settings");
  assert.ok(form);
  assert.equal(form.destination, receipt.destination);
  assert.deepEqual(form.values, {});
  const body = deploymentReconfiguration(form.options, receipt, build, guide, { APP_USER: "replacement" });
  assert.deepEqual(body, { version: guide.version, deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    source_digest: installedOptions.source_digest, package_sha256: receipt.package_sha256,
    settings: { APP_USER: "replacement" }, remove_fields: [], use_host_provider_credentials: false });
  assert.equal(JSON.stringify(body).includes(build.build_id), false);
  assert.equal(deploymentCanInstall(build, guide, privateOptions), false);
  assert.equal(deploymentConfigurationForm(privateOptions, receipt, build, guide, "new"), null);
  assert.deepEqual({ build, receipt, installedOptions }, before);
});

test("installed settings refuse wrong source, read version, package and pending guide or candidate work", () => {
  for (const change of [{ source_verified: false }, { source_verified: undefined }, { scope: "declared-source" },
    { supported: false }, { requires_stopped: false }, { deployment_id: "other" }, { build_id: "candidate" },
    { package_sha256: "c".repeat(64) }, { source_digest: "not-a-digest" }, { version: guide.version + 1 },
    { version: true }, { guide_version: 3 }, { execution_mode: "interactive" }, { destination: "/another/folder" }]) {
    assert.equal(deploymentInstalledSettingsCurrent({ ...installedOptions, ...change }, configurable, guide), false);
    assert.equal(deploymentReconfiguration({ ...installedOptions, ...change }, configurable, accepted, guide, {}), null);
  }
  assert.equal(deploymentCanConfigure({ ...configurable, source_digest: "c".repeat(64) }, accepted, guide, installedOptions), false);
  for (const status of ["queued", "running", "checking", "unknown", undefined])
    assert.equal(deploymentConfigurationForm(installedOptions, configurable, { build_id: "candidate", status }, guide, "settings"), null);
  for (const key of ["cleanup_confirmed", "worker_stop_confirmed"]) {
    for (const value of [false, null, undefined, "true"])
      assert.equal(deploymentConfigurationForm(installedOptions, configurable, { ...accepted, [key]: value }, guide, "settings"), null);
    assert.ok(deploymentConfigurationForm(installedOptions, configurable, { ...accepted, [key]: true }, guide, "settings"));
  }
  assert.ok(deploymentConfigurationForm(installedOptions, configurable, null, guide, "settings"));
  assert.equal(deploymentCanConfigure(configurable, accepted, { ...guide, operation: { status: "running" } }), false);
});

test("explicit removal submits only saved optional declared fields and never required or automatic keys", () => {
  const options = { ...installedOptions, fields: [...installedOptions.fields,
    { key: "OPTIONAL_TOKEN", label: "Adapter token", secret: true, required: false, configured: true, removable: true },
    { key: "AUTOMATIC_TOKEN", label: "Generated token", secret: true, required: false, configured: true, removable: true, automatic: true },
    { key: "UNCONFIGURED_OPTION", label: "Unused", required: false, configured: false, removable: true }] };
  const body = deploymentReconfiguration(options, configurable, accepted, guide, { APP_USER: "replacement" }, ["OPTIONAL_TOKEN"]);
  assert.deepEqual(body.remove_fields, ["OPTIONAL_TOKEN"]);
  assert.deepEqual(body.settings, { APP_USER: "replacement" });
  assert.deepEqual(deploymentReconfiguration(options, configurable, accepted, guide, {}).remove_fields, []);
  for (const removal of [["APP_USER"], ["OPENAI_API_KEY"], ["AUTOMATIC_TOKEN"], ["UNCONFIGURED_OPTION"],
    ["UNDECLARED_KEY"], ["MOBKIT_PORT"], ["OPTIONAL_TOKEN", "OPTIONAL_TOKEN"], null])
    assert.equal(deploymentReconfiguration(options, configurable, accepted, guide, {}, removal), null);
  assert.equal(deploymentReconfiguration(options, configurable, accepted, guide, { OPTIONAL_TOKEN: "replacement" }, ["OPTIONAL_TOKEN"]), null);
  assert.equal(deploymentReconfiguration({ ...options, host_provider_keys: ["OPTIONAL_TOKEN"], can_use_host_provider_credentials: true },
    configurable, accepted, guide, {}, ["OPTIONAL_TOKEN"], true), null);
  assert.equal(deploymentReconfiguration(options, configurable, accepted, guide, {}, [], true), null);
});

test("the actual installed settings read holds exact scope across delayed guide, source and ownership changes", async () => {
  const selected = { state: guide, build: { build_id: "failed-candidate", status: "failed" }, receipt: configurable };
  let current = selected;
  const urls = [];
  assert.equal(await readDeploymentSettings(selected, async (url) => { urls.push(url); return installedOptions; }, () => current), installedOptions);
  assert.deepEqual(urls, ["/api/guide/guide/deployment/settings?deployment_id=owned-installation"]);
  for (const changed of [
    { ...selected, state: { ...guide, id: "other" } }, { ...selected, state: { ...guide, version: guide.version + 1 } },
    { ...selected, state: { ...guide, operation: { status: "running" } } },
    { ...selected, build: { build_id: "other-candidate", status: "failed" } },
    { ...selected, build: { ...selected.build, source_digest: "c".repeat(64) } },
    { ...selected, build: { ...selected.build, status: "checking" } }, { ...selected, blocked: true },
    { ...selected, receipt: { ...configurable, deployment_id: "other-installation" } },
    { ...selected, receipt: { ...configurable, status: "running" } },
    { ...selected, receipt: { ...configurable, package_sha256: "c".repeat(64) } },
  ]) {
    const wait = delayed(); current = selected;
    const response = readDeploymentSettings(selected, () => wait.promise, () => current);
    current = changed; wait.resolve(installedOptions);
    assert.equal(await response, null);
    assert.equal(deploymentSettingsReadCurrent(selected, current, installedOptions), false);
  }
  // Also fence the final publication after the read promise has returned.
  current = selected;
  const options = await readDeploymentSettings(selected, async () => installedOptions, () => current);
  current = { ...selected, receipt: { ...configurable, status: "starting" } };
  assert.equal(deploymentSettingsReadCurrent(selected, current, options), false);
  let called = false;
  assert.equal(await readDeploymentSettings({ ...selected, blocked: true }, async () => { called = true; }, () => selected), null);
  assert.equal(called, false);
});

test("a delayed settings save cannot publish over a changed source, setup, process or admission epoch", () => {
  const selected = { state: guide, build: { build_id: "failed-candidate", status: "failed" },
    receipt: configurable, options: installedOptions };
  assert.equal(deploymentSettingsResponseCurrent(selected, selected, configurable), true);
  for (const changed of [{ ...selected, blocked: true }, { ...selected, state: { ...guide, version: 5 } },
    { ...selected, state: { ...guide, id: "other" } }, { ...selected, build: { build_id: "replacement", status: "failed" } },
    { ...selected, receipt: { ...configurable, status: "running" } }])
    assert.equal(deploymentSettingsResponseCurrent(selected, changed, configurable), false);
  for (const changed of [{ deployment_id: "another" }, { build_id: "new-candidate" }, { package_sha256: "c".repeat(64) },
    { source_digest: "c".repeat(64) }, { destination: "/another/folder" }, { guide_version: 3 },
    { execution_mode: "interactive" }, { status: "running" }, { process_live: true }])
    assert.equal(deploymentSettingsResponseCurrent(selected, selected, { ...configurable, ...changed }), false);
  const gate = createDeploymentPollingGate(); gate.open(guide.id);
  const token = gate.capture(guide.id); gate.admit(token, guide.id);
  assert.equal(gate.admit(token, guide.id), false);
  assert.equal(deploymentSettingsReadKey(selected).includes(installedOptions.source_digest), false);
});

test("new-folder setup cannot reveal old fields during the commit before a guide or build reset", () => {
  const opened = { state: guide, build: accepted, receipt: configurable };
  const key = deploymentSettingsReadKey(opened);
  assert.equal(deploymentConfigurationRevealCurrent("new", privateOptions, key, opened), true);
  for (const current of [{ ...opened, state: { ...guide, id: "other-guide" } },
    { ...opened, state: { ...guide, version: guide.version + 1 } },
    { ...opened, build: { ...accepted, build_id: "replacement" } },
    { ...opened, build: { ...accepted, source_digest: "changed-current-source" } },
    { ...opened, build: { ...accepted, ready_to_package: false } }, { ...opened, blocked: true }])
    assert.equal(deploymentConfigurationRevealCurrent("new", privateOptions, key, current), false);
  assert.equal(deploymentConfigurationRevealCurrent("new", { ...privateOptions, current: false }, key, opened), false);
  assert.equal(deploymentConfigurationRevealCurrent("new", privateOptions, "", opened), false);
  assert.equal(deploymentConfigurationRevealCurrent("new", privateOptions, deploymentSettingsReadKey({ ...opened, state: { ...guide, version: 5 } }),
    { ...opened, state: { ...guide, version: 5 } }), false);
  // The installed path keeps its own checked options, independent of B's readiness.
  const failed = { ...opened, build: { build_id: "failed-B", status: "failed" } };
  assert.equal(deploymentConfigurationRevealCurrent("settings", installedOptions, deploymentSettingsReadKey(failed), failed), true);
  assert.equal(deploymentConfigurationRevealCurrent("settings", installedOptions, key, failed), false);
});

function delayed() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test("a delayed passive deployment read cannot undo a later local installation or action admission", async () => {
  const gate = createDeploymentPollingGate();
  gate.open(guide.id);
  let visible = configurable;
  const pending = delayed();
  const read = gate.observe(guide.id, () => pending.promise);
  const admitted = { ...configurable, status: "queued", phase: "install" };
  assert.equal(gate.admit(gate.capture(guide.id), guide.id), true);
  visible = admitted;
  pending.resolve({ ...configurable, status: "stopped" });
  const old = await read;
  if (old && gate.current(old.token, guide.id)) visible = old.receipt;
  assert.equal(old, null);
  assert.equal(visible, admitted);
  const current = { ...admitted, status: "starting" };
  const next = await gate.observe(guide.id, async () => current);
  assert.equal(gate.current(next.token, guide.id), true);
  visible = next.receipt;
  assert.equal(visible, current);
  assert.equal(visible.deployment_id, admitted.deployment_id);
});

test("pre-admission 404 and connection failures cannot clear or mislabel the admitted receipt", async () => {
  for (const failure of [{ status: 404 }, { status: 503 }]) {
    const gate = createDeploymentPollingGate();
    gate.open(guide.id);
    const pending = delayed(), read = gate.observe(guide.id, () => pending.promise);
    assert.equal(gate.admit(gate.capture(guide.id), guide.id), true);
    pending.reject(failure);
    assert.equal(await read, null);
    const actual = await gate.observe(guide.id, async () => { throw failure; });
    assert.equal(actual.failure, failure);
    assert.equal(gate.current(actual.token, guide.id), true);
  }
});

test("the final poll publication fence catches an admission between observation and caller continuation", async () => {
  const gate = createDeploymentPollingGate();
  gate.open(guide.id);
  const observed = await gate.observe(guide.id, async () => configurable);
  assert.equal(gate.current(observed.token, guide.id), true);
  assert.equal(gate.admit(gate.capture(guide.id), guide.id), true);
  assert.equal(gate.current(observed.token, guide.id), false);
  assert.equal(gate.sameScope(observed.token, guide.id), true);
});

test("guide switching and unmount invalidate outstanding reads and action responses even on return to the same guide", async () => {
  const gate = createDeploymentPollingGate();
  const original = gate.open(guide.id), action = gate.capture(guide.id);
  const pending = delayed(), read = gate.observe(guide.id, () => pending.promise);
  gate.close(original);
  gate.open("another-guide");
  gate.open(guide.id);
  assert.equal(gate.admit(action, guide.id), false);
  assert.equal(gate.sameScope(action, guide.id), false);
  pending.resolve(configurable);
  assert.equal(await read, null);
  gate.close(original); // An old cleanup cannot close the newer mount.
  const latest = await gate.observe(guide.id, async () => configurable);
  assert.equal(latest.receipt, configurable);
  const unmount = gate.open(guide.id), final = gate.capture(guide.id);
  gate.close(unmount);
  assert.equal(gate.current(final, guide.id), false);
  assert.equal(await gate.observe(guide.id, async () => configurable), null);
});

test("installation requires authentic current acceptance rather than legacy source-only readiness", () => {
  assert.equal(deploymentCanPrepare(accepted, guide), true);
  for (const changes of [{ acceptance_current: false }, { acceptance_trusted: false }, { acceptance: null },
    { acceptance: { status: "fail" } }, { current: false }, { guide_id: "other" }, { guide_version: 3 },
    { ready_to_package: false }, { status: "running" }]) assert.equal(deploymentCanPrepare({ ...accepted, ...changes }, guide), false);
});

test("an accepted candidate still awaiting cleanup cannot prepare or install while an existing Console stays usable", () => {
  const pending = { ...accepted, cleanup_pending: true, source_verified: true, source_digest: "checked-source" };
  const running = { ...configurable, status: "running", process_live: true, health: "ready",
    console_url: "http://127.0.0.1:18088/console", bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  const before = structuredClone({ pending, running, options: privateOptions });
  assert.equal(deploymentCanPrepare(pending, guide), false);
  assert.equal(deploymentOptionsCurrent(privateOptions, pending, guide), false);
  assert.equal(deploymentCanInstall(pending, guide, privateOptions), false);
  assert.equal(deploymentConsole(running), running.console_url);
  assert.deepEqual({ pending, running, options: privateOptions }, before);

  const settled = { ...pending, cleanup_pending: false };
  assert.equal(deploymentCanPrepare(settled, guide), true);
  assert.equal(deploymentCanInstall(settled, guide, privateOptions), true);
  assert.equal(settled.source_digest, pending.source_digest);
  assert.deepEqual(settled.acceptance, pending.acceptance);
});

test("local install requires current destination capability and never treats cloud or remote Linux as this Mac", () => {
  const options = { build_id: "build", guide_version: 4, current: true, target: "mac", host_target: "mac", supported: true };
  assert.equal(deploymentCanInstall(accepted, guide, options), true);
  assert.equal(deploymentCanInstall(accepted, guide), false);
  for (const changes of [{ supported: false }, { host_target: "linux" }, { target: "gcp" },
    { build_id: "old-build" }, { guide_version: 3 }, { current: false }])
    assert.equal(deploymentCanInstall(accepted, guide, { ...options, ...changes }), false);
  for (const target of ["linux", "gcp"]) assert.equal(deploymentCanInstall(accepted,
    { ...guide, proposal: { target } }, { ...options, target, supported: false }), false);
});

test("fresh package availability leaves host compatibility and existing update staging intact", () => {
  const base = { build_id: accepted.build_id, guide_version: guide.version, current: true,
    target: "mac", host_target: "mac", supported: true, default_destination: "/owned/new-app", console_port: 18088,
    execution_modes: [{ id: "interactive", available: true }] };
  const reason = "Use the checked update of the existing stopped installation.";
  const restricted = { ...base, fresh_install: { available: false, reason } };
  const receipt = { guide_id: guide.id, build_id: "installed", status: "stopped" };
  const before = structuredClone({ restricted, receipt });
  assert.equal(deploymentCanInstall(accepted, guide, restricted), true);
  assert.equal(deploymentCanFreshInstall(accepted, guide, restricted), false);
  assert.equal(deploymentFreshInstallRestriction(accepted, guide, restricted), reason);
  assert.equal(deploymentInstallSelection(restricted, accepted, guide), null);
  assert.equal(deploymentConfigurationForm(restricted, receipt, accepted, guide), null);
  assert.equal(deploymentCanStageUpdate(receipt, accepted, guide, restricted), true);
  for (const fresh_install of [null, {}, { available: "true" }, { available: 1 }]) {
    const invalid = { ...base, fresh_install };
    assert.equal(deploymentCanFreshInstall(accepted, guide, invalid), false);
    assert.ok(deploymentFreshInstallRestriction(accepted, guide, invalid));
  }
  for (const changes of [{ current: false }, { build_id: "other" }, { guide_version: 3 }, { supported: false },
    { target: "linux", host_target: "linux" }]) {
    const stale = { ...restricted, ...changes };
    assert.equal(deploymentCanFreshInstall(accepted, guide, stale), false);
    assert.equal(deploymentFreshInstallRestriction(accepted, guide, stale), null);
    assert.equal(deploymentInstallSelection(stale, accepted, guide), null);
  }
  for (const options of [base, { ...base, fresh_install: { available: true, reason: null } }]) {
    assert.equal(deploymentCanFreshInstall(accepted, guide, options), true);
    assert.equal(deploymentFreshInstallRestriction(accepted, guide, options), null);
    assert.deepEqual(deploymentInstallSelection(options, accepted, guide), { build_id: accepted.build_id });
    assert.ok(deploymentConfigurationForm(options, receipt, accepted, guide));
  }
  assert.equal(deploymentCanFreshInstall(null, nativeGuide,
    { ...nativeOptions, fresh_install: { available: true, reason: null } }), true);
  assert.deepEqual({ restricted, receipt }, before);
});

test("remote handoff is scoped to the exact accepted build and never becomes live readiness", () => {
  const remoteGuide = { ...guide, proposal: { target: "gcp" } };
  const handoff = { mode: "remote-package", status: "not-connected", steps: [], limitations: [],
    package_url: "/api/guide/guide/implementation/package?build_id=build" };
  const options = { build_id: "build", guide_version: 4, current: true, target: "gcp", host_target: "mac", supported: false, handoff };
  assert.equal(deploymentOptionsCurrent(options, accepted, remoteGuide), true);
  assert.equal(deploymentHandoff(options, accepted, remoteGuide), handoff);
  for (const changes of [{ build_id: "other" }, { current: false }, { target: "linux" }, { supported: true },
    { handoff: { ...handoff, status: "ready" } }, { handoff: { ...handoff, package_url: "/api/guide/other/implementation/package?build_id=build" } }])
    assert.equal(deploymentHandoff({ ...options, ...changes }, accepted, remoteGuide), null);
  assert.equal(deploymentHandoff(options, { ...accepted, acceptance_current: false }, remoteGuide), null);
});

test("a console link needs actual native readiness, an owned process and live loopback health", () => {
  const receipt = { status: "running", process_live: true, health: "ready", console_url: "http://127.0.0.1:18088/console",
    bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  assert.equal(deploymentConsole(receipt), receipt.console_url);
  for (const changes of [{ status: "starting" }, { health: "unavailable" }, { process_live: false },
    { console_url: "http://example.org/console" }, { console_url: "http://secret@127.0.0.1:18088/console" },
    { bootstrap: { ...receipt.bootstrap, complete: false } }, { bootstrap: { ...receipt.bootstrap, counts: { broken: 1 } } }])
    assert.equal(deploymentConsole({ ...receipt, ...changes }), null);
});

test("private configuration submits requested data only, without implicit host credential use", () => {
  const options = { supported: true, current: true, host_provider_keys: ["OPENAI_API_KEY"], fields: [
    { key: "OPENAI_API_KEY", required: true, secret: true }, { key: "SPACE_TOKEN", automatic: true, required: true },
    { key: "FILES_ROOT", required: false, secret: false },
  ] };
  assert.equal(deploymentCanSubmit(options, {}, false), false);
  assert.equal(deploymentCanSubmit(options, {}, true), true);
  assert.equal(deploymentCanSubmit(options, { OPENAI_API_KEY: "dedicated-secret" }, false), true);
  assert.deepEqual(deploymentSettings(options, { OPENAI_API_KEY: "dedicated-secret", SPACE_TOKEN: "do-not-copy",
    PYTHONPATH: "unrequested", MOBKIT_PORT: "18088" }), { OPENAI_API_KEY: "dedicated-secret", MOBKIT_PORT: "18088" });
});

test("interrupted installations are resumable, running and stopping are active, stale installed systems remain distinct", () => {
  assert.equal(deploymentIsActive({ status: "running" }), true);
  assert.equal(deploymentIsActive({ status: "stopping" }), true);
  assert.equal(deploymentIsActive({ status: "interrupted" }), false);
  assert.equal(deploymentIsActive({ status: "failed" }), false);
  assert.match(deploymentPhase({ status: "running", health: "unavailable" }), /health check/);
  assert.match(deploymentPhase({ status: "interrupted" }), /interrupted/);
  const running = { status: "running", process_live: true, health: "ready", console_url: "http://127.0.0.1:18088/console",
    bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  assert.equal(deploymentPhase(running), "Your custom application is running");
  assert.equal(deploymentPhase({ ...running, source_kind: "native-team" }), "Your agent team is running");
  for (const change of [{ process_live: false }, { bootstrap: null }, { console_url: "https://example.com/console" }])
    assert.match(deploymentPhase({ ...running, ...change }), /health check/);
});

test("legacy startup failure explains the observed phase without inferring a model cause or successful integration", () => {
  const value = deploymentDiagnostic({ status: "failed", phase: "start", bootstrap: null,
    detail: "model gpt-4.1-mini absent; private token and folder", checks: [{ status: "pass" }] });
  assert.match(value, /startup failed/);
  assert.doesNotMatch(value, /gpt-4|private|integration.*success/);
  assert.equal(deploymentDiagnostic({ status: "running", phase: "run" }), null);
  for (const phase of ["private folder", ["start"], null])
    assert.equal(deploymentDiagnostic({ status: "failed", phase }), null);
});

test("only bound typed health observations explain Console failures and stale private fields never reach wording", () => {
  const attempt = "12345678-1234-1234-1234-123456789abc";
  const receipt = { status: "failed", phase: "start", startup_attempt_id: attempt,
    startup_console_health: { attempt_id: attempt, status: "fail", category: "authentication", attempts: 1,
      http_status: 401, detail: "private-token", headers: { Authorization: "secret" } } };
  assert.match(deploymentDiagnostic(receipt), /rejected startup authorization/);
  assert.doesNotMatch(deploymentDiagnostic(receipt), /private-token|secret/);
  for (const changes of [{ attempt_id: "older-attempt" }, { attempts: true }, { attempts: 1001 },
    { category: ["authentication"] }, { category: "private-token" }, { http_status: true }, { http_status: "private-token" }]) {
    const value = deploymentDiagnostic({ ...receipt, startup_console_health: { ...receipt.startup_console_health, ...changes } });
    assert.match(value, /startup failed/);
    assert.doesNotMatch(value, /rejected startup authorization|private-token|secret/);
  }
  assert.match(deploymentDiagnostic({ ...receipt, startup_attempt_id: undefined }), /startup failed/);
  assert.match(deploymentDiagnostic({ ...receipt, phase: "install" }), /installation step failed/);
  assert.equal(deploymentDiagnostic({ ...receipt, startup_console_health: {
    ...receipt.startup_console_health, category: "http_status", http_status: 503 } }),
    "The Console returned HTTP 503 during startup verification. Review startup activity before retrying.");
});

test("background execution is admitted only when the host advertises actual availability", () => {
  const base = { supported: true, current: true, fields: [] };
  assert.equal(deploymentCanSubmit(base, {}, false, "background"), false);
  assert.equal(deploymentCanSubmit({ ...base, execution_modes: [{ id: "background", available: true }] }, {}, false, "background"), true);
  for (const available of [false, "true", null]) assert.equal(deploymentCanSubmit({ ...base,
    execution_modes: [{ id: "background", available }] }, {}, false, "background"), false);
  assert.equal(deploymentCanSubmit(base, {}, false, "unknown"), false);
});

test("legacy background updates offer only a compatibility preview while interactive update preparation stays supported", () => {
  const receipt = { guide_id: guide.id, deployment_id: "owned", build_id: "previous", status: "running",
    execution_mode: "background" };
  const options = { ...privateOptions };
  const before = structuredClone({ receipt, build: accepted, guide, options });
  const preview = deploymentUpdatePreparation(receipt, accepted, guide, options);
  assert.equal(preview.available, true);
  assert.equal(preview.label, "Preview update compatibility");
  assert.match(preview.limitation, /cannot apply or roll back its source/);
  assert.match(preview.limitation, /service and its recorded activity stay unchanged/);
  const foreground = deploymentUpdatePreparation({ ...receipt, execution_mode: "interactive" }, accepted, guide, options);
  assert.equal(foreground.available, true);
  assert.equal(foreground.label, "Stage accepted update");
  assert.equal(foreground.limitation, null);
  const blocked = deploymentUpdatePreparation({ ...receipt, update: { status: "blocked",
    to_build_id: accepted.build_id, guide_version: accepted.guide_version } }, accepted, guide, options);
  assert.equal(blocked.available, true);
  assert.equal(blocked.label, "Recheck accepted update");
  assert.match(blocked.limitation, /update adapter could not be verified/);
  for (const changed of [{ ...receipt, guide_id: "other" }, { ...receipt, deployment_id: null }])
    assert.equal(deploymentUpdatePreparation(changed, accepted, guide, options).limitation, null);
  for (const build of [{ ...accepted, current: false }, { ...accepted, status: "running" },
    { ...accepted, acceptance_trusted: false }])
    assert.equal(deploymentUpdatePreparation(receipt, build, guide, options).available, false);
  assert.deepEqual({ receipt, build: accepted, guide, options }, before);
});

const backgroundReceipt = () => ({ guide_id: guide.id, deployment_id: "background-owned", build_id: "previous",
  package_sha256: "a".repeat(64), status: "stopped", execution_mode: "background", process_live: false, runtime_pid: null,
  service_supervisor_pid: 1234, service_cleanup_confirmed: true,
  service: { manager: "launchd", unit_sha256: "b".repeat(64), public_summary: { definition_verified: true } },
  service_observation: { owned: true, loaded: false, state: "stopped", pid: null },
  update_capabilities: { background_activation_supported: true, requires_stopped: true,
    deployment_id: "background-owned", build_id: "previous", package_sha256: "a".repeat(64),
    manager: "launchd", unit_sha256: "b".repeat(64), host_manifest_sha256: "c".repeat(64) },
  update: { update_id: "exact-stage", status: "staged", to_build_id: accepted.build_id,
    guide_version: accepted.guide_version, source_digest: "d".repeat(64), blockers: [], rollback_available: false } });

test("verified background staging remains a comparison and activation binds the exact stopped owned service", () => {
  const receipt = backgroundReceipt(), build = { ...accepted, source_digest: receipt.update.source_digest };
  const before = structuredClone({ receipt, build, guide, options: privateOptions });
  assert.deepEqual(deploymentBackgroundUpdate(receipt, guide), { supported: true, stopped: true });
  const running = { ...receipt, status: "running", process_live: true, runtime_pid: 2345,
    update: undefined, service_cleanup_confirmed: false };
  const prepared = deploymentUpdatePreparation(running, build, guide, privateOptions);
  assert.equal(prepared.available, true);
  assert.equal(prepared.activationSupported, true);
  assert.equal(prepared.label, "Stage accepted update");
  assert.match(prepared.limitation, /without changing the service/);
  assert.match(prepared.limitation, /Stop the application and wait for confirmed cleanup/);
  assert.equal(deploymentUpdateAction(running, build, guide, "apply-update"), null);
  assert.deepEqual(deploymentUpdateAction(receipt, build, guide, "apply-update"), {
    deployment_id: receipt.deployment_id, action: "apply-update", update_id: "exact-stage" });
  const recovery = { ...receipt, update: { ...receipt.update, status: "recovery-needed", rollback_available: true } };
  assert.equal(deploymentUpdateAction(recovery, build, guide, "recover-current").update_id, "exact-stage");
  const rollback = { ...receipt, update: { ...receipt.update, status: "completed", rollback_available: true } };
  assert.equal(deploymentUpdateAction(rollback, null, { ...guide, version: 10 }, "rollback").update_id, "exact-stage");
  const linux = { ...receipt, service: { ...receipt.service, manager: "systemd-user" },
    update_capabilities: { ...receipt.update_capabilities, manager: "systemd-user" },
    service_observation: { owned: true, loaded: true, state: "inactive", pid: null, control_pid: null, control_group: "" } };
  assert.deepEqual(deploymentBackgroundUpdate(linux, guide), { supported: true, stopped: true });
  assert.equal(deploymentUpdateAction(linux, build, guide, "apply-update").update_id, "exact-stage");
  for (const change of [{ acceptance_trusted: false }, { current: false }, { source_digest: "another" }])
    assert.equal(deploymentUpdateAction(receipt, { ...build, ...change }, guide, "apply-update"), null);
  assert.equal(deploymentUpdateAction({ ...receipt, update: { ...receipt.update, blockers: ["Native settings changed"] } }, build, guide, "apply-update"), null);
  assert.deepEqual({ receipt, build, guide, options: privateOptions }, before);
});

test("background capabilities cannot transfer between installations or accept malformed support", () => {
  const receipt = backgroundReceipt(), capability = receipt.update_capabilities;
  for (const change of [{ background_activation_supported: false }, { background_activation_supported: "true" },
    { requires_stopped: false }, { deployment_id: "foreign" }, { build_id: "new-build" },
    { package_sha256: "e".repeat(64) }, { manager: "systemd" }, { unit_sha256: "e".repeat(64) },
    { host_manifest_sha256: null }]) {
    const changed = { ...receipt, update_capabilities: { ...capability, ...change } };
    assert.deepEqual(deploymentBackgroundUpdate(changed, guide), { supported: false, stopped: false });
    assert.equal(deploymentUpdateAction(changed, { ...accepted, source_digest: receipt.update.source_digest }, guide, "apply-update"), null);
  }
  for (const changed of [{ ...receipt, guide_id: "foreign" }, { ...receipt, update_capabilities: null },
    { ...receipt, service: { ...receipt.service, public_summary: { definition_verified: false } } }])
    assert.equal(deploymentBackgroundUpdate(changed, guide).supported, false);
});

test("background source changes wait for actual stopped observation and native cleanup, not health or a missing PID", () => {
  const receipt = backgroundReceipt(), build = { ...accepted, source_digest: receipt.update.source_digest };
  for (const change of [{ status: "interrupted" }, { status: "failed" }, { process_live: true }, { runtime_pid: 2345 },
    { service_cleanup_confirmed: false }, { service_cleanup_confirmed: undefined },
    { service_observation: null }, { service_observation: { ...receipt.service_observation, owned: false } },
    { service_observation: { ...receipt.service_observation, loaded: true, state: "waiting" } },
    { service_observation: { ...receipt.service_observation, pid: 2345 } },
    { service_observation: { ...receipt.service_observation, control_pid: 2345 } },
    { service_observation: { ...receipt.service_observation, control_group: "/owned-group" } }]) {
    const changed = { ...receipt, health: "ready", bootstrap: { ready: true }, ...change };
    assert.equal(deploymentBackgroundUpdate(changed, guide).stopped, false);
    for (const action of ["apply-update", "recover-current", "rollback"])
      assert.equal(deploymentUpdateAction(changed, build, guide, action), null);
  }
  assert.equal(deploymentBackgroundUpdate({ ...receipt, service_supervisor_pid: null, service_cleanup_confirmed: undefined }, guide).stopped, true);
  assert.equal(deploymentBackgroundUpdate({ ...receipt, service_observation: { ...receipt.service_observation,
    loaded: true, state: "inactive", control_pid: null, control_group: "" } }, guide).stopped, true);
});

test("staging offers an exact accepted local update without turning native migrations into permission", async () => {
  const { deploymentCanStageUpdate } = await import("./deployment-panel.mjs");
  const options = { build_id: "build", guide_version: 4, current: true, target: "mac", host_target: "mac", supported: true };
  const receipt = { guide_id: guide.id, build_id: "old", status: "running", execution_mode: "interactive" };
  assert.equal(deploymentCanStageUpdate(receipt, accepted, guide, options), true);
  assert.equal(deploymentCanStageUpdate({ ...receipt, guide_id: "other" }, accepted, guide, options), false);
  assert.equal(deploymentCanStageUpdate(receipt, { ...accepted, acceptance_trusted: false }, guide, options), false);
  assert.equal(deploymentCanStageUpdate(receipt, accepted, guide, { ...options, supported: false }), false);
  assert.equal(deploymentCanStageUpdate({ ...receipt, update: { status: "verifying", to_build_id: "old" } }, accepted, guide, options), false);
  assert.equal(deploymentCanStageUpdate({ ...receipt, update: { status: "blocked", to_build_id: "build", guide_version: 4 } }, accepted, guide, options), true);
});

test("the same blocked accepted candidate can be explicitly rechecked without authorizing activation or changing its saved transaction", () => {
  const build = { ...accepted, source_digest: "same-accepted-source" };
  const options = { ...privateOptions, source_digest: build.source_digest };
  const receipt = { guide_id: guide.id, deployment_id: "deployment", build_id: "installed-build", status: "stopped",
    execution_mode: "interactive", update: { update_id: "retained-blocked-transaction", status: "blocked",
      to_build_id: build.build_id, guide_version: build.guide_version, source_digest: build.source_digest,
      blockers: ["The installed runtime does not support this update yet"], rollback_available: false } };
  const before = structuredClone({ receipt, build, guide, options });
  const prepared = deploymentUpdatePreparation(receipt, build, guide, options);
  assert.equal(prepared.available, true);
  assert.equal(prepared.label, "Recheck accepted update");
  for (const action of ["apply-update", "recover-current", "rollback"])
    assert.equal(deploymentUpdateAction(receipt, build, guide, action), null);
  for (const change of [{ current: false }, { status: "failed" }, { ready_to_package: false },
    { cleanup_pending: true }, { acceptance_current: false }, { acceptance_trusted: false },
    { acceptance: { status: "fail" } }, { guide_id: "another" }, { guide_version: guide.version - 1 }])
    assert.equal(deploymentCanStageUpdate(receipt, { ...build, ...change }, guide, options), false);
  for (const change of [{ current: false }, { build_id: "another" }, { guide_version: guide.version - 1 },
    { source_digest: "another-source" }, { supported: false }, { host_target: "linux" }])
    assert.equal(deploymentCanStageUpdate(receipt, build, guide, { ...options, ...change }), false);
  for (const change of [{ guide_id: "another" }, { build_id: build.build_id }, { status: "stopping" },
    { status: "running", phase: "update" }, { status: "running", phase: "recovery" }])
    assert.equal(deploymentCanStageUpdate({ ...receipt, ...change }, build, guide, options), false);
  for (const change of [{ to_build_id: "earlier-candidate" }, { guide_version: guide.version - 1 }])
    assert.equal(deploymentUpdatePreparation({ ...receipt, update: { ...receipt.update, ...change } },
      build, guide, options).label, "Stage accepted update");
  assert.deepEqual({ receipt, build, guide, options }, before);
});

test("explicit rollback offers a fresh stage of the same accepted candidate without automatically reapplying it", async () => {
  const { deploymentCanStageUpdate, deploymentUpdateAction } = await import("./deployment-panel.mjs");
  const build = { ...accepted, source_digest: "same-accepted-candidate" };
  const options = { build_id: "build", guide_version: 4, current: true, target: "mac", host_target: "mac", supported: true };
  const receipt = { guide_id: guide.id, deployment_id: "deployment", build_id: "previous-build", status: "stopped",
    update: { update_id: "old-transaction", status: "rolled-back", to_build_id: build.build_id,
      guide_version: guide.version, source_digest: build.source_digest, blockers: [], rollback_available: false } };
  assert.equal(deploymentCanStageUpdate(receipt, build, guide, options), true);
  assert.equal(deploymentUpdateAction(receipt, build, guide, "apply-update"), null);
  for (const status of ["snapshotting", "applying", "verifying", "restoring", "recovering-candidate", "recovering-current", "recovery-needed", "completed"])
    assert.equal(deploymentCanStageUpdate({ ...receipt, update: { ...receipt.update, status } }, build, guide, options), false);
  assert.equal(deploymentCanStageUpdate(receipt, { ...build, acceptance_current: false }, guide, options), false);
  assert.equal(deploymentCanStageUpdate({ ...receipt, build_id: build.build_id }, build, guide, options), false);
});

test("update admission uses the exact stage and current accepted source while rollback remains an explicit historical action", async () => {
  const { deploymentUpdateAction } = await import("./deployment-panel.mjs");
  const build = { ...accepted, source_digest: "new-source" };
  const receipt = { guide_id: guide.id, deployment_id: "deployment", status: "stopped", update: {
    update_id: "stage-a", status: "staged", to_build_id: build.build_id, guide_version: 4,
    source_digest: "new-source", blockers: [], rollback_available: false,
  } };
  assert.deepEqual(deploymentUpdateAction(receipt, build, guide, "apply-update"), {
    deployment_id: "deployment", action: "apply-update", update_id: "stage-a",
  });
  for (const change of [{ source_digest: "changed" }, { guide_version: 3 }, { to_build_id: "old" },
    { status: "blocked" }, { blockers: ["Native instructions require migration"] }])
    assert.equal(deploymentUpdateAction({ ...receipt, update: { ...receipt.update, ...change } }, build, guide, "apply-update"), null);
  assert.equal(deploymentUpdateAction({ ...receipt, status: "running" }, build, guide, "apply-update"), null);
  assert.equal(deploymentUpdateAction({ ...receipt, execution_mode: "background" }, build, guide, "apply-update"), null);
  const completed = { ...receipt, update: { ...receipt.update, status: "completed", rollback_available: true } };
  assert.deepEqual(deploymentUpdateAction(completed, null, { ...guide, version: 9 }, "rollback"), {
    deployment_id: "deployment", action: "rollback", update_id: "stage-a",
  });
  assert.equal(deploymentUpdateAction(completed, build, { ...guide, id: "other" }, "rollback"), null);
  assert.equal(deploymentUpdateAction(receipt, build, guide, "rollback"), null);
});

test("forward recovery is an exact current accepted candidate action, never permission to stage another build or bypass a migration", async () => {
  const { deploymentUpdateAction, deploymentCanStageUpdate } = await import("./deployment-panel.mjs");
  const build = { ...accepted, source_digest: "accepted-source" };
  const receipt = { guide_id: guide.id, deployment_id: "deployment", status: "failed", build_id: "previous",
    update: { update_id: "exact-transaction", status: "recovery-needed", to_build_id: build.build_id,
      guide_version: guide.version, source_digest: build.source_digest, blockers: [], rollback_available: true } };
  assert.deepEqual(deploymentUpdateAction(receipt, build, guide, "recover-current"), {
    deployment_id: "deployment", action: "recover-current", update_id: "exact-transaction",
  });
  const options = { build_id: "build", guide_version: 4, current: true, target: "mac", host_target: "mac", supported: true };
  assert.equal(deploymentCanStageUpdate(receipt, build, guide, options), false);
  for (const changes of [{ source_digest: "different" }, { to_build_id: "different" }, { guide_version: 3 },
    { blockers: ["Native config changed"] }, { status: "staged" }])
    assert.equal(deploymentUpdateAction({ ...receipt, update: { ...receipt.update, ...changes } }, build, guide, "recover-current"), null);
  assert.equal(deploymentUpdateAction(receipt, { ...build, acceptance_current: false }, guide, "recover-current"), null);
  assert.equal(deploymentUpdateAction(receipt, build, { ...guide, id: "another" }, "recover-current"), null);
  assert.equal(deploymentUpdateAction({ ...receipt, status: "running" }, build, guide, "recover-current"), null);
  const interrupted = { ...receipt, status: "interrupted", update: { ...receipt.update, status: "recovering-current" } };
  assert.equal(deploymentUpdateAction(interrupted, build, guide, "recover-current").update_id, "exact-transaction");
  assert.equal(deploymentCanStageUpdate(interrupted, build, guide, options), false);
});

const installed = { guide_id: guide.id, deployment_id: "installed", build_id: "installed-build", package_sha256: "a".repeat(64),
  source_digest: "b".repeat(64), status: "running", process_live: true, health: "ready",
  console_url: "http://127.0.0.1:18088/console", bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
const declared = { deployment_id: installed.deployment_id, build_id: installed.build_id, package_sha256: installed.package_sha256,
  source_digest: installed.source_digest, metadata_sha256: "c".repeat(64), scope: "declared-source", verified: false,
  entries: [{ id: "reading-list", label: "Reading list", path: "/reading", url: "http://127.0.0.1:18089/reading",
    port: 18089, status: "declared", verified: false }] };

test("UI declaration refresh keys track source and runtime transitions without following routine receipt polling", () => {
  const key = deploymentExperienceKey(installed, guide.id);
  assert.ok(key);
  assert.equal(deploymentExperienceKey({ ...installed, health_checked_at: "now", progress: [{ message: "runtime active" }],
    logs: ["new output"], current: false, guide_version: 3 }, guide.id), key);
  for (const changes of [{ build_id: "another" }, { package_sha256: "new-package" }, { status: "stopped" },
    { console_url: null }, { deployment_id: "replacement" }])
    assert.notEqual(deploymentExperienceKey({ ...installed, ...changes }, guide.id), key);
  assert.equal(deploymentExperienceKey(installed, "another-guide"), "");
});

test("declared UI uses actual source-bound URLs, including separate local ports, while native health alone never verifies the application", () => {
  const result = deploymentExperiences(declared, installed, guide.id);
  assert.equal(result.issue, null);
  assert.equal(result.entries[0].url, declared.entries[0].url);
  assert.equal(result.entries[0].verified, false);
  assert.equal(result.entries[0].status, "declared");
  const resolved = { ...declared, entries: [{ ...declared.entries[0], port: undefined, port_field: "APP_PORT" }] };
  assert.equal(deploymentExperiences(resolved, installed, guide.id).entries[0].url, declared.entries[0].url);
  for (const changes of [{ health: "unavailable" }, { status: "starting" }, { process_live: false },
    { console_url: null }, { bootstrap: { complete: false } }]) {
    const gated = deploymentExperiences(declared, { ...installed, ...changes }, guide.id);
    assert.equal(gated.entries.length, 1);
    assert.equal(gated.entries[0].url, null);
    assert.equal(gated.entries[0].verified, false);
  }
  const noUrl = { ...declared, entries: [{ ...declared.entries[0], url: null, reason: "Start the declared application." }] };
  assert.equal(deploymentExperiences(noUrl, installed, guide.id).entries[0].url, null);
  assert.deepEqual(deploymentExperiences({ ...declared, metadata_sha256: null, entries: [] }, installed, guide.id), { entries: [], issue: null });
});

test("UI declarations from another deployment/build/source or a malformed response cannot become entry links", () => {
  for (const changes of [{ deployment_id: "other" }, { build_id: "other" }, { package_sha256: "other" },
    { source_digest: "d".repeat(64) }, { metadata_sha256: null }, { scope: "verified" }, { verified: true },
    { entries: null }, { error: "metadata could not be read" }]) {
    const result = deploymentExperiences({ ...declared, ...changes }, installed, guide.id);
    assert.equal(result.entries.length, 0);
    assert.ok(result.issue);
  }
  assert.equal(deploymentExperiences(declared, installed, "another-guide").entries.length, 0);
  assert.equal(deploymentExperiences({ ...declared, entries: [declared.entries[0], declared.entries[0]] }, installed, guide.id).entries.length, 0);
});

test("UI links reject external or credential URLs, invalid ports, traversal and fabricated verification", () => {
  const entry = declared.entries[0];
  for (const changes of [{ url: "https://example.com/reading" }, { url: "http://localhost:18089/reading" },
    { url: "http://private@127.0.0.1:18089/reading" }, { url: "javascript:alert(1)" },
    { url: "http://127.0.0.1:80/reading" }, { url: "http://127.0.0.1:18089/other" },
    { url: "http://127.0.0.1:18089/reading?token=private" }, { url: "http://127.0.0.1:18089/reading#private" },
    { url: "http://127.0.0.1:18089/reading\n" }, { url: { toString: () => entry.url } }, { url: undefined },
    { port: 19000 }, { port: "18089" }, { path: "/../reading" }, { path: "/%252e%252e/reading" },
    { path: "//example.com" }, { path: "/reading%3ftoken=private" }, { path: "/reading\\other" },
    { verified: true }, { status: "ready" }, { label: "" }]) {
    const result = deploymentExperiences({ ...declared, entries: [{ ...entry, ...changes }] }, installed, guide.id);
    assert.equal(result.entries.length, 0, JSON.stringify(changes));
    assert.ok(result.issue);
  }
});

test("after setup an applied update names the owner's Start, never a verified worker start", () => {
  assert.equal(deploymentUpdateOwnerStart({ status: "completed", setup_restore_sha256: "a".repeat(64) }), true);
  for (const update of [null, undefined, { status: "completed" }, { setup_restore_sha256: "A".repeat(64) },
    { setup_restore_sha256: "a".repeat(63) }, { setup_restore_sha256: { path: "/private/restore-selection.json" } }]) {
    assert.equal(deploymentUpdateOwnerStart(update), false);
  }
});
