import React from "react";
import type {
  ConsoleAccessConfig,
  ConsoleAccessEditBase,
  ConsoleAccessSaveFailure,
  ConsoleAccessSaveResult,
  ConsoleAccessRule,
  ConsoleAccessStatus,
} from "../types";
import { accessSaveNotice } from "../lib/errors";

export interface AccessPreviewResult {
  allowed?: boolean;
  reason?: string | null;
  groups?: string[];
  is_admin?: boolean;
}

interface AccessPanelProps {
  status: ConsoleAccessStatus | null;
  config: ConsoleAccessConfig | null;
  error?: string | null;
  loading?: boolean;
  readOnly?: boolean;
  agents: { identity: string; label: string }[];
  onRefresh: () => void | boolean | Promise<void | boolean>;
  onSetEnabled: (enabled: boolean, base: ConsoleAccessEditBase) => ConsoleAccessSaveResult | Promise<ConsoleAccessSaveResult>;
  onSaveAdmins: (admins: string[], base: ConsoleAccessEditBase) => ConsoleAccessSaveResult | Promise<ConsoleAccessSaveResult>;
  onUpsertRule: (rule: ConsoleAccessRule, base: ConsoleAccessEditBase) => ConsoleAccessSaveResult | Promise<ConsoleAccessSaveResult>;
  onDeleteRule: (id: string, base: ConsoleAccessEditBase) => ConsoleAccessSaveResult | Promise<ConsoleAccessSaveResult>;
  onSaveGroup: (name: string, group: { description?: string; members: string[] }, base: ConsoleAccessEditBase) => ConsoleAccessSaveResult | Promise<ConsoleAccessSaveResult>;
  onDeleteGroup: (name: string, base: ConsoleAccessEditBase) => ConsoleAccessSaveResult | Promise<ConsoleAccessSaveResult>;
  onPreview: (subject: string, action: string, identity?: string) => Promise<AccessPreviewResult | null>;
}

type Tab = "overview" | "groups" | "rules" | "preview";
type EditKind = "admins" | "rule" | "group" | "immediate";


export function parseListInput(raw: string): string[] {
  return raw
    .split(/[,\n]/)
    .map((token) => token.trim())
    .filter((token) => token.length > 0);
}

export function formatListInput(values: string[] | undefined): string {
  return (values || []).join(", ");
}

export function parseLabelSelectorInput(raw: string): Record<string, string> {
  const labels: Record<string, string> = {};
  for (const token of parseListInput(raw)) {
    const eq = token.indexOf("=");
    if (eq <= 0) continue;
    const key = token.slice(0, eq).trim();
    const value = token.slice(eq + 1).trim();
    if (key) labels[key] = value;
  }
  return labels;
}

export function formatLabelSelectorInput(labels: Record<string, string> | undefined): string {
  return Object.entries(labels || {})
    .map(([key, value]) => `${key}=${value}`)
    .join(", ");
}

export function summarizeRuleSubjects(rule: ConsoleAccessRule): string {
  const parts: string[] = [];
  if (rule.groups?.length) parts.push(`groups: ${rule.groups.join(", ")}`);
  if (rule.subjects?.length) parts.push(rule.subjects.join(", "));
  return parts.length > 0 ? parts.join(" · ") : "everyone";
}

export function summarizeRuleResources(rule: ConsoleAccessRule): string {
  const parts: string[] = [];
  if (rule.agents?.length) parts.push(`agents: ${rule.agents.join(", ")}`);
  if (rule.roles?.length) parts.push(`roles: ${rule.roles.join(", ")}`);
  const labels = formatLabelSelectorInput(rule.match_labels);
  if (labels) parts.push(`labels: ${labels}`);
  return parts.length > 0 ? parts.join(" · ") : "all agents";
}

interface RuleDraft {
  id: string;
  description: string;
  effect: "allow" | "deny";
  subjects: string;
  groups: string;
  actions: string[];
  agents: string;
  roles: string;
  matchLabels: string;
}

