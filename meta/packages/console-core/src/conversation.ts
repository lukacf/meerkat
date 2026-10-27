import {
  conversationRichBlockHasCopyAction,
  conversationRichBlocksToText,
  type ConversationRichBlock,
} from "./rich-content";
import type { ConsoleContextMessage } from "./context-record";
import { conversationPresentationRows } from "./assistant-presentation";

export type ConversationRole = "assistant" | "user" | "system" | "other";
export type ConversationPresentation = "assistant" | "user" | "participant" | "system";

export interface ConversationTone {
  variables?: Record<string, string> | null;
}

export interface ConversationIdentity {
  id: string;
  label: string;
  role: ConversationRole;
  kind?: string | null;
  meta?: string | null;
  presentation?: ConversationPresentation;
  showLabel?: boolean;
  avatarLabel?: string | null;
  tone?: ConversationTone | null;
}

export interface ConversationConnectionPeer {
  /** Stable endpoint identity; never inferred from the display label. */
  id: string;
  label: string;
  caption?: string | null;
  scopeId?: string | null;
  scopeLabel?: string | null;
  crossScope?: boolean;
}

export interface ConversationConnectionEvent {
  action: "connected" | "disconnected" | "reconnected";
  peers: ConversationConnectionPeer[];
  status?: "succeeded" | "partial" | "degraded" | "conflict";
  operationId?: string | null;
  message?: string | null;
}

export interface ConversationEmptySuggestion {
  id: string;
  label: string;
  value: string;
  iconName?: string | null;
}

export interface ConversationEmptyStateSpec {
  title: string;
  subtitle: string;
  projectLabel?: string | null;
  iconName?: string | null;
  suggestions?: ConversationEmptySuggestion[];
}

interface ConversationTimelineEntryBase {
  id: string;
  /** Stable presentation identity from the typed input or assistant occurrence. The
   * source frame ID remains in `id` for quotes and provenance. */
  renderKey?: string;
  identity: ConversationIdentity;
  createdAt?: string;
  copyText?: string;
  /** Source frame's interaction id. UUID-form ids are authoritative for
   * live/history twin identity (mobkit 0.7.30, meerkat ask 15 addendum);
   * legacy `console-interaction-*` strings are not comparable across
   * live/history sources. */
  interactionId?: string;
  /** Host-owned stable identity for reconciling a provisional live entry with
   * its durable twin when the transport run/interaction id arrives late. */
  reconciliationKey?: string | null;
  /** Host-owned stable identity for the surrounding conversation group.
   * Activity artifacts and the response can share this anchor without sharing
   * an entry key or being mistaken for live/durable twins. */
  groupReconciliationKey?: string | null;
}

/** Canonical realtime provenance validated against the containing history frame. */
export interface ConversationRealtimeOrigin {
  sessionId: string;
  channelId: string;
  canonicalRowSequence: number;
  /** Empty on legacy rows; text and timestamps never substitute for these IDs. */
  providerItemIds: string[];
}

