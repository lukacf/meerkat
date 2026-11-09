/**
 * Meerkat member-kickoff status notices: a typed comms notice whose intent is
 * `mob.kickoff_<phase>`. Current meerkat sends them as one-way `lifecycle`
 * notices (#1608); older sessions keep the `request` form. Both carry the
 * member in their typed payload and model-facing routing text as content,
 * which is never shown.
 */
export const MEMBER_KICKOFF_PHASES = [
  "pending",
  "starting",
  "started",
  "callback_pending",
  "failed",
  "cancelled",
] as const;

export type MemberKickoffPhase = (typeof MEMBER_KICKOFF_PHASES)[number];

export interface MemberKickoffNotice {
  phase: MemberKickoffPhase;
  /** The member being kicked off: the typed payload's `peer`, else the sender. */
  member: string;
  role?: string;
  /** The sending member's comms peer id. */
  peerIdentity?: string;
}

const PHASE_LABELS: Record<MemberKickoffPhase, string> = {
  pending: "Pending",
  starting: "Starting",
  started: "Started",
  callback_pending: "Waiting for callback",
  failed: "Failed",
  cancelled: "Cancelled",
};

export function memberKickoffPhaseLabel(phase: MemberKickoffPhase): string {
  return PHASE_LABELS[phase];
}

const text = (value: unknown): string => (typeof value === "string" ? value.trim() : "");
const record = (value: unknown): Record<string, unknown> =>
  value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};

/** The typed kickoff status a comms notice block carries, or null for any other block. */
export function memberKickoffNotice(block: Record<string, unknown>): MemberKickoffNotice | null {
  if (block.type !== "comms" || (block.kind !== "lifecycle" && block.kind !== "request")) return null;
  const match = /^mob\.kickoff_([a-z_]+)$/.exec(text(block.intent));
  const phase = match?.[1] as MemberKickoffPhase | undefined;
  if (!phase || !(MEMBER_KICKOFF_PHASES as readonly string[]).includes(phase)) return null;
  const payload = record(block.payload);
  const peer = record(block.peer);
  const displayName = text(peer.display_name);
  const member = text(payload.peer) || displayName.split("/").pop() || text(peer.id) || "member";
  const role = text(payload.role);
  const peerIdentity = text(peer.id);
  return {
    phase,
    member,
    ...(role ? { role } : {}),
    ...(peerIdentity ? { peerIdentity } : {}),
  };
}
