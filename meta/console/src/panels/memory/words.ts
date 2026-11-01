// Memory in words people use: kinds, trust, status, scopes, authors,
// evidence and dream changes. Pure module (no React): the panel's node test
// bundle and the jsdom lanes import it directly. The data helpers keep their
// exact outputs (tests and pivots depend on them); the display helpers turn
// the same data into words, with the store's own term kept as the tooltip.

import { accessActionLabel, isKnownAccessAction } from "../../lib/access-actions";
import { humanizeEnum, joinMeta } from "../../../../packages/console-components/src/panel-kit/format";
import type { Tone } from "../../../../packages/console-components/src/panel-kit/tone";
import { stripAgentKeyPrefix } from "../../../../packages/console-components/src/panel-kit/agents";
import type {
  MemoryAuthor,
  MemoryDreamRun,
  MemoryEvidenceRef,
  MemoryInjectionEntry,
  MemoryRecordScope,
  MemoryRecordStatus,
  MemoryScopeOverview,
  MemoryTrust,
} from "../../types";

const TRUST_LABEL: Record<MemoryTrust, string> = {
  untrusted: "untrusted",
  agent_observed: "observed",
  agent_verified: "verified",
  application: "application",
  operator: "operator",
};

/// Lattice order (§6 of the architecture): untrusted < agent_observed <
/// agent_verified < application < operator.
export const TRUST_RANK: Record<MemoryTrust, number> = {
  untrusted: 0,
  agent_observed: 1,
  agent_verified: 2,
  application: 3,
  operator: 4,
};

export function trustLabel(trust: MemoryTrust | undefined): string {
  return (trust && TRUST_LABEL[trust]) || String(trust || "unknown");
}

/// Visual tone bucket for a trust level, reusing the gpolicy state vocabulary.
export function trustTone(trust: MemoryTrust | undefined): "positive" | "neutral" | "muted" {
  switch (trust) {
    case "operator":
    case "application":
    case "agent_verified":
      return "positive";
    case "agent_observed":
      return "neutral";
    default:
      return "muted";
  }
}

export function statusLabel(status: MemoryRecordStatus | undefined): string {
  if (!status) return "unknown";
  switch (status.status) {
    case "active":
      return "active";
    case "superseded":
      return status.by ? `superseded → ${status.by}` : "superseded";
    case "quarantined":
      return status.reason ? `quarantined: ${status.reason}` : "quarantined";
    case "tombstoned":
      return "tombstoned";
  }
}

export function statusTone(
  status: MemoryRecordStatus | undefined,
): "positive" | "warning" | "muted" {
  if (!status) return "muted";
  switch (status.status) {
    case "active":
      return "positive";
    case "quarantined":
      return "warning";
    default:
      return "muted";
  }
}

const RELATIVE_UNITS: Array<[number, string]> = [
  [365 * 24 * 60 * 60 * 1000, "y"],
  [24 * 60 * 60 * 1000, "d"],
  [60 * 60 * 1000, "h"],
  [60 * 1000, "m"],
  [1000, "s"],
];

export function relativeAge(atMs: number | undefined, now = Date.now()): string {
  if (!atMs || atMs <= 0) return "-";
  const diff = now - atMs;
  if (diff < 0) return "now";
  if (diff < 1000) return "now";
  for (const [unitMs, suffix] of RELATIVE_UNITS) {
    if (diff >= unitMs) {
      return `${Math.floor(diff / unitMs)}${suffix} ago`;
    }
  }
  return "now";
}

/// Human label for a single evidence reference. Used as the click-through
/// button text and as the degraded fallback when the session is gone from
/// the timeline (the proposal's degrade rule).
export function evidenceLabel(evidence: MemoryEvidenceRef): string {
  const parts: string[] = [];
  if (evidence.session_id) parts.push(`session ${evidence.session_id}`);
  if (typeof evidence.generation === "number") parts.push(`gen ${evidence.generation}`);
  if (evidence.revision) parts.push(`rev ${evidence.revision}`);
  if (evidence.range && evidence.range.length === 2) {
    const [start, end] = evidence.range;
    parts.push(`msgs ${start}-${end}`);
  }
  return parts.join(" • ") || "evidence";
}

export function authorLine(author: MemoryAuthor | undefined): string {
  if (!author) return "unknown author";
  switch (author.author) {
    case "agent":
      return author.identity ? `agent ${author.identity}` : "agent";
    case "steward":
      return author.run_id ? `steward run ${author.run_id}` : "steward";
    case "distiller":
      return author.run_id ? `distiller run ${author.run_id}` : "distiller";
    case "operator":
      return "operator";
    case "application":
      return "application";
  }
}