function emptyRuleDraft(): RuleDraft {
  return {
    id: "",
    description: "",
    effect: "allow",
    subjects: "",
    groups: "",
    actions: [],
    agents: "",
    roles: "",
    matchLabels: "",
  };
}

function draftFromRule(rule: ConsoleAccessRule): RuleDraft {
  return {
    id: rule.id,
    description: rule.description || "",
    effect: rule.effect === "deny" ? "deny" : "allow",
    subjects: formatListInput(rule.subjects),
    groups: formatListInput(rule.groups),
    actions: [...rule.actions],
    agents: formatListInput(rule.agents),
    roles: formatListInput(rule.roles),
    matchLabels: formatLabelSelectorInput(rule.match_labels),
  };
}

export function ruleFromDraft(draft: RuleDraft): ConsoleAccessRule {
  const rule: ConsoleAccessRule = {
    id: draft.id.trim(),
    effect: draft.effect,
    actions: draft.actions,
  };
  const description = draft.description.trim();
  if (description) rule.description = description;
  const subjects = parseListInput(draft.subjects);
  if (subjects.length) rule.subjects = subjects;
  const groups = parseListInput(draft.groups);
  if (groups.length) rule.groups = groups;
  const agents = parseListInput(draft.agents);
  if (agents.length) rule.agents = agents;
  const roles = parseListInput(draft.roles);
  if (roles.length) rule.roles = roles;
  const labels = parseLabelSelectorInput(draft.matchLabels);
  if (Object.keys(labels).length) rule.match_labels = labels;
  return rule;
}

export const __accessTest = {
  parseListInput,
  formatListInput,
  parseLabelSelectorInput,
  formatLabelSelectorInput,
  summarizeRuleSubjects,
  summarizeRuleResources,
  ruleFromDraft,
  emptyRuleDraft,
};

