import { describe, expect, it } from "vitest";

import { mapFramesToTimelineEntries as coreEntries } from "./adapters";
import { mapFramesToTimelineEntries as consoleEntries } from "../../../console/src/lib/adapters";
import type { ConsoleFrame } from "./runtime-types";

/** A peer message notice from Intake: a typed comms block with no request id. */
function intakeMessage(n: number, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return {
    id: `notice-${n}`,
    cursor: `console:${100 + n}`,
    event: "system_notice",
    identity: "dispatch",
    timestampMs: 1_000 + n * 1_000,
    sourceKind: "console_event",
    data: {
      blocks: [{
        type: "comms",
        kind: "message",
        direction: "incoming",
        peer: { id: "peer-intake", display_name: "support-mob/coordinator/intake" },
        content: [{ type: "text", text: `Ticket ${n} needs a carrier check.` }],
      }],
    },
    ...extra,
  };
}

function peerCallIds(entries: ReadonlyArray<{ blocks?: ReadonlyArray<{ type: string; toolCallId?: string }> }>): string[] {
  return entries.flatMap((entry) => (entry.blocks ?? [])
    .filter((block) => block.type === "tool-call" && typeof block.toolCallId === "string")
    .map((block) => block.toolCallId!));
}

describe.each([["core", coreEntries], ["console", consoleEntries]] as const)("%s transcript: peer messages without a request id", (_name, mapEntries) => {
  it("gives every message from one peer its own id", () => {
    const frames = [1, 2, 3, 4, 5].map((n) => intakeMessage(n));
    const ids = peerCallIds(mapEntries(null, frames) as never);
    expect(ids).toHaveLength(5);
    expect(new Set(ids).size).toBe(5);
    for (const id of ids) expect(id.startsWith("typed-comms:intake:message:")).toBe(true);
  });

  it("names two comms blocks of one notice apart, and keeps an id across renders", () => {
    const two = intakeMessage(7);
    const block = (two.data as { blocks: unknown[] }).blocks[0];
    (two.data as { blocks: unknown[] }).blocks = [block, { ...(block as object), content: [{ type: "text", text: "And another." }] }];
    const first = peerCallIds(mapEntries(null, [two]) as never);
    expect(new Set(first).size).toBe(first.length);
    expect(first).toHaveLength(2);
    expect(peerCallIds(mapEntries(null, [two]) as never)).toEqual(first);
  });

  it("keeps a typed request id as the message's id", () => {
    const request = intakeMessage(9);
    const blocks = (request.data as { blocks: Array<Record<string, unknown>> }).blocks;
    blocks[0] = { ...blocks[0], kind: "request", request_id: "req-42" };
    expect(peerCallIds(mapEntries(null, [request]) as never)).toEqual(["req-42"]);
  });
});
