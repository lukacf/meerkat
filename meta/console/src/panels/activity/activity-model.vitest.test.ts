import { describe, expect, it } from "vitest";

import { createAgentDirectory } from "../../../../packages/console-components/src/panel-kit/agents";
import type { ConsoleFrame } from "../../types";
import { buildActivityEntries, entrySentence, mergeFrameSources } from "./activity-model";

const NOW = Date.parse("2026-11-01T10:00:00Z");
let serial = 0;
function frame(identity: string, event: string, secondsAgo: number, data: Record<string, unknown> = {}, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  serial += 1;
  return { id: `f-${serial}`, event, identity, timestampMs: NOW - secondsAgo * 1000, sourceKind: "console_event", data, ...extra };
}
const directory = createAgentDirectory([
  { id: "intake", label: "Intake", aliases: ["peer-intake"] },
  { id: "billing", label: "Billing", aliases: ["peer-billing"] },
]);
const nameOf = (key: string) => directory.lookup(key)?.label ?? key;

describe("the activity entries", () => {
  it("keep one entry for a frame and its saved history copy, showing the live copy", () => {
    const live = frame("billing", "user_input", 30, { content: "Refund order 77310.", origin: "console:pane-1", origin_kind: "operator" }, { interactionId: "turn-1", sourceKind: "send" });
    const saved = frame("billing", "user_input", 29, { content: "Refund order 77310." }, { interactionId: "turn-1", sourceKind: "session_history" });
    const entries = buildActivityEntries([saved, live], { directory });
    expect(entries.length).toBe(1);
    expect(entries[0].frames.length).toBe(2);
    expect(entries[0].frame).toBe(live);
    expect(entrySentence(entries[0], nameOf)).toBe("An operator asked Billing");
    expect(entries[0].atMs).toBe(live.timestampMs);
  });

  it("keep one entry for a message: the sender's call and the recipient's notice, joined by the envelope", () => {
    const call = frame("intake", "tool_call_requested", 20, {
      name: "send_message", id: "call-1", args: { peer_id: "peer-billing", body: "Ticket 88101 for Billing." },
    }, { interactionId: "turn-intake" });
    const receipt = frame("intake", "tool_result_received", 19, {
      name: "send_message", id: "call-1",
      result: JSON.stringify({ kind: "peer_message", receipt: { envelope_id: "env-1" }, status: "sent" }),
    }, { interactionId: "turn-intake" });
    const notice = frame("billing", "system_notice", 18, {
      blocks: [{
        type: "comms", kind: "message", direction: "incoming", delivery_id: "env-1",
        peer: { id: "peer-intake", display_name: "team/coordinator/intake" },
        content: [{ type: "text", text: "Peer message from team/coordinator/intake:\nTicket 88101 for Billing." }],
      }],
    }, { sourceKind: "session_history" });
    const entries = buildActivityEntries([notice, receipt, call], { directory }).filter((entry) => entry.family === "message");
    expect(entries.length).toBe(1);
    const [message] = entries;
    expect(message.frame).toBe(call);
    expect(message.actor).toBe("intake");
    expect(message.target).toBe("billing");
    expect(message.frames.length).toBe(2);
    expect(entrySentence(message, nameOf)).toBe("Intake messaged Billing");
  });

  it("join a message's ends by sender and words when no receipt is loaded", () => {
    const call = frame("intake", "tool_call_requested", 20, {
      name: "send_message", id: "call-2", args: { peer_id: "unknown-peer", body: "Ticket 88102: refund request." },
    }, { interactionId: "turn-intake" });
    const notice = frame("billing", "system_notice", 19, {
      blocks: [{
        type: "comms", kind: "message", direction: "incoming", delivery_id: "env-2",
        peer: { id: "peer-intake", display_name: "team/coordinator/intake" },
        content: [{ type: "text", text: "Peer message from team/coordinator/intake:\nTicket 88102: refund request." }],
      }],
    });
    const entries = buildActivityEntries([call, notice], { directory }).filter((entry) => entry.family === "message");
    expect(entries.length).toBe(1);
    // The call could only say "a peer"; the notice names the recipient.
    expect(entrySentence(entries[0], nameOf)).toBe("Intake messaged Billing");
  });

  it("leave plumbing frames out unless asked, and key every row uniquely", () => {
    const frames = [
      frame("billing", "text_delta", 5, { delta: "Hi" }),
      frame("billing", "run_started", 6, { run_id: "run-1" }, { runId: "run-1", interactionId: "turn-1" }),
      frame("billing", "interaction_complete", 4, { result: "Done." }, { runId: "run-1", interactionId: "turn-1" }),
    ];
    expect(buildActivityEntries(frames).map((entry) => entry.frame.event)).toEqual(["interaction_complete"]);
    const all = buildActivityEntries(frames, { includeInternal: true });
    expect(all.length).toBe(3);
    expect(new Set(all.map((entry) => entry.rowKey)).size).toBe(3);
    expect(new Set(all.map((entry) => entry.key)).size).toBe(3);
  });

  it("merge frame sources once, keeping a frame's newest version", () => {
    const first = frame("billing", "user_input", 10, { content: "Hi" }, { frameVersion: 1 });
    const newer = { ...first, frameVersion: 2 };
    const other = frame("intake", "user_input", 9, { content: "Hello" });
    const merged = mergeFrameSources([first, other], [newer, other]);
    expect(merged.length).toBe(2);
    expect(merged.find((candidate) => candidate.id === first.id)?.frameVersion).toBe(2);
  });
});