export function AccessPanel({
  status,
  config,
  error,
  loading = false,
  readOnly = false,
  agents,
  onRefresh,
  onSetEnabled,
  onSaveAdmins,
  onUpsertRule,
  onDeleteRule,
  onSaveGroup,
  onDeleteGroup,
  onPreview,
}: AccessPanelProps): React.JSX.Element {
  const [tab, setTab] = React.useState<Tab>("overview");
  const [ruleDraft, setRuleDraft] = React.useState<RuleDraft | null>(null);
  const [adminsDraft, setAdminsDraft] = React.useState<string | null>(null);
  const [groupNameDraft, setGroupNameDraft] = React.useState("");
  const [groupMembersDraft, setGroupMembersDraft] = React.useState("");
  const [editingGroup, setEditingGroup] = React.useState<string | null>(null);
  const [previewSubject, setPreviewSubject] = React.useState("");
  const [previewAction, setPreviewAction] = React.useState("agent.view");
  const [previewIdentity, setPreviewIdentity] = React.useState("");
  const [previewFeedback, setPreviewFeedback] = React.useState<{
    scope: string; result?: AccessPreviewResult; error?: string;
  } | null>(null);
  const previewSubjectInput = React.useRef<HTMLInputElement>(null);
  const inspectFocusPending = React.useRef(false);

  const actions = status?.actions ?? [];
  const [saving, setSaving] = React.useState(false);
  const [saveFailure, setSaveFailure] = React.useState<(ConsoleAccessSaveFailure & { edit: EditKind }) | null>(null);
  const [adminsBase, setAdminsBase] = React.useState<ConsoleAccessEditBase | null>(null);
  const [ruleBase, setRuleBase] = React.useState<ConsoleAccessEditBase | null>(null);
  const [groupBase, setGroupBase] = React.useState<ConsoleAccessEditBase | null>(null);
  const [previewPending, setPreviewPending] = React.useState(false);
  const previewVersion = React.useRef(0);
  const mayView = status?.available === true && status.can_administer === true;
  const current = mayView && !loading && !error && Boolean(config);
  const actionCatalogKey = JSON.stringify(actions);
  const scope = JSON.stringify([status?.subject, status?.available, status?.can_administer]);
  const mutationScope = React.useRef(scope);
  mutationScope.current = scope;
  // Edits belong to the scope they were opened under. A changed scope
  // discards them in the render that shows it: an effect would discard them
  // one passive flush later, so the first commit under the new scope still
  // showed the old edit, and an edit opened against that commit was dropped.
  const [editScope, setEditScope] = React.useState(scope);
  if (editScope !== scope) {
    setEditScope(scope);
    setRuleDraft(null); setAdminsDraft(null); setGroupNameDraft("");
    setGroupMembersDraft(""); setEditingGroup(null); setSaveFailure(null);
    setAdminsBase(null); setRuleBase(null); setGroupBase(null);
  }
  // Refresh hides the observation until current state is available. An
  // unchanged snapshot must not discard an answer racing that refresh.
  const previewScope = JSON.stringify([scope, status?.owner_instance, status?.revision, status?.enabled, config,
    actionCatalogKey, error, readOnly, previewSubject, previewAction, previewIdentity]);
  const currentPreview = current && previewFeedback?.scope === previewScope ? previewFeedback : null;
  const previewResult = currentPreview?.result;
  const previewError = currentPreview?.error;
  const latestPreviewScope = React.useRef(previewScope);
  latestPreviewScope.current = previewScope;
  React.useEffect(() => {
    previewVersion.current += 1;
    setPreviewFeedback(null);
    setPreviewPending(false);
  }, [previewScope]);
  React.useEffect(() => {
    setPreviewAction(value => actions.includes(value) ? value : (actions[0] ?? ""));
  }, [actionCatalogKey]);
  React.useEffect(() => () => { previewVersion.current += 1; }, []);
  React.useEffect(() => {
    if (tab === "preview" && inspectFocusPending.current) {
      previewSubjectInput.current?.focus();
      inspectFocusPending.current = false;
    }
  }, [tab]);
  const rules = config?.rules || [];
  const groups = Object.entries(config?.groups || {});
  const enabled = config?.enabled === true;
  const checked = status?.conditional_mutations === "checked_v1"
    && typeof status.owner_instance === "string" && status.owner_instance.length > 0
    && Number.isSafeInteger(status.revision) && status.revision! >= 0;
  const canReview = current && checked && !readOnly && !saving;
  // An invalid draft or an ordinary failure stays editable against its own
  // base; owner, revision and capability failures need an explicit review.
  const requiresReview = saveFailure !== null && saveFailure.kind !== "failed" && saveFailure.kind !== "invalid";
  const canEdit = canReview && !requiresReview;
  function captureBase(): ConsoleAccessEditBase | null {
    if (!canReview || !config) return null;
    return { owner_instance: status!.owner_instance!, revision: status!.revision!, config: structuredClone(config) };
  }
  async function mutate(edit: EditKind, base: ConsoleAccessEditBase | null,
    action: (base: ConsoleAccessEditBase) => ConsoleAccessSaveResult | Promise<ConsoleAccessSaveResult>, done = () => {}) {
    if (!canEdit || !base) return;
    const requestedScope = scope;
    setSaving(true); setSaveFailure(null);
    try {
      const result = await action(base);
      if (requestedScope !== mutationScope.current) return;
      if (result && typeof result === "object") setSaveFailure({ ...result, edit });
      else if (result !== false) done();
    } catch {
      if (requestedScope === mutationScope.current) setSaveFailure({ kind: "failed", edit });
    } finally { setSaving(false); }
  }
  function reviewAndReapply() {
    const base = captureBase();
    if (!base || !saveFailure || !requiresReview) return;
    if (saveFailure.edit === "admins") setAdminsBase(base);
    if (saveFailure.edit === "rule") setRuleBase(base);
    if (saveFailure.edit === "group") setGroupBase(base);
    setSaveFailure(null);
  }

  function startGroupEdit(name: string, members: string[] | undefined) {
    setGroupBase(captureBase());
    setEditingGroup(name);
    setGroupNameDraft(name);
    setGroupMembersDraft(formatListInput(members));
  }

  function inspectSubject(subject: string) {
    if (!current || saving) return;
    previewVersion.current += 1;
    setPreviewFeedback(null); setPreviewPending(false);
    setPreviewSubject(subject);
    inspectFocusPending.current = true;
    setTab("preview");
  }

  function submitGroup() {
    const name = groupNameDraft.trim();
    if (!name) return;
    void mutate("group", groupBase, base => onSaveGroup(name, { ...base.config.groups?.[name], members: parseListInput(groupMembersDraft) }, base), () => {
      setEditingGroup(null); setGroupNameDraft(""); setGroupMembersDraft(""); setGroupBase(null);
    });
  }

  async function runPreview() {
    const subject = previewSubject.trim();
    if (!current || !subject || !actions.includes(previewAction) || previewPending) return;
    const version = ++previewVersion.current;
    const requestedScope = previewScope;
    setPreviewPending(true); setPreviewFeedback(null);
    try {
      const result = await onPreview(subject, previewAction, previewIdentity.trim() || undefined);
      if (version === previewVersion.current && requestedScope === latestPreviewScope.current) {
        if (result?.allowed === true || result?.allowed === false) setPreviewFeedback({ scope: requestedScope, result });
        else setPreviewFeedback({ scope: requestedScope, error: "Access preview unavailable. No decision was returned." });
      }
    } catch {
      if (version === previewVersion.current && requestedScope === latestPreviewScope.current) {
        setPreviewFeedback({ scope: requestedScope, error: "Access preview unavailable. Refresh and try again." });
      }
    } finally {
      if (version === previewVersion.current) setPreviewPending(false);
    }
  }

  if (!mayView) return (
    <section className="gating access-panel" data-testid="access-panel" style={{ padding: 24, gap: 12, alignItems: "flex-start" }}>
      <h2>Console access</h2>
      <p role="status">{loading ? "Loading Console access..." : status?.available === false
        ? "Console access administration is not available on this runtime."
        : "Current administrator access is required to view this configuration."}</p>
      <button onClick={onRefresh} data-testid="access-refresh">Refresh</button>
    </section>
  );

  return (
    <div className="gating access-panel" data-testid="access-panel">
      <div className="gating__head" style={{ flexDirection: "column", alignItems: "flex-start", gap: 6 }}>
        <h2>Console access</h2>
        <p>Manage access to Meta console and runtime surfaces. These rules do not grant native tool, source or connected-account permissions.</p>
        <p>
          · {enabled ? "enforcing" : "not enforced"} · {rules.length} rules ·{" "}
          {groups.length} groups
          {status?.subject ? <> · you are {status.subject}</> : null}
        </p>
      </div>
      {readOnly ? <p role="status">This connection is read-only.</p> : null}
      <div className="gating__tabs">
        {(["overview", "groups", "rules", "preview"] as Tab[]).map((candidate) => (
          <button
            key={candidate}
            className={`gating__tab ${tab === candidate ? "is-active" : ""}`}
            onClick={() => setTab(candidate)}
            data-testid={`access-tab:${candidate}`}
          >
            {candidate === "overview" ? "Overview"
              : candidate === "groups" ? `Groups`
              : candidate === "rules" ? `Rules`
              : "Preview"}
            {candidate === "groups" ? <span className="n">{groups.length}</span> : null}
            {candidate === "rules" ? <span className="n">{rules.length}</span> : null}
          </button>
        ))}
        <button className="gating__tab" onClick={onRefresh} data-testid="access-refresh">
          Refresh
        </button>
      </div>
      {/* Owner-state feedback comes and goes with every refresh, and stream
          events start refreshes at any moment. It renders below the tabs so a
          refresh never moves them under a click in progress. */}
      {loading ? <p role="status">Refreshing owner state. Changes are temporarily unavailable.</p> : null}
      {error || saveFailure ? <div className="gating__empty" role="alert" data-testid="access-error">{error || (saveFailure ? accessSaveNotice(saveFailure) : null)}</div> : null}
      {requiresReview && canReview ? <button onClick={reviewAndReapply}>Review and reapply</button> : null}
      {error ? <p style={{ padding: "0 24px" }}>Last loaded configuration may be out of date. Refresh before making changes.</p> : null}
      <fieldset className="gating__list access-panel__body" disabled={tab === "preview" ? !current : tab === "groups" ? !current || saving : !canEdit} style={{ border: 0, margin: 0, minWidth: 0 }}>
        {tab === "overview" ? (
          <div className="gating__policies">
            <div className="gpolicy" data-state={enabled ? "active" : "paused"}>
              <div className="gpolicy__head">
                <span className="gpolicy__action">Enforcement</span>
                <span className={`gpolicy__state gpolicy__state--${enabled ? "active" : "paused"}`}>
                  {enabled ? "enabled" : "disabled"}
                </span>
              </div>
              <div className="gpolicy__rule">
                {enabled
                  ? "Deny by default: every console caller only sees and operates what a rule (or admin standing) grants."
                  : "Access control is configured but not enforced. Enabling requires at least one admin subject."}
              </div>
              {canEdit ? (
                <div className="gpolicy__stats">
                  <button
                    data-testid="access-toggle-enabled"
                    onClick={() => void mutate("immediate", captureBase(), base => onSetEnabled(!enabled, base))}
                  >
                    {enabled ? "Disable enforcement" : "Enable enforcement"}
                  </button>
                </div>
              ) : null}
            </div>
            <div className="gpolicy" data-state="active">
              <div className="gpolicy__head">
                <span className="gpolicy__action">Admins</span>
              </div>
              <div className="gpolicy__rule">
                Admin subjects bypass every rule and manage this configuration.
              </div>
              {adminsDraft === null ? (
                <>
                  <div className="gpolicy__approvers">
                    {(config?.admins || []).length === 0 ? (
                      <span className="chip">no admins configured</span>
                    ) : (
                      (config?.admins || []).map((admin) => (
                        <span className="chip" key={admin}>{admin}</span>
                      ))
                    )}
                  </div>
                  {canEdit ? (
                    <div className="gpolicy__stats">
                      <button
                        data-testid="access-edit-admins"
                        onClick={() => { setAdminsBase(captureBase()); setAdminsDraft(formatListInput(config?.admins)); }}
                      >
                        Edit admins
                      </button>
                    </div>
                  ) : null}
                </>
              ) : (
                <div className="access-panel__form">
                  <label>
                    Admin subjects (comma separated)
                    <input
                      data-testid="access-admins-input"
                      value={adminsDraft}
                      onChange={(event) => setAdminsDraft(event.target.value)}
                      placeholder="root@example.com, ops-lead@example.com"
                    />
                  </label>
                  <div className="access-panel__form-actions">
                    <button
                      className="approve"
                      data-testid="access-save-admins"
                      onClick={() => {
                        void mutate("admins", adminsBase, base => onSaveAdmins(parseListInput(adminsDraft), base), () => { setAdminsDraft(null); setAdminsBase(null); });
                      }}
                    >
                      Save
                    </button>
                    <button onClick={() => { setAdminsDraft(null); setAdminsBase(null); }}>Cancel</button>
                  </div>
                </div>
              )}
            </div>
          </div>
        ) : null}

        {tab === "groups" ? (
          <div className="gating__policies">
            {groups.length === 0 && editingGroup === null ? (
              <div className="gating__empty">
                No groups yet. Groups assign people to rules - create one, then
                reference it from a rule.
              </div>
            ) : null}
            {groups.map(([name, group]) =>
              editingGroup === name ? null : (
                <div className="gpolicy" data-state="active" key={name} data-testid={`access-group:${name}`}>
                  <div className="gpolicy__head">
                    <span className="gpolicy__action">{name}</span>
                  </div>
                  {group.description ? (
                    <div className="gpolicy__rule">{group.description}</div>
                  ) : null}
                  <div className="gpolicy__approvers">
                    {(group.members || []).length === 0 ? (
                      <span className="chip">no members</span>
                    ) : (
                      (group.members || []).map((member) => (
                        <span className="chip" key={member} style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                          {member}
                          {current && !saving ? (
                            <button
                              type="button"
                              style={{ textDecoration: "underline" }}
                              aria-label={`Inspect access for ${member}`}
                              onClick={() => inspectSubject(member)}
                            >
                              Inspect access
                            </button>
                          ) : null}
                        </span>
                      ))
                    )}
                  </div>
                  {canEdit ? (
                    <div className="gpolicy__stats">
                      <button
                        data-testid={`access-group-edit:${name}`}
                        onClick={() => startGroupEdit(name, group.members)}
                      >
                        Edit members
                      </button>
                      <button
                        className="reject"
                        data-testid={`access-group-delete:${name}`}
                        onClick={() => {
                          if (window.confirm(`Delete group "${name}"?`)) {
                            void mutate("immediate", captureBase(), base => onDeleteGroup(name, base));
                          }
                        }}
                      >
                        Delete
                      </button>
                    </div>
                  ) : null}
                </div>
              ),
            )}
            {canEdit || groupBase ? (
              <div className="gpolicy" data-state="active">
                <div className="gpolicy__head">
                  <span className="gpolicy__action">
                    {editingGroup ? `Edit ${editingGroup}` : "New group"}
                  </span>
                </div>
                {/* Member inspection keeps this tab's fieldset enabled, so a
                    retained draft is disabled here until it can be saved. */}
                <div className="access-panel__form">
                  <label>
                    Group name
                    <input
                      data-testid="access-group-name"
                      value={groupNameDraft}
                      onChange={(event) => { if (!groupBase) setGroupBase(captureBase()); setGroupNameDraft(event.target.value); }}
                      placeholder="ops"
                      disabled={!canEdit || editingGroup !== null}
                    />
                  </label>
                  <label>
                    Members (comma separated subjects)
                    <input
                      data-testid="access-group-members"
                      value={groupMembersDraft}
                      onChange={(event) => { if (!groupBase) setGroupBase(captureBase()); setGroupMembersDraft(event.target.value); }}
                      placeholder="alice@example.com, bob@example.com"
                      disabled={!canEdit}
                    />
                  </label>
                  <div className="access-panel__form-actions">
                    <button className="approve" data-testid="access-group-save" onClick={submitGroup} disabled={!canEdit}>
                      {editingGroup ? "Save members" : "Create group"}
                    </button>
                    {editingGroup ? (
                      <button onClick={() => { setEditingGroup(null); setGroupNameDraft(""); setGroupMembersDraft(""); setGroupBase(null); }} disabled={!canEdit}>
                        Cancel
                      </button>
                    ) : null}
                  </div>
                </div>
              </div>
            ) : null}
          </div>
        ) : null}

        {tab === "rules" ? (
          <div className="gating__policies">
            {rules.length === 0 && !ruleDraft ? (
              <div className="gating__empty">
                No rules. While enforcement is on, only admins can see or do
                anything until rules grant access.
              </div>
            ) : null}
            {rules.map((rule) =>
              ruleDraft && ruleDraft.id === rule.id ? null : (
                <div
                  className="gpolicy"
                  data-state={rule.effect === "deny" ? "paused" : "active"}
                  key={rule.id}
                  data-testid={`access-rule:${rule.id}`}
                >
                  <div className="gpolicy__head">
                    <span className="gpolicy__action">{rule.id}</span>
                    <span className={`gpolicy__state gpolicy__state--${rule.effect === "deny" ? "paused" : "active"}`}>
                      {rule.effect === "deny" ? "deny" : "allow"}
                    </span>
                  </div>
                  {rule.description ? (
                    <div className="gpolicy__rule">{rule.description}</div>
                  ) : null}
                  <div className="gpolicy__meta">who: {summarizeRuleSubjects(rule)}</div>
                  <div className="gpolicy__meta">what: {rule.actions.join(", ")}</div>
                  <div className="gpolicy__meta">on: {summarizeRuleResources(rule)}</div>
                  {canEdit ? (
                    <div className="gpolicy__stats">
                      <button
                        data-testid={`access-rule-edit:${rule.id}`}
                        onClick={() => { setRuleBase(captureBase()); setRuleDraft(draftFromRule(rule)); }}
                      >
                        Edit
                      </button>
                      <button
                        className="reject"
                        data-testid={`access-rule-delete:${rule.id}`}
                        onClick={() => {
                          if (window.confirm(`Delete rule "${rule.id}"? Access it grants (or denies) stops immediately.`)) {
                            void mutate("immediate", captureBase(), base => onDeleteRule(rule.id, base));
                          }
                        }}
                      >
                        Delete
                      </button>
                    </div>
                  ) : null}
                </div>
              ),
            )}
            {canEdit && !ruleDraft ? (
              <div className="gpolicy__stats">
                <button data-testid="access-rule-new" onClick={() => { setRuleBase(captureBase()); setRuleDraft({ ...emptyRuleDraft(), actions: actions.slice(0, 1) }); }}>
                  New rule
                </button>
              </div>
            ) : null}
            {ruleDraft ? (
              <div className="gpolicy" data-state="active" data-testid="access-rule-editor">
                <div className="gpolicy__head">
                  <span className="gpolicy__action">
                    {rules.some((rule) => rule.id === ruleDraft.id) ? `Edit ${ruleDraft.id}` : "New rule"}
                  </span>
                </div>
                <div className="access-panel__form">
                  <label>
                    Rule id
                    <input
                      data-testid="access-rule-id"
                      value={ruleDraft.id}
                      onChange={(event) => setRuleDraft({ ...ruleDraft, id: event.target.value })}
                      placeholder="ops-view-all"
                    />
                  </label>
                  <label>
                    Description
                    <input
                      value={ruleDraft.description}
                      onChange={(event) => setRuleDraft({ ...ruleDraft, description: event.target.value })}
                      placeholder="Ops can see every agent"
                    />
                  </label>
                  <label>
                    Effect
                    <select
                      data-testid="access-rule-effect"
                      value={ruleDraft.effect}
                      onChange={(event) =>
                        setRuleDraft({ ...ruleDraft, effect: event.target.value === "deny" ? "deny" : "allow" })
                      }
                    >
                      <option value="allow">allow</option>
                      <option value="deny">deny</option>
                    </select>
                  </label>
                  <label>
                    Groups (comma separated; empty + empty subjects = everyone)
                    <input
                      data-testid="access-rule-groups"
                      value={ruleDraft.groups}
                      onChange={(event) => setRuleDraft({ ...ruleDraft, groups: event.target.value })}
                      placeholder="ops"
                    />
                  </label>
                  <label>
                    Subjects (comma separated emails)
                    <input
                      value={ruleDraft.subjects}
                      onChange={(event) => setRuleDraft({ ...ruleDraft, subjects: event.target.value })}
                      placeholder="alice@example.com"
                    />
                  </label>
                  <label>
                    Actions
                    <div className="access-panel__chips">
                      {actions.map((action) => {
                        const selected = ruleDraft.actions.includes(action);
                        return (
                          <button
                            key={action}
                            className={`chip ${selected ? "is-active" : ""}`}
                            data-selected={selected ? "true" : "false"}
                            data-testid={`access-rule-action:${action}`}
                            onClick={() =>
                              setRuleDraft({
                                ...ruleDraft,
                                actions: selected
                                  ? ruleDraft.actions.filter((candidate) => candidate !== action)
                                  : [...ruleDraft.actions, action],
                              })
                            }
                          >
                            {action}
                          </button>
                        );
                      })}
                    </div>
                  </label>
                  <label>
                    Agents (comma separated identities; empty = all)
                    <input
                      data-testid="access-rule-agents"
                      value={ruleDraft.agents}
                      onChange={(event) => setRuleDraft({ ...ruleDraft, agents: event.target.value })}
                      placeholder={agents.slice(0, 2).map((agent) => agent.identity).join(", ") || "identity:ops-lead"}
                    />
                  </label>
                  <label>
                    Roles (comma separated; empty = all)
                    <input
                      value={ruleDraft.roles}
                      onChange={(event) => setRuleDraft({ ...ruleDraft, roles: event.target.value })}
                      placeholder="analyst"
                    />
                  </label>
                  <label>
                    Label selector (key=value, comma separated; empty = all)
                    <input
                      value={ruleDraft.matchLabels}
                      onChange={(event) => setRuleDraft({ ...ruleDraft, matchLabels: event.target.value })}
                      placeholder="org=payments"
                    />
                  </label>
                  <div className="access-panel__form-actions">
                    <button
                      className="approve"
                      data-testid="access-rule-save"
                      disabled={!canEdit || !ruleDraft.id.trim() || ruleDraft.actions.length === 0 || ruleDraft.actions.some(action => !actions.includes(action))}
                      onClick={() => {
                        void mutate("rule", ruleBase, base => onUpsertRule(ruleFromDraft(ruleDraft), base), () => { setRuleDraft(null); setRuleBase(null); });
                      }}
                    >
                      Save rule
                    </button>
                    <button onClick={() => { setRuleDraft(null); setRuleBase(null); }}>Cancel</button>
                  </div>
                </div>
              </div>
            ) : null}
          </div>
        ) : null}

        {tab === "preview" ? (
          <div className="gating__policies">
            <div className="gpolicy" data-state="active">
              <div className="gpolicy__head">
                <span className="gpolicy__action">Check Console access as someone else</span>
              </div>
              <div className="access-panel__form">
                <label>
                  Subject
                  <input
                    data-testid="access-preview-subject"
                    ref={previewSubjectInput}
                    value={previewSubject}
                    onChange={(event) => setPreviewSubject(event.target.value)}
                    placeholder="alice@example.com"
                  />
                </label>
                <label>
                  Action
                  <select
                    data-testid="access-preview-action"
                    value={previewAction}
                    onChange={(event) => setPreviewAction(event.target.value)}
                  >
                    {actions.map((action) => (
                      <option key={action} value={action}>{action}</option>
                    ))}
                  </select>
                </label>
                <label>
                  Agent (optional)
                  <select
                    data-testid="access-preview-agent"
                    value={previewIdentity}
                    onChange={(event) => setPreviewIdentity(event.target.value)}
                  >
                    <option value="">-</option>
                    {agents.map((agent) => (
                      <option key={agent.identity} value={agent.identity}>
                        {agent.label || agent.identity}
                      </option>
                    ))}
                  </select>
                </label>
                <div className="access-panel__form-actions">
                  <button className="approve" data-testid="access-preview-run" disabled={!current || !previewSubject.trim() || !actions.includes(previewAction) || previewPending} onClick={() => void runPreview()}>
                    Evaluate
                  </button>
                </div>
                {previewError ? <p role="status" data-testid="access-preview-error">{previewError}</p> : null}
                {previewResult ? (
                  <div
                    className="gpolicy__rule"
                    data-testid="access-preview-result"
                    data-allowed={previewResult.allowed ? "true" : "false"}
                  >
                    <span>Observation only, not permission for a later action. </span>
                    {previewResult.allowed ? "ALLOWED" : "DENIED"}
                    {previewResult.reason ? ` - ${previewResult.reason}` : ""}
                    {previewResult.is_admin ? " (admin)" : ""}
                    {previewResult.groups?.length
                      ? ` · groups: ${previewResult.groups.join(", ")}`
                      : ""}
                  </div>
                ) : null}
              </div>
            </div>
          </div>
        ) : null}
      </fieldset>
    </div>
  );
}
