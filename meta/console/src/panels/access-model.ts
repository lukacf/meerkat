// The Access panel's model (spec B13): who can do what, computed from the
// access rules the way the owner evaluates them, and how the rules overlap.
// Pure functions over the access configuration, tested without React. The
// owner stays the authority: the panel checks a cell with
// `meta/access/preview` before calling it verified.

import type { ConsoleAccessConfig, ConsoleAccessRule } from "../types";

/** A column of the matrix: a family of permissions people think of as one. */
export interface AccessFamily {
  id: string;
  label: string;
  actions: readonly string[];
}

/** The families, in the matrix's order. A permission the owner lists that no
 * family names lands in "Other". */
export const ACCESS_FAMILIES: readonly AccessFamily[] = [
  { id: "view", label: "View", actions: ["agent.view", "topology.view", "topology.audit", "mob.observe"] },
  { id: "message", label: "Message", actions: ["agent.send"] },
  { id: "lifecycle", label: "Lifecycle", actions: ["agent.spawn", "agent.respawn", "agent.retire", "agent.reset"] },
  { id: "approvals", label: "Approvals", actions: ["gating.view", "gating.decide"] },
  {
    id: "memory",
    label: "Memory",
    actions: [
      "agent.memory.read", "agent.memory.write", "agent.memory.delete", "agent.memory.admin",
      "operator.memory.read", "mob.memory.read", "mob.memory.propose", "mob.memory.commit",
      "memory.quarantine.review",
    ],
  },
  { id: "workgraph", label: "WorkGraph", actions: ["workgraph.view", "workgraph.manage"] },
  {
    id: "routing",
    label: "Routing",
    actions: ["runtime.admin", "topology.connect", "topology.disconnect", "topology.reconnect", "topology.bulk", "topology.cross_authority"],
  },
  {
    id: "admin",
    label: "Admin",
    actions: ["access.admin", "mobpack.author", "mobpack.deploy", "extension.read", "extension.edit", "extension.manage"],
  },
];

/** The families with the owner's own permission list: each keeps the
 * permissions the owner lists, an empty family drops out, and permissions
 * no family names form "Other". */
export function familiesFor(catalog: readonly string[]): AccessFamily[] {
  const known = new Set(ACCESS_FAMILIES.flatMap((family) => family.actions));
  const listed = new Set(catalog);
  const families = ACCESS_FAMILIES
    .map((family) => ({ ...family, actions: family.actions.filter((action) => listed.has(action)) }))
    .filter((family) => family.actions.length > 0);
  const other = catalog.filter((action) => !known.has(action));
  if (other.length) families.push({ id: "other", label: "Other", actions: other });
  return families;
}

/** A row of the matrix: everyone with admin, a group, a person a rule names,
 * or anyone at all. */
export interface AccessPrincipal {
  id: string;
  kind: "admins" | "group" | "person" | "everyone";
  label: string;
  /** The person (a person row). */
  subject?: string;
  /** Groups the row's people belong to. */
  groups: readonly string[];
  /** Signed in: rules for "*" (any signed-in person) apply. */
  signedIn: boolean;
}

export type CellState = "all" | "some" | "none" | "denied";

/** How one permission comes out for one row. */
export interface ActionVerdict {
  action: string;
  state: CellState;
  /** The rule that decides it: the deny that wins, else the allow. */
  rule?: ConsoleAccessRule;
  /** Deny rules that cut an allow down to some agents. */
  except?: readonly ConsoleAccessRule[];
}

export interface CellVerdict {
  state: CellState;
  actions: readonly ActionVerdict[];
}

/** Whether an action pattern ("*", "agent.*", an exact name) matches an
 * action, as the owner matches them. */
export function actionPatternMatches(pattern: string, action: string): boolean {
  if (pattern === "*" || pattern === action) return true;
  if (!pattern.endsWith(".*")) return false;
  const prefix = pattern.slice(0, -2);
  return action.startsWith(`${prefix}.`);
}

/** Whether a pattern is one the owner accepts against its permission list:
 * "*", a "prefix.*" that matches a listed permission, or a listed name. */
export function actionPatternIsKnown(pattern: string, catalog: readonly string[]): boolean {
  if (pattern === "*") return true;
  if (pattern.endsWith(".*")) return catalog.some((action) => actionPatternMatches(pattern, action));
  return catalog.includes(pattern);
}

export function hasResourceSelector(rule: ConsoleAccessRule): boolean {
  return Boolean(rule.agents?.length || rule.roles?.length || Object.keys(rule.match_labels ?? {}).length);
}

function mentionsMemory(pattern: string): boolean {
  const base = pattern.endsWith(".*") ? pattern.slice(0, -2) : pattern;
  return base.split(".").includes("memory");
}

