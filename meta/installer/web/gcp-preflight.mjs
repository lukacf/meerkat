import { deploymentCanPrepare } from "./deployment-panel.mjs";

const HASH = /^[a-f0-9]{64}$/;
const PROJECT = /^[a-z][a-z0-9-]{4,28}[a-z0-9]$/;
const BINDING_KEYS = ["guide_id", "guide_version", "build_id", "blueprint_hash", "source_digest", "package_sha256"];
export const GCP_PREFLIGHT_FRESH_MS = 5 * 60 * 1000;

export function gcpCheckFresh(response, now = Date.now()) {
  const checked = Date.parse(response?.checked_at);
  return Number.isFinite(checked) && checked <= now + 1000 && now - checked < GCP_PREFLIGHT_FRESH_MS;
}

export function gcpPreflightAvailable(state, build, blocked = false) {
  return !blocked && (state?.proposal || state?.blueprint)?.target === "gcp" &&
    (state?.proposal || state?.blueprint)?.gcp?.provisioning !== "existing_host" &&
    deploymentCanPrepare(build, state) && HASH.test(build?.blueprint_hash || "") && HASH.test(build?.source_digest || "");
}

/** Include the actual proposal and source so a reused version cannot revive an old result. */
export function gcpPreflightScope(state, build, blocked = false) {
  return JSON.stringify([state?.id, state?.version, build?.build_id, build?.blueprint_hash,
    build?.source_digest, build?.package_sha256, state?.proposal || state?.blueprint,
    gcpPreflightAvailable(state, build, blocked), !!blocked]);
}

/** Selection revisions are monotonic. Going A -> B -> A never revives A's request. */
export function createGcpPreflightGate() {
  let scope = "", epoch = 0, revision = 0, live = true;
  return {
    observe(next) { if (scope !== next) { scope = next; epoch += 1; revision += 1; } return revision; },
    invalidate() { epoch += 1; return epoch; },
    capture() { return { scope, epoch, revision }; },
    current(token) { return live && !!token && token.scope === scope && token.epoch === epoch; },
    open() { live = true; },
    close() { live = false; epoch += 1; },
  };
}

export function gcpPreflightUrl(guideId, action = "") {
  return `/api/guide/${encodeURIComponent(guideId)}/gcp-preflight${action ? `/${action}` : ""}`;
}

export function gcpBindingMatches(response, state, build, expected = null) {
  const binding = response?.binding;
  return response?.protocol === 1 && gcpPreflightAvailable(state, build) && binding?.guide_id === state.id &&
    binding.guide_version === state.version && binding.build_id === build.build_id &&
    binding.blueprint_hash === build.blueprint_hash && binding.source_digest === build.source_digest &&
    HASH.test(binding.package_sha256 || "") && (!build.package_sha256 || binding.package_sha256 === build.package_sha256) &&
    (!expected || BINDING_KEYS.every((key) => binding[key] === expected[key])) &&
    typeof response.checked_at === "string" && Number.isFinite(Date.parse(response.checked_at)) &&
    response.destination?.project === (state.proposal || state.blueprint)?.gcp?.project;
}

export function gcpChecksValid(checks) {
  return Array.isArray(checks) && checks.every((check) => check && typeof check.id === "string" &&
    typeof check.label === "string" && typeof check.detail === "string" &&
    ["checked", "blocked", "unknown"].includes(check.status) && typeof check.required === "boolean" &&
    (check.action === undefined || typeof check.action === "string")) &&
    new Set(checks.map((check) => check.id)).size === checks.length;
}

export function gcpOptionsValid(response) {
  return response?.cli && typeof response.cli.available === "boolean" &&
    ["checked", "blocked", "unknown"].includes(response.cli.status) && typeof response.cli.detail === "string" &&
    (response.cli.action === undefined || typeof response.cli.action === "string") &&
    Array.isArray(response.accounts) && response.accounts.every((account) => typeof account.id === "string" && account.id &&
      typeof account.label === "string" && typeof account.active === "boolean") &&
    new Set(response.accounts.map((account) => account.id)).size === response.accounts.length &&
    gcpChecksValid(response.checks);
}

export function gcpProjectsValid(response, account) {
  return response.account === account && Array.isArray(response.projects) && typeof response.truncated === "boolean" &&
    response.projects.every((project) => PROJECT.test(project?.id || "") && typeof project.label === "string") &&
    new Set(response.projects.map((project) => project.id)).size === response.projects.length && gcpChecksValid(response.checks);
}

export function gcpCheckValid(response, account, project) {
  if (response.account !== account || response.project !== project || response.provisioning_verified !== false ||
      typeof response.ready_for_plan !== "boolean" || !gcpChecksValid(response.checks)) return false;
  if (!response.ready_for_plan) return response.plan === null;
  const plan = response.plan;
  return response.checks.some((check) => check.required) && response.checks.every((check) => !check.required || check.status === "checked") &&
    plan && typeof plan.title === "string" && plan.account === account && project === response.destination?.project &&
    ["project", "zone", "machine_type", "instance", "console_port", "disk_gb"].every((key) =>
      plan.destination?.[key] === response.destination?.[key]) &&
    ["resources", "access", "mutations", "commands", "limitations"].every((key) =>
      Array.isArray(plan[key]) && plan[key].every((value) => typeof value === "string"));
}

export function gcpProjectFeedbackCurrent(expected, state, build, blocked = false) {
  return !!expected && gcpPreflightAvailable(state, build, blocked) &&
    expected.scope === gcpPreflightScope(state, build, blocked) && PROJECT.test(expected.project || "") &&
    expected.project !== (state.proposal || state.blueprint).gcp?.project;
}

/** The only blueprint edit introduced by a cloud destination choice. */
export function gcpProjectBlueprint(base, project) {
  if (base?.target !== "gcp" || !PROJECT.test(project || "")) return null;
  const next = structuredClone(base);
  next.gcp = { ...next.gcp, project };
  return next;
}

export function gcpProjectBlueprintMatches(base, checked, project) {
  const expected = gcpProjectBlueprint(base, project);
  const canonical = (value) => Array.isArray(value) ? value.map(canonical) : value && typeof value === "object"
    ? Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])])) : value;
  return !!expected && JSON.stringify(canonical(expected)) === JSON.stringify(canonical(checked));
}
