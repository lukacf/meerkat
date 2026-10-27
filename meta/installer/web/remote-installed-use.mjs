import { artifactIsCurrent } from "./artifact-current.mjs";
import { remoteConnectionInput, remoteProductSelected, remoteProductControl, remoteProductActionSettled,
  remoteControlBindingKey } from "./remote-deployment.mjs";
import { gcpEndpointValid } from "./gcp-guided.mjs";
import { remoteApplicationExperiences } from "./remote-application-access.mjs";
import { createInstalledUseDraftStore, installedUseStartingIdeas, installedUseTaskAllowed,
  INSTALLED_USE_OUTCOMES, createInstalledUseSuccessStore, installedUseSuccessRequest,
  installedUseSuccessRecord, createInstalledUseImprovementStore, installedUseImprovementHint } from "./deployment-use.mjs";
import { installedTaskFragment } from "./installed-task-handoff.mjs";

const HASH = /^[a-f0-9]{64}$/;
const UUID = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const FINGERPRINT = /^SHA256:[A-Za-z0-9+/]{20,90}={0,2}$/;
const BINDING_KEYS = ["deployment_id", "guide_id", "guide_version", "build_id", "accepted_source_digest",
  "package_sha256", "transferred_source_digest", "controller_sha256", "endpoint", "host", "destination", "host_key_fingerprint"];
const BUSY = new Set(["connecting", "installing", "checking", "starting", "stopping", "restarting", "recovery-needed"]);
const TERMINAL = new Set(["completed", "failed", "interrupted"]);
const STATES = new Set([...BUSY, "configuration-required", "not-installed", "running", "stopped", "failed", "observation-unavailable"]);
const digest = (value) => typeof value === "string" && HASH.test(value);
const text = (value, maximum) => typeof value === "string" && value.trim() && value.length <= maximum &&
  !/[\x00-\x1f\x7f]/.test(value);
const record = (value) => !!value && typeof value === "object" && !Array.isArray(value);
const keys = (value, allowed) => record(value) && Object.keys(value).length === allowed.length &&
  Object.keys(value).every((key) => allowed.includes(key));
const path = (value) => text(value, 2000) && value.startsWith("/") && value !== "/" &&
  !value.slice(1).split("/").some((part) => !part || part === "." || part === "..");

export function canonicalRemoteInstalledBinding(value) {
  const cloudEndpoint = value?.endpoint?.transport === "gcp-iap";
  if (!keys(value, BINDING_KEYS) || typeof value.guide_id !== "string" || !UUID.test(value.guide_id) || !text(value.build_id, 64) ||
      typeof value.deployment_id !== "string" || !UUID.test(value.deployment_id) ||
      !Number.isSafeInteger(value.guide_version) || value.guide_version < 1 ||
      !["accepted_source_digest", "package_sha256", "transferred_source_digest", "controller_sha256"].every((key) => digest(value[key])) ||
      !(cloudEndpoint ? gcpEndpointValid(value.endpoint) : keys(value.endpoint, ["host", "user", "ssh_port"])) ||
      !text(value.endpoint.host, 255) || !text(value.endpoint.user, 64) || !Number.isInteger(value.endpoint.ssh_port) ||
      !remoteConnectionInput({ ...value.endpoint, destination: value.destination }) ||
      value.endpoint.host !== value.endpoint.host.trim() || value.endpoint.user !== value.endpoint.user.trim() ||
      !keys(value.host, ["uid", "home", "destination"]) || !Number.isSafeInteger(value.host.uid) || value.host.uid < 0 || value.host.uid > 2 ** 32 - 1 ||
      !path(value.host.home) || !path(value.destination) ||
      value.host.destination !== value.destination || typeof value.host_key_fingerprint !== "string" || !FINGERPRINT.test(value.host_key_fingerprint)) return null;
  return Object.fromEntries(BINDING_KEYS.map((key) => [key, key === "endpoint"
    ? cloudEndpoint ? Object.fromEntries(Object.keys(value.endpoint).sort().map((name) => [name, value.endpoint[name]]))
      : { host: value.endpoint.host, user: value.endpoint.user, ssh_port: value.endpoint.ssh_port }
    : key === "host" ? { uid: value.host.uid, home: value.host.home, destination: value.host.destination } : value[key]]));
}

