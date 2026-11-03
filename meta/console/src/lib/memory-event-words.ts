// Memory events in words people use, for the Memory live strip and the
// Signals rail: agents by name, records by title, scopes as "Research
// Analyst's memory" or "team memory", no session keys or run ids. The
// shared describeMemoryTimelineEvent (lib/adapters.ts) keeps the exact
// technical line; panels show it as the tooltip. Pure module: the Signals
// rail's test bundle cannot resolve React.

export interface MemoryEventNames {
  /** Display name for an identity key; the key itself when unknown. */
  agentName?: (key: string) => string;
  /** A record's title by memory id, when the panel knows it. */
  recordTitle?: (memoryId: string) => string | undefined;
}

function text(data: Record<string, unknown>, key: string): string {
  const value = data[key];
  return typeof value === "string" ? value.trim() : "";
}

function count(data: Record<string, unknown>, key: string): number | null {
  const value = data[key];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

function words(value: string): string {
  return value.replace(/[_.\-]+/g, " ").replace(/\s+/g, " ").trim().toLowerCase();
}

function sentence(value: string): string {
  return value ? value.charAt(0).toUpperCase() + value.slice(1) : value;
}

function withDetail(line: string, detail: string): string {
  return detail ? `${line} · ${detail}` : line;
}

/** Who wrote or proposed a memory, as the console reads it. */
export type MemoryAuthorRef =
  | { author: "agent"; identity: string }
  | { author: "steward"; run_id?: string }
  | { author: "distiller"; run_id?: string }
  | { author: "operator" }
  | { author: "application" };

const DEBUG_RECORD_AUTHOR =
  /^\s*(Agent|Steward|Distiller)\s*\{\s*(identity|run_id)\s*:\s*"((?:[^"\\]|\\.)*)"\s*,?\s*\}\s*$/;
const DEBUG_UNIT_AUTHOR = /^\s*(Operator|Application)\s*$/;

/** The text of a Rust Debug string literal's body: `\"` reads `"`. */
function unescapeDebugText(text: string): string {
  return text.replace(/\\(u\{([0-9a-fA-F]{1,6})\}|.)/g, (_match, escape: string, hex?: string) => {
    if (hex) {
      const code = Number.parseInt(hex, 16);
      return Number.isFinite(code) && code <= 0x10ffff ? String.fromCodePoint(code) : "";
    }
    switch (escape) {
      case "n":
        return "\n";
      case "r":
        return "\r";
      case "t":
        return "\t";
      case "0":
        return "\0";
      default:
        return escape;
    }
  });
}

/**
 * A memory author from what the server sends. Proposal authors and the
 * `memory.write.quarantined` event's author arrive as Rust Debug text
 * (`Agent { identity: "billing" }`, `Steward { run_id: "dream-7" }`,
 * `Operator`) until the server sends the structured author (S5); both read
 * the same here. A bare key reads as the agent it names. Null when there is
 * nothing to read.
 */
