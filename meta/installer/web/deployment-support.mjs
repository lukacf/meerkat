// Support command identity is browser intent, never installed/runtime truth.
// JSX owns explicit clicks, ordinary-command exclusion and guide/poll fences.
const PREFIX = "meta-forge-deployment-support-v1:";
const UUID = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const HASH = /^[0-9a-f]{64}$/;
const ACTIONS = new Set(["prepare-support", "apply-support"]);
const PHASES = new Set(["prepared", "retaining", "retained", "installing", "installed", "publishing", "completed"]);
const FIELDS = ["version", "guide_id", "deployment_id", "action_request_id", "action", "update_id",
  "expected_service_digest", "revision", "descriptor_sha256"];
const plain = (value) => !!value && typeof value === "object" && !Array.isArray(value) &&
  [Object.prototype, null].includes(Object.getPrototypeOf(value));
const text = (value) => typeof value === "string" && value.length > 0 && value.length <= 200;
const hash = (value) => typeof value === "string" && HASH.test(value);
const uuid = (value) => typeof value === "string" && UUID.test(value);
const sameIntent = (left, right) => FIELDS.every((key) => left[key] === right[key]);
const guideId = (guide) => typeof guide === "string" ? guide : guide?.id;

export function deploymentSupportIntentValid(intent, selectedGuide) {
  return plain(intent) && Object.keys(intent).length === FIELDS.length &&
    FIELDS.every((key) => Object.hasOwn(intent, key)) && intent.version === 1 &&
    text(selectedGuide) && intent.guide_id === selectedGuide && text(intent.deployment_id) &&
    uuid(intent.action_request_id) && ACTIONS.has(intent.action) && intent.update_id === null &&
    (intent.action === "prepare-support"
      ? hash(intent.expected_service_digest) && intent.revision === null && intent.descriptor_sha256 === null
      : intent.expected_service_digest === null && uuid(intent.revision) && hash(intent.descriptor_sha256));
}

export function deploymentSupportIntent(selectedGuide, body, requestId) {
  if (!plain(body) || Object.keys(body).some((key) => ![
    "deployment_id", "action", "action_request_id", "expected_service_digest", "revision", "descriptor_sha256",
  ].includes(key)) || body.action_request_id !== undefined && body.action_request_id !== requestId)
    throw new Error("This installation support command could not be identified. No command was sent.");
  const intent = { version: 1, guide_id: selectedGuide, deployment_id: body.deployment_id,
    action_request_id: requestId, action: body.action, update_id: null,
    expected_service_digest: body.expected_service_digest ?? null, revision: body.revision ?? null,
    descriptor_sha256: body.descriptor_sha256 ?? null };
  if (!deploymentSupportIntentValid(intent, selectedGuide))
    throw new Error("This installation support command could not be identified. No command was sent.");
  return intent;
}

export function loadDeploymentSupportIntent(storage, selectedGuide) {
  if (!text(selectedGuide)) throw new Error("Choose this setup before checking its support command.");
  let raw;
  try { raw = storage.getItem(PREFIX + selectedGuide); }
  catch { throw new Error("The saved installation support command could not be checked. No command was sent."); }
  if (raw === null) return null;
  let intent;
  try { if (typeof raw !== "string" || raw.length > 4096) throw new Error(); intent = JSON.parse(raw); }
  catch { throw new Error("The saved installation support command is unreadable. Preserve it before continuing."); }
  if (!deploymentSupportIntentValid(intent, selectedGuide))
    throw new Error("The saved installation support command does not identify this setup. Preserve it before continuing.");
  return intent;
}

export function storeDeploymentSupportIntent(storage, intent) {
  if (!deploymentSupportIntentValid(intent, intent?.guide_id))
    throw new Error("This installation support command is invalid. No command was sent.");
  const key = PREFIX + intent.guide_id, encoded = JSON.stringify(intent);
  try {
    if (storage.getItem(key) !== null) throw new Error();
    storage.setItem(key, encoded);
    if (storage.getItem(key) !== encoded) throw new Error();
  } catch { throw new Error("The installation support command could not be saved. Check it before sending a command."); }
}

export function clearDeploymentSupportIntent(storage, intent) {
  if (!deploymentSupportIntentValid(intent, intent?.guide_id))
    throw new Error("The installation support command is invalid. Nothing was cleared.");
  const retained = loadDeploymentSupportIntent(storage, intent.guide_id);
  if (!retained || !sameIntent(retained, intent))
    throw new Error("The saved installation support command changed. Check it before continuing.");
  try {
    storage.removeItem(PREFIX + intent.guide_id);
    if (storage.getItem(PREFIX + intent.guide_id) !== null) throw new Error();
  } catch { throw new Error("The resolved installation support command could not be cleared. Check it before continuing."); }
}

