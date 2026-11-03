import React from "react";
import { CopyButton } from "../../../packages/console-components/src/copy-button";
import { CopyGlyph } from "../../../packages/console-components/src/copy-glyph";
import {
  AgentAvatar,
  Button,
  Chip,
  Countdown,
  DetailDrawer,
  DrawerLayout,
  DrawerSection,
  EmptyState,
  IconButton,
  KeyValueList,
  KitGlyph,
  ListRow,
  ProgressRing,
  RelativeTime,
  RowList,
  StatusDot,
  StatusPill,
  TechnicalDetails,
  formatAbsoluteTime,
  humanizeEnum,
  joinMeta,
  plural,
  priorityTone,
  statusTone,
  useAgentDirectory,
  useNow,
} from "../../../packages/console-components/src/panel-kit";
import type { AgentAvatarSize, TechnicalDetail } from "../../../packages/console-components/src/panel-kit";
import { GraphView, StatusSquare, ZOOM, itemCardGeometry } from "../../../packages/console-components/src/viz";
import type { GraphViewHandle, VizNode, VizNodeBox, VizRelation, VizViewport } from "../../../packages/console-components/src/viz";

import type { WorkGraphWireBinding, WorkGraphWireEdge, WorkGraphWireEvent, WorkGraphWireItem } from "../types";
import {
  buildWorkGraphGraph,
  buildWorkGraphModel,
  isWorkGraphTerminal,
  workGraphBindingStateText,
  workGraphBindingStatusLabel,
  workGraphBindingTarget,
  workGraphCompletionPolicyText,
  workGraphDoneBy,
  workGraphEventItemId,
  workGraphEventSentence,
  workGraphGoalRevisionOf,
  workGraphItemOwner,
  workGraphModeWord,
  workGraphProgressText,
  workGraphStatusWord,
} from "../lib/workgraph-layout";
import type { WorkGraphModel, WorkGraphModelItem, WorkGraphNodeData, WorkGraphOwnerView } from "../lib/workgraph-layout";

/// The WorkGraph graph view (console v2 spec B2): goal frames holding their
/// items as a left-to-right DAG over "blocks" edges, the critical path in the
/// accent, and a drawer for the selected item or goal. The kit's GraphView
/// draws and navigates (one tab stop, arrow keys, Enter opens, zoom and pan
/// that a refresh never resets); this view maps the model onto it and owns
/// the drawers and their actions.

// ── Action handlers ──────────────────────────────────────────────────────

/// An absent revision means the snapshot never carried one: handlers must
/// resolve the live revision before mutating (never substitute 0).
export interface WorkGraphItemHandlers {
  onClaim?: (input: { itemId: string; revision?: number }) => void;
  onClose?: (input: { itemId: string; revision?: number }) => void;
  /// Hands a claimed item back (in progress to open).
  onRelease?: (input: { itemId: string; revision?: number }) => void;
}

/// Two CAS classes on the binding actions: goal actions (confirm /
/// request-close) carry the goal WORK ITEM's revision; attention actions
/// (pause / resume / reassign) carry the binding's machine revision.
export interface WorkGraphBindingHandlers {
  onGoalConfirm?: (input: { bindingId: string; revision?: number }) => void;
  onGoalRequestClose?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionPause?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionResume?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionReassign?: (input: { bindingId: string; revision?: number; identity: string }) => void;
}

export type WorkGraphActionHandlers = WorkGraphItemHandlers & WorkGraphBindingHandlers;

// ── People ───────────────────────────────────────────────────────────────

/// A person by name with the raw key in the tooltip: the kit's AgentName
/// markup, for names the work graph resolves itself (a stored display name
/// is preferred over a bare key the directory does not know).
export function WorkGraphPerson({
  person,
  avatar,
  className,
}: {
  person: WorkGraphOwnerView;
  avatar?: AgentAvatarSize;
  className?: string;
}): React.JSX.Element {
  return (
    <span className={className ? `cc-agent ${className}` : "cc-agent"} title={person.key || undefined}>
      {avatar ? <AgentAvatar name={person.name} size={avatar} /> : null}
      <span className="cc-agent__name">{person.name}</span>
    </span>
  );
}

// ── Item actions ─────────────────────────────────────────────────────────

