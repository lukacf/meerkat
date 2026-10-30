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
      const author = text(data, "author");
      return withDetail(
        author ? `A memory write from ${name(author)} was quarantined` : "A memory write was quarantined",
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
