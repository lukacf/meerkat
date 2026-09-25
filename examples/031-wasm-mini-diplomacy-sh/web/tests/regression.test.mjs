import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, rm, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { createServer } from "vite";
import { chromium } from "playwright";
import { anthropicReply } from "./provider-fixture.mjs";

test("Diplomacy real modules, typed event ingress and offline current WASM", { timeout: 120_000 }, async t => {
  const repo = resolve("../../.."), work = resolve(".work/regression");
  // The runtime reports its crate version, which is the workspace version.
  const runtimeVersion = (await readFile(resolve(repo, "Cargo.toml"), "utf8"))
    .match(/^\[workspace\.package\]\nversion = "([^"]+)"/m)[1];
  await mkdir(work, { recursive: true });
  process.env.TMPDIR = work;
  const server = await createServer({
    configFile: false, root: process.cwd(),
    server: { host: "127.0.0.1", port: 0, fs: { allow: [repo] } },
  });
  await server.listen();
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage();
    let external = 0;
    await page.route("**/*", route => {
      const url = route.request().url();
      if (!url.startsWith(origin + "/")) { external++; return route.abort(); }
      if (url === origin + "/") return route.fulfill({ contentType: "text/html", body: "<div id='channelGrid'></div>" });
      return route.continue();
    });
    await page.goto(origin);

    await t.test("E05 one owned runner handles Step, rapid controls, serialized replacement and safe boundaries", async () => {
      await page.evaluate(async () => {
        const { CampaignRunner } = await import("/src/runner.ts");
        const check = (ok, why) => { if (!ok) throw Error(why); };
        const barriers = [], turns = [], disposed = [];
        let factories = 0, releaseCreate;
        const runner = new CampaignRunner(
          async session => {
            turns.push(session.id);
            await new Promise(resolve => barriers.push(resolve));
            session.finished++;
          },
          session => session.finished < 10,
          async session => { disposed.push(session.id); },
        );
        const a = { id: "old", finished: 0 };
        const init = runner.replace(async () => { factories++; await new Promise(r => { releaseCreate = r; }); return a; });
        const duplicate = runner.replace(async () => { factories++; throw Error("overlap"); });
        check(init === duplicate, "concurrent startup not coalesced");
        await Promise.resolve(); releaseCreate(); await init;
        check(factories === 1, "duplicate creation");
        const step = runner.step(), secondStep = runner.step();
        check(step === secondStep && turns.length === 1, "Step duplicated work");
        barriers.shift()(); await step;
        check(turns.length === 1 && a.finished === 1 && runner.paused, "Step recursively scheduled");
        const running = runner.resume();
        runner.pause(); runner.resume(); runner.pause();
        check(turns.length === 2 && barriers.length === 1, "rapid controls duplicated submissions");
        const b = { id: "new", finished: 0 };
        const replacement = runner.replace(async () => { factories++; return b; });
        await Promise.resolve();
        check(factories === 1 && disposed.length === 0, "replaced active campaign before safe boundary");
        barriers.shift()(); await running; await replacement;
        check(disposed.join() === "old" && a.finished === 2 && b.finished === 0, "old task mutated new campaign");
        const newStep = runner.step(); barriers.shift()(); await newStep;
        check(turns.join() === "old,old,new" && b.finished === 1, "replacement owned wrong session");
      });
    });

    await t.test("E03/E06 bounded nested flow polling and fresh extraction observation window", async () => {
      await page.evaluate(async () => {
        const { pollNarrative, waitForOrders } = await import("/src/runner.ts");
        const check = (ok, why) => { if (!ok) throw Error(why); };
        let clock = 8000, order = null, polls = 0;
        await waitForOrders(() => {
          polls++;
          if (clock >= 9500) order = "late valid order";
          return order !== null;
        }, () => true, async ms => { clock += ms; }, () => clock);
        check(order === "late valid order" && polls === 5, "extraction used an expired deliberation clock");
        const start = clock;
        await waitForOrders(() => false, () => true, async ms => { clock += ms; }, () => clock);
        check(clock - start >= 20000 && clock - start < 20300, "genuine timeout not bounded");
        const responses = [
          { run: null }, { run: { status: "pending" } }, { run: { status: "running" } },
          { run: { status: "completed", step_ledger: [{ step_id: "summarize", status: "completed", output: { narrative: "Exactly once" } }] } },
        ];
        let rendered = 0;
        const narrative = await pollNarrative(async () => JSON.stringify(responses.shift()), async () => {});
        if (narrative) rendered++;
        check(narrative === "Exactly once" && rendered === 1 && responses.length === 0, "nested flow stopped too early");
        for (const status of ["failed", "canceled"]) {
          let calls = 0;
          check(await pollNarrative(async () => { calls++; return JSON.stringify({ run: { status } }); }, async () => {}) === null && calls === 1, "terminal flow not stopped");
        }
        let calls = 0;
        await pollNarrative(async () => { calls++; return JSON.stringify({ run: { status: "running" } }); }, async () => {});
        check(calls === 40, "narrator deadline missing");
      });
    });

    await t.test("E02/E04/E18/E19/E-NEW-01 current envelopes, typed sources, identity routing and structured carriers", async () => {
      const result = await page.evaluate(async () => {
        const { drainAllEvents, sourceSortKey } = await import("/src/events.ts");
        const { defaultState } = await import("/src/game.ts");
        const { setSessionRef, renderGrid } = await import("/src/ui.ts");
        renderGrid();
        const a = { agentIdentity: "france-operator", handle: "a", role: "operator", team: "france" };
        const b = { agentIdentity: "russia-ambassador", handle: "b", role: "ambassador", team: "russia" };
        const session = {
          factions: [], narratorMobId: null, subs: [a, b], state: defaultState(), messages: [], running: true,
          prevControllers: new Map(), seenEventIds: new Set(), summarizedRuns: new Set(), failures: [],
          peerMembers: new Map([["planner-key", "france-planner"], ["ambassador-key", "france-ambassador"]]),
        };
        setSessionRef(session);
        const env = (event_id, payload, source = { type: "session", session_id: "a" }, seq = 0) => ({ event_id, payload, source, seq, timestamp_ms: 1 });
        const message = body => ({ type: "tool_call_requested", id: "same-provider-call-id", name: "send_message", args: { peer_id: "planner-key", body } });
        const batches = {
          a: [env("c", message("third"), undefined, 3), env("b", message("second"), undefined, 2), env("a", message("first"), undefined, 2)],
          b: [env("d", { ...message("foreign"), args: { peer_id: "ambassador-key", body: "FINAL ORDER: target=paris aggression=90" } }, { type: "session", session_id: "b" })],
        };
        const drain = (data = batches, turn = 1) => drainAllEvents({ poll_subscription: handle => JSON.stringify(data[handle] ?? []) }, session, turn);
        const check = (ok, why) => { if (!ok) throw Error(why); };
        check(drain().events === 4, "valid typed sources rejected");
        check(session.messages.map(m => m.content).join() === "first,second,third,FINAL ORDER: target=paris aggression=90", "ordering not deterministic");
        check(session.messages[3].channel === "f-r-diplo" && session.messages[3].faction === "russia", "canonical cross-mob routing lost subscription attribution");
        check(drain().events === 0 && session.messages.length === 4, "identical event replay not suppressed");
        drain({ a: [env("next-turn", message("reused call ID"))] }, 2);
        check(session.messages.at(-1).turn === 2, "provider ID collision across turns");
        const summary = { headline: "structured headline", dispatches: [{ peer: "planner-key", summary: "canonical summary" }] };
        const output = drain({ a: [
          env("started", { type: "run_started" }),
          env("main", { type: "run_completed", extraction_required: true, result: "not JSON" }),
          env("extracted", { type: "extraction_succeeded", structured_output: summary }),
          env("duplicate-carrier", { type: "run_completed", structured_output: summary }),
          env("next-start", { type: "run_started" }),
          env("inline", { type: "run_completed", structured_output: { headline: "inline output", dispatches: [] } }),
          env("unstructured", { type: "run_completed", result: '{"headline":"must not parse natural language"}' }),
          env("failure", { type: "run_failed", error_report: { class: "llm", message: "real typed diagnostic" } }),
          env("extraction-failure", { type: "extraction_failed", reason: "schema mismatch", attempts: 2 }),
        ].map((e, index) => ({ ...e, seq: 100 + index })) });
        check(session.messages.filter(m => m.headline === "canonical summary").length === 1, "summary duplicated or missing");
        check(session.messages.some(m => m.headline === "inline output"), "inline output lost");
        check(!session.messages.some(m => m.content.includes("must not parse")), "natural result treated as structured authority");
        check(output.errors.join().includes("real typed diagnostic") && output.errors.join().includes("schema mismatch"), "typed errors lost");
        check(session.failures[0].report.class === "llm" && session.failures[0].report.message === "real typed diagnostic", "typed failure report discarded");
        let warnings = 0;
        const warn = console.warn; console.warn = () => { warnings++; };
        const malformed = drain({ a: [env("bad", message("bad"), { type: "session", source_id: "legacy" }), env("bad2", message("bad"), { type: "unknown" })] });
        console.warn = warn;
        check(malformed.events === 0 && warnings === 1, "malformed typed sources not rejected");
        for (const [source, expected] of [
          [{ type: "runtime", runtime_id: "r" }, "runtime:r"], [{ type: "interaction", interaction_id: "i" }, "interaction:i"],
          [{ type: "external", source_id: "e" }, "external:e"], [{ type: "callback" }, "callback"],
          [{ type: "session", session_id: "" }, null], [{ type: "callback", bogus: true }, null],
        ]) check(sourceSortKey(source) === expected, "typed source validation mismatch");
        return { messages: session.messages.length, errors: output.errors };
      });
      assert.match(result.errors.join(), /real typed diagnostic/);
    });

    await t.test("E17 narration uses actual per-order outcomes, including recapture and skipped targets", async () => {
      await page.evaluate(async () => {
        const { resolveOrders } = await import("/src/game.ts");
        const { buildNarratorSummary } = await import("/src/events.ts");
        const state = { turn: 1, max_turns: 10, scores: { france: 0, prussia: 0, russia: 0 }, regions: [{ id: "target", controller: "russia", defense: 40, value: 1 }] };
        const decision = (team, aggression, target_region = "target") => ({ order: { team, aggression, fortify: 100 - aggression, target_region }, reasoning: "" });
        const random = Math.random; Math.random = () => 0;
        try {
          const fixtures = [
            [[decision("france", 0), decision("prussia", 100)], ["repelled", "captured"]],
            [[decision("france", 100), decision("prussia", 100)], ["captured", "captured"]],
            [[decision("france", 100), decision("russia", 100)], ["captured", "captured"]],
            [[decision("russia", 100), decision("france", 100, "missing")], ["skipped", "skipped"]],
          ];
          for (const [orders, expected] of fixtures) {
            const result = resolveOrders(state, orders);
            if (result.outcomes.map(o => o.result).join() !== expected.join()) throw Error("wrong per-order outcome");
            const captures = new Set(result.state.regions.filter(r => r.controller !== state.regions[0].controller).map(r => r.id));
            const summary = buildNarratorSummary({ messages: [] }, 1, result.outcomes, captures, result.state);
            const lines = summary.split("\n").filter(line => line.includes(" attacked "));
            if (lines.some((line, i) => !line.endsWith(expected[i].toUpperCase()))) throw Error("narration inferred outcome from final owner");
          }
        } finally { Math.random = random; }
      });
    });

    await t.test("E02/E03/E05/E06 real UI turn loop stops failures, steps once and replaces resources", async () => {
      for (const mode of ["failure", "extraction-failure", "success"]) {
        const appPage = await browser.newPage();
        await appPage.route("**/*", route => {
          const url = route.request().url();
          if (!url.startsWith(origin + "/")) return route.abort();
          if (url.endsWith("/runtime.js")) return route.fulfill({ contentType: "text/javascript", body: `
            const queues = new Map();
            let seq = 0;
            const facts = globalThis.fixture = { mode: ${JSON.stringify(mode)}, inits: 0, flows: 0, closed: 0, destroyed: 0, sends: [], summaries: [] };
            export default async function() {}
            export function init_runtime_from_config() { facts.inits++; }
            export async function mob_create(def) { return JSON.parse(def).id; }
            export async function mob_spawn() {}
            export async function mob_wire() {}
            export async function mob_wire_peer() {}
            export async function mob_member_peer_target(_mob, member) { return JSON.stringify({ external: { peer_id: "key-" + member, name: "misleading optional display label" } }); }
            export async function mob_member_subscribe(_mob, member) { queues.set(member, []); return member; }
            export function close_subscription(handle) { facts.closed++; queues.delete(handle); }
            export async function mob_lifecycle() { facts.destroyed++; }
            export function destroy_runtime() {}
            export async function mob_member_send(_mob, member, raw) {
              const request = JSON.parse(raw);
              facts.sends.push({ member, request });
              let payload;
              if ((facts.mode === "failure" && member.endsWith("-planner")) || (facts.mode === "extraction-failure" && member.endsWith("-operator"))) {
                payload = { type: "run_failed", error_report: { class: "llm", message: "synthetic turn diagnostic" } };
              } else if (facts.mode === "success" && member.endsWith("-operator")) {
                const team = member.split("-")[0];
                payload = { type: "tool_call_requested", id: "reused-call-id", name: "send_message",
                  args: { peer_id: "key-" + team + "-planner", handling_mode: "queue", body: "FINAL ORDER: target=" + (team === "prussia" ? "paris" : "berlin") + " aggression=100" } };
              }
              if (payload) {
                const enqueue = () => queues.get(member).push({ event_id: "id-" + ++seq, source: { type: "session", session_id: member }, seq, timestamp_ms: Date.now(), payload });
                if (facts.mode === "success") setTimeout(enqueue, 1200); else enqueue();
              }
            }
            export function poll_subscription(handle) { const events = queues.get(handle) || []; queues.set(handle, []); return JSON.stringify(events); }
            export async function mob_run_flow(_mob, _flow, params) { facts.flows++; facts.summaries.push(JSON.parse(params).summary); return "flow-" + facts.flows; }
            export async function mob_flow_status() { return JSON.stringify({ run: { status: "completed", step_ledger: [{ step_id: "summarize", status: "completed", output: { narrative: "One dispatch." } }] } }); }
          ` });
          if (url === origin + "/") return route.fulfill({ contentType: "text/html", body: "<div id='app'></div>" });
          return route.continue();
        });
        await appPage.goto(origin);
        await appPage.clock.install();
        await appPage.evaluate(async () => {
          await import("/src/main.ts");
          document.getElementById("keyAnthropic").value = "synthetic-unused";
          document.getElementById("keyAnthropic").dispatchEvent(new Event("change"));
          document.getElementById("startBtn").click();
          document.getElementById("startBtn").click();
        });
        await appPage.waitForFunction(() => document.getElementById("guideOverlay")?.classList.contains("hidden") === false);
        await appPage.evaluate(() => document.getElementById("guideDismiss").click());
        if (mode === "success") await appPage.evaluate(() => document.getElementById("pauseBtn").click());
        await appPage.clock.runFor(35_000);
        const first = await appPage.evaluate(() => ({
          ...window.fixture, turn: document.getElementById("turnPill").textContent,
          badge: document.getElementById("statusBadge").textContent, status: document.getElementById("statusLine").textContent,
        }));
        assert.equal(first.inits, 1, "overlapping Start reinitialized runtime");
        if (mode !== "success") {
          assert.equal(first.turn, "Turn 1 / 10");
          assert.equal(first.flows, 0, "failed turn resolved fallback combat");
          assert.equal(first.badge, "Error");
          assert.match(first.status, /synthetic turn diagnostic/);
        } else {
          assert.equal(first.turn, "Turn 2 / 10");
          assert.equal(first.flows, 1);
          assert.equal((first.summaries[0].match(/aggression 100/g) ?? []).length, 3, "delayed extraction lost to fallback");
          assert.ok(first.sends.filter(s => s.member.endsWith("-operator")).every(s => s.request.content.includes('handling_mode="queue"') && s.request.content.includes("key-")));
          await appPage.evaluate(() => { document.getElementById("stepBtn").click(); document.getElementById("stepBtn").click(); });
          await appPage.clock.runFor(35_000);
          assert.equal(await appPage.locator("#turnPill").textContent(), "Turn 3 / 10", "Step advanced more than one round");
          assert.equal(await appPage.evaluate(() => window.fixture.flows), 2);
          await appPage.evaluate(() => { document.getElementById("startBtn").click(); document.getElementById("startBtn").click(); });
          await appPage.waitForFunction(() => window.fixture.inits === 2);
          assert.deepEqual(await appPage.evaluate(() => [window.fixture.closed, window.fixture.destroyed]), [9, 4]);
        }
        await appPage.close();
      }
    });

    await t.test("E-NEW-01/E02 real current-runtime member envelopes flow unchanged into the real drain", async () => {
      const result = await page.evaluate(async repoPath => {
        const wasm = await import(`/@fs/${repoPath}/sdks/web/wasm/meerkat_web_runtime.js`);
        const bytes = await (await fetch(`/@fs/${repoPath}/sdks/web/wasm/meerkat_web_runtime_bg.wasm`)).arrayBuffer();
        await wasm.default({ module_or_path: bytes });
        const { buildFactionDefinition, buildNarratorDefinition } = await import("/src/agents.ts");
        const { drainAllEvents } = await import("/src/events.ts");
        const { setSessionRef } = await import("/src/ui.ts");
        const { defaultState } = await import("/src/game.ts");
        const originalFetch = window.fetch;
        let intercepted = 0;
        window.fetch = async () => {
          intercepted++;
          return new Response(JSON.stringify({ type: "error", error: { type: "authentication_error", message: "synthetic offline diagnostic" } }), { status: 401, headers: { "Content-Type": "application/json" } });
        };
        let sub;
        try {
          wasm.init_runtime_from_config(JSON.stringify({ anthropic_api_key: "synthetic-unused", anthropic_base_url: "http://127.0.0.1:1/anthropic", model: "claude-sonnet-4-6" }));
          for (const team of ["france", "prussia", "russia"]) {
            const definition = buildFactionDefinition(team, "claude-sonnet-4-6");
            for (const skill of Object.values(definition.skills)) {
              if (!skill.content.includes("peer_id") || skill.content.includes("Do not call peers") || skill.content.includes("exact addresses")) throw Error("obsolete comms instructions");
            }
            await wasm.mob_create(JSON.stringify(definition));
          }
          await wasm.mob_create(JSON.stringify(buildNarratorDefinition("claude-sonnet-4-6")));
          await wasm.mob_spawn("diplomacy-france", JSON.stringify([{ profile: "planner", agent_identity: "france-planner", runtime_mode: "autonomous_host" }]));
          const peer = JSON.parse(await wasm.mob_member_peer_target("diplomacy-france", "france-planner"));
          if (typeof peer.external?.peer_id !== "string") throw Error("unexpected canonical peer target");
          sub = await wasm.mob_member_subscribe("diplomacy-france", "france-planner");
          const session = { factions: [], narratorMobId: null, subs: [{ agentIdentity: "france-planner", role: "planner", team: "france", handle: sub }],
            state: defaultState(), messages: [], running: true, seenEventIds: new Set(), summarizedRuns: new Set(), peerMembers: new Map(), prevControllers: new Map(), failures: [] };
          setSessionRef(session);
          await wasm.mob_member_send("diplomacy-france", "france-planner", JSON.stringify({ content: "Say hello.", handling_mode: "queue" }));
          let events = 0, errors = [], warnings = 0;
          const warn = console.warn; console.warn = () => { warnings++; };
          try {
            for (let i = 0; i < 150; i++) {
              await new Promise(r => setTimeout(r, 20));
              const drained = drainAllEvents(wasm, session, 1);
              events += drained.events; errors.push(...drained.errors);
              if (errors.length) break;
            }
          } finally { console.warn = warn; }
          return { version: wasm.runtime_version(), events, errors, warnings, intercepted };
        } finally {
          if (sub) wasm.close_subscription(sub);
          wasm.destroy_runtime();
          window.fetch = originalFetch;
        }
      }, repo);
      assert.equal(result.version, runtimeVersion);
      assert.ok(result.events > 0, JSON.stringify(result));
      assert.ok(result.errors.length > 0, JSON.stringify(result));
      assert.equal(result.warnings, 0);
      assert.ok(result.intercepted > 0);
    });

    await t.test("real WASM application startup, intra/cross-mob wires and successful structured events", { timeout: 45_000 }, async () => {
      const appPage = await browser.newPage();
      let requests = 0, outside = 0;
      const warnings = [];
      appPage.on("pageerror", error => warnings.push(error.message));
      appPage.on("console", message => {
        if (message.type() === "warning") warnings.push(message.text());
      });
      await appPage.route("**/*", route => {
        const url = route.request().url();
        if (!url.startsWith(origin + "/")) { outside++; return route.abort(); }
        if (url.includes("/fixtures/anthropic/")) {
          if (++requests > 80) return route.abort();
          return route.fulfill(anthropicReply(JSON.parse(route.request().postData()), requests, '{"headline":"Synthetic ready dispatch","dispatches":[]}'));
        }
        if (url.endsWith("/runtime.js")) return route.fulfill({
          contentType: "text/javascript",
          body: `import * as wasm from "/@fs/${repo}/sdks/web/wasm/meerkat_web_runtime.js";
            export * from "/@fs/${repo}/sdks/web/wasm/meerkat_web_runtime.js";
            export default async function() {
              await wasm.default({ module_or_path: "/@fs/${repo}/sdks/web/wasm/meerkat_web_runtime_bg.wasm" });
              globalThis.actualRuntime = wasm;
            }`,
        });
        if (new URL(url).pathname === "/") return route.fulfill({ contentType: "text/html", body: "<div id='app'></div>" });
        return route.continue();
      });
      const params = new URLSearchParams({ proxy: `${origin}/fixtures`, france: "claude-sonnet-4-6", prussia: "claude-sonnet-4-6", russia: "claude-sonnet-4-6", narrator: "claude-sonnet-4-6" });
      try {
        await appPage.goto(`${origin}/?${params}`);
        await appPage.evaluate(() => import("/src/main.ts"));
        await appPage.waitForFunction(() => document.getElementById("guideOverlay")?.classList.contains("hidden") === false || document.getElementById("statusBadge")?.textContent === "Error", undefined, { timeout: 40_000 }).catch(async error => {
          throw Error(`${error.message}; status=${await appPage.locator("#statusLine").textContent()}; synthetic HTTP requests=${requests}; diagnostics=${warnings.join(";")}`);
        });
        assert.notEqual(await appPage.locator("#statusBadge").textContent(), "Error", `${await appPage.locator("#statusLine").textContent()}; synthetic HTTP requests=${requests}`);
        const result = await appPage.evaluate(async () => {
          const { getSession } = await import("/src/ui.ts");
          const { drainAllEvents } = await import("/src/events.ts");
          const session = getSession(), wasm = window.actualRuntime;
          if (!session || session.subs.length !== 9 || session.peerMembers.size !== 9) throw Error("incomplete actual startup");
          for (const faction of session.factions) {
            await wasm.mob_wire(faction.mobId, `${faction.team}-planner`, `${faction.team}-operator`);
            await wasm.mob_wire(faction.mobId, `${faction.team}-planner`, `${faction.team}-ambassador`);
          }
          for (let i = 0; i < session.factions.length; i++) {
            for (const b of session.factions.slice(i + 1)) {
              const a = session.factions[i], aa = `${a.team}-ambassador`, ba = `${b.team}-ambassador`;
              await wasm.mob_wire_peer(a.mobId, aa, await wasm.mob_member_peer_target(b.mobId, ba));
              await wasm.mob_wire_peer(b.mobId, ba, await wasm.mob_member_peer_target(a.mobId, aa));
            }
          }
          for (const faction of session.factions) {
            await wasm.mob_member_send(faction.mobId, `${faction.team}-planner`, JSON.stringify({ content: "Synthetic readiness check.", handling_mode: "queue" }));
          }
          let events = 0, errors = [];
          for (let i = 0; i < 200; i++) {
            await new Promise(r => setTimeout(r, 20));
            const drained = drainAllEvents(wasm, session, 1);
            events += drained.events; errors.push(...drained.errors);
            if (session.factions.every(f => session.messages.some(m => m.faction === f.team && m.headline === "Synthetic ready dispatch"))) break;
          }
          return { version: wasm.runtime_version(), events, errors, summaries: session.messages.map(m => ({ team: m.faction, headline: m.headline })) };
        });
        assert.equal(result.version, runtimeVersion);
        assert.ok(result.events > 0);
        assert.deepEqual(result.errors, []);
        for (const team of ["france", "prussia", "russia"]) assert.ok(result.summaries.some(s => s.team === team && s.headline === "Synthetic ready dispatch"), JSON.stringify(result));
        assert.equal(warnings.filter(w => /Failed to subscribe|Cross-mob wire|malformed event/.test(w)).length, 0, warnings.join("\n"));
        assert.equal(outside, 0);
        assert.ok(requests > 0 && requests <= 80, `bounded synthetic provider requests: ${requests}`);
      } finally {
        await appPage.evaluate(() => window.actualRuntime?.destroy_runtime());
        await appPage.close();
      }
    });
    assert.equal(external, 0, "no external provider/network requests");
  } finally {
    await browser?.close(); await server.close();
    await rm(work, { recursive: true, force: true });
  }
});