export interface ConversationMessageEntry extends ConversationTimelineEntryBase {
  kind: "message";
  variant: "plain" | "rich" | "meta";
  /** Runtime-owned MCP invocation locator; UI data is resolved through the authenticated host. */
  mcpApp?: { sessionId: string; toolCallId: string };
  text?: string;
  blocks?: ConversationRichBlock[];
  /** Stable display rows for live segments and a bundled canonical message. */
  presentationRows?: { renderKey: string; blocks: ConversationRichBlock[] }[];
  /** Parent display identity, independent of which content segment arrives first. */
  assistantOccurrenceKey?: string;
  /** Display-only projection of a validated local quote envelope. */
  contextMessage?: ConsoleContextMessage;
  /** Live provider observations may join only this exact canonical witness. */
  realtimeOrigin?: ConversationRealtimeOrigin;
  richStyle?: "default" | "streaming";
  /**
   * Typed host-task metadata. System tasks stay distinct from operator turns
   * all the way through the shared renderer instead of masquerading as user
   * messages.
   */
  taskKind?: string;
  taskLabel?: string;
  taskId?: string;
  taskStatus?: string;
  runId?: string | null;
  /** Display interval from matching runtime start and successful terminal
   * evidence for this exact run. Absent when that timing is unknown; message
   * creation timestamps must not be used as a substitute. */
  runDurationMs?: number;
  connectionEvent?: ConversationConnectionEvent | null;
  /**
   * Typed provenance of a user-lane entry, copied from the wire. Renderers
   * label the entry from these fields only, never from its text.
   */
  origin?: ConversationEntryOrigin | null;
  /**
   * A runtime event the console has no dedicated card for. Carried typed so
   * the transcript can say what happened in words and keep the raw payload
   * behind a disclosure, instead of printing the payload inline.
   */
  runtimeEvent?: ConversationRuntimeEvent | null;
  /** Display-only projection of an explicit runtime refusal or audit diagnostic. */
  operationFeedback?: {
    kind: "operation-refused" | "audit-unavailable" | "confinement-refused" | "hook-denied" | "hook-launch-failed" | "review-unsatisfied" | "review-unavailable";
    title: string;
    detail: string;
    operationId?: string;
    /** Opaque native approval reference for display/correlation only, never permission or proof of execution. */
    approvalId?: string;
    toolCallId?: string;
    hookId?: string;
    confinementRefusal?: "invalid_requirement" | "invalid_launch" | "unsupported_requirement" | "backend_unavailable" | "preparation_failed";
    hookReasonCode?: "policy_violation" | "safety_violation" | "schema_violation" | "timeout" | "runtime_error";
    reviewUnsatisfied?: "denied" | "escalated" | "context_changed" | "human_consent_required" | "already_consumed";
    reviewUnavailable?: "deadline_expired" | "reviewer_failed" | "reviewer_missing" | "unsupported_entry" | "owner_unavailable" | "dispatch_ended";
  };
}

/** Typed provenance of a user-lane message. */
export interface ConversationEntryOrigin {
  /**
   * Caller identifier from a console send (`user_input.origin`). The
   * console's own composer sends `console:<panel-id>`; other callers choose
   * their own identifier.
   */
  sendOrigin?: string | null;
  /**
   * Typed caller kind from a console send (`user_input.origin_kind`, Meta
   * `ConsoleTurnOrigin`): `operator`, `operator_probe`, `connector`,
   * `scheduler`, `policy`, `flow`, `system`.
   */
  originKind?: string | null;
  /** meerkat `render_metadata.class` persisted on the transcript message. */
  renderClass?: string | null;
}

/** A peer reference as carried on a runtime event. */
export interface ConversationPeerRef {
  /** Stable peer id (UUID on the wire). */
  id: string | null;
  /** meerkat comms name, `mob_id/role/member`. */
  displayName: string | null;
}

/** A generic runtime event, typed from its frame. */
export interface ConversationRuntimeEvent {
  /** Frame event name, e.g. `peer_content_ingested`. */
  eventType: string;
  /** Payload `kind` when present, e.g. `message`, `request`, `response`. */
  kind: string | null;
  peer: ConversationPeerRef | null;
  /** meerkat `SenderContentTaint`: `clean` or `tainted`. */
  senderTaint: string | null;
  /** The raw frame payload, shown only behind a disclosure. */
  payload: unknown;
}

export interface ConversationSummaryFile {
  name: string;
  plus: number;
  minus: number;
}

export interface ConversationSummaryEntry extends ConversationTimelineEntryBase {
  kind: "summary";
  title: string;
  plus: number;
  minus: number;
  files: ConversationSummaryFile[];
  actionLabel?: string | null;
}

export type FlowRunStatus = "idle" | "queued" | "running" | "cancelling" | "completed" | "failed" | "stopped";

export interface ConversationFlowRunMemberRow {
  memberKey: string;
  label: string;
  caption: string;
  status: FlowRunStatus;
  tone?: ConversationTone | null;
  // A prebuilt view of this member's own streamed transcript (tool cards,
  // thinking, code) rendered when the row is expanded. Absent until the live
  // execution snapshot has hydrated the member's sub-transcript.
  subView?: ConversationViewState | null;
}

// A flow run (a helper-mob crew spawned on demand) rendered inline in the
// conversation as one card, replacing the deleted watch rail. Rows are the crew
// members with honest live status — there is no fabricated step ordering because
// flow definitions carry no ordered-step model.
export interface ConversationFlowRunEntry extends ConversationTimelineEntryBase {
  kind: "flow_run";
  helperId: string;
  flowName: string;
  objective?: string | null;
  status: FlowRunStatus;
  outcome?: string | null;
  rows: ConversationFlowRunMemberRow[];
  // True when the crew exists only as persisted records (no live execution
  // snapshot) and can be brought back — the card offers a Resume action.
  restorable?: boolean;
}

