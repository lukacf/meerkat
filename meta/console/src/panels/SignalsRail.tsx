import React from "react";
import type { ConsoleFrame, ConsoleRailFilterPresetConfig } from "../types";
import { countRender } from "../lib/render-counts";
import { useActivityLedger } from "../lib/activity-ledger";
import {
  AgentName,
  Button,
  EmptyIllustration,
  IconButton,
  RelativeTime,
  SegmentedControl,
  Sparkline,
  StatusMark,
  useConsoleNavigation,
  EMPTY_AGENT_DIRECTORY,
  type AgentDirectory,
  type Tone,
} from "../../../packages/console-components/src/panel-kit";
import {
  DEFAULT_FILTER_PRESETS,
  buildSignalGroups,
  groupMatchesPreset,
  signalNameResolver,
  type PendingApprovalLike,
  type Signal,
  type SignalGroup,
  type SignalSeverity,
  type SignalTarget,
} from "./activity/signals-model";

export { buildSignalGroupsForTest, signalNameResolver } from "./activity/signals-model";

interface SignalsRailProps {
  frames: ConsoleFrame[];
  collapsed: boolean;
  filterPresets?: ConsoleRailFilterPresetConfig[];
  activePresetId?: string;
  emptyText?: string;
  watchedIdentities?: Set<string>;
  /** Resolves agent keys to display names ("Support Triage replied"). Without
   * it, or for agents it does not know, a key gets the console's shared
   * display title for it. */
  agentDirectory?: AgentDirectory;
  /** Approvals waiting for a decision: each is a signal with Review. */
  approvals?: readonly PendingApprovalLike[];
  onPresetChange?: (presetId: string) => void;
  /** Opens a memory signal's record (offered only with memory access). */
  onSelect?: (frame: ConsoleFrame) => void;
  /** Fallbacks while the console provides no navigation context. */
  onOpenAgent?: (identity: string, how: "details" | "chat") => void;
  onOpenApproval?: (pendingId: string) => void;
  onOpenPanel?: (kind: string) => void;
}

const SEVERITY_TONE: Readonly<Record<SignalSeverity, Tone>> = {
  critical: "crit",
  warning: "warn",
  info: "neutral",
};

const PULSE_BINS = 15;

/** A signal's mark: its severity, or for a quiet one the good outcome or live work it reports. */
function signalTone(group: SignalGroup): Tone {
  if (group.severity !== "info") return SEVERITY_TONE[group.severity];
  const lead = group.items.find((item) => item.raw.event === "interaction_complete");
  return lead ? "ok" : "neutral";
}

/** The words of an open button: where the signal leads. */
function openLabel(target: SignalTarget): string {
  switch (target.kind) {
    case "chat": return "Open chat";
    case "agent": return "Open details";
    case "approval": return "Open approval";
    case "memory": return "Open in Memory";
    case "routing": return "Open Routing";
  }
}

