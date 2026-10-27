import { remoteReceiptOwned, remoteReceiptBinding, remoteReceiptCurrent, remoteNativeReady, remoteCloudRemoval,
  remoteProductSelected, remoteProductControl, remoteControlBindingKey, remoteProductActionSettled } from "./remote-deployment.mjs";
import { deploymentConsole } from "./deployment-panel.mjs";
import { artifactVersionCurrent } from './artifact-current.mjs';

const HASH = /^[a-f0-9]{64}$/;
const UUID = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const STATUSES = new Set(["closed", "connecting", "ready", "blocked", "unavailable", "cleanup-needed"]);
const RUNTIME_KEYS = ["runtime_pid", "startup_attempt_id", "interactive_lifecycle_id", "execution_mode",
  "service_instance_id", "service_supervisor_pid"];
const MUTATIONS = new Set(["snapshotting", "applying", "verifying", "restoring", "recovering-candidate",
  "recovering-current", "recovery-needed"]);
export const REMOTE_BROWSER_FRESH_MS = 15_000;

function productBindingOwned(binding, receipt) {
  return !!remoteControlBindingKey(binding) && binding.install_operation_id === receipt.install_operation_id &&
    binding.control_manifest_sha256 === receipt.product_control_manifest_sha256 &&
    ["guide_id", "guide_version", "build_id", "accepted_source_digest", "package_sha256", "transferred_source_digest", "controller_sha256"]
      .every((key) => binding.binding[key] === receipt[key]);
}

function runtimeKey(value) {
  if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).length !== RUNTIME_KEYS.length ||
      RUNTIME_KEYS.some((key) => !Object.hasOwn(value, key)) ||
      ["runtime_pid", "service_supervisor_pid"].some((key) => value[key] !== null &&
        (!Number.isSafeInteger(value[key]) || value[key] <= 0)) ||
      ["startup_attempt_id", "interactive_lifecycle_id", "service_instance_id"].some((key) => value[key] !== null &&
        (typeof value[key] !== "string" || !UUID.test(value[key]))) ||
      value.execution_mode !== null && !["interactive", "background"].includes(value.execution_mode)) return null;
  return JSON.stringify(RUNTIME_KEYS.map((key) => value[key]));
}

function productInstance(receipt, state) {
  const product = remoteProductControl(receipt, state), actual = product?.factory_status;
  if (!product || actual.target !== "local" || actual.status !== receipt.status) return null;
  const runtime = Object.fromEntries(RUNTIME_KEYS.map((key) => [key, actual[key] ?? null]));
  const runtimeBinding = runtimeKey(runtime);
  return runtimeBinding ? [remoteControlBindingKey(product.binding), runtimeBinding, actual.console_url ?? null] : null;
}

export function remoteBrowserProductNativeReady(receipt, state) {
  const product = remoteProductControl(receipt, state), actual = product?.factory_status;
  const action = product?.action_request, native = product?.native_evidence;
  return remoteNativeReady(receipt) && !!product && receipt.product_runtime === true && receipt.service_owned === false && receipt.service_observed === false &&
    receipt.service && typeof receipt.service === "object" && !Array.isArray(receipt.service) && Object.keys(receipt.service).length === 0 &&
    receipt.pending_operation == null &&
    (action == null || !!remoteProductActionSettled(receipt, state, action.action_request_id, action.action)) &&
    actual.status === "running" && !["update", "recovery"].includes(actual.phase) && !MUTATIONS.has(actual.update?.status) &&
    Number.isSafeInteger(actual.runtime_pid) && actual.runtime_pid > 0 && UUID.test(actual.startup_attempt_id || "") &&
    (actual.execution_mode === "interactive" && UUID.test(actual.interactive_lifecycle_id || "") ||
      actual.execution_mode === "background" && UUID.test(actual.service_instance_id || "") &&
        Number.isSafeInteger(actual.service_supervisor_pid) && actual.service_supervisor_pid > 0) &&
    native?.owned_source_verified === true && native.native_runtime_available === true &&
    native.deployment_id === receipt.deployment_id && native.build_id === receipt.build_id &&
    native.source_digest === receipt.accepted_source_digest &&
    !!productInstance(receipt, state) && !!consoleAddress(actual.console_url) && deploymentConsole(actual) === actual.console_url;
}