export function parseMemoryAuthor(value: unknown): MemoryAuthorRef | null {
  if (value && typeof value === "object") {
    const author = (value as { author?: unknown }).author;
    const identity = (value as { identity?: unknown }).identity;
    const runId = (value as { run_id?: unknown }).run_id;
    switch (author) {
      case "agent":
        return typeof identity === "string" && identity.trim() ? { author: "agent", identity: identity.trim() } : null;
      case "steward":
      case "distiller":
        return typeof runId === "string" && runId ? { author, run_id: runId } : { author };
      case "operator":
      case "application":
        return { author };
      default:
        return null;
    }
  }
  if (typeof value !== "string") return null;
  const record = DEBUG_RECORD_AUTHOR.exec(value);
  if (record) {
    const [, variant, field, raw] = record;
    const text = unescapeDebugText(raw).trim();
    if (variant === "Agent") return field === "identity" && text ? { author: "agent", identity: text } : null;
    if (field !== "run_id") return null;
    const kind = variant === "Steward" ? "steward" : "distiller";
    return text ? { author: kind, run_id: text } : { author: kind };
  }
  const unit = DEBUG_UNIT_AUTHOR.exec(value);
  if (unit) return { author: unit[1] === "Operator" ? "operator" : "application" };
  const key = value.trim();
  if (!key || /[{}"]/.test(key)) return null;
  return { author: "agent", identity: key };
}

/** An author in a sentence: the agent's name, "the memory steward", "an
 * operator". Run ids and keys never show. */
export function memoryAuthorWords(author: MemoryAuthorRef, names: MemoryEventNames = {}): string {
  switch (author.author) {
    case "agent":
      return names.agentName ? names.agentName(author.identity) : author.identity;
    case "steward":
      return "the memory steward";
    case "distiller":
      return "the memory distiller";
    case "operator":
      return "an operator";
    case "application":
      return "the application";
  }
}

/** "Research Analyst's memory", "team memory", "operator memory",
 * "shared memory"; empty when the event names no scope. */
export function memoryScopeWords(data: Record<string, unknown>, names: MemoryEventNames = {}): string {
  const kind = text(data, "scope_kind");
  const key = text(data, "scope_key");
  const name = (identity: string) => (names.agentName ? names.agentName(identity) : identity);
  switch (kind) {
    case "identity":
      return key ? `${name(key)}'s memory` : "an agent's memory";
    case "mob":
      return "team memory";
    case "operator":
      return "operator memory";
    case "realm":
      return "shared memory";
    default:
      return kind ? `${words(kind)} memory` : "";
  }
}

const VERDICT_WORDS: Readonly<Record<string, string>> = {
  release: "released",
  released: "released",
  tombstone: "discarded",
  tombstoned: "discarded",
  keep: "kept in quarantine",
  hold: "kept in quarantine",
  promote: "moved to team memory",
  promote_pending_gate: "sent for approval",
};

/** The verdict the steward's open-loop nudge carries on the quarantine
 * verdict event. It names a stale open loop and decides nothing about
 * quarantine. */
export const OPEN_LOOP_NUDGE_VERDICT = "open_loop_escalated";

/** Whether a memory event is the steward's open-loop nudge. */
export function isOpenLoopNudge(event: string, data: Record<string, unknown>): boolean {
  return event === "memory.quarantine.verdict" && text(data, "verdict") === OPEN_LOOP_NUDGE_VERDICT;
}

/** A quarantine verdict in words: "discarded", "released", "kept in
 * quarantine", "moved to team memory". */
export function quarantineVerdictWords(verdict: string): string {
  return VERDICT_WORDS[verdict] ?? (verdict ? words(verdict) : "decided");
}

export function describeMemoryEventForPeople(
  event: string,
  data: Record<string, unknown>,
  names: MemoryEventNames = {},
): string {
  const name = (identity: string) => (names.agentName ? names.agentName(identity) : identity);
  const record = (memoryId: string) => {
    const title = memoryId ? names.recordTitle?.(memoryId) : undefined;
    return title ? `“${title}”` : "a quarantined record";
  };
  switch (event) {
    case "memory.dream.started":
      return "Dream started";
    case "memory.dream.completed": {
      const ops = count(data, "ops_committed");
      return withDetail(ops === null ? "Dream finished" : `Dream finished · ${plural(ops, "change")}`, text(data, "detail"));
    }
    case "memory.dream.skipped":
      return withDetail("Dream skipped", text(data, "reason"));
    case "memory.record.promoted": {
      const scope = memoryScopeWords(data, names);
      const line = scope ? `A record moved to ${scope}` : "A record was moved";
      return data.gated === true ? `${line} after approval` : line;
    }
    case "memory.quarantine.verdict": {
      if (isOpenLoopNudge(event, data)) {
        const title = names.recordTitle?.(text(data, "record_id"));
        return title ? `“${title}” needs follow-up` : "An open loop needs follow-up";
      }
      return withDetail(`Quarantine verdict: ${quarantineVerdictWords(text(data, "verdict"))}`, text(data, "rationale"));
    }
    case "memory.quarantine.release_blocked": {
      const subject = record(text(data, "record_id"));
      const line = text(data, "verdict") === "promote_pending_gate"
        ? `Moving ${subject} was blocked`
        : `Release of ${subject} blocked`;
      return text(data, "class") ? `${line} · it looks like a secret` : line;
    }
    case "memory.conflict.signal": {
      const subject = [text(data, "entity"), text(data, "topic")].filter(Boolean).join(", ");
      return withDetail(subject ? `Conflicting memories about ${subject}` : "Conflicting memories", text(data, "reason"));
    }
    case "memory.write.quarantined": {
      const author = parseMemoryAuthor(data.author);
      return withDetail(
        author ? `A memory write from ${memoryAuthorWords(author, names)} was quarantined` : "A memory write was quarantined",
        text(data, "reason"),
      );
    }
    case "memory.taint.transition": {
      const kind = text(data, "kind");
      const line = kind === "tainted"
        ? "A session became untrusted"
        : kind === "reset_boundary"
          ? "Session trust was reset"
          : kind === "rotated_clean"
            ? "A session started clean"
            : `Session trust ${kind ? words(kind) : "changed"}`;
      return withDetail(line, text(data, "source"));
    }
    case "memory.budget.denied": {
      const stage = text(data, "stage");
      return withDetail(stage ? `Memory budget reached during ${words(stage)}` : "Memory budget reached", text(data, "reason"));
    }
    case "memory.promotion.pending_gate": {
      const scope = memoryScopeWords(data, names);
      return scope ? `A record waits for approval to move to ${scope}` : "A record waits for approval to move";
    }
    case "memory.harvest.completed": {
      const kept = count(data, "promoted");
      const discarded = count(data, "tombstoned");
      const identity = text(data, "identity");
      const parts = [kept !== null ? `${kept} kept` : "", discarded !== null ? `${discarded} discarded` : ""].filter(Boolean);
      const line = identity ? `${name(identity)}'s memory was summarized` : "A retired agent's memory was summarized";
      return withDetail(line, parts.join(", "));
    }
    case "memory.distill.timed_out":
      return withDetail("Summarizing a conversation timed out", text(data, "cause"));
    case "memory.hygiene.proposed":
    case "memory.hygiene.applied":
    case "memory.hygiene.blocked":
    case "memory.hygiene.skipped": {
      const phase = event.slice("memory.hygiene.".length);
      const ops = count(data, "ops");
      const detail = text(data, "reason") || (ops !== null ? plural(ops, "change") : "") || text(data, "cause");
      return withDetail(`Memory tidy-up ${phase}`, detail);
    }
    default: {
      const suffix = event.startsWith("memory.") ? event.slice("memory.".length) : event;
      const detail = text(data, "reason") || text(data, "detail") || text(data, "cause") || text(data, "verdict");
      return withDetail(sentence(`memory ${words(suffix)}`.trim()), detail);
    }
  }
}
