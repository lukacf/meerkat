import { gcpEndpointValid } from './gcp-guided.mjs';
import { artifactVersionCurrent } from './artifact-current.mjs';
import { deploymentCanPrepare, deploymentOptionsCurrent, deploymentSourceKind } from "./deployment-panel.mjs";
import { deploymentRequiredFields, deploymentFieldIssues, deploymentFieldConstraintsValid } from "./deployment-configuration.mjs";

const HASH = /^[a-f0-9]{64}$/;
const UUID = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const STATUSES = new Set(["connecting", "configuration-required", "installing", "checking", "starting",
  "running", "stopping", "restarting", "stopped", "failed", "not-installed", "observation-unavailable", "recovery-needed"]);
const IN_PROGRESS = new Set(["connecting", "installing", "checking", "starting", "stopping", "restarting"]);
const INSTALL_ADMITTED = new Set(["installing", "checking", "starting", "running", "stopping", "restarting", "stopped"]);
const plainText = (value, maximum) => typeof value === "string" && value.trim() && value.length <= maximum &&
  !/[\u0000-\u001f\u007f]/.test(value);

export function remoteDeploymentUrl(guideId, action = "") {
  return `/api/guide/${encodeURIComponent(guideId)}/remote-deployment${action ? `/${action}` : ""}`;
}

export function remoteSelectionKey(state, build) {
  return JSON.stringify([state?.id, state?.version, build?.build_id, build?.source_digest]);
}

// Retain only volatile, unsent private values across a transcript-only change.
// Requests and observed authority continue using the versioned selection key.
export function remotePrivateDraftScope(state, build) {
  return JSON.stringify([state?.id, build?.build_id, build?.source_digest, build?.blueprint_hash,
    build?.package_sha256, state?.proposal || state?.blueprint, state?.brief]);
}

export function remoteCanConnect(options, build, state, blocked = false) {
  return !blocked && deploymentSourceKind(options) === "application" && deploymentCanPrepare(build, state) && deploymentOptionsCurrent(options, build, state) &&
    (options.target === "gcp" || options.target === "linux" && options.supported === false && options.host_target !== "linux") &&
    HASH.test(build.source_digest || "");
}

export function remoteConnectionInput(draft) {
  const host = draft?.host?.trim(), user = draft?.user?.trim(), destination = draft?.destination?.trim();
  const rawPort = String(draft?.ssh_port ?? "22");
  if (!plainText(host, 255) || !/^[A-Za-z0-9_[\]][A-Za-z0-9_.:[\]-]*$/.test(host) ||
      !plainText(user, 64) || !/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(user) ||
      !/^\d{1,5}$/.test(rawPort) || Number(rawPort) < 1 || Number(rawPort) > 65535 ||
      !plainText(destination, 2000) || !(destination.startsWith("/") || destination.startsWith("~/"))) return null;
  return { host, user, ssh_port: Number(rawPort), destination };
}

export function remoteReceiptOwned(receipt, state) {
  return receipt?.protocol === 1 && receipt.guide_id === state?.id &&
    plainText(receipt.deployment_id, 200) && Number.isSafeInteger(receipt.guide_version) && receipt.guide_version >= 0 &&
    plainText(receipt.build_id, 200) && HASH.test(receipt.accepted_source_digest || "") &&
    HASH.test(receipt.package_sha256 || "") &&
    (receipt.transferred_source_digest == null || HASH.test(receipt.transferred_source_digest)) &&
    plainText(receipt.destination, 2000) && STATUSES.has(receipt.status) &&
    !!remoteConnectionInput({ ...receipt.endpoint, destination: receipt.destination }) &&
    (receipt.target === "gcp" ? gcpEndpointValid(receipt.endpoint) && receipt.cloud_operation_id === receipt.endpoint.operation_id &&
      (receipt.cloud_status == null || ["removing", "removal-required", "removed"].includes(receipt.cloud_status)) :
      receipt.target == null || receipt.target === "linux");
}

