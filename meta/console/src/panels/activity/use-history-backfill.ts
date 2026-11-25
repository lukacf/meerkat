// Loading the day's history for Today and Logs (console v2 spec B4): the
// most active agents' recent history (at most 24, four at a time). An agent
// is asked once per loader however many panels ask: a read that was made
// counts as loaded, and so does one that was tried and failed. Only an
// agent whose load answered that no read was tried (its mob is listed as
// not loaded) is asked again, when the roster or the set of loaded mobs
// next changes. Refresh asks every agent again.

import React from "react";
import type { ConsoleAgent } from "../../types";
import { backfillOrder, inBatches } from "./history-frames";

const BACKFILL_AGENTS = 24;
const BACKFILL_CONCURRENCY = 4;

/**
 * Loads one agent's history. It answers false when no read was made: the
 * console makes none on its own for an agent whose mob is listed as not
 * loaded. Such an agent is not counted as loaded, and is asked again when
 * the roster next changes. Any other answer is a read that was made.
 */
export type HistoryLoader = (identity: string) => Promise<boolean | void>;

/** Agents each loader has read (or is reading), shared by the panels. */
const LOADED = new WeakMap<HistoryLoader, Set<string>>();

export interface HistoryBackfill {
  loading: boolean;
  /** At least one round of loading has finished. */
  loaded: boolean;
  /** Load every agent's history again. */
  refresh: () => void;
}

export function useHistoryBackfill(
  agents: readonly ConsoleAgent[] | undefined,
  loadHistory: HistoryLoader | undefined,
  /**
   * A value that changes when the set of loaded mobs does. The page keeps
   * its roster when no row of it differs, so a mob can load with every
   * listed row as it was: an agent that was not read is asked again when
   * this value changes too.
   */
  loadedMobs?: string,
): HistoryBackfill {
  const [loading, setLoading] = React.useState(false);
  const [loaded, setLoaded] = React.useState(false);
  const agentKeys = React.useMemo(
    () => (agents ?? []).map((agent) => ({ identity: (agent.identity || agent.member_id || "").trim(), lastActivityMs: agent.last_activity_ms })),
    [agents],
  );
  const run = React.useCallback((force: boolean) => {
    if (!loadHistory) return;
    let done = LOADED.get(loadHistory);
    if (!done) {
      done = new Set();
      LOADED.set(loadHistory, done);
    }
    if (force) done.clear();
    const wanted = backfillOrder(agentKeys, BACKFILL_AGENTS).filter((identity) => !done!.has(identity));
    if (!wanted.length) {
      setLoaded(true);
      return;
    }
    for (const identity of wanted) done.add(identity);
    setLoading(true);
    const reading = done;
    void inBatches(wanted, BACKFILL_CONCURRENCY, async (identity) => {
      // An agent whose history was not read is not counted as loaded.
      if ((await loadHistory(identity)) === false) reading.delete(identity);
    }).finally(() => {
      setLoading(false);
      setLoaded(true);
    });
  }, [agentKeys, loadHistory]);
  React.useEffect(() => {
    run(false);
  }, [run, loadedMobs]);
  const refresh = React.useCallback(() => run(true), [run]);
  return { loading, loaded, refresh };
}
