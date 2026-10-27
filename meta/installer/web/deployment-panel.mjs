import { artifactIsCurrent } from "./artifact-current.mjs";
import { packagePreparationReason } from "./package-availability.mjs";
import { deploymentRequiredFields, deploymentRemovalKeys, deploymentFieldIssues } from "./deployment-configuration.mjs";
const ACTIVE = new Set(["queued", "installing", "checking", "starting", "running", "stopping"]);

export function deploymentIsActive(receipt) {
  return ACTIVE.has(receipt?.status);
}

export function deploymentSourceKind(value) {
  return value?.source_kind === "native-team" ? "native-team" : "application";
}

/** A saved installation sets the initial choice, independently of a newer build. */
export function deploymentInstalledSourceKind(receipt, guideId) {
  if (!receipt || !guideId || receipt.guide_id !== guideId || typeof receipt.deployment_id !== "string" || !receipt.deployment_id) return null;
  if (receipt.source_kind === "native-team") return nativeSource(receipt) ? "native-team" : null;
  if (receipt.source_kind != null && receipt.source_kind !== "application") return null;
  return typeof receipt.build_id === "string" && !!receipt.build_id ? "application" : null;
}

export function nativeTeamCanInstall(guide) {
  return !!guide?.id && Number.isSafeInteger(guide.version) && guide.version >= 1 &&
    guide.status === "ready" && ["mac", "linux"].includes(guide.blueprint?.target) && !guide.proposal && guide.question == null &&
    guide.readiness?.ready === true && guide.operation?.status !== "running" && !packagePreparationReason(guide);
}

/** Current source selection, never a fabricated application build. */
export function deploymentSourceKey(build, guide, sourceKind = "application") {
  return JSON.stringify(sourceKind === "native-team"
    ? [sourceKind, guide?.id, guide?.version, guide?.blueprint, guide?.brief]
    : [sourceKind, guide?.id, build?.build_id, build?.source_digest]);
}

function nativeSource(value) {
  return value?.source_kind === "native-team" && value.build_id == null &&
    typeof value.source_id === "string" && /^[a-f0-9]{64}$/.test(value.source_id) &&
    typeof value.source_digest === "string" && /^[a-f0-9]{64}$/.test(value.source_digest);
}

export function deploymentCanPrepare(build, guide, sourceKind = "application") {
  if (sourceKind === "native-team") return nativeTeamCanInstall(guide);
  return artifactIsCurrent(build, guide) && build.status === "completed" && build.ready_to_package === true && build.cleanup_pending !== true &&
    build.acceptance_trusted === true && build.acceptance_current === true &&
    build.acceptance?.status === "pass";
}

export function deploymentOptionsCurrent(options, build, guide) {
  if (options?.source_kind === "native-team") return nativeTeamCanInstall(guide) && nativeSource(options) &&
    options.current === true && options.guide_version === guide.version && options.current_guide_version === guide.version &&
    options.target === guide.blueprint.target;
  return deploymentCanPrepare(build, guide) && options?.current === true &&
    options.build_id === build.build_id && options.guide_version === build.guide_version &&
    (options.source_digest === undefined || options.source_digest === build.source_digest) &&
    (options.current_guide_version === undefined ? options.guide_version : options.current_guide_version) === guide.version &&
    options.target === (guide.proposal || guide.blueprint)?.target;
}

/** A check belongs only to this selected source and the user's current port. */
export function deploymentPortObservation(options, build, guide, port, receipt = null) {
  const installed = options?.scope === "verified-installed-private-configuration";
  if (installed ? !deploymentCanConfigure(receipt, build, guide, options) : !deploymentCanInstall(build, guide, options)) return null;
  if (!installed && (typeof options.source_digest !== "string" || !/^[a-f0-9]{64}$/.test(options.source_digest) ||
      deploymentSourceKind(options) === "application" && options.source_digest !== build?.source_digest)) return null;
  const selected = typeof port === "string" && /^[0-9]+$/.test(port) ? Number(port) : null;
  const observation = options?.console_port_observation;
  if (!Number.isSafeInteger(selected) || selected < 1024 || selected > 65535 ||
      observation?.scope !== "local-console-port-observation" || observation.port !== selected ||
      !["available", "occupied", "unknown"].includes(observation.status) ||
      typeof observation.observed_at !== "string" || !Number.isFinite(Date.parse(observation.observed_at))) return null;
  const detail = {
    available: "This port was available when checked. Availability can change before startup.",
    occupied: "This port is unavailable on this computer. Choose another port or check again before starting.",
    unknown: "Port availability could not be checked. Check again before starting this installation.",
  }[observation.status];
  const alternate = observation.alternate_port;
  return { port: selected, status: observation.status, detail,
    alternatePort: observation.status === "occupied" && Number.isSafeInteger(alternate) &&
      alternate >= 1024 && alternate <= 65535 && alternate !== selected ? alternate : null };
}

export function deploymentCanInstall(build, guide, options) {
  return deploymentOptionsCurrent(options, build, guide) && options.supported === true &&
    options.target === options.host_target && ["mac", "linux"].includes(options.target);
}

/** Host compatibility also supports updates; fresh installation has its own package availability. */
export function deploymentCanFreshInstall(build, guide, options) {
  return deploymentCanInstall(build, guide, options) &&
    (!Object.hasOwn(options, "fresh_install") || options.fresh_install?.available === true);
}

export function deploymentFreshInstallRestriction(build, guide, options) {
  if (!deploymentCanInstall(build, guide, options) || deploymentCanFreshInstall(build, guide, options)) return null;
  return typeof options.fresh_install?.reason === "string" && options.fresh_install.reason.trim()
    ? options.fresh_install.reason : "Fresh installation is unavailable for this selected package. Check its deployment options again.";
}

/** Read the host's installation projection; this never authorizes startup. */
export function deploymentFreshInstallNeedsSetup(options) {
  return (options?.source_kind == null || options.source_kind === "application") && options?.fresh_install?.available === true &&
    options.fresh_install.next_step === "configure";
}

export function deploymentInstallSelection(options, build, guide) {
  if (!deploymentCanFreshInstall(build, guide, options)) return null;
  return deploymentSourceKind(options) === "native-team"
    ? { source_kind: "native-team", source_id: options.source_id, source_digest: options.source_digest }
    : { build_id: build.build_id };
}