// Aggregate posture of one goal/root work-item card. "mixed" is a fully
// terminal graph that ended in a blend of completed/cancelled outcomes.
export type WorkGraphCardStatus = "active" | "blocked" | "completed" | "failed" | "mixed";

export interface ConversationWorkGraphItemRow {
  itemId: string;
  title: string;
  // Upstream WorkStatus wire value: open|in_progress|blocked|completed|cancelled|failed.
  status: string;
  priority?: string | null;
  ownerLabel?: string | null;
  // CAS token — operator actions must send the latest observed revision.
  // Absent means no frame ever carried one: actions must resolve it from the
  // service before mutating (never guess 0).
  revision?: number;
  depth: number;
  parentId?: string | null;
  // Upstream allows multiple parents per child; the row is placed under its
  // first observed parent (parentId) and any further parents are listed here
  // (parent titles, or ids when the parent was never observed) as an
  // "also under …" note in the row detail.
  alsoUnder?: string[];
  blocked?: boolean;
  dueAt?: string | null;
  lastEventAt?: string | null;
  description?: string | null;
  labels?: string[];
  evidence?: string[];
  createdAt?: string | null;
  updatedAt?: string | null;
}

export interface ConversationWorkGraphAttentionRow {
  bindingId: string;
  // pursue|coordinate|review|falsify|judge|observe
  mode: string;
  // Human form of the binding status: "active", "paused until …", …
  statusLabel: string;
  targetLabel?: string | null;
  // Binding machine revision (CAS token for attention mutations).
  revision?: number;
  itemId?: string | null;
}

// One evolving in-conversation card per goal/root work item, aggregated from
// the turn's workgraph tool-call frames. Rebuilt from scratch on every adapter
// pass (same posture as tool blocks) with a stable `workgraph:{rootId}` id so
// live updates land in place.
export interface ConversationWorkGraphEntry extends ConversationTimelineEntryBase {
  kind: "workgraph";
  rootId: string;
  // Stable UI-state anchor. The entry `id` migrates when a loose item grows
  // a hierarchy (catch-all `workgraph:interaction:{id}` → rooted
  // `workgraph:{rootId}`), remounting the card; this key stays pinned to the
  // first contributing interaction PLUS the first item id folded into this
  // specific card, so collapse/expansion state survives the rekey moment
  // without bleeding between cards born in the same interaction. Cards that
  // never folded an item anchor on an "unrooted" placeholder segment.
  uiStateKey?: string;
  title: string;
  objective?: string | null;
  status: WorkGraphCardStatus;
  progress: { completed: number; total: number };
  items: ConversationWorkGraphItemRow[];
  // Items hidden by the per-card render cap (the most recently active rows
  // stay in `items`). Hidden items still count toward `progress` and the
  // card status; the card renders one "+N more items" overflow row for them.
  itemOverflowCount?: number;
  attention: ConversationWorkGraphAttentionRow[];
  recentEvents?: string[];
  // True when the most recent workgraph tool call folded into this card
  // failed — the card shows a subtle failure indicator (failures never
  // resurrect generic tool rows).
  lastActionFailed?: boolean;
  lastUpdatedAt?: string;
}

/// Terminal classification of one council, derived from the sealed result's
/// `exit_reason` tag. This is deliberately COARSER than the wire enum: the
/// card shows an operator whether the council produced a usable answer, and
/// the exact typed reason is rendered as detail text underneath.
///
/// `bounded` is separated from `failed` on purpose. A council that stopped on
/// `max_exchanges_reached` or `deadline_exceeded` ran correctly and produced
/// real exchanges - it hit a budget the caller set. Rendering that the same
/// red as a seating failure would teach operators to ignore the colour.
export type CouncilCardStatus = "completed" | "bounded" | "failed" | "pending";

