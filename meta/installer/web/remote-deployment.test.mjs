import assert from "node:assert/strict";
import test from "node:test";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { remoteInstalledCanConfigure, remoteInstalledSettingsOptions, remoteInstalledSettingsBody } from "./remote-private-settings.mjs";
import { remoteInstalledUseBinding, remoteInstalledUseUrl } from "./remote-installed-use.mjs";
import { remoteDeploymentUrl, remoteSelectionKey, remotePrivateDraftScope, remoteCanConnect, remoteConnectionInput, remoteReceiptOwned,
  remoteReceiptCurrent, remotePrivateOptions, remoteCanConfigure, remoteInstallSettings, remoteCanInstall,
  remoteActionAvailable, remoteObservationAvailable, remoteIsBusy, remoteInstallAdmitted,
  remoteNativeReady, remotePhase, remoteDeploymentStep, remoteCanRetryInstall, remotePrivateBinding, remoteFailureGuidance,
  remotePendingBuildHandoff, remoteCloudRemoval,
  remoteSetupConsolePort, remoteSetupConsolePortValue, remoteSetupPortObservation, remoteSetupInstallSettings,
  remoteDeploymentChecks, remoteDeploymentActivity } from "./remote-deployment.mjs";

const guide = { id: "synthetic-remote-guide", version: 4, proposal: { target: "linux" } };
const checked = { guide_id: guide.id, guide_version: guide.version, build_id: "synthetic-accepted-build",
  source_digest: "a".repeat(64), status: "completed", current: true, ready_to_package: true,
  acceptance_current: true, acceptance_trusted: true, acceptance: { status: "pass" } };
const options = { build_id: checked.build_id, guide_version: guide.version, current: true,
  supported: false, target: "linux", host_target: "mac",
  fields: [{ key: "STALE_OPTION_FIELD", label: "Old option field", required: true, secret: true }] };
const endpoint = { host: "workshop.local", user: "volunteer", ssh_port: 22 };
const destination = "/synthetic-owned/src/workshop";
const ready = { protocol: 1, deployment_id: "synthetic-owned-remote", guide_id: guide.id,
  guide_version: guide.version, build_id: checked.build_id, accepted_source_digest: checked.source_digest,
  package_sha256: "b".repeat(64), transferred_source_digest: "c".repeat(64), source_binding_current: true,
  endpoint, destination, status: "configuration-required", observation: { available: true },
  can_install: true, can_stop: false, can_restart: false,
  preflight: { ready: true, host: "synthetic-linux", checks: [{ name: "Python", status: "passed", detail: "Synthetic 3.11 check" }] },
  fields: [
    { key: "APP_USER", label: "Application user", required: true, secret: false },
    { key: "APP_TOKEN", label: "Application token", required: true, secret: true },
    { key: "OPTIONAL_NOTE", label: "Optional note", required: false, secret: false },
  ], progress: [], checks: [] };
const portOptions = { ...options, source_digest: checked.source_digest, console_port: 18088,
  current_guide_version: guide.version };
const initialPortSetup = { ...ready, remote_admission_requested: false, admitted: false };
const failedDestinationCheck = { ...ready, status: "failed", remote_admission_requested: false, can_install: false,
  preflight: { ...ready.preflight, ready: false,
    checks: [{ name: "systemd-user", status: "failed", detail: "The destination's user service manager is unavailable." }] } };
const localPortOptions = (port, status = "available", alternate_port) => ({ ...portOptions,
  console_port_observation: { scope: "local-console-port-observation", port, status,
    observed_at: "2026-10-06T16:00:00Z", ...(alternate_port ? { alternate_port } : {}) } });
const failedOperation = "11111111-1111-4111-8111-111111111111";
const retryOperation = "22222222-2222-4222-8222-222222222222";
const otherOperation = "33333333-3333-4333-8333-333333333333";
const retryableFailure = { ...ready, status: "failed", can_install: false, can_retry_install: true,
  operation_id: failedOperation, retry_of_operation_id: failedOperation, admitted: true,
  worker_live: false, cleanup_confirmed: true, source_current: true, failure_phase: "dependencies" };
const updatedBuild = { ...checked, build_id: "synthetic-updated-build", source_digest: "d".repeat(64) };
const updatedOptions = { ...options, build_id: updatedBuild.build_id };
const pendingEarlierBuild = { ...ready, source_binding_current: false, remote_admission_requested: false };
const updatedDestination = { ...ready, deployment_id: "synthetic-updated-destination", build_id: updatedBuild.build_id,
  accepted_source_digest: updatedBuild.source_digest, package_sha256: "e".repeat(64), remote_admission_requested: false };
const settingsInstallation = { ...ready, status: "stopped", admitted: true, can_install: false, can_restart: true,
  private_settings_supported: true, can_configure: true, source_current: true, worker_live: false, cleanup_confirmed: true };
const savedSettings = (receipt = settingsInstallation, version = guide.version) => ({ version,
  deployment_id: receipt.deployment_id, build_id: receipt.build_id, source_digest: receipt.accepted_source_digest,
  package_sha256: receipt.package_sha256, guide_version: receipt.guide_version, console_port: 18088,
  can_use_host_provider_credentials: false, source_verified: true, supported: true, requires_stopped: true,
  scope: "verified-remote-installed-private-configuration",
  fields: ready.fields.map((field) => ({ ...field, configured: true, removable: !field.required })) });

test("installed remote settings require stopped current custody and exact private field metadata", () => {
  assert.equal(remoteInstalledCanConfigure(settingsInstallation, options, checked, guide), true);
  for (const changed of [{ status: "running" }, { status: "failed" }, { worker_live: true }, { worker_live: undefined },
    { cleanup_confirmed: false }, { source_current: false }, { can_configure: false },
    { private_settings_supported: false }, { observation: { available: false } }, { source_binding_current: false }])
    assert.equal(remoteInstalledCanConfigure({ ...settingsInstallation, ...changed }, options, checked, guide), false);
  const raw = savedSettings(), before = structuredClone(raw);
  const parsed = remoteInstalledSettingsOptions(raw, settingsInstallation, options, checked, guide);
  assert.equal(parsed.fields[1].configured, true);
  assert.deepEqual(raw, before);
  for (const changed of [{ version: 3 }, { source_digest: "f".repeat(64) }, { deployment_id: "other" },
    { package_sha256: "f".repeat(64) }, { requires_stopped: false }, { can_use_host_provider_credentials: true },
    { scope: "verified-installed-private-configuration" }, { fields: raw.fields.map((field) => ({ ...field, configured: undefined })) }])
    assert.equal(remoteInstalledSettingsOptions({ ...raw, ...changed }, settingsInstallation, options, checked, guide), null);
  const values = { APP_USER: "", APP_TOKEN: "  exact replacement  ", OPTIONAL_NOTE: "ignored removal value", UNDECLARED: "never send" };
  assert.deepEqual(remoteInstalledSettingsBody(parsed, settingsInstallation, options, checked, guide, values, ["OPTIONAL_NOTE"]), {
    version: guide.version, deployment_id: ready.deployment_id, build_id: checked.build_id,
    source_digest: checked.source_digest, package_sha256: ready.package_sha256,
    settings: { APP_TOKEN: values.APP_TOKEN }, remove_fields: ["OPTIONAL_NOTE"] });
  assert.equal(remoteInstalledSettingsBody(parsed, settingsInstallation, options, checked, guide, {}, []), null);
  assert.equal(remoteInstalledSettingsBody(parsed, settingsInstallation, options, checked, guide, values, ["APP_TOKEN"]), null);
  assert.equal(remoteInstalledSettingsBody(parsed, settingsInstallation, options, checked, guide, values, ["OPTIONAL_NOTE", "OPTIONAL_NOTE"]), null);
});

test("cloud removal retains historical service evidence while withdrawing every remote admission", () => {
  const cloudEndpoint = { transport: "gcp-iap", operation_id: failedOperation, account: "fixture@example.test",
    project: "fixture-project", zone: "europe-west1-b", instance: "fixture-app", instance_id: "1234",
    ownership_token: "e".repeat(32), host: "compute.1234", user: "mobkit", ssh_port: 22 };
  const cloudOptions = { ...options, target: "gcp" };
  for (const cloud_status of ["removing", "removal-required", "removed"]) {
    const owned = { ...ready, target: "gcp", cloud_operation_id: failedOperation, endpoint: cloudEndpoint, cloud_status,
      can_install: true, can_stop: true, can_restart: true, source_current: true, ready: true };
    assert.equal(remoteReceiptOwned(owned, guide), true);
    assert.equal(remoteCloudRemoval(owned), cloud_status);
    assert.equal(remoteReceiptCurrent(owned, cloudOptions, checked, guide), false);
    assert.equal(remoteCanConfigure(owned, cloudOptions, checked, guide), false);
    assert.equal(remoteCanRetryInstall({ ...owned, ...retryableFailure, target: "gcp", endpoint: cloudEndpoint,
      cloud_operation_id: failedOperation, cloud_status }, cloudOptions, checked, guide), false);
    assert.equal(remoteNativeReady({ ...owned, status: "running" }), false);
    for (const action of ["stop", "restart"]) assert.equal(remoteActionAvailable(owned, action, cloudOptions, checked, guide), false);
  }
  assert.equal(remoteCloudRemoval({ ...ready, cloud_status: "removed" }), null, "A cloud state cannot revoke a separate Linux installation.");
});

test("pending build handoff retains only a known unadmitted destination and current trusted source", () => {
  const before = structuredClone(pendingEarlierBuild);
  const handoff = remotePendingBuildHandoff(pendingEarlierBuild, updatedOptions, updatedBuild, guide);
  assert.deepEqual(handoff.input, { ...endpoint, destination });
  assert.equal(handoff.buildId, updatedBuild.build_id);
  assert.equal(handoff.previousDeploymentId, ready.deployment_id);
  assert.equal(Object.hasOwn(handoff, "fields"), false);
  assert.equal(Object.hasOwn(handoff, "settings"), false);
  assert.deepEqual(pendingEarlierBuild, before);
  for (const changed of [{ remote_admission_requested: true }, { remote_admission_requested: undefined },
    { admitted: true }, { worker_live: true }, { source_binding_current: true }, { source_binding_current: undefined },
    { observation: { available: false } }, { observation: undefined }, { guide_id: "another" },
    { guide_version: guide.version + 1 }, { endpoint: { ...endpoint, ssh_port: 0 } }])
    assert.equal(remotePendingBuildHandoff({ ...pendingEarlierBuild, ...changed }, updatedOptions, updatedBuild, guide), null);
  for (const status of ["connecting", "installing", "checking", "starting", "running", "stopping", "restarting",
    "stopped", "recovery-needed", "observation-unavailable"])
    assert.equal(remotePendingBuildHandoff({ ...pendingEarlierBuild, status }, updatedOptions, updatedBuild, guide), null);
  for (const candidate of [{ ...updatedBuild, current: false }, { ...updatedBuild, acceptance_current: false },
    { ...updatedBuild, acceptance_trusted: false }, { ...updatedBuild, ready_to_package: false },
    { ...updatedBuild, cleanup_pending: true }, { ...updatedBuild, status: "running" }])
    assert.equal(remotePendingBuildHandoff(pendingEarlierBuild, updatedOptions, candidate, guide), null);
  assert.equal(remotePendingBuildHandoff(pendingEarlierBuild, updatedOptions, updatedBuild, guide, true), null);
  assert.equal(remotePendingBuildHandoff(pendingEarlierBuild, options, checked, guide), null);
  assert.equal(remotePendingBuildHandoff(pendingEarlierBuild, { ...updatedOptions, current: false }, updatedBuild, guide), null);
});

test("preflight failure remains pending only when no installation was requested", () => {
  assert.ok(remotePendingBuildHandoff({ ...pendingEarlierBuild, status: "failed" }, updatedOptions, updatedBuild, guide));
  assert.ok(remotePendingBuildHandoff({ ...pendingEarlierBuild, status: "not-installed", admitted: false }, updatedOptions, updatedBuild, guide));
  assert.equal(remotePendingBuildHandoff({ ...pendingEarlierBuild, status: "not-installed" }, updatedOptions, updatedBuild, guide), null);
  assert.equal(remotePendingBuildHandoff({ ...pendingEarlierBuild, status: "failed", remote_admission_requested: true,
    admitted: false, worker_live: false, cleanup_confirmed: true }, updatedOptions, updatedBuild, guide), null);
});

test("remote private setup carries declared constraints and stops invalid input before admission", () => {
  const receipt = { ...ready, fields: [...ready.fields, {
    key: "APP_PORT", label: "Application port", required: false, secret: false,
    type: "integer", minimum: 1024, maximum: 65535,
  }] };
  const values = { APP_USER: "synthetic-user", APP_TOKEN: "  exact-token-value  ", APP_PORT: "70000" };
  const before = structuredClone(values);
  assert.equal(remoteCanInstall(receipt, options, checked, guide, values), false);
  assert.equal(remoteCanInstall(receipt, options, checked, guide, { ...values, APP_PORT: "18089" }), true);
  assert.equal(remoteInstallSettings(remotePrivateOptions(receipt), values).APP_TOKEN, values.APP_TOKEN);
  assert.deepEqual(values, before);
  for (const changed of [{ maximum: 2 ** 53 }, { maximum: null }, { minimum: 65536 }, { type: "float" }])
    assert.equal(remotePrivateOptions({ ...receipt, fields: [...ready.fields, { ...receipt.fields.at(-1), ...changed }] }), null);
});

test("remote Console hints use only declared receipt authentication and exact credential fields", () => {
  const fields = [
    { key: "CONSOLE_USER_ID", label: "Console user", required: true, secret: false },
    { key: "CONSOLE_LOGIN_PASSWORD", label: "Console password", required: true, secret: true },
  ];
  const receipt = { ...ready, fields, console_scope: { authentication: "local-password", health_scope: "native-bootstrap-and-console" } };
  const values = { CONSOLE_USER_ID: "test reader", CONSOLE_LOGIN_PASSWORD: "short" }, before = structuredClone(receipt);
  assert.deepEqual(remotePrivateOptions(receipt).console_scope, { authentication: "local-password" });
  assert.equal(remoteCanInstall(receipt, options, checked, guide, values), false);
  const valid = { CONSOLE_USER_ID: "test-reader", CONSOLE_LOGIN_PASSWORD: "  exact synthetic password  " };
  assert.equal(remoteCanInstall(receipt, options, checked, guide, valid), true);
  assert.deepEqual(remoteInstallSettings(remotePrivateOptions(receipt), valid), valid);
  assert.equal(remoteCanInstall({ ...receipt, console_scope: undefined }, options, checked, guide, values), false);
  const absentScope = { ...receipt }; delete absentScope.console_scope;
  assert.equal(remoteCanInstall(absentScope, options, checked, guide, values), true);
  assert.equal(remoteCanInstall({ ...receipt, console_scope: { authentication: "loopback-only" } }, options, checked, guide, values), true);
  for (const console_scope of [null, { authentication: "unknown" }, {}])
    assert.equal(remotePrivateOptions({ ...receipt, console_scope }), null);
  for (const fields of [receipt.fields.slice(1), [{ ...receipt.fields[0], required: false }, receipt.fields[1]],
    [receipt.fields[0], { ...receipt.fields[1], secret: false }],
    [receipt.fields[0], { ...receipt.fields[1], default: null }]])
    assert.equal(remotePrivateOptions({ ...receipt, fields }), null);
  assert.deepEqual(receipt, before);
});

