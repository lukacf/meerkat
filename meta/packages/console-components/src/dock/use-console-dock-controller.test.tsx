import { renderHook, act } from "@testing-library/react";

import type { ConsoleDockTarget } from "@console-core";

import { useConsoleDockController } from "./use-console-dock-controller";

describe("useConsoleDockController", () => {
  test("opens, splits, focuses, and presets dock targets without app-specific session state", () => {
    type TestTarget = ConsoleDockTarget & {
      kind: "thread";
      threadId: string;
    };

    const alpha: TestTarget = {
      id: "thread:alpha",
      kind: "thread",
      title: "Alpha",
      threadId: "alpha",
    };
    const beta: TestTarget = {
      id: "thread:beta",
      kind: "thread",
      title: "Beta",
      threadId: "beta",
    };

    const { result } = renderHook(() => useConsoleDockController<TestTarget>({
      initialTarget: alpha,
      createPanelState: ({ target }) => ({
        id: "",
        target,
        mode: "console",
      }),
      suggestTargets: ({ count, preferred, excludedIds }) => {
        const pool = [preferred, alpha, beta].filter(Boolean) as TestTarget[];
        const suggestions = pool.filter((target) => !excludedIds.includes(target.id));
        while (suggestions.length < count) {
          suggestions.push(null as never);
        }
        return suggestions.slice(0, count);
      },
    }));

    expect(result.current.viewState.panels).toHaveLength(1);
    expect(result.current.focusedTarget?.threadId).toBe("alpha");

    act(() => {
      result.current.splitPanel(result.current.focusedPanelId!, "right");
    });

    expect(result.current.viewState.panels).toHaveLength(2);

    act(() => {
      result.current.openTarget(beta, "new_tab");
    });

    expect(result.current.viewState.tabs).toHaveLength(2);
    expect(result.current.focusedTarget?.threadId).toBe("beta");

    act(() => {
      result.current.applyPreset("grid");
    });

    expect(result.current.viewState.panels.length).toBeGreaterThanOrEqual(4);
  });
});
