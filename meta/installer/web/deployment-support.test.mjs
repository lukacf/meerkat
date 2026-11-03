import assert from "node:assert/strict";
import test from "node:test";
import {
  deploymentSupportIntent, deploymentSupportIntentValid, loadDeploymentSupportIntent,
  storeDeploymentSupportIntent, clearDeploymentSupportIntent, deploymentSupportObservationUrl,
  deploymentSupportRequestMatches, deploymentSupportRequestSettled, deploymentSupportRequestValid,
  deploymentSupportState, deploymentSupportAction, deploymentSupportBodyFromIntent,
} from "./deployment-support.mjs";
import { deploymentActionIntent, loadDeploymentActionIntent, storeDeploymentActionIntent,
  createDeploymentPollingGate } from "./deployment-panel.mjs";
import { captureGuideSelection } from "./guide-selection.mjs";

// Pure synthetic status and storage only. No browser, host, API or native work.
const guide = { id: "synthetic-retained-guide", version: 5 };
const REQUEST = "11111111-1111-4111-8111-111111111111";
const REVISION = "22222222-2222-4222-8222-222222222222";
const BEFORE = "a".repeat(64), AFTER = "b".repeat(64), DESCRIPTOR = "c".repeat(64);
const clone = (value) => structuredClone(value);
const fresh = { confirmed: true };
function receipt() {
  const value = { guide_id: guide.id, guide_version: 3, deployment_id: "synthetic-installation",
    build_id: "retained-installed-build", package_sha256: "d".repeat(64), target: "local",
    source_kind: "application", execution_mode: "background", service: { manager: "launchd" },
    status: "stopped", process_live: false, runtime_pid: null, service_cleanup_confirmed: true };
  value.support_request = { scope: "owned-stopped-supervisor-refresh", deployment_id: value.deployment_id,
    build_id: value.build_id, package_sha256: value.package_sha256, requires_stopped: true, stopped_verified: true,
    available: true, prepare_available: true, apply_available: false, service_digest: BEFORE,
    phase: "available", selected: null };
  return value;
}
function selected(phase = "prepared", revision = REVISION) {
  const value = receipt();
  value.support_request = { ...value.support_request, prepare_available: false,
    available: phase !== "completed", apply_available: phase !== "completed", phase,
    service_digest: phase === "completed" ? AFTER : BEFORE,
    selected: { revision, descriptor_sha256: DESCRIPTOR, phase, before_service_digest: BEFORE,
      after_service_digest: AFTER, retained_files: 64, staged_files: 70 } };
  return value;
}
function intent(action = "prepare-support", value = receipt()) {
  return deploymentSupportIntent(guide.id, deploymentSupportAction(value, guide, action, fresh), REQUEST);
}
function observation(command, phase = "pending", outcome = null) {
  return { action_request: { ...clone(command), phase, outcome } };
}
function storage() {
  const values = new Map();
  return { values, getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value), removeItem: (key) => values.delete(key) };
}

test("support command identity is separate from unchanged ordinary intentv1", () => {
  const memory = storage(), ordinary = deploymentActionIntent(guide.id,
    { deployment_id: receipt().deployment_id, action: "restart" }, REQUEST);
  storeDeploymentActionIntent(memory, ordinary);
  const command = intent();
  assert.equal(command.version, 1);
  assert.deepEqual(Object.keys(command), ["version", "guide_id", "deployment_id", "action_request_id", "action",
    "update_id", "expected_service_digest", "revision", "descriptor_sha256"]);
  storeDeploymentSupportIntent(memory, command);
  assert.deepEqual(loadDeploymentSupportIntent(memory, guide.id), command);
  clearDeploymentSupportIntent(memory, command);
  assert.deepEqual(loadDeploymentActionIntent(memory, guide.id), ordinary);
  assert.equal(loadDeploymentSupportIntent(memory, guide.id), null);
  assert.equal(memory.values.size, 1);
});

