import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, test, vi } from "vitest";

import { CopyButton } from "./copy-button";

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("CopyButton", () => {
  test.each([undefined, null])("uses a decorative built-in icon without a host renderer (%s)", (Icon) => {
    render(<CopyButton text="Short message." label="Copy message" Icon={Icon} />);

    const button = screen.getByRole("button", { name: "Copy message" });
    expect(button).toHaveAttribute("type", "button");
    expect(button).toHaveAttribute("title", "Copy message");
    expect(button.textContent).toBe("");
    expect(button.querySelector('svg[data-icon="copy"]')).toHaveAttribute("aria-hidden", "true");
    expect(button.querySelector("svg")).toHaveAttribute("focusable", "false");
    button.focus();
    expect(button).toHaveFocus();
  });

  test("copies exact text and shows a temporary check without widening the button", async () => {
    vi.useFakeTimers();
    const text = "  A\u030A, å, 🚀 and <tag>\nKeep these exact bytes.  ";
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    render(<CopyButton text={text} label="Copy message" copiedLabel="Copied message" />);

    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Copy message" })));
    const button = screen.getByRole("button", { name: "Copied message" });
    expect(writeText).toHaveBeenCalledExactlyOnceWith(text);
    expect(button).toHaveAttribute("title", "Copied message");
    expect(button).toHaveAttribute("data-copied", "true");
    expect(button.querySelector('svg[data-icon="check"]')).toBeInTheDocument();
    expect(button.textContent).toBe("");

    act(() => vi.advanceTimersByTime(1600));
    expect(screen.getByRole("button", { name: "Copy message" })).toBe(button);
    expect(button.querySelector('svg[data-icon="copy"]')).toBeInTheDocument();
    expect(button).not.toHaveAttribute("data-copied");
  });

  test("preserves the host icon renderer for both copy and copied states", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    function HostIcon({ name }: { name: string }) {
      return <svg aria-hidden="true" data-host-icon={name} />;
    }
    render(<CopyButton text="Exact source" label="Copy source" Icon={HostIcon} />);
    const button = screen.getByRole("button", { name: "Copy source" });
    expect(button.querySelector('[data-host-icon="i-copy"]')).toBeInTheDocument();
    expect(button.querySelector(".cc-copy-glyph")).toBeNull();

    await act(async () => fireEvent.click(button));
    expect(button.querySelector('[data-host-icon="i-check"]')).toBeInTheDocument();
    expect(button.querySelector(".cc-copy-glyph")).toBeNull();
    expect(writeText).toHaveBeenCalledExactlyOnceWith("Exact source");
  });

  test("keeps blank content disabled and does not claim a failed copy succeeded", async () => {
    const writeText = vi.fn().mockRejectedValue(new Error("Clipboard unavailable"));
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    const view = render(<CopyButton text={" \n "} label="Copy source" />);
    const button = screen.getByRole("button", { name: "Copy source" });
    expect(button).toBeDisabled();
    fireEvent.click(button);
    expect(writeText).not.toHaveBeenCalled();

    view.rerender(<CopyButton text="Exact source" label="Copy source" />);
    await act(async () => fireEvent.click(button));
    expect(button).toHaveAccessibleName("Copy source");
    expect(button).not.toHaveAttribute("data-copied");
  });
});
