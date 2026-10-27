import { applicable, draftFromFields, fieldText, FormError, normalizeHelp, stringListValue, takeSession } from "./form.js";

// Bound browser observation, not host execution. This includes the JSON body
// and leaves room above the host's 120-second MCPB inspection allowance.
const RESPONSE_DEADLINE_MS = 180000;
// Status and job acknowledgements should return promptly. Admission can still
// outlast this wait; expiry neither settles nor repeats the host operation.
const SHORT_RESPONSE_PATHS = new Set(["/api/state", "/api/jobs", "/api/job", "/api/stop",
  "/api/prepare", "/api/install", "/api/retry_prepare", "/api/retry_export", "/api/selected_install"]);
const FAMILY_LABELS = { "official-playwright": "Official Playwright (isolated Chrome)", "official-github": "Official GitHub (fixed read-only route)", agno: "Agno library", activepieces: "Activepieces library", services: "Slack and Gmail service routes" };
const EFFECT_LABELS = { read: "Read", write: "Write", unspecified: "Effect not specified" };
const FIELD_ERRORS = { field_required: "Complete this required field.", integer_required: "Enter a whole number.",
  string_list_required: "Enter a JSON array of at most 256 strings, each at most 2048 characters without control characters.",
  boolean_required: "Choose Yes or No.", reference_name_required: "Enter an environment variable name, not a credential value.",
  field_out_of_bounds: "Use a value within the displayed bounds.", invalid_field: "Check this field." };
const record = value => value !== null && typeof value === "object" && !Array.isArray(value);
const nonempty = value => typeof value === "string" && value.length > 0;
const SELECTED_FAMILIES = { "selected-python": "python", "selected-node": "node", "selected-mcpb": "mcpb" };
const SELECTED_LABELS = { python: "Python", node: "Node", mcpb: "MCPB (Node)" };
const selectedKind = job => typeof job?.family === "string" && Object.hasOwn(SELECTED_FAMILIES, job.family) ? SELECTED_FAMILIES[job.family] : null;
function selectedDigestKey(value, kind) {
  if (!record(value)) return null;
  if (Object.hasOwn(value, "document_kind")) {
    return ["python", "node"].includes(kind) && value.document_kind === "local_software_declaration" &&
      !Object.hasOwn(value, "manifest_digest") && sha256(value.declaration_digest) ? "declaration_digest" : null;
  }
  return !Object.hasOwn(value, "declaration_digest") && sha256(value.manifest_digest) ? "manifest_digest" : null;
}
function validSelectedResult(value, job, scope) {
  const digestKey = selectedDigestKey(value, selectedKind(job));
  const configurationKeys = selectedKind(job) === "mcpb" ? ["configuration_sha256"] : [];
  return digestKey !== null && value.scope === scope && value.selection_kind === selectedKind(job) &&
    typeof value.component === "string" && /^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$/.test(value.component) &&
    ["selection_sha256", "effect_sha256", "receipt_sha256", ...configurationKeys].every(key =>
      typeof value[key] === "string" && /^[0-9a-f]{64}$/.test(value[key])) &&
    !["family", "profile", "credential_environment_references"].some(key => Object.hasOwn(value, key)) &&
    (job.status !== "succeeded" || selectedDigestKey(job.result, selectedKind(job)) === digestKey &&
      ["component", "selection_sha256", digestKey, "effect_sha256", ...configurationKeys].every(key => value[key] === job.result?.[key])) &&
    record(value.mcp_configuration) && nonempty(value.mcp_path) && nonempty(value.receipt_path);
}
const identityKey = value => JSON.stringify([value.publisher, value.name, value.version]);
const validIdentity = value => record(value) && Object.keys(value).sort().join(",") === "name,publisher,version" &&
  Object.values(value).every(item => nonempty(item) && item.length <= 256);
const recoverySelection = state => state?.mode === "recovery" && state.job_history?.recovery_selection === true;
const hasHistory = state => state?.job_history?.available === true && (state.mode !== "recovery" || recoverySelection(state));
const recoveredJob = job => record(job) && ["install", "export", "acquire"].includes(job.kind) && job.recovered === true && job.status === "unconfirmed" &&
  job.error === "historical_outcome_unknown" && !Object.hasOwn(job, "result") && validRecoveryRequest(job);
const sameRecovery = (job, prior) => job.recovered === prior.recovered &&
  job.recovery_request?.id === prior.recovery_request?.id && job.recovery_request?.sha256 === prior.recovery_request?.sha256;
const isRecipeExport = job => job?.kind === "export" && job.family === "remote-recipe";
const sha256 = value => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const COMPOSE_ACTIONS = { prepare: "Prepare container plan", start: "Start deployments", resume: "Resume deployments", stop: "Stop deployments", remove: "Remove deployment resources",
  prepare_successor: "Prepare successor plan", adopt_successor: "Adopt successor plan", reconcile_successor: "Reconcile successor transition", rebind: "Rebind host to selected plan" };
const COMPOSE_SUCCESSOR_ACTIONS = new Set(["prepare_successor", "adopt_successor", "reconcile_successor", "rebind"]);
const COMPOSE_RUNTIME_ERRORS = new Set([
  "invalid_json", "invalid_retained_path", "invalid_retained_file", "changed_retained_file", "retained_file_unavailable",
  "invalid_tool_binding", "tool_binding_changed", "invalid_bind_source_prefix", "command_output_limit", "workspace_unavailable",
  "operation_in_progress", "request_limit", "planning_failed", "invalid_planner_reply", "workspace_exists", "invalid_planner_files",
  "plan_changed", "inputs_changed", "record_changed", "docker_config_changed", "engine_observation_unavailable",
  "engine_observation_invalid", "engine_changed", "image_changed", "image_volume_unbound", "resource_mismatch", "resource_replaced",
  "foreign_network_member", "foreign_volume_user", "operation_unconfirmed", "project_not_absent", "resources_incomplete",
  "resources_not_stopped", "stale_observation", "deployment_not_started", "request_in_progress", "resources_disappeared",
  "invalid_state_disposition", "effect_unconfirmed", "compose_unavailable", "compose_interrupted",
  "custody_changed", "successor_unconfirmed", "successor_planning_failed", "invalid_successor", "successor_unavailable",
  "successor_limit", "successor_resources_unconfirmed", "successor_already_retained", "successor_state_changed",
  "no_pending_successor", "successor_effect_unresolved",
]);
const isCompose = job => job?.kind === "compose" && job.family === "compose";
const requestId = value => typeof value === "string" && /^[0-9a-f]{32}$/.test(value);
const exactKeys = (value, keys) => record(value) && Object.keys(value).sort().join(",") === [...keys].sort().join(",");
const ACQUISITIONS = {
  "selected-mcpb": { key: "mcpb_acquisition", scope: "trusted_public_mcpb_acquisition", label: "MCPB bundle acquisition", result: "retained_mcpb_artifact_bytes", artifact: "artifact.mcpb" },
  "selected-python": { key: "python_acquisition", scope: "trusted_pypi_wheel_acquisition", label: "Python wheel acquisition", result: "retained_python_wheel_inputs", packageKey: "distribution", artifact: "artifact.whl" },
  "selected-node": { key: "node_acquisition", scope: "trusted_npm_registry_acquisition", label: "Node package acquisition", result: "retained_node_package_inputs", packageKey: "package", artifact: "artifact.tgz" },
  "selected-compose": { key: "compose_acquisition", scope: "acquire_agent_selected_container_images", label: "Compose image acquisition", result: "compose_plan_for_host_review" },
};
const acquisition = job => job?.kind === "acquire" && Object.hasOwn(ACQUISITIONS, job.family) ? ACQUISITIONS[job.family] : null;
const boundedText = value => nonempty(value) && value.length <= 4096 && !/[\x00-\x1f\x7f]/.test(value);
const absolutePath = value => nonempty(value) && value.startsWith("/");
function validAcquisitionState(value, contract) {
  return record(value) && value.available === true && value.scope === contract.scope &&
    absolutePath(value.staging_root) && sha256(value.host_profile_sha256) &&
    (value.workspace === undefined || absolutePath(value.workspace));
}
function acquisitionJobBinding(job, state) {
  const contract = acquisition(job), summary = contract && state[contract.key];
  return Boolean(contract) && validAcquisitionState(summary, contract) &&
    (state.mode !== "recovery" || job.recovered === true && validRecoveryRequest(job)) &&
    (contract.artifact || !validRecoveryRequest(job) || absolutePath(summary.workspace)) &&
    (!job.result || contract.artifact || summary.workspace === undefined ||
      job.result.proposal_directory === `${summary.workspace.replace(/\/$/, "")}/acquisitions/${job.id}/proposal`) &&
    (!job.result || !contract.artifact || job.result.host_profile_sha256 === summary.host_profile_sha256 &&
      job.result.receipt_path === `${summary.staging_root.replace(/\/$/, "")}/${job.id}/acquisition.json`);
}
function validRecoveryRequest(job) {
  return exactKeys(job.recovery_request, ["id", "sha256"]) &&
    job.recovery_request.id === job.id && sha256(job.recovery_request.sha256);
}
function acquisitionBindingKey(job, state) {
  const summary = state[acquisition(job).key];
  return JSON.stringify([job.family, job.request_id, job.recovery_request?.id, job.recovery_request?.sha256,
    summary.scope, summary.host_profile_sha256, summary.staging_root, summary.workspace]);
}
// Envelope checks only. The acquisition owner validates transport/origin policy;
// these URLs never become navigation targets or displayed receipt fields.
function publicAcquisitionUrl(value, originOnly = false) {
  if (typeof value !== "string" || !value || value.length > (originOnly ? 320 : 4096) ||
      /[^\x21-\x7e]|[?\x23\\]/.test(value)) return false;
  if (!value.startsWith("https://")) return false;
  const authority = value.slice(8).split("/", 1)[0];
  return Boolean(authority) && !authority.includes("@") &&
    (!originOnly || value.slice(8) === authority);
}
function validMcpbAcquisitionResult(value, job) {
  return value.schema_version === 1 && value.staged_id === job.id && sha256(value.host_profile_sha256) &&
    absolutePath(value.receipt_path) && sha256(value.report_sha256) &&
    exactKeys(value.request, ["request_id", "url", "artifact_sha256"]) &&
    value.request.request_id === job.request_id && publicAcquisitionUrl(value.request.url) && sha256(value.request.artifact_sha256) &&
    exactKeys(value.primary, ["url", "sha256", "byte_length", "final_origin", "redirect_count"]) &&
    value.primary.url === value.request.url && value.primary.sha256 === value.request.artifact_sha256 &&
    Number.isSafeInteger(value.primary.byte_length) && value.primary.byte_length >= 0 && value.primary.byte_length <= 64 * 1024 * 1024 &&
    Number.isSafeInteger(value.primary.redirect_count) && value.primary.redirect_count >= 0 && value.primary.redirect_count <= 5 &&
    publicAcquisitionUrl(value.primary.final_origin, true) &&
    exactKeys(value.files, ["artifact.mcpb"]) && value.files["artifact.mcpb"] === value.primary.sha256;
}
function validAcquisitionResult(value, job) {
  const contract = acquisition(job);
  if (!contract || !record(value) || value.scope !== contract.result || !sha256(value.receipt_sha256)) return false;
  if (contract.artifact === "artifact.mcpb") return validMcpbAcquisitionResult(value, job);
  if (!contract.artifact) {
    const proposal = value.proposal;
    return absolutePath(value.proposal_directory) && value.proposal_directory.endsWith(`/${job.id}/proposal`) &&
      exactKeys(proposal, ["schema_version", "status", "scope", "plan_sha256", "files"]) &&
      proposal.schema_version === 1 && proposal.status === "proposed" && proposal.scope === contract.result && sha256(proposal.plan_sha256) &&
      exactKeys(proposal.files, ["host-profile.json", "selection.json", "backend-profile.json", "inputs.json", "request.json", "prepared.json", "image-acquisition.json"]) &&
      Object.values(proposal.files).every(sha256);
  }
  // Project only receipt identity and file hashes. Provider URLs, runtime details
  // and any future fields remain with the host's canonical acquisition receipt.
  return value.schema_version === 1 && value.staged_id === job.id && sha256(value.host_profile_sha256) &&
    absolutePath(value.receipt_path) && exactKeys(value.request, ["request_id", contract.packageKey, "version"]) &&
    value.request.request_id === job.request_id && boundedText(value.request[contract.packageKey]) && boundedText(value.request.version) &&
    record(value.primary) && value.primary.name === value.request[contract.packageKey] && value.primary.version === value.request.version &&
    sha256(value.primary.sha256) && Array.isArray(value.artifacts) && value.artifacts.length > 0 && value.artifacts.length <= 256 &&
    record(value.files) && Object.keys(value.files).length <= 512 && value.files[contract.artifact] === value.primary.sha256 &&
    Object.entries(value.files).every(([name, digest]) => boundedText(name) && !name.includes("\\") &&
      (name === contract.artifact || name.startsWith("closure/")) &&
      name.split("/").every(part => part && part !== "." && part !== "..") && sha256(digest));
}
function validComposeSummary(value) {
  const digests = ["request_sha256", "package_sha256", "manifest_sha256", "manifest_source_sha256", "profile_sha256", "configuration_sha256", "policy_sha256"];
  const orders = ["initialization_order", "shutdown_order", "deployment_order", "request_components"];
  const text = value => nonempty(value) && value.length <= 1024 && !/[\x00-\x1f\x7f]/.test(value);
  const ids = value => Array.isArray(value) && value.length <= 16 && value.every(text) && new Set(value).size === value.length;
  if (!exactKeys(value, [...digests, ...orders, "host_owner", "project", "components", "files"]) ||
      !digests.every(key => sha256(value[key])) || !text(value.host_owner) || !text(value.project) ||
      !orders.every(key => ids(value[key])) || !record(value.components) || Object.keys(value.components).length > 16 ||
      !record(value.files) || Object.keys(value.files).length > 128) return false;
  return Object.entries(value.components).every(([name, component]) => text(name) &&
    exactKeys(component, ["service", "lifetime", "image", "image_id", "platform", "dependencies", "deployment_dependencies", "readiness", "stop_timeout_ms"]) &&
    ["service", "image", "image_id", "platform"].every(key => text(component[key])) &&
    ["deployment", "request"].includes(component.lifetime) && ["running", "healthy"].includes(component.readiness) &&
    ids(component.dependencies) && ids(component.deployment_dependencies) &&
    Number.isSafeInteger(component.stop_timeout_ms) && component.stop_timeout_ms > 0) &&
    Object.entries(value.files).every(([path, file]) => text(path) && exactKeys(file, ["sha256", "mode"]) &&
      sha256(file.sha256) && Number.isSafeInteger(file.mode) && file.mode >= 0 && file.mode <= 0o777);
}
function validComposeState(value) {
  if (!(record(value) && value.available === true && value.scope === "execute_agent_selected_local_container_plan" &&
    ["new", "reopen"].includes(value.mode) && Array.isArray(value.actions) &&
    value.actions.every(action => typeof action === "string" && (Object.hasOwn(COMPOSE_ACTIONS, action) || action === "observe")) &&
    new Set(value.actions).size === value.actions.length &&
    [value.graph_workspace, value.staging_root].every(path => nonempty(path) && path.startsWith("/")) &&
    sha256(value.host_profile_sha256) && sha256(value.backend_profile_sha256) &&
    (value.execution_uncertain === undefined || typeof value.execution_uncertain === "boolean") &&
    (value.plan_sha256 === null || sha256(value.plan_sha256)))) return false;
  const target = value.successor, pending = value.pending_transition;
  if (target !== undefined && (!exactKeys(target, ["inputs_sha256", "backend_profile_sha256", "predecessor_plan_sha256", "transition_sha256", "successor_plan_sha256"]) ||
      ![target.inputs_sha256, target.backend_profile_sha256, target.predecessor_plan_sha256].every(sha256) || value.mode !== "reopen" ||
      ![target.transition_sha256, target.successor_plan_sha256].every(item => item === null || sha256(item)) ||
      target.successor_plan_sha256 !== null && target.transition_sha256 === null ||
      ![target.predecessor_plan_sha256, target.successor_plan_sha256].includes(value.plan_sha256) || value.plan_sha256 === null ||
      value.plan_sha256 === target.successor_plan_sha256 && value.backend_profile_sha256 !== target.backend_profile_sha256)) return false;
  if (value.actions.some(action => COMPOSE_SUCCESSOR_ACTIONS.has(action)) && !target) return false;
  const selectionKeys = ["selected_plan_sha256", "pending_operation", "pending_transition", "rebind_required"];
  if (target || selectionKeys.some(key => Object.hasOwn(value, key))) {
    if (!selectionKeys.every(key => Object.hasOwn(value, key)) ||
        !(value.selected_plan_sha256 === null || sha256(value.selected_plan_sha256)) ||
        !(value.pending_operation === null || typeof value.pending_operation === "boolean") ||
        typeof value.rebind_required !== "boolean" ||
        value.rebind_required !== (value.selected_plan_sha256 !== null && value.selected_plan_sha256 !== value.plan_sha256) ||
        value.selected_plan_sha256 === null && (value.pending_operation !== null || pending !== null) ||
        value.selected_plan_sha256 !== null && (typeof value.pending_operation !== "boolean" ||
          ![value.plan_sha256, target?.predecessor_plan_sha256, target?.successor_plan_sha256].includes(value.selected_plan_sha256))) return false;
    if (pending !== null && (!target || !exactKeys(pending, ["transition_sha256", "phase", "predecessor_plan_sha256", "successor_plan_sha256"]) ||
        !sha256(pending.transition_sha256) || !sha256(pending.successor_plan_sha256) ||
        pending.transition_sha256 !== target.transition_sha256 || pending.predecessor_plan_sha256 !== target.predecessor_plan_sha256 ||
        pending.successor_plan_sha256 !== target.successor_plan_sha256 || !["retire", "migrate", "activate"].includes(pending.phase) ||
        value.selected_plan_sha256 !== pending[pending.phase === "activate" ? "successor_plan_sha256" : "predecessor_plan_sha256"])) return false;
  }
  return true;
}
const validPreparedCompose = value => exactKeys(value, ["schema_version", "status", "plan_sha256", "summary"]) &&
  value.schema_version === 1 && value.status === "prepared" && sha256(value.plan_sha256) && validComposeSummary(value.summary);
