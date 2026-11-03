import React from "react";
import { queryVoiceAvailability, type VoiceAvailability } from "./voice-session";

/**
 * What the console knows about one identity's voice readiness.
 * - `checking`: the first check is in flight; nothing is known yet.
 * - `retrying`: a check failed transiently (network, timeout, the gateway's typed
 *   `voice_readiness_timed_out`) and nothing definite was ever known; a retry is scheduled.
 * - `available` / `unavailable`: the gateway's last definite answer.
 */
export type VoiceReadinessState = "checking" | "retrying" | "available" | "unavailable";

const NO_READINESS: Readonly<Record<string, VoiceReadinessState>> = {};
/** Refresh cadence after a definite answer. */
export const READINESS_REFRESH_INTERVAL_MS = 15_000;
/** First retry after a failed check; doubles per consecutive failure up to the refresh cadence. */
export const READINESS_RETRY_BASE_MS = 1_000;

/**
 * Fold one check result into the identity's state. A definite answer replaces it; a failed
 * check keeps a definite answer and otherwise becomes `retrying`, never a denial.
 */
export function nextVoiceReadiness(
  previous: VoiceReadinessState | undefined,
  availability: VoiceAvailability,
): VoiceReadinessState {
  if (availability === "available") return "available";
  if (availability === "unavailable") return "unavailable";
  return previous === "available" || previous === "unavailable" ? previous : "retrying";
}

/** Delay before the next check: prompt backoff after failures, the steady cadence otherwise. */
export function voiceReadinessRetryDelayMs(consecutiveFailures: number): number {
  if (consecutiveFailures <= 0) return READINESS_REFRESH_INTERVAL_MS;
  const exponent = Math.min(consecutiveFailures - 1, 16);
  return Math.min(READINESS_RETRY_BASE_MS * 2 ** exponent, READINESS_REFRESH_INTERVAL_MS);
}

/** Voice must close only when the gateway definitely said no, never because a check failed. */
export function voiceReadinessDenied(
  readiness: Readonly<Record<string, VoiceReadinessState>>,
  identity: string,
): boolean {
  return readiness[identity] === "unavailable";
}

/**
 * Whether the voice button is shown. It appears at once while the first check runs (and while
 * a failed check retries) so the operator sees voice is coming; only a definite no hides it.
 * Starting voice always freshly confirms readiness with the gateway.
 */
export function voiceReadinessOffersVoice(state: VoiceReadinessState | undefined): boolean {
  return state !== undefined && state !== "unavailable";
}

/** Whether a check is still pending for the identity (no definite answer yet). */
export function voiceReadinessPending(state: VoiceReadinessState | undefined): boolean {
  return state === "checking" || state === "retrying";
}

/**
 * Per-identity voice readiness for the focused chat and the ongoing voice target.
 *
 * Each identity has one check loop: at most one request is in flight per identity, the next
 * check starts only after the previous one settled, a failed check retries with backoff
 * (1 s, 2 s, 4 s, ... capped at the 15 s refresh cadence), and leaving the scope aborts the
 * in-flight request so the gateway can drop the work. An identity with no answer yet reads
 * as `checking` from the first render.
 */
export function useVoiceReadiness(
  baseUrl: string,
  focusedIdentity: string | null,
  voiceIdentity: string | null,
  enabled: boolean,
): Readonly<Record<string, VoiceReadinessState>> {
  const identities = React.useMemo(
    () => enabled
      ? [...new Set([focusedIdentity, voiceIdentity].filter(
        (identity): identity is string => Boolean(identity),
      ))]
      : [],
    [enabled, focusedIdentity, voiceIdentity],
  );
  const [state, setState] = React.useState<{
    baseUrl: string;
    values: Readonly<Record<string, VoiceReadinessState>>;
  }>({ baseUrl: "", values: NO_READINESS });

  React.useEffect(() => {
    if (!identities.length) return;
    const abort = new AbortController();
    const timers = new Set<ReturnType<typeof setTimeout>>();
    for (const identity of identities) {
      let failures = 0;
      const check = async () => {
        const availability = await queryVoiceAvailability(baseUrl, identity, abort.signal);
        if (abort.signal.aborted) return;
        failures = availability === "unknown" ? failures + 1 : 0;
        setState((previous) => {
          // Known values never transfer across gateways, even for the same identity.
          const known = previous.baseUrl === baseUrl ? previous.values : NO_READINESS;
          const values: Record<string, VoiceReadinessState> = {};
          for (const kept of identities) {
            if (kept in known) values[kept] = known[kept];
          }
          values[identity] = nextVoiceReadiness(known[identity], availability);
          return { baseUrl, values };
        });
        const timer = setTimeout(() => {
          timers.delete(timer);
          void check();
        }, voiceReadinessRetryDelayMs(failures));
        timers.add(timer);
      };
      void check();
    }
    return () => {
      abort.abort();
      for (const timer of timers) clearTimeout(timer);
      timers.clear();
    };
  }, [baseUrl, identities]);

  return React.useMemo(() => {
    if (!identities.length) return NO_READINESS;
    const known = state.baseUrl === baseUrl ? state.values : NO_READINESS;
    const values: Record<string, VoiceReadinessState> = {};
    for (const identity of identities) values[identity] = known[identity] ?? "checking";
    return values;
  }, [baseUrl, identities, state]);
}
