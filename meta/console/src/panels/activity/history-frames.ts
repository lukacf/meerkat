// The agents' history logs as one frame list for Today and Logs (console v2
// spec B4, B5): every frame the console has loaded per agent (live frames
// and backfilled history pages), concatenated once per change. The console
// keeps one log per identity and bumps its version on every change, so the
// list is rebuilt only when a log's version moves. Pure module (no React).

import type { ConsoleFrame } from "../../types";

export interface HistoryLogLike {
  readonly events: readonly ConsoleFrame[];
  readonly version: number;
}

const CACHE = new WeakMap<object, { signature: string; frames: ConsoleFrame[] }>();

/** The frames of every log, the same array while no log changed. */
export function historyFramesOf(logs: Readonly<Record<string, HistoryLogLike>>): ConsoleFrame[] {
  let signature = "";
  for (const [identity, log] of Object.entries(logs)) signature += `${identity}:${log.version}:${log.events.length}|`;
  const held = CACHE.get(logs);
  if (held && held.signature === signature) return held.frames;
  const frames: ConsoleFrame[] = [];
  for (const log of Object.values(logs)) frames.push(...log.events);
  CACHE.set(logs, { signature, frames });
  return frames;
}

/**
 * The agents whose history Today loads first: the most active (recent
 * activity first, then the roster's order), at most `limit`.
 */
export function backfillOrder(
  agents: ReadonlyArray<{ identity: string; lastActivityMs?: number }>,
  limit = 24,
): string[] {
  return agents
    .map((agent, index) => ({ ...agent, index }))
    .filter((agent) => agent.identity && agent.identity !== "_system")
    .sort((a, b) => (b.lastActivityMs ?? 0) - (a.lastActivityMs ?? 0) || a.index - b.index)
    .slice(0, Math.max(0, limit))
    .map((agent) => agent.identity);
}

/** Run `task` over `items`, at most `concurrency` at a time; settles when all have. */
export async function inBatches<T>(items: readonly T[], concurrency: number, task: (item: T) => Promise<unknown>): Promise<void> {
  let next = 0;
  const workers = Array.from({ length: Math.max(1, Math.min(concurrency, items.length)) }, async () => {
    while (next < items.length) {
      const item = items[next++];
      try {
        await task(item);
      } catch {
        // One agent's history failing to load leaves the others.
      }
    }
  });
  await Promise.all(workers);
}