export function remoteInstalledUseBinding(receipt, state) {
  if (receipt?.protocol !== 1 || receipt.guide_id !== state?.id ||
      (receipt.endpoint?.transport === "gcp-iap" ? receipt.target !== "gcp" || receipt.cloud_operation_id !== receipt.endpoint.operation_id
        : receipt.target != null && receipt.target !== "linux")) return "";
  const value = canonicalRemoteInstalledBinding(Object.fromEntries(BINDING_KEYS.map((key) => [key, key === "host"
    ? { uid: receipt.host?.uid, home: receipt.host?.home, destination: receipt.host?.destination } : receipt[key]])));
  return value ? JSON.stringify(value) : "";
}

export function remoteInstalledUseDraftScope(receipt, state) {
  return remoteInstalledUseBinding(receipt, state) ? JSON.stringify([state.id, "remote:" + receipt.deployment_id,
    receipt.build_id, receipt.package_sha256, receipt.guide_version]) : "";
}

// Reuse the local bounded draft implementation; storage ownership includes the
// remote host, trusted key and source pins so another destination cannot read it.
export function createRemoteInstalledUseDraftStore(storage, binding) {
  if (!storage || !binding) return createInstalledUseDraftStore(null);
  const prefix = "mobkit-remote-installed-use-v1:" + binding + ":";
  return createInstalledUseDraftStore({
    getItem(key) { return storage.getItem(prefix + key); },
    setItem(key, value) { storage.setItem(prefix + key, value); },
    removeItem(key) { storage.removeItem(prefix + key); },
  });
}

export function createRemoteInstalledUseSuccessStore(storage, binding) {
  if (!storage || !binding) return createInstalledUseSuccessStore(null);
  const prefix = "mobkit-remote-installed-use-v1:" + binding + ":";
  return createInstalledUseSuccessStore({
    getItem(key) { return storage.getItem(prefix + key); },
    setItem(key, value) { storage.setItem(prefix + key, value); },
    removeItem(key) { storage.removeItem(prefix + key); },
  });
}

export function createRemoteInstalledUseImprovementStore(storage, binding) {
  if (!storage || !binding) return createInstalledUseImprovementStore(null);
  const prefix = "mobkit-remote-installed-use-v1:" + binding + ":";
  return createInstalledUseImprovementStore({
    getItem(key) { return storage.getItem(prefix + key); },
    setItem(key, value) { storage.setItem(prefix + key, value); },
    removeItem(key) { storage.removeItem(prefix + key); },
  });
}

export function remoteInstalledUseImprovementHint(receipt, state, body) {
  const binding = remoteInstalledUseBinding(receipt, state);
  const supplied = canonicalRemoteInstalledBinding(body?.remote_binding);
  if (!binding || !supplied || JSON.stringify(supplied) !== binding) return null;
  return installedUseImprovementHint(remoteInstalledUseDraftScope(receipt, state), { ...body,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    source_digest: receipt.accepted_source_digest, package_sha256: receipt.package_sha256 });
}

const PRODUCT_MUTATIONS = new Set(["snapshotting", "applying", "verifying", "restoring", "recovering-candidate",
  "recovering-current", "recovery-needed"]);

/** An idle authenticated product controller remains live between application actions. */
export function remoteInstalledUseBusy(receipt, state) {
  if (remoteProductSelected(receipt)) {
    const product = remoteProductControl(receipt, state), actual = product?.factory_status;
    const action = product?.action_request;
    return !product || !["running", "stopped"].includes(receipt.status) || actual.status !== receipt.status ||
      receipt.pending_operation != null ||
      action != null && !remoteProductActionSettled(receipt, state, action.action_request_id, action.action) ||
      actual.status === "running" && ["update", "recovery"].includes(actual.phase) ||
      PRODUCT_MUTATIONS.has(actual.update?.status);
  }
  return BUSY.has(receipt?.status) || receipt?.status === "observation-unavailable" && BUSY.has(receipt.last_known_status) ||
    receipt?.worker_live != null && typeof receipt.worker_live !== "boolean" || receipt?.worker_live === true ||
    receipt?.pending_operation != null;
}

