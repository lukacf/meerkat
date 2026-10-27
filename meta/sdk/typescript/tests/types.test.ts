/**
 * Tests for every parse function in src/types.ts.
 *
 * Each section verifies:
 *   1. Parsing a valid wire-format (snake_case) object produces correct camelCase fields
 *   2. Missing optional fields get appropriate defaults
 *   3. The output is readonly / immutable where declared
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import type { GatingDecisionResult, ReconcileEdgesReport } from "../dist/index.js";

import {
  MEMBER_STATE_ACTIVE,
  MEMBER_STATE_RETIRING,
  ErrorCategory,
  isResolutionErrorEvent,
  parseStatusResult,
  parseCapabilitiesResult,
  parseReconcileResult,
  parseSpawnResult,
  parseKeepAliveConfig,
  parseEventEnvelope,
  parseSubscribeResult,
  parseSendMessageResult,
  parseRoutingResolution,
  parseDeliveryResult,
  parseDeliveryHistoryResult,
  parseMemoryQueryResult,
  parseAgentMemoryRecord,
  parseAgentMemoryRecallResult,
  parseAgentMemoryForgetResult,
  parseMemoryStoreInfo,
  parseMemoryIndexResult,
  parseCallToolResult,
  parseMemberSnapshot,
  parseRichMemberSnapshot,
  parseRuntimeRouteResult,
  parseGatingEvaluateResult,
  parseGatingDecisionResult,
  parseGatingAuditEntry,
  parseGatingOrigin,
  parseGatingPendingEntry,
  parseRediscoverReport,
  parseReconcileEdgesReport,
  parsePersistedEvent,
  parseErrorEvent,
  parseMobpackToolsCatalogResult,
  parseMobpackSkillsCatalogResult,
  parseMobpackAgentDefinitionsResult,
  parseMobpackTemplatesResult,
  parseMobpackCatalogsResult,
  parseMobpackValidationResult,
  parseMobpackSourceResult,
  parseMobpackExportResult,
  parseMobpackImportResult,
  parseIdentityResolvedToolsResult,
  parseMobpackDraftRow,
  parseMobpackDraftSummary,
  parseMobpackDraftListResult,
  parseMobpackDraftGetResult,
  parseMobpackDraftSaveResult,
  parseMobpackDraftDeleteResult,
  parseMobpackDraftHistoryResult,
  parseMobpackApplyOperationResult,
  parseMobpackDeployCommandResult,
  parseMobpackDeployResult,
  eventQueryToDict,
  parseWorkGraphOwnerKey,
  workGraphOwnerKeyInputToDict,
  parseWorkGraphOwner,
  workGraphOwnerInputToDict,
  parseWorkGraphClaim,
  parseWorkGraphExternalRef,
  parseWorkGraphEvidenceRef,
  workGraphEvidenceInputToDict,
  parseWorkGraphItem,
  parseWorkGraphEdge,
  parseWorkGraphWorkRef,
  parseWorkGraphAttentionBinding,
  parseWorkGraphSnapshotResult,
  parseWorkGraphItemsResult,
  parseWorkGraphGoalResult,
  parseWorkGraphAttentionReassignResult,
  parseWorkGraphEventEntry,
  workGraphFilterOptionsToDict,
  workGraphReadyOptionsToDict,
  workGraphEventsOptionsToDict,
  workGraphAttentionListOptionsToDict,
  workGraphCreateOptionsToDict,
  workGraphUpdateOptionsToDict,
  workGraphClaimOptionsToDict,
  workGraphCloseOptionsToDict,
  workGraphGoalTargetToDict,
  workGraphGoalCreateOptionsToDict,
  workGraphGoalConfirmOptionsToDict,
  workGraphGoalRequestCloseOptionsToDict,
  workGraphAttentionPauseOptionsToDict,
} from "../dist/index.js";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

describe("constants", () => {
  it("MEMBER_STATE_ACTIVE is 'active'", () => {
    assert.equal(MEMBER_STATE_ACTIVE, "active");
  });

  it("MEMBER_STATE_RETIRING is 'retiring'", () => {
    assert.equal(MEMBER_STATE_RETIRING, "retiring");
  });
});

// ---------------------------------------------------------------------------
// parseRichMemberSnapshot / peer connectivity tri-state
// ---------------------------------------------------------------------------

describe("parseRichMemberSnapshot peer_connectivity", () => {
  it("reads counts from the nested 0.7.x known snapshot", () => {
    // Regression: the old flat reader returned all-zeros for the tri-state shape.
    const snapshot = parseRichMemberSnapshot({
      status: "active",
      tokens_used: 5,
      is_final: false,
      peer_connectivity: {
        status: "known",
        snapshot: {
          reachable_peer_count: 3,
          unknown_peer_count: 1,
          unreachable_peers: [{ peer: "p1", reason: "x" }],
        },
      },
    });
    assert.ok(snapshot.peerConnectivity);
    assert.equal(snapshot.peerConnectivity?.status, "known");
    assert.equal(snapshot.peerConnectivity?.reachablePeerCount, 3);
    assert.equal(snapshot.peerConnectivity?.unknownPeerCount, 1);
    assert.equal(snapshot.peerConnectivity?.unreachablePeers.length, 1);
  });

  it("surfaces not_applicable and probe_timed_out distinctly with zero counts", () => {
    const notApplicable = parseRichMemberSnapshot({
      status: "active",
      tokens_used: 0,
      is_final: false,
      peer_connectivity: { status: "not_applicable" },
    });
    assert.equal(notApplicable.peerConnectivity?.status, "not_applicable");
    assert.equal(notApplicable.peerConnectivity?.reachablePeerCount, 0);

    const timedOut = parseRichMemberSnapshot({
      status: "active",
      tokens_used: 0,
      is_final: false,
      peer_connectivity: { status: "probe_timed_out" },
    });
    assert.equal(timedOut.peerConnectivity?.status, "probe_timed_out");
  });

  it("still accepts the legacy flat shape", () => {
    const snapshot = parseRichMemberSnapshot({
      status: "active",
      tokens_used: 0,
      is_final: false,
      peer_connectivity: {
        reachable_peer_count: 2,
        unknown_peer_count: 0,
        unreachable_peers: [],
      },
    });
    assert.equal(snapshot.peerConnectivity?.reachablePeerCount, 2);
    assert.equal(snapshot.peerConnectivity?.status, "known");
  });

  it("parses the meerkat 0.7.29 machine-owned progress projection", () => {
    const snapshot = parseRichMemberSnapshot({
      status: "active",
      tokens_used: 0,
      is_final: false,
      progress: {
        run_state: "run_open",
        in_flight_work: 2,
        last_progress_at_ms: 1752300000000,
        last_progress_event: "execution_advanced",
        health: "healthy",
      },
    });
    assert.ok(snapshot.progress);
    assert.equal(snapshot.progress?.runState, "run_open");
    assert.equal(snapshot.progress?.inFlightWork, 2);
    assert.equal(snapshot.progress?.lastProgressAtMs, 1752300000000);
    assert.equal(snapshot.progress?.lastProgressEvent, "execution_advanced");
    assert.equal(snapshot.progress?.health, "healthy");
  });

  it("leaves progress null on older gateways and tolerates unknown vocabulary", () => {
    const absent = parseRichMemberSnapshot({ status: "active", tokens_used: 0, is_final: false });
    assert.equal(absent.progress, null);

    const openVocab = parseRichMemberSnapshot({
      status: "active",
      tokens_used: 0,
      is_final: false,
      progress: { run_state: "hibernating", health: "quantum" },
    });
    assert.equal(openVocab.progress?.runState, "hibernating");
    assert.equal(openVocab.progress?.health, "quantum");
    assert.equal(openVocab.progress?.inFlightWork, 0);
    assert.equal(openVocab.progress?.lastProgressEvent, "unchanged");
  });
});

describe("parseIdentityResolvedToolsResult", () => {
  it("reads the per-identity resolved tool surface", () => {
    const result = parseIdentityResolvedToolsResult({
      identity: "domain:security",
      session_id: "sid-1",
      tools: ["shell", "send_message"],
    });
    assert.equal(result.identity, "domain:security");
    assert.equal(result.sessionId, "sid-1");
    assert.deepEqual(result.tools, ["shell", "send_message"]);
  });
});

// ---------------------------------------------------------------------------
// parseStatusResult
// ---------------------------------------------------------------------------

describe("parseStatusResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseStatusResult({
      contract_version: "0.5.0",
      running: true,
      loaded_modules: ["mod_a", "mod_b"],
    });
    assert.equal(result.contractVersion, "0.5.0");
    assert.equal(result.running, true);
    assert.deepEqual(result.loadedModules, ["mod_a", "mod_b"]);
  });

  it("defaults missing fields", () => {
    const result = parseStatusResult({});
    assert.equal(result.contractVersion, "");
    assert.equal(result.running, false);
    assert.deepEqual(result.loadedModules, []);
  });

  it("handles non-object input gracefully", () => {
    const result = parseStatusResult(null);
    assert.equal(result.contractVersion, "");
    assert.equal(result.running, false);
    assert.deepEqual(result.loadedModules, []);
  });

  it("produces readonly output", () => {
    const result = parseStatusResult({ running: true });
    // TypeScript enforces readonly at compile time; at runtime we verify
    // the shape is a plain frozen-style object
    assert.equal(typeof result, "object");
    assert.ok(Object.keys(result).length > 0);
  });
});

// ---------------------------------------------------------------------------
// parseCapabilitiesResult
// ---------------------------------------------------------------------------

describe("parseCapabilitiesResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseCapabilitiesResult({
      contract_version: "0.5.0",
      methods: ["status", "spawn"],
      loaded_modules: ["core"],
    });
    assert.equal(result.contractVersion, "0.5.0");
    assert.deepEqual(result.methods, ["status", "spawn"]);
    assert.deepEqual(result.loadedModules, ["core"]);
    assert.equal(result.workgraph, false);
  });

  it("defaults missing fields", () => {
    const result = parseCapabilitiesResult({});
    assert.equal(result.contractVersion, "");
    assert.deepEqual(result.methods, []);
    assert.deepEqual(result.loadedModules, []);
    assert.equal(result.workgraph, false);
  });

  it("parses workgraph: true", () => {
    const result = parseCapabilitiesResult({
      contract_version: "0.7.30",
      methods: ["status"],
      loaded_modules: ["core"],
      workgraph: true,
    });
    assert.equal(result.workgraph, true);
  });
});

// ---------------------------------------------------------------------------
// parseReconcileResult
// ---------------------------------------------------------------------------

describe("parseReconcileResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseReconcileResult({
      accepted: true,
      reconciled_modules: ["mod_x"],
      added: 3,
    });
    assert.equal(result.accepted, true);
    assert.deepEqual(result.reconciledModules, ["mod_x"]);
    assert.equal(result.added, 3);
  });

  it("defaults missing fields", () => {
    const result = parseReconcileResult({});
    assert.equal(result.accepted, false);
    assert.deepEqual(result.reconciledModules, []);
    assert.equal(result.added, 0);
  });
});

// ---------------------------------------------------------------------------
// parseSpawnResult
// ---------------------------------------------------------------------------

describe("parseSpawnResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseSpawnResult({
      accepted: true,
      module_id: "mod-1",
      agent_identity: "mk-42",
      role: "assistant",
    });
    assert.equal(result.accepted, true);
    assert.equal(result.moduleId, "mod-1");
    assert.equal(result.agentIdentity, "mk-42");
    assert.equal(result.role, "assistant");
  });

  it("nullable fields default to null", () => {
    const result = parseSpawnResult({ accepted: false, module_id: "m" });
    assert.equal(result.agentIdentity, null);
    assert.equal(result.role, null);
  });

  it("defaults missing fields", () => {
    const result = parseSpawnResult({});
    assert.equal(result.accepted, false);
    assert.equal(result.moduleId, "");
    assert.equal(result.agentIdentity, null);
    assert.equal(result.role, null);
  });
});

// ---------------------------------------------------------------------------
// parseKeepAliveConfig
// ---------------------------------------------------------------------------

describe("parseKeepAliveConfig", () => {
  it("parses valid wire-format object", () => {
    const result = parseKeepAliveConfig({
      interval_ms: 30000,
      event: "ping",
    });
    assert.equal(result.intervalMs, 30000);
    assert.equal(result.event, "ping");
  });

  it("defaults missing fields", () => {
    const result = parseKeepAliveConfig({});
    assert.equal(result.intervalMs, 0);
    assert.equal(result.event, "");
  });
});

// ---------------------------------------------------------------------------
// parseEventEnvelope
// ---------------------------------------------------------------------------

describe("parseEventEnvelope", () => {
  it("parses valid wire-format object", () => {
    const payload = { type: "text_delta", delta: "hi" };
    const result = parseEventEnvelope({
      event_id: "evt-1",
      source: "agent-1",
      timestamp_ms: 1700000000000,
      event: payload,
    });
    assert.equal(result.eventId, "evt-1");
    assert.equal(result.source, "agent-1");
    assert.equal(result.timestampMs, 1700000000000);
    assert.deepEqual(result.event, payload);
  });

  it("defaults missing fields", () => {
    const result = parseEventEnvelope({});
    assert.equal(result.eventId, "");
    assert.equal(result.source, "");
    assert.equal(result.timestampMs, 0);
    assert.equal(result.event, undefined);
  });
});

// ---------------------------------------------------------------------------
// parseSubscribeResult
// ---------------------------------------------------------------------------

describe("parseSubscribeResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseSubscribeResult({
      scope: "session:s1",
      replay_from_event_id: "evt-0",
      keep_alive: { interval_ms: 5000, event: "ka" },
      keep_alive_comment: "stay alive",
      event_frames: ["frame-1"],
      events: [
        { event_id: "e1", source: "a", timestamp_ms: 100, event: {} },
      ],
    });
    assert.equal(result.scope, "session:s1");
    assert.equal(result.replayFromEventId, "evt-0");
    assert.equal(result.keepAlive.intervalMs, 5000);
    assert.equal(result.keepAlive.event, "ka");
    assert.equal(result.keepAliveComment, "stay alive");
    assert.deepEqual(result.eventFrames, ["frame-1"]);
    assert.equal(result.events.length, 1);
    assert.equal(result.events[0].eventId, "e1");
  });

  it("defaults missing fields", () => {
    const result = parseSubscribeResult({});
    assert.equal(result.scope, "");
    assert.equal(result.replayFromEventId, null);
    assert.equal(result.keepAlive.intervalMs, 0);
    assert.equal(result.keepAliveComment, "");
    assert.deepEqual(result.eventFrames, []);
    assert.deepEqual(result.events, []);
  });

  it("handles null replay_from_event_id", () => {
    const result = parseSubscribeResult({ replay_from_event_id: null });
    assert.equal(result.replayFromEventId, null);
  });
});

// ---------------------------------------------------------------------------
// parseSendMessageResult
// ---------------------------------------------------------------------------

describe("parseSendMessageResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseSendMessageResult({
      accepted: true,
      member_id: "mem-1",
      session_id: "sess-1",
    });
    assert.equal(result.accepted, true);
    assert.equal(result.memberId, "mem-1");
    assert.equal(result.sessionId, "sess-1");
  });

  it("defaults missing fields", () => {
    const result = parseSendMessageResult({});
    assert.equal(result.accepted, false);
    assert.equal(result.memberId, "");
    assert.equal(result.sessionId, "");
  });
});

// ---------------------------------------------------------------------------
// parseRoutingResolution
// ---------------------------------------------------------------------------

describe("parseRoutingResolution", () => {
  it("parses valid wire-format object", () => {
    const route = { channel: "ch1", sink: "s1" };
    const result = parseRoutingResolution({
      recipient: "agent-2",
      route,
    });
    assert.equal(result.recipient, "agent-2");
    assert.deepEqual(result.route, route);
  });

  it("defaults missing fields", () => {
    const result = parseRoutingResolution({});
    assert.equal(result.recipient, "");
    // When route is missing, the fallback uses the whole record
    assert.equal(typeof result.route, "object");
  });
});

// ---------------------------------------------------------------------------
// parseDeliveryResult
// ---------------------------------------------------------------------------

describe("parseDeliveryResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseDeliveryResult({
      delivered: true,
      delivery_id: "del-1",
    });
    assert.equal(result.delivered, true);
    assert.equal(result.deliveryId, "del-1");
  });

  it("defaults missing fields", () => {
    const result = parseDeliveryResult({});
    assert.equal(result.delivered, false);
    assert.equal(result.deliveryId, "");
  });
});

// ---------------------------------------------------------------------------
// parseDeliveryHistoryResult
// ---------------------------------------------------------------------------

describe("parseDeliveryHistoryResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseDeliveryHistoryResult({
      deliveries: [{ id: "d1" }, { id: "d2" }],
    });
    assert.equal(result.deliveries.length, 2);
    assert.deepEqual(result.deliveries[0], { id: "d1" });
  });

  it("defaults missing deliveries to empty array", () => {
    const result = parseDeliveryHistoryResult({});
    assert.deepEqual(result.deliveries, []);
  });

  it("filters out non-object entries", () => {
    const result = parseDeliveryHistoryResult({
      deliveries: [{ id: "d1" }, "bad", 42, null],
    });
    assert.equal(result.deliveries.length, 1);
  });
});

// ---------------------------------------------------------------------------
// parseMemoryQueryResult
// ---------------------------------------------------------------------------

describe("parseMemoryQueryResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseMemoryQueryResult({
      assertions: [{
        assertion_id: "a-1",
        entity: "identity:luka",
        topic: "preferences",
        store: "knowledge_graph",
        fact: "Prefers concise summaries.",
        metadata: { source: "test" },
        indexed_at_ms: 100,
      }],
      conflicts: [{
        entity: "identity:luka",
        topic: "preferences",
        store: "knowledge_graph",
        reason: "stale",
        updated_at_ms: 200,
      }],
    });
    assert.equal(result.assertions.length, 1);
    assert.equal(result.assertions[0].assertionId, "a-1");
    assert.equal(result.assertions[0].fact, "Prefers concise summaries.");
    assert.equal(result.conflicts.length, 1);
    assert.equal(result.conflicts[0].reason, "stale");
    assert.equal(result.results.length, 2);
  });

  it("preserves legacy results when Rust fields are absent", () => {
    const result = parseMemoryQueryResult({
      results: [{ key: "k1", value: "v1" }],
    });
    assert.equal(result.assertions.length, 0);
    assert.equal(result.conflicts.length, 0);
    assert.equal(result.results.length, 1);
    assert.deepEqual(result.results[0], { key: "k1", value: "v1" });
  });

  it("defaults missing fields", () => {
    const result = parseMemoryQueryResult({});
    assert.deepEqual(result.assertions, []);
    assert.deepEqual(result.conflicts, []);
    assert.deepEqual(result.results, []);
  });
});

// ---------------------------------------------------------------------------
// parseAgentMemoryRecord
// ---------------------------------------------------------------------------

describe("parseAgentMemoryRecord", () => {
  it("parses valid wire-format object", () => {
    const result = parseAgentMemoryRecord({
      memory_id: "mem-1",
      title: "School pickup",
      body: "Pickup is before calendar planning.",
      tags: ["calendar", "team"],
      created_at_ms: 10,
      updated_at_ms: 20,
    });
    assert.equal(result.memoryId, "mem-1");
    assert.equal(result.title, "School pickup");
    assert.equal(result.body, "Pickup is before calendar planning.");
    assert.deepEqual(result.tags, ["calendar", "team"]);
    assert.equal(result.createdAtMs, 10);
    assert.equal(result.updatedAtMs, 20);
  });

  it("rejects malformed durable records", () => {
    assert.throws(
      () => parseAgentMemoryRecord({}),
      /agent_memory_record\.memory_id must be a non-empty string/,
    );
    assert.throws(
      () =>
        parseAgentMemoryRecord({
          memory_id: "mem-1",
          title: "Title",
          body: "Body",
          tags: [42],
          created_at_ms: 1,
          updated_at_ms: 1,
        }),
      /agent_memory_record\.tags must be an array of strings/,
    );
  });
});

// ---------------------------------------------------------------------------
// parseAgentMemoryRecallResult
// ---------------------------------------------------------------------------

describe("parseAgentMemoryRecallResult", () => {
  it("parses valid recall envelopes", () => {
    const result = parseAgentMemoryRecallResult({
      records: [{
        memory_id: "mem-1",
        title: "School pickup",
        body: "Pickup is before calendar planning.",
        tags: ["calendar", "team"],
        created_at_ms: 10,
        updated_at_ms: 20,
      }],
    });

    assert.equal(result.records.length, 1);
    assert.equal(result.records[0]!.memoryId, "mem-1");
  });

  it("rejects malformed recall envelopes", () => {
    assert.throws(
      () => parseAgentMemoryRecallResult({}),
      /agent_memory_recall_result\.records must be an array/,
    );
    assert.throws(
      () =>
        parseAgentMemoryRecallResult({
          records: [{ memory_id: "mem-1" }],
        }),
      /agent_memory_record\.title must be a non-empty string/,
    );
  });
});

// ---------------------------------------------------------------------------
// parseAgentMemoryForgetResult
// ---------------------------------------------------------------------------

describe("parseAgentMemoryForgetResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseAgentMemoryForgetResult({
      memory_id: "mem-1",
      deleted: true,
    });

    assert.equal(result.memoryId, "mem-1");
    assert.equal(result.deleted, true);
  });

  it("rejects malformed forget results", () => {
    assert.throws(
      () => parseAgentMemoryForgetResult({}),
      /agent_memory_forget_result\.memory_id must be a non-empty string/,
    );
    assert.throws(
      () =>
        parseAgentMemoryForgetResult({
          memory_id: "mem-1",
          deleted: "yes",
        }),
      /agent_memory_forget_result\.deleted must be a boolean/,
    );
  });
});

// ---------------------------------------------------------------------------
// parseMemoryStoreInfo
// ---------------------------------------------------------------------------

describe("parseMemoryStoreInfo", () => {
  it("parses valid wire-format object", () => {
    const result = parseMemoryStoreInfo({
      store: "long_term",
      record_count: 42,
    });
    assert.equal(result.store, "long_term");
    assert.equal(result.recordCount, 42);
  });

  it("defaults missing fields", () => {
    const result = parseMemoryStoreInfo({});
    assert.equal(result.store, "");
    assert.equal(result.recordCount, 0);
  });
});

// ---------------------------------------------------------------------------
// parseMemoryIndexResult
// ---------------------------------------------------------------------------

describe("parseMemoryIndexResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseMemoryIndexResult({
      entity: "user-1",
      topic: "preferences",
      store: "long_term",
      assertion_id: "a-1",
      conflict_active: false,
    });
    assert.equal(result.entity, "user-1");
    assert.equal(result.topic, "preferences");
    assert.equal(result.store, "long_term");
    assert.equal(result.assertionId, "a-1");
    assert.equal(result.conflictActive, false);
  });

  it("parses conflict_active true", () => {
    const result = parseMemoryIndexResult({
      entity: "e",
      topic: "t",
      store: "s",
      conflict_active: true,
    });
    assert.equal(result.conflictActive, true);
  });

  it("nullable assertionId defaults to null", () => {
    const result = parseMemoryIndexResult({
      entity: "e",
      topic: "t",
      store: "s",
    });
    assert.equal(result.assertionId, null);
  });

  it("defaults missing fields", () => {
    const result = parseMemoryIndexResult({});
    assert.equal(result.entity, "");
    assert.equal(result.topic, "");
    assert.equal(result.store, "");
    assert.equal(result.assertionId, null);
    assert.equal(result.conflictActive, false);
  });
});

// ---------------------------------------------------------------------------
// parseCallToolResult
// ---------------------------------------------------------------------------

describe("parseCallToolResult", () => {
  it("parses valid wire-format object", () => {
    const toolResult = { answer: 42 };
    const result = parseCallToolResult({
      module_id: "mod-1",
      tool: "calculator",
      result: toolResult,
    });
    assert.equal(result.moduleId, "mod-1");
    assert.equal(result.tool, "calculator");
    assert.deepEqual(result.result, toolResult);
  });

  it("defaults missing fields", () => {
    const result = parseCallToolResult({});
    assert.equal(result.moduleId, "");
    assert.equal(result.tool, "");
    assert.equal(result.result, undefined);
  });
});

// ---------------------------------------------------------------------------
// parseMemberSnapshot
// ---------------------------------------------------------------------------

describe("parseMemberSnapshot", () => {
  it("parses valid wire-format object", () => {
    const result = parseMemberSnapshot({
      agent_identity: "mk-1",
      role: "assistant",
      state: "active",
      wired_to: ["mk-2", "mk-3"],
      labels: { role: "lead", tier: "gold" },
    });
    assert.equal(result.agentIdentity, "mk-1");
    assert.equal(result.role, "assistant");
    assert.equal(result.state, "active");
    assert.deepEqual(result.wiredTo, ["mk-2", "mk-3"]);
    assert.deepEqual(result.labels, { role: "lead", tier: "gold" });
  });

  it("defaults missing fields", () => {
    const result = parseMemberSnapshot({});
    assert.equal(result.agentIdentity, "");
    assert.equal(result.role, "");
    assert.equal(result.state, "");
    assert.deepEqual(result.wiredTo, []);
    assert.deepEqual(result.labels, {});
  });

  it("filters non-string label values", () => {
    const result = parseMemberSnapshot({
      labels: { good: "yes", bad: 123, worse: null },
    });
    assert.deepEqual(result.labels, { good: "yes" });
  });
});

// ---------------------------------------------------------------------------
// parseRuntimeRouteResult
// ---------------------------------------------------------------------------

describe("parseRuntimeRouteResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseRuntimeRouteResult({
      route_key: "rk-1",
      recipient: "agent-3",
      channel: "ch-1",
      sink: "http",
      target_module: "mod-routing",
    });
    assert.equal(result.routeKey, "rk-1");
    assert.equal(result.recipient, "agent-3");
    assert.equal(result.channel, "ch-1");
    assert.equal(result.sink, "http");
    assert.equal(result.targetModule, "mod-routing");
  });

  it("nullable channel defaults to null", () => {
    const result = parseRuntimeRouteResult({
      route_key: "rk",
      recipient: "r",
      sink: "s",
      target_module: "tm",
    });
    assert.equal(result.channel, null);
  });

  it("defaults missing fields", () => {
    const result = parseRuntimeRouteResult({});
    assert.equal(result.routeKey, "");
    assert.equal(result.recipient, "");
    assert.equal(result.channel, null);
    assert.equal(result.sink, "");
    assert.equal(result.targetModule, "");
  });
});

// ---------------------------------------------------------------------------
// parseGatingEvaluateResult
// ---------------------------------------------------------------------------

describe("parseGatingEvaluateResult", () => {
  it("parses valid wire-format object", () => {
    const result = parseGatingEvaluateResult({
      action_id: "act-1",
      action: "delete_user",
      actor_id: "actor-1",
      risk_tier: "high",
      outcome: "approved",
      pending_id: "pend-1",
    });
    assert.equal(result.actionId, "act-1");
    assert.equal(result.action, "delete_user");
    assert.equal(result.actorId, "actor-1");
    assert.equal(result.riskTier, "high");
    assert.equal(result.outcome, "approved");
    assert.equal(result.pendingId, "pend-1");
  });

  it("nullable fields default to null", () => {
    const result = parseGatingEvaluateResult({
      action_id: "a",
      action: "b",
      actor_id: "c",
      outcome: "denied",
    });
    assert.equal(result.riskTier, null);
    assert.equal(result.pendingId, null);
  });

  it("defaults missing fields", () => {
    const result = parseGatingEvaluateResult({});
    assert.equal(result.actionId, "");
    assert.equal(result.action, "");
    assert.equal(result.actorId, "");
    assert.equal(result.riskTier, null);
    assert.equal(result.outcome, "");
    assert.equal(result.pendingId, null);
  });
});

// ---------------------------------------------------------------------------
// parseGatingDecisionResult
// ---------------------------------------------------------------------------

describe("parseGatingDecisionResult", () => {
  for (const [decision, outcome, nextPendingId] of [
    ["approve", "allowed", null],
    ["reject", "safe_draft", null],
    ["escalate", "pending_approval", "pend-2"],
  ] as const) {
    it(`preserves the native ${decision} result`, () => {
      const result = parseGatingDecisionResult({
        pending_id: "pend-1",
        action_id: "act-1",
        decision,
        approver_id: "reviewer-1",
        outcome,
        decided_at_ms: 1700000000000,
        reason: "Reviewed action",
        next_pending_id: nextPendingId,
      });
      assert.deepEqual(result, {
        pendingId: "pend-1",
        actionId: "act-1",
        decision,
        approverId: "reviewer-1",
        outcome,
        decidedAtMs: 1700000000000,
        reason: "Reviewed action",
        nextPendingId,
      });
    });
  }

  it("accepts legacy three-field construction and payloads", () => {
    const legacy: GatingDecisionResult = {
      pendingId: "pend-1",
      actionId: "act-1",
      decision: "approve",
    };
    assert.deepEqual(
      parseGatingDecisionResult({
        pending_id: legacy.pendingId,
        action_id: legacy.actionId,
        decision: legacy.decision,
      }),
      {
        ...legacy,
        approverId: null,
        outcome: null,
        decidedAtMs: null,
        reason: null,
        nextPendingId: null,
      },
    );
  });

  it("preserves unknown outcome strings without inferring allowance", () => {
    const result = parseGatingDecisionResult({
      decision: "approve",
      outcome: "future_outcome",
      reason: "",
      decided_at_ms: 0,
    });
    assert.equal(result.outcome, "future_outcome");
    assert.equal(result.reason, "");
    assert.equal(result.decidedAtMs, 0);
  });

  it("keeps null or malformed optional fields unknown", () => {
    for (const value of [null, false, 42, {}, []]) {
      const result = parseGatingDecisionResult({
        approver_id: value,
        outcome: value,
        reason: value,
        next_pending_id: value,
      });
      assert.equal(result.approverId, null);
      assert.equal(result.outcome, null);
      assert.equal(result.reason, null);
      assert.equal(result.nextPendingId, null);
    }
    for (const decided_at_ms of [
      null, false, "123", -1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1,
    ]) {
      assert.equal(parseGatingDecisionResult({ decided_at_ms }).decidedAtMs, null);
    }
  });

  it("defaults missing fields", () => {
    const result = parseGatingDecisionResult({});
    assert.equal(result.pendingId, "");
    assert.equal(result.actionId, "");
    assert.equal(result.decision, "");
    assert.equal(result.approverId, null);
    assert.equal(result.outcome, null);
    assert.equal(result.decidedAtMs, null);
    assert.equal(result.reason, null);
    assert.equal(result.nextPendingId, null);
  });
});

// ---------------------------------------------------------------------------
// parseGatingAuditEntry
// ---------------------------------------------------------------------------

describe("parseGatingAuditEntry", () => {
  it("parses valid wire-format object", () => {
    const result = parseGatingAuditEntry({
      audit_id: "aud-1",
      timestamp_ms: 1700000000000,
      event_type: "evaluate",
      action_id: "act-1",
      actor_id: "actor-1",
      risk_tier: "medium",
      outcome: "approved",
    });
    assert.equal(result.auditId, "aud-1");
    assert.equal(result.timestampMs, 1700000000000);
    assert.equal(result.eventType, "evaluate");
    assert.equal(result.actionId, "act-1");
    assert.equal(result.actorId, "actor-1");
    assert.equal(result.riskTier, "medium");
    assert.equal(result.outcome, "approved");
  });

  it("nullable riskTier defaults to null", () => {
    const result = parseGatingAuditEntry({
      audit_id: "a",
      timestamp_ms: 0,
      event_type: "e",
      action_id: "a",
      actor_id: "a",
      outcome: "o",
    });
    assert.equal(result.riskTier, null);
  });

  it("defaults missing fields", () => {
    const result = parseGatingAuditEntry({});
    assert.equal(result.auditId, "");
    assert.equal(result.timestampMs, 0);
    assert.equal(result.eventType, "");
    assert.equal(result.actionId, "");
    assert.equal(result.actorId, "");
    assert.equal(result.riskTier, null);
    assert.equal(result.outcome, "");
    assert.equal(result.pendingId, null);
    assert.deepEqual(result.detail, {});
  });

  it("preserves the pending id and the owner's detail", () => {
    const detail = {
      approver_id: "human:reviewer",
      decision: "approve",
      next_pending_id: null,
      origin: { identity: "agent:writer" },
    };
    const result = parseGatingAuditEntry({
      audit_id: "aud-2",
      timestamp_ms: 2000,
      event_type: "approval_decided",
      action_id: "act-1",
      pending_id: "pend-1",
      actor_id: "agent:writer",
      risk_tier: "r3",
      outcome: "allowed",
      detail,
    });
    assert.equal(result.pendingId, "pend-1");
    assert.deepEqual(result.detail, detail);
    assert.notEqual(result.detail, detail);
  });

  it("keeps malformed pending ids and details unknown", () => {
    for (const pending_id of [null, "", " ", 7, {}]) {
      for (const detail of [null, "text", [1], 3]) {
        const result = parseGatingAuditEntry({ pending_id, detail });
        assert.equal(result.pendingId, null);
        assert.deepEqual(result.detail, {});
      }
    }
  });
});

// ---------------------------------------------------------------------------
// parseGatingOrigin
// ---------------------------------------------------------------------------

describe("parseGatingOrigin", () => {
  it("preserves the recorded origin", () => {
    assert.deepEqual(
      parseGatingOrigin({
        identity: "person:alice",
        conversation_id: "conv-1",
        interaction_id: "int-1",
      }),
      { identity: "person:alice", conversationId: "conv-1", interactionId: "int-1" },
    );
    assert.deepEqual(parseGatingOrigin({ identity: "person:alice" }), {
      identity: "person:alice",
      conversationId: null,
      interactionId: null,
    });
  });

  it("returns null for an absent or malformed origin", () => {
    for (const value of [
      undefined, null, "person:alice", [], {}, { identity: "" }, { identity: 3 },
      { conversation_id: "conv-1" },
    ]) {
      assert.equal(parseGatingOrigin(value), null);
    }
  });

  it("is carried on pending entries", () => {
    const pending = parseGatingPendingEntry({
      pending_id: "p1", action_id: "a1", action: "publish", actor_id: "agent:writer",
      origin: { identity: "person:alice", conversation_id: "conv-1" },
    });
    assert.deepEqual(pending.origin, {
      identity: "person:alice",
      conversationId: "conv-1",
      interactionId: null,
    });
    assert.equal(parseGatingPendingEntry({ pending_id: "p1" }).origin, null);
  });
});

// ---------------------------------------------------------------------------
// parseGatingPendingEntry
// ---------------------------------------------------------------------------

describe("parseGatingPendingEntry", () => {
  it("preserves the owner's requested approver, notification references and deadline", () => {
    const result = parseGatingPendingEntry({
      pending_id: "p1", action_id: "a1", action: "publish", actor_id: "agent:writer",
      risk_tier: "r3", created_at_ms: 1700000000000, deadline_at_ms: 1700000060000,
      requested_approver: "human:reviewer", approval_recipient: "review-queue",
      approval_channel: "notification", approval_route_id: "route:1",
      approval_delivery_id: "delivery:1", rationale: "Requested publication",
    });
    assert.equal(result.actorId, "agent:writer");
    assert.equal(result.requestedApprover, "human:reviewer");
    assert.equal(result.approvalRecipient, "review-queue");
    assert.equal(result.approvalChannel, "notification");
    assert.equal(result.approvalRouteId, "route:1");
    assert.equal(result.approvalDeliveryId, "delivery:1");
    assert.equal(result.deadlineAtMs, 1700000060000);
    assert.equal(result.rationale, "Requested publication");
  });

  it("keeps absent or malformed optional routing facts unknown", () => {
    for (const raw of [{}, {
      requested_approver: 7, approval_recipient: {}, approval_channel: " ",
      approval_route_id: false, approval_delivery_id: [], deadline_at_ms: "60000", rationale: false,
    }]) {
      const result = parseGatingPendingEntry(raw);
      assert.equal(result.requestedApprover, null);
      assert.equal(result.approvalRecipient, null);
      assert.equal(result.approvalChannel, null);
      assert.equal(result.approvalRouteId, null);
      assert.equal(result.approvalDeliveryId, null);
      assert.equal(result.deadlineAtMs, null);
      assert.equal(result.rationale, null);
    }
    for (const deadline_at_ms of [-1, NaN, Infinity, true, 1.5]) {
      assert.equal(parseGatingPendingEntry({ deadline_at_ms }).deadlineAtMs, null);
    }
  });

  it("parses valid wire-format object", () => {
    const result = parseGatingPendingEntry({
      pending_id: "pend-1",
      action_id: "act-1",
      action: "transfer_funds",
      actor_id: "actor-1",
      risk_tier: "critical",
      created_at_ms: 1700000000000,
    });
    assert.equal(result.pendingId, "pend-1");
    assert.equal(result.actionId, "act-1");
    assert.equal(result.action, "transfer_funds");
    assert.equal(result.actorId, "actor-1");
    assert.equal(result.riskTier, "critical");
    assert.equal(result.createdAtMs, 1700000000000);
  });

  it("nullable riskTier defaults to null", () => {
    const result = parseGatingPendingEntry({
      pending_id: "p",
      action_id: "a",
      action: "x",
      actor_id: "a",
      created_at_ms: 0,
    });
    assert.equal(result.riskTier, null);
  });

  it("defaults missing fields", () => {
    const result = parseGatingPendingEntry({});
    assert.equal(result.pendingId, "");
    assert.equal(result.actionId, "");
    assert.equal(result.action, "");
    assert.equal(result.actorId, "");
    assert.equal(result.riskTier, null);
    assert.equal(result.createdAtMs, 0);
  });
});

// ---------------------------------------------------------------------------
// parseReconcileEdgesReport
// ---------------------------------------------------------------------------

describe("parseReconcileEdgesReport", () => {
  it("parses valid wire-format object", () => {
    const result = parseReconcileEdgesReport({
      desired_edges: [{ from: "a", to: "b" }],
      wired_edges: [{ from: "a", to: "b" }],
      unwired_edges: [],
      retained_edges: [{ from: "c", to: "d" }],
      preexisting_edges: [],
      skipped_missing_members: [],
      pruned_stale_managed_edges: [],
      failures: [],
    });
    assert.equal(result.desiredEdges.length, 1);
    assert.equal(result.wiredEdges.length, 1);
    assert.deepEqual(result.unwiredEdges, []);
    assert.equal(result.retainedEdges.length, 1);
    assert.deepEqual(result.preexistingEdges, []);
    assert.deepEqual(result.skippedMissingMembers, []);
    assert.deepEqual(result.prunedStaleManagedEdges, []);
    assert.deepEqual(result.failures, []);
  });

  it("isComplete is true when no failures and no skipped", () => {
    const result = parseReconcileEdgesReport({
      failures: [],
      skipped_missing_members: [],
    });
    assert.equal(result.isComplete, true);
  });

  it("isComplete is false when failures present", () => {
    const result = parseReconcileEdgesReport({
      failures: [{ reason: "timeout" }],
      skipped_missing_members: [],
    });
    assert.equal(result.isComplete, false);
  });

  it("isComplete is false when skipped_missing_members present", () => {
    const result = parseReconcileEdgesReport({
      failures: [],
      skipped_missing_members: [{ member: "mk-1" }],
    });
    assert.equal(result.isComplete, false);
  });

  it("isComplete is false when both failures and skipped present", () => {
    const result = parseReconcileEdgesReport({
      failures: [{ reason: "err" }],
      skipped_missing_members: [{ member: "mk-1" }],
    });
    assert.equal(result.isComplete, false);
  });

  it("flags a candidate's dry run as not applied", () => {
    const result = parseReconcileEdgesReport({
      wired_edges: [{ a: "x", b: "y" }],
      applied: false,
    });
    assert.equal(result.applied, false);
    assert.equal(result.wiredEdges.length, 1);
    assert.equal(parseReconcileEdgesReport({}).applied, true);
  });

  it("lets a caller build a report without applied, which reads as applied", () => {
    const built: ReconcileEdgesReport = {
      desiredEdges: [],
      wiredEdges: [],
      unwiredEdges: [],
      retainedEdges: [],
      preexistingEdges: [],
      skippedMissingMembers: [],
      prunedStaleManagedEdges: [],
      failures: [],
      isComplete: true,
    };
    assert.equal(built.applied ?? true, true);
  });

  it("defaults missing fields to empty arrays", () => {
    const result = parseReconcileEdgesReport({});
    assert.deepEqual(result.desiredEdges, []);
    assert.deepEqual(result.wiredEdges, []);
    assert.deepEqual(result.unwiredEdges, []);
    assert.deepEqual(result.retainedEdges, []);
    assert.deepEqual(result.preexistingEdges, []);
    assert.deepEqual(result.skippedMissingMembers, []);
    assert.deepEqual(result.prunedStaleManagedEdges, []);
    assert.deepEqual(result.failures, []);
    assert.equal(result.isComplete, true);
  });
});

// ---------------------------------------------------------------------------
// parseRediscoverReport
// ---------------------------------------------------------------------------

describe("parseRediscoverReport", () => {
  it("parses valid wire-format object", () => {
    const result = parseRediscoverReport({
      spawned: ["mk-1", "mk-2"],
      edges: {
        desired_edges: [{ from: "a", to: "b" }],
        wired_edges: [],
        unwired_edges: [],
        retained_edges: [],
        preexisting_edges: [],
        skipped_missing_members: [],
        pruned_stale_managed_edges: [],
        failures: [],
      },
    });
    assert.deepEqual(result.spawned, ["mk-1", "mk-2"]);
    assert.equal(result.edges.desiredEdges.length, 1);
    assert.equal(result.edges.isComplete, true);
  });

  it("defaults missing fields", () => {
    const result = parseRediscoverReport({});
    assert.deepEqual(result.spawned, []);
    assert.deepEqual(result.edges.desiredEdges, []);
    assert.equal(result.edges.isComplete, true);
  });
});

// ---------------------------------------------------------------------------
// parsePersistedEvent — Agent variant
// ---------------------------------------------------------------------------

describe("parsePersistedEvent", () => {
  it("parses Agent unified event", () => {
    const result = parsePersistedEvent({
      id: "evt-1",
      seq: 5,
      timestamp_ms: 1700000000000,
      member_id: "mem-1",
      event: {
        Agent: { agent_id: "agent-1", event_type: "text_delta" },
      },
    });
    assert.equal(result.id, "evt-1");
    assert.equal(result.seq, 5);
    assert.equal(result.timestampMs, 1700000000000);
    assert.equal(result.memberId, "mem-1");
    assert.equal(result.event.kind, "agent");
    if (result.event.kind === "agent") {
      assert.equal(result.event.agentId, "agent-1");
      assert.equal(result.event.eventType, "text_delta");
      assert.equal(result.event.payload, null);
    }
  });

  it("parses Agent unified event payload when present", () => {
    const result = parsePersistedEvent({
      id: "evt-1b",
      seq: 6,
      timestamp_ms: 1700000000002,
      member_id: "mem-1",
      event: {
        Agent: {
          agent_id: "agent-1",
          event_type: "tool_result_received",
          payload: { type: "tool_result_received", tool_call_id: "tool-1", result: "done" },
        },
      },
    });
    assert.equal(result.event.kind, "agent");
    if (result.event.kind === "agent") {
      assert.deepEqual(result.event.payload, {
        type: "tool_result_received",
        tool_call_id: "tool-1",
        result: "done",
      });
    }
  });

  it("parses Rust internally tagged agent events", () => {
    const result = parsePersistedEvent({
      id: "evt-rust-agent",
      seq: 7,
      timestamp_ms: 1700000000003,
      member_id: "mem-1",
      event: {
        kind: "agent",
        agent_id: "agent-1",
        event_type: "run_completed",
        payload: { ok: true },
      },
    });
    assert.equal(result.event.kind, "agent");
    if (result.event.kind === "agent") {
      assert.equal(result.event.agentId, "agent-1");
      assert.equal(result.event.eventType, "run_completed");
      assert.deepEqual(result.event.payload, { ok: true });
    }
  });

  it("parses Module unified event", () => {
    const result = parsePersistedEvent({
      id: "evt-2",
      seq: 6,
      timestamp_ms: 1700000000001,
      member_id: "mem-2",
      event: {
        Module: {
          module: "mod_memory",
          event_type: "store_updated",
          payload: { key: "val" },
        },
      },
    });
    assert.equal(result.event.kind, "module");
    if (result.event.kind === "module") {
      assert.equal(result.event.module, "mod_memory");
      assert.equal(result.event.eventType, "store_updated");
      assert.deepEqual(result.event.payload, { key: "val" });
    }
  });

  it("parses Rust internally tagged module events", () => {
    const result = parsePersistedEvent({
      id: "evt-rust-module",
      seq: 8,
      timestamp_ms: 1700000000004,
      event: {
        kind: "module",
        module: "routing",
        event_type: "route_added",
        payload: { route: "pager" },
      },
    });
    assert.equal(result.event.kind, "module");
    if (result.event.kind === "module") {
      assert.equal(result.event.module, "routing");
      assert.equal(result.event.eventType, "route_added");
      assert.deepEqual(result.event.payload, { route: "pager" });
    }
  });

  it("nullable memberId defaults to null", () => {
    const result = parsePersistedEvent({
      id: "e",
      seq: 0,
      timestamp_ms: 0,
      event: { Agent: { agent_id: "a", event_type: "e" } },
    });
    assert.equal(result.memberId, null);
  });

  it("fallback for unknown event shape", () => {
    const result = parsePersistedEvent({
      id: "e",
      seq: 0,
      timestamp_ms: 0,
      event: { SomethingElse: { data: "x" } },
    });
    // Falls through to module fallback
    assert.equal(result.event.kind, "module");
    if (result.event.kind === "module") {
      assert.equal(result.event.module, "unknown");
      assert.equal(result.event.eventType, "unknown");
    }
  });

  it("fallback for missing event", () => {
    const result = parsePersistedEvent({ id: "e", seq: 0, timestamp_ms: 0 });
    assert.equal(result.event.kind, "module");
    if (result.event.kind === "module") {
      assert.equal(result.event.module, "unknown");
      assert.equal(result.event.eventType, "unknown");
      assert.deepEqual(result.event.payload, {});
    }
  });

  it("defaults missing fields", () => {
    const result = parsePersistedEvent({});
    assert.equal(result.id, "");
    assert.equal(result.seq, 0);
    assert.equal(result.timestampMs, 0);
    assert.equal(result.memberId, null);
  });
});

// ---------------------------------------------------------------------------
// parseErrorEvent — all ErrorCategory variants
// ---------------------------------------------------------------------------

describe("parseErrorEvent", () => {
  it("parses spawn_failure with member_id", () => {
    const result = parseErrorEvent({
      category: "spawn_failure",
      error: "timeout",
      member_id: "mk-1",
    });
    assert.equal(result.category, "spawn_failure");
    assert.equal(result.message, "mk-1: timeout");
    assert.equal(result.context.error, "timeout");
    assert.equal(result.context.member_id, "mk-1");
  });

  it("parses spawn_failure without member_id", () => {
    const result = parseErrorEvent({
      category: "spawn_failure",
      error: "no slots",
    });
    assert.equal(result.message, "no slots");
  });

  it("parses reconcile_incomplete", () => {
    const result = parseErrorEvent({
      category: "reconcile_incomplete",
      failures: 2,
      skipped: 1,
    });
    assert.equal(result.category, "reconcile_incomplete");
    assert.equal(result.message, "2 failures, 1 skipped");
  });

  it("parses checkpoint_failure with session_id", () => {
    const result = parseErrorEvent({
      category: "checkpoint_failure",
      error: "disk full",
      session_id: "sess-1",
    });
    assert.equal(result.category, "checkpoint_failure");
    assert.equal(result.message, "sess-1: disk full");
  });

  it("parses checkpoint_failure without session_id", () => {
    const result = parseErrorEvent({
      category: "checkpoint_failure",
      error: "disk full",
    });
    assert.equal(result.message, "disk full");
  });

  it("parses host_loop_crash with member_id", () => {
    const result = parseErrorEvent({
      category: "host_loop_crash",
      error: "panic",
      member_id: "mk-2",
    });
    assert.equal(result.category, "host_loop_crash");
    assert.equal(result.message, "mk-2: panic");
  });

  it("parses host_loop_crash without member_id", () => {
    const result = parseErrorEvent({
      category: "host_loop_crash",
      error: "panic",
    });
    assert.equal(result.message, "panic");
  });

  it("parses rediscover_failure", () => {
    const result = parseErrorEvent({
      category: "rediscover_failure",
      error: "network unreachable",
    });
    assert.equal(result.category, "rediscover_failure");
    assert.equal(result.message, "network unreachable");
  });

  it("parses identity_materialization_failure", () => {
    const result = parseErrorEvent({
      category: "identity_materialization_failure",
      identity: "initiative:broken",
      initiator: "review:singleton",
      operation: "materialize_reachable_peers",
      error: "bridge create_session: missing skill",
    });
    assert.equal(result.category, "identity_materialization_failure");
    assert.equal(
      result.message,
      "initiative:broken for review:singleton: materialize_reachable_peers: bridge create_session: missing skill",
    );
    assert.equal(result.context.identity, "initiative:broken");
  });

  it("parses unknown category as JSON", () => {
    const result = parseErrorEvent({
      category: "alien_invasion",
      data: "xeno",
    });
    assert.equal(result.category, "alien_invasion");
    // Unknown categories get JSON.stringify of the full input
    const parsed = JSON.parse(result.message);
    assert.equal(parsed.category, "alien_invasion");
    assert.equal(parsed.data, "xeno");
  });

  it("defaults missing category to 'unknown'", () => {
    const result = parseErrorEvent({});
    assert.equal(result.category, "unknown");
  });

  it("context excludes category key", () => {
    const result = parseErrorEvent({
      category: "spawn_failure",
      error: "x",
      extra: "data",
    });
    assert.equal(result.context.category, undefined);
    assert.equal(result.context.error, "x");
    assert.equal(result.context.extra, "data");
  });

  it("parses compaction_persistence_rejected", () => {
    const result = parseErrorEvent({
      category: "compaction_persistence_rejected",
      identity: "review:singleton",
      session_id: "sess-9",
      error: "store rejected summary",
    });
    assert.equal(result.category, "compaction_persistence_rejected");
    assert.equal(
      result.message,
      "review:singleton (sess-9): store rejected summary",
    );
  });

  it("parses actor_loop_stalled", () => {
    const result = parseErrorEvent({
      category: "actor_loop_stalled",
      probe_waited_secs: 30,
      detail: "mob command loop busy",
    });
    assert.equal(result.category, "actor_loop_stalled");
    assert.equal(
      result.message,
      "probe unanswered for 30s: mob command loop busy",
    );
  });

  it("parses event_log_flush_failure", () => {
    const result = parseErrorEvent({
      category: "event_log_flush_failure",
      error: "disk full",
    });
    assert.equal(result.category, "event_log_flush_failure");
    assert.equal(result.message, "disk full");
  });

  it("parses actor_loop_recovered as a resolution", () => {
    const result = parseErrorEvent({
      category: "actor_loop_recovered",
      stall_id: 7,
      stalled_for_secs: 42,
    });
    assert.equal(result.category, "actor_loop_recovered");
    assert.equal(result.message, "stall 7 resolved after 42s");
    // Closes a prior stall; a host must be able to resolve rather than open
    // a second incident.
    assert.equal(isResolutionErrorEvent(result), true);
  });

  it("parses mob_stop_proceeded_without_interrupt without reading as clean", () => {
    const result = parseErrorEvent({
      category: "mob_stop_proceeded_without_interrupt",
      waited_ms: 5000,
      member: "review:singleton",
      error: "runtime attach not ready",
    });
    assert.equal(result.category, "mob_stop_proceeded_without_interrupt");
    assert.equal(
      result.message,
      "teardown proceeded after 5000ms WITHOUT a successful interrupt " +
        "on review:singleton; a turn may have been admitted and may " +
        "still be running: runtime attach not ready",
    );
    assert.equal(isResolutionErrorEvent(result), false);
  });

  it("parses mob_stop_proceeded_without_interrupt without a member", () => {
    const result = parseErrorEvent({
      category: "mob_stop_proceeded_without_interrupt",
      waited_ms: 250,
      error: "runtime attach not ready",
    });
    assert.equal(
      result.message,
      "teardown proceeded after 250ms WITHOUT a successful interrupt; " +
        "a turn may have been admitted and may still be running: " +
        "runtime attach not ready",
    );
  });

  it("parses retired_member_release_failed naming the retired incarnation", () => {
    const result = parseErrorEvent({
      category: "retired_member_release_failed",
      identity: "keeper",
      agent_runtime_id: "rt:keeper:2",
      session_id: "s-1",
      cause: "retire",
      error: "native host composition is unavailable",
    });
    assert.equal(result.category, "retired_member_release_failed");
    assert.equal(
      result.message,
      "keeper (rt:keeper:2) was retired (retire), and the host's " +
        "release failed: native host composition is unavailable",
    );
    assert.equal(result.context.cause, "retire");
    assert.equal(result.context.session_id, "s-1");
    assert.equal(isResolutionErrorEvent(result), false);
  });

  it("only recoveries and resolved escalations are resolutions", () => {
    const resolutions: string[] = [
      ErrorCategory.ACTOR_LOOP_RECOVERED,
      ErrorCategory.MOB_ANNOUNCEMENT_ESCALATION_RESOLVED,
    ];
    for (const category of Object.values(ErrorCategory)) {
      assert.equal(
        isResolutionErrorEvent({ category, message: "", context: {} }),
        resolutions.includes(category),
        category,
      );
    }
  });

  it("mob_announcement_escalated names its event, cause and subject", () => {
    const result = parseErrorEvent({
      category: "mob_announcement_escalated",
      escalation: {
        escalation_id: "esc-1",
        cause: "failure_streak",
        subject: { kind: "source", mob_id: "mob-a", source_id: "mail" },
        since_ms: 10,
        raised_at_ms: 20,
        renotify_count: 1,
      },
      event: "renotified",
      renotify_count: 1,
      cursor: 7,
    });
    assert.equal(result.category, ErrorCategory.MOB_ANNOUNCEMENT_ESCALATED);
    assert.equal(
      result.message,
      "renotified esc-1 failure_streak on source mail of mob mob-a (notified again 1 times, cursor 7)",
    );
    assert.equal(isResolutionErrorEvent(result), false);
  });

  it("mob_announcement_escalation_resolved is a resolution", () => {
    const result = parseErrorEvent({
      category: "mob_announcement_escalation_resolved",
      escalation_id: "esc-2",
      cause: "audience_hold",
      subject: { kind: "member", member: { mob_id: "mob-a", identity: "reader:1" } },
      resolved_at_ms: 30,
      cursor: 8,
    });
    assert.equal(result.message, "esc-2 audience_hold on member reader:1 of mob mob-a (cursor 8)");
    assert.equal(isResolutionErrorEvent(result), true);
    const declared = parseErrorEvent({
      category: "mob_announcement_escalation_resolved",
      escalation_id: "esc-3",
      cause: "declaration_pending",
      subject: { kind: "declaration", record_kind: "role_child", id: "c-1" },
      resolved_at_ms: 40,
      cursor: 9,
    });
    assert.equal(declared.message, "esc-3 declaration_pending on declared role_child c-1 (cursor 9)");
  });

  // The full set is gated against the Rust `ErrorEvent` enum by
  // crates/meerkat-meta/tests/sdk_error_category_parity.rs; this asserts the exact
  // tags so a rename here cannot pass by editing both sides of that gate.
  it("ErrorCategory constants match expected values", () => {
    assert.deepEqual({ ...ErrorCategory }, {
      SPAWN_FAILURE: "spawn_failure",
      RECONCILE_INCOMPLETE: "reconcile_incomplete",
      CHECKPOINT_FAILURE: "checkpoint_failure",
      COMPACTION_PERSISTENCE_REJECTED: "compaction_persistence_rejected",
      ACTOR_LOOP_STALLED: "actor_loop_stalled",
      ACTOR_LOOP_RECOVERED: "actor_loop_recovered",
      ACTOR_LOOP_TERMINATED: "actor_loop_terminated",
      HOST_LOOP_CRASH: "host_loop_crash",
      REDISCOVER_FAILURE: "rediscover_failure",
      EVENT_LOG_FLUSH_FAILURE: "event_log_flush_failure",
      IDENTITY_MATERIALIZATION_FAILURE: "identity_materialization_failure",
      MOB_STOP_PROCEEDED_WITHOUT_INTERRUPT: "mob_stop_proceeded_without_interrupt",
      RETIRED_MEMBER_RELEASE_FAILED: "retired_member_release_failed",
      MOB_ANNOUNCEMENT_ESCALATED: "mob_announcement_escalated",
      MOB_ANNOUNCEMENT_ESCALATION_RESOLVED: "mob_announcement_escalation_resolved",
    });
  });
});

// ---------------------------------------------------------------------------
// eventQueryToDict
// ---------------------------------------------------------------------------

describe("eventQueryToDict", () => {
  it("converts all fields to snake_case", () => {
    const result = eventQueryToDict({
      sinceMs: 1000,
      untilMs: 2000,
      memberId: "mem-1",
      identity: "identity:luka",
      eventTypes: ["text_delta", "run_completed"],
      limit: 50,
      afterSeq: 10,
    });
    assert.equal(result.since_ms, 1000);
    assert.equal(result.until_ms, 2000);
    assert.equal(result.member_id, "mem-1");
    assert.equal(result.identity, "identity:luka");
    assert.deepEqual(result.event_types, ["text_delta", "run_completed"]);
    assert.equal(result.limit, 50);
    assert.equal(result.after_seq, 10);
  });

  it("omits undefined fields", () => {
    const result = eventQueryToDict({});
    assert.deepEqual(result, {});
  });

  it("omits empty eventTypes array", () => {
    const result = eventQueryToDict({ eventTypes: [] });
    assert.equal(result.event_types, undefined);
  });

  it("includes sinceMs alone", () => {
    const result = eventQueryToDict({ sinceMs: 500 });
    assert.deepEqual(result, { since_ms: 500 });
  });

  it("copies eventTypes array (no shared reference)", () => {
    const types = ["a", "b"];
    const result = eventQueryToDict({ eventTypes: types });
    assert.deepEqual(result.event_types, ["a", "b"]);
    assert.notEqual(result.event_types, types);
  });
});

// ---------------------------------------------------------------------------
// Mobpack editor catalogs
// ---------------------------------------------------------------------------

describe("mobpack editor catalog parsers", () => {
  it("parses split Meta catalog payloads", () => {
    const tools = parseMobpackToolsCatalogResult({
      schema_version: "mobpack.editor.v1",
      runtime_backed: false,
      source: "meta/tool-config",
      authoring_provider: { id: "standalone_authoring", runtime_binding: "unbound" },
      runtime_unavailable_reason: "standalone",
      tool_catalog: [{ id: "shell", tag_class: "ops" }],
    });
    assert.equal(tools.schemaVersion, "mobpack.editor.v1");
    assert.equal(tools.runtimeBacked, false);
    assert.equal(tools.authoringProvider.id, "standalone_authoring");
    assert.equal(tools.runtimeUnavailableReason, "standalone");
    assert.deepEqual(tools.toolCatalog, [{ id: "shell", tag_class: "ops" }]);

    const skills = parseMobpackSkillsCatalogResult({
      schema_version: "mobpack.editor.v1",
      runtime_backed: false,
      source: "meta/authoring-skill-realms",
      authoring_provider: { id: "standalone_authoring" },
      skill_realms: [{ id: "meta/authoring", skills: [] }],
    });
    assert.equal(skills.authoringProvider.id, "standalone_authoring");
    assert.deepEqual(skills.skillRealms, [{ id: "meta/authoring", skills: [] }]);

    const definitions = parseMobpackAgentDefinitionsResult({
      schema_version: "mobpack.editor.v1",
      runtime_backed: false,
      source: "meta/authoring-agent-definitions",
      authoring_provider: { id: "standalone_authoring" },
      agent_definitions: [{ id: "authoring:reviewer", role: "reviewer" }],
    });
    assert.equal(definitions.authoringProvider.id, "standalone_authoring");
    assert.deepEqual(definitions.agentDefinitions, [
      { id: "authoring:reviewer", role: "reviewer" },
    ]);

    const templates = parseMobpackTemplatesResult({
      schema_version: "mobpack.editor.v1",
      source: "meta/mobpack-templates",
      authoring_provider: { id: "standalone_authoring" },
      runtime_unavailable_reason: "standalone",
      blank_mobpack: { document: { members: [] } },
      sample_mobpacks: [{ id: "review" }],
      sample_agent_definitions: [{ id: "sample:reviewer" }],
      templates: { blank_mobpack: { document: { members: [] } } },
    });
    assert.equal(templates.authoringProvider.id, "standalone_authoring");
    assert.equal(templates.runtimeUnavailableReason, "standalone");
    assert.deepEqual(templates.blankMobpack, { document: { members: [] } });
    assert.deepEqual(templates.sampleMobpacks, [{ id: "review" }]);
    assert.deepEqual(templates.sampleAgentDefinitions, [{ id: "sample:reviewer" }]);
  });

  it("parses composed Meta catalog snapshots", () => {
    const result = parseMobpackCatalogsResult({
      schema_version: "mobpack.editor.v1",
      runtime_backed: false,
      authoring_provider: { id: "standalone_authoring", runtime_binding: "unbound" },
      runtime_unavailable_reason: "standalone",
      sources: { tools: "meta/tools/catalog" },
      templates: { blank_mobpack: {} },
      tool_catalog: [{ id: "shell" }],
      skill_realms: [{ id: "meta/authoring" }],
      blank_mobpack: { document: {} },
      sample_mobpacks: [{ id: "sample" }],
      agent_definitions: [{ id: "authoring:reviewer" }],
      sample_agent_definitions: [{ id: "sample:reviewer" }],
      models: [{
        id: "gpt-5",
        display_name: "GPT-5",
        provider: "openai",
        tier: "frontier",
        profile: { vision: true },
      }],
      provider_defaults: [{
        provider: "openai",
        default_model_id: "gpt-5",
        models: [],
      }],
    });
    assert.equal(result.authoringProvider.runtime_binding, "unbound");
    assert.equal(result.runtimeUnavailableReason, "standalone");
    assert.equal(result.sources.tools, "meta/tools/catalog");
    assert.deepEqual(result.toolCatalog, [{ id: "shell" }]);
    assert.deepEqual(result.skillRealms, [{ id: "meta/authoring" }]);
    assert.equal(result.models[0].displayName, "GPT-5");
    assert.equal(result.providerDefaults[0].defaultModelId, "gpt-5");
  });
});

// ---------------------------------------------------------------------------
// Mobpack authoring
// ---------------------------------------------------------------------------

describe("mobpack authoring parsers", () => {
  it("parses validation results with diagnostics and display rows", () => {
    const validation = parseMobpackValidationResult({
      ok: false,
      diagnostics: [
        {
          severity: "error",
          code: "missing_member",
          message: "no members defined",
          path: "members",
        },
      ],
      display_rows: [
        {
          kind: "crit",
          glyph: "!",
          head: "invalid mobpack",
          sub: "no members defined",
          meta: "members",
        },
      ],
      mob_id: "demo",
      flow_ids: ["flow_a"],
      validation_source: "meta/mobpacks/validate",
      deploy_command: "rkat mob deploy",
    });
    assert.equal(validation.ok, false);
    assert.equal(validation.diagnostics[0].severity, "error");
    assert.equal(validation.diagnostics[0].code, "missing_member");
    assert.equal(validation.diagnostics[0].path, "members");
    assert.equal(validation.displayRows[0].kind, "crit");
    assert.equal(validation.displayRows[0].glyph, "!");
    assert.equal(validation.mobId, "demo");
    assert.deepEqual(validation.flowIds, ["flow_a"]);
    assert.equal(validation.deployCommand, "rkat mob deploy");
  });

  it("defaults missing validation fields", () => {
    const validation = parseMobpackValidationResult({ ok: true });
    assert.equal(validation.ok, true);
    assert.deepEqual(validation.diagnostics, []);
    assert.deepEqual(validation.displayRows, []);
    assert.equal(validation.mobId, null);
    assert.deepEqual(validation.flowIds, []);
  });

  it("parses source and export payloads", () => {
    const source = parseMobpackSourceResult({
      filename: "demo.mobpack",
      media_type: "application/vnd.meerkat.mobpack",
      mob_toml: "[mob]\n",
      source_files: [
        {
          path: "mob.toml",
          media_type: "text/x-toml",
          size_bytes: 7,
          content_base64: "W21vYl0K",
          sha256: "abc",
          text: "[mob]\n",
        },
      ],
      validation: { ok: true },
      source: "meta/mobpacks/source",
    });
    assert.equal(source.filename, "demo.mobpack");
    assert.equal(source.sourceFiles[0].path, "mob.toml");
    assert.equal(source.sourceFiles[0].sizeBytes, 7);
    assert.equal(source.sourceFiles[0].text, "[mob]\n");
    assert.equal(source.validation.ok, true);

    const exported = parseMobpackExportResult({
      filename: "demo.mobpack",
      media_type: "application/vnd.meerkat.mobpack",
      content_base64: "UEsDBA==",
      mob_toml: "[mob]\n",
      source_files: [],
      validation: { ok: true },
    });
    assert.equal(exported.contentBase64, "UEsDBA==");
    assert.equal(exported.mediaType, "application/vnd.meerkat.mobpack");
    assert.equal(exported.validation.ok, true);
  });

  it("parses import results", () => {
    const imported = parseMobpackImportResult({
      document: { mob_id: "demo" },
      validation: { ok: true },
      source: "meta/mobpacks/import:archive",
      source_label: "demo.mobpack",
      source_media_type: "application/vnd.meerkat.mobpack",
    });
    assert.deepEqual(imported.document, { mob_id: "demo" });
    assert.equal(imported.source, "meta/mobpacks/import:archive");
    assert.equal(imported.sourceLabel, "demo.mobpack");
  });

  it("parses draft rows and registry results", () => {
    const rowPayload = {
      id: "f_demo",
      name: "Demo",
      version: "mobpack.editor.v1",
      stage: "draft",
      trigger: "Meta authoring draft",
      source: "meta/mobpacks/create",
      revision: 3,
      etag: "f_demo:3",
      updated_at_unix_ms: 1700000000000,
      document: { mob_id: "demo" },
      validation: { ok: true },
      can_undo: true,
      can_redo: false,
    };
    const row = parseMobpackDraftRow(rowPayload);
    assert.equal(row.id, "f_demo");
    assert.equal(row.stage, "draft");
    assert.equal(row.revision, 3);
    assert.equal(row.etag, "f_demo:3");
    assert.deepEqual(row.document, { mob_id: "demo" });
    assert.deepEqual(row.validation, { ok: true });
    assert.equal(row.canUndo, true);
    assert.equal(row.canRedo, false);

    const bareRow = parseMobpackDraftRow({ id: "f_old" });
    assert.equal(bareRow.canUndo, null);
    assert.equal(bareRow.canRedo, null);

    const listed = parseMobpackDraftListResult({
      source: "meta/mobpacks/list",
      store_path: "/tmp/drafts.json",
      runtime_backed: true,
      rows: [rowPayload],
    });
    assert.equal(listed.storePath, "/tmp/drafts.json");
    assert.equal(listed.runtimeBacked, true);
    assert.equal(listed.rows[0].id, "f_demo");
    // Draft results list drafts as summaries: no document.
    assert.deepEqual(listed.rows[0], parseMobpackDraftSummary(rowPayload));
    assert.deepEqual(listed.rows[0], {
      id: "f_demo",
      name: "Demo",
      version: "mobpack.editor.v1",
      stage: "draft",
      trigger: "Meta authoring draft",
      source: "meta/mobpacks/create",
      description: "",
      revision: 3,
      etag: "f_demo:3",
      updatedAtUnixMs: 1700000000000,
      canUndo: true,
      canRedo: false,
    });

    const got = parseMobpackDraftGetResult({
      source: "meta/mobpacks/get",
      runtime_backed: true,
      row: rowPayload,
    });
    assert.equal(got.storePath, null);
    assert.equal(got.row.revision, 3);

    const saved = parseMobpackDraftSaveResult({
      source: "meta/mobpacks/save",
      store_path: "/tmp/drafts.json",
      row: rowPayload,
      rows: [rowPayload],
    });
    assert.equal(saved.row.id, "f_demo");
    assert.equal(saved.rows.length, 1);
    assert.equal(saved.durabilityWarning, null);
    const warned = parseMobpackDraftSaveResult({
      source: "meta/mobpacks/save",
      row: rowPayload,
      rows: [],
      durability_warning: "mobpack draft store was replaced, but its directory could not be synced",
    });
    assert.equal(
      warned.durabilityWarning,
      "mobpack draft store was replaced, but its directory could not be synced",
    );

    const deleted = parseMobpackDraftDeleteResult({
      source: "meta/mobpacks/delete",
      store_path: "/tmp/drafts.json",
      id: "f_demo",
      deleted: true,
      rows: [],
    });
    assert.equal(deleted.id, "f_demo");
    assert.equal(deleted.deleted, true);
    assert.deepEqual(deleted.rows, []);
  });

  it("parses draft history results for stepped and blocked steps", () => {
    const rowPayload = {
      id: "f_demo",
      name: "Demo",
      stage: "draft",
      revision: 4,
      etag: "f_demo:4",
      document: { mob_id: "demo" },
      validation: { ok: true },
      can_undo: false,
      can_redo: true,
    };
    const stepped = parseMobpackDraftHistoryResult({
      source: "meta/mobpacks/undo",
      store_path: "/tmp/drafts.json",
      stepped: true,
      row: rowPayload,
      rows: [rowPayload],
    });
    assert.equal(stepped.source, "meta/mobpacks/undo");
    assert.equal(stepped.storePath, "/tmp/drafts.json");
    assert.equal(stepped.stepped, true);
    assert.equal(stepped.reason, null);
    assert.equal(stepped.row.revision, 4);
    assert.equal(stepped.row.canUndo, false);
    assert.equal(stepped.row.canRedo, true);
    assert.equal(stepped.rows[0].id, "f_demo");

    const blocked = parseMobpackDraftHistoryResult({
      source: "meta/mobpacks/redo",
      store_path: "/tmp/drafts.json",
      stepped: false,
      reason: "nothing to redo",
      row: rowPayload,
      rows: [rowPayload],
    });
    assert.equal(blocked.stepped, false);
    assert.equal(blocked.reason, "nothing to redo");
    assert.equal(blocked.row.etag, "f_demo:4");
    assert.equal(blocked.durabilityWarning, null);
    const warned = parseMobpackDraftHistoryResult({
      source: "meta/mobpacks/undo",
      stepped: true,
      row: rowPayload,
      rows: [],
      durability_warning: "not synced",
    });
    assert.equal(warned.durabilityWarning, "not synced");
    assert.equal(
      parseMobpackDraftDeleteResult({ id: "f_demo", deleted: true, durability_warning: "not synced" })
        .durabilityWarning,
      "not synced",
    );
  });

  it("parses apply-operation results with and without selection", () => {
    const applied = parseMobpackApplyOperationResult({
      source: "meta/mobpacks/apply_operation",
      operation: "add_member",
      ok: true,
      document: { mob_id: "demo", members: [{ id: "reviewer" }] },
      selection: { kind: "agent", id: "reviewer" },
      validation: { ok: true },
    });
    assert.equal(applied.operation, "add_member");
    assert.equal(applied.ok, true);
    assert.deepEqual(applied.selection, { kind: "agent", id: "reviewer" });
    assert.equal(applied.validation.ok, true);

    const noSelection = parseMobpackApplyOperationResult({
      source: "meta/mobpacks/apply_operation",
      operation: "delete_member",
      ok: true,
      document: { mob_id: "demo" },
      selection: null,
      validation: { ok: true },
    });
    assert.equal(noSelection.selection, null);
  });

  it("parses deploy command and deploy results", () => {
    const preview = parseMobpackDeployCommandResult({
      command: "rkat mob deploy demo.mobpack",
      argv: ["rkat", "mob", "deploy", "demo.mobpack"],
      deploy_command: "rkat mob deploy",
      filename: "demo.mobpack",
      validation: { ok: true },
      source: "meerkat_meta::mobpack::deploy_argv",
    });
    assert.equal(preview.command, "rkat mob deploy demo.mobpack");
    assert.deepEqual(preview.argv, ["rkat", "mob", "deploy", "demo.mobpack"]);
    assert.equal(preview.deployCommand, "rkat mob deploy");

    const deployed = parseMobpackDeployResult({
      filename: "demo.mobpack",
      pack_path: "/tmp/demo.mobpack",
      pack_sha256: "deadbeef",
      command: "rkat mob deploy /tmp/demo.mobpack",
      argv: ["rkat", "mob", "deploy", "/tmp/demo.mobpack"],
      plan_trace: [{ step: "validate" }],
      executed: true,
      success: true,
      status_code: 0,
      stdout: "deployed",
      validation: { ok: true },
      display_rows: [
        {
          kind: "ok",
          glyph: "✓",
          head: "deploy executed",
          sub: "deployed",
          meta: "/tmp/demo.mobpack",
        },
      ],
    });
    assert.equal(deployed.executed, true);
    assert.equal(deployed.success, true);
    assert.equal(deployed.statusCode, 0);
    assert.equal(deployed.stdout, "deployed");
    assert.equal(deployed.stderr, null);
    assert.deepEqual(deployed.planTrace, [{ step: "validate" }]);
    assert.equal(deployed.displayRows[0].head, "deploy executed");
  });
});

// ---------------------------------------------------------------------------
// WorkGraph
// ---------------------------------------------------------------------------

describe("parseWorkGraphOwnerKey / owner", () => {
  it("parses an owner key", () => {
    const key = parseWorkGraphOwnerKey({ kind: "agent", id: "agent-1" });
    assert.equal(key.kind, "agent");
    assert.equal(key.id, "agent-1");
  });

  it("workGraphOwnerKeyInputToDict passes fields through verbatim", () => {
    assert.deepEqual(
      workGraphOwnerKeyInputToDict({ kind: "agent", id: "agent-1" }),
      { kind: "agent", id: "agent-1" },
    );
  });

  it("parses an owner with display_name", () => {
    const owner = parseWorkGraphOwner({
      key: { kind: "principal", id: "luka" },
      display_name: "Luka",
    });
    assert.equal(owner.key.kind, "principal");
    assert.equal(owner.displayName, "Luka");
  });

  it("defaults missing display_name to null", () => {
    const owner = parseWorkGraphOwner({ key: { kind: "principal", id: "luka" } });
    assert.equal(owner.displayName, null);
  });

  it("workGraphOwnerInputToDict omits displayName when absent", () => {
    const dict = workGraphOwnerInputToDict({ kind: "agent", id: "agent-1" });
    assert.deepEqual(dict, { kind: "agent", id: "agent-1" });
  });

  it("workGraphOwnerInputToDict includes display_name when set", () => {
    const dict = workGraphOwnerInputToDict({
      kind: "agent",
      id: "agent-1",
      displayName: "Agent One",
    });
    assert.deepEqual(dict, {
      kind: "agent",
      id: "agent-1",
      display_name: "Agent One",
    });
  });
});

describe("parseWorkGraphClaim", () => {
  it("parses a claim with a lease", () => {
    const claim = parseWorkGraphClaim({
      owner: { key: { kind: "agent", id: "a1" }, display_name: null },
      claimed_at: "2026-01-01T00:00:00Z",
      lease_expires_at: "2026-01-01T01:00:00Z",
    });
    assert.equal(claim.owner.key.id, "a1");
    assert.equal(claim.claimedAt, "2026-01-01T00:00:00Z");
    assert.equal(claim.leaseExpiresAt, "2026-01-01T01:00:00Z");
  });

  it("leaseExpiresAt defaults to null", () => {
    const claim = parseWorkGraphClaim({
      owner: { key: { kind: "agent", id: "a1" } },
      claimed_at: "2026-01-01T00:00:00Z",
    });
    assert.equal(claim.leaseExpiresAt, null);
  });
});

describe("parseWorkGraphExternalRef / parseWorkGraphEvidenceRef", () => {
  it("parses an external ref", () => {
    const ref = parseWorkGraphExternalRef({
      kind: "jira",
      id: "PROJ-1",
      url: "https://example/PROJ-1",
    });
    assert.equal(ref.kind, "jira");
    assert.equal(ref.url, "https://example/PROJ-1");
  });

  it("external ref url defaults to null", () => {
    const ref = parseWorkGraphExternalRef({ kind: "jira", id: "PROJ-1" });
    assert.equal(ref.url, null);
  });

  it("parses an evidence ref with confirmation fields", () => {
    const ref = parseWorkGraphEvidenceRef({
      kind: "host_confirmation",
      id: "ev-1",
      label: "Host confirmed",
      summary: "Verified via CI",
      confirmation_kind: "host_confirmation",
      confirming_owner_key: { kind: "agent", id: "host-1" },
    });
    assert.equal(ref.confirmationKind, "host_confirmation");
    assert.equal(ref.confirmingOwnerKey?.id, "host-1");
  });

  it("evidence ref optional fields default to null", () => {
    const ref = parseWorkGraphEvidenceRef({ kind: "self_attest", id: "ev-1" });
    assert.equal(ref.label, null);
    assert.equal(ref.summary, null);
    assert.equal(ref.confirmationKind, null);
    assert.equal(ref.confirmingOwnerKey, null);
  });

  it("workGraphEvidenceInputToDict omits optional fields when absent", () => {
    assert.deepEqual(
      workGraphEvidenceInputToDict({ kind: "self_attest", id: "ev-1" }),
      { kind: "self_attest", id: "ev-1" },
    );
  });

  it("workGraphEvidenceInputToDict includes label/summary when set", () => {
    assert.deepEqual(
      workGraphEvidenceInputToDict({
        kind: "self_attest",
        id: "ev-1",
        label: "lbl",
        summary: "sum",
      }),
      { kind: "self_attest", id: "ev-1", label: "lbl", summary: "sum" },
    );
  });
});

describe("parseWorkGraphItem", () => {
  const fullWireItem = {
    id: "work_1",
    realm_id: "realm-1",
    namespace: "default",
    title: "Ship the thing",
    description: "A description",
    status: "in_progress",
    completion_policy: { kind: "self_attest" },
    priority: "high",
    labels: ["backend", "urgent"],
    owner: { key: { kind: "agent", id: "agent-1" }, display_name: "Agent One" },
    claim: {
      owner: { key: { kind: "agent", id: "agent-1" }, display_name: null },
      claimed_at: "2026-01-01T00:00:00Z",
      lease_expires_at: "2026-01-01T01:00:00Z",
    },
    machine_state: { lifecycle_phase: "in_progress", revision: 3 },
    revision: 3,
    due_at: "2026-02-01T00:00:00Z",
    not_before: "2026-01-15T00:00:00Z",
    snoozed_until: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-05T00:00:00Z",
    terminal_at: null,
    external_refs: [{ kind: "jira", id: "PROJ-1", url: "https://x/PROJ-1" }],
    evidence_refs: [{ kind: "self_attest", id: "ev-1" }],
  };

  it("parses a fully populated wire-format WorkItem", () => {
    const item = parseWorkGraphItem(fullWireItem);
    assert.equal(item.id, "work_1");
    assert.equal(item.realmId, "realm-1");
    assert.equal(item.namespace, "default");
    assert.equal(item.title, "Ship the thing");
    assert.equal(item.description, "A description");
    assert.equal(item.status, "in_progress");
    assert.deepEqual(item.completionPolicy, { kind: "self_attest" });
    assert.equal(item.priority, "high");
    assert.deepEqual(item.labels, ["backend", "urgent"]);
    assert.equal(item.owner?.key.id, "agent-1");
    assert.equal(item.claim?.claimedAt, "2026-01-01T00:00:00Z");
    assert.deepEqual(item.machineState, { lifecycle_phase: "in_progress", revision: 3 });
    assert.equal(item.revision, 3);
    assert.equal(item.dueAt, "2026-02-01T00:00:00Z");
    assert.equal(item.notBefore, "2026-01-15T00:00:00Z");
    assert.equal(item.snoozedUntil, null);
    assert.equal(item.createdAt, "2026-01-01T00:00:00Z");
    assert.equal(item.updatedAt, "2026-01-05T00:00:00Z");
    assert.equal(item.terminalAt, null);
    assert.equal(item.externalRefs.length, 1);
    assert.equal(item.externalRefs[0].id, "PROJ-1");
    assert.equal(item.evidenceRefs.length, 1);
    assert.equal(item.evidenceRefs[0].id, "ev-1");
  });

  it("defaults missing/optional fields (skip_serializing_if omissions)", () => {
    const item = parseWorkGraphItem({
      id: "work_2",
      realm_id: "realm-1",
      namespace: "default",
      title: "Minimal item",
      status: "open",
      priority: "medium",
      revision: 0,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    });
    assert.equal(item.description, null);
    assert.deepEqual(item.labels, []);
    assert.equal(item.owner, null);
    assert.equal(item.claim, null);
    assert.equal(item.dueAt, null);
    assert.equal(item.notBefore, null);
    assert.equal(item.snoozedUntil, null);
    assert.equal(item.terminalAt, null);
    assert.deepEqual(item.externalRefs, []);
    assert.deepEqual(item.evidenceRefs, []);
  });

  it("defaults every field on an empty object", () => {
    const item = parseWorkGraphItem({});
    assert.equal(item.id, "");
    assert.equal(item.title, "");
    assert.equal(item.status, "");
    assert.equal(item.priority, "");
    assert.equal(item.revision, 0);
    assert.deepEqual(item.labels, []);
  });
});

describe("parseWorkGraphEdge", () => {
  it("parses a wire-format WorkEdge", () => {
    const edge = parseWorkGraphEdge({
      realm_id: "realm-1",
      namespace: "default",
      kind: "blocks",
      from_id: "work_1",
      to_id: "work_2",
      created_at: "2026-01-01T00:00:00Z",
    });
    assert.equal(edge.kind, "blocks");
    assert.equal(edge.fromId, "work_1");
    assert.equal(edge.toId, "work_2");
  });

  it("defaults missing fields", () => {
    const edge = parseWorkGraphEdge({});
    assert.equal(edge.realmId, "");
    assert.equal(edge.kind, "");
    assert.equal(edge.fromId, "");
    assert.equal(edge.toId, "");
  });
});

describe("parseWorkGraphAttentionBinding", () => {
  it("parses a fully populated binding, passing target/status through opaquely", () => {
    const binding = parseWorkGraphAttentionBinding({
      binding_id: "attention_1",
      work_ref: { item_id: "work_1", realm_id: "realm-1", namespace: "default" },
      target: { kind: "session", session_id: "sess-1" },
      mode: "pursue",
      status: { state: "paused", until: "2026-02-01T00:00:00Z" },
      machine_state: { lifecycle_phase: "active", revision: 2 },
      delegated_authority: "add_evidence",
      projection_policy: { max_text_chars: 4096, include_parent_context: true },
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-02T00:00:00Z",
    });
    assert.equal(binding.bindingId, "attention_1");
    assert.equal(binding.workRef.itemId, "work_1");
    assert.deepEqual(binding.target, { kind: "session", session_id: "sess-1" });
    assert.equal(binding.mode, "pursue");
    assert.deepEqual(binding.status, { state: "paused", until: "2026-02-01T00:00:00Z" });
    assert.deepEqual(binding.machineState, { lifecycle_phase: "active", revision: 2 });
    assert.equal(binding.delegatedAuthority, "add_evidence");
    assert.deepEqual(binding.projectionPolicy, {
      max_text_chars: 4096,
      include_parent_context: true,
    });
  });

  it("defaults missing fields to empty records/strings", () => {
    const binding = parseWorkGraphAttentionBinding({});
    assert.equal(binding.bindingId, "");
    assert.deepEqual(binding.target, {});
    assert.deepEqual(binding.status, {});
    assert.deepEqual(binding.projectionPolicy, {});
  });
});

describe("parseWorkGraphSnapshotResult", () => {
  it("parses a full snapshot", () => {
    const snapshot = parseWorkGraphSnapshotResult({
      realm_id: "realm-1",
      namespace: "default",
      all_namespaces: false,
      captured_at: "2026-01-01T00:00:00Z",
      event_high_water_mark: 42,
      items: [
        {
          id: "work_1",
          realm_id: "realm-1",
          namespace: "default",
          title: "Item one",
          status: "open",
          priority: "medium",
          revision: 1,
          created_at: "2026-01-01T00:00:00Z",
          updated_at: "2026-01-01T00:00:00Z",
        },
      ],
      edges: [
        {
          realm_id: "realm-1",
          namespace: "default",
          kind: "parent",
          from_id: "work_0",
          to_id: "work_1",
          created_at: "2026-01-01T00:00:00Z",
        },
      ],
      attention: [],
      ready_item_ids: ["work_1"],
    });
    assert.equal(snapshot.realmId, "realm-1");
    assert.equal(snapshot.namespace, "default");
    assert.equal(snapshot.allNamespaces, false);
    assert.equal(snapshot.eventHighWaterMark, 42);
    assert.equal(snapshot.items.length, 1);
    assert.equal(snapshot.items[0].id, "work_1");
    assert.equal(snapshot.edges.length, 1);
    assert.deepEqual(snapshot.readyItemIds, ["work_1"]);
  });

  it("defaults missing fields", () => {
    const snapshot = parseWorkGraphSnapshotResult({});
    assert.equal(snapshot.realmId, "");
    assert.equal(snapshot.namespace, null);
    assert.equal(snapshot.allNamespaces, false);
    assert.equal(snapshot.eventHighWaterMark, null);
    assert.deepEqual(snapshot.items, []);
    assert.deepEqual(snapshot.edges, []);
    assert.deepEqual(snapshot.attention, []);
    assert.deepEqual(snapshot.readyItemIds, []);
  });
});

describe("parseWorkGraphItemsResult", () => {
  it("parses a wrapped items list", () => {
    const result = parseWorkGraphItemsResult({
      items: [
        {
          id: "work_1",
          realm_id: "realm-1",
          namespace: "default",
          title: "Item one",
          status: "open",
          priority: "medium",
          revision: 1,
          created_at: "2026-01-01T00:00:00Z",
          updated_at: "2026-01-01T00:00:00Z",
        },
      ],
    });
    assert.equal(result.items.length, 1);
  });

  it("defaults to an empty list", () => {
    assert.deepEqual(parseWorkGraphItemsResult({}).items, []);
  });
});

describe("parseWorkGraphGoalResult / parseWorkGraphAttentionReassignResult", () => {
  const item = {
    id: "work_1",
    realm_id: "realm-1",
    namespace: "default",
    title: "Goal item",
    status: "open",
    priority: "medium",
    revision: 1,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
  const attention = {
    binding_id: "attention_1",
    work_ref: { item_id: "work_1", realm_id: "realm-1", namespace: "default" },
    target: { kind: "session", session_id: "sess-1" },
    mode: "pursue",
    status: { state: "active" },
    delegated_authority: "add_evidence",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };

  it("parses a goal result (item + attention)", () => {
    const result = parseWorkGraphGoalResult({ item, attention });
    assert.equal(result.item.id, "work_1");
    assert.equal(result.attention.bindingId, "attention_1");
  });

  it("parses an attention reassign result (previous + attention)", () => {
    const previous = { ...attention, binding_id: "attention_0" };
    const result = parseWorkGraphAttentionReassignResult({
      previous,
      attention,
    });
    assert.equal(result.previous.bindingId, "attention_0");
    assert.equal(result.attention.bindingId, "attention_1");
  });
});

describe("parseWorkGraphEventEntry", () => {
  it("parses an item-scoped event", () => {
    const event = parseWorkGraphEventEntry({
      seq: 7,
      realm_id: "realm-1",
      namespace: "default",
      item_id: "work_1",
      kind: "claimed",
      at: "2026-01-01T00:00:00Z",
      payload: { owner: "agent-1" },
    });
    assert.equal(event.seq, 7);
    assert.equal(event.itemId, "work_1");
    assert.equal(event.kind, "claimed");
    assert.deepEqual(event.payload, { owner: "agent-1" });
  });

  it("seq and itemId default to null for graph-scoped events", () => {
    const event = parseWorkGraphEventEntry({
      realm_id: "realm-1",
      namespace: "default",
      kind: "attention_created",
      at: "2026-01-01T00:00:00Z",
    });
    assert.equal(event.seq, null);
    assert.equal(event.itemId, null);
  });
});

describe("WorkGraph option toDict helpers", () => {
  it("workGraphFilterOptionsToDict converts and omits empty arrays", () => {
    assert.deepEqual(
      workGraphFilterOptionsToDict({
        namespace: "default",
        allNamespaces: true,
        statuses: ["open", "blocked"],
        labels: ["urgent"],
        includeTerminal: true,
        limit: 10,
      }),
      {
        namespace: "default",
        all_namespaces: true,
        statuses: ["open", "blocked"],
        labels: ["urgent"],
        include_terminal: true,
        limit: 10,
      },
    );
    assert.deepEqual(workGraphFilterOptionsToDict(), {});
    assert.deepEqual(workGraphFilterOptionsToDict({ statuses: [] }), {});
  });

  it("workGraphReadyOptionsToDict / workGraphEventsOptionsToDict / workGraphAttentionListOptionsToDict", () => {
    assert.deepEqual(workGraphReadyOptionsToDict({ namespace: "ns", limit: 5 }), {
      namespace: "ns",
      limit: 5,
    });
    assert.deepEqual(
      workGraphEventsOptionsToDict({ afterSeq: 10, allNamespaces: true }),
      { after_seq: 10, all_namespaces: true },
    );
    assert.deepEqual(
      workGraphAttentionListOptionsToDict({ status: "active" }),
      { status: "active" },
    );
  });

  it("workGraphCreateOptionsToDict converts full option set", () => {
    const dict = workGraphCreateOptionsToDict({
      description: "desc",
      priority: "high",
      completionPolicy: { kind: "self_attest" },
      labels: ["a", "b"],
      dueAt: "2026-02-01T00:00:00Z",
      notBefore: "2026-01-15T00:00:00Z",
      snoozedUntil: "2026-01-20T00:00:00Z",
      externalRefs: [{ kind: "jira", id: "PROJ-1" }],
      evidenceRefs: [{ kind: "self_attest", id: "ev-1" }],
      status: "blocked",
      namespace: "default",
    });
    assert.deepEqual(dict, {
      description: "desc",
      priority: "high",
      completion_policy: { kind: "self_attest" },
      labels: ["a", "b"],
      due_at: "2026-02-01T00:00:00Z",
      not_before: "2026-01-15T00:00:00Z",
      snoozed_until: "2026-01-20T00:00:00Z",
      external_refs: [{ kind: "jira", id: "PROJ-1" }],
      evidence_refs: [{ kind: "self_attest", id: "ev-1" }],
      status: "blocked",
      namespace: "default",
    });
    assert.deepEqual(workGraphCreateOptionsToDict(), {});
  });

  it("workGraphUpdateOptionsToDict forwards an explicit empty labels array (clears labels)", () => {
    const dict = workGraphUpdateOptionsToDict({ labels: [] });
    assert.deepEqual(dict, { labels: [] });
  });

  it("workGraphUpdateOptionsToDict omits labels entirely when unset", () => {
    const dict = workGraphUpdateOptionsToDict({ title: "New title" });
    assert.deepEqual(dict, { title: "New title" });
  });

  it("workGraphClaimOptionsToDict / workGraphCloseOptionsToDict", () => {
    assert.deepEqual(workGraphClaimOptionsToDict({ leaseSeconds: 60 }), {
      lease_seconds: 60,
    });
    assert.deepEqual(workGraphCloseOptionsToDict({ status: "failed" }), {
      status: "failed",
    });
    assert.deepEqual(workGraphCloseOptionsToDict(), {});
  });

  it("workGraphGoalTargetToDict handles all three target kinds", () => {
    assert.deepEqual(
      workGraphGoalTargetToDict({ kind: "session", sessionId: "sess-1" }),
      { kind: "session", session_id: "sess-1" },
    );
    assert.deepEqual(
      workGraphGoalTargetToDict({ kind: "identity", identity: "identity:luka" }),
      { kind: "identity", identity: "identity:luka" },
    );
    assert.deepEqual(
      workGraphGoalTargetToDict({
        kind: "owner",
        ownerKey: { kind: "agent", id: "agent-1" },
      }),
      { kind: "owner", owner_key: { kind: "agent", id: "agent-1" } },
    );
  });

  it("workGraphGoalCreateOptionsToDict / workGraphGoalConfirmOptionsToDict / workGraphGoalRequestCloseOptionsToDict", () => {
    assert.deepEqual(
      workGraphGoalCreateOptionsToDict({ mode: "review", delegatedAuthority: "request_closure" }),
      { mode: "review", delegated_authority: "request_closure" },
    );
    assert.deepEqual(
      workGraphGoalConfirmOptionsToDict({
        evidence: { kind: "self_attest", id: "ev-1" },
      }),
      { evidence: { kind: "self_attest", id: "ev-1" } },
    );
    assert.deepEqual(
      workGraphGoalRequestCloseOptionsToDict({ status: "cancelled" }),
      { status: "cancelled" },
    );
  });

  it("workGraphAttentionPauseOptionsToDict", () => {
    assert.deepEqual(
      workGraphAttentionPauseOptionsToDict({ until: "2026-03-01T00:00:00Z" }),
      { until: "2026-03-01T00:00:00Z" },
    );
    assert.deepEqual(workGraphAttentionPauseOptionsToDict(), {});
  });
});
