import { describe, expect, it } from "vitest";

import { describeMemoryEventForPeople } from "./memory-event-words";

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
      ["memory.quarantine.verdict", { verdict: "promote" }, "Quarantine verdict: moved to team or operator memory"],
      ["memory.record.promoted", {}, "A record was moved"],
      ["memory.quarantine.release_blocked", { record_id: "rec-9", verdict: "release", class: "private-key" }, "Release of “Deploy key for staging” blocked · it looks like a secret"],
      ["memory.quarantine.release_blocked", { record_id: "rec-10", verdict: "promote_pending_gate" }, "Moving a quarantined record was blocked"],
      ["memory.conflict.signal", { entity: "user", topic: "time zone", reason: "mismatch" }, "Conflicting memories about user, time zone · mismatch"],
      ["memory.write.quarantined", { author: "identity:router", reason: "tainted session" }, "A memory write from Router was quarantined · tainted session"],
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
});