export function remoteInstalledUseReadKey(receipt, state, build) {
  const binding = remoteInstalledUseBinding(receipt, state);
  const product = remoteProductSelected(receipt) ? remoteProductControl(receipt, state) : null;
  const action = product?.action_request;
  const activity = remoteProductSelected(receipt) ? [remoteInstalledUseBusy(receipt, state),
    product ? remoteControlBindingKey(product.binding) : null, product?.factory_status.status,
    product?.factory_status.phase, product?.factory_status.update?.status,
    action?.action_request_id, action?.action, action?.phase, action?.outcome] : null;
  return binding ? JSON.stringify([binding, state.version, receipt.status, receipt.observation?.available,
    receipt.last_known_status, receipt.source_current, receipt.ready, receipt.worker_live, receipt.pending_operation,
    build?.build_id, build?.source_digest, build?.status,
    build?.current, build?.source_verified, ...(remoteProductSelected(receipt) ? [activity] : [])]) : "";
}

export function remoteInstalledUseUrl(guideId, deploymentId, operation = "use") {
  const base = "/api/guide/" + encodeURIComponent(guideId) + "/remote-deployment/";
  return operation === "feedback" ? base + "feedback"
    : operation === "success" ? base + "use/success"
    : base + "use?deployment_id=" + encodeURIComponent(deploymentId);
}

export function remoteInstalledApplicationUse(metadata, receipt, state, build) {
  const binding = remoteInstalledUseBinding(receipt, state);
  const supplied = canonicalRemoteInstalledBinding(metadata?.remote_binding);
  if (!binding || !supplied || JSON.stringify(supplied) !== binding || metadata?.protocol !== 1 ||
      metadata.scope !== "frozen-remote-package-source" || metadata.guide_id !== state.id ||
      metadata.version !== state.version || metadata.guide_version !== receipt.guide_version ||
      metadata.deployment_id !== receipt.deployment_id || metadata.build_id !== receipt.build_id ||
      metadata.package_sha256 !== receipt.package_sha256 || metadata.status !== receipt.status || !STATES.has(receipt.status) ||
      metadata.accepted_source_digest !== receipt.accepted_source_digest ||
      metadata.source_digest !== receipt.accepted_source_digest ||
      metadata.transferred_source_digest !== receipt.transferred_source_digest ||
      metadata.controller_sha256 !== receipt.controller_sha256 || metadata.application_outcomes_verified !== false ||
      typeof metadata.installed_source_verified !== "boolean" || typeof metadata.native_runtime_available !== "boolean" ||
      !record(metadata.observation) || typeof metadata.observation.available !== "boolean" ||
      metadata.observation.scope !== "saved-destination-observation" || metadata.observation.status !== receipt.status ||
      !["fresh", "stale", "unavailable"].includes(metadata.observation.freshness) ||
      typeof metadata.observation.admitted !== "boolean" || typeof metadata.observation.source_current !== "boolean" ||
      metadata.observation.available !== (metadata.observation.freshness === "fresh") ||
      !metadata.observation.available && (metadata.observation.admitted || metadata.observation.source_current ||
        metadata.installed_source_verified || metadata.native_runtime_available) ||
      !text(metadata.project?.name, 200) || !text(metadata.entrypoint?.identity, 180) ||
      !text(metadata.entrypoint?.name, 200)) return null;
  const selected = metadata.continuation, newer = selected?.build_id !== receipt.build_id ||
    selected?.source_digest !== receipt.accepted_source_digest;
  const available = selected?.available === true && artifactIsCurrent(build, state) &&
    build.source_verified === true && TERMINAL.has(build.status) && selected.build_id === build.build_id &&
    digest(selected.source_digest) && selected.source_digest === build.source_digest;
  const interfaces = remoteApplicationExperiences(receipt);
  // Missing optional link metadata does not identify the application's human
  // channel or authorize a native conversation as its everyday task interface.
  return { name: metadata.project.name, entrypoint: { identity: metadata.entrypoint.identity, name: metadata.entrypoint.name },
    interfaces, noDeclaredUi: interfaces.supported === true && interfaces.status === "none",
    startingIdeas: installedUseStartingIdeas(metadata.first_use),
    installedVerified: metadata.installed_source_verified, nativeAvailable: metadata.native_runtime_available,
    observationAvailable: metadata.observation.available,
    continuation: { available, newerCandidate: newer, reason: text(selected?.reason, 600) ? selected.reason : null,
      requiresAcknowledgement: metadata.current !== true || newer || selected?.requires_acknowledgement === true } };
}