test("initial Console port selection needs exact current source and never-admitted custody", () => {
  assert.equal(remoteSetupConsolePort(initialPortSetup, portOptions, checked, guide), 18088);
  assert.equal(remoteSetupConsolePortValue(initialPortSetup, portOptions, checked, guide, {}), "18088");
  assert.equal(remoteSetupConsolePortValue(initialPortSetup, portOptions, checked, guide, { MOBKIT_PORT: "" }), "");
  for (const changed of [{ ...portOptions, source_digest: undefined }, { ...portOptions, source_digest: "f".repeat(64) },
    { ...portOptions, console_port: "18088" }, { ...portOptions, console_port: 80 },
    { ...portOptions, console_port: 65536 }, { ...portOptions, current: false }])
    assert.equal(remoteSetupConsolePort(initialPortSetup, changed, checked, guide), null);
  for (const changed of [{ ...initialPortSetup, remote_admission_requested: undefined },
    { ...initialPortSetup, remote_admission_requested: true }, { ...initialPortSetup, admitted: true },
    { ...initialPortSetup, guide_version: guide.version - 1 }, retryableFailure,
    { ...initialPortSetup, status: "stopped" }])
    assert.equal(remoteSetupConsolePort(changed, portOptions, checked, guide), null);
  assert.equal(remoteSetupConsolePort(initialPortSetup, portOptions, checked, guide, true), null);
  const values = { APP_USER: "test-user", APP_TOKEN: "exact-private-value", MOBKIT_PORT: "18089" };
  assert.deepEqual(remoteSetupInstallSettings(initialPortSetup, portOptions, checked, guide, values), values);
  assert.deepEqual(remoteSetupInstallSettings(initialPortSetup, portOptions, checked, guide,
    { APP_USER: values.APP_USER, APP_TOKEN: values.APP_TOKEN }), { APP_USER: values.APP_USER, APP_TOKEN: values.APP_TOKEN, MOBKIT_PORT: "18088" });
  assert.deepEqual(remoteSetupInstallSettings(retryableFailure, portOptions, checked, guide, values),
    { APP_USER: values.APP_USER, APP_TOKEN: values.APP_TOKEN }, "Retry never invents a compiled-default override.");
  assert.equal(remoteCanInstall(initialPortSetup, { ...portOptions, console_port: undefined }, checked, guide, values), false,
    "Missing port metadata cannot silently discard a draft override.");
  assert.equal(remoteCanInstall(initialPortSetup, { ...portOptions, console_port: undefined }, checked, guide,
    { APP_USER: values.APP_USER, APP_TOKEN: values.APP_TOKEN }), false,
    "A never-admitted setup cannot bypass the check before a port draft exists.");
  for (const MOBKIT_PORT of ["", "08088", " 18088", "18088 ", "1023", "65536", "18088.0", "1e4", 18088])
    assert.equal(remoteCanInstall(initialPortSetup, portOptions, checked, guide, { ...values, MOBKIT_PORT }), false);
  for (const MOBKIT_PORT of ["1024", "18089", "65535"])
    assert.equal(remoteCanInstall(initialPortSetup, portOptions, checked, guide, { ...values, MOBKIT_PORT }), true);
});

test("local availability is source-bound and cannot stand for remote availability", () => {
  const parse = (candidate, value = "18088", receipt = initialPortSetup) =>
    remoteSetupPortObservation(candidate, receipt, portOptions, checked, guide, value);
  assert.deepEqual(parse(localPortOptions(18088, "occupied", 18089)),
    { port: 18088, status: "occupied", alternatePort: 18089 });
  for (const changed of [{ source_digest: "f".repeat(64) }, { source_digest: undefined },
    { build_id: "different" }, { guide_version: 3 }, { console_port: 18089 }, { current: false }])
    assert.equal(parse({ ...localPortOptions(18088), ...changed }), null);
  for (const changed of [{ scope: "remote-console-port-observation" }, { port: 18089 },
    { port: "18088" }, { status: "ready" }, { observed_at: "invalid" }])
    assert.equal(parse({ ...portOptions, console_port_observation: { ...localPortOptions(18088).console_port_observation, ...changed } }), null);
  assert.equal(parse(localPortOptions(18088), "18089"), null);
  assert.equal(parse(localPortOptions(18088), "18088", retryableFailure), null);
  for (const alternate of [80, 65536, "18089", 18088, null])
    assert.equal(parse(localPortOptions(18088, "occupied", alternate)).alternatePort, null);
});

test("preflight checks stay destination evidence while distinct deployment and admitted checks remain visible", () => {
  const preflight = { ...ready, checks: structuredClone(ready.preflight.checks) };
  assert.deepEqual(remoteDeploymentChecks(preflight), []);
  const failure = { name: "Accepted source", status: "failed", detail: "Synthetic changed source" };
  assert.deepEqual(remoteDeploymentChecks({ ...preflight, checks: [...preflight.checks, failure] }), [failure]);
  assert.deepEqual(remoteDeploymentChecks({ ...preflight, admitted: true }), preflight.checks);
  assert.deepEqual(preflight.checks, ready.preflight.checks);
});

test("activity collapses adjacent unchanged observation times without erasing transitions or distinct failure evidence", () => {
  const message = "The destination checks passed. Enter private setup to install.";
  const first = { at: "synthetic-time-1", message }, later = { at: "synthetic-time-2", message };
  const failed = { at: "synthetic-time-3", message: "Dependency check failed.", phase: "dependencies", operation_id: failedOperation };
  const otherFailure = { ...failed, at: "synthetic-time-4", operation_id: retryOperation };
  const progress = [first, later, failed, otherFailure, { ...later, at: "synthetic-time-5" }];
  assert.deepEqual(remoteDeploymentActivity(progress), [first, failed, otherFailure, progress[4]]);
  assert.equal(progress.length, 5); assert.equal(progress[1], later);
  assert.deepEqual(remoteDeploymentActivity(null), []);
});

test("remote connect requires current trusted Linux acceptance and never becomes local or cloud admission", () => {
  assert.equal(remoteCanConnect(options, checked, guide), true);
  for (const changed of [{ ...options, supported: true }, { ...options, host_target: "linux" },
    { ...options, target: "gcp" }, { ...options, current: false }, { ...options, build_id: "another" }])
    assert.equal(remoteCanConnect(changed, checked, guide), false);
  for (const changed of [{ ...checked, acceptance_trusted: false }, { ...checked, acceptance_current: false },
    { ...checked, source_digest: null }, { ...checked, status: "running" }])
    assert.equal(remoteCanConnect(options, changed, guide), false);
  assert.equal(remoteCanConnect(options, checked, guide, true), false);
});

test("destination input is explicit and bounded without shell syntax or SSH passwords", () => {
  assert.deepEqual(remoteConnectionInput({ ...endpoint, destination: "~/src/workshop" }), { ...endpoint, destination: "~/src/workshop" });
  assert.deepEqual(remoteConnectionInput({ host: "[::1]", user: "staff-user", ssh_port: "2201", destination }),
    { host: "[::1]", user: "staff-user", ssh_port: 2201, destination });
  for (const changed of [{ host: "-oProxyCommand=anything" }, { host: "host name" }, { user: "user;command" },
    { ssh_port: "22;command" }, { ssh_port: "0" }, { ssh_port: "65536" }, { destination: "relative/project" },
    { destination: "/folder\ncommand" }])
    assert.equal(remoteConnectionInput({ ...endpoint, destination, ...changed }), null);
  assert.equal(remoteDeploymentUrl("guide /1", "connect"), "/api/guide/guide%20%2F1/remote-deployment/connect");
});

test("native team options cannot admit a custom remote application", () => {
  const nativeGuide = { ...guide, proposal: undefined, blueprint: { target: "linux" },
    status: "ready", question: null, readiness: { ready: true } };
  const nativeOptions = { ...options, source_kind: "native-team", build_id: undefined,
    source_id: "d".repeat(64), source_digest: "e".repeat(64), current_guide_version: guide.version };
  assert.equal(remoteCanConnect(nativeOptions, checked, nativeGuide), false);
  assert.equal(remoteActionAvailable({ ...ready, status: "running", can_stop: true }, "stop",
    nativeOptions, checked, nativeGuide, true), true);
});

test("historical same-guide receipts remain owned while current-source private access is blocked", () => {
  assert.equal(remoteReceiptOwned(ready, guide), true);
  assert.equal(remoteReceiptCurrent(ready, options, checked, guide), true);
  assert.notEqual(ready.accepted_source_digest, ready.transferred_source_digest);
  const old = { ...ready, source_binding_current: false, status: "running", can_stop: true };
  assert.equal(remoteReceiptOwned(old, guide), true);
  assert.equal(remoteReceiptCurrent(old, options, checked, guide), false);
  assert.equal(remoteActionAvailable(old, "stop", options, checked, guide), true);
  assert.equal(remoteActionAvailable({ ...old, status: "stopped", can_restart: true }, "restart", options, checked, guide), false);
  for (const changed of [{ guide_id: "another" }, { protocol: 2 }, { package_sha256: "invalid" },
    { transferred_source_digest: "invalid" }, { accepted_source_digest: "invalid" }, { endpoint: { ...endpoint, ssh_port: 0 } }])
    assert.equal(remoteReceiptOwned({ ...ready, ...changed }, guide), false);
});

test("private setup uses only verified receipt fields and requires explicit available observation", () => {
  const before = structuredClone(ready), privateOptions = remotePrivateOptions(ready);
  assert.equal(privateOptions.can_use_host_provider_credentials, false);
  assert.deepEqual(privateOptions.host_provider_keys, []);
  assert.equal(privateOptions.fields.every((field) => field.configured === false), true);
  assert.equal(remoteCanConfigure(ready, options, checked, guide), true);
  assert.equal(remoteCanConfigure({ ...ready, observation: undefined }, options, checked, guide), false);
  assert.equal(remoteCanConfigure({ ...ready, preflight: { ready: false } }, options, checked, guide), false);
  assert.equal(remoteCanConfigure({ ...ready, status: "running" }, options, checked, guide), false);
  assert.equal(remoteCanConfigure(ready, options, checked, guide, true), false);
  const values = { APP_USER: "synthetic-user", APP_TOKEN: "fixture-only-token", OPTIONAL_NOTE: "note",
    STALE_OPTION_FIELD: "must-not-submit", HOME: "must-not-submit" };
  assert.equal(remoteCanInstall(ready, options, checked, guide, values), true);
  assert.equal(remoteCanInstall(ready, options, checked, guide, { APP_USER: "synthetic-user" }), false);
  assert.deepEqual(remoteInstallSettings(privateOptions, values), { APP_USER: "synthetic-user", APP_TOKEN: "fixture-only-token", OPTIONAL_NOTE: "note" });
  assert.deepEqual(ready, before);
  for (const fields of [[ready.fields[0], ready.fields[0]], [{ ...ready.fields[0], key: "unsafe-key" }],
    [{ ...ready.fields[1], default: "private-default" }], [{ ...ready.fields[0], required: "true" }]])
    assert.equal(remotePrivateOptions({ ...ready, fields }), null);
});

test("restart needs current source, known observation and an explicit backend capability", () => {
  const stopped = { ...ready, status: "stopped", can_restart: true };
  assert.equal(remoteActionAvailable(stopped, "restart", options, checked, guide), true);
  for (const changed of [{ can_restart: undefined }, { can_restart: false }, { observation: { available: false } },
    { status: "observation-unavailable" }, { status: "recovery-needed" }, { source_binding_current: false }])
    assert.equal(remoteActionAvailable({ ...stopped, ...changed }, "restart", options, checked, guide), false);
  assert.equal(remoteObservationAvailable({ ...ready, status: "observation-unavailable", last_known_status: "running" }), false);
  assert.notEqual(remoteSelectionKey(guide, checked), remoteSelectionKey({ ...guide, version: 5 }, checked));
});

test("authoritative control and not-installed receipts retain their distinct admission and readiness semantics", () => {
  for (const status of ["stopping", "restarting"]) {
    const control = { ...ready, status, can_restart: true };
    assert.equal(remoteReceiptOwned(control, guide), true);
    assert.equal(remoteIsBusy(control), true);
    assert.equal(remoteInstallAdmitted(control), true);
    assert.equal(remoteCanConfigure(control, options, checked, guide), false);
    assert.equal(remoteActionAvailable(control, "restart", options, checked, guide), false);
  }
  const absent = { ...ready, status: "not-installed", admitted: false };
  assert.equal(remoteReceiptOwned(absent, guide), true);
  assert.equal(remoteIsBusy(absent), false);
  assert.equal(remoteInstallAdmitted(absent), false);
  assert.equal(remoteCanConfigure(absent, options, checked, guide), true);
  for (const changed of [{ admitted: undefined }, { admitted: true }, { can_install: false },
    { source_binding_current: false }, { preflight: { ready: false } }, { observation: { available: false } }])
    assert.equal(remoteCanConfigure({ ...absent, ...changed }, options, checked, guide), false);
  const running = { ...ready, status: "running", source_current: true, ready: true };
  assert.equal(remoteNativeReady(running), true);
  for (const changed of [{ source_current: undefined }, { source_current: false }, { ready: false },
    { observation: { available: false } }]) {
    const uncertain = { ...running, ...changed };
    assert.equal(remoteNativeReady(uncertain), false);
    assert.match(remotePhase(uncertain), /verified readiness check/);
  }
});

test("terminal retry requires exact failed-operation authority and verified cleanup rather than generic install permission", () => {
  assert.equal(remoteCanRetryInstall(retryableFailure, options, checked, guide), true);
  assert.equal(remoteCanConfigure(retryableFailure, options, checked, guide), true);
  assert.equal(remoteCanInstall(retryableFailure, options, checked, guide,
    { APP_USER: "fresh-user", APP_TOKEN: "fresh-fixture-token" }), true);
  for (const changed of [{ can_retry_install: false }, { can_retry_install: undefined },
    { retry_of_operation_id: undefined }, { retry_of_operation_id: otherOperation }, { operation_id: "not-a-uuid" },
    { worker_live: true }, { worker_live: undefined }, { cleanup_confirmed: false }, { cleanup_confirmed: undefined },
    { source_current: false }, { source_current: undefined }, { source_binding_current: false },
    { observation: { available: false } }, { preflight: { ready: false } },
    { status: "installing" }, { status: "recovery-needed" }, { status: "observation-unavailable" }]) {
    const receipt = { ...retryableFailure, can_install: true, ...changed };
    assert.equal(remoteCanRetryInstall(receipt, options, checked, guide), false);
    assert.equal(remoteCanConfigure(receipt, options, checked, guide), false);
  }
  assert.equal(remoteCanRetryInstall(retryableFailure, options, checked, guide, true), false);
  assert.notEqual(remotePrivateBinding(retryableFailure), remotePrivateBinding({ ...retryableFailure,
    operation_id: otherOperation, retry_of_operation_id: otherOperation }));
  assert.match(remoteFailureGuidance(retryableFailure).label, /Dependency installation/);
  assert.equal(remoteFailureGuidance({ ...retryableFailure, failure_phase: "private-canary-unknown-phase" }), null);
  assert.equal(remoteFailureGuidance({ ...retryableFailure, failure_phase: "constructor" }), null);
  assert.equal(remoteFailureGuidance({ ...retryableFailure, status: "running" }), null);
});