export function remoteReceiptBinding(receipt) {
  return JSON.stringify([receipt?.deployment_id, receipt?.guide_id, receipt?.guide_version, receipt?.build_id,
    receipt?.accepted_source_digest, receipt?.package_sha256, receipt?.transferred_source_digest,
    receipt?.destination, receipt?.endpoint?.host, receipt?.endpoint?.user, receipt?.endpoint?.ssh_port,
    ...(receipt?.target === "gcp" ? [receipt.target, receipt.cloud_operation_id, Object.entries(receipt.endpoint).sort(([a], [b]) => a.localeCompare(b))] : [])]);
}

export function remoteReceiptCurrent(receipt, options, build, state, blocked = false) {
  return remoteReceiptOwned(receipt, state) && !remoteCloudRemoval(receipt) && receipt.source_binding_current === true &&
    artifactVersionCurrent(receipt, state) &&
    receipt.build_id === build?.build_id && receipt.accepted_source_digest === build?.source_digest &&
    remoteCanConnect(options, build, state, blocked);
}

export function remoteCloudRemoval(receipt) {
  return receipt?.target === "gcp" && ["removing", "removal-required", "removed"].includes(receipt.cloud_status)
    ? receipt.cloud_status : null;
}

/** A new checked package may reuse only a never-admitted setup's public destination. */
export function remotePendingBuildHandoff(receipt, options, build, state, blocked = false) {
  if (receipt?.target === "gcp" || options?.target === "gcp" || !remoteReceiptOwned(receipt, state) || !remoteCanConnect(options, build, state, blocked) ||
      receipt.source_binding_current !== false || receipt.build_id === build.build_id ||
      receipt.guide_version > state.version || !remoteObservationAvailable(receipt) ||
      receipt.remote_admission_requested !== false || receipt.admitted === true || receipt.worker_live === true ||
      !["configuration-required", "not-installed", "failed"].includes(receipt.status) ||
      receipt.status === "not-installed" && receipt.admitted !== false) return null;
  const input = remoteConnectionInput({ ...receipt.endpoint, destination: receipt.destination });
  if (!input) return null;
  return {
    key: JSON.stringify([remoteReceiptBinding(receipt), receipt.status, remoteSelectionKey(state, build)]),
    guideId: state.id, version: state.version, buildId: build.build_id,
    previousDeploymentId: receipt.deployment_id, input,
  };
}

export function remoteObservationAvailable(receipt) {
  return !!receipt && receipt.status !== "observation-unavailable" && receipt.observation?.available === true;
}

export function remotePrivateOptions(receipt) {
  if (receipt?.preflight?.ready !== true || !Array.isArray(receipt.fields) || receipt.fields.length > 100) return null;
  const keys = new Set();
  if (!receipt.fields.every((field) => field && typeof field.key === "string" && /^[A-Z][A-Z0-9_]*$/.test(field.key) &&
      !keys.has(field.key) && keys.add(field.key) && plainText(field.label, 240) &&
      typeof field.required === "boolean" && typeof field.secret === "boolean" &&
      deploymentFieldConstraintsValid(field) &&
      (field.description == null || typeof field.description === "string" && field.description.length <= 2000) &&
      (field.default == null || !field.secret && typeof field.default === "string" && field.default.length <= 32768))) return null;
  const authentication = receipt.console_scope?.authentication;
  if (Object.hasOwn(receipt, "console_scope") && !["loopback-only", "local-password"].includes(authentication)) return null;
  if (authentication === "local-password" &&
      ![["CONSOLE_USER_ID", false], ["CONSOLE_LOGIN_PASSWORD", true]].every(([key, secret]) =>
        receipt.fields.some((field) => field.key === key && field.required === true && field.secret === secret &&
          !Object.hasOwn(field, "default")))) return null;
  return { fields: receipt.fields.map((field) => ({ ...field, configured: false })), host_provider_keys: [],
    can_use_host_provider_credentials: false,
    ...(authentication ? { console_scope: { authentication } } : {}) };
}

export function remoteCanConfigure(receipt, options, build, state, blocked = false) {
  const configurable = receipt?.status === "configuration-required" ||
    receipt?.status === "not-installed" && receipt.admitted === false;
  return remoteCanRetryInstall(receipt, options, build, state, blocked) ||
    remoteReceiptCurrent(receipt, options, build, state, blocked) && remoteObservationAvailable(receipt) &&
    receipt.can_install === true && configurable &&
    receipt.preflight?.ready === true && !!remotePrivateOptions(receipt);
}

