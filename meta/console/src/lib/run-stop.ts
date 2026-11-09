import type { ConsoleFrame } from "../types";
import { countRender } from "./render-counts";

/** One input that contributed to a stopped member run. */
export interface RunStopContributor {
  input_id: string;
  completion: string;
  terminal?: string | null;
}

/**
 * meerkat's typed run-stop receipt, relayed verbatim by
 * `meta/stop_member_run`.
 */
export type RunStopReceipt =
  | { outcome: "stopped"; run_id: string; contributors: RunStopContributor[] }
  | { outcome: "not_current"; run_id: string; current_run_id?: string | null }
  | { outcome: "not_stoppable"; run_id: string; state: string };

function frameRunId(frame: ConsoleFrame): string | null {
  const direct = frame.runId?.trim();
  if (direct) return direct;
  if (frame.data && typeof frame.data === "object") {
    const identity = (frame.data as Record<string, unknown>).identity;
    if (identity && typeof identity === "object") {
      const runId = (identity as Record<string, unknown>).run_id;
      if (typeof runId === "string" && runId.trim()) return runId.trim();
    }
  }
  return null;
}

function isSteerDelivery(frame: ConsoleFrame): boolean {
  return (
    frame.event === "interaction_complete" &&
    !!frame.data &&
    typeof frame.data === "object" &&
    (frame.data as Record<string, unknown>).reason === "steer_delivered"
  );
}

function isRunTerminal(frame: ConsoleFrame): boolean {
  switch (frame.event) {
    case "run_completed":
    case "run_failed":
    case "interaction_failed":
      return true;
    case "interaction_complete":
      return !isSteerDelivery(frame);
    default:
      return false;
  }
}

/**
 * The run id of the member's in-flight run, read from the timeline: the latest
 * `run_started` (its `identity.run_id`) that no terminal frame has closed yet.
 * `null` when no run is known to be active, so the console never offers to
 * stop a run it cannot name.
 */
export function activeRunIdFromFrames(frames: readonly ConsoleFrame[]): string | null {
  // Every `run_started` that names its run replaces the active one, so only
  // the last of them and the frames after it decide: read back from the end
  // (the current run) instead of the whole timeline on every streamed token.
  let closesAny = false;
  const closed = new Set<string>();
  for (let i = frames.length - 1; i >= 0; i--) {
    const frame = frames[i];
    if (frame.event === "run_started") {
      const runId = frameRunId(frame);
      if (!runId) continue;
      countRender("ActiveRunFramesRead", frames.length - i);
      return closesAny || closed.has(runId) ? null : runId;
    }
    if (isRunTerminal(frame)) {
      // A terminal without a run id closes whichever run is active.
      const runId = frameRunId(frame);
      if (runId) closed.add(runId);
      else closesAny = true;
    }
  }
  countRender("ActiveRunFramesRead", frames.length);
  return null;
}

/** Validate the `meta/stop_member_run` result; fail closed on anything else. */
export function parseRunStopResult(result: unknown): RunStopReceipt {
  const receipt =
    result && typeof result === "object"
      ? (result as Record<string, unknown>).receipt
      : undefined;
  if (!receipt || typeof receipt !== "object") {
    throw new Error("invalid meta/stop_member_run result: missing receipt");
  }
  const record = receipt as Record<string, unknown>;
  if (typeof record.run_id !== "string") {
    throw new Error("invalid meta/stop_member_run receipt: missing run_id");
  }
  switch (record.outcome) {
    case "stopped":
      if (
        !Array.isArray(record.contributors) ||
        !record.contributors.every(
          (row) =>
            !!row &&
            typeof row === "object" &&
            typeof (row as Record<string, unknown>).input_id === "string" &&
            typeof (row as Record<string, unknown>).completion === "string",
        )
      ) {
        throw new Error("invalid meta/stop_member_run receipt: malformed contributors");
      }
      break;
    case "not_current":
      break;
    case "not_stoppable":
      if (typeof record.state !== "string") {
        throw new Error("invalid meta/stop_member_run receipt: missing state");
      }
      break;
    default:
      throw new Error(`invalid meta/stop_member_run receipt outcome: ${String(record.outcome)}`);
  }
  return receipt as RunStopReceipt;
}

/** Operator-facing summary of a run-stop receipt. */
export function describeRunStopReceipt(receipt: RunStopReceipt): string {
  switch (receipt.outcome) {
    case "stopped": {
      const cancelled = receipt.contributors.filter((row) => row.terminal === "cancelled").length;
      const consumed = receipt.contributors.length - cancelled;
      const parts = [`${cancelled} input${cancelled === 1 ? "" : "s"} cancelled`];
      if (consumed > 0) parts.push(`${consumed} kept`);
      return `Run stopped: ${parts.join(", ")}.`;
    }
    case "not_current":
      return "That run already ended; nothing was stopped.";
    case "not_stoppable":
      return `The run cannot be stopped right now (runtime ${receipt.state}).`;
  }
}
