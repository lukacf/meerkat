import assert from "node:assert/strict";
import test from "node:test";

import { MEMORY_TABS, __memoryTest } from "./MemoryPanel";
import type {
  MemoryDreamRun,
  MemoryDreamRunSheet,
  MemoryFullRecord,
  MemoryLedgerEntry,
  MemoryPanelRecord,
  MemoryPanelRecordsResult,
  MemoryQuarantineDecideResult,
  MemoryReviewedRecordReceipt,
  MemoryScopeOverview,
} from "../types";
import type { ConsoleFrame } from "../types";

const {
  scopeGroupKey,
  scopeGroupLabel,
  groupRecordsByScope,
  trustLabel,
  statusLabel,
  relativeAge,
  evidenceLabel,
  authorLine,
  dreamOpKindsSummary,
  dreamTimeRange,
  injectionLine,
  memoryTabLabel,
  resolveMemoryTabAlias,
  buildRecordsQueryParams,
  hasActiveFilter,
  filtersEquivalent,
  memorySectionOutcome,
  buildRecordsListView,
  createMemoryRecordsPager,
  latticeFingerprint,
  recordUtility,
  sortRecordsByUtility,
  utilityLine,
  formatBytes,
  latticeInvariants,
  runLatticeWalk,
  computeVerdictTiles,
  verdictStatusLabel,
  scopeOverviewRows,
  filterForScope,
  lineageLane,
  dreamRunsTouching,
  evidenceExcerptLines,
  identityOptions,
  knowledgeComposition,
  dedupeFramesById,
  countFramesBehind,
  memoryFramePivot,
  overviewScopeKey,
  overviewScopeLabel,
  filterForOverviewScope,
  visibleOverviewScopes,
  sortOverviewScopes,
  storeFloorVerdict,
  annotateInjectionDups,
  dreamRunsNewestFirst,
  formatDurationMs,
  dreamRunDuration,
  normalizeDreamRunDetail,
  canDecideQuarantinedRecord,
  quarantineRationaleProblem,
  quarantineDecisionSummary,
  quarantineDecisionErrorText,
} = __memoryTest;

function record(overrides: Partial<MemoryPanelRecord> & Pick<MemoryPanelRecord, "id" | "scope">): MemoryPanelRecord {
  return {
    kind: "fact",
    title: overrides.id,
    trust: "agent_observed",
    status: { status: "active" },
    ...overrides,
  } as MemoryPanelRecord;
}

test("scope group keys separate identities but collapse mob/operator/realm", () => {
  assert.equal(
    scopeGroupKey({ scope: "identity", realm: "default", identity: "identity:luka" }),
    "identity:default:identity:luka",
  );
  assert.equal(scopeGroupKey({ scope: "mob", realm: "default", mob: "main" }), "mob:default:main");
  assert.equal(scopeGroupKey({ scope: "operator", realm: "default", operator: "op-1" }), "operator:default:op-1");
  assert.equal(scopeGroupKey({ scope: "realm", realm: "default" }), "realm:default");
});

test("scope group labels are human readable", () => {
  assert.equal(scopeGroupLabel({ scope: "identity", realm: "default", identity: "identity:luka" }), "identity:luka");
  assert.equal(scopeGroupLabel({ scope: "mob", realm: "default", mob: "main" }), "Mob: main");
  assert.equal(scopeGroupLabel({ scope: "operator", realm: "default", operator: "op-1" }), "Operator: op-1");
  assert.equal(scopeGroupLabel({ scope: "realm", realm: "default" }), "Realm");
});

test("records group by scope with identity groups first, then mob/operator/realm", () => {
  const groups = groupRecordsByScope([
    record({ id: "r-realm", scope: { scope: "realm", realm: "default" } }),
    record({ id: "l1", scope: { scope: "identity", realm: "default", identity: "identity:luka" } }),
    record({ id: "m1", scope: { scope: "mob", realm: "default", mob: "main" } }),
    record({ id: "l2", scope: { scope: "identity", realm: "default", identity: "identity:luka" } }),
    record({ id: "o1", scope: { scope: "operator", realm: "default", operator: "op-1" } }),
  ]);

  assert.deepEqual(
    groups.map((g) => g.label),
    ["identity:luka", "Mob: main", "Operator: op-1", "Realm"],
  );
  // Both luka records land in the same identity group.
  assert.equal(groups[0].records.length, 2);
  assert.deepEqual(groups[0].records.map((r) => r.id), ["l1", "l2"]);
});

test("trust and status labels are readable", () => {
  assert.equal(trustLabel("agent_verified"), "verified");
  assert.equal(trustLabel("operator"), "operator");
  assert.equal(statusLabel({ status: "active" }), "active");
  assert.equal(statusLabel({ status: "superseded", by: "mem-9" }), "superseded → mem-9");
  assert.equal(statusLabel({ status: "quarantined", reason: "conflict" }), "quarantined: conflict");
  assert.equal(statusLabel({ status: "tombstoned" }), "tombstoned");
});

test("relative age formats coarse buckets and handles missing timestamps", () => {
  const now = 1_000_000_000_000;
  assert.equal(relativeAge(undefined, now), "—");
  assert.equal(relativeAge(0, now), "—");
  assert.equal(relativeAge(now - 3 * 24 * 60 * 60 * 1000, now), "3d ago");
  assert.equal(relativeAge(now - 5 * 60 * 1000, now), "5m ago");
  assert.equal(relativeAge(now - 500, now), "now");
});

test("evidence labels render session/gen/msg ranges without transcript fetch", () => {
  assert.equal(
    evidenceLabel({ session_id: "s", generation: 2, range: [1, 5] }),
    "session s • gen 2 • msgs 1–5",
  );
  assert.equal(evidenceLabel({ revision: "r7" }), "rev r7");
  assert.equal(evidenceLabel({}), "evidence");
});

test("author lines describe every author variant", () => {
  assert.equal(authorLine({ author: "agent", identity: "identity:x" }), "agent identity:x");
  assert.equal(authorLine({ author: "steward", run_id: "run-1" }), "steward run run-1");
  assert.equal(authorLine({ author: "distiller", run_id: "run-2" }), "distiller run run-2");
  assert.equal(authorLine({ author: "operator" }), "operator");
  assert.equal(authorLine({ author: "application" }), "application");
  assert.equal(authorLine(undefined), "unknown author");
});

test("dream op-kind summary sorts by count and skips zeroes", () => {
  assert.equal(dreamOpKindsSummary({ create: 3, tombstone: 1, noop: 0 }), "3 create · 1 tombstone");
  assert.equal(dreamOpKindsSummary({}), "");
  assert.equal(dreamOpKindsSummary(undefined), "");
});

test("dream time range collapses identical endpoints", () => {
  const now = 2_000_000_000_000;
  const first = now - 2 * 60 * 60 * 1000;
  const last = now - 30 * 60 * 1000;
  assert.equal(dreamTimeRange({ first_op_at_ms: first, last_op_at_ms: last }, now), "2h ago → 30m ago");
  assert.equal(dreamTimeRange({ first_op_at_ms: last, last_op_at_ms: last }, now), "30m ago");
  assert.equal(dreamTimeRange({}, now), "—");
});