export function deploymentHandoff(options, build, guide) {
  if (!deploymentOptionsCurrent(options, build, guide) || options.supported === true ||
      options.handoff?.mode !== "remote-package" || options.handoff.status !== "not-connected") return null;
  const nativeTeam = deploymentSourceKind(options) === "native-team";
  if (nativeTeam && (options.supported !== false || options.host_target === options.target)) return null;
  const packageUrl = nativeTeam
    ? `/api/guide/${encodeURIComponent(guide.id)}/deployment/package?${new URLSearchParams({ source_kind: "native-team", version: guide.version })}`
    : `/api/guide/${encodeURIComponent(guide.id)}/implementation/package?${new URLSearchParams({ build_id: build.build_id })}`;
  if (options.handoff.package_url !== packageUrl) return null;
  return options.handoff;
}

export function deploymentDestinationLabel(options) {
  return { mac: "your Mac", linux: "your Linux computer", gcp: "Google Cloud" }[options?.target] || "the selected destination";
}

export function deploymentUrl(guideId) {
  return `/api/guide/${encodeURIComponent(guideId)}/deployment`;
}

const ACTION_INTENT_PREFIX = "meta-forge-deployment-action-v1:";
const ACTION_REQUEST_UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const DEPLOYMENT_ACTIONS = new Set(["stop", "restart", "retry", "prepare-setup", "finish-setup", "start-background", "apply-update", "rollback", "recover-current"]);
const ACTION_INTENT_FIELDS = ["version", "guide_id", "deployment_id", "action_request_id", "action", "update_id"];

/** Browser intent retains only the exact command identity, never runtime truth or private settings. */
export function deploymentActionIntentValid(intent, guideId) {
  return !!intent && !Array.isArray(intent) && Object.keys(intent).length === ACTION_INTENT_FIELDS.length &&
    ACTION_INTENT_FIELDS.every((key) => Object.hasOwn(intent, key)) && intent.version === 1 &&
    typeof guideId === "string" && !!guideId && intent.guide_id === guideId &&
    typeof intent.deployment_id === "string" && !!intent.deployment_id && intent.deployment_id.length <= 200 &&
    ACTION_REQUEST_UUID.test(intent.action_request_id || "") && DEPLOYMENT_ACTIONS.has(intent.action) &&
    (intent.update_id === null || typeof intent.update_id === "string" && !!intent.update_id && intent.update_id.length <= 200) &&
    (["apply-update", "rollback", "recover-current"].includes(intent.action) ? intent.update_id !== null : intent.update_id === null);
}

export function deploymentActionIntent(guideId, body, requestId) {
  const intent = { version: 1, guide_id: guideId, deployment_id: body.deployment_id,
    action_request_id: requestId, action: body.action, update_id: body.update_id ?? null };
  if (!deploymentActionIntentValid(intent, guideId)) throw new Error("This installation command could not be identified. No command was sent.");
  return intent;
}

export function loadDeploymentActionIntent(storage, guideId) {
  const raw = storage.getItem(ACTION_INTENT_PREFIX + guideId);
  if (raw === null) return null;
  let intent;
  try { intent = JSON.parse(raw); } catch { throw new Error("The saved installation command is unreadable. Preserve it before sending another command."); }
  if (!deploymentActionIntentValid(intent, guideId)) throw new Error("The saved installation command does not identify this setup. Preserve it before sending another command.");
  return intent;
}

export function storeDeploymentActionIntent(storage, intent) {
  if (!deploymentActionIntentValid(intent, intent?.guide_id)) throw new Error("This installation command is invalid. No command was sent.");
  const key = ACTION_INTENT_PREFIX + intent.guide_id, encoded = JSON.stringify(intent);
  if (storage.getItem(key) !== null) throw new Error("Check the saved installation command before sending another command.");
  storage.setItem(key, encoded);
  if (storage.getItem(key) !== encoded) throw new Error("The installation command could not be saved. No command was sent.");
}

export function clearDeploymentActionIntent(storage, intent) {
  const retained = loadDeploymentActionIntent(storage, intent.guide_id);
  if (!retained || !ACTION_INTENT_FIELDS.every((key) => retained[key] === intent[key]))
    throw new Error("The saved installation command changed. Check it before sending another command.");
  storage.removeItem(ACTION_INTENT_PREFIX + intent.guide_id);
  if (storage.getItem(ACTION_INTENT_PREFIX + intent.guide_id) !== null)
    throw new Error("The resolved installation command could not be cleared. Check it before sending another command.");
}

export function deploymentActionObservationUrl(intent) {
  return `${deploymentUrl(intent.guide_id)}/action/${encodeURIComponent(intent.deployment_id)}/${encodeURIComponent(intent.action_request_id)}`;
}

export function deploymentActionRequestSettled(value, intent) {
  const observed = value?.action_request;
  return deploymentActionIntentValid(intent, intent?.guide_id) && !!observed &&
    ACTION_INTENT_FIELDS.every((key) => observed[key] === intent[key]) &&
    observed.phase === "settled" && ["returned", "error"].includes(observed.outcome);
}

/** Passive reads cannot replace a receipt admitted after those reads began. */
export function createDeploymentPollingGate() {
  let scope = null, epoch = 0;
  const sameScope = (token, guideId) => !!token && token.scope === scope && scope?.guideId === guideId;
  const current = (token, guideId) => sameScope(token, guideId) && token.epoch === epoch;
  const invalidate = (token, guideId) => {
    if (!current(token, guideId)) return false;
    epoch += 1;
    return true;
  };
  return {
    open(guideId) { scope = Object.freeze({ guideId }); epoch += 1; return scope; },
    close(selected) { if (scope === selected) { scope = null; epoch += 1; } },
    capture(guideId) { return scope?.guideId === guideId ? { scope, epoch } : null; },
    sameScope, current, invalidate, admit: invalidate,
    async observe(guideId, read) {
      const token = this.capture(guideId);
      if (!token) return null;
      try {
        const receipt = await read();
        return current(token, guideId) ? { receipt, token } : null;
      } catch (failure) {
        return current(token, guideId) ? { failure, token } : null;
      }
    },
  };
}

const HASH = /^[a-f0-9]{64}$/;
const SETTINGS_SCOPE = "verified-installed-private-configuration";