for (const [name, change] of [
  ["guide", { guide_id: "" }], ["UUID", { action_request_id: "request" }],
  ["version type", { version: "1" }], ["ordinary action", { action: "restart" }],
  ["update identity", { update_id: "staged-update" }], ["digest", { expected_service_digest: AFTER.toUpperCase() }],
  ["inactive branch", { revision: REVISION }], ["runtime truth", { health: "healthy" }],
  ["private settings", { settings: { SECRET: "must-not-cache" } }], ["resume", { resume: true }],
]) test(`invalid support intent ${name} cannot be stored`, () => {
  const value = { ...intent(), ...change }, memory = storage();
  assert.equal(deploymentSupportIntentValid(value, guide.id), false);
  assert.throws(() => storeDeploymentSupportIntent(memory, value));
  assert.equal(memory.values.size, 0);
});

test("action builders reject extra private or runtime fields and malformed branch pins", () => {
  const body = deploymentSupportAction(receipt(), guide, "prepare-support", fresh);
  for (const extra of [{ settings: {} }, { resume: true }, { health: "healthy" },
    { action_request_id: REVISION }, { expected_service_digest: true }, { descriptor_sha256: DESCRIPTOR }])
    assert.throws(() => deploymentSupportIntent(guide.id, { ...body, ...extra }, REQUEST));
  const command = intent("apply-support", selected());
  assert.equal(command.expected_service_digest, null);
  assert.equal(command.revision, REVISION);
  assert.equal(command.descriptor_sha256, DESCRIPTOR);
});

for (const mode of ["read throws", "write throws", "write lost", "occupied"]) {
  test(`unavailable support storage blocks new intent: ${mode}`, () => {
    const memory = storage();
    if (mode === "read throws") memory.getItem = () => { throw new Error("Unavailable"); };
    if (mode === "write throws") memory.setItem = () => { throw new Error("Unavailable"); };
    if (mode === "write lost") memory.setItem = () => {};
    if (mode === "occupied") storeDeploymentSupportIntent(memory, intent());
    assert.throws(() => storeDeploymentSupportIntent(memory, intent()));
    assert.equal(deploymentSupportAction(receipt(), guide, "prepare-support", { ...fresh, storageAvailable: false }), null);
    assert.equal(deploymentSupportState(receipt(), guide, { ...fresh, storageAvailable: false }).state, "unknown");
  });
}

for (const raw of ["{broken", "null", "[]", "{}", "x".repeat(4097)]) {
  test(`unreadable or incomplete support intent is preserved: ${raw.slice(0, 12)}`, () => {
    const memory = storage(), key = "meta-forge-deployment-support-v1:" + guide.id;
    memory.values.set(key, raw);
    assert.throws(() => loadDeploymentSupportIntent(memory, guide.id));
    assert.equal(memory.values.get(key), raw);
  });
}

test("a changed retained support identity cannot be cleared by an older response", () => {
  const memory = storage(), command = intent();
  storeDeploymentSupportIntent(memory, command);
  const changed = { ...command, action_request_id: REVISION };
  memory.values.set("meta-forge-deployment-support-v1:" + guide.id, JSON.stringify(changed));
  assert.throws(() => clearDeploymentSupportIntent(memory, command));
  assert.deepEqual(loadDeploymentSupportIntent(memory, guide.id), changed);
  memory.removeItem = () => {};
  assert.throws(() => clearDeploymentSupportIntent(memory, changed));
});

test("a foreign guide intent cannot be restored or dispatched as the selected guide", () => {
  const memory = storage(), command = { ...intent(), guide_id: "another-guide" };
  const key = "meta-forge-deployment-support-v1:" + guide.id;
  memory.values.set(key, JSON.stringify(command));
  assert.throws(() => loadDeploymentSupportIntent(memory, guide.id));
  assert.deepEqual(JSON.parse(memory.values.get(key)), command);
  assert.equal(deploymentSupportBodyFromIntent(command, {
    receipt: receipt(), guide, initial: true, ...fresh,
  }), null);
});

