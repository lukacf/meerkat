import type { ToolkitSnapshot } from "./model.js";

// Validate presentation DTOs, not authority, lifecycle transitions or host policy.
// Error text deliberately excludes projection values, including rejected auth material.
function invalid(): never { throw new TypeError("Invalid or ambiguous Toolkit projection"); }
function record(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) invalid();
  return value as Record<string, unknown>;
}
function only(value: unknown, keys: readonly string[]): void {
  if (Object.keys(record(value)).some(key => !keys.includes(key))) invalid();
}
function origin(value: unknown): void {
  text(value);
  try {
    const url = new URL(value as string);
    if (!['http:', 'https:'].includes(url.protocol) || url.origin !== value || url.username || url.password) invalid();
  } catch { invalid(); }
}
function text(value: unknown): void { if (typeof value !== "string") invalid(); }
function fields(value: unknown, required: readonly string[], optional: readonly string[] = []): Record<string, unknown> {
  const row = record(value);
  for (const key of required) text(row[key]);
  for (const key of optional) if (row[key] !== undefined) text(row[key]);
  return row;
}
function list(value: unknown): unknown[] { if (!Array.isArray(value)) invalid(); return value; }
function choice(value: unknown, choices: readonly string[]): void {
  if (typeof value !== "string" || !choices.includes(value)) invalid();
}
function rows(value: unknown, keys: readonly string[] = ["id"]): Record<string, unknown>[] {
  const seen = new Set<string>();
  return list(value).map(value => {
    const row = fields(value, keys);
    const key = JSON.stringify(keys.map(key => row[key]));
    if (seen.has(key)) invalid();
    seen.add(key);
    return row;
  });
}
function strings(value: unknown): void { for (const item of list(value)) text(item); }
function nonblank(value: unknown): void {
  text(value);
  if (!(value as string).trim()) invalid();
}
function catalog(value: unknown): void {
  const root = record(value);
  only(root, ["categories", "entries"]);
  const categories = rows(Array.from(list(root.categories)));
  const ids = new Set<string>();
  const labels = new Set<string>();
  for (const category of categories) {
    only(category, ["id", "label"]);
    nonblank(category.id); nonblank(category.label);
    if (labels.has(category.label as string)) invalid();
    ids.add(category.id as string); labels.add(category.label as string);
  }
  for (const item of rows(Array.from(list(root.entries)))) {
    only(item, ["id", "label", "description", "roles", "categoryIds", "support", "prerequisites", "evidence"]);
    fields(item, ["label", "description", "support"], ["evidence"]);
    nonblank(item.id); nonblank(item.label); nonblank(item.support);
    if (item.evidence !== undefined) nonblank(item.evidence);
    const roles = list(item.roles);
    if (!roles.length || new Set(roles).size !== roles.length) invalid();
    for (const role of roles) choice(role, ["service", "frontend"]);
    const references = list(item.categoryIds);
    if (new Set(references).size !== references.length) invalid();
    for (const reference of references) {
      text(reference);
      if (!ids.has(reference as string)) invalid();
    }
    for (const prerequisite of list(item.prerequisites)) nonblank(prerequisite);
  }
}
function status(value: unknown): void {
  only(value, ["text", "tone"]);
  const row = fields(value, ["text"]);
  choice(row.tone, ["pending", "ready", "attention", "inactive"]);
}