/** The rules as the owner evaluates them: a configuration that names no
 * memory permission anywhere lets every rule that covers agent.view also
 * cover agent.memory.read ("read rides view", the owner's compatibility for
 * configurations older than the memory permissions). */
export function effectiveRules(rules: readonly ConsoleAccessRule[]): ConsoleAccessRule[] {
  const naive = !rules.some((rule) => rule.actions.some(mentionsMemory));
  if (!naive) return [...rules];
  return rules.map((rule) => {
    const view = rule.actions.some((pattern) => actionPatternMatches(pattern, "agent.view"));
    const read = rule.actions.some((pattern) => actionPatternMatches(pattern, "agent.memory.read"));
    return view && !read ? { ...rule, actions: [...rule.actions, "agent.memory.read"] } : rule;
  });
}

/** Whether a rule applies to a row's people: a rule naming no one applies to
 * everyone; "*" to anyone signed in; else the named people and groups. */
export function ruleApplies(rule: ConsoleAccessRule, principal: AccessPrincipal): boolean {
  const subjects = rule.subjects ?? [];
  const groups = rule.groups ?? [];
  if (subjects.length === 0 && groups.length === 0) return true;
  if (principal.signedIn && subjects.includes("*")) return true;
  if (principal.subject && subjects.includes(principal.subject)) return true;
  return groups.some((group) => principal.groups.includes(group));
}

export function actionVerdict(rules: readonly ConsoleAccessRule[], principal: AccessPrincipal, action: string): ActionVerdict {
  if (principal.kind === "admins") return { action, state: "all" };
  const matching = rules.filter((rule) => ruleApplies(rule, principal) && rule.actions.some((pattern) => actionPatternMatches(pattern, action)));
  const denies = matching.filter((rule) => rule.effect === "deny");
  const allows = matching.filter((rule) => rule.effect !== "deny");
  const wholeDeny = denies.find((rule) => !hasResourceSelector(rule));
  if (wholeDeny) return { action, state: "denied", rule: wholeDeny };
  const wholeAllow = allows.find((rule) => !hasResourceSelector(rule));
  if (wholeAllow) return denies.length ? { action, state: "some", rule: wholeAllow, except: denies } : { action, state: "all", rule: wholeAllow };
  if (allows.length) return { action, state: "some", rule: allows[0], except: denies.length ? denies : undefined };
  return { action, state: "none" };
}

/** One cell: every permission of the family allowed on every agent (all),
 * none allowed and at least one denied by a rule (denied), none allowed
 * (none), else partly (some agents, or some of the permissions). */
export function cellVerdict(rules: readonly ConsoleAccessRule[], principal: AccessPrincipal, family: AccessFamily): CellVerdict {
  const actions = family.actions.map((action) => actionVerdict(rules, principal, action));
  const states = new Set(actions.map((verdict) => verdict.state));
  let state: CellState;
  if (actions.length && actions.every((verdict) => verdict.state === "all")) state = "all";
  else if (actions.every((verdict) => verdict.state === "none" || verdict.state === "denied")) state = states.has("denied") ? "denied" : "none";
  else state = "some";
  return { state, actions };
}

/** The rows: admins, each group, each person a rule names (by name, not
 * "*"), then everyone. */
export function matrixRows(config: ConsoleAccessConfig): AccessPrincipal[] {
  const groups = Object.entries(config.groups ?? {});
  const rows: AccessPrincipal[] = [{ id: "admins", kind: "admins", label: "Admins", groups: [], signedIn: true }];
  for (const [name] of groups) rows.push({ id: `group:${name}`, kind: "group", label: name, groups: [name], signedIn: true });
  const named = new Set<string>();
  for (const rule of config.rules ?? []) for (const subject of rule.subjects ?? []) if (subject && subject !== "*") named.add(subject);
  for (const subject of [...named].sort()) {
    const memberOf = groups.filter(([, group]) => (group.members ?? []).includes(subject)).map(([name]) => name);
    rows.push({ id: `person:${subject}`, kind: "person", label: subject, subject, groups: memberOf, signedIn: true });
  }
  rows.push({ id: "everyone", kind: "everyone", label: "Everyone", groups: [], signedIn: false });
  return rows;
}

/** Who a row's check runs as: the first admin, a member of the group, the
 * person, or (everyone) no one signed in. Null when there is nobody to run
 * it as (a group without members, no admin). */