/// Claim and Done for one item, or null when neither applies: Claim for an
/// open item nobody owns, Done for anything not finished and not blocked;
/// with `release`, Release for an item in progress (the drawer).
export function workGraphItemActions(
  item: WorkGraphWireItem,
  itemId: string,
  canManage: boolean,
  { onClaim, onClose, onRelease }: WorkGraphItemHandlers,
  options: { variant?: "ghost" | "default"; release?: boolean } = {},
): React.ReactNode {
  const status = item.status || "open";
  const revision = typeof item.revision === "number" ? item.revision : undefined;
  const title = item.title || "this work item";
  const claim = canManage && onClaim && status === "open" && !workGraphItemOwner(item);
  const close = canManage && onClose && !isWorkGraphTerminal(status) && status !== "blocked";
  const release = options.release && canManage && onRelease && status === "in_progress";
  if (!claim && !close && !release) return null;
  const variant = options.variant ?? "ghost";
  return (
    <>
      {release ? (
        <Button
          size="sm"
          variant={variant}
          title={`Hand ${title} back so someone else can take it`}
          data-testid={`workgraph-panel-action:${itemId}:release`}
          onClick={() => onRelease?.({ itemId, revision })}
        >
          Release
        </Button>
      ) : null}
      {claim ? (
        <Button
          size="sm"
          variant={variant}
          title={`Claim ${title}`}
          data-testid={`workgraph-panel-action:${itemId}:claim`}
          onClick={() => onClaim?.({ itemId, revision })}
        >
          Claim
        </Button>
      ) : null}
      {close ? (
        <Button
          size="sm"
          variant={variant}
          title={`Mark ${title} as done`}
          data-testid={`workgraph-panel-action:${itemId}:close`}
          onClick={() => onClose?.({ itemId, revision })}
        >
          Done
        </Button>
      ) : null}
    </>
  );
}

// ── Attention bindings ───────────────────────────────────────────────────

/// One attention binding: what the agent does with the goal, its state and
/// who it rests on, then the actions (Pause or Resume, Confirm, Request
/// close, Reassign for a coordinating binding).
export function WorkGraphAttentionRow({
  binding,
  goalRevision,
  canManage,
  subject,
  handlers,
}: {
  binding: WorkGraphWireBinding;
  /// Bound goal work item's revision: the CAS token for confirm/request-close.
  goalRevision?: number;
  canManage: boolean;
  /// What the binding is on, when it does not sit with that item.
  subject?: React.ReactNode;
  handlers: WorkGraphBindingHandlers;
}): React.JSX.Element | null {
  const directory = useAgentDirectory();
  const now = useNow();
  const inputId = React.useId();
  const [reassignOpen, setReassignOpen] = React.useState(false);
  const [reassignIdentity, setReassignIdentity] = React.useState("");
  const bindingId = binding.binding_id || "";
  const revision = binding.machine_state?.revision;
  const statusLabel = workGraphBindingStatusLabel(binding);
  const isActive = statusLabel === "active";
  const isPaused = statusLabel.startsWith("paused");
  const live = isActive || isPaused;
  // Reassign authority is machine-derived from the binding mode upstream:
  // only coordinate-mode bindings can reassign, so others get no affordance.
  const canReassign = live && binding.mode === "coordinate";
  const bindingInput = { bindingId, revision };
  const goalInput = { bindingId, revision: goalRevision };
  if (!bindingId) return null;
  const state = binding.status?.state || "active";
  const target = workGraphBindingTarget(binding, directory);
  const { onAttentionPause, onAttentionResume, onGoalConfirm, onGoalRequestClose, onAttentionReassign } = handlers;
  const pause = canManage && onAttentionPause && isActive;
  const resume = canManage && onAttentionResume && isPaused;
  const confirm = canManage && onGoalConfirm && live;
  const requestClose = canManage && onGoalRequestClose && live;
  const reassignable = canManage && onAttentionReassign && canReassign;
  const submitReassign = () => {
    const identity = reassignIdentity.trim();
    if (!identity || !onAttentionReassign) return;
    onAttentionReassign({ ...bindingInput, identity });
    setReassignOpen(false);
    setReassignIdentity("");
  };
  const pausedUntil = isPaused ? binding.status?.until : undefined;
  return (
    <div className="workgraph-binding" data-testid={`workgraph-panel-binding:${bindingId}`} data-state={state}>
      <div className="workgraph-binding__line">
        <span className="workgraph-binding__summary">
          {target ? <AgentAvatar name={target.name} size={18} /> : <StatusDot tone={statusTone(state)} bound />}
          <span className="workgraph-binding__text">
            {target ? <><WorkGraphPerson person={target} className="workgraph-binding__agent" />{" · "}</> : null}
            <span className="workgraph-binding__mode" title={`Attention mode: ${binding.mode || "pursue"}`}>
              {workGraphModeWord(binding.mode)}
            </span>
            {" · "}
            <span className={`workgraph-binding__state is-${state}`}>{workGraphBindingStateText(binding, now)}</span>
            {subject ? <>{" · on "}{subject}</> : null}
          </span>
          {pausedUntil ? (
            <span className="workgraph-binding__countdown" title={`Resumes ${formatAbsoluteTime(pausedUntil)}`}>
              <Countdown to={pausedUntil} chip />
            </span>
          ) : null}
        </span>
        {pause || resume || confirm || requestClose || reassignable ? (
          <span className="workgraph-binding__actions">
            {pause ? (
              <Button size="sm" title="Pause this goal for its agent" onClick={() => onAttentionPause?.(bindingInput)}>
                Pause
              </Button>
            ) : null}
            {resume ? (
              <Button size="sm" title="Resume this goal for its agent" onClick={() => onAttentionResume?.(bindingInput)}>
                Resume
              </Button>
            ) : null}
            {confirm ? (
              <Button size="sm" title="Confirm the goal is complete" onClick={() => onGoalConfirm?.(goalInput)}>
                Confirm
              </Button>
            ) : null}
            {requestClose ? (
              <Button size="sm" title="Ask to close the goal" onClick={() => onGoalRequestClose?.(goalInput)}>
                Request close
              </Button>
            ) : null}
            {reassignable ? (
              <Button
                size="sm"
                title="Hand the goal to another agent"
                aria-expanded={reassignOpen}
                onClick={() => setReassignOpen((value) => !value)}
              >
                Reassign
              </Button>
            ) : null}
          </span>
        ) : null}
      </div>
      {reassignOpen && reassignable ? (
        <div className="workgraph-binding__reassign">
          <label className="cc-visually-hidden" htmlFor={inputId}>Agent identity to reassign to</label>
          <input
            id={inputId}
            className="cc-input"
            placeholder="Agent identity"
            value={reassignIdentity}
            onChange={(event) => setReassignIdentity(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                submitReassign();
              }
            }}
            data-testid={`workgraph-panel-reassign-input:${bindingId}`}
          />
          <Button
            size="sm"
            variant="primary"
            disabled={!reassignIdentity.trim()}
            data-testid={`workgraph-panel-reassign-submit:${bindingId}`}
            onClick={submitReassign}
          >
            Assign
          </Button>
        </div>
      ) : null}
    </div>
  );
}