export function deploymentNeedsCleanup(receipt) {
  return !!receipt && (receipt.execution_mode || "interactive") === "interactive" && !receipt.service &&
    ["failed", "interrupted"].includes(receipt.status) &&
    !["restored-stopped", "candidate-restored-stopped"].includes(receipt.update?.status);
}

/** A launchd service whose own files are in a protected macOS folder keeps its projected note. */
export function deploymentProtectedFolderNote(receipt) {
  const note = receipt?.protected_folder;
  return receipt?.execution_mode === "background" && receipt?.service?.manager === "launchd" &&
    typeof note === "string" && note.trim() && note.length <= 500 ? note : null;
}

/** Ordinary Stop uses checked service ownership, independently of update support. */
export function deploymentCanStopFailedBackground(receipt, guide) {
  const service = receipt?.service, observed = receipt?.service_observation, summary = service?.public_summary;
  return deploymentInstalledSourceKind(receipt, guide?.id) !== null && receipt.execution_mode === "background" &&
    (receipt.target ?? "local") === "local" && ["failed", "interrupted"].includes(receipt.status) &&
    (!receipt.update || !deploymentBackgroundUpdate(receipt, guide).supported) &&
    typeof receipt.package_sha256 === "string" && HASH.test(receipt.package_sha256) &&
    observed?.owned === true && typeof observed.loaded === "boolean" && summary?.definition_verified === true &&
    ["launchd", "systemd-user"].includes(service.manager) && summary.manager === service.manager &&
    typeof observed.state === "string" && !!observed.state && observed.state !== "unknown" && summary.state === observed.state &&
    (observed.pid == null || Number.isSafeInteger(observed.pid) && observed.pid > 0) &&
    (summary.pid ?? null) === (observed.pid ?? null);
}

export function deploymentCanConfirmCleanup(receipt, guide) {
  const recovery = receipt?.cleanup_recovery;
  return deploymentNeedsCleanup(receipt) && receipt.guide_id === guide?.id &&
    receipt.process_live !== true && !receipt.runtime_pid &&
    recovery?.scope === "owned-interactive-cleanup" && recovery.available === true &&
    recovery.deployment_id === receipt.deployment_id && recovery.build_id === receipt.build_id &&
    Number.isSafeInteger(recovery.guide_version) && recovery.guide_version === receipt.guide_version &&
    typeof recovery.package_sha256 === "string" && HASH.test(recovery.package_sha256) && recovery.package_sha256 === receipt.package_sha256 &&
    typeof recovery.source_digest === "string" && HASH.test(recovery.source_digest) &&
    (receipt.source_digest == null || receipt.source_digest === recovery.source_digest);
}

export function deploymentResumeAction(receipt, guide) {
  if (!receipt || receipt.guide_id !== guide?.id || deploymentIsActive(receipt) || deploymentNeedsCleanup(receipt) ||
      ["applying", "verifying", "restoring", "recovering-candidate", "recovering-current", "recovery-needed"].includes(receipt.update?.status)) return null;
  const cleanup = receipt.interactive_cleanup;
  if (receipt.status === "stopped" && cleanup?.boundary === "worker-not-admitted") {
    if ((receipt.execution_mode || "interactive") !== "interactive" || receipt.service || receipt.process_live === true || receipt.runtime_pid ||
        cleanup.confirmed !== true || cleanup.worker_exit_confirmed !== true || typeof receipt.interactive_lifecycle_id !== "string" ||
        !/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(receipt.interactive_lifecycle_id) ||
        cleanup.lifecycle_id !== receipt.interactive_lifecycle_id || receipt.startup_attempt_id != null ||
        !Object.hasOwn(receipt, "interactive_cleanup_attempt_id") || receipt.interactive_cleanup_attempt_id !== null || cleanup.startup_attempt_id !== null ||
        !Number.isSafeInteger(cleanup.supervisor_pid) || cleanup.supervisor_pid <= 0 || cleanup.return_code !== 0 ||
        !Number.isSafeInteger(receipt.guide_version) ||
        ["deployment_id", "guide_id", "build_id", "guide_version", "blueprint_hash", "destination", "package_sha256"].some((key) => cleanup[key] !== receipt[key]) ||
        ["source_digest", "owner_sha256", "package_sha256"].some((key) => typeof cleanup[key] !== "string" || !HASH.test(cleanup[key]))) return null;
    return { action: "retry", label: "Retry installation",
      detail: "No application worker started. Retry completes the saved checked package's installation; private settings and ordinary Start stay unavailable until its files are verified." };
  }
  const resume = receipt.status === "stopped" || ["restored-stopped", "candidate-restored-stopped"].includes(receipt.update?.status);
  // A failed installer can have verified teardown without an installed SDK
  // proof. Offer the existing guarded retry, never ordinary runtime startup.
  if (resume && deploymentRequiresApplicationStartup(receipt) && receipt.installed_candidate_runtime_proof == null &&
      (receipt.target || "local") === "local" && (receipt.execution_mode || "interactive") === "interactive" &&
      !receipt.service && !receipt.update && receipt.process_live === false && receipt.runtime_pid == null &&
      receipt.startup_attempt_id === null && receipt.interactive_cleanup_attempt_id === null &&
      cleanup?.boundary === "owned-process-groups-absent" && cleanup.confirmed === true && cleanup.worker_exit_confirmed === true &&
      Number.isSafeInteger(cleanup.return_code) && cleanup.return_code !== 0 &&
      Number.isSafeInteger(cleanup.supervisor_pid) && cleanup.supervisor_pid > 0 &&
      typeof receipt.interactive_lifecycle_id === "string" && ACTION_REQUEST_UUID.test(receipt.interactive_lifecycle_id) &&
      cleanup.lifecycle_id === receipt.interactive_lifecycle_id && cleanup.startup_attempt_id === null &&
      Number.isSafeInteger(receipt.guide_version) &&
      ["deployment_id", "guide_id", "build_id", "guide_version", "blueprint_hash", "destination", "package_sha256"].every(key => cleanup[key] === receipt[key]) &&
      ["source_digest", "owner_sha256", "package_sha256"].every(key => typeof cleanup[key] === "string" && HASH.test(cleanup[key])) &&
      (receipt.source_digest == null || receipt.source_digest === cleanup.source_digest))
    return { action: "retry", label: "Retry installation",
      detail: "Retry this saved package using its saved private settings. Forge rechecks its source and cleanup before continuing; application startup remains a separate step." };
  if (resume && deploymentRequiresApplicationStartup(receipt) && !restoredBackgroundRestartAvailable(receipt, guide)) return null;
  return { action: resume ? "restart" : "retry", label: resume
    ? deploymentSourceKind(receipt) === "native-team" ? "Start installed agent team" : "Start installed application"
    : "Retry saved installation", detail: null };
}