export function remoteInstalledUseFeedback(metadata, receipt, state, build, report, blocked = false) {
  const use = remoteInstalledApplicationUse(metadata, receipt, state, build);
  if (blocked || remoteInstalledUseBusy(receipt, state) ||
      !use?.continuation.available || !INSTALLED_USE_OUTCOMES.some((item) => item.id === report?.outcome) ||
      typeof report.request !== "string" || !report.request.trim() || report.request.length > 6000 ||
      !installedUseTaskAllowed(report.task ?? "") ||
      use.continuation.requiresAcknowledgement && report.continueCurrentCandidate !== true) return null;
  return { version: state.version, remote_binding: JSON.parse(remoteInstalledUseBinding(receipt, state)),
    continuation_build_id: build.build_id, continuation_source_digest: build.source_digest,
    continue_current_candidate: report.continueCurrentCandidate === true, outcome: report.outcome,
    request: report.request.trim(), task: (report.task ?? "").trim() };
}

export function remoteInstalledUseSuccessBody(metadata, receipt, state, task, requestId) {
  if (receipt?.remote_admission_requested !== true || receipt?.admitted !== true ||
      metadata?.success_reporting_available === false || remoteInstalledUseBusy(receipt, state) ||
      !remoteProductSelected(receipt) && receipt?.worker_live !== false ||
      !remoteInstalledApplicationUse(metadata, receipt, state, null)) return null;
  return installedUseSuccessRequest({ version: state.version, deployment_id: receipt.deployment_id,
    build_id: receipt.build_id, source_digest: receipt.accepted_source_digest, package_sha256: receipt.package_sha256 }, task, requestId);
}

export function remoteInstalledUseSuccessReport(value, metadata, receipt, state, body = null) {
  if (!remoteInstalledApplicationUse(metadata, receipt, state, null)) return null;
  return installedUseSuccessRecord(value, { version: state.version, deployment_id: receipt.deployment_id,
    build_id: receipt.build_id, source_digest: receipt.accepted_source_digest, package_sha256: receipt.package_sha256 }, state.id, body);
}

/** Project only the checked source and editable task; tunnel custody stays separate. */
export function remoteInstalledUseTaskHandoff(metadata, receipt, state, build, task) {
  const use = remoteInstalledApplicationUse(metadata, receipt, state, build);
  const entryId = use?.startingIdeas?.entryId;
  if (receipt?.status !== "running" || receipt.admitted !== true || receipt.ready !== true ||
      receipt.source_current !== true || receipt.observation?.available !== true || remoteInstalledUseBusy(receipt, state) ||
      !remoteProductSelected(receipt) && receipt.worker_live !== false || !use?.installedVerified || !use.nativeAvailable || !use.observationAvailable || !entryId ||
      use.interfaces.status !== "resolved" || !use.interfaces.entries.some((entry) => entry.id === entryId) ||
      !installedTaskFragment(task)) return null;
  return { readKey: remoteInstalledUseReadKey(receipt, state, build), binding: remoteInstalledUseBinding(receipt, state), entryId, task };
}