/// Memoised: the app root re-renders on every coalesced frame flush and on
/// each debounced draft publish, and the props here are stable references
/// (see ConsoleApp), so unchanged input skips the whole subtree.
export const SignalsRail = React.memo(function SignalsRail({
  frames,
  collapsed,
  filterPresets,
  activePresetId,
  emptyText,
  watchedIdentities,
  agentDirectory = EMPTY_AGENT_DIRECTORY,
  approvals,
  onPresetChange,
  onSelect,
  onOpenAgent,
  onOpenApproval,
  onOpenPanel,
}: SignalsRailProps): React.JSX.Element {
  countRender("SignalsRail");
  const navigation = useConsoleNavigation();
  const ledger = useActivityLedger({ tickMs: 60_000 });
  const presets = React.useMemo(() => {
    const configured = (filterPresets || []).filter((preset) => preset.id && preset.label);
    return configured.length > 0 ? configured : DEFAULT_FILTER_PRESETS;
  }, [filterPresets]);
  const [filter, setFilter] = React.useState<string>(activePresetId || presets[0]?.id || "all");
  const [expandedGroups, setExpandedGroups] = React.useState<Set<string>>(() => new Set());
  const detailIdPrefix = React.useId();

  React.useEffect(() => {
    if (activePresetId && presets.some((preset) => preset.id === activePresetId)) {
      setFilter(activePresetId);
    }
  }, [activePresetId, presets]);

  const groups: SignalGroup[] = React.useMemo(
    () => buildSignalGroups(frames, agentDirectory, { approvals }),
    [frames, agentDirectory, approvals],
  );
  const nameOf = React.useMemo(() => signalNameResolver(agentDirectory), [agentDirectory]);

  const activePreset = presets.find((preset) => preset.id === filter) || presets[0] || DEFAULT_FILTER_PRESETS[0];
  const counts = React.useMemo(() => new Map(presets.map((preset) => [
    preset.id,
    groups.filter((group) => groupMatchesPreset(group, preset, watchedIdentities)).length,
  ])), [groups, presets, watchedIdentities]);
  const shown = groups.filter((group) => groupMatchesPreset(group, activePreset, watchedIdentities));
  const nowMs = ledger.nowMs;
  const recent15m = groups.filter((group) => nowMs - (group.items[0]?.atMs || 0) < 15 * 60 * 1000).length;
  const pulse = React.useMemo(
    () => ledger.series(null, "events", { bins: PULSE_BINS, binMs: 60_000, endMs: nowMs }),
    // The ledger's version moves with every change it counts.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ledger.version, nowMs],
  );
  const pulseTotal = pulse.reduce((sum, value) => sum + value, 0);

  /** Opens where a signal leads; false when the console cannot open it. */
  const canOpen = (target: SignalTarget | undefined): boolean => {
    if (!target) return false;
    switch (target.kind) {
      case "chat":
      case "agent": return Boolean(navigation?.openAgent || onOpenAgent);
      case "approval": return Boolean(navigation?.openApproval || onOpenApproval);
      case "memory": return Boolean(onSelect);
      case "routing": return Boolean((target.routeKey && navigation?.openRoute) || navigation?.openPanel || onOpenPanel);
    }
  };
  const open = (target: SignalTarget | undefined): void => {
    if (!target) return;
    switch (target.kind) {
      case "chat":
      case "agent": {
        const how = target.kind === "chat" ? "chat" : "details";
        if (navigation?.openAgent) navigation.openAgent(target.identity, how);
        else onOpenAgent?.(target.identity, how);
        return;
      }
      case "approval":
        if (navigation?.openApproval) navigation.openApproval(target.pendingId);
        else onOpenApproval?.(target.pendingId);
        return;
      case "memory":
        // The memory pivot stays behind the console's memory read gate.
        onSelect?.(target.frame);
        return;
      case "routing":
        if (target.routeKey && navigation?.openRoute) navigation.openRoute(target.routeKey);
        else if (navigation?.openPanel) navigation.openPanel("routing");
        else onOpenPanel?.("routing");
        return;
    }
  };

  function toggleGroup(group: SignalGroup): void {
    if (group.items.length <= 1) {
      const item = group.items[0];
      if (item.target?.kind === "memory" || !canOpen(item.target)) onSelect?.(item.raw);
      else open(item.target);
      return;
    }
    setExpandedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(group.id)) next.delete(group.id);
      else next.add(group.id);
      return next;
    });
  }

  if (collapsed) {
    return (
      <aside
        className="rail rail--collapsed"
        data-collapsed="true"
        data-testid="signals-rail"
      >
        <i className="rail__grip" aria-hidden="true" />
      </aside>
    );
  }

  return (
    <aside className="rail" data-testid="signals-rail" aria-label="Signals">
      <div className="rail__head">
        <span className="rail__title">Signals</span>
        <span className="rail__sub">{recent15m} in 15m</span>
      </div>
      <div className="rail__pulse">
        <Sparkline
          values={pulse}
          kind="bars"
          width={260}
          height={22}
          live={pulse[pulse.length - 1] > 0}
          label={pulseTotal
            ? `Activity in the last 15 minutes: ${pulseTotal} ${pulseTotal === 1 ? "event" : "events"}, ${pulse[pulse.length - 1]} in the last minute`
            : "No activity in the last 15 minutes"}
          className="rail__spark"
          testId="signals-pulse"
        />
      </div>
      <SegmentedControl
        className="rail__filters"
        ariaLabel="Show signals"
        value={activePreset.id}
        onChange={(id) => {
          setFilter(id);
          onPresetChange?.(id);
        }}
        options={presets.map((preset) => ({
          id: preset.id,
          label: preset.label,
          count: counts.get(preset.id) || 0,
          testId: `signals-filter:${preset.id}`,
        }))}
      />
      <div className="rail__list">
        {shown.length === 0 && (
          <div className="rail__empty">
            <EmptyIllustration name={groups.length ? "search" : "timeline"} size="inline" className="rail__empty-art" />
            <span>{groups.length ? "Nothing here at this level." : emptyText || "No meaningful signals yet."}</span>
          </div>
        )}
        {shown.map((s, index) => {
          const expanded = expandedGroups.has(s.id);
          const detailId = `${detailIdPrefix}detail-${index}`;
          const grouped = s.items.length > 1;
          const agentKey = s.agent && s.agent !== "_system" ? s.agent : "";
          const single: Signal | undefined = grouped ? undefined : s.items[0];
          const approval = single?.approval;
          const target = s.target;
          const memoryPivot = Boolean(single && single.raw.event.startsWith("memory.") && onSelect);
          const showOpen = canOpen(target) && target?.kind !== "memory" && !approval;
          const tone = signalTone(s);
          return (
            <div
              key={s.id}
              className="signal cc-reveal-host"
              data-sev={s.severity}
              data-tone={tone}
              data-testid={`signal:${s.id}`}
              data-expanded={expanded ? "true" : "false"}
              onClick={(event) => {
                // The label button and the row's other controls handle their
                // own clicks; anywhere else on the row opens it.
                const clicked = event.target as Element | null;
                if (clicked?.closest?.("a, button, input, select, textarea, summary")) return;
                toggleGroup(s);
              }}
            >
              <span className="signal__bar" />
              <span className="signal__body">
                <span className="signal__head">
                  <StatusMark tone={tone} className="signal__mark" />
                  {/* The label is the row's button; the Memory pivot, Review
                      and the grouped events sit beside it, never inside. */}
                  <button
                    type="button"
                    className="signal__hit"
                    aria-expanded={grouped ? expanded : undefined}
                    aria-describedby={detailId}
                    onClick={() => toggleGroup(s)}
                  >
                    <span className="signal__label">
                      {grouped && <span className="signal__chevron" aria-hidden="true" />}
                      {s.title}
                      {grouped && <span className="signal__count">{s.items.length}</span>}
                    </span>
                  </button>
                  {s.atMs ? <RelativeTime value={s.atMs} className="signal__time" /> : null}
                </span>
                <span className="signal__detail" id={detailId}>{s.detail}</span>
                <span className="signal__foot">
                  {agentKey ? (
                    <AgentName id={agentKey} avatar={18} link directory={agentDirectory} className="signal__agent" />
                  ) : (
                    <span className="signal__agent cc-agent"><span className="cc-agent__name">{nameOf(s.agent)}</span></span>
                  )}
                  <span className="signal__actions">
                    {approval && canOpen(target) ? (
                      <Button
                        size="sm"
                        className="signal__review"
                        data-testid={`signal-review:${approval.pendingId}`}
                        onClick={(event) => {
                          event.stopPropagation();
                          open(target);
                        }}
                      >
                        Review
                      </Button>
                    ) : null}
                    {memoryPivot ? (
                      <button
                        type="button"
                        className="signal__memory-pivot"
                        data-testid="signal-memory-pivot"
                        onClick={(event) => {
                          event.stopPropagation();
                          onSelect?.(s.items[0].raw);
                        }}
                      >
                        Open in Memory
                      </button>
                    ) : null}
                    {showOpen && target ? (
                      <IconButton
                        reveal
                        quiet
                        size="sm"
                        glyph="arrow-right"
                        label={openLabel(target)}
                        className="signal__open"
                        data-testid={`signal-open:${s.id}`}
                        onClick={(event) => {
                          event.stopPropagation();
                          open(target);
                        }}
                      />
                    ) : null}
                  </span>
                </span>
                {s.items.length > 1 && expanded && (
                  <span className="signal__events">
                    {s.items.map((item) => (
                      <button
                        key={item.id}
                        className="signal__event"
                        type="button"
                        onClick={(event) => {
                          event.stopPropagation();
                          onSelect?.(item.raw);
                          if (item.target && item.target.kind !== "memory") open(item.target);
                        }}
                      >
                        <span className="signal__event-label">{item.label}</span>
                        <span className="signal__event-detail">{item.detail}</span>
                      </button>
                    ))}
                  </span>
                )}
              </span>
            </div>
          );
        })}
      </div>
    </aside>
  );
});