/** Project the selected installation; the host still owns setup and startup admission. */
export function deploymentRequiresApplicationStartup(receipt) {
  return deploymentSourceKind(receipt) === "application" &&
    receipt?.runtime_candidate?.format === "forge-meta-toolkit-runtime-selection-v1";
}

function installedRuntimeMatches(receipt) {
  const selected = receipt?.runtime_candidate, proof = receipt?.installed_candidate_runtime_proof;
  return proof?.source === "meta-toolkit-product" && typeof selected?.meta?.version === "string" &&
    proof.version === selected.meta.version &&
    [["assembly", "assembly_sha256"], ["model_catalog", "catalog_sha256"]].every(([selection, observed]) =>
      typeof selected?.[selection]?.sha256 === "string" && HASH.test(selected[selection].sha256) &&
      proof[observed] === selected[selection].sha256) &&
    ["meta", "toolkit"].every((sdk) => typeof selected?.[sdk]?.sha256 === "string" && HASH.test(selected[sdk].sha256) &&
      proof.sdk?.[sdk]?.sha256 === selected[sdk].sha256 && proof.sdk[sdk].version === selected[sdk].version);
}

// This is only the verified service's restart affordance. The original restart
// action still authenticates its exact Restore admission and installed files.
function restoredBackgroundRestartAvailable(receipt, guide) {
  const service = receipt?.service, pin = service?.bootstrap_selection;
  const observed = receipt?.service_observation, summary = service?.public_summary;
  const directory = service?.receipt_directory;
  const prefix = typeof directory === "string" ? `${directory}/bootstrap-admission-` : "";
  return deploymentInstalledSourceKind(receipt, guide?.id) === "application" && installedRuntimeMatches(receipt) &&
    receipt.execution_mode === "background" && (receipt.target ?? "local") === "local" && receipt.status === "stopped" &&
    receipt.process_live === false && receipt.runtime_pid == null &&
    ACTION_REQUEST_UUID.test(receipt.deployment_id) && service?.label === `ai.mobkit.launch.${receipt.deployment_id}` &&
    typeof directory === "string" && directory.startsWith("/") && directory.endsWith(`/${receipt.guide_id}/${receipt.deployment_id}`) &&
    pin && !Array.isArray(pin) && Object.keys(pin).length === 2 && HASH.test(pin.sha256 || "") &&
    typeof pin.path === "string" && pin.path.startsWith(prefix) && pin.path.length > prefix.length && !pin.path.slice(prefix.length).includes("/") &&
    HASH.test(service.unit_sha256 || "") && ["launchd", "systemd-user"].includes(service.manager) &&
    summary?.definition_verified === true && summary.manager === service.manager && summary.state === observed?.state &&
    summary.pid == null && observed?.owned === true && typeof observed.loaded === "boolean" &&
    ["stopped", "inactive"].includes(observed.state) && !observed.pid && !observed.control_pid && !observed.control_group &&
    (!receipt.service_supervisor_pid || receipt.service_cleanup_confirmed === true);
}

// A staged or blocked update has changed nothing installed, so the stopped
// original still offers Start. Every other update state keeps it hidden.
function updateLeavesOriginalStartable(update) {
  return !update || ["staged", "blocked"].includes(update.status);
}

export function deploymentApplicationStartupPending(receipt, guide) {
  const completed = installedRuntimeMatches(receipt);
  return typeof guide?.id === "string" && !!guide.id && receipt?.guide_id === guide.id &&
    typeof receipt.deployment_id === "string" && !!receipt.deployment_id &&
    typeof receipt.build_id === "string" && !!receipt.build_id && HASH.test(receipt.package_sha256 || "") &&
    (receipt.source_kind == null || receipt.source_kind === "application") &&
    deploymentRequiresApplicationStartup(receipt) && (receipt.target || "local") === "local" &&
    (receipt.execution_mode || "interactive") === "interactive" && !receipt.service &&
    receipt.status === "stopped" && receipt.process_live === false &&
    receipt.runtime_pid == null && receipt.interactive_cleanup?.boundary !== "worker-not-admitted" &&
    completed && !deploymentNeedsCleanup(receipt) && updateLeavesOriginalStartable(receipt.update);
}

export function deploymentAwaitingApplicationSetup(receipt, guide) {
  return deploymentApplicationStartupPending(receipt, guide) && receipt.phase === "configure";
}

function installedSettingsMatch(options, receipt) {
  return options?.scope === SETTINGS_SCOPE && options.supported === true && options.source_verified === true && options.requires_stopped === true &&
    deploymentSourceKind(options) === deploymentSourceKind(receipt) &&
    (deploymentSourceKind(receipt) !== "native-team" || nativeSource(options) && options.source_id === receipt.source_id) &&
    options.deployment_id === receipt?.deployment_id && options.build_id === receipt?.build_id &&
    typeof options.package_sha256 === "string" && HASH.test(options.package_sha256) && options.package_sha256 === receipt?.package_sha256 &&
    typeof options.source_digest === "string" && HASH.test(options.source_digest) &&
    (receipt?.source_digest == null || options.source_digest === receipt.source_digest) &&
    Number.isSafeInteger(options.guide_version) && options.guide_version === receipt?.guide_version &&
    options.destination === receipt?.destination && options.execution_mode === (receipt?.execution_mode || "interactive");
}

export function deploymentInstalledSettingsCurrent(options, receipt, guide) {
  return installedSettingsMatch(options, receipt) && receipt?.guide_id === guide?.id &&
    Number.isSafeInteger(options.version) && options.version === guide?.version;
}

