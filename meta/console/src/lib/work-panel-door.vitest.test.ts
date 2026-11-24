import { describe, expect, it } from "vitest";

import { memberDoor, mobDoorTable } from "../../../packages/console-core/src/mob-door";
import { workPanelReassignRefusal } from "./work-panel-door";

// The docked Work panel shows the work graph of the launch mob's door. A
// work item is reassigned within the mob whose graph the panel shows.

const table = mobDoorTable(
  [
    { mob_id: "launch", default: true, provenance: "host", state: "running" },
    { mob_id: "research", parent_mob_id: "launch", provenance: "member", state: "running" },
    { mob_id: "support", provenance: "host", state: "running" },
    { mob_id: "support-lab", parent_mob_id: "support", provenance: "member", state: "running" },
  ],
  "launch",
  true,
);
const door = (rowKey: string) => memberDoor(rowKey, table);

describe("a reassign of the docked Work panel", () => {
  it("is refused by the page for an agent behind another door than the panel's, in words that say why", () => {
    for (const hosted of ["support/reviewer", "support/support-lab/tester"]) {
      expect(workPanelReassignRefusal(hosted, door, "Operations team"), hosted).toBe(
        `The Work panel shows the work of Operations team. ${hosted} is an agent of another team, and a work item is reassigned within its own team. Nothing was sent.`,
      );
    }
  });

  it("is sent for an agent behind the panel's door, and for a name the page does not know", () => {
    for (const name of ["planner", "research/scout", "someone-new"]) {
      expect(workPanelReassignRefusal(name, door, "Operations team"), name).toBeNull();
    }
  });
});
