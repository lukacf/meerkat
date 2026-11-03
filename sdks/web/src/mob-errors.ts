import type { WireMobQuarantineClass } from './generated/mob.js';

/**
 * `error.data.kind` of the refusal a call that addresses a quarantined
 * persisted mob gets (see `mobQuarantined`).
 */
export const MOB_QUARANTINED_KIND = 'mob_quarantined';

/**
 * The quarantine classes this SDK knows, checked against the generated
 * vocabulary at compile time. A runtime may send others.
 */
export const MOB_QUARANTINE_CLASSES = [
  'store_unavailable',
  'store_unreadable',
  'resume_refused',
] as const satisfies readonly WireMobQuarantineClass[];

/**
 * A call addressed a quarantined persisted mob: its store did not restore
 * and is kept exactly as found, and no actor runs for it.
 */
export interface MobQuarantinedRefusal {
  readonly mobId: string;
  /**
   * Why it is quarantined (the wire `class`): `store_unavailable` (a store
   * could not serve now: `retryable` is true, and the next call that
   * addresses the mob re-attempts its restore once), `store_unreadable` (the
   * store or the state its restore needs cannot be read or decoded) or
   * `resume_refused` (another typed refusal of its restore). A class this
   * SDK does not know is kept as sent (see `MOB_QUARANTINE_CLASSES`).
   */
  readonly quarantineClass: string;
  /** Whether a later attempt may restore the mob with no repair. */
  readonly retryable: boolean;
  /** The failure the last restore attempt met. */
  readonly cause: string;
  /** The error code the refusal carries. */
  readonly code?: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * The typed refusal of a call that addressed a quarantined mob.
 *
 * A mob call that addresses a quarantined mob by id rejects with a
 * `MeerkatError` whose `data` is `{kind: 'mob_quarantined', mob_id, class,
 * retryable, cause, code, ...}`. Branch on it, never on the error text.
 * `undefined` for any other error, or when the data does not parse. The
 * browser runtime keeps no persisted mobs, so it only meets this refusal
 * from a host that relays one.
 */
export function mobQuarantined(error: unknown): MobQuarantinedRefusal | undefined {
  if (typeof error !== 'object' || error === null) return undefined;
  const data = (error as { data?: unknown }).data;
  if (!isRecord(data) || data.kind !== MOB_QUARANTINED_KIND) return undefined;
  const mobId = data.mob_id;
  const quarantineClass = data.class;
  const retryable = data.retryable;
  const cause = data.cause;
  const code = data.code;
  if (
    typeof mobId !== 'string' ||
    typeof quarantineClass !== 'string' ||
    typeof retryable !== 'boolean' ||
    typeof cause !== 'string' ||
    (code !== undefined && code !== null && typeof code !== 'string')
  ) {
    return undefined;
  }
  return {
    mobId,
    quarantineClass,
    retryable,
    cause,
    ...(typeof code === 'string' ? { code } : {}),
  };
}