async function mountedRemote(initialReceipt, verify, initialSelection = {}) {
  const require = createRequire(import.meta.url);
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], polls = new Map(), readDeadlines = new Map();
  const fixtureGuideId = initialSelection.guide?.id || guide.id;
  const originalSetTimeout = globalThis.setTimeout, originalClearTimeout = globalThis.clearTimeout;
  let receipt = initialReceipt && structuredClone(initialReceipt), nextPoll = 0, outage = false;
  let readHandler = null;
  let portCheckHandler = (params) => ({ ...currentOptions, console_port_observation: {
    scope: "local-console-port-observation", port: Number(params.get("console_port")), status: "available", observed_at: "2026-10-06T16:00:00Z" } });
  let connectHandler = () => structuredClone(ready), installHandler = () => ({ ...structuredClone(ready), status: "installing" });
  let retryHandler = () => ({ ...structuredClone(retryableFailure), status: "installing", can_retry_install: false,
    operation_id: retryOperation, previous_operation_id: failedOperation, worker_live: true });
  let actionHandler = (body) => ({ ...receipt, status: body.action === "stop" ? "stopped" : "starting" });
  let settingsReadHandler = () => savedSettings(receipt, currentGuide.version);
  let settingsBodyHandler = null;
  let settingsSaveHandler = () => ({ ...receipt, configuration_saved: true });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage,
    sessionStorage: dom.window.sessionStorage, IS_REACT_ACT_ENVIRONMENT: true,
    setTimeout(callback, delay, ...args) {
      if (delay === 30000) {
        const token = { syntheticReadDeadline: ++nextPoll };
        readDeadlines.set(token, () => callback(...args));
        return token;
      }
      if (delay !== 2000) return originalSetTimeout(callback, delay, ...args);
      const token = { syntheticPoll: ++nextPoll };
      polls.set(token, () => callback(...args));
      return token;
    },
    clearTimeout(token) { if (!polls.delete(token) && !readDeadlines.delete(token)) originalClearTimeout(token); },
    fetch: async (target, init = {}) => {
      const method = init.method || "GET", body = init.body ? JSON.parse(init.body) : null;
      calls.push({ target, method, body, signal: init.signal });
      if (method === "GET" && target.startsWith(`${remoteDeploymentUrl(fixtureGuideId, "settings")}?`)) {
        assert.equal(new URL(target, "http://localhost").searchParams.get("deployment_id"), receipt.deployment_id);
        const next = await settingsReadHandler();
        return { ok: true, json: () => settingsBodyHandler ? settingsBodyHandler(next) : Promise.resolve(structuredClone(next)) };
      }
      if (method === "POST" && target === remoteDeploymentUrl(fixtureGuideId, "settings")) {
        const next = await settingsSaveHandler(body); receipt = structuredClone(next);
        return { ok: true, json: async () => structuredClone(next) };
      }
      if (method === "GET" && target.startsWith(`/api/guide/${fixtureGuideId}/deployment/options?`)) {
        const next = await portCheckHandler(new URL(target, "http://localhost").searchParams);
        return { ok: true, json: async () => structuredClone(next) };
      }
      if (method === "GET" && initialSelection.firstUseMetadata &&
          target === remoteInstalledUseUrl(fixtureGuideId, initialSelection.firstUseMetadata.deployment_id)) {
        return { ok: true, json: async () => structuredClone(initialSelection.firstUseMetadata) };
      }
      if (method === "GET" && target === remoteDeploymentUrl(fixtureGuideId)) {
        if (readHandler) {
          const selected = await readHandler();
          if (selected) return { ok: true, json: async () => structuredClone(selected) };
        }
        if (outage) throw new Error("Synthetic status transport is unavailable.");
        return receipt ? { ok: true, json: async () => structuredClone(receipt) }
          : { ok: false, status: 404, json: async () => ({ detail: "No saved remote deployment." }) };
      }
      if (method === "POST" && target === remoteDeploymentUrl(fixtureGuideId, "connect")) {
        const next = await connectHandler(body); receipt = structuredClone(next);
        return { ok: true, json: async () => structuredClone(next) };
      }
      if (method === "POST" && target === remoteDeploymentUrl(fixtureGuideId, "install")) {
        const next = await installHandler(body); receipt = structuredClone(next);
        return { ok: true, json: async () => structuredClone(next) };
      }
      if (method === "POST" && target === remoteDeploymentUrl(fixtureGuideId, "retry")) {
        const next = await retryHandler(body); receipt = structuredClone(next);
        return { ok: true, json: async () => structuredClone(next) };
      }
      if (method === "POST" && target === remoteDeploymentUrl(fixtureGuideId, "action")) {
        assert.equal(body.deployment_id, receipt.deployment_id);
        assert.ok(["stop", "restart"].includes(body.action));
        receipt = structuredClone(await actionHandler(body));
        return { ok: true, json: async () => structuredClone(receipt) };
      }
      assert.fail(`Unexpected synthetic remote request: ${method} ${target}`);
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  const built = await bundle({ stdin: { contents: 'export { RemoteDeployment } from "./remote-deployment.jsx"; export { DeploymentConfigurationFields } from "./deployment-panel.jsx";',
      resolveDir: fileURLToPath(new URL(".", import.meta.url)), sourcefile: "synthetic-remote-component-entry.mjs" },
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
    nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
    plugins: [{ name: "shared-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
    } }] });
  const { RemoteDeployment, DeploymentConfigurationFields } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
  const root = createRoot(dom.window.document.getElementById("root"));
  let currentGuide = initialSelection.guide || guide, currentBuild = initialSelection.build || checked,
    currentOptions = initialSelection.options || options, currentBlocked = false;
  const render = async () => act(async () => root.render(React.createElement(RemoteDeployment, {
    state: currentGuide, build: currentBuild, options: currentOptions, blocked: currentBlocked,
    plan: { project: { name: "Workshop", slug: "workshop" } },
    selectionScope: initialSelection.selectionScope,
    renderConfiguration: (props) => React.createElement(DeploymentConfigurationFields, props),
  })));
  const document = dom.window.document;
  const button = (label) => [...document.querySelectorAll("button")].find((node) => node.textContent === label);
  async function enter(name, value) {
    const input = document.querySelector(`[name="${name}"]`);
    assert.ok(input, `Production input ${name} must exist.`);
    const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set;
    await act(async () => { setter.call(input, value); input.dispatchEvent(new dom.window.Event("input", { bubbles: true })); });
    assert.equal(input.value, value);
  }
  async function poll(nextReceipt, unavailable = false) {
    receipt = nextReceipt && structuredClone(nextReceipt); outage = unavailable;
    const first = polls.entries().next().value;
    assert.ok(first, "Production remote status polling must be scheduled.");
    polls.delete(first[0]);
    await act(async () => first[1]());
  }
  try {
    await render();
    await verify({ document, window: dom.window, calls, button, enter, poll,
      setConnect: (handler) => { connectHandler = handler; }, setInstall: (handler) => { installHandler = handler; },
      setRetry: (handler) => { retryHandler = handler; },
      setAction: (handler) => { actionHandler = handler; },
      setSettingsRead: (handler) => { settingsReadHandler = handler; },
      setSettingsBody: (handler) => { settingsBodyHandler = handler; },
      setSettingsSave: (handler) => { settingsSaveHandler = handler; },
      async expireRead() {
        const selected = readDeadlines.entries().next().value;
        assert.ok(selected, "A bounded read must retain its own deadline until decoding completes.");
        readDeadlines.delete(selected[0]);
        await act(async () => selected[1]());
      },
      setRead: (handler) => { readHandler = handler; },
      setPortCheck: (handler) => { portCheckHandler = handler; },
      setReceipt: (next) => { receipt = structuredClone(next); },
      async changeSelection(nextGuide, nextBuild, nextOptions) {
        currentGuide = nextGuide; currentBuild = nextBuild; currentOptions = nextOptions; await render();
      },
      async changeBlocked(nextBlocked) { currentBlocked = nextBlocked; await render(); } });
  } finally {
    await act(async () => root.unmount());
    const remaining = polls.size;
    const remainingReadDeadlines = readDeadlines.size;
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
    assert.equal(remaining, 0, "Unmount cancels remote status polling.");
    assert.equal(remainingReadDeadlines, 0, "Completed reads clear their deadlines.");
  }
}

async function chooseDestination(enter) {
  await enter("host", endpoint.host); await enter("user", endpoint.user); await enter("destination", destination);
}

test("installed remote settings save only explicit changes and never restart the stopped application", async () => {
  await mountedRemote(settingsInstallation, async ({ document, calls, button, enter }) => {
    assert.ok(button("Change private settings"));
    await act(async () => button("Change private settings").click());
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').required, false);
    assert.equal(button("Restart accepted application") === undefined, true,
      "Close or save the settings form before starting this application.");
    assert.equal(button("Save private settings").disabled, true);
    await enter("APP_TOKEN", "  exact synthetic replacement  ");
    const remove = document.querySelector('[id$="OPTIONAL_NOTE-remove"]');
    assert.ok(remove);
    await act(async () => remove.click());
    await act(async () => button("Save private settings").click());
    const writes = calls.filter((call) => call.method === "POST");
    assert.equal(writes.length, 1);
    assert.equal(writes[0].target, remoteDeploymentUrl(guide.id, "settings"));
    assert.deepEqual(writes[0].body, { version: guide.version, deployment_id: ready.deployment_id,
      build_id: checked.build_id, source_digest: checked.source_digest, package_sha256: ready.package_sha256,
      settings: { APP_TOKEN: "  exact synthetic replacement  " }, remove_fields: ["OPTIONAL_NOTE"] });
    assert.equal(writes[0].signal, undefined, "Saving does not add an abort deadline.");
    assert.match(document.body.textContent, /Private settings saved\. Your application is still stopped/);
    assert.equal(document.querySelector('[name="APP_TOKEN"]') === null, true);
    assert.equal(button("Restart accepted application").disabled, false);
    assert.equal(document.body.textContent.includes("exact synthetic replacement"), false);
  });
});

test("legacy remote settings support and uncertain observations never offer a fake recovery form", async () => {
  await mountedRemote({ ...settingsInstallation, private_settings_supported: false, can_configure: false },
    async ({ document, calls, button, poll }) => {
      assert.equal(button("Change private settings") === undefined, true);
      assert.match(document.body.textContent, /does not support settings changes in Forge/);
      assert.equal(document.querySelector('[name="APP_TOKEN"]') === null, true);
      await poll({ ...settingsInstallation, private_settings_supported: false, can_configure: false,
        status: "observation-unavailable", observation: { available: false } });
      assert.equal(document.body.textContent.includes("does not support settings changes in Forge"), false,
        "Temporary observation loss cannot establish an unsupported installation.");
      assert.equal(calls.some((call) => call.target.includes("/settings")), false);
    });
});

test("cloud removal closes installed settings and restores the parent's status recovery control", async () => {
  const cloudGuide = { ...guide, proposal: { target: "gcp" } };
  const cloudOptions = { ...options, target: "gcp" };
  const cloudEndpoint = { transport: "gcp-iap", operation_id: failedOperation, account: "fixture@example.test",
    project: "fixture-project", zone: "europe-west1-b", instance: "fixture-app", instance_id: "1234",
    ownership_token: "e".repeat(32), host: "compute.1234", user: "mobkit", ssh_port: 22 };
  const cloudReceipt = { ...settingsInstallation, target: "gcp", cloud_operation_id: failedOperation, endpoint: cloudEndpoint };
  await mountedRemote(cloudReceipt, async ({ document, button, enter, poll, calls }) => {
    await act(async () => button("Change private settings").click());
    await enter("APP_TOKEN", "synthetic unsent cloud edit");
    assert.equal(button("Restart accepted application") === undefined, true);
    await poll({ ...cloudReceipt, cloud_status: "removing" });
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(button("Recheck remote status").disabled, false,
      "Unmounting the editor must leave a way to observe the retained installation.");
    assert.equal(calls.some((call) => call.method === "POST"), false);
    assert.equal(document.body.innerHTML.includes("synthetic unsent cloud edit"), false);
  }, { guide: cloudGuide, options: cloudOptions });
});

test("a lost remote settings response preserves the draft without converting status or presence into save confirmation", async () => {
  await mountedRemote(settingsInstallation, async ({ document, calls, button, enter, poll, setSettingsSave }) => {
    await act(async () => button("Change private settings").click());
    await enter("APP_TOKEN", "synthetic uncertain replacement");
    setSettingsSave(() => { throw new Error("Synthetic lost response"); });
    await act(async () => button("Save private settings").click());
    assert.match(document.body.textContent, /could not confirm this save/);
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "synthetic uncertain replacement");
    assert.equal(button("Save private settings") === undefined, true);
    await poll(settingsInstallation);
    await act(async () => button("Reload private settings").click());
    assert.match(document.body.textContent, /checking status does not confirm that these exact values were saved/);
    assert.equal(document.body.textContent.includes("Private settings saved."), false);
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "synthetic uncertain replacement");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(document.defaultView.localStorage.length, 0);
    assert.equal(document.defaultView.sessionStorage.length, 0);
  });
});

test("remote settings reads preserve unsent replacements across outages and withhold saving until a fresh read", async () => {
  await mountedRemote(settingsInstallation, async ({ document, button, enter, setSettingsRead, changeSelection, poll, calls }) => {
    await act(async () => button("Change private settings").click());
    await enter("APP_TOKEN", "synthetic unsent setting");
    const nextGuide = { ...guide, version: guide.version + 1 };
    const nextReceipt = { ...settingsInstallation, current_guide_version: nextGuide.version };
    await changeSelection(nextGuide, { ...checked, current_guide_version: nextGuide.version },
      { ...options, current_guide_version: nextGuide.version });
    await poll(nextReceipt);
    setSettingsRead(() => { throw new Error("Synthetic read unavailable"); });
    await act(async () => button("Reload private settings").click());
    assert.match(document.body.textContent, /Private settings could not be checked/);
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "synthetic unsent setting");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').disabled, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    setSettingsRead(() => savedSettings(nextReceipt, nextGuide.version));
    await act(async () => button("Reload private settings").click());
    assert.equal(document.querySelector('[name="APP_TOKEN"]').disabled, false);
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "synthetic unsent setting");
    assert.equal(button("Save private settings").disabled, false);
  });
});

test("late remote settings success and failure cannot replace a draft after selecting the same setup again", async () => {
  for (const settlement of ["success", "failure"]) {
    let token = 1, pending = false;
    const scope = { token: () => token, capture(selected, expected) {
      if (pending || selected.id !== guide.id || expected !== undefined && expected !== token) return null;
      const captured = token;
      return () => !pending && token === captured;
    } };
    await mountedRemote(settingsInstallation, async ({ document, calls, button, enter, setSettingsSave, changeSelection, poll }) => {
      await act(async () => button("Change private settings").click());
      await enter("APP_TOKEN", "synthetic earlier replacement");
      let resolve, reject;
      setSettingsSave(() => new Promise((yes, no) => { resolve = yes; reject = no; }));
      await act(async () => button("Save private settings").click());
      assert.match(document.body.textContent, /The original request may still finish; reopening does not send it again/);
      token += 1; pending = true;
      await changeSelection(guide, checked, options);
      assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "synthetic earlier replacement");
      pending = false;
      await changeSelection(guide, checked, options);
      await poll(settingsInstallation);
      await act(async () => button("Reload private settings").click());
      await enter("APP_TOKEN", "synthetic newer unsent replacement");
      await act(async () => {
        if (settlement === "success") resolve({ ...settingsInstallation, configuration_saved: true });
        else reject(new Error("Synthetic late failure"));
      });
      assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "synthetic newer unsent replacement");
      assert.equal(document.body.textContent.includes("Private settings saved."), false);
      assert.equal(document.body.textContent.includes("could not confirm this save"), false);
      assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    }, { selectionScope: scope });
  }
});

