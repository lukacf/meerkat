import { deploymentApplicationStartupPending } from "./deployment-panel.mjs";

/** Readback is a presentation hint for the exact stopped installation. */
export function applicationSetupKey(receipt, state) {
  if (!deploymentApplicationStartupPending(receipt, state) ||
      !Number.isSafeInteger(state.version) || state.version < 0 ||
      !Number.isSafeInteger(receipt.guide_version) || receipt.guide_version < 0) return null;
  return JSON.stringify([state.id, state.version, receipt.deployment_id, receipt.build_id,
    receipt.package_sha256, receipt.source_digest, receipt.guide_version, receipt.updated_at]);
}

export function applicationSetupReadback(value, receipt, state) {
  if (!applicationSetupKey(receipt, state) || value?.scope !== "selected-stopped-application-inputs" ||
      value.guide_id !== state.id || value.deployment_id !== receipt.deployment_id ||
      value.build_id !== receipt.build_id || value.package_sha256 !== receipt.package_sha256 ||
      value.guide_version !== receipt.guide_version || typeof value.available !== "boolean") return null;
  if (value.available && !["fresh", "restore", "ordinary"].includes(value.kind)) return null;
  const reconnect = value.available === false && value.reason === "owner_reconnect" &&
    value.action === "prepare-setup" && value.kind == null;
  if (value.reason === "owner_reconnect" && !reconnect) return null;
  if (value.action != null && (value.action !== "prepare-setup" ||
      !reconnect && (!value.available || value.kind !== "fresh"))) return null;
  const missing = Array.isArray(value.missing_fields) && value.missing_fields.length <= 40 &&
    value.missing_fields.every((label) => typeof label === "string" && label.length > 0 && label.length <= 200)
    ? [...value.missing_fields] : [];
  if ((value.available || reconnect) && value.missing_fields != null && (!Array.isArray(value.missing_fields) || value.missing_fields.length)) return null;
  const background = value.available && value.kind === "restore" && applicationBackgroundReadback(value.background);
  return { available: value.available, missing, kind: value.kind, ...(value.action === "prepare-setup" ? { action: value.action } : {}),
    ...(background ? { background } : {}),
    name: typeof value.application_name === "string" && value.application_name.trim() && value.application_name.length <= 200
      ? value.application_name : "your application",
    reason: value.available === false && value.reason === "host_unavailable" ? "host-unavailable"
      : value.available === false && value.reason === "source_unavailable" ? "source-unavailable"
      : reconnect ? "owner-reconnect"
      : value.reason === "previous_attempt" ? "previous-attempt" : null };
}

/** Optional host capability cannot replace the checked installation or Restore. */
function applicationBackgroundReadback(value) {
  if (!value || Array.isArray(value) || typeof value !== "object" || typeof value.available !== "boolean" ||
      Object.keys(value).some((key) => !["available", "persistence", "detail"].includes(key)) ||
      ["persistence", "detail"].some((key) => Object.hasOwn(value, key) &&
        (typeof value[key] !== "string" || !value[key].trim() || value[key].length > 500))) return null;
  return { available: value.available,
    ...(Object.hasOwn(value, "persistence") ? { persistence: value.persistence } : {}),
    ...(Object.hasOwn(value, "detail") ? { detail: value.detail } : {}) };
}

export function applicationSetupView({ receipt, state, confirmed, pending, snapshot }) {
  const key = applicationSetupKey(receipt, state);
  if (!key) return null;
  if (!confirmed || pending) return { phase: "unconfirmed", name: "your application" };
  if (snapshot?.key !== key || snapshot.pending) return { phase: "checking", name: "your application" };
  const checked = applicationSetupReadback(snapshot.value, receipt, state);
  if (!checked) return { phase: "unavailable", name: "your application" };
  if (checked.available) return { phase: "ready", name: checked.name, kind: checked.kind,
    ...(checked.background ? { background: checked.background } : {}),
    ...(checked.action === "prepare-setup" ? { action: checked.action } : {}) };
  if (checked.reason === "owner-reconnect") return { phase: "reconnect", ...checked };
  return { phase: !["host-unavailable", "source-unavailable"].includes(checked.reason) && checked.missing.length ? "settings" : "unavailable", ...checked };
}
