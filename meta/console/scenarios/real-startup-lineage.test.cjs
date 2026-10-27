"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { performance } = require("node:perf_hooks");
const http = require("node:http");
const { once } = require("node:events");
const { assertStartupLineage, assertStartupRendering, waitForStartupLineage, observeStartupPhase, startupLineageReaders, source } = require("./real-startup-lineage.cjs");

function runFrames(runId, interactionId) {
  const identity = { run_id: runId, ...(interactionId ? { interaction_id: interactionId } : {}) };
  const frame = (suffix, kind, payload, history = false) => ({
    id: `${runId}:${suffix}`, kind, run_id: runId, interaction_id: interactionId,
    runtime_key: "default", identity: "router:main", session_id: "session",
    source: { kind: history ? "session_history" : "console_event" },
    ...(!history ? { source_event_id: `${runId}:${suffix}` } : {}), payload: structuredClone(payload),
  });
  return [
    frame("start", "run_started", { identity }),
    frame("delta", "text_delta", { delta: source, identity }),
    frame("history", "interaction_complete", { result: source, message: { identity } }, true),
    frame("complete", "interaction_complete", { result: source, identity }),
  ];
}

test("startup lineage oracle requires exact live/history runtime owners for every equal reply", () => {
  const frames = [...runFrames("run-a"), ...runFrames("run-b", "interaction-b")];
  assert.equal(assertStartupLineage(frames).length, 2);
  const badHistory = structuredClone(frames);
  badHistory[2].payload.message.identity.run_id = "run-b";
  assert.throws(() => assertStartupLineage(badHistory), /persisted owner/);
  const missingRun = structuredClone(frames);
  delete missingRun[3].run_id;
  assert.throws(() => assertStartupLineage(missingRun), /canonical run/);
  assert.throws(() => assertStartupLineage(frames.filter(frame => frame.id !== "run-a:delta")), /exact source deltas/);
});

test("startup durability requires actual committed owner messages even when timeline counterparts are pruned", () => {
  const frames = [...runFrames("run-a"), ...runFrames("run-b", "interaction-b")].filter(frame => frame.source.kind !== "session_history");
  const messages = [
    { role: "block_assistant", identity: { run_id: "run-a" }, blocks: [{ block_type: "text", data: { text: source } }] },
    { role: "block_assistant", identity: { run_id: "run-b", interaction_id: "interaction-b" }, blocks: [{ block_type: "text", data: { text: source } }] },
  ];
  const page = { session_id: "session", offset: 0, has_more: false, message_count: 2, messages };
  assert.deepEqual(assertStartupLineage(frames, [page]).map(owner => [owner.runId, owner.historyOffset]), [["run-a", 0], ["run-b", 1]]);
  assert.throws(() => assertStartupLineage(frames), /one persisted reply/);
  assert.throws(() => assertStartupLineage(frames, [{ ...page, session_id: "other-session" }]), /actual durable session/);
  assert.throws(() => assertStartupLineage(frames, [{ ...page, has_more: true }]), /complete durable transcript/);
  assert.throws(() => assertStartupLineage(frames, [{ ...page, message_count: 3 }]), /all committed messages/);
  const foreign = structuredClone(page); foreign.messages[1].identity.run_id = "foreign-run";
  assert.throws(() => assertStartupLineage(frames, [foreign]), /exactly one actual committed reply/);
  const changed = structuredClone(page); changed.messages[1].blocks[0].data.text += " |";
  assert.throws(() => assertStartupLineage(frames, [changed]), /exactly one actual committed reply/);
  const wrongInteraction = structuredClone(page); wrongInteraction.messages[1].identity.interaction_id = "foreign-interaction";
  assert.throws(() => assertStartupLineage(frames, [wrongInteraction]), /actual persisted interaction owner/);
});