test("injection lines combine surface, identity, and relative time", () => {
  const now = 3_000_000_000_000;
  assert.equal(
    injectionLine({ record_id: "m", identity: "identity:luka", surface: "build", at_ms: now - 60 * 60 * 1000 }, now),
    "build • identity:luka • 1h ago",
  );
  assert.equal(
    injectionLine({ record_id: "m", identity: "identity:luka", surface: "turn", at_ms: now - 5 * 1000 }, now),
    "turn • identity:luka • 5s ago",
  );
});

// ── Phase-1: tab restructure ───────────────────────────────────────────────

test("tab union is holdings/records/knowledge/pipeline/dreams with labels", () => {
  assert.deepEqual(MEMORY_TABS, ["holdings", "records", "knowledge", "pipeline", "dreams"]);
  assert.equal(memoryTabLabel("holdings"), "Holdings");
  assert.equal(memoryTabLabel("pipeline"), "Pipeline");
});

test("quarantine tab alias redirects to pipeline for one release (open Q6)", () => {
  assert.equal(resolveMemoryTabAlias("quarantine"), "pipeline");
  assert.equal(resolveMemoryTabAlias("records"), "records");
  assert.equal(resolveMemoryTabAlias("bogus"), null);
});

// ── Phase-1: records filter bar param building ─────────────────────────────

test("filter bar builds the server's scope/identity/scope_key/status params", () => {
  assert.deepEqual(buildRecordsQueryParams({}), {});
  assert.deepEqual(buildRecordsQueryParams({ key: "identity:ada" }), { identity: "identity:ada" });
  assert.deepEqual(
    buildRecordsQueryParams({ scope: "identity", key: "identity:ada", status: "active" }),
    { identity: "identity:ada", status: "active" },
  );
  assert.deepEqual(buildRecordsQueryParams({ scope: "identity" }), { scope: "identity" });
  assert.deepEqual(
    buildRecordsQueryParams({ scope: "mob", key: "research" }),
    { scope: "mob", scope_key: "research" },
  );
  assert.deepEqual(buildRecordsQueryParams({ scope: "realm" }), { scope: "realm" });
  assert.deepEqual(
    buildRecordsQueryParams({ status: "quarantined" }, { cursor: "123:m-1", limit: 200 }),
    { status: "quarantined", limit: 200, cursor: "123:m-1" },
  );
  // Realm names the single realm whose keyset cursor makes paging honest.
  assert.deepEqual(buildRecordsQueryParams({ realm: "example" }), { realm: "example" });
  assert.deepEqual(
    buildRecordsQueryParams({ scope: "mob", key: "research", realm: "example" }),
    { scope: "mob", scope_key: "research", realm: "example" },
  );
  assert.equal(hasActiveFilter({}), false);
  assert.equal(hasActiveFilter({ key: "  " }), false);
  assert.equal(hasActiveFilter({ status: "active" }), true);
  assert.equal(hasActiveFilter({ realm: "example" }), true);
  assert.equal(filtersEquivalent({ realm: "example" }, { realm: " example " }), true);
  assert.equal(filtersEquivalent({ realm: "example" }, {}), false);
});

test("memory section outcomes classify -32030 as denied and -32601 as unavailable", () => {
  // Mirror the transport contract: network.ts annotates thrown errors with
  // `rpcError` (code/message/data) — the exact shape jsonRpcErrorCode reads.
  const rpcError = (code: number): Error => {
    const error = new Error(`RPC error ${code}`);
    (error as Error & { rpcError?: { code?: unknown } }).rpcError = { code };
    return error;
  };
  assert.equal(memorySectionOutcome(rpcError(-32030)), "denied");
  assert.equal(memorySectionOutcome(rpcError(-32601)), "unavailable");
  assert.equal(memorySectionOutcome(rpcError(-32000)), "error");
  assert.equal(memorySectionOutcome(new Error("network down")), "error");
  assert.equal(memorySectionOutcome(null), "error");
  // A malformed annotation (non-numeric code) must not classify as denied.
  const malformed = new Error("weird");
  (malformed as Error & { rpcError?: unknown }).rpcError = { code: "-32030" };
  assert.equal(memorySectionOutcome(malformed), "error");
});

test("a per-method capability miss classifies as denied (intersection under enforcement)", () => {
  // Exact shape thrown by requireCapability (headless.ts): a plain Error,
  // raised client-side before the RPC, with no rpcError annotation. Under an
  // enforced view the server drops methods the principal cannot call from
  // meta/capabilities — e.g. a scoped read grant loses panel/dreams — so
  // the miss is denial by intersection, not a fatal panel error.
  assert.equal(
    memorySectionOutcome(new Error("Meta capability missing for meta/memory/panel/dreams")),
    "denied",
  );
  assert.equal(
    memorySectionOutcome(
      new Error("Meta capability missing for meta/memory/panel/quarantine"),
    ),
    "denied",
  );
  // The prefix must anchor at the start: mentions elsewhere are not misses.
  assert.equal(
    memorySectionOutcome(new Error("saw 'Meta capability missing for x' in logs")),
    "error",
  );
  // Non-Error values with a matching-looking message do not classify.
  assert.equal(
    memorySectionOutcome("Meta capability missing for meta/memory/panel/dreams"),
    "error",
  );
});

// ── Phase-1: recall utility mode ───────────────────────────────────────────

test("record utility flags dead weight and approximates bytes spent", () => {
  const dead = recordUtility({
    usage: { injected_count: 5, judged_useful_count: 0, explicit_recall_count: 0 },
    body_bytes: 1024,
  });
  assert.equal(dead.dead, true);
  assert.equal(dead.bytesSpent, 5 * 1024);
  assert.equal(dead.ratio, 0);

  const useful = recordUtility({
    usage: { injected_count: 4, judged_useful_count: 2 },
    body_bytes: 100,
  });
  assert.equal(useful.dead, false);
  assert.equal(useful.ratio, 0.5);

  const fresh = recordUtility({ usage: undefined, body_bytes: 100 });
  assert.equal(fresh.dead, false);
  assert.equal(fresh.ratio, null);
});

test("utility sort leads with dead weight, then ascending usefulness", () => {
  const mk = (id: string, injected: number, useful: number, bytes: number) =>
    record({
      id,
      scope: { scope: "realm", realm: "default" },
      usage: { injected_count: injected, judged_useful_count: useful },
      body_bytes: bytes,
    });
  const sorted = sortRecordsByUtility([
    mk("useful", 4, 4, 10),
    mk("dead-small", 3, 0, 10),
    mk("dead-big", 6, 0, 1000),
    mk("meh", 4, 1, 10),
  ]);
  assert.deepEqual(sorted.map((r) => r.id), ["dead-big", "dead-small", "meh", "useful"]);
  assert.match(utilityLine(sorted[0]), /inj 6 .* useful 0 .* ratio 0\.00/);
  assert.equal(formatBytes(6 * 1000), "5.9KB");
});

// ── Phase-1: lattice audit (invariants a + c) ──────────────────────────────

