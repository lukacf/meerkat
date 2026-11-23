import React from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { McpAppNotLoaded, McpAppUnavailable } from "./ChatPane";

// In the place of an interactive view that was not loaded because its
// agent's team is not loaded: the reason by the team's name, the tool's own
// words, and the way to wake a team that is asleep. A team that is
// unavailable is not offered a way to wake it.

afterEach(() => cleanup());

describe("the place of an interactive view that was not loaded", () => {
  it("says an asleep team by its name, keeps the tool's words, and wakes the team when pressed", () => {
    const onWake = vi.fn();
    render(<McpAppNotLoaded team={{ name: "Support team", onWake }} toolCallId="call:1" fallback="A chart of the week" />);
    const place = screen.getByTestId("mcp-app-asleep:call:1");
    expect(place.getAttribute("role")).toBe("status");
    expect(place.textContent).toContain("Support team is asleep, so this interactive view was not loaded. Loading it wakes the team.");
    expect(place.textContent).toContain("A chart of the week");
    fireEvent.click(screen.getByRole("button", { name: "Wake and load the view" }));
    expect(onWake).toHaveBeenCalledTimes(1);
  });

  it("shows the waking in place of the control while the team is being woken", () => {
    render(<McpAppNotLoaded team={{ name: "Support team", onWake: () => {}, waking: true }} toolCallId="call:1" fallback="A chart" />);
    expect((screen.getByRole("button", { name: "Waking" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("offers nothing to press for a team that is unavailable, and says that instead", () => {
    render(<McpAppNotLoaded team={{ name: "Billing", unavailable: true, onWake: () => {} }} toolCallId="call:2" fallback="A table" />);
    const place = screen.getByTestId("mcp-app-asleep:call:2");
    expect(place.textContent).toContain("Billing is unavailable, so this interactive view cannot be loaded.");
    expect(place.textContent).not.toContain("asleep");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("says, where interactive views are not available for the agent at all, that and the tool's words, with nothing to press", () => {
    render(<McpAppUnavailable reason="Interactive views are not available for Support reviewer yet. For now they work with agents of Ops team only." toolCallId="call:4" fallback="A chart of the week" />);
    const place = screen.getByTestId("mcp-app-unavailable:call:4");
    expect(place.getAttribute("role")).toBe("status");
    expect(place.textContent).toContain("Interactive views are not available for Support reviewer yet. For now they work with agents of Ops team only.");
    expect(place.textContent).toContain("A chart of the week");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("speaks of the agent's team when the console has no name for it, never of an id", () => {
    render(<McpAppNotLoaded team={{ name: null }} toolCallId="call:3" fallback="A table" />);
    expect(screen.getByTestId("mcp-app-asleep:call:3").textContent).toContain("This agent's team is asleep");
    expect(screen.queryByRole("button")).toBeNull();
  });
});