function consoleAddress(value) {
  try {
    const url = new URL(value);
    if (url.protocol !== "http:" || url.hostname !== "127.0.0.1" || url.username || url.password ||
        url.pathname !== "/console" || url.search || url.hash || !/^\d{4,5}$/.test(url.port) ||
        Number(url.port) < 1024 || Number(url.port) > 65535 || url.href !== value) return null;
    return url.href;
  } catch { return null; }
}

export function remoteBrowserUrl(guideId, deploymentId, action = "") {
  const base = `/api/guide/${encodeURIComponent(guideId)}/remote-deployment/browser-access`;
  return action ? `${base}/${action}` : `${base}?deployment_id=${encodeURIComponent(deploymentId)}`;
}

export function remoteBrowserQualified(receipt, options, build, state, blocked = false, unavailable = false) {
  if (remoteProductSelected(receipt)) return !unavailable && remoteBrowserOwned(receipt, state) &&
    remoteReceiptCurrent(receipt, options, build, state, blocked) && remoteNativeReady(receipt) &&
    remoteBrowserProductNativeReady(receipt, state);
  return !unavailable && remoteBrowserOwned(receipt, state) && remoteReceiptCurrent(receipt, options, build, state, blocked) && remoteNativeReady(receipt) &&
    receipt.remote_admission_requested === true && receipt.admitted === true && receipt.worker_live === false && !receipt.pending_operation &&
    receipt.service_owned === true && receipt.service_observed === true &&
    receipt.service.ready === true && receipt.service.status === "running" && receipt.service.source_current === true;
}

export function remoteBrowserBinding(receipt, state, build) {
  if (remoteProductSelected(receipt)) return JSON.stringify([state?.id, state?.version, build?.build_id, build?.source_digest,
    remoteReceiptBinding(receipt), receipt?.controller_sha256, receipt?.host_key_fingerprint,
    [receipt?.host?.uid, receipt?.host?.home, receipt?.host?.destination], productInstance(receipt, state)]);
  return JSON.stringify([state?.id, state?.version, build?.build_id, build?.source_digest, remoteReceiptBinding(receipt),
    receipt?.controller_sha256, receipt?.service?.service, receipt?.service?.source_digest, receipt?.service?.console_url]);
}

export function remoteBrowserOwned(receipt, state) {
  // Saved installation identity keeps an admitted forward inspectable for Close.
  // Fresh product/native observations are required separately for every opening.
  if (remoteProductSelected(receipt)) return remoteReceiptOwned(receipt, state) && receipt.product_runtime === true &&
    receipt.remote_admission_requested === true && receipt.admitted === true &&
    HASH.test(receipt.controller_sha256 || "") && HASH.test(receipt.transferred_source_digest || "") &&
    UUID.test(receipt.install_operation_id || "") && HASH.test(receipt.product_control_manifest_sha256 || "");
  return remoteReceiptOwned(receipt, state) && HASH.test(receipt.controller_sha256 || "") &&
    HASH.test(receipt.transferred_source_digest || "") && receipt.service?.definition_verified === true &&
    receipt.service.scope === "native-bootstrap-and-console" && receipt.service.source_digest === receipt.transferred_source_digest &&
    typeof receipt.service.service === "string" &&
    UUID.test(receipt.service.service.match(/^mobkit-launch\.(.+)\.service$/)?.[1] || "") &&
    !!consoleAddress(receipt.service.console_url);
}

export function remoteBrowserOwnedBinding(receipt, state) {
  if (remoteProductSelected(receipt)) return remoteBrowserOwned(receipt, state) ? JSON.stringify([
    remoteReceiptBinding(receipt), receipt.controller_sha256, receipt.install_operation_id, receipt.product_control_manifest_sha256,
    receipt.host_key_fingerprint, [receipt.host?.uid, receipt.host?.home, receipt.host?.destination]]) : "";
  return remoteBrowserOwned(receipt, state) ? JSON.stringify([remoteReceiptBinding(receipt), receipt.controller_sha256,
    receipt.service.service, receipt.service.source_digest, receipt.service.console_url]) : "";
}

export function remoteBrowserReplyBindingOwned(value, receipt, state) {
  if (!remoteBrowserOwned(receipt, state)) return false;
  if (remoteProductSelected(receipt)) return value.service_name === null && value.service_source_digest === null &&
    productBindingOwned(value.product_control_binding, receipt) && !!runtimeKey(value.product_runtime_binding) &&
    (value.console_url === null || !!consoleAddress(value.console_url));
  return value.service_name === receipt.service?.service && value.service_source_digest === receipt.service?.source_digest &&
    HASH.test(value.service_source_digest || "") && !!consoleAddress(receipt.service?.console_url) &&
    (value.console_url == null || value.console_url === receipt.service.console_url);
}

