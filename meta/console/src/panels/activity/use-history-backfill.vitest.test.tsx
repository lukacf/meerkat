import { cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { ConsoleAgent } from "../../types";
import { useHistoryBackfill } from "./use-history-backfill";

// The day's history is loaded once per agent. A load answers whether the
// agent's history was read: the console makes no read on its own for an
// agent whose mob is listed as not loaded, and such an agent is not counted
// as loaded. Its history is read when the roster next changes and the mob
// is loaded, with no act of a person.

afterEach(cleanup);

const agent = (identity: string, state: string): ConsoleAgent => ({ identity, member_id: identity, state } as unknown as ConsoleAgent);

describe("the day's history of an agent whose mob is not loaded", () => {
  it("is read once the mob is loaded and the roster changes, without a person's act", async () => {
    // What the mob of `support/reviewer` is, and which reads were made.
    let mobLoaded = false;
    const asked: string[] = [];
    const read: string[] = [];
    const loadHistory = async (identity: string): Promise<boolean> => {
      asked.push(identity);
      // Held back for an agent whose mob is not loaded: nothing is read.
      if (identity === "support/reviewer" && !mobLoaded) return false;
      read.push(identity);
      return true;
    };
    const hook = renderHook(
      (props: { agents: ConsoleAgent[] }) => useHistoryBackfill(props.agents, loadHistory),
      { initialProps: { agents: [agent("planner", "running"), agent("support/reviewer", "running")] } },
    );
    await waitFor(() => expect(hook.result.current.loaded).toBe(true));
    expect(read).toEqual(["planner"]);
    expect(asked.sort()).toEqual(["planner", "support/reviewer"]);

    // The mob loads, and the roster is read again: the same two agents.
    mobLoaded = true;
    hook.rerender({ agents: [agent("planner", "running"), agent("support/reviewer", "running")] });
    await waitFor(() => expect(read).toEqual(["planner", "support/reviewer"]));
    // An agent whose history was read is not read again by a change of the roster.
    hook.rerender({ agents: [agent("planner", "idle"), agent("support/reviewer", "idle")] });
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    expect(read).toEqual(["planner", "support/reviewer"]);
  });

  it("is read once the set of loaded mobs changes, also when the roster did not change", async () => {
    // The page keeps its roster when no row of it differs: a mob can load
    // with every listed row as it was. The loader is then told by the value
    // that changes with the set of loaded mobs.
    let mobLoaded = false;
    const read: string[] = [];
    const loadHistory = async (identity: string): Promise<boolean> => {
      if (identity === "support/reviewer" && !mobLoaded) return false;
      read.push(identity);
      return true;
    };
    const agents = [agent("planner", "running"), agent("support/reviewer", "running")];
    const hook = renderHook(
      (props: { loadedMobs: string }) => useHistoryBackfill(agents, loadHistory, props.loadedMobs),
      { initialProps: { loadedMobs: "support>support!" } },
    );
    await waitFor(() => expect(hook.result.current.loaded).toBe(true));
    expect(read).toEqual(["planner"]);

    // The mob loads; the roster is the same array.
    mobLoaded = true;
    hook.rerender({ loadedMobs: "support>support" });
    await waitFor(() => expect(read).toEqual(["planner", "support/reviewer"]));
    // A later change of the loaded mobs reads nobody again.
    hook.rerender({ loadedMobs: "support>support|archive>archive" });
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    expect(read).toEqual(["planner", "support/reviewer"]);
  });

  it("counts an agent whose read was tried and failed as loaded: only a load that tried no read is asked again", async () => {
    const asked: string[] = [];
    const loadHistory = async (identity: string): Promise<boolean> => {
      asked.push(identity);
      if (identity === "support/reviewer") throw new Error("the gateway refused the read");
      return true;
    };
    const hook = renderHook(
      (props: { agents: ConsoleAgent[]; loadedMobs: string }) => useHistoryBackfill(props.agents, loadHistory, props.loadedMobs),
      { initialProps: { agents: [agent("planner", "running"), agent("support/reviewer", "running")], loadedMobs: "a" } },
    );
    await waitFor(() => expect(hook.result.current.loaded).toBe(true));
    hook.rerender({ agents: [agent("planner", "idle"), agent("support/reviewer", "idle")], loadedMobs: "b" });
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    expect(asked.sort()).toEqual(["planner", "support/reviewer"]);
  });
});