test("fresh bound backend projection yields eligible and prepared cards without touching inputs", () => {
  const value = receipt(), before = clone(value);
  const card = deploymentSupportState(value, guide, fresh);
  assert.equal(card.state, "eligible");
  assert.equal(card.action, "prepare-support");
  assert.equal(card.continuation, false);
  assert.deepEqual(deploymentSupportAction(value, guide, card.action, fresh), {
    deployment_id: value.deployment_id, action: "prepare-support", expected_service_digest: BEFORE });
  assert.deepEqual(value, before);
  const prepared = selected(), saved = clone(prepared);
  assert.equal(deploymentSupportState(prepared, guide, fresh).state, "prepared");
  assert.deepEqual(deploymentSupportAction(prepared, guide, "apply-support", fresh), {
    deployment_id: prepared.deployment_id, action: "apply-support", revision: REVISION, descriptor_sha256: DESCRIPTOR });
  assert.deepEqual(prepared, saved);
});

test("unknown status and malformed or switched projection never authorize support", () => {
  assert.equal(deploymentSupportState(receipt(), guide).state, "unknown");
  assert.equal(deploymentSupportAction(receipt(), guide, "prepare-support"), null);
  for (const change of [{ scope: "other" }, { deployment_id: "other" }, { build_id: "other" },
    { package_sha256: AFTER }, { requires_stopped: "true" }, { available: 1 },
    { service_digest: "unknown" }, { phase: "pending" }, { apply_available: true }, { selected: {} },
    { reason: [] }, { prepare_available: false }, { stopped_verified: undefined },
    { stopped_verified: "true" }, { stopped_verified: false }]) {
    const value = receipt(); Object.assign(value.support_request, change);
    assert.equal(deploymentSupportRequestValid(value.support_request, value, guide), false);
    assert.equal(deploymentSupportState(value, guide, fresh).state, "unknown");
    assert.equal(deploymentSupportAction(value, guide, "prepare-support", fresh), null);
  }
});

test("native-team, remote, interactive, staged candidate and incomplete cleanup are refused", () => {
  for (const change of [{ source_kind: "native-team" }, { source_kind: "unknown" }, { target: "gcp" },
    { execution_mode: "interactive" }, { service: null }, { status: "running" }, { process_live: true },
    { runtime_pid: 123 }, { runtime_pid: undefined }, { service_cleanup_confirmed: false },
    { guide_id: "other" }, { update: { status: "staged", runtime_dependency_update: { to_version: "0.8.48" } } },
    { update: { status: "blocked" } }, { update: { status: "recovery-needed" } }]) {
    const value = { ...receipt(), ...change };
    assert.equal(deploymentSupportState(value, guide, fresh).state, "refused");
    assert.equal(deploymentSupportAction(value, guide, "prepare-support", fresh), null);
  }
  assert.equal(deploymentSupportState({ ...receipt(), update: { status: "completed" } }, guide, fresh).state, "eligible");
});

test("selected descriptor metadata must be typed and exact before applying", () => {
  for (const change of [{ revision: "not-a-UUID" }, { descriptor_sha256: "bad" }, { phase: "staged" },
    { before_service_digest: AFTER }, { after_service_digest: true }, { retained_files: true },
    { staged_files: 0 }, { staged_files: 257 }]) {
    const value = selected(); Object.assign(value.support_request.selected, change);
    assert.equal(deploymentSupportAction(value, guide, "apply-support", fresh), null);
  }
  const publishing = selected("publishing"); publishing.support_request.service_digest = AFTER;
  assert.equal(deploymentSupportAction(publishing, guide, "apply-support", fresh).revision, REVISION);
});

