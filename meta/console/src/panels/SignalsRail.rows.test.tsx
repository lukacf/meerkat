import { cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SignalsRail } from "./SignalsRail";
import type { ConsoleFrame } from "../types";

afterEach(() => { cleanup(); });

// A control inside another control: a button or link in a button, or in
// an element that claims the button role.
const NESTED_CONTROLS = [
  "button button", "button a", "button [role='button']",
  "[role='button'] button", "[role='button'] a", "[role='button'] [role='button']",
].join(", ");

const now = Date.now();
const frames: ConsoleFrame[] = [
  {
    id: "reply",
    event: "interaction_complete",
    identity: "incident-commander",
    interactionId: "turn-1",
    timestampMs: now,
    data: { result: "Worker created successfully." },
  },
  {
    id: "prompt",
    event: "user_input",
    identity: "incident-commander",
    interactionId: "turn-1",
    timestampMs: now - 1_000,
    data: { content: "Create a worker." },
  },
  {
    id: "mem-quarantine",
    event: "memory.write.quarantined",
    identity: "distiller",
    timestampMs: now - 2_000,
    data: { realm: "default", author: "agent", reason: "low trust source" },
  },
];

describe("Signals rail rows", () => {
  it("keeps a signal's events and Memory pivot beside its label button, never inside it", () => {
    const onSelect = vi.fn();
    const view = render(<SignalsRail frames={frames} collapsed={false} onSelect={onSelect} />);

    const group = view.getByTestId("signal:interaction:turn-1");
    expect(group.getAttribute("role") === null).toBe(true);
    expect(group.getAttribute("tabindex") === null).toBe(true);
    const open = within(group).getByRole("button", { name: /Turn activity/ });
    expect(open.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(open);
    expect(open.getAttribute("aria-expanded")).toBe("true");
    const events = group.querySelectorAll<HTMLButtonElement>(".signal__event");
    expect(events.length).toBe(2);
    expect(view.container.querySelectorAll(NESTED_CONTROLS).length).toBe(0);

    // An event opens its frame and leaves the group open; a click elsewhere
    // on the row folds it, like the label.
    fireEvent.click(events[0]);
    expect(onSelect.mock.calls.length).toBe(1);
    expect(open.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(group.querySelector(".signal__detail") as HTMLElement);
    expect(open.getAttribute("aria-expanded")).toBe("false");

    const memory = Array.from(view.container.querySelectorAll<HTMLElement>('[data-testid^="signal:"]'))
      .find((row) => (row.textContent || "").includes("Memory write quarantined"));
    expect(memory !== undefined).toBe(true);
    const pivot = within(memory as HTMLElement).getByTestId("signal-memory-pivot");
    const label = within(memory as HTMLElement).getByRole("button", { name: /Memory write quarantined/ });
    expect(label.contains(pivot)).toBe(false);
    fireEvent.click(pivot);
    expect(onSelect.mock.calls.length).toBe(2);
    expect(onSelect.mock.calls[1][0].id).toBe("mem-quarantine");
    expect(view.container.querySelectorAll(NESTED_CONTROLS).length).toBe(0);
  });
});
