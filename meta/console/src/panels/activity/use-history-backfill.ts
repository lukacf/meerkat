// Loading the day's history for Today and Logs (console v2 spec B4): the
// most active agents' recent history (at most 24, four at a time), each
// agent once per loader however many panels ask. Refresh asks again.

import React from "react";
import type { ConsoleAgent } from "../../types";
import { backfillOrder, inBatches } from "./history-frames";

const BACKFILL_AGENTS = 24;
const BACKFILL_CONCURRENCY = 4;

/** Agents each loader has loaded (or is loading), shared by the panels. */
const LOADED = new WeakMap<(identity: string) => Promise<void>, Set<string>>();

export interface HistoryBackfill {
  loading: boolean;
  /** At least one round of loading has finished. */
  loaded: boolean;
  /** Load every agent's history again. */
  refresh: () => void;
}

export function useHistoryBackfill(
  agents: readonly ConsoleAgent[] | undefined,
  loadHistory: ((identity: string) => Promise<void>) | undefined,
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
    void inBatches(wanted, BACKFILL_CONCURRENCY, loadHistory).finally(() => {
      setLoading(false);
      setLoaded(true);
    });
  }, [agentKeys, loadHistory]);
  React.useEffect(() => {
    run(false);
  }, [run]);
  const refresh = React.useCallback(() => run(true), [run]);
  return { loading, loaded, refresh };
}