test("lattice invariants catch LLM-authored records above the ceiling", () => {
  const ok = record({
    id: "ok",
    scope: { scope: "realm", realm: "default" },
    trust: "agent_observed",
    provenance: { author: { author: "distiller", run_id: "d-1" } },
  });
  const violation = record({
    id: "bad",
    scope: { scope: "realm", realm: "default" },
    trust: "agent_verified",
    provenance: { author: { author: "agent", identity: "identity:ada" } },
  });
  const operatorRecord = record({
    id: "op",
    scope: { scope: "realm", realm: "default" },
    trust: "operator",
    provenance: { author: { author: "operator" } },
  });
  const result = latticeInvariants([ok, violation, operatorRecord], { complete: true });
  assert.deepEqual(result.llmCeilingViolations, [{ id: "bad", realm: "default" }]);
});

test("lattice invariants catch supersede cycles; dangling only when complete", () => {
  const a = record({ id: "a", scope: { scope: "realm", realm: "default" }, supersedes: "b" });
  const b = record({ id: "b", scope: { scope: "realm", realm: "default" }, supersedes: "a" });
  const dangling = record({
    id: "c",
    scope: { scope: "realm", realm: "default" },
    supersedes: "missing",
  });
  const complete = latticeInvariants([a, b, dangling], { complete: true });
  assert.deepEqual(
    complete.chainViolations.map((ref) => ref.id).sort(),
    ["a", "b", "c"],
  );
  assert.equal(complete.chainViolations[0].realm, "default");
  const partial = latticeInvariants([dangling], { complete: false });
  assert.deepEqual(partial.chainViolations, []);
});

test("lattice walk pages a single realm with the keyset cursor", async () => {
  const pageA: MemoryPanelRecordsResult = {
    records: [record({ id: "r1", scope: { scope: "realm", realm: "default" } })],
    next_cursor: "cursor-2",
    realms: ["default"],
  };
  const pageB: MemoryPanelRecordsResult = {
    records: [record({ id: "r2", scope: { scope: "realm", realm: "default" } })],
    next_cursor: null,
    realms: ["default"],
  };
  const calls: Array<Record<string, unknown>> = [];
  const walk = await runLatticeWalk(
    async (params) => {
      calls.push(params);
      return params.cursor ? pageB : pageA;
    },
    { realms: ["default"] },
  );
  assert.equal(calls.length, 2);
  // Single-realm gateways use the unscoped listing (already single-realm).
  assert.equal(calls[0].realm, undefined);
  assert.equal(calls[1].cursor, "cursor-2");
  assert.equal(walk?.checked, 2);
  assert.equal(walk?.complete, true);

  // Access denial propagates as null → the tile renders "no grant".
  const denied = await runLatticeWalk(async () => null, { realms: ["default"] });
  assert.equal(denied, null);
});

test("lattice walk on multi-realm gateways walks each realm to exhaustion", async () => {
  // Server-real shapes: an unscoped multi-realm merge NEVER carries a
  // cursor; realm-scoped pages do. The walk must never call unscoped here.
  const realmPages: Record<string, MemoryPanelRecordsResult[]> = {
    alpha: [
      {
        records: [record({ id: "a-1", scope: { scope: "realm", realm: "alpha" } })],
        next_cursor: "alpha-2",
        realms: ["alpha"],
      },
      {
        records: [record({ id: "a-2", scope: { scope: "realm", realm: "alpha" } })],
        next_cursor: null,
        realms: ["alpha"],
      },
    ],
    beta: [
      {
        records: [record({ id: "b-1", scope: { scope: "realm", realm: "beta" } })],
        next_cursor: null,
        realms: ["beta"],
      },
    ],
  };
  const calls: Array<Record<string, unknown>> = [];
  const served: Record<string, number> = { alpha: 0, beta: 0 };
  const walk = await runLatticeWalk(
    async (params) => {
      calls.push(params);
      const realm = String(params.realm);
      return realmPages[realm][served[realm]++];
    },
    { realms: ["alpha", "beta"] },
  );
  assert.deepEqual(
    calls.map((call) => call.realm),
    ["alpha", "alpha", "beta"],
  );
  assert.equal(calls[1].cursor, "alpha-2");
  assert.equal(walk?.checked, 3);
  // Every realm's cursor ran out under the cap → honestly complete.
  assert.equal(walk?.complete, true);
});

test("lattice walk cap and cancellation keep the partial header honest", async () => {
  const endless: MemoryPanelRecordsResult = {
    records: Array.from({ length: 3 }, (_, index) =>
      record({ id: `r-${index}`, scope: { scope: "realm", realm: "alpha" } }),
    ),
    next_cursor: "more",
    realms: ["alpha"],
  };
  // Cap hit mid-realm with a live cursor → complete=false.
  const capped = await runLatticeWalk(async () => endless, {
    realms: ["alpha", "beta"],
    maxRecords: 5,
  });
  assert.equal(capped?.checked, 5);
  assert.equal(capped?.complete, false);

  // A cancelled walk stops issuing page fetches instead of finishing.
  let fetches = 0;
  let cancelled = false;
  const result = await runLatticeWalk(
    async () => {
      fetches += 1;
      cancelled = true; // cancel after the first page lands
      return endless;
    },
    { realms: ["alpha"], isCancelled: () => cancelled },
  );
  assert.equal(fetches, 1);
  assert.equal(result, null);
});

// ── Phase-1: verdict tiles ─────────────────────────────────────────────────

const emptyVerdictInputs = {
  records: [] as MemoryPanelRecord[],
  recordsDenied: false,
  dreams: [] as MemoryDreamRun[],
  dreamsDenied: false,
  lattice: null,
};

test("verdict strip mounts six tiles; missing surfaces are named", () => {
  const tiles = computeVerdictTiles(emptyVerdictInputs);
  assert.deepEqual(
    tiles.map((tile) => tile.id),
    ["echo-safety", "taint-wall", "lattice", "recall", "dreams", "store-floor"],
  );
  const byId = new Map(tiles.map((tile) => [tile.id, tile]));
  assert.equal(byId.get("echo-safety")?.status, "unverifiable");
  assert.match(byId.get("echo-safety")?.lines[0] || "", /not available here/);
  assert.equal(byId.get("taint-wall")?.status, "unverifiable");
  assert.match(byId.get("taint-wall")?.lines[0] || "", /not available here/);
  assert.equal(byId.get("store-floor")?.status, "unverifiable");
  assert.match(byId.get("store-floor")?.lines[0] || "", /panel\/overview/);
  assert.equal(verdictStatusLabel("unverifiable"), "UNVERIFIABLE");
});

test("tiles whose evidence the principal cannot read render no-grant, never green", () => {
  const tiles = computeVerdictTiles({
    ...emptyVerdictInputs,
    recordsDenied: true,
    dreamsDenied: true,
  });
  const byId = new Map(tiles.map((tile) => [tile.id, tile]));
  assert.equal(byId.get("lattice")?.status, "no-grant");
  assert.equal(byId.get("recall")?.status, "no-grant");
  assert.equal(byId.get("dreams")?.status, "no-grant");
  assert.equal(byId.get("taint-wall")?.status, "no-grant");
  assert.equal(verdictStatusLabel("no-grant"), "NO GRANT");
});

