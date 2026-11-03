/**
 * Equivalence oracle for incremental transcript derivation.
 *
 * `timeline-incremental-oracle.cjs` rebuilds the adapter test suites with
 * `./adapters` resolved here, so every frame sequence those suites craft also
 * proves that continuing a derivation equals deriving from scratch: for each
 * split point the prefix is derived, then extended with the rest (once, and
 * one frame at a time), and the entries must be deeply equal to the full
 * derivation. Extension falls back to a full derivation whenever a guard
 * cannot prove equivalence, so equality must hold either way; the counters
 * show how often the incremental path itself was exercised.
 */
import { deepStrictEqual } from "node:assert/strict";

import * as adapters from "./adapters";
import type { TimelineDerivationOptions } from "./adapters";
import type { ConsoleAgent, ConsoleFrame } from "../types";

export * from "./adapters";

const MAX_EXHAUSTIVE_FRAMES = 120;

export const incrementalOracleStats = { calls: 0, checks: 0, extended: 0 };

function splitPoints(length: number): number[] {
  if (length <= MAX_EXHAUSTIVE_FRAMES) return Array.from({ length: Math.max(0, length - 1) }, (_, i) => i + 1);
  const step = Math.ceil(length / 60);
  const points = new Set<number>();
  for (let k = 1; k < length; k += step) points.add(k);
  for (let k = Math.max(1, length - 20); k < length; k++) points.add(k);
  return [...points].sort((a, b) => a - b);
}

function derive(
  agent: ConsoleAgent | null,
  frames: ConsoleFrame[],
  options: TimelineDerivationOptions,
  previous?: adapters.TimelineDerivation,
): adapters.TimelineDerivation {
  const resumable = previous?.resume ?? null;
  const next = adapters.deriveTimelineEntries(agent, frames, options, previous);
  if (resumable && previous && previous.resume === null) incrementalOracleStats.extended += 1;
  return next;
}

export function mapFramesToTimelineEntries(
  agent: ConsoleAgent | null,
  frames: ConsoleFrame[],
  options: TimelineDerivationOptions = {},
) {
  incrementalOracleStats.calls += 1;
  const full = adapters.deriveTimelineEntries(agent, frames, options).entries;
  const points = splitPoints(frames.length);
  for (const k of points) {
    const prefix = derive(agent, frames.slice(0, k), options);
    const extended = derive(agent, frames, options, prefix);
    incrementalOracleStats.checks += 1;
    deepStrictEqual(extended.entries, full, `extending a derivation of ${k} frames to ${frames.length} diverged`);
  }
  if (points.length) {
    let chained = derive(agent, frames.slice(0, points[0]), options);
    for (let k = points[0] + 1; k <= frames.length; k++) {
      chained = derive(agent, frames.slice(0, k), options, chained);
    }
    incrementalOracleStats.checks += 1;
    deepStrictEqual(chained.entries, full, `extending one frame at a time from ${points[0]} frames diverged`);
  }
  return full;
}

process.on("exit", () => {
  if (incrementalOracleStats.calls) {
    process.stdout.write(
      `# incremental-oracle derivations=${incrementalOracleStats.calls} equivalence-checks=${incrementalOracleStats.checks} incremental-extensions=${incrementalOracleStats.extended}\n`,
    );
  }
});