test("startup rendering oracle rejects partial duplicates, missing equal-run replies, and reused owners", () => {
  const owners = assertStartupLineage([...runFrames("run-a"), ...runFrames("run-b", "interaction-b")]);
  const rendered = { rowIds: ["row-a", "row-b"], quotes: [
    { id: "run-a:delta", source }, { id: "run-b:delta", source },
  ], tables: 2 };
  assert.deepEqual(assertStartupRendering(rendered, owners).sort(), ["run-a", "run-b"]);
  assert.throws(() => assertStartupRendering({ ...rendered, quotes: rendered.quotes.slice(0, 1) }, owners), /one complete/);
  assert.throws(() => assertStartupRendering({ ...rendered, quotes: [...rendered.quotes, { id: "tail", source: " |\n" }] }, owners), /partial/);
  assert.throws(() => assertStartupRendering({ ...rendered, quotes: [rendered.quotes[0], { ...rendered.quotes[0], id: "run-a:history" }] }, owners), /one rendered owner/);
  assert.throws(() => assertStartupRendering({ ...rendered, tables: 1 }, owners), /complete Markdown table/);
});

function historyPage(runId = "run-a") {
  return { session_id: "session", offset: 0, has_more: false, message_count: 1, messages: [
    { role: "block_assistant", identity: { run_id: runId }, blocks: [{ block_type: "text", data: { text: source } }] },
  ] };
}

function reconnectFixture() {
  const messages = ["run-a", "run-b"].map((runId, index) => ({
    role: "block_assistant", assistant_message_id: `assistant-${index}`,
    identity: { run_id: runId, interaction_id: `interaction-${index}` },
    blocks: [{ block_type: "text", data: { text: source } }],
  }));
  const frames = messages.flatMap((message, index) => runFrames(message.identity.run_id, message.identity.interaction_id).map(frame => ({
    ...frame, payload: { ...frame.payload,
      ...(frame.kind !== "run_started" ? { assistant_message_id: message.assistant_message_id } : {}),
      ...(frame.source.kind === "session_history" ? { message: structuredClone(message) } : {}),
    },
  })));
  const history = [{ session_id: "session", offset: 0, has_more: false, message_count: 2, messages }];
  const priorFrames = frames.filter(frame => frame.id !== "run-b:history");
  const owners = assertStartupLineage(priorFrames, history);
  const rendered = { rowIds: ["stable-row-a", "stable-row-b"], tables: 2,
    quotes: [{ id: "run-a:history", source }, { id: "run-b:history", source }] };
  return { frames, priorFrames, history, owners, rendered };
}

function phaseResult(fixture) {
  return {
    startupAccepted: { interaction_id: "interaction-1" },
    accepted: { interaction_id: "interaction-1" },
    initialOwners: fixture.owners.slice(0, 1), repeatedOwners: fixture.owners,
    rendered: { earlier: { rowIds: ["earlier-row"] } },
  };
}

test("startup phase callers jointly refresh late canonical sources, history and DOM in every phase", async t => {
  for (const phase of ["initial", "repeated", "reconnected", "reloaded"]) {
    await t.test(phase, async () => {
      const fixture = reconnectFixture();
      const expectedOwners = structuredClone(fixture.owners);
      const result = phaseResult(fixture);
      const retainedEarlier = result.rendered.earlier;
      let reads = 0;
      const readOrder = [];
      const signals = [];
      const observation = await observeStartupPhase(phase, {
        result, pollIntervalMs: 1,
        timeline: async ({ signal }) => {
          signals.push(signal); readOrder.push("timeline");
          return { frames: ++reads === 1 ? fixture.priorFrames : fixture.frames };
        },
        durableHistory: async (frames, { signal }) => {
          signals.push(signal); readOrder.push("history");
          assert.equal(frames, reads === 1 ? fixture.priorFrames : fixture.frames);
          assert.equal(result[`${phase}Frames`], frames);
          assert.equal(result[`${phase}History`], undefined, "new frames cannot retain an older history observation");
          assert.equal(result[`${phase}Rendered`], undefined, "new frames cannot retain an older DOM observation");
          return fixture.history;
        },
        readRendered: async ({ signal }) => {
          signals.push(signal); readOrder.push("rendered");
          assert.equal(result[`${phase}History`], fixture.history);
          return fixture.rendered;
        },
      });
      assert.equal(reads, 2, `${phase} must refresh its authoritative sources when the rendered carrier arrives later`);
      assert.deepEqual(readOrder, ["timeline", "history", "rendered", "timeline", "history", "rendered"]);
      assert.equal(new Set(signals).size, 1, "every phase uses one deadline and cancellation signal for all reads");
      assert.equal(signals[0].aborted, true, "success cancels the phase deadline");
      assert.equal(observation.frames, fixture.frames);
      assert.equal(result[`${phase}Frames`], fixture.frames);
      assert.equal(result[`${phase}History`], fixture.history);
      assert.equal(result[`${phase}Rendered`], fixture.rendered);
      assert.equal(result[`${phase}Owners`], observation.owners);
      assert.equal(result.rendered[phase], fixture.rendered);
      assert.equal(result.rendered.earlier, retainedEarlier, "another phase keeps its own evidence");
      assert.deepEqual(observation.owners.map(owner => [owner.runtimeKey, owner.identity, owner.sessionId,
        owner.runId, owner.interactionId, owner.assistantMessageId, owner.historyOffset]), [
        ["default", "router:main", "session", "run-a", "interaction-0", "assistant-0", 0],
        ["default", "router:main", "session", "run-b", "interaction-1", "assistant-1", 1],
      ]);
      assert.deepEqual(fixture.owners, expectedOwners, "refresh does not mutate the previously committed owner snapshot");
      assert.equal(observation.owners[1].historyId, "run-b:history");
    });
  }
});