/// One participant slot, from the result's non-secret provenance.
///
/// There is deliberately NO action affordance on this row. Council
/// participants are forked contexts that are destroyed before the tool
/// returns, so by the time this card renders there is nothing addressable
/// left - a button here would offer to act on something that no longer
/// exists.
export interface ConversationCouncilParticipantRow {
  order: number;
  role: string;
  sourceMobId: string;
  sourceIdentity: string;
  targetIdentity: string;
  /// False means the slot was acquired but never seated - the usual cause of
  /// a `participant_seating_failed` exit.
  seated: boolean;
}

/// One bounded exchange receipt, in council order.
export interface ConversationCouncilExchangeRow {
  round: number;
  sequence: number;
  participantOrder: number;
  targetIdentity: string;
  /// Wire `outcome.status`: pending | completed | failed. `pending` is not a
  /// transient render state - a receipt left pending is what a coordinator
  /// crash looks like, so the card must show it rather than hide it.
  status: "pending" | "completed" | "failed";
  /// Committed bounded text for a completed exchange, or the typed detail for
  /// a failed one.
  text?: string;
  /// The receiver bound truncated this exchange's text.
  truncated?: boolean;
}

/// A participant-reported artifact location.
///
/// Rendered as an UNVERIFIED CLAIM, never as an artifact. meerkat's own type
/// says it plainly: the council performs no store lookup, no fetch and no
/// existence check, so presenting it as a resolved handle would assert
/// something nobody verified. The card shows the uri as inert text with an
/// explicit "claimed" affordance and never links it.
export interface ConversationCouncilArtifactClaimRow {
  uri: string;
  mediaType?: string | null;
  digest?: string | null;
  byteLen?: number | null;
}

/// One completed `council` tool call rendered as an in-conversation card.
///
/// Unlike the workgraph card this is NOT an evolving aggregate: a council is
/// a single synchronous tool call that seats participants, runs bounded
/// exchanges, merges and tears down before returning. The card therefore
/// renders one call's request and sealed result, anchored at that frame.
export interface ConversationCouncilEntry extends ConversationTimelineEntryBase {
  kind: "council";
  /// Validated council identity (`serde(transparent)` string on the wire).
  councilId: string;
  /// The decision or question the council examined, from the call arguments.
  topic: string;
  status: CouncilCardStatus;
  /// Wire `exit_reason.reason`, verbatim, for operators who need the exact
  /// typed variant rather than the coarse status.
  exitReason: string;
  /// Human-readable detail carried by the failing `exit_reason` variants.
  exitDetail?: string | null;
  roundsCompleted: number;
  participants: ConversationCouncilParticipantRow[];
  exchanges: ConversationCouncilExchangeRow[];
  /// Exchanges hidden by the render cap. Hidden rows still count in the
  /// header totals.
  exchangeOverflowCount?: number;
  /// Merge-back kind (`no_merge` | `bounded_text_summary` | ...) and its
  /// bounded text, when the policy produced one.
  mergeKind?: string;
  mergeText?: string | null;
  mergeFinalizer?: string | null;
  mergeTruncated?: boolean;
  /// Participant-reported artifact locations. See the row type: claims, not
  /// artifacts.
  artifactClaims?: ConversationCouncilArtifactClaimRow[];
  truncatedExchangeCount?: number;
  /// `durable` | `process_bound`. A process-bound council does not survive a
  /// gateway restart, which the card states rather than implying permanence.
  durability?: string | null;
  /// True when the call was answered from an existing sealed result rather
  /// than by running a new council (idempotency replay).
  replayed?: boolean;
  /// Unpaid cleanup obligations. A council can seal a perfectly good result
  /// and still fail to tear its temporary mob down, so this is reported
  /// separately from `status` and never folded into it.
  cleanupDebts?: { subject: string; detail: string }[];
  /// The bounded cleanup budget expired with work outstanding.
  cleanupBudgetExhausted?: boolean;
  concludedAt?: string | null;
}

export type ConversationTimelineEntry =
  | ConversationMessageEntry
  | ConversationSummaryEntry
  | ConversationFlowRunEntry
  | ConversationWorkGraphEntry
  | ConversationCouncilEntry;

export interface ConversationTimelineGroup {
  id: string;
  identity: ConversationIdentity;
  entries: ConversationTimelineEntry[];
  copyText?: string;
}

export interface ConversationTurnDiffLine {
  type: "context" | "add" | "remove";
  text: string;
  oldLine: number | null;
  newLine: number | null;
}

