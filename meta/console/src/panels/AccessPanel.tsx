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
import { accessActionLabel, isAccessActionPattern, isKnownAccessAction } from "../lib/access-actions";
import {
  AgentAvatar,
  Button,
  Chip,
  DetailDrawer,
  DrawerLayout,
  DrawerSection,
  EmptyState,
  IconButton,
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
  StatusMark,
  StatusPill,
  TechnicalDetails,
  TechnicalId,
  Toolbar,
  agentDisplayName,
  cx,
  joinMeta,
  plural,
  standardPanelMenu,
  useAgentDirectory,
  usePanelDensity,
  useShowTechnicalIds,
  type SummaryFact,
  type Tone,
} from "../../../packages/console-components/src/panel-kit";
import {
  accessApprovers,
  actionPatternIsKnown,
  cellVerdict,
  checkSubject,
  effectiveRules,
  familiesFor,
  matrixRows,
  ruleCoverage,
  type AccessFamily,
  type AccessPrincipal,
  type ActionVerdict,
  type CellState,
} from "./access-model";

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
  /** Asks the owner what a person (an empty `subject`: someone not signed
   *  in) may do. */
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

/** A rule's title: its description, or "Unnamed rule". The rule ID is
 * secondary: a muted line under the title. */
export function ruleTitle(rule: Pick<ConsoleAccessRule, "description">): string {
  return rule.description?.trim() || "Unnamed rule";
}

/** The delete confirmation: the rule by its description (or as unnamed),
 * with its ID only as the parenthetical that tells two rules apart. */