test("lattice tile reports violations and the partial-walk header honestly", () => {
  const holding = computeVerdictTiles({
    ...emptyVerdictInputs,
    lattice: { checked: 3, complete: true, llmCeilingViolations: [], chainViolations: [] },
  }).find((tile) => tile.id === "lattice");
  assert.equal(holding?.status, "holding");
  assert.match(holding?.lines[1] || "", /checked 3\/3/);
  assert.deepEqual(holding?.evidence, []);

  const capped = computeVerdictTiles({
    ...emptyVerdictInputs,
    lattice: {
      checked: 2000,
      complete: false,
      llmCeilingViolations: [{ id: "x", realm: "default" }],
      chainViolations: [],
    },
  }).find((tile) => tile.id === "lattice");
  assert.equal(capped?.status, "violated");
  assert.match(capped?.lines[1] || "", /first 2000 — partial/);
});

test("violated lattice tile carries its evidence refs, capped with a +N more line", () => {
  const violations = Array.from({ length: 7 }, (_, index) => ({
    id: `bad-${index}`,
    realm: "default",
  }));
  const tile = computeVerdictTiles({
    ...emptyVerdictInputs,
    lattice: {
      checked: 100,
      complete: true,
      llmCeilingViolations: violations.slice(0, 4),
      chainViolations: violations.slice(4),
    },
  }).find((candidate) => candidate.id === "lattice");
  assert.equal(tile?.status, "violated");
  assert.equal(tile?.evidence?.length, 5);
  assert.deepEqual(tile?.evidence?.[0], { id: "bad-0", realm: "default" });
  assert.ok(tile?.lines.some((line) => line === "+2 more violations"));
});

test("a settled lattice verdict is retained (with a re-check marker) during re-runs", () => {
  const settled = {
    checked: 10,
    complete: true,
    llmCeilingViolations: [],
    chainViolations: [],
  };
  const idle = computeVerdictTiles({
    ...emptyVerdictInputs,
    lattice: settled,
    latticeRunning: false,
  }).find((candidate) => candidate.id === "lattice");
  const tile = computeVerdictTiles({
    ...emptyVerdictInputs,
    lattice: settled,
    latticeRunning: true,
  }).find((candidate) => candidate.id === "lattice");
  // No flicker to UNVERIFIABLE while the debounced refresh re-walks.
  assert.equal(tile?.status, "holding");
  assert.equal(tile?.rechecking, true);
  assert.equal(idle?.rechecking, false);
  // The re-check renders over the tile, so it adds no line that would grow
  // the tile row and move the tiles below.
  assert.deepEqual(tile?.lines, idle?.lines);

  const cold = computeVerdictTiles({
    ...emptyVerdictInputs,
    lattice: null,
    latticeRunning: true,
  }).find((candidate) => candidate.id === "lattice");
  assert.equal(cold?.status, "unverifiable");
  assert.match(cold?.lines[0] || "", /page-walk running/);
});

test("lattice fingerprint is stable across content-identical refreshes", () => {
  const rows = [
    record({ id: "a", scope: { scope: "realm", realm: "default" }, updated_at_ms: 5 }),
  ];
  const again = [
    record({ id: "a", scope: { scope: "realm", realm: "default" }, updated_at_ms: 5 }),
  ];
  assert.equal(
    latticeFingerprint(rows, ["default"], "c-1"),
    latticeFingerprint(again, ["default"], "c-1"),
  );
  assert.notEqual(
    latticeFingerprint(rows, ["default"], "c-1"),
    latticeFingerprint(rows, ["default"], "c-2"),
  );
  const changed = [{ ...rows[0], trust: "operator" as const }];
  assert.notEqual(
    latticeFingerprint(rows, ["default"], "c-1"),
    latticeFingerprint(changed, ["default"], "c-1"),
  );
});

test("recall and dreams tiles compute from loaded usage and dream recency", () => {
  const now = 4_000_000_000_000;
  const tiles = computeVerdictTiles({
    records: [
      record({
        id: "dead",
        scope: { scope: "realm", realm: "default" },
        usage: { injected_count: 9, judged_useful_count: 0 },
        body_bytes: 100,
      }),
      record({ id: "fine", scope: { scope: "realm", realm: "default" } }),
    ],
    recordsDenied: false,
    dreams: [
      {
        realm: "default",
        run_id: "run-88",
        last_op_at_ms: now - 2 * 60 * 60 * 1000,
        quarantined_ops: 1,
      },
    ],
    dreamsDenied: false,
    lattice: null,
    now,
  });
  const byId = new Map(tiles.map((tile) => [tile.id, tile]));
  assert.equal(byId.get("recall")?.status, "degraded");
  assert.match(byId.get("recall")?.lines[0] || "", /1 dead weight of 2/);
  assert.equal(byId.get("dreams")?.status, "holding");
  assert.match(byId.get("dreams")?.lines[0] || "", /last run 2h ago/);
  assert.match(byId.get("dreams")?.lines[1] || "", /1 quarantined/);
});

// ── Phase-1: holdings scope overview ───────────────────────────────────────

test("scope overview rows count statuses, bytes, and trust mix per scope", () => {
  const rows = scopeOverviewRows([
    record({
      id: "a1",
      scope: { scope: "identity", realm: "default", identity: "identity:ada" },
      status: { status: "active" },
      body_bytes: 100,
    }),
    record({
      id: "a2",
      scope: { scope: "identity", realm: "default", identity: "identity:ada" },
      status: { status: "quarantined", reason: "taint" },
      trust: "untrusted",
      body_bytes: 50,
    }),
    record({ id: "r1", scope: { scope: "realm", realm: "default" } }),
  ]);
  assert.equal(rows.length, 2);
  assert.equal(rows[0].label, "identity:ada");
  assert.equal(rows[0].active, 1);
  assert.equal(rows[0].quarantined, 1);
  assert.equal(rows[0].bytes, 150);
  assert.match(rows[0].trustMix, /1 observed/);
  assert.match(rows[0].trustMix, /1 untrusted/);
});

test("scope rows pivot into the records filter that reproduces them", () => {
  assert.deepEqual(
    filterForScope({ scope: "identity", realm: "default", identity: "identity:ada" }),
    { scope: "identity", key: "identity:ada" },
  );
  assert.deepEqual(
    filterForScope({ scope: "mob", realm: "default", mob: "research" }),
    { scope: "mob", key: "research" },
  );
  assert.deepEqual(filterForScope({ scope: "realm", realm: "default" }), { scope: "realm" });
});

// ── Phase-1: biography sections ────────────────────────────────────────────

test("lineage lane renders newest-first with the current record marked", () => {
  const chain = [
    record({ id: "oldest", scope: { scope: "realm", realm: "default" } }),
    record({ id: "middle", scope: { scope: "realm", realm: "default" } }),
    record({ id: "current", scope: { scope: "realm", realm: "default" } }),
  ];
  const lane = lineageLane(chain, "current");
  assert.deepEqual(lane.map((entry) => entry.record.id), ["current", "middle", "oldest"]);
  assert.deepEqual(lane.map((entry) => entry.current), [true, false, false]);
});

test("biography dreams section joins through the lossy memory_ids sample", () => {
  const dreams: MemoryDreamRun[] = [
    { realm: "default", run_id: "run-1", memory_ids: ["m-1", "m-2"] },
    { realm: "default", run_id: "run-2", memory_ids: ["m-3"] },
    { realm: "default", run_id: "run-3" },
  ];
  assert.deepEqual(dreamRunsTouching(dreams, "m-1").map((run) => run.run_id), ["run-1"]);
  assert.deepEqual(dreamRunsTouching(dreams, "m-9"), []);
});