/** Settings belong to the installed source, independently of candidate acceptance. */
export function deploymentCanConfigure(receipt, build, guide, options) {
  const background = receipt?.execution_mode === "background", observed = receipt?.service_observation;
  const stopped = receipt?.status === "stopped" && receipt.process_live !== true && !receipt.runtime_pid &&
    (!background || observed?.owned === true && !observed.pid && !observed.control_pid && !observed.control_group &&
      (observed.loaded === false || ["stopped", "inactive"].includes(observed.state)) &&
      (!receipt.service_supervisor_pid || receipt.service_cleanup_confirmed === true));
  const candidateIdle = !build || ["completed", "failed", "interrupted"].includes(build.status) &&
    ["worker_stop_confirmed", "cleanup_confirmed"].every((key) => !Object.hasOwn(build, key) || build[key] === true);
  return stopped && guide?.operation?.status !== "running" && candidateIdle &&
    Number.isSafeInteger(guide?.version) && guide.version >= 0 && receipt.guide_id === guide.id &&
    typeof receipt.deployment_id === "string" && !!receipt.deployment_id &&
    (nativeSource(receipt) || deploymentSourceKind(receipt) === "application" && typeof receipt.build_id === "string" && !!receipt.build_id) &&
    typeof receipt.package_sha256 === "string" && HASH.test(receipt.package_sha256) &&
    Number.isSafeInteger(receipt.guide_version) && receipt.guide_version >= 0 &&
    typeof receipt.destination === "string" && !!receipt.destination &&
    (options === undefined || deploymentInstalledSettingsCurrent(options, receipt, guide)) &&
    !["snapshotting", "applying", "verifying", "restoring", "recovering-candidate", "recovering-current", "recovery-needed"].includes(receipt.update?.status);
}

/** Presence is guidance for one exact owned folder, never a private value. */
export function deploymentConfigurationHints(options, receipt, destination, keeping = false) {
  if (!options) return null;
  const legacyBound = options.configured_deployment_id === receipt?.deployment_id &&
    typeof options.configured_deployment_id === "string" && !!options.configured_deployment_id &&
    typeof options.configured_destination === "string" && !!options.configured_destination &&
    options.configured_destination === receipt?.destination && options.configured_destination === destination &&
    typeof options.build_id === "string" && !!options.build_id && options.build_id === receipt?.build_id &&
    Number.isSafeInteger(options.guide_version) && options.guide_version === receipt?.guide_version;
  const bound = keeping && (options.scope === SETTINGS_SCOPE
    ? installedSettingsMatch(options, receipt) && options.destination === destination : legacyBound);
  return { ...options, fields: (options.fields || []).map((field) => ({ ...field,
    configured: bound && field.configured === true })) };
}

/** Only the selected owned folder may retain the presence of private settings. */
export function deploymentConfigurationForm(options, receipt, build, guide, intent = "new") {
  if (!["new", "settings"].includes(intent)) return null;
  const keeping = intent === "settings";
  if (keeping ? !deploymentCanConfigure(receipt, build, guide, options) : !deploymentCanFreshInstall(build, guide, options)) return null;
  const executionMode = keeping ? receipt.execution_mode || "interactive" : "interactive";
  if (!keeping && !deploymentExecutionModes(options).some((mode) => mode.id === executionMode && mode.available)) return null;
  let destination = keeping ? receipt.destination : options.default_destination;
  if (typeof destination !== "string" || !destination) return null;
  if (!keeping && destination === receipt?.destination) destination = `${destination}-new`;
  return {
    intent, destination, executionMode,
    values: keeping ? {} : { MOBKIT_PORT: String(options.console_port) },
    options: deploymentConfigurationHints(options, receipt, destination, keeping),
  };
}

export function deploymentPhase(receipt) {
  if (deploymentAwaitingApplicationSetup(receipt, { id: receipt?.guide_id })) return "Your application is installed";
  const nativeTeam = deploymentSourceKind(receipt) === "native-team";
  const labels = { queued: "Preparing installation", installing: "Installing on your computer",
    checking: "Verifying the installed system", starting: nativeTeam ? "Starting your agent team" : "Starting your custom application",
    running: nativeTeam ? "Your agent team is running" : "Your custom application is running",
    stopping: nativeTeam ? "Stopping your agent team" : "Stopping your custom application",
    stopped: nativeTeam ? "Your agent team is stopped" : "Your custom application is stopped", interrupted: "Installation or runtime interrupted",
    failed: "This deployment needs attention" };
  if (receipt?.status === "running" && !deploymentConsole(receipt))
    return nativeTeam ? "Your running agent team needs a health check" : "Your running custom application needs a health check";
  return labels[receipt?.status] || "Run on your computer";
}

export function deploymentDiagnostic(receipt) {
  if (receipt?.status !== "failed") return null;
  const phases = {
    install: "The installation step failed. Review deployment activity before retrying.",
    knowledge: "Local knowledge setup or verification failed. Review that step before retrying.",
    check: "An installed host check failed. Review its result before retrying.",
    connections: "A configured connector handshake or tool grant check failed. Review that check before retrying.",
    start: "Application startup failed before native runtime and Console readiness were verified. Review startup before retrying.",
    service: "The operating-system service did not finish startup verification. Review service startup before retrying.",
    run: "The application stopped after its startup checks. Review its saved activity before restarting.",
    update: "The admitted application update did not finish. Review its recovery state before starting again.",
    recovery: "Application recovery did not finish. Review the saved recovery state before retrying.",
  };
  if (typeof receipt.phase !== "string" || !Object.hasOwn(phases, receipt.phase)) return null;
  const port = receipt.startup_port_observation;
  if (receipt.phase === "start" && typeof receipt.startup_attempt_id === "string" &&
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(receipt.startup_attempt_id) &&
      port?.scope === "local-console-port-observation" && port.attempt_id === receipt.startup_attempt_id &&
      port.deployment_id === receipt.deployment_id && typeof receipt.package_sha256 === "string" &&
      /^[a-f0-9]{64}$/.test(receipt.package_sha256) && port.package_sha256 === receipt.package_sha256 &&
      Number.isSafeInteger(port.port) && port.port >= 1024 && port.port <= 65535) {
    if (port.status === "occupied") return `Console port ${port.port} is unavailable on this computer. Change its port in private settings or check it again before starting this installation.`;
    if (port.status === "unknown") return "Port availability could not be checked before startup. Check the port in private settings before trying again.";
  }
  const value = receipt.startup_console_health;
  const bound = typeof receipt.startup_attempt_id === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(receipt.startup_attempt_id) &&
    value?.attempt_id === receipt.startup_attempt_id && value.status === "fail" &&
    Number.isSafeInteger(value.attempts) && value.attempts >= 0 && value.attempts <= 1000;
  const categories = {
    authentication: "The Console rejected startup authorization. Review its private authentication setup before retrying.",
    authorization_configuration: "Private Console health authorization could not be verified. Review the Console authentication setup.",
    unavailable: "The Console was not reachable during startup verification. Review startup activity before retrying.",
    timeout: "The Console did not finish its startup health check within the time limit. Review startup activity before retrying.",
    missing_identities: "The Console did not report all proposed native identities before startup verification ended. Review agent startup before retrying.",
    invalid_response: "The Console returned an invalid startup health response. Review startup activity before retrying.",
    probe_error: "The Console startup health check could not complete. Review startup activity before retrying.",
    child_exited: "The application stopped during Console startup verification. Review startup activity before retrying.",
    http_status: "The Console rejected its startup health request. Review startup activity before retrying.",
  };
  const httpValid = value?.http_status === undefined ||
    (["authentication", "http_status"].includes(value.category) && Number.isSafeInteger(value.http_status) &&
      value.http_status >= 100 && value.http_status <= 599);
  if (receipt.phase === "start" && bound && httpValid && typeof value.category === "string" && Object.hasOwn(categories, value.category)) {
    return value.category === "http_status" && value.http_status !== undefined
      ? `The Console returned HTTP ${value.http_status} during startup verification. Review startup activity before retrying.`
      : categories[value.category];
  }
  return phases[receipt.phase];
}