export function deploymentSupportObservationUrl(intent) {
  if (!deploymentSupportIntentValid(intent, intent?.guide_id)) throw new Error("The support command identity is invalid.");
  return `/api/guide/${encodeURIComponent(intent.guide_id)}/deployment/action/${encodeURIComponent(intent.deployment_id)}/${intent.action_request_id}`;
}

export function deploymentSupportRequestMatches(value, intent) {
  const observed = value?.action_request;
  return deploymentSupportIntentValid(intent, intent?.guide_id) && plain(value) &&
    plain(observed) &&
    FIELDS.every((key) => Object.hasOwn(observed, key)) && sameIntent(observed, intent) &&
    (observed.phase === "pending" && observed.outcome == null ||
      observed.phase === "settled" && ["returned", "error"].includes(observed.outcome));
}

export function deploymentSupportRequestSettled(value, intent) {
  return deploymentSupportRequestMatches(value, intent) && value.action_request.phase === "settled";
}

function selectedValid(selected) {
  return plain(selected) && uuid(selected.revision) && hash(selected.descriptor_sha256) &&
    PHASES.has(selected.phase) && hash(selected.before_service_digest) && hash(selected.after_service_digest) &&
    [selected.retained_files, selected.staged_files].every((count) => Number.isSafeInteger(count) && count > 0 && count <= 256);
}

/** Backend projection is checked against the selected installed source. */
export function deploymentSupportRequestValid(value, receipt, guide) {
  return plain(value) && plain(receipt) && text(guideId(guide)) && receipt.guide_id === guideId(guide) &&
    text(receipt.deployment_id) && text(receipt.build_id) && hash(receipt.package_sha256) &&
    value.scope === "owned-stopped-supervisor-refresh" && value.requires_stopped === true &&
    value.deployment_id === receipt.deployment_id && value.build_id === receipt.build_id &&
    value.package_sha256 === receipt.package_sha256 &&
    [value.stopped_verified, value.available, value.prepare_available, value.apply_available]
      .every((flag) => typeof flag === "boolean") &&
    value.available === (value.prepare_available || value.apply_available) &&
    (!value.available || value.stopped_verified === true) &&
    (value.service_digest === null || hash(value.service_digest)) &&
    (value.reason == null || typeof value.reason === "string") &&
    (value.selected === null
      ? ["unavailable", "available", "current"].includes(value.phase) && value.apply_available === false
      : selectedValid(value.selected) && value.phase === value.selected.phase &&
        (value.selected.phase !== "completed" || value.apply_available === false)) &&
    (!(value.prepare_available || value.apply_available) || hash(value.service_digest)) &&
    !(value.prepare_available && value.apply_available) &&
    (!value.prepare_available || value.selected === null || value.selected.phase === "completed") &&
    (value.selected !== null || (value.phase === "available" ? value.prepare_available : value.available === false)) &&
    (!value.apply_available || value.selected !== null && (value.service_digest === value.selected.before_service_digest ||
      value.selected.phase === "publishing" && value.service_digest === value.selected.after_service_digest)) &&
    (!value.stopped_verified || value.selected?.phase !== "completed" ||
      value.service_digest === value.selected.after_service_digest);
}

function scopeIssue(receipt, guide) {
  if (!plain(receipt) || receipt.guide_id !== guideId(guide)) return "Check the selected installation before continuing.";
  if (![undefined, null, "application"].includes(receipt.source_kind) || (receipt.target ?? "local") !== "local" ||
      receipt.execution_mode !== "background" || !plain(receipt.service))
    return "Installation support is available for an application running locally as a background service.";
  if (receipt.update != null && receipt.update.status !== "completed")
    return "Resolve the selected application update before preparing installation support.";
  if (receipt.status !== "stopped" || receipt.service_cleanup_confirmed !== true ||
      receipt.process_live !== false || receipt.runtime_pid !== null)
    return "Stop this installed application and wait for its cleanup before continuing.";
  return null;
}

function intentBound(intent, receipt, request) {
  if (intent.guide_id !== receipt.guide_id || intent.deployment_id !== receipt.deployment_id) return false;
  if (intent.action === "prepare-support") {
    const selected = request.selected;
    if (selected?.revision === intent.action_request_id && selected.before_service_digest === intent.expected_service_digest)
      return request.service_digest === selected.before_service_digest ||
        ["publishing", "completed"].includes(selected.phase) && request.service_digest === selected.after_service_digest;
    return request.prepare_available && request.service_digest === intent.expected_service_digest;
  }
  return request.selected?.revision === intent.revision && request.selected.descriptor_sha256 === intent.descriptor_sha256 &&
    [request.selected.before_service_digest, request.selected.after_service_digest].includes(request.service_digest);
}