test("evidence excerpts slice the message window and skip non-text entries", () => {
  const entries = [
    { kind: "message", id: "e0", identity: { id: "u", label: "You", role: "user" }, text: "zero" },
    { kind: "message", id: "e1", identity: { id: "a", label: "Ada", role: "assistant" }, text: "one" },
    { kind: "message", id: "e2", identity: { id: "a", label: "Ada", role: "assistant" }, text: "" },
    { kind: "message", id: "e3", identity: { id: "u", label: "You", role: "user" }, text: "three" },
  ] as never[];
  const all = evidenceExcerptLines(entries);
  assert.deepEqual(all.map((line) => line.text), ["zero", "one", "three"]);
  const windowed = evidenceExcerptLines(entries, [1, 3]);
  assert.deepEqual(windowed.map((line) => line.text), ["one", "three"]);
  assert.equal(windowed[0].speaker, "Ada");
});

// ── Phase-1: knowledge lens composition ────────────────────────────────────

test("knowledge lens lists identities and composes the scope union", () => {
  const records = [
    record({ id: "a", scope: { scope: "identity", realm: "default", identity: "identity:ada" } }),
    record({ id: "b", scope: { scope: "identity", realm: "default", identity: "identity:bob" } }),
    record({ id: "m", scope: { scope: "mob", realm: "default", mob: "research" } }),
    record({ id: "r", scope: { scope: "realm", realm: "default" } }),
  ];
  assert.deepEqual(identityOptions(records), ["identity:ada", "identity:bob"]);
  const segments = knowledgeComposition(records, "identity:ada");
  assert.equal(segments[0].count, 1);
  assert.equal(segments[0].approximate, false);
  assert.equal(segments[1].count, 1); // all mob-scope rows, flagged approximate
  assert.equal(segments[1].approximate, true);
  assert.equal(segments[3].count, 1);
});

// ── Phase-1: live strip + pivots ───────────────────────────────────────────

function frame(overrides: Partial<ConsoleFrame> & Pick<ConsoleFrame, "id" | "event">): ConsoleFrame {
  return { data: {}, ...overrides } as ConsoleFrame;
}

test("live strip dedupes snapshot replays by frame id and counts frames behind", () => {
  const a = frame({ id: "f-1", event: "memory.dream.completed" });
  const replay = frame({ id: "f-1", event: "memory.dream.completed" });
  const b = frame({ id: "f-2", event: "memory.record.promoted" });
  const deduped = dedupeFramesById([a, replay, b]);
  assert.deepEqual(deduped.map((f) => f.id), ["f-1", "f-2"]);
  assert.equal(countFramesBehind(deduped, [a]), 1);
  assert.equal(countFramesBehind(deduped, deduped), 0);
});

test("memory frames pivot to the record their payload names", () => {
  assert.deepEqual(
    memoryFramePivot(
      frame({
        id: "f-1",
        event: "memory.record.promoted",
        data: { record_id: "m-7", realm: "example" },
      }),
    ),
    { recordId: "m-7", realm: "example" },
  );
  assert.equal(
    memoryFramePivot(frame({ id: "f-2", event: "memory.dream.started", data: {} })),
    null,
  );
  assert.equal(
    memoryFramePivot(frame({ id: "f-3", event: "interaction_complete", data: { record_id: "x" } })),
    null,
  );
});

// ── Fix round: e2e-surfaced defects (regression net) ──────────────────────

function pagerHarness() {
  const state: {
    paged: { records: MemoryPanelRecord[]; nextCursor: string | null } | null;
    loading: boolean;
    queries: Array<Record<string, unknown>>;
  } = { paged: null, loading: false, queries: [] };
  const pending: Array<(result: MemoryPanelRecordsResult | null) => void> = [];
  const pager = createMemoryRecordsPager({
    query: (params) => {
      state.queries.push(params);
      return new Promise((resolve) => pending.push(resolve));
    },
    setPaged: (paged) => {
      state.paged = paged;
    },
    setLoading: (loading) => {
      state.loading = loading;
    },
  });
  const page = (ids: string[], nextCursor: string | null): MemoryPanelRecordsResult => ({
    records: ids.map((id) => record({ id, scope: { scope: "realm", realm: "default" } })),
    next_cursor: nextCursor,
    realms: ["default"],
  });
  return { state, pending, pager, page };
}

test("bug 1: a stale filter response never clobbers a newer query (issue-order wins)", async () => {
  const { state, pending, pager, page } = pagerHarness();

  const broad = pager.applyFilter({ scope: "mob" }); // resolves LAST
  const narrow = pager.applyFilter({ scope: "mob", key: "research" });
  assert.equal(pending.length, 2);

  // Newer (narrow) resolves first and applies…
  pending[1](page(["narrow-1"], null));
  await narrow;
  assert.deepEqual(state.paged?.records.map((r) => r.id), ["narrow-1"]);

  // …then the older broad response arrives and must be dropped.
  pending[0](page(["broad-1", "broad-2"], "cursor-x"));
  await broad;
  assert.deepEqual(state.paged?.records.map((r) => r.id), ["narrow-1"]);
  assert.equal(state.paged?.nextCursor, null);
  assert.equal(state.loading, false);
});

test("bug 1: clearing the filter invalidates an in-flight fetch", async () => {
  const { state, pending, pager, page } = pagerHarness();

  const filtered = pager.applyFilter({ status: "active" });
  await pager.applyFilter({}); // clear — synchronous reset, bumps the sequence
  assert.equal(state.paged, null);

  pending[0](page(["stale-1"], null));
  await filtered;
  assert.equal(state.paged, null, "stale filtered page must not resurrect after clear");
  assert.equal(state.loading, false);
});

test("bug 2: blur re-apply is a no-op unless the filter value actually changed", async () => {
  const { state, pending, pager, page } = pagerHarness();

  await Promise.all([
    pager.applyFilter({ scope: "mob", key: "research" }),
    (async () => {
      // resolve the initial apply
      while (pending.length === 0) await Promise.resolve();
      pending[0](page(["m-1"], "cursor-1"));
    })(),
  ]);
  assert.equal(state.queries.length, 1);

  // Unchanged (incl. whitespace-only difference) → no new query.
  await pager.applyFilterIfChanged({ scope: "mob", key: "research" });
  await pager.applyFilterIfChanged({ scope: "mob", key: " research " });
  assert.equal(state.queries.length, 1);

  // Real change → re-queries.
  const changed = pager.applyFilterIfChanged({ scope: "mob", key: "ops" });
  assert.equal(state.queries.length, 2);
  pending[1](page(["m-2"], null));
  await changed;
  assert.deepEqual(state.paged?.records.map((r) => r.id), ["m-2"]);

  assert.equal(filtersEquivalent({}, { key: "  " }), true);
  assert.equal(filtersEquivalent({ scope: "mob" }, { scope: "mob", key: "x" }), false);
});

test("bug 2: an unchanged blur during load-more cannot reset the appended page", async () => {
  const { state, pending, pager, page } = pagerHarness();
  const base = [record({ id: "base-1", scope: { scope: "realm", realm: "default" } })];

  const more = pager.loadMore({
    filter: {},
    paged: null,
    baseRecords: base,
    baseCursor: "cursor-1",
  });
  // Blur fired by the load-more click: filter is unchanged ({}), so this
  // must not issue a query or bump the sequence.
  await pager.applyFilterIfChanged({});
  assert.equal(state.queries.length, 1);

  pending[0](page(["more-1"], null));
  await more;
  assert.deepEqual(state.paged?.records.map((r) => r.id), ["base-1", "more-1"]);
});