/// Compact one-line summary of a dream run's op mix, e.g. "3 create · 1 tombstone".
export function dreamOpKindsSummary(opKinds: Record<string, number> | undefined): string {
  if (!opKinds) return "";
  const entries = Object.entries(opKinds)
    .filter(([, count]) => typeof count === "number" && count > 0)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  if (entries.length === 0) return "";
  return entries.map(([kind, count]) => `${count} ${kind}`).join(" · ");
}

export function dreamTimeRange(run: Pick<MemoryDreamRun, "first_op_at_ms" | "last_op_at_ms">, now = Date.now()): string {
  const first = run.first_op_at_ms;
  const last = run.last_op_at_ms;
  if (!first && !last) return "-";
  if (first && last && first !== last) {
    return `${relativeAge(first, now)} → ${relativeAge(last, now)}`;
  }
  return relativeAge(last || first, now);
}

export function injectionLine(injection: MemoryInjectionEntry, now = Date.now()): string {
  const surface = injection.surface === "build" ? "build" : "turn";
  return `${surface} • ${injection.identity} • ${relativeAge(injection.at_ms, now)}`;
}

/// The scope key a records search means: a known key typed as is, else the
/// one key whose display name equals the text (case-insensitive). Anything

/// A key resolver: an agent key to the name people know it by.
export type NameOf = (key: string) => string;

/// A size for people: "860 B", "2.2 KB", "32.0 MB".
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const KIND_LABEL: Record<string, string> = {
  preference: "Preference",
  fact: "Fact",
  gotcha: "Gotcha",
  procedure: "Procedure",
  relationship: "Relationship",
  open_loop: "Open loop",
  reference: "Reference",
};

/// A record kind in words: "Fact", "Open loop".
export function kindLabel(kind: string | undefined): string {
  if (!kind) return "Record";
  return KIND_LABEL[kind] ?? humanizeEnum(kind);
}

const TRUST_DISPLAY: Record<MemoryTrust, string> = {
  untrusted: "Untrusted",
  agent_observed: "Observed",
  agent_verified: "Verified",
  application: "Application",
  operator: "Operator",
};

/// A trust level in words: "Observed", "Verified".
export function trustDisplay(trust: MemoryTrust | undefined): string {
  if (!trust) return "Unknown trust";
  return TRUST_DISPLAY[trust] ?? humanizeEnum(trust);
}

/// Trust chips are green from verified up and neutral below; status chips
/// carry the warnings.
export function trustChipTone(trust: MemoryTrust | undefined): Tone {
  return trustTone(trust) === "positive" ? "ok" : "neutral";
}

export const STATUS_WORD: Record<MemoryRecordStatus["status"], string> = {
  active: "Active",
  superseded: "Replaced",
  quarantined: "Quarantined",
  tombstoned: "Discarded",
};

/// A record status in one word: "Active", "Replaced". The store's own
/// words (superseded, tombstoned) stay in the tooltip, statusTitle.
export function statusWord(status: MemoryRecordStatus | undefined): string {
  if (!status) return "Unknown";
  return STATUS_WORD[status.status] ?? humanizeEnum(status.status);
}

const STATUS_TITLE: Partial<Record<MemoryRecordStatus["status"], string>> = {
  superseded: "Superseded: a newer version replaced this record.",
  tombstoned: "Tombstoned: kept only as a deletion marker; it is never recalled.",
};

/// The tooltip of a status word: the store's term for it and what it means.
export function statusTitle(status: MemoryRecordStatus | undefined): string | undefined {
  return status ? STATUS_TITLE[status.status] : undefined;
}

export function statusChipTone(status: MemoryRecordStatus | undefined): Tone {
  switch (status?.status) {
    case "active":
      return "ok";
    case "quarantined":
      return "warn";
    default:
      return "neutral";
  }
}

/// Who wrote a record: "Coordinator (agent)", "The memory steward".
export function authorDisplay(
  author: MemoryAuthor | undefined,
  nameOf: NameOf = stripAgentKeyPrefix,
): string {
  if (!author) return "Unknown";
  switch (author.author) {
    case "agent":
      return author.identity ? `${nameOf(author.identity) || author.identity} (agent)` : "An agent";
    case "steward":
      return "The memory steward";
    case "distiller":
      return "The memory distiller";
    case "operator":
      return "An operator";
    case "application":
      return "The application";
    default:
      return "Unknown";
  }
}

