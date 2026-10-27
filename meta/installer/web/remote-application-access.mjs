import { remoteBrowserBinding, remoteBrowserOwnedBinding, remoteBrowserReplyBindingOwned, remoteBrowserReplyBindingCurrent,
  remoteBrowserProductNativeReady, remoteBrowserObservedTime, REMOTE_BROWSER_FRESH_MS } from "./remote-browser-access.mjs";
import { artifactVersionCurrent } from './artifact-current.mjs';
import { remoteProductSelected } from './remote-deployment.mjs';

const HASH = /^[a-f0-9]{64}$/;
const UUID = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const ID = /^[a-z][a-z0-9_-]{0,63}$/;
const STATUSES = new Set(["closed", "connecting", "ready", "blocked", "unavailable", "cleanup-needed"]);
const port = (value) => Number.isInteger(value) && value >= 1024 && value <= 65535;
const digest = (value) => typeof value === "string" && HASH.test(value);
const uuid = (value) => typeof value === "string" && UUID.test(value);
const text = (value, maximum) => typeof value === "string" && value.trim() && value.length <= maximum && !/[\x00-\x1f\x7f]/.test(value);

export function remoteApplicationPath(value) {
  if (typeof value !== "string" || !value.startsWith("/") || value.startsWith("//") || value.length > 1000) return false;
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

export function remoteApplicationExperiences(receipt) {
  const value = receipt?.application_experiences;
  const unavailable = { supported: true, status: "unavailable", metadata_sha256: null, entries: [] };
  if (value == null || value.supported === false)
    return { supported: false, status: "unsupported", metadata_sha256: null, entries: [] };
  if (value.protocol !== 1 || value.supported !== true || !["resolved", "none", "unavailable"].includes(value.status) ||
      !digest(value.source_digest) || value.source_digest !== receipt.accepted_source_digest ||
      !digest(value.installed_source_digest) || value.installed_source_digest !== receipt.transferred_source_digest ||
      !Array.isArray(value.entries) || value.entries.length > 12 ||
      value.metadata_sha256 !== null && !digest(value.metadata_sha256)) return unavailable;
  if (value.status === "unavailable") return unavailable;
  if (value.status === "none") return value.entries.length === 0 && value.metadata_sha256 === null
    ? { supported: true, status: "none", metadata_sha256: null, entries: [] } : unavailable;
  if (!digest(value.metadata_sha256) || value.entries.length === 0) return unavailable;
  const ids = new Set(), entries = [];
  for (const entry of value.entries) {
    if (!entry || typeof entry.id !== "string" || !ID.test(entry.id) || ids.has(entry.id) || !text(entry.label, 160) ||
        !remoteApplicationPath(entry.path) || entry.verified !== false || typeof entry.listener_owned !== "boolean" ||
        entry.destination_port !== null && !port(entry.destination_port) ||
        entry.listener_token !== null && !digest(entry.listener_token) ||
        entry.reason !== null && !text(entry.reason, 500) ||
        entry.listener_owned && (!port(entry.destination_port) || !digest(entry.listener_token))) return unavailable;
    ids.add(entry.id);
    entries.push({ id: entry.id, label: entry.label.trim(), path: entry.path, destination_port: entry.destination_port,
      listener_owned: entry.listener_owned, listener_token: entry.listener_token, reason: entry.reason, verified: false });
  }
  return { supported: true, status: "resolved", metadata_sha256: value.metadata_sha256, entries };
}

export function remoteApplicationEntryId(entry) { return `application:${entry.id}`; }

export function remoteApplicationUrl(guideId, deploymentId, entry, action = "") {
  const base = `/api/guide/${encodeURIComponent(guideId)}/remote-deployment/browser-access`;
  return action ? `${base}/${action}`
    : `${base}?${new URLSearchParams({ deployment_id: deploymentId, entry_id: remoteApplicationEntryId(entry) })}`;
}

// Listener changes withdraw readiness but keep exact admitted connection custody
// inspectable while its cleanup is unconfirmed.
export function remoteApplicationBinding(receipt, state, build, experiences, entry) {
  return JSON.stringify([remoteBrowserBinding(receipt, state, build), experiences.metadata_sha256,
    entry.id, entry.path, entry.destination_port,
    ...(remoteProductSelected(receipt) ? [entry.listener_token] : [])]);
}

export function remoteApplicationOwnedBinding(receipt, state, build, experiences, entry) {
  if (!remoteProductSelected(receipt)) return remoteApplicationBinding(receipt, state, build, experiences, entry);
  const owner = remoteBrowserOwnedBinding(receipt, state);
  return owner ? JSON.stringify([owner, experiences.metadata_sha256, entry.id, entry.path]) : "";
}

export function remoteApplicationAddress(entry) {
  if (!port(entry?.destination_port) || !remoteApplicationPath(entry.path)) return null;
  try {
    const path = encodeURI(entry.path).replace(/%25([0-9a-f]{2})/gi, "%$1");
    const value = `http://127.0.0.1:${entry.destination_port}${path}`;
    const url = new URL(value);
    return url.href === value && url.hostname === "127.0.0.1" && !url.username && !url.password && !url.search && !url.hash
      ? value : null;
  } catch { return null; }
}

export function remoteApplicationReplyOwned(value, receipt, state, build, experiences, entry) {
  const product = remoteProductSelected(receipt);
  if (!value || !receipt || !state || !build || !experiences || !entry || value.protocol !== 1 ||
      !product && !artifactVersionCurrent(value, state) || value.guide_id !== receipt.guide_id ||
      value.guide_version !== receipt?.guide_version || !product && value.build_id !== build?.build_id || value.build_id !== receipt?.build_id ||
      value.deployment_id !== receipt?.deployment_id || value.entry_id !== remoteApplicationEntryId(entry) ||
      !STATUSES.has(value.status) || typeof value.native_ready !== "boolean" || typeof value.cleanup_confirmed !== "boolean" ||
      typeof value.transport_reachable !== "boolean" || value.application_verified !== false ||
      value.authentication_verified !== false || value.workflow_verified !== false ||
      value.scope?.health_scope !== "declared-application-transport" || value.scope.custom_interfaces_verified !== false ||
      remoteBrowserObservedTime(value.observed_at) === null || value.access_id !== null && !uuid(value.access_id) ||
      value.metadata_sha256 !== experiences.metadata_sha256 || !digest(value.metadata_sha256) ||
      value.entry_path !== entry.path || !product && value.destination_port !== entry.destination_port ||
      product && value.destination_port !== null && !port(value.destination_port) ||
      value.listener_token !== null && !digest(value.listener_token) ||
      value.http_status !== null && (!Number.isInteger(value.http_status) || value.http_status < 100 || value.http_status > 599)) return false;
  for (const name of ["accepted_source_digest", "package_sha256", "transferred_source_digest", "controller_sha256"])
    if (!digest(value[name]) || value[name] !== receipt[name]) return false;
  if (product ? !remoteBrowserReplyBindingOwned(value, receipt, state) :
      value.accepted_source_digest !== build?.source_digest || value.service_name !== receipt.service?.service ||
      value.service_source_digest !== receipt.service?.source_digest || !digest(value.service_source_digest) ||
      value.console_url !== receipt.service?.console_url) return false;
  if (value.status !== "ready") return value.local_url === null;
  return !!value.access_id && value.native_ready === true && value.cleanup_confirmed === false && value.transport_reachable === true &&
    (product ? digest(value.listener_token) : entry.listener_owned === true && digest(entry.listener_token) && value.listener_token === entry.listener_token) &&
    (value.http_status >= 200 && value.http_status < 400 || [401, 403].includes(value.http_status)) &&
    remoteApplicationAddress(product ? { path: value.entry_path, destination_port: value.destination_port } : entry) !== null &&
    value.local_url === remoteApplicationAddress(product ? { path: value.entry_path, destination_port: value.destination_port } : entry);
}

export function remoteApplicationReplyCurrent(value, receipt, state, build, experiences, entry) {
  return remoteApplicationReplyOwned(value, receipt, state, build, experiences, entry) && artifactVersionCurrent(value, state) &&
    value.build_id === build?.build_id && value.accepted_source_digest === build?.source_digest &&
    value.destination_port === entry.destination_port && (!remoteProductSelected(receipt) || remoteBrowserReplyBindingCurrent(value, receipt, state)) &&
    (value.status !== "ready" || entry.listener_owned === true && digest(entry.listener_token) && value.listener_token === entry.listener_token);
}

export function remoteApplicationLink(value, receipt, state, build, experiences, entry, now = Date.now()) {
  if (!remoteApplicationReplyCurrent(value, receipt, state, build, experiences, entry) || value.status !== "ready") return null;
  if (remoteProductSelected(receipt) && !remoteBrowserProductNativeReady(receipt, state)) return null;
  const observed = remoteBrowserObservedTime(value.observed_at);
  return observed <= now + 3000 && now - observed <= REMOTE_BROWSER_FRESH_MS ? value.local_url : null;
}

export function remoteApplicationCanOpen(value, entry) {
  return entry?.listener_owned === true && digest(entry.listener_token) && !!remoteApplicationAddress(entry) &&
    !!value && ["closed", "blocked", "unavailable"].includes(value.status) && value.cleanup_confirmed === true;
}

export function remoteApplicationCanClose(value) {
  return uuid(value?.access_id) && value.cleanup_confirmed !== true;
}