export function remoteBrowserReplyBindingCurrent(value, receipt, state) {
  if (!remoteBrowserReplyBindingOwned(value, receipt, state)) return false;
  if (!remoteProductSelected(receipt)) return true;
  const instance = productInstance(receipt, state);
  return receipt.source_binding_current === true && !!instance &&
    remoteControlBindingKey(value.product_control_binding) === instance[0] &&
    runtimeKey(value.product_runtime_binding) === instance[1] && value.console_url === instance[2];
}

export function remoteBrowserObservedTime(value) {
  if (typeof value === "number" && Number.isFinite(value) && value > 0) return value < 1e12 ? value * 1000 : value;
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?(?:Z|\+00:00)$/.test(value)) return null;
  const time = Date.parse(value);
  return Number.isFinite(time) ? time : null;
}

export function remoteBrowserReplyOwned(value, receipt, state) {
  if (!remoteBrowserOwned(receipt, state) || !value || value.protocol !== 1 || value.guide_id !== receipt.guide_id ||
      value.guide_version !== receipt.guide_version || value.build_id !== receipt.build_id || value.deployment_id !== receipt.deployment_id ||
      value.entry_id !== "native-console" || !STATUSES.has(value.status) || typeof value.native_ready !== "boolean" ||
      typeof value.cleanup_confirmed !== "boolean" ||
      !["local-password", "loopback-only"].includes(value.scope?.authentication) ||
      value.scope.health_scope !== "native-bootstrap-and-console" || value.scope.custom_interfaces_verified !== false ||
      typeof value.scope.requested_interfaces !== "string" || remoteBrowserObservedTime(value.observed_at) === null ||
      value.access_id !== null && !UUID.test(value.access_id || "")) return false;
  for (const name of ["accepted_source_digest", "package_sha256", "transferred_source_digest", "controller_sha256"])
    if (!HASH.test(value[name] || "") || value[name] !== receipt[name]) return false;
  if (!remoteBrowserReplyBindingOwned(value, receipt, state)) return false;
  const expected = consoleAddress(remoteProductSelected(receipt) ? value.console_url : receipt.service?.console_url);
  if (!remoteProductSelected(receipt) && !expected || value.console_url != null && value.console_url !== expected) return false;
  if (value.status === "ready") return !!value.access_id && value.native_ready === true && value.cleanup_confirmed === false &&
    value.console_url === expected && value.local_url === expected && !!consoleAddress(value.local_url) &&
    remoteBrowserObservedTime(value.observed_at) !== null;
  return value.local_url === null;
}

export function remoteBrowserReplyCurrent(value, receipt, state, build) {
  return remoteBrowserReplyOwned(value, receipt, state) && artifactVersionCurrent(value, state) &&
    value.build_id === build?.build_id && value.accepted_source_digest === build?.source_digest &&
    remoteBrowserReplyBindingCurrent(value, receipt, state);
}

export function remoteBrowserConsole(value, receipt, state, build, now = Date.now()) {
  if (remoteCloudRemoval(receipt) || !remoteBrowserReplyCurrent(value, receipt, state, build) || value.status !== "ready") return null;
  if (remoteProductSelected(receipt) && (!remoteNativeReady(receipt) || !remoteBrowserProductNativeReady(receipt, state))) return null;
  const observed = remoteBrowserObservedTime(value.observed_at);
  return observed <= now + 3000 && now - observed <= REMOTE_BROWSER_FRESH_MS ? value.local_url : null;
}

export function remoteBrowserCanClose(value) {
  return UUID.test(value?.access_id || "") && value.cleanup_confirmed !== true;
}

export function remoteBrowserCanOpen(value) {
  return !!value && ["closed", "blocked", "unavailable"].includes(value.status) && value.cleanup_confirmed === true;
}

export function remoteBrowserStatus(value, unavailable = false, closing = false) {
  if (closing) return "Closing your browser connection";
  if (unavailable) return "Browser connection could not be verified";
  return ({ closed: "Connect to your native Console", connecting: "Opening your browser connection",
    ready: "Your browser connection is ready", blocked: "Browser connection needs attention",
    unavailable: "Browser connection could not be verified", "cleanup-needed": "Browser connection cleanup needs attention" })[value?.status]
    || "Check your browser connection";
}