test("startup phase callers reject missing, foreign and reused rendered source IDs in every phase", async t => {
  for (const phase of ["initial", "repeated", "reconnected", "reloaded"]) {
    await t.test(phase, async t => {
      const timeoutMs = 20_000;
      let now = 0;
      t.mock.method(performance, "now", () => now);
      for (const id of [undefined, "foreign-canonical-frame", "run-a:history"]) {
        now = 0;
        const fixture = reconnectFixture();
        const rendered = structuredClone(fixture.rendered);
        rendered.quotes[1].id = id;
        const result = phaseResult(fixture);
        let timelineReads = 0;
        let renderedReads = 0;
        await assert.rejects(observeStartupPhase(phase, {
          result, timeoutMs, pollIntervalMs: 1,
          timeline: async () => {
            // Expire before a new observation can clear the rejected DOM evidence.
            if (++timelineReads === 2) now = timeoutMs;
            return { frames: fixture.frames };
          },
          durableHistory: async () => fixture.history,
          readRendered: async () => { renderedReads++; return rendered; },
        }), /Timed out: .*exact canonical run|Timed out: .*one rendered owner/);
        assert.equal(timelineReads, 2, "the deadline follows one complete rejected observation");
        assert.equal(renderedReads, 1, "the invalid DOM reached the rendering assertion");
        assert.equal(result[`${phase}Rendered`], rendered, "rejected DOM stays available as failure evidence");
        assert.equal(result.rendered[phase], undefined, "rejected output cannot become a successful phase observation");
      }
    });
  }
});

test("startup initial and repeated phase callers require their actual accepted interaction", async t => {
  for (const phase of ["initial", "repeated"]) {
    await t.test(phase, async () => {
      const fixture = reconnectFixture();
      const result = phaseResult(fixture);
      result[phase === "initial" ? "startupAccepted" : "accepted"].interaction_id = "missing-accepted-interaction";
      let renderedReads = 0;
      await assert.rejects(observeStartupPhase(phase, {
        result, timeoutMs: 25, pollIntervalMs: 1,
        timeline: async () => ({ frames: fixture.frames }), durableHistory: async () => fixture.history,
        readRendered: async () => { renderedReads++; return fixture.rendered; },
      }), /accepted startup interaction completed with exact durable lineage: missing-accepted-interaction/);
      assert.equal(renderedReads, 0, "an unrelated completed reply cannot advance to the rendering check");
    });
  }
});

test("startup repeated phase caller still requires exactly one additional canonical run", async () => {
  const fixture = reconnectFixture();
  const result = phaseResult(fixture);
  result.initialOwners = fixture.owners;
  await assert.rejects(observeStartupPhase("repeated", {
    result, timeline: async () => ({ frames: fixture.frames }),
    durableHistory: async () => fixture.history, readRendered: async () => fixture.rendered,
  }), /same words belong to one additional canonical run/);
  assert.equal(result.rendered.repeated, undefined);
});

