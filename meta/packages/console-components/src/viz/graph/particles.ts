// Messages as particles (design spec A3.9, the brand's net.js): an accent dot
// travels the edge with an in-out ease over clamp(450 ms, distance / 0.26,
// 1400 ms), trailing a faint segment of 12 % of the path; landing flashes the
// target. At most 12 fly at once; the rest add to the edge's count badge.

import type { VizMessage } from "./types";

export const PARTICLES = {
  max: 12,
  minMs: 450,
  maxMs: 1400,
  /** px per ms (the brand: 260 px per second at scale 1). */
  speed: 0.26,
  trail: 0.12,
  /** Messages older than this at first sight are history, not played. */
  staleMs: 10_000,
  flashMs: 500,
  /** Reduced motion: how long the edge stays strong instead of a particle. */
  boostMs: 500,
  badgeMs: 2400,
  memoryMs: 900,
} as const;

/** Travel time for a screen distance in px. */
export function travelMs(distance: number): number {
  return Math.max(PARTICLES.minMs, Math.min(PARTICLES.maxMs, distance / PARTICLES.speed));
}

/** The brand's quadratic in-out ease. */
export function easeTravel(p: number): number {
  if (p <= 0) return 0;
  if (p >= 1) return 1;
  return p < 0.5 ? 2 * p * p : 1 - (-2 * p + 2) ** 2 / 2;
}

export function messageKey(message: VizMessage): string {
  return message.id ?? `${message.from}>${message.to}@${message.at}`;
}

export interface Flight {
  key: string;
  from: string;
  to: string;
  /** The edge it travels, when one joins the pair. */
  edgeId: string | null;
  start: number;
  duration: number;
}

/**
 * Which new messages fly, which only count: unseen and fresh messages, in
 * time order, up to the free slots; the rest are returned as overflow.
 * Marks every given message as seen.
 */
export function admitMessages(
  messages: readonly VizMessage[],
  seen: Set<string>,
  inFlight: number,
  wallNow: number,
): { fly: VizMessage[]; overflow: VizMessage[] } {
  const fresh: VizMessage[] = [];
  for (const message of messages) {
    const key = messageKey(message);
    if (seen.has(key)) continue;
    seen.add(key);
    if (wallNow - message.at > PARTICLES.staleMs) continue;
    fresh.push(message);
  }
  fresh.sort((a, b) => a.at - b.at);
  const free = Math.max(0, PARTICLES.max - inFlight);
  return { fly: fresh.slice(0, free), overflow: fresh.slice(free) };
}

/** Bounded memory of played keys: forget the oldest beyond `limit`. */
export function trimSeen(seen: Set<string>, limit = 4000): void {
  if (seen.size <= limit) return;
  const drop = seen.size - limit;
  let index = 0;
  for (const key of seen) {
    if (index >= drop) break;
    seen.delete(key);
    index += 1;
  }
}