const cards = {
  unknown: ["Support status unknown", "Check the current installation support status before continuing."],
  eligible: ["Installation support is ready to prepare", "Prepare the support update for this stopped installation."],
  prepared: ["Installation support is prepared", "Apply the prepared support update to this stopped installation."],
  "in-progress": ["Installation support needs continuation", "Continue the exact saved support command explicitly."],
  completed: ["Installation support update completed", "The selected support update is complete. The installed application remains stopped."],
  refused: ["Installation support is unavailable", "This installation cannot change support in its current state."],
};

export function deploymentSupportState(receipt, guide, { confirmed = false, storageAvailable = true,
  intent = null, observation = null } = {}) {
  const card = (state, detail = cards[state][1], action = null, continuation = false, selected = null) => ({
    state, label: cards[state][0], detail, action, continuation, selected,
  });
  if (confirmed !== true) return card("unknown");
  if (storageAvailable !== true) return card("unknown", "This browser could not retain the support command. Check storage before continuing.");
  const issue = scopeIssue(receipt, guide);
  if (issue) return card("refused", issue);
  const request = receipt.support_request;
  if (!deploymentSupportRequestValid(request, receipt, guide)) return card("unknown");
  if (typeof request.reason === "string" && request.reason.length > 0)
    return card("refused", request.reason, null, false, request.selected);
  if (request.stopped_verified !== true)
    return card("unknown", "The stopped installation support owner could not be verified. Check its current status before continuing.");
  if (intent !== null) {
    // The exact command GET proves admission and settlement only. The separately
    // checked receipt above owns stopped-source and selected-custody proof.
    if (!deploymentSupportIntentValid(intent, guideId(guide)) || !deploymentSupportRequestMatches(observation, intent))
      return card("unknown", "Checking the saved support command. It will not be sent again automatically.");
    if (!intentBound(intent, receipt, request)) return card("refused", "The installation support binding changed. Check the saved command before continuing.");
    if (observation.action_request.phase === "pending")
      return card("in-progress", undefined, intent.action, true, request.selected);
    return card("unknown", "The support command is resolved. Check the current installation support status before another command.");
  }
  if (request.prepare_available) return card("eligible", undefined, "prepare-support", false, request.selected);
  if (request.apply_available) return card(request.phase === "prepared" ? "prepared" : "in-progress",
    undefined, "apply-support", false, request.selected);
  if (request.phase === "current") return { ...card("completed", "The installed support source is current. The application remains stopped."),
    label: "Installation support is current" };
  if (request.phase === "completed") return card("completed", undefined, null, false, request.selected);
  return card("refused", request.reason || cards.refused[1], null, false, request.selected);
}

export function deploymentSupportAction(receipt, guide, action, options = {}) {
  if (!ACTIONS.has(action)) return null;
  if (options.intent != null) return options.resume === true && options.intent.action === action
    ? deploymentSupportBodyFromIntent(options.intent, { ...options, receipt, guide }) : null;
  const state = deploymentSupportState(receipt, guide, options);
  if (state.action !== action || state.continuation) return null;
  const request = receipt.support_request;
  return action === "prepare-support"
    ? { deployment_id: receipt.deployment_id, action, expected_service_digest: request.service_digest }
    : { deployment_id: receipt.deployment_id, action, revision: request.selected.revision,
      descriptor_sha256: request.selected.descriptor_sha256 };
}

/** No default replay: initial dispatch or exact pending continuation is explicit. */
export function deploymentSupportBodyFromIntent(intent, { initial = false, resume = false, receipt, guide,
  confirmed = false, storageAvailable = true, observation = null } = {}) {
  if (!deploymentSupportIntentValid(intent, guideId(guide)) || confirmed !== true || storageAvailable !== true) return null;
  if (resume === true) {
    if (initial !== false) return null;
    const state = deploymentSupportState(receipt, guide, { confirmed, storageAvailable, intent, observation });
    if (!state.continuation || state.action !== intent.action) return null;
  } else {
    if (resume !== false || initial !== true) return null;
    const body = deploymentSupportAction(receipt, guide, intent.action, { confirmed, storageAvailable });
    if (!body || !sameIntent(deploymentSupportIntent(guideId(guide), body, intent.action_request_id), intent)) return null;
  }
  const body = { deployment_id: intent.deployment_id, action: intent.action, action_request_id: intent.action_request_id };
  if (intent.action === "prepare-support") body.expected_service_digest = intent.expected_service_digest;
  else { body.revision = intent.revision; body.descriptor_sha256 = intent.descriptor_sha256; }
  return resume ? { ...body, resume: true } : body;
}