export function remoteCanRetryInstall(receipt, options, build, state, blocked = false) {
  return remoteReceiptCurrent(receipt, options, build, state, blocked) && remoteObservationAvailable(receipt) &&
    receipt.status === "failed" && receipt.can_retry_install === true &&
    UUID.test(receipt.retry_of_operation_id || "") && receipt.retry_of_operation_id === receipt.operation_id &&
    receipt.worker_live === false && receipt.cleanup_confirmed === true && receipt.source_current === true &&
    receipt.preflight?.ready === true && !!remotePrivateOptions(receipt);
}

export function remotePrivateBinding(receipt) {
  return JSON.stringify([remoteReceiptBinding(receipt), receipt?.status === "failed"
    ? ["retry", receipt.retry_of_operation_id, receipt.operation_id] : ["install"]]);
}

export function remotePrivateDraftBound(receipt, options, build, state, formBinding) {
  if (!formBinding || formBinding !== remotePrivateBinding(receipt) ||
      !remoteReceiptCurrent(receipt, options, build, state)) return false;
  // Volatile draft retention does not grant permission to install. A temporary
  // design block or failed HTTP read can leave the same observed setup intact.
  if (remoteCanConfigure(receipt, options, build, state)) return true;
  // SSH observation loss withholds package fields. Keep only the draft for an
  // exact destination whose initial installation has never been requested.
  return receipt.status === "observation-unavailable" &&
    receipt.last_known_status === "configuration-required" &&
    receipt.remote_admission_requested === false && receipt.preflight?.ready === true;
}

export function remotePrivateDraftRetained(receipt, options, build, state, formBinding) {
  if (!formBinding || formBinding !== remotePrivateBinding(receipt) || !remoteReceiptOwned(receipt, state) ||
      remoteCloudRemoval(receipt) || receipt.source_binding_current !== true ||
      receipt.build_id !== build?.build_id || receipt.accepted_source_digest !== build?.source_digest) return false;
  if (remotePrivateDraftBound(receipt, options, build, state, formBinding)) return true;
  // Retention grants no form authority. A new receipt can arrive before the
  // matching options/build response, so do not clear its unsent values then.
  const observedVersion = receipt.current_guide_version === undefined ? receipt.guide_version : receipt.current_guide_version;
  if (!Number.isSafeInteger(observedVersion) || observedVersion > state.version) return false;
  if (receipt.status === 'failed') return receipt.can_retry_install === true && receipt.worker_live === false &&
    receipt.cleanup_confirmed === true && receipt.source_current === true && remoteObservationAvailable(receipt) &&
    UUID.test(receipt.retry_of_operation_id || '') && receipt.retry_of_operation_id === receipt.operation_id &&
    receipt.preflight?.ready === true && !!remotePrivateOptions(receipt);
  return receipt.status === 'configuration-required' && receipt.remote_admission_requested !== true && receipt.admitted !== true && receipt.can_install === true &&
    remoteObservationAvailable(receipt) && receipt.preflight?.ready === true && !!remotePrivateOptions(receipt) ||
    receipt.status === 'observation-unavailable' && receipt.last_known_status === 'configuration-required' &&
    receipt.remote_admission_requested === false && receipt.preflight?.ready === true;
}

export function remoteFailureGuidance(receipt) {
  if (receipt?.status !== "failed") return null;
  const guidance = {
    "python-environment": { label: "Python environment setup did not finish.", detail: "Check free disk space, folder permissions and Python virtual environment support." },
    dependencies: { label: "Dependency installation did not finish.", detail: "Check package download access and free disk space on your Linux computer." },
    "application-dependencies": { label: "Application dependency installation did not finish.", detail: "Check package download access and the application's required packages." },
    "native-tools": { label: "MobKit tool installation did not finish.", detail: "Check access to the official release downloads and free disk space." },
    "application-source": { label: "Application source checks did not finish.", detail: "Review the checks below. A source correction needs a new accepted build." },
    "knowledge-setup": { label: "Knowledge setup did not finish.", detail: "Check the private provider settings and the destination checks below." },
    "knowledge-start": { label: "Knowledge startup did not finish.", detail: "Check Docker, Compose and the owned knowledge service on your Linux computer." },
    "service-check": { label: "Application startup checks did not finish.", detail: "Review the installed checks and required private settings below." },
    "service-install": { label: "Application startup did not finish.", detail: "Recheck remote status and the installed checks before another attempt." },
    "service-retry-install": { label: "Application startup did not finish.", detail: "Recheck remote status and the installed checks before another attempt." },
  };
  return Object.hasOwn(guidance, receipt.failure_phase) ? guidance[receipt.failure_phase] : null;
}