test("completed history never authorizes startup or another apply and explicit refusal wins", () => {
  const value = selected("completed");
  const card = deploymentSupportState(value, guide, fresh);
  assert.equal(card.state, "completed");
  assert.match(card.label, /update completed/);
  assert.equal(card.action, null);
  for (const action of ["apply-support", "restart", "start", "retry", "stop"])
    assert.equal(deploymentSupportAction(value, guide, action, fresh), null);
  value.support_request.reason = "The current source could not be verified.";
  assert.equal(deploymentSupportState(value, guide, fresh).state, "refused");
  const current = receipt();
  Object.assign(current.support_request, { available: false, prepare_available: false, phase: "current" });
  assert.equal(deploymentSupportState(current, guide, fresh).label, "Installation support is current");
});

test("cold saved support intent observes by exact URL and never replays by default", () => {
  const memory = storage(), value = receipt(), command = intent();
  storeDeploymentSupportIntent(memory, command);
  const restored = loadDeploymentSupportIntent(memory, guide.id);
  assert.equal(deploymentSupportObservationUrl(restored),
    `/api/guide/${guide.id}/deployment/action/${value.deployment_id}/${REQUEST}`);
  assert.equal(deploymentSupportBodyFromIntent(restored), null);
  assert.equal(deploymentSupportBodyFromIntent(restored, { receipt: value, guide, ...fresh }), null);
  assert.equal(deploymentSupportAction(value, guide, "prepare-support", { ...fresh, intent: restored }), null);
  const card = deploymentSupportState(value, guide, { ...fresh, intent: restored });
  assert.equal(card.state, "unknown");
  assert.equal(card.action, null);
  assert.match(card.detail, /not be sent again automatically/);
});

test("metadata-only pending and settled observations use the exact nested command identity", () => {
  const value = selected(), command = intent("apply-support", value);
  const pending = { action_request: { ...command, phase: "pending", outcome: null } };
  assert.equal(deploymentSupportRequestMatches(pending, command), true);
  assert.equal(deploymentSupportRequestSettled(pending, command), false);
  assert.deepEqual(deploymentSupportBodyFromIntent(command, {
    receipt: value, guide, ...fresh, resume: true, observation: pending,
  }), { deployment_id: value.deployment_id, action: "apply-support", action_request_id: REQUEST,
    revision: REVISION, descriptor_sha256: DESCRIPTOR, resume: true });
  assert.equal(deploymentSupportBodyFromIntent(command, {
    receipt: value, guide, resume: true, observation: pending,
  }), null, "Command metadata cannot confirm installation status.");
  for (const outcome of ["returned", "error"]) {
    const settled = { action_request: { ...command, phase: "settled", outcome } };
    assert.equal(deploymentSupportRequestSettled(settled, command), true);
    assert.equal(deploymentSupportBodyFromIntent(command, {
      receipt: value, guide, ...fresh, resume: true, observation: settled,
    }), null);
  }
});

test("first dispatch must be explicit and preserve the full bound support request", () => {
  const value = receipt(), command = intent();
  assert.deepEqual(deploymentSupportBodyFromIntent(command, { initial: true, receipt: value, guide, ...fresh }), {
    deployment_id: value.deployment_id, action: "prepare-support", action_request_id: REQUEST,
    expected_service_digest: BEFORE });
  assert.equal(deploymentSupportBodyFromIntent(command, { initial: true, receipt: value, guide }), null);
  assert.equal(deploymentSupportBodyFromIntent(command, { initial: "true", receipt: value, guide, ...fresh }), null);
  value.support_request.service_digest = AFTER;
  assert.equal(deploymentSupportBodyFromIntent(command, { initial: true, receipt: value, guide, ...fresh }), null);
});

test("explicit pending prepare continuation reuses its deterministic selected revision", () => {
  const command = intent(), value = selected("prepared", REQUEST), observed = observation(command);
  assert.equal(value.support_request.prepare_available, false);
  const options = { ...fresh, intent: command, observation: observed };
  assert.equal(deploymentSupportState(value, guide, options).state, "in-progress");
  assert.equal(deploymentSupportState(value, guide, options).continuation, true);
  assert.equal(deploymentSupportAction(value, guide, "prepare-support", options), null);
  assert.deepEqual(deploymentSupportAction(value, guide, "prepare-support", { ...options, resume: true }), {
    deployment_id: value.deployment_id, action: "prepare-support", action_request_id: REQUEST,
    expected_service_digest: BEFORE, resume: true });
  assert.equal(deploymentSupportBodyFromIntent(command, { initial: true, resume: true, receipt: value, guide, ...options }), null);
});

