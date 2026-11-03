// Shared generic normalizers and small utilities for the Flow Editor
// controller plane. Re-homed verbatim from the controller.js residue: the
// member-patches tail normalizers, misfiled doc-build utils (numberOrNull,
// findMember, jsonEquivalent), source-view's escapeHtml, and the single
// canonical graphInstanceIdSet (the residue previously carried two
// hoisting-resolved duplicates).
export function normalizeProfileBackend(value) {
  return String(value || "").trim();
}

export function normalizeMaxInlinePeerNotifications(value) {
  if (value === null || value === undefined || value === "") return null;
  const number = typeof value === "number" ? value : Number(value);
  if (!Number.isInteger(number) || number < -1) return null;
  return number;
}

export function normalizePositiveInteger(value) {
  if (value === null || value === undefined || value === "") return null;
  const number = typeof value === "number" ? value : Number(value);
  if (!Number.isInteger(number) || number <= 0) return null;
  return number;
}

export function normalizeStringList(value) {
  const source = Array.isArray(value)
    ? value
    : String(value || "").split(",");
  return source
    .map((item) => String(item || "").trim())
    .filter(Boolean);
}

export function normalizeOutputFormat(value) {
  return String(value || "").trim();
}

export function normalizeProviderParams(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  return JSON.parse(JSON.stringify(value));
}

export function normalizeOptionalObject(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  return JSON.parse(JSON.stringify(value));
}

export function jsonEquivalent(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

export function findMember(members, id) {
  return (members || []).find((member) => member.id === id) || null;
}

export function numberOrNull(value) {
  if (value === "" || value === null || value === undefined) return null;
  const n = Number(value);
  return Number.isFinite(n) ? n : null;
}

export function escapeHtml(source) {
  return String(source || "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

export function graphInstanceIdSet(instances = []) {
  return new Set((Array.isArray(instances) ? instances : [])
    .map((instance) => String(instance?.id || "").trim())
    .filter(Boolean));
}