export function remoteInstallSettings(privateOptions, values) {
  const result = {};
  for (const field of privateOptions?.fields || [])
    if (!field.automatic && typeof values[field.key] === "string" && values[field.key]) result[field.key] = values[field.key];
  return result;
}

const consolePort = (value) => typeof value === "string" && /^[1-9][0-9]{3,4}$/.test(value) &&
  Number(value) >= 1024 && Number(value) <= 65535 ? Number(value) : null;

/** Only a never-admitted setup may select a new Console address here. */
export function remoteSetupConsolePort(receipt, options, build, state, blocked = false) {
  if (!remoteCanConfigure(receipt, options, build, state, blocked) ||
      receipt.remote_admission_requested !== false || receipt.admitted === true ||
      options.source_digest !== build.source_digest ||
      !Number.isSafeInteger(options.console_port)) return null;
  return consolePort(String(options.console_port));
}

export function remoteSetupConsolePortValue(receipt, options, build, state, values, blocked = false) {
  const defaultPort = remoteSetupConsolePort(receipt, options, build, state, blocked);
  return defaultPort === null ? null : values.MOBKIT_PORT ?? String(defaultPort);
}

/** The scoped options response observes this computer, not the Linux guest. */
export function remoteSetupPortObservation(checkedOptions, receipt, options, build, state, value, blocked = false) {
  const selected = consolePort(value), observation = checkedOptions?.console_port_observation;
  if (selected === null || remoteSetupConsolePort(receipt, options, build, state, blocked) === null ||
      !deploymentOptionsCurrent(checkedOptions, build, state) || checkedOptions.source_digest !== build.source_digest ||
      checkedOptions.console_port !== options.console_port ||
      observation?.scope !== "local-console-port-observation" || observation.port !== selected ||
      !["available", "occupied", "unknown"].includes(observation.status) ||
      typeof observation.observed_at !== "string" || !Number.isFinite(Date.parse(observation.observed_at))) return null;
  const alternate = observation.alternate_port;
  return { port: selected, status: observation.status,
    alternatePort: observation.status === "occupied" && Number.isSafeInteger(alternate) &&
      consolePort(String(alternate)) !== null && alternate !== selected ? alternate : null };
}

export function remoteSetupInstallSettings(receipt, options, build, state, values, blocked = false) {
  const result = remoteInstallSettings(remotePrivateOptions(receipt), values);
  const value = remoteSetupConsolePortValue(receipt, options, build, state, values, blocked);
  if (value !== null && consolePort(value) !== null) result.MOBKIT_PORT = value;
  return result;
}

export function remoteCanInstall(receipt, options, build, state, values, blocked = false) {
  const port = remoteSetupConsolePortValue(receipt, options, build, state, values, blocked);
  const initial = receipt?.remote_admission_requested === false && receipt?.admitted !== true;
  return remoteCanConfigure(receipt, options, build, state, blocked) &&
    (port === null ? !initial && !Object.hasOwn(values, "MOBKIT_PORT") : consolePort(port) !== null) &&
    deploymentRequiredFields(remotePrivateOptions(receipt), values, false).length === 0 &&
    deploymentFieldIssues(remotePrivateOptions(receipt), values).length === 0;
}

export function remoteProductSelected(receipt) {
  return receipt?.product_runtime === true || receipt?.product_control != null ||
    receipt?.product_scope === "retained-original-product-owner";
}