test("startup reconnect and reload phase callers preserve every typed owner and transcript position", async t => {
  for (const phase of ["reconnected", "reloaded"]) {
    await t.test(phase, async () => {
      for (const changed of ["runtimeKey", "identity", "sessionId", "runId", "interactionId", "assistantMessageId", "historyOffset"]) {
        const fixture = reconnectFixture();
        const result = phaseResult(fixture);
        result.repeatedOwners = structuredClone(fixture.owners);
        result.repeatedOwners[1][changed] = changed === "historyOffset" ? 4 : `foreign-${changed}`;
        await assert.rejects(observeStartupPhase(phase, {
          result, timeoutMs: 25, pollIntervalMs: 1,
          timeline: async () => ({ frames: fixture.frames }), durableHistory: async () => fixture.history,
          readRendered: async () => fixture.rendered,
        }), /same durable startup owners and transcript positions after refresh/);
        assert.equal(result[`${phase}Owners`], undefined, "a changed owner cannot be published as a completed phase");
      }
    });
  }
});

test("startup phase callers cancel a hanging DOM read under the same bounded deadline", { timeout: 2000 }, async t => {
  for (const phase of ["initial", "repeated", "reconnected", "reloaded"]) {
    await t.test(phase, async () => {
      const fixture = reconnectFixture();
      const result = phaseResult(fixture);
      let reads = 0;
      const signals = [];
      let release;
      const stalled = new Promise(resolve => { release = resolve; });
      const pending = observeStartupPhase(phase, {
        result, timeoutMs: 25, pollIntervalMs: 1,
        timeline: async ({ signal }) => {
          signals.push(signal);
          reads++;
          return { frames: fixture.frames };
        },
        durableHistory: async (_, { signal }) => { signals.push(signal); return fixture.history; },
        readRendered: async ({ signal }) => {
          signals.push(signal);
          return stalled;
        },
      });
      let guard;
      try {
        const outcome = await Promise.race([
          pending.then(value => ({ value }), error => ({ error })),
          new Promise(resolve => { guard = setTimeout(() => resolve({ overdue: true }), 250); }),
        ]);
        assert(!outcome.overdue, "the phase must settle while the DOM read remains pending");
        assert.match(outcome.error?.message ?? "", /Timed out: .*startup Acceptance reply/);
        assert.equal(reads, 1);
        assert.equal(new Set(signals).size, 1, "timeline, history and DOM share the same deadline signal");
        assert.equal(signals[0].aborted, true, "the pending DOM read receives cancellation");
        assert.equal(result[`${phase}Frames`], fixture.frames);
        assert.equal(result[`${phase}History`], fixture.history);
        assert.equal(result[`${phase}Rendered`], undefined, "a pending DOM read cannot publish rendered evidence");
        assert.equal(result.rendered[phase], undefined);
      } finally {
        clearTimeout(guard);
        const retained = structuredClone(result);
        release(fixture.rendered);
        await pending.catch(() => {});
        await new Promise(resolve => setImmediate(resolve));
        assert.deepEqual(result, retained, "late DOM completion cannot alter a failed phase's evidence");
      }
    });
  }
});

