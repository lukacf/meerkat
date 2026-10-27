import { bindConsolePanelService, createConsolePanelService, useConsolePanels } from "../../../console/src/lib/custom-panels";
import React from "react";
import { act, render, renderHook, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { consolePanelModuleUrl, consoleCustomPanelTarget, migrateConsoleWorkbenchTarget, validateConsolePanels, type ConsolePanelDefinition, type ConsolePanelContext } from "@console-core";
import { ConsolePanelSurface, ConsolePanelsProvider, ConsoleCustomPanel } from "./custom-panels";

const context: ConsolePanelContext = { baseUrl: "http://localhost", readOnly: false, experience: null, visibleIdentities: ["agent:a", "agent:b", "agent:c", "agent", "identity:demo"], authority: { key: "scope:one" }, selection: { scopeKey: "scope:one", identity: "agent:b" }, conversation: null, request: vi.fn(), openPanel: vi.fn() };
const panel: ConsolePanelDefinition = { id: "demo/panel", title: "Demo", mount: () => {} };

describe("custom panel lifecycle", () => {
  it("updates without remounting, then aborts and disposes on unmount", () => {
    const dispose = vi.fn(), update = vi.fn();
    let signal: AbortSignal;
    const mount = vi.fn((element, _value, lifetime) => {
      signal = lifetime; element.textContent = "Mounted"; return { update, dispose };
    });
    const view = render(<ConsolePanelSurface mount={mount} context={context} fallback="Unavailable" />);
    expect(screen.getByText("Mounted")).toBeVisible();
    const next = { ...context, readOnly: true };
    view.rerender(<ConsolePanelSurface mount={mount} context={next} fallback="Unavailable" />);
    expect(mount).toHaveBeenCalledTimes(1);
    expect(update).toHaveBeenCalledWith(next);
    view.unmount();
    expect(signal!.aborted).toBe(true);
    expect(dispose).toHaveBeenCalledTimes(1);
  });

  it("cleans up and renders fallback for mount and update exceptions", () => {
    const broken = (element: HTMLElement) => { element.textContent = "Partial"; throw Error("broken"); };
    const view = render(<ConsolePanelSurface mount={broken} context={context} fallback="Unavailable" />);
    expect(screen.getByText("Unavailable")).toBeVisible();
    expect(screen.queryByText("Partial")).toBeNull();
    const dispose = vi.fn(() => { throw Error("bad cleanup"); });
    const mount = (element: HTMLElement) => {
      element.textContent = "Good";
      return { update: () => { throw Error("bad update"); }, dispose };
    };
    view.rerender(<ConsolePanelSurface mount={mount} context={context} fallback="Unavailable" />);
    expect(screen.getByText("Good")).toBeVisible();
    view.rerender(<ConsolePanelSurface mount={mount} context={{ ...context, readOnly: true }} fallback="Unavailable" />);
    expect(screen.getByText("Unavailable")).toBeVisible();
    expect(dispose).toHaveBeenCalledTimes(1);
  });


});

describe("custom panel registration", () => {
  it("validates serializable panel selectors before persisting independent instances", () => {
    const validatedPanel = { ...panel, validateParams: (value: unknown) => !!value && typeof value === "object" && "recordId" in value };
    const a = consoleCustomPanelTarget(validatedPanel, { instanceKey: "a", params: { recordId: "one" } });
    const b = consoleCustomPanelTarget(validatedPanel, { instanceKey: "b", params: { recordId: "two" } });
    expect(a.id).not.toBe(b.id);
    expect(migrateConsoleWorkbenchTarget(JSON.parse(JSON.stringify(a)))).toEqual(a);
    expect(() => consoleCustomPanelTarget(validatedPanel, { params: { wrong: true } })).toThrow(/parameters/);
    expect(() => consoleCustomPanelTarget(panel, { params: { recordId: "one" } })).toThrow(/parameters/);
    expect(() => consoleCustomPanelTarget(validatedPanel, { params: { recordId: Infinity } })).toThrow(/JSON/);
    const cyclic: Record<string, any> = {}; cyclic.self = cyclic;
    expect(() => consoleCustomPanelTarget(validatedPanel, { params: cyclic })).toThrow(/JSON/);
  });

  it("rejects collisions and reserved stock names", () => {
    expect(() => validateConsolePanels([panel, panel])).toThrow(/duplicate/);
    expect(() => validateConsolePanels([{ ...panel, id: "meta/workgraph" }])).toThrow();
  });

  it("preserves panel identity through the shared host-target migration", () => {
    const target = consoleCustomPanelTarget(panel);
    expect(migrateConsoleWorkbenchTarget(target)).toMatchObject(target);
  });

  it("accepts same-origin URLs and refuses executable or foreign URLs", () => {
    expect(consolePanelModuleUrl("./plugin.js", "https://example.test/runtime")).toBe("https://example.test/runtime/plugin.js");
    for (const url of ["javascript:alert(1)", "data:text/javascript,bad", "https://other.test/plugin.js", "//other.test/plugin.js", "https://user@example.test/plugin.js", "", "/plugin.js#x"]) {
      expect(() => consolePanelModuleUrl(url, "https://example.test")).toThrow();
    }
  });

  it("isolates module failures and validates all registrations before exposing them", async () => {
    const load = vi.fn(async (url: string) => {
      if (url.endsWith("bad.js")) throw Error("load failed");
      return { default: [panel] };
    });
    const { result } = renderHook(() => useConsolePanels(undefined, ["/ok.js", "/bad.js", "/duplicate.js"], "http://localhost", load));
    await waitFor(() => expect(result.current.errors).toHaveLength(2));
    expect(result.current.panels).toEqual([panel]);
  });

  it("ignores stale module completion after changing runtime scope", async () => {
    let resolveOld: (value: { default: readonly ConsolePanelDefinition[] }) => void;
    const load = vi.fn((url: string) => url.includes("old.test")
      ? new Promise<{ default: readonly ConsolePanelDefinition[] }>(resolve => { resolveOld = resolve; })
      : Promise.resolve({ default: [{ ...panel, id: "demo/new" }] }));
    const { result, rerender } = renderHook(({ base }) => useConsolePanels(undefined, ["/plugin.js"], base, load), { initialProps: { base: "http://old.test" } });
    rerender({ base: "http://new.test" });
    await waitFor(() => expect(result.current.panels[0]?.id).toBe("demo/new"));
    await act(async () => resolveOld!({ default: [panel] }));
    expect(result.current.panels[0]?.id).toBe("demo/new");
  });
});

describe("panel context ownership", () => {
  const original = { scopeKey: "scope:one", identity: "agent:a" };
  it("follows the current visible conversation after the original selection leaves", () => {
    const signals: AbortSignal[] = [];
    const mount = vi.fn((element, value, signal) => {
      signals.push(signal);
      element.textContent = value.conversation.identity;
      return { dispose: vi.fn() };
    });
    const definition = { ...panel, mount };
    const target = consoleCustomPanelTarget(definition, { conversation: original, followSelection: true });
    const show = (ctx: ConsolePanelContext) => <ConsolePanelsProvider value={{ panels: [definition], context: ctx }}>
      <ConsoleCustomPanel target={target} focused />
    </ConsolePanelsProvider>;
    const view = render(show({ ...context, selection: original }));
    expect(screen.getByText("agent:a")).toBeVisible();
    view.rerender(show({ ...context, visibleIdentities: ["agent:b"], selection: { ...original, identity: "agent:b" } }));
    expect(signals[0].aborted).toBe(true);
    expect(screen.getByText("agent:b")).toBeVisible();
    expect(mount).toHaveBeenCalledTimes(2);
  });

  it("pins panels, remounts follow-selection panels, and clears DOM on authority changes", () => {
    const signals: AbortSignal[] = [];
    const mount = vi.fn((element, value, signal) => {
      signals.push(signal); element.textContent = value.conversation.identity;
      return { update: vi.fn(), dispose: vi.fn() };
    });
    const definition = { ...panel, mount };
    const panels = [definition];
    const target = consoleCustomPanelTarget(definition, { conversation: original });
    const show = (ctx: ConsolePanelContext, followSelection = false) => <ConsolePanelsProvider value={{ panels, context: ctx }}>
      <ConsoleCustomPanel target={{ ...target, payload: { ...target.payload, followSelection } }} focused />
    </ConsolePanelsProvider>;
    const view = render(show(context));
    expect(screen.getByText("agent:a")).toBeVisible();
    view.rerender(show({ ...context, selection: { ...original, identity: "agent:c" } }));
    expect(mount).toHaveBeenCalledTimes(1);
    view.rerender(show(context, true));
    expect(signals[0].aborted).toBe(true);
    expect(screen.getByText("agent:b")).toBeVisible();
    view.rerender(show({ ...context, authority: { key: "scope:two" } }, true));
    expect(signals[1].aborted).toBe(true);
    expect(screen.queryByText("agent:b")).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent("unavailable");
  });
});

describe("restored panel scope", () => {
  it.each([
    { scopeKey: "scope:other", conversation: null },
    { scopeKey: "scope:one", conversation: { scopeKey: "scope:one", identity: "private:other" } },
  ])("does not mount unavailable authority or conversation selectors", options => {
    const mount = vi.fn();
    const definition = { ...panel, mount };
    render(<ConsolePanelsProvider value={{ panels: [definition], context }}>
      <ConsoleCustomPanel target={consoleCustomPanelTarget(definition, options)} focused />
    </ConsolePanelsProvider>);
    expect(mount).not.toHaveBeenCalled();
    expect(screen.getByRole("status")).toHaveTextContent("unavailable");
  });
});

describe("panel host services", () => {
  const scope = { authority: { key: "scope:one" }, conversation: { scopeKey: "scope:one", identity: "agent:a" }, readOnly: false };
  it("blocks writes in read-only mode even with an injected service", async () => {
    const injected = vi.fn();
    const service = bindConsolePanelService(injected, new AbortController().signal);
    await expect(service({ path: "/action", method: "POST" }, { ...scope, readOnly: true }, new AbortController().signal)).rejects.toThrow(/view only/);
    expect(injected).not.toHaveBeenCalled();
  });

  it.each(["surface", "authority"])("aborts %s requests and discards late results without replay", async lifetime => {
    const surface = new AbortController(), authority = new AbortController();
    let resolve: (value: unknown) => void;
    const injected = vi.fn(() => new Promise(done => { resolve = done; }));
    const request = bindConsolePanelService(injected, authority.signal)({ path: "/data" }, scope, surface.signal);
    const rejected = expect(request).rejects.toMatchObject({ name: "AbortError" });
    (lifetime === "surface" ? surface : authority).abort();
    expect(injected.mock.calls[0][2].aborted).toBe(true);
    resolve!("private result");
    await rejected;
    expect(injected).toHaveBeenCalledTimes(1);
    expect(injected.mock.calls[0][1]).toEqual(scope);
  });

  it("uses same-origin cookies, refuses redirects, and does not retry HTTP failures", async () => {
    const fetch = vi.fn(async () => ({ ok: false, status: 403 }));
    vi.stubGlobal("fetch", fetch);
    try {
      const service = createConsolePanelService("http://localhost");
      await expect(service({ path: "https://other.test/data" }, scope, new AbortController().signal)).rejects.toThrow(/same-origin/);
      expect(fetch).not.toHaveBeenCalled();
      await expect(service({ path: "/data" }, scope, new AbortController().signal)).rejects.toThrow(/403/);
      expect(fetch).toHaveBeenCalledTimes(1);
      expect(fetch.mock.calls[0][1]).toMatchObject({ credentials: "same-origin", redirect: "error", headers: {
        "X-Console-Authority": encodeURIComponent(JSON.stringify(scope.authority)),
        "X-Console-Conversation": encodeURIComponent(JSON.stringify(scope.conversation)),
      } });
    } finally { vi.unstubAllGlobals(); }
  });
});
