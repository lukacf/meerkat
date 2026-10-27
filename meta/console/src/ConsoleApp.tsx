import { ConsoleMcpAppsProvider, type ConsoleMcpAppsHost } from "@console-components";
import { createNativeMcpAppsHost } from "./mcp-apps/native-host";
import { useConsolePanels, createConsolePanelService, bindConsolePanelService } from "./lib/custom-panels";
import { ConsolePanelsProvider, ConsoleCustomPanel } from "@console-components";
import { consoleCustomPanelTarget, type ConsolePanelDefinition, type ConsolePanelContext, type ConsolePanelService } from "@console-core";
import { withConsoleSendStorageLock } from "./lib/send-storage-lock";
import {
  framesAfterIdleWatermark,
  consoleFrameKey,
  decideIdleSettlement,
  idleSettleClock,
  idleWatermarkFor,
  type IdleArm,
  isAfterIdleWatermark,
  type IdleWatermark,
} from "./lib/idle-reconcile";
import React from "react";
import { selectedPanelRuntime, scopedPanelParams, framesForPanelRuntime, loadPanelRuntimeSelection, savePanelRuntimeSelection } from "./lib/panel-runtime";
import "@console-components/styles";
import "./console-host.css";

import {
  ConsoleActivityRail,
  ConsoleComposer,
  ConsoleDock,
  ConsoleSidebar,
  TopologyPanel,
  ConsoleWorkbench,
  ConsoleTransportStatus,
  useConsoleDockController,
} from "@console-components";
import type { ConversationBlobReader, MarkdownUrlPolicy, WorkGraphCardActions } from "@console-components";
import type {
  ConsoleDockState,
  ConsoleTransportState,
  PendingApprovalResource,
  PendingApprovalSnapshot,
  ConsoleWorkbenchTarget,
  ConversationTimelineEntry,
  IdentityInspectViewState,
  IdentityStatusRow,
  TopologyMutationIntent,
  TopologyOperationReceipt,
} from "@console-core";
import {
  collectConsoleDockPanelIds,
  consoleMobStatus,
  createPendingApprovalResource,
  identityStateLabel,
  migrateConsoleWorkbenchTarget,
  normalizeConsoleDockState,
  normalizeIdentityInspectViewState,
  ConsoleActivityProjection,
  topologyMutationIntent,
} from "@console-core";

import {
  buildConsoleIdentityAliasMap,
  canonicalConsoleIdentityFromMap,
  normalizeAgents,
  type ConsoleIdentityAliasMap,
} from "./lib/agents";
import {
  buildControlTarget,
  buildDockTarget,
  buildInspectTarget,
  buildPanelConversationKey,
  buildRoutingSectionView,
  buildWorkGraphOperatorResultFrame,
  createUserEntry,
  createWorkGraphHydrationGate,
  appendOptimisticConversationEntry,
  buildOperatorPanelTarget,
  deriveTimelineEntries,
  framesContainWorkGraphCards,
  inferResponsePhaseFromFrames,
  mapFramesToTimelineEntries,
  operatorPanelPath,
  optimisticUserMessageForPanel,
  resolvePanelResponsePhase,
  systemNoticeClearsBusyState,
  type MetaDockTarget,
  type OptimisticUserMessage,
  type TimelineDerivation,
} from "./lib/adapters";
import { accessSaveFailure, errorMessage, jsonRpcErrorCode } from "./lib/errors";
import { createSingleFlight } from "./lib/single-flight";
import { sanitizeConversationEntries } from "./lib/conversation-visibility";
import {
  DEFAULT_CONSOLE_FETCH_TIMEOUT_MS,
} from "./lib/network";
import {
  CONSOLE_COMMAND_NAMES,
  ConsoleCapabilityUnavailableError,
  consoleCommandMethod,
  createHttpConsoleTransport,
  createMetaConsoleController,
  type MetaConsoleTransport,
} from "./lib/headless";
import {
  WORKGRAPH_CONFLICT_CODE,
  resolveWorkGraphBindingRevision,
  resolveWorkGraphGoalItemRevision,
  resolveWorkGraphItemRevision,
  workGraphClaimOwnerId,
  workGraphConflictRefreshRequest,
  type WorkGraphCommandRunner,
} from "./lib/workgraph-actions";
import { createConsoleId } from "./lib/id";
import {
  createConsoleTopologyMutationRequest,
  executeConsoleTopologyMutation,
  normalizeConsoleTopologyQuery,
  mergeTopologyOperationReceipt,
  pendingTopologyReceipt,
  resolveAmbiguousConsoleTopologyMutation,
  type ConsoleTopologyControlCapabilities,
  type ConsoleTopologyRpcOperation,
} from "./lib/topology";
import { findPaneResizeRoot } from "./lib/pane-resize";
import { activeRunIdFromFrames, describeRunStopReceipt, parseRunStopResult } from "./lib/run-stop";
import { resolveConsoleReadOnlyOverride } from "./lib/read-only-override";
import { Icon, SpriteSheet } from "./icon";
import type {
  ConsoleAccessConfig,
  ConsoleAccessEditBase,
  ConsoleAccessSaveFailure,
  ConsoleAccessRule,
  ConsoleAccessStatus,
  ConsoleActionsUiConfig,
  ConsoleAgent,
  ConsoleExperience,
  ConsoleFrame,
  ConsoleGatingActionPayload,
  ConsoleReplayUnavailablePayload,
  ConsoleTimelinePage,
  MemoryAuditVerdictEntry,
  MemoryDreamRun,
  MemoryDreamRunSheet,
  MemoryEvidenceRef,
  MemoryFullRecord,
  MemoryHarvestEntry,
  MemoryLedgerEntry,
  MemoryPanelAuditVerdictsResult,
  MemoryPanelDreamRunsResult,
  MemoryPanelDreamsResult,
  MemoryPanelHarvestsResult,
  MemoryPanelInjectionsResult,
  MemoryPanelOverviewResult,
  MemoryPanelProposalsResult,
  MemoryPanelQuarantineResult,
  MemoryPanelRecord,
  MemoryPanelRecordResult,
  MemoryPanelRecordsResult,
  MemoryPendingPromotion,
  MemoryProposalEntry,
  MemoryQuarantineDecideResult,
  MemoryQuarantineVerdict,
  WorkGraphEventsResult,
  WorkGraphSnapshotResult,
  WorkGraphWireEvent,
} from "./types";
import {
  type IdentityLogCore,
  cursorSeq,
  mergeFrameUpdate,
  pushFrame,
  resetIdentityLogCore,
  sortedEvents,
  trimIdentityLogCore,
} from "./lib/identity-log";
import { TimelinePanel } from "./panels/TimelinePanel";
import { OperatorPanel } from "./panels/OperatorPanel";
import { GatingInboxPanel } from "./panels/GatingInboxPanel";
import { AccessPanel, type AccessPreviewResult } from "./panels/AccessPanel";
import {
  MemoryPanel,
  memoryFramePivot,
  memorySectionOutcome,
  type MemoryRecordDetail,
} from "./panels/MemoryPanel";
import { RosterPanel } from "./panels/RosterPanel";
import {
  WorkGraphPanel,
  createWorkGraphRefreshSequencer,
  workGraphEventsNewestFirst,
  workGraphEventsParams,
  type WorkGraphPanelData,
} from "./panels/WorkGraphPanel";
import { RoutingPanel } from "./panels/RoutingPanel";
import { LogsPanel } from "./panels/LogsPanel";
import { Topbar } from "./panels/Topbar";
import { useConsoleVariant, type ConsoleTheme } from "./panels/Tweaks";
import {
  Sidebar as DesignSidebar,
  normalizeNavKind,
  pruneStaleSidebarStorage,
  readSidebarStringSet,
  sidebarAgentPinId,
  sidebarPinnedFamilyPinIds,
  sidebarStorageKey,
  writeSidebarStringSet,
  SIDEBAR_PINS_STORAGE_PREFIX,
  type NavKind,
} from "./panels/Sidebar";
import { SignalsRail } from "./panels/SignalsRail";
import { ChatPane, type StagedAttachment } from "./panels/ChatPane";
import { MetaDock } from "./panels/MetaDock";
import { PendingStack, type PendingCheckView, type PendingDeliveredNotice, type PendingItem } from "./panels/PendingStack";
import { beginConsoleSendAttempt, classifyConsoleSendFailure, consoleCheckDelivered, consoleCheckNotFound, createConsoleSendAttempt, describeConsoleCheckFailure, finishConsoleSendAttempt, reconcileConsoleSendReceipt, recoverConsoleSendAttempt, type ConsoleFrozenSendEnvelope, type ConsoleSendAttempt, type ConsoleSendFailure } from "../../packages/console-core/src/send-attempt";
import { createConsoleContextRecord, validateConsoleContexts, type ConsoleContextRecord } from "../../packages/console-core/src/context-record";
import { QuoteContextChips } from "../../packages/console-components/src/conversation/context-chips";
import { editConsoleContextQuote } from "../../packages/console-core/src/context-edit";
import type { ConsoleQuoteSelection } from "../../packages/console-components/src/conversation/context-selection";
import { consoleSendStorageKey, loadConsoleSendAttempts, saveConsoleSendAttempts, readLegacyConsoleQueue, readPredecessorConsoleQueue, consoleLegacyQueueImported, consoleComposerTabId, loadConsoleComposerDraft, saveConsoleComposerDraft } from "./lib/send-attempt-storage";

import { VoiceBar } from "./panels/VoiceBar";
import { useVoiceController } from "./lib/use-voice-controller";
import {
  useVoiceReadiness,
  voiceReadinessDenied,
  voiceReadinessOffersVoice,
  voiceReadinessPending,
} from "./lib/use-voice-readiness";
import { countRender } from "./lib/render-counts";

export interface ConsoleAppProps {
  customPanels?: readonly ConsolePanelDefinition[];
  mcpAppsHost?: ConsoleMcpAppsHost;
  panelService?: ConsolePanelService;
  baseUrl: string;
  /** Opaque host scope covering authority/runtime, realm and authenticated principal. */
  storageNamespace?: string;
  /** Host decisions for Markdown links and images. Resolvers do not grant access. */
  markdownUrlPolicy?: MarkdownUrlPolicy;
  /// Test seam: supply a transport instead of the HTTP one built from
  /// `baseUrl`. Production entry points never set it.
  transport?: MetaConsoleTransport;
}

type RoutingPanelData = ReturnType<typeof buildRoutingSectionView>;
type GatingPanelData = { pending: unknown[]; audit: unknown[] };
type AccessPanelData = {
  scope: string;
  loading: boolean;
  status: ConsoleAccessStatus | null;
  config: ConsoleAccessConfig | null;
  error: string | null;
};
type MemoryPanelData = {
  records: MemoryPanelRecord[];
  realms: string[];
  quarantineRecords: MemoryPanelRecord[];
  pendingPromotions: MemoryPendingPromotion[];
  dreams: MemoryDreamRun[];
  detail: MemoryRecordDetail | null;
  detailLoading: boolean;
  unavailable: boolean;
  error: string | null;
  /// Keyset cursor from the base records page (single-realm only).
  nextCursor: string | null;
  /// Per-section -32030 outcomes so the panel can render "no grant"
  /// (never green) instead of an indistinguishable empty section.
  recordsDenied: boolean;
  dreamsDenied: boolean;
  /// One-row scope-probe outcomes: the scope exists behind a grant this
  /// principal lacks (the unfiltered listing row-filters them silently).
  operatorScopeDenied: boolean;
  mobScopeDenied: boolean;
  /// Phase-2 read surfaces, each with its own -32030 outcome.
  overview: MemoryPanelOverviewResult | null;
  overviewDenied: boolean;
  proposals: MemoryProposalEntry[];
  proposalsDenied: boolean;
  injections: MemoryLedgerEntry[];
  injectionsDenied: boolean;
  harvests: MemoryHarvestEntry[];
  harvestsDenied: boolean;
  dreamRuns: MemoryDreamRunSheet[];
  dreamRunsDenied: boolean;
  auditVerdicts: MemoryAuditVerdictEntry[];
  auditVerdictsDenied: boolean;
  /// The caller is advertised meta/memory/quarantine/decide (reviewer
  /// grant, mutable console). Per-identity grants are still enforced per
  /// call.
  canDecideQuarantine: boolean;
};
type DockPresetId = "single" | "two_columns" | "two_rows" | "grid";

/// Per-identity event log ceiling. The console is a live view, not the
/// archive: once an identity has more than this many frames in memory the
/// oldest (in transcript order) are dropped and the identity's oldest
/// cursor moves forward so "load older history" re-fetches them from the
/// runtime's event log on demand. Trimming is amortised: it runs once the
/// log exceeds the ceiling by the slack, and removes down to the ceiling.
const MAX_IDENTITY_LOG_EVENTS = 5000;
const IDENTITY_LOG_TRIM_SLACK = 500;
/// Coalesced render flush cadence while the tab is hidden (no rAF there).
const HIDDEN_TAB_FLUSH_MS = 250;
/// Animation frames between renders while only streamed text has arrived.
const STREAM_TEXT_FRAMES = 3;
/// Why a render is requested: streamed text (paced), any other change (next
/// animation frame), or a stream's completion (at once).
type RenderCause = "text" | "frame" | "now";

/// See `IdentityLogCore` (src/lib/identity-log.ts) for the frame store and
/// its transcript-ordered view; this adds the console's paging and busy
/// state around it.
interface IdentityLog extends IdentityLogCore {
  /// Incrementally folded busy lifecycle, valid while `busyFoldedThrough`
  /// tracks the newest lifecycle frame seen in timestamp order. An older
  /// lifecycle frame arriving late invalidates it and forces one replay.
  busyLifecycle: ConsoleActivityProjection;
  busyFoldedThroughMs: number;
  busyFoldValid: boolean;
  /// Set when two member status polls in a row reported the member idle:
  /// every frame at or before this watermark is settled, and the busy and
  /// phase folds, including a full replay, read only newer frames (see
  /// `lib/idle-reconcile.ts`).
  idleSettledThrough?: IdleWatermark;
  /// The first of those idle polls: the log version then, and when.
  idlePollArmed?: IdleArm;
  /// `null` while we haven't asked the server yet; `true` if the
  /// runtime has an EventLogStore (we'll fetch backfill); `false`
  /// once we've observed `available: false` (SSE is the only source).
  hasServerLog: boolean | null;
  oldestTimelineCursor?: string;
  latestTimelineCursor?: string;
  olderHistoryExhausted?: boolean;
  olderHistoryExhaustedAtCursor?: string;
  olderHistoryLoading?: boolean;
}

/// The server holds older pages than the log has loaded: the pane offers
/// "Load older history", and the transcript keeps session notices older than
/// every loaded frame back until those pages arrive.
function identityHasOlderHistory(log: IdentityLog): boolean {
  return log.hasServerLog === true && Boolean(log.oldestTimelineCursor) && log.olderHistoryExhausted !== true;
}

function normalizeConsoleTheme(value: unknown): ConsoleTheme | null {
  return value === "dark" || value === "light" ? value : null;
}

function normalizeConsoleVariant(
  value: unknown,
): "rams" | "terminal" | "graphite" | null {
  return value === "rams" || value === "terminal" || value === "graphite"
    ? value
    : null;
}

function normalizeDockPreset(value: unknown): DockPresetId | null {
  return value === "single" ||
    value === "two_columns" ||
    value === "two_rows" ||
    value === "grid"
    ? value
    : null;
}

function actionLabel(
  actions: ConsoleActionsUiConfig | undefined,
  key: keyof ConsoleActionsUiConfig,
  fallback: string,
): string {
  const value = actions?.[key];
  return typeof value === "string" && value.trim() ? value.trim() : fallback;
}

function actionVisible(
  actions: ConsoleActionsUiConfig | undefined,
  key: keyof ConsoleActionsUiConfig,
): boolean {
  return actions?.[key] !== false;
}

function normalizeConsoleInspectResult(
  value: unknown,
): IdentityInspectViewState | null {
  const direct = normalizeIdentityInspectViewState(value);
  if (direct) return direct;
  const record =
    value && typeof value === "object"
      ? (value as Record<string, unknown>)
      : {};
  const identityRecord =
    record.identity && typeof record.identity === "object"
      ? (record.identity as Record<string, unknown>)
      : null;
  if (!identityRecord) return null;
  return normalizeIdentityInspectViewState({
    identity: identityRecord.identity,
    display_name: identityRecord.display_name,
    role:
      identityRecord.labels && typeof identityRecord.labels === "object"
        ? (identityRecord.labels as Record<string, unknown>).role
        : undefined,
    state: identityRecord.health,
    addressability:
      identityRecord.addressable === true ? "addressable" : "internal_only",
    session_id: identityRecord.session_id,
    labels: identityRecord.labels,
    continuity: {
      session_id: identityRecord.session_id,
      agent_runtime_id: identityRecord.runtime_member_id,
    },
    topology_peers: Array.isArray(record.peers) ? record.peers : [],
  });
}

const DEFAULT_APPROVER_ID = "console-ops-lead";

/// Live signals that should freshen a docked WorkGraph panel: dedicated
/// workgraph.* timeline frames or workgraph_* tool completions in any turn.
function isWorkGraphSignalFrame(frame: ConsoleFrame): boolean {
  if (frame.event.startsWith("workgraph.")) return true;
  if (frame.event !== "tool_execution_completed" && frame.event !== "tool_result_received") {
    return false;
  }
  const data = frame.data && typeof frame.data === "object"
    ? (frame.data as Record<string, unknown>)
    : null;
  const name = typeof data?.name === "string"
    ? data.name
    : typeof data?.tool_name === "string"
      ? data.tool_name
      : "";
  return name.startsWith("workgraph_");
}
const DOCK_LAYOUT_STORAGE_PREFIX = "meta-console-dock-state";

function createIdempotencyKey(): string {
  return createConsoleId("console");
}

function dockLayoutStorageKey(authorityScope: string): string {
  return `${DOCK_LAYOUT_STORAGE_PREFIX}:${authorityScope}`;
}

function stableHash(value: string): string {
  let hash = 5381;
  for (let i = 0; i < value.length; i += 1) {
    hash = ((hash << 5) + hash) ^ value.charCodeAt(i);
  }
  return (hash >>> 0).toString(36);
}

function sidebarAgentListConfigIdentity(experience: ConsoleExperience | null): string {
  const agentList = experience?.console_config?.agent_list;
  if (!agentList) return "no-agent-list";
  const sections = (agentList.sections || []).map((section) => ({
    name: section.name,
    empty_title: section.empty_title,
    empty_text: section.empty_text,
  }));
  return stableHash(JSON.stringify({
    group_by: agentList.group_by || [],
    subgroup_by: agentList.subgroup_by || [],
    section_order: agentList.section_order || [],
    fallback_group: agentList.fallback_group || "",
    fallback_subgroup: agentList.fallback_subgroup || "",
    collapse_single_subgroup: agentList.collapse_single_subgroup !== false,
    sections,
  }));
}

function sidebarPreferencesScope(
  baseUrl: string,
  experience: ConsoleExperience | null,
): string {
  const runtimeId = experience?.runtime_id?.trim();
  const title = experience?.console_config?.title?.trim();
  return runtimeId || title || baseUrl;
}

function sidebarPreferencesNamespace(
  baseUrl: string,
  experience: ConsoleExperience | null,
): string {
  return [sidebarPreferencesScope(baseUrl, experience), sidebarAgentListConfigIdentity(experience)]
    .map((part) => encodeURIComponent(part))
    .join(":");
}

function browserLocalStorage(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/// Settle a saved send attempt in its own namespace after the view that
/// dispatched it ended (remount) or moved to another principal's scope. Runs
/// under the same storage lock and merge rules as every durable writer, then
/// announces the key so a live view of that namespace reloads it.
async function settleDetachedConsoleSendAttempt(
  namespace: string,
  identity: string,
  id: string,
  settle: (attempt: ConsoleSendAttempt) => ConsoleSendAttempt,
  removeWhenAccepted: boolean,
): Promise<void> {
  const storage = browserLocalStorage();
  if (!storage) return;
  const key = consoleSendStorageKey(namespace, identity);
  await withConsoleSendStorageLock(key, () => {
    const loaded = loadConsoleSendAttempts(storage, namespace, identity);
    if (loaded.kind !== "ready") return;
    const current = loaded.attempts.find((attempt) => attempt.id === id);
    if (!current?.envelopeJson || current.state === "accepted" || current.state === "draft") return;
    const settled = settle(current);
    const next = removeWhenAccepted && settled.state === "accepted"
      ? loaded.attempts.filter((attempt) => attempt.id !== id)
      : loaded.attempts.map((attempt) => attempt.id === id ? settled : attempt);
    saveConsoleSendAttempts(storage, namespace, identity, next, loaded.attempts);
  }).catch(() => { /* A live view recovers the lease to "Acceptance unknown". */ });
  try { window.dispatchEvent(new StorageEvent("storage", { key })); } catch { /* No window. */ }
}

function browserComposerStorage(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    // Browsers clone sessionStorage for duplicated tabs, then isolate writes.
    // Keeping unsent composers here preserves reloads without allowing a
    // copied tab ID to overwrite another tab's draft in localStorage.
    return window.sessionStorage;
  } catch {
    return null;
  }
}

function isTerminalTurnCompletedFrame(frame: ConsoleFrame): boolean {
  if (frame.event !== "turn_completed") return false;
  const data =
    frame.data && typeof frame.data === "object"
      ? (frame.data as Record<string, unknown>)
      : {};
  const stopReason = data.stop_reason ?? data.stopReason;
  return typeof stopReason === "string" ? stopReason !== "tool_use" : true;
}

function isActiveServerToolContentFrame(frame: ConsoleFrame): boolean {
  if (frame.event !== "server_tool_content") return false;
  const record =
    frame.data && typeof frame.data === "object"
      ? (frame.data as Record<string, unknown>)
      : null;
  const content =
    record?.content && typeof record.content === "object"
      ? (record.content as Record<string, unknown>)
      : null;
  const type =
    typeof content?.type === "string"
      ? content.type
      : typeof record?.type === "string"
        ? record.type
        : "";
  if (
    type === "message_annotations" ||
    Array.isArray(content?.annotations) ||
    type.includes(".completed") ||
    type.includes(".done") ||
    type.includes(".failed") ||
    type.includes(".error")
  ) {
    return false;
  }
  return (
    type.includes(".in_progress") ||
    type.includes(".searching") ||
    type.includes(".started") ||
    type.includes("_call")
  );
}

function isTerminalServerToolContentFrame(frame: ConsoleFrame): boolean {
  if (frame.event !== "server_tool_content") return false;
  const record =
    frame.data && typeof frame.data === "object"
      ? (frame.data as Record<string, unknown>)
      : null;
  const content =
    record?.content && typeof record.content === "object"
      ? (record.content as Record<string, unknown>)
      : null;
  const type =
    typeof content?.type === "string"
      ? content.type
      : typeof record?.type === "string"
        ? record.type
        : "";
  const status =
    typeof content?.status === "string"
      ? content.status
      : typeof record?.status === "string"
        ? record.status
        : "";
  if (type === "message_annotations" || Array.isArray(content?.annotations)) {
    return false;
  }
  return (
    type.includes(".completed") ||
    type.includes(".done") ||
    type.includes(".failed") ||
    type.includes(".error") ||
    status === "completed" ||
    status === "done" ||
    status === "succeeded" ||
    status === "failed" ||
    status === "error"
  );
}

// --- Event sets for the SSE handler ---
const REFRESH_TRIGGER_EVENTS = new Set([
  "interaction_started",
  "run_started",
  "interaction_complete",
  "interaction_failed",
  "state_changed",
  "member_ready",
  "member_retired",
  "topology_updated",
  "gating_decision",
  "route_changed",
  "tool_call_requested",
  "tool_call",
  "tool_result_received",
  "tool_execution_started",
  "tool_execution_completed",
  "server_tool_content",
]);
/// Periodic experience refreshes rebuild every agent record. Keep unchanged
/// records (and the list, when nothing changed) so the roster, transcripts
/// and panels memoised on them do not re-render or re-derive.
function internAgents(current: ConsoleAgent[], next: ConsoleAgent[]): ConsoleAgent[] {
  const byKey = new Map(current.map((agent) => [JSON.stringify(agent), agent]));
  let changed = next.length !== current.length;
  const interned = next.map((agent, index) => {
    const prior = byKey.get(JSON.stringify(agent));
    if (!prior || prior !== current[index]) changed = true;
    return prior ?? agent;
  });
  return changed ? interned : current;
}

const PANEL_ROUTABLE_EVENTS = new Set([
  "mcp_app",
  "user_input",
  "interaction_started",
  "interaction_complete",
  "interaction_failed",
  "assistant_image",
  "assistant_image_appended",
  "text_delta",
  "text_complete",
  "reasoning_delta",
  "reasoning_complete",
  "turn_completed",
  "tool_call_requested",
  "tool_call",
  "tool_result_received",
  "tool_execution_started",
  "tool_execution_completed",
  "server_tool_content",
  "run_started",
  "run_completed",
  "run_failed",
  "message_delivery_failed",
  "system_notice",
  "boundary_append_applied",
  "boundary_appends_discarded",
  "runtime_notice_snapshot",
  "assistant_history_snapshot",
  "frame_updated",
]);
const HISTORY_REFRESH_EVENTS = new Set([
  "interaction_complete",
  "interaction_failed",
  "run_completed",
  "run_failed",
  "message_delivery_failed",
]);
// Events filtered from the activity rail — don't buffer them
const ACTIVITY_SKIP_EVENTS = new Set([
  "mcp_app",
  "subscribed",
  "run_started",
  "run_completed",
  "turn_started",
  "turn_completed",
  "text_complete",
  "reasoning_delta",
  "reasoning_complete",
  "snapshot_complete",
  "snapshot_started",
  "assistant_history_snapshot",
  "run_failed",
  "keep-alive",
  "tool_config_changed",
  "tool_scope_changed",
  "frame_updated",
  "text_delta",
  "tool_call",
  "tool_execution_started",
  "tool_result_received",
  "tool_execution_completed",
  "server_tool_content",
]);

// ============================================================================
// CONSOLE APP
// ============================================================================

function emptyMemoryPanelData(): MemoryPanelData {
  return {
    records: [],
    realms: [],
    quarantineRecords: [],
    pendingPromotions: [],
    dreams: [],
    detail: null,
    detailLoading: false,
    unavailable: false,
    error: null,
    nextCursor: null,
    recordsDenied: false,
    dreamsDenied: false,
    operatorScopeDenied: false,
    mobScopeDenied: false,
    overview: null,
    overviewDenied: false,
    proposals: [],
    proposalsDenied: false,
    injections: [],
    injectionsDenied: false,
    harvests: [],
    harvestsDenied: false,
    dreamRuns: [],
    dreamRunsDenied: false,
    auditVerdicts: [],
    auditVerdictsDenied: false,
    canDecideQuarantine: false,
  };
}

function emptyWorkGraphPanelData(): WorkGraphPanelData {
  return {
    items: [],
    edges: [],
    attention: [],
    events: [],
    capturedAt: null,
    unavailable: false,
    denied: false,
    error: null,
  };
}

export function ConsoleApp(props: ConsoleAppProps): React.JSX.Element {
  // All authorized state belongs to one host authority and transport lifetime.
  // A keyed instance clears it in the same commit as the host scope change.
  const instanceKey = React.useMemo(() => createConsoleId("console-instance"), [props.baseUrl, props.transport, props.storageNamespace, props.panelService, props.mcpAppsHost]);
  return <ConsoleAppAuthority key={instanceKey} {...props} />;
}

function ConsoleAppAuthority(props: ConsoleAppProps): React.JSX.Element {
  const observedScope = React.useRef<{ value: string | undefined } | null>(null);
  const [generation, setGeneration] = React.useState(0);
  const observeScope = React.useCallback((value: string | undefined): boolean => {
    const previous = observedScope.current;
    observedScope.current = { value };
    if (previous && previous.value !== value) {
      // A same-URL account switch ends all old subscriptions, locks, drafts
      // and transcript state before the replacement authority loads.
      setGeneration(current => current + 1);
      return false;
    }
    return true;
  }, []);
  return <ConsoleAppInstance key={generation} {...props} observeScope={observeScope} />;
}