test("startup reconnect refreshes canonical source IDs without changing durable owners", async () => {
  const fixture = reconnectFixture();
  assert.throws(() => assertStartupRendering(fixture.rendered, fixture.owners), /exact canonical run/);
  let reads = 0;
  const observations = [];
  const refreshed = await waitForStartupLineage({
    expectedOwners: fixture.owners, pollIntervalMs: 1,
    timeline: async () => ({ frames: ++reads === 1 ? fixture.priorFrames : fixture.frames }),
    durableHistory: async () => fixture.history,
    readRendered: async () => fixture.rendered,
    onObservation: value => observations.push(value),
  });
  assert.equal(reads, 2, "the current rendered canonical carrier must exist in a fresh authoritative observation");
  assert.deepEqual(refreshed.frames, fixture.frames);
  assert.deepEqual(refreshed.history, fixture.history);
  assert.deepEqual(refreshed.rendered, fixture.rendered);
  assert.deepEqual(assertStartupRendering(refreshed.rendered, refreshed.owners), ["run-a", "run-b"]);
  assert.equal(fixture.owners[1].historyId, undefined, "the older owner snapshot stays unchanged");
  assert.equal(refreshed.owners[1].historyId, "run-b:history");
  assert.deepEqual(refreshed.owners.map(owner => [owner.runtimeKey, owner.identity, owner.sessionId, owner.runId,
    owner.interactionId, owner.assistantMessageId, owner.historyOffset]), [
    ["default", "router:main", "session", "run-a", "interaction-0", "assistant-0", 0],
    ["default", "router:main", "session", "run-b", "interaction-1", "assistant-1", 1],
  ]);
  assert(observations.some(value => value.rendered === fixture.rendered), "the actual rendered observation is retained");
});

test("startup reconnect rejects unknown or reused rendered source owners", async () => {
  for (const sourceId of ["unknown-canonical-frame", "run-a:history"]) {
    const fixture = reconnectFixture();
    const rendered = structuredClone(fixture.rendered);
    rendered.quotes[1].id = sourceId;
    await assert.rejects(waitForStartupLineage({
      expectedOwners: fixture.owners, timeoutMs: 25, pollIntervalMs: 1,
      timeline: async () => ({ frames: fixture.frames }), durableHistory: async () => fixture.history,
      readRendered: async () => rendered,
    }), /exact canonical run|one rendered owner/);
  }
});

test("startup reconnect rejects changed durable scope, occurrence, or transcript position", async () => {
  for (const changed of ["runtimeKey", "identity", "sessionId", "runId", "interactionId", "assistantMessageId", "historyOffset"]) {
    const fixture = reconnectFixture();
    const expected = structuredClone(fixture.owners);
    expected[1][changed] = changed === "historyOffset" ? 4 : `other-${changed}`;
    await assert.rejects(waitForStartupLineage({
      expectedOwners: expected, timeoutMs: 25, pollIntervalMs: 1,
      timeline: async () => ({ frames: fixture.frames }), durableHistory: async () => fixture.history,
      readRendered: async () => fixture.rendered,
    }), /same durable startup owners/);
  }
});

test("startup reconnect rejects a canonical carrier with another assistant message identity", async () => {
  const fixture = reconnectFixture();
  fixture.frames.find(frame => frame.id === "run-b:history").payload.assistant_message_id = "foreign-assistant";
  await assert.rejects(waitForStartupLineage({
    expectedOwners: fixture.owners, timeoutMs: 25, pollIntervalMs: 1,
    timeline: async () => ({ frames: fixture.frames }), durableHistory: async () => fixture.history,
    readRendered: async () => fixture.rendered,
  }), /persisted assistant message owner/);
});

test("startup readiness waits past initial Ready and incomplete durable history", async () => {
  const full = runFrames("run-a");
  const ready = full.map(frame => ({ ...frame, payload: { ...frame.payload, result: "Ready.", delta: "Ready." } }));
  let attempt = 0;
  const observations = [];
  const result = await waitForStartupLineage({
    pollIntervalMs: 1,
    timeline: async () => ({ frames: ++attempt === 1 ? ready : full }),
    durableHistory: async () => [attempt < 3 ? { ...historyPage(), message_count: 0, messages: [] } : historyPage()],
    onObservation: value => observations.push(value),
  });
  assert.equal(attempt, 3, "neither member readiness nor a live terminal alone proves durable lineage");
  assert.deepEqual(result.frames, full);
  assert.deepEqual(result.history, [historyPage()]);
  assert.deepEqual(result.owners.map(owner => owner.runId), ["run-a"]);
  assert(observations.some(value => value.frames === ready), "unsatisfied observation is retained for failure evidence");
});

