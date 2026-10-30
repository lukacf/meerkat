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
import { accessActionLabel, isKnownAccessAction } from "../lib/access-actions";
import {
  AgentAvatar,
  Button,
  Chip,
  EmptyState,
  KeyValueList,
  KitGlyph,
  ListRow,
  Notice,
  PanelBody,
  PanelHeader,
  PanelSection,
  PanelShell,
  PanelTabs,
  RowList,
  SectionHeader,
  SelectField,
  TechnicalDetails,
  TechnicalId,
  Toolbar,
  agentDisplayName,
  cx,
  joinMeta,
  plural,
  useAgentDirectory,
} from "../../../packages/console-components/src/panel-kit";

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

const TAB_LABELS: Record<Tab, string> = {
  overview: "Overview",
  groups: "Groups",
  rules: "Rules",
  preview: "Preview",
};

/** A labelled text input with an optional hint under it. */
function TextField({
  label,
  hint,
  value,
  onChange,
  placeholder,
  disabled,
  readOnly,
  testId,
  inputRef,
  wide,
}: {
  label: string;
  hint?: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  /** Shown and copyable but not editable (an existing rule's ID). */
  readOnly?: boolean;
  testId?: string;
  inputRef?: React.Ref<HTMLInputElement>;
  /** Spans the whole form row (lists of subjects). */
  wide?: boolean;
}): React.JSX.Element {
  const id = React.useId();
  const hintId = `${id}-hint`;
  return (
    <div className={cx("cc-field", "access-panel__field", wide && "access-panel__field--wide")}>
      <label htmlFor={id} className="cc-field__label">{label}</label>
      <input
        id={id}
        ref={inputRef}
        className="cc-input"
        data-testid={testId}
        value={value}
        onChange={(event) => { if (!readOnly) onChange(event.target.value); }}
        placeholder={placeholder}
        disabled={disabled}
        readOnly={readOnly}
        aria-describedby={hint ? hintId : undefined}
      />
      {hint ? <span id={hintId} className="access-panel__hint">{hint}</span> : null}
    </div>
  );
}

/** One person or service in a list: an initial avatar and the subject. */
function SubjectRow({ subject, actions }: { subject: string; actions?: React.ReactNode }): React.JSX.Element {
  return (
    <ListRow
      dense
      lead={<AgentAvatar name={subject} />}
      title={subject}
      titleAttr={subject}
      actions={actions}
    />
  );
}

/** A rule as a card: what it does in words, who it covers, what it allows
 * or denies and where. The rule ID stays available as a secondary line. */