/// What kind of memory a scope holds: "Agent memory", "Team memory".
export function scopeKindLabel(kind: string): string {
  switch (kind) {
    case "identity":
      return "Agent memory";
    case "mob":
      return "Team memory";
    case "operator":
      return "Operator memory";
    case "realm":
      return "Shared memory";
    default:
      return humanizeEnum(kind) || "Memory";
  }
}

/// The key a scope is filed under (empty for the realm-wide scope).
export function scopeKeyOf(scope: MemoryRecordScope): string {
  switch (scope.scope) {
    case "identity":
      return scope.identity;
    case "mob":
      return scope.mob;
    case "operator":
      return scope.operator;
    case "realm":
      return "";
  }
}

/// A team named from its key: "ops-team" reads "Ops team", "research" reads
/// "Research team". The key itself stays the tooltip.
export function teamName(key: string): string {
  const words = humanizeEnum(key);
  if (!words) return "Team";
  return /\bteam$/i.test(words) ? words : `${words} team`;
}

/// The owner of a scope by name: the agent's display name, the team's name
/// from its key, the operator key as given, "Everyone".
export function scopeOwnerName(kind: string, key: string, nameOf: NameOf = stripAgentKeyPrefix): string {
  switch (kind) {
    case "identity":
      return nameOf(key) || key;
    case "mob":
      return teamName(key);
    case "realm":
      return "Everyone";
    default:
      return key || humanizeEnum(kind);
  }
}

/// A records group heading: the agent's name, or the kind of memory and
/// whose it is ("Team memory · Ops team").
export function scopeHeading(scope: MemoryRecordScope, nameOf: NameOf = stripAgentKeyPrefix): string {
  switch (scope.scope) {
    case "identity":
      return nameOf(scope.identity) || scope.identity;
    case "mob":
      return joinMeta(scopeKindLabel(scope.scope), teamName(scope.mob));
    case "realm":
      return "Shared memory";
    default:
      return joinMeta(scopeKindLabel(scope.scope), scopeKeyOf(scope));
  }
}

/// A store-totals scope named in a sentence: "Support Triage",
/// "Research team", "Shared memory".
export function overviewScopeName(scope: MemoryScopeOverview, nameOf: NameOf = stripAgentKeyPrefix): string {
  switch (scope.scope_kind) {
    case "identity":
      return nameOf(scope.scope_key) || scope.scope_key;
    case "mob":
      return teamName(scope.scope_key);
    case "operator":
      return `Operator ${scope.scope_key}`;
    case "realm":
      return "Shared memory";
    default:
      return [scope.scope_kind, scope.scope_key].filter(Boolean).join(" ");
  }
}

/// A scope's record counts in words: "3 active, 1 replaced".
export function scopeCountsText(counts: {
  active?: number;
  quarantined?: number;
  superseded?: number;
  tombstoned?: number;
}): string {
  const parts = [`${counts.active ?? 0} active`];
  if ((counts.quarantined ?? 0) > 0) parts.push(`${counts.quarantined} quarantined`);
  if ((counts.superseded ?? 0) > 0) parts.push(`${counts.superseded} replaced`);
  if ((counts.tombstoned ?? 0) > 0) parts.push(`${counts.tombstoned} discarded`);
  return parts.join(", ");
}

/// Where a proposal or promotion goes: an agent by name, "Ops team",
/// "admin@example.test (operator)", "everyone".
export function targetName(kind: string, key: string, nameOf: NameOf = stripAgentKeyPrefix): string {
  switch (kind) {
    case "identity":
      return nameOf(key) || key;
    case "mob":
      return teamName(key);
    case "operator":
      return `${key} (operator)`;
    case "realm":
      return "everyone";
    default:
      return [kind, key].filter(Boolean).join(" ");
  }
}

/// An evidence reference as a sentence: "Conversation with Coordinator,
/// messages 12-18". The session, generation and revision stay technical.
export function evidenceDisplay(evidence: MemoryEvidenceRef, who?: string): string {
  const base = who ? `Conversation with ${who}` : "Conversation";
  if (evidence.range && evidence.range.length === 2) {
    const [start, end] = evidence.range;
    return start === end ? `${base}, message ${start}` : `${base}, messages ${start}-${end}`;
  }
  return base;
}

/// The technical half of an evidence reference: "generation 4".
export function evidenceDetails(evidence: MemoryEvidenceRef): string {
  return joinMeta(
    typeof evidence.generation === "number" ? `generation ${evidence.generation}` : null,
    evidence.revision ? `revision ${evidence.revision}` : null,
  );
}