test("startup readiness keeps deadline failure when no expected terminal ever arrives", async () => {
  const ready = runFrames("ready").map(frame => ({ ...frame, payload: { ...frame.payload, result: "Ready." } }));
  const observations = [];
  await assert.rejects(waitForStartupLineage({
    timeoutMs: 25, pollIntervalMs: 1,
    timeline: async () => ({ frames: ready }),
    durableHistory: async () => [{ ...historyPage(), message_count: 0, messages: [] }],
    onObservation: value => observations.push(value),
  }), /Timed out: .*actual completed Acceptance reply/);
  assert.deepEqual(observations.at(-1).frames, ready);
});

test("startup readiness never accepts a durable owner mismatch or incomplete source deltas", async () => {
  for (const corrupt of ["owner", "delta"]) {
    const frames = runFrames("run-a");
    if (corrupt === "delta") frames.find(frame => frame.kind === "text_delta").payload.delta = "partial";
    await assert.rejects(waitForStartupLineage({
      timeoutMs: 25, pollIntervalMs: 1,
      timeline: async () => ({ frames }),
      durableHistory: async () => [historyPage(corrupt === "owner" ? "other-run" : "run-a")],
    }), corrupt === "owner" ? /exactly one actual committed reply/ : /exact source deltas/);
  }
});


test("startup production deadline aborts a hanging timeline read", { timeout: 2000 }, async () => {
  const frames = runFrames("run-a");
  let release;
  const read = new Promise(resolve => { release = resolve; });
  let signal;
  let historyReads = 0;
  const observations = [];
  const pending = waitForStartupLineage({
    timeoutMs: 25,
    timeline: async options => { signal = options?.signal; return read; },
    durableHistory: async () => { historyReads++; return [historyPage()]; },
    onObservation: value => observations.push(value),
  });
  let guard;
  try {
    const result = await Promise.race([
      pending.then(value => ({ value }), error => ({ error })),
      new Promise(resolve => { guard = setTimeout(() => resolve({ overdue: true }), 250); }),
    ]);
    assert(!result.overdue, "production helper must settle while the read is still pending");
    assert.match(result.error?.message ?? "", /Timed out: .*startup Acceptance reply/);
    assert.equal(signal?.aborted, true, "the pending read receives cancellation");
    assert.equal(historyReads, 0, "expired timeline work cannot start another read");
  } finally {
    clearTimeout(guard);
    release({ frames });
    await pending.catch(() => {});
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(historyReads, 0, "late timeline resolution cannot start history after failure");
    assert.deepEqual(observations, [], "late resolution cannot overwrite failure evidence");
  }
});

test("startup production deadline rejects valid lineage observed after timer dispatch was delayed", async () => {
  const frames = runFrames("run-a");
  let historyReads = 0;
  await assert.rejects(waitForStartupLineage({
    timeoutMs: 10,
    timeline: async () => {
      // Hold the event loop beyond the budget: a timer alone cannot reject
      // a late successful read before its continuation runs.
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 30);
      return { frames };
    },
    durableHistory: async () => { historyReads++; return [historyPage()]; },
  }), /Timed out: .*startup Acceptance reply/);
  assert.equal(historyReads, 0, "deadline is checked before recording or reading more state");
});


test("startup deadline cancels production HTTP headers and JSON body reads", { timeout: 5000 }, async t => {
  for (const stalled of ["timeline headers", "timeline body", "history headers", "history body"]) {
    await t.test(stalled, async () => {
      const frames = runFrames("run-a");
      const [resource, phase] = stalled.split(" ");
      const resourceOf = url => String(url).includes("/console/timeline?") ? "timeline" : "history";
      let releaseClosed;
      const closed = new Promise(resolve => { releaseClosed = resolve; });
      // The deadline expires once the intended read has started, never on
      // elapsed time: a cold first request on a loaded runner can take longer
      // than any fixed budget to reach the server.
      const expiry = new AbortController();
      let stalledRead = false;
      const server = http.createServer((request, response) => {
        const requested = resourceOf(request.url);
        if (requested === resource) {
          stalledRead = true;
          response.on("close", releaseClosed);
          if (phase === "body") {
            response.writeHead(200, { "content-type": "application/json" });
            response.write("{");
          } else {
            // The client is now waiting for headers that never come.
            expiry.abort();
          }
          return;
        }
        response.writeHead(200, { "content-type": "application/json" });
        response.end(JSON.stringify(requested === "timeline" ? { frames } : historyPage()));
      });
      server.listen(0, "127.0.0.1");
      await once(server, "listening");
      const baseUrl = `http://127.0.0.1:${server.address().port}`;
      // The production readers, with the moment they start reading the
      // stalled JSON body observed.
      const readers = startupLineageReaders({
        baseUrl, backendUrl: baseUrl,
        fetch: async (url, init) => {
          const response = await fetch(url, init);
          if (phase === "body" && resourceOf(url) === resource) {
            const json = response.json.bind(response);
            response.json = () => {
              const reading = json();
              expiry.abort();
              return reading;
            };
          }
          return response;
        },
      });
      try {
        await assert.rejects(waitForStartupLineage({ ...readers, expiry: expiry.signal }),
          /Timed out: .*startup Acceptance reply/);
        assert(stalledRead, "the deadline interrupted the intended actual HTTP read");
        // The cancelled read closes its connection; the test's own timeout
        // bounds this wait.
        await closed;
      } finally {
        server.closeAllConnections();
        await new Promise(resolve => server.close(resolve));
      }
    });
  }
});

