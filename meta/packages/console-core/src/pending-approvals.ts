import { normalizeGatingActionResult, type GatingActionResult } from "./control-plane";
import { CONSOLE_RPC_METHODS } from "./contract";

export type ApprovalAction = "approve" | "reject" | "escalate";
export const APPROVAL_ACTIONS: readonly ApprovalAction[] = ["approve", "reject", "escalate"];
export interface ApprovalOrigin {
  identity: string;
  conversationId?: string;
  interactionId?: string;
}
export interface PendingApproval {
  pendingId: string;
  actionId: string;
  action: string;
  /** Whether the owner can escalate this request: "unknown" until it says
   * (its `supported_actions`), "unavailable" when no escalation route is set
   * up for it. */
  escalation: "available" | "unavailable" | "unknown";
  actorId?: string;
  requestedApprover?: string;
  approvalRecipient?: string;
  approvalChannel?: string;
  approvalRouteId?: string;
  approvalDeliveryId?: string;
  rationale?: string;
  riskTier?: string;
  status: "pending" | "settled" | "expired";
  actions: readonly ApprovalAction[];
  createdAtMs?: number;
  deadlineAtMs?: number;
  origin?: ApprovalOrigin;
  raw: Readonly<Record<string, unknown>>;
}
/** A decision the owner refused for a stated reason (a typed refusal): the
 * request was not decided and stays as it was. */
export interface ApprovalRefusal {
  /** The owner's machine kind ("no_escalation_route"). */
  kind: string;
  /** The reason in words for the person deciding. */
  words: string;
}
export interface ApprovalDecisionState {
  /** Unavailable means a typed capability refusal before dispatch; refused a
   * typed refusal by the owner after dispatch (nothing was decided); failed
   * retains outcome uncertainty. */
  phase: "submitting" | "settled" | "failed" | "unavailable" | "refused";
  action: ApprovalAction;
  result?: GatingActionResult;
  error?: string;
  refusal?: ApprovalRefusal;
}
export interface PendingApprovalSnapshot {
  scopeKey: string;
  /** Unsupported comes from absent capability; unavailable is a transient read failure. */
  status: "loading" | "ready" | "stale" | "unavailable" | "forbidden" | "unsupported";
  requests: readonly PendingApproval[];
  decisions: Readonly<Record<string, ApprovalDecisionState>>;
  readOnly: boolean;
  updatedAtMs?: number;
  error?: string;
}
export interface ApprovalResourceEnvironment {
  now(): number;
  visible(): boolean;
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(timer: unknown): void;
  onVisibilityChange(callback: () => void): () => void;
}
export interface PendingApprovalResource {
  getSnapshot(): PendingApprovalSnapshot;
  subscribe(listener: () => void): () => void;
  refresh(): Promise<void>;
  decide(pendingId: string, action: ApprovalAction): Promise<void>;
  dispose(): void;
}
const POLL_MS = 15_000;
const STALE_MS = 2 * POLL_MS;
const text = (value: unknown) => typeof value === "string" && value.trim() ? value : undefined;
const millis = (value: unknown) => typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 8_640_000_000_000_000 ? value : undefined;
export function normalizePendingApproval(value: unknown): PendingApproval | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Record<string, unknown>;
  const pendingId = text(raw.pending_id);
  if (!pendingId) return null;
  if (raw.status !== undefined && raw.status !== "pending" && raw.status !== "settled" && raw.status !== "expired") return null;
  const record = raw.origin && typeof raw.origin === "object" ? raw.origin as Record<string, unknown> : undefined;
  const identity = text(record?.identity);
  const supported = Array.isArray(raw.supported_actions) ? raw.supported_actions as unknown[] : null;
  const actions = supported ? APPROVAL_ACTIONS.filter((action) => supported.includes(action)) : APPROVAL_ACTIONS;
  return {
    pendingId, actionId: text(raw.action_id) || "Unknown action scope",
    action: text(raw.action) || text(raw.summary) || text(raw.action_id) || "Approval requested",
    escalation: !supported ? "unknown" : supported.includes("escalate") ? "available" : "unavailable",
    actorId: text(raw.actor_id), rationale: text(raw.rationale), riskTier: text(raw.risk_tier),
    requestedApprover: text(raw.requested_approver), approvalRecipient: text(raw.approval_recipient),
    approvalChannel: text(raw.approval_channel), approvalRouteId: text(raw.approval_route_id),
    approvalDeliveryId: text(raw.approval_delivery_id),
    status: raw.status === "expired" ? "expired" : raw.status === "settled" ? "settled" : "pending",
    actions, createdAtMs: millis(raw.created_at_ms), deadlineAtMs: millis(raw.deadline_at_ms),
    ...(identity ? { origin: { identity, conversationId: text(record?.conversation_id), interactionId: text(record?.interaction_id) } } : {}),
    raw,
  };
}