const composeResultPlan = backend => backend.status === "prepared_successor" ? backend.predecessor.plan_sha256 :
  backend.status === "rebound" ? backend.selected.plan_sha256 : backend.plan_sha256;
function validComposeResult(value, job) {
  if (!exactKeys(value, ["scope", "action", "backend", "runtime_bindings"]) || value.scope !== "compose_backend_observation_only" || value.action !== job.action ||
      !record(value.backend) ||
      !Array.isArray(value.runtime_bindings) || value.runtime_bindings.length > 16) return false;
  const backend = value.backend;
  if (job.action === "prepare") {
    if (!validPreparedCompose(backend)) return false;
  } else if (job.action === "prepare_successor") {
    if (!exactKeys(backend, ["schema_version", "status", "transition_sha256", "predecessor", "successor"]) ||
        ![1, 2].includes(backend.schema_version) || backend.status !== "prepared_successor" || !sha256(backend.transition_sha256) ||
        !validPreparedCompose(backend.predecessor) || !validPreparedCompose(backend.successor)) return false;
  } else if (job.action === "rebind") {
    if (!exactKeys(backend, ["status", "transition_sha256", "selected"]) || backend.status !== "rebound" ||
        !sha256(backend.transition_sha256) || !validPreparedCompose(backend.selected)) return false;
  } else if (!exactKeys(backend, ["status", "plan_sha256", "observation", ...(
        COMPOSE_SUCCESSOR_ACTIONS.has(job.action) ? ["transition_sha256"] : [])]) ||
      !sha256(backend.plan_sha256) || COMPOSE_SUCCESSOR_ACTIONS.has(job.action) && !sha256(backend.transition_sha256) ||
      !["observed", "unconfirmed"].includes(backend.status) ||
      backend.status === "observed" && backend.observation === null ||
      !(backend.observation === null || record(backend.observation) &&
        backend.observation.plan_sha256 === backend.plan_sha256 && sha256(backend.observation.observation_sha256))) return false;
  if (job.status === "succeeded" ? !["prepared", "prepared_successor", "rebound", "observed"].includes(backend.status) :
      job.status !== "unconfirmed" || backend.status !== "unconfirmed") return false;
  if (backend.status === "unconfirmed" && value.runtime_bindings.length) return false;
  const components = new Set();
  return value.runtime_bindings.every(binding => {
    if (!exactKeys(binding, ["scope", "component", "plan_sha256", "command", "args", "env", "runtime_sha256", "planner_sha256"]) ||
        binding.scope !== "compose_backend_stdio" || !nonempty(binding.component) ||
        components.has(binding.component) || binding.plan_sha256 !== composeResultPlan(backend) ||
        !nonempty(binding.command) || !binding.command.startsWith("/") || !Array.isArray(binding.args) ||
        !binding.args.every(arg => typeof arg === "string") || !record(binding.env) || Object.keys(binding.env).length ||
        !sha256(binding.runtime_sha256) || !sha256(binding.planner_sha256)) return false;
    components.add(binding.component); return true;
  });
}
function composeJobBinding(job, state) {
  if (!validComposeState(state.compose)) return false;
  const host = state.compose, target = host.successor, backend = job.result?.backend;
  if (COMPOSE_SUCCESSOR_ACTIONS.has(job.action) && !target) return false;
  if (!backend) return true;
  if (COMPOSE_SUCCESSOR_ACTIONS.has(job.action) && backend.transition_sha256 !== target.transition_sha256) return false;
  if (job.action === "prepare_successor") return backend.predecessor.plan_sha256 === target.predecessor_plan_sha256 &&
    backend.successor.plan_sha256 === target.successor_plan_sha256 &&
    backend.successor.summary.profile_sha256 === target.backend_profile_sha256 &&
    (host.plan_sha256 !== target.predecessor_plan_sha256 || backend.predecessor.summary.profile_sha256 === host.backend_profile_sha256);
  if (job.action === "rebind") return backend.selected.plan_sha256 === target.successor_plan_sha256 &&
    backend.selected.summary.profile_sha256 === target.backend_profile_sha256;
  // A retained predecessor result stays about A after an explicit B rebind.
  // The exact host-declared predecessor/target is the only additional identity.
  return [host.plan_sha256, target?.predecessor_plan_sha256, target?.successor_plan_sha256].includes(backend.plan_sha256) &&
    (job.action !== "prepare" || backend.summary.profile_sha256 === host.backend_profile_sha256);
}
function validComposeIdentity(job) {
  return isCompose(job) && requestId(job.id) && typeof job.action === "string" && Object.hasOwn(COMPOSE_ACTIONS, job.action) &&
    requestId(job.request_id) && job.stop_supported === false && job.stop === undefined && job.retry_of === undefined;
}
function composeHostStatus(host, job) {
  if (!host) return "";
  if (host.execution_uncertain) return "Runtime execution is uncertain. The host must establish quiescent custody before opening a new session; no automatic retry is performed.";
  if (isCompose(job) && job.status === "running") return `${COMPOSE_ACTIONS[job.action]} is running in the local host. Refresh status to observe it; no automatic retry is performed.`;
  if (host.pending_transition) return `Successor transition is pending: ${host.pending_transition.phase === "retire" ? "retiring the predecessor" : host.pending_transition.phase === "migrate" ? "waiting for confirmed state migration" : "activating the successor"}. Use the authorized setup tools to observe and explicitly reconcile this transition. No current runtime bindings are available.`;
  if (host.pending_operation) return "A container operation is pending. Use the existing setup tools to observe and explicitly resume it as permitted by the host. No current runtime bindings are available.";
  if (host.rebind_required) return "The successor is selected, but this host is still bound to the predecessor. Use the authorized setup tools for an explicit rebind, or open a compatible successor host. No current runtime bindings are available.";
  if (host.successor) return host.plan_sha256 === host.successor.successor_plan_sha256 ?
    "This host is explicitly bound to the selected successor. This does not establish application readiness." : host.successor.successor_plan_sha256 ?
    "The successor plan is prepared. Adoption requires an explicit request through the authorized setup tools with a fresh observation." :
    "Successor inputs are pinned. Use the authorized setup tools to prepare the successor plan; preparation does not start containers.";
  return "";
}
function validRecipeExport(value) {
  return record(value) && value.scope === "portable_mcp_configuration_only" &&
    record(value.recipe) && validIdentity(value.recipe.identity) && sha256(value.recipe.digest) &&
    sha256(value.recipe.package_content_digest) && sha256(value.selection_digest) &&
    nonempty(value.destination) && value.destination.startsWith("/") &&
    value.mcp_path === `${value.destination}/mcp.json` && value.plugin_path === `${value.destination}/plugin.json` &&
    record(value.mcp_configuration) && record(value.mcp_configuration.mcpServers) &&
    Object.keys(value.mcp_configuration.mcpServers).length > 0 &&
    record(value.files) && Object.keys(value.files).sort().join(",") === "mcp.json,plugin.json" &&
    Object.values(value.files).every(file => record(file) && sha256(file.sha256) &&
      Number.isSafeInteger(file.bytes) && file.bytes > 0);
}
function validReferenceHandoff(value, expected = {}) {
  if (!record(value)) return false;
  for (const key of ["family", "profile", "configuration_sha256", "contents_sha256"]) {
    if (value[key] !== undefined && expected[key] !== undefined && value[key] !== expected[key]) return false;
  }
  if (!Object.hasOwn(value, "credential_environment_references")) return true;
  if (!Object.hasOwn(FAMILY_LABELS, value.family) || typeof value.profile !== "string" ||
      !/^[A-Za-z0-9_.-]{1,128}$/.test(value.profile) || typeof value.configuration_sha256 !== "string" ||
      !/^[0-9a-f]{64}$/.test(value.configuration_sha256) || typeof value.contents_sha256 !== "string" ||
      !/^[0-9a-f]{64}$/.test(value.contents_sha256) ||
      !Array.isArray(value.credential_environment_references) || value.credential_environment_references.length > 256) return false;
  const paths = new Set(), name = item => typeof item === "string" && /^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(item);
  return value.credential_environment_references.every(row => {
    if (!record(row) || Object.keys(row).sort().join(",") !== "environment_variable,field_path" ||
        !Array.isArray(row.field_path) || !row.field_path.every(name) || !name(row.environment_variable) ||
        (value.family === "activepieces" ? row.field_path.length !== 1 :
          row.field_path.length !== 2 || row.field_path[0] !== "config")) return false;
    const path = JSON.stringify(row.field_path);
    if (paths.has(path)) return false;
    paths.add(path); return true;
  });
}
function validJob(job) {
  if (job?.kind === "compose" || job?.family === "compose") return validComposeIdentity(job) &&
    ["running", "succeeded", "unconfirmed"].includes(job.status) &&
    (job.runtime_error === undefined || job.status === "unconfirmed" && job.error === "compose_operation_unconfirmed" &&
      job.result === undefined && COMPOSE_RUNTIME_ERRORS.has(job.runtime_error)) &&
    (job.result === undefined ? job.status !== "succeeded" : validComposeResult(job.result, job));
  if (job?.retry_of !== undefined && (job.kind !== "prepare" && !isRecipeExport(job) && !selectedKind(job) ||
      typeof job.retry_of !== "string" || !/^[0-9a-f]{32}$/.test(job.retry_of) || job.retry_of === job.id)) return false;
  if (job?.stop !== undefined && (!record(job.stop) || job.stop.scope !== "local_job_only" ||
      !["requested", "settled"].includes(job.stop.status) ||
      (job.stop.status === "requested" ? job.status !== "running" : job.status !== "unconfirmed"))) return false;
  if (job?.kind === "acquire") return Boolean(acquisition(job)) && requestId(job.id) && requestId(job.request_id) &&
    job.retry_of === undefined && (job.recovery_request === undefined || validRecoveryRequest(job)) &&
    (job.recovered === undefined || job.recovered === true && job.status === "unconfirmed" && validRecoveryRequest(job)) &&
    ["running", "succeeded", "unconfirmed"].includes(job.status) &&
    (job.status === "succeeded" ? validAcquisitionResult(job.result, job) : job.result === undefined);
  if (job?.kind === "export" || job?.family === "remote-recipe") return isRecipeExport(job) &&
    typeof job.id === "string" && /^[0-9a-f]{32}$/.test(job.id) && sha256(job.request_sha256) &&
    ["running", "succeeded", "unconfirmed"].includes(job.status) &&
    (job.status !== "succeeded" || validRecipeExport(job.result));
  if (selectedKind(job)) return record(job) && typeof job.id === "string" && /^[0-9a-f]{32}$/.test(job.id) && job.kind === "install" &&
    ["running", "succeeded", "unconfirmed"].includes(job.status) &&
    (job.status !== "succeeded" || validSelectedResult(job.result, job, "observed_software_installation_only"));
  return record(job) && nonempty(job.id) && nonempty(job.family) &&
    ["prepare", "install"].includes(job.kind) && ["running", "succeeded", "unconfirmed"].includes(job.status) &&
    !(job.kind === "install" && job.status === "succeeded" &&
      (!record(job.result) || !record(job.result.mcp_configuration) || !nonempty(job.result.mcp_path) ||
       !nonempty(job.result.receipt_path) || !nonempty(job.result.scope) ||
       !validReferenceHandoff(job.result, { family: job.family })));
}
function validState(value) {
  if (!record(value) || (value.mode !== undefined && !["setup", "recovery"].includes(value.mode)) || !nonempty(value.scope) || !Array.isArray(value.families) ||
      !(value.job === null || validJob(value.job))) return false;
  if (value.operation_in_progress !== undefined && typeof value.operation_in_progress !== "boolean") return false;
  if (value.recovery_manifest !== undefined && (!exactKeys(value.recovery_manifest, ["available", "max_requests"]) ||
      value.recovery_manifest.available !== true || value.recovery_manifest.max_requests !== 256)) return false;
  if (value.stop !== undefined && (!record(value.stop) || typeof value.stop.available !== "boolean")) return false;
  if (value.preparation_retry !== undefined && (!record(value.preparation_retry) || typeof value.preparation_retry.available !== "boolean")) return false;
  if (value.export_retry !== undefined && (!record(value.export_retry) || typeof value.export_retry.available !== "boolean")) return false;
  if (value.selected_retry !== undefined && (!record(value.selected_retry) || typeof value.selected_retry.available !== "boolean")) return false;
  if (value.compose !== undefined && !validComposeState(value.compose)) return false;
  if (Object.values(ACQUISITIONS).some(contract => value[contract.key] !== undefined &&
      !validAcquisitionState(value[contract.key], contract))) return false;
  if (value.job_history !== undefined && (!record(value.job_history) || value.job_history.available !== true ||
      !Number.isSafeInteger(value.job_history.count) || value.job_history.count < 0)) return false;
  if (value.job_history?.recovery_selection !== undefined &&
      (!recoverySelection(value) || value.job_history.count < 1 || value.job_history.count > 256 || !recoveredJob(value.job))) return false;
  if (value.reconciliation !== undefined && (!record(value.reconciliation) ||
      typeof value.reconciliation.available !== "boolean" ||
      !(value.reconciliation.observation === null || validObservation(value.reconciliation.observation, value.job)))) return false;
  if (acquisition(value.job) && (value.reconciliation?.available && !validRecoveryRequest(value.job) ||
      value.reconciliation?.observation?.status === "matched" &&
      !acquisitionJobBinding({ ...value.job, result: value.reconciliation.observation.result }, value))) return false;
  const ids = new Set();
  return value.families.every(family => {
    if (!record(family) || !nonempty(family.id) || ids.has(family.id) ||
        !["unprepared", "preparing", "prepared", "unconfirmed"].includes(family.status) || !Array.isArray(family.profiles)) return false;
    if (family.preparation_job_id !== undefined && (typeof family.preparation_job_id !== "string" ||
        !/^[0-9a-f]{32}$/.test(family.preparation_job_id))) return false;
    ids.add(family.id);
    const profiles = new Set();
    return family.profiles.every(profile => {
      if (!record(profile) || !nonempty(profile.id) || profiles.has(profile.id) ||
          !Array.isArray(profile.operations) || !profile.operations.length) return false;
      profiles.add(profile.id);
      const operations = new Set();
      return profile.operations.every(operation => {
        if (!record(operation) || !nonempty(operation.id) || operations.has(operation.id) || !Object.hasOwn(EFFECT_LABELS, operation.effect)) return false;
        operations.add(operation.id); return true;
      });
    });
  }) && (value.job === null || (acquisition(value.job) ? acquisitionJobBinding(value.job, value) : isCompose(value.job) ? composeJobBinding(value.job, value) : isRecipeExport(value.job) ? (value.mode === "recovery" ?
    value.job.status === "unconfirmed" && value.job.recovered === true && sha256(value.job.recovery_request?.sha256) :
    value.local_recipes?.export_available === true) : selectedKind(value.job) ? value.mode === "recovery" ||
    record(value.selected_installation) && value.selected_installation.scope === "standing_local_software_installation_only" &&
    Array.isArray(value.selected_installation.profiles) && value.selected_installation.profiles.includes(selectedKind(value.job)) :
    ids.has(value.job.family)));
}

function validObservation(value, job) {
  if (acquisition(job)) return record(value) && value.job_id === job.id &&
    Number.isSafeInteger(value.revision) && value.revision > 0 &&
    ["checking", "matched", "unconfirmed"].includes(value.status) && value.scope === "retained_acquisition_files_only" &&
    Array.isArray(value.excluded) && value.excluded.every(item => typeof item === "string") &&
    (value.status === "matched" ? validAcquisitionResult(value.result, job) &&
      (!job.result || value.result.receipt_sha256 === job.result.receipt_sha256) : value.result === undefined);
  if (isRecipeExport(job)) return record(value) && value.job_id === job.id &&
    Number.isSafeInteger(value.revision) && value.revision > 0 &&
    ["checking", "matched", "unconfirmed"].includes(value.status) && value.scope === "current_portable_export_files_only" &&
    Array.isArray(value.excluded) && value.excluded.every(item => typeof item === "string") &&
    (value.status !== "matched" || value.result?.scope === value.scope &&
      value.result?.export_check?.state === "matches" && validRecipeExport(value.result.configuration) &&
      value.result.selection_digest === value.result.configuration.selection_digest &&
      value.result.destination === value.result.configuration.destination &&
      validIdentity(value.result.recipe?.identity) &&
      identityKey(value.result.recipe.identity) === identityKey(value.result.configuration.recipe.identity) &&
      value.result.recipe.digest === value.result.configuration.recipe.digest &&
      value.result.recipe.package_content_digest === value.result.configuration.recipe.package_content_digest &&
      (job.status !== "succeeded" || value.result.selection_digest === job.result.selection_digest &&
        value.result.destination === job.result.destination));
  return nonempty(job?.id) && record(value) && value.job_id === job.id && Number.isSafeInteger(value.revision) && value.revision > 0 &&
    ["checking", "matched", "unconfirmed"].includes(value.status) && value.scope === "retained_payload_and_launch_binding_only" &&
    Array.isArray(value.excluded) && value.excluded.every(item => typeof item === "string") &&
    (value.status !== "matched" || (selectedKind(job) ? validSelectedResult(value.result, job, value.scope) :
      record(value.result) && record(value.result.mcp_configuration) &&
      nonempty(value.result.mcp_path) && nonempty(value.result.receipt_path) && nonempty(value.result.family) &&
      nonempty(value.result.profile) && value.result.family === job.family &&
      /^[0-9a-f]{64}$/.test(value.result.receipt_sha256)));
}