export function deploymentUpdateDiagnostics(update) {
  const historical = ["completed", "rolled-back"].includes(update?.status);
  const entries = [
    { label: historical ? "Earlier startup attempt" : "Startup result", detail: update?.activation_error },
    { label: historical ? "Earlier recovery attempt" : "Current application recovery result", detail: update?.recovery_error },
  ].filter((entry) => typeof entry.detail === "string" && entry.detail);
  return { historical, entries };
}

/** Diagnostic archives describe earlier attempts, never current runtime truth. */
export function deploymentFailedAttempts(update) {
  const result = { attempts: [], truncated: 0, unavailable: false };
  if (!update?.failed_attempts && update?.failed_attempts_truncated === undefined) return result;
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
  const validUuid = (value) => typeof value === "string" && uuid.test(value);
  const sha256 = /^[a-f0-9]{64}$/;
  const boundedText = (value, size) => typeof value === "string" && value.length <= size * 2 &&
    Array.from(value).length <= size;
  if (!validUuid(update.update_id) || !Array.isArray(update.failed_attempts) || update.failed_attempts.length > 8) {
    result.unavailable = true;
    return result;
  }
  if (update.failed_attempts_truncated !== undefined) {
    if (Number.isSafeInteger(update.failed_attempts_truncated) && update.failed_attempts_truncated >= 0)
      result.truncated = update.failed_attempts_truncated;
    else result.unavailable = true;
  }
  const seen = new Set(), encoder = new TextEncoder();
  let totalBytes = 0;
  for (const record of update.failed_attempts) {
    const previous = record?.source === "previous";
    const build = previous ? update.from_build_id : update.to_build_id;
    const packageHash = previous ? update.previous_package_sha256 : update.package_sha256;
    const sourceHash = previous ? update.previous_source_digest : update.source_digest;
    const valid = record && ["candidate", "previous"].includes(record.source) &&
      validUuid(record.attempt_id) && !seen.has(record.attempt_id) && record.update_id === update.update_id &&
      boundedText(build, 128) && !!build && record.build_id === build &&
      sha256.test(packageHash || "") && record.package_sha256 === packageHash &&
      sha256.test(sourceHash || "") && record.source_digest === sourceHash &&
      [record.service_instance_id, record.startup_attempt_id].every((value) => value == null || validUuid(value)) &&
      boundedText(record.captured_at, 64) && boundedText(record.error_type, 80) && boundedText(record.error, 1500) &&
      Array.isArray(record.logs) && record.logs.length <= 120 && typeof record.logs_truncated === "boolean" &&
      record.logs.every((entry) => entry && boundedText(entry.at, 64) && boundedText(entry.phase, 80) && boundedText(entry.text, 4000));
    if (!valid) { result.unavailable = true; continue; }
    const logs = record.logs.map(({ at, phase, text }) => ({ at, phase, text }));
    const projected = { id: record.attempt_id, source: record.source, build_id: build,
      package_sha256: packageHash, source_digest: sourceHash, captured_at: record.captured_at,
      error_type: record.error_type, error: record.error, logs, logs_truncated: record.logs_truncated };
    const bytes = encoder.encode(JSON.stringify(projected)).length;
    if (encoder.encode(JSON.stringify(logs)).length > 32 * 1024 || totalBytes + bytes > 128 * 1024) {
      result.unavailable = true;
      continue;
    }
    seen.add(record.attempt_id);
    totalBytes += bytes;
    result.attempts.push(projected);
  }
  return result;
}

export function deploymentConsole(receipt) {
  if (receipt?.status !== "running" || receipt.process_live !== true || receipt.health !== "ready" ||
      receipt.bootstrap?.complete !== true || receipt.bootstrap.ready !== true ||
      receipt.bootstrap.counts?.broken !== 0) return null;
  try {
    const url = new URL(receipt.console_url);
    return url.protocol === "http:" && url.hostname === "127.0.0.1" && url.pathname === "/console" && !url.username && !url.password
      ? url.href : null;
  } catch { return null; }
}

export function deploymentSettings(options, values) {
  const result = {};
  for (const field of options?.fields || []) {
    if (!field.automatic && typeof values[field.key] === "string" && values[field.key]) result[field.key] = values[field.key];
  }
  if (typeof values.MOBKIT_PORT === "string" && values.MOBKIT_PORT) result.MOBKIT_PORT = values.MOBKIT_PORT;
  return result;
}

export function deploymentExecutionModes(options) {
  if (!Array.isArray(options?.execution_modes)) return [{ id: "interactive", label: "While Forge is open", description: "Run under Forge supervision.", available: true }];
  return options.execution_modes.filter((item) => ["interactive", "background"].includes(item?.id))
    .map((item) => ({ ...item, available: item.available === true }));
}