// ── Activity ─────────────────────────────────────────────────────────────

/// Activity rows: a sentence about the item by title, and when.
export function WorkGraphActivityList({
  events,
  titleOf,
  limit,
  onSelectItem,
  testId,
}: {
  events: readonly WorkGraphWireEvent[];
  titleOf: (itemId: string) => string | undefined;
  limit?: number;
  onSelectItem?: (itemId: string) => void;
  testId?: string;
}): React.JSX.Element {
  const directory = useAgentDirectory();
  const shown = typeof limit === "number" ? events.slice(0, limit) : events;
  return (
    <RowList className="workgraph-activity" testId={testId}>
      {shown.map((event, index) => {
        const sentence = workGraphEventSentence(event, { titleOf, directory });
        const itemId = workGraphEventItemId(event);
        return (
          <ListRow
            key={`${event.seq ?? index}`}
            dense
            className="workgraph-event"
            title={sentence}
            titleAttr={sentence}
            onActivate={onSelectItem && itemId && titleOf(itemId) ? () => onSelectItem(itemId) : undefined}
            meta={<RelativeTime value={event.at} />}
          />
        );
      })}
    </RowList>
  );
}

// ── The drawer ───────────────────────────────────────────────────────────

function ItemCopyIcon({ name }: { name: string }) {
  return <CopyGlyph state={name === "i-check" ? "copied" : "idle"} />;
}

/// A linked item in the drawer: its square (no data-status: the drawer pins
/// exactly one), its title as a button that selects it, and a short meta.
function ItemLinkRow({ entry, meta, onSelect }: {
  entry: WorkGraphModelItem;
  meta?: string;
  onSelect?: (itemId: string) => void;
}) {
  return (
    <li className="workgraph-drawer__link">
      <StatusSquare status={entry.status} size={12} />
      {onSelect ? (
        <button type="button" className="workgraph-drawer__link-title" onClick={() => onSelect(entry.id)} title={entry.title}>
          {entry.title}
        </button>
      ) : (
        <span className="workgraph-drawer__link-title">{entry.title}</span>
      )}
      {meta ? <span className="workgraph-drawer__link-meta">{meta}</span> : null}
    </li>
  );
}

function linkMeta(entry: WorkGraphModelItem, directory: ReturnType<typeof useAgentDirectory>): string {
  const owner = workGraphItemOwner(entry.item, directory);
  const priority = entry.item.priority === "high" || entry.item.priority === "low" ? humanizeEnum(entry.item.priority) : "";
  return joinMeta(owner?.name ?? (isWorkGraphTerminal(entry.status) ? "" : "Unassigned"), priority, workGraphStatusWord(entry.status));
}

