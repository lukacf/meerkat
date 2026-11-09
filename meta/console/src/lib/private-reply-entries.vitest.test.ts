/**
 * A private reply appended to a person's own thread (a `private_reply`
 * console frame) reads as the agent's message: its text, from the agent of
 * the conversation, never a runtime event whose text hides behind a
 * disclosure. Both the stock console's transcript and the shared package's.
 */
import { describe, expect, it } from "vitest";

import { mapFramesToTimelineEntries as stock } from "./adapters";
import { mapFramesToTimelineEntries as shared } from "../../../packages/console-core/src/adapters";

const AGENT = {
  agent_id: "alpha-side",
  member_id: "alpha-side",
  identity: "alpha-side",
  label: "Alpha (side chat)",
  kind: "mob_agent",
};

function replyFrame(data: unknown) {
  return {
    id: "reply-frame",
    cursor: "console:9",
    event: "private_reply",
    identity: "alpha-side",
    sourceKind: "synthetic",
    status: "completed",
    timestampMs: 2_000,
    data,
  };
}

const REPLY = { text: "Your release is approved.", in_reply_to: { interaction_id: "turn-1", input_frame_id: "request-frame" } };

describe.each([
  ["stock", stock],
  ["shared", shared],
] as const)("the %s transcript", (_name, mapFrames) => {
  it("shows a private reply as the agent's message", () => {
    const entries = mapFrames(AGENT as never, [replyFrame(REPLY)] as never);
    expect(entries).toHaveLength(1);
    const entry = entries[0] as Record<string, unknown> & { identity: { role: string; label: string } };
    expect(entry.kind).toBe("message");
    expect(entry.identity.role).toBe("assistant");
    expect(entry.identity.label).toBe("Alpha (side chat)");
    expect(entry.runtimeEvent ?? null).toBeNull();
    expect(JSON.stringify(entry.blocks ?? entry.text)).toContain("Your release is approved.");
  });

  it("shows nothing for a private reply without text", () => {
    for (const data of [{ ...REPLY, text: "" }, { ...REPLY, text: 7 }, null]) {
      expect(mapFrames(AGENT as never, [replyFrame(data)] as never)).toEqual([]);
    }
  });
});