export interface ConversationTurnDiffHunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: ConversationTurnDiffLine[];
}

export interface ConversationTurnDiffFile {
  path: string;
  plus: number;
  minus: number;
  hunks: ConversationTurnDiffHunk[];
}

export interface ConversationTurnDiff {
  fileCount: number;
  plus: number;
  minus: number;
  capturedAt?: string;
  files: ConversationTurnDiffFile[];
}

export interface ConversationViewState {
  conversationId: string;
  title?: string | null;
  entries: ConversationTimelineEntry[];
  groups: ConversationTimelineGroup[];
  turnDiff: ConversationTurnDiff | null;
  emptyState: ConversationEmptyStateSpec | null;
}

export function conversationIdentityPresentation(
  identity: ConversationIdentity | null | undefined,
): ConversationPresentation {
  if (identity?.presentation) {
    return identity.presentation;
  }
  if (identity?.role === "user") {
    return "user";
  }
  if (identity?.role === "system") {
    return "system";
  }
  if (identity?.role === "other") {
    return "participant";
  }
  return "assistant";
}

export function conversationIdentityShowsLabel(
  identity: ConversationIdentity | null | undefined,
): boolean {
  if (!identity?.label) {
    return false;
  }
  if (typeof identity.showLabel === "boolean") {
    return identity.showLabel;
  }
  const presentation = conversationIdentityPresentation(identity);
  return presentation === "participant" || presentation === "system";
}

export function conversationIdentityGroupKey(
  identity: ConversationIdentity | null | undefined,
): string {
  if (!identity) {
    return "unknown:assistant:hidden";
  }

  return [
    identity.id || "unknown",
    conversationIdentityPresentation(identity),
    conversationIdentityShowsLabel(identity) ? "label" : "hidden",
  ].join(":");
}

export function conversationEntryText(entry: ConversationTimelineEntry): string {
  if (entry.kind === "summary") {
    const fileLines = entry.files
      .map((file) => `${file.name} +${file.plus} -${file.minus}`)
      .join("\n");
    return [entry.title, fileLines].filter(Boolean).join("\n");
  }

  if (entry.kind === "flow_run") {
    const rowLines = entry.rows.map((row) => `${row.label}: ${row.caption}`);
    return [entry.flowName, entry.objective || "", ...rowLines, entry.outcome || ""]
      .filter(Boolean)
      .join("\n");
  }

  if (entry.kind === "council") {
    // Without this branch a council entry falls through to
    // `copyText || text || blocks`, and it has none of the three - so copy and
    // transcript surfaces silently yield an empty string. The card renders
    // fine, which is what makes the absence easy to miss.
    const participantLines = entry.participants.map((row) => (
      `${row.role}: ${row.targetIdentity}${row.seated ? "" : " (never seated)"}`
    ));
    const exchangeLines = entry.exchanges.map((row) => (
      `r${row.round + 1} ${row.targetIdentity} — ${row.status}${row.text ? `: ${row.text}` : ""}`
    ));
    return [
      `${entry.topic} (${entry.exitReason}, ${entry.roundsCompleted} rounds)`,
      entry.exitDetail || "",
      entry.mergeText || "",
      ...participantLines,
      ...exchangeLines,
      ...(entry.exchangeOverflowCount
        ? [`+${entry.exchangeOverflowCount} more exchanges`]
        : []),
      // Claims stay marked as claims in copied text too: pasting a bare uri
      // into a ticket is exactly how an unverified claim becomes a fact.
      ...(entry.artifactClaims || []).map((row) => `claimed artifact: ${row.uri}`),
    ].filter(Boolean).join("\n");
  }

  if (entry.kind === "workgraph") {
    const itemLines = entry.items.map((item) => (
      `${"  ".repeat(item.depth)}${item.title} — ${item.status.replace(/_/g, " ")}`
    ));
    const attentionLines = entry.attention.map((row) => (
      `${row.mode}: ${row.statusLabel}${row.targetLabel ? ` → ${row.targetLabel}` : ""}`
    ));
    return [
      `${entry.title} (${entry.progress.completed}/${entry.progress.total})`,
      entry.objective || "",
      ...itemLines,
      ...attentionLines,
    ].filter(Boolean).join("\n");
  }

  const text = String(entry.copyText ?? entry.text ?? conversationRichBlocksToText(entry.blocks));
  return entry.blocks?.some((block) => block.type === "markdown") ? text : text.trim();
}