test("a held remote settings JSON read expires without losing the draft or admitting a stale body", async () => {
  await mountedRemote(settingsInstallation, async ({ document, calls, button, enter, changeSelection, poll,
    setSettingsRead, setSettingsBody, expireRead }) => {
    await act(async () => button("Change private settings").click());
    await enter("APP_TOKEN", "synthetic retained draft");
    const nextGuide = { ...guide, version: guide.version + 1 };
    const nextReceipt = { ...settingsInstallation, current_guide_version: nextGuide.version };
    await changeSelection(nextGuide, { ...checked, current_guide_version: nextGuide.version },
      { ...options, current_guide_version: nextGuide.version });
    await poll(nextReceipt);
    setSettingsRead(() => savedSettings(nextReceipt, nextGuide.version));
    let resolve;
    setSettingsBody(() => new Promise((yes) => { resolve = yes; }));
    await act(async () => button("Reload private settings").click());
    assert.equal(button("Loading private settings...").disabled, true);
    await expireRead();
    assert.match(document.body.textContent, /Private settings could not be checked/);
    assert.equal(button("Reload private settings").disabled, false);
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "synthetic retained draft");
    const heldRequest = calls.filter((call) => call.method === "GET" && call.target.includes("/settings?")).at(-1);
    assert.equal(heldRequest.signal.aborted, true);
    setSettingsBody(null);
    await act(async () => button("Reload private settings").click());
    await act(async () => resolve({ ...savedSettings(), fields: [{ key: "LATE_BODY", label: "Stale read", required: true, secret: true,
      configured: false, removable: false }] }));
    assert.equal(document.querySelector('[name="LATE_BODY"]') === null, true);
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "synthetic retained draft");
    assert.equal(button("Save private settings").disabled, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});

test("an occupied initial Console port offers a checked alternative without losing private draft or touching the existing app", async () => {
  await mountedRemote(initialPortSetup, async ({ document, window, calls, button, enter, setPortCheck, poll, setInstall }) => {
    setPortCheck((params) => localPortOptions(Number(params.get("console_port")),
      params.get("console_port") === "18088" ? "occupied" : "available", 18089));
    await act(async () => button("Continue to private setup").click());
    assert.equal(document.querySelector('[name="MOBKIT_PORT"]').value, "18088");
    assert.match(document.body.textContent, /An app on this computer already uses this port/);
    await enter("APP_USER", "test-volunteer"); await enter("APP_TOKEN", "private-port-test-token");
    assert.equal(button("Install and start on Linux").disabled, true);
    await act(async () => button("Use port 18089").click());
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "private-port-test-token");
    assert.equal(button("Install and start on Linux").disabled, false);
    const checks = calls.filter((call) => call.target.includes("/deployment/options?"));
    assert.equal(checks.length, 2);
    for (const call of checks) {
      assert.equal(call.method, "GET"); assert.equal(call.body, null);
      assert.equal(new URL(call.target, "http://localhost").searchParams.get("build_id"), checked.build_id);
      assert.equal(call.target.includes("private-port-test-token"), false);
    }
    await poll(initialPortSetup);
    assert.equal(document.querySelector('[name="MOBKIT_PORT"]').value, "18089", "Passive polls do not reset the chosen port.");
    assert.equal(calls.filter((call) => call.target.includes("/deployment/options?")).length, 2);
    setInstall((body) => {
      assert.deepEqual(body.settings, { APP_USER: "test-volunteer", APP_TOKEN: "private-port-test-token", MOBKIT_PORT: "18089" });
      return { ...initialPortSetup, status: "installing", remote_admission_requested: true, admitted: true };
    });
    await act(async () => button("Install and start on Linux").click());
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(window.localStorage.length, 0); assert.equal(window.sessionStorage.length, 0);
  }, { options: portOptions });
});

test("empty and invalid Console overrides cannot silently reinstall the compiled default", async () => {
  await mountedRemote(initialPortSetup, async ({ calls, button, enter, document }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "test-volunteer"); await enter("APP_TOKEN", "private-port-test-token");
    const before = calls.length;
    for (const value of ["", "65536", "08088", "18088;command"]) {
      await enter("MOBKIT_PORT", value);
      assert.equal(button("Install and start on Linux").disabled, true);
      assert.match(document.querySelector(".remote-console-port").textContent, /Choose a port from 1024 to 65535/);
      await act(async () => document.querySelector(".remote-private-form").dispatchEvent(new document.defaultView.Event("submit", { bubbles: true, cancelable: true })));
    }
    assert.equal(calls.length, before, "Invalid values cause neither checks nor installation requests.");
    await enter("MOBKIT_PORT", "18089");
    assert.equal(button("Install and start on Linux").disabled, false);
  }, { options: portOptions });
});

test("a check for an earlier port cannot enable installation or replace the current suggestion", async () => {
  await mountedRemote(initialPortSetup, async ({ button, enter, setPortCheck, document, calls }) => {
    let finishOld;
    setPortCheck((params) => params.get("console_port") === "18088"
      ? new Promise((resolve) => { finishOld = resolve; }) : localPortOptions(18089, "occupied", 18090));
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "test-volunteer"); await enter("APP_TOKEN", "private-port-test-token");
    assert.equal(button("Install and start on Linux").disabled, true);
    await enter("MOBKIT_PORT", "18089");
    await act(async () => finishOld(localPortOptions(18088)));
    assert.ok(button("Use port 18090"));
    assert.equal(button("Install and start on Linux").disabled, true);
    assert.equal(document.querySelector('[name="MOBKIT_PORT"]').value, "18089");
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { options: portOptions });
});

test("unavailable and incorrectly bound local checks keep the draft and offer deliberate recheck", async () => {
  for (const failure of ["network", "unknown", "wrong-source"]) await mountedRemote(initialPortSetup,
    async ({ document, button, enter, setPortCheck, calls }) => {
      setPortCheck(() => {
        if (failure === "network") throw new Error("Synthetic check lost.");
        return failure === "unknown" ? localPortOptions(18088, "unknown")
          : { ...localPortOptions(18088), source_digest: "f".repeat(64) };
      });
      await act(async () => button("Continue to private setup").click());
      await enter("APP_USER", "test-volunteer"); await enter("APP_TOKEN", "private-port-test-token");
      assert.equal(button("Install and start on Linux").disabled, true);
      assert.match(document.querySelector(".remote-console-port").textContent, /port could not be checked/);
      setPortCheck(() => localPortOptions(18088));
      await act(async () => button("Check this port again").click());
      assert.equal(button("Install and start on Linux").disabled, false);
      assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "private-port-test-token");
      assert.equal(calls.some((call) => call.method === "POST"), false);
    }, { options: portOptions });
});

test("closed or superseded private forms ignore late port checks and never restore private values", async () => {
  for (const change of ["cancel", "source", "observation"]) await mountedRemote(initialPortSetup,
    async ({ document, button, enter, setPortCheck, changeSelection, poll, calls }) => {
      let finish;
      setPortCheck(() => new Promise((resolve) => { finish = resolve; }));
      await act(async () => button("Continue to private setup").click());
      await enter("APP_TOKEN", "private-port-test-token");
      if (change === "cancel") await act(async () => button("Cancel private setup").click());
      else if (change === "source") await changeSelection(guide, updatedBuild,
        { ...portOptions, build_id: updatedBuild.build_id, source_digest: updatedBuild.source_digest });
      else await poll({ ...initialPortSetup, status: "observation-unavailable", last_known_status: "configuration-required", observation: { available: false } });
      await act(async () => finish(localPortOptions(18088, "occupied", 18089)));
      assert.equal(document.querySelector(".remote-private-form") === null, true);
      assert.equal(button("Use port 18089") === undefined, true);
      assert.equal(calls.some((call) => call.method === "POST"), false);
      assert.equal(document.body.innerHTML.includes("private-port-test-token"), false);
    }, { options: portOptions });
});

test("lost port metadata cannot silently enable a default installation or erase the selected draft", async () => {
  await mountedRemote(initialPortSetup, async ({ document, button, enter, changeSelection, calls }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "test-volunteer"); await enter("APP_TOKEN", "private-port-test-token");
    await enter("MOBKIT_PORT", "18089");
    await changeSelection(guide, checked, { ...portOptions, console_port: undefined });
    assert.equal(button("Install and start on Linux").disabled, true);
    assert.match(document.querySelector(".remote-private-form").textContent, /Your port choice is kept in this draft/);
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "private-port-test-token");
    await changeSelection(guide, checked, portOptions);
    assert.equal(document.querySelector('[name="MOBKIT_PORT"]').value, "18089");
    assert.equal(button("Install and start on Linux").disabled, false);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { options: portOptions });
});

test("retry and legacy setup do not introduce an override from the new default port control", async () => {
  for (const receipt of [retryableFailure, { ...ready, remote_admission_requested: true }]) await mountedRemote(receipt,
    async ({ document, button, enter, calls, setRetry, setInstall }) => {
      await act(async () => button(receipt.status === "failed" ? "Prepare another install attempt" : "Continue to private setup").click());
      assert.equal(document.querySelector('[name="MOBKIT_PORT"]') === null, true);
      await enter("APP_USER", "test-volunteer"); await enter("APP_TOKEN", "private-port-test-token");
      const admission = (body) => {
        assert.equal(Object.hasOwn(body.settings, "MOBKIT_PORT"), false);
        return { ...receipt, status: "installing" };
      };
      setRetry(admission); setInstall(admission);
      await act(async () => button(receipt.status === "failed" ? "Retry installation on this Linux computer" : "Install and start on Linux").click());
      assert.equal(calls.filter((call) => call.method === "POST").length, 1);
      assert.equal(calls.some((call) => call.target.includes("/deployment/options?")), false);
    }, { options: portOptions });
});

test("cloud private setup uses the same port choice and withdraws stale checks when cloud removal begins", async () => {
  const cloudGuide = { ...guide, proposal: { target: "gcp" } };
  const cloudOptions = { ...portOptions, target: "gcp" };
  const cloudEndpoint = { transport: "gcp-iap", operation_id: failedOperation, account: "fixture@example.test",
    project: "fixture-project", zone: "europe-west1-b", instance: "fixture-app", instance_id: "1234",
    ownership_token: "e".repeat(32), host: "compute.1234", user: "mobkit", ssh_port: 22 };
  const cloudReceipt = { ...initialPortSetup, target: "gcp", cloud_operation_id: failedOperation, endpoint: cloudEndpoint };
  await mountedRemote(cloudReceipt, async ({ document, button, enter, setPortCheck, poll, calls }) => {
    let finish;
    setPortCheck(() => new Promise((resolve) => { finish = resolve; }));
    await act(async () => button("Continue to private setup").click());
    assert.equal(document.querySelector('[name="MOBKIT_PORT"]').value, "18088");
    await enter("APP_USER", "test-volunteer"); await enter("APP_TOKEN", "private-port-test-token");
    assert.equal(button("Install and start in Google Cloud").disabled, true);
    await poll({ ...cloudReceipt, cloud_status: "removing" });
    await act(async () => finish({ ...localPortOptions(18088, "occupied", 18089), target: "gcp" }));
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(button("Use port 18089") === undefined, true);
    assert.equal(calls.some((call) => call.method === "POST"), false);
    assert.equal(document.body.innerHTML.includes("private-port-test-token"), false);
  }, { guide: cloudGuide, options: cloudOptions });
});

test("an updated accepted build offers a deliberate same-destination check without carrying private drafts", async () => {
  await mountedRemote({ ...ready, remote_admission_requested: false }, async ({ document, window, calls, button, enter, changeSelection, poll, setConnect }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "fixture-only-old-user"); await enter("APP_TOKEN", "fixture-only-old-private-token");
    await enter("host", "unsent-other.local"); await enter("destination", "/synthetic-unsent/other-folder");
    await changeSelection(guide, updatedBuild, updatedOptions);
    await poll(pendingEarlierBuild);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.ok(button("Check the updated application here"));
    assert.match(document.querySelector(".remote-build-handoff").textContent, /fresh private settings/);
    assert.equal(document.querySelector(".remote-destination-change").open, false);
    assert.equal(document.querySelector(".deployment-warning") === null, true);
    assert.equal(calls.some((call) => call.method === "POST"), false);
    setConnect(() => structuredClone(updatedDestination));
    await act(async () => button("Check the updated application here").click());
    const posts = calls.filter((call) => call.method === "POST");
    assert.equal(posts.length, 1);
    assert.equal(posts[0].target, remoteDeploymentUrl(guide.id, "connect"));
    assert.deepEqual(posts[0].body, { version: guide.version, build_id: updatedBuild.build_id, ...endpoint, destination });
    assert.equal(document.querySelector('[name="host"]').value, endpoint.host);
    assert.equal(document.querySelector('[name="destination"]').value, destination);
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
    assert.equal(JSON.stringify(posts).includes("fixture-only-old-private-token"), false);
    assert.equal(document.body.innerHTML.includes("fixture-only-old-private-token"), false);
    assert.equal(window.localStorage.length, 0); assert.equal(window.sessionStorage.length, 0);
    assert.equal(button("Check the updated application here") === undefined, true);
  });
});