function ConsoleAppInstance({ baseUrl, transport, storageNamespace, markdownUrlPolicy, customPanels: suppliedPanels, panelService, mcpAppsHost, observeScope }: ConsoleAppProps & { observeScope: (scope: string | undefined) => boolean }): React.JSX.Element {
  countRender("ConsoleApp");
  const lifetimeRef = React.useRef({ active: true, generation: 0 });
  React.useLayoutEffect(() => {
    lifetimeRef.current.active = true;
    lifetimeRef.current.generation += 1;
    return () => {
      lifetimeRef.current.active = false;
      lifetimeRef.current.generation += 1;
    };
  }, []);
  const consoleFetchTimeoutMsRef = React.useRef(DEFAULT_CONSOLE_FETCH_TIMEOUT_MS);
  const consoleTransport = React.useMemo(
    () => {
      const source = transport ?? createHttpConsoleTransport({
        baseUrl,
        fetchTimeoutMs: () => consoleFetchTimeoutMsRef.current,
      });
      const current = (generation = lifetimeRef.current.generation) => {
        if (!lifetimeRef.current.active || lifetimeRef.current.generation !== generation) {
          throw new DOMException("Console authority lifetime ended", "AbortError");
        }
      };
      const call = async <T,>(operation: () => Promise<T>): Promise<T> => {
        const generation = lifetimeRef.current.generation;
        current(generation);
        const value = await operation();
        current(generation);
        return value;
      };
      const scoped: MetaConsoleTransport = {
        loadExperience: () => call(() => source.loadExperience()),
        loadModules: source.loadModules ? () => call(() => source.loadModules!()) : undefined,
        capabilities: () => call(() => source.capabilities()),
        queryTimeline: input => call(() => source.queryTimeline(input)),
        send: input => call(() => source.send(input)),
        executeCommand: source.executeCommand ? input => call(() => source.executeCommand!(input)) : undefined,
        upload: source.upload ? input => call(() => source.upload!(input)) : undefined,
        readBlob: source.readBlob ? input => call(() => source.readBlob!(input)) : undefined,
        blobUrl: source.blobUrl ? id => { current(); return source.blobUrl!(id); } : undefined,
        subscribeTimeline(input, onFrame, options) {
          const generation = lifetimeRef.current.generation;
          current(generation);
          const active = () => lifetimeRef.current.active && lifetimeRef.current.generation === generation;
          return source.subscribeTimeline({ ...input, onTransportState: state => { if (active()) input.onTransportState?.(state); } },
            frame => { if (active()) onFrame(frame); },
            { ...options, onTransportState: state => { if (active()) options?.onTransportState?.(state); } });
        },
      };
      return scoped;
    },
    [baseUrl, transport],
  );
  const consoleController = React.useMemo(
    () => createMetaConsoleController({ transport: consoleTransport }),
    [consoleTransport],
  );
  // Stored images are read with the transport's own authority and lifetime:
  // the scoped transport refuses a read once this Console instance retires.
  const blobReader = React.useMemo<ConversationBlobReader | undefined>(
    () => consoleTransport.readBlob ? input => consoleTransport.readBlob!(input) : undefined,
    [consoleTransport],
  );
  const { voice, state: voiceState } = useVoiceController(baseUrl);
  const sampleVoiceWaveform = React.useCallback(
    (source: "microphone" | "speaker", samples: Float32Array<ArrayBuffer>) => voice?.sampleWaveform(source, samples),
    [voice],
  );

  // --- Low-frequency React state (UI-driven) ---
  /// Explicit Check state per identity and row id (view state, never saved).
  const [pendingChecks, setPendingChecks] = React.useState<Record<string, Record<string, PendingCheckView>>>({});
  /// "Delivered at <time>" notices for rows a Check found and cleared.
  const [deliveredNotices, setDeliveredNotices] = React.useState<Record<string, PendingDeliveredNotice[]>>({});
  const [experience, setExperience] = React.useState<ConsoleExperience | null>(
    null,
  );
  const persistentSendScope = storageNamespace?.trim() || (experience?.storage_scope?.trim()
    ? JSON.stringify([baseUrl, experience.storage_scope]) : null);
  const savedPanelRuntimeSelection = React.useMemo(
    () => loadPanelRuntimeSelection(browserLocalStorage(), persistentSendScope),
    [persistentSendScope],
  );
  const [panelRuntimeSelection, setPanelRuntimeSelection] = React.useState<{scope: string | null; key: string} | null>(null);
  const explicitPanelRuntime = panelRuntimeSelection?.scope === persistentSendScope
    ? panelRuntimeSelection.key : savedPanelRuntimeSelection;
  const panelRuntimeKey = selectedPanelRuntime(explicitPanelRuntime, experience?.panel_runtimes);
  // An explicitly empty shared registry is a setup state, not an access grant.
  // Missing experience or a denied configured service must keep its normal gate.
  const emptyPanelRegistry = experience?.panel_runtimes?.length === 0
    && experience.memory?.available === false
    && experience.workgraph?.available === false;
  React.useEffect(() => {
    if (!panelRuntimeKey) return;
    setPanelRuntimeSelection(current => current?.scope === persistentSendScope && current.key === panelRuntimeKey
      ? current : {scope: persistentSendScope, key: panelRuntimeKey});
    savePanelRuntimeSelection(browserLocalStorage(), persistentSendScope, panelRuntimeKey);
  }, [persistentSendScope, panelRuntimeKey]);
  const panelRuntimeKeyRef = React.useRef(panelRuntimeKey);
  const memoryDetailRequestRef = React.useRef(0);
  panelRuntimeKeyRef.current = panelRuntimeKey;
  const [memoryDataRuntimeKey, setMemoryDataRuntimeKey] = React.useState("");
  const [workGraphDataRuntimeKey, setWorkGraphDataRuntimeKey] = React.useState("");
  const [agents, setAgents] = React.useState<ConsoleAgent[]>([]);
  // Roster labels keyed by public alias (durable identity and member id), so
  // transcript rows can name a peer the way the sidebar does.
  const peerLabels = React.useMemo(() => {
    const labels = new Map<string, string>();
    for (const agent of agents) {
      const label = agent.label?.trim();
      if (!label) continue;
      const identity = agent.identity?.trim();
      if (identity) labels.set(identity, label);
      const memberId = agent.member_id?.trim();
      if (memberId && !labels.has(memberId)) labels.set(memberId, label);
    }
    return labels;
  }, [agents]);
  const chatDisplayLabels = React.useMemo(() => ({ peers: peerLabels }), [peerLabels]);
  const [draftByKey, setDraftByKey] = React.useState<Record<string, string>>(
    {},
  );
  const [stagedAttachmentsByIdentity, setStagedAttachmentsByIdentity] =
    React.useState<Record<string, StagedAttachment[]>>({});
  const [sendingPanels, setSendingPanels] = React.useState<Set<string>>(
    new Set(),
  );
  const sendingPanelsRef = React.useRef(sendingPanels);
  sendingPanelsRef.current = sendingPanels;
  const [pinnedAgentIds, setPinnedAgentIds] = React.useState<Set<string>>(
    new Set(),
  );
  const [inspectByIdentity, setInspectByIdentity] = React.useState<
    Record<string, IdentityInspectViewState | null>
  >({});
  const [routingData, setRoutingData] = React.useState<RoutingPanelData>({
    routes: [],
    deliveries: [],
  });
  const [gatingData, setGatingData] = React.useState<GatingPanelData>({
    pending: [],
    audit: [],
  });
  const [accessData, setAccessData] = React.useState<AccessPanelData>({
    scope: "", loading: false,
    status: null,
    config: null,
    error: null,
  });
  const [memoryData, setMemoryData] = React.useState<MemoryPanelData>(emptyMemoryPanelData);
  const [workGraphData, setWorkGraphData] = React.useState<WorkGraphPanelData>(emptyWorkGraphPanelData);
  React.useEffect(() => {
    // Never display a previous repository's cached data under a new scope,
    // including when that new scope fails to load or disappears.
    setMemoryDataRuntimeKey("");
    setWorkGraphDataRuntimeKey("");
    memoryDetailRequestRef.current += 1;
    setMemoryData(emptyMemoryPanelData());
    setWorkGraphData(emptyWorkGraphPanelData());
  }, [panelRuntimeKey]);
  const [topologyQueryResult, setTopologyQueryResult] = React.useState<unknown>(null);
  const [topologyCapabilities, setTopologyCapabilities] =
    React.useState<ConsoleTopologyControlCapabilities | null>(null);
  const [topologyOperations, setTopologyOperations] =
    React.useState<TopologyOperationReceipt[]>([]);
  const [topologyConnectionSourceId, setTopologyConnectionSourceId] =
    React.useState<string | null>(null);
  const [activeActivityPresetId, setActiveActivityPresetId] =
    React.useState("");
  const [selectedRosterMemberId, setSelectedRosterMemberId] =
    React.useState("");
  const [loading, setLoading] = React.useState(true);
  // Per-identity reactive flag for an in-flight initial session-history fetch, so
  // the chat pane can show a loading indicator instead of an empty "No messages
  // yet" while a (potentially large) history is loading. (timelineFetchInFlightRef
  // is a ref and doesn't trigger re-renders.)
  const [loadingHistory, setLoadingHistory] = React.useState<Record<string, boolean>>({});
  const [error, setError] = React.useState("");
  // Recoverable per-action failures (e.g. an access-denied send). Rendered
  // as a dismissible banner inside the shell — never the fatal error screen.
  const [actionError, setActionError] = React.useState("");
  // Last run-stop receipt summary per identity (shown in the chat header).
  const [runStopNotices, setRunStopNotices] = React.useState<Record<string, string>>({});
  const [transportState, setTransportState] = React.useState<ConsoleTransportState>({
    phase: "connecting", stale: true, freshness: "unknown",
  });
  const [transportRetry, setTransportRetry] = React.useState(0);
  const [theme, setTheme] = React.useState<ConsoleTheme>(() => {
    try {
      return (
        (localStorage.getItem("meta-console-theme") as ConsoleTheme) ||
        "light"
      );
    } catch {
      return "light";
    }
  });
  const [variant, setVariant] = useConsoleVariant();
  const sidebarStorageScope = React.useMemo(
    () => sidebarPreferencesScope(baseUrl, experience),
    [baseUrl, experience],
  );
  const sidebarStorageNamespace = React.useMemo(
    () => sidebarPreferencesNamespace(baseUrl, experience),
    [baseUrl, experience],
  );
  const sidebarPinsStorageKey = React.useMemo(
    () => sidebarStorageKey(SIDEBAR_PINS_STORAGE_PREFIX, sidebarStorageNamespace),
    [sidebarStorageNamespace],
  );
  React.useEffect(() => {
    pruneStaleSidebarStorage(browserLocalStorage(), sidebarStorageScope, sidebarStorageNamespace);
  }, [sidebarStorageScope, sidebarStorageNamespace]);

  const [sidebarCollapsed, setSidebarCollapsed] = React.useState<boolean>(
    () => {
      try {
        return localStorage.getItem("meta-console-sidebar-collapsed") === "1";
      } catch {
        return false;
      }
    },
  );
  const toggleSidebarCollapsed = React.useCallback(() => {
    setSidebarCollapsed((c) => {
      const next = !c;
      try {
        localStorage.setItem(
          "meta-console-sidebar-collapsed",
          next ? "1" : "0",
        );
      } catch {
        /* ignore */
      }
      return next;
    });
  }, []);

  const [railCollapsed, setRailCollapsed] = React.useState<boolean>(() => {
    try {
      return localStorage.getItem("meta-console-rail-collapsed") === "1";
    } catch {
      return false;
    }
  });
  const toggleRailCollapsed = React.useCallback(() => {
    setRailCollapsed((c) => {
      const next = !c;
      try {
        localStorage.setItem("meta-console-rail-collapsed", next ? "1" : "0");
      } catch {
        /* ignore */
      }
      return next;
    });
  }, []);

  const defaultPinnedAgentIdsKey = React.useMemo(
    () => JSON.stringify(experience?.console_config?.agent_list?.default_pinned_agent_ids || []),
    [experience?.console_config?.agent_list?.default_pinned_agent_ids],
  );
  React.useEffect(() => {
    const defaults = new Set(experience?.console_config?.agent_list?.default_pinned_agent_ids || []);
    const stored = readSidebarStringSet(
      browserLocalStorage(),
      sidebarPinsStorageKey,
    );
    setPinnedAgentIds(stored ?? defaults);
  }, [defaultPinnedAgentIdsKey, experience?.console_config?.agent_list, sidebarPinsStorageKey]);

  const togglePinnedAgent = React.useCallback((agent: ConsoleAgent, renderedFamilyPinIds?: Set<string>) => {
    const pinId = sidebarAgentPinId(agent);
    setPinnedAgentIds((current) => {
      const next = new Set(current);
      const familyPinIds = renderedFamilyPinIds && renderedFamilyPinIds.size > 0
        ? renderedFamilyPinIds
        : sidebarPinnedFamilyPinIds(agent, agents);
      const familyPinned = Array.from(familyPinIds).some((id) => next.has(id));
      // Pins are matched on either durable ids or volatile member_ids. When a
      // descendant pin pulls an ancestor into Pinned for context, unpinning the
      // ancestor should clear the visible pinned family instead of looking inert.
      if (next.has(pinId) || next.has(agent.member_id) || familyPinned) {
        for (const id of familyPinIds) next.delete(id);
      } else {
        next.add(pinId);
      }
      writeSidebarStringSet(
        browserLocalStorage(),
        sidebarPinsStorageKey,
        next,
      );
      return next;
    });
  }, [agents, sidebarPinsStorageKey]);

  // --- Render trigger ---
  const [, setRenderTick] = React.useState(0);
  const liveFramesRef = React.useRef<ConsoleFrame[]>([]);
  const [liveFrames, setLiveFrames] = React.useState<ConsoleFrame[]>([]);
  // One render per animation frame, however many frames or refreshes ask
  // for it. Every SSE frame used to call setRenderTick directly, so a burst
  // of 20 deltas cost 20 full app renders; now they cost one. Browsers do
  // not run requestAnimationFrame in a hidden tab, so while the document is
  // hidden the flush runs on a 250 ms timer instead (frames keep landing in
  // the refs either way), and a pending timer flush is brought forward the
  // moment the tab becomes visible.
  //
  // Streamed text alone renders less often: on every STREAM_TEXT_FRAMES-th
  // animation frame, counted in frames so it slows with the frame rate
  // instead of fighting it, and as a transition, so a keystroke always
  // preempts it. Any other frame commits on the next animation frame, and a
  // stream's completion commits at once.
  const renderScheduledRef = React.useRef<{ kind: "raf" | "timeout"; id: number; textOnly: boolean; frames: number } | null>(null);
  const liveFramesDirtyRef = React.useRef(false);
  const flushScheduledRender = React.useCallback((transition = false) => {
    renderScheduledRef.current = null;
    countRender("LiveRenderFlush");
    const commit = () => {
      if (liveFramesDirtyRef.current) {
        liveFramesDirtyRef.current = false;
        setLiveFrames(liveFramesRef.current);
      }
      setRenderTick((n) => n + 1);
    };
    if (transition) React.startTransition(commit);
    else commit();
  }, []);
  const onScheduledFrame = React.useCallback(() => {
    const pending = renderScheduledRef.current;
    if (!pending || pending.kind !== "raf") return;
    pending.frames += 1;
    if (pending.textOnly && pending.frames < STREAM_TEXT_FRAMES) {
      pending.id = window.requestAnimationFrame(onScheduledFrame);
      return;
    }
    flushScheduledRender(pending.textOnly);
  }, [flushScheduledRender]);
  const cancelScheduledRender = React.useCallback(() => {
    const pending = renderScheduledRef.current;
    if (!pending || typeof window === "undefined") return;
    if (pending.kind === "raf") window.cancelAnimationFrame(pending.id);
    else window.clearTimeout(pending.id);
    renderScheduledRef.current = null;
  }, []);
  const forceRender = React.useCallback((cause: RenderCause = "frame") => {
    const pending = renderScheduledRef.current;
    if (cause === "now") {
      cancelScheduledRender();
      flushScheduledRender();
      return;
    }
    if (pending !== null) {
      // Anything but streamed text ends the text cadence: next frame, urgent.
      if (cause !== "text") pending.textOnly = false;
      return;
    }
    const hidden = typeof document !== "undefined" && document.visibilityState === "hidden";
    if (!hidden && typeof window !== "undefined" && typeof window.requestAnimationFrame === "function") {
      renderScheduledRef.current = {
        kind: "raf",
        id: window.requestAnimationFrame(onScheduledFrame),
        textOnly: cause === "text",
        frames: 0,
      };
      return;
    }
    renderScheduledRef.current = {
      kind: "timeout",
      id: window.setTimeout(() => flushScheduledRender(), hidden ? HIDDEN_TAB_FLUSH_MS : 16),
      textOnly: false,
      frames: 0,
    };
  }, [cancelScheduledRender, flushScheduledRender, onScheduledFrame]);
  React.useEffect(() => {
    if (typeof document === "undefined") return;
    const onVisibilityChange = () => {
      if (document.visibilityState !== "visible") return;
      if (renderScheduledRef.current?.kind !== "timeout") return;
      cancelScheduledRender();
      flushScheduledRender();
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      cancelScheduledRender();
    };
  }, [cancelScheduledRender, flushScheduledRender]);
  const stagedAttachmentsRef = React.useRef(stagedAttachmentsByIdentity);
  React.useEffect(() => {
    stagedAttachmentsRef.current = stagedAttachmentsByIdentity;
  }, [stagedAttachmentsByIdentity]);
  React.useEffect(
    () => () => {
      for (const items of Object.values(stagedAttachmentsRef.current)) {
        items.forEach((item) => URL.revokeObjectURL(item.previewUrl));
      }
    },
    [],
  );

  function setStagedAttachmentsForIdentity(
    identity: string,
    action: React.SetStateAction<StagedAttachment[]>,
  ) {
    setStagedAttachmentsByIdentity((current) => {
      const previous = current[identity] ?? [];
      const next = typeof action === "function" ? action(previous) : action;
      const updated = { ...current };
      if (next.length > 0) updated[identity] = next;
      else delete updated[identity];
      return updated;
    });
  }

  async function inspectIdentityViaHeadless(identity: string): Promise<unknown> {
    return executeHeadlessCommand(
      CONSOLE_COMMAND_NAMES.inspectIdentity,
      identityWorkbenchTarget(identity, "inspect"),
    );
  }

  function requireWorkbenchTarget(input: unknown): ConsoleWorkbenchTarget {
    const target = migrateConsoleWorkbenchTarget(input);
    if (!target) {
      throw new Error("invalid Meta console target");
    }
    return target;
  }

  function identityWorkbenchTarget(identity: string, mode: "chat" | "inspect"): ConsoleWorkbenchTarget {
    return requireWorkbenchTarget({
      id: mode === "inspect" ? `inspect:${identity}` : `chat:${identity}`,
      kind: mode === "inspect" ? "identity-inspect" : "agent-chat",
      title: identity,
      identity,
    });
  }

  function controlWorkbenchTarget(kind: "routing" | "gating" | "access" | "memory" | "workgraph" | "topology"): ConsoleWorkbenchTarget {
    return requireWorkbenchTarget(buildControlTarget(kind));
  }

  function runtimeScopedPanel(kind: "memory" | "workgraph", content: React.ReactNode) {
    if (emptyPanelRegistry) return <div className="console-panel runtime-panel-empty">
      <h2>{kind === "memory" ? "Memory" : "WorkGraph"}</h2>
      <p role="status">No teams are running.</p>
      <p>Finish setting up a team to inspect its memory and work graph.</p>
    </div>;
    const runtimes = experience?.panel_runtimes;
    if (!runtimes) return content;
    const loaded = (kind === "memory" ? memoryDataRuntimeKey : workGraphDataRuntimeKey) === panelRuntimeKey;
    return <div className="runtime-scoped-panel" key={`${kind}:${panelRuntimeKey}`}>
      <label className="runtime-panel-selector">Mob
        <select aria-label={`${kind === "memory" ? "Memory" : "Work Graph"} mob`} value={panelRuntimeKey}
          onChange={event => setPanelRuntimeSelection({scope: persistentSendScope, key: event.target.value})}>
          {!runtimes.some(runtime => runtime.key === panelRuntimeKey) && <option value={panelRuntimeKey}>Runtime unavailable</option>}
          {runtimes.map(runtime => <option key={runtime.key} value={runtime.key}>{runtime.label}</option>)}
        </select>
      </label>
      {loaded ? content : <div className="console-panel" role="status">Loading {kind === "memory" ? "memory" : "work graph"}...</div>}
    </div>;
  }

  async function executeHeadlessCommand(
    command: typeof CONSOLE_COMMAND_NAMES[keyof typeof CONSOLE_COMMAND_NAMES],
    target: ConsoleWorkbenchTarget,
    params?: Record<string, unknown>,
  ): Promise<unknown> {
    return (await consoleController.commands.execute({
      command,
      target,
      params: scopedPanelParams(target.kind, panelRuntimeKey, params),
    })).result;
  }

  // =========================================================================
  // DATA MODEL — single canonical event log per identity
  //
  // The previous design split state across `serverHistoryRef`,
  // `liveOverlayRef`, `serverHasEventLogRef`, and `optimisticUserRef`.
  // Two adapter passes ran independently (one per side); their outputs
  // were concatenated at render time. Same logical event arriving via
  // RPC and SSE produced different keys depending on which path
  // normalized it, so cross-store dedup was unreliable, tool-call
  // grouping fragmented across the boundary, and refetches racing the
  // SSE handler caused entries to vanish or duplicate.
  //
  // We now keep a single sorted, deduped log per identity. SSE appends
  // into it. Server fetches reconcile into it (insert-by-key, no
  // wholesale replacement, no live-overlay wipe). The renderer makes
  // exactly one adapter pass over the merged log.
  const identityLogRef = React.useRef<Record<string, IdentityLog>>({});
  const timelineFetchInFlightRef = React.useRef<Record<string, Promise<void>>>(
    {},
  );
  const optimisticUserByPanelKeyRef = React.useRef<
    Record<string, OptimisticUserMessage>
  >({});

  function getOrCreateLog(identity: string): IdentityLog {
    let log = identityLogRef.current[identity];
    if (!log) {
      log = {
        events: [],
    version: 0,
    sorted: null,
    busyLifecycle: new ConsoleActivityProjection(),
    busyFoldedThroughMs: Number.NEGATIVE_INFINITY,
    busyFoldValid: true,
        byKey: new Map(),
        hasServerLog: null,
        olderHistoryExhausted: false,
        olderHistoryLoading: false,
      };
      identityLogRef.current[identity] = log;
    }
    return log;
  }

  function clearOptimisticUserByInteraction(interactionId: string): boolean {
    const clearedPanelKeys: string[] = [];
    for (const [panelKey, optimistic] of Object.entries(
      optimisticUserByPanelKeyRef.current,
    )) {
      if (optimistic.interactionId !== interactionId) continue;
      optimistic.objectUrls?.forEach((url) => URL.revokeObjectURL(url));
      delete optimisticUserByPanelKeyRef.current[panelKey];
      clearedPanelKeys.push(panelKey);
    }
    if (clearedPanelKeys.length > 0) {
      setSendingPanels((current) => {
        const next = new Set(current);
        for (const panelKey of clearedPanelKeys) next.delete(panelKey);
        return next;
      });
    }
    return clearedPanelKeys.length > 0;
  }

  function clearSendingPanelsForIdentity(identity: string): void {
    if (!identity.trim()) return;
    setSendingPanels((current) => {
      let changed = false;
      const next = new Set(current);
      const suffix = `:agent-chat:${identity}`;
      for (const panelKey of current) {
        if (panelKey.endsWith(suffix)) {
          next.delete(panelKey);
          changed = true;
        }
      }
      return changed ? next : current;
    });
  }

  function clearOptimisticUserByContent(
    identity: string,
    frame: ConsoleFrame,
  ): boolean {
    if (
      frame.event !== "interaction_started" &&
      frame.event !== "user_input" &&
      frame.event !== "run_started"
    )
      return false;
    const record =
      frame.data && typeof frame.data === "object"
        ? (frame.data as Record<string, unknown>)
        : {};
    const contentValue = frame.event === "run_started"
      ? record.prompt
      : record.content;
    const content =
      typeof contentValue === "string" ? contentValue.trim() : "";
    if (!content) return false;
    const clearedPanelKeys: string[] = [];
    for (const [panelKey, optimistic] of Object.entries(
      optimisticUserByPanelKeyRef.current,
    )) {
      if (!panelKey.endsWith(`:agent-chat:${identity}`)) continue;
      if (optimistic.interactionId) continue;
      if (
        !("text" in optimistic.entry) ||
        typeof optimistic.entry.text !== "string"
      )
        continue;
      if (optimistic.entry.text.trim() !== content) continue;
      optimistic.objectUrls?.forEach((url) => URL.revokeObjectURL(url));
      delete optimisticUserByPanelKeyRef.current[panelKey];
      clearedPanelKeys.push(panelKey);
    }
    if (clearedPanelKeys.length > 0) {
      setSendingPanels((current) => {
        const next = new Set(current);
        for (const panelKey of clearedPanelKeys) next.delete(panelKey);
        return next;
      });
    }
    return clearedPanelKeys.length > 0;
  }

  function clearOptimisticUserForFrame(identity: string, frame: ConsoleFrame): void {
    if (
      (frame.event === "interaction_started" ||
        frame.event === "user_input" ||
        frame.event === "run_started") &&
      frame.interactionId &&
      clearOptimisticUserByInteraction(frame.interactionId)
    ) {
      return;
    }
    clearOptimisticUserByContent(identity, frame);
  }

  /// Stable identity for a frame across RPC and SSE pipelines. Both
  /// produce `event_id`-shaped IDs (e.g. `evt-agent-019dde54-…`) for
  /// the same logical event, so `frame.id` is the primary key. The
  /// fallback only fires for synthetic frames without an id; including
  /// `interactionId` keeps interaction-bound events from colliding.
  function frameKey(frame: ConsoleFrame): string {
    return consoleFrameKey(frame);
  }

  /// Append one frame to the identity log, deduped by key. Appended
  /// frames are kept in insertion order in `events`; the transcript-order
  /// view in `sorted` is maintained incrementally. If the appended frame is an
  /// `interaction_started` whose interaction_id matches a pending
  /// optimistic user message, drop the optimistic — the server is now
  /// rendering the user turn itself.
  function appendFrame(identity: string, frame: ConsoleFrame): boolean {
    const log = getOrCreateLog(identity);
    if (
      frame.event === "frame_updated" &&
      frame.data &&
      typeof frame.data === "object"
    ) {
      const updated = (frame.data as Record<string, unknown>).frame as
        | ConsoleFrame
        | undefined;
      if (!updated || !updated.id) return false;
      const merged = mergeFrameUpdate(log, updated);
      if (!merged) return false;
      // History coverage also depends on exact ownership and cursor. A row
      // enriched or corrected in place must be checked against its boundary
      // again, even when its timestamp and busy transition did not change.
      if (
        merged.moved ||
        merged.previous.event !== merged.next.event ||
        merged.previous.sourceKind !== merged.next.sourceKind ||
        merged.previous.runtimeKey !== merged.next.runtimeKey ||
        merged.previous.identity !== merged.next.identity ||
        merged.previous.sessionId !== merged.next.sessionId ||
        merged.previous.interactionId !== merged.next.interactionId ||
        merged.previous.runId !== merged.next.runId ||
        merged.previous.status !== merged.next.status ||
        merged.previous.data !== merged.next.data ||
        merged.previous.cursor !== merged.next.cursor ||
        merged.previous.event === "assistant_history_snapshot" ||
        merged.next.event === "assistant_history_snapshot" ||
        busyTransitionForFrame(merged.previous) !== busyTransitionForFrame(merged.next)
      ) {
        log.busyFoldValid = false;
      }
      clearOptimisticUserForFrame(identity, updated);
      return true;
    }
    if (!pushFrame(log, frameKey(frame), frame)) return false;
    if (log.events.length > MAX_IDENTITY_LOG_EVENTS + IDENTITY_LOG_TRIM_SLACK) {
      trimIdentityLog(log);
    }
    clearOptimisticUserForFrame(identity, frame);
    return true;
  }

  /// Drop the oldest frames (transcript order) down to the ceiling and
  /// rebuild the key index. The retained frames' oldest cursor becomes the
  /// paging boundary so the dropped range is fetchable again as older
  /// history; the incremental busy fold is unaffected because it only
  /// depends on the newest lifecycle frames, which are always retained.
  function trimIdentityLog(log: IdentityLog): void {
    const retained = trimIdentityLogCore(log, MAX_IDENTITY_LOG_EVENTS, frameKey);
    if (!retained) return;
    let oldest: string | undefined;
    for (const frame of retained) oldest = olderCursor(oldest, frame.cursor);
    log.oldestTimelineCursor = oldest;
    log.olderHistoryExhausted = false;
    log.olderHistoryExhaustedAtCursor = undefined;
  }

  function busyTransitionForFrame(frame: ConsoleFrame): boolean | null {
    if (frame.event === "user_input") {
      return isTerminalUserInputStatus(frame.status) ? false : true;
    }
    if (
      frame.event === "interaction_started" ||
      frame.event === "run_started" ||
      frame.event === "reasoning_delta" ||
      frame.event === "reasoning_complete" ||
      frame.event === "tool_call_requested" ||
      frame.event === "tool_call" ||
      frame.event === "tool_execution_started" ||
      (frame.event === "server_tool_content" && isActiveServerToolContentFrame(frame)) ||
      (frame.event === "server_tool_content" && isTerminalServerToolContentFrame(frame)) ||
      frame.event === "tool_result_received" ||
      frame.event === "tool_execution_completed"
    ) {
      return true;
    }
    if (
      (frame.event === "turn_completed" && isTerminalTurnCompletedFrame(frame)) ||
      frame.event === "interaction_complete" ||
      frame.event === "interaction_failed" ||
      frame.event === "run_completed" ||
      frame.event === "run_failed" ||
      (frame.event === "system_notice" && systemNoticeClearsBusyState(frame)) ||
      frame.event === "message_delivery_failed"
    ) {
      // Queue draining follows run-level terminals so server-side
      // interaction correlation advances before the next send leaves.
      return false;
    }
    return null;
  }

  function isTerminalUserInputStatus(status?: string): boolean {
    return status === "completed" || status === "delivery_failed" || status === "failed";
  }

  function busyTransitionSortRank(frame: ConsoleFrame): number {
    const transition = busyTransitionForFrame(frame);
    // When session-history projection gives lifecycle frames the same
    // timestamp, a terminal event must win over its matching start/user
    // frame. Otherwise backfilled history can leave an idle agent marked
    // busy forever and trap future sends in the pending stack.
    return transition === false ? 1 : 0;
  }

  function applyBusyState(identity: string, nextBusy: boolean): void {
    const wasBusy = identityBusyRef.current[identity] === true;
    identityBusyRef.current[identity] = nextBusy;
    if (wasBusy && !nextBusy) {
      clearSendingPanelsForIdentity(identity);
      maybeDrainHead(identity);
    }
  }

  /// Live path: fold the frame in place when it is not older than the newest
  /// lifecycle frame already folded. Frames arrive in order almost always,
  /// so this is O(1) per frame; a late frame (older timestamp) invalidates
  /// the fold and the next read replays the log once.
  function updateBusyStateForFrame(
    identity: string,
    frame: ConsoleFrame,
  ): void {
    const log = getOrCreateLog(identity);
    if (frame.event === "assistant_history_snapshot"
      || (frame.event === "frame_updated" && !log.busyFoldValid)) {
      recomputeBusyStateFromLog(identity);
      return;
    }
    if (!isAfterIdleWatermark(frame, log.idleSettledThrough)) return;
    const ts = frame.timestampMs ?? log.busyFoldedThroughMs;
    if (!log.busyFoldValid || ts < log.busyFoldedThroughMs || frame.sourceKind === "session_history") {
      recomputeBusyStateFromLog(identity);
      return;
    }
    if (!log.busyLifecycle.fold(frame)) return;
    log.busyFoldedThroughMs = ts;
    identityLifecycleRef.current[identity] = {
      interactionOpen: log.busyLifecycle.interactionOpen,
      runOpen: log.busyLifecycle.runOpen,
    };
    applyBusyState(identity, log.busyLifecycle.busy);
  }

  /// Full replay over the transcript-ordered lifecycle frames. Used for the
  /// initial fold, after in-place frame updates, and after a late frame;
  /// the ordinary live path never pays for it.
  function recomputeBusyStateFromLog(identity: string): void {
    const log = getOrCreateLog(identity);
    const lifecycle = new ConsoleActivityProjection();
    let foldedThrough = log.idleSettledThrough?.timestampMs ?? Number.NEGATIVE_INFINITY;
    const ordered = framesAfterIdleWatermark(sortedEvents(log), log.idleSettledThrough)
      .sort((a, b) => {
        const timeDelta = (a.timestampMs || 0) - (b.timestampMs || 0);
        if (timeDelta !== 0) return timeDelta;
        const rankDelta = busyTransitionSortRank(a) - busyTransitionSortRank(b);
        if (rankDelta !== 0) return rankDelta;
        return (a.cursor || a.id || "").localeCompare(b.cursor || b.id || "");
      });
    for (const frame of ordered) {
      lifecycle.fold(frame);
      if (typeof frame.timestampMs === "number" && frame.timestampMs > foldedThrough) {
        foldedThrough = frame.timestampMs;
      }
    }
    log.busyLifecycle = lifecycle;
    log.busyFoldedThroughMs = foldedThrough;
    log.busyFoldValid = true;
    identityLifecycleRef.current[identity] = {
      interactionOpen: lifecycle.interactionOpen,
      runOpen: lifecycle.runOpen,
    };
    applyBusyState(identity, lifecycle.busy);
  }

  /// Reconcile a server-history fetch into the identity log. Frames
  /// already present (by key) are skipped; new frames are appended.
  /// The live overlay is preserved — both sides feed the same log now.
  ///
  /// `available: false` means the runtime has no `EventLogStore`
  /// configured — the response is the in-memory recent buffer rather
  /// than authoritative replay. We still ingest those frames (they're
  /// the only backfill we'll ever get for this identity) and just
  /// remember that a refetch wouldn't get anything more, so SSE is the
  /// going-forward source of truth.
  function reconcileServerLog(
    identity: string,
    frames: ConsoleFrame[],
    available: boolean,
  ): boolean {
    const log = getOrCreateLog(identity);
    log.hasServerLog = available;
    let changed = false;
    let appended = false;
    for (const frame of frames) {
      if (!appendFrame(identity, frame)) continue;
      changed = true;
      appended = true;
    }
    // A backfill page can carry frames older than the live fold, so it
    // invalidates the incremental busy fold and replays once; a page that
    // added nothing leaves the fold alone.
    if (appended || !log.busyFoldValid) {
      log.busyFoldValid = false;
      recomputeBusyStateFromLog(identity);
    }
    if (recomputePhaseForIdentity(identity)) changed = true;
    return changed;
  }

  function newerCursor(a: string | undefined, b: string | undefined): string | undefined {
    const aSeq = cursorSeq(a);
    const bSeq = cursorSeq(b);
    if (aSeq === null) return b || a;
    if (bSeq === null) return a || b;
    return bSeq > aSeq ? b : a;
  }

  function olderCursor(a: string | undefined, b: string | undefined): string | undefined {
    const aSeq = cursorSeq(a);
    const bSeq = cursorSeq(b);
    if (aSeq === null) return b || a;
    if (bSeq === null) return a || b;
    return bSeq < aSeq ? b : a;
  }

  function noteIdentityTimelinePage(
    identity: string,
    page: ConsoleTimelinePage,
    target: { mode: "recent" | "since"; before?: string },
  ): boolean {
    const log = getOrCreateLog(identity);
    const previousOldest = log.oldestTimelineCursor;
    const previousLatest = log.latestTimelineCursor;
    const previousExhausted = log.olderHistoryExhausted;
    const previousExhaustedAtCursor = log.olderHistoryExhaustedAtCursor;
    for (const frame of page.frames) {
      log.oldestTimelineCursor = olderCursor(log.oldestTimelineCursor, frame.cursor);
      log.latestTimelineCursor = newerCursor(log.latestTimelineCursor, frame.cursor);
    }
    if (target.mode === "recent") {
      log.latestTimelineCursor = newerCursor(log.latestTimelineCursor, page.latestCursor);
      if (target.before) {
        log.olderHistoryExhausted = page.exhausted === true;
        log.olderHistoryExhaustedAtCursor =
          page.exhausted === true ? log.oldestTimelineCursor : undefined;
      } else if (!log.olderHistoryExhaustedAtCursor) {
        log.olderHistoryExhausted = page.exhausted === true;
      }
    } else {
      log.latestTimelineCursor = newerCursor(
        log.latestTimelineCursor,
        page.nextCursor || page.latestCursor,
      );
    }
    return (
      previousOldest !== log.oldestTimelineCursor ||
      previousLatest !== log.latestTimelineCursor ||
      previousExhausted !== log.olderHistoryExhausted ||
      previousExhaustedAtCursor !== log.olderHistoryExhaustedAtCursor
    );
  }

  function resetIdentityTimelineReplayMetadata(identity: string): boolean {
    const log = getOrCreateLog(identity);
    const changed =
      log.events.length > 0 ||
      log.byKey.size > 0 ||
      log.oldestTimelineCursor !== undefined ||
      log.latestTimelineCursor !== undefined ||
      log.olderHistoryExhausted !== false ||
      log.olderHistoryExhaustedAtCursor !== undefined;
    resetIdentityLogCore(log);
    log.busyFoldValid = false;
    log.oldestTimelineCursor = undefined;
    log.latestTimelineCursor = undefined;
    log.olderHistoryExhausted = false;
    log.olderHistoryExhaustedAtCursor = undefined;
    return changed;
  }

  async function queryIdentityTimelinePage(
    identity: string,
    target: { mode: "recent" | "since"; after?: string; before?: string; limit?: number },
  ): Promise<{ page: ConsoleTimelinePage; metadataChanged: boolean }> {
    const pageFact = await consoleController.timeline.query(
      {
        identity,
        mode: target.mode,
        after: target.after,
        before: target.before,
        limit: target.limit ?? 200,
      },
    );
    const page = pageFact.value;
    const metadataChanged = noteIdentityTimelinePage(identity, page, target);
    return { page, metadataChanged };
  }

  function refreshIdentityTimelineNow(
    identity: string,
    options: { clearPhase?: boolean } = {},
  ): Promise<void> {
    const normalized = identity.trim();
    if (!normalized) return Promise.resolve();
    const inFlight = timelineFetchInFlightRef.current[normalized];
    if (inFlight) {
      return inFlight.then(() => {
        if (options.clearPhase) {
          recomputePhaseForIdentity(normalized);
          forceRender();
        }
      });
    }

    setLoadingHistory((current) =>
      current[normalized] ? current : { ...current, [normalized]: true },
    );
    const request = (async () => {
      const { page } = await queryIdentityTimelinePage(normalized, {
        mode: "recent",
        limit: 200,
      });
      reconcileServerLog(normalized, page.frames, page.available);
      if (options.clearPhase) recomputePhaseForIdentity(normalized);
      forceRender();
    })().finally(() => {
      delete timelineFetchInFlightRef.current[normalized];
      setLoadingHistory((current) => {
        if (!current[normalized]) return current;
        const next = { ...current };
        delete next[normalized];
        return next;
      });
    });
    timelineFetchInFlightRef.current[normalized] = request;
    return request;
  }

  async function loadOlderIdentityTimeline(identity: string): Promise<void> {
    const normalized = identity.trim();
    if (!normalized) return;
    const log = getOrCreateLog(normalized);
    if (log.olderHistoryLoading || log.olderHistoryExhausted) return;
    log.olderHistoryLoading = true;
    forceRender();
    try {
      const { page } = await queryIdentityTimelinePage(normalized, {
        mode: "recent",
        before: log.oldestTimelineCursor,
        limit: 200,
      });
      reconcileServerLog(normalized, page.frames, page.available);
      // Older frames just folded in may be the first page to contain a
      // workgraph card for this identity; re-run the (idempotent) hydration
      // check so that case doesn't stay permanently un-hydrated.
      void hydrateWorkGraphCardsForIdentity(normalized);
    } catch {
      // The current view remains usable; a later scroll can retry.
    } finally {
      log.olderHistoryLoading = false;
      forceRender();
    }
  }

  /// Render-time chat view: transcript time is the primary order. Aggregate
  /// cursor order is useful for replay paging, but it can arrive out of
  /// conversational order when delayed peer-message/session-history frames are
  /// backfilled after newer tool or completion frames. Use cursor only as a
  /// stable tie-breaker for same-timestamp frames.
  function getSortedFrames(identity: string): ConsoleFrame[] {
    const log = identityLogRef.current[identity];
    if (!log) return [];
    return sortedEvents(log);
  }

  const derivedTranscriptRef = React.useRef<
    Record<
      string,
      {
        version: number;
        agent: ConsoleAgent | null;
        sortedFrames: ConsoleFrame[];
        conversationEntries: ConversationTimelineEntry[];
        derivation: TimelineDerivation;
        agentKey: string;
        hasOlderHistory: boolean;
      }
    >
  >({});
  function derivedTranscriptFor(
    identity: string,
    panelId: string,
    agent: ConsoleAgent | null,
  ): { sortedFrames: ConsoleFrame[]; conversationEntries: ConversationTimelineEntry[] } {
    const log = getOrCreateLog(identity);
    const cached = derivedTranscriptRef.current[identity];
    // Roster refreshes replace agent objects without changing them; the
    // derivation reads only the agent's plain fields, so key on its content
    // and keep the object the cached derivation was built with.
    const agentKey = agent ? JSON.stringify(agent) : "";
    if (cached && cached.agentKey === agentKey) agent = cached.agent;
    // Paging metadata can change without a frame (an empty last page).
    const hasOlderHistory = identityHasOlderHistory(log);
    if (cached && cached.version === log.version && cached.agent === agent
      && cached.hasOlderHistory === hasOlderHistory) {
      return cached;
    }
    const sortedFrames = framesVisibleInPanel(getSortedFrames(identity), panelId);
    // Continues the previous derivation when only streamed text arrived, so a
    // token costs O(token) instead of a pass over the whole identity log.
    const derivation = deriveTimelineEntries(agent, sortedFrames, {
      renderInteractionStartsAsUser: true,
      renderTextDeltas: true,
      blobBaseUrl: baseUrl,
      hasOlderHistory,
    }, cached?.derivation);
    const conversationEntries = derivation.entries;
    const next = { version: log.version, agent, sortedFrames, conversationEntries, derivation, agentKey, hasOlderHistory };
    derivedTranscriptRef.current[identity] = next;
    return next;
  }

  // Both scan the whole identity log; recompute only when it changed (or,
  // for the phase, when its local or server inputs did), not every render.
  const panelPhaseRef = React.useRef<Record<string, {
    frames: ConsoleFrame[];
    serverPhase: unknown;
    phase: ReturnType<typeof resolvePanelResponsePhase>;
  }>>({});
  function panelPhaseFor(
    panelKey: string,
    sortedFrames: ConsoleFrame[],
    inputs: Omit<Parameters<typeof resolvePanelResponsePhase>[0], "frames">,
  ): ReturnType<typeof resolvePanelResponsePhase> {
    // A local phase wins outright (resolvePanelResponsePhase's first branch).
    if (inputs.hasLocalPhase) return inputs.localPhase ?? null;
    const cached = panelPhaseRef.current[panelKey];
    if (cached && cached.serverPhase === inputs.serverPhase && sortedFrames.length >= cached.frames.length) {
      let same = true;
      for (let i = 0; i < cached.frames.length && same; i++) same = sortedFrames[i] === cached.frames[i];
      if (same) {
        let phase = cached.phase;
        let extendable = true;
        for (let i = cached.frames.length; i < sortedFrames.length && extendable; i++) {
          const frame = sortedFrames[i];
          // Live text only: each one sets "generating", and none changes
          // which earlier frames settled history covers.
          extendable = frame.event === "text_delta" && frame.sourceKind !== "session_history";
          if (extendable) phase = "generating";
        }
        if (extendable) {
          if (sortedFrames.length !== cached.frames.length) {
            panelPhaseRef.current[panelKey] = { frames: sortedFrames.slice(), serverPhase: inputs.serverPhase, phase };
          }
          return phase;
        }
      }
    }
    const phase = resolvePanelResponsePhase({
      frames: sortedFrames.filter((frame) => PANEL_ROUTABLE_EVENTS.has(frame.event)),
      ...inputs,
    });
    panelPhaseRef.current[panelKey] = { frames: sortedFrames.slice(), serverPhase: inputs.serverPhase, phase };
    return phase;
  }
  const activeRunIdRef = React.useRef<Record<string, { version: number; runId: string | null }>>({});
  function activeRunIdFor(identity: string, version: number, sortedFrames: ConsoleFrame[]): string | null {
    const cached = activeRunIdRef.current[identity];
    if (cached && cached.version === version) return cached.runId;
    const runId = activeRunIdFromFrames(sortedFrames);
    activeRunIdRef.current[identity] = { version, runId };
    return runId;
  }

  function framesVisibleInPanel(
    frames: ConsoleFrame[],
    panelId: string,
  ): ConsoleFrame[] {
    void panelId;
    // Panel ids are ephemeral UI instance ids. Persisted user_input frames
    // keep the original `console:<panel-id>` origin, so filtering by the
    // current panel id hides the operator's historical prompts after a
    // refresh or reopen. Identity-scoped logs are already routed before
    // this point, so every frame in the identity log belongs in the pane.
    return frames;
  }

  // Activity rail (global, unchanged)
  const activityRef = React.useRef<ConsoleFrame[]>([]);
  // Unfiltered recent-frames ring for topology-class panels that need to
  // see tool calls (peer-comms send_*, etc.) in addition to interaction
  // lifecycle. The activity rail filters tool events out; this buffer
  // doesn't.
  function commitLiveFrames(frames: ConsoleFrame[], cause: RenderCause = "frame"): void {
    liveFramesRef.current = frames;
    // Published with the next scheduled render instead of per call, so a
    // frame burst commits the topology buffer once.
    liveFramesDirtyRef.current = true;
    forceRender(cause);
  }

  // ──────────────────────────────────────────────────────────────
  // Queue entries are immutable attempts once dispatched. Persistence requires
  // an opaque authenticated host scope; unknown outcomes never auto-replay.
  const sendControllerRef = React.useRef(consoleController);
  sendControllerRef.current = consoleController;
  const transientSendScope = React.useMemo(() => `transient:${createIdempotencyKey()}`, [consoleController]);
  const sendScope = persistentSendScope || `${transientSendScope}:${baseUrl}`;
  const [composerTabId] = React.useState(() => {
    try { return consoleComposerTabId(window.sessionStorage, createIdempotencyKey); }
    catch { return createIdempotencyKey(); }
  });
  const composerIdFor = (panelKey: string) => JSON.stringify([composerTabId, panelKey]);
  const sendScopeRef = React.useRef(sendScope);
  const persistentSendScopeRef = React.useRef(persistentSendScope);
  const pendingStackRef = React.useRef<Record<string, PendingItem[]>>({});
  const autoDrainRequestedRef = React.useRef(new Map<string, { inFlight: boolean; token: string }>());
  const sendRetryEpochRef = React.useRef(0);
  const pendingStorageErrorRef = React.useRef<Record<string, string>>({});
  const queueStorageBannerRef = React.useRef<{ scope: string; identity: string; message: string } | null>(null);
  const [contextDrafts, setContextDrafts] = React.useState<Record<string, ConsoleContextRecord[]>>({});
  const [submittedFrames, setSubmittedFrames] = React.useState<Record<string, string>>({});
  const loadedComposerDraftsRef = React.useRef<Record<string, { text: string; contexts: ConsoleContextRecord[] }>>({});
  function storedComposerDraft(identity: string, panelKey: string) {
    const namespace = persistentSendScopeRef.current;
    const key = `${sendScopeRef.current}:${composerIdFor(panelKey)}`;
    if (!loadedComposerDraftsRef.current[key]) {
      try {
        const storage = browserComposerStorage();
        loadedComposerDraftsRef.current[key] = namespace && storage
          ? loadConsoleComposerDraft(storage, namespace, identity, composerIdFor(panelKey)) : { text: "", contexts: [] };
      } catch (error) {
        pendingStorageErrorRef.current[identity] = errorMessage(error);
        return { text: "", contexts: [] };
      }
    }
    return loadedComposerDraftsRef.current[key];
  }
  function persistComposerDraft(identity: string, panelKey: string, text: string, contexts: ConsoleContextRecord[]): boolean {
    // The pane flushes its debounced draft while unmounting. Its captured
    // namespace/composer still owns this write even after outbound work stops.
    const namespace = persistentSendScopeRef.current;
    try {
      validateConsoleContexts(contexts);
      if (namespace) {
        const storage = browserComposerStorage();
        if (!storage) throw new Error("Draft storage is unavailable.");
        saveConsoleComposerDraft(storage, namespace, identity, { text, contexts }, composerIdFor(panelKey));
      }
      loadedComposerDraftsRef.current[`${sendScopeRef.current}:${composerIdFor(panelKey)}`] = { text, contexts };
      return true;
    } catch (error) {
      setActionError(`Draft remains visible but was not saved: ${errorMessage(error)}`);
      return false;
    }
  }
  if (sendScopeRef.current !== sendScope) {
    // Render uses scope-prefixed draft keys, so prior-principal text is never exposed.
    sendScopeRef.current = sendScope;
    pendingStackRef.current = {};
    autoDrainRequestedRef.current.clear();
    pendingStorageErrorRef.current = {};
    queueStorageBannerRef.current = null;
    loadedComposerDraftsRef.current = {};
    for (const optimistic of Object.values(optimisticUserByPanelKeyRef.current)) {
      optimistic.objectUrls?.forEach((url) => URL.revokeObjectURL(url));
    }
    optimisticUserByPanelKeyRef.current = {};
  }
  React.useEffect(() => {
    setDraftByKey({});
    setContextDrafts({});
    setSubmittedFrames({});
    setSendingPanels(new Set());
    setPendingChecks({});
    setDeliveredNotices({});
  }, [sendScope]);
  persistentSendScopeRef.current = persistentSendScope;
  const scopedDraftKey = (panelKey: string) => `${sendScopeRef.current}:${panelKey}`;

  function loadPendingStack(identity: string): PendingItem[] {
    const namespace = persistentSendScopeRef.current;
    if (!namespace) return pendingStackRef.current[identity] ?? [];
    const storage = browserLocalStorage();
    if (!storage) {
      pendingStorageErrorRef.current[identity] = "Queue storage is unavailable. Your message has not been sent.";
      return pendingStackRef.current[identity] ?? [];
    }
    const loaded = loadConsoleSendAttempts(storage, namespace, identity);
    if (loaded.kind === "blocked") {
      pendingStorageErrorRef.current[identity] = loaded.reason;
      return pendingStackRef.current[identity] ?? [];
    }
    delete pendingStorageErrorRef.current[identity];
    return loaded.attempts;
  }

  function getPendingStack(identity: string): PendingItem[] {
    return pendingStackRef.current[identity] ??= loadPendingStack(identity);
  }

  function commitPendingStack(identity: string, update: (prev: PendingItem[]) => PendingItem[], legacyImported = false): boolean {
    if (!lifetimeRef.current.active) return false;
    const previous = getPendingStack(identity);
    let next = update(previous);
    const namespace = persistentSendScopeRef.current;
    if (namespace) {
      try {
        const storage = browserLocalStorage();
        if (!storage) throw new Error("Queue storage is unavailable.");
        const clean = (items: PendingItem[]) => items.map(({ status: _status, editing: _editing, expanded: _expanded, ...attempt }) => attempt);
        const saved = saveConsoleSendAttempts(storage, namespace, identity, clean(next), clean(previous), legacyImported);
        next = saved.map((attempt) => ({ ...next.find((item) => item.id === attempt.id), ...attempt }));
        delete pendingStorageErrorRef.current[identity];
        const banner = queueStorageBannerRef.current;
        if (banner?.scope === sendScopeRef.current && banner.identity === identity) {
          setActionError((current) => current === banner.message ? "" : current);
          queueStorageBannerRef.current = null;
        }
      } catch (error) {
        // The composer or prior queue stays visible until persistence succeeds.
        pendingStorageErrorRef.current[identity] = errorMessage(error);
        const message = `Queue could not be saved: ${errorMessage(error)}`;
        queueStorageBannerRef.current = { scope: sendScopeRef.current, identity, message };
        setActionError(message);
        forceRender();
        return false;
      }
    }
    pendingStackRef.current[identity] = next;
    forceRender();
    return true;
  }

  async function setPendingStack(identity: string, update: (prev: PendingItem[]) => PendingItem[], legacyImported = false): Promise<boolean> {
    const generation = lifetimeRef.current.generation;
    if (!lifetimeRef.current.active) return false;
    const scope = sendScopeRef.current;
    const controller = sendControllerRef.current;
    const namespace = persistentSendScopeRef.current;
    if (!namespace) return commitPendingStack(identity, update, legacyImported);
    return withConsoleSendStorageLock(consoleSendStorageKey(namespace, identity), () => {
      if (!lifetimeRef.current.active || generation !== lifetimeRef.current.generation || scope !== sendScopeRef.current || controller !== sendControllerRef.current) return false;
      // Every durable writer shares this lock, including enqueue/edit/discard.
      const visible = getPendingStack(identity);
      pendingStackRef.current[identity] = loadPendingStack(identity).map((attempt) => ({
        ...visible.find((item) => item.id === attempt.id), ...attempt,
      }));
      return commitPendingStack(identity, update, legacyImported);
    }).catch((error) => { setActionError(errorMessage(error)); return false; });
  }

  React.useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      const namespace = persistentSendScopeRef.current;
      if (!namespace) return;
      for (const identity of Object.keys(pendingStackRef.current)) {
        if (event.key !== null && event.key !== consoleSendStorageKey(namespace, identity)) continue;
        pendingStackRef.current[identity] = loadPendingStack(identity);
      }
      sendRetryEpochRef.current += 1;
      forceRender();
    };
    const onRetryOpportunity = () => { sendRetryEpochRef.current += 1; forceRender(); };
    window.addEventListener("storage", onStorage);
    window.addEventListener("focus", onRetryOpportunity);
    window.addEventListener("online", onRetryOpportunity);
    return () => {
      window.removeEventListener("storage", onStorage);
      window.removeEventListener("focus", onRetryOpportunity);
      window.removeEventListener("online", onRetryOpportunity);
    };
  }, []);

  React.useEffect(() => {
    const timer = window.setInterval(() => {
      let changed = false;
      for (const [identity, items] of Object.entries(pendingStackRef.current)) {
        const next = items.map((item) => recoverConsoleSendAttempt(item, Date.now()));
        if (next.some((item, index) => item !== items[index])) {
          pendingStackRef.current[identity] = next;
          changed = true;
        }
      }
      if (changed) forceRender();
    }, 15_000);
    return () => window.clearInterval(timer);
  }, []);

  // Per-identity busy state — driven by interaction lifecycle events on
  // the SSE stream. Used both for the stack's "agent busy" indicator
  // and to decide whether a fresh Send should bypass the stack
  // (idle + empty stack) or push to it (anything else).
  const identityBusyRef = React.useRef<Record<string, boolean>>({});
  const identityLifecycleRef = React.useRef<
    Record<string, { interactionOpen: boolean; runOpen: boolean }>
  >({});
  const isIdentityBusy = (identity: string) =>
    identityBusyRef.current[identity] === true;

  // Phase tracking (per-panel, unchanged)
  const phaseRef = React.useRef<
    Record<string, "waiting" | "tool-executing" | "generating" | null>
  >({});
  const phaseValueByKey = React.useRef<
    Record<string, "waiting" | "tool-executing" | "generating" | null>
  >({});
  const phaseSinceByKey = React.useRef<Record<string, number>>({});
  const phaseTimerByKey = React.useRef<Record<string, number>>({});

  // Per-identity refresh debounce timers
  const refreshTimersRef = React.useRef<Record<string, number>>({});

  // Experience refresh debounce
  const experienceTimerRef = React.useRef<number | null>(null);
  const experienceLoadInFlightRef = React.useRef<Promise<ConsoleAgent[]> | null>(
    null,
  );
  const experienceLoadGenerationRef = React.useRef(-1);
  // Stable agent ref for async callbacks
  const agentsRef = React.useRef<ConsoleAgent[]>([]);
  const identityAliasesRef = React.useRef<ConsoleIdentityAliasMap>(new Map());
  React.useEffect(() => {
    agentsRef.current = agents;
    identityAliasesRef.current = buildConsoleIdentityAliasMap(agents);
  }, [agents]);

  const initialTargetOpened = React.useRef(false);
  const dockLayoutHydrated = React.useRef(false);
  const dockLayoutRestored = React.useRef(false);
  const dockLayoutRestoring = React.useRef(false);

  // =========================================================================
  // DOCK CONTROLLER
  // =========================================================================

  const dock = useConsoleDockController<MetaDockTarget>({
    createPanelState: ({ target }) => ({
      id: createConsoleId("panel"),
      target: target || null,
      mode: "console" as const,
    }),
  });
  const currentDockLayoutStorageKey = React.useMemo(
    () => dockLayoutStorageKey(sendScope),
    [sendScope],
  );

  // A layout effect, so the saved layout lands in a synchronous follow-up
  // commit before paint, before any input can be handled. As a passive effect
  // it ran a task later, and a click in between (opening a nav panel) was
  // overwritten by the restore.
  React.useLayoutEffect(() => {
    if (!experience || dockLayoutHydrated.current) return;
    dockLayoutHydrated.current = true;
    try {
      const raw = localStorage.getItem(currentDockLayoutStorageKey);
      if (!raw) return;
      const parsed = JSON.parse(raw) as ConsoleDockState<MetaDockTarget>;
      const restored = normalizeConsoleDockState(parsed);
      if (restored.tabs.length === 0 || restored.panels.length === 0) return;
      // Saved labels and targets are preferences, not current authority. A
      // different principal on the same runtime must not inherit a hidden
      // identity (or its draft) merely because the browser saved that pane.
      restored.panels = restored.panels.map(panel => {
        const target = panel.target;
        if (!target || (target.kind !== "agent-chat" && target.kind !== "identity-inspect")) return panel;
        const identity = target.identity || target.memberId;
        const agent = agents.find(agent => [agent.identity, agent.member_id, agent.agent_id].includes(identity));
        return { ...panel, target: agent ? (target.kind === "agent-chat" ? buildDockTarget(agent) : buildInspectTarget(agent)) : null };
      });
      dockLayoutRestored.current = true;
      dockLayoutRestoring.current = true;
      dock.setState(restored);
    } catch {
      /* ignore corrupt saved layout */
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentDockLayoutStorageKey, experience]);

  React.useEffect(() => {
    if (!experience || !dockLayoutHydrated.current) return;
    if (dockLayoutRestoring.current) {
      dockLayoutRestoring.current = false;
      return;
    }
    try {
      localStorage.setItem(
        currentDockLayoutStorageKey,
        JSON.stringify(dock.state),
      );
    } catch {
      /* ignore storage failures */
    }
  }, [currentDockLayoutStorageKey, dock.state, experience]);

  // =========================================================================
  // PHASE TRACKING (unchanged logic)
  // =========================================================================

  function clearPhaseTimer(panelKey: string) {
    const timer = phaseTimerByKey.current[panelKey];
    if (timer !== undefined) {
      window.clearTimeout(timer);
      delete phaseTimerByKey.current[panelKey];
    }
  }

  function commitPanelPhase(
    panelKey: string,
    phase: "waiting" | "tool-executing" | "generating" | null,
  ): boolean {
    const previous = phaseValueByKey.current[panelKey] ?? null;
    clearPhaseTimer(panelKey);
    phaseValueByKey.current[panelKey] = phase;
    phaseSinceByKey.current[panelKey] = Date.now();
    phaseRef.current[panelKey] = phase;
    return previous !== phase;
  }

  function schedulePanelPhase(
    panelKey: string,
    phase: "waiting" | "tool-executing" | "generating" | null,
    delayMs: number,
  ) {
    clearPhaseTimer(panelKey);
    phaseTimerByKey.current[panelKey] = window.setTimeout(() => {
      delete phaseTimerByKey.current[panelKey];
      phaseValueByKey.current[panelKey] = phase;
      phaseSinceByKey.current[panelKey] = Date.now();
      phaseRef.current[panelKey] = phase;
      forceRender();
    }, delayMs);
  }

  function updatePanelPhaseFromFrame(
    panelKey: string,
    frame: ConsoleFrame,
    projectedPhase: "waiting" | "tool-executing" | "generating" | null = null,
  ): boolean {
    const currentPhase = phaseValueByKey.current[panelKey] ?? null;
    const elapsedMs = Date.now() - (phaseSinceByKey.current[panelKey] ?? 0);
    switch (frame.event) {
      case "user_input":
        if (isTerminalUserInputStatus(frame.status)) return commitPanelPhase(panelKey, projectedPhase);
        return commitPanelPhase(panelKey, "waiting");
      case "interaction_started":
      case "run_started":
        return commitPanelPhase(panelKey, "waiting");
      case "tool_call_requested":
      case "tool_call":
      case "tool_execution_started":
      case "server_tool_content":
        if (frame.event === "server_tool_content") {
          if (isTerminalServerToolContentFrame(frame)) {
            return commitPanelPhase(panelKey, "waiting");
          }
          if (!isActiveServerToolContentFrame(frame)) {
            return false;
          }
        }
        if (currentPhase === "waiting" && elapsedMs < 300) {
          schedulePanelPhase(panelKey, "tool-executing", 300 - elapsedMs);
          return true;
        }
        return commitPanelPhase(panelKey, "tool-executing");
      case "tool_result_received":
      case "tool_execution_completed":
        // A completed tool means this specific operation is done, but the
        // agent turn is still active until a terminal text/run frame arrives.
        // Keep the pane visibly busy so mid-turn sends queue instead of
        // slipping into the runtime as a live boundary input.
        return commitPanelPhase(panelKey, "waiting");
      case "reasoning_delta":
        return commitPanelPhase(panelKey, "generating");
      case "reasoning_complete":
        return commitPanelPhase(panelKey, "waiting");
      case "text_delta": {
        if (currentPhase === "tool-executing") {
          const r = Math.max(0, 300 - elapsedMs);
          if (r > 0) {
            schedulePanelPhase(panelKey, "generating", r);
            return true;
          }
        }
        if (currentPhase === "waiting" && elapsedMs < 300) {
          schedulePanelPhase(panelKey, "generating", 300 - elapsedMs);
          return true;
        }
        return commitPanelPhase(panelKey, "generating");
      }
      case "text_complete":
        return commitPanelPhase(panelKey, projectedPhase);
      case "interaction_complete":
      case "interaction_failed":
        return commitPanelPhase(panelKey, projectedPhase);
      case "run_completed":
      case "run_failed":
        return commitPanelPhase(panelKey, projectedPhase);
      case "system_notice":
        if (systemNoticeClearsBusyState(frame)) return commitPanelPhase(panelKey, projectedPhase);
        return false;
      case "turn_completed":
        if (isTerminalTurnCompletedFrame(frame)) {
          return commitPanelPhase(panelKey, projectedPhase);
        }
        return false;
      case "message_delivery_failed":
        return commitPanelPhase(panelKey, projectedPhase);
      default:
        return false;
    }
  }

  // The SSE handler runs from inside the stream effect, so its
  // closure captures `dock` from the first render — when panels[] was empty.
  // Route panel-iterating phase updates through a ref so they always see the
  // current panel set; otherwise interaction_started/text_delta/
  // interaction_complete arrive at panel:none and the typing indicator
  // sticks at "waiting" indefinitely (and the "still busy" perception breaks
  // the pending-stack auto-queue, which depends on `identityBusyRef`).
  const dockRef = React.useRef(dock);
  dockRef.current = dock;

  // Helper: update phase for ALL panels showing a given identity
  function updatePhaseForIdentity(identity: string, frame: ConsoleFrame): boolean {
    // Replayed history can arrive after live events. Reconcile the retained
    // lifecycle evidence without animating saved auxiliary rows as new work.
    // In-place updates may change the same scope.
    if (frame.sourceKind === "session_history" || frame.event === "assistant_history_snapshot"
      || frame.event === "frame_updated") {
      return recomputePhaseForIdentity(identity);
    }
    if (!isAfterIdleWatermark(frame, identityLogRef.current[identity]?.idleSettledThrough)) return false;
    let changed = false;
    for (const panel of dockRef.current.viewState.panels) {
      const target = panel.target as MetaDockTarget | null;
      if (!target || target.kind !== "agent-chat") continue;
      if ((target.identity || target.memberId) !== identity) continue;
      if (updatePanelPhaseFromFrame(
        buildPanelConversationKey(panel.id, target),
        frame,
        getOrCreateLog(identity).busyLifecycle.phase,
      )) changed = true;
    }
    return changed;
  }

  // Helper: clear phase for all panels showing a given identity
  function clearPhaseForIdentity(identity: string): boolean {
    let changed = false;
    for (const panel of dockRef.current.viewState.panels) {
      const target = panel.target as MetaDockTarget | null;
      if (!target || target.kind !== "agent-chat") continue;
      if ((target.identity || target.memberId) !== identity) continue;
      if (commitPanelPhase(buildPanelConversationKey(panel.id, target), null)) {
        changed = true;
      }
    }
    return changed;
  }

  function commitPhaseForIdentity(
    identity: string,
    phase: "waiting" | "tool-executing" | "generating" | null,
  ): boolean {
    let changed = false;
    for (const panel of dockRef.current.viewState.panels) {
      const target = panel.target as MetaDockTarget | null;
      if (!target || target.kind !== "agent-chat") continue;
      if ((target.identity || target.memberId) !== identity) continue;
      if (commitPanelPhase(buildPanelConversationKey(panel.id, target), phase)) {
        changed = true;
      }
    }
    return changed;
  }

  function recomputePhaseForIdentity(identity: string): boolean {
    const frames = framesAfterIdleWatermark(
      getSortedFrames(identity),
      identityLogRef.current[identity]?.idleSettledThrough,
    ).filter((frame) => PANEL_ROUTABLE_EVENTS.has(frame.event));
    const phase = inferResponsePhaseFromFrames(frames, null);
    let changed = false;
    for (const panel of dockRef.current.viewState.panels) {
      const target = panel.target as MetaDockTarget | null;
      if (!target || target.kind !== "agent-chat") continue;
      if ((target.identity || target.memberId) !== identity) continue;
      if (commitPanelPhase(buildPanelConversationKey(panel.id, target), phase)) {
        changed = true;
      }
    }
    return changed;
  }

  // The member status poll is the server's authoritative idle signal (see
  // lib/idle-reconcile.ts). A fold that still holds an owner open after the
  // member finished (frames that opened it never got a matching terminal)
  // reads "working" and queues every send; two idle polls in a row, at
  // least IDLE_SETTLE_MIN_GAP_MS apart and with no frame for the member
  // since the first, settle every frame the console had seen.
  const settleIdleIdentitiesRef = React.useRef<
    (agents: ConsoleAgent[], logVersionsAtRequest: Map<string, number>) => void
  >(() => {});
  function settleIdleIdentities(
    agents: ConsoleAgent[],
    logVersionsAtRequest: Map<string, number>,
  ): void {
    let changed = false;
    const nowMs = idleSettleClock.now();
    for (const agent of agents) {
      for (const identity of new Set([agent.identity, agent.member_id, agent.agent_id])) {
        if (!identity) continue;
        const log = identityLogRef.current[identity];
        if (!log) continue;
        const suffix = `:agent-chat:${identity}`;
        const sendInFlight =
          [...sendingPanelsRef.current].some((panelKey) => panelKey.endsWith(suffix)) ||
          Object.keys(optimisticUserByPanelKeyRef.current).some((panelKey) => panelKey.endsWith(suffix));
        const panelPhase = Object.entries(phaseValueByKey.current).find(
          ([panelKey, phase]) => panelKey.endsWith(suffix) && phase != null,
        )?.[1] ?? null;
        const decision = decideIdleSettlement({
          agent,
          logVersionAtRequest: logVersionsAtRequest.get(identity),
          logVersionNow: log.version,
          sendInFlight,
          foldBusy: log.busyLifecycle.busy || isIdentityBusy(identity),
          foldPhase: log.busyLifecycle.phase ?? panelPhase,
          armed: log.idlePollArmed,
          nowMs,
        });
        if (decision === "arm") log.idlePollArmed = { version: log.version, atMs: nowMs };
        if (decision !== "settle") {
          if (decision === "reset") log.idlePollArmed = undefined;
          continue;
        }
        log.idlePollArmed = undefined;
        const watermark = idleWatermarkFor(log.events);
        if (!watermark) continue;
        log.idleSettledThrough = watermark;
        log.busyLifecycle = new ConsoleActivityProjection();
        log.busyFoldedThroughMs = watermark.timestampMs;
        log.busyFoldValid = true;
        identityLifecycleRef.current[identity] = { interactionOpen: false, runOpen: false };
        applyBusyState(identity, false);
        clearPhaseForIdentity(identity);
        maybeDrainHead(identity);
        changed = true;
      }
    }
    if (changed) forceRender();
  }
  settleIdleIdentitiesRef.current = settleIdleIdentities;

  // =========================================================================
  // LOAD EXPERIENCE
  // =========================================================================

  const loadExperience = React.useCallback(() => {
    if (experienceLoadInFlightRef.current && experienceLoadGenerationRef.current === lifetimeRef.current.generation) {
      return experienceLoadInFlightRef.current;
    }

    let request: Promise<ConsoleAgent[]>;
    request = (async () => {
      // Each identity log's version when the status was requested: a frame
      // that arrives before the answer keeps that identity's fold as it is.
      const logVersionsAtRequest = new Map(
        Object.entries(identityLogRef.current).map(([identity, log]) => [identity, log.version] as const),
      );
      const [experienceJson, modulesJson] = await Promise.all([
        consoleTransport.loadExperience(),
        consoleTransport.loadModules?.() ?? Promise.resolve({ modules: [] }),
      ]);
      if (!observeScope(experienceJson.storage_scope?.trim() || undefined)) return [];
      const configuredTimeoutMs = experienceJson.console_policy?.fetch_timeout_ms;
      if (
        typeof configuredTimeoutMs === "number" &&
        Number.isFinite(configuredTimeoutMs) &&
        configuredTimeoutMs > 0
      ) {
        consoleFetchTimeoutMsRef.current = configuredTimeoutMs;
      }
      const loadedModules = Array.isArray(modulesJson.modules)
        ? modulesJson.modules.map(String)
        : [];
      const nextAgents = normalizeAgents(experienceJson, loadedModules);
      setExperience(experienceJson);
      setAgents((current) => internAgents(current, nextAgents));
      settleIdleIdentitiesRef.current(nextAgents, logVersionsAtRequest);
      setActiveActivityPresetId(
        (c) =>
          c ||
          experienceJson.console_config?.rail?.active_preset_id ||
          experienceJson.activity_feed?.active_preset_id ||
          "all",
      );
      return nextAgents;
    })().finally(() => {
      if (experienceLoadInFlightRef.current === request) {
        experienceLoadInFlightRef.current = null;
      }
    });

    experienceLoadInFlightRef.current = request;
    experienceLoadGenerationRef.current = lifetimeRef.current.generation;
    return request;
  }, [consoleTransport]);

  React.useEffect(() => {
    let mounted = true;
    setLoading(true);
    setError("");
    void loadExperience()
      .catch((e) => {
        if (mounted) setError(errorMessage(e));
      })
      .finally(() => {
        if (mounted) setLoading(false);
      });
    return () => {
      mounted = false;
    };
  }, [loadExperience]);

  React.useEffect(() => {
    const timer = window.setInterval(() => {
      void loadExperience().catch(() => {});
    }, 15_000);
    return () => window.clearInterval(timer);
  }, [loadExperience]);

  React.useEffect(() => {
    const appearance = experience?.console_config?.appearance;
    if (!appearance) return;
    const configuredTheme = normalizeConsoleTheme(appearance.default_theme);
    if (configuredTheme) {
      try {
        if (!localStorage.getItem("meta-console-theme"))
          setTheme(configuredTheme);
      } catch {
        setTheme(configuredTheme);
      }
    }
    const configuredVariant = normalizeConsoleVariant(
      appearance.default_variant,
    );
    if (configuredVariant) {
      try {
        if (!localStorage.getItem("meta-console-variant"))
          setVariant(configuredVariant);
      } catch {
        setVariant(configuredVariant);
      }
    }
  }, [experience?.console_config?.appearance, setVariant]);

  React.useEffect(() => {
    const configured = experience?.console_config?.layout?.sidebar_collapsed;
    if (typeof configured !== "boolean") return;
    try {
      if (localStorage.getItem("meta-console-sidebar-collapsed") !== null)
        return;
    } catch {
      /* ignore */
    }
    setSidebarCollapsed(configured);
  }, [experience?.console_config?.layout?.sidebar_collapsed]);

  React.useEffect(() => {
    const configured = experience?.console_config?.rail?.collapsed;
    if (typeof configured !== "boolean") return;
    try {
      if (localStorage.getItem("meta-console-rail-collapsed") !== null)
        return;
    } catch {
      /* ignore */
    }
    setRailCollapsed(configured);
  }, [experience?.console_config?.rail?.collapsed]);

  const hasMobControlSurface = experience?.runtime_id !== "console-aggregator";
  const frontendReadOnly = React.useMemo(() => resolveConsoleReadOnlyOverride(), []);
  // When access control is enforcing, `can_send_messages` is intersected
  // with the caller's per-agent send grants, so it can be false simply
  // because they currently have no send-able agent. That must NOT flip the
  // whole console to deployment read-only (which would also suppress retire/
  // respawn affordances they may still hold) — per-agent affordances are the
  // authoritative gate. Only the deployment policy and the frontend override
  // make the console globally read-only under access control.
  const accessEnforcing = experience?.access?.enabled === true;
  const consoleReadOnly =
    frontendReadOnly ||
    experience?.console_policy?.read_only === true ||
    (!accessEnforcing &&
      experience?.runtime_capabilities?.can_send_messages === false);
  const consoleReadOnlyRef = React.useRef(false);
  consoleReadOnlyRef.current = consoleReadOnly;
  // Same-key resend of an uncertain send is safe only against a durable
  // dedupe store; older gateways (field absent) and in-memory stores are not.
  const resendUncertain = experience?.send_dedupe?.durable === true;
  const resendUncertainRef = React.useRef(false);
  resendUncertainRef.current = resendUncertain;
  const [approvalSnapshot, setApprovalSnapshot] = React.useState<{
    owner: typeof consoleController;
    snapshot: PendingApprovalSnapshot;
  }>();
  const [selectedApprovalId, setSelectedApprovalId] = React.useState<string>();
  const approvalResourceRef = React.useRef<PendingApprovalResource | null>(null);
  const approvalScope = `${sendScope}:${experience?.runtime_id || "loading"}`;
  React.useEffect(() => {
    setApprovalSnapshot(undefined);
    setSelectedApprovalId(undefined);
    if (!experience || !hasMobControlSurface) return;
    const target = controlWorkbenchTarget("gating");
    const resource = createPendingApprovalResource({
      scopeKey: approvalScope,
      readOnly: consoleReadOnly,
      load: async signal => (await consoleController.commands.execute({
        command: CONSOLE_COMMAND_NAMES.listGatingPending, target, signal,
      })).result,
      decide: async (pendingId, decision, signal) => (await consoleController.commands.execute({
        command: CONSOLE_COMMAND_NAMES.decideGating, target, signal,
        params: { pending_id: pendingId, approver_id: DEFAULT_APPROVER_ID, decision, reason: `console_${decision}` },
      })).result,
    });
    approvalResourceRef.current = resource;
    const publish = () => setApprovalSnapshot({ owner: consoleController, snapshot: resource.getSnapshot() });
    const unsubscribe = resource.subscribe(publish);
    publish();
    return () => {
      unsubscribe(); resource.dispose();
      if (approvalResourceRef.current === resource) approvalResourceRef.current = null;
    };
  }, [approvalScope, Boolean(experience), hasMobControlSurface, consoleReadOnly, consoleController]);
  // Scope changes hide the previous principal's snapshot before effects run.
  const activeApprovals = approvalSnapshot?.owner === consoleController && approvalSnapshot.snapshot.scopeKey === approvalScope
    ? approvalSnapshot.snapshot : undefined;
  function openApproval(pendingId?: string) {
    setSelectedApprovalId(pendingId);
    dock.openTarget(buildControlTarget("gating"), "replace_focused");
  }
  const hasVoiceHost = experience?.voice?.readiness_method === "meta/console/voice/readiness";
  const voiceReadiness = useVoiceReadiness(
    baseUrl,
    dock.focusedTarget?.kind === "agent-chat"
      ? dock.focusedTarget.identity || dock.focusedTarget.memberId
      : null,
    voiceState.target?.identity ?? null,
    hasVoiceHost && !consoleReadOnly && voice !== null,
  );
  React.useEffect(() => {
    const target = voiceState.target;
    if (!experience || !voice || !target || !["requesting", "connecting", "active"].includes(voiceState.phase)) return;
    const voiceAgent = agents.find((agent) =>
      [agent.identity, agent.member_id, agent.agent_id].includes(target.identity),
    );
    // Only a definite gateway answer ends voice; a failed readiness poll keeps the last known state.
    if (!hasVoiceHost || voiceReadinessDenied(voiceReadiness, target.identity) || consoleReadOnly || voiceAgent?.affordances?.can_send_message !== true) {
      setActionError("Voice ended because OpenAI voice readiness or permission to message this agent could no longer be confirmed.");
      void voice.close();
    }
  }, [agents, consoleReadOnly, experience, hasVoiceHost, voice, voiceReadiness, voiceState.phase, voiceState.target]);
  const normalizedTopology = React.useMemo(
    () => normalizeConsoleTopologyQuery(topologyQueryResult, {
      agents,
      fallbackNodes: experience?.topology?.live_snapshot?.nodes || [],
      capabilities: topologyCapabilities,
      connectionSourceId: topologyConnectionSourceId,
      operations: topologyOperations,
      consoleReadOnly,
    }),
    [
      agents,
      consoleReadOnly,
      experience?.topology?.live_snapshot?.nodes,
      topologyCapabilities,
      topologyConnectionSourceId,
      topologyOperations,
      topologyQueryResult,
    ],
  );
  const visibleControls = React.useMemo<NavKind[]>(() => {
    const runtimeControls: NavKind[] = hasMobControlSurface
      ? [
          "topology",
          "timeline",
          "gating",
          "roster",
          "routing",
          "logs",
          "health",
        ]
      : ["topology", "timeline", "roster", "logs", "health"];
    const sidebarConfig = experience?.console_config?.sidebar;
    const allowedByRuntime = new Set(runtimeControls);
    const configuredVisible = (sidebarConfig?.visible_controls || [])
      .map(normalizeNavKind)
      .filter(
        (kind): kind is NavKind => Boolean(kind) && allowedByRuntime.has(kind),
      );
    if (configuredVisible.length > 0) {
      const extra: NavKind[] = [];
      if (experience?.access?.can_administer === true) extra.push("access");
      // An empty shared registry offers a setup explanation only. Configured
      // services still require the server-projected read/view grants.
      if (emptyPanelRegistry || experience?.memory?.can_read === true) extra.push("memory");
      if (emptyPanelRegistry || (experience?.workgraph?.available === true && experience?.workgraph?.can_view === true)) {
        extra.push("workgraph");
      }
      return extra.length > 0 ? [...configuredVisible, ...extra] : configuredVisible;
    }
    const hidden = new Set(
      (sidebarConfig?.hidden_controls || [])
        .map(normalizeNavKind)
        .filter((kind): kind is NavKind => Boolean(kind)),
    );
    const controls = runtimeControls.filter((kind) => !hidden.has(kind));
    // The Access admin surface is gated server-side per principal, never by
    // view config: administrators always get it, nobody else ever does.
    if (experience?.access?.can_administer === true) controls.push("access");
    // Empty shared registries keep these destinations discoverable without
    // making store requests or replacing configured-service access checks.
    if (emptyPanelRegistry || experience?.memory?.can_read === true) controls.push("memory");
    if (emptyPanelRegistry || (experience?.workgraph?.available === true && experience?.workgraph?.can_view === true)) {
      controls.push("workgraph");
    }
    return controls;
  }, [
    experience?.console_config?.sidebar,
    experience?.access?.can_administer,
    experience?.memory?.can_read,
    experience?.workgraph?.available,
    experience?.workgraph?.can_view,
    emptyPanelRegistry,
    hasMobControlSurface,
  ]);

  // =========================================================================
  // OPEN INITIAL TARGET
  // =========================================================================

  // A layout effect for the same reason as the saved-layout restore: with no
  // saved layout, the configured initial target must land before a click can.
  React.useLayoutEffect(() => {
    if (initialTargetOpened.current || dock.focusedTarget || !experience)
      return;
    if (!dockLayoutHydrated.current) return;
    if (dockLayoutRestored.current) {
      initialTargetOpened.current = true;
      return;
    }
    const layoutConfig = experience.console_config?.layout;
    let target: MetaDockTarget | null = null;
    const configuredControl = normalizeNavKind(layoutConfig?.initial_control);
    if (configuredControl && visibleControls.includes(configuredControl)) {
      target = buildControlTarget(
        configuredControl as Parameters<typeof buildControlTarget>[0],
      );
    }
    const configuredAgent = layoutConfig?.initial_agent?.trim().toLowerCase();
    if (!target && configuredAgent) {
      const match = agents.find((agent) => {
        return [
          agent.identity,
          agent.member_id,
          agent.agent_id,
          agent.label,
        ].some((value) => value?.toLowerCase() === configuredAgent);
      });
      if (match) target = buildDockTarget(match);
    }
    initialTargetOpened.current = true;
    if (!target) return;
    const preset = normalizeDockPreset(layoutConfig?.initial_preset);
    if (preset) dock.applyPreset(preset);
    dock.openTarget(target, "replace_focused");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agents, dock, experience, visibleControls]);

  React.useEffect(() => {
    const target = dock.focusedTarget;
    if (!target || target.kind !== "agent-chat" || agents.length === 0) return;
    const identity = target.identity || target.memberId;
    if (
      agents.some(
        (agent) => agent.identity === identity || agent.member_id === identity,
      )
    )
      return;
    const fallback =
      agents.find(
        (agent) => agent.addressable || agent.affordances?.can_send_message,
      ) || agents[0];
    if (fallback) {
      openAgentChat(fallback, "replace_focused");
    } else {
      dock.openTarget(buildControlTarget("roster"), "replace_focused");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agents, dock.focusedTarget]);

  // =========================================================================
  // REFRESH PANEL DATA (inspect, routing, gating)
  // =========================================================================

  // Panel and experience refreshes are triggered by stream events. One run
  // per kind at a time, with one trailing re-run for requests made mid-run.
  const panelRefreshFlight = React.useMemo(() => createSingleFlight(), []);
  // Only panels in the active tab are on screen; hidden tabs refresh when
  // they become active instead of on every event.
  const visiblePanelTargets = React.useMemo(() => {
    const activeTab = dock.viewState.tabs.find((tab) => tab.id === dock.viewState.activeTabId);
    const visible = new Set(collectConsoleDockPanelIds(activeTab?.layout));
    return dock.viewState.panels
      .filter((panel) => visible.has(panel.id))
      .map((panel) => panel.target)
      .filter(Boolean) as MetaDockTarget[];
  }, [dock.viewState.panels, dock.viewState.tabs, dock.viewState.activeTabId]);
  const accessScope = JSON.stringify([baseUrl, storageNamespace, experience?.runtime_id, experience?.access?.subject]);
  const accessScopeRef = React.useRef(accessScope);
  accessScopeRef.current = accessScope;
  const accessRefreshVersion = React.useRef(0);
  // The scope whose access read has completed at least once (success,
  // refusal or failure). Until then every read blocks edits.
  const accessSettledScope = React.useRef<string | null>(null);
  // The newest access revision of this scope that the Console has read or
  // that the owner signalled (access_changed); an older or equal signal
  // reads nothing.
  const accessRevisionRef = React.useRef<number | undefined>(undefined);
  const visibleAccessData = accessData.scope === accessScope ? accessData : null;
  React.useEffect(() => {
    accessRefreshVersion.current += 1;
    accessSettledScope.current = null;
    accessRevisionRef.current = undefined;
    setAccessData({ scope: accessScope, loading: false, status: null, config: null, error: null });
  }, [accessScope, experience?.access?.can_administer]);
  // An explicit read (Refresh, the read after a save) disables edits from the
  // moment it is requested and drops any read already in flight, so nothing
  // older lands after it. The first read of a scope disables edits from its
  // request too. A background read in a settled scope changes nothing until
  // it lands; the owner still checks the draft's edit token on every save.
  // The single flight keeps only the latest task for its trailing run, so a
  // background trigger can replace an explicit request's trailing read. That
  // is safe because both modes complete the same way and the explicit request
  // applied its effects when it was made.
  const refreshAccessData = React.useCallback((mode: "explicit" | "background" = "explicit") => {
    if (accessScope === accessScopeRef.current && (mode === "explicit" || accessSettledScope.current !== accessScope)) {
      if (mode === "explicit") accessRefreshVersion.current += 1;
      setAccessData(current => ({
        scope: accessScope, loading: true, error: null,
        status: current.scope === accessScope ? current.status : null,
        config: current.scope === accessScope ? current.config : null,
      }));
    }
    return panelRefreshFlight("access", async () => {
      if (accessScope !== accessScopeRef.current) return;
      const version = ++accessRefreshVersion.current;
      const isCurrent = () => version === accessRefreshVersion.current && accessScope === accessScopeRef.current;
      const accessTarget = controlWorkbenchTarget("access");
      let status: ConsoleAccessStatus | null = null;
      try {
        status = ((await executeHeadlessCommand(CONSOLE_COMMAND_NAMES.accessStatus, accessTarget)) as ConsoleAccessStatus | null) || null;
        if (!isCurrent()) return;
        let config: ConsoleAccessConfig | null = null;
        if (status?.available && status?.can_administer) {
          const result = (await executeHeadlessCommand(CONSOLE_COMMAND_NAMES.getAccessConfig, accessTarget)) as { config?: ConsoleAccessConfig; revision?: number; owner_instance?: string; conditional_mutations?: string } | null;
          config = result?.config || null;
          // Config and its edit token must come from the same protected read.
          status = { ...status, revision: result?.revision, owner_instance: result?.owner_instance,
            conditional_mutations: status.conditional_mutations === "checked_v1" ? result?.conditional_mutations : undefined };
        }
        if (!isCurrent()) return;
        accessSettledScope.current = accessScope;
        if (typeof status?.revision === "number") {
          accessRevisionRef.current = Math.max(accessRevisionRef.current ?? status.revision, status.revision);
        }
        setAccessData({ scope: accessScope, loading: false, status, config, error: null });
      } catch (err) {
        if (!isCurrent()) return;
        accessSettledScope.current = accessScope;
        const failure = classifyConsoleSendFailure(err);
        const forbidden = failure.kind === "access_denied" || failure.kind === "unauthenticated";
        setAccessData(current => ({ ...current, loading: false, error: errorMessage(err),
          ...(forbidden ? { status: null, config: null } : {}),
        }));
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [panelRefreshFlight, baseUrl, accessScope]);

  const refreshMemoryData = React.useCallback(() => panelRefreshFlight(`memory:${panelRuntimeKey}`, async () => {
    if (emptyPanelRegistry) return;
    const memoryTarget = controlWorkbenchTarget("memory");
    try {
      let records: MemoryPanelRecord[] = [];
      let realms: string[] = [];
      let nextCursor: string | null = null;
      let recordsDenied = false;
      try {
        const recordsResult = (await executeHeadlessCommand(
          CONSOLE_COMMAND_NAMES.listMemoryRecords,
          memoryTarget,
        )) as MemoryPanelRecordsResult | null;
        records = recordsResult?.records || [];
        realms = recordsResult?.realms || [];
        nextCursor = recordsResult?.next_cursor ?? null;
      } catch (err) {
        // Access denied to records leaves the section empty but flagged, so
        // the panel renders "no grant" instead of an empty store.
        if (memorySectionOutcome(err) !== "denied") throw err;
        recordsDenied = true;
      }

      // The unfiltered listing row-filters restricted scopes silently, so a
      // one-row probe per restricted scope kind is the only way to render
      // the spec'd access-denied Holdings row instead of the scope silently
      // not existing. Skipped when the listing itself was denied.
      let operatorScopeDenied = false;
      let mobScopeDenied = false;
      if (!recordsDenied) {
        const probeScope = async (scope: "operator" | "mob"): Promise<boolean> => {
          try {
            await executeHeadlessCommand(CONSOLE_COMMAND_NAMES.listMemoryRecords, memoryTarget, {
              scope,
              limit: 1,
            });
            return false;
          } catch (err) {
            if (memorySectionOutcome(err) === "denied") return true;
            throw err;
          }
        };
        [operatorScopeDenied, mobScopeDenied] = await Promise.all([
          probeScope("operator"),
          probeScope("mob"),
        ]);
      }

      let quarantineRecords: MemoryPanelRecord[] = [];
      let pendingPromotions: MemoryPendingPromotion[] = [];
      let canDecideQuarantine = false;
      if (experience?.memory?.can_review_quarantine === true) {
        // The decision is advertised only to reviewers on a mutable console;
        // a capabilities failure leaves the queue read-only.
        try {
          const capabilities = await consoleTransport.capabilities();
          canDecideQuarantine = capabilities.methods.includes(
            consoleCommandMethod(CONSOLE_COMMAND_NAMES.decideMemoryQuarantine),
          );
        } catch {
          canDecideQuarantine = false;
        }
        try {
          const quarantineResult = (await executeHeadlessCommand(
            CONSOLE_COMMAND_NAMES.listMemoryQuarantine,
            memoryTarget,
          )) as MemoryPanelQuarantineResult | null;
          quarantineRecords = quarantineResult?.records || [];
          pendingPromotions = quarantineResult?.pending_promotions || [];
        } catch (err) {
          // Access denied to the quarantine queue leaves it empty rather than
          // failing the whole panel.
          if (jsonRpcErrorCode(err) !== -32030) throw err;
        }
      }

      let dreams: MemoryDreamRun[] = [];
      let dreamsDenied = false;
      try {
        const dreamsResult = (await executeHeadlessCommand(
          CONSOLE_COMMAND_NAMES.listMemoryDreams,
          memoryTarget,
        )) as MemoryPanelDreamsResult | null;
        dreams = dreamsResult?.runs || [];
      } catch (err) {
        // Tolerate access-denied for dreams: leave the list empty but flagged.
        if (memorySectionOutcome(err) !== "denied") throw err;
        dreamsDenied = true;
      }

      // Phase-2 read surfaces, fetched in parallel with the same per-section
      // -32030 tolerance: a denied surface renders "no grant", never an
      // indistinguishable empty section, and never aborts the panel load.
      const section = async <T,>(
        command: (typeof CONSOLE_COMMAND_NAMES)[keyof typeof CONSOLE_COMMAND_NAMES],
        empty: T,
        pick: (result: unknown) => T,
      ): Promise<{ value: T; denied: boolean }> => {
        try {
          const result = await executeHeadlessCommand(command, memoryTarget);
          return { value: pick(result), denied: false };
        } catch (err) {
          if (memorySectionOutcome(err) !== "denied") throw err;
          return { value: empty, denied: true };
        }
      };
      const [overview, proposals, injections, harvests, dreamRuns, auditVerdicts] =
        await Promise.all([
          section<MemoryPanelOverviewResult | null>(
            CONSOLE_COMMAND_NAMES.getMemoryOverview,
            null,
            (result) => (result as MemoryPanelOverviewResult | null) ?? null,
          ),
          section<MemoryProposalEntry[]>(
            CONSOLE_COMMAND_NAMES.listMemoryProposals,
            [],
            (result) => (result as MemoryPanelProposalsResult | null)?.proposals || [],
          ),
          section<MemoryLedgerEntry[]>(
            CONSOLE_COMMAND_NAMES.listMemoryInjections,
            [],
            (result) => (result as MemoryPanelInjectionsResult | null)?.injections || [],
          ),
          section<MemoryHarvestEntry[]>(
            CONSOLE_COMMAND_NAMES.listMemoryHarvests,
            [],
            (result) => (result as MemoryPanelHarvestsResult | null)?.harvests || [],
          ),
          section<MemoryDreamRunSheet[]>(
            CONSOLE_COMMAND_NAMES.listMemoryDreamRuns,
            [],
            (result) => (result as MemoryPanelDreamRunsResult | null)?.runs || [],
          ),
          section<MemoryAuditVerdictEntry[]>(
            CONSOLE_COMMAND_NAMES.listMemoryAuditVerdicts,
            [],
            (result) => (result as MemoryPanelAuditVerdictsResult | null)?.verdicts || [],
          ),
        ]);

      if (panelRuntimeKeyRef.current !== panelRuntimeKey) return;
      setMemoryDataRuntimeKey(panelRuntimeKey);
      setMemoryData((current) => ({
        ...current,
        records,
        realms,
        quarantineRecords,
        pendingPromotions,
        dreams,
        nextCursor,
        recordsDenied,
        dreamsDenied,
        operatorScopeDenied,
        mobScopeDenied,
        overview: overview.value,
        overviewDenied: overview.denied,
        proposals: proposals.value,
        proposalsDenied: proposals.denied,
        injections: injections.value,
        injectionsDenied: injections.denied,
        harvests: harvests.value,
        harvestsDenied: harvests.denied,
        dreamRuns: dreamRuns.value,
        dreamRunsDenied: dreamRuns.denied,
        auditVerdicts: auditVerdicts.value,
        auditVerdictsDenied: auditVerdicts.denied,
        canDecideQuarantine,
        unavailable: false,
        error: null,
      }));
    } catch (err) {
      // -32601 means the panel is not configured on this runtime.
      if (panelRuntimeKeyRef.current !== panelRuntimeKey) return;
      setMemoryDataRuntimeKey(panelRuntimeKey);
      if (jsonRpcErrorCode(err) === -32601) {
        setMemoryData((current) => ({ ...current, unavailable: true, error: null }));
        return;
      }
      setMemoryData((current) => ({ ...current, error: errorMessage(err) }));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }), [panelRefreshFlight, baseUrl, panelRuntimeKey, emptyPanelRegistry, experience?.memory?.can_review_quarantine]);

  // Overlapping refreshes (debounced live signals, manual refresh, post-
  // mutation re-reads) can resolve out of order — sequence them so a stale
  // snapshot never overwrites a fresher one.
  const workGraphRefreshSequencerRef = React.useRef(createWorkGraphRefreshSequencer());
  const refreshWorkGraphData = React.useCallback(() => panelRefreshFlight(`workgraph:${panelRuntimeKey}`, async () => {
    if (emptyPanelRegistry) return;
    const workGraphTarget = controlWorkbenchTarget("workgraph");
    const isCurrent = workGraphRefreshSequencerRef.current.begin();
    try {
      let snapshot: WorkGraphSnapshotResult | null = null;
      let denied = false;
      try {
        // The panel is the operator inspection surface: completed/cancelled/
        // failed items must stay visible (tree status column, graph status
        // classes), so opt into terminal rows - the default snapshot is the
        // live working set and would drop them on the first refresh.
        snapshot = (await executeHeadlessCommand(
          CONSOLE_COMMAND_NAMES.workgraphSnapshot,
          workGraphTarget,
          { include_terminal: true },
        )) as WorkGraphSnapshotResult | null;
      } catch (err) {
        // Access denied renders as "no grant", never an empty graph.
        if (jsonRpcErrorCode(err) !== -32030) throw err;
        denied = true;
      }
      let events: WorkGraphWireEvent[] = [];
      if (!denied) {
        try {
          // Page the tail from the snapshot's high-water mark: upstream
          // returns events ASCENDING truncated to limit, so a bare {limit}
          // query would freeze on the oldest window forever.
          const eventsResult = (await executeHeadlessCommand(
            CONSOLE_COMMAND_NAMES.workgraphEvents,
            workGraphTarget,
            workGraphEventsParams(snapshot?.event_high_water_mark, 50),
          )) as WorkGraphEventsResult | null;
          events = workGraphEventsNewestFirst(eventsResult?.events || []);
        } catch (err) {
          // The events tail is optional; a denied ledger leaves it empty.
          if (jsonRpcErrorCode(err) !== -32030) throw err;
        }
      }
      if (!isCurrent() || panelRuntimeKeyRef.current !== panelRuntimeKey) return;
      setWorkGraphDataRuntimeKey(panelRuntimeKey);
      setWorkGraphData({
        items: snapshot?.items || [],
        edges: snapshot?.edges || [],
        attention: snapshot?.attention || [],
        events,
        capturedAt: snapshot?.captured_at || null,
        unavailable: false,
        denied,
        error: null,
      });
    } catch (err) {
      if (!isCurrent() || panelRuntimeKeyRef.current !== panelRuntimeKey) return;
      setWorkGraphDataRuntimeKey(panelRuntimeKey);
      // -32601 (method absent) and -32041 (workgraph_unavailable) both mean
      // no WorkGraph service on this runtime; so does a missing capability
      // advertisement (the headless layer throws before dispatch).
      const code = jsonRpcErrorCode(err);
      const capabilityMissing =
        err instanceof Error && err.message.startsWith("Meta capability missing");
      if (code === -32601 || code === -32041 || capabilityMissing) {
        setWorkGraphData((current) => ({ ...current, unavailable: true, error: null }));
        return;
      }
      setWorkGraphData((current) => ({ ...current, error: errorMessage(err) }));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }), [panelRefreshFlight, baseUrl, panelRuntimeKey, emptyPanelRegistry]);

  /// Filtered/paged panel/records query for the Records filter bar, the
  /// keyset load-more, and the lattice page-walk. Resolves null STRICTLY on
  /// -32030 (denied — consumers render "no grant", never an empty store);
  /// other failures surface via the panel error banner and rethrow so the
  /// pager keeps the prior page instead of clobbering it.
  const queryMemoryRecords = React.useCallback(
    async (params: Record<string, unknown>): Promise<MemoryPanelRecordsResult | null> => {
      try {
        return (await executeHeadlessCommand(
          CONSOLE_COMMAND_NAMES.listMemoryRecords,
          controlWorkbenchTarget("memory"),
          params,
        )) as MemoryPanelRecordsResult | null;
      } catch (err) {
        if (memorySectionOutcome(err) === "denied") return null;
        if (panelRuntimeKeyRef.current === panelRuntimeKey) setMemoryData((current) => ({ ...current, error: errorMessage(err) }));
        throw err;
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [baseUrl, panelRuntimeKey],
  );

  /// Evidence click-through: resolve a MemoryEvidenceRef against the
  /// contracted query_timeline surface. Resolves null when the session is no
  /// longer in the timeline — the Biography degrades to the evidenceLabel text.
  const loadMemoryEvidence = React.useCallback(
    async (
      identity: string | undefined,
      evidence: MemoryEvidenceRef,
    ): Promise<ConversationTimelineEntry[] | null> => {
      if (!evidence.session_id) return null;
      try {
        const namespace = experience?.panel_runtimes?.find(runtime => runtime.key === panelRuntimeKey)?.namespace;
        const scopedIdentity = identity && namespace && !identity.startsWith(`${namespace}/`) ? `${namespace}/${identity}` : identity;
        const pageFact = await consoleController.timeline.query({
          ...(scopedIdentity ? { identity: scopedIdentity } : {}),
          mode: "recent",
          limit: 1000,
        });
        const page = pageFact.value;
        if (!page.available) return null;
        const frames = page.frames.filter(
          (frame) => frame.sessionId === evidence.session_id,
        );
        if (frames.length === 0) return null;
        return mapFramesToTimelineEntries(null, frames, {
          renderInteractionStartsAsUser: true,
        });
      } catch {
        return null;
      }
    },
    [consoleController, panelRuntimeKey, experience?.panel_runtimes],
  );

  const loadMemoryRecordDetail = React.useCallback(
    async (realm: string | undefined, memoryId: string, preserveDetail = false) => {
      const request = ++memoryDetailRequestRef.current;
      setMemoryData((current) => ({ ...current, detail: preserveDetail ? current.detail : null, detailLoading: true, error: null }));
      try {
        const result = (await executeHeadlessCommand(
          CONSOLE_COMMAND_NAMES.getMemoryRecord,
          controlWorkbenchTarget("memory"),
          realm ? { realm, memory_id: memoryId } : { memory_id: memoryId },
        )) as MemoryPanelRecordResult | null;
        const detail: MemoryRecordDetail | null = result?.record
          ? {
              realm: result.realm,
              record: result.record,
              chain: result.chain || [],
              injections: result.injections || [],
            }
          : null;
        if (panelRuntimeKeyRef.current !== panelRuntimeKey || request !== memoryDetailRequestRef.current) return;
        setMemoryData((current) => ({ ...current, detail, detailLoading: false }));
      } catch (err) {
        if (panelRuntimeKeyRef.current !== panelRuntimeKey || request !== memoryDetailRequestRef.current) return;
        setMemoryData((current) => ({
          ...current,
          detail: preserveDetail ? current.detail : null,
          detailLoading: false,
          error: errorMessage(err),
        }));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [baseUrl, panelRuntimeKey],
  );

  /// One quarantine decision (meta/memory/quarantine/decide), bound to the
  /// content hash of the record the reviewer is looking at. Rejects with the
  /// transport's typed error; the panel renders refusals. The queue is
  /// re-read after every decision.
  const decideMemoryQuarantine = React.useCallback(
    async (
      realm: string,
      record: MemoryFullRecord,
      verdict: MemoryQuarantineVerdict,
      rationale: string | undefined,
    ): Promise<MemoryQuarantineDecideResult> => {
      const identity = record.scope.scope === "identity" ? record.scope.identity : undefined;
      const result = (await executeHeadlessCommand(
        CONSOLE_COMMAND_NAMES.decideMemoryQuarantine,
        controlWorkbenchTarget("memory"),
        {
          realm,
          identity,
          memory_id: record.id,
          verdict,
          expected_content_hash: record.content_hash,
          ...(rationale ? { rationale } : {}),
        },
      )) as MemoryQuarantineDecideResult;
      void refreshMemoryData();
      return result;
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [baseUrl, refreshMemoryData],
  );

  const runAccessMutation = React.useCallback(
    async (
      command:
        | typeof CONSOLE_COMMAND_NAMES.setAccessConfig
        | typeof CONSOLE_COMMAND_NAMES.enableAccess
        | typeof CONSOLE_COMMAND_NAMES.upsertAccessRule
        | typeof CONSOLE_COMMAND_NAMES.deleteAccessRule
        | typeof CONSOLE_COMMAND_NAMES.setAccessGroup
        | typeof CONSOLE_COMMAND_NAMES.deleteAccessGroup,
      params: Record<string, unknown>,
      base: ConsoleAccessEditBase,
    ) => {
      if (accessData.scope !== accessScope || accessData.loading || accessData.error
          || accessData.status?.available !== true || accessData.status.can_administer !== true
          || accessData.status.conditional_mutations !== "checked_v1"
          || typeof accessData.status.owner_instance !== "string" || !accessData.status.owner_instance
          || !Number.isSafeInteger(accessData.status.revision) || accessData.status.revision! < 0
          || typeof base.owner_instance !== "string" || !base.owner_instance
          || !Number.isSafeInteger(base.revision) || base.revision < 0
          || experience?.access?.can_administer !== true || frontendReadOnly
          || experience?.console_policy?.read_only === true) return false;
      // The draft's own token travels with the edit; the owner, not this
      // client, decides whether it is still current.
      let failure: ConsoleAccessSaveFailure | null = null;
      try {
        await executeHeadlessCommand(command, controlWorkbenchTarget("access"), {
          checked_v1: { ...params, owner_instance: base.owner_instance, expected_revision: base.revision },
        });
      } catch (err) {
        failure = accessSaveFailure(err);
      }
      if (accessScope !== accessScopeRef.current) return false;
      await refreshAccessData();
      if (accessScope !== accessScopeRef.current) return false;
      await loadExperience().catch(() => {});
      if (accessScope !== accessScopeRef.current) return false;
      return failure ?? true;
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [baseUrl, refreshAccessData, loadExperience, accessData, accessScope, experience?.access?.can_administer, frontendReadOnly, experience?.console_policy?.read_only],
  );

  const refreshTopologyData = React.useCallback(() => panelRefreshFlight("topology", async () => {
    try {
      const capabilities = await consoleTransport.capabilities();
      setTopologyCapabilities(capabilities.topologyControl || null);
      const queryMethod = consoleCommandMethod(CONSOLE_COMMAND_NAMES.topologyQuery);
      if (!capabilities.methods.includes(queryMethod)) {
        // Older and topology-unaware runtimes retain the passive graph/roles
        // console. Mutation UI is never inferred from generic wiring support.
        setTopologyQueryResult(null);
        return;
      }
      const result = await executeHeadlessCommand(
        CONSOLE_COMMAND_NAMES.topologyQuery,
        controlWorkbenchTarget("topology"),
      );
      setTopologyQueryResult(result);
    } catch (error) {
      // Capability loss, an authorization change, or a failed refresh must
      // revoke stale edit affordances immediately. The passive topology view
      // remains available from the experience snapshot.
      setTopologyCapabilities(null);
      setTopologyQueryResult(null);
      throw error;
    }
  }), [panelRefreshFlight, consoleTransport]);

  // Access is read when its panel opens or its scope changes, on Refresh and
  // after a save, and when the owner signals a newer access revision
  // (access_changed); a refresh caused by activity leaves it out.
  const refreshPanelData = React.useCallback(async ({ access = true }: { access?: boolean } = {}) => {
    const openPanels = visiblePanelTargets;
    const inspects = openPanels.filter(
      (t): t is Extract<MetaDockTarget, { kind: "identity-inspect" }> =>
        t.kind === "identity-inspect",
    );
    // Each kind refreshes independently, so one slow surface (topology on a
    // loaded server) neither delays the others nor stacks up behind itself.
    const refreshes: Promise<void>[] = inspects.map((t) => panelRefreshFlight(`inspect:${t.identity}`, async () => {
      const r = await inspectIdentityViaHeadless(t.identity);
      const result = normalizeConsoleInspectResult(r);
      setInspectByIdentity((c) => ({ ...c, [t.identity]: result }));
    }));
    if (hasMobControlSurface && openPanels.some((t) => t.kind === "routing")) {
      refreshes.push(panelRefreshFlight("routing", async () => {
        const routingTarget = controlWorkbenchTarget("routing");
        const [routes, history] = await Promise.all([
          executeHeadlessCommand(CONSOLE_COMMAND_NAMES.listRoutingRoutes, routingTarget),
          executeHeadlessCommand(CONSOLE_COMMAND_NAMES.listDeliveryHistory, routingTarget),
        ]);
        setRoutingData(
          buildRoutingSectionView({
            routesResponse: routes,
            historyResponse: history,
          }),
        );
      }));
    }
    if (access && openPanels.some((t) => t.kind === "access")) refreshes.push(refreshAccessData("background"));
    if (openPanels.some((t) => t.kind === "memory")) refreshes.push(refreshMemoryData());
    if (openPanels.some((t) => t.kind === "workgraph")) refreshes.push(refreshWorkGraphData());
    if (openPanels.some((t) => t.kind === "topology")) refreshes.push(refreshTopologyData());
    if (
      hasMobControlSurface &&
      openPanels.some((t) => t.kind === "gating" || t.kind === "gates")
    ) {
      refreshes.push(panelRefreshFlight("gating-audit", async () => {
        const audit = await executeHeadlessCommand(CONSOLE_COMMAND_NAMES.listGatingAudit, controlWorkbenchTarget("gating"), { limit: 50 }) as { entries?: unknown[] };
        setGatingData({ pending: [], audit: Array.isArray(audit?.entries) ? audit.entries : [] });
      }));
    }
    await Promise.all(refreshes);
  }, [baseUrl, visiblePanelTargets, hasMobControlSurface, panelRefreshFlight, refreshAccessData, refreshMemoryData, refreshTopologyData, refreshWorkGraphData]);

  const refreshPanelDataRef = React.useRef(refreshPanelData);
  refreshPanelDataRef.current = refreshPanelData;
  // Refresh visible panels when their layout or access owner changes.
  const visiblePanelKey = JSON.stringify(visiblePanelTargets);
  React.useEffect(() => {
    void refreshPanelDataRef.current().catch(() => {});
  }, [visiblePanelKey, accessScope, panelRuntimeKey, emptyPanelRegistry, experience?.access?.can_administer]);

  const scheduleExperienceRefresh = React.useCallback(() => {
    if (experienceTimerRef.current !== null) return;
    experienceTimerRef.current = window.setTimeout(() => {
      experienceTimerRef.current = null;
      // Events keep arriving while a refresh is in flight; they coalesce
      // into one trailing refresh instead of overlapping requests. The roster
      // and each panel kind coalesce separately, so a slow panel (topology
      // on a loaded server) never holds back the others.
      void panelRefreshFlight("experience-events", () => loadExperience().then(() => {}, () => {}));
      void refreshPanelDataRef.current({ access: false }).catch(() => {});
    }, 150);
  }, [loadExperience, panelRefreshFlight]);

  // The timeline stream names the owner's access revision when it goes live
  // and again after each committed access change (access_changed). A
  // revision newer than any this Console has read or been told, or the first
  // one it hears, reads Access again when its panel is open, and the roster,
  // whose visibility the change may have moved.
  const handleAccessChangedRef = React.useRef<(revision: number | undefined) => void>(() => {});
  handleAccessChangedRef.current = (revision) => {
    const known = accessRevisionRef.current;
    if (typeof revision === "number") {
      if (typeof known === "number" && revision <= known) return;
      accessRevisionRef.current = revision;
    }
    if (visiblePanelTargets.some((target) => target.kind === "access")) {
      void refreshAccessData("background").catch(() => {});
    }
    scheduleExperienceRefresh();
  };

  // =========================================================================
  // HISTORY REFRESH — server is the single source of truth
  // =========================================================================

  const scheduleHistoryRefresh = React.useCallback(
    (identity: string) => {
      clearTimeout(refreshTimersRef.current[identity]);
      refreshTimersRef.current[identity] = window.setTimeout(async () => {
        const log = getOrCreateLog(identity);
        // No event log on this runtime — SSE is the canonical source,
        // there's nothing to backfill.
        if (log.hasServerLog === false) {
          recomputePhaseForIdentity(identity);
          forceRender();
          return;
        }
        try {
          await refreshIdentityTimelineNow(identity, { clearPhase: true });
        } catch {
          /* silent — will retry on next terminal event */
        }
      }, 200);
      // eslint-disable-next-line react-hooks/exhaustive-deps
    },
    [baseUrl, forceRender],
  );

  // =========================================================================
  // PANEL OPEN / SWITCH — fetch history for new identities
  // =========================================================================

  // Operator workgraph mutations live only as client-local echo frames, so a
  // page reload silently reverts the inline cards to agent-observed state
  // until an agent next touches the graph. Once per identity per mount, after
  // a page of history folds in, a pane whose restored transcript contains a
  // workgraph card fetches ONE snapshot and folds it through the refresh-echo
  // path (`refresh: true` — a read, never an action outcome) so the cards
  // re-hydrate to live status/revisions. See `createWorkGraphHydrationGate`
  // for the once-per-identity decision: a scroll-up fold that first
  // introduces cards (loadOlderIdentityTimeline) can still trigger the fetch.
  const workGraphHydrationGateRef = React.useRef(createWorkGraphHydrationGate());
  async function hydrateWorkGraphCardsForIdentity(identity: string): Promise<void> {
    const shouldFetch = workGraphHydrationGateRef.current.shouldFetch(identity, {
      workgraphAvailable: experience?.workgraph?.available === true,
      hasCards: framesContainWorkGraphCards(getSortedFrames(identity)),
    });
    if (!shouldFetch) return;
    try {
      const snapshot = await executeHeadlessCommand(
        CONSOLE_COMMAND_NAMES.workgraphSnapshot,
        controlWorkbenchTarget("workgraph"),
        experience?.panel_runtimes ? {
          runtime_key: experience.panel_runtimes.find(runtime => identity.startsWith(`${runtime.namespace}/`))?.key || "",
        } : undefined,
      );
      appendFrame(
        identity,
        buildWorkGraphOperatorResultFrame({
          method: consoleCommandMethod(CONSOLE_COMMAND_NAMES.workgraphSnapshot),
          params: {},
          // The RPC returns the WorkGraphSnapshot verbatim; the fold expects
          // the tool-result wrapper shape.
          result: { snapshot },
          identity,
          refresh: true,
        }),
      );
      forceRender();
    } catch {
      // Best-effort hydration: the card keeps its restored state and the
      // next operator action heals revisions through the conflict path.
    }
  }

  React.useEffect(() => {
    for (const panel of dock.viewState.panels) {
      const target = panel.target as MetaDockTarget | null;
      if (!target || target.kind !== "agent-chat") continue;
      const identity = target.identity || target.memberId;
      const log = getOrCreateLog(identity);
      // Only fetch backfill once per identity — when we don't yet
      // know whether the runtime has an event log. Subsequent panel
      // re-opens reuse the existing log; we never wipe it.
      if (log.hasServerLog !== null) {
        void hydrateWorkGraphCardsForIdentity(identity);
        continue;
      }
      void refreshIdentityTimelineNow(identity)
        .then(() => hydrateWorkGraphCardsForIdentity(identity))
        .catch(() => {});
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [baseUrl, dock.viewState.panels, forceRender, experience?.workgraph?.available]);

  /// Repair one docked identity's log after the stream reported a replay
  /// gap: a `since` query from the newest known cursor, falling back to a
  /// fresh recent page when the server rejects the cursor as stale
  /// (`replay_unavailable`). This is the only paged query outside panel
  /// open, older-history paging and the terminal-frame reconcile
  /// (`scheduleHistoryRefresh`). The unscoped live stream carries every
  /// frame the runtime appends, including asynchronously backfilled session
  /// history and synthetic `replay_unavailable` frames (see
  /// `append_and_emit` in the console aggregator), so there is no periodic
  /// poll; the 2 s per-identity poll this replaces duplicated the stream
  /// and replayed the busy fold on every response. Concurrent calls for
  /// the same identity coalesce into the in-flight one.
  const identityRefreshInFlightRef = React.useRef(new Set<string>());
  const repairIdentityAfterReplayGap = React.useCallback(
    async (identity: string): Promise<boolean> => {
      const log = getOrCreateLog(identity);
      if (log.hasServerLog === false) return false;
      if (identityRefreshInFlightRef.current.has(identity)) return false;
      identityRefreshInFlightRef.current.add(identity);
      let changed = false;
      try {
        const sinceCursor =
          log.latestTimelineCursor &&
          !(log.olderHistoryExhausted === true && !log.olderHistoryExhaustedAtCursor)
            ? log.latestTimelineCursor
            : undefined;
        const { page, metadataChanged } = await queryIdentityTimelinePage(identity, {
          mode: sinceCursor ? "since" : "recent",
          after: sinceCursor,
          limit: sinceCursor ? 1000 : 200,
        });
        if (reconcileServerLog(identity, page.frames, page.available) || metadataChanged) {
          changed = true;
        }
      } catch (error) {
        const replay = error as Error & {
          replayError?: ConsoleReplayUnavailablePayload;
          timelineReplayUnavailable?: boolean;
        };
        if (replay.timelineReplayUnavailable || replay.replayError?.stream === "timeline") {
          if (resetIdentityTimelineReplayMetadata(identity)) changed = true;
          try {
            const { page, metadataChanged } = await queryIdentityTimelinePage(identity, {
              mode: "recent",
              limit: 200,
            });
            if (reconcileServerLog(identity, page.frames, page.available) || metadataChanged) {
              changed = true;
            }
          } catch {
            // Keep the panel usable; the next gap or refresh will retry.
          }
        }
      } finally {
        identityRefreshInFlightRef.current.delete(identity);
      }
      if (changed) forceRender();
      return changed;
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [baseUrl, forceRender],
  );
  const repairIdentityAfterReplayGapRef = React.useRef(repairIdentityAfterReplayGap);
  repairIdentityAfterReplayGapRef.current = repairIdentityAfterReplayGap;

  // =========================================================================
  // GLOBAL SSE EVENT STREAM — the core event loop
  // =========================================================================

  // Stable refs for callbacks used in SSE handler — prevents effect re-runs
  const scheduleHistoryRefreshRef = React.useRef(scheduleHistoryRefresh);
  scheduleHistoryRefreshRef.current = scheduleHistoryRefresh;
  const scheduleExperienceRefreshRef = React.useRef(scheduleExperienceRefresh);
  scheduleExperienceRefreshRef.current = scheduleExperienceRefresh;
  // Memory panels anchor on durable SQLite state; live memory.* frames are
  // freshness signals only. Debounced so dream-commit bursts coalesce into
  // one re-read, and gated on a memory panel actually being docked.
  const refreshMemoryDataRef = React.useRef(refreshMemoryData);
  refreshMemoryDataRef.current = refreshMemoryData;
  const memoryPanelDockedRef = React.useRef(false);
  memoryPanelDockedRef.current = visiblePanelTargets.some((target) => target.kind === "memory");
  // Identities with a docked chat, read by the mount-scoped stream
  // subscription when it repairs after a replay gap.
  const dockedChatIdentitiesRef = React.useRef<string[]>([]);
  dockedChatIdentitiesRef.current = dock.viewState.panels.flatMap((panel) => {
    const target = panel.target as MetaDockTarget | null;
    return target && target.kind === "agent-chat" ? [target.identity || target.memberId] : [];
  });
  const memoryRefreshTimerRef = React.useRef<number | null>(null);
  // WorkGraph panel mirrors the memory freshness pattern: live workgraph
  // signals (workgraph.* frames or workgraph_* tool completions) trigger one
  // debounced snapshot re-read, only while a WorkGraph panel is docked.
  const refreshWorkGraphDataRef = React.useRef(refreshWorkGraphData);
  refreshWorkGraphDataRef.current = refreshWorkGraphData;
  const workGraphPanelDockedRef = React.useRef(false);
  workGraphPanelDockedRef.current = visiblePanelTargets.some((target) => target.kind === "workgraph");
  const workGraphRefreshTimerRef = React.useRef<number | null>(null);

  React.useEffect(() => {
    // Seed activity with recent history (only on mount) — apply same
    // filter as SSE. The activity rail is its own concern; it doesn't
    // share state with the per-identity logs.
    const handleLiveFrame = (incomingFrame: ConsoleFrame) => {
      if (incomingFrame.event === "access_changed") {
        const signalled = (incomingFrame.data as { revision?: unknown } | null | undefined)?.revision;
        handleAccessChangedRef.current(typeof signalled === "number" ? signalled : undefined);
        return;
      }
      const canonicalIdentity = canonicalConsoleIdentityFromMap(
        incomingFrame.identity,
        identityAliasesRef.current,
      );
      const frame =
        canonicalIdentity && canonicalIdentity !== incomingFrame.identity
          ? { ...incomingFrame, identity: canonicalIdentity }
          : incomingFrame;
      const cause: RenderCause = frame.event === "text_delta" ? "text"
        : frame.event === "text_complete" || HISTORY_REFRESH_EVENTS.has(frame.event) || isTerminalTurnCompletedFrame(frame) ? "now"
        : "frame";
      // Activity rail (independent buffer)
      if (!ACTIVITY_SKIP_EVENTS.has(frame.event)) {
        activityRef.current = [frame, ...activityRef.current].slice(0, 200);
      }

      // Topology-class buffer keeps tool events (peer-comms etc.) which
      // the activity rail filters out. Capped at 300; older frames roll
      // off naturally as live pulses age past their lifetime.
      if (PANEL_ROUTABLE_EVENTS.has(frame.event)) {
        // A completion renders once, from forceRender below.
        commitLiveFrames([frame, ...liveFramesRef.current].slice(0, 300), cause === "now" ? "frame" : cause);
      }

      // Identity log (single canonical store)
      const identity = canonicalIdentity || frame.identity?.trim();
      if (
        PANEL_ROUTABLE_EVENTS.has(frame.event) &&
        identity &&
        identity !== "_system"
      ) {
        appendFrame(identity, frame);
        updateBusyStateForFrame(identity, frame);
        updatePhaseForIdentity(identity, frame);
      }

      forceRender(cause);

      // Terminal events → reconcile server backfill (idempotent — keys
      // already seen via SSE are skipped). If hasServerLog is false,
      // scheduleHistoryRefresh short-circuits.
      if (
        (HISTORY_REFRESH_EVENTS.has(frame.event) || isTerminalTurnCompletedFrame(frame)) &&
        identity &&
        identity !== "_system"
      ) {
        scheduleHistoryRefreshRef.current(identity);
      }
      if (REFRESH_TRIGGER_EVENTS.has(frame.event)) {
        scheduleExperienceRefreshRef.current();
      }
      if (
        frame.event.startsWith("memory.") &&
        memoryPanelDockedRef.current &&
        memoryRefreshTimerRef.current === null
      ) {
        memoryRefreshTimerRef.current = window.setTimeout(() => {
          memoryRefreshTimerRef.current = null;
          void refreshMemoryDataRef.current().catch(() => {});
        }, 250);
      }
      if (
        isWorkGraphSignalFrame(frame) &&
        workGraphPanelDockedRef.current &&
        workGraphRefreshTimerRef.current === null
      ) {
        workGraphRefreshTimerRef.current = window.setTimeout(() => {
          workGraphRefreshTimerRef.current = null;
          void refreshWorkGraphDataRef.current().catch(() => {});
        }, 250);
      }
    };

    let stopped = false;
    let unsubscribe: (() => void) | null = null;
    const subscriptionLifetime = new AbortController();

    void consoleController.timeline.subscribeWithBackfill(
      {
        limit: 200,
        signal: subscriptionLifetime.signal,
        onTransportState: (state) => { if (!stopped) setTransportState(state); },
      },
      (frame) => {
        if (!stopped) handleLiveFrame(frame.value);
      },
      () => {
        if (stopped) return;
        // The stream resumed past a gap it could not replay; docked
        // identities repair their own logs from their cursors, once.
        for (const identity of new Set(dockedChatIdentitiesRef.current)) {
          void repairIdentityAfterReplayGapRef.current(identity);
        }
      },
    )
      .then((nextUnsubscribe) => {
        if (stopped) {
          nextUnsubscribe();
        } else {
          unsubscribe = nextUnsubscribe;
        }
      })
      .catch((error) => {
        if (!stopped) setTransportState({ phase: "stopped", stale: true, freshness: "unknown", error });
      });

    return () => {
      stopped = true;
      subscriptionLifetime.abort();
      unsubscribe?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [consoleController, consoleTransport, sidebarStorageNamespace, transportRetry]);

  // Timer cleanup on unmount
  React.useEffect(() => {
    return () => {
      for (const timer of Object.values(phaseTimerByKey.current))
        window.clearTimeout(timer);
      for (const timer of Object.values(refreshTimersRef.current))
        window.clearTimeout(timer);
      if (experienceTimerRef.current !== null)
        window.clearTimeout(experienceTimerRef.current);
      if (memoryRefreshTimerRef.current !== null)
        window.clearTimeout(memoryRefreshTimerRef.current);
      if (workGraphRefreshTimerRef.current !== null)
        window.clearTimeout(workGraphRefreshTimerRef.current);
    };
  }, []);

  // =========================================================================
  // AGENT SELECTION
  // =========================================================================

  function openAgentChat(
    agent: ConsoleAgent,
    intent:
      | "replace_focused"
      | "new_tab"
      | "split_right"
      | "split_down" = "replace_focused",
  ) {
    const target = buildDockTarget(agent);
    void refreshIdentityTimelineNow(target.identity || target.memberId).catch(
      () => {},
    );
    dock.openTarget(target, intent);
  }

  function openDockTarget(
    target: MetaDockTarget,
    intent:
      | "replace_focused"
      | "new_tab"
      | "split_right"
      | "split_down" = "replace_focused",
  ) {
    if (target.kind === "agent-chat") {
      void refreshIdentityTimelineNow(target.identity || target.memberId).catch(
        () => {},
      );
    }
    dock.openTarget(target, intent);
  }

  function onSelectAgent(
    _block: unknown,
    _section: unknown,
    item: { id: string },
  ) {
    const agent = agents.find((c) => c.member_id === item.id);
    if (agent) openAgentChat(agent);
  }

  // =========================================================================
  // SEND MESSAGE — optimistic + interaction_id reconciliation
  // =========================================================================

  /// Inner "actually fire the message at the wire" step. Used both by
  /// `onSendMessage` (idle-bypass path), by the stack auto-drain hook,
  /// and by the Steer button (which passes `handlingMode = "steer"`).
  /// Marks the identity as busy optimistically so a Steer click that
  /// races with the wire response doesn't accidentally bypass the
  /// stack on the very next Send.
  async function submitMessageNow(
    panelId: string,
    target: MetaDockTarget,
    text: string,
    handlingMode: "queue" | "steer",
    attachments: File[] = [],
    pendingAttempt?: PendingItem,
    dispatchController = sendControllerRef.current,
  ): Promise<boolean> {
    if (target.kind !== "agent-chat") return false;
    if (!lifetimeRef.current.active || consoleReadOnlyRef.current || dispatchController !== sendControllerRef.current) return false;
    const panelKey = buildPanelConversationKey(panelId, target);
    const identity = target.identity || target.memberId;
    const attemptScope = sendScopeRef.current;
    const envelope: ConsoleFrozenSendEnvelope | undefined = pendingAttempt?.envelopeJson ? JSON.parse(pendingAttempt.envelopeJson) : undefined;

    const optimisticObjectUrls = attachments.map((file) =>
      URL.createObjectURL(file),
    );
    const userEntry = createUserEntry(
      text,
      attachments.map((file, index) => ({
        src: optimisticObjectUrls[index] || "",
        mediaType: file.type || "application/octet-stream",
        alt: file.name,
      })),
    );
    setSendingPanels((c) => new Set(c).add(panelKey));
    const log = getOrCreateLog(identity);
    optimisticUserByPanelKeyRef.current[panelKey] = {
      interactionId: "",
      entry: userEntry,
      sentAtMs: Date.now(),
      objectUrls: optimisticObjectUrls,
    };
    // Use commitPanelPhase (not bare phaseRef assignment) so the
    // value/since bookkeeping is consistent with what
    // `updatePanelPhaseFromFrame` reads — otherwise the next text_delta's
    // "elapsedMs since waiting" check is computed against `since=0`,
    // which we want anyway, but `currentPhase` would read undefined.
    commitPhaseForIdentity(identity, "waiting");
    identityBusyRef.current[identity] = true;
    // The saved attempt lives in this namespace even if the scope or the
    // console lifetime ends before the request settles.
    const attemptNamespace = persistentSendScopeRef.current;
    const optimisticTopologyFrameId = `optimistic-topology:${identity}:${Date.now()}`;
    commitLiveFrames([{
      id: optimisticTopologyFrameId,
      event: "interaction_started",
      identity,
      interactionId: "",
      timestampMs: Date.now(),
      data: {
        origin: `console:${panelId}`,
        handling_mode: handlingMode,
      },
    }, ...liveFramesRef.current].slice(0, 300));
    forceRender();

    try {
      const workbenchTarget = migrateConsoleWorkbenchTarget(target);
      if (!workbenchTarget) {
        throw new Error("console send requires an identity-addressed target");
      }
      const result = (await dispatchController.commands.sendMessage(
        workbenchTarget,
        {
          content: envelope?.content ?? text,
          origin: envelope?.origin ?? `console:${panelId}`,
          idempotencyKey: envelope?.idempotency_key ?? createIdempotencyKey(),
          handlingMode: envelope?.handling_mode ?? handlingMode,
          attachments,
        },
      )).accepted.value;
      if (!result.interaction_id?.trim() || !result.identity?.trim()) throw new Error("Server response did not prove acceptance for this destination.");
      if (!lifetimeRef.current.active || attemptScope !== sendScopeRef.current || dispatchController !== sendControllerRef.current) {
        // The receipt is still proof for the saved attempt it answers:
        // settle it in its own namespace so no later view shows it pending.
        if (pendingAttempt && attemptNamespace) {
          await settleDetachedConsoleSendAttempt(attemptNamespace, identity, pendingAttempt.id, (item) =>
            finishConsoleSendAttempt(item, { state: "accepted", interactionId: result.interaction_id, inputFrameId: result.input_frame_id }), true);
        }
        return false;
      }
      if (pendingAttempt) {
        // Save acceptance before removing the row; if storage fails, retain the attempt.
        if (await setPendingStack(identity, (previous) => previous.map((item) => item.id === pendingAttempt.id ? finishConsoleSendAttempt(item, { state: "accepted", interactionId: result.interaction_id, inputFrameId: result.input_frame_id }) : item))) {
          await setPendingStack(identity, (previous) => previous.filter((item) => item.id !== pendingAttempt.id));
        }
      }
      if (result.input_frame_id) setSubmittedFrames((current) => ({ ...current, [scopedDraftKey(panelKey)]: result.input_frame_id! }));
      const optimisticUser = optimisticUserByPanelKeyRef.current[panelKey];
      if (optimisticUser) {
        optimisticUser.interactionId = result.interaction_id;
        // The interaction_started frame may have arrived between
        // the send and the RPC response — reconcile retroactively.
        const matched = log.events.some(
          (f) =>
            (f.event === "interaction_started" ||
              f.event === "user_input" ||
              f.event === "run_started") &&
            f.interactionId === result.interaction_id,
        );
        if (matched) {
          optimisticUser.objectUrls?.forEach((url) =>
            URL.revokeObjectURL(url),
          );
          delete optimisticUserByPanelKeyRef.current[panelKey];
        }
      }
      if (!pendingStorageErrorRef.current[identity]) setActionError("");
      return true;
    } catch (submitError) {
      const failure: ConsoleSendFailure = submitError instanceof ConsoleCapabilityUnavailableError
        ? { state: "definitely-rejected", kind: "capability_unavailable", message: `${errorMessage(submitError)}. Nothing was sent.` }
        : classifyConsoleSendFailure(submitError);
      // The console lifetime ended (a transport or base-URL change remounts
      // it) or another principal's scope now owns the view: neither may
      // touch the current view's rows. The saved attempt still belongs to
      // its own namespace, so settle it there with the typed failure rather
      // than leaving it "Awaiting acceptance" behind a dead request.
      // Remove this send's optimistic marks only while it owns the active
      // view. A failed send does not finish an existing owner interaction;
      // derive busy state and phase from the current owner events, including
      // any terminal that arrived while this request was in flight.
      if (lifetimeRef.current.active && attemptScope === sendScopeRef.current
          && dispatchController === sendControllerRef.current) {
        optimisticUserByPanelKeyRef.current[panelKey]?.objectUrls?.forEach(
          (url) => URL.revokeObjectURL(url),
        );
        delete optimisticUserByPanelKeyRef.current[panelKey];
        commitLiveFrames(liveFramesRef.current.filter((frame) => frame.id !== optimisticTopologyFrameId));
        recomputeBusyStateFromLog(identity);
        recomputePhaseForIdentity(identity);
      }
      if (!lifetimeRef.current.active || attemptScope !== sendScopeRef.current) {
        if (pendingAttempt && attemptNamespace) {
          await settleDetachedConsoleSendAttempt(attemptNamespace, identity, pendingAttempt.id, (item) =>
            finishConsoleSendAttempt(item, { state: failure.state, error: failure.message, kind: failure.kind }), false);
        }
        if (lifetimeRef.current.active) forceRender();
        return false;
      }
      if (pendingAttempt) {
        // The saved row owns recovery instructions. Persistence failures retain
        // their own banner instead of being overwritten by the transport error.
        await setPendingStack(identity, (previous) => previous.map((item) => item.id === pendingAttempt.id ? finishConsoleSendAttempt(item, { state: failure.state, error: failure.message, kind: failure.kind }) : item));
      } else {
        // Attachment sends are not queued rows: the composer keeps the text
        // and files, and the banner names the typed reason.
        setActionError(failure.message);
      }
      forceRender();
      return false;
    } finally {
      if (lifetimeRef.current.active && attemptScope === sendScopeRef.current) setSendingPanels((c) => {
        const n = new Set(c);
        n.delete(panelKey);
        return n;
      });
    }
  }

  async function onSendMessage(
    panelId: string,
    target: MetaDockTarget | null,
    attachments: File[] = [],
    composerText?: string,
  ): Promise<boolean> {
    if (!target || target.kind !== "agent-chat") return false;
    if (!lifetimeRef.current.active || consoleReadOnly) return false;
    const panelKey = buildPanelConversationKey(panelId, target);
    const identity = target.identity || target.memberId;
    // The pane owns the live composer value and hands it over at submit;
    // the persisted copy is only a fallback for callers without one.
    const draftKey = scopedDraftKey(panelKey);
    const rawDraft = composerText ?? (draftByKey[draftKey] || "");
    const text = rawDraft;
    if (!text.trim() && attachments.length === 0) return false;

    const stack = getPendingStack(identity);
    const visiblePhase =
      phaseValueByKey.current[panelKey] ?? phaseRef.current[panelKey] ?? null;
    const agentPhase =
      agentsRef.current.find((candidate) =>
        [candidate.identity, candidate.member_id, candidate.agent_id].includes(
          identity,
        ),
      )?.response_phase ?? null;
    const shouldQueue =
      isIdentityBusy(identity) ||
      visiblePhase !== null ||
      agentPhase !== null ||
      stack.length > 0;

    const contexts = contextDrafts[draftKey] ?? storedComposerDraft(identity, panelKey).contexts;
    const submittedScope = sendScopeRef.current;
    const submittedController = sendControllerRef.current;
    const clearSubmittedContexts = () => {
      if (!lifetimeRef.current.active || submittedScope !== sendScopeRef.current || submittedController !== sendControllerRef.current) return;
      // ChatPane owns the live text, including edits made while this send
      // waited for storage or the network. Remove only the submitted quotes.
      const latest = storedComposerDraft(identity, panelKey);
      const submittedIds = new Set(contexts.map((context) => context.id));
      const remaining = latest.contexts.filter((context) => !submittedIds.has(context.id));
      setContextDrafts((current) => ({ ...current, [draftKey]: remaining }));
      persistComposerDraft(identity, panelKey, latest.text, remaining);
    };
    if (attachments.length > 0) {
      if (contexts.length) {
        setActionError("Send quoted context separately from file attachments.");
        return false;
      }
      const sent = await submitMessageNow(panelId, target, text, "queue", attachments);
      if (sent) clearSubmittedContexts();
      return sent;
    }
    let item: PendingItem;
    try {
      item = createConsoleSendAttempt({
        id: `pmsg-${createIdempotencyKey()}`, scope: sendScopeRef.current,
        destination: identity, origin: `console:${panelId}`, idempotencyKey: createIdempotencyKey(),
        text, contexts, now: Date.now(),
      });
    } catch (error) {
      setActionError(errorMessage(error));
      return false;
    }
    // A row sent right away stays out of the pending stack while its send is
    // in flight (PendingStack's ACCEPTANCE_NOTICE_GRACE_MS); name it before
    // the saved draft first renders.
    if (!shouldQueue) directSendIdsRef.current.add(item.id);
    if (!await setPendingStack(identity, (previous) => [...previous, item])) {
      directSendIdsRef.current.delete(item.id);
      return false;
    }
    if (!lifetimeRef.current.active || submittedScope !== sendScopeRef.current || submittedController !== sendControllerRef.current) {
      directSendIdsRef.current.delete(item.id);
      return false;
    }
    clearSubmittedContexts();
    if (!shouldQueue) {
      void dispatchPendingAttempt(identity, item.id, "queue").finally(() => directSendIdsRef.current.delete(item.id));
    }
    // This acknowledges local persistence only. submittedRowId is set on server acceptance.
    return true;
  }

  // Drafts are dispatched only after persisting their frozen attempt.
  const reducedMotion =
    typeof window !== "undefined"
      ? (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ??
        false)
      : false;
  const directSendIdsRef = React.useRef(new Set<string>());
  const pendingDrainOwnerRef = React.useRef(
    `tab-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
  );

  function findChatTargetFor(
    identity: string,
  ): { panelId: string; target: MetaDockTarget } | null {
    // This is also called from the long-lived SSE subscription closure via
    // maybeDrainHead(); read the dock ref so pending queue auto-drain sees
    // panels opened after the first render.
    for (const panel of dockRef.current.viewState.panels) {
      const t = panel.target as MetaDockTarget | null;
      if (!t || t.kind !== "agent-chat") continue;
      if ((t.identity || t.memberId) === identity) {
        return { panelId: panel.id, target: t };
      }
    }
    return null;
  }

  async function dispatchPendingAttempt(identity: string, id: string, handlingMode: "queue" | "steer", resend = false) {
    const generation = lifetimeRef.current.generation;
    const scope = sendScopeRef.current;
    const namespace = persistentSendScopeRef.current;
    const dispatchController = sendControllerRef.current;
    const freeze = (): { attempting: PendingItem; target: { panelId: string; target: MetaDockTarget } } | null => {
      if (!lifetimeRef.current.active || generation !== lifetimeRef.current.generation || scope !== sendScopeRef.current || dispatchController !== sendControllerRef.current || consoleReadOnlyRef.current) return null;
      if (namespace) pendingStackRef.current[identity] = loadPendingStack(identity);
      const item = getPendingStack(identity).find((candidate) => candidate.id === id);
      const target = findChatTargetFor(identity);
      const resendable = resend && (item?.state === "definitely-rejected" || (item?.state === "outcome-unknown" && resendUncertainRef.current));
      if (!item || (item.state !== "draft" && !resendable) || item.scope !== scope || !target) return null;
      let attempting: PendingItem;
      try {
        attempting = { ...beginConsoleSendAttempt(item, { owner: pendingDrainOwnerRef.current, now: Date.now(), handlingMode, resend }), checkResult: undefined };
      } catch (error) { setActionError(errorMessage(error)); return null; }
      if (!commitPendingStack(identity, (previous) => previous.map((candidate) => candidate.id === id ? attempting : candidate))) return null;
      return { attempting, target };
    };
    const frozen = namespace ? await withConsoleSendStorageLock(consoleSendStorageKey(namespace, identity), freeze)
      .catch((error) => { setActionError(errorMessage(error)); return null; }) : freeze();
    if (frozen && lifetimeRef.current.active && generation === lifetimeRef.current.generation && scope === sendScopeRef.current && dispatchController === sendControllerRef.current) {
      await submitMessageNow(frozen.target.panelId, frozen.target.target, frozen.attempting.text, handlingMode, [], frozen.attempting, dispatchController);
    }
  }

  function onStackSteer(identity: string, id: string) {
    if (!consoleReadOnlyRef.current) void dispatchPendingAttempt(identity, id, "steer");
  }

  function pendingRowNames(identity: string, destination: string) {
    const agent = agentsRef.current.find((candidate) => [candidate.identity, candidate.member_id, candidate.agent_id].includes(destination))
      ?? agentsRef.current.find((candidate) => [candidate.identity, candidate.member_id, candidate.agent_id].includes(identity));
    const host = experience?.console_config?.brand?.label?.trim();
    return { agent: agent?.label || destination, host: host || undefined };
  }
  function setPendingCheck(identity: string, id: string, view: PendingCheckView | null) {
    setPendingChecks((current) => {
      const rows = { ...(current[identity] ?? {}) };
      if (view) rows[id] = view; else delete rows[id];
      return { ...current, [identity]: rows };
    });
  }

  /// Explicit Check: look for this exact message in the owner's recent
  /// timeline. Every path ends visibly: found clears the row with a
  /// "Delivered at" notice; not found and failures land on the row as plain
  /// text and leave its state (and saved message) unchanged. A row that was
  /// discarded or whose console scope changed is gone, so nothing is left to
  /// answer on.
  async function onStackReconcile(identity: string, id: string) {
    const scope = sendScopeRef.current;
    const generation = lifetimeRef.current.generation;
    const controller = sendControllerRef.current;
    const active = () => lifetimeRef.current.active && generation === lifetimeRef.current.generation
      && scope === sendScopeRef.current && controller === sendControllerRef.current;
    const original = getPendingStack(identity).find((candidate) => candidate.id === id);
    if (!original) return;
    const names = pendingRowNames(identity, original.destination);
    const answer = (text: string) => { if (active()) setPendingCheck(identity, id, { phase: "result", text }); };
    if (!original.envelopeJson) { answer("Couldn't check: this message was never sent."); return; }
    setPendingCheck(identity, id, { phase: "checking" });
    let page: ConsoleTimelinePage;
    let canonicalIdentity: string;
    try {
      const inspection = await inspectIdentityViaHeadless(original.destination);
      if (!active()) return;
      const record = inspection && typeof inspection === "object" ? inspection as Record<string, unknown> : null;
      const owner = record?.identity && typeof record.identity === "object"
        ? record.identity as Record<string, unknown> : record;
      if (typeof owner?.identity !== "string" || !owner.identity.trim()) {
        answer(`Couldn't check: ${names.agent} couldn't be found.`);
        return;
      }
      canonicalIdentity = owner.identity;
      // The inspection supplies the alias correspondence. Timeline stores
      // filter exact canonical identities; querying the alias cannot find it.
      // Defer both the log merge and its cursor until this check still owns
      // the current scope and saved attempt when the query returns.
      page = (await consoleController.timeline.query({ identity: canonicalIdentity, mode: "recent", limit: 200 })).value;
    } catch (error) {
      answer(describeConsoleCheckFailure(error, names));
      return;
    }
    if (!active()) return;
    const item = getPendingStack(identity).find((candidate) => candidate.id === id);
    if (!item) return;
    if (item.envelopeJson !== original.envelopeJson) { answer("This message changed while it was being checked. Check again."); return; }
    const resolution = { requestedIdentity: original.destination, canonicalIdentity };
    // The latest page can omit an older receipt already loaded from the
    // authorized stream or history. Search only the freshly resolved owner's
    // merged log, after its current query succeeded and the scope stayed live.
    const frames = [...page.frames, ...(identityLogRef.current[canonicalIdentity]?.events ?? [])];
    const receipt = frames.find((frame) => reconcileConsoleSendReceipt(item, frame, resolution));
    const accepted = receipt ? reconcileConsoleSendReceipt(item, receipt, resolution) : null;
    const logChanged = reconcileServerLog(canonicalIdentity, page.frames, page.available);
    const metadataChanged = noteIdentityTimelinePage(canonicalIdentity, page, { mode: "recent" });
    if (logChanged || metadataChanged) forceRender();
    if (!accepted) { answer(consoleCheckNotFound(names)); return; }
    const delivered = consoleCheckDelivered((receipt as { timestampMs?: number }).timestampMs);
    if (await setPendingStack(identity, (previous) => previous.map((candidate) => candidate.id === id ? finishConsoleSendAttempt(candidate, { state: "accepted", ...accepted.accepted! }) : candidate))
      && await setPendingStack(identity, (previous) => previous.filter((candidate) => candidate.id !== id))) {
      if (!active()) return;
      setPendingCheck(identity, id, null);
      setDeliveredNotices((current) => ({ ...current, [identity]: [...(current[identity] ?? []).filter((notice) => notice.id !== id), { id, text: delivered }] }));
      return;
    }
    answer(`${delivered} The saved copy couldn't be removed; discard it.`);
  }
  function updatePendingContexts(identity: string, id: string, update: (contexts: ConsoleContextRecord[]) => ConsoleContextRecord[]) {
    setPendingStack(identity, (previous) => previous.map((item) => item.id === id && item.state === "draft" ? { ...item, contexts: update(item.contexts) } : item));
  }
  async function editPendingContext(identity: string, id: string, contextId: string, quote: string) {
    const saved = await setPendingStack(identity, (previous) => {
      const item = previous.find(candidate => candidate.id === id);
      if (!item || item.state !== "draft" || item.envelopeJson) throw new Error("This message is no longer an editable queued draft.");
      const contexts = editConsoleContextQuote(item.contexts, contextId, quote);
      return previous.map(candidate => candidate.id === id ? { ...candidate, contexts } : candidate);
    });
    if (!saved) throw new Error("The quote edit was not saved. Keep this draft and try again.");
  }
  function reorderContexts(contexts: ConsoleContextRecord[], id: string, direction: "up" | "down") {
    const index = contexts.findIndex((record) => record.id === id);
    const to = index + (direction === "up" ? -1 : 1);
    if (index < 0 || to < 0 || to >= contexts.length) return contexts;
    const next = contexts.slice();
    [next[index], next[to]] = [next[to], next[index]];
    return next;
  }
  function onStackTrash(identity: string, id: string) {
    // Explicit discard is permitted even when acceptance is unknown.
    setPendingStack(identity, (previous) => previous.filter((item) => item.id !== id));
  }
  function onStackEdit(identity: string, id: string) {
    setPendingStack(identity, (previous) => previous.map((item) => ({ ...item, editing: item.id === id && item.state === "draft" })));
  }
  function onStackCommitEdit(identity: string, id: string, text: string) {
    if (!text.trim()) return;
    setPendingStack(identity, (previous) => previous.map((item) => item.id === id && item.state === "draft" ? { ...item, text, editing: false } : item));
  }
  function onStackCancelEdit(identity: string, id: string) {
    setPendingStack(identity, (previous) => previous.map((item) => item.id === id ? { ...item, editing: false } : item));
  }
  function onStackReorder(identity: string, dragId: string, dropId: string, where: "above" | "below") {
    setPendingStack(identity, (previous) => {
      const from = previous.findIndex((item) => item.id === dragId && item.state === "draft");
      if (from < 0 || !previous.some((item) => item.id === dropId)) return previous;
      const next = previous.slice();
      const [item] = next.splice(from, 1);
      next.splice(next.findIndex((item) => item.id === dropId) + (where === "below" ? 1 : 0), 0, item);
      return next;
    });
  }
  function onStackClearAll(identity: string) { setPendingStack(identity, () => []); }
  function onStackToggleExpand(identity: string, id: string) {
    setPendingStack(identity, (previous) => previous.map((item) => item.id === id ? { ...item, expanded: !item.expanded } : item));
  }
  React.useEffect(() => {
    // Resume persisted drafts after the authoritative initial history settles,
    // including when a terminal arrived before acceptance removed the prior head.
    for (const identity of Object.keys(pendingStackRef.current)) {
      if (getOrCreateLog(identity).hasServerLog === null) continue;
      maybeDrainHead(identity);
    }
  });

  function maybeDrainHead(identity: string) {
    if (consoleReadOnlyRef.current || isIdentityBusy(identity)) return;
    const ownerPhase = agentsRef.current.find((agent) => [agent.identity, agent.member_id, agent.agent_id].includes(identity))?.response_phase;
    if (ownerPhase) return;
    const head = getPendingStack(identity)[0];
    // An unknown/in-flight head blocks automatic progress until explicit reconciliation.
    if (head?.state === "draft" && !head.editing) {
      const target = findChatTargetFor(identity);
      const key = `${sendScopeRef.current}:${head.id}`;
      if (!target) {
        if (!autoDrainRequestedRef.current.get(key)?.inFlight) autoDrainRequestedRef.current.delete(key);
        return;
      }
      const tokenFor = () => JSON.stringify([findChatTargetFor(identity)?.panelId ?? null, sendRetryEpochRef.current, Boolean(navigator.locks || globalThis.indexedDB), head.text, head.contexts]);
      const token = tokenFor();
      const previous = autoDrainRequestedRef.current.get(key);
      if (previous?.inFlight || previous?.token === token) return;
      const request = { inFlight: true, token };
      autoDrainRequestedRef.current.set(key, request);
      void dispatchPendingAttempt(identity, head.id, "queue").finally(() => {
        if (autoDrainRequestedRef.current.get(key) !== request) return;
        const current = getPendingStack(identity).find((item) => item.id === head.id);
        if (current?.state === "draft") autoDrainRequestedRef.current.set(key, { inFlight: false, token: tokenFor() });
        else autoDrainRequestedRef.current.delete(key);
      });
    }
  }

  async function importLegacyPending(identity: string) {
    const namespace = persistentSendScopeRef.current;
    const storage = browserLocalStorage();
    if (!namespace || !storage) return;
    try {
      if (consoleLegacyQueueImported(storage, namespace, identity)) return;
      const legacy = readLegacyConsoleQueue(storage, identity);
      const predecessor = readPredecessorConsoleQueue(storage, namespace, identity);
      if (predecessor.kind === "blocked") throw new Error(predecessor.reason);
      const imported = [...legacy.map((item) => createConsoleSendAttempt({
        id: `legacy:${item.id}`, scope: namespace, destination: identity, origin: "console:legacy-import",
        idempotencyKey: createIdempotencyKey(), text: item.text, now: item.addedAt,
      })), ...predecessor.attempts];
      await setPendingStack(identity, (previous) => [...previous, ...imported.filter((item) => !previous.some((old) => old.id === item.id))], true);
      // Original legacy and predecessor bytes are preserved, including after a successful import.
    } catch (error) { setActionError(errorMessage(error)); }
  }

  // =========================================================================
  // LIFECYCLE ACTIONS
  // =========================================================================

  async function onLifecycleAction(
    identity: string,
    method: "meta/retire" | "meta/respawn" | "meta/reset",
  ) {
    if (consoleReadOnly) return;
    const command =
      method === "meta/retire"
        ? CONSOLE_COMMAND_NAMES.retireIdentity
        : method === "meta/respawn"
          ? CONSOLE_COMMAND_NAMES.respawnIdentity
          : CONSOLE_COMMAND_NAMES.resetIdentity;
    try {
      await executeHeadlessCommand(command, identityWorkbenchTarget(identity, "chat"), {
        identity,
      });
      setActionError("");
    } catch (lifecycleError) {
      // Capability/grant gates and RPC failures surface in the action-error
      // banner instead of escaping as an unhandled rejection (the callers all
      // `void` this promise).
      setActionError(errorMessage(lifecycleError));
      return;
    }
    const nextAgents = await loadExperience();
    if (method !== "meta/retire") return;
    if (
      nextAgents.some(
        (agent) => agent.identity === identity || agent.member_id === identity,
      )
    )
      return;
    const fallback =
      nextAgents.find(
        (agent) => agent.addressable || agent.affordances?.can_send_message,
      ) || nextAgents[0];
    if (fallback) {
      openAgentChat(fallback, "replace_focused");
    } else {
      dock.openTarget(buildControlTarget("roster"), "replace_focused");
    }
  }

  async function onStopRun(identity: string, runId: string) {
    if (consoleReadOnly) return;
    try {
      const result = await executeHeadlessCommand(
        CONSOLE_COMMAND_NAMES.stopMemberRun,
        identityWorkbenchTarget(identity, "chat"),
        { member_id: identity, run_id: runId, reason: "Stopped from the console" },
      );
      const notice = describeRunStopReceipt(parseRunStopResult(result));
      setRunStopNotices((current) => ({ ...current, [identity]: notice }));
      setActionError("");
    } catch (stopError) {
      setActionError(errorMessage(stopError));
    }
  }

  async function onGatingDecision(
    pendingId: string,
    decision: "approve" | "reject" | "escalate",
  ) {
    await approvalResourceRef.current?.decide(pendingId, decision);
    if (dock.viewState.panels.some(panel => panel.target?.kind === "gating" || panel.target?.kind === "gates")) {
      await refreshPanelData().catch(() => {});
    }
  }

  function upsertTopologyOperation(receipt: TopologyOperationReceipt) {
    setTopologyOperations((current) => mergeTopologyOperationReceipt(current, receipt));
  }

  function executeTopologyRpc(
    operation: ConsoleTopologyRpcOperation,
    params: Record<string, unknown>,
  ): Promise<unknown> {
    const command = operation === "plan"
      ? CONSOLE_COMMAND_NAMES.topologyPlan
      : operation === "apply"
        ? CONSOLE_COMMAND_NAMES.topologyApply
        : CONSOLE_COMMAND_NAMES.topologyOperationGet;
    return executeHeadlessCommand(command, controlWorkbenchTarget("topology"), params);
  }

  async function refreshTopologySurfaces() {
    await Promise.all([
      refreshTopologyData(),
      loadExperience(),
    ]);
  }

  async function onTopologyMutation(intent: TopologyMutationIntent) {
    const idempotencyKey = createConsoleId("topology");
    const request = createConsoleTopologyMutationRequest(intent, idempotencyKey);
    const pending = pendingTopologyReceipt(request.intent, request.idempotencyKey);
    upsertTopologyOperation(pending);
    setActionError("");
    const attempt = await executeConsoleTopologyMutation(request, executeTopologyRpc);
    upsertTopologyOperation(attempt.receipt);
    setActionError(attempt.error || "");
    try {
      await refreshTopologySurfaces();
    } catch (refreshError) {
      if (!attempt.error) setActionError(errorMessage(refreshError));
    }
  }

  async function onRetryTopologyOperation(receipt: TopologyOperationReceipt) {
    if (receipt.retryMode === "revision_rebase") {
      if (!receipt.edge || !normalizedTopology) {
        setActionError("Refresh topology before rebasing this operation.");
        return;
      }
      const rebased = topologyMutationIntent(
        normalizedTopology.management,
        receipt.action,
        receipt.edge,
        "host_action",
      );
      if (!rebased) {
        setActionError("Meta no longer permits this topology change.");
        return;
      }
      await onTopologyMutation({
        ...rebased,
        reason: receipt.request?.reason || rebased.reason,
      });
      return;
    }
    const attempt = await resolveAmbiguousConsoleTopologyMutation(receipt, executeTopologyRpc);
    upsertTopologyOperation(attempt.receipt);
    setActionError(attempt.error || "");
    try {
      await refreshTopologySurfaces();
    } catch (refreshError) {
      if (!attempt.error) setActionError(errorMessage(refreshError));
    }
  }

  // Application extension state stays scoped to this console authority.
  const panelState = useConsolePanels(suppliedPanels, experience?.console_config?.panel_modules, baseUrl);
  const customPanels = panelState.panels;
  const panelDockRef = React.useRef(dock.openTarget);
  panelDockRef.current = dock.openTarget;
  const panelVisibleIdentities = React.useMemo(() => agents.flatMap(agent => [agent.identity, agent.member_id, agent.agent_id]).filter((id): id is string => Boolean(id)), [agents]);
  const panelSelectionRef = React.useRef<string | null>(null);
  if (dock.focusedTarget?.kind === "agent-chat" && panelVisibleIdentities.includes(dock.focusedTarget.identity)) {
    panelSelectionRef.current = dock.focusedTarget.identity;
  } else if (panelSelectionRef.current && !panelVisibleIdentities.includes(panelSelectionRef.current)) {
    panelSelectionRef.current = null;
  }
  const selectedPanelIdentity = panelSelectionRef.current;
  const panelSelection = React.useMemo(() => selectedPanelIdentity
    ? { scopeKey: sendScope, identity: selectedPanelIdentity } : null, [sendScope, selectedPanelIdentity]);
  const panelAuthority = React.useMemo(() => ({ key: sendScope, runtimeId: experience?.runtime_id }), [sendScope, experience?.runtime_id]);
  const panelLifetime = React.useMemo(() => ({ abort: new AbortController() }), [sendScope]);
  React.useLayoutEffect(() => {
    // React's development effect replay starts a fresh authority lifetime.
    if (panelLifetime.abort.signal.aborted) panelLifetime.abort = new AbortController();
    return () => panelLifetime.abort.abort();
  }, [panelLifetime]);
  const panelHostService = React.useMemo<ConsolePanelService>(() => {
    const service = panelService ?? createConsolePanelService(baseUrl);
    return (request, scope, signal) => bindConsolePanelService(service, panelLifetime.abort.signal)(request, scope, signal);
  }, [panelService, baseUrl, panelLifetime]);
  const openCustomPanel = React.useCallback<ConsolePanelContext["openPanel"]>((id, options) => {
    const panel = customPanels.find(panel => panel.id === id);
    if (!panel) return;
    const input = typeof options === "string" ? { intent: options } : options ?? {};
    const conversation = input.conversation === undefined ? panelSelection : input.conversation;
    if (conversation && (conversation.scopeKey !== sendScope || !panelVisibleIdentities.includes(conversation.identity))) throw new Error("Conversation is not visible in this console");
    panelDockRef.current(consoleCustomPanelTarget(panel, {
      ...input, scopeKey: sendScope, conversation,
    }), input.intent);
  }, [customPanels, panelSelection, panelVisibleIdentities, sendScope]);
  const panelContext = React.useMemo<ConsolePanelContext>(() => ({
    baseUrl, readOnly: consoleReadOnly, experience, openPanel: openCustomPanel,
    visibleIdentities: panelVisibleIdentities, authority: panelAuthority, selection: panelSelection, conversation: panelSelection,
    request: (request, signal, conversation = panelSelection) => {
      if (conversation && (conversation.scopeKey !== sendScope || !panelVisibleIdentities.includes(conversation.identity))) return Promise.reject(new Error("Conversation is not visible in this console"));
      return panelHostService(request, { authority: panelAuthority, conversation, readOnly: consoleReadOnlyRef.current }, signal);
    },
  }), [baseUrl, consoleReadOnly, experience, openCustomPanel, panelAuthority, panelSelection, panelHostService, panelVisibleIdentities, sendScope]);
  const panelProvider = React.useMemo(() => ({ panels: panelState.panels, context: panelContext }), [panelState.panels, panelContext]);
  const effectiveMcpAppsHost = React.useMemo(() => {
    if (mcpAppsHost) return mcpAppsHost;
    const sandboxProxyUrl = experience?.console_config?.mcp_apps_sandbox_url;
    if (!sandboxProxyUrl) return undefined;
    try {
      return createNativeMcpAppsHost({ baseUrl, sandboxProxyUrl, authority: sendScope,
        runtimeId: experience?.runtime_id, readOnly: consoleReadOnly,
        requestTimeoutMs: consoleFetchTimeoutMsRef.current });
    } catch {
      // A missing or invalid isolated sandbox retains the ordinary transcript.
      return undefined;
    }
  }, [mcpAppsHost, baseUrl, experience?.console_config?.mcp_apps_sandbox_url, experience?.runtime_id, sendScope, consoleReadOnly]);

  // =========================================================================
  // WORKGRAPH OPERATOR ACTIONS (inline card + panel)
  // =========================================================================

  const canManageWorkGraph =
    experience?.workgraph?.can_manage === true && !consoleReadOnly;

  const runWorkGraphCommand = React.useCallback(
    async (
      command:
        | typeof CONSOLE_COMMAND_NAMES.workgraphClaim
        | typeof CONSOLE_COMMAND_NAMES.workgraphRelease
        | typeof CONSOLE_COMMAND_NAMES.workgraphClose
        | typeof CONSOLE_COMMAND_NAMES.workgraphGoalConfirm
        | typeof CONSOLE_COMMAND_NAMES.workgraphGoalRequestClose
        | typeof CONSOLE_COMMAND_NAMES.workgraphAttentionPause
        | typeof CONSOLE_COMMAND_NAMES.workgraphAttentionResume
        | typeof CONSOLE_COMMAND_NAMES.workgraphAttentionReassign,
      params: Record<string, unknown>,
      cardIdentity?: string,
    ) => {
      if (consoleReadOnlyRef.current) return;
      // Operator RPCs run outside any agent turn, so the server emits no
      // console frames for them. Echo the RPC outcome into the identity log
      // as a client-local synthetic frame: the inline card folds the fresh
      // item/binding revisions (or the failure note) immediately, so
      // consecutive card actions CAS against the updated state instead of
      // conflicting on a stale one.
      const echoResultToCard = (result?: unknown, failureMessage?: string) => {
        if (!cardIdentity) return;
        appendFrame(
          cardIdentity,
          buildWorkGraphOperatorResultFrame({
            method: consoleCommandMethod(command),
            params,
            ...(failureMessage !== undefined
              ? { errorMessage: failureMessage }
              : { result }),
            identity: cardIdentity,
          }),
        );
        forceRender();
      };
      try {
        const result = await executeHeadlessCommand(command, controlWorkbenchTarget("workgraph"), params);
        setActionError("");
        echoResultToCard(result);
      } catch (err) {
        // CAS conflicts and grant gates land in the action-error banner and,
        // through the same synthetic frame, as a failure note on the card.
        const message = errorMessage(err);
        setActionError(message);
        echoResultToCard(undefined, message);
        // A -32042 conflict proves the revision the card folded is stale;
        // without a fresh sighting every next click would resend it forever.
        // Re-read the entity and fold the live state through the same echo
        // path (marked `refresh` so the failure flag survives): the NEXT
        // action CASes against the live revision. Best-effort — the banner
        // and failure note above already surfaced the conflict.
        if (cardIdentity && jsonRpcErrorCode(err) === WORKGRAPH_CONFLICT_CODE) {
          const refresh = workGraphConflictRefreshRequest(params);
          if (refresh) {
            try {
              const fresh = await executeHeadlessCommand(
                refresh.command,
                controlWorkbenchTarget("workgraph"),
                refresh.params,
              );
              appendFrame(
                cardIdentity,
                buildWorkGraphOperatorResultFrame({
                  method: consoleCommandMethod(refresh.command),
                  params: refresh.params,
                  result: fresh,
                  identity: cardIdentity,
                  refresh: true,
                }),
              );
              forceRender();
            } catch {
              // The stale card stays; the conflict is already visible.
            }
          }
        }
      }
      if (workGraphPanelDockedRef.current) {
        await refreshWorkGraphData().catch(() => {});
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [baseUrl, refreshWorkGraphData],
  );

  const runWorkGraphQuery = React.useCallback<WorkGraphCommandRunner>(
    (command, params) =>
      executeHeadlessCommand(command, controlWorkbenchTarget("workgraph"), params),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [baseUrl],
  );

  // Operator handlers shared by the inline card and the docked panel.
  // `cardIdentity` scopes the synthetic result echo to that identity's chat
  // log; the panel passes none (it re-reads the snapshot instead). A payload
  // without a revision means the UI never observed the CAS token — resolve
  // the live one first and, if that fails, surface the banner without
  // sending (a guessed 0 is a guaranteed conflict).
  const makeWorkGraphOperatorHandlers = React.useCallback(
    (cardIdentity?: string) => {
      const dispatch = (
        resolveRevision: () => Promise<number>,
        send: (expectedRevision: number) => Promise<void>,
      ) => {
        void (async () => {
          let expectedRevision: number;
          try {
            expectedRevision = await resolveRevision();
          } catch (err) {
            setActionError(errorMessage(err));
            return;
          }
          await send(expectedRevision);
        })();
      };
      const revisionOr = (
        revision: number | undefined,
        resolve: () => Promise<number>,
      ) => (revision !== undefined ? () => Promise.resolve(revision) : resolve);
      return {
        onClaim: ({ itemId, revision }: { itemId: string; revision?: number }) =>
          dispatch(
            revisionOr(revision, () => resolveWorkGraphItemRevision(runWorkGraphQuery, itemId)),
            (expectedRevision) =>
              runWorkGraphCommand(CONSOLE_COMMAND_NAMES.workgraphClaim, {
                id: itemId,
                expected_revision: expectedRevision,
                owner: {
                  kind: "principal",
                  id: workGraphClaimOwnerId(experience?.access?.subject, DEFAULT_APPROVER_ID),
                },
              }, cardIdentity),
          ),
        onClose: ({ itemId, revision }: { itemId: string; revision?: number }) =>
          dispatch(
            revisionOr(revision, () => resolveWorkGraphItemRevision(runWorkGraphQuery, itemId)),
            (expectedRevision) =>
              runWorkGraphCommand(CONSOLE_COMMAND_NAMES.workgraphClose, {
                id: itemId,
                expected_revision: expectedRevision,
              }, cardIdentity),
          ),
        onGoalConfirm: ({ bindingId, revision }: { bindingId: string; revision?: number }) =>
          dispatch(
            revisionOr(revision, () => resolveWorkGraphGoalItemRevision(runWorkGraphQuery, bindingId)),
            (expectedRevision) =>
              runWorkGraphCommand(CONSOLE_COMMAND_NAMES.workgraphGoalConfirm, {
                binding_id: bindingId,
                expected_revision: expectedRevision,
              }, cardIdentity),
          ),
        onGoalRequestClose: ({ bindingId, revision }: { bindingId: string; revision?: number }) =>
          dispatch(
            revisionOr(revision, () => resolveWorkGraphGoalItemRevision(runWorkGraphQuery, bindingId)),
            (expectedRevision) =>
              runWorkGraphCommand(CONSOLE_COMMAND_NAMES.workgraphGoalRequestClose, {
                binding_id: bindingId,
                expected_revision: expectedRevision,
              }, cardIdentity),
          ),
        onAttentionPause: ({ bindingId, revision }: { bindingId: string; revision?: number }) =>
          dispatch(
            revisionOr(revision, () => resolveWorkGraphBindingRevision(runWorkGraphQuery, bindingId)),
            (expectedRevision) =>
              runWorkGraphCommand(CONSOLE_COMMAND_NAMES.workgraphAttentionPause, {
                binding_id: bindingId,
                expected_revision: expectedRevision,
              }, cardIdentity),
          ),
        onAttentionResume: ({ bindingId, revision }: { bindingId: string; revision?: number }) =>
          dispatch(
            revisionOr(revision, () => resolveWorkGraphBindingRevision(runWorkGraphQuery, bindingId)),
            (expectedRevision) =>
              runWorkGraphCommand(CONSOLE_COMMAND_NAMES.workgraphAttentionResume, {
                binding_id: bindingId,
                expected_revision: expectedRevision,
              }, cardIdentity),
          ),
        onAttentionReassign: ({ bindingId, revision, identity }: { bindingId: string; revision?: number; identity: string }) =>
          dispatch(
            revisionOr(revision, () => resolveWorkGraphBindingRevision(runWorkGraphQuery, bindingId)),
            (expectedRevision) =>
              runWorkGraphCommand(CONSOLE_COMMAND_NAMES.workgraphAttentionReassign, {
                binding_id: bindingId,
                expected_revision: expectedRevision,
                target: { kind: "identity", identity },
              }, cardIdentity),
          ),
      };
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [runWorkGraphCommand, runWorkGraphQuery, experience?.access?.subject],
  );

  // Inline-card affordances: undefined when the caller lacks the manage
  // grant (or the console is read-only) — the card then renders no buttons.
  // Reassign needs a typed-in target identity, so only the panel offers it.
  const workGraphCardActions = React.useCallback(
    (cardIdentity: string): WorkGraphCardActions | undefined => {
      if (!canManageWorkGraph) return undefined;
      const { onAttentionReassign: _panelOnly, ...cardHandlers } =
        makeWorkGraphOperatorHandlers(cardIdentity);
      return cardHandlers;
    },
    [canManageWorkGraph, makeWorkGraphOperatorHandlers],
  );
  // One handler bundle per identity for as long as the factory is stable:
  // MessageRow compares `workGraphActions` by reference, so a fresh bundle
  // per render would re-render every mounted row on every flush.
  const workGraphCardActionsByIdentity = React.useMemo(
    () => new Map<string, WorkGraphCardActions | undefined>(),
    [workGraphCardActions],
  );
  const workGraphCardActionsFor = (cardIdentity: string): WorkGraphCardActions | undefined => {
    if (!workGraphCardActionsByIdentity.has(cardIdentity)) {
      workGraphCardActionsByIdentity.set(cardIdentity, workGraphCardActions(cardIdentity));
    }
    return workGraphCardActionsByIdentity.get(cardIdentity);
  };

  // =========================================================================
  // RESIZE HANDLERS (unchanged)
  // =========================================================================

  const SIDEBAR_MIN = 180,
    SIDEBAR_MAX = 420;
  function handleSidebarResize(event: React.PointerEvent<HTMLDivElement>) {
    event.preventDefault();
    const startX = event.clientX;
    const root = findPaneResizeRoot(event.currentTarget);
    if (!root) return;
    const startWidth =
      parseInt(
        getComputedStyle(root).getPropertyValue(
          "--cc-workbench-sidebar-width",
        ) || "260",
        10,
      ) || 260;
    const handle = event.currentTarget;
    if ("setPointerCapture" in handle)
      handle.setPointerCapture(event.pointerId);
    document.documentElement.setAttribute("data-cc-resizing", "true");
    function onPointerMove(e: PointerEvent) {
      root!.style.setProperty(
        "--cc-workbench-sidebar-width",
        `${Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, startWidth + (e.clientX - startX)))}px`,
      );
    }
    function cleanup() {
      document.documentElement.removeAttribute("data-cc-resizing");
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", cleanup);
      window.removeEventListener("pointercancel", cleanup);
      if (
        "hasPointerCapture" in handle &&
        handle.hasPointerCapture(event.pointerId)
      )
        handle.releasePointerCapture(event.pointerId);
    }
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", cleanup);
    window.addEventListener("pointercancel", cleanup);
  }

  const ACTIVITY_MIN = 200,
    ACTIVITY_MAX = 480;
  function handleActivityResize(event: React.PointerEvent<HTMLDivElement>) {
    event.preventDefault();
    const startX = event.clientX;
    const root = findPaneResizeRoot(event.currentTarget);
    if (!root) return;
    const startWidth =
      parseInt(
        getComputedStyle(root).getPropertyValue(
          "--cc-workbench-activity-width",
        ) || "280",
        10,
      ) || 280;
    const handle = event.currentTarget;
    if ("setPointerCapture" in handle)
      handle.setPointerCapture(event.pointerId);
    document.documentElement.setAttribute("data-cc-resizing", "true");
    function onPointerMove(e: PointerEvent) {
      root!.style.setProperty(
        "--cc-workbench-activity-width",
        `${Math.min(ACTIVITY_MAX, Math.max(ACTIVITY_MIN, startWidth - (e.clientX - startX)))}px`,
      );
    }
    function cleanup() {
      document.documentElement.removeAttribute("data-cc-resizing");
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", cleanup);
      window.removeEventListener("pointercancel", cleanup);
      if (
        "hasPointerCapture" in handle &&
        handle.hasPointerCapture(event.pointerId)
      )
        handle.releasePointerCapture(event.pointerId);
    }
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", cleanup);
    window.addEventListener("pointercancel", cleanup);
  }

  // =========================================================================
  // RENDER GUARDS
  // =========================================================================

  // Stable handlers for the memoised shell components (Sidebar, VoiceBar,
  // SignalsRail): they read the latest closures through refs so their
  // identity never changes and a root render with unchanged data skips them.
  const voiceRef = React.useRef(voice);
  voiceRef.current = voice;
  const closeVoice = React.useCallback(() => void voiceRef.current?.close(), []);
  const toggleVoiceMicrophone = React.useCallback(() => voiceRef.current?.toggleMicrophone(), []);
  const toggleVoiceSpeaker = React.useCallback(() => voiceRef.current?.toggleSpeaker(), []);
  const openAgentChatRef = React.useRef(openAgentChat);
  openAgentChatRef.current = openAgentChat;
  const selectSidebarAgent = React.useCallback(
    (agent: ConsoleAgent) => openAgentChatRef.current(agent),
    [],
  );
  const openSidebarControl = React.useCallback((kind: NavKind) => {
    dockRef.current.openTarget(buildControlTarget(kind), "replace_focused");
  }, []);
  const sidebarButtons = experience?.console_config?.sidebar?.buttons;
  const openOperatorPanel = React.useCallback((buttonId: string) => {
    const button = (sidebarButtons || []).find((candidate) => candidate.id === buttonId);
    if (!button || !operatorPanelPath(button.panel)) return;
    dockRef.current.openTarget(buildOperatorPanelTarget(button), "replace_focused");
  }, [sidebarButtons]);
  const loadMemoryRecordDetailRef = React.useRef(loadMemoryRecordDetail);
  loadMemoryRecordDetailRef.current = loadMemoryRecordDetail;
  // "State here" pivot: a live memory signal opens the Memory panel, and
  // lands on the record's Biography when the frame names one. Offered only
  // when the server-projected experience grants memory.can_read; the
  // affordance must never outrun the nav gate.
  const selectRailFrame = React.useCallback((frame: ConsoleFrame) => {
    if (!frame.event.startsWith("memory.")) return;
    dockRef.current.openTarget(buildControlTarget("memory"), "replace_focused");
    const pivot = memoryFramePivot(frame);
    if (pivot) void loadMemoryRecordDetailRef.current(pivot.realm, pivot.recordId);
  }, []);
  const watchedIdentities = React.useMemo(
    () =>
      new Set(
        agents
          .filter((agent) => agent.watched)
          .map((agent) => agent.identity || agent.member_id)
          .filter((value): value is string => Boolean(value)),
      ),
    [agents],
  );

  if (loading)
    return (
      <div
        data-testid="console-loading"
        aria-live="polite"
        aria-busy="true"
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          gap: "0.6rem",
          minHeight: "100vh",
        }}
      >
        <span className="msg__typing-dots" aria-hidden="true">
          <span /><span /><span />
        </span>
        <span>Loading console…</span>
      </div>
    );
  if (error) return <div data-testid="console-error">{error}</div>;

  // =========================================================================
  // BUILD VIEW STATES
  // =========================================================================

  const focusedMemberId =
    dock.focusedTarget?.kind === "agent-chat"
      ? dock.focusedTarget.memberId
      : selectedRosterMemberId;
  const actionConfig = experience?.console_config?.actions;
  const configuredActionLabels = {
    inspect: actionLabel(actionConfig, "inspect_label", "Details"),
    chat: actionLabel(actionConfig, "chat_label", "Open chat"),
    send: actionLabel(actionConfig, "send_label", "Send"),
    respawn: actionLabel(actionConfig, "respawn_label", "Respawn"),
    retire: actionLabel(actionConfig, "retire_label", "Retire"),
    reset: actionLabel(actionConfig, "reset_label", "Reset"),
  };
  const configuredActionVisibility = {
    inspect: actionVisible(actionConfig, "show_inspect"),
    chat: actionVisible(actionConfig, "show_chat"),
    respawn: actionVisible(actionConfig, "show_respawn"),
    retire: actionVisible(actionConfig, "show_retire"),
    reset: actionVisible(actionConfig, "show_reset"),
  };

  // =========================================================================
  // RENDER: CHAT PANEL — reads from 3 identity-keyed refs
  // =========================================================================

  const voiceBar = (
    <VoiceBar
      state={voiceState}
      sampleWaveform={sampleVoiceWaveform}
      onClose={closeVoice}
      onToggleMicrophone={toggleVoiceMicrophone}
      onToggleSpeaker={toggleVoiceSpeaker}
    />
  );

  function renderChatPanel(panel: {
    id: string;
    target?: MetaDockTarget | null;
  }) {
    const target = panel.target;
    if (!target || target.kind !== "agent-chat") return null;
    const panelKey = buildPanelConversationKey(panel.id, target);
    const identity = target.identity || target.memberId;
    const agent = agents.find((c) => c.member_id === target.memberId) || null;

    // Single adapter pass over the canonical sorted log. No more
    // server/live split, no cross-store dedup, no duplicate grouping.
    // Text deltas are rendered as the interaction streams; the
    // adapter's `streamedText === terminalText` check suppresses the
    // duplicate when text_complete/interaction_complete arrives.
    //
    // Keyed on the identity log's version: the sort and the adapter run
    // only when this identity's frames changed, not on every app render.
    const { sortedFrames, conversationEntries } = derivedTranscriptFor(
      identity,
      panel.id,
      agent,
    );

    // Optimistic user message: rendered until an interaction_started
    // with the matching interaction_id is appended to the log (which
    // clears it via appendFrame). Until then, it sits at the tail of
    // the conversation as a synthetic entry.
    const optimisticUser = optimisticUserMessageForPanel(
      optimisticUserByPanelKeyRef.current,
      panelKey,
      identity,
    );
    const optimisticEntry = optimisticUser ? optimisticUser.entry : null;

    // `conversationEntries` are already in transcript order. Re-sorting
    // rendered entries by createdAt here would lose the adapter's
    // same-turn grouping/tie-break rules.
    const entries = sanitizeConversationEntries(
      appendOptimisticConversationEntry(conversationEntries, optimisticEntry),
    );

    const draftKey = scopedDraftKey(panelKey);
    const draft = draftByKey[draftKey] ?? storedComposerDraft(identity, panelKey).text;
    const quotedContexts = contextDrafts[draftKey] ?? storedComposerDraft(identity, panelKey).contexts;
    const staged = stagedAttachmentsByIdentity[identity] ?? [];
    const identityLog = getOrCreateLog(identity);
    const isSending = sendingPanels.has(panelKey);
    const hasLocalPhase = Object.prototype.hasOwnProperty.call(
      phaseRef.current,
      panelKey,
    );
    const honorLocalPhase = hasLocalPhase && (isSending || optimisticEntry !== null);
    const phase = panelPhaseFor(panelKey, framesAfterIdleWatermark(sortedFrames, identityLog.idleSettledThrough), {
      localPhase: honorLocalPhase ? phaseRef.current[panelKey] ?? null : null,
      hasLocalPhase: honorLocalPhase,
      serverPhase: agent?.response_phase ?? null,
    });
    const canRespawn =
      !consoleReadOnly &&
      configuredActionVisibility.respawn &&
      agent?.affordances?.can_respawn === true;
    const canRetire =
      !consoleReadOnly &&
      configuredActionVisibility.retire &&
      agent?.affordances?.can_retire === true;
    // Offered only while the timeline names the in-flight run, and with the
    // same authority as force-cancel (the gateway authorizes it as retire).
    const activeRunId = activeRunIdFor(identity, identityLog.version, sortedFrames);
    const canStopRun =
      !consoleReadOnly && agent?.affordances?.can_retire === true && activeRunId !== null;

    const stackItems = getPendingStack(identity);
    let hasLegacyQueue = false;
    try {
      const storage = browserLocalStorage();
      const namespace = persistentSendScopeRef.current;
      hasLegacyQueue = Boolean(storage && namespace && !consoleLegacyQueueImported(storage, namespace, identity)
        && (Boolean(storage.getItem(`mobkit-pending-stack:${identity}`)) || readPredecessorConsoleQueue(storage, namespace, identity).attempts.length > 0));
    } catch { /* Queue reader reports preserved invalid bytes separately. */ }
    const agentBusy = isIdentityBusy(identity);
    const stackSlot = <>
      {stackItems.length > 0 ? <small className="queue-storage-note" role="status">{persistentSendScopeRef.current ? "Saved in this browser until sent" : "Not saved: these messages are lost if you reload"}</small> : null}
      {pendingStorageErrorRef.current[identity] && <p role="alert">{pendingStorageErrorRef.current[identity]}</p>}
      {hasLegacyQueue && (
        <div className="queue-import" role="group" aria-label="Older queued messages" data-testid="legacy-queue-import">
          <span className="queue-import__icon" aria-hidden="true"><Icon name="i-clock" /></span>
          <span className="queue-import__text">
            <span className="queue-import__title">Older queued messages</span>
            <span>Resume them with this agent in this account.</span>
          </span>
          <button
            type="button"
            className="queue-import__action"
            aria-label="Import and send older queued messages"
            onClick={() => importLegacyPending(identity)}
          >Import and send</button>
        </div>
      )}
      {stackItems.length > 0 || (deliveredNotices[identity]?.length ?? 0) > 0 ? (
        <PendingStack
          items={stackItems}
          agentLabel={target.title || agent?.label || identity}
          hostLabel={experience?.console_config?.brand?.label?.trim() || undefined}
          resendUncertain={resendUncertain}
          checks={pendingChecks[identity]}
          delivered={deliveredNotices[identity]}
          onDismissDelivered={(itemId) => setDeliveredNotices((current) => ({ ...current, [identity]: (current[identity] ?? []).filter((notice) => notice.id !== itemId) }))}
          directSendIds={directSendIdsRef.current}
          agentBusy={agentBusy}
          reducedMotion={reducedMotion}
          onSteer={(itemId) => onStackSteer(identity, itemId)}
          onRetry={(itemId) => {
            const item = getPendingStack(identity).find((candidate) => candidate.id === itemId);
            if (item?.envelopeJson) void dispatchPendingAttempt(identity, itemId, JSON.parse(item.envelopeJson).handling_mode, true);
          }}
          onReconcile={(itemId) => onStackReconcile(identity, itemId)}
          onRemoveContext={(itemId, contextId) => updatePendingContexts(identity, itemId, (contexts) => contexts.filter((record) => record.id !== contextId))}
          onEditContext={(itemId, contextId, quote) => editPendingContext(identity, itemId, contextId, quote)}
          onReorderContext={(itemId, contextId, direction) => updatePendingContexts(identity, itemId, (contexts) => reorderContexts(contexts, contextId, direction))}
          onTrash={(itemId) => onStackTrash(identity, itemId)}
          onEdit={(itemId) => onStackEdit(identity, itemId)}
          onCommitEdit={(itemId, t) => onStackCommitEdit(identity, itemId, t)}
          onCancelEdit={(itemId) => onStackCancelEdit(identity, itemId)}
          onReorder={(dragId, dropId, where) =>
            onStackReorder(identity, dragId, dropId, where)
          }
          onClearAll={() => onStackClearAll(identity)}
          onToggleExpand={(itemId) => onStackToggleExpand(identity, itemId)}
        />
      ) : null}
    </>;
    const submittedRowId = submittedFrames[draftKey];
    const addQuote = (quote: ConsoleQuoteSelection) => {
      if (sendScope !== sendScopeRef.current) return;
      try {
        const context = createConsoleContextRecord({ id: `quote:${createIdempotencyKey()}`, sourceScope: sendScopeRef.current,
          // The renderer ID names a stable message assembled from potentially many
          // frames. Without canonical frame provenance, no frame-relative range is claimed.
          sourceIdentity: identity, messageId: quote.messageId,
          quote: quote.text, label: target.title || agent?.label || identity });
        const next = [...quotedContexts, context];
        validateConsoleContexts(next);
        setContextDrafts((current) => ({ ...current, [draftKey]: next }));
        persistComposerDraft(identity, panelKey, draft, next);
      } catch (error) { setActionError(errorMessage(error)); }
    };

    return (
      <ChatPane
        agent={agent}
        markdownUrlPolicy={markdownUrlPolicy}
        blobReader={blobReader}
        headerVariant="compact"
        displayLabels={chatDisplayLabels}
        approvalSnapshot={activeApprovals}
        onApprovalDecision={onGatingDecision}
        peerLabels={peerLabels}
        agentLabel={target.title || agent?.label || identity}
        identity={identity}
        key={`${sendScope}:${panel.id}:${identity}`}
        viewportKey={{ authority: sendScope, identity, conversation: identity, pane: panel.id }}
        submittedRowId={submittedRowId}
        onQuoteSelection={addQuote}
        contextSlot={<QuoteContextChips records={quotedContexts} destinationLabel={target.title || agent?.label || identity} onEdit={(id, quote) => {
          if (sendScope !== sendScopeRef.current || !lifetimeRef.current.active) throw new Error("This draft is no longer active.");
          const latest = storedComposerDraft(identity, panelKey);
          const next = editConsoleContextQuote(latest.contexts, id, quote);
          if (!persistComposerDraft(identity, panelKey, latest.text, next)) throw new Error("The quote edit was not saved. Keep this draft and try again.");
          setContextDrafts((current) => ({ ...current, [draftKey]: next }));
        }} onRemove={(id) => {
          if (sendScope !== sendScopeRef.current) return;
          const next = quotedContexts.filter((record) => record.id !== id);
          setContextDrafts((current) => ({ ...current, [draftKey]: next }));
          persistComposerDraft(identity, panelKey, draft, next);
        }} onReorder={(id, direction) => {
          if (sendScope !== sendScopeRef.current) return;
          const next = reorderContexts(quotedContexts, id, direction);
          setContextDrafts((current) => ({ ...current, [draftKey]: next }));
          persistComposerDraft(identity, panelKey, draft, next);
        }} />}
        entries={entries}
        phase={phase}
        isLoadingHistory={Boolean(loadingHistory[identity])}
        draft={draft}
        sending={isSending}
        readOnly={consoleReadOnly}
        accessEnforcing={accessEnforcing}
        staged={staged}
        onDraftChange={(value) => {
          if (sendScope !== sendScopeRef.current) return;
          setDraftByKey((current) => ({ ...current, [draftKey]: value }));
          persistComposerDraft(identity, panelKey, value, storedComposerDraft(identity, panelKey).contexts);
        }}
        onStagedChange={(action) =>
          setStagedAttachmentsForIdentity(identity, action)
        }
        onSend={(attachments, text) => sendScope === sendScopeRef.current ? onSendMessage(panel.id, target, attachments, text) : false}
        onInspect={
          configuredActionVisibility.inspect
            ? () => {
                if (agent) handleShowRosterDetails(agent);
              }
            : undefined
        }
        onRespawn={
          canRespawn
            ? () => void onLifecycleAction(identity, "meta/respawn")
            : undefined
        }
        onRetire={
          canRetire
            ? () => void onLifecycleAction(identity, "meta/retire")
            : undefined
        }
        onStopRun={
          canStopRun && activeRunId
            ? () => void onStopRun(identity, activeRunId)
            : undefined
        }
        runStopNotice={runStopNotices[identity] ?? null}
        inspectLabel={configuredActionLabels.inspect}
        respawnLabel={configuredActionLabels.respawn}
        retireLabel={configuredActionLabels.retire}
        sendLabel={configuredActionLabels.send}
        hasOlderHistory={identityHasOlderHistory(identityLog)}
        loadingOlderHistory={identityLog.olderHistoryLoading === true}
        onLoadOlder={() => void loadOlderIdentityTimeline(identity)}
        stackSlot={stackSlot}
        voiceSlot={dock.viewState.focusedPanelId === panel.id ? voiceBar : null}
        onVoiceToggle={
          voice && voiceReadinessOffersVoice(voiceReadiness[identity]) && agent?.affordances?.can_send_message === true
            ? () => {
                if (voiceState.target?.identity === identity && voiceState.phase !== "idle" && voiceState.phase !== "error") {
                  void voice.close();
                } else {
                  void voice.start({ identity, label: target.title || agent?.label || identity });
                }
              }
            : undefined
        }
        voiceActive={voiceState.target?.identity === identity && voiceState.phase !== "idle" && voiceState.phase !== "error"}
        voiceDisabled={voiceState.phase === "closing"}
        voiceChecking={voiceReadinessPending(voiceReadiness[identity])}
        liveSpeech={
          voiceState.target?.identity === identity && voiceState.phase === "active"
            ? voiceState.liveSpeech
            : undefined
        }
        activeVoiceScope={
          voiceState.target?.identity === identity && voiceState.phase === "active"
            && agent?.session_id && voiceState.activeChannelId
            ? { sessionId: agent.session_id, channelId: voiceState.activeChannelId }
            : null
        }
        workGraphActions={workGraphCardActionsFor(identity)}
      />
    );
  }

  // =========================================================================
  // RENDER: CONTROL PANELS (unchanged)
  // =========================================================================

  function renderInspectPanel(
    target: Extract<MetaDockTarget, { kind: "identity-inspect" }>,
  ) {
    const inspect = inspectByIdentity[target.identity];
    const agent = agents.find(
      (candidate) =>
        candidate.identity === target.identity ||
        candidate.member_id === target.identity,
    );
    const canRespawn =
      !consoleReadOnly &&
      configuredActionVisibility.respawn &&
      agent?.affordances?.can_respawn === true;
    const canRetire =
      !consoleReadOnly &&
      configuredActionVisibility.retire &&
      agent?.affordances?.can_retire === true;
    const canReset =
      !consoleReadOnly &&
      configuredActionVisibility.reset &&
      experience?.runtime_capabilities?.can_retire_members === true;
    return (
      <div
        className="console-panel"
        data-testid={`inspect-panel:${target.identity}`}
      >
        <div className="console-panel__header">
          <h3>{target.identity}</h3>
          <div className="console-panel__actions">
            {canRespawn ? (
              <button
                data-testid={`inspect-action:${target.identity}:respawn`}
                type="button"
                onClick={() =>
                  void onLifecycleAction(target.identity, "meta/respawn")
                }
              >
                {configuredActionLabels.respawn}
              </button>
            ) : null}
            {canReset ? (
              <button
                data-testid={`inspect-action:${target.identity}:reset`}
                type="button"
                onClick={() =>
                  void onLifecycleAction(target.identity, "meta/reset")
                }
              >
                {configuredActionLabels.reset}
              </button>
            ) : null}
            {canRetire ? (
              <button
                data-testid={`inspect-action:${target.identity}:retire`}
                type="button"
                onClick={() =>
                  void onLifecycleAction(target.identity, "meta/retire")
                }
              >
                {configuredActionLabels.retire}
              </button>
            ) : null}
          </div>
        </div>
        {!inspect ? (
          <p>Loading identity details…</p>
        ) : (
          <dl className="console-panel__grid">
            <dt>State</dt>
            <dd data-testid={`inspect-state:${target.identity}`}>
              {identityStateLabel(inspect)}
            </dd>
            {inspect.session_repair ? (
              <>
                <dt>Repair</dt>
                <dd data-testid={`inspect-session-repair:${target.identity}`}>
                  <p>
                    The durable session is intact but refused until it is
                    repaired. Run the diagnose command, then the repair
                    command, then reload the member (meta/reload_member).
                  </p>
                  <code data-testid="inspect-session-repair-diagnose">
                    {inspect.session_repair.diagnose_command}
                  </code>
                  <code data-testid="inspect-session-repair-apply">
                    {inspect.session_repair.apply_command}
                  </code>
                </dd>
              </>
            ) : null}
            <dt>Role</dt>
            <dd>{inspect.role || "n/a"}</dd>
            <dt>Addressability</dt>
            <dd>{inspect.addressability}</dd>
            <dt>Generation</dt>
            <dd>{inspect.continuity?.generation ?? "n/a"}</dd>
            <dt>Session</dt>
            <dd>{inspect.continuity?.session_id || "n/a"}</dd>
            <dt>Runtime</dt>
            <dd>{inspect.continuity?.agent_runtime_id || "n/a"}</dd>
            <dt>Peers</dt>
            <dd>{inspect.topology_peers?.join(", ") || "none"}</dd>
            <dt>Output Preview</dt>
            <dd>{inspect.output_preview || "n/a"}</dd>
          </dl>
        )}
      </div>
    );
  }

  function renderHealthPanel(identities: IdentityStatusRow[]) {
    return (
      <div className="console-panel" data-testid="health-panel">
        <ul className="console-panel__list">
          {identities.map((r) => (
            <li data-testid={`health-identity:${r.identity}`} key={r.identity}>
              <strong>{r.display_name || r.identity}</strong> · {identityStateLabel(r)} ·{" "}
              {r.addressability}
            </li>
          ))}
        </ul>
      </div>
    );
  }

  async function refreshInspectIdentity(identity: string): Promise<void> {
    const r = await inspectIdentityViaHeadless(identity);
    setInspectByIdentity((current) => ({
      ...current,
      [identity]: normalizeConsoleInspectResult(r),
    }));
  }

  function handleShowRosterDetails(agent: ConsoleAgent) {
    setSelectedRosterMemberId(agent.member_id);
    const target = buildInspectTarget(agent);
    dock.openTarget(target, "replace_focused");
    void refreshInspectIdentity(target.identity).catch(() => {});
  }

  // =========================================================================
  // MAIN RENDER
  // =========================================================================

  const mobName =
    experience?.console_config?.title ||
    experience?.agent_sidebar?.title ||
    "mob";
  const brand = experience?.console_config?.brand;
  const environmentLabel =
    experience?.console_config?.environment?.label || "dev";
  const railConfig = experience?.console_config?.rail;
  const railVisible = railConfig?.visible !== false;
  const mobStatus = consoleMobStatus(experience?.health_overview?.live_snapshot);

  function toggleTheme() {
    const next: ConsoleTheme = theme === "dark" ? "light" : "dark";
    setTheme(next);
    try {
      localStorage.setItem("meta-console-theme", next);
    } catch {
      /* ignore */
    }
  }

  function renderPanelBody(panel: {
    id: string;
    target?: MetaDockTarget | null;
  }) {
    const target = panel.target as MetaDockTarget | null;
    if (!target) return <div className="console-panel">No panel target</div>;
    if (target.kind === "custom/panel") return <ConsoleCustomPanel target={target} focused={dock.viewState.focusedPanelId === panel.id} />;
    if (target.kind === "agent-chat") return renderChatPanel(panel);
    if (target.kind === "operator-panel") {
      return <OperatorPanel buttonId={target.buttonId} buttons={experience?.console_config?.sidebar?.buttons} />;
    }
    if (target.kind === "identity-inspect") {
      return renderInspectPanel(target);
    }
    if (
      (target.kind === "routing" ||
        target.kind === "gating" ||
        target.kind === "gates") &&
      !hasMobControlSurface
    ) {
      return (
        <div className="console-panel">
          This view requires a mob runtime control surface.
        </div>
      );
    }
    if (target.kind === "routing") return <RoutingPanel data={routingData} />;
    if (target.kind === "gating")
      return (
        <GatingInboxPanel
          pending={activeApprovals?.requests.map(request => request.raw) || []}
          resource={activeApprovals}
          selectedPendingId={selectedApprovalId}
          onRefresh={() => void approvalResourceRef.current?.refresh()}
          audit={gatingData.audit}
          onDecide={(pid, decision) => void onGatingDecision(pid, decision)}
          readOnly={consoleReadOnly}
        />
      );
    if (target.kind === "topology")
      return (
        <TopologyPanel
          nodes={normalizedTopology?.nodes || experience?.topology?.live_snapshot?.nodes || []}
          agents={agents}
          activity={liveFrames}
          management={normalizedTopology?.management || null}
          connectionSourceId={topologyConnectionSourceId}
          onConnectionSourceChange={setTopologyConnectionSourceId}
          onRequestMutation={(intent) => onTopologyMutation(intent)}
          onRetryOperation={(receipt) => onRetryTopologyOperation(receipt)}
        />
      );
    if (target.kind === "health")
      return renderHealthPanel(
        experience?.health_overview?.live_snapshot?.identities || [],
      );
    if (target.kind === "timeline")
      return <TimelinePanel frames={activityRef.current} />;
    if (target.kind === "roster")
      return (
        <RosterPanel
          agents={agents}
          selectedMemberId={selectedRosterMemberId}
          onSelect={(a) => setSelectedRosterMemberId(a.member_id)}
          onChat={(a) => openAgentChat(a)}
          onDetails={(a) => handleShowRosterDetails(a)}
          onLifecycle={(identity, method) =>
            void onLifecycleAction(identity, method)
          }
          canResetLifecycle={!consoleReadOnly && hasMobControlSurface}
          actionLabels={configuredActionLabels}
          actionVisibility={{
            ...configuredActionVisibility,
            respawn: !consoleReadOnly && configuredActionVisibility.respawn,
            retire: !consoleReadOnly && configuredActionVisibility.retire,
            reset: !consoleReadOnly && configuredActionVisibility.reset,
          }}
        />
      );
    if (target.kind === "gates")
      return (
        <GatingInboxPanel
          pending={activeApprovals?.requests.map(request => request.raw) || []}
          resource={activeApprovals}
          selectedPendingId={selectedApprovalId}
          onRefresh={() => void approvalResourceRef.current?.refresh()}
          audit={gatingData.audit}
          onDecide={(pid, decision) => void onGatingDecision(pid, decision)}
          readOnly={consoleReadOnly}
        />
      );
    if (target.kind === "logs")
      return <LogsPanel frames={activityRef.current} />;
    if (target.kind === "access")
      return (
        <AccessPanel
          key={accessScope}
          status={experience?.access?.can_administer === true ? visibleAccessData?.status ?? null : null}
          config={visibleAccessData?.config ?? null}
          error={visibleAccessData?.error}
          loading={visibleAccessData?.loading ?? true}
          readOnly={frontendReadOnly || experience?.console_policy?.read_only === true}
          agents={agents.map((agent) => ({
            identity: agent.identity || agent.member_id,
            label: agent.label,
          }))}
          onRefresh={() => void refreshAccessData()}
          onSetEnabled={(enabled, base) =>
            runAccessMutation(CONSOLE_COMMAND_NAMES.enableAccess, { enabled }, base)
          }
          onSaveAdmins={(admins, base) =>
            runAccessMutation(CONSOLE_COMMAND_NAMES.setAccessConfig, { config: { ...base.config, admins } }, base)
          }
          onUpsertRule={(rule, base) =>
            runAccessMutation(CONSOLE_COMMAND_NAMES.upsertAccessRule, { rule }, base)
          }
          onDeleteRule={(id, base) =>
            runAccessMutation(CONSOLE_COMMAND_NAMES.deleteAccessRule, { id }, base)
          }
          onSaveGroup={(name, group, base) =>
            runAccessMutation(CONSOLE_COMMAND_NAMES.setAccessGroup, { name, group }, base)
          }
          onDeleteGroup={(name, base) =>
            runAccessMutation(CONSOLE_COMMAND_NAMES.deleteAccessGroup, { name }, base)
          }
          onPreview={async (subject, action, identity) =>
            ((await executeHeadlessCommand(
              CONSOLE_COMMAND_NAMES.previewAccess,
              controlWorkbenchTarget("access"),
              identity ? { subject, action, identity } : { subject, action },
            )) as AccessPreviewResult | null) || null
          }
        />
      );
    if (target.kind === "memory")
      return runtimeScopedPanel("memory",
        <MemoryPanel
          records={memoryData.records}
          realms={memoryData.realms}
          quarantineRecords={memoryData.quarantineRecords}
          pendingPromotions={memoryData.pendingPromotions}
          dreams={memoryData.dreams}
          detail={memoryData.detail}
          detailLoading={memoryData.detailLoading}
          canReviewQuarantine={experience?.memory?.can_review_quarantine === true}
          unavailable={memoryData.unavailable}
          error={memoryData.error}
          nextCursor={memoryData.nextCursor}
          recordsDenied={memoryData.recordsDenied}
          dreamsDenied={memoryData.dreamsDenied}
          operatorScopeDenied={memoryData.operatorScopeDenied}
          mobScopeDenied={memoryData.mobScopeDenied}
          overview={memoryData.overview}
          overviewDenied={memoryData.overviewDenied}
          proposals={memoryData.proposals}
          proposalsDenied={memoryData.proposalsDenied}
          injections={memoryData.injections}
          injectionsDenied={memoryData.injectionsDenied}
          harvests={memoryData.harvests}
          harvestsDenied={memoryData.harvestsDenied}
          dreamRuns={memoryData.dreamRuns}
          dreamRunsDenied={memoryData.dreamRunsDenied}
          auditVerdicts={memoryData.auditVerdicts}
          auditVerdictsDenied={memoryData.auditVerdictsDenied}
          liveFrames={framesForPanelRuntime(activityRef.current, experience?.panel_runtimes ? panelRuntimeKey : undefined)}
          onRefresh={(record) => {
            if (record) {
              void loadMemoryRecordDetail(record.realm, record.memoryId, true);
            } else {
              void refreshMemoryData();
            }
          }}
          onSelectRecord={(realm, memoryId) => void loadMemoryRecordDetail(realm, memoryId)}
          onClearDetail={() => {
            memoryDetailRequestRef.current += 1;
            setMemoryData((current) => ({ ...current, detail: null, detailLoading: false }));
          }}
          onQueryRecords={queryMemoryRecords}
          onLoadEvidence={loadMemoryEvidence}
          onDecideQuarantine={
            memoryData.canDecideQuarantine &&
            !frontendReadOnly &&
            experience?.console_policy?.read_only !== true
              ? decideMemoryQuarantine
              : undefined
          }
          onOpenGating={
            // Only offered where the nav itself offers gating — on runtimes
            // without a mob control surface (or with gating hidden) the
            // target would land on a dead-end placeholder.
            visibleControls.includes("gating")
              ? () => dock.openTarget(buildControlTarget("gating"), "replace_focused")
              : undefined
          }
        />
      );
    if (target.kind === "workgraph") {
      const workGraphPanelHandlers = makeWorkGraphOperatorHandlers();
      return runtimeScopedPanel("workgraph",
        <WorkGraphPanel
          data={workGraphData}
          canManage={canManageWorkGraph}
          onRefresh={() => void refreshWorkGraphData()}
          onClaim={workGraphPanelHandlers.onClaim}
          onClose={workGraphPanelHandlers.onClose}
          onGoalConfirm={workGraphPanelHandlers.onGoalConfirm}
          onGoalRequestClose={workGraphPanelHandlers.onGoalRequestClose}
          onAttentionPause={workGraphPanelHandlers.onAttentionPause}
          onAttentionResume={workGraphPanelHandlers.onAttentionResume}
          onAttentionReassign={workGraphPanelHandlers.onAttentionReassign}
        />
      );
    }
    return <div className="console-panel">Unsupported panel</div>;
  }

  return (
    <ConsoleMcpAppsProvider value={effectiveMcpAppsHost ? { host: effectiveMcpAppsHost, authority: sendScope, readOnly: consoleReadOnly } : null}>
    <ConsolePanelsProvider value={panelProvider}>
    <div
      className="cc-theme-scope meta-shell"
      data-cc-theme={theme}
      data-cc-variant={variant}
      data-testid="meerkat-console"
    >
      <SpriteSheet />
      {panelState.errors.length > 0 ? <div className="meta-action-error" role="alert">
        Custom panels could not load: {panelState.errors.join("; ")}
      </div> : null}
      {actionError && (
        <div className="meta-action-error" data-testid="console-action-error" role="alert">
          <span>{actionError}</span>
          <button
            aria-label="Dismiss error"
            data-testid="console-action-error-dismiss"
            onClick={() => setActionError("")}
            type="button"
          >
            ×
          </button>
        </div>
      )}
      <Topbar
        connectionStatus={<ConsoleTransportStatus state={transportState} onRetry={() => setTransportRetry((value) => value + 1)} />}
        mobName={mobName}
        brandLabel={brand?.label}
        brandLogoUrl={brand?.logo_url}
        brandLogoAlt={brand?.logo_alt}
        mobStatus={mobStatus}
        environment={environmentLabel}
        theme={theme}
        onToggleTheme={toggleTheme}
        sidebarCollapsed={sidebarCollapsed}
        railCollapsed={railCollapsed}
        railVisible={railVisible}
        onToggleSidebar={toggleSidebarCollapsed}
        onToggleRail={toggleRailCollapsed}
      />
      <div
        className="shell"
        data-console-workbench="root"
        data-sidebar-collapsed={sidebarCollapsed ? "true" : "false"}
        data-rail-collapsed={railCollapsed ? "true" : "false"}
      >
        <DesignSidebar
          agents={agents}
          selectedMemberId={focusedMemberId}
          recentActivity={activityRef.current}
          collapsed={sidebarCollapsed}
          visibleControls={visibleControls}
          customButtons={experience?.console_config?.sidebar?.buttons}
          customPanels={customPanels}
          onOpenCustomPanel={openCustomPanel}
          grouping={experience?.console_config?.agent_list}
          storageNamespace={sidebarStorageNamespace}
          pinnedAgentIds={pinnedAgentIds}
          onSelect={selectSidebarAgent}
          onTogglePinnedAgent={togglePinnedAgent}
          onOpenControl={openSidebarControl}
          onOpenOperatorPanel={openOperatorPanel}
          approvals={activeApprovals}
          onOpenApproval={openApproval}
        />
        <div
          className="pane-resizer"
          aria-hidden="true"
          data-testid="resize:sidebar"
          onPointerDown={handleSidebarResize}
        />
        <div className="main">
          <MetaDock
            viewState={dock.viewState}
            agents={agents}
            renderPanelBody={renderPanelBody}
            customPanels={customPanels}
            visibleControls={visibleControls}
            onSelectTab={(id) => dock.selectTab(id)}
            onCloseTab={(id) => dock.closeTab(id)}
            onCreateTab={() => dock.createTab()}
            onFocusPanel={(id) => dock.focusPanel(id)}
            onSplitPanel={(id, dir) => dock.splitPanel(id, dir)}
            onClosePanel={(id) => dock.closePanel(id)}
            onResizeSplit={(id, ratio) => dock.resizeSplit(id, ratio)}
            onOpenTargetInPanel={(panelId, target) => {
              dock.focusPanel(panelId);
              // Pane-picker registrations are unbound until the host supplies
              // its current conversation context.
              if (target.kind === "custom/panel") {
                const definition = customPanels.find(panel => panel.id === target.payload.panelId);
                if (definition) openDockTarget(consoleCustomPanelTarget(definition, { conversation: panelSelection, scopeKey: sendScope }));
              } else openDockTarget(target);
            }}
          />
          {dock.focusedTarget?.kind !== "agent-chat" ? voiceBar : null}
        </div>
        {railVisible ? (
          <>
            <div
              className="pane-resizer pane-resizer--activity"
              aria-hidden="true"
              data-testid="resize:activity"
              onPointerDown={handleActivityResize}
            />
            <SignalsRail
              frames={activityRef.current}
              collapsed={railCollapsed}
              filterPresets={railConfig?.filter_presets}
              activePresetId={
                activeActivityPresetId || railConfig?.active_preset_id
              }
              emptyText={railConfig?.empty_text}
              watchedIdentities={watchedIdentities}
              onPresetChange={setActiveActivityPresetId}
              onSelect={experience?.memory?.can_read === true ? selectRailFrame : undefined}
            />
          </>
        ) : null}
      </div>
    </div>
    </ConsolePanelsProvider>
    </ConsoleMcpAppsProvider>
  );
}