export function remoteControlBindingKey(value) {
  if (value?.protocol !== 1 || !UUID.test(value.install_operation_id || "") ||
      !HASH.test(value.control_manifest_sha256 || "") || !HASH.test(value.initial_install_sha256 || "")) return null;
  const binding = value.binding;
  if (!binding || !Number.isSafeInteger(binding.guide_version) ||
      !["guide_id", "build_id"].every((key) => plainText(binding[key], 200)) ||
      !["accepted_source_digest", "package_sha256", "transferred_source_digest", "controller_sha256"].every((key) => HASH.test(binding[key] || ""))) return null;
  const births = [value.worker, value.command];
  if (!births.every((birth) => birth && Number.isSafeInteger(birth.pid) && birth.pid > 0 &&
      Number.isSafeInteger(birth.uid) && birth.uid >= 0 && plainText(birth.start_ticks, 100) && plainText(birth.boot_id, 100)) ||
      births[0].uid !== births[1].uid) return null;
  return JSON.stringify([value.protocol, value.install_operation_id, value.control_manifest_sha256, value.initial_install_sha256,
    binding.guide_id, binding.guide_version, binding.build_id, binding.accepted_source_digest, binding.package_sha256,
    binding.transferred_source_digest, binding.controller_sha256,
    ...births.map((birth) => [birth.pid, birth.uid, birth.start_ticks, birth.boot_id])]);
}

/** Presentation of the authenticated existing owner; this grants no runtime authority. */
export function remoteProductControl(receipt, state) {
  const product = receipt?.product_control, held = product?.binding, actual = product?.factory_status;
  if (!remoteReceiptOwned(receipt, state) || remoteCloudRemoval(receipt) || !remoteObservationAvailable(receipt) ||
      receipt.product_scope !== "retained-original-product-owner" || product?.owner_live !== true ||
      product.initial_install_complete !== true || receipt.worker_live !== true || receipt.cleanup_confirmed !== false ||
      !remoteControlBindingKey(held) || held.install_operation_id !== receipt.install_operation_id ||
      held.control_manifest_sha256 !== receipt.product_control_manifest_sha256 ||
      ["guide_id", "guide_version", "build_id", "accepted_source_digest", "package_sha256", "transferred_source_digest", "controller_sha256"]
        .some((key) => held.binding[key] !== receipt[key]) ||
      ["guide_id", "guide_version", "deployment_id", "build_id", "package_sha256", "destination"]
        .some((key) => actual?.[key] !== receipt[key]) ||
      actual?.source_verified !== true || actual.source_digest !== receipt.accepted_source_digest ||
      !["queued", "installing", "checking", "starting", "running", "restarting", "stopping", "stopped", "failed", "interrupted"].includes(actual.status)) return null;
  return product;
}

export function remoteProductActionSettled(receipt, state, identity, action) {
  const record = remoteProductControl(receipt, state)?.action_request;
  return UUID.test(identity || "") && record?.action_request_id === identity && record.action === action &&
    record.guide_id === state.id && record.deployment_id === receipt.deployment_id && record.phase === "settled" &&
    ["returned", "error"].includes(record.outcome) ? record : null;
}

export function remoteActionBody(receipt, action, options, build, state, identity, blocked = false) {
  if (!remoteActionAvailable(receipt, action, options, build, state, blocked)) return null;
  const body = { deployment_id: receipt.deployment_id, action };
  if (!remoteProductSelected(receipt)) return body;
  return UUID.test(identity || "") ? { ...body, action_request_id: identity } : null;
}

export function remoteActionAvailable(receipt, action, options, build, state, blocked = false) {
  if (!remoteReceiptOwned(receipt, state) || remoteCloudRemoval(receipt)) return false;
  if (remoteProductSelected(receipt)) {
    const product = remoteProductControl(receipt, state);
    if (!product || receipt.pending_operation) return false;
    if (action === "stop") return receipt.can_stop === true && ["queued", "installing", "checking", "starting", "running", "restarting", "stopping"].includes(product.factory_status.status);
    if (!remoteReceiptCurrent(receipt, options, build, state, blocked)) return false;
    const flags = { restart: "can_restart", "prepare-setup": "can_prepare_setup", "finish-setup": "can_finish_setup", "start-background": "can_start_background" };
    return Object.hasOwn(flags, action) && receipt[flags[action]] === true && product.factory_status.status === "stopped" &&
      receipt.source_current === true && product.factory_status.process_live === false && product.factory_status.runtime_pid == null;
  }
  if (action === "stop") return receipt.can_stop === true;
  return action === "restart" && remoteReceiptCurrent(receipt, options, build, state, blocked) &&
    remoteObservationAvailable(receipt) && !IN_PROGRESS.has(receipt.status) && receipt.status !== "recovery-needed" && receipt.can_restart === true;
}

