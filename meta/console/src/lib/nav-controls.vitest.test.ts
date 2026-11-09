import { describe, expect, it } from "vitest";

import { consoleNavControls, type ConsoleNavControlsInput } from "./nav-controls";

const plainFixture: ConsoleNavControlsInput = {
  hasMobControlSurface: true,
  sidebar: null,
  access: null,
  memory: { available: false, can_read: false },
  workgraph: { available: true, can_view: true },
  emptyPanelRegistry: false,
  schedulesListed: false,
};

const STOCK = ["topology", "timeline", "gating", "roster", "routing", "logs", "health"];

describe("the views the console offers", () => {
  it("offers Memory as a not-set-up entry on a team runtime without a memory store", () => {
    expect(consoleNavControls(plainFixture)).toEqual([...STOCK, "memory", "workgraph"]);
  });

  it("offers Memory to a reader of a configured store, never to a caller without a read grant", () => {
    expect(consoleNavControls({ ...plainFixture, memory: { available: true, can_read: true } })).toContain("memory");
    expect(consoleNavControls({ ...plainFixture, memory: { available: true, can_read: false } })).not.toContain("memory");
    // Older gateways name no memory section.
    expect(consoleNavControls({ ...plainFixture, memory: undefined })).not.toContain("memory");
  });

  it("keeps the shared console's Memory to its setup explanation", () => {
    const shared = { ...plainFixture, hasMobControlSurface: false, workgraph: null };
    expect(consoleNavControls(shared)).toEqual(["topology", "timeline", "roster", "logs", "health"]);
    expect(consoleNavControls({ ...shared, emptyPanelRegistry: true })).toEqual(["topology", "timeline", "roster", "logs", "health", "memory", "workgraph"]);
  });

  it("offers Schedules when the gateway lists them, and Access to administrators only", () => {
    expect(consoleNavControls({ ...plainFixture, schedulesListed: true }).at(-1)).toBe("schedules");
    expect(consoleNavControls({ ...plainFixture, access: { can_administer: true } })).toEqual([...STOCK, "access", "memory", "workgraph"]);
    expect(consoleNavControls({ ...plainFixture, access: { can_administer: false } })).not.toContain("access");
  });

  it("hides what hidden_controls names, Memory and Schedules included, but never Access", () => {
    const controls = consoleNavControls({
      ...plainFixture,
      access: { can_administer: true },
      schedulesListed: true,
      sidebar: { hidden_controls: ["gating", "memory", "schedules", "access", "unknown"] },
    });
    expect(controls).toEqual(["topology", "timeline", "roster", "routing", "logs", "health", "access", "workgraph"]);
  });

  it("lets visible_controls choose the stock views the runtime has, ignoring hidden_controls", () => {
    const controls = consoleNavControls({
      ...plainFixture,
      hasMobControlSurface: false,
      schedulesListed: true,
      sidebar: { visible_controls: ["roster", "gating", "logs"], hidden_controls: ["memory", "schedules"] },
    });
    // Gating needs a team control surface; the runtime-owned views follow.
    expect(controls).toEqual(["roster", "logs", "workgraph", "schedules"]);
  });
});