function timing(item: WorkGraphWireItem): Array<{ key: string; label: string; value: React.ReactNode } | null> {
  const at = (value: string | undefined, label: string, key: string) =>
    value ? { key, label, value: <RelativeTime value={value} title={formatAbsoluteTime(value)} /> } : null;
  return [
    at(item.created_at, "Created", "created"),
    at(item.claim?.claimed_at, "Claimed", "claimed"),
    at(item.claim?.lease_expires_at, "Lease ends", "lease"),
    at(item.due_at, "Becomes due", "due"),
    at(item.not_before, "Not before", "not-before"),
    at(item.snoozed_until, "Snoozed until", "snoozed"),
    at(item.terminal_at, "Done", "done"),
    at(item.updated_at, "Updated", "updated"),
  ];
}

function ItemDetails({ itemId, item, bindings }: {
  itemId: string;
  item: WorkGraphWireItem | undefined;
  bindings: readonly WorkGraphWireBinding[];
}) {
  const technical: Array<TechnicalDetail | null> = [
    {
      label: "ID",
      value: (
        <span className="cc-tech-id workgraph-graph__detail-id">
          <code>{itemId}</code>
          <CopyButton text={itemId} label="Copy work item ID" copiedLabel="Work item ID copied" Icon={ItemCopyIcon} />
        </span>
      ),
    },
    item?.labels?.length ? {
      label: "Labels",
      value: <span className="cc-chips">{item.labels.map((label) => <Chip key={label} mono>{label}</Chip>)}</span>,
    } : null,
    typeof item?.revision === "number" ? { label: "Revision", value: item.revision } : null,
    ...bindings.flatMap((binding) => (binding.binding_id ? [{ label: "Attention binding ID", value: binding.binding_id }] : [])),
  ];
  return <TechnicalDetails className="workgraph-graph__metadata" summary="Item details" items={technical} />;
}

interface DrawerProps {
  model: WorkGraphModel;
  itemId: string;
  events: readonly WorkGraphWireEvent[];
  items: readonly WorkGraphWireItem[];
  doneBy: ReadonlyMap<string, string>;
  canManage: boolean;
  handlers: WorkGraphActionHandlers;
  onSelect?: (itemId: string | null) => void;
  returnFocus?: () => void;
}

function ItemActivity({ events, itemId, titleOf }: {
  events: readonly WorkGraphWireEvent[];
  itemId: string;
  titleOf: (itemId: string) => string | undefined;
}) {
  const [all, setAll] = React.useState(false);
  const own = events.filter((event) => workGraphEventItemId(event) === itemId);
  if (own.length === 0) return <p className="workgraph-drawer__empty">No activity in the recent window.</p>;
  return (
    <>
      <WorkGraphActivityList events={own} titleOf={titleOf} limit={all ? undefined : 5} />
      {own.length > 5 ? (
        <Button size="sm" variant="ghost" className="workgraph-drawer__more" onClick={() => setAll((value) => !value)}>
          {all ? "Show fewer" : `Show all ${own.length}`}
        </Button>
      ) : null}
    </>
  );
}