function RuleCard({
  rule,
  actions,
}: {
  rule: ConsoleAccessRule;
  actions?: React.ReactNode;
}): React.JSX.Element {
  const directory = useAgentDirectory();
  const deny = rule.effect === "deny";
  const description = rule.description?.trim() || "";
  const title = description || rule.id;
  const groups = rule.groups ?? [];
  const subjects = rule.subjects ?? [];
  const agents = rule.agents ?? [];
  const roles = rule.roles ?? [];
  const labels = Object.entries(rule.match_labels ?? {});
  const who = groups.length || subjects.length ? (
    <span className="cc-chips">
      {groups.map((group) => <Chip key={`group:${group}`} title={`Group ${group}`}>{`${group} group`}</Chip>)}
      {subjects.map((subject) => <Chip key={`subject:${subject}`} title={subject}>{subject}</Chip>)}
    </span>
  ) : "Everyone";
  const what = rule.actions.length ? (
    <span className="cc-chips">
      {rule.actions.map((action) => (
        <Chip key={action} mono={!isKnownAccessAction(action)} title={action}>{accessActionLabel(action)}</Chip>
      ))}
    </span>
  ) : "No actions";
  const on = agents.length || roles.length || labels.length ? (
    <span className="cc-chips">
      {agents.map((agent) => <Chip key={`agent:${agent}`} title={agent}>{agentDisplayName(agent, directory)}</Chip>)}
      {roles.map((role) => <Chip key={`role:${role}`} title={`Role ${role}`}>{`${role} role`}</Chip>)}
      {labels.map(([key, value]) => <Chip key={`label:${key}`} mono title={`Label ${key}=${value}`}>{`${key}=${value}`}</Chip>)}
    </span>
  ) : "All agents";
  return (
    <div className="access-panel__rule" data-effect={deny ? "deny" : "allow"} data-testid={`access-rule:${rule.id}`}>
      <div className="access-panel__rule-head">
        <h3 className="access-panel__rule-title" title={title}>{title}</h3>
        <Chip tone={deny ? "crit" : "ok"}>{deny ? "Deny" : "Allow"}</Chip>
      </div>
      {description ? <TechnicalId value={rule.id} label="Rule ID" copy={false} className="access-panel__rule-id" /> : null}
      <KeyValueList
        items={[
          { label: "Who", value: who },
          { label: "What", value: what },
          { label: "On", value: on },
        ]}
      />
      <TechnicalDetails
        className="access-panel__rule-tech"
        items={[
          rule.actions.length ? { label: "Permission keys", value: rule.actions.join(", "), plain: true } : null,
        ]}
      />
      {actions ? <div className="access-panel__rule-actions">{actions}</div> : null}
    </div>
  );
}

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
  const tabIds = React.useId();
  const [ruleDraft, setRuleDraft] = React.useState<RuleDraft | null>(null);
  // The rule an open editor replaces in place; null for a new rule.
  const [editingRuleId, setEditingRuleId] = React.useState<string | null>(null);
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
  const actionsLabelId = React.useId();

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
    setRuleDraft(null); setEditingRuleId(null); setAdminsDraft(null); setGroupNameDraft("");
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
  const admins = config?.admins || [];
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

  function closeRuleEditor() {
    setRuleDraft(null); setEditingRuleId(null); setRuleBase(null);
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

  const refreshButton = (
    <Button icon={<KitGlyph name="refresh" />} onClick={onRefresh} data-testid="access-refresh">
      Refresh
    </Button>
  );

  if (!mayView) return (
    <PanelShell className="access-panel" testId="access-panel">
      <PanelHeader title="Console access" actions={refreshButton} />
      <PanelBody>
        {loading ? (
          <Notice tone="info" role="status">Loading Console access...</Notice>
        ) : status?.available === false ? (
          <EmptyState
            kind="unavailable"
            role="status"
            title="Not available here"
            body="Console access administration is not available on this runtime."
          />
        ) : (
          <EmptyState
            kind="no-access"
            role="status"
            title="Administrators only"
            body="Current administrator access is required to view this configuration."
          />
        )}
      </PanelBody>
    </PanelShell>
  );

  const subtitle = joinMeta(
    config ? plural(rules.length, "rule") : null,
    config ? plural(groups.length, "group") : null,
    status?.subject ? `Signed in as ${status.subject}` : null,
  );
  const reviewNeeded = requiresReview && canReview;
  const alertText = error || (saveFailure ? accessSaveNotice(saveFailure) : null);
  const groupFormShown = canEdit || groupBase !== null;
  const groupEditedInPlace = editingGroup !== null && groups.some(([name]) => name === editingGroup);
  const ruleEditedInPlace = ruleDraft !== null && editingRuleId !== null && rules.some((rule) => rule.id === editingRuleId);

  const groupForm = (
    <section className="access-panel__editor" aria-label={editingGroup ? `Edit group ${editingGroup}` : "New group"}>
      <h3 className="access-panel__editor-title">{editingGroup ? "Edit group" : "New group"}</h3>
      {/* Member inspection keeps this tab's fieldset enabled, so a
          retained draft is disabled here until it can be saved. */}
      <div className="access-panel__form">
        <TextField
          label="Group name"
          testId="access-group-name"
          value={groupNameDraft}
          onChange={(value) => { if (!groupBase) setGroupBase(captureBase()); setGroupNameDraft(value); }}
          placeholder="ops"
          disabled={!canEdit || editingGroup !== null}
        />
        <TextField
          label="Members"
          hint="Subjects, separated by commas."
          testId="access-group-members"
          value={groupMembersDraft}
          onChange={(value) => { if (!groupBase) setGroupBase(captureBase()); setGroupMembersDraft(value); }}
          placeholder="alice@example.com, bob@example.com"
          disabled={!canEdit}
          wide
        />
      </div>
      <div className="access-panel__form-actions">
        <Button variant="primary" data-testid="access-group-save" onClick={submitGroup} disabled={!canEdit}>
          {editingGroup ? "Save members" : "Create group"}
        </Button>
        {editingGroup ? (
          <Button variant="ghost" onClick={() => { setEditingGroup(null); setGroupNameDraft(""); setGroupMembersDraft(""); setGroupBase(null); }} disabled={!canEdit}>
            Cancel
          </Button>
        ) : null}
      </div>
    </section>
  );

  const ruleEditor = ruleDraft ? (
    <div className="access-panel__rule access-panel__rule--editor" data-testid="access-rule-editor">
      <h3 className="access-panel__editor-title">{editingRuleId !== null ? "Edit rule" : "New rule"}</h3>
      <SectionHeader as="h4" label="Rule" className="access-panel__form-head" />
      <div className="access-panel__form">
        <TextField
          label="Rule ID"
          testId="access-rule-id"
          value={ruleDraft.id}
          // An existing rule keeps its ID: saving under a new ID would add a
          // second rule and leave this one in force.
          readOnly={editingRuleId !== null}
          hint={editingRuleId !== null ? "An existing rule keeps its ID. To rename it, create a new rule and delete this one." : undefined}
          onChange={(value) => setRuleDraft({ ...ruleDraft, id: value })}
          placeholder="ops-view-all"
        />
        <TextField
          label="Description"
          value={ruleDraft.description}
          onChange={(value) => setRuleDraft({ ...ruleDraft, description: value })}
          placeholder="Ops can see every agent"
        />
        <SelectField
          label="Effect"
          className="access-panel__field"
          testId="access-rule-effect"
          value={ruleDraft.effect}
          onChange={(value) => setRuleDraft({ ...ruleDraft, effect: value === "deny" ? "deny" : "allow" })}
          options={[{ value: "allow", label: "Allow" }, { value: "deny", label: "Deny" }]}
        />
      </div>
      <SectionHeader as="h4" label="Who" className="access-panel__form-head" />
      <div className="access-panel__form">
        <TextField
          label="Groups"
          hint="Separated by commas."
          testId="access-rule-groups"
          value={ruleDraft.groups}
          onChange={(value) => setRuleDraft({ ...ruleDraft, groups: value })}
          placeholder="ops"
        />
        <TextField
          label="Subjects"
          hint="Email addresses, separated by commas."
          value={ruleDraft.subjects}
          onChange={(value) => setRuleDraft({ ...ruleDraft, subjects: value })}
          placeholder="alice@example.com"
        />
      </div>
      <p className="access-panel__form-note">With no groups and no subjects the rule covers everyone.</p>
      <SectionHeader as="h4" id={actionsLabelId} label="What" className="access-panel__form-head" />
      <div className="access-panel__actions" role="group" aria-labelledby={actionsLabelId}>
        {actions.map((action) => {
          const selected = ruleDraft.actions.includes(action);
          return (
            <button
              key={action}
              type="button"
              className={cx("access-panel__action", selected && "is-active")}
              aria-pressed={selected}
              data-selected={selected ? "true" : "false"}
              data-testid={`access-rule-action:${action}`}
              title={action}
              onClick={() =>
                setRuleDraft({
                  ...ruleDraft,
                  actions: selected
                    ? ruleDraft.actions.filter((candidate) => candidate !== action)
                    : [...ruleDraft.actions, action],
                })
              }
            >
              {selected ? <KitGlyph name="check" className="access-panel__action-mark" /> : null}
              {accessActionLabel(action)}
            </button>
          );
        })}
      </div>
      <SectionHeader as="h4" label="On" className="access-panel__form-head" />
      <div className="access-panel__form">
        <TextField
          label="Agents"
          hint="Agent identities, separated by commas."
          testId="access-rule-agents"
          value={ruleDraft.agents}
          onChange={(value) => setRuleDraft({ ...ruleDraft, agents: value })}
          placeholder={agents.slice(0, 2).map((agent) => agent.identity).join(", ") || "identity:ops-lead"}
        />
        <TextField
          label="Roles"
          hint="Separated by commas."
          value={ruleDraft.roles}
          onChange={(value) => setRuleDraft({ ...ruleDraft, roles: value })}
          placeholder="analyst"
        />
        <TextField
          label="Labels"
          hint="key=value pairs, separated by commas."
          value={ruleDraft.matchLabels}
          onChange={(value) => setRuleDraft({ ...ruleDraft, matchLabels: value })}
          placeholder="org=payments"
        />
      </div>
      <p className="access-panel__form-note">Leave agents, roles and labels empty to cover every agent.</p>
      <div className="access-panel__form-actions">
        <Button
          variant="primary"
          data-testid="access-rule-save"
          disabled={!canEdit || !ruleDraft.id.trim() || ruleDraft.actions.length === 0 || ruleDraft.actions.some(action => !actions.includes(action))}
          onClick={() => {
            void mutate("rule", ruleBase, base => onUpsertRule(ruleFromDraft(ruleDraft), base), closeRuleEditor);
          }}
        >
          Save rule
        </Button>
        <Button variant="ghost" onClick={closeRuleEditor}>Cancel</Button>
      </div>
    </div>
  ) : null;

  return (
    <PanelShell className="access-panel" testId="access-panel">
      <PanelHeader
        title="Console access"
        subtitle={
          <>
            {config ? (
              <Chip tone={enabled ? "ok" : "neutral"} dot className="access-panel__state">
                {enabled ? "Enforcing" : "Not enforced"}
              </Chip>
            ) : null}
            {subtitle}
          </>
        }
        actions={refreshButton}
        divider={false}
      />
      <p className="access-panel__intro">
        Manage access to Meta console and runtime surfaces. These rules do not grant native tool, source or connected-account permissions.
      </p>
      <PanelTabs<Tab>
        ariaLabel="Console access sections"
        idBase={tabIds}
        active={tab}
        onSelect={setTab}
        tabs={(["overview", "groups", "rules", "preview"] as Tab[]).map((candidate) => ({
          id: candidate,
          label: TAB_LABELS[candidate],
          count: candidate === "groups" ? groups.length : candidate === "rules" ? rules.length : null,
          testId: `access-tab:${candidate}`,
        }))}
      />
      {/* Owner-state feedback comes and goes with every refresh, and stream
          events start refreshes at any moment. It renders below the tabs so a
          refresh never moves them under a click in progress. */}
      {readOnly || loading || alertText ? (
        <div className="access-panel__notices">
          {readOnly ? <Notice tone="locked" role="status" flush>This connection is read-only.</Notice> : null}
          {loading ? <Notice tone="info" role="status" flush>Refreshing owner state. Changes are temporarily unavailable.</Notice> : null}
          {alertText ? (
            <Notice
              tone="crit"
              role="alert"
              flush
              action={reviewNeeded ? <Button variant="primary" size="sm" onClick={reviewAndReapply}>Review and reapply</Button> : null}
            >
              <span data-testid="access-error">{alertText}</span>
              {error ? (
                <span className="access-panel__alert-hint">Last loaded configuration may be out of date. Refresh before making changes.</span>
              ) : null}
            </Notice>
          ) : null}
        </div>
      ) : null}
      <PanelBody tabPanel={{ idBase: tabIds, tab }}>
        <fieldset className="access-panel__fieldset" disabled={tab === "preview" ? !current : tab === "groups" ? !current || saving : !canEdit}>
          {tab === "overview" ? (
            <>
              <PanelSection
                label="Enforcement"
                padded
                action={canEdit ? (
                  <Button
                    data-testid="access-toggle-enabled"
                    onClick={() => void mutate("immediate", captureBase(), base => onSetEnabled(!enabled, base))}
                  >
                    {enabled ? "Disable enforcement" : "Enable enforcement"}
                  </Button>
                ) : null}
              >
                <p className="access-panel__text">
                  {enabled
                    ? "Deny by default: every console caller only sees and operates what a rule (or admin standing) grants."
                    : "Access control is configured but not enforced. Enabling requires at least one admin subject."}
                </p>
              </PanelSection>
              <PanelSection
                label="Admins"
                count={admins.length}
                action={canEdit && adminsDraft === null ? (
                  <Button
                    data-testid="access-edit-admins"
                    onClick={() => { setAdminsBase(captureBase()); setAdminsDraft(formatListInput(config?.admins)); }}
                  >
                    Edit admins
                  </Button>
                ) : null}
              >
                <p className="access-panel__text access-panel__text--inset">
                  Admin subjects bypass every rule and manage this configuration.
                </p>
                {adminsDraft === null ? (
                  admins.length === 0 ? (
                    <EmptyState inline title="No admins configured" />
                  ) : (
                    <RowList>
                      {admins.map((admin) => <SubjectRow key={admin} subject={admin} />)}
                    </RowList>
                  )
                ) : (
                  <div className="access-panel__inset">
                    <div className="access-panel__form">
                      <TextField
                        label="Admin subjects"
                        hint="Separated by commas."
                        testId="access-admins-input"
                        value={adminsDraft}
                        onChange={setAdminsDraft}
                        placeholder="root@example.com, ops-lead@example.com"
                        wide
                      />
                    </div>
                    <div className="access-panel__form-actions">
                      <Button
                        variant="primary"
                        data-testid="access-save-admins"
                        onClick={() => {
                          void mutate("admins", adminsBase, base => onSaveAdmins(parseListInput(adminsDraft), base), () => { setAdminsDraft(null); setAdminsBase(null); });
                        }}
                      >
                        Save
                      </Button>
                      <Button variant="ghost" onClick={() => { setAdminsDraft(null); setAdminsBase(null); }}>Cancel</Button>
                    </div>
                  </div>
                )}
              </PanelSection>
            </>
          ) : null}

          {tab === "groups" ? (
            <>
              {groups.length === 0 && editingGroup === null ? (
                <EmptyState
                  title="No groups yet"
                  body="Groups gather people so one rule can cover all of them. Create a group, then use it in a rule."
                />
              ) : null}
              {groups.map(([name, group]) =>
                editingGroup === name ? (
                  groupFormShown ? <React.Fragment key={name}>{groupForm}</React.Fragment> : null
                ) : (
                  <section key={name} data-testid={`access-group:${name}`}>
                    <div className="access-panel__group-head">
                      <div className="access-panel__group-text">
                        <h3 className="access-panel__group-name" title={name}>{name}</h3>
                        <span className="access-panel__group-meta">{plural((group.members || []).length, "member")}</span>
                      </div>
                      {canEdit ? (
                        <div className="cc-btn-row">
                          <Button
                            size="sm"
                            data-testid={`access-group-edit:${name}`}
                            onClick={() => startGroupEdit(name, group.members)}
                          >
                            Edit members
                          </Button>
                          <Button
                            size="sm"
                            variant="danger"
                            data-testid={`access-group-delete:${name}`}
                            onClick={() => {
                              if (window.confirm(`Delete group "${name}"?`)) {
                                void mutate("immediate", captureBase(), base => onDeleteGroup(name, base));
                              }
                            }}
                          >
                            Delete
                          </Button>
                        </div>
                      ) : null}
                    </div>
                    {group.description ? <p className="access-panel__text access-panel__text--inset">{group.description}</p> : null}
                    {(group.members || []).length === 0 ? (
                      <EmptyState inline title="No members" />
                    ) : (
                      <RowList>
                        {(group.members || []).map((member) => (
                          <SubjectRow
                            key={member}
                            subject={member}
                            actions={current && !saving ? (
                              <Button
                                variant="ghost"
                                size="sm"
                                aria-label={`Inspect access for ${member}`}
                                onClick={() => inspectSubject(member)}
                              >
                                Inspect access
                              </Button>
                            ) : null}
                          />
                        ))}
                      </RowList>
                    )}
                  </section>
                ),
              )}
              {groupFormShown && !groupEditedInPlace ? groupForm : null}
            </>
          ) : null}

          {tab === "rules" ? (
            <>
              {canEdit && !ruleDraft ? (
                <Toolbar className="access-panel__toolbar">
                  <Button
                    variant="primary"
                    data-testid="access-rule-new"
                    onClick={() => { setRuleBase(captureBase()); setEditingRuleId(null); setRuleDraft({ ...emptyRuleDraft(), actions: actions.slice(0, 1) }); }}
                  >
                    New rule
                  </Button>
                </Toolbar>
              ) : null}
              {rules.length === 0 && !ruleDraft ? (
                <EmptyState
                  title="No rules yet"
                  body="While enforcement is on, only admins can see or do anything until a rule grants access."
                />
              ) : null}
              {ruleDraft || rules.length ? (
                <div className="access-panel__rules">
                  {ruleEditedInPlace ? null : ruleEditor}
                  {rules.map((rule) =>
                    ruleEditedInPlace && rule.id === editingRuleId ? (
                      <React.Fragment key={rule.id}>{ruleEditor}</React.Fragment>
                    ) : (
                      <RuleCard
                        key={rule.id}
                        rule={rule}
                        actions={canEdit ? (
                          <>
                            <Button
                              size="sm"
                              data-testid={`access-rule-edit:${rule.id}`}
                              onClick={() => { setRuleBase(captureBase()); setEditingRuleId(rule.id); setRuleDraft(draftFromRule(rule)); }}
                            >
                              Edit
                            </Button>
                            <Button
                              size="sm"
                              variant="danger"
                              data-testid={`access-rule-delete:${rule.id}`}
                              onClick={() => {
                                if (window.confirm(`Delete rule "${rule.id}"? Access it grants (or denies) stops immediately.`)) {
                                  void mutate("immediate", captureBase(), base => onDeleteRule(rule.id, base));
                                }
                              }}
                            >
                              Delete
                            </Button>
                          </>
                        ) : null}
                      />
                    ),
                  )}
                </div>
              ) : null}
            </>
          ) : null}

          {tab === "preview" ? (
            <PanelSection label="Check access as someone else" padded>
              <p className="access-panel__text">
                Check what a person or service may do in this console.
              </p>
              <div className="access-panel__form">
                <TextField
                  label="Subject"
                  testId="access-preview-subject"
                  inputRef={previewSubjectInput}
                  value={previewSubject}
                  onChange={setPreviewSubject}
                  placeholder="alice@example.com"
                />
                <SelectField
                  label="Action"
                  className="access-panel__field"
                  testId="access-preview-action"
                  value={previewAction}
                  onChange={(value) => setPreviewAction(value)}
                  options={actions.map((action) => ({ value: action, label: accessActionLabel(action) }))}
                />
                <SelectField
                  label="Agent"
                  className="access-panel__field"
                  testId="access-preview-agent"
                  value={previewIdentity}
                  onChange={(value) => setPreviewIdentity(value)}
                  options={[
                    { value: "", label: "Any agent" },
                    ...agents.map((agent) => ({ value: agent.identity, label: agent.label || agent.identity })),
                  ]}
                />
              </div>
              <div className="access-panel__form-actions">
                <Button
                  variant="primary"
                  data-testid="access-preview-run"
                  disabled={!current || !previewSubject.trim() || !actions.includes(previewAction) || previewPending}
                  onClick={() => void runPreview()}
                >
                  Evaluate
                </Button>
              </div>
              {previewError ? (
                <Notice tone="warn" role="status" testId="access-preview-error" className="access-panel__result">
                  {previewError}
                </Notice>
              ) : null}
              {previewResult ? (
                <div
                  className="access-panel__result"
                  data-testid="access-preview-result"
                  data-allowed={previewResult.allowed ? "true" : "false"}
                >
                  <Notice tone={previewResult.allowed ? "ok" : "crit"}>
                    <span className="access-panel__verdict">{previewResult.allowed ? "Allowed" : "Denied"}</span>
                    {" "}
                    {joinMeta(
                      previewResult.reason,
                      previewResult.is_admin ? "Admin" : null,
                      previewResult.groups?.length ? `Groups ${previewResult.groups.join(", ")}` : null,
                    )}
                    <span className="access-panel__alert-hint">Observation only, not permission for a later action.</span>
                  </Notice>
                </div>
              ) : null}
            </PanelSection>
          ) : null}
        </fieldset>
      </PanelBody>
    </PanelShell>
  );
}