/** Actor names and human labels are not a conversation-correlation authority. */
export function approvalMatchesConversation(
  request: PendingApproval,
  target: { identity: string; conversationId?: string; interactionIds?: readonly string[] },
): boolean {
  const origin = request.origin;
  if (!origin || origin.identity !== target.identity) return false;
  if (origin.conversationId && target.conversationId && origin.conversationId !== target.conversationId) return false;
  if (origin.interactionId) return target.interactionIds?.includes(origin.interactionId) === true;
  return Boolean(origin.conversationId && target.conversationId === origin.conversationId);
}

function browserEnvironment(): ApprovalResourceEnvironment {
  return {
    now: Date.now,
    visible: () => typeof document === "undefined" || document.visibilityState !== "hidden",
    setTimeout: (callback, ms) => globalThis.setTimeout(callback, ms),
    clearTimeout: (timer) => globalThis.clearTimeout(timer as ReturnType<typeof setTimeout>),
    onVisibilityChange(callback) {
      if (typeof document === "undefined") return () => {};
      document.addEventListener("visibilitychange", callback);
      return () => document.removeEventListener("visibilitychange", callback);
    },
  };
}
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

/** The owner's typed decision refusals, in words. A refusal names its kind
 * in the error data (`data.kind`, or `data.error`); a refusal without a
 * kind stays an uncertain failure. */
const REFUSAL_WORDS: Readonly<Record<string, string>> = {
  no_escalation_route: "No escalation route is set up for this request, so it can't be escalated. It is still waiting for a decision.",
  self_approval_forbidden: "You can't decide a request you made yourself.",
  approver_mismatch: "This request is waiting for another approver.",
  unknown_pending_id: "This request was already decided or has expired.",
  empty_principal: "Sign in to decide approvals.",
  gating_ids_unavailable: "The decision couldn't be recorded right now. Nothing changed; try again.",
};
export function approvalRefusal(error: unknown): ApprovalRefusal | null {
  const typed = error as { rpcError?: { data?: unknown }; responseRpcError?: { data?: unknown } } | null;
  const data = (typed?.rpcError ?? typed?.responseRpcError)?.data;
  const record = data && typeof data === "object" ? data as { kind?: unknown; error?: unknown } : null;
  const kind = typeof record?.kind === "string" ? record.kind : typeof record?.error === "string" ? record.error : "";
  const words = kind ? REFUSAL_WORDS[kind] : undefined;
  return words ? { kind, words } : null;
}
function unavailableCapability(error: unknown): { method: string; availableMethods: readonly string[] } | null {
  const record = error as { kind?: unknown; method?: unknown; availableMethods?: unknown } | null;
  return record?.kind === "console-capability-unavailable" && typeof record.method === "string"
    && Array.isArray(record.availableMethods) && record.availableMethods.every((method) => typeof method === "string")
    ? { method: record.method, availableMethods: record.availableMethods } : null;
}
const isDenied = (error: unknown) => {
  const record = error as { httpStatus?: number; rpcError?: { code?: number; data?: { kind?: string } } } | null;
  return record?.httpStatus === 401 || record?.httpStatus === 403 || record?.rpcError?.code === -32030 || record?.rpcError?.data?.kind === "access_denied";
};