export function validateProjection(snapshot: ToolkitSnapshot): void {
  only(snapshot, ["scope", "revision", "observedAt", "announcement", "tasks", "connections", "frontends", "packages", "catalog"]);
  const root = fields(snapshot, ["scope", "observedAt", "announcement"]);
  if (!Number.isSafeInteger(root.revision) || (root.revision as number) < 0) invalid();
  if (root.catalog !== undefined) catalog(root.catalog);
  const offers = new Map<string, string>();
  function controls(value: unknown, presentation?: readonly unknown[]): void {
    const inRow = new Set<string>();
    for (const item of list(value)) {
      only(item, ["offer", "kind", "target", "explanation"]);
      const row = fields(item, ["offer"], ["explanation"]);
      choice(row.kind, ["open_authorization", "continue", "stop_setup", "cancel_task",
        "change_account", "retry", "choose_strategy", "reconnect", "revoke_grant",
        "disconnect", "pause_frontend", "resume_frontend", "remove_frontend", "review_activation"]);
      only(row.target, ["owner", "reference", "revision"]);
      const target = fields(row.target, ["owner", "reference", "revision"]);
      const key = JSON.stringify([target.owner, row.offer]);
      // A repeated offer may appear in different rows. Its typed payload and
      // displayed sign-in context must agree within this one projection.
      const payload = JSON.stringify([row.kind, target.reference, target.revision,
        row.explanation, row.kind === "open_authorization" ? presentation : undefined]);
      if (inRow.has(key) || (offers.has(key) && offers.get(key) !== payload)) invalid();
      inRow.add(key);
      offers.set(key, payload);
    }
  }
  function service(value: unknown): void {
    only(value, ["kind", "id", "label", "status", "account", "usedBy", "operations", "resources", "lifetime", "standingAuthority", "controls"]);
    const row = fields(value, ["id", "label", "account", "usedBy", "lifetime"], ["standingAuthority"]);
    choice(row.kind, ["service"]);
    status(row.status);
    strings(row.operations); strings(row.resources);
    controls(row.controls);
  }
  function frontend(value: unknown): void {
    only(value, ["kind", "id", "label", "packageId", "status", "endpoint", "audience", "target", "conversation", "output", "serviceAccess", "lifetime", "controls"]);
    const row = fields(value, ["id", "label", "packageId", "endpoint", "audience", "target",
      "conversation", "output", "serviceAccess", "lifetime"]);
    choice(row.kind, ["frontend"]);
    status(row.status); controls(row.controls);
  }
  for (const task of rows(root.tasks)) {
    only(task, ["id", "title", "application", "requester", "reason", "phase", "nextAction", "roles", "userActions", "controls", "diagnostics"]);
    fields(task, ["title", "application", "requester", "reason", "nextAction"], ["diagnostics"]);
    status(task.phase);
    for (const role of rows(task.roles, ["kind", "id"])) {
      choice(role.kind, ["service", "frontend"]);
      if (role.kind === "service") service(role); else frontend(role);
    }
    for (const action of rows(task.userActions)) {
      only(action, ["id", "integration", "kind", "state", "instructions", "expiresAt", "authorizationOrigin", "requestedAccountLabel", "controls"]);
      fields(action, ["integration", "instructions"], ["expiresAt", "authorizationOrigin", "requestedAccountLabel"]);
      if (action.authorizationOrigin !== undefined) origin(action.authorizationOrigin);
      choice(action.kind, ["browser_authorization", "device_authorization", "secret_entry",
        "interactive_setup", "administrator_action"]);
      choice(action.state, ["queued", "action_required", "verifying", "complete", "expired", "no_longer_needed"]);
      controls(action.controls, action.authorizationOrigin === undefined && action.requestedAccountLabel === undefined
        ? undefined : [action.authorizationOrigin, action.requestedAccountLabel]);
    }
    controls(task.controls);
  }
  const connections = rows(root.connections);
  for (const connection of connections) {
    only(connection, ["id", "packageId", "service", "account", "credentialScope", "status", "grants", "pendingTaskIds", "controls", "diagnostics"]);
    fields(connection, ["packageId", "service", "account", "credentialScope"], ["diagnostics"]);
    status(connection.status);
    strings(connection.pendingTaskIds);
    if (new Set(connection.pendingTaskIds as string[]).size !== list(connection.pendingTaskIds).length) invalid();
    for (const grant of rows(connection.grants)) service(grant);
    controls(connection.controls);
  }
  const frontends = rows(root.frontends);
  for (const item of frontends) frontend(item);
  const instancePackages = new Map<string, unknown>();
  for (const item of connections) instancePackages.set(JSON.stringify(["service", item.id]), item.packageId);
  for (const item of frontends) instancePackages.set(JSON.stringify(["frontend", item.id]), item.packageId);
  const linkedInstances = new Set<string>();
  for (const item of rows(root.packages)) {
    only(item, ["id", "label", "version", "status", "exports", "components", "diagnostics"]);
    fields(item, ["label", "version"], ["diagnostics"]);
    status(item.status);
    for (const exported of rows(item.exports, ["kind", "id"])) {
      only(exported, ["kind", "id", "label", "status", "instanceIds"]);
      fields(exported, ["label"]);
      choice(exported.kind, ["service", "frontend"]);
      status(exported.status);
      if (exported.instanceIds !== undefined) {
        for (const id of list(exported.instanceIds)) {
          nonblank(id);
          const key = JSON.stringify([exported.kind, id]);
          // This is a complete viewer projection, not an inventory lookup.
          // Missing, cross-package and multiply assigned targets are ambiguous.
          if (instancePackages.get(key) !== item.id || linkedInstances.has(key)) invalid();
          linkedInstances.add(key);
        }
      }
    }
    for (const component of list(item.components)) {
      only(component, ["label", "status"]);
      const row = fields(component, ["label"]);
      status(row.status);
    }
  }
}