export function conversationMessageHasIntrinsicCopyAction(
  entry: ConversationTimelineEntry,
): boolean {
  if (entry.kind !== "message" || entry.variant !== "rich") {
    return false;
  }

  return Boolean(entry.blocks?.some((block) => conversationRichBlockHasCopyAction(block)));
}

function conversationGroupSubstantiveEntries(
  entries: ConversationTimelineEntry[],
): ConversationTimelineEntry[] {
  return entries.filter((entry) => {
    if (entry.kind === "summary") {
      return true;
    }
    if (entry.kind !== "message" || entry.variant === "meta") {
      return false;
    }
    const blocks = entry.blocks || [];
    return !blocks.length || !blocks.every((block) => block.type === "tool-call");
  });
}

function conversationGroupReconciliationAnchor(
  entries: ConversationTimelineEntry[],
): string | null {
  const substantive = conversationGroupSubstantiveEntries(entries);
  // A late peer tool can carry its own interaction/run id. It must not steal
  // the group key from the substantive response that was already mounted.
  const scanOrder = [...substantive, ...entries.filter((entry) => !substantive.includes(entry))];
  // Strongest key ACROSS the group wins, not the first key in entry order: a
  // host that stamps `groupReconciliationKey` only on the response entry must
  // not have it shadowed by a provisional interactionId on an earlier
  // thinking/summary entry (that provisional id changes when the durable twin
  // lands, remounting the group the host keyed precisely to keep stable).
  for (const entry of scanOrder) {
    const groupReconciliationKey = entry.groupReconciliationKey?.trim();
    if (groupReconciliationKey) {
      return `group-reconciliation-${groupReconciliationKey}`;
    }
  }
  for (const entry of scanOrder) {
    const reconciliationKey = entry.reconciliationKey?.trim();
    if (reconciliationKey) {
      return `reconciliation-${reconciliationKey}`;
    }
  }
  // interaction/run anchors are read from SUBSTANTIVE entries only: a
  // tool-call/meta entry carrying its own interaction id (a late peer tool)
  // joining an otherwise keyless group must not re-key it.
  for (const entry of substantive) {
    const interactionId = entry.interactionId?.trim();
    if (interactionId) {
      return `interaction-${interactionId}`;
    }
  }
  for (const entry of substantive) {
    if (entry.kind === "message") {
      const runId = entry.runId?.trim();
      if (runId) {
        return `run-${runId}`;
      }
    }
  }
  return null;
}

function startsSeparateAssistantResponse(
  current: ConversationTimelineGroup,
  entry: ConversationTimelineEntry,
): boolean {
  if (conversationIdentityPresentation(entry.identity) !== "assistant"
    || entry.kind !== "message"
    || !conversationGroupSubstantiveEntries([entry]).length) {
    return false;
  }

  const groupKey = entry.groupReconciliationKey?.trim();
  const runId = entry.runId?.trim();
  const interactionId = entry.interactionId?.trim();
  const substantive = conversationGroupSubstantiveEntries(current.entries);
  // A host anchor reconciles the whole group, including partial chunks
  // that carry only the provisional transport owner.
  if (groupKey && substantive.some(previous => previous.groupReconciliationKey?.trim() === groupKey)) return false;
  return substantive.some((previous) => {
    if (previous.kind !== "message") return false;
    const previousRunId = previous.runId?.trim();
    const previousInteractionId = previous.interactionId?.trim();
    return Boolean((runId && previousRunId && runId !== previousRunId)
      || (interactionId && previousInteractionId && interactionId !== previousInteractionId));
  });
}