test("the pending handoff survives a source-stale preflight failure and waits for the new check", async () => {
  await mountedRemote(pendingEarlierBuild, async ({ document, calls, button, changeSelection, poll, setConnect }) => {
    await changeSelection(guide, updatedBuild, updatedOptions);
    await poll({ ...pendingEarlierBuild, status: "failed", preflight: failedDestinationCheck.preflight,
      detail: "The application changed during the destination check." });
    assert.ok(button("Check the updated application here"));
    assert.equal(document.querySelector(".remote-deployment h3").textContent, "Check your updated application on this computer");
    assert.match(document.body.textContent, /The application changed during the destination check/);
    assert.doesNotMatch(document.body.textContent, /Recover this installation|Your installation needs a recovery step|Cleanup of the previous attempt/);
    assert.equal(calls.some((call) => call.method === "POST"), false);
    setConnect(() => ({ ...updatedDestination, status: "connecting", observation: { available: false }, preflight: null }));
    await act(async () => button("Check the updated application here").click());
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "connecting");
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    await poll(updatedDestination);
    assert.ok(button("Continue to private setup"));
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("unavailable observation and design edits withhold the pending handoff without sending anything", async () => {
  await mountedRemote(pendingEarlierBuild, async ({ calls, button, changeSelection, changeBlocked, poll }) => {
    await changeSelection(guide, updatedBuild, updatedOptions);
    assert.ok(button("Check the updated application here"));
    await changeBlocked(true); assert.equal(button("Check the updated application here") === undefined, true);
    await changeBlocked(false); assert.ok(button("Check the updated application here"));
    await poll(pendingEarlierBuild, true); assert.equal(button("Check the updated application here") === undefined, true);
    await poll(pendingEarlierBuild); assert.ok(button("Check the updated application here"));
    await poll({ ...pendingEarlierBuild, status: "observation-unavailable", observation: { available: false } });
    assert.equal(button("Check the updated application here") === undefined, true);
    await poll({ ...pendingEarlierBuild, remote_admission_requested: true, admitted: true });
    assert.equal(button("Check the updated application here") === undefined, true);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  });
});

test("double clicks share one explicit pending handoff and a changed receipt rejects its delayed response", async () => {
  await mountedRemote(pendingEarlierBuild, async ({ document, calls, button, changeSelection, poll, setConnect }) => {
    await changeSelection(guide, updatedBuild, updatedOptions);
    let resolveConnect;
    setConnect(() => new Promise((resolve) => { resolveConnect = resolve; }));
    const handoff = button("Check the updated application here");
    await act(async () => { handoff.click(); handoff.click(); });
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    const other = { ...pendingEarlierBuild, deployment_id: "synthetic-other-pending", endpoint: { ...endpoint, host: "other.local" },
      destination: "/synthetic-other/folder" };
    await poll(other);
    await act(async () => resolveConnect(updatedDestination));
    assert.match(document.querySelector(".deployment-location").textContent, /other.local/);
    assert.match(document.querySelector(".deployment-location").textContent, /synthetic-other\/folder/);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
  });
});

test("a paused-and-returned build or design cannot adopt an older pending-handoff response", async () => {
  for (const pause of ["build", "design"]) {
    await mountedRemote(pendingEarlierBuild, async ({ document, button, changeSelection, changeBlocked, setConnect }) => {
      await changeSelection(guide, updatedBuild, updatedOptions);
      let resolveConnect;
      setConnect(() => new Promise((resolve) => { resolveConnect = resolve; }));
      await act(async () => button("Check the updated application here").click());
      if (pause === "build") {
        const other = { ...updatedBuild, build_id: "synthetic-later-build", source_digest: "f".repeat(64) };
        await changeSelection(guide, other, { ...updatedOptions, build_id: other.build_id });
        await changeSelection(guide, updatedBuild, updatedOptions);
      } else { await changeBlocked(true); await changeBlocked(false); }
      await act(async () => resolveConnect(updatedDestination));
      assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "configuration-required");
      assert.equal(document.querySelector(".remote-private-form") === null, true);
      assert.ok(button("Check the updated application here"));
    });
  }
});

test("a pending handoff never adopts a response for another folder, endpoint, build or old installation", async () => {
  for (const change of [{ destination: "/synthetic-other/folder" }, { endpoint: { ...endpoint, host: "other.local" } },
    { build_id: checked.build_id }, { accepted_source_digest: checked.source_digest }, { deployment_id: ready.deployment_id }]) {
    await mountedRemote(pendingEarlierBuild, async ({ document, button, changeSelection, setConnect }) => {
      await changeSelection(guide, updatedBuild, updatedOptions);
      setConnect(() => ({ ...updatedDestination, ...change }));
      await act(async () => button("Check the updated application here").click());
      assert.equal(document.querySelector(".remote-private-form") === null, true);
      assert.match(document.querySelector(".deployment-location").textContent, /workshop.local/);
      assert.match(document.querySelector(".deployment-location").textContent, /synthetic-owned\/src\/workshop/);
      assert.ok(button("Check the updated application here"));
    });
  }
});

test("a failed explicit updated check preserves its destination and needs another deliberate click", async () => {
  await mountedRemote(pendingEarlierBuild, async ({ document, calls, button, changeSelection, setConnect }) => {
    await changeSelection(guide, updatedBuild, updatedOptions);
    setConnect(() => { throw new Error("Synthetic updated destination check failed."); });
    await act(async () => button("Check the updated application here").click());
    assert.match(document.body.textContent, /Synthetic updated destination check failed/);
    assert.equal(document.querySelector('[name="host"]').value, endpoint.host);
    assert.equal(document.querySelector('[name="destination"]').value, destination);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.ok(button("Check the updated application here"));
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("unaccepted or stale builds without an owned receipt withhold SSH fields until current acceptance", async () => {
  for (const build of [
    { ...checked, status: "running", ready_to_package: false, acceptance_trusted: false, acceptance: { status: "pending" } },
    { ...checked, current: false, acceptance_current: false },
    { ...checked, guide_version: guide.version - 1 },
  ]) await mountedRemote(null, async ({ document, calls, button, changeSelection, enter }) => {
    assert.equal(document.querySelector(".remote-destination-form") === null, true);
    for (const name of ["host", "user", "destination", "ssh_port"])
      assert.equal(document.querySelector(`[name="${name}"]`) === null, true);
    assert.equal(button("Connect and check destination") === undefined, true);
    assert.ok(calls.every((call) => call.method === "GET" && call.target === remoteDeploymentUrl(guide.id)));
    await changeSelection(guide, checked, options);
    assert.ok(document.querySelector(".remote-destination-form"));
    assert.ok(button("Connect and check destination"));
    await chooseDestination(enter);
    assert.equal(button("Connect and check destination").disabled, false);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { build });
});

test("mounted wizard waits for explicit connection and ready preflight, then uses exact receipt fields", async () => {
  await mountedRemote(null, async ({ document, calls, button, enter }) => {
    assert.equal(calls.some((call) => call.method === "POST"), false);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(document.querySelector('input[type="password"]') === null, true);
    await chooseDestination(enter);
    assert.equal(calls.some((call) => call.method === "POST"), false);
    await act(async () => button("Connect and check destination").click());
    const posts = calls.filter((call) => call.method === "POST");
    assert.equal(posts.length, 1);
    assert.deepEqual(posts[0].body, { version: guide.version, build_id: checked.build_id, ...endpoint, destination });
    assert.ok(document.querySelector(".remote-private-form"));
    assert.ok(document.querySelector('[name="APP_USER"]'));
    assert.equal(document.querySelector('[name="APP_TOKEN"]').type, "password");
    assert.equal(document.querySelector('[name="STALE_OPTION_FIELD"]') === null, true);
    assert.equal(button("Install and start on Linux").disabled, true);
    assert.equal(document.querySelector(".deployment-optional-settings").open, false);
    assert.equal(document.querySelector('.remote-private-form input[type="checkbox"]') === null, true);
    assert.match(document.querySelector('[name="host"]').closest("label").textContent, /^Computer address$/);
  });
});

test("lost install response is reconciled through authoritative polling and private values never reappear", async () => {
  await mountedRemote(ready, async ({ document, window, calls, button, enter, poll, setInstall, setReceipt }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "synthetic-staff"); await enter("APP_TOKEN", "fixture-only-token");
    setInstall((body) => {
      assert.deepEqual(body, { version: guide.version, build_id: checked.build_id, deployment_id: ready.deployment_id,
        settings: { APP_USER: "synthetic-staff", APP_TOKEN: "fixture-only-token" } });
      setReceipt({ ...ready, status: "installing" });
      throw new Error("Synthetic response was lost after admission.");
    });
    await act(async () => button("Install and start on Linux").click());
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "installing");
    assert.doesNotMatch(document.body.textContent, /could not confirm the installation response/);
    assert.equal(document.body.innerHTML.includes("fixture-only-token"), false);
    assert.equal(window.localStorage.length, 0); assert.equal(window.sessionStorage.length, 0);
    await poll({ ...ready, status: "running" });
    assert.match(document.body.textContent, /phone access.*have not been verified/);
    assert.equal(document.querySelector(".remote-first-use a") === null, true);
    await poll({ ...ready, status: "stopped", can_restart: true });
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.ok(button("Restart accepted application"));
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await poll(ready);
    await act(async () => button("Continue to private setup").click());
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
  });
});

test("transport outages pause exact private drafts without losing values or admitting an installation", async () => {
  await mountedRemote(ready, async ({ document, window, calls, button, enter, poll }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "synthetic-draft");
    await enter("APP_TOKEN", "fixture-only-unsent-token");
    await poll(ready, true);
    const form = document.querySelector(".remote-private-form");
    assert.ok(form);
    assert.equal(document.querySelector('[name="APP_USER"]').value, "synthetic-draft");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "fixture-only-unsent-token");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').disabled, true);
    assert.equal(button("Install and start on Linux").disabled, true);
    await act(async () => form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })));
    assert.equal(calls.some((call) => call.method === "POST"), false);
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "configuration-required");
    assert.match(document.body.textContent, /observation is unavailable/);
    assert.match(document.body.textContent, /does not prove the application failed or stopped/);
    assert.match(document.body.textContent, /private setup draft is kept on this page/);
    assert.equal(document.querySelector('[name="host"]').value, endpoint.host);
    assert.equal(document.querySelector('[name="user"]').value, endpoint.user);
    assert.equal(document.querySelector('[name="destination"]').value, destination);
    await poll(ready);
    assert.ok(document.querySelector(".remote-private-form"));
    assert.equal(document.querySelector('[name="APP_USER"]').value, "synthetic-draft");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "fixture-only-unsent-token");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').disabled, false);
    assert.equal(button("Install and start on Linux").disabled, false);
    assert.equal(calls.some((call) => call.method === "POST"), false);
    assert.equal(window.localStorage.length, 0); assert.equal(window.sessionStorage.length, 0);
  });
});

test("an unsent design edit pauses the same private draft and a genuine source change discards it", async () => {
  await mountedRemote(ready, async ({ document, window, calls, button, enter, changeBlocked, changeSelection, poll }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "synthetic-blocked-user"); await enter("APP_TOKEN", "fixture-only-blocked-token");
    await changeBlocked(true);
    const form = document.querySelector(".remote-private-form");
    assert.ok(form); assert.equal(button("Install and start on Linux").disabled, true);
    assert.equal(document.querySelector('[name="APP_TOKEN"]').disabled, true);
    await act(async () => form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })));
    assert.equal(calls.some((call) => call.method === "POST"), false);
    await changeBlocked(false);
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "fixture-only-blocked-token");
    assert.equal(button("Install and start on Linux").disabled, false);
    const nextBuild = { ...checked, build_id: "synthetic-new-build", source_digest: "d".repeat(64) };
    await changeSelection(guide, nextBuild, { ...options, build_id: nextBuild.build_id });
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    await changeSelection(guide, checked, options); await poll(ready);
    await act(async () => button("Continue to private setup").click());
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
    assert.equal(window.localStorage.length, 0); assert.equal(window.sessionStorage.length, 0);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  });
});

test("canonical SSH loss retains only an exact initial unadmitted draft while withholding fields", async () => {
  const initial = { ...ready, remote_admission_requested: false };
  await mountedRemote(initial, async ({ document, window, calls, button, enter, poll }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "synthetic-ssh-user"); await enter("APP_TOKEN", "fixture-only-ssh-token");
    await poll({ ...initial, status: "observation-unavailable", last_known_status: "configuration-required",
      observation: { available: false }, can_install: false, fields: [] });
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(document.querySelector('[name="APP_TOKEN"]') === null, true);
    assert.match(document.body.textContent, /private setup draft is kept on this page/);
    await poll(initial);
    assert.equal(document.querySelector('[name="APP_USER"]').value, "synthetic-ssh-user");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "fixture-only-ssh-token");
    assert.equal(calls.some((call) => call.method === "POST"), false);
    assert.equal(window.localStorage.length, 0); assert.equal(window.sessionStorage.length, 0);
  });
});

test("an uncertain admitted SSH operation cannot restore an earlier private draft", async () => {
  await mountedRemote({ ...ready, remote_admission_requested: false }, async ({ document, calls, button, enter, poll }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_TOKEN", "fixture-only-discarded-token");
    await poll({ ...ready, status: "observation-unavailable", last_known_status: "configuration-required",
      remote_admission_requested: true, observation: { available: false }, can_install: false, fields: [] });
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.doesNotMatch(document.body.textContent, /private setup draft is kept on this page/);
    await poll(ready);
    await act(async () => button("Continue to private setup").click());
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
    assert.equal(calls.some((call) => call.method === "POST"), false);
  });
});

test("a changed destination during transport loss cannot recover the previous private draft", async () => {
  await mountedRemote(ready, async ({ document, calls, button, enter, poll }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_TOKEN", "fixture-only-previous-destination-token");
    await poll(ready, true);
    await poll({ ...ready, destination: "/synthetic-owned/src/another-workshop" });
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    await act(async () => button("Continue to private setup").click());
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
    assert.equal(calls.some((call) => call.method === "POST"), false);
  });
});

test("canceling a paused private setup clears its volatile values", async () => {
  await mountedRemote(ready, async ({ document, calls, button, enter, poll }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_TOKEN", "fixture-only-canceled-token"); await poll(ready, true);
    await act(async () => button("Cancel private setup").click());
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    await poll(ready); await act(async () => button("Continue to private setup").click());
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
    assert.equal(calls.some((call) => call.method === "POST"), false);
  });
});

test("initial submission clears private values before its reply and lost observation cannot restore them", async () => {
  await mountedRemote(ready, async ({ document, window, calls, button, enter, setInstall, setRead, poll }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "synthetic-sent-user"); await enter("APP_TOKEN", "fixture-only-sent-token");
    let rejectInstall;
    setInstall(() => new Promise((_resolve, reject) => { rejectInstall = reject; }));
    await act(async () => button("Install and start on Linux").click());
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.match(document.body.textContent, /Sending your installation request/);
    setRead(() => { throw new Error("Synthetic observation unavailable."); });
    await act(async () => rejectInstall(new Error("Synthetic reply unavailable.")));
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    setRead(null); await poll(ready);
    await act(async () => button("Continue to private setup").click());
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(window.localStorage.length, 0); assert.equal(window.sessionStorage.length, 0);
  });
});

test("older same-guide source remains inspectable with exact-owned stop and no restart or private access", async () => {
  const old = { ...ready, source_binding_current: false, status: "running", can_stop: true };
  await mountedRemote(old, async ({ document, calls, button, changeSelection }) => {
    const candidate = { ...checked, build_id: "newer-pending-build", status: "running", source_digest: "d".repeat(64) };
    await changeSelection(guide, candidate, null);
    assert.match(document.body.textContent, /earlier proposal or build/);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(button("Restart accepted application") === undefined, true);
    assert.ok(button("Stop owned application"));
    await act(async () => button("Stop owned application").click());
    const posts = calls.filter((call) => call.method === "POST");
    assert.equal(posts.length, 1);
    assert.deepEqual(posts[0].body, { deployment_id: old.deployment_id, action: "stop" });
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "stopped");
  });
});

test("a delayed connect response cannot publish private setup for an edited acceptance or another guide", async () => {
  await mountedRemote(null, async ({ document, button, enter, setConnect, changeSelection }) => {
    let resolveConnect;
    setConnect(() => new Promise((resolve) => { resolveConnect = resolve; }));
    await chooseDestination(enter);
    await act(async () => button("Connect and check destination").click());
    const nextGuide = { ...guide, version: 5 };
    const nextBuild = { ...checked, guide_version: 5, source_digest: "d".repeat(64), build_id: "newer-build" };
    const nextOptions = { ...options, guide_version: 5, build_id: nextBuild.build_id };
    await changeSelection(nextGuide, nextBuild, nextOptions);
    await act(async () => resolveConnect(structuredClone(ready)));
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(document.querySelector(".remote-owned-installation") === null, true);
    assert.equal(document.querySelector('input[type="password"]') === null, true);
  });
});

for (const kind of ["install", "retry"]) test(`remote recovery read after a finished ${kind} POST cannot block a newer checked status or release a newer action`, async () => {
  const initial = kind === "retry" ? retryableFailure : ready;
  await mountedRemote(initial, async ({ document, calls, button, enter, setRead, setInstall, setRetry, setAction, setReceipt, poll }) => {
    await act(async () => button(kind === "retry" ? "Prepare another install attempt" : "Continue to private setup").click());
    await enter("APP_USER", "recovery-user"); await enter("APP_TOKEN", "fixture-only-recovery-token");
    const running = { ...ready, status: "running", can_install: false, can_retry_install: false,
      can_stop: true, can_restart: true, source_current: true, ready: true,
      operation_id: kind === "retry" ? retryOperation : failedOperation };
    let resolveRecovery, reads = 0;
    setRead(() => ++reads === 1 ? new Promise((resolve) => { resolveRecovery = resolve; }) : null);
    const lostReply = () => { setReceipt(running); throw new Error("Synthetic POST reply lost after remote admission."); };
    if (kind === "retry") setRetry(lostReply); else setInstall(lostReply);
    await act(async () => button(kind === "retry" ? "Retry installation on this Linux computer" : "Install and start on Linux").click());
    assert.equal(typeof resolveRecovery, "function", "The catch-issued recovery GET, rather than a pre-command read, is held.");
    assert.equal(button("Recheck remote status").disabled, false,
      "A finished POST cannot hold read-only recovery behind its pending status GET.");
    assert.match(document.body.textContent, /Remote observation is unavailable/);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);

    await poll(running);
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "running");
    assert.equal(document.querySelector(".remote-first-use h4").textContent, "Use your installed application");
    assert.equal(button("Recheck remote status").disabled, false);
    assert.equal(button("Restart accepted application").disabled, false);
    assert.doesNotMatch(document.body.textContent, /Remote observation is unavailable/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1,
      "A newer scoped status poll never repeats the lost installation request.");

    let resolveAction;
    setAction(() => new Promise((resolve) => { resolveAction = resolve; }));
    await act(async () => button("Restart accepted application").click());
    assert.equal(typeof resolveAction, "function");
    assert.equal(button("Restarting accepted application...").disabled, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 2);
    await act(async () => resolveRecovery({ ...initial, detail: "STALE CATCH RECOVERY RECEIPT" }));
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "running");
    assert.doesNotMatch(document.body.textContent, /STALE CATCH RECOVERY RECEIPT/);
    assert.equal(button("Restarting accepted application...").disabled, true,
      "The obsolete recovery GET cannot retire the newer action's transport waiting.");
    assert.equal(button("Recheck remote status").disabled, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 2);
    await act(async () => resolveAction({ ...running, detail: "CURRENT EXPLICIT RESTART RECEIPT" }));
    assert.match(document.body.textContent, /CURRENT EXPLICIT RESTART RECEIPT/);
    assert.equal(button("Recheck remote status").disabled, false);
    assert.equal(button("Restart accepted application").disabled, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 2);
  });
});

