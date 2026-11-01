// The one status-to-tone map for every panel (design brief 3.3, console v2
// spec A2.4). Tones pick the fill, mark and text tokens in panel-kit.css.
// Every tone has a shape too (StatusMark), so colour never carries meaning
// alone. `live` is the accent's own meaning, "happening now": it is told
// apart from ok and info by its pulse ring, never by hue alone (in the
// terminal variant the accent is the ok green, in graphite the link blue).

export type Tone = "neutral" | "info" | "ok" | "warn" | "crit" | "locked" | "live";

const TONE_WORDS: Record<Exclude<Tone, "neutral">, readonly string[]> = {
  live: [
    "in_progress",
    "running",
    "working",
    "busy",
    "streaming",
    "thinking",
    "starting",
    "processing",
    "claimed",
    "replying",
    "delivering",
  ],
  info: ["retrying", "queued", "scheduled", "updating"],
  ok: [
    "healthy",
    "active",
    "completed",
    "complete",
    "done",
    "delivered",
    "sent",
    "approved",
    "approve",
    "allowed",
    "allow",
    "holding",
    "ok",
    "ready",
    "succeeded",
    "success",
    "passed",
    "released",
    "enabled",
    "enforcing",
    "verified",
  ],
  warn: [
    "degraded",
    "blocked",
    "paused",
    "waiting",
    "waiting_for_approval",
    "awaiting_approval",
    "pending_approval",
    "quarantined",
    "held",
    "near_limit",
    "stale",
    "retiring",
    "warning",
    "partial",
    "conflict",
    "escalated",
    "escalate",
  ],
  crit: [
    "stalled",
    "wedged",
    "failed",
    "failure",
    "delivery_failed",
    "dead_letter",
    "violated",
    "rejected",
    "reject",
    "denied",
    "deny",
    "error",
    "errored",
    "broken",
    "timed_out",
  ],
  locked: [
    "no_grant",
    "no_access",
    "unavailable",
    "not_checked",
    "unverifiable",
    "forbidden",
    "not_configured",
  ],
};

const TONE_BY_WORD: ReadonlyMap<string, Tone> = new Map(
  (Object.entries(TONE_WORDS) as Array<[Tone, readonly string[]]>).flatMap(([tone, words]) =>
    words.map((word) => [word, tone] as const),
  ),
);

/** Normalize a status word: lower case, spaces and hyphens as underscores. */
export function normalizeStatusWord(value: string | null | undefined): string {
  return (value ?? "").trim().toLowerCase().replace(/[\s-]+/g, "_");
}

/** The tone for a status word. Unknown words, and the deliberately quiet ones
 * (open, idle, pending, superseded, retired, cancelled), are neutral. */
export function statusTone(value: string | null | undefined): Tone {
  return TONE_BY_WORD.get(normalizeStatusWord(value)) ?? "neutral";
}

const TONE_WORD: Record<Tone, string> = {
  live: "Happening now",
  info: "Informational",
  ok: "Good",
  warn: "Needs attention",
  crit: "Failed",
  neutral: "Quiet",
  locked: "Not visible",
};

/** The plain word for a tone, for a mark that stands alone (a graph node's
 * tooltip, an accessible name). Prefer the status word itself when there is
 * one. */
export function toneWord(tone: Tone): string {
  return TONE_WORD[tone];
}

/** Priority shows only when it matters: high is a critical outline chip, low
 * a neutral one, and medium (or no priority) shows nothing. */
export function priorityTone(priority: string | null | undefined): Tone | null {
  const word = normalizeStatusWord(priority);
  if (word === "high" || word === "urgent" || word === "critical") return "crit";
  if (word === "low") return "neutral";
  return null;
}

/** Risk tiers in words: R0 Minimal, R1 Low, R2 Elevated, R3 High. Null for
 * anything else, which callers show as recorded. */
export function riskTierWord(tier: string | null | undefined): string | null {
  switch ((tier ?? "").trim().toLowerCase()) {
    case "r0": return "Minimal";
    case "r1": return "Low";
    case "r2": return "Elevated";
    case "r3": return "High";
    default: return null;
  }
}

/** The tone of a risk level (approvals): high critical, medium warning, low
 * neutral. Unknown levels are neutral. */
export function riskTone(risk: string | null | undefined): Tone {
  const word = normalizeStatusWord(risk);
  if (word === "high" || word === "critical" || word === "r3" || word === "r4") return "crit";
  if (word === "medium" || word === "r2") return "warn";
  return "neutral";
}