/// The drawer for a selected item (or a goal: its progress, agents and
/// items first). Exactly one heading, one [data-status], one <details> and
/// one <code> (the item id), which the scenarios pin.
function WorkItemDrawer({ model, itemId, events, items, doneBy, canManage, handlers, onSelect, returnFocus }: DrawerProps) {
  const directory = useAgentDirectory();
  const entry = model.items.get(itemId);
  const item = entry?.item;
  const status = entry?.status ?? "open";
  const titleOf = (id: string) => model.items.get(id)?.title;
  const owner = item ? workGraphItemOwner(item, directory) : null;
  const priority = priorityTone(item?.priority);
  const frame = entry?.frameId ? model.frames.find((candidate) => candidate.id === entry.frameId) : undefined;
  const goal = entry && (entry.isGoal || entry.isNestedGoal) ? model.frames.find((candidate) => candidate.id === entry.id) : undefined;
  const subtitle = goal
    ? `Goal · ${workGraphProgressText(goal.progress)}`
    : frame ? frame.label : entry?.isGoal ? "Goal" : undefined;
  const select = onSelect ? (id: string) => onSelect(id) : undefined;
  const actions = item && canManage ? workGraphItemActions(item, itemId, canManage, handlers, { variant: "default", release: true }) : null;
  const blockers = entry?.blockerIds ?? [];
  const dependents = entry?.dependentIds.map((id) => model.items.get(id)).filter((value): value is WorkGraphModelItem => Boolean(value)) ?? [];
  const ownerValue = owner
    ? <WorkGraphPerson person={owner} avatar={18} />
    : status === "completed"
      ? (doneBy.get(itemId) ? `Done by ${doneBy.get(itemId)}` : "Done")
      : isWorkGraphTerminal(status) ? workGraphStatusWord(status) : "Unassigned";
  return (
    <DetailDrawer
      testId="workgraph-graph-detail"
      title={item?.title || (item ? "Untitled work item" : "Work item unavailable in this snapshot")}
      titleClassName="workgraph-graph__detail-title"
      headingLevel={4}
      mark={goal ? <KitGlyph name="flag" /> : <StatusSquare status={status} size={16} />}
      status={item ? (
        <span className="workgraph-graph__detail-status" data-status={status}>
          <StatusPill tone={statusTone(status)}>{workGraphStatusWord(status)}</StatusPill>
        </span>
      ) : undefined}
      subtitle={subtitle}
      onClose={() => onSelect?.(null)}
      returnFocus={returnFocus}
    >
      <div className="workgraph-drawer__facts">
        {priority ? <Chip tone={priority} variant="outline">{humanizeEnum(item?.priority)}</Chip> : null}
        {entry?.ready && !isWorkGraphTerminal(status) ? <Chip tone="ok" mark>Ready</Chip> : null}
        {entry?.critical ? <Chip tone="live" variant="quiet">On the critical path</Chip> : null}
        {item?.created_at ? (
          <span className="workgraph-drawer__created">created <RelativeTime value={item.created_at} /></span>
        ) : null}
      </div>
      {item?.description ? <p className="workgraph-graph__detail-description">{item.description}</p> : null}
      {goal ? (
        <DrawerSection label="Progress" testId="workgraph-drawer:progress">
          <div className="workgraph-drawer__progress">
            <ProgressRing
              done={goal.progress.completed}
              inProgress={goal.progress.inProgress}
              blocked={goal.progress.blocked}
              total={goal.progress.total}
              size={24}
            />
            <span>
              {joinMeta(
                `${goal.progress.completed} done`,
                goal.progress.inProgress ? `${goal.progress.inProgress} in progress` : "",
                goal.progress.blocked ? `${goal.progress.blocked} blocked` : "",
                goal.progress.failed ? `${goal.progress.failed} failed` : "",
                goal.progress.open ? `${goal.progress.open} open` : "",
                goal.progress.cancelled ? `${goal.progress.cancelled} cancelled` : "",
              )}
            </span>
          </div>
          <p className="workgraph-drawer__policy">Completion: {workGraphCompletionPolicyText(item?.completion_policy, directory)}</p>
        </DrawerSection>
      ) : null}
      {entry && entry.bindings.length > 0 ? (
        <DrawerSection label={goal ? "Agents on this goal" : "Agents on this item"} count={entry.bindings.length}>
          <div className="workgraph-drawer__bindings">
            {entry.bindings.map((binding, index) => (
              <WorkGraphAttentionRow
                key={binding.binding_id || `binding-${index}`}
                binding={binding}
                goalRevision={workGraphGoalRevisionOf(binding, items)}
                canManage={canManage}
                handlers={handlers}
              />
            ))}
          </div>
        </DrawerSection>
      ) : null}
      {goal && goal.allItemIds.length > 0 ? (
        <DrawerSection label="Items" count={goal.allItemIds.length}>
          <ul className="workgraph-drawer__links">
            {goal.allItemIds.map((id) => {
              const child = model.items.get(id);
              return child ? <ItemLinkRow key={id} entry={child} meta={linkMeta(child, directory)} onSelect={select} /> : null;
            })}
          </ul>
        </DrawerSection>
      ) : null}
      {blockers.length > 0 ? (
        <DrawerSection label="Blocked by" count={blockers.length} testId="workgraph-drawer:blocked-by">
          <ul className="workgraph-drawer__links">
            {blockers.map((id) => {
              const blocker = model.items.get(id);
              return blocker
                ? <ItemLinkRow key={id} entry={blocker} meta={linkMeta(blocker, directory)} onSelect={select} />
                : <li key={id} className="workgraph-drawer__link is-missing">An item outside this view</li>;
            })}
          </ul>
        </DrawerSection>
      ) : null}
      {dependents.length > 0 ? (
        <DrawerSection label="Unblocks" count={dependents.length} testId="workgraph-drawer:unblocks">
          <ul className="workgraph-drawer__links">
            {dependents.map((dependent) => (
              <ItemLinkRow key={dependent.id} entry={dependent} meta={linkMeta(dependent, directory)} onSelect={select} />
            ))}
          </ul>
        </DrawerSection>
      ) : null}
      <DrawerSection label="Owner and timing">
        <KeyValueList
          className="workgraph-graph__detail-facts"
          items={[
            { key: "owner", label: "Owner", value: ownerValue },
            ...timing(item ?? {}),
            entry?.extraParentIds.length
              ? { key: "also-under", label: "Also under", value: entry.extraParentIds.map((id) => titleOf(id) ?? "another item").join(", ") }
              : null,
          ]}
        />
      </DrawerSection>
      <DrawerSection label="Evidence" count={item?.evidence_refs?.length || undefined}>
        {item?.evidence_refs?.length ? (
          <ul className="workgraph-drawer__evidence">
            {item.evidence_refs.map((evidence, index) => (
              <li key={`${evidence.kind ?? ""}:${evidence.id ?? index}`}>
                <span className="workgraph-drawer__evidence-label">{evidence.label || humanizeEnum(evidence.kind) || "Evidence"}</span>
                {evidence.summary ? <span className="workgraph-drawer__evidence-summary">{evidence.summary}</span> : null}
              </li>
            ))}
          </ul>
        ) : <p className="workgraph-drawer__empty">None yet</p>}
      </DrawerSection>
      <DrawerSection label="Activity">
        <ItemActivity events={events} itemId={itemId} titleOf={titleOf} />
      </DrawerSection>
      {/* The actions sit after the sections and before the identifiers
          (spec B2), so they stay in the body rather than the drawer's
          trailing action row. */}
      {actions ? <div className="cc-drawer__actions workgraph-drawer__actions">{actions}</div> : null}
      <ItemDetails itemId={itemId} item={item} bindings={entry?.bindings ?? []} />
    </DetailDrawer>
  );
}