test("bug 3: unfiltered load-more rows render — grouped view reads the accumulated page", () => {
  const base = [
    record({ id: "a1", scope: { scope: "identity", realm: "default", identity: "identity:ada" } }),
  ];
  const appended = [
    ...base,
    record({ id: "a2", scope: { scope: "identity", realm: "default", identity: "identity:ada" } }),
  ];

  const view = buildRecordsListView({
    records: base,
    paged: { records: appended, nextCursor: "cursor-2" },
    baseCursor: null,
    filter: {},
    sortMode: "recency",
  });
  assert.equal(view.mode, "grouped");
  assert.deepEqual(
    view.groups.flatMap((group) => group.records.map((r) => r.id)),
    ["a1", "a2"],
  );
  assert.equal(view.cursor, "cursor-2");

  // Without paged state the base page + cursor drive the view.
  const baseView = buildRecordsListView({
    records: base,
    paged: null,
    baseCursor: "cursor-1",
    filter: {},
    sortMode: "recency",
  });
  assert.equal(baseView.mode, "grouped");
  assert.equal(baseView.cursor, "cursor-1");

  // Filter or utility sort flips to the flat list over the same source.
  const flat = buildRecordsListView({
    records: base,
    paged: { records: appended, nextCursor: null },
    baseCursor: null,
    filter: { status: "active" },
    sortMode: "recency",
  });
  assert.equal(flat.mode, "flat");
  assert.deepEqual(flat.records.map((r) => r.id), ["a1", "a2"]);
});

// ── Gate fix round: denied propagation through the pager and list view ─────

test("a denied filtered query renders no-grant, never an empty store", async () => {
  const { state, pending, pager } = pagerHarness();

  const filtered = pager.applyFilter({ scope: "operator" });
  pending[0](null); // -32030 → the query callback resolves null
  await filtered;
  assert.deepEqual(state.paged, { records: [], nextCursor: null, denied: true });

  const view = buildRecordsListView({
    records: [],
    paged: state.paged,
    baseCursor: null,
    filter: { scope: "operator" },
    sortMode: "recency",
  });
  assert.equal(view.denied, true);
  assert.equal(view.records.length, 0);

  // Clearing the filter resets the denial.
  await pager.applyFilter({});
  assert.equal(state.paged, null);
});

test("a denied load-more continuation keeps the shown rows and flags the denial", async () => {
  const { state, pending, pager } = pagerHarness();
  const base = [record({ id: "base-1", scope: { scope: "realm", realm: "default" } })];

  const more = pager.loadMore({
    filter: {},
    paged: null,
    baseRecords: base,
    baseCursor: "cursor-1",
  });
  pending[0](null);
  await more;
  assert.deepEqual(state.paged?.records.map((r) => r.id), ["base-1"]);
  assert.equal(state.paged?.nextCursor, null);
  assert.equal(state.paged?.denied, true);
});

// ── Phase-2: overview scopes + STORE FLOOR tile ────────────────────────────

function overviewScope(
  overrides: Partial<MemoryScopeOverview> & Pick<MemoryScopeOverview, "scope_kind" | "scope_key">,
): MemoryScopeOverview {
  return { realm: "default", active: 1, body_bytes: 100, floor_pressure: false, ...overrides };
}

test("overview scope keys and labels match the loaded-records grouping scheme", () => {
  const identity = overviewScope({ scope_kind: "identity", scope_key: "router" });
  const mob = overviewScope({ scope_kind: "mob", scope_key: "research" });
  const operator = overviewScope({ scope_kind: "operator", scope_key: "op-1" });
  const realm = overviewScope({ scope_kind: "realm", scope_key: "" });
  // Parity with scopeGroupKey/scopeGroupLabel keeps testids and pivots
  // stable when Holdings flips from loaded rows to store totals.
  assert.equal(
    overviewScopeKey(identity),
    scopeGroupKey({ scope: "identity", realm: "default", identity: "router" }),
  );
  assert.equal(
    overviewScopeKey(mob),
    scopeGroupKey({ scope: "mob", realm: "default", mob: "research" }),
  );
  assert.equal(
    overviewScopeKey(realm),
    scopeGroupKey({ scope: "realm", realm: "default" }),
  );
  assert.equal(
    overviewScopeLabel(identity),
    scopeGroupLabel({ scope: "identity", realm: "default", identity: "router" }),
  );
  assert.equal(
    overviewScopeLabel(operator),
    scopeGroupLabel({ scope: "operator", realm: "default", operator: "op-1" }),
  );
  assert.equal(overviewScopeLabel(realm), "Realm");
  assert.deepEqual(filterForOverviewScope(identity), { scope: "identity", key: "router" });
  assert.deepEqual(filterForOverviewScope(mob), { scope: "mob", key: "research" });
  assert.deepEqual(filterForOverviewScope(realm), { scope: "realm" });
});

test("denied scope kinds vanish from the overview table (the denied row renders instead)", () => {
  const scopes = [
    overviewScope({ scope_kind: "identity", scope_key: "router" }),
    overviewScope({ scope_kind: "mob", scope_key: "research" }),
    overviewScope({ scope_kind: "operator", scope_key: "op-1" }),
  ];
  const visible = visibleOverviewScopes(scopes, { operatorScopeDenied: true });
  assert.deepEqual(
    visible.map((scope) => scope.scope_kind),
    ["identity", "mob"],
  );
  const both = visibleOverviewScopes(scopes, {
    operatorScopeDenied: true,
    mobScopeDenied: true,
  });
  assert.deepEqual(both.map((scope) => scope.scope_kind), ["identity"]);
});

test("overview rows sort identities first, then mob/operator/realm", () => {
  const sorted = sortOverviewScopes([
    overviewScope({ scope_kind: "realm", scope_key: "" }),
    overviewScope({ scope_kind: "operator", scope_key: "op-1" }),
    overviewScope({ scope_kind: "identity", scope_key: "router" }),
    overviewScope({ scope_kind: "mob", scope_key: "research" }),
    overviewScope({ scope_kind: "identity", scope_key: "delivery" }),
  ]);
  assert.deepEqual(
    sorted.map((scope) => overviewScopeLabel(scope)),
    ["delivery", "router", "Mob: research", "Operator: op-1", "Realm"],
  );
});

test("store floor verdict is OK with no pressure, PRESSURE otherwise", () => {
  const calm = storeFloorVerdict([
    overviewScope({ scope_kind: "identity", scope_key: "router" }),
    overviewScope({ scope_kind: "realm", scope_key: "" }),
  ]);
  assert.equal(calm.status, "ok");
  assert.deepEqual(calm.pressured, []);

  const pressured = storeFloorVerdict([
    overviewScope({ scope_kind: "identity", scope_key: "router", floor_pressure: true }),
    overviewScope({ scope_kind: "realm", scope_key: "" }),
  ]);
  assert.equal(pressured.status, "pressure");
  assert.equal(pressured.pressured.length, 1);
  assert.equal(pressured.pressured[0].scope_key, "router");
});