/** One active scoped owner feeds the inbox and every attention/inline view. */
export function createPendingApprovalResource(input: {
  scopeKey: string;
  load(signal: AbortSignal): Promise<unknown>;
  decide(pendingId: string, action: ApprovalAction, signal: AbortSignal): Promise<unknown>;
  readOnly?: boolean;
  environment?: ApprovalResourceEnvironment;
}): PendingApprovalResource {
  const env = input.environment || browserEnvironment();
  let snapshot: PendingApprovalSnapshot = {
    scopeKey: input.scopeKey, status: "loading", requests: [], decisions: {}, readOnly: input.readOnly === true,
  };
  const listeners = new Set<() => void>();
  const lifetime = new AbortController();
  let disposed = false;
  let denied = false;
  let timer: unknown;
  let activeRead: Promise<void> | undefined;
  let invalidated = false;
  let decisionGeneration = 0;
  let settledDecisions: string[] = [];
  const decisionJobs = new Map<string, Promise<void>>();
  const publish = (patch: Partial<PendingApprovalSnapshot>) => {
    if (disposed) return;
    snapshot = { ...snapshot, ...patch };
    for (const listener of listeners) listener();
  };
  const stale = () => {
    if (snapshot.status === "ready" && snapshot.updatedAtMs !== undefined && env.now() - snapshot.updatedAtMs >= STALE_MS) publish({ status: "stale" });
  };
  const schedule = () => {
    if (timer !== undefined) env.clearTimeout(timer);
    timer = undefined;
    if (disposed || denied || !env.visible()) return;
    timer = env.setTimeout(() => {
      timer = undefined;
      stale();
      // A slow read owns the sole slot. Poll ticks do not enqueue more reads.
      if (!activeRead) void refresh(false);
      schedule();
    }, POLL_MS);
  };
  const refresh = (explicit: boolean): Promise<void> => {
    if (disposed || denied) return Promise.resolve();
    if (activeRead) {
      if (explicit) invalidated = true;
      return activeRead;
    }
    const generation = decisionGeneration;
    activeRead = (async () => {
      try {
        const value = await Promise.resolve().then(() => input.load(lifetime.signal));
        if (disposed || denied) return;
        if (generation !== decisionGeneration) { invalidated = true; return; }
        const payload = value && typeof value === "object" ? value as { pending?: unknown } : undefined;
        const raw = Array.isArray(value) ? value : payload?.pending;
        if (!Array.isArray(raw)) throw new Error("Pending approval response is unavailable");
        const requests: PendingApproval[] = [];
        const ids = new Set<string>();
        for (const row of raw) {
          const request = normalizePendingApproval(row);
          if (!request) throw new Error("Pending approval response contains an invalid request");
          if (!ids.has(request.pendingId)) { ids.add(request.pendingId); requests.push(request); }
        }
        publish({ requests, status: "ready", updatedAtMs: env.now(), error: undefined });
      } catch (error) {
        if (disposed || denied) return;
        const unsupported = unavailableCapability(error)?.method === CONSOLE_RPC_METHODS.gatingPending;
        if (isDenied(error) || unsupported) {
          denied = true;
          publish({ requests: [], decisions: {}, status: isDenied(error) ? "forbidden" : "unsupported", readOnly: true, error: errorText(error) });
          lifetime.abort();
        } else if (generation === decisionGeneration) {
          publish({ status: snapshot.updatedAtMs === undefined ? "unavailable" : "stale", error: errorText(error) });
        }
      } finally {
        activeRead = undefined;
        if (invalidated && !disposed && !denied) { invalidated = false; void refresh(false); }
      }
    })();
    return activeRead;
  };
  const setDecision = (id: string, value: ApprovalDecisionState) => {
    let decisions = { ...snapshot.decisions, [id]: value };
    if (value.phase !== "submitting") {
      settledDecisions = settledDecisions.filter((key) => key !== id);
      settledDecisions.push(id);
      while (settledDecisions.length > 100) {
        const expired = settledDecisions.shift()!;
        const { [expired]: _old, ...remaining } = decisions;
        decisions = remaining;
      }
    }
    publish({ decisions });
  };
  const unsubscribeVisibility = env.onVisibilityChange(() => {
    if (env.visible()) { stale(); void refresh(true); }
    schedule();
  });
  const resource: PendingApprovalResource = {
    getSnapshot: () => snapshot,
    subscribe(listener) { if (disposed) return () => {}; listeners.add(listener); return () => { listeners.delete(listener); }; },
    refresh: () => refresh(true),
    decide(pendingId, action) {
      if (disposed) return Promise.resolve();
      const existing = decisionJobs.get(pendingId);
      if (existing) return existing;
      const request = snapshot.requests.find((candidate) => candidate.pendingId === pendingId);
      if (snapshot.readOnly || denied || snapshot.status !== "ready" || !request || request.status !== "pending" || !request.actions.includes(action)) {
        return Promise.resolve();
      }
      const previousDecision = snapshot.decisions[pendingId];
      ++decisionGeneration;
      setDecision(pendingId, { phase: "submitting", action });
      const job = (async () => {
        try {
          const value = await Promise.resolve().then(() => input.decide(pendingId, action, lifetime.signal));
          if (disposed || denied) return;
          const result = normalizeGatingActionResult(value);
          if (!result || result.pending_id !== pendingId || result.action_id !== request.actionId || result.decision !== action
            || (action === "approve" && result.outcome !== "allowed")
            || (action === "reject" && result.outcome !== "safe_draft")
            || (action === "escalate" && (result.outcome !== "pending_approval" || !result.next_pending_id || result.next_pending_id === pendingId))) {
            throw new Error("Decision outcome is unconfirmed; refreshing approval state");
          }
          setDecision(pendingId, { phase: "settled", action, result });
          // The response is authoritative for this request. A successor remains
          // pending and will be read through the same owner on refresh.
          publish({ requests: snapshot.requests.filter((candidate) => candidate.pendingId !== pendingId) });
        } catch (error) {
          if (disposed || denied) return;
          const capability = unavailableCapability(error);
          if (capability?.method === CONSOLE_RPC_METHODS.gatingDecide
            && capability.availableMethods.includes(CONSOLE_RPC_METHODS.gatingPending)) {
            publish({ readOnly: true });
            // A refused retry says nothing about an earlier dispatched attempt.
            setDecision(pendingId, previousDecision?.phase === "failed" ? previousDecision : {
              phase: "unavailable", action, error: "Approval decisions are unavailable with current access",
            });
          } else if (isDenied(error) || capability?.method === CONSOLE_RPC_METHODS.gatingDecide) {
            denied = true;
            publish({ requests: [], decisions: {}, status: isDenied(error) ? "forbidden" : "unsupported", readOnly: true, error: errorText(error) });
            lifetime.abort();
          } else {
            const refusal = approvalRefusal(error);
            setDecision(pendingId, refusal
              ? { phase: "refused", action, refusal, error: refusal.words }
              : { phase: "failed", action, error: errorText(error) });
          }
        } finally {
          ++decisionGeneration;
          decisionJobs.delete(pendingId);
          if (!disposed && !denied) await refresh(true);
        }
      })();
      decisionJobs.set(pendingId, job);
      return job;
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      lifetime.abort();
      if (timer !== undefined) env.clearTimeout(timer);
      unsubscribeVisibility();
      listeners.clear();
      // Preserve a stable detached empty snapshot for references retained by a
      // view while React swaps to the next authority-scoped resource.
      snapshot = { scopeKey: input.scopeKey, status: "unavailable", requests: [], decisions: {}, readOnly: true };
    },
  };
  schedule();
  void refresh(false);
  return resource;
}