test("pending apply can explicitly settle completed custody without creating a new request", () => {
  const command = intent("apply-support", selected()), value = selected("completed");
  const observed = observation(command), options = { ...fresh, intent: command, observation: observed, resume: true };
  assert.equal(value.support_request.apply_available, false);
  assert.deepEqual(deploymentSupportAction(value, guide, "apply-support", options), {
    deployment_id: value.deployment_id, action: "apply-support", action_request_id: REQUEST,
    revision: REVISION, descriptor_sha256: DESCRIPTOR, resume: true });
  assert.equal(deploymentSupportAction(value, guide, "prepare-support", options), null);
});

test("pending prepare can explicitly settle its own completed custody with the original before digest", () => {
  const command = intent(), value = selected("completed", REQUEST), observed = observation(command);
  const options = { ...fresh, intent: command, observation: observed, resume: true };
  assert.equal(value.support_request.service_digest, AFTER);
  assert.deepEqual(deploymentSupportAction(value, guide, "prepare-support", options), {
    deployment_id: value.deployment_id, action: "prepare-support", action_request_id: REQUEST,
    expected_service_digest: BEFORE, resume: true,
  });
  const changed = clone(value); changed.support_request.selected.before_service_digest = DESCRIPTOR;
  assert.equal(deploymentSupportAction(changed, guide, "prepare-support", options), null);
  const foreign = selected("completed");
  assert.equal(deploymentSupportAction(foreign, guide, "prepare-support", options), null);
});

test("canonical stopped history cannot resume without fresh stopped-owner verification", () => {
  const command = intent("apply-support", selected()), value = selected("completed");
  const observed = observation(command), options = { ...fresh, intent: command, observation: observed, resume: true };
  value.support_request.stopped_verified = false;
  assert.equal(deploymentSupportRequestValid(value.support_request, value, guide), true);
  assert.equal(deploymentSupportState(value, guide, options).state, "unknown");
  assert.equal(deploymentSupportAction(value, guide, "apply-support", options), null);
  value.support_request.service_digest = null;
  value.support_request.reason = "The stopped service owner could not be verified.";
  assert.equal(deploymentSupportState(value, guide, options).state, "refused");
  assert.equal(deploymentSupportAction(value, guide, "apply-support", options), null);
  const current = selected("completed");
  const enriched = { ...clone(current), ...observation(command) };
  current.support_request.stopped_verified = false;
  assert.equal(deploymentSupportBodyFromIntent(command, {
    receipt: current, guide, ...fresh, resume: true, observation: enriched,
  }), null, "An observation envelope cannot replace fresh stopped-owner proof.");
});

test("verified completed custody can prepare a different maintained source only with explicit availability", () => {
  const value = selected("completed");
  value.support_request.prepare_available = true;
  value.support_request.available = true;
  assert.equal(value.support_request.phase, "completed");
  assert.equal(deploymentSupportState(value, guide, fresh).state, "eligible");
  assert.deepEqual(deploymentSupportAction(value, guide, "prepare-support", fresh), {
    deployment_id: value.deployment_id, action: "prepare-support", expected_service_digest: AFTER,
  });
  assert.equal(deploymentSupportAction(value, guide, "apply-support", fresh), null);
});