test("store-floor tile flips from unverifiable to a data-driven verdict", () => {
  const tileById = (tiles: ReturnType<typeof computeVerdictTiles>) =>
    new Map(tiles.map((tile) => [tile.id, tile]));

  // No overview yet → the phase-1 boot state, naming the surface.
  const booted = tileById(computeVerdictTiles(emptyVerdictInputs)).get("store-floor");
  assert.equal(booted?.status, "unverifiable");

  // Denied → no-grant, never green.
  const denied = tileById(
    computeVerdictTiles({ ...emptyVerdictInputs, overviewDenied: true }),
  ).get("store-floor");
  assert.equal(denied?.status, "no-grant");

  // Calm store → holding with the OK verdict line and the floors echoed.
  const calm = tileById(
    computeVerdictTiles({
      ...emptyVerdictInputs,
      overview: {
        scopes: [overviewScope({ scope_kind: "identity", scope_key: "router" })],
        floors: { records: 4000, bytes: 32 * 1024 * 1024 },
      },
    }),
  ).get("store-floor");
  assert.equal(calm?.status, "holding");
  assert.match(calm?.lines[0] || "", /^OK — no scope at floor pressure/);
  assert.match(calm?.lines[1] || "", /4000 records \/ 32\.0MB/);

  // Pressure → degraded, naming the pressured scopes.
  const pressure = tileById(
    computeVerdictTiles({
      ...emptyVerdictInputs,
      overview: {
        scopes: [
          overviewScope({ scope_kind: "identity", scope_key: "router", floor_pressure: true }),
          overviewScope({ scope_kind: "mob", scope_key: "research" }),
        ],
        floors: { records: 4000, bytes: 32 * 1024 * 1024 },
      },
    }),
  ).get("store-floor");
  assert.equal(pressure?.status, "degraded");
  assert.match(pressure?.lines[0] || "", /^PRESSURE — 1 scope at floor/);
  assert.match(pressure?.lines[1] || "", /router/);
});

// ── Phase-2: injection ledger DUP annotation ───────────────────────────────

function ledgerEntry(
  identity: string,
  recordId: string,
  atMs: number,
  overrides: Partial<MemoryLedgerEntry> = {},
): MemoryLedgerEntry {
  return {
    realm: "default",
    record_id: recordId,
    identity,
    surface: "turn",
    at_ms: atMs,
    ...overrides,
  };
}

test("consecutive duplicate injections per identity carry the DUP flag", () => {
  // Newest-first, as panel/injections serves them.
  const annotated = annotateInjectionDups([
    ledgerEntry("ada", "rec-1", 400), // dup: ada's previous row was also rec-1
    ledgerEntry("bob", "rec-1", 300), // NOT a dup — different identity
    ledgerEntry("ada", "rec-1", 200),
    ledgerEntry("ada", "rec-2", 100),
  ]);
  assert.deepEqual(
    annotated.map(({ dup }) => dup),
    [true, false, false, false],
  );
  // Original (newest-first) order is preserved.
  assert.deepEqual(
    annotated.map(({ entry }) => entry.at_ms),
    [400, 300, 200, 100],
  );
});

test("non-consecutive repeats are not DUPs; interleaved identities keep separate lanes", () => {
  const annotated = annotateInjectionDups([
    ledgerEntry("ada", "rec-1", 500), // ada's previous was rec-2 → not a dup
    ledgerEntry("bob", "rec-9", 400), // dup: bob's previous was rec-9
    ledgerEntry("ada", "rec-2", 300),
    ledgerEntry("bob", "rec-9", 200),
    ledgerEntry("ada", "rec-1", 100),
  ]);
  assert.deepEqual(
    annotated.map(({ dup }) => dup),
    [false, true, false, false, false],
  );
  assert.deepEqual(annotateInjectionDups([]), []);
});

// ── Phase-2: durable dream verdict sheets ──────────────────────────────────

function sheet(
  overrides: Partial<MemoryDreamRunSheet> & Pick<MemoryDreamRunSheet, "run_id">,
): MemoryDreamRunSheet {
  return { realm: "default", ...overrides };
}

test("dream run sheets sort newest-first by completion (started as fallback)", () => {
  const sorted = dreamRunsNewestFirst([
    sheet({ run_id: "old", completed_at_ms: 100 }),
    sheet({ run_id: "new", completed_at_ms: 300 }),
    sheet({ run_id: "started-only", started_at_ms: 200 }),
  ]);
  assert.deepEqual(
    sorted.map((run) => run.run_id),
    ["new", "started-only", "old"],
  );
});

test("dream run duration formats ms/s/m buckets and degrades to a dash", () => {
  assert.equal(formatDurationMs(412), "412ms");
  assert.equal(formatDurationMs(3200), "3.2s");
  assert.equal(formatDurationMs(4 * 60 * 1000 + 5 * 1000), "4m 5s");
  assert.equal(formatDurationMs(-1), "—");
  assert.equal(
    dreamRunDuration({ started_at_ms: 1_000, completed_at_ms: 4_200 }),
    "3.2s",
  );
  assert.equal(dreamRunDuration({ started_at_ms: 1_000 }), "—");
  assert.equal(dreamRunDuration({}), "—");
});

test("dream run detail normalizes phases in order, non-zero verdicts, and skips", () => {
  const detail = normalizeDreamRunDetail({
    phases: [
      ["orient", "ok"],
      ["gather", "31 candidates"],
      ["prune", ""],
    ],
    verdicts: {
      proposals_accepted: 3,
      proposals_rejected: 0,
      quarantine_release_blocked: 1,
      usage_dead_weight: 0,
    },
    skips: ["group g-2 failed"],
  });
  // Phase order is the steward's execution order — never re-sorted.
  assert.deepEqual(detail.phases, [
    ["orient", "ok"],
    ["gather", "31 candidates"],
    ["prune", ""],
  ]);
  // Zero counters drop; declaration order is preserved.
  assert.deepEqual(detail.verdicts, [
    ["proposals_accepted", 3],
    ["quarantine_release_blocked", 1],
  ]);
  assert.deepEqual(detail.skips, ["group g-2 failed"]);
  assert.equal(detail.raw, null);
});

test("dream run detail degrades to raw text when the stored JSON did not parse", () => {
  const raw = normalizeDreamRunDetail("not-json");
  assert.deepEqual(raw.phases, []);
  assert.deepEqual(raw.verdicts, []);
  assert.equal(raw.raw, "not-json");

  const missing = normalizeDreamRunDetail(undefined);
  assert.equal(missing.raw, null);
  assert.deepEqual(missing.phases, []);

  // Malformed phase tuples are tolerated row by row.
  const partial = normalizeDreamRunDetail({ phases: [["solo"]] as never });
  assert.deepEqual(partial.phases, [["solo", ""]]);
});