function validJobPage(value, state, before) {
  if (!record(value) || !Array.isArray(value.jobs) || value.jobs.length > 50 ||
      !(value.next_before === null || value.jobs.length > 0 && value.next_before === value.jobs.at(-1)?.id)) return false;
  const ids = new Set();
  return value.jobs.every(job => {
    if (!record(job) || !nonempty(job.id) || job.id === before || ids.has(job.id) ||
        !["prepare", "install", "export", "compose", "acquire"].includes(job.kind) || !["running", "succeeded", "unconfirmed"].includes(job.status) ||
        !(job.profile === null || typeof job.profile === "string" && /^[A-Za-z0-9_.-]{1,128}$/.test(job.profile)) ||
        !(job.kind === "acquire" ? requestId(job.id) && acquisitionJobBinding(job, state) && job.profile === null :
          job.kind === "compose" || job.family === "compose" ? validComposeIdentity(job) && composeJobBinding(job, state) && job.profile === null :
          job.kind === "export" || job.family === "remote-recipe" ? isRecipeExport(job) &&
          (state.mode === "recovery" || state.local_recipes?.export_available === true) && job.profile === null :
          state.families.some(family => family.id === job.family) || selectedKind(job) &&
          (recoverySelection(state) || state.mode === "recovery" && state.job?.family === job.family || state.selected_installation?.profiles?.includes(selectedKind(job))))) return false;
    if (recoverySelection(state) && (!["install", "export", "acquire"].includes(job.kind) || job.recovered !== true || job.status !== "unconfirmed" || !validRecoveryRequest(job))) return false;
    ids.add(job.id); return true;
  });
}

function validJobDetail(value, summary) {
  return record(value) && validJob(value.job) &&
    ["id", "family", "kind"].every(key => value.job[key] === summary[key]) &&
    (!summary.recovered || recoveredJob(value.job) && sameRecovery(value.job, summary)) &&
    (!isCompose(value.job) || ["action", "request_id", "stop_supported"].every(key => value.job[key] === summary[key])) &&
    (!value.job.result?.profile || value.job.result.profile === summary.profile) &&
    record(value.reconciliation) && typeof value.reconciliation.available === "boolean" &&
    (!acquisition(value.job) || !value.reconciliation.available || validRecoveryRequest(value.job)) &&
    (value.reconciliation.observation === null || validObservation(value.reconciliation.observation, value.job) &&
      (value.reconciliation.observation.status !== "matched" ||
        (!summary.profile || value.reconciliation.observation.result.profile === summary.profile) &&
        ["profile", "configuration_sha256", "contents_sha256"].every(key => value.job.result?.[key] === undefined ||
          value.reconciliation.observation.result[key] === value.job.result[key])));
}

class RequestError extends Error {
  constructor(code, httpStatus = 0) { super(code); this.code = code; this.httpStatus = httpStatus; }
}

function recoveryAnchor(job) {
  return ["install", "export", "acquire"].includes(job?.kind) &&
    typeof job.id === "string" && job.id.length === 32 && requestId(job.id) && validRecoveryRequest(job) &&
    job.recovery_request.sha256.length === 64 ? { id: job.id, sha256: job.recovery_request.sha256 } : null;
}

async function recoveryFile(value, anchors, crypto) {
  if (!exactKeys(value, ["scope", "filename", "media_type", "text", "bytes", "sha256"]) ||
      value.scope !== "retained_setup_requests_only" || value.filename !== "toolkit-recovery.json" ||
      value.media_type !== "application/json" || typeof value.text !== "string" || value.text.length > 65536 ||
      !Number.isSafeInteger(value.bytes) || value.bytes < 1 || value.bytes > 65536 ||
      !sha256(value.sha256) || value.sha256.length !== 64) throw new RequestError("invalid_recovery_file");
  const parsed = JSON.parse(value.text);
  if (!exactKeys(parsed, ["schema_version", "workspace", "requests"]) || parsed.schema_version !== 1 ||
      !absolutePath(parsed.workspace) || parsed.workspace.includes("\0") || !Array.isArray(parsed.requests) ||
      parsed.requests.length !== anchors.length || parsed.requests.some((entry, index) =>
        !exactKeys(entry, ["id", "sha256"]) || entry.id !== anchors[index].id || entry.sha256 !== anchors[index].sha256)) {
    throw new RequestError("invalid_recovery_binding");
  }
  // Match the owner's sorted, ASCII JSON encoding. This also rejects duplicate
  // keys or alternate payloads hidden behind JSON.parse's last-key behavior.
  const canonical = JSON.stringify({ requests: anchors, schema_version: 1, workspace: parsed.workspace })
    .replace(/[\u007f-\uffff]/g, character => `\\u${character.charCodeAt(0).toString(16).padStart(4, "0")}`) + "\n";
  if (value.text !== canonical) throw new RequestError("invalid_recovery_encoding");
  const bytes = new TextEncoder().encode(value.text);
  if (bytes.byteLength !== value.bytes) throw new RequestError("invalid_recovery_length");
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  const actual = [...hash].map(byte => byte.toString(16).padStart(2, "0")).join("");
  if (actual !== value.sha256) throw new RequestError("invalid_recovery_digest");
  return bytes;
}

// Presentation only: the installed validator chooses the cause. Never infer it
// from form values or display arbitrary text from a failed subprocess.
const CONFIGURATION_ISSUES = {
  invalid_document: [[], "Check the configuration reference and submit the required configuration fields."],
  invalid_config_file: [[], "The configuration file could not be read. Validate again or reopen setup."],
  invalid_profile: [["profile"], "Choose a supported service profile."],
  invalid_operations: [["operations"], "Choose supported operations without duplicates."],
  invalid_config_fields: [["config"], "Check the required fields and remove unsupported fields."],
  invalid_channel_ids: [["config.allowed_channel_ids"], "Use at most 256 unique channel IDs containing only letters and digits, each 1-128 characters."],
  channel_list_conflict: [["operations"], "Channel restrictions cannot be combined with list_conversations_page. Keep the restriction and deselect that operation, then validate again."],
  invalid_recipient_addresses: [["config.allowed_recipient_addresses"], "Use at most 256 unique bare mailbox addresses, without display names or wildcards."],
  recipient_operation_conflict: [["operations"], "Recipient restrictions cannot be combined with send_raw or create_draft_raw. Keep the restriction and choose structured send_message for sending."],
  invalid_credential_reference: [["config.token_env"], "Enter an environment variable name, not a credential value."],
  integer_out_of_range: [["config.timeout_seconds", "config.max_response_bytes", "config.max_message_bytes", "config.max_interval_requests", "config.max_interval_messages", "config.max_interval_result_bytes"], "Use a whole number within the bounds in the configuration reference."],
  invalid_mailbox: [["config.user_id"], "Use me or a supported bare ASCII mailbox address."],
};
function configurationIssue(value, current, contents) {
  if (!exactKeys(value, ["error", "diagnostic"]) || value.error !== "invalid_configuration") return null;
  const d = value.diagnostic;
  if (!exactKeys(d, ["version", "phase", "code", "field_path", "message", "family", "profile", "configuration_sha256", "contents_sha256"]) ||
      d.version !== 1 || d.phase !== "configuration_validation" || current.family !== "services" ||
      d.family !== current.family || d.profile !== current.profile || !["slack", "gmail"].includes(d.profile) ||
      !sha256(d.configuration_sha256) || !sha256(contents) || d.contents_sha256 !== contents ||
      typeof d.message !== "string" || typeof d.code !== "string" || !Object.hasOwn(CONFIGURATION_ISSUES, d.code) ||
      !Array.isArray(d.field_path) || !d.field_path.every(part => typeof part === "string" && /^[a-z_]+$/.test(part))) return null;
  const [paths, message] = CONFIGURATION_ISSUES[d.code], path = d.field_path.join(".");
  if (!(paths.length ? paths.includes(path) : d.field_path.length === 0)) return null;
  if (d.profile !== "slack" && (["invalid_channel_ids", "channel_list_conflict"].includes(d.code) || path.startsWith("config.max_interval_"))) return null;
  if (d.profile !== "gmail" && (["invalid_recipient_addresses", "recipient_operation_conflict", "invalid_mailbox"].includes(d.code) || path === "config.max_message_bytes")) return null;
  return { path: d.field_path, message };
}

function validSelectedRetry(value, job) {
  const digestKey = selectedDigestKey(value, selectedKind(job));
  const selectionFile = digestKey === "declaration_digest" ? "declaration.json" : "activation.json";
  const identifier = value => typeof value === "string" && /^[0-9a-f]{32}$/.test(value);
  const bounded = (value, max) => typeof value === "string" && value.length <= max;
  const fileName = value => bounded(value, 1024) && value.split("/").every(part => part && part !== "." && part !== ".." && !part.includes("\\") && !/[\x00-\x1f\x7f]/.test(part));
  return digestKey !== null && value.scope === "selected_software_inputs_only" && value.retry_of === job.id &&
    value.profile === selectedKind(job) && identifier(value.validation_id) && sha256(value.effect_sha256) &&
    (value.job_id === null || identifier(value.job_id) && value.job_id !== job.id) &&
    validIdentity(value.package) && bounded(value.component, 128) && /^[A-Za-z0-9][A-Za-z0-9_.-]*$/.test(value.component) &&
    sha256(value.selection_sha256) &&
    (value.profile !== "mcpb" || sha256(value.configuration_sha256)) &&
    bounded(value.destination, 8192) && value.destination.startsWith("/") &&
    value.destination.endsWith("/installations/" + value.validation_id) &&
    record(value.entrypoint) && bounded(value.entrypoint.executable, 8192) &&
    Array.isArray(value.entrypoint.arguments) && value.entrypoint.arguments.length <= 256 &&
    value.entrypoint.arguments.every(arg => bounded(arg, 8192)) && JSON.stringify(value.entrypoint.arguments).length <= 262144 &&
    bounded(value.mcp_name, 256) && value.mcp_name.length > 0 &&
    record(value.files) && Object.keys(value.files).length >= 4 && Object.keys(value.files).length <= 259 &&
    Object.entries(value.files).every(([name, hash]) => fileName(name) && sha256(hash)) &&
    value.files[selectionFile] === value.selection_sha256 && sha256(value.files["package.json"]) &&
    (value.profile === "mcpb" ? Object.keys(value.files).length === 4 &&
      value.files["configuration.json"] === value.configuration_sha256 && sha256(value.files["artifact.mcpb"]) :
      sha256(value.files[value.profile === "python" ? "artifact.whl" : "artifact.tgz"]) &&
      Object.keys(value.files).every(name => ["package.json", selectionFile, value.profile === "python" ? "artifact.whl" : "artifact.tgz"].includes(name) ||
        /^closure\/[^/]+$/.test(name)));
}