test("startup deadline preserves the last assertion while a later read hangs", async () => {
  const ready = runFrames("ready").map(frame => ({ ...frame, payload: { ...frame.payload, result: "Ready." } }));
  const observations = [];
  let reads = 0;
  await assert.rejects(waitForStartupLineage({
    timeoutMs: 25, pollIntervalMs: 1,
    timeline: async ({ signal }) => {
      if (++reads === 1) return { frames: ready };
      return new Promise((_, reject) => signal.addEventListener("abort", () => reject(signal.reason), { once: true }));
    },
    durableHistory: async () => [historyPage()],
    onObservation: value => observations.push(value),
  }), /Timed out: .*actual completed Acceptance reply/);
  assert.deepEqual(observations.at(-1), { frames: ready, history: [historyPage()] });
});

test("startup success uses one signal and cancels its deadline cleanup", async () => {
  const frames = runFrames("run-a");
  let timelineSignal;
  let historySignal;
  const result = await waitForStartupLineage({
    timeline: async ({ signal }) => { timelineSignal = signal; return { frames }; },
    durableHistory: async (_, { signal }) => { historySignal = signal; return [historyPage()]; },
  });
  assert.deepEqual(result.owners.map(owner => owner.runId), ["run-a"]);
  assert.equal(historySignal, timelineSignal);
  assert.equal(timelineSignal.aborted, true, "successful completion also releases pending read resources");
});


test("startup readiness rejects an unrelated completed reply while its accepted interaction is missing", async () => {
  const frames = runFrames("other-run", "other-interaction");
  const page = historyPage("other-run");
  page.messages[0].identity.interaction_id = "other-interaction";
  await assert.rejects(waitForStartupLineage({
    expectedInteractionId: "accepted-interaction", timeoutMs: 25, pollIntervalMs: 1,
    timeline: async () => ({ frames }), durableHistory: async () => [page],
  }), /accepted startup interaction completed with exact durable lineage/);
});


test("startup readiness waits for its accepted durable owner while retaining other canonical replies", async () => {
  const other = runFrames("other-run", "other-interaction");
  const expected = runFrames("accepted-run", "accepted-interaction");
  const otherPage = historyPage("other-run");
  otherPage.messages[0].identity.interaction_id = "other-interaction";
  const expectedPage = historyPage("accepted-run");
  expectedPage.messages[0].identity.interaction_id = "accepted-interaction";
  let reads = 0;
  const result = await waitForStartupLineage({
    expectedInteractionId: "accepted-interaction", pollIntervalMs: 1,
    timeline: async () => ({ frames: ++reads === 1 ? other : [...other, ...expected] }),
    durableHistory: async () => [reads < 3 ? otherPage : {
      ...otherPage, message_count: 2, messages: [...otherPage.messages, ...expectedPage.messages],
    }],
  });
  assert.equal(reads, 3, "unrelated completion and then non-durable expected completion both retry");
  assert.deepEqual(result.owners.map(owner => owner.interactionId), ["other-interaction", "accepted-interaction"]);
});
