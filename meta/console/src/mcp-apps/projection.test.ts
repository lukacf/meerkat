import { describe, expect, it } from "vitest";
import { buildActivityRailViewState as coreActivity, ConsoleActivityProjection, mapFramesToTimelineEntries as coreEntries } from "../../../packages/console-core/src/adapters";
import { buildActivityRailViewState as stockActivity, deriveTimelineEntries, mapFramesToTimelineEntries as stockEntries, timelineDerivationStats } from "../lib/adapters";
import { parseSseFrames } from "../../../packages/console-core/src/network";
import type { ConsoleFrame } from "../../../packages/console-core/src/runtime-types";

function frame(overrides: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return {
    id: "app-frame", event: "mcp_app", sourceKind: "session_history",
    identity: "member-a", sessionId: "session-a", runtimeKey: "runtime-a",
    data: { session_id: "session-a", tool_call_id: "call-a", fallback: "Saved result" },
    ...overrides,
  };
}

describe.each([
  ["shared", coreEntries, coreActivity],
  ["stock", stockEntries, stockActivity],
] as const)("%s native MCP App timeline projection", (_name, mapFramesToTimelineEntries, buildActivityRailViewState) => {
  it.each(["session_history", "tool_application"])("projects only the %s locator and fallback, with stable invocation identity", (sourceKind) => {
    const entries = mapFramesToTimelineEntries(null, [frame({
      sourceKind,
      data: { session_id: "session-a", tool_call_id: "call-a", fallback: "Saved result", _meta: { secret: "private" } },
    }), frame({ id: "replayed-frame", sourceKind })]);
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      kind: "message", identity: { id: "member-a" }, text: "Saved result",
      mcpApp: { sessionId: "session-a", toolCallId: "call-a" },
      renderKey: JSON.stringify(["mcp-app", "runtime-a", "member-a", "session-a", "call-a"]),
    });
    expect(JSON.stringify(entries)).not.toContain("private");
    const reloaded = mapFramesToTimelineEntries(null, [frame({ id: "reloaded-frame" })]);
    expect(reloaded[0].renderKey).toBe(entries[0].renderKey);
  });

  it("parses a native live source and deduplicates its committed-history twin", () => {
    const live = parseSseFrames(`event: frame_appended\ndata: ${JSON.stringify({
      type: "frame_appended",
      frame: {
        id: "live-app", kind: "mcp_app", identity: "member-a",
        runtime_key: "runtime-a", session_id: "session-a",
        source: { kind: "tool_application" },
        payload: { session_id: "session-a", tool_call_id: "call-a", fallback: "Live result" },
      },
    })}\n\n`);
    expect(live).toHaveLength(1);
    expect(live[0].sourceKind).toBe("tool_application");
    const entries = mapFramesToTimelineEntries(null, [...live, frame()]);
    expect(entries).toHaveLength(1);
    expect(entries[0].mcpApp).toEqual({ sessionId: "session-a", toolCallId: "call-a" });
  });

  it("does not turn a live view locator into activity or a work reservation", () => {
    const live = frame({ sourceKind: "tool_application", runId: "run-a", interactionId: "input-a" });
    const activity = buildActivityRailViewState({ agents: [], eventFrames: [live] });
    expect(activity.panels[0]).toMatchObject({ kind: "pulse", items: [] });
    const lifecycle = new ConsoleActivityProjection();
    expect(lifecycle.fold(live)).toBe(false);
    expect(lifecycle.busy).toBe(false);
    expect(lifecycle.phase).toBe(null);
  });

  it("rejects non-native, incomplete and conflicting invocation locators", () => {
    for (const invalid of [
      frame({ sourceKind: "console_event" }),
      ...["session_history", "tool_application"].flatMap(sourceKind => [
        frame({ sourceKind, identity: "" }),
        frame({ sourceKind, sessionId: "different-session" }),
        frame({ sourceKind, data: { session_id: "session-a", tool_call_id: "" } }),
        frame({ sourceKind, data: { session_id: "", tool_call_id: "call-a" } }),
      ]),
    ]) expect(mapFramesToTimelineEntries(null, [invalid])).toEqual([]);
  });

  it("does not conflate tool-call ids belonging to different native sessions", () => {
    const entries = mapFramesToTimelineEntries(null, [frame(), frame({
      id: "second", sessionId: "session-b",
      data: { session_id: "session-b", tool_call_id: "call-a" },
    })]);
    expect(entries).toHaveLength(2);
    expect(entries[0].renderKey).not.toBe(entries[1].renderKey);
  });
});

it("preserves the stock inline app through incremental final-model rendering", () => {
  const live = frame({ sourceKind: "tool_application", cursor: "console:1", timestampMs: 1000 });
  const initial = deriveTimelineEntries(null, [live]);
  const delta = frame({ id: "delta", event: "text_delta", sourceKind: "console_event", cursor: "console:2", timestampMs: 1001, data: { delta: "Working" } });
  const extendedBefore = timelineDerivationStats.extended;
  const extended = deriveTimelineEntries(null, [live, delta], {}, initial);
  expect(timelineDerivationStats.extended).toBe(extendedBefore + 1);
  expect(extended.entries.filter(entry => entry.kind === "message" && entry.mcpApp)).toEqual(initial.entries);
  const replay = deriveTimelineEntries(null, [live, delta, frame()], {}, extended);
  expect(replay.entries.filter(entry => entry.kind === "message" && entry.mcpApp)).toHaveLength(1);
});