// ── The graph view ───────────────────────────────────────────────────────

export interface WorkGraphGraphViewProps {
  items: WorkGraphWireItem[];
  edges: WorkGraphWireEdge[];
  attention: WorkGraphWireBinding[];
  events?: WorkGraphWireEvent[];
  readyItemIds?: string[] | null;
  /// The panel's model of the same snapshot; built here when absent.
  model?: WorkGraphModel;
  selectedId?: string;
  onSelect?: (itemId: string | null) => void;
  showDone?: boolean;
  showCancelled?: boolean;
  highlightCriticalPath?: boolean;
  density?: "comfortable" | "compact";
  canManage?: boolean;
  handlers?: WorkGraphActionHandlers;
  /// Drawn under the canvas, inside the drawer layout's main column (the
  /// panel's activity strip).
  footer?: React.ReactNode;
  /// Shown in the drawer column while no item is selected (the activity).
  aside?: { title: string; content: React.ReactNode; onClose: () => void; testId?: string } | null;
  /// "<scope>": the view's per-panel caches (minimap toggle) key on it.
  cacheKey?: string;
  /// Bring this item into view once it is drawn (a link from another panel
  /// opened it). A new object asks again, even for the same item.
  revealRequest?: { id: string } | null;
  /// Link kinds the person hid from the header menu ("blocks", "related",
  /// "parent").
  hiddenRelations?: readonly VizRelation[];
}

const NO_EVENTS: WorkGraphWireEvent[] = [];
const NO_HANDLERS: WorkGraphActionHandlers = {};
const NO_RELATIONS: readonly VizRelation[] = [];

/// The graph's zoom controls, in a bar of their own under the canvas: no
/// control sits on the picture. The level follows the view as it moves
/// (one update a frame, outside the view's own render).
function WorkGraphZoomBar({
  graphRef,
  listen,
  notDrawn,
}: {
  graphRef: React.RefObject<GraphViewHandle | null>;
  listen: (onScale: (scale: number) => void) => () => void;
  notDrawn: number;
}): React.JSX.Element {
  const [scale, setScale] = React.useState(() => graphRef.current?.viewport().scale ?? 1);
  React.useEffect(() => listen(setScale), [listen]);
  const percent = `${Math.round(scale * 100)}%`;
  return (
    <div className="workgraph-graph__bar">
      <div className="workgraph-graph__zoom" role="toolbar" aria-label="Zoom">
        <IconButton
          label="Zoom out"
          title="Zoom out (-)"
          glyph="zoom-out"
          size="sm"
          data-testid="workgraph-graph-zoom:out"
          onClick={() => graphRef.current?.zoomBy(1 / ZOOM.buttonStep)}
        />
        <button
          type="button"
          className="workgraph-graph__zoom-level"
          aria-label={`Zoom ${percent}, reset to 100%`}
          title="Reset to 100%"
          data-testid="workgraph-graph-zoom:reset"
          onClick={() => graphRef.current?.zoomBy(1 / scale)}
        >
          {percent}
        </button>
        <IconButton
          label="Zoom in"
          title="Zoom in (+)"
          glyph="zoom-in"
          size="sm"
          data-testid="workgraph-graph-zoom:in"
          onClick={() => graphRef.current?.zoomBy(ZOOM.buttonStep)}
        />
        <IconButton
          label="Fit to view"
          title="Fit (0)"
          glyph="fit"
          size="sm"
          data-testid="workgraph-graph-fit"
          onClick={() => graphRef.current?.fit()}
        />
      </div>
      {notDrawn > 0 ? (
        <span className="workgraph-graph__overflow" data-testid="workgraph-graph-overflow">
          {`+${notDrawn} more ${notDrawn === 1 ? "item" : "items"} not drawn`}
        </span>
      ) : null}
    </div>
  );
}