test("remote recovery read after a finished Connect POST keeps the destination draft and prevents duplicate connection admission", async () => {
  await mountedRemote(null, async ({ document, calls, button, enter, setRead, setConnect, setReceipt, poll }) => {
    await chooseDestination(enter); await enter("ssh_port", "2207");
    const checkedDestination = { ...ready, endpoint: { ...endpoint, ssh_port: 2207 } };
    let resolveRecovery, reads = 0;
    setRead(() => ++reads === 1 ? new Promise((resolve) => { resolveRecovery = resolve; }) : null);
    setConnect(() => { setReceipt(checkedDestination); throw new Error("Synthetic destination check reply was lost."); });
    await act(async () => button("Connect and check destination").click());
    assert.equal(typeof resolveRecovery, "function");
    assert.equal(button("Recheck saved remote status").disabled, false,
      "Finished Connect transport cannot block its read-only status recovery.");
    assert.equal(button("Connect and check destination").disabled, true,
      "Unknown destination status cannot admit a second connection POST.");
    for (const [name, value] of Object.entries({ host: endpoint.host, user: endpoint.user, destination, ssh_port: "2207" }))
      assert.equal(document.querySelector(`[name="${name}"]`).value, value);
    await act(async () => button("Connect and check destination").click());
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);

    await poll(checkedDestination);
    assert.equal(button("Connect and check destination").disabled, false);
    assert.equal(button("Continue to private setup").disabled, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    let resolveConnect;
    setConnect(() => new Promise((resolve) => { resolveConnect = resolve; }));
    await act(async () => button("Connect and check destination").click());
    assert.equal(typeof resolveConnect, "function");
    await act(async () => resolveRecovery({ ...checkedDestination, status: "not-installed", admitted: false,
      detail: "STALE DESTINATION RECOVERY" }));
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "configuration-required");
    assert.doesNotMatch(document.body.textContent, /STALE DESTINATION RECOVERY/);
    assert.equal(button("Checking SSH access...").disabled, true,
      "The obsolete read cannot release a newer explicit destination check.");
    assert.equal(calls.filter((call) => call.method === "POST").length, 2);
    await act(async () => resolveConnect({ ...checkedDestination, detail: "CURRENT DESTINATION CHECK" }));
    assert.match(document.body.textContent, /CURRENT DESTINATION CHECK/);
    assert.equal(button("Connect and check destination").disabled, false);
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(calls.filter((call) => call.method === "POST").length, 2);
  });
});

test("an older status GET cannot restore private setup after lost-response admission is observed", async () => {
  await mountedRemote(ready, async ({ document, button, enter, setRead, setInstall, setReceipt }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "synthetic-staff"); await enter("APP_TOKEN", "fixture-only-token");
    let resolveOldRead, reads = 0;
    setRead(() => ++reads === 1 ? new Promise((resolve) => { resolveOldRead = resolve; }) : null);
    await act(async () => button("Recheck remote status").click());
    assert.equal(typeof resolveOldRead, "function");
    setInstall(() => {
      setReceipt({ ...ready, status: "installing", can_install: false, can_stop: true });
      throw new Error("Synthetic installation response was lost.");
    });
    await act(async () => button("Install and start on Linux").click());
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "installing");
    await act(async () => resolveOldRead(structuredClone(ready)));
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "installing");
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(button("Continue to private setup") === undefined, true);
  });
});

test("recovery-needed stays uncertain and only the backend's exact-owned stop capability is offered", async () => {
  const uncertain = { ...ready, status: "recovery-needed", observation: { available: false },
    last_known_status: "starting", can_install: false, can_restart: false, can_stop: true };
  await mountedRemote(uncertain, async ({ document, calls, button }) => {
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "recovery-needed");
    assert.match(document.body.textContent, /cleanup needs to be verified/);
    assert.match(document.body.textContent, /missing SSH connection does not establish a stopped application/);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(button("Restart accepted application") === undefined, true);
    assert.equal(button("Connect and check destination").disabled, true);
    assert.ok(button("Stop owned application"));
    assert.equal(calls.some((call) => call.method === "POST"), false);
  });
});

test("mounted control-state observations close private setup, lock the endpoint and keep progress polling active", async () => {
  await mountedRemote(ready, async ({ document, calls, button, enter, poll }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "synthetic-private-draft");
    for (const status of ["stopping", "restarting"]) {
      await poll({ ...ready, status, can_install: false, can_stop: false, can_restart: false,
        progress: [{ message: `Synthetic ${status} operation remains active.` }] });
      assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, status);
      assert.equal(document.querySelector(".remote-private-form") === null, true);
      assert.equal(document.querySelector('[name="host"]').disabled, true);
      assert.equal(document.querySelector('[name="user"]').disabled, true);
      assert.equal(document.querySelector('[name="destination"]').disabled, true);
      assert.equal(button("Connect and check destination").disabled, true);
      assert.equal(button("Restart accepted application") === undefined, true);
      assert.equal(button("Continue to private setup") === undefined, true);
      const activity = [...document.querySelectorAll("details")].find((node) => node.querySelector("summary").textContent === "Remote deployment activity");
      assert.equal(activity.open, true);
    }
    await poll(ready);
    await act(async () => button("Continue to private setup").click());
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(calls.some((call) => call.method === "POST"), false);
  });
});

test("mounted installation steps follow observed phases without claiming retained steps are current", async () => {
  await mountedRemote(ready, async ({ document, calls, poll }) => {
    const step = () => document.querySelector(".remote-deployment-step");
    assert.equal(step() === null, true);
    for (const [phase, label] of [
      ["python-environment", "Preparing Python"],
      ["dependencies", "Installing runtime dependencies"],
      ["application-dependencies", "Installing application dependencies"],
      ["native-tools", "Installing MobKit"],
      ["application-source", "Checking application code"],
      ["service-install", "Starting and checking your application"],
    ]) {
      await poll({ ...ready, status: "installing", can_install: false, phase });
      assert.equal(step().dataset.deploymentPhase, phase);
      assert.equal(step().getAttribute("role"), "status");
      assert.equal(step().textContent.trim(), `Current step: ${label}`);
      assert.equal(document.querySelector(".remote-private-form") === null, true);
    }
    await poll({ ...ready, status: "installing", phase: "service-install" }, true);
    assert.equal(step().textContent.trim(), "Last observed step: Starting and checking your application");
    for (const next of [
      { status: "observation-unavailable", observation: { available: false }, last_known_status: "installing" },
      { status: "stopped" },
      { status: "failed" },
      { status: "recovery-needed" },
    ]) {
      await poll({ ...ready, can_install: false, phase: "service-install", ...next });
      assert.equal(step().textContent.trim(), "Last observed step: Starting and checking your application");
    }
    await poll({ ...ready, status: "running", can_install: false, source_current: true, ready: true, phase: "complete" });
    assert.equal(step().textContent.trim(), "Last observed step: Application operation finished");
    await poll({ ...ready, status: "installing", can_install: false, phase: "private-unknown-phase" });
    assert.equal(step() === null, true);
    assert.doesNotMatch(document.body.textContent, /private-unknown-phase|Current step:/);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  });
});

test("known remote steps use current captions only for observed active operations", () => {
  const receipt = { ...ready, status: "installing", phase: "knowledge-start" };
  const before = structuredClone(receipt);
  assert.deepEqual(remoteDeploymentStep(receipt), {
    phase: "knowledge-start", label: "Starting the knowledge base", caption: "Current step",
  });
  assert.equal(remoteDeploymentStep(receipt, true).caption, "Last observed step");
  assert.equal(remoteDeploymentStep({ ...receipt, observation: undefined }).caption, "Last observed step");
  assert.equal(remoteDeploymentStep({ ...receipt, status: "stopped" }).caption, "Last observed step");
  assert.deepEqual(remoteDeploymentStep({ ...receipt, status: "stopping", phase: "admitted" }), {
    phase: "admitted", label: "Preparing your request", caption: "Current step",
  });
  assert.equal(remoteDeploymentStep({ ...receipt, phase: "unknown" }), null);
  assert.equal(remoteDeploymentStep({ ...receipt, phase: "toString" }), null);
  assert.equal(remoteDeploymentStep({ ...receipt, phase: null }), null);
  assert.equal(remoteDeploymentStep(null), null);
  assert.deepEqual(receipt, before);
});

test("an altered-source running service never presents verified readiness and preserves exact-owned stop", async () => {
  const altered = { ...ready, status: "running", ready: false, source_current: false,
    can_install: false, can_restart: false, can_stop: true };
  await mountedRemote(altered, async ({ document, calls, button }) => {
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "running");
    assert.match(document.querySelector(".remote-deployment h3").textContent, /verified readiness check/);
    assert.match(document.body.textContent, /Native readiness is unverified/);
    assert.match(document.body.textContent, /installed source differs from the accepted package/);
    assert.equal(document.querySelector(".remote-first-use h4").textContent, "Check your installed runtime");
    assert.doesNotMatch(document.body.textContent, /Use your installed application/);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(button("Restart accepted application") === undefined, true);
    await act(async () => button("Stop owned application").click());
    assert.deepEqual(calls.filter((call) => call.method === "POST").map((call) => call.body),
      [{ deployment_id: altered.deployment_id, action: "stop" }]);
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "stopped");
  });
});

test("not-installed retries require authoritative nonadmission and fresh explicitly supplied private setup", async () => {
  await mountedRemote(ready, async ({ document, calls, button, enter, setInstall, setReceipt, poll }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "previous-synthetic-user"); await enter("APP_TOKEN", "previous-fixture-token");
    const notInstalled = { ...ready, status: "not-installed", admitted: false, can_install: true };
    setInstall(() => {
      setReceipt(notInstalled);
      throw new Error("Synthetic response was lost before installation admission.");
    });
    await act(async () => button("Install and start on Linux").click());
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "not-installed");
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.ok(button("Retry private setup"));
    await act(async () => button("Retry private setup").click());
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
    assert.equal(button("Install and start on Linux").disabled, true);
    await poll(notInstalled);
    assert.ok(document.querySelector(".remote-private-form"), "Repeated authoritative nonadmission does not close a new explicit retry form.");
    await enter("APP_USER", "fresh-synthetic-user"); await enter("APP_TOKEN", "fresh-fixture-token");
    setInstall((body) => {
      assert.deepEqual(body.settings, { APP_USER: "fresh-synthetic-user", APP_TOKEN: "fresh-fixture-token" });
      return { ...ready, status: "installing", admitted: true, can_install: false };
    });
    await act(async () => button("Install and start on Linux").click());
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "installing");
    assert.equal(calls.filter((call) => call.method === "POST").length, 2);
    await poll({ ...notInstalled, admitted: undefined });
    assert.equal(button("Retry private setup") === undefined, true);
    await poll({ ...notInstalled, can_install: false });
    assert.equal(button("Retry private setup") === undefined, true);
    await poll({ ...notInstalled, admitted: true });
    assert.equal(button("Retry private setup") === undefined, true);
  });
});

test("mounted check disclosures consume the controller's passed/failed wire statuses and retain pass compatibility", async () => {
  await mountedRemote(ready, async ({ document, poll }) => {
    const disclosure = (title) => [...document.querySelectorAll("details")]
      .find((node) => node.querySelector("summary").textContent === title);
    assert.equal(disclosure("Destination checks").open, false);
    assert.match(disclosure("Destination checks").textContent, /Python: passed/);
    await poll({ ...ready, checks: [{ name: "Native source", status: "passed", detail: "Synthetic accepted source check" }] });
    assert.equal(disclosure("Deployment checks").open, false);
    await poll({ ...ready, preflight: { ...ready.preflight,
      checks: [{ name: "Python", status: "pass", detail: "Compatible success spelling" }] } });
    assert.equal(disclosure("Destination checks").open, false);
    await poll({ ...ready, status: "failed", can_install: false,
      preflight: { ...ready.preflight, ready: false, checks: [{ name: "systemd-user", status: "failed", detail: "Synthetic manager check failure" }] },
      checks: [{ name: "Native source", status: "failed", detail: "Synthetic source check failure" }] });
    assert.equal(disclosure("Destination checks").open, true);
    assert.equal(disclosure("Deployment checks").open, true);
    assert.match(disclosure("Destination checks").textContent, /systemd-user: failed/);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
  });
});

test("mounted preflight shows destination checks once and repeated observations do not imply installation or repeat unchanged activity", async () => {
  const message = "The destination checks passed. Enter private setup to install.";
  const preflight = { ...ready, admitted: false, checks: structuredClone(ready.preflight.checks),
    progress: Array.from({ length: 6 }, (_, index) => ({ at: `synthetic-time-${index}`, message })) };
  await mountedRemote(preflight, async ({ document, calls, poll }) => {
    const summaries = () => [...document.querySelectorAll("summary")].map((node) => node.textContent);
    const activity = () => [...document.querySelectorAll("details")]
      .find((node) => node.querySelector("summary")?.textContent === "Remote deployment activity");
    assert.equal(summaries().filter((title) => title === "Destination checks").length, 1);
    assert.equal(summaries().includes("Installed checks"), false);
    assert.equal(summaries().includes("Deployment checks"), false);
    assert.equal(activity().querySelectorAll("li").length, 1);
    assert.equal(document.querySelector(".remote-installed-use") === null, true);
    assert.ok(calls.every((call) => call.method === "GET" && call.target === remoteDeploymentUrl(guide.id)));
    const failure = { message: "The destination needs attention. Resolve the failed checks, then check it again.", at: "synthetic-time-7" };
    await poll({ ...preflight, status: "failed", can_install: false,
      checks: [{ name: "Accepted source", status: "failed", detail: "Synthetic changed source" }],
      progress: [...preflight.progress, failure] });
    assert.ok(summaries().includes("Deployment checks"));
    assert.equal(activity().querySelectorAll("li").length, 2);
    assert.match(activity().textContent, /destination needs attention/);
    await poll({ ...preflight, progress: [...preflight.progress, failure, { at: "synthetic-time-8", message }] });
    assert.equal(activity().querySelectorAll("li").length, 3);
    assert.equal(document.querySelector(".remote-installed-use") === null, true);
  });
});