test("a non-denial query error keeps the prior page instead of clobbering it", async () => {
  const state: {
    paged: { records: MemoryPanelRecord[]; nextCursor: string | null; denied?: boolean } | null;
    loading: boolean;
  } = { paged: null, loading: false };
  let mode: "ok" | "fail" = "ok";
  const pager = createMemoryRecordsPager({
    query: async () => {
      if (mode === "fail") throw new Error("gateway hiccup");
      return {
        records: [record({ id: "ok-1", scope: { scope: "realm", realm: "default" } })],
        next_cursor: "cursor-2",
        realms: ["default"],
      };
    },
    setPaged: (paged) => {
      state.paged = paged;
    },
    setLoading: (loading) => {
      state.loading = loading;
    },
  });

  await pager.applyFilter({ status: "active" });
  assert.deepEqual(state.paged?.records.map((r) => r.id), ["ok-1"]);

  mode = "fail";
  await pager.applyFilter({ status: "quarantined" });
  // The failed re-query neither cleared the page nor marked it denied.
  assert.deepEqual(state.paged?.records.map((r) => r.id), ["ok-1"]);
  assert.notEqual(state.paged?.denied, true);
  assert.equal(state.loading, false);
});

function quarantinedDetail(overrides: Partial<MemoryFullRecord> = {}): MemoryFullRecord {
  return {
    id: "mem-q",
    scope: { scope: "identity", realm: "default", identity: "lead:main" },
    kind: "preference",
    title: "Reading preference",
    body: "Prefers slow literary fantasy.",
    trust: "agent_observed",
    status: { status: "quarantined", reason: "session tainted by configured untrusted tool" },
    content_hash: "c0ffee",
    ...overrides,
  } as MemoryFullRecord;
}

function receipt(memoryId: string, status: MemoryReviewedRecordReceipt["status"]): MemoryReviewedRecordReceipt {
  return {
    memory_id: memoryId,
    scope: { scope: "identity", realm: "default", identity: "lead:main" },
    kind: "preference",
    status,
    trust: "agent_observed",
    ever_quarantined: true,
    content_hash: "c0ffee",
    derived_from: [],
    created_at_ms: 1,
    updated_at_ms: 2,
  };
}

test("the quarantine decision is offered only for hash-bound identity quarantine", () => {
  assert.equal(canDecideQuarantinedRecord(quarantinedDetail()), true);
  assert.equal(
    canDecideQuarantinedRecord(quarantinedDetail({ status: { status: "active" } })),
    false,
    "active records are not awaiting review",
  );
  assert.equal(
    canDecideQuarantinedRecord(
      quarantinedDetail({ scope: { scope: "mob", realm: "default", mob: "reading" } }),
    ),
    false,
    "mob-scope quarantine stays with the steward and gating",
  );
  assert.equal(
    canDecideQuarantinedRecord(quarantinedDetail({ content_hash: undefined })),
    false,
    "an older gateway without content_hash cannot bind a decision",
  );
});

test("quarantine decision summaries name the successor and replays", () => {
  const released: MemoryQuarantineDecideResult = {
    outcome: "released",
    realm: "default",
    origin: receipt("mem-q", { status: "tombstoned" }),
    successor: receipt("mem-q-released", { status: "active" }),
    decision: {
      audit_token: "review-1",
      decided_at_ms: 3,
      review: {
        verdict: "release",
        reviewer: { kind: "operator", principal: "operator@example.test" },
        origin: "mem-q",
        successor: "mem-q-released",
        expected_content_hash: "c0ffee",
        origin_quarantine_reason: "session tainted by configured untrusted tool",
      },
    },
  };
  assert.match(quarantineDecisionSummary(released), /Released as mem-q-released/);
  assert.match(
    quarantineDecisionSummary({ ...released, outcome: "already_released" }),
    /Already released as mem-q-released; nothing changed/,
  );
  assert.match(
    quarantineDecisionSummary({ ...released, outcome: "tombstoned", successor: null }),
    /Tombstoned/,
  );
  assert.match(
    quarantineDecisionSummary({
      ...released,
      outcome: "tombstoned",
      successor: null,
      decision: {
        ...released.decision,
        review: { ...released.decision.review, verdict: "tombstone", invalidated_promotions: ["gate-1"] },
      },
    }),
    /Pending promotion gate-1 invalidated; it can no longer publish/,
  );
  assert.match(
    quarantineDecisionSummary({ ...released, outcome: "already_tombstoned", successor: null }),
    /Already tombstoned/,
  );
});

test("quarantine rationale is limited in UTF-8 bytes, as the server counts it", () => {
  assert.equal(quarantineRationaleProblem(""), null);
  assert.equal(quarantineRationaleProblem("a".repeat(400)), null);
  assert.match(quarantineRationaleProblem("a".repeat(401)) ?? "", /401 bytes/);
  // Two-byte characters: 201 of them are far fewer than 400 UTF-16 units
  // (what an input maxLength counts) but over the server's 400 bytes.
  assert.equal(quarantineRationaleProblem("\u00e9".repeat(200)), null);
  assert.match(quarantineRationaleProblem("\u00e9".repeat(201)) ?? "", /402 bytes/);
  // Astral characters: four bytes, two UTF-16 units each.
  assert.equal(quarantineRationaleProblem("\u{1f600}".repeat(100)), null);
  assert.match(quarantineRationaleProblem("\u{1f600}".repeat(101)) ?? "", /404 bytes/);
  // Measured as sent: surrounding whitespace is trimmed first.
  assert.equal(quarantineRationaleProblem(`  ${"a".repeat(400)}\n`), null);
});

test("quarantine decision errors render typed refusals without echoing content", () => {
  const refused = (data: Record<string, unknown>) =>
    Object.assign(new Error("quarantine review refused"), {
      rpcError: {
        code: -32043,
        data: { kind: "memory_quarantine_review_refused", ...data },
      },
    });
  assert.match(
    quarantineDecisionErrorText(refused({ reason: "gate_pending", pending_id: "gate-1" })),
    /gate-1 is waiting on it; decide it in the Gating inbox/,
  );
  assert.match(
    quarantineDecisionErrorText(
      refused({ reason: "gate_pending", pending_id: "gate-1", expires_at_ms: 0 }),
    ),
    /or wait until it expires, 1970-01-01T00:00:00.000Z\)\. Tombstoning the record instead invalidates that promotion/,
  );
  assert.match(
    quarantineDecisionErrorText(refused({ reason: "successor_conflict", successor_id: "mem-q-released" })),
    /already holds the release id mem-q-released; nothing was changed/,
  );
  assert.match(
    quarantineDecisionErrorText(refused({ reason: "secret_detected", class: "aws-access-key-id" })),
    /aws-access-key-id secret pattern; tombstone is the only exit/,
  );
  assert.match(
    quarantineDecisionErrorText(refused({ reason: "content_mismatch" })),
    /Reload it and review again/,
  );
  assert.match(
    quarantineDecisionErrorText(
      refused({ reason: "not_quarantined", status: "tombstoned", released_as: "mem-q-released" }),
    ),
    /already released as mem-q-released/,
  );
  assert.match(
    quarantineDecisionErrorText(refused({ reason: "not_quarantined", status: "active" })),
    /the record is active, not quarantined/,
  );
  assert.match(
    quarantineDecisionErrorText(refused({ reason: "stale_update", prior: "mem-p", prior_status: "tombstoned" })),
    /\(mem-p\) is tombstoned/,
  );
  assert.match(
    quarantineDecisionErrorText(
      Object.assign(new Error("access denied: agent.memory.write"), { rpcError: { code: -32030 } }),
    ),
    /No grant/,
  );
  assert.equal(quarantineDecisionErrorText(new Error("network down")), "network down");
});