/// Goal ring around an attention-bound node's mark: the item's status
/// square, or the goal header's flag.
function renderGoalRing(node: VizNode, box: VizNodeBox): React.ReactNode {
  const data = node.data as WorkGraphNodeData | undefined;
  if (!data?.bound) return null;
  if (node.kind === "goal") {
    return <circle className="workgraph-graph__node-goal-ring" cx={-box.w / 2 + 14} cy={0} r={10} />;
  }
  // Around the card's status square, wherever the kit's card puts it.
  const { square } = itemCardGeometry(box.w, box.h, Boolean(node.sublabel));
  return <circle className="workgraph-graph__node-goal-ring" cx={square.x} cy={square.y} r={10} />;
}

export function WorkGraphGraphView({
  items,
  edges,
  attention,
  events = NO_EVENTS,
  readyItemIds,
  model: givenModel,
  selectedId,
  onSelect,
  showDone = false,
  showCancelled = false,
  highlightCriticalPath = false,
  density = "comfortable",
  canManage = false,
  handlers = NO_HANDLERS,
  footer,
  aside,
  cacheKey,
  revealRequest = null,
  hiddenRelations = NO_RELATIONS,
}: WorkGraphGraphViewProps): React.JSX.Element {
  const directory = useAgentDirectory();
  const now = useNow();
  const builtModel = React.useMemo(
    () => (givenModel ? null : buildWorkGraphModel({ items, edges, attention, readyItemIds })),
    [givenModel, items, edges, attention, readyItemIds],
  );
  const model = givenModel ?? builtModel!;
  const doneBy = React.useMemo(() => workGraphDoneBy(events, directory), [events, directory]);
  // Ages and "Done by" move slowly; the graph rebuilds at most once a minute
  // for the clock, so a refresh that changes nothing redraws nothing.
  const minute = Math.floor(now / 60_000);
  const graph = React.useMemo(
    () => buildWorkGraphGraph(model, {
      selectedId: selectedId ?? null,
      showDone,
      showCancelled,
      highlightCriticalPath,
      directory,
      now: minute * 60_000,
      doneBy,
    }),
    [model, selectedId, showDone, showCancelled, highlightCriticalPath, directory, minute, doneBy],
  );
  const graphRef = React.useRef<GraphViewHandle | null>(null);
  const canvasRef = React.useRef<HTMLDivElement | null>(null);
  // The zoom bar's level: the view reports every move from inside its own
  // state update, so the bar hears of it on the next frame instead.
  const scaleListener = React.useRef<((scale: number) => void) | null>(null);
  const scaleFrame = React.useRef(0);
  const latestScale = React.useRef<number | null>(null);
  const listenToScale = React.useCallback((onScale: (scale: number) => void) => {
    scaleListener.current = onScale;
    if (latestScale.current !== null) onScale(latestScale.current);
    return () => {
      if (scaleListener.current === onScale) scaleListener.current = null;
    };
  }, []);
  const onViewportChange = React.useCallback((viewport: VizViewport) => {
    latestScale.current = viewport.scale;
    if (scaleFrame.current) return;
    const deliver = () => {
      scaleFrame.current = 0;
      if (latestScale.current !== null) scaleListener.current?.(latestScale.current);
    };
    if (typeof requestAnimationFrame === "function") scaleFrame.current = requestAnimationFrame(deliver);
    else {
      scaleFrame.current = 1;
      queueMicrotask(deliver);
    }
  }, []);
  React.useEffect(() => () => {
    if (scaleFrame.current && typeof cancelAnimationFrame === "function") cancelAnimationFrame(scaleFrame.current);
  }, []);
  const selection = React.useMemo(() => (selectedId ? [selectedId] : []), [selectedId]);

  // A link from elsewhere asked for an item: once its card is drawn, pan it
  // into view, and again over the next frames while the drawer docks and
  // narrows the canvas.
  const pendingReveal = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (revealRequest) pendingReveal.current = revealRequest.id;
  }, [revealRequest]);
  React.useEffect(() => {
    const id = pendingReveal.current;
    if (!id || !graph.nodes.some((node) => node.id === id)) return undefined;
    const drawn = () => Array.from(canvasRef.current?.querySelectorAll('[data-testid="workgraph-graph-node"]') ?? [])
      .some((element) => element.getAttribute("data-item-id") === id);
    if (typeof requestAnimationFrame !== "function") {
      if (drawn()) {
        graphRef.current?.reveal(id);
        pendingReveal.current = null;
      }
      return undefined;
    }
    let waited = 0;
    let settled = 0;
    let handle = 0;
    const step = () => {
      if (drawn()) {
        graphRef.current?.reveal(id);
        if (settled++ < 3) handle = requestAnimationFrame(step);
        else pendingReveal.current = null;
      } else if (waited++ < 60) {
        handle = requestAnimationFrame(step);
      }
    };
    step();
    return () => cancelAnimationFrame(handle);
  }, [graph.nodes, revealRequest]);
  const drawnItems = graph.nodes.filter((node) => node.kind === "item").length;
  const goals = graph.nodes.length - drawnItems;
  const ariaLabel = joinMeta(
    plural(drawnItems, "item"),
    goals ? plural(goals, "goal") : "",
    plural(graph.edges.length, "link"),
  ).replace(/ · /g, ", ");

  if (model.items.size === 0) {
    return <EmptyState inline title="No work items yet" />;
  }

  const drawer = selectedId ? (
    <div className="workgraph-drawer" data-testid="workgraph-drawer">
      <WorkItemDrawer
        model={model}
        itemId={selectedId}
        events={events}
        items={items}
        doneBy={doneBy}
        canManage={canManage}
        handlers={handlers}
        onSelect={onSelect}
        returnFocus={() => graphRef.current?.focusNode(selectedId)}
      />
    </div>
  ) : aside ? (
    <div className="workgraph-drawer" data-testid="workgraph-drawer">
      <DetailDrawer
        testId={aside.testId}
        title={aside.title}
        headingLevel={4}
        mark={<KitGlyph name="clock" />}
        onClose={aside.onClose}
      >
        {aside.content}
      </DetailDrawer>
    </div>
  ) : null;

  const main = (
    <div className="workgraph-graph__main">
      <div className="workgraph-graph__canvas" ref={canvasRef}>
        <GraphView
          ref={graphRef}
          nodes={graph.nodes}
          edges={graph.edges}
          frames={graph.frames}
          layout="layered"
          density={density}
          selection={selection}
          onSelect={(ids) => onSelect?.(ids.length > 0 ? ids[ids.length - 1] : null)}
          onOpen={(id) => {
            onSelect?.(id);
            // Double-click (or Enter on) a goal's header zooms to that goal.
            if (model.frames.some((frame) => frame.id === id)) graphRef.current?.fitTo([id]);
          }}
          hoverHighlight="chain"
          accentPath={graph.accentPath}
          edgeLabels={graph.edgeLabels}
          // Nothing floats on the picture: the edges carry their words (the
          // legend's link toggles are in the header menu), the zoom controls
          // sit in the bar below, and the minimap stays off.
          legend="off"
          controls={false}
          minimap="off"
          hiddenRelations={hiddenRelations}
          onViewportChange={onViewportChange}
          renderer="svg"
          labels="all"
          ariaLabel={ariaLabel}
          testIdPrefix="workgraph-graph"
          testIds={{
            root: "workgraph-graph-frame",
            svg: "workgraph-graph",
            viewport: "workgraph-graph-viewport",
            node: "workgraph-graph-node",
            edge: "workgraph-graph-edge",
            edgeLabel: "workgraph-graph-edge-label",
          }}
          classNames={{
            node: "workgraph-graph__node",
            itemTitle: "workgraph-graph__node-title",
            itemMeta: "workgraph-graph__node-meta",
            edge: "workgraph-graph__edge",
            edgeLabel: "workgraph-graph__edge-label",
            frame: "workgraph-graph__frame",
          }}
          renderNodeDecor={renderGoalRing}
          cacheKey={cacheKey ? `workgraph:${cacheKey}` : "workgraph:default"}
          className="workgraph-graph__view"
        />
      </div>
      <WorkGraphZoomBar graphRef={graphRef} listen={listenToScale} notDrawn={graph.notDrawn} />
      {footer}
    </div>
  );

  return (
    <div className="workgraph-graph" data-lod={graph.lod ? "true" : undefined}>
      <DrawerLayout
        main={main}
        drawer={drawer}
        open={Boolean(drawer)}
        onClose={() => {
          if (selectedId) onSelect?.(null);
          else aside?.onClose();
        }}
        persistKey="workgraph"
        backLabel="Back to the graph"
        testId="workgraph-graph-layout"
      />
    </div>
  );
}