test("a confirmed failed initial destination check returns to private setup through the same receipt's explicit read", async () => {
  await mountedRemote(failedDestinationCheck, async ({ document, button, calls, setReceipt }) => {
    const location = document.querySelector(".deployment-location").textContent;
    assert.equal(document.querySelector(".remote-deployment h3").textContent, "Destination checks need attention");
    const recovery = document.querySelector(".remote-recovery");
    assert.equal(recovery.querySelector("h4").textContent, "Destination checks need attention");
    assert.match(recovery.textContent, /Destination checks on this same Linux computer, then choose Recheck remote status/);
    assert.doesNotMatch(recovery.textContent, /installation|previous attempt|worker|cleanup/i);
    const checks = [...document.querySelectorAll("details")].find((node) => node.querySelector("summary").textContent === "Destination checks");
    assert.equal(checks.open, true);
    assert.match(checks.textContent, /systemd-user: failed/);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(button("Continue to private setup") === undefined, true);
    assert.equal(button("Stop owned application") === undefined, true);
    assert.equal(button("Prepare another install attempt") === undefined, true);
    const beforeRecheck = calls.length;
    setReceipt({ ...ready, remote_admission_requested: false });
    await act(async () => button("Recheck remote status").click());
    assert.equal(calls.length, beforeRecheck + 1);
    assert.equal(calls.at(-1).method, "GET");
    assert.equal(calls.at(-1).target, remoteDeploymentUrl(guide.id));
    assert.equal(document.querySelector(".deployment-location").textContent, location);
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "configuration-required");
    assert.equal(document.querySelector(".remote-deployment h3").textContent, "Your destination is ready for private setup");
    assert.equal(document.querySelector(".remote-recovery") === null, true);
    assert.equal(button("Continue to private setup").disabled, false);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.ok(calls.every((call) => call.method === "GET" && call.target === remoteDeploymentUrl(guide.id)));
  });
});

test("uncertain initial checks and contradictory installation custody preserve conservative recovery", async () => {
  await mountedRemote(failedDestinationCheck, async ({ document, button, calls, poll }) => {
    for (const changed of [{ remote_admission_requested: undefined }, { remote_admission_requested: null },
      { remote_admission_requested: true }, { admitted: true }, { worker_live: true }, { worker_pid: 4321 },
      { service_owned: true }, { service_observed: true }, { service: { status: "running" } },
      { operation_id: failedOperation }, { install_operation_id: failedOperation },
      { pending_operation: { id: failedOperation, action: "install" } },
      { retry_of_operation_id: failedOperation }, { previous_operation_id: failedOperation },
      { preflight: { ...failedDestinationCheck.preflight, ready: true } },
      { preflight: { ...failedDestinationCheck.preflight, checks: ready.preflight.checks } },
      { observation: { available: false } }]) {
      await poll({ ...failedDestinationCheck, ...changed });
      assert.equal(document.querySelector(".remote-deployment h3").textContent, "Your installation needs a recovery step");
      assert.equal(document.querySelector(".remote-recovery h4").textContent, "Recover this installation");
      assert.doesNotMatch(document.querySelector(".remote-recovery").textContent, /checks on this same Linux computer/);
      assert.equal(document.querySelector(".remote-private-form") === null, true);
      assert.equal(button("Continue to private setup") === undefined, true);
      assert.ok(button("Recheck remote status"));
    }
    await poll(failedDestinationCheck, true);
    assert.match(document.querySelector(".remote-recovery").textContent, /cannot verify its current state/);
    assert.equal(document.querySelector(".remote-recovery h4").textContent, "Recover this installation");
    await poll({ ...retryableFailure, remote_admission_requested: true });
    assert.equal(document.querySelector(".remote-recovery h4").textContent, "Recover this installation");
    assert.match(document.querySelector(".remote-recovery").textContent, /worker was cleaned up/);
    assert.equal(button("Prepare another install attempt").disabled, false);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  });
});

test("initial cloud destination checks keep the same VM and cloud removal takes presentation precedence", async () => {
  const cloudGuide = { ...guide, proposal: { target: "gcp" } };
  const cloudOptions = { ...options, target: "gcp" };
  const cloudEndpoint = { transport: "gcp-iap", operation_id: failedOperation, account: "fixture@example.test",
    project: "fixture-project", zone: "europe-west1-b", instance: "fixture-app", instance_id: "1234",
    ownership_token: "e".repeat(32), host: "compute.1234", user: "mobkit", ssh_port: 22 };
  const cloudReceipt = { ...failedDestinationCheck, target: "gcp", cloud_operation_id: failedOperation, endpoint: cloudEndpoint };
  await mountedRemote(cloudReceipt, async ({ document, button, calls, setReceipt, poll }) => {
    const location = document.querySelector(".deployment-location").textContent;
    assert.equal(document.querySelector(".remote-deployment h3").textContent, "Destination checks need attention");
    assert.match(document.querySelector(".remote-recovery").textContent, /Destination checks on this same cloud VM, then choose Recheck remote status/);
    assert.equal(button("Connect and check destination") === undefined, true);
    setReceipt({ ...cloudReceipt, status: "configuration-required", can_install: true, preflight: ready.preflight });
    await act(async () => button("Recheck remote status").click());
    assert.equal(document.querySelector(".deployment-location").textContent, location);
    assert.equal(document.querySelector(".remote-recovery") === null, true);
    assert.equal(button("Continue to private setup").disabled, false);
    await poll({ ...cloudReceipt, cloud_status: "removing" });
    assert.equal(document.querySelector(".remote-deployment h3").textContent, "Cloud removal is requested");
    assert.equal(document.querySelector(".remote-recovery") === null, true);
    assert.equal(button("Continue to private setup") === undefined, true);
    assert.ok(calls.every((call) => call.method === "GET" && call.target === remoteDeploymentUrl(guide.id)));
  }, { guide: cloudGuide, options: cloudOptions });
});

test("private admission and verified-running views explain user-service persistence without promising logout or reboot survival", async () => {
  await mountedRemote(ready, async ({ document, button, poll }) => {
    await act(async () => button("Continue to private setup").click());
    const setup = document.querySelector(".remote-private-form").textContent;
    assert.match(setup, /systemd service for your Linux user/);
    assert.match(setup, /may stop on logout unless this computer keeps user services running/);
    assert.match(setup, /does not enable persistence after logout or verify restart after reboot/);
    await poll({ ...ready, status: "running", source_current: true, ready: true, can_install: false });
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(document.querySelector(".remote-first-use h4").textContent, "Use your installed application");
    const installed = document.querySelector(".remote-first-use").textContent;
    assert.match(installed, /Running now does not prove it will survive logout or reboot/);
    assert.match(installed, /keeping it running after logout is a separate host setting/);
  });
});

test("mounted remote first-job success pauses after a lost Stop response until matching status is explicitly rechecked", async () => {
  const selectedGuide = { ...guide, id: "44444444-4444-4444-8444-444444444444" };
  const selectedBuild = { ...checked, guide_id: selectedGuide.id, source_verified: true };
  const running = { ...ready, guide_id: selectedGuide.id, deployment_id: "55555555-5555-4555-8555-555555555555",
    status: "running", can_install: false, can_stop: true, can_restart: true, admitted: true,
    remote_admission_requested: true, worker_live: false, source_current: true, ready: true,
    controller_sha256: "d".repeat(64), host_key_fingerprint: "SHA256:" + "A".repeat(43),
    host: { uid: 1001, home: "/home/volunteer", destination, platform: "linux", python: "3.11.9" } };
  const task = "Compare the workshop's two draft schedules and explain the tradeoffs.";
  const firstUseMetadata = { protocol: 1, guide_id: selectedGuide.id, version: selectedGuide.version,
    guide_version: running.guide_version, deployment_id: running.deployment_id, build_id: running.build_id,
    accepted_source_digest: running.accepted_source_digest, source_digest: running.accepted_source_digest,
    package_sha256: running.package_sha256, transferred_source_digest: running.transferred_source_digest,
    controller_sha256: running.controller_sha256, remote_binding: JSON.parse(remoteInstalledUseBinding(running, selectedGuide)),
    scope: "frozen-remote-package-source", current: true, status: "running",
    project: { name: "Workshop" }, entrypoint: { identity: "lead:main", name: "Workshop lead" },
    first_use: { scope: "authored-first-use-suggestions", metadata_sha256: "e".repeat(64),
      tasks: [{ label: "Compare two schedules", request: task, description: "Use the schedules you want to compare." }], detail: null },
    observation: { scope: "saved-destination-observation", status: "running", available: true,
      freshness: "fresh", admitted: true, source_current: true },
    installed_source_verified: true, native_runtime_available: true, application_outcomes_verified: false,
    success_reporting_available: true, continuation: { build_id: selectedBuild.build_id,
      source_digest: selectedBuild.source_digest, available: true, requires_acknowledgement: false } };
  await mountedRemote(running, async ({ document, calls, button, setRead, setAction }) => {
    const idea = document.querySelector(".remote-installed-use .deployment-use-ideas button");
    assert.ok(idea, "The actual first-use panel must mount with its source-bound suggestions.");
    await act(async () => idea.click());
    const draft = () => document.querySelector(".remote-installed-use .deployment-use-field textarea");
    assert.equal(draft().value, task);
    assert.equal(button("It worked").disabled, false);
    assert.equal(calls.some((call) => call.method === "POST"), false);

    setRead(() => { throw new Error("Synthetic status read is unavailable after the lost Stop response."); });
    setAction((body) => {
      assert.equal(body.action, "stop");
      throw new Error("Synthetic Stop response was lost.");
    });
    await act(async () => button("Stop owned application").click());
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "running",
      "The unconfirmed read preserves the previous receipt without treating it as a new observation.");
    assert.equal(calls.filter((call) => call.target === remoteInstalledUseUrl(selectedGuide.id, running.deployment_id)).length, 1,
      "Retained first-use metadata must not bypass the parent's uncertain command status.");
    assert.match(document.body.textContent, /Success reporting is paused until installation command status can be checked/);
    assert.equal(button("It worked").disabled, true);
    assert.equal(draft().value, task);
    assert.equal(draft().disabled, false, "The editable first-job draft remains available during uncertain status.");
    await act(async () => button("It worked").click());
    assert.deepEqual(calls.filter((call) => call.method === "POST").map(({ target, body }) => ({ target, body })),
      [{ target: remoteDeploymentUrl(selectedGuide.id, "action"), body: { deployment_id: running.deployment_id, action: "stop" } }]);

    setRead(() => structuredClone(running));
    await act(async () => button("Recheck remote status").click());
    assert.equal(button("It worked").disabled, false);
    assert.doesNotMatch(document.body.textContent, /Success reporting is paused/);
    assert.equal(draft().value, task);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1,
      "Matching read-only recovery restores the action without replaying Stop or submitting a success report.");
  }, { guide: selectedGuide, build: selectedBuild, options, firstUseMetadata });
});

test("verified terminal failure offers an explicit same-folder retry with fresh private settings and immediate form clearing", async () => {
  await mountedRemote(ready, async ({ document, window, calls, button, enter, poll, setRetry }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "previous-user"); await enter("APP_TOKEN", "previous-fixture-token");
    await poll(retryableFailure);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.match(document.querySelector(".remote-recovery").textContent, /Dependency installation did not finish/);
    assert.match(document.querySelector(".remote-recovery").textContent, /worker was cleaned up/);
    assert.equal(calls.some((call) => call.method === "POST"), false);
    await act(async () => button("Prepare another install attempt").click());
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
    assert.equal(button("Retry installation on this Linux computer").disabled, true);
    assert.equal(document.querySelector(".deployment-optional-settings").open, false);
    assert.equal(document.querySelector('.remote-private-form input[type="checkbox"]') === null, true);
    assert.match(document.querySelector(".remote-private-form").textContent, /keeps the accepted source, application data and saved installation evidence/);
    assert.match(document.querySelector(".remote-private-form").textContent, /replaces the validated private configuration and reruns guarded dependency and startup steps/);
    assert.doesNotMatch(document.querySelector(".remote-private-form").textContent, /transfers the accepted source/);
    await enter("APP_USER", "fresh-user"); await enter("APP_TOKEN", "fresh-fixture-token");
    let resolveRetry;
    setRetry((body) => {
      assert.deepEqual(body, { version: guide.version, build_id: checked.build_id,
        deployment_id: ready.deployment_id, previous_operation_id: failedOperation,
        settings: { APP_USER: "fresh-user", APP_TOKEN: "fresh-fixture-token" } });
      return new Promise((resolve) => { resolveRetry = resolve; });
    });
    await act(async () => button("Retry installation on this Linux computer").click());
    assert.equal(document.querySelector(".remote-private-form") === null, true, "Clear private form before the retry response arrives.");
    assert.equal(document.body.innerHTML.includes("fresh-fixture-token"), false);
    assert.match(document.querySelector(".remote-recovery").textContent, /Saved remote status will confirm/);
    assert.equal(button("Prepare another install attempt").disabled, true);
    assert.equal(window.localStorage.length, 0); assert.equal(window.sessionStorage.length, 0);
    await act(async () => resolveRetry({ ...retryableFailure, status: "installing", can_retry_install: false,
      operation_id: retryOperation, previous_operation_id: failedOperation, worker_live: true }));
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "installing");
    assert.equal(document.querySelector(".remote-recovery") === null, true);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(calls.at(-1).target, remoteDeploymentUrl(guide.id, "retry"));
  });
});

