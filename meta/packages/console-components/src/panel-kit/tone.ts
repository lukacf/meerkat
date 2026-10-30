// The one status-to-tone map for every panel (design brief 3.3). Tones pick
// the fill, dot and text tokens in panel-kit.css; the member accent is never
// a status colour (it marks selection and live activity only).

export type Tone = "neutral" | "info" | "ok" | "warn" | "crit" | "locked";

const TONE_WORDS: Record<Exclude<Tone, "neutral">, readonly string[]> = {
  info: [
    "in_progress",
    "running",
    "working",
    "retrying",
    "busy",
    "streaming",
    "thinking",
    "starting",
    "processing",
  ],
  ok: [
    "healthy",
    "active",
    "completed",
    "complete",
    "done",
    "delivered",
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

/** Priority shows only when it matters: high is a critical outline chip, low
 * a neutral one, and medium (or no priority) shows nothing. */
export function priorityTone(priority: string | null | undefined): Tone | null {
  const word = normalizeStatusWord(priority);
  if (word === "high" || word === "urgent" || word === "critical") return "crit";
  if (word === "low") return "neutral";
  return null;
}

/** The tone of a risk level (approvals): high critical, medium warning, low
 * neutral. Unknown levels are neutral. */
export function riskTone(risk: string | null | undefined): Tone {
  const word = normalizeStatusWord(risk);
  if (word === "high" || word === "critical" || word === "r3" || word === "r4") return "crit";
  if (word === "medium" || word === "r2") return "warn";
  return "neutral";
}