export function checkSubject(principal: AccessPrincipal, config: ConsoleAccessConfig): { subject: string | null; note: string } | null {
  if (principal.kind === "admins") {
    const admin = (config.admins ?? []).find((value) => value.trim());
    return admin ? { subject: admin, note: `Checked as ${admin}, an admin.` } : null;
  }
  if (principal.kind === "group") {
    const name = principal.groups[0];
    const member = (config.groups?.[name]?.members ?? []).find((value) => value.trim());
    return member ? { subject: member, note: `Checked as ${member}, a member of ${name}.` } : null;
  }
  if (principal.kind === "person" && principal.subject) return { subject: principal.subject, note: `Checked as ${principal.subject}.` };
  return { subject: null, note: "Checked as someone who is not signed in." };
}

/** How the deny rules cut into an allow rule: the permissions of the allow
 * rule a deny takes away for the same people and agents, by deny rule. */
export interface RuleOverlap {
  rule: ConsoleAccessRule;
  actions: readonly string[];
}

export interface RuleCoverage {
  /** Every permission the rule grants is denied again. */
  shadowed: boolean;
  /** Deny rules that take some of an allow rule's permissions away. */
  deniedBy: readonly RuleOverlap[];
  /** Allow rules a deny rule cuts into. */
  overrides: readonly RuleOverlap[];
}

function covers<T>(outer: readonly T[] | undefined, inner: readonly T[] | undefined): boolean {
  if (!outer || outer.length === 0) return true;
  if (!inner || inner.length === 0) return false;
  return inner.every((value) => outer.includes(value));
}

/** Whether a deny rule reaches everyone and everything an allow rule does. */
function denyCoversScope(deny: ConsoleAccessRule, allow: ConsoleAccessRule): boolean {
  const denyNamesNoOne = !(deny.subjects?.length) && !(deny.groups?.length);
  const allowNamesNoOne = !(allow.subjects?.length) && !(allow.groups?.length);
  let people: boolean;
  if (denyNamesNoOne) people = true;
  else if (allowNamesNoOne) people = false;
  else {
    const subjects = allow.subjects ?? [];
    const groups = allow.groups ?? [];
    people = groups.every((group) => (deny.groups ?? []).includes(group))
      && subjects.every((subject) => (deny.subjects ?? []).includes(subject) || ((deny.subjects ?? []).includes("*") && subject !== ""));
  }
  if (!people) return false;
  if (!hasResourceSelector(deny)) return true;
  const labels = (rule: ConsoleAccessRule) => Object.entries(rule.match_labels ?? {}).map(([key, value]) => `${key}=${value}`);
  return covers(deny.agents, allow.agents) && covers(deny.roles, allow.roles) && covers(labels(deny), labels(allow));
}

/** The permissions a rule names, expanded against the owner's list. */
export function ruleActions(rule: ConsoleAccessRule, catalog: readonly string[]): string[] {
  return catalog.filter((action) => rule.actions.some((pattern) => actionPatternMatches(pattern, action)));
}

export function ruleCoverage(rule: ConsoleAccessRule, rules: readonly ConsoleAccessRule[], catalog: readonly string[]): RuleCoverage {
  const own = ruleActions(rule, catalog);
  if (rule.effect === "deny") {
    const overrides = rules
      .filter((other) => other.id !== rule.id && other.effect !== "deny" && denyCoversScope(rule, other))
      .map((other) => ({ rule: other, actions: ruleActions(other, catalog).filter((action) => own.includes(action)) }))
      .filter((overlap) => overlap.actions.length > 0);
    return { shadowed: false, deniedBy: [], overrides };
  }
  const deniedBy = rules
    .filter((other) => other.id !== rule.id && other.effect === "deny" && denyCoversScope(other, rule))
    .map((other) => ({ rule: other, actions: own.filter((action) => ruleActions(other, catalog).includes(action)) }))
    .filter((overlap) => overlap.actions.length > 0);
  const denied = new Set(deniedBy.flatMap((overlap) => overlap.actions));
  return { shadowed: own.length > 0 && own.every((action) => denied.has(action)), deniedBy, overrides: [] };
}

/** The approver identities the configuration names and who holds them. */
export interface AccessApprover {
  id: string;
  subjects: string[];
  groups: string[];
}

/** The configuration's approvers (read only in this console). */
export function accessApprovers(config: ConsoleAccessConfig | null | undefined): AccessApprover[] {
  const raw = (config as { approvers?: unknown } | null | undefined)?.approvers;
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return [];
  const strings = (value: unknown) => (Array.isArray(value) ? value.filter((item): item is string => typeof item === "string" && item.trim() !== "") : []);
  return Object.entries(raw as Record<string, unknown>)
    .map(([id, holders]) => {
      const record = holders && typeof holders === "object" ? holders as { subjects?: unknown; groups?: unknown } : {};
      return { id, subjects: strings(record.subjects), groups: strings(record.groups) };
    })
    .sort((a, b) => a.id.localeCompare(b.id));
}