test("lost retry response is observed without repeating submission or accepting an older failed-status GET", async () => {
  await mountedRemote(retryableFailure, async ({ document, button, enter, calls, setRetry, setRead, setReceipt }) => {
    await act(async () => button("Prepare another install attempt").click());
    await enter("APP_USER", "fresh-user"); await enter("APP_TOKEN", "fresh-fixture-token");
    let resolveOldRead, reads = 0;
    setRead(() => ++reads === 1 ? new Promise((resolve) => { resolveOldRead = resolve; }) : null);
    await act(async () => button("Recheck remote status").click());
    setRetry(() => {
      setReceipt({ ...retryableFailure, status: "installing", can_retry_install: false,
        operation_id: retryOperation, previous_operation_id: failedOperation, worker_live: true });
      throw new Error("Synthetic reply was lost after deliberate retry admission.");
    });
    await act(async () => button("Retry installation on this Linux computer").click());
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "installing");
    await act(async () => resolveOldRead(structuredClone(retryableFailure)));
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "installing");
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(button("Prepare another install attempt") === undefined, true);
    assert.doesNotMatch(document.body.textContent, /could not confirm the retry response/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("lost retry observation preserves the known failed attempt while withholding another retry until status is available", async () => {
  await mountedRemote(retryableFailure, async ({ document, button, enter, calls, setRetry, setRead, poll }) => {
    await act(async () => button("Prepare another install attempt").click());
    await enter("APP_USER", "fresh-user"); await enter("APP_TOKEN", "fresh-fixture-token");
    setRead(() => { throw new Error("Synthetic SSH status is unavailable."); });
    setRetry(() => { throw new Error("Synthetic retry reply is unavailable."); });
    await act(async () => button("Retry installation on this Linux computer").click());
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "failed");
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(button("Prepare another install attempt") === undefined, true);
    assert.match(document.querySelector(".remote-recovery").textContent, /cannot verify its current state/);
    assert.match(document.body.textContent, /does not prove the application failed or stopped/);
    setRead(null);
    await poll(retryableFailure);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await act(async () => button("Prepare another install attempt").click());
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
  });
});

test("failed fresh retry prerequisites remain a visible recovery step without changing the failed operation or automatically retrying", async () => {
  await mountedRemote(retryableFailure, async ({ document, button, enter, calls, setRetry }) => {
    await act(async () => button("Prepare another install attempt").click());
    await enter("APP_USER", "fresh-user"); await enter("APP_TOKEN", "fresh-fixture-token");
    setRetry(() => ({ ...retryableFailure, retry_preflight_ready: false,
      retry_checks: [{ name: "systemd-user", status: "failed", detail: "The current user service manager is unavailable." }] }));
    await act(async () => button("Retry installation on this Linux computer").click());
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "failed");
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    const checks = [...document.querySelectorAll("details")].find((node) => node.querySelector("summary").textContent === "Retry prerequisites");
    assert.equal(checks.open, true);
    assert.match(checks.textContent, /systemd-user: failed/);
    assert.match(document.querySelector(".remote-recovery").textContent, /latest retry check found an unmet prerequisite/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await act(async () => button("Prepare another install attempt").click());
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
    assert.equal(button("Retry installation on this Linux computer").disabled, true);
  });
});

test("retry private setup belongs to one failed operation and clears when operation or accepted source changes", async () => {
  await mountedRemote(retryableFailure, async ({ document, button, enter, calls, poll }) => {
    await act(async () => button("Prepare another install attempt").click());
    await enter("APP_USER", "prior-operation-user"); await enter("APP_TOKEN", "prior-operation-token");
    const nextFailure = { ...retryableFailure, operation_id: otherOperation, retry_of_operation_id: otherOperation,
      previous_operation_id: failedOperation, failure_phase: "private-canary-unknown-phase" };
    await poll(nextFailure);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.doesNotMatch(document.body.textContent, /private-canary-unknown-phase/);
    await act(async () => button("Prepare another install attempt").click());
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
    await enter("APP_USER", "current-operation-user");
    await poll(nextFailure);
    assert.equal(document.querySelector('[name="APP_USER"]').value, "current-operation-user",
      "Repeated observation of the same eligible failed operation preserves this newly opened form.");
    await poll({ ...nextFailure, source_binding_current: false });
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(button("Prepare another install attempt") === undefined, true);
    assert.match(document.querySelector(".remote-recovery").textContent, /matching accepted build/);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  });
});

test("uncertain cleanup, lost observation and generic failed install permission never offer terminal retry", async () => {
  await mountedRemote(retryableFailure, async ({ document, button, enter, calls, poll }) => {
    await act(async () => button("Prepare another install attempt").click());
    await enter("APP_USER", "discarded-private-draft");
    for (const changed of [{ worker_live: undefined }, { cleanup_confirmed: false },
      { observation: { available: false } }, { source_current: false },
      { can_retry_install: false }, { status: "recovery-needed" }, { status: "observation-unavailable" }]) {
      await poll({ ...retryableFailure, can_install: true, can_stop: true, ...changed });
      assert.equal(document.querySelector(".remote-private-form") === null, true);
      assert.equal(button("Prepare another install attempt") === undefined, true);
      assert.equal(button("Continue to private setup") === undefined, true);
      assert.ok(button("Recheck remote status"));
      assert.ok(button("Stop owned application"));
    }
    assert.match(document.body.textContent, /does not prove the application failed or stopped/);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  });
});

test("a delayed retry reply cannot replace a newer failed operation or follow a changed accepted build", async () => {
  await mountedRemote(retryableFailure, async ({ document, button, enter, poll, setRetry, changeSelection }) => {
    await act(async () => button("Prepare another install attempt").click());
    await enter("APP_USER", "fresh-user"); await enter("APP_TOKEN", "fresh-fixture-token");
    let resolveRetry;
    setRetry(() => new Promise((resolve) => { resolveRetry = resolve; }));
    await act(async () => button("Retry installation on this Linux computer").click());
    const nextFailure = { ...retryableFailure, operation_id: otherOperation, retry_of_operation_id: otherOperation };
    await poll(nextFailure);
    await act(async () => resolveRetry({ ...retryableFailure, status: "installing", can_retry_install: false,
      operation_id: retryOperation, previous_operation_id: failedOperation, worker_live: true }));
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "failed");
    await act(async () => button("Prepare another install attempt").click());
    assert.equal(document.querySelector('[name="APP_USER"]').value, "");
    await enter("APP_USER", "new-user"); await enter("APP_TOKEN", "new-fixture-token");
    await act(async () => button("Retry installation on this Linux computer").click());
    const nextGuide = { ...guide, version: 5 };
    const nextBuild = { ...checked, build_id: "new-accepted-build", guide_version: 5, source_digest: "d".repeat(64) };
    await changeSelection(nextGuide, nextBuild, { ...options, guide_version: 5, build_id: nextBuild.build_id });
    await act(async () => resolveRetry({ ...nextFailure, status: "installing", can_retry_install: false,
      operation_id: retryOperation, previous_operation_id: otherOperation, worker_live: true }));
    assert.equal(document.querySelector(".remote-deployment").dataset.remoteStatus, "failed");
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.equal(button("Prepare another install attempt") === undefined, true);
  });
});


test("unchanged remote controls require a refreshed chat stamp and preserve original installation identity", () => {
  const next = { ...guide, version: 5 };
  const currentBuild = { ...checked, current_guide_version: 5 };
  const currentOptions = { ...portOptions, current_guide_version: 5 };
  const currentReceipt = { ...initialPortSetup, current_guide_version: 5 };
  assert.equal(remoteReceiptCurrent(currentReceipt, currentOptions, currentBuild, next), true);
  assert.equal(remoteCanConfigure(currentReceipt, currentOptions, currentBuild, next), true);
  assert.equal(remoteSetupConsolePort(currentReceipt, currentOptions, currentBuild, next), 18088);
  assert.equal(remoteActionAvailable({ ...currentReceipt, status: "stopped", can_restart: true },
    "restart", currentOptions, currentBuild, next), true);
  assert.equal(currentReceipt.guide_version, 4);
  assert.notEqual(remoteSelectionKey(next, currentBuild), remoteSelectionKey(guide, checked));
  assert.equal(remotePrivateDraftScope(next, currentBuild), remotePrivateDraftScope(guide, checked));
  for (const patch of [{ current_guide_version: undefined }, { current_guide_version: 4 },
    { current_guide_version: null }, { current_guide_version: "5" }, { current_guide_version: 6 },
    { source_binding_current: false }, { accepted_source_digest: "e".repeat(64) }, { build_id: "other-build" }]) {
    const invalid = { ...currentReceipt, ...patch };
    assert.equal(remoteReceiptCurrent(invalid, currentOptions, currentBuild, next), false);
    assert.equal(remoteCanConfigure(invalid, currentOptions, currentBuild, next), false);
    assert.equal(remoteSetupConsolePort(invalid, currentOptions, currentBuild, next), null);
    assert.equal(remoteActionAvailable({ ...invalid, status: "stopped", can_restart: true },
      "restart", currentOptions, currentBuild, next), false);
  }
});

test("a harmless chat retains hidden private values until refreshed evidence and canonical edits erase them", async () => {
  await mountedRemote(initialPortSetup, async ({ document, window, calls, button, enter, changeBlocked, changeSelection, poll, setPortCheck }) => {
    await act(async () => button("Continue to private setup").click());
    await enter("APP_USER", "fixture-chat-user"); await enter("APP_TOKEN", "fixture-only-unsent-chat-token");
    const next = { ...guide, version: 5 }, currentBuild = { ...checked, current_guide_version: 5 };
    const pending = { ...next, operation: { status: "running" } };
    const currentOptions = { ...portOptions, current_guide_version: 5 };
    setPortCheck(async params => ({ ...localPortOptions(Number(params.get("console_port"))), current_guide_version: 5 }));
    await changeBlocked(true);
    await changeSelection(pending, checked, portOptions);
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    assert.match(document.body.textContent, /private setup draft is kept on this page/);
    await poll({ ...initialPortSetup, current_guide_version: 5 });
    assert.equal(document.querySelector(".remote-private-form") === null, true);
    await changeSelection(pending, currentBuild, currentOptions);
    assert.equal(document.querySelector('[name="APP_TOKEN"]').disabled, true);
    assert.equal(button("Install and start on Linux").disabled, true);
    await changeSelection(next, currentBuild, currentOptions);
    await changeBlocked(false);
    assert.equal(document.querySelector('[name="APP_USER"]').value, "fixture-chat-user");
    assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "fixture-only-unsent-chat-token");
    assert.equal(button("Install and start on Linux").disabled, false);
    for (const edited of [{ ...next, brief: { facts: { authority: { summary: "A changed mandate" } } } },
      { ...next, proposal: { ...next.proposal, project: { name: "A changed application" } } }]) {
      await changeSelection(edited, currentBuild, currentOptions);
      assert.equal(document.querySelector(".remote-private-form") === null, true);
      await changeSelection(next, currentBuild, currentOptions);
      await act(async () => button("Continue to private setup").click());
      assert.equal(document.querySelector('[name="APP_TOKEN"]').value, "");
      await enter("APP_TOKEN", "fixture-only-new-unsent-token");
    }
    assert.equal(calls.some(call => call.method === "POST"), false);
    assert.equal(window.localStorage.length, 0); assert.equal(window.sessionStorage.length, 0);
  }, { options: portOptions });
});


// Exercise the real mounted action owner with finite product observation replies.
// No native runtime, Console host, SSH operation or browser connection is started.
function productActionAlertInstallation() {
  const receipt = { ...structuredClone(ready), deployment_id: failedOperation, controller_sha256: "d".repeat(64),
    status: "running", can_install: false, can_stop: true, can_restart: false, remote_admission_requested: true,
    source_current: true, worker_live: true, cleanup_confirmed: false, private_settings_supported: false,
    product_runtime: true, product_scope: "retained-original-product-owner", install_operation_id: failedOperation,
    product_control_manifest_sha256: "e".repeat(64), can_prepare_setup: false, can_finish_setup: false, can_start_background: false };
  receipt.product_control = { owner_live: true, initial_install_complete: true,
    binding: { protocol: 1, install_operation_id: failedOperation, control_manifest_sha256: receipt.product_control_manifest_sha256,
      initial_install_sha256: "f".repeat(64),
      binding: Object.fromEntries(["guide_id", "guide_version", "build_id", "accepted_source_digest", "package_sha256", "transferred_source_digest", "controller_sha256"].map(key => [key, receipt[key]])),
      worker: { pid: 100, uid: 501, start_ticks: "100", boot_id: "synthetic-boot" },
      command: { pid: 101, uid: 501, start_ticks: "101", boot_id: "synthetic-boot" } },
    factory_status: { guide_id: receipt.guide_id, guide_version: receipt.guide_version, deployment_id: receipt.deployment_id,
      build_id: receipt.build_id, package_sha256: receipt.package_sha256, destination: receipt.destination,
      source_digest: receipt.accepted_source_digest, source_verified: true, status: "running", phase: "run",
      target: "local", execution_mode: "interactive", process_live: true, runtime_pid: 4242 } };
  return receipt;
}
function productActionAlertReadback(receipt, packet, outcome = "returned") {
  const value = structuredClone(receipt);
  value.product_control.action_request = { action_request_id: packet.action_request_id, action: packet.action,
    guide_id: value.guide_id, deployment_id: value.deployment_id, phase: "settled", outcome };
  return value;
}

for (const scenario of ["returned", "missing UUID", "wrong UUID", "error", "unknown", "source binding", "control binding", "selection", "owner lifetime"]) {
  test(`remote product action alert recovery - ${scenario}`, async () => {
    const initial = productActionAlertInstallation();
    let lifetime = {};
    const scope = { token: () => lifetime, capture(_state, expected) {
      if (expected !== undefined && expected !== lifetime) return null;
      const selected = lifetime;
      return () => selected === lifetime;
    } };
    await mountedRemote(initial, async ({ document, calls, button, setAction, setRead, poll, changeSelection }) => {
      let packet;
      setRead(() => { throw new Error("Synthetic remote status is unavailable after a lost action reply."); });
      setAction(body => { packet = body; throw new Error("Synthetic product action reply was lost."); });
      await act(async () => button("Stop owned application").click());
      assert.match(packet.action_request_id, /^[a-f0-9-]{36}$/);
      const alert = () => document.querySelector(".remote-deployment > .deployment-error");
      assert.match(alert().textContent, /could not confirm the stop/);
      const recovered = productActionAlertReadback(initial, packet);
      if (scenario === "missing UUID") delete recovered.product_control.action_request.action_request_id;
      if (scenario === "wrong UUID") recovered.product_control.action_request.action_request_id = otherOperation;
      if (scenario === "error") recovered.product_control.action_request.outcome = "error";
      if (scenario === "unknown") { recovered.product_control.action_request.phase = "unknown"; recovered.product_control.action_request.outcome = null; }
      if (scenario === "source binding") {
        recovered.accepted_source_digest = "f".repeat(64);
        recovered.product_control.binding.binding.accepted_source_digest = recovered.accepted_source_digest;
        recovered.product_control.factory_status.source_digest = recovered.accepted_source_digest;
      }
      if (scenario === "control binding") recovered.product_control.binding.command.pid++;
      if (scenario === "selection") await changeSelection({ ...guide, version: 5 }, { ...checked, guide_version: 5 }, { ...options, guide_version: 5 });
      if (scenario === "owner lifetime") { lifetime = {}; await changeSelection(guide, checked, options); }
      setRead(null);
      await poll(recovered);
      if (scenario === "returned") assert.equal(alert() === null, true, "Exact original canonical success retires its obsolete alert.");
      else assert.match(alert().textContent, /could not confirm the stop/, "Uncertain, failed or differently owned replies cannot retire this alert.");
      assert.equal(calls.filter(call => call.method === "POST").length, 1, "Rechecking observes and never repeats the action.");
      assert.doesNotMatch(document.body.textContent, new RegExp(packet.action_request_id), "Internal command UUIDs are not novice-facing text.");
    }, { selectionScope: scope });
  });
}

test("remote product action alert recovery - newer same-text alert", async () => {
  const initial = productActionAlertInstallation();
  await mountedRemote(initial, async ({ document, calls, button, setAction, setRead, poll }) => {
    const packets = [];
    setAction(body => { packets.push(body); throw new Error("Synthetic product action reply was lost."); });
    setRead(() => { throw new Error("Synthetic remote observation is unavailable."); });
    await act(async () => button("Stop owned application").click());
    const alert = () => document.querySelector(".remote-deployment > .deployment-error");
    const text = alert().textContent;
    setRead(null);
    await poll(productActionAlertReadback(initial, packets[0], "error"));
    assert.equal(alert().textContent, text);
    setRead(() => { throw new Error("Synthetic remote observation is unavailable."); });
    await act(async () => button("Stop owned application").click());
    assert.equal(packets.length, 2);
    assert.notEqual(packets[0].action_request_id, packets[1].action_request_id);
    setRead(null);
    await poll(productActionAlertReadback(initial, packets[0]));
    assert.equal(alert().textContent, text, "The old returned journal cannot retire a newer same-text alert.");
    await poll(productActionAlertReadback(initial, packets[1]));
    assert.equal(alert() === null, true, "Only the newer alert's own canonical success can retire it.");
    assert.equal(calls.filter(call => call.method === "POST").length, 2, "The two actions were explicit clicks; recovery never resends either.");
  });
});

test("remote product action alert recovery - ordinary SSH alert remains", async () => {
  const initial = { ...structuredClone(ready), status: "running", can_install: false, can_stop: true };
  await mountedRemote(initial, async ({ document, calls, button, setAction, setRead, poll }) => {
    setRead(() => { throw new Error("Synthetic remote status is unavailable."); });
    setAction(body => { assert.equal(body.action_request_id, undefined); throw new Error("Synthetic ordinary Stop reply was lost."); });
    await act(async () => button("Stop owned application").click());
    setRead(null);
    await poll({ ...initial, status: "stopped", can_stop: false });
    assert.match(document.querySelector(".remote-deployment > .deployment-error").textContent, /could not confirm the stop/,
      "Ordinary status alone is not canonical product command completion.");
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
  });
});