export function groupConversationTimelineEntries(
  entries: ConversationTimelineEntry[],
): ConversationTimelineGroup[] {
  const groups: ConversationTimelineGroup[] = [];
  const turnAnchorsByGroup: string[] = [];
  let turnAnchor = "conversation-start";

  for (const entry of entries) {
    const current = groups.at(-1);
    const identityKey = conversationIdentityGroupKey(entry.identity);
    if (!current || conversationIdentityGroupKey(current.identity) !== identityKey
      || startsSeparateAssistantResponse(current, entry)) {
      if (conversationIdentityPresentation(entry.identity) === "user") {
        // A turn is the durable reconciliation boundary. Entries can arrive
        // late or move ahead of an already-rendered assistant response (peer
        // tool traffic is a common example), so a group cannot be keyed from
        // whichever entry happens to be first on this render. Prefer the
        // host's reconciliation key: a provisional user entry whose id swaps
        // at the durable pass must not re-key every group in its turn.
        turnAnchor =
          entry.groupReconciliationKey?.trim()
          || entry.reconciliationKey?.trim()
          || entry.renderKey
          || entry.id;
      }
      turnAnchorsByGroup.push(turnAnchor);
      groups.push({
        // Placeholder id; every group is re-keyed in the post-pass once its
        // entry list is complete.
        id: `${turnAnchor}-group-${identityKey}-entry-${entry.id}`,
        identity: entry.identity,
        entries: [entry],
        copyText: conversationEntryText(entry),
      });
      continue;
    }

    current.entries.push(entry);
    const nextCopyText = conversationEntryText(entry);
    current.copyText = [current.copyText, nextCopyText].filter(Boolean).join("\n\n");
  }

  // A live response and its durable twin carry the same interaction/run
  // identity even when a late peer tool creates another assistant group
  // earlier in the turn. Prefer that source identity over the first-entry
  // fallback. Anchored ids are NOT guaranteed unique (two same-identity
  // groups split by an interleaved entry can share the turn's interaction
  // id when the host sets no reconciliationKey), and React keys must be:
  // suffix repeat occurrences deterministically by document order — the
  // first occurrence keeps the unsuffixed id, so the common single-group
  // case stays stable across live/durable passes.
  const seenIds = new Set<string>();
  return groups.map((group, index) => {
    const reconciliationAnchor = conversationGroupReconciliationAnchor(group.entries);
    // Fallback: the first SUBSTANTIVE entry's id — structurally unique,
    // immune to late-inserted sibling groups (a positional ordinal shifts
    // every later keyless group's id; in user-less conversations an ordinal
    // sequence never resets), and stable when a tool-call/meta entry is
    // prepended into the group (the late-peer-tool case). Known limitation:
    // a group that STARTS with only tool-call entries re-keys once when its
    // first substantive entry arrives, and a fully keyless group re-keys
    // once if a keyed terminal entry later joins — hosts wanting stability
    // through those transitions should supply reconciliation keys (or
    // interaction ids from the first frame, as the bundled adapter does).
    const anchorEntry = conversationGroupSubstantiveEntries(group.entries)[0] || group.entries[0];
    const occurrenceAnchor = conversationGroupSubstantiveEntries(group.entries)
      .find(entry => entry.kind === "message" && entry.assistantOccurrenceKey);
    const displayAnchor = (occurrenceAnchor?.kind === "message" ? occurrenceAnchor.assistantOccurrenceKey : undefined)
      ?? anchorEntry.renderKey ?? anchorEntry.id;
    const base = reconciliationAnchor
      ? `${turnAnchorsByGroup[index] || "conversation-start"}-group-${conversationIdentityGroupKey(group.identity)}-${reconciliationAnchor}`
      : `${turnAnchorsByGroup[index] || "conversation-start"}-group-${conversationIdentityGroupKey(group.identity)}-entry-${displayAnchor}`;
    let id = base;
    // React keys must be unique. Same-anchor twins keep the unsuffixed id on
    // the FIRST occurrence — the common single-group case stays stable across
    // live/durable passes — and later twins get a suffix keyed by their own
    // anchor entry (position-independent, so a reordered twin keeps its
    // suffix instead of stealing a sibling's). The loop also guards anchors
    // that literally collide with a previously-issued suffixed id.
    // Two responses can share one interaction while owning distinct runs.
    // Prefer the typed run over a live row ID that changes at persistence.
    const runAnchor = conversationGroupSubstantiveEntries(group.entries)
      .find(entry => entry.kind === "message" && entry.runId?.trim());
    let discriminator = runAnchor?.kind === "message" && runAnchor.runId?.trim()
      ? `run-${runAnchor.runId.trim()}` : displayAnchor;
    while (seenIds.has(id)) {
      id = `${base}-dup-${discriminator}`;
      discriminator = `${discriminator}x`;
    }
    seenIds.add(id);
    return { ...group, id, entries: conversationPresentationRows(group.entries) };
  });
}