export function deploymentCanSubmit(options, values, useHost, executionMode = "interactive") {
  if (!options?.supported || !options.current) return false;
  if (!deploymentExecutionModes(options).some((item) => item.id === executionMode && item.available)) return false;
  return deploymentRequiredFields(options, values, useHost).length === 0 && deploymentFieldIssues(options, values).length === 0;
}

export function deploymentReconfiguration(options, receipt, build, guide, values, removeFields = [], useHost = false) {
  if (!deploymentCanConfigure(receipt, build, guide, options) || deploymentRequiredFields(options, values, useHost).length ||
      deploymentFieldIssues(options, values, removeFields).length) return null;
  const removal = deploymentRemovalKeys(options, removeFields);
  if (!removal || removal.some((key) => typeof values[key] === "string" && !!values[key]) ||
      useHost && (!options.can_use_host_provider_credentials || removal.some((key) => options.host_provider_keys?.includes(key)))) return null;
  return { version: guide.version, deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    ...(deploymentSourceKind(receipt) === "native-team" ? { source_kind: "native-team", source_id: receipt.source_id } : {}),
    source_digest: options.source_digest, package_sha256: receipt.package_sha256,
    settings: deploymentSettings(options, values), remove_fields: removal, use_host_provider_credentials: useHost === true };
}

/** A late settings read cannot attach to another installation or candidate edit. */
export function deploymentSettingsReadKey({ state, build, receipt, sourceKind = "application" }) {
  return JSON.stringify([state?.id, state?.version, build?.build_id, build?.source_digest,
    sourceKind, receipt?.source_kind, receipt?.source_id,
    receipt?.deployment_id, receipt?.build_id, receipt?.package_sha256, receipt?.source_digest,
    receipt?.destination, receipt?.execution_mode, receipt?.guide_version]);
}

export async function readDeploymentSettings(selected, read, current) {
  if (selected.blocked || !deploymentCanConfigure(selected.receipt, selected.build, selected.state)) return null;
  const options = await read(`${deploymentUrl(selected.state.id)}/settings?${new URLSearchParams({ deployment_id: selected.receipt.deployment_id })}`);
  return deploymentSettingsReadCurrent(selected, current(), options) ? options : null;
}

export function deploymentSettingsReadCurrent(selected, current, options) {
  return !current.blocked && deploymentSettingsReadKey(selected) === deploymentSettingsReadKey(current) &&
    deploymentCanConfigure(current.receipt, current.build, current.state, options);
}

export function deploymentConfigurationRevealCurrent(intent, options, readKey, current) {
  if (current.blocked || !readKey || readKey !== deploymentSettingsReadKey(current)) return false;
  return intent === "settings" ? deploymentCanConfigure(current.receipt, current.build, current.state, options)
    : intent === "new" && deploymentOptionsCurrent(options, current.build, current.state);
}

export function deploymentSettingsResponseCurrent(selected, current, response) {
  const receipt = selected.receipt;
  return deploymentSettingsReadCurrent(selected, current, selected.options) &&
    response?.guide_id === selected.state.id && response.deployment_id === receipt.deployment_id &&
    response.build_id === receipt.build_id && response.package_sha256 === receipt.package_sha256 &&
    deploymentSourceKind(response) === deploymentSourceKind(receipt) &&
    (deploymentSourceKind(receipt) !== "native-team" || response.source_id === receipt.source_id) &&
    response.destination === receipt.destination && response.guide_version === receipt.guide_version &&
    (response.execution_mode || "interactive") === (receipt.execution_mode || "interactive") &&
    (response.source_digest == null || response.source_digest === selected.options.source_digest) &&
    deploymentCanConfigure(response, current.build, current.state);
}

export function deploymentCanStageUpdate(receipt, build, guide, options) {
  if (deploymentSourceKind(receipt) === "native-team" || deploymentSourceKind(options) === "native-team") return false;
  if (!receipt || receipt.guide_id !== guide?.id || !deploymentCanInstall(build, guide, options) ||
      receipt.build_id === build.build_id || receipt.status === "stopping" ||
      deploymentIsActive(receipt) && ["update", "recovery"].includes(receipt.phase)) return false;
  const update = receipt.update;
  return !update || ["rolled-back", "blocked"].includes(update.status) || (!new Set(["snapshotting", "applying", "verifying", "restoring", "recovering-candidate", "recovering-current", "recovery-needed"]).has(update.status) &&
    (update.to_build_id !== build.build_id || update.guide_version !== build.guide_version));
}

/** Only the host's exact owned service projection enables background updates. */
export function deploymentBackgroundUpdate(receipt, guide) {
  const capability = receipt?.update_capabilities, service = receipt?.service;
  const sha256 = /^[a-f0-9]{64}$/;
  const supported = receipt?.guide_id === guide?.id && receipt.execution_mode === "background" &&
    typeof receipt.deployment_id === "string" && !!receipt.deployment_id &&
    typeof receipt.build_id === "string" && !!receipt.build_id && sha256.test(receipt.package_sha256 || "") &&
    capability?.background_activation_supported === true && capability.requires_stopped === true &&
    capability.deployment_id === receipt.deployment_id && capability.build_id === receipt.build_id &&
    capability.package_sha256 === receipt.package_sha256 &&
    ["launchd", "systemd-user"].includes(capability.manager) && capability.manager === service?.manager &&
    sha256.test(capability.unit_sha256 || "") && capability.unit_sha256 === service?.unit_sha256 &&
    sha256.test(capability.host_manifest_sha256 || "") && service?.public_summary?.definition_verified === true;
  const observation = receipt?.service_observation;
  const stopped = supported && receipt.status === "stopped" && receipt.process_live !== true && !receipt.runtime_pid &&
    observation?.owned === true && !observation.pid && !observation.control_pid && !observation.control_group &&
    (observation.loaded === false || ["stopped", "inactive"].includes(observation.state)) &&
    (!receipt.service_supervisor_pid || receipt.service_cleanup_confirmed === true);
  return { supported: !!supported, stopped: !!stopped };
}