export function remoteIsBusy(receipt) { return IN_PROGRESS.has(receipt?.status); }
export function remoteInstallAdmitted(receipt) { return INSTALL_ADMITTED.has(receipt?.status); }

export function remoteDeploymentChecks(receipt) {
  const checks = Array.isArray(receipt?.checks) ? receipt.checks : [];
  if (receipt?.admitted === true) return checks;
  const destination = new Set((Array.isArray(receipt?.preflight?.checks) ? receipt.preflight.checks : [])
    .map((check) => JSON.stringify(check)));
  return checks.filter((check) => !destination.has(JSON.stringify(check)));
}

export function remoteDeploymentActivity(progress) {
  const activity = [];
  let previous = null;
  for (const entry of Array.isArray(progress) ? progress : []) {
    if (!entry || typeof entry.message !== "string") continue;
    // Repeated observations have new times without being new activity. Keep
    // changes in phase, attempt or other evidence, including repeated failures.
    const key = JSON.stringify(Object.fromEntries(Object.entries(entry).filter(([name]) => name !== "at")
      .sort(([left], [right]) => left.localeCompare(right))));
    if (key !== previous) activity.push(entry);
    previous = key;
  }
  return activity;
}

export function remoteNativeReady(receipt) {
  return !remoteCloudRemoval(receipt) && receipt?.status === "running" && remoteObservationAvailable(receipt) &&
    receipt.source_current === true && receipt.ready === true;
}

const DEPLOYMENT_STEPS = new Map([
  ["admitted", "Preparing your request"],
  ["configuration-pending", "Preparing private application settings"],
  ["python-environment", "Preparing Python"],
  ["dependencies", "Installing runtime dependencies"],
  ["application-dependencies", "Installing application dependencies"],
  ["native-tools", "Installing MobKit"],
  ["application-source", "Checking application code"],
  ["knowledge-setup", "Preparing the knowledge base"],
  ["knowledge-start", "Starting the knowledge base"],
  ["service-install", "Starting and checking your application"],
  ["service-retry-install", "Starting and checking your application"],
  ["service-stop", "Stopping your application"],
  ["service-restart", "Restarting and checking your application"],
  ["service-check", "Checking application startup"],
  ["complete", "Application operation finished"],
  ["attention-required", "Application needs attention"],
]);

export function remoteDeploymentStep(receipt, unavailable = false) {
  const label = DEPLOYMENT_STEPS.get(receipt?.phase);
  if (!label) return null;
  const current = !unavailable && remoteObservationAvailable(receipt) && remoteIsBusy(receipt);
  return { phase: receipt.phase, label, caption: current ? "Current step" : "Last observed step" };
}

export function remotePhase(receipt) {
  if (remoteCloudRemoval(receipt)) return receipt.cloud_status === "removed" ? "This cloud installation's resources were removed"
    : "This cloud installation is being removed";
  if (receipt?.status === "running" && !remoteNativeReady(receipt)) return "Your remote runtime needs a verified readiness check";
  return ({ connecting: "Checking your Linux computer", "configuration-required": "Your destination is ready for private setup",
    installing: "Installing the accepted application", checking: "Checking the installed application",
    starting: "Starting your agents", running: "Your remote runtime is running", stopping: "Stopping your owned application",
    restarting: "Restarting your accepted application", stopped: "Your remote application is stopped",
    "not-installed": "Your remote application is not installed",
    failed: "Your installation needs a recovery step", "observation-unavailable": "Remote status could not be checked",
    "recovery-needed": "Remote cleanup needs to be verified" })[receipt?.status]
    || "Run on your Linux computer";
}
