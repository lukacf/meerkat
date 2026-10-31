import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Chip, IconButton, StatusDot, StatusMark, StatusPill } from "./index";

// Console v2 kit parts (spec A3). DOM assertions compare booleans and
// strings, never elements: a failing element comparison walks React
// internals and can exhaust memory.

describe("status marks", () => {
  it("draws the tone's shape, decorative beside a word", () => {
    const view = render(<StatusMark tone="warn" />);
    const mark = view.container.querySelector(".cc-mark");
    expect(mark?.getAttribute("data-tone")).toBe("warn");
    expect(mark?.getAttribute("aria-hidden")).toBe("true");
    expect(mark?.hasAttribute("role")).toBe(false);
  });

  it("stands alone as an image named by its label", () => {
    const view = render(<StatusMark tone="live" label="Working now" size="md" />);
    const mark = view.getByRole("img", { name: "Working now" });
    expect(mark.getAttribute("data-size")).toBe("md");
    expect(mark.getAttribute("title")).toBe("Working now");
  });

  it("puts the mark before the word in a status pill", () => {
    const view = render(<StatusPill tone="crit" testId="pill">Stalled</StatusPill>);
    const pill = view.getByTestId("pill");
    expect(pill.className).toBe("cc-chip cc-chip--mark");
    expect(pill.getAttribute("data-tone")).toBe("crit");
    expect(pill.firstElementChild?.className).toBe("cc-mark cc-chip__mark");
    expect(pill.textContent).toBe("Stalled");
  });

  it("keeps the older chip and dot markup", () => {
    const chip = render(<Chip tone="ok" dot outline>Allowed</Chip>).container.innerHTML;
    expect(chip).toBe('<span class="cc-chip cc-chip--outline" data-tone="ok"><span class="cc-chip__dot" aria-hidden="true"></span>Allowed</span>');
    const dot = render(<StatusDot tone="info" />).container.innerHTML;
    expect(dot).toBe('<span class="cc-dot" data-tone="info" aria-hidden="true"></span>');
  });

  it("offers a quiet chip for dense rows", () => {
    const view = render(<Chip tone="warn" mark variant="quiet">Paused</Chip>);
    expect(view.container.firstElementChild?.className).toBe("cc-chip cc-chip--quiet cc-chip--mark");
  });
});

describe("icon buttons", () => {
  it("names itself by its label and shows it as the tooltip", () => {
    const onClick = vi.fn();
    const view = render(<IconButton label="Refresh approvals" glyph="refresh" onClick={onClick} data-testid="refresh" />);
    const button = view.getByRole("button", { name: "Refresh approvals" });
    expect(button.getAttribute("title")).toBe("Refresh approvals");
    expect(button.className).toBe("cc-btn cc-btn--icon cc-btn--ghost");
    expect(button.querySelector('svg[data-glyph="refresh"]') !== null).toBe(true);
    fireEvent.click(button);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("carries the reveal, quiet and badge states as classes", () => {
    const view = render(<IconButton label="Roster view options" glyph="ellipsis" size="sm" quiet reveal badge />);
    const button = view.getByRole("button", { name: "Roster view options" });
    expect(button.className).toBe("cc-btn cc-btn--icon cc-btn--ghost cc-btn--sm cc-btn--quiet cc-reveal has-badge");
    expect(button.querySelector(".cc-btn__badge") !== null).toBe(true);
  });
});