/** The comparison is useful even when this lifecycle cannot activate it. */
export function deploymentUpdatePreparation(receipt, build, guide, options) {
  const available = deploymentCanStageUpdate(receipt, build, guide, options);
  const background = receipt?.guide_id === guide?.id && typeof receipt?.deployment_id === "string" &&
    !!receipt.deployment_id && receipt.execution_mode === "background";
  const activationSupported = deploymentBackgroundUpdate(receipt, guide).supported;
  const recheck = available && receipt.update?.status === "blocked" &&
    receipt.update.to_build_id === build.build_id && receipt.update.guide_version === build.guide_version;
  return { available, background, activationSupported,
    label: recheck ? "Recheck accepted update" : background && !activationSupported ? "Preview update compatibility" : "Stage accepted update",
    limitation: background && (available || receipt.update)
      ? activationSupported
        ? "Staging checks compatibility without changing the service. Stop the application and wait for confirmed cleanup before applying an update or restoring its previous source. Each change must pass fresh native startup and Console health checks."
        : "This background service can only preview update compatibility. Its update adapter could not be verified, so the preview cannot apply or roll back its source. Your service and its recorded activity stay unchanged."
      : null };
}

/** After setup, Apply installs and checks the update; only the setup owner's Start runs it. */
export function deploymentUpdateOwnerStart(update) {
  return typeof update?.setup_restore_sha256 === "string" && /^[0-9a-f]{64}$/.test(update.setup_restore_sha256);
}

export function deploymentUpdateAction(receipt, build, guide, action) {
  if (deploymentSourceKind(receipt) === "native-team") return null;
  const update = receipt?.update;
  if (!update?.update_id || receipt.guide_id !== guide?.id || deploymentIsActive(receipt)) return null;
  const mode = receipt.execution_mode || "interactive";
  if (mode === "background" ? !deploymentBackgroundUpdate(receipt, guide).stopped : mode !== "interactive") return null;
  if (["apply-update", "recover-current"].includes(action)) {
    const eligible = action === "apply-update" ? ["staged", "snapshotting"] : ["recovery-needed", "recovering-current"];
    if (!deploymentCanPrepare(build, guide) || !eligible.includes(update.status) ||
        update.blockers?.length || update.to_build_id !== build.build_id || update.guide_version !== build.guide_version ||
        typeof build.source_digest !== "string" || update.source_digest !== build.source_digest) return null;
  } else if (action === "rollback") {
    if (update.rollback_available !== true || !["completed", "applying", "verifying", "restoring", "recovering-candidate", "recovering-current", "candidate-restored-stopped", "recovery-needed"].includes(update.status)) return null;
  } else return null;
  return { deployment_id: receipt.deployment_id, action, update_id: update.update_id };
}

export function deploymentExperienceKey(receipt, guideId) {
  if (deploymentSourceKind(receipt) === "native-team") return "";
  if (!receipt || receipt.guide_id !== guideId || !receipt.deployment_id || !receipt.build_id || !receipt.package_sha256) return "";
  // Receipt polling replaces objects; only source/runtime transitions refresh
  // the more expensive installed-source metadata check.
  return JSON.stringify([guideId, receipt.deployment_id, receipt.build_id, receipt.package_sha256,
    receipt.status, receipt.console_url || null]);
}

function experiencePath(value) {
  if (typeof value !== "string" || value.length > 1000 || !value.startsWith("/") || value.startsWith("//")) return false;
  let decoded = value;
  for (let depth = 0; depth < 4; depth++) {
    if (decoded.startsWith("//") || /[\\?#\x00-\x20\x7f]/.test(decoded) ||
        decoded.split("/").some((part) => part === "." || part === "..")) return false;
    let next;
    try { next = decodeURIComponent(decoded); } catch { return false; }
    if (next === decoded) return true;
    decoded = next;
  }
  return false;
}

export function deploymentExperiences(value, receipt, guideId) {
  const unavailable = { entries: [], issue: "Declared application UI links could not be validated for this installed package. Installation and native Console status are unchanged." };
  if (!value) return { entries: [], issue: null };
  if (!deploymentExperienceKey(receipt, guideId) || value.deployment_id !== receipt.deployment_id ||
      value.build_id !== receipt.build_id || value.package_sha256 !== receipt.package_sha256 ||
      value.scope !== "declared-source" || value.verified !== false || !Array.isArray(value.entries) ||
      value.entries.length > 12 || value.error) return unavailable;
  if (!value.entries.length) return { entries: [], issue: null };
  const digest = /^[a-f0-9]{64}$/;
  if (!digest.test(value.source_digest || "") || !digest.test(value.metadata_sha256 || "") ||
      receipt.source_digest && receipt.source_digest !== value.source_digest) return unavailable;
  const ids = new Set(), entries = [];
  for (const entry of value.entries) {
    if (!entry || !/^[a-z][a-z0-9_-]{0,63}$/.test(entry.id || "") || ids.has(entry.id) ||
        typeof entry.label !== "string" || !entry.label.trim() || entry.label.length > 160 ||
        /[\x00-\x1f\x7f]/.test(entry.label) || !experiencePath(entry.path) ||
        entry.status !== "declared" || entry.verified !== false ||
        entry.port !== undefined && (!Number.isInteger(entry.port) || entry.port < 1024 || entry.port > 65535)) return unavailable;
    ids.add(entry.id);
    let url = null;
    if (entry.url !== null) {
      if (typeof entry.url !== "string" || entry.url.length > 1300 || /[\x00-\x20\x7f]/.test(entry.url)) return unavailable;
      try {
        const parsed = new URL(entry.url);
        const port = Number(parsed.port);
        if (parsed.protocol !== "http:" || parsed.hostname !== "127.0.0.1" || !Number.isInteger(port) ||
            port < 1024 || port > 65535 || parsed.username || parsed.password || parsed.search || parsed.hash ||
            parsed.pathname !== new URL(entry.path, parsed.origin).pathname || entry.port !== undefined && entry.port !== port) return unavailable;
        url = deploymentConsole(receipt) ? parsed.href : null;
      } catch { return unavailable; }
    }
    entries.push({ id: entry.id, label: entry.label.trim(), path: entry.path, url, status: "declared", verified: false,
      reason: typeof entry.reason === "string" && entry.reason.length <= 500 ? entry.reason : null });
  }
  return { entries, issue: null };
}