export function ruleDeleteConfirmText(rule: Pick<ConsoleAccessRule, "id" | "description">): string {
  const description = rule.description?.trim();
  const which = description ? `the rule "${description}" (ID ${rule.id})` : `this unnamed rule (ID ${rule.id})`;
  return `Delete ${which}? Access it grants (or denies) stops immediately.`;
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
  ruleTitle,
  ruleDeleteConfirmText,
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

/** How a rule sits among the others: an allow rule a deny rule takes
 * permissions from, or a deny rule that overrides allow rules. Nothing when
 * no rule overlaps it. */
function RuleCoverageLine({ rule, rules, catalog }: { rule: ConsoleAccessRule; rules: readonly ConsoleAccessRule[]; catalog: readonly string[] }): React.JSX.Element | null {
  const coverage = ruleCoverage(rule, rules, catalog);
  const names = (overlap: { rule: ConsoleAccessRule }) => `"${ruleTitle(overlap.rule)}"`;
  const actions = (list: readonly string[]) => {
    const words = list.slice(0, 3).map(accessActionLabel);
    return list.length > 3 ? `${words.join(", ")} and ${list.length - 3} more` : words.join(", ");
  };
  let tone: Tone;
  let text: string;
  if (coverage.shadowed) {
    tone = "warn";
    text = `Shadowed by deny rule ${coverage.deniedBy.map(names).join(" and ")}: it grants nothing.`;
  } else if (coverage.deniedBy.length) {
    tone = "warn";
    text = coverage.deniedBy.map((overlap) => `${actions(overlap.actions)} denied by ${names(overlap)}`).join("; ");
    text = `Partly overridden: ${text}.`;
  } else if (coverage.overrides.length) {
    tone = "info";
    text = `Overrides ${coverage.overrides.map((overlap) => `${names(overlap)} for ${actions(overlap.actions)}`).join("; ")}.`;
  } else {
    return null;
  }
  return (
    <p className="access-panel__coverage" data-tone={tone} data-testid={`access-rule-coverage:${rule.id}`}>
      <StatusMark tone={tone} />
      <span>{text}</span>
    </p>
  );
}

/** A rule as a card: what it does in words, who it covers, what it allows
 * or denies and where, and how it overlaps the other rules. A rule without
 * a description is an "Unnamed rule"; the rule ID is always a muted
 * secondary line. */
function RuleCard({
  rule,
  rules,
  catalog,
  actions,
}: {
  rule: ConsoleAccessRule;
  rules: readonly ConsoleAccessRule[];
  catalog: readonly string[];
  actions?: React.ReactNode;
}): React.JSX.Element {
  const directory = useAgentDirectory();
  const deny = rule.effect === "deny";
  const description = rule.description?.trim() || "";
  const title = ruleTitle(rule);
  const groups = rule.groups ?? [];
  const subjects = rule.subjects ?? [];
  const agents = rule.agents ?? [];
  const roles = rule.roles ?? [];
  const labels = Object.entries(rule.match_labels ?? {});
  const who = groups.length || subjects.length ? (
    <span className="cc-chips">
      {groups.map((group) => <Chip key={`group:${group}`} title={`Group ${group}`}>{`${group} group`}</Chip>)}
      {subjects.map((subject) => <Chip key={`subject:${subject}`} title={subject}>{subject === "*" ? "Anyone signed in" : subject}</Chip>)}
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
      {agents.map((agent) => <Chip key={`agent:${agent}`} title={agent}>{agent === "*" ? "Any agent" : agentDisplayName(agent, directory)}</Chip>)}
      {roles.map((role) => <Chip key={`role:${role}`} title={`Role ${role}`}>{`${role} role`}</Chip>)}
      {labels.map(([key, value]) => <Chip key={`label:${key}`} mono title={`Label ${key}=${value}`}>{`${key}=${value}`}</Chip>)}
    </span>
  ) : "All agents";
  return (
    <div className="access-panel__rule" data-effect={deny ? "deny" : "allow"} data-testid={`access-rule:${rule.id}`}>
      <div className="access-panel__rule-head">
        <h3 className="access-panel__rule-title" title={description || `Rule ${rule.id}`}>{title}</h3>
        <StatusPill tone={deny ? "crit" : "ok"}>{deny ? "Deny" : "Allow"}</StatusPill>
      </div>
      <TechnicalId value={rule.id} label="Rule ID" copy={false} className="access-panel__rule-id" />
      <KeyValueList
        items={[
          { label: "Who", value: who },
          { label: "What", value: what },
          { label: "On", value: on },
        ]}
      />
      <RuleCoverageLine rule={rule} rules={rules} catalog={catalog} />
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

const CELL_WORDS: Record<CellState, string> = {
  all: "allowed on every agent",
  some: "partly allowed",
  none: "not allowed",
  denied: "denied by a rule",
};

const CELL_TONE: Record<CellState, Tone> = { all: "ok", some: "ok", none: "neutral", denied: "crit" };

/** A matrix cell's mark: a filled circle (allowed on every agent), a half
 * filled circle (partly), a hollow circle (not allowed), a diamond (denied
 * by a rule). The words are in the cell's name and tooltip. */
function CellMark({ state }: { state: CellState }): React.JSX.Element {
  return (
    <svg className="access-matrix__mark" data-state={state} viewBox="0 0 14 14" width="14" height="14" aria-hidden="true" focusable="false">
      {state === "denied" ? (
        <path d="M7 1.6 12.4 7 7 12.4 1.6 7Z" />
      ) : state === "some" ? (
        <>
          <circle cx="7" cy="7" r="5" className="access-matrix__ring" />
          <path d="M7 2a5 5 0 0 0 0 10Z" />
        </>
      ) : (
        <circle cx="7" cy="7" r={state === "all" ? 5.5 : 5} />
      )}
    </svg>
  );
}

function actionWords(verdict: ActionVerdict): string {
  const rule = verdict.rule ? `"${ruleTitle(verdict.rule)}"` : "";
  switch (verdict.state) {
    case "all": return rule ? `On every agent, by ${rule}` : "Admins can do everything";
    case "some": {
      const except = verdict.except?.length ? `, except where ${verdict.except.map((deny) => `"${ruleTitle(deny)}"`).join(" and ")} denies it` : "";
      return `On some agents, by ${rule}${except}`;
    }
    case "denied": return `Denied by ${rule}`;
    default: return "No rule allows it";
  }
}

/** Who can do what (spec B13): a row per admin, group, person a rule names
 * and everyone; a column per family of permissions; a mark per cell. One
 * tab stop: the arrow keys, Home and End move between cells, Enter or
 * Space checks the cell. */
function AccessMatrix({
  config,
  catalog,
  selected,
  onCheck,
}: {
  config: ConsoleAccessConfig;
  catalog: readonly string[];
  selected: string | null;
  onCheck: (row: AccessPrincipal, family: AccessFamily) => void;
}): React.JSX.Element {
  const families = React.useMemo(() => familiesFor(catalog), [catalog]);
  const rules = React.useMemo(() => effectiveRules(config.rules ?? []), [config.rules]);
  const rows = React.useMemo(() => matrixRows(config), [config]);
  const [focus, setFocus] = React.useState<[number, number]>([0, 0]);
  const tableRef = React.useRef<HTMLTableElement | null>(null);
  const noteId = React.useId();
  const groups = config.groups ?? {};
  const move = (row: number, column: number) => {
    const next: [number, number] = [Math.min(rows.length - 1, Math.max(0, row)), Math.min(families.length - 1, Math.max(0, column))];
    setFocus(next);
    tableRef.current?.querySelector<HTMLButtonElement>(`[data-cell="${next[0]}:${next[1]}"]`)?.focus();
  };
  const onKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>, row: number, column: number) => {
    const steps: Record<string, [number, number]> = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] };
    if (event.key in steps) {
      event.preventDefault();
      move(row + steps[event.key][0], column + steps[event.key][1]);
    } else if (event.key === "Home") {
      event.preventDefault();
      move(event.ctrlKey ? 0 : row, 0);
    } else if (event.key === "End") {
      event.preventDefault();
      move(event.ctrlKey ? rows.length - 1 : row, families.length - 1);
    }
  };
  if (!families.length) {
    return <EmptyState inline title="This runtime lists no permissions to show." />;
  }
  return (
    <div className="access-matrix" data-testid="access-matrix">
      <div className="access-matrix__scroll">
        <table className="access-matrix__table" role="grid" aria-label="Who can do what" aria-describedby={noteId} ref={tableRef}>
          <caption className="cc-visually-hidden">Who can do what, computed from the rules. Each cell checks itself with the runtime.</caption>
          <thead>
            <tr>
              <th scope="col" className="access-matrix__corner"><span className="cc-visually-hidden">Who</span></th>
              {families.map((family) => (
                <th key={family.id} scope="col" className="access-matrix__family" title={family.actions.map(accessActionLabel).join(", ")}>
                  {family.label}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, rowIndex) => {
              const members = row.kind === "group" ? (groups[row.groups[0]]?.members ?? []).length : null;
              const sub = row.kind === "admins" ? "can do everything"
                : row.kind === "group" ? plural(members ?? 0, "member")
                  : row.kind === "person" ? (row.groups.length ? `in ${row.groups.join(", ")}` : "named in a rule")
                    : "signed in or not";
              return (
                <tr key={row.id} data-row={row.id}>
                  <th scope="row" className="access-matrix__who">
                    <span className="access-matrix__who-name" title={row.label}>
                      {row.kind === "group" ? `${row.label} group` : row.label}
                    </span>
                    <span className="access-matrix__who-sub">{sub}</span>
                  </th>
                  {families.map((family, columnIndex) => {
                    const verdict = cellVerdict(rules, row, family);
                    const key = `${row.id}:${family.id}`;
                    const words = verdict.state === "some"
                      ? `${CELL_WORDS.some}: ${verdict.actions.filter((action) => action.state === "all" || action.state === "some").length} of ${verdict.actions.length} permissions${verdict.actions.some((action) => action.state === "some") ? ", some on some agents only" : ""}`
                      : CELL_WORDS[verdict.state];
                    const label = `${row.kind === "group" ? `${row.label} group` : row.label}, ${family.label}: ${words}`;
                    return (
                      <td key={family.id} role="gridcell" className="access-matrix__cell" data-state={verdict.state} aria-selected={selected === key}>
                        <button
                          type="button"
                          className={cx("access-matrix__button", selected === key && "is-selected")}
                          data-cell={`${rowIndex}:${columnIndex}`}
                          data-testid={`access-matrix-cell:${row.id}:${family.id}`}
                          data-state={verdict.state}
                          aria-label={label}
                          title={label}
                          tabIndex={focus[0] === rowIndex && focus[1] === columnIndex ? 0 : -1}
                          onFocus={() => setFocus([rowIndex, columnIndex])}
                          onKeyDown={(event) => onKeyDown(event, rowIndex, columnIndex)}
                          onClick={() => onCheck(row, family)}
                        >
                          <CellMark state={verdict.state} />
                        </button>
                      </td>
                    );
                  })}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <div className="access-matrix__foot">
        <span className="access-matrix__legend" aria-hidden="true">
          {(["all", "some", "none", "denied"] as CellState[]).map((state) => (
            <span key={state} className="access-matrix__legend-item">
              <CellMark state={state} />
              {state === "all" ? "allowed on every agent" : state === "some" ? "partly (some agents or permissions)" : state === "none" ? "not allowed" : "denied by a rule"}
            </span>
          ))}
        </span>
        <span id={noteId} className="access-matrix__note">Computed from the rules; click a cell to check it.</span>
      </div>
    </div>
  );
}

interface CheckAnswer {
  action: string;
  allowed?: boolean;
  reason?: string | null;
  error?: boolean;
}

/** The checked answer for one cell (spec B13): the cell's permissions as the
 * rules decide them, then the owner's own answer for each (`verified`), as a
 * person of that row on no agent or on the agent picked. */
function AccessCheckDrawer({
  row,
  family,
  config,
  current,
  agents,
  onPreview,
  onClose,
}: {
  row: AccessPrincipal;
  family: AccessFamily;
  config: ConsoleAccessConfig;
  current: boolean;
  agents: { identity: string; label: string }[];
  onPreview: AccessPanelProps["onPreview"];
  onClose: () => void;
}): React.JSX.Element {
  const rules = React.useMemo(() => effectiveRules(config.rules ?? []), [config.rules]);
  const verdict = cellVerdict(rules, row, family);
  const as = checkSubject(row, config);
  // A permission allowed on some agents only is checked on one of them.
  const firstNamedAgent = verdict.actions.find((action) => action.state === "some")?.rule?.agents?.find((agent) => agent !== "*") ?? "";
  const [identity, setIdentity] = React.useState(firstNamedAgent && agents.some((agent) => agent.identity === firstNamedAgent) ? firstNamedAgent : "");
  const [answers, setAnswers] = React.useState<CheckAnswer[] | null>(null);
  const [checking, setChecking] = React.useState(false);
  const version = React.useRef(0);
  const run = React.useCallback(async () => {
    if (!as || !current) return;
    const mine = ++version.current;
    setChecking(true);
    setAnswers(null);
    const results = await Promise.all(family.actions.map(async (action): Promise<CheckAnswer> => {
      try {
        const result = await onPreview(as.subject ?? "", action, identity || undefined);
        if (result?.allowed === true || result?.allowed === false) return { action, allowed: result.allowed, reason: result.reason };
        return { action, error: true };
      } catch {
        return { action, error: true };
      }
    }));
    if (mine !== version.current) return;
    setAnswers(results);
    setChecking(false);
  }, [as?.subject, current, family.actions, identity, onPreview]); // eslint-disable-line react-hooks/exhaustive-deps
  React.useEffect(() => {
    void run();
    return () => { version.current += 1; };
  }, [run]);
  const computedAllows = (action: ActionVerdict) => (identity
    ? action.state === "all" || (action.state === "some" && Boolean(action.rule?.agents?.includes(identity) || action.rule?.agents?.includes("*")))
    : action.state === "all");
  const differs = answers?.some((answer, index) => answer.allowed !== undefined
    && verdict.actions[index]?.state !== "some" && answer.allowed !== computedAllows(verdict.actions[index]));
  const rowName = row.kind === "group" ? `${row.label} group` : row.label;
  return (
    <DetailDrawer
      className="access-check"
      testId="access-check"
      closeTestId="access-check-close"
      title={`${rowName} · ${family.label}`}
      subtitle="Computed from the rules, checked with the runtime"
      status={<StatusPill tone={CELL_TONE[verdict.state]}>{verdict.state === "some" ? "Partly allowed" : CELL_WORDS[verdict.state].replace(/^./, (c) => c.toUpperCase())}</StatusPill>}
      ariaLabel={`Check access: ${rowName}, ${family.label}`}
      onClose={onClose}
    >
      <DrawerSection
        label="Checked with the runtime"
        action={as ? (
          <Button size="sm" variant="ghost" onClick={() => void run()} disabled={!current || checking} data-testid="access-check-run">
            {checking ? "Checking" : "Check again"}
          </Button>
        ) : null}
      >
        {as ? (
          <>
            <p className="access-check__as">{as.note}</p>
            <SelectField
              label="Agent"
              className="access-panel__field access-check__agent"
              testId="access-check-agent"
              value={identity}
              onChange={(value) => setIdentity(value)}
              options={[
                { value: "", label: "No specific agent" },
                ...agents.map((agent) => ({ value: agent.identity, label: agent.label || agent.identity })),
              ]}
            />
            {!current ? <Notice tone="locked">Checks wait until Console access has loaded.</Notice> : null}
            {differs ? (
              <Notice tone="warn" testId="access-check-differs">
                The runtime's answer differs from the rules shown here: roles, labels or an agent's lineage decide it.
              </Notice>
            ) : null}
          </>
        ) : (
          <EmptyState inline title="No one to check as" body="This group has no members yet, so there is no one to check its access as." />
        )}
      </DrawerSection>
      <DrawerSection label="Permissions" count={verdict.actions.length}>
        <ul className="access-check__list" data-testid="access-check-answers" aria-busy={checking}>
          {verdict.actions.map((action, index) => {
            const answer = answers?.[index];
            const tone: Tone = !answer ? "neutral" : answer.error ? "warn" : answer.allowed ? "ok" : "crit";
            return (
              <li
                key={action.action}
                className="access-check__item"
                data-state={action.state}
                data-testid={`access-check-answer:${action.action}`}
                data-allowed={answer?.allowed === undefined ? undefined : String(answer.allowed)}
              >
                <CellMark state={action.state} />
                <span className="access-check__main">
                  <span className="access-check__action" title={action.action}>{accessActionLabel(action.action)}</span>
                  <span className="access-check__why">{actionWords(action)}</span>
                  {answer?.reason && !answer.error && answer.allowed !== computedAllows(action) ? (
                    <span className="access-check__why access-check__why--runtime">{`The runtime says: ${answer.reason}`}</span>
                  ) : null}
                </span>
                {as ? (
                  <span className="access-check__answer" data-tone={tone} title={answer && !answer.error ? "Checked with the runtime just now" : undefined}>
                    {!answer ? (checking ? "Checking" : "Not checked")
                      : answer.error ? "No answer"
                        : (
                          <>
                            <StatusMark tone={tone} />
                            {answer.allowed ? "Allowed" : "Denied"}
                            <span className="access-check__verified">
                              <KitGlyph name="check" />
                              verified
                            </span>
                          </>
                        )}
                  </span>
                ) : null}
              </li>
            );
          })}
        </ul>
        {as ? <p className="access-check__note">Observation only, not permission for a later action.</p> : null}
      </DrawerSection>
    </DetailDrawer>
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
  const [check, setCheck] = React.useState<{ row: AccessPrincipal; family: AccessFamily } | null>(null);
  const [density, setDensity] = usePanelDensity("access");
  const [showTechnicalIds, setShowTechnicalIds] = useShowTechnicalIds("access");
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
    setAdminsBase(null); setRuleBase(null); setGroupBase(null); setCheck(null);
  }
  // When the shown configuration was read: the moment a read lands.
  const [loadedAt, setLoadedAt] = React.useState<number | null>(null);
  React.useEffect(() => {
    if (!loading && !error && config && status) setLoadedAt(Date.now());
  }, [loading, error, config, status]);
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
  const approvers = accessApprovers(config);
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
    <IconButton label="Refresh access" glyph="refresh" onClick={() => void onRefresh()} data-testid="access-refresh" />
  );
  const menu = {
    label: "More actions for Console access",
    triggerTestId: "access-menu",
    items: standardPanelMenu({
      density,
      onDensityChange: setDensity,
      showTechnicalIds,
      onShowTechnicalIdsChange: setShowTechnicalIds,
      getCopyData: mayView ? () => ({ status, config }) : undefined,
      onRefresh: () => void onRefresh(),
      testIdPrefix: "access",
    }),
  };

  if (!mayView) return (
    <PanelShell className="access-panel" testId="access-panel">
      <PanelHeader title="Console access" tools={refreshButton} menu={menu} />
      <PanelBody>
        {loading ? (
          <Notice tone="info" role="status">Loading Console access...</Notice>
        ) : status?.available === false ? (
          <EmptyState
            kind="unavailable"
            illustration="not-configured"
            role="status"
            title="Not available here"
            body="Console access administration is not available on this runtime."
          />
        ) : (
          <EmptyState
            kind="no-access"
            illustration="no-access"
            role="status"
            title="Administrators only"
            body="Current administrator access is required to view this configuration."
          />
        )}
      </PanelBody>
    </PanelShell>
  );

  const summary: SummaryFact[] = [];
  if (config) {
    summary.push({ key: "rules", text: plural(rules.length, "rule") });
    summary.push({ key: "groups", text: plural(groups.length, "group") });
  }
  summary.push({ key: "who", text: status?.subject ? `Signed in as ${status.subject}` : "Not signed in", announce: false, testId: "access-summary-who" });
  const isAdmin = status?.is_admin === true || Boolean(status?.subject && admins.includes(status.subject));
  summary.push({
    key: "standing",
    text: isAdmin ? "admin" : "you manage access through a rule",
    title: isAdmin ? "You are one of the admins" : "The admins do not include you; a rule lets you manage Console access",
    announce: false,
    testId: "access-summary-standing",
  });
  const reviewNeeded = requiresReview && canReview;
  const alertText = error || (saveFailure ? accessSaveNotice(saveFailure) : null);
  const groupFormShown = canEdit || groupBase !== null;
  const groupEditedInPlace = editingGroup !== null && groups.some(([name]) => name === editingGroup);
  const ruleEditedInPlace = ruleDraft !== null && editingRuleId !== null && rules.some((rule) => rule.id === editingRuleId);
  // Rules that name each group, for its card.
  const groupRules = (name: string) => rules.filter((rule) => (rule.groups ?? []).includes(name));

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
          hint="Email addresses or service names, separated by commas."
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

  // A rule saved with patterns ("*", "agent.*") or with permissions this
  // runtime no longer lists can be edited: each such entry shows as a
  // removable choice, and only an entry the runtime would refuse blocks Save.
  const extraActions = ruleDraft ? ruleDraft.actions.filter((action) => !actions.includes(action)) : [];
  const unknownActions = extraActions.filter((action) => !actionPatternIsKnown(action, actions));
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
          label="People and services"
          hint="Email addresses or service names, separated by commas."
          value={ruleDraft.subjects}
          onChange={(value) => setRuleDraft({ ...ruleDraft, subjects: value })}
          placeholder="alice@example.com"
        />
      </div>
      <p className="access-panel__form-note">With no groups and no people or services, the rule covers everyone.</p>
      <SectionHeader as="h4" id={actionsLabelId} label="What" className="access-panel__form-head" />
      <div className="access-panel__actions" role="group" aria-labelledby={actionsLabelId}>
        {extraActions.map((action) => {
          const unknown = !actionPatternIsKnown(action, actions);
          return (
            <button
              key={action}
              type="button"
              className={cx("access-panel__action", "is-active", "access-panel__action--extra", unknown && "is-unknown")}
              aria-pressed={true}
              aria-label={`Remove ${accessActionLabel(action)}`}
              data-selected="true"
              data-testid={`access-rule-action:${action}`}
              title={unknown ? `${action}: this runtime does not list it` : action}
              onClick={() => setRuleDraft({ ...ruleDraft, actions: ruleDraft.actions.filter((candidate) => candidate !== action) })}
            >
              {unknown ? <StatusMark tone="warn" /> : null}
              {isAccessActionPattern(action) || !unknown ? accessActionLabel(action) : action}
              <KitGlyph name="close" className="access-panel__action-remove" />
            </button>
          );
        })}
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
      {unknownActions.length ? (
        <p className="access-panel__form-note access-panel__form-note--warn" data-testid="access-rule-unknown-actions">
          {`This runtime does not list ${unknownActions.join(", ")}. Remove ${unknownActions.length === 1 ? "it" : "them"} to save the rule.`}
        </p>
      ) : null}
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
          disabled={!canEdit || !ruleDraft.id.trim() || ruleDraft.actions.length === 0 || unknownActions.length > 0}
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

  const tabBody = (
    <PanelBody tabPanel={{ idBase: tabIds, tab }} className="access-panel__body">
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
                  ? "Deny by default: everyone using this console sees and does only what a rule allows. Admins can do everything."
                  : "Access control is set up but not enforced. Turning it on needs at least one admin."}
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
                Admins can do everything, whatever the rules say, and manage these settings.
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
                      label="Admins"
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
            {approvers.length ? (
              <PanelSection label="Approvers" count={approvers.length} testId="access-approvers">
                <p className="access-panel__text access-panel__text--inset">
                  Who may decide an approval that asks for a named approver. Set in the access configuration; shown here read only.
                </p>
                <RowList>
                  {approvers.map((approver) => (
                    <ListRow
                      key={approver.id}
                      dense
                      lead={<KitGlyph name="approvals" />}
                      title={approver.id}
                      titleAttr={approver.id}
                      subtitle={joinMeta(
                        approver.subjects.length ? approver.subjects.join(", ") : null,
                        approver.groups.length ? approver.groups.map((group) => `${group} group`).join(", ") : null,
                      )}
                    />
                  ))}
                </RowList>
              </PanelSection>
            ) : null}
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
                <section key={name} className="access-group" data-testid={`access-group:${name}`}>
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
                    <div className="access-group__members">
                      {(group.members || []).map((member) => (
                        <button
                          key={member}
                          type="button"
                          className="access-member"
                          aria-label={`Inspect access for ${member}`}
                          title={`Inspect access for ${member}`}
                          onClick={() => inspectSubject(member)}
                        >
                          <AgentAvatar name={member} />
                          <span className="access-member__name">{member}</span>
                        </button>
                      ))}
                    </div>
                  )}
                  <p className="access-group__used" data-testid={`access-group-rules:${name}`}>
                    {groupRules(name).length ? (
                      <>
                        <span className="access-group__used-label">Used by</span>
                        {groupRules(name).map((rule) => (
                          <Chip key={rule.id} mark tone={rule.effect === "deny" ? "crit" : "ok"} variant="quiet" title={`Rule ${rule.id}`}>
                            {ruleTitle(rule)}
                          </Chip>
                        ))}
                      </>
                    ) : "No rule uses this group yet."}
                  </p>
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
                      rules={rules}
                      catalog={actions}
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
                              if (window.confirm(ruleDeleteConfirmText(rule))) {
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
                label="Person or service"
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
                  { value: "", label: "No specific agent" },
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
      {/* The matrix reads the rules and checks with the owner, which a
          read-only or refreshing session may do too: it stays outside the
          edit fieldset. */}
      {tab === "overview" && config ? (
        <PanelSection label="Who can do what" testId="access-matrix-section">
          <AccessMatrix
            config={config}
            catalog={actions}
            selected={check ? `${check.row.id}:${check.family.id}` : null}
            onCheck={(row, family) => setCheck({ row, family })}
          />
        </PanelSection>
      ) : null}
    </PanelBody>
  );

  return (
    <PanelShell className="access-panel" testId="access-panel" density={density} showTechnicalIds={showTechnicalIds}>
      <PanelHeader
        title="Console access"
        scope={config ? (
          <StatusPill tone={enabled ? "ok" : "neutral"} className="access-panel__state" testId="access-state">
            {enabled ? "Enforcing" : "Not enforced"}
          </StatusPill>
        ) : undefined}
        summary={summary}
        summaryTestId="access-summary"
        freshness={loadedAt ? { at: loadedAt, updating: loading } : undefined}
        refreshing={loading && Boolean(config)}
        tools={refreshButton}
        menu={menu}
        divider={false}
      />
      <p className="access-panel__intro">
        Manage who can see and do what in this console and the runtime behind it. These rules do not give anyone access to agents' tools, data sources or connected accounts.
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
      <div className="access-panel__workspace">
        <DrawerLayout
          main={tabBody}
          open={Boolean(check && tab === "overview" && config)}
          onClose={() => setCheck(null)}
          persistKey="access"
          backLabel="Who can do what"
          backTestId="access-check-back"
          drawer={check && tab === "overview" && config ? (
            <AccessCheckDrawer
              key={`${check.row.id}:${check.family.id}`}
              row={check.row}
              family={check.family}
              config={config}
              current={current}
              agents={agents}
              onPreview={onPreview}
              onClose={() => setCheck(null)}
            />
          ) : null}
        />
      </div>
    </PanelShell>
  );
}
