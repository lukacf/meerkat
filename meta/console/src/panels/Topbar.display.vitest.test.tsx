import { act, cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Topbar } from "./Topbar";
import {
  DEFAULT_DISPLAY,
  DISPLAY_STORAGE_KEYS,
  readDisplayChoices,
  resolveDisplaySettings,
  useConsoleDisplay,
  type ConsoleDisplay,
  type ConsoleDisplaySettings,
} from "./Tweaks";

// DOM assertions compare booleans and strings, never elements.

afterEach(() => { cleanup(); localStorage.clear(); });

describe("Display settings (B21)", () => {
  it("takes the user's choice, else the configured default, else the console's", () => {
    expect(resolveDisplaySettings({}, {})).toEqual(DEFAULT_DISPLAY);
    expect(resolveDisplaySettings({}, { theme: "dark", variant: "graphite" })).toEqual({
      theme: "dark", variant: "graphite", density: "comfortable", motion: "system",
    });
    expect(resolveDisplaySettings({ theme: "light", density: "compact" }, { theme: "dark" })).toEqual({
      theme: "light", variant: "rams", density: "compact", motion: "system",
    });
  });

  it("reads only valid stored choices", () => {
    localStorage.setItem(DISPLAY_STORAGE_KEYS.theme, "dark");
    localStorage.setItem(DISPLAY_STORAGE_KEYS.variant, "neon");
    localStorage.setItem(DISPLAY_STORAGE_KEYS.motion, "reduced");
    expect(readDisplayChoices()).toEqual({ theme: "dark", motion: "reduced" });
  });

  it("stores the user's own choices only: a configured default is applied, never written", () => {
    let current: ConsoleDisplay | null = null;
    function Harness({ configured }: { configured: Partial<ConsoleDisplaySettings> }) {
      current = useConsoleDisplay(configured);
      return null;
    }
    const view = render(<Harness configured={{ variant: "terminal", theme: "dark" }} />);
    expect(current!.settings).toEqual({ theme: "dark", variant: "terminal", density: "comfortable", motion: "system" });
    expect(current!.chosen).toEqual({});
    for (const key of Object.values(DISPLAY_STORAGE_KEYS)) expect(localStorage.getItem(key)).toBe(null);

    act(() => current!.choose("density", "compact"));
    act(() => current!.choose("theme", "light"));
    expect(current!.settings).toEqual({ theme: "light", variant: "terminal", density: "compact", motion: "system" });
    expect(localStorage.getItem(DISPLAY_STORAGE_KEYS.density)).toBe("compact");
    expect(localStorage.getItem(DISPLAY_STORAGE_KEYS.theme)).toBe("light");
    expect(localStorage.getItem(DISPLAY_STORAGE_KEYS.variant)).toBe(null);

    // A new configured default still applies where the user made no choice.
    view.rerender(<Harness configured={{ variant: "graphite", theme: "dark" }} />);
    expect(current!.settings.variant).toBe("graphite");
    expect(current!.settings.theme).toBe("light");

    act(() => current!.reset());
    expect(current!.chosen).toEqual({});
    expect(current!.settings).toEqual({ theme: "dark", variant: "graphite", density: "comfortable", motion: "system" });
    for (const key of Object.values(DISPLAY_STORAGE_KEYS)) expect(localStorage.getItem(key)).toBe(null);
  });

  it("follows a choice made in another tab", () => {
    let current: ConsoleDisplay | null = null;
    function Harness() {
      current = useConsoleDisplay({});
      return null;
    }
    render(<Harness />);
    localStorage.setItem(DISPLAY_STORAGE_KEYS.motion, "reduced");
    act(() => { window.dispatchEvent(new StorageEvent("storage", { key: DISPLAY_STORAGE_KEYS.motion })); });
    expect(current!.settings.motion).toBe("reduced");
  });
});

describe("Topbar (B15) and its Display popover", () => {
  const base = {
    mobName: "Operations team",
    mobStatus: "running",
    environment: "staging",
    theme: "light" as const,
    onToggleTheme: vi.fn(),
    sidebarCollapsed: false,
    railCollapsed: false,
    onToggleSidebar: vi.fn(),
    onToggleRail: vi.fn(),
  };

  it("names the team's state in words, shows the environment as a pill and keeps its ids", () => {
    const view = render(<Topbar {...base} mobStatus="unknown" />);
    expect(view.getByTestId("topbar-team").textContent).toBe("Operations teamnot observed");
    expect(view.container.querySelector(".meta-topbar__mob-status")?.getAttribute("data-status")).toBe("unknown");
    expect(view.getByTestId("topbar-environment").textContent).toBe("staging");
    for (const id of ["sidebar-collapse-toggle", "theme-toggle", "signals-rail-collapse-toggle"]) {
      expect(view.queryByTestId(id) !== null).toBe(true);
    }
    const toggle = view.getByTestId("theme-toggle");
    expect(toggle.getAttribute("aria-label")).toBe("Switch to dark mode");
    fireEvent.click(toggle);
    expect(base.onToggleTheme).toHaveBeenCalledTimes(1);
    view.rerender(<Topbar {...base} theme="dark" />);
    expect(view.getByTestId("theme-toggle").getAttribute("aria-label")).toBe("Switch to light mode");
  });

  it("opens Display with Theme, Style, Density and Motion, applies a choice at once and resets", () => {
    const onChooseDisplay = vi.fn();
    const onResetDisplay = vi.fn();
    const display: ConsoleDisplaySettings = { theme: "light", variant: "rams", density: "comfortable", motion: "system" };
    const view = render(
      <Topbar {...base} display={display} displayChosen={false} onChooseDisplay={onChooseDisplay} onResetDisplay={onResetDisplay} />,
    );
    const trigger = view.getByTestId("display-options");
    expect(trigger.getAttribute("aria-label")).toBe("Display");
    expect(trigger.getAttribute("aria-haspopup")).toBe("dialog");
    expect(trigger.classList.contains("has-badge")).toBe(false);
    fireEvent.click(trigger);
    const dialog = view.getByRole("dialog", { name: "Display" });
    expect(within(dialog).getAllByRole("group").map((group) => group.getAttribute("aria-label"))).toEqual(["Theme", "Style", "Density", "Motion"]);
    for (const id of ["display-theme:light", "display-style:rams", "display-density:comfortable", "display-motion:system"]) {
      expect(view.getByTestId(id).getAttribute("aria-pressed")).toBe("true");
    }
    expect(view.getByTestId("display-reset").hasAttribute("disabled")).toBe(true);

    fireEvent.click(view.getByTestId("display-style:terminal"));
    fireEvent.click(view.getByTestId("display-density:compact"));
    fireEvent.click(view.getByTestId("display-motion:reduced"));
    fireEvent.click(view.getByTestId("display-theme:dark"));
    expect(onChooseDisplay.mock.calls).toEqual([["variant", "terminal"], ["density", "compact"], ["motion", "reduced"], ["theme", "dark"]]);
    expect(view.queryByRole("dialog", { name: "Display" }) !== null).toBe(true);

    view.rerender(
      <Topbar {...base} display={{ ...display, density: "compact" }} displayChosen onChooseDisplay={onChooseDisplay} onResetDisplay={onResetDisplay} />,
    );
    expect(view.getByTestId("display-options").classList.contains("has-badge")).toBe(true);
    expect(view.getByTestId("display-density:compact").getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(view.getByTestId("display-reset"));
    expect(onResetDisplay).toHaveBeenCalledTimes(1);
  });
});
