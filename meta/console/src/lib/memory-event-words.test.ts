import { describe, expect, it } from "vitest";

import { describeMemoryEventForPeople, memoryAuthorWords, parseMemoryAuthor } from "./memory-event-words";

const names = {
  agentName: (key: string) => (key === "identity:router" ? "Router" : key.replace(/^identity:/, "")),
  recordTitle: (id: string) => (id === "rec-9" ? "Deploy key for staging" : undefined),
};

describe("memory events in words", () => {
  it("covers every documented subtype without keys, run ids or session keys", () => {
    const cases: Array<[string, Record<string, unknown>, string]> = [
      ["memory.dream.started", { run_id: "run-1" }, "Dream started"],
      ["memory.dream.completed", { ops_committed: 3, run_id: "run-1" }, "Dream finished · 3 changes"],
      ["memory.dream.skipped", { reason: "no changes" }, "Dream skipped · no changes"],
      ["memory.record.promoted", { scope_kind: "identity", scope_key: "identity:router", gated: true }, "A record moved to Router's memory after approval"],
      ["memory.record.promoted", { scope_kind: "realm", scope_key: "default" }, "A record moved to shared memory"],
      ["memory.quarantine.verdict", { verdict: "tombstone", rationale: "duplicate" }, "Quarantine verdict: discarded · duplicate"],
      ["memory.quarantine.verdict", { verdict: "unverifiable" }, "Quarantine verdict: unverifiable"],
      ["memory.quarantine.verdict", { verdict: "promote" }, "Quarantine verdict: moved to team memory"],
      ["memory.quarantine.verdict", { verdict: "open_loop_escalated", record_id: "rec-9" }, "“Deploy key for staging” needs follow-up"],
      ["memory.quarantine.verdict", { verdict: "open_loop_escalated", record_id: "rec-10" }, "An open loop needs follow-up"],
      ["memory.record.promoted", {}, "A record was moved"],
      ["memory.quarantine.release_blocked", { record_id: "rec-9", verdict: "release", class: "private-key" }, "Release of “Deploy key for staging” blocked · it looks like a secret"],
      ["memory.quarantine.release_blocked", { record_id: "rec-10", verdict: "promote_pending_gate" }, "Moving a quarantined record was blocked"],
      ["memory.conflict.signal", { entity: "user", topic: "time zone", reason: "mismatch" }, "Conflicting memories about user, time zone · mismatch"],
      ["memory.write.quarantined", { author: "identity:router", reason: "tainted session" }, "A memory write from Router was quarantined · tainted session"],
      // The server sends the author as Rust Debug text until it sends the
      // structured author; the sentence names the agent either way.
      ["memory.write.quarantined", { author: "Agent { identity: \"identity:router\" }", reason: "tainted session" }, "A memory write from Router was quarantined · tainted session"],
      ["memory.write.quarantined", { author: "Steward { run_id: \"run-1\" }" }, "A memory write from the memory steward was quarantined"],
      ["memory.write.quarantined", { author: { author: "agent", identity: "identity:router" } }, "A memory write from Router was quarantined"],
      ["memory.taint.transition", { kind: "tainted", session_key: "sess-1", source: "a configured untrusted tool" }, "A session became untrusted · a configured untrusted tool"],
      ["memory.budget.denied", { stage: "harvest", reason: "over cap" }, "Memory budget reached during harvest · over cap"],
      ["memory.promotion.pending_gate", { scope_kind: "mob", scope_key: "main" }, "A record waits for approval to move to team memory"],
      ["memory.harvest.completed", { promoted: 2, tombstoned: 1, identity: "identity:router" }, "Router's memory was summarized · 2 kept, 1 discarded"],
      ["memory.distill.timed_out", { cause: "slow", session_key: "sess-2" }, "Summarizing a conversation timed out · slow"],
      ["memory.hygiene.applied", { ops: 4 }, "Memory tidy-up applied · 4 changes"],
      ["memory.hygiene.blocked", { reason: "revision drift" }, "Memory tidy-up blocked · revision drift"],
      ["memory.future_event", { reason: "new" }, "Memory future event · new"],
    ];
    for (const [event, data, expected] of cases) {
      const line = describeMemoryEventForPeople(event, data, names);
      expect(line).toBe(expected);
      expect(/identity:|sess-|run-1|mob:|\u2014|\u2013/.test(line)).toBe(false);
    }
  });

  it("reads memory authors from the server's Debug text, structured authors and bare keys", () => {
    expect(parseMemoryAuthor("Agent { identity: \"billing\" }")).toEqual({ author: "agent", identity: "billing" });
    expect(parseMemoryAuthor("Agent { identity: \"say \\\"hi\\\" \\u{e9}\" }")).toEqual({ author: "agent", identity: "say \"hi\" \u00e9" });
    expect(parseMemoryAuthor("Steward { run_id: \"dream-7\" }")).toEqual({ author: "steward", run_id: "dream-7" });
    expect(parseMemoryAuthor("Distiller { run_id: \"\" }")).toEqual({ author: "distiller" });
    expect(parseMemoryAuthor("Operator")).toEqual({ author: "operator" });
    expect(parseMemoryAuthor(" Application ")).toEqual({ author: "application" });
    expect(parseMemoryAuthor("identity:router")).toEqual({ author: "agent", identity: "identity:router" });
    expect(parseMemoryAuthor({ author: "steward", run_id: "dream-7" })).toEqual({ author: "steward", run_id: "dream-7" });
    expect(parseMemoryAuthor("Agent { name: \"x\" }")).toBe(null);
    expect(parseMemoryAuthor("")).toBe(null);
    expect(parseMemoryAuthor(42)).toBe(null);
    const words = (value: unknown) => {
      const author = parseMemoryAuthor(value);
      return author ? memoryAuthorWords(author, names) : "";
    };
    expect(words("Agent { identity: \"identity:router\" }")).toBe("Router");
    expect(words("Steward { run_id: \"dream-7\" }")).toBe("the memory steward");
    expect(words("Operator")).toBe("an operator");
  });
});