/// When a record was injected: a build-surface injection happens as the
/// agent starts, a turn-surface one during a turn.
export function injectionMoment(surface: string | undefined): string {
  return surface === "build" ? "at startup" : "during a turn";
}

const REVIEW_VERDICT_LABEL: Record<string, string> = {
  dead_weight: "Never useful",
};

/// A usage-audit verdict in words: dead_weight reads "Never useful".
export function reviewVerdictLabel(verdict: string): string {
  return REVIEW_VERDICT_LABEL[verdict] ?? humanizeEnum(verdict);
}

/// The store's own words in a humanized label, said plainly: "tombstoned"
/// reads "discarded", "superseded" reads "replaced".
function plainStoreWords(words: string): string {
  return words
    .replace(/\btombstoned\b/g, "discarded")
    .replace(/\btombstone\b/g, "discard")
    .replace(/\bsuperseded\b/g, "replaced")
    .replace(/\bsupersede\b/g, "replace");
}

/// A dream run's non-zero verdict counters as chips: "5 kept", "1
/// quarantined". Counters about quarantine, blocks or dead weight warn.
export function dreamVerdictChips(
  verdicts: Array<[string, number]>,
): Array<{ key: string; label: string; tone: Tone }> {
  return verdicts.map(([name, count]) => {
    const words = plainStoreWords(humanizeEnum(name).toLowerCase());
    return {
      key: name,
      label: `${count} ${count === 1 ? singularCounterWords(words) : words}`,
      tone: /quarantin|blocked|dead|reject/.test(name) ? "warn" : "neutral",
    };
  });
}

/// A counter's words for a count of one: its first plural noun singular
/// ("1 open loop escalated", never "1 open loops escalated").
function singularCounterWords(words: string): string {
  const parts = words.split(" ");
  const index = parts.findIndex((word) => word.length > 3 && /[^su]s$/.test(word) && !/(ss|us|is)$/.test(word));
  if (index < 0) return words;
  parts[index] = parts[index].slice(0, -1);
  return parts.join(" ");
}

const OP_KIND_DONE: Record<string, string> = {
  create: "created",
  promote: "moved",
  merge: "merged",
  quarantine: "quarantined",
  supersede: "replaced",
  tombstone: "discarded",
  update: "updated",
  release: "released",
};

/// A dream run's changes by kind, most first: "3 moved, 2 merged".
export function dreamOpKindsDisplay(opKinds: Record<string, number> | undefined): string {
  if (!opKinds) return "";
  return Object.entries(opKinds)
    .filter(([, count]) => typeof count === "number" && count > 0)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([kind, count]) => `${count} ${OP_KIND_DONE[kind] ?? plainStoreWords(humanizeEnum(kind).toLowerCase())}`)
    .join(", ");
}

/// Whose memory a dream ran over, from its partition key
/// ("identity:coordinator" reads "Coordinator", "mob:ops-team" reads
/// "Ops team").
export function partitionName(partition: string | undefined, nameOf: NameOf = stripAgentKeyPrefix): string {
  if (!partition) return "";
  const separator = partition.indexOf(":");
  const kind = separator > 0 ? partition.slice(0, separator) : partition;
  const key = separator > 0 ? partition.slice(separator + 1) : "";
  if (kind === "realm") return "everyone";
  if (!key) return partition === "team" ? "the team" : partition;
  switch (kind) {
    case "identity":
    case "agent":
    case "member":
      return nameOf(partition) || key;
    case "mob":
      return teamName(key);
    case "operator":
      return `${key} (operator)`;
    default:
      return partition;
  }
}

/// A permission the reader lacks, as a sentence: "You need permission to
/// review quarantined memory". The grant's own name is the tooltip
/// (grantTitle), never the sentence.
export function needsGrantText(grant: string): string {
  if (!isKnownAccessAction(grant)) return "You need a permission you do not have";
  const words = accessActionLabel(grant);
  return `You need permission to ${words.charAt(0).toLowerCase()}${words.slice(1)}`;
}

/// The tooltip under a needsGrantText sentence: the grant's name.
export function grantTitle(grant: string): string {
  return `Grant: ${grant}`;
}

/// A realm named in a line: "ops space". Realms are the separate stores of
/// state a gateway can serve; the word "realm" stays the tooltip.
export function realmSpaceName(realm: string): string {
  return `${realm} space`;
}

/// A kind in the plural for headings: "Facts", "Open loops".
export function kindPlural(kind: string | undefined): string {
  const word = kindLabel(kind);
  return word.endsWith("s") ? word : `${word}s`;
}
