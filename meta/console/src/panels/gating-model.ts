// The Approvals panel's model (spec B7): the recorded approvals in words,
// joined to the requests they decided, by day, newest first; the decisions
// for the strip; the risk mix; and the pending queue by deadline. Pure
// functions over the gating reads, tested without React.

import { normalizeGatingAuditEntry, type GatingAuditEntryView } from "../../../packages/console-core/src/control-plane";
import type { PendingApproval } from "../../../packages/console-core/src/pending-approvals";

export type RawRecord = Record<string, unknown>;

export interface AuditRow {
  raw: RawRecord;
  record: GatingAuditEntryView;
  /** When it was recorded (epoch ms), or null. */
  at: number | null;
}

/** A human decision or an expiry, for the decisions strip. */
export type DecisionKind = "approve" | "reject" | "escalate" | "timeout";

export interface DecisionEvent {
  id: string;
  at: number;
  kind: DecisionKind;
  title: string;
  approverId?: string;
}

export type RiskTier = "r0" | "r1" | "r2" | "r3";
export const RISK_TIERS: readonly RiskTier[] = ["r0", "r1", "r2", "r3"];

export function recordOf(value: unknown): RawRecord {
  return value && typeof value === "object" && !Array.isArray(value) ? value as RawRecord : {};
}

function textOf(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function timeOf(raw: RawRecord): number | null {
  const value = raw.timestamp_ms ?? raw.decided_at_ms ?? raw.recorded_at_ms;
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : null;
}

export function auditRows(audit: readonly unknown[]): AuditRow[] {
  return audit.map((value) => {
    const raw = recordOf(value);
    return { raw, record: normalizeGatingAuditEntry(value), at: timeOf(raw) };
  });
}

/**
 * The words each action reads by: the label its request or its evaluation
 * carried (`detail.action`), keyed by action id. A decision row carries no
 * label of its own, so its title comes from here instead of the action id
 * ("Gate action v2 ...").
 */
export function actionTitles(rows: readonly AuditRow[], requests: readonly PendingApproval[] = []): Map<string, string> {
  const titles = new Map<string, string>();
  for (const request of requests) {
    if (request.actionId && request.action && request.action !== request.actionId) titles.set(request.actionId, request.action);
  }
  for (const { raw, record } of rows) {
    const label = textOf(recordOf(raw.detail).action) || textOf(raw.action);
    if (record.actionId && label && !titles.has(record.actionId)) titles.set(record.actionId, label);
  }
  return titles;
}

/** The human decision or expiry a recorded event is, if any. */
export function decisionKind(record: GatingAuditEntryView): DecisionKind | null {
  if (record.decision === "approve") return "approve";
  if (record.decision === "reject") return "reject";
  if (record.decision === "escalate") return "escalate";
  if (record.eventType === "approval_decided") return "approve";
  if (record.eventType === "rejection_decided") return "reject";
  if (record.eventType === "escalation_decided") return "escalate";
  if (record.eventType === "timeout_fallback") return "timeout";
  return null;
}

export function decisionEvents(rows: readonly AuditRow[], titles: ReadonlyMap<string, string>): DecisionEvent[] {
  const events: DecisionEvent[] = [];
  rows.forEach(({ record, at }, index) => {
    const kind = decisionKind(record);
    if (!kind || at === null) return;
    events.push({
      id: record.auditId || `decision-${index}`,
      at,
      kind,
      title: (record.actionId && titles.get(record.actionId)) || "A request",
      approverId: record.approverId,
    });
  });
  return events;
}

/** Decisions recorded since local midnight, by kind. */
export function decidedSince(events: readonly DecisionEvent[], since: number): Record<DecisionKind, number> & { total: number } {
  const counts = { approve: 0, reject: 0, escalate: 0, timeout: 0, total: 0 };
  for (const event of events) {
    if (event.at < since) continue;
    counts[event.kind] += 1;
    counts.total += 1;
  }
  return counts;
}

export function startOfDay(now: number): number {
  const date = new Date(now);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

/** How many distinct actions the recorded window holds at each risk tier
 * (an action counted once, however many records it has). */
export function riskMix(rows: readonly AuditRow[]): Record<RiskTier, number> & { total: number; unknown: number } {
  const tierByAction = new Map<string, string>();
  rows.forEach(({ record }, index) => {
    const key = record.actionId || record.auditId || `row-${index}`;
    if (!tierByAction.has(key) || record.riskTier !== "unknown") tierByAction.set(key, record.riskTier);
  });
  const mix = { r0: 0, r1: 0, r2: 0, r3: 0, total: 0, unknown: 0 };
  for (const tier of tierByAction.values()) {
    if (tier === "r0" || tier === "r1" || tier === "r2" || tier === "r3") mix[tier] += 1;
    else mix.unknown += 1;
    mix.total += 1;
  }
  return mix;
}

export interface DayGroup<T> {
  key: string;
  label: string;
  rows: T[];
}

function dayLabel(dayStart: number, today: number): string {
  if (dayStart === today) return "Today";
  if (dayStart === startOfDay(today - 12 * 3_600_000)) return "Yesterday";
  return new Date(dayStart).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
}

/** Rows newest first in groups by local day ("Today", "Yesterday", "Wed,
 * Oct 29"); rows without a time last, in their own group. */
export function groupByDay<T extends { at: number | null }>(rows: readonly T[], now: number): DayGroup<T>[] {
  const today = startOfDay(now);
  const sorted = rows
    .map((row, index) => ({ row, index }))
    .sort((a, b) => {
      const at = a.row.at ?? -Infinity;
      const bt = b.row.at ?? -Infinity;
      return bt - at || b.index - a.index;
    });
  const groups: DayGroup<T>[] = [];
  for (const { row } of sorted) {
    const key = row.at === null ? "undated" : String(startOfDay(row.at));
    let group = groups[groups.length - 1];
    if (!group || group.key !== key) {
      group = { key, label: row.at === null ? "Time not recorded" : dayLabel(startOfDay(row.at), today), rows: [] };
      groups.push(group);
    }
    group.rows.push(row);
  }
  return groups;
}

/** The pending queue soonest deadline first; requests without a deadline
 * after them, oldest first. */
export function byDeadline(requests: readonly PendingApproval[]): PendingApproval[] {
  return [...requests].sort((a, b) => {
    const ad = a.deadlineAtMs ?? Infinity;
    const bd = b.deadlineAtMs ?? Infinity;
    if (ad !== bd) return ad - bd;
    return (a.createdAtMs ?? 0) - (b.createdAtMs ?? 0);
  });
}
