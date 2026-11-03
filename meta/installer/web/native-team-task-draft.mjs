import { createInstalledUseDraftStore, installedUseSessionStorage } from "./deployment-use.mjs";

const digest = /^[a-f0-9]{64}$/;
const binding = /^native-team:[a-f0-9]{64}:[a-f0-9]{64}$/;
const text = value => typeof value === "string" && value.trim() && value.length <= 200 &&
  !/[\u0000-\u001f\u007f]/.test(value);

/** Human writing belongs to the installed team, independently of current health. */
export function nativeTeamTaskScopeKey(receipt, guide) {
  if (receipt?.source_kind !== "native-team" || receipt.build_id != null || receipt.guide_id !== guide?.id ||
      !text(guide?.id) || !text(receipt.deployment_id) ||
      ![receipt.source_id, receipt.source_digest, receipt.package_sha256].every(value => typeof value === "string" && digest.test(value)) ||
      !Number.isSafeInteger(receipt.guide_version) || receipt.guide_version < 1) return "";
  return JSON.stringify([guide.id, receipt.deployment_id,
    `native-team:${receipt.source_id}:${receipt.source_digest}`, receipt.package_sha256, receipt.guide_version]);
}

function selectedScope(scope) {
  try {
    const value = JSON.parse(scope);
    return Array.isArray(value) && value.length === 5 && text(value[0]) && text(value[1]) &&
      typeof value[2] === "string" && binding.test(value[2]) &&
      typeof value[3] === "string" && digest.test(value[3]) &&
      Number.isSafeInteger(value[4]) && value[4] >= 1 && JSON.stringify(value) === scope;
  } catch { return false; }
}

/** Reuse the installed draft mechanism in a separate native-team storage namespace. */
export function createNativeTeamTaskDraftStore(storage) {
  const prefix = "mobkit-native-team-task:";
  const ownedStorage = storage?.getItem && storage?.setItem && storage?.removeItem ? {
    getItem: key => storage.getItem(prefix + key),
    setItem: (key, value) => storage.setItem(prefix + key, value),
    removeItem: key => storage.removeItem(prefix + key),
  } : null;
  const drafts = createInstalledUseDraftStore(ownedStorage);
  return {
    read: scope => selectedScope(scope) ? drafts.read(scope).task : "",
    persistent: scope => selectedScope(scope) && drafts.persistent(scope),
    update: (scope, task) => selectedScope(scope) && drafts.update(scope, "task", task),
  };
}

const pageStores = new WeakMap();

/** Preserve the page fallback when the panel closes or its source changes. */
export function getNativeTeamTaskDraftStore(page = globalThis.window) {
  if (!page || !["object", "function"].includes(typeof page)) return createNativeTeamTaskDraftStore(null);
  if (!pageStores.has(page)) pageStores.set(page, createNativeTeamTaskDraftStore(installedUseSessionStorage(page)));
  return pageStores.get(page);
}