test("changed command bindings or unknown observations cannot resume or settle support", () => {
  const value = selected(), command = intent("apply-support", value), observed = observation(command);
  for (const change of [{ guide_id: "other" }, { deployment_id: "other" }, { action_request_id: REVISION },
    { action: "prepare-support" }, { update_id: "other" }, { expected_service_digest: BEFORE },
    { revision: REQUEST }, { descriptor_sha256: AFTER }, { phase: "unknown" }, { outcome: "returned" }]) {
    const bad = clone(observed); Object.assign(bad.action_request, change);
    assert.equal(deploymentSupportRequestMatches(bad, command), false);
    assert.equal(deploymentSupportRequestSettled(bad, command), false);
    assert.equal(deploymentSupportBodyFromIntent(command, { receipt: value, guide, ...fresh, resume: true, observation: bad }), null);
  }
});

test("pending metadata cannot override changed source, stopped state or selected custody in the fresh receipt", () => {
  const value = selected(), command = intent("apply-support", value), observed = observation(command);
  for (const change of [{ guide_id: "other" }, { deployment_id: "other" }, { build_id: "other" },
    { package_sha256: AFTER }, { support_request: null }, { execution_mode: "interactive" },
    { source_kind: "native-team" }, { status: "running" }, { service_cleanup_confirmed: false },
    { process_live: true }, { runtime_pid: 123 }, { service: null }, { target: "gcp" }]) {
    const changed = { ...clone(value), ...change };
    assert.equal(deploymentSupportBodyFromIntent(command, {
      receipt: changed, guide, ...fresh, resume: true, observation: observed,
    }), null);
  }
  for (const change of [{ revision: REQUEST }, { descriptor_sha256: AFTER }, { before_service_digest: DESCRIPTOR }]) {
    const changed = clone(value); Object.assign(changed.support_request.selected, change);
    assert.equal(deploymentSupportBodyFromIntent(command, {
      receipt: changed, guide, ...fresh, resume: true, observation: observed,
    }), null);
  }
  const changed = clone(value); changed.support_request.service_digest = AFTER;
  assert.equal(deploymentSupportBodyFromIntent(command, {
    receipt: changed, guide, ...fresh, resume: true, observation: observed,
  }), null);
});

test("settlement proves only the exact support command and does not claim current support", () => {
  const value = selected(), command = intent("apply-support", value);
  for (const outcome of ["returned", "error"]) {
    const observed = observation(command, "settled", outcome);
    assert.equal(deploymentSupportRequestSettled(observed, command), true);
    assert.equal(deploymentSupportState(value, guide, { ...fresh, intent: command, observation: observed }).action, null);
    assert.equal(deploymentSupportBodyFromIntent(command, { receipt: value, guide, ...fresh, resume: true, observation: observed }), null);
  }
  const ordinary = { version: 1, guide_id: guide.id, deployment_id: value.deployment_id,
    action_request_id: REQUEST, action: "restart", update_id: null, phase: "settled", outcome: "returned" };
  assert.equal(deploymentSupportRequestSettled({ ...value, action_request: ordinary }, command), false);
});

test("existing guide selection and polling gates fence late support results across away and back", async () => {
  const polling = createDeploymentPollingGate(), memory = storage(), command = intent();
  storeDeploymentSupportIntent(memory, command);
  let active = { id: guide.id, generation: 1 };
  const selectedToken = active, scope = { capture(value, token) {
    return () => active === token && active.id === value.id;
  } };
  const ownerCurrent = captureGuideSelection(scope, guide, selectedToken);
  const opened = polling.open(guide.id), pollToken = polling.capture(guide.id);
  const late = Promise.resolve(observation(command, "settled", "returned"));
  active = { id: "other-guide", generation: 2 }; polling.close(opened); polling.open(active.id);
  active = { id: guide.id, generation: 3 }; polling.open(active.id);
  const response = await late;
  let admitted = false;
  if (ownerCurrent() && polling.current(pollToken, active.id) && deploymentSupportRequestSettled(response, command)) {
    clearDeploymentSupportIntent(memory, command); admitted = true;
  }
  assert.equal(admitted, false);
  assert.deepEqual(loadDeploymentSupportIntent(memory, guide.id), command);
});