export function createWorkbench(document, options = {}) {
  const window = document.defaultView;
  const fetcher = options.fetch ?? window.fetch.bind(window);
  const later = options.setTimeout ?? window.setTimeout.bind(window);
  const clear = options.clearTimeout ?? window.clearTimeout.bind(window);
  const crypto = options.crypto ?? window.crypto;
  let session = options.session ?? takeSession(window.location, window.history);
  const byId = id => document.getElementById(id);
  const headerDescription = document.querySelector(".page-header > p");
  const setupDescription = headerDescription.textContent;
  const elements = Object.fromEntries(["family", "profile", "prepare", "refresh", "configuration", "configuration-form",
    "operations", "fields", "validate", "install", "reconcile", "stop-job", "retry-prepare", "retry-export", "retry-selected", "install-selected-retry", "selected-retry-review", "review", "problem", "job-panel", "result"].map(id => [id, byId(id)]));
  let state = null, help = null, validation = null, pending = null, available = false, disposed = false;
  let helpFailure = null;
  let revision = 0, helpGeneration = 0, refreshGeneration = 0, timer = null, lastJob = null;
  let catalog = null, catalogLoading = false, catalogGeneration = 0, entryGeneration = 0;
  let entryRequest = null, entryRetry = null;
  const inputs = new Map(), removers = [];
  const responseWaits = new Set();
  const observations = new Map();
  const acquisitionBindings = new Map();
  const composeResultBindings = new Map();
  const stopRequests = new Map();
  const preparationRetries = new Map();
  const exportRetries = new Map();
  const selectedRetries = new Map();
  let routeSelection = null, routeGeneration = 0;
  let priorSelection = null, priorReview = null;
  let selectedJobId = null, jobView = null, historyCount = null, historyBefore = null, historyLoading = false;
  let historyRows = new Map();
  const recoveryEntries = new Map(), recoveryConflicts = new Set();
  let recoveryRequest = null, recoveryURL = null, recoveryListKey = null;

  function on(node, event, callback) {
    node.addEventListener(event, callback);
    removers.push(() => node.removeEventListener(event, callback));
  }
  function node(tag, text, className) {
    const result = document.createElement(tag);
    if (text !== undefined) result.textContent = text;
    if (className) result.className = className;
    return result;
  }
  function selectedFamily() { return state?.families.find(item => item.id === elements.family.value); }
  function running() { return state?.operation_in_progress === true || state?.job?.status === "running" || state?.reconciliation?.observation?.status === "checking"; }
  function viewedJob() { return selectedJobId ? jobView?.job : state?.job; }
  function viewedReconciliation() { return selectedJobId ? jobView?.reconciliation : state?.reconciliation; }
  function focus(id) { if (!disposed) byId(id)?.focus(); }
  function problem(message) { elements.problem.textContent = message; elements.problem.hidden = !message; }
  function describeFailure(error, fallback) {
    if (error instanceof RequestError && error.httpStatus === 403) return "The setup session is unavailable. Reopen the host's private launcher.";
    if (error instanceof RequestError && error.httpStatus === 409) return "The host could not accept this step in its current state. Refresh status before continuing.";
    return fallback;
  }
  async function post(path, payload) {
    if (!session) throw new RequestError("session_unavailable", 403);
    let response, value;
    const wait = {};
    const deadline = new Promise((_, reject) => {
      wait.expire = () => reject(new RequestError("response_unconfirmed"));
      wait.timer = later(wait.expire, SHORT_RESPONSE_PATHS.has(path) ? 30000 : RESPONSE_DEADLINE_MS);
    });
    responseWaits.add(wait);
    try {
      const observation = (async () => {
        const response = await fetcher(path, { method: "POST", headers: { "Content-Type": "application/json", "X-Toolkit-Session": session },
          body: JSON.stringify(payload), credentials: "omit", mode: "same-origin", redirect: "error", cache: "no-store" });
        return { response, value: await response.json() };
      })();
      // Losing this wait never aborts host work or retries an effect. The race
      // also consumes a late rejection; only its first result reaches callers.
      ({ response, value } = await Promise.race([observation, deadline]));
    } catch { throw new RequestError("response_unconfirmed"); }
    finally { clear(wait.timer); responseWaits.delete(wait); }
    if (!response.ok) {
      const error = new RequestError("host_refused", response.status);
      if (path === "/api/check" && response.status === 400) error.configurationResponse = value;
      throw error;
    }
    return value;
  }
  function catalogBinding(value) {
    return record(value) && value.scope === "catalog_metadata_only" &&
      /^[0-9a-f]{64}$/.test(value.catalog_sha256) && /^[0-9a-f]{64}$/.test(value.contents_sha256) &&
      value.catalog_sha256 === state?.catalog?.catalog_sha256 && value.contents_sha256 === state?.catalog?.contents_sha256;
  }
  function clearCatalogEntry(keepRetry = false) {
    entryGeneration += 1; entryRequest = null; entryRetry = null;
    byId("catalog-detail").hidden = true;
    byId("catalog-entry-retry").hidden = !keepRetry;
    byId("catalog-entry-retry").setAttribute("aria-disabled", "true");
  }
  function currentCatalogEntry(request) {
    return !disposed && available && state?.mode !== "recovery" && request.generation === entryGeneration &&
      request.catalog === catalog && catalogBinding(catalog) && request.key === byId("catalog-choice").value;
  }
  function filterCatalog() {
    if (!catalog) return;
    const query = byId("catalog-filter").value.toLowerCase(), choice = byId("catalog-choice"), prior = choice.value;
    const rows = catalog.entries.filter(row => [row.display_name, row.description, ...Object.values(row.identity)]
      .some(text => text.toLowerCase().includes(query)));
    choice.replaceChildren(...rows.map(row => new window.Option(`${row.display_name} - ${row.identity.version} (${row.identity.publisher}/${row.identity.name})`, identityKey(row.identity))));
    choice.value = rows.some(row => identityKey(row.identity) === prior) ? prior : "";
    if (choice.value !== prior) { cancelRoute(); clearCatalogEntry(); }
    byId("catalog-status").textContent = `${rows.length} of ${catalog.entries.length} catalog entries. Select one to read its setup options.`;
  }
  async function loadCatalog() {
    if (state?.mode === "recovery" || !state?.catalog?.available) {
      if (state?.mode === "recovery" || routeSelection) cancelRoute();
      catalogGeneration += 1; clearCatalogEntry(); catalog = null; catalogLoading = false;
      byId("catalog-controls").hidden = true; byId("catalog-detail").hidden = true;
      byId("catalog-status").textContent = state?.mode === "recovery" ? "Catalog browsing is unavailable in this readback-only recovery session." :
        "This host bundle does not expose a catalog. Available setup families remain below.";
      return;
    }
    if (catalog && catalogBinding(catalog) || catalogLoading) return;
    const generation = ++catalogGeneration; catalogLoading = true; catalog = null;
    clearCatalogEntry(); byId("catalog-controls").hidden = true;
    byId("catalog-status").textContent = "Reading the host-pinned catalog...";
    try {
      const value = await post("/api/catalog", {});
      if (disposed || generation !== catalogGeneration) return;
      const ids = new Set();
      if (!catalogBinding(value) || !Array.isArray(value.entries) || !value.entries.length || value.entries.length > 512 ||
          !value.entries.every(row => {
            if (!record(row) || !validIdentity(row.identity) || !nonempty(row.display_name) || !nonempty(row.description)) return false;
            const key = identityKey(row.identity); if (ids.has(key)) return false; ids.add(key); return true;
          })) throw new RequestError("invalid_catalog");
      catalog = value; byId("catalog-controls").hidden = false; filterCatalog();
    } catch {
      if (!disposed && generation === catalogGeneration) byId("catalog-status").textContent = "The pinned catalog could not be read. Refresh status to try reading it again. No library was prepared.";
    } finally { if (generation === catalogGeneration) catalogLoading = false; }
  }
  async function loadCatalogEntry() {
    if (disposed || !available || state?.mode === "recovery" || !catalogBinding(catalog)) return;
    const selected = catalog?.entries.find(row => identityKey(row.identity) === byId("catalog-choice").value);
    if (entryRequest && currentCatalogEntry(entryRequest)) return;
    const retrying = entryRetry && currentCatalogEntry(entryRetry);
    cancelRoute(); clearCatalogEntry(Boolean(retrying));
    if (!selected) return;
    const request = { generation: entryGeneration, catalog, key: identityKey(selected.identity) };
    entryRequest = request;
    byId("catalog-status").textContent = "Reading catalog entry...";
    try {
      const value = await post("/api/catalog_entry", selected.identity);
      if (!currentCatalogEntry(request)) return;
      const entry = value?.entry;
      if (!catalogBinding(value) || !record(entry) || !validIdentity(entry.identity) ||
          identityKey(entry.identity) !== identityKey(selected.identity) || entry.display_name !== selected.display_name ||
          entry.description !== selected.description || !Array.isArray(entry.strategies)) throw new RequestError("invalid_catalog_entry");
      const options = value.setup_options;
      if (options !== undefined && (!/^[0-9a-f]{64}$/.test(value.routes_sha256) || !Array.isArray(options) ||
          options.length !== entry.strategies.length || !options.every((option, index) =>
            validRouteOption(option, entry.identity, entry.strategies[index]?.id)))) throw new RequestError("invalid_route_options");
      byId("catalog-title").textContent = entry.display_name;
      byId("catalog-description").textContent = "Choosing a setup option may prepare local software before opening configuration. Review its operations and account requirements before installation. Account access and readiness are checked separately.";
      byId("catalog-source-description").textContent = entry.description;
      byId("catalog-identity").textContent = `${entry.identity.publisher} / ${entry.identity.name} / ${entry.identity.version}`;
      const choices = [], strategyDetails = [];
      entry.strategies.forEach((strategy, index) => {
        const detail = node("details");
        if (!record(strategy) || typeof strategy.instructions !== "string") throw new RequestError("invalid_strategy");
        // The catalog contains literal escaped line breaks. Only this readable
        // presentation expands them; the exact record below stays unchanged.
        detail.append(node("summary", `Option ${index + 1}: ${strategy.id} (${String(strategy.kind).replaceAll("_", " ")})`),
          node("p", strategy.instructions.replaceAll("\\n", "\n"), "catalog-instructions"));
        for (const [key, label, empty] of [["known_requirements", "Host requirements", "No structured host requirements are listed. Check the instructions above."],
          ["unresolved_details", "Details still to resolve", "No unresolved details are listed for this strategy."],
          ["sources", "Source references", "No additional source references are listed."]]) {
          const values = strategy[key] ?? [];
          if (!Array.isArray(values)) throw new RequestError("invalid_strategy");
          const list = node("ul");
          description(list, values.map(value => typeof value === "string" ? value : JSON.stringify(value)), empty);
          detail.append(node("h4", label), list);
        }
        const option = options?.find(item => item.strategy_id === strategy.id);
        const choice = node("div", "", "catalog-option");
        choice.append(node("h5", `Option ${index + 1}: ${option && option.status !== "manual" ? `${FAMILY_LABELS[option.family]} / ${option.profile}` : `Manual setup - ${String(strategy.kind).replaceAll("_", " ")} / ${strategy.id}`}`));
        if (option && ["unprepared", "prepared"].includes(option.status)) {
          const button = node("button", "Set up this option"); button.type = "button";
          button.setAttribute("aria-label", `Set up this option: ${FAMILY_LABELS[option.family]} / ${option.profile} (option ${index + 1})`);
          button.dataset.catalogRoute = "true";
          button.addEventListener("click", () => { void chooseRoute(option, value.routes_sha256); });
          choice.append(node("p", option.status === "unprepared" ? "Prepare this local library, then review its configuration. This does not install a configured integration or sign in." :
            "Local setup information is prepared. Open configuration to review operations and account requirements."), button);
        } else {
          choice.append(node("p", !option || option.status === "manual" ? "Manual setup: no setup action is supplied here. Review this option's instructions below, then choose a library and profile using the controls below." :
            option.status === "disabled" ? "This library is not enabled by the host. Ask the host owner to enable it or choose another option." :
            option.status === "preparing" ? "Local preparation is running. Refresh host status; this option cannot start another preparation." :
            "The previous preparation outcome is unconfirmed. Check host status and its recovery controls before continuing; this option cannot start another preparation."));
        }
        if (option?.family === "official-playwright") choice.append(node("p", "This option exposes the whole Playwright endpoint. Its operations cannot be limited here.", "notice"));
        if (option?.family === "official-github") choice.append(node("p", "This is a fixed read-only route. Its listed operations cannot be changed here.", "notice"));
        choices.push(choice); strategyDetails.push(detail);
      });
      byId("catalog-strategies").replaceChildren(...choices);
      byId("catalog-strategy-details").replaceChildren(...strategyDetails);
      byId("catalog-technical").open = false;
      byId("catalog-record").textContent = JSON.stringify(entry, null, 2);
      byId("catalog-binding").textContent = `Catalog SHA-256 ${value.catalog_sha256}; bundle contents SHA-256 ${value.contents_sha256}.`;
      byId("catalog-next").textContent = options?.some(option => ["unprepared", "prepared"].includes(option.status)) ? "Use Set up this option to prepare its library if needed and open its exact profile. Review operations and enter configuration before installing. Setup choices are above; their technical instructions are below." :
        "No setup action is currently available for this entry. Review the reasons above and its technical instructions below. Selecting a catalog entry does not choose a library or profile for you.";
      byId("catalog-detail").hidden = false; byId("catalog-status").textContent = "Catalog entry loaded. Choose a setup option below. Reading this entry did not start setup.";
      // Only an explicit retry may return focus to the list when its button
      // disappears. Passive reads never move focus, including delayed replies.
      if (retrying && document.activeElement === byId("catalog-entry-retry")) focus("catalog-choice");
      byId("catalog-entry-retry").hidden = true;
    } catch {
      if (currentCatalogEntry(request)) {
        entryRetry = request; byId("catalog-entry-retry").hidden = false;
        byId("catalog-status").textContent = "This catalog entry could not be confirmed. Retry entry details to read its metadata again. No setup was started.";
      }
    } finally {
      if (entryRequest === request) { entryRequest = null; locks(); }
    }
  }
  function validRouteOption(option, identity, strategy) {
    if (!record(option) || option.strategy_id !== strategy) return false;
    if (option.status === "manual") return option.reason === "no_bound_setup_route";
    return ["disabled", "unprepared", "preparing", "prepared", "unconfirmed"].includes(option.status) &&
      validIdentity(option.identity) && identityKey(option.identity) === identityKey(identity) &&
      typeof option.family === "string" && Object.hasOwn(FAMILY_LABELS, option.family) &&
      typeof option.profile === "string" && /^[A-Za-z0-9_.-]{1,128}$/.test(option.profile) &&
      record(option.artifact) && typeof option.artifact.path === "string" && /^[0-9a-f]{64}$/.test(option.artifact.sha256);
  }
  function cancelRoute() {
    routeGeneration += 1;
    if (priorSelection || priorReview) { priorSelection = null; priorReview = null; resetHelp(); }
    if (pending?.kind === "prior-selection") pending = null;
    if (routeSelection) { routeSelection = null; resetHelp(); }
    if (pending?.kind === "catalog-route") pending = null;
  }
  async function chooseRoute(option, routesSha) {
    if (pending || running() || !available || state?.mode === "recovery") return;
    cancelRoute(); resetHelp(); problem("");
    const generation = routeGeneration;
    routeSelection = { option: structuredClone(option), generation, preparationRequested: false,
      request: { ...option.identity, strategy_id: option.strategy_id, routes_sha256: routesSha } };
    elements.family.value = option.family; elements.profile.value = ""; families();
    await continueRoute();
  }
  async function continueRoute() {
    const selection = routeSelection;
    if (!selection || pending || !available || running()) return;
    if (state?.mode === "recovery" || selectedFamily()?.id !== selection.option.family ||
        selectedFamily()?.status === "unconfirmed") { cancelRoute(); return; }
    const current = () => !disposed && selection === routeSelection && selection.generation === routeGeneration;
    const request = { kind: "catalog-route" }; pending = request; locks();
    let prepare = false;
    try {
      const value = await post("/api/catalog_route", selection.request);
      if (!current()) return;
      const { option } = selection;
      if (value?.scope !== "catalog_setup_handoff" || !catalogBinding({ ...value, scope: "catalog_metadata_only" }) || value.routes_sha256 !== selection.request.routes_sha256 ||
          !record(value.route) || !validIdentity(value.route.identity) ||
          identityKey(value.route.identity) !== identityKey(option.identity) ||
          value.route.strategy_id !== option.strategy_id || value.route.family !== option.family || value.route.profile !== option.profile ||
          !record(value.artifact) || value.artifact.path !== option.artifact.path || value.artifact.sha256 !== option.artifact.sha256)
        throw new RequestError("invalid_catalog_route");
      if (value.status === "preparation_required") {
        if (selection.preparationRequested || selectedFamily()?.status !== "unprepared") throw new RequestError("preparation_unconfirmed");
        selection.preparationRequested = true; prepare = true;
      } else if (value.status === "prepared") {
        const expected = selectedFamily()?.profiles.find(item => item.id === option.profile)?.operations;
        const next = normalizeHelp(value.help, option.family, option.profile);
        if (!expected || JSON.stringify(next.operations.map(({ id, effect }) => ({ id, effect }))) !==
            JSON.stringify(expected.map(({ id, effect }) => ({ id, effect })))) throw new RequestError("invalid_route_profile");
        elements.profile.value = option.profile; help = next; routeSelection = null; drawHelp();
      } else throw new RequestError("invalid_catalog_route");
    } catch (error) {
      if (current()) { cancelRoute(); resetHelp(); problem(describeFailure(error, "This exact catalog option could not be confirmed. No fallback profile was selected. Refresh host status before continuing.")); }
    } finally { if (pending === request) { pending = null; locks(); } }
    if (prepare && current()) await startJob("prepare");
  }
  function priorState() {
    const value = state?.prior_selection;
    return record(value) && /^[0-9a-f]{64}$/.test(value.request_sha256) &&
      typeof value.family === "string" && Object.hasOwn(FAMILY_LABELS, value.family) &&
      record(state.family_contents_sha256) && Object.hasOwn(state.family_contents_sha256, value.family) &&
      sha256(state.family_contents_sha256[value.family]) &&
      typeof value.profile === "string" && /^[A-Za-z0-9_.-]{1,128}$/.test(value.profile) ? value : null;
  }
  function drawPriorAvailability() {
    const source = priorState();
    byId("prior-panel").hidden = !source || state?.mode === "recovery";
    byId("prior-summary").textContent = source ? `Historical ${source.family} / ${source.profile}; request SHA-256 ${source.request_sha256}.` : "";
  }
  async function reusePrior() {
    if (pending || running() || !available || state?.mode === "recovery") return;
    const source = priorState();
    if (!source) return;
    cancelRoute(); resetHelp(); problem("");
    priorSelection = { ...source, generation: routeGeneration, preparationRequested: false };
    elements.family.value = source.family; elements.profile.value = ""; families();
    await continuePrior();
  }
  async function continuePrior() {
    const selection = priorSelection;
    if (!selection || pending || !available || running()) return;
    if (state?.mode === "recovery" || selectedFamily()?.id !== selection.family || selectedFamily()?.status === "unconfirmed" ||
        priorState()?.request_sha256 !== selection.request_sha256) { cancelRoute(); return; }
    const current = () => !disposed && priorSelection === selection && routeGeneration === selection.generation;
    const request = { kind: "prior-selection" }; pending = request; locks();
    let prepare = false;
    try {
      const value = await post("/api/successor", { prior_request_sha256: selection.request_sha256 });
      if (!current()) return;
      if (!record(value) || value.scope !== "successor_configuration_candidate" ||
          value.prior_request_sha256 !== selection.request_sha256 || !sha256(value.current_contents_sha256) ||
          value.current_contents_sha256 !== state.family_contents_sha256?.[selection.family] ||
          value.family !== selection.family || value.profile !== selection.profile) throw new RequestError("invalid_prior_selection");
      if (value.status === "preparation_required") {
        if (selection.preparationRequested || selectedFamily()?.status !== "unprepared") throw new RequestError("preparation_unconfirmed");
        selection.preparationRequested = true; prepare = true;
      } else if (value.status === "review_required") {
        const next = normalizeHelp(value.help, selection.family, selection.profile);
        const expected = selectedFamily()?.profiles.find(item => item.id === selection.profile)?.operations;
        if (!expected || JSON.stringify(next.operations.map(({ id, effect }) => ({ id, effect }))) !==
            JSON.stringify(expected.map(({ id, effect }) => ({ id, effect }))) || !record(value.values) ||
            !Array.isArray(value.prior_operations) || !value.prior_operations.length || value.prior_operations.length > 128 ||
            new Set(value.prior_operations).size !== value.prior_operations.length ||
            !value.prior_operations.every(name => typeof name === "string" && /^[A-Za-z0-9_.-]{1,128}$/.test(name)) ||
            !Array.isArray(value.unapplied) || value.unapplied.length > 384) throw new RequestError("invalid_prior_selection");
        const seen = new Set();
        for (const item of value.unapplied) {
          if (!record(item) || !["field", "operation"].includes(item.kind) || typeof item.name !== "string" ||
              !/^[A-Za-z0-9_.-]{1,128}$/.test(item.name) || typeof item.reason !== "string" ||
              !["not_in_current_profile", "current_type_differs", "not_representable_in_current_form", "current_reference_name_required", "outside_current_bounds"].includes(item.reason) ||
              item.kind === "field" && !["string", "integer", "boolean", "string_list"].includes(item.prior_type) || seen.has(item.kind + ":" + item.name))
            throw new RequestError("invalid_prior_selection");
          seen.add(item.kind + ":" + item.name);
        }
        for (const [name, entered] of Object.entries(value.values)) {
          const field = next.fields.find(item => item.name === name);
          if (!field || seen.has("field:" + name) ||
              field.kind === "string" && (typeof entered !== "string" || !entered || entered.length > 2048 || /[\x00-\x1f\x7f]/.test(entered) || field.credential_reference && !/^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(entered)) ||
              field.kind === "integer" && !Number.isSafeInteger(entered) || field.kind === "boolean" && typeof entered !== "boolean" ||
              field.kind === "string_list" && !stringListValue(entered))
            throw new RequestError("invalid_prior_selection");
        }
        elements.profile.value = selection.profile; help = next; priorSelection = null; drawHelp();
        for (const [name, entered] of Object.entries(value.values)) inputs.get(name).input.value = fieldText(entered);
        priorReview = { request_sha256: selection.request_sha256, values: structuredClone(value.values),
          operations: [...value.prior_operations], unapplied: structuredClone(value.unapplied), omitted: new Set(), edited: new Set() };
        invalidate();
      } else if (value.status === "profile_unavailable") {
        throw new RequestError("prior_profile_unavailable");
      } else throw new RequestError("invalid_prior_selection");
    } catch (error) {
      if (current()) { cancelRoute(); resetHelp(); problem(describeFailure(error, "The exact historical profile or its current proposal could not be confirmed. No different profile was selected. Use the current library controls to start a new configuration.")); }
    } finally { if (pending === request) { pending = null; locks(); } }
    if (prepare && current()) await startJob("prepare");
  }
  function priorIssues() {
    if (!priorReview || !help) return [];
    const rows = priorReview.unapplied.map(item => ({ ...item }));
    for (const name of Object.keys(priorReview.values)) {
      const field = inputs.get(name)?.field;
      if (field && !applicable(field, selectedOperations())) rows.push({ kind: "field", name, reason: "not_used_by_selected_operations" });
    }
    return rows.map(item => {
      const replacement = item.kind === "field" && priorReview.edited.has(item.name) && inputs.get(item.name)?.input.value !== "" &&
        applicable(inputs.get(item.name).field, selectedOperations());
      return { ...item, key: item.kind + ":" + item.name, resolved: Boolean(replacement) || priorReview.omitted.has(item.kind + ":" + item.name) };
    });
  }
  function drawPriorReview() {
    const list = byId("prior-unapplied");
    const focusedIssue = list.contains(document.activeElement) ? document.activeElement.dataset.priorIssue : null;
    byId("prior-review").hidden = !priorReview;
    if (!priorReview) { byId("prior-unapplied").replaceChildren(); return; }
    const currentOperations = help.selectable_operations ?
      "Choose the current operations again below; no prior operation was selected automatically." :
      `The current profile requires the complete list shown below${help.family === "official-playwright" ? " and exposes the whole endpoint" : ""}. Review it before validation and installation; the historical list does not restore permission.`;
    byId("prior-operations").textContent = `Previously requested operations: ${priorReview.operations.join(", ")}. ${currentOperations}`;
    const rows = priorIssues();
    byId("prior-unapplied").replaceChildren(...rows.map(item => {
      const row = node("li"), label = node("label"), checkbox = node("input");
      checkbox.type = "checkbox"; checkbox.checked = priorReview.omitted.has(item.key);
      checkbox.dataset.priorIssue = item.key;
      checkbox.addEventListener("change", () => {
        if (!priorReview) return;
        if (checkbox.checked) {
          priorReview.omitted.add(item.key);
          if (item.kind === "field" && Object.hasOwn(priorReview.values, item.name) && !priorReview.edited.has(item.name)) inputs.get(item.name).input.value = "";
        } else priorReview.omitted.delete(item.key);
        invalidate();
      });
      label.append(checkbox, document.createTextNode(` Do not carry forward prior ${item.kind} ${item.name} (${item.reason.replaceAll("_", " ")}).`));
      row.append(label);
      if (item.resolved && !checkbox.checked) row.append(node("span", " Replaced with an explicitly edited current value."));
      return row;
    }));
    if (focusedIssue) [...list.querySelectorAll("[data-prior-issue]")].find(item => item.dataset.priorIssue === focusedIssue)?.focus();
    const remaining = rows.filter(item => !item.resolved).length;
    byId("prior-review-status").textContent = remaining ? `${remaining} historical items need an explicit replacement or omission before validation.` :
      "Historical differences are accounted for. The current parser must still validate the new selection; this is not compatibility or account proof.";
  }
  function invalidate() {
    revision += 1;
    validation = null;
    for (const input of elements["configuration-form"].querySelectorAll("[aria-invalid]")) input.removeAttribute("aria-invalid");
    elements.profile.removeAttribute("aria-invalid");
    byId("configure-heading").removeAttribute("aria-invalid");
    drawPriorReview();
    const hadReviewFocus = elements.review.contains(document.activeElement);
    elements.review.hidden = true;
    byId("review-credential-references").replaceChildren();
    byId("validation-status").textContent = help ? "Validate after choosing operations and entering configuration." : "";
    if (hadReviewFocus) focus("configure-heading");
    locks();
  }
  function selectedOperations() {
    return [...elements.operations.querySelectorAll("input:checked")].map(input => input.value);
  }
  function stopAwaitingObservation(jobId) {
    return stopRequests.has(jobId) && stopRequests.get(jobId) !== "retry_available";
  }
  function retryablePreparation(job) {
    return job?.kind === "prepare" && job.status === "unconfirmed" &&
      state?.families.some(family => family.id === job.family && family.status === "unconfirmed" &&
        family.preparation_job_id === job.id);
  }
  function retryableExport(job) {
    return isRecipeExport(job) && job.status === "unconfirmed" && job.recovered !== true;
  }
  function retryableSelected(job) {
    return Boolean(selectedKind(job)) && job.kind === "install" && job.status === "unconfirmed" && job.recovered !== true;
  }
  function locks() {
    byId("catalog-entry-retry").setAttribute("aria-disabled", String(!entryRetry || !currentCatalogEntry(entryRetry) || Boolean(entryRequest)));
    drawRecovery();
    const busy = !available || Boolean(pending) || running();
    const readOnly = state?.mode === "recovery";
    // Choosing another profile is local navigation. Keep it available while
    // the one admitted settings read finishes, without starting another read.
    const selectionBusy = !available || (Boolean(pending) || running()) && pending?.kind !== "help";
    elements.family.disabled = selectionBusy || readOnly || !state?.families.length;
    elements.profile.disabled = selectionBusy || readOnly || selectedFamily()?.status !== "prepared";
    byId("show-profile").setAttribute("aria-disabled", String(busy || readOnly || selectedFamily()?.status !== "prepared" || !elements.profile.value));
    drawProfileStatus();
    elements.prepare.disabled = busy || readOnly || selectedFamily()?.status !== "unprepared";
    elements.validate.disabled = busy || readOnly || !help || priorIssues().some(item => !item.resolved);
    elements.install.disabled = busy || readOnly || !validation;
    elements.reconcile.disabled = busy || historyLoading || !viewedReconciliation()?.available || !["install", "export", "acquire"].includes(viewedJob()?.kind);
    elements["stop-job"].hidden = !isCompose(viewedJob()) && (!state?.stop?.available || readOnly || !viewedJob());
    elements["stop-job"].disabled = !available || Boolean(pending) || historyLoading || readOnly ||
      !state?.stop?.available || viewedJob()?.stop_supported === false || viewedJob()?.status !== "running" || Boolean(viewedJob()?.stop) || stopAwaitingObservation(viewedJob()?.id);
    elements["retry-prepare"].hidden = !state?.preparation_retry?.available || readOnly ||
      viewedJob()?.kind !== "prepare" || viewedJob()?.status !== "unconfirmed";
    elements["retry-prepare"].disabled = busy || historyLoading || readOnly || !state?.preparation_retry?.available ||
      !retryablePreparation(viewedJob()) || ["pending", "accepted", "unconfirmed"].includes(preparationRetries.get(viewedJob()?.id));
    elements["retry-export"].hidden = !state?.export_retry?.available || readOnly || !retryableExport(viewedJob());
    elements["retry-export"].disabled = busy || historyLoading || readOnly || !state?.export_retry?.available ||
      !retryableExport(viewedJob()) || ["pending", "accepted", "unconfirmed"].includes(exportRetries.get(viewedJob()?.id));
    const selectedRetry = selectedRetries.get(viewedJob()?.id);
    elements["retry-selected"].hidden = !state?.selected_retry?.available || readOnly || !retryableSelected(viewedJob());
    elements["retry-selected"].disabled = busy || historyLoading || readOnly || !state?.selected_retry?.available ||
      !retryableSelected(viewedJob());
    elements["install-selected-retry"].disabled = busy || historyLoading || readOnly || !state?.selected_retry?.available ||
      !retryableSelected(viewedJob()) || selectedRetry?.phase !== "review" || selectedRetry.proposal.job_id !== null;
    byId("jobs-earlier").disabled = historyLoading;
    for (const input of elements.operations.querySelectorAll("input")) input.disabled = busy || readOnly || help?.selectable_operations === false;
    for (const { input, field, container } of inputs.values()) {
      const shown = applicable(field, selectedOperations());
      container.hidden = !shown;
      input.disabled = busy || readOnly || !shown;
      input.required = shown && field.required;
    }
    for (const button of document.querySelectorAll("[data-catalog-route]")) button.disabled = busy || readOnly;
    byId("reuse-prior").disabled = busy || readOnly || !priorState() || !state?.families.some(item => item.id === priorState()?.family && ["unprepared", "prepared"].includes(item.status));
    for (const checkbox of document.querySelectorAll("[data-prior-issue]")) checkbox.disabled = busy || readOnly;
    elements.refresh.disabled = disposed || !session;
  }

  function revokeRecoveryURL() {
    if (recoveryURL !== null) { window.URL.revokeObjectURL(recoveryURL); recoveryURL = null; }
  }
  function drawRecovery() {
    const supported = state?.recovery_manifest?.available === true;
    byId("recovery-panel").hidden = !supported;
    for (const job of [state?.job, jobView?.job]) {
      const selected = recoveryEntries.get(job?.id);
      if (selected && JSON.stringify(recoveryAnchor(job)) !== JSON.stringify(selected.anchor)) recoveryConflicts.add(job.id);
    }
    if (recoveryRequest && (!supported || !available || recoveryConflicts.size)) {
      recoveryRequest = null;
      byId("recovery-status").textContent = "Host status or a selected request changed. No download was requested; selection kept. Refresh and review it before saving again.";
    }
    const anchor = recoveryAnchor(viewedJob()), busy = recoveryRequest !== null;
    byId("recovery-add").disabled = !supported || !available || busy || historyLoading || !anchor ||
      recoveryEntries.has(anchor.id) || recoveryEntries.size >= 256;
    byId("recovery-save").disabled = !supported || !available || busy || !recoveryEntries.size || Boolean(recoveryConflicts.size);
    byId("recovery-current").textContent = !anchor ? "The viewed job has no retained request to select." :
      recoveryEntries.has(anchor.id) ? "The viewed request is selected." : "Add the viewed request, including work that is still running.";
    const listKey = JSON.stringify([[...recoveryEntries], [...recoveryConflicts], busy]);
    if (listKey === recoveryListKey) return;
    recoveryListKey = listKey;
    byId("recovery-selection").replaceChildren();
    for (const { anchor: entry, label } of recoveryEntries.values()) {
      const row = node("li"), description = node("span", `${label} (${entry.id})${recoveryConflicts.has(entry.id) ? " - request changed; remove and review again" : ""}`);
      const remove = node("button", "Remove"); remove.type = "button"; remove.dataset.recoveryId = entry.id;
      remove.setAttribute("aria-label", `Remove ${label} (${entry.id}) from recovery selection`); remove.disabled = busy;
      row.append(description, document.createTextNode(" "), remove); byId("recovery-selection").append(row);
    }
  }
  function addRecovery() {
    if (byId("recovery-add").disabled || disposed) return;
    const job = viewedJob(), anchor = recoveryAnchor(job);
    if (!anchor) return;
    recoveryEntries.set(anchor.id, { anchor, label: `${job.family}: ${job.kind}` });
    byId("recovery-status").textContent = `${recoveryEntries.size} request${recoveryEntries.size === 1 ? "" : "s"} selected.`;
    drawRecovery();
  }
  async function saveRecovery() {
    if (byId("recovery-save").disabled || disposed) return;
    const request = { anchors: [...recoveryEntries.values()].map(entry => ({ ...entry.anchor })) };
    recoveryRequest = request; byId("recovery-status").textContent = "Verifying the selected recovery file..."; drawRecovery();
    try {
      const value = await post("/api/recovery_manifest", { job_ids: request.anchors.map(entry => entry.id) });
      if (disposed || recoveryRequest !== request) return;
      const bytes = await recoveryFile(value, request.anchors, crypto);
      if (disposed || recoveryRequest !== request) return;
      revokeRecoveryURL();
      recoveryURL = window.URL.createObjectURL(new window.Blob([bytes], { type: "application/json" }));
      const link = node("a"); link.href = recoveryURL; link.download = "toolkit-recovery.json";
      link.hidden = true; document.body.append(link);
      try { link.click(); } finally { link.remove(); }
      byId("recovery-status").textContent = `Recovery file download requested. SHA-256: ${value.sha256}. Keep the original workspace; the file contains references, not a backup or a completion claim.`;
    } catch (error) {
      if (!disposed && recoveryRequest === request) {
        revokeRecoveryURL();
        byId("recovery-status").textContent = describeFailure(error, "Could not verify or complete the recovery download. Selection kept. Check your downloads and refresh status before trying again.");
      }
    } finally { if (!disposed && recoveryRequest === request) { recoveryRequest = null; drawRecovery(); } }
  }
  function fillSelect(select, items, placeholder, desired) {
    select.replaceChildren(new window.Option(placeholder, ""));
    for (const item of items) select.append(new window.Option(item.label ?? item.id, item.id));
    select.value = items.some(item => item.id === desired) ? desired : "";
  }
  function families() {
    if (!state) return;
    const familyId = elements.family.value, profileId = elements.profile.value;
    fillSelect(elements.family, state.families.map(item => ({ id: item.id, label: FAMILY_LABELS[item.id] ?? item.id })), "Choose a library", familyId);
    const family = selectedFamily();
    fillSelect(elements.profile, family?.profiles ?? [], family?.status === "prepared" ? "Choose an exact profile" : "Prepare a library first", profileId);
    byId("preparation").hidden = !family || family.status === "prepared";
    byId("preparation-description").textContent = family?.status === "preparing" ? "The host is preparing this library. Profile choices appear when preparation succeeds." :
      family?.status === "unconfirmed" ? (state?.preparation_retry?.available ?
        "Preparation is unconfirmed. View its latest preparation job below to try again in a fresh directory. Earlier files are retained." :
        "Preparation is unconfirmed. Inspect the host result before requesting another preparation.") :
      family?.id === "official-playwright" ? "Prepare the isolated Chrome profile. This validates metadata only; it does not install or launch a browser. All endpoint tools remain exposed." :
      family?.id === "official-github" ? "Prepare the fixed GitHub.com read-only route. This validates pinned metadata only; it does not copy or start GitHub." :
      "Prepare this library to read its installed profiles and configuration guidance.";
    if (help && (help.family !== elements.family.value || help.profile !== elements.profile.value)) resetHelp();
    locks();
  }
  function resetHelp() {
    helpGeneration += 1;
    // Invalidating the selection does not release an outstanding host read.
    help = null; helpFailure = null; priorReview = null; inputs.clear(); elements.configuration.hidden = true;
    elements.operations.replaceChildren(); elements.fields.replaceChildren(); invalidate();
  }
  function drawProfileStatus() {
    const family = selectedFamily(), profile = elements.profile.value;
    const selected = family?.status === "prepared" && profile;
    const reading = pending?.kind === "help";
    const message = state?.mode === "recovery" ? "This session is read-only. Profile setup is unavailable." :
      !available ? "Connect to the local setup host to view profile settings." :
      help ? `Settings loaded for ${help.profile}. Review operations and enter configuration below.` :
      reading ? (pending.family === family?.id && pending.profile === profile && pending.generation === helpGeneration ?
        `Loading settings for ${profile}. You can keep browsing profiles while this request finishes.` :
        "The earlier settings request is still finishing. Then choose Show profile settings for your current selection.") :
      helpFailure?.family === family?.id && helpFailure?.profile === profile ? helpFailure.message :
      !family || family.status !== "prepared" ? "Choose a library and prepare it to view profile settings." :
      !selected ? "Choose a profile, then select Show profile settings. Browsing profiles does not load settings." :
      running() || pending ? "Host work is in progress. Show profile settings when it finishes." :
      `Select Show profile settings to configure ${profile}. This reads guidance without installing software or connecting an account.`;
    const status = byId("profile-status");
    if (status.textContent !== message) status.textContent = message;
  }
  function description(list, values, empty) {
    list.replaceChildren(...(values.length ? values : [empty]).map(value => node("li", value)));
  }
  function drawHelp(moveFocus = true) {
    elements.configuration.hidden = false;
    byId("profile-description").textContent = `${FAMILY_LABELS[help.family] ?? help.family} / ${help.profile}`;
    description(byId("prerequisites"), help.prerequisites.map(item => `${item.kind.replaceAll("_", " ")}: ${item.instruction}`), "No additional prerequisite is declared by this profile.");
    description(byId("unresolved"), help.unresolved_details, "No unresolved detail is listed. This is not an account-readiness check.");
    byId("help-scope").textContent = help.scope;
    byId("reference-text").textContent = help.guide_text || "The host did not supply a configuration reference. Check its retained profile guide before proceeding.";
    elements.operations.replaceChildren();
    const operationFieldset = byId("operations-fieldset");
    operationFieldset.querySelector("legend").textContent = help.selectable_operations ? "Choose operations" : "Fixed operations";
    operationFieldset.querySelector(".hint").textContent = help.selectable_operations ?
      "Select each operation explicitly. An available write operation is not permission to perform it." :
      "This profile uses the complete operation list below. Individual operations cannot be selected.";
    help.operations.forEach((operation, index) => {
      const label = node("label", undefined, "operation");
      const input = node("input"); input.type = "checkbox";
      if (help.selectable_operations === false) { input.checked = true; input.disabled = true; } input.value = operation.id; input.id = `operation-${index}`;
      const content = node("span", operation.label, "operation-name");
      content.append(node("span", EFFECT_LABELS[operation.effect], `effect ${operation.effect}`));
      label.append(input, content); elements.operations.append(label);
    });
    inputs.clear(); elements.fields.replaceChildren();
    help.fields.forEach((field, index) => {
      const container = node("div", undefined, "field"), label = node("label");
      const fieldId = `configuration-field-${index}`, hintId = `${fieldId}-hint`;
      label.htmlFor = fieldId;
      label.append(node("span", field.name.replaceAll("_", " ") + (field.credential_reference ? " (environment variable name)" : "")));
      label.append(node("span", field.required ? "Required" : "Optional", "field-necessity"), node("span", field.name, "field-code"));
      let input;
      if (field.kind === "boolean") {
        input = node("select"); input.append(new window.Option(field.required ? "Choose Yes or No" : "Not supplied", ""), new window.Option("Yes", "true"), new window.Option("No", "false"));
      } else if (field.kind === "string_list") {
        input = node("textarea"); input.rows = 3; input.autocomplete = "off"; input.spellcheck = false;
      } else {
        input = node("input"); input.type = "text"; input.autocomplete = "off"; input.spellcheck = false; input.maxLength = field.credential_reference ? 128 : 8192;
        if (field.kind === "integer") input.inputMode = "numeric";
        if (field.credential_reference) input.pattern = "[A-Za-z_][A-Za-z0-9_]{0,127}";
      }
      input.id = fieldId; input.name = field.name; input.required = field.required; input.setAttribute("aria-describedby", hintId);
      const hints = [];
      if (field.credential_reference) hints.push("Name only, for example MY_SERVICE_TOKEN. Do not paste a token, key or password.");
      if (field.kind === "integer") hints.push("Whole number" + (field.minimum !== undefined ? `; minimum ${field.minimum}` : "") + (field.maximum !== undefined ? `; maximum ${field.maximum}` : "") + ".");
      if (field.kind === "string_list") hints.push('JSON array of strings, for example ["item"]. [] explicitly supplies an empty list. At most 256 strings, each at most 2048 characters without control characters.');
      if (!field.required) hints.push("Leave unspecified to omit this field; the installed profile decides its behavior.");
      if (field.only_when_operation_selected) hints.push(`Used only with ${field.only_when_operation_selected}.`);
      const hint = node("p", hints.join(" "), "hint"); hint.id = hintId;
      container.append(label, input, hint); elements.fields.append(container); inputs.set(field.name, { input, field, container });
    });
    invalidate(); locks(); if (moveFocus) focus("configure-heading");
  }
  async function loadHelp() {
    if (disposed || state?.mode === "recovery" || pending || running() || !available) return;
    const family = elements.family.value, profile = elements.profile.value;
    if (selectedFamily()?.status !== "prepared" || !selectedFamily().profiles.some(item => item.id === profile)) return;
    const button = byId("show-profile"), ownedFocus = document.activeElement === button;
    resetHelp();
    const generation = helpGeneration;
    const current = () => !disposed && available && generation === helpGeneration && state?.mode !== "recovery" &&
      elements.family.value === family && elements.profile.value === profile && selectedFamily()?.status === "prepared";
    const request = { kind: "help", family, profile, generation }; pending = request; problem(""); locks();
    try {
      const value = await post("/api/help", { family, profile });
      if (!current()) return;
      const next = normalizeHelp(value, family, profile);
      const expected = selectedFamily()?.profiles.find(item => item.id === profile)?.operations;
      if (!expected || JSON.stringify(next.operations.map(({ id, effect }) => ({ id, effect }))) !== JSON.stringify(expected.map(({ id, effect }) => ({ id, effect })))) throw new FormError("invalid_help");
      help = next; drawHelp(false);
    } catch (error) {
      if (current()) {
        resetHelp();
        helpFailure = { family, profile, message: describeFailure(error, "Could not load verified configuration guidance. Use Show profile settings to try this selection again. No installation was requested.") };
        problem(helpFailure.message);
      }
    } finally {
      if (!disposed && pending === request) {
        pending = null; locks();
        // An explicit settings action can advance to its result only while
        // that same action still owns focus. Delayed reads never reclaim it.
        if (current() && help && ownedFocus && document.activeElement === button) focus("configure-heading");
      }
    }
  }
  function draft() {
    const entered = Object.create(null);
    for (const [name, { input }] of inputs) entered[name] = input.value;
    return draftFromFields(help, selectedOperations(), entered);
  }
  function row(list, key, value) { list.append(node("dt", key), node("dd", value)); }
  function showReferences(id, value, readback = false) {
    const container = byId(id); container.replaceChildren();
    container.append(node("h3", "Host environment reference names"));
    if (readback) {
      container.append(node("p", "This file check did not revalidate credential references. No earlier reference list is restored or presented as current."));
    } else if (!Object.hasOwn(value, "credential_environment_references")) {
      container.append(node("p", "This validator does not report credential references. Their requirements are unknown; do not assume none."));
    } else if (!value.credential_environment_references.length) {
      container.append(node("p", "The validator reports no credential environment references for this configuration."));
    } else {
      const list = node("dl", undefined, "review-values");
      for (const reference of value.credential_environment_references) {
        row(list, JSON.stringify(reference.field_path), reference.environment_variable);
      }
      container.append(list);
    }
    container.append(node("p", "Names only. Your host owns credential lookup and permission to inject them. No credential or account access was checked.", "hint"));
  }
  function drawReview() {
    const list = byId("review-values"); list.replaceChildren();
    row(list, "Library", FAMILY_LABELS[validation.draft.family] ?? validation.draft.family);
    row(list, "Profile", validation.draft.profile);
    row(list, validation.draft.family === "official-playwright" ? "Adopted coverage (whole endpoint exposed)" : "Operations", validation.draft.operations.map(id => `${id} (${EFFECT_LABELS[help.operations.find(item => item.id === id).effect]})`).join("\n"));
    for (const field of help.fields) {
      if (Object.hasOwn(validation.draft.values, field.name)) row(list, field.name + (field.credential_reference ? " (reference name)" : ""), fieldText(validation.draft.values[field.name]));
    }
    byId("configuration-digest").textContent = `Exact configuration SHA-256: ${validation.configuration_sha256}`;
    showReferences("review-credential-references", validation);
    elements.review.hidden = false; focus("review-heading");
  }
  async function validate(event) {
    if (state?.mode === "recovery") { event?.preventDefault(); return; }
    event.preventDefault();
    if (pending || running() || !available || !help || priorIssues().some(item => !item.resolved)) return;
    problem("");
    for (const { input } of inputs.values()) input.setCustomValidity("");
    let current;
    try { current = draft(); }
    catch (error) {
      const input = inputs.get(error.field)?.input;
      const message = FIELD_ERRORS[error.code] ?? "Choose at least one operation.";
      if (input) { input.setCustomValidity(message); input.reportValidity(); input.focus(); }
      else elements.operations.querySelector("input")?.focus();
      byId("validation-status").textContent = message; return;
    }
    if (!elements["configuration-form"].reportValidity()) return;
    invalidate(); const atRevision = revision;
    const contents = state.family_contents_sha256?.[current.family];
    let diagnosticFocus = null;
    const request = { kind: "checking" }; pending = request; byId("validation-status").textContent = "Checking with the installed profile validator..."; locks();
    try {
      const result = await post("/api/check", current);
      if (disposed || revision !== atRevision) return;
      if (typeof result.validation_id !== "string" || !result.validation_id ||
          !/^[0-9a-f]{64}$/.test(result.configuration_sha256) || result.scope !== "configuration_only" ||
          !validReferenceHandoff(result, current)) throw new RequestError("invalid_validation");
      validation = { validation_id: result.validation_id, configuration_sha256: result.configuration_sha256,
        draft: JSON.parse(JSON.stringify(current)),
        ...(Object.hasOwn(result, "credential_environment_references") ? {
          credential_environment_references: structuredClone(result.credential_environment_references) } : {}) };
      byId("validation-status").textContent = "Configuration accepted. Review the exact selection before installing.";
      drawReview();
    } catch (error) {
      if (!disposed && revision === atRevision) {
        const issue = error instanceof RequestError ? configurationIssue(error.configurationResponse, current, contents) : null;
        const message = issue?.message ?? describeFailure(error, "The installed validator did not accept this configuration. Check the configuration reference, correct the fields and validate again.");
        if (issue) {
          diagnosticFocus = issue.path[0] === "config" && issue.path.length === 2 ? inputs.get(issue.path[1])?.input :
            issue.path[0] === "operations" ? elements.operations.querySelector("input") :
            issue.path[0] === "profile" ? elements.profile : byId("configure-heading");
          diagnosticFocus?.setAttribute("aria-invalid", "true");
        }
        byId("validation-status").textContent = message; problem(message); byId("configuration-reference").open = true;
      }
    } finally {
      if (!disposed && pending === request) {
        pending = null; locks();
        if (revision === atRevision) diagnosticFocus?.focus();
      }
    }
  }
  function drawSelectedRetry(job) {
    const retry = selectedRetries.get(job?.id);
    const visible = state?.mode !== "recovery" && state?.selected_retry?.available && retryableSelected(job);
    const proposal = visible ? retry?.proposal : null;
    elements["selected-retry-review"].hidden = !proposal;
    byId("selected-retry-values").replaceChildren();
    byId("selected-retry-files").replaceChildren();
    byId("retry-selected-status").textContent = !visible ? "" : retry?.phase === "pending" ? "Preparing a new proposal for review. Installation has not been requested." :
      retry?.phase === "unconfirmed" ? "Could not confirm the proposal or installation response. Review this attempt again to read its current state; no automatic installation was requested." :
      proposal?.job_id ? `This proposal already has installation job ${proposal.job_id}. View that job in the session history; requesting the proposal again does not install it again.` :
      proposal ? "A new proposal is ready. Review its exact inputs and fresh destination, then explicitly install if appropriate." :
      "Review another attempt to create a fresh installation proposal from the retained package. Earlier files and outcomes are preserved.";
    if (!proposal) return;
    const values = byId("selected-retry-values");
    row(values, "Earlier attempt", job.id);
    row(values, "Package", `${proposal.package.publisher}/${proposal.package.name}@${proposal.package.version}`);
    row(values, "Component", proposal.component);
    row(values, "Runtime type", SELECTED_LABELS[proposal.profile]);
    row(values, "Entrypoint", proposal.entrypoint.executable);
    row(values, "Literal arguments", JSON.stringify(proposal.entrypoint.arguments));
    row(values, "MCP server name", proposal.mcp_name);
    row(values, "New destination", proposal.destination);
    row(values, "Selection SHA-256", proposal.selection_sha256);
    const digestKey = selectedDigestKey(proposal, proposal.profile);
    row(values, digestKey === "declaration_digest" ? "Software declaration digest" : "Manifest digest", proposal[digestKey]);
    if (proposal.configuration_sha256) row(values, "Configuration SHA-256", proposal.configuration_sha256);
    row(values, "New installation effect SHA-256", proposal.effect_sha256);
    for (const [name, hash] of Object.entries(proposal.files)) row(byId("selected-retry-files"), name, hash);
  }
  function showJob(job, moveFocus = true) {
    drawRecovery();
    drawSelectedRetry(job);
    elements.result.hidden = true;
    byId("result-paths").replaceChildren(); byId("mcp-configuration").textContent = "";
    byId("mcp-configuration").setAttribute("aria-label", "Actual emitted MCP configuration");
    byId("result-credential-references").replaceChildren();
    elements["job-panel"].hidden = !job;
    byId("reconciliation-status").textContent = "";
    byId("stop-status").textContent = "";
    byId("retry-prepare-status").textContent = "";
    byId("retry-export-status").textContent = "";
    byId("reconciliation-limits").replaceChildren();
    elements.reconcile.hidden = isCompose(job) || (acquisition(job) || isRecipeExport(job)) && !viewedReconciliation()?.available;
    elements.reconcile.textContent = acquisition(job) ? "Check acquired files" : isRecipeExport(job) ? "Check exported files" : "Check installation";
    if (!job) return;
    if (isCompose(job)) {
      const action = COMPOSE_ACTIONS[job.action], result = job.result;
      byId("job-status").textContent = job.status === "running" ? `${action} is running in the local host.` :
        result ? `${action}: backend ${result.backend.status}.` : `${action} is unconfirmed. No backend result was confirmed.`;
      byId("job-context").textContent = "This job records a Compose backend action. Prepared files and observed resources do not establish application readiness, account access or native attachment. Closing this page does not cancel the admitted action.";
      if (job.runtime_error) byId("job-context").textContent +=
        ` Runtime reported ${job.runtime_error}. This does not establish whether effects occurred or whether retry is safe.`;
      byId("stop-status").textContent = "Stop local setup is unavailable for Compose jobs. The bounded action settles in the host. Stopping deployments is a separate explicit backend action; canceling a waiter does not stop containers.";
      const hostStatus = composeHostStatus(state?.compose, state?.job);
      if (hostStatus) byId("job-context").textContent += " " + hostStatus;
      if (result) {
        const backend = result.backend, resultPlan = composeResultPlan(backend);
        byId("result-heading").textContent = "Compose backend result and runtime bindings";
        byId("result-description").textContent = backend.status === "prepared_successor" ?
          "The predecessor and successor metadata are retained for this exact transition. Full configuration and file contents remain with the host. Preparation does not adopt the successor or start containers." : backend.status === "rebound" ?
          "The host explicitly rebound this session to the selected successor under its existing action scope. Rebinding does not start containers or establish readiness." : backend.status === "prepared" ?
          "The canonical plan was prepared. This summary contains metadata only; full configuration and file contents remain in the host workspace. Preparation does not start containers." : result.backend.status === "unconfirmed" ?
          "The backend could not confirm this action. Resources may exist; preserve the result and obtain a separate current observation." :
          "The backend returned this resource observation for this action. It is not a current availability guarantee.";
        byId("result-limits").textContent = "These are retained action results. Any stdio runtime binding is a literal host launch handoff, not an installed MCP configuration or proof of a connected tool. It does not grant current authority; launch performs its own backend checks. State deletion requires separate explicit authorization.";
        row(byId("result-paths"), "Action", action);
        row(byId("result-paths"), backend.status === "prepared_successor" ? "Predecessor plan SHA-256" : "Result plan SHA-256", resultPlan);
        if (backend.status === "prepared_successor") row(byId("result-paths"), "Successor plan SHA-256", backend.successor.plan_sha256);
        if (backend.transition_sha256) row(byId("result-paths"), "Transition SHA-256", backend.transition_sha256);
        if (state.compose?.plan_sha256) row(byId("result-paths"), "Host-admitted plan SHA-256", state.compose.plan_sha256);
        if (state.compose?.selected_plan_sha256) row(byId("result-paths"), "Owner-selected plan SHA-256", state.compose.selected_plan_sha256);
        row(byId("result-paths"), "Backend status", result.backend.status);
        byId("mcp-configuration").setAttribute("aria-label", ["prepared", "prepared_successor", "rebound"].includes(backend.status) ?
          "Compose prepared metadata summary and stdio runtime bindings" : "Exact Compose backend result and stdio runtime bindings");
        byId("mcp-configuration").textContent = JSON.stringify({ backend: result.backend, runtime_bindings: result.runtime_bindings }, null, 2);
        elements.result.hidden = false;
      }
      const changed = lastJob !== `${job.id}/${job.status}`; lastJob = `${job.id}/${job.status}`;
      if (moveFocus && changed && job.status !== "running" && result) focus("result-heading");
      return;
    }
    const retryState = preparationRetries.get(job.id);
    if (job.kind === "prepare" && job.status === "unconfirmed" && state?.preparation_retry?.available) {
      byId("retry-prepare-status").textContent = retryState === "pending" ? "Requesting a new preparation attempt..." :
        retryState === "unconfirmed" ? "Could not confirm the new attempt. Refresh status before trying again; a preparation may already have started." :
        !retryablePreparation(job) ? "This preparation has a newer attempt. View the latest preparation for this library in the session jobs. Earlier files are retained." :
        retryState === "accepted" ? "The host returned a new attempt. Refresh status to view its progress." :
        "Try preparation again creates a new attempt with fresh files. Earlier attempts are retained. It does not retry a configured installation or connect an account.";
    }
    if (retryableExport(job) && state?.export_retry?.available && state.mode !== "recovery") {
      const exportState = exportRetries.get(job.id);
      byId("retry-export-status").textContent = exportState === "pending" ? "Requesting a new export attempt..." :
        exportState === "unconfirmed" ? "Could not confirm the new attempt. Refresh status before trying again; an export may already have started." :
        exportState === "accepted" ? "The host returned a new export attempt. View its progress in the session jobs." :
        "Try export again writes configuration to a fresh directory. Earlier attempts and files are retained, even if already complete. No server is connected.";
    }
    const stopRequest = stopRequests.get(job.id);
    byId("stop-status").textContent = job.stop?.status === "settled" ?
      `Local setup stopped. Files and any completed changes are retained; the ${acquisition(job) ? "acquisition" : isRecipeExport(job) ? "configuration export" : "installation"} outcome remains unconfirmed.` :
      job.stop?.status === "requested" ? "Stop requested. Waiting for this local job to settle; changes may already have occurred." :
      stopRequest === "pending" ? "Requesting a stop for this local job..." :
      stopRequest === "accepted" && job.status === "running" ? "The host accepted the stop request. Refreshing the job outcome..." :
      stopRequest === "retry_available" && job.status === "running" ? "The host still reports this job as running without a stop request. You can try Stop local setup again." :
      stopRequest === "unconfirmed" ? "Could not confirm the stop request. Refresh status; the job may still be running." : "";
    const acquired = acquisition(job);
    if (acquired) {
      const observation = viewedReconciliation()?.observation;
      const result = observation ? observation.status === "matched" ? observation.result : null : job.result;
      const packages = Boolean(acquired.artifact);
      byId("job-status").textContent = job.recovered ? "Recovered acquisition request. The historical acquisition outcome is unknown." :
        job.status === "running" ? `${acquired.label} is running in the local host.` :
        job.status === "succeeded" ? `${acquired.label} completed.` : `${acquired.label} is unconfirmed.`;
      byId("job-context").textContent = job.status === "running" ?
        "The host is acquiring the agent-selected inputs. Closing this page does not cancel host work. Status is refreshed from the host; detailed download progress is not reported." :
        job.status === "succeeded" ? packages ?
          "The host retained package inputs for a separate installation proposal. Acquisition does not install or start the selected software." :
          "The host observed selected images and prepared a proposal for review. Acquisition does not start containers or apply the proposed graph." :
        "Acquisition is unconfirmed. Preserve the host files and any cached images; completed changes are retained. No automatic retry or cleanup is performed.";
      if (job.recovered) byId("job-context").textContent =
        "Readback only. The host must ensure the earlier acquisition worker has exited before checking. This session cannot download, pull images, install or retry.";
      if (validRecoveryRequest(job)) byId("job-context").textContent +=
        ` Retained request ${job.recovery_request.id}, SHA-256 ${job.recovery_request.sha256}.`;
      if (viewedReconciliation()?.available) byId("reconciliation-status").textContent =
        "Check acquired files compares retained receipts and files with the original request. It does not download or pull images.";
      if (observation) {
        byId("reconciliation-status").textContent = observation.status === "checking" ? "Checking current acquired files..." :
          observation.status === "matched" ? "Current acquired files match the retained request. The historical job outcome above is unchanged." :
          "Current acquired files are unconfirmed. No download, image pull, repair or retry was performed.";
        description(byId("reconciliation-limits"), observation.excluded, "");
      }
      if (result) {
        byId("result-heading").textContent = packages ? "Retained package inputs" : "Compose proposal for host review";
        byId("result-description").textContent = packages ?
          "The acquisition receipt identifies the staged package files. The agent can use these inputs to prepare a separate installation proposal." :
          "The proposal receipt identifies the canonical plan and its retained inputs. The host must separately admit the exact proposal for execution.";
        byId("result-limits").textContent = "Acquisition does not install software or establish a usable tool, account access or native attachment. " +
          (observation ? "This check covers retained files only; it does not establish current image availability or successful historical execution." :
            "These are retained results of this job, not a current integrity or availability check.");
        const paths = byId("result-paths"), files = packages ? result.files : result.proposal.files;
        row(paths, "Acquisition job", job.id);
        row(paths, "Request", job.request_id);
        if (packages) {
          if (acquired.packageKey) {
            row(paths, "Package", `${result.request[acquired.packageKey]}@${result.request.version}`);
            row(paths, "Retained archives", String(result.artifacts.length));
          } else {
            row(paths, "Bundle", "MCPB archive");
            row(paths, "Bundle bytes", String(result.primary.byte_length));
            row(paths, "Bundle SHA-256", result.primary.sha256);
          }
          row(paths, "Staged inputs", result.staged_id);
          row(paths, "Acquisition receipt", result.receipt_path);
          row(paths, "Host profile SHA-256", result.host_profile_sha256);
        } else {
          row(paths, "Proposal directory", result.proposal_directory);
          row(paths, "Proposal receipt", `${result.proposal_directory}/proposal.json`);
          row(paths, "Plan SHA-256", result.proposal.plan_sha256);
        }
        row(paths, "Receipt SHA-256", result.receipt_sha256);
        byId("mcp-configuration").setAttribute("aria-label", "Retained acquisition file hashes");
        byId("mcp-configuration").textContent = JSON.stringify({ files }, null, 2);
        elements.result.hidden = false;
      }
      const changed = lastJob !== `${job.id}/${job.status}`; lastJob = `${job.id}/${job.status}`;
      if (moveFocus && changed && job.status !== "running" && result) focus("result-heading");
      return;
    }
    const kind = selectedKind(job);
    const action = kind ? `Selected ${SELECTED_LABELS[kind]} package installation` :
      isRecipeExport(job) ? "MCP configuration export" : job.kind === "prepare" ? "Library preparation" : "Installation";
    byId("job-status").textContent = state?.mode === "recovery" ? `Recovered ${isRecipeExport(job) ? "export" : "install"} request. The historical ${isRecipeExport(job) ? "export" : "installation"} outcome is unknown.` : job.status === "running" ? `${action} is running in the local host.` : job.status === "succeeded" ?
      `${action} completed.` : `${action} is unconfirmed. Inspect the host output before assuming completion or requesting another operation.`;
    byId("job-context").textContent = state?.mode === "recovery" ?
      "Readback only. The host must ensure no earlier export or installer is still running before checking. No setup or retry is available in this session." : job.status === "running" ?
      "Closing this page does not cancel host work. Leave it open for progress, or reopen the private launcher and refresh status." :
      isRecipeExport(job) ? job.status === "succeeded" ?
        "The host checked exported configuration files. Your MCP client still owns connection and authentication. The whole endpoint is exposed; this export applies no operation filter." :
        "Configuration export is unconfirmed. Preserve and inspect the host files; they may be absent, partial or complete. No connection or authentication is established." :
      "This observation describes local preparation or installation. It does not establish service access.";
    if (job.recovery_request && /^[0-9a-f]{32}$/.test(job.recovery_request.id) && /^[0-9a-f]{64}$/.test(job.recovery_request.sha256)) {
      byId("job-context").textContent += ` Retained request ${job.recovery_request.id}, SHA-256 ${job.recovery_request.sha256}.`;
    }
    if (job.retry_of) byId("job-context").textContent += ` New ${isRecipeExport(job) ? "export" : selectedKind(job) ? "installation" : "preparation"} after attempt ${job.retry_of}.`;
    const observation = viewedReconciliation()?.observation;
    let result = ["install", "export"].includes(job.kind) && job.status === "succeeded" ? job.result : null;
    if (job.kind === "install") {
      byId("reconciliation-status").textContent = !viewedReconciliation()?.available ?
        "Installation checking is unavailable. The host must explicitly select a trusted verifier before starting this session." :
        "Check installation compares current files with the original request. It does not retry installation.";
    }
    if (observation && observation.job_id === job.id && isRecipeExport(job)) {
      result = observation.status === "matched" ? observation.result.configuration : null;
      byId("reconciliation-status").textContent = observation.status === "checking" ? "Checking current exported files..." :
        observation.status === "matched" ? "Current exported files match the selected recipe. The historical export outcome above is unchanged." :
        "Current exported files are unconfirmed. No export, repair or retry was performed.";
      description(byId("reconciliation-limits"), observation.excluded, "");
    } else if (observation && observation.job_id === job.id) {
      result = observation.status === "matched" ? observation.result : null;
      byId("reconciliation-status").textContent = observation.status === "checking" ? "Checking current installation files..." :
        observation.status === "matched" ? "Current retained package files and launch settings match the original request. The historical job outcome above is unchanged." :
          "The current installation could not be confirmed. No repair or retry was performed. Preserve the host files for inspection.";
      description(byId("reconciliation-limits"), observation.excluded, "");
    }
    if (isRecipeExport(job) && result) {
      byId("result-heading").textContent = "Exported MCP configuration";
      byId("result-description").textContent = "The host read these configuration files back and matched the recipe export hashes. Load the configuration in your MCP client to connect and authenticate.";
      byId("result-limits").textContent = "This exports the whole hosted endpoint. No software installation, live connection, operation filtering or native agent attachment is established.";
      const paths = byId("result-paths");
      row(paths, "Recipe", `${result.recipe.identity.publisher}/${result.recipe.identity.name}@${result.recipe.identity.version}`);
      row(paths, "Recipe digest", result.recipe.digest);
      row(paths, "Current selection digest", result.selection_digest);
      row(paths, "Plugin metadata", result.plugin_path);
      row(paths, "MCP configuration", result.mcp_path);
      for (const [name, file] of Object.entries(result.files)) row(paths, `${name} SHA-256`, file.sha256);
      byId("mcp-configuration").textContent = JSON.stringify(result.mcp_configuration, null, 2);
      elements.result.hidden = false;
    } else if (result?.mcp_configuration && typeof result.mcp_configuration === "object" && !Array.isArray(result.mcp_configuration) &&
        typeof result.mcp_path === "string" && typeof result.receipt_path === "string") {
      byId("result-heading").textContent = "Installed MCP configuration";
      byId("result-description").textContent = "The host published these actual launch settings. Load them through your MCP client, which owns the process environment and credential injection.";
      byId("result-limits").textContent = "Installed software is not a connected account, a native agent binding or a completed user task. Provider authentication and operation approval remain with your host.";
      const paths = byId("result-paths"); paths.replaceChildren(); row(paths, "MCP configuration", result.mcp_path); row(paths, "Installation receipt", result.receipt_path);
      if (kind) {
        row(paths, "Selection type", `${SELECTED_LABELS[kind]} package`);
        row(paths, "Component", result.component);
        row(paths, "Selection SHA-256", result.selection_sha256);
        const digestKey = selectedDigestKey(result, kind);
        row(paths, digestKey === "declaration_digest" ? "Software declaration digest" : "Manifest digest", result[digestKey]);
        if (kind === "mcpb") row(paths, "Configuration SHA-256", result.configuration_sha256);
        row(paths, "Installation effect SHA-256", result.effect_sha256);
        row(paths, "Observed receipt SHA-256", result.receipt_sha256);
      } else if (observation?.status === "matched") {
        row(paths, "Checked profile", `${result.family} / ${result.profile}`);
        row(paths, "Observed receipt SHA-256", result.receipt_sha256);
      }
      if (!observation && result.family && result.profile) {
        row(paths, "Validated profile", `${result.family} / ${result.profile}`);
        row(paths, "Configuration SHA-256", result.configuration_sha256);
      }
      showReferences("result-credential-references", result, Boolean(observation));
      byId("mcp-configuration").textContent = JSON.stringify(result.mcp_configuration, null, 2); elements.result.hidden = false;
    }
    const changed = lastJob !== `${job.id}/${job.status}`; lastJob = `${job.id}/${job.status}`;
    if (moveFocus && changed && job.status !== "running" && result) focus("result-heading");
  }
  function scheduleRefresh(delay) {
    if (disposed || !session) return;
    if (timer !== null) clear(timer);
    timer = later(() => { timer = null; void refresh(true); }, delay);
  }
  function drawHistory() {
    byId("job-history").hidden = !hasHistory(state);
    const choice = byId("job-choice");
    choice.replaceChildren();
    const current = node("option", recoverySelection(state) ? "Last request in recovery selection" : "Current job"); current.value = ""; choice.append(current);
    for (const summary of historyRows.values()) {
      if (summary.id === state?.job?.id && summary.id !== selectedJobId) continue;
      const label = acquisition(summary)?.label ?? (isCompose(summary) ? `Compose: ${COMPOSE_ACTIONS[summary.action]}` : `${summary.family}${summary.profile ? " / " + summary.profile : ""}: ${summary.kind}`);
      const option = node("option", `${label}, ${recoverySelection(state) ? "historical outcome unknown" : summary.status} (${summary.id.slice(0, 8)})`);
      option.value = summary.id; choice.append(option);
    }
    choice.value = selectedJobId ?? "";
    byId("current-job-status").textContent = recoverySelection(state) ?
      `${state.job_history.count} host-selected requests, shown in reverse selection order. Check each result independently; historical outcomes remain unknown.` : state?.job ?
      `Current job: ${acquisition(state.job)?.label ?? (isCompose(state.job) ? `Compose, ${COMPOSE_ACTIONS[state.job.action]}` : `${state.job.family}, ${state.job.kind}`)}, ${state.job.status}.` : "No current job.";
    byId("jobs-earlier").hidden = historyBefore === null;
  }
  function updateSummary(job) {
    const prior = historyRows.get(job?.id);
    if (!prior) return;
    if (prior.family !== job.family || prior.kind !== job.kind ||
        recoverySelection(state) && (!recoveredJob(job) || !sameRecovery(job, prior)) ||
        isCompose(job) && ["action", "request_id", "stop_supported"].some(key => job[key] !== prior[key]) ||
        job.result?.profile !== undefined && prior.profile !== job.result.profile) throw new RequestError("invalid_job_history_binding");
    historyRows.set(job.id, { ...prior, status: job.status });
  }
  async function readHistory(generation) {
    if (!hasHistory(state)) {
      selectedJobId = null; jobView = null; historyRows.clear(); historyCount = null; historyBefore = null;
    } else if (historyCount !== state.job_history.count) {
      const page = await post("/api/jobs", { before: null });
      if (disposed || generation !== refreshGeneration) return false;
      if (!validJobPage(page, state, null)) throw new RequestError("invalid_job_history");
      const selected = historyRows.get(selectedJobId);
      for (const summary of page.jobs) {
        const prior = historyRows.get(summary.id);
        if (prior && (["family", "kind", "profile", ...(isCompose(summary) ? ["action", "request_id", "stop_supported"] : [])].some(key => prior[key] !== summary[key]) ||
            recoverySelection(state) && !sameRecovery(summary, prior))) throw new RequestError("invalid_job_history_binding");
      }
      historyRows = new Map(page.jobs.map(job => [job.id, job]));
      if (selected && !historyRows.has(selected.id)) historyRows.set(selected.id, selected);
      historyCount = state.job_history.count; historyBefore = page.next_before;
    }
    if (selectedJobId) {
      const summary = historyRows.get(selectedJobId);
      if (!summary) throw new RequestError("unknown_job_view");
      const value = await post("/api/job", { job_id: selectedJobId });
      if (disposed || generation !== refreshGeneration) return false;
      if (!validJobDetail(value, summary) || isCompose(value.job) && !composeJobBinding(value.job, state) ||
          acquisition(value.job) && (!acquisitionJobBinding(value.job, state) ||
            value.reconciliation.observation?.status === "matched" &&
            !acquisitionJobBinding({ ...value.job, result: value.reconciliation.observation.result }, state))) throw new RequestError("invalid_job_view");
      checkAcquisitionBinding(value.job, state);
      checkComposeResultBinding(value.job);
      const observed = value.reconciliation.observation, prior = observations.get(value.job.id);
      if (prior && (!validObservation(prior, value.job) || acquisition(value.job) && prior.status === "matched" &&
          !acquisitionJobBinding({ ...value.job, result: prior.result }, state))) throw new RequestError("invalid_observation_binding");
      if (observed && (!prior || observed.revision > prior.revision ||
          observed.revision === prior.revision && prior.status === "checking")) observations.set(value.job.id, observed);
      else if (prior) value.reconciliation.observation = prior;
      jobView = value;
      updateSummary(value.job);
    }
    updateSummary(state.job);
    drawHistory(); return true;
  }
  function checkAcquisitionBinding(job, host) {
    if (!acquisition(job)) return;
    const binding = acquisitionBindingKey(job, host), previous = acquisitionBindings.get(job.id);
    if (previous !== undefined && previous !== binding) throw new RequestError("invalid_acquisition_binding");
    acquisitionBindings.set(job.id, binding);
  }
  function checkComposeResultBinding(job) {
    if (!isCompose(job) || !job.result) return;
    const value = job.result.backend;
    const identity = JSON.stringify([job.action, job.request_id, value.status, composeResultPlan(value), value.transition_sha256,
      value.successor?.plan_sha256, value.summary?.profile_sha256, value.predecessor?.summary.profile_sha256,
      value.successor?.summary.profile_sha256, value.selected?.summary.profile_sha256]);
    const previous = composeResultBindings.get(job.id);
    if (previous !== undefined && previous !== identity) throw new RequestError("invalid_compose_result_binding");
    composeResultBindings.set(job.id, identity);
  }
  async function earlierJobs() {
    if (historyLoading || historyBefore === null || !available) return;
    const before = historyBefore, generation = refreshGeneration;
    historyLoading = true; locks();
    try {
      const page = await post("/api/jobs", { before });
      if (disposed || generation !== refreshGeneration || before !== historyBefore) return;
      if (!validJobPage(page, state, before) || page.jobs.some(job => historyRows.has(job.id) && job.id !== selectedJobId)) throw new RequestError("invalid_job_history");
      if (recoverySelection(state) && page.jobs.some(job => {
        const prior = historyRows.get(job.id);
        return prior && (["kind", "family", "profile"].some(key => job[key] !== prior[key]) || !sameRecovery(job, prior));
      })) throw new RequestError("invalid_job_history_binding");
      for (const job of page.jobs) historyRows.set(job.id, job);
      historyBefore = page.next_before; drawHistory();
    } catch (error) {
      if (!disposed && generation === refreshGeneration) problem(describeFailure(error, "Could not read earlier jobs. No work was started."));
    } finally { historyLoading = false; if (!disposed) locks(); }
  }
  async function refresh(background = false, inRequest = false, moveFocus = !background) {
    if (disposed || !session) return;
    if (timer !== null) { clear(timer); timer = null; }
    // A second surface can start work while this page is idle. Observe it
    // without competing with this page's in-flight selection or validation.
    if (background && pending && !inRequest) { scheduleRefresh(1000); return; }
    const generation = ++refreshGeneration;
    const unconfirmedStops = new Set([...stopRequests].filter(([, status]) => status === "unconfirmed").map(([id]) => id));
    const unconfirmedRetries = new Set([...preparationRetries].filter(([, status]) => status === "unconfirmed").map(([id]) => id));
    const unconfirmedExports = new Set([...exportRetries].filter(([, status]) => status === "unconfirmed").map(([id]) => id));
    try {
      const value = await post("/api/state", {});
      if (disposed || generation !== refreshGeneration) return;
      if (!validState(value)) throw new RequestError("invalid_state");
      if (recoverySelection(state) && (!recoverySelection(value) || value.job_history.count !== state.job_history.count ||
          ["id", "kind", "family"].some(key => value.job[key] !== state.job[key]) || !sameRecovery(value.job, state.job))) throw new RequestError("invalid_recovery_selection");
      checkAcquisitionBinding(value.job, value);
      checkComposeResultBinding(value.job);
      if (state?.job && value.job?.id === state.job.id && (isCompose(state.job) || isCompose(value.job)) &&
          ["kind", "family", "action", "request_id", "stop_supported"].some(key => state.job[key] !== value.job[key])) {
        throw new RequestError("invalid_job_binding");
      }
      const observed = value.reconciliation?.observation, prior = observations.get(value.job?.id);
      if (prior && (!validObservation(prior, value.job) || acquisition(value.job) && prior.status === "matched" &&
          !acquisitionJobBinding({ ...value.job, result: prior.result }, value))) throw new RequestError("invalid_observation_binding");
      if (observed && (!prior || observed.revision > prior.revision ||
          (observed.revision === prior.revision && prior.status === "checking"))) observations.set(value.job.id, observed);
      else if (prior && value.reconciliation) value.reconciliation.observation = prior;
      if ((priorSelection || priorReview) && value.prior_selection?.request_sha256 !== (priorSelection ?? priorReview).request_sha256) cancelRoute();
      state = value;
      if (!await readHistory(generation)) return;
      for (const job of [state.job, jobView?.job]) {
        if (job && unconfirmedStops.has(job.id) && stopRequests.get(job.id) === "unconfirmed" &&
            job.status === "running" && !job.stop) stopRequests.set(job.id, "retry_available");
        if (job && unconfirmedRetries.has(job.id) && preparationRetries.get(job.id) === "unconfirmed" &&
            retryablePreparation(job)) preparationRetries.set(job.id, "retry_available");
        if (job && unconfirmedExports.has(job.id) && exportRetries.get(job.id) === "unconfirmed" &&
            retryableExport(job)) exportRetries.set(job.id, "retry_available");
      }
      available = true; drawPriorAvailability(); byId("connection-status").textContent =
        "Connected to the local setup host." + (composeHostStatus(state.compose, state.job) ? " " + composeHostStatus(state.compose, state.job) : "");
      headerDescription.textContent = state.compose ?
        "This host also permits agent-selected local container execution. Container actions are requested through setup tools and observed here. " + setupDescription : setupDescription;
      document.querySelector("footer").textContent = state.compose ?
        "Local setup and host-authorized container execution. Compose observations do not establish account access or application readiness." :
        "Local consumer setup. No provider login or service operation is performed here.";
      if (!background) problem(""); families(); showJob(viewedJob(), moveFocus);
      await loadCatalog();
      await continueRoute();
      await continuePrior();
      if (!disposed && generation === refreshGeneration) scheduleRefresh(running() ? 1000 : 5000);
    } catch (error) {
      if (!disposed && generation === refreshGeneration) {
        cancelRoute(); available = false; byId("connection-status").textContent = "Host status is unconfirmed.";
        if (selectedJobId || isRecipeExport(viewedJob()) || isCompose(viewedJob()) || acquisition(viewedJob())) { jobView = null; showJob(null, false); }
        problem(describeFailure(error, "Could not confirm host status. Existing host work may still be running. Use Refresh status; do not assume the operation stopped.")); locks();
      }
    }
  }
  async function startJob(kind) {
    if (state?.mode === "recovery") return;
    if (pending || running() || !available) return;
    let payload;
    if (kind === "prepare") {
      if (selectedFamily()?.status !== "unprepared") return;
      payload = { family: elements.family.value };
    } else {
      if (!validation) return;
      try { if (JSON.stringify(draft()) !== JSON.stringify(validation.draft)) { invalidate(); return; } }
      catch { invalidate(); return; }
      payload = { ...validation.draft, validation_id: validation.validation_id, configuration_sha256: validation.configuration_sha256 };
    }
    const selectedRoute = routeSelection, selectedPrior = priorSelection;
    const request = { kind }; pending = request; problem(""); locks();
    try {
      const result = await post(`/api/${kind}`, payload);
      if (disposed) return;
      if (selectedRoute && selectedRoute !== routeSelection || selectedPrior && selectedPrior !== priorSelection) return;
      if (!validJob(result.job) || result.job.kind !== kind || result.job.family !== payload.family) throw new RequestError("invalid_job");
      selectedJobId = null; jobView = null; state.job = result.job; showJob(state.job); focus("job-heading");
      if (kind === "install") { validation = null; elements.review.hidden = true; }
      await refresh();
    } catch (error) {
      if (!disposed) { cancelRoute(); available = false; problem(describeFailure(error, "The host response is unconfirmed. Refresh status to inspect existing work before attempting another installation.")); }
    } finally { if (!disposed && pending === request) { pending = null; locks(); } }
    if (selectedRoute && selectedRoute === routeSelection) await continueRoute();
    if (selectedPrior && selectedPrior === priorSelection) await continuePrior();
  }

  async function reconcile() {
    if (pending || running() || !available || historyLoading || !viewedReconciliation()?.available || !["install", "export", "acquire"].includes(viewedJob()?.kind)) return;
    const job = viewedJob(), jobId = job.id, request = { kind: "reconcile" };
    pending = request; refreshGeneration += 1; problem(""); locks();
    elements.result.hidden = true;
    byId("reconciliation-status").textContent = acquisition(job) ? "Checking current acquired files..." : isRecipeExport(job) ? "Checking current exported files..." : "Checking current installation files...";
    try {
      const value = await post("/api/reconcile", { job_id: jobId });
      if (disposed) return;
      if (!validObservation(value.observation, job) || acquisition(job) && value.observation.status === "matched" &&
          !acquisitionJobBinding({ ...job, result: value.observation.result }, state)) throw new RequestError("invalid_observation");
      const prior = observations.get(jobId);
      if (!prior || value.observation.revision > prior.revision ||
          (value.observation.revision === prior.revision && prior.status === "checking")) observations.set(jobId, value.observation);
      // Read current host state instead of attaching a delayed response to a
      // different job that became current while this request was outstanding.
      await refresh();
    } catch (error) {
      if (!disposed) {
        available = false;
        problem(describeFailure(error, `The ${acquisition(job) ? "acquisition" : isRecipeExport(job) ? "export" : "installation"} check response is unconfirmed. Refresh status to observe the existing check; no download, export or installation was retried.`));
      }
    } finally { if (!disposed && pending === request) { pending = null; locks(); } }
  }

  async function stopJob() {
    const job = viewedJob();
    if (disposed || !available || pending || historyLoading || state?.mode === "recovery" ||
        !state?.stop?.available || !job || job.stop_supported === false || job.status !== "running" || job.stop || stopAwaitingObservation(job.id)) return;
    const jobId = job.id, request = { kind: "stop", jobId };
    stopRequests.set(jobId, "pending"); pending = request;
    refreshGeneration += 1; cancelRoute(); problem(""); showJob(job, false); locks();
    try {
      const value = await post("/api/stop", { job_id: jobId });
      if (disposed) return;
      if (!validJob(value?.job) || ["id", "family", "kind"].some(key => value.job[key] !== job[key]) ||
          (value.job.status === "running" && value.job.stop?.status !== "requested")) throw new RequestError("invalid_stop");
      // Read from the owner rather than applying a delayed response to another job.
      stopRequests.set(jobId, "accepted");
      await refresh();
    } catch (error) {
      if (!disposed) {
        stopRequests.set(jobId, "unconfirmed");
        if (viewedJob()?.id === jobId) {
          showJob(viewedJob(), false);
          problem(describeFailure(error, "Could not confirm the stop request. Refresh status; no stop or rollback is assumed."));
        }
      }
    } finally {
      if (!disposed && pending === request) { pending = null; locks(); }
    }
  }

  async function reviewSelectedRetry() {
    const job = viewedJob();
    if (disposed || !available || pending || running() || historyLoading || state?.mode === "recovery" ||
        !state?.selected_retry?.available || !retryableSelected(job)) return;
    const request = { kind: "selected_retry", jobId: job.id };
    selectedRetries.set(job.id, { phase: "pending" }); pending = request;
    refreshGeneration += 1; cancelRoute(); problem(""); drawSelectedRetry(job); locks();
    try {
      const value = await post("/api/selected_retry", { job_id: job.id });
      if (disposed) return;
      if (!validSelectedRetry(value, job)) throw new RequestError("invalid_retry_proposal");
      selectedRetries.set(job.id, { phase: value.job_id ? "started" : "review", proposal: value });
      if (viewedJob()?.id === job.id) {
        drawSelectedRetry(job);
        if (!value.job_id) focus("selected-retry-heading");
      }
      // Refresh also discovers a job started on another surface. No install
      // call follows proposal creation or recovery of a lost proposal reply.
      await refresh(true, true);
    } catch (error) {
      if (!disposed) {
        selectedRetries.set(job.id, { phase: "unconfirmed" });
        if (viewedJob()?.id === job.id) {
          drawSelectedRetry(job);
          problem(describeFailure(error, "Could not confirm the new proposal. Review this attempt again to retrieve it. No installation was requested."));
        }
      }
    } finally { if (!disposed && pending === request) { pending = null; locks(); } }
  }

  async function installSelectedRetry() {
    const job = viewedJob(), retry = selectedRetries.get(job?.id);
    if (disposed || !available || pending || running() || historyLoading || state?.mode === "recovery" ||
        !state?.selected_retry?.available || !retryableSelected(job) || retry?.phase !== "review" ||
        retry.proposal.job_id !== null) return;
    const proposal = retry.proposal;
    const request = { kind: "selected_retry_install", jobId: job.id };
    pending = request; refreshGeneration += 1; cancelRoute(); problem(""); locks();
    try {
      const value = await post("/api/selected_install", { validation_id: proposal.validation_id, effect_sha256: proposal.effect_sha256 });
      if (disposed) return;
      if (!validJob(value?.job) || value.job.family !== job.family || value.job.retry_of !== job.id || value.job.id === job.id ||
          value.job.status === "succeeded" && value.job.result.effect_sha256 !== proposal.effect_sha256)
        throw new RequestError("invalid_retry_install");
      selectedRetries.set(job.id, { phase: "started", proposal: { ...proposal, job_id: value.job.id } });
      if (viewedJob()?.id === job.id) {
        if (state.job_history?.available) {
          historyRows.set(value.job.id, { id: value.job.id, kind: value.job.kind, family: value.job.family, status: value.job.status, profile: null });
          selectedJobId = value.job.id;
          jobView = { job: value.job, reconciliation: { available: false, observation: null } };
        } else { state.job = value.job; selectedJobId = null; jobView = null; }
        showJob(viewedJob());
      }
      await refresh();
    } catch (error) {
      if (!disposed) {
        selectedRetries.set(job.id, { phase: "unconfirmed" });
        if (viewedJob()?.id === job.id) {
          drawSelectedRetry(job);
          problem(describeFailure(error, "Could not confirm installation. Refresh status and review this attempt again; the host may already have admitted it. No automatic retry was issued."));
        }
      }
    } finally { if (!disposed && pending === request) { pending = null; locks(); } }
  }

  async function retryAttempt(kind) {
    const job = viewedJob();
    const exporting = kind === "export", retries = exporting ? exportRetries : preparationRetries;
    const capability = exporting ? state?.export_retry : state?.preparation_retry;
    const retryable = exporting ? retryableExport : retryablePreparation;
    const label = exporting ? "export" : "preparation";
    if (disposed || !available || pending || running() || historyLoading || state?.mode === "recovery" ||
        !capability?.available || !retryable(job) ||
        ["pending", "accepted", "unconfirmed"].includes(retries.get(job.id))) return;
    const request = { kind: `retry_${kind}`, jobId: job.id };
    retries.set(job.id, "pending"); pending = request;
    refreshGeneration += 1; cancelRoute(); problem(""); showJob(job, false); locks();
    try {
      const value = await post(`/api/retry_${kind}`, { job_id: job.id });
      if (disposed) return;
      if (!validJob(value?.job) || value.job.kind !== kind || value.job.family !== job.family ||
          exporting && value.job.request_sha256 !== job.request_sha256 ||
          !/^[0-9a-f]{32}$/.test(value.job.id) || value.job.retry_of !== job.id || value.job.id === job.id) throw new RequestError("invalid_retry");
      retries.set(job.id, "accepted");
      // A user may have selected another history item while this request ran.
      // Follow the returned attempt by identity, even if another surface has
      // already made an unrelated job current. Do not steal a changed view.
      if (viewedJob()?.id === job.id) {
        if (state.job_history?.available) {
          historyRows.set(value.job.id, { id: value.job.id, kind: value.job.kind,
            family: value.job.family, status: value.job.status, profile: null });
          selectedJobId = value.job.id;
          jobView = { job: value.job, reconciliation: { available: false, observation: null } };
        } else { selectedJobId = null; jobView = null; }
      }
      await refresh();
    } catch (error) {
      if (!disposed) {
        retries.set(job.id, "unconfirmed");
        if (viewedJob()?.id === job.id) {
          showJob(viewedJob(), false);
          problem(describeFailure(error, `Could not confirm the new ${label}. Refresh status; no automatic retry was issued.`));
        }
      }
    } finally {
      if (!disposed && pending === request) { pending = null; locks(); }
    }
  }

  on(elements.family, "change", () => { cancelRoute(); resetHelp(); elements.profile.value = ""; families(); problem(""); });
  on(byId("catalog-filter"), "input", filterCatalog);
  on(byId("catalog-choice"), "change", () => { void loadCatalogEntry(); });
  on(byId("catalog-entry-retry"), "click", () => {
    if (entryRetry && currentCatalogEntry(entryRetry) && !entryRequest) void loadCatalogEntry();
  });
  on(elements.profile, "change", () => { cancelRoute(); resetHelp(); problem(""); });
  on(byId("show-profile"), "click", () => { void loadHelp(); });
  on(byId("reuse-prior"), "click", () => { void reusePrior(); });
  on(elements["configuration-form"], "input", event => { if (priorReview && inputs.has(event.target.name)) priorReview.edited.add(event.target.name); if (typeof event.target.setCustomValidity === "function") event.target.setCustomValidity(""); invalidate(); });
  on(elements["configuration-form"], "change", () => { invalidate(); });
  on(elements["configuration-form"], "submit", event => { void validate(event); });
  on(elements.prepare, "click", () => { void startJob("prepare"); });
  on(elements.install, "click", () => { void startJob("install"); });
  on(elements.reconcile, "click", () => { void reconcile(); });
  on(elements["stop-job"], "click", () => { void stopJob(); });
  on(elements["retry-prepare"], "click", () => { void retryAttempt("prepare"); });
  on(elements["retry-export"], "click", () => { void retryAttempt("export"); });
  on(elements["retry-selected"], "click", () => { void reviewSelectedRetry(); });
  on(elements["install-selected-retry"], "click", () => { void installSelectedRetry(); });
  on(elements.refresh, "click", () => { void refresh(); });
  on(byId("recovery-add"), "click", addRecovery);
  on(byId("recovery-save"), "click", () => { void saveRecovery(); });
  on(byId("recovery-selection"), "click", event => {
    const button = event.target.closest?.("button[data-recovery-id]");
    if (disposed || recoveryRequest || !button || !byId("recovery-selection").contains(button)) return;
    recoveryEntries.delete(button.dataset.recoveryId); recoveryConflicts.delete(button.dataset.recoveryId);
    byId("recovery-status").textContent = `${recoveryEntries.size} request${recoveryEntries.size === 1 ? "" : "s"} selected.`;
    drawRecovery(); focus(recoveryEntries.size ? "recovery-save" : "recovery-add");
  });
  on(byId("job-choice"), "change", () => {
    selectedJobId = byId("job-choice").value || null; jobView = null;
    // Reading a selection must preserve keyboard browsing and any later focus
    // choice, while retaining foreground status and error handling.
    showJob(null, false); locks(); void refresh(false, false, false);
  });
  on(byId("jobs-earlier"), "click", () => { void earlierJobs(); });
  return {
    async start() {
      if (!session) { byId("connection-status").textContent = "Open the host's private launcher to begin."; problem("This page has no setup session. Use the private launcher created by your local setup host."); locks(); return; }
      await refresh();
    },
    dispose() { cancelRoute(); clearCatalogEntry(); disposed = true; recoveryRequest = null; revokeRecoveryURL(); recoveryEntries.clear(); recoveryConflicts.clear(); helpGeneration += 1; refreshGeneration += 1; catalogGeneration += 1; if (timer !== null) clear(timer); for (const wait of responseWaits) { clear(wait.timer); wait.expire(); } responseWaits.clear(); removers.forEach(remove => remove()); session = null; },
  };
}

if (typeof window !== "undefined" && typeof document !== "undefined") {
  const workbench = createWorkbench(document);
  void workbench.start();
}
